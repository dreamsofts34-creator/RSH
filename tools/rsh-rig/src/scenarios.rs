//! Scenario registry — the 26 C# rig scenarios plus `emit-200hz`
//! (H-2, the 200 Hz coalescing probe) plus the 3 Phase-2 pairing
//! scenarios (`pairing-window-open`, `pairing-window-expired`,
//! `pairing-unreachable` — contract-only, duration 0), same IDs,
//! same durations, same assertion shapes (`TESTING-INPUT.md` §8.4,
//! `test-env/scenarios/scenarios.json` v1).
//!
//! # Rust adaptations (all deliberate, all documented here)
//!
//! * The C# file lists 26 IDs while the plan text says "25" — the
//!   extra one is `long-soak`, the 4-hour variant of `soak`. The Rust
//!   rig runs all 26; "25/25" is satisfied as a subset.
//! * `fault-vjoy-drop` is renamed `fault-output-drop` (backend-agnostic,
//!   §8.4); the recording sink fails instead of a `vJoy` driver.
//! * `avgTickUs` / `maxTickUs` (C# engine-tick timings) do not exist
//!   in the Rust engine — it exposes output-latency p99 instead
//!   (`EngineMetrics::output_latency_p99`, handle-frame + send per
//!   tick). Soak/user-chaos assert `outputLatencyP99Us <= 2000`
//!   (the §8.4 `sloTargets.outputLatencyP99Us`); criterion benches
//!   measure tick nanoseconds separately.
//! * `clutch-range` gains a `clutchAxisMax <= 100` assertion: the C#
//!   v1 harness could not observe clutch, the Rust harness can (§8.4
//!   "naya rule": full assertions everywhere observable).
//! * `long-soak` gains `heapSlopeMBPer10Min <= 1` and
//!   `rssSlopeMBPer10Min <= 1` (SW-R8.3: growth ≤ 1 MB / 10 min),
//!   evaluated only on runs ≥ 600 s (`min_duration_s`).
//! * Axis metrics are recorded in `vJoy` units (0..32767) by the
//!   recording sink, so the C# thresholds (`<= 100`, `>= 32600`,
//!   `>= 32000`, `>= 30000`) apply unchanged.
//! * `wheelCtrlAccepted` is the wheel-side auth-gate count: the rig's
//!   virtual wheel opens its stream gate only after a real
//!   HMAC-verified `GET_CAPS`. This is the rig's mirror of SEC-E-14;
//!   the app-side session gate is still open (Phase G condition 13)
//!   and is H1's to cover — the rig does not claim otherwise.

use crate::metrics::{Assertion, MetricValue};
use crate::wheel::{Fault, WheelConfig, WheelKind};

/// Harness-side knobs for one scenario.
#[derive(Clone, Debug, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct HarnessConfig {
    /// Enable the clutch module in the pipeline plan (default plan
    /// has it disabled).
    pub clutch_enabled: bool,
    /// Disable the accelerator module in the pipeline plan.
    pub accel_disabled: bool,
    /// H-pattern shifter gate (default: sequential).
    pub hpattern: bool,
    /// Drive the engine's 1 Hz NTP-style ping (ntp-ping only).
    pub ping: bool,
    /// Do NOT send the auto `GET_CAPS` (auth-gate-blocked only).
    pub no_auto_get_caps: bool,
    /// Output-drop window `(start_s, dur_s)` (fault-output-drop).
    pub output_drop: Option<(f64, f64)>,
    /// Engine restart at `t` seconds (fault-app-restart, game-restore).
    pub restart_at: Option<f64>,
}

/// One scenario: identity, durations per mode, wheel + harness
/// config, and assertions.
#[derive(Clone, Debug)]
pub struct Scenario {
    /// Registry id (C# id, unchanged).
    pub id: &'static str,
    /// One-line description.
    pub description: &'static str,
    /// Duration in seconds for (quick, full, long) modes.
    pub durations: (u64, u64, u64),
    /// Virtual wheel configuration.
    pub wheel: WheelConfig,
    /// Harness configuration.
    pub harness: HarnessConfig,
    /// Assertions (`metric/op/value`, C# format).
    pub assertions: Vec<Assertion>,
}

/// Duration in seconds for a scenario in a given mode.
#[must_use]
pub fn duration_for(s: &Scenario, mode: &str) -> u64 {
    match mode {
        "full" => s.durations.1,
        "long" => s.durations.2,
        _ => s.durations.0,
    }
}

/// Shorthand for a numeric assertion.
const fn num(metric: &'static str, op: &'static str, value: f64, note: &'static str) -> Assertion {
    Assertion {
        metric,
        op,
        value: MetricValue::Num(value),
        note,
        min_duration_s: 0.0,
    }
}

/// Shorthand for a numeric assertion that only applies on runs of at
/// least `min_s` seconds.
const fn num_min(
    metric: &'static str,
    op: &'static str,
    value: f64,
    note: &'static str,
    min_s: f64,
) -> Assertion {
    Assertion {
        metric,
        op,
        value: MetricValue::Num(value),
        note,
        min_duration_s: min_s,
    }
}

/// Shorthand for the `wheelCtrlAccepted >= 1` assertion every live
/// scenario carries (auth gate exercised via real HMAC `GET_CAPS`).
const fn ctrl_accepted() -> Assertion {
    num(
        "wheelCtrlAccepted",
        ">=",
        1.0,
        "SEC-E-14: harness auto-GET_CAPS opened the wheel stream gate (real HMAC verify).",
    )
}

/// Shorthand for the `engineStopFailed == 0` assertion every live
/// scenario carries (M-6): a leg that did not stop in 5 s FAILS the
/// scenario — a stuck stop is a hang, not a slow teardown, and must
/// never hide behind an event line.
const fn stop_ok() -> Assertion {
    num(
        "engineStopFailed",
        "==",
        0.0,
        "M-6: every engine leg stopped within the 5 s timeout.",
    )
}

/// Shorthand for the `allocBytesDelta` assertion (M-1, L-5): the
/// net heap growth over the run. `bound` is 0 for clean scenarios
/// (steady-state hot path allocates nothing; the harness
/// pre-allocates its own bookkeeping before the measured window, so
/// a clean run must show exactly 0) and a small documented allowance
/// for fault/restart scenarios, where one-time window fills and
/// episode bookkeeping legitimately grow the heap once.
const fn alloc_bound(bound: f64, note: &'static str) -> Assertion {
    num("allocBytesDelta", "<=", bound, note)
}

