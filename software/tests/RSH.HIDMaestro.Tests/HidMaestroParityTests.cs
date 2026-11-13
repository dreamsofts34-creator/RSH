using System.Reflection;
using HIDMaestro;
using RSH.Core.Output;
using RSH.Output.HIDMaestro;
using RSH.Output.VJoy;

namespace RSH.HIDMaestro.Tests;

/// <summary>
/// Parity: wahi ProcessedState vJoy aur HIDMaestro dono ko do — values
/// milni chahiye (jahan dono raste represent kar sakte hain). VJoy ka
/// BuildPosition internal hai — repo convention ke mutabiq (VJoyLayoutTests
/// bhi reflection istemal karta hai) reflection se bulaya jata hai.
/// Gaps (buttons 26+, Ex, pulses — SubmitState rasta) alag tests mein
/// KNOWN GAP ke tor par darj hain, chhupaye nahi gaye.
/// </summary>
public class HidMaestroParityTests
{
    private static readonly MethodInfo BuildPositionMethod =
        typeof(VJoyOutput).GetMethod("BuildPosition", BindingFlags.NonPublic | BindingFlags.Static)!;

    private static JoystickPosition VJoyPosition(ProcessedState state)
    {
        object?[] args = [state, VJoyCapabilities.Unknown, 1u];
        var result = BuildPositionMethod.Invoke(null, args);
        return (JoystickPosition)result!;
    }

    private static ProcessedState State(
        int steer = 16384, int accel = 1, int brake = 1,
        uint buttons = 0, ulong buttonsEx = 0, int clutch = 1, int handbrake = 1,
        ulong pulses = 0) => new(
            SteeringAxis: steer, AccelAxis: accel, BrakeAxis: brake,
            ButtonsMask: buttons, SteeringNorm: 0, AccelNorm: 0, BrakeNorm: 0,
            ButtonsMaskEx: buttonsEx, ClutchAxis: clutch, HandbrakeAxis: handbrake,
            EncoderPulseMask: pulses);

    private static float Unit(int axis) => (axis - 1) / 32767f;

    /// <summary>Signed axis (steering): 0.5 + (v-16384)/32768 — center exact 0.5 (B-H1).</summary>
    private static float UnitSigned(int axis) => 0.5f + (axis - 16384) / 32768f;

    [Theory]
    [InlineData(1, 1, 1, 1, 1)]
    [InlineData(1000, 20000, 30000, 32768, 16384)]
    [InlineData(16384, 16384, 16384, 16384, 16384)]
    [InlineData(32768, 32768, 32768, 32768, 32768)]
    public void Axes_Parity_VJoyVsHidMaestroGamepad(int steer, int accel, int brake, int clutch, int handbrake)
    {
        var state = State(steer, accel, brake, clutch: clutch, handbrake: handbrake);
        var vjoy = VJoyPosition(state);
        var hm = HidMaestroMapping.BuildGamepadState(state, HidMaestroCapabilities.Unknown);

        Assert.Equal(UnitSigned(vjoy.WAxisX), hm.Axes![HMAxis.X], precision: 5);
        Assert.Equal(Unit(vjoy.WAxisY), hm.Axes![HMAxis.Y], precision: 5);
        Assert.Equal(Unit(vjoy.WAxisZ), hm.Axes![HMAxis.Z], precision: 5);
        Assert.Equal(Unit(vjoy.WClutch), hm.Axes![HMAxis.Rx], precision: 5);
        Assert.Equal(Unit(vjoy.WSlider), hm.Axes![HMAxis.Slider], precision: 5);
        // V3 driving axes bhi wahi values rakhte hain:
        Assert.Equal(vjoy.WAxisX, vjoy.WSteering);
        Assert.Equal(vjoy.WAxisY, vjoy.WAccelerator);
        Assert.Equal(vjoy.WAxisZ, vjoy.WBrake);
    }

