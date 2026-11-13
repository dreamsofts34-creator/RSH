using System.Buffers.Binary;
using RSH.Core.Protocol;
using RSH.Core.Transport;

namespace RSH.Tests;

/// <summary>
/// Phase H WS2 (PC-side): LED_TEST brightness + pattern wiring.
/// BoardSession.SendLedTestAsync — brightness PC-side scaling (r/g/b ko
/// brightnessPct% se zarb, §3.7 mein koi brightness field nahi), range
/// validation, aur HMAC-signed frame (cmd 0x0A, 8 B payload).
/// Fixtures PhaseDWorkstream7Tests jaisi hain (deterministic, NOT real
/// secrets) — yahan self-contained taake file akeli parhi ja sake.
/// </summary>
public class PhaseHWorkstream2LedTests
{
    private static readonly byte[] TestKey = Enumerable.Range(0, 32).Select(i => (byte)i).ToArray();
    private static readonly byte[] TestNonce = Enumerable.Range(0, 16).Select(i => (byte)(0xA0 + i)).ToArray();

    private sealed class FakeControlTransport : IControlTransport
    {
        private readonly Func<byte[], byte[]?> _responder;
        public List<byte[]> CapturedFrames { get; } = new();
        public FakeControlTransport(Func<byte[], byte[]?> responder) => _responder = responder;
        public Task<byte[]?> ExchangeAsync(byte[] frame, TimeSpan timeout, CancellationToken cancellationToken)
        {
            CapturedFrames.Add(frame.ToArray());
            return Task.FromResult(_responder(frame));
        }
        public void Dispose() { }
    }

    private static byte[] ReplyFor(byte[] request, byte[] payload, byte[] key, byte[] nonce)
    {
        ushort reqId = BinaryPrimitives.ReadUInt16LittleEndian(request.AsSpan(5, 2));
        uint counter = BinaryPrimitives.ReadUInt32LittleEndian(request.AsSpan(7, 4));
        return ControlFrameCodec.Build((byte)(request[4] | 0x80), reqId, counter, payload, key, nonce);
    }

    private static byte[] BuildHelloReply(byte flags, byte[] nonce, byte[] key)
    {
        var bytes = new byte[92];
        bytes[0] = (byte)'R'; bytes[1] = (byte)'S'; bytes[2] = (byte)'H'; bytes[3] = (byte)'R';
        bytes[4] = 2; bytes[5] = 1; bytes[6] = flags; bytes[7] = 0;
        new byte[] { 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF }.CopyTo(bytes, 8);
        bytes[14] = 4; bytes[15] = 0; bytes[16] = 0; bytes[17] = 0;
        nonce.CopyTo(bytes, 18);
        key.CopyTo(bytes, 34);
        ushort crc = Crc16.Compute(bytes.AsSpan(0, 90));
        BinaryPrimitives.WriteUInt16LittleEndian(bytes.AsSpan(90, 2), crc);
        return bytes;
    }

    private static HelloReply HelloReplyFrom(byte[] bytes)
    {
        Assert.Equal(HelloReplyParseResult.Ok, HelloReply.TryParse(bytes, out var reply));
        return reply;
    }

    private static BoardSession SessionWithKey()
    {
        var session = new BoardSession();
        session.UpdateFromHello(HelloReplyFrom(BuildHelloReply(0x09, TestNonce, TestKey)));
        Assert.True(session.CanSendControl);
        return session;
    }

    private static ControlFrame SentFrame(FakeControlTransport transport)
    {
        Assert.Single(transport.CapturedFrames);
        byte[] frame = transport.CapturedFrames[0];
        Assert.Equal(ControlParseResult.Ok, ControlFrameCodec.TryParse(frame, TestKey, TestNonce, out var parsed));
        Assert.Equal((byte)0x0A, parsed.Cmd); // LED_TEST (§3.7)
        Assert.Equal(8, parsed.Payload.Length);
        return parsed;
    }

    [Fact]
    public async Task LedTest_Brightness50_ScalesColour()
    {
        using var session = SessionWithKey();
        var transport = new FakeControlTransport(req => ReplyFor(req, new byte[] { 0x00 }, TestKey, TestNonce));
        var (sent, error) = await session.SendLedTestAsync("127.0.0.1", 1, 255, 0, 0, 1000, 0, 50, transport, TestContext.Current.CancellationToken);
        Assert.True(sent, error);
        ControlFrame parsed = SentFrame(transport);
        Assert.Equal((byte)1, parsed.Payload[0]);   // SOLID
        Assert.Equal((byte)127, parsed.Payload[1]); // 255 * 50% = 127 (integer)
        Assert.Equal((byte)0, parsed.Payload[2]);
        Assert.Equal((byte)0, parsed.Payload[3]);
        Assert.Equal((ushort)1000, BinaryPrimitives.ReadUInt16LittleEndian(parsed.Payload.AsSpan(4, 2)));
        Assert.Equal((byte)0, parsed.Payload[6]);   // repeats 0 = 5 s
        Assert.Equal((byte)0, parsed.Payload[7]);   // reserved 0
    }

