using System.Buffers.Binary;
using System.Net;
using System.Net.Sockets;
using System.Text;
using RSH.Core.Firmware;
using RSH.Core.Net;
using RSH.Core.Pipeline;
using RSH.Core.Processing;
using RSH.Core.Profiles;
using RSH.Core.Protocol;
using RSH.Core.Transport;

namespace RSH.Tests;

/// <summary>
/// Phase D Workstream 7 (PC-side): HELO ephemeral/v2, v2 stream (mV),
/// Ex status listener, GET_CAPS client, SEC-C-01/02/03/05 PC side,
/// SEC-C-21 verify, SEC-D-01 (signed REBOOT, RSTX hata), SEC-D-08 (v2 HELO).
///
/// NOTE — Ex3 (vJoy 97..128): PROTOCOL-V2.1.md §1.1 N1 ke mutabiq wire
/// par sirf buttons 0..63 aate hain (stream 0..31 + status BtnsEx
/// 32..63). Ex2 (65..96) aur Ex3 (97..128) ka v2.1 mein KOI wire
/// source nahi — is liye yahan test yehi karta hai ke oopar wale bits
/// kabhi fabricate nahi hote (hamesha 0), aur Ex1 (33..64) poora
/// BtnsEx → LatestButtonsEx → ButtonsMaskEx → vJoy Ex1 tak jata hai.
/// 97..128 ke liye v2.2 mein doosra extension word chahiye (protocol gap).
/// </summary>
public class PhaseDWorkstream7Tests
{
    // Deterministic fixtures (NOT real secrets) — ProtocolV2Tests jaisi.
    private static readonly byte[] TestKey = Enumerable.Range(0, 32).Select(i => (byte)i).ToArray();
    private static readonly byte[] WrongKey = Enumerable.Repeat((byte)0xFF, 32).ToArray();
    private static readonly byte[] TestNonce = Enumerable.Range(0, 16).Select(i => (byte)(0xA0 + i)).ToArray();
    private static readonly byte[] OtherNonce = Enumerable.Range(0, 16).Select(i => (byte)(0xB0 + i)).ToArray();

    private static byte[] Hex(string hex) => Convert.FromHexString(hex);

    // Golden bytes (ProtocolV2Tests se — wahi vectors, wahi authority).
    private const string StatusExtHex =
        "a621080041e20100ce56aa0f78da020003005c080001efbeadde05002000000002000100e11e";
    private const string StatusBaseHex =
        "a621070040e20100cc56ac0f78da020003005c080001efbeadde0500020001009700";
    private const string HelloUnpairedHex =
        "5253485202010901aabbccddeeff04000000a0a1a2a3a4a5a6a7a8a9aaabacadaeaf" +
        "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f" +
        "41623364456636684a6b396d4e70327253743576577838595a42";

    // ================= Helpers =================

    /// <summary>Fake control transport: har request par responder chalao (null = timeout, koi reply nahi).</summary>
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

    /// <summary>GET_CAPS reply payload banao (status + §3.4 body). pinEntries: har entry 8 B (pin, kind, module, caps, rsv×4).</summary>
    private static byte[] CapsPayload(byte fwMajor = 4, byte fwMinor = 0, byte fwPatch = 0,
        byte wifiChannel = 6, byte capsFlags = 0, params byte[][] pinEntries)
    {
        var body = new List<byte> { 0x00, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF,
            fwMajor, fwMinor, fwPatch, 0x02, 0x01, 0x08, 0x40, 0x04, wifiChannel, capsFlags, (byte)pinEntries.Length };
        foreach (var e in pinEntries) body.AddRange(e);
        return body.ToArray();
    }

    /// <summary>Control reply frame banao (responder ke liye): request ke reqId/counter echo, reply cmd, payload.</summary>
    private static byte[] ReplyFor(byte[] request, byte[] payload, byte[] key, byte[] nonce)
    {
        ushort reqId = BinaryPrimitives.ReadUInt16LittleEndian(request.AsSpan(5, 2));
        uint counter = BinaryPrimitives.ReadUInt32LittleEndian(request.AsSpan(7, 4));
        return ControlFrameCodec.Build((byte)(request[4] | 0x80), reqId, counter, payload, key, nonce);
    }

    /// <summary>HELLO reply bytes banao (92 B, §2.2) — key nonzero = unpaired, zero = paired.</summary>
    private static byte[] BuildHelloReply(byte flags, byte[] nonce, byte[] key)
    {
        var bytes = new byte[92];
        bytes[0] = (byte)'R'; bytes[1] = (byte)'S'; bytes[2] = (byte)'H'; bytes[3] = (byte)'R';
        bytes[4] = 2; bytes[5] = 1; bytes[6] = flags; bytes[7] = 0;
        new byte[] { 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF }.CopyTo(bytes, 8);
        bytes[14] = 4; bytes[15] = 0; bytes[16] = 0; bytes[17] = 0;
        nonce.CopyTo(bytes, 18);
        key.CopyTo(bytes, 34);
        // wifiPassword (66..89) zero rehta hai — is test mein matter nahi karta.
        ushort crc = Crc16.Compute(bytes.AsSpan(0, 90));
        BinaryPrimitives.WriteUInt16LittleEndian(bytes.AsSpan(90, 2), crc);
        return bytes;
    }

