using System.Buffers.Binary;
using System.IO.Compression;
using System.Text.Json;
using RSH.App.ViewModels;
using RSH.Core.Pipeline;
using RSH.Core.Profiles;
using RSH.Core.Protocol;
using RSH.Core.Security;
using RSH.Core.Transport;
using RSH.Core.Utilities;

namespace RSH.Tests;

/// <summary>
/// Phase F Workstream 2 (Cybersecurity): SEC-E-02/05/07/08/11/12/13/14 +
/// SEC-C-15/18 + W8 restore gaps. Rate-limit tests WS5 ke saath ek hi
/// collection mein hain (static process-wide limiter — parallel run mein
/// ek doosre ko rate-limit na karein).
/// </summary>
[Collection("SetPasswordRateLimit")]
public class PhaseFWorkstream2Tests : IDisposable
{
    private static readonly byte[] TestKey = Enumerable.Range(0, 32).Select(i => (byte)i).ToArray();
    private static readonly byte[] TestNonce = Enumerable.Range(0, 16).Select(i => (byte)(0xA0 + i)).ToArray();
    private static readonly byte[] TestMac = { 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF };

    public PhaseFWorkstream2Tests()
    {
        ControlClient.ResetSetPasswordRateLimitForTest();
    }

    public void Dispose()
    {
        ControlClient.ResetSetPasswordRateLimitForTest();
    }

    private sealed class FakeControlTransport : IControlTransport
    {
        private readonly Func<byte[], byte[]?> _responder;
        public List<byte[]> CapturedFrames { get; } = new();
        public FakeControlTransport(Func<byte[], byte[]?> responder) => _responder = responder;
        public Task<byte[]?> ExchangeAsync(byte[] frame, TimeSpan timeout, CancellationToken cancellationToken)
        {
            CapturedFrames.Add(frame.ToArray());
            return Task.FromResult(_responder(frame));
        }
        public void Dispose() { }
    }

    private static byte[] ReplyFor(byte[] request, byte[] payload, byte[] key, byte[] nonce)
    {
        ushort reqId = BinaryPrimitives.ReadUInt16LittleEndian(request.AsSpan(5, 2));
        uint counter = BinaryPrimitives.ReadUInt32LittleEndian(request.AsSpan(7, 4));
        return ControlFrameCodec.Build((byte)(request[4] | 0x80), reqId, counter, payload, key, nonce);
    }

    private static ControlClient OkClient(FakeControlTransport transport) =>
        new("192.168.4.1", TestKey, TestNonce, transport);

    private static FakeControlTransport OkTransport() =>
        new(req => ReplyFor(req, [0x00], TestKey, TestNonce));

    // ================= SEC-E-02: SET_PASSWORD rate limit =================

    [Fact]
    public async Task SetPassword_SecondCallWithin60s_RejectedWithClearError_NoFrameSent()
    {
        var now = new DateTimeOffset(2026, 11, 13, 10, 0, 0, TimeSpan.Zero);
        ControlClient.SetPasswordClockForTest = () => now;
        var transport = OkTransport();
        using var client = OkClient(transport);

        var first = await client.SetPasswordAsync("NewPassw0rd!", TestContext.Current.CancellationToken);
        Assert.True(first.Succeeded);
        Assert.Single(transport.CapturedFrames);

        var second = await client.SetPasswordAsync("NewPassw0rd!", TestContext.Current.CancellationToken);
        Assert.Null(second.ReplyPayload);
        Assert.Contains("rate limit", second.Error, StringComparison.OrdinalIgnoreCase);
        Assert.Contains("60", second.Error, StringComparison.Ordinal);
        Assert.Single(transport.CapturedFrames); // doosri call board tak gayi hi nahi
    }

    [Fact]
    public async Task SetPassword_After60s_AllowedAgain()
    {
        var now = new DateTimeOffset(2026, 11, 13, 10, 0, 0, TimeSpan.Zero);
        ControlClient.SetPasswordClockForTest = () => now;
        var transport = OkTransport();
        using var client = OkClient(transport);

        var first = await client.SetPasswordAsync("NewPassw0rd!", TestContext.Current.CancellationToken);
        Assert.True(first.Succeeded);

        now = now.AddSeconds(61);
        var second = await client.SetPasswordAsync("NewPassw0rd!", TestContext.Current.CancellationToken);
        Assert.True(second.Succeeded);
        Assert.Equal(2, transport.CapturedFrames.Count);
    }

