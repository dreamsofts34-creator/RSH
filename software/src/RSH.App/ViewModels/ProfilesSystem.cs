using System.Collections.ObjectModel;
using System.IO;
using System.Text.Json;
using RSH.App.Services;
using RSH.Core.Output;
using RSH.Core.Profiles;
using RSH.Output.HIDMaestro;
using RSH.Output.VJoy;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using Microsoft.Win32;

namespace RSH.App.ViewModels;

public sealed partial class ProfilesViewModel : ObservableObject
{
    private readonly AppServices services;

    public ObservableCollection<string> ProfileNames { get; } = [];
    [ObservableProperty] private string? _selectedProfile;
    [ObservableProperty] private string _newName = "";
    [ObservableProperty] private string _statusText = "";
    [ObservableProperty] private string _activeGameProcess = "";
    // Phase H: manual override lock — user ne haath se profile chuni ho
    // to auto-switch band rehta hai jab tak ResumeAutoSwitch na ho.
    [ObservableProperty] private bool _autoSwitchLocked;
    private bool _refreshing;

    // A8 / U-P4: game-process input 400 ms debounce — har keystroke par
    // save banned hai. Timer ThreadPool par chalta hai (U-P1: save = file
    // I/O, UI thread par kabhi nahi); StatusText UI dispatcher par jata hai.
    private readonly RSH.Core.Utilities.Debouncer _gameProcessDebouncer =
        new(TimeSpan.FromMilliseconds(400));

    public ProfilesViewModel(AppServices services)
    {
        this.services = services;
        Refresh();
        // UI-9 sync fix (WS1 pending): launcher se profile switch (game
        // launch / restore-on-exit) par Profiles page par naya active
        // profile nazar aaye — singleton VM hai, page dobara banta hai
        // lekin VM nahi, is liye ProfileChanged par Refresh lazmi hai.
        // OnSelectedProfileChanged khud RaiseProfileChanged karta hai —
        // Refresh mein SelectedProfile wahi rehta hai, is liye loop nahi.
        ViewModelStore.ProfileChanged += Refresh;
    }

    partial void OnSelectedProfileChanged(string? value)
    {
        if (value is null || value == services.Profiles.ActiveProfileName) return;
        // Phase H: haath se profile chunna = manual override — auto-switch
        // band rehta hai jab tak user ResumeAutoSwitch na kare (auto-switch
        // khud ActiveProfileName badalta hai, lekin woh is handler se nahi
        // guzarta: woh value == ActiveProfileName ki wajah se early return karta hai).
        services.AutoSwitcher.ManualOverride = true;
        AutoSwitchLocked = true;
        services.Profiles.ActiveProfileName = value;
        SaveProfilesInBackground();
        ViewModelStore.RaiseProfileChanged();
        StatusText = $"'{value}' is now the active profile.";
        // SEC-C-18 (D21): profile apply ka rasta — boardMac check yahin
        // lagta hai: doosre board ki profile active ho to warning +
        // dobara calibration ka rasta, khamoshi se ghalat profile nahi.
        if (ActiveProfileBoardWarning() is { } warning)
            StatusText = $"'{value}' is now the active profile. {warning}";
    }

    /// <summary>
    /// SEC-C-18 (D21): active profile is board ki hai? Board abhi
    /// maloom na ho (HELLO nahi aaya) ya profile unbound ho to null —
    /// warning sirf tab jab dono maloom hon aur na milein.
    /// </summary>
    private string? ActiveProfileBoardWarning()
    {
        var active = services.Profiles.Profiles.FirstOrDefault(
            p => string.Equals(p.Name, services.Profiles.ActiveProfileName, StringComparison.OrdinalIgnoreCase));
        return WheelProfile.BoardBindingWarning(active, services.BoardSession.BoardMac);
    }

    /// <summary>Is board ka MAC (canonical string) — nayi profile bind karne ke liye; board maloom na ho to null.</summary>
    private string? CurrentBoardMac() =>
        services.BoardSession.BoardMac is { } mac
            ? WheelProfile.NormalizeBoardMac(Convert.ToHexString(mac))
            : null;

