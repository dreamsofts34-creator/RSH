using RSH.Core.Pipeline;
using RSH.Core.Processing;
using RSH.Core.Telemetry;

namespace RSH.Core.Profiles;

/// <summary>
/// Ek mukammal profile: calibration + processing settings + button map.
/// JSON mein save hota hai (ProfileStore). Har qeemat ki sane hadain
/// Clamp/validate ProfileStore ke load par bhi laagu hoti hain — kharab
/// ya haath se badli hui file app ko kabhi ajeeb halat mein nahi dalti.
/// </summary>
public sealed record WheelProfile
{
    public string Name { get; init; } = "Default";

    /// <summary>
    /// SEC-C-15: Profile naam ka allowlist — sirf A-Z a-z 0-9 space _ -
    /// (max 64 chars). Separators (<c>/ \ : * ? " &lt; &gt; |</c>) aur
    /// control characters kabhi naam mein nahi rehte: <see cref="Sanitized"/>
    /// unhein hata deta hai (na ke reject — Load kharab naam par poori
    /// file ko backup/defaults par nahi girata), aur
    /// <see cref="IsValidProfileName"/> sakht check hai (UI naye naam
    /// par reject karta hai). <c>../../evil</c> jaisa naam <c>evil</c>
    /// ban jata hai — traversal ka rasta khatam.
    /// </summary>
    public const int MaxProfileNameLength = 64;

    /// <summary>Naam ka ek character allowlist mein hai?</summary>
    public static bool IsAllowedProfileNameChar(char ch) =>
        (ch >= 'A' && ch <= 'Z') || (ch >= 'a' && ch <= 'z') ||
        (ch >= '0' && ch <= '9') || ch == ' ' || ch == '_' || ch == '-';

    /// <summary>
    /// Naam allowlist ke mutabiq saaf karo: na-jaiz characters hatao,
    /// 64 chars par kaato, trim karo; khaali reh jaye to "Default".
    /// Kabhi null nahi deta.
    /// </summary>
    public static string SanitizeProfileName(string? name)
    {
        if (string.IsNullOrWhiteSpace(name)) return "Default";
        var kept = new char[name.Length];
        int n = 0;
        foreach (char ch in name)
            if (IsAllowedProfileNameChar(ch))
                kept[n++] = ch;
        string cleaned = new string(kept, 0, n).Trim();
        if (cleaned.Length > MaxProfileNameLength)
            cleaned = cleaned[..MaxProfileNameLength].TrimEnd();
        return cleaned.Length == 0 ? "Default" : cleaned;
    }

    /// <summary>Sakht check (UI): naam 1..64 chars aur sirf allowlist characters?</summary>
    public static bool IsValidProfileName(string? name) =>
        !string.IsNullOrWhiteSpace(name) &&
        name.Length <= MaxProfileNameLength &&
        name.All(IsAllowedProfileNameChar) &&
        name.Trim().Length > 0;

    /// <summary>
    /// SEC-C-18 (D21): profile-board binding check — UI ko dikhane qaabil
    /// warning, ya null (theek). BoardMac khaali ho (kisi bhi board) ya
    /// board abhi maloom na ho (HELLO nahi aaya) to koi warning nahi —
    /// mismatch sirf tab jab dono maloom hon aur na milein. Mismatch par
    /// dobara calibration ka rasta: wizard se dobara calibrate karo.
    /// </summary>
    public static string? BoardBindingWarning(WheelProfile? profile, string? boardMac)
    {
        if (profile is null) return null;
        if (string.IsNullOrWhiteSpace(profile.BoardMac)) return null;
        if (string.IsNullOrWhiteSpace(boardMac)) return null;
        if (profile.MatchesBoard(boardMac)) return null;
        return $"This profile was made for another wheel (board {profile.BoardMac}) — " +
            $"this wheel is {NormalizeBoardMac(boardMac)}. Its calibration may be wrong here: " +
            "recalibrate from the Setup wizard before driving (D21).";
    }

    /// <summary>BoardMac bytes (HELLO reply / BoardSession.BoardMac) se binding warning — string overload ka wrapper.</summary>
    public static string? BoardBindingWarning(WheelProfile? profile, byte[]? boardMac) =>
        boardMac is null ? null : BoardBindingWarning(profile, Convert.ToHexString(boardMac));