/// The full registry, in C# order.
#[must_use]
pub fn registry() -> Vec<Scenario> {
    vec![
        Scenario {
            id: "soak",
            description: "Clean 60 s / 10 min soak at 196 Hz emit (documented harness limitation — see emit-200hz).",
            durations: (60, 600, 600),
            wheel: WheelConfig {
                kind: WheelKind::Soak,
                ..WheelConfig::default()
            },
            harness: HarnessConfig::default(),
            assertions: vec![
                num(
                    "lossPercent",
                    "<",
                    0.5,
                    "SLO: TRUE transport loss < 0.5% Wi-Fi (arrival seq gaps; engine-coalesced frames reported separately as coalescedPercent, never hidden).",
                ),
                num(
                    "coalescedPercent",
                    "<=",
                    0.5,
                    "H-1: at 196 Hz emit the engine's multi-frame drain must process every arrived frame; coalesced stays <= 0.5% of arrived frames. The 200 Hz rate is measured by emit-200hz, not here.",
                ),
                num("currentRatePerSecond", ">=", 195.0, "SLO: ~200 pps"),
                num(
                    "tickJitterP99Us",
                    "<=",
                    5000.0,
                    "PC-receive inter-arrival jitter p99 (plan SLO basis); sandbox threshold 5000 us, real target 1000 us. Engine board-ts jitter is engineTickJitterP99Us.",
                ),
                num(
                    "engineTickJitterP99Us",
                    "<=",
                    1000.0,
                    "H-3: engine board-timestamp jitter p99 against the REAL 1000 us target (asserted, not informational-only). The sample is |board-ts delta - 5000 us| (engine.rs inter_arrival_jitter_us), so a p99 of ~5001 us is the DOUBLE-STEP signature: two wheel emits (~5100 us apart) landing in one engine tick after a delayed tokio timer fires, delta ~10001 us. On a quiet host the wheel's 5100 us spacing means <= 1 frame per 5000 us engine tick and p99 ~= 100 us; under host load the timer bursts and ~1% double-steps pin the 256-sample ring's p99 at ~5001. Do NOT weaken this bound to pass under load — the gate phase re-runs it on a quiet host.",
                ),
                num(
                    "outputLatencyP99Us",
                    "<=",
                    2000.0,
                    "Rust: engine-measured output-latency p99 replaces C# avgTickUs (see header).",
                ),
                num(
                    "heapSlope60sMBPer10Min",
                    "<=",
                    1.0,
                    "M-8: heap slope over the 60 s window (samples from 10 s onward), pro-rated to MB/10 min against the SW-R8.3 1 MB/10 min bound; see heapSlope60sStdErrMBPer10Min for the fit's standard error.",
                ),
                ctrl_accepted(),
                stop_ok(),
                alloc_bound(
                    0.0,
                    "M-1: net heap growth must be exactly 0 (see dropout-storm).",
                ),
            ],
        },
        Scenario {
            id: "emit-200hz",
            description: "200 Hz emit probe: the REAL coalescing rate at the true tick rate (record-only).",
            durations: (20, 30, 30),
            wheel: WheelConfig {
                kind: WheelKind::Soak,
                // True 200 Hz emit (H-2): the wheel ticks at exactly
                // the engine's 200 Hz. The OLD latest-wins drain
                // coalesced a visible share of frames by phase drift;
                // the Round 2 multi-frame drain processes every frame
                // (deep-scan gate: coalesced 0% at 200 Hz). This
                // scenario RECORDS that rate — it carries NO
                // coalescedPercent / lossPercent /
                // engineTickJitterP99Us assertions, because at 200 Hz
                // those were expected to exceed the 196 Hz SLOs under
                // the old drain. Its job is to make the real number
                // visible, not to pass.
                tick_interval_us: 5000,
                ..WheelConfig::default()
            },
            harness: HarnessConfig::default(),
            assertions: vec![
                num(
                    "validPackets",
                    ">",
                    1000.0,
                    "Valid flow continues at 200 Hz emit",
                ),
                num(
                    "engineTicks",
                    ">=",
                    3000.0,
                    "Engine kept ticking at 200 Hz emit (20 s x 200 = 4000, 25% margin)",
                ),
                ctrl_accepted(),
                stop_ok(),
                alloc_bound(
                    0.0,
                    "M-1: net heap growth must be exactly 0 (see dropout-storm).",
                ),
            ],
        },
        Scenario {
            id: "dropout-storm",
            description: "5% loss, 3% duplicates, 3% bad CRC.",
            durations: (20, 60, 60),
            wheel: WheelConfig {
                kind: WheelKind::Soak,
                loss_pct: 0.05,
                duplicate_pct: 0.03,
                bad_crc_pct: 0.03,
                ..WheelConfig::default()
            },
            harness: HarnessConfig::default(),
            assertions: vec![
                num("validPackets", ">", 1000.0, "Valid flow continues"),
                num("badCrc", ">", 0.0, "Bad-CRC packets seen and rejected"),
                num(
                    "lossPercent",
                    "<",
                    15.0,
                    "H-4: loss ceiling from the injected fault model — 5% iid loss + 3% bad CRC unusable = ~7.9% expected; 15% is ~2x model (baseline measured 8.2%). A storm that loses more than the model injects is a product defect, not weather.",
                ),
                num(
                    "engineTicks",
                    ">=",
                    3000.0,
                    "Engine kept ticking in the storm (20 s x 200 = 4000, 25% margin)",
                ),
                ctrl_accepted(),
                stop_ok(),
                alloc_bound(
                    0.0,
                    "M-1: clean fault-injection run — net heap growth must be exactly 0 (steady-state hot path allocates nothing; harness bookkeeping is pre-allocated before the measured window).",
                ),
            ],
        },
        Scenario {
            id: "steering-calibration",
            description: "Full-range steering/pedal sweeps (rails must reach).",
            // Quick is 30 s, not 20 s: the trapezoid period is 12 s, so
            // a 20 s window holds only ONE bottom dwell (t=11..12 s) and
            // a random 2 s floating-sensor burst over it keeps the filter
            // ~2% off the rail (H2: min 2619 seed 42 / 824 seed 7). A
            // 30 s window holds TWO bottom dwells; the 30 s re-run with
            // the same seed/burst PASSED (min 85). Thresholds unchanged.
            durations: (30, 30, 30),
            wheel: WheelConfig {
                kind: WheelKind::Calibration,
                ..WheelConfig::default()
            },
            harness: HarnessConfig::default(),
            assertions: vec![
                num("steeringAxisMin", "<=", 100.0, "Left rail reached"),
                num("steeringAxisMax", ">=", 32600.0, "Right rail reached"),
                num("accelAxisMax", ">=", 32000.0, "Accel full range"),
                num("brakeAxisMax", ">=", 32000.0, "Brake full range"),
                ctrl_accepted(),
                stop_ok(),
                alloc_bound(
                    0.0,
                    "M-1: net heap growth must be exactly 0 (see dropout-storm).",
                ),
            ],
        },
        Scenario {
            id: "calibration-span",
            description: "Calibration span: both rails on every analog axis.",
            // Quick is 30 s for the same reason as steering-calibration:
            // the 12 s trapezoid needs two bottom dwells in the window so
            // one floating-sensor burst cannot hide the rail.
            durations: (30, 30, 30),
            wheel: WheelConfig {
                kind: WheelKind::Calibration,
                ..WheelConfig::default()
            },
            harness: HarnessConfig::default(),
            assertions: vec![
                num("steeringAxisMin", "<=", 100.0, "Left rail (span low end)"),
                num(
                    "steeringAxisMax",
                    ">=",
                    32600.0,
                    "Right rail (span high end)",
                ),
                num("accelAxisMin", "<=", 100.0, "Accel released = axis min"),
                num(
                    "accelAxisMax",
                    ">=",
                    32000.0,
                    "Accel fully pressed = full range",
                ),
                num("brakeAxisMin", "<=", 100.0, "Brake released = axis min"),
                num(
                    "brakeAxisMax",
                    ">=",
                    32000.0,
                    "Brake fully pressed = full range",
                ),
                num(
                    "handbrakeAxisMin",
                    "<=",
                    100.0,
                    "M-10: handbrake released = axis min (soak sweep reaches the 300 mV rail)",
                ),
                num(
                    "handbrakeAxisMax",
                    ">=",
                    32000.0,
                    "M-10: handbrake fully pulled = full range (soak sweep reaches the 3300 mV rail)",
                ),
                num("validPackets", ">", 1000.0, "Valid flow during calibration"),
                ctrl_accepted(),
                stop_ok(),
                alloc_bound(
                    0.0,
                    "M-1: net heap growth must be exactly 0 (see dropout-storm).",
                ),
            ],
        },
        Scenario {
            id: "config-transaction",
            description: "Config transaction state machine (contract-only self-check).",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Self-check PASS: SET_CFG chunk staging round-trip + tampered-chunk reject + incomplete-finish reject.",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "safe-mode",
            description: "3 s mid-run silence — watchdog must fire, then data resumes.",
            durations: (15, 15, 15),
            wheel: WheelConfig {
                kind: WheelKind::SafeMode,
                silence: Some((5.0, 3.0)),
                ..WheelConfig::default()
            },
            harness: HarnessConfig::default(),
            assertions: vec![
                num(
                    "watchdogActivations",
                    ">=",
                    1.0,
                    "Watchdog fired during silence",
                ),
                num("validPackets", ">", 1000.0, "Data back after resume"),
                num(
                    "neutralSends",
                    ">=",
                    100.0,
                    "H-4: safe-mode must emit NEUTRAL output while silent (3 s silence x 200 Hz = ~600 neutral sends; >= 100 leaves margin for tick phase). A watchdog that fires but never drives neutral is not safe.",
                ),
                num(
                    "lossPercent",
                    "<",
                    30.0,
                    "H-4: loss ceiling from the injected fault model — 3 s silence of 15 s, seq keeps counting = 20% expected; 30% is 1.5x model (baseline measured 20.1%).",
                ),
                num(
                    "blackoutExpectedFrames",
                    "<=",
                    750.0,
                    "H-5: the 3 s silence costs ~600 frames at the nominal 200 Hz emit rate (750 = 600 + 25% margin), reported separately from lossPercent.",
                ),
                num(
                    "silenceMaxMs",
                    "<=",
                    3500.0,
                    "H-5: no single silence episode longer than the 3 s injected silence + 500 ms margin.",
                ),
                ctrl_accepted(),
                stop_ok(),
                alloc_bound(
                    4096.0,
                    "M-1: fault-scenario allowance (<= 4096 B) — one-time silence-episode + neutral-send bookkeeping may grow the heap once; the pre-fix run showed 1352 B here, all of it the rig's own neutral_times Vec growth, now pre-allocated.",
                ),
            ],
        },
        Scenario {
            id: "modules-onoff",
            description: "Accelerator module disabled (wheel omits slot + plan disables it).",
            durations: (10, 10, 10),
            wheel: WheelConfig {
                kind: WheelKind::Modules,
                disable_slots: vec![1],
                ..WheelConfig::default()
            },
            harness: HarnessConfig {
                accel_disabled: true,
                ..HarnessConfig::default()
            },
            assertions: vec![
                num(
                    "accelAxisMax",
                    "<=",
                    100.0,
                    "Disabled accel stays neutral (min)",
                ),
                num("steeringAxisMax", ">=", 30000.0, "Steering still works"),
                ctrl_accepted(),
                stop_ok(),
                alloc_bound(
                    0.0,
                    "M-1: net heap growth must be exactly 0 (see dropout-storm).",
                ),
            ],
        },
        Scenario {
            id: "fuzz",
            description: "Fuzz packets (seed 42): corrupt CRC, truncated, random garbage.",
            durations: (20, 60, 60),
            wheel: WheelConfig {
                kind: WheelKind::Soak,
                fuzz: true,
                seed: 42,
                ..WheelConfig::default()
            },
            harness: HarnessConfig::default(),
            assertions: vec![
                num(
                    "engineTicks",
                    ">=",
                    3000.0,
                    "Engine kept ticking through fuzz",
                ),
                num("badCrc", ">", 0.0, "Bad-CRC fuzz packets rejected"),
                num(
                    "lossPercent",
                    "<",
                    45.0,
                    "H-4: loss ceiling from the injected fault model — 15% corrupt-CRC + 10% truncated + 5% garbage = ~30% of ticks unusable; 45% is 1.5x model (baseline measured 30.2%).",
                ),
                ctrl_accepted(),
                stop_ok(),
                alloc_bound(
                    0.0,
                    "M-1: net heap growth must be exactly 0 (see dropout-storm).",
                ),
            ],
        },
        Scenario {
            id: "chaos-net",
            description: "Gilbert-Elliott bursty loss + reorder + interference windows.",
            durations: (30, 60, 60),
            wheel: WheelConfig {
                kind: WheelKind::Soak,
                net_chaos: true,
                ..WheelConfig::default()
            },
            harness: HarnessConfig::default(),
            assertions: vec![
                num(
                    "engineTicks",
                    ">=",
                    4500.0,
                    "Engine kept ticking in chaos (30 s x 200 = 6000, 25% margin)",
                ),
                num("validPackets", ">", 1000.0, "Valid flow continues"),
                num(
                    "lossPercent",
                    "<",
                    20.0,
                    "H-4: loss ceiling from the injected fault model — Gilbert-Elliott bursts + ~200 ms interference windows every ~7 s ≈ 5–11% expected; 20% is ~2x the baseline measurement (10.5%).",
                ),
                ctrl_accepted(),
                stop_ok(),
                alloc_bound(
                    0.0,
                    "M-1: net heap growth must be exactly 0 (see dropout-storm).",
                ),
            ],
        },
        Scenario {
            id: "user-chaos",
            description: "Jerks, pedal slams, button mash, short dropouts.",
            durations: (20, 30, 30),
            wheel: WheelConfig {
                kind: WheelKind::UserChaos,
                ..WheelConfig::default()
            },
            harness: HarnessConfig::default(),
            assertions: vec![
                num(
                    "engineTicks",
                    ">=",
                    3000.0,
                    "Engine kept ticking through user chaos",
                ),
                num(
                    "lossPercent",
                    "<",
                    20.0,
                    "H-4: loss ceiling from the injected fault model — 0.5 s dropout every 5 s = 10% of ticks silent, seq keeps counting; 20% is 2x model (baseline measured 10.1%).",
                ),
                num(
                    "outputLatencyP99Us",
                    "<=",
                    2000.0,
                    "Rust: output-latency p99 replaces C# avgTickUs (see header).",
                ),
                ctrl_accepted(),
                stop_ok(),
                alloc_bound(
                    0.0,
                    "M-1: net heap growth must be exactly 0 (see dropout-storm).",
                ),
            ],
        },
        Scenario {
            id: "fault-reboot",
            description: "Board reboot at t=5 s (P3 regression: no duplicate explosion).",
            durations: (15, 15, 15),
            wheel: WheelConfig {
                kind: WheelKind::Soak,
                fault: Some(Fault::Reboot(5.0)),
                ..WheelConfig::default()
            },
            harness: HarnessConfig::default(),
            assertions: vec![
                num(
                    "lossPercent",
                    "<",
                    5.0,
                    "Loss sane after reboot (P3: no 5-min stuck)",
                ),
                num(
                    "duplicatesOrLate",
                    "<",
                    100.0,
                    "P3 REGRESSION: duplicates must not explode after reboot",
                ),
                num("validPackets", ">", 1500.0, "Data back after reboot"),
                num(
                    "blackoutExpectedFrames",
                    "<=",
                    500.0,
                    "H-5: the 2 s reboot silence costs ~400 frames at the nominal 200 Hz emit rate (500 = 400 + 25% margin). The reboot rebaselines the sequence, so this blackout does NOT count as transport loss — this number is where it is visible.",
                ),
                num(
                    "silenceMaxMs",
                    "<=",
                    2500.0,
                    "H-5: no single silence episode longer than the 2 s reboot silence + 500 ms margin.",
                ),
                ctrl_accepted(),
                stop_ok(),
                alloc_bound(
                    4096.0,
                    "M-1: fault-scenario allowance (<= 4096 B) — one-time reboot bookkeeping may grow the heap once.",
                ),
            ],
        },
        Scenario {
            id: "fault-wifi-drop",
            description: "Wi-Fi drop at t=5 s for 5 s (seq gap on resume).",
            durations: (15, 15, 15),
            wheel: WheelConfig {
                kind: WheelKind::Soak,
                fault: Some(Fault::WifiDrop(5.0, 5.0)),
                ..WheelConfig::default()
            },
            harness: HarnessConfig::default(),
            assertions: vec![
                num(
                    "watchdogActivations",
                    ">=",
                    1.0,
                    "Watchdog fired during drop",
                ),
                num("validPackets", ">", 1000.0, "Data back after drop"),
                num(
                    "lossPercent",
                    "<",
                    45.0,
                    "H-4: loss ceiling from the injected fault model — 5 s drop of 15 s, seq keeps counting = 33.3% expected; 45% is ~1.35x model (baseline measured 33.5%).",
                ),
                num(
                    "neutralSends",
                    ">=",
                    100.0,
                    "H-4: the 5 s drop must emit NEUTRAL output while silent (baseline measured 934 neutral sends).",
                ),
                num(
                    "blackoutExpectedFrames",
                    "<=",
                    1100.0,
                    "H-5: the 5 s drop costs ~1000 frames at the nominal 200 Hz emit rate (1100 = 1000 + 10% margin), reported separately from lossPercent.",
                ),
                num(
                    "silenceMaxMs",
                    "<=",
                    5500.0,
                    "H-5: no single silence episode longer than the 5 s injected drop + 500 ms margin.",
                ),
                ctrl_accepted(),
                stop_ok(),
                alloc_bound(
                    4096.0,
                    "M-1: fault-scenario allowance (<= 4096 B) — one-time drop bookkeeping may grow the heap once; the pre-fix run showed 558 B here, all of it the rig's own neutral_times Vec growth, now pre-allocated.",
                ),
            ],
        },
        Scenario {
            id: "fault-output-drop",
            description: "Recording output fails for 2 s at t=2 s (C# fault-vjoy-drop, renamed).",
            durations: (6, 6, 6),
            wheel: WheelConfig {
                kind: WheelKind::Soak,
                ..WheelConfig::default()
            },
            harness: HarnessConfig {
                output_drop: Some((2.0, 2.0)),
                ..HarnessConfig::default()
            },
            assertions: vec![
                // engineTicks counts SUCCESSFUL sends, and the 2 s drop
                // in this 6 s run fails ~400 of the ~1200 ticks by
                // design, so the ceiling is ~800 — the old >= 1000 was
                // unachievable on any run (H2 rig bug). >= 600 is the
                // 800 ceiling minus the standard 25% margin.
                num(
                    "engineTicks",
                    ">=",
                    600.0,
                    "Engine kept ticking through output drop",
                ),
                num("updateCalls", ">", 500.0, "Updates resume after reconnect"),
                ctrl_accepted(),
                stop_ok(),
                alloc_bound(
                    0.0,
                    "M-1: net heap growth must be exactly 0 (see dropout-storm).",
                ),
            ],
        },
        Scenario {
            id: "fault-app-restart",
            description: "Engine restart at t=3 s (app restart mid-drive).",
            durations: (6, 6, 6),
            wheel: WheelConfig {
                kind: WheelKind::Soak,
                ..WheelConfig::default()
            },
            harness: HarnessConfig {
                restart_at: Some(3.0),
                ..HarnessConfig::default()
            },
            assertions: vec![
                num("restarts", "==", 1.0, "One restart happened"),
                num(
                    "validPackets",
                    ">",
                    800.0,
                    "Packets still flow after restart",
                ),
                num(
                    "engineRestartMs",
                    "<=",
                    5000.0,
                    "H-5: the engine restart blackout (stop -> new leg spawned) must fit inside the 5 s stop timeout. Arrival lossPercent is 0.0% across a restart by construction — this number, not lossPercent, is the restart's cost.",
                ),
                ctrl_accepted(),
                stop_ok(),
                alloc_bound(
                    4096.0,
                    "M-1: restart-scenario allowance (<= 4096 B) — the second engine leg's one-time setup may grow the heap once.",
                ),
            ],
        },
        Scenario {
            id: "long-soak",
            description: "Long soak (600 s proxy): 60 s quick / 10 min full / 4 h long (release blocker).",
            durations: (60, 600, 14400),
            wheel: WheelConfig {
                kind: WheelKind::Soak,
                seq_start: 65500,
                ts_start: 4_294_960_000,
                ..WheelConfig::default()
            },
            harness: HarnessConfig::default(),
            assertions: vec![
                num(
                    "lossPercent",
                    "<",
                    0.5,
                    "SLO: TRUE transport loss < 0.5% (arrival seq gaps; coalesced reported separately as coalescedPercent).",
                ),
                num(
                    "coalescedPercent",
                    "<=",
                    0.5,
                    "H-1: at 196 Hz emit the engine's multi-frame drain must process every arrived frame; coalesced stays <= 0.5% of arrived frames (same rule as soak).",
                ),
                num("currentRatePerSecond", ">=", 195.0, "SLO: ~200 pps"),
                num(
                    "engineTickJitterP99Us",
                    "<=",
                    1000.0,
                    "H-3: engine board-timestamp jitter p99 against the REAL 1000 us target (asserted, not informational-only).",
                ),
                num(
                    "heapSlope60sMBPer10Min",
                    "<=",
                    1.0,
                    "M-8: heap slope over the 60 s window (samples from 10 s onward), pro-rated to MB/10 min against the SW-R8.3 1 MB/10 min bound; the 600 s slope below is the binding one on long runs.",
                ),
                num_min(
                    "heapSlopeMBPer10Min",
                    "<=",
                    1.0,
                    "SW-R8.3: heap growth <= 1 MB / 10 min (slope from 60 s onward).",
                    600.0,
                ),
                num_min(
                    "rssSlopeMBPer10Min",
                    "<=",
                    1.0,
                    "SW-R8.3: RSS growth <= 1 MB / 10 min (slope from 60 s onward).",
                    600.0,
                ),
                ctrl_accepted(),
                stop_ok(),
                alloc_bound(
                    0.0,
                    "M-1: net heap growth must be exactly 0 (see dropout-storm).",
                ),
            ],
        },
        Scenario {
            id: "ntp-ping",
            description: "1 Hz NTP-style ping for 200 s (E2E SLO measurable).",
            // 200 s, not 30 s: the p99 SLO needs enough samples to be
            // a real percentile. Nearest-rank p99 over 31 samples IS
            // the single slowest sample, so one VM scheduling stall
            // (gate full run: 21,768 µs, p50 3,273 µs — the signature
            // of a single stall, confirmed by 4 quiet standalone
            // retests: 124 samples, 0 over 10,000 µs, max 8,705 µs)
            // fails the scenario. At 1 Hz, 200 s gives ~201 samples
            // and p99 is the 3rd slowest — it tolerates 2 stalls.
            // The SLO itself is unchanged (10,000 µs); only the
            // sample count makes the percentile meaningful.
            durations: (200, 200, 200),
            wheel: WheelConfig {
                kind: WheelKind::Soak,
                ..WheelConfig::default()
            },
            harness: HarnessConfig {
                ping: true,
                ..HarnessConfig::default()
            },
            assertions: vec![
                num(
                    "e2eMeasured",
                    "==",
                    1.0,
                    "Ping replies arrived — E2E SLO measurable",
                ),
                num(
                    "e2eSamples",
                    ">=",
                    150.0,
                    "1 Hz ping, >= 150 samples in 200 s (75% of ~201 expected; >= 100 is what makes the p99 a real percentile, not the single max).",
                ),
                num(
                    "e2eTimeouts",
                    "==",
                    0.0,
                    "No ping timeout (virtual wheel answers)",
                ),
                num(
                    "e2eDelayP50Us",
                    "<=",
                    10000.0,
                    "Typical E2E delay <= 10 ms Wi-Fi SLO",
                ),
                num(
                    "e2eDelayP99Us",
                    "<=",
                    10000.0,
                    "SLO: E2E p99 <= 10 ms Wi-Fi — LOOPBACK p99 (H-8, in-process channel, no real Wi-Fi; the p99 carries a bootstrap 95% CI in e2eDelayP99CiLowUs/HighUs). p99 over ~201 samples is the 3rd slowest, so one VM stall cannot fail it. Real-Wi-Fi re-measure is a Windows/human gate.",
                ),
                ctrl_accepted(),
                stop_ok(),
                alloc_bound(
                    0.0,
                    "M-1: net heap growth must be exactly 0 (see dropout-storm).",
                ),
            ],
        },
        Scenario {
            id: "control-channel",
            description: "Control channel: HMAC verify / replay / duplicate (contract-only).",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Self-check PASS: GET_CAPS verify OK + tampered tag rejected + replay rejected + duplicate reqId rejected.",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "game-restore",
            description: "App restart mid-drive at t=5 s (proxy — real A2 verify is Windows).",
            durations: (10, 10, 10),
            wheel: WheelConfig {
                kind: WheelKind::Soak,
                ..WheelConfig::default()
            },
            harness: HarnessConfig {
                restart_at: Some(5.0),
                ..HarnessConfig::default()
            },
            assertions: vec![
                num("restarts", "==", 1.0, "One restart happened"),
                num(
                    "validPackets",
                    ">",
                    800.0,
                    "Packets still flow after restart",
                ),
                num("lossPercent", "<", 5.0, "Loss sane after restart"),
                num(
                    "engineRestartMs",
                    "<=",
                    5000.0,
                    "H-5: the engine restart blackout (stop -> new leg spawned) must fit inside the 5 s stop timeout. Arrival lossPercent is 0.0% across a restart by construction — this number, not lossPercent, is the restart's cost.",
                ),
                ctrl_accepted(),
                stop_ok(),
                alloc_bound(
                    4096.0,
                    "M-1: restart-scenario allowance (<= 4096 B) — the second engine leg's one-time setup may grow the heap once.",
                ),
            ],
        },
        Scenario {
            id: "pin-map-push",
            description: "Pin-map push: SET_CFG staging + PIN-RULES validation (contract-only).",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Self-check PASS: factory blob round-trips through SET_CFG chunks + PIN-RULES accepts it + conflicting map rejected.",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "encoder-glitch",
            description: "Encoder glitches, seed 42 (no max-delta assertion — v1 parity).",
            durations: (20, 30, 30),
            wheel: WheelConfig {
                kind: WheelKind::Soak,
                encoder_glitch: true,
                seed: 42,
                ..WheelConfig::default()
            },
            harness: HarnessConfig::default(),
            assertions: vec![
                num(
                    "engineTicks",
                    ">=",
                    3000.0,
                    "Engine kept ticking (20 s x 200 = 4000, 25% margin)",
                ),
                num("validPackets", ">", 1000.0, "Valid flow continues"),
                ctrl_accepted(),
                stop_ok(),
                alloc_bound(
                    0.0,
                    "M-1: net heap growth must be exactly 0 (see dropout-storm).",
                ),
            ],
        },
        Scenario {
            id: "clutch-range",
            description: "Clutch module disabled (v2-first: clutch output asserted).",
            durations: (10, 10, 10),
            wheel: WheelConfig {
                kind: WheelKind::Modules,
                disable_slots: vec![3],
                ..WheelConfig::default()
            },
            harness: HarnessConfig::default(),
            assertions: vec![
                num("steeringAxisMax", ">=", 30000.0, "Steering still works"),
                num("validPackets", ">", 1000.0, "Valid flow continues"),
                num(
                    "engineTicks",
                    ">=",
                    1500.0,
                    "Engine kept ticking (10 s x 200 = 2000, 25% margin)",
                ),
                num(
                    "clutchAxisMax",
                    "<=",
                    100.0,
                    "V2-FIRST: disabled clutch stays neutral (C# v1 could not observe clutch).",
                ),
                ctrl_accepted(),
                stop_ok(),
                alloc_bound(
                    0.0,
                    "M-1: net heap growth must be exactly 0 (see dropout-storm).",
                ),
            ],
        },
        Scenario {
            id: "clutch-sweep",
            description: "Clutch full-range sweep (clutch module enabled).",
            durations: (30, 30, 30),
            wheel: WheelConfig {
                kind: WheelKind::Clutch,
                ..WheelConfig::default()
            },
            harness: HarnessConfig {
                clutch_enabled: true,
                ..HarnessConfig::default()
            },
            assertions: vec![
                num("clutchAxisMin", "<=", 100.0, "Clutch released = axis min"),
                num(
                    "clutchAxisMax",
                    ">=",
                    32000.0,
                    "Clutch fully pressed = full range",
                ),
                num("validPackets", ">", 1000.0, "Valid flow continues"),
                ctrl_accepted(),
                stop_ok(),
                alloc_bound(
                    0.0,
                    "M-1: net heap growth must be exactly 0 (see dropout-storm).",
                ),
            ],
        },
        Scenario {
            id: "encoder-glitch-v2",
            description: "Encoder glitch jumps must be filtered (|delta| <= 32/tick).",
            durations: (30, 30, 30),
            wheel: WheelConfig {
                kind: WheelKind::Soak,
                encoder_glitch: true,
                glitch_big_rate: 0.005,
                seed: 42,
                ..WheelConfig::default()
            },
            harness: HarnessConfig::default(),
            assertions: vec![
                num(
                    "maxAbsEncoderDelta",
                    "<=",
                    32.0,
                    "Glitch jumps filtered — |delta| > 32 in one tick is impossible",
                ),
                num("validPackets", ">", 1000.0, "Valid flow continues"),
                num(
                    "engineTicks",
                    ">=",
                    4500.0,
                    "Engine kept ticking (30 s x 200 = 6000, 25% margin)",
                ),
                ctrl_accepted(),
                stop_ok(),
                alloc_bound(
                    0.0,
                    "M-1: net heap growth must be exactly 0 (see dropout-storm).",
                ),
            ],
        },
        Scenario {
            id: "shifter-sequential",
            description: "Sequential shifter: gear bits stay in the button word.",
            durations: (30, 30, 30),
            wheel: WheelConfig {
                kind: WheelKind::Shifter,
                ..WheelConfig::default()
            },
            harness: HarnessConfig::default(),
            assertions: vec![
                num(
                    "buttonMaskOr",
                    "==",
                    2016.0,
                    "Gear bits 5..10 (0x7E0) arrived in buttons",
                ),
                num(
                    "gearMaskSeen",
                    "==",
                    0.0,
                    "No gear decode in sequential mode",
                ),
                num("validPackets", ">", 1000.0, "Valid flow continues"),
                ctrl_accepted(),
                stop_ok(),
                alloc_bound(
                    0.0,
                    "M-1: net heap growth must be exactly 0 (see dropout-storm).",
                ),
            ],
        },
        Scenario {
            id: "shifter-hpattern",
            description: "H-pattern shifter: gear bits decode to gears 1..6.",
            durations: (30, 30, 30),
            wheel: WheelConfig {
                kind: WheelKind::Shifter,
                ..WheelConfig::default()
            },
            harness: HarnessConfig {
                hpattern: true,
                ..HarnessConfig::default()
            },
            assertions: vec![
                num(
                    "gearMaskSeen",
                    "==",
                    126.0,
                    "Gears 1..6 all decoded (gear g -> bit g, bits 1..6 = 126)",
                ),
                num(
                    "buttonMaskOr",
                    "==",
                    0.0,
                    "Gear bits did not leak into the button mask",
                ),
                num("validPackets", ">", 1000.0, "Valid flow continues"),
                ctrl_accepted(),
                stop_ok(),
                alloc_bound(
                    0.0,
                    "M-1: net heap growth must be exactly 0 (see dropout-storm).",
                ),
            ],
        },
        Scenario {
            id: "auth-gate-blocked",
            description: "No GET_CAPS sent: the wheel must emit zero stream packets.",
            durations: (10, 10, 10),
            wheel: WheelConfig {
                kind: WheelKind::Soak,
                ..WheelConfig::default()
            },
            harness: HarnessConfig {
                no_auto_get_caps: true,
                ..HarnessConfig::default()
            },
            assertions: vec![
                num(
                    "validPackets",
                    "==",
                    0.0,
                    "Gate closed: no stream packets without auth",
                ),
                num(
                    "wheelCtrlAccepted",
                    "==",
                    0.0,
                    "No control command accepted",
                ),
                num(
                    "authGetCapsOk",
                    "==",
                    0.0,
                    "Harness sent no GET_CAPS (--no-auto-get-caps)",
                ),
                stop_ok(),
                alloc_bound(
                    4096.0,
                    "M-1/L-5: auth-gate allowance (<= 4096 B). The pre-fix run showed 15456 B here — measured to be the rig's own neutral_times Vec growth (2001 neutral sends while gated, capacity doubling), NOT engine startup: the engine allocates nothing per tick (alloc gate) and its one-time setup happens before the measured window. The rig's neutral_times buffer is now pre-allocated (8192), so this allowance is a backstop, not an expectation.",
                ),
            ],
        },
        // Phase 2 (V2-PHASE-2-GATE): pairing/window scenarios (Phase 0
        // §4.4 gap — the rig had no pairing scenario). Contract-only
        // self-checks over the L1 Connection Manager: window-open
        // pairing succeeds, window-expired pairing fails closed with
        // the typed reason, board-unreachable fails closed with a
        // distinct reason. No wheel, no UDP — the manager is pure.
        Scenario {
            id: "pairing-window-open",
            description: "Pairing with the window open: HELLO (unpaired, window open) -> Authenticating -> GET_CAPS ok -> Active.",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Self-check PASS: window-open pairing reaches Active (Connected) via the L1 Connection Manager.",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "pairing-window-expired",
            description: "Pairing with the window expired: HELLO (unpaired, window closed) -> Error(WindowClosed), auto-retry stopped, never Active.",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Self-check PASS: window-expired pairing fails closed with typed reason WindowClosed and no auto-retry.",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "pairing-unreachable",
            description: "Pairing with no board: 3 HELLO timeouts -> Error(Unreachable), a reason distinct from WindowClosed, never Active.",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Self-check PASS: unreachable board fails closed with typed reason Unreachable (distinct from WindowClosed).",
                min_duration_s: 0.0,
            }],
        },
        // Round 2C — WebOTA end to end against a fake board, upgrade
        // preservation, installer data-dir safety. Contract-only
        // (duration 0): the self-checks run the real app-core flash
        // flow over loopback TCP, no real ESP32 / Wi-Fi.
        Scenario {
            id: "ota-e2e-success",
            description: "WebOTA e2e: signed 4.0.2 image flashes to the fake board (4.0.1), board restarts on 4.0.2, flow confirms the new version with upload progress.",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Self-check PASS: flash succeeded, board on 4.0.2, upload reached 100%.",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "ota-e2e-same-version",
            description: "WebOTA e2e: reinstalling the version the board already runs (4.0.2 over 4.0.2) is allowed — the flash succeeds, the confirm reads 4.0.2, the board stays on 4.0.2.",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Self-check PASS: same-version reinstall succeeded, board stayed on 4.0.2, upload reached 100%.",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "ota-e2e-bad-signature",
            description: "WebOTA e2e: tampered image rejected app-side (signature) and wrong-key image rejected board-side; board stays on 4.0.1 in both cases.",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Self-check PASS: both bad images rejected, board stayed on 4.0.1.",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "ota-e2e-disconnect-rollback",
            description: "WebOTA e2e: connection drops mid-upload — flash fails, board keeps running 4.0.1 (rollback); retry with the link back succeeds to 4.0.2.",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Self-check PASS: aborted upload rolled back to 4.0.1, retry succeeded to 4.0.2.",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "upgrade-preserve",
            description: "Upgrade preservation: a v2.0.0/v2.0.1-era store (schema 2, hand-written, missing newer fields) opens intact — profiles, tunings, active id, settings — and survives a save + reopen cycle.",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Self-check PASS: old store opened intact with no load warnings; save + reopen cycle clean.",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "installer-preserves-data",
            description: "Installer safety: the release workflow's generated NSIS script never touches the data dir (%AppData%\\RSH) and bundles firmware-signed.bin; uninstall deletes only $INSTDIR / $SMPROGRAMS paths.",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Self-check PASS: NSIS script touches no data-dir path; firmware-signed.bin bundled.",
                min_duration_s: 0.0,
            }],
        },
        // ---- v2.0.2 gate checks (fix plan §C) ----
        // Contract-only (duration 0): the harness runs them via
        // run_contract, like the OTA checks above. `cargo run -p
        // rsh-rig -- --gate` runs only these.
        Scenario {
            id: "gate-backend-switch",
            description: "Gate (A10): backend switch vJoy ↔ HID state machine + startup UAC deferral (persisted HIDMaestro builds zero factories at startup, so zero UAC prompts) + UAC copy.",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Gate PASS: switch round-trip + fallback honest; HID deferred at startup (0 factory builds).",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "gate-user-errors",
            description: "Gate (B6): every ConnectionError variant renders as a typed UserError with non-empty, jargon-free what-happened / what-to-do, a stable code, and distinct copy per cause.",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Gate PASS: all 15 error variants render distinct, jargon-free user copy.",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "gate-ipc-timeout",
            description: "Gate (B5/A9): a silent daemon times out the IPC call within 5 s (500 ms client bound) instead of hanging the UI; connecting to a dead socket fails fast.",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Gate PASS: silent peer times out with a typed error; dead socket fails fast.",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "gate-perf",
            description: "Gate (B7): startup path (store open + AppStateCore build) under 1 s; IPC ping round-trip p50 < 5 ms, p99 < 50 ms over 100 pings on loopback.",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Gate PASS: startup + IPC round-trip inside thresholds.",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "gate-ota-e2e",
            description: "Gate (B4): OTA end-to-end via the 2C rig scenarios — signed flash, same-version reinstall, bad-signature reject, disconnect rollback, upgrade preserve, installer preserves data.",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Gate PASS: all six OTA self-checks pass on the fake board.",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "gate-installer-audit",
            description: "Gate (B3): read-only audit of the release workflow — smart-installer version compare (update/reinstall/downgrade prompts), daemon stop+restart, registry version, user-data survival, firewall rule, uninstall cleanup; records that the smoke test only installs the same version twice.",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Gate PASS: installer markers present in the workflow (version-compare branches stay user-PC verified).",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "gate-slider-source",
            description: "Gate (A6), source audit: SliderRow fill is parent.width * frac (clamped 0..1) from the left with the thumb at parent.width * frac - 7px; no inverted (1 - value) fill. Pixel proof lives in rsh-gate-ui (ui-slider-visual).",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Gate PASS: slider fill binding is min→thumb in the source.",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "gate-tray-persistence-source",
            description: "Gate (A4), source audit: tray popup hides only on click-outside (Windows foreground check, 500 ms grace), Escape, or its own buttons — no unconditional auto-hide timer. Runtime proof is Windows-only (user PC).",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Gate PASS: no auto-hide timer; hide paths are click-outside/Escape/action only.",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "gate-tray-data-source",
            description: "Gate (A12), source audit: tray popup data comes from the live daemon API (label cloned from the live connection, bars clamped 0–100) and defaults to Disconnected with zero bars.",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Gate PASS: tray data is live-derived, never hardcoded fake values.",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "gate-x-close-source",
            description: "Gate (A11), source audit: window close hides to the tray (never quits) and tray reopen restores the existing window.",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Gate PASS: close-to-tray wiring present in rsh-ui.",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "gate-wizard-reopen-source",
            description: "Gate (A1), source audit: wizard reopen from Settings (Run setup again → run-setup-again → marker cleared, wizard at step 0) AND from the tray menu.",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Gate PASS: wizard reopens from Settings and from the tray menu.",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "gate-wizard-fake-data-source",
            description: "Gate (A3), source audit: wizard defaults are placeholders ('No games added yet.', 'Waiting for the wheel…', connect-done=false) and simulated data never advances the wizard.",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Gate PASS: wizard shows placeholders, never fake live values.",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "gate-loading-source",
            description: "Gate (B5/A9), source audit: IPC calls bounded (500 ms call, 2 s connect), poll loop uses the bounded call, loading/spinner markers present in the UI. Runtime bound is proven by gate-ipc-timeout.",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Gate PASS: bounded IPC + loading-state wiring present.",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "gate-hover-source",
            description: "Gate (A5), source audit: Btn changes border/background on hover/press with a pointer cursor; hover bindings exist across components and screens. Rendered proof is user-PC verified.",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Gate PASS: hover/pressed feedback bindings present.",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "gate-theme-source",
            description: "Gate (A8), source audit: Theme global has mode (0 Dark / 1 Light / 2 Auto) with a real Light palette; Settings forwards the choice; Rust persists/restores it. Rendered proof lives in rsh-gate-ui (ui-theme-switch).",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Gate PASS: theme mode binding + Light palette + persist wiring present.",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "gate-overflow-source",
            description: "Gate (A7), source audit: the Backup and updates card wraps its sub-text and gives the text column the free width, so the Check button cannot be pushed out of the card.",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Gate PASS: update rows wrap + stretch, button stays in-card.",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "gate-notifications-source",
            description: "Gate (B6), source audit: NotificationCenter is wired into the UI shell and app-core commands surface typed UserError, so failures notify visibly instead of failing silently.",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Gate PASS: notification delivery path present.",
                min_duration_s: 0.0,
            }],
        },
        // ---- Cards layout fix (2026-11-19): Extras ON/OFF size + responsive gaps ----
        Scenario {
            id: "gate-extras-card-size-source",
            description: "Gate (cards fix, Bug 2), source audit: the 4 Extras module cards keep the OFF note and the ON body in the tree with `visible` (no `if` branches), so a card is the same size ON and OFF instead of collapsing when off.",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Gate PASS: Extras cards use `visible` (no `if` size swap).",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "gate-card-gap-source",
            description: "Gate (cards fix, Bug 1), source audit: inter-card gaps are spacer Rectangles (min-width = design gap, stretch 0.05) and growing cards use `min-width` (not explicit `width`), so gaps and cards scale with the window.",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Gate PASS: responsive gap spacers + min-width cards present.",
                min_duration_s: 0.0,
            }],
        },
        // ---- v2.0.2 gate checks, re-scan findings (B-1, B-2, H-3, H-4, M-9) ----
        Scenario {
            id: "gate-pairing-long-call",
            description: "Gate (B-1): begin_pairing is a long call (daemon waits up to 3 s) and must NOT time out on the 500 ms IPC bound — it must ack within 500 ms and the countdown must arrive via pairing_status; no-board begin_pairing fails fast.",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Gate PASS: pairing acks fast, countdown arrives, no 500 ms timeout failure.",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "gate-hid-uac-dwell",
            description: "Gate (B-2): HID switch UAC dwell — spawn/hello wait covers 10 s + user thinking time (>= 60 s); decline yields one clear message, no retry loop, nothing installed.",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Gate PASS: UAC dwell covered; decline is a single message, no retry.",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "gate-bindings-ui",
            description: "Gate (H-3): Buttons screen Save reaches list_bindings/set_binding/clear_binding — a saved binding exists in the backend afterwards. Backend + IPC server are ready; the UI save handler + IPC client forwarding are the gap.",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Gate PASS: Buttons Save persists a binding through the backend.",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "gate-ota-flash-caller",
            description: "Gate (H-4): an 'Update firmware' flow exists in Settings/wizard that starts a flash from the bundled firmware-signed.bin with progress + confirmed version; the caller entry point is proven end-to-end on the fake board.",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Gate PASS: UI update-firmware flow wired; caller flashes + confirms on the fake board.",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "gate-version-bump",
            description: "Gate (M-9): the workspace Cargo.toml version, MainWindow app-version, and Settings app-version all agree (the gate reads the version from Cargo.toml, so no literal to bump; release workflow tag check is fail-closed on the Cargo.toml version).",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Gate PASS: workspace version agrees everywhere it is displayed or checked.",
                min_duration_s: 0.0,
            }],
        },
        // ---- v2.0.3 lifecycle checks (F17): real daemon/UI processes under Xvfb ----
        Scenario {
            id: "gate-respawn-bound",
            description: "Gate (F17.1), runtime: crash-on-start daemon + UI health monitor 300 s — spawns <= 6, non-decreasing gaps (backoff 1/5/30/60 s), latch observed via the 'RSH stopped — automatic restart off' notification, UI alive. Pre-fix: 17 spawns in 185 s, flat ~11 s cadence.",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Gate PASS: respawn bounded + backoff grows + latch stops the loop.",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "gate-ipc-bind-failure",
            description: "Gate (F17.2), runtime: real daemon against a pre-bound socket — exactly 5 bind attempts, exit(1), daemon-status.json code IpcBindFailed; the UI spawns 0 daemons in 30 s while the fatal is fresh. Pre-fix: UI respawned ~3x in 30 s.",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Gate PASS: 5 bind attempts then fatal; UI does not respawn on a fresh fatal.",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "gate-hung-daemon",
            description: "Gate (F17.3), runtime: real daemon SIGSTOPed — IPC ping fails, second daemon exits(1) DaemonHung, exactly 1 daemon, UI spawns 0 in 45 s; after SIGCONT the UI reconnects with no duplicate daemon.",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Gate PASS: hung daemon classified DaemonHung; no takeover, no respawn loop, clean resume.",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "gate-already-running-race",
            description: "Gate (F17.4), runtime: 8 daemons at once x3 rounds — exactly 1 First, rest exit 0 (O_EXCL pid claim); 2 UIs at once — exactly 1 window, second exits 0.",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Gate PASS: exactly one daemon and one UI window under simultaneous launch.",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "gate-xvfb-hidden-shown",
            description: "Gate (F17.5), runtime: real daemon + UI, 10x X-level hide/show (xdotool unmap/map) — same UI pid + daemon pid, reopen < 1 s, zero daemon spawns while hidden. NOTE: the app-driven show() path panics under Xvfb in winit's draw (F17-HS1, X11-no-WM artifact); this check uses X-level unmap/map.",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Gate PASS: hide/show keeps the same processes, reopen < 1 s, no respawns.",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "gate-rapid-reconnect",
            description: "Gate (F17.6), runtime: 20 hide/show cycles ~200 ms apart — after the burst exactly 1 daemon, 1 UI, IPC answers, no zombies, pid files name the live processes, spawns <= 2.",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Gate PASS: rapid cycles leave one daemon, one UI, live IPC, no zombies.",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "gate-frame-budget",
            description: "Gate (F17.8), runtime: real UI under Xvfb vs dead / silent / 300 ms-slow daemon — window present at every 100 ms sample, input ack < 1 s (windowclose, incl. mid seed-fetch for the slow case), exit within 5 s of --quit-after (dead/silent only — the slow close lands mid seed-fetch and exposes the F17-FINDING-5 SIGSEGV, recorded not asserted). Tick p99 is NOT directly measured (no in-process tick log — external proxies only).",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Gate PASS: degraded daemons never freeze the UI past 1 s of input ack.",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "gate-toast-dedupe-integration",
            description: "Gate (F17.9), runtime: flap daemon (answers 3 pings, dies) + UI for 90 s — DaemonDown toasts <= 2, total notification rows <= 5, no tight respawn loop (spawns <= 20). Pre-fix: ~5+ DaemonDown rows in 90 s.",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Gate PASS: flapping folds into <= 2 DaemonDown toasts, spawns bounded.",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "gate-lifecycle-soak",
            description: "Gate (F17.10), runtime: 300 s soak — every 5 s sample daemon == 1, UI == 1, IPC ping true (except <15 s recovery after a daemon SIGKILL at 180 s); X hide/show at 120 s and 240 s. Tray icons == 1 is Windows-only (n/a on Linux).",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Gate PASS: 300 s soak stable, kill recovered within 15 s, spawns bounded.",
                min_duration_s: 0.0,
            }],
        },
        Scenario {
            id: "gate-tray-popup-timeline",
            description: "Gate (F17.7), source audit: the daemon drives the tray popup through PopupDismissPoll + TrayToggleGuard (F9), and tray_popup.rs carries the F17 matrix cases (transient baseline back by 700 ms stays open; 200-250 ms activation blip stays open; sustained foreground loss at 1500 ms dismisses).",
            durations: (0, 0, 0),
            wheel: WheelConfig::default(),
            harness: HarnessConfig::default(),
            assertions: vec![Assertion {
                metric: "selfCheck",
                op: "==",
                value: MetricValue::Str("pass"),
                note: "Gate PASS: popup dismissal uses the F9 state machine + F17 matrix cases present.",
                min_duration_s: 0.0,
            }],
        },
    ]
}
