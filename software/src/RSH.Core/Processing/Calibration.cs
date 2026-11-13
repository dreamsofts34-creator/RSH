namespace RSH.Core.Processing;

/// <summary>
/// Steering ki calibration: teen nuqte (min / center / max) — hamesha
/// milliVolts mein (plan D10 / P9). Packet v2.1 mein sab axes mV hote
/// hain (D6), is liye raw-ADC units wali calibration yahan kabhi
/// lagti hi nahi: <see cref="IsMillivolts"/> false ho to
/// <see cref="IsSane"/> false hai aur mapper default range istemal
/// karta hai. IsSet false ho to mapper mode ke hisaab se default
/// range istemal karta hai (GUI wizard asal calibration lazmi
/// karwayega).
/// </summary>
public sealed record SteeringCalibration
{
    /// <summary>Full-scale mV range (D10): ADC reference 0–3300 mV.</summary>
    public const int FullScaleMillivolts = 3300;

    /// <summary>
    /// Kam se kam span (D10): full-scale ka 20% = 660 mV. Is se
    /// tang calibration shor/deadzone jesi hoti hai — reject hoti hai.
    /// </summary>
    public const int MinSpanMillivolts = FullScaleMillivolts * 20 / 100;

    /// <summary>
    /// Kam se kam ek taraf ka safar (D10, SetupGates): span ka aadha =
    /// 330 mV. Wizard ka gate har taraf 330 mV mangta hai — dono taraf
    /// mil kar span kam se kam 660 mV banta hai, is liye wizard se
    /// guzri calibration IsSane se kabhi reject nahi hoti (pehle wizard
    /// 250 raw/side mangta tha = sirf ~403 mV kul span, aur Finish
    /// IsSane par reject kar deta tha — SETUP-GATES-DECISION.md).
    /// </summary>
    public const int MinSideMillivolts = MinSpanMillivolts / 2;

    public int RawMin { get; init; }
    public int RawCenter { get; init; }
    public int RawMax { get; init; }
    public bool IsSet { get; init; }

    /// <summary>
    /// Unit tag (P9, lazmi): true = qeematein milliVolts mein hain.
    /// Purani / raw-ADC calibration (false) mV packet par kabhi nahi
    /// lagti — ghalat unit silently ghalat mapping banata tha.
    /// </summary>
    public bool IsMillivolts { get; init; }

    public bool IsSane =>
        IsSet && IsMillivolts
        && RawMin < RawCenter && RawCenter < RawMax
        && RawMin >= 0 && RawMax <= FullScaleMillivolts
        && (RawMax - RawMin) >= MinSpanMillivolts;
}

/// <summary>
/// Pedal ki calibration: do nuqte (min = chora hua, max = pura dabaya)
/// — hamesha milliVolts mein (P9/D10, SteeringCalibration jaisa).
/// </summary>
public sealed record PedalCalibration
{
    /// <summary>Full-scale mV range (D10): 0–3300 mV.</summary>
    public const int FullScaleMillivolts = 3300;

    /// <summary>Kam se kam span (D10): full-scale ka 20% = 660 mV.</summary>
    public const int MinSpanMillivolts = FullScaleMillivolts * 20 / 100;

    public int RawMin { get; init; }
    public int RawMax { get; init; }
    public bool IsSet { get; init; }

    /// <summary>Unit tag (P9, lazmi): true = qeematein milliVolts mein hain.</summary>
    public bool IsMillivolts { get; init; }

    public bool IsSane =>
        IsSet && IsMillivolts
        && RawMax > RawMin
        && RawMin >= 0 && RawMax <= FullScaleMillivolts
        && (RawMax - RawMin) >= MinSpanMillivolts;
}
