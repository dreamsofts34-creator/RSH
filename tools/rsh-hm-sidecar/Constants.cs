// Pinned values for the HIDMaestro backend (G2). These mirror
// crates/rsh-output-hidmaestro/src/lib.rs — keep the two in sync.
// The DLL is never bundled; the sidecar loads it from the installed
// location at runtime and verifies its SHA-256 at startup.
namespace RshHmSidecar;

/// <summary>Pinned HIDMaestro SDK facts and the RSH custom profile.</summary>
internal static class Constants
{
    /// <summary>Pinned HIDMaestro release (latest, 2026-11-14).</summary>
    public const string PinnedVersion = "2.1.0";

    /// <summary>SHA-256 of the HIDMaestro v2.1.0 release ZIP.</summary>
    public const string PinnedZipSha256 =
        "7fe349f163864b8dfc36117af260d51bac94a2263b6cdfa719cb364a808397b4";

    /// <summary>SHA-256 of HIDMaestro.Core.dll from the v2.1.0 release.</summary>
    public const string PinnedDllSha256 =
        "f13e32591404db0f418af9265b8e240f71257297cfae5a41dcd696fd87e9b4d5";

    /// <summary>JSON-lines protocol version spoken on stdin/stdout (and TCP).</summary>
    public const int ProtocolVersion = 1;

    /// <summary>Custom no-brand RSH wheel profile id.</summary>
    public const string ProfileId = "rsh-wheel-custom";

    /// <summary>Identity key passed to <c>CreateController</c> for the custom profile.</summary>
    public const string IdentityKey = "rsh-wheel";

    /// <summary>
    /// The HIDMaestro built-in Logitech G29 catalog profile id.
    /// G29 spoof mode (v2.0.16) creates ONLY this catalog profile
    /// besides <see cref="ProfileId"/> — every other catalog id is
    /// still refused. Loaded via <c>HMContext.GetProfile</c> (after
    /// <c>LoadDefaultProfiles</c> — verified against the real v2.1.0
    /// DLL: <c>GetProfile</c> with no load throws
    /// <c>NullReferenceException</c>), never hand-built.
    /// RISK DISCLOSURE (research 2026-11-27): presenting Logitech's
    /// VID/PID violates USB-IF policy as written (no enforcement
    /// found for private hobby use); no Forza ToS text on spoofing
    /// was found (risk low, not zero); the user's pid.codes
    /// 0x1209/0x5253 claim is UNAFFECTED (simply unused while G29 is
    /// active). Even a REAL G29 needs a custom control scheme in FH4
    /// (Forza Known Issues) — this mode promises no auto-recognition,
    /// and FH4 end-to-end is unproven.
    /// </summary>
    public const string G29ProfileId = "logitech-g29";

    /// <summary>Identity key for the G29 controller — distinct from
    /// <see cref="IdentityKey"/> so the two profiles never collide in
    /// the SDK's identity store when the user toggles the mode.</summary>
    public const string G29IdentityKey = "rsh-wheel-g29";

    /// <summary>G29 VID (Logitech).</summary>
    public const ushort G29Vid = 0x046D;

    /// <summary>G29 PID (PC mode 0xC24F — NOT the PS4-mode 0xC260,
    /// whose 160-byte descriptor must never be paired with it).</summary>
    public const ushort G29Pid = 0xC24F;

    /// <summary>
    /// The catalog <c>logitech-g29</c> profile's 123-byte descriptor,
    /// lowercase hex — byte-for-byte the v2.1.0 DLL resource AND the
    /// research report (they match). A loaded profile whose descriptor
    /// bytes differ from this is refused at create.
    /// </summary>
    public const string G29DescriptorHex =
        "05010904a1010939150025073500463b0165147504950181420509190129196500250145017501951981027501950381030501093027ffff000047ffff000075109501810209320935093126ff0046ff007508950381020600ff0901950281020600ff0901950181020600ff090226ff0046ff00951075089102c0";

    /// <summary>Expected G29 input-report size, derived from the
    /// descriptor above (96 bits = 12 bytes, no Report ID). The SDK's
    /// own <c>HMProfile.InputReportSize</c> is 0 for catalog profiles
    /// (verified against the real DLL) and is NEVER trusted — the
    /// size is always derived with <c>HidReportSize</c>.</summary>
    public const int G29InputReportSize = 12;

    /// <summary>Profile vendor / manufacturer strings (no brand spoofing).</summary>
    public const string Vendor = "RSH";

    /// <summary>Profile product string.</summary>
    public const string Product = "RSH Wheel";

    /// <summary>
    /// VID from the pid.codes open-source pool (0x1209). RSH is
    /// MIT-licensed, so it qualifies; the VID is shared, not a brand.
    /// </summary>
    public const ushort Vid = 0x1209;

    /// <summary>
    /// PID 0x5253 = ASCII "RS". Chosen because it is absent from the
    /// pid.codes 1209 allocation list (verified 2026-11-14) and does
    /// not collide with any listed allocation. PROVISIONAL until a
    /// pid.codes PR claiming 0x5253 for "RSH — Real Sim Hub Wheel"
    /// is merged — that PR is the release gate before v1.0.0.
    /// The spike's placeholder 0x0001 is pid.codes' own "Test PID"
    /// (PIDs 0x0000–0x0FFF are reserved) and is unusable.
    /// </summary>
    public const ushort Pid = 0x5253;

    /// <summary>True while <see cref="Pid"/> is provisional (see above).</summary>
    public const bool VidPidProvisional = true;

    /// <summary>
    /// The C# app's old assumption (a 128-button profile that was
    /// never built). The real custom profile derives its report size
    /// from its descriptor — 15 bytes — and must never hardcode 26.
    /// </summary>
    public const int LegacyReportSizeAssumption = 26;

    /// <summary>Button-count mask for the SubmitState path (buttons 1–25).</summary>
    public const uint SubmitStateButtonMask = 0x01FF_FFFF;
}