    /// <summary>
    /// Single-profile (import/export) file ka schema version (SEC-C-19,
    /// A9): 0 = v1 (field maujood nahi thi — aisi file v1 samjhi jati
    /// hai), 2 = maujooda. Export hamesha maujooda version likhta hai;
    /// Import is se zyada version reject karta hai. profiles.json ke
    /// andar ProfileFile.SchemaVersion alag hai (poori file ka version)
    /// — ye ek profile ka version hai. Sanitized() isay chhoota nahi.
    /// SEC-E-08: is field ki authority <see cref="ProfileFile.CurrentSchemaVersion"/>
    /// hai — ye field sirf usi ki likhi hui value rakhta hai (koi doosra
    /// "current version" number kahin define nahi hota).
    /// </summary>
    public int SchemaVersion { get; init; }

    public SteeringCalibration SteeringCalibration { get; init; } = new();
    public PedalCalibration AccelCalibration { get; init; } = new();
    public PedalCalibration BrakeCalibration { get; init; } = new();

    /// <summary>
    /// Clutch ki calibration (Phase F W3 follow-up — W6 test
    /// WheelProfile_HasClutchCalibration): PedalCalibration jaisi —
    /// RawMin/RawMax mV mein, <see cref="PedalCalibration.IsMillivolts"/>
    /// tag + span ≥ 660 mV (<see cref="PedalCalibration.IsSane"/>) lazmi.
    /// Set na ho (ya insane ho) to pipeline
    /// <see cref="ProcessingPipeline"/> ki DefaultPedalCalibration
    /// (300..3300 mV) istemal karta hai — clutch kabhi be-calibration
    /// nahi rehta. Wizard abhi clutch capture nahi karta (steering +
    /// pedals hi), is liye ye field profile import / mustaqbil ke wizard
    /// step se aayegi; JSON mein na ho to default (unset) rehti hai.
    /// </summary>
    public PedalCalibration ClutchCalibration { get; init; } = new();

    /// <summary>Center deadzone 0..0.5 (steering).</summary>
    public double Deadzone { get; init; } = 0.02;

    /// <summary>Curve strength -1..1 (0 = linear).</summary>
    public double CurveStrength { get; init; }

    /// <summary>Smoothing 0..1 (0 = off). Pipeline ise One Euro mincutoff mein badalta hai.</summary>
    public double Smoothing { get; init; } = 0.3;

    public bool InvertSteering { get; init; }
    public bool InvertAccel { get; init; }
    public bool InvertBrake { get; init; }

    /// <summary>Dono pedals ek hi axis par (purane games ke liye).</summary>
    public bool CombinePedals { get; init; }

    /// <summary>
    /// vJoy button numbers (1-based) jin par gear buttons map hote hain.
    /// Is app path mein sirf 1..32 (D9): vJoy ke LButtons field mein
    /// sirf 32 bit hain aur protocol v2.1 ke btnsEx ka rasta is path mein
    /// istemal nahi hota. 33–128 reject hote hain — clamp/alias kabhi nahi.
    /// </summary>
    public const int MinVJoyButton = 1;
    public const int MaxVJoyButton = 32;

    public int GearUpVJoyButton { get; init; } = 1;
    public int GearDownVJoyButton { get; init; } = 2;

    /// <summary>Digital pedal ramp ke auqaat (ms).</summary>
    public double PedalPressMs { get; init; } = 150;
    public double PedalReleaseMs { get; init; } = 100;

    /// <summary>
    /// Output rotation scale (industry ke "rotation presets" ka input-only
    /// tarjuma): 1.0 = poora 900°, 0.6 = 540°, 0.4 = 360°. Motor nahi hai
    /// to soft-lock mehsoos nahi hota — pipeline steering ko is se
    /// DIVIDE karta hai (D7): scale 0.4 par steer 0.4 full lock deta hai
    /// (0.4 / 0.4 = 1.0). Ye output limit nahi hai.
    /// </summary>
    public double SteeringScale { get; init; } = 1.0;

    /// <summary>
    /// Jis game ke process ke samne aane par ye profile khud load ho
    /// (masalan "AssettoCorsa"). Khaali ho to auto-switch nahi hota.
    /// </summary>
    public string GameProcessName { get; init; } = "";

