using System.Collections.Concurrent;
using System.Text.Json;

namespace RSH.Core.Profiles;

/// <summary>profiles.json ki andar wali shakal (Profile v2 — SW-12).</summary>
public sealed class ProfileFile
{
    /// <summary>Profile schema version: 2 = maujooda. 0/1 = v1 file (migration ke liye).</summary>
    public const int CurrentSchemaVersion = 2;

    /// <summary>
    /// File ka schema version. v1 files mein ye field hoti hi nahi thi —
    /// is liye default 0 hai (0 = v1 samjho), taake Load v1 ko pehchan
    /// sake. Save() hamesha <see cref="CurrentSchemaVersion"/> likhta hai.
    /// </summary>
    public int SchemaVersion { get; set; }

    public List<WheelProfile> Profiles { get; set; } = [];

    public string ActiveProfileName { get; set; } = "Default";

    /// <summary>
    /// Aakhri Load mein kitni kharab entries skip hui (F3) — file mein
    /// save nahi hota.
    /// </summary>
    [System.Text.Json.Serialization.JsonIgnore]
    public int SkippedProfiles { get; set; }

    /// <summary>
    /// Ye data v1 file se migrate hua (Load ne set kiya) — file mein
    /// save nahi hota. Save() is se .v1.bak banata hai.
    /// </summary>
    [System.Text.Json.Serialization.JsonIgnore]
    public bool MigratedFromV1 { get; set; }
}

/// <summary>
/// Profiles ka crash-safe store:
/// - Save atomic hai: pehle unique .tmp-&lt;guid&gt; likho (SEC-C-14),
///   phir purani file ko backup bana kar replace — bijli jaye ya app
///   crash ho, file kabhi aadhi nahi milti. Save/Load per-path lock ke
///   neeche hain (SEC-C-14) — do threads/instances ek saath Save karein
///   to bhi .tmp clash ya aadhi file nahi banti.
/// - Size/count caps (SEC-C-11): file ≤ 1 MB, profiles ≤ 64 — is se
///   bari/zyada file kharab file jaisi treat hoti hai; Load koi bhi
///   exception (OOM/access/format) par backup/defaults par girta hai,
///   startup par persistent crash nahi hota.
/// - 3-generation backup (A3, R-M11): .bak (nayi), .bak1, .bak2 (purani).
///   Rotation sirf tab jab maujooda file Load-jaisi sakht validation
///   pass kare (SEC-C-12: ParsesOk = TryLoadFile — Sanitized + dedupe +
///   count cap + version check, alag kamzor logic nahi). Kharab current
///   file ki rozana ek .corrupt-YYYYMMDD copy mehfooz hoti hai (SEC-C-13).
/// - Load: file kharab ho to .bak → .bak1 → .bak2 se, wo bhi kharab hon
///   to defaults se — app corrupt profile par band NAHI hota.
/// - F3: ek kharab profile entry poori file ko nahi maarti — wo entry
///   skip hoti hai, baqi load hoti hain (SkippedProfiles mein gin'i).
/// - Profile v2 (A10, SW-12): SchemaVersion har file mein; v1 file
///   migrate hoti hai (migration se pehle asal v1 bytes .v1.bak mein),
///   downgrade par <see cref="RestoreV1Backup"/> v1 backup se restore
///   karta hai (R-M9 rollback).
/// - Migration calibration faisla (P9/D10 — WheelProfile.
///   NeedsRecalibration dekho): v1 ki unit-less calibration ko mV tag
///   NAHI kiya jata (unit values se andaza nahi lagaya ja sakta);
///   values verbatim rehti hain, mapper insane calibration ignore karta
///   hai, aur flag UI se dobara calibration karwata hai.
/// - Har loaded profile Sanitized() se guzarta hai.
/// </summary>
public sealed class ProfileStore
{
    private static readonly JsonSerializerOptions JsonOptions = new()
    {
        WriteIndented = true,
        PropertyNameCaseInsensitive = true,
    };

