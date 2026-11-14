using RSH.Core.Output;
using RSH.Core.Pipeline;
using RSH.Core.Profiles;
using RSH.Core.Protocol;

namespace RSH.Core.Processing;

/// <summary>
/// Poora processing silsila ek packet par (docs 02 section 6.2):
/// map → deadzone → curve → One Euro smoothing → pedal ramp fusion →
/// vJoy axis values. 200 Hz par pipeline thread se bulaya jata hai.
///
/// P11 (Phase C): pipeline <see cref="PipelinePlan"/> se chalti hai —
/// har module ki apni tuning + on/off. Band module ka data pipeline se
/// nahi guzarta (us ka axis neutral rehta hai). Plan na diya jaye to
/// profile se bana plan istemal hota hai
/// (<see cref="PipelinePlanBuilder.FromProfile"/>), is liye purana
/// behaviour bilkul waisa hi rehta hai. <see cref="SetPlan"/> plan
/// badalne par pipeline dobara wire karta hai (filters dobara bante hain).
///
/// Smoothing ka tarjuma (profile 0..1 → One Euro mincutoff):
/// 0 = filter band; warna mincutoff 25 Hz (halki) se 1.5 Hz (sakht) tak
/// linear. Beta 0.1 (author ki misaal), dcutoff 1.0, freq 200.
/// Ye numbers asal wheel par tune honge — yahan "research defaults" hain,
/// koi jadooi dawa nahi.
/// </summary>
public sealed class ProcessingPipeline
{
    public const double SampleFrequencyHz = 200.0;

    // AUDIT FIX: SetProfile UI thread se aur Process engine thread se
    // chalta hai — baghair lock ke profile/filter ka adla-badli ek tick ke
    // liye purane filter + nayi profile ka jor bana sakta tha. Ab tamam
    // access ek hi lock se guzarta hai (200 Hz par lock sasta hai).
    private readonly object _gate = new();
    private WheelProfile _profile;
    private PipelinePlan _plan;
    private OneEuroFilter? _steerFilter;
    private OneEuroFilter? _accelFilter;
    private OneEuroFilter? _brakeFilter;
    private OneEuroFilter? _clutchFilter;
    private OneEuroFilter? _handbrakeFilter;
    private PedalRamp _ramp;
    private readonly EncoderTracker _encoderTracker = new();
    private readonly short[] _encoderDeltas = new short[EncoderTracker.MaxEncoders];

    /// <summary>
    /// Clutch/handbrake ki default calibration (Phase F WS5): pedal pots
    /// rest par ~300 mV dete hain (wheel.py PEDAL_MIN; v1 mv_to_raw bhi
    /// 300 mV → raw 0 karta hai). Profile mein clutch calibration na ho
    /// (ya insane ho) to ye default 300..3300 mV (span 3000 ≥ D10 660)
    /// istemal hoti hai taake rest = 0% aur full press = 100% mile —
    /// <see cref="ClutchCalibrationOrDefault"/> dekho. Handbrake ki
    /// profile calibration abhi bhi nahi hai (W2 ka baaqi hissa nahi —
    /// sirf clutch W6 test ka mutalba tha), is liye handbrake hamesha
    /// isi default par chalta hai. Accel/brake ki profile calibration
    /// is se mutasir nahi hoti.
    /// </summary>
    internal static readonly PedalCalibration DefaultPedalCalibration = new()
    {
        RawMin = 300, RawMax = 3300, IsSet = true, IsMillivolts = true,
    };

    /// <summary>
    /// Clutch calibration ka intikhab (Phase F W3 follow-up): profile
    /// ki calibration sane ho (<see cref="PedalCalibration.IsSane"/> —
    /// set + mV tag + span ≥ 660 mV) to wahi, warna
    /// <see cref="DefaultPedalCalibration"/> (300..3300 mV). Seedha
    /// profile calibration dena kaafi nahi — <see
    /// cref="InputMapper.MapPedal"/> insane calibration ko 0..3300 par
    /// gira deta hai, jis se rest (300 mV) ~9% ban jata; default
    /// fallback rest = 0% barkarar rakhta hai.
    /// </summary>
    internal static PedalCalibration ClutchCalibrationOrDefault(WheelProfile profile) =>
        profile.ClutchCalibration.IsSane ? profile.ClutchCalibration : DefaultPedalCalibration;

