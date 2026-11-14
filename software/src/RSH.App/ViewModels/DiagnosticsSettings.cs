using System.Collections.ObjectModel;
using System.Diagnostics;
using System.IO;
using System.IO.Compression;
using System.Net.Http;
using System.Text.Json;
using RSH.App.Resources;
using RSH.App.Services;
using RSH.App.Views;
using RSH.Core.Net;
using RSH.Core.Pipeline;
using RSH.Core.Profiles;
using RSH.Core.Security;
using RSH.Core.Transport;
using RSH.Core.Utilities;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using Microsoft.Win32;

namespace RSH.App.ViewModels;

public sealed partial class DiagnosticsViewModel(AppServices services) : ObservableObject, ILiveViewModel
{
    [ObservableProperty] private string _linkText = "";
    [ObservableProperty] private string _engineText = "";
    [ObservableProperty] private string _processText = "";
    [ObservableProperty] private string _statusText = "";
    [ObservableProperty] private string _logStatusText = "";

    /// <summary>Monitor → Logs (UI-10, U-P5): aakhri 200 lines, virtualized ListBox mein. Taaza sirf har 2 s (Tick 30 Hz par file parhna I/O hoga).</summary>
    public System.Collections.ObjectModel.ObservableCollection<string> LogLines { get; } = [];

    private readonly Process _process = Process.GetCurrentProcess();
    private TimeSpan _lastCpu;
    private DateTime _lastCpuAt = DateTime.UtcNow;
    private readonly DateTime _startedAt = DateTime.UtcNow;
    // AUDIT FIX (F1): cpuPct pehle har Tick() par 0 se shuru hota tha aur
    // ProcessText har tick overwrite hota tha — screen par value 0.0% aur
    // asal % ke darmiyan flicker karti thi. Ab ye field hai jo sirf
    // >=0.5s wale sample par update hoti hai; display hamesha isi se hai.
    private double _cpuPct;
    // Tick jitter (engine loop ka asal dt, nominal 5 ms se inhiraf) ka
    // EMA — Dashboard jaisa hisaab, Monitor ke Performance panel ke liye.
    private double _tickJitterEmaMs;
    private DateTime _lastLogRefresh = DateTime.MinValue;
    private bool _logsRefreshing;

    public void Tick()
    {
        var st = services.Source.Statistics;
        LinkText =
            $"Rate {st.CurrentRatePerSecond} pkt/s · Valid {st.ValidPackets:N0} · Lost {st.LostPackets:N0} ({st.LossPercent:F2}%) · " +
            $"CRC {st.BadCrc:N0} · Magic {st.BadMagic:N0} · Version {st.BadVersion:N0} · Short {st.TooShort:N0} · Dup/late {st.DuplicatesOrLate:N0}";

        var e = services.Engine;
        long avgTick = e.TickCount > 0 ? e.TotalTickDurationUs / e.TickCount : 0;
        double dev = Math.Abs(e.LastDtSeconds * 1000.0 - 5.0);
        _tickJitterEmaMs = _tickJitterEmaMs * 0.9 + dev * 0.1;
        EngineText =
            $"Ticks {e.TickCount:N0} · avg tick {avgTick} µs · max tick {e.MaxTickDurationUs} µs · tick jitter {_tickJitterEmaMs:F2} ms · late ticks {e.LateTickCount:N0} · " +
            $"watchdog {(e.WatchdogActive ? "ACTIVE (neutral output)" : "off")} · output {(e.OutputConnected ? "connected" : "not connected")}";

        // W4 (Round 2): Refresh() sirf CPU% sample par — har 33 ms Tick
        // par Refresh() bekaar tha (CPU% sirf 0.5 s par recompute hota
        // hai); WorkingSet64 display 0.5 s stale bhi theek hai (diagnostics
        // panel, 30 Hz par number waise bhi parha nahi jata).
        var now = DateTime.UtcNow;
        double elapsed = (now - _lastCpuAt).TotalSeconds;
        if (elapsed >= 0.5)
        {
            _process.Refresh();
            var cpu = _process.TotalProcessorTime;
            _cpuPct = (cpu - _lastCpu).TotalSeconds / elapsed / Environment.ProcessorCount * 100;
            _lastCpu = cpu;
            _lastCpuAt = now;
        }
        ProcessText =
            $"CPU {_cpuPct:F1}% · Working set {_process.WorkingSet64 / 1048576.0:F1} MB · GC heap {GC.GetTotalMemory(false) / 1048576.0:F1} MB · " +
            $"Gen2 collections {GC.CollectionCount(2)} · Uptime {(now - _startedAt):hh\\:mm\\:ss}";

        // Logs: har 2 s se zyada taaza nahi (U-P1: file I/O background par).
        if ((now - _lastLogRefresh) > TimeSpan.FromSeconds(2))
        {
            _lastLogRefresh = now;
            _ = RefreshLogsAsync();
        }
    }

    /// <summary>Logs view taaza karo — nayi rsh-*.log file ki aakhri 200 lines, background par parhi hui.</summary>
    [RelayCommand]
    public async Task RefreshLogsAsync()
    {
        if (_logsRefreshing) return;
        _logsRefreshing = true;
        try
        {
            string logDir = services.LogDirectory;
            string[] lines = await Task.Run(() => ReadTailLines(logDir, 200)).ConfigureAwait(false);
            RunOnUi(() =>
            {
                LogLines.Clear();
                foreach (string line in lines) LogLines.Add(line);
                LogStatusText = lines.Length == 0
                    ? "No log lines yet."
                    : $"{lines.Length} recent lines from the newest log file.";
            });
        }
        catch (Exception ex)
        {
            RunOnUi(() => LogStatusText = $"Could not read logs: {ex.Message}");
        }
        finally
        {
            _logsRefreshing = false;
        }
    }

