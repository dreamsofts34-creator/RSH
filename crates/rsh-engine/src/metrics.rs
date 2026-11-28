//! Engine metrics: p99 tracking and the loss sliding window (D8).
//!
//! [`EngineMetrics`] is the data model the Performance panel consumes
//! (via `serde`): inter-arrival jitter p99, output-latency p99, E2E
//! latency p99 (`None` = "unmeasured", never a fake 0), packet-loss
//! fraction over the last 1000 packets, and the `seqGap`/`txFail`
//! counters passed through from status packets. F4 (v2.0.21) adds
//! the Phase 0 PC-side instrumentation: raw arrival-gap p99 + max,
//! age-at-output p99, and the `frames_received` / `stale_dropped` /
//! `outputs_sent` / `frames_per_tick_max` counters.
//!
//! The record path allocates nothing: p99 uses a fixed 256-sample ring
//! buffer (sorted copy on query only) and the loss window a fixed
//! 1000-entry ring buffer.

use rsh_protocol::ping::PingMeasurement;
use serde::{Serialize, Serializer};

/// Samples kept per p99 tracker.
pub const P99_CAPACITY: usize = 256;

/// Process-local millisecond clock for the p99 cache (PERF P1-3).
fn p99_cache_now_ms() -> u64 {
    static EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let ms = EPOCH
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_millis();
    // Process uptime in ms fits u64 for ~584M years; saturate
    // rather than truncate the u128.
    u64::try_from(ms).unwrap_or(u64::MAX)
}
/// Packets in the loss sliding window (D8).
pub const LOSS_WINDOW: usize = 1000;

/// Fixed-size p99 tracker: ring buffer, no heap allocation on record.
///
/// [`Serialize`] emits the current p99 value (`null` while unmeasured),
/// not the raw sample buffer.
///
/// The sort itself stays uncached here on purpose: `Cell` is not
/// `Copy`, so a cache field would break this type's `Copy` (and
/// `EngineMetrics`'s). The caching lives one level up, in
/// [`EngineMetrics::cached_p99s`] (PERF P1-3) — the 200 Hz
/// `ingest_frame` path uses that; this direct `p99()` is the
/// slow-path / serde read.
#[derive(Clone, Copy, Debug)]
pub struct P99Tracker {
    /// Ring buffer of samples (µs).
    samples: [u64; P99_CAPACITY],
    /// Next slot to overwrite.
    head: usize,
    /// Samples recorded (≤ [`P99_CAPACITY`]).
    len: usize,
}

impl Default for P99Tracker {
    /// Empty tracker (arrays over 32 elements have no `Default` impl).
    fn default() -> Self {
        Self::new()
    }
}

impl P99Tracker {
    /// Create an empty tracker.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            samples: [0; P99_CAPACITY],
            head: 0,
            len: 0,
        }
    }

    /// Record one sample (µs). Overwrites the oldest sample when full.
    pub fn record(&mut self, value_us: u64) {
        self.samples[self.head] = value_us;
        self.head = (self.head + 1) % P99_CAPACITY;
        if self.len < P99_CAPACITY {
            self.len += 1;
        }
    }

    /// The 99th percentile of the recorded samples (nearest-rank), or
    /// [`None`] when no sample has been recorded yet.
    #[must_use]
    pub fn p99(&self) -> Option<u64> {
        if self.len == 0 {
            return None;
        }
        let mut sorted = [0u64; P99_CAPACITY];
        sorted[..self.len].copy_from_slice(&self.samples[..self.len]);
        sorted[..self.len].sort_unstable();
        // Nearest-rank p99: ceil(0.99 * n)-th smallest, 1-based.
        let rank = (99 * self.len).div_ceil(100);
        let idx = rank.saturating_sub(1).min(self.len - 1);
        Some(sorted[idx])
    }

    /// The largest sample held ([`None`] while unmeasured). A linear
    /// scan over ≤256 samples — the 1 Hz diagnostics path only, never
    /// the 200 Hz hot path.
    #[must_use]
    pub fn max(&self) -> Option<u64> {
        self.samples[..self.len].iter().max().copied()
    }

    /// Number of samples currently held.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Whether no sample has been recorded yet.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Serialize for P99Tracker {
    /// Serializes as the current p99 value (`null` while unmeasured).
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.p99().serialize(serializer)
    }
}

