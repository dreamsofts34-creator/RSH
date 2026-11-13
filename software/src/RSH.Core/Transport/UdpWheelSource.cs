using System.Diagnostics;
using System.Net;
using System.Net.Sockets;
using System.Text;
using RSH.Core.Protocol;

namespace RSH.Core.Transport;

/// <summary>
/// v2.1 stream source (IV2SampleSource): WheelPacketV2 packets
/// (sab axes mV, D6) ka taaza slot. UdpWheelSource implement karta hai.
/// </summary>
public interface IV2SampleSource : ISampleSource
{
    bool TryGetLatestV2(out WheelPacketV2 packet, out long ageMs, long maxAgeMs = long.MaxValue);
}

/// <summary>
/// Status source (IStatusSource): 1 Hz status packet se BtnsEx word
/// (buttons 32..63 → vJoy 33..64, Ex1). UdpWheelSource implement karta
/// hai; WheelEngine har tick yahan se LatestButtonsEx leta hai (Ex listener).
/// </summary>
public interface IStatusSource
{
    /// <summary>Taaza StatusPacketV2.BtnsEx word (0 jab koi extended status na aaya ho).</summary>
    uint LatestButtonsEx { get; }
    /// <summary>Aakhri valid status packet ko kitni der hui (kabhi na aya ho to null).</summary>
    long? LatestButtonsExAgeMs { get; }
}

/// <summary>
/// ESP se data lene wala hissa: UDP par sunta hai, har datagram
/// parse karta hai, aur sirf SAB SE NAYA valid packet rakhta hai
/// (latest-sample slot — purana kaam ka nahi, queue kabhi barhti nahi).
/// Pipeline is se TryGetLatest se padhta hai jab chahe.
///
/// Demux (PacketRouter.Classify, PROTOCOL-V2.1.md §1.4/§2):
/// v1 stream (0xA5 0x01) → v1 slot; v2.1 stream ("RSH2") → v2 slot
/// (sab axes mV, D6); status (0xA6, 34/38 B) → LatestStatus +
/// LatestButtonsEx (BtnsEx, Ex1 — 33..64); HELLO reply ("RSHR", 92 B)
/// → LatestHelloReply + HelloReplyReceived event. Control/probe/ping
/// is port par nahi aane chahiye — sirf ginay jate hain. Koi bhi
/// qism engine ko nahi torti: ghalat datagram drop + count hota hai.
///
/// (T10) Default listen port ephemeral hai (0 — OS deta hai); board
/// ko ye port HELO ke source port se pata chalta hai
/// (<see cref="SendHelloV2Async"/> — HELO isi socket se jata hai;
/// Phase D firmware us remotePort par data bhejega). Jab tak board
/// fixed 4211 par bhejta hai (firmware v4.0.0, W3 ka remotePort kaam
/// baaqi hai), explicit port dena parta hai, dekho
/// <see cref="LegacyFirmwareDataPort"/>.
/// </summary>
public sealed class UdpWheelSource : ISampleSource, IV2SampleSource, IStatusSource, IWheelSource, IDisposable
{
    /// <summary>
    /// Default listen port (T10): 0 = ephemeral — OS khaali port deta
    /// hai, taake doosri instance / doosri app se takrao ("Address
    /// already in use") na ho. Asal port <see cref="ListenPort"/> se
    /// milta hai. Purana fixed 4211 sirf tab istemal karo jab peer
    /// (board v4.0.0 firmware / simulator) abhi bhi fixed port par
    /// bhejta ho — explicit <c>new UdpWheelSource(4211)</c> se.
    /// </summary>
    public const int DefaultListenPort = 0;

    /// <summary>
    /// Legacy firmware data port (V-C1): firmware v4.0.0 data FIXED is
    /// port par bhejta hai (config.h PORT_DATA 4211, net.cpp
    /// udpData.beginPacket(pcIp, PORT_DATA)) — HELO ke source port ko
    /// ignore karta hai. Phase D firmware (W3) HELO ke remotePort par
    /// data bhejega — tab ephemeral default hi sahi rasta hai.
    /// </summary>
    public const int LegacyFirmwareDataPort = 4211;

