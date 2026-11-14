// RSH GPIO pin map — Settings → GPIO pins (UI-11) ka local store +
// live validation (docs/PIN-RULES.md, ESP32 DevKit V1) + board push
// (Phase F, SW-14: PinMapBoardPush — SET_CFG/COMMIT/CONFIRM §3.5/§3.6).
//
// Board ka asal pin map firmware mein rehta hai (GET_CAPS pin table);
// Settings mein chuna hua map PC par `pin-map.json` mein save hota hai
// aur Setup → Pins step ki table (board na mile to) isi se banti hai —
// board ki table mile to wahi jeetti hai. Board push ka rasta
// PinMapBoardPush hai (SET_CFG/COMMIT/CONFIRM — neeche dekho).
// Map badalne par calibration stale ho sakti hai
// (SETUP-GATES-DECISION.md T2: PinMapChanged) — is liye Apply ke baad
// user ko recalibration prompt dikhega (CheckRecalibration board
// fingerprint se milata hai).
// UPDATE (Phase F W3): PC-side push ab hai — PinMapBoardPush pin map se
// RSHG config blob banata hai aur SET_CFG chunks + COMMIT + CONFIRM
// bhejta hai (ControlClient.SendCommandAsync ke zariye). POORI tarah
// live NAHI hai: firmware SET_CFG transaction ko accept + store + persist
// karta hai, lekin live input path (inputs.cpp) abhi bhi compile-time
// PIN_* parhta hai — board nayi pins tab tak istemal nahi karta jab tak
// firmware inputs ko txn current blob se na parhne lage. Detail:
// docs/PIN-MAP-PUSH.md (blocker + baaqi firmware kaam).
using System.IO;
using System.Text.Json;
using RSH.Core.Protocol;
using RSH.Core.Transport;

namespace RSH.App.ViewModels;

/// <summary>Pin map ki ek function: naam + qism (analog/digital/LED) + default pin.</summary>
public enum PinFunctionKind
{
    /// <summary>Analog axis/battery — sirf ADC1 pins (32, 33, 34, 35, 36, 39).</summary>
    Analog,
    /// <summary>Digital input (button/encoder) — pull-up capable pin chahiye.</summary>
    Digital,
    /// <summary>Onboard status LED — GPIO 2 allowed hai (yehi us ka pin hai).</summary>
    StatusLed,
    /// <summary>WS2812 strip — digital-out pin chahiye, input-only nahi.</summary>
    LedStrip,
}

/// <summary>Pin map ka ek row: function + us ka GPIO pin (null = not assigned).</summary>
public sealed record PinFunction(string Name, PinFunctionKind Kind, int? DefaultPin);

/// <summary>ComboBox ka ek option: value (null = Not assigned) + label.</summary>
public sealed record PinOption(int? Value, string Label);

/// <summary>
/// GPIO pin map ka store + validation (PIN-RULES.md §2/§3).
/// Pure validation yahan hai taake host tests bina UI ke chala sakein;
/// file I/O sirf Load/Save mein hai (U-P1: VM hamesha Task.Run se bulata hai).
/// </summary>
public static class PinMap
{
    /// <summary>ADC1 pins — analog axis/battery sirf yehi (PIN-RULES §1).</summary>
    public static readonly int[] Adc1Pins = [32, 33, 34, 35, 36, 39];

    /// <summary>Input-only pins — na pull-up, na output (34/35/36/39).</summary>
    public static readonly int[] InputOnlyPins = [34, 35, 36, 39];

    /// <summary>Dropdown ke options: Not assigned + har GPIO jo board par maujood hai (20/24/28–31/37/38 maujood hi nahi).</summary>
    public static readonly IReadOnlyList<PinOption> PinOptions =
    [
        new(null, "Not assigned"),
        .. new[] { 0, 1, 2, 3, 4, 5, 12, 13, 14, 15, 16, 17, 18, 19, 21, 22, 23, 25, 26, 27, 32, 33, 34, 35, 36, 39 }
            .Select(p => new PinOption(p, $"GPIO {p}")),
    ];

