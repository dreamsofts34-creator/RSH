using HIDMaestro;

namespace RSH.Output.HIDMaestro;

/// <summary>
/// Asal HIDMaestro SDK binding — compile-time reference, vendored
/// HIDMaestro.Core.dll v2.1.0 (lib/). SDK surface SDK Reference se
/// verify hua hai (hidmaestro.org/docs/sdk/sdk-reference/, 2026-11-13):
/// <c>HMContext</c> (process entry point), <c>InstallDriver()</c>,
/// <c>LoadDefaultProfiles()</c>, <c>GetProfile(id)</c>,
/// <c>CreateController(profile, identityKey)</c>,
/// <c>HMController.SubmitState(in HMGamepadState)</c> /
/// <c>SubmitRawReport(ReadOnlySpan&lt;byte&gt;)</c>.
///
/// SEC-H-06 hard rules (yahan enforce hote hain, sirf doc mein nahi):
/// - Composite/usbip profile (<c>RequiresUsbipBackend</c> ya Backend =
///   "usbip") kabhi create nahi hota — bundled usbip-win2 kernel
///   transport RSH mein kabhi enable nahi hoga.
/// - <c>BuildsItsOwnReports</c> wala profile bhi refuse hota hai (us ka
///   state block hamari report se nahi milta).
/// - Driver install + device create admin mangta hai — na mile to wazeh
///   error, crash nahi (UnauthorizedAccessException → insani wajah).
/// - Pinned version v2.1.0 hi — DLL ka hash test mein verify hota hai.
///
/// Windows-only: Linux/macOS par TryCreateDevice foran false deta hai
/// (HMContext banane se pehle hi) — is liye Linux sandbox mein ye class
/// kabhi driver ko chhooti hi nahi.
/// </summary>
public sealed class HidMaestroSdkAdapter : IHidMaestroSdk
{
    private readonly string _profileId;
    private HMContext? _context;
    private HMController? _controller;

    public HidMaestroSdkAdapter(string profileId)
    {
        _profileId = profileId ?? "";
    }

    public string ProfileId => _profileId;
    public string? SdkVersion => typeof(HMContext).Assembly.GetName().Version?.ToString();
    public string? DriverVersionText => _controller?.DriverVersion?.ToString();

    public HidMaestroCapabilities Capabilities =>
        _controller?.Profile is { } profile
            ? HidMaestroCapabilities.FromProfile(profile)
            : HidMaestroCapabilities.Unknown;

    /// <summary>
    /// Profile create karne se pehle ki jaanch — pure logic, is liye
    /// test seedha asal v2.1.0 catalog ke profile par chala sakta hai
    /// (baghair device create kiye). SEC-H-06: usbip/composite aur
    /// builds-its-own-reports refuse; deployable na ho to bhi refuse.
    /// </summary>
    public static string? ValidateProfile(HMProfile profile)
    {
        ArgumentNullException.ThrowIfNull(profile);
        if (profile.RequiresUsbipBackend ||
            string.Equals(profile.Backend, HidMaestroConstants.UsbipBackendName, StringComparison.OrdinalIgnoreCase))
            return $"Profile '{profile.Id}' composite/usbip hai — bundled usbip-win2 kernel transport RSH mein kabhi enable nahi hota (SEC-H-06).";
        if (profile.BuildsItsOwnReports)
            return $"Profile '{profile.Id}' apni reports khud banata hai — RSH ki report is se nahi milti.";
        if (!profile.IsDeployable)
            return $"Profile '{profile.Id}' deployable nahi hai.";
        return null;
    }

    public bool TryCreateDevice(out string error)
    {
        error = "";
        if (_controller is not null) return true;

        if (!OperatingSystem.IsWindows())
        {
            error = "HIDMaestro driver sirf Windows par chalta hai — is machine (Linux/macOS) par HIDMaestro backend nahi chal sakta. vJoy backend istemal karo.";
            return false;
        }
        if (string.IsNullOrWhiteSpace(_profileId))
        {
            error = "Koi HIDMaestro profile nahi chuni gayi — pehle Output settings mein HIDMaestro profile id darj karo (Windows PC par gate H-HID-2 se tay hogi; kisi brand ki identity default nahi).";
            return false;
        }

        try
        {
            _context = new HMContext();
            if (!_context.IsDriverInstalled)
                _context.InstallDriver();   // admin lazmi — na ho to UnauthorizedAccessException
            _context.LoadDefaultProfiles();
            var profile = _context.GetProfile(_profileId);
            if (profile is null)
            {
                error = $"HIDMaestro profile '{_profileId}' catalog mein nahi mili (v{HidMaestroConstants.PinnedVersion}).";
                DestroyDevice();
                return false;
            }
            string? refusal = ValidateProfile(profile);
            if (refusal is not null)
            {
                error = refusal;
                DestroyDevice();
                return false;
            }
            _controller = _context.CreateController(profile, HidMaestroConstants.IdentityKey);
            return true;
        }
        catch (UnauthorizedAccessException)
        {
            DestroyDevice();
            error = "HIDMaestro device banane ke liye administrator privileges chahiye — app ko admin ke saath chalao, ya pehle driver install step (admin) poora karo.";
            return false;
        }
        catch (Exception ex) when (ex is InvalidOperationException or HMPayloadMissingException or HMProfileException)
        {
            DestroyDevice();
            error = $"HIDMaestro device nahi ban saka ({ex.GetType().Name}): {ex.Message}";
            return false;
        }
    }

    public void SubmitState(in HMGamepadState state)
    {
        if (_controller is null) throw new InvalidOperationException("HIDMaestro device connected nahi hai.");
        _controller.SubmitState(state);
    }

    public void SubmitRawReport(ReadOnlySpan<byte> report)
    {
        if (_controller is null) throw new InvalidOperationException("HIDMaestro device connected nahi hai.");
        _controller.SubmitRawReport(report);
    }

    public void DestroyDevice()
    {
        // Idempotent: null karne ke baad dobara call kuch nahi karti —
        // is liye TryCreateDevice ke baad dobara DestroyDevice bhi mehfooz hai.
        try { _controller?.Dispose(); } catch { /* safai best-effort */ }
        _controller = null;
        try { _context?.Dispose(); } catch { /* safai best-effort */ }
        _context = null;
    }

    public void Dispose() => DestroyDevice();
}