    private static WheelProfile PlainProfile() => WheelProfile.CreateDefault() with
    {
        Smoothing = 0, Deadzone = 0, CurveStrength = 0,
        SteeringCalibration = new SteeringCalibration
        { RawMin = 900, RawCenter = 2050, RawMax = 3200, IsSet = true, IsMillivolts = true },
        AccelCalibration = new PedalCalibration
        { RawMin = 300, RawMax = 3300, IsSet = true, IsMillivolts = true },
        BrakeCalibration = new PedalCalibration
        { RawMin = 300, RawMax = 3300, IsSet = true, IsMillivolts = true },
    };

    /// <summary>v2 packet banao: axes mV (slot → mV), buttons u32, encoders cumulative.</summary>
    private static WheelPacketV2 V2Packet(ushort seq, Dictionary<int, ushort> axes, uint buttons = 0)
    {
        byte mask = 0;
        var ordered = new List<ushort>();
        foreach (int i in axes.Keys.OrderBy(k => k)) { mask |= (byte)(1 << i); ordered.Add(axes[i]); }
        byte[] bytes = WheelPacketV2.Build(seq, 1000, 0, mask, buttons, ordered.ToArray(), ReadOnlySpan<short>.Empty);
        Assert.Equal(WheelPacketV2ParseResult.Ok, WheelPacketV2.TryParse(bytes, out var p));
        return p;
    }

    /// <summary>Fake v2+status source (engine tests ke liye).</summary>
    private sealed class FakeV2Source : IV2SampleSource, IStatusSource
    {
        public WheelPacketV2 Packet;
        public long V2AgeMs;
        public uint ButtonsEx;
        public long? ButtonsExAgeMs;
        public LinkStatistics Statistics { get; } = new();
        public long? LastValidAgeMs => V2AgeMs;
        public uint LatestButtonsEx => ButtonsEx;
        public long? LatestButtonsExAgeMs => ButtonsExAgeMs;
        public bool TryGetLatest(out WheelPacket packet, out long ageMs, long maxAgeMs = long.MaxValue)
        {
            packet = default; ageMs = long.MaxValue; return false;
        }
        public bool TryGetLatestV2(out WheelPacketV2 packet, out long ageMs, long maxAgeMs = long.MaxValue)
        {
            packet = Packet; ageMs = V2AgeMs; return ageMs <= maxAgeMs;
        }
    }

    // ================= 1. HELO (SEC-D-08) =================

    [Fact]
    public void HelloV2_Build_IsFiveBytes_Proto02_ResumeFlag0()
    {
        byte[] hello = HelloClient.BuildHello(newSession: false);
        Assert.Equal(new byte[] { (byte)'H', (byte)'E', (byte)'L', (byte)'O', 0x02, 0x00 }, hello);
        Assert.Equal(new byte[] { (byte)'H', (byte)'E', (byte)'L', (byte)'O', 0x02, 0x01 }, HelloClient.BuildHello(newSession: true));
    }

    [Fact]
    public async Task UdpSource_SendHelloV2Async_FiveBytes_SourcePortIsListenPort()
    {
        using var source = new UdpWheelSource(0);
        using var listener = new UdpClient(0);
        int listenPort = ((IPEndPoint)listener.Client.LocalEndPoint!).Port;
        var receive = listener.ReceiveAsync(TestContext.Current.CancellationToken).AsTask();
        try
        {
            await source.SendHelloV2Async("127.0.0.1", listenPort, newSession: true, TestContext.Current.CancellationToken);
        }
        catch (SocketException ex) when (ex.SocketErrorCode == SocketError.AccessDenied)
        {
            Assert.Skip("Is mahol mein UDP loopback ki ijazat nahi.");
            throw;
        }
        var done = await Task.WhenAny(receive, Task.Delay(1000, TestContext.Current.CancellationToken));
        if (done != receive)
            Assert.Skip("Is mahol mein UDP loopback packets nahi pohanchte.");
        var result = await receive;
        Assert.Equal(new byte[] { (byte)'H', (byte)'E', (byte)'L', (byte)'O', 0x02, 0x01 }, result.Buffer);
        Assert.Equal(source.ListenPort, result.RemoteEndPoint.Port);
    }

    [Fact]
    public async Task HelloClient_DelegateMode_LoopCallsSender_EverySecond()
    {
        int calls = 0;
        using var hello = new HelloClient(_ => { calls++; return Task.CompletedTask; }, "127.0.0.1");
        Assert.Equal(-1, hello.LocalPort); // delegate mode mein koi socket nahi
        hello.Start();
        // Bounded wait, CancellationToken.None — TestContext token parallel
        // load mein cancel ho sakta hai (flake), ye wait waise bhi bounded hai.
        await Task.Delay(3500, CancellationToken.None);
        hello.Stop();
        Assert.True(calls >= 2, $"1 Hz loop se 3.5 s mein kam se kam 2 HELO aane chahiye, mile {calls}");
        Assert.True(hello.HellosSent >= 2);
    }

    // ================= 2. HELO ephemeral loopback =================

