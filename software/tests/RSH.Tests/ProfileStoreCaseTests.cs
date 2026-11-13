using System.Collections.Concurrent;
using System.Text.Json;
using RSH.Core.Profiles;

namespace RSH.Tests;

/// <summary>
/// F1 — P6 (SW-5) verification: active-profile match case-insensitive
/// + Save lock. ProfileStore.cs ko haath nahi lagaya — sirf tests.
/// Ye tests P6 ke contract ko define karte hain; agar Backend ki
/// implementation se pehle chalaye jayen to fail hona P6 ka bug hai.
/// </summary>
public class ProfileStoreCaseTests
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

    private static ProfileFile TwoProfiles(string activeName) => new()
    {
        Profiles =
        [
            WheelProfile.CreateDefault() with { Deadzone = 0.07 },
            WheelProfile.CreateDefault("Race") with { Smoothing = 0.9 },
        ],
        ActiveProfileName = activeName,
    };

    // ---- P6a: GetActive case-insensitive ----

    [Theory]
    [InlineData("race")]
    [InlineData("RACE")]
    [InlineData("rAcE")]
    [InlineData("Race")]
    public void GetActive_CaseMismatch_ReturnsCorrectProfile(string activeName)
    {
        var data = TwoProfiles(activeName);
        var active = ProfileStore.GetActive(data);
        Assert.Equal("Race", active.Name);
        Assert.Equal(0.9, active.Smoothing, 6);
    }

    [Fact]
    public void GetActive_CaseMismatch_WhenTargetIsNotFirst_ReturnsTargetNotFirst()
    {
        // "Default" doosri position par hai — case-sensitive fallback
        // (Profiles[0]) ghalat jawab dega (Race), match sahi dega.
        var data = new ProfileFile
        {
            Profiles =
            [
                WheelProfile.CreateDefault("Race") with { Smoothing = 0.9 },
                WheelProfile.CreateDefault() with { Deadzone = 0.07 },
            ],
            ActiveProfileName = "DEFAULT",
        };
        var active = ProfileStore.GetActive(data);
        Assert.Equal("Default", active.Name);
        Assert.Equal(0.07, active.Deadzone, 6);
    }

    [Fact]
    public void GetActive_UnknownName_FallsBackToFirst()
    {
        var data = TwoProfiles("NoSuchProfile");
        Assert.Equal("Default", ProfileStore.GetActive(data).Name);
    }

    // ---- P6a: Save → Load round-trip mein case nahi haarna chahiye ----

    [Theory]
    [InlineData("race")]
    [InlineData("RACE")]
    [InlineData("DEFAULT")]
    public void SaveLoad_ActiveNameCase_RoundtripsToSameProfile(string activeName)
    {
        string file = TempFile();
        try
        {
            var store = new ProfileStore(file);
            var expected = activeName.Equals("default", StringComparison.OrdinalIgnoreCase)
                ? "Default" : "Race";
            store.Save(TwoProfiles(activeName));

            var loaded = store.Load();
            Assert.Equal(expected, ProfileStore.GetActive(loaded).Name);
            // Stored naam ya to jaisa tha waisa rahe, ya canonical
            // profile naam par normalize ho — case ke siwa farq na ho.
            Assert.Equal(expected, loaded.ActiveProfileName, ignoreCase: true);
        }
        finally { Cleanup(file); }
    }

    [Fact]
    public void SaveLoad_ActiveLowercaseSecondProfile_NotResetToFirst()
    {
        // Regression: Load ka case-sensitive check ActiveProfileName ko
        // khamoshi se Profiles[0] ("Default") par reset kar deta tha,
        // jab ke asal active "Race" tha.
        string file = TempFile();
        try
        {
            var store = new ProfileStore(file);
            store.Save(TwoProfiles("race"));
            var loaded = store.Load();
            Assert.Equal("Race", ProfileStore.GetActive(loaded).Name);
            Assert.Equal(0.9, ProfileStore.GetActive(loaded).Smoothing, 6);
        }
        finally { Cleanup(file); }
    }

    // ---- P6a: duplicate naam alag case mein ----

    [Fact]
    public void Load_DuplicateNamesDifferentCase_DedupedToOne()
    {
        string file = TempFile();
        try
        {
            var store = new ProfileStore(file);
            store.Save(new ProfileFile
            {
                Profiles =
                [
                    WheelProfile.CreateDefault("Race") with { Smoothing = 0.9 },
                    WheelProfile.CreateDefault("RACE") with { Smoothing = 0.5 },
                ],
                ActiveProfileName = "race",
            });
            var loaded = store.Load();
            Assert.Single(loaded.Profiles);
            Assert.Equal("Race", loaded.Profiles[0].Name);
            Assert.Equal(0.9, loaded.Profiles[0].Smoothing, 6); // pehli jeetti hai
            Assert.Equal("Race", ProfileStore.GetActive(loaded).Name);
        }
        finally { Cleanup(file); }
    }

    // ---- P6b: Save lock — concurrent Save/Load ----

    // Har Save ek alag Smoothing likhta hai (0.3 default se kabhi nahi
    // milta — taake corrupt file par defaults par girna pehchana jaye).
    private static ProfileFile SavePayload(int n) => new()
    {
        Profiles = [WheelProfile.CreateDefault() with { Smoothing = 0.11 + (n % 17) * 0.01 }],
        ActiveProfileName = "Default",
    };

    private static void AssertFileIntact(string file, IReadOnlySet<double> written)
    {
        string raw = File.ReadAllText(file);
        using var doc = JsonDocument.Parse(raw); // torn JSON → JsonException
        Assert.Equal(JsonValueKind.Object, doc.RootElement.ValueKind);
        var loaded = new ProfileStore(file).Load();
        Assert.Single(loaded.Profiles);
        Assert.Contains(loaded.Profiles[0].Smoothing, written);
        Assert.False(File.Exists(file + ".tmp")); // aadhi save ka kachra nahi
    }

    [Fact]
    public async Task ConcurrentSave_SameStore_NoCorruptionOrException()
    {
        string file = TempFile();
        var written = new HashSet<double>();
        for (int n = 0; n < 400; n++) written.Add(0.11 + (n % 17) * 0.01);
        var errors = new ConcurrentBag<Exception>();
        try
        {
            var store = new ProfileStore(file);
            var tasks = Enumerable.Range(0, 8).Select(taskId => Task.Run(() =>
            {
                try
                {
                    for (int i = 0; i < 50; i++)
                        store.Save(SavePayload(taskId * 50 + i));
                }
                catch (Exception ex) { errors.Add(ex); }
            })).ToArray();
            await Task.WhenAll(tasks);
            Assert.Empty(errors);
            AssertFileIntact(file, written);
        }
        finally { Cleanup(file); }
    }

    [Fact]
    public async Task ConcurrentSave_MultipleStoresSameFile_NoCorruptionOrException()
    {
        // Sakht shart: lock sirf ek instance ke andar ho to ye fail
        // hoga — alag stores ek hi file par Save kar rahe hain. Lock
        // file-path (ya global static) par hona chahiye.
        string file = TempFile();
        var written = new HashSet<double>();
        for (int n = 0; n < 400; n++) written.Add(0.11 + (n % 17) * 0.01);
        var errors = new ConcurrentBag<Exception>();
        try
        {
            var tasks = Enumerable.Range(0, 8).Select(taskId => Task.Run(() =>
            {
                try
                {
                    var store = new ProfileStore(file);
                    for (int i = 0; i < 50; i++)
                        store.Save(SavePayload(taskId * 50 + i));
                }
                catch (Exception ex) { errors.Add(ex); }
            })).ToArray();
            await Task.WhenAll(tasks);
            Assert.Empty(errors);
            AssertFileIntact(file, written);
        }
        finally { Cleanup(file); }
    }

    [Fact]
    public async Task ConcurrentSaveLoad_Mixed_NoThrowAndFinalLoadValid()
    {
        string file = TempFile();
        var written = new HashSet<double>();
        for (int n = 0; n < 200; n++) written.Add(0.11 + (n % 17) * 0.01);
        var errors = new ConcurrentBag<Exception>();
        try
        {
            var store = new ProfileStore(file);
            store.Save(SavePayload(0));
            var tasks = new List<Task>();
            for (int t = 0; t < 4; t++)
            {
                int taskId = t;
                tasks.Add(Task.Run(() =>
                {
                    try
                    {
                        for (int i = 0; i < 50; i++)
                            store.Save(SavePayload(taskId * 50 + i));
                    }
                    catch (Exception ex) { errors.Add(ex); }
                }, TestContext.Current.CancellationToken));
                tasks.Add(Task.Run(() =>
                {
                    try
                    {
                        for (int i = 0; i < 50; i++)
                        {
                            var loaded = store.Load();
                            Assert.NotEmpty(loaded.Profiles);
                            Assert.False(string.IsNullOrEmpty(loaded.ActiveProfileName));
                        }
                    }
                    catch (Exception ex) { errors.Add(ex); }
                }, TestContext.Current.CancellationToken));
            }
            await Task.WhenAll(tasks);
            Assert.Empty(errors);
            AssertFileIntact(file, written);
        }
        finally { Cleanup(file); }
    }
}
