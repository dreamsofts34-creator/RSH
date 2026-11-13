using RSH.Core.Processing;
using Xunit;

namespace RSH.Tests;

/// <summary>AutoCalibrator — wizard ki auto-calibration ki buniyad.</summary>
public class AutoCalibratorTests
{
    private static void Feed(AutoCalibrator c, Action<ushort> add, int center, int noise, int count, int seed = 42)
    {
        var rng = new Random(seed);
        for (int i = 0; i < count; i++)
            add((ushort)(center + rng.Next(-noise, noise + 1)));
    }

    [Fact]
    public void Center_NoSamples_ReturnsFalse()
    {
        // T14 (C# side): TrimmedMean khaali list par kabhi nahi chalta —
        // TryGetCenter 20 samples se pehle hi false deta hai.
        var c = new AutoCalibrator();
        Assert.False(c.TryGetCenter(out _));
    }

    [Fact]
    public void Center_NeedsEnoughSamples()
    {
        var c = new AutoCalibrator();
        Feed(c, c.AddCenterSample, 2050, 3, 10);
        Assert.False(c.TryGetCenter(out _));
        Feed(c, c.AddCenterSample, 2050, 3, 30);
        Assert.True(c.TryGetCenter(out var center));
        Assert.InRange(center, (ushort)2045, (ushort)2055);
    }

    [Fact]
    public void SteeringRange_SweepWithSpikes_EndpointsSurvive()
    {
        var c = new AutoCalibrator();
        // Lock-to-lock sweep 1000..3100 + do ADC spikes (0 aur 4095)
        for (int v = 1000; v <= 3100; v += 10) c.AddSteeringRangeSample((ushort)v);
        for (int v = 3100; v >= 1000; v -= 10) c.AddSteeringRangeSample((ushort)v);
        c.AddSteeringRangeSample(0);
        c.AddSteeringRangeSample(4095);
        Assert.True(c.TryGetSteeringRange(out var min, out var max));
        Assert.InRange(min, (ushort)990, (ushort)1060);
        Assert.InRange(max, (ushort)3040, (ushort)3110);
    }

    [Fact]
    public void SteeringRange_TooLittleMovement_Fails()
    {
        var c = new AutoCalibrator();
        Feed(c, c.AddSteeringRangeSample, 2050, 20, 200);
        Assert.False(c.TryGetSteeringRange(out _, out _));
    }

    [Fact]
    public void Pedal_PressAndRelease_RangeDetected()
    {
        var c = new AutoCalibrator();
        Feed(c, c.AddAccelSample, 300, 10, 100, seed: 1);          // resting
        for (int v = 300; v <= 3800; v += 50) c.AddAccelSample((ushort)v);
        Feed(c, c.AddAccelSample, 3800, 15, 100, seed: 2);          // full press
        Assert.True(c.TryGetAccelRange(out var min, out var max));
        Assert.InRange(min, (ushort)250, (ushort)450);
        Assert.InRange(max, (ushort)3650, (ushort)3900);
    }

    [Fact]
    public void Pedal_NeverPressed_Fails()
    {
        var c = new AutoCalibrator();
        Feed(c, c.AddBrakeSample, 400, 8, 200);
        Assert.False(c.TryGetBrakeRange(out _, out _));
    }

    // ---- P10: bounded ring buffers ----

    [Fact]
    public void Bounded_LongRun_StaysBounded()
    {
        // 100,000 samples (500 s @ 200 Hz) — Count capacity par rukta
        // hai, memory bounded rehti hai; centre phir bhi sahi milta hai.
        var c = new AutoCalibrator();
        Feed(c, c.AddCenterSample, 2050, 3, 100_000, seed: 7);
        Assert.Equal(AutoCalibrator.MaxSamplesPerChannel, c.CenterSampleCount);
        Assert.True(c.TryGetCenter(out var center));
        Assert.InRange(center, (ushort)2045, (ushort)2055);
    }

    [Fact]
    public void Bounded_RingKeepsNewest_OldSamplesEvicted()
    {
        var c = new AutoCalibrator();
        for (int i = 0; i < AutoCalibrator.MaxSamplesPerChannel; i++)
            c.AddCenterSample(1000);
        for (int i = 0; i < AutoCalibrator.MaxSamplesPerChannel; i++)
            c.AddCenterSample(3000);
        // Purane 1000 wale sab bahar gir gaye — sirf 3000 bache hain.
        Assert.Equal(AutoCalibrator.MaxSamplesPerChannel, c.CenterSampleCount);
        Assert.True(c.TryGetCenter(out var center));
        Assert.Equal((ushort)3000, center);
    }

