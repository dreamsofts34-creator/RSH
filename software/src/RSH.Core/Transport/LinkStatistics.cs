using RSH.Core.Protocol;

namespace RSH.Core.Transport;

/// <summary>
/// Link ke tamam counters ek jaga — receive thread likhta hai, UI/
/// diagnostics padhta hai. Tamam updates atomic/interlocked hain.
/// Rate: last-second ke valid packets ki ginati (har second refresh).
/// LossPercent (P8/D8): cumulative NAHI — last-1000 expected packets
/// (valid + sequence gap se ghayeb) ki sliding window. Purana loss
/// window se bahar girte hi percent se nikal jata hai; koi periodic
/// reset nahi hota. Duplicate/late packets window mein nahi ginay
/// jate — wo pehle se gini hui sequence position ki dobara delivery
/// hain, naye expected packet nahi.
/// </summary>
public sealed class LinkStatistics
{
    /// <summary>LossPercent window ka size (D8): last 1000 packets.</summary>
    public const int LossWindowSize = 1000;

    private long _totalDatagrams;
    private long _validPackets;
    private long _tooShort;
    private long _badMagic;
    private long _badVersion;
    private long _badCrc;
    private long _validThisSecond;
    private long _currentRate;
    private long _lastRateStampMs;
    private readonly SequenceTracker _sequences = new();
    private readonly object _seqLock = new();

    // Sliding window (sirf _seqLock ke andar chhoo): ring buffer jisme
    // har entry ek expected packet hai — true = ghayeb (lost), false =
    // valid. _windowHead agle likhne ki jagah hai (bharne par sab se
    // purani entry wahi hoti hai).
    private readonly bool[] _window = new bool[LossWindowSize];
    private int _windowHead;
    private int _windowCount;
    private int _windowLost;

    public long TotalDatagrams => Interlocked.Read(ref _totalDatagrams);
    public long ValidPackets => Interlocked.Read(ref _validPackets);
    public long TooShort => Interlocked.Read(ref _tooShort);
    public long BadMagic => Interlocked.Read(ref _badMagic);
    public long BadVersion => Interlocked.Read(ref _badVersion);
    public long BadCrc => Interlocked.Read(ref _badCrc);
    public long CurrentRatePerSecond => Interlocked.Read(ref _currentRate);

    public long LostPackets { get { lock (_seqLock) return _sequences.TotalLost; } }
    public long DuplicatesOrLate { get { lock (_seqLock) return _sequences.TotalDuplicatesOrLate; } }

    /// <summary>Jo datagram aya, us ka hisaab. Valid ho to sequence bhi observe hota hai.</summary>
    public void Record(PacketParseResult result, ushort sequence = 0)
    {
        Interlocked.Increment(ref _totalDatagrams);
        switch (result)
        {
            case PacketParseResult.Ok:
                Interlocked.Increment(ref _validPackets);
                Interlocked.Increment(ref _validThisSecond);
                lock (_seqLock)
                {
                    var obs = _sequences.Observe(sequence);
                    // Duplicate/late: koi naya expected packet nahi —
                    // window mein nahi ginna (warna loss% ghata dikhega).
                    // Rebaseline (reboot) par bhi lost=0 hota hai, is
                    // liye reboot se LossPercent explode nahi hota (P3).
                    if (!obs.IsDuplicateOrLate)
                    {
                        // Gap ke ghayeb packets pehle, phir ye valid packet.
                        // Gap window se bara ho to sirf aakhri 1000 entries
                        // matter karti hain — zyada likhne ka faida nahi.
                        long gap = Math.Min(obs.Lost, LossWindowSize);
                        for (long i = 0; i < gap; i++) PushWindowEntry(lost: true);
                        PushWindowEntry(lost: false);
                    }
                }
                break;
            case PacketParseResult.TooShort: Interlocked.Increment(ref _tooShort); break;
            case PacketParseResult.BadMagic: Interlocked.Increment(ref _badMagic); break;
            case PacketParseResult.BadVersion: Interlocked.Increment(ref _badVersion); break;
            case PacketParseResult.BadCrc: Interlocked.Increment(ref _badCrc); break;
        }
    }

    /// <summary>Har ~1 s par pipeline/UI se call ho — rate ka hisaab taaza hota hai.</summary>
    public void RefreshRate(long nowMs)
    {
        long last = Interlocked.Read(ref _lastRateStampMs);
        if (last == 0)
        {
            Interlocked.Exchange(ref _lastRateStampMs, nowMs);
            return;
        }
        if (nowMs - last >= 1000)
        {
            long count = Interlocked.Exchange(ref _validThisSecond, 0);
            Interlocked.Exchange(ref _currentRate, count * 1000 / Math.Max(1, nowMs - last));
            Interlocked.Exchange(ref _lastRateStampMs, nowMs);
        }
    }

    /// <summary>Window mein ek expected packet darj karo. Sirf _seqLock ke andar.</summary>
    private void PushWindowEntry(bool lost)
    {
        if (_windowCount == LossWindowSize)
        {
            // Buffer bhara hai — sab se purani entry (_windowHead par)
            // bahar girti hai.
            if (_window[_windowHead]) _windowLost--;
        }
        else
        {
            _windowCount++;
        }
        _window[_windowHead] = lost;
        if (lost) _windowLost++;
        _windowHead = (_windowHead + 1) % LossWindowSize;
    }

    /// <summary>
    /// Sequence ki buniyad saaf karo — AUDIT FIX: ESP reboot ke baad us ki
    /// sequence 0 se shuru hoti hai; purani baseline rehti to pehla packet
    /// "bari loss" ya duplicate gin liya jata tha. Link dobara bane
    /// (lamba waqfa khatam ho) to source yahan se reset bulata hai.
    /// Naya link/session hai, is liye sliding window bhi saaf hoti hai —
    /// warna LostPackets (cumulative, 0 ho gaya) aur LossPercent (purane
    /// session ka loss) ka display mutazad hota. Window ke andar (baghair
    /// reset) purana loss sirf age-out se nikalta hai — yehi D8 hai.
    /// </summary>
    public void ResetSequenceBaseline()
    {
        lock (_seqLock)
        {
            _sequences.Reset();
            Array.Clear(_window);
            _windowHead = 0;
            _windowCount = 0;
            _windowLost = 0;
        }
    }

    /// <summary>
    /// Last-1000 expected packets (valid + lost) mein loss ka faisad
    /// (P8/D8 sliding window). 0 jab abhi koi packet hi nahi aaya.
    /// </summary>
    public double LossPercent
    {
        get
        {
            lock (_seqLock)
            {
                return _windowCount == 0 ? 0.0 : 100.0 * _windowLost / _windowCount;
            }
        }
    }
}
