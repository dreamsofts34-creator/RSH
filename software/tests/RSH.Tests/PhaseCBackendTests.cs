using System.Text.Json;
using RSH.Core.Output;
using RSH.Core.Pipeline;
using RSH.Core.Processing;
using RSH.Core.Profiles;
using RSH.Core.Protocol;
using RSH.Core.Transport;

namespace RSH.Tests;

/// <summary>
/// Phase C — Backend (SW-5/SW-12): P6, A3, T15, A10, F3, P11 + SEC-C-11/12/13/14.
/// P6 ka case-mismatch contract ProfileStoreCaseTests.cs mein bhi hai
/// (parallel verification worker) — yahan sirf ek smoke test hai.
/// </summary>
public class PhaseCBackendTests
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

    private static ProfileFile DataWithDeadzone(double deadzone) => new()
    {
        Profiles = [WheelProfile.CreateDefault() with { Deadzone = deadzone, Smoothing = 0 }],
        ActiveProfileName = "Default",
    };

    private static double DeadzoneOf(string path) =>
        new ProfileStore(path).Load().Profiles[0].Deadzone;

    // ================= P6: GetActive case-insensitive (smoke) =================

    [Fact]
    public void GetActive_CaseMismatch_ReturnsCorrectProfile()
    {
        var data = new ProfileFile
        {
            Profiles =
            [
                WheelProfile.CreateDefault() with { Deadzone = 0.07 },
                WheelProfile.CreateDefault("Race") with { Smoothing = 0.9 },
            ],
            ActiveProfileName = "rAcE",
        };
        Assert.Equal("Race", ProfileStore.GetActive(data).Name);
        Assert.Equal(0.9, ProfileStore.GetActive(data).Smoothing, 6);
    }

    // ================= A3: 3-generation backup rotation =================

    [Fact]
    public void Save_Rotation_BakBak1Bak2_HoldThreeGenerations()
    {
        string file = TempFile();
        try
        {
            var store = new ProfileStore(file);
            store.Save(DataWithDeadzone(0.01));   // v1 — koi backup nahi
            Assert.False(File.Exists(file + ".bak"));
            store.Save(DataWithDeadzone(0.02));   // .bak = 0.01
            Assert.Equal(0.01, DeadzoneOf(file + ".bak"), 6);
            store.Save(DataWithDeadzone(0.03));   // .bak = 0.02, .bak1 = 0.01
            Assert.Equal(0.02, DeadzoneOf(file + ".bak"), 6);
            Assert.Equal(0.01, DeadzoneOf(file + ".bak1"), 6);
            store.Save(DataWithDeadzone(0.04));   // .bak = 0.03, .bak1 = 0.02, .bak2 = 0.01
            Assert.Equal(0.03, DeadzoneOf(file + ".bak"), 6);
            Assert.Equal(0.02, DeadzoneOf(file + ".bak1"), 6);
            Assert.Equal(0.01, DeadzoneOf(file + ".bak2"), 6);
            Assert.Equal(0.04, store.Load().Profiles[0].Deadzone, 6);
            store.Save(DataWithDeadzone(0.05));   // sab se purani (0.01) gir jati hai
            Assert.Equal(0.02, DeadzoneOf(file + ".bak2"), 6);
            // SEC-C-14: koi .tmp-* kachra nahi rehta.
            string? dir = Path.GetDirectoryName(file);
            Assert.Empty(Directory.GetFiles(dir!).Where(f => f.Contains(".tmp-")));
            Assert.False(File.Exists(file + ".tmp"));
        }
        finally { Cleanup(file); }
    }

    [Fact]
    public void Save_CorruptCurrentFile_DoesNotOverwriteGoodBak()
    {
        string file = TempFile();
        try
        {
            var store = new ProfileStore(file);
            store.Save(DataWithDeadzone(0.11));
            store.Save(DataWithDeadzone(0.11));   // .bak = 0.11 (acha)
            File.WriteAllText(file, "{ ye JSON nahi hai !!!");
            store.Save(DataWithDeadzone(0.22));
            Assert.Equal(0.11, DeadzoneOf(file + ".bak"), 6);   // acha .bak mehfooz
            Assert.Equal(0.22, store.Load().Profiles[0].Deadzone, 6);
        }
        finally { Cleanup(file); }
    }

    [Fact]
    public void Save_CorruptBak_DoesNotOverwriteGoodBak1()
    {
        string file = TempFile();
        try
        {
            var store = new ProfileStore(file);
            store.Save(DataWithDeadzone(0.01));
            store.Save(DataWithDeadzone(0.02));
            store.Save(DataWithDeadzone(0.03));   // .bak = 0.02, .bak1 = 0.01
            File.WriteAllText(file + ".bak", "garbage");   // .bak kharab kar do
            store.Save(DataWithDeadzone(0.04));
            // Kharab .bak aage nahi barha — .bak1 abhi bhi 0.01 hai,
            // aur .bak ab maujooda achi file (0.03) hai.
            Assert.Equal(0.01, DeadzoneOf(file + ".bak1"), 6);
            Assert.Equal(0.03, DeadzoneOf(file + ".bak"), 6);
            Assert.Equal(0.04, store.Load().Profiles[0].Deadzone, 6);
        }
        finally { Cleanup(file); }
    }

    // SEC-C-12 (doc ka lazmi test): na-jaiz values wali current file
    // (parse hoti hai, Sanitized fail karta hai) achay .bak ko overwrite nahi karti.
    [Fact]
    public void Save_InvalidValuesFile_DoesNotOverwriteGoodBak()
    {
        string file = TempFile();
        try
        {
            var store = new ProfileStore(file);
            store.Save(DataWithDeadzone(0.11));
            store.Save(DataWithDeadzone(0.11));   // .bak = 0.11 (acha)
            // Button 33 wali file: parse hoti hai, Sanitized throw karta hai.
            File.WriteAllText(file, """
                {"Profiles":[{"Name":"Default","GearUpVJoyButton":33}],"ActiveProfileName":"Default"}
                """);
            store.Save(DataWithDeadzone(0.22));
            Assert.Equal(0.11, DeadzoneOf(file + ".bak"), 6);   // acha .bak mehfooz
            Assert.Equal(0.22, store.Load().Profiles[0].Deadzone, 6);
        }
        finally { Cleanup(file); }
    }

    [Fact]
    public void Load_CorruptMainAndBak_FallsBackToBak1()
    {
        string file = TempFile();
        try
        {
            var store = new ProfileStore(file);
            store.Save(DataWithDeadzone(0.01));
            store.Save(DataWithDeadzone(0.02));
            store.Save(DataWithDeadzone(0.03));   // .bak1 = 0.01
            File.WriteAllText(file, "garbage");
            File.WriteAllText(file + ".bak", "garbage");
            var loaded = store.Load();
            Assert.Equal(0.01, loaded.Profiles[0].Deadzone, 6);   // .bak1 se wapas
        }
        finally { Cleanup(file); }
    }

    // ================= T15: khali list =================

    [Fact]
    public void GetActive_EmptyList_DoesNotCrash_ReturnsDefault()
    {
        var data = new ProfileFile { Profiles = [], ActiveProfileName = "Race" };
        var active = ProfileStore.GetActive(data);
        Assert.Equal("Race", active.Name);   // sane default — naam bhi mehfooz
    }

    [Fact]
    public void GetActive_EmptyList_NoName_ReturnsDefaultProfile()
    {
        var active = ProfileStore.GetActive(new ProfileFile { Profiles = [] });
        Assert.Equal("Default", active.Name);
    }

    [Fact]
    public void Load_EmptyProfilesFile_FallsBackToDefaults_NoCrash()
    {
        string file = TempFile();
        try
        {
            Directory.CreateDirectory(Path.GetDirectoryName(file)!);
            File.WriteAllText(file, """{"Profiles":[],"ActiveProfileName":"Default"}""");
            var loaded = new ProfileStore(file).Load();
            Assert.Single(loaded.Profiles);
            Assert.Equal("Default", loaded.ActiveProfileName);
        }
        finally { Cleanup(file); }
    }

    // ================= A10: Profile v2 + migration + rollback =================

    private const string V1Json = """
        {"Profiles":[{"Name":"Default","Deadzone":0.07},{"Name":"Race","Smoothing":0.9}],"ActiveProfileName":"Race"}
        """;

    [Fact]
    public void V1Profile_MigratesToV2_AndV1BackupPreserved()
    {
        string file = TempFile();
        try
        {
            Directory.CreateDirectory(Path.GetDirectoryName(file)!);
            File.WriteAllText(file, V1Json);
            var store = new ProfileStore(file);
            var loaded = store.Load();
            Assert.True(loaded.MigratedFromV1);
            Assert.Equal(2, loaded.SchemaVersion);
            Assert.Equal(2, loaded.Profiles.Count);
            Assert.Equal("Race", ProfileStore.GetActive(loaded).Name);

            store.Save(loaded);
            // Migration se pehle asal v1 bytes .v1.bak mein hain.
            Assert.True(store.HasV1Backup());
            Assert.Equal(V1Json, File.ReadAllText(file + ".v1.bak"));
            // Main file ab v2 hai.
            Assert.Contains("\"SchemaVersion\": 2", File.ReadAllText(file));
            var reloaded = store.Load();
            Assert.Equal(2, reloaded.Profiles.Count);
            Assert.False(reloaded.MigratedFromV1);
        }
        finally { Cleanup(file); }
    }

    [Fact]
    public void Downgrade_RestoreV1Backup_BringsBackOriginalFile()
    {
        string file = TempFile();
        try
        {
            Directory.CreateDirectory(Path.GetDirectoryName(file)!);
            File.WriteAllText(file, V1Json);
            var store = new ProfileStore(file);
            var loaded = store.Load();
            store.Save(loaded);
            Assert.True(store.HasV1Backup());

            Assert.True(store.RestoreV1Backup());
            // Main file dobara asal v1 hai — koi profile khoi nahi.
            Assert.Equal(V1Json, File.ReadAllText(file));
            var restored = store.Load();
            Assert.Equal(2, restored.Profiles.Count);
            Assert.Equal(0.07, restored.Profiles.Single(p => p.Name == "Default").Deadzone, 6);
            Assert.Equal(0.9, restored.Profiles.Single(p => p.Name == "Race").Smoothing, 6);
        }
        finally { Cleanup(file); }
    }

    [Fact]
    public void RestoreV1Backup_NoBackup_ReturnsFalse_FileUnchanged()
    {
        string file = TempFile();
        try
        {
            var store = new ProfileStore(file);
            store.Save(DataWithDeadzone(0.05));
            Assert.False(store.HasV1Backup());
            Assert.False(store.RestoreV1Backup());
            Assert.Equal(0.05, store.Load().Profiles[0].Deadzone, 6);
        }
        finally { Cleanup(file); }
    }

    [Fact]
    public void V1Backup_FirstMigrationWins_NeverOverwritten()
    {
        string file = TempFile();
        try
        {
            Directory.CreateDirectory(Path.GetDirectoryName(file)!);
            File.WriteAllText(file, V1Json);
            var store = new ProfileStore(file);
            store.Save(store.Load());
            Assert.Equal(V1Json, File.ReadAllText(file + ".v1.bak"));
            // Dobara v1 file likh kar migrate karo — pehla .v1.bak rehta hai.
            File.WriteAllText(file, """{"Profiles":[{"Name":"Other","Deadzone":0.03}],"ActiveProfileName":"Other"}""");
            store.Save(store.Load());
            Assert.Equal(V1Json, File.ReadAllText(file + ".v1.bak"));
        }
        finally { Cleanup(file); }
    }

    [Fact]
    public void UnknownSchemaVersion_Rejected_FallsBackToDefaults()
    {
        string file = TempFile();
        try
        {
            Directory.CreateDirectory(Path.GetDirectoryName(file)!);
            File.WriteAllText(file, """{"SchemaVersion":99,"Profiles":[{"Name":"Future","Deadzone":0.07}],"ActiveProfileName":"Future"}""");
            var loaded = new ProfileStore(file).Load();
            Assert.Single(loaded.Profiles);
            Assert.Equal("Default", loaded.Profiles[0].Name);   // mustaqbil ka version khamoshi se nahi lagta
        }
        finally { Cleanup(file); }
    }

    [Fact]
    public void BoardMac_RoundtripsNormalized_InvalidCleared()
    {
        var p = (WheelProfile.CreateDefault() with { BoardMac = "aa:bb:cc:dd:ee:ff" }).Sanitized();
        Assert.Equal("AA:BB:CC:DD:EE:FF", p.BoardMac);
        Assert.True(p.MatchesBoard("aa-bb-cc-dd-ee-ff"));
        Assert.False(p.MatchesBoard("11:22:33:44:55:66"));
        Assert.True(WheelProfile.CreateDefault().MatchesBoard("11:22:33:44:55:66"));   // khaali = kisi bhi board

        var bad = (WheelProfile.CreateDefault() with { BoardMac = "not-a-mac" }).Sanitized();
        Assert.Null(bad.BoardMac);

        string file = TempFile();
        try
        {
            var store = new ProfileStore(file);
            store.Save(new ProfileFile
            {
                Profiles = [WheelProfile.CreateDefault() with { BoardMac = "aa:bb:cc:dd:ee:ff" }],
                ActiveProfileName = "Default",
            });
            var loaded = store.Load();
            Assert.Equal("AA:BB:CC:DD:EE:FF", loaded.Profiles[0].BoardMac);
        }
        finally { Cleanup(file); }
    }

    // ================= A10: v1 migration — unit-less calibration faisla =================
    // Faisla: RECALIBRATE PROMPT, tag nahi. v1 calibration mein unit tag
    // nahi hota; values se unit andaza nahi lagaya ja sakta (raw aur mV
    // overlap karte hain), is liye blindly mV tag karna P9 ka bug dobara
    // kholta. Values verbatim rehti hain, mapper ignore karta hai,
    // NeedsRecalibration UI se dobara karwata hai.

    private static ProfileFile LoadV1Json(string file, string json)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(file)!);
        File.WriteAllText(file, json);
        return new ProfileStore(file).Load();
    }

    [Fact]
    public void V1Migration_UntaggedCalibration_NotTagged_NeedsRecalibration()
    {
        string file = TempFile();
        try
        {
            var loaded = LoadV1Json(file, """
                {"Profiles":[{"Name":"Default","Deadzone":0,
                  "SteeringCalibration":{"RawMin":1000,"RawCenter":2050,"RawMax":3100,"IsSet":true}}],
                 "ActiveProfileName":"Default"}
                """);
            Assert.True(loaded.MigratedFromV1);
            var p = loaded.Profiles[0];
            // Tag NAHI lagaya gaya — values verbatim, unit abhi bhi maloom nahi.
            Assert.False(p.SteeringCalibration.IsMillivolts);
            Assert.True(p.SteeringCalibration.IsSet);   // values delete nahi hui
            Assert.Equal(1000, p.SteeringCalibration.RawMin);
            Assert.Equal(2050, p.SteeringCalibration.RawCenter);
            Assert.Equal(3100, p.SteeringCalibration.RawMax);
            Assert.False(p.SteeringCalibration.IsSane);
            Assert.True(p.NeedsRecalibration);
            // Aur mapper is calibration ko ignore karta hai (safe defaults):
            // steer 3100 stored calibration se full lock (32768) hota,
            // default mV range (900/2050/3200) se 0.913 → 31343.
            var pipeline = new ProcessingPipeline(p);
            var state = pipeline.Process(Packet(steer: 3100), 0.005);
            Assert.Equal(31343, state.SteeringAxis);
        }
        finally { Cleanup(file); }
    }

    [Fact]
    public void V1Migration_UntaggedPedalCalibration_NeedsRecalibration()
    {
        string file = TempFile();
        try
        {
            // Span 2300 ≥ 660 — sirf unit tag ki kami hai, phir bhi prompt.
            var loaded = LoadV1Json(file, """
                {"Profiles":[{"Name":"Default",
                  "AccelCalibration":{"RawMin":300,"RawMax":2600,"IsSet":true}}],
                 "ActiveProfileName":"Default"}
                """);
            var p = loaded.Profiles[0];
            Assert.False(p.AccelCalibration.IsMillivolts);
            Assert.False(p.AccelCalibration.IsSane);
            Assert.True(p.NeedsRecalibration);
        }
        finally { Cleanup(file); }
    }

    [Fact]
    public void V1Migration_TaggedSaneCalibration_Kept_NoPrompt()
    {
        string file = TempFile();
        try
        {
            var loaded = LoadV1Json(file, """
                {"Profiles":[{"Name":"Default",
                  "SteeringCalibration":{"RawMin":1000,"RawCenter":2050,"RawMax":3100,"IsSet":true,"IsMillivolts":true}}],
                 "ActiveProfileName":"Default"}
                """);
            var p = loaded.Profiles[0];
            Assert.True(p.SteeringCalibration.IsSane);
            Assert.False(p.NeedsRecalibration);
            var pipeline = new ProcessingPipeline(p);
            Assert.Equal(32768, pipeline.Process(Packet(steer: 3100), 0.005).SteeringAxis);
        }
        finally { Cleanup(file); }
    }

    [Fact]
    public void V1Migration_NarrowSpanCalibration_NeedsRecalibration()
    {
        string file = TempFile();
        try
        {
            // Tag hai lekin span 100 < 660 (D10) — phir bhi dobara calibration.
            var loaded = LoadV1Json(file, """
                {"Profiles":[{"Name":"Default",
                  "SteeringCalibration":{"RawMin":2000,"RawCenter":2050,"RawMax":2100,"IsSet":true,"IsMillivolts":true}}],
                 "ActiveProfileName":"Default"}
                """);
            Assert.False(loaded.Profiles[0].SteeringCalibration.IsSane);
            Assert.True(loaded.Profiles[0].NeedsRecalibration);
        }
        finally { Cleanup(file); }
    }

    [Fact]
    public void V1Migration_NoCalibration_NoPrompt()
    {
        string file = TempFile();
        try
        {
            var loaded = LoadV1Json(file, """
                {"Profiles":[{"Name":"Default","Deadzone":0.07}],"ActiveProfileName":"Default"}
                """);
            Assert.False(loaded.Profiles[0].NeedsRecalibration);
        }
        finally { Cleanup(file); }
    }

    [Fact]
    public void V1Migration_NeedsRecalibration_PersistsAndClearsAfterRecalibration()
    {
        string file = TempFile();
        try
        {
            var store = new ProfileStore(file);
            var loaded = LoadV1Json(file, """
                {"Profiles":[{"Name":"Default",
                  "SteeringCalibration":{"RawMin":1000,"RawCenter":2050,"RawMax":3100,"IsSet":true}}],
                 "ActiveProfileName":"Default"}
                """);
            Assert.True(loaded.Profiles[0].NeedsRecalibration);
            store.Save(loaded);
            // Save → Load: flag file mein mehfooz rehta hai (sticky).
            var reloaded = store.Load();
            Assert.True(reloaded.Profiles[0].NeedsRecalibration);
            Assert.Equal(1000, reloaded.Profiles[0].SteeringCalibration.RawMin);   // values khoi nahi
            // Wizard dobara calibration karta hai (tagged + sane) → flag khud saaf.
            var recalibrated = reloaded.Profiles[0] with
            {
                SteeringCalibration = new SteeringCalibration
                { RawMin = 1000, RawCenter = 2050, RawMax = 3100, IsSet = true, IsMillivolts = true },
            };
            Assert.False(recalibrated.Sanitized().NeedsRecalibration);
        }
        finally { Cleanup(file); }
    }

    // ================= F3: tolerant profile file =================

    [Fact]
    public void Load_OneCorruptEntry_SkipsIt_RestLoad()
    {
        string file = TempFile();
        try
        {
            Directory.CreateDirectory(Path.GetDirectoryName(file)!);
            File.WriteAllText(file, """
                {"Profiles":[
                  {"Name":"Good1","Deadzone":0.07},
                  {"Name":"Bad","GearUpVJoyButton":33},
                  {"Name":"Good2","Smoothing":0.8}
                ],"ActiveProfileName":"Good2"}
                """);
            var store = new ProfileStore(file);
            var loaded = store.Load();
            Assert.Equal(2, loaded.Profiles.Count);
            Assert.Equal(1, loaded.SkippedProfiles);
            Assert.Equal(1, store.LastSkippedProfiles);
            Assert.Contains(loaded.Profiles, p => p.Name == "Good1");
            Assert.Contains(loaded.Profiles, p => p.Name == "Good2");
            Assert.Equal("Good2", ProfileStore.GetActive(loaded).Name);
        }
        finally { Cleanup(file); }
    }

    [Fact]
    public void Load_WrongTypeEntry_SkipsIt_RestLoad()
    {
        string file = TempFile();
        try
        {
            Directory.CreateDirectory(Path.GetDirectoryName(file)!);
            File.WriteAllText(file, """
                {"Profiles":[
                  {"Name":"Good","Deadzone":0.07},
                  {"Name":"Bad","Deadzone":"not-a-number"}
                ],"ActiveProfileName":"Good"}
                """);
            var loaded = new ProfileStore(file).Load();
            Assert.Single(loaded.Profiles);
            Assert.Equal("Good", loaded.Profiles[0].Name);
            Assert.Equal(1, loaded.SkippedProfiles);
        }
        finally { Cleanup(file); }
    }

    [Fact]
    public void Load_AllEntriesCorrupt_FallsBackToDefaults()
    {
        string file = TempFile();
        try
        {
            Directory.CreateDirectory(Path.GetDirectoryName(file)!);
            File.WriteAllText(file, """
                {"Profiles":[{"Name":"Bad","GearUpVJoyButton":33}],"ActiveProfileName":"Bad"}
                """);
            var loaded = new ProfileStore(file).Load();
            Assert.Single(loaded.Profiles);
            Assert.Equal("Default", loaded.Profiles[0].Name);
        }
        finally { Cleanup(file); }
    }

    // ================= SEC-C-11: size/count caps =================

    [Fact]
    public void Load_OversizeFile_Rejected_FallsBackToBackup()
    {
        string file = TempFile();
        try
        {
            var store = new ProfileStore(file);
            store.Save(DataWithDeadzone(0.11));
            store.Save(DataWithDeadzone(0.11));   // .bak = 0.11
            // 1 MB se bara file — bada Naam field se.
            string huge = new string('x', 1_100_000);
            File.WriteAllText(file, $$"""{"Profiles":[{"Name":"{{huge}}","Deadzone":0.07}],"ActiveProfileName":"Default"}""");
            Assert.True(new FileInfo(file).Length > ProfileStore.MaxFileBytes);
            var loaded = store.Load();
            Assert.Equal(0.11, loaded.Profiles[0].Deadzone, 6);   // backup se wapas
        }
        finally { Cleanup(file); }
    }

    [Fact]
    public void Load_TooManyProfiles_Rejected()
    {
        string file = TempFile();
        try
        {
            Directory.CreateDirectory(Path.GetDirectoryName(file)!);
            var data = new ProfileFile
            {
                Profiles = Enumerable.Range(0, ProfileStore.MaxProfiles + 1)
                    .Select(i => WheelProfile.CreateDefault("P" + i)).ToList(),
                ActiveProfileName = "P0",
            };
            File.WriteAllText(file, JsonSerializer.Serialize(data));
            var loaded = new ProfileStore(file).Load();
            Assert.Single(loaded.Profiles);
            Assert.Equal("Default", loaded.Profiles[0].Name);
        }
        finally { Cleanup(file); }
    }

    [Fact]
    public void Save_TooManyProfiles_CappedAtMax()
    {
        string file = TempFile();
        try
        {
            var store = new ProfileStore(file);
            var data = new ProfileFile
            {
                Profiles = Enumerable.Range(0, ProfileStore.MaxProfiles + 5)
                    .Select(i => WheelProfile.CreateDefault("P" + i)).ToList(),
                ActiveProfileName = "P0",
            };
            store.Save(data);
            var loaded = store.Load();
            Assert.Equal(ProfileStore.MaxProfiles, loaded.Profiles.Count);
        }
        finally { Cleanup(file); }
    }

    // ================= P11: PipelinePlan wiring =================

    private static WheelProfile CalibratedProfile() => WheelProfile.CreateDefault() with
    {
        Smoothing = 0,
        Deadzone = 0,
        SteeringCalibration = new SteeringCalibration
        { RawMin = 1000, RawCenter = 2050, RawMax = 3100, IsSet = true, IsMillivolts = true },
    };

    private static WheelPacket Packet(ushort steer = 2050, ushort accel = 300, ushort brake = 300, byte buttons = 0) =>
        new(1, 0, steer, accel, brake, buttons, true);

    [Fact]
    public void SetPlan_SteeringDeadzone_AppliesPerModule()
    {
        var pipeline = new ProcessingPipeline(CalibratedProfile());
        // steer 2327 → normalized ~0.264 — default plan mein guzarta hai.
        var before = pipeline.Process(Packet(steer: 2327), 0.005);
        Assert.True(before.SteeringNorm > 0.2);
        pipeline.SetPlan(new PipelinePlanBuilder()
            .WithTuning(ModuleKind.Steering, new ModuleTuning { Deadzone = 0.5 })
            .Build());
        var after = pipeline.Process(Packet(steer: 2327), 0.005);
        Assert.Equal(0, after.SteeringNorm, 6);
        Assert.Equal(16384, after.SteeringAxis);
    }

    [Fact]
    public void SetPlan_DisabledModules_GoNeutral()
    {
        var pipeline = new ProcessingPipeline(CalibratedProfile());
        pipeline.SetPlan(new PipelinePlanBuilder()
            .WithEnabled(ModuleKind.Steering, false)
            .WithEnabled(ModuleKind.Accelerator, false)
            .WithEnabled(ModuleKind.Brake, false)
            .WithEnabled(ModuleKind.Buttons, false)
            .Build());
        // Full lock + full pedals + gear button — sab neutral hona chahiye.
        var state = pipeline.Process(Packet(steer: 3100, accel: 4095, brake: 4095, buttons: WheelPacket.ButtonGearUp), 0.005);
        Assert.Equal(16384, state.SteeringAxis);
        Assert.Equal(0, state.SteeringNorm, 6);
        Assert.Equal(1, state.AccelAxis);
        Assert.Equal(0, state.AccelNorm, 6);
        Assert.Equal(1, state.BrakeAxis);
        Assert.Equal(0, state.BrakeNorm, 6);
        Assert.Equal(0u, state.ButtonsMask);
    }

    [Fact]
    public void SetPlan_OnlyBrakeDisabled_OthersUnaffected()
    {
        var pipeline = new ProcessingPipeline(CalibratedProfile());
        pipeline.SetPlan(new PipelinePlanBuilder()
            .WithEnabled(ModuleKind.Brake, false)
            .Build());
        var state = pipeline.Process(Packet(steer: 3100, accel: 4095, brake: 4095), 0.005);
        Assert.Equal(32768, state.SteeringAxis);
        Assert.Equal(32768, state.AccelAxis);
        Assert.Equal(1, state.BrakeAxis);
    }

    [Fact]
    public void SetPlan_PedalDeadzone_AppliesPerModule()
    {
        var pipeline = new ProcessingPipeline(CalibratedProfile());
        pipeline.SetPlan(new PipelinePlanBuilder()
            .WithTuning(ModuleKind.Accelerator, new ModuleTuning { Deadzone = 0.5 })
            .Build());
        // accel raw 2000 → ~0.488 normalized → deadzone 0.5 ke andar → 0.
        var state = pipeline.Process(Packet(accel: 2000), 0.005);
        Assert.Equal(0, state.AccelNorm, 6);
        Assert.Equal(1, state.AccelAxis);
        // Full pedal deadzone ke bahir — guzarta hai.
        var full = pipeline.Process(Packet(accel: 4095), 0.005);
        Assert.Equal(1, full.AccelNorm, 6);
    }

    [Fact]
    public void SetPlan_Rewire_InvertTogglesWithoutRebuild()
    {
        var pipeline = new ProcessingPipeline(CalibratedProfile());
        pipeline.SetPlan(new PipelinePlanBuilder()
            .WithTuning(ModuleKind.Steering, new ModuleTuning { Invert = true })
            .Build());
        Assert.Equal(1, pipeline.Process(Packet(steer: 3100), 0.005).SteeringAxis);   // ulta
        pipeline.SetPlan(new PipelinePlanBuilder()
            .WithTuning(ModuleKind.Steering, new ModuleTuning { Invert = false })
            .Build());
        Assert.Equal(32768, pipeline.Process(Packet(steer: 3100), 0.005).SteeringAxis);   // seedha
    }

    [Fact]
    public void SetPlan_InvalidPlan_Throws_AndOldPlanStays()
    {
        var pipeline = new ProcessingPipeline(CalibratedProfile());
        var entries = PipelinePlan.AllModules.ToDictionary(
            m => m, m => new ModulePlanEntry { Module = m });
        entries[ModuleKind.Steering] = new ModulePlanEntry
        {
            Module = ModuleKind.Steering,
            Enabled = true,
            Tuning = new ModuleTuning { Deadzone = 9.9 },
        };
        var bad = new PipelinePlan(entries);   // ctor sirf missing entries check karta hai
        Assert.False(bad.IsValid);
        Assert.Throws<PipelinePlanValidationException>(() => pipeline.SetPlan(bad));
        Assert.True(pipeline.Plan.IsValid);
        Assert.Equal(0.0, pipeline.Plan[ModuleKind.Steering].Tuning.Deadzone);
    }

    [Fact]
    public void SetProfile_RebuildsPlanFromProfile()
    {
        var pipeline = new ProcessingPipeline(CalibratedProfile());
        pipeline.SetPlan(new PipelinePlanBuilder()
            .WithTuning(ModuleKind.Steering, new ModuleTuning { Deadzone = 0.4 })
            .Build());
        Assert.Equal(0.4, pipeline.Plan[ModuleKind.Steering].Tuning.Deadzone);
        pipeline.SetProfile(CalibratedProfile() with { Deadzone = 0.05 });
        Assert.Equal(0.05, pipeline.Plan[ModuleKind.Steering].Tuning.Deadzone);
    }

    [Fact]
    public void FromProfile_ModuleTunings_OverrideLegacy()
    {
        var profile = CalibratedProfile() with
        {
            Deadzone = 0.02,
            ModuleTunings = new Dictionary<string, PerModuleTuning>
            {
                ["Steering"] = new PerModuleTuning { Deadzone = 0.2, Enabled = false },
                ["Brake"] = new PerModuleTuning { Deadzone = 0.1 },
                ["NoSuchModule"] = new PerModuleTuning { Deadzone = 0.1 },
            },
        };
        var plan = PipelinePlanBuilder.FromProfile(profile);
        Assert.False(plan.IsEnabled(ModuleKind.Steering));
        Assert.Equal(0.1, plan[ModuleKind.Brake].Tuning.Deadzone);
        Assert.True(plan.IsEnabled(ModuleKind.Accelerator));   // legacy se — override nahi hua
        Assert.Equal(0.2, plan[ModuleKind.Steering].Tuning.Deadzone, 6);   // override ki tuning mehfooz hai
    }

    // ================= P11: WheelEngine plan load/save =================

    [Fact]
    public void Engine_SetPlan_AndSaveLoad_RoundtripsThroughProfileV2()
    {
        string file = TempFile();
        try
        {
            var store = new ProfileStore(file);
            var data = new ProfileFile
            {
                Profiles = [CalibratedProfile()],
                ActiveProfileName = "Default",
            };
            store.Save(data);

            var source = new FakeSource { Packet = Packet(steer: 3100, accel: 4095, brake: 4095), AgeMs = 5 };
            var output = new FakeOutput();
            using var engine = new WheelEngine(source, output, ProfileStore.GetActive(data));
            var plan = new PipelinePlanBuilder()
                .WithTuning(ModuleKind.Steering, new ModuleTuning { Deadzone = 0.1 })
                .WithEnabled(ModuleKind.Brake, false)
                .WithName("Default")
                .Build();
            engine.SetPlan(plan);
            Assert.Equal(0.1, engine.Plan[ModuleKind.Steering].Tuning.Deadzone);
            Assert.False(engine.Plan.IsEnabled(ModuleKind.Brake));

            engine.SavePlanToProfile(store, data);

            // Dobara load — plan waisa hi milna chahiye, koi profile khoi nahi.
            var reloaded = store.Load();
            Assert.Single(reloaded.Profiles);
            using var engine2 = new WheelEngine(source, output, ProfileStore.GetActive(reloaded));
            engine2.LoadPlanFromActiveProfile(reloaded);
            Assert.Equal(0.1, engine2.Plan[ModuleKind.Steering].Tuning.Deadzone);
            Assert.False(engine2.Plan.IsEnabled(ModuleKind.Brake));

            // Aur asar bhi: brake band hai to engine tick par brake neutral.
            engine2.TickOnce(0.005, 1000);
            engine2.TickOnce(0.005, 1005);
            Assert.Equal(1, output.LastState.BrakeAxis);
            Assert.Equal(32768, output.LastState.SteeringAxis);
        }
        finally { Cleanup(file); }
    }

    [Fact]
    public void Engine_SetPlan_Invalid_Throws_PlanUnchanged()
    {
        var source = new FakeSource { Packet = Packet(), AgeMs = 5 };
        var output = new FakeOutput();
        using var engine = new WheelEngine(source, output, CalibratedProfile());
        var entries = PipelinePlan.AllModules.ToDictionary(
            m => m, m => new ModulePlanEntry { Module = m });
        entries[ModuleKind.Brake] = new ModulePlanEntry
        {
            Module = ModuleKind.Brake,
            Enabled = true,
            Tuning = new ModuleTuning { Smoothing = 5 },
        };
        var bad = new PipelinePlan(entries);
        Assert.Throws<PipelinePlanValidationException>(() => engine.SetPlan(bad));
        Assert.True(engine.Plan.IsValid);
    }

    // ================= T11/Ex: BtnsEx → pipeline → ProcessedState =================

    [Fact]
    public void Process_ButtonsEx_FlowsToButtonsMaskEx()
    {
        var pipeline = new ProcessingPipeline(CalibratedProfile());
        // BtnsEx bit 5 = 0-based button 37 = vJoy button 38 → maskEx bit 5.
        var state = pipeline.Process(Packet(), 0.005, buttonsEx: 0x0000_0020u);
        Assert.Equal(0x20u, state.ButtonsMaskEx);
        Assert.Equal(0u, state.ButtonsMask);   // gear bits chhoo nahi gaye
        // Baghair Ex ke — 0 rehta hai (purana behaviour).
        var plain = pipeline.Process(Packet(), 0.005);
        Assert.Equal(0u, plain.ButtonsMaskEx);
    }

    [Fact]
    public void Process_ButtonsEx_Zero_WhenButtonsModuleDisabled()
    {
        var pipeline = new ProcessingPipeline(CalibratedProfile());
        pipeline.SetPlan(new PipelinePlanBuilder()
            .WithEnabled(ModuleKind.Buttons, false)
            .Build());
        var state = pipeline.Process(Packet(buttons: WheelPacket.ButtonGearUp), 0.005, buttonsEx: 0xFFFF_FFFFu);
        Assert.Equal(0u, state.ButtonsMaskEx);
        Assert.Equal(0u, state.ButtonsMask);
    }

    [Fact]
    public void Engine_LatestButtonsEx_FlowsToOutput()
    {
        var source = new FakeSource { Packet = Packet(), AgeMs = 5 };
        var output = new FakeOutput();
        using var engine = new WheelEngine(source, output, CalibratedProfile());
        engine.LatestButtonsEx = 0x0000_0020u;   // 0-based button 37
        engine.TickOnce(0.005, 1000);
        engine.TickOnce(0.005, 1005);
        Assert.Equal(0x20u, output.LastState.ButtonsMaskEx);
        engine.LatestButtonsEx = 0;
        engine.TickOnce(0.005, 1010);
        Assert.Equal(0u, output.LastState.ButtonsMaskEx);
    }
}
