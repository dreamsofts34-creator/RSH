namespace ApexLink.Core.Processing;

/// <summary>
/// Khatti (pure) mapping mathematics — koi state nahi, har function
/// akela test ho sakta hai. Tamam normalized values double mein hain:
/// steering -1..1, pedals 0..1.
/// </summary>
public static class InputMapper
{
    /// <summary>vJoy axis ki hadain (research-verified: 1..32768, center 16384).</summary>
    public const int AxisMin = 1;
    public const int AxisMax = 32768;
    public const int AxisCenter = 16384;

    /// <summary>
    /// Steering raw → -1..1, center ke dono taraf alag scale (piecewise),
    /// taake center hamesha bilkul 0 aye chahe min/max ghair-mutawazan hon.
    /// </summary>
    public static double MapSteering(int raw, SteeringCalibration cal, bool inMillivolts, bool invert)
    {
        double min, center, max;
        if (cal.IsSane)
        {
            min = cal.RawMin; center = cal.RawCenter; max = cal.RawMax;
        }
        else if (inMillivolts) { min = 900; center = 2050; max = 3200; }
        else { min = 0; center = 2048; max = 4095; }

        double x = raw <= center
            ? -(center - raw) / Math.Max(1, center - min)
            : (raw - center) / Math.Max(1, max - center);
        x = Math.Clamp(x, -1.0, 1.0);
        return invert ? -x : x;
    }

    /// <summary>Pedal raw → 0..1 (min..max ke darmiyan clamp).</summary>
    public static double MapPedal(int raw, PedalCalibration cal, bool invert)
    {
        double min = cal.IsSane ? cal.RawMin : 0;
        double max = cal.IsSane ? cal.RawMax : 4095;
        double x = Math.Clamp((raw - min) / Math.Max(1, max - min), 0.0, 1.0);
        return invert ? 1.0 - x : x;
    }

    /// <summary>
    /// Center deadzone (steering ke liye): deadzone ke andar 0, bahir
    /// bachi hui range par dobara scale — kinare hamesha pura ±1 rehta hai.
    /// </summary>
    public static double ApplyDeadzone(double x, double deadzone)
    {
        if (deadzone <= 0) return x;
        double dz = Math.Min(deadzone, 0.95);
        double ax = Math.Abs(x);
        if (ax <= dz) return 0.0;
        return Math.Sign(x) * (ax - dz) / (1.0 - dz);
    }

    /// <summary>
    /// Response curve (docs 02 section 6.3 ka formula):
    /// strength 0 = linear; +1 tak = soft center (cubic ki taraf);
    /// -1 tak = aggressive center (sqrt ki taraf). Kinare 0/±1 hamesha sabit.
    /// </summary>
    public static double ApplyCurve(double x, double strength)
    {
        double s = Math.Clamp(strength, -1.0, 1.0);
        if (s == 0) return x;
        double ax = Math.Abs(x);
        double y = s > 0
            ? (1 - s) * ax + s * ax * ax * ax
            : (1 + s) * ax - s * Math.Sqrt(ax);   // s manfi hai: (1-|s|)*ax + |s|*sqrt(ax)
        return Math.Sign(x) * Math.Clamp(y, 0.0, 1.0);
    }

    /// <summary>Steering -1..1 → vJoy axis 1..32768 (center bilkul 16384).</summary>
    public static int SteeringToAxis(double x) =>
        Math.Clamp(AxisCenter + (int)Math.Round(Math.Clamp(x, -1, 1) * AxisCenter), AxisMin, AxisMax);

    /// <summary>Pedal 0..1 → vJoy axis 1..32768.</summary>
    public static int PedalToAxis(double v) =>
        Math.Clamp(AxisMin + (int)Math.Round(Math.Clamp(v, 0, 1) * (AxisMax - AxisMin)), AxisMin, AxisMax);

    /// <summary>
    /// Combined pedals: ek hi axis par accel oopar ki taraf, brake neeche
    /// (docs: purane games ke liye). Dono ek sath dabain to farq jata hai.
    /// </summary>
    public static int CombinedPedalsToAxis(double accel, double brake) =>
        SteeringToAxis(Math.Clamp(accel - brake, -1, 1));
}