    /// <summary>
    /// Receive (SO_RCVBUF) buffer — 256 KB. OS default (Windows par
    /// 8 KB) par chhoti si rukawat (GC/scheduling) se bhi kernel
    /// datagram silently gira deta tha; 256 KB 200 Hz par 30 s se
    /// zyada ka backlog sambhalta hai. Na mil sake to 64 KB try
    /// hota hai, phir OS default rehta hai.
    /// </summary>
    public const int DefaultReceiveBufferSizeBytes = 256 * 1024;

    /// <summary>Receive buffer ka fallback size (jab 256 KB na mile).</summary>
    public const int MinimumReceiveBufferSizeBytes = 64 * 1024;

    private readonly UdpClient _udp;
    private static readonly byte[] HelloBytes = Encoding.ASCII.GetBytes("HELO");
    private readonly LinkStatistics _stats = new();
    private readonly object _slotLock = new();
    private WheelPacket _latest;
    private bool _hasLatest;
    private long _lastValidMs;
    private WheelPacketV2 _latestV2;
    private bool _hasLatestV2;
    private long _lastValidV2Ms;
    private StatusPacketV2? _latestStatus;
    private uint _latestButtonsEx;
    private long _lastStatusMs;
    private HelloReply? _latestHello;
    private IPEndPoint? _lastHelloRemote;
    private long _statusReceived;
    private long _statusBad;
    private long _helloReceived;
    private long _helloBad;
    private long _v2Valid;
    private long _v2Bad;
    private long _unknownDatagrams;

    // P7: Start/Stop/Dispose ki lifecycle ek hi lock se — concurrent
    // Start() pehle do receive loop bana deta tha, aur Stop() ke doran
    // Start() aadhi bani halat (_cts null, _receiveTask purana) dekh
    // sakta tha. Receive loop ye lock kabhi nahi leta, is liye Stop()
    // ka task ka intezar yahan deadlock nahi karta.
    private readonly object _lifecycleLock = new();
    private CancellationTokenSource? _cts;
    private Task? _receiveTask;
    private bool _disposed;

    // P7: socket errors silent nahi — counter + aakhri error hamesha
    // padhne ke liye maujood (UI/diagnostics). Core mein koi logger
    // nahi hai (Serilog sirf RSH.App mein hai), is liye Trace bhi likha
    // jata hai — naya logging framework nahi banaya gaya.
    private long _socketErrorCount;
    private SocketError _lastSocketErrorCode;
    private string? _lastSocketErrorMessage;

    /// <summary>Valid HELLO reply aaya — BoardSession is se nonce/key leta hai (pairing/session).</summary>
    public event Action<HelloReply>? HelloReplyReceived;
    /// <summary>Valid status packet aaya — diagnostics / BtnsEx listener.</summary>
    public event Action<StatusPacketV2>? StatusReceived;

    public UdpWheelSource(int listenPort = DefaultListenPort)
    {
        if (listenPort < 0 || listenPort > 65535)
            throw new ArgumentOutOfRangeException(nameof(listenPort), "Port 0..65535 ke andar ho (0 = ephemeral).");
        _udp = new UdpClient(listenPort);
        // P7 follow-up (Phase C): receive buffer explicit — OS default
        // par kernel backlog bharte hi datagram silently girata hai
        // (koi counter/API nahi batata), is liye buffer bara aur
        // observable rakho. Set na ho sake to chhota try karo; aakhir
        // mein OS default par bhi socket kaam karta hai.
        ReceiveBufferSizeBytes = TrySetReceiveBufferSize(DefaultReceiveBufferSizeBytes)
            ?? TrySetReceiveBufferSize(MinimumReceiveBufferSizeBytes)
            ?? _udp.Client.ReceiveBufferSize;
    }

    /// <summary>Asal bound listen port (ephemeral ho to OS-assigned).</summary>
    public int ListenPort => ((IPEndPoint)_udp.Client.LocalEndPoint!).Port;

    /// <summary>UI/diagnostics ke liye: "UDP port 4211".</summary>
    public string SourceDescription => $"UDP port {ListenPort}";

    /// <summary>
    /// Asal receive buffer size (bytes) — OS set ki hui value ko
    /// adjust kar sakta hai (Linux read-back par dugna dikhata hai),
    /// is liye set karne ke baad parhi hui qeemat hai, mangi hui nahi.
    /// </summary>
    public int ReceiveBufferSizeBytes { get; }

