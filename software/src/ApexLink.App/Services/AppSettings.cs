using System.IO;
using System.Text.Json;

namespace ApexLink.App.Services;

/// <summary>App ki apni settings (profiles se alag — ye machine/user ki pasandein hain).</summary>
public sealed class AppSettings
{
    public string Theme { get; set; } = "System";          // System | Dark | Light
    public bool Autostart { get; set; }
    public bool StartMinimized { get; set; }
    public bool CloseToTray { get; set; } = true;
    public bool OutputEnabled { get; set; } = true;
    public string EspIp { get; set; } = "192.168.4.1";
    public int VJoyDeviceId { get; set; } = 1;
    public string LogLevel { get; set; } = "Warning";      // Debug | Information | Warning | Error
    public bool WizardCompleted { get; set; }
    public bool FirstRunDone { get; set; }
    /// <summary>Samne wali game ke liye profile khud load ho (Profiles page par har profile ka game naam).</summary>
    public bool AutoSwitchProfiles { get; set; } = true;
    /// <summary>Design v2 ki ek-dafa theme migration ho gayi?</summary>
    public bool ThemeMigratedV12 { get; set; }
    /// <summary>Wheel ke APEXLINK Wi-Fi ka password — auto-connect isi se profile banata hai.</summary>
    public string WheelWifiPassword { get; set; } = "change-me-123";
}

/// <summary>Settings ka wohi crash-safe tareeqa jo profiles ka hai: atomic write + backup + defaults par wapasi.</summary>
public sealed class AppSettingsStore
{
    private static readonly JsonSerializerOptions JsonOptions = new() { WriteIndented = true, PropertyNameCaseInsensitive = true };
    private readonly string _filePath;

    public AppSettingsStore(string filePath) => _filePath = filePath;
    public string FilePath => _filePath;

    public AppSettings Load()
    {
        foreach (string path in new[] { _filePath, _filePath + ".bak" })
        {
            try
            {
                if (!File.Exists(path)) continue;
                var s = JsonSerializer.Deserialize<AppSettings>(File.ReadAllText(path), JsonOptions);
                if (s is not null) return Sanitize(s);
            }
            catch (Exception) { /* kharab file — backup/defaults */ }
        }
        return new AppSettings();
    }

    public void Save(AppSettings settings)
    {
        string dir = Path.GetDirectoryName(Path.GetFullPath(_filePath))!;
        Directory.CreateDirectory(dir);
        string tmp = _filePath + ".tmp";
        File.WriteAllText(tmp, JsonSerializer.Serialize(Sanitize(settings), JsonOptions));
        if (File.Exists(_filePath)) File.Copy(_filePath, _filePath + ".bak", overwrite: true);
        File.Move(tmp, _filePath, overwrite: true);
    }

    private static AppSettings Sanitize(AppSettings s)
    {
        if (s.Theme is not ("System" or "Dark" or "Light")) s.Theme = "System";
        if (s.LogLevel is not ("Debug" or "Information" or "Warning" or "Error")) s.LogLevel = "Warning";
        if (string.IsNullOrWhiteSpace(s.EspIp)) s.EspIp = "192.168.4.1";
        s.VJoyDeviceId = Math.Clamp(s.VJoyDeviceId, 1, 16);
        return s;
    }
}
