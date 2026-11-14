using System.Collections.Concurrent;
using System.IO;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace RSH.App.Services;

/// <summary>App ki apni settings (profiles se alag — ye machine/user ki pasandein hain).</summary>
public sealed class AppSettings
{
    public string Theme { get; set; } = "System";          // System | Dark | Light | HighContrast
    /// <summary>UI language: System (OS UI culture) | en | ur | hi — Phase F live switch, restart nahi.</summary>
    public string Language { get; set; } = "System";
    public bool Autostart { get; set; }
    public bool StartMinimized { get; set; }
    public bool CloseToTray { get; set; } = true;
    public bool OutputEnabled { get; set; } = true;
    public string EspIp { get; set; } = "192.168.4.1";
    public int VJoyDeviceId { get; set; } = 1;
    /// <summary>
    /// Output backend: "VJoy" (default, v2.0.0) ya "HIDMaestro"
    /// (Phase H experimental). HIDMaestro sirf tab chalta hai jab
    /// <see cref="ExperimentalHidMaestroEnabled"/> bhi true ho — warna
    /// Sanitize + selector dono fail-closed vJoy par girte hain.
    /// </summary>
    public string OutputBackend { get; set; } = "VJoy";
    /// <summary>
    /// HIDMaestro experimental toggle — explicit opt-in. Band ho to
    /// OutputBackend chahe kuch bhi ho, vJoy hi chalta hai.
    /// </summary>
    public bool ExperimentalHidMaestroEnabled { get; set; }
    /// <summary>
    /// HIDMaestro catalog profile id (masalan "logitech-g29") ya custom
    /// profile id. Khaali = koi profile nahi chuni — HIDMaestro
    /// TryConnect refuse karta hai (koi brand identity default nahi;
    /// profile Windows-PC gate H-HID-2 se tay hogi).
    /// </summary>
    public string HidMaestroProfileId { get; set; } = "";
    public string LogLevel { get; set; } = "Warning";      // Debug | Information | Warning | Error
    public bool WizardCompleted { get; set; }
    public bool FirstRunDone { get; set; }
    /// <summary>Samne wali game ke liye profile khud load ho (Profiles page par har profile ka game naam).</summary>
    public bool AutoSwitchProfiles { get; set; } = true;
    /// <summary>Design v2 ki ek-dafa theme migration ho gayi?</summary>
    public bool ThemeMigratedV12 { get; set; }
    /// <summary>Wheel ke RSH Wi-Fi ka per-device password (pairing se milta
    /// hai — HELLO reply / first boot; kabhi default nahi, SEC-C-04).
    /// Khaali = abhi pair nahi hua — Wi-Fi connect karne se pehle password
    /// darj karo. Purana default ("change-me-123") Sanitize mein khaali
    /// kar diya jata hai (wo kabhi valid per-device password nahi tha).
    /// SEC-E-04 (Phase F): ye property <b>kabhi settings.json mein nahi
    /// likhi jati</b> ([JsonIgnore]) — password ka ghar DPAPI key store hai
    /// (BoardSession.TryGetWifiPassword / TrySaveWifiPassword, PROTOCOL
    /// §5.3). Ye sirf in-memory working value hai (UI textbox buffer,
    /// HELLO se pehle darj kiya hua password); restart par dobara KeyStore
    /// se aata hai, disk se kabhi nahi.</summary>
    [JsonIgnore]
    public string WheelWifiPassword { get; set; } = "";

    /// <summary>SEC-E-04 migration: Load ke waqt on-disk settings.json mein
    /// abhi bhi plaintext password mila ho to wo yahan rakha jata hai —
    /// <b>kabhi serialize nahi hota</b> ([JsonIgnore]). AppServices pehle
    /// HELLO par ise KeyStore mein migrate karta hai (BoardSession.
    /// MigrateLegacyPassword) aur phir saaf kar ke settings dobara save
    /// karta hai — us ke baad disk par password ka koi nishan nahi rehta.
    /// Agar board kabhi connect na ho to legacy value sirf is session ki
    /// memory mein rehti hai; agle Save par disk wali copy bhi mit jati
    /// hai (password phir dobara darj karna hoga — security > convenience).</summary>
    [JsonIgnore]
    public string LegacyWheelWifiPassword { get; set; } = "";
}

