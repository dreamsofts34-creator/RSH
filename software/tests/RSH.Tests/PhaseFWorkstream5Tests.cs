using RSH.Core.Output;
using RSH.Core.Pipeline;
using RSH.Core.Processing;
using RSH.Core.Profiles;
using RSH.Core.Protocol;

namespace RSH.Tests;

/// <summary>
/// Phase F Workstream 5 — Phase E notes: clutch channel, shifter gate
/// mode, encoder pulses/debounce (Plan v2.3 §4, FW-5/FW-11/FW-12, SW-6 P11).
/// </summary>
public class PhaseFWorkstream5Tests
{
    private static ProcessingPipeline DefaultPipeline() =>
        new(WheelProfile.CreateDefault("WS5"));

    /// <summary>v2 packet banao: axes mask order mein (slot ascending).</summary>
    private static WheelPacketV2 V2Packet(
        ushort[] axesMv, uint buttons = 0, short[]? encoders = null, byte axisMask = 0)
    {
        byte mask = axisMask;
        if (mask == 0)
        {
            for (int i = 0; i < axesMv.Length; i++) mask |= (byte)(1 << i);
        }
        byte[] bytes = WheelPacketV2.Build(1, 1000, 0, mask, buttons, axesMv, encoders ?? []);
        Assert.Equal(WheelPacketV2ParseResult.Ok, WheelPacketV2.TryParse(bytes, out var packet));
        return packet;
    }

    // ------------------------------------------------------------------
    // 1) Clutch channel
    // ------------------------------------------------------------------

    [Fact]
    public void Clutch_MapsZeroToHundred()
    {
        var pipeline = DefaultPipeline();
        // Slots: 0 steer 2050 (center), 1 accel rest, 2 brake rest,
        // 3 clutch, 4 handbrake rest.
        var rest = pipeline.ProcessV2(V2Packet([2050, 300, 300, 300, 300]), 0.005);
        Assert.Equal(0.0, rest.ClutchNorm, 6);
        Assert.Equal(1, rest.ClutchAxis);

        var full = pipeline.ProcessV2(V2Packet([2050, 300, 300, 3300, 300]), 0.005);
        Assert.Equal(1.0, full.ClutchNorm, 6);
        Assert.Equal(32768, full.ClutchAxis);

        var half = pipeline.ProcessV2(V2Packet([2050, 300, 300, 1800, 300]), 0.005);
        Assert.Equal(0.5, half.ClutchNorm, 6);
        Assert.Equal(16384 + 1, half.ClutchAxis); // PedalToAxis(0.5) = 1 + 0.5*32767
    }

    [Fact]
    public void Clutch_SlotAbsent_Neutral()
    {
        var pipeline = DefaultPipeline();
        // Sirf slots 0..2 — clutch slot 3 maujood nahi.
        var state = pipeline.ProcessV2(V2Packet([2050, 300, 300]), 0.005);
        Assert.Equal(0.0, state.ClutchNorm, 6);
        Assert.Equal(1, state.ClutchAxis);
    }

    [Fact]
    public void Clutch_DisabledModule_Neutral()
    {
        var pipeline = DefaultPipeline();
        pipeline.SetPlan(new PipelinePlanBuilder()
            .WithEnabled(ModuleKind.Clutch, false).Build());
        var state = pipeline.ProcessV2(V2Packet([2050, 300, 300, 3300, 300]), 0.005);
        Assert.Equal(0.0, state.ClutchNorm, 6);
        Assert.Equal(1, state.ClutchAxis);
    }

    [Fact]
    public void Clutch_PerModuleTuning_DeadzoneCurveInvert()
    {
        var pipeline = DefaultPipeline();
        // Deadzone 0.2: clutch 600 mV = 0.1 norm → deadzone ke andar → 0.
        pipeline.SetPlan(new PipelinePlanBuilder()
            .WithTuning(ModuleKind.Clutch, new ModuleTuning { Deadzone = 0.2 }).Build());
        var dz = pipeline.ProcessV2(V2Packet([2050, 300, 300, 600, 300]), 0.005);
        Assert.Equal(0.0, dz.ClutchNorm, 6);

        // Curve 1.0 (soft center): 0.5 norm → (1-1)*0.5 + 1*0.5³ = 0.125.
        pipeline.SetPlan(new PipelinePlanBuilder()
            .WithTuning(ModuleKind.Clutch, new ModuleTuning { CurveStrength = 1.0 }).Build());
        var curved = pipeline.ProcessV2(V2Packet([2050, 300, 300, 1800, 300]), 0.005);
        Assert.Equal(0.125, curved.ClutchNorm, 6);

        // Invert: rest (300 mV) → 1.0.
        pipeline.SetPlan(new PipelinePlanBuilder()
            .WithTuning(ModuleKind.Clutch, new ModuleTuning { Invert = true }).Build());
        var inv = pipeline.ProcessV2(V2Packet([2050, 300, 300, 300, 300]), 0.005);
        Assert.Equal(1.0, inv.ClutchNorm, 6);
    }

