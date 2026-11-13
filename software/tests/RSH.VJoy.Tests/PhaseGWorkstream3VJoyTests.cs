using RSH.Core.Output;
using RSH.Core.Pipeline;
using RSH.Core.Processing;
using RSH.Core.Profiles;
using RSH.Core.Protocol;
using RSH.Output.VJoy;

namespace RSH.VJoy.Tests;

/// <summary>
/// Phase G — Workstream 3, C8: handbrake → vJoy WSlider (analog).
/// Faisla: docs/HANDBRAKE-MAPPING.md — vJoy V3 mein handbrake ka koi
/// dedicated axis nahi (sirf Steering/Accel/Brake/Clutch); Slider generic
/// 1-D axis hai (HID usage 0x36). Analog barkarar, caps-gated (D11),
/// neutral = AxisMin (chhora hua), pedals jaisa.
/// Ye tests baghair driver ke chalte hain (pure BuildPosition + validation).
/// </summary>
public class PhaseGWorkstream3VJoyTests
{
    private static ProcessedState StateWithHandbrake(int handbrakeAxis) => new(
        SteeringAxis: InputMapper.AxisCenter,
        AccelAxis: InputMapper.AxisMin,
        BrakeAxis: InputMapper.AxisMin,
        ButtonsMask: 0,
        SteeringNorm: 0,
        AccelNorm: 0,
        BrakeNorm: 0,
        HandbrakeAxis: handbrakeAxis);

    private static VJoyCapabilities SliderCaps(bool slider = true) =>
        new(true, true, true, 32, HasAxisSlider: slider);

    private static PipelinePlan PlanWithHandbrake(bool enabled) =>
        new PipelinePlanBuilder().WithEnabled(ModuleKind.Handbrake, enabled).Build();

    // ---- Mapping: BuildPosition ----

    [Fact]
    public void BuildPosition_HandbrakeFull_WritesWSliderMax()
    {
        var p = VJoyOutput.BuildPosition(
            StateWithHandbrake(32768), SliderCaps(), 1);
        Assert.Equal(32768, p.WSlider);
    }

    [Fact]
    public void BuildPosition_HandbrakeHalf_WritesWSliderHalf()
    {
        var p = VJoyOutput.BuildPosition(
            StateWithHandbrake(16385), SliderCaps(), 1);
        Assert.Equal(16385, p.WSlider);
    }

    [Fact]
    public void BuildPosition_HandbrakeReleased_WSliderIsMin()
    {
        var p = VJoyOutput.BuildPosition(
            StateWithHandbrake(InputMapper.AxisMin), SliderCaps(), 1);
        Assert.Equal(InputMapper.AxisMin, p.WSlider);
    }

    [Fact]
    public void BuildPosition_NoSliderCaps_HandbrakeStaysNeutral()
    {
        // Caps gating (P12 jaisa): device mein Slider na ho to handbrake
        // neutral rehta hai — kisi aur axis par nahi likha jata.
        var p = VJoyOutput.BuildPosition(
            StateWithHandbrake(32768), SliderCaps(slider: false), 1);
        Assert.Equal(InputMapper.AxisMin, p.WSlider);
        Assert.Equal(InputMapper.AxisCenter, p.WAxisX);
        Assert.Equal(InputMapper.AxisMin, p.WAxisY);
        Assert.Equal(InputMapper.AxisMin, p.WAxisZ);
    }

    [Fact]
    public void BuildPosition_UnknownCaps_HandbrakeWritten()
    {
        // Unknown = purana behaviour, koi gating nahi (Slider farz karo).
        var p = VJoyOutput.BuildPosition(
            StateWithHandbrake(32768), VJoyCapabilities.Unknown, 1);
        Assert.Equal(32768, p.WSlider);
    }

    [Fact]
    public void NeutralPosition_WSliderIsMin_NotCenter()
    {
        // Center aadha handbrake hota — neutral lazmi min hai.
        var neutral = VJoyOutput.NeutralPosition(1);
        Assert.Equal(InputMapper.AxisMin, neutral.WSlider);
        var p = VJoyOutput.BuildPosition(ProcessedState.Neutral, SliderCaps(), 1);
        Assert.Equal(InputMapper.AxisMin, p.WSlider);
    }