    public ProcessingPipeline(WheelProfile profile)
    {
        _profile = profile.Sanitized();
        _plan = PipelinePlanBuilder.FromProfile(_profile);
        _ramp = new PedalRamp(_profile.PedalPressMs, _profile.PedalReleaseMs);
        RebuildFilters();
    }

    public WheelProfile Profile { get { lock (_gate) return _profile; } }

    /// <summary>Maujooda plan — profile se bana ho ya <see cref="SetPlan"/> se aaya ho.</summary>
    public PipelinePlan Plan { get { lock (_gate) return _plan; } }

    /// <summary>
    /// Profile/settings badle jayen to filter aur ramp saaf halat se shuru
    /// hote hain — lekin AUDIT improvement: jo cheez nahi badli, us ki
    /// state bhi reset nahi hoti (masalan sirf button mapping badalne par
    /// smoothing ki history mehfooz rehti hai).
    /// Plan profile se dobara banta hai — <see cref="SetPlan"/> se diya
    /// gaya custom plan profile badalne par profile wale plan se badal
    /// jata hai (plan profile ka hi aks hai jab tak SetPlan na karo).
    /// </summary>
    public void SetProfile(WheelProfile profile)
    {
        lock (_gate)
        {
            var next = profile.Sanitized();
            if (next.PedalPressMs != _profile.PedalPressMs || next.PedalReleaseMs != _profile.PedalReleaseMs)
                _ramp = new PedalRamp(next.PedalPressMs, next.PedalReleaseMs);
            // Sirf tab filters dobara banao jab kisi analog module ki
            // smoothing badli ho — warna filter ki history mehfooz rehti
            // hai (masalan sirf button mapping badalne par).
            var oldPlan = _plan;
            _profile = next;
            _plan = PipelinePlanBuilder.FromProfile(next);
            if (oldPlan[ModuleKind.Steering].Tuning.Smoothing != _plan[ModuleKind.Steering].Tuning.Smoothing ||
                oldPlan[ModuleKind.Accelerator].Tuning.Smoothing != _plan[ModuleKind.Accelerator].Tuning.Smoothing ||
                oldPlan[ModuleKind.Brake].Tuning.Smoothing != _plan[ModuleKind.Brake].Tuning.Smoothing ||
                oldPlan[ModuleKind.Clutch].Tuning.Smoothing != _plan[ModuleKind.Clutch].Tuning.Smoothing ||
                oldPlan[ModuleKind.Handbrake].Tuning.Smoothing != _plan[ModuleKind.Handbrake].Tuning.Smoothing)
                RebuildFilters();
        }
    }

    /// <summary>
    /// P11: naya plan lagao — pipeline dobara wire hoti hai (har module
    /// ki tuning + on/off, filters per-module smoothing se dobara bante
    /// hain). Ghalat plan (sirf enabled modules validate hote hain)
    /// <see cref="PipelinePlanValidationException"/> phenkta hai aur
    /// purana plan barkarar rehta hai.
    /// </summary>
    public void SetPlan(PipelinePlan plan)
    {
        ArgumentNullException.ThrowIfNull(plan);
        var errors = plan.Validate();
        if (errors.Count > 0)
            throw new PipelinePlanValidationException(errors);
        lock (_gate)
        {
            _plan = plan;
            RebuildFilters();
            _ramp.Reset();   // naya plan = saaf processing state (filters ki tarah)
        }
    }

    private void RebuildFilters()
    {
        // P11: har analog module ki apni smoothing — steering, accel,
        // brake, clutch, handbrake ke filters alag alag cutoff se bante hain.
        _steerFilter = MakeFilter(_plan[ModuleKind.Steering].Tuning.Smoothing);
        _accelFilter = MakeFilter(_plan[ModuleKind.Accelerator].Tuning.Smoothing);
        _brakeFilter = MakeFilter(_plan[ModuleKind.Brake].Tuning.Smoothing);
        _clutchFilter = MakeFilter(_plan[ModuleKind.Clutch].Tuning.Smoothing);
        _handbrakeFilter = MakeFilter(_plan[ModuleKind.Handbrake].Tuning.Smoothing);
    }

