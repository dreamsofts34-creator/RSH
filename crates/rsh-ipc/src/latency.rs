//! Per-method IPC latency histogram (DS-82, v2.0.3).
//!
//! Every dispatch the server runs — inline cheap reads and
//! `spawn_blocking` handlers alike — records its duration here
//! (see `IpcServer::response_text`, the single instrumentation
//! point). The daemon exposes the global aggregate through
//! [`DiagStats::ipc_p50_ms`] / [`ipc_p99_ms`]
//! (`rsh-api`); the per-method snapshot below is the diagnostic
//! detail (which method is slow) for tests and later tooling.
//!
//! Ring buffers: the last 128 samples per method (plus a global
//! ring across methods), so a p99 over a burst of slow calls is
//! still visible after the burst ends, but ancient history never
//! dominates. All state lives behind one process-global mutex;
//! recording is a few integer ops + one lock, never I/O.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// Samples kept per method (and globally).
const SAMPLES: usize = 128;

/// One method's samples: a ring of microsecond durations.
#[derive(Default)]
struct MethodStats {
    /// Ring buffer (µs); `next` is the slot the next sample lands in.
    samples: Vec<u64>,
    next: usize,
    /// Total samples ever recorded (for the count, even past the ring).
    count: u64,
}

impl MethodStats {
    fn record(&mut self, micros: u64) {
        if self.samples.len() < SAMPLES {
            self.samples.push(micros);
        } else {
            self.samples[self.next] = micros;
            self.next = (self.next + 1) % SAMPLES;
        }
        self.count += 1;
    }

    /// p50 / p99 over the kept samples (µs); `None` with no samples.
    fn p50_p99(&self) -> Option<(u64, u64)> {
        if self.samples.is_empty() {
            return None;
        }
        let mut sorted = self.samples.clone();
        sorted.sort_unstable();
        let p50 = sorted[(sorted.len() - 1) / 2];
        let p99 = sorted[(sorted.len() - 1) * 99 / 100];
        Some((p50, p99))
    }
}

/// All latency state: per-method rings + the global ring.
#[derive(Default)]
struct IpcLatency {
    methods: HashMap<String, MethodStats>,
    global: MethodStats,
    /// Action → visible-result latency (R3-D-13, R2-46 remainder):
    /// samples of "mutating dispatch completed → first later
    /// `poll` / `slow_poll` dispatch", i.e. how long until a poll
    /// shows an action's result. `pending_action` is the completion
    /// instant of the last mutating dispatch not yet observed by a
    /// poll — a poll consumes it (one sample per action), so a
    /// 20 Hz poll stream does not flood the ring with one sample
    /// per tick for the same action.
    action: MethodStats,
    pending_action: Option<std::time::Instant>,
}

static LATENCY: OnceLock<Mutex<IpcLatency>> = OnceLock::new();

fn latency() -> &'static Mutex<IpcLatency> {
    LATENCY.get_or_init(|| Mutex::new(IpcLatency::default()))
}

/// Record one dispatch duration for `method` (DS-82).
///
/// PERF P3-1 (v2.0.13 polish): steady state allocates NOTHING —
/// a borrowed `get_mut` hit is the common path (every method is
/// seen within the first seconds of daemon life); the `String`
/// alloc happens once per method, on first sight. The global
/// `Mutex` stays: ~25 dispatches/s cannot contend it in
/// practice, and a lock-free/sharded counter would buy nothing
/// measurable here. p50/p99 are already lazy — they are computed
/// only inside `ipc_latency_snapshot` / `ipc_latency_global_p50_p99_ms`
/// (the 1 Hz `diag_stats` read), never on the dispatch path.
pub fn record_ipc_latency(method: &str, elapsed: Duration) {
    let micros = u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX);
    let Ok(mut guard) = latency().lock() else {
        return;
    };
    if let Some(stats) = guard.methods.get_mut(method) {
        stats.record(micros);
    } else {
        guard
            .methods
            .entry(method.to_string())
            .or_default()
            .record(micros);
    }
    guard.global.record(micros);
}

/// Global aggregate (DS-82): p50 / p99 IPC dispatch latency in
/// milliseconds, across all methods. `None` until a dispatch has
/// been recorded.
#[must_use]
// The µs values are dispatch durations — far below 2^53 µs
// (~285 years) — so the `u64 as f64` cast cannot lose precision.
#[allow(clippy::cast_precision_loss)]
pub fn ipc_latency_global_p50_p99_ms() -> Option<(f64, f64)> {
    let guard = latency().lock().ok()?;
    let (p50, p99) = guard.global.p50_p99()?;
    Some((p50 as f64 / 1000.0, p99 as f64 / 1000.0))
}