    /// <summary>
    /// Phase H (TELEMETRY-READER.md §4): is profile ki car — normalized
    /// lowercase (masalan "ferrari_488_gt3"). Khaali = kisi bhi car par
    /// (game profile). Resolver separator-insensitive match karta hai
    /// (AC ki `ferrari_488_gt3` aur ACC ki `Ferrari 488 GT3 Evo` ek hi
    /// key hain). <b>Schema bump nahi</b> — optional field hai, purani
    /// files baghair is ke load hoti hain. Allowlist: A-Z a-z 0-9 space
    /// _ - . (max 64) — path separators kabhi nahi (Sanitized saaf karta hai).
    /// </summary>
    public string CarModel { get; init; } = "";

    /// <summary>
    /// Phase H (TELEMETRY-READER.md §4): is profile ka game.
    /// <see cref="GameId.Unknown"/> = kisi bhi game par (sirf
    /// <see cref="GameProcessName"/> / CarModel se match). Na-maloom enum
    /// value Sanitized() mein Unknown ban jati hai.
    /// </summary>
    public GameId Game { get; init; } = GameId.Unknown;

    /// <summary>CarModel allowlist — naam allowlist jaisi (SEC-C-15), plus '.' (masalan "488 gt3 evo").</summary>
    public static bool IsAllowedCarModelChar(char ch) =>
        (ch >= 'A' && ch <= 'Z') || (ch >= 'a' && ch <= 'z') ||
        (ch >= '0' && ch <= '9') || ch == ' ' || ch == '_' || ch == '-' || ch == '.';

    /// <summary>CarModel saaf karo: trim + lowercase, na-jaiz characters hatao, 64 par kaato. Kabhi null nahi deta.</summary>
    public static string SanitizeCarModel(string? carModel)
    {
        if (string.IsNullOrWhiteSpace(carModel)) return "";
        string lowered = carModel.Trim().ToLowerInvariant();
        var kept = new char[lowered.Length];
        int n = 0;
        foreach (char ch in lowered)
            if (IsAllowedCarModelChar(ch))
                kept[n++] = ch;
        string cleaned = new string(kept, 0, n).Trim();
        if (cleaned.Length > 64) cleaned = cleaned[..64].TrimEnd();
        return cleaned;
    }

    /// <summary>
    /// Profile v2 (SW-12, P11): per-module tuning + on/off, key module ka
    /// naam hai ("Steering", "Brake", ...). Khali dict = legacy v1 profile
    /// — tuning v1 fields (Deadzone/Curve/Smoothing/Invert/Scale) se
    /// banti hai (PipelinePlanBuilder.FromProfile). Sirf mojood modules
    /// override hote hain; na-maloom key Sanitized() mein hata di jati hai.
    /// </summary>
    public Dictionary<string, PerModuleTuning> ModuleTunings { get; init; } = new();

    /// <summary>
    /// Profile v2 (SW-12, D21): jis board ke liye ye profile bani, us ka
    /// MAC — <b>optional</b>, v2 mein single-device hi support hai. Khaali
    /// ho to profile kisi bhi board par chalti hai. Doosre board par
    /// profile khole to calibration warning ke saath dobara karni hogi
    /// (wo check UI ka kaam hai — yahan sirf field + normalise).
    /// Canonical shakal: uppercase, colon-separated ("AA:BB:CC:DD:EE:FF").
    /// </summary>
    public string? BoardMac { get; init; }

