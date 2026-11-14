//! Frame watchdog: 250 ms stale threshold (SW-R3.1).
//!
//! The engine calls [`Watchdog::tick`] on every received frame and polls
//! [`Watchdog::is_stale`]; when no frame has arrived for at least
//! [`STALE_THRESHOLD`] the engine drives outputs to NEUTRAL. Uses
//! [`std::time::Instant`] (monotonic), so wall-clock jumps cannot fake
//! freshness or staleness. Before any frame arrives the watchdog reports
//! stale — no data is stale data.

use std::time::{Duration, Instant};

/// No frame for at least this long means the stream is stale.
pub const STALE_THRESHOLD: Duration = Duration::from_millis(250);

/// Tracks the arrival time of the most recent frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct Watchdog {
    /// Arrival time of the last frame, `None` until the first frame.
    last_frame: Option<Instant>,
}

impl Watchdog {
    /// Create a watchdog with no frame seen yet (reports stale).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a frame arrival at `now`. Called on every received frame.
    pub fn tick(&mut self, now: Instant) {
        self.last_frame = Some(now);
    }

    /// `true` when no frame has arrived for at least [`STALE_THRESHOLD`],
    /// or when no frame has ever arrived.
    #[must_use]
    pub fn is_stale(&self, now: Instant) -> bool {
        self.last_frame
            .is_none_or(|last| now.saturating_duration_since(last) >= STALE_THRESHOLD)
    }

    /// Arrival time of the last frame, `None` until the first frame.
    #[must_use]
    pub fn last_tick(&self) -> Option<Instant> {
        self.last_frame
    }
}

#[cfg(test)]
mod tests {
    use super::{STALE_THRESHOLD, Watchdog};
    use std::time::{Duration, Instant};

    #[test]
    fn stale_before_any_frame() {
        let watchdog = Watchdog::new();
        assert!(watchdog.is_stale(Instant::now()));
        assert_eq!(watchdog.last_tick(), None);
    }

    #[test]
    fn fresh_right_after_tick() {
        let mut watchdog = Watchdog::new();
        let now = Instant::now();
        watchdog.tick(now);
        assert!(!watchdog.is_stale(now));
        assert_eq!(watchdog.last_tick(), Some(now));
    }

    #[test]
    fn fresh_at_100_ms_stale_after_300_ms() {
        // Tolerance-based timing test: 100 ms is clearly below and 300 ms
        // clearly above the 250 ms threshold; no exact-ms assertion.
        let mut watchdog = Watchdog::new();
        watchdog.tick(Instant::now());
        std::thread::sleep(Duration::from_millis(100));
        assert!(!watchdog.is_stale(Instant::now()));
        std::thread::sleep(Duration::from_millis(200));
        assert!(watchdog.is_stale(Instant::now()));
        assert_eq!(STALE_THRESHOLD, Duration::from_millis(250));
    }

    #[test]
    fn tick_resets_staleness() {
        let mut watchdog = Watchdog::new();
        let t0 = Instant::now();
        watchdog.tick(t0);
        assert!(watchdog.is_stale(t0 + Duration::from_millis(300)));
        watchdog.tick(t0 + Duration::from_millis(300));
        assert!(!watchdog.is_stale(t0 + Duration::from_millis(300)));
        assert!(!watchdog.is_stale(t0 + Duration::from_millis(400)));
        assert!(watchdog.is_stale(t0 + Duration::from_millis(550)));
    }

    #[test]
    fn stale_exactly_at_threshold() {
        // Deterministic check using synthetic instants, no sleeping.
        let mut watchdog = Watchdog::new();
        let t0 = Instant::now();
        watchdog.tick(t0);
        assert!(!watchdog.is_stale(t0 + Duration::from_millis(249)));
        assert!(watchdog.is_stale(t0 + STALE_THRESHOLD));
    }
}
