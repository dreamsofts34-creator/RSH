using System.Buffers.Binary;

namespace RSH.Core.Protocol;

/// <summary>HELLO reply (92 B fixed, §2.2) parse ka nateeja.</summary>
public enum HelloReplyParseResult
{
    Ok,
    TooShort,
    BadMagic,
    BadVersion,
    BadLength,
    BadCrc,
    /// <summary>Reserved byte 17 != 0 ya flags bits 4–7 nonzero.</summary>
    ReservedNonzero,
}

/// <summary>
/// HELLO reply (§2.2): "RSHR" | proto 2.1 | flags | sessionEcho | boardMac 6 B |
/// fw 4.0.0 | rsv | nonce 16 B | deviceKey 32 B | wifiPassword 24 B | CRC16. Total 92 B fixed.
/// deviceKey/wifiPassword sirf unpaired + pairing window ke andar valid hain,
/// warna sab 0x00 (§2.2/§2.3). Key/password is se derive kabhi nahi hote (R-H10).
/// </summary>
public readonly struct HelloReply
{
    public byte Flags { get; }
    public byte SessionEcho { get; }
    public byte[] BoardMac { get; }
    public byte FwMajor { get; }
    public byte FwMinor { get; }
    public byte FwPatch { get; }
    public byte[] Nonce { get; }
    public byte[] DeviceKey { get; }
    public byte[] WifiPassword { get; }

    // Flags byte (§2.2)
    public bool Unpaired => (Flags & 0x01) != 0;
    public bool KeyResetArmed => (Flags & 0x02) != 0;
    public bool SafeMode => (Flags & 0x04) != 0;
    public bool PairingWindowOpen => (Flags & 0x08) != 0;

    public HelloReply(byte flags, byte sessionEcho, byte[] boardMac,
        byte fwMajor, byte fwMinor, byte fwPatch,
        byte[] nonce, byte[] deviceKey, byte[] wifiPassword)
    {
        Flags = flags;
        SessionEcho = sessionEcho;
        BoardMac = boardMac;
        FwMajor = fwMajor;
        FwMinor = fwMinor;
        FwPatch = fwPatch;
        Nonce = nonce;
        DeviceKey = deviceKey;
        WifiPassword = wifiPassword;
    }

    public static HelloReplyParseResult TryParse(ReadOnlySpan<byte> data, out HelloReply reply)
    {
        reply = default;
        if (data.Length < 6) return HelloReplyParseResult.TooShort;
        if (data[0] != (byte)'R' || data[1] != (byte)'S' || data[2] != (byte)'H' || data[3] != (byte)'R')
            return HelloReplyParseResult.BadMagic;
        if (data[4] != 2 || data[5] != 1) return HelloReplyParseResult.BadVersion;
        if (data.Length < ProtocolV2.HelloSize) return HelloReplyParseResult.TooShort;
        if (data.Length != ProtocolV2.HelloSize) return HelloReplyParseResult.BadLength;
        if (data[17] != 0) return HelloReplyParseResult.ReservedNonzero;
        if ((data[6] & 0xF0) != 0) return HelloReplyParseResult.ReservedNonzero;

        ushort storedCrc = BinaryPrimitives.ReadUInt16LittleEndian(data[90..92]);
        if (Crc16.Compute(data[..90]) != storedCrc) return HelloReplyParseResult.BadCrc;

        reply = new HelloReply(
            data[6],
            data[7],
            data[8..14].ToArray(),
            data[14], data[15], data[16],
            data[18..34].ToArray(),
            data[34..66].ToArray(),
            data[66..90].ToArray());
        return HelloReplyParseResult.Ok;
    }
}

/// <summary>PIN_PROBE stream frame (§3.8) parse ka nateeja.</summary>
public enum ProbeFrameParseResult
{
    Ok,
    TooShort,
    BadMagic,
    BadVersion,
    /// <summary>mode &gt; 1, sampleCount 0 ya &gt; 64, ya length samples se match na kare.</summary>
    BadFrame,
    BadCrc,
}

/// <summary>
/// PIN_PROBE frame (§3.8): "RSHP" | ver 0x21 | seq | pin | mode | sampleCount |
/// samples (mode 0: u8 × N level 0/1; mode 1: u16 LE × N mV) | CRC16.
/// Probe ke dauraan normal data packets rukte nahi — alag magic se aate hain.
/// </summary>
public readonly struct ProbeFrame
{
    public ushort Sequence { get; }
    public byte Pin { get; }
    /// <summary>0 = digital level, 1 = ADC mV.</summary>
    public byte Mode { get; }
    public int SampleCount { get; }
    /// <summary>Mode 0 samples (0/1). Sirf pehle SampleCount entries valid.</summary>
    public byte[] SamplesLevel { get; }
    /// <summary>Mode 1 samples (mV). Sirf pehle SampleCount entries valid.</summary>
    public ushort[] SamplesMv { get; }

    public ProbeFrame(ushort sequence, byte pin, byte mode, int sampleCount,
        byte[] samplesLevel, ushort[] samplesMv)
    {
        Sequence = sequence;
        Pin = pin;
        Mode = mode;
        SampleCount = sampleCount;
        SamplesLevel = samplesLevel;
        SamplesMv = samplesMv;
    }

    public static ProbeFrameParseResult TryParse(ReadOnlySpan<byte> data, out ProbeFrame frame)
    {
        frame = default;
        if (data.Length < 6) return ProbeFrameParseResult.TooShort;
        if (data[0] != (byte)'R' || data[1] != (byte)'S' || data[2] != (byte)'H' || data[3] != (byte)'P')
            return ProbeFrameParseResult.BadMagic;
        if (data[4] != ProtocolV2.Version) return ProbeFrameParseResult.BadVersion;
        if (data.Length < 11) return ProbeFrameParseResult.TooShort;

        byte mode = data[8];
        int sampleCount = BinaryPrimitives.ReadUInt16LittleEndian(data[9..11]);
        if (mode > 1) return ProbeFrameParseResult.BadFrame;
        if (sampleCount < 1 || sampleCount > 64) return ProbeFrameParseResult.BadFrame;
        int expected = 11 + sampleCount * (mode == 1 ? 2 : 1) + 2;
        if (data.Length != expected) return ProbeFrameParseResult.BadFrame;

        ushort storedCrc = BinaryPrimitives.ReadUInt16LittleEndian(data[(data.Length - 2)..]);
        if (Crc16.Compute(data[..(data.Length - 2)]) != storedCrc) return ProbeFrameParseResult.BadCrc;

        var levels = new byte[64];
        var mvs = new ushort[64];
        if (mode == 0)
        {
            data.Slice(11, sampleCount).CopyTo(levels.AsSpan(0, sampleCount));
        }
        else
        {
            for (int i = 0; i < sampleCount; i++)
                mvs[i] = BinaryPrimitives.ReadUInt16LittleEndian(data[(11 + 2 * i)..(13 + 2 * i)]);
        }

        frame = new ProbeFrame(
            BinaryPrimitives.ReadUInt16LittleEndian(data[5..7]),
            data[7],
            mode,
            sampleCount,
            levels,
            mvs);
        return ProbeFrameParseResult.Ok;
    }
}
