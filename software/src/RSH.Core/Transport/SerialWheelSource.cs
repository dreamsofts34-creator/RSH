// RSH wired USB-serial transport — wheel source (Phase G, D26 / OPEN-20).
// UdpWheelSource ka serial twin: SerialLink se aane wale stream/status/
// HELLO frames ko usi taaza-slot mein rakhta hai jis mein UDP source
// rakhta hai — WheelEngine ko farq nahi parta ke packet UDP se aaya ya
// USB-serial se (transport abstraction; engine code duplicate nahi hua).
// Is liye IV2SampleSource + IStatusSource implement karta hai, aur
// LinkStatistics / watchdog / SequenceTracker ka behaviour UDP jaisa hai.
//
// Link SHARED hota hai (control transport bhi usi link par hai), is liye
// is class ki malkiyat by default link par NAHI hoti (ownsLink: false) —
// Stop() link ko band nahi karta, Dispose() sirf events hatata hai.
// Standalone istemal (rig harness) mein ownsLink: true do — tab Dispose()
// link ko bhi band karta hai.
using RSH.Core.Protocol;

namespace RSH.Core.Transport;

/// <summary>
/// Serial wheel source: "RSHS" stream/status/HELLO frames → taaza slot.
/// Payload wahi bytes hain jo UDP par aate (v2.1 "RSH2" / v1 / status / HELLO),
/// is liye parsing UdpWheelSource jaisi hai — sirf transport alag hai.
/// </summary>
public sealed class SerialWheelSource : ISampleSource, IV2SampleSource, IStatusSource, IWheelSource, IDisposable
{
    private readonly SerialLink _link;
    private readonly bool _ownsLink;
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
    private long _statusReceived;
    private long _statusBad;
    private long _helloReceived;
    private long _v2Valid;
    private long _v2Bad;
    private long _probeReceived;
    private bool _disposed;

    /// <summary>Valid HELLO reply aaya — BoardSession is se nonce/key leta hai (pairing/session).</summary>
    public event Action<HelloReply>? HelloReplyReceived;
    /// <summary>Valid status packet aaya — diagnostics / BtnsEx listener.</summary>
    public event Action<StatusPacketV2>? StatusReceived;

    /// <summary>
    /// Serial source banao. <paramref name="ownsLink"/> true ho to Dispose()
    /// link ko bhi band karta hai (standalone istemal — rig harness);
    /// false ho to link caller ka rehta hai (AppServices — control transport
    /// bhi usi link par hai).
    /// </summary>
    public SerialWheelSource(SerialLink link, bool ownsLink = false)
    {
        ArgumentNullException.ThrowIfNull(link);
        _link = link;
        _ownsLink = ownsLink;
        _link.StreamFrameReceived += OnStreamFrame;
        _link.StatusFrameReceived += OnStatusFrame;
        _link.HelloReplyReceived += OnHelloReply;
        _link.ProbeFrameReceived += OnProbeFrame;
    }

    /// <summary>Link (control transport / detector ke liye).</summary>
    public SerialLink Link => _link;
    /// <summary>Port ka naam (misal COM3 ya /dev/ttyUSB0).</summary>
    public string PortName => _link.PortName;

    /// <summary>UI/diagnostics ke liye: "Serial COM3".</summary>
    public string SourceDescription => $"Serial {PortName}";

    public LinkStatistics Statistics => _stats;
    /// <summary>Read loop chal raha hai?</summary>
    public bool IsRunning => _link.IsRunning;
    /// <summary>Ab tak kitne valid v2.1 stream packets aaye.</summary>
    public long V2ValidPackets => Interlocked.Read(ref _v2Valid);
    /// <summary>Ab tak kitne v2.1 stream packets reject hue.</summary>
    public long V2BadPackets => Interlocked.Read(ref _v2Bad);
    /// <summary>Ab tak kitne valid status packets aaye.</summary>
    public long StatusReceivedCount => Interlocked.Read(ref _statusReceived);
    /// <summary>Ab tak kitne status packets reject hue.</summary>
    public long StatusBadCount => Interlocked.Read(ref _statusBad);
    /// <summary>Ab tak kitne valid HELLO replies aaye.</summary>
    public long HelloReceivedCount => Interlocked.Read(ref _helloReceived);
    /// <summary>Ab tak kitne HELLO replies reject hue (link ne parse nahi kiye — yahan sirf valid aate hain, is liye link ka counter dekho).</summary>
    public long HelloBadCount => _link.HelloBadCount;
    /// <summary>Ab tak kitne probe frames aaye (sirf ginay jate hain — engine ko chhoo nahi).</summary>
    public long ProbeReceivedCount => Interlocked.Read(ref _probeReceived);
    /// <summary>Taaza valid status packet (null jab abhi na aaya ho).</summary>
    public StatusPacketV2? LatestStatus { get { lock (_slotLock) return _latestStatus; } }
    /// <summary>Taaza StatusPacketV2.BtnsEx word (buttons 32..63 → vJoy 33..64, Ex1).</summary>
    public uint LatestButtonsEx { get { lock (_slotLock) return _latestButtonsEx; } }
    /// <summary>Aakhri valid status packet ko kitni der hui (kabhi na aya ho to null).</summary>
    public long? LatestButtonsExAgeMs
    {
        get { lock (_slotLock) return _latestStatus is null ? null : Environment.TickCount64 - _lastStatusMs; }
    }
    /// <summary>Taaza valid HELLO reply (null jab abhi na aaya ho).</summary>
    public HelloReply? LatestHelloReply { get { lock (_slotLock) return _latestHello; } }

