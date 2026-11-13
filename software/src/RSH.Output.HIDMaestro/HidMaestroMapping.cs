using HIDMaestro;
using RSH.Core.Output;
using RSH.Core.Processing;

namespace RSH.Output.HIDMaestro;

/// <summary>
/// ProcessedState → HIDMaestro frame. Do raste hain (dono pure hain —
/// baghair driver ke test hote hain):
///
/// 1. <see cref="BuildGamepadState"/> — SDK ka documented abstract frame
///    (<c>HMGamepadState</c>, <c>HMController.SubmitState</c>). Axes
///    [0,1] floats hain (signed axis par 0.5 = center, unsigned par 0.0 =
///    released); SDK profile ke descriptor ke mutabiq encode karta hai,
///    aur jo axis profile mein nahi, use khamoshi se ignore karta hai —
///    is liye mapping caps se chhant ti hai. Buttons: <c>HMButton</c>
///    sirf 25 named bits hai — wheel buttons 1..25 aate hain, 26..128
///    is raste se NAHI ja sakte (parity gap — gate point 2 mein darj hai;
///    encoder pulses 65..72 bhi nahi). Hat: ProcessedState mein hat ka
///    koi source nahi — <c>HMHat.None</c>.
/// 2. <see cref="BuildRawReport"/> — 26-byte raw HID input report
///    (<c>HMController.SubmitRawReport</c>, data bytes only, Report ID
///    prefix nahi — SDK contract). Ye RSH ke custom wheel profile ke
///    liye hai (HidDescriptorBuilder se banega — Windows-PC follow-up,
///    gate H-HID-2): 5× u16 axes + 128 button bits, vJoy parity poori
///    (Ex + encoder pulses shamil). Custom profile abhi bandha nahi hai,
///    is liye ye rasta sirf tab chalta hai jab <c>useRawReport</c> ke
///    saath custom profile id di jaye — warna SubmitState rasta chalta hai.
///
/// Axis scale: ProcessedState axis 1..32768 → float 0..1 ya u16 0..32767
/// (value-1). Signed axis X (steering): 0.5 + (v-16384)/32768 — center
/// exact 0.5 (SDK: "0.5 is centered for signed axes"); unsigned axes
/// (pedals/slider): (v-1)/32767 — released 0.0 exact. Raw report mein
/// center 16383 (u16) hai — float center se farq sirf representation ka hai.
/// </summary>
public static class HidMaestroMapping
{
    /// <summary>Raw report ka size: 5 axes × 2 B + 128 buttons × 1 bit = 26 B.</summary>
    public const int RawReportSize = 26;

    /// <summary>SubmitState raste se kitne wheel buttons ja sakte hain (HMButton ke 25 bits).</summary>
    public const int SubmitStateMaxButton = 25;

    private const int AxisMin = InputMapper.AxisMin;       // 1
    private const int AxisMax = InputMapper.AxisMax;       // 32768
    private const int AxisSpan = AxisMax - AxisMin;        // 32767

    /// <summary>
    /// Unsigned axis (pedal/slider: 0.0 = released, 1.0 = full).
    /// (v-1)/32767 — dono ends exact hain.
    /// </summary>
    private static float ToUnit(int axis) =>
        (Math.Clamp(axis, AxisMin, AxisMax) - AxisMin) / (float)AxisSpan;

    /// <summary>
    /// Signed axis (steering: 0.5 = center — SDK doc ka contract).
    /// 0.5 + (v-16384)/32768 — center AUR full-right exact hain; full-left
    /// 1 LSB oopar hai (0.00003, na-mahsoos). Seedha (v-1)/32767 center
    /// ko 0.49998 par le jata — test ne ye bug pakda (B-H1).
    /// </summary>
    private static float ToUnitSigned(int axis) =>
        0.5f + (Math.Clamp(axis, AxisMin, AxisMax) - InputMapper.AxisCenter) / 32768f;

    private static ushort ToU16(int axis) =>
        (ushort)(Math.Clamp(axis, AxisMin, AxisMax) - AxisMin);

    /// <summary>
    /// ProcessedState → HMGamepadState (SubmitState rasta). <paramref name="axesReuse"/>
    /// diya jaye to wahi dictionary dobara bhari jati hai (200 Hz hot path
    /// par per-tick allocation nahi — SDK doc bhi yahi kehta hai); null par
    /// nayi banti hai. Caps mein na-mojood axis key hi nahi dalta.
    /// Buttons 1..25 → HMButton bits 0..24; baaqi buttons is raste mein
    /// nahi aate (dekho class doc).
    /// </summary>
    public static HMGamepadState BuildGamepadState(
        in ProcessedState state,
        HidMaestroCapabilities caps,
        Dictionary<HMAxis, float>? axesReuse = null)
    {
        var axes = axesReuse ?? new Dictionary<HMAxis, float>(5);
        axes.Clear();
        if (caps.HasAxisX) axes[HMAxis.X] = ToUnitSigned(state.SteeringAxis);
        if (caps.HasAxisY) axes[HMAxis.Y] = ToUnit(state.AccelAxis);
        if (caps.HasAxisZ) axes[HMAxis.Z] = ToUnit(state.BrakeAxis);
        if (caps.HasAxisRx) axes[HMAxis.Rx] = ToUnit(state.ClutchAxis);
        if (caps.HasAxisSlider) axes[HMAxis.Slider] = ToUnit(state.HandbrakeAxis);

        // Wheel buttons 1..25 → HMButton bits 0..24. ButtonsMaskEx
        // (33..64) aur encoder pulses (65..72) HMButton mein nahi aate.
        uint buttons = state.ButtonsMask & 0x01FF_FFFFu;
        return new HMGamepadState
        {
            Axes = axes,
            Buttons = (HMButton)buttons,
            Hat = HMHat.None,
        };
    }