    /// <summary>Receive buffer set karo; kamyab ho to read-back value, warna null.</summary>
    private int? TrySetReceiveBufferSize(int bytes)
    {
        try
        {
            _udp.Client.ReceiveBufferSize = bytes;
            return _udp.Client.ReceiveBufferSize;
        }
        catch (SocketException)
        {
            return null;
        }
        catch (ObjectDisposedException)
        {
            return null;
        }
    }

    public LinkStatistics Statistics => _stats;

    /// <summary>Receive loop chal raha hai?</summary>
    public bool IsRunning
    {
        get { lock (_lifecycleLock) return _receiveTask is { IsCompleted: false }; }
    }

    /// <summary>Ab tak kitni socket errors aain (receive loop mein).</summary>
    public long SocketErrorCount => Interlocked.Read(ref _socketErrorCount);

    /// <summary>Aakhri socket error ka code (koi error na ho to Success).</summary>
    public SocketError LastSocketErrorCode => _lastSocketErrorCode;

    /// <summary>Aakhri socket error ka message (koi error na ho to null).</summary>
    public string? LastSocketErrorMessage => _lastSocketErrorMessage;

    /// <summary>Taaza valid status packet (null jab abhi na aaya ho) — BtnsEx isi se aata hai.</summary>
    public StatusPacketV2? LatestStatus { get { lock (_slotLock) return _latestStatus; } }

    /// <summary>Taaza StatusPacketV2.BtnsEx word (buttons 32..63 → vJoy 33..64, Ex1).</summary>
    public uint LatestButtonsEx { get { lock (_slotLock) return _latestButtonsEx; } }

    /// <summary>Aakhri valid status packet ko kitni der hui (kabhi na aya ho to null).</summary>
    public long? LatestButtonsExAgeMs
    {
        get { lock (_slotLock) return _latestStatus is null ? null : Environment.TickCount64 - _lastStatusMs; }
    }

    /// <summary>Ab tak kitne valid status packets aaye.</summary>
    public long StatusReceivedCount => Interlocked.Read(ref _statusReceived);
    /// <summary>Ab tak kitne status packets reject hue (ghalat magic/version/length/CRC).</summary>
    public long StatusBadCount => Interlocked.Read(ref _statusBad);
    /// <summary>Taaza valid HELLO reply (null jab abhi na aaya ho).</summary>
    public HelloReply? LatestHelloReply { get { lock (_slotLock) return _latestHello; } }
    /// <summary>Aakhri HELLO reply kis endpoint se aaya (us ka source port = board ka data destination nahi — HELO ka source port hai).</summary>
    public IPEndPoint? LastHelloRemoteEndPoint { get { lock (_slotLock) return _lastHelloRemote; } }
    /// <summary>Ab tak kitne valid HELLO replies aaye.</summary>
    public long HelloReceivedCount => Interlocked.Read(ref _helloReceived);
    /// <summary>Ab tak kitne HELLO replies reject hue.</summary>
    public long HelloBadCount => Interlocked.Read(ref _helloBad);
    /// <summary>Ab tak kitne valid v2.1 stream packets aaye.</summary>
    public long V2ValidPackets => Interlocked.Read(ref _v2Valid);
    /// <summary>Ab tak kitne v2.1 stream packets reject hue.</summary>
    public long V2BadPackets => Interlocked.Read(ref _v2Bad);
    /// <summary>Is (data) port par aane wale control/probe/ping datagrams — sirf ginay jate hain.</summary>
    public long UnknownDatagrams => Interlocked.Read(ref _unknownDatagrams);

    /// <summary>
    /// "HELO" (legacy 4-byte, v1) isi listen socket se bhejo: source
    /// port = listen port. Sirf legacy compat ke liye — naya code
    /// <see cref="SendHelloV2Async"/> istemal kare (SEC-D-08: proto +
    /// sessionFlag lazmi hain, warna firmware hijack kar sakta hai).
    /// </summary>
    public async Task SendHelloAsync(string espIp, int espPort, CancellationToken token = default)
    {
        if (espPort is < 1 or > 65535)
            throw new ArgumentOutOfRangeException(nameof(espPort), "Board ka HELO port 1..65535 ke andar ho.");
        await _udp.SendAsync(HelloBytes.AsMemory(), espIp, espPort, token).ConfigureAwait(false);
    }