    /// <summary>Link start karo (port kholo + read loop). Pehle se chal raha ho to kuch nahi.</summary>
    public void Start() => _link.Start();

    /// <summary>
    /// Source roko. Link SHARED hai (control transport bhi usi par hai),
    /// is liye Stop() link ko band NAHI karta — sirf is source ke events
    /// ka asar khatam hota hai (slot taaza nahi hota). Link band karna ho
    /// to <see cref="SerialLink.Stop"/> / Dispose() bulayo.
    /// </summary>
    public void Stop()
    {
        // Link shared hai — yahan kuch nahi karna (UdpWheelSource jaisa
        // socket-stop yahan galat hota: control channel bhi usi link par hai).
    }

    /// <summary>
    /// v2 HELO bhejo (5 B, §2.1) aur HELLO reply ka intezar karo.
    /// <paramref name="newSession"/> true = naya session (naya nonce,
    /// counter 0 se); false = resume. Reply 500 ms mein na aaye to null
    /// (HelloClient loop har 1 s bulata hai, is liye timeout chhota hai).
    /// </summary>
    public Task<HelloReply?> SendHelloV2Async(bool newSession = false, CancellationToken token = default)
    {
        try
        {
            return _link.SendHelloAsync(newSession, TimeSpan.FromMilliseconds(500), token);
        }
        catch (InvalidOperationException)
        {
            // Pehle se HELO in-flight hai (overlap) — is tick ko chhoro, agla HELO 1 s baad aayega.
            return Task.FromResult<HelloReply?>(null);
        }
    }

    private void OnStreamFrame(byte[] payload) => ProcessStreamPayload(payload);

    /// <summary>
    /// Ek stream payload ka poora amal: qism pehchano, parse karo, taaza
    /// slot mein rakho. Link ka read loop yahi bulata hai; tests baghair
    /// port ke yahi istemal karte hain (in-memory seam).
    /// </summary>
    internal void ProcessStreamPayload(ReadOnlySpan<byte> data)
    {
        switch (PacketRouter.Classify(data))
        {
            case PacketKind.StreamV2:
                ProcessV2Datagram(data);
                return;
            case PacketKind.StatusV2:
                ProcessStatusDatagram(data);
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

    private void OnStatusFrame(byte[] payload) => ProcessStatusDatagram(payload);

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
            try { StatusReceived?.Invoke(packet); }
            catch { /* listener ki exception read loop ko nahi torti */ }
        }
        else
        {
            Interlocked.Increment(ref _statusBad);
        }
    }

    private void OnHelloReply(HelloReply reply)
    {
        Interlocked.Increment(ref _helloReceived);
        lock (_slotLock) { _latestHello = reply; }
        try { HelloReplyReceived?.Invoke(reply); }
        catch { /* listener ki exception read loop ko nahi torti */ }
    }

    private void OnProbeFrame(byte[] payload)
    {
        _ = payload;
        Interlocked.Increment(ref _probeReceived);
    }

    /// <summary>Sirf _slotLock ke andar: v1 ya v2, jo bhi taaza ho, us ka waqt.</summary>
    private long LastValidMsLocked => Math.Max(_hasLatest ? _lastValidMs : 0, _hasLatestV2 ? _lastValidV2Ms : 0);

    /// <summary>Sab se naya valid v1 packet + us ki umar (ms).</summary>
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
        if (_disposed) return;
        _disposed = true;
        _link.StreamFrameReceived -= OnStreamFrame;
        _link.StatusFrameReceived -= OnStatusFrame;
        _link.HelloReplyReceived -= OnHelloReply;
        _link.ProbeFrameReceived -= OnProbeFrame;
        if (_ownsLink) _link.Dispose();
    }
}
