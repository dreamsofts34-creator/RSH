#![allow(
    clippy::too_many_lines,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::cast_lossless
)]
//! `rsh-rig` — the RSH Virtual Test Rig (Phase H2, SW-R8.2).
//!
//! `too_many_lines` is allowed crate-wide: the scenario registry is a
//! 600-line table and the run orchestration is one linear function;
//! splitting either would hurt readability without helping review.
//!
//! Seeded virtual wheel + real `WheelEngine` + recording output,
//! 26 scenarios, three modes:
//!
//! * `quick` — every scenario at its short duration (~8 min total).
//! * `full` — longer durations incl. the 10-minute soak (~15 min).
//! * `long` — the 4-hour soak (`--only long-soak`, the release
//!   blocker) plus the other scenarios at their long durations.
//!
//! The numeric casts above are allowed crate-wide on purpose: every
//! number in the rig ends up as an `f64` JSON metric or as a wire
//! value clamped/bounded by construction (axis mV 0..3300, wrapping
//! seq/ts, counters far below 2^53). None of them feeds control flow
//! that depends on exactness beyond those bounds.
//!
//! Exit code is 0 only when every selected scenario passes; the
//! harness itself never crashes on a scenario failure (a failing
//! scenario is reported, not hidden).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rsh_benchmarks::alloc::CountingAllocator;
use serde::Serialize;

use crate::metrics::Metrics;

mod harness;
mod metrics;
mod rng;
mod scenarios;
mod wheel;

#[global_allocator]
static ALLOC: CountingAllocator = CountingAllocator;

/// One scenario's outcome for the report.
#[derive(Debug, Serialize)]
struct ScenarioResult {
    id: String,
    description: String,
    duration_sec: f64,
    passed: bool,
    /// Per-assertion outcomes (`metric: actual op expected (PASS|FAIL|SKIPPED)`).
    assertions: Vec<String>,
    metrics: Metrics,
}

/// Whole-run report, serialized to `results.json`.
#[derive(Debug, Serialize)]
struct RunReport {
    tool: String,
    mode: String,
    seed: u64,
    started_utc: String,
    host: String,
    rustc: String,
    scenarios_run: usize,
    scenarios_passed: usize,
    scenarios_failed: Vec<String>,
    results: Vec<ScenarioResult>,
}

/// UTC timestamp `YYYY-MM-DDTHH:MM:SSZ` from the system clock
/// (no `chrono` dependency for one formatted string).
fn utc_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let days = secs / 86_400;
    let tod = secs % 86_400;
    let (hour, min, sec) = (tod / 3600, (tod / 60) % 60, tod % 60);
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02}T{hour:02}:{min:02}:{sec:02}Z")
}

/// Short host identity (kernel + arch + CPU count) for the report.
fn host_string() -> String {
    let uname = std::process::Command::new("uname")
        .args(["-s", "-r", "-m"])
        .output()
        .map_or_else(
            |_| "unknown".to_string(),
            |o| String::from_utf8_lossy(&o.stdout).trim().to_string(),
        );
    format!("{uname}, {n} CPUs", n = num_cpus())
}

/// CPU count from `/sys` (fallback 1).
fn num_cpus() -> usize {
    std::fs::read_dir("/sys/devices/system/cpu")
        .map_or(1, |d| {
            d.filter_map(Result::ok)
                .filter(|e| {
                    e.file_name().to_str().is_some_and(|n| {
                        n.starts_with("cpu") && n[3..].chars().all(|c| c.is_ascii_digit())
                    })
                })
                .count()
        })
        .max(1)
}

/// `rustc --version` for the report.
fn rustc_string() -> String {
    std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .map_or_else(
            |_| "unknown".to_string(),
            |o| String::from_utf8_lossy(&o.stdout).trim().to_string(),
        )
}

/// Evaluate one scenario's assertions against its metrics.
/// Assertions whose `min_duration_s` exceeds the run duration are
/// reported as skipped, not failed.
fn evaluate(scenario: &scenarios::Scenario, m: &Metrics) -> (bool, Vec<String>) {
    let mut all_pass = true;
    let mut lines = Vec::new();
    for a in &scenario.assertions {
        if m.duration_sec < a.min_duration_s {
            lines.push(format!(
                "{}: SKIPPED (needs >= {} s run, ran {:.0} s)",
                a.metric, a.min_duration_s, m.duration_sec
            ));
            continue;
        }
        let (pass, detail) = m.check(a);
        if !pass {
            all_pass = false;
        }
        let verdict = if pass { "PASS" } else { "FAIL" };
        lines.push(format!("{detail} ({verdict}) — {note}", note = a.note));
    }
    (all_pass, lines)
}