    /// <summary>
    /// Pin map ke functions — order hi Settings + Setup table ka order hai.
    /// Defaults: firmware/rsh/config.h (DevKit V1) + SetupViewModel ka purana
    /// DefaultPinRows; Clutch ka koi default pin nahi (not assigned).
    /// </summary>
    public static readonly IReadOnlyList<PinFunction> Functions =
    [
        new("Steering", PinFunctionKind.Analog, 34),
        new("Accelerator", PinFunctionKind.Analog, 35),
        new("Brake", PinFunctionKind.Analog, 36),
        new("Clutch", PinFunctionKind.Analog, null),
        new("Handbrake", PinFunctionKind.Analog, 32),
        new("Battery", PinFunctionKind.Analog, 39),
        new("Shifter up", PinFunctionKind.Digital, 27),
        new("Shifter down", PinFunctionKind.Digital, 33),
        new("Horn", PinFunctionKind.Digital, 4),
        new("Encoder 1 A", PinFunctionKind.Digital, 23),
        new("Encoder 1 B", PinFunctionKind.Digital, 19),
        new("Encoder 2 A", PinFunctionKind.Digital, 16),
        new("Encoder 2 B", PinFunctionKind.Digital, 17),
        new("Status LED", PinFunctionKind.StatusLed, 2),
        new("LED strip", PinFunctionKind.LedStrip, 21),
    ];

    public static Dictionary<string, int?> Defaults() =>
        Functions.ToDictionary(f => f.Name, f => f.DefaultPin, StringComparer.Ordinal);

    /// <summary>Ek pin ki class (PIN-RULES §2/§3): forbidden / strapping / reserved / caution / ok.</summary>
    public static string PinClass(int pin) => pin switch
    {
        1 or 3 or 6 or 7 or 8 or 9 or 10 or 11 or 20 or 24 or 28 or 29 or 30 or 31 or 37 or 38 => "forbidden",
        0 or 12 or 15 => "strapping",
        2 => "reserved",
        5 => "caution",
        _ => "ok",
    };

    /// <summary>
    /// SEC-E-12/13: pin-map entries ki count cap — Functions ki tadad
    /// (15). Is se zyada entries wala map save/board-push se pehle
    /// reject hota hai (na ke baad).
    /// </summary>
    public const int MaxPinMapEntries = 15;

    /// <summary>
    /// SEC-E-12/13: poore map ki entry-level validation — count cap +
    /// har GPIO ki 0–39 range (ESP32 ke GPIO sirf 0–39 hain). Naam se
    /// farq nahi parta: har entry check hoti hai, Functions se bahir
    /// ki bhi. True = theek, false + <paramref name="error"/> = reject.
    /// </summary>
    public static bool TryValidateEntries(IReadOnlyDictionary<string, int?> pins, out string? error)
    {
        error = null;
        if (pins.Count > MaxPinMapEntries)
        {
            error = $"Pin map has {pins.Count} entries — at most {MaxPinMapEntries} are allowed.";
            return false;
        }
        foreach (var (name, pin) in pins)
        {
            if (pin is { } p && (p < 0 || p > 39))
            {
                error = $"GPIO {p} for '{name}' is out of range — ESP32 GPIOs are 0–39.";
                return false;
            }
        }
        return true;
    }

