// Config blob (PROTOCOL-V2.1.md §3.6) — SET_CFG ke zariye board ko bheja
// jaane wala pin/config blob: header + fixed 16 B module records + CRC32
// trailer. Ye builder PC-side hai (firmware config_transaction.h ka
// rsh_cfg_blob_build / rsh_cfg_blob_validate jaisa — wahi layout, wahi CRC).
//
// Header (8 B): 'RSHG' | schema u8 (= 2) | moduleCount u8 | cfgVersion u16 (LE).
//   (magic 4 B + schema 1 B + moduleCount 1 B + cfgVersion 2 B = 8 B;
//   koi flags/reserved byte nahi — Build() neeche wahi likhta hai.)
// Record (16 B): type u8 | slot u8 | flags u8 | pinA u8 | pinB u8 | pinC u8
//   | params[10]. Pin 0xFF = istemal nahi.
// Trailer (4 B): CRC32 (ISO-HDLC) header + records par, little-endian.
// Types (§3.6): 1 AXIS_ADC · 2 AXIS_ADS1115 · 3 BUTTON · 4 BUTTON_MATRIX ·
//   5 ENCODER · 6 LED_STRIP · 7 BATTERY · 8 EXPANDER_MCP23017 ·
//   9 EXPANDER_PCF8574 · 10 AXIS_GENERIC · 11 SHIFTER (Phase F WS5 —
//   gate mode; neeche ShifterRecord dekho).
// Had: total ≤ 2048 B (§3.5), moduleCount ≤ 127 (firmware RSH_CFG_MAX_MODULES).
using RSH.Core.Pipeline;

namespace RSH.Core.Protocol;

/// <summary>Config blob ka ek module record (16 B wire layout).</summary>
public sealed record ConfigModuleRecord(
    byte Type,
    byte Slot,
    byte Flags,
    byte PinA,
    byte PinB = 0xFF,
    byte PinC = 0xFF,
    byte[]? Params = null);

public static class ConfigBlob
{
    public const int HeaderSize = 8;
    public const int RecordSize = 16;
    public const int TrailerSize = 4;
    public const int MaxTotalBytes = 2048;
    public const int MaxModules = 127;
    public const byte Schema = 2;

    /// <summary>Shifter gate-mode record ka type (§3.6, firmware RSH_CFG_TYPE_SHIFTER).</summary>
    public const byte ShifterRecordType = 11;

    /// <summary>
    /// Default H-pattern gear pins (firmware config.h PIN_SHIFT_H1..H6):
    /// 13, 14, 16, 17, 18, 19. Note: 16/17/19 encoder pins bhi hain —
    /// H-pattern + encoders ek saath firmware COMMIT par reject hote hain
    /// (config_transaction.h: has_hpattern &amp;&amp; n_enc &gt; 0).
    /// </summary>
    public static readonly byte[] DefaultHpatternGearPins = [13, 14, 16, 17, 18, 19];

    /// <summary>
    /// Shifter gate-mode record banao (type 11, §3.6 / firmware
    /// config_transaction.h): slot 0, flags 0, pinA/B/C = gear pins 1..3,
    /// params[0] = gate mode (0 Sequential, 1 HPattern), params[1..3] =
    /// gear pins 4..6, params[4..9] = 0. Sequential mode mein gear pins
    /// ki zaroorat nahi (default Sequential — record na ho to bhi) —
    /// is liye Sequential ke liye pins 0xFF likhe jate hain; HPattern
    /// ke liye exactly 6 pins lazmi hain (warna ArgumentException).
    /// Ghalat mode (defined enum se bahar) bhi ArgumentException hai —
    /// firmware getter aise mode ko 0 par gira deta hai, lekin PC-side
    /// ghalat record bhejna hi nahi chahiye.
    /// </summary>
    public static ConfigModuleRecord ShifterRecord(ShifterGateMode mode, ReadOnlySpan<byte> gearPins = default)
    {
        if (!Enum.IsDefined(mode))
            throw new ArgumentException($"GateMode {(int)mode} ghalat hai (Sequential ya HPattern).", nameof(mode));
        if (mode == ShifterGateMode.HPattern)
        {
            if (gearPins.Length != 6)
                throw new ArgumentException("H-pattern ke liye exactly 6 gear pins lazmi hain.", nameof(gearPins));
            var pins = gearPins.ToArray();
            var @params = new byte[10];
            @params[0] = 1;
            @params[1] = pins[3];
            @params[2] = pins[4];
            @params[3] = pins[5];
            return new ConfigModuleRecord(ShifterRecordType, Slot: 0, Flags: 0,
                PinA: pins[0], PinB: pins[1], PinC: pins[2], Params: @params);
        }
        return new ConfigModuleRecord(ShifterRecordType, Slot: 0, Flags: 0,
            PinA: 0xFF, PinB: 0xFF, PinC: 0xFF,
            Params: new byte[10]);
    }

