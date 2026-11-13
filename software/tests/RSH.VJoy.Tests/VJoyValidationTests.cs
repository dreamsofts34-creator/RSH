using RSH.Core.Output;
using RSH.Core.Pipeline;
using RSH.Core.Processing;
using RSH.Output.VJoy;

namespace RSH.VJoy.Tests;

/// <summary>
/// P12 (D11, SW-7): vJoy validation configured modules ke khilaf —
/// fixed X/Y/Z ya ButtonCount ≥ 2 ki shart nahi. Steering-only wheel
/// valid hai. Update mein NeutralPosition rebuild + caps gating.
/// Ye tests baghair driver ke chalte hain (pure validation + BuildPosition).
/// </summary>
public class VJoyValidationTests
{
    private static PipelinePlan PlanWith(params (ModuleKind module, bool enabled)[] states)
    {
        var builder = new PipelinePlanBuilder();
        foreach (var (module, enabled) in states)
            builder.WithEnabled(module, enabled);
        return builder.Build();
    }

    private static PipelinePlan SteeringOnly() => PlanWith(
        (ModuleKind.Steering, true),
        (ModuleKind.Accelerator, false),
        (ModuleKind.Brake, false),
        (ModuleKind.Clutch, false),
        (ModuleKind.Handbrake, false),
        (ModuleKind.Shifter, false),
        (ModuleKind.Horn, false),
        (ModuleKind.Buttons, false),
        (ModuleKind.Encoders, false),
        (ModuleKind.LED, false));

    [Fact]
    public void SteeringOnlyWheel_XAxisOnly_NoButtons_IsValid()
    {
        // D11 ka asal nuqta: fixed "X+Y+Z aur ≥2 buttons" ki shart
        // steering-only wheel ko ghalat reject karti thi.
        var errors = VJoyOutput.ValidateCapabilities(
            SteeringOnly(), new VJoyCapabilities(HasAxisX: true, HasAxisY: false, HasAxisZ: false, ButtonCount: 0));
        Assert.Empty(errors);
    }

    [Fact]
    public void SteeringOnlyWheel_NoXAxis_IsInvalid()
    {
        var errors = VJoyOutput.ValidateCapabilities(
            SteeringOnly(), new VJoyCapabilities(false, false, false, 0));
        Assert.Contains(errors, e => e.Contains("Steering") && e.Contains("X"));
    }

    [Fact]
    public void DefaultPlan_FullCaps_IsValid()
    {
        // Phase G C8: "full caps" mein ab Slider bhi hai (Handbrake on hai
        // DefaultPlan mein) — X/Y/Z-only caps ab full nahi.
        var errors = VJoyOutput.ValidateCapabilities(
            PipelinePlan.DefaultPlan(), new VJoyCapabilities(true, true, true, 32, HasAxisSlider: true));
        Assert.Empty(errors);
    }

    [Fact]
    public void DefaultPlan_MissingAxes_ReportsEachMissingAxis()
    {
        var errors = VJoyOutput.ValidateCapabilities(
            PipelinePlan.DefaultPlan(), new VJoyCapabilities(HasAxisX: true, HasAxisY: false, HasAxisZ: false, ButtonCount: 8));
        Assert.Contains(errors, e => e.Contains("Accelerator") && e.Contains("Y"));
        Assert.Contains(errors, e => e.Contains("Brake") && e.Contains("Z"));
        // Phase G C8: DefaultPlan mein Handbrake on hai, Slider nahi — ye bhi report ho.
        Assert.Contains(errors, e => e.Contains("Handbrake") && e.Contains("Slider"));
        Assert.DoesNotContain(errors, e => e.Contains("Steering"));
        Assert.DoesNotContain(errors, e => e.Contains("Buttons"));
    }

    [Fact]
    public void ButtonsDisabled_NoButtons_IsValid()
    {
        var plan = PlanWith(
            (ModuleKind.Steering, true),
            (ModuleKind.Accelerator, true),
            (ModuleKind.Brake, true),
            (ModuleKind.Handbrake, false),
            (ModuleKind.Buttons, false));
        var errors = VJoyOutput.ValidateCapabilities(plan, new VJoyCapabilities(true, true, true, 0));
        Assert.Empty(errors);
    }

