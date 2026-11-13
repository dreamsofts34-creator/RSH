using System.Buffers.Binary;
using RSH.Core.Processing;
using RSH.Core.Profiles;
using RSH.Core.Protocol;
using RSH.Core.Security;
using RSH.Core.Transport;

namespace RSH.Tests;

/// <summary>
/// Phase F W3 follow-up wiring (Linux par chalne wale Core-level tests):
/// BoardSession.SendControlAsync (docs/PIN-MAP-PUSH.md proposal) — RSH.App.
/// Tests (WPF/WindowsDesktop runtime) Linux sandbox mein run nahi hote
/// (Microsoft.WindowsDesktop.App 10.0.0 maujood nahi), is liye is naye
/// method ka asal execution gate yahan hai; App-side twins
/// PhaseFWorkstream3AppTests.cs mein bhi hain (Windows-PC gate par chalenge).
/// </summary>
public class PhaseFWorkstream3FollowUpTests
{
    private static readonly byte[] TestKey = Enumerable.Range(0, 32).Select(i => (byte)i).ToArray();
    private static readonly byte[] TestNonce = Enumerable.Range(0, 16).Select(i => (byte)(0xA0 + i)).ToArray();
    private static readonly byte[] TestMac = { 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF };

    private static HelloReply Hello(byte[] nonce, byte[] key) =>
        new(0x09, 0, TestMac.ToArray(), 4, 0, 0, nonce.ToArray(), key.ToArray(), new byte[24]);

    private sealed class FakeControlTransport : IControlTransport
    {
        private readonly Func<byte[], byte[]?> _responder;
        public FakeControlTransport(Func<byte[], byte[]?> responder) => _responder = responder;
        public Task<byte[]?> ExchangeAsync(byte[] frame, TimeSpan timeout, CancellationToken cancellationToken) =>
            Task.FromResult(_responder(frame));
        public void Dispose() { }
    }

    /// <summary>In-memory test double (IKeyStore) — PhaseFWorkstream1Tests jaisa.</summary>
    private sealed class FakeKeyStore : IKeyStore
    {
        public bool IsPersistent => false;
        public Dictionary<string, byte[]> Keys { get; } = new(StringComparer.OrdinalIgnoreCase);
        public Dictionary<string, string> Passwords { get; } = new(StringComparer.OrdinalIgnoreCase);
        public bool TrySaveKey(string boardId, byte[] key) { Keys[boardId] = key.ToArray(); return true; }
        public bool TryLoadKey(string boardId, out byte[]? key)
        {
            if (Keys.TryGetValue(boardId, out byte[]? k)) { key = k.ToArray(); return true; }
            key = null;
            return false;
        }
        public bool TrySavePassword(string boardId, string password) { Passwords[boardId] = password; return true; }
        public bool TryLoadPassword(string boardId, out string? password)
        {
            if (Passwords.TryGetValue(boardId, out string? p)) { password = p; return true; }
            password = null;
            return false;
        }
        public bool Delete(string boardId) { Keys.Remove(boardId); Passwords.Remove(boardId); return true; }
    }

    [Fact]
    public async Task SendControlAsync_WithoutSession_ReturnsErrorNoThrow()
    {
        using var session = new BoardSession(new FakeKeyStore()); // koi HELLO nahi
        Assert.False(session.CanSendControl);
        var result = await session.SendControlAsync("127.0.0.1", ControlCommand.SetCfg,
            ReadOnlyMemory<byte>.Empty, cancellationToken: TestContext.Current.CancellationToken);
        Assert.Null(result.ReplyPayload);
        Assert.NotNull(result.Error);
        Assert.Contains("HELLO", result.Error!, StringComparison.Ordinal);
    }

    [Fact]
    public async Task SendControlAsync_WithSession_SignsAndAdvancesCounter()
    {
        using var session = new BoardSession(new FakeKeyStore());
        session.UpdateFromHello(Hello(TestNonce, TestKey));
        Assert.True(session.CanSendControl);

        var counters = new List<uint>();
        var commands = new List<ControlCommand>();
        var transport = new FakeControlTransport(frame =>
        {
            if (ControlFrameCodec.TryParse(frame, TestKey, TestNonce, out var req) != ControlParseResult.Ok)
                return null; // bad tag — firmware khamoshi se drop karta hai
            counters.Add(req.Counter);
            commands.Add((ControlCommand)req.Cmd);
            ushort reqId = BinaryPrimitives.ReadUInt16LittleEndian(frame.AsSpan(5, 2));
            uint counter = BinaryPrimitives.ReadUInt32LittleEndian(frame.AsSpan(7, 4));
            // Reply payload: status byte Ok (+ SET_CFG jaisa 4 B body).
            byte[] body = req.Cmd == (byte)ControlCommand.SetCfg ? [0, 0, 0, 0, 0] : [(byte)ControlStatus.Ok];
            return ControlFrameCodec.Build((byte)(req.Cmd | 0x80), reqId, counter, body, TestKey, TestNonce);
        });

        var first = await session.SendControlAsync("127.0.0.1", ControlCommand.SetCfg,
            new byte[8], transport, TestContext.Current.CancellationToken);
        Assert.True(first.Succeeded, first.Error);
        Assert.Equal(ControlStatus.Ok, first.ReplyStatus);

        var second = await session.SendControlAsync("127.0.0.1", ControlCommand.Commit,
            ReadOnlyMemory<byte>.Empty, transport, TestContext.Current.CancellationToken);
        Assert.True(second.Succeeded, second.Error);

        // Har call apna ControlClient banata hai, lekin counter session ke
        // saath monotonic rehta hai (dobara 0 se shuru = REPLAY hota).
        Assert.Equal(new[] { ControlCommand.SetCfg, ControlCommand.Commit }, commands);
        Assert.Equal(2, counters.Count);
        Assert.Equal(0u, counters[0]);
        Assert.True(counters[1] > counters[0], $"Doosri call ka counter ({counters[1]}) pehli ({counters[0]}) se bara hona chahiye tha.");
    }

