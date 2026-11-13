namespace RSH.Core.Processing;

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
    /// Raw ADC counts (12-bit, 0..4095) → milliVolts (0..3300 mV).
    /// ADC reference 3300 mV hai (D10) — is liye raw * 3300 / 4095,
    /// round karke. v1 packets mein pedals hamesha raw hote hain aur
    /// steering raw hota hai jab packet flag set na ho; calibration
    /// hamesha mV mein hoti hai (D6/D10), is liye mapper pehle value
    /// ko mV mein normalize karta hai, phir sane cal ya defaults lagte hain.
    /// </summary>
    public static int RawToMillivolts(int raw) =>
        (int)Math.Round(Math.Clamp(raw, 0, 4095) * 3300.0 / 4095.0);

    /// <summary>
    /// Steering raw/mV → -1..1, center ke dono taraf alag scale (piecewise),
    /// taake center hamesha bilkul 0 aye chahe min/max ghair-mutawazan hon.
    /// V-C2: value pehle mV mein normalize hoti hai
    /// (<paramref name="inMillivolts"/> false ho to
    /// <see cref="RawToMillivolts"/>), phir sane mV cal ya defaults —
    /// mV mode defaults 900/2050/3200, raw mode defaults 0/1650/3300
    /// (purane raw defaults 0/2048/4095 ka mV conversion — purana
    /// behaviour mehfooz: raw 2048 → 1650 mV = center = 0).
    /// </summary>
    public static double MapSteering(int raw, SteeringCalibration cal, bool inMillivolts, bool invert)
    {
        double value = inMillivolts ? raw : RawToMillivolts(raw);
        double min, center, max;
        if (cal.IsSane)
        {
            min = cal.RawMin; center = cal.RawCenter; max = cal.RawMax;
        }
        else if (inMillivolts) { min = 900; center = 2050; max = 3200; }
        else { min = 0; center = 1650; max = 3300; }

        double x = value <= center
            ? -(center - value) / Math.Max(1, center - min)
            : (value - center) / Math.Max(1, max - center);
        x = Math.Clamp(x, -1.0, 1.0);
        return invert ? -x : x;
    }

    /// <summary>
    /// Pedal raw/mV → 0..1 (min..max ke darmiyan clamp). V-C2:
    /// <paramref name="inMillivolts"/> false (default — v1 pedals
    /// hamesha raw 0..4095) ho to value pehle
    /// <see cref="RawToMillivolts"/> se mV mein badalti hai; v2 stream
    /// path baad mein true pass karega (Phase D). Sane cal hamesha mV
    /// mein hoti hai; defaults 0..3300 mV (purane 0..4095 raw ke
    /// barabar — behaviour mehfooz, ratio wahi rehta hai).
    /// </summary>
    public static double MapPedal(int raw, PedalCalibration cal, bool invert, bool inMillivolts = false)
    {
        double value = inMillivolts ? raw : RawToMillivolts(raw);
        double min = cal.IsSane ? cal.RawMin : 0;
        double max = cal.IsSane ? cal.RawMax : 3300;
        double x = Math.Clamp((value - min) / Math.Max(1, max - min), 0.0, 1.0);
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