/// <summary>
/// Settings ka wohi crash-safe tareeqa jo profiles ka hai: atomic write +
/// guarded backup + defaults par wapasi.
/// SEC-C-13: backup sirf tab jab maujooda settings file theek parse hoti
/// ho (kharab settings achay .bak ko overwrite nahi karti — pehle ye
/// guard bilkul nahi tha). SEC-C-14: Load/Save per-path lock ke neeche,
/// temp naam unique (.tmp-&lt;guid&gt;). SEC-C-11: size cap (64 KB — asal
/// settings.json chand KB ki hoti hai); cap se bari file kharab mani jati hai.
/// SEC-E-04 (Phase F): settings.json mein Wi-Fi password <b>kabhi nahi</b>
/// likha jata — <see cref="AppSettings.WheelWifiPassword"/> [JsonIgnore] hai
/// (ghar DPAPI KeyStore hai); purani file mein plaintext mile to sirf
/// <see cref="AppSettings.LegacyWheelWifiPassword"/> (in-memory) mein aata
/// hai, migration ke liye — dobara disk par kabhi nahi jata.
/// </summary>
public sealed class AppSettingsStore
{
    private static readonly JsonSerializerOptions JsonOptions = new() { WriteIndented = true, PropertyNameCaseInsensitive = true };

    /// <summary>SEC-C-11: settings file ki size cap.</summary>
    public const long MaxFileBytes = 65_536;

    // SEC-C-14: per-path lock — do threads/instances ek hi file par
    // ek saath Save karein to .tmp clash ya aadhi file nahi banti.
    private static readonly ConcurrentDictionary<string, object> PathLocks = new(StringComparer.OrdinalIgnoreCase);

    private readonly string _filePath;
    private readonly object _ioLock;

    public AppSettingsStore(string filePath)
    {
        _filePath = Path.GetFullPath(filePath);
        _ioLock = PathLocks.GetOrAdd(_filePath, static _ => new object());
    }
    public string FilePath => _filePath;
    private string BackupPath => _filePath + ".bak";
    private string Backup1Path => _filePath + ".bak1";
    private string Backup2Path => _filePath + ".bak2";

    public AppSettings Load()
    {
        lock (_ioLock)
        {
            // DATA-AI FIX (Round 2 W7-5): sirf ek .bak tha — current +
            // .bak dono kharab hon to settings hamesha ke liye defaults
            // ho jati thin. Ab ProfileStore jaisi 3-generation chain
            // (.bak → .bak1 → .bak2).
            foreach (string path in new[] { _filePath, BackupPath, Backup1Path, Backup2Path })
            {
                try
                {
                    var s = TryLoadFile(path);
                    if (s is not null) return s;
                }
                catch (Exception) { /* kharab file — backup/defaults */ }
            }
            return new AppSettings();
        }
    }

    /// <summary>Ek settings file ko size cap + parse + Sanitize ke saath parho; na-qabil-e-istemal par null.
    /// SEC-E-04: raw JSON mein abhi bhi plaintext password ho (purani
    /// settings.json / purana backup restore) to use
    /// <see cref="AppSettings.LegacyWheelWifiPassword"/> mein rakho —
    /// migration ke liye; <see cref="AppSettings.WheelWifiPassword"/> mein
    /// kabhi mat dalo (wo in-memory buffer hai, disk value ka ghar nahi).</summary>
    private static AppSettings? TryLoadFile(string path)
    {
        try
        {
            if (!File.Exists(path)) return null;
            // SEC-C-11: capped read — length check ke baad File.ReadAllText
            // TOCTOU tha (check aur parhne ke darmiyan file barh sakti thi).
            string? json = TryReadCapped(path);
            if (json is null) return null;
            var s = JsonSerializer.Deserialize<AppSettings>(json, JsonOptions);
            if (s is null) return null;
            s = Sanitize(s);
            s.LegacyWheelWifiPassword = ExtractLegacyPassword(json);
            return s;
        }
        catch (Exception) { return null; }   // kharab file — caller backup/defaults azmata hai
    }