    /// <summary>
    /// v2 HELO (5 B, §2.1: "HELO" + proto 0x02 + sessionFlag) isi
    /// listen socket se bhejo (SEC-D-08): source port = listen port,
    /// taake board (Phase D firmware, jo HELO ke remotePort par
    /// data bhejega) data usi ephemeral port par bheje. Alag
    /// <see cref="HelloClient"/> socket is port par bind nahi ho
    /// sakta — aik hi UDP port par do sockets nahi bandhte
    /// ("Address already in use") — is liye jab UdpWheelSource chal
    /// raha ho to HELO yahin se jata hai, aur HelloClient sirf
    /// standalone (baghair data socket ke) ya delegate mode mein
    /// istemal hota hai. <paramref name="newSession"/> true = naya
    /// session (naya nonce, counter 0 se); false = resume.
    /// </summary>
    public async Task SendHelloV2Async(string espIp, int espPort, bool newSession = false, CancellationToken token = default)
    {
        if (espPort is < 1 or > 65535)
            throw new ArgumentOutOfRangeException(nameof(espPort), "Board ka HELO port 1..65535 ke andar ho.");
        byte[] hello = HelloClient.BuildHello(newSession);
        await _udp.SendAsync(hello.AsMemory(), espIp, espPort, token).ConfigureAwait(false);
    }

    /// <summary>Receive loop background mein shuru karo (pehle se chal raha ho to kuch nahi).</summary>
    public void Start()
    {
        lock (_lifecycleLock)
        {
            if (_disposed) return;
            if (_receiveTask is { IsCompleted: false }) return;
            _cts = new CancellationTokenSource();
            _receiveTask = Task.Run(() => ReceiveLoopAsync(_cts.Token));
        }
    }

    /// <summary>
    /// Receive loop roko. Idempotent — Start() se pehle ya do dafa
    /// bulana mehfooz hai. Rukne ke baad Start() dobara chal sakta hai.
    /// Task ke khatam hone ka intezar lock se BAHAR hota hai, warna
    /// concurrent Stop() calls lock par qataar bana kar har ek 500 ms
    /// tak rukti (race test mein 10 s+).
    /// </summary>
    public void Stop()
    {
        Task? task;
        CancellationTokenSource? cts;
        lock (_lifecycleLock)
        {
            (task, cts) = TakeLocked();
        }
        WaitForStop(task, cts);
    }

    /// <summary>
    /// Loop + CTS fields se nikalo aur null karo. Sirf _lifecycleLock ke andar se bulana.
    /// </summary>
    private (Task? Task, CancellationTokenSource? Cts) TakeLocked()
    {
        var task = _receiveTask;
        var cts = _cts;
        _receiveTask = null;
        _cts = null;
        return (task, cts);
    }

    /// <summary>Loop ko cancel karo aur khatam hone ka intezar karo (max 500 ms). Lock ke bahar.</summary>
    private static void WaitForStop(Task? task, CancellationTokenSource? cts)
    {
        if (task is null)
        {
            // Task ke baghair CTS nahi hona chahiye, lekin ho to bhi saaf karo.
            try { cts?.Dispose(); } catch (ObjectDisposedException) { }
            return;
        }
        try { cts?.Cancel(); } catch (ObjectDisposedException) { }
        try
        {
            // Apne hi loop thread se Stop() aye to Wait deadlock hoga — us soorat mein chhoro.
            if (Task.CurrentId != task.Id) task.Wait(500);
        }
        catch { /* band hote waqt khamoshi behtareen */ }
        try { cts?.Dispose(); } catch (ObjectDisposedException) { }
    }

    private async Task ReceiveLoopAsync(CancellationToken token)
    {
        while (!token.IsCancellationRequested)
        {
            UdpReceiveResult result;
            try
            {
                result = await _udp.ReceiveAsync(token).ConfigureAwait(false);
            }
            catch (OperationCanceledException) { break; }
            catch (SocketException ex)
            {
                // P7: socket error ab silent nahi — gino + log karo.
                Interlocked.Increment(ref _socketErrorCount);
                _lastSocketErrorCode = ex.SocketErrorCode;
                _lastSocketErrorMessage = ex.Message;
                Trace.TraceWarning($"[RSH] UdpWheelSource socket error #{SocketErrorCount}: {ex.SocketErrorCode} — {ex.Message}");
                // AUDIT FIX: lagataar socket error par garam loop (100% CPU)
                // ki bajaye chhota sa waqfa — error ki wajah jo bhi ho,
                // receive dobara koshish hoti rahegi.
                try { await Task.Delay(100, token).ConfigureAwait(false); }
                catch (OperationCanceledException) { break; }
                catch (ObjectDisposedException) { break; }   // Dispose ke doran CTS gaya to khamoshi se ruk jao
                continue;
            }
            catch (ObjectDisposedException) { break; }

            ProcessDatagram(result.Buffer, result.RemoteEndPoint);
        }
    }

