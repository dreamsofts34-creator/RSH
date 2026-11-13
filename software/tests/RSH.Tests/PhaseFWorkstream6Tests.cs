using System.Buffers.Binary;
using System.Text;
using RSH.Core.Net;
using RSH.Core.Pipeline;
using RSH.Core.Processing;
using RSH.Core.Profiles;
using RSH.Core.Protocol;
using RSH.Core.Transport;

namespace RSH.Tests;

/// <summary>
/// Phase F — Workstream 6 (Testers/QA): Phase F items ke naye tests.
///
/// Do qism ke tests hain (WPF app Linux par run nahi hota, is liye
/// RSH.Tests sirf RSH.Core ko reference karta hai):
///  - Behavioral (RSH.Core): asal API ke khilaf — PASS = feature maujood,
///    FAIL = bug report (file:line test ke message mein hai).
///  - Static (file-content): RSH.App ki cheezein (AppSettings, GameLauncher,
///    PinMap, Theme.xaml) Linux par instantiate nahi ho sakti, is liye
///    source file parhi jati hai (PhaseCAppTests.FindRepoFile pattern).
///    Ye tests requirement ko pin karte hain; jab doosra workstream
///    feature implement kare, test ko usi ke mutabiq update karna hai.
///
/// FAIL hone wale tests baaqi gaps hain — har ek ke peeche ek bug
/// report hai (neeche har test par anchor). NOTE (2026-11-13, task ke
/// doran): parallel workstreams ne kaam land kiya — SEC-E-04 (WS1),
/// SEC-E-02 rate limit, SEC-E-10, SEC-E-12/13, SEC-C-15, SEC-C-18,
/// mDNS D33 (WS3), pin-map push (WS3) ab implemented hain; in ke tests
/// maujooda implementation ke khilaf likhe gaye hain (PASS expected).
/// Jo tests FAIL hain, wo abhi khule items hain.
/// </summary>
[Collection("SetPasswordRateLimit")]
public class PhaseFWorkstream6Tests
{
    // Deterministic fixtures (NOT real secrets) — PhaseDWorkstream7Tests jaisi.
    private static readonly byte[] TestKey = Enumerable.Range(0, 32).Select(i => (byte)i).ToArray();
    private static readonly byte[] TestNonce = Enumerable.Range(0, 16).Select(i => (byte)(0xA0 + i)).ToArray();

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

    private static int CountOccurrences(string text, string needle, StringComparison comparison = StringComparison.OrdinalIgnoreCase)
    {
        int count = 0, at = 0;
        while ((at = text.IndexOf(needle, at, comparison)) >= 0) { count++; at += needle.Length; }
        return count;
    }

    // ---------- Fake control transport (PhaseD pattern) ----------

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

    private static WheelProfile PlainProfile() => WheelProfile.CreateDefault() with
    {
        Smoothing = 0, Deadzone = 0, CurveStrength = 0,
        SteeringCalibration = new SteeringCalibration
        { RawMin = 900, RawCenter = 2050, RawMax = 3200, IsSet = true, IsMillivolts = true },
        AccelCalibration = new PedalCalibration
        { RawMin = 300, RawMax = 3300, IsSet = true, IsMillivolts = true },
        BrakeCalibration = new PedalCalibration
        { RawMin = 300, RawMax = 3300, IsSet = true, IsMillivolts = true },
    };

    private static WheelPacketV2 V2Packet(ushort seq, Dictionary<int, ushort> axes, uint buttons = 0, short[]? enc = null)
    {
        byte mask = 0;
        var ordered = new List<ushort>();
        foreach (int i in axes.Keys.OrderBy(k => k)) { mask |= (byte)(1 << i); ordered.Add(axes[i]); }
        byte[] bytes = WheelPacketV2.Build(seq, 1000, 0, mask, buttons, ordered.ToArray(),
            enc is null ? ReadOnlySpan<short>.Empty : enc.AsSpan());
        Assert.Equal(WheelPacketV2ParseResult.Ok, WheelPacketV2.TryParse(bytes, out var p));
        return p;
    }