/// Packet-loss sliding window over the last [`LOSS_WINDOW`] packets (D8).
///
/// [`Serialize`] emits the current loss fraction, not the raw window.
#[derive(Clone, Copy, Debug)]
pub struct LossWindow {
    /// Ring buffer: `true` = packet lost.
    lost: [bool; LOSS_WINDOW],
    /// Next slot to overwrite.
    head: usize,
    /// Packets recorded (≤ [`LOSS_WINDOW`]).
    len: u32,
    /// Lost packets currently in the window.
    lost_count: u32,
}

impl Default for LossWindow {
    /// Empty window (arrays over 32 elements have no `Default` impl).
    fn default() -> Self {
        Self::new()
    }
}

impl LossWindow {
    /// Create an empty window.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            lost: [false; LOSS_WINDOW],
            head: 0,
            len: 0,
            lost_count: 0,
        }
    }

    /// Record one packet outcome: `received` is `true` when it arrived.
    pub fn record_packet(&mut self, received: bool) {
        self.record_many(u32::from(!received), u32::from(received));
    }

    /// Record a batch of outcomes at once: `lost` lost packets followed
    /// by `received` received packets (L1 — a sequence gap of `n`
    /// skipped packets is one batch, not `n` per-packet calls). The
    /// window contents are identical to recording the same outcomes
    /// one by one; the ring is filled with slice writes instead of a
    /// per-packet loop.
    pub fn record_many(&mut self, mut lost: u32, mut received: u32) {
        // Fill in two runs (lost first, then received); each run is
        // written with slice fills, evicting the oldest entries as the
        // ring wraps. Saturating arithmetic keeps the counts exact
        // when the window is already full.
        for (value, count) in [(true, &mut lost), (false, &mut received)] {
            let mut remaining = *count;
            while remaining > 0 {
                // Space to the end of the ring from `head`.
                let to_end = (LOSS_WINDOW - self.head).min(remaining as usize);
                // Evicted entries stop counting (only when full).
                if self.len as usize == LOSS_WINDOW {
                    let evicted = self.lost[self.head..self.head + to_end]
                        .iter()
                        .filter(|lost_entry| **lost_entry)
                        .count();
                    self.lost_count -= u32::try_from(evicted).unwrap_or(u32::MAX);
                } else {
                    // Window not full yet: only the part that fits
                    // before the window fills grows `len`.
                    let room = LOSS_WINDOW - self.len as usize;
                    let grown = to_end.min(room);
                    self.len += u32::try_from(grown).unwrap_or(u32::MAX);
                }
                self.lost[self.head..self.head + to_end].fill(value);
                if value {
                    self.lost_count += u32::try_from(to_end).unwrap_or(u32::MAX);
                }
                self.head = (self.head + to_end) % LOSS_WINDOW;
                remaining -= u32::try_from(to_end).unwrap_or(u32::MAX);
            }
        }
    }

    /// Loss fraction over the window (`0.0` when nothing recorded yet).
    #[must_use]
    pub fn loss_fraction(&self) -> f64 {
        if self.len == 0 {
            0.0
        } else {
            f64::from(self.lost_count) / f64::from(self.len)
        }
    }

    /// Packets currently in the window.
    #[must_use]
    pub const fn packets_tracked(&self) -> u32 {
        self.len
    }

    /// Lost packets currently in the window.
    #[must_use]
    pub const fn packets_lost(&self) -> u32 {
        self.lost_count
    }
}

impl Serialize for LossWindow {
    /// Serializes as the current loss fraction.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.loss_fraction().serialize(serializer)
    }
}

/// The five p99 values [`EngineMetrics::cached_p99s`] returns, in
/// fixed order: (inter-arrival jitter, output latency, end-to-end
/// latency, arrival gap, age at output) — all µs, `None` while
/// unmeasured.
pub type P99Set = (
    Option<u64>,
    Option<u64>,
    Option<u64>,
    Option<u64>,
    Option<u64>,
);

