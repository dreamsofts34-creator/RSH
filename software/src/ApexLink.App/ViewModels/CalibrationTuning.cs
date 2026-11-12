using ApexLink.App.Services;
using ApexLink.Core.Processing;
using ApexLink.Core.Profiles;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace ApexLink.App.ViewModels;

public sealed partial class CalibrationViewModel(AppServices services) : ObservableObject, ILiveViewModel
{
    [ObservableProperty] private int _liveSteering;
    [ObservableProperty] private int _liveAccel;
    [ObservableProperty] private int _liveBrake;
    [ObservableProperty] private bool _hasData;
    [ObservableProperty] private string _statusText = "Move the wheel and pedals — live values show here.";

    [ObservableProperty] private int _steerMin;
    [ObservableProperty] private int _steerCenter;
    [ObservableProperty] private int _steerMax;
    [ObservableProperty] private int _accelMin;
    [ObservableProperty] private int _accelMax;
    [ObservableProperty] private int _brakeMin;
    [ObservableProperty] private int _brakeMax;

    private bool _capturingSteering;
    private bool _capturingPedals;
    private bool _loaded;
    private string _loadedProfileName = "";

    private void LoadFromProfile()
    {
        _loadedProfileName = services.Profiles.ActiveProfileName;
        var p = services.ActiveProfile;
        SteerMin = p.SteeringCalibration.RawMin;
        SteerCenter = p.SteeringCalibration.RawCenter;
        SteerMax = p.SteeringCalibration.RawMax;
        AccelMin = p.AccelCalibration.RawMin;
        AccelMax = p.AccelCalibration.RawMax;
        BrakeMin = p.BrakeCalibration.RawMin;
        BrakeMax = p.BrakeCalibration.RawMax;
        _loaded = true;
    }

    [RelayCommand] private void CaptureCenter() { if (HasData) SteerCenter = LiveSteering; }

    [RelayCommand]
    private void ToggleCaptureSteering()
    {
        _capturingSteering = !_capturingSteering;
        // F12: page ke button ko capture ki halat dikhani hai — notification
        // uthao warna binding taaza nahi hoti.
        OnPropertyChanged(nameof(CapturingSteering));
        if (_capturingSteering && HasData) { SteerMin = LiveSteering; SteerMax = LiveSteering; }
        StatusText = _capturingSteering
            ? "Capturing steering range… turn fully left and right, then press Stop."
            : "Steering range captured.";
    }
    public bool CapturingSteering => _capturingSteering;

    [RelayCommand]
    private void ToggleCapturePedals()
    {
        _capturingPedals = !_capturingPedals;
        OnPropertyChanged(nameof(CapturingPedals));
        if (_capturingPedals && HasData)
        {
            AccelMin = LiveAccel; AccelMax = LiveAccel;
            BrakeMin = LiveBrake; BrakeMax = LiveBrake;
        }
        StatusText = _capturingPedals
            ? "Capturing pedals… release both, then press each fully, then press Stop."
            : "Pedal range captured.";
    }
    public bool CapturingPedals => _capturingPedals;

    /// <summary>Chalta capture roko (profile reload par istemal hota hai).</summary>
    private bool StopCaptures()
    {
        bool was = _capturingSteering || _capturingPedals;
        if (!was) return false;
        _capturingSteering = _capturingPedals = false;
        OnPropertyChanged(nameof(CapturingSteering));
        OnPropertyChanged(nameof(CapturingPedals));
        return true;
    }

    [RelayCommand]
    private void Save()
    {
        var profile = services.ActiveProfile with
        {
            SteeringCalibration = new SteeringCalibration { RawMin = SteerMin, RawCenter = SteerCenter, RawMax = SteerMax, IsSet = true },
            AccelCalibration = new PedalCalibration { RawMin = AccelMin, RawMax = AccelMax, IsSet = true },
            BrakeCalibration = new PedalCalibration { RawMin = BrakeMin, RawMax = BrakeMax, IsSet = true },
        };
        if (!profile.SteeringCalibration.IsSane)
        {
            StatusText = "Steering values don't look right (need min < center < max, span at least 100). Capture again.";
            return;
        }
        if (!profile.AccelCalibration.IsSane || !profile.BrakeCalibration.IsSane)
        {
            StatusText = "Pedal values don't look right (need max > min, span at least 100). Capture again.";
            return;
        }
        services.UpdateActiveProfile(profile);
        StatusText = "Calibration saved to the active profile.";
    }

