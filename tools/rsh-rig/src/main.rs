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
//! the scenario registry (registry size and the `gate-*` subset
//! count are printed by the runner at startup — the header used
//! to hardcode "30 scenarios" while the registry held 73, R3-14),
//! three modes:
//!
//! * `quick` — every scenario at its short duration (~8 min total).
//! * `full` — longer durations incl. the 10-minute soak (~15 min).
//! * `long` — the 4-hour soak (`--only long-soak`, the release
//!   blocker) plus the other scenarios at their long durations.
//!
//! `--gate` runs only the `gate-*` contract checks; `--rig-quick`
//! (R3-02) runs the full quick suite — the same command CI's
//! `rig-gate` job runs. The local gate's default `--gate` run does
//! NOT reproduce CI; only `--rig-quick` does.
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

mod gate_checks;
mod harness;
mod lifecycle_checks;
mod metrics;
mod ota_checks;
mod rng;
mod scenarios;
mod webota_board;
mod wheel;

#[global_allocator]
static ALLOC: CountingAllocator = CountingAllocator;

/// One scenario's outcome for the report.
#[derive(Debug, Serialize)]
struct ScenarioResult {
    id: String,
    description: String,
    duration_sec: f64,
    seed: u64,
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
    /// Every seed this run executed (M-3 multi-seed support:
    /// `--seeds 42,7,1337`; single-seed runs list one seed).
    seeds: Vec<u64>,
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
        "Usage: rsh-rig [--mode quick|full|long] [--only <id>] [--seed N] [--seeds 42,7,1337] [--out DIR] [--list] [--gate] [--rig-quick]\n\
         \n\
         Modes: quick (~8 min), full (~15 min, 10-min soak), long (4-hour soak with --only long-soak).\n\
         --gate runs only the v2.0.2 gate checks (`gate-*` scenarios:\n\
         backend switch, typed errors, IPC timeout, performance, OTA,\n\
         installer audit, and the UI source audits). Fast, no engine run.\n\
         --rig-quick (R3-02) runs the FULL quick suite — every registry\n\
         scenario at quick durations, the same command CI's `rig-gate`\n\
         job runs. The default local gate (--gate) deliberately covers\n\
         only the gate-* contract checks; a green --gate run does NOT\n\
         reproduce CI. Use --rig-quick (phase gates only, ~30 min) for\n\
         the CI-equivalent local run.\n\
         --seed N runs every selected scenario once with seed N (default 42).\n\
         --seeds 42,7,1337 runs every selected scenario once per seed (M-3 multi-seed gate support;\n\
         the gate phase runs the full rig on seeds 42, 7 and 1337) and overrides --seed.\n\
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
    let mut seeds: Option<Vec<u64>> = None;
    let mut out: Option<PathBuf> = None;
    let mut list = false;
    // v2.0.2 gate: run only the `gate-*` contract checks (fast,
    // no engine run) instead of the full scenario set.
    let mut gate_only = false;
    // R3-02: `--rig-quick` runs the FULL quick suite (every registry
    // scenario, no gate filter) — the CI-equivalent local run.
    // It overrides `--gate` when both are given.
    let mut rig_quick = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--gate" => gate_only = true,
            "--rig-quick" => rig_quick = true,
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
            "--seeds" => {
                i += 1;
                let raw = args.get(i).cloned().unwrap_or_default();
                let parsed: Vec<u64> = raw
                    .split(',')
                    .filter_map(|v| v.trim().parse().ok())
                    .collect();
                if parsed.is_empty() {
                    eprintln!("--seeds needs a comma-separated list, e.g. --seeds 42,7,1337");
                    usage();
                    std::process::exit(2);
                }
                seeds = Some(parsed);
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
    // R3-02: --rig-quick is the full quick suite — it overrides
    // --gate (a run cannot be both the gate subset and the full
    // suite), and pins the mode to quick.
    if rig_quick {
        if gate_only {
            eprintln!("--rig-quick overrides --gate: running the FULL quick suite");
        }
        gate_only = false;
        mode = "quick".to_string();
    }
    // M-3: --seeds overrides --seed; default is the single --seed value.
    let seeds: Vec<u64> = seeds.unwrap_or_else(|| vec![seed]);

    let registry = scenarios::registry();
    // R3-14: counts are derived from the registry at runtime and
    // printed — never hardcoded in a header again.
    let gate_count = registry
        .iter()
        .filter(|s| s.id.starts_with("gate-"))
        .count();
    if list {
        for s in &registry {
            println!("{} — {}", s.id, s.description);
        }
        println!(
            "registry: {} scenarios, {} gate-*",
            registry.len(),
            gate_count
        );
        return;
    }
    let selected: Vec<&scenarios::Scenario> = registry
        .iter()
        .filter(|s| only.as_deref().is_none_or(|id| s.id == id))
        .filter(|s| !gate_only || s.id.starts_with("gate-"))
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
        "rsh-rig mode={mode} seeds={seeds:?} out={out} registry={total} gate={gate_count} selected={selected_n}",
        seeds = seeds,
        out = out_dir.display(),
        total = registry.len(),
        selected_n = selected.len()
    );