/// A cached [`P99Set`] plus the millisecond timestamp it was
/// computed at (`0` = never computed).
type CachedP99s = (
    u64,
    Option<u64>,
    Option<u64>,
    Option<u64>,
    Option<u64>,
    Option<u64>,
);

/// Engine metrics — the data model the Performance panel consumes.
///
/// Serializes as the current snapshot values: p99 fields are `null`
/// while unmeasured (never a fake 0).
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct EngineMetrics {
    /// Inter-arrival jitter p99 (µs).
    pub inter_arrival_jitter_p99_us: P99Tracker,
    /// Output latency p99 (µs).
    pub output_latency_p99_us: P99Tracker,
    /// E2E latency p99 (µs); `null` until the first ping reply.
    pub e2e_latency_p99_us: P99Tracker,
    /// Packet-loss fraction over the last 1000 packets (D8).
    pub packet_loss: LossWindow,
    /// `seqGap` counter passed through from the latest status packet.
    pub seq_gap: u64,
    /// `txFail` counter passed through from the latest status packet.
    pub tx_fail: u64,
    /// PC-side inter-arrival gap p99 (µs, F4 v2.0.21): the gap
    /// between successive accepted frames as the engine received
    /// them. Unlike the jitter tracker (which measures deviation
    /// from the expected period), this is the raw spacing — the
    /// number that proves whether frames arrive steadily or in
    /// bursts after Wi-Fi stalls.
    pub arrival_gap_p99_us: P99Tracker,
    /// Largest PC-side inter-arrival gap observed (µs, F4): the
    /// worst single stall. A separate scalar (not a tracker) so a
    /// single long gap is never averaged away by the p99.
    pub arrival_gap_max_us: u64,
    /// Age-at-output p99 (µs, F4): how old the frame the engine
    /// just sent was when it was sent (engine receipt → output
    /// send). The queueing delay the output path adds on top of
    /// the transport delay.
    pub age_at_output_p99_us: P99Tracker,
    /// Frames received by the engine (F4 `seqReceived`): accepted
    /// stream frames, duplicates excluded.
    pub frames_received: u64,
    /// Frames dropped as stale (F4): duplicates the dedupe
    /// dropped plus frames the drain cap discarded. A rising count
    /// with a healthy `frames_received` means the board is
    /// double-sending or the engine is draining late.
    pub stale_dropped: u64,
    /// Output sends (F4): how many times the engine actually sent
    /// a state to the output backend (ViGEm/Xbox).
    pub outputs_sent: u64,
    /// Largest burst in one engine tick (F4): most frames drained
    /// in a single tick. >1 means frames arrived bunched (Wi-Fi
    /// stall + catch-up, or double-send).
    pub frames_per_tick_max: u32,
    /// Cached p99 quintuple (PERF P1-3): (`computed_at_ms`,
    /// jitter, output latency, e2e, arrival gap, age at output) —
    /// all `Copy`, so `EngineMetrics` stays `Copy`.
    /// `0` = never computed. Skipped by serde (it is a cache, not a
    /// measurement). Recomputed at most every 100 ms by
    /// [`EngineMetrics::cached_p99s`]; a stale-by-≤100 ms p99 is fine
    /// for the diagnostics that read it at ≤20 Hz.
    #[serde(skip)]
    cached_p99s: CachedP99s,
}