    // ================= SEC-E-02: SET_PASSWORD rate limit =================
    // Anchor: ControlClient.cs — process-wide 60 s limit (SetPasswordMinInterval):
    // ek attempt ke 60 s ke andar doosri attempt reject hoti hai (ControlResult.Error,
    // Attempts 0, board tak kuch nahi jata). Instance-level limit kaam nahi
    // karti (BoardSession har call par naya client banata hai), is liye static hai.
    // ResetSetPasswordRateLimitForTest sirf tests ke liye hai.

    [Fact]
    public async Task SetPassword_RapidRepeatCalls_AreThrottled()
    {
        // Rate limit process-wide static hai (BoardSession har call par
        // naya ControlClient banata hai) — doosre tests ki state se
        // bachne ke liye shuru + aakhir mein reset lazmi hai.
        ControlClient.ResetSetPasswordRateLimitForTest();
        try
        {
            var transport = new FakeControlTransport(frame =>
                ReplyFor(frame, [0x00], TestKey, TestNonce));
            using var client = new ControlClient("127.0.0.1", TestKey, TestNonce, transport);
            var first = await client.SetPasswordAsync("NewPass123");
            Assert.True(first.Succeeded, "Pehli SET_PASSWORD kamyaab honi chahiye (fake board OK deta hai).");
            for (int i = 0; i < 4; i++)
            {
                var throttled = await client.SetPasswordAsync("NewPass123");
                Assert.False(throttled.Succeeded, "60 s ke andar dobara SET_PASSWORD kamyaab nahi honi chahiye.");
                Assert.NotNull(throttled.Error);
                Assert.Contains("rate limit", throttled.Error, StringComparison.OrdinalIgnoreCase);
                Assert.Equal(0, throttled.Attempts);
            }
            Assert.Single(transport.CapturedFrames);
        }
        finally
        {
            ControlClient.ResetSetPasswordRateLimitForTest();
        }
    }

    // ================= Pin-map board push: SET_CFG frame banta hai =================
    // Anchor: ProtocolV2.cs ControlCommand.SetCfg = 0x03. Frame-level
    // readiness PASS hai; board tak push ki wiring (PinMap → SET_CFG →
    // Commit) abhi nahi hai — neeche static test dekho.

    [Fact]
    public async Task SetCfg_FrameBuildsAndSends_WithCommandByte()
    {
        var transport = new FakeControlTransport(frame =>
            ReplyFor(frame, [0x00], TestKey, TestNonce));
        using var client = new ControlClient("127.0.0.1", TestKey, TestNonce, transport);
        byte[] payload = [0x22, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]; // 1 pin entry (placeholder)
        var result = await client.SendCommandAsync(ControlCommand.SetCfg, payload);
        Assert.True(result.Succeeded, "SET_CFG reply OK honi chahiye (fake board).");
        Assert.Single(transport.CapturedFrames);
        Assert.Equal(0x03, transport.CapturedFrames[0][4]);
    }

    // ================= SEC-C-15: Profile Name allowlist =================
    // Anchor: SECURITY-REVIEW-PHASE-C.md SEC-C-15 + WheelProfile.cs —
    // SanitizeProfileName (A-Z a-z 0-9 space _ -, max 64): na-jaiz
    // characters hatao (reject nahi — Load kharab naam par poori file ko
    // backup/defaults par nahi girata); IsValidProfileName sakht check hai.

    [Theory]
    [InlineData("../../evil")]
    [InlineData("a/b")]
    [InlineData(@"C:\evil")]
    [InlineData("a*b?c")]
    [InlineData("name\twith-control")]
    public void ProfileName_BadNames_AreRejectedOrCleaned(string badName)
    {
        var profile = WheelProfile.CreateDefault(badName);
        var clean = profile.Sanitized().Name;
        Assert.DoesNotContain("/", clean);
        Assert.DoesNotContain("\\", clean);
        Assert.DoesNotContain(":", clean);
        Assert.DoesNotContain("*", clean);
        Assert.DoesNotContain("?", clean);
        Assert.DoesNotContain("\"", clean);
        Assert.DoesNotContain("<", clean);
        Assert.DoesNotContain(">", clean);
        Assert.DoesNotContain("|", clean);
        Assert.True(clean.All(c => c is >= ' ' and <= '~' and not '\t'),
            $"SEC-C-15 OPEN (WheelProfile.cs Sanitized): Name '{badName}' se '{clean}' bana — " +
            "allowlist (A-Z a-z 0-9 space _ -) ya control chars ki safai nahi hai.");
    }

