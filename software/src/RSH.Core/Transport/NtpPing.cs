// RSH NTP-style ping (SW-13, Phase C) — E2E latency, sirf isi se measure hoti hai
// (Plan v2.3 §3, R-H2). Wire format: docs/PROTOCOL-V2.1.md §9.
// Security: docs/SECURITY-REVIEW-PHASE-C.md §4 — SEC-C-21 board rate limit
// (PingRateLimiter: 10/s, burst 5), SEC-C-22 reply ≤ request + source-only,
// SEC-C-24 PC-side flood guard (1–4 Hz, MinSendInterval 250 ms),
// SEC-C-25 offset sirf display (kabhi security faisla nahi).
//
// 4 timestamps (NTP-style): t0 PC send, t1 board receive, t2 board send,
// t3 PC receive (t3 wire par nahi aata — PC receive karte waqt khud naapta hai).
//   delay  = (t3 - t0) - (t2 - t1)   (round-trip minus board processing)
//   offset = ((t1 - t0) + (t2 - t3)) / 2   (board clock - PC clock, µs)
// Board firmware abhi Phase D mein aayega — PC side aise bana hai ke virtual
// wheel (test-env/virtual-wheel/wheel.py, ping reply) ke khilaf test ho sake.
// Ping HMAC/control channel se NAHI guzarta: ye timing measurement hai, koi
// command nahi — is liye is par na key chahiye na session (pairing se pehle
// bhi chal sakta hai). Isi liye magic "RSHN" control "RSHC" se alag hai,
// lekin port wahi 4212 hai (magic se demux hota hai — naya firewall rule nahi).
//
// Clocks: dono taraf MONOTONIC µs (PC: Stopwatch, board: boot se µs).
// Epochs alag hain — offset formula hi un ka farq nikaalta hai; delay par
// epoch ka koi asar nahi. Board timestamps wire par u64 hain (u32 µs 71 min
// mein wrap hota — ping mein wrap nahi chahiye, firmware u64 rakhega, §9).
using System.Buffers.Binary;
using System.Diagnostics;
using System.Net.Sockets;

namespace RSH.Core.Transport;

/// <summary>Ping frame parse ka nateeja (§9). TooShort sirf tab jab 8 B ka header hi na parha ja sake (control §3.1 jaisa rule); header parh liya ho to har length mismatch → BadLength.</summary>
public enum PingParseResult
{
    Ok,
    TooShort,
    BadMagic,
    BadVersion,
    BadLength,
    BadCrc,
    BadType,
}

/// <summary>Ping frame ki qism: request (PC → board) ya reply (board → PC).</summary>
public enum PingFrameType : byte
{
    Request = 0,
    Reply = 1,
}

