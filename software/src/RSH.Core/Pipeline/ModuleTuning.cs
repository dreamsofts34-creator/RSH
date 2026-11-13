namespace RSH.Core.Pipeline;

/// <summary>
/// Ek module ki tuning slots. Haddain WheelProfile / InputMapper ki
/// mojooda haddoon se li gayi hain (deadzone 0..0.5, curve -1..1,
/// smoothing 0..1, scale 0.2..1.0) taake skeleton aur pipeline ek hi
/// zuban bolein. Discrete modules (Buttons, Encoders, Horn, Shifter,
/// LED) ke liye analog slots (deadzone/curve/smoothing) ka koi matlab
/// nahi — un ke liye defaults rakho; Phase C wiring mein ye slots
/// ignore honge, reject nahi (sirf hadd se bahar qeemat reject hoti hai).
/// </summary>
public sealed record ModuleTuning
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

    /// <summary>
    /// Shifter gate mode (Phase F WS5). Sirf Shifter module ke liye
    /// maani rakhta hai — baqi modules isay ignore karte hain (lekin
    /// ghalat enum value phir bhi validate hoti hai). Default Sequential.
    /// </summary>
    public ShifterGateMode GateMode { get; init; } = ShifterGateMode.Sequential;

    public static ModuleTuning Default { get; } = new();

    public bool IsDefault => Deadzone == 0 && CurveStrength == 0
        && Smoothing == 0 && !Invert && Scale == 1.0
        && GateMode == ShifterGateMode.Sequential;

    /// <summary>Hadd se bahar qeematon ki list; khaali = valid.</summary>
    public IEnumerable<string> Validate()
    {
        if (double.IsNaN(Deadzone) || Deadzone < 0 || Deadzone > 0.5)
            yield return $"Deadzone {Deadzone} hadd se bahar hai (0..0.5).";
        if (double.IsNaN(CurveStrength) || CurveStrength < -1 || CurveStrength > 1)
            yield return $"CurveStrength {CurveStrength} hadd se bahar hai (-1..1).";
        if (double.IsNaN(Smoothing) || Smoothing < 0 || Smoothing > 1)
            yield return $"Smoothing {Smoothing} hadd se bahar hai (0..1).";
        if (double.IsNaN(Scale) || Scale < 0.2 || Scale > 1.0)
            yield return $"Scale {Scale} hadd se bahar hai (0.2..1.0).";
        if (!Enum.IsDefined(GateMode))
            yield return $"GateMode {(int)GateMode} ghalat hai (Sequential ya HPattern).";
    }
}
