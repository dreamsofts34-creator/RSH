using System.IO;
using RSH.Core.Pipeline;
using RSH.Core.Profiles;
using RSH.Core.Transport;
using RSH.Output.VJoy;
using Serilog;

namespace RSH.App.Services;

/// <summary>
/// Poori app ka composition root: tamam services ek jaga bante hain,
/// ViewModels inhe yahin se lete hain. Koi static jaadu nahi — App
/// isay banata hai aur MainWindow ko deta hai.
/// </summary>
public sealed class AppServices : IDisposable
{
    public AppServices()
    {
        DataDirectory = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData), "RSH");
        Directory.CreateDirectory(DataDirectory);
        Directory.CreateDirectory(LogDirectory);

        SettingsStore = new AppSettingsStore(Path.Combine(DataDirectory, "settings.json"));
        Settings = SettingsStore.Load();

        ProfileStore = new ProfileStore(Path.Combine(DataDirectory, "profiles.json"));
        Profiles = ProfileStore.Load();

        Source = new UdpWheelSource();
        Hello = new HelloClient(Settings.EspIp);
        Output = new VJoyOutput((uint)Settings.VJoyDeviceId);
        Engine = new WheelEngine(Source, Output, ProfileStore.GetActive(Profiles))
        {
            OutputEnabled = Settings.OutputEnabled,
        };
        Current = this;
    }

    /// <summary>Chal rahi app ka wahid instance — Views yahin se services lete hain (App banata hai).</summary>
    public static AppServices Current { get; set; } = null!;

    public string DataDirectory { get; }
    public string LogDirectory => Path.Combine(DataDirectory, "logs");

    public AppSettingsStore SettingsStore { get; }
    public AppSettings Settings { get; }

    public ProfileStore ProfileStore { get; }
    public ProfileFile Profiles { get; private set; }

    public UdpWheelSource Source { get; }
    public HelloClient Hello { get; private set; }
    public VJoyOutput Output { get; private set; }
    public WheelEngine Engine { get; private set; }

    /// <summary>vJoy device badalna = naya output + naya engine (purana saaf band karke).</summary>
    public void RestartEngineWithDevice(int deviceId)
    {
        Engine.Stop();
        Engine.Dispose();
        Output = new VJoyOutput((uint)deviceId);
        Engine = new WheelEngine(Source, Output, ActiveProfile) { OutputEnabled = Settings.OutputEnabled };
        Engine.Start();
    }

    /// <summary>ESP IP badalna = naya HELO client.</summary>
    public void RestartHello(string espIp)
    {
        Hello.Dispose();
        Hello = new HelloClient(espIp);
        Hello.Start();
    }

    public WheelProfile ActiveProfile => ProfileStore.GetActive(Profiles);

    public void StartRuntime()
    {
        Source.Start();
        Hello.Start();
        Engine.Start();
        Log.Information("RSH runtime shuru — profile {Profile}", ActiveProfile.Name);
    }

    /// <summary>Profiles ko mehfooz karo aur engine ko nayi active profile do.</summary>
    public void SaveProfilesAndApply()
    {
        ProfileStore.Save(Profiles);
        Engine.SetProfile(ActiveProfile);
    }

    /// <summary>Ek profile ki settings badal kar save + engine par laagu (debounce caller ki zimmedari).</summary>
    public void UpdateActiveProfile(WheelProfile updated)
    {
        int idx = Profiles.Profiles.FindIndex(p => p.Name == Profiles.ActiveProfileName);
        if (idx >= 0) Profiles.Profiles[idx] = updated;
        SaveProfilesAndApply();
    }

    public void SaveSettings()
    {
        SettingsStore.Save(Settings);
        Engine.OutputEnabled = Settings.OutputEnabled;
    }

    public void Dispose()
    {
        try { Engine.Stop(); } catch { /* bandish par khamoshi */ }
        Hello.Dispose();
        Source.Dispose();
        Engine.Dispose();
    }
}