    private static void RunOnUi(Action action)
    {
        var dispatcher = System.Windows.Application.Current?.Dispatcher;
        if (dispatcher is not null && !dispatcher.CheckAccess())
            dispatcher.Invoke(action);
        else
            action();
    }

    /// <summary>Nayi log file ki aakhri maxLines lines — poori file RAM mein nahi (rolling queue).</summary>
    private static string[] ReadTailLines(string logDir, int maxLines)
    {
        try
        {
            if (!Directory.Exists(logDir)) return [];
            string? newest = Directory.GetFiles(logDir, "rsh-*.log")
                .Select(f => new FileInfo(f))
                .OrderByDescending(f => f.LastWriteTimeUtc)
                .Select(f => f.FullName)
                .FirstOrDefault();
            if (newest is null) return [];
            var tail = new Queue<string>(maxLines);
            foreach (string line in File.ReadLines(newest))
            {
                if (tail.Count >= maxLines) tail.Dequeue();
                tail.Enqueue(line);
            }
            return tail.ToArray();
        }
        catch (Exception) { return []; }
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
    private async Task ExportBundle()
    {
        // U-P1: zip banana file I/O hai — UI thread par kabhi nahi.
        // Pehle UI-thread strings snapshot karo, phir zip background par.
        string stamp = DateTime.Now.ToString("yyyyMMdd-HHmmss");
        string zipPath = Path.Combine(services.DataDirectory, $"rsh-diagnostics-{stamp}.zip");
        string logDir = services.LogDirectory;
        string link = LinkText, engine = EngineText, process = ProcessText;
        string profile = services.Profiles.ActiveProfileName;
        string esp = services.Settings.EspIp;
        int vjoy = services.Settings.VJoyDeviceId;
        try
        {
            await Task.Run(() => WriteBundle(zipPath, logDir, link, engine, process, profile, esp, vjoy));
            StatusText = $"Saved: {zipPath}";
        }
        catch (Exception ex) { StatusText = $"Export failed: {ex.Message}"; }
    }

    private static void WriteBundle(string zipPath, string logDir, string link, string engine,
        string process, string profile, string esp, int vjoy)
    {
        using (var zip = ZipFile.Open(zipPath, ZipArchiveMode.Create))
        {
            foreach (string log in Directory.GetFiles(logDir, "*.log"))
                zip.CreateEntryFromFile(log, "logs/" + Path.GetFileName(log));
            var entry = zip.CreateEntry("diagnostics.txt");
            using var writer = new StreamWriter(entry.Open());
            writer.WriteLine($"RSH diagnostics — {DateTime.Now}");
            writer.WriteLine(link);
            writer.WriteLine(engine);
            writer.WriteLine(process);
            writer.WriteLine($"Profile: {profile}");
            writer.WriteLine($"Settings: ESP {esp}, vJoy device {vjoy}");
        }
    }
}

public sealed partial class SettingsViewModel(AppServices services) : ObservableObject
{
    public string[] Themes { get; } = ["System", "Dark", "Light", "HighContrast"];
    public string[] LogLevels { get; } = ["Debug", "Information", "Warning", "Error"];

    /// <summary>Phase F i18n: language options (Value = settings mein mehfooz, Display = ComboBox mein).</summary>
    public LanguageOption[] Languages { get; } =
    [
        new("System", "System"),
        new("en", "English"),
        new("ur", "اردو (Urdu)"),
        new("hi", "हिन्दी (Hindi)"),
    ];

    [ObservableProperty] private string _theme = services.Settings.Theme;
    [ObservableProperty] private string _language = services.Settings.Language;
    [ObservableProperty] private bool _autostart = WindowsIntegration.GetAutostart();
    [ObservableProperty] private bool _startMinimized = services.Settings.StartMinimized;
    [ObservableProperty] private bool _closeToTray = services.Settings.CloseToTray;
    [ObservableProperty] private bool _autoSwitchProfiles = services.Settings.AutoSwitchProfiles;
    [ObservableProperty] private string _logLevel = services.Settings.LogLevel;
    [ObservableProperty] private string _statusText = "";

    /// <summary>
    /// Phase H WS5: transport switch (Wi-Fi ↔ USB-serial) — asal logic
    /// <see cref="TransportSwitchController"/> mein hai (WPF-free,
    /// testable); Settings page sirf isay expose karta hai. SettingsPage
    /// <c>Transport.IsUsbSerial</c> / <c>Transport.SelectedPortName</c> /
    /// <c>Transport.AvailablePorts</c> / <c>Transport.StatusText</c> se
    /// bind karta hai.
    /// </summary>
    public TransportSwitchController Transport { get; } = new(services, SerialDeviceDetector.GetSystemPortNames);

    /// <summary>Transport → Refresh ports (dropdown taaza).</summary>
    [RelayCommand]
    private void RefreshTransportPorts() => Transport.RefreshPorts();

    // A5: failure par toggle wapas karte waqt OnAutostartChanged dobara
    // na chale (warna loop) — is liye guard flag.
    private bool _applyingAutostart;

    public string ConfigPath => services.DataDirectory;

    // UI-11 Modules (sirf yahin): 10 module toggles — profile ki
    // ModuleTunings se. Tuning page ke cards isi se sync hote hain
    // (wo har tick profile parhta hai + ProfileChanged sunta hai).
    public ObservableCollection<ModuleToggleRow> ModuleRows { get; } = [];

