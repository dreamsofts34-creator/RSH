//! App-core snapshot API (Phase F, E5 contract gap).
//!
//! Maps the engine's [`EngineMetrics`] into the frontend `EngineStats`
//! contract (`frontend/src/lib/tauri.ts`) and builds the 30 Hz
//! [`EngineSnapshot`] the UI plan §6.1 `rsh://snapshot` event carries.
//!
//! Conversion rules (the contract — never a fake 0):
//! - p99 latencies are microseconds in the engine, milliseconds in the
//!   frontend: µs / 1000. `None` (unmeasured) stays `null`.
//! - Packet loss is a fraction (0.0–1.0) in the engine, percent (0–100)
//!   in the frontend. A loss of 0 % is real and shown as `0.0`, never
//!   as "unmeasured".
//! - `loopHz` has no source in [`EngineMetrics`], so it is measured here
//!   by [`LoopRateTracker`]: engine ticks counted over a sliding 1 s
//!   window. Fewer than 2 ticks in the window means "unmeasured"
//!   (`None`) — a single tick cannot define a rate, and a constant
//!   (e.g. a hardcoded 200) would be fake data.
//! - v2.0.19: encoders are deleted (firmware sends zeros, nothing
//!   reads them) and the H-pattern gear is deleted — the snapshot
//!   carries steering/pedals/handbrake only, never a gear.
//!
//! The snapshot event name is [`SNAPSHOT_EVENT`]. Emission itself is the
//! Tauri shell's job (`src-tauri/src/main.rs` emits it from the engine
//! task); this module only builds the payload, so it stays testable
//! without a Tauri runtime.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use rsh_engine::metrics::EngineMetrics;
use serde::{Deserialize, Serialize};

/// Event name for the 30 Hz engine snapshot feed (UI plan §6.1).
pub const SNAPSHOT_EVENT: &str = "rsh://snapshot";

/// Length of the sliding window [`LoopRateTracker`] measures over.
pub const LOOP_RATE_WINDOW: Duration = Duration::from_secs(1);

/// Microseconds in one millisecond (the engine→frontend unit step).
const US_PER_MS: f64 = 1000.0;

/// Convert a microsecond p99 value to milliseconds for the frontend.
///
/// The `u64 → f64` cast cannot lose precision for real latencies: p99
/// values are engine timings (µs, far below 2^53 µs ≈ 285 years).
#[allow(clippy::cast_precision_loss)]
#[must_use]
pub fn us_to_ms(value_us: u64) -> f64 {
    value_us as f64 / US_PER_MS
}

/// Convert a packet-loss fraction (0.0–1.0) to percent (0–100).
#[must_use]
pub fn loss_fraction_to_pct(fraction: f64) -> f64 {
    fraction * 100.0
}

/// Engine stats as the frontend consumes them (`EngineStats` in
/// `frontend/src/lib/tauri.ts`).
///
/// Nullable fields are `null` while unmeasured — never a fake 0. Only
/// `loss_pct` is always a number: 0 % loss is a real, measured value.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EngineStats {
    /// Engine loop rate in Hz (target 200); `None` until ticks are measured.
    pub loop_hz: Option<f64>,
    /// Packet loss in percent, sliding window.
    pub loss_pct: f64,
    /// Inter-arrival jitter p99 in milliseconds.
    pub jitter_ms_p99: Option<f64>,
    /// Output latency p99 in milliseconds.
    pub output_latency_ms: Option<f64>,
    /// End-to-end latency p99 in milliseconds (ping-derived).
    pub e2e_ms: Option<f64>,
}


/// Pedal positions in a snapshot, 0..1 (`clutch` is `None` when the
/// clutch module is not present).
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PedalSnapshot {
    /// Accelerator position, 0..1.
    pub accel: f64,
    /// Brake position, 0..1.
    pub brake: f64,
    /// Clutch position, 0..1, or `None` when there is no clutch module.
    pub clutch: Option<f64>,
}

