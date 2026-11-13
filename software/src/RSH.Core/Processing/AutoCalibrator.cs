namespace RSH.Core.Processing;

/// <summary>
/// Auto-calibration ka dimagh — Advanced Setup Wizard yahi istemal karta
/// hai taake user ko haath se numbers na likhne paren:
/// - Centre: wheel ko sukoon se darmiyan choro, samples ka trimmed mean
///   centre ban jata hai.
/// - Range: wheel lock-to-lock ghumao (ya pedal poora dabao), observed
///   extremes ke 1%/99% percentile endpoints bante hain — ADC spike
///   (0/4095 jesi achanak qeemat) endpoint kharab nahi karti.
/// - Khatam tabhi hota hai jab span kaafi ho (D10: steering aur pedal
///   dono ke liye kam se kam 660 mV = full-scale 0–3300 mV ka 20%) —
///   warna wizard "aur ghumao/dabao" kehta rehta hai.
/// Samples sirf is session ke liye jama hote hain; koi file nahi banti.
/// (P10) Samples bounded ring buffer mein rehte hain — lambi wizard
/// session mein bhi memory bounded rehti hai; purane samples sab se
/// pehle bahar girte hain.
/// </summary>
public sealed class AutoCalibrator
{
    /// <summary>
    /// Kam se kam steering span (D10: 20% of 3300 mV). Pehle 200 raw
    /// tha — us se bani calibration nayi <see cref="SteeringCalibration.IsSane"/>
    /// (660 mV) se reject ho jati thi, is liye dono ab ek hain.
    /// </summary>
    public const int MinSteeringSpan = SteeringCalibration.MinSpanMillivolts;

    /// <summary>Kam se kam pedal span (D10: 20% of 3300 mV). Pehle 100 raw tha.</summary>
    public const int MinPedalSpan = PedalCalibration.MinSpanMillivolts;

    /// <summary>
    /// Har channel par zyada se zyada kitne samples rakhe jate hain
    /// (P10). 2048 samples = 200 Hz par ~10 s — percentile/trimmed-mean
    /// ke liye kaafi, memory har channel par 4 KB (ushort) par bounded.
    /// </summary>
    public const int MaxSamplesPerChannel = 2048;

    /// <summary>
    /// Bounded ring buffer: capacity par pohanch kar sab se purana
    /// sample overwrite hota hai — Add kabhi allocate nahi karta aur
    /// Count kabhi capacity se zyada nahi hota.
    /// </summary>
    private sealed class BoundedSamples
    {
        private readonly ushort[] _buffer = new ushort[MaxSamplesPerChannel];
        private int _head;   // agle likhne ki jagah
        private int _count;

        public int Count => _count;

        public void Add(ushort value)
        {
            _buffer[_head] = value;
            _head = (_head + 1) % MaxSamplesPerChannel;
            if (_count < MaxSamplesPerChannel) _count++;
        }

        public void Clear()
        {
            _head = 0;
            _count = 0;
        }

        /// <summary>Maujooda samples ki copy (insertion order mein nahi — ring order).</summary>
        public ushort[] Snapshot()
        {
            var copy = new ushort[_count];
            for (int i = 0; i < _count; i++)
                copy[i] = _buffer[(_head - _count + i + MaxSamplesPerChannel * 2) % MaxSamplesPerChannel];
            return copy;
        }
    }

    private readonly BoundedSamples _centerSamples = new();
    private readonly BoundedSamples _steerRangeSamples = new();
    private readonly BoundedSamples _accelSamples = new();
    private readonly BoundedSamples _brakeSamples = new();

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
    public int AccelSampleCount => _accelSamples.Count;
    public int BrakeSampleCount => _brakeSamples.Count;

    /// <summary>Centre tayyar hai? Kam se kam 20 samples chahiye (200 Hz par 0.1 s).</summary>
    public bool TryGetCenter(out ushort center)
    {
        center = 0;
        if (_centerSamples.Count < 20) return false;
        center = TrimmedMean(_centerSamples.Snapshot());
        return true;
    }

    /// <summary>Steering endpoints tayyar hain? Span kaafi ho tabhi true.</summary>
    public bool TryGetSteeringRange(out ushort min, out ushort max)
    {
        min = 0; max = 0;
        if (_steerRangeSamples.Count < 50) return false;
        (min, max) = PercentileRange(_steerRangeSamples.Snapshot());
        return max - min >= MinSteeringSpan;
    }

    /// <summary>Pedal ka (resting, pressed) jora — span kaafi ho tabhi true.</summary>
    public bool TryGetAccelRange(out ushort min, out ushort max) => TryPedal(_accelSamples, out min, out max);
    public bool TryGetBrakeRange(out ushort min, out ushort max) => TryPedal(_brakeSamples, out min, out max);

    private static bool TryPedal(BoundedSamples samples, out ushort min, out ushort max)
    {
        min = 0; max = 0;
        if (samples.Count < 50) return false;
        (min, max) = PercentileRange(samples.Snapshot());
        return max - min >= MinPedalSpan;
    }

    private static ushort TrimmedMean(ReadOnlySpan<ushort> samples)
    {
        if (samples.Length == 0) return 0;
        var sorted = samples.ToArray();
        Array.Sort(sorted);
        int trim = sorted.Length / 10;   // dono taraf se 10% — spike mehfooz
        long sum = 0;
        int n = 0;
        for (int i = trim; i < sorted.Length - trim; i++) { sum += sorted[i]; n++; }
        return (ushort)(sum / Math.Max(1, n));
    }

    /// <summary>
    /// 1%/99% percentile range. Guard: chhote n par bhi endpoints
    /// hamesha andar ke samples hote hain — n=50 par lo index 0 hota
    /// (ek spike hi endpoint ban jata), is liye lo kam se kam 1 aur
    /// hi zyada se zyada n-2 hai: ek akela spike kisi bhi n par
    /// endpoint nahi ban sakta. Khaali/tanhai sample par (0, 0).
    /// </summary>
    private static (ushort Min, ushort Max) PercentileRange(ReadOnlySpan<ushort> samples)
    {
        if (samples.Length == 0) return (0, 0);
        if (samples.Length == 1) return (samples[0], samples[0]);
        var sorted = samples.ToArray();
        Array.Sort(sorted);
        if (sorted.Length < 3) return (sorted[0], sorted[^1]);
        int lo = Math.Clamp((int)(sorted.Length * 0.01), 1, sorted.Length - 2);
        int hi = Math.Clamp((int)(sorted.Length * 0.99), 1, sorted.Length - 2);
        if (hi < lo) hi = lo;
        return (sorted[lo], sorted[hi]);
    }
}
