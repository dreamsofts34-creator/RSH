using System.Text.Json;

namespace ApexLink.Core.Profiles;

/// <summary>profiles.json ki andar wali shakal.</summary>
public sealed class ProfileFile
{
    public List<WheelProfile> Profiles { get; set; } = [];
    public string ActiveProfileName { get; set; } = "Default";
}

/// <summary>
/// Profiles ka crash-safe store:
/// - Save atomic hai: pehle .tmp likho, phir purani file ko .bak bana kar
///   replace — bijli jaye ya app crash ho, file kabhi aadhi nahi milti.
/// - Load: file kharab ho to .bak se, wo bhi kharab ho to defaults se —
///   app corrupt profile par band NAHI hota.
/// - Har loaded profile Sanitized() se guzarta hai.
/// </summary>
public sealed class ProfileStore
{
    private static readonly JsonSerializerOptions JsonOptions = new()
    {
        WriteIndented = true,
        PropertyNameCaseInsensitive = true,
    };

    private readonly string _filePath;

    public ProfileStore(string filePath)
    {
        _filePath = filePath;
    }

    public string FilePath => _filePath;
    private string BackupPath => _filePath + ".bak";
    private string TempPath => _filePath + ".tmp";

    public ProfileFile Load()
    {
        foreach (string path in new[] { _filePath, BackupPath })
        {
            try
            {
                if (!File.Exists(path)) continue;
                string json = File.ReadAllText(path);
                var data = JsonSerializer.Deserialize<ProfileFile>(json, JsonOptions);
                if (data is not null && data.Profiles.Count > 0)
                {
                    // AUDIT FIX: ek naam ke do profiles hon to GetActive
                    // hamesha pehla deta tha aur fix-up khamoshi se active
                    // badal sakta tha — naam (case-insensitive) par dedupe.
                    data.Profiles = data.Profiles
                        .Select(p => p.Sanitized())
                        .GroupBy(p => p.Name, StringComparer.OrdinalIgnoreCase)
                        .Select(g => g.First())
                        .ToList();
                    if (data.Profiles.All(p => p.Name != data.ActiveProfileName))
                        data.ActiveProfileName = data.Profiles[0].Name;
                    return data;
                }
            }
            catch (JsonException) { /* kharab file — agli koshish (backup) */ }
            catch (IOException) { /* parhna na ho saka — agli koshish */ }
        }

        var fresh = new ProfileFile
        {
            Profiles = [WheelProfile.CreateDefault()],
            ActiveProfileName = "Default",
        };
        return fresh;
    }

    public void Save(ProfileFile data)
    {
        string dir = Path.GetDirectoryName(Path.GetFullPath(_filePath))!;
        Directory.CreateDirectory(dir);

        string json = JsonSerializer.Serialize(data, JsonOptions);
        File.WriteAllText(TempPath, json);

        // Purani file ko backup banao — LEKIN sirf tab jab wo khud theek
        // parse hoti ho. AUDIT FIX: mojooda file kharab ho to usi kharab
        // file se acha .bak overwrite ho jata tha, yani backup bilkul usi
        // waqt bekaar hota tha jab us ki sab se zyada zaroorat thi.
        if (File.Exists(_filePath) && ParsesOk(_filePath))
            File.Copy(_filePath, BackupPath, overwrite: true);
        File.Move(TempPath, _filePath, overwrite: true);
    }

    private static bool ParsesOk(string path)
    {
        try
        {
            var data = JsonSerializer.Deserialize<ProfileFile>(File.ReadAllText(path), JsonOptions);
            return data is not null && data.Profiles.Count > 0;
        }
        catch { return false; }
    }

    /// <summary>Active profile (hamesha milega — Load guarantees kam se kam ek profile).</summary>
    public static WheelProfile GetActive(ProfileFile data) =>
        data.Profiles.FirstOrDefault(p => p.Name == data.ActiveProfileName) ?? data.Profiles[0];
}
