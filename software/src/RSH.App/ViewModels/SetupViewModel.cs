using System.Collections.ObjectModel;
using RSH.App.Services;
using RSH.Core.Net;
using RSH.Core.Processing;
using RSH.Core.Profiles;
using RSH.Core.Protocol;
using RSH.Core.Utilities;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace RSH.App.ViewModels;

/// <summary>
/// UI-7 Setup flow: Connect → Pins → Calibrate, ek flow mein.
/// Connect: wheel address + Wi-Fi join + live link (fail → clear error + retry).
/// Pins: board ki pin table, read-only (edit sirf Settings → GPIO pins mein).
/// Calibrate: centre → full range → pedals → save (pehle wala capture logic).
/// Wizard beech mein band ho to kuch save nahi hota — profile sirf Finish
/// (calibration save) par badalti hai; band karne par Discard() sirf
/// in-memory capture reset karta hai.
/// </summary>
public sealed partial class SetupViewModel : ObservableObject, ILiveViewModel
{
    // Gates: SetupGates (D10, mV) — single source of truth. Raw units par
    // gate kabhi nahi (SETUP-GATES-DECISION.md §2.1): live values pehle
    // mV mein badlo, phir SteeringRangeComplete / PedalComplete se napo.
    // (Purane constants SteerSide=250 / PedalSpan=150 raw thay — wizard
    // "captured ✓" keh kar Finish par IsSane se reject karwata tha.)

    // Aakhri GET_CAPS reply (RefreshPinsAsync se) — Dashboard banner +
    // recalibration prompt isi se live context banate hain (board se
    // dobara mangne ki zaroorat nahi, Tick mein network I/O nahi).
    public static BoardCaps? LastBoardCaps { get; private set; }

    private readonly AppServices services;

    /// <summary>
    /// SEC-E-14 bridge (Phase F W2 VM bridge): AppServices se aane wale
    /// caps is static mein — Dashboard/Setup yahin se parhte hain.
    /// </summary>
    public SetupViewModel(AppServices services)
    {
        this.services = services;
        services.BoardCapsUpdated += () => { if (services.LastBoardCaps is { } c) LastBoardCaps = c; };
    }

    // --- Flow state (Connect → Pins → Calibrate) ---
    [ObservableProperty] private int _flowStep;
    [ObservableProperty] private bool _isConnected;
    [ObservableProperty] private string _hostText = "";
    [ObservableProperty] private string _hostError = "";
    [ObservableProperty] private string _connectStatus = "Not connected yet.";
    [ObservableProperty] private string _connectError = "";
    [ObservableProperty] private bool _pinsLoading;
    [ObservableProperty] private string _pinsSourceText = "";
    [ObservableProperty] private string _pinsError = "";

    public bool HasConnectError => ConnectError.Length > 0;
    partial void OnConnectErrorChanged(string value) => OnPropertyChanged(nameof(HasConnectError));

    // --- Calibration capture state (pehle wala logic, Step = calib sub-step) ---
    [ObservableProperty] private int _step;
    [ObservableProperty] private bool _hasData;
    [ObservableProperty] private int _liveSteering;
    [ObservableProperty] private int _liveAccel;
    [ObservableProperty] private int _liveBrake;
    [ObservableProperty] private double _livePos = 50;
    [ObservableProperty] private double _minPos;
    [ObservableProperty] private double _maxPos;
    [ObservableProperty] private double _rangePercent;
    [ObservableProperty] private double _accelPercent;
    [ObservableProperty] private double _brakePercent;
    [ObservableProperty] private string _leftText = "";
    [ObservableProperty] private string _rightText = "";
    [ObservableProperty] private string _accelText = "";
    [ObservableProperty] private string _brakeText = "";
    [ObservableProperty] private string _summary = "";
    [ObservableProperty] private string _message = "";