    [Fact]
    public void ProfileName_LongName_IsCappedAt64()
    {
        string longName = new('A', 100);
        string clean = WheelProfile.CreateDefault(longName).Sanitized().Name;
        Assert.True(clean.Length <= 64,
            $"SEC-C-15 OPEN (WheelProfile.cs Sanitized): 100-char Name {clean.Length} chars ka raha — max 64 hona chahiye.");
    }

    [Fact]
    public void ProfileName_GoodName_PassesThrough()
    {
        string clean = WheelProfile.CreateDefault("GT Racing-1_2").Sanitized().Name;
        Assert.Equal("GT Racing-1_2", clean);
    }

    // ================= SEC-C-18: boardMac mismatch warning =================
    // Anchor: WheelProfile.cs BoardBindingWarning + ProfilesSystem.cs:65,319
    // (consumer — warning UI tak jata hai). Predicate (MatchesBoard) +
    // warning API + UI wiring — teeno maujood hain (Phase C/E + F).

    [Fact]
    public void BoardMac_Mismatch_IsDetected()
    {
        var profile = WheelProfile.CreateDefault() with { BoardMac = "AA:BB:CC:DD:EE:FF" };
        Assert.False(profile.MatchesBoard("11:22:33:44:55:66"));
    }

    [Fact]
    public void BoardMac_Match_AcceptsSameMacAnyFormat()
    {
        var profile = WheelProfile.CreateDefault() with { BoardMac = "AA:BB:CC:DD:EE:FF" };
        Assert.True(profile.MatchesBoard("aa-bb-cc-dd-ee-ff"));
        Assert.True(profile.MatchesBoard("AABBCCDDEEFF"));
    }

    [Fact]
    public void BoardMac_NullProfile_MatchesAnyBoard()
    {
        var profile = WheelProfile.CreateDefault() with { BoardMac = null };
        Assert.True(profile.MatchesBoard("11:22:33:44:55:66"));
    }

    [Fact]
    public void BoardBindingWarning_Mismatch_ReturnsWarningText()
    {
        var profile = WheelProfile.CreateDefault() with { BoardMac = "AA:BB:CC:DD:EE:FF" };
        string? warning = WheelProfile.BoardBindingWarning(profile, "11:22:33:44:55:66");
        Assert.NotNull(warning);
        Assert.Contains("AA:BB:CC:DD:EE:FF", warning, StringComparison.OrdinalIgnoreCase);
        Assert.Contains("11:22:33:44:55:66", warning, StringComparison.OrdinalIgnoreCase);
    }

    [Fact]
    public void BoardBindingWarning_MatchOrUnknown_ReturnsNull()
    {
        var bound = WheelProfile.CreateDefault() with { BoardMac = "AA:BB:CC:DD:EE:FF" };
        Assert.Null(WheelProfile.BoardBindingWarning(bound, "aa-bb-cc-dd-ee-ff"));
        var unbound = WheelProfile.CreateDefault() with { BoardMac = null };
        Assert.Null(WheelProfile.BoardBindingWarning(unbound, "11:22:33:44:55:66"));
        Assert.Null(WheelProfile.BoardBindingWarning(null, "11:22:33:44:55:66"));
        Assert.Null(WheelProfile.BoardBindingWarning(bound, (string?)null));
        Assert.Null(WheelProfile.BoardBindingWarning(bound, new byte[] { 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF }));
    }

    // ================= mDNS fallback: rsh.local → 192.168.4.1 =================
    // Anchor: EspAddress.cs — D33 (Phase F W3): ResolveAsync mDNS pehle
    // try karta hai, fallback DefaultEspIp (192.168.4.1); mDNS/DNS se mila
    // address bhi private-range check (SEC-C-03) se guzarta hai.

    [Fact]
    public void EspAddress_RejectsHostname_rsh_local()
    {
        Assert.False(EspAddress.TryValidate("rsh.local", out _, out _));
    }

    [Fact]
    public void EspAddress_AcceptsPrivateIp()
    {
        Assert.True(EspAddress.TryValidate("192.168.4.1", out var address, out _));
        Assert.Equal("192.168.4.1", address!.ToString());
    }

