namespace RSH.Core.Windows;

/// <summary>
/// v2.0.3 HOTFIX — Win10 par FluentWindow chrome ka faisla.
///
/// Root cause: FluentWindow + ExtendsContentIntoTitleBar Win10
/// (build &lt; 22000) par paint nahi hota — app blank rehti hai —
/// jabke plain WPF window usi PC par render hoti hai (RenderTest:
/// Render Tier 2, hardware acceleration theek — GPU/driver ruled out).
/// Is liye Win10 par AUTOMATICALLY classic window (plain OS title bar),
/// Win11+ par FluentWindow waisa hi. Koi flag darkaar nahi; flags sirf
/// override hain (--classic-window force classic, --fluent-window
/// force Fluent — escape hatch).
///
/// Pure logic — koi OS call nahi — taake unit tests har OS version
/// ke liye chal sakein (yeh class RSH.Core mein hai, RSH.App mein nahi).
/// </summary>
public static class WindowChromePolicy
{
    /// <summary>Windows 11 ka pehla build — is se neeche sab Win10 (ya purana) hai.</summary>
    public const int Windows11Build = 22000;

    /// <summary>
    /// True jab OS ko classic window chahiye: Major &lt; 10 (Win7/8),
    /// ya Major == 10 aur Build &lt; 22000 (Win10). Win11+ par false.
    /// </summary>
    public static bool RequiresClassicWindow(Version osVersion)
    {
        ArgumentNullException.ThrowIfNull(osVersion);
        return osVersion.Major < 10
            || (osVersion.Major == 10 && osVersion.Build < Windows11Build);
    }

    /// <summary>
    /// Aakhri faisla — precedence: classicFlag jeet-ta hai, phir
    /// fluentFlag, phir OS version (<see cref="RequiresClassicWindow"/>).
    /// </summary>
    public static bool ShouldUseClassicWindow(Version osVersion, bool classicFlag, bool fluentFlag)
    {
        ArgumentNullException.ThrowIfNull(osVersion);
        return ShouldUseClassicWindow(!RequiresClassicWindow(osVersion), classicFlag, fluentFlag);
    }

    /// <summary>
    /// v2.0.4 — wahi faisla, lekin OS version ke bajaye Win11 boundary
    /// (RtlGetVersion-based <c>OperatingSystem.IsWindowsVersionAtLeast</c>)
    /// se: <paramref name="windows11OrNewer"/> true = Win11+ (build &gt;=
    /// 22000), false = Win10 ya purana. Manifest-independent rasta —
    /// <c>Environment.OSVersion</c> manifest par inhisar karta hai (shipped
    /// exe mein manifest verify hai, lekin agar kabhi jhoot bole to
    /// faisla phir bhi sahi ho). Flag precedence wahi: classicFlag
    /// jeet-ta hai, phir fluentFlag, phir OS boundary.
    /// </summary>
    public static bool ShouldUseClassicWindow(bool windows11OrNewer, bool classicFlag, bool fluentFlag)
    {
        if (classicFlag) return true;
        if (fluentFlag) return false;
        return !windows11OrNewer;
    }

    /// <summary>
    /// v2.0.3 HOTFIX — plain navigation ka faisla (sidebar original-style
    /// saada ho ya modern). Precedence classic jaisi: plainNavFlag
    /// jeet-ta hai (force-on), phir fluentFlag (force-off — user ne
    /// Fluent manga to poora modern UI mile), phir OS version: Win10
    /// (classic chahiye) par auto ON, Win11+ par auto OFF.
    /// User koi flag test nahi karega — Win10 par dono fallbacks
    /// (classic chrome + plain sidebar) ek saath auto ON hone chahiye.
    /// </summary>
    public static bool ShouldUsePlainNavigation(Version osVersion, bool plainNavFlag, bool fluentFlag)
    {
        ArgumentNullException.ThrowIfNull(osVersion);
        return ShouldUsePlainNavigation(!RequiresClassicWindow(osVersion), plainNavFlag, fluentFlag);
    }

    /// <summary>
    /// v2.0.4 — wahi plain-nav faisla, Win11 boundary (RtlGetVersion-based)
    /// se — <see cref="ShouldUseClassicWindow(bool, bool, bool)"/> wala
    /// rasta. Precedence: plainNavFlag jeet-ta hai (force-on), phir
    /// fluentFlag (force-off), phir OS boundary: Win10 par auto ON,
    /// Win11+ par auto OFF.
    /// </summary>
    public static bool ShouldUsePlainNavigation(bool windows11OrNewer, bool plainNavFlag, bool fluentFlag)
    {
        if (plainNavFlag) return true;
        if (fluentFlag) return false;
        return !windows11OrNewer;
    }

    /// <summary>
    /// Faisle ki ek line, hamesha wajah ke saath — startup log ke liye.
    /// Masalan: "Windows 10.0.19045 -&gt; classic window (build 19045 &lt; 22000)"
    /// ya "Windows 10.0.22621 -&gt; FluentWindow (build 22621 &gt;= 22000)".
    /// Flag override ho to OS reason ghalat ho jata hai (masalan Win10 +
    /// --fluent-window par "build 19045 &gt;= 22000" jhoot hota) — is liye
    /// override wazeh likha jata hai, numbers ke saath, masalan:
    /// "Windows 10.0.19045 -&gt; FluentWindow (build 19045 &lt; 22000;
    /// Fluent requested by --fluent-window flag)".
    /// </summary>
    public static string DescribeDecision(Version osVersion, bool useClassic)
    {
        ArgumentNullException.ThrowIfNull(osVersion);
        string mode = useClassic ? "classic window" : "FluentWindow";
        // OS reason hamesha OS se aaye (RequiresClassicWindow), useClassic
        // se nahi — warna flag override par jhoot likhta hai: Win10 (19045)
        // + --fluent-window par "build 19045 >= 22000", ya Win11 (22621)
        // + --classic-window par "build 22621 < 22000" (dono ghalat;
        // V203 tests ke comment mein sahi string darj hai).
        bool osWantsClassic = RequiresClassicWindow(osVersion);
        string osReason = osWantsClassic
            ? osVersion.Major < 10
                ? $"Windows major {osVersion.Major} < 10"
                : $"build {osVersion.Build} < {Windows11Build}"
            : osVersion.Major > 10
                ? $"Windows major {osVersion.Major} > 10"
                : $"build {osVersion.Build} >= {Windows11Build}";
        // Flag override ho (RequiresClassicWindow != useClassic) to OS
        // reason jhoot bolta — override wazeh likho, numbers ke saath.
        string reason = RequiresClassicWindow(osVersion) == useClassic
            ? osReason
            : useClassic
                ? $"{osReason}; classic requested by --classic-window flag"
                : $"{osReason}; Fluent requested by --fluent-window flag";
        return $"Windows {osVersion} -> {mode} ({reason})";
    }
}