    /// <summary>Ek pin ki validation — (error, warning), dono null = theek. Kind ke rules + class rules.</summary>
    public static (string? Error, string? Warning) ValidatePin(PinFunction function, int? pin)
    {
        if (pin is null) return (null, null);
        int p = pin.Value;
        // SEC-E-12: range check sab se pehle — PinClass switch 40+
        // ko "ok" kehta tha (default branch), is liye Digital pin 40
        // aaj baghair error ke guzar jata tha.
        if (p < 0 || p > 39)
            return ($"GPIO {p} is out of range — ESP32 GPIOs are 0–39.", null);
        switch (PinClass(p))
        {
            case "forbidden":
                return ($"GPIO {p} is forbidden — " + (p is 6 or 7 or 8 or 9 or 10 or 11
                    ? "connected to flash."
                    : p is 1 or 3
                        ? "UART0 (used by the USB serial link)."
                        : "does not exist on the ESP32 DevKit V1."), null);
            case "strapping":
                return ($"GPIO {p} is a strapping pin — using it can stop the board from booting.", null);
            case "reserved":
                return function.Kind == PinFunctionKind.StatusLed
                    ? (null, null)
                    : ($"GPIO {p} is reserved for the onboard status LED.", null);
        }
        switch (function.Kind)
        {
            case PinFunctionKind.Analog:
                if (!Adc1Pins.Contains(p))
                    return ($"Analog inputs need an ADC1 pin (32, 33, 34, 35, 36, 39) — GPIO {p} is not ADC1.", null);
                break;
            case PinFunctionKind.Digital:
                if (InputOnlyPins.Contains(p))
                    return (null, $"GPIO {p} is input-only — a button here needs an external pull-up resistor.");
                if (p is 21 or 22)
                    return (null, "GPIO 21/22 is the I²C bus — unavailable while expanders are used.");
                // W3 FIX-2 (Round 2 W2, PC-side warning): GPIO 25/26 fixed
                // pedal button-mode pins hain (config.h PIN_ACCEL_BTN 25,
                // PIN_BRAKE_BTN 26). Koi bhi digital function (horn, gear,
                // encoder) in par assign ho to board us pedal button ko
                // suppress karta hai (inputs.cpp: pinmap-uses-pin check) —
                // sahi behaviour hai, lekin user ko pata hona chahiye.
                // Warning hai, error nahi — Apply phir bhi allowed hai.
                if (p is 25 or 26)
                    return (null, $"GPIO {p} is the fixed {(p == 25 ? "accelerator" : "brake")} pedal button pin — assigning '{function.Name}' here suppresses that pedal button.");
                break;
            case PinFunctionKind.StatusLed:
                if (InputOnlyPins.Contains(p))
                    return ($"The status LED needs an output pin — GPIO {p} is input-only.", null);
                break;
            case PinFunctionKind.LedStrip:
                if (InputOnlyPins.Contains(p))
                    return ($"The LED strip needs an output pin — GPIO {p} is input-only.", null);
                if (p is 21 or 22)
                    return (null, "GPIO 21/22 is the I²C bus — the strip moves if expanders are enabled.");
                break;
        }
        if (p == 5)
            return (null, "GPIO 5 prints boot log output — allowed, but the board logs on this pin at boot.");
        return (null, null);
    }

    /// <summary>
    /// Poore map ki validation: har row ka error/warning + duplicate pin
    /// errors. Result: row naam → (error, warning). ErrorCount == 0 ho to
    /// hi Apply allowed hai. SEC-E-12: count cap bhi yahan hai — zyada
    /// entries hon to "Pin map" row mein error aata hai.
    /// </summary>
    public static (Dictionary<string, (string? Error, string? Warning)> Rows, int ErrorCount) ValidateAll(
        IReadOnlyDictionary<string, int?> pins)
    {
        var rows = new Dictionary<string, (string? Error, string? Warning)>(StringComparer.Ordinal);
        var seen = new Dictionary<int, string>();
        foreach (var function in Functions)
        {
            int? pin = pins.TryGetValue(function.Name, out var p) ? p : null;
            var (error, warning) = ValidatePin(function, pin);
            if (error is null && pin is { } used)
            {
                if (seen.TryGetValue(used, out string? other))
                    error = $"GPIO {used} is already used by {other} — no pin is used twice.";
                else
                    seen[used] = function.Name;
            }
            rows[function.Name] = (error, warning);
        }
        if (!TryValidateEntries(pins, out string? entriesError))
            rows["Pin map"] = (entriesError, null);
        return (rows, rows.Values.Count(v => v.Error is not null));
    }
}

/// <summary>
/// Pin map ki file (`pin-map.json`, DataDirectory mein): function naam →
/// GPIO pin (null = not assigned). Na mile / kharab ho to defaults.
/// Save atomic hai (temp + move) — kharab file kabhi aadhi nahi likhti.
/// </summary>
public sealed class PinMapStore
{
    private readonly string _filePath;

    public PinMapStore(string dataDirectory) =>
        _filePath = Path.Combine(dataDirectory, "pin-map.json");

    public string FilePath => _filePath;

    /// <summary>SEC-C-11 jaisi size cap (pin-map chand KB ka hota hai) — is se bari file kharab mani jati hai.</summary>
    public const long MaxFileBytes = 65_536;

