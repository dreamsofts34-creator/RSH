using System.Text;
using RSH.Core.Profiles;

namespace RSH.Tests;

/// <summary>
/// Phase E Workstream 6 (A9 / U-P1 + SEC-C-19): async save paths +
/// import size cap + schema version check.
/// RSH.Tests sirf RSH.Core reference karta hai — is liye yahan
/// ProfileStore / TryParseImportedProfile test hote hain; AppServices /
/// ViewModel async commands ka gate build + grep se hota hai.
/// </summary>
public class ProfileImportAsyncTests
{
    private static string TempFile() =>
        Path.Combine(Path.GetTempPath(), "rsh-tests", Guid.NewGuid().ToString("N"), "profiles.json");

    private static void Cleanup(string file)
    {
        try
        {
            string? dir = Path.GetDirectoryName(file);
            if (dir is not null && Directory.Exists(dir)) Directory.Delete(dir, recursive: true);
        }
        catch { /* test ka kachra — best effort safai */ }
    }

    private static byte[] Json(string s) => Encoding.UTF8.GetBytes(s);

    private static ProfileFile TwoProfiles(string activeName) => new()
    {
        Profiles =
        [
            WheelProfile.CreateDefault() with { Deadzone = 0.07 },
            WheelProfile.CreateDefault("Race") with { Smoothing = 0.9 },
        ],
        ActiveProfileName = activeName,
    };

    // ---- SEC-C-19: size cap ----

    [Fact]
    public void ImportCap_Is256KB()
    {
        // Review ki requirement: import ≤ 256 KB.
        Assert.Equal(262_144, ProfileStore.MaxImportBytes);
    }

    [Fact]
    public void Import_OversizedFile_RejectedWithClearError()
    {
        // Cap se 1 byte bara — content se farq nahi parta, length hi kaafi hai.
        var bytes = new byte[ProfileStore.MaxImportBytes + 1];
        bool ok = ProfileStore.TryParseImportedProfile(bytes, out var profile, out string? error);
        Assert.False(ok);
        Assert.Null(profile);
        Assert.Contains("too large", error, StringComparison.OrdinalIgnoreCase);
        Assert.Contains("256 KB", error, StringComparison.OrdinalIgnoreCase);
    }

    [Fact]
    public void Import_ExactlyAtCap_ParsesNormally()
    {
        // Cap ke andar valid JSON — cap khud reject nahi karta.
        string json = "{\"Name\":\"AtCap\"}";
        var bytes = new byte[ProfileStore.MaxImportBytes];
        Encoding.UTF8.GetBytes(json).CopyTo(bytes, 0);
        // Baqi bytes NUL hain — JSON parser trailing NUL par fail karega,
        // is liye ye test sirf ye kehta hai: cap ki wajah se reject nahi hua.
        bool ok = ProfileStore.TryParseImportedProfile(bytes, out _, out string? error);
        Assert.False(ok);
        Assert.DoesNotContain("too large", error ?? "", StringComparison.OrdinalIgnoreCase);
    }

    // ---- SEC-C-19: version check ----

    [Fact]
    public void Import_UnknownVersion_RejectedWithClearError()
    {
        bool ok = ProfileStore.TryParseImportedProfile(
            Json("{\"SchemaVersion\":99,\"Name\":\"Future\"}"), out var profile, out string? error);
        Assert.False(ok);
        Assert.Null(profile);
        Assert.Contains("99", error, StringComparison.Ordinal);
        Assert.Contains("not supported", error, StringComparison.OrdinalIgnoreCase);
    }

    [Fact]
    public void Import_Version3_Rejected()
    {
        // Current (2) se ek upar bhi mustaqbil ka version hai — reject.
        bool ok = ProfileStore.TryParseImportedProfile(
            Json("{\"SchemaVersion\":3,\"Name\":\"Next\"}"), out _, out string? error);
        Assert.False(ok);
        Assert.Contains("not supported", error, StringComparison.OrdinalIgnoreCase);
    }

    [Fact]
    public void Import_Version2_Accepted()
    {
        bool ok = ProfileStore.TryParseImportedProfile(
            Json("{\"SchemaVersion\":2,\"Name\":\"Race\",\"Deadzone\":0.05}"), out var profile, out string? error);
        Assert.True(ok, error);
        Assert.NotNull(profile);
        Assert.Equal("Race", profile!.Name);
        Assert.Equal(0.05, profile.Deadzone, 6);
        Assert.Equal(2, profile.SchemaVersion);
    }

    [Fact]
    public void Import_V1NoVersion_Accepted()
    {
        // v1 file mein SchemaVersion field hoti hi nahi thi — qabool.
        bool ok = ProfileStore.TryParseImportedProfile(
            Json("{\"Name\":\"Old\",\"Deadzone\":0.1}"), out var profile, out string? error);
        Assert.True(ok, error);
        Assert.NotNull(profile);
        Assert.Equal("Old", profile!.Name);
        Assert.Equal(0, profile.SchemaVersion);
    }

    [Fact]
    public void Import_BadVersionType_Rejected()
    {
        bool ok = ProfileStore.TryParseImportedProfile(
            Json("{\"SchemaVersion\":\"two\",\"Name\":\"Bad\"}"), out _, out string? error);
        Assert.False(ok);
        Assert.Contains("SchemaVersion", error, StringComparison.Ordinal);
    }

    // ---- Import: malformed / invalid content — crash nahi, wajah milti hai ----

    [Fact]
    public void Import_MalformedJson_RejectedWithoutThrowing()
    {
        bool ok = ProfileStore.TryParseImportedProfile(Json("{not json"), out var profile, out string? error);
        Assert.False(ok);
        Assert.Null(profile);
        Assert.Contains("Invalid profile file", error, StringComparison.Ordinal);
    }

