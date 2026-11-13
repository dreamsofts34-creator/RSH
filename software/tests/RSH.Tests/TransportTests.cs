using System.Net;
using System.Net.Sockets;
using RSH.Core.Protocol;
using RSH.Core.Transport;

namespace RSH.Tests;

public class TransportTests
{
    [Fact]
    public void UdpSource_ProcessDatagram_ParsesStatsAndKeepsLatest()
    {
        // Socket ke baghair wahi amal jo receive loop karta hai —
        // har mahol mein chalta hai (is VM mein UDP loopback band hai).
        using var source = new UdpWheelSource(44213);
        var p1 = new WheelPacket(1, 1000, 2000, 300, 400, 0x00, true);
        var p2 = new WheelPacket(2, 6000, 2100, 500, 600, 0x01, true);

        source.ProcessDatagram(p1.ToBytes());
        source.ProcessDatagram(p2.ToBytes());
        source.ProcessDatagram(new byte[] { 0x00, 0x01, 0x02 });   // kharab

        Assert.True(source.TryGetLatest(out var latest, out var age));
        Assert.Equal((ushort)2, latest.Sequence);
        Assert.Equal((ushort)2100, latest.Steering);
        Assert.True(age < 5000);
        Assert.Equal(2, source.Statistics.ValidPackets);
        Assert.Equal(1, source.Statistics.TooShort);
        Assert.Equal(3, source.Statistics.TotalDatagrams);
    }

    [Fact]
    public async Task UdpSource_ReceivesParsesAndKeepsLatest()
    {
        const int port = 44211;
        using var source = new UdpWheelSource(port);
        source.Start();

        using var sender = new UdpClient();
        var ep = new IPEndPoint(IPAddress.Loopback, port);

        var p1 = new WheelPacket(1, 1000, 2000, 300, 400, 0x00, true);
        var p2 = new WheelPacket(2, 6000, 2100, 500, 600, 0x01, true);
        try
        {
            await sender.SendAsync(p1.ToBytes(), WheelPacket.PacketSize, ep);
        }
        catch (SocketException ex) when (ex.SocketErrorCode == SocketError.AccessDenied)
        {
            // Is mahol mein UDP loopback hi band hai (VM policy) —
            // test maqsad nahi rakhta; Windows par ye poora chalega.
            Assert.Skip("Is mahol mein UDP loopback ki ijazat nahi.");
            throw; // unreachable — Skip hamesha throw karta hai
        }
        await sender.SendAsync(p2.ToBytes(), WheelPacket.PacketSize, ep);
        // Ek kharab packet bhi — stats mein gin'na chahiye, crash nahi
        await sender.SendAsync(new byte[] { 0x00, 0x01, 0x02 }, 3, ep);

        // Receive loop ko waqt do (loopback par aam tor par chand ms).
        // Sirf pehle packet ka intezar kafi NAHI: teeno datagrams process
        // hone tak ruko — loaded runner (2 cores, parallel tests) par
        // receive loop p2 abhi buffer mein rakh sakta hai jab pehla packet
        // mil jaye, aur ValidPackets==2 / Sequence==2 ka assert race karta
        // hai (CI run 43415259155: Expected 2, Actual 1). Test-only fix;
        // product code mein koi tabdeeli nahi.
        WheelPacket latest = default;
        long age = long.MaxValue;
        for (int i = 0; i < 100 && source.Statistics.TotalDatagrams < 3; i++)
        {
            source.TryGetLatest(out latest, out age);
            await Task.Delay(10, TestContext.Current.CancellationToken);
        }
        source.TryGetLatest(out latest, out age);

        Assert.Equal((ushort)2, latest.Sequence);          // sab se naya wala
        Assert.Equal((ushort)2100, latest.Steering);
        Assert.True(age < 5000);
        Assert.Equal(2, source.Statistics.ValidPackets);
        Assert.Equal(1, source.Statistics.TooShort);
        Assert.Equal(3, source.Statistics.TotalDatagrams);
    }

    [Fact]
    public void UdpSource_NoDataYet_ReturnsFalse()
    {
        using var source = new UdpWheelSource(44212);
        Assert.False(source.TryGetLatest(out _, out var age));
        Assert.Equal(long.MaxValue, age);
        Assert.Null(source.LastValidAgeMs);
    }

    [Fact]
    public void LinkStatistics_RateAndLoss()
    {
        var stats = new LinkStatistics();
        stats.Record(PacketParseResult.Ok, 100);
        stats.Record(PacketParseResult.Ok, 101);
        stats.Record(PacketParseResult.Ok, 104);   // 2 ghayeb
        stats.Record(PacketParseResult.BadCrc);
        Assert.Equal(4, stats.TotalDatagrams);
        Assert.Equal(3, stats.ValidPackets);
        Assert.Equal(1, stats.BadCrc);
        Assert.Equal(2, stats.LostPackets);
        Assert.Equal(40.0, stats.LossPercent, precision: 1);   // 2 / (3+2)
    }

    // ---- P8 (D8): LossPercent sliding window, last 1000 packets ----

    private static void FeedContiguous(LinkStatistics stats, int start, int count)
    {
        for (int i = 0; i < count; i++)
            stats.Record(PacketParseResult.Ok, (ushort)(start + i));
    }

    [Fact]
    public void LossPercent_NoPackets_IsZero()
    {
        var stats = new LinkStatistics();
        Assert.Equal(0.0, stats.LossPercent, precision: 1);
    }