    [RelayCommand]
    private void Reset()
    {
        var profile = services.ActiveProfile with
        {
            SteeringCalibration = new SteeringCalibration(),
            AccelCalibration = new PedalCalibration(),
            BrakeCalibration = new PedalCalibration(),
        };
        services.UpdateActiveProfile(profile);
        LoadFromProfile();
        StatusText = "Calibration cleared — the wheel will use default ranges until you calibrate.";
    }

    public void Tick()
    {
        // AUDIT FIX: profile badal jaye to is page ki qeematein bhi nayi
        // profile ki hon — pehle purani profile ke numbers chipke rehte thay.
        // AUDIT FIX (F7): reload par chalta capture bhi band karo — pehle
        // min/max nayi profile se badal jate thay lekin capture khamoshi se
        // naye base par aage barhta rehta tha, user ko pata nahi chalta tha.
        if (!_loaded || _loadedProfileName != services.Profiles.ActiveProfileName)
        {
            bool wasCapturing = _loaded && StopCaptures();
            LoadFromProfile();
            if (wasCapturing)
                StatusText = "Profile changed — the capture was stopped. Start the capture again for this profile.";
        }
        if (services.Source.TryGetLatest(out var p, out _, long.MaxValue))
        {
            HasData = true;
            LiveSteering = p.Steering;
            LiveAccel = p.Accel;
            LiveBrake = p.Brake;
            if (_capturingSteering)
            {
                SteerMin = Math.Min(SteerMin, p.Steering);
                SteerMax = Math.Max(SteerMax, p.Steering);
            }
            if (_capturingPedals)
            {
                AccelMin = Math.Min(AccelMin, p.Accel);
                AccelMax = Math.Max(AccelMax, p.Accel);
                BrakeMin = Math.Min(BrakeMin, p.Brake);
                BrakeMax = Math.Max(BrakeMax, p.Brake);
            }
        }
        else HasData = false;
    }
}

/// <summary>Slider wali settings ka mushtarka debounce pattern.</summary>
public abstract partial class TuningViewModelBase(AppServices services) : ObservableObject
{
    private System.Windows.Threading.DispatcherTimer? _saveTimer;
    protected AppServices Services => services;
    protected bool Loading { get; private set; }

    protected void LoadProfile()
    {
        Loading = true;
        LoadFrom(services.ActiveProfile);
        Loading = false;
    }

    protected abstract void LoadFrom(WheelProfile profile);
    protected abstract WheelProfile BuildProfile();

    protected void ScheduleSave()
    {
        if (Loading) return;
        _saveTimer?.Stop();
        _saveTimer = new System.Windows.Threading.DispatcherTimer { Interval = TimeSpan.FromMilliseconds(400) };
        _saveTimer.Tick += (_, _) =>
        {
            _saveTimer?.Stop();
            services.UpdateActiveProfile(BuildProfile());
        };
        _saveTimer.Start();
    }
}

public sealed partial class ProcessingViewModel : TuningViewModelBase, ILiveViewModel
{
    [ObservableProperty] private double _deadzonePercent = 2;
    [ObservableProperty] private double _curvePercent;
    [ObservableProperty] private double _smoothingPercent = 30;
    [ObservableProperty] private bool _invertSteering;
    [ObservableProperty] private bool _invertAccel;
    [ObservableProperty] private bool _invertBrake;
    [ObservableProperty] private bool _combinePedals;

    public ProcessingViewModel(AppServices services) : base(services)
    {
        LoadProfile();
        ViewModelStore.ProfileChanged += LoadProfile;
    }

    protected override void LoadFrom(WheelProfile p)
    {
        DeadzonePercent = p.Deadzone * 100;
        CurvePercent = p.CurveStrength * 100;
        SmoothingPercent = p.Smoothing * 100;
        InvertSteering = p.InvertSteering;
        InvertAccel = p.InvertAccel;
        InvertBrake = p.InvertBrake;
        CombinePedals = p.CombinePedals;
    }

    protected override WheelProfile BuildProfile() => Services.ActiveProfile with
    {
        Deadzone = DeadzonePercent / 100.0,
        CurveStrength = CurvePercent / 100.0,
        Smoothing = SmoothingPercent / 100.0,
        InvertSteering = InvertSteering,
        InvertAccel = InvertAccel,
        InvertBrake = InvertBrake,
        CombinePedals = CombinePedals,
    };