    /// <summary>
    /// SEC-C-11: profile file ki size cap — is se bari file kharab file
    /// jaisi treat hoti hai (backup/defaults). Parhne se pehle length
    /// check + capped read, taake multi-GB file kabhi RAM mein na aye.
    /// Asal profiles.json chand KB ki hoti hai — 1 MB bohat farakh hai.
    /// </summary>
    public const long MaxFileBytes = 1_048_576;

    /// <summary>SEC-C-11: profiles ki tadad cap — is se zyada hon to file reject hoti hai.</summary>
    public const int MaxProfiles = 64;

    /// <summary>
    /// SEC-C-19: import ki hui single-profile file ki size cap (256 KB).
    /// Asal export chand KB ka hota hai — is se bari file parhi hi nahi
    /// jati (Import pehle length check karta hai, phir capped read) aur
    /// <see cref="TryParseImportedProfile"/> bhi yehi cap lagata hai.
    /// </summary>
    public const long MaxImportBytes = 262_144;

    // P6: Save lock — do threads / do ProfileStore instances ek hi file
    // par ek saath Save karein to .tmp clash ya aadhi file nahi banti.
    // Lock file path par hai (normalized), taake alag instances bhi
    // ek hi file ke liye ek doosre ka intezar karein.
    private static readonly ConcurrentDictionary<string, object> PathLocks = new(StringComparer.OrdinalIgnoreCase);

    private readonly string _filePath;
    private readonly object _ioLock;

    public ProfileStore(string filePath)
    {
        _filePath = Path.GetFullPath(filePath);
        _ioLock = PathLocks.GetOrAdd(_filePath, static _ => new object());
    }

    public string FilePath => _filePath;
    private string BackupPath => _filePath + ".bak";
    private string Backup1Path => _filePath + ".bak1";
    private string Backup2Path => _filePath + ".bak2";
    private string V1BackupPath => _filePath + ".v1.bak";
    private string TempPath => _filePath + ".tmp";

    /// <summary>Aakhri Load mein skip hui kharab entries (F3) — har Load par reset hota hai.</summary>
    public int LastSkippedProfiles { get; private set; }

    public ProfileFile Load()
    {
        lock (_ioLock)
        {
            LastSkippedProfiles = 0;
            // SEC-C-11: Load kabhi crash nahi karta — koi bhi kharab file
            // (OOM / access / ghalat format) backup/defaults par girti hai.
            try
            {
                foreach (string path in new[] { _filePath, BackupPath, Backup1Path, Backup2Path })
                {
                    var data = TryLoadFile(path);
                    if (data is not null)
                    {
                        LastSkippedProfiles = data.SkippedProfiles;
                        return data;
                    }
                }
            }
            catch (Exception) { /* neeche defaults — app startup par kabhi crash nahi */ }

            var fresh = new ProfileFile
            {
                SchemaVersion = ProfileFile.CurrentSchemaVersion,
                Profiles = [WheelProfile.CreateDefault()],
                ActiveProfileName = "Default",
            };
            return fresh;
        }
    }

