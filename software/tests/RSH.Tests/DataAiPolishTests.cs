using System.Text;
using RSH.App.ViewModels;
using RSH.Core.Profiles;
using RSH.Core.Telemetry;

namespace RSH.Tests;

/// <summary>
/// Data &amp; AI final polish sweep (v2.0.3): profiles/settings/telemetry
/// data-integrity fixes ke tests — resolver specificity, iRacing missing
/// SessionState var, watcher stale snapshot, ProfileStore rotation/
/// version/truncation, PinMapStore tolerant load + guarded backup.
/// </summary>
public class DataAiPolishTests
{
    private static string TempDir() =>
        Path.Combine(Path.GetTempPath(), "rsh-tests", Guid.NewGuid().ToString("N"));

    private static void Cleanup(string dir)
    {
        try { if (Directory.Exists(dir)) Directory.Delete(dir, recursive: true); } catch { /* best-effort */ }
    }

    private static string ProfileJson(string name, int schemaVersion = 2) =>
        $$"""{"SchemaVersion":{{schemaVersion}},"Profiles":[{"Name":"{{name}}"}],"ActiveProfileName":"{{name}}"}""";

    private static void WriteFile(string path, string content)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        File.WriteAllText(path, content);
    }

    // ================= Resolver: game-specific beats any-game =================

    [Fact]
    public void ResolverPrefersGameSpecificCarOverAnyGame()
    {
        var profiles = new List<WheelProfile>
        {
            // Naam se pehle aane wala any-game profile — pehle yehi jeetta tha.
            new() { Name = "A Generic", CarModel = "ferrari_488_gt3", Game = GameId.Unknown },
            new() { Name = "Z AC Only", CarModel = "ferrari_488_gt3", Game = GameId.AssettoCorsa },
        };
        var snap = new GameTelemetrySnapshot(GameId.AssettoCorsa, "Ferrari_488_GT3", "", SessionState.OnTrack, DateTimeOffset.UtcNow);
        Assert.Equal("Z AC Only", CarProfileResolver.Resolve(snap, profiles)?.Name);
        // ACC snapshot par any-game hi milta hai (koi ACC-specific nahi).
        var accSnap = new GameTelemetrySnapshot(GameId.Acc, "Ferrari_488_GT3", "", SessionState.OnTrack, DateTimeOffset.UtcNow);
        Assert.Equal("A Generic", CarProfileResolver.Resolve(accSnap, profiles)?.Name);
    }

    // ================= iRacing: SessionState var na ho → Unknown =================

    private static void WriteInt(byte[] buf, int offset, int value) =>
        BitConverter.GetBytes(value).CopyTo(buf, offset);

    private static byte[] BuildIRacingMapNoSessionStateVar()
    {
        const string yaml =
            "WeekendInfo:\n  TrackName: Spa-Francorchamps\n" +
            "DriverInfo:\n  Drivers:\n  - CarIdx: 0\n    IsPlayer: 1\n    CarScreenName: Ferrari 488 GT3\n";
        int varHeaderOffset = 112;
        int bufLen = 16;
        int buf0 = varHeaderOffset + 144;
        int sessionInfoOffset = buf0 + bufLen;
        byte[] yamlBytes = Encoding.UTF8.GetBytes(yaml);
        var map = new byte[sessionInfoOffset + yamlBytes.Length];
        WriteInt(map, 0, 1);   // ver
        WriteInt(map, 4, 1);   // status: connected
        WriteInt(map, 16, yamlBytes.Length);
        WriteInt(map, 20, sessionInfoOffset);
        WriteInt(map, 24, 1);  // numVars
        WriteInt(map, 28, varHeaderOffset);
        WriteInt(map, 32, 1);  // numBuf
        WriteInt(map, 36, bufLen);
        WriteInt(map, 48, 100); WriteInt(map, 52, buf0);
        // Var header: koi aur var (SessionState nahi) — type int, offset 0.
        WriteInt(map, varHeaderOffset, 2);
        WriteInt(map, varHeaderOffset + 4, 0);
        WriteInt(map, varHeaderOffset + 8, 1);
        Encoding.ASCII.GetBytes("SessionTime").CopyTo(map, varHeaderOffset + 16);
        WriteInt(map, buf0, 4); // buffer mein "Racing" jaisi value bhi ho — var na ho to nahi parhni
        yamlBytes.CopyTo(map, sessionInfoOffset);
        return map;
    }

    [Fact]
    public void IRacingReaderMissingSessionStateVarIsUnknownNotMenu()
    {
        var map = BuildIRacingMapNoSessionStateVar();
        var reader = new IRacingSharedMemoryReader(new FakeMemoryMapAccessor(IRacingSharedMemoryReader.MapName, map), _ => true);
        Assert.True(reader.TryRead(out var snap));
        Assert.Equal("ferrari 488 gt3", snap.CarModel);
        Assert.Equal(SessionState.Unknown, snap.State); // pehle khamoshi se Menu milta tha
        Assert.False(snap.IsSwitchable);
    }

    // ================= Watcher: game band → stable snapshot saaf =================

    private sealed class ManualTimeProvider : TimeProvider
    {
        private DateTimeOffset _now = new(2026, 11, 13, 12, 0, 0, TimeSpan.Zero);
        public override DateTimeOffset GetUtcNow() => _now;
        public void Advance(TimeSpan d) => _now += d;
    }

    private sealed class StubReader : ITelemetryReader
    {
        public GameId Game => GameId.AssettoCorsa;
        public GameTelemetrySnapshot? Next;
        public bool TryRead(out GameTelemetrySnapshot snapshot)
        {
            if (Next is null) { snapshot = default!; return false; }
            snapshot = Next;
            return true;
        }
    }

    private static GameTelemetrySnapshot Snap(string car) =>
        new(GameId.AssettoCorsa, car, "Spa", SessionState.OnTrack, DateTimeOffset.UtcNow);

    [Fact]
    public void WatcherClearsStableSnapshotWhenGameClosesAndReemitsOnReturn()
    {
        var time = new ManualTimeProvider();
        var stub = new StubReader { Next = Snap("ferrari_488_gt3") };
        using var watcher = new TelemetryWatcher([stub], time);
        var emitted = new List<GameTelemetrySnapshot>();
        watcher.SnapshotChanged += emitted.Add;
        watcher.PollOnce();
        time.Advance(TimeSpan.FromSeconds(5));
        watcher.PollOnce();
        Assert.Single(emitted);
        Assert.NotNull(watcher.LatestStableSnapshot);
        Assert.NotNull(watcher.LatestRawSnapshot); // game chal raha hai — raw bhi hai

        stub.Next = null; // game band
        watcher.PollOnce();
        Assert.Null(watcher.LatestStableSnapshot); // pehle purani car rehti thi
        Assert.Null(watcher.LatestRawSnapshot); // Round 2 W7-1: raw bhi saaf — warna "current car" button purani car bharta rehta

        stub.Next = Snap("ferrari_488_gt3"); // wahi car dobara
        watcher.PollOnce();
        time.Advance(TimeSpan.FromSeconds(5));
        watcher.PollOnce();
        Assert.Equal(2, emitted.Count); // dobara emit — warna stable snapshot kabhi wapas na aata
        Assert.NotNull(watcher.LatestStableSnapshot);
    }

    // ================= ProfileStore: rotation / version / truncation =================

    [Fact]
    public void SaveDoesNotTruncateCallerProfileList()
    {
        string dir = TempDir();
        try
        {
            string file = Path.Combine(dir, "profiles.json");
            var data = new ProfileFile
            {
                Profiles = Enumerable.Range(0, 65).Select(i => WheelProfile.CreateDefault("P" + i)).ToList(),
                ActiveProfileName = "P0",
            };
            new ProfileStore(file).Save(data);
            Assert.Equal(65, data.Profiles.Count); // pehle caller ki list 64 par kat jati thi
            var reloaded = new ProfileStore(file).Load();
            Assert.Equal(64, reloaded.Profiles.Count); // disk par cap phir bhi lagti hai
        }
        finally { Cleanup(dir); }
    }

    [Fact]
    public void LoadRejectsNegativeSchemaVersion()
    {
        string dir = TempDir();
        try
        {
            string file = Path.Combine(dir, "profiles.json");
            WriteFile(file, ProfileJson("Bad", schemaVersion: -1));
            var loaded = new ProfileStore(file).Load();
            Assert.Equal("Default", loaded.Profiles[0].Name); // manfi file backup/defaults par girti hai
        }
        finally { Cleanup(dir); }
    }

    [Fact]
    public void SaveDoesNotPreserveCorruptFileAsV1Backup()
    {
        string dir = TempDir();
        try
        {
            string file = Path.Combine(dir, "profiles.json");
            WriteFile(file, "{ this is not json");
            var data = new ProfileFile
            {
                Profiles = [WheelProfile.CreateDefault()],
                ActiveProfileName = "Default",
            };
            new ProfileStore(file).Save(data);
            Assert.False(File.Exists(file + ".v1.bak")); // pehle kharab bytes .v1.bak ban jate thay
            Assert.True(File.Exists(file + ".corrupt-" + DateTime.UtcNow.ToString("yyyyMMdd")));
        }
        finally { Cleanup(dir); }
    }

    [Fact]
    public void CorruptBak1DoesNotOverwriteGoodBak2()
    {
        string dir = TempDir();
        try
        {
            string file = Path.Combine(dir, "profiles.json");
            WriteFile(file, ProfileJson("Main"));
            WriteFile(file + ".bak", ProfileJson("Bak"));
            WriteFile(file + ".bak1", "{ corrupt");
            WriteFile(file + ".bak2", ProfileJson("Bak2"));
            var data = new ProfileFile
            {
                Profiles = [WheelProfile.CreateDefault("New")],
                ActiveProfileName = "New",
            };
            new ProfileStore(file).Save(data);
            // .bak2 mein abhi bhi "Bak2" hona chahiye — pehle kharab .bak1 use overwrite kar deta tha.
            var bak2 = new ProfileStore(file + ".bak2").Load();
            Assert.Equal("Bak2", bak2.Profiles[0].Name);
            // .bak1 mein ab purani achi .bak ("Bak") aayi — kharab generation aage nahi barhi.
            var bak1 = new ProfileStore(file + ".bak1").Load();
            Assert.Equal("Bak", bak1.Profiles[0].Name);
        }
        finally { Cleanup(dir); }
    }

    // ================= PinMapStore: tolerant load + guarded backup =================

    [Fact]
    public void PinMapLoadToleratesOneBadEntry()
    {
        string dir = TempDir();
        try
        {
            var store = new PinMapStore(dir);
            Directory.CreateDirectory(dir);
            File.WriteAllText(store.FilePath,
                """{"Steering":34,"Brake":"abc","Horn":999,"NoSuchFunction":5,"Clutch":null}""");
            var map = store.Load();
            Assert.Equal(34, map["Steering"]);   // achi entry mehfooz
            Assert.Equal(36, map["Brake"]);      // kharab entry → default (pehle poora map defaults hota tha)
            Assert.Equal(4, map["Horn"]);        // range se bahir → default
            Assert.Null(map["Clutch"]);          // null theek se load hota hai
            Assert.Equal(15, map.Count);         // na-maloom function nahi aata
        }
        finally { Cleanup(dir); }
    }

    [Fact]
    public void PinMapLoadRejectsOversizeFile()
    {
        string dir = TempDir();
        try
        {
            var store = new PinMapStore(dir);
            Directory.CreateDirectory(dir);
            File.WriteAllText(store.FilePath, new string(' ', (int)PinMapStore.MaxFileBytes + 1));
            var map = store.Load();
            Assert.Equal(PinMap.Defaults()["Steering"], map["Steering"]);
        }
        finally { Cleanup(dir); }
    }

    [Fact]
    public void PinMapSaveKeepsGuardedBackup()
    {
        string dir = TempDir();
        try
        {
            var store = new PinMapStore(dir);
            // Horn (digital) — 25/26/22 khaali GPIO hain, kisi default se duplicate nahi.
            var map1 = PinMap.Defaults(); map1["Horn"] = 25;
            var map2 = PinMap.Defaults(); map2["Horn"] = 26;
            var map3 = PinMap.Defaults(); map3["Horn"] = 22;
            store.Save(map1);
            store.Save(map2);
            Assert.True(PinMapStore.TryParseFile(store.FilePath + ".bak", out var bak));
            Assert.Equal(25, bak!["Horn"]); // .bak mein map1
            // Current file kharab kar ke save karo — .bak kharab se overwrite nahi honi chahiye.
            File.WriteAllText(store.FilePath, "{ corrupt");
            store.Save(map3);
            Assert.True(PinMapStore.TryParseFile(store.FilePath + ".bak", out var bak2));
            Assert.Equal(25, bak2!["Horn"]);
            Assert.Equal(22, store.Load()["Horn"]);
        }
        finally { Cleanup(dir); }
    }
}