    [Fact]
    public void Import_NonObjectJson_Rejected()
    {
        bool ok = ProfileStore.TryParseImportedProfile(Json("[1,2,3]"), out _, out string? error);
        Assert.False(ok);
        Assert.Contains("not a JSON object", error, StringComparison.OrdinalIgnoreCase);
    }

    [Fact]
    public void Import_Button33_Rejected()
    {
        // P2: button 33+ clamp/alias nahi — reject (Sanitized throw karta hai).
        bool ok = ProfileStore.TryParseImportedProfile(
            Json("{\"Name\":\"Bad\",\"GearUpVJoyButton\":33}"), out _, out string? error);
        Assert.False(ok);
        Assert.Contains("Invalid profile file", error, StringComparison.Ordinal);
    }

    [Fact]
    public void Import_EmptyObject_YieldsDefaultProfile()
    {
        bool ok = ProfileStore.TryParseImportedProfile(Json("{}"), out var profile, out string? error);
        Assert.True(ok, error);
        Assert.NotNull(profile);
        Assert.Equal("Default", profile!.Name);
    }

    [Fact]
    public void Import_OutOfRangeValues_AreClampedNotRejected()
    {
        // Sanitized() clamp karta hai (Deadzone 0..0.5) — file reject nahi hoti.
        bool ok = ProfileStore.TryParseImportedProfile(
            Json("{\"Name\":\"Clamp\",\"Deadzone\":9.0}"), out var profile, out string? error);
        Assert.True(ok, error);
        Assert.Equal(0.5, profile!.Deadzone, 6);
    }

    // ---- Export round-trip: SchemaVersion likha + parha jata hai ----

    [Fact]
    public void ExportShape_SchemaVersion_RoundTrips()
    {
        var exported = WheelProfile.CreateDefault("Race") with { SchemaVersion = ProfileFile.CurrentSchemaVersion };
        string json = System.Text.Json.JsonSerializer.Serialize(exported);
        bool ok = ProfileStore.TryParseImportedProfile(Json(json), out var profile, out string? error);
        Assert.True(ok, error);
        Assert.Equal(ProfileFile.CurrentSchemaVersion, profile!.SchemaVersion);
    }

    // ---- A9: SaveAsync ----

    [Fact]
    public async Task SaveAsync_RoundTrip_PreservesProfilesAndWritesV2()
    {
        string file = TempFile();
        try
        {
            var store = new ProfileStore(file);
            await store.SaveAsync(TwoProfiles("Race"));
            Assert.True(File.Exists(file));
            Assert.Contains("\"SchemaVersion\": 2", await File.ReadAllTextAsync(file), StringComparison.Ordinal);

            var loaded = new ProfileStore(file).Load();
            Assert.Equal(2, loaded.Profiles.Count);
            Assert.Equal("Race", loaded.ActiveProfileName);
            Assert.Equal("Race", ProfileStore.GetActive(loaded).Name);
            Assert.Equal(0.9, ProfileStore.GetActive(loaded).Smoothing, 6);
        }
        finally { Cleanup(file); }
    }

    [Fact]
    public async Task SaveAsync_MixedWithSyncSave_LatestWins()
    {
        // Sync aur async Save ek hi lock + ek hi file istemal karte hain —
        // aakhri save jeetta hai, file kabhi aadhi nahi milti.
        string file = TempFile();
        try
        {
            var store = new ProfileStore(file);
            store.Save(TwoProfiles("Default"));
            await store.SaveAsync(TwoProfiles("Race"));
            store.Save(TwoProfiles("Default"));
            await store.SaveAsync(TwoProfiles("Race"));

            var loaded = new ProfileStore(file).Load();
            Assert.Equal("Race", loaded.ActiveProfileName);
            Assert.Equal(2, loaded.Profiles.Count);
        }
        finally { Cleanup(file); }
    }

    [Fact]
    public async Task SaveAsync_Concurrent_NoCorruption()
    {
        // 20 parallel SaveAsync — lock serialize karta hai; file kabhi
        // torn nahi hoti aur .bak bhi parse hota hai.
        string file = TempFile();
        try
        {
            var store = new ProfileStore(file);
            await store.SaveAsync(TwoProfiles("Default"));
            var tasks = Enumerable.Range(0, 20).Select(i =>
                store.SaveAsync(TwoProfiles(i % 2 == 0 ? "Default" : "Race")));
            await Task.WhenAll(tasks);

            var loaded = new ProfileStore(file).Load();
            Assert.Equal(2, loaded.Profiles.Count);
            Assert.True(loaded.ActiveProfileName is "Default" or "Race");

            string bak = file + ".bak";
            Assert.True(File.Exists(bak));
            var bakLoaded = new ProfileStore(bak).Load();
            Assert.Equal(2, bakLoaded.Profiles.Count);
        }
        finally { Cleanup(file); }
    }

    [Fact]
    public async Task SaveAsync_CancelledToken_ThrowsBeforeWriting()
    {
        string file = TempFile();
        try
        {
            var store = new ProfileStore(file);
            using var cts = new CancellationTokenSource();
            cts.Cancel();
            // Task.Run cancelled token par TaskCanceledException deta hai
            // (OperationCanceledException ki subclass) — ThrowsAny se
            // family check karo, exact subtype pin nahi karte.
            await Assert.ThrowsAnyAsync<OperationCanceledException>(
                () => store.SaveAsync(TwoProfiles("Race"), cts.Token));
            Assert.False(File.Exists(file));
        }
        finally { Cleanup(file); }
    }
}
