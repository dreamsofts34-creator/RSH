using RSH.App.Services;

namespace RSH.App.ViewModels;

/// <summary>Jo ViewModel har UI tick (30 Hz) par taaza hota hai jab us ka page samne ho.</summary>
public interface ILiveViewModel
{
    void Tick();
}

/// <summary>
/// Tamam page ViewModels ke singleton — navigation par pages dobara
/// bante hain, lekin state yahan mehfooz rehta hai. Profile badalne par
/// ProfileChanged se tuning wale VMs dobara load karte hain.
/// </summary>
public static class ViewModelStore
{
    private static AppServices S => AppServices.Current;

    public static event Action? ProfileChanged;
    public static void RaiseProfileChanged() => ProfileChanged?.Invoke();

    private static DashboardViewModel? _dashboard;
    public static DashboardViewModel Dashboard => _dashboard ??= new DashboardViewModel(S);

    private static SetupViewModel? _setup;
    public static SetupViewModel Setup => _setup ??= new SetupViewModel(S);

    private static MonitorViewModel? _monitor;
    public static MonitorViewModel Monitor => _monitor ??= new MonitorViewModel(S);

    private static CalibrationViewModel? _calibration;
    public static CalibrationViewModel Calibration => _calibration ??= new CalibrationViewModel(S);

    private static ProcessingViewModel? _processing;
    public static ProcessingViewModel Processing => _processing ??= new ProcessingViewModel(S);

    private static ButtonsViewModel? _buttons;
    public static ButtonsViewModel Buttons => _buttons ??= new ButtonsViewModel(S);

    private static ProfilesViewModel? _profiles;
    public static ProfilesViewModel Profiles => _profiles ??= new ProfilesViewModel(S);

    private static OutputViewModel? _output;
    public static OutputViewModel Output => _output ??= new OutputViewModel(S);

    private static ConnectionViewModel? _connection;
    public static ConnectionViewModel Connection => _connection ??= new ConnectionViewModel(S);

    private static DiagnosticsViewModel? _diagnostics;
    public static DiagnosticsViewModel Diagnostics => _diagnostics ??= new DiagnosticsViewModel(S);

    private static SettingsViewModel? _settings;
    public static SettingsViewModel Settings => _settings ??= new SettingsViewModel(S);
}
