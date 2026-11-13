// Firmware upload gate (SEC-C-01/02/05, Phase D) — UI-free faisle,
// taake WizardWindow thin rahe aur gate unit test ho sake.
//
// Upload ka faisla (CheckUpload):
//   1. FirmwareImageValidator ka nateeja (size/magic/SHA/version-floor)
//      — invalid ho to upload kabhi nahi (SEC-C-01).
//   2. GET_CAPS caps milen hon to image version >= board version honi
//      chahiye — kam = downgrade reject (SEC-C-05). Barabar = reinstall,
//      allowed (warning ke saath).
//   3. Caps na milen (session key nahi — SEC-D-05) to upload floor
//      check par hi allowed hoti hai, lekin WARNING ke saath: live
//      anti-downgrade check nahi hua, upload ke baad version verify
//      lazmi hai (CheckPostUpload).
//
// Upload ke baad (CheckPostUpload, SEC-C-02): kamyabi SIRF tab jab
// GET_CAPS se naya version confirm ho (image version == board version).
// HTTP 200 + "OK" akela kamyabi nahi — connection tootna, board reject,
// ya purana version rehna sab "verify nahi hua" hai, kabhi success nahi.
using RSH.Core.Protocol;

namespace RSH.Core.Firmware;

/// <summary>Upload se pehle ka faisla: allowed? reject ki wajah? warning?</summary>
public sealed record UploadGateDecision(bool Allowed, string? Error, string? Warning);

/// <summary>Upload ke baad version confirm ka nateeja.</summary>
public sealed record PostUploadCheck(bool Confirmed, string Message);

public static class FirmwareUploadGate
{
    /// <summary>Upload bhejne se pehle: validator + (agar maujood ho to) GET_CAPS version compare.</summary>
    public static UploadGateDecision CheckUpload(FirmwareImageValidator.ValidationResult validated, BoardCaps? currentCaps)
    {
        ArgumentNullException.ThrowIfNull(validated);
        if (!validated.IsValid)
            return new UploadGateDecision(false, validated.Error ?? "Firmware image invalid hai.", null);
        // SEC-C-06: signed trailer lazmi hai (validator bhi reject karta
        // hai) — gate par dobara check karo taake validator ka behaviour
        // badle bhi unsigned image kabhi upload na ho.
        if (!validated.IsSigned)
            return new UploadGateDecision(false, "Image signed nahi hai — sirf signed official RSH release upload karo.", null);
        if (validated.FirmwareVersion is null)
            return new UploadGateDecision(false, "Image ka version nahi mila — upload nahi hogi.", null);
        if (currentCaps is null)
        {
            return new UploadGateDecision(true, null,
                "Board ka current version verify nahi ho saka (GET_CAPS nahi mila — session key maujood nahi). " +
                "Sirf minimum-version floor (v4.0.0) check hua hai; upload ke baad version dobara verify karo.");
        }
        if (currentCaps.IsDowngrade(validated.FirmwareVersion))
        {
            return new UploadGateDecision(false,
                $"Downgrade reject: image v{validated.FirmwareVersion} hai, board par v{currentCaps.FirmwareVersion} chal rahi hai (purani firmware mein maloom bugs hain).", null);
        }
        if (validated.FirmwareVersion == currentCaps.FirmwareVersion)
        {
            return new UploadGateDecision(true, null,
                $"Image v{validated.FirmwareVersion} board wali version jaisi hi hai — ye reinstall hogi, upgrade nahi.");
        }
        return new UploadGateDecision(true, null, null);
    }

    /// <summary>Upload + board restart ke baad: GET_CAPS se naya version confirm karo (SEC-C-02).</summary>
    public static PostUploadCheck CheckPostUpload(BoardCaps? afterCaps, Version imageVersion)
    {
        ArgumentNullException.ThrowIfNull(imageVersion);
        if (afterCaps is null)
        {
            return new PostUploadCheck(false,
                "Update VERIFY NAHI HUA — board se version confirm nahi ho saka (GET_CAPS nahi mila). " +
                "Board restart ho raha ho sakta hai ya upload fail hua ho; version check karo, purana ho to dobara koshish karo.");
        }
        if (afterCaps.FirmwareVersion == imageVersion)
        {
            return new PostUploadCheck(true, $"Update verify hua ✓ Board ab firmware v{afterCaps.FirmwareVersion} chala raha hai.");
        }
        return new PostUploadCheck(false,
            $"Update VERIFY NAHI HUA — board abhi v{afterCaps.FirmwareVersion} chala raha hai, image v{imageVersion} thi. " +
            "Upload fail hui ya board ne purani firmware rakhi; dobara koshish karo.");
    }
}
