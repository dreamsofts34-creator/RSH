// RSH Protocol v2.1 — shared constants, error codes, packet routing (WP3, Phase A).
// Authority: docs/PROTOCOL-V2.1.md. Koi design decision yahan nahi badli gayi —
// sirf us doc ke byte-level faisle code mein likhe gaye hain.
// Product naam RSH hai.
namespace RSH.Core.Protocol;

/// <summary>Protocol v2.1 ke shared constants (PROTOCOL-V2.1.md §0–§1).</summary>
public static class ProtocolV2
{
    /// <summary>Wire version byte: major 2, minor 1 → 0x21 (§0).</summary>
    public const byte Version = 0x21;

    public const int MaxAxes = 8;
    public const int MaxEncoders = 4;

    /// <summary>Wire par buttons 0–63 aate hain (§1.1 N1).</summary>
    public const int MaxButtonsWire = 64;

    /// <summary>Config ceiling: nBtn ≤ 128 (§0 bounds, §1.1 N1).</summary>
    public const int MaxButtonsConfig = 128;

    public const int StatusBaseSize = 34;
    public const int StatusExtendedSize = 38;
    public const int HelloSize = 92;

    public const int ControlHeaderSize = 13;
    public const int ControlTagSize = 16;
    public const int ControlMaxPayload = 1024;
    public const int ControlMaxFrame = ControlHeaderSize + ControlMaxPayload + ControlTagSize; // 1053 (§3.1)

    /// <summary>Ping port — control port 4212 hi hai, magic "RSHN" se demux (§9).</summary>
    public const int PingPort = 4212;
    /// <summary>Control port — HELO (4210) aur data (4211) se alag; ping bhi isi par hai (§3.1/§9).</summary>
    public const int ControlPort = 4212;
    /// <summary>Ping frame fixed size: magic(4) + ver + type + seq(2) + t0/t1/t2 (8×3) + CRC16(2) = 34 (§9).</summary>
    public const int PingFrameSize = 34;

    /// <summary>Stream header size: magic(4) + ver + flags + seq + ts + mask + nEnc + buttons(4) + rsv(2) = 20 (§1.1).</summary>
    public const int StreamHeaderSize = 20;

    public static int PopCount(byte mask)
    {
        int n = 0;
        while (mask != 0) { n += mask & 1; mask >>= 1; }
        return n;
    }

    /// <summary>
    /// Configured button count ka faisla (§1.1 N1): wire sirf 0–63 cover karta hai.
    /// nBtn 65–128 SET_CFG par UNSUPPORTED se reject hota hai; nBtn &gt; 128 hamesha BAD_FRAME hai.
    /// </summary>
    public static ButtonCountResult CheckButtonCount(int nBtn)
    {
        if (nBtn < 0 || nBtn > MaxButtonsConfig) return ButtonCountResult.BadFrame;
        if (nBtn > MaxButtonsWire) return ButtonCountResult.Unsupported;
        return ButtonCountResult.Ok;
    }
}

public enum ButtonCountResult
{
    Ok,
    /// <summary>nBtn 65–128: config ceiling ke andar, wire slot nahi — SET_CFG UNSUPPORTED (§1.1 N1).</summary>
    Unsupported,
    /// <summary>nBtn &gt; 128: parser bound, BAD_FRAME (§0).</summary>
    BadFrame,
}

/// <summary>Control reply status codes (§3.9) — numeric table hi authority hai.</summary>
public enum ControlStatus : byte
{
    Ok = 0,
    BadTag = 1,
    Replay = 2,
    BadCrc = 3,
    TooBig = 4,
    PinConflict = 5,
    Unsupported = 6,
    Busy = 7,
}

/// <summary>Control commands (§3.3). Reply mein cmd = reqCmd | 0x80.</summary>
public enum ControlCommand : byte
{
    GetCaps = 0x01,
    GetCfg = 0x02,
    SetCfg = 0x03,
    Commit = 0x04,
    Confirm = 0x05,
    Revert = 0x06,
    PinProbe = 0x07,
    FactoryReset = 0x08,
    Reboot = 0x09,
    LedTest = 0x0A,
    KeyReset = 0x0B,
    SetPassword = 0x0C,
}

public enum PacketKind
{
    Unknown,
    LegacyV1,
    StreamV2,
    StatusV2,
    HelloV2,
    ControlV2,
    ProbeV2,
    PingV2,
}

/// <summary>
/// Pehle bytes se packet ki qism pehchanna (§1.4, §2): legacy v1 = 0xA5 0x01,
/// stream = "RSH2", status = 0xA6, HELLO = "RSHR", control = "RSHC", probe = "RSHP".
/// Ye sirf routing hai — validation har parser khud karta hai.
/// </summary>
public static class PacketRouter
{
    public static PacketKind Classify(ReadOnlySpan<byte> data)
    {
        if (data.Length >= 2 && data[0] == 0xA5 && data[1] == 0x01) return PacketKind.LegacyV1;
        if (data.Length >= 1 && data[0] == 0xA6) return PacketKind.StatusV2;
        if (data.Length < 4) return PacketKind.Unknown;
        if (data[0] == (byte)'R' && data[1] == (byte)'S' && data[2] == (byte)'H')
        {
            return data[3] switch
            {
                (byte)'2' => PacketKind.StreamV2,
                (byte)'R' => PacketKind.HelloV2,
                (byte)'C' => PacketKind.ControlV2,
                (byte)'P' => PacketKind.ProbeV2,
                (byte)'N' => PacketKind.PingV2,
                _ => PacketKind.Unknown,
            };
        }
        return PacketKind.Unknown;
    }
}
