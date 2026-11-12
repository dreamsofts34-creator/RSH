namespace ApexLink.Core.Profiles;

/// <summary>
/// Tayyar presets — industry ka "plug-and-play preset" pattern (Moza /
/// Fanatec). Har preset ek aam profile hai jisme sirf rotation scale
/// (aur naam) ka farq hai; user baad mein apni marzi se badal sakta hai.
/// Calibration user ki apni rehti hai — preset calibration NAHI badalta,
/// sirf scale lagata hai (Profiles page isi liye maujooda calibration
/// par scale copy karke naya profile banata hai).
/// </summary>
public static class ProfilePresets
{
    public static WheelProfile Full900(string name = "Full 900") =>
        WheelProfile.CreateDefault(name) with { SteeringScale = 1.0 };

    public static WheelProfile Gt540(string name = "GT 540") =>
        WheelProfile.CreateDefault(name) with { SteeringScale = 0.6 };

    public static WheelProfile F1_360(string name = "F1 360") =>
        WheelProfile.CreateDefault(name) with { SteeringScale = 0.4 };
}