    /// <summary>SEC-E-04: raw settings JSON mein "WheelWifiPassword" field
    /// ho to us ki value nikalo (migration ke liye). Khaali / "change-me-123"
    /// (kabhi valid password nahi tha) / ghalat JSON par "" — kabhi throw nahi.</summary>
    private static string ExtractLegacyPassword(string json)
    {
        try
        {
            using var doc = JsonDocument.Parse(json);
            if (doc.RootElement.ValueKind != JsonValueKind.Object) return "";
            foreach (JsonProperty prop in doc.RootElement.EnumerateObject())
            {
                if (!string.Equals(prop.Name, "WheelWifiPassword", StringComparison.OrdinalIgnoreCase)) continue;
                if (prop.Value.ValueKind != JsonValueKind.String) return "";
                string value = prop.Value.GetString() ?? "";
                if (string.IsNullOrWhiteSpace(value) || value == "change-me-123") return "";
                return value;
            }
            return "";
        }
        catch (Exception) { return ""; }
    }

    /// <summary>
    /// SEC-E-04 / backup leak (Phase F W8): backup zip ke liye settings ka
    /// JSON — live <paramref name="settings"/> se serialize hota hai, disk
    /// wali file se kabhi nahi. <see cref="AppSettings.WheelWifiPassword"/>
    /// aur <see cref="AppSettings.LegacyWheelWifiPassword"/> dono
    /// [JsonIgnore] hain, is liye backup mein password <b>kabhi</b> nahi
    /// aata — purani disk file mein plaintext ho tab bhi nahi.
    /// </summary>
    public static string ToBackupJson(AppSettings settings) =>
        JsonSerializer.Serialize(Sanitize(settings), JsonOptions);

    /// <summary>SEC-C-11: file ko cap ke saath parho — cap se zyada bytes milen to null (file kabhi poori RAM mein nahi aati).</summary>
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

    /// <summary>SEC-C-12 jaisa guard (settings ke liye): file theek parse hoti hai aur Sanitize se guzarti hai?</summary>
    private static bool ParsesOk(string path) => TryLoadFile(path) is not null;