    // NeedsRecalibration prompt (SETUP-GATES-DECISION.md §3.2, verbatim):
    // Setup flow mein entry par + Dashboard banner par. Dismiss sirf is
    // visit ke liye hai — dobara Setup khule ya wajah badle to prompt
    // dobara aata hai jab tak trigger zinda ho; kamyaab recalibration
    // (Finish → MarkCalibrated) use hamesha ke liye saaf karta hai.
    [ObservableProperty] private string? _recalibrationPrompt;
    public bool HasRecalibrationPrompt => RecalibrationPrompt is not null;
    partial void OnRecalibrationPromptChanged(string? value) => OnPropertyChanged(nameof(HasRecalibrationPrompt));
    private RecalibrationReason _promptDismissedFor = RecalibrationReason.None;
    private string _promptKey = "";

    /// <summary>Live recalibration context: board caps (pin map + firmware) ya HELLO firmware version, jab caps na hon.</summary>
    private CalibrationContext CurrentCalibrationContext()
    {
        string? fingerprint = LastBoardCaps is not null
            ? CalibrationFreshness.PinMapFingerprint(LastBoardCaps)
            : null;
        string? firmware = LastBoardCaps?.FirmwareVersion.ToString()
            ?? services.BoardSession.HelloFirmwareVersion?.ToString();
        return new CalibrationContext(fingerprint, firmware, DateTimeOffset.UtcNow);
    }

    /// <summary>
    /// Prompt taaza karo — sirf tab jab profile / caps / firmware badle
    /// hon (Tick har 33 ms chalta hai; CheckRecalibration pure hai lekin
    /// prompt string har tick banana allocation hai, U-P7).
    /// </summary>
    public void RefreshRecalibrationPrompt()
    {
        var profile = services.ActiveProfile;
        var check = profile.CheckRecalibration(CurrentCalibrationContext());
        if (!check.NeedsRecalibration || check.Reasons == _promptDismissedFor)
        {
            RecalibrationPrompt = null;
            return;
        }
        RecalibrationPrompt = check.PromptMessage;
    }

    private void RefreshRecalibrationPromptIfChanged()
    {
        // Key mein calibration stamps bhi hon — Finish (MarkCalibrated)
        // profile ka naam nahi badalta, lekin stamps badalte hain.
        var profile = services.ActiveProfile;
        string key = profile.Name + "|"
            + (profile.CalibratedAtUtc?.ToString("O") ?? "nocal") + "|"
            + (profile.CalibrationPinMapFingerprint ?? "nofp") + "|"
            + (LastBoardCaps is null ? "nocaps" : LastBoardCaps.GetHashCode().ToString(System.Globalization.CultureInfo.InvariantCulture)) + "|"
            + (services.BoardSession.HelloFirmwareVersion?.ToString() ?? "nofw");
        if (key == _promptKey) return;
        _promptKey = key;
        RefreshRecalibrationPrompt();
    }

    [RelayCommand]
    private void DismissRecalibrationPrompt()
    {
        _promptDismissedFor = services.ActiveProfile
            .CheckRecalibration(CurrentCalibrationContext()).Reasons;
        RecalibrationPrompt = null;
    }

    /// <summary>Prompt se seedha calibration step par jao (flow step 2).</summary>
    [RelayCommand]
    private void StartRecalibration()
    {
        RecalibrationPrompt = null;
        FlowStep = 2;
    }

    private int _center, _sMin, _sMax, _aMin, _aMax, _bMin, _bMax;
    private bool _leftDone, _rightDone, _accelDone, _brakeDone, _lastCanNext;
    private bool _lastCanFlowNext, _lastCanFlowBack;
    private bool _connectAttempted;
    private bool _pinsRefreshing;

    // U-P4: host input 400 ms debounce — har keystroke par save/HELO restart banned.
    private readonly Debouncer _hostDebouncer = new(TimeSpan.FromMilliseconds(400));

    // V-C2: aakhri packet ka steering unit flag — Finish mein steering
    // ki saved values ko mV mein badalne ke liye.
    private bool _steeringInMillivolts;

    public Action? RequestHome { get; set; }

    /// <summary>Connect step ke liye Connection VM (Wi-Fi join, HELO/link text).</summary>
    public ConnectionViewModel Connection => ViewModelStore.Connection;

    /// <summary>Pin table ki ek row (read-only view).</summary>
    public sealed record PinRow(string Function, string Gpio, string Kind, string Detail);

