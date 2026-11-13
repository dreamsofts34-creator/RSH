using RSH.Core.Windows;

namespace RSH.Tests;

/// <summary>
/// v2.0.3 HOTFIX tests: Windows 10 par automatic classic window
/// (<see cref="WindowChromePolicy"/>) + AUTOMATIC plain navigation
/// (classic ke saath — user koi flag test nahi karega) + version bump
/// 2.0.2 -> 2.0.3.
///
/// Root cause: FluentWindow + ExtendsContentIntoTitleBar Win10
/// (build &lt; 22000) par paint nahi hota — app blank rehti hai —
/// jabke plain WPF window usi PC par render hoti hai (RenderTest:
/// Render Tier 2, hardware acceleration theek — GPU/driver ruled out).
/// Is liye Win10 par AUTOMATICALLY classic window, Win11+ par
/// FluentWindow waisa hi. Flags sirf override hain:
/// --classic-window force classic, --fluent-window force Fluent
/// (escape hatch); dono hon to classic jeet-ta hai.
///
/// Policy pure logic hai (koi OS call nahi) — is liye har OS version
/// ke liye test Linux par bhi chal sakta hai. Version bump source-text
/// par assert hota hai (V202 pattern): RSH.Tests sirf RSH.Core ko
/// reference karta hai, aur Core ki assembly ka version
/// Directory.Build.props (stale 2.0.0) se aata hai — app version nahi.
/// </summary>
public class V203HotfixTests
{
    private static string? FindRepoFile(string relative)
    {
        string? dir = AppContext.BaseDirectory;
        for (int i = 0; i < 10 && dir is not null; i++)
        {
            string candidate = Path.Combine(dir, relative);
            if (File.Exists(candidate)) return candidate;
            dir = Directory.GetParent(dir)?.FullName;
        }
        return null;
    }

    private static string ReadRepoFileOrSkip(string relative)
    {
        string? path = FindRepoFile(relative);
        if (path is null) Assert.Skip($"Repo file nahi mili (test run ki layout alag hai): {relative}");
        return File.ReadAllText(path);
    }

    // ================= 1. RequiresClassicWindow — OS version =================

    [Fact]
    public void RequiresClassicWindow_Win10_19045_True()
    {
        // User ka PC: Windows 10 22H2, build 19045 — asal failing case.
        Assert.True(WindowChromePolicy.RequiresClassicWindow(new Version(10, 0, 19045)));
    }

    [Fact]
    public void RequiresClassicWindow_Win10_19041_True()
    {
        // Win10 2004, build 19041 — abhi bhi Win10, classic chahiye.
        Assert.True(WindowChromePolicy.RequiresClassicWindow(new Version(10, 0, 19041)));
    }

    [Fact]
    public void RequiresClassicWindow_Win10_21999_True_BoundaryBelow()
    {
        // Win11 se ek build neeche — boundary ka neeche wala hissa.
        Assert.True(WindowChromePolicy.RequiresClassicWindow(new Version(10, 0, 21999)));
    }

    [Fact]
    public void RequiresClassicWindow_Win11_22000_False_Boundary()
    {
        // Windows 11 ka pehla build — yahan se FluentWindow waisa hi.
        Assert.False(WindowChromePolicy.RequiresClassicWindow(new Version(10, 0, 22000)));
        Assert.Equal(22000, WindowChromePolicy.Windows11Build);
    }

    [Fact]
    public void RequiresClassicWindow_Win11_22621_False()
    {
        Assert.False(WindowChromePolicy.RequiresClassicWindow(new Version(10, 0, 22621)));
    }

    [Fact]
    public void RequiresClassicWindow_Win11_26100_False()
    {
        Assert.False(WindowChromePolicy.RequiresClassicWindow(new Version(10, 0, 26100)));
    }

    [Fact]
    public void RequiresClassicWindow_PreWin10_True()
    {
        // Major < 10 (Win7/8) — classic, Fluent chrome ki koshish hi nahi.
        Assert.True(WindowChromePolicy.RequiresClassicWindow(new Version(6, 3, 9600)));
    }

    // ================= 2. ShouldUseClassicWindow — flags + OS =================

    [Fact]
    public void ShouldUseClassicWindow_Win11_ClassicFlag_True()
    {
        Assert.True(WindowChromePolicy.ShouldUseClassicWindow(
            new Version(10, 0, 22621), classicFlag: true, fluentFlag: false));
    }