    [Fact]
    public void Handbrake_Slot4_NotSlot3()
    {
        var pipeline = DefaultPipeline();
        // Sirf slot 3 (clutch) bhara ho to handbrake 0 rehta hai —
        // Phase E WS5 mein handbrake ghalti se slot 3 par tha.
        var state = pipeline.ProcessV2(V2Packet([2050, 300, 300, 3300, 300]), 0.005);
        Assert.Equal(0.0, state.HandbrakeNorm, 6);
        Assert.Equal(1, state.HandbrakeAxis);
        Assert.Equal(1.0, state.ClutchNorm, 6);

        // Slot 4 (handbrake) full → 1.0; clutch rest → 0.
        var hb = pipeline.ProcessV2(V2Packet([2050, 300, 300, 300, 3300]), 0.005);
        Assert.Equal(1.0, hb.HandbrakeNorm, 6);
        Assert.Equal(32768, hb.HandbrakeAxis);
        Assert.Equal(0.0, hb.ClutchNorm, 6);
    }

    [Fact]
    public void Clutch_V1Packet_Neutral()
    {
        // v1 mein clutch ka koi wire slot nahi — hamesha neutral.
        var pipeline = DefaultPipeline();
        var state = pipeline.Process(new WheelPacket(1, 0, 2050, 300, 300, 0, true), 0.005);
        Assert.Equal(0.0, state.ClutchNorm, 6);
        Assert.Equal(1, state.ClutchAxis);
        Assert.Equal(0, state.Gear);
        Assert.Equal(0, state.Enc0Delta);
    }

    // ------------------------------------------------------------------
    // 2) Shifter gate mode
    // ------------------------------------------------------------------

    [Fact]
    public void Shifter_Sequential_ButtonsPassThrough_GearZero()
    {
        var pipeline = DefaultPipeline(); // default plan: Sequential
        var up = pipeline.ProcessV2(V2Packet([2050, 300, 300], buttons: 1u << 0), 0.005);
        Assert.Equal(1u, up.ButtonsMask);
        Assert.Equal(0, up.Gear);
        var down = pipeline.ProcessV2(V2Packet([2050, 300, 300], buttons: 1u << 1), 0.005);
        Assert.Equal(2u, down.ButtonsMask);
        Assert.Equal(0, down.Gear);
    }

    [Fact]
    public void Shifter_HPattern_GearDecode_MaskStripped()
    {
        var pipeline = DefaultPipeline();
        pipeline.SetPlan(new PipelinePlanBuilder()
            .WithTuning(ModuleKind.Shifter, new ModuleTuning { GateMode = ShifterGateMode.HPattern })
            .Build());

        var g1 = pipeline.ProcessV2(V2Packet([2050, 300, 300], buttons: 1u << 5), 0.005);
        Assert.Equal(1, g1.Gear);
        Assert.Equal(0u, g1.ButtonsMask); // gear bit mask mein nahi jata

        var g6 = pipeline.ProcessV2(V2Packet([2050, 300, 300], buttons: 1u << 10), 0.005);
        Assert.Equal(6, g6.Gear);
        Assert.Equal(0u, g6.ButtonsMask);

        var g3 = pipeline.ProcessV2(V2Packet([2050, 300, 300], buttons: 1u << 7), 0.005);
        Assert.Equal(3, g3.Gear);

        // Do gears ek saath = invalid → neutral; baaqi button (bit 2) rehta hai.
        var invalid = pipeline.ProcessV2(
            V2Packet([2050, 300, 300], buttons: (1u << 5) | (1u << 6) | (1u << 2)), 0.005);
        Assert.Equal(0, invalid.Gear);
        Assert.Equal(1u << 2, invalid.ButtonsMask);

        // Sequential up/down bits HPattern mein bhi aam buttons rehte hain.
        var up = pipeline.ProcessV2(V2Packet([2050, 300, 300], buttons: (1u << 0) | (1u << 5)), 0.005);
        Assert.Equal(1, up.Gear);
        Assert.Equal(1u, up.ButtonsMask);
    }