    public Dictionary<string, int?> Load()
    {
        try
        {
            if (!File.Exists(_filePath)) return PinMap.Defaults();
            // DATA-AI FIX (Medium): pehle File.ReadAllText (uncapped) +
            // poori-dict deserialize tha — ek kharab entry (misal pin ki
            // jagah string) poori file ko defaults par gira deti thi aur
            // user ka saara pin map kho jata tha (ProfileStore ka F3
            // tolerance yahan nahi tha). Ab capped read + har entry alag
            // se parse: kharab/na-maloom entry skip, baqi mehfooz rehti hain.
            string? json = TryReadCapped(_filePath);
            if (json is null) return PinMap.Defaults();
            using var doc = JsonDocument.Parse(json);
            if (doc.RootElement.ValueKind != JsonValueKind.Object) return PinMap.Defaults();
            var map = PinMap.Defaults();
            foreach (var prop in doc.RootElement.EnumerateObject())
            {
                if (!PinMap.Functions.Any(f => f.Name == prop.Name)) continue; // na-maloom function ignore
                if (prop.Value.ValueKind == JsonValueKind.Null) { map[prop.Name] = null; continue; }
                if (prop.Value.ValueKind != JsonValueKind.Number || !prop.Value.TryGetInt32(out int pin)) continue; // kharab entry — default rehta hai
                // SEC-E-12: range se bahir pin (haath se badli file) kabhi
                // memory mein nahi aata — us function ka default rehta hai.
                if (pin < 0 || pin > 39) continue;
                map[prop.Name] = pin;
            }
            return map;
        }
        catch (Exception) { return PinMap.Defaults(); }
    }

    /// <summary>File ko cap ke saath parho — cap se zyada bytes milen to null (file kabhi poori RAM mein nahi aati).</summary>
    private static string? TryReadCapped(string path)
    {
        try
        {
            using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read);
            if (stream.Length > MaxFileBytes) return null;
            using var capped = new MemoryStream();
            byte[] buffer = new byte[8192];
            int read;
            long total = 0;
            while ((read = stream.Read(buffer, 0, buffer.Length)) > 0)
            {
                total += read;
                if (total > MaxFileBytes) return null;
                capped.Write(buffer, 0, read);
            }
            return System.Text.Encoding.UTF8.GetString(capped.ToArray());
        }
        catch (Exception) { return null; }
    }

    /// <summary>File ko parho aur batao ke wo theek JSON map hai ya nahi (Restore validation ke liye).</summary>
    public static bool TryParseFile(string path, out Dictionary<string, int?>? map)
    {
        map = null;
        try
        {
            if (!File.Exists(path)) return false;
            // DATA-AI FIX: uncapped ReadAllText tha — capped read (restore
            // path pehle se size check karta hai, ye defense in depth hai).
            string? json = TryReadCapped(path);
            if (json is null) return false;
            var raw = JsonSerializer.Deserialize<Dictionary<string, int?>>(json);
            if (raw is null) return false;
            // DATA-AI FIX (Round 2 W7-4): sirf parse hona kaafi nahi —
            // range se bahir GPIO (999) ya 15 se zyada entries wali file
            // bhi "valid" lagti thi, is liye Save ka backup guard use
            // achi backup bana leta tha. Restore path
            // (DiagnosticsSettings.TryValidatePinMap) ye check pehle se
            // karta hai — yahan bhi lagao taake guard ek jaisa ho.
            if (!PinMap.TryValidateEntries(raw, out _)) return false;
            map = raw;
            return true;
        }
        catch (Exception) { return false; }
    }

    /// <summary>
    /// Save — SEC-E-12/13: likhne se PEHLE validate karo (count cap +
    /// GPIO 0–39 range + poora ValidateAll). Na-jaiz map par
    /// <see cref="ArgumentException"/> — file ko haath nahi lagta,
    /// board-push ka rasta bhi yahin se band hai (Save hi gate hai;
    /// SET_CFG push aane par wahi map jayega jo Save se guzra).
    /// </summary>
    public void Save(IReadOnlyDictionary<string, int?> pins)
    {
        if (!PinMap.TryValidateEntries(pins, out string? entriesError))
            throw new ArgumentException(entriesError, nameof(pins));
        var (_, errorCount) = PinMap.ValidateAll(pins);
        if (errorCount > 0)
            throw new ArgumentException($"Pin map has {errorCount} validation error(s) — fix them before saving.", nameof(pins));
        string dir = Path.GetDirectoryName(_filePath)!;
        Directory.CreateDirectory(dir);
        string tempPath = _filePath + ".tmp-" + Guid.NewGuid().ToString("N");
        try
        {
            File.WriteAllText(tempPath, JsonSerializer.Serialize(pins, new JsonSerializerOptions { WriteIndented = true }));
            // DATA-AI FIX (Medium): pehle koi backup nahi tha — kharab
            // pin-map.json Load par defaults deti thi aur agla Save
            // asal map hamesha ke liye mita deta tha. Ab guarded .bak:
            // sirf theek parse hone wali file backup banti hai (kharab
            // file achi backup ko overwrite nahi karti — ProfileStore/
            // AppSettingsStore wala SEC-C-13 pattern).
            if (File.Exists(_filePath) && TryParseFile(_filePath, out _))
            {
                try { File.Copy(_filePath, _filePath + ".bak", overwrite: true); }
                catch (IOException) { /* backup best-effort — save nahi rukti */ }
                catch (UnauthorizedAccessException) { /* backup best-effort */ }
            }
            File.Move(tempPath, _filePath, overwrite: true);
        }
        catch
        {
            try { if (File.Exists(tempPath)) File.Delete(tempPath); } catch (IOException) { }
            throw;
        }
    }
}