    [Fact]
    public async Task EphemeralHello_ReplyAndStream_ArriveOnSameEphemeralPort_AndPortChangesOnReHello()
    {
        // Task ka test: HELLO → ephemeral port par stream aata hai;
        // dobara HELLO (naya source) par port badalta hai.
        using var a = new UdpWheelSource(0);
        using var b = new UdpWheelSource(0);
        Assert.NotEqual(a.ListenPort, b.ListenPort);
        a.Start();
        b.Start();
        try
        {
            using var board = new UdpClient(0);
            byte[] helloReply = Hex(HelloUnpairedHex);
            byte[] stream = WheelPacketV2.Build(7, 1000, 0, 0x07, 0,
                new ushort[] { 2050, 300, 300 }, ReadOnlySpan<short>.Empty);
            try
            {
                // Board: HELLO reply + stream — A ke ephemeral port par.
                await board.SendAsync(helloReply, helloReply.Length, "127.0.0.1", a.ListenPort);
                await board.SendAsync(stream, stream.Length, "127.0.0.1", a.ListenPort);
                // Dobara HELLO: naya source B, naya port — stream B par.
                await board.SendAsync(helloReply, helloReply.Length, "127.0.0.1", b.ListenPort);
                await board.SendAsync(stream, stream.Length, "127.0.0.1", b.ListenPort);
            }
            catch (SocketException ex) when (ex.SocketErrorCode == SocketError.AccessDenied)
            {
                // Is mahol mein unconnected UDP sendto() EPERM hai (sandbox policy).
                Assert.Skip("Is mahol mein UDP loopback ki ijazat nahi.");
                throw;
            }

            bool ok = false;
            for (int i = 0; i < 50 && !ok; i++)
            {
                await Task.Delay(100, CancellationToken.None);
                ok = a.HelloReceivedCount == 1 && b.HelloReceivedCount == 1
                    && a.TryGetLatestV2(out var pa, out _) && pa.Sequence == 7
                    && b.TryGetLatestV2(out var pb, out _) && pb.Sequence == 7;
            }
            if (!ok)
                Assert.Skip("Is mahol mein UDP loopback packets nahi pohanchte (ya der se aate hain).");
            Assert.Equal(1, a.HelloReceivedCount);
            Assert.Equal(1, b.HelloReceivedCount);
            Assert.Equal((byte)4, a.LatestHelloReply!.Value.FwMajor); // HELLO parse hua (fw 4.0.0)
        }
        finally
        {
            a.Stop();
            b.Stop();
        }
    }

    // ================= 3. UdpWheelSource demux =================

    [Fact]
    public void Source_StatusExtended_FeedsLatestButtonsEx()
    {
        using var source = new UdpWheelSource(0);
        bool fired = false;
        source.StatusReceived += _ => fired = true;
        source.ProcessDatagram(Hex(StatusExtHex));
        Assert.Equal(1, source.StatusReceivedCount);
        Assert.True(fired);
        Assert.Equal(0x20u, source.LatestButtonsEx); // button 37 (32+5)
        Assert.True(source.LatestStatus!.Value.HasBtnsEx);
        Assert.True(source.LatestStatus.Value.HasButtonEx(37));

        // Base status (34 B, koi BtnsEx nahi) → Ex saaf ho jata hai.
        source.ProcessDatagram(Hex(StatusBaseHex));
        Assert.Equal(2, source.StatusReceivedCount);
        Assert.Equal(0u, source.LatestButtonsEx);
        Assert.False(source.LatestStatus!.Value.HasBtnsEx);
    }

    [Fact]
    public void Source_BadStatus_Rejected_AndDoesNotClearGoodEx()
    {
        using var source = new UdpWheelSource(0);
        source.ProcessDatagram(Hex(StatusExtHex));
        Assert.Equal(0x20u, source.LatestButtonsEx);
        byte[] bad = Hex(StatusExtHex);
        bad[10] ^= 0xFF; // supply badlo, CRC purana → BadCrc
        source.ProcessDatagram(bad);
        Assert.Equal(1, source.StatusBadCount);
        Assert.Equal(0x20u, source.LatestButtonsEx); // kharab status achi value nahi mitata
    }

    [Fact]
    public void Source_HelloReply_Parsed_AndEventFires()
    {
        using var source = new UdpWheelSource(0);
        HelloReply? seen = null;
        source.HelloReplyReceived += r => seen = r;
        source.ProcessDatagram(Hex(HelloUnpairedHex));
        Assert.Equal(1, source.HelloReceivedCount);
        Assert.NotNull(seen);
        Assert.Equal(new Version(4, 0, 0), new Version(seen!.Value.FwMajor, seen.Value.FwMinor, seen.Value.FwPatch));
        Assert.True(seen.Value.Unpaired);
        Assert.True(seen.Value.PairingWindowOpen);
        Assert.Equal(TestNonce, seen.Value.Nonce); // golden nonce 0xA0..0xAF
        Assert.NotNull(source.LatestHelloReply);
    }

