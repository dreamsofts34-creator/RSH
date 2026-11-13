using RSH.Core.Output;
using RSH.Core.Pipeline;
using RSH.Core.Processing;
using RSH.Output.VJoy;

namespace RSH.VJoy.Tests;

/// <summary>
/// Phase F — Workstream 6 (Testers/QA): clutch → vJoy WClutch mapping
/// (Phase F WS5) + handbrake → vJoy WSlider mapping (Phase G C8 —
/// pehle "koi vJoy axis nahi" tha, faisla ab Slider hai;
/// docs/HANDBRAKE-MAPPING.md).
///
/// Anchor: VJoyOutput.cs BuildPosition — p.WClutch = state.ClutchAxis
/// (caps se aazad — field NeutralPosition mein bhi hamesha likha jata
/// hai; device mein axis na ho to driver ignore karta hai) aur
/// p.WSlider = state.HandbrakeAxis (caps-gated — HasAxisSlider).
/// Ex3 (97..128) ka koi source nahi — VJoyLayoutTests mein covered hai,
/// yahan dobara nahi.
/// Ye tests baghair driver ke chalte hain (pure BuildPosition).
/// </summary>
public class PhaseFWorkstream6VJoyTests
{
    private static ProcessedState StateWithClutch(int clutchAxis, int handbrakeAxis = 1) => new(
        SteeringAxis: InputMapper.AxisCenter,
        AccelAxis: InputMapper.AxisMin,
        BrakeAxis: InputMapper.AxisMin,
        ButtonsMask: 0,
        SteeringNorm: 0,
        AccelNorm: 0,
        BrakeNorm: 0,
        ClutchAxis: clutchAxis,
        HandbrakeAxis: handbrakeAxis);

    [Fact]
    public void BuildPosition_ClutchAxis_WritesWClutch()
    {
        var full = VJoyOutput.BuildPosition(
            StateWithClutch(32768), new VJoyCapabilities(true, true, true, 32), 1);
        Assert.Equal(32768, full.WClutch);
        var min = VJoyOutput.BuildPosition(
            StateWithClutch(InputMapper.AxisMin), new VJoyCapabilities(true, true, true, 32), 1);
        Assert.Equal(InputMapper.AxisMin, min.WClutch);
    }

    [Fact]
    public void BuildPosition_ClutchAxis_WritesWClutch_EvenWithoutXYZCaps()
    {
        // WClutch caps-gated nahi (X/Y/Z flags se aazad hai).
        var p = VJoyOutput.BuildPosition(
            StateWithClutch(20000), new VJoyCapabilities(false, false, false, 0), 1);
        Assert.Equal(20000, p.WClutch);
        Assert.Equal(InputMapper.AxisCenter, p.WAxisX);
    }

    [Fact]
    public void BuildPosition_HandbrakeAxis_WritesWSlider()
    {
        // Phase G C8 (faisla): handbrake → WSlider (analog). Purana test
        // "koi vJoy field nahi" is faisle se supersede ho gaya hai.
        var full = VJoyOutput.BuildPosition(
            StateWithClutch(InputMapper.AxisMin, handbrakeAxis: 32768),
            new VJoyCapabilities(true, true, true, 32, HasAxisSlider: true), 1);
        Assert.Equal(32768, full.WSlider);
        Assert.Equal(InputMapper.AxisMin, full.WClutch);
        // Slider caps na ho to handbrake neutral rehta hai (caps gating).
        var noSlider = VJoyOutput.BuildPosition(
            StateWithClutch(InputMapper.AxisMin, handbrakeAxis: 32768),
            new VJoyCapabilities(true, true, true, 32), 1);
        Assert.Equal(InputMapper.AxisMin, noSlider.WSlider);
        // Baqi axes par handbrake ka koi asar nahi.
        Assert.Equal(InputMapper.AxisCenter, full.WAxisX);
        Assert.Equal(InputMapper.AxisMin, full.WAxisY);
        Assert.Equal(InputMapper.AxisMin, full.WAxisZ);
        Assert.Equal(InputMapper.AxisCenter, full.WAxisXRot);
        Assert.Equal(InputMapper.AxisCenter, full.WAxisYRot);
        Assert.Equal(InputMapper.AxisCenter, full.WAxisZRot);
    }

    [Fact]
    public void ValidateCapabilities_ClutchEnabled_RequiresNoExtraAxis()
    {
        // Clutch ka vJoy mapping hai (WClutch) lekin caps mein us ka koi
        // flag nahi — Clutch enabled plan X/Y/Z na hone par bhi valid hai.
        var plan = new PipelinePlanBuilder()
            .WithEnabled(ModuleKind.Steering, false)
            .WithEnabled(ModuleKind.Accelerator, false)
            .WithEnabled(ModuleKind.Brake, false)
            .WithEnabled(ModuleKind.Buttons, false)
            .WithEnabled(ModuleKind.Handbrake, false)
            .WithEnabled(ModuleKind.Clutch, true)
            .Build();
        var errors = VJoyOutput.ValidateCapabilities(plan, new VJoyCapabilities(false, false, false, 0));
        Assert.Empty(errors);
    }
}