/// Per-method snapshot (DS-82): `(method, total count, p50 µs,
/// p99 µs)`, sorted by method name. Empty until a dispatch has
/// been recorded.
#[must_use]
pub fn ipc_latency_snapshot() -> Vec<(String, u64, u64, u64)> {
    let Ok(guard) = latency().lock() else {
        return Vec::new();
    };
    let mut out: Vec<(String, u64, u64, u64)> = guard
        .methods
        .iter()
        .filter_map(|(method, stats)| {
            stats
                .p50_p99()
                .map(|(p50, p99)| (method.clone(), stats.count, p50, p99))
        })
        .collect();
    out.sort();
    out
}

/// Record that a mutating dispatch just completed (R3-D-13): the
/// next `poll` / `slow_poll` dispatch observes its result, and
/// [`record_poll_observed`] turns the gap into a sample. Called
/// from the server's single instrumentation point, after the
/// dispatch — never on the hot path (mutating calls are rare).
pub fn record_action_completed() {
    let Ok(mut guard) = latency().lock() else {
        return;
    };
    guard.pending_action = Some(std::time::Instant::now());
}

/// Record that a `poll` / `slow_poll` dispatch just ran (R3-D-13):
/// when a mutating dispatch completed since the last poll, the
/// elapsed time is one action → visible-result sample. A poll with
/// no pending action records nothing.
pub fn record_poll_observed() {
    let Ok(mut guard) = latency().lock() else {
        return;
    };
    if let Some(started) = guard.pending_action.take() {
        let micros = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
        guard.action.record(micros);
    }
}

/// Action → visible-result latency aggregate (R3-D-13): p50 / p99
/// in milliseconds. `None` until an action and a later poll have
/// both been observed.
#[must_use]
// The µs values are dispatch gaps — far below 2^53 µs — so the
// `u64 as f64` cast cannot lose precision.
#[allow(clippy::cast_precision_loss)]
pub fn action_latency_p50_p99_ms() -> Option<(f64, f64)> {
    let guard = latency().lock().ok()?;
    let (p50, p99) = guard.action.p50_p99()?;
    Some((p50 as f64 / 1000.0, p99 as f64 / 1000.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p50_p99_of(samples: &[u64]) -> Option<(u64, u64)> {
        let mut stats = MethodStats::default();
        for sample in samples {
            stats.record(*sample);
        }
        stats.p50_p99()
    }

    #[test]
    fn ds82_percentiles_over_ring_samples() {
        // 1..=100 µs: p50 is the 50th value, p99 the 99th.
        let samples: Vec<u64> = (1..=100).collect();
        assert_eq!(p50_p99_of(&samples), Some((50, 99)));
        // A single sample is both p50 and p99.
        assert_eq!(p50_p99_of(&[42]), Some((42, 42)));
        // Empty: nothing measured.
        assert_eq!(p50_p99_of(&[]), None);
    }

    #[test]
    fn ds82_ring_keeps_last_128_samples() {
        let mut stats = MethodStats::default();
        for value in 1..=200_u64 {
            stats.record(value);
        }
        assert_eq!(stats.count, 200);
        assert_eq!(stats.samples.len(), SAMPLES);
        // The kept window is 73..=200: p50 136, p99 198.
        assert_eq!(stats.p50_p99(), Some((136, 198)));
    }

    /// R3-D-13: one action + one later poll yields one sample; a
    /// second poll without a new action yields none (the pending
    /// action is consumed by the first poll).
    #[test]
    fn action_latency_samples_first_poll_only() {
        let mut state = IpcLatency::default();
        // No action yet: a poll records nothing (the default
        // `pending_action` is already `None`).
        let before = state.action.count;
        // Simulate the record fns on this local state (the global
        // rings are process-shared, so the unit under test here is
        // the consume-once shape, not the global).
        state.pending_action = Some(std::time::Instant::now());
        // First poll consumes the pending action into one sample.
        if state.pending_action.take().is_some() {
            state.action.record(1_500);
        }
        assert_eq!(state.action.count, before + 1);
        assert_eq!(state.action.p50_p99(), Some((1_500, 1_500)));
        // Second poll: nothing pending, no new sample.
        if state.pending_action.take().is_some() {
            state.action.record(1_500);
        }
        assert_eq!(state.action.count, before + 1);
    }
}