    private static OneEuroFilter? MakeFilter(double smoothing)
    {
        if (smoothing <= 0) return null;
        double minCutoff = 25.0 - 23.5 * smoothing;
        return new OneEuroFilter(SampleFrequencyHz, minCutoff, beta: 0.1);
    }

    /// <summary>
    /// Ek packet process karo.
    /// <paramref name="buttonsEx"/>: taaza StatusPacketV2.BtnsEx word
    /// (0-based buttons 32..63 = vJoy 33..64) — 1 Hz status packet se
    /// aata hai, 200 Hz stream se nahi (stream mein sirf buttons 0..31
    /// hain). Buttons module band ho to Ex bits bhi 0 hote hain (P11 rule).
    /// Gear mapping (profile, 1..32) se is ka koi talluq nahi — Ex bits
    /// absolute button numbers hain (D1) aur raw pass hote hain.
    /// </summary>
    public ProcessedState Process(in WheelPacket packet, double dtSeconds, uint buttonsEx = 0)
    {
        lock (_gate)
        {
        var p = _profile;
        var plan = _plan;

        // --- Steering (P11: plan ki Steering entry — band ho to neutral) ---
        double steer = 0;
        if (plan.IsEnabled(ModuleKind.Steering))
        {
            var t = plan[ModuleKind.Steering].Tuning;
            steer = ApplySteeringModule(packet.Steering, packet.SteeringInMillivolts, p.SteeringCalibration, t, _steerFilter, dtSeconds);
        }

        // --- Pedals: analog aur digital (ramp) mein se jo zyada ho ---
        // P11: har pedal ki apni tuning (deadzone/curve/invert plan se);
        // band pedal ka data pipeline se nahi guzarta (0 rehta hai).
        _ramp.Update(packet.AccelButton, packet.BrakeButton, dtSeconds);
        double accel = ApplyPedalModule(packet.Accel, p.AccelCalibration, _ramp.Accel,
            plan, ModuleKind.Accelerator, _accelFilter, dtSeconds);
        double brake = ApplyPedalModule(packet.Brake, p.BrakeCalibration, _ramp.Brake,
            plan, ModuleKind.Brake, _brakeFilter, dtSeconds);

        // --- Buttons (P11: band ho to mask 0) ---
        uint mask = 0;
        ulong maskEx = 0;
        if (plan.IsEnabled(ModuleKind.Buttons))
        {
            // P2 (D9): is app path mein vJoy buttons sirf 1..32. Sanitized()
            // aam tor par ye laagu karta hai, lekin yahan dobara check karo:
            // C# mein `1u << 32` wrap hokar `1u << 0` ban jata hai, is liye
            // button 33 baghair guard ke chup chaap button 1 par alias ho
            // jata tha (64 → 32, 65 → 1, ...). >32 kabhi bit mein nahi
            // badalna — error do. Exception type Sanitized()/ValidateVJoyButton
            // wali hi hai (ArgumentOutOfRangeException) taake callers ek hi
            // catch se handle kar sakein.
            if (p.GearUpVJoyButton is < WheelProfile.MinVJoyButton or > WheelProfile.MaxVJoyButton)
                throw new ArgumentOutOfRangeException(nameof(WheelProfile.GearUpVJoyButton), p.GearUpVJoyButton,
                    "vJoy button is not allowed: buttons are 1–32 only (D9). Buttons 33–128 are rejected, never clamped or aliased.");
            if (p.GearDownVJoyButton is < WheelProfile.MinVJoyButton or > WheelProfile.MaxVJoyButton)
                throw new ArgumentOutOfRangeException(nameof(WheelProfile.GearDownVJoyButton), p.GearDownVJoyButton,
                    "vJoy button is not allowed: buttons are 1–32 only (D9). Buttons 33–128 are rejected, never clamped or aliased.");
            if (packet.GearUp) mask |= 1u << (p.GearUpVJoyButton - 1);
            if (packet.GearDown) mask |= 1u << (p.GearDownVJoyButton - 1);
            // T11/Ex: BtnsEx word (0-based buttons 32..63) seedha
            // ButtonsMaskEx ke neeche 32 bits mein (vJoy 33..64 → Ex1).
            // Oopar 32 bits (65..96 → Ex2) ka abhi koi source nahi.
            maskEx = buttonsEx;
        }

        // --- Axes ---
        int accelAxis, brakeAxis;
        if (p.CombinePedals)
        {
            accelAxis = InputMapper.CombinedPedalsToAxis(accel, brake);
            brakeAxis = InputMapper.AxisMin;
        }
        else
        {
            accelAxis = InputMapper.PedalToAxis(accel);
            brakeAxis = InputMapper.PedalToAxis(brake);
        }

        return new ProcessedState(
            SteeringAxis: InputMapper.SteeringToAxis(steer),
            AccelAxis: accelAxis,
            BrakeAxis: brakeAxis,
            ButtonsMask: mask,
            SteeringNorm: steer,
            AccelNorm: accel,
            BrakeNorm: brake,
            ButtonsMaskEx: maskEx);
        }
    }