    [Fact]
    public async Task LedTest_Brightness100_Unchanged_Brightness0_AllZero()
    {
        using var session = SessionWithKey();
        var transport = new FakeControlTransport(req => ReplyFor(req, new byte[] { 0x00 }, TestKey, TestNonce));
        var (sent, _) = await session.SendLedTestAsync("127.0.0.1", 1, 200, 100, 50, 1000, 0, 100, transport, TestContext.Current.CancellationToken);
        Assert.True(sent);
        ControlFrame full = SentFrame(transport);
        Assert.Equal((byte)200, full.Payload[1]);
        Assert.Equal((byte)100, full.Payload[2]);
        Assert.Equal((byte)50, full.Payload[3]);

        transport.CapturedFrames.Clear();
        (sent, _) = await session.SendLedTestAsync("127.0.0.1", 1, 200, 100, 50, 1000, 0, 0, transport, TestContext.Current.CancellationToken);
        Assert.True(sent);
        ControlFrame zero = SentFrame(transport);
        Assert.Equal((byte)0, zero.Payload[1]);
        Assert.Equal((byte)0, zero.Payload[2]);
        Assert.Equal((byte)0, zero.Payload[3]);
    }

    [Theory]
    [InlineData(-1)]
    [InlineData(101)]
    public async Task LedTest_BrightnessOutOfRange_Rejected_NoFrame(int brightness)
    {
        using var session = SessionWithKey();
        var transport = new FakeControlTransport(req => ReplyFor(req, new byte[] { 0x00 }, TestKey, TestNonce));
        var (sent, error) = await session.SendLedTestAsync("127.0.0.1", 1, 255, 0, 0, 1000, 0, brightness, transport, TestContext.Current.CancellationToken);
        Assert.False(sent);
        Assert.Contains("brightness", error!);
        Assert.Empty(transport.CapturedFrames);
    }

    [Fact]
    public async Task LedTest_UnknownPattern_Rejected_NoFrame()
    {
        using var session = SessionWithKey();
        var transport = new FakeControlTransport(req => ReplyFor(req, new byte[] { 0x00 }, TestKey, TestNonce));
        var (sent, error) = await session.SendLedTestAsync("127.0.0.1", 5, 0, 0, 0, 1000, 0, 100, transport, TestContext.Current.CancellationToken);
        Assert.False(sent);
        Assert.Contains("Unknown LED pattern", error!);
        Assert.Empty(transport.CapturedFrames);
    }

    [Fact]
    public async Task LedTest_BlinkPeriodOutOfRange_Rejected_NoFrame()
    {
        using var session = SessionWithKey();
        var transport = new FakeControlTransport(req => ReplyFor(req, new byte[] { 0x00 }, TestKey, TestNonce));
        var (sent, error) = await session.SendLedTestAsync("127.0.0.1", 2, 0, 255, 0, 50, 0, 100, transport, TestContext.Current.CancellationToken);
        Assert.False(sent);
        Assert.Contains("period", error!);
        Assert.Empty(transport.CapturedFrames);
    }

    [Fact]
    public async Task LedTest_OffPattern_SendsZeroColour()
    {
        using var session = SessionWithKey();
        var transport = new FakeControlTransport(req => ReplyFor(req, new byte[] { 0x00 }, TestKey, TestNonce));
        var (sent, error) = await session.SendLedTestAsync("127.0.0.1", 0, 0, 0, 0, 1000, 0, 100, transport, TestContext.Current.CancellationToken);
        Assert.True(sent, error);
        ControlFrame parsed = SentFrame(transport);
        Assert.Equal((byte)0, parsed.Payload[0]);
        Assert.Equal((byte)0, parsed.Payload[1]);
    }

    [Fact]
    public async Task LedTest_BoardRejects_PropagatesStatus()
    {
        using var session = SessionWithKey();
        // Board reply status UNSUPPORTED (6) — LED module configured nahi.
        var transport = new FakeControlTransport(req => ReplyFor(req, new byte[] { 0x06 }, TestKey, TestNonce));
        var (sent, error) = await session.SendLedTestAsync("127.0.0.1", 4, 0, 0, 0, 1000, 0, 100, transport, TestContext.Current.CancellationToken);
        Assert.False(sent);
        Assert.Contains("Unsupported", error!);
    }

    [Fact]
    public async Task LedTest_WithoutKey_Fails_NoFrame()
    {
        using var session = new BoardSession(); // koi HELLO nahi — na key, na nonce
        var transport = new FakeControlTransport(_ => null);
        var (sent, _) = await session.SendLedTestAsync("127.0.0.1", 4, 0, 0, 0, 1000, 0, 100, transport, TestContext.Current.CancellationToken);
        Assert.False(sent);
        Assert.Empty(transport.CapturedFrames);
    }
}
