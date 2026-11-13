// Phase G — Workstream 1 (wired USB-serial transport, D26 / OPEN-20).
// C# tests: SerialFrame codec + decoder, SerialLink (loopback fake port),
// SerialWheelSource, SerialControlTransport, SerialPingTransport,
// SerialDeviceDetector. Asal USB port / board ki zaroorat nahi —
// LoopbackSerialPort do ISerialPort ko in-memory jorta hai (rig pty
// jaisa, lekin process ke andar), FakeBoardResponder board ka kirdaar
// ada karta hai (HELO→HELLO, GET_CAPS reply, ping echo).
using System.Buffers.Binary;
using System.Collections.Concurrent;
using RSH.Core.Protocol;
using RSH.Core.Transport;

namespace RSH.Tests;

/// <summary>Phase G WS1 tests — serial transport (Core level, Linux par chalte hain).</summary>
public class PhaseGWorkstream1Tests
{
    private static readonly byte[] TestKey = Enumerable.Range(0, 32).Select(i => (byte)i).ToArray();
    private static readonly byte[] TestNonce = Enumerable.Range(0, 16).Select(i => (byte)(0xA0 + i)).ToArray();

    private const string StatusExtHex =
        "a621080041e20100ce56aa0f78da020003005c080001efbeadde05002000000002000100e11e";

    private static byte[] Hex(string hex) => Convert.FromHexString(hex);

