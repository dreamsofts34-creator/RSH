using RSH.Core.Pipeline;

namespace RSH.Core.Profiles;

/// <summary>
/// Profile v2 (SW-12) mein per-module tuning ka ek entry — key module ka
/// naam hai (misal "Steering"). Slots <see cref="RSH.Core.Pipeline.ModuleTuning"/>
/// wale hi hain + on/off. Khali/missing dict = legacy v1 profile
/// (tuning v1 fields — Deadzone/Curve/Smoothing/Invert — se banti hai).
/// </summary>
public sealed record PerModuleTuning
{
    /// <summary>Center deadzone 0..0.5.</summary>
    public double Deadzone { get; init; }

    /// <summary>Curve strength -1..1 (0 = linear).</summary>
    public double CurveStrength { get; init; }

    /// <summary>Smoothing 0..1 (0 = off).</summary>
    public double Smoothing { get; init; }

    /// <summary>Input ulta karo.</summary>
    public bool Invert { get; init; }

    /// <summary>Output scale 0.2..1.0 (1.0 = poori range).</summary>
    public double Scale { get; init; } = 1.0;

    /// <summary>Module on hai? Band module ka data pipeline se nahi guzarta.</summary>
    public bool Enabled { get; init; } = true;

    /// <summary>
    /// Shifter gate mode (Phase F WS5) — sirf Shifter module ke liye
    /// maani rakhta hai; baqi modules default Sequential rakhte hain.
    /// </summary>
    public ShifterGateMode GateMode { get; init; } = ShifterGateMode.Sequential;

    /// <summary>Hadd se bahar qeematein clamp karo (load par — file kabhi nahi marti).</summary>
    public PerModuleTuning Sanitized() => this with
    {
        Deadzone = Math.Clamp(Deadzone, 0, 0.5),
        CurveStrength = Math.Clamp(CurveStrength, -1, 1),
        Smoothing = Math.Clamp(Smoothing, 0, 1),
        Scale = double.IsNaN(Scale) ? 1.0 : Math.Clamp(Scale, 0.2, 1.0),
        GateMode = Enum.IsDefined(GateMode) ? GateMode : ShifterGateMode.Sequential,
    };
}