    [Fact]
    public void Shifter_HPattern_DisabledModule_GearZero()
    {
        var pipeline = DefaultPipeline();
        pipeline.SetPlan(new PipelinePlanBuilder()
            .WithModule(ModuleKind.Shifter, false,
                new ModuleTuning { GateMode = ShifterGateMode.HPattern }).Build());
        var state = pipeline.ProcessV2(V2Packet([2050, 300, 300], buttons: 1u << 5), 0.005);
        Assert.Equal(0, state.Gear);
    }

    [Fact]
    public void Shifter_V1_HPattern_GearZero()
    {
        // v1 mein H-pattern ka koi wire slot nahi — Gear hamesha 0.
        var pipeline = DefaultPipeline();
        pipeline.SetPlan(new PipelinePlanBuilder()
            .WithTuning(ModuleKind.Shifter, new ModuleTuning { GateMode = ShifterGateMode.HPattern })
            .Build());
        var state = pipeline.Process(new WheelPacket(1, 0, 2050, 300, 300, 0x01, true), 0.005);
        Assert.Equal(0, state.Gear);
        Assert.Equal(1u, state.ButtonsMask); // gear-up v1 remap se button 1
    }

    [Fact]
    public void GateMode_Validation_And_PlanRoundTrip()
    {
        var bad = new ModuleTuning { GateMode = (ShifterGateMode)99 };
        Assert.Contains(bad.Validate(), e => e.Contains("GateMode"));
        Assert.False(bad.IsDefault);
        Assert.Throws<PipelinePlanValidationException>(() =>
            new PipelinePlanBuilder()
                .WithTuning(ModuleKind.Shifter, bad).Build());

        var plan = new PipelinePlanBuilder()
            .WithTuning(ModuleKind.Shifter, new ModuleTuning { GateMode = ShifterGateMode.HPattern })
            .Build();
        Assert.Equal(ShifterGateMode.HPattern, plan[ModuleKind.Shifter].Tuning.GateMode);
        Assert.False(plan[ModuleKind.Shifter].Tuning.IsDefault);

        // Round-trip: plan → profile ModuleTunings → plan (GateMode mehfooz).
        var tunings = PipelinePlanBuilder.ToModuleTunings(plan);
        Assert.Equal(ShifterGateMode.HPattern, tunings["Shifter"].GateMode);
        var profile = WheelProfile.CreateDefault("WS5") with { ModuleTunings = tunings };
        var restored = PipelinePlanBuilder.FromProfile(profile);
        Assert.Equal(ShifterGateMode.HPattern, restored[ModuleKind.Shifter].Tuning.GateMode);

        // Sanitized: ghalat GateMode Sequential par girta hai.
        var sanitized = new PerModuleTuning { GateMode = (ShifterGateMode)7 }.Sanitized();
        Assert.Equal(ShifterGateMode.Sequential, sanitized.GateMode);
    }

    // ------------------------------------------------------------------
    // 3) Encoder pulses / debounce (Core side)
    // ------------------------------------------------------------------

    [Fact]
    public void EncoderTracker_WrapAwareDelta()
    {
        var tracker = new EncoderTracker();
        Span<short> deltas = stackalloc short[4];
        tracker.Update([32767, 0, 0, 0], 1, deltas); // baseline
        Assert.Equal(0, deltas[0]);
        tracker.Update([-32768, 0, 0, 0], 1, deltas);
        Assert.Equal(1, deltas[0]); // wrap +1, -65535 nahi
        tracker.Update([-32768, 5, 0, 0], 2, deltas);
        Assert.Equal(0, deltas[0]);
        Assert.Equal(5, deltas[1]);
    }

    [Fact]
    public void EncoderTracker_GlitchJumpFiltered_BaselineAdvances()
    {
        var tracker = new EncoderTracker();
        Span<short> deltas = stackalloc short[4];
        tracker.Update([0, 0, 0, 0], 1, deltas);
        // +100 ek tick mein namumkin hai (virtual wheel ka bada jump) → filter.
        tracker.Update([100, 0, 0, 0], 1, deltas);
        Assert.Equal(0, deltas[0]);
        Assert.Equal(1, tracker.GlitchesFiltered);
        // Baseline nayi value par hai — agla asal +1 step sahi milta hai.
        tracker.Update([101, 0, 0, 0], 1, deltas);
        Assert.Equal(1, deltas[0]);
        Assert.Equal(1, tracker.GlitchesFiltered);
        // Manfi jump bhi filter hota hai; hadd (32) ke andar chalta hai.
        tracker.Update([101 - 200, 0, 0, 0], 1, deltas);
        Assert.Equal(0, deltas[0]);
        Assert.Equal(2, tracker.GlitchesFiltered);
        tracker.Update([101 - 200 + 32, 0, 0, 0], 1, deltas);
        Assert.Equal(32, deltas[0]);
    }

