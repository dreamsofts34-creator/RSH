using RSH.App.Services;
using RSH.Core.Windows;
using Serilog;
using Serilog.Events;

namespace RSH.Tests;

/// <summary>
/// v2.0.4 classic-window fix tests: startup-status second channel,
/// log-directory fallback, window-title format. Pure logic (RSH.Core) —
/// Linux par chalte hain. WPF windows (ClassicMainWindow / App.xaml.cs
/// selection) yahan test nahi ho sakte — un ki verification
/// <c>dotnet build</c> RSH.App (XAML compile) + Win10 real paint hai.
/// </summary>
public class V204ClassicWindowTests
{
    private static string NewTempRoot() =>
        Path.Combine(Path.GetTempPath(), "rsh-v204-test-" + Guid.NewGuid().ToString("N"));

    // ---- StartupStatus ----

    [Fact]
    public void StartupStatus_Paths_UnderTempRsh()
    {
        Assert.EndsWith(Path.Combine("RSH", "startup-status.txt"), StartupStatus.StatusFilePath());
        Assert.EndsWith(Path.Combine("RSH", "serilog-selflog.txt"), StartupStatus.SelfLogFilePath());
        Assert.EndsWith(Path.Combine("RSH", "logs"), StartupStatus.FallbackLogDirectory());
    }

    [Fact]
    public void StartupStatus_Paths_HonourTempBaseOverride()
    {
        string root = NewTempRoot();
        Assert.Equal(Path.Combine(root, "RSH", "startup-status.txt"), StartupStatus.StatusFilePath(root));
        Assert.Equal(Path.Combine(root, "RSH", "logs"), StartupStatus.FallbackLogDirectory(root));
    }

    [Fact]
    public void StartupStatus_FormatLine_TimestampPlusMilestone()
    {
        string line = StartupStatus.FormatLine(new DateTimeOffset(2026, 11, 13, 21, 30, 0, TimeSpan.Zero), "window shown");
        Assert.Equal("[2026-11-13 21:30:00.000] window shown", line);
    }

    [Fact]
    public void StartupStatus_Reset_WritesFreshFile_ThenWriteAppends()
    {
        string root = NewTempRoot();
        string path = StartupStatus.StatusFilePath(root);

        // Purani run ki lines hon to Reset unhen mita de.
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        File.WriteAllText(path, "[old] stale line\n[old] stale line 2\n");

        StartupStatus.Reset("2.0.4", root);
        string[] afterReset = File.ReadAllLines(path);
        Assert.Single(afterReset);
        Assert.Contains("started v2.0.4", afterReset[0]);

        StartupStatus.Write("window created (ClassicMainWindow)", root);
        StartupStatus.Write("window shown", root);
        string[] lines = File.ReadAllLines(path);
        Assert.Equal(3, lines.Length);
        Assert.Contains("window created (ClassicMainWindow)", lines[1]);
        Assert.Contains("window shown", lines[2]);
    }

    [Fact]
    public void StartupStatus_Write_NeverThrows_OnUnwritableBase()
    {
        // /proc par likhna namumkin hai — phir bhi koi exception nahi.
        var ex = Record.Exception(() =>
        {
            StartupStatus.Reset("2.0.4", "/proc/definitely-not-writable-rsh");
            StartupStatus.Write("window shown", "/proc/definitely-not-writable-rsh");
        });
        Assert.Null(ex);
    }

    // ---- StartupStatus.WriteException (swallowed UI exception) ----

    [Fact]
    public void StartupStatus_FormatException_SingleLine_WithTypeMessageAndFirstStackLine()
    {
        Exception ex;
        try
        {
            throw new InvalidOperationException("render\nfailed");
        }
        catch (Exception caught)
        {
            ex = caught;
        }
        string line = StartupStatus.FormatException("Unhandled UI exception (handled, app continues)", ex);
        Assert.DoesNotContain("\n", line);
        Assert.DoesNotContain("\r", line);
        Assert.Contains("Unhandled UI exception (handled, app continues)", line);
        Assert.Contains("System.InvalidOperationException", line);
        Assert.Contains("render failed", line);
        Assert.Contains("StartupStatus_FormatException", line); // stack ki pehli line: ye test method
    }

    [Fact]
    public void StartupStatus_FormatException_NullException_NoThrow()
    {
        string line = StartupStatus.FormatException("ctx", null);
        Assert.Equal("ctx: (no exception)", line);
    }

    [Fact]
    public void StartupStatus_WriteException_AppendsOneLine_NeverThrows()
    {
        string root = NewTempRoot();
        StartupStatus.Reset("2.0.4", root);
        Exception ex;
        try
        {
            throw new ArgumentException("bad arg");
        }
        catch (Exception caught)
        {
            ex = caught;
        }
        StartupStatus.WriteException("Unhandled UI exception (handled, app continues)", ex, root);
        StartupStatus.WriteException("ctx", null, "/proc/definitely-not-writable-rsh");
        string[] lines = File.ReadAllLines(StartupStatus.StatusFilePath(root));
        Assert.Equal(2, lines.Length);
        Assert.Contains("System.ArgumentException", lines[1]);
        Assert.Contains("bad arg", lines[1]);
    }

    // ---- LogPathSelector ----

    [Fact]
    public void LogPathSelector_WritablePrimary_ReturnsPrimary()
    {
        string root = NewTempRoot();
        string primary = Path.Combine(root, "primary");
        string fallback = Path.Combine(root, "fallback");
        Assert.Equal(primary, LogPathSelector.ChooseLogDirectory(primary, fallback));
        Assert.True(Directory.Exists(primary));
    }