/// <summary>
/// Ping frame (§9), fixed 34 B: "RSHN" | ver 0x21 | type u8 | seq u16 |
/// t0 u64 | t1 u64 | t2 u64 | CRC16. Request mein t1 = t2 = 0; reply mein
/// t0 wahi echo hota hai jo request mein tha.
/// </summary>
public readonly struct PingFrame
{
    public const int Size = 34;

    public PingFrameType Type { get; }
    public ushort Seq { get; }
    /// <summary>PC send, monotonic µs.</summary>
    public long T0Us { get; }
    /// <summary>Board receive, board monotonic µs (request mein 0).</summary>
    public long T1Us { get; }
    /// <summary>Board send, board monotonic µs (request mein 0).</summary>
    public long T2Us { get; }

    public PingFrame(PingFrameType type, ushort seq, long t0Us, long t1Us, long t2Us)
    {
        Type = type;
        Seq = seq;
        T0Us = t0Us;
        T1Us = t1Us;
        T2Us = t2Us;
    }

    /// <summary>Request banao (t1 = t2 = 0).</summary>
    public static byte[] BuildRequest(ushort seq, long t0Us)
        => Build(PingFrameType.Request, seq, t0Us, 0, 0);

    /// <summary>Reply banao (t0 request wala echo).</summary>
    public static byte[] BuildReply(ushort seq, long t0Us, long t1Us, long t2Us)
        => Build(PingFrameType.Reply, seq, t0Us, t1Us, t2Us);

    private static byte[] Build(PingFrameType type, ushort seq, long t0Us, long t1Us, long t2Us)
    {
        var frame = new byte[Size];
        frame[0] = (byte)'R'; frame[1] = (byte)'S'; frame[2] = (byte)'H'; frame[3] = (byte)'N';
        frame[4] = Protocol.ProtocolV2.Version;
        frame[5] = (byte)type;
        BinaryPrimitives.WriteUInt16LittleEndian(frame.AsSpan(6, 2), seq);
        BinaryPrimitives.WriteUInt64LittleEndian(frame.AsSpan(8, 8), (ulong)t0Us);
        BinaryPrimitives.WriteUInt64LittleEndian(frame.AsSpan(16, 8), (ulong)t1Us);
        BinaryPrimitives.WriteUInt64LittleEndian(frame.AsSpan(24, 8), (ulong)t2Us);
        ushort crc = Protocol.Crc16.Compute(frame.AsSpan(0, Size - 2));
        BinaryPrimitives.WriteUInt16LittleEndian(frame.AsSpan(Size - 2, 2), crc);
        return frame;
    }

    /// <summary>Frame parse + validate (magic, version, type, length, CRC). Timestamps ki samajh (t2 ≥ t1 waghera) client karta hai — parser sirf wire check karta hai.</summary>
    public static PingParseResult TryParse(ReadOnlySpan<byte> data, out PingFrame frame)
    {
        frame = default;
        if (data.Length < 8) return PingParseResult.TooShort;
        if (data[0] != (byte)'R' || data[1] != (byte)'S' || data[2] != (byte)'H' || data[3] != (byte)'N')
            return PingParseResult.BadMagic;
        if (data[4] != Protocol.ProtocolV2.Version) return PingParseResult.BadVersion;
        if (data[5] > (byte)PingFrameType.Reply) return PingParseResult.BadType;
        if (data.Length != Size) return PingParseResult.BadLength;
        ushort stored = BinaryPrimitives.ReadUInt16LittleEndian(data[(Size - 2)..]);
        if (Protocol.Crc16.Compute(data[..(Size - 2)]) != stored) return PingParseResult.BadCrc;

        frame = new PingFrame(
            (PingFrameType)data[5],
            BinaryPrimitives.ReadUInt16LittleEndian(data[6..8]),
            (long)BinaryPrimitives.ReadUInt64LittleEndian(data[8..16]),
            (long)BinaryPrimitives.ReadUInt64LittleEndian(data[16..24]),
            (long)BinaryPrimitives.ReadUInt64LittleEndian(data[24..32]));
        return PingParseResult.Ok;
    }
}

/// <summary>
/// Ek ping ka nateeja — 4 timestamps se NTP formulas. Compute() null deta hai
/// jab timestamps namumkin hon (t2 &lt; t1, t3 &lt; t0, ya delay &lt; 0) — aisa
/// reply invalid hai, use kabhi metric mein nahi daalna.
/// </summary>
public readonly struct PingResult
{
    public ushort Seq { get; }
    public long T0Us { get; }
    public long T1Us { get; }
    public long T2Us { get; }
    public long T3Us { get; }
    /// <summary>Round-trip minus board processing: (t3-t0)-(t2-t1). Yehi E2E network delay hai.</summary>
    public long DelayUs { get; }
    /// <summary>Board clock − PC clock (µs). Epochs alag hain — yehi farq hai.
    /// SEC-C-25: offset SIRF SLO display (E2E latency) ke liye hai — is se kabhi
    /// auth / replay / expiry / koi security faisla nahi hoga. Ping timestamps
    /// untrusted measurement hain; attacker jhooti timestamps bhej sakta hai.</summary>
    public long OffsetUs { get; }
    /// <summary>Board ne reply banane mein kitna waqt liya: t2-t1.</summary>
    public long BoardProcessingUs { get; }
    /// <summary>Poora round-trip: t3-t0 (processing samet).</summary>
    public long RoundTripUs { get; }

    private PingResult(ushort seq, long t0, long t1, long t2, long t3, long delay, long offset)
    {
        Seq = seq;
        T0Us = t0; T1Us = t1; T2Us = t2; T3Us = t3;
        DelayUs = delay;
        OffsetUs = offset;
        BoardProcessingUs = t2 - t1;
        RoundTripUs = t3 - t0;
    }

    /// <summary>NTP formulas. Namumkin timestamps → null (invalid reply).</summary>
    public static PingResult? Compute(ushort seq, long t0Us, long t1Us, long t2Us, long t3Us)
    {
        if (t2Us < t1Us || t3Us < t0Us) return null;
        long delay = (t3Us - t0Us) - (t2Us - t1Us);
        if (delay < 0) return null;
        long offset = ((t1Us - t0Us) + (t2Us - t3Us)) / 2;
        return new PingResult(seq, t0Us, t1Us, t2Us, t3Us, delay, offset);
    }
}

