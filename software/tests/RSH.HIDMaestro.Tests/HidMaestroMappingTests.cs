using HIDMaestro;
using RSH.Core.Output;
using RSH.Core.Processing;
using RSH.Output.HIDMaestro;

namespace RSH.HIDMaestro.Tests;

/// <summary>
/// SubmitState rasta (HMGamepadState): axes floats, buttons 1..25,
/// neutral, caps gating, reuse dictionary. Driver ke baghair chalta hai.
/// </summary>
public class HidMaestroMappingTests
{
    private static ProcessedState State(
        int steer = 16384, int accel = 1, int brake = 1,
        uint buttons = 0, ulong buttonsEx = 0, int clutch = 1, int handbrake = 1,
        ulong pulses = 0) => new(
            SteeringAxis: steer, AccelAxis: accel, BrakeAxis: brake,
            ButtonsMask: buttons, SteeringNorm: 0, AccelNorm: 0, BrakeNorm: 0,
            ButtonsMaskEx: buttonsEx, ClutchAxis: clutch, HandbrakeAxis: handbrake,
            EncoderPulseMask: pulses);

    private static float Unit(int axis) => (axis - 1) / 32767f;

    /// <summary>Signed axis (steering): 0.5 + (v-16384)/32768 — center exact 0.5.</summary>
    private static float UnitSigned(int axis) => 0.5f + (axis - 16384) / 32768f;

    [Fact]
    public void Steering_MapsToX_SignedCenterHalf()
    {
        var s = HidMaestroMapping.BuildGamepadState(State(steer: 16384), HidMaestroCapabilities.Unknown);
        Assert.Equal(0.5f, s.Axes![HMAxis.X], precision: 5);
        var left = HidMaestroMapping.BuildGamepadState(State(steer: 1), HidMaestroCapabilities.Unknown);
        Assert.InRange(left.Axes![HMAxis.X], 0f, 0.001f); // full-left ≈ 0 (1 LSB oopar — B-H1, dekho mapping doc)
        var right = HidMaestroMapping.BuildGamepadState(State(steer: 32768), HidMaestroCapabilities.Unknown);
        Assert.Equal(1f, right.Axes![HMAxis.X], precision: 5);
    }

    [Fact]
    public void Pedals_MapToY_Z_UnsignedReleasedZero()
    {
        var s = HidMaestroMapping.BuildGamepadState(State(accel: 20000, brake: 30000), HidMaestroCapabilities.Unknown);
        Assert.Equal(Unit(20000), s.Axes![HMAxis.Y], precision: 5);
        Assert.Equal(Unit(30000), s.Axes![HMAxis.Z], precision: 5);
        var released = HidMaestroMapping.BuildGamepadState(State(), HidMaestroCapabilities.Unknown);
        Assert.Equal(0f, released.Axes![HMAxis.Y], precision: 5);
        Assert.Equal(0f, released.Axes![HMAxis.Z], precision: 5);
    }

    [Fact]
    public void Clutch_MapsToRx_Handbrake_MapsToSlider()
    {
        var s = HidMaestroMapping.BuildGamepadState(State(clutch: 32768, handbrake: 16384), HidMaestroCapabilities.Unknown);
        Assert.Equal(1f, s.Axes![HMAxis.Rx], precision: 5);
        Assert.Equal(Unit(16384), s.Axes![HMAxis.Slider], precision: 5);
    }

    [Fact]
    public void Neutral_MatchesExpectedFloats()
    {
        var s = HidMaestroMapping.NeutralGamepadState(HidMaestroCapabilities.Unknown);
        Assert.Equal(0.5f, s.Axes![HMAxis.X], precision: 5);
        Assert.Equal(0f, s.Axes![HMAxis.Y], precision: 5);
        Assert.Equal(0f, s.Axes![HMAxis.Z], precision: 5);
        Assert.Equal(0f, s.Axes![HMAxis.Rx], precision: 5);
        Assert.Equal(0f, s.Axes![HMAxis.Slider], precision: 5);
        Assert.Equal(HMButton.None, s.Buttons);
        Assert.Equal(HMHat.None, s.Hat);
    }