    /// <summary>
    /// Ek datagram ka poora amal: qism pehchano, parse karo, taaza slot
    /// mein rakho. Receive loop yahi bulata hai; tests baghair socket
    /// ke yahi istemal karte hain (har mahol mein UDP loopback dastiyab nahi).
    /// </summary>
    internal void ProcessDatagram(ReadOnlySpan<byte> data) => ProcessDatagram(data, null);

    /// <summary>ProcessDatagram + sender endpoint (HELLO reply ka remote record karne ke liye).</summary>
    internal void ProcessDatagram(ReadOnlySpan<byte> data, IPEndPoint? remoteEndPoint)
    {
        switch (PacketRouter.Classify(data))
        {
            case PacketKind.StreamV2:
                ProcessV2Datagram(data);
                return;
            case PacketKind.StatusV2:
                ProcessStatusDatagram(data);
                return;
            case PacketKind.HelloV2:
                ProcessHelloDatagram(data, remoteEndPoint);
                return;
            case PacketKind.ControlV2:
            case PacketKind.ProbeV2:
            case PacketKind.PingV2:
                // Ghalat port par aaya — sirf gino, engine ko chhoo nahi.
                Interlocked.Increment(ref _unknownDatagrams);
                return;
            default:
                // LegacyV1 ya Unknown — purana v1 path (bad bytes yahin BadMagic gine jate hain).
                ProcessV1Datagram(data);
                return;
        }
    }

    private void ProcessV1Datagram(ReadOnlySpan<byte> data)
    {
        var parse = WheelPacket.TryParse(data, out var packet);
        if (parse == PacketParseResult.Ok)
        {
            lock (_slotLock)
            {
                // AUDIT FIX: 5 s se zyada khamoshi ke baad pehla packet —
                // link naye sire se bana hai (masalan ESP reboot). Sequence
                // baseline saaf karo warna ye packet bari "loss" gin liya
                // jata tha aur LossPercent hamesha ke liye bigar jata tha.
                // (P3: chhoti khamoshi par reboot ab SequenceTracker khud
                // bare negative delta se pehchanta hai; ye 5 s wala rasta
                // sirf backup hai — masalan sequence wrap ke qareeb reboot.)
                if ((_hasLatest || _hasLatestV2) && Environment.TickCount64 - LastValidMsLocked > 5000)
                    _stats.ResetSequenceBaseline();
                _latest = packet;
                _hasLatest = true;
                _lastValidMs = Environment.TickCount64;
            }
        }
        _stats.Record(parse, packet.Sequence);
    }

    private void ProcessV2Datagram(ReadOnlySpan<byte> data)
    {
        var parse = WheelPacketV2.TryParse(data, out var packet);
        if (parse == WheelPacketV2ParseResult.Ok)
        {
            Interlocked.Increment(ref _v2Valid);
            lock (_slotLock)
            {
                if ((_hasLatest || _hasLatestV2) && Environment.TickCount64 - LastValidMsLocked > 5000)
                    _stats.ResetSequenceBaseline();
                _latestV2 = packet;
                _hasLatestV2 = true;
                _lastValidV2Ms = Environment.TickCount64;
            }
            // v2 stream bhi wahi sequence/loss/rate hisaab mein aata hai
            // (ek hi stream hai — v1 ya v2, dono 200 Hz).
            _stats.Record(PacketParseResult.Ok, packet.Sequence);
        }
        else
        {
            Interlocked.Increment(ref _v2Bad);
            _stats.Record(MapV2ParseResult(parse), 0);
        }
    }

