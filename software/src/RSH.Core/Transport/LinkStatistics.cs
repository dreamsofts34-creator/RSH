using RSH.Core.Protocol;

namespace RSH.Core.Transport;

/// <summary>
/// Link ke tamam counters ek jaga — receive thread likhta hai, UI/
/// diagnostics padhta hai. Tamam updates atomic/interlocked hain.
/// Rate: last-second ke valid packets ki ginati (har second refresh).
/// </summary>
public sealed class LinkStatistics
{
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
                lock (_seqLock) _sequences.Observe(sequence);
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

    /// <summary>
    /// Sequence ki buniyad saaf karo — AUDIT FIX: ESP reboot ke baad us ki
    /// sequence 0 se shuru hoti hai; purani baseline rehti to pehla packet
    /// "bari loss" ya duplicate gin liya jata tha. Link dobara bane
    /// (lamba waqfa khatam ho) to source yahan se reset bulata hai.
    /// </summary>
    public void ResetSequenceBaseline()
    {
        lock (_seqLock) _sequences.Reset();
    }

    /// <summary>Ab tak ke valid packets ke hisaab se loss ka faisad (0 jab koi data hi nahi).</summary>
    public double LossPercent
    {
        get
        {
            long valid = ValidPackets;
            long lost = LostPackets;
            long total = valid + lost;
            return total == 0 ? 0.0 : 100.0 * lost / total;
        }
    }
}
