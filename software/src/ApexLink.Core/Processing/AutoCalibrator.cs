namespace ApexLink.Core.Processing;

/// <summary>
/// Auto-calibration ka dimagh — Advanced Setup Wizard yahi istemal karta
/// hai taake user ko haath se numbers na likhne paren:
/// - Centre: wheel ko sukoon se darmiyan choro, samples ka trimmed mean
///   centre ban jata hai.
/// - Range: wheel lock-to-lock ghumao (ya pedal poora dabao), observed
///   extremes ke 1%/99% percentile endpoints bante hain — ADC spike
///   (0/4095 jesi achanak qeemat) endpoint kharab nahi karti.
/// - Khatam tabhi hota hai jab span kaafi ho (steering >= 200 raw,
///   pedal >= 100 raw) — warna wizard "aur ghumao/dabao" kehta rehta hai.
/// Samples sirf is session ke liye jama hote hain; koi file nahi banti.
/// </summary>
public sealed class AutoCalibrator
{
    public const int MinSteeringSpan = 200;
    public const int MinPedalSpan = 100;

    private readonly List<ushort> _centerSamples = [];
    private readonly List<ushort> _steerRangeSamples = [];
    private readonly List<ushort> _accelSamples = [];
    private readonly List<ushort> _brakeSamples = [];

    public void Reset()
    {
        _centerSamples.Clear();
        _steerRangeSamples.Clear();
        _accelSamples.Clear();
        _brakeSamples.Clear();
    }

    public void AddCenterSample(ushort raw) => _centerSamples.Add(raw);
    public void AddSteeringRangeSample(ushort raw) => _steerRangeSamples.Add(raw);
    public void AddAccelSample(ushort raw) => _accelSamples.Add(raw);
    public void AddBrakeSample(ushort raw) => _brakeSamples.Add(raw);

    public int CenterSampleCount => _centerSamples.Count;
    public int SteeringRangeSampleCount => _steerRangeSamples.Count;

    /// <summary>Centre tayyar hai? Kam se kam 20 samples chahiye (200 Hz par 0.1 s).</summary>
    public bool TryGetCenter(out ushort center)
    {
        center = 0;
        if (_centerSamples.Count < 20) return false;
        center = TrimmedMean(_centerSamples);
        return true;
    }

    /// <summary>Steering endpoints tayyar hain? Span kaafi ho tabhi true.</summary>
    public bool TryGetSteeringRange(out ushort min, out ushort max)
    {
        min = 0; max = 0;
        if (_steerRangeSamples.Count < 50) return false;
        (min, max) = PercentileRange(_steerRangeSamples);
        return max - min >= MinSteeringSpan;
    }

    /// <summary>Pedal ka (resting, pressed) jora — span kaafi ho tabhi true.</summary>
    public bool TryGetAccelRange(out ushort min, out ushort max) => TryPedal(_accelSamples, out min, out max);
    public bool TryGetBrakeRange(out ushort min, out ushort max) => TryPedal(_brakeSamples, out min, out max);

    private static bool TryPedal(List<ushort> samples, out ushort min, out ushort max)
    {
        min = 0; max = 0;
        if (samples.Count < 50) return false;
        (min, max) = PercentileRange(samples);
        return max - min >= MinPedalSpan;
    }

    private static ushort TrimmedMean(List<ushort> samples)
    {
        var sorted = samples.OrderBy(v => v).ToArray();
        int trim = sorted.Length / 10;   // dono taraf se 10% — spike mehfooz
        long sum = 0;
        int n = 0;
        for (int i = trim; i < sorted.Length - trim; i++) { sum += sorted[i]; n++; }
        return (ushort)(sum / Math.Max(1, n));
    }

    private static (ushort Min, ushort Max) PercentileRange(List<ushort> samples)
    {
        var sorted = samples.OrderBy(v => v).ToArray();
        int lo = (int)(sorted.Length * 0.01);
        int hi = Math.Min(sorted.Length - 1, (int)(sorted.Length * 0.99));
        return (sorted[lo], sorted[hi]);
    }
}
