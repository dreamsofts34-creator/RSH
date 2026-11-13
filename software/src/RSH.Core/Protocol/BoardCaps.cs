// RSH Protocol v2.1 — GET_CAPS reply body (§3.4), PC-side client model.
// Authority: docs/PROTOCOL-V2.1.md §3.3/§3.4. BoardCaps sirf parse + compare
// karta hai — koi network I/O nahi (wo GetCapsClient ka kaam hai), is liye
// ye unit test mein baghair socket ke test ho sakta hai.
//
// Reply payload layout (status byte ke baad, §3.4):
//   status u8 | boardMac 6 B | fwMajor/Minor/Patch | protoMajor/Minor |
//   nAxesMax | nBtnMax | nEncMax | wifiChannel | capsFlags | pinCount |
//   pin table (8 B × N). Total = 18 + 8·N, N ≤ 40.
// SEC-C-05 (anti-downgrade): image version ka muqabla isi se hota hai
// (FirmwareUploadGate) — GET_CAPS ke baghair sirf floor v4.0.0 check
// ho sakta hai, live board version ka muqabla nahi.
using System.Buffers.Binary;

namespace RSH.Core.Protocol;

/// <summary>GET_CAPS pin table entry ki qism (§3.4.1 kind codes).</summary>
public enum CapsPinKind : byte
{
    None = 0,
    AdcNative = 1,
    AdcAds1115 = 2,
    DigitalIn = 3,
    DigitalOut = 4,
    LedStrip = 5,
    I2cSda = 6,
    I2cScl = 7,
    ExpanderMcp23017 = 8,
    ExpanderPcf8574 = 9,
    AxisGeneric = 10,
    ReservedPin = 11,
}

/// <summary>GET_CAPS pin table entry ka module/role (§3.4.1 module codes).</summary>
public enum CapsPinModule : byte
{
    None = 0,
    Steering = 1,
    Accelerator = 2,
    Brake = 3,
    Clutch = 4,
    Handbrake = 5,
    Shifter = 6,
    Horn = 7,
    Buttons = 8,
    Encoders = 9,
    Led = 10,
    Battery = 11,
}

/// <summary>GET_CAPS pin table ki ek entry (8 B, §3.4).</summary>
public readonly record struct CapsPinEntry(
    byte Pin,
    CapsPinKind Kind,
    CapsPinModule Module,
    byte Caps)
{
    /// <summary>caps bits: bit0 input, bit1 output, bit2 adc, bit3 pwm, bit4 pullup.</summary>
    public bool IsInput => (Caps & 0x01) != 0;
    public bool IsOutput => (Caps & 0x02) != 0;
    public bool IsAdc => (Caps & 0x04) != 0;
    public bool IsPwm => (Caps & 0x08) != 0;
    public bool HasPullup => (Caps & 0x10) != 0;
}