    // UI-11 GPIO pin map (sirf yahin): har function ka pin dropdown +
    // live validation (PinMap). Apply sirf zero errors par.
    public ObservableCollection<GpioPinRow> PinRows { get; } = [];
    [ObservableProperty] private string _pinValidationText = "";
    [ObservableProperty] private bool _canApplyPins;
    [ObservableProperty] private string _pinPushStatusText = "";
    [ObservableProperty] private bool _pinPushBusy;

    // UI-11 Data (sirf yahin): Backup / Restore / Open config folder /
    // Change wheel password.
    [ObservableProperty] private string _dataStatusText = "";

    // A13 (Phase H): update check ka nateeja — Settings → About mein dikhta hai.
    [ObservableProperty] private string _updateStatusText = "";
    [ObservableProperty] private bool _updateCheckBusy;

    private readonly PinMapStore _pinStore = new(services.DataDirectory);
    private bool _loadingModules;
    private bool _backupBusy;
    private bool _restoreBusy;
    // SEC-E-07 / W8-1: aakhri restore se pehle ka snapshot dir (success
    // message mein dikhate hain — user ko wapas aane ka rasta maloom ho).
    private string? _lastRestoreSnapshotDir;

    // Primary-constructor class ka koi ctor body nahi — modules/pins ka
    // init SettingsPage se hota hai (EnsureLoaded, ek dafa). Field
    // initializer se instance method / `this` nahi milta (CS0236).
    private bool _initialized;

    /// <summary>Settings page navigation par ek dafa: modules + pins load karo + ProfileChanged subscribe karo.</summary>
    public void EnsureLoaded()
    {
        if (_initialized) return;
        _initialized = true;
        LoadModules();
        LoadPins();
        ViewModelStore.ProfileChanged += LoadModules;
    }

    // Phase F i18n: module naam/subtitle resx se — Settings rows aur
    // Tuning cards ek hi zuban mein (pehle hard-coded English thay).
    private static string ModuleDisplayName(ModuleKind module) => module switch
    {
        ModuleKind.Steering => UiStrings.GetString("Steering"),
        ModuleKind.Accelerator => UiStrings.GetString("Accelerator"),
        ModuleKind.Brake => UiStrings.GetString("Brake"),
        ModuleKind.Clutch => UiStrings.GetString("ModuleClutch"),
        ModuleKind.Handbrake => UiStrings.GetString("ModuleHandbrake"),
        ModuleKind.Shifter => UiStrings.GetString("ModuleShifter"),
        ModuleKind.Horn => UiStrings.GetString("Horn"),
        ModuleKind.Buttons => UiStrings.GetString("ModuleButtons"),
        ModuleKind.Encoders => UiStrings.GetString("ModuleEncoders"),
        ModuleKind.LED => UiStrings.GetString("StatusLED"),
        _ => module.ToString(),
    };

    private static string ModuleSubtitle(ModuleKind module) => module switch
    {
        ModuleKind.Steering => UiStrings.GetString("ModuleSubSteering"),
        ModuleKind.Accelerator => UiStrings.GetString("ModuleSubAccelerator"),
        ModuleKind.Brake => UiStrings.GetString("ModuleSubBrake"),
        ModuleKind.Clutch => UiStrings.GetString("ModuleSubClutch"),
        ModuleKind.Handbrake => UiStrings.GetString("ModuleSubHandbrake"),
        ModuleKind.Shifter => UiStrings.GetString("ModuleSubShifter"),
        ModuleKind.Horn => UiStrings.GetString("ModuleSubHorn"),
        ModuleKind.Buttons => UiStrings.GetString("ModuleSubButtons"),
        ModuleKind.Encoders => UiStrings.GetString("ModuleSubEncoders"),
        ModuleKind.LED => UiStrings.GetString("ModuleSubLed"),
        _ => "",
    };

    private void LoadModules()
    {
        _loadingModules = true;
        try
        {
            var plan = PipelinePlanBuilder.FromProfile(services.ActiveProfile);
            ModuleRows.Clear();
            foreach (var module in PipelinePlan.AllModules)
                ModuleRows.Add(new ModuleToggleRow(module, ModuleDisplayName(module),
                    ModuleSubtitle(module), plan.IsEnabled(module), OnModuleToggled));
        }
        finally
        {
            _loadingModules = false;
        }
    }

    /// <summary>Module toggle (sirf yahin): active profile ki ModuleTunings mein Enabled badlo + engine par laagu karo.</summary>
    private async void OnModuleToggled(ModuleToggleRow row)
    {
        if (_loadingModules) return;
        var profile = services.ActiveProfile;
        var tunings = new Dictionary<string, PerModuleTuning>(profile.ModuleTunings, StringComparer.OrdinalIgnoreCase);
        PerModuleTuning current = tunings.TryGetValue(row.Module.ToString(), out var existing) && existing is not null
            ? existing : new PerModuleTuning();
        tunings[row.Module.ToString()] = current with { Enabled = row.Enabled };
        try
        {
            await services.UpdateActiveProfileAsync(profile with { ModuleTunings = tunings });
            ViewModelStore.RaiseProfileChanged();
            StatusText = row.Enabled
                ? $"{row.Name} on — its data reaches the game."
                : $"{row.Name} off — its data no longer reaches the game.";
        }
        catch (Exception ex)
        {
            StatusText = $"Could not save: {ex.Message}";
            LoadModules();
        }
    }