    public void Save(AppSettings settings)
    {
        lock (_ioLock)
        {
            string dir = Path.GetDirectoryName(Path.GetFullPath(_filePath))!;
            Directory.CreateDirectory(dir);

            // SEC-C-14: unique temp naam + finally mein safai.
            string tempPath = _filePath + ".tmp-" + Guid.NewGuid().ToString("N");
            try
            {
                File.WriteAllText(tempPath, JsonSerializer.Serialize(Sanitize(settings), JsonOptions));
            }
            catch
            {
                try { if (File.Exists(tempPath)) File.Delete(tempPath); } catch (IOException) { }
                throw;
            }

            try
            {
                // SEC-C-13: backup sirf tab jab maujooda file theek ho —
                // kharab settings achay .bak ko overwrite nahi karti.
                // DATA-AI FIX (Round 2 W7-5): 3-generation rotation
                // (ProfileStore wala pattern) — kharab .bak aage nahi
                // barhta; kharab .bak1 achi .bak2 ko overwrite nahi karta.
                if (File.Exists(_filePath) && ParsesOk(_filePath))
                {
                    if (File.Exists(BackupPath))
                    {
                        if (ParsesOk(BackupPath))
                        {
                            if (File.Exists(Backup1Path))
                            {
                                if (ParsesOk(Backup1Path))
                                    File.Copy(Backup1Path, Backup2Path, overwrite: true);
                                else
                                    File.Delete(Backup1Path);
                            }
                            File.Move(BackupPath, Backup1Path, overwrite: true);
                        }
                        else
                        {
                            File.Delete(BackupPath);
                        }
                    }
                    File.Copy(_filePath, BackupPath, overwrite: true);
                }
                else if (File.Exists(_filePath))
                    PreserveCorruptCopy(_filePath);
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
    /// Save ka async rasta (U-P1 / A9): file I/O ThreadPool par hota hai —
    /// UI thread kabhi block nahi hota. Andar wahi sync <see cref="Save"/>
    /// chalta hai (wahi lock, wahi guarded backup, wahi atomic move),
    /// is liye sync/async Save mein logic ka farq kabhi nahi parta.
    /// </summary>
    public Task SaveAsync(AppSettings settings, CancellationToken cancellationToken = default) =>
        Task.Run(() => Save(settings), cancellationToken);

    /// <summary>SEC-C-13: kharab file ki rozana ek diagnostic copy (overwrite, jama nahi hoti).</summary>
    private void PreserveCorruptCopy(string path)
    {
        try
        {
            if (new FileInfo(path).Length > MaxFileBytes) return;
            string dest = _filePath + ".corrupt-" + DateTime.UtcNow.ToString("yyyyMMdd", System.Globalization.CultureInfo.InvariantCulture);
            File.Copy(path, dest, overwrite: true);
        }
        catch (IOException) { /* best-effort */ }
        catch (UnauthorizedAccessException) { /* best-effort */ }
    }

    private static AppSettings Sanitize(AppSettings s)
    {
        if (s.Theme is not ("System" or "Dark" or "Light" or "HighContrast")) s.Theme = "System";
        if (s.Language is not ("System" or "en" or "ur" or "hi")) s.Language = "System";
        if (s.LogLevel is not ("Debug" or "Information" or "Warning" or "Error")) s.LogLevel = "Warning";
        // SEC-C-03: EspIp par sirf null/whitespace check nahi — ghalat
        // settings.json (hostname/path/port) URL injection se firmware +
        // password attacker host par bhej sakti thi. Sirf private IPv4
        // literal qabool; ghalat ho to default 192.168.4.1 par giro.
        s.EspIp = RSH.Core.Net.EspAddress.SanitizeOrDefault(s.EspIp);
        s.VJoyDeviceId = Math.Clamp(s.VJoyDeviceId, 1, 16);
        // Output backend (Phase H): sirf "VJoy" ya "HIDMaestro" — ghalat
        // string vJoy par girti hai. HIDMaestro experimental toggle ke
        // baghair nahi chalta — toggle band ho to backend bhi vJoy.
        s.OutputBackend = RSH.Core.Output.OutputBackendSelector.ToSettingsString(
            RSH.Core.Output.OutputBackendSelector.Parse(s.OutputBackend));
        if (s.OutputBackend == RSH.Core.Output.OutputBackendSelector.HidMaestroName
            && !s.ExperimentalHidMaestroEnabled)
            s.OutputBackend = RSH.Core.Output.OutputBackendSelector.VJoyName;
        s.HidMaestroProfileId = (s.HidMaestroProfileId ?? "").Trim();
        if (s.HidMaestroProfileId.Length > 128)
            s.HidMaestroProfileId = s.HidMaestroProfileId[..128];
        // SEC-C-04: purana hard-coded default kabhi valid password nahi tha
        // (24-char per-device password hota hai, charset mein '-' nahi) —
        // stored copy mile to use khaali (unset) samjho, dobara istemal nahi.
        if (s.WheelWifiPassword is null) s.WheelWifiPassword = "";
        if (s.WheelWifiPassword == "change-me-123") s.WheelWifiPassword = "";
        return s;
    }
}
