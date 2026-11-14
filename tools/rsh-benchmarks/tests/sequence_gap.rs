//! Maximum-size sequence-gap guard (M-7, Phase 2 deep scan).
//!
//! A forward jump of delta 32767 is the largest `SequenceTracker::check`
//! classifies as a gap (`delta <= u16::MAX / 2`); it yields
//! `Gap(32766)`. A `Gap(32767)` can never occur: delta 32768 crosses
//! the half-range boundary and is a reboot rebaseline instead. The
//! tracker must handle the max gap arithmetically —
//! `record_outcome` fills the 1000-entry loss window in one shot for
//! `lost >= 1000` — never by iterating once per skipped packet. The
//! test below bounds the wall time of 10 000 consecutive max-gap
//! checks: a per-skipped-packet implementation would do ~327M
//! iterations here, the arithmetic one ~10M bounded window writes.
//!
//! The per-skipped-packet loop that DID exist is in the engine, not
//! the tracker: `WheelEngine::handle_frame`
//! (`crates/rsh-engine/src/engine.rs`) ran
//! `for _ in 0..skipped { self.metrics.record_packet(false); }`.
//! Its fix (ENGINE worker) is one call to the already-implemented
//! `LossWindow::record_many` (L1) via a new `EngineMetrics::record_many`
//! forwarding method — see the Phase 2 report. This test guards the
//! tracker half from the rig/benchmark side.

use std::time::Instant;

use rsh_engine::sequence::{SeqDecision, SequenceTracker};

/// The largest forward jump `check()` classifies as a gap.
const MAX_GAP_DELTA: u16 = u16::MAX / 2;
/// The `Gap` value that jump yields (`delta - 1` skipped sequences).
const MAX_GAP_SKIPPED: u16 = MAX_GAP_DELTA - 1;

#[test]
fn max_gap_is_gap_32766_not_a_rebaseline() {
    let mut tracker = SequenceTracker::new();
    assert_eq!(tracker.check(100), SeqDecision::Accept);
    assert_eq!(
        tracker.check(100u16.wrapping_add(MAX_GAP_DELTA)),
        SeqDecision::Gap(MAX_GAP_SKIPPED),
        "delta 32767 is the largest forward gap"
    );
    assert_eq!(tracker.gaps(), u64::from(MAX_GAP_SKIPPED));
    assert_eq!(tracker.accepted(), 2);
    // The loss window is saturated by one max gap: 999 lost of 1000.
    assert!(
        tracker.loss_percent() > 99.0,
        "one max gap saturates the window: {}%",
        tracker.loss_percent()
    );
    // And one step further (delta 32768 from the last accepted seq)
    // is a reboot rebaseline — but ONLY with reboot evidence (L8):
    // plain `check()` passes no evidence, so the same jump is a
    // Duplicate there. Last accepted is 100 + 32767 = 32867; +32768
    // wraps to 99.
    let rebaseline_target = 100u16
        .wrapping_add(MAX_GAP_DELTA)
        .wrapping_add(MAX_GAP_DELTA)
        .wrapping_add(1);
    assert_eq!(
        tracker.check(rebaseline_target),
        SeqDecision::Duplicate,
        "delta 32768 with no evidence is a duplicate, never a rebaseline (L8)"
    );
    assert_eq!(
        tracker.check_with_evidence(rebaseline_target, true),
        SeqDecision::Accept,
        "delta 32768 WITH reboot evidence is a rebaseline accept"
    );
    assert_eq!(tracker.rebaselines(), 1);
}

#[test]
fn ten_thousand_max_gaps_are_fast() {
    let mut tracker = SequenceTracker::new();
    let mut last: u16 = 0;
    for _ in 0..2000 {
        let _decision = tracker.check(last);
        last = last.wrapping_add(1);
    }
    // Warm-up accepted seqs 0..=1999, so `last` is now 2000 and the
    // tracker's last accepted is 1999. Each max gap jumps from the
    // last ACCEPTED seq (not the next expected one): target =
    // last_accepted + 32767.
    let mut last_accepted: u16 = 1999;
    let start = Instant::now();
    for _ in 0..10_000 {
        let target = last_accepted.wrapping_add(MAX_GAP_DELTA);
        let decision = tracker.check(target);
        assert_eq!(decision, SeqDecision::Gap(MAX_GAP_SKIPPED));
        last_accepted = target;
    }
    let _ = last;
    let elapsed = start.elapsed();
    // Warm-up checks are all delta-1 accepts, so every gap comes
    // from the 10 000 max-gap checks.
    assert_eq!(tracker.gaps(), 10_000 * u64::from(MAX_GAP_SKIPPED));
    assert!(
        elapsed.as_millis() < 500,
        "10 000 max-gap checks took {elapsed:?} — the gap path must be arithmetic, not per-skipped-packet iteration"
    );
}