    [Fact]
    public void ShouldUseClassicWindow_Win10_FluentFlag_False()
    {
        // Escape hatch: Win10 par bhi Fluent force ho sakta hai.
        Assert.False(WindowChromePolicy.ShouldUseClassicWindow(
            new Version(10, 0, 19045), classicFlag: false, fluentFlag: true));
    }

    [Fact]
    public void ShouldUseClassicWindow_Win10_NoFlags_True()
    {
        // v2.0.3 ka asal fix: Win10 par koi flag nahi — khud classic.
        Assert.True(WindowChromePolicy.ShouldUseClassicWindow(
            new Version(10, 0, 19045), classicFlag: false, fluentFlag: false));
    }

    [Fact]
    public void ShouldUseClassicWindow_Win11_NoFlags_False()
    {
        // Win11 par default waisa hi — FluentWindow.
        Assert.False(WindowChromePolicy.ShouldUseClassicWindow(
            new Version(10, 0, 22621), classicFlag: false, fluentFlag: false));
    }

    [Fact]
    public void ShouldUseClassicWindow_BothFlags_ClassicWins()
    {
        // Dono flags hon to classic jeet-ta hai — Win11 par bhi...
        Assert.True(WindowChromePolicy.ShouldUseClassicWindow(
            new Version(10, 0, 22621), classicFlag: true, fluentFlag: true));
        // ...aur Win10 par bhi.
        Assert.True(WindowChromePolicy.ShouldUseClassicWindow(
            new Version(10, 0, 19045), classicFlag: true, fluentFlag: true));
    }

    [Fact]
    public void ShouldUseClassicWindow_Win10_ClassicFlag_True()
    {
        Assert.True(WindowChromePolicy.ShouldUseClassicWindow(
            new Version(10, 0, 19045), classicFlag: true, fluentFlag: false));
    }

    // ================= 3. DescribeDecision — mode + build + reason =================

    [Fact]
    public void DescribeDecision_Win10_19045_Classic_WithBuildAndReason()
    {
        bool useClassic = WindowChromePolicy.ShouldUseClassicWindow(
            new Version(10, 0, 19045), classicFlag: false, fluentFlag: false);
        string text = WindowChromePolicy.DescribeDecision(new Version(10, 0, 19045), useClassic);
        Assert.Contains("10.0.19045", text, StringComparison.Ordinal);
        Assert.Contains("classic window", text, StringComparison.Ordinal);
        Assert.Contains("19045", text, StringComparison.Ordinal);
        Assert.Contains("22000", text, StringComparison.Ordinal);
    }

    [Fact]
    public void DescribeDecision_Win11_22621_Fluent_WithBuildAndReason()
    {
        bool useClassic = WindowChromePolicy.ShouldUseClassicWindow(
            new Version(10, 0, 22621), classicFlag: false, fluentFlag: false);
        string text = WindowChromePolicy.DescribeDecision(new Version(10, 0, 22621), useClassic);
        Assert.Contains("10.0.22621", text, StringComparison.Ordinal);
        Assert.Contains("FluentWindow", text, StringComparison.Ordinal);
        Assert.Contains("22621", text, StringComparison.Ordinal);
        Assert.Contains("22000", text, StringComparison.Ordinal);
    }

    [Fact]
    public void DescribeDecision_Win10_FluentFlag_FluentMode()
    {
        // Escape hatch ke saath mode Fluent hota hai — reason mein flag
        // override wazeh likha hota hai ("build 19045 < 22000; Fluent
        // requested by --fluent-window flag"), OS se ghalat reason nahi.
        // Is case mein sirf mode assert hota hai, reason nahi.
        bool useClassic = WindowChromePolicy.ShouldUseClassicWindow(
            new Version(10, 0, 19045), classicFlag: false, fluentFlag: true);
        string text = WindowChromePolicy.DescribeDecision(new Version(10, 0, 19045), useClassic);
        Assert.Contains("FluentWindow", text, StringComparison.Ordinal);
        Assert.Contains("10.0.19045", text, StringComparison.Ordinal);
        Assert.DoesNotContain("classic window", text, StringComparison.Ordinal);
    }

    [Fact]
    public void DescribeDecision_Win11_ClassicFlag_ClassicMode()
    {
        bool useClassic = WindowChromePolicy.ShouldUseClassicWindow(
            new Version(10, 0, 22621), classicFlag: true, fluentFlag: false);
        string text = WindowChromePolicy.DescribeDecision(new Version(10, 0, 22621), useClassic);
        Assert.Contains("classic window", text, StringComparison.Ordinal);
        Assert.Contains("10.0.22621", text, StringComparison.Ordinal);
        Assert.DoesNotContain("FluentWindow", text, StringComparison.Ordinal);
    }

