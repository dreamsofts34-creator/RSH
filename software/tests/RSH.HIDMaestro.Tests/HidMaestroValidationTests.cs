using RSH.Core.Output;
using RSH.Core.Pipeline;
using RSH.Output.HIDMaestro;

namespace RSH.HIDMaestro.Tests;

/// <summary>
/// P12 jaisi capability validation (HIDMaestro caps ke khilaf) +
/// OutputBackendSelector (fail-closed vJoy default).
/// </summary>
public class HidMaestroValidationTests
{
    private static PipelinePlan PlanWith(ModuleKind module, bool enabled) =>
        new PipelinePlanBuilder().WithEnabled(module, enabled).Build();

    private static PipelinePlan AllOn() => new PipelinePlanBuilder().Build();

    private static HidMaestroCapabilities Caps(
        bool x = true, bool y = true, bool z = true, bool rx = true, bool slider = true, int buttons = 128) =>
        new(x, y, z, rx, slider, buttons);

    [Fact]
    public void ValidateCapabilities_AllAxes_AllButtons_IsValid()
    {
        Assert.Empty(HidMaestroOutput.ValidateCapabilities(AllOn(), Caps()));
    }

    [Fact]
    public void ValidateCapabilities_SteeringOnly_WithoutOtherAxes_IsValid()
    {
        // Steering-only wheel valid hai (sirf X chahiye) — vJoy P12 jaisa.
        var plan = new PipelinePlanBuilder()
            .WithEnabled(ModuleKind.Accelerator, false)
            .WithEnabled(ModuleKind.Brake, false)
            .WithEnabled(ModuleKind.Clutch, false)
            .WithEnabled(ModuleKind.Handbrake, false)
            .WithEnabled(ModuleKind.Buttons, false)
            .Build();
        var caps = Caps(x: true, y: false, z: false, rx: false, slider: false, buttons: 0);
        Assert.Empty(HidMaestroOutput.ValidateCapabilities(plan, caps));
    }

    [Fact]
    public void ValidateCapabilities_SteeringEnabled_WithoutX_IsInvalid()
    {
        var errors = HidMaestroOutput.ValidateCapabilities(PlanWith(ModuleKind.Steering, true), Caps(x: false));
        Assert.Single(errors);
        Assert.Contains("X axis", errors[0]);
    }

    [Fact]
    public void ValidateCapabilities_HandbrakeEnabled_WithoutSlider_IsInvalid()
    {
        var errors = HidMaestroOutput.ValidateCapabilities(PlanWith(ModuleKind.Handbrake, true), Caps(slider: false));
        Assert.Single(errors);
        Assert.Contains("Slider", errors[0]);
    }

    [Fact]
    public void ValidateCapabilities_HandbrakeDisabled_WithoutSlider_IsValid()
    {
        Assert.Empty(HidMaestroOutput.ValidateCapabilities(PlanWith(ModuleKind.Handbrake, false), Caps(slider: false)));
    }

    [Fact]
    public void ValidateCapabilities_ButtonsEnabled_NoButtons_IsInvalid()
    {
        var errors = HidMaestroOutput.ValidateCapabilities(PlanWith(ModuleKind.Buttons, true), Caps(buttons: 0));
        Assert.Single(errors);
    }

    [Fact]
    public void ValidateCapabilities_ClutchEnabled_RequiresNoExtraAxis()
    {
        // Clutch ka koi caps flag nahi (Rx hamesha likha jata hai) — vJoy jaisa.
        Assert.Empty(HidMaestroOutput.ValidateCapabilities(
            PlanWith(ModuleKind.Clutch, true), Caps(rx: false)));
    }

    [Fact]
    public void ValidateCapabilities_ButtonCountOutOfRange_IsInvalid()
    {
        Assert.NotEmpty(HidMaestroOutput.ValidateCapabilities(AllOn(), Caps(buttons: 129)));
        Assert.NotEmpty(HidMaestroOutput.ValidateCapabilities(AllOn(), Caps(buttons: -1)));
    }

    // ------------------------------------------------------------------
    // OutputBackendSelector — fail-closed vJoy default
    // ------------------------------------------------------------------

    [Theory]
    [InlineData("VJoy", OutputBackendKind.VJoy)]
    [InlineData("HIDMaestro", OutputBackendKind.HidMaestro)]
    [InlineData("hidmaestro", OutputBackendKind.HidMaestro)]
    [InlineData("", OutputBackendKind.VJoy)]
    [InlineData(null, OutputBackendKind.VJoy)]
    [InlineData("bogus", OutputBackendKind.VJoy)]
    public void Selector_Parse_UnknownFallsBackToVJoy(string? value, OutputBackendKind expected)
    {
        Assert.Equal(expected, OutputBackendSelector.Parse(value));
    }

    [Fact]
    public void Selector_Resolve_RequiresBothRequestAndToggle()
    {
        Assert.Equal(OutputBackendKind.VJoy, OutputBackendSelector.Resolve(OutputBackendKind.HidMaestro, experimentalEnabled: false));
        Assert.Equal(OutputBackendKind.VJoy, OutputBackendSelector.Resolve(OutputBackendKind.VJoy, experimentalEnabled: true));
        Assert.Equal(OutputBackendKind.HidMaestro, OutputBackendSelector.Resolve(OutputBackendKind.HidMaestro, experimentalEnabled: true));
    }

    [Fact]
    public void Selector_Create_CallsOnlyChosenFactory()
    {
        int vjoyCalls = 0, hmCalls = 0;
        var chosen = OutputBackendSelector.Create(
            OutputBackendKind.HidMaestro, experimentalEnabled: false,
            () => { vjoyCalls++; return new FakeJoystickOutput("vJoy"); },
            () => { hmCalls++; return new FakeJoystickOutput("HIDMaestro"); });
        Assert.Equal("vJoy", chosen.Name);
        Assert.Equal(1, vjoyCalls);
        Assert.Equal(0, hmCalls);

        var chosen2 = OutputBackendSelector.Create(
            OutputBackendKind.HidMaestro, experimentalEnabled: true,
            () => { vjoyCalls++; return new FakeJoystickOutput("vJoy"); },
            () => { hmCalls++; return new FakeJoystickOutput("HIDMaestro"); });
        Assert.Equal("HIDMaestro", chosen2.Name);
        Assert.Equal(1, hmCalls);
    }
}
