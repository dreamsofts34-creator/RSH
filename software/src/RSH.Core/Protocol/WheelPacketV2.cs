using System.Buffers.Binary;

namespace RSH.Core.Protocol;

/// <summary>Stream v2.1 packet parse ka nateeja (§1.4 reject rules).</summary>
public enum WheelPacketV2ParseResult
{
    Ok,
    TooShort,
    BadMagic,
    BadVersion,
    /// <summary>nEnc &gt; 4 ya length != 20 + 2·axes + 2·nEnc + 2.</summary>
    BadFrame,
    BadCrc,
    /// <summary>Reserved bytes 18–19 != 0 ya flags bits 4–7 nonzero (D6).</summary>
    ReservedNonzero,
    /// <summary>Pehle 2 bytes 0xA5 0x01 — legacy v1 packet; v1 parser (WheelPacket) istemal karo.</summary>
    LegacyV1,
}

/// <summary>
/// Data packet v2.1 (§1.1): "RSH2" | ver 0x21 | flags | seq | ts | axisMask | nEnc |
/// buttons u32 | rsv u16=0 | axes u16 × popcount(mask) mV | encoders i16 × nEnc (cumulative) | CRC16.
/// Length = 20 + 2·popcount(axisMask) + 2·nEnc + 2. Sab axes mV mein hain (mvMask khatam, D6).
/// </summary>
public readonly struct WheelPacketV2
{
    public ushort Sequence { get; }
    public uint TimestampUs { get; }
    public byte Flags { get; }
    public byte AxisMask { get; }
    public uint Buttons { get; }

    /// <summary>Axes, axis index par (0–7). Sirf mask mein maujood indices valid hain; baaqi 0.</summary>
    public ushort[] Axes { get; }

    /// <summary>Cumulative encoder counts (i16, wrap allowed — PC delta i16 wrap se nikale, D1).</summary>
    public short[] Encoders { get; }

    public int EncoderCount { get; }
    public int AxisCount => ProtocolV2.PopCount(AxisMask);

    // Flags byte (§1.3, D4)
    public bool ConfigChanged => (Flags & 0x01) != 0;
    public bool LowSupply => (Flags & 0x02) != 0;
    public bool TickSkipped => (Flags & 0x04) != 0;
    public bool AxisInvalid => (Flags & 0x08) != 0;

    public WheelPacketV2(ushort sequence, uint timestampUs, byte flags, byte axisMask,
        uint buttons, ushort[] axes, short[] encoders, int encoderCount)
    {
        Sequence = sequence;
        TimestampUs = timestampUs;
        Flags = flags;
        AxisMask = axisMask;
        Buttons = buttons;
        Axes = axes;
        Encoders = encoders;
        EncoderCount = encoderCount;
    }

    public bool AxisPresent(int index) => index is >= 0 and < 8 && (AxisMask & (1 << index)) != 0;
    public ushort Axis(int index) => Axes[index];

    /// <summary>Button n (0–31): bit n = button n (D1).</summary>
    public bool HasButton(int n) => n is >= 0 and < 32 && (Buttons & (1u << n)) != 0;

    public static WheelPacketV2ParseResult TryParse(ReadOnlySpan<byte> data, out WheelPacketV2 packet)
    {
        packet = default;
        if (data.Length < 2) return WheelPacketV2ParseResult.TooShort;
        // Legacy pehchan (§0, R-H18): pehla byte 0xA5 aur doosra 0x01 → v1 path.
        if (data[0] == 0xA5 && data[1] == 0x01) return WheelPacketV2ParseResult.LegacyV1;
        if (data.Length < 4) return WheelPacketV2ParseResult.TooShort;
        if (data[0] != (byte)'R' || data[1] != (byte)'S' || data[2] != (byte)'H' || data[3] != (byte)'2')
            return WheelPacketV2ParseResult.BadMagic;
        if (data.Length < 5) return WheelPacketV2ParseResult.TooShort;
        if (data[4] != ProtocolV2.Version) return WheelPacketV2ParseResult.BadVersion;
        if (data.Length < ProtocolV2.StreamHeaderSize) return WheelPacketV2ParseResult.TooShort;

        byte flags = data[5];
        byte axisMask = data[12];
        byte nEnc = data[13];
        if (nEnc > ProtocolV2.MaxEncoders) return WheelPacketV2ParseResult.BadFrame;
        int expected = ProtocolV2.StreamHeaderSize
            + 2 * ProtocolV2.PopCount(axisMask) + 2 * nEnc + 2;
        if (data.Length != expected) return WheelPacketV2ParseResult.BadFrame;
        if (BinaryPrimitives.ReadUInt16LittleEndian(data[18..20]) != 0) return WheelPacketV2ParseResult.ReservedNonzero;
        if ((flags & 0xF0) != 0) return WheelPacketV2ParseResult.ReservedNonzero; // flags bits 4–7 reserved 0 (§1.3)

        ushort storedCrc = BinaryPrimitives.ReadUInt16LittleEndian(data[(data.Length - 2)..]);
        if (Crc16.Compute(data[..(data.Length - 2)]) != storedCrc) return WheelPacketV2ParseResult.BadCrc;

        var axes = new ushort[8];
        int offset = ProtocolV2.StreamHeaderSize;
        for (int i = 0; i < 8; i++)
        {
            if ((axisMask & (1 << i)) == 0) continue;
            axes[i] = BinaryPrimitives.ReadUInt16LittleEndian(data[offset..(offset + 2)]);
            offset += 2;
        }
        var encoders = new short[4];
        for (int i = 0; i < nEnc; i++)
        {
            encoders[i] = BinaryPrimitives.ReadInt16LittleEndian(data[offset..(offset + 2)]);
            offset += 2;
        }

        packet = new WheelPacketV2(
            BinaryPrimitives.ReadUInt16LittleEndian(data[6..8]),
            BinaryPrimitives.ReadUInt32LittleEndian(data[8..12]),
            flags,
            axisMask,
            BinaryPrimitives.ReadUInt32LittleEndian(data[14..18]),
            axes,
            encoders,
            nEnc);
        return WheelPacketV2ParseResult.Ok;
    }

    /// <summary>
    /// Stream v2.1 packet banao (parser ka aaina — roundtrip tests aur host-side sender ke liye).
    /// axesInMaskOrder: axisMask mein set bits ki ascending order mein mV values.
    /// </summary>
    public static byte[] Build(ushort sequence, uint timestampUs, byte flags, byte axisMask,
        uint buttons, ReadOnlySpan<ushort> axesInMaskOrder, ReadOnlySpan<short> encoders)
    {
        if ((flags & 0xF0) != 0)
            throw new ArgumentException("Flags bits 4–7 reserved hain, 0 hone chahiye.", nameof(flags));
        if (axesInMaskOrder.Length != ProtocolV2.PopCount(axisMask))
            throw new ArgumentException("Axes ki ginti axisMask ke popcount se match karni chahiye.", nameof(axesInMaskOrder));
        if (encoders.Length > ProtocolV2.MaxEncoders)
            throw new ArgumentException("nEnc ≤ 4 hona chahiye.", nameof(encoders));

        int total = ProtocolV2.StreamHeaderSize + 2 * axesInMaskOrder.Length + 2 * encoders.Length + 2;
        var bytes = new byte[total];
        bytes[0] = (byte)'R'; bytes[1] = (byte)'S'; bytes[2] = (byte)'H'; bytes[3] = (byte)'2';
        bytes[4] = ProtocolV2.Version;
        bytes[5] = flags;
        BinaryPrimitives.WriteUInt16LittleEndian(bytes.AsSpan(6, 2), sequence);
        BinaryPrimitives.WriteUInt32LittleEndian(bytes.AsSpan(8, 4), timestampUs);
        bytes[12] = axisMask;
        bytes[13] = (byte)encoders.Length;
        BinaryPrimitives.WriteUInt32LittleEndian(bytes.AsSpan(14, 4), buttons);
        // bytes 18–19 reserved = 0 (array pehle se zero hai).
        int offset = ProtocolV2.StreamHeaderSize;
        foreach (ushort axis in axesInMaskOrder)
        {
            BinaryPrimitives.WriteUInt16LittleEndian(bytes.AsSpan(offset, 2), axis);
            offset += 2;
        }
        foreach (short enc in encoders)
        {
            BinaryPrimitives.WriteInt16LittleEndian(bytes.AsSpan(offset, 2), enc);
            offset += 2;
        }
        ushort crc = Crc16.Compute(bytes.AsSpan(0, total - 2));
        BinaryPrimitives.WriteUInt16LittleEndian(bytes.AsSpan(total - 2, 2), crc);
        return bytes;
    }
}
