//! Rig metrics: the `§8.3` metric set, assertion evaluation, and the
//! small statistics helpers (p50/p99, linear slope) the runner needs.
//!
//! Metric names intentionally match the C# rig's `metrics.json` keys
//! (`TESTING-INPUT.md` §8.3), so a C# `REPORT.md` and a Rust
//! `results.json` can be compared key for key. Where the Rust engine
//! cannot observe a C# metric, the Rust registry uses the closest
//! engine-measured equivalent and the difference is documented in
//! `scenarios.rs` — never silently renamed.

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

/// Least-squares slope plus its standard error (units of
/// value-per-second). The standard error comes from the residual
/// variance (`s² / Σ(t − t̄)²`, `n − 2` degrees of freedom); it is
/// `0.0` when the fit is exact or the slope is undefined (< 3
/// samples, or all times equal). A 60 s soak slope over ~6 samples is
/// noisy — the SE is what says whether the slope is distinguishable
/// from flat, so the rig reports it next to every slope (M-8).
/// The slope alone is the first element (`slope_with_se(..).0`).
#[must_use]
pub fn slope_with_se(times_s: &[f64], values: &[f64]) -> (f64, f64) {
    let n = times_s.len().min(values.len());
    if n < 2 {
        return (0.0, 0.0);
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
    if den == 0.0 {
        return (0.0, 0.0);
    }
    let slope = num / den;
    if n < 3 {
        return (slope, 0.0);
    }
    // Residual variance: Σ(v − (v̄ + slope·(t − t̄)))² / (n − 2).
    let mut rss = 0.0;
    for i in 0..n {
        let fit = mean_v + slope * (times_s[i] - mean_t);
        let r = values[i] - fit;
        rss += r * r;
    }
    let s2 = rss / (n_f - 2.0);
    (slope, (s2 / den).sqrt())
}

/// Deterministic bootstrap confidence interval for the nearest-rank
/// p99 of `samples` (µs): resample with replacement `resamples`
/// times (seeded LCG — same input, same interval), take the p99 of
/// each resample, and return the 2.5th / 97.5th percentiles of those
/// p99s. Returns `None` for an empty slice. With ~200 ping samples
/// the single p99 hides how wide the tail estimate is; the interval
/// is the evidence (H-8). The interval is descriptive, not an SLO:
/// the SLO still asserts on the p99 itself.
#[must_use]
pub fn bootstrap_p99_ci(samples: &[u64], resamples: usize, seed: u64) -> Option<(u64, u64)> {
    if samples.is_empty() || resamples == 0 {
        return None;
    }
    // LCG (Numerical Recipes constants); deterministic per seed.
    let mut state = seed | 1;
    let mut next = move || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 33) as usize
    };
    let n = samples.len();
    let mut p99s: Vec<u64> = Vec::with_capacity(resamples);
    let mut draw: Vec<u64> = Vec::with_capacity(n);
    for _ in 0..resamples {
        draw.clear();
        for _ in 0..n {
            draw.push(samples[next() % n]);
        }
        if let Some(p) = percentile_us(&draw, 99) {
            p99s.push(p);
        }
    }
    if p99s.is_empty() {
        return None;
    }
    p99s.sort_unstable();
    let lo = p99s[(p99s.len() * 25 / 1000).min(p99s.len() - 1)];
    let hi = p99s[(p99s.len() * 975 / 1000).min(p99s.len() - 1)];
    Some((lo, hi))
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
    /// The sample buffer is pre-allocated (2048 entries ≈ 5.7 h at
    /// 10 s) BEFORE the run's `reset_counts()`: sampling must add no
    /// net heap growth inside the measured window (M-1), and a 4-hour
    /// soak needs 1440 samples.
    #[must_use]
    pub fn new(interval: Duration) -> Self {
        Self {
            start: Instant::now(),
            interval,
            next: interval,
            samples: Vec::with_capacity(2048),
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
        self.heap_slope_se_from(from_s).0
    }

    /// RSS slope in bytes/second over samples at/after `from_s`.
    #[must_use]
    pub fn rss_slope_from(&self, from_s: f64) -> f64 {
        self.rss_slope_se_from(from_s).0
    }

    /// Heap slope + standard error in bytes/second over samples
    /// at/after `from_s` (least squares + residual SE, M-8).
    #[must_use]
    pub fn heap_slope_se_from(&self, from_s: f64) -> (f64, f64) {
        let (times, vals) = self.heap_series_from(from_s);
        slope_with_se(&times, &vals)
    }

    /// RSS slope + standard error in bytes/second over samples
    /// at/after `from_s` (M-8).
    #[must_use]
    pub fn rss_slope_se_from(&self, from_s: f64) -> (f64, f64) {
        let (times, vals) = self.rss_series_from(from_s);
        slope_with_se(&times, &vals)
    }

    /// (times, heap values) at/after `from_s`.
    fn heap_series_from(&self, from_s: f64) -> (Vec<f64>, Vec<f64>) {
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
        (times, vals)
    }

    /// (times, RSS values) at/after `from_s`.
    fn rss_series_from(&self, from_s: f64) -> (Vec<f64>, Vec<f64>) {
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
        (times, vals)
    }
}

