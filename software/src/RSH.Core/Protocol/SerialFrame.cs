// RSH wired USB-serial transport — serial framing (Phase G, D26 / OPEN-20).
// Authority: docs/PROTOCOL-V2.1.md §10. Firmware mirror:
// firmware/rsh/serial_transport.h (wahi format, wahi numbers —
// 3-language match: C++ host tests + Python vectors + ye file).
//
// Frame (dono directions):
//   magic "RSHS" (4 B) | kind u8 | len u16 LE | payload (len B) | CRC16 (2 B)
//   CRC16-CCITT (§0: poly 0x1021, init 0xFFFF) — CRC field se pehle ke
//   tamam bytes par (D2). Max payload 1053 B (bara control frame, §3.1)
//   → max frame 1062 B.
//
// Kinds (payload wahi bytes hain jo UDP par jate — koi naya wire format
// nahi, sirf serial envelope hai): 1 STREAM, 2 STATUS, 3 HELLO,
// 4 CONTROL, 5 PING, 6 HELO, 7 PROBE.
//
// Baud: 921600, 8N1 (115200 kaafi nahi — 200 Hz x ~45 B = ~78%
// utilization, control replies ke liye headroom nahi).
using System.Buffers.Binary;

namespace RSH.Core.Protocol;

/// <summary>Serial frame ki qism (§10).</summary>
public enum SerialFrameKind : byte
{
    Stream = 1,
    Status = 2,
    Hello = 3,
    Control = 4,
    Ping = 5,
    Helo = 6,
    Probe = 7,
}

/// <summary>Serial frame parse ka nateeja.</summary>
public enum SerialFrameParseResult
{
    Ok,
    TooShort,
    BadMagic,
    BadLength,
    BadCrc,
}

/// <summary>Ek complete serial frame (kind + payload bytes).</summary>
public sealed record SerialFrame(SerialFrameKind Kind, byte[] Payload);

/// <summary>Decoder ne poora header parh liye hue frame ko kyun giraya (kind maloom hai).</summary>
public enum SerialFrameDropReason
{
    /// <summary>Frame CRC16 mismatch — frame ka kind header se maloom hai.</summary>
    BadCrc,
    /// <summary>Kind sahi hai lekin payload len &gt; 1053.</summary>
    BadLength,
}

/// <summary>
/// Serial framing codec (§10): frame banao / ek dafa mein parse karo.
/// Incremental stream ke liye <see cref="SerialFrameDecoder"/> dekho.
/// </summary>
public static class SerialFrameCodec
{
    /// <summary>Magic "RSHS" (ASCII).</summary>
    public static ReadOnlySpan<byte> Magic => "RSHS"u8;

    /// <summary>Header size: magic(4) + kind(1) + len(2) = 7.</summary>
    public const int HeaderSize = 7;
    /// <summary>CRC field size = 2.</summary>
    public const int CrcSize = 2;
    /// <summary>Zyada se zyada payload = bara control frame (§3.1) = 1053 B.</summary>
    public const int MaxPayload = ProtocolV2.ControlMaxFrame;
    /// <summary>Zyada se zyada poora frame = 1062 B.</summary>
    public const int MaxFrame = HeaderSize + MaxPayload + CrcSize;

    /// <summary>Default baud rate (921600, 8N1 — §10).</summary>
    public const int DefaultBaudRate = 921600;

    public static bool IsValidKind(byte kind) => kind is >= 1 and <= 7;

    /// <summary>
    /// Canonical test vector (§10 — C++/Python se match lazmi):
    /// kind=Stream, payload 01..08 → frame hex
    /// 525348530108000102030405060708d95d (CRC 0x5DD9).
    /// </summary>
    public static readonly byte[] CanonicalVectorFrame = Convert.FromHexString("525348530108000102030405060708D95D");

    /// <summary>
    /// Doosra canonical vector: kind=Helo, payload "HELO" 02 00 →
    /// 5253485306060048454c4f02004749 (CRC 0x4947).
    /// </summary>
    public static readonly byte[] CanonicalHeloVectorFrame = Convert.FromHexString("5253485306060048454C4F02004749");