    /// <summary>
    /// U-P1: sync property-changed handler se async save — file I/O
    /// background par, UI thread kabhi block nahi hota. Failure
    /// StatusText mein (khamoshi se nahi).
    /// </summary>
    private async void SaveProfilesInBackground()
    {
        try { await services.SaveProfilesAndApplyAsync(); }
        catch (Exception ex) { SetStatusText($"Save failed: {ex.Message}"); }
    }

    /// <summary>Auto-switch ka ek rule: game process samne aaye to ye profile active ho (A2: game band hone par purani profile wapas aati hai). Phase H: CarModel khaali ho to kisi bhi car par, warna sirf is car par.</summary>
    public sealed record AutoSwitchRuleRow(string GameProcess, string ProfileName, string CarModel)
    {
        /// <summary>UI label — car na ho to khaali (kisi bhi car par), warna " · car: X".</summary>
        public string CarLabel => CarModel.Length > 0 ? $"  · car: {CarModel}" : "";
    }

    /// <summary>Sab profiles ke auto-switch rules (game process → profile) — sirf jin mein game naam ya car model hai.</summary>
    public ObservableCollection<AutoSwitchRuleRow> AutoSwitchRules { get; } = [];

    private void Refresh()
    {
        _refreshing = true;
        ProfileNames.Clear();
        foreach (var p in services.Profiles.Profiles) ProfileNames.Add(p.Name);
        SelectedProfile = services.Profiles.ActiveProfileName;
        var active = services.Profiles.Profiles.FirstOrDefault(p => p.Name == services.Profiles.ActiveProfileName);
        ActiveGameProcess = active?.GameProcessName ?? "";
        AutoSwitchLocked = services.AutoSwitcher.ManualOverride;
        AutoSwitchRules.Clear();
        foreach (var p in services.Profiles.Profiles)
            if (p.GameProcessName.Length > 0 || p.CarModel.Length > 0)
                AutoSwitchRules.Add(new AutoSwitchRuleRow(p.GameProcessName, p.Name, p.CarModel));
        _refreshing = false;
    }

    /// <summary>Phase H: manual override khatam karo — auto-switch dobara chalu (car badalne par profile khud badlegi).</summary>
    [RelayCommand]
    private void ResumeAutoSwitch()
    {
        services.AutoSwitcher.ManualOverride = false;
        AutoSwitchLocked = false;
        StatusText = "Auto-switch resumed — RSH will switch profiles when the car changes on track.";
    }

    /// <summary>
    /// Phase H: active profile ki car reader se live bharo ("current car"
    /// button — user ko carModel string yaad nahi karni parta). Game chal
    /// raha ho aur car nazar aa rahi ho to wohi bhar jata hai; warna saaf wajah.
    /// </summary>
    [RelayCommand]
    private async Task FillCarFromTelemetry()
    {
        string? car = services.TelemetryWatcher.LatestRawSnapshot?.CarModel;
        if (string.IsNullOrEmpty(car))
        {
            StatusText = "No car detected yet — start a session on track first (Assetto Corsa / ACC / iRacing).";
            return;
        }
        int i = services.Profiles.Profiles.FindIndex(p => p.Name == services.Profiles.ActiveProfileName);
        if (i < 0) return;
        var updated = (services.Profiles.Profiles[i] with { CarModel = car }).Sanitized();
        services.Profiles.Profiles[i] = updated;
        await services.SaveProfilesAndApplyAsync();
        Refresh();
        StatusText = $"Car for '{updated.Name}' set to '{updated.CarModel}' (from live telemetry).";
    }

    /// <summary>Navigation par dobara taaza karo — launcher / doosri jagah se profile badla ho to ye page purana na dikhaye.</summary>
    public void RefreshOnNavigate()
    {
        Refresh();
        // SEC-C-18: profile load ka rasta — page khulte hi board binding
        // check (mismatch par warning + dobara calibration ka rasta).
        if (ActiveProfileBoardWarning() is { } warning)
            StatusText = warning;
    }

    /// <summary>Selected (= active) profile ka game process naam — auto-switch isi se milata hai.</summary>
    partial void OnActiveGameProcessChanged(string value)
    {
        if (_refreshing) return;
        // A8: foran save nahi — 400 ms tak mazeed keystroke na aaye to ek save.
        // Taaza value capture hoti hai; purani pending save replace ho jati hai.
        string captured = value;
        _gameProcessDebouncer.Debounce(() => ApplyGameProcess(captured));
    }