    /// <summary>v2 parse nateeja → v1 LinkStatistics bucket (v1 enum mein BadFrame/ReservedNonzero nahi — wo BadMagic mein ginay jate hain).</summary>
    private static PacketParseResult MapV2ParseResult(WheelPacketV2ParseResult result) => result switch
    {
        WheelPacketV2ParseResult.TooShort => PacketParseResult.TooShort,
        WheelPacketV2ParseResult.BadVersion => PacketParseResult.BadVersion,
        WheelPacketV2ParseResult.BadCrc => PacketParseResult.BadCrc,
        _ => PacketParseResult.BadMagic,
    };

    private void ProcessStatusDatagram(ReadOnlySpan<byte> data)
    {
        var parse = StatusPacketV2.TryParse(data, out var packet);
        if (parse == StatusPacketV2ParseResult.Ok)
        {
            Interlocked.Increment(ref _statusReceived);
            lock (_slotLock)
            {
                _latestStatus = packet;
                _latestButtonsEx = packet.HasBtnsEx ? packet.BtnsEx : 0;
                _lastStatusMs = Environment.TickCount64;
            }
            // Event lock ke bahar — subscriber (BoardSession) ka kaam
            // receive loop ko block na kare.
            try { StatusReceived?.Invoke(packet); }
            catch { /* listener ki exception receive loop ko nahi torti */ }
        }
        else
        {
            Interlocked.Increment(ref _statusBad);
        }
    }

    private void ProcessHelloDatagram(ReadOnlySpan<byte> data, IPEndPoint? remoteEndPoint)
    {
        var parse = HelloReply.TryParse(data, out var reply);
        if (parse == HelloReplyParseResult.Ok)
        {
            Interlocked.Increment(ref _helloReceived);
            lock (_slotLock)
            {
                _latestHello = reply;
                _lastHelloRemote = remoteEndPoint;
            }
            try { HelloReplyReceived?.Invoke(reply); }
            catch { /* listener ki exception receive loop ko nahi torti */ }
        }
        else
        {
            Interlocked.Increment(ref _helloBad);
        }
    }

    /// <summary>Sirf _slotLock ke andar: v1 ya v2, jo bhi taaza ho, us ka waqt.</summary>
    private long LastValidMsLocked => Math.Max(_hasLatest ? _lastValidMs : 0, _hasLatestV2 ? _lastValidV2Ms : 0);

    /// <summary>
    /// Sab se naya valid v1 packet + us ki umar (ms). Data na ho ya
    /// <paramref name="maxAgeMs"/> se purana ho to false (watchdog yahi
    /// istemal karega).
    /// </summary>
    public bool TryGetLatest(out WheelPacket packet, out long ageMs, long maxAgeMs = long.MaxValue)
    {
        lock (_slotLock)
        {
            packet = _latest;
            if (!_hasLatest)
            {
                ageMs = long.MaxValue;
                return false;
            }
            ageMs = Environment.TickCount64 - _lastValidMs;
            return ageMs <= maxAgeMs;
        }
    }

    /// <summary>Sab se naya valid v2.1 packet + us ki umar (ms). v2 stream ka rasta (D6: sab axes mV).</summary>
    public bool TryGetLatestV2(out WheelPacketV2 packet, out long ageMs, long maxAgeMs = long.MaxValue)
    {
        lock (_slotLock)
        {
            packet = _latestV2;
            if (!_hasLatestV2)
            {
                ageMs = long.MaxValue;
                return false;
            }
            ageMs = Environment.TickCount64 - _lastValidV2Ms;
            return ageMs <= maxAgeMs;
        }
    }

    /// <summary>Aakhri valid packet (v1 ya v2) ko kitni der hui (kabhi na aya ho to null).</summary>
    public long? LastValidAgeMs
    {
        get
        {
            lock (_slotLock)
            {
                if (!_hasLatest && !_hasLatestV2) return null;
                return Environment.TickCount64 - LastValidMsLocked;
            }
        }
    }

    public void Dispose()
    {
        Task? task;
        CancellationTokenSource? cts;
        lock (_lifecycleLock)
        {
            if (_disposed) return;
            _disposed = true;
            (task, cts) = TakeLocked();
        }
        WaitForStop(task, cts);
        // Loop ab bhi ReceiveAsync mein ho to socket band karne se
        // ObjectDisposedException aye ga aur loop khamoshi se ruk jaye ga.
        _udp.Dispose();
    }
}
