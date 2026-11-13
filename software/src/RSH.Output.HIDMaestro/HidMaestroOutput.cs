using HIDMaestro;
using RSH.Core.Output;
using RSH.Core.Pipeline;
using RSH.Core.Processing;

namespace RSH.Output.HIDMaestro;

/// <summary>
/// HIDMaestro backend — <see cref="IJoystickOutput"/> ki parallel
/// implementation (vJoy ke saath, default vJoy). vJoy ka koi behaviour
/// nahi badla — ye class vJoy ko chhooti hi nahi.
///
/// Raste:
/// - Default: <c>HMGamepadState</c> → <c>HMController.SubmitState</c>
///   (axes floats, buttons 1..25 — baaqi buttons is raste mein nahi aate).
/// - <paramref name="useRawReport"/>: 26-byte raw report →
///   <c>SubmitRawReport</c> (custom RSH wheel profile ke liye — profile
///   abhi Windows-PC follow-up hai, gate H-HID-2).
///
/// Lifecycle vJoy jaisa hai: TryConnect false par wazeh wajah (kabhi
/// throw nahi); Update connected na ho to InvalidOperationException;
/// Disconnect/Dispose idempotent; SendNeutral best-effort (watchdog
/// path par kabhi throw nahi — VJoyOutput se zyada mehfooz, engine ke
/// Stop/neutral path ke liye).
///
/// Linux sandbox mein TryConnect hamesha false deta hai (adapter ka
/// Windows guard) — driver load ka daawa yahan nahi kiya jata; live
/// driver sirf Windows PC par verify hoga (gate H-HID-1).
/// </summary>
public sealed class HidMaestroOutput : IJoystickOutput
{
    private readonly string _profileId;
    private readonly bool _useRawReport;
    private readonly IHidMaestroSdk? _sdkOverride;
    private IHidMaestroSdk? _sdk;

    private bool _connected;
    private bool _capsKnown;
    private bool _deviceReleased; // DestroyDevice ek device par ek dafa — dobara Disconnect/Dispose dobara destroy nahi karta
    private HidMaestroCapabilities _caps = HidMaestroCapabilities.Unknown;

    // 200 Hz hot path: axes dictionary ek dafa banti hai, har tick dobara
    // bhari jati hai (per-tick allocation nahi — SDK doc ka mashwara).
    private readonly Dictionary<HMAxis, float> _axes = new(5);
    private readonly byte[] _rawReport = new byte[HidMaestroMapping.RawReportSize];

    /// <summary>
    /// <paramref name="profileId"/>: HIDMaestro catalog profile id
    /// (masalan "logitech-g29") ya custom profile id — khaali ho to
    /// TryConnect refuse karta hai (koi brand identity default nahi).
    /// <paramref name="sdk"/>: test seam — production mein null (asal
    /// adapter TryConnect par banta hai). <paramref name="useRawReport"/>:
    /// SubmitState ke bajaye 26-byte raw report bhejo (custom profile).
    /// </summary>
    public HidMaestroOutput(string profileId = "", IHidMaestroSdk? sdk = null, bool useRawReport = false)
    {
        _profileId = profileId ?? "";
        _sdkOverride = sdk;
        _useRawReport = useRawReport;
    }

    public string Name => "HIDMaestro";
    public bool IsConnected => _connected;
    public string ProfileId => _profileId;
    public bool UseRawReport => _useRawReport;

    /// <summary>SDK version (pinned v2.1.0 — adapter se, ya override se).</summary>
    public string? SdkVersion => ActiveSdk?.SdkVersion;

    /// <summary>Driver version (device create ke baad; pehle null).</summary>
    public string? DriverVersionText => ActiveSdk?.DriverVersionText;

    public int ButtonCount => _caps.ButtonCount;
    public bool HasAxisX => _caps.HasAxisX;
    public bool HasAxisY => _caps.HasAxisY;
    public bool HasAxisZ => _caps.HasAxisZ;
    public bool HasAxisRx => _caps.HasAxisRx;
    public bool HasAxisSlider => _caps.HasAxisSlider;

    private IHidMaestroSdk? ActiveSdk => _sdkOverride ?? _sdk;

    /// <summary>Is device ki salahiyatein (TryConnect ke baad sahi hoti hain).</summary>
    public HidMaestroCapabilities Capabilities => _caps;