    /// <summary>Debounce ke baad asal save (background thread — file I/O + engine SetProfile lock-protected hain).</summary>
    private void ApplyGameProcess(string value)
    {
        int i = services.Profiles.Profiles.FindIndex(p => p.Name == services.Profiles.ActiveProfileName);
        if (i < 0) return;
        var updated = (services.Profiles.Profiles[i] with { GameProcessName = value.Trim() }).Sanitized();
        if (updated.GameProcessName == services.Profiles.Profiles[i].GameProcessName) return;
        services.Profiles.Profiles[i] = updated;
        services.SaveProfilesAndApply();
        string status = updated.GameProcessName.Length > 0
            ? $"Auto-switch: '{updated.Name}' will load when '{updated.GameProcessName}' is in front."
            : $"Auto-switch cleared for '{updated.Name}'.";
        SetStatusText(status);
    }

    private void SetStatusText(string text)
    {
        // Debounced save background thread par chalta hai — WPF property
        // sirf UI thread se badlo; test/host mein dispatcher na ho to seedha.
        var dispatcher = System.Windows.Application.Current?.Dispatcher;
        if (dispatcher is not null && !dispatcher.CheckAccess())
            dispatcher.Invoke(() => StatusText = text);
        else
            StatusText = text;
    }

    [RelayCommand] private async Task AddPresetGt() => await AddPreset("GT 540", 0.6);
    [RelayCommand] private async Task AddPresetF1() => await AddPreset("F1 360", 0.4);

    private async Task AddPreset(string baseName, double scale)
    {
        var active = services.Profiles.Profiles.First(p => p.Name == services.Profiles.ActiveProfileName);
        string name = UniqueName(baseName);
        services.Profiles.Profiles.Add(active with { Name = name, SteeringScale = scale });
        await services.SaveProfilesAndApplyAsync();
        Refresh();
        StatusText = $"Preset '{name}' created from '{active.Name}' — calibration copied, rotation {(int)(scale * 900)}°. Select it to drive with it.";
    }

    private string UniqueName(string baseName)
    {
        string name = baseName.Trim();
        if (name.Length == 0) name = "Profile";
        string candidate = name;
        int n = 2;
        while (ProfileNames.Contains(candidate)) candidate = $"{name} {n++}";
        return candidate;
    }

    [RelayCommand]
    private async Task Create()
    {
        // SEC-C-15: naya naam allowlist par sakht check — na-jaiz naam
        // (separators / control chars / 64+ chars) reject, khamoshi se
        // saaf nahi (Sanitized Load path par saaf karta hai; UI par
        // user ko wajah milti hai).
        if (NewName.Length > 0 && !WheelProfile.IsValidProfileName(NewName))
        {
            StatusText = "Profile name can only use A–Z, a–z, 0–9, space, _ and - (max 64 characters).";
            return;
        }
        string name = UniqueName(NewName.Length > 0 ? NewName : "New profile");
        // SEC-C-18 (D21): nayi profile is board se bind hoti hai (board
        // maloom ho to) — doosre board par khole to warning milegi.
        var created = WheelProfile.CreateDefault(name);
        if (CurrentBoardMac() is { } mac)
            created = created with { BoardMac = mac };
        services.Profiles.Profiles.Add(created);
        services.Profiles.ActiveProfileName = name;
        await services.SaveProfilesAndApplyAsync();
        NewName = "";
        Refresh();
        ViewModelStore.RaiseProfileChanged();
        StatusText = $"Profile '{name}' created and active.";
    }

    /// <summary>Selected profile ko foran active karo (list mein select karna bhi activate karta hai — ye button wazeh action hai).</summary>
    [RelayCommand]
    private async Task Activate()
    {
        if (SelectedProfile is null) return;
        // Phase H: button se activate karna bhi manual override hai.
        services.AutoSwitcher.ManualOverride = true;
        AutoSwitchLocked = true;
        services.Profiles.ActiveProfileName = SelectedProfile;
        await services.SaveProfilesAndApplyAsync();
        Refresh();
        ViewModelStore.RaiseProfileChanged();
        StatusText = $"'{SelectedProfile}' is now the active profile.";
        // SEC-C-18: apply ka rasta — mismatch par warning bhi.
        if (ActiveProfileBoardWarning() is { } warning)
            StatusText = $"'{SelectedProfile}' is now the active profile. {warning}";
    }

