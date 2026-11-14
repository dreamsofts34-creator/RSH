//! NTP-style ping tracker (client side, §9 / SW-R2.5).
//!
//! No transport lives here: the engine feeds reply frames in via
//! [`PingTracker::on_reply`] and reads the latest measurement out. The
//! tracker owns one outstanding round at a time; a reply whose `seq`
//! does not match the outstanding request is stale and ignored, and a
//! round with no reply inside 500 ms is dropped with no retry — the E2E
//! metric then stays "unmeasured" instead of reporting a fake value.
//!
//! Delay/offset math lives in [`rsh_protocol::ping::measure`] (same
//! formulas as the C# series, §9.2); this module only tracks rounds.

use rsh_protocol::ping::{PING_TIMEOUT_MS, PingFrame, PingMeasurement, measure};

/// Timeout for one ping round in µs — 500 ms, no retry (§9.5).
pub const PING_TIMEOUT_US: u64 = PING_TIMEOUT_MS * 1000;

/// NTP-style ping tracker: sends requests, matches replies, measures
/// delay/offset.
///
/// One round at a time: [`PingTracker::next_request`] supersedes any
/// round still in flight, and a late reply is stale, never a
/// measurement.
#[derive(Clone, Copy, Debug, Default)]
pub struct PingTracker {
    /// Sequence for the next request (+1 per ping, wraps at `u16::MAX`).
    next_seq: u16,
    /// The request still waiting for a reply, if any.
    outstanding: Option<PingFrame>,
    /// Measurement from the most recent accepted reply.
    last_measurement: Option<PingMeasurement>,
    /// Rounds dropped because no reply arrived within the timeout.
    timeouts: u64,
    /// Replies ignored because they were invalid (wrong `t0`, bad
    /// timestamps, wrong type).
    invalid_replies: u64,
}

impl PingTracker {
    /// Create a tracker with no outstanding round and no measurement.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            next_seq: 0,
            outstanding: None,
            last_measurement: None,
            timeouts: 0,
            invalid_replies: 0,
        }
    }

    /// Build the next ping request for `now_us` (PC monotonic µs).
    ///
    /// Sequence is +1 per call and wraps past `u16::MAX`; `t1`/`t2`
    /// are 0 (the board fills them in). If a previous round is still
    /// outstanding it is superseded: its reply, if it ever arrives, is
    /// stale and ignored.
    #[must_use]
    pub fn next_request(&mut self, now_us: u64) -> PingFrame {
        let frame = PingFrame::request(self.next_seq, now_us);
        self.next_seq = self.next_seq.wrapping_add(1);
        self.outstanding = Some(frame);
        frame
    }

    /// Feed a reply frame received at `now_us` (PC monotonic µs, `t3`).
    ///
    /// Returns the measurement when the reply matches the outstanding
    /// request and its timestamps are valid; otherwise returns [`None`]
    /// and the reply is ignored — unknown/stale `seq`, a reply later
    /// than the 500 ms timeout (counted as a timeout), or invalid
    /// timestamps (counted, never fed into a metric).
    #[must_use]
    pub fn on_reply(&mut self, frame: PingFrame, now_us: u64) -> Option<PingMeasurement> {
        let request = self.outstanding?;
        if frame.seq != request.seq {
            return None;
        }
        // Late reply: the round already timed out — drop it, count it.
        if now_us.saturating_sub(request.t0) > PING_TIMEOUT_US {
            self.outstanding = None;
            self.timeouts += 1;
            return None;
        }
        if let Ok(measurement) = measure(&request, &frame, now_us) {
            self.outstanding = None;
            self.last_measurement = Some(measurement);
            Some(measurement)
        } else {
            // The board answered, but badly: the round is over and
            // no valid reply will follow for this `seq`.
            self.outstanding = None;
            self.invalid_replies += 1;
            None
        }
    }

    /// Drop the outstanding round if no reply arrived within 500 ms.
    ///
    /// Returns `true` when a round was dropped (the E2E metric stays
    /// "unmeasured"); never panics, never retries. Poll this on the
    /// ping cadence (1 Hz); a reply arriving after the timeout is stale
    /// and ignored by [`PingTracker::on_reply`].
    #[must_use]
    pub fn check_timeout(&mut self, now_us: u64) -> bool {
        let Some(request) = self.outstanding else {
            return false;
        };
        if now_us.saturating_sub(request.t0) > PING_TIMEOUT_US {
            self.outstanding = None;
            self.timeouts += 1;
            true
        } else {
            false
        }
    }

    /// Measurement from the most recent accepted reply, if any.
    #[must_use]
    pub const fn last_measurement(&self) -> Option<PingMeasurement> {
        self.last_measurement
    }

    /// E2E delay (µs) from the most recent accepted reply, [`None`]
    /// while unmeasured.
    #[must_use]
    pub const fn last_delay_us(&self) -> Option<u64> {
        match self.last_measurement {
            Some(measurement) => Some(measurement.delay_us),
            None => None,
        }
    }

    /// Sequence of the outstanding request, if a round is in flight.
    #[must_use]
    pub const fn outstanding_seq(&self) -> Option<u16> {
        match self.outstanding {
            Some(frame) => Some(frame.seq),
            None => None,
        }
    }

    /// Number of rounds dropped by timeout.
    #[must_use]
    pub const fn timeouts(&self) -> u64 {
        self.timeouts
    }

    /// Number of replies ignored for invalid timestamps/type.
    #[must_use]
    pub const fn invalid_replies(&self) -> u64 {
        self.invalid_replies
    }
}