    /// <summary>
    /// v2.1 stream packet process karo (Phase D — D6: sab axes mV).
    /// Axis slots (default mapping — wheel.py AXES + Plan v2.3 §4 module
    /// order jaisi; GET_CAPS pin table se remap Phase E ka kaam hai):
    /// slot 0 = steering, 1 = accelerator, 2 = brake, 3 = clutch,
    /// 4 = handbrake. Slot maujood na ho to us module ka output neutral
    /// rehta hai (0). Buttons D1 ke mutabiq absolute hain: stream buttons
    /// u32 bit n = vJoy button n+1 (1..32), BtnsEx (status se,
    /// <paramref name="buttonsEx"/>) = vJoy 33..64 (Ex1). Profile ki
    /// GearUp/GearDown → vJoy remap sirf v1 legacy path ki hai — v2 mein
    /// button numbers absolute hain.
    /// Digital accel/brake ramp v2 mein nahi hai (wire par koi digital
    /// pedal bits nahi) — pedals sirf analog mV se aate hain.
    /// Clutch/handbrake (Phase F WS5): apni per-module tuning
    /// (deadzone/curve/smoothing) se guzarte hain; band module ya
    /// ghair-maujood slot → 0. Shifter gate mode (Phase F WS5):
    /// Sequential → gear bits 0/1 aam buttons; HPattern → bits 5..10
    /// gear hain (Gear 0..6, mask se hata diye jate hain).
    /// Encoders: cumulative i16 se wrap-aware delta (EncoderTracker,
    /// glitch-filtered) — ProcessedState.Enc0Delta..3 mein; deltas se
    /// vJoy pulse buttons (65..72) EncoderPulseMask mein. Encoders
    /// module band ho to deltas 0 aur tracker reset (P11).
    /// </summary>
    public ProcessedState ProcessV2(in WheelPacketV2 packet, double dtSeconds, uint buttonsEx = 0)
    {
        lock (_gate)
        {
            var p = _profile;
            var plan = _plan;

            double steer = 0;
            if (plan.IsEnabled(ModuleKind.Steering) && packet.AxisPresent(SteeringSlot))
            {
                var t = plan[ModuleKind.Steering].Tuning;
                steer = ApplySteeringModule(packet.Axis(SteeringSlot), inMillivolts: true, p.SteeringCalibration, t, _steerFilter, dtSeconds);
            }

            _ramp.Update(false, false, dtSeconds);
            double accel = 0, brake = 0;
            if (plan.IsEnabled(ModuleKind.Accelerator) && packet.AxisPresent(AccelSlot))
                accel = ApplyPedalModule(packet.Axis(AccelSlot), p.AccelCalibration, _ramp.Accel,
                    plan, ModuleKind.Accelerator, _accelFilter, dtSeconds, inMillivolts: true);
            if (plan.IsEnabled(ModuleKind.Brake) && packet.AxisPresent(BrakeSlot))
                brake = ApplyPedalModule(packet.Axis(BrakeSlot), p.BrakeCalibration, _ramp.Brake,
                    plan, ModuleKind.Brake, _brakeFilter, dtSeconds, inMillivolts: true);

            // --- Clutch / handbrake (Phase F WS5 + W3 follow-up) ---
            // Clutch: profile ki sane calibration ho to wahi, warna
            // DefaultPedalCalibration (300..3300 mV) — ClutchCalibrationOrDefault.
            // Handbrake ki profile calibration abhi nahi — hamesha default.
            // Deadzone/curve/smoothing/invert plan se aate hain.
            double clutch = 0, handbrake = 0;
            if (plan.IsEnabled(ModuleKind.Clutch) && packet.AxisPresent(ClutchSlot))
                clutch = ApplyPedalModule(packet.Axis(ClutchSlot), ClutchCalibrationOrDefault(p), 0,
                    plan, ModuleKind.Clutch, _clutchFilter, dtSeconds, inMillivolts: true);
            if (plan.IsEnabled(ModuleKind.Handbrake) && packet.AxisPresent(HandbrakeSlot))
                handbrake = ApplyPedalModule(packet.Axis(HandbrakeSlot), DefaultPedalCalibration, 0,
                    plan, ModuleKind.Handbrake, _handbrakeFilter, dtSeconds, inMillivolts: true);

            // --- Shifter gate mode (Phase F WS5) ---
            // HPattern: bits 5..10 gear hain — Shifter enabled ho to Gear
            // nikalo (baaqi bits aam buttons rehte hain). Sequential ya
            // Shifter band ho to Gear 0.
            int gear = 0;
            uint gearBits = 0;
            if (plan.IsEnabled(ModuleKind.Shifter)
                && plan[ModuleKind.Shifter].Tuning.GateMode == ShifterGateMode.HPattern)
            {
                gearBits = packet.Buttons & GearBitsMask;
                gear = DecodeHpatternGear(gearBits);
            }

            // --- Encoders (Phase F WS5): cumulative → delta, glitch-filtered ---
            // P11: Encoders module band ho to deltas/pulses pipeline se
            // nahi guzarte — tracker reset (dobara on hone par purani
            // baseline se jhoota bara delta na bane) aur deltas 0.
            ulong encoderPulseMask = 0;
            if (plan.IsEnabled(ModuleKind.Encoders))
            {
                _encoderTracker.Update(packet.Encoders, packet.EncoderCount, _encoderDeltas);
                encoderPulseMask = EncoderPulseMask(_encoderDeltas);
            }
            else
            {
                _encoderTracker.Reset();
                Array.Clear(_encoderDeltas);
            }

            uint mask = 0;
            ulong maskEx = 0;
            if (plan.IsEnabled(ModuleKind.Buttons))
            {
                // D1: v2 buttons absolute — bit n = button n (vJoy n+1).
                // HPattern mein gear bits mask se hata do (wo gear state
                // hain, buttons nahi).
                mask = packet.Buttons & ~gearBits;
                // Round 2 B2-08: Horn module band ho to horn bit (bit 4)
                // bhi hata do — Horn toggle pehle no-op tha.
                if (!plan.IsEnabled(ModuleKind.Horn))
                    mask &= ~HornButtonBit;
                // T11/Ex: BtnsEx word (0-based buttons 32..63) seedha
                // ButtonsMaskEx ke neeche 32 bits mein (vJoy 33..64 → Ex1).
                // Oopar 32 bits (65..96 → Ex2) aur Ex3 (97..128) ka v2.1
                // mein koi wire source nahi (§1.1 N1: wire max 64 buttons)
                // — wo hamesha 0 rehte hain (v2.2 mein doosra extension
                // word define ho sakta hai; yahan fabricate nahi karte).
                // Encoder pulses (65..72) ButtonsMaskEx mein NAHI aate —
                // wo alag field EncoderPulseMask mein hain (neeche dekho).
                maskEx = buttonsEx;
            }

            int accelAxis, brakeAxis;
            if (p.CombinePedals)
            {
                accelAxis = InputMapper.CombinedPedalsToAxis(accel, brake);
                brakeAxis = InputMapper.AxisMin;
            }
            else
            {
                accelAxis = InputMapper.PedalToAxis(accel);
                brakeAxis = InputMapper.PedalToAxis(brake);
            }

            return new ProcessedState(
                SteeringAxis: InputMapper.SteeringToAxis(steer),
                AccelAxis: accelAxis,
                BrakeAxis: brakeAxis,
                ButtonsMask: mask,
                SteeringNorm: steer,
                AccelNorm: accel,
                BrakeNorm: brake,
                ButtonsMaskEx: maskEx,
                ClutchNorm: clutch,
                ClutchAxis: InputMapper.PedalToAxis(clutch),
                HandbrakeNorm: handbrake,
                HandbrakeAxis: InputMapper.PedalToAxis(handbrake),
                Gear: gear,
                Enc0Delta: _encoderDeltas[0],
                Enc1Delta: _encoderDeltas[1],
                Enc2Delta: _encoderDeltas[2],
                Enc3Delta: _encoderDeltas[3],
                EncoderGlitchesFiltered: _encoderTracker.GlitchesFiltered,
                EncoderPulseMask: encoderPulseMask);
        }
    }

