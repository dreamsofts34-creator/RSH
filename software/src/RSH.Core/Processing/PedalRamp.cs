namespace RSH.Core.Processing;

/// <summary>
/// Digital pedal buttons ke liye ramp (docs 02 section 6.3): ek dam
/// on/off ki bajaye aahista charhna/utarna, warna game mein throttle
/// jhatke se lagta hai. Default: pura dabna 150 ms, pura chorna 100 ms.
/// Ek accel aur ek brake — dono ki apni state.
/// </summary>
public sealed class PedalRamp
{
    private readonly double _pressRatePerSec;
    private readonly double _releaseRatePerSec;

    public double Accel { get; private set; }
    public double Brake { get; private set; }

    public PedalRamp(double pressMs = 150, double releaseMs = 100)
    {
        _pressRatePerSec = 1000.0 / Math.Max(1, pressMs);
        _releaseRatePerSec = 1000.0 / Math.Max(1, releaseMs);
    }

    private double Step(double current, bool pressed, double dtSeconds)
    {
        if (dtSeconds <= 0) return current;
        double next = pressed
            ? current + _pressRatePerSec * dtSeconds
            : current - _releaseRatePerSec * dtSeconds;
        return Math.Clamp(next, 0.0, 1.0);
    }

    public void Update(bool accelPressed, bool brakePressed, double dtSeconds)
    {
        Accel = Step(Accel, accelPressed, dtSeconds);
        Brake = Step(Brake, brakePressed, dtSeconds);
    }

    public void Reset()
    {
        Accel = 0;
        Brake = 0;
    }
}