    /// <summary>Frame banao. Payload ≤ 1053 aur kind 1..7 warna ArgumentException (C++ build_frame jaisa — wahan bhi kind validate hota hai).</summary>
    public static byte[] Build(SerialFrameKind kind, ReadOnlySpan<byte> payload)
    {
        if (!IsValidKind((byte)kind))
            throw new ArgumentException($"Serial frame kind 1..7 hona chahiye (§10), mila {(byte)kind}.", nameof(kind));
        if (payload.Length > MaxPayload)
            throw new ArgumentException($"Serial payload ≤ {MaxPayload} bytes hona chahiye (§10).", nameof(payload));
        byte[] frame = new byte[HeaderSize + payload.Length + CrcSize];
        "RSHS"u8.CopyTo(frame.AsSpan(0, 4));
        frame[4] = (byte)kind;
        BinaryPrimitives.WriteUInt16LittleEndian(frame.AsSpan(5, 2), (ushort)payload.Length);
        payload.CopyTo(frame.AsSpan(HeaderSize, payload.Length));
        ushort crc = Crc16.Compute(frame.AsSpan(0, HeaderSize + payload.Length));
        BinaryPrimitives.WriteUInt16LittleEndian(frame.AsSpan(frame.Length - CrcSize, 2), crc);
        return frame;
    }

    /// <summary>
    /// Ek poora frame ek dafa mein parse karo. TOO_SHORT sirf tab jab 7 B
    /// header bhi poora na ho; header parh liya gaya ho to har length
    /// mismatch (truncation samet) → BadLength (control §3.1 jaisa rule).
    /// </summary>
    public static SerialFrameParseResult TryParse(ReadOnlySpan<byte> data, out SerialFrame? frame)
    {
        frame = null;
        if (data.Length < HeaderSize) return SerialFrameParseResult.TooShort;
        if (data[0] != (byte)'R' || data[1] != (byte)'S' || data[2] != (byte)'H' || data[3] != (byte)'S')
            return SerialFrameParseResult.BadMagic;
        byte kind = data[4];
        if (!IsValidKind(kind)) return SerialFrameParseResult.BadLength;
        int payloadLen = BinaryPrimitives.ReadUInt16LittleEndian(data.Slice(5, 2));
        if (payloadLen > MaxPayload) return SerialFrameParseResult.BadLength;
        if (data.Length != HeaderSize + payloadLen + CrcSize) return SerialFrameParseResult.BadLength;
        ushort stored = BinaryPrimitives.ReadUInt16LittleEndian(data.Slice(data.Length - CrcSize, 2));
        if (Crc16.Compute(data[..(data.Length - CrcSize)]) != stored) return SerialFrameParseResult.BadCrc;
        frame = new SerialFrame((SerialFrameKind)kind, data.Slice(HeaderSize, payloadLen).ToArray());
        return SerialFrameParseResult.Ok;
    }
}

/// <summary>
/// Incremental serial frame decoder — Serial se jese bytes aain, wese
/// feed karo. Poora valid frame mile to <see cref="Feed"/> true return
/// karta hai aur frame <see cref="LastFrame"/> mein hota hai (agle byte
/// se pehle copy kar lo). Ghalat frame (bad magic/CRC/length) khamoshi
/// se gira kar resync hota hai — counters mein gin'a jata hai, kabhi
/// aadha frame deliver nahi hota. Boot garbage (bootloader output) magic
/// scan mein girta hai.
/// </summary>
public sealed class SerialFrameDecoder
{
    private readonly byte[] _buf = new byte[SerialFrameCodec.MaxFrame];
    private int _pos;   // buf mein ab tak kitne bytes hain
    private int _want;  // poore frame ke liye kitne bytes chahiye (0 = header ka intezar)

    /// <summary>Aakhri complete frame (Feed ne true diya ho to valid).</summary>
    public SerialFrame? LastFrame { get; private set; }

    /// <summary>
    /// Gira hua frame (poora header parh liya gaya tha — kind maloom hai).
    /// Bad-CRC / bad-length frame kabhi deliver nahi hota, lekin jise
    /// kind chahiye ho (misal SerialLink ka HelloBadCount) wo yahan se
    /// gin sakta hai. Magic scan mein gire bytes ka kind maloom nahi
    /// hota — wo is event mein nahi aate (sirf <see cref="BadMagic"/>).
    /// Subscriber ki exception decoder ko nahi torti.
    /// </summary>
    public event Action<SerialFrameKind, SerialFrameDropReason>? FrameDropped;