    [Fact]
    public void Bounded_AllChannels_Capped()
    {
        var c = new AutoCalibrator();
        for (int i = 0; i < 5000; i++)
        {
            c.AddSteeringRangeSample((ushort)(1000 + i % 2000));
            c.AddAccelSample((ushort)(300 + i % 3000));
            c.AddBrakeSample((ushort)(300 + i % 3000));
        }
        Assert.Equal(AutoCalibrator.MaxSamplesPerChannel, c.SteeringRangeSampleCount);
        Assert.Equal(AutoCalibrator.MaxSamplesPerChannel, c.AccelSampleCount);
        Assert.Equal(AutoCalibrator.MaxSamplesPerChannel, c.BrakeSampleCount);
        c.Reset();
        Assert.Equal(0, c.SteeringRangeSampleCount);
        Assert.Equal(0, c.AccelSampleCount);
        Assert.Equal(0, c.BrakeSampleCount);
    }

    // ---- P10: PercentileRange guard, n=50 ----

    [Fact]
    public void Percentile_N50_ExactRangeDetected()
    {
        // Bilkul 50 samples — kam se kam tadad — lock-to-lock ramp.
        var c = new AutoCalibrator();
        for (int i = 0; i < 50; i++)
            c.AddSteeringRangeSample((ushort)(1000 + i * 2100 / 49));
        Assert.True(c.TryGetSteeringRange(out var min, out var max));
        Assert.InRange(min, (ushort)990, (ushort)1100);
        Assert.InRange(max, (ushort)3000, (ushort)3110);
    }

    [Fact]
    public void Percentile_N49_NeedsMoreSamples()
    {
        var c = new AutoCalibrator();
        for (int i = 0; i < 49; i++)
            c.AddSteeringRangeSample((ushort)(1000 + i * 2100 / 48));
        Assert.False(c.TryGetSteeringRange(out _, out _));
    }

    [Fact]
    public void Percentile_N50_SingleSpike_DoesNotSetEndpoint()
    {
        // n=50 par 1% index 0 hota — guard ke baghair ek spike (0 ya
        // 4095) hi endpoint ban jata. Guard: endpoints hamesha andar
        // ke samples (lo >= 1, hi <= n-2) hote hain.
        var low = new AutoCalibrator();
        for (int i = 0; i < 49; i++)
            low.AddSteeringRangeSample((ushort)(1000 + i * 2100 / 48));
        low.AddSteeringRangeSample(0);   // spike
        Assert.True(low.TryGetSteeringRange(out var min, out _));
        Assert.InRange(min, (ushort)990, (ushort)1100);

        var high = new AutoCalibrator();
        for (int i = 0; i < 49; i++)
            high.AddSteeringRangeSample((ushort)(1000 + i * 2100 / 48));
        high.AddSteeringRangeSample(4095);   // spike
        Assert.True(high.TryGetSteeringRange(out _, out var max));
        Assert.InRange(max, (ushort)3000, (ushort)3110);
    }

    // ---- P9/D10: spans AutoCalibrator aur IsSane mein ek hain ----

    [Fact]
    public void MinSpans_MatchD10_660Millivolts()
    {
        Assert.Equal(660, AutoCalibrator.MinSteeringSpan);
        Assert.Equal(660, AutoCalibrator.MinPedalSpan);
        Assert.Equal(SteeringCalibration.MinSpanMillivolts, AutoCalibrator.MinSteeringSpan);
        Assert.Equal(PedalCalibration.MinSpanMillivolts, AutoCalibrator.MinPedalSpan);
    }

    [Fact]
    public void SteeringRange_SpanBelow660_FailsEvenWithManySamples()
    {
        // Pehle 200 kaafi tha — ab 660 se kam span wali sweep bhi
        // reject hoti hai (warna bani hui calibration IsSane se fail hoti).
        var c = new AutoCalibrator();
        for (int i = 0; i < 200; i++)
            c.AddSteeringRangeSample((ushort)(1800 + i % 500));
        Assert.False(c.TryGetSteeringRange(out _, out _));
    }
}
