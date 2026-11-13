using RSH.Core.Processing;
using RSH.Core.Profiles;
using RSH.Core.Protocol;

namespace RSH.Tests;

public class ProcessingTests
{
    private static readonly SteeringCalibration Cal = new()
    { RawMin = 1000, RawCenter = 2050, RawMax = 3100, IsSet = true, IsMillivolts = true };

    [Fact]
    public void SteeringMapping_CenterMinMax()
    {
        Assert.Equal(0.0, InputMapper.MapSteering(2050, Cal, true, false), 6);
        Assert.Equal(-1.0, InputMapper.MapSteering(1000, Cal, true, false), 6);
        Assert.Equal(1.0, InputMapper.MapSteering(3100, Cal, true, false), 6);
        // Center ke dono taraf alag scale: aadha raasta left = -0.5
        Assert.Equal(-0.5, InputMapper.MapSteering(1525, Cal, true, false), 6);
        // Had se bahir clamp
        Assert.Equal(-1.0, InputMapper.MapSteering(200, Cal, true, false), 6);
        Assert.Equal(1.0, InputMapper.MapSteering(3900, Cal, true, false), 6);
        // Invert
        Assert.Equal(1.0, InputMapper.MapSteering(1000, Cal, true, true), 6);
    }

    [Fact]
    public void PedalMapping_AndDeadzone()
    {
        var cal = new PedalCalibration { RawMin = 300, RawMax = 2600, IsSet = true, IsMillivolts = true };
        // Ye qeematein mV mein hain (sane mV cal) — inMillivolts: true.
        Assert.Equal(0.0, InputMapper.MapPedal(300, cal, false, inMillivolts: true), 6);
        Assert.Equal(1.0, InputMapper.MapPedal(2600, cal, false, inMillivolts: true), 6);
        Assert.Equal(0.5, InputMapper.MapPedal(1450, cal, false, inMillivolts: true), 6);

        Assert.Equal(0.0, InputMapper.ApplyDeadzone(0.05, 0.1), 6);
        Assert.Equal(0.0, InputMapper.ApplyDeadzone(-0.1, 0.1), 6);
        Assert.Equal(1.0, InputMapper.ApplyDeadzone(1.0, 0.1), 6);   // kinara hamesha pura
        double mid = InputMapper.ApplyDeadzone(0.55, 0.1);
        Assert.Equal(0.5, mid, 6);                                     // rescale ke baad
    }

    [Fact]
    public void Curve_KinareSabit_CenterBadaltaHai()
    {
        foreach (double s in new[] { -1.0, -0.5, 0.0, 0.5, 1.0 })
        {
            Assert.Equal(0.0, InputMapper.ApplyCurve(0, s), 6);
            Assert.Equal(1.0, InputMapper.ApplyCurve(1, s), 6);
            Assert.Equal(-1.0, InputMapper.ApplyCurve(-1, s), 6);
        }
        // Soft (cubic taraf): center par kam response
        Assert.True(InputMapper.ApplyCurve(0.5, 1.0) < 0.5);
        // Aggressive (sqrt taraf): center par zyada response
        Assert.True(InputMapper.ApplyCurve(0.5, -1.0) > 0.5);
        // Linear: jaisa hai waisa
        Assert.Equal(0.5, InputMapper.ApplyCurve(0.5, 0.0), 6);
    }

    [Fact]
    public void AxisConversion_CenterBilkul16384()
    {
        Assert.Equal(16384, InputMapper.SteeringToAxis(0));
        Assert.Equal(32768, InputMapper.SteeringToAxis(1));
        Assert.Equal(1, InputMapper.SteeringToAxis(-1));
        Assert.Equal(1, InputMapper.PedalToAxis(0));
        Assert.Equal(32768, InputMapper.PedalToAxis(1));
        Assert.Equal(16384, InputMapper.CombinedPedalsToAxis(0.5, 0.5));   // dono barabar = center
        Assert.Equal(32768, InputMapper.CombinedPedalsToAxis(1, 0));
        Assert.Equal(1, InputMapper.CombinedPedalsToAxis(0, 1));
    }

    [Fact]
    public void OneEuro_ConstantInputParSabit_JitterKamKartaHai()
    {
        var filter = new OneEuroFilter(200, 5.0, 0.1);
        double last = 0;
        for (int i = 0; i < 100; i++) last = filter.Filter(0.5);
        Assert.Equal(0.5, last, 3);   // constant par wahi qeemat

        // Jitter: 0.5 ke gird ±0.05 ka shore — filter ke baad shore kam
        var jitter = new OneEuroFilter(200, 5.0, 0.0);
        double min = double.MaxValue, max = double.MinValue;
        for (int i = 0; i < 200; i++)
        {
            double input = 0.5 + (i % 2 == 0 ? 0.05 : -0.05);
            double output = jitter.Filter(input);
            if (i > 100) { min = Math.Min(min, output); max = Math.Max(max, output); }
        }
        Assert.True(max - min < 0.05, $"jitter range {max - min} kam hona chahiye tha");
    }