    // ================= 4. Version 2.0.3 (IsV203 / Version203) =================

    [Fact]
    public void IsV203_AppCsproj_Version_Is203()
    {
        // App version source-text par (V202 pattern): RSH.Tests ka
        // Core assembly version app version nahi hota (stale 2.0.0).
        string csproj = ReadRepoFileOrSkip(Path.Combine("software", "src", "RSH.App", "RSH.App.csproj"));
        Assert.Contains("<Version>2.0.3</Version>", csproj, StringComparison.Ordinal);
        Assert.DoesNotContain("<Version>2.0.2</Version>", csproj, StringComparison.Ordinal);
    }

    [Fact]
    public void Version203_MutexAndPipeNames_Contain203_AndDifferFrom202()
    {
        var v203 = new Version(2, 0, 3, 0);
        Assert.Equal("2.0.3", AppVersionInfo.VersionSuffix(v203));
        Assert.Equal(@"Global\RSH.SingleInstance.2.0.3", AppVersionInfo.MutexNameForVersion(v203));
        Assert.Equal("RSH.SingleInstance.2.0.3", AppVersionInfo.PipeNameForVersion(v203));
        // v2.0.2 ke naamon se mukhtalif hon — warna version-aware
        // single instance purani instance ko naya samajh lega.
        Assert.NotEqual(AppVersionInfo.MutexNameForVersion(new Version(2, 0, 2, 0)),
            AppVersionInfo.MutexNameForVersion(v203));
        Assert.NotEqual(AppVersionInfo.PipeNameForVersion(new Version(2, 0, 2, 0)),
            AppVersionInfo.PipeNameForVersion(v203));
    }

    [Fact]
    public void IsV203_MsixManifest_Version_Is203()
    {
        string manifest = ReadRepoFileOrSkip(Path.Combine("software", "packaging", "msix", "Package.appxmanifest"));
        Assert.Contains("Version=\"2.0.3.0\"", manifest, StringComparison.Ordinal);
        Assert.DoesNotContain("Version=\"2.0.2.0\"", manifest, StringComparison.Ordinal);
    }

    [Fact]
    public void IsV203_WingetManifests_Version203()
    {
        string installer = ReadRepoFileOrSkip(Path.Combine("software", "packaging", "winget", "dreamsofts34-creator.RSH.installer.yaml"));
        Assert.Contains("PackageVersion: 2.0.3", installer, StringComparison.Ordinal);
        string version = ReadRepoFileOrSkip(Path.Combine("software", "packaging", "winget", "dreamsofts34-creator.RSH.yaml"));
        Assert.Contains("PackageVersion: 2.0.3", version, StringComparison.Ordinal);
        string locale = ReadRepoFileOrSkip(Path.Combine("software", "packaging", "winget", "dreamsofts34-creator.RSH.locale.en-US.yaml"));
        Assert.Contains("PackageVersion: 2.0.3", locale, StringComparison.Ordinal);
        Assert.Contains("releases/tag/v2.0.3", locale, StringComparison.Ordinal);
    }

    [Fact]
    public void IsV203_UserGuide_Version_Is203_AndDocumentsAutoClassic()
    {
        string guide = ReadRepoFileOrSkip(Path.Combine("docs", "USER-GUIDE.md"));
        Assert.Contains("Version 2.0.3", guide, StringComparison.Ordinal);
        Assert.Contains("classic window", guide, StringComparison.Ordinal);
        Assert.Contains("--fluent-window", guide, StringComparison.Ordinal);
        Assert.Contains("build < 22000", guide, StringComparison.Ordinal);
    }

    [Fact]
    public void IsV203_ReleaseNotes_Exist_AndMentionAutoClassic()
    {
        string notes = ReadRepoFileOrSkip(Path.Combine("docs", "RELEASE-NOTES-v2.0.3.md"));
        Assert.Contains("v2.0.3", notes, StringComparison.Ordinal);
        Assert.Contains("classic window", notes, StringComparison.Ordinal);
        Assert.Contains("--fluent-window", notes, StringComparison.Ordinal);
        Assert.Contains("RSH-render-test-x64.zip", notes, StringComparison.Ordinal);
        Assert.Contains("v4.0.0", notes, StringComparison.Ordinal);
        // Notes mein koi adhura placeholder nahi hona chahiye.
        Assert.DoesNotContain("TODO", notes, StringComparison.Ordinal);
        Assert.DoesNotContain("TBD", notes, StringComparison.Ordinal);
        Assert.DoesNotContain("placeholder", notes, StringComparison.OrdinalIgnoreCase);
    }

