using System.Buffers.Binary;

namespace RSH.Core.Protocol;

/// <summary>Parse ke nateeje — nakami ki wajah alag alag gin'i jati hai (diagnostics).</summary>
public enum PacketParseResult
{
    Ok,
    TooShort,
    BadMagic,
    BadVersion,
    BadCrc,
}

/// <summary>
/// Ek parsed wheel packet. Layout firmware protocol.h ke sath fixed hai:
/// 18 bytes, little-endian. Steering milliVolts mein hota hai jab
/// <see cref="SteeringInMillivolts"/> true ho, warna raw ADC (0..4095).
/// </summary>
public readonly record struct WheelPacket(
    ushort Sequence,
    uint EspTimestampUs,
    ushort Steering,
    ushort Accel,
    ushort Brake,
    byte Buttons,
    bool SteeringInMillivolts)
{
    public const byte Magic = 0xA5;
    public const byte Version = 1;
    public const int PacketSize = 18;

    // Button bits (firmware ke sath ek)
    public const byte ButtonGearUp = 0x01;
    public const byte ButtonGearDown = 0x02;
    public const byte ButtonAccel = 0x04;
    public const byte ButtonBrake = 0x08;

    public bool GearUp => (Buttons & ButtonGearUp) != 0;
    public bool GearDown => (Buttons & ButtonGearDown) != 0;
    public bool AccelButton => (Buttons & ButtonAccel) != 0;
    public bool BrakeButton => (Buttons & ButtonBrake) != 0;

    public static PacketParseResult TryParse(ReadOnlySpan<byte> data, out WheelPacket packet)
    {
        packet = default;
        if (data.Length < PacketSize) return PacketParseResult.TooShort;
        if (data[0] != Magic) return PacketParseResult.BadMagic;
        if (data[1] != Version) return PacketParseResult.BadVersion;

        ushort storedCrc = BinaryPrimitives.ReadUInt16LittleEndian(data[16..18]);
        if (Crc16.Compute(data[..16]) != storedCrc) return PacketParseResult.BadCrc;

        packet = new WheelPacket(
            Sequence: BinaryPrimitives.ReadUInt16LittleEndian(data[2..4]),
            EspTimestampUs: BinaryPrimitives.ReadUInt32LittleEndian(data[4..8]),
            Steering: BinaryPrimitives.ReadUInt16LittleEndian(data[8..10]),
            Accel: BinaryPrimitives.ReadUInt16LittleEndian(data[10..12]),
            Brake: BinaryPrimitives.ReadUInt16LittleEndian(data[12..14]),
            Buttons: data[14],
            SteeringInMillivolts: (data[15] & 0x01) != 0);
        return PacketParseResult.Ok;
    }

    /// <summary>Packet wapas bytes mein — Simulator aur tests ke liye (firmware build ka aaina).</summary>
    public void WriteTo(Span<byte> destination)
    {
        if (destination.Length < PacketSize)
            throw new ArgumentException($"Destination kam se kam {PacketSize} bytes ka ho.", nameof(destination));

        destination[0] = Magic;
        destination[1] = Version;
        BinaryPrimitives.WriteUInt16LittleEndian(destination[2..4], Sequence);
        BinaryPrimitives.WriteUInt32LittleEndian(destination[4..8], EspTimestampUs);
        BinaryPrimitives.WriteUInt16LittleEndian(destination[8..10], Steering);
        BinaryPrimitives.WriteUInt16LittleEndian(destination[10..12], Accel);
        BinaryPrimitives.WriteUInt16LittleEndian(destination[12..14], Brake);
        destination[14] = Buttons;
        destination[15] = SteeringInMillivolts ? (byte)0x01 : (byte)0x00;
        ushort crc = Crc16.Compute(destination[..16]);
        BinaryPrimitives.WriteUInt16LittleEndian(destination[16..18], crc);
    }

    public byte[] ToBytes()
    {
        var bytes = new byte[PacketSize];
        WriteTo(bytes);
        return bytes;
    }
}