    /// <summary>v2.1 stream mein axis slots (default mapping — GET_CAPS remap Phase E).</summary>
    public const int SteeringSlot = 0;
    /// <summary>v2.1 stream mein accelerator axis slot.</summary>
    public const int AccelSlot = 1;
    /// <summary>v2.1 stream mein brake axis slot.</summary>
    public const int BrakeSlot = 2;
    /// <summary>v2.1 stream mein clutch axis slot (Phase F WS5 — wheel.py AXES jaisa).</summary>
    public const int ClutchSlot = 3;
    /// <summary>v2.1 stream mein handbrake axis slot (Phase F WS5 — pehle slot 3 tha, clutch ke liye khaali kiya).</summary>
    public const int HandbrakeSlot = 4;

    /// <summary>H-pattern gear bits: stream buttons bit 5..10 (gear 1..6).</summary>
    public const uint GearBitsMask = 0x7E0u;

    /// <summary>
    /// Horn button bit (firmware protocol.h RSH_BTN_HORN = 0x10 — stream
    /// buttons bit 4, vJoy button 5). Round 2 B2-08: Horn module band ho
    /// to ye bit mask se hata di jati hai — pehle Horn toggle ka pipeline
    /// par koi asar hi nahi tha (bit Buttons module se guzar kar hamesha
    /// output tak pahunchti thi; v1 path mein bit kabhi nikli hi nahi —
    /// wahan sirf gear bits map hote hain, legacy parity note).
    /// </summary>
    public const uint HornButtonBit = 0x10u;