    // mDNS resolve (D33) ka poora coverage PhaseFWorkstream3Tests mein
    // hai (EspAddress.ResolveAsync — success, public-IP fallback, throws,
    // no-answer, private literal, other hostname, IsMdnsName) — yahan
    // dobara nahi likha; sirf TryValidate guard (SEC-C-03) oopar hai.

    // ================= Clutch / Handbrake / Shifter / Encoder =================
    // Anchors (Researchers W7 + Phase F WS5 implementation):
    //  - firmware/rsh/modules.h — v2 stream axis slots: 0 steering,
    //    1 accel, 2 brake, 3 clutch, 4 handbrake (WS5 contract change:
    //    handbrake slot 3 se 4 par gaya; wheel.py AXES waisa hi hai).
    //  - ProcessedState.cs — ClutchNorm/ClutchAxis (slot 3),
    //    HandbrakeNorm/HandbrakeAxis (slot 4), Gear (H-pattern bits 5..10),
    //    Enc0Delta..Enc3Delta + EncoderGlitchesFiltered (EncoderTracker:
    //    wrap-aware, |delta| > 32/tick = glitch, baseline aage barhta hai).
    //  - VJoyOutput.cs BuildPosition — clutch → WClutch (V3 driving axis);
    //    handbrake → WSlider (Phase G C8 — docs/HANDBRAKE-MAPPING.md).
    //  - Labels: Clutch "vJoy Axis Rx" jhoota tha (follow-up) — FIXED:
    //    ab "vJoy WClutch" (CalibrationTuning.cs:432, gen_strings.py +
    //    3 resx). Handbrake "vJoy Slider" (Phase G C8 — pehle "no vJoy
    //    mapping yet" tha, faisle se supersede hua).
    //  - WheelProfile mein ClutchCalibration ab hai (Phase F W3
    //    follow-up — pipeline sane profile calibration ko tarjeeh deta
    //    hai, warna DefaultPedalCalibration 300..3300 mV).

    [Fact]
    public void ClutchAndHandbrake_Labels_AreTruthful()
    {
        string tuning = ReadRepoFileOrSkip(Path.Combine("software", "src", "RSH.App", "ViewModels", "CalibrationTuning.cs"));
        Assert.DoesNotContain("vJoy Axis Rx", tuning);
        Assert.DoesNotContain("vJoy Axis Ry", tuning);
        Assert.Contains("vJoy WClutch", tuning, StringComparison.Ordinal);
        // Phase G C8: handbrake ka label ab "vJoy Slider" hai — purana
        // "no vJoy mapping yet" faisle se supersede ho gaya.
        Assert.Contains("vJoy Slider", tuning, StringComparison.Ordinal);
        Assert.DoesNotContain("no vJoy mapping yet", tuning);
        string resx = ReadRepoFileOrSkip(Path.Combine("software", "src", "RSH.App", "Resources", "Strings.resx"));
        Assert.DoesNotContain("vJoy Axis Rx", resx);
        Assert.DoesNotContain("vJoy Axis Ry", resx);
        Assert.Contains("vJoy WClutch", resx, StringComparison.Ordinal);
        Assert.Contains("vJoy Slider", resx, StringComparison.Ordinal);
        Assert.DoesNotContain("no vJoy mapping yet", resx);
    }

    [Fact]
    public void WheelProfile_HasClutchCalibration()
    {
        string src = ReadRepoFileOrSkip(Path.Combine("software", "src", "RSH.Core", "Profiles", "WheelProfile.cs"));
        Assert.Contains("ClutchCalibration", src, StringComparison.Ordinal);
    }

    [Fact]
    public void ClutchSlot3_FullPress_MapsToClutchAxis_AbsentSlot_IsNeutral()
    {
        var pipeline = new ProcessingPipeline(PlainProfile());
        var full = pipeline.ProcessV2(V2Packet(1, new Dictionary<int, ushort> { [3] = 3300 }), 0.005);
        Assert.True(full.ClutchNorm > 0.99, $"ClutchNorm {full.ClutchNorm} — full press par ~1 hona chahiye.");
        Assert.True(full.ClutchAxis > 32000, $"ClutchAxis {full.ClutchAxis} — full press par max ke qareeb hona chahiye.");
        var absent = pipeline.ProcessV2(V2Packet(2, new Dictionary<int, ushort> { [0] = 2050 }), 0.005);
        Assert.Equal(0, absent.ClutchNorm);
        Assert.Equal(InputMapper.AxisMin, absent.ClutchAxis);
    }