    // ================= Helpers (PhaseDWorkstream7Tests jaisi fixtures) =================

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
        ushort crc = Crc16.Compute(bytes.AsSpan(0, 90));
        BinaryPrimitives.WriteUInt16LittleEndian(bytes.AsSpan(90, 2), crc);
        return bytes;
    }

    /// <summary>GET_CAPS reply payload banao (status + §3.4 body, koi pin nahi).</summary>
    private static byte[] CapsPayload()
    {
        return new byte[] { 0x00, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF,
            4, 0, 0, 0x02, 0x01, 0x08, 0x40, 0x04, 6, 0, 0 };
    }

    /// <summary>Control reply frame banao: request ke reqId/counter echo, reply cmd, payload.</summary>
    private static byte[] ReplyFor(byte[] request, byte[] payload, byte[] key, byte[] nonce)
    {
        ushort reqId = BinaryPrimitives.ReadUInt16LittleEndian(request.AsSpan(5, 2));
        uint counter = BinaryPrimitives.ReadUInt32LittleEndian(request.AsSpan(7, 4));
        return ControlFrameCodec.Build((byte)(request[4] | 0x80), reqId, counter, payload, key, nonce);
    }

    /// <summary>v2 stream packet banao: axes mV (slot → mV), buttons u32.</summary>
    private static byte[] V2PacketBytes(ushort seq, Dictionary<int, ushort> axes, uint buttons = 0)
    {
        byte mask = 0;
        var ordered = new List<ushort>();
        foreach (int i in axes.Keys.OrderBy(k => k)) { mask |= (byte)(1 << i); ordered.Add(axes[i]); }
        byte[] bytes = WheelPacketV2.Build(seq, 1000, 0, mask, buttons, ordered.ToArray(), ReadOnlySpan<short>.Empty);
        Assert.Equal(WheelPacketV2ParseResult.Ok, WheelPacketV2.TryParse(bytes, out _));
        return bytes;
    }

    private static async Task WaitUntilAsync(Func<bool> condition, int timeoutMs = 3000, int pollMs = 10)
    {
        var deadline = DateTime.UtcNow.AddMilliseconds(timeoutMs);
        while (!condition())
        {
            if (DateTime.UtcNow > deadline) break;
            await Task.Delay(pollMs, TestContext.Current.CancellationToken).ConfigureAwait(false);
        }
        Assert.True(condition());
    }

    // ================= In-memory fake port + fake board =================

    /// <summary>
    /// In-memory ISerialPort: do port aik doosre se link ho sakte hain
    /// (A.Write → B ki inbound queue). Bina peer ke Write drop hoti hai
    /// (kamyab — koi exception nahi). InjectReceived se test seedha
    /// inbound bytes daal sakta hai (App test seam).
    /// </summary>
    internal sealed class LoopbackSerialPort : ISerialPort
    {
        private readonly ConcurrentQueue<byte> _inbound = new();
        private readonly SemaphoreSlim _available = new(0);
        private LoopbackSerialPort? _peer;
        private bool _open;

        public LoopbackSerialPort(string portName) => PortName = portName;
        public string PortName { get; }
        public bool IsOpen => _open;
        public int ReadTimeout { get; set; } = 250;
        public int WriteTimeout { get; set; } = 1000;

        public void Link(LoopbackSerialPort peer) => _peer = peer;

        public void InjectReceived(byte[] bytes) => Enqueue(bytes);

        private void Enqueue(byte[] bytes)
        {
            foreach (byte b in bytes) { _inbound.Enqueue(b); _available.Release(); }
        }

        private void Enqueue(ReadOnlySpan<byte> bytes)
        {
            foreach (byte b in bytes) { _inbound.Enqueue(b); _available.Release(); }
        }

        public void Open() => _open = true;
        public void Close() => _open = false;

        public int Read(byte[] buffer, int offset, int count)
        {
            if (!_open) return 0;
            if (!_available.Wait(TimeSpan.FromMilliseconds(ReadTimeout))) return 0;
            int n = 0;
            while (n < count && _inbound.TryDequeue(out byte b))
            {
                buffer[offset + n] = b;
                n++;
            }
            return n;
        }

        /// <summary>
        /// Cancellable blocking read (Phase H WS5, C1 fix — FakeBoardResponder
        /// ke liye): semaphore ke AvailableWaitHandle + token ke WaitHandle
        /// par WaitAny — data aaye ya cancel ho, foran jaagta hai (250 ms
        /// poll quantum nahi). Ek signal consume karke poori queued burst
        /// parhta hai; phantom signal (bytes pehle hi drain ho chuke hon)
        /// par dobara wait karta hai. Port dispose ho to
        /// ObjectDisposedException — caller loop torta hai. Port band ho
        /// (Close, dispose nahi) to 0 — caller bhi loop torta hai.
        /// </summary>
        public int ReadCancellable(byte[] buffer, int offset, int count, CancellationToken ct)
        {
            if (!_open) return 0;
            while (true)
            {
                ct.ThrowIfCancellationRequested();
                int signaled;
                try
                {
                    signaled = WaitHandle.WaitAny(new[] { _available.AvailableWaitHandle, ct.WaitHandle });
                }
                catch (ObjectDisposedException)
                {
                    ct.ThrowIfCancellationRequested();
                    throw;
                }
                if (signaled != 0)
                {
                    ct.ThrowIfCancellationRequested();
                    return 0; // na-qabil-e-rasai — signaled==1 hamesha cancel hai
                }
                // Ek signal consume karo: AvailableWaitHandle khud count
                // nahi ghataata — Wait(0) ke baghair ye hamesha signaled
                // rehta aur loop busy-spin karta.
                if (!_available.Wait(0)) continue; // race: kisi aur ne consume kiya — dobara wait
                int n = 0;
                while (n < count && _inbound.TryDequeue(out byte b))
                {
                    buffer[offset + n] = b;
                    n++;
                }
                if (n > 0) return n;
                // Phantom signal thi — dobara wait karo.
            }
        }

        public void Write(byte[] buffer, int offset, int count)
        {
            if (!_open) throw new InvalidOperationException("Port band hai.");
            _peer?.Enqueue(buffer.AsSpan(offset, count));
        }

        public void DiscardInBuffer()
        {
            while (_inbound.TryDequeue(out _)) { }
            while (_available.Wait(0)) { }
        }

        public void Dispose()
        {
            _open = false;
            _available.Dispose();
        }
    }

    /// <summary>
    /// Fake board: board-side port par frames parho, jawab do.
    /// HELO → HELLO (unpaired + window-open, TestKey/TestNonce);
    /// CONTROL GET_CAPS → CapsPayload reply; baqi control → empty reply;
    /// PING → echo. Stream/status/HELLO bhi bhej sakta hai (Send*).
    ///
    /// Phase H WS5 (C1 fix — 2 flaky tests #5 Link_HelloExchange aur
    /// #9 Link_ControlExchange_GetCaps): pehle Task.Run(RunAsync) tha —
    /// xUnit parallel load mein thread-pool starvation responder ko 2 s
    /// timeouts tak late kar deti thi, aur 250 ms poll Read + Dispose ka
    /// _task.Wait(1000) timeout par khamoshi se chhor deta tha (thread
    /// test ke baad bhi disposed port par likh sakta tha). Ab dedicated
    /// thread (SerialLink ke read loop jaisa pattern) + cancellable
    /// blocking wait (LoopbackSerialPort.ReadCancellable) + control delay
    /// WaitOne par hai; Dispose thread join ho kar hi wapas aata hai.
    /// </summary>
    internal sealed class FakeBoardResponder : IDisposable
    {
        private readonly LoopbackSerialPort _port;
        private readonly CancellationTokenSource _cts = new();
        private readonly Thread _thread;
        private readonly ManualResetEventSlim _started = new(false);
        private readonly int _controlDelayMs;
        private bool _isThreadPoolThread;

        public FakeBoardResponder(LoopbackSerialPort port, int controlDelayMs = 0)
        {
            _port = port;
            _controlDelayMs = controlDelayMs;
            _port.Open();
            _thread = new Thread(Run) { IsBackground = true, Name = "FakeBoardResponder" };
            _thread.Start();
        }

        /// <summary>Responder thread chal para hai? (Run ki pehli line set karti hai.)</summary>
        public bool Started => _started.IsSet;
        /// <summary>Responder thread ka id — dedicated thread ka saboot.</summary>
        public int ThreadId => _thread.ManagedThreadId;
        /// <summary>Responder thread zinda hai? Dispose ke baad hamesha false (Join guarantee).</summary>
        public bool IsAlive => _thread.IsAlive;
        /// <summary>Responder thread pool thread hai? Hamesha false hona chahiye (dedicated thread).</summary>
        public bool IsThreadPoolThread => _isThreadPoolThread;

        public void SendFrame(SerialFrameKind kind, byte[] payload)
        {
            byte[] frame = SerialFrameCodec.Build(kind, payload);
            _port.Write(frame, 0, frame.Length);
        }

        public void SendStream(byte[] v2Packet) => SendFrame(SerialFrameKind.Stream, v2Packet);
        public void SendStatus(byte[] statusPacket) => SendFrame(SerialFrameKind.Status, statusPacket);
        public void SendHello(byte flags = 0x09) => SendFrame(SerialFrameKind.Hello, BuildHelloReply(flags, TestNonce, TestKey));

        private void Run()
        {
            _isThreadPoolThread = Thread.CurrentThread.IsThreadPoolThread;
            _started.Set();
            var decoder = new SerialFrameDecoder();
            byte[] buf = new byte[256];
            while (!_cts.IsCancellationRequested)
            {
                int n;
                try { n = _port.ReadCancellable(buf, 0, buf.Length, _cts.Token); }
                catch (OperationCanceledException) { break; }
                catch (ObjectDisposedException) { break; } // teardown: port dispose ho gaya
                catch { break; }
                if (n <= 0) break; // port band — ReadCancellable sirf isi par 0 deta hai
                for (int i = 0; i < n; i++)
                {
                    if (!decoder.Feed(buf[i]) || decoder.LastFrame is not { } frame) continue;
                    try
                    {
                        switch (frame.Kind)
                        {
                            case SerialFrameKind.Helo:
                                SendHello();
                                break;
                            case SerialFrameKind.Control:
                                // Cancellable delay: Task.Delay + CTS ki jagah
                                // WaitOne — cancel foran jaagta hai, OCE nahi.
                                if (_controlDelayMs > 0 && _cts.Token.WaitHandle.WaitOne(_controlDelayMs)) return;
                                byte[] reply = ReplyFor(frame.Payload,
                                    frame.Payload.Length >= 5 && frame.Payload[4] == (byte)ControlCommand.GetCaps
                                        ? CapsPayload()
                                        : Array.Empty<byte>(),
                                    TestKey, TestNonce);
                                SendFrame(SerialFrameKind.Control, reply);
                                break;
                            case SerialFrameKind.Ping:
                                SendFrame(SerialFrameKind.Ping, frame.Payload);
                                break;
                        }
                    }
                    catch (ObjectDisposedException) { return; }   // teardown: peer port dispose ho gaya
                    catch (InvalidOperationException) { return; } // teardown: port band ho gaya
                }
            }
        }

        public void Dispose()
        {
            _cts.Cancel();
            // Deterministic shutdown: thread join ho kar hi Dispose wapas
            // aata hai — pehle _task.Wait(1000) timeout par khamoshi se
            // chhor deta tha.
            _thread.Join(TimeSpan.FromSeconds(2));
            _started.Dispose();
            _cts.Dispose();
        }
    }

    private static (LoopbackSerialPort Pc, LoopbackSerialPort Board) LinkedPair(string pcName = "COM-PC", string boardName = "COM-BOARD")
    {
        var pc = new LoopbackSerialPort(pcName);
        var board = new LoopbackSerialPort(boardName);
        pc.Link(board);
        board.Link(pc);
        return (pc, board);
    }

    // ================= SerialFrame codec =================

    [Fact]
    public void Codec_CanonicalVectors_ByteExact()
    {
        // §10 canonical vectors — C++ host tests + Python vectors se match.
        byte[] stream = SerialFrameCodec.Build(SerialFrameKind.Stream, new byte[] { 1, 2, 3, 4, 5, 6, 7, 8 });
        Assert.Equal(SerialFrameCodec.CanonicalVectorFrame, stream);
        byte[] helo = SerialFrameCodec.Build(SerialFrameKind.Helo, "HELO"u8.ToArray().Concat(new byte[] { 2, 0 }).ToArray());
        Assert.Equal(SerialFrameCodec.CanonicalHeloVectorFrame, helo);
        Assert.Equal(SerialFrameParseResult.Ok, SerialFrameCodec.TryParse(SerialFrameCodec.CanonicalVectorFrame, out var f1));
        Assert.NotNull(f1);
        Assert.Equal(SerialFrameKind.Stream, f1!.Kind);
        Assert.Equal(new byte[] { 1, 2, 3, 4, 5, 6, 7, 8 }, f1.Payload);
        Assert.Equal(SerialFrameParseResult.Ok, SerialFrameCodec.TryParse(SerialFrameCodec.CanonicalHeloVectorFrame, out var f2));
        Assert.NotNull(f2);
        Assert.Equal(SerialFrameKind.Helo, f2!.Kind);
    }

    [Fact]
    public void Codec_RoundTrip_AllKinds_AndLimits()
    {
        var rnd = new Random(1234);
        for (byte k = 1; k <= 7; k++)
        {
            int len = k * 7 + 3;
            byte[] payload = new byte[len];
            rnd.NextBytes(payload);
            byte[] frame = SerialFrameCodec.Build((SerialFrameKind)k, payload);
            Assert.Equal(7 + len + 2, frame.Length);
            Assert.Equal(SerialFrameParseResult.Ok, SerialFrameCodec.TryParse(frame, out var parsed));
            Assert.NotNull(parsed);
            Assert.Equal((SerialFrameKind)k, parsed!.Kind);
            Assert.Equal(payload, parsed.Payload);
        }
        // Empty payload OK; max payload 1053 OK; 1054 → ArgumentException.
        Assert.Equal(SerialFrameParseResult.Ok,
            SerialFrameCodec.TryParse(SerialFrameCodec.Build(SerialFrameKind.Probe, Array.Empty<byte>()), out var empty));
        Assert.NotNull(empty);
        Assert.Empty(empty!.Payload);
        byte[] max = new byte[SerialFrameCodec.MaxPayload];
        rnd.NextBytes(max);
        Assert.Equal(SerialFrameCodec.MaxFrame, SerialFrameCodec.Build(SerialFrameKind.Control, max).Length);
        Assert.Throws<ArgumentException>(() => SerialFrameCodec.Build(SerialFrameKind.Control, new byte[SerialFrameCodec.MaxPayload + 1]));
        Assert.Throws<ArgumentException>(() => SerialFrameCodec.Build((SerialFrameKind)0, new byte[] { 1 }));
        Assert.Throws<ArgumentException>(() => SerialFrameCodec.Build((SerialFrameKind)8, new byte[] { 1 }));
    }

    [Fact]
    public void Codec_Errors_MatchCppRules()
    {
        byte[] good = SerialFrameCodec.CanonicalVectorFrame;
        // TOO_SHORT sirf <7 B par.
        Assert.Equal(SerialFrameParseResult.TooShort, SerialFrameCodec.TryParse(Array.Empty<byte>(), out _));
        Assert.Equal(SerialFrameParseResult.TooShort, SerialFrameCodec.TryParse(good.AsSpan(0, 6).ToArray(), out _));
        // Bad magic / bad CRC.
        byte[] badMagic = good.ToArray(); badMagic[0] = (byte)'X';
        Assert.Equal(SerialFrameParseResult.BadMagic, SerialFrameCodec.TryParse(badMagic, out _));
        byte[] badCrc = good.ToArray(); badCrc[^1] ^= 0xFF;
        Assert.Equal(SerialFrameParseResult.BadCrc, SerialFrameCodec.TryParse(badCrc, out _));
        // Truncated (header poora, payload adhoora) → BadLength (B1 rule).
        Assert.Equal(SerialFrameParseResult.BadLength, SerialFrameCodec.TryParse(good.AsSpan(0, 12).ToArray(), out _));
        // Len 1054 / kind 9 / extra byte → BadLength.
        byte[] badLen = good.ToArray(); badLen[5] = 0x1E; badLen[6] = 0x04;
        Assert.Equal(SerialFrameParseResult.BadLength, SerialFrameCodec.TryParse(badLen, out _));
        byte[] badKind = good.ToArray(); badKind[4] = 9;
        Assert.Equal(SerialFrameParseResult.BadLength, SerialFrameCodec.TryParse(badKind, out _));
        byte[] extra = good.Concat(new byte[] { 0 }).ToArray();
        Assert.Equal(SerialFrameParseResult.BadLength, SerialFrameCodec.TryParse(extra, out _));
    }

    [Fact]
    public void Decoder_ByteAtATime_Chunked_Garbage_BadCrc()
    {
        byte[] frame = SerialFrameCodec.CanonicalVectorFrame;
        // Byte-at-a-time.
        var dec = new SerialFrameDecoder();
        int got = 0;
        foreach (byte b in frame)
            if (dec.Feed(b))
            {
                got++;
                Assert.Equal(SerialFrameKind.Stream, dec.LastFrame!.Kind);
                Assert.Equal(new byte[] { 1, 2, 3, 4, 5, 6, 7, 8 }, dec.LastFrame.Payload);
            }
        Assert.Equal(1, got);
        Assert.Equal(1, dec.FramesOk);
        // Chunked: aadha, phir baqi.
        dec = new SerialFrameDecoder();
        Assert.Equal(0, dec.Feed(frame.AsSpan(0, 9)));
        Assert.Equal(1, dec.Feed(frame.AsSpan(9)));
        Assert.Equal(1, dec.FramesOk);
        // Garbage (boot log) ke baad valid frame — resync.
        dec = new SerialFrameDecoder();
        Assert.Equal(0, dec.Feed("boot\r\n"u8));
        Assert.True(dec.BadMagic > 0);
        Assert.Equal(1, dec.Feed(frame));
        Assert.Equal(1, dec.FramesOk);
        // Bad CRC gira, counter barha, agla valid frame phir bhi aaya.
        byte[] badCrc = frame.ToArray(); badCrc[^1] ^= 0xFF;
        dec = new SerialFrameDecoder();
        Assert.Equal(0, dec.Feed(badCrc));
        Assert.Equal(1, dec.BadCrc);
        Assert.Equal(0, dec.FramesOk);
        Assert.Equal(1, dec.Feed(frame));
        Assert.Equal(1, dec.FramesOk);
        // Do frame peeche peeche.
        dec = new SerialFrameDecoder();
        Assert.Equal(2, dec.Feed(frame.Concat(frame).ToArray()));
        Assert.Equal(2, dec.FramesOk);
        // Oversize len header → BadLength, phir resync.
        dec = new SerialFrameDecoder();
        Assert.Equal(0, dec.Feed(new byte[] { (byte)'R', (byte)'S', (byte)'H', (byte)'S', 4, 0x1E, 0x04 }));
        Assert.Equal(1, dec.BadLength);
        Assert.Equal(1, dec.Feed(frame));
        Assert.Equal(1, dec.FramesOk);
    }

    // ================= SerialLink (loopback) =================

    [Fact]
    public async Task Link_HelloExchange_Loopback_ReturnsHello()
    {
        var (pc, board) = LinkedPair();
        using var responder = new FakeBoardResponder(board);
        using var link = new SerialLink(pc);
        link.Start();
        HelloReply? hello = await link.SendHelloAsync(newSession: true, TimeSpan.FromSeconds(2), TestContext.Current.CancellationToken);
        Assert.NotNull(hello);
        Assert.True(hello!.Value.Unpaired);
        Assert.True(hello.Value.PairingWindowOpen);
        Assert.Equal(TestNonce, hello.Value.Nonce);
        Assert.Equal(TestKey, hello.Value.DeviceKey);
        Assert.Equal(1, link.FramesOk);
        link.Stop();
    }

    [Fact]
    public async Task Link_HelloTimeout_ReturnsNull()
    {
        var (pc, board) = LinkedPair();
        board.Open(); // peer khula lekin koi responder nahi — jawab nahi aayega
        using var link = new SerialLink(pc);
        link.Start();
        HelloReply? hello = await link.SendHelloAsync(newSession: true, TimeSpan.FromMilliseconds(300), TestContext.Current.CancellationToken);
        Assert.Null(hello);
        link.Stop();
    }

    [Fact]
    public async Task Link_StreamStatusProbeEvents_Loopback()
    {
        var (pc, board) = LinkedPair();
        using var responder = new FakeBoardResponder(board);
        using var link = new SerialLink(pc);
        byte[]? gotStream = null;
        byte[]? gotStatus = null;
        byte[]? gotProbe = null;
        link.StreamFrameReceived += p => gotStream = p;
        link.StatusFrameReceived += p => gotStatus = p;
        link.ProbeFrameReceived += p => gotProbe = p;
        link.Start();
        byte[] v2 = V2PacketBytes(7, new Dictionary<int, ushort> { [0] = 1650, [1] = 1000 });
        responder.SendStream(v2);
        responder.SendStatus(Hex(StatusExtHex));
        responder.SendFrame(SerialFrameKind.Probe, Hex("52534850"));
        await WaitUntilAsync(() => gotStream is not null && gotStatus is not null && gotProbe is not null);
        Assert.Equal(v2, gotStream);
        Assert.Equal(Hex(StatusExtHex), gotStatus);
        Assert.Equal(Hex("52534850"), gotProbe);
        Assert.True(link.FramesOk >= 3);
        link.Stop();
    }

    [Fact]
    public async Task Link_BadHelloCounted_UnsolicitedControlCounted()
    {
        var (pc, board) = LinkedPair();
        using var responder = new FakeBoardResponder(board);
        using var link = new SerialLink(pc);
        link.Start();
        // Bad-CRC HELLO frame → HelloBadCount, koi event nahi.
        byte[] helloFrame = SerialFrameCodec.Build(SerialFrameKind.Hello, BuildHelloReply(0x09, TestNonce, TestKey));
        helloFrame[^1] ^= 0xFF;
        board.Write(helloFrame, 0, helloFrame.Length);
        await WaitUntilAsync(() => link.HelloBadCount == 1);
        Assert.Equal(1, link.BadCrc);
        // Control frame bina waiter ke → unsolicited.
        byte[] ctrl = SerialFrameCodec.Build(SerialFrameKind.Control, new byte[] { 1, 2, 3 });
        board.Write(ctrl, 0, ctrl.Length);
        await WaitUntilAsync(() => link.UnsolicitedControlFrames == 1);
        // HELO kind PC ki taraf (board ko jata hai, PC ko nahi) → dropped.
        byte[] helo = SerialFrameCodec.Build(SerialFrameKind.Helo, new byte[] { (byte)'H', (byte)'E', (byte)'L', (byte)'O', 2, 0 });
        board.Write(helo, 0, helo.Length);
        await WaitUntilAsync(() => link.DroppedInboundFrames == 1);
        link.Stop();
    }

    [Fact]
    public async Task Link_ControlExchange_GetCaps_Loopback()
    {
        var (pc, board) = LinkedPair();
        using var responder = new FakeBoardResponder(board);
        using var link = new SerialLink(pc);
        link.Start();
        byte[] request = ControlFrameCodec.Build((byte)ControlCommand.GetCaps, 0x1234, 5, Array.Empty<byte>(), TestKey, TestNonce);
        byte[]? reply = await link.ExchangeControlAsync(request, TimeSpan.FromSeconds(2), TestContext.Current.CancellationToken);
        Assert.NotNull(reply);
        // Structure + HMAC tag dono verify (responder ne sahi key/nonce se sign kiya).
        Assert.Equal(ControlParseResult.Ok, ControlFrameCodec.TryParse(reply!, TestKey, TestNonce, out var parsed));
        Assert.Equal((byte)((byte)ControlCommand.GetCaps | 0x80), parsed.Cmd);
        Assert.Equal(0x1234, parsed.ReqId);
        Assert.Equal(5u, parsed.Counter);
        Assert.Equal(CapsPayload(), parsed.Payload);
        link.Stop();
    }

    [Fact]
    public async Task Link_SecondControlExchangeWhileBusy_Throws()
    {
        var (pc, board) = LinkedPair();
        using var responder = new FakeBoardResponder(board, controlDelayMs: 400);
        using var link = new SerialLink(pc);
        link.Start();
        byte[] request = ControlFrameCodec.Build((byte)ControlCommand.GetCaps, 1, 1, Array.Empty<byte>(), TestKey, TestNonce);
        Task<byte[]?> first = link.ExchangeControlAsync(request, TimeSpan.FromSeconds(2), TestContext.Current.CancellationToken);
        await Assert.ThrowsAsync<InvalidOperationException>(() =>
            link.ExchangeControlAsync(request, TimeSpan.FromSeconds(2), TestContext.Current.CancellationToken));
        Assert.NotNull(await first);
        link.Stop();
    }

    [Fact]
    public async Task Link_ControlExchangeTimeout_ReturnsNull()
    {
        var (pc, board) = LinkedPair();
        board.Open(); // koi responder nahi
        using var link = new SerialLink(pc);
        link.Start();
        byte[] request = ControlFrameCodec.Build((byte)ControlCommand.GetCaps, 1, 1, Array.Empty<byte>(), TestKey, TestNonce);
        byte[]? reply = await link.ExchangeControlAsync(request, TimeSpan.FromMilliseconds(300), TestContext.Current.CancellationToken);
        Assert.Null(reply);
        link.Stop();
    }

    // ================= Transports =================

    [Fact]
    public async Task SerialControlTransport_ExchangeAsync_Loopback()
    {
        var (pc, board) = LinkedPair();
        using var responder = new FakeBoardResponder(board);
        using var link = new SerialLink(pc);
        using var transport = new SerialControlTransport(link);
        link.Start();
        byte[] request = ControlFrameCodec.Build((byte)ControlCommand.GetCaps, 0x42, 9, Array.Empty<byte>(), TestKey, TestNonce);
        byte[]? reply = await transport.ExchangeAsync(request, TimeSpan.FromSeconds(2), TestContext.Current.CancellationToken);
        Assert.NotNull(reply);
        Assert.Equal(ControlParseResult.Ok, ControlFrameCodec.TryParse(reply!, TestKey, TestNonce, out var parsed));
        Assert.Equal(0x42, parsed.ReqId);
        Assert.Equal(9u, parsed.Counter);
        Assert.Equal(CapsPayload(), parsed.Payload);
        link.Stop();
    }

    [Fact]
    public async Task SerialPingTransport_ExchangeAsync_Echoes_Loopback()
    {
        var (pc, board) = LinkedPair();
        using var responder = new FakeBoardResponder(board);
        using var link = new SerialLink(pc);
        using var transport = new SerialPingTransport(link);
        link.Start();
        // 34 B ping request (§9): "RSHN" + timestamp + key.
        byte[] request = new byte[34];
        "RSHN"u8.CopyTo(request.AsSpan(0, 4));
        BinaryPrimitives.WriteUInt64LittleEndian(request.AsSpan(4, 8), 0x1122334455667788);
        byte[]? reply = await transport.ExchangeAsync(request, TimeSpan.FromSeconds(2), TestContext.Current.CancellationToken);
        Assert.NotNull(reply);
        Assert.Equal(request, reply);
        link.Stop();
    }

    // ================= SerialWheelSource =================

    [Fact]
    public async Task SerialWheelSource_V2StreamAndStatus_Loopback()
    {
        var (pc, board) = LinkedPair();
        using var responder = new FakeBoardResponder(board);
        using var link = new SerialLink(pc);
        using var source = new SerialWheelSource(link, ownsLink: false);
        source.Start();
        responder.SendStream(V2PacketBytes(11, new Dictionary<int, ushort> { [0] = 1650, [1] = 2000 }, buttons: 0x5));
        await WaitUntilAsync(() => source.V2ValidPackets == 1);
        Assert.True(source.TryGetLatestV2(out var packet, out _));
        Assert.Equal(11, packet.Sequence);
        Assert.Equal(0x5u, packet.Buttons);
        // Status (ext, BtnsEx) → LatestButtonsEx.
        Assert.Equal(StatusPacketV2ParseResult.Ok, StatusPacketV2.TryParse(Hex(StatusExtHex), out var status));
        responder.SendStatus(Hex(StatusExtHex));
        await WaitUntilAsync(() => source.StatusReceivedCount == 1);
        Assert.Equal(status.BtnsEx, source.LatestButtonsEx);
        // HELLO → source slot + event.
        responder.SendHello();
        await WaitUntilAsync(() => source.HelloReceivedCount == 1);
        Assert.NotNull(source.LatestHelloReply);
        Assert.Equal(TestNonce, source.LatestHelloReply!.Value.Nonce);
        // Stop() link ko band NAHI karta (shared) — control ab bhi chalega.
        source.Stop();
        Assert.True(link.IsRunning);
        byte[] request = ControlFrameCodec.Build((byte)ControlCommand.GetCaps, 2, 2, Array.Empty<byte>(), TestKey, TestNonce);
        byte[]? reply = await link.ExchangeControlAsync(request, TimeSpan.FromSeconds(2), TestContext.Current.CancellationToken);
        Assert.NotNull(reply);
        link.Stop();
    }

    [Fact]
    public void SerialWheelSource_ProcessStreamPayload_InMemorySeam()
    {
        var (pc, _) = LinkedPair();
        using var link = new SerialLink(pc);
        using var source = new SerialWheelSource(link, ownsLink: false);
        // Bina port ke seedha seam: acha v2 packet → valid; kharab bytes → bad.
        source.ProcessStreamPayload(V2PacketBytes(3, new Dictionary<int, ushort> { [0] = 1650 }));
        Assert.Equal(1, source.V2ValidPackets);
        Assert.True(source.TryGetLatestV2(out var packet, out _));
        Assert.Equal(3, packet.Sequence);
        source.ProcessStreamPayload(new byte[] { (byte)'R', (byte)'S', (byte)'H', (byte)'2', 0xFF });
        Assert.Equal(1, source.V2BadPackets);
        Assert.Equal(1, source.V2ValidPackets);
    }

    // ================= Detector =================

    [Fact]
    public async Task Detector_FindsLivePort_SkipsDeadPort()
    {
        var (deadPc, deadBoard) = LinkedPair("COM-DEAD-PC", "COM-DEAD-BOARD");
        deadBoard.Open(); // khula lekin koi responder nahi
        var (livePc, liveBoard) = LinkedPair("COM-LIVE-PC", "COM-LIVE-BOARD");
        using var responder = new FakeBoardResponder(liveBoard);
        var links = new Dictionary<string, SerialLink>();
        SerialLink OpenLink(string name)
        {
            var link = new SerialLink(name == "COM-DEAD" ? deadPc : livePc);
            links[name] = link;
            return link;
        }
        try
        {
            var found = await SerialDeviceDetector.DetectAsync(
                new[] { "COM-DEAD", "COM-LIVE" }, OpenLink,
                TimeSpan.FromMilliseconds(500), TestContext.Current.CancellationToken);
            Assert.NotNull(found);
            Assert.Equal("COM-LIVE", found!.PortName);
            Assert.Equal(TestNonce, found.Hello.Nonce);
        }
        finally
        {
            foreach (var l in links.Values) l.Dispose();
            deadPc.Dispose();
            deadBoard.Dispose();
            livePc.Dispose();
            liveBoard.Dispose();
        }
    }

    [Fact]
    public async Task Detector_NoBoard_ReturnsNull()
    {
        var (pc, board) = LinkedPair();
        board.Open();
        SerialLink OpenLink(string name) => new SerialLink(pc);
        var found = await SerialDeviceDetector.DetectAsync(
            new[] { "COM-EMPTY" }, OpenLink,
            TimeSpan.FromMilliseconds(300), TestContext.Current.CancellationToken);
        Assert.Null(found);
        pc.Dispose();
        board.Dispose();
    }
}