    [Fact]
    public void LogPathSelector_PrimaryIsFile_ReturnsFallback()
    {
        string root = NewTempRoot();
        Directory.CreateDirectory(root);
        // Primary aik FILE hai (directory nahi) — CreateDirectory fail.
        string primaryAsFile = Path.Combine(root, "logs");
        File.WriteAllText(primaryAsFile, "not a directory");
        string fallback = Path.Combine(root, "fallback");
        Assert.Equal(fallback, LogPathSelector.ChooseLogDirectory(primaryAsFile, fallback));
        Assert.False(LogPathSelector.IsWritableDirectory(primaryAsFile));
    }

    [Fact]
    public void LogPathSelector_IsWritableDirectory_NullOrEmpty_False_NoThrow()
    {
        Assert.False(LogPathSelector.IsWritableDirectory(null));
        Assert.False(LogPathSelector.IsWritableDirectory(""));
        Assert.False(LogPathSelector.IsWritableDirectory("   "));
    }

    // ---- WindowTitle ----

    [Fact]
    public void WindowTitle_Format_IncludesVersion()
    {
        Assert.Equal("RSH — Real Sim Hub v2.0.4", WindowTitle.Format("2.0.4"));
    }

    [Fact]
    public void WindowTitle_Format_NullOrEmpty_NoVersionSuffix()
    {
        Assert.Equal("RSH — Real Sim Hub", WindowTitle.Format(null));
        Assert.Equal("RSH — Real Sim Hub", WindowTitle.Format(""));
        Assert.Equal("RSH — Real Sim Hub", WindowTitle.Format("   "));
    }

    [Fact]
    public void WindowTitle_ForAssembly_ReturnsVersionedTitle()
    {
        string title = WindowTitle.ForAssembly(typeof(WindowChromePolicy).Assembly);
        Assert.StartsWith("RSH — Real Sim Hub v", title);
    }

    // ---- LogFileNaming (dashes NAHI) ----

    [Fact]
    public void LogFileNaming_FileName_HasNoDashes()
    {
        // Serilog rolling pattern rsh-.log -> rsh-yyyyMMdd.log.
        // User/parent dashed naam (rsh-2026-11-11.log) dhoondh rahe thay.
        Assert.Equal("rsh-20261111.log", LogFileNaming.FileName(new DateTime(2026, 11, 11)));
        Assert.NotEqual("rsh-2026-11-11.log", LogFileNaming.FileName(new DateTime(2026, 11, 11)));
    }

    [Fact]
    public void LogFileNaming_FilePath_JoinsDirectoryAndDatedName()
    {
        Assert.Equal(
            Path.Combine("/tmp/x", "rsh-20261113.log"),
            LogFileNaming.FilePath("/tmp/x", new DateTime(2026, 11, 13)));
        Assert.EndsWith("rsh-" + DateTime.Today.ToString("yyyyMMdd") + ".log",
            LogFileNaming.FilePathForToday("/tmp/x"));
    }

    // ---- BootstrapLogging single-config (linked App file) ----

    [Fact]
    public void BootstrapLogging_ConfigureTwice_SecondCallOnlyChangesLevel()
    {
        // NOTE: BootstrapLogging ka _configured flag process-static hai —
        // is liye is class mein ye AKELA test hai jo ConfigureLogger
        // bulata hai (dobara call ka behaviour hi test ho raha hai).
        string root = NewTempRoot();
        string primary1 = Path.Combine(root, "primary1");
        string primary2 = Path.Combine(root, "primary2");
        string fallback = Path.Combine(root, "fallback");
        try
        {
            string dir1 = BootstrapLogging.ConfigureLogger(LogEventLevel.Information, out bool fb1, primary1, fallback);
            Assert.False(fb1);
            Assert.Equal(primary1, dir1);
            Log.Information("bootstrap-line");

            // Doosri config jagah (settings level): logger replace NAHI
            // hona chahiye — dir wahi, primary2 kabhi bane hi nahi.
            string dir2 = BootstrapLogging.ConfigureLogger(LogEventLevel.Debug, out bool fb2, primary2, fallback);
            Assert.False(fb2);
            Assert.Equal(primary1, dir2);
            Assert.False(Directory.Exists(primary2));
            Log.Information("after-second-config-line");
            Log.Debug("debug-line-after-level-change");
            Log.CloseAndFlush();

            string file = Path.Combine(primary1, LogFileNaming.FileName(DateTime.Today));
            Assert.True(File.Exists(file));
            string[] lines = File.ReadAllLines(file);
            Assert.Contains(lines, l => l.Contains("bootstrap-line"));
            Assert.Contains(lines, l => l.Contains("after-second-config-line"));
            // Level switch kaam karta hai: Debug ab likhta hai.
            Assert.Contains(lines, l => l.Contains("debug-line-after-level-change"));
        }
        finally
        {
            try { Log.CloseAndFlush(); } catch { /* test cleanup best-effort */ }
        }
    }

    // ---- Selection mapping (policy -> window type) ----

    [Fact]
    public void ClassicWindowSelection_UserPc_Win10_19045_UsesClassic()
    {
        // User ka PC: Win10 22H2 build 19045 — policy classic kehti hai,
        // is liye App ko ClassicMainWindow kholna chahiye (selection
        // App.xaml.cs mein isi policy par hai).
        Assert.True(WindowChromePolicy.ShouldUseClassicWindow(
            new Version(10, 0, 19045), classicFlag: false, fluentFlag: false));
        Assert.True(WindowChromePolicy.ShouldUseClassicWindow(
            windows11OrNewer: false, classicFlag: false, fluentFlag: false));
    }
}
