//! 60 s headless soak runner (Phase B5, SW-R8.3) — the measured body
//! behind `tests/soak.rs` and the `rsh-soak` helper binary.
//!
//! Drives the pipeline, the sequence tracker and the watchdog at the
//! real 200 Hz pace — 12 000 ticks, 5 ms apart, wall-clock paced —
//! over the fixed representative input, and reports how much the
//! process grew:
//!
//! * **Heap (binding assertion):** net live bytes via the counting
//!   allocator must be `0` after warm-up. The pro-rated budget from
//!   `thresholds.json` (`soak_growth_mb_per_10min_max = 1`, i.e.
//!   ~100 KiB per 60 s) is far looser than `0`; the hot path allocates
//!   nothing at all, so a single leaked byte per tick would already be
//!   12 KiB here and is a bug, not noise.
//! * **RSS (coarse sanity bound, Linux only):** `VmRSS` growth over
//!   the 60 s must stay within the full 1 MiB / 10 min budget. RSS is
//!   page-granular and noisy at the 60 s scale (allocator arenas,
//!   lazy page-in), so it is checked against the whole 10-min budget
//!   rather than the pro-rated 100 KiB — the pro-rated check is the
//!   heap one above.
//!
//! This 60 s check is the **per-phase proxy, not the release proof**:
//! the full 10-min soak is the Phase H release gate (SW-R8.3) and the
//! 4-hour soak is the final release blocker (D-R14); both run in the
//! Virtual Test Rig, not here.
//!
//! # Why this lives here, not in the test
//!
//! [`run_soak`] must execute in a process with **no libtest harness**
//! (see `tests/soak.rs`): libtest fires a 60 s "has been running"
//! notice on its own thread inside the test process, and that notice
//! allocates — 512 + 82 = 594 bytes, transiently — at exactly the
//! instant the old in-process soak read its counters. The measured
//! code allocates nothing; the harness does. So the test spawns the
//! `rsh-soak` binary, which calls [`run_soak`] in a plain process,
//! and asserts on the numbers it reports.

use std::hint::black_box;
use std::time::{Duration, Instant};

use rsh_engine::pipeline::ProcessingPipeline;
use rsh_engine::sequence::SequenceTracker;
use rsh_engine::watchdog::Watchdog;
use rsh_protocol::StreamFrame;

use crate::alloc::{net_live_bytes, reset_counts};
use crate::fixture::representative_inputs;

/// Soak length: 60 s at 200 Hz.
pub const SOAK_TICKS: u64 = 12_000;
/// Real tick period (200 Hz).
pub const TICK: Duration = Duration::from_millis(5);
/// RSS growth allowed over the 60 s proxy: the full 10-min budget
/// (1 MiB), used as a coarse bound — see the module docs.
pub const RSS_GROWTH_BUDGET_BYTES: u64 = 1024 * 1024;
/// Tracker warm-up checks: past the 1000-entry loss window, so the
/// `VecDeque` is full and steady-state checks cannot grow it.
const TRACKER_WARMUP_CHECKS: u16 = 2000;
/// Hot-path warm-up ticks (pipeline state, watchdog).
const HOT_PATH_WARMUP_TICKS: u64 = 64;
/// Ticks the tracker must have accepted when the soak finishes:
/// warm-up checks + warm-up ticks + soak ticks.
pub const EXPECTED_ACCEPTED: u64 =
    TRACKER_WARMUP_CHECKS as u64 + HOT_PATH_WARMUP_TICKS + SOAK_TICKS;

/// Outcome of one [`run_soak`] run: the numbers `tests/soak.rs`
/// asserts on.
#[derive(Clone, Copy, Debug)]
pub struct SoakOutcome {
    /// Ticks driven in the measured window ([`SOAK_TICKS`]).
    pub ticks: u64,
    /// Packets the tracker accepted overall (warm-up + soak).
    pub accepted: u64,
    /// Wall-clock time the measured window took.
    pub elapsed: Duration,
    /// Net live heap bytes over the measured window (`0` expected).
    pub heap_growth: u64,
    /// RSS growth over the measured window, `None` where RSS is not
    /// measurable (non-Linux).
    pub rss_growth: Option<u64>,
}

impl SoakOutcome {
    /// Failure descriptions, empty when the soak passes. The checks
    /// are the original in-process test's, unchanged: heap growth
    /// must be exactly 0, every tick must be accepted, the soak must
    /// run a full 60 s, and RSS growth must stay within budget.
    #[must_use]
    pub fn failures(&self) -> Vec<String> {
        let mut failures = Vec::new();
        if self.accepted != EXPECTED_ACCEPTED {
            failures.push(format!(
                "accepted {} ticks, expected {EXPECTED_ACCEPTED}",
                self.accepted
            ));
        }
        if self.elapsed < Duration::from_secs(60) {
            failures.push(format!("soak ran for {:?}, expected >= 60 s", self.elapsed));
        }
        if self.heap_growth != 0 {
            failures.push(format!(
                "BUG: heap grew by {} net live bytes over {} ticks",
                self.heap_growth, self.ticks
            ));
        }
        if let Some(rss_growth) = self
            .rss_growth
            .filter(|rss_growth| *rss_growth > RSS_GROWTH_BUDGET_BYTES)
        {
            failures.push(format!(
                "RSS grew by {rss_growth} bytes over 60 s (budget {RSS_GROWTH_BUDGET_BYTES})"
            ));
        }
        failures
    }
}