    [Fact]
    public void LossPercent_Under1000_PartialWindowCounts()
    {
        var stats = new LinkStatistics();
        FeedContiguous(stats, 0, 10);          // 10 valid, 0 lost
        Assert.Equal(0.0, stats.LossPercent, precision: 1);
        stats.Record(PacketParseResult.Ok, 20); // gap 10..19 ghayeb
        // Window: 11 valid + 10 lost = 21 entries → 10/21 = 47.6%
        Assert.Equal(47.6, stats.LossPercent, precision: 1);
        Assert.Equal(10, stats.LostPackets);   // cumulative counter waisa hi
    }

    [Fact]
    public void LossPercent_Exactly1000_BoundaryEvictsOldest()
    {
        var stats = new LinkStatistics();
        stats.Record(PacketParseResult.Ok, 0);     // window: [V]
        stats.Record(PacketParseResult.Ok, 1000);  // 999 lost + 1 valid
        // 1001 entries hoti — sab se purani (pehli V) bahar girti hai:
        // window = 999 lost + 1 valid = 99.9%
        Assert.Equal(99.9, stats.LossPercent, precision: 1);
        stats.Record(PacketParseResult.Ok, 1001);  // ek aur valid
        // Sab se purana lost bahar gira: 998 lost + 2 valid = 99.8%
        Assert.Equal(99.8, stats.LossPercent, precision: 1);
    }

    [Fact]
    public void LossPercent_Exactly1000Valid_ThenSmallLoss()
    {
        var stats = new LinkStatistics();
        FeedContiguous(stats, 0, 1000);        // 1000 valid → 0%
        Assert.Equal(0.0, stats.LossPercent, precision: 1);
        stats.Record(PacketParseResult.Ok, 1009); // 9 ghayeb
        // Window ab bhi 1000: 991 valid + 9 lost → 0.9%
        Assert.Equal(0.9, stats.LossPercent, precision: 1);
    }

    [Fact]
    public void LossPercent_OldLossAgesOut_After1000GoodPackets()
    {
        var stats = new LinkStatistics();
        stats.Record(PacketParseResult.Ok, 0);
        stats.Record(PacketParseResult.Ok, 100); // 99 ghayeb
        Assert.Equal(98.0, stats.LossPercent, precision: 1); // 99/101
        FeedContiguous(stats, 101, 500);
        // Window 601 entries, loss abhi bhi andar: 99/601 = 16.5%
        Assert.Equal(16.5, stats.LossPercent, precision: 1);
        FeedContiguous(stats, 601, 1000);
        // 1000 naye valid packets ne purana loss poora bahar dhakel diya
        Assert.Equal(0.0, stats.LossPercent, precision: 1);
        // Cumulative counter window se mutasir nahi hota — loss hua tha
        Assert.Equal(99, stats.LostPackets);
    }

    [Fact]
    public void LossPercent_RebootDoesNotSpike()
    {
        var stats = new LinkStatistics();
        FeedContiguous(stats, 0, 100);         // 0% loss
        Assert.Equal(0.0, stats.LossPercent, precision: 1);
        stats.Record(PacketParseResult.Ok, 5); // reboot: seq 0 se shuru
        Assert.Equal(0.0, stats.LossPercent, precision: 1);
        Assert.Equal(0, stats.LostPackets);    // koi fake loss nahi
        FeedContiguous(stats, 6, 100);
        Assert.Equal(0.0, stats.LossPercent, precision: 1);
    }

    [Fact]
    public void LossPercent_DuplicatesDoNotDiluteWindow()
    {
        var stats = new LinkStatistics();
        stats.Record(PacketParseResult.Ok, 0);
        stats.Record(PacketParseResult.Ok, 10); // 9 ghayeb → 9/11 = 81.8%
        Assert.Equal(81.8, stats.LossPercent, precision: 1);
        for (int i = 0; i < 10; i++)
            stats.Record(PacketParseResult.Ok, 10); // duplicate
        Assert.Equal(10, stats.DuplicatesOrLate);
        Assert.Equal(81.8, stats.LossPercent, precision: 1); // badla nahi
        stats.Record(PacketParseResult.Ok, 11); // naya valid → 9/12
        Assert.Equal(75.0, stats.LossPercent, precision: 1);
    }

    [Fact]
    public void LossPercent_HugeGapSaturatesAtWindow()
    {
        var stats = new LinkStatistics();
        stats.Record(PacketParseResult.Ok, 0);
        stats.Record(PacketParseResult.Ok, 5000); // 4999 ghayeb
        // Window mein sirf aakhri 1000: 999 lost + 1 valid = 99.9%
        Assert.Equal(99.9, stats.LossPercent, precision: 1);
        Assert.Equal(4999, stats.LostPackets); // cumulative poora
    }

    [Fact]
    public void LossPercent_ResetSequenceBaselineClearsWindow()
    {
        var stats = new LinkStatistics();
        stats.Record(PacketParseResult.Ok, 0);
        stats.Record(PacketParseResult.Ok, 10); // 81.8%
        Assert.Equal(81.8, stats.LossPercent, precision: 1);
        stats.ResetSequenceBaseline();           // naya link/session
        Assert.Equal(0.0, stats.LossPercent, precision: 1);
        Assert.Equal(0, stats.LostPackets);
        FeedContiguous(stats, 0, 3);
        Assert.Equal(0.0, stats.LossPercent, precision: 1);
    }
}