impl EngineMetrics {
    /// Create empty metrics (everything unmeasured, counters zero).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            inter_arrival_jitter_p99_us: P99Tracker::new(),
            output_latency_p99_us: P99Tracker::new(),
            e2e_latency_p99_us: P99Tracker::new(),
            packet_loss: LossWindow::new(),
            seq_gap: 0,
            tx_fail: 0,
            arrival_gap_p99_us: P99Tracker::new(),
            arrival_gap_max_us: 0,
            age_at_output_p99_us: P99Tracker::new(),
            frames_received: 0,
            stale_dropped: 0,
            outputs_sent: 0,
            frames_per_tick_max: 0,
            cached_p99s: (0, None, None, None, None, None),
        }
    }

    /// Record one inter-arrival jitter sample (µs).
    pub fn record_inter_arrival_jitter(&mut self, jitter_us: u64) {
        self.inter_arrival_jitter_p99_us.record(jitter_us);
    }

    /// Record one PC-side inter-arrival gap sample (µs, F4): the
    /// time since the previous accepted frame reached the engine.
    /// Updates the gap max as well.
    pub fn record_arrival_gap(&mut self, gap_us: u64) {
        self.arrival_gap_p99_us.record(gap_us);
        self.arrival_gap_max_us = self.arrival_gap_max_us.max(gap_us);
    }

    /// Record one age-at-output sample (µs, F4): how old the frame
    /// was when the engine sent it to the output backend.
    pub fn record_age_at_output(&mut self, age_us: u64) {
        self.age_at_output_p99_us.record(age_us);
    }

    /// Count one accepted stream frame (F4 `seqReceived`).
    pub fn record_frame_received(&mut self) {
        self.frames_received += 1;
    }

    /// Count `count` frames dropped as stale (F4): duplicates the
    /// dedupe dropped, or frames the drain cap discarded.
    pub fn record_stale_dropped(&mut self, count: u64) {
        self.stale_dropped += count;
    }

    /// Count one output send (F4): the engine sent a state to the
    /// output backend.
    pub fn record_output_sent(&mut self) {
        self.outputs_sent += 1;
    }

    /// Record one tick's burst size (F4): the number of frames
    /// drained in that tick. Only the max is kept.
    pub fn record_burst(&mut self, frames: u32) {
        self.frames_per_tick_max = self.frames_per_tick_max.max(frames);
    }

    /// Record one output-latency sample (µs).
    pub fn record_output_latency(&mut self, latency_us: u64) {
        self.output_latency_p99_us.record(latency_us);
    }

    /// Record one E2E latency sample (µs, from a ping reply).
    pub fn record_e2e_latency(&mut self, latency_us: u64) {
        self.e2e_latency_p99_us.record(latency_us);
    }

    /// Record a ping measurement's E2E delay into the E2E tracker.
    pub fn record_ping_measurement(&mut self, measurement: &PingMeasurement) {
        self.record_e2e_latency(measurement.delay_us);
    }

    /// Record one stream packet outcome for the loss window.
    pub fn record_packet(&mut self, received: bool) {
        self.packet_loss.record_packet(received);
    }

    /// Record a sequence gap as one batch: `skipped` lost packets plus
    /// the one packet that arrived after them (L1).
    pub fn record_gap(&mut self, skipped: u16) {
        self.packet_loss.record_many(u32::from(skipped), 1);
    }

    /// Pass through the `seqGap`/`txFail` counters from a status packet.
    pub fn update_status_counters(&mut self, seq_gap: u64, tx_fail: u64) {
        self.seq_gap = seq_gap;
        self.tx_fail = tx_fail;
    }

    /// The five p99s, cached (PERF P1-3): the five 256-sample
    /// sorts run at most once per 100 ms (10 Hz) instead of on every
    /// 200 Hz `ingest_frame` tick (~14 µs/tick → ~0.1–0.2 µs/tick
    /// amortized). Callers on the hot path use this; the plain
    /// `*_p99()` accessors below stay exact for tests / slow paths.
    #[must_use]
    pub fn cached_p99s(&mut self) -> P99Set {
        let now_ms = p99_cache_now_ms();
        let (at, jitter, output, e2e, arrival_gap, age_at_output) = self.cached_p99s;
        if at != 0 && now_ms.saturating_sub(at) < 100 {
            return (jitter, output, e2e, arrival_gap, age_at_output);
        }
        let fresh = (
            self.inter_arrival_jitter_p99_us.p99(),
            self.output_latency_p99_us.p99(),
            self.e2e_latency_p99_us.p99(),
            self.arrival_gap_p99_us.p99(),
            self.age_at_output_p99_us.p99(),
        );
        self.cached_p99s = (now_ms, fresh.0, fresh.1, fresh.2, fresh.3, fresh.4);
        fresh
    }

    /// Inter-arrival jitter p99 (µs), [`None`] while unmeasured.
    #[must_use]
    pub fn jitter_p99(&self) -> Option<u64> {
        self.inter_arrival_jitter_p99_us.p99()
    }

    /// Output latency p99 (µs), [`None`] while unmeasured.
    #[must_use]
    pub fn output_latency_p99(&self) -> Option<u64> {
        self.output_latency_p99_us.p99()
    }

    /// E2E latency p99 (µs), [`None`] while unmeasured.
    #[must_use]
    pub fn e2e_latency_p99(&self) -> Option<u64> {
        self.e2e_latency_p99_us.p99()
    }

    /// PC-side inter-arrival gap p99 (µs, F4), [`None`] while
    /// unmeasured.
    #[must_use]
    pub fn arrival_gap_p99(&self) -> Option<u64> {
        self.arrival_gap_p99_us.p99()
    }

    /// Largest PC-side inter-arrival gap observed (µs, F4).
    #[must_use]
    pub const fn arrival_gap_max(&self) -> u64 {
        self.arrival_gap_max_us
    }

    /// Age-at-output p99 (µs, F4), [`None`] while unmeasured.
    #[must_use]
    pub fn age_at_output_p99(&self) -> Option<u64> {
        self.age_at_output_p99_us.p99()
    }

    /// Packet-loss fraction over the last 1000 packets (D8).
    #[must_use]
    pub fn loss_fraction(&self) -> f64 {
        self.packet_loss.loss_fraction()
    }
}