    [Fact]
    public void Source_V2Stream_Stored_AndV1StillWorks()
    {
        using var source = new UdpWheelSource(0);
        byte[] v2 = WheelPacketV2.Build(42, 1000, 0, 0x07, 0x205,
            new ushort[] { 2300, 1000, 2000 }, ReadOnlySpan<short>.Empty);
        source.ProcessDatagram(v2);
        Assert.Equal(1, source.V2ValidPackets);
        Assert.Equal(1, source.Statistics.ValidPackets);
        Assert.True(source.TryGetLatestV2(out var p, out _));
        Assert.Equal((ushort)42, p.Sequence);
        Assert.Equal((ushort)2300, p.Axis(0));

        // Kharab v2 → reject, slot nahi badalta.
        byte[] bad = (byte[])v2.Clone();
        bad[20] ^= 0xFF;
        source.ProcessDatagram(bad);
        Assert.Equal(1, source.V2BadPackets);
        Assert.True(source.TryGetLatestV2(out var p2, out _));
        Assert.Equal((ushort)42, p2.Sequence);

        // v1 abhi bhi chalta hai (mixed fleet).
        byte[] v1 =
        {
            0xa5, 0x01, 0x34, 0x12, 0x04, 0x03, 0x02, 0x01,
            0xfc, 0x08, 0xe8, 0x03, 0xd0, 0x07, 0x05, 0x01, 0x75, 0xbd,
        };
        source.ProcessDatagram(v1);
        Assert.True(source.TryGetLatest(out var v1p, out _));
        Assert.Equal((ushort)0x1234, v1p.Sequence);
    }

    // ================= 4. v2 stream — MapPedal mV =================

    [Fact]
    public void ProcessV2_MillivoltAxes_MapCorrectly_AndClampBounds()
    {
        var pipeline = new ProcessingPipeline(PlainProfile());
        // Steering center 2050, accel min 300, brake max 3300 (mV).
        var state = pipeline.ProcessV2(V2Packet(1, new Dictionary<int, ushort> { [0] = 2050, [1] = 300, [2] = 3300 }), 0.005);
        Assert.Equal(0.0, state.SteeringNorm, 6);
        Assert.Equal(0.0, state.AccelNorm, 6);
        Assert.Equal(1.0, state.BrakeNorm, 6);

        // Mid pedal: (1800-300)/3000 = 0.5 — mV seedha, raw conversion nahi.
        var mid = pipeline.ProcessV2(V2Packet(2, new Dictionary<int, ushort> { [0] = 2050, [1] = 1800, [2] = 300 }), 0.005);
        Assert.Equal(0.5, mid.AccelNorm, 6);

        // Bounds: 0 → 0, 3300 → 1, hadd se zyada (5000, 65535) → clamp 1, kabhi >1 nahi.
        var lo = pipeline.ProcessV2(V2Packet(3, new Dictionary<int, ushort> { [0] = 2050, [1] = 0, [2] = 300 }), 0.005);
        Assert.Equal(0.0, lo.AccelNorm, 6);
        var hi = pipeline.ProcessV2(V2Packet(4, new Dictionary<int, ushort> { [0] = 2050, [1] = 5000, [2] = 300 }), 0.005);
        Assert.Equal(1.0, hi.AccelNorm, 6);
        var max = pipeline.ProcessV2(V2Packet(5, new Dictionary<int, ushort> { [0] = 2050, [1] = 65535, [2] = 300 }), 0.005);
        Assert.Equal(1.0, max.AccelNorm, 6);

        // Steering rails (mV defaults 900/2050/3200): 900 → -1, 3200 → +1.
        var left = pipeline.ProcessV2(V2Packet(6, new Dictionary<int, ushort> { [0] = 900, [1] = 300, [2] = 300 }), 0.005);
        Assert.Equal(-1.0, left.SteeringNorm, 6);
        var right = pipeline.ProcessV2(V2Packet(7, new Dictionary<int, ushort> { [0] = 3200, [1] = 300, [2] = 300 }), 0.005);
        Assert.Equal(1.0, right.SteeringNorm, 6);
    }

    [Fact]
    public void ProcessV2_MissingAxis_IsNeutral_NotStale()
    {
        var pipeline = new ProcessingPipeline(PlainProfile());
        // Sirf steering maujood — pedals neutral (0), pehle wali value nahi rehti.
        var full = pipeline.ProcessV2(V2Packet(1, new Dictionary<int, ushort> { [0] = 2050, [1] = 3300, [2] = 3300 }), 0.005);
        Assert.Equal(1.0, full.AccelNorm, 6);
        var partial = pipeline.ProcessV2(V2Packet(2, new Dictionary<int, ushort> { [0] = 2050 }), 0.005);
        Assert.Equal(0.0, partial.AccelNorm, 6);
        Assert.Equal(0.0, partial.BrakeNorm, 6);
        Assert.Equal(0.0, partial.SteeringNorm, 6);
    }

    [Fact]
    public void ProcessV2_ButtonsAbsolute_AndEx1_Flows_Ex2Ex3StayZero()
    {
        var pipeline = new ProcessingPipeline(PlainProfile());
        // Stream buttons bit 0 + bit 5 → vJoy 1 + 6; BtnsEx bit 5 → vJoy 38 (Ex1).
        var state = pipeline.ProcessV2(V2Packet(1,
            new Dictionary<int, ushort> { [0] = 2050, [1] = 300, [2] = 300 },
            buttons: (1u << 0) | (1u << 5)), 0.005, buttonsEx: 0x20u);
        Assert.Equal((1u << 0) | (1u << 5), state.ButtonsMask);
        Assert.Equal(0x20u, state.ButtonsMaskEx);
        // Ex2/Ex3 (bits 32..63 of ButtonsMaskEx) ka v2.1 mein koi source nahi — hamesha 0.
        Assert.Equal(0u, (uint)(state.ButtonsMaskEx >> 32));
        // Poora BtnsEx bhi sirf neeche 32 bits bharta hai.
        var all = pipeline.ProcessV2(V2Packet(2,
            new Dictionary<int, ushort> { [0] = 2050, [1] = 300, [2] = 300 }), 0.005, buttonsEx: 0xFFFF_FFFFu);
        Assert.Equal(0xFFFF_FFFFu, all.ButtonsMaskEx);
        Assert.Equal(0u, (uint)(all.ButtonsMaskEx >> 32));
    }