    private bool IsPreset(double curve, double smooth, double dead) =>
        Math.Abs(CurvePercent - curve) < 0.5 && Math.Abs(SmoothingPercent - smooth) < 0.5 && Math.Abs(DeadzonePercent - dead) < 0.5;
    public bool IsRelaxed => IsPreset(35, 45, 3);
    public bool IsBalanced => IsPreset(0, 30, 2);
    public bool IsSharp => IsPreset(-30, 12, 1);

    private void RaisePresets()
    {
        OnPropertyChanged(nameof(IsRelaxed));
        OnPropertyChanged(nameof(IsBalanced));
        OnPropertyChanged(nameof(IsSharp));
    }

    partial void OnDeadzonePercentChanged(double value) { RaisePresets(); ScheduleSave(); }
    partial void OnCurvePercentChanged(double value) { RaisePresets(); ScheduleSave(); }
    partial void OnSmoothingPercentChanged(double value) { RaisePresets(); ScheduleSave(); }
    partial void OnInvertSteeringChanged(bool value) => ScheduleSave();
    partial void OnInvertAccelChanged(bool value) => ScheduleSave();
    partial void OnInvertBrakeChanged(bool value) => ScheduleSave();
    partial void OnCombinePedalsChanged(bool value) => ScheduleSave();

    [RelayCommand] private void PresetRelaxed() { CurvePercent = 35; SmoothingPercent = 45; DeadzonePercent = 3; }
    [RelayCommand] private void PresetBalanced() { CurvePercent = 0; SmoothingPercent = 30; DeadzonePercent = 2; }
    [RelayCommand] private void PresetSharp() { CurvePercent = -30; SmoothingPercent = 12; DeadzonePercent = 1; }

    public void Tick() { /* preview graph page khud sambhalti hai */ }
}

public sealed partial class ButtonsViewModel : TuningViewModelBase, ILiveViewModel
{
    [ObservableProperty] private int _gearUpButton = 1;
    [ObservableProperty] private int _gearDownButton = 2;
    [ObservableProperty] private double _pressMs = 150;
    [ObservableProperty] private double _releaseMs = 100;
    [ObservableProperty] private bool _gearUpPressed;
    [ObservableProperty] private bool _gearDownPressed;
    [ObservableProperty] private bool _accelBtnPressed;
    [ObservableProperty] private bool _brakeBtnPressed;
    [ObservableProperty] private string _maskText = "0x00000000";

    public ButtonsViewModel(AppServices services) : base(services)
    {
        LoadProfile();
        ViewModelStore.ProfileChanged += LoadProfile;
    }

    protected override void LoadFrom(WheelProfile p)
    {
        GearUpButton = p.GearUpVJoyButton;
        GearDownButton = p.GearDownVJoyButton;
        PressMs = p.PedalPressMs;
        ReleaseMs = p.PedalReleaseMs;
    }

    protected override WheelProfile BuildProfile() => Services.ActiveProfile with
    {
        GearUpVJoyButton = Math.Clamp(GearUpButton, 1, 128),
        GearDownVJoyButton = Math.Clamp(GearDownButton, 1, 128),
        PedalPressMs = PressMs,
        PedalReleaseMs = ReleaseMs,
    };

    // AUDIT FIX (F14): dono gear ek hi vJoy number par hon to Buttons page
    // par warning dikhao — Core ka Sanitized() save par ek ko nudge kar
    // deta hai, ye user ko pehle se khabar deta hai.
    public bool GearButtonsConflict => GearUpButton == GearDownButton;

    partial void OnGearUpButtonChanged(int value)
    {
        OnPropertyChanged(nameof(GearButtonsConflict));
        ScheduleSave();
    }

    partial void OnGearDownButtonChanged(int value)
    {
        OnPropertyChanged(nameof(GearButtonsConflict));
        ScheduleSave();
    }
    partial void OnPressMsChanged(double value) => ScheduleSave();
    partial void OnReleaseMsChanged(double value) => ScheduleSave();

    public void Tick()
    {
        if (Services.Source.TryGetLatest(out var p, out _, long.MaxValue))
        {
            GearUpPressed = p.GearUp;
            GearDownPressed = p.GearDown;
            AccelBtnPressed = p.AccelButton;
            BrakeBtnPressed = p.BrakeButton;
        }
        MaskText = $"0x{Services.Engine.CurrentState.ButtonsMask:X8}";
    }
}
