using RSH.App.Services;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace RSH.App.ViewModels;

public sealed partial class DashboardViewModel(AppServices services) : ObservableObject, ILiveViewModel
{
    [ObservableProperty] private bool _wheelConnected;
    [ObservableProperty] private string _wheelStatusText = "Wheel not connected";
    [ObservableProperty] private string _packetRateText = "0 pkt/s";
    [ObservableProperty] private string _packetAgeText = "—";
    [ObservableProperty] private bool _outputConnected;
    [ObservableProperty] private string _outputStatusText = "vJoy not connected";
    [ObservableProperty] private string _activeProfileName = "";
    [ObservableProperty] private double _steeringNorm;
    [ObservableProperty] private double _accelNorm;
    [ObservableProperty] private double _brakeNorm;
    [ObservableProperty] private bool _watchdogActive = true;

    // AUDIT FIX (F2): tray se OutputEnabled badle to Dashboard ka toggle
    // purani state dikhata rehta tha. Aakhri dekhi hui value yaad rakho —
    // Tick() farq dekhe to notification uthata hai.
    private bool _lastOutputEnabled = services.Settings.OutputEnabled;

    public bool OutputEnabled
    {
        get => services.Settings.OutputEnabled;
        set
        {
            if (services.Settings.OutputEnabled == value) return;
            services.Settings.OutputEnabled = value;
            services.SaveSettings();
            _lastOutputEnabled = value;
            OnPropertyChanged();
        }
    }

    [ObservableProperty] private double _gaugeAngle;
    [ObservableProperty] private string _steeringBigText = "0%";
    [ObservableProperty] private string _steeringSideText = "centred";
    [ObservableProperty] private string _wheelPillText = "Wheel not connected";

    public Action? RequestSetup { get; set; }

    [RelayCommand]
    private void StartSetup() => RequestSetup?.Invoke();

    public Action? RequestWizard { get; set; }

    [RelayCommand]
    private void RunWizard() => RequestWizard?.Invoke();

    public void Tick()
    {
        var engine = services.Engine;
        var state = engine.CurrentState;

        // AUDIT FIX (F2): OutputEnabled kahin aur se (tray menu) badla ho
        // to is page ke toggle ko bhi taaza karo.
        if (_lastOutputEnabled != services.Settings.OutputEnabled)
        {
            _lastOutputEnabled = services.Settings.OutputEnabled;
            OnPropertyChanged(nameof(OutputEnabled));
        }

        WheelConnected = engine.WheelConnected;
        WheelStatusText = WheelConnected ? "Wheel connected" : "Wheel not connected — check power and WiFi";
        PacketRateText = $"{services.Source.Statistics.CurrentRatePerSecond} pkt/s";
        PacketAgeText = services.Source.LastValidAgeMs is { } age ? $"{age} ms ago" : "no data yet";
        OutputConnected = engine.OutputConnected;
        OutputStatusText = engine.OutputConnected
            ? $"vJoy device {services.Settings.VJoyDeviceId} feeding"
            : engine.LastOutputError ?? "vJoy not connected";
        ActiveProfileName = services.Profiles.ActiveProfileName;
        SteeringNorm = state.SteeringNorm;
        AccelNorm = state.AccelNorm;
        BrakeNorm = state.BrakeNorm;
        WatchdogActive = engine.WatchdogActive;
        GaugeAngle = state.SteeringNorm * 80.0;
        double pct = Math.Abs(state.SteeringNorm) * 100;
        SteeringBigText = $"{pct:F0}%";
        SteeringSideText = pct < 0.5 ? "centred" : state.SteeringNorm < 0 ? "left" : "right";
        WheelPillText = WheelConnected
            ? $"Wheel connected · {services.Source.Statistics.CurrentRatePerSecond}/s"
            : "Wheel not connected";
    }
}

public sealed partial class MonitorViewModel(AppServices services) : ObservableObject, ILiveViewModel
{
    [ObservableProperty] private int _rawSteering;
    [ObservableProperty] private int _rawAccel;
    [ObservableProperty] private int _rawBrake;
    [ObservableProperty] private string _steeringModeText = "—";
    [ObservableProperty] private int _sequence;
    [ObservableProperty] private bool _hasData;
    [ObservableProperty] private bool _gearUp;
    [ObservableProperty] private bool _gearDown;
    [ObservableProperty] private bool _accelBtn;
    [ObservableProperty] private bool _brakeBtn;
    [ObservableProperty] private string _countersText = "";

    public void Tick()
    {
        if (services.Source.TryGetLatest(out var p, out _, long.MaxValue))
        {
            HasData = true;
            RawSteering = p.Steering;
            RawAccel = p.Accel;
            RawBrake = p.Brake;
            SteeringModeText = p.SteeringInMillivolts ? "milliVolts" : "raw ADC";
            Sequence = p.Sequence;
            GearUp = p.GearUp;
            GearDown = p.GearDown;
            AccelBtn = p.AccelButton;
            BrakeBtn = p.BrakeButton;
        }
        else
        {
            HasData = false;
        }

        var st = services.Source.Statistics;
        CountersText =
            $"Valid {st.ValidPackets:N0}   Lost {st.LostPackets:N0} ({st.LossPercent:F1}%)   " +
            $"CRC errors {st.BadCrc:N0}   Bad magic {st.BadMagic:N0}   Bad version {st.BadVersion:N0}   " +
            $"Too short {st.TooShort:N0}   Duplicates/late {st.DuplicatesOrLate:N0}   Rate {st.CurrentRatePerSecond}/s";
    }
}
