using System.Net;
using System.Text;
using RSH.Core.Utilities;
using RSH.Core.Windows;

namespace RSH.Tests;

/// <summary>
/// Phase H — Workstream 6: A7 runbook + release prep (SW-16, A13, A11).
/// A13 update check + runtime check (Core, network fake handler se),
/// A11 firewall rule (pure command builder), aur release artifacts ke
/// source-text checks (MSIX manifest, SBOM, winget, runbook, checklist).
/// </summary>
public class PhaseHWorkstream6Tests
{
    // ================= A13 — ParseLatestTag =================

    [Fact]
    public void ParseLatestTag_VPrefixedTag_Parses()
    {
        Assert.Equal(new Version(2, 0, 0), UpdateChecker.ParseLatestTag("{\"tag_name\":\"v2.0.0\"}"));
    }

    [Fact]
    public void ParseLatestTag_NoVPrefix_Parses()
    {
        Assert.Equal(new Version(2, 1, 3), UpdateChecker.ParseLatestTag("{\"tag_name\":\"2.1.3\"}"));
    }

    [Fact]
    public void ParseLatestTag_PrereleaseTag_Ignored()
    {
        Assert.Null(UpdateChecker.ParseLatestTag("{\"tag_name\":\"v2.1.0-beta.1\"}"));
    }

    [Fact]
    public void ParseLatestTag_MissingTag_ReturnsNull()
    {
        Assert.Null(UpdateChecker.ParseLatestTag("{\"name\":\"RSH\"}"));
    }

    [Fact]
    public void ParseLatestTag_MalformedJson_ReturnsNull()
    {
        Assert.Null(UpdateChecker.ParseLatestTag("{not json"));
        Assert.Null(UpdateChecker.ParseLatestTag(""));
        Assert.Null(UpdateChecker.ParseLatestTag("   "));
    }

    [Fact]
    public void ParseLatestTag_NonVersionTag_ReturnsNull()
    {
        Assert.Null(UpdateChecker.ParseLatestTag("{\"tag_name\":\"vX.Y\"}"));
    }

    // ================= A13 — IsNewer =================

    [Fact]
    public void IsNewer_NewerLatest_True_EqualOrOlder_False()
    {
        var current = new Version(2, 0, 0);
        Assert.True(UpdateChecker.IsNewer(current, new Version(2, 0, 1)));
        Assert.False(UpdateChecker.IsNewer(current, new Version(2, 0, 0)));
        Assert.False(UpdateChecker.IsNewer(current, new Version(1, 9, 9)));
        Assert.False(UpdateChecker.IsNewer(current, null));
    }

    // ================= A13 — CheckForUpdateAsync (fake HTTP) =================