/// Connection state strings, matching the frontend `ConnectionState` union.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionState {
    /// No transport connected.
    #[default]
    Disconnected,
    /// A transport connect is in progress.
    Connecting,
    /// Transport connected.
    Connected,
    /// Transport in an error state.
    Error,
}

impl ConnectionState {
    /// The wire string the frontend matches on (`"connected"`, ...).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Disconnected => "disconnected",
            Self::Connecting => "connecting",
            Self::Connected => "connected",
            Self::Error => "error",
        }
    }
}

/// One 30 Hz engine snapshot (UI plan §6.1, event `rsh://snapshot`).
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EngineSnapshot {
    /// Steering angle in degrees, -450..450.
    pub steering_deg: f64,
    /// Pedal positions 0..1.
    pub pedals: PedalSnapshot,
    /// Transport connection state.
    pub connection: ConnectionState,
    /// Engine stats (see [`EngineStats`]).
    pub stats: EngineStats,
    /// Active profile id, or `None` when no profile is active.
    pub active_profile_id: Option<String>,
    /// Monotonic sequence number; gaps mean dropped snapshots.
    pub seq: u64,
    /// `seqGap` counter passed through from the latest status packet.
    pub seq_gap: u64,
    /// `txFail` counter passed through from the latest status packet.
    pub tx_fail: u64,
    /// Handbrake position 0..1 (M-3, v2.0.2): the pipeline's handbrake
    /// axis this tick. `0.0` = released; never faked — the pipeline
    /// reports the real axis, and no frame yet means `0.0`.
    pub handbrake: f64,
}

/// Measures the engine tick rate over a sliding 1 s window.
///
/// Feed it from the engine tick (200 Hz nominal). The rate is the number
/// of ticks inside the last [`LOOP_RATE_WINDOW`]; fewer than 2 ticks
/// means the rate is unmeasured (`None`). The window slides, so a stall
/// decays the rate instead of freezing a stale value — and a burst of
/// ticks older than 1 s never inflates it.
///
/// `tick_at` / `rate_hz_at` take an explicit clock so tests can prove
/// the value is measured from real tick spacing, not a constant.
#[derive(Clone, Debug, Default)]
pub struct LoopRateTracker {
    /// Tick instants inside the current window, oldest first.
    ticks: VecDeque<Instant>,
}

impl LoopRateTracker {
    /// An empty tracker (rate unmeasured).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one engine tick at `now`.
    pub fn tick_at(&mut self, now: Instant) {
        self.ticks.push_back(now);
        self.prune(now);
    }

    /// Record one engine tick at the current time.
    pub fn tick(&mut self) {
        self.tick_at(Instant::now());
    }

    /// The measured loop rate in Hz at `now`, or `None` when fewer than
    /// 2 ticks are inside the window.
    ///
    /// The window is half-open (L1): a tick exactly 1 s old is
    /// *outside* the window, so 201 ticks at 5 ms over `[0, 1000]`
    /// measure 200 Hz, not 201 — the old `<=` counted both endpoints
    /// of a full second. The `usize → f64` cast cannot lose precision
    /// here: the count is the number of engine ticks inside a 1 s
    /// window (hundreds at most), far below 2^53.
    #[allow(clippy::cast_precision_loss)]
    #[must_use]
    pub fn rate_hz_at(&self, now: Instant) -> Option<f64> {
        let mut count = 0_usize;
        for &tick in &self.ticks {
            // A tick stamped after `now` (clock misuse) counts as
            // in-window rather than panicking `duration_since`.
            let age = now.checked_duration_since(tick).unwrap_or(Duration::ZERO);
            if age < LOOP_RATE_WINDOW {
                count += 1;
            }
        }
        if count < 2 { None } else { Some(count as f64) }
    }

