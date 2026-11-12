using System.Text;
using ApexLink.Core.Protocol;

namespace ApexLink.Tests;

public class ProtocolTests
{
    // Firmware ke host tests wala GOLDEN packet — dono taraf ek jaisa
    // parse hona hi firmware↔software contract ka saboot hai.
    private static readonly byte[] Golden =
    {
        0xa5, 0x01, 0x34, 0x12, 0x04, 0x03, 0x02, 0x01,
        0xfc, 0x08, 0xe8, 0x03, 0xd0, 0x07, 0x05, 0x01, 0x75, 0xbd,
    };

    [Fact]
    public void Crc_StandardCheckVector()
    {
        Assert.Equal((ushort)0x29B1, Crc16.Compute(Encoding.ASCII.GetBytes("123456789")));
        Assert.Equal((ushort)0xFFFF, Crc16.Compute(ReadOnlySpan<byte>.Empty));
    }

    [Fact]
    public void GoldenPacket_ParsesExactlyLikeFirmware()
    {
        var result = WheelPacket.TryParse(Golden, out var p);
        Assert.Equal(PacketParseResult.Ok, result);
        Assert.Equal((ushort)0x1234, p.Sequence);
        Assert.Equal(0x01020304u, p.EspTimestampUs);
        Assert.Equal((ushort)2300, p.Steering);
        Assert.Equal((ushort)1000, p.Accel);
        Assert.Equal((ushort)2000, p.Brake);
        Assert.Equal((byte)0x05, p.Buttons);
        Assert.True(p.GearUp);
        Assert.False(p.GearDown);
        Assert.True(p.AccelButton);
        Assert.False(p.BrakeButton);
        Assert.True(p.SteeringInMillivolts);
    }

    [Fact]
    public void Build_ThenParse_Roundtrips_AndMatchesGolden()
    {
        var original = new WheelPacket(0x1234, 0x01020304, 2300, 1000, 2000, 0x05, true);
        byte[] bytes = original.ToBytes();
        Assert.Equal(Golden, bytes);   // builder firmware ke bytes hi banata hai

        var result = WheelPacket.TryParse(bytes, out var parsed);
        Assert.Equal(PacketParseResult.Ok, result);
        Assert.Equal(original, parsed);
    }

    [Fact]
    public void Parse_RejectsBadPackets_WithRightReason()
    {
        Assert.Equal(PacketParseResult.TooShort, WheelPacket.TryParse(Golden.AsSpan(0, 10), out _));

        var badMagic = (byte[])Golden.Clone(); badMagic[0] = 0xA6;
        Assert.Equal(PacketParseResult.BadMagic, WheelPacket.TryParse(badMagic, out _));

        var badVersion = (byte[])Golden.Clone(); badVersion[1] = 0x02;
        Assert.Equal(PacketParseResult.BadVersion, WheelPacket.TryParse(badVersion, out _));

        var badCrc = (byte[])Golden.Clone(); badCrc[9] ^= 0xFF;   // payload badla, CRC purana
        Assert.Equal(PacketParseResult.BadCrc, WheelPacket.TryParse(badCrc, out _));
    }

    [Fact]
    public void SequenceTracker_CountsLossAndDuplicates()
    {
        var tracker = new SequenceTracker();
        tracker.Observe(10);                       // baseline
        var step = tracker.Observe(11);
        Assert.Equal(0, step.Lost);
        step = tracker.Observe(14);                // 12, 13 ghayeb
        Assert.Equal(2, step.Lost);
        step = tracker.Observe(14);                // duplicate
        Assert.True(step.IsDuplicateOrLate);
        step = tracker.Observe(13);                // late aya hua purana
        Assert.True(step.IsDuplicateOrLate);
        Assert.Equal(2, tracker.TotalLost);
        Assert.Equal(2, tracker.TotalDuplicatesOrLate);
    }

    [Fact]
    public void SequenceTracker_HandlesWrap()
    {
        var tracker = new SequenceTracker();
        tracker.Observe(65534);
        tracker.Observe(65535);
        var step = tracker.Observe(1);             // 0 ghayeb, wrap ke paar
        Assert.Equal(1, step.Lost);
        Assert.False(step.IsDuplicateOrLate);
    }
}
