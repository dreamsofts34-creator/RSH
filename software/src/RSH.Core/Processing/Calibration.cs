namespace RSH.Core.Processing;

/// <summary>
/// Steering ki calibration: teen nuqte (min / center / max) raw units
/// mein — mV jab packet mV mode mein ho, warna raw ADC. IsSet false ho
/// to mapper mode ke hisaab se default range istemal karta hai
/// (GUI wizard asal calibration lazmi karwayega).
/// </summary>
public sealed record SteeringCalibration
{
    public int RawMin { get; init; }
    public int RawCenter { get; init; }
    public int RawMax { get; init; }
    public bool IsSet { get; init; }

    public bool IsSane =>
        IsSet && RawMin < RawCenter && RawCenter < RawMax && (RawMax - RawMin) >= 100;
}

/// <summary>Pedal ki calibration: do nuqte (min = chora hua, max = pura dabaya).</summary>
public sealed record PedalCalibration
{
    public int RawMin { get; init; }
    public int RawMax { get; init; }
    public bool IsSet { get; init; }

    public bool IsSane => IsSet && RawMax > RawMin && (RawMax - RawMin) >= 100;
}