    [Fact]
    public async Task SendControlAsync_Disposed_Throws()
    {
        var session = new BoardSession(new FakeKeyStore());
        session.Dispose();
        await Assert.ThrowsAsync<ObjectDisposedException>(() =>
            session.SendControlAsync("127.0.0.1", ControlCommand.SetCfg, ReadOnlyMemory<byte>.Empty));
    }

    // ================= ClutchCalibration (W6 follow-up) =================

    private static WheelProfile PlainProfile(WheelProfile? profile = null) => (profile ?? WheelProfile.CreateDefault()) with
    {
        Smoothing = 0, Deadzone = 0, CurveStrength = 0,
        SteeringCalibration = new SteeringCalibration
        { RawMin = 900, RawCenter = 2050, RawMax = 3200, IsSet = true, IsMillivolts = true },
        AccelCalibration = new PedalCalibration
        { RawMin = 300, RawMax = 3300, IsSet = true, IsMillivolts = true },
        BrakeCalibration = new PedalCalibration
        { RawMin = 300, RawMax = 3300, IsSet = true, IsMillivolts = true },
    };

    private static WheelPacketV2 V2Packet(ushort seq, Dictionary<int, ushort> axes)
    {
        byte mask = 0;
        var ordered = new List<ushort>();
        foreach (int i in axes.Keys.OrderBy(k => k)) { mask |= (byte)(1 << i); ordered.Add(axes[i]); }
        byte[] bytes = WheelPacketV2.Build(seq, 1000, 0, mask, 0, ordered.ToArray(), ReadOnlySpan<short>.Empty);
        Assert.Equal(WheelPacketV2ParseResult.Ok, WheelPacketV2.TryParse(bytes, out var p));
        return p;
    }

    [Fact]
    public void ClutchCalibration_ProfilePreferred_MapsFromProfileRange()
    {
        var profile = PlainProfile(WheelProfile.CreateDefault() with
        {
            ClutchCalibration = new PedalCalibration
            { RawMin = 1000, RawMax = 2000, IsSet = true, IsMillivolts = true },
        });
        var pipeline = new ProcessingPipeline(profile);
        Assert.Equal(0, pipeline.ProcessV2(V2Packet(1, new Dictionary<int, ushort> { [3] = 1000 }), 0.005).ClutchNorm, precision: 2);
        Assert.Equal(0.5, pipeline.ProcessV2(V2Packet(2, new Dictionary<int, ushort> { [3] = 1500 }), 0.005).ClutchNorm, precision: 2);
        Assert.True(pipeline.ProcessV2(V2Packet(3, new Dictionary<int, ushort> { [3] = 2000 }), 0.005).ClutchNorm > 0.99);
    }

    [Fact]
    public void ClutchCalibration_Absent_FallsBackToDefault300To3300()
    {
        // Profile mein clutch calibration nahi — default 300..3300 mV:
        // rest (300) = 0, darmiyan (1800) = 0.5 (MapPedal ka 0..3300
        // fallback 0.545 deta — ye test farq pakarta hai).
        var pipeline = new ProcessingPipeline(PlainProfile());
        Assert.Equal(0, pipeline.ProcessV2(V2Packet(1, new Dictionary<int, ushort> { [3] = 300 }), 0.005).ClutchNorm, precision: 2);
        Assert.Equal(0.5, pipeline.ProcessV2(V2Packet(2, new Dictionary<int, ushort> { [3] = 1800 }), 0.005).ClutchNorm, precision: 2);
        Assert.True(pipeline.ProcessV2(V2Packet(3, new Dictionary<int, ushort> { [3] = 3300 }), 0.005).ClutchNorm > 0.99);
    }

    [Fact]
    public void ClutchCalibration_Insane_FallsBackToDefault_AndFlagsRecalibration()
    {
        var profile = PlainProfile(WheelProfile.CreateDefault() with
        {
            // Span 100 mV < D10 660 — insane: fallback + recalibration flag.
            ClutchCalibration = new PedalCalibration
            { RawMin = 1000, RawMax = 1100, IsSet = true, IsMillivolts = true },
        });
        Assert.True(profile.Sanitized().NeedsRecalibration);
        var pipeline = new ProcessingPipeline(profile);
        Assert.Equal(0, pipeline.ProcessV2(V2Packet(1, new Dictionary<int, ushort> { [3] = 300 }), 0.005).ClutchNorm, precision: 2);
        Assert.Equal(0.5, pipeline.ProcessV2(V2Packet(2, new Dictionary<int, ushort> { [3] = 1800 }), 0.005).ClutchNorm, precision: 2);
    }
}