#[cfg(test)]
mod tests {
    use super::{PING_TIMEOUT_US, PingTracker};
    use rsh_protocol::ping::{PingFrame, PingType};

    #[test]
    fn request_seq_increments_and_wraps() {
        let mut tracker = PingTracker::new();
        // Every `seq` from 0 to `u16::MAX`, then wrap back to 0.
        for expected in 0..=u16::MAX {
            let frame = tracker.next_request(u64::from(expected) * 1000);
            assert_eq!(frame.seq, expected);
            assert_eq!(frame.frame_type, PingType::Request);
            assert_eq!(frame.t0, u64::from(expected) * 1000);
            assert_eq!(frame.t1, 0);
            assert_eq!(frame.t2, 0);
        }
        let wrapped = tracker.next_request(70_000_000);
        assert_eq!(wrapped.seq, 0);
    }

    #[test]
    fn delay_and_offset_formulas() {
        // t0=1000, t1=1100, t2=1150, t3=1300 (µs).
        let mut tracker = PingTracker::new();
        let request = tracker.next_request(1000);
        assert_eq!(request.seq, 0);
        let reply = PingFrame::reply(0, 1000, 1100, 1150);
        let measurement = tracker.on_reply(reply, 1300).expect("valid reply");
        assert_eq!(measurement.delay_us, 250); // (1300-1000) - (1150-1100)
        assert_eq!(measurement.board_processing_us, 50);
        assert_eq!(measurement.offset_us, -25); // ((1100-1000) + (1150-1300)) / 2
        assert_eq!(tracker.last_measurement(), Some(measurement));
        assert_eq!(tracker.last_delay_us(), Some(250));
        assert_eq!(tracker.outstanding_seq(), None);
    }

    #[test]
    fn stale_seq_ignored_then_correct_reply_accepted() {
        let mut tracker = PingTracker::new();
        let _ = tracker.next_request(1000);
        let stale = PingFrame::reply(5, 1000, 1100, 1150);
        assert_eq!(tracker.on_reply(stale, 1300), None);
        assert_eq!(tracker.last_measurement(), None);
        // The real reply for the outstanding round still works.
        let reply = PingFrame::reply(0, 1000, 1100, 1150);
        assert!(tracker.on_reply(reply, 1300).is_some());
    }

    #[test]
    fn reply_without_outstanding_ignored() {
        let mut tracker = PingTracker::new();
        let reply = PingFrame::reply(0, 1000, 1100, 1150);
        assert_eq!(tracker.on_reply(reply, 1300), None);
        assert_eq!(tracker.last_measurement(), None);
    }