    [Theory]
    [InlineData(1, 1u)]     // wheel button 1 → HMButton bit 0 (A)
    [InlineData(2, 2u)]
    [InlineData(8, 128u)]
    [InlineData(25, 1u << 24)] // aakhri HMButton bit
    public void Buttons1To25_MapToHMButtonBits(int button, uint expectedBits)
    {
        var s = HidMaestroMapping.BuildGamepadState(State(buttons: 1u << (button - 1)), HidMaestroCapabilities.Unknown);
        Assert.Equal((HMButton)expectedBits, s.Buttons);
    }

    [Fact]
    public void Buttons1To25_AllSetTogether()
    {
        var s = HidMaestroMapping.BuildGamepadState(State(buttons: 0x01FF_FFFF), HidMaestroCapabilities.Unknown);
        Assert.Equal((HMButton)0x01FF_FFFFu, s.Buttons);
    }

    [Fact]
    public void Buttons26Plus_DroppedOnSubmitStatePath_DocumentedGap()
    {
        // GAP (gate point 2): HMButton sirf 25 bits hai — wheel buttons
        // 26..32 is raste se nahi ja sakte. Ye test is farq ko pakka
        // karta hai (khamoshi se ghalat map nahi hota — drop hota hai).
        var s = HidMaestroMapping.BuildGamepadState(State(buttons: 0xFFFF_FFFF), HidMaestroCapabilities.Unknown);
        Assert.Equal((HMButton)0x01FF_FFFFu, s.Buttons);
    }

    [Fact]
    public void ButtonsEx_AndEncoderPulses_DroppedOnSubmitStatePath()
    {
        // GAP (gate point 2): ButtonsMaskEx (33..64) aur encoder pulses
        // (65..72) HMButton mein nahi aate — sirf raw-report raste mein.
        var s = HidMaestroMapping.BuildGamepadState(
            State(buttonsEx: 0xFFFF_FFFF_FFFF_FFFF, pulses: 0xFF), HidMaestroCapabilities.Unknown);
        Assert.Equal(HMButton.None, s.Buttons);
    }

    [Fact]
    public void CapsGating_MissingAxis_NotInDict()
    {
        var caps = new HidMaestroCapabilities(HasAxisX: true, HasAxisY: false, HasAxisZ: false, HasAxisRx: false, HasAxisSlider: false, ButtonCount: 25);
        var s = HidMaestroMapping.BuildGamepadState(State(steer: 1000, accel: 20000), caps);
        Assert.True(s.Axes!.ContainsKey(HMAxis.X));
        Assert.False(s.Axes!.ContainsKey(HMAxis.Y));
        Assert.False(s.Axes!.ContainsKey(HMAxis.Z));
        Assert.False(s.Axes!.ContainsKey(HMAxis.Rx));
        Assert.False(s.Axes!.ContainsKey(HMAxis.Slider));
    }

    [Fact]
    public void AxesOutOfRange_Clamped_NotWrapped()
    {
        var s = HidMaestroMapping.BuildGamepadState(State(steer: 99999, accel: -5), HidMaestroCapabilities.Unknown);
        Assert.Equal(1f, s.Axes![HMAxis.X], precision: 5);
        Assert.Equal(0f, s.Axes![HMAxis.Y], precision: 5);
    }

    [Fact]
    public void ReuseDictionary_SameInstance_NoGrowth()
    {
        var dict = new Dictionary<HMAxis, float>();
        var s1 = HidMaestroMapping.BuildGamepadState(State(steer: 1000), HidMaestroCapabilities.Unknown, dict);
        var s2 = HidMaestroMapping.BuildGamepadState(State(steer: 30000), HidMaestroCapabilities.Unknown, dict);
        Assert.Same(dict, s1.Axes);
        Assert.Same(dict, s2.Axes);
        Assert.Equal(5, dict.Count);
        Assert.Equal(UnitSigned(30000), dict[HMAxis.X], precision: 5);
    }
}