    let started_utc = utc_now();
    let run_started = Instant::now();
    let mut results: Vec<ScenarioResult> = Vec::new();
    let mut failed: Vec<String> = Vec::new();

    for seed in &seeds {
        let seed = *seed;
        for scenario in &selected {
            let duration = scenarios::duration_for(scenario, &mode);
            let t0 = Instant::now();
            println!(
                "--- {id} (seed {seed}, {duration} s, {desc})",
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
                failed.push(format!("{} (seed {seed})", scenario.id));
            }
            results.push(ScenarioResult {
                id: scenario.id.to_string(),
                description: scenario.description.to_string(),
                duration_sec: metrics.duration_sec,
                seed,
                passed,
                assertions: assertion_lines,
                metrics,
            });
        }
    }

    let passed_count = results.iter().filter(|r| r.passed).count();
    let report = RunReport {
        tool: "rsh-rig 1.0.0 (Phase H2)".to_string(),
        mode: mode.clone(),
        seed: seeds[0],
        seeds: seeds.clone(),
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
    let seeds_s = report
        .seeds
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let _ = write!(
        md,
        "- Started (UTC): {started}\n- Host: {host}\n- Toolchain: {toolchain}\n- Seed: {seed} (seeds run: {seeds_s})\n- Result: **{passed}/{total} PASS** in {wall_s:.1} s wall\n\n",
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
    // M-6: a stuck engine stop is LOUD — it fails its scenario, and
    // the report calls it out here too, not just in the per-scenario
    // lines. (The run already exits non-zero on any failed scenario.)
    let stuck: Vec<&str> = report
        .results
        .iter()
        .filter(|r| r.metrics.get("engineStopFailed") == Some(1.0))
        .map(|r| r.id.as_str())
        .collect();
    if !stuck.is_empty() {
        let names = stuck.join(", ");
        md.push_str(
            "## ⚠️ WARNING — engine did not stop in 5 s\n\n\
             The engine leg failed to stop within the 5 s timeout in: ",
        );
        let _ = write!(md, "{names}. ");
        md.push_str(
            "Those scenarios FAILED (`engineStopFailed == 1`); a stop\n\
             that hangs is a hang, not a slow teardown — investigate\n\
             before release.\n\n",
        );
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
         but were never processed by the engine are reported separately\n\
         as `coalesced` (never hidden) — a \"0% loss\" line is only honest next to its\n\
         `coalesced` number, which every per-scenario line below carries. `jitterP99` is\n\
         PC-RECEIVE inter-arrival jitter (wall-clock arrival spacing vs 5000 us — the plan\n\
         SLO's basis); the engine's board-timestamp jitter is `engineJitterP99`\n\
         (asserted ≤ 1000 us on soak / long-soak against the real target, not\n\
         informational-only). Ping raw samples are in `results.json` (`e2e_samples_us`)\n\
         with max / second-max / over-SLO count and a bootstrap 95% CI for the p99.\n\
         The wheel emits at 196 Hz by default — a DOCUMENTED HARNESS LIMITATION (H-2):\n\
         at exactly 200 Hz the OLD latest-wins drain coalesced ~3–14% of frames\n\
         by phase drift; the Round 2 multi-frame drain processes every frame
         (deep-scan gate: emit-200hz recorded 0% coalesced at 200 Hz). The soak
         still emits at 196 Hz by design, and the true 200 Hz\n\
         coalescing rate is measured by the `emit-200hz` scenario instead.\n\
         Startup ≤ 1200 ms SLO: UNMEASURED in this rig (M-5 — a Tauri window launch\n\
         cannot be measured in-process; see the Windows/human gates below).\n\n",
    );
    md.push_str("## Windows / human gates\n\n");
    md.push_str(
        "The following are NOT proven by this rig and need a Windows PC / human gate —\n\
         they are listed here so a green rig run is never read as proof of them:\n\n\
         - WINDOWS/CI GATE (H-7): UDP socket coverage — the 2 ignored `UdpTransport`\n\
         send tests (`helo_received_by_board_at_one_hertz`,\n\
         `control_send_and_reply_round_trip`, `crates/rsh-transport/src/udp.rs`) are\n\
         ignored in this sandbox because it blocks `sendto` (EPERM); they must run\n\
         on Windows CI before the socket path is called covered.\n\
         - Real-Wi-Fi NTP re-measure (H-8): the rig's E2E ping p99 is a LOOPBACK p99\n\
         (in-process channel); the 10 ms Wi-Fi SLO needs a re-measure over real\n\
         Wi-Fi on Windows hardware.\n\
         - Startup ≤ 1200 ms (M-5): launch-to-window time on the owner PC.\n\
         - LED output (M-10): the engine's `OutputState` carries no LED field, so\n\
         the rig cannot observe LED output at all — needs a firmware/app change to\n\
         expose LED state plus a Windows observation.\n\
         - FFB, per-game telemetry, profiles UI, serial transport, output\n\
         backends (`vJoy`/`HIDMaestro`), game detection / foreground detector\n\
         (incl. the `windows_foreground_detector_needs_windows_ci` test, which\n\
         needs a real foreground game window on Windows CI) — all Windows/human\n\
         gates (M-10, L-2).\n\
         - Real-board OTA (flashing the ESP32 + reboot) is a human gate:\n\
         the rig covers the WebOTA flow end to end against a fake board\n\
         (`ota-e2e-*` scenarios: success, bad-signature reject,\n\
         mid-upload disconnect + rollback) plus upgrade preservation\n\
         (`upgrade-preserve`, `installer-preserves-data`), but the real\n\
         board's flash + reboot is only proven on hardware.\n\
         - Multi-seed gate (M-3): this run's seeds are listed at the top; the gate\n\
         phase runs the full rig on seeds 42, 7 and 1337\n\
         (`cargo run -p rsh-rig -- --mode full --seeds 42,7,1337`).\n\n",
    );
    md.push_str("## Per-scenario\n\n");
    for r in &report.results {
        let verdict = if r.passed { "PASS" } else { "FAIL" };
        let _ = write!(
            md,
            "### {id} — {desc} — {verdict} ({dur:.0} s, seed {seed})\n\n",
            id = r.id,
            desc = r.description,
            dur = r.duration_sec,
            seed = r.seed,
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
        let blackout = get("blackoutExpectedFrames");
        let silence_max = get("silenceMaxMs");
        let restart_ms = get("engineRestartMs");
        let queue_drop = get("queueDropped");
        let alloc = get("allocBytesDelta");
        let heap_slope = get("heapSlope60sMBPer10Min");
        let ci_lo = get("e2eDelayP99CiLowUs");
        let ci_hi = get("e2eDelayP99CiHighUs");
        let _ = write!(
            md,
            "\nKey metrics: loss={loss}% (true transport loss at arrival) rate={rate}/s jitterP99={jitter}us (PC receive) outP99={out_p99}us valid={valid} dup={dup} watchdog={wd} ctrlAccepted={ctrl} coalesced={coalesced}% engineLoss={engine_loss}% engineJitterP99={engine_jitter}us blackout={blackout}f silenceMax={silence_max}ms restartMs={restart_ms}ms queueDrop={queue_drop} allocDelta={alloc}B heapSlope60s={heap_slope}MB/10min e2eP99CI=[{ci_lo},{ci_hi}]us\n\n",
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
    let seeds_arg = if report.seeds.len() == 1 {
        format!("--seed {}", report.seeds[0])
    } else {
        format!(
            "--seeds {}",
            report
                .seeds
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(",")
        )
    };
    let _ = write!(
        md,
        "cargo run -p rsh-rig -- --mode {mode} {seeds_arg}\n```\n",
        mode = report.mode,
        seeds_arg = seeds_arg,
    );
    if let Err(e) = std::fs::write(out_dir.join("REPORT.md"), &md) {
        eprintln!("cannot write REPORT.md: {e}");
    }
}
