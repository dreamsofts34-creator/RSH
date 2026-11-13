using System.Text;
using RSH.Core.Profiles;
using RSH.Core.Telemetry;

namespace RSH.Tests;

/// <summary>
/// Phase H — Workstream 4: per-car telemetry readers (AC + ACC + iRacing).
/// Sandbox mein asal games/shared memory nahi hain — tamam reader tests
/// fake shared-memory buffers se chalte hain (FakeMemoryMapAccessor).
/// Windows-PC verification (real game + real map) baaqi hai — dekho
/// TELEMETRY-READER.md §4 acceptance.
/// SEC-H-05 tests bhi yahin hain (TOCTOU double-read, YAML size bound,
/// process gate).
/// </summary>
public class PhaseHTelemetryTests
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

    // ================= Fake Kunos bytes (wchar_t / UTF-16LE) =================

    private static void WriteWide(byte[] buf, int offset, string value, int charCount)
    {
        byte[] raw = Encoding.Unicode.GetBytes(value);
        int n = Math.Min(raw.Length, charCount * 2 - 2); // null terminator ke liye jagah
        Array.Copy(raw, 0, buf, offset, n);
    }

    private static byte[] BuildAcStatic(string carModel, string track, string smVersion = "1.4", int totalSize = 200)
    {
        var bytes = new byte[totalSize];
        WriteWide(bytes, 0, smVersion, 15);
        WriteWide(bytes, 30, "1.0", 15);
        BitConverter.GetBytes(1).CopyTo(bytes, 60);   // numberOfSessions
        BitConverter.GetBytes(20).CopyTo(bytes, 64);  // numCars
        WriteWide(bytes, 68, carModel, 33);           // carModel @68 (wchar)
        WriteWide(bytes, 134, track, 33);             // track @134 (wchar)
        return bytes;
    }

    private static byte[] BuildGraphics(int status)
    {
        var bytes = new byte[8];
        BitConverter.GetBytes(123).CopyTo(bytes, 0); // packetId
        BitConverter.GetBytes(status).CopyTo(bytes, 4);
        return bytes;
    }

    private static FakeMemoryMapAccessor KunosMaps(string car, string track, int status, string smVersion = "1.4") =>
        new(new Dictionary<string, byte[]>
        {
            [KunosSharedMemoryReader.StaticMapName] = BuildAcStatic(car, track, smVersion),
            [KunosSharedMemoryReader.GraphicsMapName] = BuildGraphics(status),
        });

    // ================= AC reader =================

    [Fact]
    public void AcReaderParsesCapturedStatic()
    {
        // Asal AC bytes jaisi shakal: UTF-16LE wide strings, carModel @68.
        var reader = new AcSharedMemoryReader(KunosMaps("ferrari_488_gt3", "Spa", 2), _ => true);
        Assert.True(reader.TryRead(out var snap));
        Assert.Equal(GameId.AssettoCorsa, snap.Game);
        Assert.Equal("ferrari_488_gt3", snap.CarModel);
        Assert.Equal("Spa", snap.TrackName);
        Assert.Equal(SessionState.OnTrack, snap.State);
        Assert.Equal(ReaderStatus.Ok, reader.LastStatus);
        Assert.Equal("1.4", reader.LastSmVersion);
    }

    [Fact]
    public void AcReaderNormalizesCarModelCase()
    {
        var reader = new AcSharedMemoryReader(KunosMaps("Ferrari 488 GT3", "Monza", 2), _ => true);
        Assert.True(reader.TryRead(out var snap));
        Assert.Equal("ferrari 488 gt3", snap.CarModel); // snapshot: trim + lowercase
    }

    [Fact]
    public void AcReaderMapsAllStatuses()
    {
        foreach ((int raw, SessionState expected) in new[] { (0, SessionState.Menu), (1, SessionState.Replay), (2, SessionState.OnTrack), (3, SessionState.Paused), (9, SessionState.Unknown) })
        {
            var reader = new AcSharedMemoryReader(KunosMaps("ks_test", "t", raw), _ => true);
            Assert.True(reader.TryRead(out var snap));
            Assert.Equal(expected, snap.State);
        }
    }

    [Fact]
    public void AcReaderRejectsZeroedStaticMap()
    {
        // Map khula lekin smVersion khaali (zeroed) — UnsupportedVersion, kabhi ghalat offset se nahi parha.
        var maps = new FakeMemoryMapAccessor(new Dictionary<string, byte[]>
        {
            [KunosSharedMemoryReader.StaticMapName] = new byte[200],
            [KunosSharedMemoryReader.GraphicsMapName] = BuildGraphics(2),
        });
        var reader = new AcSharedMemoryReader(maps, _ => true);
        Assert.False(reader.TryRead(out _));
        Assert.Equal(ReaderStatus.UnsupportedVersion, reader.LastStatus);
    }

    [Fact]
    public void AcReaderRequiresGameProcess()
    {
        // Default gate (asal Process list): sandbox mein koi acs.exe nahi — map hone ke bawajood false.
        var reader = new AcSharedMemoryReader(KunosMaps("ferrari_488_gt3", "Spa", 2));
        Assert.False(reader.TryRead(out _));
        Assert.Equal(ReaderStatus.GameNotRunning, reader.LastStatus);
    }

    [Fact]
    public void AcReaderReturnsFalseWhenGameNotRunning()
    {
        var reader = new AcSharedMemoryReader(KunosMaps("ferrari_488_gt3", "Spa", 2), _ => false);
        Assert.False(reader.TryRead(out _));
        Assert.Equal(ReaderStatus.GameNotRunning, reader.LastStatus);
    }

    // ================= ACC reader =================

    [Fact]
    public void AccReaderRejectsWrongSize()
    {
        // Ghalat size ka map (50 B < 134 B min) → TryRead == false, kabhi crash nahi.
        var maps = new FakeMemoryMapAccessor(new Dictionary<string, byte[]>
        {
            [KunosSharedMemoryReader.StaticMapName] = new byte[50],
            [KunosSharedMemoryReader.GraphicsMapName] = BuildGraphics(2),
        });
        var reader = new AccSharedMemoryReader(maps, _ => true);
        Assert.False(reader.TryRead(out _));
    }

    [Fact]
    public void AccReaderParsesSameLayoutAsAc()
    {
        var reader = new AccSharedMemoryReader(KunosMaps("Ferrari 488 GT3 Evo", "Spa", 2), _ => true);
        Assert.True(reader.TryRead(out var snap));
        Assert.Equal(GameId.Acc, snap.Game);
        Assert.Equal("ferrari 488 gt3 evo", snap.CarModel);
    }

    // ================= SEC-H-05: TOCTOU double-read =================

    /// <summary>Har ReadBytes par alag bytes dene wala accessor — game likh raha ho jaisa.</summary>
    private sealed class SequencedAccessor : IMemoryMapAccessor
    {
        private readonly Dictionary<string, Func<int, byte[]>> _maps;
        private string? _current;
        private int _reads;

        public SequencedAccessor(Dictionary<string, Func<int, byte[]>> maps) => _maps = maps;
        public int Size => _current is null ? 0 : _maps[_current](0).Length;

        public bool TryOpen(string mapName, int minSize)
        {
            _current = null;
            _reads = 0;
            if (!_maps.TryGetValue(mapName, out var fn)) return false;
            if (fn(0).Length < minSize) return false;
            _current = mapName;
            return true;
        }

        public int ReadBytes(long offset, byte[] buffer, int count)
        {
            if (_current is null) return 0;
            byte[] bytes = _maps[_current](_reads++);
            int toRead = (int)Math.Min(Math.Min((long)bytes.Length - offset, count), buffer.Length);
            if (toRead <= 0) return 0;
            Array.Copy(bytes, (int)offset, buffer, 0, toRead);
            return toRead;
        }

        public void Dispose() => _current = null;
    }

    [Fact]
    public void KunosDoubleReadRejectsFlappingMap()
    {
        // SEC-H-05: static block har parhne par badal raha hai (A, B, A, B…)
        // → kabhi stable copy nahi, snapshot reject (aadha snapshot nahi lo).
        byte[] a = BuildAcStatic("car_a", "Spa");
        byte[] b = BuildAcStatic("car_b", "Spa");
        var accessor = new SequencedAccessor(new Dictionary<string, Func<int, byte[]>>
        {
            [KunosSharedMemoryReader.StaticMapName] = i => i % 2 == 0 ? a : b,
            [KunosSharedMemoryReader.GraphicsMapName] = _ => BuildGraphics(2),
        });
        var reader = new AcSharedMemoryReader(accessor, _ => true);
        Assert.False(reader.TryRead(out _));
        Assert.Equal(ReaderStatus.Corrupt, reader.LastStatus);
    }

    [Fact]
    public void KunosDoubleReadAcceptsOnceFlappingThenStable()
    {
        // SEC-H-05: pehli copy purani, phir stable (A, B, B) → teesri
        // copy doosri se milti hai, snapshot B wali (stable) milti hai.
        byte[] a = BuildAcStatic("car_a", "Spa");
        byte[] b = BuildAcStatic("car_b", "Spa");
        var accessor = new SequencedAccessor(new Dictionary<string, Func<int, byte[]>>
        {
            [KunosSharedMemoryReader.StaticMapName] = i => i == 0 ? a : b,
            [KunosSharedMemoryReader.GraphicsMapName] = _ => BuildGraphics(2),
        });
        var reader = new AcSharedMemoryReader(accessor, _ => true);
        Assert.True(reader.TryRead(out var snap));
        Assert.Equal("car_b", snap.CarModel);
    }

    // ================= iRacing reader =================

    private static void WriteInt(byte[] buf, int offset, int value) =>
        BitConverter.GetBytes(value).CopyTo(buf, offset);

    private static byte[] BuildIRacingMap(string yaml, int stateBuf0, int stateBuf1, int stateBuf2, int tick0 = 100, int tick1 = 99, int tick2 = 98)
    {
        int varHeaderOffset = 112;
        int bufLen = 16;
        int buf0 = varHeaderOffset + 144;
        int buf1 = buf0 + bufLen;
        int buf2 = buf1 + bufLen;
        int sessionInfoOffset = buf2 + bufLen;
        byte[] yamlBytes = Encoding.UTF8.GetBytes(yaml);
        var map = new byte[sessionInfoOffset + yamlBytes.Length];
        WriteInt(map, 0, 1);   // ver
        WriteInt(map, 4, 1);   // status: connected
        WriteInt(map, 8, 60);  // tickRate
        WriteInt(map, 12, 1);  // sessionInfoUpdate
        WriteInt(map, 16, yamlBytes.Length);
        WriteInt(map, 20, sessionInfoOffset);
        WriteInt(map, 24, 1);  // numVars
        WriteInt(map, 28, varHeaderOffset);
        WriteInt(map, 32, 3);  // numBuf
        WriteInt(map, 36, bufLen);
        WriteInt(map, 48, tick0); WriteInt(map, 52, buf0);
        WriteInt(map, 64, tick1); WriteInt(map, 68, buf1);
        WriteInt(map, 80, tick2); WriteInt(map, 84, buf2);
        // Var header: type int (2), offset 0, count 1, name @+16.
        WriteInt(map, varHeaderOffset, 2);
        WriteInt(map, varHeaderOffset + 4, 0);
        WriteInt(map, varHeaderOffset + 8, 1);
        Encoding.ASCII.GetBytes("SessionState").CopyTo(map, varHeaderOffset + 16);
        WriteInt(map, buf0, stateBuf0);
        WriteInt(map, buf1, stateBuf1);
        WriteInt(map, buf2, stateBuf2);
        yamlBytes.CopyTo(map, sessionInfoOffset);
        return map;
    }

    private const string SampleYaml =
        "WeekendInfo:\n" +
        "  TrackName: Spa-Francorchamps\n" +
        "DriverInfo:\n" +
        "  Drivers:\n" +
        "  - CarIdx: 0\n" +
        "    UserName: Alice\n" +
        "    IsPlayer: 0\n" +
        "    CarScreenName: Porsche 911 GT3 R\n" +
        "    CarPath: porsche911gt3r\n" +
        "  - CarIdx: 1\n" +
        "    UserName: Bob\n" +
        "    IsPlayer: 1\n" +
        "    CarScreenName: Ferrari 488 GT3\n" +
        "    CarPath: ferrari488gt3\n";

    [Fact]
    public void IRacingReaderParsesYamlCar()
    {
        var map = BuildIRacingMap(SampleYaml, 4, 4, 4);
        var reader = new IRacingSharedMemoryReader(new FakeMemoryMapAccessor(IRacingSharedMemoryReader.MapName, map), _ => true);
        Assert.True(reader.TryRead(out var snap));
        Assert.Equal(GameId.IRacing, snap.Game);
        Assert.Equal("ferrari 488 gt3", snap.CarModel); // player driver ki CarScreenName
        Assert.Equal("Spa-Francorchamps", snap.TrackName);
        Assert.Equal(SessionState.OnTrack, snap.State);
    }

    [Fact]
    public void IRacingReaderUsesSecondHighestTickBuffer()
    {
        // Triple-buffer: tick 100 wala buffer Racing (4), tick 99 wala
        // GetInCar (1), tick 98 wala Racing — latest MUKAMMAL buffer
        // second-highest (tick 99) hai, is liye state Loading honi chahiye.
        var map = BuildIRacingMap(SampleYaml, 4, 1, 4);
        var reader = new IRacingSharedMemoryReader(new FakeMemoryMapAccessor(IRacingSharedMemoryReader.MapName, map), _ => true);
        Assert.True(reader.TryRead(out var snap));
        Assert.Equal(SessionState.Loading, snap.State);
    }

    [Fact]
    public void IRacingReaderRejectsDisconnectedHeader()
    {
        var map = BuildIRacingMap(SampleYaml, 4, 4, 4);
        WriteInt(map, 4, 0); // status: connected bit nahi
        var reader = new IRacingSharedMemoryReader(new FakeMemoryMapAccessor(IRacingSharedMemoryReader.MapName, map), _ => true);
        Assert.False(reader.TryRead(out _));
        Assert.Equal(ReaderStatus.UnsupportedVersion, reader.LastStatus);
    }

    [Fact]
    public void IRacingReaderRejectsOversizeYaml()
    {
        // SEC-H-05: session YAML MaxSessionInfoBytes (256 KB) se bari ho to reject.
        var map = BuildIRacingMap(SampleYaml, 4, 4, 4);
        WriteInt(map, 16, IRacingSharedMemoryReader.MaxSessionInfoBytes + 1);
        var reader = new IRacingSharedMemoryReader(new FakeMemoryMapAccessor(IRacingSharedMemoryReader.MapName, map), _ => true);
        Assert.False(reader.TryRead(out _));
        Assert.Equal(ReaderStatus.UnsupportedVersion, reader.LastStatus);
    }

    [Fact]
    public void IRacingYamlParserFallsBackToCarPath()
    {
        IRacingSharedMemoryReader.ParseSessionYaml(
            "DriverInfo:\n  Drivers:\n  - CarIdx: 0\n    IsPlayer: 1\n    CarPath: dallaraf3\n",
            out string car, out _);
        Assert.Equal("dallaraf3", car);
    }

    [Fact]
    public void IRacingYamlParserHandlesTruncatedYaml()
    {
        // Aadhi YAML — kabhi throw nahi, car khaali (switch nahi hoga).
        IRacingSharedMemoryReader.ParseSessionYaml("WeekendInfo:\n  TrackName: Spa-Francor", out string car, out string track);
        Assert.Equal("", car);
        Assert.Equal("Spa-Francor", track);
    }

    // ================= Watcher (debounce) =================

    private sealed class ManualTimeProvider : TimeProvider
    {
        private DateTimeOffset _now = new(2026, 11, 13, 12, 0, 0, TimeSpan.Zero);
        public override DateTimeOffset GetUtcNow() => _now;
        public void Advance(TimeSpan d) => _now += d;
    }

    private sealed class StubReader : ITelemetryReader
    {
        public GameId Game { get; set; } = GameId.AssettoCorsa;
        public GameTelemetrySnapshot? Next { get; set; }
        public bool TryRead(out GameTelemetrySnapshot snapshot)
        {
            if (Next is null) { snapshot = default!; return false; }
            snapshot = Next;
            return true;
        }
    }

    private static GameTelemetrySnapshot Snap(string car, SessionState state, GameId game = GameId.AssettoCorsa) =>
        new(game, car, "Spa", state, DateTimeOffset.UtcNow);

    [Fact]
    public void WatcherDebouncesCarChange()
    {
        var time = new ManualTimeProvider();
        var stub = new StubReader();
        var watcher = new TelemetryWatcher([stub], time);
        var emitted = new List<GameTelemetrySnapshot>();
        watcher.SnapshotChanged += s => emitted.Add(s);

        stub.Next = Snap("car_a", SessionState.OnTrack);
        watcher.PollOnce();
        Assert.Empty(emitted); // abhi stable nahi (0 s)

        time.Advance(TimeSpan.FromSeconds(1));
        watcher.PollOnce();
        Assert.Empty(emitted); // sirf 1 s stable — 2 s chahiye

        stub.Next = Snap("car_b", SessionState.OnTrack); // 1 s ke liye badli…
        watcher.PollOnce();
        time.Advance(TimeSpan.FromSeconds(1));
        watcher.PollOnce();
        Assert.Empty(emitted); // …phir wapas — koi event nahi

        stub.Next = Snap("car_a", SessionState.OnTrack);
        watcher.PollOnce();
        time.Advance(TimeSpan.FromSeconds(2));
        watcher.PollOnce();
        Assert.Single(emitted); // 2 s stable → ek event
        Assert.Equal("car_a", emitted[0].CarModel);

        time.Advance(TimeSpan.FromSeconds(5));
        watcher.PollOnce();
        Assert.Single(emitted); // wahi snapshot — dobara event nahi
    }

    [Fact]
    public void WatcherEmitsOnlyOnChange()
    {
        var time = new ManualTimeProvider();
        var stub = new StubReader { Next = Snap("car_a", SessionState.OnTrack) };
        var watcher = new TelemetryWatcher([stub], time);
        var emitted = new List<GameTelemetrySnapshot>();
        watcher.SnapshotChanged += s => emitted.Add(s);

        watcher.PollOnce();
        time.Advance(TimeSpan.FromSeconds(3));
        watcher.PollOnce();
        Assert.Single(emitted);

        stub.Next = Snap("car_a", SessionState.Paused); // state badla → naya event (2 s baad)
        watcher.PollOnce();
        Assert.Single(emitted);
        time.Advance(TimeSpan.FromSeconds(2));
        watcher.PollOnce();
        Assert.Equal(2, emitted.Count);
        Assert.Equal(SessionState.Paused, emitted[1].State);
    }

    [Fact]
    public void WatcherIgnoresEmptyCarAndMissingGame()
    {
        var time = new ManualTimeProvider();
        var stub = new StubReader { Next = Snap("", SessionState.OnTrack) };
        var watcher = new TelemetryWatcher([stub], time);
        var emitted = new List<GameTelemetrySnapshot>();
        watcher.SnapshotChanged += s => emitted.Add(s);
        watcher.PollOnce();
        time.Advance(TimeSpan.FromSeconds(5));
        watcher.PollOnce();
        Assert.Empty(emitted); // khaali car — candidate hi nahi banta

        stub.Next = null; // game band
        watcher.PollOnce();
        Assert.Empty(emitted);
    }

    // ================= Resolver =================

    private static WheelProfile Profile(string name, string car = "", GameId game = GameId.Unknown, string process = "") =>
        new() { Name = name, CarModel = car, Game = game, GameProcessName = process };

    [Fact]
    public void ResolverPrefersCarOverGame()
    {
        var profiles = new List<WheelProfile>
        {
            Profile("Game AC", game: GameId.AssettoCorsa),
            Profile("Ferrari GT3", car: "ferrari_488_gt3", game: GameId.AssettoCorsa),
        };
        var snap = Snap("Ferrari_488_GT3", SessionState.OnTrack, GameId.AssettoCorsa);
        var resolved = CarProfileResolver.Resolve(snap, profiles);
        Assert.NotNull(resolved);
        Assert.Equal("Ferrari GT3", resolved.Name);
    }

    [Fact]
    public void ResolverMatchesSeparatorsInsensitively()
    {
        var profiles = new List<WheelProfile> { Profile("GT3", car: "ferrari_488_gt3") };
        // ACC ki shakal: spaces + capital — wahi key honi chahiye.
        var snap = Snap("Ferrari 488 GT3 Evo", SessionState.OnTrack, GameId.Acc);
        Assert.Null(CarProfileResolver.Resolve(snap, profiles)); // evo alag car hai — match nahi
        var snap2 = Snap("Ferrari 488 GT3", SessionState.OnTrack, GameId.Acc);
        Assert.Equal("GT3", CarProfileResolver.Resolve(snap2, profiles)?.Name);
    }

    [Fact]
    public void ResolverFallsBackToGameThenNull()
    {
        var profiles = new List<WheelProfile>
        {
            Profile("GT3 Ferrari", car: "ferrari_488_gt3", game: GameId.AssettoCorsa),
            Profile("AC Default", game: GameId.AssettoCorsa),
        };
        // Doosri car — car match nahi, game profile milti hai.
        Assert.Equal("AC Default", CarProfileResolver.Resolve(Snap("porsche_911", SessionState.OnTrack), profiles)?.Name);
        // Koi game profile bhi na ho → null (caller current rakhta hai).
        var onlyCar = new List<WheelProfile> { Profile("GT3 Ferrari", car: "ferrari_488_gt3", game: GameId.AssettoCorsa) };
        Assert.Null(CarProfileResolver.Resolve(Snap("porsche_911", SessionState.OnTrack), onlyCar));
        // GameProcessName se bhi game fallback milta hai (aaj ka mechanism).
        var byProcess = new List<WheelProfile> { Profile("AC via process", process: "acs") };
        Assert.Equal("AC via process", CarProfileResolver.Resolve(Snap("porsche_911", SessionState.OnTrack), byProcess)?.Name);
    }

    [Fact]
    public void ResolverFindsDuplicateCarProfiles()
    {
        var profiles = new List<WheelProfile>
        {
            Profile("B copy", car: "ferrari_488_gt3"),
            Profile("A copy", car: "Ferrari 488 GT3"),
            Profile("Other", car: "porsche_911"),
        };
        var dupes = CarProfileResolver.FindDuplicateCarProfiles(profiles);
        Assert.Single(dupes);
        Assert.Equal("B copy", dupes[0].Name); // pehli (naam se) "A copy" rehti hai
    }

    // ================= Auto-switcher (manual override) =================

    [Fact]
    public void ManualOverrideBlocksAutoSwitch()
    {
        var switcher = new ProfileAutoSwitcher { ManualOverride = true };
        var profiles = new List<WheelProfile> { Profile("Ferrari GT3", car: "ferrari_488_gt3") };
        var target = switcher.TrySwitch(Snap("ferrari_488_gt3", SessionState.OnTrack), profiles, "Default", out string reason);
        Assert.Null(target);
        Assert.Contains("Manual override", reason, StringComparison.Ordinal);

        switcher.ManualOverride = false;
        target = switcher.TrySwitch(Snap("ferrari_488_gt3", SessionState.OnTrack), profiles, "Default", out reason);
        Assert.NotNull(target);
        Assert.Equal("Ferrari GT3", target.Name);
        Assert.Contains("(auto)", reason, StringComparison.Ordinal);
    }

    [Fact]
    public void SwitcherDoesNotSwitchInMenuOrLoading()
    {
        var switcher = new ProfileAutoSwitcher();
        var profiles = new List<WheelProfile> { Profile("Ferrari GT3", car: "ferrari_488_gt3") };
        // Menu mein car badalna switch nahi karega (acceptance c).
        Assert.Null(switcher.TrySwitch(Snap("ferrari_488_gt3", SessionState.Menu), profiles, "Default", out _));
        Assert.Null(switcher.TrySwitch(Snap("ferrari_488_gt3", SessionState.Loading), profiles, "Default", out _));
        Assert.Null(switcher.TrySwitch(Snap("ferrari_488_gt3", SessionState.Replay), profiles, "Default", out _));
        Assert.Null(switcher.TrySwitch(Snap("ferrari_488_gt3", SessionState.Paused), profiles, "Default", out _));
        Assert.NotNull(switcher.TrySwitch(Snap("ferrari_488_gt3", SessionState.OnTrack), profiles, "Default", out _));
    }

    [Fact]
    public void SwitcherDoesNotSwitchWhenAlreadyActive()
    {
        var switcher = new ProfileAutoSwitcher();
        var profiles = new List<WheelProfile> { Profile("Ferrari GT3", car: "ferrari_488_gt3") };
        Assert.Null(switcher.TrySwitch(Snap("ferrari_488_gt3", SessionState.OnTrack), profiles, "Ferrari GT3", out _));
    }

    // ================= WheelProfile per-car fields =================

    [Fact]
    public void WheelProfileSanitizesCarModelAndGame()
    {
        var p = new WheelProfile { Name = "T", CarModel = "Ferrari 488 GT3/../evil", Game = (GameId)99 }.Sanitized();
        Assert.Equal("ferrari 488 gt3..evil", p.CarModel); // lowercase, '/' nikal gaya ('.' allowlist mein hai, path nahi banta)
        Assert.Equal(GameId.Unknown, p.Game); // na-maloom enum → Unknown
        Assert.DoesNotContain("/", p.CarModel, StringComparison.Ordinal);
    }

    [Fact]
    public void WheelProfileOldFileLoadsWithoutCarFields()
    {
        // Purani file (CarModel/Game nahi) — schema bump nahi, defaults aate hain.
        string json = """{"Name":"Old","GameProcessName":"acs"}""";
        Assert.True(ProfileStore.TryParseImportedProfile(Encoding.UTF8.GetBytes(json), out var profile, out _));
        Assert.NotNull(profile);
        Assert.Equal("", profile.CarModel);
        Assert.Equal(GameId.Unknown, profile.Game);
    }

    // ================= SnapshotNeverThrows =================

    [Fact]
    public void SnapshotNeverThrows()
    {
        // Map ghayab / corrupt bytes / aadhi YAML → false, koi exception bahar nahi.
        var readers = new ITelemetryReader[]
        {
            new AcSharedMemoryReader(new FakeMemoryMapAccessor(new Dictionary<string, byte[]>()), _ => true),
            new AccSharedMemoryReader(new FakeMemoryMapAccessor(new Dictionary<string, byte[]>
            {
                [KunosSharedMemoryReader.StaticMapName] = new byte[200],
                [KunosSharedMemoryReader.GraphicsMapName] = new byte[8],
            }), _ => true),
            new IRacingSharedMemoryReader(new FakeMemoryMapAccessor(IRacingSharedMemoryReader.MapName, new byte[64]), _ => true),
        };
        foreach (var r in readers)
        {
            var ex = Record.Exception(() => r.TryRead(out _));
            Assert.Null(ex);
            Assert.False(r.TryRead(out _));
        }
        // Aadhi YAML (session info kata hua) — no throw.
        var map = BuildIRacingMap("WeekendInfo:\n  TrackName: Spa", 4, 4, 4);
        var ir = new IRacingSharedMemoryReader(new FakeMemoryMapAccessor(IRacingSharedMemoryReader.MapName, map), _ => true);
        Assert.Null(Record.Exception(() => ir.TryRead(out _)));
    }

    // ================= App wiring (source-text — WPF Linux par run nahi hota) =================

    [Fact]
    public void AppServicesWiresTelemetryWatcher()
    {
        string src = ReadRepoFileOrSkip(Path.Combine("software", "src", "RSH.App", "Services", "AppServices.cs"));
        Assert.Contains("new AcSharedMemoryReader()", src, StringComparison.Ordinal);
        Assert.Contains("new AccSharedMemoryReader()", src, StringComparison.Ordinal);
        Assert.Contains("new IRacingSharedMemoryReader()", src, StringComparison.Ordinal);
        Assert.Contains("TelemetryWatcher.Start()", src, StringComparison.Ordinal);
        Assert.Contains("CarProfileAutoSwitched", src, StringComparison.Ordinal);
        Assert.Contains("AutoSwitcher.TrySwitch", src, StringComparison.Ordinal);
    }

    [Fact]
    public void ProfilesSystemWiresManualOverrideAndCurrentCar()
    {
        string src = ReadRepoFileOrSkip(Path.Combine("software", "src", "RSH.App", "ViewModels", "ProfilesSystem.cs"));
        Assert.Contains("AutoSwitcher.ManualOverride = true", src, StringComparison.Ordinal);
        Assert.Contains("ResumeAutoSwitch", src, StringComparison.Ordinal);
        Assert.Contains("FillCarFromTelemetry", src, StringComparison.Ordinal);
        Assert.Contains("AutoSwitchRuleRow(string GameProcess, string ProfileName, string CarModel)", src, StringComparison.Ordinal);
        Assert.Contains("LatestRawSnapshot", src, StringComparison.Ordinal);
    }

    [Fact]
    public void ProfilesPageShowsCarColumnAndButtons()
    {
        string xaml = ReadRepoFileOrSkip(Path.Combine("software", "src", "RSH.App", "Views", "Pages", "ProfilesPage.xaml"));
        Assert.Contains("CarLabel", xaml, StringComparison.Ordinal);
        Assert.Contains("FillCarFromTelemetryCommand", xaml, StringComparison.Ordinal);
        Assert.Contains("ResumeAutoSwitchCommand", xaml, StringComparison.Ordinal);
        Assert.Contains("AutoSwitchLocked", xaml, StringComparison.Ordinal);
    }

    [Fact]
    public void TelemetryDocRecordsSecH05Rules()
    {
        string doc = ReadRepoFileOrSkip(Path.Combine("docs", "TELEMETRY-READER.md"));
        Assert.Contains("SEC-H-05", doc, StringComparison.Ordinal);
        Assert.Contains("double-read", doc, StringComparison.OrdinalIgnoreCase);
        Assert.Contains("MaxSessionInfoBytes", doc, StringComparison.Ordinal);
    }
}