    [Fact]
    public void EncoderTracker_Reset_Rebaselines()
    {
        var tracker = new EncoderTracker();
        Span<short> deltas = stackalloc short[4];
        tracker.Update([10, 0, 0, 0], 1, deltas);
        tracker.Update([15, 0, 0, 0], 1, deltas);
        Assert.Equal(5, deltas[0]);
        tracker.Reset();
        tracker.Update([100, 0, 0, 0], 1, deltas);
        Assert.Equal(0, deltas[0]); // dobara baseline, jhoota delta nahi
        tracker.Update([0, 0, 0, 0], 0, deltas); // count 0 → baseline reset
        tracker.Update([50, 0, 0, 0], 1, deltas);
        Assert.Equal(0, deltas[0]);
    }

    [Fact]
    public void Encoder_V2_DeltasInProcessedState_GlitchFiltered()
    {
        var pipeline = DefaultPipeline();
        var first = pipeline.ProcessV2(V2Packet([2050, 300, 300], encoders: [0, 0]), 0.005);
        Assert.Equal(0, first.Enc0Delta);
        var second = pipeline.ProcessV2(V2Packet([2050, 300, 300], encoders: [3, -2]), 0.005);
        Assert.Equal(3, second.Enc0Delta);
        Assert.Equal(-2, second.Enc1Delta);
        // +100 jump (glitch) → delta 0, counter barhta hai.
        var glitch = pipeline.ProcessV2(V2Packet([2050, 300, 300], encoders: [103, -2]), 0.005);
        Assert.Equal(0, glitch.Enc0Delta);
        Assert.Equal(1, glitch.EncoderGlitchesFiltered);
        // Us ke baad asal motion phir se track hoti hai.
        var after = pipeline.ProcessV2(V2Packet([2050, 300, 300], encoders: [104, -2]), 0.005);
        Assert.Equal(1, after.Enc0Delta);
        Assert.Equal(1, after.EncoderGlitchesFiltered);
    }

    [Fact]
    public void Encoder_DisabledModule_NoDeltasNoPulses_ReenableRebaselines()
    {
        // P11: band module ka data pipeline se nahi guzarta — ye pehle
        // bug tha (tracker ProcessV2 mein bina gate ke chalta tha).
        var pipeline = DefaultPipeline();
        _ = pipeline.ProcessV2(V2Packet([2050, 300, 300], encoders: [5, 0]), 0.005); // baseline
        var moving = pipeline.ProcessV2(V2Packet([2050, 300, 300], encoders: [8, 0]), 0.005);
        Assert.Equal(3, moving.Enc0Delta);
        Assert.Equal(1ul, moving.EncoderPulseMask); // enc0 CW → vJoy 65 → bit 0

        pipeline.SetPlan(new PipelinePlanBuilder()
            .WithEnabled(ModuleKind.Encoders, false).Build());
        var off = pipeline.ProcessV2(V2Packet([2050, 300, 300], encoders: [20, 0]), 0.005);
        Assert.Equal(0, off.Enc0Delta);
        Assert.Equal(0ul, off.EncoderPulseMask);
        Assert.Equal(0, off.EncoderGlitchesFiltered);

        // Dobara on: purani baseline (8) se jump nahi — 20 nayi baseline.
        pipeline.SetPlan(new PipelinePlanBuilder()
            .WithEnabled(ModuleKind.Encoders, true).Build());
        var rebase = pipeline.ProcessV2(V2Packet([2050, 300, 300], encoders: [20, 0]), 0.005);
        Assert.Equal(0, rebase.Enc0Delta);
        Assert.Equal(0ul, rebase.EncoderPulseMask);
        var resumed = pipeline.ProcessV2(V2Packet([2050, 300, 300], encoders: [21, 0]), 0.005);
        Assert.Equal(1, resumed.Enc0Delta);
        Assert.Equal(1ul, resumed.EncoderPulseMask);
    }

