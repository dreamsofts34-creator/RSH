using RSH.App.Services;
using RSH.Core.Profiles;
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
            SaveSettingsInBackground();
            _lastOutputEnabled = value;
            OnPropertyChanged();
        }
    }

    /// <summary>
    /// U-P1: property setter (UI thread) se async save — file I/O
    /// background par. Failure yahan khamoshi se nahi chhupti: in-memory
    /// state aur toggle pehle hi consistent hain, aur agla save (kisi bhi
    /// settings change par) dobara koshish karta hai — Tick() har 33 ms
    /// mein settings se sync rehta hai, is liye ghalat state dikhne ka
    /// khatra nahi.
    /// </summary>
    private async void SaveSettingsInBackground()
    {
        try { await services.SaveSettingsAsync(); }
        catch { /* upar dekho — toggle/in-memory state consistent rehti hai */ }
    }

    // UI-4: game launcher — DashboardViewModel page ka owner hai, is
    // liye launcher VM yahin banta hai (ViewModelStore.cs doosre
    // workstream ki file hai — wahan kuch nahi badla). Profile switch
    // par ViewModelStore.ProfileChanged uthta hai (topbar + Profiles sync).
    public GameLauncherViewModel GameLauncher { get; } = new(services, ViewModelStore.RaiseProfileChanged);

    // UI-3: steering angle — purani 0–80° gauge sweep ki jagah
    // asal angle: lock-to-lock range = SteeringScale × 900° (D7).
    [ObservableProperty] private double _steeringDegrees;
    [ObservableProperty] private string _steeringBigText = "0°";
    [ObservableProperty] private string _steeringSideText = "centred";
    [ObservableProperty] private string _steeringRangeText = "";
    [ObservableProperty] private string _wheelPillText = "Wheel not connected";
    [ObservableProperty] private string _wheelAddressText = "";
    [ObservableProperty] private string _firmwareText = "";
    [ObservableProperty] private string _engineStatsText = "";
    [ObservableProperty] private string _restartStatusText = "";

    // NeedsRecalibration banner (SETUP-GATES-DECISION.md §3.2, verbatim):
    // CheckRecalibration se — Dashboard par banner, Setup flow mein entry
    // par prompt (SetupViewModel). Dismiss sirf tab tak jab tak wajah
    // na badle; kamyaab recalibration (MarkCalibrated) saaf karta hai.
    [ObservableProperty] private string? _recalibrationBannerText;
    public bool HasRecalibrationBanner => RecalibrationBannerText is not null;
    partial void OnRecalibrationBannerTextChanged(string? value) => OnPropertyChanged(nameof(HasRecalibrationBanner));
    private RecalibrationReason _bannerDismissedFor = RecalibrationReason.None;
    private string _bannerKey = "";

    [RelayCommand]
    private void DismissRecalibrationBanner()
    {
        _bannerDismissedFor = services.ActiveProfile
            .CheckRecalibration(CurrentCalibrationContext()).Reasons;
        RecalibrationBannerText = null;
    }

    private CalibrationContext CurrentCalibrationContext()
    {
        string? fingerprint = SetupViewModel.LastBoardCaps is not null
            ? CalibrationFreshness.PinMapFingerprint(SetupViewModel.LastBoardCaps)
            : null;
        string? firmware = SetupViewModel.LastBoardCaps?.FirmwareVersion.ToString()
            ?? services.BoardSession.HelloFirmwareVersion?.ToString();
        return new CalibrationContext(fingerprint, firmware, DateTimeOffset.UtcNow);
    }

    private void RefreshRecalibrationBanner()
    {
        // Key mein calibration stamps bhi hon — Finish (MarkCalibrated)
        // profile ka naam nahi badalta, lekin stamps badalte hain; bina
        // in ke banner recalibration ke baad bhi chipka rehta.
        var profile = services.ActiveProfile;
        string key = profile.Name + "|"
            + (profile.CalibratedAtUtc?.ToString("O") ?? "nocal") + "|"
            + (profile.CalibrationPinMapFingerprint ?? "nofp") + "|"
            + (SetupViewModel.LastBoardCaps is null ? "nocaps" : SetupViewModel.LastBoardCaps.GetHashCode().ToString(System.Globalization.CultureInfo.InvariantCulture)) + "|"
            + (services.BoardSession.HelloFirmwareVersion?.ToString() ?? "nofw");
        if (key == _bannerKey) return;
        _bannerKey = key;
        _bannerDismissedFor = RecalibrationReason.None;
        var check = profile.CheckRecalibration(CurrentCalibrationContext());
        RecalibrationBannerText = check.NeedsRecalibration && check.Reasons != _bannerDismissedFor
            ? check.PromptMessage
            : null;
    }

    // Tick jitter (engine loop ka asal dt, nominal 5 ms se inhiraf) ka
    // EMA — har Tick par ek multiply/add, koi allocation nahi (U-P7).
    private double _tickJitterEmaMs;

    public Action? RequestSetup { get; set; }

    [RelayCommand]
    private void StartSetup() => RequestSetup?.Invoke();

    public Action? RequestWizard { get; set; }

    [RelayCommand]
    private void RunWizard() => RequestWizard?.Invoke();

    private bool _restarting;

    /// <summary>
    /// UI-3: Restart wheel — sirf Dashboard par (kahin aur nahi).
    /// HMAC-signed REBOOT (SEC-D-01) — session key na ho to saaf wajah
    /// status mein, khamoshi se purana rasta kabhi nahi.
    /// </summary>
    [RelayCommand]
    private async Task RestartWheel()
    {
        if (_restarting) return;
        _restarting = true;
        RestartStatusText = "Sending restart…";
        try
        {
            var (sent, error) = await services.RequestBoardRebootAsync();
            RestartStatusText = sent
                ? "Restart sent — the wheel will be back in a few seconds."
                : $"Restart failed: {error ?? "no session with the wheel"}";
        }
        catch (Exception ex)
        {
            RestartStatusText = $"Restart failed: {ex.Message}";
        }
        finally
        {
            _restarting = false;
        }
    }

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

        // UI-3: asal steering angle — norm ±1 hi full lock hai (D7:
        // pipeline scale se divide karta hai), is liye angle =
        // norm × (range / 2), range = scale × 900°.
        double scale = Math.Clamp(services.ActiveProfile.SteeringScale, 0.2, 1.0);
        double range = scale * 900.0;
        double degrees = state.SteeringNorm * (range / 2.0);
        SteeringDegrees = degrees;
        SteeringRangeText = $"{range:F0}° lock to lock";
        SteeringBigText = $"{degrees:F0}°";
        SteeringSideText = Math.Abs(degrees) < 1.0 ? "centred" : degrees < 0 ? "left" : "right";
        WheelPillText = WheelConnected
            ? $"Connected · {services.Settings.EspIp}"
            : "Wheel not connected";
        WheelAddressText = $"{services.Settings.EspIp} · rsh.local";
        FirmwareText = services.BoardSession.HelloFirmwareVersion is { } fw
            ? $"Firmware v{fw}"
            : "Firmware —";

        // UI-3: engine stats — asal engine se (Hz, loss, tick jitter).
        // Tick jitter = engine loop dt ka nominal 5 ms se inhiraf (EMA);
        // link jitter ka Core mein koi metric nahi, is liye naam saaf hai.
        var stats = services.Source.Statistics;
        RefreshRecalibrationBanner();
        if (engine.IsRunning)
        {
            double dev = Math.Abs(engine.LastDtSeconds * 1000.0 - 5.0);
            _tickJitterEmaMs = _tickJitterEmaMs * 0.9 + dev * 0.1;
            EngineStatsText = $"{stats.CurrentRatePerSecond} pkt/s · {stats.LossPercent:F1}% lost · {_tickJitterEmaMs:F2} ms tick jitter";
        }
        else
        {
            EngineStatsText = "Engine stopped";
        }

        GameLauncher.Tick();
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
    [ObservableProperty] private string _packetStatsText = "";

    /// <summary>Diagnostics (UI-10 merge): Link / Engine / Process panels + logs isi page par hain — alag screen nahi.</summary>
    public DiagnosticsViewModel Diagnostics => ViewModelStore.Diagnostics;

    // Tick jitter (engine loop dt, nominal 5 ms se inhiraf) ka EMA —
    // link-level jitter ka Core mein koi metric nahi, is liye packet
    // stats mein sequence gaps (lost) + ye tick jitter dikhta hai.
    private double _tickJitterEmaMs;

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
        // Packet stats: rate + loss (sliding window, last 1000) + seq
        // gaps (lost = ghayeb sequences) + duplicates/late + tick jitter.
        double dev = Math.Abs(services.Engine.LastDtSeconds * 1000.0 - 5.0);
        _tickJitterEmaMs = _tickJitterEmaMs * 0.9 + dev * 0.1;
        PacketStatsText =
            $"Rate {st.CurrentRatePerSecond} pkt/s · Loss {st.LossPercent:F2}% · " +
            $"Sequence gaps {st.LostPackets:N0} · Duplicates/late {st.DuplicatesOrLate:N0} · " +
            $"Tick jitter {_tickJitterEmaMs:F2} ms";
    }
}