    [RelayCommand]
    private async Task Duplicate()
    {
        if (SelectedProfile is null) return;
        var src = services.Profiles.Profiles.First(p => p.Name == SelectedProfile);
        string name = UniqueName(src.Name + " copy");
        services.Profiles.Profiles.Add(src with { Name = name });
        await services.SaveProfilesAndApplyAsync();
        Refresh();
        StatusText = $"Duplicated as '{name}'.";
    }

    [RelayCommand]
    private async Task Delete()
    {
        if (SelectedProfile is null) return;
        if (services.Profiles.Profiles.Count <= 1)
        {
            StatusText = "At least one profile must stay.";
            return;
        }
        string name = SelectedProfile;
        services.Profiles.Profiles.RemoveAll(p => p.Name == name);
        if (services.Profiles.ActiveProfileName == name)
            services.Profiles.ActiveProfileName = services.Profiles.Profiles[0].Name;
        await services.SaveProfilesAndApplyAsync();
        Refresh();
        ViewModelStore.RaiseProfileChanged();
        StatusText = $"Profile '{name}' deleted.";
    }

    [RelayCommand]
    private async Task Export()
    {
        if (SelectedProfile is null) return;
        var dlg = new SaveFileDialog
        {
            Filter = "RSH profile (*.json)|*.json",
            FileName = SelectedProfile + ".json",
        };
        if (dlg.ShowDialog() != true) return;
        // AUDIT FIX (F3): Export pehle baghair try/catch ke tha (Import ke
        // bar-aks) — unwritable jagah (read-only folder, USB nikal jana)
        // par exception command se bahir ja kar app crash kar sakta tha.
        // A9 / U-P1: file write background par (WriteAllTextAsync) — UI
        // thread kabhi block nahi hota.
        try
        {
            var profile = services.Profiles.Profiles.First(p => p.Name == SelectedProfile);
            // SEC-C-19: export maujooda schema version likhta hai, taake
            // Import version check kar sake (v1 file mein ye field nahi thi).
            string json = JsonSerializer.Serialize(
                profile with { SchemaVersion = ProfileFile.CurrentSchemaVersion },
                new JsonSerializerOptions { WriteIndented = true });
            await File.WriteAllTextAsync(dlg.FileName, json);
            StatusText = $"Exported to {dlg.FileName}";
        }
        catch (Exception ex)
        {
            StatusText = $"Export failed: {ex.Message}";
        }
    }

    [RelayCommand]
    private async Task Import()
    {
        var dlg = new OpenFileDialog { Filter = "RSH profile (*.json)|*.json" };
        if (dlg.ShowDialog() != true) return;
        try
        {
            // SEC-C-19 / U-P1: size cap + async capped read — cap se bari
            // file parhi hi nahi jati (OOM/hang nahi), aur parhna bhi
            // background par hota hai (ReadExactlyAsync, UI thread free).
            // TOCTOU bhi band hai: buffer utna hi banta hai jitni length
            // check hui; file beech mein barh jaye to zyada bytes parhe
            // hi nahi jate, chhoti ho jaye to ReadExactlyAsync fail karta hai.
            byte[] bytes;
            await using (var stream = new FileStream(dlg.FileName, FileMode.Open, FileAccess.Read, FileShare.Read,
                bufferSize: 4096, useAsync: true))
            {
                if (stream.Length > ProfileStore.MaxImportBytes)
                {
                    StatusText = $"Import failed: file is too large (max {ProfileStore.MaxImportBytes / 1024} KB).";
                    return;
                }
                bytes = new byte[stream.Length];
                await stream.ReadExactlyAsync(bytes);
            }
            // SEC-C-19: version check + Sanitized() — TryParse kabhi
            // exception nahi phenkta, ghalat file par wajah deta hai.
            if (!ProfileStore.TryParseImportedProfile(bytes, out var profile, out string? error) || profile is null)
            {
                StatusText = $"Import failed: {error}";
                return;
            }
            string name = UniqueName(profile.Name);
            services.Profiles.Profiles.Add(profile with { Name = name });
            await services.SaveProfilesAndApplyAsync();
            Refresh();
            StatusText = $"Imported as '{name}'.";
            // SEC-C-18: imported profile kisi aur board ki ho to warning
            // (profile khud reject nahi hoti — user ko wajah milti hai).
            if (WheelProfile.BoardBindingWarning(profile, services.BoardSession.BoardMac) is { } warning)
                StatusText = $"Imported as '{name}'. {warning}";
        }
        catch (Exception ex)
        {
            StatusText = $"Import failed: {ex.Message}";
        }
    }
}