/// <summary>PC monotonic clock, µs (Stopwatch — wall clock nahi, NTP adjustments se nahi hilta).</summary>
public static class MonotonicClock
{
    private static readonly long StartTimestamp = Stopwatch.GetTimestamp();

    /// <summary>Process start se guzre µs (monotonic).</summary>
    public static long NowUs() => Stopwatch.GetElapsedTime(StartTimestamp).Ticks / 10;
}

/// <summary>
/// E2E latency metric (Plan v2.3 §3) — ping samples se. Jab tak koi ping
/// reply na aaye, metric "unmeasured" hai: jhoota number kabhi nahi.
/// Ye LinkStatistics se ALAG jagah hai (LinkStatistics.cs ka P8 sliding-window
/// kaam Data &amp; AI team ka hai — us file ko haath nahi lagaya gaya).
/// Record() allocation-free hai (fixed ring); percentile sirf parhne par banta hai.
/// </summary>
public sealed class E2ELatency
{
    /// <summary>Wi-Fi SLO: E2E p99 ≤ 10 ms (v2.3 §3).</summary>
    public const long SloWifiUs = 10_000;
    /// <summary>Wired SLO: E2E p99 ≤ 5 ms (v2.3 §3).</summary>
    public const long SloWiredUs = 5_000;

    private const int RingCapacity = 128;
    private readonly object _lock = new();
    private readonly long[] _delaysUs = new long[RingCapacity];
    private int _count;
    private int _next;
    private long _samples;
    private long _timeouts;
    private long _invalidReplies;
    private long? _lastDelayUs;
    private long? _lastOffsetUs;

    /// <summary>Ab tak kitne valid ping replies mile.</summary>
    public long Samples { get { lock (_lock) return _samples; } }
    /// <summary>Ab tak kitne ping timeout hue (koi reply nahi).</summary>
    public long Timeouts { get { lock (_lock) return _timeouts; } }
    /// <summary>Ab tak kitne invalid replies aaye (ghalat seq/t0, kharab CRC, namumkin timestamps) — timeout se alag ginati.</summary>
    public long InvalidReplies { get { lock (_lock) return _invalidReplies; } }
    /// <summary>Koi valid ping reply mila hai? Nahi → metric "unmeasured".</summary>
    public bool IsMeasured { get { lock (_lock) return _samples > 0; } }
    /// <summary>Aakhri ping ka delay (µs), ya null jab unmeasured.</summary>
    public long? LastDelayUs { get { lock (_lock) return _lastDelayUs; } }
    /// <summary>Aakhri ping ka clock offset (µs, board − PC), ya null jab unmeasured.
    /// SEC-C-25: SIRF display ke liye — is se kabhi auth/replay/expiry faisla nahi.</summary>
    public long? LastOffsetUs { get { lock (_lock) return _lastOffsetUs; } }
    /// <summary>Ab tak ka sab se bara delay (µs), ya null jab unmeasured — spike diagnosis ke liye.</summary>
    public long? MaxDelayUs
    {
        get
        {
            lock (_lock)
            {
                if (_samples == 0) return null;
                long max = long.MinValue;
                for (int i = 0; i < _count; i++)
                    if (_delaysUs[i] > max) max = _delaysUs[i];
                return max;
            }
        }
    }

