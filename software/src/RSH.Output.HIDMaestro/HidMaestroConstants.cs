namespace RSH.Output.HIDMaestro;

/// <summary>
/// HIDMaestro v2.1.0 — pinned release (SEC-H-06: koi NuGet nahi, ZIP +
/// SHA-256 pin lazmi; bus factor 1 + tez churn ki wajah se version pin
/// ke baghair upgrade kabhi nahi).
/// ZIP + DLL ke SHA-256 main ne 2026-11-13 ko asal release se naap kar
/// darj kiye hain (github.com/hifihedgehog/HIDMaestro/releases, v2.1.0 —
/// us waqt ki latest release; catalog mein 336 profiles load hote hain,
/// evaluation mein likhe 337 nahi — ek profile kam hai, dekho
/// docs/HIDMAESTRO-GATE.md). <see cref="PinnedDllSha256"/> ki tasdeeq
/// test mein hoti hai (loaded assembly ka hash — RSH.HIDMaestro.Tests).
/// </summary>
public static class HidMaestroConstants
{
    /// <summary>Pinned SDK version — is se hat kar koi version istemal nahi hota.</summary>
    public const string PinnedVersion = "2.1.0";

    /// <summary>Release ZIP (x64) — github.com/hifihedgehog/HIDMaestro/releases/download/v2.1.0/HIDMaestro-v2.1.0.zip</summary>
    public const string PinnedZipFileName = "HIDMaestro-v2.1.0.zip";

    /// <summary>Release ZIP ka SHA-256 (2026-11-13 ko download karke naapa gaya).</summary>
    public const string PinnedZipSha256 = "7fe349f163864b8dfc36117af260d51bac94a2263b6cdfa719cb364a808397b4";

    /// <summary>Vendored HIDMaestro.Core.dll ka SHA-256 (lib/HIDMaestro.Core.dll — test mein verify hota hai).</summary>
    public const string PinnedDllSha256 = "f13e32591404db0f418af9265b8e240f71257297cfae5a41dcd696fd87e9b4d5";

    /// <summary>MIT — lib/LICENSE-HIDMaestro.txt (THIRD-PARTY-NOTICES.txt bhi saath hai).</summary>
    public const string License = "MIT";

    /// <summary>
    /// RSH device ki identity key — dobara start par wahi device wapas
    /// milta hai (SDK: keyed controller apni paths/container id rakhta hai).
    /// </summary>
    public const string IdentityKey = "rsh-wheel";

    /// <summary>
    /// SEC-H-06 hard rule: composite profiles ka bundled usbip-win2
    /// kernel transport KABHI enable nahi hota — aisa profile milte hi
    /// TryConnect refuse karta hai (HidMaestroSdkAdapter.ValidateProfile).
    /// </summary>
    public const string UsbipBackendName = "usbip";
}
