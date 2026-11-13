using RSH.Core.Windows;

namespace RSH.Tests;

/// <summary>
/// v2.0.2 HOTFIX (P0) tests: version-aware single instance, purane instance
/// ka kill-decision logic, CloseToTray trap (source-text — WPF window Linux
/// par run nahi hoti, PhaseGWorkstream3Tests pattern), startup version log
/// (source-text), autostart stale-entry repair.
/// <para>
/// Version ko assembly se lene ka tareeqa use hota hai — literal "2.0.2"
/// par kahin assert nahi (bump se pehle test fail hota warna). Jo cheez
/// Windows-only hai (asal process terminate, registry) wo Linux par skip
/// ya fail-closed behaviour par assert hoti hai — reason test mein likha hai.
/// </para>
/// </summary>
public class V202HotfixTests
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

    private sealed class FakeStore : IAutostartStore
    {
        public string? Value;
        public bool ThrowOnWrite;
        public string? ReadValue() => Value;
        public void WriteValue(string value)
        {
            if (ThrowOnWrite) throw new UnauthorizedAccessException("registry denied");
            Value = value;
        }
        public void DeleteValue() => Value = null;
    }

    // ================= 1. Version-aware naming (AppVersionInfo) =================

    [Fact]
    public void VersionSuffix_IsMajorMinorBuild_NoLiteralVersion()
    {
        // Assembly se liya hua version — literal par assert nahi.
        var info = AppVersionInfo.ForAssembly(typeof(AppVersionInfo).Assembly);
        string expected = $"{info.AssemblyVersion.Major}.{info.AssemblyVersion.Minor}.{info.AssemblyVersion.Build}";
        Assert.Equal(expected, info.MutexSuffix);
        Assert.Equal(expected, AppVersionInfo.VersionSuffix(info.AssemblyVersion));
    }

    [Fact]
    public void MutexAndPipeNames_ContainVersionSuffix_AndDifferPerVersion()
    {
        var v1 = new Version(2, 0, 1, 0);
        var v2 = new Version(2, 0, 2, 0);
        string m1 = AppVersionInfo.MutexNameForVersion(v1);
        string m2 = AppVersionInfo.MutexNameForVersion(v2);
        Assert.Equal(@"Global\RSH.SingleInstance.2.0.1", m1);
        Assert.Equal(@"Global\RSH.SingleInstance.2.0.2", m2);
        Assert.NotEqual(m1, m2);
        Assert.Equal("RSH.SingleInstance.2.0.1", AppVersionInfo.PipeNameForVersion(v1));
        Assert.Equal("RSH.SingleInstance.2.0.2", AppVersionInfo.PipeNameForVersion(v2));
        // Legacy (unversioned) naam se mukhtalif hon — warna versioning bekar.
        Assert.NotEqual(SingleInstanceCoordinator.DefaultMutexName, m1);
        Assert.NotEqual(SingleInstanceCoordinator.DefaultPipeName, AppVersionInfo.PipeNameForVersion(v1));
    }

    [Fact]
    public void ForAssembly_DisplayVersionMatchesAssemblyVersion()
    {
        var info = AppVersionInfo.ForAssembly(typeof(AppVersionInfo).Assembly);
        Assert.Equal(info.AssemblyVersion, typeof(AppVersionInfo).Assembly.GetName().Version);
        Assert.False(string.IsNullOrWhiteSpace(info.DisplayVersion));
        Assert.StartsWith(info.MutexSuffix, info.DisplayVersion, StringComparison.Ordinal);
        Assert.Equal(AppVersionInfo.MutexNameForVersion(info.MutexSuffix), info.MutexName);
        Assert.Equal(AppVersionInfo.PipeNameForVersion(info.MutexSuffix), info.PipeName);
    }

    [Fact]
    public void ForCurrentVersion_CoordinatorUsesVersionedNames()
    {
        // ForAppVersion sirf naam banata hai — asal acquire yahan nahi
        // (mutex side-effect se bacho); naam ka check kaafi hai.
        using var coord = SingleInstanceCoordinator.ForAppVersion(typeof(AppVersionInfo).Assembly);
        Assert.NotNull(coord);
        // Versioned naam ka saboot: legacy naam se alag hona chahiye —
        // ye sirf naam par assert hai, acquire par nahi.
        var info = AppVersionInfo.ForAssembly(typeof(AppVersionInfo).Assembly);
        Assert.NotEqual(SingleInstanceCoordinator.DefaultMutexName, info.MutexName);
    }

    // ================= 2. Kill-decision logic (pure, Linux par bhi chalti hai) =================

    [Fact]
    public void ShouldTerminate_OlderSameUserSameExe_DifferentPid_True()
    {
        Assert.True(SingleInstanceCoordinator.ShouldTerminateOlderInstance(
            new Version(2, 0, 0, 0), new Version(2, 0, 1, 0),
            @"C:\RSH-old\RSH.exe", "user", "user", candidatePid: 1234, currentPid: 5678));
    }

    [Fact]
    public void ShouldTerminate_SameVersion_False()
    {
        Assert.False(SingleInstanceCoordinator.ShouldTerminateOlderInstance(
            new Version(2, 0, 1, 0), new Version(2, 0, 1, 0),
            @"C:\RSH\RSH.exe", "user", "user", 1234, 5678));
    }

    [Fact]
    public void ShouldTerminate_NewerVersion_False()
    {
        Assert.False(SingleInstanceCoordinator.ShouldTerminateOlderInstance(
            new Version(2, 0, 3, 0), new Version(2, 0, 1, 0),
            @"C:\RSH\RSH.exe", "user", "user", 1234, 5678));
    }

    [Fact]
    public void ShouldTerminate_OtherUser_False()
    {
        Assert.False(SingleInstanceCoordinator.ShouldTerminateOlderInstance(
            new Version(2, 0, 0, 0), new Version(2, 0, 1, 0),
            @"C:\RSH-old\RSH.exe", "other-user", "user", 1234, 5678));
    }

    [Fact]
    public void ShouldTerminate_UnknownOwner_False_FailClosed()
    {
        // Owner maloom na ho (access fail) to kabhi kill nahi — fail-closed.
        Assert.False(SingleInstanceCoordinator.ShouldTerminateOlderInstance(
            new Version(2, 0, 0, 0), new Version(2, 0, 1, 0),
            @"C:\RSH-old\RSH.exe", null, "user", 1234, 5678));
        Assert.False(SingleInstanceCoordinator.ShouldTerminateOlderInstance(
            new Version(2, 0, 0, 0), new Version(2, 0, 1, 0),
            @"C:\RSH-old\RSH.exe", "", "user", 1234, 5678));
    }

    [Fact]
    public void ShouldTerminate_SamePid_False()
    {
        Assert.False(SingleInstanceCoordinator.ShouldTerminateOlderInstance(
            new Version(2, 0, 0, 0), new Version(2, 0, 1, 0),
            @"C:\RSH\RSH.exe", "user", "user", 1234, 1234));
    }

    [Fact]
    public void ShouldTerminate_OtherExeName_False()
    {
        Assert.False(SingleInstanceCoordinator.ShouldTerminateOlderInstance(
            new Version(2, 0, 0, 0), new Version(2, 0, 1, 0),
            @"C:\RSH-old\Other.exe", "user", "user", 1234, 5678));
    }

    [Fact]
    public void ShouldTerminate_NullFileVersion_False()
    {
        Assert.False(SingleInstanceCoordinator.ShouldTerminateOlderInstance(
            null, new Version(2, 0, 1, 0),
            @"C:\RSH-old\RSH.exe", "user", "user", 1234, 5678));
    }

    [Fact]
    public void ParseFileVersion_ValidAndInvalid()
    {
        Assert.Equal(new Version(2, 0, 0, 0), SingleInstanceCoordinator.ParseFileVersion("2.0.0.0"));
        Assert.Null(SingleInstanceCoordinator.ParseFileVersion(null));
        Assert.Null(SingleInstanceCoordinator.ParseFileVersion(""));
        Assert.Null(SingleInstanceCoordinator.ParseFileVersion("not-a-version"));
    }

    [Fact]
    public void TerminateOlderInstances_NonWindows_ReturnsZero()
    {
        if (OperatingSystem.IsWindows())
            Assert.Skip("Windows-only asal behaviour — Windows PC par verify hoga (process enumeration + kill).");
        var seen = new List<string>();
        int n = SingleInstanceCoordinator.TerminateOlderInstances(
            new Version(2, 0, 2, 0), seen.Add);
        Assert.Equal(0, n);
        Assert.Single(seen); // non-Windows log line aayi
    }

    // ================= 3. CloseToTray trap (source-text) =================

    [Fact]
    public void CloseToTray_OnClosing_HidesOnlyWhenTrayExists()
    {
        string src = ReadRepoFileOrSkip(Path.Combine("software", "src", "RSH.App", "MainWindow.xaml.cs"));
        // Guard: hide sirf jab tray asal mein bana ho.
        Assert.Contains("_trayIcon is not null", src, StringComparison.Ordinal);
        Assert.Contains("_services.Settings.CloseToTray && _trayIcon is not null", src, StringComparison.Ordinal);
        // Exit path pehle se null-safe hai.
        Assert.Contains("_trayIcon?.Unregister()", src, StringComparison.Ordinal);
        // Default abhi bhi CloseToTray=true hai (koi UX change nahi).
        string settings = ReadRepoFileOrSkip(Path.Combine("software", "src", "RSH.App", "Services", "AppSettings.cs"));
        Assert.Contains("public bool CloseToTray { get; set; } = true;", settings, StringComparison.Ordinal);
    }

    // ================= 4. Startup version log + versioned single instance (source-text) =================

    [Fact]
    public void StartupLog_VersionFromAssembly_AndVersionedSingleInstance()
    {
        string src = ReadRepoFileOrSkip(Path.Combine("software", "src", "RSH.App", "App.xaml.cs"));
        // Version assembly se — hard-code "2.0.2" kahin nahi.
        Assert.Contains("AppVersionInfo.ForAssembly(typeof(App).Assembly)", src, StringComparison.Ordinal);
        Assert.Contains("RSH started v{DisplayVersion} (assembly {AssemblyVersion}", src, StringComparison.Ordinal);
        Assert.DoesNotContain("\"2.0.2\"", src, StringComparison.Ordinal);
        // Versioned mutex/pipe + purane instance ka terminate, TryAcquire se pehle.
        Assert.Contains("appVersion.MutexName, appVersion.PipeName", src, StringComparison.Ordinal);
        Assert.Contains("TerminateOlderInstances(", src, StringComparison.Ordinal);
        Assert.Contains("SignalExisting(appVersion.PipeName)", src, StringComparison.Ordinal);
        Assert.True(src.IndexOf("TerminateOlderInstances(", StringComparison.Ordinal)
            < src.IndexOf("TryAcquire()", StringComparison.Ordinal),
            "Purane instance ka terminate TryAcquire se pehle hona chahiye.");
        // Autostart stale-entry repair bhi startup mein hai.
        Assert.Contains("EnsureAutostartCurrent(", src, StringComparison.Ordinal);
    }

    // ================= 5. Autostart stale-entry repair =================

    [Fact]
    public void Autostart_IsStaleValue_OnlyWhenPresentAndDifferent()
    {
        Assert.True(AutostartService.IsStaleValue(
            "\"C:\\RSH-old\\RSH.exe\" --minimized", "\"C:\\RSH-new\\RSH.exe\" --minimized"));
        Assert.False(AutostartService.IsStaleValue(
            "\"C:\\RSH\\RSH.exe\" --minimized", "\"C:\\RSH\\RSH.exe\" --minimized"));
        Assert.False(AutostartService.IsStaleValue(null, "\"C:\\RSH\\RSH.exe\" --minimized"));
        Assert.False(AutostartService.IsStaleValue("\"C:\\RSH\\RSH.exe\" --minimized", null));
    }

    [Fact]
    public void Autostart_RepairStaleEntry_OverwritesOldPath()
    {
        var store = new FakeStore { Value = "\"C:\\RSH-old\\RSH.exe\" --minimized" };
        var svc = new AutostartService(store, () => @"C:\RSH-new\RSH.exe");
        Assert.True(svc.RepairStaleEntry(out bool repaired, out string? error), error);
        Assert.True(repaired);
        Assert.Equal("\"C:\\RSH-new\\RSH.exe\" --minimized", store.Value);
    }

    [Fact]
    public void Autostart_RepairStaleEntry_CurrentEntry_Untouched()
    {
        var store = new FakeStore { Value = "\"C:\\RSH\\RSH.exe\" --minimized" };
        var svc = new AutostartService(store, () => @"C:\RSH\RSH.exe");
        Assert.True(svc.RepairStaleEntry(out bool repaired, out _));
        Assert.False(repaired);
        Assert.Equal("\"C:\\RSH\\RSH.exe\" --minimized", store.Value);
    }

    [Fact]
    public void Autostart_RepairStaleEntry_NoEntry_NothingToDo()
    {
        var store = new FakeStore();
        var svc = new AutostartService(store, () => @"C:\RSH\RSH.exe");
        Assert.True(svc.RepairStaleEntry(out bool repaired, out _));
        Assert.False(repaired);
        Assert.Null(store.Value);
    }

    [Fact]
    public void Autostart_EnsureCurrent_CreatesMissingEntry_WhenDesiredOn()
    {
        var store = new FakeStore();
        var svc = new AutostartService(store, () => @"C:\RSH\RSH.exe");
        Assert.True(svc.EnsureAutostartCurrent(true, out bool changed, out string? error), error);
        Assert.True(changed);
        Assert.Equal("\"C:\\RSH\\RSH.exe\" --minimized", store.Value);
    }

    [Fact]
    public void Autostart_EnsureCurrent_DesiredOff_DoesNothing()
    {
        var store = new FakeStore();
        var svc = new AutostartService(store, () => @"C:\RSH\RSH.exe");
        Assert.True(svc.EnsureAutostartCurrent(false, out bool changed, out _));
        Assert.False(changed);
        Assert.Null(store.Value);
    }

    [Fact]
    public void Autostart_RepairStaleEntry_WriteThrows_FailsWithoutBlindChange()
    {
        var store = new FakeStore { Value = "\"C:\\RSH-old\\RSH.exe\" --minimized", ThrowOnWrite = true };
        var svc = new AutostartService(store, () => @"C:\RSH-new\RSH.exe");
        Assert.False(svc.RepairStaleEntry(out bool repaired, out string? error));
        Assert.False(repaired);
        Assert.NotNull(error);
        Assert.Equal("\"C:\\RSH-old\\RSH.exe\" --minimized", store.Value);
    }
}
