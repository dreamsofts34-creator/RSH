using System.Collections.ObjectModel;
using System.IO;
using System.Text.Json;
using ApexLink.App.Services;
using ApexLink.Core.Profiles;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using Microsoft.Win32;

namespace ApexLink.App.ViewModels;

public sealed partial class ProfilesViewModel : ObservableObject
{
    private readonly AppServices services;

    public ObservableCollection<string> ProfileNames { get; } = [];
    [ObservableProperty] private string? _selectedProfile;
    [ObservableProperty] private string _newName = "";
    [ObservableProperty] private string _statusText = "";
    [ObservableProperty] private string _activeGameProcess = "";
    private bool _refreshing;

    public ProfilesViewModel(AppServices services)
    {
        this.services = services;
        Refresh();
    }

    partial void OnSelectedProfileChanged(string? value)
    {
        if (value is null || value == services.Profiles.ActiveProfileName) return;
        services.Profiles.ActiveProfileName = value;
        services.SaveProfilesAndApply();
        ViewModelStore.RaiseProfileChanged();
        StatusText = $"'{value}' is now the active profile.";
    }

    private void Refresh()
    {
        _refreshing = true;
        ProfileNames.Clear();
        foreach (var p in services.Profiles.Profiles) ProfileNames.Add(p.Name);
        SelectedProfile = services.Profiles.ActiveProfileName;
        var active = services.Profiles.Profiles.FirstOrDefault(p => p.Name == services.Profiles.ActiveProfileName);
        ActiveGameProcess = active?.GameProcessName ?? "";
        _refreshing = false;
    }

    /// <summary>Selected (= active) profile ka game process naam — auto-switch isi se milata hai.</summary>
    partial void OnActiveGameProcessChanged(string value)
    {
        if (_refreshing) return;
        int i = services.Profiles.Profiles.FindIndex(p => p.Name == services.Profiles.ActiveProfileName);
        if (i < 0) return;
        var updated = (services.Profiles.Profiles[i] with { GameProcessName = value.Trim() }).Sanitized();
        if (updated.GameProcessName == services.Profiles.Profiles[i].GameProcessName) return;
        services.Profiles.Profiles[i] = updated;
        services.SaveProfilesAndApply();
        StatusText = updated.GameProcessName.Length > 0
            ? $"Auto-switch: '{updated.Name}' will load when '{updated.GameProcessName}' is in front."
            : $"Auto-switch cleared for '{updated.Name}'.";
    }

    [RelayCommand] private void AddPresetGt() => AddPreset("GT 540", 0.6);
    [RelayCommand] private void AddPresetF1() => AddPreset("F1 360", 0.4);

    private void AddPreset(string baseName, double scale)
    {
        var active = services.Profiles.Profiles.First(p => p.Name == services.Profiles.ActiveProfileName);
        string name = UniqueName(baseName);
        services.Profiles.Profiles.Add(active with { Name = name, SteeringScale = scale });
        services.SaveProfilesAndApply();
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
    private void Create()
    {
        string name = UniqueName(NewName.Length > 0 ? NewName : "New profile");
        services.Profiles.Profiles.Add(WheelProfile.CreateDefault(name));
        services.Profiles.ActiveProfileName = name;
        services.SaveProfilesAndApply();
        NewName = "";
        Refresh();
        ViewModelStore.RaiseProfileChanged();
        StatusText = $"Profile '{name}' created and active.";
    }

    [RelayCommand]
    private void Duplicate()
    {
        if (SelectedProfile is null) return;
        var src = services.Profiles.Profiles.First(p => p.Name == SelectedProfile);
        string name = UniqueName(src.Name + " copy");
        services.Profiles.Profiles.Add(src with { Name = name });
        services.SaveProfilesAndApply();
        Refresh();
        StatusText = $"Duplicated as '{name}'.";
    }

    [RelayCommand]
    private void Delete()
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
        services.SaveProfilesAndApply();
        Refresh();
        ViewModelStore.RaiseProfileChanged();
        StatusText = $"Profile '{name}' deleted.";
    }

