//! Reboot-safe stream sequence tracking (`SequenceTracker`).
//!
//! The board numbers every stream frame with a `u16` sequence (`+1` per
//! packet, wrapping `0xFFFF → 0`). This tracker dedups retransmissions,
//! counts gaps (skipped sequences) and — the C# P3 lesson — rebaselines
//! after a board reboot instead of counting the whole post-reboot stream
//! as duplicates.
//!
//! # P3 regression
//!
//! The C# tracker never updated its baseline in the `delta <= 0` branch,
//! so after a board reboot (sequence restarts near 0) the ~1000 subsequent
//! valid packets were all counted as duplicates. The test
//! `p3_reboot_does_not_count_valid_packets_as_duplicates` reproduces that
//! scenario first (TDD: it was written before the implementation and failed
//! against the buggy baseline) and now guards the fix.

use std::collections::VecDeque;

use rsh_protocol::StatusPacket;

/// Backward jump (in sequence numbers) larger than this is treated as a
/// board reboot, not as a burst of duplicates. The C# tracker used 32.
pub const REBASELINE_THRESHOLD: u16 = 32;

/// Number of recent packet outcomes kept for the sliding-window loss
/// percentage (SW-R3.2, D8 — the reset-based method is gone).
pub const LOSS_WINDOW: usize = 1000;

/// Decision for one incoming sequence number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeqDecision {
    /// New in-order packet (includes the first packet seen and the first
    /// packet after a reboot rebaseline).
    Accept,
    /// Same sequence as, or an old sequence within the rebaseline
    /// threshold of, the last accepted one.
    Duplicate,
    /// New packet, but `n` sequences were skipped before it.
    Gap(u16),
}

/// Tracks stream sequence numbers: dedup, gap counting, reboot rebaseline,
/// board-side loss counters (`seqGap`/`txFail` from [`StatusPacket`]) and a
/// sliding-window loss percentage over the last [`LOSS_WINDOW`] packets.
#[derive(Clone, Debug, Default)]
pub struct SequenceTracker {
    /// Last accepted sequence, `None` until the first packet.
    last: Option<u16>,
    /// Duplicate packets seen.
    duplicates: u64,
    /// Sequences skipped (sum of all [`SeqDecision::Gap`] counts).
    gaps: u64,
    /// Packets accepted.
    accepted: u64,
    /// Times the baseline was reset after a reboot signature.
    rebaselines: u64,
    /// Board-reported `seqGap` accumulated across status packets
    /// (including across reboots).
    board_seq_gap: u64,
    /// Last board-reported `seqGap` value, for delta computation.
    last_board_seq_gap: Option<u16>,
    /// Board-reported `txFail` accumulated across status packets
    /// (including across reboots).
    board_tx_fail: u64,
    /// Last board-reported `txFail` value, for delta computation.
    last_board_tx_fail: Option<u16>,
    /// Recent outcomes (`true` = lost) for the loss window.
    window: VecDeque<bool>,
    /// Number of `true` entries currently in `window`.
    window_lost: usize,
}