    /// <summary>
    /// Ek file ko tolerant tareeqe se parho (F3): har profile entry
    /// alag se parse hoti hai — kharab entry skip + baqi load hon.
    /// Na-qabil-e-istemal file (kharab JSON, koi valid profile nahi)
    /// par null — caller agla backup azmata hai.
    /// </summary>
    private static ProfileFile? TryLoadFile(string path)
    {
        try
        {
            if (!File.Exists(path)) return null;
            // SEC-C-11: capped read — size cap se bari file kabhi poori
            // RAM mein nahi aati (File.ReadAllText ki jagah).
            string? json = TryReadCapped(path);
            if (json is null) return null;
            using var doc = JsonDocument.Parse(json);
            var root = doc.RootElement;
            if (root.ValueKind != JsonValueKind.Object) return null;

            var data = new ProfileFile
            {
                // v1 files mein SchemaVersion field nahi thi → 0 = v1.
                SchemaVersion = TryGetInt(root, "SchemaVersion"),
                ActiveProfileName = TryGetString(root, "ActiveProfileName") ?? "Default",
            };

            // SEC-C-12/SEC-C-18: na-maloom (mustaqbil ka) schema version
            // khamoshi se ignore nahi hota — file reject, backup/defaults.
            if (data.SchemaVersion > ProfileFile.CurrentSchemaVersion) return null;

            if (!root.TryGetProperty("Profiles", out var profilesEl) ||
                profilesEl.ValueKind != JsonValueKind.Array)
                return null;

            // SEC-C-11: count cap — lakhon profiles wali file reject.
            // Pehle gino (parse se pehle), phir entries parho.
            int total = 0;
            foreach (var _ in profilesEl.EnumerateArray()) total++;
            if (total > MaxProfiles) return null;

            int skipped = 0;
            var profiles = new List<WheelProfile>();
            foreach (var element in profilesEl.EnumerateArray())
            {
                try
                {
                    var profile = JsonSerializer.Deserialize<WheelProfile>(element.GetRawText(), JsonOptions);
                    if (profile is null) { skipped++; continue; }
                    profiles.Add(profile.Sanitized());
                }
                catch (JsonException) { skipped++; }
                catch (ArgumentOutOfRangeException) { skipped++; }   // P2: button 33+ wali entry
                catch (NotSupportedException) { skipped++; }
                catch (InvalidOperationException) { skipped++; }
            }
            data.SkippedProfiles = skipped;
            if (profiles.Count == 0) return null;   // T15: khali list = na-qabil-e-istemal, backup/defaults

            // AUDIT FIX: ek naam ke do profiles hon to GetActive
            // hamesha pehla deta tha aur fix-up khamoshi se active
            // badal sakta tha — naam (case-insensitive) par dedupe.
            data.Profiles = profiles
                .GroupBy(p => p.Name, StringComparer.OrdinalIgnoreCase)
                .Select(g => g.First())
                .ToList();
            // P6: active naam ka match case-insensitive — warna
            // "race" likhi active "Race" se nahi milti thi aur
            // fix-up khamoshi se pehla profile active bana deta tha.
            if (data.Profiles.All(p => !string.Equals(p.Name, data.ActiveProfileName, StringComparison.OrdinalIgnoreCase)))
                data.ActiveProfileName = data.Profiles[0].Name;

            // A10: v1 file migrate hui — in-memory nishani (Save is se
            // .v1.bak banata hai; file mein ye field save nahi hoti).
            if (data.SchemaVersion < ProfileFile.CurrentSchemaVersion)
            {
                data.SchemaVersion = ProfileFile.CurrentSchemaVersion;
                data.MigratedFromV1 = true;
            }
            return data;
        }
        catch (JsonException) { return null; }   // kharab file — agli koshish (backup)
        catch (IOException) { return null; }     // parhna na ho saka — agli koshish
        catch (ArgumentOutOfRangeException) { return null; }
        // SEC-C-11: ye exceptions pehle catch nahi hoti thin — bari ya
        // na-qabil-e-rasai file startup par persistent crash ban jati thi.
        catch (OutOfMemoryException) { return null; }
        catch (UnauthorizedAccessException) { return null; }
        catch (NotSupportedException) { return null; }
        catch (Exception) { return null; }       // koi bhi kharab file app ko band nahi karti
    }