    /// <summary>Watchdog/band karte waqt ki mehfooz halat (SubmitState rasta).</summary>
    public static HMGamepadState NeutralGamepadState(
        HidMaestroCapabilities caps,
        Dictionary<HMAxis, float>? axesReuse = null) =>
        BuildGamepadState(ProcessedState.Neutral, caps, axesReuse);

    /// <summary>
    /// ProcessedState → 26-byte raw HID input report (SubmitRawReport
    /// rasta — custom RSH wheel profile ke liye). Layout:
    /// [0..1] X steering, [2..3] Y accel, [4..5] Z brake, [6..7] Rx
    /// clutch, [8..9] Slider handbrake (sab u16 LE, value = axis-1);
    /// [10..13] buttons 1..32, [14..17] buttons 33..64, [18..21]
    /// buttons 65..96 (EncoderPulseMask OR — vJoy LButtonsEx2 jaisa),
    /// [22..25] buttons 97..128 = 0 (koi wire source nahi — vJoy Ex3 jaisa).
    /// Caps mein na-mojood axis neutral rehta hai; buttons caps ke
    /// ButtonCount se mask hote hain.
    /// </summary>
    public static void BuildRawReport(
        in ProcessedState state,
        HidMaestroCapabilities caps,
        Span<byte> destination)
    {
        if (destination.Length < RawReportSize)
            throw new ArgumentException($"Raw report ke liye {RawReportSize} bytes chahiye.", nameof(destination));
        destination.Clear();

        WriteAxis(destination, 0, caps.HasAxisX ? state.SteeringAxis : InputMapper.AxisCenter);
        WriteAxis(destination, 2, caps.HasAxisY ? state.AccelAxis : InputMapper.AxisMin);
        WriteAxis(destination, 4, caps.HasAxisZ ? state.BrakeAxis : InputMapper.AxisMin);
        WriteAxis(destination, 6, caps.HasAxisRx ? state.ClutchAxis : InputMapper.AxisMin);
        WriteAxis(destination, 8, caps.HasAxisSlider ? state.HandbrakeAxis : InputMapper.AxisMin);

        WriteU32(destination, 10, state.ButtonsMask & caps.ButtonMask);
        WriteU32(destination, 14, (uint)(state.ButtonsMaskEx & caps.Ex1Mask));
        WriteU32(destination, 18, (uint)(((state.ButtonsMaskEx >> 32) | state.EncoderPulseMask) & caps.Ex2Mask));
        // [22..25] buttons 97..128: koi source nahi — 0 rehte hain.
    }

    /// <summary>Neutral raw report (26 B) — watchdog/band karte waqt.</summary>
    public static void BuildNeutralRawReport(HidMaestroCapabilities caps, Span<byte> destination) =>
        BuildRawReport(ProcessedState.Neutral, caps, destination);

    /// <summary>Raw report se axis parho (test/verify ke liye): index 0=X, 1=Y, 2=Z, 3=Rx, 4=Slider.</summary>
    public static ushort ReadRawAxis(ReadOnlySpan<byte> report, int axisIndex)
    {
        if (report.Length < RawReportSize) throw new ArgumentException("Report chhota hai.", nameof(report));
        if (axisIndex is < 0 or > 4) throw new ArgumentOutOfRangeException(nameof(axisIndex));
        int offset = axisIndex * 2;
        return (ushort)(report[offset] | (report[offset + 1] << 8));
    }

    /// <summary>Raw report mein wheel button daba hua hai ya nahi (1..128).</summary>
    public static bool IsRawButtonPressed(ReadOnlySpan<byte> report, int button)
    {
        if (report.Length < RawReportSize) throw new ArgumentException("Report chhota hai.", nameof(report));
        if (button is < 1 or > 128) throw new ArgumentOutOfRangeException(nameof(button));
        int bit = button - 1;
        return (report[10 + bit / 8] & (1 << (bit % 8))) != 0;
    }

    private static void WriteAxis(Span<byte> destination, int offset, int axisValue)
    {
        ushort v = ToU16(axisValue);
        destination[offset] = (byte)(v & 0xFF);
        destination[offset + 1] = (byte)(v >> 8);
    }

    private static void WriteU32(Span<byte> destination, int offset, uint value)
    {
        destination[offset] = (byte)(value & 0xFF);
        destination[offset + 1] = (byte)((value >> 8) & 0xFF);
        destination[offset + 2] = (byte)((value >> 16) & 0xFF);
        destination[offset + 3] = (byte)((value >> 24) & 0xFF);
    }
}
