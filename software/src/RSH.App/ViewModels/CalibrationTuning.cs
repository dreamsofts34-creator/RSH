using RSH.App.Services;
using RSH.Core.Pipeline;
using RSH.Core.Processing;
using RSH.Core.Profiles;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using System.Collections.ObjectModel;

namespace RSH.App.ViewModels;

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

    // V-C2: aakhri packet ka steering unit flag. Calibration ke fields
    // (SteerMin/Center/Max, Accel/BrakeMin/Max) hamesha mV-domain mein
    // hote hain (profile se load bhi mV mein hota hai, D10) — is liye
    // capture karte waqt hi packet units ko mV mein badal liya jata
    // hai (neeche Steer/PedalToMillivolts). Save-time conversion
    // ghalat hoti: user sirf center capture kare to min/max loaded
    // (mV) rehte aur dobara convert ho kar kharab ho jate.
    private bool _steeringInMillivolts;

    /// <summary>Packet ki steering value ko mV mein lao (flag true = pehle se mV).</summary>
    private int SteerToMillivolts(int value) =>
        _steeringInMillivolts ? value : InputMapper.RawToMillivolts(value);

    /// <summary>Pedals v1 mein hamesha raw ADC hote hain — mV mein lao.</summary>
    private static int PedalToMillivolts(int value) => InputMapper.RawToMillivolts(value);

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

    [RelayCommand] private void CaptureCenter() { if (HasData) SteerCenter = SteerToMillivolts(LiveSteering); }

    [RelayCommand]
    private void ToggleCaptureSteering()
    {
        _capturingSteering = !_capturingSteering;
        // F12: page ke button ko capture ki halat dikhani hai — notification
        // uthao warna binding taaza nahi hoti.
        OnPropertyChanged(nameof(CapturingSteering));
        if (_capturingSteering && HasData) { SteerMin = SteerToMillivolts(LiveSteering); SteerMax = SteerToMillivolts(LiveSteering); }
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
            AccelMin = PedalToMillivolts(LiveAccel); AccelMax = PedalToMillivolts(LiveAccel);
            BrakeMin = PedalToMillivolts(LiveBrake); BrakeMax = PedalToMillivolts(LiveBrake);
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
    private async Task Save()
    {
        // V-C2: fields pehle se mV-domain mein hain (capture karte
        // waqt convert hue, load bhi mV mein hota hai) — yahan sirf
        // unit tag lagta hai, dobara conversion nahi.
        var profile = services.ActiveProfile with
        {
            SteeringCalibration = new SteeringCalibration { RawMin = SteerMin, RawCenter = SteerCenter, RawMax = SteerMax, IsSet = true, IsMillivolts = true },
            AccelCalibration = new PedalCalibration { RawMin = AccelMin, RawMax = AccelMax, IsSet = true, IsMillivolts = true },
            BrakeCalibration = new PedalCalibration { RawMin = BrakeMin, RawMax = BrakeMax, IsSet = true, IsMillivolts = true },
        };
        if (!profile.SteeringCalibration.IsSane)
        {
            StatusText = "Steering values don't look right (need min < center < max, span at least 660 mV). Capture again.";
            return;
        }
        if (!profile.AccelCalibration.IsSane || !profile.BrakeCalibration.IsSane)
        {
            StatusText = "Pedal values don't look right (need max > min, span at least 660 mV). Capture again.";
            return;
        }
        await services.UpdateActiveProfileAsync(profile);
        LoadFromProfile();
        StatusText = "Calibration saved to the active profile.";
    }

    [RelayCommand]
    private async Task Reset()
    {
        var profile = services.ActiveProfile with
        {
            SteeringCalibration = new SteeringCalibration(),
            AccelCalibration = new PedalCalibration(),
            BrakeCalibration = new PedalCalibration(),
        };
        await services.UpdateActiveProfileAsync(profile);
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
            _steeringInMillivolts = p.SteeringInMillivolts;
            LiveSteering = p.Steering;
            LiveAccel = p.Accel;
            LiveBrake = p.Brake;
            if (_capturingSteering)
            {
                SteerMin = Math.Min(SteerMin, SteerToMillivolts(p.Steering));
                SteerMax = Math.Max(SteerMax, SteerToMillivolts(p.Steering));
            }
            if (_capturingPedals)
            {
                AccelMin = Math.Min(AccelMin, PedalToMillivolts(p.Accel));
                AccelMax = Math.Max(AccelMax, PedalToMillivolts(p.Accel));
                BrakeMin = Math.Min(BrakeMin, PedalToMillivolts(p.Brake));
                BrakeMax = Math.Max(BrakeMax, PedalToMillivolts(p.Brake));
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
            SaveInBackground();
        };
        _saveTimer.Start();
    }

    /// <summary>
    /// U-P1: DispatcherTimer UI thread par chalta hai — save kabhi us par
    /// nahi hota; file I/O background par. In-memory profile update
    /// foran hota hai (UpdateActiveProfileAsync pehle assign karta hai),
    /// is liye disk save fail bhi ho to engine/state durust rehte hain
    /// aur agla ScheduleSave disk dobara likhta hai.
    /// </summary>
    protected async void SaveInBackground()
    {
        try { await Services.UpdateActiveProfileAsync(BuildProfile()); }
        catch { /* upar dekho — in-memory state durust hai, agla save retry karta hai */ }
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
    // P2 (D9): button 33+ par error text — save nahi hota, clamp nahi hota.
    [ObservableProperty] private string _gearButtonsError = "";

    public bool HasGearButtonsError => GearButtonsError.Length > 0;

    partial void OnGearButtonsErrorChanged(string value) => OnPropertyChanged(nameof(HasGearButtonsError));

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

    protected override WheelProfile BuildProfile()
    {
        // P2 (D9): clamp kabhi nahi — had se bahir button par throw karo
        // (UI aam tor par ValidateGearButtons se save rokti hai; ye
        // aakhri hisaar hai taake ghalat number kabhi profile mein na jaye).
        WheelProfile.ValidateVJoyButton(GearUpButton, nameof(GearUpButton));
        WheelProfile.ValidateVJoyButton(GearDownButton, nameof(GearDownButton));
        return Services.ActiveProfile with
        {
            GearUpVJoyButton = GearUpButton,
            GearDownVJoyButton = GearDownButton,
            PedalPressMs = PressMs,
            PedalReleaseMs = ReleaseMs,
        };
    }

    /// <summary>P2 (D9): gear buttons 1..32 hon — warna error dikhao aur save roko.</summary>
    private bool ValidateGearButtons()
    {
        string? error = null;
        if (GearUpButton is < WheelProfile.MinVJoyButton or > WheelProfile.MaxVJoyButton)
            error = $"Gear up button {GearUpButton} is not allowed — vJoy buttons are 1–32 only. Buttons 33–128 are rejected, never remapped.";
        else if (GearDownButton is < WheelProfile.MinVJoyButton or > WheelProfile.MaxVJoyButton)
            error = $"Gear down button {GearDownButton} is not allowed — vJoy buttons are 1–32 only. Buttons 33–128 are rejected, never remapped.";
        GearButtonsError = error ?? "";
        return error is null;
    }

    // AUDIT FIX (F14): dono gear ek hi vJoy number par hon to Buttons page
    // par warning dikhao — Core ka Sanitized() save par ek ko nudge kar
    // deta hai, ye user ko pehle se khabar deta hai.
    public bool GearButtonsConflict => GearUpButton == GearDownButton;

    partial void OnGearUpButtonChanged(int value)
    {
        OnPropertyChanged(nameof(GearButtonsConflict));
        if (ValidateGearButtons()) ScheduleSave();
    }

    partial void OnGearDownButtonChanged(int value)
    {
        OnPropertyChanged(nameof(GearButtonsConflict));
        if (ValidateGearButtons()) ScheduleSave();
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

/// <summary>UI-5: Tuning cards grid ki ek card — naam, icon initials, live On/Off, mini value.</summary>
public sealed partial class ModuleCard : ObservableObject
{
    public ModuleCard(ModuleKind module, string name, string subtitle, string initials)
    {
        Module = module;
        Name = name;
        Subtitle = subtitle;
        Initials = initials;
    }

    public ModuleKind Module { get; }
    public string Name { get; }
    public string Subtitle { get; }
    public string Initials { get; }

    [ObservableProperty] private bool _enabled = true;
    [ObservableProperty] private bool _isSelected;
    [ObservableProperty] private string _miniText = "—";
    [ObservableProperty] private string _miniUnit = "";

    public string StateText => Enabled ? "ON" : "OFF";

    partial void OnEnabledChanged(bool value) => OnPropertyChanged(nameof(StateText));
}

/// <summary>
/// UI-5/UI-6: Tuning page ka ViewModel — 10 module cards + ek waqt mein
/// ek module ki tuning. Card tap sirf us module ka panel kholta hai;
/// on/off toggle yahan nahi — wo sirf Settings → Modules mein hai
/// (plan §0.2: har control sirf aik jagah), is liye band module ka
/// panel disabled dikhta hai. Har tabdeeli 400 ms debounce se profile
/// mein save hoti hai aur <see cref="AppServices.UpdateActiveProfile"/>
/// se engine par foran lagti hai (live). Koi fake control nahi: jin
/// modules ka koi input ya backing field nahi (Shifter gate type,
/// Horn assign, LED brightness/pattern, Encoder pulses-per-step) un ke
/// panel mein asal haalat + wajah likhi hai, kaam na karne wala control
/// nahi. Clutch/Handbrake ka input v2 stream se aata hai (slots 3/4).
/// </summary>
public sealed partial class TuningViewModel : TuningViewModelBase, ILiveViewModel
{
    private static readonly (ModuleKind Module, string Name, string Subtitle, string Initials)[] ModuleTable =
    [
        (ModuleKind.Steering, "Steering", "Wheel axis · vJoy Axis X", "ST"),
        (ModuleKind.Accelerator, "Accelerator", "Analog pedal · vJoy Axis Y", "AC"),
        (ModuleKind.Brake, "Brake", "Analog pedal · vJoy Axis Z", "BR"),
        (ModuleKind.Clutch, "Clutch", "Analog pedal · vJoy WClutch", "CL"),
        (ModuleKind.Handbrake, "Handbrake", "Analog lever · vJoy Slider", "HB"),
        (ModuleKind.Shifter, "Shifter", "Digital gates · Sequential / H-pattern", "SH"),
        (ModuleKind.Horn, "Horn", "Momentary button", "HO"),
        (ModuleKind.Buttons, "Wheel Buttons", "Wheel buttons · vJoy 1–32", "BT"),
        (ModuleKind.Encoders, "Encoders", "Rotary encoders · vJoy buttons 65–72 (pulse)", "EN"),
        (ModuleKind.LED, "Status LED", "Status LED strip", "LED"),
    ];

    public ObservableCollection<ModuleCard> Cards { get; } = [];

    [ObservableProperty] private ModuleKind _selectedModule = ModuleKind.Steering;

    // Selected module ki editing state (sliders/toggles se bandhi).
    [ObservableProperty] private double _deadzonePct;
    [ObservableProperty] private double _curvePct;
    [ObservableProperty] private double _smoothingPct;
    [ObservableProperty] private double _rotationDeg = 900;
    [ObservableProperty] private bool _invert;

    // Live values (Tick se, 30 Hz).
    [ObservableProperty] private string _liveInText = "—";
    [ObservableProperty] private string _liveOutText = "—";
    [ObservableProperty] private double _liveCurveX = double.NaN;
    [ObservableProperty] private double _liveCurveY = double.NaN;
    [ObservableProperty] private bool _gearUpPressed;
    [ObservableProperty] private bool _gearDownPressed;
    [ObservableProperty] private string _shifterGateText = "–";
    [ObservableProperty] private string _buttonsMaskText = "0x00000000";
    [ObservableProperty] private int _pressedCount;
    [ObservableProperty] private string _gearUpVJoyText = "1";
    [ObservableProperty] private string _gearDownVJoyText = "2";
    [ObservableProperty] private bool _hasV2Stream;
    [ObservableProperty] private string _encoderSummaryText = "—";
    [ObservableProperty] private string _encoder1Text = "—";
    [ObservableProperty] private string _encoder2Text = "—";
    [ObservableProperty] private string _encoder3Text = "—";
    [ObservableProperty] private string _encoder4Text = "—";

    [ObservableProperty] private string _endpointText = "";
    [ObservableProperty] private string _endpointsWarning = "";

    private PipelinePlan _plan = PipelinePlan.DefaultPlan();
    private string _loadedProfileName = "";
    private bool _loadingSelection;
    private bool _dirty;

    public TuningViewModel(AppServices services) : base(services)
    {
        foreach (var (module, name, subtitle, initials) in ModuleTable)
            Cards.Add(new ModuleCard(module, name, subtitle, initials));
        LoadProfile();
        ViewModelStore.ProfileChanged += LoadProfile;
    }

    public ModuleCard? SelectedCard => Cards.FirstOrDefault(c => c.Module == SelectedModule);
    public string SelectedName => SelectedCard?.Name ?? "";
    public string SelectedSubtitle => SelectedCard?.Subtitle ?? "";
    public bool SelectedEnabled => SelectedCard?.Enabled ?? true;
    public bool SelectedDisabled => !SelectedEnabled;
    public string SelectedStateText => SelectedEnabled ? "ON" : "OFF";

    public bool IsAnalog => SelectedModule is ModuleKind.Steering or ModuleKind.Accelerator or ModuleKind.Brake or ModuleKind.Clutch or ModuleKind.Handbrake;
    public bool IsSteering => SelectedModule == ModuleKind.Steering;
    public bool IsPedal => SelectedModule is ModuleKind.Accelerator or ModuleKind.Brake or ModuleKind.Clutch or ModuleKind.Handbrake;
    public bool IsButtons => SelectedModule == ModuleKind.Buttons;
    public bool IsEncoders => SelectedModule == ModuleKind.Encoders;
    public bool IsShifter => SelectedModule == ModuleKind.Shifter;
    public bool IsHorn => SelectedModule == ModuleKind.Horn;
    public bool IsLed => SelectedModule == ModuleKind.LED;
    public bool HasCalibration => SelectedModule is ModuleKind.Steering or ModuleKind.Accelerator or ModuleKind.Brake;
    // Clutch/Handbrake: v2 stream in ka axis bhejta hai (slots 3/4) —
    // note sirf tab jab wheel v2 na bhej raha ho (v1 board).
    public bool ShowNoInputNote => (SelectedModule is ModuleKind.Clutch or ModuleKind.Handbrake) && !HasV2Stream;
    public bool HasNoV2Stream => !HasV2Stream;

    // Deadzone pedal par bite point hai (0..1 domain mein neeche ka
    // hissa), steering par centre deadzone — ek hi slider, label badalta hai.
    public string DeadzoneLabel => IsPedal ? "Bite point (bottom deadzone)" : "Centre deadzone";

    public bool IsLinearCurve => Math.Abs(CurvePct) < 0.5;
    public bool IsProgressiveCurve => Math.Abs(CurvePct - 40) < 0.5;
    public bool IsCustomCurve => !IsLinearCurve && !IsProgressiveCurve;
    public string CurvePresetText => IsLinearCurve ? "Linear" : IsProgressiveCurve ? "Progressive" : "Custom";
    public bool HasEndpointsWarning => EndpointsWarning.Length > 0;

    partial void OnHasV2StreamChanged(bool value)
    {
        OnPropertyChanged(nameof(HasNoV2Stream));
        // v2 aate hi Clutch/Handbrake ka "not sending" note khatam ho.
        OnPropertyChanged(nameof(ShowNoInputNote));
    }

    private void RaisePanelFlags()
    {
        OnPropertyChanged(nameof(SelectedCard));
        OnPropertyChanged(nameof(SelectedName));
        OnPropertyChanged(nameof(SelectedSubtitle));
        OnPropertyChanged(nameof(SelectedEnabled));
        OnPropertyChanged(nameof(SelectedDisabled));
        OnPropertyChanged(nameof(SelectedStateText));
        OnPropertyChanged(nameof(IsAnalog));
        OnPropertyChanged(nameof(IsSteering));
        OnPropertyChanged(nameof(IsPedal));
        OnPropertyChanged(nameof(IsButtons));
        OnPropertyChanged(nameof(IsEncoders));
        OnPropertyChanged(nameof(IsShifter));
        OnPropertyChanged(nameof(IsHorn));
        OnPropertyChanged(nameof(IsLed));
        OnPropertyChanged(nameof(HasCalibration));
        OnPropertyChanged(nameof(ShowNoInputNote));
        OnPropertyChanged(nameof(DeadzoneLabel));
        RaisePresets();
    }

    private void RaisePresets()
    {
        OnPropertyChanged(nameof(IsLinearCurve));
        OnPropertyChanged(nameof(IsProgressiveCurve));
        OnPropertyChanged(nameof(IsCustomCurve));
        OnPropertyChanged(nameof(CurvePresetText));
    }

    protected override void LoadFrom(WheelProfile p)
    {
        _loadedProfileName = Services.Profiles.ActiveProfileName;
        _plan = PipelinePlanBuilder.FromProfile(p);
        foreach (var card in Cards)
            card.Enabled = _plan.IsEnabled(card.Module);
        GearUpVJoyText = p.GearUpVJoyButton.ToString();
        GearDownVJoyText = p.GearDownVJoyButton.ToString();
        LoadSelectedEditing();
        UpdateCardSelection();
        RaisePanelFlags();
    }

    private void UpdateCardSelection()
    {
        foreach (var card in Cards)
            card.IsSelected = card.Module == SelectedModule;
    }

    private void LoadSelectedEditing()
    {
        _loadingSelection = true;
        try
        {
            var t = _plan[SelectedModule].Tuning;
            DeadzonePct = t.Deadzone * 100;
            CurvePct = t.CurveStrength * 100;
            SmoothingPct = t.Smoothing * 100;
            Invert = t.Invert;
            RotationDeg = Math.Round(t.Scale * 900);
            UpdateEndpoints();
        }
        finally { _loadingSelection = false; }
    }

    /// <summary>
    /// Endpoints read-only hain (calibration capture Setup mein hai —
    /// yahan dobara edit control nahi). P9 guard: span ≥ 660 mV
    /// (full-scale 3300 mV ka 20%); IsSane yehi check karta hai.
    /// </summary>
    private void UpdateEndpoints()
    {
        var p = Services.ActiveProfile;
        switch (SelectedModule)
        {
            case ModuleKind.Steering:
            {
                var c = p.SteeringCalibration;
                EndpointText = c.IsSet
                    ? $"Min {c.RawMin} mV · Centre {c.RawCenter} mV · Max {c.RawMax} mV"
                    : "Not calibrated — full sensor range in use.";
                EndpointsWarning = c.IsSet && !c.IsSane
                    ? "Calibration span is under 660 mV — recalibrate in Setup."
                    : "";
                break;
            }
            case ModuleKind.Accelerator:
            {
                var c = p.AccelCalibration;
                EndpointText = c.IsSet
                    ? $"Min {c.RawMin} mV · Max {c.RawMax} mV"
                    : "Not calibrated — full sensor range in use.";
                EndpointsWarning = c.IsSet && !c.IsSane
                    ? "Calibration span is under 660 mV — recalibrate in Setup."
                    : "";
                break;
            }
            case ModuleKind.Brake:
            {
                var c = p.BrakeCalibration;
                EndpointText = c.IsSet
                    ? $"Min {c.RawMin} mV · Max {c.RawMax} mV"
                    : "Not calibrated — full sensor range in use.";
                EndpointsWarning = c.IsSet && !c.IsSane
                    ? "Calibration span is under 660 mV — recalibrate in Setup."
                    : "";
                break;
            }
            default:
                EndpointText = "No calibration stored for this module.";
                EndpointsWarning = "";
                break;
        }
        OnPropertyChanged(nameof(HasEndpointsWarning));
    }

    protected override WheelProfile BuildProfile()
    {
        var profile = Services.ActiveProfile;
        var tunings = new Dictionary<string, PerModuleTuning>(profile.ModuleTunings, StringComparer.OrdinalIgnoreCase);
        PerModuleTuning current = tunings.TryGetValue(SelectedModule.ToString(), out var existing) && existing is not null
            ? existing : new PerModuleTuning();
        double scale = SelectedModule == ModuleKind.Steering
            ? Math.Clamp(RotationDeg / 900.0, 0.2, 1.0)
            : current.Scale;
        tunings[SelectedModule.ToString()] = current with
        {
            Deadzone = Math.Clamp(DeadzonePct / 100.0, 0, 0.5),
            CurveStrength = Math.Clamp(CurvePct / 100.0, -1, 1),
            Smoothing = Math.Clamp(SmoothingPct / 100.0, 0, 1),
            Invert = Invert,
            Scale = scale,
            Enabled = _plan.IsEnabled(SelectedModule),
        };
        var updated = profile with { ModuleTunings = tunings };
        // Legacy v1 fields ka mirror — purani ProcessingPage / readers
        // steering ki qeematein yahin se parhte hain; ModuleTunings
        // override phir bhi jeetta hai (FromProfile), is liye dono
        // ek jaisi rehti hain.
        if (SelectedModule == ModuleKind.Steering)
            updated = updated with
            {
                Deadzone = DeadzonePct / 100.0,
                CurveStrength = CurvePct / 100.0,
                Smoothing = SmoothingPct / 100.0,
                SteeringScale = scale,
                InvertSteering = Invert,
            };
        else if (SelectedModule == ModuleKind.Accelerator)
            updated = updated with { InvertAccel = Invert };
        else if (SelectedModule == ModuleKind.Brake)
            updated = updated with { InvertBrake = Invert };
        return updated;
    }

    partial void OnDeadzonePctChanged(double value) { if (Loading || _loadingSelection) return; _dirty = true; ScheduleSave(); }
    partial void OnSmoothingPctChanged(double value) { if (Loading || _loadingSelection) return; _dirty = true; ScheduleSave(); }
    partial void OnRotationDegChanged(double value) { if (Loading || _loadingSelection) return; _dirty = true; ScheduleSave(); }
    partial void OnInvertChanged(bool value) { if (Loading || _loadingSelection) return; _dirty = true; ScheduleSave(); }

    partial void OnCurvePctChanged(double value)
    {
        RaisePresets();
        if (Loading || _loadingSelection) return;
        _dirty = true;
        ScheduleSave();
    }

    [RelayCommand] private void PresetLinear() => CurvePct = 0;
    [RelayCommand] private void PresetProgressive() => CurvePct = 40;

    [ObservableProperty] private string _ledTestStatusText = "";
    private bool _ledTesting;

    // Phase H WS2: LED pattern + brightness (Tuning panel se bandhe hain).
    // Pattern IDs (§3.7): 0 OFF, 1 SOLID, 2 BLINK, 3 CHASE, 4 RAINBOW.
    // Brightness double hai (Slider Value double hoti hai — baqi sliders
    // jaisi); LedTest() mein int mein round hoti hai (0..100).
    [ObservableProperty] private int _ledPatternIndex = 4;
    [ObservableProperty] private double _ledBrightnessPct = 100;

    /// <summary>LED pattern names (status text + XAML ComboBox items).</summary>
    public static string[] LedPatternNames { get; } =
        ["OFF", "SOLID", "BLINK", "CHASE", "RAINBOW"];

    /// <summary>
    /// LED panel → Test pattern (UI-11 stretch): HMAC-signed LED_TEST
    /// (BoardSession.SendLedTestAsync — chuna hua pattern + brightness,
    /// 1000 ms, 5 s). Pattern ke rang: SOLID laal, BLINK hari, CHASE
    /// neeli, RAINBOW/OFF rang nahi lete (A7 §25 ke mutabiq). Board par
    /// LED module na ho ya session key na ho to saaf wajah status mein
    /// — khamoshi se fake success kabhi nahi.
    /// </summary>
    [RelayCommand]
    private async Task LedTest()
    {
        if (_ledTesting) return;
        _ledTesting = true;
        LedTestStatusText = "Sending LED test…";
        try
        {
            int pattern = Math.Clamp(LedPatternIndex, 0, 4);
            (byte r, byte g, byte b) = pattern switch
            {
                1 => ((byte)255, (byte)0, (byte)0),    // SOLID red
                2 => ((byte)0, (byte)255, (byte)0),    // BLINK green
                3 => ((byte)0, (byte)0, (byte)255),    // CHASE blue
                _ => ((byte)0, (byte)0, (byte)0),      // OFF / RAINBOW
            };
            int brightness = (int)Math.Clamp(Math.Round(LedBrightnessPct), 0, 100);
            var (sent, error) = await Services.BoardSession.SendLedTestAsync(
                Services.Settings.EspIp, (byte)pattern, r, g, b, 1000, 0, brightness);
            LedTestStatusText = sent
                ? $"LED test sent — the strip runs a {LedPatternNames[pattern]} pattern for 5 seconds at {brightness}% brightness."
                : $"LED test failed: {error ?? "no session with the wheel"}";
        }
        catch (Exception ex)
        {
            LedTestStatusText = $"LED test failed: {ex.Message}";
        }
        finally
        {
            _ledTesting = false;
        }
    }

    /// <summary>Card tap — ek waqt mein ek panel. Pehle module ki pending save foran likho, phir selection badlo.</summary>
    [RelayCommand]
    private void SelectModule(ModuleKind module)
    {
        if (module == SelectedModule) return;
        FlushPendingSave();
        SelectedModule = module;
    }

    partial void OnSelectedModuleChanged(ModuleKind value)
    {
        LoadSelectedEditing();
        UpdateCardSelection();
        RaisePanelFlags();
    }

    private void FlushPendingSave()
    {
        if (!_dirty) return;
        _dirty = false;
        SaveInBackground();
    }

    public void Tick()
    {
        // Profile badal jaye to dobara load karo; warna Settings toggles
        // (UI-11) se sync ke liye enabled flags har tick taaza karo —
        // card state ka wahid source profile hai.
        if (_loadedProfileName != Services.Profiles.ActiveProfileName)
        {
            LoadProfile();
        }
        else
        {
            var tunings = Services.ActiveProfile.ModuleTunings;
            bool enabledChanged = false;
            foreach (var card in Cards)
            {
                bool enabled = tunings.TryGetValue(card.Module.ToString(), out var t) && t is not null
                    ? t.Enabled : true;
                if (card.Enabled != enabled) { card.Enabled = enabled; enabledChanged = true; }
            }
            if (enabledChanged) RaisePanelFlags();
        }

        var state = Services.Engine.CurrentState;
        int pressed = System.Numerics.BitOperations.PopCount(state.ButtonsMask)
            + System.Numerics.BitOperations.PopCount(state.ButtonsMaskEx);
        PressedCount = pressed;
        ButtonsMaskText = $"0x{state.ButtonsMask:X8}";

        bool hasV1 = Services.Source.TryGetLatest(out var packet, out _, long.MaxValue);
        GearUpPressed = hasV1 && packet.GearUp;
        GearDownPressed = hasV1 && packet.GearDown;
        ShifterGateText = GearUpPressed ? "Up" : GearDownPressed ? "Down" : "–";

        // Encoders — sirf v2 stream mein hote hain (cumulative counts).
        if (Services.Source.TryGetLatestV2(out var v2, out _, 500))
        {
            HasV2Stream = true;
            Encoder1Text = v2.EncoderCount > 0 ? v2.Encoders[0].ToString() : "—";
            Encoder2Text = v2.EncoderCount > 1 ? v2.Encoders[1].ToString() : "—";
            Encoder3Text = v2.EncoderCount > 2 ? v2.Encoders[2].ToString() : "—";
            Encoder4Text = v2.EncoderCount > 3 ? v2.Encoders[3].ToString() : "—";
            int total = 0;
            for (int i = 0; i < v2.EncoderCount; i++) total += Math.Abs(v2.Encoders[i]);
            EncoderSummaryText = total.ToString();
        }
        else
        {
            HasV2Stream = false;
            Encoder1Text = Encoder2Text = Encoder3Text = Encoder4Text = "—";
            EncoderSummaryText = "—";
        }

        double steerRotation = _plan[ModuleKind.Steering].Tuning.Scale * 900;
        foreach (var card in Cards)
        {
            switch (card.Module)
            {
                case ModuleKind.Steering:
                    card.MiniText = $"{Math.Abs(state.SteeringNorm) * steerRotation / 2:F0}";
                    card.MiniUnit = "°";
                    break;
                case ModuleKind.Accelerator:
                    card.MiniText = $"{state.AccelNorm * 100:F0}";
                    card.MiniUnit = "%";
                    break;
                case ModuleKind.Brake:
                    card.MiniText = $"{state.BrakeNorm * 100:F0}";
                    card.MiniUnit = "%";
                    break;
                case ModuleKind.Clutch:
                    card.MiniText = $"{state.ClutchNorm * 100:F0}";
                    card.MiniUnit = "%";
                    break;
                case ModuleKind.Handbrake:
                    card.MiniText = $"{state.HandbrakeNorm * 100:F0}";
                    card.MiniUnit = "%";
                    break;
                case ModuleKind.Shifter:
                    card.MiniText = ShifterGateText;
                    card.MiniUnit = "gate";
                    break;
                case ModuleKind.Horn:
                case ModuleKind.Buttons:
                    card.MiniText = pressed.ToString();
                    card.MiniUnit = "pressed";
                    break;
                case ModuleKind.Encoders:
                    card.MiniText = EncoderSummaryText;
                    card.MiniUnit = "steps";
                    break;
                case ModuleKind.LED:
                    card.MiniText = "—";
                    card.MiniUnit = "strip";
                    break;
            }
        }

        // Selected analog panel: live input (packet) vs live output (engine state).
        switch (SelectedModule)
        {
            case ModuleKind.Steering:
                if (hasV1)
                {
                    var p = Services.ActiveProfile;
                    double input = InputMapper.MapSteering(packet.Steering, p.SteeringCalibration, packet.SteeringInMillivolts, invert: false);
                    LiveCurveX = input;
                    LiveCurveY = state.SteeringNorm;
                    LiveInText = $"{input * steerRotation / 2:F0}°";
                    LiveOutText = $"{state.SteeringNorm * steerRotation / 2:F0}°";
                }
                else { LiveCurveX = LiveCurveY = double.NaN; LiveInText = LiveOutText = "—"; }
                break;
            case ModuleKind.Accelerator:
                if (hasV1)
                {
                    var p = Services.ActiveProfile;
                    double input = InputMapper.MapPedal(packet.Accel, p.AccelCalibration, invert: false);
                    LiveCurveX = input;
                    LiveCurveY = state.AccelNorm;
                    LiveInText = $"{input * 100:F0}%";
                    LiveOutText = $"{state.AccelNorm * 100:F0}%";
                }
                else { LiveCurveX = LiveCurveY = double.NaN; LiveInText = LiveOutText = "—"; }
                break;
            case ModuleKind.Brake:
                if (hasV1)
                {
                    var p = Services.ActiveProfile;
                    double input = InputMapper.MapPedal(packet.Brake, p.BrakeCalibration, invert: false);
                    LiveCurveX = input;
                    LiveCurveY = state.BrakeNorm;
                    LiveInText = $"{input * 100:F0}%";
                    LiveOutText = $"{state.BrakeNorm * 100:F0}%";
                }
                else { LiveCurveX = LiveCurveY = double.NaN; LiveInText = LiveOutText = "—"; }
                break;
            default:
                LiveCurveX = LiveCurveY = double.NaN;
                LiveInText = LiveOutText = "—";
                break;
        }
    }
}