    [Fact]
    public async Task SetPassword_InvalidPassword_DoesNotConsumeBudget()
    {
        var now = new DateTimeOffset(2026, 11, 13, 10, 0, 0, TimeSpan.Zero);
        ControlClient.SetPasswordClockForTest = () => now;
        var transport = OkTransport();
        using var client = OkClient(transport);

        await Assert.ThrowsAsync<ArgumentException>(() => client.SetPasswordAsync("short", TestContext.Current.CancellationToken));
        await Assert.ThrowsAsync<ArgumentException>(() => client.SetPasswordAsync("short", TestContext.Current.CancellationToken));
        var ok = await client.SetPasswordAsync("NewPassw0rd!", TestContext.Current.CancellationToken);
        Assert.True(ok.Succeeded); // ghalat passwords ne budget nahi khaya
        Assert.Single(transport.CapturedFrames);
    }

    [Fact]
    public async Task SetPassword_RateLimit_IsProcessWide_AcrossClientInstances()
    {
        // BoardSession har call par naya ControlClient banata hai — limit
        // sirf tab kaam karti hai jab wo instance se bahir (static) ho.
        var now = new DateTimeOffset(2026, 11, 13, 10, 0, 0, TimeSpan.Zero);
        ControlClient.SetPasswordClockForTest = () => now;
        var transport = OkTransport();
        using var client1 = OkClient(transport);
        using var client2 = OkClient(transport);

        Assert.True((await client1.SetPasswordAsync("NewPassw0rd!", TestContext.Current.CancellationToken)).Succeeded);
        var second = await client2.SetPasswordAsync("NewPassw0rd!", TestContext.Current.CancellationToken);
        Assert.Null(second.ReplyPayload);
        Assert.Contains("rate limit", second.Error, StringComparison.OrdinalIgnoreCase);
    }

    // ================= SEC-C-15: profile Name allowlist =================

    [Theory]
    [InlineData("../../evil", "evil")]
    [InlineData(@"C:\evil", "Cevil")]
    [InlineData("Race | night", "Race  night")]
    [InlineData("a/b\\c:d*e?f\"g<h>i", "abcdefghi")]
    [InlineData("  Race  ", "Race")]
    [InlineData("", "Default")]
    [InlineData("   ", "Default")]
    [InlineData("///", "Default")]
    public void SanitizeProfileName_StripsSeparatorsAndControlChars(string input, string expected)
    {
        Assert.Equal(expected, WheelProfile.SanitizeProfileName(input));
    }

    [Fact]
    public void SanitizeProfileName_TruncatesTo64Chars()
    {
        string name = new string('a', 100);
        Assert.Equal(64, WheelProfile.SanitizeProfileName(name).Length);
        Assert.Equal(new string('a', 64), WheelProfile.SanitizeProfileName(name));
    }

    [Fact]
    public void IsValidProfileName_RejectsTraversalAndSeparators()
    {
        Assert.True(WheelProfile.IsValidProfileName("GT 540"));
        Assert.True(WheelProfile.IsValidProfileName("F1 360"));
        Assert.True(WheelProfile.IsValidProfileName("Race_2-x"));
        Assert.False(WheelProfile.IsValidProfileName("../../evil"));
        Assert.False(WheelProfile.IsValidProfileName("a/b"));
        Assert.False(WheelProfile.IsValidProfileName("a:b"));
        Assert.False(WheelProfile.IsValidProfileName(new string('a', 65)));
        Assert.False(WheelProfile.IsValidProfileName(""));
        Assert.False(WheelProfile.IsValidProfileName("a\tb"));
    }

    [Fact]
    public void Sanitized_AppliesNameAllowlist()
    {
        var profile = WheelProfile.CreateDefault("../../evil");
        Assert.Equal("evil", profile.Sanitized().Name);
        Assert.Equal("Default", WheelProfile.CreateDefault("///").Sanitized().Name);
    }

    // ================= SEC-C-18: boardMac binding =================