    private void LoadPins()
    {
        var saved = _pinStore.Load();
        PinRows.Clear();
        foreach (var function in PinMap.Functions)
        {
            int? pin = saved.TryGetValue(function.Name, out var p) ? p : function.DefaultPin;
            string kindLabel = function.Kind switch
            {
                PinFunctionKind.Analog => "Analog · ADC1",
                PinFunctionKind.Digital => "Digital in",
                PinFunctionKind.StatusLed => "Digital out",
                PinFunctionKind.LedStrip => "LED strip",
                _ => "",
            };
            PinRows.Add(new GpioPinRow(function.Name, kindLabel, pin, OnPinChanged));
        }
        ValidatePins();
    }

    private void OnPinChanged(GpioPinRow row) => ValidatePins();

    /// <summary>Live validation (PIN-RULES.md): har row ka error/warning + duplicate check; Apply sirf zero errors par.</summary>
    private void ValidatePins()
    {
        var pins = PinRows.ToDictionary(r => r.Function, r => r.SelectedPin, StringComparer.Ordinal);
        var (results, errorCount) = PinMap.ValidateAll(pins);
        foreach (var row in PinRows)
        {
            var (error, warning) = results[row.Function];
            row.ErrorText = error ?? "";
            row.WarningText = warning ?? "";
        }
        CanApplyPins = errorCount == 0;
        PinValidationText = errorCount > 0
            ? $"{errorCount} error{(errorCount == 1 ? "" : "s")} — fix them before applying."
            : results.Values.Any(v => v.Warning is not null)
                ? "No errors — check the warnings above before applying."
                : "No errors — ready to apply.";
    }

    [RelayCommand]
    private async Task ApplyPins()
    {
        if (!CanApplyPins) return;
        var pins = PinRows.ToDictionary(r => r.Function, r => r.SelectedPin, StringComparer.Ordinal);
        try
        {
            await Task.Run(() => _pinStore.Save(pins));
            StatusText = "Pin map saved. Setup → Pins shows the new map. The board keeps its own wiring until a SET_CFG push lands — recalibrate if the wiring changed.";
        }
        catch (Exception ex)
        {
            StatusText = $"Could not save the pin map: {ex.Message}";
        }
    }

    /// <summary>Unsaved pin edits wapas lo (saved map dobara parho).</summary>
    [RelayCommand]
    private void ResetPins()
    {
        LoadPins();
        StatusText = "Pin map reset to the saved values.";
    }

    /// <summary>
    /// Pin map board par push karo (Phase F W3 wiring): on-screen map pehle
    /// disk par save hoti hai (Apply jaisa), phir PinMapBoardPush use
    /// SET_CFG chunks → COMMIT → CONFIRM bhejta hai — signed sender
    /// BoardSession.SendControlAsync hai (key/nonce/counter session ke).
    /// NOTE (docs/PIN-MAP-PUSH.md): board blob store + persist karta hai,
    /// lekin live input abhi purani pins par chalta hai — firmware
    /// inputs.cpp → txn blob Phase G ka kaam hai. Is liye kamyabi ka matlab
    /// "board ne config qabool ki" hai, "wheel nayi pins par chal raha hai"
    /// nahi; pin badle to recalibrate karo (calibration stale ho jati hai).
    /// Session (HELLO/pairing) na ho to board tak kuch nahi jata — saaf
    /// wajah status mein aati hai.
    /// </summary>
    [RelayCommand]
    private async Task PushPinsToBoard()
    {
        if (PinPushBusy) return;
        if (!CanApplyPins)
        {
            PinPushStatusText = "Fix the pin errors above before pushing to the board.";
            return;
        }
        PinPushBusy = true;
        try
        {
            var pins = PinRows.ToDictionary(r => r.Function, r => r.SelectedPin, StringComparer.Ordinal);
            await Task.Run(() => _pinStore.Save(pins));
            var (ok, error, chunks) = await PinMapBoardPush.PushAsync(
                pins,
                (cmd, payload, ct) => services.BoardSession.SendControlAsync(
                    services.Settings.EspIp, cmd, payload, services.ControlTransportOverride, ct));
            PinPushStatusText = ok
                ? $"Pin map sent to the board ({chunks} chunk{(chunks == 1 ? "" : "s")}) — board stored it. The wheel still runs on its previous wiring until the firmware applies the new map (Phase G); recalibrate if the wiring changed."
                : $"Could not push the pin map: {error}";
        }
        catch (Exception ex)
        {
            PinPushStatusText = $"Could not push the pin map: {ex.Message}";
        }
        finally
        {
            PinPushBusy = false;
        }
    }

    /// <summary>Data → Backup now: profiles + settings + pin map, ek zip mein (U-P1: zip background par).</summary>
    [RelayCommand]
    private async Task BackupNow()
    {
        if (_backupBusy) return;
        _backupBusy = true;
        try
        {
            string stamp = DateTime.Now.ToString("yyyyMMdd-HHmmss");
            string zipPath = Path.Combine(services.DataDirectory, $"rsh-backup-{stamp}.zip");
            await Task.Run(() => WriteBackup(zipPath));
            DataStatusText = $"Backup saved: {zipPath}";
        }
        catch (Exception ex)
        {
            DataStatusText = $"Backup failed: {ex.Message}";
        }
        finally
        {
            _backupBusy = false;
        }
    }

