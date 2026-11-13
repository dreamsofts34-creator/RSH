using RSH.Core.Protocol;

namespace RSH.Tests;

/// <summary>P3 — SequenceTracker reboot / re-baseline tests.</summary>
public class SequenceTrackerTests
{
    [Fact]
    public void Reboot_SequenceRestartsAtZero_NoDuplicateExplosion()
    {
        // Rig ka fault-reboot scenario (200 Hz, reboot@5 s, 2 s silence):
        // seq ...1000 phir 0,1,2... — pehle har packet duplicate gina
        // jata tha jab tak seq 1000 paar na kare (duplicatesOrLate ~988).
        var tracker = new SequenceTracker();
        for (int s = 0; s <= 1000; s++)
            tracker.Observe((ushort)s);

        for (int s = 0; s <= 999; s++)
            tracker.Observe((ushort)s);

        Assert.Equal(0, tracker.TotalDuplicatesOrLate);
        Assert.Equal(0, tracker.TotalLost);
        Assert.Equal(1, tracker.TotalRebaselines);
        Assert.Equal(2001, tracker.TotalObserved);
    }

    [Fact]
    public void Reboot_ShortSilence_StillRebaselines()
    {
        // Reboot foran baad (koi lambi khamoshi nahi) bhi pehchanna jaye —
        // SequenceTracker ko waqfe ka ilm nahi, sirf delta dekh kar.
        var tracker = new SequenceTracker();
        for (int s = 500; s <= 600; s++)
            tracker.Observe((ushort)s);
        tracker.Observe(0);
        tracker.Observe(1);
        tracker.Observe(2);

        Assert.Equal(0, tracker.TotalDuplicatesOrLate);
        Assert.Equal(1, tracker.TotalRebaselines);
    }

    [Fact]
    public void SmallBackwardJump_StillCountsAsDuplicate()
    {
        // Had (32) ke andar peeche hatna aam reorder/duplicate hai —
        // re-baseline nahi hona chahiye.
        var tracker = new SequenceTracker();
        tracker.Observe(100);
        tracker.Observe(101);
        tracker.Observe(101);   // duplicate, delta 0
        tracker.Observe(100);   // late, delta -1
        tracker.Observe(69);    // delta -32: had par, ab bhi duplicate

        Assert.Equal(3, tracker.TotalDuplicatesOrLate);
        Assert.Equal(0, tracker.TotalRebaselines);
        Assert.Equal(0, tracker.TotalLost);
    }

    [Fact]
    public void BackwardJumpBeyondThreshold_Rebaselines()
    {
        var tracker = new SequenceTracker();
        tracker.Observe(100);
        tracker.Observe(101);
        tracker.Observe(68);    // delta -33: had se bahar → reboot

        Assert.Equal(0, tracker.TotalDuplicatesOrLate);
        Assert.Equal(1, tracker.TotalRebaselines);
        // Re-baseline ke baad silsila nayi baseline se chalta hai.
        tracker.Observe(69);
        Assert.Equal(0, tracker.TotalDuplicatesOrLate);
        Assert.Equal(0, tracker.TotalLost);
    }

    [Fact]
    public void Wraparound_NotCountedAsLossOrDuplicate()
    {
        var tracker = new SequenceTracker();
        tracker.Observe(65534);
        tracker.Observe(65535);
        tracker.Observe(0);
        tracker.Observe(1);

        Assert.Equal(0, tracker.TotalDuplicatesOrLate);
        Assert.Equal(0, tracker.TotalLost);
        Assert.Equal(0, tracker.TotalRebaselines);
    }

    [Fact]
    public void ForwardJump_StillCountsAsLoss()
    {
        var tracker = new SequenceTracker();
        tracker.Observe(10);
        var obs = tracker.Observe(20);   // 9 packets ghayeb

        Assert.Equal(9, obs.Lost);
        Assert.Equal(9, tracker.TotalLost);
        Assert.Equal(0, tracker.TotalDuplicatesOrLate);
        Assert.Equal(0, tracker.TotalRebaselines);
    }

    [Fact]
    public void Reset_ClearsRebaselineCounterToo()
    {
        var tracker = new SequenceTracker();
        tracker.Observe(1000);
        tracker.Observe(0);
        Assert.Equal(1, tracker.TotalRebaselines);

        tracker.Reset();
        Assert.Equal(0, tracker.TotalRebaselines);
        Assert.Equal(0, tracker.TotalDuplicatesOrLate);
        Assert.Equal(0, tracker.TotalLost);
        Assert.Equal(0, tracker.TotalObserved);
    }

    // ---- Phase C, item 4: wraparound verify + T9 half-range ----

