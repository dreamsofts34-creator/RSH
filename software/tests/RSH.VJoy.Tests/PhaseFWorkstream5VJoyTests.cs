using RSH.Core.Output;
using RSH.Core.Processing;
using RSH.Output.VJoy;

namespace RSH.VJoy.Tests;

/// <summary>
/// Phase F — Workstream 5 (Backend + firmware): encoder pulses → vJoy
/// buttons 65..72 (Phase F WS5).
///
/// Mapping (ProcessedState.EncoderPulseMask, bit (n-65) = vJoy button n):
/// encoder i CW (delta &gt; 0) → button 65+2i, CCW (delta &lt; 0) → 66+2i.
/// BuildPosition inhein LButtonsEx2 mein OR karta hai — ButtonsMaskEx
/// (wire buttons) se alag, taake Phase D ka "Ex2 wire source nahi"
/// contract na toote. Pulses caps.Ex2Mask se mask hote hain: 72 se kam
/// buttons wale device par pulses khamoshi se mask ho jati hain.
/// Ye tests baghair driver ke chalte hain (pure BuildPosition).
/// </summary>
public class PhaseFWorkstream5VJoyTests
{
    private static ProcessedState StateWithPulses(ulong pulseMask) => new(
        SteeringAxis: InputMapper.AxisCenter,
        AccelAxis: InputMapper.AxisMin,
        BrakeAxis: InputMapper.AxisMin,
        ButtonsMask: 0,
        SteeringNorm: 0,
        AccelNorm: 0,
        BrakeNorm: 0,
        EncoderPulseMask: pulseMask);

    [Fact]
    public void BuildPosition_EncoderPulses_GoToEx2()
    {
        // enc0 CW (button 65 → Ex2 bit 0) + enc1 CCW (button 68 → bit 3).
        var p = VJoyOutput.BuildPosition(
            StateWithPulses(0b1001), new VJoyCapabilities(true, true, true, 128), 1);
        Assert.Equal(0b1001, p.LButtonsEx2);
        Assert.Equal(0, p.LButtonsEx1);
        Assert.Equal(0, p.LButtonsEx3);
        Assert.Equal(0, p.LButtons);

        // Sab 4 encoders dono dishaein → bits 0..7 (buttons 65..72).
        var all = VJoyOutput.BuildPosition(
            StateWithPulses(0xFF), new VJoyCapabilities(true, true, true, 128), 1);
        Assert.Equal(0xFF, all.LButtonsEx2);
    }

    [Fact]
    public void BuildPosition_EncoderPulses_OrWithWireEx2()
    {
        // Wire Ex2 bits (ButtonsMaskEx oopar 32) + pulses dono milte hain.
        var state = new ProcessedState(
            SteeringAxis: InputMapper.AxisCenter,
            AccelAxis: InputMapper.AxisMin,
            BrakeAxis: InputMapper.AxisMin,
            ButtonsMask: 0,
            SteeringNorm: 0,
            AccelNorm: 0,
            BrakeNorm: 0,
            ButtonsMaskEx: 0x10ul << 32, // vJoy 69 (wire) → Ex2 bit 4
            EncoderPulseMask: 0b0001);    // vJoy 65 (enc0 CW) → Ex2 bit 0
        var p = VJoyOutput.BuildPosition(
            state, new VJoyCapabilities(true, true, true, 128), 1);
        Assert.Equal(0b10001, p.LButtonsEx2);
    }

    [Fact]
    public void BuildPosition_EncoderPulses_MaskedToButtonCount()
    {
        // 70-button device: sirf buttons 65..70 (Ex2 bits 0..5) guzarte hain.
        var p = VJoyOutput.BuildPosition(
            StateWithPulses(0xFF), new VJoyCapabilities(true, true, true, 70), 1);
        Assert.Equal(0x3F, p.LButtonsEx2);

        // 64 ya kam buttons: Ex2 poora mask — pulses kahin nahi jati.
        var q = VJoyOutput.BuildPosition(
            StateWithPulses(0xFF), new VJoyCapabilities(true, true, true, 64), 1);
        Assert.Equal(0, q.LButtonsEx2);
        var r = VJoyOutput.BuildPosition(
            StateWithPulses(0xFF), new VJoyCapabilities(true, true, true, 32), 1);
        Assert.Equal(0, r.LButtonsEx2);
    }

    [Fact]
    public void BuildPosition_NoPulses_Ex2Unchanged()
    {
        var p = VJoyOutput.BuildPosition(
            StateWithPulses(0), new VJoyCapabilities(true, true, true, 128), 1);
        Assert.Equal(0, p.LButtonsEx2);
        Assert.Equal(0, p.LButtonsEx3);
    }
}
