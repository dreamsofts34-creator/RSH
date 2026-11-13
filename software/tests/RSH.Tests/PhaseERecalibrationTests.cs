using RSH.Core.Processing;
using RSH.Core.Profiles;
using RSH.Core.Protocol;

namespace RSH.Tests;

/// <summary>
/// Phase E WS7 — Setup wizard gates vs IsSane (D10) + NeedsRecalibration
/// triggers (pin map / firmware / stale) + clear path.
/// </summary>
public class PhaseERecalibrationTests
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

    private static readonly DateTimeOffset Now = new(2026, 11, 13, 9, 0, 0, TimeSpan.Zero);

    private const string FpA = "32:1:1;33:1:2;34:1:3";
    private const string FpB = "32:1:1;33:1:2;35:1:3";   // ek pin badla (34 → 35)

    private static SteeringCalibration SaneSteering => new()
        { RawMin = 1000, RawCenter = 2050, RawMax = 3100, IsSet = true, IsMillivolts = true };

    private static PedalCalibration SanePedal => new()
        { RawMin = 300, RawMax = 2600, IsSet = true, IsMillivolts = true };

    /// <summary>Sane calibrations + stamps (fp A, fw 4.0.0, abhi calibrate hui).</summary>
    private static WheelProfile StampedProfile() => WheelProfile.CreateDefault() with
    {
        SteeringCalibration = SaneSteering,
        AccelCalibration = SanePedal,
        BrakeCalibration = SanePedal,
        CalibrationPinMapFingerprint = FpA,
        CalibrationFirmwareVersion = "4.0.0",
        CalibratedAtUtc = Now,
    };

    private static CalibrationContext CurrentContext(
        string? fp = FpA, string? fw = "4.0.0", DateTimeOffset? now = null) =>
        new(fp, fw, now ?? Now);

    // ================= Item 1: wizard gates vs IsSane (D10) =================

    [Fact]
    public void GateValues_MatchD10()
    {
        Assert.Equal(330, SetupGates.MinSteeringSideMillivolts);
        Assert.Equal(660, SetupGates.MinSteeringSpanMillivolts);
        Assert.Equal(660, SetupGates.MinPedalSpanMillivolts);
        Assert.Equal(330, SteeringCalibration.MinSideMillivolts);
        // AutoCalibrator pehle se D10 par tha — teeno jagah ek hi value.
        Assert.Equal(660, AutoCalibrator.MinSteeringSpan);
        Assert.Equal(660, AutoCalibrator.MinPedalSpan);
    }

    [Fact]
    public void SteeringRangeComplete_Boundaries()
    {
        Assert.True(SetupGates.SteeringRangeComplete(330, 330));    // dono taraf minimum
        Assert.False(SetupGates.SteeringRangeComplete(329, 330));   // ek taraf 1 mV kam
        Assert.False(SetupGates.SteeringRangeComplete(330, 329));
        Assert.False(SetupGates.SteeringRangeComplete(0, 0));
        Assert.False(SetupGates.SteeringRangeComplete(660, 0));     // kul span kaafi, taraf khaali
        Assert.True(SetupGates.SteeringRangeComplete(500, 500));
    }

    [Fact]
    public void PedalComplete_Boundaries()
    {
        Assert.True(SetupGates.PedalComplete(660));
        Assert.False(SetupGates.PedalComplete(659));
        Assert.False(SetupGates.PedalComplete(0));
    }

    [Fact]
    public void MinimumPassingWizardCapture_IsSane_OldGatesWereNot()
    {
        // Naye gates se guzri kam se kam capture: centre 2050, har taraf
        // 330 mV → min 1720, max 2380, span 660 → IsSane true.
        var steering = new SteeringCalibration
            { RawMin = 1720, RawCenter = 2050, RawMax = 2380, IsSet = true, IsMillivolts = true };
        Assert.True(steering.IsSane);
        // Pedal: span bilkul 660 → IsSane true.
        Assert.True(new PedalCalibration
            { RawMin = 1000, RawMax = 1660, IsSet = true, IsMillivolts = true }.IsSane);

        // PURANE gates ka bug (regression guard): 250/side → span 500 mV
        // → IsSane FALSE (wizard pass karta tha, Finish reject karta tha).
        var oldSteering = new SteeringCalibration
            { RawMin = 1800, RawCenter = 2050, RawMax = 2300, IsSet = true, IsMillivolts = true };
        Assert.False(oldSteering.IsSane);
        // Purana pedal gate 150 mV span → IsSane FALSE.
        Assert.False(new PedalCalibration
            { RawMin = 1000, RawMax = 1150, IsSet = true, IsMillivolts = true }.IsSane);
        // Raw units mein bhi wahi: 250 raw/side = 500 raw = ~403 mV.
        Assert.True(InputMapper.RawToMillivolts(500) < SteeringCalibration.MinSpanMillivolts);
        Assert.True(InputMapper.RawToMillivolts(150) < PedalCalibration.MinSpanMillivolts);
    }

    [Fact]
    public void HintStrings_Exact()
    {
        Assert.Equal(
            "Turn the wheel further left — 120 mV more to reach the minimum range.",
            SetupGates.SteeringLeftHint(120));
        Assert.Equal(
            "Turn the wheel further right — 45 mV more to reach the minimum range.",
            SetupGates.SteeringRightHint(45));
        Assert.Equal(
            "Press the accelerator fully down — 540 mV more to reach the minimum range.",
            SetupGates.PedalHint("accelerator", 540));
        Assert.Equal(
            "Press the brake fully down — 540 mV more to reach the minimum range.",
            SetupGates.PedalHint("brake", 540));
        Assert.Equal(
            "The centre looks off — hold the wheel straight ahead and restart this step.",
            SetupGates.CentreOffHint);
        Assert.Equal(
            "Steering range is too small — the wheel needs at least 660 mV from end to end. Turn it lock to lock and try again.",
            SetupGates.CalibrationTooNarrowHint("Steering"));
        Assert.Equal(
            "Brake range is too small — the wheel needs at least 660 mV from end to end. Turn it lock to lock and try again.",
            SetupGates.CalibrationTooNarrowHint("Brake"));
    }

    // ================= Item 2: NeedsRecalibration triggers =================

    [Fact]
    public void Check_AllMatch_NoRecalibration()
    {
        var check = StampedProfile().CheckRecalibration(CurrentContext());
        Assert.False(check.NeedsRecalibration);
        Assert.Equal(RecalibrationReason.None, check.Reasons);
        Assert.Null(check.PromptMessage);
    }

    [Fact]
    public void Check_PinMapChanged_ReasonAndPrompt()
    {
        var check = StampedProfile().CheckRecalibration(CurrentContext(fp: FpB));
        Assert.True(check.NeedsRecalibration);
        Assert.Equal(RecalibrationReason.PinMapChanged, check.Reasons);
        Assert.Contains("wiring (pin assignment) changed", check.PromptMessage);
    }

    [Fact]
    public void Check_PinMapUnknown_NoTrigger()
    {
        // Stored fingerprint nahi (legacy profile) → koi trigger nahi.
        var legacy = StampedProfile() with { CalibrationPinMapFingerprint = null };
        Assert.Equal(RecalibrationReason.None, legacy.CheckRecalibration(CurrentContext(fp: FpB)).Reasons);
        // Current fingerprint nahi (board se GET_CAPS nahi mila) → koi trigger nahi.
        Assert.Equal(RecalibrationReason.None, StampedProfile().CheckRecalibration(CurrentContext(fp: null)).Reasons);
    }

    [Fact]
    public void Check_FirmwareMinorAndMajorChanged_PatchOnlyNot()
    {
        var profile = StampedProfile();
        Assert.Equal(RecalibrationReason.FirmwareChanged,
            profile.CheckRecalibration(CurrentContext(fw: "4.1.0")).Reasons);
        Assert.Equal(RecalibrationReason.FirmwareChanged,
            profile.CheckRecalibration(CurrentContext(fw: "5.0.0")).Reasons);
        // Patch-only (4.0.0 → 4.0.1) trigger nahi karta — faisla doc mein.
        Assert.Equal(RecalibrationReason.None,
            profile.CheckRecalibration(CurrentContext(fw: "4.0.1")).Reasons);
        Assert.Equal(RecalibrationReason.None,
            profile.CheckRecalibration(CurrentContext(fw: "4.0.0")).Reasons);
    }

    [Fact]
    public void Check_FirmwareUnknown_NoTrigger()
    {
        var legacy = StampedProfile() with { CalibrationFirmwareVersion = null };
        Assert.Equal(RecalibrationReason.None, legacy.CheckRecalibration(CurrentContext(fw: "5.0.0")).Reasons);
        Assert.Equal(RecalibrationReason.None, StampedProfile().CheckRecalibration(CurrentContext(fw: null)).Reasons);
    }

    [Fact]
    public void Check_StaleCalibration_Boundary()
    {
        var profile = StampedProfile();
        // 181 din purani → stale.
        var stale = profile with { CalibratedAtUtc = Now.AddDays(-181) };
        Assert.Equal(RecalibrationReason.StaleCalibration,
            stale.CheckRecalibration(CurrentContext()).Reasons);
        // 179 din → nahi.
        var fresh = profile with { CalibratedAtUtc = Now.AddDays(-179) };
        Assert.Equal(RecalibrationReason.None, fresh.CheckRecalibration(CurrentContext()).Reasons);
        // Bilkul 180 din → nahi (age > 180 chahiye).
        var edge = profile with { CalibratedAtUtc = Now.AddDays(-180) };
        Assert.Equal(RecalibrationReason.None, edge.CheckRecalibration(CurrentContext()).Reasons);
        // Mustaqbil ki tareekh (clock skew) → stale nahi.
        var future = profile with { CalibratedAtUtc = Now.AddDays(5) };
        Assert.Equal(RecalibrationReason.None, future.CheckRecalibration(CurrentContext()).Reasons);
        // Tareekh hi nahi (legacy) → stale nahi.
        var unknown = profile with { CalibratedAtUtc = null };
        Assert.Equal(RecalibrationReason.None, unknown.CheckRecalibration(CurrentContext()).Reasons);
        Assert.Equal(180, CalibrationFreshness.StaleAfterDays);
    }

    [Fact]
    public void Check_NoCalibrationSet_NoContextTriggers()
    {
        // Pehli dafa setup — stamps hon aur context badla ho to bhi koi
        // trigger nahi (normal first-run flow, nuqsan nahi).
        var fresh = WheelProfile.CreateDefault() with
        {
            CalibrationPinMapFingerprint = FpA,
            CalibrationFirmwareVersion = "4.0.0",
            CalibratedAtUtc = Now.AddDays(-300),
        };
        var check = fresh.CheckRecalibration(CurrentContext(fp: FpB, fw: "9.9.9"));
        Assert.False(check.NeedsRecalibration);
        Assert.Equal(RecalibrationReason.None, check.Reasons);
    }

    [Fact]
    public void Check_InsaneCalibration_ReasonAndPersistedFlag()
    {
        var narrow = WheelProfile.CreateDefault() with
        {
            SteeringCalibration = new SteeringCalibration
                { RawMin = 2000, RawCenter = 2050, RawMax = 2100, IsSet = true, IsMillivolts = true },
        };
        var sanitized = narrow.Sanitized();
        Assert.True(sanitized.NeedsRecalibration);   // persisted flag (Phase C)
        var check = sanitized.CheckRecalibration(CurrentContext());
        Assert.True(check.NeedsRecalibration);
        Assert.Equal(RecalibrationReason.InsaneCalibration, check.Reasons);
        Assert.Contains("out of range", check.PromptMessage);
    }

    [Fact]
    public void Check_MultipleReasons_Combined()
    {
        var profile = StampedProfile() with { CalibratedAtUtc = Now.AddDays(-200) };
        var check = profile.CheckRecalibration(CurrentContext(fp: FpB, fw: "4.2.0"));
        Assert.True(check.NeedsRecalibration);
        Assert.Equal(
            RecalibrationReason.PinMapChanged | RecalibrationReason.FirmwareChanged | RecalibrationReason.StaleCalibration,
            check.Reasons);
        Assert.Contains("wiring (pin assignment) changed", check.PromptMessage);
        Assert.Contains("firmware changed", check.PromptMessage);
        Assert.Contains("over 6 months old", check.PromptMessage);
    }

    [Fact]
    public void MarkCalibrated_ClearsAllReasons()
    {
        // Pin map + firmware + staleness — teeno triggers lage hue.
        var profile = StampedProfile() with { CalibratedAtUtc = Now.AddDays(-200) };
        var before = profile.CheckRecalibration(CurrentContext(fp: FpB, fw: "4.2.0"));
        Assert.True(before.NeedsRecalibration);

        // Kamyaab recalibration: naye context ke stamps lagao.
        var recalibrated = profile.MarkCalibrated(FpB, "4.2.0", Now);
        var after = recalibrated.CheckRecalibration(CurrentContext(fp: FpB, fw: "4.2.0"));
        Assert.False(after.NeedsRecalibration);
        Assert.Equal(RecalibrationReason.None, after.Reasons);
        Assert.Null(after.PromptMessage);
        // Stamps save hue.
        Assert.Equal(FpB, recalibrated.CalibrationPinMapFingerprint);
        Assert.Equal("4.2.0", recalibrated.CalibrationFirmwareVersion);
        Assert.Equal(Now, recalibrated.CalibratedAtUtc);
    }

    [Fact]
    public void MarkCalibrated_ClearsPersistedFlag_WhenCalibrationsSane()
    {
        // Insane calibration se flag set…
        var narrow = WheelProfile.CreateDefault() with
        {
            SteeringCalibration = new SteeringCalibration
                { RawMin = 2000, RawCenter = 2050, RawMax = 2100, IsSet = true, IsMillivolts = true },
        };
        Assert.True(narrow.Sanitized().NeedsRecalibration);
        // …dobara calibration (sane) + MarkCalibrated → flag saaf.
        var recalibrated = (narrow with { SteeringCalibration = SaneSteering })
            .MarkCalibrated(FpA, "4.0.0", Now);
        Assert.False(recalibrated.NeedsRecalibration);
        Assert.False(recalibrated.CheckRecalibration(CurrentContext()).NeedsRecalibration);
    }

    [Fact]
    public void Sanitized_PreservesAndNormalizesStamps()
    {
        var p = (StampedProfile() with
        {
            CalibrationPinMapFingerprint = "  " + FpA + "  ",
            CalibrationFirmwareVersion = "4.0.0 ",
        }).Sanitized();
        Assert.Equal(FpA, p.CalibrationPinMapFingerprint);
        Assert.Equal("4.0.0", p.CalibrationFirmwareVersion);
        Assert.Equal(Now, p.CalibratedAtUtc);

        // Khaali fingerprint / ghalat version → null (unknown).
        var bad = (StampedProfile() with
        {
            CalibrationPinMapFingerprint = "   ",
            CalibrationFirmwareVersion = "not-a-version",
        }).Sanitized();
        Assert.Null(bad.CalibrationPinMapFingerprint);
        Assert.Null(bad.CalibrationFirmwareVersion);
    }

    [Fact]
    public void PinMapFingerprint_DeterministicAndSensitive()
    {
        static BoardCaps Caps(params CapsPinEntry[] pins) => new(
            new byte[] { 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF },
            4, 0, 0, 2, 1, 6, 32, 2, 6, 0, pins);

        var a = Caps(
            new CapsPinEntry(32, CapsPinKind.AdcNative, CapsPinModule.Steering, 0x05),
            new CapsPinEntry(33, CapsPinKind.AdcNative, CapsPinModule.Accelerator, 0x05));
        var sameDifferentOrder = Caps(
            new CapsPinEntry(33, CapsPinKind.AdcNative, CapsPinModule.Accelerator, 0x05),
            new CapsPinEntry(32, CapsPinKind.AdcNative, CapsPinModule.Steering, 0x05));
        var onePinChanged = Caps(
            new CapsPinEntry(32, CapsPinKind.AdcNative, CapsPinModule.Steering, 0x05),
            new CapsPinEntry(34, CapsPinKind.AdcNative, CapsPinModule.Accelerator, 0x05));

        Assert.Equal("32:1:1;33:1:2", CalibrationFreshness.PinMapFingerprint(a));
        Assert.Equal(CalibrationFreshness.PinMapFingerprint(a),
            CalibrationFreshness.PinMapFingerprint(sameDifferentOrder));
        Assert.NotEqual(CalibrationFreshness.PinMapFingerprint(a),
            CalibrationFreshness.PinMapFingerprint(onePinChanged));

        // FromBoard: fingerprint + version ek saath.
        var ctx = CalibrationContext.FromBoard(a, Now);
        Assert.Equal("32:1:1;33:1:2", ctx.PinMapFingerprint);
        Assert.Equal("4.0.0", ctx.FirmwareVersion);
    }

    [Fact]
    public void ProfileStore_RoundTrip_PreservesStampsAndFlag()
    {
        string file = TempFile();
        try
        {
            var insane = WheelProfile.CreateDefault() with
            {
                SteeringCalibration = new SteeringCalibration
                    { RawMin = 2000, RawCenter = 2050, RawMax = 2100, IsSet = true, IsMillivolts = true },
                CalibrationPinMapFingerprint = FpA,
                CalibrationFirmwareVersion = "4.0.0",
                CalibratedAtUtc = Now,
            };
            new ProfileStore(file).Save(new ProfileFile
            {
                Profiles = [insane],
                ActiveProfileName = "Default",
            });
            var loaded = new ProfileStore(file).Load().Profiles[0];
            Assert.True(loaded.NeedsRecalibration);
            Assert.Equal(FpA, loaded.CalibrationPinMapFingerprint);
            Assert.Equal("4.0.0", loaded.CalibrationFirmwareVersion);
            Assert.Equal(Now, loaded.CalibratedAtUtc);
            // Load ke baad bhi live check wahi wajah deta hai.
            var check = loaded.CheckRecalibration(CurrentContext(fp: FpB));
            Assert.Equal(RecalibrationReason.InsaneCalibration | RecalibrationReason.PinMapChanged, check.Reasons);
        }
        finally { Cleanup(file); }
    }
}