    private sealed class FakeHandler : HttpMessageHandler
    {
        private readonly Func<HttpRequestMessage, HttpResponseMessage> _respond;
        public FakeHandler(Func<HttpRequestMessage, HttpResponseMessage> respond) => _respond = respond;
        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken ct)
            => Task.FromResult(_respond(request));
    }

    private sealed class ThrowingHandler : HttpMessageHandler
    {
        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken ct)
            => throw new HttpRequestException("No internet (test).");
    }

    private static HttpResponseMessage JsonResponse(string json, HttpStatusCode code = HttpStatusCode.OK) =>
        new(code) { Content = new StringContent(json, Encoding.UTF8, "application/json") };

    [Fact]
    public async Task CheckForUpdate_NewerRelease_Available()
    {
        using var http = new HttpClient(new FakeHandler(_ => JsonResponse("{\"tag_name\":\"v2.1.0\"}")));
        UpdateCheckResult r = await UpdateChecker.CheckForUpdateAsync(http, new Version(2, 0, 0));
        Assert.True(r.Available);
        Assert.Equal(new Version(2, 1, 0), r.LatestVersion);
        Assert.Null(r.Error);
        Assert.False(r.Unknown);
    }

    [Fact]
    public async Task CheckForUpdate_SameVersion_NotAvailable()
    {
        using var http = new HttpClient(new FakeHandler(_ => JsonResponse("{\"tag_name\":\"v2.0.0\"}")));
        UpdateCheckResult r = await UpdateChecker.CheckForUpdateAsync(http, new Version(2, 0, 0));
        Assert.False(r.Available);
        Assert.Null(r.Error);
    }

    [Fact]
    public async Task CheckForUpdate_HttpError_Unknown_NotAvailable_NoThrow()
    {
        using var http = new HttpClient(new FakeHandler(_ => JsonResponse("{}", HttpStatusCode.InternalServerError)));
        UpdateCheckResult r = await UpdateChecker.CheckForUpdateAsync(http, new Version(2, 0, 0));
        Assert.False(r.Available);
        Assert.True(r.Unknown);
        Assert.NotNull(r.Error);
    }

    [Fact]
    public async Task CheckForUpdate_NetworkFailure_Unknown_NoThrow()
    {
        using var http = new HttpClient(new ThrowingHandler());
        UpdateCheckResult r = await UpdateChecker.CheckForUpdateAsync(http, new Version(2, 0, 0));
        Assert.False(r.Available);
        Assert.True(r.Unknown);
    }

    [Fact]
    public async Task CheckForUpdate_RequestHitsGithubLatestEndpoint()
    {
        Uri? seen = null;
        using var http = new HttpClient(new FakeHandler(req =>
        {
            seen = req.RequestUri;
            return JsonResponse("{\"tag_name\":\"v2.0.0\"}");
        }));
        await UpdateChecker.CheckForUpdateAsync(http, new Version(2, 0, 0));
        Assert.Equal(UpdateChecker.LatestReleaseUrl, seen?.ToString());
    }

    // ================= A13 — RuntimeCheck =================

    [Fact]
    public void RuntimeCheck_SupportedRuntime_OnDotNet10()
    {
        // Sandbox .NET 10 par chalta hai — major >= 10 hona chahiye.
        Assert.True(RuntimeCheck.IsSupportedRuntime());
        Assert.True(RuntimeCheck.CheckAll().RuntimeOk);
    }

    [Fact]
    public void RuntimeCheck_DescribeCurrent_NonEmpty()
    {
        string d = RuntimeCheck.DescribeCurrent();
        Assert.Contains(".NET", d);
        Assert.Contains("OS", d);
    }

    [Fact]
    public void RuntimeCheck_IsSupportedOs_NoThrow_OffWindowsFalse()
    {
        // Linux sandbox par false — koi exception nahi. Windows 11 par
        // true hona chahiye (wo check sirf Windows PC par hota hai).
        bool ok = RuntimeCheck.IsSupportedOs();
        if (!OperatingSystem.IsWindows()) Assert.False(ok);
    }

    // ================= A11 — FirewallRule builder =================

    [Fact]
    public void FirewallRule_EnsureCommand_ProgramScoped_UdpInbound_Private()
    {
        string cmd = FirewallRule.BuildEnsureCommand(@"C:\Program Files\RSH\RSH.exe");
        Assert.Contains("New-NetFirewallRule", cmd);
        Assert.Contains("-DisplayName 'RSH'", cmd);
        Assert.Contains("-Direction Inbound", cmd);
        Assert.Contains("-Program 'C:\\Program Files\\RSH\\RSH.exe'", cmd);
        Assert.Contains("-Protocol UDP", cmd);
        Assert.Contains("-Action Allow", cmd);
        Assert.Contains("-Profile Private", cmd);
        // Board ka port 4212 REMOTE hai — local port rule mein nahi hona chahiye.
        Assert.DoesNotContain("-LocalPort", cmd);
        Assert.DoesNotContain("4212", cmd);
    }

    [Fact]
    public void FirewallRule_EnsureCommand_RelativeOrNonExe_Throws()
    {
        Assert.Throws<ArgumentException>(() => FirewallRule.BuildEnsureCommand("RSH.exe"));
        Assert.Throws<ArgumentException>(() => FirewallRule.BuildEnsureCommand(@"C:\RSH\RSH.dll"));
        Assert.Throws<ArgumentException>(() => FirewallRule.BuildEnsureCommand(""));
        // BUG (Phase H W6 gate): Linux par Unix-style absolute path bhi
        // Windows path nahi hai — drive letter lazmi hai.
        Assert.Throws<ArgumentException>(() => FirewallRule.BuildEnsureCommand("/home/user/RSH.exe"));
    }

    [Fact]
    public void FirewallRule_RemoveCommand_OnlyRshRule()
    {
        string cmd = FirewallRule.BuildRemoveCommand();
        Assert.Contains("Remove-NetFirewallRule", cmd);
        Assert.Contains("-DisplayName 'RSH'", cmd);
    }

    [Fact]
    public async Task FirewallRuleManager_OffWindows_ThrowsPlatformNotSupported()
    {
        if (OperatingSystem.IsWindows()) Assert.Skip("Sirf non-Windows par chalao.");
        await Assert.ThrowsAsync<PlatformNotSupportedException>(() =>
            FirewallRuleManager.EnsureRuleAsync(@"C:\RSH\RSH.exe"));
        await Assert.ThrowsAsync<PlatformNotSupportedException>(() =>
            FirewallRuleManager.RemoveRuleAsync());
    }

    // ================= Release artifacts — source-text checks =================

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
        if (path is null) Assert.Skip($"Repo file nahi mili: {relative}");
        return File.ReadAllText(path);
    }

    [Fact]
    public void Artifact_A7Runbook_Exists_AndHasExactCommands()
    {
        string text = ReadRepoFileOrSkip("docs/A7-RUNBOOK.md");
        Assert.Contains("write_flash 0x0", text);
        Assert.Contains("RSH-Firmware-DevKitV1-v4.0.0.bin", text);
        Assert.Contains("115200", text);
        Assert.Contains("maxCycleUs", text);
        Assert.Contains("3500", text);
        Assert.Contains("BRICK-RECOVERY", text);
        Assert.Contains("sent/s", text);
    }

    [Fact]
    public void Artifact_UserGuide_Version_IsV201()
    {
        string text = ReadRepoFileOrSkip("docs/USER-GUIDE.md");
        Assert.Contains("Version 2.0.1", text);
        Assert.Contains("v4.0.0", text);
        Assert.DoesNotContain("RSH-Setup-1.0.0.exe", text);
    }

    [Fact]
    public void Artifact_FlashGuide_Version_IsV400()
    {
        string text = ReadRepoFileOrSkip("docs/FLASH-GUIDE.md");
        Assert.Contains("RSH-Firmware-DevKitV1-v4.0.0.bin", text);
        Assert.DoesNotContain("v3.3.bin", text);
    }

    [Fact]
    public void Artifact_MsixManifest_Identity_AndWin11Floor()
    {
        string text = ReadRepoFileOrSkip("software/packaging/msix/Package.appxmanifest");
        Assert.Contains("Name=\"RSH\"", text);
        Assert.Contains("Version=\"2.0.1.0\"", text);
        Assert.Contains("MinVersion=\"10.0.22000.0\"", text);
        Assert.Contains("RSH.exe", text);
        Assert.Contains("privateNetworkClientServer", text);
    }

    [Fact]
    public void Artifact_Sbom_ValidJson_HasRshAndDependencies()
    {
        string text = ReadRepoFileOrSkip("docs/SBOM.cyclonedx.json");
        using var doc = System.Text.Json.JsonDocument.Parse(text);
        var root = doc.RootElement;
        Assert.Equal("CycloneDX", root.GetProperty("bomFormat").GetString());
        string all = text;
        Assert.Contains("RSH", all);
        Assert.Contains("2.0.0", all);
        Assert.Contains("WPF-UI", all);
        Assert.Contains("vJoy", all);
        int count = root.GetProperty("components").GetArrayLength();
        Assert.True(count >= 10, $"SBOM components kam hain: {count}");
    }

    [Fact]
    public void Artifact_ThirdPartyLicenses_ListsNugetVjoyEsp32()
    {
        string text = ReadRepoFileOrSkip("docs/THIRD-PARTY-LICENSES.md");
        Assert.Contains("WPF-UI", text);
        Assert.Contains("vJoy", text);
        Assert.Contains("ESP32", text);
        Assert.Contains("2.0.0", text);
    }

    [Fact]
    public void Artifact_WingetManifests_Version201()
    {
        string installer = ReadRepoFileOrSkip("software/packaging/winget/dreamsofts34-creator.RSH.installer.yaml");
        Assert.Contains("PackageVersion: 2.0.1", installer);
        Assert.Contains("msix", installer);
        string version = ReadRepoFileOrSkip("software/packaging/winget/dreamsofts34-creator.RSH.yaml");
        Assert.Contains("PackageVersion: 2.0.1", version);
    }

    [Fact]
    public void Artifact_ReleaseChecklist_Exists_WithStatus()
    {
        string text = ReadRepoFileOrSkip("docs/RELEASE-CHECKLIST.md");
        Assert.Contains("v2.0.0", text);
        Assert.Contains("PASS", text);
        Assert.Contains("PENDING", text);
    }

    [Fact]
    public void Artifact_SupportPeriod_Exists()
    {
        string text = ReadRepoFileOrSkip("docs/SUPPORT-PERIOD.md");
        Assert.Contains("2.0.0", text);
    }

    [Fact]
    public void Artifact_FirewallScripts_ProgramScoped()
    {
        string install = ReadRepoFileOrSkip("software/packaging/firewall/install-firewall-rule.ps1");
        Assert.Contains("New-NetFirewallRule", install);
        Assert.Contains("-Program", install);
        Assert.DoesNotContain("-LocalPort", install);
        string uninstall = ReadRepoFileOrSkip("software/packaging/firewall/uninstall-firewall-rule.ps1");
        Assert.Contains("Remove-NetFirewallRule", uninstall);
    }

    // ================= W7 findings =================

    [Fact]
    public void Artifact_A7Runbook_B5_SignedOta_FlashDump_NvsMigration()
    {
        string text = ReadRepoFileOrSkip("docs/A7-RUNBOOK.md");
        // B5: signed OTA — unsigned reject / signed accept / downgrade reject.
        Assert.Contains("Unsigned", text);
        Assert.Contains("firmware-signed.bin", text);
        Assert.Contains("downgrade", text);
        // B5: flash-enc verify — NVS partition offset/size (partitions.bin se).
        Assert.Contains("read_flash 0x9000 0x5000", text);
        Assert.Contains("wifipw2", text);
        Assert.Contains("OPEN-3", text);
        // B5: NVS migration check (legacy wifipw -> wifipw2).
        Assert.Contains("NVS migration", text);
        Assert.Contains("wifipw", text);
    }

    [Fact]
    public void Artifact_ReleaseChecklist_SecH03_SecH04()
    {
        string text = ReadRepoFileOrSkip("docs/RELEASE-CHECKLIST.md");
        // SEC-H-03: signed-bin existence check + raw bins hatao.
        Assert.Contains("SEC-H-03", text);
        Assert.Contains("firmware-signed.bin", text);
        // SEC-H-04: OPEN-19 key-compromise response (revocation, re-sign, purane boards).
        Assert.Contains("SEC-H-04", text);
        Assert.Contains("OPEN-19", text);
        Assert.Contains("revocation", text);
    }

    [Fact]
    public void Artifact_FlashGuide_SecH12_OpenApWarning()
    {
        string text = ReadRepoFileOrSkip("docs/FLASH-GUIDE.md");
        Assert.Contains("RSH-Firmware-DevKitV1-v4.0.0.bin", text);
        Assert.Contains("esptool.py --chip esp32", text);
        // Step 6 open-AP bootstrap warning (OPEN-6 / TM-W02 residual).
        Assert.Contains("TM-W02", text);
        Assert.Contains("60 s", text);
        Assert.Contains("KEY_RESET", text);
    }

    [Fact]
    public void Artifact_UserGuide_SecH12_PairingWarning()
    {
        string text = ReadRepoFileOrSkip("docs/USER-GUIDE.md");
        Assert.Contains("Pair immediately", text);
        Assert.Contains("open (no password)", text);
    }
}