    /// <summary>Blob banao: header + records + CRC32 trailer. Ghalat input par ArgumentException.</summary>
    public static byte[] Build(IReadOnlyList<ConfigModuleRecord> modules, ushort cfgVersion)
    {
        ArgumentNullException.ThrowIfNull(modules);
        if (modules.Count > MaxModules)
            throw new ArgumentException($"Config blob mein zyada se zyada {MaxModules} modules ho sakte hain (mila {modules.Count}).", nameof(modules));
        int length = HeaderSize + RecordSize * modules.Count + TrailerSize;
        if (length > MaxTotalBytes)
            throw new ArgumentException($"Config blob {length} B — had {MaxTotalBytes} B se zyada hai.", nameof(modules));
        var blob = new byte[length];
        blob[0] = (byte)'R'; blob[1] = (byte)'S'; blob[2] = (byte)'H'; blob[3] = (byte)'G';
        blob[4] = Schema;
        blob[5] = (byte)modules.Count;
        blob[6] = (byte)(cfgVersion & 0xFF);
        blob[7] = (byte)(cfgVersion >> 8);
        // NOTE: PROTOCOL-V2.1.md §3.6 header mein "flags u8 | reserved u8"
        // bhi likhta hai (kul 10 B banta) — wo doc ki ghalti hai. Firmware
        // (config_transaction.h rsh_cfg_blob_build, RSH_CFG_HEADER_SIZE 8)
        // authority hai: header sirf 8 B hai — magic(4) | schema(1) |
        // moduleCount(1) | cfgVersion u16 LE (offset 6). Yahan wahi hai.
        for (int i = 0; i < modules.Count; i++)
        {
            var m = modules[i];
            int off = HeaderSize + i * RecordSize;
            blob[off] = m.Type;
            blob[off + 1] = m.Slot;
            blob[off + 2] = m.Flags;
            blob[off + 3] = m.PinA;
            blob[off + 4] = m.PinB;
            blob[off + 5] = m.PinC;
            if (m.Params is not null)
                Array.Copy(m.Params, 0, blob, off + 6, Math.Min(10, m.Params.Length));
        }
        uint crc = Crc32.Compute(blob.AsSpan(0, length - TrailerSize));
        blob[length - 4] = (byte)(crc & 0xFF);
        blob[length - 3] = (byte)((crc >> 8) & 0xFF);
        blob[length - 2] = (byte)((crc >> 16) & 0xFF);
        blob[length - 1] = (byte)((crc >> 24) & 0xFF);
        return blob;
    }

    /// <summary>
    /// Blob ki saakht check karo (firmware rsh_cfg_blob_validate ka
    /// saakht-hissa: magic/schema/size/CRC32 — pin rules yahan nahi,
    /// wo firmware COMMIT par ya PC-side PinMap.ValidateAll mein hain).
    /// Valid ho to true, warna false + wajah.
    /// </summary>
    public static bool TryValidateStructure(ReadOnlySpan<byte> blob, out string? error)
    {
        error = null;
        if (blob.Length < HeaderSize + TrailerSize || blob.Length > MaxTotalBytes)
        {
            error = $"Config blob {blob.Length} B — had {HeaderSize + TrailerSize}..{MaxTotalBytes} B se bahar hai.";
            return false;
        }
        if (blob[0] != (byte)'R' || blob[1] != (byte)'S' || blob[2] != (byte)'H' || blob[3] != (byte)'G' || blob[4] != Schema)
        {
            error = "Config blob ka magic 'RSHG' ya schema 2 nahi hai.";
            return false;
        }
        int expected = HeaderSize + RecordSize * blob[5] + TrailerSize;
        if (expected != blob.Length)
        {
            error = $"Config blob ki length {blob.Length} B — moduleCount {blob[5]} ke liye {expected} B honi chahiye.";
            return false;
        }
        uint want = Crc32.Compute(blob[..^TrailerSize]);
        uint got = (uint)(blob[^4] | (blob[^3] << 8) | (blob[^2] << 16) | (blob[^1] << 24));
        if (want != got)
        {
            error = "Config blob ka CRC32 trailer match nahi karta.";
            return false;
        }
        return true;
    }
}
