using RSH.Core.Processing;
using RSH.Core.Profiles;
using RSH.Core.Protocol;

namespace RSH.Tests;

/// <summary>P9 (D10) — calibration unit tag + span rule.</summary>
public class CalibrationTests
{
    // ---- Steering ----

    [Fact]
    public void Steering_MillivoltCalibration_IsSane()
    {
        var cal = new SteeringCalibration { RawMin = 1000, RawCenter = 2050, RawMax = 3100, IsSet = true, IsMillivolts = true };
        Assert.True(cal.IsSane);
        Assert.Equal(3300, SteeringCalibration.FullScaleMillivolts);
        Assert.Equal(660, SteeringCalibration.MinSpanMillivolts);   // 20% of 3300
    }

    [Fact]
    public void Steering_RawUnitCalibration_IsRejected()
    {
        // Wahi numbers, lekin unit tag nahi — raw-ADC calibration hai.
        var cal = new SteeringCalibration { RawMin = 1000, RawCenter = 2050, RawMax = 3100, IsSet = true, IsMillivolts = false };
        Assert.False(cal.IsSane);
    }

    [Fact]
    public void Steering_RawCalibration_NotAppliedToMillivoltPacket()
    {
        // mV packet (raw 1000) par raw-unit calibration lagti to -1.0
        // milta; reject hone par mV default range (900/2050/3200) lagti
        // hai: -(2050-1000)/(2050-900) = -0.913.
        var rawCal = new SteeringCalibration { RawMin = 1000, RawCenter = 2050, RawMax = 3100, IsSet = true, IsMillivolts = false };
        Assert.Equal(-0.913, InputMapper.MapSteering(1000, rawCal, inMillivolts: true, invert: false), 3);

        var mvCal = rawCal with { IsMillivolts = true };
        Assert.Equal(-1.0, InputMapper.MapSteering(1000, mvCal, inMillivolts: true, invert: false), 6);
    }

    [Fact]
    public void Steering_RawCalibration_NotAppliedThroughPipeline()
    {
        var profile = WheelProfile.CreateDefault() with
        {
            SteeringScale = 1.0, Deadzone = 0, CurveStrength = 0, Smoothing = 0,
            SteeringCalibration = new SteeringCalibration { RawMin = 1000, RawCenter = 2050, RawMax = 3100, IsSet = true, IsMillivolts = false },
        };
        var pipeline = new ProcessingPipeline(profile);
        var state = pipeline.Process(new WheelPacket(1, 0, 1000, 300, 300, 0, true), 0.005);
        Assert.Equal(-0.913, state.SteeringNorm, 3);   // default range, cal nahi lagi

        var mvProfile = profile with
        {
            SteeringCalibration = new SteeringCalibration { RawMin = 1000, RawCenter = 2050, RawMax = 3100, IsSet = true, IsMillivolts = true },
        };
        var mvState = new ProcessingPipeline(mvProfile).Process(new WheelPacket(1, 0, 1000, 300, 300, 0, true), 0.005);
        Assert.Equal(-1.0, mvState.SteeringNorm, 6);    // cal lagi
    }

    [Fact]
    public void Steering_SpanBoundary_660Passes_659Fails()
    {
        // Span bilkul 660: min 1390, center 1720, max 2050.
        var ok = new SteeringCalibration { RawMin = 1390, RawCenter = 1720, RawMax = 2050, IsSet = true, IsMillivolts = true };
        Assert.True(ok.IsSane);
        // Span 659: max 2049.
        var narrow = new SteeringCalibration { RawMin = 1390, RawCenter = 1720, RawMax = 2049, IsSet = true, IsMillivolts = true };
        Assert.False(narrow.IsSane);
        // Bohat tang span (pehle 100 kaafi tha) ab reject hota hai.
        var tiny = new SteeringCalibration { RawMin = 2000, RawCenter = 2050, RawMax = 2100, IsSet = true, IsMillivolts = true };
        Assert.False(tiny.IsSane);
    }

    [Fact]
    public void Steering_OutOfMillivoltRange_IsRejected()
    {
        // RawMax full-scale (3300) se oopar — mV nahi ho sakta.
        var high = new SteeringCalibration { RawMin = 1000, RawCenter = 2050, RawMax = 3301, IsSet = true, IsMillivolts = true };
        Assert.False(high.IsSane);
        var negative = new SteeringCalibration { RawMin = -1, RawCenter = 1650, RawMax = 3300, IsSet = true, IsMillivolts = true };
        Assert.False(negative.IsSane);
        // Kinara bilkul 3300 theek hai (span 660, center andar).
        var edge = new SteeringCalibration { RawMin = 2640, RawCenter = 2970, RawMax = 3300, IsSet = true, IsMillivolts = true };
        Assert.True(edge.IsSane);
    }