    /// <summary>
    /// Encoder → vJoy pulse buttons (Phase F WS5): encoder i CW
    /// (delta &gt; 0) → vJoy button 65+2i, CCW (delta &lt; 0) → 66+2i.
    /// Mask mein bit (n-65) = vJoy button n (ProcessedState
    /// .EncoderPulseMask ki convention). Delta ki magnitude se farq
    /// nahi parta — ek tick mein ek pulse (button us tick daba rehta
    /// hai); 0 delta = koi pulse nahi.
    /// </summary>
    public const int EncoderPulseBaseButton = 65;

    internal static ulong EncoderPulseMask(ReadOnlySpan<short> deltas)
    {
        ulong mask = 0;
        int n = Math.Min(deltas.Length, EncoderTracker.MaxEncoders);
        for (int i = 0; i < n; i++)
        {
            if (deltas[i] > 0) mask |= 1ul << (2 * i);
            else if (deltas[i] < 0) mask |= 1ul << (2 * i + 1);
        }
        return mask;
    }

    /// <summary>
    /// H-pattern gear decode (firmware rsh_shifter_hpattern_gear wala usool):
    /// bits 5..10 mein se EXACTLY ek bit set ho to gear (bit 5 = 1 … bit 10 = 6),
    /// warna 0 (neutral — koi gear nahi ya 2+ gears = invalid).
    /// </summary>
    internal static int DecodeHpatternGear(uint gearBits)
    {
        uint bits = gearBits & GearBitsMask;
        if (bits == 0) return 0;
        if ((bits & (bits - 1)) != 0) return 0;   // 2+ bits — invalid
        int index = 0;
        while ((bits >>= 1) != 0) index++;
        // bit 5 (index 5) → gear 1 … bit 10 (index 10) → gear 6.
        return index - 4;
    }

