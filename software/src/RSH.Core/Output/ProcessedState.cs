using RSH.Core.Processing;

namespace RSH.Core.Output;

/// <summary>
/// Pipeline ki tayyar output — vJoy par bhejne ke qabil aakhri halat.
/// Axis values vJoy ki range (1..32768) mein hain. Normalized values
/// GUI ke liye sath rakhe hain taake wo dobara hisaab na kare.
/// ButtonsMask: bit (n-1) = vJoy button n dabba hua.
/// </summary>
public readonly record struct ProcessedState(
    int SteeringAxis,
    int AccelAxis,
    int BrakeAxis,
    uint ButtonsMask,
    double SteeringNorm,
    double AccelNorm,
    double BrakeNorm)
{
    /// <summary>Watchdog wali mehfooz halat: wheel seedha, pedals khaali, buttons band.</summary>
    public static ProcessedState Neutral => new(
        SteeringAxis: InputMapper.AxisCenter,
        AccelAxis: InputMapper.AxisMin,
        BrakeAxis: InputMapper.AxisMin,
        ButtonsMask: 0,
        SteeringNorm: 0,
        AccelNorm: 0,
        BrakeNorm: 0);
}
