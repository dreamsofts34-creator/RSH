using System.Diagnostics;
using System.IO;
using System.IO.Compression;
using ApexLink.App.Services;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace ApexLink.App.ViewModels;

public sealed partial class DiagnosticsViewModel(AppServices services) : ObservableObject, ILiveViewModel
{
    [ObservableProperty] private string _linkText = "";
    [ObservableProperty] private string _engineText = "";
    [ObservableProperty] private string _processText = "";
    [ObservableProperty] private string _statusText = "";

    private readonly Process _process = Process.GetCurrentProcess();
    private TimeSpan _lastCpu;
    private DateTime _lastCpuAt = DateTime.UtcNow;
    private readonly DateTime _startedAt = DateTime.UtcNow;
    // AUDIT FIX (F1): cpuPct pehle har Tick() par 0 se shuru hota tha aur
    // ProcessText har tick overwrite hota tha — screen par value 0.0% aur
    // asal % ke darmiyan flicker karti thi. Ab ye field hai jo sirf
    // >=0.5s wale sample par update hoti hai; display hamesha isi se hai.
    private double _cpuPct;

    public void Tick()
    {
        var st = services.Source.Statistics;
        LinkText =
            $"Rate {st.CurrentRatePerSecond} pkt/s · Valid {st.ValidPackets:N0} · Lost {st.LostPackets:N0} ({st.LossPercent:F2}%) · " +
            $"CRC {st.BadCrc:N0} · Magic {st.BadMagic:N0} · Version {st.BadVersion:N0} · Short {st.TooShort:N0} · Dup/late {st.DuplicatesOrLate:N0}";

        var e = services.Engine;
        long avgTick = e.TickCount > 0 ? e.TotalTickDurationUs / e.TickCount : 0;
        EngineText =
            $"Ticks {e.TickCount:N0} · avg tick {avgTick} µs · max tick {e.MaxTickDurationUs} µs · late ticks {e.LateTickCount:N0} · " +
            $"watchdog {(e.WatchdogActive ? "ACTIVE (neutral output)" : "off")} · output {(e.OutputConnected ? "connected" : "not connected")}";

        _process.Refresh();
        var now = DateTime.UtcNow;
        double elapsed = (now - _lastCpuAt).TotalSeconds;
        if (elapsed >= 0.5)
        {
            var cpu = _process.TotalProcessorTime;
            _cpuPct = (cpu - _lastCpu).TotalSeconds / elapsed / Environment.ProcessorCount * 100;
            _lastCpu = cpu;
            _lastCpuAt = now;
        }
        ProcessText =
            $"CPU {_cpuPct:F1}% · Working set {_process.WorkingSet64 / 1048576.0:F1} MB · GC heap {GC.GetTotalMemory(false) / 1048576.0:F1} MB · " +
            $"Gen2 collections {GC.CollectionCount(2)} · Uptime {(now - _startedAt):hh\\:mm\\:ss}";
    }

    [RelayCommand]
    private void OpenLogsFolder() => OpenFolder(services.LogDirectory);

    [RelayCommand]
    private void OpenConfigFolder() => OpenFolder(services.DataDirectory);

    private void OpenFolder(string path)
    {
        try { Process.Start(new ProcessStartInfo(path) { UseShellExecute = true }); }
        catch (Exception ex) { StatusText = ex.Message; }
    }

    [RelayCommand]
    private void ExportBundle()
    {
        try
        {
            string stamp = DateTime.Now.ToString("yyyyMMdd-HHmmss");
            string zipPath = Path.Combine(services.DataDirectory, $"apexlink-diagnostics-{stamp}.zip");
            using (var zip = ZipFile.Open(zipPath, ZipArchiveMode.Create))
            {
                foreach (string log in Directory.GetFiles(services.LogDirectory, "*.log"))
                    zip.CreateEntryFromFile(log, "logs/" + Path.GetFileName(log));
                var entry = zip.CreateEntry("diagnostics.txt");
                using var writer = new StreamWriter(entry.Open());
                writer.WriteLine($"ApexLink diagnostics — {DateTime.Now}");
                writer.WriteLine(LinkText);
                writer.WriteLine(EngineText);
                writer.WriteLine(ProcessText);
                writer.WriteLine($"Profile: {services.Profiles.ActiveProfileName}");
                writer.WriteLine($"Settings: ESP {services.Settings.EspIp}, vJoy device {services.Settings.VJoyDeviceId}");
            }
            StatusText = $"Saved: {zipPath}";
        }
        catch (Exception ex) { StatusText = $"Export failed: {ex.Message}"; }
    }
}

public sealed partial class SettingsViewModel(AppServices services) : ObservableObject
{
    public string[] Themes { get; } = ["System", "Dark", "Light"];
    public string[] LogLevels { get; } = ["Debug", "Information", "Warning", "Error"];

    [ObservableProperty] private string _theme = services.Settings.Theme;
    [ObservableProperty] private bool _autostart = WindowsIntegration.GetAutostart();
    [ObservableProperty] private bool _startMinimized = services.Settings.StartMinimized;
    [ObservableProperty] private bool _closeToTray = services.Settings.CloseToTray;
    [ObservableProperty] private bool _autoSwitchProfiles = services.Settings.AutoSwitchProfiles;
    [ObservableProperty] private string _logLevel = services.Settings.LogLevel;
    [ObservableProperty] private string _statusText = "";

    public string ConfigPath => services.DataDirectory;

    // AUDIT FIX (F15): version pehle hardcode "1.0.0" tha — agle bump par
    // About purana version dikhata. Ab assembly se parho.
    public string VersionText
    {
        get
        {
            var v = System.Reflection.Assembly.GetExecutingAssembly().GetName().Version;
            string ver = v is null ? "?" : $"{v.Major}.{v.Minor}.{v.Build}";
            return $"ApexLink {ver} · by Dreamsofts · .NET {Environment.Version}";
        }
    }

    public Action? RequestWizard { get; set; }

    partial void OnThemeChanged(string value)
    {
        services.Settings.Theme = value;
        services.SaveSettings();
        App.ApplyTheme(value);
    }

    partial void OnAutostartChanged(bool value)
    {
        WindowsIntegration.SetAutostart(value);
        services.Settings.Autostart = value;
        services.SaveSettings();
    }

    partial void OnStartMinimizedChanged(bool value)
    {
        services.Settings.StartMinimized = value;
        services.SaveSettings();
    }

    partial void OnCloseToTrayChanged(bool value)
    {
        services.Settings.CloseToTray = value;
        services.SaveSettings();
    }

    partial void OnAutoSwitchProfilesChanged(bool value)
    {
        services.Settings.AutoSwitchProfiles = value;
        services.SaveSettings();
    }

    partial void OnLogLevelChanged(string value)
    {
        services.Settings.LogLevel = value;
        services.SaveSettings();
        StatusText = "Log level applies from the next app start.";
    }

    [RelayCommand] private void RunWizard() => RequestWizard?.Invoke();

    [RelayCommand]
    private void OpenConfig()
    {
        try { Process.Start(new ProcessStartInfo(services.DataDirectory) { UseShellExecute = true }); }
        catch (Exception ex) { StatusText = ex.Message; }
    }
}