    /// The measured loop rate in Hz right now.
    ///
    /// Deliberately NOT cached (PERF P1-3 follow-up): a time-based
    /// cache served the value computed before the latest ticks for
    /// up to 100 ms — after two ticks `rate_hz()` still answered
    /// the pre-tick `None` (the `ingest_…_loop_rate` test caught
    /// it). The scan is a ~200 ns linear pass over at most 200
    /// `Instant`s; the real P1-3 win is the p99 cache in
    /// `EngineMetrics`, kept. A cache here would have to invalidate
    /// on every tick, which is exactly the per-tick call pattern —
    /// so it could never hit anyway.
    #[must_use]
    pub fn rate_hz(&self) -> Option<f64> {
        self.rate_hz_at(Instant::now())
    }

    /// Ticks currently inside the window (test/debug aid).
    #[must_use]
    pub fn ticks_in_window(&self) -> usize {
        self.ticks.len()
    }

    /// Drop ticks older than the window relative to `now`. The window
    /// is half-open (L1): a tick exactly 1 s old is pruned.
    fn prune(&mut self, now: Instant) {
        while let Some(&oldest) = self.ticks.front() {
            let age = now.checked_duration_since(oldest).unwrap_or(Duration::ZERO);
            if age >= LOOP_RATE_WINDOW {
                self.ticks.pop_front();
            } else {
                break;
            }
        }
    }
}



/// Map [`EngineMetrics`] to the frontend [`EngineStats`] contract.
///
/// `loop_hz` comes from [`LoopRateTracker::rate_hz`] — pass `None` when
/// the tracker has not measured a rate yet. µs→ms, `None` p99→`null`,
/// loss fraction→percent.
#[must_use]
pub fn build_engine_stats(metrics: &EngineMetrics, loop_hz: Option<f64>) -> EngineStats {
    EngineStats {
        loop_hz,
        loss_pct: loss_fraction_to_pct(metrics.loss_fraction()),
        jitter_ms_p99: metrics.jitter_p99().map(us_to_ms),
        output_latency_ms: metrics.output_latency_p99().map(us_to_ms),
        e2e_ms: metrics.e2e_latency_p99().map(us_to_ms),
    }
}

/// Hot-path variant of [`build_engine_stats`] (PERF P1-3): the p99s
/// come from the metrics' 100 ms cache instead of three fresh sorts,
/// so the 200 Hz `ingest_frame` tick pays ~0.1 µs of stats work
/// instead of ~14 µs. The 1 Hz diag path keeps using the exact
/// `build_engine_stats`.
#[must_use]
pub fn build_engine_stats_cached(metrics: &mut EngineMetrics, loop_hz: Option<f64>) -> EngineStats {
    let (jitter, output, e2e) = metrics.cached_p99s();
    EngineStats {
        loop_hz,
        loss_pct: loss_fraction_to_pct(metrics.loss_fraction()),
        jitter_ms_p99: jitter.map(us_to_ms),
        output_latency_ms: output.map(us_to_ms),
        e2e_ms: e2e.map(us_to_ms),
    }
}

/// Inputs for one [`EngineSnapshot`]. The caller owns the live values
/// (steering/pedals come from the pipeline output, stats from
/// [`build_engine_stats`]).
#[derive(Clone, Debug, Default)]
pub struct SnapshotInput {
    /// Steering angle in degrees, -450..450.
    pub steering_deg: f64,
    /// Pedal positions 0..1.
    pub pedals: PedalSnapshot,
    /// Transport connection state.
    pub connection: ConnectionState,
    /// Engine stats for this snapshot.
    pub stats: EngineStats,
    /// Active profile id, or `None` when no profile is active.
    pub active_profile_id: Option<String>,
    /// Monotonic sequence number.
    pub seq: u64,
    /// `seqGap` counter from the latest status packet.
    pub seq_gap: u64,
    /// `txFail` counter from the latest status packet.
    pub tx_fail: u64,
    /// Handbrake position 0..1 (pipeline output this tick).
    pub handbrake: f64,
}