    [Fact]
    public void OneEuro_InvalidParameters_Rejected()
    {
        // T14: freq=0 C# mein crash nahi karta — double Infinity deta hai aur
        // filter degenerate ho jata hai. Ab ctor hi reject karta hai.
        Assert.Throws<ArgumentOutOfRangeException>(() => new OneEuroFilter(0, 5.0, 0.1));
        Assert.Throws<ArgumentOutOfRangeException>(() => new OneEuroFilter(-200, 5.0, 0.1));
        Assert.Throws<ArgumentOutOfRangeException>(() => new OneEuroFilter(double.NaN, 5.0, 0.1));
        Assert.Throws<ArgumentOutOfRangeException>(() => new OneEuroFilter(double.PositiveInfinity, 5.0, 0.1));
        Assert.Throws<ArgumentOutOfRangeException>(() => new OneEuroFilter(200, 0, 0.1));
        Assert.Throws<ArgumentOutOfRangeException>(() => new OneEuroFilter(200, -1, 0.1));
        Assert.Throws<ArgumentOutOfRangeException>(() => new OneEuroFilter(200, 5.0, -0.1));
        Assert.Throws<ArgumentOutOfRangeException>(() => new OneEuroFilter(200, 5.0, 0.1, 0));
        Assert.Throws<ArgumentOutOfRangeException>(() => new OneEuroFilter(200, 5.0, 0.1, double.NaN));
    }

    [Fact]
    public void OneEuro_NonFiniteDt_FiniteOutput()
    {
        // T14: NaN/Infinity dt pehle Math.Clamp se guzar kar filter ko NaN
        // kar deta tha. Ab nominal 5 ms mana jata hai — output finite rehta hai.
        var filter = new OneEuroFilter(200, 5.0, 0.1);
        Assert.Equal(0.5, filter.Filter(0.5, double.NaN), 6);   // pehla sample seedha guzarta hai
        double next = filter.Filter(0.6, double.PositiveInfinity);
        Assert.True(double.IsFinite(next), $"dt=Infinity par output finite hona chahiye, mila {next}");
        double next2 = filter.Filter(0.6, -1.0);
        Assert.True(double.IsFinite(next2), $"dt<0 par output finite hona chahiye, mila {next2}");
    }

    [Fact]
    public void PedalRamp_DabnaAurChornaWaqtPar()
    {
        var ramp = new PedalRamp(pressMs: 150, releaseMs: 100);
        // 150 ms tak dabaye rakho (5 ms ke 30 steps) — pura 1.0
        for (int i = 0; i < 30; i++) ramp.Update(true, false, 0.005);
        Assert.Equal(1.0, ramp.Accel, 6);
        Assert.Equal(0.0, ramp.Brake, 6);
        // Aadha waqt (75 ms) — aadha dabna
        var ramp2 = new PedalRamp(pressMs: 150, releaseMs: 100);
        for (int i = 0; i < 15; i++) ramp2.Update(true, false, 0.005);
        Assert.Equal(0.5, ramp2.Accel, 6);
        // Chorna: 100 ms mein 0
        for (int i = 0; i < 20; i++) ramp2.Update(false, false, 0.005);
        Assert.Equal(0.0, ramp2.Accel, 6);
    }

    // P1 (D7): steering scale DIVIDE hota hai. Cal 1000/2050/3100 par
    // raw 2470 → mapped +0.4; scale 0.4 (F1 360°) par 0.4 / 0.4 = 1.0
    // → full lock (axis 32768). Multiply hota to 0.16 → axis 19006 milta.
    [Fact]
    public void SteeringScale_Divide_Scale04_Steer04_FullLock()
    {
        var profile = WheelProfile.CreateDefault() with
        {
            SteeringScale = 0.4, Deadzone = 0, CurveStrength = 0, Smoothing = 0,
            SteeringCalibration = Cal,
        };
        var pipeline = new ProcessingPipeline(profile);

        var right = pipeline.Process(new WheelPacket(1, 0, 2470, 300, 300, 0, true), 0.005);
        Assert.Equal(1.0, right.SteeringNorm, 6);
        Assert.Equal(32768, right.SteeringAxis);

        // Ulti taraf bhi: raw 1630 → mapped -0.4 → -0.4 / 0.4 = -1.0
        var left = pipeline.Process(new WheelPacket(2, 0, 1630, 300, 300, 0, true), 0.005);
        Assert.Equal(-1.0, left.SteeringNorm, 6);
        Assert.Equal(1, left.SteeringAxis);
    }

    [Fact]
    public void SteeringScale_Divide_Scale10_Steer04_AadhaNahi()
    {
        // Scale 1.0 (Full 900°) par 0.4 waisa hi 0.4 rehta hai —
        // axis 16384 + round(0.4 × 16384) = 22938.
        var profile = WheelProfile.CreateDefault() with
        {
            SteeringScale = 1.0, Deadzone = 0, CurveStrength = 0, Smoothing = 0,
            SteeringCalibration = Cal,
        };
        var pipeline = new ProcessingPipeline(profile);
        var state = pipeline.Process(new WheelPacket(1, 0, 2470, 300, 300, 0, true), 0.005);
        Assert.Equal(0.4, state.SteeringNorm, 6);
        Assert.Equal(22938, state.SteeringAxis);
    }

    [Fact]
    public void SteeringScale_Divide_Scale06_Steer06_FullLock()
    {
        // GT 540° preset: raw 2680 → mapped +0.6; 0.6 / 0.6 = 1.0.
        var profile = WheelProfile.CreateDefault() with
        {
            SteeringScale = 0.6, Deadzone = 0, CurveStrength = 0, Smoothing = 0,
            SteeringCalibration = Cal,
        };
        var pipeline = new ProcessingPipeline(profile);
        var state = pipeline.Process(new WheelPacket(1, 0, 2680, 300, 300, 0, true), 0.005);
        Assert.Equal(1.0, state.SteeringNorm, 6);
        Assert.Equal(32768, state.SteeringAxis);
    }
}
