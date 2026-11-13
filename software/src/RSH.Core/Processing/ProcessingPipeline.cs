using RSH.Core.Output;
using RSH.Core.Profiles;
using RSH.Core.Protocol;

namespace RSH.Core.Processing;

/// <summary>
/// Poora processing silsila ek packet par (docs 02 section 6.2):
/// map → deadzone → curve → One Euro smoothing → pedal ramp fusion →
/// vJoy axis values. 200 Hz par pipeline thread se bulaya jata hai.
///
/// Smoothing ka tarjuma (profile 0..1 → One Euro mincutoff):
/// 0 = filter band; warna mincutoff 25 Hz (halki) se 1.5 Hz (sakht) tak
/// linear. Beta 0.1 (author ki misaal), dcutoff 1.0, freq 200.
/// Ye numbers asal wheel par tune honge — yahan "research defaults" hain,
/// koi jadooi dawa nahi.
/// </summary>
public sealed class ProcessingPipeline
{
    public const double SampleFrequencyHz = 200.0;

    // AUDIT FIX: SetProfile UI thread se aur Process engine thread se
    // chalta hai — baghair lock ke profile/filter ka adla-badli ek tick ke
    // liye purane filter + nayi profile ka jor bana sakta tha. Ab tamam
    // access ek hi lock se guzarta hai (200 Hz par lock sasta hai).
    private readonly object _gate = new();
    private WheelProfile _profile;
    private OneEuroFilter? _steerFilter;
    private OneEuroFilter? _accelFilter;
    private OneEuroFilter? _brakeFilter;
    private PedalRamp _ramp;

    public ProcessingPipeline(WheelProfile profile)
    {
        _profile = profile.Sanitized();
        _ramp = new PedalRamp(_profile.PedalPressMs, _profile.PedalReleaseMs);
        RebuildFilters();
    }

    public WheelProfile Profile { get { lock (_gate) return _profile; } }

    /// <summary>
    /// Profile/settings badle jayen to filter aur ramp saaf halat se shuru
    /// hote hain — lekin AUDIT improvement: jo cheez nahi badli, us ki
    /// state bhi reset nahi hoti (masalan sirf button mapping badalne par
    /// smoothing ki history mehfooz rehti hai).
    /// </summary>
    public void SetProfile(WheelProfile profile)
    {
        lock (_gate)
        {
            var next = profile.Sanitized();
            if (next.PedalPressMs != _profile.PedalPressMs || next.PedalReleaseMs != _profile.PedalReleaseMs)
                _ramp = new PedalRamp(next.PedalPressMs, next.PedalReleaseMs);
            bool smoothingChanged = next.Smoothing != _profile.Smoothing;
            _profile = next;
            if (smoothingChanged) RebuildFilters();
        }
    }

    private void RebuildFilters()
    {
        double s = _profile.Smoothing;
        if (s <= 0)
        {
            _steerFilter = null;
            _accelFilter = null;
            _brakeFilter = null;
        }
        else
        {
            double minCutoff = 25.0 - 23.5 * s;
            _steerFilter = new OneEuroFilter(SampleFrequencyHz, minCutoff, beta: 0.1);
            _accelFilter = new OneEuroFilter(SampleFrequencyHz, minCutoff, beta: 0.1);
            _brakeFilter = new OneEuroFilter(SampleFrequencyHz, minCutoff, beta: 0.1);
        }
    }

    public ProcessedState Process(in WheelPacket packet, double dtSeconds)
    {
        lock (_gate)
        {
        var p = _profile;

        // --- Steering ---
        double steer = InputMapper.MapSteering(packet.Steering, p.SteeringCalibration, packet.SteeringInMillivolts, p.InvertSteering);
        steer = InputMapper.ApplyDeadzone(steer, p.Deadzone);
        steer = InputMapper.ApplyCurve(steer, p.CurveStrength);
        steer = Math.Clamp(steer * p.SteeringScale, -1, 1);   // rotation preset (output soft-lock)
        if (_steerFilter is not null) steer = Math.Clamp(_steerFilter.Filter(steer, dtSeconds), -1, 1);

        // --- Pedals: analog aur digital (ramp) mein se jo zyada ho ---
        _ramp.Update(packet.AccelButton, packet.BrakeButton, dtSeconds);
        double accel = Math.Max(
            InputMapper.MapPedal(packet.Accel, p.AccelCalibration, p.InvertAccel),
            _ramp.Accel);
        double brake = Math.Max(
            InputMapper.MapPedal(packet.Brake, p.BrakeCalibration, p.InvertBrake),
            _ramp.Brake);
        if (_accelFilter is not null) accel = Math.Clamp(_accelFilter.Filter(accel, dtSeconds), 0, 1);
        if (_brakeFilter is not null) brake = Math.Clamp(_brakeFilter.Filter(brake, dtSeconds), 0, 1);

        // --- Buttons ---
        uint mask = 0;
        if (packet.GearUp) mask |= 1u << (p.GearUpVJoyButton - 1);
        if (packet.GearDown) mask |= 1u << (p.GearDownVJoyButton - 1);

        // --- Axes ---
        int accelAxis, brakeAxis;
        if (p.CombinePedals)
        {
            accelAxis = InputMapper.CombinedPedalsToAxis(accel, brake);
            brakeAxis = InputMapper.AxisMin;
        }
        else
        {
            accelAxis = InputMapper.PedalToAxis(accel);
            brakeAxis = InputMapper.PedalToAxis(brake);
        }

        return new ProcessedState(
            SteeringAxis: InputMapper.SteeringToAxis(steer),
            AccelAxis: accelAxis,
            BrakeAxis: brakeAxis,
            ButtonsMask: mask,
            SteeringNorm: steer,
            AccelNorm: accel,
            BrakeNorm: brake);
        }
    }

    /// <summary>Filters/ramp ko saaf karo (reconnect ya calibration ke baad).</summary>
    public void Reset()
    {
        lock (_gate)
        {
            _steerFilter?.Reset();
            _accelFilter?.Reset();
            _brakeFilter?.Reset();
            _ramp.Reset();
        }
    }
}