    [Fact]
    public void HandbrakeSlot4_FullPress_MapsToHandbrakeAxis_Slot3DoesNot()
    {
        var pipeline = new ProcessingPipeline(PlainProfile());
        var full = pipeline.ProcessV2(V2Packet(1, new Dictionary<int, ushort> { [4] = 3300 }), 0.005);
        Assert.True(full.HandbrakeNorm > 0.99, $"HandbrakeNorm {full.HandbrakeNorm} — full press par ~1 hona chahiye.");
        Assert.True(full.HandbrakeAxis > 32000, $"HandbrakeAxis {full.HandbrakeAxis} — full press par max ke qareeb hona chahiye.");
        // Slot 3 clutch hai (handbrake nahi raha — WS5 contract change):
        var clutchOnly = pipeline.ProcessV2(V2Packet(2, new Dictionary<int, ushort> { [3] = 3300 }), 0.005);
        Assert.Equal(0, clutchOnly.HandbrakeNorm);
        Assert.Equal(InputMapper.AxisMin, clutchOnly.HandbrakeAxis);
        Assert.True(clutchOnly.ClutchNorm > 0.99, "Slot 3 se clutch bharna chahiye, handbrake nahi.");
    }

    [Fact]
    public void Shifter_UpDownBits_MapToVJoyButtons()
    {
        // firmware modules.h: shift-up = bit 0, shift-down = bit 1
        // (v2 stream buttons u32, D1 absolute: bit n = vJoy button n+1).
        var pipeline = new ProcessingPipeline(PlainProfile());
        var up = pipeline.ProcessV2(V2Packet(1, new Dictionary<int, ushort>(), buttons: 0x01), 0.005);
        Assert.Equal(0x01u, up.ButtonsMask);
        var down = pipeline.ProcessV2(V2Packet(2, new Dictionary<int, ushort>(), buttons: 0x02), 0.005);
        Assert.Equal(0x02u, down.ButtonsMask);
        var both = pipeline.ProcessV2(V2Packet(3, new Dictionary<int, ushort>(), buttons: 0x03), 0.005);
        Assert.Equal(0x03u, both.ButtonsMask);
    }

    [Fact]
    public void Shifter_HPattern_GearDecoded_AndGearBitsRemovedFromMask()
    {
        var profile = PlainProfile() with
        {
            ModuleTunings = new Dictionary<string, PerModuleTuning>(StringComparer.OrdinalIgnoreCase)
            {
                ["Shifter"] = new PerModuleTuning { GateMode = ShifterGateMode.HPattern },
            },
        };
        var pipeline = new ProcessingPipeline(profile);
        // bit 5 = gear 1, bit 10 = gear 6; gear bits mask se nikal jate hain.
        var g1 = pipeline.ProcessV2(V2Packet(1, new Dictionary<int, ushort>(), buttons: 0x20), 0.005);
        Assert.Equal(1, g1.Gear);
        Assert.Equal(0u, g1.ButtonsMask);
        var g6 = pipeline.ProcessV2(V2Packet(2, new Dictionary<int, ushort>(), buttons: 0x400), 0.005);
        Assert.Equal(6, g6.Gear);
        Assert.Equal(0u, g6.ButtonsMask);
        // Gear bit + aam button (bit 0) — button rehta hai, gear nikalta hai.
        var mixed = pipeline.ProcessV2(V2Packet(3, new Dictionary<int, ushort>(), buttons: 0x20 | 0x01), 0.005);
        Assert.Equal(1, mixed.Gear);
        Assert.Equal(0x01u, mixed.ButtonsMask);
        // 2 gear bits = invalid → gear 0, gear bits phir bhi mask se bahir.
        var invalid = pipeline.ProcessV2(V2Packet(4, new Dictionary<int, ushort>(), buttons: 0x60), 0.005);
        Assert.Equal(0, invalid.Gear);
        Assert.Equal(0u, invalid.ButtonsMask);
    }