    // ================= 5. ShouldUsePlainNavigation — auto + overrides =================
    // User koi flag test nahi karega — Win10 par plain navigation
    // classic chrome ke saath AUTOMATICALLY on hona chahiye; Win11 par
    // auto off. --plain-nav force-on, --fluent-window dono ko off.

    [Fact]
    public void ShouldUsePlainNavigation_Win10_19045_True()
    {
        Assert.True(WindowChromePolicy.ShouldUsePlainNavigation(
            new Version(10, 0, 19045), plainNavFlag: false, fluentFlag: false));
    }

    [Fact]
    public void ShouldUsePlainNavigation_Win11_22621_False()
    {
        Assert.False(WindowChromePolicy.ShouldUsePlainNavigation(
            new Version(10, 0, 22621), plainNavFlag: false, fluentFlag: false));
    }

    [Fact]
    public void ShouldUsePlainNavigation_Win10_FluentFlag_False()
    {
        // --fluent-window poora modern UI mangta hai — plain-nav bhi off.
        Assert.False(WindowChromePolicy.ShouldUsePlainNavigation(
            new Version(10, 0, 19045), plainNavFlag: false, fluentFlag: true));
    }

    [Fact]
    public void ShouldUsePlainNavigation_Win11_PlainNavFlag_True()
    {
        Assert.True(WindowChromePolicy.ShouldUsePlainNavigation(
            new Version(10, 0, 22621), plainNavFlag: true, fluentFlag: false));
    }

    [Fact]
    public void ShouldUsePlainNavigation_Win10_PlainNavFlag_True()
    {
        Assert.True(WindowChromePolicy.ShouldUsePlainNavigation(
            new Version(10, 0, 19045), plainNavFlag: true, fluentFlag: false));
    }

    // ================= 6. Plain navigation — parity (8/8 screens) =================
    // WPF window Linux par instantiate nahi hoti — source-text pattern
    // (V202/V203 jaisa): XAML mein 8/8 Tag values, _pages dict mein
    // 8/8 keys, aur ApplyPlainNavigation 8/8 buttons ko cover karta hai.

    [Fact]
    public void PlainNavigation_Parity_AllEightNavButtonsPresent()
    {
        string xaml = ReadRepoFileOrSkip(Path.Combine("software", "src", "RSH.App", "MainWindow.xaml"));
        string code = ReadRepoFileOrSkip(Path.Combine("software", "src", "RSH.App", "MainWindow.xaml.cs"));
        string methodBody = PlainNavigationMethodBody(code);
        string[] tags = ["Dashboard", "Tuning", "Setup", "Buttons", "Output", "Profiles", "Monitor", "Settings"];
        foreach (string tag in tags)
        {
            // XAML: har nav button ka Tag + Checked handler barkarar.
            Assert.Contains($"Tag=\"{tag}\"", xaml, StringComparison.Ordinal);
            Assert.Contains("Checked=\"Nav_Checked\"", xaml, StringComparison.Ordinal);
            // Code-behind: har page ki factory _pages dict mein.
            Assert.Contains($"[\"{tag}\"]", code, StringComparison.Ordinal);
            // ApplyPlainNavigation: har button ko cover karta hai.
            Assert.Contains($"Nav{tag}", methodBody, StringComparison.Ordinal);
        }
    }

    [Fact]
    public void PlainNavigation_AutoMode_WiredInAppStartup()
    {
        string app = ReadRepoFileOrSkip(Path.Combine("software", "src", "RSH.App", "App.xaml.cs"));
        Assert.Contains("ShouldUsePlainNavigation", app, StringComparison.Ordinal);
        Assert.Contains("PlainNavigationRequested", app, StringComparison.Ordinal);
        Assert.Contains("\"--plain-nav\"", app, StringComparison.Ordinal);
        string main = ReadRepoFileOrSkip(Path.Combine("software", "src", "RSH.App", "MainWindow.xaml.cs"));
        Assert.Contains("App.PlainNavigationRequested", main, StringComparison.Ordinal);
        Assert.Contains("ApplyPlainNavigation()", main, StringComparison.Ordinal);
    }

    private static string PlainNavigationMethodBody(string code)
    {
        int start = code.IndexOf("private void ApplyPlainNavigation()", StringComparison.Ordinal);
        Assert.True(start >= 0, "ApplyPlainNavigation method nahi mili");
        int end = code.IndexOf("private void Nav_Checked", StringComparison.Ordinal);
        Assert.True(end > start, "ApplyPlainNavigation method ka end nahi mila");
        return code[start..end];
    }
}
