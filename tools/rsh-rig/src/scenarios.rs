//! Scenario registry — the 26 C# rig scenarios, same IDs, same
//! durations, same assertion shapes (`TESTING-INPUT.md` §8.4,
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
//!   measure tick nanoseconds separately (`docs/PERF-H2.md`).
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

/// The full registry, in C# order.
#[must_use]
pub fn registry() -> Vec<Scenario> {
    vec![
        Scenario {
            id: "soak",
            description: "Clean 60 s / 10 min soak at 200 Hz (quick/full).",
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
                num("currentRatePerSecond", ">=", 195.0, "SLO: ~200 pps"),
                num(
                    "tickJitterP99Us",
                    "<=",
                    5000.0,
                    "PC-receive inter-arrival jitter p99 (plan SLO basis); sandbox threshold 5000 us, real target 1000 us. Engine board-ts jitter is engineTickJitterP99Us.",
                ),
                num(
                    "outputLatencyP99Us",
                    "<=",
                    2000.0,
                    "Rust: engine-measured output-latency p99 replaces C# avgTickUs (see header).",
                ),
                ctrl_accepted(),
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
                    "engineTicks",
                    ">=",
                    3000.0,
                    "Engine kept ticking in the storm (20 s x 200 = 4000, 25% margin)",
                ),
                ctrl_accepted(),
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
                num("validPackets", ">", 1000.0, "Valid flow during calibration"),
                ctrl_accepted(),
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
                ctrl_accepted(),
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
                ctrl_accepted(),
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
                ctrl_accepted(),
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
                    "outputLatencyP99Us",
                    "<=",
                    2000.0,
                    "Rust: output-latency p99 replaces C# avgTickUs (see header).",
                ),
                ctrl_accepted(),
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
                ctrl_accepted(),
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
                ctrl_accepted(),
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
                ctrl_accepted(),
            ],
        },
        Scenario {
            id: "long-soak",
            description: "Long soak: 60 s quick / 10 min full / 4 h long (release blocker).",
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
                num("currentRatePerSecond", ">=", 195.0, "SLO: ~200 pps"),
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
                    "SLO: E2E p99 <= 10 ms Wi-Fi (loopback measurement — see PERF-H2.md). p99 over ~201 samples is the 3rd slowest, so one VM stall cannot fail it.",
                ),
                ctrl_accepted(),
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
                ctrl_accepted(),
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
            ],
        },
    ]
}