    /// <summary>Ring mein kitne samples <paramref name="thresholdUs"/> se upar hain — SLO breach diagnosis (p99 fail ho to ye batata hai 1 spike thi ya systemic).</summary>
    public int CountOver(long thresholdUs)
    {
        lock (_lock)
        {
            int n = 0;
            for (int i = 0; i < _count; i++)
                if (_delaysUs[i] > thresholdUs) n++;
            return n;
        }
    }

    /// <summary>Valid ping result record karo.</summary>
    public void Record(PingResult result)
    {
        lock (_lock)
        {
            _delaysUs[_next] = result.DelayUs;
            _next = (_next + 1) % RingCapacity;
            if (_count < RingCapacity) _count++;
            _samples++;
            _lastDelayUs = result.DelayUs;
            _lastOffsetUs = result.OffsetUs;
        }
    }

    /// <summary>Timeout record karo (metric unmeasured hi rehta hai — timeout se koi delay nahi banta).</summary>
    public void RecordTimeout()
    {
        lock (_lock) _timeouts++;
    }

    /// <summary>Invalid reply (ghalat seq/t0, namumkin timestamps, kharab CRC) — metric par koi asar nahi, sirf alag se gin lo (timeout nahi hai).</summary>
    public void RecordInvalidReply()
    {
        lock (_lock) _invalidReplies++;
    }

    /// <summary>Delay percentile (µs), ya null jab unmeasured. Ring (aakhri ≤128 samples) se.</summary>
    public double? Percentile(double p)
    {
        long[] copy;
        int n;
        lock (_lock)
        {
            if (_samples == 0) return null;
            n = _count;
            copy = new long[n];
            Array.Copy(_delaysUs, copy, n);
        }
        Array.Sort(copy);
        int idx = (int)Math.Ceiling(p * n) - 1;
        return copy[Math.Clamp(idx, 0, n - 1)];
    }

    /// <summary>p50 delay (µs), ya null jab unmeasured.</summary>
    public double? DelayP50Us => Percentile(0.50);
    /// <summary>p99 delay (µs), ya null jab unmeasured. SLO isi par lagta hai.</summary>
    public double? DelayP99Us => Percentile(0.99);

    /// <summary>Panel ke liye text: "unmeasured" jab koi sample nahi, warna "p99 X.X ms (n=N)". Kabhi jhoota number nahi.</summary>
    public string DisplayText
    {
        get
        {
            lock (_lock)
            {
                if (_samples == 0) return "unmeasured";
            }
            double? p99 = DelayP99Us;
            return p99 is null ? "unmeasured" : $"p99 {p99.Value / 1000.0:F1} ms (n={Samples})";
        }
    }

    /// <summary>SLO pass? Unmeasured ho to hamesha false — bina measurement ke pass ka daawa nahi.</summary>
    public bool MeetsSlo(bool wired)
    {
        double? p99 = DelayP99Us;
        if (p99 is null) return false;
        return p99.Value <= (wired ? SloWiredUs : SloWifiUs);
    }

    /// <summary>Sab bhool jao (reconnect / naya session).</summary>
    public void Reset()
    {
        lock (_lock)
        {
            _count = 0; _next = 0; _samples = 0; _timeouts = 0; _invalidReplies = 0;
            _lastDelayUs = null; _lastOffsetUs = null;
            Array.Clear(_delaysUs);
        }
    }
}

/// <summary>Ping request/reply ka ek exchange (transport ki tafseel chhupata hai — tests fake transport de sakte hain).</summary>
public interface IPingTransport : IDisposable
{
    /// <summary>Request bhejo, reply ka intezar karo. Timeout ya koi reply na aaye to null.</summary>
    Task<byte[]?> ExchangeAsync(byte[] request, TimeSpan timeout, CancellationToken cancellationToken);
}

/// <summary>Asal UDP transport: board ke ping port (4212) par connected socket.
/// Socket bind nahi hota — OS ephemeral port deta hai (T10, SEC-C-24).
/// Raw ping payload kabhi log nahi hota (SEC-C-24).</summary>
public sealed class UdpPingTransport : IPingTransport
{
    private readonly UdpClient _udp;

    public UdpPingTransport(string boardHost, int boardPort)
    {
        _udp = new UdpClient();
        _udp.Connect(boardHost, boardPort);
    }