    /// <summary>
    /// P12 (D11) jaisi validation — vJoy wali hi: sirf enabled modules
    /// ki zarooratein lagti hain. Steering-only wheel valid hai.
    /// Clutch ka koi caps flag nahi (Rx hamesha likha jata hai, profile
    /// mein na ho to SDK ignore karta hai — vJoy WClutch jaisa).
    /// </summary>
    public static IReadOnlyList<string> ValidateCapabilities(PipelinePlan plan, HidMaestroCapabilities caps)
    {
        ArgumentNullException.ThrowIfNull(plan);
        var errors = new List<string>();
        if (caps.ButtonCount is < 0 or > HidMaestroCapabilities.MaxButtons)
            errors.Add($"ButtonCount {caps.ButtonCount} hadd se bahar hai (0..{HidMaestroCapabilities.MaxButtons}).");
        if (plan.IsEnabled(ModuleKind.Steering) && !caps.HasAxisX)
            errors.Add("Steering module on hai lekin HIDMaestro profile mein X axis nahi hai.");
        if (plan.IsEnabled(ModuleKind.Accelerator) && !caps.HasAxisY)
            errors.Add("Accelerator module on hai lekin HIDMaestro profile mein Y axis nahi hai.");
        if (plan.IsEnabled(ModuleKind.Brake) && !caps.HasAxisZ)
            errors.Add("Brake module on hai lekin HIDMaestro profile mein Z axis nahi hai.");
        if (plan.IsEnabled(ModuleKind.Handbrake) && !caps.HasAxisSlider)
            errors.Add("Handbrake module on hai lekin HIDMaestro profile mein Slider axis nahi hai.");
        if (plan.IsEnabled(ModuleKind.Buttons) && caps.ButtonCount < 1)
            errors.Add("Buttons module on hai lekin HIDMaestro profile mein koi button nahi hai.");
        return errors;
    }

    public IReadOnlyList<string> ValidateCurrentCapabilities(PipelinePlan plan) =>
        ValidateCapabilities(plan, _caps);

    public bool TryConnect(out string error) => TryConnect(out error, plan: null);

    /// <summary>
    /// Plan diya jaye to connect ke baad capabilities us ke khilaf
    /// validate hoti hain — fail ho to device destroy karke false.
    /// </summary>
    public bool TryConnect(out string error, PipelinePlan? plan)
    {
        error = "";
        if (_connected) return true;
        _deviceReleased = false; // nayi koshish — naya device, dobara destroy hoga

        var sdk = _sdkOverride ?? (_sdk ??= new HidMaestroSdkAdapter(_profileId));
        if (!sdk.TryCreateDevice(out error))
            return false;

        _caps = sdk.Capabilities;
        _capsKnown = true;

        if (plan is not null)
        {
            var errors = ValidateCapabilities(plan, _caps);
            if (errors.Count > 0)
            {
                DestroyDeviceIfOwned();
                error = "HIDMaestro device is plan ke liye kaafi nahi hai: " + string.Join(" ", errors);
                return false;
            }
        }

        try
        {
            SendNeutralCore();
        }
        catch (Exception ex)
        {
            DestroyDeviceIfOwned();
            error = $"HIDMaestro neutral send nakam: {ex.Message}";
            return false;
        }

        _connected = true;
        return true;
    }

    private void DestroyDeviceIfOwned()
    {
        _connected = false;
        _capsKnown = false;
        _caps = HidMaestroCapabilities.Unknown;
        if (_deviceReleased) return;
        _deviceReleased = true;
        try { ActiveSdk?.DestroyDevice(); } catch { /* safai best-effort */ }
    }

    public void Disconnect()
    {
        bool wasConnected = _connected;
        _connected = false;
        // Neutral sirf tab jab device connected tha — band device par
        // dobara neutral bhejna bekaar hai (aur fake/real submit barhata hai).
        if (wasConnected)
        {
            try { SendNeutralCore(); } catch { /* neutral best-effort hai */ }
        }
        if (_deviceReleased) return;
        _deviceReleased = true;
        try { ActiveSdk?.DestroyDevice(); } catch { /* safai best-effort */ }
    }

    public void Update(in ProcessedState state)
    {
        if (!_connected) throw new InvalidOperationException("HIDMaestro connected nahi hai.");
        var sdk = ActiveSdk ?? throw new InvalidOperationException("HIDMaestro connected nahi hai.");
        try
        {
            if (_useRawReport)
            {
                HidMaestroMapping.BuildRawReport(state, _capsKnown ? _caps : HidMaestroCapabilities.Unknown, _rawReport);
                sdk.SubmitRawReport(_rawReport);
            }
            else
            {
                var frame = HidMaestroMapping.BuildGamepadState(state, _capsKnown ? _caps : HidMaestroCapabilities.Unknown, _axes);
                sdk.SubmitState(frame);
            }
        }
        catch (Exception ex) when (ex is not InvalidOperationException)
        {
            throw new InvalidOperationException($"HIDMaestro submit nakam — device haath se nikal gaya lagta hai ({ex.GetType().Name}: {ex.Message}).", ex);
        }
    }

    /// <summary>
    /// Mehfooz neutral halat bhejo — kabhi throw nahi karta (watchdog /
    /// band karte waqt ka rasta; VJoy se zyada mehfooz).
    /// </summary>
    public void SendNeutral()
    {
        if (!_connected) return;
        try { SendNeutralCore(); } catch { /* neutral best-effort hai */ }
    }

    private void SendNeutralCore()
    {
        var sdk = ActiveSdk ?? throw new InvalidOperationException("HIDMaestro connected nahi hai.");
        if (_useRawReport)
        {
            HidMaestroMapping.BuildNeutralRawReport(_capsKnown ? _caps : HidMaestroCapabilities.Unknown, _rawReport);
            sdk.SubmitRawReport(_rawReport);
        }
        else
        {
            var frame = HidMaestroMapping.NeutralGamepadState(_capsKnown ? _caps : HidMaestroCapabilities.Unknown, _axes);
            sdk.SubmitState(frame);
        }
    }

    public void Dispose() => Disconnect();
}