    private void WriteBackup(string zipPath)
    {
        using var zip = ZipFile.Open(zipPath, ZipArchiveMode.Create);
        foreach (string name in new[] { "profiles.json", "pin-map.json" })
        {
            string path = Path.Combine(services.DataDirectory, name);
            if (File.Exists(path)) zip.CreateEntryFromFile(path, name);
        }
        // SEC-E-04 / backup leak (Phase F W8): settings.json ko zip mein
        // kabhi verbatim mat dalo — purani disk file mein plaintext
        // WheelWifiPassword ho sakta hai aur har rsh-backup-*.zip use leak
        // karta. Live settings se serialize karo (AppSettingsStore.
        // ToBackupJson) — password fields [JsonIgnore] hain, is liye backup
        // mein password kabhi nahi aata.
        var entry = zip.CreateEntry("settings.json");
        using (var writer = new StreamWriter(entry.Open()))
        {
            writer.Write(AppSettingsStore.ToBackupJson(services.Settings));
        }
    }

    /// <summary>Data → Restore: backup zip chuno, validate karo, phir overwrite. Kamyab restore par profiles foran live reload hote hain (<see cref="AppServices.ReloadProfilesAsync"/> — restart nahi); restore ki hui settings disk par hain aur agle start par laagu hoti hain (settings ka koi live-reload API nahi).</summary>
    [RelayCommand]
    private async Task Restore()
    {
        if (_restoreBusy) return;
        var dlg = new OpenFileDialog { Filter = "RSH backup (*.zip)|*.zip" };
        if (dlg.ShowDialog() != true) return;
        _restoreBusy = true;
        DataStatusText = "Restoring…";
        try
        {
            string? error = await Task.Run(() => RestoreBackup(dlg.FileName));
            if (error is null)
            {
                // Phase F live reload (W3 API): disk par restore ki hui
                // profiles.json dobara parho + engine ko nayi active
                // profile do — restart ke baghair. Reload kabhi throw
                // nahi karta (cancellation ke ilawa), lekin failure par
                // status mein saaf likho — khamoshi se "restored" nahi.
                var (ok, active, reloadError) = await services.ReloadProfilesAsync();
                LoadPins();
                if (ok)
                {
                    ViewModelStore.RaiseProfileChanged();
                    DataStatusText = _lastRestoreSnapshotDir is null
                        ? $"Profiles restored and reloaded — active profile is now '{active}'. Restored settings apply on the next start."
                        : $"Profiles restored and reloaded — active profile is now '{active}'. Restored settings apply on the next start. Previous data kept in {_lastRestoreSnapshotDir} if you need it back.";
                }
                else
                {
                    DataStatusText = $"Profiles restored on disk, but live reload failed ({reloadError}) — restart RSH to pick up the restored profiles.";
                }
            }
            else
            {
                DataStatusText = $"Restore failed: {error}";
            }
        }
        catch (Exception ex)
        {
            DataStatusText = $"Restore failed: {ex.Message}";
        }
        finally
        {
            _restoreBusy = false;
        }
    }

    /// <summary>
    /// Backup zip validate + overwrite (background thread). Null = kamyaab, warna wajah.
    /// SEC-E-07 + W8: (1) zip-bomb caps + allowlist extraction
    /// (<see cref="BackupZipGuard"/> — entry count, per-file/total caps,
    /// sirf 3 naam) — cap se zyada ho to reject, khaali/aadha restore
    /// nahi; (2) sakht validation — profiles:
    /// <see cref="ProfileStore.TryValidateFile"/> (Sanitized + 64-count
    /// cap + dedupe + koi skipped entry nahi), settings: size cap +
    /// field checks, pin-map: parse + range/count + ValidateAll;
    /// (3) overwrite se PEHLE maujooda data ka snapshot — overwrite
    /// darmiyan mein fail ho to snapshot se rollback. Validation
    /// overwrite se pehle hoti hai, is liye ghalat backup DataDirectory
    /// ki files ko haath nahi lagata.
    /// </summary>
    private string? RestoreBackup(string zipPath)
    {
        string tempDir = Path.Combine(Path.GetTempPath(), "rsh-restore-" + Guid.NewGuid().ToString("N"));
        _lastRestoreSnapshotDir = null;
        try
        {
            Directory.CreateDirectory(tempDir);
            if (!BackupZipGuard.TryExtractBackupZip(zipPath, tempDir, out string? zipError))
                return zipError;
            string profilesPath = Path.Combine(tempDir, "profiles.json");
            string settingsPath = Path.Combine(tempDir, "settings.json");
            string pinPath = Path.Combine(tempDir, "pin-map.json");
            if (!File.Exists(profilesPath) || !File.Exists(settingsPath))
                return "This backup has no profiles.json or settings.json.";
            if (!ProfileStore.TryValidateFile(profilesPath, out string? profilesError)) return profilesError;
            if (!TryValidateSettings(settingsPath, out string? settingsError)) return settingsError;
            if (File.Exists(pinPath) && !TryValidatePinMap(pinPath, out string? pinError)) return pinError;

            // W8-1: overwrite se pehle snapshot — restore fail ho to wapas aaya ja sake.
            // Ek hi snapshot dir (restore-snapshot) — har restore use taaza karta hai (bounded disk).
            string snapshotDir = Path.Combine(services.DataDirectory, "restore-snapshot");
            try
            {
                Directory.CreateDirectory(snapshotDir);
                foreach (string name in new[] { "profiles.json", "settings.json", "pin-map.json" })
                {
                    string current = Path.Combine(services.DataDirectory, name);
                    if (File.Exists(current))
                        File.Copy(current, Path.Combine(snapshotDir, name), overwrite: true);
                }
                _lastRestoreSnapshotDir = snapshotDir;
            }
            catch (Exception ex)
            {
                return $"Restore failed before overwriting: current data snapshot could not be made ({ex.Message}) — nothing changed.";
            }

            try
            {
                File.Copy(profilesPath, Path.Combine(services.DataDirectory, "profiles.json"), overwrite: true);
                // SEC-E-04: purane backup ki settings.json mein plaintext
                // password ho sakta hai — yahan verbatim restore karna theek
                // hai kyun ke agla Load use LegacyWheelWifiPassword mein le kar
                // KeyStore mein migrate karta hai aur settings dobara save kar
                // ke disk se saaf karta hai (AppSettingsStore.TryLoadFile).
                File.Copy(settingsPath, Path.Combine(services.DataDirectory, "settings.json"), overwrite: true);
                if (File.Exists(pinPath)) File.Copy(pinPath, Path.Combine(services.DataDirectory, "pin-map.json"), overwrite: true);
                return null;
            }
            catch (Exception ex)
            {
                // Overwrite darmiyan mein fail ho to snapshot se wapas lao —
                // jo file snapshot mein nahi thi (pehle maujood hi nahi thi)
                // use hatao, taake aadha restore na rahe.
                try
                {
                    foreach (string name in new[] { "profiles.json", "settings.json", "pin-map.json" })
                    {
                        string snap = Path.Combine(snapshotDir, name);
                        string dest = Path.Combine(services.DataDirectory, name);
                        if (File.Exists(snap))
                            File.Copy(snap, dest, overwrite: true);
                        else if (File.Exists(dest))
                            File.Delete(dest);
                    }
                    return $"Restore failed during overwrite ({ex.Message}) — previous data was restored from the snapshot.";
                }
                catch (Exception rollbackEx)
                {
                    return $"Restore failed during overwrite ({ex.Message}) and rollback also failed ({rollbackEx.Message}) — previous data is in {snapshotDir}.";
                }
            }
        }
        finally
        {
            try { Directory.Delete(tempDir, recursive: true); } catch (IOException) { } catch (UnauthorizedAccessException) { }
        }
    }

