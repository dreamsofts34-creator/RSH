using System.Buffers.Binary;

namespace RSH.Core.Protocol;

/// <summary>Status packet v2.1 parse ka nateeja (§1.2).</summary>
public enum StatusPacketV2ParseResult
{
    Ok,
    TooShort,
    BadMagic,
    BadVersion,
    /// <summary>Length 34 ya 38 ke ilawa kuch aur (base 34 B, extended 38 B).</summary>
    BadLength,
    BadCrc,
}

/// <summary>
/// Status packet v2.1 (§1.2), 1 Hz, port 4211.
/// Base 34 B (nBtn ≤ 32). Extended 38 B (nBtn 33–64): btnsEx u32 bytes 28–31 mein,
/// seqGap 32–33, txFail 34–35, CRC 36–37.
/// Base offsets (coordinator decision — PROTOCOL-V2.1.md §1.2 hi authority):
/// seqGap 28–29, txFail 30–31, CRC 32–33 (CRC-last invariant, §0/D2).
/// </summary>
public readonly struct StatusPacketV2
{
    public ushort Sequence { get; }
    public uint UptimeMs { get; }
    public sbyte RssiDbm { get; }
    public byte BatteryPercent { get; }
    public ushort SupplyMv { get; }
    public uint FreeHeapBytes { get; }
    public ushort Overruns { get; }
    public ushort MaxCycleUs { get; }
    public byte Governor { get; }
    public byte ResetReason { get; }
    public uint CfgCrc { get; }
    public ushort CfgVer { get; }
    /// <summary>Boot se skip hue stream sequence numbers ki ginti (D5). Wrap allowed.</summary>
    public ushort SeqGap { get; }
    /// <summary>Boot se failed UDP transmissions ki ginti (D5/F3). Wrap allowed.</summary>
    public ushort TxFail { get; }
    /// <summary>Buttons 32–63: bit (n−32) = button n (D1). Sirf extended (38 B) packet mein maujood.</summary>
    public uint BtnsEx { get; }
    public bool HasBtnsEx { get; }

    public StatusPacketV2(ushort sequence, uint uptimeMs, sbyte rssiDbm, byte batteryPercent,
        ushort supplyMv, uint freeHeapBytes, ushort overruns, ushort maxCycleUs, byte governor,
        byte resetReason, uint cfgCrc, ushort cfgVer, ushort seqGap, ushort txFail,
        uint btnsEx, bool hasBtnsEx)
    {
        Sequence = sequence;
        UptimeMs = uptimeMs;
        RssiDbm = rssiDbm;
        BatteryPercent = batteryPercent;
        SupplyMv = supplyMv;
        FreeHeapBytes = freeHeapBytes;
        Overruns = overruns;
        MaxCycleUs = maxCycleUs;
        Governor = governor;
        ResetReason = resetReason;
        CfgCrc = cfgCrc;
        CfgVer = cfgVer;
        SeqGap = seqGap;
        TxFail = txFail;
        BtnsEx = btnsEx;
        HasBtnsEx = hasBtnsEx;
    }

    /// <summary>Button n (32–63, sirf extended packet mein): bit (n−32) = button n.</summary>
    public bool HasButtonEx(int n) => HasBtnsEx && n is >= 32 and < 64 && (BtnsEx & (1u << (n - 32))) != 0;

    public static StatusPacketV2ParseResult TryParse(ReadOnlySpan<byte> data, out StatusPacketV2 packet)
    {
        packet = default;
        if (data.Length < 2) return StatusPacketV2ParseResult.TooShort;
        if (data[0] != 0xA6) return StatusPacketV2ParseResult.BadMagic;
        if (data[1] != ProtocolV2.Version) return StatusPacketV2ParseResult.BadVersion;
        if (data.Length < ProtocolV2.StatusBaseSize) return StatusPacketV2ParseResult.TooShort;
        if (data.Length != ProtocolV2.StatusBaseSize && data.Length != ProtocolV2.StatusExtendedSize)
            return StatusPacketV2ParseResult.BadLength;

        ushort storedCrc = BinaryPrimitives.ReadUInt16LittleEndian(data[(data.Length - 2)..]);
        if (Crc16.Compute(data[..(data.Length - 2)]) != storedCrc) return StatusPacketV2ParseResult.BadCrc;

        bool extended = data.Length == ProtocolV2.StatusExtendedSize;
        // Extended mein btnsEx 28–31, seqGap 32–33, txFail 34–35; base mein seqGap 28–29, txFail 30–31.
        int seqGapOff = extended ? 32 : 28;
        packet = new StatusPacketV2(
            BinaryPrimitives.ReadUInt16LittleEndian(data[2..4]),
            BinaryPrimitives.ReadUInt32LittleEndian(data[4..8]),
            (sbyte)data[8],
            data[9],
            BinaryPrimitives.ReadUInt16LittleEndian(data[10..12]),
            BinaryPrimitives.ReadUInt32LittleEndian(data[12..16]),
            BinaryPrimitives.ReadUInt16LittleEndian(data[16..18]),
            BinaryPrimitives.ReadUInt16LittleEndian(data[18..20]),
            data[20],
            data[21],
            BinaryPrimitives.ReadUInt32LittleEndian(data[22..26]),
            BinaryPrimitives.ReadUInt16LittleEndian(data[26..28]),
            BinaryPrimitives.ReadUInt16LittleEndian(data[seqGapOff..(seqGapOff + 2)]),
            BinaryPrimitives.ReadUInt16LittleEndian(data[(seqGapOff + 2)..(seqGapOff + 4)]),
            extended ? BinaryPrimitives.ReadUInt32LittleEndian(data[28..32]) : 0,
            extended);
        return StatusPacketV2ParseResult.Ok;
    }

    /// <summary>Status packet banao (parser ka aaina). btnsEx null = base 34 B; value = extended 38 B.</summary>
    public static byte[] Build(ushort sequence, uint uptimeMs, sbyte rssiDbm, byte batteryPercent,
        ushort supplyMv, uint freeHeapBytes, ushort overruns, ushort maxCycleUs, byte governor,
        byte resetReason, uint cfgCrc, ushort cfgVer, ushort seqGap, ushort txFail, uint? btnsEx)
    {
        bool extended = btnsEx.HasValue;
        int total = extended ? ProtocolV2.StatusExtendedSize : ProtocolV2.StatusBaseSize;
        var bytes = new byte[total];
        bytes[0] = 0xA6;
        bytes[1] = ProtocolV2.Version;
        BinaryPrimitives.WriteUInt16LittleEndian(bytes.AsSpan(2, 2), sequence);
        BinaryPrimitives.WriteUInt32LittleEndian(bytes.AsSpan(4, 4), uptimeMs);
        bytes[8] = (byte)rssiDbm;
        bytes[9] = batteryPercent;
        BinaryPrimitives.WriteUInt16LittleEndian(bytes.AsSpan(10, 2), supplyMv);
        BinaryPrimitives.WriteUInt32LittleEndian(bytes.AsSpan(12, 4), freeHeapBytes);
        BinaryPrimitives.WriteUInt16LittleEndian(bytes.AsSpan(16, 2), overruns);
        BinaryPrimitives.WriteUInt16LittleEndian(bytes.AsSpan(18, 2), maxCycleUs);
        bytes[20] = governor;
        bytes[21] = resetReason;
        BinaryPrimitives.WriteUInt32LittleEndian(bytes.AsSpan(22, 4), cfgCrc);
        BinaryPrimitives.WriteUInt16LittleEndian(bytes.AsSpan(26, 2), cfgVer);
        int seqGapOff = extended ? 32 : 28;
        if (extended) BinaryPrimitives.WriteUInt32LittleEndian(bytes.AsSpan(28, 4), btnsEx!.Value);
        BinaryPrimitives.WriteUInt16LittleEndian(bytes.AsSpan(seqGapOff, 2), seqGap);
        BinaryPrimitives.WriteUInt16LittleEndian(bytes.AsSpan(seqGapOff + 2, 2), txFail);
        ushort crc = Crc16.Compute(bytes.AsSpan(0, total - 2));
        BinaryPrimitives.WriteUInt16LittleEndian(bytes.AsSpan(total - 2, 2), crc);
        return bytes;
    }
}