    /// <summary>
    /// SEC-C-11: file ko cap ke saath parho — cap se zyada bytes milen
    /// to null (file kabhi poori RAM mein nahi aati). Length check pehle
    /// bhi hai, lekin parhte waqt chunk-wise ginna TOCTOU bhi band karta
    /// hai (check ke baad file barh jaye to bhi cap lagti hai).
    /// </summary>
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
        catch (IOException) { return null; }
        catch (UnauthorizedAccessException) { return null; }
        catch (OutOfMemoryException) { return null; }
        catch (NotSupportedException) { return null; }
        catch (Exception) { return null; }
    }

    private static string? TryGetString(JsonElement root, string name)
    {
        foreach (var prop in root.EnumerateObject())
        {
            if (string.Equals(prop.Name, name, StringComparison.OrdinalIgnoreCase) &&
                prop.Value.ValueKind == JsonValueKind.String)
                return prop.Value.GetString();
        }
        return null;
    }

    private static int TryGetInt(JsonElement root, string name)
    {
        foreach (var prop in root.EnumerateObject())
        {
            if (string.Equals(prop.Name, name, StringComparison.OrdinalIgnoreCase) &&
                prop.Value.ValueKind == JsonValueKind.Number &&
                prop.Value.TryGetInt32(out int value))
                return value;
        }
        return 0;
    }

    public void Save(ProfileFile data)
    {
        lock (_ioLock)
        {
            string dir = Path.GetDirectoryName(Path.GetFullPath(_filePath))!;
            Directory.CreateDirectory(dir);

            // SEC-C-11: save-side count cap — 64 se zyada profiles kabhi
            // disk par nahi jati (pehli 64 mehfooz rehti hain).
            if (data.Profiles.Count > MaxProfiles)
                data.Profiles = data.Profiles.Take(MaxProfiles).ToList();

            // A10: hamesha maujooda schema likho.
            data.SchemaVersion = ProfileFile.CurrentSchemaVersion;
            string json = JsonSerializer.Serialize(data, JsonOptions);

            // SEC-C-14: unique temp naam — do concurrent Save (UI +
            // auto-save) ek hi .tmp par nahi likhte; purana fixed .tmp
            // ka bacha kachra bhi saaf karo. Temp hamesha finally mein
            // hataya jata hai (nakami par bhi).
            string tempPath = _filePath + ".tmp-" + Guid.NewGuid().ToString("N");
            try
            {
                try { if (File.Exists(TempPath)) File.Delete(TempPath); } catch (IOException) { /* purana kachra — best-effort */ }
                File.WriteAllText(tempPath, json);
            }
            catch
            {
                try { if (File.Exists(tempPath)) File.Delete(tempPath); } catch (IOException) { }
                throw;
            }

            try
            {
                // A10: on-disk file abhi v1 ho to migration se PEHLE us ke
                // asal bytes .v1.bak mein mehfooz karo (sirf pehli dafa —
                // pehle se mojood v1 backup kabhi overwrite nahi hota).
                // Yehi backup downgrade par RestoreV1Backup() istemal karta hai.
                // SEC-C-11: bari file ki .v1.bak copy nahi banti.
                if (File.Exists(_filePath) && !File.Exists(V1BackupPath) &&
                    new FileInfo(_filePath).Length <= MaxFileBytes &&
                    OnDiskSchemaVersion(_filePath) < ProfileFile.CurrentSchemaVersion)
                {
                    try { File.Copy(_filePath, V1BackupPath); } catch (IOException) { /* backup best-effort hai — save nahi rukti */ }
                }

                // A3: 3-generation rotation — LEKIN sirf tab jab maujooda
                // file khud theek parse hoti ho (ParsesOk guard — SEC-C-12:
                // yehi shared strict validation hai jo Load istemal karta
                // hai: Sanitized + dedupe + count cap + version check).
                // Aur kharab .bak kabhi achi .bak1 ko overwrite nahi karta:
                // sirf ParsesOk backup aage barhta hai; kharab .bak hata
                // diya jata hai (us ki jagah maujooda achi file .bak banti hai).
                if (File.Exists(_filePath) && ParsesOk(_filePath))
                {
                    if (File.Exists(BackupPath))
                    {
                        if (ParsesOk(BackupPath))
                        {
                            if (File.Exists(Backup1Path))
                                File.Copy(Backup1Path, Backup2Path, overwrite: true);
                            File.Move(BackupPath, Backup1Path, overwrite: true);
                        }
                        else
                        {
                            File.Delete(BackupPath);   // kharab backup — aage nahi barhega
                        }
                    }
                    File.Copy(_filePath, BackupPath, overwrite: true);
                }
                else if (File.Exists(_filePath))
                {
                    // SEC-C-13: kharab current file ko tashkhees ke liye
                    // mehfooz karo (ek din mein ek copy — overwrite,
                    // jama nahi hoti), backup generations ko haath mat lagao.
                    PreserveCorruptCopy(_filePath);
                }
                File.Move(tempPath, _filePath, overwrite: true);
                data.MigratedFromV1 = false;
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
    /// chalta hai (wahi lock, wahi backup rotation, wahi atomic move),
    /// is liye sync/async Save mein logic ka farq kabhi nahi parta —
    /// doosri copy mein bug fix bhoolne ka khatra nahi. Save chand KB
    /// ka hota hai (ms), is liye ThreadPool offload hi kaafi hai; streaming
    /// async I/O ka faida yahan na hone ke barabar hota.
    /// </summary>
    public Task SaveAsync(ProfileFile data, CancellationToken cancellationToken = default) =>
        Task.Run(() => Save(data), cancellationToken);

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

    /// <summary>
    /// Downgrade rollback (A10, R-M9): .v1.bak se v1 file wapas lao.
    /// Pehle maujooda (v2) file .bak mein mehfooz hoti hai (agar theek
    /// parse hoti ho), phir v1 backup main file ban jata hai.
    /// Koi v1 backup na ho ya wo kharab ho to false — kuch nahi badalta.
    /// </summary>
    public bool RestoreV1Backup()
    {
        lock (_ioLock)
        {
            if (!File.Exists(V1BackupPath) || !ParsesOk(V1BackupPath)) return false;
            if (File.Exists(_filePath) && ParsesOk(_filePath))
            {
                try { File.Copy(_filePath, BackupPath, overwrite: true); } catch (IOException) { /* best-effort */ }
            }
            File.Copy(V1BackupPath, _filePath, overwrite: true);
            return true;
        }
    }

    /// <summary>v1 backup mojood hai aur theek parse hota hai?</summary>
    public bool HasV1Backup()
    {
        lock (_ioLock)
        {
            return File.Exists(V1BackupPath) && ParsesOk(V1BackupPath);
        }
    }

    private static int OnDiskSchemaVersion(string path)
    {
        try
        {
            string? json = TryReadCapped(path);
            if (json is null) return 0;
            using var doc = JsonDocument.Parse(json);
            return TryGetInt(doc.RootElement, "SchemaVersion");
        }
        catch { return 0; }
    }

    private static bool ParsesOk(string path)
    {
        try
        {
            // F3 ke baad ParsesOk ka matlab: kam se kam ek valid profile.
            return TryLoadFile(path) is not null;
        }
        catch { return false; }
    }

    /// <summary>
    /// SEC-E-07 / W8 (restore): ek profiles.json file ki SAKHT validation —
    /// Load wali shared validation (<see cref="TryLoadFile"/>: size cap
    /// 1 MB + version check + 64-count cap + har entry
    /// <see cref="WheelProfile.Sanitized"/> + dedupe + kam se kam ek
    /// valid profile), lekin Load se sakht: koi entry skip hui ho to
    /// bhi reject (restore aadha/na-mukammal na ho — khamoshi se entry
    /// girana yahan data loss hai, F3 tolerance yahan nahi chalti).
    /// True = file poori tarah theek; false + <paramref name="error"/> = wajah.
    /// </summary>
    public static bool TryValidateFile(string path, out string? error)
    {
        error = null;
        ProfileFile? data;
        try
        {
            data = TryLoadFile(path);
        }
        catch (Exception ex)
        {
            error = $"profiles.json in this backup is not valid ({ex.Message}).";
            return false;
        }
        if (data is null)
        {
            error = "profiles.json in this backup is not valid — check the version (max 2), at most 64 profiles, and that every profile entry is valid.";
            return false;
        }
        if (data.SkippedProfiles > 0)
        {
            error = $"profiles.json in this backup has {data.SkippedProfiles} invalid profile entr{(data.SkippedProfiles == 1 ? "y" : "ies")} — restore would silently drop them, so the backup is rejected.";
            return false;
        }
        return true;
    }

    /// <summary>
    /// Active profile — hamesha milega.
    /// P6: naam ka match case-insensitive hai. T15: khali list par
    /// crash nahi — sane default (Default profile) milta hai.
    /// </summary>
    public static WheelProfile GetActive(ProfileFile data)
    {
        if (data.Profiles.Count == 0)
            return WheelProfile.CreateDefault(data.ActiveProfileName switch
            {
                { Length: > 0 } name => name,
                _ => "Default",
            });
        return data.Profiles.FirstOrDefault(p =>
                string.Equals(p.Name, data.ActiveProfileName, StringComparison.OrdinalIgnoreCase))
            ?? data.Profiles[0];
    }

    /// <summary>
    /// SEC-C-19 (A9): import ki hui single-profile file ko validate karo —
    /// size cap + schema version + parse + <see cref="WheelProfile.Sanitized"/>.
    /// Na-qabil-e-qabool file par false + saaf wajah (error) — kabhi
    /// exception nahi phenkta, kabhi crash nahi hota. Version: field na ho
    /// to v1 samjho (qabool); 0..CurrentSchemaVersion qabool; is se zyada
    /// (mustaqbil ka version) ya ghalat qism reject. Caller bytes pehle
    /// se capped parh kar de (Import length check + capped read karta hai);
    /// ye method bhi cap dobara lagata hai (defense in depth).
    /// </summary>
    /// <returns>true + <paramref name="profile"/> (Sanitized), ya false + <paramref name="error"/> (wajah).</returns>
    public static bool TryParseImportedProfile(byte[] utf8Json, out WheelProfile? profile, out string? error)
    {
        profile = null;
        error = null;
        if (utf8Json.Length > MaxImportBytes)
        {
            error = $"Profile file is too large (max {MaxImportBytes / 1024} KB).";
            return false;
        }
        try
        {
            // JsonDocument.Parse ka ReadOnlySpan<byte> overload nahi hai —
            // is liye signature byte[] hai (→ ReadOnlyMemory<byte> overload).
            using var doc = JsonDocument.Parse(utf8Json);
            var root = doc.RootElement;
            if (root.ValueKind != JsonValueKind.Object)
            {
                error = "Profile file is not a JSON object.";
                return false;
            }
            // Version check — field case-insensitive dhoondo (export
            // "SchemaVersion" likhta hai; haath se bani file ka case
            // mukhtalif ho sakta hai).
            foreach (var prop in root.EnumerateObject())
            {
                if (!string.Equals(prop.Name, "SchemaVersion", StringComparison.OrdinalIgnoreCase)) continue;
                if (prop.Value.ValueKind != JsonValueKind.Number || !prop.Value.TryGetInt32(out int version))
                {
                    error = "Profile 'SchemaVersion' must be a whole number.";
                    return false;
                }
                if (version < 0 || version > ProfileFile.CurrentSchemaVersion)
                {
                    error = $"Profile version {version} is not supported — this app supports up to version {ProfileFile.CurrentSchemaVersion}.";
                    return false;
                }
                break;
            }
            var parsed = JsonSerializer.Deserialize<WheelProfile>(utf8Json, JsonOptions);
            if (parsed is null)
            {
                error = "Profile file is empty.";
                return false;
            }
            profile = parsed.Sanitized();
            return true;
        }
        catch (JsonException ex)
        {
            error = $"Invalid profile file: {ex.Message}";
            return false;
        }
        catch (ArgumentOutOfRangeException ex)
        {
            // P2: button 33+ jaisi na-jaiz qeemat — Sanitized() throw karta hai.
            error = $"Invalid profile file: {ex.Message}";
            return false;
        }
        catch (Exception ex)
        {
            error = $"Invalid profile file: {ex.Message}";
            return false;
        }
    }
}