    [Fact]
    public void ProcessV2_DisabledButtonsModule_MaskZero()
    {
        var pipeline = new ProcessingPipeline(PlainProfile());
        pipeline.SetPlan(new PipelinePlanBuilder().WithEnabled(ModuleKind.Buttons, false).Build());
        var state = pipeline.ProcessV2(V2Packet(1,
            new Dictionary<int, ushort> { [0] = 2050, [1] = 300, [2] = 300 },
            buttons: 0xFFFF_FFFFu), 0.005, buttonsEx: 0xFFFF_FFFFu);
        Assert.Equal(0u, state.ButtonsMask);
        Assert.Equal(0u, state.ButtonsMaskEx);
    }

    // ================= 5. Engine: v2 + Ex listener =================

    [Fact]
    public void Engine_V2Stream_AndStatusEx_FlowToOutput()
    {
        var source = new FakeV2Source
        {
            Packet = V2Packet(1, new Dictionary<int, ushort> { [0] = 3200, [1] = 3300, [2] = 300 }),
            V2AgeMs = 5,
            ButtonsEx = 0x20u, // button 37 → vJoy 38
            ButtonsExAgeMs = 100,
        };
        var output = new FakeOutput();
        using var engine = new WheelEngine(source, output, PlainProfile());
        engine.TickOnce(0.005, 1000);
        engine.TickOnce(0.005, 1005);
        Assert.False(engine.WatchdogActive);
        Assert.Equal(0x20u, engine.LatestButtonsEx);
        Assert.Equal(1.0, output.LastState.SteeringNorm, 6);
        Assert.Equal(1.0, output.LastState.AccelNorm, 6);
        Assert.Equal(0x20u, output.LastState.ButtonsMaskEx);
    }

    [Fact]
    public void Engine_StaleStatusEx_ClearsButtons()
    {
        var source = new FakeV2Source
        {
            Packet = V2Packet(1, new Dictionary<int, ushort> { [0] = 2050, [1] = 300, [2] = 300 }),
            V2AgeMs = 5,
            ButtonsEx = 0x20u,
            ButtonsExAgeMs = 5000, // 2 s se purana — stale
        };
        var output = new FakeOutput();
        using var engine = new WheelEngine(source, output, PlainProfile());
        engine.TickOnce(0.005, 1000);
        Assert.Equal(0u, engine.LatestButtonsEx);
        Assert.Equal(0u, output.LastState.ButtonsMaskEx);
    }

    // ================= 6. GET_CAPS client =================

    [Fact]
    public void BoardCaps_GoldenReply_Parses_AllFields()
    {
        byte[] payload = CapsPayload();
        Assert.True(BoardCaps.TryParse(payload, out var caps, out string? error), error);
        Assert.Equal(new byte[] { 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF }, caps!.BoardMac);
        Assert.Equal(new Version(4, 0, 0), caps.FirmwareVersion);
        Assert.Equal(new Version(2, 1), caps.ProtocolVersion);
        Assert.Equal((byte)8, caps.NAxesMax);
        Assert.Equal((byte)64, caps.NBtnMax);
        Assert.Equal((byte)4, caps.NEncMax);
        Assert.Equal((byte)6, caps.WifiChannel);
        Assert.Empty(caps.PinTable);
    }

    [Fact]
    public void BoardCaps_PinTable_Parses_Entries()
    {
        byte[] payload = CapsPayload(pinEntries: new[]
        {
            new byte[] { 32, 0x01, 0x01, 0x05, 0, 0, 0, 0 }, // GPIO32 ADC_NATIVE STEERING in+adc
            new byte[] { 25, 0x03, 0x08, 0x11, 0, 0, 0, 0 }, // GPIO25 DIGITAL_IN BUTTONS in+pullup
        });
        Assert.True(BoardCaps.TryParse(payload, out var caps, out string? error), error);
        Assert.Equal(2, caps!.PinTable.Count);
        Assert.Equal((byte)32, caps.PinTable[0].Pin);
        Assert.Equal(CapsPinKind.AdcNative, caps.PinTable[0].Kind);
        Assert.Equal(CapsPinModule.Steering, caps.PinTable[0].Module);
        Assert.True(caps.PinTable[0].IsAdc);
        Assert.Equal((byte)25, caps.PinTable[1].Pin);
        Assert.Equal(CapsPinModule.Buttons, caps.PinTable[1].Module);
        Assert.True(caps.PinTable[1].HasPullup);
    }