    public async Task<byte[]?> ExchangeAsync(byte[] request, TimeSpan timeout, CancellationToken cancellationToken)
    {
        try
        {
            await _udp.SendAsync(request.AsMemory(), cancellationToken).ConfigureAwait(false);
        }
        catch (OperationCanceledException) { return null; }
        catch (SocketException) { return null; }
        catch (ObjectDisposedException) { return null; }

        using var timeoutCts = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        timeoutCts.CancelAfter(timeout);
        try
        {
            var result = await _udp.ReceiveAsync(timeoutCts.Token).ConfigureAwait(false);
            return result.Buffer;
        }
        catch (OperationCanceledException) { return null; }
        catch (SocketException) { return null; }
        catch (ObjectDisposedException) { return null; }
    }

    public void Dispose() => _udp.Dispose();
}

/// <summary>
/// NTP-style ping client (SW-13): PC → board ping, 4 timestamps, clock offset.
/// Ek ping = ek request + ek reply; timeout (default 500 ms) par null — metric
/// "unmeasured" rehta hai, crash nahi. Har reply E2ELatency mein record hota hai.
///
/// SEC-C-24 (PC-side flood guard): sender 1–4 Hz — PingAsync do bheje gaye
/// pings ke darmiyan kam se kam MinSendInterval (250 ms) ka waqfa lazmi rakhta
/// hai; is se pehle bulaya jaye to packet bheje BAGHAIR null milta hai
/// (ThrottledSends mein gin'a jata hai — ye timeout nahi hai, metric par
/// koi asar nahi). PingLoopAsync ka default interval 1 s hai. Ping kabhi UI
/// thread par nahi chalta (async, ConfigureAwait(false)) aur raw payload
/// kabhi log nahi hota.
/// </summary>
public sealed class NtpPingClient : IDisposable
{
    /// <summary>Ping port — control port 4212 hi hai (magic "RSHN" se demux, §9).</summary>
    public const int DefaultPingPort = 4212;
    /// <summary>Ek ping ka default timeout (control timeout 250 ms × 3 se alag — ping retry nahi karta, agla ping 1 s baad aata hai).</summary>
    public static readonly TimeSpan DefaultTimeout = TimeSpan.FromMilliseconds(500);
    /// <summary>Ping loop ka default interval — 1 Hz (SEC-C-24: sender 1–4 Hz).</summary>
    public static readonly TimeSpan DefaultInterval = TimeSpan.FromSeconds(1);
    /// <summary>Do pings ke darmiyan kam se kam waqfa — 250 ms = max 4 Hz (SEC-C-24). Is se tez bhejne ki koshish throttle hoti hai, flood nahi banta.</summary>
    public static readonly TimeSpan MinSendInterval = TimeSpan.FromMilliseconds(250);

    private readonly IPingTransport _transport;
    private readonly Func<long> _nowUs;
    private readonly long _minSendIntervalTicks;
    // Throttle ka waqt measurement clock se ALAG hai: measurement clock
    // tests mein fake ho sakta hai, throttle hamesha asal waqt se naapta hai.
    private long _lastSendTimestamp;
    // DATA-AI FIX (Round 2 W7-2): seq + throttle claim atomic hone
    // chahiye — pehle `_nextSeq++` aur check-then-Exchange do concurrent
    // PingAsync calls ko ek hi seq / ek hi 250 ms window mein do pings
    // bhejane dete thay (SEC-C-24 flood guard bypass). Seq counter int
    // hai (ushort mein cast par wrap waisa hi hai jaisa pehle tha).
    private int _nextSeqCounter;
    private bool _disposed;

    /// <summary>E2E latency metric — Performance panel yahi parhega (read-only exposure).</summary>
    public E2ELatency Latency { get; } = new();

    public long PingsSent { get; private set; }
    public long RepliesReceived { get; private set; }
    public long Timeouts { get; private set; }
    public long InvalidReplies { get; private set; }
    /// <summary>SEC-C-24 guard ne kitni koshishein roki (packet bheja hi nahi gaya).</summary>
    public long ThrottledSends { get; private set; }

    /// <summary>Asal UDP transport ke saath client banao.</summary>
    public NtpPingClient(string boardHost, int boardPort = DefaultPingPort)
        : this(new UdpPingTransport(boardHost, boardPort), null)
    {
    }