    #[test]
    fn timeout_drops_round_and_stays_unmeasured() {
        let mut tracker = PingTracker::new();
        let _ = tracker.next_request(0);
        assert!(!tracker.check_timeout(PING_TIMEOUT_US - 1));
        assert_eq!(tracker.outstanding_seq(), Some(0));
        assert!(tracker.check_timeout(PING_TIMEOUT_US + 1));
        assert_eq!(tracker.timeouts(), 1);
        assert_eq!(tracker.last_measurement(), None);
        assert_eq!(tracker.last_delay_us(), None);
        assert_eq!(tracker.outstanding_seq(), None);
        // A late reply for the dropped round is stale, never measured.
        let late = PingFrame::reply(0, 0, 100, 150);
        assert_eq!(tracker.on_reply(late, 600_000), None);
        assert_eq!(tracker.last_measurement(), None);
        // Polling again with nothing outstanding is a no-op, no panic.
        assert!(!tracker.check_timeout(900_000));
        assert_eq!(tracker.timeouts(), 1);
    }

    #[test]
    fn reply_at_timeout_boundary_accepted() {
        let mut tracker = PingTracker::new();
        let _ = tracker.next_request(1000);
        // Exactly 500 ms after `t0`: still within the timeout.
        let reply = PingFrame::reply(0, 1000, 2000, 2100);
        let measurement = tracker
            .on_reply(reply, 1000 + PING_TIMEOUT_US)
            .expect("boundary reply accepted");
        assert_eq!(measurement.delay_us, PING_TIMEOUT_US - 100);
    }

    #[test]
    fn reply_one_us_past_timeout_dropped() {
        let mut tracker = PingTracker::new();
        let _ = tracker.next_request(1000);
        let reply = PingFrame::reply(0, 1000, 2000, 2100);
        assert_eq!(tracker.on_reply(reply, 1000 + PING_TIMEOUT_US + 1), None);
        assert_eq!(tracker.timeouts(), 1);
        assert_eq!(tracker.last_measurement(), None);
    }

    #[test]
    fn invalid_reply_counted_not_measured() {
        let mut tracker = PingTracker::new();
        let _ = tracker.next_request(1000);
        // `t2 < t1` — impossible board timestamps.
        let bad = PingFrame::reply(0, 1000, 1200, 1100);
        assert_eq!(tracker.on_reply(bad, 1300), None);
        assert_eq!(tracker.invalid_replies(), 1);
        assert_eq!(tracker.last_measurement(), None);
        // The round is over; a second (valid) reply for it is stale.
        let good = PingFrame::reply(0, 1000, 1100, 1150);
        assert_eq!(tracker.on_reply(good, 1300), None);
    }

    #[test]
    fn second_round_after_first() {
        let mut tracker = PingTracker::new();
        let _ = tracker.next_request(1000);
        let first = PingFrame::reply(0, 1000, 1100, 1150);
        assert_eq!(
            tracker.on_reply(first, 1300).expect("first reply").delay_us,
            250
        );
        let second = tracker.next_request(2_000_000);
        assert_eq!(second.seq, 1);
        let reply = PingFrame::reply(1, 2_000_000, 2_000_100, 2_000_200);
        let measurement = tracker.on_reply(reply, 2_000_500).expect("second reply");
        assert_eq!(measurement.delay_us, 400); // (500) - (100)
        assert_eq!(tracker.last_delay_us(), Some(400));
    }

    #[test]
    fn superseded_round_reply_is_stale() {
        let mut tracker = PingTracker::new();
        let _ = tracker.next_request(1000);
        let _ = tracker.next_request(2_000_000); // seq 1 supersedes seq 0
        let old_reply = PingFrame::reply(0, 1000, 1100, 1150);
        assert_eq!(tracker.on_reply(old_reply, 2_000_300), None);
        assert_eq!(tracker.last_measurement(), None);
    }
}