/// <summary>
/// Pin map ko board par push karo (Phase F, SW-14 — SET_CFG/COMMIT/CONFIRM,
/// PROTOCOL-V2.1.md §3.5/§3.6, transaction D28).
///
/// Rasta: pin map → RSHG config blob (<see cref="PinMapBoardPush.BuildBlob"/>) →
/// SET_CFG chunks (≤ 1024 B data) → COMMIT → CONFIRM (30 s window).
/// <paramref name="sendCommand"/> wahi delegate hai jo
/// <c>ControlClient.SendCommandAsync</c> deta hai — BoardSession ke paas
/// key/nonce hain, is liye production wiring mein W1 BoardSession se ye
/// delegate dega (docs/PIN-MAP-PUSH.md mein exact snippet hai); yahan
/// BoardSession ko chhua nahi gaya (W1 ki file hai). Is class ko
/// <c>ControlClient</c> banane ki zaroorat nahi — bhejna caller ka kaam hai,
/// yahan sirf blob + transaction sequence hai.
///
/// Blob mapping (pin-function naam → blob record; slots FIXED hain —
/// assign/unassign se doosre functions ke slots nahi badalte):
/// <list type="bullet">
/// <item>Steering/Accelerator/Brake/Clutch/Handbrake → AXIS_ADC (type 1),
/// slots 0/1/2/3/4 (function ka mustaqil slot).</item>
/// <item>Battery → BATTERY (type 7), slot 0.</item>
/// <item>Shifter up/down, Horn → BUTTON (type 3), slots 0/1/2.</item>
/// <item>Encoder 1/2 (A+B) → ENCODER (type 5), slots 0/1, pinA/pinB.</item>
/// <item>LED strip → LED_STRIP (type 6), slot 0.</item>
/// <item>Status LED → blob mein NAHI (onboard LED firmware ki zimmedari hai,
/// is ka koi blob type nahi).</item>
/// <item>Not-assigned function → blob mein record hi nahi (board usay
/// sample nahi karta; pin hatao to record gayab ho jata hai).</item>
/// </list>
/// Validation yahan nahi hoti — <see cref="PinMap.ValidateAll"/> pehle se
/// honi chahiye; phir bhi PushAsync ErrorCount &gt; 0 par koi frame nahi
/// bhejta (ghalat map board tak kabhi nahi jata).
/// Nakami par staged transaction REVERT se discard ki jati hai (best-effort).
/// </summary>
public static class PinMapBoardPush
{
    /// <summary>Ek SET_CFG chunk mein zyada se zyada data bytes (§3.5).</summary>
    public const int MaxChunkDataBytes = 1024;

