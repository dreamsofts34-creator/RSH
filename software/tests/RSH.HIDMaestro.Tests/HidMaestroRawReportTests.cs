using RSH.Core.Output;
using RSH.Output.HIDMaestro;

namespace RSH.HIDMaestro.Tests;

/// <summary>
/// Raw-report rasta (SubmitRawReport, custom profile ke liye): 26-byte
/// layout, axes u16 LE, buttons 1..128 (Ex + encoder pulses shamil),
/// neutral, caps gating. Driver ke baghair chalta hai.
/// </summary>
public class HidMaestroRawReportTests
{
    private static ProcessedState State(
        int steer = 16384, int accel = 1, int brake = 1,
        uint buttons = 0, ulong buttonsEx = 0, int clutch = 1, int handbrake = 1,
        ulong pulses = 0) => new(
            SteeringAxis: steer, AccelAxis: accel, BrakeAxis: brake,
            ButtonsMask: buttons, SteeringNorm: 0, AccelNorm: 0, BrakeNorm: 0,
            ButtonsMaskEx: buttonsEx, ClutchAxis: clutch, HandbrakeAxis: handbrake,
            EncoderPulseMask: pulses);

    private static byte[] Build(ProcessedState state, HidMaestroCapabilities caps)
    {
        var buf = new byte[HidMaestroMapping.RawReportSize];
        HidMaestroMapping.BuildRawReport(state, caps, buf);
        return buf;
    }

    [Fact]
    public void ReportSize_Is26Bytes()
    {
        Assert.Equal(26, HidMaestroMapping.RawReportSize);
        var buf = new byte[26];
        HidMaestroMapping.BuildRawReport(ProcessedState.Neutral, HidMaestroCapabilities.Unknown, buf);
        Assert.Equal(26, buf.Length);
    }

    [Fact]
    public void ReportTooShort_Throws()
    {
        Assert.Throws<ArgumentException>(() =>
            HidMaestroMapping.BuildRawReport(ProcessedState.Neutral, HidMaestroCapabilities.Unknown, new byte[25]));
    }

    [Fact]
    public void Axes_AreU16LittleEndian_ValueMinusOne()
    {
        var r = Build(State(steer: 1000, accel: 20000, brake: 30000, clutch: 32768, handbrake: 16384), HidMaestroCapabilities.Unknown);
        Assert.Equal(999, HidMaestroMapping.ReadRawAxis(r, 0));    // X steering
        Assert.Equal(19999, HidMaestroMapping.ReadRawAxis(r, 1));  // Y accel
        Assert.Equal(29999, HidMaestroMapping.ReadRawAxis(r, 2));  // Z brake
        Assert.Equal(32767, HidMaestroMapping.ReadRawAxis(r, 3));  // Rx clutch
        Assert.Equal(16383, HidMaestroMapping.ReadRawAxis(r, 4));  // Slider handbrake
    }

    [Fact]
    public void NeutralReport_MatchesExpected()
    {
        var buf = new byte[26];
        HidMaestroMapping.BuildNeutralRawReport(HidMaestroCapabilities.Unknown, buf);
        Assert.Equal(16383, HidMaestroMapping.ReadRawAxis(buf, 0)); // steering center
        Assert.Equal(0, HidMaestroMapping.ReadRawAxis(buf, 1));
        Assert.Equal(0, HidMaestroMapping.ReadRawAxis(buf, 2));
        Assert.Equal(0, HidMaestroMapping.ReadRawAxis(buf, 3));
        Assert.Equal(0, HidMaestroMapping.ReadRawAxis(buf, 4));
        for (int b = 1; b <= 128; b++)
            Assert.False(HidMaestroMapping.IsRawButtonPressed(buf, b));
    }

    [Theory]
    [InlineData(1)]
    [InlineData(2)]
    [InlineData(32)]
    [InlineData(33)]
    [InlineData(64)]
    [InlineData(65)]
    [InlineData(96)]
    public void Buttons_SetInRawReport(int button)
    {
        ProcessedState s;
        if (button <= 32) s = State(buttons: 1u << (button - 1));
        else if (button <= 64) s = State(buttonsEx: 1ul << (button - 33));
        else s = State(buttonsEx: 1ul << (button - 33));
        var r = Build(s, HidMaestroCapabilities.Unknown);
        Assert.True(HidMaestroMapping.IsRawButtonPressed(r, button));
        Assert.False(HidMaestroMapping.IsRawButtonPressed(r, button == 1 ? 2 : 1));
    }

    [Fact]
    public void Buttons97To128_NoWireSource_AlwaysZero()
    {
        // vJoy Ex3 jaisa: wire max 64 buttons hai — 97..128 kabhi set nahi hote.
        var r = Build(State(buttons: 0xFFFF_FFFF, buttonsEx: 0xFFFF_FFFF_FFFF_FFFF), HidMaestroCapabilities.Unknown);
        for (int b = 97; b <= 128; b++)
            Assert.False(HidMaestroMapping.IsRawButtonPressed(r, b));
    }

    [Fact]
    public void EncoderPulses_OrIntoButtons65To96()
    {
        // EncoderPulseMask bit (n-65) = wheel button n (65..72).
        var r = Build(State(pulses: 0b101), HidMaestroCapabilities.Unknown);
        Assert.True(HidMaestroMapping.IsRawButtonPressed(r, 65));  // enc0 CW
        Assert.False(HidMaestroMapping.IsRawButtonPressed(r, 66));
        Assert.True(HidMaestroMapping.IsRawButtonPressed(r, 67));  // enc1 CW
    }

    [Fact]
    public void CapsGating_MissingAxis_StaysNeutral_ButtonMaskApplies()
    {
        var caps = new HidMaestroCapabilities(HasAxisX: false, HasAxisY: true, HasAxisZ: true, HasAxisRx: false, HasAxisSlider: false, ButtonCount: 2);
        var r = Build(State(steer: 1000, accel: 20000, buttons: 0xFFFF_FFFF), caps);
        Assert.Equal(16383, HidMaestroMapping.ReadRawAxis(r, 0)); // X neutral (center)
        Assert.Equal(19999, HidMaestroMapping.ReadRawAxis(r, 1));
        Assert.True(HidMaestroMapping.IsRawButtonPressed(r, 1));
        Assert.True(HidMaestroMapping.IsRawButtonPressed(r, 2));
        Assert.False(HidMaestroMapping.IsRawButtonPressed(r, 3)); // ButtonCount=2 se mask
    }

    [Fact]
    public void FullDeflection_EdgesExact()
    {
        var r = Build(State(steer: 32768, accel: 32768, brake: 32768), HidMaestroCapabilities.Unknown);
        Assert.Equal(32767, HidMaestroMapping.ReadRawAxis(r, 0));
        Assert.Equal(32767, HidMaestroMapping.ReadRawAxis(r, 1));
        Assert.Equal(32767, HidMaestroMapping.ReadRawAxis(r, 2));
        var q = Build(State(steer: 1, accel: 1, brake: 1), HidMaestroCapabilities.Unknown);
        Assert.Equal(0, HidMaestroMapping.ReadRawAxis(q, 0));
        Assert.Equal(0, HidMaestroMapping.ReadRawAxis(q, 1));
        Assert.Equal(0, HidMaestroMapping.ReadRawAxis(q, 2));
    }
}