impl SequenceTracker {
    /// Create an empty tracker (no packet seen yet).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Classify one incoming sequence number.
    ///
    /// * `+1` (with `u16` wrap) → [`SeqDecision::Accept`].
    /// * Same or old sequence, backward jump `<= REBASELINE_THRESHOLD` →
    ///   [`SeqDecision::Duplicate`].
    /// * Forward jump → [`SeqDecision::Gap`] with the skipped count.
    /// * Backward jump `> REBASELINE_THRESHOLD` → board reboot signature:
    ///   rebaseline to the new sequence and [`SeqDecision::Accept`].
    #[must_use]
    pub fn check(&mut self, seq: u16) -> SeqDecision {
        let Some(last) = self.last else {
            self.last = Some(seq);
            self.accepted += 1;
            self.record_outcome(0);
            return SeqDecision::Accept;
        };

        let delta = seq.wrapping_sub(last);
        if delta == 0 {
            self.duplicates += 1;
            SeqDecision::Duplicate
        } else if delta == 1 {
            self.last = Some(seq);
            self.accepted += 1;
            self.record_outcome(0);
            SeqDecision::Accept
        } else if delta <= u16::MAX / 2 {
            let skipped = delta - 1;
            self.gaps += u64::from(skipped);
            self.last = Some(seq);
            self.accepted += 1;
            self.record_outcome(skipped);
            SeqDecision::Gap(skipped)
        } else {
            // Backward jump (`delta` past half the `u16` range means
            // `seq < last`): how far back did it go?
            let backward = last.wrapping_sub(seq);
            if backward <= REBASELINE_THRESHOLD {
                // Small backward jump: retransmission / old packet.
                self.duplicates += 1;
                SeqDecision::Duplicate
            } else {
                // Large backward jump: board reboot signature — the
                // sequence restarted near 0. Rebaseline instead of
                // counting every subsequent valid packet as a duplicate
                // (C# P3 fix).
                self.last = Some(seq);
                self.accepted += 1;
                self.rebaselines += 1;
                self.record_outcome(0);
                SeqDecision::Accept
            }
        }
    }

    /// Ingest a board status packet's loss counters (`seqGap`/`txFail`,
    /// cumulative since boot). Deltas are accumulated; a backward jump of
    /// the board counter is a reboot signature, so the baseline resets and
    /// the new (post-reboot) value is added instead of a huge wrap delta.
    pub fn ingest_status(&mut self, status: &StatusPacket) {
        self.board_seq_gap = accumulate_board_counter(
            self.board_seq_gap,
            &mut self.last_board_seq_gap,
            status.seq_gap,
        );
        self.board_tx_fail = accumulate_board_counter(
            self.board_tx_fail,
            &mut self.last_board_tx_fail,
            status.tx_fail,
        );
    }

    /// Duplicate packets seen so far.
    #[must_use]
    pub fn duplicates(&self) -> u64 {
        self.duplicates
    }

    /// Sequences skipped so far (sum of all gap counts).
    #[must_use]
    pub fn gaps(&self) -> u64 {
        self.gaps
    }

    /// Packets accepted so far.
    #[must_use]
    pub fn accepted(&self) -> u64 {
        self.accepted
    }

    /// Times the baseline was reset after a reboot signature.
    #[must_use]
    pub fn rebaselines(&self) -> u64 {
        self.rebaselines
    }

    /// Last accepted sequence, `None` until the first packet.
    #[must_use]
    pub fn last_seq(&self) -> Option<u16> {
        self.last
    }

    /// Board-reported `seqGap` accumulated across status packets.
    #[must_use]
    pub fn board_seq_gap(&self) -> u64 {
        self.board_seq_gap
    }

    /// Board-reported `txFail` accumulated across status packets.
    #[must_use]
    pub fn board_tx_fail(&self) -> u64 {
        self.board_tx_fail
    }

    /// Loss percentage over the last [`LOSS_WINDOW`] packet outcomes
    /// (`0.0` when nothing has been seen yet).
    #[must_use]
    pub fn loss_percent(&self) -> f64 {
        if self.window.is_empty() {
            return 0.0;
        }
        // Both counts are bounded by `LOSS_WINDOW` (1000), so the `u32`
        // conversion is exact and the `f64` conversion is lossless.
        let lost = u32::try_from(self.window_lost).unwrap_or(u32::MAX);
        let total = u32::try_from(self.window.len()).unwrap_or(u32::MAX);
        100.0 * f64::from(lost) / f64::from(total)
    }