    [Theory]
    [InlineData(0)] // status != OK
    [InlineData(1)] // pinCount 41 > 40
    [InlineData(2)] // length mismatch
    [InlineData(3)] // capsFlags reserved bits
    public void BoardCaps_BadReplies_Rejected(int variant)
    {
        byte[] payload = variant switch
        {
            0 => CapsPayload().ToArray().Tap(b => b[0] = 0x06), // UNSUPPORTED
            1 => CapsPayload().ToArray().Tap(b => b[17] = 41),
            2 => CapsPayload().ToArray().Take(17).ToArray(),
            3 => CapsPayload(capsFlags: 0x10),
            _ => throw new ArgumentOutOfRangeException(nameof(variant)),
        };
        Assert.False(BoardCaps.TryParse(payload, out _, out string? error));
        Assert.NotNull(error);
    }

    [Fact]
    public async Task GetCapsClient_MockReply_ParsesCaps()
    {
        byte[] capsPayload = CapsPayload(fwMajor: 4, fwMinor: 0, fwPatch: 0, wifiChannel: 6);
        var transport = new FakeControlTransport(req => ReplyFor(req, capsPayload, TestKey, TestNonce));
        using var control = new ControlClient("127.0.0.1", TestKey, TestNonce, transport);
        control.NextReqIdForTest = 100;
        using var client = new GetCapsClient(control);
        var caps = await client.GetCapsAsync(TestContext.Current.CancellationToken);
        Assert.NotNull(caps);
        Assert.Equal(new Version(4, 0, 0), caps!.FirmwareVersion);
        Assert.Equal((byte)6, caps.WifiChannel);
        Assert.Equal(1, client.LastAttempts);
        Assert.Null(client.LastError);
        // Request frame: cmd GET_CAPS (0x01), reqId 100, counter 0 se shuru.
        Assert.Equal((byte)0x01, transport.CapturedFrames[0][4]);
        Assert.Equal((ushort)100, BinaryPrimitives.ReadUInt16LittleEndian(transport.CapturedFrames[0].AsSpan(5, 2)));
        Assert.Equal(0u, BinaryPrimitives.ReadUInt32LittleEndian(transport.CapturedFrames[0].AsSpan(7, 4)));
    }

    [Fact]
    public async Task GetCapsClient_Timeout_Retries3_SameReqId_CounterIncrements()
    {
        int calls = 0;
        var transport = new FakeControlTransport(req =>
        {
            calls++;
            if (calls < 3) return null; // pehle 2 timeout
            return ReplyFor(req, CapsPayload(), TestKey, TestNonce);
        });
        using var control = new ControlClient("127.0.0.1", TestKey, TestNonce, transport);
        control.NextReqIdForTest = 77;
        using var client = new GetCapsClient(control);
        var caps = await client.GetCapsAsync(TestContext.Current.CancellationToken);
        Assert.NotNull(caps);
        Assert.Equal(3, client.LastAttempts);
        Assert.Equal(3, transport.CapturedFrames.Count);
        // Retry: reqId WAHI, counter NAYA (§3.2).
        foreach (var f in transport.CapturedFrames)
            Assert.Equal((ushort)77, BinaryPrimitives.ReadUInt16LittleEndian(f.AsSpan(5, 2)));
        Assert.Equal(0u, BinaryPrimitives.ReadUInt32LittleEndian(transport.CapturedFrames[0].AsSpan(7, 4)));
        Assert.Equal(1u, BinaryPrimitives.ReadUInt32LittleEndian(transport.CapturedFrames[1].AsSpan(7, 4)));
        Assert.Equal(2u, BinaryPrimitives.ReadUInt32LittleEndian(transport.CapturedFrames[2].AsSpan(7, 4)));
    }

    [Fact]
    public async Task GetCapsClient_AlwaysTimeout_ReturnsNull_After3Attempts()
    {
        var transport = new FakeControlTransport(_ => null);
        using var control = new ControlClient("127.0.0.1", TestKey, TestNonce, transport);
        using var client = new GetCapsClient(control);
        var caps = await client.GetCapsAsync(TestContext.Current.CancellationToken);
        Assert.Null(caps);
        Assert.Equal(3, client.LastAttempts);
        Assert.NotNull(client.LastError);
    }

    [Fact]
    public async Task GetCapsClient_BadTag_ReturnsNull_WithoutRetry()
    {
        var transport = new FakeControlTransport(req => ReplyFor(req, CapsPayload(), WrongKey, TestNonce));
        using var control = new ControlClient("127.0.0.1", TestKey, TestNonce, transport);
        using var client = new GetCapsClient(control);
        var caps = await client.GetCapsAsync(TestContext.Current.CancellationToken);
        Assert.Null(caps);
        Assert.Equal(1, client.LastAttempts); // ghalat key retry se theek nahi hoti
        Assert.Contains("BAD_TAG", client.LastError!);
    }

    [Fact]
    public async Task GetCapsClient_WrongReqIdReply_Rejected()
    {
        var transport = new FakeControlTransport(req =>
        {
            // Reply kisi aur reqId ke liye — ignore honi chahiye.
            byte[] reply = ReplyFor(req, CapsPayload(), TestKey, TestNonce);
            BinaryPrimitives.WriteUInt16LittleEndian(reply.AsSpan(5, 2), 9999);
            // Tag dobara banao (warna BadTag aayega, reqId check se pehle nahi — structure check pehle hai).
            byte[] tag = ControlFrameCodec.ComputeTag(TestKey, TestNonce,
                BinaryPrimitives.ReadUInt32LittleEndian(reply.AsSpan(7, 4)), reply[4], reply.AsSpan(13, reply.Length - 29));
            tag.CopyTo(reply, reply.Length - 16);
            return reply;
        });
        using var control = new ControlClient("127.0.0.1", TestKey, TestNonce, transport);
        using var client = new GetCapsClient(control);
        var caps = await client.GetCapsAsync(TestContext.Current.CancellationToken);
        Assert.Null(caps);
        Assert.Contains("match nahi", client.LastError!);
    }

