//! Rig metrics: the `§8.3` metric set, assertion evaluation, and the
//! small statistics helpers (p50/p99, linear slope) the runner needs.
//!
//! Metric names intentionally match the C# rig's `metrics.json` keys
//! (`TESTING-INPUT.md` §8.3), so a C# `REPORT.md` and a Rust
//! `results.json` can be compared key for key. Where the Rust engine
//! cannot observe a C# metric, the Rust registry uses the closest
//! engine-measured equivalent and the difference is documented in
//! `scenarios.rs` and `docs/PERF-H2.md` — never silently renamed.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde::Serialize;

/// One assertion from the scenario registry: `metric op value`.
#[derive(Clone, Debug)]
pub struct Assertion {
    /// Metric key in [`Metrics::values`].
    pub metric: &'static str,
    /// One of `<`, `<=`, `>`, `>=`, `==`, `!=`.
    pub op: &'static str,
    /// Expected value: a number, or the string `"pass"` for self-checks.
    pub value: MetricValue,
    /// Human note (from the C# registry, Rust-adapted where noted).
    pub note: &'static str,
    /// Minimum run duration (seconds) for this assertion to apply;
    /// the runner skips it (reports "skipped") on shorter runs
    /// instead of failing on a metric that cannot exist yet
    /// (e.g. a 10-minute memory slope on a 60 s smoke run).
    pub min_duration_s: f64,
}

/// An assertion value: numeric or the string `"pass"`/`"fail"`.
#[derive(Clone, Debug, PartialEq)]
pub enum MetricValue {
    /// Numeric threshold.
    Num(f64),
    /// String expectation (self-check scenarios).
    Str(&'static str),
}

impl MetricValue {
    /// The value as a number, if it is one.
    #[must_use]
    pub fn as_num(&self) -> Option<f64> {
        match self {
            Self::Num(v) => Some(*v),
            Self::Str(_) => None,
        }
    }
}

/// All metrics for one scenario run, serialized to `results.json`.
///
/// Numeric metrics live in [`Metrics::values`] (keyed exactly like the
/// C# rig); the string self-check outcome lives in `self_check`.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Metrics {
    /// Run identity.
    pub duration_sec: f64,
    /// Wall-clock seconds the engine actually ran.
    pub uptime_sec: f64,
    /// Peak resident set size observed (MiB), Linux only.
    pub peak_working_set_mb: f64,
    /// Seed used for this scenario.
    pub seed: u64,
    /// Numeric metrics, C# key names (§8.3).
    pub values: BTreeMap<String, f64>,
    /// Self-check outcome (`"pass"` / `"fail"` / `""` when N/A).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub self_check: String,
    /// Raw E2E ping delays (µs), in reply order (empty when the
    /// scenario sends no pings). Recorded so a failing p99 can be
    /// diagnosed from the run itself: with 31 samples the nearest-rank
    /// p99 IS the single slowest sample, so the raw list — not just
    /// the p99 — is the evidence for "one stall" vs "broadly slow".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub e2e_samples_us: Vec<u64>,
    /// PC-receive jitter histogram, non-zero bins only as
    /// `[bin_start_us, count]` pairs (10 µs bins, 0..20 ms; samples
    /// above 20 ms are a single `[20000, overflow_count]` entry).
    /// The tail shape is the evidence for "drift artefact" vs
    /// "contention": a drift artefact piles samples at ~5000 µs,
    /// contention spreads them to 10–60 ms.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arrival_jitter_hist: Vec<[u64; 2]>,
    /// Free-form events (faults, glitches, restarts) with tick times.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<String>,
}

impl Metrics {
    /// Create an empty metric set for a scenario run.
    #[must_use]
    pub fn new(duration_sec: f64, seed: u64) -> Self {
        Self {
            duration_sec,
            uptime_sec: 0.0,
            peak_working_set_mb: 0.0,
            seed,
            values: BTreeMap::new(),
            self_check: String::new(),
            e2e_samples_us: Vec::new(),
            arrival_jitter_hist: Vec::new(),
            events: Vec::new(),
        }
    }

    /// Record a numeric metric.
    pub fn set(&mut self, key: &str, value: f64) {
        self.values.insert(key.to_string(), value);
    }

