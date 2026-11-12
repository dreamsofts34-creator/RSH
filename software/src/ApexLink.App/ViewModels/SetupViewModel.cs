using ApexLink.App.Services;
using ApexLink.Core.Processing;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace ApexLink.App.ViewModels;

/// <summary>Guided 4-step setup: centre, steering range, pedals, save.</summary>
public sealed partial class SetupViewModel(AppServices services) : ObservableObject, ILiveViewModel
{
    private const int SteerSide = 250;   // raw units needed on each side of centre
    private const int PedalSpan = 150;   // raw units of pedal travel needed

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

    private int _center, _sMin, _sMax, _aMin, _aMax, _bMin, _bMax;
    private bool _leftDone, _rightDone, _accelDone, _brakeDone, _lastCanNext;

    public Action? RequestHome { get; set; }

    public bool IsStep0 => Step == 0;
    public bool IsStep1 => Step == 1;
    public bool IsStep2 => Step == 2;
    public bool IsStep3 => Step == 3;
    public bool CanGoBack => Step > 0;
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
    public double OverallProgress => Step * 100.0 / 3.0;
    public string StepCounter => $"Step {Step + 1} of 4";

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

    partial void OnStepChanged(int value)
    {
        OnPropertyChanged(nameof(IsStep0));
        OnPropertyChanged(nameof(IsStep1));
        OnPropertyChanged(nameof(IsStep2));
        OnPropertyChanged(nameof(IsStep3));
        OnPropertyChanged(nameof(CanGoBack));
        OnPropertyChanged(nameof(OverallProgress));
        OnPropertyChanged(nameof(StepCounter));
        OnPropertyChanged(nameof(StepTitle));
        OnPropertyChanged(nameof(StepHint));
        OnPropertyChanged(nameof(NextLabel));
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

    public void Begin()
    {
        _center = _sMin = _sMax = _aMin = _aMax = _bMin = _bMax = 0;
        Message = "";
        Step = 0;
        Refresh();
    }

    private bool CanNext() => Step switch
    {
        0 => HasData,
        1 => _leftDone && _rightDone,
        2 => _accelDone && _brakeDone,
        _ => true,
    };

    [RelayCommand(CanExecute = nameof(CanNext))]
    private void Next()
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
                Finish();
                break;
        }
        Refresh();
    }

    [RelayCommand]
    private void Back()
    {
        if (Step > 0) Step--;
        Refresh();
    }

    [RelayCommand]
    private void Restart() => Begin();

    private void Finish()
    {
        var profile = services.ActiveProfile with
        {
            SteeringCalibration = new SteeringCalibration { RawMin = _sMin, RawCenter = _center, RawMax = _sMax, IsSet = true },
            AccelCalibration = new PedalCalibration { RawMin = _aMin, RawMax = _aMax, IsSet = true },
            BrakeCalibration = new PedalCalibration { RawMin = _bMin, RawMax = _bMax, IsSet = true },
        };
        if (!profile.SteeringCalibration.IsSane || !profile.AccelCalibration.IsSane || !profile.BrakeCalibration.IsSane)
        {
            Message = "Those values don't look right. Press Restart and try again.";
            return;
        }
        services.UpdateActiveProfile(profile);
        RequestHome?.Invoke();
    }

    public void Tick()
    {
        if (services.Source.TryGetLatest(out var p, out _, long.MaxValue))
        {
            HasData = true;
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
    }

    private void Refresh()
    {
        int c = Step == 0 ? LiveSteering : _center;
        int lo = Math.Min(Step == 0 ? c : _sMin, c - 400);
        int hi = Math.Max(Step == 0 ? c : _sMax, c + 400);
        double span = Math.Max(1, hi - lo);
        LivePos = Math.Clamp((LiveSteering - lo) * 100.0 / span, 0, 100);
        MinPos = Math.Clamp((_sMin - lo) * 100.0 / span, 0, 100);
        MaxPos = Math.Clamp((_sMax - lo) * 100.0 / span, 0, 100);

        int left = Math.Max(0, _center - _sMin);
        int right = Math.Max(0, _sMax - _center);
        _leftDone = left >= SteerSide;
        _rightDone = right >= SteerSide;
        RangePercent = (Math.Min(left, SteerSide) + Math.Min(right, SteerSide)) * 50.0 / SteerSide;
        LeftText = _leftDone ? "Left end ✓ captured" : "Left end: turn the wheel left";
        RightText = _rightDone ? "Right end ✓ captured" : "Right end: turn the wheel right";

        _accelDone = _aMax - _aMin >= PedalSpan;
        _brakeDone = _bMax - _bMin >= PedalSpan;
        AccelPercent = Math.Min(100.0, (_aMax - _aMin) * 100.0 / PedalSpan);
        BrakePercent = Math.Min(100.0, (_bMax - _bMin) * 100.0 / PedalSpan);
        AccelText = _accelDone ? "Accelerator ✓ captured" : "Accelerator: press fully down";
        BrakeText = _brakeDone ? "Brake ✓ captured" : "Brake: press fully down";

        if (Step == 3)
            Summary = $"Centre {_center}, steering {_sMin} to {_sMax}\nAccelerator {_aMin} to {_aMax}, brake {_bMin} to {_bMax}";

        bool can = CanNext();
        if (can != _lastCanNext)
        {
            _lastCanNext = can;
            NextCommand.NotifyCanExecuteChanged();
        }
    }
}
