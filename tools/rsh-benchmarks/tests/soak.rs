//! 60 s headless soak proxy (Phase B5, SW-R8.3).
//!
//! Drives the pipeline, the sequence tracker and the watchdog at the
//! real 200 Hz pace — 12 000 ticks, 5 ms apart, wall-clock paced —
//! over the fixed representative input, and asserts the process does
//! not grow:
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
//! Takes ~60 s by design (wall-clock paced). Single test function.
//!
//! # Why the measurement runs in a child process (Phase H FIX-A)
//!
//! The measurement used to run in this test process — and failed in
//! loaded full-suite runs with a byte-identical `594` net live bytes
//! while passing standalone. The 594 bytes were **not** the product
//! code: an allocation trace showed the measured window contains
//! exactly `+512 / +82 / -82 / -512` after tick 10 000, i.e. at the
//! 60 s boundary, while ticks 0..10 000 allocate nothing at all. The
//! source is libtest itself: with the default (multi-threaded)
//! runner it arms a 60 s timeout (`TEST_WARN_TIMEOUT_S = 60`) and,
//! when the soak is still running at 60 s, its main thread formats
//! and prints `test soak_60s_no_growth has been running for over 60
//! seconds` — a `format!` `String` (~82 B) plus the stdout write's
//! buffer (~512 B) — 594 bytes, transiently live. The soak reads its
//! counters at ~60.000 s, the same instant; when the read lands
//! inside the notice's alloc-to-free window the test fails, and when
//! it lands after the free it passes. Under load the harness thread
//! is preempted mid-notice, which stretches that window — hence
//! full-suite-only failures, byte-identical every time.
//!
//! So the measurement now runs in the `rsh-soak` child process (a
//! plain binary, no libtest — see `src/soak.rs` / `src/bin/rsh-soak.rs`)
//! and this test only spawns it and asserts on the `KEY=VALUE`
//! numbers it reports. The harness lives in a different process from
//! the measured code, by construction: no notice, no capture buffer,
//! no runner thread can allocate into the measured window. The
//! assertions themselves are unchanged (`heap_growth == 0`, same RSS
//! budget, same tick counts).

use rsh_benchmarks::soak::{EXPECTED_ACCEPTED, RSS_GROWTH_BUDGET_BYTES, SOAK_TICKS};

/// Numbers the `rsh-soak` child reports on stdout, one `KEY=VALUE`
/// line each.
struct SoakMetrics {
    /// Ticks driven in the measured window.
    ticks: u64,
    /// Packets the tracker accepted overall (warm-up + soak).
    accepted: u64,
    /// Wall-clock time of the measured window, in milliseconds.
    elapsed_ms: u64,
    /// Net live heap bytes over the measured window (`0` expected).
    heap_growth: u64,
    /// RSS growth over the measured window, `None` where the child
    /// could not measure RSS (non-Linux).
    rss_growth: Option<u64>,
}

/// Parse the child's `KEY=VALUE` lines. Panics (with the child's
/// full output attached) when a line is missing or malformed, so a
/// child-side crash or protocol change fails loudly instead of
/// asserting on a default.
fn parse_metrics(stdout: &str, stderr: &str, status: std::process::ExitStatus) -> SoakMetrics {
    let context = || {
        format!(
            "could not parse rsh-soak output (status {status:?}):\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
        )
    };
    let value = |key: &str| -> &str {
        stdout
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{key}=")))
            .unwrap_or_else(|| panic!("{}", context()))
    };
    let number = |key: &str| -> u64 {
        value(key)
            .parse()
            .unwrap_or_else(|_| panic!("{}", context()))
    };
    SoakMetrics {
        ticks: number("TICKS"),
        accepted: number("ACCEPTED"),
        elapsed_ms: number("ELAPSED_MS"),
        heap_growth: number("HEAP_GROWTH"),
        rss_growth: match value("RSS_GROWTH") {
            "unknown" => None,
            raw => Some(raw.parse().unwrap_or_else(|_| panic!("{}", context()))),
        },
    }
}

/// 60 s at 200 Hz: heap growth must be zero, RSS growth within budget.
#[test]
fn soak_60s_no_growth() {
    let exe = env!("CARGO_BIN_EXE_rsh-soak");
    let output = std::process::Command::new(exe)
        .output()
        .expect("failed to spawn the rsh-soak child process");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let metrics = parse_metrics(&stdout, &stderr, output.status);

    assert_eq!(
        metrics.ticks, SOAK_TICKS,
        "soak did not run every tick\n--- child stdout ---\n{stdout}\n--- child stderr ---\n{stderr}"
    );
    assert_eq!(
        metrics.accepted, EXPECTED_ACCEPTED,
        "soak did not process every tick through the tracker\n--- child stdout ---\n{stdout}\n--- child stderr ---\n{stderr}"
    );
    assert!(
        metrics.elapsed_ms >= 60_000,
        "soak ran for {} ms, expected >= 60 s\n--- child stdout ---\n{stdout}\n--- child stderr ---\n{stderr}",
        metrics.elapsed_ms
    );
    assert_eq!(
        metrics.heap_growth, 0,
        "BUG: heap grew by {} net live bytes over {} ticks\n--- child stdout ---\n{stdout}\n--- child stderr ---\n{stderr}",
        metrics.heap_growth, metrics.ticks
    );
    if let Some(rss_growth) = metrics.rss_growth {
        assert!(
            rss_growth <= RSS_GROWTH_BUDGET_BYTES,
            "RSS grew by {rss_growth} bytes over 60 s (budget {RSS_GROWTH_BUDGET_BYTES})\n--- child stdout ---\n{stdout}\n--- child stderr ---\n{stderr}"
        );
    }
    // The child exits nonzero exactly when its own checks fail; the
    // asserts above already name the failing metric, this one catches
    // any other child-side failure with the full output attached.
    assert!(
        output.status.success(),
        "rsh-soak child failed (status {:?}):\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        output.status
    );
}