/// Print usage to stderr.
fn usage() {
    eprintln!(
        "Usage: rsh-rig [--mode quick|full|long] [--only <id>] [--seed N] [--out DIR] [--list]\n\
         \n\
         Modes: quick (~8 min), full (~15 min, 10-min soak), long (4-hour soak with --only long-soak).\n\
         --out DIR writes results.json + REPORT.md there (default: <repo>/../rig-runs/<timestamp>/ is NOT used;\n\
         default is $HOME/workspace/goals/rsh-real-sim-hub-project/hidden_files/rig-runs/<timestamp>/)."
    );
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut mode = "quick".to_string();
    let mut only: Option<String> = None;
    let mut seed: u64 = 42;
    let mut out: Option<PathBuf> = None;
    let mut list = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--mode" => {
                i += 1;
                mode = args.get(i).cloned().unwrap_or_default();
            }
            "--only" => {
                i += 1;
                only = args.get(i).cloned();
            }
            "--seed" => {
                i += 1;
                seed = args.get(i).and_then(|v| v.parse().ok()).unwrap_or(42);
            }
            "--out" => {
                i += 1;
                out = args.get(i).map(PathBuf::from);
            }
            "--list" => list = true,
            "--help" | "-h" => {
                usage();
                return;
            }
            other => {
                eprintln!("unknown argument: {other}");
                usage();
                std::process::exit(2);
            }
        }
        i += 1;
    }
    if !["quick", "full", "long"].contains(&mode.as_str()) {
        eprintln!("unknown mode: {mode}");
        usage();
        std::process::exit(2);
    }

    let registry = scenarios::registry();
    if list {
        for s in &registry {
            println!("{} — {}", s.id, s.description);
        }
        return;
    }
    let selected: Vec<&scenarios::Scenario> = registry
        .iter()
        .filter(|s| only.as_deref().is_none_or(|id| s.id == id))
        .collect();
    if selected.is_empty() {
        let want = only.unwrap_or_default();
        eprintln!("no scenario matches --only {want}");
        std::process::exit(2);
    }

    let out_dir = out.unwrap_or_else(|| {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home)
            .join("workspace/goals/rsh-real-sim-hub-project/hidden_files/rig-runs")
            .join(format!("{mode}-{ts}", ts = utc_now().replace(':', "")))
    });
    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        eprintln!("cannot create out dir {dir}: {e}", dir = out_dir.display());
        std::process::exit(2);
    }
    println!(
        "rsh-rig mode={mode} seed={seed} out={out}",
        out = out_dir.display()
    );

    let started_utc = utc_now();
    let run_started = Instant::now();
    let mut results: Vec<ScenarioResult> = Vec::new();
    let mut failed: Vec<String> = Vec::new();

    for scenario in selected {
        let duration = scenarios::duration_for(scenario, &mode);
        let t0 = Instant::now();
        println!(
            "--- {id} ({duration} s, {desc})",
            id = scenario.id,
            desc = scenario.description
        );
        let metrics = if duration == 0 {
            harness::run_contract(scenario)
        } else {
            let (m, _sampler) =
                harness::run_live(scenario, duration, seed, |line| println!("{line}")).await;
            m
        };
        let (passed, assertion_lines) = evaluate(scenario, &metrics);
        for line in &assertion_lines {
            println!("    {line}");
        }
        if !metrics.events.is_empty() {
            let ev = metrics.events.join(" | ");
            println!("    events: {ev}");
        }
        let verdict = if passed { "PASS" } else { "FAIL" };
        let wall_s = t0.elapsed().as_secs_f64();
        println!("    {verdict} in {wall_s:.1} s (wall)");
        if !passed {
            failed.push(scenario.id.to_string());
        }
        results.push(ScenarioResult {
            id: scenario.id.to_string(),
            description: scenario.description.to_string(),
            duration_sec: metrics.duration_sec,
            passed,
            assertions: assertion_lines,
            metrics,
        });
    }

    let passed_count = results.iter().filter(|r| r.passed).count();
    let report = RunReport {
        tool: "rsh-rig 1.0.0 (Phase H2)".to_string(),
        mode: mode.clone(),
        seed,
        started_utc,
        host: host_string(),
        rustc: rustc_string(),
        scenarios_run: results.len(),
        scenarios_passed: passed_count,
        scenarios_failed: failed.clone(),
        results,
    };
    write_reports(&out_dir, &report, run_started.elapsed());
    let total = report.scenarios_run;
    let wall_s = run_started.elapsed().as_secs_f64();
    let out_s = out_dir.display();
    println!("== rig {mode}: {passed_count}/{total} PASS in {wall_s:.1} s; reports in {out_s}");
    if !failed.is_empty() {
        let names = failed.join(", ");
        println!("FAILED: {names}");
        std::process::exit(1);
    }
}