    /// Read a numeric metric (`None` when the run never produced it).
    #[must_use]
    pub fn get(&self, key: &str) -> Option<f64> {
        self.values.get(key).copied()
    }

    /// Evaluate one assertion against the recorded metrics.
    /// A missing metric fails (a missing measurement is never a pass).
    #[must_use]
    pub fn check(&self, a: &Assertion) -> (bool, String) {
        if let MetricValue::Str(expected) = a.value {
            let actual = self.self_check.as_str();
            let pass = match a.op {
                "==" => actual == expected,
                "!=" => actual != expected,
                _ => false,
            };
            return (
                pass,
                format!("selfCheck {actual} {op} {expected}", op = a.op),
            );
        }
        let expected = a.value.as_num().unwrap_or(f64::NAN);
        match self.get(a.metric) {
            None => (
                false,
                format!(
                    "{metric}: MISSING (expected {op} {expected})",
                    metric = a.metric,
                    op = a.op
                ),
            ),
            Some(actual) => {
                let pass = match a.op {
                    "<" => actual < expected,
                    "<=" => actual <= expected,
                    ">" => actual > expected,
                    ">=" => actual >= expected,
                    "==" => (actual - expected).abs() < f64::EPSILON,
                    "!=" => (actual - expected).abs() >= f64::EPSILON,
                    _ => false,
                };
                (
                    pass,
                    format!(
                        "{metric}: {actual} {op} {expected}",
                        metric = a.metric,
                        op = a.op
                    ),
                )
            }
        }
    }
}

/// Nearest-rank percentile of a sample slice (µs). Returns `None`
/// for an empty slice. `pct` is 50 or 99 in the rig.
#[must_use]
pub fn percentile_us(samples: &[u64], pct: u64) -> Option<u64> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let rank = (pct as usize * sorted.len()).div_ceil(100);
    let idx = rank.saturating_sub(1).min(sorted.len() - 1);
    Some(sorted[idx])
}

/// Fixed-memory histogram for PC-receive inter-arrival jitter
/// samples (µs): 10 µs bins up to 20 ms, plus an overflow bin.
/// A 10-minute soak holds ~118k samples; storing them all would
/// break the rig's memory-bounded design (a 4-hour soak would hold
/// ~2.9M), so the p99 comes from counts, not a sample list. The p99
/// is the UPPER edge of its bin — conservative by at most one bin
/// (10 µs) against the SLO.
pub const JITTER_HIST_BIN_US: u64 = 10;
/// Bins cover 0..20 ms; anything larger lands in `overflow`.
pub const JITTER_HIST_BINS: usize = 2000;

#[derive(Clone, Debug)]
pub struct JitterHistogram {
    bins: [u64; JITTER_HIST_BINS],
    /// Samples > 20 ms (silence gaps, VM stalls).
    overflow: u64,
    /// Largest sample seen (exact p99 when it lands in `overflow`).
    max_us: u64,
    /// Samples recorded.
    total: u64,
    /// Samples > 5000 µs (the sandbox jitter SLO).
    over_5000: u64,
}

impl Default for JitterHistogram {
    fn default() -> Self {
        Self {
            // Arrays over 32 elements have no `Default` impl.
            bins: [0; JITTER_HIST_BINS],
            overflow: 0,
            max_us: 0,
            total: 0,
            over_5000: 0,
        }
    }
}

impl JitterHistogram {
    /// Record one jitter sample (µs).
    pub fn record(&mut self, jitter_us: u64) {
        self.total += 1;
        if jitter_us > self.max_us {
            self.max_us = jitter_us;
        }
        if jitter_us > 5000 {
            self.over_5000 += 1;
        }
        let bin = (jitter_us / JITTER_HIST_BIN_US) as usize;
        if bin < JITTER_HIST_BINS {
            self.bins[bin] += 1;
        } else {
            self.overflow += 1;
        }
    }

    /// Samples above the 5000 µs sandbox SLO.
    #[must_use]
    pub fn over_5000(&self) -> u64 {
        self.over_5000
    }