    [Fact]
    public void Encoder_CumulativeDeltas_ParseFromV2Stream()
    {
        // Protocol v2.1 §1.1: encoders cumulative i16.
        var packet = V2Packet(1, new Dictionary<int, ushort> { [0] = 2050 }, enc: [5, -3]);
        Assert.Equal(2, packet.EncoderCount);
        Assert.Equal((short)5, packet.Encoders[0]);
        Assert.Equal((short)-3, packet.Encoders[1]);
    }

    [Fact]
    public void Encoder_Deltas_WrapAware_AndGlitchFiltered()
    {
        var pipeline = new ProcessingPipeline(PlainProfile());
        // Pehla packet baseline (deltas 0).
        var b = pipeline.ProcessV2(V2Packet(1, new Dictionary<int, ushort>(), enc: [100, -50]), 0.005);
        Assert.Equal(0, b.Enc0Delta);
        Assert.Equal(0, b.Enc1Delta);
        Assert.Equal(0, b.EncoderGlitchesFiltered);
        // Chhote steps: delta seedha.
        var s = pipeline.ProcessV2(V2Packet(2, new Dictionary<int, ushort>(), enc: [103, -51]), 0.005);
        Assert.Equal(3, s.Enc0Delta);
        Assert.Equal(-1, s.Enc1Delta);
        Assert.Equal(0, s.EncoderGlitchesFiltered);
        // Bara jump (>32/tick): glitch — delta 0, counter barhe, baseline aage barhe.
        var g = pipeline.ProcessV2(V2Packet(3, new Dictionary<int, ushort>(), enc: [153, -51]), 0.005);
        Assert.Equal(0, g.Enc0Delta);
        Assert.Equal(1, g.EncoderGlitchesFiltered);
        // Wrap: 32760 → -32768 = +8 delta (i16 wrap-aware). Pehle 32760
        // par baseline set hoti hai (153 se jump glitch-filter hota hai).
        var w0 = pipeline.ProcessV2(V2Packet(4, new Dictionary<int, ushort>(), enc: [32760, -51]), 0.005);
        Assert.Equal(0, w0.Enc0Delta);
        Assert.Equal(2, w0.EncoderGlitchesFiltered);
        var w1 = pipeline.ProcessV2(V2Packet(5, new Dictionary<int, ushort>(), enc: [-32768, -51]), 0.005);
        Assert.Equal(8, w1.Enc0Delta);
        Assert.Equal(2, w1.EncoderGlitchesFiltered);
    }

    // ================= SEC-E-04: settings.json mein password nahi =================
    // Anchor: AppSettings.cs — WheelWifiPassword + LegacyWheelWifiPassword
    // dono [JsonIgnore] hain (Phase F W1); password ka ghar DPAPI KeyStore
    // hai (BoardSession.TryGetWifiPassword / TrySaveWifiPassword). Purani
    // file mein plaintext mile to ExtractLegacyPassword use
    // LegacyWheelWifiPassword (in-memory) mein leta hai aur migration
    // ke baad disk copy mit jati hai; ToBackupJson bhi password-free hai.

    [Fact]
    public void SettingsJson_PasswordProperties_AreJsonIgnored()
    {
        string src = ReadRepoFileOrSkip(Path.Combine("software", "src", "RSH.App", "Services", "AppSettings.cs"));
        Assert.Contains("[JsonIgnore]\n    public string WheelWifiPassword", src, StringComparison.Ordinal);
        Assert.Contains("[JsonIgnore]\n    public string LegacyWheelWifiPassword", src, StringComparison.Ordinal);
        Assert.Contains("ToBackupJson", src, StringComparison.Ordinal);
    }

    // ================= SEC-E-10: Stop sirf tracked process =================
    // Anchor: GameLauncher.cs StopAsync (Phase F) — SIRF tracked process
    // ko maarta hai. Pehle ProcessNames par SafeGetProcessesByName loop
    // tha jo user ki khud chalayi hui doosri same-name instance bhi maar
    // deta tha; wo loop hata diya gaya (helper acs.exe ka mamla comment
    // mein likha hai). Ye test pin karta hai ke loop wapas na aaye.

