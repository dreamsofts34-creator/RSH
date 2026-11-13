using System.Buffers.Binary;
using System.Security.Cryptography;

namespace RSH.Core.Protocol;

/// <summary>Control frame parse ka nateeja (§3.1/§3.2).</summary>
public enum ControlParseResult
{
    Ok,
    TooShort,
    BadMagic,
    /// <summary>len &gt; 1024 ya total length != 13 + len + 16 (truncated / len mismatch).</summary>
    BadLength,
    /// <summary>HMAC tag verify fail (ghalat key / ghalat nonce / tampered frame) → BAD_TAG (1).</summary>
    BadTag,
}

/// <summary>
/// Control frame (§3.1): "RSHC" | cmd | reqId u16 | counter u32 | len u16 |
/// payload (len bytes) | tag[16]. Total = 13 + len + 16, max 1053 B.
/// Tag = HMAC-SHA256(key32, nonce16 ‖ counter_LE ‖ cmd ‖ payload)[0:16] (§3.2).
/// Frame header (magic/reqId/len) HMAC mein NAHI hai — sirf nonce, counter, cmd, payload.
/// Key BoardSession/KeyStore (DPAPI, Phase E WS5) se aati hai — yahan key parameter hai, kabhi hard-code nahi.
/// </summary>
public readonly struct ControlFrame
{
    public byte Cmd { get; }
    public ushort ReqId { get; }
    public uint Counter { get; }
    public byte[] Payload { get; }
    public byte[] Tag { get; }

    /// <summary>Reply mein cmd = reqCmd | 0x80 (§3.1).</summary>
    public bool IsReply => (Cmd & 0x80) != 0;
    public byte RequestCmd => (byte)(Cmd & 0x7F);

    public ControlFrame(byte cmd, ushort reqId, uint counter, byte[] payload, byte[] tag)
    {
        Cmd = cmd;
        ReqId = reqId;
        Counter = counter;
        Payload = payload;
        Tag = tag;
    }

    /// <summary>Reply payload ka pehla byte = status (§3.3/§3.9). Payload khaali ho to null.</summary>
    public ControlStatus? ReplyStatus => Payload.Length == 0 ? null : (ControlStatus)Payload[0];
}

public static class ControlFrameCodec
{
    private static void CheckKeyAndNonce(ReadOnlySpan<byte> key, ReadOnlySpan<byte> nonce)
    {
        if (key.Length != 32)
            throw new ArgumentException("Device key 32 bytes ki honi chahiye (§4.1).", nameof(key));
        if (nonce.Length != 16)
            throw new ArgumentException("Nonce 16 bytes ka hona chahiye (§2.3).", nameof(nonce));
    }

    /// <summary>Tag compute karo: HMAC-SHA256(key, nonce ‖ counter_LE ‖ cmd ‖ payload)[0:16].</summary>
    public static byte[] ComputeTag(ReadOnlySpan<byte> key, ReadOnlySpan<byte> nonce,
        uint counter, byte cmd, ReadOnlySpan<byte> payload)
    {
        CheckKeyAndNonce(key, nonce);
        // HMAC input ko ek buffer mein joro (max 16 + 4 + 1 + 1024 = 1045 B).
        var input = new byte[16 + 4 + 1 + payload.Length];
        nonce.CopyTo(input.AsSpan(0, 16));
        BinaryPrimitives.WriteUInt32LittleEndian(input.AsSpan(16, 4), counter);
        input[20] = cmd;
        payload.CopyTo(input.AsSpan(21));
        using var hmac = new HMACSHA256(key.ToArray());
        return hmac.ComputeHash(input).AsSpan(0, ProtocolV2.ControlTagSize).ToArray();
    }

    /// <summary>Frame banao (tag sameet). Payload ≤ 1024 warna ArgumentException.</summary>
    public static byte[] Build(byte cmd, ushort reqId, uint counter,
        ReadOnlySpan<byte> payload, ReadOnlySpan<byte> key, ReadOnlySpan<byte> nonce)
    {
        if (payload.Length > ProtocolV2.ControlMaxPayload)
            throw new ArgumentException("Control payload ≤ 1024 bytes hona chahiye (§3.1).", nameof(payload));
        byte[] tag = ComputeTag(key, nonce, counter, cmd, payload);
        var frame = new byte[ProtocolV2.ControlHeaderSize + payload.Length + ProtocolV2.ControlTagSize];
        frame[0] = (byte)'R'; frame[1] = (byte)'S'; frame[2] = (byte)'H'; frame[3] = (byte)'C';
        frame[4] = cmd;
        BinaryPrimitives.WriteUInt16LittleEndian(frame.AsSpan(5, 2), reqId);
        BinaryPrimitives.WriteUInt32LittleEndian(frame.AsSpan(7, 4), counter);
        BinaryPrimitives.WriteUInt16LittleEndian(frame.AsSpan(11, 2), (ushort)payload.Length);
        payload.CopyTo(frame.AsSpan(ProtocolV2.ControlHeaderSize, payload.Length));
        tag.CopyTo(frame.AsSpan(ProtocolV2.ControlHeaderSize + payload.Length, ProtocolV2.ControlTagSize));
        return frame;
    }