    [Fact]
    public void Steering_UnsetOrUnordered_IsRejected()
    {
        Assert.False(new SteeringCalibration { RawMin = 1000, RawCenter = 2050, RawMax = 3100, IsSet = false, IsMillivolts = true }.IsSane);
        Assert.False(new SteeringCalibration { RawMin = 3100, RawCenter = 2050, RawMax = 1000, IsSet = true, IsMillivolts = true }.IsSane);
        Assert.False(new SteeringCalibration { RawMin = 1000, RawCenter = 1000, RawMax = 3100, IsSet = true, IsMillivolts = true }.IsSane);
    }

    // ---- Pedal ----

    [Fact]
    public void Pedal_MillivoltCalibration_IsSane_RawUnitRejected()
    {
        var mv = new PedalCalibration { RawMin = 300, RawMax = 2600, IsSet = true, IsMillivolts = true };
        Assert.True(mv.IsSane);
        var raw = new PedalCalibration { RawMin = 300, RawMax = 2600, IsSet = true, IsMillivolts = false };
        Assert.False(raw.IsSane);
    }

    [Fact]
    public void Pedal_RawCalibration_NotApplied()
    {
        // Raw-unit cal reject: default 0..4095 lagta hai (300/4095).
        var rawCal = new PedalCalibration { RawMin = 300, RawMax = 2600, IsSet = true, IsMillivolts = false };
        Assert.Equal(0.073, InputMapper.MapPedal(300, rawCal, invert: false), 3);
        var mvCal = rawCal with { IsMillivolts = true };
        // mvCal par mV qeematein — inMillivolts: true (default false
        // raw ADC samajhta hai, V-C2).
        Assert.Equal(0.0, InputMapper.MapPedal(300, mvCal, invert: false, inMillivolts: true), 6);
        Assert.Equal(1.0, InputMapper.MapPedal(2600, mvCal, invert: false, inMillivolts: true), 6);
    }

    [Fact]
    public void Pedal_SpanBoundary_660Passes_659Fails()
    {
        Assert.True(new PedalCalibration { RawMin = 200, RawMax = 860, IsSet = true, IsMillivolts = true }.IsSane);
        Assert.False(new PedalCalibration { RawMin = 200, RawMax = 859, IsSet = true, IsMillivolts = true }.IsSane);
        Assert.False(new PedalCalibration { RawMin = 200, RawMax = 3400, IsSet = true, IsMillivolts = true }.IsSane);  // > 3300
    }

    // ---- V-C2: raw→mV normalization (P9 ne capture tod di thi) ----

    [Fact]
    public void RawToMillivolts_Conversions()
    {
        Assert.Equal(0, InputMapper.RawToMillivolts(0));
        Assert.Equal(3300, InputMapper.RawToMillivolts(4095));
        Assert.Equal(1650, InputMapper.RawToMillivolts(2048));   // 2048 * 3300 / 4095 = 1650.4
        Assert.Equal(825, InputMapper.RawToMillivolts(1024));    // 825.2
        Assert.Equal(242, InputMapper.RawToMillivolts(300));     // 241.8
        Assert.Equal(3300, InputMapper.RawToMillivolts(9999));   // clamp
        Assert.Equal(0, InputMapper.RawToMillivolts(-5));        // clamp
    }

    [Fact]
    public void Steering_RawPacket_InsaneCal_DefaultsMatchOldBehaviour()
    {
        // Insane cal (IsSet false) + raw packet: raw defaults 0/2048/4095
        // ka mV conversion 0/1650/3300 lagta hai — purana behaviour:
        // raw 2048 → 0 (jaisa pehle), raw 0 → -1, raw 4095 → 1.
        var insane = new SteeringCalibration { IsSet = false };
        Assert.Equal(0.0, InputMapper.MapSteering(2048, insane, inMillivolts: false, invert: false), 6);
        Assert.Equal(-1.0, InputMapper.MapSteering(0, insane, inMillivolts: false, invert: false), 6);
        Assert.Equal(1.0, InputMapper.MapSteering(4095, insane, inMillivolts: false, invert: false), 6);
        Assert.Equal(-0.5, InputMapper.MapSteering(1024, insane, inMillivolts: false, invert: false), 6);
        Assert.Equal(0.5, InputMapper.MapSteering(3071, insane, inMillivolts: false, invert: false), 6);
        // mV packet + insane cal: mV defaults 900/2050/3200 — mV 2050 → 0.
        Assert.Equal(0.0, InputMapper.MapSteering(2050, insane, inMillivolts: true, invert: false), 6);
    }