    /// Record one accepted packet plus `lost` skipped packets before it
    /// into the sliding loss window. Duplicates are not recorded: they are
    /// retransmissions, not stream losses.
    fn record_outcome(&mut self, lost: u16) {
        if usize::from(lost) >= LOSS_WINDOW {
            // The whole window is losses; the accepted packet is the only
            // survivor and is pushed below.
            self.window.clear();
            self.window_lost = 0;
            self.window.resize(LOSS_WINDOW - 1, true);
            self.window_lost = LOSS_WINDOW - 1;
        } else {
            for _ in 0..lost {
                self.window.push_back(true);
                self.window_lost += 1;
            }
        }
        self.window.push_back(false);
        while self.window.len() > LOSS_WINDOW {
            if self.window.pop_front() == Some(true) {
                self.window_lost -= 1;
            }
        }
    }
}

/// Accumulate one board-side `u16` counter (cumulative since boot) into a
/// running `u64` total. First sighting seeds the total with the value
/// itself; later sightings add the wrapping delta. A backward jump larger
/// than half the `u16` range is a reboot signature: the counter restarted,
/// so the new value is added and the baseline resets.
fn accumulate_board_counter(total: u64, last: &mut Option<u16>, value: u16) -> u64 {
    match *last {
        None => {
            *last = Some(value);
            u64::from(value)
        }
        Some(prev) => {
            let delta = value.wrapping_sub(prev);
            *last = Some(value);
            if delta <= u16::MAX / 2 {
                total + u64::from(delta)
            } else {
                // Reboot: board counter restarted near 0.
                total + u64::from(value)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{REBASELINE_THRESHOLD, SeqDecision, SequenceTracker};
    use rsh_protocol::StatusPacket;

    /// Build a status packet carrying only the loss counters we care about.
    fn status(seq_gap: u16, tx_fail: u16) -> StatusPacket {
        StatusPacket {
            seq: 0,
            uptime_ms: 0,
            rssi: 0,
            battery_pct: 255,
            supply_mv: 0,
            heap: 0,
            overruns: 0,
            max_cycle_us: 0,
            governor: 0,
            reset_reason: 1,
            cfg_crc: 0,
            cfg_ver: 0,
            seq_gap,
            tx_fail,
            btns_ex: None,
        }
    }

    #[test]
    fn p3_reboot_does_not_count_valid_packets_as_duplicates() {
        // C# P3 regression: board reboots, sequence restarts near 0, and
        // the old tracker counted ~1000 subsequent VALID packets as
        // duplicates because the delta<=0 branch never updated the
        // baseline. Zero of the post-reboot valid packets may be
        // duplicates.
        let mut tracker = SequenceTracker::new();
        assert_eq!(tracker.check(1000), SeqDecision::Accept);
        assert_eq!(tracker.check(1001), SeqDecision::Accept);
        // Board reboots: sequence restarts near 0.
        let mut post_reboot_duplicates = 0u64;
        for seq in 0..=999u16 {
            if tracker.check(seq) == SeqDecision::Duplicate {
                post_reboot_duplicates += 1;
            }
        }
        assert_eq!(post_reboot_duplicates, 0);
        assert_eq!(tracker.duplicates(), 0);
        assert_eq!(tracker.accepted(), 1002);
        assert_eq!(tracker.rebaselines(), 1);
    }

    #[test]
    fn in_order_packets_accepted() {
        let mut tracker = SequenceTracker::new();
        for seq in 0..100u16 {
            assert_eq!(tracker.check(seq), SeqDecision::Accept);
        }
        assert_eq!(tracker.accepted(), 100);
        assert_eq!(tracker.duplicates(), 0);
        assert_eq!(tracker.gaps(), 0);
    }

    #[test]
    fn sequence_wraps_at_u16_max() {
        let mut tracker = SequenceTracker::new();
        assert_eq!(tracker.check(u16::MAX - 1), SeqDecision::Accept);
        assert_eq!(tracker.check(u16::MAX), SeqDecision::Accept);
        assert_eq!(tracker.check(0), SeqDecision::Accept);
        assert_eq!(tracker.check(1), SeqDecision::Accept);
        assert_eq!(tracker.duplicates(), 0);
        assert_eq!(tracker.gaps(), 0);
    }

    #[test]
    fn same_sequence_is_duplicate() {
        let mut tracker = SequenceTracker::new();
        assert_eq!(tracker.check(42), SeqDecision::Accept);
        assert_eq!(tracker.check(42), SeqDecision::Duplicate);
        assert_eq!(tracker.check(42), SeqDecision::Duplicate);
        assert_eq!(tracker.duplicates(), 2);
        assert_eq!(tracker.accepted(), 1);
        assert_eq!(tracker.last_seq(), Some(42));
    }

    #[test]
    fn old_sequence_within_threshold_is_duplicate() {
        let mut tracker = SequenceTracker::new();
        assert_eq!(tracker.check(100), SeqDecision::Accept);
        // One step back: retransmission, not a reboot.
        assert_eq!(tracker.check(99), SeqDecision::Duplicate);
        assert_eq!(tracker.check(100), SeqDecision::Duplicate);
        assert_eq!(tracker.duplicates(), 2);
        assert_eq!(tracker.rebaselines(), 0);
    }

    #[test]
    fn rebaseline_threshold_boundary() {
        let mut tracker = SequenceTracker::new();
        assert_eq!(tracker.check(1000), SeqDecision::Accept);
        // Exactly the threshold back: still a duplicate.
        assert_eq!(
            tracker.check(1000 - REBASELINE_THRESHOLD),
            SeqDecision::Duplicate
        );
        // One past the threshold: reboot signature, rebaseline + accept.
        assert_eq!(
            tracker.check(1000 - REBASELINE_THRESHOLD - 1),
            SeqDecision::Accept
        );
        assert_eq!(tracker.rebaselines(), 1);
        assert_eq!(tracker.duplicates(), 1);
    }

    #[test]
    fn gap_counts_skipped_sequences() {
        let mut tracker = SequenceTracker::new();
        assert_eq!(tracker.check(10), SeqDecision::Accept);
        assert_eq!(tracker.check(15), SeqDecision::Gap(4));
        assert_eq!(tracker.gaps(), 4);
        assert_eq!(tracker.accepted(), 2);
        assert_eq!(tracker.check(16), SeqDecision::Accept);
    }

    #[test]
    fn ingest_status_accumulates_board_counters() {
        let mut tracker = SequenceTracker::new();
        tracker.ingest_status(&status(3, 1));
        assert_eq!(tracker.board_seq_gap(), 3);
        assert_eq!(tracker.board_tx_fail(), 1);
        tracker.ingest_status(&status(5, 4));
        assert_eq!(tracker.board_seq_gap(), 5);
        assert_eq!(tracker.board_tx_fail(), 4);
    }

    #[test]
    fn ingest_status_survives_board_reboot() {
        let mut tracker = SequenceTracker::new();
        tracker.ingest_status(&status(500, 20));
        // Board reboots: counters restart near 0.
        tracker.ingest_status(&status(2, 1));
        assert_eq!(tracker.board_seq_gap(), 502);
        assert_eq!(tracker.board_tx_fail(), 21);
    }

    #[test]
    fn loss_percent_sliding_window() {
        let mut tracker = SequenceTracker::new();
        assert_eq!(tracker.loss_percent(), 0.0);
        // 10 accepted, then a gap of 10 skipped (10 lost of 21 outcomes).
        for seq in 0..10u16 {
            assert_eq!(tracker.check(seq), SeqDecision::Accept);
        }
        assert_eq!(tracker.check(20), SeqDecision::Gap(10));
        let pct = tracker.loss_percent();
        assert!((pct - 100.0 * 10.0 / 21.0).abs() < f64::EPSILON);
        // Duplicates do not enter the window.
        assert_eq!(tracker.check(20), SeqDecision::Duplicate);
        assert!((tracker.loss_percent() - pct).abs() < f64::EPSILON);
    }
}