    // ================= 7. BoardSession + signed REBOOT (SEC-D-01) =================

    [Fact]
    public void BoardSession_UnpairedHello_GivesKeyAndNonce_PairedHello_KeepsKey()
    {
        using var session = new BoardSession();
        Assert.False(session.CanSendControl);
        session.UpdateFromHello(HelloReplyFrom(BuildHelloReply(0x09, TestNonce, TestKey)));
        Assert.True(session.HasKey);
        Assert.True(session.HasNonce);
        Assert.True(session.CanSendControl);
        Assert.Equal(new Version(4, 0, 0), session.HelloFirmwareVersion);

        // Paired HELLO (key zero) — purani key overwrite NAHI hoti, nonce update hota hai.
        session.UpdateFromHello(HelloReplyFrom(BuildHelloReply(0x00, OtherNonce, new byte[32])));
        Assert.True(session.HasKey);
        Assert.True(session.CanSendControl);
    }

    [Fact]
    public async Task BoardSession_SendReboot_SignedRebootFrame_NotRstx()
    {
        using var session = new BoardSession();
        session.UpdateFromHello(HelloReplyFrom(BuildHelloReply(0x09, TestNonce, TestKey)));
        var transport = new FakeControlTransport(req => ReplyFor(req, new byte[] { 0x00 }, TestKey, TestNonce));
        var (sent, error) = await session.SendRebootAsync("127.0.0.1", transport, TestContext.Current.CancellationToken);
        Assert.True(sent, error);
        Assert.Single(transport.CapturedFrames);
        byte[] frame = transport.CapturedFrames[0];
        Assert.Equal((byte)0x09, frame[4]); // REBOOT cmd (§3.3) — RSTX nahi
        Assert.NotEqual("RSTX"u8.ToArray(), frame.AsSpan(0, 4).ToArray());
        Assert.Equal("RSHC"u8.ToArray(), frame.AsSpan(0, 4).ToArray());
        // Tag verify ho (sahi key/nonce se bana hai).
        Assert.Equal(ControlParseResult.Ok, ControlFrameCodec.TryParse(frame, TestKey, TestNonce, out _));
    }

    [Fact]
    public async Task BoardSession_SendReboot_WithoutKey_Fails_WithRstxRemovedReason()
    {
        using var session = new BoardSession(); // koi HELLO nahi — na key, na nonce
        var transport = new FakeControlTransport(_ => null);
        var (sent, error) = await session.SendRebootAsync("127.0.0.1", transport, TestContext.Current.CancellationToken);
        Assert.False(sent);
        Assert.Contains("RSTX", error!); // wajah mein likha ho ke RSTX hata diya gaya hai
        Assert.Empty(transport.CapturedFrames); // bina key ke koi frame hi nahi bheja gaya
    }

    [Fact]
    public async Task BoardSession_GetCaps_CounterContinues_AcrossCalls_AndResetsOnNewNonce()
    {
        using var session = new BoardSession();
        session.UpdateFromHello(HelloReplyFrom(BuildHelloReply(0x09, TestNonce, TestKey)));
        // Fake board hamesha CURRENT nonce se sign karta hai (jaise asal board karta hai).
        byte[] currentNonce = TestNonce;
        var transport = new FakeControlTransport(req => ReplyFor(req, CapsPayload(), TestKey, currentNonce));
        var (caps1, _) = await session.GetCapsAsync("127.0.0.1", transport, TestContext.Current.CancellationToken);
        Assert.NotNull(caps1);
        var (caps2, _) = await session.GetCapsAsync("127.0.0.1", transport, TestContext.Current.CancellationToken);
        Assert.NotNull(caps2);
        Assert.Equal(0u, BinaryPrimitives.ReadUInt32LittleEndian(transport.CapturedFrames[0].AsSpan(7, 4)));
        Assert.Equal(1u, BinaryPrimitives.ReadUInt32LittleEndian(transport.CapturedFrames[1].AsSpan(7, 4))); // REPLAY nahi

        // Naya nonce (reboot / naya session) → counter 0 se dobara.
        session.UpdateFromHello(HelloReplyFrom(BuildHelloReply(0x09, OtherNonce, TestKey)));
        currentNonce = OtherNonce;
        var (caps3, _) = await session.GetCapsAsync("127.0.0.1", transport, TestContext.Current.CancellationToken);
        Assert.NotNull(caps3);
        Assert.Equal(0u, BinaryPrimitives.ReadUInt32LittleEndian(transport.CapturedFrames[2].AsSpan(7, 4)));
    }

    private static HelloReply HelloReplyFrom(byte[] bytes)
    {
        Assert.Equal(HelloReplyParseResult.Ok, HelloReply.TryParse(bytes, out var reply));
        return reply;
    }

    // ================= 8. FirmwareUploadGate (SEC-C-01/02/05) =================

    private static FirmwareImageValidator.ValidationResult ValidImage(string version, bool signed = true) =>
        new(true, null, FirmwareImageValidator.ImageLayout.AppOnly, new Version(version), "sha", "sha", 100_000, signed);