    /// <summary>Pin map se RSHG config blob banao (§3.6). <paramref name="cfgVersion"/> blob header mein jata hai.</summary>
    public static byte[] BuildBlob(IReadOnlyDictionary<string, int?> pins, ushort cfgVersion = 1)
    {
        ArgumentNullException.ThrowIfNull(pins);
        // Slots FIXED hain (function ka mustaqil slot — assign/unassign se
        // doosre functions ke slots nahi badalte; firmware factory blob bhi
        // steering 0 / accel 1 / brake 2 istemal karta hai).
        var records = new List<ConfigModuleRecord>();
        foreach (var function in PinMap.Functions)
        {
            int? pin = pins.TryGetValue(function.Name, out var p) ? p : null;
            if (pin is null) continue; // not assigned → board isay sample nahi karta
            switch (function.Name)
            {
                case "Steering":
                    records.Add(new ConfigModuleRecord(Type: 1, Slot: 0, Flags: 0, PinA: (byte)pin.Value));
                    break;
                case "Accelerator":
                    records.Add(new ConfigModuleRecord(Type: 1, Slot: 1, Flags: 0, PinA: (byte)pin.Value));
                    break;
                case "Brake":
                    records.Add(new ConfigModuleRecord(Type: 1, Slot: 2, Flags: 0, PinA: (byte)pin.Value));
                    break;
                case "Clutch":
                    records.Add(new ConfigModuleRecord(Type: 1, Slot: 3, Flags: 0, PinA: (byte)pin.Value));
                    break;
                case "Handbrake":
                    records.Add(new ConfigModuleRecord(Type: 1, Slot: 4, Flags: 0, PinA: (byte)pin.Value));
                    break;
                case "Battery":
                    records.Add(new ConfigModuleRecord(Type: 7, Slot: 0, Flags: 0, PinA: (byte)pin.Value));
                    break;
                case "Shifter up":
                    records.Add(new ConfigModuleRecord(Type: 3, Slot: 0, Flags: 0, PinA: (byte)pin.Value));
                    break;
                case "Shifter down":
                    records.Add(new ConfigModuleRecord(Type: 3, Slot: 1, Flags: 0, PinA: (byte)pin.Value));
                    break;
                case "Horn":
                    records.Add(new ConfigModuleRecord(Type: 3, Slot: 2, Flags: 0, PinA: (byte)pin.Value));
                    break;
                case "Encoder 1 A":
                case "Encoder 2 A":
                    // A aaya to poora encoder record (B ho to saath, warna 0xFF).
                    byte encSlot = function.Name.StartsWith("Encoder 1", StringComparison.Ordinal) ? (byte)0 : (byte)1;
                    string bName = function.Name.Replace(" A", " B", StringComparison.Ordinal);
                    int? pinB = pins.TryGetValue(bName, out var pb) ? pb : null;
                    records.Add(new ConfigModuleRecord(Type: 5, Slot: encSlot, Flags: 0,
                        PinA: (byte)pin.Value, PinB: pinB is null ? (byte)0xFF : (byte)pinB.Value));
                    break;
                case "Encoder 1 B":
                case "Encoder 2 B":
                    // B akela (A not assigned) ho to bhi record — pinA 0xFF.
                    string aName = function.Name.Replace(" B", " A", StringComparison.Ordinal);
                    int? pinA = pins.TryGetValue(aName, out var pa) ? pa : null;
                    if (pinA is null)
                        records.Add(new ConfigModuleRecord(Type: 5,
                            Slot: function.Name.StartsWith("Encoder 1", StringComparison.Ordinal) ? (byte)0 : (byte)1,
                            Flags: 0, PinA: 0xFF, PinB: (byte)pin.Value));
                    break;
                case "LED strip":
                    records.Add(new ConfigModuleRecord(Type: 6, Slot: 0, Flags: 0, PinA: (byte)pin.Value));
                    break;
                case "Status LED":
                    // Onboard LED — firmware ki zimmedari, blob type nahi hai.
                    break;
            }
        }
        return ConfigBlob.Build(records, cfgVersion);
    }