    [Fact]
    public void ButtonsEnabled_NoButtons_IsInvalid()
    {
        var plan = PlanWith(
            (ModuleKind.Buttons, true),
            (ModuleKind.Handbrake, false));
        var errors = VJoyOutput.ValidateCapabilities(plan, new VJoyCapabilities(true, true, true, 0));
        Assert.Contains(errors, e => e.Contains("Buttons"));
    }

    [Fact]
    public void BrakeDisabled_NoZAxis_IsValid()
    {
        var plan = PlanWith(
            (ModuleKind.Steering, true),
            (ModuleKind.Accelerator, true),
            (ModuleKind.Brake, false),
            (ModuleKind.Handbrake, false),
            (ModuleKind.Buttons, false));
        var errors = VJoyOutput.ValidateCapabilities(plan, new VJoyCapabilities(true, true, false, 0));
        Assert.Empty(errors);
    }

    [Theory]
    [InlineData(-1)]
    [InlineData(129)]
    public void ButtonCount_OutOfRange_IsInvalid(int count)
    {
        var errors = VJoyOutput.ValidateCapabilities(
            PipelinePlan.DefaultPlan(), new VJoyCapabilities(true, true, true, count));
        Assert.Contains(errors, e => e.Contains("ButtonCount"));
    }

    [Fact]
    public void ButtonCount_One_WithButtonsEnabled_IsValid_NoFixedMinimumTwo()
    {
        // D11: ButtonCount ≥ 2 ki shart nahi — ek button bhi valid hai.
        var plan = PlanWith(
            (ModuleKind.Steering, true),
            (ModuleKind.Accelerator, false),
            (ModuleKind.Brake, false),
            (ModuleKind.Handbrake, false),
            (ModuleKind.Buttons, true));
        var errors = VJoyOutput.ValidateCapabilities(plan, new VJoyCapabilities(true, false, false, 1));
        Assert.Empty(errors);
    }

    // ---- Update: NeutralPosition rebuild + caps gating (BuildPosition) ----

    private static ProcessedState FullState() => new(
        SteeringAxis: 32768, AccelAxis: 32768, BrakeAxis: 32768,
        ButtonsMask: 0b1111, SteeringNorm: 1, AccelNorm: 1, BrakeNorm: 1);

    [Fact]
    public void BuildPosition_FullCaps_WritesAllAxesAndButtons()
    {
        var p = VJoyOutput.BuildPosition(FullState(), new VJoyCapabilities(true, true, true, 32), 1);
        Assert.Equal(32768, p.WAxisX);
        Assert.Equal(32768, p.WAxisY);
        Assert.Equal(32768, p.WAxisZ);
        Assert.Equal(32768, p.WSteering);
        Assert.Equal(32768, p.WAccelerator);
        Assert.Equal(32768, p.WBrake);
        Assert.Equal(0b1111, p.LButtons);
    }

    [Fact]
    public void BuildPosition_XOnlyCaps_OtherAxesStayNeutral()
    {
        var p = VJoyOutput.BuildPosition(FullState(), new VJoyCapabilities(true, false, false, 0), 1);
        Assert.Equal(32768, p.WAxisX);
        Assert.Equal(32768, p.WSteering);
        Assert.Equal(InputMapper.AxisMin, p.WAxisY);   // pedal neutral (min), likha nahi gaya
        Assert.Equal(InputMapper.AxisMin, p.WAxisZ);
        Assert.Equal(InputMapper.AxisMin, p.WAccelerator);
        Assert.Equal(InputMapper.AxisMin, p.WBrake);
        // Phase G C8: Slider caps nahi — handbrake (default min) neutral rehta hai.
        Assert.Equal(InputMapper.AxisMin, p.WSlider);
        Assert.Equal(0, p.LButtons);
    }

    [Fact]
    public void BuildPosition_ButtonsMaskedToButtonCount()
    {
        var p = VJoyOutput.BuildPosition(FullState(), new VJoyCapabilities(true, true, true, 2), 1);
        Assert.Equal(0b11, p.LButtons);   // 4 bits mein se sirf 2 buttons
        var none = VJoyOutput.BuildPosition(FullState(), new VJoyCapabilities(true, true, true, 0), 1);
        Assert.Equal(0, none.LButtons);
    }