    [Fact]
    public void GameLauncher_Stop_KillsOnlyTrackedProcess()
    {
        string src = ReadRepoFileOrSkip(Path.Combine("software", "src", "RSH.App", "Services", "GameLauncher.cs"));
        int kills = CountOccurrences(src, ".Kill(", StringComparison.Ordinal);
        Assert.True(kills <= 1,
            $"SEC-E-10 OPEN (GameLauncher.cs StopAsync): '.Kill(' {kills} dafa hai — tracked process ke ilawa " +
            "naam se milne wale tamam processes bhi maare jate hain (helper-process loop). " +
            "Stop sirf TrackedProcess (aur us ke helper, agar woh bhi tracked ho) ko mare.");
    }

    // ================= SEC-E-12/13: pin-map cap + GPIO range =================
    // Anchor: PinMapStore.cs (PinMap.TryValidateEntries / ValidatePin / Load).
    //  - SEC-E-12: entry-count cap — MaxPinMapEntries = 15 (Functions ki
    //    tadad); is se zyada entries reject hoti hain.
    //  - SEC-E-13: GPIO range 0–39 — ValidatePin mein sab se pehle check
    //    (PinClass ka default "ok" 40+ ko pehle jaane deta tha); Load bhi
    //    out-of-range pin ko default par chhorta hai (memory mein nahi aata).

    [Fact]
    public void PinMap_ValidatePin_RejectsOutOfRangeGpio()
    {
        string src = ReadRepoFileOrSkip(Path.Combine("software", "src", "RSH.App", "ViewModels", "PinMapStore.cs"));
        Assert.Contains("p > 39", src, StringComparison.Ordinal);
    }

    [Fact]
    public void PinMapStore_EntryCountCap_Exists()
    {
        string src = ReadRepoFileOrSkip(Path.Combine("software", "src", "RSH.App", "ViewModels", "PinMapStore.cs"));
        Assert.Contains("MaxPinMapEntries", src, StringComparison.Ordinal);
        Assert.Contains("TryValidateEntries", src, StringComparison.Ordinal);
    }

    // ================= Pin-map board push =================
    // Anchor: PinMapStore.cs — PinMapBoardPush (Phase F W3): pin map →
    // RSHG config blob (BuildBlob) → SET_CFG chunks (≤ 1024 B) → COMMIT →
    // CONFIRM. Frame-level SET_CFG test oopar hai (SetCfg_FrameBuilds...).
    // NOTE: header comment mein abhi bhi purana jumla hai ("board ko push
    // nahi hota — wo Phase H / SET_CFG ka kaam hai") jab ke push ab hai —
    // neeche doc test is tazad ko pakarta hai (sirf comment ka masla hai).

    [Fact]
    public void PinMap_PushToBoard_Exists()
    {
        string src = ReadRepoFileOrSkip(Path.Combine("software", "src", "RSH.App", "ViewModels", "PinMapStore.cs"));
        Assert.Contains("PinMapBoardPush", src, StringComparison.Ordinal);
        Assert.Contains("BuildBlob", src, StringComparison.Ordinal);
        Assert.Contains("PushAsync", src, StringComparison.Ordinal);
    }

    [Fact]
    public void PinMap_HeaderComment_DoesNotContradictPush()
    {
        string src = ReadRepoFileOrSkip(Path.Combine("software", "src", "RSH.App", "ViewModels", "PinMapStore.cs"));
        Assert.DoesNotContain("board ko push nahi hota", src, StringComparison.Ordinal);
    }

    // ================= Restore live reload =================
    // Anchor: DiagnosticsSettings.cs Restore — "Live reload ka API
    // AppServices mein nahi hai — restart ke baad laagu hota hai" aur
    // status text "Restart RSH to use the restored profiles".

    [Fact]
    public void Restore_LiveReload_Exists()
    {
        string src = ReadRepoFileOrSkip(Path.Combine("software", "src", "RSH.App", "ViewModels", "DiagnosticsSettings.cs"));
        Assert.DoesNotContain("Restart RSH to use the restored profiles", src, StringComparison.Ordinal);
    }

