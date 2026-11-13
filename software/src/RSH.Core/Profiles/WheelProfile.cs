using RSH.Core.Processing;

namespace RSH.Core.Profiles;

/// <summary>
/// Ek mukammal profile: calibration + processing settings + button map.
/// JSON mein save hota hai (ProfileStore). Har qeemat ki sane hadain
/// Clamp/validate ProfileStore ke load par bhi laagu hoti hain — kharab
/// ya haath se badli hui file app ko kabhi ajeeb halat mein nahi dalti.
/// </summary>
public sealed record WheelProfile
{
    public string Name { get; init; } = "Default";

    public SteeringCalibration SteeringCalibration { get; init; } = new();
    public PedalCalibration AccelCalibration { get; init; } = new();
    public PedalCalibration BrakeCalibration { get; init; } = new();

    /// <summary>Center deadzone 0..0.5 (steering).</summary>
    public double Deadzone { get; init; } = 0.02;

    /// <summary>Curve strength -1..1 (0 = linear).</summary>
    public double CurveStrength { get; init; }

    /// <summary>Smoothing 0..1 (0 = off). Pipeline ise One Euro mincutoff mein badalta hai.</summary>
    public double Smoothing { get; init; } = 0.3;

    public bool InvertSteering { get; init; }
    public bool InvertAccel { get; init; }
    public bool InvertBrake { get; init; }

    /// <summary>Dono pedals ek hi axis par (purane games ke liye).</summary>
    public bool CombinePedals { get; init; }

    /// <summary>vJoy button numbers (1-based) jin par gear buttons map hote hain.</summary>
    public int GearUpVJoyButton { get; init; } = 1;
    public int GearDownVJoyButton { get; init; } = 2;

    /// <summary>Digital pedal ramp ke auqaat (ms).</summary>
    public double PedalPressMs { get; init; } = 150;
    public double PedalReleaseMs { get; init; } = 100;

    /// <summary>
    /// Output rotation scale (industry ke "rotation presets" ka input-only
    /// tarjuma): 1.0 = poora 900°, 0.6 = 540°, 0.4 = 360°. Motor nahi hai
    /// to soft-lock mehsoos nahi hota — ye output ko utni hadd par clamp/
    /// compress karta hai jitni game umeed karti hai.
    /// </summary>
    public double SteeringScale { get; init; } = 1.0;

    /// <summary>
    /// Jis game ke process ke samne aane par ye profile khud load ho
    /// (masalan "AssettoCorsa"). Khaali ho to auto-switch nahi hota.
    /// </summary>
    public string GameProcessName { get; init; } = "";

    /// <summary>Har qeemat ko us ki mehfooz hadd mein rakhta hai — load ke baad lazmi call hota hai.</summary>
    public WheelProfile Sanitized()
    {
        int up = Math.Clamp(GearUpVJoyButton, 1, 128);
        int down = Math.Clamp(GearDownVJoyButton, 1, 128);
        // AUDIT FIX: gear up/down ek hi vJoy button par hon to game mein
        // dono gear ek saath girte thay — down ko chup chaap agle number
        // par hata do (128 par ho to pichle par).
        if (down == up) down = up < 128 ? up + 1 : up - 1;
        return this with
        {
            Name = string.IsNullOrWhiteSpace(Name) ? "Default" : Name.Trim(),
            Deadzone = Math.Clamp(Deadzone, 0, 0.5),
            CurveStrength = Math.Clamp(CurveStrength, -1, 1),
            Smoothing = Math.Clamp(Smoothing, 0, 1),
            GearUpVJoyButton = up,
            GearDownVJoyButton = down,
            PedalPressMs = Math.Clamp(PedalPressMs, 10, 2000),
            PedalReleaseMs = Math.Clamp(PedalReleaseMs, 10, 2000),
            SteeringScale = Math.Clamp(SteeringScale, 0.2, 1.0),
            GameProcessName = (GameProcessName ?? "").Trim().Replace(".exe", "", StringComparison.OrdinalIgnoreCase),
        };
    }

    public static WheelProfile CreateDefault(string name = "Default") => new() { Name = name };
}