    public ObservableCollection<PinRow> PinRows { get; } = [];

    public bool HasPinRows => PinRows.Count > 0;

    // --- Flow derived state ---
    public bool IsFlowConnect => FlowStep == 0;
    public bool IsFlowPins => FlowStep == 1;
    public bool IsFlowCalibrate => FlowStep == 2;
    public bool CanFlowBack => FlowStep > 0 || Step > 0;

    private int FlowChipState(int i) => FlowStep == i ? 1 : FlowStep > i ? 2 : 0;
    private string FlowChipText(int i, string name) => FlowStep > i ? $"{i + 1} {name} ✓" : $"{i + 1} {name}";
    public int FlowChip0 => FlowChipState(0);
    public int FlowChip1 => FlowChipState(1);
    public int FlowChip2 => FlowChipState(2);
    public string FlowChip0Text => FlowChipText(0, "Connect");
    public string FlowChip1Text => FlowChipText(1, "Pins");
    public string FlowChip2Text => FlowChipText(2, "Calibrate");
    public string FlowStepCounter => $"Step {FlowStep + 1} of 3";

    public double OverallProgress => FlowStep switch
    {
        0 => IsConnected ? 15 : 0,
        1 => 33,
        _ => 66 + Step * 34.0 / 3.0,
    };

    public string FlowNextLabel => FlowStep switch
    {
        0 => "Continue",
        1 => "Continue to calibration",
        _ => NextLabel,
    };

    // --- Calibration derived state (pehle jaisi) ---
    public bool IsStep0 => Step == 0;
    public bool IsStep1 => Step == 1;
    public bool IsStep2 => Step == 2;
    public bool IsStep3 => Step == 3;
    public bool ShowTrack => Step <= 1;
    public bool ShowMarkers => Step == 1;
    private int ChipState(int i) => Step == i ? 1 : Step > i ? 2 : 0;
    private string ChipText(int i, string name) => Step > i ? $"{i + 1} {name} ✓" : $"{i + 1} {name}";
    public int Chip0 => ChipState(0);
    public int Chip1 => ChipState(1);
    public int Chip2 => ChipState(2);
    public int Chip3 => ChipState(3);
    public string Chip0Text => ChipText(0, "Centre");
    public string Chip1Text => ChipText(1, "Full range");
    public string Chip2Text => ChipText(2, "Pedals");
    public string Chip3Text => ChipText(3, "Done");

    public string StepTitle => Step switch
    {
        0 => "Centre the wheel",
        1 => "Find the full range",
        2 => "Calibrate the pedals",
        _ => "Ready to save",
    };

    public string StepHint => Step switch
    {
        0 => "Hold the wheel straight ahead, then press Set centre.",
        1 => "Turn slowly all the way left, then all the way right. Stop gently at each end.",
        2 => "Keep both pedals released, then press each one fully down once.",
        _ => "All set. Save to finish. You can redo this any time.",
    };

    public string NextLabel => Step switch
    {
        0 => "Set centre",
        3 => "Save and finish",
        _ => "Continue",
    };

    partial void OnFlowStepChanged(int value)
    {
        OnPropertyChanged(nameof(IsFlowConnect));
        OnPropertyChanged(nameof(IsFlowPins));
        OnPropertyChanged(nameof(IsFlowCalibrate));
        OnPropertyChanged(nameof(CanFlowBack));
        OnPropertyChanged(nameof(OverallProgress));
        OnPropertyChanged(nameof(FlowStepCounter));
        OnPropertyChanged(nameof(FlowNextLabel));
        OnPropertyChanged(nameof(FlowChip0));
        OnPropertyChanged(nameof(FlowChip1));
        OnPropertyChanged(nameof(FlowChip2));
        OnPropertyChanged(nameof(FlowChip0Text));
        OnPropertyChanged(nameof(FlowChip1Text));
        OnPropertyChanged(nameof(FlowChip2Text));
        if (value == 1) _ = RefreshPinsAsync();
        if (value == 2) ResetCalibration();
    }