    /// <summary>
    /// W8-2: settings.json ki sakht validation — size cap (64 KB, SEC-C-11)
    /// + JSON object + field checks (AppSettingsStore.Sanitize ke rules,
    /// lekin reject — khamoshi se default par nahi girate). Legacy
    /// plaintext WheelWifiPassword allowed hai (SEC-E-04 migration path —
    /// Load use KeyStore mein le jata hai, is liye reject nahi hota).
    /// </summary>
    private static bool TryValidateSettings(string path, out string? error)
    {
        error = null;
        try
        {
            var info = new FileInfo(path);
            if (!info.Exists || info.Length > AppSettingsStore.MaxFileBytes)
            {
                error = $"settings.json in this backup is too large (max {AppSettingsStore.MaxFileBytes / 1024} KB).";
                return false;
            }
            string json = File.ReadAllText(path); // ≤64 KB (cap upar check hui)
            using var doc = JsonDocument.Parse(json);
            if (doc.RootElement.ValueKind != JsonValueKind.Object)
            {
                error = "settings.json in this backup is not valid.";
                return false;
            }
            var settings = JsonSerializer.Deserialize<AppSettings>(json, new JsonSerializerOptions { PropertyNameCaseInsensitive = true });
            if (settings is null)
            {
                error = "settings.json in this backup is not valid.";
                return false;
            }
            if (settings.EspIp is { Length: > 0 } espIp && !EspAddress.TryValidate(espIp, out _, out string? ipError))
            {
                error = $"settings.json in this backup has an invalid wheel address: {ipError}";
                return false;
            }
            if (settings.VJoyDeviceId is < 1 or > 16)
            {
                error = $"settings.json in this backup has an invalid vJoy device id ({settings.VJoyDeviceId} — must be 1–16).";
                return false;
            }
            // DATA-AI FIX: "HighContrast" bhi valid theme hai (AppSettings.
            // Sanitize allow karta hai) — pehle restore validator sirf
            // System/Dark/Light qabool karta tha, is liye HighContrast
            // wali backup restore par reject ho jati thi.
            if (settings.Theme is not ("System" or "Dark" or "Light" or "HighContrast"))
            {
                error = $"settings.json in this backup has an unknown theme '{settings.Theme}'.";
                return false;
            }
            if (settings.LogLevel is not ("Debug" or "Information" or "Warning" or "Error"))
            {
                error = $"settings.json in this backup has an unknown log level '{settings.LogLevel}'.";
                return false;
            }
            return true;
        }
        catch (Exception ex)
        {
            error = $"settings.json in this backup is not valid ({ex.Message}).";
            return false;
        }
    }

    /// <summary>
    /// W8-2: pin-map.json ki sakht validation — size cap + parse +
    /// count cap + GPIO 0–39 range + poora ValidateAll (forbidden /
    /// duplicate / strapping pin reject).
    /// </summary>
    private static bool TryValidatePinMap(string path, out string? error)
    {
        error = null;
        var info = new FileInfo(path);
        if (!info.Exists || info.Length > BackupZipGuard.MaxSmallFileBytes)
        {
            error = $"pin-map.json in this backup is too large (max {BackupZipGuard.MaxSmallFileBytes / 1024} KB).";
            return false;
        }
        if (!PinMapStore.TryParseFile(path, out var map) || map is null)
        {
            error = "pin-map.json in this backup is not valid.";
            return false;
        }
        if (!PinMap.TryValidateEntries(map, out string? entriesError))
        {
            error = $"pin-map.json in this backup is not valid: {entriesError}";
            return false;
        }
        var (_, errorCount) = PinMap.ValidateAll(map);
        if (errorCount > 0)
        {
            error = $"pin-map.json in this backup has {errorCount} validation error(s) — restore rejected.";
            return false;
        }
        return true;
    }