    [RelayCommand]
    private void Export()
    {
        if (SelectedProfile is null) return;
        var dlg = new SaveFileDialog
        {
            Filter = "ApexLink profile (*.json)|*.json",
            FileName = SelectedProfile + ".json",
        };
        if (dlg.ShowDialog() != true) return;
        // AUDIT FIX (F3): Export pehle baghair try/catch ke tha (Import ke
        // bar-aks) — unwritable jagah (read-only folder, USB nikal jana)
        // par exception command se bahir ja kar app crash kar sakta tha.
        try
        {
            var profile = services.Profiles.Profiles.First(p => p.Name == SelectedProfile);
            File.WriteAllText(dlg.FileName, JsonSerializer.Serialize(profile, new JsonSerializerOptions { WriteIndented = true }));
            StatusText = $"Exported to {dlg.FileName}";
        }
        catch (Exception ex)
        {
            StatusText = $"Export failed: {ex.Message}";
        }
    }

    [RelayCommand]
    private void Import()
    {
        var dlg = new OpenFileDialog { Filter = "ApexLink profile (*.json)|*.json" };
        if (dlg.ShowDialog() != true) return;
        try
        {
            var profile = JsonSerializer.Deserialize<WheelProfile>(File.ReadAllText(dlg.FileName),
                new JsonSerializerOptions { PropertyNameCaseInsensitive = true });
            if (profile is null) throw new JsonException("empty");
            string name = UniqueName(profile.Name);
            services.Profiles.Profiles.Add((profile with { Name = name }).Sanitized());
            services.SaveProfilesAndApply();
            Refresh();
            StatusText = $"Imported as '{name}'.";
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
    private void ApplyDevice()
    {
        DeviceId = Math.Clamp(DeviceId, 1, 16);
        services.Settings.VJoyDeviceId = DeviceId;
        services.SaveSettings();
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
        var o = services.Output;
        DetailsText = Connected
            ? $"DLL v{o.DllVersionText} · Driver v{o.DriverVersionText} · Axes X:{(o.HasAxisX ? "yes" : "NO")} Y:{(o.HasAxisY ? "yes" : "NO")} Z:{(o.HasAxisZ ? "yes" : "NO")} · Buttons: {o.ButtonCount}"
            : "";
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
}

public sealed partial class ConnectionViewModel(AppServices services) : ObservableObject, ILiveViewModel
{
    [ObservableProperty] private string _espIp = services.Settings.EspIp;
    [ObservableProperty] private string _statusText = "";
    [ObservableProperty] private string _helloText = "";
    [ObservableProperty] private string _wheelText = "";
    [ObservableProperty] private string _wifiPassword = services.Settings.WheelWifiPassword;
    [ObservableProperty] private string _wifiStatus = "";

    partial void OnWifiPasswordChanged(string value)
    {
        services.Settings.WheelWifiPassword = value;
        services.SaveSettings();
    }

    /// <summary>Auto-setup ka asal kaam: PC ko APEXLINK Wi-Fi se khud jor do.</summary>
    [RelayCommand]
    private async Task ConnectWifi()
    {
        WifiStatus = "Working…";
        var r = await Task.Run(() => WindowsIntegration.ConnectToWheelWifi(services.Settings.WheelWifiPassword));
        WifiStatus = r.Ok ? "Connecting… packets should start within seconds." : r.Message;
    }

    [RelayCommand]
    private void ApplyIp()
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
            services.SaveSettings();
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
            await services.Hello.SendRestartAsync();
            StatusText = "Restart command sent to the wheel (firmware v3 feature). It will be back in a few seconds.";
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
            ? "No packets yet. Is the wheel on and this PC connected to the APEXLINK WiFi?"
            : age < 2000
                ? $"Receiving on port 4211 — last packet {age} ms ago · protocol v1 (18-byte packets)"
                : $"Silent for {age / 1000.0:F1} s — the wheel stops sending 2 s after the last HELO it hears.";
    }
}
