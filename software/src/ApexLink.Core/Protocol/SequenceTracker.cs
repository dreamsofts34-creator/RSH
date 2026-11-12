namespace ApexLink.Core.Protocol;

/// <summary>Ek sequence number dekhne ka nateeja.</summary>
public readonly record struct SequenceObservation(long Lost, bool IsDuplicateOrLate);

/// <summary>
/// u16 sequence numbers se packet loss gin'na. Firmware ke scheduler ki
/// tarah signed-difference idiom: aage barhna = kuch packets ghayeb,
/// peeche/duplicate = late ya dohra packet (loss nahi).
/// Pehla packet baseline hota hai (koi loss nahi).
/// </summary>
public sealed class SequenceTracker
{
    private ushort _last;
    private bool _hasBaseline;

    public long TotalLost { get; private set; }
    public long TotalDuplicatesOrLate { get; private set; }
    public long TotalObserved { get; private set; }

    public SequenceObservation Observe(ushort sequence)
    {
        TotalObserved++;
        if (!_hasBaseline)
        {
            _hasBaseline = true;
            _last = sequence;
            return new SequenceObservation(0, false);
        }

        short delta = (short)(sequence - _last);
        if (delta <= 0)
        {
            TotalDuplicatesOrLate++;
            return new SequenceObservation(0, true);
        }

        long lost = delta - 1;
        TotalLost += lost;
        _last = sequence;
        return new SequenceObservation(lost, false);
    }

    public void Reset()
    {
        _hasBaseline = false;
        TotalLost = 0;
        TotalDuplicatesOrLate = 0;
        TotalObserved = 0;
    }
}