    /// <summary>
    /// Sirf structure parse karo (tag verify NAHI) — BAD_TAG reply ka reqId/counter
    /// parhne ke liye lazmi hai: PC BAD_TAG reply ko sirf tab maanta hai jab
    /// reqId/counter match karein (§3.2).
    /// </summary>
    public static ControlParseResult TryParseStructure(ReadOnlySpan<byte> data, out ControlFrame frame)
    {
        frame = default;
        // TooShort sirf tab jab 13 B ka header hi parha na ja sake (§3.1).
        // Header parh liya gaya ho to har length mismatch (truncation samet)
        // → BadLength (doc comment dekho).
        if (data.Length < ProtocolV2.ControlHeaderSize)
            return ControlParseResult.TooShort;
        if (data[0] != (byte)'R' || data[1] != (byte)'S' || data[2] != (byte)'H' || data[3] != (byte)'C')
            return ControlParseResult.BadMagic;
        int len = BinaryPrimitives.ReadUInt16LittleEndian(data[11..13]);
        if (len > ProtocolV2.ControlMaxPayload) return ControlParseResult.BadLength;
        if (data.Length != ProtocolV2.ControlHeaderSize + len + ProtocolV2.ControlTagSize)
            return ControlParseResult.BadLength;

        frame = new ControlFrame(
            data[4],
            BinaryPrimitives.ReadUInt16LittleEndian(data[5..7]),
            BinaryPrimitives.ReadUInt32LittleEndian(data[7..11]),
            data[13..(13 + len)].ToArray(),
            data[(13 + len)..].ToArray());
        return ControlParseResult.Ok;
    }

    /// <summary>Structure + HMAC tag verify (§3.2). Tag fail → BadTag (reply status BAD_TAG = 1).</summary>
    public static ControlParseResult TryParse(ReadOnlySpan<byte> data,
        ReadOnlySpan<byte> key, ReadOnlySpan<byte> nonce, out ControlFrame frame)
    {
        var result = TryParseStructure(data, out frame);
        if (result != ControlParseResult.Ok) return result;
        CheckKeyAndNonce(key, nonce);
        byte[] expected = ComputeTag(key, nonce, frame.Counter, frame.Cmd, frame.Payload);
        if (!CryptographicOperations.FixedTimeEquals(expected, frame.Tag))
        {
            frame = default;
            return ControlParseResult.BadTag;
        }
        return ControlParseResult.Ok;
    }
}

/// <summary>Duplicate / replay faisla (§3.2): board pichle 32 (reqId, counter) pairs cache rakhta hai.</summary>
public enum ControlSessionDecision
{
    /// <summary>Naya pair, counter &gt; max-seen — execute karo.</summary>
    Accept,
    /// <summary>Wahi (reqId, counter) dobara — cached reply dobara bhejo, dobara execute NAHI.</summary>
    Duplicate,
    /// <summary>Naya reqId lekin counter ≤ max-seen — reply status REPLAY (2).</summary>
    Replay,
}

/// <summary>
/// Per-session counter state (§2.3/§3.2). Counter gap accept hota hai (counter &gt; max-seen
/// theek hai — khoya packet session ko wedge nahi karta). Pehla frame counter 0 se bhi Accept.
/// NOTE: ye receiver (board) side ka rule hai — C# mein host tests + future board-mirror ke liye.
/// </summary>
public sealed class ControlSession
{
    private const int CacheSize = 32;
    private readonly (ushort ReqId, uint Counter)[] _recent = new (ushort, uint)[CacheSize];
    private int _count;
    private int _next;
    private uint _maxSeen;
    private bool _hasSeen;

    public uint MaxSeenCounter => _maxSeen;
    public bool HasSeen => _hasSeen;

    public ControlSessionDecision Classify(ushort reqId, uint counter)
    {
        for (int i = 0; i < _count; i++)
        {
            if (_recent[i].ReqId == reqId && _recent[i].Counter == counter)
                return ControlSessionDecision.Duplicate;
        }
        // Counter-wrap rule (Phase D, SEC-D-07c — PROTOCOL-V2.1.md §3.2
        // amendment): "naya" counter wahi hai jis ka (int)(counter - maxSeen)
        // > 0 ho (RFC 1982 serial arithmetic). 0xFFFFFFFF ke baad 0 bhi naya
        // hai; purana counter wrap ke baad bhi Replay hai. C++ (protocol.h)
        // ke rsh_ctrl_session_classify se byte-identical behaviour.
        if (_hasSeen && (int)(counter - _maxSeen) <= 0) return ControlSessionDecision.Replay;

        _maxSeen = counter;
        _hasSeen = true;
        _recent[_next] = (reqId, counter);
        _next = (_next + 1) % CacheSize;
        if (_count < CacheSize) _count++;
        return ControlSessionDecision.Accept;
    }
}