    /// <summary>
    /// Migration faisla — RECALIBRATE PROMPT, tag nahi (Phase C, P9/D10).
    /// v1 files mein calibration ka unit tag (IsMillivolts) hota hi nahi
    /// tha, is liye v1 se aayi unit-less calibration ki unit maloom nahi:
    /// raw 12-bit (0..4095) aur mV (0..3300) ranges taqreeban poori tarah
    /// overlap karti hain (misal center 2050 dono mein mumkin hai) — is
    /// liye values se unit andaza nahi lagaya ja sakta. Blindly mV tag
    /// karna P9 ka bug dobara kholta (silent ghalat mapping: raw center
    /// 2048 ko mV 2048 samajhna range bigar deta hai). Is liye:
    /// <list type="bullet">
    /// <item>values verbatim mehfooz rehti hain (kuch delete nahi hota);</item>
    /// <item>mapper insane calibration ko pehle hi ignore karta hai
    /// (IsSane gate — safe defaults istemal hote hain, ghalat output nahi);</item>
    /// <item>ye flag UI ko batata hai ke dobara calibration karwaye
    /// (recalibrate prompt);</item>
    /// <item>flag khud-ba-khud saaf ho jata hai jab tamam set
    /// calibrations sane ho jayen (Sanitized dobara hisaab karta hai) —
    /// UI ko alag se clear nahi karna parta.</item>
    /// </list>
    /// Flag ka matlab: kam se kam ek SET calibration v2 rules (D10:
    /// mV tag + span ≥ 660 mV) par insane hai. Kabhi set hi na hui
    /// calibration (pehli dafa) par flag nahi lagta — wo normal
    /// first-run flow hai, migration ka nuqsan nahi.
    /// <para>
    /// Ye flag SIRF insane-calibration case hai — persisted, load par
    /// Sanitized se dobara hisaab hota hai. Baqi triggers (pin map
    /// badla, firmware badla, calibration stale — Phase E WS7) live
    /// board context mangte hain, is liye wo
    /// <see cref="CheckRecalibration"/> se check hote hain, is flag
    /// mein persist nahi hote: Wave 2 ki prompt
    /// <see cref="CheckRecalibration"/> parhti hai, ye flag sirf
    /// no-board / migration case ke liye hai. Trigger table:
    /// docs/SETUP-GATES-DECISION.md.
    /// </para>
    /// </summary>
    public bool NeedsRecalibration { get; init; }

    /// <summary>
    /// Calibration ke waqt board ka pin map — <see
    /// cref="CalibrationFreshness.PinMapFingerprint"/> ki shakal mein
    /// (GET_CAPS pin table). Null = maloom nahi (legacy profile) — koi
    /// trigger nahi lagta. Pin map badle to <see
    /// cref="CheckRecalibration"/> PinMapChanged deta hai.
    /// </summary>
    public string? CalibrationPinMapFingerprint { get; init; }

    /// <summary>
    /// Calibration ke waqt board ki firmware version ("4.0.0" jesi
    /// canonical shakal). Null = maloom nahi — koi trigger nahi lagta.
    /// Major/minor badle to <see cref="CheckRecalibration"/>
    /// FirmwareChanged deta hai; patch-only nahi (faisla doc mein).
    /// </summary>
    public string? CalibrationFirmwareVersion { get; init; }

    /// <summary>
    /// Maujooda calibration kab capture hui (UTC). Null = maloom nahi —
    /// koi trigger nahi lagta. <see
    /// cref="CalibrationFreshness.StaleAfterDays"/> din se purani ho to
    /// <see cref="CheckRecalibration"/> StaleCalibration deta hai.
    /// Mustaqbil ki tareekh (clock skew) stale nahi mani jati.
    /// </summary>
    public DateTimeOffset? CalibratedAtUtc { get; init; }

    /// <summary>
    /// Live recalibration check (Phase E WS7): insane calibration +
    /// pin map + firmware + staleness — sab triggers ek saath. Pure hai
    /// (kuch nahi badalta) — Wave 2 ki UI prompt yehi parhti hai.
    /// Koi calibration set hi na ho to koi context trigger nahi lagta
    /// (pehli dafa setup normal flow hai, nuqsan nahi).
    /// </summary>
    public RecalibrationCheck CheckRecalibration(CalibrationContext current)
    {
        ArgumentNullException.ThrowIfNull(current);
        var reasons = RecalibrationReason.None;
        if ((SteeringCalibration.IsSet && !SteeringCalibration.IsSane) ||
            (AccelCalibration.IsSet && !AccelCalibration.IsSane) ||
            (BrakeCalibration.IsSet && !BrakeCalibration.IsSane) ||
            (ClutchCalibration.IsSet && !ClutchCalibration.IsSane))
            reasons |= RecalibrationReason.InsaneCalibration;

        bool anySet = SteeringCalibration.IsSet || AccelCalibration.IsSet || BrakeCalibration.IsSet || ClutchCalibration.IsSet;
        if (anySet)
        {
            string? storedFp = CalibrationFreshness.NormalizeFingerprint(CalibrationPinMapFingerprint);
            string? currentFp = CalibrationFreshness.NormalizeFingerprint(current.PinMapFingerprint);
            if (storedFp is not null && currentFp is not null &&
                !string.Equals(storedFp, currentFp, StringComparison.Ordinal))
                reasons |= RecalibrationReason.PinMapChanged;

            if (CalibrationFreshness.FirmwareChanged(CalibrationFirmwareVersion, current.FirmwareVersion))
                reasons |= RecalibrationReason.FirmwareChanged;

            if (CalibratedAtUtc is { } at)
            {
                var age = current.NowUtc - at;
                if (age > TimeSpan.FromDays(CalibrationFreshness.StaleAfterDays))
                    reasons |= RecalibrationReason.StaleCalibration;
            }
        }
        return new RecalibrationCheck(
            reasons != RecalibrationReason.None,
            reasons,
            CalibrationFreshness.PromptMessageFor(reasons));
    }