    [Theory]
    [InlineData(1, 1, 1, 1, 1)]
    [InlineData(1000, 20000, 30000, 32768, 16384)]
    [InlineData(32768, 32768, 32768, 32768, 32768)]
    public void Axes_Parity_VJoyVsHidMaestroRawReport(int steer, int accel, int brake, int clutch, int handbrake)
    {
        var state = State(steer, accel, brake, clutch: clutch, handbrake: handbrake);
        var vjoy = VJoyPosition(state);
        var buf = new byte[HidMaestroMapping.RawReportSize];
        HidMaestroMapping.BuildRawReport(state, HidMaestroCapabilities.Unknown, buf);

        Assert.Equal(vjoy.WAxisX - 1, HidMaestroMapping.ReadRawAxis(buf, 0));
        Assert.Equal(vjoy.WAxisY - 1, HidMaestroMapping.ReadRawAxis(buf, 1));
        Assert.Equal(vjoy.WAxisZ - 1, HidMaestroMapping.ReadRawAxis(buf, 2));
        Assert.Equal(vjoy.WClutch - 1, HidMaestroMapping.ReadRawAxis(buf, 3));
        Assert.Equal(vjoy.WSlider - 1, HidMaestroMapping.ReadRawAxis(buf, 4));
    }

    [Theory]
    [InlineData(1)]
    [InlineData(8)]
    [InlineData(25)]
    public void Buttons1To25_Parity_BothBackends(int button)
    {
        var state = State(buttons: 1u << (button - 1));
        var vjoy = VJoyPosition(state);
        var hm = HidMaestroMapping.BuildGamepadState(state, HidMaestroCapabilities.Unknown);
        uint vjoyBit = (uint)vjoy.LButtons >> (button - 1) & 1u;
        uint hmBit = ((uint)hm.Buttons >> (button - 1)) & 1u;
        Assert.Equal(1u, vjoyBit);
        Assert.Equal(vjoyBit, hmBit);
    }

    [Fact]
    public void Buttons26To32_KnownGap_SubmitStateDrops_ButRawReportKeeps()
    {
        // KNOWN GAP (gate point 2): vJoy buttons 26..32 rakhta hai,
        // SubmitState rasta nahi — sirf raw-report rasta rakhta hai.
        var state = State(buttons: 0xFFFF_FFFF);
        var vjoy = VJoyPosition(state);
        var hm = HidMaestroMapping.BuildGamepadState(state, HidMaestroCapabilities.Unknown);
        var buf = new byte[HidMaestroMapping.RawReportSize];
        HidMaestroMapping.BuildRawReport(state, HidMaestroCapabilities.Unknown, buf);

        Assert.Equal(-1, vjoy.LButtons);                       // vJoy: tamam 32
        Assert.Equal((HMButton)0x01FF_FFFFu, hm.Buttons);      // gamepad: sirf 1..25
        for (int b = 26; b <= 32; b++)
            Assert.True(HidMaestroMapping.IsRawButtonPressed(buf, b)); // raw: poora
    }

    [Fact]
    public void ButtonsEx_AndPulses_KnownGap_SubmitStateDrops_ButRawReportKeeps()
    {
        var state = State(buttonsEx: 0xFFFF_FFFF_FFFF_FFFF, pulses: 0xFF);
        var vjoy = VJoyPosition(state);
        var hm = HidMaestroMapping.BuildGamepadState(state, HidMaestroCapabilities.Unknown);
        var buf = new byte[HidMaestroMapping.RawReportSize];
        HidMaestroMapping.BuildRawReport(state, HidMaestroCapabilities.Unknown, buf);

        Assert.Equal(-1, vjoy.LButtonsEx1);   // vJoy Ex1: 33..64
        Assert.Equal(-1, vjoy.LButtonsEx2);   // vJoy Ex2: 65..96 (pulses OR)
        Assert.Equal(HMButton.None, hm.Buttons); // gamepad: kuch nahi
        for (int b = 33; b <= 96; b++)
            Assert.True(HidMaestroMapping.IsRawButtonPressed(buf, b));
    }

    [Fact]
    public void Neutral_Parity_BothBackends()
    {
        var vjoy = VJoyPosition(ProcessedState.Neutral);
        var hm = HidMaestroMapping.NeutralGamepadState(HidMaestroCapabilities.Unknown);
        var buf = new byte[HidMaestroMapping.RawReportSize];
        HidMaestroMapping.BuildNeutralRawReport(HidMaestroCapabilities.Unknown, buf);

        Assert.Equal(UnitSigned(vjoy.WAxisX), hm.Axes![HMAxis.X], precision: 5);
        Assert.Equal(0f, hm.Axes![HMAxis.Y], precision: 5);
        Assert.Equal(vjoy.WAxisX - 1, HidMaestroMapping.ReadRawAxis(buf, 0));
        Assert.Equal(0, vjoy.LButtons);
        Assert.Equal(HMButton.None, hm.Buttons);
    }
}