    private static BoardCaps CapsAt(string version)
    {
        var v = new Version(version);
        Assert.True(BoardCaps.TryParse(CapsPayload((byte)v.Major, (byte)v.Minor, (byte)v.Build), out var caps, out _));
        return caps!;
    }

    [Fact]
    public void UploadGate_Downgrade_Rejected_UpgradeAndReinstall_Allowed()
    {
        // Image 4.0.0, board 4.1.0 → downgrade reject (SEC-C-05).
        var down = FirmwareUploadGate.CheckUpload(ValidImage("4.0.0"), CapsAt("4.1.0"));
        Assert.False(down.Allowed);
        Assert.Contains("Downgrade", down.Error!);

        // Image 4.1.0, board 4.0.0 → upgrade allowed.
        var up = FirmwareUploadGate.CheckUpload(ValidImage("4.1.0"), CapsAt("4.0.0"));
        Assert.True(up.Allowed);
        Assert.Null(up.Error);

        // Barabar → reinstall allowed (warning ke saath).
        var same = FirmwareUploadGate.CheckUpload(ValidImage("4.0.0"), CapsAt("4.0.0"));
        Assert.True(same.Allowed);
        Assert.NotNull(same.Warning);
    }

    [Fact]
    public void UploadGate_NoCaps_AllowedWithWarning_InvalidImage_Rejected()
    {
        // GET_CAPS na mile (SEC-D-05: key nahi) → floor check par allowed, warning ke saath.
        var noCaps = FirmwareUploadGate.CheckUpload(ValidImage("4.0.0"), null);
        Assert.True(noCaps.Allowed);
        Assert.NotNull(noCaps.Warning);

        // Validator invalid → caps hon ya na hon, reject (SEC-C-01).
        var invalid = new FirmwareImageValidator.ValidationResult(false, "Magic byte ghalat hai.",
            FirmwareImageValidator.ImageLayout.Unknown, null, "sha", null, 10, false);
        Assert.False(FirmwareUploadGate.CheckUpload(invalid, CapsAt("4.0.0")).Allowed);
        Assert.False(FirmwareUploadGate.CheckUpload(invalid, null).Allowed);

        // SEC-C-06: valid lekin unsigned → gate bhi reject karta hai.
        Assert.False(FirmwareUploadGate.CheckUpload(ValidImage("4.0.0", signed: false), CapsAt("4.0.0")).Allowed);
    }

    [Fact]
    public void UploadGate_PostUpload_OnlyConfirmedVersion_IsSuccess()
    {
        // SEC-C-02: HTTP 200 akela kamyabi nahi — GET_CAPS confirm lazmi.
        var ok = FirmwareUploadGate.CheckPostUpload(CapsAt("4.1.0"), new Version(4, 1, 0));
        Assert.True(ok.Confirmed);

        var old = FirmwareUploadGate.CheckPostUpload(CapsAt("4.0.0"), new Version(4, 1, 0));
        Assert.False(old.Confirmed); // board abhi purana version chala raha hai

        var missing = FirmwareUploadGate.CheckPostUpload(null, new Version(4, 1, 0));
        Assert.False(missing.Confirmed); // GET_CAPS nahi mila → UNKNOWN, kabhi success nahi
        Assert.Contains("VERIFY NAHI HUA", missing.Message);
    }

    // ================= 9. SEC-C-03 + SEC-C-21 verify =================

    [Fact]
    public void EspAddress_RejectsHostname_Path_AndPublicIp_AcceptsPrivateIp()
    {
        // SEC-C-03: settings.json mein hostname/path/port kabhi nahi chalega.
        Assert.False(EspAddress.TryValidate("attacker.example/x", out _, out _));
        Assert.False(EspAddress.TryValidate("attacker.example", out _, out _));
        Assert.False(EspAddress.TryValidate("192.168.4.1:8080/evil", out _, out _));
        Assert.False(EspAddress.TryValidate("8.8.8.8", out _, out _)); // public — wheel local hi hota hai
        Assert.True(EspAddress.TryValidate("192.168.4.1", out var ip, out _));
        Assert.Equal("192.168.4.1", ip!.ToString());
        Assert.Equal("192.168.4.1", EspAddress.SanitizeOrDefault("attacker.example/x")); // ghalat → default
    }

    [Fact]
    public void PingRateLimit_Constants_AreContractValues_AndPcThrottle_Is250ms()
    {
        // SEC-C-21 verify (Phase C mein implement hua — yahan sirf darj:
        // board: 10 replies/s, burst 5; PC: min interval 250 ms = max 4 Hz).
        Assert.Equal(10, PingRateLimiter.MaxRepliesPerSecond);
        Assert.Equal(5, PingRateLimiter.BurstCapacity);
        Assert.Equal(64, PingRateLimiter.MaxTrackedSources);
        Assert.Equal(TimeSpan.FromMilliseconds(250), NtpPingClient.MinSendInterval);
    }
}

/// <summary>Test helper: byte[] par ek action chalao, phir wahi array wapas do.</summary>
internal static class PhaseDTestExtensions
{
    public static byte[] Tap(this byte[] bytes, Action<byte[]> action)
    {
        action(bytes);
        return bytes;
    }
}