    // A13 (Phase H): in-app update check — GitHub latest release se compare.
    // Koi auto-download/install nahi: naya version ho to user ko release page
    // ka link bataya jata hai. Background par chalta hai; offline ho to
    // "unknown" status — app kabhi nahi rukti, kabhi crash nahi karti.
    [RelayCommand]
    private async Task CheckForUpdates()
    {
        if (UpdateCheckBusy) return;
        UpdateCheckBusy = true;
        UpdateStatusText = "Checking for updates…";
        try
        {
            Version current = System.Reflection.Assembly.GetExecutingAssembly().GetName().Version ?? new Version(2, 0, 0);
            current = new Version(current.Major, current.Minor, current.Build >= 0 ? current.Build : 0);
            var (runtimeOk, osOk) = RuntimeCheck.CheckAll();
            if (!runtimeOk || !osOk)
            {
                UpdateStatusText = $"This PC does not meet the minimum requirements ({RuntimeCheck.DescribeCurrent()}). {RuntimeCheck.RequirementsText}";
                return;
            }
            using var http = new HttpClient { Timeout = TimeSpan.FromSeconds(15) };
            UpdateCheckResult result = await UpdateChecker.CheckForUpdateAsync(http, current);
            UpdateStatusText = result switch
            {
                { Available: true, LatestVersion: not null } =>
                    $"RSH {result.LatestVersion} is available (you have {result.CurrentVersion}). Download it from {UpdateChecker.ReleasesPageUrl} — do not install from anywhere else.",
                { Unknown: true } => $"Could not check for updates. {result.Error}",
                _ => $"You are up to date (RSH {result.CurrentVersion}).",
            };
        }
        catch (Exception ex)
        {
            UpdateStatusText = $"Could not check for updates: {ex.Message}";
        }
        finally
        {
            UpdateCheckBusy = false;
        }
    }

    /// <summary>Data → Change wheel password: dialog kholo; kamyaabi par
    /// SEC-E-04: naya password settings.json mein <b>kabhi nahi</b> likha
    /// jata — BoardSession.SetPasswordAsync khud DPAPI KeyStore mein save
    /// karta hai. Dialog ki SaveWarning (store save fail) ho to use
    /// StatusText mein dikhao — khamoshi se nigalna mana hai.
    /// O-1 (Round 2 W2): classic mode mein plain-Window variant khulti hai
    /// (ChangePasswordDialogClassic — FluentWindow dialog modal hang jaisa
    /// lag sakta tha); Win11 par FluentWindow dialog waisi hi. Dono
    /// IChangePasswordDialogResult deti hain, is liye aage ka rasta ek hai.</summary>
    [RelayCommand]
    private async Task ChangePassword()
    {
        System.Windows.Window dialog = ChangePasswordDialogPicker.Pick(App.ClassicWindowRequested) == ChangePasswordDialogKind.Classic
            ? new ChangePasswordDialogClassic(services)
            : new ChangePasswordDialog(services);
        dialog.Owner = System.Windows.Application.Current?.MainWindow;
        dialog.ShowDialog();
        var result = (IChangePasswordDialogResult)dialog;
        if (!result.Succeeded || result.NewPassword is null) return;
        if (result.SaveWarning is not null)
        {
            StatusText = $"Wheel password changed. {result.SaveWarning}";
            return;
        }
        StatusText = "Wheel password changed. Reconnect to the wheel Wi-Fi with the new password — the stored Wi-Fi password was updated too.";
        await Task.CompletedTask;
    }

    /// <summary>
    /// SEC-E-05: Data → Wipe device key — is PC par mehfooz device key +
    /// Wi-Fi password mitayo (KeyStore.Delete — W1 ka API, yahan sirf
    /// call hota hai). Confirm dialog ke baghair kabhi nahi: wipe ke
    /// baad re-pairing lazmi hai, ye dialog + status dono par saaf
    /// likha hai. Board abhi connected na ho (BoardMac maloom na ho)
    /// to wipe nahi hoti — kis board ki key mitani hai, maloom hi nahi.
    /// Note: in-memory session key restart par saaf hoti hai (BoardSession
    /// usay khud rakhta hai) — is liye wipe ke baad restart + re-pair
    /// ka rasta status mein likha hai.
    /// </summary>
    [RelayCommand]
    private void WipeDeviceKey()
    {
        byte[]? mac = services.BoardSession.BoardMac;
        if (mac is null)
        {
            DataStatusText = "No wheel connected yet — wipe needs the connected wheel's key. Connect the wheel first (HELO), then wipe.";
            return;
        }
        string boardId = KeyStore.BoardIdForMac(mac);
        var answer = System.Windows.MessageBox.Show(
            "This deletes the wheel's device key and Wi-Fi password stored on THIS PC.\n\nAfter wiping, the app cannot send signed commands to the wheel until you re-pair it (pairing window / KEY_RESET on the board). The wheel itself keeps working — only this PC forgets it.\n\nWipe the device key for this wheel?",
            "Wipe device key",
            System.Windows.MessageBoxButton.YesNo,
            System.Windows.MessageBoxImage.Warning,
            System.Windows.MessageBoxResult.No);
        if (answer != System.Windows.MessageBoxResult.Yes)
        {
            DataStatusText = "Key wipe cancelled — nothing changed.";
            return;
        }
        bool ok;
        try
        {
            ok = services.BoardSession.KeyStore.Delete(boardId);
        }
        catch (Exception ex)
        {
            DataStatusText = $"Key wipe failed: {ex.Message}";
            return;
        }
        DataStatusText = ok
            ? "Device key wiped from this PC. Restart RSH — the in-memory session key clears on restart — then re-pair the wheel before signed commands (GET_CAPS, password change, reboot) work again."
            : "Key wipe failed — the stored key could not be deleted. Try again, or delete it by hand from the keys folder.";
    }