    /// <summary>Blob ko SET_CFG chunk payloads mein todo (§3.5: offset|total|chunkLen|chunkCrc|data, sab u16 LE, chunkCrc = CRC16-CCITT data par).</summary>
    public static IReadOnlyList<byte[]> BuildSetCfgChunks(byte[] blob)
    {
        ArgumentNullException.ThrowIfNull(blob);
        if (blob.Length == 0 || blob.Length > ConfigBlob.MaxTotalBytes)
            throw new ArgumentException($"Config blob {blob.Length} B — had 1..{ConfigBlob.MaxTotalBytes} B se bahar hai.", nameof(blob));
        var chunks = new List<byte[]>();
        for (int offset = 0; offset < blob.Length; offset += MaxChunkDataBytes)
        {
            int len = Math.Min(MaxChunkDataBytes, blob.Length - offset);
            var payload = new byte[8 + len];
            payload[0] = (byte)(offset & 0xFF);
            payload[1] = (byte)(offset >> 8);
            payload[2] = (byte)(blob.Length & 0xFF);
            payload[3] = (byte)(blob.Length >> 8);
            payload[4] = (byte)(len & 0xFF);
            payload[5] = (byte)(len >> 8);
            ushort crc = Crc16.Compute(blob.AsSpan(offset, len));
            payload[6] = (byte)(crc & 0xFF);
            payload[7] = (byte)(crc >> 8);
            Array.Copy(blob, offset, payload, 8, len);
            chunks.Add(payload);
        }
        return chunks;
    }

    /// <summary>
    /// Poora push: validate → SET_CFG chunks → COMMIT → CONFIRM.
    /// Kamyabi par (true, null, chunks); nakami par (false, wajah, bheje
    /// gaye chunks) aur staged transaction REVERT se discard hoti hai.
    /// CONFIRM-retry edge (final polish): CONFIRM ki pehli koshish
    /// kamyab ho kar reply kho de (timeout) to dobara CONFIRM par board
    /// UNSUPPORTED deta hai (config_transaction.h: pending na ho to
    /// CONFIRM unsupported — pehle se confirmed transaction IDLE hai) —
    /// ye "fail" NAHI, "already confirmed" hai: success report hoti hai,
    /// REVERT nahi bheja jata (REVERT yahan nuksan-deh hota — config
    /// apply ho chuki hoti hai). ControlClient andar hi 3× retry karta
    /// hai (wahi reqId, naya counter), is liye ye UNSUPPORTED reply
    /// aksar usi ke andar se aata hai; PushAsync-level ek retry sirf
    /// no-reply (timeout) par hoti hai.
    /// </summary>
    public static async Task<(bool Ok, string? Error, int ChunksSent)> PushAsync(
        IReadOnlyDictionary<string, int?> pins,
        Func<ControlCommand, ReadOnlyMemory<byte>, CancellationToken, Task<ControlResult>> sendCommand,
        ushort cfgVersion = 1,
        CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(pins);
        ArgumentNullException.ThrowIfNull(sendCommand);
        var (_, errorCount) = PinMap.ValidateAll(pins);
        if (errorCount > 0)
            return (false, $"Pin map mein {errorCount} error hain — board ko kuch nahi bheja gaya (pehle Settings mein theek karo).", 0);
        byte[] blob;
        try { blob = BuildBlob(pins, cfgVersion); }
        catch (Exception ex) { return (false, $"Config blob nahi bana: {ex.Message}", 0); }
        var chunks = BuildSetCfgChunks(blob);
        // Koi bhi SET_CFG koshish hui ho to fail par REVERT bhejo — pehla
        // chunk timeout par bhi board ne stage kar liya ho sakta hai
        // (REVERT khaali transaction par no-op OK hai, is liye be-ja
        // bhejna mehfooz hai).
        bool sentAnything = false;
        try
        {
            for (int i = 0; i < chunks.Count; i++)
            {
                byte[] chunk = chunks[i];
                sentAnything = true;
                var result = await sendCommand(ControlCommand.SetCfg, chunk, cancellationToken).ConfigureAwait(false);
                if (!result.Succeeded)
                    return await FailAsync($"SET_CFG chunk {i + 1}/{chunks.Count} reject hua: {result.Error ?? "jawab nahi aaya"}", i).ConfigureAwait(false);
                if (!TryParseSetCfgReply(result.ReplyPayload, offset: i * MaxChunkDataBytes, chunkDataLen: chunk.Length - 8, total: blob.Length, out string? replyError))
                    return await FailAsync($"SET_CFG chunk {i + 1}/{chunks.Count} ka reply ghalat hai: {replyError}", i).ConfigureAwait(false);
            }
            var commit = await sendCommand(ControlCommand.Commit, ReadOnlyMemory<byte>.Empty, cancellationToken).ConfigureAwait(false);
            if (!commit.Succeeded)
                return await FailAsync($"COMMIT reject hua: {commit.Error ?? "jawab nahi aaya"}", chunks.Count).ConfigureAwait(false);
            var confirm = await sendCommand(ControlCommand.Confirm, ReadOnlyMemory<byte>.Empty, cancellationToken).ConfigureAwait(false);
            if (confirm.Succeeded)
                return (true, null, chunks.Count);
            if (IsAlreadyConfirmed(confirm))
                return (true, null, chunks.Count);
            if (confirm.ReplyPayload is null && !cancellationToken.IsCancellationRequested)
            {
                // No-reply (timeout): pehli koshish board tak pohanch kar
                // confirm kar chuki ho sakti hai, reply raaste mein kho
                // gaya ho — ek retry: Ok ya UNSUPPORTED (already
                // confirmed) dono success hain; dobara no-reply ya asal
                // reject hi fail hai (phir REVERT — config 30 s deadline
                // par waise bhi rollback hoti).
                var retry = await sendCommand(ControlCommand.Confirm, ReadOnlyMemory<byte>.Empty, cancellationToken).ConfigureAwait(false);
                if (retry.Succeeded || IsAlreadyConfirmed(retry))
                    return (true, null, chunks.Count);
                return await FailAsync($"CONFIRM reject hua: {retry.Error ?? "jawab nahi aaya"}", chunks.Count).ConfigureAwait(false);
            }
            return await FailAsync($"CONFIRM reject hua: {confirm.Error ?? "jawab nahi aaya"}", chunks.Count).ConfigureAwait(false);
        }
        catch (OperationCanceledException)
        {
            if (sentAnything)
            {
                try { await sendCommand(ControlCommand.Revert, ReadOnlyMemory<byte>.Empty, CancellationToken.None).ConfigureAwait(false); }
                catch (Exception) { /* cancel ke waqt revert best-effort */ }
            }
            throw;
        }

        async Task<(bool Ok, string? Error, int ChunksSent)> FailAsync(string error, int chunksSent)
        {
            if (sentAnything)
            {
                try { await sendCommand(ControlCommand.Revert, ReadOnlyMemory<byte>.Empty, CancellationToken.None).ConfigureAwait(false); }
                catch (Exception) { /* revert best-effort — asal wajah hi report hoti hai */ }
            }
            return (false, error, chunksSent);
        }
    }

