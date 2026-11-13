using System.Net.Sockets;
using RSH.Core.Protocol;
using RSH.Core.Transport;

namespace RSH.Tests;

/// <summary>P7 — UdpWheelSource Start/Stop lifecycle + socket-error counting.
/// Port 0 (ephemeral) — doosre agents ke concurrent test runs se
/// fixed-port takrao (Address already in use) nahi hota; ye tests
/// UDP bhejte nahi, ProcessDatagram se seedha chalate hain.</summary>
public class UdpWheelSourceLifecycleTests
{
    [Fact]
    public async Task ConcurrentStopStart_NoExceptionOrDeadlock()
    {
        using var source = new UdpWheelSource(0);
        source.Start();

        // 30 s mein khatam na ho to deadlock — WaitAsync timeout par throw karta hai.
        await Task.Run(() =>
        {
            var actions = Enumerable.Range(0, 30)
                .SelectMany(_ => new Action[] { () => source.Start(), () => source.Stop() })
                .ToArray();
            Parallel.ForEach(actions, a => a());
        }, TestContext.Current.CancellationToken)
            .WaitAsync(TimeSpan.FromSeconds(30), TestContext.Current.CancellationToken);
        // Race ke baad source dobara start ho aur kaam kare.
        source.Start();
        Assert.True(source.IsRunning);
        source.ProcessDatagram(new WheelPacket(1, 1000, 2000, 300, 400, 0x00, true).ToBytes());
        Assert.True(source.TryGetLatest(out var latest, out _));
        Assert.Equal((ushort)1, latest.Sequence);
        source.Stop();
        Assert.False(source.IsRunning);
    }

    [Fact]
    public void Start_IsIdempotent_StopBeforeStart_IsSafe()
    {
        using var source = new UdpWheelSource(0);
        Assert.False(source.IsRunning);
        source.Stop();   // Start se pehle Stop — crash nahi
        Assert.False(source.IsRunning);

        source.Start();
        source.Start();  // do dafa Start — doosra loop nahi
        Assert.True(source.IsRunning);
        source.Stop();
        source.Stop();   // do dafa Stop — mehfooz
        Assert.False(source.IsRunning);
    }

    [Fact]
    public void StopThenStart_RestartsAndStillReceives()
    {
        using var source = new UdpWheelSource(0);
        source.Start();
        source.ProcessDatagram(new WheelPacket(1, 1000, 2000, 300, 400, 0x00, true).ToBytes());
        source.Stop();
        Assert.False(source.IsRunning);

        source.Start();
        Assert.True(source.IsRunning);
        source.ProcessDatagram(new WheelPacket(2, 6000, 2100, 500, 600, 0x01, true).ToBytes());
        Assert.True(source.TryGetLatest(out var latest, out _));
        Assert.Equal((ushort)2, latest.Sequence);
        source.Stop();
    }

    [Fact]
    public void Dispose_ThenStart_DoesNotRestart()
    {
        var source = new UdpWheelSource(0);
        source.Start();
        source.Dispose();
        source.Start();   // Dispose ke baad restart nahi hona chahiye
        Assert.False(source.IsRunning);
        source.Dispose(); // do dafa Dispose — mehfooz
    }

    [Fact]
    public void SocketErrorCounters_StartAtZero()
    {
        using var source = new UdpWheelSource(0);
        Assert.Equal(0, source.SocketErrorCount);
        Assert.Equal(SocketError.Success, source.LastSocketErrorCode);
        Assert.Null(source.LastSocketErrorMessage);
    }
}