    // ================= i18n runtime switch + C7 =================
    // Anchor: UiStrings.cs (Phase F i18n) — ApplyLanguage("System"/"en"/
    // "ur"/"hi") + INotifyPropertyChanged: bound strings restart ke
    // baghair badalti hain; Settings → Appearance → Language se jorti
    // hai (AppSettings.Language, DiagnosticsSettings.OnLanguageChanged).
    // C7 (brief ke mutabiq — coordinator decision, W4 report):
    // Theme.xaml mein hard-coded #6C4CF1 sirf AxAccent DEFINITION par
    // allowed hai (L7, single source of truth); baaqi har jagah
    // {StaticResource}/{DynamicResource} AxAccent istemal hona chahiye.
    // Definition delete nahi ho sakti — WizardWindow/ChangePasswordDialog
    // Theme.xaml se AxAccent lete hain (RSHTheme.xaml sirf MainWindow
    // mein merge hota hai). Is liye test count <= 1 mangta hai, aur
    // agar 1 ho to woh sirf definition line ho. (Pehle test count == 0
    // mangta tha aur W4 ke kaam se pehle bhi fail tha — count 3 tha.)

    [Fact]
    public void I18n_RuntimeLanguageSetting_Exists()
    {
        string settings = ReadRepoFileOrSkip(Path.Combine("software", "src", "RSH.App", "Services", "AppSettings.cs"));
        Assert.Contains("public string Language", settings, StringComparison.Ordinal);
        string uiStrings = ReadRepoFileOrSkip(Path.Combine("software", "src", "RSH.App", "Resources", "UiStrings.cs"));
        Assert.Contains("public static void ApplyLanguage", uiStrings, StringComparison.Ordinal);
        Assert.Contains("INotifyPropertyChanged", uiStrings, StringComparison.Ordinal);
        string diag = ReadRepoFileOrSkip(Path.Combine("software", "src", "RSH.App", "ViewModels", "DiagnosticsSettings.cs"));
        Assert.Contains("ApplyLanguage", diag, StringComparison.Ordinal);
    }

    [Fact]
    public void UrduStrings_ReadyForRuntimeSwitch()
    {
        string? path = FindRepoFile(Path.Combine("software", "src", "RSH.App", "Resources", "Strings.ur.resx"));
        Assert.True(path is not null, "Strings.ur.resx maujood honi chahiye (runtime switch ke liye resources tayaar hain).");
        string content = File.ReadAllText(path);
        Assert.Contains("NavDashboard", content, StringComparison.Ordinal);
    }

    [Fact]
    public void ThemeXaml_HasNoHardcodedAccent()
    {
        string src = ReadRepoFileOrSkip(Path.Combine("software", "src", "RSH.App", "Themes", "Theme.xaml"));
        int count = CountOccurrences(src, "#6C4CF1", StringComparison.OrdinalIgnoreCase);
        Assert.True(count <= 1,
            $"C7 OPEN (Themes/Theme.xaml): hard-coded '#6C4CF1' abhi {count} dafa hai — " +
            "accent ki definition (AxAccent, L7) ke ilawa har jagah {StaticResource}/{DynamicResource} hona chahiye.");
        if (count == 1)
        {
            // Wahid occurrence AxAccent ki definition hi honi chahiye —
            // koi aur literal (pehle L147/L221 the, W4 ne hata diye) fail hai.
            string line = src.Split('\n')
                .First(l => l.Contains("#6C4CF1", StringComparison.OrdinalIgnoreCase));
            Assert.Contains("x:Key=\"AxAccent\"", line, StringComparison.Ordinal);
        }
    }

    // ================= Game compatibility matrix: DRAFT hi rahe =================
    // Anchor: docs/GAME-COMPATIBILITY.md — verification procedure (Phase F)
    // sirf real build + Windows 11 + asal games par hoti hai; Linux rig
    // par koi game "verified" nahi ho sakta. Ye test jhoote daawe ko rokta hai.

    [Fact]
    public void GameMatrix_StaysDraftUntilRealVerification()
    {
        string doc = ReadRepoFileOrSkip(Path.Combine("docs", "GAME-COMPATIBILITY.md"));
        Assert.Contains("UNVERIFIED DRAFT", doc, StringComparison.Ordinal);
        // Procedure paragraph mein "VERIFIED (Phase F, date)" ka zikr hai
        // (tareeqa, daawa nahi) — asal daawa dated verification hoti hai.
        Assert.DoesNotContain("VERIFIED (Phase F, 2026", doc, StringComparison.Ordinal);
        Assert.DoesNotContain("| **VERIFIED", doc, StringComparison.Ordinal);
    }
}