    /// Non-zero bins as `[bin_start_us, count]` pairs, for the run
    /// report (plus a `[20000, overflow]` entry when any sample
    /// exceeded 20 ms). The tail shape is the diagnostic: drift
    /// piles samples at ~5000 µs, contention spreads them wide.
    #[must_use]
    pub fn nonzero_bins(&self) -> Vec<[u64; 2]> {
        let mut out: Vec<[u64; 2]> = self
            .bins
            .iter()
            .enumerate()
            .filter(|(_, count)| **count > 0)
            .map(|(i, count)| [i as u64 * JITTER_HIST_BIN_US, *count])
            .collect();
        if self.overflow > 0 {
            out.push([JITTER_HIST_BINS as u64 * JITTER_HIST_BIN_US, self.overflow]);
        }
        out
    }

    /// Nearest-rank p99 (µs): the upper edge of the bin holding the
    /// `ceil(0.99 * n)`-th smallest sample; the max (an upper bound on
    /// the true p99) when that rank lands in the overflow bin.
    /// `None` when empty.
    #[must_use]
    pub fn p99(&self) -> Option<u64> {
        if self.total == 0 {
            return None;
        }
        // Nearest-rank: ceil(0.99 * n), 1-based.
        let rank = (99 * self.total).div_ceil(100);
        let mut seen: u64 = 0;
        for (i, count) in self.bins.iter().enumerate() {
            seen += *count;
            if seen >= rank {
                return Some((i as u64 + 1) * JITTER_HIST_BIN_US);
            }
        }
        Some(self.max_us)
    }
}

/// Least-squares slope of `values` over `times` (both seconds-based):
/// units of value-per-second. Used for the soak memory-growth slope
/// (bytes/second → MiB per 10 min). Returns 0.0 for < 2 samples.
#[must_use]
pub fn slope_per_sec(times_s: &[f64], values: &[f64]) -> f64 {
    let n = times_s.len().min(values.len());
    if n < 2 {
        return 0.0;
    }
    let n_f = n as f64;
    let mean_t = times_s[..n].iter().sum::<f64>() / n_f;
    let mean_v = values[..n].iter().sum::<f64>() / n_f;
    let mut num = 0.0;
    let mut den = 0.0;
    for i in 0..n {
        let dt = times_s[i] - mean_t;
        num += dt * (values[i] - mean_v);
        den += dt * dt;
    }
    if den == 0.0 { 0.0 } else { num / den }
}

/// Resident set size in bytes, read from `/proc/self/status`
/// (`VmRSS`). `None` on non-Linux platforms or on read failure.
#[cfg(target_os = "linux")]
#[must_use]
pub fn rss_bytes() -> Option<u64> {
    use std::io::Read as _;
    let mut file = std::fs::File::open("/proc/self/status").ok()?;
    let mut buf = [0u8; 4096];
    let n = file.read(&mut buf).ok()?;
    let text = std::str::from_utf8(&buf[..n]).ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let kib: u64 = rest.split_whitespace().next()?.parse().ok()?;
            return Some(kib * 1024);
        }
    }
    None
}

/// RSS is only measurable via `/proc` on Linux.
#[cfg(not(target_os = "linux"))]
#[must_use]
pub fn rss_bytes() -> Option<u64> {
    None
}

/// A periodic sampler: records `(elapsed_s, rss_bytes, heap_bytes)`
/// every `interval` while a run is in flight, for the soak report.
pub struct Sampler {
    start: Instant,
    interval: Duration,
    next: Duration,
    /// (elapsed seconds, RSS bytes, live heap bytes).
    pub samples: Vec<(f64, u64, u64)>,
}

impl Sampler {
    /// Create a sampler ticking every `interval` from now.
    #[must_use]
    pub fn new(interval: Duration) -> Self {
        Self {
            start: Instant::now(),
            interval,
            next: interval,
            samples: Vec::new(),
        }
    }

    /// Record a sample if the interval has elapsed since the last one.
    /// `heap_bytes` is the counting-allocator live-bytes reading.
    pub fn maybe_sample(&mut self, heap_bytes: u64) {
        let elapsed = self.start.elapsed();
        if elapsed >= self.next {
            if let Some(rss) = rss_bytes() {
                self.samples.push((elapsed.as_secs_f64(), rss, heap_bytes));
            }
            self.next += self.interval;
        }
    }

