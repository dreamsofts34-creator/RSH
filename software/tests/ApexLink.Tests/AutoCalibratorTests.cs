using ApexLink.Core.Processing;
using Xunit;

namespace ApexLink.Tests;

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
}