    /// <summary>Ab tak kitne valid frames mile.</summary>
    public long FramesOk { get; private set; }
    /// <summary>Magic scan mein gire hue bytes (garbage / boot log).</summary>
    public long BadMagic { get; private set; }
    /// <summary>CRC mismatch par gire hue frames.</summary>
    public long BadCrc { get; private set; }
    /// <summary>Len &gt; 1053 ya kind unknown par gire hue frames.</summary>
    public long BadLength { get; private set; }

    /// <summary>Ek byte feed karo. True = poora valid frame tayyar hai (<see cref="LastFrame"/> parho).</summary>
    public bool Feed(byte b)
    {
        if (_want == 0)
        {
            // Header ka intezar: 4-byte magic window slide karo.
            if (_pos < 4)
            {
                _buf[_pos++] = b;
                if (_pos < 4) return false;
            }
            else
            {
                _buf[0] = _buf[1]; _buf[1] = _buf[2]; _buf[2] = _buf[3];
                _buf[3] = b;
            }
            if (_buf[0] != (byte)'R' || _buf[1] != (byte)'S' || _buf[2] != (byte)'H' || _buf[3] != (byte)'S')
            {
                BadMagic++;
                return false;
            }
            _pos = 4;
            _want = SerialFrameCodec.HeaderSize;
            return false;
        }

        _buf[_pos++] = b;
        if (_pos < _want) return false;
        if (_want == SerialFrameCodec.HeaderSize)
        {
            byte kind = _buf[4];
            int payloadLen = BinaryPrimitives.ReadUInt16LittleEndian(_buf.AsSpan(5, 2));
            if (!SerialFrameCodec.IsValidKind(kind) || payloadLen > SerialFrameCodec.MaxPayload)
            {
                BadLength++;
                // Kind sahi ho to giraya gaya frame gin'ne layak hai
                // (kind ghalat ho to attribute nahi ho sakta).
                if (SerialFrameCodec.IsValidKind(kind))
                    RaiseFrameDropped((SerialFrameKind)kind, SerialFrameDropReason.BadLength);
                _pos = 0;
                _want = 0;
                return false;
            }
            _want = SerialFrameCodec.HeaderSize + payloadLen + SerialFrameCodec.CrcSize;
            return false; // CRC bytes abhi aane hain (len 0 par bhi 2 bytes)
        }

        // Poora frame aa gaya — CRC check karo.
        int total = _want;
        int payloadLen2 = BinaryPrimitives.ReadUInt16LittleEndian(_buf.AsSpan(5, 2));
        ushort stored = BinaryPrimitives.ReadUInt16LittleEndian(_buf.AsSpan(total - SerialFrameCodec.CrcSize, 2));
        if (Crc16.Compute(_buf.AsSpan(0, total - SerialFrameCodec.CrcSize)) != stored)
        {
            BadCrc++;
            // Header pehle hi validate ho chuka hai — kind pakka 1..7 hai.
            RaiseFrameDropped((SerialFrameKind)_buf[4], SerialFrameDropReason.BadCrc);
            _pos = 0;
            _want = 0;
            return false;
        }
        byte[] payload = _buf.AsSpan(SerialFrameCodec.HeaderSize, payloadLen2).ToArray();
        LastFrame = new SerialFrame((SerialFrameKind)_buf[4], payload);
        FramesOk++;
        _pos = 0;
        _want = 0;
        return true;
    }

    /// <summary>Buffer feed karo; complete frames ki ginti return karta hai (sirf aakhri frame <see cref="LastFrame"/> mein rehta hai).</summary>
    public int Feed(ReadOnlySpan<byte> data)
    {
        int n = 0;
        foreach (byte b in data)
            if (Feed(b)) n++;
        return n;
    }

    /// <summary>Decoder reset karo (naye connection par).</summary>
    public void Reset()
    {
        _pos = 0;
        _want = 0;
        LastFrame = null;
    }

    private void RaiseFrameDropped(SerialFrameKind kind, SerialFrameDropReason reason)
    {
        try { FrameDropped?.Invoke(kind, reason); }
        catch { /* subscriber ki exception decoder ko nahi torti */ }
    }
}