#[cfg(test)]
mod tests {
    use super::{EngineMetrics, LOSS_WINDOW, P99_CAPACITY, P99Tracker};
    use rsh_protocol::ping::PingMeasurement;

    #[test]
    fn p99_empty_and_single_sample() {
        let tracker = P99Tracker::new();
        assert_eq!(tracker.p99(), None);
        assert!(tracker.is_empty());
        assert_eq!(tracker.len(), 0);
        let mut tracker = P99Tracker::new();
        tracker.record(42);
        assert_eq!(tracker.p99(), Some(42));
        assert_eq!(tracker.len(), 1);
    }

    #[test]
    fn p99_known_samples_exact() {
        // 100 samples 1..=100: nearest-rank p99 = 99th smallest = 99.
        let mut tracker = P99Tracker::new();
        for value in 1..=100_u64 {
            tracker.record(value);
        }
        assert_eq!(tracker.p99(), Some(99));
    }

    #[test]
    fn p99_full_buffer_exact() {
        // 256 samples 0..=255: rank ceil(0.99*256) = 254th smallest = 253.
        let mut tracker = P99Tracker::new();
        for value in 0..u64::try_from(P99_CAPACITY).expect("capacity fits u64") {
            tracker.record(value);
        }
        assert_eq!(tracker.len(), P99_CAPACITY);
        assert_eq!(tracker.p99(), Some(253));
    }

    #[test]
    fn p99_ring_wraps_to_newest_samples() {
        // 300 samples 1..=300: buffer holds the newest 256 (45..=300);
        // 254th smallest of those is 45 + 253 = 298.
        let mut tracker = P99Tracker::new();
        for value in 1..=300_u64 {
            tracker.record(value);
        }
        assert_eq!(tracker.len(), P99_CAPACITY);
        assert_eq!(tracker.p99(), Some(298));
    }

    #[test]
    fn p99_unsorted_input_same_result() {
        // p99 must not depend on insertion order.
        let mut forward = P99Tracker::new();
        let mut reverse = P99Tracker::new();
        for value in 1..=100_u64 {
            forward.record(value);
            reverse.record(101 - value);
        }
        assert_eq!(forward.p99(), reverse.p99());
        assert_eq!(forward.p99(), Some(99));
    }