    [Fact]
    public void BuildPosition_UnknownCaps_WritesEverything()
    {
        // Caps maloom na hon ("unknown") = purana behaviour, koi gating
        // nahi — T11 mapping tests isi par hain. Phase G C8: Unknown mein
        // Slider bhi farz karo — FullState ka HandbrakeAxis default min hai.
        var p = VJoyOutput.BuildPosition(FullState(), VJoyCapabilities.Unknown, 1);
        Assert.Equal(32768, p.WAxisX);
        Assert.Equal(32768, p.WAxisY);
        Assert.Equal(32768, p.WAxisZ);
        Assert.Equal(InputMapper.AxisMin, p.WSlider);
        Assert.Equal(0b1111, p.LButtons);
    }

    // ---- T11/Ex: ButtonsMaskEx → LButtonsEx1/2/3 ----

    private static ProcessedState ExState(ulong maskEx) => new(
        SteeringAxis: 16384, AccelAxis: 1, BrakeAxis: 1,
        ButtonsMask: 0, SteeringNorm: 0, AccelNorm: 0, BrakeNorm: 0,
        ButtonsMaskEx: maskEx);

    [Fact]
    public void BuildPosition_ExFields_FromButtonsMaskEx()
    {
        var p = VJoyOutput.BuildPosition(ExState(0xFFFF_FFFF_FFFF_FFFF), new VJoyCapabilities(true, true, true, 128), 1);
        Assert.Equal(-1, p.LButtonsEx1);   // buttons 33..64
        Assert.Equal(-1, p.LButtonsEx2);   // buttons 65..96
        Assert.Equal(0, p.LButtonsEx3);    // buttons 97..128 — koi source nahi
        Assert.Equal(0, p.LButtons);
    }

    [Fact]
    public void BuildPosition_ExFields_BtnsExWord_MapsToEx1()
    {
        // StatusPacketV2.BtnsEx bit 5 = 0-based button 37 = vJoy 38 → Ex1 bit 5.
        var p = VJoyOutput.BuildPosition(ExState(0x20), new VJoyCapabilities(true, true, true, 64), 1);
        Assert.Equal(0x20, p.LButtonsEx1);
        Assert.Equal(0, p.LButtonsEx2);
        Assert.Equal(0, p.LButtonsEx3);
    }

    [Fact]
    public void BuildPosition_ExFields_MaskedToButtonCount()
    {
        // 40 buttons: Ex1 mein sirf pehle 8 bits (33..40), Ex2 kuch nahi.
        var p = VJoyOutput.BuildPosition(ExState(0xFFFF_FFFF_FFFF_FFFF), new VJoyCapabilities(true, true, true, 40), 1);
        Assert.Equal(0xFF, p.LButtonsEx1);
        Assert.Equal(0, p.LButtonsEx2);
        Assert.Equal(0, p.LButtonsEx3);
        // 70 buttons: Ex1 poora, Ex2 mein pehle 6 bits (65..70).
        var q = VJoyOutput.BuildPosition(ExState(0xFFFF_FFFF_FFFF_FFFF), new VJoyCapabilities(true, true, true, 70), 1);
        Assert.Equal(-1, q.LButtonsEx1);
        Assert.Equal(0x3F, q.LButtonsEx2);
        Assert.Equal(0, q.LButtonsEx3);
    }

    [Fact]
    public void BuildPosition_UnknownCaps_ExFieldsFull()
    {
        var p = VJoyOutput.BuildPosition(ExState(0xFFFF_FFFF_FFFF_FFFF), VJoyCapabilities.Unknown, 1);
        Assert.Equal(-1, p.LButtonsEx1);
        Assert.Equal(-1, p.LButtonsEx2);
        Assert.Equal(0, p.LButtonsEx3);
    }

    [Fact]
    public void BuildPosition_NeutralState_IsNeutralPosition()
    {
        var p = VJoyOutput.BuildPosition(ProcessedState.Neutral, new VJoyCapabilities(true, true, true, 32), 1);
        var neutral = VJoyOutput.NeutralPosition(1);
        Assert.Equal(neutral.WAxisX, p.WAxisX);
        Assert.Equal(neutral.WAxisY, p.WAxisY);
        Assert.Equal(neutral.WAxisZ, p.WAxisZ);
        Assert.Equal(neutral.WSlider, p.WSlider);
        Assert.Equal(0, p.LButtons);
    }
}