public sealed partial class OutputViewModel(AppServices services) : ObservableObject, ILiveViewModel
{
    [ObservableProperty] private int _deviceId = services.Settings.VJoyDeviceId;
    [ObservableProperty] private bool _connected;
    [ObservableProperty] private string _statusText = "";
    [ObservableProperty] private string _detailsText = "";

    // AUDIT FIX (F6): pichli connected state — disconnected message sirf
    // state badalne par likha jata hai (neeche Tick dekho).
    private bool? _prevConnected;

    [RelayCommand]
    private async Task ApplyDevice()
    {
        DeviceId = Math.Clamp(DeviceId, 1, 16);
        services.Settings.VJoyDeviceId = DeviceId;
        await services.SaveSettingsAsync();
        services.RestartEngineWithDevice(DeviceId);
        StatusText = $"Now using vJoy device {DeviceId}.";
    }

    [RelayCommand]
    private void OpenGameControllers()
    {
        try { System.Diagnostics.Process.Start(new System.Diagnostics.ProcessStartInfo("joy.cpl") { UseShellExecute = true }); }
        catch (Exception ex) { StatusText = ex.Message; }
    }

    [RelayCommand]
    private void OpenVJoyConf()
    {
        string path = @"C:\Program Files\vJoy\x64\vJoyConf.exe";
        try
        {
            if (File.Exists(path))
                System.Diagnostics.Process.Start(new System.Diagnostics.ProcessStartInfo(path) { UseShellExecute = true });
            else
                StatusText = "vJoyConf not found — vJoy may not be installed.";
        }
        catch (Exception ex) { StatusText = ex.Message; }
    }

    public void Tick()
    {
        Connected = services.Engine.OutputConnected;
        DetailsText = Connected ? OutputDetails(services.Output) : "";
        // AUDIT FIX (F6): pehle disconnected halat mein har tick (33 ms)
        // StatusText overwrite hota tha — ApplyDevice ki confirmation
        // ("Now using vJoy device N.") ek tick mein mit jati thi. Ab
        // status sirf tab badalta hai jab connected state badle.
        if (_prevConnected != Connected)
        {
            StatusText = Connected
                ? "Connected."
                : services.Engine.LastOutputError ?? "Not connected — the engine retries every 2 seconds.";
            _prevConnected = Connected;
        }
    }

    /// <summary>
    /// Connected output ki tafseel — backend ke mutabiq (vJoy ya HIDMaestro
    /// experimental). WizardWindow mein bhi aisi hi tafseel hai (alag jagah,
    /// wahi format).
    /// </summary>
    internal static string OutputDetails(IJoystickOutput output) => output switch
    {
        VJoyOutput vjoy =>
            $"DLL v{vjoy.DllVersionText} · Driver v{vjoy.DriverVersionText} · Axes X:{(vjoy.HasAxisX ? "yes" : "NO")} Y:{(vjoy.HasAxisY ? "yes" : "NO")} Z:{(vjoy.HasAxisZ ? "yes" : "NO")} Slider:{(vjoy.HasAxisSlider ? "yes" : "NO")} · Buttons: {vjoy.ButtonCount}",
        HidMaestroOutput hm =>
            $"HIDMaestro v{hm.SdkVersion} (experimental) · Driver v{hm.DriverVersionText} · Profile {hm.ProfileId} · Axes X:{(hm.HasAxisX ? "yes" : "NO")} Y:{(hm.HasAxisY ? "yes" : "NO")} Z:{(hm.HasAxisZ ? "yes" : "NO")} Slider:{(hm.HasAxisSlider ? "yes" : "NO")} · Buttons: {hm.ButtonCount}",
        var other => $"{other.Name} · connected",
    };
}

public sealed partial class ConnectionViewModel(AppServices services) : ObservableObject, ILiveViewModel
{
    [ObservableProperty] private string _espIp = services.Settings.EspIp;
    [ObservableProperty] private string _statusText = "";
    [ObservableProperty] private string _helloText = "";
    [ObservableProperty] private string _wheelText = "";
    [ObservableProperty] private string _wifiPassword = services.GetWheelPassword();
    [ObservableProperty] private string _wifiStatus = "";