    /// <summary>
    /// Ek steering module ka hissa: map (mV ya raw) → deadzone → curve →
    /// scale DIVIDE (P1/D7) → smoothing. v1 aur v2 dono isi se guzarte
    /// hain — farq sirf <paramref name="inMillivolts"/> hai.
    /// </summary>
    private static double ApplySteeringModule(int value, bool inMillivolts, SteeringCalibration calibration,
        ModuleTuning tuning, OneEuroFilter? filter, double dtSeconds)
    {
        double steer = InputMapper.MapSteering(value, calibration, inMillivolts, tuning.Invert);
        steer = InputMapper.ApplyDeadzone(steer, tuning.Deadzone);
        steer = InputMapper.ApplyCurve(steer, tuning.CurveStrength);
        // P1 (D7): scale DIVIDE hota hai, multiply nahi. Presets ka matlab
        // rotation range hai (F1 360° = 0.4): steer 0.4 par full lock
        // (0.4 / 0.4 = 1.0), output limit nahi. Multiply karne se F1
        // preset par wheel kabhi full lock tak nahi pahunchta tha.
        // Scale plan validation mein 0.2..1.0 hai, is liye yahan
        // divide-by-zero nahi ho sakta.
        steer = Math.Clamp(steer / tuning.Scale, -1, 1);
        if (filter is not null) steer = Math.Clamp(filter.Filter(steer, dtSeconds), -1, 1);
        return steer;
    }

    /// <summary>
    /// Ek pedal module (Accelerator/Brake) ka hissa: band ho to 0, warna
    /// map → deadzone → curve → ramp max → per-module smoothing.
    /// Deadzone/curve 0..1 domain par endpoints (0/1) mehfooz rakhte hain.
    /// <paramref name="inMillivolts"/>: v1 mein false (pedals raw ADC
    /// 0..4095, andar hi mV mein badalte hain); v2.1 mein true (D6:
    /// sab axes pehle se mV mein hain — dobara convert karna ghalat hoga).
    /// </summary>
    private static double ApplyPedalModule(int raw, PedalCalibration calibration, double rampValue,
        PipelinePlan plan, ModuleKind module, OneEuroFilter? filter, double dtSeconds, bool inMillivolts = false)
    {
        if (!plan.IsEnabled(module)) return 0;
        var t = plan[module].Tuning;
        double value = Math.Max(InputMapper.MapPedal(raw, calibration, t.Invert, inMillivolts), rampValue);
        value = InputMapper.ApplyDeadzone(value, t.Deadzone);
        value = InputMapper.ApplyCurve(value, t.CurveStrength);
        value = Math.Clamp(value, 0, 1);
        if (filter is not null) value = Math.Clamp(filter.Filter(value, dtSeconds), 0, 1);
        return value;
    }

    /// <summary>Filters/ramp/encoder baseline ko saaf karo (reconnect ya calibration ke baad).</summary>
    public void Reset()
    {
        lock (_gate)
        {
            _steerFilter?.Reset();
            _accelFilter?.Reset();
            _brakeFilter?.Reset();
            _clutchFilter?.Reset();
            _handbrakeFilter?.Reset();
            _encoderTracker.Reset();
            Array.Clear(_encoderDeltas);
            _ramp.Reset();
        }
    }
}