    [Fact]
    public void UsageSlider_IsHidSliderUsage()
    {
        // HID Generic Desktop: X 0x30 … Rz 0x35, Slider 0x36, Dial 0x37.
        Assert.Equal((ushort)0x36, VJoyOutput.UsageSlider);
    }

    // ---- Validation (D11) ----

    [Fact]
    public void ValidateCapabilities_HandbrakeEnabled_WithSlider_IsValid()
    {
        var errors = VJoyOutput.ValidateCapabilities(PlanWithHandbrake(true), SliderCaps());
        Assert.Empty(errors);
    }

    [Fact]
    public void ValidateCapabilities_HandbrakeEnabled_WithoutSlider_IsInvalid()
    {
        var errors = VJoyOutput.ValidateCapabilities(PlanWithHandbrake(true), SliderCaps(slider: false));
        Assert.Contains(errors, e => e.Contains("Handbrake") && e.Contains("Slider"));
    }

    [Fact]
    public void ValidateCapabilities_HandbrakeDisabled_WithoutSlider_IsValid()
    {
        // Band module ki koi shart nahi (D11) — Slider na ho to bhi valid.
        var errors = VJoyOutput.ValidateCapabilities(PlanWithHandbrake(false), SliderCaps(slider: false));
        Assert.Empty(errors);
    }

    [Fact]
    public void ValidateCapabilities_DefaultPlan_NeedsSlider()
    {
        // DefaultPlan mein Handbrake on hai — X/Y/Z-only caps kaafi nahi.
        var errors = VJoyOutput.ValidateCapabilities(
            PipelinePlan.DefaultPlan(), new VJoyCapabilities(true, true, true, 32));
        Assert.Contains(errors, e => e.Contains("Handbrake") && e.Contains("Slider"));
        var ok = VJoyOutput.ValidateCapabilities(
            PipelinePlan.DefaultPlan(), SliderCaps());
        Assert.Empty(ok);
    }

    // ---- End-to-end: firmware slot 4 → pipeline → WSlider ----

    private static WheelPacketV2 V2Packet(ushort[] axesMv)
    {
        byte mask = 0;
        for (int i = 0; i < axesMv.Length; i++) mask |= (byte)(1 << i);
        byte[] bytes = WheelPacketV2.Build(1, 1000, 0, mask, 0, axesMv, []);
        Assert.Equal(WheelPacketV2ParseResult.Ok, WheelPacketV2.TryParse(bytes, out var packet));
        return packet;
    }

    [Fact]
    public void EndToEnd_Slot4FullPress_ReachesWSlider()
    {
        var pipeline = new ProcessingPipeline(WheelProfile.CreateDefault("G-WS3"));
        // Slots: 0 steer center, 1 accel rest, 2 brake rest, 3 clutch rest, 4 handbrake full.
        var state = pipeline.ProcessV2(V2Packet([2050, 300, 300, 300, 3300]), 0.005);
        Assert.Equal(1.0, state.HandbrakeNorm, 6);
        Assert.Equal(32768, state.HandbrakeAxis);
        var p = VJoyOutput.BuildPosition(state, SliderCaps(), 1);
        Assert.Equal(32768, p.WSlider);
    }

    [Fact]
    public void EndToEnd_HandbrakeDisabled_StaysNeutralOnSlider()
    {
        var pipeline = new ProcessingPipeline(WheelProfile.CreateDefault("G-WS3"));
        pipeline.SetPlan(new PipelinePlanBuilder()
            .WithEnabled(ModuleKind.Handbrake, false).Build());
        var state = pipeline.ProcessV2(V2Packet([2050, 300, 300, 300, 3300]), 0.005);
        Assert.Equal(0.0, state.HandbrakeNorm, 6);
        var p = VJoyOutput.BuildPosition(state, SliderCaps(), 1);
        Assert.Equal(InputMapper.AxisMin, p.WSlider);
    }

    [Fact]
    public void EndToEnd_Slot4Absent_StaysNeutralOnSlider()
    {
        var pipeline = new ProcessingPipeline(WheelProfile.CreateDefault("G-WS3"));
        // Sirf slots 0..3 — handbrake slot 4 maujood nahi.
        var state = pipeline.ProcessV2(V2Packet([2050, 300, 300, 300]), 0.005);
        Assert.Equal(0.0, state.HandbrakeNorm, 6);
        var p = VJoyOutput.BuildPosition(state, SliderCaps(), 1);
        Assert.Equal(InputMapper.AxisMin, p.WSlider);
    }
}
