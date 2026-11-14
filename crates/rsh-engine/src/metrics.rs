//! Engine metrics: p99 tracking and the loss sliding window (D8).
//!
//! [`EngineMetrics`] is the data model the Performance panel consumes
//! (via `serde`): inter-arrival jitter p99, output-latency p99, E2E
//! latency p99 (`None` = "unmeasured", never a fake 0), packet-loss
//! fraction over the last 1000 packets, and the `seqGap`/`txFail`
//! counters passed through from status packets.
//!
//! The record path allocates nothing: p99 uses a fixed 256-sample ring
//! buffer (sorted copy on query only) and the loss window a fixed
//! 1000-entry ring buffer.

use rsh_protocol::ping::PingMeasurement;
use serde::{Serialize, Serializer};

/// Samples kept per p99 tracker.
pub const P99_CAPACITY: usize = 256;
/// Packets in the loss sliding window (D8).
pub const LOSS_WINDOW: usize = 1000;

/// Fixed-size p99 tracker: ring buffer, no heap allocation on record.
///
/// [`Serialize`] emits the current p99 value (`null` while unmeasured),
/// not the raw sample buffer.
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
        if self.len as usize == LOSS_WINDOW {
            // Window full: the evicted slot stops counting.
            if self.lost[self.head] {
                self.lost_count -= 1;
            }
        } else {
            self.len += 1;
        }
        self.lost[self.head] = !received;
        if !received {
            self.lost_count += 1;
        }
        self.head = (self.head + 1) % LOSS_WINDOW;
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
        }
    }

    /// Record one inter-arrival jitter sample (µs).
    pub fn record_inter_arrival_jitter(&mut self, jitter_us: u64) {
        self.inter_arrival_jitter_p99_us.record(jitter_us);
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

    /// Pass through the `seqGap`/`txFail` counters from a status packet.
    pub fn update_status_counters(&mut self, seq_gap: u64, tx_fail: u64) {
        self.seq_gap = seq_gap;
        self.tx_fail = tx_fail;
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
}
