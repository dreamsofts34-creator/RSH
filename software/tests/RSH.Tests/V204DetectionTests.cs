using RSH.Core.Windows;

namespace RSH.Tests;

/// <summary>
/// v2.0.4 detection hardening tests: WindowChromePolicy ke bool-based
/// overloads (RtlGetVersion boundary — manifest-independent rasta).
/// Pure logic — Linux par chalte hain. Version-based overloads
/// (V203) inhi par delegate karte hain — dono ke natije barabar hone chahiye.
/// </summary>
public class V204DetectionTests
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

    [Fact]
    public void IsV204_ReleaseNotes_Exist_AndMentionPlainWindow()
    {
        // release.yml notes-file isi file ki taraf ishara karta hai.
        string notes = ReadRepoFileOrSkip(Path.Combine("docs", "RELEASE-NOTES-v2.0.4.md"));
        Assert.Contains("v2.0.4", notes, StringComparison.Ordinal);
        Assert.Contains("v4.0.1", notes, StringComparison.Ordinal);
        Assert.Contains("System.Windows.Window", notes, StringComparison.Ordinal);
        Assert.Contains("ClassicMainWindow", notes, StringComparison.Ordinal);
    }

    [Fact]
    public void ClassicWindow_BoolOverload_Win10_True_Win11_False()
    {
        Assert.True(WindowChromePolicy.ShouldUseClassicWindow(windows11OrNewer: false, classicFlag: false, fluentFlag: false));
        Assert.False(WindowChromePolicy.ShouldUseClassicWindow(windows11OrNewer: true, classicFlag: false, fluentFlag: false));
    }

    [Fact]
    public void ClassicWindow_BoolOverload_FlagPrecedence()
    {
        // --classic-window Win11 par bhi classic force karta hai.
        Assert.True(WindowChromePolicy.ShouldUseClassicWindow(windows11OrNewer: true, classicFlag: true, fluentFlag: false));
        // --fluent-window Win10 par bhi Fluent force karta hai.
        Assert.False(WindowChromePolicy.ShouldUseClassicWindow(windows11OrNewer: false, classicFlag: false, fluentFlag: true));
        // Dono flags hon to classic jeet-ta hai.
        Assert.True(WindowChromePolicy.ShouldUseClassicWindow(windows11OrNewer: true, classicFlag: true, fluentFlag: true));
        Assert.True(WindowChromePolicy.ShouldUseClassicWindow(windows11OrNewer: false, classicFlag: true, fluentFlag: true));
    }

    [Fact]
    public void PlainNavigation_BoolOverload_Win10_True_Win11_False()
    {
        Assert.True(WindowChromePolicy.ShouldUsePlainNavigation(windows11OrNewer: false, plainNavFlag: false, fluentFlag: false));
        Assert.False(WindowChromePolicy.ShouldUsePlainNavigation(windows11OrNewer: true, plainNavFlag: false, fluentFlag: false));
    }

    [Fact]
    public void PlainNavigation_BoolOverload_FlagPrecedence()
    {
        // --plain-nav Win11 par bhi plain force karta hai.
        Assert.True(WindowChromePolicy.ShouldUsePlainNavigation(windows11OrNewer: true, plainNavFlag: true, fluentFlag: false));
        // --fluent-window Win10 par bhi modern force karta hai.
        Assert.False(WindowChromePolicy.ShouldUsePlainNavigation(windows11OrNewer: false, plainNavFlag: false, fluentFlag: true));
        // Dono flags hon to plain-nav jeet-ta hai.
        Assert.True(WindowChromePolicy.ShouldUsePlainNavigation(windows11OrNewer: true, plainNavFlag: true, fluentFlag: true));
    }

    [Fact]
    public void BoolAndVersionOverloads_Agree()
    {
        // User ka PC: Win10 22H2 build 19045 — dono raste classic + plain.
        var win10 = new Version(10, 0, 19045);
        Assert.Equal(
            WindowChromePolicy.ShouldUseClassicWindow(win10, classicFlag: false, fluentFlag: false),
            WindowChromePolicy.ShouldUseClassicWindow(windows11OrNewer: false, classicFlag: false, fluentFlag: false));
        Assert.Equal(
            WindowChromePolicy.ShouldUsePlainNavigation(win10, plainNavFlag: false, fluentFlag: false),
            WindowChromePolicy.ShouldUsePlainNavigation(windows11OrNewer: false, plainNavFlag: false, fluentFlag: false));
        // Win11 22621 — dono raste Fluent + modern.
        var win11 = new Version(10, 0, 22621);
        Assert.Equal(
            WindowChromePolicy.ShouldUseClassicWindow(win11, classicFlag: false, fluentFlag: false),
            WindowChromePolicy.ShouldUseClassicWindow(windows11OrNewer: true, classicFlag: false, fluentFlag: false));
        Assert.Equal(
            WindowChromePolicy.ShouldUsePlainNavigation(win11, plainNavFlag: false, fluentFlag: false),
            WindowChromePolicy.ShouldUsePlainNavigation(windows11OrNewer: true, plainNavFlag: false, fluentFlag: false));
    }
}