    partial void OnWifiPasswordChanged(string value)
    {
        // SEC-E-04: ye sirf in-memory buffer hai — SaveSettingsAsync bhi
        // ise disk par kabhi nahi likhta (WheelWifiPassword [JsonIgnore]
        // hai). KeyStore mein tab jata hai jab Wi-Fi join kamyab ho
        // (ConnectWifi) ya HELLO aaye (AppServices.TryMigrateWifiPassword).
        services.Settings.WheelWifiPassword = value;
        SaveSettingsInBackground();
    }

    /// <summary>U-P1: sync property-changed handler se async save — file I/O background par, UI thread kabhi block nahi hota.</summary>
    private async void SaveSettingsInBackground()
    {
        try { await services.SaveSettingsAsync(); }
        catch (Exception ex) { StatusText = $"Save failed: {ex.Message}"; }
    }

    /// <summary>Auto-setup ka asal kaam: PC ko RSH Wi-Fi se khud jor do.</summary>
    [RelayCommand]
    private async Task ConnectWifi()
    {
        // SEC-C-04: password khaali ho (abhi pair nahi hua) to netsh ko
        // khaali password mat do — pehle password darj karne ko kaho.
        // WifiPassword (ye ViewModel ki property) parho — Settings wala
        // field KeyStore se seed hui value ko nahi rakhta.
        string password = WifiPassword;
        if (string.IsNullOrWhiteSpace(password))
        {
            WifiStatus = "Wheel password set nahi hai — board ka per-device password darj karo (pairing / first boot par mila tha).";
            return;
        }
        WifiStatus = "Working…";
        var r = await Task.Run(() => WindowsIntegration.ConnectToWheelWifi(password));
        if (r.Ok)
        {
            // SEC-E-04: join kamyab hua — password sahi hai, is liye ise
            // DPAPI KeyStore mein mehfooz karo (board jana hua ho to).
            // Fail ho to khamoshi se rehne do — agle HELLO par migration
            // dobara koshish karegi; in-memory value tab tak kaam karegi.
            try { services.BoardSession.TrySaveWifiPassword(password); } catch (Exception) { /* best-effort */ }
        }
        WifiStatus = r.Ok ? "Connecting… packets should start within seconds." : r.Message;
    }

    [RelayCommand]
    private async Task ApplyIp()
    {
        // AUDIT FIX (F5): pehle koi validation nahi thi — ghalat text
        // (khaali, letters) seedha RestartHello ko jata tha aur exception
        // par command crash kar sakta tha. Ab pehle TryParse, warna
        // wazeh wajah StatusText mein.
        string ip = EspIp.Trim();
        if (!System.Net.IPAddress.TryParse(ip, out _))
        {
            StatusText = $"'{ip}' is not a valid IP address — nothing changed.";
            return;
        }
        try
        {
            services.Settings.EspIp = ip;
            await services.SaveSettingsAsync();
            services.RestartHello(ip);
            StatusText = $"HELO now goes to {ip}:4210.";
        }
        catch (Exception ex) { StatusText = ex.Message; }
    }

    [RelayCommand]
    private async Task RestartFirmware()
    {
        try
        {
            // SEC-D-01: restart sirf HMAC-signed REBOOT (0x09) se — RSTX hata diya gaya hai.
            var (sent, error) = await services.RequestBoardRebootAsync();
            StatusText = sent
                ? "Restart command sent to the wheel (signed REBOOT). It will be back in a few seconds."
                : $"Restart nahi ho saka: {error}";
        }
        catch (Exception ex) { StatusText = ex.Message; }
    }

    public void Tick()
    {
        HelloText = services.Hello.IsRunning
            ? $"HELO running — {services.Hello.HellosSent} sent to {services.Settings.EspIp}:4210"
            : "HELO stopped";
        var age = services.Source.LastValidAgeMs;
        WheelText = age is null
            ? "No packets yet. Is the wheel on and this PC connected to the RSH WiFi?"
            : age < 2000
                ? $"Receiving ({services.Source.SourceDescription}) — last packet {age} ms ago"
                : $"Silent for {age / 1000.0:F1} s — the wheel stops sending 2 s after the last HELO it hears.";
    }
}