    #[test]
    fn loss_window_fraction_math() {
        let mut metrics = EngineMetrics::new();
        assert_eq!(metrics.loss_fraction(), 0.0);
        // 1000 packets, 10 lost -> 0.01.
        for i in 0..LOSS_WINDOW {
            metrics.record_packet(i % 100 != 0);
        }
        assert_eq!(metrics.packet_loss.packets_tracked(), 1000);
        assert_eq!(metrics.packet_loss.packets_lost(), 10);
        assert_eq!(metrics.loss_fraction(), 10.0 / 1000.0);
        // 1000 more, all received: the window slid past every loss.
        for _ in 0..LOSS_WINDOW {
            metrics.record_packet(true);
        }
        assert_eq!(metrics.loss_fraction(), 0.0);
        // Small window: 10 packets, 1 lost -> 0.1.
        let mut small = EngineMetrics::new();
        for _ in 0..9 {
            small.record_packet(true);
        }
        small.record_packet(false);
        assert_eq!(small.loss_fraction(), 1.0 / 10.0);
    }

    #[test]
    fn loss_window_slides_one_by_one() {
        let mut metrics = EngineMetrics::new();
        for _ in 0..LOSS_WINDOW {
            metrics.record_packet(true);
        }
        metrics.record_packet(false); // 1 lost in 1000
        assert_eq!(metrics.loss_fraction(), 1.0 / 1000.0);
        // 999 more received: the lost packet is still in the window.
        for _ in 0..(LOSS_WINDOW - 1) {
            metrics.record_packet(true);
        }
        assert_eq!(metrics.loss_fraction(), 1.0 / 1000.0);
        // One more received packet evicts the lost one.
        metrics.record_packet(true);
        assert_eq!(metrics.loss_fraction(), 0.0);
        assert_eq!(metrics.packet_loss.packets_lost(), 0);
    }

    #[test]
    fn e2e_stays_unmeasured_without_ping() {
        let metrics = EngineMetrics::new();
        assert_eq!(metrics.e2e_latency_p99(), None);
        assert_eq!(metrics.jitter_p99(), None);
        assert_eq!(metrics.output_latency_p99(), None);
        let json = serde_json::to_string(&metrics).expect("metrics serialize");
        assert!(json.contains("\"e2e_latency_p99_us\":null"), "{json}");
        assert!(
            json.contains("\"inter_arrival_jitter_p99_us\":null"),
            "{json}"
        );
        assert!(json.contains("\"output_latency_p99_us\":null"), "{json}");
        assert!(json.contains("\"packet_loss\":0.0"), "{json}");
    }

    #[test]
    fn serde_snapshot_values() {
        let mut metrics = EngineMetrics::new();
        metrics.record_inter_arrival_jitter(100);
        metrics.record_output_latency(200);
        metrics.record_ping_measurement(&PingMeasurement {
            delay_us: 300,
            offset_us: -25,
            board_processing_us: 50,
        });
        for _ in 0..9 {
            metrics.record_packet(true);
        }
        metrics.record_packet(false);
        metrics.update_status_counters(7, 3);
        assert_eq!(metrics.jitter_p99(), Some(100));
        assert_eq!(metrics.output_latency_p99(), Some(200));
        assert_eq!(metrics.e2e_latency_p99(), Some(300));
        let json = serde_json::to_string(&metrics).expect("metrics serialize");
        assert!(
            json.contains("\"inter_arrival_jitter_p99_us\":100"),
            "{json}"
        );
        assert!(json.contains("\"output_latency_p99_us\":200"), "{json}");
        assert!(json.contains("\"e2e_latency_p99_us\":300"), "{json}");
        assert!(json.contains("\"packet_loss\":0.1"), "{json}");
        assert!(json.contains("\"seq_gap\":7"), "{json}");
        assert!(json.contains("\"tx_fail\":3"), "{json}");
    }

    #[test]
    fn loss_window_record_many_matches_per_packet() {
        // L1: a batch must be identical to recording the same outcomes
        // one by one — including across the ring wrap.
        use super::LossWindow;
        let mut batched = LossWindow::new();
        let mut single = LossWindow::new();
        // Fill, wrap, and evict: 2500 outcomes in mixed batches.
        for (lost, received) in [(0u32, 900u32), (250, 1), (700, 49), (0, 600)] {
            batched.record_many(lost, received);
            for _ in 0..lost {
                single.record_packet(false);
            }
            for _ in 0..received {
                single.record_packet(true);
            }
            assert_eq!(batched.packets_tracked(), single.packets_tracked());
            assert_eq!(batched.packets_lost(), single.packets_lost());
            assert_eq!(batched.loss_fraction(), single.loss_fraction());
        }
        // record_gap: skipped lost + the arriving packet.
        let mut metrics = EngineMetrics::new();
        metrics.record_gap(4);
        assert_eq!(metrics.packet_loss.packets_tracked(), 5);
        assert_eq!(metrics.packet_loss.packets_lost(), 4);
        assert_eq!(metrics.loss_fraction(), 4.0 / 5.0);
    }

