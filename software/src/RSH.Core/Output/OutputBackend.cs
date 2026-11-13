namespace RSH.Core.Output;

/// <summary>
/// Output backend ka intekhab: vJoy (default, v2.0.0) ya HIDMaestro
/// (Phase H experimental). Selector ka ek hi usool hai — HIDMaestro sirf
/// tab jab user ne backend bhi HIDMaestro chuna ho AUR experimental
/// toggle bhi on ho. Koi bhi aur halat (toggle band, ghalat string,
/// khaali) fail-closed vJoy par girti hai. Ye faisla settings ki
/// Sanitize mein bhi dohraya jata hai (AppSettings) — yahan defense
/// in depth hai, aur yahi unit-test hota hai.
/// </summary>
public enum OutputBackendKind
{
    VJoy,
    HidMaestro,
}

public static class OutputBackendSelector
{
    public const string VJoyName = "VJoy";
    public const string HidMaestroName = "HIDMaestro";

    public static OutputBackendKind Parse(string? value) =>
        string.Equals(value, HidMaestroName, StringComparison.OrdinalIgnoreCase)
            ? OutputBackendKind.HidMaestro
            : OutputBackendKind.VJoy;

    public static string ToSettingsString(OutputBackendKind kind) =>
        kind == OutputBackendKind.HidMaestro ? HidMaestroName : VJoyName;

    /// <summary>
    /// HIDMaestro sirf jab requested bhi ho aur experimental toggle bhi
    /// on ho — warna hamesha vJoy. Fail-closed: koi exception nahi,
    /// koi aadhi halat nahi.
    /// </summary>
    public static OutputBackendKind Resolve(OutputBackendKind requested, bool experimentalEnabled) =>
        requested == OutputBackendKind.HidMaestro && experimentalEnabled
            ? OutputBackendKind.HidMaestro
            : OutputBackendKind.VJoy;

    /// <summary>
    /// Chuna hua backend banao. Factories tab hi bulayi jati hain jab
    /// un ki zaroorat ho (lazy) — vJoy factory HIDMaestro chunte waqt
    /// nahi chalti aur vice versa.
    /// </summary>
    public static IJoystickOutput Create(
        OutputBackendKind requested,
        bool experimentalEnabled,
        Func<IJoystickOutput> vjoyFactory,
        Func<IJoystickOutput> hidMaestroFactory)
    {
        ArgumentNullException.ThrowIfNull(vjoyFactory);
        ArgumentNullException.ThrowIfNull(hidMaestroFactory);
        return Resolve(requested, experimentalEnabled) == OutputBackendKind.HidMaestro
            ? hidMaestroFactory()
            : vjoyFactory();
    }
}