    /// Peak RSS across samples (bytes), 0 when no sample was taken.
    #[must_use]
    pub fn peak_rss_bytes(&self) -> u64 {
        self.samples.iter().map(|s| s.1).max().unwrap_or(0)
    }

    /// Heap slope in bytes/second over samples at/after `from_s`
    /// (least squares). Used for the soak leak check.
    #[must_use]
    pub fn heap_slope_from(&self, from_s: f64) -> f64 {
        let times: Vec<f64> = self
            .samples
            .iter()
            .filter(|s| s.0 >= from_s)
            .map(|s| s.0)
            .collect();
        let vals: Vec<f64> = self
            .samples
            .iter()
            .filter(|s| s.0 >= from_s)
            .map(|s| s.2 as f64)
            .collect();
        slope_per_sec(&times, &vals)
    }

    /// RSS slope in bytes/second over samples at/after `from_s`.
    #[must_use]
    pub fn rss_slope_from(&self, from_s: f64) -> f64 {
        let times: Vec<f64> = self
            .samples
            .iter()
            .filter(|s| s.0 >= from_s)
            .map(|s| s.0)
            .collect();
        let vals: Vec<f64> = self
            .samples
            .iter()
            .filter(|s| s.0 >= from_s)
            .map(|s| s.1 as f64)
            .collect();
        slope_per_sec(&times, &vals)
    }
}

#[cfg(test)]
mod tests {
    use super::{JitterHistogram, Metrics, percentile_us, slope_per_sec};

    #[test]
    fn percentile_nearest_rank() {
        let samples: Vec<u64> = (1..=100).collect();
        assert_eq!(percentile_us(&samples, 50), Some(50));
        assert_eq!(percentile_us(&samples, 99), Some(99));
        assert_eq!(percentile_us(&[], 99), None);
    }

    #[test]
    fn jitter_histogram_p99_is_conservative_upper_edge() {
        // 100 samples: 99 at 100 µs, 1 at 5100 µs. Nearest-rank p99
        // (99th smallest) is 100 µs; the histogram reports the upper
        // edge of the 100 µs bin (110 µs) — conservative by one bin.
        let mut h = JitterHistogram::default();
        for _ in 0..99 {
            h.record(100);
        }
        h.record(5100);
        assert_eq!(h.over_5000(), 1);
        assert_eq!(h.p99(), Some(110));
        assert_eq!(JitterHistogram::default().p99(), None);
    }

    #[test]
    fn jitter_histogram_p99_lands_on_the_tail() {
        // 100 samples at 5100 µs: p99 is the 5100 µs bin's upper edge.
        let mut h = JitterHistogram::default();
        for _ in 0..100 {
            h.record(5100);
        }
        assert_eq!(h.p99(), Some(5110));
        assert_eq!(h.over_5000(), 100);
    }

    #[test]
    fn jitter_histogram_overflow_reports_max() {
        // >1% of samples above 20 ms: the p99 rank lands in overflow
        // and the exact max is reported (a silence gap, not a bin).
        // Both overflow samples are equal here, so the max IS the
        // nearest-rank p99; in general the max is an upper bound on
        // the true p99 when the rank lands in overflow.
        let mut h = JitterHistogram::default();
        for _ in 0..98 {
            h.record(100);
        }
        h.record(3_000_000);
        h.record(3_000_000);
        assert_eq!(h.p99(), Some(3_000_000));
    }

    #[test]
    fn slope_flat_is_zero() {
        assert!(slope_per_sec(&[0.0, 1.0, 2.0], &[5.0, 5.0, 5.0]).abs() < 1e-9);
        assert!((slope_per_sec(&[0.0, 1.0, 2.0], &[0.0, 2.0, 4.0]) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn missing_metric_fails() {
        let m = Metrics::new(10.0, 42);
        let a = super::Assertion {
            metric: "lossPercent",
            op: "<",
            value: super::MetricValue::Num(0.5),
            note: "x",
            min_duration_s: 0.0,
        };
        assert!(!m.check(&a).0);
    }
}