    #[test]
    fn status_counters_pass_through() {
        let mut metrics = EngineMetrics::new();
        assert_eq!(metrics.seq_gap, 0);
        assert_eq!(metrics.tx_fail, 0);
        metrics.update_status_counters(7, 3);
        assert_eq!(metrics.seq_gap, 7);
        assert_eq!(metrics.tx_fail, 3);
        // Latest status packet wins (pass-through, not accumulation).
        metrics.update_status_counters(9, 3);
        assert_eq!(metrics.seq_gap, 9);
        assert_eq!(metrics.tx_fail, 3);
    }

    /// F4 (v2.0.21): arrival-gap p99 + max, age-at-output p99, and
    /// the counters. The max is a scalar, so one long stall is
    /// never averaged away by the p99.
    #[test]
    fn f4_arrival_gap_and_age_at_output() {
        let mut metrics = EngineMetrics::new();
        assert_eq!(metrics.arrival_gap_p99(), None);
        assert_eq!(metrics.arrival_gap_max(), 0);
        assert_eq!(metrics.age_at_output_p99(), None);
        assert_eq!(metrics.frames_received, 0);
        assert_eq!(metrics.stale_dropped, 0);
        assert_eq!(metrics.outputs_sent, 0);
        assert_eq!(metrics.frames_per_tick_max, 0);
        for gap in [5000_u64, 5000, 5000, 20_000] {
            metrics.record_arrival_gap(gap);
        }
        assert_eq!(metrics.arrival_gap_p99(), Some(20_000));
        assert_eq!(metrics.arrival_gap_max(), 20_000);
        metrics.record_age_at_output(1200);
        metrics.record_age_at_output(800);
        assert_eq!(metrics.age_at_output_p99(), Some(1200));
        metrics.record_frame_received();
        metrics.record_frame_received();
        metrics.record_stale_dropped(3);
        metrics.record_output_sent();
        metrics.record_burst(1);
        metrics.record_burst(4);
        metrics.record_burst(2);
        assert_eq!(metrics.frames_received, 2);
        assert_eq!(metrics.stale_dropped, 3);
        assert_eq!(metrics.outputs_sent, 1);
        assert_eq!(metrics.frames_per_tick_max, 4);
        let json = serde_json::to_string(&metrics).expect("metrics serialize");
        assert!(json.contains("\"arrival_gap_p99_us\":20000"), "{json}");
        assert!(json.contains("\"arrival_gap_max_us\":20000"), "{json}");
        assert!(json.contains("\"age_at_output_p99_us\":1200"), "{json}");
        assert!(json.contains("\"frames_received\":2"), "{json}");
        assert!(json.contains("\"stale_dropped\":3"), "{json}");
        assert!(json.contains("\"outputs_sent\":1"), "{json}");
        assert!(json.contains("\"frames_per_tick_max\":4"), "{json}");
    }

    /// F4: the cached quintuple carries the two new p99s (and the
    /// cache still refreshes after 100 ms — the cached values are
    /// the fresh ones here because nothing was cached yet).
    #[test]
    fn f4_cached_p99s_include_arrival_gap_and_age() {
        let mut metrics = EngineMetrics::new();
        metrics.record_arrival_gap(6000);
        metrics.record_age_at_output(900);
        let (jitter, output, e2e, arrival_gap, age) = metrics.cached_p99s();
        assert_eq!(jitter, None);
        assert_eq!(output, None);
        assert_eq!(e2e, None);
        assert_eq!(arrival_gap, Some(6000));
        assert_eq!(age, Some(900));
    }
}