/// Build one [`EngineSnapshot`] from its inputs.
#[must_use]
pub fn build_snapshot(input: SnapshotInput) -> EngineSnapshot {
    EngineSnapshot {
        steering_deg: input.steering_deg,
        pedals: input.pedals,
        connection: input.connection,
        stats: input.stats,
        active_profile_id: input.active_profile_id,
        seq: input.seq,
        seq_gap: input.seq_gap,
        tx_fail: input.tx_fail,
        handbrake: input.handbrake,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ConnectionState, EngineSnapshot, LoopRateTracker, PedalSnapshot, SnapshotInput,
        build_engine_stats, build_snapshot, loss_fraction_to_pct, us_to_ms,
    };
    use rsh_engine::metrics::EngineMetrics;
    use std::time::{Duration, Instant};

    #[test]
    fn us_to_ms_conversion() {
        assert_eq!(us_to_ms(0), 0.0);
        assert_eq!(us_to_ms(1000), 1.0);
        assert_eq!(us_to_ms(3400), 3.4);
        assert_eq!(us_to_ms(4800), 4.8);
        assert_eq!(us_to_ms(250), 0.25);
    }

    #[test]
    fn loss_fraction_to_percent() {
        assert_eq!(loss_fraction_to_pct(0.0), 0.0);
        assert_eq!(loss_fraction_to_pct(0.005), 0.5);
        assert_eq!(loss_fraction_to_pct(0.1), 10.0);
        assert_eq!(loss_fraction_to_pct(1.0), 100.0);
    }

    #[test]
    fn mapping_empty_metrics_is_all_null_except_loss() {
        let metrics = EngineMetrics::new();
        let stats = build_engine_stats(&metrics, None);
        assert_eq!(stats.loop_hz, None);
        assert_eq!(stats.loss_pct, 0.0);
        assert_eq!(stats.jitter_ms_p99, None);
        assert_eq!(stats.output_latency_ms, None);
        assert_eq!(stats.e2e_ms, None);
        let json = serde_json::to_string(&stats).expect("stats serialize");
        assert!(json.contains("\"loopHz\":null"), "{json}");
        assert!(json.contains("\"jitterMsP99\":null"), "{json}");
        assert!(json.contains("\"outputLatencyMs\":null"), "{json}");
        assert!(json.contains("\"e2eMs\":null"), "{json}");
        assert!(json.contains("\"lossPct\":0.0"), "{json}");
    }

    #[test]
    fn mapping_measured_metrics_converts_units() {
        let mut metrics = EngineMetrics::new();
        metrics.record_inter_arrival_jitter(3400);
        metrics.record_output_latency(1200);
        metrics.record_e2e_latency(4800);
        for _ in 0..99 {
            metrics.record_packet(true);
        }
        metrics.record_packet(false); // 1 lost in 100 -> 1 %
        let stats = build_engine_stats(&metrics, Some(200.0));
        assert_eq!(stats.loop_hz, Some(200.0));
        assert_eq!(stats.loss_pct, 1.0);
        assert_eq!(stats.jitter_ms_p99, Some(3.4));
        assert_eq!(stats.output_latency_ms, Some(1.2));
        assert_eq!(stats.e2e_ms, Some(4.8));
        let json = serde_json::to_string(&stats).expect("stats serialize");
        assert!(json.contains("\"loopHz\":200.0"), "{json}");
        assert!(json.contains("\"lossPct\":1.0"), "{json}");
        assert!(json.contains("\"jitterMsP99\":3.4"), "{json}");
    }

    #[test]
    fn loop_rate_empty_and_single_tick_are_unmeasured() {
        let tracker = LoopRateTracker::new();
        let now = Instant::now();
        assert_eq!(tracker.rate_hz_at(now), None);
        let mut tracker = LoopRateTracker::new();
        tracker.tick_at(now);
        assert_eq!(tracker.rate_hz_at(now), None);
        assert_eq!(tracker.ticks_in_window(), 1);
    }

    #[test]
    fn loop_rate_measured_from_simulated_200hz_ticks() {
        // 200 ticks at exactly 5 ms spacing: a real measured 200 Hz,
        // not a constant — the value comes from the tick timestamps.
        let mut tracker = LoopRateTracker::new();
        let start = Instant::now();
        for i in 0..200_u32 {
            tracker.tick_at(start + Duration::from_millis(5) * i);
        }
        let end = start + Duration::from_millis(5) * 199;
        assert_eq!(tracker.rate_hz_at(end), Some(200.0));
        // 100 Hz ticks measure 100 Hz: the tracker follows the input.
        let mut slow = LoopRateTracker::new();
        for i in 0..100_u32 {
            slow.tick_at(start + Duration::from_millis(10) * i);
        }
        let slow_end = start + Duration::from_millis(10) * 99;
        assert_eq!(slow.rate_hz_at(slow_end), Some(100.0));
    }

    #[test]
    fn loop_rate_window_slides_and_stall_decays() {
        let mut tracker = LoopRateTracker::new();
        let start = Instant::now();
        for i in 0..200_u32 {
            tracker.tick_at(start + Duration::from_millis(5) * i);
        }
        // 1.5 s after the last tick the window is empty: unmeasured,
        // not a frozen stale 200.
        let later = start + Duration::from_millis(5) * 199 + Duration::from_millis(1500);
        assert_eq!(tracker.rate_hz_at(later), None);
        // A burst 2 s ago plus a fresh pair measures only the fresh pair.
        let mut tracker = LoopRateTracker::new();
        for i in 0..200_u32 {
            tracker.tick_at(start + Duration::from_millis(5) * i);
        }
        let fresh = start + Duration::from_secs(2);
        tracker.tick_at(fresh);
        tracker.tick_at(fresh + Duration::from_millis(5));
        assert_eq!(
            tracker.rate_hz_at(fresh + Duration::from_millis(5)),
            Some(2.0)
        );
    }

    #[test]
    fn loop_rate_full_second_counts_200_not_201() {
        // L1: 201 ticks at 5 ms span exactly [0, 1000] ms. The window
        // is half-open, so the tick at t=0 is outside at t=1000 ms
        // and the rate is 200 Hz — the old `<=` counted both
        // endpoints and reported 201.
        let mut tracker = LoopRateTracker::new();
        let start = Instant::now();
        for i in 0..201_u32 {
            tracker.tick_at(start + Duration::from_millis(5) * i);
        }
        let end = start + Duration::from_secs(1);
        assert_eq!(tracker.rate_hz_at(end), Some(200.0));
    }

    #[test]
    fn snapshot_build_serializes() {
        let metrics = EngineMetrics::new();
        let stats = build_engine_stats(&metrics, Some(200.0));
        let snapshot = build_snapshot(SnapshotInput {
            steering_deg: 12.5,
            pedals: PedalSnapshot {
                accel: 0.5,
                brake: 0.0,
                clutch: None,
            },
            connection: ConnectionState::Connected,
            stats,
            active_profile_id: Some("default".to_string()),
            seq: 42,
            seq_gap: 7,
            tx_fail: 3,
            handbrake: 0.0,
        });
        assert_eq!(snapshot.seq, 42);
        let json = serde_json::to_string(&snapshot).expect("snapshot serialize");
        assert!(json.contains("\"steeringDeg\":12.5"), "{json}");
        assert!(json.contains("\"connection\":\"connected\""), "{json}");
        assert!(json.contains("\"seqGap\":7"), "{json}");
        assert!(json.contains("\"txFail\":3"), "{json}");
        // Round-trip through the frontend shape.
        let back: EngineSnapshot = serde_json::from_str(&json).expect("snapshot round-trip");
        assert_eq!(back, snapshot);
    }

    #[test]
    fn connection_state_strings_match_frontend_union() {
        assert_eq!(ConnectionState::Connected.as_str(), "connected");
        assert_eq!(ConnectionState::Disconnected.as_str(), "disconnected");
        assert_eq!(ConnectionState::Error.as_str(), "error");
        assert_eq!(ConnectionState::Connecting.as_str(), "connecting");
    }
}
