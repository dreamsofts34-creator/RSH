namespace RSH.Core.Processing;

/// <summary>
/// Setup wizard ke gates — D10 ka canonical source of truth (Phase E WS7).
///
/// Masla (SETUP-GATES-DECISION.md): wizard ke gates (SetupViewModel:
/// 250 raw/side steering, 150 raw pedal) IsSane ke 660 mV se milte hi
/// nahi thay — wizard "captured ✓" keh kar aage barha deta tha, phir
/// Finish IsSane par reject kar deta tha ("Those values don't look
/// right"). Gates ab mV mein hain aur D10 se barabar hain, is liye
/// wizard se guzri calibration IsSane se kabhi reject nahi hoti.
///
/// Ahem: gates hamesha **milliVolts** mein compare hote hain. Live
/// values packet units mein aati hain — v1/raw ho to pehle
/// <see cref="InputMapper.RawToMillivolts"/> se mV mein badlo
/// (pedals v1 mein hamesha raw hote hain; steering sirf tab raw hai
/// jab packet ka mV flag set na ho — V-C2, Finish mein wahi conversion
/// pehle se hai). Raw units par gate lagana dobara wahi mismatch kholta hai.
///
/// Hint strings: Wave 2 inhein verbatim dikhayega (professional
/// English, plan §4) — yahan ek jagah hain taake doc aur UI mein
/// farq na aye. `{n}` ki jagah baqi reh gaye mV aate hain.
/// </summary>
public static class SetupGates
{
    /// <summary>Steering: har taraf kam se kam 330 mV centre se (D10 span ka aadha).</summary>
    public const int MinSteeringSideMillivolts = SteeringCalibration.MinSideMillivolts;

    /// <summary>Steering: kul span kam se kam 660 mV (D10).</summary>
    public const int MinSteeringSpanMillivolts = SteeringCalibration.MinSpanMillivolts;

    /// <summary>Pedal (accel/brake): span kam se kam 660 mV (D10).</summary>
    public const int MinPedalSpanMillivolts = PedalCalibration.MinSpanMillivolts;

    /// <summary>Ek taraf ka safar kaafi hai? (mV mein — raw nahi.)</summary>
    public static bool SteeringSideComplete(int sideMillivolts) =>
        sideMillivolts >= MinSteeringSideMillivolts;

    /// <summary>
    /// Step 1 (full range) mukammal? Dono taraf ≥ 330 mV — kul span
    /// phir khud ≥ 660 mV hota hai. Sirf kul span par gate lagana kaafi
    /// nahi tha: 500/160 jesa ektarfa safar IsSane to pass kar jata,
    /// lekin wheel asal mein ek taraf adhoora hota — is liye per-side
    /// gate jaan boojh kar IsSane se sakht hai (faisla doc mein).
    /// </summary>
    public static bool SteeringRangeComplete(int leftMillivolts, int rightMillivolts) =>
        SteeringSideComplete(leftMillivolts) && SteeringSideComplete(rightMillivolts)
        && leftMillivolts + rightMillivolts >= MinSteeringSpanMillivolts;

    /// <summary>Pedal ka safar kaafi hai? (mV mein — raw nahi.)</summary>
    public static bool PedalComplete(int spanMillivolts) =>
        spanMillivolts >= MinPedalSpanMillivolts;

    // ---- Exact UI hint strings (Wave 2 verbatim) ----

    /// <summary>Baayein taraf aur ghumana hai — baqi mV ke saath.</summary>
    public static string SteeringLeftHint(int remainingMillivolts) =>
        $"Turn the wheel further left — {remainingMillivolts} mV more to reach the minimum range.";

    /// <summary>Daayein taraf aur ghumana hai — baqi mV ke saath.</summary>
    public static string SteeringRightHint(int remainingMillivolts) =>
        $"Turn the wheel further right — {remainingMillivolts} mV more to reach the minimum range.";

    /// <summary>Pedal poora dabana hai — baqi mV ke saath. Naam: "accelerator" / "brake".</summary>
    public static string PedalHint(string pedalName, int remainingMillivolts) =>
        $"Press the {pedalName} fully down — {remainingMillivolts} mV more to reach the minimum range.";

    /// <summary>
    /// Centre mashkook lage (ek taraf ka safar doosri taraf se bohat
    /// kam ho jab ke kul span kaafi ho — centre ghalat pakra gaya):
    /// user wheel seedha pakar kar step dobara kare.
    /// </summary>
    public const string CentreOffHint =
        "The centre looks off — hold the wheel straight ahead and restart this step.";

    /// <summary>
    /// Finish par IsSane reject (ye ab sirf hifazati jaal hai — naye
    /// gates se guzri calibration yahan nahi pohanchti): kaun sa axis
    /// aur kyun. Naam: "Steering" / "Accelerator" / "Brake".
    /// </summary>
    public static string CalibrationTooNarrowHint(string axisName) =>
        $"{axisName} range is too small — the wheel needs at least 660 mV from end to end. Turn it lock to lock and try again.";
}