    [Fact]
    public void Steering_SaneMillivoltCal_AppliesToRawPacket()
    {
        // Sane mV cal 1000/2050/3100. Raw packet: 1000 mV ≈ raw 1241,
        // 2050 mV ≈ raw 2544, 3100 mV ≈ raw 3848.
        var mvCal = new SteeringCalibration { RawMin = 1000, RawCenter = 2050, RawMax = 3100, IsSet = true, IsMillivolts = true };
        Assert.Equal(-1.0, InputMapper.MapSteering(1241, mvCal, inMillivolts: false, invert: false), 6);
        Assert.Equal(0.0, InputMapper.MapSteering(2544, mvCal, inMillivolts: false, invert: false), 6);
        Assert.Equal(1.0, InputMapper.MapSteering(3848, mvCal, inMillivolts: false, invert: false), 6);
    }

    [Fact]
    public void Steering_SaneMillivoltCal_AppliesToRawPacketThroughPipeline()
    {
        var profile = WheelProfile.CreateDefault() with
        {
            SteeringScale = 1.0, Deadzone = 0, CurveStrength = 0, Smoothing = 0,
            SteeringCalibration = new SteeringCalibration { RawMin = 1000, RawCenter = 2050, RawMax = 3100, IsSet = true, IsMillivolts = true },
            AccelCalibration = new PedalCalibration { RawMin = 300, RawMax = 2600, IsSet = true, IsMillivolts = true },
            BrakeCalibration = new PedalCalibration { RawMin = 300, RawMax = 2600, IsSet = true, IsMillivolts = true },
        };
        var pipeline = new ProcessingPipeline(profile);
        // Raw packet (flag false): steering raw 1241 ≈ 1000 mV → full left;
        // pedals raw 372 ≈ 300 mV → 0, raw 3227 ≈ 2600 mV → 1.
        var left = pipeline.Process(new WheelPacket(1, 0, 1241, 372, 372, 0, false), 0.005);
        Assert.Equal(-1.0, left.SteeringNorm, 6);
        Assert.Equal(0.0, left.AccelNorm, 6);
        var right = pipeline.Process(new WheelPacket(2, 0, 3848, 3227, 3227, 0, false), 0.005);
        Assert.Equal(1.0, right.SteeringNorm, 6);
        Assert.Equal(1.0, right.AccelNorm, 6);
        Assert.Equal(1.0, right.BrakeNorm, 6);
    }

    [Fact]
    public void Pedal_RawToMillivoltMapping_InsaneCal()
    {
        // Insane cal + raw packet: defaults 0..3300 mV (purane 0..4095
        // raw ke barabar — ratio wahi: raw 300 → 242 mV → 0.073).
        var insane = new PedalCalibration { IsSet = false };
        Assert.Equal(0.0, InputMapper.MapPedal(0, insane, invert: false), 6);
        Assert.Equal(1.0, InputMapper.MapPedal(4095, insane, invert: false), 6);
        Assert.Equal(0.5, InputMapper.MapPedal(2048, insane, invert: false), 6);
        Assert.Equal(0.073, InputMapper.MapPedal(300, insane, invert: false), 3);
        // mV mode mein wahi qeematein seedha mV hain.
        Assert.Equal(0.5, InputMapper.MapPedal(1650, insane, invert: false, inMillivolts: true), 6);
    }

    [Fact]
    public void Pedal_SaneMillivoltCal_AppliesToRawPacket()
    {
        // Sane mV cal 300..2600. Raw packet: 300 mV ≈ raw 372 → 0,
        // 2600 mV ≈ raw 3227 → 1, 1450 mV ≈ raw 1800 → ~0.5.
        var mvCal = new PedalCalibration { RawMin = 300, RawMax = 2600, IsSet = true, IsMillivolts = true };
        Assert.Equal(0.0, InputMapper.MapPedal(372, mvCal, invert: false), 6);
        Assert.Equal(1.0, InputMapper.MapPedal(3227, mvCal, invert: false), 6);
        Assert.Equal(0.5, InputMapper.MapPedal(1800, mvCal, invert: false), 3);
        // mV mode mein seedha mV qeematein.
        Assert.Equal(0.5, InputMapper.MapPedal(1450, mvCal, invert: false, inMillivolts: true), 6);
    }
}