#[cfg(test)]
mod tests {
    use super::{JitterHistogram, Metrics, bootstrap_p99_ci, percentile_us, slope_with_se};

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
        assert!(slope_with_se(&[0.0, 1.0, 2.0], &[5.0, 5.0, 5.0]).0.abs() < 1e-9);
        assert!((slope_with_se(&[0.0, 1.0, 2.0], &[0.0, 2.0, 4.0]).0 - 2.0).abs() < 1e-9);
    }

    #[test]
    fn slope_se_is_zero_on_an_exact_line_and_positive_on_noise() {
        // Exact line: residuals are 0, so the SE is 0.
        let (slope, se) = slope_with_se(&[0.0, 1.0, 2.0, 3.0], &[0.0, 2.0, 4.0, 6.0]);
        assert!((slope - 2.0).abs() < 1e-9);
        assert!(se < 1e-9, "SE on an exact line: {se}");
        // Noisy data around a flat line: slope ~0 but SE > 0.
        let (_, se) = slope_with_se(&[0.0, 1.0, 2.0, 3.0], &[0.0, 10.0, 0.0, 10.0]);
        assert!(se > 0.0, "SE on noisy data must be positive");
        // < 3 samples: slope defined, SE reported as 0 (undefined).
        let (slope, se) = slope_with_se(&[0.0, 1.0], &[0.0, 2.0]);
        assert!((slope - 2.0).abs() < 1e-9);
        assert_eq!(se, 0.0);
    }

    #[test]
    fn bootstrap_p99_ci_brackets_the_p99_and_is_deterministic() {
        // 200 samples, one slow tail value: nearest-rank p99 is the
        // 198th value (ceil(0.99 * 200)) = 3000, so the single p99
        // HIDES the 9000 tail — the bootstrap interval's upper bound
        // (9000: ~63% of resamples draw the tail value at least once)
        // is what reveals it. That is the whole point of the CI.
        let mut samples: Vec<u64> = vec![3000; 199];
        samples.push(9000);
        let (lo, hi) = bootstrap_p99_ci(&samples, 200, 42).expect("CI");
        let p99 = percentile_us(&samples, 99).expect("p99");
        assert_eq!(p99, 3000);
        assert_eq!((lo, hi), (3000, 9000));
        assert!(
            lo <= p99 && p99 <= hi,
            "CI [{lo}, {hi}] must contain p99 {p99}"
        );
        assert_eq!(
            bootstrap_p99_ci(&samples, 200, 42),
            bootstrap_p99_ci(&samples, 200, 42),
            "same seed, same interval"
        );
        assert_eq!(bootstrap_p99_ci(&[], 200, 42), None);
        // Flat samples: the interval collapses to the single value.
        assert_eq!(bootstrap_p99_ci(&[5000; 50], 200, 7), Some((5000, 5000)));
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