    // AUDIT FIX (F15): version pehle hardcode "1.0.0" tha — agle bump par
    // About purana version dikhata. Ab assembly se parho.
    public string VersionText
    {
        get
        {
            var v = System.Reflection.Assembly.GetExecutingAssembly().GetName().Version;
            string ver = v is null ? "?" : $"{v.Major}.{v.Minor}.{v.Build}";
            return $"RSH {ver} · by Dreamsofts · .NET {Environment.Version}";
        }
    }

    public Action? RequestWizard { get; set; }

    partial void OnThemeChanged(string value)
    {
        services.Settings.Theme = value;
        SaveSettingsInBackground();
        // UI-12: App.ApplyTheme WPF-UI + RSH palette dono badalta hai
        // (pehle RSH palette kabhi swap nahi hoti thi — Dark chunte hi
        // sidebar/pages Light rehte thay).
        App.ApplyTheme(value);
    }

    /// <summary>
    /// Phase F i18n: language switch — restart nahi. UiStrings.ApplyLanguage
    /// PropertyChanged(null) uthata hai, tamam bound UI strings foran badal
    /// jati hain (U-P1: save background par, UI thread par koi I/O nahi).
    /// </summary>
    partial void OnLanguageChanged(string value)
    {
        services.Settings.Language = value;
        SaveSettingsInBackground();
        UiStrings.ApplyLanguage(value);
    }

    partial void OnAutostartChanged(bool value)
    {
        if (_applyingAutostart) return;
        // A5: UI sirf tab ON dikhaye jab registry write kamyab ho.
        // Failure par toggle asal state par wapas + error — khamoshi se ON nahi.
        if (!WindowsIntegration.SetAutostart(value, out string? error))
        {
            _applyingAutostart = true;
            try
            {
                Autostart = WindowsIntegration.GetAutostart();
                services.Settings.Autostart = Autostart;
            }
            finally { _applyingAutostart = false; }
            StatusText = $"Autostart {(value ? "on" : "off")} nahi ho saka: {error ?? "registry write failed."} (admin rights check karo).";
            return;
        }
        services.Settings.Autostart = value;
        SaveSettingsInBackground();
        StatusText = value ? "Autostart on — RSH Windows start par chalega." : "Autostart off.";
    }

    /// <summary>
    /// U-P1: sync property-changed handler se async save — file I/O
    /// background par, UI thread kabhi block nahi hota. Failure
    /// StatusText mein (khamoshi se nahi).
    /// </summary>
    private async void SaveSettingsInBackground()
    {
        try { await services.SaveSettingsAsync(); }
        catch (Exception ex) { StatusText = $"Save failed: {ex.Message}"; }
    }

    partial void OnStartMinimizedChanged(bool value)
    {
        services.Settings.StartMinimized = value;
        SaveSettingsInBackground();
    }

    partial void OnCloseToTrayChanged(bool value)
    {
        services.Settings.CloseToTray = value;
        SaveSettingsInBackground();
    }

    partial void OnAutoSwitchProfilesChanged(bool value)
    {
        services.Settings.AutoSwitchProfiles = value;
        SaveSettingsInBackground();
    }

    partial void OnLogLevelChanged(string value)
    {
        services.Settings.LogLevel = value;
        SaveSettingsInBackground();
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

/// <summary>Settings → Modules ki ek row: module naam + on/off toggle (sirf yahin).</summary>
public sealed partial class ModuleToggleRow : ObservableObject
{
    public ModuleKind Module { get; }
    public string Name { get; }
    public string Subtitle { get; }
    private readonly Action<ModuleToggleRow> _onChanged;

    [ObservableProperty] private bool _enabled;

    public ModuleToggleRow(ModuleKind module, string name, string subtitle, bool enabled, Action<ModuleToggleRow> onChanged)
    {
        Module = module;
        Name = name;
        Subtitle = subtitle;
        _enabled = enabled;
        _onChanged = onChanged;
    }

    partial void OnEnabledChanged(bool value) => _onChanged(this);
}

/// <summary>Settings → GPIO pins ki ek row: function + pin dropdown + live validation text.</summary>
public sealed partial class GpioPinRow : ObservableObject
{
    public string Function { get; }
    public string KindLabel { get; }
    public IReadOnlyList<PinOption> Options { get; } = PinMap.PinOptions;
    private readonly Action<GpioPinRow> _onChanged;

    [ObservableProperty] private int? _selectedPin;
    [ObservableProperty] private string _errorText = "";
    [ObservableProperty] private string _warningText = "";

    public bool HasError => ErrorText.Length > 0;
    public bool HasWarning => WarningText.Length > 0;

    public GpioPinRow(string function, string kindLabel, int? selectedPin, Action<GpioPinRow> onChanged)
    {
        Function = function;
        KindLabel = kindLabel;
        _selectedPin = selectedPin;
        _onChanged = onChanged;
    }

    partial void OnSelectedPinChanged(int? value) => _onChanged(this);
    partial void OnErrorTextChanged(string value) => OnPropertyChanged(nameof(HasError));
    partial void OnWarningTextChanged(string value) => OnPropertyChanged(nameof(HasWarning));
}

/// <summary>Phase F i18n: Settings → Appearance → Language ka ek option — Value settings.json mein mehfooz hota hai, Display ComboBox mein dikhta hai.</summary>
public sealed record LanguageOption(string Value, string Display);