/// Write `results.json` + `REPORT.md` into `out_dir`.
fn write_reports(out_dir: &Path, report: &RunReport, wall: Duration) {
    let json = serde_json::to_string_pretty(report).expect("report serializes");
    if let Err(e) = std::fs::write(out_dir.join("results.json"), &json) {
        eprintln!("cannot write results.json: {e}");
    }
    let mut md = String::new();
    let _ = write!(md, "# RSH Rust Rig Report — mode `{}`\n\n", report.mode);
    let wall_s = wall.as_secs_f64();
    let _ = write!(
        md,
        "- Started (UTC): {started}\n- Host: {host}\n- Toolchain: {toolchain}\n- Seed: {seed}\n- Result: **{passed}/{total} PASS** in {wall_s:.1} s wall\n\n",
        started = report.started_utc,
        host = report.host,
        toolchain = report.rustc,
        seed = report.seed,
        passed = report.scenarios_passed,
        total = report.scenarios_run,
    );
    if !report.scenarios_failed.is_empty() {
        let names = report.scenarios_failed.join(", ");
        let _ = write!(md, "- FAILED: {names}\n\n");
    }
    md.push_str("## Honest limits\n\n");
    md.push_str(
        "No real ADC, no real Wi-Fi (loopback-equivalent in-process channel — no real RF\n\
         loss/RSSI/AP roaming), no real `vJoy`/`HIDMaestro` driver (recording sink),\n\
         no ESP32 timing (firmware `maxCycleUs`/overruns/heap are plausible fixed values\n\
         in the status packet), no real Windows Tauri webview, sandbox jitter is VM\n\
         scheduling noise (threshold 5 ms sandbox vs 1 ms real). UDP is not used at all:\n\
         the sandbox blocks `sendto` (Phase G condition 5), so the wheel hands wire bytes\n\
         to the transport over a channel and the real `StreamFrame::parse` runs on them.\n\
         The rig's job is to catch bugs, not to claim to be a real board.\n\
         `loss` is TRUE TRANSPORT LOSS (sequence gaps at arrival); frames that arrived\n\
         but were superseded by the engine's latest-wins drain are reported separately\n\
         as `coalesced` (never hidden). `jitterP99` is PC-RECEIVE inter-arrival jitter\n\
         (wall-clock arrival spacing vs 5000 us — the plan SLO's basis); the engine's\n\
         board-timestamp jitter is `engineJitterP99` (informational). Ping raw samples\n\
         are in `results.json` (`e2e_samples_us`) with max / second-max / over-SLO count.\n\n",
    );
    md.push_str("## Per-scenario\n\n");
    for r in &report.results {
        let verdict = if r.passed { "PASS" } else { "FAIL" };
        let _ = write!(
            md,
            "### {id} — {verdict} ({dur:.0} s)\n\n",
            id = r.id,
            dur = r.duration_sec
        );
        for a in &r.assertions {
            let _ = writeln!(md, "- {a}");
        }
        let m = &r.metrics;
        let get = |k: &str| m.get(k).map_or_else(|| "-".into(), |v| format!("{v:.1}"));
        let loss = get("lossPercent");
        let rate = get("currentRatePerSecond");
        let jitter = get("tickJitterP99Us");
        let out_p99 = get("outputLatencyP99Us");
        let valid = get("validPackets");
        let dup = get("duplicatesOrLate");
        let wd = get("watchdogActivations");
        let ctrl = get("wheelCtrlAccepted");
        let coalesced = get("coalescedPercent");
        let engine_loss = get("engineLossPercent");
        let engine_jitter = get("engineTickJitterP99Us");
        let _ = write!(
            md,
            "\nKey metrics: loss={loss}% (true transport loss at arrival) rate={rate}/s jitterP99={jitter}us (PC receive) outP99={out_p99}us valid={valid} dup={dup} watchdog={wd} ctrlAccepted={ctrl} coalesced={coalesced}% engineLoss={engine_loss}% engineJitterP99={engine_jitter}us\n\n",
        );
        if !m.events.is_empty() {
            md.push_str("Events:\n\n");
            for e in &m.events {
                let _ = writeln!(md, "- {e}");
            }
            md.push('\n');
        }
    }
    md.push_str("## Reproduce\n\n```sh\n");
    let _ = write!(
        md,
        "cargo run -p rsh-rig -- --mode {mode} --seed {seed}\n```\n",
        mode = report.mode,
        seed = report.seed,
    );
    if let Err(e) = std::fs::write(out_dir.join("REPORT.md"), &md) {
        eprintln!("cannot write REPORT.md: {e}");
    }
}