    [Fact]
    public void MatchesBoard_StringAndBytesAgree()
    {
        var bound = WheelProfile.CreateDefault("Race") with { BoardMac = "AA:BB:CC:DD:EE:FF" };
        Assert.True(bound.MatchesBoard("AA:BB:CC:DD:EE:FF"));
        Assert.True(bound.MatchesBoard("aabbccddeeff"));
        Assert.True(bound.MatchesBoard(TestMac));
        Assert.False(bound.MatchesBoard("11:22:33:44:55:66"));
        Assert.False(bound.MatchesBoard(new byte[] { 0x11, 0x22, 0x33, 0x44, 0x55, 0x66 }));

        var unbound = WheelProfile.CreateDefault("Race");
        Assert.True(unbound.MatchesBoard("11:22:33:44:55:66"));
        Assert.True(unbound.MatchesBoard(TestMac));
        Assert.True(bound.MatchesBoard((string?)null)); // board maloom nahi = koi rok nahi
        Assert.True(bound.MatchesBoard((byte[]?)null));
    }

    [Fact]
    public void BoardBindingWarning_NullWhenOk_MessageOnMismatch()
    {
        var bound = WheelProfile.CreateDefault("Race") with { BoardMac = "AA:BB:CC:DD:EE:FF" };
        Assert.Null(WheelProfile.BoardBindingWarning(bound, "AA:BB:CC:DD:EE:FF"));
        Assert.Null(WheelProfile.BoardBindingWarning(bound, TestMac));
        Assert.Null(WheelProfile.BoardBindingWarning(bound, (string?)null));
        Assert.Null(WheelProfile.BoardBindingWarning(WheelProfile.CreateDefault("Race"), "11:22:33:44:55:66"));
        Assert.Null(WheelProfile.BoardBindingWarning(null, "11:22:33:44:55:66"));

        string? warning = WheelProfile.BoardBindingWarning(bound, "11:22:33:44:55:66");
        Assert.NotNull(warning);
        Assert.Contains("another wheel", warning, StringComparison.OrdinalIgnoreCase);
        Assert.Contains("recalibrate", warning, StringComparison.OrdinalIgnoreCase);
        string? warningBytes = WheelProfile.BoardBindingWarning(bound, new byte[] { 0x11, 0x22, 0x33, 0x44, 0x55, 0x66 });
        Assert.Equal(warning, warningBytes);
    }

    // ================= SEC-E-08: ek hi SchemaVersion authority =================

    [Fact]
    public void SchemaVersion_SingleAuthority()
    {
        Assert.Equal(2, ProfileFile.CurrentSchemaVersion);
        Assert.Equal(1, PipelinePlan.PlanSchemaVersion);
        var entries = new Dictionary<ModuleKind, ModulePlanEntry>();
        foreach (var m in PipelinePlan.AllModules)
            entries[m] = new ModulePlanEntry { Module = m };
        var plan = new PipelinePlan(entries);
        Assert.Equal(PipelinePlan.PlanSchemaVersion, plan.Version);
    }

    // ================= SEC-E-11: VDF path validation =================

    [Theory]
    [InlineData(@"C:\Steam", @"C:\Steam")]
    [InlineData(@"D:\Games\Steam", @"D:\Games\Steam")]
    [InlineData(@"C:\\Steam\\library", @"C:\Steam\library")]
    [InlineData(@"C:\Steam\.", @"C:\Steam")]
    public void VdfLibraryPath_Valid_Normalizes(string raw, string expected)
    {
        Assert.True(SteamLibraryPathValidator.TryValidateLibraryPath(raw, out string? normalized, out string? error));
        Assert.Equal(expected, normalized);
        Assert.Null(error);
    }

