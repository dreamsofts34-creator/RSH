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

    /// <summary>Identity key passed to <c>CreateController</c>.</summary>
    public const string IdentityKey = "rsh-wheel";

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