/// Resident set size in bytes, read from `/proc/self/status`
/// (`VmRSS`), without allocating (fixed stack buffer). `None` on
/// non-Linux platforms or if the status file cannot be read.
#[cfg(target_os = "linux")]
fn rss_bytes() -> Option<u64> {
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

/// RSS is only measurable via `/proc` on Linux; elsewhere the soak
/// reports heap growth only.
#[cfg(not(target_os = "linux"))]
fn rss_bytes() -> Option<u64> {
    None
}

/// Run the 60 s soak in this process and return the outcome.
///
/// Call only from a process with no libtest harness (the `rsh-soak`
/// binary): the counting-allocator counters are process-global, so
/// anything else running in the process — the test harness included —
/// allocates into the measured window. Warm-up runs before the
/// counters are reset, so warm-up allocations are not measured.
/// Takes ~60 s by design (wall-clock paced).
///
/// # Panics
///
/// Panics if a fixed representative input fails to parse; the inputs
/// are built by [`representative_inputs`] and always parse, so this
/// cannot happen in practice.
#[must_use]
pub fn run_soak() -> SoakOutcome {
    let inputs = representative_inputs();
    let mut pipeline = ProcessingPipeline::default();
    let mut tracker = SequenceTracker::new();
    let mut watchdog = Watchdog::new();

    // Warm up: pipeline state, tracker loss window (2000 checks so
    // the VecDeque is full), watchdog. All warm-up allocations happen
    // before the counters are reset.
    let mut seq: u16 = 0;
    for _ in 0..TRACKER_WARMUP_CHECKS {
        let _decision = tracker.check(seq);
        seq = seq.wrapping_add(1);
    }
    let now = Instant::now();
    let mut index = 0usize;
    for _ in 0..HOT_PATH_WARMUP_TICKS {
        let input = &inputs[index % inputs.len()];
        index += 1;
        let frame = StreamFrame::parse(&input.bytes).expect("fixed input parses");
        // Monotonic tracker seq (L8): the fixture cycles 1024 frames
        // whose seqs repeat (0..1023), and feeding `frame.seq` to the
        // tracker replays old seqs — which L8 correctly classifies as
        // duplicates, so `accepted` would stall after the first cycle
        // and the soak would fail its tick count. The soak measures
        // heap growth over realistic hot-path work, so the tracker
        // gets the same monotonic stream the wheel emits in the rig;
        // the frame itself still goes through the pipeline unchanged.
        let _decision = tracker.check(seq);
        seq = seq.wrapping_add(1);
        watchdog.tick(now);
        let _out = pipeline.process(&frame);
    }

    reset_counts();
    let rss_before = rss_bytes();
    let start = Instant::now();
    let mut checksum: u64 = 0;
    for tick in 0..SOAK_TICKS {
        // Pace to the real 200 Hz grid; never sleep a negative span.
        let target = start + TICK * u32::try_from(tick).expect("tick fits u32");
        let now_instant = Instant::now();
        if target > now_instant {
            std::thread::sleep(target - now_instant);
        }
        let input = &inputs[index % inputs.len()];
        index += 1;
        let frame = StreamFrame::parse(black_box(&input.bytes)).expect("fixed input parses");
        let _decision = tracker.check(seq);
        seq = seq.wrapping_add(1);
        let tick_now = Instant::now();
        watchdog.tick(tick_now);
        let _stale = watchdog.is_stale(tick_now);
        let out = pipeline.process(black_box(&frame));
        checksum = checksum
            .wrapping_add(u64::from(out.steering.to_bits()))
            .wrapping_add(u64::from(out.buttons));
    }
    let elapsed = {
        // The last tick is due at start + 59.995 s; wait out the final
        // 5 ms so the soak is a full 60 s wall-clock, not 59.995 s.
        let end = start + TICK * u32::try_from(SOAK_TICKS).expect("ticks fit u32");
        let now_instant = Instant::now();
        if end > now_instant {
            std::thread::sleep(end - now_instant);
        }
        start.elapsed()
    };
    let heap_growth = net_live_bytes();
    let rss_after = rss_bytes();
    black_box(checksum);

    SoakOutcome {
        ticks: SOAK_TICKS,
        accepted: tracker.accepted(),
        elapsed,
        heap_growth,
        rss_growth: match (rss_before, rss_after) {
            (Some(before), Some(after)) => Some(after.saturating_sub(before)),
            _ => None,
        },
    }
}