    /// <summary>
    /// CONFIRM ka "already confirmed" pehchano: board ne asal reply diya
    /// aur status UNSUPPORTED hai. Isi PushAsync call mein COMMIT kamyab
    /// hua tha, is liye board CONFIRM ke waqt COMMITTED state mein tha —
    /// UNSUPPORTED ka matlab hai koi pehli CONFIRM koshish (jis ka reply
    /// kho gaya) pehle hi confirm kar chuki hai aur transaction IDLE hai
    /// (config_transaction.h rsh_cfg_confirm: state != COMMITTED →
    /// UNSUPPORTED). No-reply (timeout) is mein nahi aata — us ka faisla
    /// retry karta hai. Sirf yehi ek jagah hai jahan UNSUPPORTED ko
    /// success samjha jata hai: SET_CFG/COMMIT par UNSUPPORTED abhi bhi
    /// fail hai (wahan FailAsync hi chalta hai).
    /// </summary>
    internal static bool IsAlreadyConfirmed(ControlResult confirm) =>
        confirm.ReplyPayload is not null && confirm.ReplyStatus == ControlStatus.Unsupported;

    /// <summary>SET_CFG reply check (§3.5): status Ok + stagedBytes == offset+chunkLen + totalExpected == total.</summary>
    internal static bool TryParseSetCfgReply(byte[]? payload, int offset, int chunkDataLen, int total, out string? error)
    {
        error = null;
        if (payload is null || payload.Length < 5)
        {
            error = "SET_CFG reply khaali ya bohat chhota hai (status + stagedBytes + totalExpected chahiye).";
            return false;
        }
        if ((ControlStatus)payload[0] != ControlStatus.Ok)
        {
            error = $"Board ne SET_CFG par {(ControlStatus)payload[0]} diya.";
            return false;
        }
        int staged = payload[1] | (payload[2] << 8);
        int expected = payload[3] | (payload[4] << 8);
        if (staged != offset + chunkDataLen || expected != total)
        {
            error = $"Board ne staged={staged} (chahiye {offset + chunkDataLen}), total={expected} (chahiye {total}) bataya.";
            return false;
        }
        return true;
    }
}