    /// <summary>
    /// Kamyaab recalibration par stamps lagao (Phase E WS7 — clear path):
    /// calibration ke waqt ka pin map + firmware version + waqt save
    /// hota hai, phir <see cref="CheckRecalibration"/> usi context par
    /// false deta hai. Wave 2 wizard ke Finish mein nayi calibrations
    /// ke saath ye call kare. Sanitized se guzarta hai — nayi
    /// calibrations sane hon to persisted <see
    /// cref="NeedsRecalibration"/> flag bhi saaf ho jata hai.
    /// </summary>
    public WheelProfile MarkCalibrated(string? pinMapFingerprint, string? firmwareVersion, DateTimeOffset calibratedAtUtc) =>
        (this with
        {
            CalibrationPinMapFingerprint = CalibrationFreshness.NormalizeFingerprint(pinMapFingerprint),
            CalibrationFirmwareVersion = CalibrationFreshness.NormalizeFirmwareVersion(firmwareVersion),
            CalibratedAtUtc = calibratedAtUtc,
        }).Sanitized();

    /// <summary>
    /// BoardMac ko canonical shakal mein lao — khaali/ghalat ho to null.
    /// Ghalat MAC file ko nahi maarta (Sanitized kabhi throw nahi karta
    /// is par) — sirf saaf ho jata hai, baqi profile mehfooz rehti hai.
    /// </summary>
    public static string? NormalizeBoardMac(string? mac)
    {
        if (string.IsNullOrWhiteSpace(mac)) return null;
        string hex = mac.Trim().Replace(":", "").Replace("-", "").ToUpperInvariant();
        if (hex.Length != 12 || hex.Any(c => !Uri.IsHexDigit(c))) return null;
        return string.Join(":", Enumerable.Range(0, 6).Select(i => hex.Substring(i * 2, 2)));
    }

    /// <summary>Ye profile is board ki hai? BoardMac khaali ho to "kisi bhi board" = haan.</summary>
    public bool MatchesBoard(string? boardMac) =>
        BoardMac is null || string.IsNullOrWhiteSpace(boardMac) ||
        string.Equals(BoardMac, NormalizeBoardMac(boardMac), StringComparison.OrdinalIgnoreCase);

    /// <summary>BoardMac bytes (HELLO reply / BoardSession.BoardMac) se match — string overload ka wrapper.</summary>
    public bool MatchesBoard(byte[]? boardMac) =>
        boardMac is null || MatchesBoard(Convert.ToHexString(boardMac));

    /// <summary>
    /// P2 (D9): ek vJoy button number validate karo. Is app path mein
    /// sirf 1..32 — 33–128 (aur 0 ya manfi) reject hote hain, error ke
    /// sath. Clamp kabhi nahi: clamp bhi silent remapping hai, aur
    /// pipeline mein `1u &lt;&lt; (n-1)` C# mein wrap hokar 33+ ko 1..32
    /// par alias kar deta tha.
    /// </summary>
    /// <exception cref="ArgumentOutOfRangeException">Button 1..32 se bahir ho.</exception>
    public static void ValidateVJoyButton(int button, string paramName)
    {
        if (button is < MinVJoyButton or > MaxVJoyButton)
            throw new ArgumentOutOfRangeException(paramName, button,
                $"vJoy button {button} is not allowed in this app path (D9): buttons are 1–32 only. " +
                "Buttons 33–128 are rejected, never clamped or aliased to 1–32.");
    }

