using RSH.Core.Processing;

namespace RSH.Core.Output;

/// <summary>
/// Pipeline ki tayyar output — vJoy par bhejne ke qabil aakhri halat.
/// Axis values vJoy ki range (1..32768) mein hain. Normalized values
/// GUI ke liye sath rakhe hain taake wo dobara hisaab na kare.
/// ButtonsMask: bit (n-1) = vJoy button n dabba hua (buttons 1..32).
/// ButtonsMaskEx: bit (n-33) = vJoy button n dabba hua (buttons 33..96) —
/// T11/Ex fix (SW-7): StatusPacketV2.BtnsEx (0-based buttons 32..63 =
/// vJoy 33..64) is ke neeche 32 bits mein aata hai; oopar 32 bits
/// (65..96) ka abhi koi wire source nahi (wire max 64 buttons hai),
/// aur 97..128 (LButtonsEx3) ka bhi koi source nahi — Ex3 hamesha 0.
/// ClutchNorm/ClutchAxis (Phase F WS5): clutch pedal 0..1 / vJoy axis
/// (WClutch) — sirf tab bhara hota hai jab v2 stream mein clutch slot (3)
/// maujood ho aur Clutch module enabled ho; warna 0 / AxisMin (UI is se
/// pehchanti hai ke clutch data asal mein aa raha hai ya nahi — fake bar
/// nahi). HandbrakeNorm/HandbrakeAxis waisa hi (slot 4); vJoy mein ye
/// Slider axis par jata hai (Phase G C8 — docs/HANDBRAKE-MAPPING.md),
/// neutral = AxisMin (chhora hua).
/// Gear (Phase F WS5): H-pattern gate mode mein engaged gear (0 = neutral,
/// 1..6); sequential mode ya v1 mein hamesha 0.
/// Enc0Delta..Enc3Delta (Phase F WS5): is tick ke encoder deltas
/// (cumulative i16 se wrap-aware; glitch-filtered — EncoderTracker).
/// EncoderGlitchesFiltered: ab tak filter hue glitch jumps (cumulative).
/// EncoderPulseMask (Phase F WS5): encoder deltas → vJoy pulse buttons.
/// Bit (n-65) = vJoy button n dabba hua (is tick): encoder i CW (delta
/// &gt; 0) → button 65+2i, CCW (delta &lt; 0) → button 66+2i (i = 0..3 →
/// buttons 65..72). Ye ButtonsMaskEx se ALAG field hai — ButtonsMaskEx
/// sirf wire buttons hain (stream 0..31 + status BtnsEx 32..63; us ke
/// oopar wale bits hamesha 0), jab ke pulses PC-side derived hain
/// (deltas se). VJoyOutput.BuildPosition inhein LButtonsEx2 mein OR
/// karta hai (device caps se mask — 72 se kam buttons wale device par
/// pulses khamoshi se mask ho jati hain, koi error nahi). Ek tick mein
/// delta 0 ho to pulse nahi (button chhoota); musalsal ghumane par har
/// tick naya pulse — button daba rehta hai jab tak encoder ghum raha ho.
/// Encoders module band ho to mask 0 (deltas ki tarah).
/// </summary>
public readonly record struct ProcessedState(
    int SteeringAxis,
    int AccelAxis,
    int BrakeAxis,
    uint ButtonsMask,
    double SteeringNorm,
    double AccelNorm,
    double BrakeNorm,
    ulong ButtonsMaskEx = 0,
    double ClutchNorm = 0,
    int ClutchAxis = InputMapper.AxisMin,
    double HandbrakeNorm = 0,
    int HandbrakeAxis = InputMapper.AxisMin,
    int Gear = 0,
    short Enc0Delta = 0,
    short Enc1Delta = 0,
    short Enc2Delta = 0,
    short Enc3Delta = 0,
    long EncoderGlitchesFiltered = 0,
    ulong EncoderPulseMask = 0)
{
    /// <summary>Watchdog wali mehfooz halat: wheel seedha, pedals khaali, buttons band.</summary>
    public static ProcessedState Neutral => new(
        SteeringAxis: InputMapper.AxisCenter,
        AccelAxis: InputMapper.AxisMin,
        BrakeAxis: InputMapper.AxisMin,
        ButtonsMask: 0,
        SteeringNorm: 0,
        AccelNorm: 0,
        BrakeNorm: 0,
        ClutchAxis: InputMapper.AxisMin,
        HandbrakeAxis: InputMapper.AxisMin);
}