    /// <summary>Tests ke liye: fake transport + fake clock de sakte hain (deterministic timestamps). <paramref name="minSendInterval"/> se PC-side flood guard test ho sakta hai (default 250 ms; TimeSpan.Zero = guard band).</summary>
    public NtpPingClient(IPingTransport transport, Func<long>? nowUs = null, TimeSpan? minSendInterval = null)
    {
        _transport = transport;
        _nowUs = nowUs ?? MonotonicClock.NowUs;
        _minSendIntervalTicks = (long)((minSendInterval ?? MinSendInterval).TotalSeconds * Stopwatch.Frequency);
    }

    /// <summary>
    /// Ek ping karo. Reply mile to PingResult (Latency mein record ho gaya);
    /// timeout / invalid reply par null (Latency unmeasured rehta hai ya
    /// pichle samples par rehta hai — timeout purane samples ko nahi mitata).
    /// SEC-C-24: pichle ping se MinSendInterval se pehle bulaya jaye to
    /// packet bheje baghair null milta hai (ThrottledSends +1).
    /// </summary>
    public async Task<PingResult?> PingAsync(TimeSpan timeout, CancellationToken cancellationToken = default)
    {
        if (_disposed) throw new ObjectDisposedException(nameof(NtpPingClient));
        // PC-side flood guard (SEC-C-24) — asal waqt se, measurement clock se nahi.
        // DATA-AI FIX (Round 2 W7-2): claim atomic hai — CompareExchange
        // loop: sirf wahi caller bhejta hai jis ne _lastSendTimestamp ko
        // kamyabi se badla; doosra caller throttle hota hai (pehle
        // check-then-Exchange race mein dono guzar sakte thay).
        long sendStamp = Stopwatch.GetTimestamp();
        while (true)
        {
            long last = Interlocked.Read(ref _lastSendTimestamp);
            if (last != 0 && sendStamp - last < _minSendIntervalTicks)
            {
                ThrottledSends++;
                return null;
            }
            if (Interlocked.CompareExchange(ref _lastSendTimestamp, sendStamp, last) == last) break;
        }

        ushort seq = (ushort)Interlocked.Increment(ref _nextSeqCounter);
        long t0 = _nowUs();
        byte[] request = PingFrame.BuildRequest(seq, t0);
        PingsSent++;

        byte[]? replyBytes = await _transport.ExchangeAsync(request, timeout, cancellationToken).ConfigureAwait(false);
        long t3 = _nowUs();
        if (replyBytes is null)
        {
            Timeouts++;
            Latency.RecordTimeout();
            return null;
        }

        if (PingFrame.TryParse(replyBytes, out var frame) != PingParseResult.Ok
            || frame.Type != PingFrameType.Reply
            || frame.Seq != seq
            || frame.T0Us != t0)
        {
            InvalidReplies++;
            Latency.RecordInvalidReply();
            return null;
        }

        var result = PingResult.Compute(seq, t0, frame.T1Us, frame.T2Us, t3);
        if (result is null)
        {
            InvalidReplies++;
            Latency.RecordInvalidReply();
            return null;
        }

        RepliesReceived++;
        Latency.Record(result.Value);
        return result;
    }

    /// <summary>Ping loop: har <paramref name="interval"/> par ek ping, jab tak cancel na ho. Har ping ke baad interval ka intezar (ping ka apna timeout alag hai). Interval MinSendInterval (250 ms) se kam ho to 250 ms par clamp hota hai (SEC-C-24 — loop bhi 4 Hz se tez nahi).</summary>
    public async Task PingLoopAsync(TimeSpan interval, CancellationToken cancellationToken = default)
    {
        if (interval < MinSendInterval) interval = MinSendInterval;
        while (!cancellationToken.IsCancellationRequested)
        {
            try
            {
                await PingAsync(DefaultTimeout, cancellationToken).ConfigureAwait(false);
            }
            catch (OperationCanceledException) { break; }
            try
            {
                await Task.Delay(interval, cancellationToken).ConfigureAwait(false);
            }
            catch (OperationCanceledException) { break; }
        }
    }

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        _transport.Dispose();
    }
}