    partial void OnStepChanged(int value)
    {
        OnPropertyChanged(nameof(IsStep0));
        OnPropertyChanged(nameof(IsStep1));
        OnPropertyChanged(nameof(IsStep2));
        OnPropertyChanged(nameof(IsStep3));
        OnPropertyChanged(nameof(OverallProgress));
        OnPropertyChanged(nameof(StepTitle));
        OnPropertyChanged(nameof(StepHint));
        OnPropertyChanged(nameof(NextLabel));
        OnPropertyChanged(nameof(FlowNextLabel));
        OnPropertyChanged(nameof(ShowTrack));
        OnPropertyChanged(nameof(ShowMarkers));
        OnPropertyChanged(nameof(Chip0));
        OnPropertyChanged(nameof(Chip1));
        OnPropertyChanged(nameof(Chip2));
        OnPropertyChanged(nameof(Chip3));
        OnPropertyChanged(nameof(Chip0Text));
        OnPropertyChanged(nameof(Chip1Text));
        OnPropertyChanged(nameof(Chip2Text));
        OnPropertyChanged(nameof(Chip3Text));
    }

    partial void OnHostTextChanged(string value)
    {
        // Begin() se set ho raha hai to debounce nahi — seedha HELO
        // restart (unchanged value par bhi) nahi chahiye.
        if (_settingHostText) return;
        _hostDebouncer.Debounce(ApplyHost);
    }

    // Begin() ke doran HostText set karte waqt debounce dabao (upar dekho).
    private bool _settingHostText;

    /// <summary>
    /// U-P4/U-P1: debounce ke baad background thread par apply — file I/O
    /// (SaveSettingsAsync) + HELO restart kabhi UI thread par nahi. SEC-C-03:
    /// sirf private IPv4 literal qabool hai — hostname/mDNS yahan nahi
    /// chalta (rsh.local sirf hint text mein hai; wheel AP address
    /// 192.168.4.1 hi apply hota hai).
    /// </summary>
    private async void ApplyHost()
    {
        string host = HostText;
        if (!EspAddress.TryValidate(host, out var ip, out string? error))
        {
            SetHostError(error ?? "That is not a valid wheel address.");
            return;
        }
        string text = ip.ToString();
        services.Settings.EspIp = text;
        await services.SaveSettingsAsync();
        services.RestartHello(text);
        RunOnUi(() =>
        {
            ViewModelStore.Connection.EspIp = text;
            HostError = "";
        });
    }

    private void SetHostError(string text)
    {
        var dispatcher = System.Windows.Application.Current?.Dispatcher;
        if (dispatcher is not null && !dispatcher.CheckAccess())
            dispatcher.Invoke(() => HostError = text);
        else
            HostError = text;
    }

    private static void RunOnUi(Action action)
    {
        var dispatcher = System.Windows.Application.Current?.Dispatcher;
        if (dispatcher is not null && !dispatcher.CheckAccess())
            dispatcher.Invoke(action);
        else
            action();
    }

    public void Begin()
    {
        FlowStep = 0;
        // Property setter se — lekin debounce daba kar, warna Begin()
        // (navigation/wizard band) bhi HELO restart kar deta (unchanged
        // value par bhi). MVVMTK0034: backing field (_hostText) ko
        // seedha haath mat lagao — generated property istemal karo.
        _settingHostText = true;
        try { HostText = services.Settings.EspIp; }
        finally { _settingHostText = false; }
        HostError = "";
        ConnectError = "";
        _connectAttempted = false;
        _promptDismissedFor = RecalibrationReason.None;
        _promptKey = "";
        ResetCalibration();
        Refresh();
        RefreshRecalibrationPrompt();
    }

    /// <summary>
    /// Wizard beech mein band ho / page se navigate ho to partial state
    /// save nahi hoti — sirf in-memory capture + flow step reset hota hai.
    /// Profile ko kuch nahi hota (Finish hi save karta hai).
    /// </summary>
    public void Discard() => Begin();

    private void ResetCalibration()
    {
        _center = _sMin = _sMax = _aMin = _aMax = _bMin = _bMax = 0;
        Message = "";
        Step = 0;
        Refresh();
    }