    [Fact]
    public void Wraparound_WithLossStillCounts()
    {
        // Wrap ke paar chhota gap asal loss hai — rebaseline nahi.
        var tracker = new SequenceTracker();
        tracker.Observe(65500);
        var obs = tracker.Observe(100);   // 135 packets ghayeb (wrap ke paar)
        Assert.Equal(135, obs.Lost);
        Assert.Equal(135, tracker.TotalLost);
        Assert.Equal(0, tracker.TotalRebaselines);
    }

    [Fact]
    public void Wraparound_DuplicateAfterWrap_CountsAsDuplicate()
    {
        var tracker = new SequenceTracker();
        tracker.Observe(65534);
        tracker.Observe(65535);
        tracker.Observe(0);
        tracker.Observe(0);   // duplicate
        tracker.Observe(65535);   // late (wrap se pehle wala)
        Assert.Equal(2, tracker.TotalDuplicatesOrLate);
        Assert.Equal(0, tracker.TotalLost);
        Assert.Equal(0, tracker.TotalRebaselines);
    }

    [Fact]
    public void Reboot_UpperHalfBaseline_RebaselinesNotPhantomLoss()
    {
        // BUG (Phase C mein mila): baseline jab 32768 se oopar ho
        // (masalan 40000), to reboot ke baad seq=0 par signed delta
        // +25536 banta hai — negative-delta wali P3 shart nahi pakarti
        // aur 25535 phantom lost gin liye jate thay (LossPercent
        // explode, long run mein har ~doosre reboot par). Reboot
        // hamesha 0 se shuru hota hai, is liye bari aage ki chhalang
        // jo 0 ke qareeb utre, reboot hai.
        var tracker = new SequenceTracker();
        tracker.Observe(40000);
        tracker.Observe(0);
        tracker.Observe(1);
        tracker.Observe(2);

        Assert.Equal(0, tracker.TotalDuplicatesOrLate);
        Assert.Equal(0, tracker.TotalLost);
        Assert.Equal(1, tracker.TotalRebaselines);
    }

    [Fact]
    public void Reboot_UpperHalfBaseline_MissedFirstPackets_StillRebaselines()
    {
        // Reboot ke baad pehle chand packets miss bhi hon (seq 500 se
        // shuru mile) to bhi reboot pehchanna jaye.
        var tracker = new SequenceTracker();
        tracker.Observe(40000);
        tracker.Observe(500);
        tracker.Observe(501);

        Assert.Equal(0, tracker.TotalDuplicatesOrLate);
        Assert.Equal(0, tracker.TotalLost);
        Assert.Equal(1, tracker.TotalRebaselines);
    }

    [Fact]
    public void Reboot_NearWrap_SmallPhantomLoss_AcceptedResidual()
    {
        // T9 mein darj residual: baseline wrap ke 1024 ke andar ho
        // (65530) to reboot par delta chhota (+6) banta hai aur
        // reboot pehchanna nahi jata — 5 phantom lost gin liye jate
        // hain. Ye bounded (≤ 1024) hai aur silsila khud theek ho
        // jata hai; LossPercent par asar na hone ke barabar hai.
        var tracker = new SequenceTracker();
        tracker.Observe(65530);
        var obs = tracker.Observe(0);
        Assert.Equal(5, obs.Lost);
        Assert.Equal(0, tracker.TotalRebaselines);
        tracker.Observe(1);
        tracker.Observe(2);
        Assert.Equal(5, tracker.TotalLost);
        Assert.Equal(0, tracker.TotalDuplicatesOrLate);
    }

    [Fact]
    public void HalfRange_Exact32768_RebaselinesNotLoss()
    {
        // T9: aage ki chhalang bilkul 32768 par signed delta -32768
        // ban jata hai — reboot samajh kar re-baseline hoti hai,
        // 32767 lost nahi gina jata (dekho docs/T9-DECISION.md).
        var tracker = new SequenceTracker();
        tracker.Observe(0);
        tracker.Observe(32768);

        Assert.Equal(0, tracker.TotalLost);
        Assert.Equal(1, tracker.TotalRebaselines);
    }

    [Fact]
    public void HalfRange_JustBelow32767_CountsAsLoss()
    {
        // Had ke neeche (32767) aage ki chhalang asal loss hai.
        var tracker = new SequenceTracker();
        tracker.Observe(0);
        var obs = tracker.Observe(32767);

        Assert.Equal(32766, obs.Lost);
        Assert.Equal(32766, tracker.TotalLost);
        Assert.Equal(0, tracker.TotalRebaselines);
    }

    [Fact]
    public void GenuineLoss_LandingAwayFromZero_CountsAsLoss()
    {
        // Bari chhalang jo 0 ke qareeb NA utre, asal loss hai —
        // reboot heuristic sirf seq ≤ 1024 par utarne par lagta hai.
        var tracker = new SequenceTracker();
        tracker.Observe(100);
        var obs = tracker.Observe(2000);

        Assert.Equal(1899, obs.Lost);
        Assert.Equal(1899, tracker.TotalLost);
        Assert.Equal(0, tracker.TotalRebaselines);
    }
}