    [Theory]
    [InlineData(@"..\..\Windows")]
    [InlineData(@"C:\Steam\..\..\Windows")]
    [InlineData(@"C:\Steam\..\evil")]
    [InlineData(@"relative\path")]
    [InlineData(@"\\server\share\steam")]
    [InlineData(@"C:\")]
    [InlineData(@"C:\Windows")]
    [InlineData(@"C:\Windows\System32")]
    [InlineData("")]
    [InlineData("   ")]
    public void VdfLibraryPath_TraversalOrBad_Rejects(string raw)
    {
        Assert.False(SteamLibraryPathValidator.TryValidateLibraryPath(raw, out string? normalized, out string? error));
        Assert.Null(normalized);
        Assert.NotNull(error);
    }

    [Fact]
    public void VdfExeUnderLibrary_AndLaunchGate()
    {
        var libs = new List<string> { @"C:\Steam", @"D:\Games" };
        Assert.True(SteamLibraryPathValidator.IsExeUnderLibrary(@"C:\Steam\steamapps\common\assettocorsa\AssettoCorsa.exe", libs));
        Assert.True(SteamLibraryPathValidator.IsExeUnderLibrary(@"D:\Games\steamapps\common\acc\acc.exe", libs));
        Assert.False(SteamLibraryPathValidator.IsExeUnderLibrary(@"C:\Steam\steamapps\common\..\..\evil.exe", libs));
        Assert.False(SteamLibraryPathValidator.IsExeUnderLibrary(@"C:\Other\steamapps\common\game.exe", libs));
        Assert.False(SteamLibraryPathValidator.IsExeUnderLibrary(@"C:\Steam\steamapps\downloading\game.exe", libs));
        Assert.False(SteamLibraryPathValidator.IsExeUnderLibrary(null, libs));

        Assert.True(SteamLibraryPathValidator.IsPlausibleGameExePath(@"C:\Steam\steamapps\common\assettocorsa\AssettoCorsa.exe"));
        Assert.False(SteamLibraryPathValidator.IsPlausibleGameExePath(@"C:\Steam\steamapps\common\..\..\evil.exe"));
        Assert.False(SteamLibraryPathValidator.IsPlausibleGameExePath(@"relative\game.exe"));
        Assert.False(SteamLibraryPathValidator.IsPlausibleGameExePath(@"C:\Steam\steamapps\common\game.txt"));
        Assert.False(SteamLibraryPathValidator.IsPlausibleGameExePath(null));
    }

    // ================= SEC-E-07: zip-bomb guard + strict restore validation =================

    private static string TempDir()
    {
        string dir = Path.Combine(Path.GetTempPath(), "rsh-ws2-test-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(dir);
        return dir;
    }

    private static string BuildZip(string dir, params (string Name, byte[] Content)[] entries)
    {
        string zipPath = Path.Combine(dir, "backup.zip");
        using var zip = ZipFile.Open(zipPath, ZipArchiveMode.Create);
        foreach (var (name, content) in entries)
        {
            var entry = zip.CreateEntry(name);
            using var stream = entry.Open();
            stream.Write(content, 0, content.Length);
        }
        return zipPath;
    }

    private static byte[] ValidProfilesJson() =>
        System.Text.Encoding.UTF8.GetBytes(
            """{"SchemaVersion":2,"Profiles":[{"Name":"Race","Deadzone":0.05}],"ActiveProfileName":"Race"}""");

    private static byte[] ValidSettingsJson() =>
        System.Text.Encoding.UTF8.GetBytes(
            """{"Theme":"Dark","EspIp":"192.168.4.1","VJoyDeviceId":1,"LogLevel":"Warning"}""");

    [Fact]
    public void ZipGuard_ValidBackup_Extracts()
    {
        string dir = TempDir();
        string zip = BuildZip(dir,
            ("profiles.json", ValidProfilesJson()),
            ("settings.json", ValidSettingsJson()));
        string dest = Path.Combine(dir, "out");
        Assert.True(BackupZipGuard.TryExtractBackupZip(zip, dest, out string? error));
        Assert.Null(error);
        Assert.True(File.Exists(Path.Combine(dest, "profiles.json")));
        Assert.True(File.Exists(Path.Combine(dest, "settings.json")));
    }

    [Fact]
    public void ZipGuard_TraversalEntry_Rejected()
    {
        string dir = TempDir();
        string zip = BuildZip(dir,
            ("profiles.json", ValidProfilesJson()),
            ("../evil.json", [1, 2, 3]));
        Assert.False(BackupZipGuard.TryExtractBackupZip(zip, Path.Combine(dir, "out"), out string? error));
        Assert.Contains("unexpected entry", error, StringComparison.OrdinalIgnoreCase);
    }

    [Fact]
    public void ZipGuard_UnknownEntry_Rejected()
    {
        string dir = TempDir();
        string zip = BuildZip(dir,
            ("profiles.json", ValidProfilesJson()),
            ("settings.json", ValidSettingsJson()),
            ("extra.exe", [1, 2, 3]));
        Assert.False(BackupZipGuard.TryExtractBackupZip(zip, Path.Combine(dir, "out"), out string? error));
        Assert.Contains("unexpected entry", error, StringComparison.OrdinalIgnoreCase);
    }

    [Fact]
    public void ZipGuard_OversizedProfileEntry_Rejected()
    {
        string dir = TempDir();
        string zip = BuildZip(dir,
            ("profiles.json", new byte[ProfileStore.MaxFileBytes + 1]),
            ("settings.json", ValidSettingsJson()));
        Assert.False(BackupZipGuard.TryExtractBackupZip(zip, Path.Combine(dir, "out"), out string? error));
        Assert.Contains("too large", error, StringComparison.OrdinalIgnoreCase);
    }

    [Fact]
    public void ZipGuard_TooManyEntries_Rejected()
    {
        string dir = TempDir();
        var entries = Enumerable.Range(0, BackupZipGuard.MaxBackupEntries + 1)
            .Select(i => ($"extra{i}.txt", new byte[] { 1 }))
            .ToArray();
        string zip = BuildZip(dir, entries);
        Assert.False(BackupZipGuard.TryExtractBackupZip(zip, Path.Combine(dir, "out"), out string? error));
        Assert.Contains("entries", error, StringComparison.OrdinalIgnoreCase);
    }

    [Fact]
    public void ZipGuard_TotalSizeCap_Rejected()
    {
        // Har entry apni cap ke andar, lekin kul 2 MB se zyada (duplicate
        // profiles.json entries — sirf total cap pakarti hai).
        string dir = TempDir();
        var big = new byte[ProfileStore.MaxFileBytes];
        string zip = BuildZip(dir,
            ("profiles.json", big),
            ("profiles.json", big),
            ("settings.json", new byte[65_536]));
        Assert.False(BackupZipGuard.TryExtractBackupZip(zip, Path.Combine(dir, "out"), out string? error));
        Assert.Contains("too large", error, StringComparison.OrdinalIgnoreCase);
    }

    [Fact]
    public void ZipGuard_EmptyZip_Rejected()
    {
        string dir = TempDir();
        string zip = Path.Combine(dir, "empty.zip");
        using (ZipFile.Open(zip, ZipArchiveMode.Create)) { }
        Assert.False(BackupZipGuard.TryExtractBackupZip(zip, Path.Combine(dir, "out"), out string? error));
        Assert.NotNull(error);
    }

    [Fact]
    public void TryValidateFile_ValidPasses_StrictRejects()
    {
        string dir = TempDir();
        string valid = Path.Combine(dir, "profiles.json");
        File.WriteAllBytes(valid, ValidProfilesJson());
        Assert.True(ProfileStore.TryValidateFile(valid, out string? error));
        Assert.Null(error);

        // Unknown version.
        string future = Path.Combine(dir, "future.json");
        File.WriteAllText(future, """{"SchemaVersion":99,"Profiles":[{"Name":"F"}],"ActiveProfileName":"F"}""");
        Assert.False(ProfileStore.TryValidateFile(future, out _));

        // 65 profiles (> 64 cap).
        string many = Path.Combine(dir, "many.json");
        var profiles = Enumerable.Range(0, 65).Select(i => new { Name = "P" + i });
        File.WriteAllText(many, JsonSerializer.Serialize(new { SchemaVersion = 2, Profiles = profiles, ActiveProfileName = "P0" }));
        Assert.False(ProfileStore.TryValidateFile(many, out _));

        // Kharab entry (button 33+) — skip hui to restore reject (aadha restore nahi).
        string badEntry = Path.Combine(dir, "badentry.json");
        File.WriteAllText(badEntry, """{"SchemaVersion":2,"Profiles":[{"Name":"Good"},{"Name":"Bad","GearUpVJoyButton":33}],"ActiveProfileName":"Good"}""");
        Assert.False(ProfileStore.TryValidateFile(badEntry, out string? badError));
        Assert.Contains("invalid profile", badError, StringComparison.OrdinalIgnoreCase);

        // Khaali profiles list.
        string empty = Path.Combine(dir, "empty.json");
        File.WriteAllText(empty, """{"SchemaVersion":2,"Profiles":[],"ActiveProfileName":"Default"}""");
        Assert.False(ProfileStore.TryValidateFile(empty, out _));

        // Duplicate naam — dedupe se ek bachta hai, valid rehta hai (reject nahi).
        string dup = Path.Combine(dir, "dup.json");
        File.WriteAllText(dup, """{"SchemaVersion":2,"Profiles":[{"Name":"Race"},{"Name":"RACE"}],"ActiveProfileName":"Race"}""");
        Assert.True(ProfileStore.TryValidateFile(dup, out _));
    }

    // ================= SEC-E-12/13: pin-map cap + GPIO 0–39 =================

    [Fact]
    public void PinMap_MaxEntries_EqualsFunctionCount()
    {
        Assert.Equal(PinMap.MaxPinMapEntries, PinMap.Functions.Count);
    }

    [Theory]
    [InlineData(40)]
    [InlineData(100)]
    [InlineData(-1)]
    public void PinMap_GpioOutOfRange_Rejected(int pin)
    {
        var fn = PinMap.Functions.First(f => f.Name == "Shifter up");
        var (error, _) = PinMap.ValidatePin(fn, pin);
        Assert.NotNull(error);
        Assert.Contains("out of range", error, StringComparison.OrdinalIgnoreCase);
        Assert.Contains("0–39", error, StringComparison.Ordinal);
    }

    [Fact]
    public void PinMap_Gpio39_Digital_NoError()
    {
        var fn = PinMap.Functions.First(f => f.Name == "Shifter up");
        var (error, _) = PinMap.ValidatePin(fn, 39);
        Assert.Null(error); // 39 input-only hai — Digital par warning, error nahi
    }

    [Fact]
    public void PinMap_ValidateAll_Gpio40_CountsAsError()
    {
        var pins = PinMap.Defaults();
        pins["Shifter up"] = 40;
        var (_, errorCount) = PinMap.ValidateAll(pins);
        Assert.True(errorCount >= 1);
        var (rows, _) = PinMap.ValidateAll(pins);
        Assert.Contains("out of range", rows["Shifter up"].Error, StringComparison.OrdinalIgnoreCase);
    }

    [Fact]
    public void PinMap_TryValidateEntries_TooManyEntries_Rejected()
    {
        var pins = new Dictionary<string, int?>();
        for (int i = 0; i < 100; i++) pins[$"Extra {i}"] = 2;
        Assert.False(PinMap.TryValidateEntries(pins, out string? error));
        Assert.Contains("at most", error, StringComparison.OrdinalIgnoreCase);

        Assert.True(PinMap.TryValidateEntries(PinMap.Defaults(), out _));
        var oneBad = PinMap.Defaults();
        oneBad["Horn"] = 40;
        Assert.False(PinMap.TryValidateEntries(oneBad, out _));
    }

    [Fact]
    public void PinMap_Save_RejectsOutOfRange_BeforeWriting()
    {
        string dir = TempDir();
        var store = new PinMapStore(dir);
        var bad = PinMap.Defaults();
        bad["Horn"] = 40;
        Assert.Throws<ArgumentException>(() => store.Save(bad));
        Assert.False(File.Exists(store.FilePath)); // file ko haath nahi laga

        var tooMany = new Dictionary<string, int?>();
        for (int i = 0; i < 100; i++) tooMany[$"Extra {i}"] = 2;
        Assert.Throws<ArgumentException>(() => store.Save(tooMany));
        Assert.False(File.Exists(store.FilePath));
    }

    [Fact]
    public void PinMap_SaveDefaults_RoundTrips_LoadDropsOutOfRange()
    {
        string dir = TempDir();
        var store = new PinMapStore(dir);
        store.Save(PinMap.Defaults());
        var loaded = store.Load();
        Assert.Equal(34, loaded["Steering"]);

        // Haath se badli file mein GPIO 40 — Load use default par girata hai.
        File.WriteAllText(store.FilePath, """{"Steering":40,"Horn":4}""");
        var loaded2 = store.Load();
        Assert.Equal(34, loaded2["Steering"]);
        Assert.Equal(4, loaded2["Horn"]);
    }
}