    // --- Flow navigation ---

    private bool CanFlowNext() => FlowStep switch
    {
        0 => IsConnected,
        1 => true,
        _ => CanNext(),
    };

    [RelayCommand(CanExecute = nameof(CanFlowNext))]
    private async Task FlowNext()
    {
        Message = "";
        switch (FlowStep)
        {
            case 0: FlowStep = 1; break;
            case 1: FlowStep = 2; break;
            default: await Next(); break;
        }
        Refresh();
    }

    [RelayCommand(CanExecute = nameof(CanFlowBack))]
    private void FlowBack()
    {
        if (FlowStep == 2 && Step > 0) Back();
        else if (FlowStep > 0) FlowStep--;
        Refresh();
    }

    /// <summary>Connect fail → Retry: HELO dobara shuru karo aur link dobara check karo.</summary>
    [RelayCommand]
    private void RetryConnect()
    {
        _connectAttempted = true;
        services.RestartHello(services.Settings.EspIp);
        Refresh();
    }

    /// <summary>Connect step ka Connect button: Wi-Fi join (Connection VM, async) + link check.</summary>
    [RelayCommand]
    private void Connect()
    {
        _connectAttempted = true;
        Connection.ConnectWifiCommand.Execute(null);
        Refresh();
    }

    [RelayCommand]
    private void RefreshPins() => _ = RefreshPinsAsync();