    /// <summary>Har qeemat ko us ki mehfooz hadd mein rakhta hai — load ke baad lazmi call hota hai.</summary>
    public WheelProfile Sanitized()
    {
        // P2 (D9): button had se bahir ho to clamp nahi — throw karo.
        // ProfileStore.Load is throw ko corrupt file jaisa treat karke
        // backup/defaults par girta hai, is liye app band nahi hoti.
        ValidateVJoyButton(GearUpVJoyButton, nameof(GearUpVJoyButton));
        ValidateVJoyButton(GearDownVJoyButton, nameof(GearDownVJoyButton));
        int up = GearUpVJoyButton;
        int down = GearDownVJoyButton;
        // AUDIT FIX: gear up/down ek hi vJoy button par hon to game mein
        // dono gear ek saath girte thay — down ko chup chaap agle number
        // par hata do (32 par ho to pichle par).
        if (down == up) down = up < MaxVJoyButton ? up + 1 : up - 1;
        // P11: per-module tuning saaf karo — na-maloom module ki key
        // hatao, hadd se bahar qeematein clamp karo.
        var tunings = new Dictionary<string, PerModuleTuning>(StringComparer.OrdinalIgnoreCase);
        foreach (var (key, tuning) in ModuleTunings)
        {
            if (tuning is null) continue;
            if (!Enum.TryParse<ModuleKind>(key, ignoreCase: true, out var module)) continue;
            tunings[module.ToString()] = tuning.Sanitized();
        }
        // Migration faisla (recalibrate prompt, tag nahi): flag dobara
        // hisaab karo — set magar insane calibration ho to true, warna
        // false (dobara calibration ke baad khud saaf ho jata hai).
        bool needsRecalibration =
            (SteeringCalibration.IsSet && !SteeringCalibration.IsSane) ||
            (AccelCalibration.IsSet && !AccelCalibration.IsSane) ||
            (BrakeCalibration.IsSet && !BrakeCalibration.IsSane) ||
            (ClutchCalibration.IsSet && !ClutchCalibration.IsSane);
        return this with
        {
            // SEC-C-15: naam allowlist (A-Z a-z 0-9 space _ -, max 64) —
            // separators/control characters hatao, khaali ho to "Default".
            Name = SanitizeProfileName(Name),
            Deadzone = Math.Clamp(Deadzone, 0, 0.5),
            CurveStrength = Math.Clamp(CurveStrength, -1, 1),
            Smoothing = Math.Clamp(Smoothing, 0, 1),
            GearUpVJoyButton = up,
            GearDownVJoyButton = down,
            PedalPressMs = Math.Clamp(PedalPressMs, 10, 2000),
            PedalReleaseMs = Math.Clamp(PedalReleaseMs, 10, 2000),
            SteeringScale = Math.Clamp(SteeringScale, 0.2, 1.0),
            GameProcessName = (GameProcessName ?? "").Trim().Replace(".exe", "", StringComparison.OrdinalIgnoreCase),
            // Phase H (TELEMETRY-READER.md §4): per-car fields — CarModel
            // normalize (lowercase + allowlist, path separators kabhi
            // nahi), Game validate (na-maloom enum → Unknown).
            CarModel = SanitizeCarModel(CarModel),
            Game = Enum.IsDefined(Game) ? Game : GameId.Unknown,
            BoardMac = NormalizeBoardMac(BoardMac),
            ModuleTunings = tunings,
            NeedsRecalibration = needsRecalibration,
            // Phase E WS7 stamps: normalise + preserve — khaali/ghalat
            // ho to null (unknown = koi trigger nahi). Sanitized inhein
            // kabhi dobara hisaab nahi karta (live context chahiye).
            CalibrationPinMapFingerprint = CalibrationFreshness.NormalizeFingerprint(CalibrationPinMapFingerprint),
            CalibrationFirmwareVersion = CalibrationFreshness.NormalizeFirmwareVersion(CalibrationFirmwareVersion),
            CalibratedAtUtc = CalibratedAtUtc,
        };
    }

    public static WheelProfile CreateDefault(string name = "Default") => new() { Name = name };
}
