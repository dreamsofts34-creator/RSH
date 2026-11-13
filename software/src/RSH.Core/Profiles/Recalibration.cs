using RSH.Core.Protocol;

namespace RSH.Core.Profiles;

/// <summary>
/// <see cref="WheelProfile.NeedsRecalibration"/> kyun set hai — Wave 2
/// ki UI prompt isi se sahi wajah + sahi jumla dikhati hai. Ek se zyada
/// wajah ek saath lag sakti hai (flags).
/// </summary>
[Flags]
public enum RecalibrationReason
{
    None = 0,

    /// <summary>Kam se kam ek SET calibration D10 (mV tag + span ≥ 660 mV) par insane hai — Phase C migration case bhi yehi hai.</summary>
    InsaneCalibration = 1,

    /// <summary>Calibration ke waqt wala pin map ab ke pin map se mukhtalif hai (wiring badli).</summary>
    PinMapChanged = 2,

    /// <summary>Calibration ke waqt wala firmware major/minor ab ke version se mukhtalif hai.</summary>
    FirmwareChanged = 4,

    /// <summary>Calibration <see cref="CalibrationFreshness.StaleAfterDays"/> din se purani hai.</summary>
    StaleCalibration = 8,
}

/// <summary>
/// Recalibration check ke liye maujooda (live) context: board ka pin
/// map + firmware version + aaj ki tareekh. Profile par mojood stamps
/// (calibration ke waqt ke) se muqabla hota hai — jo stamp null ho
/// (legacy/unknown), us ka trigger nahi lagta, andaza nahi lagaya jata.
/// </summary>
public sealed record CalibrationContext(
    string? PinMapFingerprint,
    string? FirmwareVersion,
    DateTimeOffset NowUtc)
{
    /// <summary>GET_CAPS se context banao (pin map fingerprint + firmware version).</summary>
    public static CalibrationContext FromBoard(BoardCaps caps, DateTimeOffset nowUtc) =>
        new(CalibrationFreshness.PinMapFingerprint(caps), caps.FirmwareVersion.ToString(), nowUtc);
}

/// <summary>
/// <see cref="WheelProfile.CheckRecalibration"/> ka natija — Wave 2 ki
/// UI prompt yehi parhti hai: flag + wajah + dikhane wala jumla.
/// </summary>
public sealed record RecalibrationCheck(
    bool NeedsRecalibration,
    RecalibrationReason Reasons,
    string? PromptMessage);

/// <summary>
/// Calibration ki "taazgi" (freshness) — kab dobara calibration chahiye
/// (Phase E WS7). Trigger table docs/SETUP-GATES-DECISION.md mein hai.
/// </summary>
public static class CalibrationFreshness
{
    /// <summary>
    /// Stale policy (faisla, measured nahi): is se purani calibration
    /// dobara karwayi jati hai. Potentiometer drift + 6 mahine ka nudge —
    /// koi safety daawa nahi, sirf policy. 180 din = ~6 mahine.
    /// </summary>
    public const int StaleAfterDays = 180;

    /// <summary>
    /// Pin map ka canonical fingerprint: GET_CAPS pin table se — pin
    /// order mein, har entry "pin:kind:module" (kind/module byte codes,
    /// §3.4.1). Sirf analog pins nahi — poori table, taake koi bhi wiring
    /// tabdeeli pakri jaye. Do boards ki table ek jesi ho to fingerprint
    /// ek jesa; koi bhi pin/kind/module badle to mukhtalif.
    /// </summary>
    public static string PinMapFingerprint(BoardCaps caps)
    {
        ArgumentNullException.ThrowIfNull(caps);
        return string.Join(";", caps.PinTable
            .OrderBy(e => e.Pin)
            .Select(e => $"{e.Pin}:{(byte)e.Kind}:{(byte)e.Module}"));
    }

    /// <summary>Fingerprint normalise karo — khaali/ghalat ho to null (unknown = koi trigger nahi).</summary>
    public static string? NormalizeFingerprint(string? fingerprint) =>
        string.IsNullOrWhiteSpace(fingerprint) ? null : fingerprint.Trim();

    /// <summary>
    /// Firmware version normalise karo — "4.0.0" jesi canonical shakal;
    /// parse na ho to null (unknown = koi trigger nahi, andaza nahi).
    /// </summary>
    public static string? NormalizeFirmwareVersion(string? version)
    {
        if (string.IsNullOrWhiteSpace(version)) return null;
        return Version.TryParse(version.Trim(), out var parsed) ? parsed.ToString() : null;
    }

    /// <summary>
    /// Firmware badla? Sirf major/minor dekhte hain — patch-only
    /// (4.0.0 → 4.0.1) trigger nahi karta (faisla: semver ke mutabiq
    /// patch bug-fix hota hai, ADC path nahi badalta; ADC ko chhoone
    /// wali tabdeeli minor/major bump karti hai). Dono mein se koi
    /// version maloom na ho to false (unknown = koi trigger nahi).
    /// Parse na hone wali string par seedha string muqabla.
    /// </summary>
    public static bool FirmwareChanged(string? calibratedVersion, string? currentVersion)
    {
        string? oldVersion = NormalizeFirmwareVersion(calibratedVersion);
        string? newVersion = NormalizeFirmwareVersion(currentVersion);
        if (oldVersion is null || newVersion is null)
        {
            // Kam se kam ek parse nahi hui: dono raw strings hon aur
            // mukhtalif hon to badla mana jata hai, warna nahi.
            return calibratedVersion is not null && currentVersion is not null &&
                !string.Equals(calibratedVersion.Trim(), currentVersion.Trim(), StringComparison.Ordinal);
        }
        var oldParsed = Version.Parse(oldVersion);
        var newParsed = Version.Parse(newVersion);
        return oldParsed.Major != newParsed.Major || oldParsed.Minor != newParsed.Minor;
    }

    /// <summary>Wajah se prompt ka jumla banao (professional English — Wave 2 verbatim dikhayega).</summary>
    public static string? PromptMessageFor(RecalibrationReason reasons)
    {
        if (reasons == RecalibrationReason.None) return null;
        var parts = new List<string>();
        if (reasons.HasFlag(RecalibrationReason.InsaneCalibration))
            parts.Add("Your calibration looks out of range, so the wheel needs to be calibrated again.");
        if (reasons.HasFlag(RecalibrationReason.PinMapChanged))
            parts.Add("The wiring (pin assignment) changed since this calibration was made, so the wheel needs to be calibrated again.");
        if (reasons.HasFlag(RecalibrationReason.FirmwareChanged))
            parts.Add("The board firmware changed since this calibration was made, so the wheel needs to be calibrated again.");
        if (reasons.HasFlag(RecalibrationReason.StaleCalibration))
            parts.Add("This calibration is over 6 months old, so the wheel needs to be calibrated again.");
        return string.Join(" ", parts);
    }
}