    [Fact]
    public void Encoder_PulseMask_CwCcwMapping()
    {
        // Mapping: enc i CW → vJoy 65+2i (bit 2i), CCW → 66+2i (bit 2i+1).
        var pipeline = DefaultPipeline();
        _ = pipeline.ProcessV2(V2Packet([2050, 300, 300], encoders: [0, 0, 0, 0]), 0.005);
        var s = pipeline.ProcessV2(V2Packet([2050, 300, 300], encoders: [3, -2, 0, 0]), 0.005);
        Assert.Equal(0b1001ul, s.EncoderPulseMask); // bit 0 (65, enc0 CW) + bit 3 (68, enc1 CCW)
        Assert.Equal(3, s.Enc0Delta);
        Assert.Equal(-2, s.Enc1Delta);

        // Koi harkat nahi → koi pulse nahi (button chhoota).
        var still = pipeline.ProcessV2(V2Packet([2050, 300, 300], encoders: [3, -2, 0, 0]), 0.005);
        Assert.Equal(0ul, still.EncoderPulseMask);

        // Magnitude se farq nahi — ek tick mein ek pulse.
        var more = pipeline.ProcessV2(V2Packet([2050, 300, 300], encoders: [3, -2, 1, -1]), 0.005);
        Assert.Equal(0b10010000ul, more.EncoderPulseMask); // bit 4 (69, enc2 CW) + bit 7 (72, enc3 CCW)
    }

    [Fact]
    public void Encoder_PulseMask_ButtonsModuleOff_StillFlows()
    {
        // Pulses Encoders module ka data hain — Buttons module band ho to
        // bhi aate hain (alag field EncoderPulseMask, ButtonsMaskEx nahi).
        var pipeline = DefaultPipeline();
        pipeline.SetPlan(new PipelinePlanBuilder()
            .WithEnabled(ModuleKind.Buttons, false).Build());
        _ = pipeline.ProcessV2(V2Packet([2050, 300, 300], encoders: [0, 0]), 0.005);
        var s = pipeline.ProcessV2(
            V2Packet([2050, 300, 300], buttons: 1u << 0, encoders: [2, 0]), 0.005);
        Assert.Equal(0u, s.ButtonsMask);
        Assert.Equal(0ul, s.ButtonsMaskEx);
        Assert.Equal(1ul, s.EncoderPulseMask);
    }

    // ------------------------------------------------------------------
    // 4) SET_CFG shifter record (C# factory — firmware type 11)
    // ------------------------------------------------------------------

    [Fact]
    public void ConfigBlob_ShifterRecord_HPattern_Layout()
    {
        // Firmware layout (config_transaction.h): slot 0, pinA/B/C =
        // gear pins 1..3, params[0] = mode, params[1..3] = gear pins 4..6.
        var rec = ConfigBlob.ShifterRecord(ShifterGateMode.HPattern,
            ConfigBlob.DefaultHpatternGearPins);
        Assert.Equal(11, rec.Type);
        Assert.Equal(0, rec.Slot);
        Assert.Equal(13, rec.PinA);
        Assert.Equal(14, rec.PinB);
        Assert.Equal(16, rec.PinC);
        Assert.NotNull(rec.Params);
        Assert.Equal(1, rec.Params![0]);
        Assert.Equal(17, rec.Params[1]);
        Assert.Equal(18, rec.Params[2]);
        Assert.Equal(19, rec.Params[3]);
        Assert.Equal(0, rec.Params[4]);

        var blob = ConfigBlob.Build([rec], cfgVersion: 2);
        Assert.True(ConfigBlob.TryValidateStructure(blob, out var error), error);
        Assert.Equal(11, blob[8]); // pehle record ka type
        Assert.Equal(1, blob[8 + 6]); // params[0] = mode

        // Custom pins bhi waisi hi layout mein jate hain.
        var custom = ConfigBlob.ShifterRecord(ShifterGateMode.HPattern,
            [21, 22, 23, 24, 25, 26]);
        Assert.Equal(21, custom.PinA);
        Assert.Equal(24, custom.Params![1]);
    }

    [Fact]
    public void ConfigBlob_ShifterRecord_Sequential_NoPinsNeeded()
    {
        var rec = ConfigBlob.ShifterRecord(ShifterGateMode.Sequential);
        Assert.Equal(11, rec.Type);
        Assert.Equal(0, rec.Params![0]);
        Assert.Equal(0xFF, rec.PinA);
        Assert.Equal(0xFF, rec.PinB);
        Assert.Equal(0xFF, rec.PinC);
        var blob = ConfigBlob.Build([rec], cfgVersion: 2);
        Assert.True(ConfigBlob.TryValidateStructure(blob, out var error), error);
    }

    [Fact]
    public void ConfigBlob_ShifterRecord_InvalidArgs_Rejected()
    {
        Assert.Throws<ArgumentException>(() =>
            ConfigBlob.ShifterRecord((ShifterGateMode)99));
        Assert.Throws<ArgumentException>(() =>
            ConfigBlob.ShifterRecord(ShifterGateMode.HPattern, [13, 14, 16]));
        Assert.Throws<ArgumentException>(() =>
            ConfigBlob.ShifterRecord(ShifterGateMode.HPattern, []));
    }
}
