namespace RSH.Core.Protocol;

/// <summary>Ek sequence number dekhne ka nateeja.</summary>
public readonly record struct SequenceObservation(long Lost, bool IsDuplicateOrLate);

/// <summary>
/// u16 sequence numbers se packet loss gin'na. Firmware ke scheduler ki
/// tarah signed-difference idiom: aage barhna = kuch packets ghayeb,
/// peeche/duplicate = late ya dohra packet (loss nahi).
/// Pehla packet baseline hota hai (koi loss nahi).
///
/// Half-range / reboot rules (T9 — poora faisla
/// <c>docs/T9-DECISION.md</c> mein):
/// - Signed delta ki had 32768 hai: aage ki chhalang 32768 par delta
///   -32768 ban jata hai aur reboot samajh kar re-baseline hoti hai,
///   32767 lost nahi gina jata.
/// - Reboot hamesha sequence 0 se shuru karta hai, is liye bari aage
///   ki chhalang (delta &gt; 1024) jo 0 ke qareeb utre (seq ≤ 1024)
///   bhi reboot hai — ye woh case pakarta hai jo negative-delta wali
///   P3 shart se nahi pakra jata (baseline jab 32768 se oopar ho,
///   masalan 40000, to reboot par delta +25536 banta hai aur pehle
///   25535 phantom lost gin liye jate thay).
/// </summary>
public sealed class SequenceTracker
{
    /// <summary>
    /// Re-baseline ki had: is se bara peeche chhalang (delta &lt; -32)
    /// aam reorder/duplicate nahi ho sakta — rig mein reorder sirf 1–3
    /// packet hold karta hai, aur duplicate ka delta 0 hota hai. Is se
    /// bara negative delta matlab board reboot hua aur sequence 0 se
    /// shuru hui (P3). Had se chhota peeche hatna ab bhi duplicate
    /// gina jata hai (neeche dekho).
    /// </summary>
    public const int RebaselineThreshold = 32;

    /// <summary>
    /// Reboot window (T9): reboot ke baad pehla dekha gaya sequence
    /// number is se zyada nahi hota — board seq 0 se shuru karta hai
    /// aur PC 200 Hz par ~5 s ke andar dobara sunne lagta hai (1024
    /// packets ≈ 5.1 s). Aage ki chhalang jo is window ke andar utre
    /// aur delta is se bara ho, reboot ki nishani hai (neeche dekho).
    /// </summary>
    public const int RebootWindow = 1024;

    private ushort _last;
    private bool _hasBaseline;

    public long TotalLost { get; private set; }
    public long TotalDuplicatesOrLate { get; private set; }
    public long TotalObserved { get; private set; }

    /// <summary>Bare negative delta par kitni dafa baseline naye sire se bani (reboot).</summary>
    public long TotalRebaselines { get; private set; }

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
        if (delta < -RebaselineThreshold)
        {
            // P3 FIX: board reboot — sequence 0 se shuru, purani baseline
            // (_last ≈ 1000) rehti to seq ke _last paar karne tak HAR packet
            // duplicate gina jata (~988 duplicates rig mein). Bara negative
            // delta = naya session, is liye baseline dobara banao; ye na
            // loss hai na duplicate. (Chhoti khamoshi par bhi kaam karta
            // hai — UdpWheelSource ka 5 s reset ab sirf backup hai.)
            TotalRebaselines++;
            _last = sequence;
            return new SequenceObservation(0, false);
        }

        if (delta <= 0)
        {
            TotalDuplicatesOrLate++;
            return new SequenceObservation(0, true);
        }

        // T9 FIX (Phase C): reboot jab baseline oopri half-range mein ho.
        // Masalan _last=40000 aur reboot ke baad seq=0: signed delta
        // +25536 banta hai — negative shart nahi pakarti aur 25535
        // phantom lost gin liye jate thay (LossPercent explode, long
        // run mein har ~doosre reboot par). Reboot hamesha 0 se shuru
        // hota hai, is liye delta > 1024 ke saath seq ≤ 1024 par
        // utarna reboot hai, aam loss nahi.
        // Had: _last wrap ke 1024 ke andar ho (masalan 65000 → seq 0,
        // delta 536) to ye shart nahi pakarti — phantom loss ≤ 1024
        // rehta hai aur silsila khud theek ho jata hai (T9 doc).
        // Ulta case (asal loss jo 0 ke qareeb utre) ke liye ≥ 5 s ki
        // khamoshi chahiye hoti hai — woh UdpWheelSource ka 5 s backup
        // reset pehle hi pakar leta hai, is liye tracker tak nahi
        // pohanchta.
        if (sequence <= RebootWindow && delta > RebootWindow)
        {
            TotalRebaselines++;
            _last = sequence;
            return new SequenceObservation(0, false);
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
        TotalRebaselines = 0;
    }
}