/// <summary>
/// Board ki capabilities (GET_CAPS reply, §3.4). Immutable — parse ke
/// baad badalti nahi. Version compare ke liye <see cref="FirmwareVersion"/>
/// (System.Version) istemal karo, kabhi raw bytes ka lexical compare nahi.
/// </summary>
public sealed record BoardCaps(
    byte[] BoardMac,
    byte FwMajor,
    byte FwMinor,
    byte FwPatch,
    byte ProtoMajor,
    byte ProtoMinor,
    byte NAxesMax,
    byte NBtnMax,
    byte NEncMax,
    byte WifiChannel,
    byte CapsFlags,
    IReadOnlyList<CapsPinEntry> PinTable)
{
    /// <summary>Reply body ka fixed hissa (status samet): 18 B (§3.4).</summary>
    public const int FixedBodySize = 18;
    /// <summary>Pin table entries ki zyada se zyada tadad (§3.4: 0..40).</summary>
    public const int MaxPinCount = 40;

    /// <summary>Firmware version, compare ke qaabil (SEC-C-05).</summary>
    public Version FirmwareVersion => new(FwMajor, FwMinor, FwPatch);
    /// <summary>Protocol version (2.1 aana chahiye).</summary>
    public Version ProtocolVersion => new(ProtoMajor, ProtoMinor);

    // capsFlags bits (§3.4)
    public bool SlowAxis => (CapsFlags & 0x01) != 0;
    public bool SafeMode => (CapsFlags & 0x02) != 0;
    public bool Unpaired => (CapsFlags & 0x04) != 0;
    public bool KeyResetArmed => (CapsFlags & 0x08) != 0;

    /// <summary>
    /// Reply payload (status byte samet) parse karo. Status != OK ho,
    /// length pinCount se match na kare, ya reserved bits set hon to
    /// false + wajah — aisi reply kabhi BoardCaps nahi banti (D6 strictness:
    /// reserved nonzero → drop).
    /// </summary>
    public static bool TryParse(ReadOnlySpan<byte> payload, out BoardCaps? caps, out string? error)
    {
        caps = null;
        error = null;
        if (payload.Length < FixedBodySize)
        {
            error = $"GET_CAPS reply bohat chhota hai ({payload.Length} B, kam se kam {FixedBodySize} B).";
            return false;
        }
        var status = (ControlStatus)payload[0];
        if (status != ControlStatus.Ok)
        {
            error = $"Board ne GET_CAPS par {status} ({(byte)status}) diya — capabilities nahi mili.";
            return false;
        }
        byte pinCount = payload[17];
        if (pinCount > MaxPinCount)
        {
            error = $"Pin table count na-jaiz hai ({pinCount}, zyada se zyada {MaxPinCount}).";
            return false;
        }
        if (payload.Length != FixedBodySize + 8 * pinCount)
        {
            error = $"GET_CAPS reply length ({payload.Length} B) pinCount ({pinCount}) se match nahi karti (chahiye {FixedBodySize + 8 * pinCount} B).";
            return false;
        }
        byte capsFlags = payload[16];
        if ((capsFlags & 0xF0) != 0)
        {
            error = "capsFlags ke reserved bits set hain — reply drop ki gayi.";
            return false;
        }

        var pins = new List<CapsPinEntry>(pinCount);
        for (int i = 0; i < pinCount; i++)
        {
            int off = FixedBodySize + 8 * i;
            byte pin = payload[off];
            byte kind = payload[off + 1];
            byte module = payload[off + 2];
            byte pinCaps = payload[off + 3];
            if (kind > (byte)CapsPinKind.ReservedPin)
            {
                error = $"Pin entry {i}: kind code na-jaiz hai ({kind}).";
                return false;
            }
            if (module > (byte)CapsPinModule.Battery)
            {
                error = $"Pin entry {i}: module code na-jaiz hai ({module}).";
                return false;
            }
            if ((pinCaps & 0xE0) != 0)
            {
                error = $"Pin entry {i}: caps ke reserved bits set hain — reply drop ki gayi.";
                return false;
            }
            if (payload[off + 4] != 0 || payload[off + 5] != 0 || payload[off + 6] != 0 || payload[off + 7] != 0)
            {
                error = $"Pin entry {i}: reserved bytes nonzero hain — reply drop ki gayi.";
                return false;
            }
            pins.Add(new CapsPinEntry(pin, (CapsPinKind)kind, (CapsPinModule)module, pinCaps));
        }

        caps = new BoardCaps(
            payload.Slice(1, 6).ToArray(),
            payload[7], payload[8], payload[9],
            payload[10], payload[11],
            payload[12], payload[13], payload[14],
            payload[15], capsFlags, pins);
        return true;
    }

    /// <summary>Is caps ke khilaf image version downgrade hai? (SEC-C-05: kam = reject.)</summary>
    public bool IsDowngrade(Version imageVersion) => imageVersion < FirmwareVersion;
}