    /// <summary>
    /// Pins step: board ki pin table GET_CAPS se lao (async, UI thread par
    /// koi I/O nahi — U-P1). Session key / link na ho to default DevKit V1
    /// wiring dikhao aur wajah batao; table kabhi khaali nahi rehti.
    /// </summary>
    public async Task RefreshPinsAsync()
    {
        if (_pinsRefreshing) return;
        _pinsRefreshing = true;
        RunOnUi(() => PinsLoading = true);
        try
        {
            BoardCaps? caps = null;
            string? error = null;
            try
            {
                using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(5));
                (caps, error) = await services.BoardSession.GetCapsAsync(
                    services.Settings.EspIp, cancellationToken: cts.Token).ConfigureAwait(false);
            }
            catch (OperationCanceledException)
            {
                error = "Timed out waiting for the board.";
            }
            catch (Exception ex) when (ex is System.Net.Sockets.SocketException or System.IO.IOException or InvalidOperationException)
            {
                error = ex.Message;
            }

            List<PinRow> rows;
            string source;
            string pinsError = "";
            if (caps is { PinTable.Count: > 0 })
            {
                LastBoardCaps = caps;
                rows = caps.PinTable
                    .OrderBy(p => p.Pin)
                    .Select(p => new PinRow(
                        ModuleName(p.Module),
                        $"GPIO {p.Pin}",
                        KindName(p.Kind),
                        CapsDetail(p)))
                    .ToList();
                source = $"Board pin table — firmware v{caps.FirmwareVersion}, protocol v{caps.ProtocolVersion}.";
            }
            else
            {
                // Board table nahi mili: Settings → GPIO pins ka local map
                // dikhao (PinMapStore) — pin change yahin se sync hota hai.
                rows = LocalPinRows();
                source = "Local pin map — Settings → GPIO pins. (Board pin table unavailable.)";
                pinsError = caps is null
                    ? $"Board pin table unavailable ({error ?? "no session yet — connect first"}). Showing the saved pin map instead."
                    : "Board reported an empty pin table. Showing the saved pin map instead.";
            }
            RunOnUi(() =>
            {
                PinRows.Clear();
                foreach (var row in rows) PinRows.Add(row);
                PinsSourceText = source;
                PinsError = pinsError;
                OnPropertyChanged(nameof(HasPinRows));
                RefreshRecalibrationPrompt();
            });
        }
        finally
        {
            _pinsRefreshing = false;
            RunOnUi(() => PinsLoading = false);
        }
    }

    private static string KindName(CapsPinKind kind) => kind switch
    {
        CapsPinKind.AdcNative => "ADC (native)",
        CapsPinKind.AdcAds1115 => "ADC (ADS1115)",
        CapsPinKind.DigitalIn => "Digital in",
        CapsPinKind.DigitalOut => "Digital out",
        CapsPinKind.LedStrip => "LED strip",
        CapsPinKind.I2cSda => "I²C SDA",
        CapsPinKind.I2cScl => "I²C SCL",
        CapsPinKind.ExpanderMcp23017 => "MCP23017",
        CapsPinKind.ExpanderPcf8574 => "PCF8574",
        CapsPinKind.AxisGeneric => "Axis",
        _ => "—",
    };

    private static string ModuleName(CapsPinModule module) => module switch
    {
        CapsPinModule.Steering => "Steering",
        CapsPinModule.Accelerator => "Accelerator",
        CapsPinModule.Brake => "Brake",
        CapsPinModule.Clutch => "Clutch",
        CapsPinModule.Handbrake => "Handbrake",
        CapsPinModule.Shifter => "Shifter",
        CapsPinModule.Horn => "Horn",
        CapsPinModule.Buttons => "Buttons",
        CapsPinModule.Encoders => "Encoders",
        CapsPinModule.Led => "Status LED",
        CapsPinModule.Battery => "Battery",
        _ => "Unassigned",
    };

    private static string CapsDetail(CapsPinEntry p)
    {
        var parts = new List<string>();
        if (p.IsAdc) parts.Add("analog");
        if (p.IsInput) parts.Add("input");
        if (p.IsOutput) parts.Add("output");
        if (p.IsPwm) parts.Add("PWM");
        if (p.HasPullup) parts.Add("pull-up");
        return string.Join(", ", parts);
    }

    /// <summary>
    /// Local pin map (Settings → GPIO pins, PinMapStore) ko Setup ki
    /// read-only table mein badlo — board table na mile to yehi dikhti
    /// hai, taake pin change Setup se sync rahe. Board table mile to
    /// wahi jeetti hai (RefreshPinsAsync, oopar).
    /// </summary>
    private List<PinRow> LocalPinRows()
    {
        var pins = new PinMapStore(services.DataDirectory).Load();
        var rows = new List<PinRow>();
        foreach (var function in PinMap.Functions)
        {
            int? pin = pins.TryGetValue(function.Name, out var p) ? p : function.DefaultPin;
            string gpio = pin is null ? "Not assigned" : $"GPIO {pin}";
            string kind = function.Kind switch
            {
                PinFunctionKind.Analog => "ADC (native)",
                PinFunctionKind.Digital => "Digital in",
                PinFunctionKind.StatusLed => "Digital out",
                PinFunctionKind.LedStrip => "LED strip",
                _ => "—",
            };
            string detail = function.Kind == PinFunctionKind.Analog
                ? pin is null ? "Analog input, ADC1" : $"Analog input, ADC1 (GPIO {pin})"
                : function.Kind == PinFunctionKind.LedStrip
                    ? "WS2812 data"
                    : function.Kind == PinFunctionKind.StatusLed
                        ? "Onboard LED"
                        : "Pull-up";
            rows.Add(new PinRow(function.Name, gpio, kind, detail));
        }
        return rows;
    }

    // --- Calibration capture (pehle wala logic) ---

    private bool CanNext() => Step switch
    {
        0 => HasData,
        1 => _leftDone && _rightDone,
        2 => _accelDone && _brakeDone,
        _ => true,
    };

    private async Task Next()
    {
        Message = "";
        switch (Step)
        {
            case 0:
                _center = LiveSteering;
                _sMin = _center;
                _sMax = _center;
                Step = 1;
                break;
            case 1:
                _aMin = LiveAccel; _aMax = LiveAccel;
                _bMin = LiveBrake; _bMax = LiveBrake;
                Step = 2;
                break;
            case 2:
                Step = 3;
                break;
            default:
                await Finish();
                break;
        }
        Refresh();
    }

    private void Back()
    {
        if (Step > 0) Step--;
        Refresh();
    }

    [RelayCommand]
    private void Restart() => ResetCalibration();

    private async Task Finish()
    {
        // V-C2: captured values mV mein save hoti hain (D6/D10) + unit
        // tag. Steering: flag true = pehle se mV, false = raw→mV.
        // Pedals v1 mein hamesha raw hote hain — hamesha raw→mV.
        int steerMv(int v) => _steeringInMillivolts ? v : InputMapper.RawToMillivolts(v);
        var profile = services.ActiveProfile with
        {
            SteeringCalibration = new SteeringCalibration { RawMin = steerMv(_sMin), RawCenter = steerMv(_center), RawMax = steerMv(_sMax), IsSet = true, IsMillivolts = true },
            AccelCalibration = new PedalCalibration { RawMin = InputMapper.RawToMillivolts(_aMin), RawMax = InputMapper.RawToMillivolts(_aMax), IsSet = true, IsMillivolts = true },
            BrakeCalibration = new PedalCalibration { RawMin = InputMapper.RawToMillivolts(_bMin), RawMax = InputMapper.RawToMillivolts(_bMax), IsSet = true, IsMillivolts = true },
        };
        // SETUP-GATES-DECISION.md §2.2: generic message hata kar per-axis
        // hint — kaun sa axis aur kyun (660 mV). Naye gates se yahan
        // pohanchna hi nahi chahiye; ye sirf hifazati jaal hai.
        if (!profile.SteeringCalibration.IsSane)
        {
            Message = SetupGates.CalibrationTooNarrowHint("Steering");
            return;
        }
        if (!profile.AccelCalibration.IsSane)
        {
            Message = SetupGates.CalibrationTooNarrowHint("Accelerator");
            return;
        }
        if (!profile.BrakeCalibration.IsSane)
        {
            Message = SetupGates.CalibrationTooNarrowHint("Brake");
            return;
        }
        // SETUP-GATES-DECISION.md §3.3 (clear path): kamyaab recalibration
        // par stamps lagao — us ke baad CheckRecalibration false deta hai.
        var ctx = CurrentCalibrationContext();
        profile = profile.MarkCalibrated(ctx.PinMapFingerprint, ctx.FirmwareVersion, ctx.NowUtc);
        _promptDismissedFor = RecalibrationReason.None;
        // U-P1: Finish FlowNext command (UI thread) se chalta hai — save
        // background par, UI thread kabhi block nahi hota.
        await services.UpdateActiveProfileAsync(profile);
        RefreshRecalibrationPrompt();
        RequestHome?.Invoke();
    }

    public void Tick()
    {
        if (FlowStep == 0) Connection.Tick();
        if (services.Source.TryGetLatest(out var p, out _, long.MaxValue))
        {
            HasData = true;
            _steeringInMillivolts = p.SteeringInMillivolts;
            LiveSteering = p.Steering;
            LiveAccel = p.Accel;
            LiveBrake = p.Brake;
            if (Step == 1)
            {
                _sMin = Math.Min(_sMin, p.Steering);
                _sMax = Math.Max(_sMax, p.Steering);
            }
            if (Step == 2)
            {
                _aMin = Math.Min(_aMin, p.Accel);
                _aMax = Math.Max(_aMax, p.Accel);
                _bMin = Math.Min(_bMin, p.Brake);
                _bMax = Math.Max(_bMax, p.Brake);
            }
        }
        else HasData = false;
        Refresh();
        RefreshRecalibrationPromptIfChanged();
    }

    private void Refresh()
    {
        var age = services.Source.LastValidAgeMs;
        IsConnected = age is < 2000;
        ConnectStatus = IsConnected
            ? $"Connected — receiving {services.Source.Statistics.CurrentRatePerSecond} packets/sec from the wheel."
            : _connectAttempted
                ? "Still waiting for packets…"
                : "Not connected yet — press Connect, then wait a few seconds.";
        ConnectError = _connectAttempted && !IsConnected
            ? "Could not reach the wheel. Check the wheel is powered and this PC is on the RSH Wi-Fi, then press Retry."
            : "";

        int c = Step == 0 ? LiveSteering : _center;
        int lo = Math.Min(Step == 0 ? c : _sMin, c - 400);
        int hi = Math.Max(Step == 0 ? c : _sMax, c + 400);
        double span = Math.Max(1, hi - lo);
        LivePos = Math.Clamp((LiveSteering - lo) * 100.0 / span, 0, 100);
        MinPos = Math.Clamp((_sMin - lo) * 100.0 / span, 0, 100);
        MaxPos = Math.Clamp((_sMax - lo) * 100.0 / span, 0, 100);

        // Gates mV mein (SETUP-GATES-DECISION.md §2.1): packet units ko
        // pehle mV mein badlo — steering flag ke mutabiq, pedals hamesha
        // raw→mV (v1 mein pedals hamesha raw hote hain). Raw par gate
        // lagana purana mismatch dobara kholta hai.
        int steerMv(int v) => _steeringInMillivolts ? v : InputMapper.RawToMillivolts(v);
        int centerMv = steerMv(_center);
        int leftMv = Math.Max(0, centerMv - steerMv(_sMin));
        int rightMv = Math.Max(0, steerMv(_sMax) - centerMv);
        _leftDone = SetupGates.SteeringSideComplete(leftMv);
        _rightDone = SetupGates.SteeringSideComplete(rightMv);
        RangePercent = (Math.Min(leftMv, SetupGates.MinSteeringSideMillivolts) + Math.Min(rightMv, SetupGates.MinSteeringSideMillivolts)) * 50.0 / SetupGates.MinSteeringSideMillivolts;
        // Centre-off: kul span kaafi hai lekin ek taraf kam — centre
        // ghalat pakra gaya; user wheel seedha pakar kar step dobara kare.
        bool centreOff = leftMv + rightMv >= SetupGates.MinSteeringSpanMillivolts && (!_leftDone || !_rightDone);
        LeftText = _leftDone ? "Left end ✓ captured"
            : centreOff ? SetupGates.CentreOffHint
            : SetupGates.SteeringLeftHint(SetupGates.MinSteeringSideMillivolts - leftMv);
        RightText = _rightDone ? "Right end ✓ captured"
            : centreOff ? ""
            : SetupGates.SteeringRightHint(SetupGates.MinSteeringSideMillivolts - rightMv);

        int accelSpanMv = InputMapper.RawToMillivolts(_aMax) - InputMapper.RawToMillivolts(_aMin);
        int brakeSpanMv = InputMapper.RawToMillivolts(_bMax) - InputMapper.RawToMillivolts(_bMin);
        _accelDone = SetupGates.PedalComplete(accelSpanMv);
        _brakeDone = SetupGates.PedalComplete(brakeSpanMv);
        AccelPercent = Math.Min(100.0, accelSpanMv * 100.0 / SetupGates.MinPedalSpanMillivolts);
        BrakePercent = Math.Min(100.0, brakeSpanMv * 100.0 / SetupGates.MinPedalSpanMillivolts);
        AccelText = _accelDone ? "Accelerator ✓ captured" : SetupGates.PedalHint("accelerator", SetupGates.MinPedalSpanMillivolts - accelSpanMv);
        BrakeText = _brakeDone ? "Brake ✓ captured" : SetupGates.PedalHint("brake", SetupGates.MinPedalSpanMillivolts - brakeSpanMv);

        if (Step == 3)
            Summary = $"Centre {_center}, steering {_sMin} to {_sMax}\nAccelerator {_aMin} to {_aMax}, brake {_bMin} to {_bMax}";

        bool can = CanNext();
        if (can != _lastCanNext)
        {
            _lastCanNext = can;
            OnPropertyChanged(nameof(NextLabel));
        }
        bool canFlow = CanFlowNext();
        if (canFlow != _lastCanFlowNext)
        {
            _lastCanFlowNext = canFlow;
            FlowNextCommand.NotifyCanExecuteChanged();
        }
        bool canBack = CanFlowBack;
        if (canBack != _lastCanFlowBack)
        {
            _lastCanFlowBack = canBack;
            FlowBackCommand.NotifyCanExecuteChanged();
        }
        OnPropertyChanged(nameof(OverallProgress));
        OnPropertyChanged(nameof(IsConnected));
    }
}
