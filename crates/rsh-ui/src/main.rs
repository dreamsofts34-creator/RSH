//! RSH v2.0 — Phase 3 shell + components (REDESIGN-PLAN §3-§6),
//! v2.0.1 hotfix: UI core (daemon self-heal, first-run wizard,
//! wired Settings / Pedals / Buttons, diagnostics drawer, theme
//! persist) + UI-FINISH deep-scan fixes (see the fix notes on each
//! function: A1–A25, B1–B7, C1–C8, D, E) + v2.0.2 Round 2A
//! (daemon-down honesty + hang/startup fixes: B1–B4, H1, H5, H10).
//!
//! Binary driver for the Slint main window. Data flows through the
//! real `rsh_api::AppApi` trait behind `Arc<dyn AppApi>`: the live
//! [`crate::ipc_api::IpcApi`] when the Phase 2 daemon answers on its
//! IPC socket, the simulated [`crate::sim_api::SimApi`] otherwise
//! (see `live_probe.rs`). The API handle is swappable at runtime
//! ([`LiveState`]): Reconnect re-probes (spawning the daemon when
//! it is not running) and swaps live data in without restarting.
//! Steering slider moves push live to the daemon through
//! [`AppApi::set_tuning`] (L4): the cached stored record supplies
//! min/max mV, the sliders supply deadzone/curve/smoothing/invert/
//! lock, and Save persists the draft via [`AppApi::save_draft`].
//!
//! Honesty rule (UI-FINISH acceptance): shipped UI shows no
//! simulated or hardcoded live-looking value. No daemon means
//! "Disconnected" + "—" placeholders, never fake Online; every
//! button either does real work or is disabled with a short reason.
//! User-facing text never says "daemon" / "Simulated data" — the
//! background service is called by its plain name.
//!
//! Startup (B4): the window shows first with a "Connecting…"
//! state; a background health-monitor thread probes (spawning the
//! daemon when needed) and swaps live data in when it answers.
//! The same thread pings the daemon every second (B2): on a death
//! it clears stale values, shows Disconnected + Reconnect, and
//! re-probes in the background until the daemon is back.
//!
//! CLI: --page overview|steering|pedals|buttons|extras|settings|about|setup
//!      --static            render once, no simulation timer
//!      --quit-after SECONDS
//!      --data-dir DIR      test-only: daemon data dir for the IPC socket
//!
//! Timing / event logs go to stderr as `RSH-UI ...` lines, but
//! only with `--verbose` (DS-79): the UI thread never does an
//! unconditional `eprintln!` per event.
#![allow(
    missing_docs,
    reason = "slint::include_modules! generates undocumented getters/setters"
)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod curve;
mod file_picker;
mod ipc_api;
mod live_probe;
mod notifications;
mod sim_api;
mod ui_lifecycle;
mod ui_log;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rsh_api::connection::ConnectionState;
use rsh_api::tuning::snap_steering_lock_deg;
use rsh_api::{
    app_version, ApiTransport, AppApi, AppEvent, CalibrationInfo, ConnectionError, ConnectionInfo,
    FirmwareCheck, FirmwareUpdateStatus, GameSummary, LedPattern, ProfileSummary, ShifterGate,
    SnapshotInfo, TuningSettings, UserError,
};
use slint::{Color, ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel};

use crate::curve::{crosshair_commands, curve_commands, curve_output, dashed_line_commands};
use crate::ipc_api::IpcApi;
use crate::notifications::{sync_to_window, NotificationCenter};
use crate::sim_api::SimApi;

slint::include_modules!();

/// Verbose UI logging (DS-79 / R2-41): the UI thread never does
/// an unconditional `eprintln!` per event — stderr is invisible
/// under `windows_subsystem = "windows"` and an unbuffered write
/// per event is pure UI-thread cost. `ui_log!` writes to stderr
/// only with `--verbose`; without it even the `format!` is skipped
/// (the macro expands to an `if` around the `eprintln!`). Set once in
/// `main` from the parsed args; tests leave it off.
///
/// Phase 2 (L-1): every `ui_log!` line ALSO goes to the UI log
/// file (`<data_dir>/logs/rsh-ui-YYYYMMDD.log`, always-on — the
/// file is the evidence on the user's PC, where there is no
/// terminal and no `--verbose`). Perf (F-7): the file write is
/// buffered (one held-open handle, flushed every 64 lines / at
/// exit — see `ui_log.rs`), not one open/write/flush/close per
/// line; tests never init the file log, so it no-ops there.
static UI_VERBOSE: AtomicBool = AtomicBool::new(false);

macro_rules! ui_log {
    ($($arg:tt)*) => {
        {
            // The file log needs the formatted line even when
            // stderr is off — format once, use twice.
            let __msg = format!($($arg)*);
            crate::ui_log::ui_file_log(&__msg);
            if UI_VERBOSE.load(Ordering::Relaxed) {
                eprintln!("{__msg}");
            }
        }
    };
}

/// View-box geometry of the steering curve (see `curve.rs`).
const VX0: f32 = 30.0;
/// View-box geometry of the steering curve.
const VX1: f32 = 290.0;
/// View-box geometry of the steering curve.
const VY1: f32 = 190.0;

/// Live chart history (20 Hz × 10 s, "Last 10 s").
const CHART_SAMPLES: usize = 200;

/// Default 12 wheel-button actions (local display defaults until
/// the button-mapping backend lands — see the SEAM note on
/// [`Ctx::capture_available`]).
const DEFAULT_BUTTON_ACTIONS: [&str; 12] = [
    "Shift up",
    "Shift down",
    "Horn",
    "Gear 1",
    "Gear 2",
    "Gear 3",
    "Gear 4",
    "Gear 5",
    "Gear 6",
    "Handbrake",
    "Not set",
    "Not set",
];

/// Pedal feel exponents: Linear 1.0, Soft 0.7, Sharp 1.5 (the curve
/// field of the pedal's stored tuning record).
const FEEL_EXPONENTS: [f32; 3] = [1.0, 0.7, 1.5];

/// Rewrite user-facing "daemon" wording (C1/L3): the background
/// service is called "RSH", never "daemon" / "Simulated data".
#[must_use]
pub fn user_text(s: &str) -> String {
    s.replace("Daemon", "RSH")
        .replace("daemon", "RSH")
        .replace("Simulated data", "Live data unavailable")
}

/// One mV reading as shown in the range blocks ("410 mV", "—" for
/// the neutral 0 mV that means "no reading yet" — the neutral record
/// is 0..3300 mV, so a stored 0 minimum is not a real reading).
#[must_use]
pub fn mv_text(mv: f64) -> String {
    if mv <= 0.0 {
        "—".to_string()
    } else {
        format!("{mv:.0} mV")
    }
}

/// Pedal index (0 throttle, 1 brake, 2 clutch — see
/// `ui/screens/pedals.slint`) → capture pedal key + display name.
fn pedal_key(index: i32) -> Option<(&'static str, &'static str)> {
    match index {
        0 => Some(("accel", "Accelerator")),
        1 => Some(("brake", "Brake")),
        2 => Some(("clutch", "Clutch")),
        _ => None,
    }
}

/// A percent as shown in the value boxes ("2%").
#[must_use]
pub fn pct_text(pct: f64) -> String {
    format!("{pct:.0}%")
}

/// Mini-curve path commands for a pedal card (110×110 viewBox):
/// quadratic from (6,104) to (104,6); the control point bows up for
/// Soft (more output early) and down for Sharp (progressive).
#[must_use]
pub fn pedal_curve_commands(_deadzone_pct: f32, exponent: f32) -> String {
    let cy = 55.0 + 30.0 * (exponent - 1.0);
    let cy = cy.clamp(40.0, 75.0);
    format!("M 6 104 Q 55 {cy:.0} 104 6")
}

/// Live marker position on a pedal mini-curve for the current pedal
/// percent (input → curve output, deadzone applied).
#[must_use]
pub fn pedal_marker(pct: f32, deadzone_pct: f32, exponent: f32) -> (f32, f32) {
    let out = curve_output(pct, deadzone_pct, exponent);
    (6.0 + pct / 100.0 * 98.0, 104.0 - out / 100.0 * 98.0)
}

// --- Pedal ramps/curves per-module tuning ---
/// Press/release ramp slider range in ms for a pedal index
/// (0 throttle, 1 brake, 2 clutch): throttle 0..500 both ways,
/// brake press 0..300, clutch release 0..1000. A pedal with no
/// slider on that side reports 0.
#[must_use]
pub fn pedal_ramp_range_ms(index: usize, press: bool) -> f64 {
    match (index, press) {
        (0, _) => 500.0,
        (1, true) => 300.0,
        (2, false) => 1000.0,
        _ => 0.0,
    }
}

/// A ramp value in ms as a slider fraction (0..1) + text ("120 ms").
#[allow(clippy::cast_possible_truncation)]
#[must_use]
pub fn ramp_frac_text(ms: f64, max_ms: f64) -> (f32, String) {
    ((ms / max_ms).clamp(0.0, 1.0) as f32, format!("{ms:.0} ms"))
}

/// A curve exponent (0.5..2.0) as a slider fraction (0..1) + text.
#[allow(clippy::cast_possible_truncation)]
#[must_use]
pub fn curve_frac_text(exponent: f64) -> (f32, String) {
    (
        ((exponent - 0.5) / 1.5).clamp(0.0, 1.0) as f32,
        format!("{exponent:.2}"),
    )
}

/// Tile code letters from a game name (H6): first letters of up to
/// three letter-starting words, uppercased — "Assetto Corsa" → "AC",
/// "Assetto Corsa Competizione" → "ACC" (no more AC/ACC collision),
/// "Dirt Rally 2.0" → "DR" ("2.0" starts with a digit, skipped);
/// a single word gives its first two letters, case preserved
/// ("iRacing" → "iR"). An all-caps abbreviation as the first word
/// (2–3 letters) is taken whole, plus the next word's initial
/// ("AC Competizione" → "ACC").
#[must_use]
pub fn game_code(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    if words.len() >= 2 {
        let first = words[0];
        if first.len() >= 2 && first.len() <= 3 && first.chars().all(|c| c.is_ascii_uppercase()) {
            let second = words[1..]
                .iter()
                .find(|w| w.chars().next().is_some_and(char::is_alphabetic));
            return match second {
                Some(w) => {
                    format!("{}{}", first, w.chars().next().unwrap_or_default()).to_uppercase()
                }
                None => first.to_string(),
            };
        }
        words
            .iter()
            .filter(|w| w.chars().next().is_some_and(char::is_alphabetic))
            .take(3)
            .filter_map(|w| w.chars().next())
            .collect::<String>()
            .to_uppercase()
    } else {
        name.chars().take(2).collect::<String>()
    }
}

/// Display name ↔ backend action id for button/knob bindings (H-3 /
/// B1 wiring): the Buttons screen shows display names ("Shift up",
/// "Gear 3"), the daemon stores ids (`shifter_up`, `gear_3`).
/// "Not set" (and anything unknown) maps to no id — saving it clears
/// the binding.
#[must_use]
pub fn action_id(display: &str) -> Option<&'static str> {
    match display {
        "Shift up" => Some("shifter_up"),
        "Shift down" => Some("shifter_down"),
        "Horn" => Some("horn"),
        "Gear 1" => Some("gear_1"),
        "Gear 2" => Some("gear_2"),
        "Gear 3" => Some("gear_3"),
        "Gear 4" => Some("gear_4"),
        "Gear 5" => Some("gear_5"),
        "Gear 6" => Some("gear_6"),
        "Handbrake" => Some("handbrake"),
        _ => None,
    }
}

/// Backend action id → display name (H-3): the inverse of
/// [`action_id`]. App actions have display names too ("Next
/// profile") even though the Buttons screen offers no chip for
/// them — a stored app-action binding still reads correctly.
#[must_use]
pub fn action_display(id: &str) -> &'static str {
    match id {
        "shifter_up" => "Shift up",
        "shifter_down" => "Shift down",
        "horn" => "Horn",
        "gear_1" => "Gear 1",
        "gear_2" => "Gear 2",
        "gear_3" => "Gear 3",
        "gear_4" => "Gear 4",
        "gear_5" => "Gear 5",
        "gear_6" => "Gear 6",
        "handbrake" => "Handbrake",
        "profile_next" => "Next profile",
        "profile_previous" => "Previous profile",
        _ => "Not set",
    }
}

/// A game slug ("`assetto_corsa`") as a display name ("Assetto Corsa").
#[must_use]
pub fn pretty_game_slug(slug: &str) -> String {
    slug.split('_')
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<String>>()
        .join(" ")
}

/// Polyline path commands for a live chart (380×90 viewBox) from a
/// ring buffer: x spreads the samples across the width, y maps
/// `min_v..=max_v` to the height. Empty when fewer than 2 samples —
/// no line is drawn until there is real data.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn chart_commands(samples: &VecDeque<f32>, min_v: f32, max_v: f32) -> String {
    if samples.len() < 2 {
        return String::new();
    }
    let n = samples.len();
    let span = (max_v - min_v).max(f32::EPSILON);
    let mut out = String::new();
    for (i, v) in samples.iter().enumerate() {
        let x = i as f32 / (n - 1) as f32 * 380.0;
        // y spans 85 (min) .. 5 (max) inside the 90px viewBox.
        let y = 85.0 - (v.clamp(min_v, max_v) - min_v) / span * 80.0;
        if i == 0 {
            let _ = write!(out, "M {x:.1} {y:.1}");
        } else {
            let _ = write!(out, " L {x:.1} {y:.1}");
        }
    }
    out
}

/// Several traces in ONE path-commands string (R2-10 / pedals live
/// graphs): the Diagnostics "Pedals" chart is a single Slint `Path`
/// (one `pedals-chart` prop, one accent stroke), so the throttle /
/// brake / clutch histories are drawn as separate `M…L…` subpaths
/// in one string — all three traces update on the same 20 Hz
/// `push_frame` path as the steering chart. A history with fewer
/// than 2 samples contributes nothing; empty when no trace has data.
#[must_use]
pub fn multi_chart_commands(histories: &[&VecDeque<f32>], min_v: f32, max_v: f32) -> String {
    let mut out = String::new();
    for history in histories {
        let trace = chart_commands(history, min_v, max_v);
        if !trace.is_empty() {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(&trace);
        }
    }
    out
}

/// LED swatch palette (M2, `extras.slint` order): the six colours
/// the user can pick, as 0xRRGGBB. Index 3 is the historic accent
/// blue — the default when no colour is stored yet.
pub const LED_PALETTE: [u32; 6] = [
    0xFF_6B_6B, 0xF5_B8_4A, 0x3D_D6_8C, 0x2F_6B_FF, 0xB4_6C_FF, 0xFF_FF_FF,
];

/// LED period (L1) from a normalised slider value: 0..1 maps to
/// 100..5000 ms (the firmware's blink/chase/rainbow cycle range).
#[must_use]
pub fn led_period_ms_from_slider(v: f64) -> f64 {
    100.0 + v.clamp(0.0, 1.0) * 4900.0
}

/// Horn minimum honk (HO1) from a normalised slider value: 0..1
/// maps to 100..1000 ms.
#[must_use]
pub fn horn_min_ms_from_slider(v: f64) -> f64 {
    100.0 + v.clamp(0.0, 1.0) * 900.0
}

/// A custom LED colour (L2) from normalised RGB channel sliders
/// (0..1 each) — the full-RGB value the protocol takes.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn led_color_from_channels(r: f32, g: f32, b: f32) -> u32 {
    let channel = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u32;
    (channel(r) << 16) | (channel(g) << 8) | channel(b)
}

/// Static LED strip preview (8 circles) from pattern + brightness +
/// colour (M2): Off is dark; any lit pattern shows the STORED colour
/// at the set brightness (a still preview, not a live strip — the
/// strip itself is not readable through the API). `None` keeps the
/// historic accent blue (no colour stored yet).
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn led_preview(pattern: LedPattern, brightness: u8, color: Option<u32>) -> Vec<Color> {
    if pattern == LedPattern::Off {
        return vec![Color::from_rgb_u8(0x1D, 0x25, 0x33); 8];
    }
    let b = f32::from(brightness) / 100.0;
    let rgb = color.unwrap_or(0x2F_6B_FF);
    let [_, r, g, bl] = rgb.to_be_bytes();
    let (r, g, bl) = (f32::from(r), f32::from(g), f32::from(bl));
    let (br, bg, bb) = (13.0, 17.0, 24.0);
    let mix = |c: f32, base: f32| (base + (c - base) * b).round().clamp(0.0, 255.0) as u8;
    vec![Color::from_rgb_u8(mix(r, br), mix(g, bg), mix(bl, bb)); 8]
}

/// Cached pedal tuning records (A1/A2): the stored record per pedal
/// supplies min/max mV, deadzone and the feel exponent, so the page
/// never shows literals.
/// Perf (F-4): `Clone + PartialEq` so `push_frame` can snapshot
/// the pedal state into [`FrameInputs`] and skip a frame whose
/// inputs did not move.
#[derive(Clone, PartialEq)]
struct PedalCache {
    records: [TuningSettings; 3],
    feel_exp: [f32; 3],
}

impl PedalCache {
    fn new() -> Self {
        Self {
            records: [
                TuningSettings::neutral("throttle"),
                TuningSettings::neutral("brake"),
                TuningSettings::neutral("clutch"),
            ],
            feel_exp: [1.0; 3],
        }
    }

    /// Feel chip index from a stored curve exponent.
    fn feel_index(exp: f32) -> i32 {
        if exp < 0.95 {
            1
        } else if exp > 1.05 {
            2
        } else {
            0
        }
    }

    /// Curve exponent for a feel chip index (0 Linear, 1 Soft, 2 Sharp).
    fn feel_exponent(feel: i32) -> f32 {
        FEEL_EXPONENTS
            .get(usize::try_from(feel).unwrap_or(0))
            .copied()
            .unwrap_or(1.0)
    }
}

/// UI-side tuning state (deadzone / curve / preset).
///
/// This is deliberately separate from the `AppApi` handle: in Phase 3
/// the sliders edit local state and mark the profile unsaved; pushing
/// the values into the engine lands in Phase 4.
struct Tuning {
    /// Centre deadzone, percent (0..30 — Bug 7: the Basic cap was
    /// 10%, the core contract allows 30%).
    deadzone_pct: f32,
    /// Response-curve exponent (1.0..2.0 — A24: the core contract
    /// caps the curve at 2.0, so the UI slider runs to 2.0 too).
    exponent: f32,
    /// 0 soft, 1 balanced, 2 sharp, 3 custom.
    preset: i32,
    /// Smoothing, percent (0..100 — Bug 7 Advanced slider).
    smoothing_pct: f32,
    /// Invert steering (Bug 7 Advanced toggle, default off).
    invert: bool,
    /// Steering lock in degrees (Bug 7 Advanced slider: one of the
    /// 270/360/540/720/900° steps, default 900°).
    lock_deg: f32,
    /// Left/right lock balance, percent (−20..+20 — S2 Advanced
    /// slider, default 0 = even; positive favours the left).
    balance_pct: f32,
    /// Centre trim, percent (−5..+5 — S3 Advanced slider,
    /// default 0 = no trim).
    trim_pct: f32,
}

impl Tuning {
    /// Build from a stored steering record (L4 startup): deadzone /
    /// curve come from the daemon, the preset is derived when the
    /// values match a preset exactly, else custom (3).
    ///
    /// Casts are safe here: both values are clamped to the UI range
    /// first, so no truncation can occur in practice.
    #[allow(clippy::cast_possible_truncation)]
    fn from_settings(settings: &TuningSettings) -> Self {
        let deadzone_pct = settings.deadzone.clamp(0.0, 30.0) as f32;
        let exponent = settings.curve.clamp(1.0, 2.0) as f32;
        let preset = match (deadzone_pct, exponent) {
            (3.0, 1.0) => 0,
            (2.0, 1.4) => 1,
            (1.0, 2.0) => 2,
            _ => 3,
        };
        Self {
            deadzone_pct,
            exponent,
            preset,
            smoothing_pct: settings.smoothing.clamp(0.0, 100.0) as f32,
            invert: settings.invert.unwrap_or(false),
            lock_deg: snap_steering_lock_deg(settings.steering_lock_deg.unwrap_or(900.0)) as f32,
            // --- Steering per-module tuning ---
            balance_pct: settings
                .steering_balance_pct
                .unwrap_or(0.0)
                .clamp(-20.0, 20.0) as f32,
            trim_pct: settings
                .steering_center_trim_pct
                .unwrap_or(0.0)
                .clamp(-5.0, 5.0) as f32,
        }
    }

    /// Build the daemon record to push (L4): the cached stored
    /// record supplies min/max mV and the extension fields; the
    /// sliders supply deadzone/curve/smoothing/invert/lock. The core
    /// contract caps the curve at 2.0 (A24) — the slider range
    /// matches, so the clamp here is only a guard.
    fn to_settings(&self, base: &TuningSettings) -> TuningSettings {
        TuningSettings {
            module: "steering".to_string(),
            deadzone: f64::from(self.deadzone_pct),
            curve: f64::from(self.exponent).clamp(0.5, 2.0),
            smoothing: f64::from(self.smoothing_pct),
            invert: Some(self.invert),
            steering_lock_deg: Some(f64::from(self.lock_deg)),
            // --- Steering per-module tuning ---
            steering_balance_pct: Some(f64::from(self.balance_pct)),
            steering_center_trim_pct: Some(f64::from(self.trim_pct)),
            ..base.clone()
        }
    }

    fn set_deadzone(&mut self, pct: f32) {
        self.deadzone_pct = pct.clamp(0.0, 30.0);
    }

    fn set_exponent(&mut self, exp: f32) {
        self.exponent = exp.clamp(1.0, 2.0);
    }

    /// Smoothing in percent (0..100).
    fn set_smoothing(&mut self, pct: f32) {
        self.smoothing_pct = pct.clamp(0.0, 100.0);
    }

    /// Invert steering on/off.
    fn set_invert(&mut self, invert: bool) {
        self.invert = invert;
    }

    /// Steering lock in degrees, snapped to the nearest step
    /// (270/360/540/720/900°). The snap lands on a whole step, so
    /// the `f64` → `f32` cast loses nothing in practice.
    #[allow(clippy::cast_possible_truncation)]
    fn set_lock_deg(&mut self, deg: f32) {
        self.lock_deg = snap_steering_lock_deg(f64::from(deg)) as f32;
    }

    /// Left/right lock balance in percent (−20..+20; positive
    /// favours the left side).
    fn set_balance(&mut self, pct: f32) {
        self.balance_pct = pct.clamp(-20.0, 20.0);
    }

    /// Centre trim in percent (−5..+5).
    fn set_trim(&mut self, pct: f32) {
        self.trim_pct = pct.clamp(-5.0, 5.0);
    }

    /// Apply a preset (0 soft, 1 balanced, 2 sharp; 3 custom keeps values).
    fn set_preset(&mut self, preset: i32) {
        self.preset = preset;
        match preset {
            0 => {
                self.exponent = 1.0;
                self.deadzone_pct = 3.0;
            }
            1 => {
                self.exponent = 1.4;
                self.deadzone_pct = 2.0;
            }
            2 => {
                self.exponent = 2.0;
                self.deadzone_pct = 1.0;
            }
            _ => {}
        }
    }

    fn reset_defaults(&mut self) {
        self.set_preset(1);
        // Bug 7: Reset restores the whole steering tuning, not just
        // the Basic feel — smoothing 30%, not inverted, full lock.
        self.smoothing_pct = 30.0;
        self.invert = false;
        self.lock_deg = 900.0;
        // --- Steering per-module tuning ---
        self.balance_pct = 0.0;
        self.trim_pct = 0.0;
    }
}

// CLI flags: four bools is the struct's whole job (same allow as
// `commands.rs` / daemon `main.rs` flag structs).
#[allow(clippy::struct_excessive_bools)]
struct Args {
    page: i32,
    quit_after: Option<u64>,
    static_mode: bool,
    /// First-time setup wizard instead of the shell layout.
    setup: bool,
    /// Test-only override for the daemon data dir (IPC socket path).
    data_dir: Option<std::path::PathBuf>,
    /// Verbose: log live events to stderr (DS-79). Off by default —
    /// the UI thread never does an `eprintln!` per event.
    verbose: bool,
    /// Relaunched by WER after a crash (Round 3 WS-6, R3-AK-04):
    /// the restart registration's crash-loop guard reads this.
    wer_restart: bool,
}

fn parse_args() -> Args {
    let mut a = Args {
        page: 0,
        quit_after: None,
        static_mode: false,
        setup: false,
        data_dir: None,
        verbose: false,
        wer_restart: false,
    };
    let argv: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < argv.len() {
        match argv[i].as_str() {
            "--page" => {
                if i + 1 < argv.len() {
                    // "setup" is a mode, not a page: it swaps the
                    // whole shell layout for the SetupWizard.
                    if argv[i + 1].as_str() == "setup" {
                        a.setup = true;
                    }
                    a.page = match argv[i + 1].as_str() {
                        "steering" => 1,
                        "pedals" => 2,
                        "buttons" => 3,
                        "extras" => 4,
                        "settings" => 5,
                        "about" => 6,
                        _ => 0,
                    };
                    i += 1;
                }
            }
            "--quit-after" => {
                if i + 1 < argv.len() {
                    a.quit_after = argv[i + 1].parse().ok();
                    i += 1;
                }
            }
            "--data-dir" => {
                if i + 1 < argv.len() {
                    a.data_dir = Some(std::path::PathBuf::from(&argv[i + 1]));
                    i += 1;
                }
            }
            "--static" => a.static_mode = true,
            "--verbose" => a.verbose = true,
            "--wer-restart" => a.wer_restart = true,
            other => eprintln!("RSH-UI unknown arg: {other}"),
        }
        i += 1;
    }
    a
}

/// Read the persisted UI theme choice (0 Dark, 1 Light, 2 Auto)
/// from `<data_dir>/ui-theme` ("Dark"/"Light"/"Auto", default
/// Dark). The returned index is the STORED choice — callers resolve
/// Auto via [`resolve_theme_mode`] before setting the window theme.
fn read_theme(data_dir: Option<&PathBuf>) -> i32 {
    let Some(dir) = data_dir else {
        return 0;
    };
    match std::fs::read_to_string(dir.join("ui-theme")) {
        Ok(text) if text.trim() == "Light" => 1,
        Ok(text) if text.trim() == "Auto" => 2,
        _ => 0,
    }
}

/// Persist the UI theme choice (`settings_idx` is the Settings
/// screen index: 0 Dark, 1 Light, 2 Auto). Returns `true` on
/// success — DS-51: a persist failure is no longer silent
/// (`eprintln` only); the caller notifies and reverts.
/// (DS-49/DS-50: the OS preference itself is cached in
/// [`Ctx::os_light`] — this only writes the choice.)
fn write_theme(data_dir: Option<&PathBuf>, settings_idx: i32) -> bool {
    let Some(dir) = data_dir else {
        return false;
    };
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let text = match settings_idx {
        1 => "Light",
        2 => "Auto",
        _ => "Dark",
    };
    match std::fs::write(dir.join("ui-theme"), format!("{text}\n")) {
        Ok(()) => {
            ui_log!("RSH-UI theme persisted: {text}");
            true
        }
        Err(e) => {
            ui_log!("RSH-UI theme persist failed: {e}");
            false
        }
    }
}

/// Resolve a stored theme choice to the window theme (0 Dark,
/// 1 Light). Dark/Light are used as stored; Auto follows the OS
/// preference (`system_light`). Pure — unit-tested below.
fn resolve_theme_mode(choice: i32, system_light: bool) -> i32 {
    match choice {
        1 => 1,
        2 => i32::from(system_light),
        _ => 0,
    }
}

/// Whether the OS currently prefers a light theme (A8/Auto,
/// Windows): `AppsUseLightTheme` under the Personalize key.
/// Best-effort — any failure (no `reg`, no key) means Dark.
/// R2-03 theme half: `CREATE_NO_WINDOW` — a console-subsystem
/// child spawned without it flashes a console window on every
/// sample (the sampler runs this every 30 s).
#[cfg(windows)]
fn system_light() -> bool {
    // R2-03 (S-02): no console window flashes for the `reg` probe.
    use std::os::windows::process::CommandExt;
    let Ok(out) = std::process::Command::new("reg")
        .creation_flags(0x0800_0000)
        .args([
            "query",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
            "/v",
            "AppsUseLightTheme",
        ])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .output()
    else {
        return false;
    };
    String::from_utf8_lossy(&out.stdout).contains("0x1")
}

/// Whether the OS currently prefers a light theme (A8/Auto):
/// non-Windows has no system light preference wired yet — Dark.
#[cfg(not(windows))]
fn system_light() -> bool {
    false
}

/// Sample the OS light preference on a background thread (DS-49):
/// `reg` must never run on the UI thread, and a synchronous spawn
/// on every theme read froze the UI while `reg` started up. The
/// returned flag is what the UI reads (a cheap atomic load);
/// the sampler re-reads every 30 s, so Theme Auto follows a live
/// OS change within half a minute (DS-50). The thread is detached
/// — it ends with the process.
fn spawn_os_light_sampler() -> Arc<AtomicBool> {
    let shared = Arc::new(AtomicBool::new(system_light()));
    let writer = shared.clone();
    let _ = std::thread::Builder::new()
        .name("rsh-os-light".to_string())
        .spawn(move || loop {
            writer.store(system_light(), Ordering::Relaxed);
            std::thread::sleep(Duration::from_secs(30));
        });
    shared
}

/// Connection label for the top bar / sidebar (DS-67, pure —
/// unit-tested): while the board is rejoining Wi-Fi with the new
/// password after a password set, the link is down on
/// purpose — say so instead of a generic "Disconnected", which
/// reads as if the connection broke.
fn conn_label_for(connecting: bool, live: bool, conn_label: &str, rejoining: bool) -> String {
    if rejoining {
        "Rejoining with the new password…".to_string()
    } else if connecting {
        // B4: the first probe is still running.
        "Connecting…".to_string()
    } else if live {
        conn_label.to_string()
    } else {
        "Disconnected".to_string()
    }
}

/// Swappable data source (v2.0.1 hotfix): the active [`AppApi`]
/// plus the live link. Reconnect re-probes (spawning the daemon when
/// needed) and swaps `api` to live without restarting; the sim is
/// the startup fallback and is dropped once live takes over.
struct LiveState {
    api: Arc<dyn AppApi>,
    live_api: Option<Arc<IpcApi>>,
    live: bool,
    /// True while the first probe is still running (B4): the window
    /// shows "Connecting…" until the health monitor resolves it.
    connecting: bool,
    /// Transport of the last live session (H1): Reconnect re-uses
    /// it (USB stays USB) instead of forcing Wi-Fi.
    last_transport: ApiTransport,
}

/// One tuning field waiting for its debounced daemon push (M-7).
/// (The steering curve is not here: the whole steering record goes
/// through `push_steering`'s debounced push instead.)
#[derive(Clone, Copy, PartialEq)]
enum FieldKind {
    /// Deadzone percent (0..10).
    Deadzone,
    /// Clutch bite point (0..1).
    Bite,
    // --- Brake per-module tuning ---
    /// Brake force / saturation (0.5..=1.0).
    Force,
    /// LED brightness percent (0..100).
    Brightness,
    // --- Pedal ramps/curves per-module tuning ---
    /// Pedal press ramp in ms (throttle 0..500, brake 0..300).
    PressMs,
    /// Pedal release ramp in ms (throttle 0..500, clutch 0..1000).
    ReleaseMs,
    /// Response curve exponent (0.5..2.0).
    Curve,
    /// Throttle saturation as the effective max in mV (the queued
    /// value is already converted from percent by the handler).
    Saturation,
    /// Encoder pulse length in ms, 20..200 (E1).
    EncoderPulseMs,
    /// Encoder acceleration sensitivity, 0..2 (E3).
    EncoderAccelSens,
    // --- Buttons per-module tuning ---
    /// Button debounce in ms, 0..50 (BU1).
    ButtonDebounceMs,
    /// Long-press threshold in ms, 300..1500 (BU2).
    ButtonLongPressMs,
    /// Double-press window in ms, 150..500 (BU3).
    ButtonDoublePressMs,
    // --- Shifter per-module tuning ---
    /// Shift lockout in ms, 0..200 (SH2).
    ShiftLockoutMs,
    // --- Handbrake/clutch per-module tuning ---
    /// Clutch slip-zone width as a fraction, 0.05..=0.40 (C2).
    BiteWidth,
    // --- LED/horn per-module tuning ---
    /// LED pattern-cycle period in ms, 100..5000 (L1).
    LedPeriodMs,
    /// Horn minimum honk in ms, 100..1000 (HO1).
    HornMinMs,
    /// LED colour as 0xRRGGBB (L2, composed from the RGB sliders).
    LedColor,
}

/// A slider drag's latest value, waiting out its debounce (M-7):
/// the slider already moved on screen (the component owns the drag
/// state); only the daemon push waits 150 ms after the last move.
/// The value text / mini-curve lag the drag by at most that long.
/// `PartialEq` (R2-18): a flush completion / timeout only clears or
/// reverts the pending value when it is still the one that was
/// pushed — a newer drag supersedes it instead of being reverted.
#[derive(Clone, Copy, PartialEq)]
struct PendingField {
    module: &'static str,
    kind: FieldKind,
    value: f64,
}

/// What the running [`JobKind::TuningPush`] job is pushing (R2-18):
/// the snapshot taken when the job started, so a timeout can revert
/// exactly that slider — and a completion only clears the pending
/// flag when the pending value is still this snapshot (a newer drag
/// supersedes the push instead of being cleared or reverted by it).
/// `base` is the pre-push stored record the slider snaps back to.
#[derive(Clone, PartialEq)]
enum TuningPushTarget {
    /// Steering push: `due` is the `steer_push_due` instant the
    /// flush consumed; `base` the stored steering record.
    Steering { due: Instant, base: TuningSettings },
    /// Pedal / Extras field push: `pending` (+ `due`) is the queued
    /// value the flush consumed; `base` the stored module record.
    Field {
        pending: PendingField,
        due: Instant,
        base: TuningSettings,
    },
}

/// Content hash (R2-11): one `u64` over the fields a list model (or
/// the slow generation) is built from, so a model is only rebuilt —
/// and the generation only bumped — when its content actually
/// moved. `DefaultHasher` is not stable across releases, but the
/// hash is only ever compared with itself in this process, so that
/// is fine.
fn content_hash(value: &impl std::hash::Hash) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::Hasher;
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

/// Health-monitor verdicts (B2): the background thread in
/// [`spawn_health_monitor`] watches the fast poller's link-dead
/// flag every second and re-probes after a death; the frame timer
/// drains these on the UI thread and applies them — the monitor
/// never touches UI state.
/// (The shared `Daemon` prefix is the point: every variant is a
/// daemon-health verdict.)
#[allow(clippy::enum_variant_names)]
enum HealthEvent {
    /// The live daemon stopped answering (or the first probe found
    /// none): show Disconnected, clear stale values.
    DaemonDown,
    /// A (new) live daemon answered: swap it in.
    DaemonBack(Arc<IpcApi>),
    /// The daemon must NOT be auto-respawned (F3): either a fresh
    /// fatal `daemon-status.json` blocks spawning, or the respawn
    /// budget is exhausted. The UI latches the visible fatal state
    /// (reason + manual Retry) instead of spawning another daemon.
    DaemonFatal { code: String, detail: String },
}

/// Background-job kinds (A9/B5): one entry per long operation, so the
/// frame timer can time each out (never infinite loading) and the UI
/// can ignore a second click while one is in flight.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum JobKind {
    /// Reconnect probe (spawn + ~3 s of retries).
    Reconnect,
    /// Transport switch (`connect`).
    Transport,
    /// Output-backend switch (`set_output_backend`, up to 75 s while
    /// the user answers UAC + the sidecar spawns).
    Backend,
    /// Firmware check (board vs bundled version).
    FirmwareCheck,
    /// App update check (DS-20): GitHub Releases latest tag vs
    /// the running version.
    AppUpdateCheck,
    /// Firmware update start (the flash itself then runs on the
    /// daemon and is polled separately).
    FirmwareUpdate,
    /// Raw-mV availability probe for the wizard Calibrate step.
    RawAvailable,
    /// Slider drag flush (F6, v2.0.3): `set_tuning` runs on a job
    /// thread, never on the UI thread.
    TuningPush,
    /// Profile operations (F6, v2.0.3): activate / create /
    /// duplicate / delete / rename / link / save / discard.
    ProfileOp,
    /// Button / knob mapping (F6, v2.0.3): set / clear binding,
    /// set knob action.
    BindingOp,
    /// Capture (F6, v2.0.3): centre / pedal min-max.
    CaptureOp,
    /// Learn poll (F6, v2.0.3): the 1 Hz `take_learned_button` /
    /// `is_learning` poll while listening. Start / cancel live on
    /// [`JobKind::LearnToggle`] (R2-35), not here.
    LearnOp,
    /// Learn toggle (R2-35): start / cancel ONLY — the 1 Hz poll
    /// job keeps [`JobKind::LearnOp`], so a Learn click while a
    /// poll job is in flight is no longer swallowed by
    /// `run_generic_job`'s duplicate-start rule (it used to share
    /// `LearnOp` with the poll job and was ignored with only an
    /// `eprintln`).
    LearnToggle,
    /// Games (F6, v2.0.3): launch / stop / refresh.
    GameOp,
    /// Settings (F6, v2.0.3): autostart / backup / restore /
    /// output toggle / auto-pause / board address / keep-in-tray /
    /// module enable.
    SettingsOp,
    /// Reconnect seed (F5, v2.0.3): the slow-cache seed fetch
    /// (`LiveSeed::fetch`) runs on a job thread, then applies on
    /// the UI thread.
    ReconnectSeed,
    /// Shifter sensitivity (SH1): the board-config read (one-shot,
    /// on first Extras visit) and the debounced config-transaction
    /// write (stage → commit → confirm). Its OWN kind, so a
    /// sensitivity push is never swallowed by an in-flight
    /// `TuningPush` — and vice versa — by the duplicate-start rule.
    SensitivityPush,
}

impl JobKind {
    /// How long the job may stay busy before the frame timer times
    /// it out with a visible error (B5: loading never infinite).
    /// Backend is long on purpose — the user may take a minute to
    /// answer the UAC prompt while the sidecar spawn waits.
    fn timeout(self) -> Duration {
        match self {
            // The probe and the seed fetch it follows share the
            // 15 s bound (the seed is ~35 round-trips on a live
            // daemon; a dead one fails fast per call).
            JobKind::Reconnect | JobKind::ReconnectSeed => Duration::from_secs(15),
            JobKind::Backend => Duration::from_secs(80),
            // FirmwareUpdate is 10 s on purpose (A9); settings ops
            // share it — they can cross a UAC prompt (HIDMaestro
            // enable), the same bound the old callers tolerated,
            // but now with a visible timeout. The app update check
            // (DS-20) shares it too: it is one bounded network
            // round-trip (`curl --max-time 8`). The sensitivity push
            // shares it as well: it is four board control sessions
            // (read → stage → commit → confirm), not one round-trip.
            JobKind::FirmwareUpdate
            | JobKind::SettingsOp
            | JobKind::AppUpdateCheck
            | JobKind::SensitivityPush => Duration::from_secs(10),
            // Everything else is a couple of round-trips at most
            // (F6: a tuning push is one `set_tuning`).
            JobKind::Transport
            | JobKind::FirmwareCheck
            | JobKind::RawAvailable
            | JobKind::TuningPush
            | JobKind::ProfileOp
            | JobKind::BindingOp
            | JobKind::CaptureOp
            | JobKind::LearnOp
            | JobKind::LearnToggle
            | JobKind::GameOp => Duration::from_secs(5),
        }
    }

    /// Stable order for the busy strip's tie-break: when two jobs
    /// started in the same instant, the earlier-declared kind wins
    /// so the shown label is deterministic.
    fn order(self) -> u8 {
        match self {
            JobKind::Reconnect => 0,
            JobKind::Transport => 1,
            JobKind::Backend => 2,
            JobKind::FirmwareCheck => 3,
            JobKind::FirmwareUpdate => 4,
            JobKind::RawAvailable => 5,
            // F5/F6: a settings or seed job never hides an
            // in-flight reconnect / backend switch.
            JobKind::TuningPush => 7,
            JobKind::ProfileOp => 8,
            JobKind::BindingOp => 9,
            JobKind::CaptureOp => 10,
            JobKind::LearnOp => 11,
            // R2-35: the toggle has its OWN strip slot (17), not
            // LearnOp's — the duplicate-start rule keys on the
            // `JobKind` variant, so the distinct order keeps the
            // tie-break deterministic and the toggle visible as
            // its own job. Slint disables the Learn controls on
            // `busy-kind == 11 || busy-kind == 17`, so the toggle
            // job (start/cancel) dims the button exactly like the
            // poll job instead of leaving it live.
            JobKind::LearnToggle => 17,
            JobKind::GameOp => 13,
            JobKind::SettingsOp => 14,
            JobKind::ReconnectSeed => 15,
            JobKind::AppUpdateCheck => 16,
            JobKind::SensitivityPush => 18,
        }
    }
}

/// The oldest running job (DS-54): the strip shows one job at a
/// time — the one that started first; ties break by
/// [`JobKind::order`]. Returns the kind + its label.
fn oldest_busy_job(jobs: &HashMap<JobKind, (String, Instant)>) -> Option<(JobKind, &str)> {
    let mut best: Option<(Instant, u8, JobKind, &str)> = None;
    for (kind, (label, started)) in jobs {
        let candidate = (*started, kind.order(), *kind, label.as_str());
        let take = best.is_none_or(|(old_started, old_order, _, _)| {
            candidate.0 < old_started || (candidate.0 == old_started && candidate.1 < old_order)
        });
        if take {
            best = Some(candidate);
        }
    }
    best.map(|(_, _, kind, label)| (kind, label))
}

/// Busy-strip state (B5, FC2) for a `busy_jobs` map: `(false, "")`
/// when nothing runs, otherwise `(true, label)` of the OLDEST
/// running job (see [`oldest_busy_job`]).
fn busy_state_for(jobs: &HashMap<JobKind, (String, Instant)>) -> (bool, String) {
    oldest_busy_job(jobs).map_or_else(
        || (false, String::new()),
        |(_, label)| (true, label.to_string()),
    )
}

/// Which job kind the busy strip is showing (DS-54), as the
/// [`JobKind::order`] value the Slint `busy-kind` prop reads
/// (`-1` = idle): screens disable + dim the button that started it,
/// so a second click while its job runs is impossible.
fn busy_kind_for(jobs: &HashMap<JobKind, (String, Instant)>) -> i32 {
    oldest_busy_job(jobs).map_or(-1, |(kind, _)| i32::from(kind.order()))
}

/// Board-address validation (H4, FC2): the same rule the daemon
/// enforces — a private/loopback IPv4 literal only. A hostname
/// such as `rsh.local` or a public IP is not a board address.
fn valid_board_address(address: &str) -> bool {
    address
        .trim()
        .parse::<std::net::Ipv4Addr>()
        .is_ok_and(|ip| ip.is_private() || ip.is_loopback())
}

/// A typed profile name, trimmed — `None` when nothing was typed.
/// Length / character rules live in the backend (`rename_profile`
/// validates them); the UI only rejects an empty name.
fn clean_profile_name(name: &str) -> Option<String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

/// F12 (v2.0.3): a down-episode shorter than this is a flap — the
/// daemon died and came back almost at once, so the "Wheel connected"
/// notify is noise (suppressed by `reconnect_to_live` via
/// [`Ctx::suppress_connected_notify`]) and the episode counts toward
/// the burst total instead of earning its own entry.
const FLAP_DOWN_WINDOW: Duration = Duration::from_secs(10);

/// Wizard step debounce (R2-36): Next/Back clicks closer than
/// this to the last accepted step change are ignored, so a rapid
/// double-click advances exactly one step.
const SETUP_STEP_DEBOUNCE: Duration = Duration::from_millis(300);

/// F12 (v2.0.3): a death within this long after a reconnect belongs
/// to the same flapping burst — its down-notify is collapsed into
/// the burst's single entry instead of one entry per death.
const FLAP_BURST_UPTIME: Duration = Duration::from_secs(30);

/// F12 (v2.0.3): a burst count older than this is stale — a much
/// later, unrelated death must not claim "flapped ×N".
const FLAP_COUNT_STALE: Duration = Duration::from_secs(600);

/// F12 (v2.0.3): whether a down-episode of `down` counts as a flap.
/// `None` (never down since launch — the first connect) is not a
/// flap: a fast first connect is normal, not a flicker.
fn is_flap_down_episode(down: Option<Duration>) -> bool {
    down.is_some_and(|d| d < FLAP_DOWN_WINDOW)
}

/// Daemon-error banner text (M-2, FC2): empty while the daemon is
/// live or no fatal file exists; otherwise the fatal file's plain
/// detail (L3: "daemon" wording rewritten to "RSH"). A fatal file
/// with no detail still shows a plain line instead of an empty
/// banner.
fn daemon_error_for(live: bool, status: Option<(String, String)>) -> String {
    if live {
        return String::new();
    }
    let Some((_code, detail)) = status else {
        return String::new();
    };
    if detail.trim().is_empty() {
        "RSH stopped unexpectedly — try Reconnect.".to_string()
    } else {
        user_text(&detail)
    }
}

/// Write one Extras module `-on` prop by module index (H7, FC2):
/// 0 handbrake, 1 shifter, 2 horn, 3 led. The props are `MainWindow`
/// in-out props, so Rust can revert a toggle the backend rejected.
/// R2-11: change-guarded like `push_frame` — a re-sync that agrees
/// with the current value writes nothing.
fn set_extras_on_prop(win: &MainWindow, index: usize, value: bool) {
    match index {
        0 => set_if_changed(win.get_extras_handbrake_on(), value, |v| {
            win.set_extras_handbrake_on(v);
        }),
        1 => set_if_changed(win.get_extras_shifter_on(), value, |v| {
            win.set_extras_shifter_on(v);
        }),
        2 => set_if_changed(win.get_extras_horn_on(), value, |v| {
            win.set_extras_horn_on(v);
        }),
        _ => set_if_changed(win.get_extras_leds_on(), value, |v| {
            win.set_extras_leds_on(v);
        }),
    }
}

/// Write the B5 busy props, only when they change (so the 1 Hz
/// safety call in the frame timer is free). A transition logs to
/// stderr — the Xvfb smoke run reads the busy on/off off the log.
/// `kind` is the [`JobKind::order`] value for the running job
/// (DS-54: `-1` = idle) — screens disable + dim the button that
/// started it while it runs.
fn set_busy_props(win: &MainWindow, busy: bool, label: &str, kind: i32) {
    if win.get_busy() == busy
        && win.get_busy_label().as_str() == label
        && win.get_busy_kind() == kind
    {
        return;
    }
    win.set_busy(busy);
    win.set_busy_label(label.into());
    win.set_busy_kind(kind);
    if busy {
        ui_log!("RSH-UI busy: {label}");
    } else {
        ui_log!("RSH-UI busy: idle");
    }
}

/// Write the M-2 daemon-error prop, only when it changes.
fn set_daemon_error_prop(win: &MainWindow, text: &str) {
    if win.get_daemon_error().as_str() == text {
        return;
    }
    win.set_daemon_error(text.into());
    if text.is_empty() {
        ui_log!("RSH-UI daemon-error banner cleared");
    } else {
        ui_log!("RSH-UI daemon-error banner: {text}");
    }
}

/// Set a window prop only when the value moved (DS-78): an
/// unchanged `set_*` still crosses into Slint and marks the
/// property dirty, so a 20 Hz frame that re-sets ~50 props repaints
/// even when nothing moved. Reads (`get_*`) are cheap — they never
/// mark anything dirty.
///
/// `next` is passed by value on purpose: the setter closure
/// consumes it (Slint setters take values, not references), so the
/// `needless_pass_by_value` lint is a false positive here.
#[allow(clippy::needless_pass_by_value)]
fn set_if_changed<T: PartialEq>(current: T, next: T, set: impl FnOnce(T)) {
    if current != next {
        set(next);
    }
}

/// Background-job completions (A9/B5): a job thread sends exactly one
/// of these on `job_tx`; the frame timer drains them on the UI thread
/// and applies the result (notify / revert / re-sync). The payloads
/// are the API's own types, so no result is re-interpreted here.
enum JobDone {
    /// Reconnect probe finished: the live API, or the plain reason
    /// it never answered.
    Reconnect(Result<Arc<IpcApi>, String>),
    /// Transport switch finished: `index` is the Settings segment
    /// the user picked (0 Wi-Fi, 1 USB) when the switch came from
    /// Settings (`None` for the wizard's Connect step, which has no
    /// switch to revert) — a failure reverts the switch.
    Transport {
        index: Option<i32>,
        result: Result<ConnectionInfo, UserError>,
    },
    /// Backend switch finished: `index` is the Settings segment the
    /// user picked (0 `HIDMaestro`, 1 vJoy), so a failure reverts the
    /// switch to the other segment.
    Backend {
        index: i32,
        result: Result<String, UserError>,
    },
    /// Firmware check finished (board vs bundled version).
    FirmwareCheck(Result<FirmwareCheck, UserError>),
    /// App update check finished (DS-20): the GitHub Releases
    /// latest tag vs the running version.
    AppUpdateCheck(Result<AppUpdateVerdict, UserError>),
    /// Firmware update start finished (the flash then runs on the
    /// daemon; progress comes from `firmware_update_status` polls).
    FirmwareUpdate(Result<FirmwareUpdateStatus, UserError>),
    /// Raw-mV availability probe finished (any axis reported).
    RawAvailable(bool),
    /// Reconnect seed finished (F5): the live API plus its fetched
    /// seed — applied on the UI thread by `apply_reconnect_seed`.
    ReconnectSeed {
        api: Arc<IpcApi>,
        seed: crate::ipc_api::LiveSeed,
    },
    /// Generic job finished (F6): `id` routes the result to the
    /// `PendingApply` the starter registered; `result` is the job
    /// thread's answer, applied on the UI thread.
    Generic { id: u64, result: JobResult },
}

/// The app-update check's verdict (DS-20, U-02): what the GitHub
/// Releases latest tag says about the running version.
#[derive(Clone, Debug, PartialEq, Eq)]
enum AppUpdateVerdict {
    /// The running version is the latest release.
    UpToDate,
    /// A newer release exists upstream (its version, no `v`).
    Available(String),
    /// The running build is newer than the latest release (a dev
    /// build) — never offered as an update.
    CurrentIsNewer(String),
}

/// GitHub Releases API endpoint for the latest RSH release (DS-20).
const GITHUB_LATEST_RELEASE_URL: &str =
    "https://api.github.com/repos/dreamsofts34-creator/RSH/releases/latest";

/// `curl` binary: Windows 10+ ships `curl.exe` (the same inbox
/// binary the vJoy download uses); elsewhere plain `curl`.
#[cfg(windows)]
const CURL_BIN: &str = "curl.exe";
#[cfg(not(windows))]
const CURL_BIN: &str = "curl";

/// Parse the `tag_name` out of a GitHub Releases `latest` payload
/// (DS-20). Pure — unit-tested below. Returns the tag without the
/// leading `v` (`"2.0.4"`), or `None` when the payload has no
/// usable tag (error bodies, HTML, garbage).
fn parse_latest_release_tag(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let tag = value.get("tag_name")?.as_str()?;
    let tag = tag.strip_prefix('v').unwrap_or(tag);
    if tag.is_empty() {
        None
    } else {
        Some(tag.to_string())
    }
}

/// Parse an `a.b.c` version into numeric parts (DS-20). A leading
/// `v` and any `-`/`+` suffix are stripped; a missing part reads
/// as 0; a non-numeric part fails the parse.
fn parse_semver(version: &str) -> Option<(u64, u64, u64)> {
    let version = version.strip_prefix('v').unwrap_or(version);
    let version = version.split(['-', '+']).next().unwrap_or(version);
    let mut parts = version.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = match parts.next() {
        Some(part) => part.parse().ok()?,
        None => 0,
    };
    let patch = match parts.next() {
        Some(part) => part.parse().ok()?,
        None => 0,
    };
    Some((major, minor, patch))
}

/// Compare two `a.b.c` versions numerically (DS-20 — `2.0.10` is
/// newer than `2.0.9`, which a string compare gets wrong).
/// `None` when either side does not parse; the caller reports
/// "unreadable" instead of guessing an update.
fn compare_app_versions(a: &str, b: &str) -> Option<std::cmp::Ordering> {
    Some(parse_semver(a)?.cmp(&parse_semver(b)?))
}

/// Decide the update verdict from the current + latest versions
/// (DS-20). Pure — unit-tested below.
fn app_update_verdict(current: &str, latest: &str) -> Option<AppUpdateVerdict> {
    match compare_app_versions(latest, current)? {
        std::cmp::Ordering::Greater => Some(AppUpdateVerdict::Available(latest.to_string())),
        std::cmp::Ordering::Equal => Some(AppUpdateVerdict::UpToDate),
        std::cmp::Ordering::Less => Some(AppUpdateVerdict::CurrentIsNewer(latest.to_string())),
    }
}

/// Fetch the latest release tag from GitHub Releases and decide
/// the update verdict (DS-20): the Settings "Check" button really
/// checks now. Runs on a job thread, never the UI thread; bounded
/// by `--max-time 8` (inside the job's 10 s timeout) so a dead
/// network can never hang the check. Public repo, no auth — the
/// 60 req/h/IP unauthenticated limit is plenty for a manual
/// button. Any failure is a plain-language [`UserError`], never a
/// raw curl/JSON string.
fn fetch_app_update() -> Result<AppUpdateVerdict, UserError> {
    let err = |what_happened: &str| UserError {
        code: "UpdateCheck".to_string(),
        title: "Update check".to_string(),
        what_happened: what_happened.to_string(),
        what_to_do: "Check your connection and try again.".to_string(),
        detail: None,
    };
    // R2-03 (S-08): no console window flashes for the `curl` update
    // check. `CURL_BIN` is `curl.exe` on Windows and `curl`
    // elsewhere, so the flag is applied in a `#[cfg(windows)]`
    // block after building the command, not in the chain.
    let mut curl = std::process::Command::new(CURL_BIN);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        curl.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let output = curl
        .args([
            "-sS",
            "--fail",
            "--max-time",
            "8",
            "--connect-timeout",
            "5",
            "-H",
            "Accept: application/vnd.github+json",
            // GitHub requires a User-Agent; the default `curl/x.y`
            // one is refused by some egress filters, so send an
            // explicit one (verified against the live API).
            "-H",
            "User-Agent: RSH-update-check",
            // The `-w` trailer puts the HTTP status on the last
            // line, so the verdict can tell "rate-limited" apart
            // from "no network".
            "-w",
            "\n%{http_code}",
            GITHUB_LATEST_RELEASE_URL,
        ])
        .output()
        .map_err(|_| err("Couldn't reach GitHub to check for updates."))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let (body, status) = match text.rsplit_once('\n') {
        Some((body, status)) => (body, status.trim()),
        None => ("", ""),
    };
    if !output.status.success() {
        if status == "403" {
            // Unauthenticated API limit is 60 req/h/IP — a manual
            // button cannot hit it alone, but a shared IP can.
            return Err(UserError {
                code: "UpdateCheck".to_string(),
                title: "Update check".to_string(),
                what_happened: "GitHub is rate-limiting update checks right now.".to_string(),
                what_to_do: "Try again in a little while — no download was started.".to_string(),
                detail: None,
            });
        }
        return Err(err("GitHub didn't answer the update check."));
    }
    let Some(latest) = parse_latest_release_tag(body) else {
        return Err(err("GitHub answered, but the release info was unreadable."));
    };
    app_update_verdict(app_version(), &latest)
        .ok_or_else(|| err("The latest release version was unreadable."))
}

/// A destructive action waiting on the small confirm dialog
/// (D17): delete profile / remove game never run without the
/// user confirming. Set by [`Ctx::open_confirm`], taken by the
/// confirm Yes handler.
#[derive(Clone, Debug)]
enum ConfirmAction {
    /// Delete the profile at this popover index.
    DeleteProfile(i32),
    /// Remove the game with this id.
    RemoveGame(String),
}

/// One background-job answer (F6): the job thread's work returns
/// one of these; the UI-thread `apply` closure turns it into
/// notifications / re-syncs / property writes. The payloads are
/// the API's own types, so no result is re-interpreted here.
enum JobResult {
    /// A `set_tuning` push: the stored record, or the error.
    /// Boxed: `TuningSettings` grew past clippy's 200-byte
    /// `large_enum_variant` threshold, so only the `Ok` payload
    /// is boxed — the `Err` payload stays inline.
    Tuning(Result<Box<TuningSettings>, UserError>),
    /// A yes/no op (module enable, output toggle, auto-pause,
    /// autostart, keep-in-tray, save / discard draft, …): the new
    /// state, or the error.
    Bool(Result<bool, UserError>),
    /// A text op (board address, backend, profile id, …): the new
    /// value, or the error.
    Text(Result<String, UserError>),
    /// A no-value op (backup, restore, …): success or the error.
    Unit(Result<(), UserError>),
    /// A game op: `true` = the game is now running, or the error.
    Game(Result<bool, UserError>),
    /// A capture op: the captured mV, or the error.
    Capture(Result<u16, UserError>),
    /// A learn poll: the learned button, or `None` when learning
    /// ended with no pick.
    LearnPoll(Option<u8>),
    /// A shifter sensitivity read / write (SH1): `Ok(Some(v))` =
    /// the board's sensitivity, `Ok(None)` = no shifter configured
    /// on the board, or the error.
    Sensitivity(Result<Option<u8>, UserError>),
}

/// A pending generic-job apply (F6): registered by
/// `run_generic_job` when a job starts, run once on the UI thread
/// when its `JobDone::Generic` arrives (or dropped when the job
/// times out — the late result is then ignored).
type PendingApply = Box<dyn FnOnce(&Ctx, &MainWindow, JobResult)>;

/// Shared frame context for callbacks: the swappable API state,
/// tuning state, the unsaved flag, the notification center, and the
/// resolved data dir (setup marker + theme file).
/// Cached per-frame command strings (DS-78): `curve_commands`
/// rebuilds ~25 segments and `crosshair_commands` formats on every
/// frame, but their inputs only move when the tuning or the marker
/// moves. The cache keys on the inputs and reuses the last string;
/// `dashed_line_commands` is constant, so it is built once.
#[derive(Default)]
struct FrameCmdCache {
    dashed_line: Option<SharedString>,
    curve: Option<(f32, f32, SharedString)>,
    crosshair: Option<(f32, f32, SharedString)>,
    /// R2-43: the per-tick formatted strings, cached by their
    /// cheap inputs — `push_frame` used to `format!` ~4 strings
    /// per 50 ms tick even when nothing moved.
    dash: Option<SharedString>,
    steer_text: Option<(f32, SharedString)>,
    deadzone_text: Option<(f32, SharedString)>,
    curve_text: Option<(f32, SharedString)>,
    lock_text: Option<(f32, SharedString)>,
    smoothing_text: Option<(f32, SharedString)>,
    // --- Steering per-module tuning ---
    balance_text: Option<(f32, SharedString)>,
    trim_text: Option<(f32, SharedString)>,
    conn_label: Option<(bool, bool, bool, String, SharedString)>,
    conn_sub: Option<(bool, bool, String, String, SharedString)>,
    gear_text: Option<(u8, SharedString)>,
    pedals_readout: Option<(f64, f64, f64, SharedString)>,
}

impl FrameCmdCache {
    fn dashed_line(&mut self) -> SharedString {
        if let Some(cached) = &self.dashed_line {
            return cached.clone();
        }
        let built: SharedString = dashed_line_commands().into();
        self.dashed_line = Some(built.clone());
        built
    }

    /// The "—" placeholder, built once.
    fn dash(&mut self) -> SharedString {
        if let Some(cached) = &self.dash {
            return cached.clone();
        }
        let built: SharedString = "—".into();
        self.dash = Some(built.clone());
        built
    }

    /// Steering angle text (`"+12°"`, "—" without live data —
    /// B1: never a sine angle), rebuilt only when the angle moved.
    #[allow(clippy::float_cmp)]
    fn steer_text(&mut self, live: bool, steer_deg: f32) -> SharedString {
        if !live {
            return self.dash();
        }
        if let Some((deg, cached)) = &self.steer_text {
            if *deg == steer_deg {
                return cached.clone();
            }
        }
        let built: SharedString = format!("{steer_deg:+.0}°").into();
        self.steer_text = Some((steer_deg, built.clone()));
        built
    }

    /// Deadzone percent text, rebuilt only when it moved.
    #[allow(clippy::float_cmp)]
    fn deadzone_text(&mut self, deadzone: f32) -> SharedString {
        if let Some((dz, cached)) = &self.deadzone_text {
            if *dz == deadzone {
                return cached.clone();
            }
        }
        let built: SharedString = format!("{deadzone:.0}%").into();
        self.deadzone_text = Some((deadzone, built.clone()));
        built
    }

    /// Curve exponent text, rebuilt only when it moved.
    #[allow(clippy::float_cmp)]
    fn curve_text(&mut self, exponent: f32) -> SharedString {
        if let Some((ex, cached)) = &self.curve_text {
            if *ex == exponent {
                return cached.clone();
            }
        }
        let built: SharedString = format!("{exponent:.2}").into();
        self.curve_text = Some((exponent, built.clone()));
        built
    }

    /// Steering lock text (`"540°"`), rebuilt only when it moved.
    #[allow(clippy::float_cmp)]
    fn lock_text(&mut self, lock_deg: f32) -> SharedString {
        if let Some((deg, cached)) = &self.lock_text {
            if *deg == lock_deg {
                return cached.clone();
            }
        }
        let built: SharedString = format!("{lock_deg:.0}°").into();
        self.lock_text = Some((lock_deg, built.clone()));
        built
    }

    /// Smoothing percent text, rebuilt only when it moved.
    #[allow(clippy::float_cmp)]
    fn smoothing_text(&mut self, smoothing_pct: f32) -> SharedString {
        if let Some((pct, cached)) = &self.smoothing_text {
            if *pct == smoothing_pct {
                return cached.clone();
            }
        }
        let built: SharedString = format!("{smoothing_pct:.0}%").into();
        self.smoothing_text = Some((smoothing_pct, built.clone()));
        built
    }

    // --- Steering per-module tuning ---
    /// Signed percent text (`"+10%"`, `"-5%"`, `"0%"`), rebuilt
    /// only when it moved. Shared by the balance and trim sliders.
    #[allow(clippy::float_cmp)]
    fn signed_pct_text(cache: &mut Option<(f32, SharedString)>, pct: f32) -> SharedString {
        if let Some((cached_pct, cached)) = cache.as_ref() {
            if *cached_pct == pct {
                return cached.clone();
            }
        }
        let built: SharedString = if pct == 0.0 {
            "0%".into()
        } else {
            format!("{pct:+.0}%").into()
        };
        *cache = Some((pct, built.clone()));
        built
    }

    /// Left/right balance text (`"+10%"`, `"-5%"`, `"0%"`).
    fn balance_text(&mut self, balance_pct: f32) -> SharedString {
        Self::signed_pct_text(&mut self.balance_text, balance_pct)
    }

    /// Centre trim text (`"+2%"`, `"-5%"`, `"0%"`).
    fn trim_text(&mut self, trim_pct: f32) -> SharedString {
        Self::signed_pct_text(&mut self.trim_text, trim_pct)
    }

    /// Top-bar connection label, rebuilt only when one of its
    /// inputs moved. The `conn_label_for` rule stays the single
    /// source of truth (see the DS-67 test).
    fn conn_label(
        &mut self,
        connecting: bool,
        live: bool,
        label: &str,
        rejoining: bool,
    ) -> SharedString {
        if let Some((c, l, r, old_label, cached)) = &self.conn_label {
            if *c == connecting && *l == live && *r == rejoining && old_label == label {
                return cached.clone();
            }
        }
        let built: SharedString = conn_label_for(connecting, live, label, rejoining).into();
        self.conn_label = Some((
            connecting,
            live,
            rejoining,
            label.to_string(),
            built.clone(),
        ));
        built
    }

    /// Top-bar connection sub-line (`"Wi-Fi · 192.168.4.1"`),
    /// rebuilt only when one of its inputs moved.
    fn conn_sub(
        &mut self,
        connecting: bool,
        live: bool,
        transport: &str,
        address: &str,
    ) -> SharedString {
        if let Some((c, l, t, a, cached)) = &self.conn_sub {
            if *c == connecting && *l == live && t == transport && a == address {
                return cached.clone();
            }
        }
        let built: SharedString = if connecting {
            "Starting RSH…".into()
        } else if live {
            if address.is_empty() {
                transport.to_string().into()
            } else {
                format!("{transport} · {address}").into()
            }
        } else {
            "Not connected".into()
        };
        self.conn_sub = Some((
            connecting,
            live,
            transport.to_string(),
            address.to_string(),
            built.clone(),
        ));
        built
    }

    /// Gear text (`"—"` in neutral), rebuilt only when the gear moved.
    fn gear_text(&mut self, gear: u8) -> SharedString {
        if let Some((g, cached)) = &self.gear_text {
            if *g == gear {
                return cached.clone();
            }
        }
        let built: SharedString = if gear == 0 {
            self.dash()
        } else {
            gear.to_string().into()
        };
        self.gear_text = Some((gear, built.clone()));
        built
    }

    /// Diagnostics pedals readout, rebuilt only when a value moved.
    #[allow(clippy::float_cmp)]
    fn pedals_readout(&mut self, throttle: f64, brake: f64, clutch: f64) -> SharedString {
        if let Some((t, b, c, cached)) = &self.pedals_readout {
            if *t == throttle && *b == brake && *c == clutch {
                return cached.clone();
            }
        }
        let built: SharedString = format!(
            "Throttle {:.0}% · Brake {:.0}% · Clutch {:.0}%",
            throttle * 100.0,
            brake * 100.0,
            clutch * 100.0
        )
        .into();
        self.pedals_readout = Some((throttle, brake, clutch, built.clone()));
        built
    }

    // Cache keys compare with exact equality on purpose: the key
    // IS the input, bit for bit — an epsilon comparison could
    // return a stale string for a genuinely different input.
    #[allow(clippy::float_cmp)]
    fn curve(&mut self, deadzone: f32, exponent: f32) -> SharedString {
        if let Some((dz, ex, cached)) = &self.curve {
            if *dz == deadzone && *ex == exponent {
                return cached.clone();
            }
        }
        let built: SharedString = curve_commands(deadzone, exponent).into();
        self.curve = Some((deadzone, exponent, built.clone()));
        built
    }

    #[allow(clippy::float_cmp)]
    fn crosshair(&mut self, mx: f32, my: f32) -> SharedString {
        if let Some((x, y, cached)) = &self.crosshair {
            if *x == mx && *y == my {
                return cached.clone();
            }
        }
        let built: SharedString = crosshair_commands(mx, my).into();
        self.crosshair = Some((mx, my, built.clone()));
        built
    }
}

struct Ctx {
    state: Rc<RefCell<LiveState>>,
    tuning: Rc<RefCell<Tuning>>,
    /// Cached stored steering record (L4): supplies min/max mV,
    /// smoothing and extension fields for each slider push.
    steering: Rc<RefCell<TuningSettings>>,
    /// Cached pedal records + feel exponents (A1/A2).
    pedals: Rc<RefCell<PedalCache>>,
    /// Cached handbrake / LED records (R2-10): the local copy a drag
    /// edits and a flush completion re-seeds from the `set_tuning`
    /// return value, so a ~1 Hz re-sync never snaps a slider back
    /// to a stale cached value mid-drag.
    handbrake_record: Rc<RefCell<TuningSettings>>,
    led_record: Rc<RefCell<TuningSettings>>,
    /// Cached horn record (R2-10, HO1/HO2): same local-copy pattern
    /// as the handbrake / LED records, for the minimum-honk drag.
    horn_record: Rc<RefCell<TuningSettings>>,
    /// Last good calibration read (A2/A12/A22): pedal mV, capture
    /// date, centre-set state. `None` = never read / never captured.
    calibration: Rc<RefCell<Option<CalibrationInfo>>>,
    /// True while a vJoy install started by the wizard is polling.
    vjoy_polling: Rc<Cell<bool>>,
    /// 12 wheel-button display actions (A3/A4) + the pending chip
    /// choice not yet saved (None = the tile's stored action).
    button_actions: Rc<RefCell<Vec<String>>>,
    pending_action: Rc<RefCell<Option<String>>>,
    /// 12 wheel-button long-press display actions (BU2) + the
    /// pending chip choice not yet saved (None = the tile's stored
    /// long-press action, "Not set" = none).
    long_press_actions: Rc<RefCell<Vec<String>>>,
    pending_long_action: Rc<RefCell<Option<String>>>,
    selected_button: Rc<Cell<i32>>,
    /// Edit-panel mirror of the Buttons page state (Rust tracks it
    /// from the tile/knob callbacks): 0 button, 1 knob.
    edit_kind: Rc<Cell<i32>>,
    edit_knob: Rc<Cell<i32>>,
    knob_row: Rc<Cell<i32>>,
    /// Knob actions [knob][row: left, right, press], local only
    /// until the mapping backend lands.
    knob_actions: Rc<RefCell<[[String; 3]; 2]>>,
    /// Last profiles/games lists (A8/A14): index → id lookups.
    profiles_cache: Rc<RefCell<Vec<ProfileSummary>>>,
    games_cache: Rc<RefCell<Vec<GameSummary>>>,
    /// Add/edit game dialog state (manual games): the id being
    /// edited (`None` = adding a new game), and the bound profile
    /// id chosen in the dialog (`None` = no profile bound).
    game_dialog_edit_id: Rc<RefCell<Option<String>>>,
    game_dialog_profile_id: Rc<RefCell<Option<String>>>,
    /// R2-11: the games list the profile presets were last built
    /// from — the presets model is only rebuilt when it changed.
    presets_games: Rc<RefCell<Vec<GameSummary>>>,
    /// R2-11: the profiles list the game tiles were last built
    /// from — a tile's sub-line names its bound profile, so the
    /// tiles rebuild when the profiles change even if the games
    /// themselves did not.
    games_profiles: Rc<RefCell<Vec<ProfileSummary>>>,
    /// Profile id waiting on the unsaved-changes prompt (A8).
    pending_profile_id: Rc<RefCell<Option<String>>>,
    /// Learn target: 0 none, 1 Buttons page, 2 horn (Extras).
    learn_target: Rc<Cell<i32>>,
    /// Horn test flash end (Extras Test lights the indicator).
    horn_test_until: Rc<Cell<Option<Instant>>>,
    /// Handbrake test flash end (R2-17): the Button-mode tile's
    /// "Pressed" flash, mirroring the horn test — the
    /// `handbrake-test` action sets this, `push_frame` drives
    /// `handbrake-pressed` from it.
    handbrake_test_until: Rc<Cell<Option<Instant>>>,
    /// Status-bar expiry (B01, was the DS-24 toast):
    /// `notify_error` shows the message until this instant; the
    /// 1 Hz tick clears it after. `None` = no status showing.
    status_until: Rc<Cell<Option<Instant>>>,
    /// Live chart histories (A20): steering deg + throttle 0..1.
    steer_hist: Rc<RefCell<VecDeque<f32>>>,
    pedal_hist: Rc<RefCell<VecDeque<f32>>>,
    /// Brake / clutch histories (R2-10, pedals live graphs): the
    /// Diagnostics "Pedals" chart used to record throttle only —
    /// brake and clutch never entered any history, so their traces
    /// did not exist at all. All three now push every live frame,
    /// on the same `push_frame` path as `steer_hist`.
    brake_hist: Rc<RefCell<VecDeque<f32>>>,
    clutch_hist: Rc<RefCell<VecDeque<f32>>>,
    /// Cached per-frame command strings (DS-78): the curve /
    /// crosshair / dashed-line strings are rebuilt only when their
    /// inputs move.
    frame_cmds: Rc<RefCell<FrameCmdCache>>,
    /// Frame times (DS-82): the last 600 `refresh` durations (30 s
    /// at 20 Hz) for the Diagnostics p50/p99. This is the Rust
    /// frame work (`push_frame` + capture sync) — the Slint paint
    /// itself is timed separately (see `frame_paint_times`, R2-46).
    frame_times: Rc<RefCell<VecDeque<Duration>>>,
    /// Paint-completion times (R2-46): the last 600 durations from
    /// a frame's `refresh` start to the Slint `AfterRendering`
    /// notifier firing — i.e. Rust work + layout + paint, the
    /// closest number to what the user sees. Filled only when a
    /// frame actually paints (a tick with no dirty props produces
    /// no `AfterRendering` and leaves no sample); empty until the
    /// first paint, e.g. on a backend without notifier support.
    frame_paint_times: Rc<RefCell<VecDeque<Duration>>>,
    /// Frame start awaiting its paint (R2-46): set by the frame
    /// timer before `refresh`, taken by the `AfterRendering`
    /// notifier. `None` = no frame in flight / no paint pending.
    frame_paint_pending: Rc<Cell<Option<Instant>>>,
    unsaved: Rc<Cell<bool>>,
    notifications: Rc<RefCell<NotificationCenter>>,
    data_dir: Option<PathBuf>,
    /// Shared live link for the health monitor (B2): the monitor
    /// thread pings whatever is here and re-probes when it is
    /// empty. Kept in sync with `state.live_api` by
    /// [`Ctx::swap_to_live`] / [`Ctx::mark_dead`].
    live_link: Arc<Mutex<Option<Arc<IpcApi>>>>,
    /// True while the window is hidden to tray (H-1/A11): the health
    /// monitor must not spawn a daemon while hidden (a tray Quit
    /// must not resurrect it). Shared with the monitor thread.
    hidden: Arc<AtomicBool>,
    /// Set by the close handler when "Keep running in the tray" is
    /// off: the frame timer quits the event loop on the next tick
    /// (quitting from inside the close callback would be re-entrant).
    quit_requested: Rc<Cell<bool>>,
    /// Page requests forwarded by a second UI instance (M-5/A1):
    /// the Windows pipe listener pushes here; the frame timer drains
    /// them. Linux requests arrive via the request file instead.
    page_requests: Arc<Mutex<VecDeque<String>>>,
    /// Cached OS light preference (DS-49/DS-50): sampled by a
    /// background thread, never by spawning `reg` on the UI thread.
    /// `true` = the OS currently prefers a light theme.
    os_light: Arc<AtomicBool>,
    /// Stored theme choice (0 Dark, 1 Light, 2 Auto), cached so the
    /// 1 Hz Auto re-sample never reads the file (DS-50).
    theme_choice: Rc<Cell<i32>>,
    /// True while the board is rejoining Wi-Fi with the new password
    /// after a password set (DS-67, no-pairing redesign): the link
    /// drops while the AP restarts to WPA2, so the UI says
    /// "Rejoining with the new password…" instead of a generic
    /// disconnect. Cleared when the session goes Active again, or
    /// after 30 s.
    wifi_rejoining: Rc<Cell<bool>>,
    wifi_rejoin_since: Rc<Cell<Option<Instant>>>,
    /// Last `password_set` value `sync_wifi_password_state` saw
    /// (R3-UI-02): the success message is cleared when the state
    /// moves on — otherwise it persists across Settings visits
    /// after the state it describes has changed. `None` = never
    /// synced yet.
    wifi_password_set_known: Rc<Cell<Option<bool>>>,
    /// The `password_set` value the currently displayed
    /// `wifi-password-message` describes (R3-UI-02): the message
    /// is kept across a re-sync that lands on the state it already
    /// describes (e.g. the daemon's status flips to `true` on the
    /// 1 Hz tick just after a successful set), and cleared only
    /// when the state moves somewhere the message does not
    /// describe. `None` = no message shown.
    wifi_password_message_for: Rc<Cell<Option<bool>>>,
    /// D17: the destructive action the confirm dialog is waiting
    /// on (delete profile / remove game) — `None` = no confirm
    /// open. Set by `open_confirm`, taken by the Yes handler.
    confirm_pending: Rc<RefCell<Option<ConfirmAction>>>,
    /// D16: when the current vJoy install started (the Drivers
    /// step shows indeterminate progress + elapsed time while it
    /// runs). `None` = no install in flight.
    vjoy_install_started: Rc<Cell<Option<Instant>>>,
    /// Phase 2 (Wi-Fi gap): the first-`Unreachable` notification
    /// was already shown — set on first Unreachable, cleared when
    /// the board becomes reachable / the error clears, so the user
    /// is told once per outage, not once per second.
    unreachable_notified: Rc<Cell<bool>>,
    /// Background-job completions (A9/B5): job threads send on
    /// `job_tx`; the frame timer drains `job_rx` on the UI thread.
    /// `Rc<RefCell<Receiver>>` — the UI is single-threaded, so no
    /// `Mutex` is needed for the drain side.
    job_tx: std::sync::mpsc::Sender<JobDone>,
    job_rx: Rc<RefCell<std::sync::mpsc::Receiver<JobDone>>>,
    /// Jobs currently running, by kind: (busy label, started at).
    /// The frame timer times a stale entry out with a visible error
    /// — loading is never infinite (B5).
    busy_jobs: Rc<RefCell<HashMap<JobKind, (String, Instant)>>>,
    /// Next generic-job id (F6): handed out by `run_generic_job`,
    /// routed back by `JobDone::Generic`.
    next_job_id: Rc<Cell<u64>>,
    /// Pending generic-job applies (F6): id → (kind, apply). The
    /// apply runs once on the UI thread when the job's result
    /// arrives; a timed-out job's entry is dropped, so its late
    /// result is ignored.
    pending_applies: Rc<RefCell<HashMap<u64, (JobKind, PendingApply)>>>,
    /// Last slow-cache generation the pages re-synced from (F5):
    /// the 1 Hz tick re-syncs the pages when the poller bumps it,
    /// so a daemon-side change reaches the UI within ~1 s.
    last_cache_generation: Rc<Cell<u64>>,
    /// When the current `connecting` state started (A9): the frame
    /// timer clears a `connecting` older than 15 s with a message
    /// instead of leaving "Connecting…" forever.
    connecting_since: Rc<Cell<Option<Instant>>>,
    /// When the wizard last changed step (R2-36): Next/Back clicks
    /// within [`SETUP_STEP_DEBOUNCE`] of it are ignored, so a rapid
    /// double-click cannot skip a step. `None` = no step change yet.
    last_setup_step_at: Rc<Cell<Option<Instant>>>,
    /// True while a Reconnect job is in flight (A9): a second click
    /// is ignored instead of queueing another ~3 s probe.
    reconnect_in_flight: Rc<Cell<bool>>,
    /// F3 (v2.0.3): the auto-respawn budget, shared with the health
    /// monitor thread (which spends it) — the monitor never spawns
    /// while this is latched or a manual retry is pending.
    spawn_budget: Arc<live_probe::SpawnBudget>,
    /// F3 (v2.0.3): latched respawn-failure reason (budget exhausted
    /// or fresh fatal file) shown in the daemon-error banner until a
    /// live daemon clears it. `None` = no latch.
    spawn_latch: Rc<RefCell<Option<String>>>,
    /// F3 (v2.0.3): the detail `mark_dead` last showed, so
    /// `mark_spawn_latched` does not notify the same fatal reason
    /// twice. `None` = the generic "not running" was shown.
    last_dead_detail: Rc<RefCell<Option<String>>>,
    /// F12 (v2.0.3): when the current down-episode started (set by
    /// `mark_dead`, cleared by `swap_to_live`). `None` = never down
    /// since launch, or back live.
    down_since: Rc<Cell<Option<Instant>>>,
    /// F12 (v2.0.3): when the last reconnect happened (set by
    /// `swap_to_live`); a death soon after it belongs to the same
    /// flapping burst as the previous one.
    last_back_at: Rc<Cell<Option<Instant>>>,
    /// F12 (v2.0.3): when the last flap reconnect happened (set by
    /// `swap_to_live`); bounds how long a burst count stays
    /// meaningful before it goes stale.
    last_flap_at: Rc<Cell<Option<Instant>>>,
    /// F12 (v2.0.3): how many short down-episodes (flaps) ran in
    /// the current burst; folded into the next non-burst down
    /// notification as "flapped ×N".
    flap_count: Rc<Cell<u32>>,
    /// F12 (v2.0.3): the last reconnect ended a flap (down-episode
    /// < [`FLAP_DOWN_WINDOW`]) — the signal for `reconnect_to_live`
    /// to suppress its "Wheel connected" notify (the notify itself
    /// lives in `reconnect_to_live`, owned by the F5 worker — see
    /// the note on [`Ctx::swap_to_live`]).
    suppress_connected_notify: Rc<Cell<bool>>,
    /// Last firmware check (M-8): refreshed by the `FirmwareCheck`
    /// job and on reconnect; `sync_overview` reads the cache, never
    /// the bundled file on the hot path.
    firmware_check_cache: Rc<RefCell<Option<FirmwareCheck>>>,
    /// True when the wheel recently reported raw mV (H9): refreshed
    /// by a background job when the wizard enters Calibrate; the
    /// wizard's Next gates on this, not just on the daemon being live.
    raw_available: Rc<Cell<bool>>,
    /// How many preset rows the popover shows (M1): the popover's
    /// footer buttons pass a COMBINED index (presets first, then my
    /// profiles) — this count converts it to a my-profiles index.
    presets_count: Rc<Cell<usize>>,
    /// Last known module on/off (H7): the Extras toggles flip their
    /// .slint state first and call Rust with no payload, so Rust
    /// tracks the state and flips it per toggle. Seeded from the API
    /// on (re)connect. Order: handbrake, shifter, horn, led.
    module_states: Rc<RefCell<[bool; 4]>>,
    /// True while a firmware flash started from Settings is running
    /// (H-4): the 1 Hz tick polls `firmware_update_status()` and
    /// drives `firmware-text` with the progress.
    firmware_update_running: Rc<Cell<bool>>,
    /// Last "address applies" notification (H4): the address field
    /// fires `edited` per keystroke, so the notify is throttled to
    /// one per 3 s instead of spamming the drawer.
    last_address_notify: Rc<Cell<Option<Instant>>>,
    /// Steering push due at (M-7): slider drags set this instead of
    /// pushing per move — the frame timer flushes it (150 ms
    /// debounce), so a drag is one IPC call, not one per pixel.
    steer_push_due: Rc<Cell<Option<Instant>>>,
    /// Pedal / Extras slider drags waiting out their debounce (M-7):
    /// the latest (module, field, value) + when it may be pushed.
    /// The frame timer flushes it (150 ms after the last move).
    pending_field: Rc<RefCell<Option<(PendingField, Instant)>>>,
    /// Shifter sensitivity drag waiting out its debounce (SH1, M-7):
    /// the latest percent (0..100) + when it may be pushed. Kept
    /// apart from `pending_field`: a sensitivity push is a board
    /// config transaction, not a `set_tuning` field push.
    pending_sensitivity: Rc<RefCell<Option<(u8, Instant)>>>,
    /// Last-known shifter sensitivity from the board (SH1): `None`
    /// = unknown (never read), `Some(None)` = the board has no
    /// shifter configured, `Some(Some(v))` = the value. Read once
    /// (on first Extras visit) and after every successful push —
    /// never on the ~1 Hz sync, which would open a board control
    /// session every second. The nested `Option` is deliberate:
    /// three states, no sentinel value.
    #[allow(clippy::option_option)]
    sensitivity_cached: Rc<Cell<Option<Option<u8>>>>,
    /// When the last sensitivity read failed (SH1): the fetch
    /// retries at most every 30 s, not on every ~1 Hz sync.
    sensitivity_failed_at: Rc<Cell<Option<Instant>>>,
    /// Last confirmed shifter sensitivity (SH1): the value a failed
    /// or timed-out push snaps the slider back to. `None` = no
    /// shifter configured (nothing to snap back to).
    sensitivity_push_base: Rc<Cell<Option<u8>>>,
    /// What the running [`JobKind::TuningPush`] job is pushing
    /// (R2-18): the snapshot taken when the job started, so a
    /// timeout reverts exactly that slider. `None` = no tuning
    /// push in flight.
    tuning_push_in_flight: Rc<RefCell<Option<TuningPushTarget>>>,
    /// Content hashes of the last list models written (R2-11): a
    /// model is only rebuilt when its content hash moved, so the
    /// 1 Hz re-sync and the 5 s tick stop rebuilding every `for`
    /// row while nothing changed. `None` = never built yet.
    profiles_model_hash: Rc<Cell<Option<u64>>>,
    games_model_hash: Rc<Cell<Option<u64>>>,
    presets_model_hash: Rc<Cell<Option<u64>>>,
    /// Last LED preview key (pattern, brightness, colour) (R2-11):
    /// the preview `ModelRc` is only rebuilt when the key moved.
    /// Colour `None` is stored as `u32::MAX`. `(-1, 255, MAX)` =
    /// never built yet (no real pattern/brightness matches it).
    led_preview_key: Rc<Cell<(i32, u8, u32)>>,
    /// Content hashes of the last diagnostics models (R2-11): the
    /// button grid / event log are only rebuilt when their content
    /// moved. `None` = never built yet.
    diag_buttons_hash: Rc<Cell<Option<u64>>>,
    diag_events_hash: Rc<Cell<Option<u64>>>,
    /// Last frame's inputs (perf F-4): `push_frame` returns early
    /// when the current inputs equal these — an idle / disconnected
    /// frame then costs one cache read + compare instead of ~52
    /// Slint crossings + 4 history pushes + chart formatting.
    /// `None` = no frame has run yet (the first frame always runs).
    last_frame: Rc<RefCell<Option<FrameInputs>>>,
    /// Startup syncs deferred to the first frame tick (perf F-2):
    /// `main` sets this; the frame timer runs `refresh_profiles` /
    /// `refresh_games` / `sync_diag` once and clears it, so the
    /// window paints before those list/diag syncs run.
    startup_sync_pending: Rc<Cell<bool>>,
    /// Notification persistence coalescing (perf F-7): `true` while
    /// a save is due — the 1 Hz tick flushes at most one disk
    /// write per second, and `main` flushes once more at exit.
    notif_save_pending: Rc<Cell<bool>>,
    /// When the notification file was last written (perf F-7): the
    /// ≤1 write/s bound for the coalesced save.
    notif_last_save: Rc<Cell<Option<Instant>>>,
}

/// Every value a frame is derived from (perf F-4): `push_frame`
/// compares these with the previous frame's and returns early when
/// they are identical. Every input `push_frame` — or the capture
/// sync it gates — reads is in here, so a skipped frame can never
/// hide a visible change: the polled connection + snapshot, the
/// local tuning / pedal / handbrake state, the unsaved dot, the
/// test-flash instants, the raw-mV gate (value + its background
/// job, whose start/stop flips the Calibrate reason), the Wi-Fi
/// rejoin flag, whether Diagnostics is open (it gates the chart
/// strings), the output flag, and the live / connecting flags.
///
/// The bools are one flat comparison struct on purpose (a single
/// `==` decides the frame rest) — not a state machine.
#[derive(Clone, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
struct FrameInputs {
    conn: ConnectionInfo,
    snap: SnapshotInfo,
    tuning: (f32, f32, i32, f32, bool, f32),
    pedals: PedalCache,
    handbrake: TuningSettings,
    unsaved: bool,
    horn_flash: bool,
    handbrake_flash: bool,
    raw_available: bool,
    raw_job_running: bool,
    diag_open: bool,
    output_enabled: bool,
    live: bool,
    connecting: bool,
}

/// Whether a 400 ms test flash (Horn Test, handbrake Button-mode
/// test — R2-17) is still lit: `Some(until)` in the future means
/// pressed, anything else means released. Pure — unit-tested.
fn test_flash_active(until: Option<Instant>) -> bool {
    until.is_some_and(|until| Instant::now() < until)
}

/// p50/p99 of a duration ring, in ms (DS-82 frame histogram,
/// R2-46 paint histogram). `None` until a sample exists.
fn percentile_p50_p99(times: &VecDeque<Duration>) -> Option<(f64, f64)> {
    if times.is_empty() {
        return None;
    }
    let mut sorted: Vec<Duration> = times.iter().copied().collect();
    sorted.sort();
    let p50 = sorted[(sorted.len() - 1) / 2];
    let p99 = sorted[(sorted.len() - 1) * 99 / 100];
    Some((p50.as_secs_f64() * 1000.0, p99.as_secs_f64() * 1000.0))
}

impl Ctx {
    /// Current API handle (live daemon or sim, whichever is active).
    fn api(&self) -> Arc<dyn AppApi> {
        self.state.borrow().api.clone()
    }

    /// True while the live daemon is the active data source.
    fn is_live(&self) -> bool {
        self.state.borrow().live
    }

    /// SEAM (v2.0.2): capture exists now — `capture_centre` /
    /// `capture_pedal_min` / `capture_pedal_max` on the daemon, via
    /// the capture link below (raw mV is plumbed through, see
    /// `AppStateCore::note_raw_axes`). `set_button_action`
    /// deliberately does NOT exist — button→action rebinding is not
    /// a board feature.
    /// Capture is available while the live daemon is the data
    /// source. The sim never captures — its readings are not the
    /// wheel's. A live daemon with no wheel frames yet still enables
    /// the buttons; the capture call itself then fails visibly
    /// ("No live wheel data") instead of the buttons lying disabled.
    fn capture_available(&self) -> bool {
        self.is_live()
    }

    /// Reason shown under a disabled capture control.
    fn capture_unavailable_reason() -> &'static str {
        "RSH isn't running — reconnect to capture."
    }

    /// Push the capture availability into the window (centre /
    /// pedal Set / wizard Calibrate + their reasons). Called every
    /// frame from [`Self::refresh`], so a daemon that dies
    /// mid-session disables the buttons again instead of leaving
    /// them enabled against a dead link.
    /// Re-sync the capture-availability props every frame (DS-78:
    /// change-detected — the availability flags are constant for
    /// long stretches, and the reason strings are only rebuilt when
    /// the frame needs them).
    fn sync_capture_availability(&self, win: &MainWindow) {
        let available = self.capture_available();
        set_if_changed(win.get_centre_available(), available, |v| {
            win.set_centre_available(v);
        });
        set_if_changed(win.get_pedal_set_disabled(), !available, |v| {
            win.set_pedal_set_disabled(v);
        });
        set_if_changed(win.get_extras_calibration_disabled(), !available, |v| {
            win.set_extras_calibration_disabled(v);
        });
        // H9: the wizard's Calibrate step gates on REAL raw mV, not
        // just on the daemon being live — `raw_available` is probed
        // by a background job when the step is entered (and on the
        // 5 s refresh while it is open).
        set_if_changed(
            win.get_setup_calibration_available(),
            self.raw_available.get(),
            |v| win.set_setup_calibration_available(v),
        );
        let reason: &str = if available {
            ""
        } else {
            Self::capture_unavailable_reason()
        };
        // R2-43: compare as `&str` first — the old code built a
        // fresh `String` every frame just to compare it away.
        if win.get_centre_reason().as_str() != reason {
            win.set_centre_reason(reason.into());
        }
        let setup_reason: &str = if self.raw_available.get() {
            ""
        } else if !available {
            Self::capture_unavailable_reason()
        } else if self.busy_jobs.borrow().contains_key(&JobKind::RawAvailable) {
            "Waiting for wheel data…"
        } else {
            "No wheel data yet — check the wheel is connected."
        };
        if win.get_setup_calibration_reason().as_str() != setup_reason {
            win.set_setup_calibration_reason(setup_reason.into());
        }
    }

    /// Refresh the cached calibration read (`calibration()`):
    /// `Ok` stores it, `Err` (sim / never captured) clears it.
    /// Called at startup, on reconnect and on the 5 s refresh.
    /// Callers that own a window also call [`Self::sync_advanced`]
    /// next (R2-16), which reads the same cache points.
    fn refresh_calibration(&self) {
        *self.calibration.borrow_mut() = self.api().calibration().ok();
    }

    /// Push the Advanced-mode readouts (R2-16, REDESIGN-PLAN
    /// §7.2): the live raw mV per pedal axis from the slow-cache
    /// `raw_mv()` (~1 Hz). "—" until measured — never a fake 0.
    /// Steering has no raw readout any more (Bug 7: the Advanced
    /// card is tuning, not readouts). Called from the 1 Hz re-sync,
    /// the 5 s tick, and startup.
    fn sync_advanced(&self, win: &MainWindow) {
        let raw = self.api().raw_mv().ok();
        let raw_text =
            |slot: Option<u16>| slot.map_or_else(|| "—".to_string(), |mv| mv_text(f64::from(mv)));
        set_if_changed(
            win.get_throttle_raw_mv_text(),
            raw_text(raw.and_then(|r| r.accel_mv)).into(),
            |v| win.set_throttle_raw_mv_text(v),
        );
        set_if_changed(
            win.get_brake_raw_mv_text(),
            raw_text(raw.and_then(|r| r.brake_mv)).into(),
            |v| win.set_brake_raw_mv_text(v),
        );
        set_if_changed(
            win.get_clutch_raw_mv_text(),
            raw_text(raw.and_then(|r| r.clutch_mv)).into(),
            |v| win.set_clutch_raw_mv_text(v),
        );
    }

    /// Re-sync the pages from the slow cache when the daemon's
    /// slow values changed (F5): the poller bumps
    /// `cache_generation` once per slow cycle, so a daemon-side
    /// change reaches the UI within ~1 s without any UI-thread
    /// IPC. Every read here is a cache read. Skipped while not
    /// live — the sim's generation never moves, and the 5 s tick
    /// already covers its (unchanging) lists.
    fn resync_from_cache(&self, win: &MainWindow) {
        if !self.is_live() {
            return;
        }
        let generation = self.api().cache_generation();
        if generation == self.last_cache_generation.get() {
            return;
        }
        self.last_cache_generation.set(generation);
        self.refresh_calibration();
        self.sync_advanced(win);
        self.sync_pedals(win);
        self.sync_extras(win);
        self.sync_overview(win);
        self.refresh_profiles(win);
        self.sync_diag(win);
        self.sync_keep_in_tray(win);
        self.sync_vjoy(win);
    }

    /// Short date ("2026-11-16") from an RFC 3339 capture timestamp.
    fn calibration_date(&self) -> Option<String> {
        self.calibration
            .borrow()
            .as_ref()
            .and_then(|c| c.calibrated_at.as_ref())
            .map(|s| s.chars().take(10).collect())
    }

    /// Cached dashed-line commands (DS-78): constant, built once.
    fn dashed_line_cached(&self) -> SharedString {
        self.frame_cmds.borrow_mut().dashed_line()
    }

    /// Cached curve commands for the current tuning (DS-78).
    fn curve_commands_cached(&self, deadzone: f32, exponent: f32) -> SharedString {
        self.frame_cmds.borrow_mut().curve(deadzone, exponent)
    }

    /// Cached crosshair commands for the current marker (DS-78).
    fn crosshair_commands_cached(&self, mx: f32, my: f32) -> SharedString {
        self.frame_cmds.borrow_mut().crosshair(mx, my)
    }

    /// Record one frame's `refresh` time (DS-82): ring of the last
    /// 600 frames (30 s at 20 Hz).
    fn record_frame_time(&self, elapsed: Duration) {
        let mut times = self.frame_times.borrow_mut();
        times.push_back(elapsed);
        while times.len() > 600 {
            times.pop_front();
        }
    }

    /// Frame-time p50/p99 over the recorded window (DS-82), in ms.
    /// `None` until a frame has been recorded.
    fn frame_time_p50_p99(&self) -> Option<(f64, f64)> {
        percentile_p50_p99(&self.frame_times.borrow())
    }

    /// Paint-completion p50/p99 (R2-46): same window, but the
    /// durations run to the Slint `AfterRendering` notifier — Rust
    /// work + layout + paint. `None` until a paint is observed
    /// (e.g. a backend without notifier support never fills it).
    fn frame_paint_p50_p99(&self) -> Option<(f64, f64)> {
        percentile_p50_p99(&self.frame_paint_times.borrow())
    }

    /// Push the current API + tuning state into the window (one
    /// frame). Live values only: no daemon means Disconnected +
    /// "—", never fake Online (acceptance); the pressed tile is
    /// always -1 — the snapshot carries no button state (A5); the
    /// handbrake is 0 and the gear "—" — neither exists in the
    /// snapshot (A7).
    ///
    /// B1 (v2.0.2): the sim's sine-wave snapshot is NEVER displayed
    /// — without a live daemon the wheel/pedals read "—"/0 and the
    /// charts stay empty, even though the sim is the active API.
    #[allow(clippy::cast_possible_truncation, clippy::too_many_lines)]
    fn push_frame(&self, win: &MainWindow, tick: u64) -> bool {
        let (api, live, connecting) = {
            let state = self.state.borrow();
            (state.api.clone(), state.live, state.connecting)
        };
        let conn = api.connection();
        // H1: remember the live session's transport every frame, so
        // Reconnect re-uses it (USB stays USB).
        if live {
            self.state.borrow_mut().last_transport = conn.transport;
        }
        // B1: no live daemon → zeroed snapshot, never the sim sine.
        let snap = if live {
            api.snapshot()
        } else {
            SnapshotInfo::default()
        };
        let connected = live && conn.label == "Connected";
        let active = live && matches!(conn.state, ConnectionState::Active);
        // D15: the board is REACHABLE once the daemon's sticky
        // `board_reachable` latch is set (a HELLO was observed) or
        // the session is Active. `LinkUp` alone does NOT prove the
        // board is there (transport bound, still waiting for a
        // HELLO), so it must not unlock the wizard's Connect step.
        // NEVER gated on Active/pairing — the latch is the signal.
        let reachable = live && (conn.board_reachable || active);
        // DS-67: a rejoining board clears the flag once the session
        // is Active again — the rejoin is done, the link is back.
        // This runs BEFORE the frame-rest comparison below: the
        // clear itself is a visible change (the flag is an input),
        // so clearing it here can never be skipped away.
        if active && self.wifi_rejoining.get() {
            self.wifi_rejoining.set(false);
            self.wifi_rejoin_since.set(None);
        }
        // Perf (F-4): rest the frame when nothing moved. An idle /
        // disconnected frame used to re-run ~52 guarded Slint
        // writes + 4 history pushes every 50 ms even though no
        // value changed; now it costs one cache read + compare and
        // returns. Returns `false` when skipped, `true` when the
        // frame ran — `refresh` gates the capture sync on it (same
        // inputs, same rest).
        let diag_open = win.get_diag_open();
        let inputs = FrameInputs {
            conn: conn.clone(),
            snap: snap.clone(),
            tuning: {
                let tuning = self.tuning.borrow();
                (
                    tuning.deadzone_pct,
                    tuning.exponent,
                    tuning.preset,
                    tuning.smoothing_pct,
                    tuning.invert,
                    tuning.lock_deg,
                )
            },
            pedals: self.pedals.borrow().clone(),
            handbrake: self.handbrake_record.borrow().clone(),
            unsaved: self.unsaved.get(),
            horn_flash: test_flash_active(self.horn_test_until.get()),
            handbrake_flash: test_flash_active(self.handbrake_test_until.get()),
            raw_available: self.raw_available.get(),
            raw_job_running: self.busy_jobs.borrow().contains_key(&JobKind::RawAvailable),
            diag_open,
            output_enabled: api.output_enabled(),
            live,
            connecting,
        };
        {
            let mut last = self.last_frame.borrow_mut();
            if last.as_ref() == Some(&inputs) {
                return false;
            }
            *last = Some(inputs);
        }
        let deadzone;
        let exponent;
        {
            let tuning = self.tuning.borrow();
            deadzone = tuning.deadzone_pct;
            exponent = tuning.exponent;
        }
        let steer_deg = snap.steering_deg as f32;
        let input_pct = (steer_deg.abs() / 180.0 * 100.0).clamp(0.0, 100.0);
        let output_pct = curve_output(input_pct, deadzone, exponent);
        let mx = VX0 + input_pct / 100.0 * (VX1 - VX0);
        let my = VY1 - output_pct / 100.0 * (VY1 - crate::curve::VY0);

        // DS-78: every prop below is set only when it moved — an
        // unchanged `set_*` still marks the property dirty and
        // repaints, so a 20 Hz frame that re-sets ~50 props costs a
        // repaint even when nothing moved.
        set_if_changed(win.get_connected(), connected, |v| win.set_connected(v));
        set_if_changed(
            win.get_conn_label(),
            self.frame_cmds.borrow_mut().conn_label(
                connecting,
                live,
                &conn.label,
                self.wifi_rejoining.get(),
            ),
            |v| win.set_conn_label(v),
        );
        // R2-19: the wizard's Connect step is done exactly when the
        // board is REACHABLE on the LIVE daemon — re-derived on
        // every frame, so a reconnect also advances the wizard. The
        // sim reports Active too — it must never advance the wizard.
        set_if_changed(win.get_setup_connect_done(), reachable, |v| {
            win.set_setup_connect_done(v);
        });
        set_if_changed(
            win.get_setup_wifi_joined(),
            reachable && conn.transport == ApiTransport::Wifi,
            |v| {
                win.set_setup_wifi_joined(v);
            },
        );
        set_if_changed(
            win.get_setup_usb_available(),
            reachable && conn.transport == ApiTransport::Usb,
            |v| {
                win.set_setup_usb_available(v);
            },
        );
        // R2-19: step 0 shows WHY it cannot advance — the hint is
        // the connection error's own hint (daemon-provided when
        // present, else the typed reason's), so a stuck Connect step
        // explains itself.
        let connect_hint = if !live {
            "RSH isn't running — reconnect to try again.".to_string()
        } else if let ConnectionState::Error { reason, hint, .. } = &conn.state {
            if hint.is_empty() {
                conn.hint
                    .clone()
                    .unwrap_or_else(|| reason.hint().to_string())
            } else {
                hint.clone()
            }
        } else {
            String::new()
        };
        set_if_changed(win.get_setup_connect_hint(), connect_hint.into(), |v| {
            win.set_setup_connect_hint(v);
        });
        // D16: the Connect step's Wi-Fi card shows the network the
        // daemon found ("" = none yet) — a cache read, safe here.
        {
            let ssid = self.api().wifi_status().ssid.clone().unwrap_or_default();
            set_if_changed(win.get_setup_wifi_ssid(), ssid.into(), |v| {
                win.set_setup_wifi_ssid(v);
            });
        }
        // D16: the Calibrate step's pedal graphs are live — same
        // snapshot values as the Overview pedal bars.
        set_if_changed(
            win.get_setup_live_throttle(),
            (snap.throttle * 100.0) as f32,
            |v| {
                win.set_setup_live_throttle(v);
            },
        );
        set_if_changed(
            win.get_setup_live_brake(),
            (snap.brake * 100.0) as f32,
            |v| {
                win.set_setup_live_brake(v);
            },
        );
        set_if_changed(
            win.get_setup_live_clutch(),
            (snap.clutch.unwrap_or(0.0) * 100.0) as f32,
            |v| win.set_setup_live_clutch(v),
        );
        let transport_label = match conn.transport {
            rsh_api::ApiTransport::Wifi => "Wi-Fi",
            rsh_api::ApiTransport::Usb => "USB",
        };
        set_if_changed(
            win.get_conn_sub(),
            self.frame_cmds
                .borrow_mut()
                .conn_sub(connecting, live, transport_label, &conn.address),
            |v| win.set_conn_sub(v),
        );
        set_if_changed(win.get_send_to_game(), api.output_enabled(), |v| {
            win.set_send_to_game(v);
        });
        set_if_changed(win.get_steer_angle(), steer_deg, |v| win.set_steer_angle(v));
        // B1: no live daemon → "—", never a sine angle (cached —
        // see `FrameCmdCache::steer_text`).
        set_if_changed(
            win.get_steer_text(),
            self.frame_cmds.borrow_mut().steer_text(live, steer_deg),
            |v| win.set_steer_text(v),
        );
        set_if_changed(win.get_throttle(), (snap.throttle * 100.0) as f32, |v| {
            win.set_throttle(v);
        });
        set_if_changed(win.get_brake(), (snap.brake * 100.0) as f32, |v| {
            win.set_brake(v);
        });
        set_if_changed(
            win.get_clutch(),
            (snap.clutch.unwrap_or(0.0) * 100.0) as f32,
            |v| win.set_clutch(v),
        );
        // H10/M-3: handbrake + gear come from the live snapshot now
        // (0 / "—" only without live data — the snapshot defaults).
        set_if_changed(win.get_handbrake(), (snap.handbrake * 100.0) as f32, |v| {
            win.set_handbrake(v);
        });
        set_if_changed(
            win.get_gear(),
            self.frame_cmds.borrow_mut().gear_text(snap.gear),
            |v| win.set_gear(v),
        );
        set_if_changed(win.get_input_pct(), input_pct, |v| win.set_input_pct(v));
        set_if_changed(win.get_output_pct(), output_pct, |v| win.set_output_pct(v));
        set_if_changed(win.get_marker_vx(), mx, |v| win.set_marker_vx(v));
        set_if_changed(win.get_marker_vy(), my, |v| win.set_marker_vy(v));
        set_if_changed(win.get_dashed_line(), self.dashed_line_cached(), |v| {
            win.set_dashed_line(v);
        });
        set_if_changed(
            win.get_crosshair(),
            self.crosshair_commands_cached(mx, my),
            |v| {
                win.set_crosshair(v);
            },
        );
        set_if_changed(
            win.get_curve_commands(),
            self.curve_commands_cached(deadzone, exponent),
            |v| win.set_curve_commands(v),
        );
        set_if_changed(win.get_deadzone(), deadzone / 30.0, |v| win.set_deadzone(v));
        set_if_changed(
            win.get_deadzone_text(),
            self.frame_cmds.borrow_mut().deadzone_text(deadzone),
            |v| {
                win.set_deadzone_text(v);
            },
        );
        // A24: slider range is 1.0..2.0 now, so the normalised value
        // is exponent - 1.0 (was / 2.0 for the old 1.0..3.0 range).
        set_if_changed(win.get_curve_val(), exponent - 1.0, |v| {
            win.set_curve_val(v);
        });
        set_if_changed(
            win.get_curve_text(),
            self.frame_cmds.borrow_mut().curve_text(exponent),
            |v| win.set_curve_text(v),
        );
        set_if_changed(win.get_preset(), self.tuning.borrow().preset, |v| {
            win.set_preset(v);
        });
        // Bug 7: Advanced controls — invert, lock (fraction of the
        // 270–900° range), smoothing — sync from the same local
        // tuning state the Basic sliders use.
        set_if_changed(win.get_steer_invert(), self.tuning.borrow().invert, |v| {
            win.set_steer_invert(v);
        });
        set_if_changed(
            win.get_steer_lock(),
            (self.tuning.borrow().lock_deg - 270.0) / 630.0,
            |v| win.set_steer_lock(v),
        );
        set_if_changed(
            win.get_steer_lock_text(),
            self.frame_cmds
                .borrow_mut()
                .lock_text(self.tuning.borrow().lock_deg),
            |v| win.set_steer_lock_text(v),
        );
        set_if_changed(
            win.get_steer_smoothing(),
            self.tuning.borrow().smoothing_pct / 100.0,
            |v| win.set_steer_smoothing(v),
        );
        set_if_changed(
            win.get_steer_smoothing_text(),
            self.frame_cmds
                .borrow_mut()
                .smoothing_text(self.tuning.borrow().smoothing_pct),
            |v| win.set_steer_smoothing_text(v),
        );
        // --- Steering per-module tuning ---
        // S2 balance: slider fraction over the −20..+20% range
        // (0.5 = even); S3 trim: fraction over −5..+5% (0.5 = none).
        set_if_changed(
            win.get_steer_balance(),
            (self.tuning.borrow().balance_pct + 20.0) / 40.0,
            |v| win.set_steer_balance(v),
        );
        set_if_changed(
            win.get_steer_balance_text(),
            self.frame_cmds
                .borrow_mut()
                .balance_text(self.tuning.borrow().balance_pct),
            |v| win.set_steer_balance_text(v),
        );
        set_if_changed(
            win.get_steer_trim(),
            (self.tuning.borrow().trim_pct + 5.0) / 10.0,
            |v| win.set_steer_trim(v),
        );
        set_if_changed(
            win.get_steer_trim_text(),
            self.frame_cmds
                .borrow_mut()
                .trim_text(self.tuning.borrow().trim_pct),
            |v| win.set_steer_trim_text(v),
        );
        set_if_changed(win.get_unsaved(), self.unsaved.get(), |v| {
            win.set_unsaved(v);
        });
        // H10: pressed-index comes ONLY from live button state — the
        // first pressed wheel button (1–12) highlights its tile
        // (including while learning — the physical button being held
        // lights up); -1 when nothing is pressed.
        let pressed_index = if live {
            (0..12)
                .find(|i| snap.button_mask & (1_u64 << i) != 0)
                .unwrap_or(-1)
        } else {
            -1
        };
        set_if_changed(win.get_pressed_index(), pressed_index, |v| {
            win.set_pressed_index(v);
        });
        // Pedal mini-curve markers (A1): live pct + cached feel.
        {
            let pedals = self.pedals.borrow();
            let pcts = [
                snap.throttle as f32 * 100.0,
                snap.brake as f32 * 100.0,
                snap.clutch.unwrap_or(0.0) as f32 * 100.0,
            ];
            let mut markers = [(6.0_f32, 104.0_f32); 3];
            for (i, pct) in pcts.iter().enumerate() {
                let dz = pedals.records[i].deadzone as f32;
                markers[i] = pedal_marker(*pct, dz, pedals.feel_exp[i]);
            }
            set_if_changed(win.get_throttle_marker_x(), markers[0].0, |v| {
                win.set_throttle_marker_x(v);
            });
            set_if_changed(win.get_throttle_marker_y(), markers[0].1, |v| {
                win.set_throttle_marker_y(v);
            });
            set_if_changed(win.get_brake_marker_x(), markers[1].0, |v| {
                win.set_brake_marker_x(v);
            });
            set_if_changed(win.get_brake_marker_y(), markers[1].1, |v| {
                win.set_brake_marker_y(v);
            });
            set_if_changed(win.get_clutch_marker_x(), markers[2].0, |v| {
                win.set_clutch_marker_x(v);
            });
            set_if_changed(win.get_clutch_marker_y(), markers[2].1, |v| {
                win.set_clutch_marker_y(v);
            });
        }
        // Handbrake mini-curve marker (R2-10): live like the pedals
        // above — it used to be pinned to rest (6, 104) by
        // `sync_extras` and never moved with the wheel. The marker
        // math is the same `pedal_marker` (input → curve output,
        // deadzone applied) over the cached handbrake record, so a
        // drag's local deadzone edit moves it at once too.
        {
            let hb = self.handbrake_record.borrow();
            let pct = snap.handbrake as f32 * 100.0;
            let (mx, my) = pedal_marker(pct, hb.deadzone as f32, hb.curve as f32);
            set_if_changed(win.get_handbrake_marker_x(), mx, |v| {
                win.set_handbrake_marker_x(v);
            });
            set_if_changed(win.get_handbrake_marker_y(), my, |v| {
                win.set_handbrake_marker_y(v);
            });
        }
        // Horn test flash (Extras): lights the indicator briefly.
        let horn_pressed = test_flash_active(self.horn_test_until.get());
        set_if_changed(win.get_horn_pressed(), horn_pressed, |v| {
            win.set_horn_pressed(v);
        });
        // R2-17: handbrake Button-mode test flash — same 400 ms
        // indicator as Horn Test, driven by `handbrake_test_until`.
        let handbrake_pressed = test_flash_active(self.handbrake_test_until.get());
        set_if_changed(win.get_handbrake_pressed(), handbrake_pressed, |v| {
            win.set_handbrake_pressed(v);
        });
        // Live chart histories (A20) + chart props every 10th tick.
        // B1: histories only grow while live — without a daemon the
        // charts stay empty ("—") instead of drawing the sim sine
        // or a flat zero line as if it were data.
        if live {
            let mut steer_hist = self.steer_hist.borrow_mut();
            steer_hist.push_back(steer_deg);
            while steer_hist.len() > CHART_SAMPLES {
                steer_hist.pop_front();
            }
            // R2-10 (pedals live graphs): throttle / brake / clutch
            // all push every live frame, on this same 20 Hz path as
            // the steering history — the pedals chart used to record
            // throttle only, so brake and clutch had no trace at all.
            let mut pedal_hist = self.pedal_hist.borrow_mut();
            pedal_hist.push_back(snap.throttle as f32);
            while pedal_hist.len() > CHART_SAMPLES {
                pedal_hist.pop_front();
            }
            let mut brake_hist = self.brake_hist.borrow_mut();
            brake_hist.push_back(snap.brake as f32);
            while brake_hist.len() > CHART_SAMPLES {
                brake_hist.pop_front();
            }
            let mut clutch_hist = self.clutch_hist.borrow_mut();
            clutch_hist.push_back(snap.clutch.unwrap_or(0.0) as f32);
            while clutch_hist.len() > CHART_SAMPLES {
                clutch_hist.pop_front();
            }
            if tick.is_multiple_of(10) {
                if diag_open {
                    // Perf (F-8): the chart strings cost ~800 float
                    // formats/s — build them only while Diagnostics
                    // is open. Histories above still push every live
                    // frame, so opening Diagnostics finds a full
                    // 10 s trace waiting.
                    set_if_changed(
                        win.get_diag_steer_chart(),
                        chart_commands(&steer_hist, -180.0, 180.0).into(),
                        |v| win.set_diag_steer_chart(v),
                    );
                    set_if_changed(
                        win.get_diag_pedals_chart(),
                        multi_chart_commands(&[&pedal_hist, &brake_hist, &clutch_hist], 0.0, 1.0)
                            .into(),
                        |v| win.set_diag_pedals_chart(v),
                    );
                    set_if_changed(
                        win.get_diag_steer_readout(),
                        self.frame_cmds.borrow_mut().steer_text(live, steer_deg),
                        |v| win.set_diag_steer_readout(v),
                    );
                    set_if_changed(
                        win.get_diag_pedals_readout(),
                        self.frame_cmds.borrow_mut().pedals_readout(
                            snap.throttle,
                            snap.brake,
                            snap.clutch.unwrap_or(0.0),
                        ),
                        |v| win.set_diag_pedals_readout(v),
                    );
                } else {
                    // Diagnostics closed: clear the stale chart once
                    // (`set_if_changed` no-ops after that) instead
                    // of formatting it for a hidden page.
                    set_if_changed(win.get_diag_steer_chart(), "".into(), |v| {
                        win.set_diag_steer_chart(v);
                    });
                    set_if_changed(win.get_diag_pedals_chart(), "".into(), |v| {
                        win.set_diag_pedals_chart(v);
                    });
                    set_if_changed(
                        win.get_diag_steer_readout(),
                        self.frame_cmds.borrow_mut().dash(),
                        |v| win.set_diag_steer_readout(v),
                    );
                    set_if_changed(
                        win.get_diag_pedals_readout(),
                        self.frame_cmds.borrow_mut().dash(),
                        |v| win.set_diag_pedals_readout(v),
                    );
                }
            }
        } else if tick.is_multiple_of(10) {
            set_if_changed(win.get_diag_steer_chart(), "".into(), |v| {
                win.set_diag_steer_chart(v);
            });
            set_if_changed(win.get_diag_pedals_chart(), "".into(), |v| {
                win.set_diag_pedals_chart(v);
            });
            set_if_changed(
                win.get_diag_steer_readout(),
                self.frame_cmds.borrow_mut().dash(),
                |v| win.set_diag_steer_readout(v),
            );
            set_if_changed(
                win.get_diag_pedals_readout(),
                self.frame_cmds.borrow_mut().dash(),
                |v| win.set_diag_pedals_readout(v),
            );
        }
        true
    }

    fn refresh(&self, win: &MainWindow, tick: u64) {
        // Perf (F-4): the capture sync derives from the same inputs
        // `push_frame` compared — a skipped frame has nothing new
        // for it either, so rest it too.
        if self.push_frame(win, tick) {
            // v2.0.2: capture availability follows the live daemon every
            // frame (see `sync_capture_availability`).
            self.sync_capture_availability(win);
        }
    }

    /// Queue a steering push (M-7): slider drags call this per move,
    /// but the daemon push is debounced — the frame timer flushes it
    /// 150 ms after the last move, so a drag is one IPC call, not one
    /// per pixel. The local slider state already moved (the UI feels
    /// instant); only the daemon push waits.
    fn push_steering(&self) {
        self.steer_push_due
            .set(Some(Instant::now() + Duration::from_millis(150)));
    }

    /// Flush a due steering push (frame timer, M-7): builds a
    /// `TuningSettings` from the local slider state over the cached
    /// stored record and calls `set_tuning`. On success the cached
    /// record updates and the profile is marked unsaved; on failure
    /// the error is shown (never silent) and the local state stays —
    /// the push is retried on the next edit.
    /// Flush a due steering push (frame timer, L4/F6): the push
    /// runs on a [`JobKind::TuningPush`] job thread — a slow daemon
    /// can no longer freeze the UI for ~500 ms per drag flush.
    /// R2-18: the due flag clears only when the job COMPLETES (and
    /// only when this push is still the pending one — a newer drag
    /// supersedes it); on failure or timeout the sliders snap back
    /// to the stored record instead of showing an unapplied value.
    fn flush_steering_push(&self, win: &MainWindow) {
        let Some(due) = self.steer_push_due.get() else {
            return;
        };
        if Instant::now() < due {
            return;
        }
        let base = self.steering.borrow().clone();
        let settings = {
            let tuning = self.tuning.borrow();
            tuning.to_settings(&base)
        };
        let started = self.run_generic_job(
            win,
            JobKind::TuningPush,
            "Saving steering…",
            move |api| JobResult::Tuning(api.set_tuning(&settings).map(Box::new)),
            move |ctx, win, result| {
                let JobResult::Tuning(result) = result else {
                    return;
                };
                // R2-18: clear the due flag only on completion, and
                // only when this push is still the pending one.
                let still_pending = ctx.steer_push_due.get() == Some(due);
                if still_pending {
                    ctx.steer_push_due.set(None);
                }
                ctx.tuning_push_in_flight.borrow_mut().take();
                match result {
                    Ok(stored) => {
                        *ctx.steering.borrow_mut() = *stored;
                        ctx.unsaved.set(true);
                    }
                    Err(e) => {
                        if still_pending {
                            // The push never landed — snap the
                            // sliders back to the stored record
                            // instead of lying about the value.
                            *ctx.tuning.borrow_mut() =
                                Tuning::from_settings(&ctx.steering.borrow());
                            ctx.refresh(win, 0);
                        }
                        ctx.notify_error(win, "Steering", &e);
                        ui_log!("RSH-UI set_tuning failed: {} ({})", e.code, e.title);
                    }
                }
            },
        );
        if started {
            // R2-18: record what is in flight (the due flag stays
            // until the job completes — a timeout still has to
            // revert this push).
            *self.tuning_push_in_flight.borrow_mut() =
                Some(TuningPushTarget::Steering { due, base });
        }
    }

    /// Queue a pedal / Extras slider drag's latest value (M-7): the
    /// drag keeps flowing (the component owns the thumb); the daemon
    /// push runs 150 ms after the last move. A newer move replaces
    /// the pending one — a drag is one IPC call, not one per pixel.
    fn queue_field_push(&self, module: &'static str, kind: FieldKind, value: f64) {
        *self.pending_field.borrow_mut() = Some((
            PendingField {
                module,
                kind,
                value,
            },
            Instant::now() + Duration::from_millis(150),
        ));
    }

    /// Flush a due field push (frame timer, M-7/F6): read the
    /// module's LOCAL stored record, apply the queued field, push
    /// it — the whole push runs on a [`JobKind::TuningPush`] job
    /// thread, so a drag flush can no longer freeze the UI for ~5 s
    /// on a slow daemon. R2-18: the pending flag clears only when
    /// the job COMPLETES (and only when this push is still the
    /// pending one — a newer drag supersedes it); on failure or
    /// timeout the slider snaps back to the stored record instead
    /// of lying. R2-10: on success the page re-syncs from the
    /// `set_tuning` return value, not the stale ~1 Hz cache.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn flush_field_push(&self, win: &MainWindow) {
        let Some((pending, due)) = *self.pending_field.borrow() else {
            return;
        };
        if Instant::now() < due {
            return;
        }
        let module = pending.module;
        let kind = pending.kind;
        let value = pending.value;
        // R2-10: the base record is the LOCAL copy (kept current by
        // drag handlers + flush completions), not the ~1 Hz slow
        // cache — a slow cycle that started before the last push
        // must not seed this push with pre-push values.
        let base = self.local_tuning_record(module);
        let work_base = base.clone();
        let apply_base = base.clone();
        let started = self.run_generic_job(
            win,
            JobKind::TuningPush,
            "Saving tuning…",
            move |api| {
                let mut record = work_base;
                match kind {
                    FieldKind::Deadzone => record.deadzone = value,
                    FieldKind::Bite => record.bite_point = Some(value),
                    // --- Brake per-module tuning ---
                    FieldKind::Force => record.saturation = Some(value),
                    FieldKind::Brightness => record.led_brightness = Some(value.round() as u8),
                    // --- Pedal ramps/curves per-module tuning ---
                    FieldKind::PressMs => record.press_ms = Some(value),
                    FieldKind::ReleaseMs => record.release_ms = Some(value),
                    FieldKind::Curve => record.curve = value,
                    FieldKind::Saturation => record.max_mv = value,
                    FieldKind::EncoderPulseMs => {
                        record.encoder_pulse_ms = Some(value.clamp(20.0, 200.0));
                    }
                    FieldKind::EncoderAccelSens => {
                        record.encoder_accel_sensitivity = Some(value.clamp(0.0, 2.0));
                    }
                    // --- Buttons per-module tuning ---
                    FieldKind::ButtonDebounceMs => {
                        record.button_debounce_ms = Some(value.clamp(0.0, 50.0));
                    }
                    FieldKind::ButtonLongPressMs => {
                        record.button_long_press_ms = Some(value.clamp(300.0, 1500.0));
                    }
                    FieldKind::ButtonDoublePressMs => {
                        record.button_double_press_ms = Some(value.clamp(150.0, 500.0));
                    }
                    // --- Shifter per-module tuning ---
                    FieldKind::ShiftLockoutMs => {
                        record.shift_lockout_ms = Some(value.clamp(0.0, 200.0));
                    }
                    // --- Handbrake/clutch per-module tuning ---
                    FieldKind::BiteWidth => {
                        record.bite_width = Some(value.clamp(0.05, 0.40));
                    }
                    // --- LED/horn per-module tuning ---
                    FieldKind::LedPeriodMs => {
                        record.led_period_ms = Some(value.clamp(100.0, 5000.0));
                    }
                    FieldKind::HornMinMs => {
                        record.horn_min_ms = Some(value.clamp(100.0, 1000.0));
                    }
                    FieldKind::LedColor => {
                        record.led_color = Some(value as u32);
                    }
                }
                JobResult::Tuning(api.set_tuning(&record).map(Box::new))
            },
            move |ctx, win, result| {
                let JobResult::Tuning(result) = result else {
                    return;
                };
                // R2-18: clear the pending flag only on completion,
                // and only when this push is still the pending one.
                let current = *ctx.pending_field.borrow();
                let still_pending = current.is_some_and(|(p, d)| p == pending && d == due);
                if still_pending {
                    *ctx.pending_field.borrow_mut() = None;
                }
                ctx.tuning_push_in_flight.borrow_mut().take();
                match result {
                    Ok(stored) => {
                        ctx.unsaved.set(true);
                        // R2-10: re-sync from the `set_tuning`
                        // return value — the authoritative stored
                        // record — not the ~1 Hz cache.
                        ctx.apply_tuning_record(win, &stored);
                    }
                    Err(e) => {
                        if still_pending {
                            // The push never landed — snap the
                            // slider back to the stored record.
                            ctx.apply_tuning_record(win, &apply_base);
                        }
                        ctx.notify_error(win, "Tuning", &e);
                        ui_log!(
                            "RSH-UI field push {module} failed: {} ({})",
                            e.code,
                            e.title
                        );
                    }
                }
            },
        );
        if started {
            // R2-18: record what is in flight (the pending flag
            // stays until the job completes — a timeout still has
            // to revert this push).
            *self.tuning_push_in_flight.borrow_mut() =
                Some(TuningPushTarget::Field { pending, due, base });
        }
    }

    /// Fetch the board's shifter sensitivity once (SH1): a
    /// [`JobKind::SensitivityPush`] job thread reads it, then the
    /// UI thread caches + seeds the slider. A failed read records
    /// the time (30 s retry backoff) and leaves the slider disabled
    /// with the reason in place — no toast: a background read
    /// failure is not a user action.
    fn fetch_shifter_sensitivity(&self, win: &MainWindow) {
        let started = self.run_generic_job(
            win,
            JobKind::SensitivityPush,
            "Reading shifter sensitivity…",
            move |api| JobResult::Sensitivity(api.shifter_sensitivity()),
            move |ctx, win, result| {
                let JobResult::Sensitivity(result) = result else {
                    return;
                };
                match result {
                    Ok(value) => {
                        ctx.sensitivity_cached.set(Some(value));
                        ctx.sensitivity_failed_at.set(None);
                        ctx.sensitivity_push_base.set(value);
                        ctx.sync_shifter_props(win, &ctx.api().get_tuning("shifter"));
                    }
                    Err(e) => {
                        ctx.sensitivity_failed_at.set(Some(Instant::now()));
                        ui_log!(
                            "RSH-UI shifter sensitivity read failed: {} ({})",
                            e.code,
                            e.title
                        );
                        ctx.sync_shifter_props(win, &ctx.api().get_tuning("shifter"));
                    }
                }
            },
        );
        let _ = started;
    }

    /// Queue a shifter sensitivity drag's latest value (SH1, M-7):
    /// the drag keeps flowing; the board config push runs 150 ms
    /// after the last move. A newer move replaces the pending one —
    /// a drag is one config transaction, not one per pixel.
    fn queue_sensitivity_push(&self, value: u8) {
        *self.pending_sensitivity.borrow_mut() =
            Some((value, Instant::now() + Duration::from_millis(150)));
    }

    /// Flush a due sensitivity push (frame timer, SH1/M-7/F6): the
    /// debounced board-config write runs on a
    /// [`JobKind::SensitivityPush`] job thread, so a drag flush can
    /// no longer freeze the UI. R2-18: the pending flag clears only
    /// when the job COMPLETES (and only when this push is still the
    /// pending one — a newer drag supersedes it); on failure or
    /// timeout the slider snaps back to the last confirmed value
    /// instead of lying.
    fn flush_sensitivity_push(&self, win: &MainWindow) {
        let Some((pending, due)) = *self.pending_sensitivity.borrow() else {
            return;
        };
        if Instant::now() < due {
            return;
        }
        let base = self.sensitivity_push_base.get();
        let started = self.run_generic_job(
            win,
            JobKind::SensitivityPush,
            "Saving shifter sensitivity…",
            move |api| JobResult::Sensitivity(api.set_shifter_sensitivity(pending).map(Some)),
            move |ctx, win, result| {
                let JobResult::Sensitivity(result) = result else {
                    return;
                };
                // R2-18: clear the pending flag only on completion,
                // and only when this push is still the pending one.
                let current = *ctx.pending_sensitivity.borrow();
                let still_pending = current.is_some_and(|(p, d)| p == pending && d == due);
                if still_pending {
                    *ctx.pending_sensitivity.borrow_mut() = None;
                }
                match result {
                    Ok(value) => {
                        // The board confirmed it: the cache is the
                        // applied value now. No profile save — this
                        // is board config, not tuning.
                        ctx.sensitivity_cached.set(Some(value));
                        ctx.sensitivity_push_base.set(value);
                        ctx.sync_shifter_props(win, &ctx.api().get_tuning("shifter"));
                    }
                    Err(e) => {
                        if still_pending {
                            // The push never landed — snap the
                            // slider back to the last confirmed value.
                            ctx.sensitivity_cached.set(Some(base));
                            ctx.sync_shifter_props(win, &ctx.api().get_tuning("shifter"));
                        }
                        ctx.notify_error(win, "Shifter", &e);
                        ui_log!("RSH-UI sensitivity push failed: {} ({})", e.code, e.title);
                    }
                }
            },
        );
        let _ = started;
    }

    /// Sensitivity percent (0..100) from a 0..1 slider fraction
    /// (SH1): rounded + clamped, so the `f64 → u8` cast below is
    /// exact.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn sensitivity_pct(value: f32) -> u8 {
        (f64::from(value) * 100.0).round().clamp(0.0, 100.0) as u8
    }

    /// Echo a sensitivity drag locally (SH1, R2-10): the slider +
    /// text move with the drag, not at the next ~1 Hz re-sync.
    #[allow(clippy::cast_possible_truncation)]
    fn apply_sensitivity_local(win: &MainWindow, value: u8) {
        set_if_changed(
            win.get_shifter_sensitivity(),
            f32::from(value) / 100.0,
            |v| {
                win.set_shifter_sensitivity(v);
            },
        );
        set_if_changed(
            win.get_shifter_sensitivity_text(),
            format!("{value}%").into(),
            |v| win.set_shifter_sensitivity_text(v),
        );
    }

    /// Echo a lockout drag locally (SH2, R2-10): the slider + text
    /// move with the drag. Only the props move — the pending field
    /// push owns the value until it lands.
    #[allow(clippy::cast_possible_truncation)]
    fn apply_shifter_lockout_local(win: &MainWindow, lockout_ms: f64) {
        set_if_changed(
            win.get_shifter_lockout(),
            (lockout_ms / 200.0) as f32,
            |v| {
                win.set_shifter_lockout(v);
            },
        );
        set_if_changed(
            win.get_shifter_lockout_text(),
            format!("{lockout_ms:.0} ms").into(),
            |v| win.set_shifter_lockout_text(v),
        );
    }

    /// Push a VISIBLE notification and sync the drawer model. Every
    /// user-facing handler uses this — never silent, never
    /// eprintln-only (stderr is invisible under
    /// `windows_subsystem = "windows"`). "daemon" wording is
    /// rewritten to "RSH" (L3).
    fn notify(&self, win: &MainWindow, kind: &str, title: &str, body: &str) {
        let title = user_text(title);
        let body = user_text(body);
        self.notifications.borrow_mut().push(kind, &title, &body);
        // M10: the drawer persists — a notification survives a
        // restart (best-effort save; a failed write never blocks
        // the notification itself).
        self.persist_notifications();
        // F27: the entry is kept in the center + on disk, but the
        // model write is skipped while the window is hidden — a
        // model change on a hidden window can drive a draw on the
        // torn-down drawable (the F17-FINDING-5 close-vs-seed
        // SIGSEGV class). `resync_after_reopen` syncs the model
        // when the window comes back, so nothing is lost.
        if !self.hidden.load(Ordering::SeqCst) {
            sync_to_window(&self.notifications.borrow(), win);
        }
    }

    /// Show a typed error gracefully (B6): the title is the plain
    /// headline, the body is what happened + the next step — never a
    /// raw code or stack trace. The daemon maps untyped state errors
    /// to `Unknown` with the real message in `detail`, so a generic
    /// "Something went wrong." is replaced by that detail, and an
    /// "Unknown" title by a plain one.
    ///
    /// R3-D-15: the title is now a human headline, so the
    /// generic-error check keys on `code`, not `title` (the old
    /// `title == "Unknown"` check could never fire once the title
    /// stopped being the raw code).
    fn notify_error(&self, win: &MainWindow, kind: &str, err: &UserError) {
        let title = if err.code == "Unknown" {
            "Couldn't complete that"
        } else {
            err.title.as_str()
        };
        let happened = match (&err.detail, err.what_happened.as_str()) {
            (Some(detail), "Something went wrong.") => detail.clone(),
            _ => err.what_happened.clone(),
        };
        // B02: never join `what_to_do` when it already says what
        // happened (case-insensitive) — TransportClosed used to
        // render "…dropped. Connection to the wheel dropped —
        // reconnecting." with the sentence twice.
        let body = if err.what_to_do.is_empty()
            || happened
                .to_lowercase()
                .contains(&err.what_to_do.to_lowercase())
        {
            happened
        } else {
            format!("{happened} {}", err.what_to_do)
        };
        self.notify(win, kind, title, &body);
        // B01 (was DS-24): the failure also surfaces in place, as
        // a transient status-bar message on the current screen — a
        // user who never opens the bell drawer still sees that an
        // action failed.
        self.set_toast(win, title, &body);
    }

    // D17: `notify_error_urgent` is DELETED — no failure ever
    // auto-opens the drawer any more. The drawer opens only on
    // bell click; every failure shows in the status bar via
    // [`Self::notify_error`] (+ [`Self::set_toast`]) and lands in
    // the drawer + badge for later.

    /// Show a user-initiated success visibly (D17): drawer entry
    /// + a success status-bar message —
    ///   reconnected, firmware updated, controller switched, profile
    ///   ops, game added/removed, wizard finish, backup restored.
    fn notify_success(&self, win: &MainWindow, kind: &str, title: &str, body: &str) {
        self.notify(win, kind, title, body);
        self.set_toast_success(win, title, body);
    }

    /// Drawer entry + status-bar message for a plain-string
    /// message (D17): validation refusals and other non-`UserError`
    /// results — "RSH isn't running", "Admin permission needed",
    /// "Game refused", timeouts. Same status-bar-only rule as
    /// [`Self::notify_error`]: the drawer never auto-opens.
    fn notify_toast(&self, win: &MainWindow, kind: &str, title: &str, body: &str) {
        self.notify(win, kind, title, body);
        self.set_toast(win, title, body);
    }

    /// Show a failure in the bottom StatusBar (B01, was the DS-24
    /// toast): title + body on the current screen for 8 s, then
    /// the 1 Hz tick clears it. Skipped while the window is hidden
    /// (F27: no property writes on a hidden window) — the
    /// notification itself is still stored and synced on reopen.
    /// FULL text, never truncated: the bar elides the one line
    /// visually (H4: the drawer carries the same full text). A
    /// status already showing means this message supersedes it:
    /// `status-more` counts the superseded ones (+N).
    fn set_toast(&self, win: &MainWindow, title: &str, body: &str) {
        if self.hidden.load(Ordering::SeqCst) {
            return;
        }
        let more = if self.status_until.get().is_some() {
            win.get_status_more() + 1
        } else {
            0
        };
        win.set_status_kind("error".into());
        win.set_status_title(title.into());
        win.set_status_text(body.into());
        win.set_status_more(more);
        ui_log!("RSH-UI status shown (error): {title} — {body}");
        self.status_until
            .set(Some(Instant::now() + Duration::from_secs(8)));
    }

    /// Show a success in the bottom StatusBar (B01, was the D17
    /// toast): same 8 s / 4 s rule as [`Self::set_toast`] — success
    /// lives 4 s. Same full text, same +N counter.
    fn set_toast_success(&self, win: &MainWindow, title: &str, body: &str) {
        if self.hidden.load(Ordering::SeqCst) {
            return;
        }
        let more = if self.status_until.get().is_some() {
            win.get_status_more() + 1
        } else {
            0
        };
        win.set_status_kind("success".into());
        win.set_status_title(title.into());
        win.set_status_text(body.into());
        win.set_status_more(more);
        ui_log!("RSH-UI status shown (success): {title} — {body}");
        self.status_until
            .set(Some(Instant::now() + Duration::from_secs(4)));
    }

    /// Clear an expired status-bar message (B01, was the DS-24
    /// toast, 1 Hz): the error lives 8 s / success 4 s from the
    /// last message, then vanishes — a newer message re-arms the
    /// deadline via [`Self::set_toast`].
    fn check_status_expiry(&self, win: &MainWindow) {
        let Some(until) = self.status_until.get() else {
            return;
        };
        if Instant::now() >= until {
            self.clear_status(win);
        }
    }

    /// Clear the status bar now (expiry, ✕, Esc): title, text and
    /// kind reset, the deadline and the +N counter with them.
    fn clear_status(&self, win: &MainWindow) {
        self.status_until.set(None);
        win.set_status_title("".into());
        win.set_status_text("".into());
        win.set_status_kind("error".into());
        win.set_status_more(0);
    }

    /// click (no previous change) always passes.
    fn setup_step_allowed(&self) -> bool {
        let now = Instant::now();
        if self
            .last_setup_step_at
            .get()
            .is_some_and(|last| now.duration_since(last) < SETUP_STEP_DEBOUNCE)
        {
            return false;
        }
        self.last_setup_step_at.set(Some(now));
        true
    }

    /// Current busy state (B5): whether any job runs + the oldest
    /// running job's label (the strip shows one job at a time) + its
    /// [`JobKind::order`] value (DS-54: `-1` = idle — the screens
    /// disable + dim the button that started it).
    fn busy_state(&self) -> (bool, String, i32) {
        let jobs = self.busy_jobs.borrow();
        let (busy, label) = busy_state_for(&jobs);
        (busy, label, busy_kind_for(&jobs))
    }

    /// Drive the B5 busy strip from `busy_jobs` (FC2): a running
    /// job shows the strip with its label, no job hides it. Fed on
    /// every job start/finish/timeout (see [`Ctx::run_job`],
    /// [`Ctx::drain_jobs`], [`Ctx::check_job_timeouts`]) plus a 1 Hz
    /// safety call from the frame timer — the write itself is
    /// change-detected, so the safety call costs nothing.
    fn sync_busy(&self, win: &MainWindow) {
        let (busy, label, kind) = self.busy_state();
        set_busy_props(win, busy, &label, kind);
    }

    /// Re-sample Theme Auto from the cached OS preference (DS-50):
    /// the sampler thread refreshes `os_light` every 30 s; the tick
    /// repaints when the resolved theme changed. A cheap atomic
    /// load + one guarded prop write — no `reg` spawn, no file read.
    fn sync_auto_theme(&self, win: &MainWindow) {
        if self.theme_choice.get() != 2 {
            return;
        }
        let resolved = resolve_theme_mode(2, self.os_light.load(Ordering::Relaxed));
        if win.get_theme() != resolved {
            win.set_theme(resolved);
        }
    }

    /// Point the Settings controller segment at the backend that is
    /// actually live (Bug 4/8): after a failed or mismatched switch
    /// the radio must show the real backend, not the one the user
    /// just clicked. `output_backend()` is the ~1 Hz cache — it
    /// never touches the socket from the UI thread. When the cache
    /// has no known backend yet (`"Unknown"`), fall back to
    /// `fallback_index` (the segment the click flipped away from).
    fn resync_controller_from_live(&self, win: &MainWindow, fallback_index: i32) {
        let index = match self.api().output_backend().as_str() {
            "HIDMaestro" => 0,
            "vJoy" => 1,
            _ => fallback_index,
        };
        win.set_controller(index);
    }

    /// Give up the "Rejoining…" state (DS-67): if the session never
    /// comes back Active within 30 s of a password set, the rejoin
    /// failed and the plain disconnect text is honest again.
    fn check_wifi_rejoin_timeout(&self) {
        if !self.wifi_rejoining.get() {
            return;
        }
        if self
            .wifi_rejoin_since
            .get()
            .is_some_and(|since| since.elapsed() >= Duration::from_secs(30))
        {
            self.wifi_rejoining.set(false);
            self.wifi_rejoin_since.set(None);
        }
    }

    /// Drive the M-2 daemon-error banner (FC2): the fatal reason
    /// from `daemon-status.json` while the daemon is down, empty
    /// (banner hidden) while it is live or no fatal file exists.
    /// Called on every daemon death / return (`mark_dead`,
    /// `reconnect_to_live`, Reconnect failure) plus a 1 Hz safety
    /// call, so a fatal file that lands without a health event
    /// still reaches the banner.
    fn sync_daemon_error(&self, win: &MainWindow) {
        let status = live_probe::read_daemon_status(self.data_dir.as_deref());
        let mut text = daemon_error_for(self.is_live(), status);
        // F3: a respawn latch has no fatal file to read from — show
        // its reason instead of an empty banner. A fatal file still
        // wins when one exists: it is the daemon's own newer word.
        if text.is_empty() && !self.is_live() {
            if let Some(latch) = self.spawn_latch.borrow().as_ref() {
                text.clone_from(latch);
            }
        }
        set_daemon_error_prop(win, &text);
    }

    /// Latch the respawn-failure state (F3): the monitor sent
    /// `HealthEvent::DaemonFatal` because a fresh fatal file blocks
    /// spawning or the respawn budget is exhausted. Drives the
    /// disconnected UI (no-op when already down — the `DaemonDown`
    /// that preceded this already did it), records the reason so the
    /// Overview banner shows it until a live daemon clears it, and
    /// notifies once. A fatal-file reason `mark_dead` already showed
    /// is not notified twice; the budget-exhausted reason always
    /// notifies (there is no fatal file behind it). The existing
    /// Reconnect button is the manual Retry — it resets the budget
    /// once (see [`Ctx::start_reconnect_job`]).
    fn mark_spawn_latched(&self, win: &MainWindow, code: &str, detail: &str) {
        // Only drive the disconnected state when it isn't already
        // down — in the normal flow the `DaemonDown` that preceded
        // this already ran `mark_dead` (and notified), so calling it
        // again would double-notify. At startup (still
        // `connecting`) there was no `DaemonDown`, so it runs here.
        let needs_dead = self.is_live() || self.state.borrow().connecting;
        if needs_dead {
            self.mark_dead(win);
        }
        let text = if detail.trim().is_empty() {
            format!("RSH stopped unexpectedly ({code}) — try Reconnect.")
        } else {
            detail.to_string()
        };
        let already_shown = self
            .last_dead_detail
            .borrow()
            .as_deref()
            .is_some_and(|shown| shown == detail);
        *self.spawn_latch.borrow_mut() = Some(text.clone());
        if code == "RespawnBudgetExhausted" || !already_shown {
            let title = if code == "RespawnBudgetExhausted" {
                "RSH stopped — automatic restart off"
            } else {
                "RSH stopped unexpectedly"
            };
            self.notify(win, "Connection", title, &text);
        }
        self.sync_daemon_error(win);
        self.refresh(win, 0);
    }

    /// Seed the keep-in-tray switch from the API (FC2): the close
    /// handler reads the same value, so the switch never disagrees
    /// with what X will do. R2-11: change-guarded.
    fn sync_keep_in_tray(&self, win: &MainWindow) {
        set_if_changed(win.get_keep_in_tray(), self.api().close_to_tray(), |v| {
            win.set_keep_in_tray(v);
        });
    }

    /// Start a background job (A9/B5): the work runs on its own
    /// thread — the UI thread never blocks on IPC, a spawn, or a
    /// flash. The busy label is recorded immediately (feedback in
    /// <100 ms: the caller notifies + the Reconnect job sets the
    /// `connecting` state), and the result arrives on `job_tx` for
    /// the frame timer to drain. A second job of the same kind while
    /// one is in flight is ignored (no double-click queue) and the
    /// call returns `false`. Loading never goes infinite: the frame
    /// timer times a stale busy entry out (see
    /// [`Ctx::check_job_timeouts`]).
    fn run_job(
        &self,
        win: &MainWindow,
        kind: JobKind,
        label: &str,
        work: impl FnOnce() -> JobDone + Send + 'static,
    ) -> bool {
        if self.busy_jobs.borrow().contains_key(&kind) {
            ui_log!("RSH-UI job {kind:?} already running — ignoring duplicate start");
            return false;
        }
        self.busy_jobs
            .borrow_mut()
            .insert(kind, (label.to_string(), Instant::now()));
        // B5/FC2: the strip shows the job immediately (<100 ms
        // feedback — the frame timer is 50 ms, but the label is
        // already in `busy_jobs`, so drive the props now).
        self.sync_busy(win);
        let tx = self.job_tx.clone();
        let _ = std::thread::Builder::new()
            .name(format!("rsh-job-{kind:?}"))
            .spawn(move || {
                let done = work();
                if tx.send(done).is_err() {
                    ui_log!("RSH-UI job {kind:?} finished after the UI went away");
                }
            });
        true
    }

    /// Start a generic background job (F6): `work` runs on a job
    /// thread (it may call the API — never the UI thread), `apply`
    /// runs once on the UI thread with the result. `apply` may hold
    /// `Rc` state and window props — it never leaves the UI thread.
    /// A second job of the same kind is ignored (same rule as
    /// [`Ctx::run_job`]); the apply is dropped when the job times
    /// out, so a late result never applies.
    fn run_generic_job(
        &self,
        win: &MainWindow,
        kind: JobKind,
        label: &str,
        work: impl FnOnce(Arc<dyn AppApi>) -> JobResult + Send + 'static,
        apply: impl FnOnce(&Ctx, &MainWindow, JobResult) + 'static,
    ) -> bool {
        if self.busy_jobs.borrow().contains_key(&kind) {
            ui_log!("RSH-UI job {kind:?} already running — ignoring duplicate start");
            return false;
        }
        let id = self.next_job_id.get();
        self.next_job_id.set(id + 1);
        self.pending_applies
            .borrow_mut()
            .insert(id, (kind, Box::new(apply)));
        self.busy_jobs
            .borrow_mut()
            .insert(kind, (label.to_string(), Instant::now()));
        self.sync_busy(win);
        let api = self.api();
        let tx = self.job_tx.clone();
        let _ = std::thread::Builder::new()
            .name(format!("rsh-job-{kind:?}"))
            .spawn(move || {
                let result = work(api);
                if tx.send(JobDone::Generic { id, result }).is_err() {
                    ui_log!("RSH-UI job {kind:?} finished after the UI went away");
                }
            });
        true
    }

    /// Drain finished jobs on the UI thread (frame timer): each
    /// result is applied (notify / revert / re-sync) exactly once.
    fn drain_jobs(&self, win: &MainWindow) {
        let rx = self.job_rx.borrow();
        while let Ok(done) = rx.try_recv() {
            self.apply_job_done(win, done);
        }
        // B5/FC2: a finished job clears its busy entry — hide the
        // strip (or fall back to the next running job's label).
        self.sync_busy(win);
    }

    /// Apply one finished job (A9/B5). A result whose kind is no
    /// longer busy was already timed out — ignore it (the timeout
    /// already notified). Every branch notifies visibly on failure
    /// and re-syncs the affected page on success.
    #[allow(clippy::too_many_lines)]
    fn apply_job_done(&self, win: &MainWindow, done: JobDone) {
        match done {
            JobDone::Reconnect(result) => {
                self.reconnect_in_flight.set(false);
                self.busy_jobs.borrow_mut().remove(&JobKind::Reconnect);
                match result {
                    Ok(api) => self.reconnect_to_live(win, api),
                    Err(reason) => {
                        {
                            let mut state = self.state.borrow_mut();
                            state.connecting = false;
                        }
                        self.connecting_since.set(None);
                        // F3: the manual retry failed — re-latch the
                        // respawn budget when it was latched before
                        // (no-op otherwise; the monitor resumes its
                        // bounded attempts with a fresh budget).
                        self.spawn_budget.record_manual_failure();
                        // M-2: the daemon's fatal file names the
                        // reason (IpcBindFailed / DaemonHung) — show
                        // it instead of the generic "not running".
                        if let Some((_code, detail)) =
                            live_probe::read_daemon_status(self.data_dir.as_deref())
                        {
                            self.notify_toast(
                                win,
                                "Connection",
                                "RSH stopped unexpectedly",
                                &detail,
                            );
                        } else {
                            self.notify_toast(win, "Connection", "Couldn't reconnect", &reason);
                        }
                        // FC2: a fatal file names the reason — show
                        // it in the Overview banner too, not just in
                        // the notification drawer.
                        self.sync_daemon_error(win);
                        self.refresh(win, 0);
                    }
                }
            }
            JobDone::ReconnectSeed { api, seed } => {
                // F5: the seed fetch finished on its job thread —
                // apply it exactly like the old `reconnect_to_live`,
                // but from cached values only.
                self.reconnect_in_flight.set(false);
                self.busy_jobs.borrow_mut().remove(&JobKind::ReconnectSeed);
                self.apply_reconnect_seed(win, api, seed);
            }
            JobDone::Generic { id, result } => {
                // F6: a generic job finished — run its registered
                // apply once. A missing entry means the job timed
                // out (the timeout already notified) — ignore the
                // late result.
                let Some((kind, apply)) = self.pending_applies.borrow_mut().remove(&id) else {
                    return;
                };
                self.busy_jobs.borrow_mut().remove(&kind);
                // R2-18: the apply below clears the in-flight tuning
                // record on completion; a hidden-window skip drops
                // the apply, so clear here too — a timeout must never
                // revert a push that already finished.
                if kind == JobKind::TuningPush {
                    self.tuning_push_in_flight.borrow_mut().take();
                }
                // F27: a job that finished after the window was
                // hidden must not apply to it — the apply closures
                // touch the window (props, models, notifies) and
                // the close-vs-apply race SIGSEGVs under Xvfb
                // (F17-FINDING-5). The result is dropped; the
                // frame timer re-derives the display from
                // state/cache when the window comes back.
                if self.hidden.load(Ordering::SeqCst) {
                    ui_log!("RSH-UI job {kind:?} finished while hidden — apply skipped (F27)");
                    return;
                }
                apply(self, win, result);
                self.refresh(win, 0);
            }
            JobDone::Transport { index, result } => {
                if self
                    .busy_jobs
                    .borrow_mut()
                    .remove(&JobKind::Transport)
                    .is_none()
                {
                    return;
                }
                match result {
                    Ok(info) => {
                        ui_log!("RSH-UI transport switch ok: {}", info.label);
                        self.refresh(win, 0);
                    }
                    Err(e) => {
                        if let Some(i) = index {
                            win.set_transport(1 - i);
                        }
                        self.notify_error(win, "Connection", &e);
                    }
                }
            }
            JobDone::Backend { index, result } => {
                if self
                    .busy_jobs
                    .borrow_mut()
                    .remove(&JobKind::Backend)
                    .is_none()
                {
                    return;
                }
                // Bug 4/8: a success toast is only honest when the
                // backend that is actually live is the one the user
                // asked for. The daemon reports the LIVE name, so a
                // silent fallback used to arrive here as
                // `Ok("vJoy")` for a HIDMaestro request and toast
                // "Game controller is now vJoy". A mismatch is a
                // failure now: error toast, radio back on the live
                // backend, no success toast.
                // Both failure paths (mismatch and Err) funnel
                // through the single failure notification below, so
                // a failed switch shows exactly one message.
                let requested = if index == 1 { "vJoy" } else { "HIDMaestro" };
                let failure: Option<UserError> = match result {
                    Ok(active) if active == requested => {
                        self.sync_overview(win);
                        // The wizard's Drivers step mirrors the same
                        // backend choice (fallback flag + controller
                        // segment + any install error cleared).
                        win.set_controller(index);
                        if index == 0 {
                            win.set_setup_vjoy_fallback(true);
                            win.set_setup_vjoy_error("".into());
                        }
                        self.notify_success(
                            win,
                            "Game controller",
                            "Controller switched",
                            &format!("Game controller is now {active}."),
                        );
                        None
                    }
                    Ok(active) => Some(UserError {
                        code: "Unknown".to_string(),
                        title: "Controller switch did not take effect".to_string(),
                        what_happened: format!(
                            "{requested} is not active — {active} is still the game controller."
                        ),
                        what_to_do: format!("Try the switch again, or keep using {active}."),
                        detail: None,
                    }),
                    Err(e) => Some(e),
                };
                if let Some(error) = failure {
                    // A10: revert the switch the binding already
                    // flipped, and show the typed reason (a UAC
                    // decline says admin is needed, not "Unknown").
                    // The radio re-syncs from the live backend
                    // (cached, never the socket) — the flipped
                    // segment is only a guess when the live
                    // backend moved mid-switch.
                    self.resync_controller_from_live(win, 1 - index);
                    self.notify_error(win, "Game controller", &error);
                }
            }
            JobDone::FirmwareCheck(result) => {
                if self
                    .busy_jobs
                    .borrow_mut()
                    .remove(&JobKind::FirmwareCheck)
                    .is_none()
                {
                    return;
                }
                match result {
                    Ok(check) => {
                        *self.firmware_check_cache.borrow_mut() = Some(check.clone());
                        self.sync_overview(win);
                        if check.board_version == "Unknown" {
                            self.notify_toast(
                                win,
                                "Firmware",
                                "Wheel firmware",
                                "Firmware version unavailable — reconnect to see it.",
                            );
                        } else if check.update_available {
                            let bundled = check.bundled_version.as_deref().unwrap_or("?");
                            self.notify_success(
                                win,
                                "Firmware",
                                "Firmware update available",
                                &format!(
                                    "Firmware {bundled} is available — the wheel runs {}.",
                                    check.board_version
                                ),
                            );
                        } else if let Some(bundled) = &check.bundled_version {
                            self.notify_success(
                                win,
                                "Firmware",
                                "Firmware up to date",
                                &format!(
                                    "Wheel firmware {} — up to date (bundled {bundled}).",
                                    check.board_version
                                ),
                            );
                        } else {
                            self.notify_success(
                                win,
                                "Firmware",
                                "Wheel firmware",
                                &format!("Wheel firmware {}", check.board_version),
                            );
                        }
                    }
                    Err(e) => self.notify_error(win, "Firmware", &e),
                }
            }
            JobDone::AppUpdateCheck(result) => {
                if self
                    .busy_jobs
                    .borrow_mut()
                    .remove(&JobKind::AppUpdateCheck)
                    .is_none()
                {
                    return;
                }
                // DS-20: the verdict lands in the Settings line
                // itself (so the answer survives the notification)
                // and as a notification (so the user sees it now).
                // Auto-update stays off — an available update is
                // always a manual installer download from GitHub
                // Releases, never a silent install.
                match result {
                    Ok(AppUpdateVerdict::UpToDate) => {
                        win.set_app_update_text(
                            format!(
                                "You're on the latest version (RSH {}) — auto-update is off",
                                app_version()
                            )
                            .into(),
                        );
                        self.notify_success(
                            win,
                            "Update",
                            "RSH is up to date",
                            &format!("You're on the latest version (RSH {}).", app_version()),
                        );
                    }
                    Ok(AppUpdateVerdict::Available(latest)) => {
                        win.set_app_update_text(
                            format!(
                                "RSH {latest} is available on GitHub Releases — auto-update is off, download the installer there"
                            )
                            .into(),
                        );
                        self.notify_success(
                            win,
                            "Update",
                            "Update available",
                            &format!(
                                "RSH {latest} is available — download the installer from GitHub Releases."
                            ),
                        );
                    }
                    Ok(AppUpdateVerdict::CurrentIsNewer(latest)) => {
                        win.set_app_update_text(
                            format!(
                                "You're on RSH {} (newer than the latest release {latest}) — auto-update is off",
                                app_version()
                            )
                            .into(),
                        );
                        self.notify_success(
                            win,
                            "Update",
                            "RSH is up to date",
                            &format!(
                                "You're on RSH {} — newer than the latest release.",
                                app_version()
                            ),
                        );
                    }
                    Err(e) => self.notify_error(win, "Update", &e),
                }
            }
            JobDone::FirmwareUpdate(result) => {
                if self
                    .busy_jobs
                    .borrow_mut()
                    .remove(&JobKind::FirmwareUpdate)
                    .is_none()
                {
                    return;
                }
                match result {
                    Ok(status) => {
                        if status.running {
                            // D17: the start itself is not a
                            // notification — the busy strip + the
                            // Settings firmware line are the surface
                            // while the flash runs; the finish
                            // toasts (see `poll_firmware_update`).
                            self.firmware_update_running.set(true);
                        } else {
                            // The flash never started — a firmware
                            // flash failure: toast in place (D17 —
                            // the drawer never auto-opens).
                            let err = UserError {
                                code: "firmware_update_failed".to_string(),
                                title: "Firmware update failed".to_string(),
                                what_happened: "The update didn't start.".to_string(),
                                what_to_do: "Try again.".to_string(),
                                detail: None,
                            };
                            self.notify_error(win, "Firmware", &err);
                        }
                    }
                    // The flash start failed — toast in place (D17).
                    Err(e) => self.notify_error(win, "Firmware", &e),
                }
            }
            JobDone::RawAvailable(available) => {
                self.busy_jobs.borrow_mut().remove(&JobKind::RawAvailable);
                // H9: the wizard's Calibrate Next gates on this.
                self.raw_available.set(available);
                self.sync_capture_availability(win);
            }
        }
    }

    /// Time out stale busy jobs (B5, 1 Hz): a job older than its
    /// kind's timeout is removed and reported — loading never hangs
    /// forever. A late result for a timed-out job is ignored by
    /// [`Ctx::apply_job_done`] (its kind is no longer busy). The
    /// Reconnect timeout also clears the `connecting` state.
    fn check_job_timeouts(&self, win: &MainWindow) {
        let now = Instant::now();
        let mut timed_out: Vec<(JobKind, String)> = Vec::new();
        for (kind, (label, started)) in self.busy_jobs.borrow().iter() {
            if now.duration_since(*started) > kind.timeout() {
                timed_out.push((*kind, label.clone()));
            }
        }
        for (kind, label) in timed_out {
            self.busy_jobs.borrow_mut().remove(&kind);
            // F6: drop the timed-out kind's pending applies — the
            // late result must never apply.
            self.pending_applies
                .borrow_mut()
                .retain(|_, (k, _)| *k != kind);
            if kind == JobKind::Reconnect {
                self.reconnect_in_flight.set(false);
                // F3: a timed-out manual retry counts as a failed
                // retry — re-latch when it was latched before (the
                // late job result, if any, is ignored by
                // `apply_job_done`, so this is the only accounting).
                self.spawn_budget.record_manual_failure();
                {
                    let mut state = self.state.borrow_mut();
                    state.connecting = false;
                }
                self.connecting_since.set(None);
            }
            if kind == JobKind::ReconnectSeed {
                // F5: the seed fetch never resolved — same cleanup
                // as a timed-out Reconnect.
                self.reconnect_in_flight.set(false);
                {
                    let mut state = self.state.borrow_mut();
                    state.connecting = false;
                }
                self.connecting_since.set(None);
            }
            if kind == JobKind::Backend {
                // The switch already flipped; the late result is
                // ignored, so restore the live backend's segment now.
                let backend = self.api().output_backend();
                win.set_controller(i32::from(backend != "HIDMaestro"));
            }
            if kind == JobKind::SettingsOp {
                // R3-UI-02: the timed-out job's apply is dropped
                // (see the `retain` above), so its busy-clear never
                // runs — clear the Wi-Fi password button here, or
                // it would stay "Setting…" forever. Only one
                // SettingsOp runs at a time, and the busy prop is
                // true only while a Wi-Fi password job is in
                // flight — so for any other SettingsOp timeout
                // this is a no-op clear of an already-false prop.
                win.set_wifi_password_busy(false);
            }
            // R2-18: a timed-out tuning push reverts its slider to
            // the stored value — the UI must not keep showing a
            // value the daemon never received. A push superseded by
            // a newer drag reverts nothing and stays silent (the
            // newer push is still pending and resolves on its own);
            // a push with no in-flight record (typed path, which
            // never echoes locally) needs no revert either — the
            // slider already shows the stored value.
            let mut tuning_reverted = false;
            let mut tuning_silent = false;
            if kind == JobKind::TuningPush {
                if let Some(target) = self.tuning_push_in_flight.borrow_mut().take() {
                    tuning_reverted = self.revert_tuning_push(win, &target);
                    tuning_silent = !tuning_reverted;
                }
            }
            // SH1: a timed-out sensitivity push reverts its slider
            // to the last confirmed value. A timed-out one-shot
            // READ has no pending push — no slider to revert, and
            // the 30 s backoff path shows the reason in place —
            // so it stays silent instead of toasting a background
            // read.
            let mut sensitivity_reverted = false;
            let mut sensitivity_silent = false;
            if kind == JobKind::SensitivityPush {
                if self.pending_sensitivity.borrow().is_some() {
                    *self.pending_sensitivity.borrow_mut() = None;
                    let base = self.sensitivity_push_base.get();
                    self.sensitivity_cached.set(Some(base));
                    self.sync_shifter_props(win, &self.api().get_tuning("shifter"));
                    sensitivity_reverted = true;
                } else {
                    self.sensitivity_failed_at.set(Some(Instant::now()));
                    sensitivity_silent = true;
                }
            }
            ui_log!("RSH-UI job {kind:?} timed out ({label})");
            if !tuning_silent && !sensitivity_silent {
                let body = if tuning_reverted || sensitivity_reverted {
                    "The change wasn't saved — the slider was put back to its previous value."
                } else if kind == JobKind::Backend {
                    // Bug 4: a switch that never answers must say so
                    // plainly — the previous controller is still
                    // active, nothing silently changed under the user.
                    "The controller switch took too long — the previous controller is still active. Try again."
                } else {
                    "The operation took too long — try again."
                };
                self.notify(
                    win,
                    "Loading timed out",
                    &format!("{label} timed out"),
                    body,
                );
                // DS-24: a timeout is a failure — it toasts in place
                // like any other error, not just behind the bell.
                self.set_toast(win, &format!("{label} timed out"), body);
                // D17: a timeout toasts in place — it never
                // auto-opens the drawer.
                self.refresh(win, 0);
            }
        }
        // B5/FC2: a timed-out job leaves the busy map — hide the
        // strip (or fall back to the next running job's label).
        self.sync_busy(win);
    }

    /// Clear a `connecting` state older than 15 s (A9): the first
    /// probe or a Reconnect that never resolves must not leave
    /// "Connecting…" on screen forever.
    fn check_connecting_timeout(&self, win: &MainWindow) {
        if let Some(since) = self.connecting_since.get() {
            if since.elapsed() > Duration::from_secs(15) {
                self.connecting_since.set(None);
                {
                    let mut state = self.state.borrow_mut();
                    state.connecting = false;
                }
                self.notify_toast(
                    win,
                    "Connection",
                    "Couldn't reach RSH",
                    "RSH didn't answer — try Reconnect.",
                );
                self.refresh(win, 0);
            }
        }
    }

    /// Poll a running firmware update (H-4): the daemon flashes on
    /// its own thread; the UI asks once a second and reports the
    /// finish. While it runs, the Settings firmware line shows live
    /// progress instead of going silent.
    fn poll_firmware_update(&self, win: &MainWindow) {
        if !self.firmware_update_running.get() {
            return;
        }
        let st = self.api().firmware_update_status();
        if st.done {
            self.firmware_update_running.set(false);
            if st.success {
                if let Some(version) = st.version.clone() {
                    self.notify_success(
                        win,
                        "Firmware",
                        "Firmware updated",
                        &format!(
                            "Wheel firmware {version} installed — the wheel restarted with it."
                        ),
                    );
                } else {
                    self.notify_success(
                        win,
                        "Firmware",
                        "Firmware updated",
                        "The wheel restarted with the new firmware.",
                    );
                }
                *self.firmware_check_cache.borrow_mut() = self.api().firmware_check().ok();
                self.sync_overview(win);
            } else {
                let err = UserError {
                    code: "firmware_update_failed".to_string(),
                    title: "Firmware update failed".to_string(),
                    what_happened: st
                        .error
                        .clone()
                        .unwrap_or_else(|| "The wheel did not take the new firmware.".to_string()),
                    what_to_do: "Try again.".to_string(),
                    detail: None,
                };
                self.notify_error(win, "Firmware", &err);
                // The failure must not leave the stale "Updating
                // firmware… uploading 42%" line in Settings — re-sync
                // so the firmware line shows the real state again.
                self.sync_overview(win);
            }
            return;
        }
        if !st.running {
            // H-4 fail-closed: the flash was running (the flag is
            // set) but the daemon's status slot is idle — neither
            // running nor finished. The daemon died or restarted
            // mid-flash, so no finish will ever arrive; showing
            // "Updating firmware…" forever would be an infinite
            // loading state (B5). Report the interruption and reset
            // the firmware line instead.
            self.firmware_update_running.set(false);
            let err = UserError {
                code: "firmware_update_failed".to_string(),
                title: "Firmware update interrupted".to_string(),
                what_happened:
                    "RSH restarted while the firmware was updating, so the update did not finish."
                        .to_string(),
                what_to_do: "Check the wheel is on and try the update again.".to_string(),
                detail: None,
            };
            self.notify_error(win, "Firmware", &err);
            self.sync_overview(win);
            return;
        }
        let stage = st.stage.as_deref().unwrap_or("updating");
        let percent = st
            .percent
            .map_or_else(|| "…".to_string(), |p| format!("{p}%"));
        win.set_firmware_text(format!("Updating firmware… {stage} {percent}").into());
    }

    /// Start the Reconnect job (A9): probe + spawn on a background
    /// thread (6 retries ≈ 3 s), never on the UI thread. The
    /// `connecting` state is set immediately — the Overview shows
    /// "Connecting…" while the probe runs. A second click while one
    /// is in flight is ignored.
    ///
    /// F3 (v2.0.3): the click resets the respawn budget once — this
    /// manual retry is the single attempt, and the health monitor
    /// stays out until it resolves (success clears the budget,
    /// failure re-latches it when it was latched before).
    fn start_reconnect_job(&self, win: &MainWindow) {
        if self.reconnect_in_flight.get() {
            return;
        }
        self.reconnect_in_flight.set(true);
        self.spawn_budget.reset_for_manual_retry();
        {
            let mut state = self.state.borrow_mut();
            state.connecting = true;
        }
        self.connecting_since.set(Some(Instant::now()));
        self.refresh(win, 0);
        let data_dir = self.data_dir.clone();
        self.run_job(win, JobKind::Reconnect, "Reconnecting…", move || {
            match live_probe::try_live_api_with_spawn_n(data_dir.as_deref(), 6) {
                Some(api) => JobDone::Reconnect(Ok(api)),
                None => {
                    JobDone::Reconnect(Err("RSH didn't answer — it isn't running.".to_string()))
                }
            }
        });
    }

    /// Start a transport switch job (A9): `connect` runs on a
    /// background thread. `index` is the Settings segment (0 Wi-Fi,
    /// 1 USB) when the switch came from Settings, `None` for the
    /// wizard's Connect step (no switch to revert).
    fn start_transport_job(&self, win: &MainWindow, transport: ApiTransport, index: Option<i32>) {
        // R2-15: no live daemon → the sim would answer `Ok(Active)`
        // and the wizard would sit with Next disabled and no error.
        // Refuse visibly instead of running the job — never a silent
        // Ok against the sim.
        if !self.is_live() {
            if let Some(i) = index {
                win.set_transport(1 - i);
            }
            self.notify_toast(
                win,
                "Connection",
                "RSH isn't running",
                "Reconnect to RSH, then try again.",
            );
            return;
        }
        // R2-02: the old early-return swallowed every same-transport
        // retry — `connection().transport` reports Wi-Fi even while
        // disconnected, so the wizard's "Use Wi-Fi" (and any retry)
        // did nothing with no job and no notification. Return early
        // only when the session is genuinely connected (`Active`);
        // otherwise the daemon `connect` re-triggers a fresh
        // forwarder activation for the transport (WS-A: `Connect`
        // from `Error` → `LinkUp` + `RequestHello`).
        let conn = self.api().connection();
        if conn.transport == transport && matches!(conn.state, ConnectionState::Active) {
            // Already on this transport and connected — the switch
            // already shows it; nothing to do.
            return;
        }
        // D17: the switch itself is not a notification — the busy
        // strip is the surface while it runs.
        let api = self.api();
        self.run_job(
            win,
            JobKind::Transport,
            "Switching connection…",
            move || JobDone::Transport {
                index,
                result: api.connect(transport),
            },
        );
    }

    /// Start an output-backend switch job (A9/B-2): `set_output_backend`
    /// runs on a background thread with its 75 s bound (UAC dwell +
    /// 60 s sidecar spawn). Switching to `HIDMaestro` warns first —
    /// Windows will ask once for admin permission. A second switch
    /// while one is in flight is ignored (no re-spawn loop, A10).
    fn start_backend_job(&self, win: &MainWindow, index: i32) {
        let name = if index == 1 { "vJoy" } else { "HIDMaestro" };
        if self.api().output_backend() == name {
            return;
        }
        if self.busy_jobs.borrow().contains_key(&JobKind::Backend) {
            return;
        }
        if name == "HIDMaestro" {
            // D17: the admin warning toasts in place (the UAC
            // prompt itself follows) — no drawer entry needed
            // beyond the toast's own.
            self.notify_toast(
                win,
                "Game controller",
                "Admin permission needed",
                "HIDMaestro needs admin permission — Windows will ask once.",
            );
        }
        // D17: the switch itself is not a notification — the busy
        // strip is the surface while it runs. The label stays
        // "Switching controller…" for the whole switch (Bug 4/8):
        // the UI has no switch-progress source — the switch runs
        // synchronously inside the daemon's `set_output_backend`
        // call and the sidecar spawn reports no phase — so a
        // time-based relabel to "Waiting for admin permission…"
        // would claim knowledge the UI does not have (it stayed
        // stuck on that label after the prompt was answered). The
        // 75 s timeout already ends the job with a clear error.
        let api = self.api();
        self.run_job(
            win,
            JobKind::Backend,
            "Switching controller…",
            move || JobDone::Backend {
                index,
                result: api.set_output_backend(name),
            },
        );
    }

    /// Start a firmware-check job (M-8): board live version vs the
    /// bundled OTA image version — never the hardcoded "4.0.2".
    fn start_firmware_check_job(&self, win: &MainWindow) {
        // D17: the check itself is not a notification — the busy
        // strip is the surface; the result toasts.
        let api = self.api();
        self.run_job(
            win,
            JobKind::FirmwareCheck,
            "Checking firmware…",
            move || JobDone::FirmwareCheck(api.firmware_check()),
        );
    }

    /// Start an app-update check (DS-20, U-02): the Settings
    /// "Check" button used to answer without checking anything.
    /// Now it really queries GitHub Releases for the latest tag
    /// and reports up-to-date / available / unreachable — with
    /// the busy strip + timeout + result like the firmware check.
    fn start_app_update_check_job(&self, win: &MainWindow) {
        // D17: the check itself is not a notification — the busy
        // strip is the surface; the result toasts.
        self.run_job(
            win,
            JobKind::AppUpdateCheck,
            "Checking for updates…",
            move || JobDone::AppUpdateCheck(fetch_app_update()),
        );
    }

    /// Start a firmware update (H-4): the daemon validates the
    /// bundled image + spawns the flash thread at once, so the start
    /// call itself is fast — the minutes-long flash is polled via
    /// `firmware_update_status()` on the 1 Hz tick.
    fn start_firmware_update_job(&self, win: &MainWindow) {
        // H-4: a flash is already running — a second click must not
        // start another one (the daemon would refuse it with a
        // generic error; saying "already running" here is the honest
        // answer). The start job itself is also guarded by `run_job`.
        if self.firmware_update_running.get() {
            self.notify_toast(
                win,
                "Firmware",
                "Firmware update already running",
                "The wheel firmware is already updating — wait for it to finish.",
            );
            return;
        }
        // D17: the start itself is not a notification — the busy
        // strip + Settings firmware line are the surface.
        let api = self.api();
        self.run_job(
            win,
            JobKind::FirmwareUpdate,
            "Starting firmware update…",
            move || JobDone::FirmwareUpdate(api.update_firmware()),
        );
    }

    /// Start a raw-mV availability probe (H9): any axis reporting
    /// means the wizard's Calibrate step can proceed. Silent — no
    /// notification; the wizard's Next button is the feedback.
    fn start_raw_available_job(&self, win: &MainWindow) {
        let api = self.api();
        self.run_job(
            win,
            JobKind::RawAvailable,
            "Checking wheel data…",
            move || {
                let available = api.raw_mv().is_ok_and(|mv| {
                    mv.steering_mv.is_some()
                        || mv.accel_mv.is_some()
                        || mv.brake_mv.is_some()
                        || mv.clutch_mv.is_some()
                        || mv.handbrake_mv.is_some()
                });
                JobDone::RawAvailable(available)
            },
        );
    }

    /// Validate a Wi-Fi password the user typed (D19): 8–63
    /// printable ASCII characters, both fields matching. Returns
    /// the inline error text, or `None` when it is valid.
    fn wifi_password_error(new: &str, confirm: &str) -> Option<String> {
        if new.len() < 8 || new.len() > 63 {
            return Some("Password must be 8–63 characters.".to_string());
        }
        if !new.chars().all(|c| c.is_ascii_graphic()) {
            return Some("Password must use printable ASCII characters only.".to_string());
        }
        if new != confirm {
            return Some("Passwords don't match.".to_string());
        }
        None
    }

    /// Start a Wi-Fi password set (D19, Settings): validates in
    /// Rust first (inline error + toast, no job), then
    /// `set_wifi_password` runs on a [`JobKind::SettingsOp`] job
    /// thread. Success clears the fields, sets the password-set
    /// state, and arms the "Rejoining…" label — the board restarts
    /// its AP to WPA2, so the link drops on purpose for a moment.
    fn start_set_wifi_password_job(&self, win: &MainWindow) {
        let new = win.get_wifi_password_new().to_string();
        let confirm = win.get_wifi_password_confirm().to_string();
        if let Some(err) = Self::wifi_password_error(&new, &confirm) {
            win.set_wifi_password_error(err.clone().into());
            self.set_toast(win, "Wi-Fi password", &err);
            return;
        }
        win.set_wifi_password_error("".into());
        // R3-UI-02: the per-button "Setting…" state was never set
        // — drive it here, and clear it in the apply closure (both
        // arms) and on timeout (see `check_job_timeouts`, where the
        // apply is dropped). The old success message is stale the
        // moment a new attempt starts.
        win.set_wifi_password_busy(true);
        win.set_wifi_password_message("".into());
        self.wifi_password_message_for.set(None);
        let password = new.clone();
        self.run_generic_job(
            win,
            JobKind::SettingsOp,
            "Saving Wi-Fi password…",
            move |api| {
                JobResult::Text(
                    api.set_wifi_password(&password, &confirm)
                        .map(|_| String::new()),
                )
            },
            move |ctx, win, result| {
                // R3-UI-02: the button un-busy on every outcome —
                // including a non-Text result, which returns below.
                win.set_wifi_password_busy(false);
                let JobResult::Text(result) = result else {
                    return;
                };
                match result {
                    Ok(_) => {
                        win.set_wifi_password_new("".into());
                        win.set_wifi_password_confirm("".into());
                        win.set_wifi_password_error("".into());
                        win.set_wifi_password_message(
                            "Wi-Fi password set — the wheel's hotspot now needs it.".into(),
                        );
                        // R3-UI-02: this message describes the
                        // post-set state (`password_set` = true),
                        // so the 1 Hz re-sync keeps it until the
                        // state moves on.
                        ctx.wifi_password_message_for.set(Some(true));
                        // DS-67: the AP restarts to WPA2 — the link
                        // drops on purpose; say "Rejoining…" until
                        // the session is Active again (or 30 s).
                        ctx.wifi_rejoining.set(true);
                        ctx.wifi_rejoin_since.set(Some(Instant::now()));
                        ctx.sync_wifi_password_state(win);
                        ctx.notify_success(
                            win,
                            "Connection",
                            "Wi-Fi password set",
                            "The wheel's hotspot now needs this password.",
                        );
                    }
                    Err(e) => {
                        win.set_wifi_password_error(e.title.clone().into());
                        ctx.notify_error(win, "Connection", &e);
                    }
                }
            },
        );
    }

    /// Start a Wi-Fi password forget (D19, Settings): clears the
    /// PC-side stores; the board side needs the BOOT 10 s hold,
    /// which the success toast says.
    fn start_forget_wifi_password_job(&self, win: &MainWindow) {
        // R3-UI-02: same per-button busy state as the set job (see
        // above) — the old success message is stale the moment a
        // new attempt starts.
        win.set_wifi_password_busy(true);
        win.set_wifi_password_message("".into());
        self.wifi_password_message_for.set(None);
        self.run_generic_job(
            win,
            JobKind::SettingsOp,
            "Forgetting Wi-Fi password…",
            move |api| JobResult::Text(api.clear_wifi_password().map(|_| String::new())),
            move |ctx, win, result| {
                // R3-UI-02: the button un-busy on every outcome.
                win.set_wifi_password_busy(false);
                let JobResult::Text(result) = result else {
                    return;
                };
                match result {
                    Ok(_) => {
                        win.set_wifi_password_message(
                            "Wi-Fi password forgotten — the wheel's hotspot is open again. Hold BOOT 10 s on the wheel to clear its side too."
                                .into(),
                        );
                        // R3-UI-02: this message describes the
                        // post-forget state (`password_set` =
                        // false) — see the set job above.
                        ctx.wifi_password_message_for.set(Some(false));
                        ctx.sync_wifi_password_state(win);
                        ctx.notify_success(
                            win,
                            "Connection",
                            "Wi-Fi password forgotten",
                            "The wheel's hotspot is open again — hold BOOT 10 s on the wheel to clear its side too.",
                        );
                    }
                    Err(e) => ctx.notify_error(win, "Connection", &e),
                }
            },
        );
    }

    /// Start the wizard's Wi-Fi join (D16, Step 1 Connect): the
    /// card performs the join itself — scan for `RSH-*` networks,
    /// then join the first one (open, or with the stored WLAN
    /// profile when a password is set) and connect. Failure shows
    /// inline in the card AND toasts; success toasts.
    fn start_wifi_join_job(&self, win: &MainWindow) {
        if !self.is_live() {
            win.set_setup_wifi_error("RSH isn't running — reconnect to try again.".into());
            self.notify_toast(
                win,
                "Connection",
                "RSH isn't running",
                "Reconnect to RSH, then try again.",
            );
            return;
        }
        win.set_setup_wifi_joining(true);
        win.set_setup_wifi_error("".into());
        self.run_generic_job(
            win,
            JobKind::SettingsOp,
            "Joining wheel Wi-Fi…",
            move |api| {
                let ssids = api.scan_rsh_networks();
                let Some(ssid) = ssids.into_iter().next() else {
                    return JobResult::Text(Err(UserError {
                        code: "wifi_not_found".to_string(),
                        title: "Wheel Wi-Fi not found".to_string(),
                        what_happened: "No RSH-XXXX network was found nearby.".to_string(),
                        what_to_do:
                            "Make sure the wheel is on and its hotspot is up, then try again."
                                .to_string(),
                        detail: None,
                    }));
                };
                // A set password lives in the stored WLAN profile —
                // it cannot be read back, so the daemon reconnects
                // with it; an open AP is joined directly.
                let joined = if api.wifi_status().password_set {
                    api.connect(ApiTransport::Wifi).map(|_| ())
                } else {
                    match api.rejoin_wifi_open(&ssid) {
                        Ok(_) => api.connect(ApiTransport::Wifi).map(|_| ()),
                        Err(e) => Err(e),
                    }
                };
                JobResult::Text(joined.map(|()| String::new()))
            },
            move |ctx, win, result| {
                win.set_setup_wifi_joining(false);
                let JobResult::Text(result) = result else {
                    return;
                };
                match result {
                    Ok(_) => {
                        win.set_setup_wifi_error("".into());
                        ctx.notify_success(
                            win,
                            "Connection",
                            "Wheel Wi-Fi joined",
                            "Joined the wheel's hotspot — the wheel should appear shortly.",
                        );
                    }
                    Err(e) => {
                        win.set_setup_wifi_error(e.title.clone().into());
                        ctx.notify_error(win, "Connection", &e);
                    }
                }
            },
        );
    }

    /// Sync the Settings Wi-Fi password card from the cached
    /// `wifi_status()` (D19): a cache read on the UI thread —
    /// safe on the 1 Hz tick / slow resync.
    fn sync_wifi_password_state(&self, win: &MainWindow) {
        let status = self.api().wifi_status();
        win.set_wifi_password_set(status.password_set);
        // R3-UI-02: the success message describes the state at the
        // time it was set — when the state moves on, the message is
        // stale, so clear it. The message is kept when it already
        // describes the state the sync just landed on (e.g. the
        // daemon's status flips to `true` on the 1 Hz tick just
        // after a successful set — clearing there would erase the
        // message within a second of the action that wrote it).
        // `wifi_password_set_known` tracks the last synced state;
        // `wifi_password_message_for` tracks which state the
        // displayed message describes (`None` = no message).
        if self.wifi_password_set_known.get() != Some(status.password_set) {
            self.wifi_password_set_known.set(Some(status.password_set));
            if self.wifi_password_message_for.get() != Some(status.password_set) {
                win.set_wifi_password_message("".into());
                self.wifi_password_message_for.set(None);
            }
        }
        if let Some(ssid) = status.ssid {
            win.set_setup_wifi_ssid(ssid.into());
        }
    }

    /// Open the small confirm dialog (D17) for a destructive
    /// action — delete profile / remove game never run without the
    /// user confirming.
    fn open_confirm(
        &self,
        win: &MainWindow,
        title: &str,
        text: &str,
        confirm_label: &str,
        action: ConfirmAction,
    ) {
        *self.confirm_pending.borrow_mut() = Some(action);
        win.set_confirm_title(title.into());
        win.set_confirm_text(text.into());
        win.set_confirm_confirm_label(confirm_label.into());
        win.set_confirm_open(true);
    }

    /// Seed the Buttons page from the daemon's stored bindings (H-3 /
    /// B1 wiring): button 1–12 ← stored action, unbound ← the
    /// display default. Runs on (re)connect — at startup the sim has
    /// no bindings, so the defaults stand until the daemon answers.
    /// The long-press actions (BU2) seed the same way, from
    /// `list_long_press_bindings` ("Not set" = none).
    fn load_bindings(&self, win: &MainWindow) {
        let mut actions: Vec<String> = DEFAULT_BUTTON_ACTIONS
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        for binding in self.api().list_bindings() {
            let idx = usize::from(binding.button.saturating_sub(1));
            if idx < actions.len() {
                actions[idx] = action_display(&binding.action).to_string();
            }
        }
        (*self.button_actions.borrow_mut()).clone_from(&actions);
        win.set_button_actions(ModelRc::new(Rc::new(VecModel::from(
            actions
                .iter()
                .map(SharedString::from)
                .collect::<Vec<SharedString>>(),
        ))));
        let idx = usize::try_from(self.selected_button.get()).unwrap_or(0);
        if let Some(action) = actions.get(idx) {
            win.set_selected_action(action.clone().into());
        }
        let mut long_actions: Vec<String> = vec!["Not set".to_string(); 12];
        for binding in self.api().list_long_press_bindings() {
            let idx = usize::from(binding.button.saturating_sub(1));
            if idx < long_actions.len() {
                long_actions[idx] = action_display(&binding.action).to_string();
            }
        }
        (*self.long_press_actions.borrow_mut()).clone_from(&long_actions);
        if let Some(action) = long_actions.get(idx) {
            win.set_selected_long_action(action.clone().into());
        }
    }

    /// Seed the knob actions from the daemon's stored knobs (M9):
    /// action ids → display names, "" → "Not set". Runs on
    /// (re)connect alongside [`Ctx::load_bindings`].
    fn load_knobs(&self, win: &MainWindow) {
        let stored = self.api().knob_actions();
        let mut knobs: [[String; 3]; 2] = [
            [
                "Not set".to_string(),
                "Not set".to_string(),
                "Not set".to_string(),
            ],
            [
                "Not set".to_string(),
                "Not set".to_string(),
                "Not set".to_string(),
            ],
        ];
        for (k, rows) in stored.iter().enumerate() {
            for (r, action) in rows.iter().enumerate() {
                knobs[k][r] = action_display(action).to_string();
            }
        }
        let knob = usize::try_from(self.edit_knob.get()).unwrap_or(0);
        let row = usize::try_from(self.knob_row.get()).unwrap_or(0);
        let row_action = knobs[knob][row].clone();
        *self.knob_actions.borrow_mut() = knobs;
        win.set_knob_row_action(row_action.into());
    }

    /// Seed everything the daemon owns on (re)connect (B1 wiring):
    /// bindings + knobs + auto-pause + module on/off + the firmware
    /// check cache + the keep-in-tray switch (FC2: the switch is a
    /// `MainWindow` in-out prop now, seeded from the API like the
    /// others — the close handler reads the same value).
    fn seed_live_state(&self, win: &MainWindow) {
        self.load_bindings(win);
        self.load_knobs(win);
        win.set_auto_paused(self.api().auto_pause());
        self.sync_keep_in_tray(win);
        *self.module_states.borrow_mut() = [
            self.api().module_enabled("handbrake"),
            self.api().module_enabled("shifter"),
            self.api().module_enabled("horn"),
            self.api().module_enabled("led"),
        ];
        *self.firmware_check_cache.borrow_mut() = self.api().firmware_check().ok();
        self.raw_available.set(false);
    }

    /// Apply a board address from Settings (H4, FC2): the
    /// `address-accepted` callback carries the typed value. A
    /// private/loopback IPv4 literal is validated locally first
    /// (the daemon's rule — `rsh.local` is not an address), then
    /// persisted via the API (the transport uses it on the next
    /// reconnect). Any failure notifies plainly and the field
    /// reverts to the live address via [`Ctx::sync_overview`] —
    /// never a silent no-op, never a stale typed value left on
    /// screen.
    fn apply_board_address(&self, win: &MainWindow, address: &str) {
        let address = address.trim();
        if !valid_board_address(address) {
            self.notify(
                win,
                "Settings",
                "Address not saved",
                &format!(
                    "\"{address}\" is not a board address — enter the wheel's Wi-Fi IP (for example 192.168.4.1)."
                ),
            );
            self.sync_overview(win);
            return;
        }
        let address = address.to_string();
        self.run_generic_job(
            win,
            JobKind::SettingsOp,
            "Saving address…",
            move |api| JobResult::Text(api.set_board_address(&address)),
            move |ctx, win, result| {
                let JobResult::Text(result) = result else {
                    return;
                };
                match result {
                    Ok(stored) => {
                        ctx.notify(
                            win,
                            "Settings",
                            "Address saved",
                            &format!(
                                "Wheel address is now {stored} — it applies on the next reconnect."
                            ),
                        );
                        ctx.sync_overview(win);
                    }
                    Err(e) => {
                        ctx.notify_error(win, "Settings", &e);
                        ctx.sync_overview(win);
                    }
                }
            },
        );
    }

    /// Apply the keep-in-tray switch (M-7, FC2): the
    /// `keep-tray-selected` callback carries the NEW state (the
    /// .slint already flipped the prop). F6: the persist runs on a
    /// [`JobKind::SettingsOp`] job thread. Success confirms the
    /// authoritative value; failure reverts the prop to the old
    /// state and explains — the switch never lies about the real
    /// setting.
    fn apply_keep_tray(&self, win: &MainWindow, enabled: bool) {
        self.run_generic_job(
            win,
            JobKind::SettingsOp,
            "Saving…",
            move |api| JobResult::Bool(api.set_close_to_tray(enabled)),
            move |ctx, win, result| {
                let JobResult::Bool(result) = result else {
                    return;
                };
                match result {
                    Ok(new_state) => {
                        win.set_keep_in_tray(new_state);
                        ctx.notify(
                            win,
                            "Settings",
                            "Keep in tray",
                            if new_state {
                                "RSH keeps running in the tray after close."
                            } else {
                                "RSH quits when the window closes."
                            },
                        );
                    }
                    Err(e) => {
                        win.set_keep_in_tray(!enabled);
                        ctx.notify_error(win, "Settings", &e);
                    }
                }
            },
        );
    }

    /// Open the setup wizard at Step 0 (Connect, D16): the user
    /// joins the wheel's Wi-Fi or plugs in USB there — no Begin,
    /// no pairing. A reopen starts from a clean wizard slate (no
    /// stale connect error).
    fn open_setup_wizard(&self, win: &MainWindow) {
        self.clear_setup_marker();
        self.learn_target.set(0);
        win.set_setup_step(0);
        win.set_setup_centre_done(false);
        win.set_setup_pedals_done(false);
        win.set_setup_wifi_error("".into());
        win.set_setup_wifi_joining(false);
        win.set_setup_mode(true);
        self.sync_vjoy(win);
        self.sync_wifi_password_state(win);
        ui_log!("RSH-UI wizard opened at Connect step");
    }

    /// Persist the notification center after a mutation (M10) —
    /// coalesced (perf F-7): a notification storm used to rewrite
    /// the whole `notifications.json` on the UI thread per
    /// notification. Now this only marks a save due; the 1 Hz tick
    /// flushes at most one write per second, and `main` flushes
    /// once more at exit.
    fn persist_notifications(&self) {
        self.notif_save_pending.set(true);
    }

    /// Flush a due coalesced notification save (perf F-7): called
    /// on the 1 Hz tick and once at exit. No-op when nothing is
    /// pending, or — unless `force` — when the last save was under
    /// a second ago (the next tick retries, so bursts still cost at
    /// most one disk write per second).
    fn flush_pending_notification_save(&self, force: bool) {
        if !self.notif_save_pending.get() {
            return;
        }
        if !force {
            if let Some(last) = self.notif_last_save.get() {
                if last.elapsed() < Duration::from_secs(1) {
                    return;
                }
            }
        }
        self.notifications.borrow().save(self.data_dir.as_deref());
        self.notif_last_save.set(Some(Instant::now()));
        self.notif_save_pending.set(false);
    }

    /// Swap the shared API state to a live daemon connection (the
    /// Reconnect path): the daemon becomes the API and the steering
    /// sliders re-seed from the daemon's stored record. Caller
    /// re-syncs the pedal / extras / overview props afterwards.
    fn swap_to_live(&self, live_api: Arc<IpcApi>) {
        let api = live_api.clone() as Arc<dyn AppApi>;
        let stored = api.get_tuning("steering");
        *self.tuning.borrow_mut() = Tuning::from_settings(&stored);
        *self.steering.borrow_mut() = stored;
        self.unsaved
            .set(api.draft_status().is_some_and(|s| s.dirty));
        // F12: measure the down-episode this reconnect ends. A short
        // one is a flap — count it and arm the "Wheel connected"
        // suppression for `reconnect_to_live` (F5 worker: gate that
        // notify on `self.suppress_connected_notify.get()` — the flag
        // is set here and cleared on the next non-flap reconnect).
        // A long down-episode ends the burst: the count stays for
        // `mark_dead` to fold into the next down notification.
        let down = self.down_since.get().map(|t| t.elapsed());
        self.down_since.set(None);
        self.last_back_at.set(Some(Instant::now()));
        if is_flap_down_episode(down) {
            self.flap_count.set(self.flap_count.get().saturating_add(1));
            self.last_flap_at.set(Some(Instant::now()));
            self.suppress_connected_notify.set(true);
        } else {
            self.suppress_connected_notify.set(false);
        }
        // F3: a live link clears the respawn latch and the budget —
        // the daemon is back, whatever killed it is over.
        self.spawn_latch.borrow_mut().take();
        self.spawn_budget.record_success();
        let mut state = self.state.borrow_mut();
        state.api = api;
        state.live_api = Some(live_api.clone());
        state.live = true;
        state.connecting = false;
        if let Ok(mut link) = self.live_link.lock() {
            *link = Some(live_api);
        }
        drop(state);
        // A9: the probe resolved — the `connecting` clock stops.
        self.connecting_since.set(None);
    }

    /// Transport of the last live session (H1): Reconnect re-uses
    /// it instead of forcing Wi-Fi. Updated every frame from the
    /// live connection (see [`Ctx::push_frame`]).
    fn last_transport(&self) -> ApiTransport {
        self.state.borrow().last_transport
    }

    /// Mark the daemon dead (B2): clear the stale caches, drop the
    /// live link, show Disconnected + Reconnect. No-op when already
    /// down (the monitor only notifies on a transition, and the
    /// Reconnect button cannot kill a live link).
    ///
    /// F7 (v2.0.3): the dead `IpcApi` is dropped — `state.api` swaps
    /// back to a fresh `SimApi`, so the dead link's Tokio runtime +
    /// ~20 Hz poller die with the last `Arc` instead of spinning
    /// forever on a dead socket.
    fn mark_dead(&self, win: &MainWindow) {
        let old_api = {
            let mut state = self.state.borrow_mut();
            if !state.live && !state.connecting {
                return;
            }
            state.live = false;
            state.connecting = false;
            // F7: swap back to the sim — dropping the last `Arc` of
            // the dead `IpcApi` shuts its runtime + poller down.
            state.api = Arc::new(SimApi::new());
            let old = state.live_api.take();
            if let Ok(mut link) = self.live_link.lock() {
                *link = None;
            }
            old
        };
        // F12: the down-episode starts now.
        self.down_since.set(Some(Instant::now()));
        if let Some(api) = old_api {
            api.clear_cached_state();
        }
        self.steer_hist.borrow_mut().clear();
        self.pedal_hist.borrow_mut().clear();
        self.brake_hist.borrow_mut().clear();
        self.clutch_hist.borrow_mut().clear();
        win.set_daemon_live(false);
        // FC2: the fatal reason (if any) reaches the Overview
        // banner now, not just the notification drawer.
        self.sync_daemon_error(win);
        self.connecting_since.set(None);
        // F12: a death that follows a flapping burst — soon after a
        // reconnect that itself ended a flap — is part of the same
        // burst: stay silent instead of pushing one "not running"
        // entry per death. The burst's count is folded into the next
        // non-burst notification below.
        let burst_death = self.flap_count.get() > 0
            && self
                .last_back_at
                .get()
                .is_some_and(|t| t.elapsed() < FLAP_BURST_UPTIME);
        let mut flap_note = String::new();
        if !burst_death {
            let n = self.flap_count.get();
            let recent = self
                .last_flap_at
                .get()
                .is_some_and(|t| t.elapsed() < FLAP_COUNT_STALE);
            if n > 0 && recent {
                flap_note = format!(" Connection flapped ×{n} before this drop.");
            }
            self.flap_count.set(0);
        }
        // M-2: the daemon's fatal file names the reason
        // (IpcBindFailed / DaemonHung) — show it instead of the
        // generic "not running". The detail is the daemon's own plain
        // words; the code goes to the log only.
        if let Some((code, detail)) = live_probe::read_daemon_status(self.data_dir.as_deref()) {
            *self.last_dead_detail.borrow_mut() = Some(detail.clone());
            ui_log!("RSH-UI daemon fatal: {code}: {detail}");
            if !burst_death {
                self.notify_toast(
                    win,
                    "Connection",
                    "RSH stopped unexpectedly",
                    &format!("{detail}{flap_note}"),
                );
            }
        } else {
            *self.last_dead_detail.borrow_mut() = None;
            if !burst_death {
                self.notify_toast(
                    win,
                    "Connection",
                    "RSH not running",
                    &format!("Live data unavailable — RSH isn't running.{flap_note}"),
                );
            }
        }
        self.refresh(win, 0);
    }

    /// Swap a (re)connected daemon in (B2/H1): the shared path for
    /// the Reconnect button and the health monitor's re-probe.
    /// F5: the fetch runs on a background job — this entry only
    /// dedupes and starts it; [`Ctx::apply_reconnect_seed`] does
    /// the UI-thread apply once the seed arrives. A duplicate
    /// delivery of the link that is already live is a no-op.
    fn reconnect_to_live(&self, win: &MainWindow, live_api: Arc<IpcApi>) {
        {
            let state = self.state.borrow();
            if state.live {
                if let Some(current) = &state.live_api {
                    if Arc::ptr_eq(current, &live_api) {
                        return;
                    }
                }
            }
        }
        // A second seed fetch while one is in flight is ignored —
        // the in-flight one applies when it lands.
        if self
            .busy_jobs
            .borrow()
            .contains_key(&JobKind::ReconnectSeed)
        {
            return;
        }
        let transport = self.last_transport();
        self.run_job(win, JobKind::ReconnectSeed, "Reconnecting…", move || {
            let seed = crate::ipc_api::LiveSeed::fetch(&live_api, transport);
            JobDone::ReconnectSeed {
                api: live_api,
                seed,
            }
        });
    }

    /// Apply a finished reconnect seed (F5, UI thread): the old
    /// `reconnect_to_live` body, but every value comes from the
    /// seed or the slow cache — no IPC round-trip runs here.
    ///
    /// F27: when the window is hidden to tray (`hidden`), the
    /// state swap still runs but NO window property is touched.
    /// Defence-in-depth for the close-vs-seed race: gdb showed the
    /// actual `SIGSEGV` is the frame timer's per-tick refresh
    /// racing the hide (the fault is in the `llvmpipe-3` render
    /// thread ~0.2 s after the close, before the fetch completes
    /// — the seed apply itself was never the crash site), and the
    /// timer now writes nothing while hidden; this guard keeps the
    /// apply safe on any path that drains a job while hidden. The
    /// live state is already swapped in, and
    /// [`Ctx::resync_after_reopen`] re-syncs every prop from
    /// state/cache when the window comes back — nothing the user
    /// sees is lost, only deferred.
    fn apply_reconnect_seed(
        &self,
        win: &MainWindow,
        live_api: Arc<IpcApi>,
        seed: crate::ipc_api::LiveSeed,
    ) {
        if self.hidden.load(Ordering::SeqCst) {
            self.swap_to_live(live_api);
            ui_log!(
                "RSH-UI reconnect seed landed while hidden — state swapped, window apply skipped (F27)"
            );
            return;
        }
        self.swap_to_live(live_api);
        win.set_daemon_live(true);
        // FC2: live again — the daemon-error banner clears.
        self.sync_daemon_error(win);
        // Seed the controller + autostart switches from the daemon
        // (the startup seeding cannot run — the daemon was not
        // there when the window opened).
        if seed.backend == "HIDMaestro" {
            win.set_controller(0);
        } else {
            win.set_controller(1);
        }
        win.set_autostart_enabled(seed.autostart);
        // D19: the seed carries the fresh Wi-Fi state — sync the
        // Settings password card + the wizard's SSID from it (the
        // 1 Hz tick re-syncs from the cache afterwards).
        win.set_wifi_password_set(seed.wifi.password_set);
        if let Some(ssid) = &seed.wifi.ssid {
            win.set_setup_wifi_ssid(ssid.clone().into());
        }
        // B1 wiring: bindings + knobs + auto-pause + module on/off +
        // the firmware check cache all seed from the daemon now that
        // it is live (at startup the sim has none of these). All
        // cache reads now (F5).
        self.seed_live_state(win);
        // F12 (v2.0.3): a flap reconnect stays silent — the
        // "Wheel connected" notify is noise when the daemon died
        // and came back within seconds (`swap_to_live` armed
        // `suppress_connected_notify`). A non-flap reconnect that
        // ends a burst folds the count in ("flapped ×N before
        // stabilising") and resets it — the burst is reported
        // exactly once, here, not again by the next `mark_dead`.
        // A stale count (last flap > `FLAP_COUNT_STALE` ago) is
        // cleared without folding: it belongs to an old burst.
        // B1 (PC-test Bug 1): the notify is about the BOARD, not
        // the daemon link — this function runs whenever the
        // UI↔daemon feed resumes, so the old code toasted
        // "RSH connected / Live data is back" while the board was
        // offline (the Overview already showed "Wheel not found").
        // Gate on the seed's session `connect` verdict: only an
        // Ok seed means the wheel is really connected. An Err seed
        // stays silent here — F26 already reports non-Unreachable
        // errors below, and the Overview banner covers
        // Unreachable. The flap count is left alone when gated
        // off: it describes daemon-link flaps and is folded into
        // the next notify that actually fires (if still recent).
        // The title says "Wheel connected" (the board), never
        // "RSH connected" (the daemon link) — the two must not
        // read as the same thing.
        if seed.connect.is_ok() && !self.suppress_connected_notify.get() {
            let n = self.flap_count.get();
            let recent = self
                .last_flap_at
                .get()
                .is_some_and(|t| t.elapsed() < FLAP_COUNT_STALE);
            let body = if n > 0 && recent {
                format!("Live data is back — the connection flapped ×{n} before stabilising.")
            } else {
                "Live data is back.".to_string()
            };
            self.notify_success(win, "Connection", "Wheel connected", &body);
            self.flap_count.set(0);
        }
        // The session `connect` already ran on the seed job's
        // thread — report its verdict here.
        if let Err(e) = seed.connect {
            // F26: `Unreachable` (no board) is the expected
            // disconnected state, not a failure to report — the
            // Overview already shows it, and notifying on every
            // no-board (re)connect is spam. Anything else (a real
            // transport failure, `Unknown`, …) still notifies.
            if e.code != ConnectionError::Unreachable.code() {
                self.notify_error(win, "Connection", &e);
            }
            ui_log!("RSH-UI reconnect failed: {} ({})", e.code, e.title);
        }
        self.refresh_calibration();
        self.sync_pedals(win);
        self.sync_extras(win);
        self.sync_overview(win);
        self.refresh_profiles(win);
        self.refresh_games(win);
        self.sync_diag(win);
        self.refresh(win, 0);
    }

    /// Re-sync the window after it comes back from tray-hide
    /// (F27): a seed that landed while hidden swapped the live
    /// state but skipped every window write, so the props are
    /// stale until something re-derives them. The live flag +
    /// the cache-seeded switches (controller, autostart, bindings,
    /// knobs, firmware cache) are re-applied here, the
    /// notification model is re-synced (entries pushed while
    /// hidden were kept in the center + on disk, never drawn),
    /// then a full refresh re-derives the rest from state/cache.
    /// Called from [`apply_page_request`] only when the window
    /// actually was hidden — a page forward while visible needs
    /// no re-sync (the frame timer already re-derives every tick).
    fn resync_after_reopen(&self, win: &MainWindow) {
        if self.is_live() {
            win.set_daemon_live(true);
            self.seed_live_state(win);
        }
        sync_to_window(&self.notifications.borrow(), win);
        self.refresh(win, 0);
    }

    /// Apply one pedal-tuning edit (deadzone / bite / feel curve) to
    /// the named module: read the stored record, patch it, push it.
    /// F6: the push runs on a [`JobKind::TuningPush`] job thread —
    /// the UI never blocks on `set_tuning`. The record is patched
    /// here (the read is a cache read, cheap); only the owned
    /// record crosses into the job. Success marks the profile
    /// unsaved and re-syncs the pedal page; failure shows an error
    /// notification (never silent).
    fn set_pedal_tuning(
        &self,
        win: &MainWindow,
        module: &'static str,
        update: impl FnOnce(&mut TuningSettings),
    ) {
        let mut record = self.api().get_tuning(module);
        update(&mut record);
        self.run_generic_job(
            win,
            JobKind::TuningPush,
            "Saving pedal tuning…",
            move |api| JobResult::Tuning(api.set_tuning(&record).map(Box::new)),
            move |ctx, win, result| {
                let JobResult::Tuning(result) = result else {
                    return;
                };
                match result {
                    Ok(stored) => {
                        ctx.unsaved.set(true);
                        // R2-10: re-sync from the `set_tuning`
                        // return value — the authoritative stored
                        // record — not the ~1 Hz cache, which may
                        // still hold the pre-push values.
                        ctx.apply_tuning_record(win, &stored);
                    }
                    Err(e) => {
                        ctx.notify(win, "Pedals", "Couldn't save pedal tuning", &e.title);
                        ui_log!(
                            "RSH-UI pedal set_tuning {module} failed: {} ({})",
                            e.code,
                            e.title
                        );
                    }
                }
            },
        );
    }

    /// Apply one tuning edit to a non-pedal module (handbrake / LED /
    /// shifter / horn): same push as [`Ctx::set_pedal_tuning`], then
    /// re-sync the Extras page.
    fn set_module_tuning(
        &self,
        win: &MainWindow,
        module: &'static str,
        update: impl FnOnce(&mut TuningSettings),
    ) {
        let mut record = self.api().get_tuning(module);
        update(&mut record);
        self.run_generic_job(
            win,
            JobKind::TuningPush,
            "Saving setting…",
            move |api| JobResult::Tuning(api.set_tuning(&record).map(Box::new)),
            move |ctx, win, result| {
                let JobResult::Tuning(result) = result else {
                    return;
                };
                match result {
                    Ok(stored) => {
                        ctx.unsaved.set(true);
                        // R2-10: re-sync from the `set_tuning`
                        // return value — the authoritative stored
                        // record — not the ~1 Hz cache, which may
                        // still hold the pre-push values.
                        ctx.apply_tuning_record(win, &stored);
                    }
                    Err(e) => {
                        ctx.notify(win, "Extras", "Couldn't save setting", &e.title);
                        ui_log!(
                            "RSH-UI extras set_tuning {module} failed: {} ({})",
                            e.code,
                            e.title
                        );
                    }
                }
            },
        );
    }

    /// True while a field push is pending for `module` (R2-10):
    /// the drag's local value owns the slider until the push
    /// completes — a ~1 Hz re-sync must not overwrite it from the
    /// stale slow cache.
    fn field_push_pending_for(&self, module: &str) -> bool {
        self.pending_field
            .borrow()
            .as_ref()
            .is_some_and(|(p, _)| p.module == module)
    }

    /// Local stored record for `module` (R2-10): the copy the drag
    /// handlers edit and flush completions re-seed from the
    /// `set_tuning` return value — the base a debounced push builds
    /// on, instead of the ~1 Hz slow cache. Modules with no local
    /// copy (shifter / horn / steering) fall back to the slow cache.
    fn local_tuning_record(&self, module: &str) -> TuningSettings {
        match module {
            // --- Pedal ramps/curves per-module tuning ---
            // Both throttle spellings read pedal index 0 (the
            // daemon canonicalizes to "accelerator", the sim echoes
            // "throttle").
            "accelerator" | "throttle" => self.pedals.borrow().records[0].clone(),
            "brake" => self.pedals.borrow().records[1].clone(),
            "clutch" => self.pedals.borrow().records[2].clone(),
            "handbrake" => self.handbrake_record.borrow().clone(),
            "led" => self.led_record.borrow().clone(),
            "horn" => self.horn_record.borrow().clone(),
            _ => self.api().get_tuning(module),
        }
    }

    /// Re-sync one module's page from a stored record (R2-10): the
    /// `set_tuning` return value is authoritative — the page is
    /// written from it (and the local record updated), never from
    /// the ~1 Hz cache, which may still hold pre-push values.
    #[allow(clippy::cast_possible_truncation)]
    fn apply_tuning_record(&self, win: &MainWindow, stored: &TuningSettings) {
        match stored.module.as_str() {
            // --- Pedal ramps/curves per-module tuning ---
            // Both throttle spellings: the daemon canonicalizes to
            // "accelerator", the sim echoes "throttle" — either way
            // the record belongs to pedal index 0.
            "accelerator" | "throttle" | "brake" | "clutch" => {
                let index = match stored.module.as_str() {
                    "accelerator" | "throttle" => 0,
                    "brake" => 1,
                    _ => 2,
                };
                // R2-10: a drag whose push is still pending owns its
                // field — a completion for another field must not
                // snap it back to the pre-drag stored value. The
                // local record holds the drag value, so the
                // effective record carries it over the stored one
                // for exactly that field (one pending field at a
                // time). Without this, dragging saturation and then
                // typing a ramp value would snap saturation back.
                let effective = {
                    let mut effective = stored.clone();
                    if let Some((pending, _)) = *self.pending_field.borrow() {
                        // Both throttle spellings are the same pedal
                        // (the daemon canonicalizes to "accelerator",
                        // the sim echoes "throttle").
                        let same_module = pending.module == stored.module.as_str()
                            || pending.module == "throttle" && stored.module == "accelerator";
                        if same_module {
                            let local = self.local_tuning_record(&stored.module);
                            match pending.kind {
                                FieldKind::Deadzone => effective.deadzone = local.deadzone,
                                FieldKind::Bite => effective.bite_point = local.bite_point,
                                // --- Brake per-module tuning ---
                                FieldKind::Force => effective.saturation = local.saturation,
                                FieldKind::Brightness => {
                                    effective.led_brightness = local.led_brightness;
                                }
                                // --- Pedal ramps/curves per-module tuning ---
                                FieldKind::PressMs => effective.press_ms = local.press_ms,
                                FieldKind::ReleaseMs => {
                                    effective.release_ms = local.release_ms;
                                }
                                FieldKind::Curve => effective.curve = local.curve,
                                FieldKind::Saturation => effective.max_mv = local.max_mv,
                                // --- Encoder per-module tuning ---
                                // An encoder drag can never reach this
                                // pedal arm (`same_module` is false when
                                // the pending module is "encoders"), but
                                // the match must cover the new variants —
                                // same carry-over as the pedal fields.
                                FieldKind::EncoderPulseMs => {
                                    effective.encoder_pulse_ms = local.encoder_pulse_ms;
                                }
                                FieldKind::EncoderAccelSens => {
                                    effective.encoder_accel_sensitivity =
                                        local.encoder_accel_sensitivity;
                                }
                                // --- Buttons per-module tuning ---
                                // Same as the encoder fields above: a
                                // buttons drag never reaches the pedal
                                // arm, but the match must cover them.
                                FieldKind::ButtonDebounceMs => {
                                    effective.button_debounce_ms = local.button_debounce_ms;
                                }
                                FieldKind::ButtonLongPressMs => {
                                    effective.button_long_press_ms = local.button_long_press_ms;
                                }
                                FieldKind::ButtonDoublePressMs => {
                                    effective.button_double_press_ms = local.button_double_press_ms;
                                }
                                // --- Shifter per-module tuning ---
                                // A shifter drag can never reach this
                                // pedal arm (`same_module` is false
                                // when the pending module is
                                // "shifter"), but the match must cover
                                // the new variant — same carry-over
                                // as the pedal fields.
                                FieldKind::ShiftLockoutMs => {
                                    effective.shift_lockout_ms = local.shift_lockout_ms;
                                }
                                // --- Handbrake/clutch per-module tuning ---
                                FieldKind::BiteWidth => {
                                    effective.bite_width = local.bite_width;
                                }
                                // --- LED/horn per-module tuning ---
                                // Same: an LED/horn drag never reaches
                                // this pedal arm, but the match must
                                // cover the new variants.
                                FieldKind::LedPeriodMs => {
                                    effective.led_period_ms = local.led_period_ms;
                                }
                                FieldKind::HornMinMs => {
                                    effective.horn_min_ms = local.horn_min_ms;
                                }
                                FieldKind::LedColor => {
                                    effective.led_color = local.led_color;
                                }
                            }
                        }
                    }
                    effective
                };
                let feel_exp = {
                    let mut pedals = self.pedals.borrow_mut();
                    pedals.records[index] = effective.clone();
                    pedals.feel_exp[index] = effective.curve as f32;
                    pedals.feel_exp[index]
                };
                Self::sync_pedal_module_props(win, index, &effective, feel_exp);
                // --- Pedal ramps/curves per-module tuning ---
                // Throttle only: the saturation slider and the
                // combined-pedals switch follow the effective
                // record (a pending saturation drag included).
                if index == 0 {
                    self.sync_throttle_saturation_props(win, &effective);
                    set_if_changed(
                        win.get_combine_pedals(),
                        effective.combine_pedals.unwrap_or(false),
                        |v| win.set_combine_pedals(v),
                    );
                }
            }
            "handbrake" => {
                *self.handbrake_record.borrow_mut() = stored.clone();
                Self::sync_extras_handbrake_props(win, stored);
            }
            "led" => {
                *self.led_record.borrow_mut() = stored.clone();
                self.sync_extras_led_props(win, stored);
            }
            "horn" => {
                *self.horn_record.borrow_mut() = stored.clone();
                self.sync_extras_horn_props(win, stored);
            }
            "encoders" => {
                Self::sync_encoder_props(win, stored);
            }
            // --- Buttons per-module tuning ---
            "buttons" => {
                Self::sync_button_props(win, stored);
            }
            // Shifter has no per-module writer — the slow re-sync
            // owns its props (as before).
            _ => self.sync_extras(win),
        }
    }

    /// Revert one timed-out tuning push (R2-18): snap exactly that
    /// slider back to its pre-push stored record and clear its
    /// pending flag — only when it is still the pending one. A push
    /// superseded by a newer drag returns `false` (the newer drag
    /// owns the slider now) and reverts nothing. Returns `true`
    /// when a revert happened (the caller then tells the user the
    /// change was put back).
    fn revert_tuning_push(&self, win: &MainWindow, target: &TuningPushTarget) -> bool {
        match target {
            TuningPushTarget::Steering { due, base } => {
                if self.steer_push_due.get() != Some(*due) {
                    return false;
                }
                self.steer_push_due.set(None);
                *self.tuning.borrow_mut() = Tuning::from_settings(base);
                self.refresh(win, 0);
                true
            }
            TuningPushTarget::Field { pending, due, base } => {
                let current = *self.pending_field.borrow();
                if !current.is_some_and(|(p, d)| p == *pending && d == *due) {
                    return false;
                }
                *self.pending_field.borrow_mut() = None;
                self.apply_tuning_record(win, base);
                true
            }
        }
    }

    /// Write one pedal card's tuning props from a record (R2-10):
    /// feel, deadzone slider + text, mini-curve, clutch bite, and
    /// (brake) the bite + brake-force sliders. Shared by
    /// `sync_pedals` (slow re-sync), the drag handlers (local echo
    /// while dragging) and flush completions (the authoritative
    /// `set_tuning` return value). All writes are change-guarded,
    /// like `push_frame`.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn sync_pedal_module_props(
        win: &MainWindow,
        index: usize,
        record: &TuningSettings,
        feel_exp: f32,
    ) {
        let feel = PedalCache::feel_index(feel_exp);
        let curve = pedal_curve_commands(record.deadzone as f32, FEEL_EXPONENTS[feel as usize]);
        match index {
            0 => {
                set_if_changed(win.get_throttle_feel(), feel, |v| win.set_throttle_feel(v));
                set_if_changed(
                    win.get_throttle_deadzone(),
                    (record.deadzone / 10.0) as f32,
                    |v| win.set_throttle_deadzone(v),
                );
                set_if_changed(
                    win.get_throttle_deadzone_text(),
                    pct_text(record.deadzone).into(),
                    |v| win.set_throttle_deadzone_text(v),
                );
                set_if_changed(win.get_throttle_curve(), curve.into(), |v| {
                    win.set_throttle_curve(v);
                });
                set_if_changed(win.get_throttle_dashed(), "M 6 104 L 104 6".into(), |v| {
                    win.set_throttle_dashed(v);
                });
            }
            1 => {
                set_if_changed(win.get_brake_feel(), feel, |v| win.set_brake_feel(v));
                set_if_changed(
                    win.get_brake_deadzone(),
                    (record.deadzone / 10.0) as f32,
                    |v| win.set_brake_deadzone(v),
                );
                set_if_changed(
                    win.get_brake_deadzone_text(),
                    pct_text(record.deadzone).into(),
                    |v| win.set_brake_deadzone_text(v),
                );
                // --- Brake per-module tuning ---
                // B2 bite: the slider is 0..1 of 50%, so the stored
                // 0..0.5 bite is shown doubled; the text is the real
                // percent. B1 force: the slider value IS the
                // saturation fraction (0.5..1.0 = 50–100%).
                let brake_bite = record.bite_point.unwrap_or(0.0);
                set_if_changed(win.get_brake_bite(), (brake_bite / 0.5) as f32, |v| {
                    win.set_brake_bite(v);
                });
                set_if_changed(
                    win.get_brake_bite_text(),
                    pct_text(brake_bite * 100.0).into(),
                    |v| win.set_brake_bite_text(v),
                );
                let brake_force = record.saturation.unwrap_or(1.0);
                set_if_changed(win.get_brake_force(), brake_force as f32, |v| {
                    win.set_brake_force(v);
                });
                set_if_changed(
                    win.get_brake_force_text(),
                    pct_text(brake_force * 100.0).into(),
                    |v| win.set_brake_force_text(v),
                );
                set_if_changed(win.get_brake_curve(), curve.into(), |v| {
                    win.set_brake_curve(v);
                });
                set_if_changed(win.get_brake_dashed(), "M 6 104 L 104 6".into(), |v| {
                    win.set_brake_dashed(v);
                });
            }
            _ => {
                set_if_changed(win.get_clutch_feel(), feel, |v| win.set_clutch_feel(v));
                set_if_changed(
                    win.get_clutch_deadzone(),
                    (record.deadzone / 10.0) as f32,
                    |v| win.set_clutch_deadzone(v),
                );
                set_if_changed(
                    win.get_clutch_deadzone_text(),
                    pct_text(record.deadzone).into(),
                    |v| win.set_clutch_deadzone_text(v),
                );
                let bite = record.bite_point.unwrap_or(0.0);
                set_if_changed(win.get_clutch_bite(), bite as f32, |v| {
                    win.set_clutch_bite(v);
                });
                set_if_changed(
                    win.get_clutch_bite_text(),
                    pct_text(bite * 100.0).into(),
                    |v| win.set_clutch_bite_text(v),
                );
                // --- Handbrake/clutch per-module tuning ---
                // C2 slip zone: the slider is 0..1 of 5..40%, so
                // the stored 0.05..0.40 fraction is shown as
                // (width - 0.05) / 0.35; the text is the real percent.
                let bite_width = record.bite_width.unwrap_or(0.15);
                set_if_changed(
                    win.get_clutch_bite_width(),
                    ((bite_width - 0.05) / 0.35) as f32,
                    |v| win.set_clutch_bite_width(v),
                );
                set_if_changed(
                    win.get_clutch_bite_width_text(),
                    pct_text(bite_width * 100.0).into(),
                    |v| win.set_clutch_bite_width_text(v),
                );
                set_if_changed(win.get_clutch_curve(), curve.into(), |v| {
                    win.set_clutch_curve(v);
                });
                set_if_changed(win.get_clutch_dashed(), "".into(), |v| {
                    win.set_clutch_dashed(v);
                });
            }
        }
        // --- Pedal ramps/curves per-module tuning ---
        Self::sync_pedal_ramp_curve_props(win, index, record);
    }

    /// Write one pedal card's Advanced ramp/curve slider props from
    /// a record (Workstream C): the own curve slider (continuous —
    /// not just the Feel chips), tip-in / lift-off ramps (throttle,
    /// defaults 100/50 ms), initial bite (brake, default 50 ms),
    /// clutch release (default 300 ms). Split out of
    /// [`Self::sync_pedal_module_props`] so that function stays
    /// under the 100-line clippy limit.
    fn sync_pedal_ramp_curve_props(win: &MainWindow, index: usize, record: &TuningSettings) {
        let (frac, text) = curve_frac_text(record.curve);
        match index {
            0 => {
                set_if_changed(win.get_throttle_curve_value(), frac, |v| {
                    win.set_throttle_curve_value(v);
                });
                set_if_changed(win.get_throttle_curve_text(), text.into(), |v| {
                    win.set_throttle_curve_text(v);
                });
                let (frac, text) = ramp_frac_text(record.press_ms.unwrap_or(100.0), 500.0);
                set_if_changed(win.get_throttle_press_ms_value(), frac, |v| {
                    win.set_throttle_press_ms_value(v);
                });
                set_if_changed(win.get_throttle_press_ms_text(), text.into(), |v| {
                    win.set_throttle_press_ms_text(v);
                });
                let (frac, text) = ramp_frac_text(record.release_ms.unwrap_or(50.0), 500.0);
                set_if_changed(win.get_throttle_release_ms_value(), frac, |v| {
                    win.set_throttle_release_ms_value(v);
                });
                set_if_changed(win.get_throttle_release_ms_text(), text.into(), |v| {
                    win.set_throttle_release_ms_text(v);
                });
            }
            1 => {
                set_if_changed(win.get_brake_curve_value(), frac, |v| {
                    win.set_brake_curve_value(v);
                });
                set_if_changed(win.get_brake_curve_text(), text.into(), |v| {
                    win.set_brake_curve_text(v);
                });
                let (frac, text) = ramp_frac_text(record.press_ms.unwrap_or(50.0), 300.0);
                set_if_changed(win.get_brake_press_ms_value(), frac, |v| {
                    win.set_brake_press_ms_value(v);
                });
                set_if_changed(win.get_brake_press_ms_text(), text.into(), |v| {
                    win.set_brake_press_ms_text(v);
                });
            }
            _ => {
                set_if_changed(win.get_clutch_curve_value(), frac, |v| {
                    win.set_clutch_curve_value(v);
                });
                set_if_changed(win.get_clutch_curve_text(), text.into(), |v| {
                    win.set_clutch_curve_text(v);
                });
                let (frac, text) = ramp_frac_text(record.release_ms.unwrap_or(300.0), 1000.0);
                set_if_changed(win.get_clutch_release_ms_value(), frac, |v| {
                    win.set_clutch_release_ms_value(v);
                });
                set_if_changed(win.get_clutch_release_ms_text(), text.into(), |v| {
                    win.set_clutch_release_ms_text(v);
                });
            }
        }
    }

    /// Echo a pedal deadzone drag locally (R2-10, Steering pattern):
    /// the slider + text + mini-curve move with the mouse, and the
    /// local record follows — the thumb tracks the drag and the
    /// ~1 Hz re-sync agrees instead of snapping it back. Only the
    /// daemon push waits out the 150 ms debounce.
    fn apply_pedal_deadzone_local(&self, win: &MainWindow, index: usize, deadzone: f64) {
        let (record, feel_exp) = {
            let mut pedals = self.pedals.borrow_mut();
            pedals.records[index].deadzone = deadzone;
            (pedals.records[index].clone(), pedals.feel_exp[index])
        };
        Self::sync_pedal_module_props(win, index, &record, feel_exp);
    }

    /// Echo a clutch / brake bite-point drag locally (R2-10): same
    /// pattern as [`Self::apply_pedal_deadzone_local`]. `index` is
    /// the pedal index (1 brake, 2 clutch).
    fn apply_pedal_bite_local(&self, win: &MainWindow, index: usize, bite: f64) {
        let (record, feel_exp) = {
            let mut pedals = self.pedals.borrow_mut();
            pedals.records[index].bite_point = Some(bite);
            (pedals.records[index].clone(), pedals.feel_exp[index])
        };
        Self::sync_pedal_module_props(win, index, &record, feel_exp);
    }

    /// Echo a clutch slip-zone drag locally (R2-10, C2): the same
    /// pattern as the bite echo — the slider + text move with the
    /// mouse, the local record follows, and only the daemon push
    /// waits out the 150 ms debounce.
    fn apply_pedal_bite_width_local(&self, win: &MainWindow, width: f64) {
        let (record, feel_exp) = {
            let mut pedals = self.pedals.borrow_mut();
            pedals.records[2].bite_width = Some(width);
            (pedals.records[2].clone(), pedals.feel_exp[2])
        };
        Self::sync_pedal_module_props(win, 2, &record, feel_exp);
    }

    /// Echo a brake-force drag locally (R2-10, B1): same pattern —
    /// the slider + text move with the mouse, the local record
    /// follows, and only the daemon push waits out the 150 ms
    /// debounce.
    fn apply_pedal_brake_force_local(&self, win: &MainWindow, saturation: f64) {
        let (record, feel_exp) = {
            let mut pedals = self.pedals.borrow_mut();
            pedals.records[1].saturation = Some(saturation);
            (pedals.records[1].clone(), pedals.feel_exp[1])
        };
        Self::sync_pedal_module_props(win, 1, &record, feel_exp);
    }

    // --- Pedal ramps/curves per-module tuning ---
    /// Throttle saturation base: the calibrated fully-pressed mV
    /// when a capture exists, else full scale (3300 mV). Saturation
    /// is a percent of the pedal's own travel (cal max − min),
    /// never of the raw ADC scale — so a pot that only reaches
    /// 3100 mV still reads 100% at full press.
    fn throttle_saturation_base_max(&self) -> f64 {
        self.calibration
            .borrow()
            .as_ref()
            .and_then(|c| c.pedal_max_mv.get("accel").copied())
            .unwrap_or(3300.0)
    }

    /// Saturation percent (0..100) for a throttle record, from its
    /// effective max against the calibrated base.
    fn throttle_saturation_pct(&self, record: &TuningSettings) -> f64 {
        let base = self.throttle_saturation_base_max();
        let span = (base - record.min_mv).max(1.0);
        ((record.max_mv - record.min_mv) / span * 100.0).clamp(0.0, 100.0)
    }

    /// Effective max in mV for a saturation percent, clamped so the
    /// endpoint span stays ≥ 660 mV (the tuning contract) — a high
    /// released endpoint can otherwise make 70% unrepresentable.
    fn throttle_saturation_max_mv(&self, record: &TuningSettings, pct: f64) -> f64 {
        let base = self.throttle_saturation_base_max();
        let full_span = (base - record.min_mv).max(1.0);
        let min_pct = 660.0 / full_span * 100.0;
        let pct = pct.clamp(min_pct, 100.0);
        record.min_mv + pct / 100.0 * full_span
    }

    /// Write the throttle saturation slider + text from a record
    /// (frac is 0..1 over 70..100%).
    #[allow(clippy::cast_possible_truncation)]
    fn sync_throttle_saturation_props(&self, win: &MainWindow, record: &TuningSettings) {
        let pct = self.throttle_saturation_pct(record);
        set_if_changed(
            win.get_throttle_saturation_value(),
            ((pct - 70.0) / 30.0).clamp(0.0, 1.0) as f32,
            |v| win.set_throttle_saturation_value(v),
        );
        set_if_changed(
            win.get_throttle_saturation_text(),
            pct_text(pct).into(),
            |v| win.set_throttle_saturation_text(v),
        );
    }

    /// Echo a pedal curve drag locally (R2-10): the slider + text +
    /// mini-curve move with the mouse; the Feel chip follows the
    /// curve (soft < 0.95, sharp > 1.05, else linear).
    #[allow(clippy::cast_possible_truncation)]
    fn apply_pedal_curve_local(&self, win: &MainWindow, index: usize, exponent: f64) {
        let (record, feel_exp) = {
            let mut pedals = self.pedals.borrow_mut();
            pedals.records[index].curve = exponent;
            pedals.feel_exp[index] = exponent as f32;
            (pedals.records[index].clone(), pedals.feel_exp[index])
        };
        Self::sync_pedal_module_props(win, index, &record, feel_exp);
    }

    /// Echo a pedal press-ramp drag locally (R2-10): same pattern.
    fn apply_pedal_press_ms_local(&self, win: &MainWindow, index: usize, ms: f64) {
        let (record, feel_exp) = {
            let mut pedals = self.pedals.borrow_mut();
            pedals.records[index].press_ms = Some(ms);
            (pedals.records[index].clone(), pedals.feel_exp[index])
        };
        Self::sync_pedal_module_props(win, index, &record, feel_exp);
    }

    /// Echo a pedal release-ramp drag locally (R2-10): same pattern.
    fn apply_pedal_release_ms_local(&self, win: &MainWindow, index: usize, ms: f64) {
        let (record, feel_exp) = {
            let mut pedals = self.pedals.borrow_mut();
            pedals.records[index].release_ms = Some(ms);
            (pedals.records[index].clone(), pedals.feel_exp[index])
        };
        Self::sync_pedal_module_props(win, index, &record, feel_exp);
    }

    /// Echo a throttle saturation drag locally (R2-10): the slider +
    /// text move with the mouse; the effective max follows in the
    /// local record (the "Pedal down" mV re-syncs from calibration
    /// on the next ~1 Hz cycle when a capture exists).
    fn apply_throttle_saturation_local(&self, win: &MainWindow, max_mv: f64) {
        let record = {
            let mut pedals = self.pedals.borrow_mut();
            pedals.records[0].max_mv = max_mv;
            pedals.records[0].clone()
        };
        self.sync_throttle_saturation_props(win, &record);
    }

    /// Write the handbrake card's tuning props from a record
    /// (R2-10): mV, feel, deadzone slider + text, mini-curve, mode.
    /// The live marker is owned by `push_frame` and never written
    /// here — it used to be pinned to rest here on every re-sync.
    /// All writes are change-guarded, like `push_frame`.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn sync_extras_handbrake_props(win: &MainWindow, hb: &TuningSettings) {
        set_if_changed(win.get_handbrake_up_mv(), mv_text(hb.min_mv).into(), |v| {
            win.set_handbrake_up_mv(v);
        });
        set_if_changed(
            win.get_handbrake_down_mv(),
            mv_text(hb.max_mv).into(),
            |v| win.set_handbrake_down_mv(v),
        );
        let feel = PedalCache::feel_index(hb.curve as f32);
        set_if_changed(win.get_handbrake_feel(), feel, |v| {
            win.set_handbrake_feel(v);
        });
        set_if_changed(
            win.get_handbrake_deadzone(),
            (hb.deadzone / 10.0) as f32,
            |v| win.set_handbrake_deadzone(v),
        );
        set_if_changed(
            win.get_handbrake_deadzone_text(),
            pct_text(hb.deadzone).into(),
            |v| win.set_handbrake_deadzone_text(v),
        );
        set_if_changed(
            win.get_handbrake_curve(),
            pedal_curve_commands(hb.deadzone as f32, FEEL_EXPONENTS[feel as usize]).into(),
            |v| win.set_handbrake_curve(v),
        );
        set_if_changed(win.get_handbrake_dashed(), "M 6 104 L 104 6".into(), |v| {
            win.set_handbrake_dashed(v);
        });
        // H8: the stored handbrake mode (Axis 0 / Button 1) — never
        // a hardcoded 0.
        set_if_changed(
            win.get_handbrake_mode(),
            i32::from(hb.handbrake_button_mode.unwrap_or(false)),
            |v| win.set_handbrake_mode(v),
        );
        // --- Handbrake/clutch per-module tuning ---
        // H1 latch + H3 auto-release (both off by default); H2 grab
        // speed is the handbrake press ramp (0..200 ms, 0 =
        // instant), shown as a 0..1 fraction + "120 ms" text.
        set_if_changed(
            win.get_handbrake_latch(),
            hb.handbrake_latch.unwrap_or(false),
            |v| win.set_handbrake_latch(v),
        );
        let (grab_frac, grab_text) = ramp_frac_text(hb.press_ms.unwrap_or(0.0), 200.0);
        set_if_changed(win.get_handbrake_grab_ms(), grab_frac, |v| {
            win.set_handbrake_grab_ms(v);
        });
        set_if_changed(win.get_handbrake_grab_ms_text(), grab_text.into(), |v| {
            win.set_handbrake_grab_ms_text(v);
        });
        set_if_changed(
            win.get_handbrake_auto_release(),
            hb.handbrake_auto_release.unwrap_or(false),
            |v| win.set_handbrake_auto_release(v),
        );
    }

    /// Write the LED card's tuning props from a record (R2-10 /
    /// R2-11): pattern, brightness slider + text, colour, preview.
    /// The preview model is only rebuilt when its (pattern,
    /// brightness, colour) key moved. All writes are
    /// change-guarded, like `push_frame`.
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    fn sync_extras_led_props(&self, win: &MainWindow, led: &TuningSettings) {
        let pattern = led.led_pattern.unwrap_or(LedPattern::Solid);
        let pattern_i = match pattern {
            LedPattern::Off => 0,
            LedPattern::Solid => 1,
            LedPattern::Blink => 2,
            LedPattern::Chase => 3,
            LedPattern::Rainbow => 4,
        };
        set_if_changed(win.get_led_pattern(), pattern_i, |v| {
            win.set_led_pattern(v);
        });
        let brightness = led.led_brightness.unwrap_or(80);
        set_if_changed(
            win.get_led_brightness(),
            f32::from(brightness) / 100.0,
            |v| win.set_led_brightness(v),
        );
        set_if_changed(
            win.get_led_brightness_text(),
            pct_text(f64::from(brightness)).into(),
            |v| win.set_led_brightness_text(v),
        );
        // M2: the stored colour drives the swatch + the preview —
        // never always-blue. A custom (non-palette) colour selects
        // no swatch (-1) instead of highlighting a wrong one.
        let color_index = led
            .led_color
            .and_then(|c| LED_PALETTE.iter().position(|p| *p == c))
            .map_or(-1, |i| i32::try_from(i).unwrap_or(-1));
        set_if_changed(win.get_led_color(), color_index, |v| {
            win.set_led_color(v);
        });
        // L1: the pattern-cycle period slider + text (100–5000 ms).
        let period_ms = led.led_period_ms.unwrap_or(1000.0);
        set_if_changed(win.get_led_period_ms(), period_ms as f32, |v| {
            win.set_led_period_ms(v);
        });
        set_if_changed(
            win.get_led_period_text(),
            format!("{period_ms:.0} ms").into(),
            |v| win.set_led_period_text(v),
        );
        // L2: the custom-colour sliders + hex text follow the stored
        // colour (palette or custom — the preview already does); no
        // colour stored yet = the accent blue.
        let rgb = led.led_color.unwrap_or(0x2F_6B_FF);
        let [_, r, g, bl] = rgb.to_be_bytes();
        set_if_changed(win.get_led_red(), f32::from(r) / 255.0, |v| {
            win.set_led_red(v);
        });
        set_if_changed(win.get_led_green(), f32::from(g) / 255.0, |v| {
            win.set_led_green(v);
        });
        set_if_changed(win.get_led_blue(), f32::from(bl) / 255.0, |v| {
            win.set_led_blue(v);
        });
        set_if_changed(
            win.get_led_color_hex_text(),
            format!("#{rgb:06X}").into(),
            |v| win.set_led_color_hex_text(v),
        );
        let key = (pattern_i, brightness, led.led_color.unwrap_or(u32::MAX));
        if self.led_preview_key.get() != key {
            self.led_preview_key.set(key);
            win.set_led_preview(ModelRc::new(Rc::new(VecModel::from(led_preview(
                pattern,
                brightness,
                led.led_color,
            )))));
        }
    }

    /// Write the Horn card's tuning props from a record (HO1/HO2):
    /// button label, learning state, minimum-honk slider + text,
    /// mode segment. All writes are change-guarded, like
    /// `push_frame`.
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    fn sync_extras_horn_props(&self, win: &MainWindow, horn: &TuningSettings) {
        set_if_changed(
            win.get_horn_button(),
            match horn.horn_button {
                Some(b) => format!("Button {b}").into(),
                None => "Not set".into(),
            },
            |v| win.set_horn_button(v),
        );
        set_if_changed(win.get_horn_learning(), self.learn_target.get() == 2, |v| {
            win.set_horn_learning(v);
        });
        let min_ms = horn.horn_min_ms.unwrap_or(400.0);
        set_if_changed(win.get_horn_min_ms(), min_ms as f32, |v| {
            win.set_horn_min_ms(v);
        });
        set_if_changed(
            win.get_horn_min_text(),
            format!("{min_ms:.0} ms").into(),
            |v| win.set_horn_min_text(v),
        );
        set_if_changed(
            win.get_horn_mode(),
            i32::from(horn.horn_latch.unwrap_or(false)),
            |v| win.set_horn_mode(v),
        );
    }

    /// Write the Buttons page's encoder tuning props from a record
    /// (E1/E2/E3/E4, R2-10): pulse slider + text, steps-per-click
    /// segment, acceleration switch + strength slider + text, and
    /// the per-knob invert switches. All writes are change-guarded,
    /// like `push_frame`.
    #[allow(clippy::cast_possible_truncation)]
    fn sync_encoder_props(win: &MainWindow, enc: &TuningSettings) {
        let pulse_ms = enc.encoder_pulse_ms.unwrap_or(50.0);
        set_if_changed(win.get_enc_pulse_ms(), pulse_ms as f32, |v| {
            win.set_enc_pulse_ms(v);
        });
        set_if_changed(
            win.get_enc_pulse_text(),
            format!("{pulse_ms:.0} ms").into(),
            |v| win.set_enc_pulse_text(v),
        );
        set_if_changed(
            win.get_enc_counts(),
            i32::from(enc.encoder_counts_per_click.unwrap_or(4)),
            |v| win.set_enc_counts(v),
        );
        set_if_changed(
            win.get_enc_accel(),
            enc.encoder_accel.unwrap_or(false),
            |v| {
                win.set_enc_accel(v);
            },
        );
        let sens = enc.encoder_accel_sensitivity.unwrap_or(1.0);
        set_if_changed(win.get_enc_accel_sens(), sens as f32, |v| {
            win.set_enc_accel_sens(v);
        });
        set_if_changed(
            win.get_enc_accel_sens_text(),
            format!("{:.0}%", sens * 100.0).into(),
            |v| win.set_enc_accel_sens_text(v),
        );
        let mask = enc.encoder_invert_mask.unwrap_or(0);
        set_if_changed(win.get_enc_invert1(), mask & 0b0001 != 0, |v| {
            win.set_enc_invert1(v);
        });
        set_if_changed(win.get_enc_invert2(), mask & 0b0010 != 0, |v| {
            win.set_enc_invert2(v);
        });
    }

    /// Write the Extras page's shifter props from a tuning record +
    /// the cached board sensitivity (SH1/SH2, R2-10): the lockout
    /// slider + text from the record, the sensitivity slider + text
    /// from the board cache. While a lockout drag's push is pending,
    /// the local echo owns that slider — the slow cache must not
    /// snap it back mid-drag; the same guard covers a sensitivity
    /// drag. All writes are change-guarded, like `push_frame`.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn sync_shifter_props(&self, win: &MainWindow, shifter: &TuningSettings) {
        if !self.field_push_pending_for("shifter") {
            let lockout_ms = shifter.shift_lockout_ms.unwrap_or(50.0);
            set_if_changed(
                win.get_shifter_lockout(),
                (lockout_ms / 200.0) as f32,
                |v| {
                    win.set_shifter_lockout(v);
                },
            );
            set_if_changed(
                win.get_shifter_lockout_text(),
                format!("{lockout_ms:.0} ms").into(),
                |v| win.set_shifter_lockout_text(v),
            );
        }
        if self.pending_sensitivity.borrow().is_some() {
            return;
        }
        match self.sensitivity_cached.get() {
            Some(Some(value)) => {
                set_if_changed(win.get_sensitivity_disabled(), false, |v| {
                    win.set_sensitivity_disabled(v);
                });
                set_if_changed(
                    win.get_shifter_sensitivity(),
                    f32::from(value) / 100.0,
                    |v| {
                        win.set_shifter_sensitivity(v);
                    },
                );
                set_if_changed(
                    win.get_shifter_sensitivity_text(),
                    format!("{value}%").into(),
                    |v| win.set_shifter_sensitivity_text(v),
                );
            }
            Some(None) => {
                set_if_changed(win.get_sensitivity_disabled(), true, |v| {
                    win.set_sensitivity_disabled(v);
                });
                set_if_changed(
                    win.get_shifter_sensitivity_reason(),
                    "No shifter is configured on the board.".into(),
                    |v| win.set_shifter_sensitivity_reason(v),
                );
            }
            None => {
                // Unknown: "Reading…" while the fetch runs (or has
                // not started yet); after a failed read the reason
                // says so until the 30 s backoff lets it retry.
                set_if_changed(win.get_sensitivity_disabled(), true, |v| {
                    win.set_sensitivity_disabled(v);
                });
                let reason = if self
                    .busy_jobs
                    .borrow()
                    .contains_key(&JobKind::SensitivityPush)
                    || self.sensitivity_failed_at.get().is_none()
                {
                    "Reading the shifter setting…"
                } else {
                    "Couldn't read the shifter setting — check the board connection."
                };
                set_if_changed(win.get_shifter_sensitivity_reason(), reason.into(), |v| {
                    win.set_shifter_sensitivity_reason(v);
                });
            }
        }
    }

    /// Echo an encoder slider drag locally (R2-10): the slider + text
    /// move with the drag, not at the next ~1 Hz re-sync. Only the
    /// encoder fields are read, so the neutral record is enough —
    /// no API round-trip per drag move. Both slider values are
    /// passed in (the moving one fresh, the other from the window),
    /// so a drag never clobbers the other slider's value.
    fn apply_encoder_local(win: &MainWindow, pulse_ms: f64, sens: f64) {
        let record = TuningSettings {
            module: "encoders".to_string(),
            encoder_pulse_ms: Some(pulse_ms),
            encoder_counts_per_click: Some(u8::try_from(win.get_enc_counts()).unwrap_or(4)),
            encoder_accel: Some(win.get_enc_accel()),
            encoder_accel_sensitivity: Some(sens),
            encoder_invert_mask: Some(
                (u8::from(win.get_enc_invert1())) | (u8::from(win.get_enc_invert2()) << 1),
            ),
            ..TuningSettings::neutral("encoders")
        };
        Self::sync_encoder_props(win, &record);
    }

    /// Write the Buttons page's button tuning props from a record
    /// (BU1/BU2/BU3, R2-10): debounce slider + text, long-press
    /// threshold slider + text, double-press window slider + text.
    /// All writes are change-guarded, like `push_frame`.
    #[allow(clippy::cast_possible_truncation)]
    fn sync_button_props(win: &MainWindow, buttons: &TuningSettings) {
        let debounce_ms = buttons.button_debounce_ms.unwrap_or(10.0);
        set_if_changed(win.get_btn_debounce_ms(), debounce_ms as f32, |v| {
            win.set_btn_debounce_ms(v);
        });
        set_if_changed(
            win.get_btn_debounce_text(),
            format!("{debounce_ms:.0} ms").into(),
            |v| win.set_btn_debounce_text(v),
        );
        let long_press_ms = buttons.button_long_press_ms.unwrap_or(800.0);
        set_if_changed(win.get_btn_longpress_ms(), long_press_ms as f32, |v| {
            win.set_btn_longpress_ms(v);
        });
        set_if_changed(
            win.get_btn_longpress_text(),
            format!("{long_press_ms:.0} ms").into(),
            |v| win.set_btn_longpress_text(v),
        );
        let double_press_ms = buttons.button_double_press_ms.unwrap_or(300.0);
        set_if_changed(win.get_btn_doublepress_ms(), double_press_ms as f32, |v| {
            win.set_btn_doublepress_ms(v);
        });
        set_if_changed(
            win.get_btn_doublepress_text(),
            format!("{double_press_ms:.0} ms").into(),
            |v| win.set_btn_doublepress_text(v),
        );
    }

    /// Echo a button tuning slider drag locally (R2-10): same
    /// pattern as [`Self::apply_encoder_local`]. All three slider
    /// values are passed in (the moving one fresh, the others from
    /// the window), so a drag never clobbers another slider's value.
    fn apply_button_local(
        win: &MainWindow,
        debounce_ms: f64,
        long_press_ms: f64,
        double_press_ms: f64,
    ) {
        let record = TuningSettings {
            module: "buttons".to_string(),
            button_debounce_ms: Some(debounce_ms),
            button_long_press_ms: Some(long_press_ms),
            button_double_press_ms: Some(double_press_ms),
            ..TuningSettings::neutral("buttons")
        };
        Self::sync_button_props(win, &record);
    }

    /// Echo a handbrake deadzone drag locally (R2-10): same pattern
    /// as [`Self::apply_pedal_deadzone_local`].
    fn apply_handbrake_deadzone_local(&self, win: &MainWindow, deadzone: f64) {
        let record = {
            let mut stored = self.handbrake_record.borrow_mut();
            stored.deadzone = deadzone;
            stored.clone()
        };
        Self::sync_extras_handbrake_props(win, &record);
    }

    /// Echo a handbrake grab-speed drag locally (R2-10, H2): the
    /// slider + text move with the mouse, the local record follows,
    /// and only the daemon push waits out the 150 ms debounce.
    fn apply_handbrake_grab_local(&self, win: &MainWindow, grab_ms: f64) {
        let record = {
            let mut stored = self.handbrake_record.borrow_mut();
            stored.press_ms = Some(grab_ms);
            stored.clone()
        };
        Self::sync_extras_handbrake_props(win, &record);
    }

    /// Echo an LED brightness drag locally (R2-10): same pattern as
    /// [`Self::apply_pedal_deadzone_local`].
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn apply_led_brightness_local(&self, win: &MainWindow, brightness: f64) {
        let record = {
            let mut stored = self.led_record.borrow_mut();
            stored.led_brightness = Some(brightness.round() as u8);
            stored.clone()
        };
        self.sync_extras_led_props(win, &record);
    }

    /// Echo an LED period drag locally (R2-10, L1): same pattern as
    /// [`Self::apply_led_brightness_local`]. The pattern is stamped
    /// from the window prop so the `set_tuning` flash fires at the
    /// new speed even on a fresh profile (stored pattern `None`).
    fn apply_led_period_local(&self, win: &MainWindow, period_ms: f64) {
        let record = {
            let mut stored = self.led_record.borrow_mut();
            stored.led_period_ms = Some(period_ms);
            if stored.led_pattern.is_none() {
                stored.led_pattern = Some(match win.get_led_pattern() {
                    0 => LedPattern::Off,
                    2 => LedPattern::Blink,
                    3 => LedPattern::Chase,
                    4 => LedPattern::Rainbow,
                    _ => LedPattern::Solid,
                });
            }
            stored.clone()
        };
        self.sync_extras_led_props(win, &record);
    }

    /// Echo a custom-colour drag locally (R2-10, L2): the composed
    /// colour follows in the local record; the preview, the RGB
    /// sliders and the hex text re-render from it.
    fn apply_led_color_local(&self, win: &MainWindow, color: u32) {
        let record = {
            let mut stored = self.led_record.borrow_mut();
            stored.led_color = Some(color);
            stored.clone()
        };
        self.sync_extras_led_props(win, &record);
    }

    /// Echo a horn minimum-honk drag locally (R2-10, HO1): same
    /// pattern as [`Self::apply_led_brightness_local`].
    fn apply_horn_min_local(&self, win: &MainWindow, min_ms: f64) {
        let record = {
            let mut stored = self.horn_record.borrow_mut();
            stored.horn_min_ms = Some(min_ms);
            stored.clone()
        };
        self.sync_extras_horn_props(win, &record);
    }

    /// Refresh the Pedals page from the stored records (A1/A2):
    /// mV from min/max, deadzone slider = pct/10, feel from the
    /// stored curve, mini-curve paths from feel + deadzone, bite for
    /// the clutch. Called at startup, on swap-to-live and after
    /// every pedal edit — the page never shows literals.
    /// R2-11: every write is change-guarded like `push_frame`, so
    /// the ~1 Hz re-sync stops rewriting (and repainting) props
    /// that did not move. R2-10: a module with a field push still
    /// pending keeps its LOCAL record — the drag handler's value —
    /// instead of being snapped back to the ~1 Hz cache mid-drag.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn sync_pedals(&self, win: &MainWindow) {
        const MODULES: [&str; 3] = ["throttle", "brake", "clutch"];
        let api = self.api();
        // A2: mV comes from calibration() (keys "accel" / "brake" /
        // "clutch") when a capture exists, else the tuning record.
        let cal = self.calibration.borrow().clone();
        let cal_mv = |key: &str, min: bool| -> Option<f64> {
            cal.as_ref().and_then(|c| {
                if min {
                    c.pedal_min_mv.get(key).copied()
                } else {
                    c.pedal_max_mv.get(key).copied()
                }
            })
        };
        let mut cache = self.pedals.borrow_mut();
        for (i, module) in MODULES.iter().enumerate() {
            // R2-10: while a drag's push is pending for this module,
            // the local record IS the drag value — re-reading the
            // slow cache here would snap the slider back mid-drag.
            if !self.field_push_pending_for(module) {
                let record = api.get_tuning(module);
                cache.feel_exp[i] = record.curve as f32;
                cache.records[i] = record;
            }
        }
        let (t, b, c) = (
            cache.records[0].clone(),
            cache.records[1].clone(),
            cache.records[2].clone(),
        );
        let feel_exps = cache.feel_exp;
        drop(cache);
        // A1: slider value = pct/10 (0..10% range), handler does v*10.
        set_if_changed(
            win.get_throttle_up_mv(),
            mv_text(cal_mv("accel", true).unwrap_or(t.min_mv)).into(),
            |v| win.set_throttle_up_mv(v),
        );
        set_if_changed(
            win.get_throttle_down_mv(),
            mv_text(cal_mv("accel", false).unwrap_or(t.max_mv)).into(),
            |v| win.set_throttle_down_mv(v),
        );
        Self::sync_pedal_module_props(win, 0, &t, feel_exps[0]);
        set_if_changed(
            win.get_brake_up_mv(),
            mv_text(cal_mv("brake", true).unwrap_or(b.min_mv)).into(),
            |v| win.set_brake_up_mv(v),
        );
        set_if_changed(
            win.get_brake_down_mv(),
            mv_text(cal_mv("brake", false).unwrap_or(b.max_mv)).into(),
            |v| win.set_brake_down_mv(v),
        );
        Self::sync_pedal_module_props(win, 1, &b, feel_exps[1]);
        set_if_changed(
            win.get_clutch_up_mv(),
            mv_text(cal_mv("clutch", true).unwrap_or(c.min_mv)).into(),
            |v| win.set_clutch_up_mv(v),
        );
        set_if_changed(
            win.get_clutch_down_mv(),
            mv_text(cal_mv("clutch", false).unwrap_or(c.max_mv)).into(),
            |v| win.set_clutch_down_mv(v),
        );
        Self::sync_pedal_module_props(win, 2, &c, feel_exps[2]);
        // --- Pedal ramps/curves per-module tuning ---
        // Throttle saturation (percent of the calibrated travel)
        // and the combined-pedals switch follow the throttle record.
        self.sync_throttle_saturation_props(win, &t);
        set_if_changed(
            win.get_combine_pedals(),
            t.combine_pedals.unwrap_or(false),
            |v| win.set_combine_pedals(v),
        );
        // Set up/down capture the live endpoints (v2.0.2): enabled
        // while the daemon is live, disabled with a reason otherwise.
        set_if_changed(
            win.get_pedal_set_disabled(),
            !self.capture_available(),
            |v| win.set_pedal_set_disabled(v),
        );
    }

    /// Refresh the Extras page from the stored records (A6):
    /// handbrake mV/deadzone/feel, shifter gate + lockout, horn
    /// button, LED pattern + brightness — all `set_tuning` fields —
    /// plus the shifter sensitivity from the board config blob
    /// (SH1, `sync_shifter_props`). Controls with no backend field
    /// (LED colour, handbrake mode) stay disabled / explain
    /// themselves in the Slint.
    /// R2-11: every write is change-guarded like `push_frame`, so
    /// the ~1 Hz re-sync stops rewriting props that did not move.
    /// R2-10: a module with a field push still pending keeps its
    /// LOCAL record — the drag handler's value — instead of being
    /// snapped back to the ~1 Hz cache mid-drag.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn sync_extras(&self, win: &MainWindow) {
        let api = self.api();
        // R2-10: while a drag's push is pending for handbrake/LED,
        // the local record IS the drag value — re-reading the slow
        // cache here would snap the slider back mid-drag.
        let hb = if self.field_push_pending_for("handbrake") {
            self.handbrake_record.borrow().clone()
        } else {
            let record = api.get_tuning("handbrake");
            *self.handbrake_record.borrow_mut() = record.clone();
            record
        };
        let led = if self.field_push_pending_for("led") {
            self.led_record.borrow().clone()
        } else {
            let record = api.get_tuning("led");
            *self.led_record.borrow_mut() = record.clone();
            record
        };
        let shifter = api.get_tuning("shifter");
        let horn = if self.field_push_pending_for("horn") {
            self.horn_record.borrow().clone()
        } else {
            let record = api.get_tuning("horn");
            *self.horn_record.borrow_mut() = record.clone();
            record
        };
        Self::sync_extras_handbrake_props(win, &hb);
        // H7: keep the toggle tracking in sync with the daemon (the
        // .slint toggles flip first and call Rust with no payload, so
        // Rust tracks the last known state per module).
        *self.module_states.borrow_mut() = [
            self.api().module_enabled("handbrake"),
            self.api().module_enabled("shifter"),
            self.api().module_enabled("horn"),
            self.api().module_enabled("led"),
        ];
        // FC2: the Extras `-on` props are Rust-written too — every
        // re-sync re-seeds them from the daemon, so the switches
        // never disagree with the real module state (a failed toggle
        // reverts the prop in `wire_extras`; this is the seed).
        {
            let states = *self.module_states.borrow();
            set_extras_on_prop(win, 0, states[0]);
            set_extras_on_prop(win, 1, states[1]);
            set_extras_on_prop(win, 2, states[2]);
            set_extras_on_prop(win, 3, states[3]);
        }
        set_if_changed(
            win.get_extras_calibration_disabled(),
            !self.capture_available(),
            |v| win.set_extras_calibration_disabled(v),
        );
        set_if_changed(
            win.get_shifter_mode(),
            match shifter.shifter_gate {
                Some(ShifterGate::HPattern) => 1,
                _ => 0,
            },
            |v| win.set_shifter_mode(v),
        );
        self.sync_extras_horn_props(win, &horn);
        self.sync_extras_led_props(win, &led);
        // E1/E2: the Buttons page's encoder tuning rides this slow
        // re-sync (no separate sync_buttons exists). While a drag's
        // push is pending, the local echo owns the slider — the
        // slow cache must not snap it back mid-drag (R2-10).
        if !self.field_push_pending_for("encoders") {
            let enc = api.get_tuning("encoders");
            Self::sync_encoder_props(win, &enc);
        }
        // BU1/BU2: the Buttons page's button tuning rides the same
        // slow re-sync, with the same pending-drag guard.
        if !self.field_push_pending_for("buttons") {
            let buttons = api.get_tuning("buttons");
            Self::sync_button_props(win, &buttons);
        }
        // SH1/SH2: the shifter props ride this slow re-sync — the
        // lockout from the tuning record, the sensitivity from the
        // board cache (see `sync_shifter_props`).
        self.sync_shifter_props(win, &shifter);
        // SH1: the board sensitivity is read once, on first Extras
        // visit — a `get_config` board session on every ~1 Hz sync
        // would be a control session per second. A failed read
        // retries at most every 30 s; a SensitivityPush job already
        // running (fetch or write) is never doubled.
        if win.get_page() == 4
            && self.sensitivity_cached.get().is_none()
            && !self
                .busy_jobs
                .borrow()
                .contains_key(&JobKind::SensitivityPush)
            && self
                .sensitivity_failed_at
                .get()
                .is_none_or(|at| at.elapsed() >= Duration::from_secs(30))
        {
            self.fetch_shifter_sensitivity(win);
        }
    }

    /// Refresh the Overview live values (A11/A12/A14/E): firmware
    /// line from the API, delay/jitter/loss from `diag_stats()` ("—"
    /// until the daemon measures), calibration label from
    /// `calibration().calibrated_at`, backend name + readiness from
    /// `output_backend()`.
    /// R2-11: every write is change-guarded like `push_frame`, so
    /// the ~1 Hz re-sync stops rewriting (and repainting) props
    /// that did not move.
    #[allow(clippy::too_many_lines)]
    fn sync_overview(&self, win: &MainWindow) {
        let api = self.api();
        let conn = api.connection();
        let firmware = api.firmware_version();
        // H4: the address falls back to the persisted board address
        // (never the hardcoded "rsh.local") — the transport already
        // uses the same persisted value.
        let address = if conn.address.is_empty() {
            api.board_address()
        } else {
            conn.address.clone()
        };
        // L-2: the wizard's address field shows the live address
        // (never its hardcoded "rsh.local" default).
        set_if_changed(win.get_setup_address(), address.clone().into(), |v| {
            win.set_setup_address(v);
        });
        // A11: measured stats, or "—" — never a hardcoded 4 ms / 0%.
        let stats = api.diag_stats().ok();
        let delay = stats
            .as_ref()
            .and_then(|s| s.e2e_ms)
            .map_or_else(|| "—".to_string(), |v| format!("{v:.0} ms"));
        let jitter = stats
            .as_ref()
            .and_then(|s| s.jitter_ms_p99)
            .map_or_else(|| "—".to_string(), |v| format!("{v:.0} ms"));
        let lost = stats
            .as_ref()
            .map_or_else(|| "—".to_string(), |s| format!("{:.1}%", s.loss_pct));
        let loop_suffix = stats
            .as_ref()
            .and_then(|s| s.loop_hz)
            .map_or_else(String::new, |hz| format!(" · {hz:.0} updates/s"));
        let firmware_line = if firmware == "Unknown" {
            "Firmware —".to_string()
        } else {
            format!("Firmware {firmware} · {address}{loop_suffix}")
        };
        set_if_changed(win.get_firmware_line(), firmware_line.into(), |v| {
            win.set_firmware_line(v);
        });
        // Phase 2 (Wi-Fi gap / RC-3): the Overview disconnected
        // banner names the fix while the daemon's last activation
        // error is Unreachable, and the first Unreachable of an
        // outage also raises a notification — the user is told
        // proactively ("join the RSH Wi-Fi / plug in USB"), not
        // only after repeated silent retries. Once per outage:
        // the flag clears when the error clears.
        let unreachable = matches!(
            conn.state,
            ConnectionState::Error {
                reason: ConnectionError::Unreachable,
                ..
            }
        );
        set_if_changed(win.get_overview_unreachable(), unreachable, |v| {
            win.set_overview_unreachable(v);
        });
        if unreachable && !self.unreachable_notified.get() {
            self.unreachable_notified.set(true);
            self.notify(
                win,
                "Connection",
                "Wheel not found",
                "Join the 'RSH-XXXX' Wi-Fi network (or plug in USB), then press Reconnect.",
            );
            ui_log!(
                "RSH-UI first Unreachable this outage — proactive Wi-Fi/USB notification shown"
            );
        } else if !unreachable {
            self.unreachable_notified.set(false);
        }
        set_if_changed(win.get_delay_text(), delay.into(), |v| {
            win.set_delay_text(v);
        });
        set_if_changed(win.get_jitter_text(), jitter.into(), |v| {
            win.set_jitter_text(v);
        });
        set_if_changed(win.get_lost_text(), lost.into(), |v| win.set_lost_text(v));
        // A12: the capture date, or "Not calibrated" (warn state).
        let cal_date = self.calibration_date();
        if let Some(date) = cal_date.as_ref() {
            set_if_changed(
                win.get_calibrated_label(),
                format!("Calibrated {date}").into(),
                |v| win.set_calibrated_label(v),
            );
            set_if_changed(win.get_calibrated_warn(), false, |v| {
                win.set_calibrated_warn(v);
            });
        } else {
            set_if_changed(win.get_calibrated_label(), "Not calibrated".into(), |v| {
                win.set_calibrated_label(v);
            });
            set_if_changed(win.get_calibrated_warn(), true, |v| {
                win.set_calibrated_warn(v);
            });
        }
        // A22: the "Set" pill only once a centre was captured.
        set_if_changed(win.get_centre_set(), cal_date.is_some(), |v| {
            win.set_centre_set(v);
        });
        let backend = api.output_backend();
        let backend_name = if backend == "Unknown" {
            "Not ready".to_string()
        } else {
            backend.clone()
        };
        set_if_changed(win.get_backend_name(), backend_name.into(), |v| {
            win.set_backend_name(v);
        });
        set_if_changed(win.get_backend_ready(), backend != "Unknown", |v| {
            win.set_backend_ready(v);
        });
        // C3/M-8: Settings firmware text from the cached firmware
        // check (board live version vs bundled OTA image version) —
        // never the hardcoded "4.0.2". No cache yet (startup, before
        // the first check) falls back to the live version alone.
        let firmware_text = match self.firmware_check_cache.borrow().as_ref() {
            Some(check) if check.board_version != "Unknown" => {
                if check.update_available {
                    format!(
                        "Version {} · update {} available",
                        check.board_version,
                        check.bundled_version.as_deref().unwrap_or("?")
                    )
                } else {
                    format!("Version {} · up to date", check.board_version)
                }
            }
            _ => {
                if firmware == "Unknown" {
                    "—".to_string()
                } else {
                    format!("Version {firmware}")
                }
            }
        };
        set_if_changed(win.get_firmware_text(), firmware_text.into(), |v| {
            win.set_firmware_text(v);
        });
        set_if_changed(
            win.get_app_update_text(),
            "Auto-update is off — new versions are on GitHub Releases".into(),
            |v| win.set_app_update_text(v),
        );
    }

    /// Refresh the profile lists (A8/C6): the popover model, the
    /// Settings list, the top-bar / Overview profile name and the
    /// auto-profile chip — all from `list_profiles()`, never
    /// hardcoded. Empty lists stay empty (no invented profiles).
    /// R2-11: the list models are only rebuilt when their content
    /// hash moved — the 1 Hz re-sync and the 5 s tick no longer
    /// rebuild every `for` row (and reset hover/selection) while
    /// nothing changed. The scalar props are change-guarded too.
    fn refresh_profiles(&self, win: &MainWindow) {
        let profiles = self.api().list_profiles();
        let active = profiles.iter().find(|p| p.active);
        let profile_name = active.map_or_else(|| "No profile".to_string(), |p| p.name.clone());
        set_if_changed(win.get_profile_name(), profile_name.into(), |v| {
            win.set_profile_name(v);
        });
        let games = self.games_cache.borrow();
        let auto = active.map(|p| {
            let game = p
                .game
                .as_ref()
                .and_then(|slug| games.iter().find(|g| &g.id == slug).map(|g| g.name.clone()))
                .unwrap_or_else(|| {
                    p.game
                        .as_deref()
                        .map_or_else(|| "No game".to_string(), pretty_game_slug)
                });
            format!("{game} → {}", p.name)
        });
        drop(games);
        set_if_changed(
            win.get_auto_profile_text(),
            auto.unwrap_or_else(|| "No profile yet".to_string()).into(),
            |v| win.set_auto_profile_text(v),
        );
        // Profiles model (popover + Settings list) — rebuild only
        // when (id, name, game, active) moved.
        let profiles_hash = content_hash(
            &profiles
                .iter()
                .map(|p| (p.id.as_str(), p.name.as_str(), p.game.as_deref(), p.active))
                .collect::<Vec<_>>(),
        );
        if self.profiles_model_hash.get() != Some(profiles_hash) {
            self.profiles_model_hash.set(Some(profiles_hash));
            let rows: Vec<ProfileItem> = profiles
                .iter()
                .map(|p| ProfileItem {
                    name: p.name.clone().into(),
                    game: p
                        .game
                        .as_deref()
                        .map_or_else(|| "No game".to_string(), pretty_game_slug)
                        .into(),
                    active: p.active,
                })
                .collect();
            win.set_popover_profiles(ModelRc::new(Rc::new(VecModel::from(rows))));
            let settings_rows: Vec<ProfileRow> = profiles
                .iter()
                .map(|p| ProfileRow {
                    name: p.name.clone().into(),
                    games: p
                        .game
                        .as_deref()
                        .map_or_else(|| "No game".to_string(), pretty_game_slug)
                        .into(),
                    active: p.active,
                })
                .collect();
            win.set_settings_profiles(ModelRc::new(Rc::new(VecModel::from(settings_rows))));
        }
        // M1: presets are one row per catalogue game (real data —
        // the games list), read-only templates: "Use" creates a
        // profile from the game. No invented preset store. The games
        // cache may be empty on the very first call (refresh_games
        // runs after this) — then read the list live once.
        let preset_games = {
            let cached = self.games_cache.borrow();
            if cached.is_empty() {
                drop(cached);
                self.api().list_games()
            } else {
                cached.clone()
            }
        };
        self.presets_count.set(preset_games.len());
        // Presets model — rebuild only when the games (id, name) moved.
        let presets_hash = content_hash(
            &preset_games
                .iter()
                .map(|g| (g.id.as_str(), g.name.as_str()))
                .collect::<Vec<_>>(),
        );
        if self.presets_model_hash.get() != Some(presets_hash)
            || preset_games != *self.presets_games.borrow()
        {
            self.presets_model_hash.set(Some(presets_hash));
            let presets: Vec<ProfileItem> = preset_games
                .iter()
                .map(|g| ProfileItem {
                    name: g.name.clone().into(),
                    game: g.name.clone().into(),
                    active: false,
                })
                .collect();
            win.set_popover_presets(ModelRc::new(Rc::new(VecModel::from(presets))));
            *self.presets_games.borrow_mut() = preset_games;
        }
        *self.profiles_cache.borrow_mut() = profiles;
    }

    /// Refresh the game tiles (A14) + the wizard Games step (A17)
    /// from `list_games()`. Tiles: running → Stop, installed →
    /// Launch, not installed → "Not installed" with NO action button
    /// (no fake Locate). Games are manual-only now (no auto-scan):
    /// an empty list shows the add-game empty state, never
    /// invented games.
    /// R2-11: the tiles model is only rebuilt when the games or
    /// the profiles they name changed — the ~1 Hz / 5 s re-sync
    /// used to rebuild it with zero changes. The wizard text is
    /// change-guarded, so the first call still seeds it.
    fn refresh_games(&self, win: &MainWindow) {
        let games = self.api().list_games();
        let profiles = self.profiles_cache.borrow().clone();
        if games.is_empty() {
            set_if_changed(
                win.get_setup_games_text(),
                "No games added yet.".into(),
                |v| win.set_setup_games_text(v),
            );
            set_if_changed(win.get_setup_games_done(), false, |v| {
                win.set_setup_games_done(v);
            });
        } else {
            let names: Vec<String> = games.iter().map(|g| g.name.clone()).collect();
            set_if_changed(
                win.get_setup_games_text(),
                format!(
                    "Added {} game{}: {}",
                    games.len(),
                    if games.len() == 1 { "" } else { "s" },
                    names.join(", ")
                )
                .into(),
                |v| win.set_setup_games_text(v),
            );
            set_if_changed(win.get_setup_games_done(), true, |v| {
                win.set_setup_games_done(v);
            });
        }
        if games == *self.games_cache.borrow() && profiles == *self.games_profiles.borrow() {
            return;
        }
        let games_hash = content_hash(
            &games
                .iter()
                .map(|g| {
                    let bound = g.bound_profile_id.as_ref().and_then(|id| {
                        profiles
                            .iter()
                            .find(|p| &p.id == id)
                            .map(|p| p.name.as_str())
                    });
                    (
                        g.id.as_str(),
                        g.name.as_str(),
                        g.running,
                        g.installed,
                        g.bound_profile_id.as_deref(),
                        bound,
                    )
                })
                .collect::<Vec<_>>(),
        );
        if self.games_model_hash.get() != Some(games_hash) {
            self.games_model_hash.set(Some(games_hash));
            let items: Vec<GameItem> = games
                .iter()
                .map(|g| {
                    let bound = g.bound_profile_id.as_ref().and_then(|id| {
                        profiles
                            .iter()
                            .find(|p| &p.id == id)
                            .map(|p| p.name.clone())
                    });
                    let (sub, sub_warn, action, action_primary) = if g.running {
                        (
                            match &bound {
                                Some(name) => format!("Running · {name}"),
                                None => "Running".to_string(),
                            },
                            true,
                            "Stop".to_string(),
                            false,
                        )
                    } else if g.installed {
                        (
                            bound.unwrap_or_else(|| "Ready".to_string()),
                            false,
                            "Launch".to_string(),
                            true,
                        )
                    } else {
                        ("Not installed".to_string(), false, String::new(), false)
                    };
                    GameItem {
                        id: g.id.clone().into(),
                        code: game_code(&g.name).into(),
                        name: g.name.clone().into(),
                        sub: sub.into(),
                        sub_warn,
                        action: action.into(),
                        action_primary,
                    }
                })
                .collect();
            win.set_games(ModelRc::new(Rc::new(VecModel::from(items))));
        }
        *self.games_cache.borrow_mut() = games;
        *self.games_profiles.borrow_mut() = profiles;
    }

    /// Open the add/edit game dialog for a new game (`None`) or
    /// the game with this id. Games are manual-only: the exe path
    /// comes from the native file picker (Browse — see
    /// [`Ctx::pick_game_exe`]), and [`GameSummary`] carries no exe
    /// path, so Edit pre-fills the name + bound profile and the exe
    /// field starts empty — the user picks the file again with
    /// Browse. Refuses
    /// while RSH isn't running: there is no store to save to, so
    /// say so instead of opening a dialog whose Save can only fail.
    fn open_game_dialog(&self, win: &MainWindow, edit_id: Option<String>) {
        if !self.is_live() {
            self.notify_toast(
                win,
                "Games",
                "RSH isn't running",
                "RSH isn't running — reconnect, then add a game.",
            );
            return;
        }
        self.refresh_profiles(win);
        let game = edit_id.as_deref().and_then(|id| {
            self.games_cache
                .borrow()
                .iter()
                .find(|g| g.id == id)
                .cloned()
        });
        if edit_id.is_some() && game.is_none() {
            self.notify_toast(
                win,
                "Game",
                "Game not found",
                "That game isn't in the list any more.",
            );
            return;
        }
        let (title, name, profile_id) = match &game {
            Some(g) => (
                "Edit game".to_string(),
                g.name.clone(),
                g.bound_profile_id.clone(),
            ),
            None => ("Add game".to_string(), String::new(), None),
        };
        *self.game_dialog_edit_id.borrow_mut() = edit_id;
        self.game_dialog_profile_id
            .borrow_mut()
            .clone_from(&profile_id);
        win.set_game_dialog_title(title.into());
        win.set_game_dialog_name(name.into());
        win.set_game_dialog_exe("".into());
        win.set_game_dialog_profile_name(
            self.game_dialog_profile_label(profile_id.as_ref()).into(),
        );
        win.set_game_dialog_open(true);
        ui_log!("RSH-UI game dialog opened");
    }

    /// Browse for the game file (PC-test Bug 6, WS-C): open the
    /// native Windows file picker ([`file_picker::pick_game_exe`])
    /// and fill the dialog from the selection. Cancel leaves the
    /// dialog exactly as it was — no error, no toast. A selection
    /// fills the (read-only) exe path and auto-fills the name from
    /// the file stem, but only when the name field is still empty
    /// (an Edit keeps its existing name; a typed name is never
    /// overwritten). A picker failure is a plain toast, never a
    /// silent no-op and never raw COM wording.
    fn pick_game_exe(&self, win: &MainWindow) {
        match file_picker::pick_game_exe() {
            Ok(Some(path)) => {
                win.set_game_dialog_exe(path.to_string_lossy().into_owned().into());
                if win.get_game_dialog_name().trim().is_empty() {
                    let name = file_picker::game_name_from_path(&path);
                    if !name.is_empty() {
                        win.set_game_dialog_name(name.into());
                    }
                }
                ui_log!("RSH-UI game picker selected {}", path.display());
            }
            Ok(None) => {
                // Cancelled — the dialog stays as it was.
            }
            Err(detail) => {
                ui_log!("RSH-UI game picker failed: {detail}");
                self.notify_toast(
                    win,
                    "Game",
                    "Couldn't open the file picker",
                    "The file picker couldn't be opened — try again.",
                );
            }
        }
    }

    /// Display name for the dialog's bound profile: "No profile"
    /// when none is bound (a stale id falls back the same way).
    fn game_dialog_profile_label(&self, profile_id: Option<&String>) -> String {
        match profile_id {
            Some(id) => self
                .profiles_cache
                .borrow()
                .iter()
                .find(|p| &p.id == id)
                .map_or_else(|| "No profile".to_string(), |p| p.name.clone()),
            None => "No profile".to_string(),
        }
    }

    /// Cycle the dialog's bound profile: none → first profile →
    /// … → last → none again (the profile-link control's cycling
    /// pattern — the dialog has no dropdown component).
    fn cycle_game_dialog_profile(&self, win: &MainWindow) {
        let profiles = self.profiles_cache.borrow().clone();
        if profiles.is_empty() {
            self.notify(
                win,
                "Profile",
                "No profiles yet",
                "Create a profile first, then link it to a game.",
            );
            return;
        }
        let current = self.game_dialog_profile_id.borrow().clone();
        let next = match current
            .as_deref()
            .and_then(|id| profiles.iter().position(|p| p.id == id))
        {
            None => Some(profiles[0].id.clone()),
            Some(i) if i + 1 < profiles.len() => Some(profiles[i + 1].id.clone()),
            Some(_) => None,
        };
        win.set_game_dialog_profile_name(self.game_dialog_profile_label(next.as_ref()).into());
        *self.game_dialog_profile_id.borrow_mut() = next;
    }

    /// Save the dialog: validate name + exe (empty is refused
    /// through `notify_error`, like every other failure), then run
    /// the add / update on a [`JobKind::GameOp`] job thread — the
    /// click never blocks on the daemon. Success refreshes the
    /// game tiles + profiles (a new bound profile changes the
    /// auto-profile line) and closes the dialog.
    fn save_game_dialog(&self, win: &MainWindow, name: &str, exe: &str) {
        let name = name.trim().to_string();
        let exe = exe.trim().to_string();
        if name.is_empty() || exe.is_empty() {
            self.notify_error(
                win,
                "Game",
                &UserError::from_connection(
                    ConnectionError::Unknown,
                    "Choose the game's file with Browse, and enter a name.",
                ),
            );
            return;
        }
        let edit_id = self.game_dialog_edit_id.borrow().clone();
        let profile_id = self.game_dialog_profile_id.borrow().clone();
        let adding = edit_id.is_none();
        let label = if adding {
            "Adding game…"
        } else {
            "Saving game…"
        };
        self.run_generic_job(
            win,
            JobKind::GameOp,
            label,
            move |api| {
                let result = if let Some(id) = &edit_id {
                    api.update_game(id, &name, &exe, profile_id.clone())
                } else {
                    api.add_game(&name, &exe, profile_id)
                };
                JobResult::Text(result.map(|g| g.name))
            },
            move |ctx, w, result| {
                let JobResult::Text(result) = result else {
                    return;
                };
                match result {
                    Ok(saved_name) => {
                        w.set_game_dialog_open(false);
                        ctx.refresh_games(w);
                        ctx.refresh_profiles(w);
                        let body = if adding {
                            format!("{saved_name} added.")
                        } else {
                            format!("{saved_name} saved.")
                        };
                        ctx.notify_success(
                            w,
                            "Game",
                            if adding { "Game added" } else { "Game saved" },
                            &body,
                        );
                    }
                    Err(e) => ctx.notify_error(w, "Game", &e),
                }
            },
        );
    }

    /// Remove a manually added game on a [`JobKind::GameOp`] job
    /// thread. Refuses while RSH isn't running, refuses an unknown
    /// id, and refuses a running game (stop it first) — all with a
    /// plain message, never a silent no-op. Success refreshes the
    /// tiles.
    fn remove_game(&self, win: &MainWindow, id: String) {
        if !self.is_live() {
            self.notify_toast(
                win,
                "Games",
                "RSH isn't running",
                "RSH isn't running — reconnect, then remove the game.",
            );
            return;
        }
        let Some(game) = self
            .games_cache
            .borrow()
            .iter()
            .find(|g| g.id == id)
            .cloned()
        else {
            self.notify_toast(
                win,
                "Game",
                "Game not found",
                "That game isn't in the list any more.",
            );
            return;
        };
        if game.running {
            self.notify_toast(
                win,
                "Game",
                "Game is running",
                &format!("Stop {} first, then remove it.", game.name),
            );
            return;
        }
        let name = game.name.clone();
        self.run_generic_job(
            win,
            JobKind::GameOp,
            "Removing game…",
            move |api| JobResult::Bool(api.remove_game(&id)),
            move |ctx, w, result| {
                let JobResult::Bool(result) = result else {
                    return;
                };
                match result {
                    Ok(true) => {
                        ctx.refresh_games(w);
                        ctx.notify_success(w, "Game", "Game removed", &format!("{name} removed."));
                    }
                    Ok(false) => {
                        ctx.refresh_games(w);
                        ctx.notify_toast(
                            w,
                            "Game",
                            "Game not found",
                            "That game isn't in the list any more.",
                        );
                    }
                    Err(e) => ctx.notify_error(w, "Game", &e),
                }
            },
        );
    }

    /// Sync the diagnostics drawer props (v2.0.1 hotfix): connection
    /// label, background-service liveness, app version, firmware
    /// version, live charts from the ring buffers, measured stats
    /// from `diag_stats()` ("—" + "No telemetry yet" while the
    /// daemon does not measure, A20/A21), plus status lines (and
    /// recent notifications) as the drawer's event log.
    /// Connection lines for the diagnostics event log (D16):
    /// the live connection address + state + the Wi-Fi network
    /// the daemon found, so the drawer can diagnose a wheel that
    /// will not connect. Split out of [`Self::sync_diag`] to
    /// keep it under the line limit.
    fn push_connection_diag_lines(&self, lines: &mut Vec<SharedString>) {
        let conn = self.api().connection();
        lines.push(format!("Connection: {} ({})", conn.address, conn.label).into());
        if let Some(ssid) = self.api().wifi_status().ssid {
            lines.push(format!("Wi-Fi: {ssid}").into());
        }
    }

    /// Frame + paint p50/p99 cells for the diagnostics drawer
    /// (DS-82 frame time, R2-46 paint completion). "—" until
    /// measured — never a fake 0; the paint cells stay "—" on a
    /// backend without render-notifier support. Split out of
    /// [`Self::sync_diag`] to keep it under the line limit.
    fn sync_diag_frame_times(&self, win: &MainWindow) {
        // DS-82: frame-time histogram (this UI, last 30 s). Perf
        // (F-4): change-guarded like the rest of `sync_diag` — the
        // old unconditional sets marked the props dirty every
        // second even when the p50/p99 had not moved.
        let (frame_p50, frame_p99) = self.frame_time_p50_p99().unzip();
        set_if_changed(
            win.get_diag_frame_p50_text(),
            frame_p50
                .map_or_else(|| "—".to_string(), |v| format!("{v:.1} ms"))
                .into(),
            |v| win.set_diag_frame_p50_text(v),
        );
        set_if_changed(
            win.get_diag_frame_p99_text(),
            frame_p99
                .map_or_else(|| "—".to_string(), |v| format!("{v:.1} ms"))
                .into(),
            |v| win.set_diag_frame_p99_text(v),
        );
        // R2-46: paint-completion p50/p99 (refresh start to
        // `AfterRendering`).
        let (paint_p50, paint_p99) = self.frame_paint_p50_p99().unzip();
        set_if_changed(
            win.get_diag_paint_p50_text(),
            paint_p50
                .map_or_else(|| "—".to_string(), |v| format!("{v:.1} ms"))
                .into(),
            |v| win.set_diag_paint_p50_text(v),
        );
        set_if_changed(
            win.get_diag_paint_p99_text(),
            paint_p99
                .map_or_else(|| "—".to_string(), |v| format!("{v:.1} ms"))
                .into(),
            |v| win.set_diag_paint_p99_text(v),
        );
    }

    /// R2-11: every scalar write is change-guarded like `push_frame`,
    /// and the button-grid / event-log models are only rebuilt when
    /// their content hash moved.
    #[allow(clippy::too_many_lines)]
    fn sync_diag(&self, win: &MainWindow) {
        let api = self.api();
        let conn = api.connection();
        let version = api.version();
        // H5: "Unknown" renders as "—" (never a hardcoded version).
        let firmware = api.firmware_version();
        let firmware = if firmware == "Unknown" {
            "—".to_string()
        } else {
            firmware
        };
        set_if_changed(win.get_diag_conn_label(), conn.label.clone().into(), |v| {
            win.set_diag_conn_label(v);
        });
        set_if_changed(win.get_diag_daemon_live(), self.is_live(), |v| {
            win.set_diag_daemon_live(v);
        });
        set_if_changed(win.get_diag_app_version(), version.clone().into(), |v| {
            win.set_diag_app_version(v);
        });
        set_if_changed(win.get_diag_firmware(), firmware.clone().into(), |v| {
            win.set_diag_firmware(v);
        });
        // A20: measured stats, or "—" (the drawer shows
        // "No telemetry yet" while these are "—").
        if let Ok(stats) = api.diag_stats() {
            {
                set_if_changed(
                    win.get_diag_delay_text(),
                    stats
                        .e2e_ms
                        .map_or_else(|| "—".to_string(), |v| format!("{v:.0} ms"))
                        .into(),
                    |v| win.set_diag_delay_text(v),
                );
                set_if_changed(
                    win.get_diag_jitter_text(),
                    stats
                        .jitter_ms_p99
                        .map_or_else(|| "—".to_string(), |v| format!("{v:.0} ms"))
                        .into(),
                    |v| win.set_diag_jitter_text(v),
                );
                set_if_changed(
                    win.get_diag_lost_text(),
                    format!("{:.1}%", stats.loss_pct).into(),
                    |v| win.set_diag_lost_text(v),
                );
                set_if_changed(
                    win.get_diag_loop_rate_text(),
                    stats
                        .loop_hz
                        .map_or_else(|| "—".to_string(), |hz| format!("{hz:.0} Hz"))
                        .into(),
                    |v| win.set_diag_loop_rate_text(v),
                );
                set_if_changed(
                    win.get_diag_tx_failures_text(),
                    stats.tx_fail.to_string().into(),
                    |v| win.set_diag_tx_failures_text(v),
                );
                set_if_changed(
                    win.get_diag_seq_gaps_text(),
                    stats.seq_gap.to_string().into(),
                    |v| win.set_diag_seq_gaps_text(v),
                );
                // DS-82: IPC dispatch latency (the daemon, all
                // methods). "—" until measured — never a fake 0.
                set_if_changed(
                    win.get_diag_ipc_p50_text(),
                    stats
                        .ipc_p50_ms
                        .map_or_else(|| "—".to_string(), |v| format!("{v:.2} ms"))
                        .into(),
                    |v| win.set_diag_ipc_p50_text(v),
                );
                set_if_changed(
                    win.get_diag_ipc_p99_text(),
                    stats
                        .ipc_p99_ms
                        .map_or_else(|| "—".to_string(), |v| format!("{v:.2} ms"))
                        .into(),
                    |v| win.set_diag_ipc_p99_text(v),
                );
            }
        } else {
            set_if_changed(win.get_diag_delay_text(), "—".into(), |v| {
                win.set_diag_delay_text(v);
            });
            set_if_changed(win.get_diag_jitter_text(), "—".into(), |v| {
                win.set_diag_jitter_text(v);
            });
            set_if_changed(win.get_diag_lost_text(), "—".into(), |v| {
                win.set_diag_lost_text(v);
            });
            set_if_changed(win.get_diag_loop_rate_text(), "—".into(), |v| {
                win.set_diag_loop_rate_text(v);
            });
            set_if_changed(win.get_diag_tx_failures_text(), "—".into(), |v| {
                win.set_diag_tx_failures_text(v);
            });
            set_if_changed(win.get_diag_seq_gaps_text(), "—".into(), |v| {
                win.set_diag_seq_gaps_text(v);
            });
            set_if_changed(win.get_diag_ipc_p50_text(), "—".into(), |v| {
                win.set_diag_ipc_p50_text(v);
            });
            set_if_changed(win.get_diag_ipc_p99_text(), "—".into(), |v| {
                win.set_diag_ipc_p99_text(v);
            });
        }
        // DS-82 + R2-46: frame and paint histograms (this UI,
        // last 30 s) — local, so they are set regardless of
        // whether the daemon answered `diag_stats` above.
        self.sync_diag_frame_times(win);
        // H10: the live button grid shows the real pressed buttons
        // (bits 1–12 of the snapshot mask) — empty when not live, so
        // the section hides instead of rendering 12 dead tiles.
        let pressed: Vec<bool> = if self.is_live() {
            let mask = self.api().snapshot().button_mask;
            (0..12).map(|i| mask & (1_u64 << i) != 0).collect()
        } else {
            Vec::new()
        };
        let buttons_hash = content_hash(&pressed);
        if self.diag_buttons_hash.get() != Some(buttons_hash) {
            self.diag_buttons_hash.set(Some(buttons_hash));
            win.set_diag_buttons_pressed(ModelRc::new(Rc::new(VecModel::from(pressed))));
        }
        let mut lines: Vec<SharedString> = vec![
            format!("App {version}").into(),
            format!(
                "RSH: {}",
                if self.is_live() {
                    "running"
                } else {
                    "not running"
                }
            )
            .into(),
            format!("Connection: {}", conn.label).into(),
            format!("Firmware: {firmware}").into(),
        ];
        self.push_connection_diag_lines(&mut lines);
        // H4 (v2.0.8 retest Bug 1): the event log used to show only
        // the notification title — a failed switch read "Couldn't
        // complete that" with no cause. The body carries the full
        // reason (e.g. the sidecar hello verdict), so include it;
        // the lines wrap.
        for n in self.notifications.borrow().to_model_items().iter().take(6) {
            lines.push(format!("{} {} — {}", n.time, n.title, n.body).into());
        }
        let events_hash = content_hash(
            &lines
                .iter()
                .map(SharedString::as_str)
                .collect::<Vec<_>>()
                .join("\n"),
        );
        if self.diag_events_hash.get() != Some(events_hash) {
            self.diag_events_hash.set(Some(events_hash));
            win.set_diag_events(ModelRc::new(Rc::new(VecModel::from(lines))));
        }
    }

    /// Sync the wizard Drivers step (vJoy) from `vjoy_status()`:
    /// installed / installing / version / last install error are all
    /// real. When the call errors (sim / no daemon) the status is
    /// honestly unknown — never a fake "installed" — and the
    /// `HIDMaestro` fallback stays the way forward. The user's
    /// fallback choice is preserved across calls.
    /// R2-11: every write is change-guarded.
    fn sync_vjoy(&self, win: &MainWindow) {
        if let Ok(status) = self.api().vjoy_status() {
            set_if_changed(win.get_setup_vjoy_installed(), status.installed, |v| {
                win.set_setup_vjoy_installed(v);
            });
            set_if_changed(win.get_setup_vjoy_installing(), status.installing, |v| {
                win.set_setup_vjoy_installing(v);
            });
            set_if_changed(
                win.get_setup_vjoy_version(),
                status.version.unwrap_or_default().into(),
                |v| win.set_setup_vjoy_version(v),
            );
            // R2-40: never clobber a failure the install job already
            // reported with the daemon's (possibly empty)
            // `install_error` — the 1 Hz tick used to wipe the
            // message within a second of the failure, leaving the
            // Drivers step on "Not installed" with no reason and no
            // prompt to pick the fallback. A daemon-reported error
            // replaces it; a success / new install clears it.
            if status.installed || status.installing {
                set_if_changed(win.get_setup_vjoy_error(), "".into(), |v| {
                    win.set_setup_vjoy_error(v);
                });
            } else if let Some(error) = status.install_error {
                set_if_changed(win.get_setup_vjoy_error(), error.into(), |v| {
                    win.set_setup_vjoy_error(v);
                });
            }
        } else {
            set_if_changed(win.get_setup_vjoy_installed(), false, |v| {
                win.set_setup_vjoy_installed(v);
            });
            set_if_changed(win.get_setup_vjoy_installing(), false, |v| {
                win.set_setup_vjoy_installing(v);
            });
            set_if_changed(win.get_setup_vjoy_version(), "".into(), |v| {
                win.set_setup_vjoy_version(v);
            });
            set_if_changed(
                win.get_setup_vjoy_error(),
                "Driver status unavailable — use HIDMaestro instead.".into(),
                |v| win.set_setup_vjoy_error(v),
            );
        }
        // D16: the Drivers step shows indeterminate progress +
        // elapsed while an install runs (the daemon reports no %).
        let elapsed = self
            .vjoy_install_started
            .get()
            .map(|t| {
                let s = t.elapsed().as_secs();
                format!("{}:{:02}", s / 60, s % 60)
            })
            .unwrap_or_default();
        set_if_changed(win.get_setup_vjoy_elapsed(), elapsed.into(), |v| {
            win.set_setup_vjoy_elapsed(v);
        });
        set_if_changed(win.get_setup_vjoy_progress(), -1, |v| {
            win.set_setup_vjoy_progress(v);
        });
    }

    /// Path of the first-run marker (`<data_dir>/setup-done`).
    fn setup_marker_path(&self) -> Option<PathBuf> {
        self.data_dir.as_ref().map(|d| d.join("setup-done"))
    }

    /// Read the setup marker. No data dir means no marker can be
    /// read — TR §3 edge: that must NOT count as done, or the wizard
    /// could never show on a machine without a data dir.
    fn read_setup_marker(&self) -> SetupMarker {
        let Some(path) = self.setup_marker_path() else {
            return SetupMarker::Missing;
        };
        read_setup_marker_file(&path)
    }

    /// True when the wizard should show on this launch (F23).
    /// Replaces the old `is_setup_done` boolean: a bare "done" file
    /// can no longer tell a Skip from a Finish, so the decision
    /// needs the marker state + the current version (see
    /// [`setup_marker_should_show`]).
    fn setup_should_show(&self) -> bool {
        setup_marker_should_show(&self.read_setup_marker(), app_version())
    }

    /// Record that setup finished, so the wizard never shows again
    /// on this machine (any version — a finished setup stays done).
    fn mark_setup_done(&self) {
        self.write_setup_marker("done");
    }

    /// Record that setup was SKIPPED (F23): unlike Finish, a skip
    /// re-offers the wizard once on the next version, so a skipped
    /// setup (e.g. the vJoy Drivers step never seen) is not lost
    /// forever on an update install.
    fn mark_setup_skipped(&self) {
        self.write_setup_marker("skipped");
    }

    /// Write a versioned marker (`<state> <app version>`).
    fn write_setup_marker(&self, state: &str) {
        let Some(path) = self.setup_marker_path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                return;
            }
        }
        match std::fs::write(&path, format!("{state} {}\n", app_version())) {
            Ok(()) => ui_log!("RSH-UI setup marker written: {}", path.display()),
            Err(e) => ui_log!("RSH-UI setup marker write failed: {e}"),
        }
    }

    /// Delete the first-run marker so "Run setup again" (A18) shows
    /// the wizard from step 0.
    fn clear_setup_marker(&self) {
        if let Some(path) = self.setup_marker_path() {
            if let Err(e) = std::fs::remove_file(&path) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    ui_log!("RSH-UI setup marker delete failed: {e}");
                }
            }
        }
    }
}

/// F23 — setup-marker states (file `<data_dir>/setup-done`).
///
/// v2.0.2 and earlier wrote the bare word `done` for BOTH Finish
/// and Skip, so an update install could never tell them apart: a
/// user who skipped in v2.0.1 never saw the wizard again in v2.0.2,
/// and with it never got the vJoy Drivers step. v2.0.3+ markers are
/// versioned (`done <version>` / `skipped <version>`).
#[derive(Debug, Clone, PartialEq, Eq)]
enum SetupMarker {
    /// No marker file (fresh profile, or no data dir).
    Missing,
    /// Setup finished on the recorded version — never re-offer.
    Done(String),
    /// Setup skipped on the recorded version — re-offer once the
    /// app version changes, then the new choice is recorded.
    Skipped(String),
    /// Pre-v2.0.3 bare `done` (or unreadable/corrupt content):
    /// Skip vs Finish is unknowable, so the wizard is offered once
    /// and the user's new choice writes a versioned marker.
    Legacy,
}

/// Parse marker file content into a [`SetupMarker`].
fn parse_setup_marker(content: &str) -> SetupMarker {
    let mut parts = content.split_whitespace();
    match (parts.next(), parts.next()) {
        (Some("done"), Some(version)) => SetupMarker::Done(version.to_string()),
        (Some("skipped"), Some(version)) => SetupMarker::Skipped(version.to_string()),
        _ => SetupMarker::Legacy,
    }
}

/// Read a marker file: missing/unreadable file is [`SetupMarker::Missing`].
fn read_setup_marker_file(path: &std::path::Path) -> SetupMarker {
    match std::fs::read_to_string(path) {
        Ok(content) => parse_setup_marker(&content),
        Err(_) => SetupMarker::Missing,
    }
}

/// Should the wizard show on this launch for this marker state?
/// `current_version` is [`app_version()`].
fn setup_marker_should_show(marker: &SetupMarker, current_version: &str) -> bool {
    match marker {
        SetupMarker::Done(_) => false,
        SetupMarker::Skipped(version) => version != current_version,
        SetupMarker::Missing | SetupMarker::Legacy => true,
    }
}

impl Ctx {
    /// Profile row by list index (popover + Settings share the same
    /// `list_profiles()` order; presets are always empty, so the
    /// popover's selected index maps directly).
    fn profile_at(&self, index: i32) -> Option<ProfileSummary> {
        let idx = usize::try_from(index).ok()?;
        self.profiles_cache.borrow().get(idx).cloned()
    }

    /// Request activating a profile (A8): with unsaved changes the
    /// popover's Save/Discard prompt resolves the pending switch
    /// instead of silently discarding.
    fn request_activate(&self, win: &MainWindow, index: i32) {
        let Some(p) = self.profile_at(index) else {
            return;
        };
        if p.active {
            self.notify(
                win,
                "Profile",
                "Profile already active",
                &format!("{} is already active.", p.name),
            );
            return;
        }
        if self.unsaved.get() {
            *self.pending_profile_id.borrow_mut() = Some(p.id.clone());
            win.set_popover_pending_name(p.name.clone().into());
            win.set_popover_show_prompt(true);
        } else {
            self.do_activate(win, &p);
        }
    }

    /// Perform the activation (after any save/discard prompt).
    /// F6: `activate_profile` runs on a [`JobKind::ProfileOp`]
    /// job thread — the click never blocks on the daemon.
    fn do_activate(&self, win: &MainWindow, p: &ProfileSummary) {
        let id = p.id.clone();
        self.run_generic_job(
            win,
            JobKind::ProfileOp,
            "Switching profile…",
            move |api| JobResult::Text(api.activate_profile(&id)),
            move |ctx, win, result| {
                let JobResult::Text(result) = result else {
                    return;
                };
                match result {
                    Ok(name) => {
                        ctx.unsaved.set(false);
                        ctx.refresh_profiles(win);
                        ctx.refresh_games(win);
                        win.set_popover_open(false);
                        win.set_popover_show_prompt(false);
                        *ctx.pending_profile_id.borrow_mut() = None;
                        ctx.notify_success(
                            win,
                            "Profile",
                            "Profile active",
                            &format!("{name} is now active."),
                        );
                    }
                    Err(e) => {
                        ctx.notify_error(win, "Profile", &e);
                        ui_log!("RSH-UI activate_profile failed: {} ({})", e.code, e.title);
                    }
                }
            },
        );
    }

    /// Resolve the pending profile switch after the prompt.
    /// F6: the save/discard runs on a [`JobKind::ProfileOp`] job
    /// thread; the activation chains from the apply (it starts its
    /// own job).
    fn resolve_pending_switch(&self, win: &MainWindow, save_first: bool) {
        let pending = self.pending_profile_id.borrow().clone();
        let Some(id) = pending else {
            win.set_popover_show_prompt(false);
            return;
        };
        if save_first {
            let activate_id = id.clone();
            self.run_generic_job(
                win,
                JobKind::ProfileOp,
                "Saving changes…",
                move |api| JobResult::Bool(api.save_draft()),
                move |ctx, win, result| {
                    let JobResult::Bool(result) = result else {
                        return;
                    };
                    match result {
                        Ok(_) => {
                            ctx.unsaved.set(false);
                        }
                        Err(e) => {
                            ctx.notify_error(win, "Profile", &e);
                            win.set_popover_show_prompt(false);
                            return;
                        }
                    }
                    win.set_popover_show_prompt(false);
                    *ctx.pending_profile_id.borrow_mut() = None;
                    if let Some(p) = ctx
                        .profiles_cache
                        .borrow()
                        .iter()
                        .find(|p| p.id == activate_id)
                        .cloned()
                    {
                        ctx.do_activate(win, &p);
                    }
                },
            );
        } else {
            let activate_id = id.clone();
            self.run_generic_job(
                win,
                JobKind::ProfileOp,
                "Discarding changes…",
                move |api| JobResult::Bool(Ok(api.discard_draft())),
                move |ctx, win, result| {
                    let JobResult::Bool(result) = result else {
                        return;
                    };
                    if result.unwrap_or(false) {
                        ctx.unsaved.set(false);
                    }
                    win.set_popover_show_prompt(false);
                    *ctx.pending_profile_id.borrow_mut() = None;
                    if let Some(p) = ctx
                        .profiles_cache
                        .borrow()
                        .iter()
                        .find(|p| p.id == activate_id)
                        .cloned()
                    {
                        ctx.do_activate(win, &p);
                    }
                },
            );
        }
    }

    /// Duplicate the profile at `index` (A8).
    /// F6: `duplicate_profile` runs on a [`JobKind::ProfileOp`]
    /// job thread.
    fn duplicate_profile_index(&self, win: &MainWindow, index: i32) {
        let Some(p) = self.profile_at(index) else {
            return;
        };
        let id = p.id.clone();
        self.run_generic_job(
            win,
            JobKind::ProfileOp,
            "Duplicating profile…",
            move |api| JobResult::Text(api.duplicate_profile(&id)),
            move |ctx, win, result| {
                let JobResult::Text(result) = result else {
                    return;
                };
                match result {
                    Ok(name) => {
                        ctx.refresh_profiles(win);
                        ctx.notify_success(
                            win,
                            "Profile",
                            "Profile duplicated",
                            &format!("Duplicated as {name}."),
                        );
                    }
                    Err(e) => ctx.notify_error(win, "Profile", &e),
                }
            },
        );
    }

    /// Delete the profile at `index` (A8).
    /// F6: `delete_profile` runs on a [`JobKind::ProfileOp`] job
    /// thread.
    fn delete_profile_index(&self, win: &MainWindow, index: i32) {
        let Some(p) = self.profile_at(index) else {
            return;
        };
        let id = p.id.clone();
        let name = p.name.clone();
        self.run_generic_job(
            win,
            JobKind::ProfileOp,
            "Deleting profile…",
            move |api| JobResult::Bool(api.delete_profile(&id)),
            move |ctx, win, result| {
                let JobResult::Bool(result) = result else {
                    return;
                };
                match result {
                    Ok(true) => {
                        ctx.refresh_profiles(win);
                        ctx.refresh_games(win);
                        ctx.notify_success(
                            win,
                            "Profile",
                            "Profile deleted",
                            &format!("{name} deleted."),
                        );
                    }
                    Ok(false) => {
                        ctx.notify_toast(
                            win,
                            "Profile",
                            "Couldn't delete profile",
                            &format!("{name} couldn't be deleted."),
                        );
                    }
                    Err(e) => ctx.notify_error(win, "Profile", &e),
                }
            },
        );
    }

    /// Create a new profile with a fresh unique name (A8).
    /// F6: `create_profile` runs on a [`JobKind::ProfileOp`] job
    /// thread.
    fn create_new_profile(&self, win: &MainWindow) {
        let existing: Vec<String> = self
            .profiles_cache
            .borrow()
            .iter()
            .map(|p| p.name.clone())
            .collect();
        let mut n = existing.len() + 1;
        let name = loop {
            let candidate = format!("Profile {n}");
            if !existing.iter().any(|e| e == &candidate) {
                break candidate;
            }
            n += 1;
        };
        self.run_generic_job(
            win,
            JobKind::ProfileOp,
            "Creating profile…",
            move |api| JobResult::Text(api.create_profile(&name)),
            move |ctx, win, result| {
                let JobResult::Text(result) = result else {
                    return;
                };
                match result {
                    Ok(created) => {
                        ctx.refresh_profiles(win);
                        ctx.notify_success(
                            win,
                            "Profile",
                            "Profile created",
                            &format!("{created} created."),
                        );
                    }
                    Err(e) => ctx.notify_error(win, "Profile", &e),
                }
            },
        );
    }

    /// Convert a popover footer index to a my-profiles index (M1):
    /// the popover's footer buttons pass a COMBINED index (preset
    /// rows first, then my profiles). `None` means the row is a
    /// preset (read-only) or out of range.
    fn popover_my_index(&self, combined: i32) -> Option<i32> {
        let presets = i32::try_from(self.presets_count.get()).unwrap_or(0);
        let my = combined.checked_sub(presets).filter(|m| *m >= 0)?;
        let idx = usize::try_from(my).ok()?;
        if idx >= self.profiles_cache.borrow().len() {
            None
        } else {
            Some(my)
        }
    }

    /// Use a preset (M1): presets are one row per catalogue game
    /// (read-only templates) — "Use" creates a profile from the
    /// game (name = game name, "Name 2" when taken) and activates
    /// it, so the template becomes a real, editable profile.
    fn use_preset(&self, win: &MainWindow, preset_index: i32) {
        let games = {
            let cached = self.games_cache.borrow();
            if cached.is_empty() {
                drop(cached);
                self.api().list_games()
            } else {
                cached.clone()
            }
        };
        let Some(game) = games.get(usize::try_from(preset_index).unwrap_or(usize::MAX)) else {
            return;
        };
        let existing: Vec<String> = self
            .profiles_cache
            .borrow()
            .iter()
            .map(|p| p.name.clone())
            .collect();
        let mut name = game.name.clone();
        let mut n = 2;
        while existing.iter().any(|e| e == &name) {
            name = format!("{} {n}", game.name);
            n += 1;
        }
        let game_name = game.name.clone();
        self.run_generic_job(
            win,
            JobKind::ProfileOp,
            "Creating profile…",
            move |api| JobResult::Text(api.create_profile(&name)),
            move |ctx, win, result| {
                let JobResult::Text(result) = result else {
                    return;
                };
                match result {
                    Ok(created) => {
                        ctx.refresh_profiles(win);
                        if let Some(p) = ctx
                            .profiles_cache
                            .borrow()
                            .iter()
                            .find(|p| p.name == created)
                            .cloned()
                        {
                            ctx.do_activate(win, &p);
                        }
                        ctx.notify_success(
                            win,
                            "Profile",
                            "Profile created",
                            &format!("{created} created from the {game_name} preset."),
                        );
                    }
                    Err(e) => ctx.notify_error(win, "Profile", &e),
                }
            },
        );
    }

    /// Rename the profile at a popover COMBINED index (M1): the new
    /// name is the profile's linked game — the old no-text-input
    /// path, kept for `popover-rename(int)`. Free-text rename lives
    /// in [`Ctx::rename_profile_free_text`] (the popover's
    /// `rename-text` input + `popover-rename-confirmed`). A preset
    /// row or an unlinked profile explains itself instead of
    /// pretending to rename.
    fn rename_profile_index(&self, win: &MainWindow, combined: i32) {
        let Some(index) = self.popover_my_index(combined) else {
            self.notify(
                win,
                "Profile",
                "Presets are read-only",
                "Presets are read-only — duplicate one to edit it.",
            );
            return;
        };
        let Some(p) = self.profile_at(index) else {
            return;
        };
        let Some(slug) = p.game.clone() else {
            self.notify(
                win,
                "Profile",
                "Rename",
                "Link this profile to a game first — Rename names it after its game.",
            );
            return;
        };
        let game_name = self
            .games_cache
            .borrow()
            .iter()
            .find(|g| g.id == slug)
            .map_or_else(|| pretty_game_slug(&slug), |g| g.name.clone());
        if p.name == game_name {
            self.notify(
                win,
                "Profile",
                "Rename",
                &format!("{game_name} is already named after its game."),
            );
            return;
        }
        let id = p.id.clone();
        self.run_generic_job(
            win,
            JobKind::ProfileOp,
            "Renaming profile…",
            move |api| JobResult::Text(api.rename_profile(&id, &game_name)),
            move |ctx, win, result| {
                let JobResult::Text(result) = result else {
                    return;
                };
                match result {
                    Ok(name) => {
                        ctx.refresh_profiles(win);
                        ctx.notify_success(
                            win,
                            "Profile",
                            "Profile renamed",
                            &format!("Renamed to {name}."),
                        );
                    }
                    Err(e) => ctx.notify_error(win, "Profile", &e),
                }
            },
        );
    }

    /// Rename the profile at a popover COMBINED index to a typed
    /// name (M1, FC2): the `popover-rename-confirmed` callback
    /// carries the row + the typed name. Unlike the old
    /// `popover-rename(int)` path (rename to the linked game's
    /// name, kept as-is), this needs no game link — any profile
    /// renames to any typed name. A preset row explains itself; an
    /// empty name asks for one; the backend validates the rest
    /// (length / characters) and a failure notifies instead of
    /// failing silently.
    fn rename_profile_free_text(&self, win: &MainWindow, combined: i32, name: &str) {
        let Some(index) = self.popover_my_index(combined) else {
            self.notify(
                win,
                "Profile",
                "Presets are read-only",
                "Presets are read-only — duplicate one to edit it.",
            );
            return;
        };
        let Some(p) = self.profile_at(index) else {
            return;
        };
        let Some(name) = clean_profile_name(name) else {
            self.notify(win, "Profile", "Rename", "Enter a name first.");
            return;
        };
        if p.name == name {
            self.notify(
                win,
                "Profile",
                "Rename",
                &format!("{name} is already the profile name."),
            );
            return;
        }
        let id = p.id.clone();
        self.run_generic_job(
            win,
            JobKind::ProfileOp,
            "Renaming profile…",
            move |api| JobResult::Text(api.rename_profile(&id, &name)),
            move |ctx, win, result| {
                let JobResult::Text(result) = result else {
                    return;
                };
                match result {
                    Ok(renamed) => {
                        ctx.refresh_profiles(win);
                        ctx.notify_success(
                            win,
                            "Profile",
                            "Profile renamed",
                            &format!("Renamed to {renamed}."),
                        );
                    }
                    Err(e) => ctx.notify_error(win, "Profile", &e),
                }
            },
        );
    }

    /// Link the profile at a popover COMBINED index to its next game
    /// (M1): each click cycles to the next catalogue game (the
    /// popover has no game picker — cycling is the link action).
    /// A preset row explains itself instead of linking.
    fn link_profile_index(&self, win: &MainWindow, combined: i32) {
        let Some(index) = self.popover_my_index(combined) else {
            self.notify(
                win,
                "Profile",
                "Presets are read-only",
                "Presets are read-only — duplicate one to edit it.",
            );
            return;
        };
        let Some(p) = self.profile_at(index) else {
            return;
        };
        let games = {
            let cached = self.games_cache.borrow();
            if cached.is_empty() {
                drop(cached);
                self.api().list_games()
            } else {
                cached.clone()
            }
        };
        if games.is_empty() {
            self.notify(
                win,
                "Profile",
                "Link to game",
                "No games added yet — add a game from the Overview page.",
            );
            return;
        }
        let current = p.game.as_deref();
        let pos = current.and_then(|slug| games.iter().position(|g| g.id == slug));
        let next = games[pos.map_or(0, |i| i + 1) % games.len()].clone();
        let id = p.id.clone();
        let next_id = next.id.clone();
        let next_name = next.name.clone();
        self.run_generic_job(
            win,
            JobKind::ProfileOp,
            "Linking profile…",
            move |api| JobResult::Text(api.link_profile(&id, &next_id)),
            move |ctx, win, result| {
                let JobResult::Text(result) = result else {
                    return;
                };
                match result {
                    Ok(name) => {
                        ctx.refresh_profiles(win);
                        ctx.refresh_games(win);
                        ctx.notify_success(
                            win,
                            "Profile",
                            "Profile linked",
                            &format!("{name} is now linked to {next_name}."),
                        );
                    }
                    Err(e) => ctx.notify_error(win, "Profile", &e),
                }
            },
        );
    }

    /// Poll a running learn (Buttons / horn) on a job thread (F6):
    /// `take_learned_button` / `is_learning` are IPC calls — the
    /// 1 Hz tick never blocks on them. A learned button applies on
    /// the UI thread; a horn learn chains a [`JobKind::TuningPush`]
    /// job to persist it. Still listening = no-op (the next tick
    /// polls again); ended-with-nothing clears the listening state.
    fn start_learn_poll_job(&self, win: &MainWindow) {
        let target = self.learn_target.get();
        if target == 0 {
            return;
        }
        self.run_generic_job(
            win,
            JobKind::LearnOp,
            "Listening…",
            move |api| {
                if let Some(b) = api.take_learned_button() {
                    JobResult::LearnPoll(Some(b))
                } else if api.is_learning() {
                    JobResult::Unit(Ok(()))
                } else {
                    JobResult::LearnPoll(None)
                }
            },
            move |ctx, win, result| match result {
                JobResult::LearnPoll(Some(b)) => {
                    ctx.learn_target.set(0);
                    win.set_buttons_listening(false);
                    win.set_horn_learning(false);
                    if target == 2 {
                        ctx.run_generic_job(
                            win,
                            JobKind::TuningPush,
                            "Saving horn button…",
                            move |api| {
                                let mut record = api.get_tuning("horn");
                                record.horn_button = Some(b);
                                JobResult::Tuning(api.set_tuning(&record).map(Box::new))
                            },
                            move |ctx, win, result| {
                                let JobResult::Tuning(result) = result else {
                                    return;
                                };
                                match result {
                                    Ok(_) => {
                                        ctx.unsaved.set(true);
                                        ctx.sync_extras(win);
                                        ctx.notify(
                                            win,
                                            "Horn",
                                            "Horn learned",
                                            &format!("Horn is now Button {b}."),
                                        );
                                    }
                                    Err(e) => ctx.notify(
                                        win,
                                        "Horn",
                                        "Couldn't save horn button",
                                        &e.title,
                                    ),
                                }
                            },
                        );
                    } else {
                        let idx = i32::from(b).saturating_sub(1);
                        if (0..12).contains(&idx) {
                            ctx.selected_button.set(idx);
                            win.set_selected_index(idx);
                            let action = ctx.button_actions.borrow()
                                [usize::try_from(idx).unwrap_or(0)]
                            .clone();
                            *ctx.pending_action.borrow_mut() = None;
                            win.set_selected_action(action.into());
                        }
                        ctx.notify(
                            win,
                            "Buttons",
                            "Button learned",
                            &format!("Learned Button {b}."),
                        );
                    }
                }
                JobResult::LearnPoll(None) => {
                    // R2-35: Learn ended without a press (backend
                    // stopped learning) — say so instead of going
                    // silent; the listening state was the only cue.
                    ctx.learn_target.set(0);
                    win.set_buttons_listening(false);
                    win.set_horn_learning(false);
                    // R2-35: Learn ended with nothing learned — say
                    // so instead of silently clearing the listening
                    // state (the backend stopped learning, e.g. its
                    // confirm window expired).
                    ctx.notify_toast(
                        win,
                        "Learn",
                        "Learning ended",
                        "Learning stopped without a button press — try again if you still need to learn a button.",
                    );
                }
                _ => {}
            },
        );
    }

    /// Start (or cancel) learning for `target` (1 Buttons, 2 horn):
    /// `start_learn` / `cancel_learn` run on a [`JobKind::LearnToggle`]
    /// job thread — a dedicated kind (R2-35), NOT the poll job's
    /// [`JobKind::LearnOp`], so a click while the 1 Hz poll job is in
    /// flight is no longer swallowed by the duplicate-start rule.
    /// The frame timer picks the press up via
    /// [`Ctx::start_learn_poll_job`].
    fn toggle_learn(&self, win: &MainWindow, target: i32) {
        if self.learn_target.get() == target {
            self.run_generic_job(
                win,
                JobKind::LearnToggle,
                "Cancelling…",
                move |api| JobResult::Unit(api.cancel_learn().map(|_| ())),
                move |ctx, win, result| {
                    let JobResult::Unit(result) = result else {
                        return;
                    };
                    match result {
                        Ok(()) => {
                            ctx.learn_target.set(0);
                            win.set_buttons_listening(false);
                            win.set_horn_learning(false);
                            ctx.notify(win, "Learn", "Learning cancelled", "Learning cancelled.");
                        }
                        Err(e) => ctx.notify(win, "Learn", "Couldn't cancel learning", &e.title),
                    }
                },
            );
            return;
        }
        self.run_generic_job(
            win,
            JobKind::LearnToggle,
            "Listening…",
            move |api| JobResult::Bool(api.start_learn()),
            move |ctx, win, result| {
                let JobResult::Bool(result) = result else {
                    return;
                };
                match result {
                    Ok(true) => {
                        ctx.learn_target.set(target);
                        win.set_buttons_listening(target == 1);
                        win.set_horn_learning(target == 2);
                        ctx.notify(
                            win,
                            "Learn",
                            "Listening…",
                            "Listening — press a button on the wheel.",
                        );
                    }
                    Ok(false) => {
                        ctx.notify(
                            win,
                            "Learn",
                            "Couldn't start learning",
                            "Learning didn't start.",
                        );
                    }
                    Err(e) => ctx.notify(win, "Learn", "Couldn't start learning", &e.title),
                }
            },
        );
    }
}

/// Wire all Slint callbacks to the [`AppApi`] implementation.
fn wire_callbacks(win: &MainWindow, ctx: &Rc<Ctx>) {
    wire_tuning(win, ctx);
    wire_actions(win, ctx);
    wire_keyboard(win, ctx);
    wire_notifications(win, ctx);
    wire_setup(win, ctx);
    wire_game_dialog(win, ctx);
    wire_pedals(win, ctx);
    wire_buttons(win, ctx);
    wire_extras(win, ctx);
    wire_theme(win, ctx);
    wire_popover(win, ctx);
    wire_transport(win, ctx);
    wire_fc2_callbacks(win, ctx);
    wire_confirm(win, ctx);
    wire_settings_password(win, ctx);
}

/// Wire the small confirm dialog (D17): Yes runs the pending
/// destructive action (delete profile / remove game); No (or Esc,
/// or backdrop click) cancels it and clears the pending action.
fn wire_confirm(win: &MainWindow, ctx: &Rc<Ctx>) {
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_confirm_yes(move || {
            if let Some(w) = win.upgrade() {
                w.set_confirm_open(false);
                if let Some(action) = ctx.confirm_pending.borrow_mut().take() {
                    match action {
                        ConfirmAction::DeleteProfile(index) => {
                            ctx.delete_profile_index(&w, index);
                        }
                        ConfirmAction::RemoveGame(id) => {
                            ctx.remove_game(&w, id);
                        }
                    }
                }
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_confirm_no(move || {
            if let Some(w) = win.upgrade() {
                w.set_confirm_open(false);
                *ctx.confirm_pending.borrow_mut() = None;
                ui_log!("RSH-UI confirm dialog cancelled");
            }
        });
    }
}

/// Wire the Settings Wi-Fi password card (D19): Set/Change runs
/// the validated password job; Forget clears the PC-side stores
/// (the board side needs the BOOT 10 s hold — the success toast
/// says so).
fn wire_settings_password(win: &MainWindow, ctx: &Rc<Ctx>) {
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_set_wifi_password(move || {
            if let Some(w) = win.upgrade() {
                ctx.start_set_wifi_password_job(&w);
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_forget_wifi_password(move || {
            if let Some(w) = win.upgrade() {
                ctx.start_forget_wifi_password_job(&w);
            }
        });
    }
}

/// Wire the FC2 callbacks (v2.0.2 Slint follow-up): the .slint side
/// already fires these — Rust had no handlers, so the clicks did
/// nothing. Each one lands on a real handler now:
/// - `address-accepted(string)` → validate + persist the board
///   address (Settings field, Enter).
/// - `keep-tray-selected(bool)` → persist keep-in-tray, reverting
///   the switch on failure.
/// - `popover-rename-confirmed(int, string)` → free-text rename.
///   The old `popover-rename(int)` (rename to the linked game's
///   name) stays wired in `wire_popover`.
fn wire_fc2_callbacks(win: &MainWindow, ctx: &Rc<Ctx>) {
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_address_accepted(move |a| {
            if let Some(w) = win.upgrade() {
                ctx.apply_board_address(&w, &a);
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_keep_tray_selected(move |enabled| {
            if let Some(w) = win.upgrade() {
                ctx.apply_keep_tray(&w, enabled);
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_popover_rename_confirmed(move |i, name| {
            if let Some(w) = win.upgrade() {
                ctx.rename_profile_free_text(&w, i, &name);
            }
        });
    }
}

/// Connect the wizard to one transport and drive its props
/// (`setup-transport`; `setup-connect-done` follows from the live
/// connection's reachability on the next frame via `push_frame`).
/// A9: the connect runs on a background job — the click returns
/// immediately; the wizard's Connect step is done when the board
/// answers (reachable — see `push_frame`, R2-19), or the job drain
/// notifies the failure.
fn setup_connect_to(w: &MainWindow, ctx: &Ctx, transport: ApiTransport) {
    w.set_setup_transport(i32::from(transport == ApiTransport::Usb));
    ctx.start_transport_job(w, transport, None);
}

/// Wire the setup-wizard callbacks (v2.0.1 hotfix, D16 4-step).
///
/// Back/Next move through the wizard steps (0..=3: Connect,
/// Drivers, Calibrate, Games); Next on the last step finishes
/// (success toast + `setup-done` marker) and Skip leaves setup
/// (marked skipped, re-offered on the next version). Connect /
/// calibrate / games are wired to the real `AppApi`:
/// `connect-done` comes from the live connection's reachability
/// (D15 `board_reachable`, set at startup + every frame),
/// calibrate needs the calibration backend (disabled + reason
/// while it is missing — never a fake done), games come from
/// `list_games()`.
fn wire_setup(win: &MainWindow, ctx: &Rc<Ctx>) {
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_setup_back(move || {
            if let Some(w) = win.upgrade() {
                // R2-36: debounce — a rapid double-click goes back
                // exactly one step, never two.
                if !ctx.setup_step_allowed() {
                    return;
                }
                let next = (w.get_setup_step() - 1).max(0);
                w.set_setup_step(next);
                ui_log!("RSH-UI wizard step {} -> {next} (Back)", next + 1);
                if next == 1 {
                    ctx.sync_vjoy(&w);
                }
                if next == 2 {
                    // H9: re-entering Calibrate re-probes raw mV.
                    ctx.start_raw_available_job(&w);
                }
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_setup_next(move || {
            if let Some(w) = win.upgrade() {
                // R2-36: debounce — a rapid double-click advances
                // exactly one step, never two.
                if !ctx.setup_step_allowed() {
                    return;
                }
                if w.get_setup_step() >= 3 {
                    // D16: Finish — the wizard is done; the success
                    // toast is the visible confirmation.
                    w.set_setup_mode(false);
                    ctx.mark_setup_done();
                    ctx.notify_success(
                        &w,
                        "Setup",
                        "Setup complete",
                        "Setup complete — your wheel is ready to use.",
                    );
                    ui_log!("RSH-UI setup finished (wizard closed)");
                } else {
                    let next = w.get_setup_step() + 1;
                    w.set_setup_step(next);
                    ui_log!("RSH-UI wizard step {} -> {next} (Next)", next - 1);
                    if next == 1 {
                        ctx.sync_vjoy(&w);
                    }
                    if next == 2 {
                        // H9: entering Calibrate probes raw mV in the
                        // background — Next gates on real data, not
                        // just on the daemon being live.
                        ctx.start_raw_available_job(&w);
                    }
                    if next == 3 {
                        ctx.refresh_games(&w);
                    }
                }
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_setup_skip(move || {
            if let Some(w) = win.upgrade() {
                w.set_setup_mode(false);
                // F23: a skip is NOT a finish — record it as skipped
                // so the wizard is re-offered on the next version.
                ctx.mark_setup_skipped();
                ui_log!("RSH-UI setup skipped (wizard closed)");
            }
        });
    }
    wire_setup_connect(win, ctx);
    wire_setup_calibrate(win, ctx);
    wire_setup_vjoy(win, ctx);
}

/// Wire the wizard's Drivers step (vJoy install / `HIDMaestro`
/// fallback). Install needs the vJoy backend (`install_vjoy` seam):
/// until it lands the button explains itself and the fallback is
/// the way forward — `set_output_backend("HIDMaestro")` is real.
fn wire_setup_vjoy(win: &MainWindow, ctx: &Rc<Ctx>) {
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_setup_install_vjoy(move || {
            if let Some(w) = win.upgrade() {
                // F6: `install_vjoy` runs on a SettingsOp job
                // thread — the click never blocks on the daemon.
                ctx.run_generic_job(
                    &w,
                    JobKind::SettingsOp,
                    "Starting vJoy install…",
                    move |api| JobResult::Text(api.install_vjoy()),
                    move |ctx, w, result| {
                        let JobResult::Text(result) = result else {
                            return;
                        };
                        match result {
                            Ok(_) => {
                                // Background install: the 1 Hz tick polls
                                // vjoy_status() until it lands or fails.
                                // D17: the start itself is not a
                                // notification — the busy strip +
                                // elapsed time are the surface.
                                ctx.vjoy_polling.set(true);
                                ctx.vjoy_install_started.set(Some(Instant::now()));
                                w.set_setup_vjoy_installing(true);
                                w.set_setup_vjoy_error("".into());
                            }
                            Err(e) => {
                                w.set_setup_vjoy_error(user_text(&e.title).into());
                                // D16: install failure toasts + the
                                // inline error above.
                                ctx.notify_error(w, "Drivers", &e);
                                ui_log!("RSH-UI install_vjoy failed: {} ({})", e.code, e.title);
                            }
                        }
                    },
                );
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_setup_use_hidmaestro(move || {
            if let Some(w) = win.upgrade() {
                // A9/B-2/A10: the shared backend-switch job (warn +
                // 75 s bound); the wizard's fallback flag is set in
                // the job drain on success.
                ctx.start_backend_job(&w, 0);
            }
        });
    }
}

/// Wire the wizard's connect / calibrate / games
/// callbacks (split from [`wire_setup`] for readability).
#[allow(clippy::too_many_lines)]
/// Wire the wizard's Connect step (D16): the Wi-Fi card performs
/// the join itself (`start_wifi_join_job` — scan → join
/// `RSH-XXXX`); the USB card probes the cable (`setup_connect_to`).
fn wire_setup_connect(win: &MainWindow, ctx: &Rc<Ctx>) {
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_setup_connect(move |t| {
            let Some(w) = win.upgrade() else {
                return;
            };
            match t.as_str() {
                "usb" => setup_connect_to(&w, &ctx, ApiTransport::Usb),
                "wifi" => ctx.start_wifi_join_job(&w),
                other => ui_log!("RSH-UI setup connect: unknown transport {other}"),
            }
        });
    }
}

/// Wire the manual add/edit/remove game dialog (no auto-scan):
/// `add-game` / `edit-game` open the dialog, `remove-game`
/// deletes, the dialog's save runs the add/update job and its
/// profile button cycles the bound profile like the profile-link
/// control (no picker component exists). The dialog is a
/// `GameDialog` overlay in ui/main.slint, following the profile
/// popover pattern (Rust-owned props, callbacks carry the typed
/// values). The exe path comes from the native Windows file
/// picker (`game-dialog-browse` → [`Ctx::pick_game_exe`], PC-test
/// Bug 6) — the exe field is read-only, never typed; the same
/// Browse button serves Add and Edit.
fn wire_game_dialog(win: &MainWindow, ctx: &Rc<Ctx>) {
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_add_game(move || {
            if let Some(w) = win.upgrade() {
                ctx.open_game_dialog(&w, None);
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_edit_game(move |id| {
            if let Some(w) = win.upgrade() {
                ctx.open_game_dialog(&w, Some(id.to_string()));
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_remove_game(move |id| {
            if let Some(w) = win.upgrade() {
                // D17: removing a game is destructive — confirm
                // first (small dialog, not a silent delete).
                let name = ctx
                    .games_cache
                    .borrow()
                    .iter()
                    .find(|g| id == g.id)
                    .map_or_else(|| "this game".to_string(), |g| g.name.clone());
                ctx.open_confirm(
                    &w,
                    "Remove game",
                    &format!("Remove {name}? You can add it again later."),
                    "Remove",
                    ConfirmAction::RemoveGame(id.to_string()),
                );
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_game_dialog_browse(move || {
            if let Some(w) = win.upgrade() {
                ctx.pick_game_exe(&w);
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_game_dialog_save(move |name, exe| {
            if let Some(w) = win.upgrade() {
                ctx.save_game_dialog(&w, &name, &exe);
            }
        });
    }
    {
        let win = win.as_weak();
        win.unwrap().on_game_dialog_cancel(move || {
            if let Some(w) = win.upgrade() {
                w.set_game_dialog_open(false);
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_game_dialog_cycle_profile(move || {
            if let Some(w) = win.upgrade() {
                ctx.cycle_game_dialog_profile(&w);
            }
        });
    }
}

/// Wire the wizard's Calibrate step (v2.0.2): "centre" captures the
/// live steering centre; "pedals" captures the fully-pressed
/// endpoint of accelerator + brake (the wizard says "press each
/// pedal fully, then set" — the clutch is optional and is set on
/// the Pedals page instead).
#[allow(clippy::too_many_lines)]
fn wire_setup_calibrate(win: &MainWindow, ctx: &Rc<Ctx>) {
    {
        // A17 (v2.0.2): Calibrate calls the real capture backend.
        // "centre" captures the live steering centre; "pedals"
        // captures the fully-pressed endpoint of accelerator +
        // brake (the wizard says "press each pedal fully, then
        // set" — the clutch is optional and is set on the Pedals
        // page instead).
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_setup_calibrate(move |a| {
            let Some(w) = win.upgrade() else {
                return;
            };
            match a.as_str() {
                "centre" => {
                    // F6: the capture runs on a CaptureOp job
                    // thread — the click never blocks on the daemon.
                    if ctx.is_live() {
                        ctx.run_generic_job(
                            &w,
                            JobKind::CaptureOp,
                            "Setting centre…",
                            move |api| JobResult::Capture(api.capture_centre()),
                            move |ctx, w, result| {
                                let JobResult::Capture(result) = result else {
                                    return;
                                };
                                match result {
                                    Ok(mv) => {
                                        w.set_setup_centre_done(true);
                                        ctx.refresh_calibration();
                                        ctx.notify(
                                            w,
                                            "Calibration",
                                            "Centre set",
                                            &format!("Centre set at {mv} mV."),
                                        );
                                    }
                                    Err(e) => {
                                        ctx.notify_toast(w, "Calibration", "Couldn't set centre", &e.title);
                                        ui_log!("RSH-UI setup calibrate centre failed: {}", e.title);
                                    }
                                }
                            },
                        );
                    } else {
                        let reason = Ctx::capture_unavailable_reason();
                        ctx.notify_toast(&w, "Calibration", "Couldn't set centre", reason);
                        ui_log!("RSH-UI setup calibrate centre failed: {reason}");
                    }
                }
                "pedals" => {
                    // F6: the two captures run as chained CaptureOp
                    // jobs (brake starts when the accelerator lands)
                    // — the click never blocks on the daemon.
                    if ctx.is_live() {
                        ctx.run_generic_job(
                            &w,
                            JobKind::CaptureOp,
                            "Setting pedals…",
                            move |api| JobResult::Capture(api.capture_pedal_max("accel")),
                            move |ctx, w, result| {
                                let JobResult::Capture(result) = result else {
                                    return;
                                };
                                match result {
                                    Ok(accel_mv) => {
                                        ctx.run_generic_job(
                                            w,
                                            JobKind::CaptureOp,
                                            "Setting pedals…",
                                            move |api| {
                                                JobResult::Capture(api.capture_pedal_max("brake"))
                                            },
                                            move |ctx, w, result| {
                                                let JobResult::Capture(result) = result else {
                                                    return;
                                                };
                                                match result {
                                                    Ok(brake_mv) => {
                                                        w.set_setup_pedals_done(true);
                                                        ctx.refresh_calibration();
                                                        ctx.notify(
                                                            w,
                                                            "Calibration",
                                                            "Pedals set",
                                                            &format!(
                                                                "Pedals set: Accelerator {accel_mv} mV, Brake {brake_mv} mV."
                                                            ),
                                                        );
                                                    }
                                                    Err(e) => {
                                                        ctx.notify_toast(
                                                            w,
                                                            "Calibration",
                                                            "Couldn't set pedals",
                                                            &e.title,
                                                        );
                                                        ui_log!(
                                                            "RSH-UI setup calibrate pedals failed: {}",
                                                            e.title
                                                        );
                                                    }
                                                }
                                            },
                                        );
                                    }
                                    Err(e) => {
                                        ctx.notify_toast(w, "Calibration", "Couldn't set pedals", &e.title);
                                        ui_log!(
                                            "RSH-UI setup calibrate pedals failed: {}",
                                            e.title
                                        );
                                    }
                                }
                            },
                        );
                    } else {
                        let reason = Ctx::capture_unavailable_reason();
                        ctx.notify_toast(&w, "Calibration", "Couldn't set pedals", reason);
                        ui_log!("RSH-UI setup calibrate pedals failed: {reason}");
                    }
                }
                other => ui_log!("RSH-UI setup calibrate: unknown action {other}"),
            }
        });
    }
}

/// Wire the Pedals page callbacks (v2.0.1 hotfix + UI-FINISH A1/A2).
///
/// Pedal index order (verified in `ui/screens/pedals.slint`):
/// 0 throttle, 1 brake, 2 clutch. A1: the deadzone slider is 0..1
/// of 10% — slider value = pct/10, handler does v*10 (the old
/// `v * 100` read 20% as "2%"). Typed deadzone values are percents
/// ("2%"); the bite slider is 0..1 and typed bite values are
/// percents ("40%" → 0.40). Feel chips push the curve (Linear 1.0 /
/// Soft 0.7 / Sharp 1.5) through `set_tuning`. Set up/down capture
/// the live endpoints (v2.0.2): up = released minimum, down = fully
/// pressed maximum.
fn wire_pedals(win: &MainWindow, ctx: &Rc<Ctx>) {
    const MODULES: [&str; 3] = ["throttle", "brake", "clutch"];
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_pedal_deadzone_changed(move |p, v| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let Ok(idx) = usize::try_from(p) else {
                return;
            };
            let Some(module) = MODULES.get(idx) else {
                return;
            };
            // A1: v is 0..1 of 10% → deadzone percent is v * 10.
            let deadzone = (f64::from(v) * 10.0).clamp(0.0, 10.0);
            // R2-10 (Steering pattern): the slider + text +
            // mini-curve move with the drag, not at the next ~1 Hz
            // re-sync — the thumb tracks the mouse.
            ctx.apply_pedal_deadzone_local(&w, idx, deadzone);
            // M-7: a drag queues one debounced push, not one per move.
            ctx.queue_field_push(module, FieldKind::Deadzone, deadzone);
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_pedal_deadzone_edited(move |p, t| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let Ok(pct) = t.trim().trim_end_matches('%').parse::<f32>() else {
                return;
            };
            let Ok(idx) = usize::try_from(p) else {
                return;
            };
            let Some(module) = MODULES.get(idx) else {
                return;
            };
            let deadzone = f64::from(pct).clamp(0.0, 10.0);
            ctx.set_pedal_tuning(&w, module, |t| t.deadzone = deadzone);
        });
    }
    wire_pedals_rest(win, ctx);
    wire_pedals_bite_width(win, ctx);
    wire_pedals_tuning(win, ctx);
    wire_pedals_set(win, ctx);
}

/// Wire the per-module tuning callbacks (Workstream C): the
/// Advanced curve / ramp / saturation sliders and the combined
/// pedals switch. Sliders echo locally + queue one debounced push
/// (M-7, like deadzone/bite); typed values push at once; the
/// toggle pushes at once (no drag to debounce). Ranges: curve
/// 0.5..2.0, throttle ramps 0..500 ms, brake press 0..300 ms,
/// clutch release 0..1000 ms, saturation 70..100%.
// Nine near-identical callback blocks (one per control), like
// `wire_buttons` above — splitting them would hide the pattern.
#[allow(clippy::too_many_lines)]
fn wire_pedals_tuning(win: &MainWindow, ctx: &Rc<Ctx>) {
    const MODULES: [&str; 3] = ["throttle", "brake", "clutch"];
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_pedal_curve_changed(move |p, v| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let Ok(idx) = usize::try_from(p) else {
                return;
            };
            let Some(module) = MODULES.get(idx) else {
                return;
            };
            let exponent = (0.5 + f64::from(v) * 1.5).clamp(0.5, 2.0);
            ctx.apply_pedal_curve_local(&w, idx, exponent);
            ctx.queue_field_push(module, FieldKind::Curve, exponent);
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_pedal_curve_edited(move |p, t| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let Ok(exponent) = t.trim().parse::<f32>() else {
                return;
            };
            let Ok(idx) = usize::try_from(p) else {
                return;
            };
            let Some(module) = MODULES.get(idx) else {
                return;
            };
            let exponent = f64::from(exponent).clamp(0.5, 2.0);
            ctx.set_pedal_tuning(&w, module, |t| t.curve = exponent);
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_pedal_press_ms_changed(move |p, v| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let Ok(idx) = usize::try_from(p) else {
                return;
            };
            let Some(module) = MODULES.get(idx) else {
                return;
            };
            let max_ms = pedal_ramp_range_ms(idx, true);
            if max_ms <= 0.0 {
                return;
            }
            let ms = (f64::from(v) * max_ms).clamp(0.0, max_ms);
            ctx.apply_pedal_press_ms_local(&w, idx, ms);
            ctx.queue_field_push(module, FieldKind::PressMs, ms);
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_pedal_press_ms_edited(move |p, t| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let Ok(ms) = t.trim().trim_end_matches("ms").trim().parse::<f32>() else {
                return;
            };
            let Ok(idx) = usize::try_from(p) else {
                return;
            };
            let Some(module) = MODULES.get(idx) else {
                return;
            };
            let max_ms = pedal_ramp_range_ms(idx, true);
            if max_ms <= 0.0 {
                return;
            }
            let ms = f64::from(ms).clamp(0.0, max_ms);
            ctx.set_pedal_tuning(&w, module, |t| t.press_ms = Some(ms));
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_pedal_release_ms_changed(move |p, v| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let Ok(idx) = usize::try_from(p) else {
                return;
            };
            let Some(module) = MODULES.get(idx) else {
                return;
            };
            let max_ms = pedal_ramp_range_ms(idx, false);
            if max_ms <= 0.0 {
                return;
            }
            let ms = (f64::from(v) * max_ms).clamp(0.0, max_ms);
            ctx.apply_pedal_release_ms_local(&w, idx, ms);
            ctx.queue_field_push(module, FieldKind::ReleaseMs, ms);
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_pedal_release_ms_edited(move |p, t| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let Ok(ms) = t.trim().trim_end_matches("ms").trim().parse::<f32>() else {
                return;
            };
            let Ok(idx) = usize::try_from(p) else {
                return;
            };
            let Some(module) = MODULES.get(idx) else {
                return;
            };
            let max_ms = pedal_ramp_range_ms(idx, false);
            if max_ms <= 0.0 {
                return;
            }
            let ms = f64::from(ms).clamp(0.0, max_ms);
            ctx.set_pedal_tuning(&w, module, |t| t.release_ms = Some(ms));
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_throttle_saturation_changed(move |v| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let pct = 70.0 + f64::from(v) * 30.0;
            let record = ctx.local_tuning_record("throttle");
            let max_mv = ctx.throttle_saturation_max_mv(&record, pct);
            ctx.apply_throttle_saturation_local(&w, max_mv);
            ctx.queue_field_push("throttle", FieldKind::Saturation, max_mv);
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_throttle_saturation_edited(move |t| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let Ok(pct) = t.trim().trim_end_matches('%').parse::<f32>() else {
                return;
            };
            let pct = f64::from(pct).clamp(70.0, 100.0);
            let record = ctx.api().get_tuning("throttle");
            let max_mv = ctx.throttle_saturation_max_mv(&record, pct);
            ctx.set_pedal_tuning(&w, "throttle", |t| t.max_mv = max_mv);
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_combine_pedals_changed(move |b| {
            let Some(w) = win.upgrade() else {
                return;
            };
            ctx.set_pedal_tuning(&w, "throttle", |t| t.combine_pedals = Some(b));
        });
    }
}

/// Wire the clutch bite-point and feel-preset callbacks (split
/// from [`wire_pedals`]; Set up/down live in [`wire_pedals_set`]).
fn wire_pedals_rest(win: &MainWindow, ctx: &Rc<Ctx>) {
    const MODULES: [&str; 3] = ["throttle", "brake", "clutch"];
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_pedal_bite_changed(move |v| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let bite = f64::from(v).clamp(0.0, 1.0);
            // R2-10: local echo — the bite slider + text move with
            // the drag, not at the next ~1 Hz re-sync.
            ctx.apply_pedal_bite_local(&w, 2, bite);
            // M-7: a drag queues one debounced push, not one per move.
            ctx.queue_field_push("clutch", FieldKind::Bite, bite);
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_pedal_bite_edited(move |t| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let Ok(pct) = t.trim().trim_end_matches('%').parse::<f32>() else {
                return;
            };
            let bite = (f64::from(pct) / 100.0).clamp(0.0, 1.0);
            ctx.set_pedal_tuning(&w, "clutch", |t| t.bite_point = Some(bite));
        });
    }
    // --- Brake per-module tuning ---
    // B2 brake bite: the slider is 0..1 of 50% (like deadzone is
    // 0..1 of 10%), so a drag value is halved; typed percents are
    // 0–50%. The engine already applies the bite to the brake
    // (shared pedal chain) — this is UI-only.
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_pedal_brake_bite_changed(move |v| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let bite = (f64::from(v) * 0.5).clamp(0.0, 0.5);
            ctx.apply_pedal_bite_local(&w, 1, bite);
            ctx.queue_field_push("brake", FieldKind::Bite, bite);
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_pedal_brake_bite_edited(move |t| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let Ok(pct) = t.trim().trim_end_matches('%').parse::<f32>() else {
                return;
            };
            let bite = (f64::from(pct) / 100.0).clamp(0.0, 0.5);
            ctx.set_pedal_tuning(&w, "brake", |t| t.bite_point = Some(bite));
        });
    }
    // B1 brake force: the slider value IS the saturation fraction
    // (0.5..1.0 = 50–100% travel for full brake); typed percents
    // are 50–100%.
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_pedal_brake_force_changed(move |v| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let saturation = f64::from(v).clamp(0.5, 1.0);
            ctx.apply_pedal_brake_force_local(&w, saturation);
            ctx.queue_field_push("brake", FieldKind::Force, saturation);
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_pedal_brake_force_edited(move |t| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let Ok(pct) = t.trim().trim_end_matches('%').parse::<f32>() else {
                return;
            };
            let saturation = (f64::from(pct) / 100.0).clamp(0.5, 1.0);
            ctx.set_pedal_tuning(&w, "brake", |t| t.saturation = Some(saturation));
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_pedal_feel_selected(move |p, f| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let Ok(idx) = usize::try_from(p) else {
                return;
            };
            let Some(module) = MODULES.get(idx) else {
                return;
            };
            let curve = f64::from(PedalCache::feel_exponent(f));
            ctx.set_pedal_tuning(&w, module, |t| t.curve = curve);
        });
    }
}

/// Wire the clutch slip-zone callbacks (split from
/// [`wire_pedals_rest`]: C2 bite width, 5–40% around the bite
/// point).
// --- Handbrake/clutch per-module tuning ---
fn wire_pedals_bite_width(win: &MainWindow, ctx: &Rc<Ctx>) {
    // C2 slip zone: the slider is 0..1 of 5..40% (like deadzone is
    // 0..1 of 10%), so a drag value maps to 0.05..0.40; typed
    // percents are 5–40%.
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_pedal_clutch_bite_width_changed(move |v| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let width = (0.05 + f64::from(v) * 0.35).clamp(0.05, 0.40);
            // R2-10: local echo — the slider + text move with the
            // drag, not at the next ~1 Hz re-sync.
            ctx.apply_pedal_bite_width_local(&w, width);
            // M-7: a drag queues one debounced push, not one per move.
            ctx.queue_field_push("clutch", FieldKind::BiteWidth, width);
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_pedal_clutch_bite_width_edited(move |t| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let Ok(pct) = t.trim().trim_end_matches('%').parse::<f32>() else {
                return;
            };
            let width = (f64::from(pct) / 100.0).clamp(0.05, 0.40);
            ctx.set_pedal_tuning(&w, "clutch", |t| t.bite_width = Some(width));
        });
    }
}

/// Wire the Pedals page Set up/down buttons (v2.0.2): up captures
/// the released endpoint from the live raw reading, down the fully
/// pressed one. Failure shows the reason, never silently.
fn wire_pedals_set(win: &MainWindow, ctx: &Rc<Ctx>) {
    {
        // A2 (v2.0.2): "Set up" captures the pedal's released
        // endpoint from the live raw reading; "Set down" the fully
        // pressed one. Failure shows the reason, never silently.
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_pedal_set_up(move |p| {
            if let Some(w) = win.upgrade() {
                let Some((key, name)) = pedal_key(p) else {
                    return;
                };
                // F6: the capture runs on a CaptureOp job thread.
                if !ctx.is_live() {
                    let reason = Ctx::capture_unavailable_reason();
                    ctx.notify(&w, "Pedals", &format!("Couldn't set {name} up"), reason);
                    ui_log!("RSH-UI pedal set up {key} failed: {reason}");
                    return;
                }
                ctx.run_generic_job(
                    &w,
                    JobKind::CaptureOp,
                    "Capturing…",
                    move |api| JobResult::Capture(api.capture_pedal_min(key)),
                    move |ctx, w, result| {
                        let JobResult::Capture(result) = result else {
                            return;
                        };
                        match result {
                            Ok(mv) => {
                                ctx.refresh_calibration();
                                ctx.sync_pedals(w);
                                ctx.notify(
                                    w,
                                    "Pedals",
                                    &format!("{name} released point set"),
                                    &format!("{name} up (released) captured at {mv} mV."),
                                );
                            }
                            Err(e) => {
                                ctx.notify(
                                    w,
                                    "Pedals",
                                    &format!("Couldn't set {name} up"),
                                    &e.title,
                                );
                                ui_log!("RSH-UI pedal set up {key} failed: {}", e.title);
                            }
                        }
                    },
                );
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_pedal_set_down(move |p| {
            if let Some(w) = win.upgrade() {
                let Some((key, name)) = pedal_key(p) else {
                    return;
                };
                // F6: the capture runs on a CaptureOp job thread.
                if !ctx.is_live() {
                    let reason = Ctx::capture_unavailable_reason();
                    ctx.notify(&w, "Pedals", &format!("Couldn't set {name} down"), reason);
                    ui_log!("RSH-UI pedal set down {key} failed: {reason}");
                    return;
                }
                ctx.run_generic_job(
                    &w,
                    JobKind::CaptureOp,
                    "Capturing…",
                    move |api| JobResult::Capture(api.capture_pedal_max(key)),
                    move |ctx, w, result| {
                        let JobResult::Capture(result) = result else {
                            return;
                        };
                        match result {
                            Ok(mv) => {
                                ctx.refresh_calibration();
                                ctx.sync_pedals(w);
                                ctx.notify(
                                    w,
                                    "Pedals",
                                    &format!("{name} pressed point set"),
                                    &format!("{name} down (pressed) captured at {mv} mV."),
                                );
                            }
                            Err(e) => {
                                ctx.notify(
                                    w,
                                    "Pedals",
                                    &format!("Couldn't set {name} down"),
                                    &e.title,
                                );
                                ui_log!("RSH-UI pedal set down {key} failed: {}", e.title);
                            }
                        }
                    },
                );
            }
        });
    }
}

/// Wire the Buttons page callbacks (v2.0.1 hotfix + UI-FINISH A3–A5).
///
/// A3: an action chip updates the pending mapping for the selected
/// tile (visible at once); a knob click opens the knob edit panel
/// (the Slint sets edit-kind itself); Save calls
/// `set_button_action` (BACKEND-2 seam) — until it lands Save
/// explains the gap and keeps the pending choice. A4: the Pill
/// follows the selected tile's action (Rust-owned, index 5 / "Gear
/// 3" at startup). A5: pressed-index is always -1 (no live button
/// state in the snapshot).
#[allow(clippy::too_many_lines, clippy::cast_sign_loss)]
fn wire_buttons(win: &MainWindow, ctx: &Rc<Ctx>) {
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_tile_selected(move |i| {
            let Some(w) = win.upgrade() else {
                return;
            };
            ctx.selected_button.set(i);
            *ctx.pending_action.borrow_mut() = None;
            *ctx.pending_long_action.borrow_mut() = None;
            let idx = usize::try_from(i).unwrap_or(0);
            let action = ctx
                .button_actions
                .borrow()
                .get(idx)
                .cloned()
                .unwrap_or_else(|| "Not set".to_string());
            let long_action = ctx
                .long_press_actions
                .borrow()
                .get(idx)
                .cloned()
                .unwrap_or_else(|| "Not set".to_string());
            w.set_selected_index(i);
            w.set_selected_action(action.into());
            w.set_selected_long_action(long_action.into());
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_knob_selected(move |i| {
            ctx.edit_kind.set(1);
            ctx.edit_knob.set(i);
            let row = ctx.knob_row.get();
            let action = ctx.knob_actions.borrow()[usize::try_from(i).unwrap_or(0)]
                [usize::try_from(row).unwrap_or(0)]
            .clone();
            if let Some(w) = win.upgrade() {
                w.set_knob_row_action(action.into());
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_knob_row_selected(move |i| {
            ctx.knob_row.set(i);
            let knob = ctx.edit_knob.get();
            let action = ctx.knob_actions.borrow()[usize::try_from(knob).unwrap_or(0)]
                [usize::try_from(i).unwrap_or(0)]
            .clone();
            if let Some(w) = win.upgrade() {
                w.set_knob_row_action(action.into());
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_action_chip(move |a| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let action = a.to_string();
            if ctx.edit_kind.get() == 1 {
                let knob = usize::try_from(ctx.edit_knob.get()).unwrap_or(0);
                let row = usize::try_from(ctx.knob_row.get()).unwrap_or(0);
                ctx.knob_actions.borrow_mut()[knob][row].clone_from(&action);
                w.set_knob_row_action(action.into());
            } else {
                *ctx.pending_action.borrow_mut() = Some(action.clone());
                w.set_selected_action(action.into());
            }
            ctx.unsaved.set(true);
            ctx.refresh(&w, 0);
        });
    }
    // BU2: a long-press chip updates the pending long-press mapping
    // for the selected tile (visible at once, same pattern as the
    // normal action chip above — button edits only, knobs have no
    // long-press action).
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_long_action_chip(move |a| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let action = a.to_string();
            *ctx.pending_long_action.borrow_mut() = Some(action.clone());
            w.set_selected_long_action(action.into());
            ctx.unsaved.set(true);
            ctx.refresh(&w, 0);
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_learn_clicked(move || {
            if let Some(w) = win.upgrade() {
                ctx.toggle_learn(&w, 1);
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_edit_save(move || {
            let Some(w) = win.upgrade() else {
                return;
            };
            // Knob edit (edit_kind == 1): persist the knob row's
            // action (M9) — the chip tap already staged it in
            // `knob_actions`; Save writes it to the daemon store.
            // F6: `set_knob_action` runs on a BindingOp job thread.
            if ctx.edit_kind.get() == 1 {
                let knob = ctx.edit_knob.get();
                let row = ctx.knob_row.get();
                let display = ctx.knob_actions.borrow()[usize::try_from(knob).unwrap_or(0)]
                    [usize::try_from(row).unwrap_or(0)]
                .clone();
                let action = action_id(&display).unwrap_or("");
                let knob_u8 = u8::try_from(knob).unwrap_or(0);
                let row_u8 = u8::try_from(row).unwrap_or(0);
                let action = action.to_string();
                ctx.run_generic_job(
                    &w,
                    JobKind::BindingOp,
                    "Saving knob mapping…",
                    move |api| JobResult::Unit(api.set_knob_action(knob_u8, row_u8, &action)),
                    move |ctx, w, result| {
                        let JobResult::Unit(result) = result else {
                            return;
                        };
                        match result {
                            Ok(()) => {
                                ctx.unsaved
                                    .set(ctx.api().draft_status().is_some_and(|s| s.dirty));
                                ctx.load_knobs(w);
                                ctx.notify(
                                    w,
                                    "Buttons",
                                    "Knob mapping saved",
                                    &format!("Knob {} → {display}.", knob + 1),
                                );
                            }
                            Err(e) => {
                                ctx.notify_error(w, "Buttons", &e);
                                // Revert the staged choice to the stored one.
                                ctx.load_knobs(w);
                            }
                        }
                    },
                );
                return;
            }
            // Button edit: persist the pending chip choices (H-3 +
            // BU2) — "Not set" clears the binding, any other chip
            // sets it; the long-press chip does the same for the
            // long-press action. Both save in one BindingOp job so
            // the tile never shows a half-saved pair. The tile
            // refreshes from the daemon immediately after. F6:
            // `set_binding` / `clear_binding` /
            // `set_long_press_binding` / `clear_long_press_binding`
            // run on a BindingOp job thread.
            let pending = ctx.pending_action.borrow().clone();
            let pending_long = ctx.pending_long_action.borrow().clone();
            if pending.is_some() || pending_long.is_some() {
                let button = ctx.selected_button.get() + 1;
                let button_u8 = u8::try_from(button).unwrap_or(0);
                let binding_id = pending.as_deref().and_then(action_id);
                let long_binding_id = pending_long.as_deref().and_then(action_id);
                let saved_action = pending.clone().unwrap_or_default();
                let saved_long = pending_long.clone().unwrap_or_default();
                ctx.run_generic_job(
                    &w,
                    JobKind::BindingOp,
                    "Saving button mapping…",
                    move |api| {
                        JobResult::Unit(
                            (|| -> Result<(), UserError> {
                                // Normal binding first, then the
                                // long-press one — either failing
                                // aborts the job, and the tile
                                // reverts to the stored pair.
                                if pending.is_some() {
                                    match binding_id {
                                        Some(id) => {
                                            api.set_binding(button_u8, id).map(|_| ())?;
                                        }
                                        None => {
                                            api.clear_binding(button_u8).map(|_| ())?;
                                        }
                                    }
                                }
                                if pending_long.is_some() {
                                    match long_binding_id {
                                        Some(id) => {
                                            api.set_long_press_binding(button_u8, id)
                                                .map(|_| ())?;
                                        }
                                        None => {
                                            api.clear_long_press_binding(button_u8)
                                                .map(|_| ())?;
                                        }
                                    }
                                }
                                Ok(())
                            })(),
                        )
                    },
                    move |ctx, w, result| {
                        let JobResult::Unit(result) = result else {
                            return;
                        };
                        match result {
                            Ok(()) => {
                                *ctx.pending_action.borrow_mut() = None;
                                *ctx.pending_long_action.borrow_mut() = None;
                                ctx.unsaved
                                    .set(ctx.api().draft_status().is_some_and(|s| s.dirty));
                                ctx.load_bindings(w);
                                let detail = if saved_long.is_empty() {
                                    format!("Button {button} → {saved_action}.")
                                } else if saved_action.is_empty() {
                                    format!("Button {button} long-press → {saved_long}.")
                                } else {
                                    format!(
                                        "Button {button} → {saved_action}; long-press → {saved_long}."
                                    )
                                };
                                ctx.notify(w, "Buttons", "Button mapping saved", &detail);
                            }
                            Err(e) => {
                                ctx.notify_error(w, "Buttons", &e);
                                // Revert the tile to the stored actions.
                                ctx.load_bindings(w);
                            }
                        }
                    },
                );
                return;
            }
            // F6: `save_draft` runs on a ProfileOp job thread.
            ctx.run_generic_job(
                &w,
                JobKind::ProfileOp,
                "Saving changes…",
                move |api| JobResult::Bool(api.save_draft()),
                move |ctx, w, result| {
                    let JobResult::Bool(result) = result else {
                        return;
                    };
                    match result {
                        Ok(true) => {
                            ctx.unsaved.set(false);
                            ctx.notify(w, "Buttons", "Changes saved", "Button changes saved.");
                        }
                        Ok(false) => {
                            ctx.notify(w, "Buttons", "Nothing to save", "No changes to save.");
                        }
                        Err(e) => {
                            ctx.notify_error(w, "Buttons", &e);
                            ui_log!("RSH-UI buttons edit-save failed: {} ({})", e.code, e.title);
                        }
                    }
                },
            );
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_edit_cancel(move || {
            let Some(w) = win.upgrade() else {
                return;
            };
            *ctx.pending_action.borrow_mut() = None;
            *ctx.pending_long_action.borrow_mut() = None;
            let idx = usize::try_from(ctx.selected_button.get()).unwrap_or(0);
            let action = ctx
                .button_actions
                .borrow()
                .get(idx)
                .cloned()
                .unwrap_or_else(|| "Not set".to_string());
            w.set_selected_action(action.into());
            let long_action = ctx
                .long_press_actions
                .borrow()
                .get(idx)
                .cloned()
                .unwrap_or_else(|| "Not set".to_string());
            w.set_selected_long_action(long_action.into());
            // F6: `discard_draft` runs on a ProfileOp job thread.
            ctx.run_generic_job(
                &w,
                JobKind::ProfileOp,
                "Discarding changes…",
                move |api| JobResult::Bool(Ok(api.discard_draft())),
                move |ctx, w, result| {
                    let JobResult::Bool(result) = result else {
                        return;
                    };
                    if result.unwrap_or(false) {
                        ctx.unsaved.set(false);
                        ctx.notify(w, "Buttons", "Changes discarded", "Changes discarded.");
                    } else {
                        ctx.notify(w, "Buttons", "Nothing to discard", "No changes to discard.");
                    }
                },
            );
        });
    }
    // Encoder tuning (E1/E2): pulse slider (debounced push, local
    // echo while dragging) + steps-per-click segment (immediate
    // push, like the handbrake Axis/Button segment).
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_enc_pulse_changed(move |v| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let pulse_ms = (20.0 + f64::from(v) * 180.0).clamp(20.0, 200.0);
            Ctx::apply_encoder_local(&w, pulse_ms, f64::from(w.get_enc_accel_sens()));
            // M-7: a drag queues one debounced push, not one per move.
            ctx.queue_field_push("encoders", FieldKind::EncoderPulseMs, pulse_ms);
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_enc_pulse_edited(move |t| {
            let Some(w) = win.upgrade() else {
                return;
            };
            if let Ok(ms) = t.trim().trim_end_matches(['m', 's', ' ']).parse::<f64>() {
                let pulse_ms = ms.clamp(20.0, 200.0);
                Ctx::apply_encoder_local(&w, pulse_ms, f64::from(w.get_enc_accel_sens()));
                ctx.queue_field_push("encoders", FieldKind::EncoderPulseMs, pulse_ms);
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_enc_counts_changed(move |i| {
            if let Some(w) = win.upgrade() {
                let counts = match i {
                    1 => 1u8,
                    2 => 2u8,
                    _ => 4u8,
                };
                ctx.set_module_tuning(&w, "encoders", move |t| {
                    t.encoder_counts_per_click = Some(counts);
                });
            }
        });
    }
    // Encoder tuning (E3/E4): acceleration switch (immediate push)
    // + strength slider (debounced push, local echo) + per-knob
    // invert switches (immediate push, mask bit per knob).
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_enc_accel_changed(move |b| {
            if let Some(w) = win.upgrade() {
                ctx.set_module_tuning(&w, "encoders", move |t| {
                    t.encoder_accel = Some(b);
                });
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_enc_accel_sens_changed(move |v| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let sens = (f64::from(v) * 2.0).clamp(0.0, 2.0);
            Ctx::apply_encoder_local(&w, f64::from(w.get_enc_pulse_ms()), sens);
            ctx.queue_field_push("encoders", FieldKind::EncoderAccelSens, sens);
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_enc_accel_sens_edited(move |t| {
            let Some(w) = win.upgrade() else {
                return;
            };
            if let Ok(pct) = t.trim().trim_end_matches(['%', ' ']).parse::<f64>() {
                let sens = (pct / 100.0).clamp(0.0, 2.0);
                Ctx::apply_encoder_local(&w, f64::from(w.get_enc_pulse_ms()), sens);
                ctx.queue_field_push("encoders", FieldKind::EncoderAccelSens, sens);
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_enc_invert_changed(move |i, b| {
            if let Some(w) = win.upgrade() {
                let bit: u8 = match i {
                    0 => 0b0001,
                    1 => 0b0010,
                    _ => return,
                };
                ctx.set_module_tuning(&w, "encoders", move |t| {
                    let mut mask = t.encoder_invert_mask.unwrap_or(0);
                    if b {
                        mask |= bit;
                    } else {
                        mask &= !bit;
                    }
                    t.encoder_invert_mask = Some(mask);
                });
            }
        });
    }
    // Button tuning (BU1/BU2): debounce + long-press threshold
    // sliders (debounced push, local echo while dragging — same
    // pattern as the encoder pulse slider above).
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_btn_debounce_changed(move |v| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let debounce_ms = (f64::from(v) * 50.0).clamp(0.0, 50.0);
            Ctx::apply_button_local(
                &w,
                debounce_ms,
                f64::from(w.get_btn_longpress_ms()),
                f64::from(w.get_btn_doublepress_ms()),
            );
            // M-7: a drag queues one debounced push, not one per move.
            ctx.queue_field_push("buttons", FieldKind::ButtonDebounceMs, debounce_ms);
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_btn_debounce_edited(move |t| {
            let Some(w) = win.upgrade() else {
                return;
            };
            if let Ok(ms) = t.trim().trim_end_matches(['m', 's', ' ']).parse::<f64>() {
                let debounce_ms = ms.clamp(0.0, 50.0);
                Ctx::apply_button_local(
                    &w,
                    debounce_ms,
                    f64::from(w.get_btn_longpress_ms()),
                    f64::from(w.get_btn_doublepress_ms()),
                );
                ctx.queue_field_push("buttons", FieldKind::ButtonDebounceMs, debounce_ms);
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_btn_longpress_changed(move |v| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let long_press_ms = (300.0 + f64::from(v) * 1200.0).clamp(300.0, 1500.0);
            Ctx::apply_button_local(
                &w,
                f64::from(w.get_btn_debounce_ms()),
                long_press_ms,
                f64::from(w.get_btn_doublepress_ms()),
            );
            // M-7: a drag queues one debounced push, not one per move.
            ctx.queue_field_push("buttons", FieldKind::ButtonLongPressMs, long_press_ms);
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_btn_longpress_edited(move |t| {
            let Some(w) = win.upgrade() else {
                return;
            };
            if let Ok(ms) = t.trim().trim_end_matches(['m', 's', ' ']).parse::<f64>() {
                let long_press_ms = ms.clamp(300.0, 1500.0);
                Ctx::apply_button_local(
                    &w,
                    f64::from(w.get_btn_debounce_ms()),
                    long_press_ms,
                    f64::from(w.get_btn_doublepress_ms()),
                );
                ctx.queue_field_push("buttons", FieldKind::ButtonLongPressMs, long_press_ms);
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_btn_doublepress_changed(move |v| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let double_press_ms = (150.0 + f64::from(v) * 350.0).clamp(150.0, 500.0);
            Ctx::apply_button_local(
                &w,
                f64::from(w.get_btn_debounce_ms()),
                f64::from(w.get_btn_longpress_ms()),
                double_press_ms,
            );
            // M-7: a drag queues one debounced push, not one per move.
            ctx.queue_field_push("buttons", FieldKind::ButtonDoublePressMs, double_press_ms);
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_btn_doublepress_edited(move |t| {
            let Some(w) = win.upgrade() else {
                return;
            };
            if let Ok(ms) = t.trim().trim_end_matches(['m', 's', ' ']).parse::<f64>() {
                let double_press_ms = ms.clamp(150.0, 500.0);
                Ctx::apply_button_local(
                    &w,
                    f64::from(w.get_btn_debounce_ms()),
                    f64::from(w.get_btn_longpress_ms()),
                    double_press_ms,
                );
                ctx.queue_field_push("buttons", FieldKind::ButtonDoublePressMs, double_press_ms);
            }
        });
    }
}

/// Wire the Extras page callbacks (A6): LED pattern/brightness/colour
/// and handbrake feel/deadzone push through `set_tuning`; horn Learn
/// arms `start_learn`; Test flashes the indicator; the shifter link
/// opens the Buttons page; the shifter sensitivity slider pushes
/// through the board config transaction (SH1) and the lockout slider
/// through `set_tuning` (SH2). Handbrake Set up/down capture the live
/// endpoints (v2.0.2). Controls with no backend (handbrake mode)
/// explain themselves instead of pretending to work.
#[allow(clippy::too_many_lines)]
fn wire_extras(win: &MainWindow, ctx: &Rc<Ctx>) {
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_extras_action(move |a| {
            let Some(w) = win.upgrade() else {
                return;
            };
            match a.as_str() {
                "handbrake-toggle" | "shifter-toggle" | "horn-toggle" | "leds-toggle" => {
                    // H7: the toggle REALLY enables/disables the
                    // module (draft entry + pipeline plan) — the card
                    // greying out is only the visual echo. The .slint
                    // flips its `-on` prop first and carries no
                    // payload, so Rust tracks the intended state in
                    // `module_states`; a failure reverts the prop
                    // (FC2: the `-on` props are MainWindow in-out
                    // props, so Rust can write them back).
                    let (module, index) = match a.as_str() {
                        "handbrake-toggle" => ("handbrake", 0),
                        "shifter-toggle" => ("shifter", 1),
                        "horn-toggle" => ("horn", 2),
                        _ => ("led", 3),
                    };
                    let old_state = ctx.module_states.borrow()[index];
                    let new_state = !old_state;
                    // F6: `set_module_enabled` runs on a SettingsOp
                    // job thread — the toggle never blocks on the
                    // daemon.
                    ctx.run_generic_job(
                        &w,
                        JobKind::SettingsOp,
                        "Saving…",
                        move |api| JobResult::Bool(api.set_module_enabled(module, new_state)),
                        move |ctx, w, result| {
                            let JobResult::Bool(result) = result else {
                                return;
                            };
                            match result {
                                Ok(enabled) => {
                                    ctx.module_states.borrow_mut()[index] = enabled;
                                    // FC2: confirm the prop with the
                                    // authoritative value (`sync_extras` in
                                    // refresh re-seeds it too).
                                    set_extras_on_prop(w, index, enabled);
                                }
                                Err(e) => {
                                    // FC2: the .slint already flipped the
                                    // prop — flip it back so the switch
                                    // never lies about the real module state.
                                    set_extras_on_prop(w, index, old_state);
                                    ctx.notify_error(w, "Extras", &e);
                                }
                            }
                        },
                    );
                }
                "handbrake-up" | "handbrake-down" => {
                    // v2.0.2: capture the handbrake endpoint from
                    // the live raw reading (up = released, down =
                    // fully pressed). F6: the capture runs on a
                    // CaptureOp job thread.
                    let endpoint = if a.as_str() == "handbrake-up" {
                        "min"
                    } else {
                        "max"
                    };
                    if ctx.is_live() {
                        ctx.run_generic_job(
                            &w,
                            JobKind::CaptureOp,
                            "Capturing…",
                            move |api| {
                                JobResult::Capture(if endpoint == "min" {
                                    api.capture_pedal_min("handbrake")
                                } else {
                                    api.capture_pedal_max("handbrake")
                                })
                            },
                            move |ctx, w, result| {
                                let JobResult::Capture(result) = result else {
                                    return;
                                };
                                match result {
                                    Ok(mv) => {
                                        ctx.refresh_calibration();
                                        ctx.sync_extras(w);
                                        ctx.notify(
                                            w,
                                            "Extras",
                                            "Handbrake set",
                                            &format!(
                                                "Handbrake {} captured at {mv} mV.",
                                                if endpoint == "min" {
                                                    "up (released)"
                                                } else {
                                                    "down (pressed)"
                                                }
                                            ),
                                        );
                                    }
                                    Err(e) => {
                                        ctx.notify(w, "Extras", "Couldn't set handbrake", &e.title);
                                        ui_log!(
                                            "RSH-UI extras handbrake capture failed: {}",
                                            e.title
                                        );
                                    }
                                }
                            },
                        );
                    } else {
                        let reason = Ctx::capture_unavailable_reason();
                        ctx.notify(&w, "Extras", "Couldn't set handbrake", reason);
                        ui_log!("RSH-UI extras handbrake capture failed: {reason}");
                    }
                }
                "horn-learn" => ctx.toggle_learn(&w, 2),
                "handbrake-test" => {
                    // R2-17: the Button-mode tile lights "Pressed"
                    // for 400 ms, mirroring the horn test — the tile
                    // used to do nothing at all.
                    ctx.handbrake_test_until
                        .set(Some(Instant::now() + Duration::from_millis(400)));
                    ctx.refresh(&w, 0);
                }
                "horn-test" => {
                    // HO1: the indicator flash follows the configured
                    // minimum honk (default 400 ms), not a fixed value.
                    let ms = ctx
                        .horn_record
                        .borrow()
                        .horn_min_ms
                        .unwrap_or(400.0)
                        .clamp(100.0, 1000.0);
                    ctx.horn_test_until
                        .set(Some(Instant::now() + Duration::from_secs_f64(ms / 1000.0)));
                    ctx.refresh(&w, 0);
                }
                "shifter-change-in-buttons" => w.set_page(3),
                other => ui_log!("RSH-UI unknown extras-action: {other}"),
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_extras_handbrake_mode(move |i| {
            if let Some(w) = win.upgrade() {
                // H8: "Button" mode is selectable — persisted in the
                // profile (the board-side pinmap push lives in
                // `commands.rs`; see the report).
                ctx.set_module_tuning(&w, "handbrake", |t| {
                    t.handbrake_button_mode = Some(i == 1);
                });
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_extras_handbrake_feel(move |f| {
            if let Some(w) = win.upgrade() {
                let curve = f64::from(PedalCache::feel_exponent(f));
                ctx.set_module_tuning(&w, "handbrake", |t| t.curve = curve);
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_extras_handbrake_deadzone(move |v| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let deadzone = (f64::from(v) * 10.0).clamp(0.0, 10.0);
            // R2-10: local echo — the slider + text + mini-curve
            // move with the drag, not at the next ~1 Hz re-sync.
            ctx.apply_handbrake_deadzone_local(&w, deadzone);
            // M-7: a drag queues one debounced push, not one per move.
            ctx.queue_field_push("handbrake", FieldKind::Deadzone, deadzone);
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_extras_handbrake_deadzone_edited(move |t| {
            if let Some(w) = win.upgrade() {
                if let Ok(pct) = t.trim().trim_end_matches('%').parse::<f32>() {
                    let deadzone = f64::from(pct).clamp(0.0, 10.0);
                    ctx.set_module_tuning(&w, "handbrake", |t| t.deadzone = deadzone);
                }
            }
        });
    }
    // --- Handbrake/clutch per-module tuning ---
    // H1 latch + H3 auto-release: switches push immediately.
    // H2 grab speed: the slider is 0..1 of 0..200 ms (the
    // handbrake press ramp); drags echo locally + push debounced,
    // typed values parse like the encoder pulse ("120 ms").
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_extras_handbrake_latch_changed(move |b| {
            if let Some(w) = win.upgrade() {
                ctx.set_module_tuning(&w, "handbrake", |t| {
                    t.handbrake_latch = Some(b);
                });
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_extras_handbrake_grab_ms_changed(move |v| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let grab_ms = (f64::from(v) * 200.0).clamp(0.0, 200.0);
            // R2-10: local echo — the slider + text move with the
            // drag, not at the next ~1 Hz re-sync.
            ctx.apply_handbrake_grab_local(&w, grab_ms);
            // M-7: a drag queues one debounced push, not one per move.
            ctx.queue_field_push("handbrake", FieldKind::PressMs, grab_ms);
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_extras_handbrake_grab_ms_edited(move |t| {
            let Some(w) = win.upgrade() else {
                return;
            };
            if let Ok(ms) = t.trim().trim_end_matches(['m', 's', ' ']).parse::<f64>() {
                let grab_ms = ms.clamp(0.0, 200.0);
                ctx.set_module_tuning(&w, "handbrake", |t| t.press_ms = Some(grab_ms));
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap()
            .on_extras_handbrake_auto_release_changed(move |b| {
                if let Some(w) = win.upgrade() {
                    ctx.set_module_tuning(&w, "handbrake", |t| {
                        t.handbrake_auto_release = Some(b);
                    });
                }
            });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_extras_led_pattern(move |i| {
            if let Some(w) = win.upgrade() {
                let pattern = match i {
                    0 => LedPattern::Off,
                    2 => LedPattern::Blink,
                    3 => LedPattern::Chase,
                    4 => LedPattern::Rainbow,
                    _ => LedPattern::Solid,
                };
                ctx.set_module_tuning(&w, "led", |t| t.led_pattern = Some(pattern));
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_extras_led_brightness(move |v| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let brightness = (v.clamp(0.0, 1.0) * 100.0).round();
            // R2-10: local echo — the slider + text move with the drag.
            ctx.apply_led_brightness_local(&w, f64::from(brightness));
            // M-7: a drag queues one debounced push, not one per move.
            ctx.queue_field_push("led", FieldKind::Brightness, f64::from(brightness));
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_extras_led_brightness_edited(move |t| {
            if let Some(w) = win.upgrade() {
                if let Ok(pct) = t.trim().trim_end_matches('%').parse::<f32>() {
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let brightness = (pct.clamp(0.0, 100.0)).round() as u8;
                    ctx.set_module_tuning(&w, "led", |t| t.led_brightness = Some(brightness));
                }
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_extras_led_color(move |i| {
            if let Some(w) = win.upgrade() {
                // M2: the swatch persists in the profile and the
                // preview re-renders in that colour (see
                // `led_preview` / `sync_extras`).
                let color = LED_PALETTE
                    .get(usize::try_from(i).unwrap_or(3))
                    .copied()
                    .unwrap_or(0x2F_6B_FF);
                ctx.set_module_tuning(&w, "led", |t| t.led_color = Some(color));
            }
        });
    }
    // Shifter tuning (SH1/SH2): sensitivity slider (debounced
    // board-config push, local echo while dragging) + lockout
    // slider (debounced `set_tuning` field push, like the encoder
    // pulse slider). Both have typed-value handlers, like the
    // handbrake deadzone / LED brightness sliders.
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_extras_shifter_sensitivity(move |v| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let pct = Ctx::sensitivity_pct(v);
            Ctx::apply_sensitivity_local(&w, pct);
            // M-7: a drag queues one debounced push, not one per move.
            ctx.queue_sensitivity_push(pct);
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_extras_shifter_sensitivity_edited(move |t| {
            let Some(w) = win.upgrade() else {
                return;
            };
            if let Ok(pct) = t.trim().trim_end_matches(['%', ' ']).parse::<f32>() {
                let pct = Ctx::sensitivity_pct(pct / 100.0);
                Ctx::apply_sensitivity_local(&w, pct);
                ctx.queue_sensitivity_push(pct);
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_extras_shifter_lockout(move |v| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let ms = (f64::from(v) * 200.0).clamp(0.0, 200.0);
            Ctx::apply_shifter_lockout_local(&w, ms);
            // M-7: a drag queues one debounced push, not one per move.
            ctx.queue_field_push("shifter", FieldKind::ShiftLockoutMs, ms);
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_extras_shifter_lockout_edited(move |t| {
            let Some(w) = win.upgrade() else {
                return;
            };
            if let Ok(ms) = t.trim().trim_end_matches(['m', 's', ' ']).parse::<f64>() {
                let ms = ms.clamp(0.0, 200.0);
                Ctx::apply_shifter_lockout_local(&w, ms);
                ctx.queue_field_push("shifter", FieldKind::ShiftLockoutMs, ms);
            }
        });
    }
    // LED pattern-cycle period (L1): slider (debounced push, local
    // echo while dragging) + typed value.
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_extras_led_period(move |v| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let period_ms = led_period_ms_from_slider(f64::from(v));
            // R2-10: local echo — the slider + text move with the drag.
            ctx.apply_led_period_local(&w, period_ms);
            // M-7: a drag queues one debounced push, not one per move.
            ctx.queue_field_push("led", FieldKind::LedPeriodMs, period_ms);
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_extras_led_period_edited(move |t| {
            let Some(w) = win.upgrade() else {
                return;
            };
            if let Ok(ms) = t.trim().trim_end_matches(['m', 's', ' ']).parse::<f64>() {
                let period_ms = ms.clamp(100.0, 5000.0);
                ctx.apply_led_period_local(&w, period_ms);
                ctx.queue_field_push("led", FieldKind::LedPeriodMs, period_ms);
            }
        });
    }
    // Custom LED colour (L2): each RGB slider re-composes the colour
    // from the three channel props (the moved channel first) and
    // queues one debounced push; typing a channel value (0–255)
    // does the same. The composed colour rides the push as the
    // field value (see `FieldKind::LedColor`).
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_extras_led_red(move |v| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let color = led_color_from_channels(v, w.get_led_green(), w.get_led_blue());
            ctx.apply_led_color_local(&w, color);
            ctx.queue_field_push("led", FieldKind::LedColor, f64::from(color));
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_extras_led_red_edited(move |t| {
            let Some(w) = win.upgrade() else {
                return;
            };
            if let Ok(channel) = t.trim().parse::<f32>() {
                let color =
                    led_color_from_channels(channel / 255.0, w.get_led_green(), w.get_led_blue());
                ctx.apply_led_color_local(&w, color);
                ctx.queue_field_push("led", FieldKind::LedColor, f64::from(color));
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_extras_led_green(move |v| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let color = led_color_from_channels(w.get_led_red(), v, w.get_led_blue());
            ctx.apply_led_color_local(&w, color);
            ctx.queue_field_push("led", FieldKind::LedColor, f64::from(color));
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_extras_led_green_edited(move |t| {
            let Some(w) = win.upgrade() else {
                return;
            };
            if let Ok(channel) = t.trim().parse::<f32>() {
                let color =
                    led_color_from_channels(w.get_led_red(), channel / 255.0, w.get_led_blue());
                ctx.apply_led_color_local(&w, color);
                ctx.queue_field_push("led", FieldKind::LedColor, f64::from(color));
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_extras_led_blue(move |v| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let color = led_color_from_channels(w.get_led_red(), w.get_led_green(), v);
            ctx.apply_led_color_local(&w, color);
            ctx.queue_field_push("led", FieldKind::LedColor, f64::from(color));
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_extras_led_blue_edited(move |t| {
            let Some(w) = win.upgrade() else {
                return;
            };
            if let Ok(channel) = t.trim().parse::<f32>() {
                let color =
                    led_color_from_channels(w.get_led_red(), w.get_led_green(), channel / 255.0);
                ctx.apply_led_color_local(&w, color);
                ctx.queue_field_push("led", FieldKind::LedColor, f64::from(color));
            }
        });
    }
    // Horn minimum honk (HO1): slider (debounced push, local echo
    // while dragging) + typed value.
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_extras_horn_min(move |v| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let min_ms = horn_min_ms_from_slider(f64::from(v));
            // R2-10: local echo — the slider + text move with the drag.
            ctx.apply_horn_min_local(&w, min_ms);
            // M-7: a drag queues one debounced push, not one per move.
            ctx.queue_field_push("horn", FieldKind::HornMinMs, min_ms);
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_extras_horn_min_edited(move |t| {
            let Some(w) = win.upgrade() else {
                return;
            };
            if let Ok(ms) = t.trim().trim_end_matches(['m', 's', ' ']).parse::<f64>() {
                let min_ms = ms.clamp(100.0, 1000.0);
                ctx.apply_horn_min_local(&w, min_ms);
                ctx.queue_field_push("horn", FieldKind::HornMinMs, min_ms);
            }
        });
    }
    // Honk mode (HO2): Momentary 0 / Latch 1 — immediate push, like
    // the handbrake Axis/Button segment.
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_extras_horn_mode(move |i| {
            if let Some(w) = win.upgrade() {
                ctx.set_module_tuning(&w, "horn", |t| {
                    t.horn_latch = Some(i == 1);
                });
            }
        });
    }
}

/// Wire the theme-persist callback (v2.0.1 hotfix, A8 v2.0.2): the
/// Settings screen reports its theme index (0 Dark, 1 Light,
/// 2 Auto) and Rust persists it AND resolves it to the window theme
/// (0 Dark / 1 Light — Auto follows the OS preference). R2-31: the
/// segment keeps showing the stored choice (`theme-choice`), so
/// Auto stays visibly selected.
fn wire_theme(win: &MainWindow, ctx: &Rc<Ctx>) {
    let win = win.as_weak();
    let ctx = ctx.clone();
    win.unwrap().on_theme_persist(move |i| {
        // DS-51: a persist failure notifies and reverts the segment
        // to the stored choice instead of failing silently.
        // R2-31: the segment shows the STORED choice (`theme-choice`,
        // 0/1/2); only the resolved value (0/1) goes to `theme`,
        // so Auto stays visibly selected and never flashes Dark.
        if !write_theme(ctx.data_dir.as_ref(), i) {
            let stored = ctx.theme_choice.get();
            if let Some(w) = win.upgrade() {
                w.set_theme_choice(stored);
                w.set_theme(resolve_theme_mode(
                    stored,
                    ctx.os_light.load(Ordering::Relaxed),
                ));
                ctx.notify(
                    &w,
                    "Settings",
                    "Couldn't save theme",
                    "Your theme choice wasn't saved — it will reset next time you open RSH.",
                );
                // DS-24: a persist failure is a failure — show it in
                // place as well as in the drawer.
                ctx.set_toast(
                    &w,
                    "Couldn't save theme",
                    "Your theme choice wasn't saved — it will reset next time you open RSH.",
                );
            }
            return;
        }
        ctx.theme_choice.set(i);
        if let Some(w) = win.upgrade() {
            // R2-31: Rust owns both props on change — the STORED
            // choice for the segment, the RESOLVED 0/1 for
            // `Theme.mode`. (Slint also writes `theme-choice` from
            // the segment callback; this re-asserts it so the two
            // can never drift.)
            w.set_theme_choice(i);
            // DS-49: the OS preference comes from the cached sampler
            // flag — no `reg` spawn on the UI thread.
            w.set_theme_choice(i);
            w.set_theme(resolve_theme_mode(i, ctx.os_light.load(Ordering::Relaxed)));
        }
    });
}

/// Wire the notification bell / drawer callbacks (Phase 6).
/// Bell toggles the drawer; the drawer buttons close it, mark all
/// read, or clear it. Esc is NOT handled here — the keyboard worker
/// owns key handling and closes the drawer via `notif-open`.
fn wire_notifications(win: &MainWindow, ctx: &Rc<Ctx>) {
    {
        let win = win.as_weak();
        win.unwrap().on_bell_clicked(move || {
            if let Some(w) = win.upgrade() {
                let open = !w.get_notif_open();
                w.set_notif_open(open);
                ui_log!(
                    "RSH-UI notifications drawer {}",
                    if open { "opened" } else { "closed" }
                );
            }
        });
    }
    {
        let win = win.as_weak();
        win.unwrap().on_notif_close(move || {
            if let Some(w) = win.upgrade() {
                w.set_notif_open(false);
                ui_log!("RSH-UI notifications drawer closed");
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_notif_mark_all_read(move || {
            ctx.notifications.borrow_mut().mark_all_read();
            ctx.persist_notifications();
            if let Some(w) = win.upgrade() {
                sync_to_window(&ctx.notifications.borrow(), &w);
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_notif_clear(move || {
            ctx.notifications.borrow_mut().clear();
            ctx.persist_notifications();
            if let Some(w) = win.upgrade() {
                sync_to_window(&ctx.notifications.borrow(), &w);
            }
        });
    }
    {
        // B01: the status bar's ✕ dismisses the message; Details
        // opens the notifications drawer (the full text lives there).
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_status_dismiss(move || {
            if let Some(w) = win.upgrade() {
                ctx.clear_status(&w);
                ui_log!("RSH-UI status bar dismissed");
            }
        });
    }
    {
        let win = win.as_weak();
        win.unwrap().on_status_details(move || {
            if let Some(w) = win.upgrade() {
                w.set_notif_open(true);
                ui_log!("RSH-UI status bar Details opened notifications drawer");
            }
        });
    }
}

/// Wire the tuning callbacks (deadzone / curve / preset / reset / save).
/// Tuning changes mark the profile unsaved (REDESIGN-PLAN §3.6).
/// A24: the curve slider is 1.0..2.0 (exponent = 1.0 + v * 1.0).
/// R2-13: no synchronous `refresh()` per move — the 50 ms frame
/// timer repaints the new local value within one frame, and a full
/// refresh per mouse-move ran `push_frame` + capture sync tens of
/// times per second on top of the timer. Only the debounced daemon
/// push (`push_steering`) still runs.
/// (S2+S3: the balance / trim callbacks pushed this over the
/// 100-line lint — same allow as the other wire_* functions.)
#[allow(clippy::too_many_lines)]
fn wire_tuning(win: &MainWindow, ctx: &Rc<Ctx>) {
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_deadzone_changed(move |v| {
            ctx.tuning.borrow_mut().set_deadzone(v * 30.0);
            ctx.tuning.borrow_mut().set_preset(3);
            ctx.push_steering();
            // R2-13: no full `refresh` per move — the 50 ms timer
            // paints the new local value within one frame.
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_curve_changed(move |v| {
            ctx.tuning.borrow_mut().set_exponent(1.0 + v * 1.0);
            ctx.tuning.borrow_mut().set_preset(3);
            ctx.push_steering();
            // R2-13: no full `refresh` per move — the 50 ms timer
            // paints the new local value within one frame.
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_deadzone_edited(move |t| {
            if let Ok(pct) = t.trim().trim_end_matches('%').parse::<f32>() {
                ctx.tuning.borrow_mut().set_deadzone(pct);
                ctx.tuning.borrow_mut().set_preset(3);
                ctx.push_steering();
            }
            // R2-13: no full `refresh` per edit — the 50 ms timer
            // paints the new local value within one frame.
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_curve_edited(move |t| {
            if let Ok(exp) = t.trim().parse::<f32>() {
                ctx.tuning.borrow_mut().set_exponent(exp);
                ctx.tuning.borrow_mut().set_preset(3);
                ctx.push_steering();
            }
            // R2-13: no full `refresh` per edit — the 50 ms timer
            // paints the new local value within one frame.
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_invert_changed(move |b| {
            ctx.tuning.borrow_mut().set_invert(b);
            ctx.push_steering();
            // R2-13: no full `refresh` per move — the 50 ms timer
            // paints the new local value within one frame.
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_lock_changed(move |v| {
            // Slider fraction over the 270–900° range, snapped to
            // the nearest lock step by `set_lock_deg`.
            ctx.tuning.borrow_mut().set_lock_deg(270.0 + v * 630.0);
            ctx.push_steering();
            // R2-13: no full `refresh` per move — the 50 ms timer
            // paints the new local value within one frame.
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_lock_edited(move |t| {
            if let Ok(deg) = t.trim().trim_end_matches(['°', ' ']).parse::<f32>() {
                ctx.tuning.borrow_mut().set_lock_deg(deg);
                ctx.push_steering();
            }
            // R2-13: no full `refresh` per edit — the 50 ms timer
            // paints the new local value within one frame.
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_smoothing_changed(move |v| {
            ctx.tuning.borrow_mut().set_smoothing(v * 100.0);
            ctx.push_steering();
            // R2-13: no full `refresh` per move — the 50 ms timer
            // paints the new local value within one frame.
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_smoothing_edited(move |t| {
            if let Ok(pct) = t.trim().trim_end_matches('%').parse::<f32>() {
                ctx.tuning.borrow_mut().set_smoothing(pct);
                ctx.push_steering();
            }
            // R2-13: no full `refresh` per edit — the 50 ms timer
            // paints the new local value within one frame.
        });
    }
    // --- Steering per-module tuning ---
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_balance_changed(move |v| {
            // Slider fraction over the −20..+20% range.
            ctx.tuning.borrow_mut().set_balance(v * 40.0 - 20.0);
            ctx.push_steering();
            // R2-13: no full `refresh` per move — the 50 ms timer
            // paints the new local value within one frame.
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_balance_edited(move |t| {
            if let Ok(pct) = t.trim().trim_end_matches('%').parse::<f32>() {
                ctx.tuning.borrow_mut().set_balance(pct);
                ctx.push_steering();
            }
            // R2-13: no full `refresh` per edit — the 50 ms timer
            // paints the new local value within one frame.
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_trim_changed(move |v| {
            // Slider fraction over the −5..+5% range.
            ctx.tuning.borrow_mut().set_trim(v * 10.0 - 5.0);
            ctx.push_steering();
            // R2-13: no full `refresh` per move — the 50 ms timer
            // paints the new local value within one frame.
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_trim_edited(move |t| {
            if let Ok(pct) = t.trim().trim_end_matches('%').parse::<f32>() {
                ctx.tuning.borrow_mut().set_trim(pct);
                ctx.push_steering();
            }
            // R2-13: no full `refresh` per edit — the 50 ms timer
            // paints the new local value within one frame.
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_preset_selected(move |p| {
            ctx.tuning.borrow_mut().set_preset(p);
            ctx.push_steering();
            // R2-13: no full `refresh` per select — the 50 ms timer
            // paints the new local value within one frame.
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_reset_defaults(move || {
            ctx.tuning.borrow_mut().reset_defaults();
            ctx.push_steering();
            // R2-13: no full `refresh` per reset — the 50 ms timer
            // paints the new local value within one frame.
        });
    }
}

/// Wire the keyboard callbacks (FIX-4: the root `FocusScope` in
/// `ui/main.slint` captures Return / Up / Down / Escape and the per-control
/// `FocusScope`s capture Enter/Space on the control itself; this function
/// owns what each window-level key DOES).
///
/// - `keyboard-save` (Return when no focused control consumes it): Save
///   on the device pages (1..=4), where the top-bar Save button lives;
///   elsewhere a no-op.
/// - `keyboard-nav` (Up/Down): move through the navigator pages 0..=5
///   (About is 6, reached from the sidebar footer, not the navigator).
/// - `keyboard-escape`: closes the game dialog first, then the
///   profile popover, then the
///   notifications drawer, then the diagnostics drawer (v2.0.1 hotfix).
/// - `focus-keyboard`: focuses the root keyboard scope once at startup so
///   keys work before the user Tabs anywhere.
fn wire_keyboard(win: &MainWindow, ctx: &Rc<Ctx>) {
    {
        let win = win.as_weak();
        win.unwrap().on_keyboard_save(move || {
            if let Some(w) = win.upgrade() {
                let page = w.get_page();
                if (1..=4).contains(&page) {
                    w.invoke_save_profile();
                } else {
                    ui_log!("RSH-UI keyboard-save ignored on page {page} (no Save there)");
                }
            }
        });
    }
    {
        let win = win.as_weak();
        win.unwrap().on_keyboard_nav(move |delta| {
            if let Some(w) = win.upgrade() {
                let next = (w.get_page() + delta).rem_euclid(6);
                w.set_page(next);
                ui_log!("RSH-UI keyboard-nav page={next}");
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_keyboard_escape(move || {
            if let Some(w) = win.upgrade() {
                // D16: Esc dismisses only the TOPMOST overlay /
                // status-bar message — never the wizard itself
                // (closing the wizard needs the explicit
                // Skip/Finish path).
                if w.get_confirm_open() {
                    w.set_confirm_open(false);
                    *ctx.confirm_pending.borrow_mut() = None;
                    ui_log!("RSH-UI keyboard-escape closed confirm dialog");
                } else if w.get_status_title() != "" {
                    ctx.clear_status(&w);
                    ui_log!("RSH-UI keyboard-escape dismissed status bar");
                } else if w.get_game_dialog_open() {
                    w.set_game_dialog_open(false);
                    ui_log!("RSH-UI keyboard-escape closed game dialog");
                } else if w.get_popover_open() {
                    w.set_popover_open(false);
                    ui_log!("RSH-UI keyboard-escape closed profile popover");
                } else if w.get_notif_open() {
                    w.set_notif_open(false);
                    ui_log!("RSH-UI keyboard-escape closed notifications drawer");
                } else if w.get_diag_open() {
                    w.set_diag_open(false);
                    ui_log!("RSH-UI keyboard-escape closed diagnostics drawer");
                } else {
                    ui_log!("RSH-UI keyboard-escape (no overlay open)");
                }
            }
        });
    }
    // Focus the root keyboard scope so keys work before the first Tab.
    win.invoke_focus_keyboard();
}

/// Wire Reconnect (A9): the probe + spawn runs on a background job
/// thread (see [`Ctx::start_reconnect_job`]) — the click returns
/// immediately with the "Connecting…" state, and the verdict arrives
/// via the job channel. A second click while one is in flight is
/// ignored instead of queueing another probe.
fn wire_reconnect(win: &MainWindow, ctx: &Rc<Ctx>) {
    let win = win.as_weak();
    let ctx = ctx.clone();
    win.unwrap().on_reconnect(move || {
        let Some(w) = win.upgrade() else {
            return;
        };
        ctx.start_reconnect_job(&w);
    });
}

/// Wire Set centre + Launch game (A14/A22): Set centre captures
/// the live centre (v2.0.2). Launch game works off the real games
/// list: running → Stop, installed → Launch, not installed → honest
/// refusal.
#[allow(clippy::too_many_lines)]
fn wire_action_notifications(win: &MainWindow, ctx: &Rc<Ctx>) {
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_set_centre(move || {
            if let Some(w) = win.upgrade() {
                // v2.0.2: capture the live centre over IPC. F6: the
                // capture runs on a CaptureOp job thread.
                if ctx.is_live() {
                    ctx.run_generic_job(
                        &w,
                        JobKind::CaptureOp,
                        "Setting centre…",
                        move |api| JobResult::Capture(api.capture_centre()),
                        move |ctx, w, result| {
                            let JobResult::Capture(result) = result else {
                                return;
                            };
                            match result {
                                Ok(mv) => {
                                    ctx.refresh_calibration();
                                    ctx.notify(
                                        w,
                                        "Steering",
                                        "Centre set",
                                        &format!("Centre set at {mv} mV."),
                                    );
                                }
                                Err(e) => {
                                    ctx.notify(w, "Steering", "Couldn't set centre", &e.title);
                                    ui_log!("RSH-UI set centre failed: {}", e.title);
                                }
                            }
                        },
                    );
                } else {
                    let reason = Ctx::capture_unavailable_reason();
                    ctx.notify(&w, "Steering", "Couldn't set centre", reason);
                    ui_log!("RSH-UI set centre failed: {reason}");
                }
            }
        });
    }
    // --- Steering per-module tuning (S1) ---
    // Capture full lock: turn fully left / right, press. Same
    // CaptureOp job shape as Set centre — the capture runs on a
    // job thread and the result is a toast, never silent.
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_capture_full_left(move || {
            if let Some(w) = win.upgrade() {
                if ctx.is_live() {
                    ctx.run_generic_job(
                        &w,
                        JobKind::CaptureOp,
                        "Capturing full left…",
                        move |api| JobResult::Capture(api.capture_steering_left()),
                        move |ctx, w, result| {
                            let JobResult::Capture(result) = result else {
                                return;
                            };
                            match result {
                                Ok(mv) => {
                                    ctx.refresh_calibration();
                                    ctx.notify(
                                        w,
                                        "Steering",
                                        "Full left captured",
                                        &format!("Full left captured at {mv} mV."),
                                    );
                                }
                                Err(e) => {
                                    ctx.notify(
                                        w,
                                        "Steering",
                                        "Couldn't capture full left",
                                        &e.title,
                                    );
                                    ui_log!("RSH-UI capture full left failed: {}", e.title);
                                }
                            }
                        },
                    );
                } else {
                    let reason = Ctx::capture_unavailable_reason();
                    ctx.notify(&w, "Steering", "Couldn't capture full left", reason);
                    ui_log!("RSH-UI capture full left failed: {reason}");
                }
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_capture_full_right(move || {
            if let Some(w) = win.upgrade() {
                if ctx.is_live() {
                    ctx.run_generic_job(
                        &w,
                        JobKind::CaptureOp,
                        "Capturing full right…",
                        move |api| JobResult::Capture(api.capture_steering_right()),
                        move |ctx, w, result| {
                            let JobResult::Capture(result) = result else {
                                return;
                            };
                            match result {
                                Ok(mv) => {
                                    ctx.refresh_calibration();
                                    ctx.notify(
                                        w,
                                        "Steering",
                                        "Full right captured",
                                        &format!("Full right captured at {mv} mV."),
                                    );
                                }
                                Err(e) => {
                                    ctx.notify(
                                        w,
                                        "Steering",
                                        "Couldn't capture full right",
                                        &e.title,
                                    );
                                    ui_log!("RSH-UI capture full right failed: {}", e.title);
                                }
                            }
                        },
                    );
                } else {
                    let reason = Ctx::capture_unavailable_reason();
                    ctx.notify(&w, "Steering", "Couldn't capture full right", reason);
                    ui_log!("RSH-UI capture full right failed: {reason}");
                }
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_launch_game(move |id| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let id = id.to_string();
            let Some(game) = ctx
                .games_cache
                .borrow()
                .iter()
                .find(|g| g.id == id)
                .cloned()
            else {
                ctx.notify(
                    &w,
                    "Game",
                    "Game not found",
                    "That game isn't in the list any more.",
                );
                return;
            };
            if !game.installed {
                ctx.notify(
                    &w,
                    "Game",
                    "Game not installed",
                    &format!("{} isn't installed.", game.name),
                );
                return;
            }
            let game_id = game.id.clone();
            let game_name = game.name.clone();
            let running = game.running;
            // F6: launch / stop runs on a GameOp job thread — the
            // click never blocks on the daemon.
            ctx.run_generic_job(
                &w,
                JobKind::GameOp,
                if running {
                    "Stopping game…"
                } else {
                    "Launching game…"
                },
                move |api| {
                    JobResult::Game(if running {
                        api.stop_game(&game_id).map(|g| g.running)
                    } else {
                        api.launch_game(&game_id).map(|g| g.running)
                    })
                },
                move |ctx, w, result| {
                    let JobResult::Game(result) = result else {
                        return;
                    };
                    match result {
                        Ok(is_running) => {
                            ctx.refresh_games(w);
                            let body = if is_running {
                                format!("{game_name} launched.")
                            } else {
                                format!("{game_name} stopped.")
                            };
                            ctx.notify(
                                w,
                                "Game",
                                if is_running {
                                    "Game launched"
                                } else {
                                    "Game stopped"
                                },
                                &body,
                            );
                        }
                        Err(e) => {
                            ctx.notify(w, "Game", "Couldn't launch game", &e.title);
                            ui_log!("RSH-UI launch-game failed: {} ({})", e.code, e.title);
                        }
                    }
                },
            );
        });
    }
}

/// Wire the profile popover (A8): toggle from the top-bar
/// `ProfileButton` / Overview profile box, real list + actions.
#[allow(clippy::too_many_lines)]
fn wire_popover(win: &MainWindow, ctx: &Rc<Ctx>) {
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_profile_clicked(move || {
            if let Some(w) = win.upgrade() {
                if w.get_popover_open() {
                    w.set_popover_open(false);
                    ui_log!("RSH-UI profile popover closed");
                } else {
                    ctx.refresh_profiles(&w);
                    w.set_popover_open(true);
                    ui_log!("RSH-UI profile popover opened");
                }
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_popover_use_preset(move |i| {
            if let Some(w) = win.upgrade() {
                // M1: presets are one row per catalogue game
                // (read-only templates) — "Use" creates a profile
                // from that game and activates it.
                ctx.use_preset(&w, i);
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_popover_use_profile(move |i| {
            if let Some(w) = win.upgrade() {
                ctx.request_activate(&w, i);
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_popover_new_profile(move || {
            if let Some(w) = win.upgrade() {
                ctx.create_new_profile(&w);
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_popover_duplicate(move |i| {
            if let Some(w) = win.upgrade() {
                // M1: the footer passes a COMBINED index (presets
                // first) — a preset row duplicates by creating a
                // profile from its game; a my-profile row duplicates
                // the profile itself.
                if let Some(my) = ctx.popover_my_index(i) {
                    ctx.duplicate_profile_index(&w, my);
                } else {
                    ctx.use_preset(&w, i);
                }
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_popover_rename(move |i| {
            if let Some(w) = win.upgrade() {
                // M1: renames the profile to its linked game's name
                // (the old no-text-input path — kept on purpose).
                // Free-text rename is `on_popover_rename_confirmed`
                // (see `wire_fc2_callbacks`).
                ctx.rename_profile_index(&w, i);
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_popover_delete(move |i| {
            if let Some(w) = win.upgrade() {
                // M1: the footer passes a COMBINED index — presets
                // are read-only and cannot be deleted.
                if let Some(my) = ctx.popover_my_index(i) {
                    // D17: deleting a profile is destructive —
                    // confirm first (small dialog, not a silent
                    // delete).
                    let name = ctx
                        .profile_at(my)
                        .map_or_else(|| "this profile".to_string(), |p| p.name.clone());
                    ctx.open_confirm(
                        &w,
                        "Delete profile",
                        &format!("Delete {name}? This can't be undone."),
                        "Delete",
                        ConfirmAction::DeleteProfile(my),
                    );
                } else {
                    ctx.notify_toast(
                        &w,
                        "Profile",
                        "Presets are read-only",
                        "Presets are read-only — duplicate one to edit it.",
                    );
                }
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_popover_link(move |i| {
            if let Some(w) = win.upgrade() {
                // M1: each click links the profile to its next game.
                ctx.link_profile_index(&w, i);
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_popover_save_switch(move || {
            if let Some(w) = win.upgrade() {
                ctx.resolve_pending_switch(&w, true);
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_popover_discard_switch(move || {
            if let Some(w) = win.upgrade() {
                ctx.resolve_pending_switch(&w, false);
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_popover_cancel_switch(move || {
            if let Some(w) = win.upgrade() {
                *ctx.pending_profile_id.borrow_mut() = None;
                w.set_popover_show_prompt(false);
            }
        });
    }
    {
        let win = win.as_weak();
        win.unwrap().on_popover_closed(move || {
            if let Some(w) = win.upgrade() {
                w.set_popover_open(false);
                w.set_popover_show_prompt(false);
            }
        });
    }
}

/// Wire the Settings transport / backend / autostart switches
/// (A9/A10): the switch flips first (two-way binding), Rust calls
/// the backend and REVERTS the switch on failure — never a silent
/// local-only change.
#[allow(clippy::too_many_lines)]
fn wire_transport(win: &MainWindow, ctx: &Rc<Ctx>) {
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_transport_selected(move |i| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let transport = if i == 1 {
                ApiTransport::Usb
            } else {
                ApiTransport::Wifi
            };
            // A9: the switch runs on a background job — the click
            // returns immediately; a failure reverts the switch in
            // the job drain (never a silent local-only change).
            ctx.start_transport_job(&w, transport, Some(i));
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_output_backend_selected(move |i| {
            let Some(w) = win.upgrade() else {
                return;
            };
            // A9/B-2/A10: the switch runs on a background job with
            // its 75 s bound (UAC dwell + 60 s sidecar spawn) — the
            // click returns immediately; a failure (including a UAC
            // decline) reverts the switch in the job drain.
            ctx.start_backend_job(&w, i);
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_autostart_selected(move |enabled| {
            let Some(w) = win.upgrade() else {
                return;
            };
            // A10: the passed bool is the new state — never ignored.
            // F6: `set_autostart` runs on a SettingsOp job thread.
            ctx.run_generic_job(
                &w,
                JobKind::SettingsOp,
                "Saving…",
                move |api| JobResult::Bool(api.set_autostart(enabled)),
                move |ctx, w, result| {
                    let JobResult::Bool(result) = result else {
                        return;
                    };
                    match result {
                        Ok(new_state) => {
                            w.set_autostart_enabled(new_state);
                            ctx.notify(
                                w,
                                "Startup",
                                "Start with Windows",
                                if new_state {
                                    "RSH will start with Windows."
                                } else {
                                    "RSH will not start with Windows."
                                },
                            );
                        }
                        Err(e) => {
                            w.set_autostart_enabled(!enabled);
                            ctx.notify(w, "Startup", "Autostart failed", &e.title);
                            ui_log!("RSH-UI set_autostart failed: {} ({})", e.code, e.title);
                        }
                    }
                },
            );
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_run_setup_again(move || {
            if let Some(w) = win.upgrade() {
                // A18: delete the marker and open the wizard at step 0.
                ctx.open_setup_wizard(&w);
                ui_log!("RSH-UI run setup again (wizard reopened)");
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_settings_use_profile(move |i| {
            if let Some(w) = win.upgrade() {
                ctx.request_activate(&w, i);
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_settings_duplicate_profile(move |i| {
            if let Some(w) = win.upgrade() {
                ctx.duplicate_profile_index(&w, i);
            }
        });
    }
}

/// Report the wheel firmware version from Settings (M-8): a
/// background job compares the board's live version against the
/// bundled OTA image version — never the hardcoded "4.0.2". The
/// verdict (up to date / update available / unavailable) arrives via
/// the job drain.
fn settings_check_firmware(w: &MainWindow, ctx: &Ctx) {
    ctx.start_firmware_check_job(w);
}

/// Wire the Settings screen actions (v2.0.1 hotfix). Every
/// action notifies visibly on success AND failure — never
/// silent, never eprintln-only.
fn wire_settings_action(win: &MainWindow, ctx: &Rc<Ctx>) {
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_settings_action(move |a| {
            let Some(w) = win.upgrade() else {
                return;
            };
            match a.as_str() {
                "address" => {
                    // H4/FC2: fallback only — nothing in the .slint
                    // fires this any more (the address field fires
                    // `address-accepted(string)`, wired in
                    // `wire_fc2_callbacks`). Kept so a stray call
                    // still answers instead of hitting the unknown
                    // arm; throttled to one note per 3 s.
                    let now = Instant::now();
                    let due = ctx
                        .last_address_notify
                        .get()
                        .is_none_or(|last| now.duration_since(last) >= Duration::from_secs(3));
                    if due {
                        ctx.last_address_notify.set(Some(now));
                        ctx.notify(
                            &w,
                            "Settings",
                            "Address",
                            "Address applies when you reconnect",
                        );
                    }
                }
                "tray" => {
                    // M7/FC2: fallback only — the switch fires
                    // `keep-tray-selected(bool)` now (wired in
                    // `wire_fc2_callbacks`), which persists the
                    // value. This arm only answers a stray call.
                    ctx.notify(
                        &w,
                        "Settings",
                        "Keep in tray",
                        "RSH keeps running in the tray.",
                    );
                }
                "new-profile" => ctx.create_new_profile(&w),
                "check-firmware" => settings_check_firmware(&w, &ctx),
                // H-4: start the firmware flash (the daemon validates
                // the bundled image + runs the minutes-long flash on
                // its own thread; progress is polled on the 1 Hz tick).
                // The Settings button itself is a .slint addition (see
                // the report) — this arm is wired and waiting for it.
                "update-firmware" => ctx.start_firmware_update_job(&w),
                // DS-20: a real check now — GitHub Releases latest
                // tag vs the running version (was a bare notify
                // that answered without checking anything).
                "update-check" => ctx.start_app_update_check_job(&w),
                // F6: backup / restore run on a SettingsOp job
                // thread — the click never blocks on the daemon.
                "backup" => {
                    ctx.run_generic_job(
                        &w,
                        JobKind::SettingsOp,
                        "Saving backup…",
                        move |api| JobResult::Text(api.backup_now()),
                        move |ctx, w, result| {
                            let JobResult::Text(result) = result else {
                                return;
                            };
                            match result {
                                Ok(path) => {
                                    // C3: the backup line updates after a backup.
                                    w.set_backup_text("Saved just now".into());
                                    ctx.notify(
                                        w,
                                        "Backup",
                                        "Backup saved",
                                        &format!("Backup saved to {path}"),
                                    );
                                }
                                Err(e) => ctx.notify_error(w, "Backup", &e),
                            }
                        },
                    );
                }
                "restore" => {
                    ctx.run_generic_job(
                        &w,
                        JobKind::SettingsOp,
                        "Restoring backup…",
                        move |api| JobResult::Text(api.restore_backup()),
                        move |ctx, w, result| {
                            let JobResult::Text(result) = result else {
                                return;
                            };
                            match result {
                                Ok(message) => {
                                    ctx.notify(w, "Backup", "Backup restored", &message);
                                    ctx.refresh_profiles(w);
                                }
                                Err(e) => ctx.notify_error(w, "Backup", &e),
                            }
                        },
                    );
                }
                other => ui_log!("RSH-UI unknown settings-action: {other}"),
            }
        });
    }
}

/// Wire the diagnostics drawer opener (v2.0.1 hotfix): syncs
/// the drawer props, then opens it. Esc and the drawer's Close
/// button close it (see `wire_keyboard`).
fn wire_diagnostics_opener(win: &MainWindow, ctx: &Rc<Ctx>) {
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_open_diagnostics(move || {
            if let Some(w) = win.upgrade() {
                ctx.sync_diag(&w);
                w.set_diag_open(true);
                ui_log!("RSH-UI diagnostics drawer opened");
            }
        });
    }
}

/// Wire the remaining callbacks (save-profile, send-to-game).
/// Reconnect, the notify-only actions, the Settings actions and
/// the diagnostics opener live in their own functions above
/// (v2.0.1 hotfix). Save-profile notifies visibly now (A23) —
/// never silent.
fn wire_actions(win: &MainWindow, ctx: &Rc<Ctx>) {
    wire_reconnect(win, ctx);
    wire_action_notifications(win, ctx);
    wire_settings_action(win, ctx);
    wire_diagnostics_opener(win, ctx);
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_save_profile(move || {
            let Some(w) = win.upgrade() else {
                return;
            };
            // F6: `save_draft` runs on a ProfileOp job thread.
            ctx.run_generic_job(
                &w,
                JobKind::ProfileOp,
                "Saving changes…",
                move |api| JobResult::Bool(api.save_draft()),
                move |ctx, w, result| {
                    let JobResult::Bool(result) = result else {
                        return;
                    };
                    match result {
                        Ok(true) => {
                            ctx.unsaved.set(false);
                            ctx.notify(w, "Profile", "Changes saved", "Changes saved.");
                        }
                        Ok(false) => {
                            ctx.notify(w, "Profile", "Nothing to save", "No changes to save.");
                        }
                        Err(e) => {
                            ctx.notify_error(w, "Profile", &e);
                            ui_log!("RSH-UI save-profile failed: {} ({})", e.code, e.title);
                        }
                    }
                },
            );
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_toggled_send(move || {
            // M6: send-to-game persists + applies in the daemon
            // (the trait's `set_output_enabled` is infallible `()`
            // — there is no failure to revert on at this level; the
            // daemon stores it in settings and applies it to the
            // output path). F6: the persist runs on a SettingsOp
            // job thread — the toggle never blocks on the daemon.
            // `output_enabled()` is a cache read (F5).
            let Some(w) = win.upgrade() else {
                return;
            };
            let next = !ctx.api().output_enabled();
            ctx.run_generic_job(
                &w,
                JobKind::SettingsOp,
                "Updating output…",
                move |api| {
                    api.set_output_enabled(next);
                    JobResult::Unit(Ok(()))
                },
                move |_ctx, w, _result| {
                    ui_log!("RSH-UI send-to-game={next}");
                    w.set_send_to_game(next);
                },
            );
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_auto_pause_toggled(move || {
            if let Some(w) = win.upgrade() {
                // M6: auto-pause persists in settings and applies
                // (the daemon stops auto-switching while paused) —
                // never a local-only prop flip. F6: the persist runs
                // on a SettingsOp job thread.
                let next = !w.get_auto_paused();
                ctx.run_generic_job(
                    &w,
                    JobKind::SettingsOp,
                    "Saving…",
                    move |api| JobResult::Bool(api.set_auto_pause(next)),
                    move |ctx, w, result| {
                        let JobResult::Bool(result) = result else {
                            return;
                        };
                        match result {
                            Ok(stored) => {
                                w.set_auto_paused(stored);
                                ctx.notify(
                                    w,
                                    "Profile",
                                    "Auto-switch",
                                    if stored {
                                        "Auto-switch paused — the active profile stays put."
                                    } else {
                                        "Auto-switch resumed."
                                    },
                                );
                                ui_log!("RSH-UI auto-profile paused={stored}");
                            }
                            Err(e) => {
                                w.set_auto_paused(!next);
                                ctx.notify_error(w, "Profile", &e);
                            }
                        }
                    },
                );
            }
        });
    }
}

/// Log one drained push event (DS-79 / F-10): routed through the
/// buffered `ui_log!` (file log, always-on) instead of an
/// unbuffered `eprintln!` per event on the UI thread. The macro
/// still gates the stderr copy on `--verbose`, and `Snapshot`
/// stays unlogged — it fires at poll rate and is not an event.
/// Notification events are ALSO fed to the [`NotificationCenter`]
/// by the timer drain (see `main`); this function only logs, it
/// never stores.
fn log_event(ev: &AppEvent) {
    match ev {
        AppEvent::ConnectionChanged {
            state,
            label,
            reason,
        } => {
            ui_log!(
                "RSH-UI event ConnectionChanged state={state:?} label={label} reason={reason:?}"
            );
        }
        AppEvent::Snapshot(_) => {}
        AppEvent::Notification { kind, title, body } => {
            ui_log!("RSH-UI event Notification kind={kind} title={title} body={body}");
        }
        AppEvent::JobProgress {
            id,
            state,
            progress,
            error,
        } => {
            ui_log!(
                "RSH-UI event JobProgress id={id} state={state} progress={progress:?} error={error:?}"
            );
        }
    }
}

/// React to one drained live event: Notification events feed
/// the bell / drawer center (other events are ignored by it).
/// (D16: no pairing events exist any more — the wizard's Connect
/// step follows `board_reachable` from `push_frame` instead.)
fn handle_live_event(win: &slint::Weak<MainWindow>, ctx: &Ctx, ev: &AppEvent) {
    let pushed = ctx.notifications.borrow_mut().push_event(ev);
    if pushed {
        // M10: daemon events persist like any other notification.
        ctx.persist_notifications();
        // F27: same hidden-window guard as `Ctx::notify` — the
        // model write is skipped while hidden (the entry is kept
        // in the center + on disk; `resync_after_reopen` syncs it
        // when the window comes back).
        if let Some(w) = win.upgrade() {
            if !ctx.hidden.load(Ordering::SeqCst) {
                sync_to_window(&ctx.notifications.borrow(), &w);
            }
        }
    }
}

/// Drain the health monitor's verdicts on the UI thread (B2): a
/// death clears stale values, a re-probe swaps live data in.
fn drain_health(
    rx: &std::sync::mpsc::Receiver<HealthEvent>,
    win: &slint::Weak<MainWindow>,
    ctx: &Ctx,
) {
    while let Ok(event) = rx.try_recv() {
        match event {
            HealthEvent::DaemonDown => {
                if let Some(w) = win.upgrade() {
                    ctx.mark_dead(&w);
                }
            }
            HealthEvent::DaemonBack(api) => {
                if let Some(w) = win.upgrade() {
                    ctx.reconnect_to_live(&w, api);
                }
            }
            HealthEvent::DaemonFatal { code, detail } => {
                if let Some(w) = win.upgrade() {
                    ctx.mark_spawn_latched(&w, &code, &detail);
                }
            }
        }
    }
}

/// Publish a found live API into the shared slot (if still empty)
/// and report `DaemonBack`; returns false only when the UI went
/// away (the monitor thread exits then). A slot Reconnect filled
/// first is not an error — the monitor just stands down.
fn publish_daemon_back(
    live_link: &Arc<Mutex<Option<Arc<IpcApi>>>>,
    tx: &std::sync::mpsc::Sender<HealthEvent>,
    api: Arc<IpcApi>,
) -> bool {
    let mut published = false;
    if let Ok(mut slot) = live_link.lock() {
        if slot.is_none() {
            *slot = Some(api.clone());
            published = true;
        }
    }
    if published {
        tx.send(HealthEvent::DaemonBack(api)).is_ok()
    } else {
        true
    }
}

/// Report `DaemonDown` once per down-episode; returns false only
/// when the UI went away.
fn send_daemon_down_once(
    tx: &std::sync::mpsc::Sender<HealthEvent>,
    down_notified: &mut bool,
) -> bool {
    if *down_notified {
        return true;
    }
    if tx.send(HealthEvent::DaemonDown).is_err() {
        return false;
    }
    *down_notified = true;
    true
}

/// Report `DaemonFatal` once per distinct reason; returns false
/// only when the UI went away. A latched daemon must not spam the
/// drawer once per poll.
fn send_daemon_fatal_once(
    tx: &std::sync::mpsc::Sender<HealthEvent>,
    fatal_sent: &mut Option<String>,
    code: String,
    detail: String,
) -> bool {
    let reason = format!("{code}:{detail}");
    if fatal_sent.as_deref() == Some(&reason) {
        return true;
    }
    if tx.send(HealthEvent::DaemonFatal { code, detail }).is_err() {
        return false;
    }
    *fatal_sent = Some(reason);
    true
}

/// Spawn the health-monitor thread (B2/B4): ping the live daemon
/// every second; on a death clear the shared link and report it,
/// then re-probe (spawning the daemon when needed) until it answers
/// again. The thread only talks to the UI through `tx` — the frame
/// timer applies the verdicts on the UI thread — and it owns no
/// `Rc` state, only the shared link. It exits when the UI drops
/// the receiver (app quit).
///
/// Perf (F-1) — the 1 Hz probe is folded into the fast poller:
/// the monitor reads the poller's debounced `link_dead` flag
/// (`IpcApi::link_dead()`, trips after 3 failed poll rounds)
/// instead of issuing its own `ping()` IPC round-trip every
/// second. The flag read costs no IPC; the `PING_DEBOUNCE`
/// counting below is kept so death is still declared after
/// ~3 s of a dead link, exactly as before.
///
/// F3 (v2.0.3): the respawn is bounded, not a loop —
/// * death needs [`live_probe::PING_DEBOUNCE`] consecutive
///   monitor ticks with the link-dead flag set (~3 s), not one;
/// * a fresh fatal `daemon-status.json` blocks spawning entirely
///   (the daemon died on purpose — show it, don't re-run it);
/// * spawn attempts back off exponentially (1 s → 5 s → 30 s →
///   60 s cap) and stop after [`live_probe::MAX_FAILED_SPAWNS`]
///   consecutive failures, latching the visible fatal state
///   (`DaemonFatal`) with a manual Retry;
/// * while the latch holds, the monitor only cheap-probes (the
///   daemon may come back via a manual start) and never spawns.
#[allow(clippy::too_many_lines)]
fn spawn_health_monitor(
    data_dir: Option<PathBuf>,
    live_link: Arc<Mutex<Option<Arc<IpcApi>>>>,
    tx: std::sync::mpsc::Sender<HealthEvent>,
    hidden: Arc<AtomicBool>,
    budget: Arc<live_probe::SpawnBudget>,
) {
    let _ = std::thread::Builder::new()
        .name("rsh-health".to_string())
        .spawn(move || {
            let mut down_notified = false;
            let mut failed_pings = 0_u32;
            let mut fatal_sent: Option<String> = None;
            loop {
                let current = live_link
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                if let Some(api) = current {
                    // Perf (F-1): no separate 1 Hz `ping()` IPC
                    // round-trip — the fast poller already proves
                    // the link dead (see `IpcApi::link_dead()`).
                    if api.link_dead() {
                        failed_pings += 1;
                        if failed_pings >= live_probe::PING_DEBOUNCE {
                            failed_pings = 0;
                            // Only declare death when this api is
                            // still the live one — Reconnect may have
                            // swapped in a fresh link while these
                            // pings were in flight, and a stale
                            // `DaemonDown` would kill the new link.
                            let mut still_current = false;
                            if let Ok(mut slot) = live_link.lock() {
                                if let Some(cur) = slot.as_ref() {
                                    if Arc::ptr_eq(cur, &api) {
                                        *slot = None;
                                        still_current = true;
                                    }
                                }
                            }
                            if still_current {
                                // F25: the link that just died is
                                // charged against the budget when it
                                // flapped (died within FLAP_WINDOW
                                // of going live) — a flap daemon
                                // must not respawn forever on
                                // `record_success` resets. This is
                                // the one place a death is counted,
                                // so it can never double-count.
                                budget.record_live_link_lost();
                                if !send_daemon_down_once(&tx, &mut down_notified) {
                                    return;
                                }
                            }
                        }
                    } else {
                        failed_pings = 0;
                    }
                } else if let Some(api) = live_probe::try_live_api(data_dir.as_deref()) {
                    // Cheap probe first (no spawn): the daemon may
                    // have come back on its own, or via Reconnect.
                    down_notified = false;
                    fatal_sent = None;
                    budget.record_success();
                    if !publish_daemon_back(&live_link, &tx, api) {
                        return;
                    }
                } else if hidden.load(Ordering::Relaxed) {
                    // H-1/A11: while the window is hidden to tray
                    // the monitor only probes for an EXISTING daemon
                    // — it must never spawn one, or a tray Quit
                    // (daemon gone on purpose) would resurrect it.
                    if !send_daemon_down_once(&tx, &mut down_notified) {
                        return;
                    }
                } else if let Some((code, detail)) = live_probe::read_daemon_status_fresh(
                    data_dir.as_deref(),
                    live_probe::FATAL_FRESHNESS,
                ) {
                    // F3: a fresh fatal file means the daemon died
                    // on purpose — spawning another one just re-runs
                    // the death loop. Latch it and show the reason.
                    if !send_daemon_fatal_once(&tx, &mut fatal_sent, code, detail) {
                        return;
                    }
                    if !send_daemon_down_once(&tx, &mut down_notified) {
                        return;
                    }
                    std::thread::sleep(Duration::from_secs(5));
                    continue;
                } else if budget.is_latched() {
                    // F3: budget exhausted — latch, don't spawn. The
                    // monitor keeps cheap-probing above: a manually
                    // started daemon still comes back.
                    if !send_daemon_fatal_once(
                        &tx,
                        &mut fatal_sent,
                        "RespawnBudgetExhausted".to_string(),
                        format!(
                            "RSH keeps failing to start — automatic restarts stopped after {} tries. Use Reconnect to try again.",
                            live_probe::MAX_FAILED_SPAWNS
                        ),
                    ) {
                        return;
                    }
                    if !send_daemon_down_once(&tx, &mut down_notified) {
                        return;
                    }
                    std::thread::sleep(Duration::from_secs(5));
                    continue;
                } else if !budget.auto_spawn_allowed() {
                    // A manual retry is in flight (Reconnect click):
                    // stay out of its way — no spawn, no fatal
                    // event — until it resolves. The cheap probe
                    // above already ran this cycle.
                    if !send_daemon_down_once(&tx, &mut down_notified) {
                        return;
                    }
                    std::thread::sleep(Duration::from_secs(5));
                    continue;
                } else {
                    // F3: exponential backoff before the next spawn
                    // attempt (1 s → 5 s → 30 s → 60 s cap).
                    let delay = budget
                        .next_delay()
                        .unwrap_or(Duration::from_secs(60));
                    std::thread::sleep(delay);
                    // Re-check: Reconnect may have filled the slot
                    // while we slept, or the daemon may have
                    // appeared on its own.
                    let current = live_link
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone();
                    if current.is_some() {
                        continue;
                    }
                    if let Some(api) = live_probe::try_live_api(data_dir.as_deref()) {
                        down_notified = false;
                        fatal_sent = None;
                        budget.record_success();
                        if !publish_daemon_back(&live_link, &tx, api) {
                            return;
                        }
                        continue;
                    }
                    // F4: the spawn itself is serialised inside
                    // `try_live_api_with_spawn` (guard) — a Reconnect
                    // attempt in flight means this probes only.
                    if let Some(api) = live_probe::try_live_api_with_spawn(data_dir.as_deref())
                    {
                        down_notified = false;
                        fatal_sent = None;
                        budget.record_success();
                        if !publish_daemon_back(&live_link, &tx, api) {
                            return;
                        }
                    } else {
                        budget.record_spawn_failure();
                        if !send_daemon_down_once(&tx, &mut down_notified) {
                            return;
                        }
                    }
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        });
}

/// Apply a page request forwarded by a second UI instance (M-5/A1):
/// restore/show the window (instant — no fresh load, the daemon link
/// is untouched) and navigate. `setup` re-opens the wizard on the
/// same path as Settings "Run setup again"; any other page exits
/// setup mode and selects that page.
fn apply_page_request(win: &MainWindow, ctx: &Ctx, page: &str) {
    let _ = win.show();
    // F27: the spinner restarts on the visible window (it was
    // stopped by the close handler — see there).
    win.set_window_hidden(false);
    #[cfg(windows)]
    ui_lifecycle::restore_and_foreground();
    // F27: only a window that actually was hidden needs the
    // reopen re-sync — a seed may have landed while it was hidden
    // (state swapped, window writes skipped), and notifications
    // pushed while hidden were never drawn.
    let was_hidden = ctx.hidden.swap(false, Ordering::SeqCst);
    if was_hidden {
        ctx.resync_after_reopen(win);
    }
    if page == "setup" {
        ctx.open_setup_wizard(win);
        ui_log!("RSH-UI page request: wizard reopened");
    } else {
        win.set_setup_mode(false);
        win.set_page(ui_lifecycle::page_index(page));
        ui_log!("RSH-UI page request: {page}");
    }
}

/// Drain single-instance + lifecycle requests on each frame tick
/// (M-5/A1, H-1): queued page forwards (Windows pipe), the Linux
/// request file, the daemon's tray-Quit file, and the
/// close-with-tray-off flag. Runs on the UI thread, so `show()` /
/// `quit_event_loop()` are safe here.
fn drain_ui_lifecycle(win: &MainWindow, ctx: &Ctx) {
    let mut pages: Vec<String> = Vec::new();
    if let Ok(mut pending) = ctx.page_requests.lock() {
        while let Some(page) = pending.pop_front() {
            pages.push(page);
        }
    }
    if let Some(page) = ui_lifecycle::poll_page_request_file(ctx.data_dir.as_ref()) {
        pages.push(page);
    }
    // Last request wins (a burst of tray clicks navigates once).
    if let Some(page) = pages.pop() {
        apply_page_request(win, ctx, &page);
    }
    if ui_lifecycle::take_quit_request(ctx.data_dir.as_ref()) {
        ui_log!("RSH-UI tray quit requested");
        slint::quit_event_loop().ok();
    } else if ctx.quit_requested.get() {
        ctx.quit_requested.set(false);
        slint::quit_event_loop().ok();
    }
}

/// R3-UI-05: whether this frame-timer tick runs a full `refresh`
/// — live + foreground runs every tick (20 Hz); live-but-
/// unfocused/minimized and not-live run every 2nd tick (10 Hz).
/// Pure so the truth table is unit-testable (the Win32 foreground
/// query itself only exists on Windows).
fn should_refresh(is_live: bool, foreground: bool, tick: u64) -> bool {
    (is_live && foreground) || tick.is_multiple_of(2)
}

/// Start the 20 Hz frame timer: drain health verdicts, drain live
/// events only while a live API is attached, refresh the frame, and
/// honour `--quit-after`. The data source comes from the shared
/// state, so a Reconnect swap takes effect on the next tick. 1 Hz
/// tasks (learn poll) run every 20th
/// tick; 5 s tasks (profiles/games refresh, diagnostics re-sync
/// while open) every 100th tick.
///
/// The sim is NOT advanced here any more (B1): its sine snapshot is
/// never displayed, so advancing the clock would only burn cycles.
#[allow(clippy::too_many_lines)]
fn start_frame_timer(
    win: &MainWindow,
    ctx: &Rc<Ctx>,
    quit_after: Option<u64>,
    t0: Instant,
    health_rx: std::sync::mpsc::Receiver<HealthEvent>,
) {
    let win = win.as_weak();
    let ctx = ctx.clone();
    let tick = Rc::new(Cell::new(0_u64));
    // R3-UI-05: the UI window's native handle for the per-tick
    // foreground query (Windows only) — resolved lazily inside
    // the tick and cached once found, so a window that does not
    // exist yet on the first tick is retried, not assumed. The
    // query fails OPEN: an unknown window counts as foreground,
    // so the live path never throttles on a lookup failure.
    #[cfg(windows)]
    let ui_hwnd: Rc<Cell<Option<isize>>> = Rc::new(Cell::new(None));
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(50), move || {
        let t = tick.get();
        tick.set(t + 1);
        // F27: the `--quit-after` check runs every tick, hidden or
        // not (the rig's dead/silent closes rely on the timer
        // firing while hidden) — it is checked first so the hidden
        // early-return below cannot skip it.
        if let Some(secs) = quit_after {
            if t0.elapsed() >= Duration::from_secs(secs) {
                slint::quit_event_loop().ok();
            }
        }
        // F27: while the window is hidden to tray, this tick writes
        // NOTHING to the window except `drain_ui_lifecycle` (tray
        // Quit + page requests — a page request SHOWS the window
        // first, so its writes are safe). gdb proved the
        // close-during-seed crash is a redraw racing the hide: the
        // fault is in the `llvmpipe-3` render thread, not app code
        // — while the seed fetch is in flight, live data changes a
        // property on nearly every tick, scheduling a redraw on the
        // just-hidden window, and the software renderer draws on
        // the torn-down drawable (SIGSEGV ~0.2 s after the close,
        // before the fetch completes — so the seed apply itself was
        // never the crash site; the apply guards stay as
        // defence-in-depth). Health verdicts, live events and job
        // completions queue in their channels and drain on reopen,
        // when the window is visible again.
        if ctx.hidden.load(Ordering::SeqCst) {
            if let Some(w) = win.upgrade() {
                drain_ui_lifecycle(&w, &ctx);
            }
            return;
        }
        // Health verdicts first: a death clears stale values, a
        // re-probe swaps live data in — both on the UI thread.
        drain_health(&health_rx, &win, &ctx);
        // Data source comes from the shared state so a Reconnect
        // swap takes effect on the next tick: drain live events
        // only while a live API is attached.
        let live_api = {
            let state = ctx.state.borrow();
            state.live_api.clone()
        };
        if let Some(live) = &live_api {
            for ev in live.poll_events() {
                log_event(&ev);
                handle_live_event(&win, &ctx, &ev);
            }
        }
        if let Some(w) = win.upgrade() {
            // M-5/A1 + H-1: forwarded page requests (second
            // instance) + tray Quit + close-with-tray-off.
            drain_ui_lifecycle(&w, &ctx);
            // B5/B6: background-job verdicts arrive here — the UI
            // thread never blocks on a job.
            ctx.drain_jobs(&w);
            // M-7: queued slider pushes (150 ms debounce) flush here.
            ctx.flush_steering_push(&w);
            ctx.flush_field_push(&w);
            ctx.flush_sensitivity_push(&w);
            // Perf (F-2): the startup list/diag syncs were deferred
            // out of `main` — run them once, on the first visible
            // frame tick, so the window paints before they run.
            if ctx.startup_sync_pending.get() {
                ctx.startup_sync_pending.set(false);
                ctx.refresh_profiles(&w);
                ctx.refresh_games(&w);
                ctx.sync_diag(&w);
            }
            // DS-82: time the frame's Rust work for the Diagnostics
            // frame-time histogram.
            let frame_started = Instant::now();
            // Perf (F-4): not live = no new data can arrive, and the
            // frame-rest comparison would skip the frame anyway —
            // run `refresh` at 10 Hz (every 2nd tick) instead of
            // paying the timer + compare every 50 ms.
            // R3-UI-05: focus-based throttling via a Win32
            // foreground query (Slint 1.18 exposes no window-focus
            // query on `Window`): a live-but-unfocused/minimized
            // window drops to the same 10 Hz as not-live. The
            // query fails open — an unresolvable window counts as
            // foreground, so the live path never throttles on a
            // lookup failure. (Non-Windows has no query: always
            // foreground, behaviour unchanged.)
            #[cfg(windows)]
            let foreground = {
                let hwnd = if let Some(hwnd) = ui_hwnd.get() {
                    Some(hwnd)
                } else {
                    let hwnd = ui_lifecycle::ui_window_hwnd();
                    ui_hwnd.set(hwnd);
                    hwnd
                };
                hwnd.is_none_or(ui_lifecycle::window_is_foreground)
            };
            #[cfg(not(windows))]
            let foreground = true;
            if should_refresh(ctx.is_live(), foreground, t) {
                // R2-46: arm the paint timer — the `AfterRendering`
                // notifier (registered in `main`) takes this start and
                // records refresh-to-paint. A tick that paints nothing
                // leaves no sample; a paint delayed past the next tick
                // is attributed to the newer start (a small underestimate
                // on a stalled frame, never an invented sample).
                ctx.frame_paint_pending.set(Some(frame_started));
                ctx.refresh(&w, t);
                ctx.record_frame_time(frame_started.elapsed());
            }
            if t.is_multiple_of(20) {
                // 1 Hz: job timeouts + the `connecting` guard (A9 —
                // never infinite) + firmware-update poll (H-4) +
                // learn poll.
                // FC2 safety net: the busy strip + daemon-error
                // banner re-derive here too (both writes are
                // change-detected, so this is free when nothing
                // changed) — a job or fatal file that lands outside
                // the drain/timeout paths still reaches the UI.
                ctx.sync_busy(&w);
                ctx.sync_daemon_error(&w);
                ctx.check_job_timeouts(&w);
                ctx.check_status_expiry(&w);
                ctx.sync_auto_theme(&w);
                ctx.check_wifi_rejoin_timeout();
                ctx.check_connecting_timeout(&w);
                ctx.poll_firmware_update(&w);
                // F5: daemon-side slow changes (profiles, tunings,
                // firmware, …) reach the pages within ~1 s, from
                // the cache only — no UI-thread IPC.
                ctx.resync_from_cache(&w);
                // Perf (F-7): flush a due coalesced notification
                // save — at most one disk write per second.
                ctx.flush_pending_notification_save(false);
                // D19: the Settings Wi-Fi password state + the
                // wizard's SSID re-sync within ~1 s, from the cache
                // only.
                ctx.sync_wifi_password_state(&w);
                // R2-11: diag / overview text is excluded from the
                // generation hash (diag p99s move every cycle), so
                // it re-syncs on the 1 Hz tick itself — guarded
                // writes make this free when nothing changed.
                ctx.sync_overview(&w);
                ctx.sync_diag(&w);
                // R2-16: Advanced-mode readouts at 1 Hz (centre +
                // live raw mV per axis, from the slow cache).
                ctx.sync_advanced(&w);
                ctx.start_learn_poll_job(&w);
                // vJoy install polling: until installed or failed.
                if ctx.vjoy_polling.get() {
                    ctx.sync_vjoy(&w);
                    if w.get_setup_vjoy_installed() {
                        ctx.vjoy_polling.set(false);
                        ctx.vjoy_install_started.set(None);
                        ctx.notify_success(
                            &w,
                            "Drivers",
                            "vJoy installed",
                            "vJoy is installed — games can see your wheel.",
                        );
                    } else if !w.get_setup_vjoy_installing() && w.get_setup_vjoy_error() != "" {
                        ctx.vjoy_polling.set(false);
                        ctx.vjoy_install_started.set(None);
                        ctx.notify_error(
                            &w,
                            "Drivers",
                            &UserError {
                                code: "vjoy_install_failed".to_string(),
                                title: "vJoy install failed".to_string(),
                                what_happened: w.get_setup_vjoy_error().to_string(),
                                what_to_do: "Try again, or use HIDMaestro instead.".to_string(),
                                detail: None,
                            },
                        );
                    }
                } else if w.get_setup_mode() && w.get_setup_step() == 1 {
                    ctx.sync_vjoy(&w);
                }
            }
            if t.is_multiple_of(100) {
                ctx.refresh_calibration();
                ctx.sync_advanced(&w);
                ctx.refresh_profiles(&w);
                // Games are manual-only now (no auto-scan): the list
                // changes only via add / edit / remove / launch /
                // stop / profile ops, which refresh it themselves —
                // the 5 s tick no longer re-reads it.
                // H9: while the wizard sits on Calibrate, re-probe
                // raw mV so Next enables when wheel data arrives.
                if w.get_setup_mode() && w.get_setup_step() == 2 {
                    ctx.start_raw_available_job(&w);
                }
                if w.get_diag_open() {
                    ctx.sync_diag(&w);
                }
            }
        }
    });
    // Keep the timer alive for the run.
    std::mem::forget(timer);
}

/// Install a panic hook that flushes the buffered UI log
/// (R3-UI-04): without it a crash strands the last ≤64 buffered
/// lines — exactly the lines a crash diagnosis needs. The
/// default hook still runs first (panic message + location to
/// stderr), then the flush runs best-effort (`flush_ui_file_log`
/// never panics and no-ops when logging is off or the lock is
/// poisoned, e.g. a panic inside the logger itself).
fn install_ui_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default_hook(info);
        crate::ui_log::flush_ui_file_log();
    }));
}

#[allow(clippy::too_many_lines)]
fn main() -> Result<(), slint::PlatformError> {
    let t0 = Instant::now();
    let args = parse_args();
    // DS-79: `--verbose` gates every `ui_log!` line in the UI.
    UI_VERBOSE.store(args.verbose, Ordering::Relaxed);
    let data_dir = live_probe::resolve_data_dir(args.data_dir.as_deref());
    // Phase 2 (L-1): the UI file log lives in
    // `<data_dir>/logs/rsh-ui-YYYYMMDD.log` — init before the
    // window is created so startup lines are recorded too.
    crate::ui_log::init_ui_file_log(data_dir.as_deref());
    // R3-UI-04: a crash must flush the buffered log (see
    // `install_ui_panic_hook`) — installed before anything else
    // that could panic.
    install_ui_panic_hook();

    // M-5/A1 (Tray-team hook): single instance, decided BEFORE the
    // window is created. A second instance (daemon tray action
    // `rsh-ui --page <page>` while the UI runs) forwards the page
    // to the first instance and exits 0 — never a duplicate window.
    let page_name = ui_lifecycle::page_name(args.page, args.setup);
    let _ui_instance = match ui_lifecycle::ensure_single_instance(data_dir.as_ref(), page_name) {
        ui_lifecycle::InstanceRole::First(guard) => guard,
        ui_lifecycle::InstanceRole::Second => return Ok(()),
    };
    // A stale quit file (crashed session) must not quit this launch.
    let _ = ui_lifecycle::take_quit_request(data_dir.as_ref());

    // Round 3 WS-6 (R3-AK-04): register for Windows Application
    // Restart (+ recovery callback) — the OS's own crash/hang
    // recovery path. This covers the case the daemon-side respawn
    // cannot: the UI itself crashing while the daemon is alive.
    // Best-effort: a failed registration is logged and never fails
    // the launch (no-op off Windows). Honest limit: NOT a silent
    // auto-restart — WER needs >= 60 s uptime and the user's consent
    // on crash/hang. Registered only for the first instance: a
    // second instance forwards its page and exits, so it must not
    // overwrite the first instance's restart command line.
    if let Some(dir) = data_dir.as_deref() {
        match rsh_api::restart::register_application_restart(dir, args.wer_restart) {
            Ok(outcome) => ui_log!("application restart registration: {outcome:?}"),
            Err(error) => ui_log!(
                "WARNING application restart registration failed ({error}) — continuing without WER restart"
            ),
        }
    }

    // Data source at startup (B4): the sim fallback, frozen — the
    // window shows first with a "Connecting…" state, and the health
    // monitor probes (spawning the sibling daemon when it is not
    // running) on a background thread, swapping live data in when
    // the daemon answers. No ~10 s block before the first paint.
    let api: Arc<dyn AppApi> = Arc::new(SimApi::new());
    let live_link: Arc<Mutex<Option<Arc<IpcApi>>>> = Arc::new(Mutex::new(None));
    let (health_tx, health_rx) = std::sync::mpsc::channel::<HealthEvent>();
    eprintln!("RSH-UI starting with sim fallback; health monitor probing for the daemon");

    // L4 startup: seed the slider state from the stored steering
    // record (live daemon or sim) and the unsaved dot from the draft.
    let stored_steering = api.get_tuning("steering");
    let initial_unsaved = api.draft_status().is_some_and(|s| s.dirty);
    // A9/B5: the background-job channel — job threads send JobDone
    // here; the frame timer drains it on the UI thread.
    let (job_tx, job_rx) = std::sync::mpsc::channel::<JobDone>();
    let ctx = Rc::new(Ctx {
        state: Rc::new(RefCell::new(LiveState {
            api: api.clone(),
            live_api: None,
            live: false,
            connecting: true,
            last_transport: ApiTransport::Wifi,
        })),
        tuning: Rc::new(RefCell::new(Tuning::from_settings(&stored_steering))),
        steering: Rc::new(RefCell::new(stored_steering)),
        pedals: Rc::new(RefCell::new(PedalCache::new())),
        handbrake_record: Rc::new(RefCell::new(TuningSettings::neutral("handbrake"))),
        led_record: Rc::new(RefCell::new(TuningSettings::neutral("led"))),
        horn_record: Rc::new(RefCell::new(TuningSettings::neutral("horn"))),
        calibration: Rc::new(RefCell::new(None)),
        vjoy_polling: Rc::new(Cell::new(false)),
        button_actions: Rc::new(RefCell::new(
            DEFAULT_BUTTON_ACTIONS
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
        )),
        pending_action: Rc::new(RefCell::new(None)),
        long_press_actions: Rc::new(RefCell::new(vec!["Not set".to_string(); 12])),
        pending_long_action: Rc::new(RefCell::new(None)),
        selected_button: Rc::new(Cell::new(5)),
        edit_kind: Rc::new(Cell::new(0)),
        edit_knob: Rc::new(Cell::new(0)),
        knob_row: Rc::new(Cell::new(0)),
        knob_actions: Rc::new(RefCell::new([
            [
                "Gear 1".to_string(),
                "Gear 2".to_string(),
                "Not set".to_string(),
            ],
            [
                "Not set".to_string(),
                "Not set".to_string(),
                "Not set".to_string(),
            ],
        ])),
        profiles_cache: Rc::new(RefCell::new(Vec::new())),
        games_cache: Rc::new(RefCell::new(Vec::new())),
        game_dialog_edit_id: Rc::new(RefCell::new(None)),
        game_dialog_profile_id: Rc::new(RefCell::new(None)),
        presets_games: Rc::new(RefCell::new(Vec::new())),
        games_profiles: Rc::new(RefCell::new(Vec::new())),
        pending_profile_id: Rc::new(RefCell::new(None)),
        learn_target: Rc::new(Cell::new(0)),
        horn_test_until: Rc::new(Cell::new(None)),
        handbrake_test_until: Rc::new(Cell::new(None)),
        status_until: Rc::new(Cell::new(None)),
        steer_hist: Rc::new(RefCell::new(VecDeque::new())),
        pedal_hist: Rc::new(RefCell::new(VecDeque::new())),
        brake_hist: Rc::new(RefCell::new(VecDeque::new())),
        clutch_hist: Rc::new(RefCell::new(VecDeque::new())),
        frame_cmds: Rc::new(RefCell::new(FrameCmdCache::default())),
        frame_times: Rc::new(RefCell::new(VecDeque::new())),
        frame_paint_times: Rc::new(RefCell::new(VecDeque::new())),
        frame_paint_pending: Rc::new(Cell::new(None)),
        unsaved: Rc::new(Cell::new(initial_unsaved)),
        // M10: notifications persist across restarts — the drawer is
        // reloaded from `<data_dir>/notifications.json` here (a
        // missing/corrupt file is an empty drawer, never a crash).
        notifications: Rc::new(RefCell::new(NotificationCenter::load(data_dir.as_deref()))),
        data_dir: data_dir.clone(),
        live_link: live_link.clone(),
        hidden: Arc::new(AtomicBool::new(false)),
        quit_requested: Rc::new(Cell::new(false)),
        page_requests: Arc::new(Mutex::new(VecDeque::new())),
        os_light: spawn_os_light_sampler(),
        theme_choice: Rc::new(Cell::new(read_theme(data_dir.as_ref()))),
        wifi_rejoining: Rc::new(Cell::new(false)),
        wifi_rejoin_since: Rc::new(Cell::new(None)),
        wifi_password_set_known: Rc::new(Cell::new(None)),
        wifi_password_message_for: Rc::new(Cell::new(None)),
        confirm_pending: Rc::new(RefCell::new(None)),
        vjoy_install_started: Rc::new(Cell::new(None)),
        unreachable_notified: Rc::new(Cell::new(false)),
        job_tx,
        job_rx: Rc::new(RefCell::new(job_rx)),
        busy_jobs: Rc::new(RefCell::new(HashMap::new())),
        // F6: generic-job registry.
        next_job_id: Rc::new(Cell::new(0)),
        pending_applies: Rc::new(RefCell::new(HashMap::new())),
        // F5: slow-cache generation the pages last re-synced from.
        last_cache_generation: Rc::new(Cell::new(0)),
        // A9: the window starts in "Connecting…" — the clock starts
        // now, so a daemon that never answers is reported after 15 s
        // instead of leaving the Overview on "Connecting…" forever.
        connecting_since: Rc::new(Cell::new(Some(t0))),
        last_setup_step_at: Rc::new(Cell::new(None)),
        reconnect_in_flight: Rc::new(Cell::new(false)),
        // F3/F12 (v2.0.3): respawn budget (shared with the health
        // monitor), latch reason, flap tracking — all start empty.
        spawn_budget: Arc::new(live_probe::SpawnBudget::new()),
        spawn_latch: Rc::new(RefCell::new(None)),
        last_dead_detail: Rc::new(RefCell::new(None)),
        down_since: Rc::new(Cell::new(None)),
        last_back_at: Rc::new(Cell::new(None)),
        last_flap_at: Rc::new(Cell::new(None)),
        flap_count: Rc::new(Cell::new(0)),
        suppress_connected_notify: Rc::new(Cell::new(false)),
        firmware_check_cache: Rc::new(RefCell::new(None)),
        raw_available: Rc::new(Cell::new(false)),
        presets_count: Rc::new(Cell::new(0)),
        module_states: Rc::new(RefCell::new([true; 4])),
        firmware_update_running: Rc::new(Cell::new(false)),
        last_address_notify: Rc::new(Cell::new(None)),
        steer_push_due: Rc::new(Cell::new(None)),
        pending_field: Rc::new(RefCell::new(None)),
        pending_sensitivity: Rc::new(RefCell::new(None)),
        sensitivity_cached: Rc::new(Cell::new(None)),
        sensitivity_failed_at: Rc::new(Cell::new(None)),
        sensitivity_push_base: Rc::new(Cell::new(None)),
        tuning_push_in_flight: Rc::new(RefCell::new(None)),
        profiles_model_hash: Rc::new(Cell::new(None)),
        games_model_hash: Rc::new(Cell::new(None)),
        presets_model_hash: Rc::new(Cell::new(None)),
        led_preview_key: Rc::new(Cell::new((-1, 255, u32::MAX))),
        diag_buttons_hash: Rc::new(Cell::new(None)),
        diag_events_hash: Rc::new(Cell::new(None)),
        last_frame: Rc::new(RefCell::new(None)),
        // Perf (F-2): the list/diag syncs run on the first frame
        // tick, not in `main` before the first paint.
        startup_sync_pending: Rc::new(Cell::new(true)),
        notif_save_pending: Rc::new(Cell::new(false)),
        notif_last_save: Rc::new(Cell::new(None)),
    });

    // The health monitor owns the first probe (B4): it starts now,
    // on its own thread, while the window below opens.
    spawn_health_monitor(
        data_dir.clone(),
        live_link,
        health_tx,
        ctx.hidden.clone(),
        ctx.spawn_budget.clone(),
    );

    let win = MainWindow::new()?;
    // Perf (F-2): show the window BEFORE the synchronous sync block
    // below — the first paint no longer waits behind
    // `refresh_profiles` / `refresh_games` / `sync_diag` (those now
    // run on the first frame tick instead) or behind `wire_*`.
    let _ = win.show();
    // R2-46: paint-completion timestamp for the frame histogram.
    // The `AfterRendering` notifier fires once Slint finished
    // painting the frame (before presentation) — the closest
    // number to user-visible jank. The callback only records a
    // timestamp; it draws nothing and changes no rendering. A
    // backend without notifier support returns `Err` and the
    // Rust-only histogram keeps working (the paint p50/p99 then
    // stay "—").
    {
        let paint_times = ctx.frame_paint_times.clone();
        let paint_pending = ctx.frame_paint_pending.clone();
        // Perf (F-2): the first `AfterRendering` fire is the true
        // first paint — log it once. (The old "window shown in X
        // ms" line below fires before `run()` and measures setup,
        // not paint.)
        let first_paint_logged = Rc::new(Cell::new(false));
        let first_paint_t0 = t0;
        if let Err(e) = win.window().set_rendering_notifier(move |state, _api| {
            if matches!(state, slint::RenderingState::AfterRendering) {
                if !first_paint_logged.get() {
                    first_paint_logged.set(true);
                    let ms = first_paint_t0.elapsed().as_millis();
                    ui_log!("RSH-UI first paint in {ms} ms");
                    eprintln!("RSH-UI first paint in {ms} ms");
                }
                if let Some(started) = paint_pending.take() {
                    let mut times = paint_times.borrow_mut();
                    times.push_back(started.elapsed());
                    while times.len() > 600 {
                        times.pop_front();
                    }
                }
            }
        }) {
            // R3-UI-03: the renderer gives no notifier — log the
            // absence to the UI log file, not just stderr (stderr
            // is invisible in the windowed release build, so
            // without this the log carries neither startup-timing
            // line and the missing first paint looks like a gap).
            ui_log!("RSH-UI first paint timing unavailable: {e}");
            eprintln!("RSH-UI paint timing unavailable: {e}");
        }
    }
    // H-1/A11: X hides to tray — the UI process stays alive (the
    // daemon keeps running, tray "Open RSH" restores it instantly).
    // Real quit is only via tray "Quit RSH" (quit file) or with
    // "Keep running in the tray" off (then the frame timer quits on
    // the next tick — quitting from inside this callback would be
    // re-entrant).
    {
        let ctx_close = ctx.clone();
        let win_close = win.as_weak();
        win.window().on_close_requested(move || {
            let to_tray = ctx_close.api().close_to_tray();
            ctx_close.hidden.store(true, Ordering::SeqCst);
            // F27: stop the busy spinner BEFORE hiding — its
            // 90 ms Timer would otherwise keep scheduling redraws
            // on the hidden window, and the software renderer
            // draws on the torn-down drawable (llvmpipe SIGSEGV
            // under Xvfb). Cleared on show (`apply_page_request`).
            if let Some(w) = win_close.upgrade() {
                w.set_window_hidden(true);
            }
            if !to_tray {
                ctx_close.quit_requested.set(true);
            }
            slint::CloseRequestResponse::HideWindow
        });
    }
    // M-5/A1: listen for page forwards from second instances
    // (Windows named pipe; Linux uses the request file).
    ui_lifecycle::listen_for_page_requests(ctx.page_requests.clone());
    // First-run wizard: --page setup, or the marker says so (F23).
    // Finished setups never re-show; skipped setups re-show once
    // per new version; a pre-v2.0.3 bare "done" marker (Skip vs
    // Finish unknowable) re-shows once so a skipped setup is not
    // lost on an update install — the v2.0.2 report.
    let setup = args.setup || ctx.setup_should_show();
    win.set_page(args.page);
    win.set_setup_mode(setup);
    win.set_setup_step(0);
    // R2-31: set BOTH theme props at startup — the STORED choice
    // (0 Dark / 1 Light / 2 Auto) for the Settings segment and the
    // RESOLVED 0/1 for `Theme.mode`. Writing only `theme` left the
    // segment showing Dark when the stored choice was Auto.
    win.set_theme_choice(ctx.theme_choice.get());
    win.set_theme(resolve_theme_mode(
        ctx.theme_choice.get(),
        ctx.os_light.load(Ordering::Relaxed),
    ));
    // R2-31: the segment shows the stored choice (Auto stays
    // Auto); `theme` above carries only the resolved value.
    win.set_theme_choice(ctx.theme_choice.get());
    win.set_app_version(api.version().into());
    // B4: not live yet — the monitor is still probing. The first
    // frame paints "Connecting…" (see `push_frame`).
    win.set_daemon_live(false);
    // FC2: a fatal file left by a previous crash shows its reason
    // in the Overview banner from the first paint; the busy strip
    // starts hidden (no job runs yet).
    ctx.sync_daemon_error(&win);
    ctx.sync_busy(&win);
    win.set_gear("—".into());
    win.set_notif_open(false);
    win.set_diag_open(false);
    win.set_selected_index(5);
    win.set_selected_action("Gear 3".into());
    win.set_button_actions(ModelRc::new(Rc::new(VecModel::from(
        DEFAULT_BUTTON_ACTIONS
            .iter()
            .map(|s| SharedString::from(*s))
            .collect::<Vec<SharedString>>(),
    ))));
    // A10: Start with Windows seeds from the API at startup.
    win.set_autostart_enabled(api.autostart());
    // FC2: keep-in-tray seeds from the API too (the close handler
    // reads the same value).
    ctx.sync_keep_in_tray(&win);
    // Default game controller is vJoy (user order 2026-11-16); the
    // live backend re-seeds it on (re)connect (see
    // `reconnect_to_live`) — at startup there is no daemon yet.
    // Capture gate (v2.0.2): centre / pedal Set / wizard Calibrate
    // are enabled while the daemon is live, disabled with a reason
    // otherwise. Kept fresh every frame by `refresh`.
    ctx.sync_capture_availability(&win);
    ctx.refresh_calibration();
    ctx.sync_advanced(&win);
    sync_to_window(&ctx.notifications.borrow(), &win);
    ctx.sync_pedals(&win);
    ctx.sync_extras(&win);
    ctx.sync_overview(&win);
    // Perf (F-2): `refresh_profiles` / `refresh_games` / `sync_diag`
    // are deferred to the first frame tick (see
    // `startup_sync_pending` in the frame timer) — the window
    // paints before those list/diag syncs run.
    ctx.sync_vjoy(&win);
    ctx.refresh(&win, 0);
    // No startup wizard seeding here any more (B4): the live API
    // does not exist yet — `reconnect_to_live` seeds the wizard
    // when the daemon answers. The "not running"
    // notification comes from the monitor's first verdict instead.
    // R3-UI-03: same class as the first-paint fallback above —
    // the "window shown" line is startup evidence too, so it goes
    // to the UI log file as well as stderr.
    ui_log!("RSH-UI window shown in {} ms", t0.elapsed().as_millis());
    eprintln!("RSH-UI window shown in {} ms", t0.elapsed().as_millis());

    wire_callbacks(&win, &ctx);

    // ---- frame timer (20 Hz) + --quit-after
    if args.static_mode {
        // Static mode has no frame timer: drain health verdicts on
        // a 1 Hz timer so a late daemon still swaps in.
        let win_weak = win.as_weak();
        let ctx_static = ctx.clone();
        let health_timer = Timer::default();
        health_timer.start(TimerMode::Repeated, Duration::from_secs(1), move || {
            // F27: same hidden-window guard as the frame timer —
            // while hidden, only the lifecycle drain runs (tray
            // Quit / page requests); health verdicts and job
            // completions queue and drain on reopen.
            if ctx_static.hidden.load(Ordering::SeqCst) {
                if let Some(w) = win_weak.upgrade() {
                    drain_ui_lifecycle(&w, &ctx_static);
                }
                return;
            }
            drain_health(&health_rx, &win_weak, &ctx_static);
            if let Some(w) = win_weak.upgrade() {
                drain_ui_lifecycle(&w, &ctx_static);
                // B5/B6/A9: the same job drain + timeout guards as
                // the frame timer — static mode has no frame timer,
                // but jobs still complete and loading must never
                // hang forever.
                ctx_static.drain_jobs(&w);
                ctx_static.check_job_timeouts(&w);
                ctx_static.check_connecting_timeout(&w);
                // Perf (F-7): flush a due coalesced notification save.
                ctx_static.flush_pending_notification_save(false);
                // FC2: same 1 Hz safety net as the frame timer —
                // the daemon-error banner re-derives (change-
                // detected, so free when nothing changed).
                ctx_static.sync_daemon_error(&w);
                ctx_static.poll_firmware_update(&w);
                ctx_static.flush_steering_push(&w);
                ctx_static.flush_field_push(&w);
                ctx_static.flush_sensitivity_push(&w);
            }
        });
        std::mem::forget(health_timer);
        if let Some(secs) = args.quit_after {
            let timer = Timer::default();
            timer.start(TimerMode::SingleShot, Duration::from_secs(secs), || {
                slint::quit_event_loop().ok();
            });
            std::mem::forget(timer);
        }
    } else {
        start_frame_timer(&win, &ctx, args.quit_after, t0, health_rx);
    }

    let result = win.run();
    // Perf (F-7): flush the buffered UI log and any coalesced
    // notification save still due — the process exits here, so no
    // 1 Hz tick will do it.
    crate::ui_log::flush_ui_file_log();
    ctx.flush_pending_notification_save(true);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    // Only the #[cfg(unix)] reconnect-seed test below uses this; on
    // the Windows target that test is compiled out and an ungated
    // import warns (windows-check --all-targets).
    #[cfg(unix)]
    use rsh_api::WifiStatus;

    #[test]
    fn curve_range_is_1_to_2_not_3() {
        // A24: the core clamps the curve at 2.0 — the UI range matches.
        let mut t = Tuning::from_settings(&TuningSettings::neutral("steering"));
        t.set_exponent(3.0);
        assert_eq!(t.exponent, 2.0);
        t.set_preset(2);
        assert_eq!(t.exponent, 2.0);
        assert_eq!(t.deadzone_pct, 1.0);
        t.set_exponent(0.5);
        assert_eq!(t.exponent, 1.0);
    }

    #[test]
    fn preset_round_trips_through_settings() {
        let mut t = Tuning::from_settings(&TuningSettings::neutral("steering"));
        t.set_preset(1);
        let back = Tuning::from_settings(&t.to_settings(&TuningSettings::neutral("steering")));
        assert_eq!(back.preset, 1);
        t.set_preset(2);
        let back = Tuning::from_settings(&t.to_settings(&TuningSettings::neutral("steering")));
        assert_eq!(back.preset, 2);
    }

    /// Bug 7: the Advanced fields round-trip through the daemon
    /// record — invert, lock (snapped), smoothing — and
    /// `reset_defaults` restores all of them, not just the feel.
    /// (S2+S3: balance and centre trim round-trip the same way.)
    #[test]
    fn steering_advanced_fields_round_trip_through_settings() {
        let mut t = Tuning::from_settings(&TuningSettings::neutral("steering"));
        assert!(!t.invert);
        assert_eq!(t.lock_deg, 900.0);
        assert_eq!(t.balance_pct, 0.0);
        assert_eq!(t.trim_pct, 0.0);
        t.set_invert(true);
        t.set_lock_deg(500.0);
        t.set_smoothing(45.0);
        t.set_deadzone(25.0);
        t.set_balance(10.0);
        t.set_trim(-3.0);
        let settings = t.to_settings(&TuningSettings::neutral("steering"));
        assert_eq!(settings.invert, Some(true));
        assert_eq!(settings.steering_lock_deg, Some(540.0));
        assert_eq!(settings.smoothing, 45.0);
        assert_eq!(settings.deadzone, 25.0);
        assert_eq!(settings.steering_balance_pct, Some(10.0));
        assert_eq!(settings.steering_center_trim_pct, Some(-3.0));
        let back = Tuning::from_settings(&settings);
        assert!(back.invert);
        assert_eq!(back.lock_deg, 540.0);
        assert_eq!(back.smoothing_pct, 45.0);
        assert_eq!(back.deadzone_pct, 25.0);
        assert_eq!(back.balance_pct, 10.0);
        assert_eq!(back.trim_pct, -3.0);
        // The sliders clamp at their ends, never beyond.
        t.set_balance(99.0);
        assert_eq!(t.balance_pct, 20.0);
        t.set_trim(-99.0);
        assert_eq!(t.trim_pct, -5.0);
        // Deadzone clamps at the new 30% cap, not the old 10%.
        t.set_deadzone(99.0);
        assert_eq!(t.deadzone_pct, 30.0);
        t.reset_defaults();
        assert_eq!(t.preset, 1);
        assert_eq!(t.smoothing_pct, 30.0);
        assert!(!t.invert);
        assert_eq!(t.lock_deg, 900.0);
        assert_eq!(t.balance_pct, 0.0);
        assert_eq!(t.trim_pct, 0.0);
    }

    #[test]
    fn pedal_curve_linear_is_straight() {
        let cmds = pedal_curve_commands(0.0, 1.0);
        assert_eq!(cmds, "M 6 104 Q 55 55 104 6");
    }

    #[test]
    fn pedal_curve_soft_bows_up_sharp_bows_down() {
        let soft = pedal_curve_commands(0.0, 0.7);
        let sharp = pedal_curve_commands(0.0, 1.5);
        // Control-point y: soft < linear < sharp.
        assert!(soft.contains("Q 55 46"));
        assert!(sharp.contains("Q 55 70"));
    }

    #[test]
    fn pedal_marker_at_rest_and_full() {
        let (x, y) = pedal_marker(0.0, 0.0, 1.0);
        assert!((x - 6.0).abs() < 0.01);
        assert!((y - 104.0).abs() < 0.01);
        let (x, y) = pedal_marker(100.0, 0.0, 1.0);
        assert!((x - 104.0).abs() < 0.01);
        assert!((y - 6.0).abs() < 0.01);
    }

    #[test]
    fn pedal_marker_respects_deadzone() {
        // 5% input inside a 10% deadzone reads as zero output.
        let (_, y) = pedal_marker(5.0, 10.0, 1.0);
        assert!((y - 104.0).abs() < 0.01);
    }

    #[test]
    fn resolve_theme_mode_maps_choice_to_window_theme() {
        // Dark/Light as stored; Auto follows the OS preference.
        assert_eq!(resolve_theme_mode(0, false), 0);
        assert_eq!(resolve_theme_mode(0, true), 0);
        assert_eq!(resolve_theme_mode(1, false), 1);
        assert_eq!(resolve_theme_mode(1, true), 1);
        assert_eq!(resolve_theme_mode(2, false), 0);
        assert_eq!(resolve_theme_mode(2, true), 1);
        // Out-of-range choices fall back to Dark.
        assert_eq!(resolve_theme_mode(99, true), 0);
    }

    #[test]
    fn ds51_write_theme_reports_failure() {
        // DS-51: persist success returns true and round-trips; a
        // missing data dir or an unwritable path returns false (the
        // caller notifies and reverts instead of failing silently).
        assert!(!write_theme(None, 1));
        let dir = std::env::temp_dir().join(format!("rsh-ds51-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("ds51 temp dir");
        assert!(write_theme(Some(&dir), 1));
        assert_eq!(read_theme(Some(&dir)), 1);
        assert!(write_theme(Some(&dir), 2));
        assert_eq!(read_theme(Some(&dir)), 2);
        // A file where the directory should be: create_dir_all fails.
        let file = dir.join("not-a-dir");
        std::fs::write(&file, "x").expect("ds51 temp file");
        assert!(!write_theme(Some(&file), 1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ds67_conn_label_for_rejoining() {
        // DS-67: a password set drops the link on purpose (the AP
        // restarts to WPA2) — the label says "rejoining", never a
        // generic disconnect.
        assert_eq!(
            conn_label_for(false, false, "Disconnected", true),
            "Rejoining with the new password…"
        );
        assert_eq!(
            conn_label_for(false, true, "Connected", true),
            "Rejoining with the new password…"
        );
        assert_eq!(conn_label_for(true, false, "", false), "Connecting…");
        assert_eq!(conn_label_for(false, true, "Connected", false), "Connected");
        assert_eq!(
            conn_label_for(false, false, "Disconnected", false),
            "Disconnected"
        );
    }

    #[test]
    fn game_code_from_names() {
        assert_eq!(game_code("Assetto Corsa"), "AC");
        // H6: AC Competizione gets its own code — no more collision.
        assert_eq!(game_code("AC Competizione"), "ACC");
        assert_eq!(game_code("Assetto Corsa Competizione"), "ACC");
        assert_eq!(game_code("iRacing"), "iR");
        assert_eq!(game_code("Dirt Rally 2.0"), "DR");
    }

    #[test]
    fn action_names_round_trip_through_ids() {
        // H-3: the Buttons screen's display names map to backend ids
        // and back — "Not set" clears, unknown ids read as "Not set".
        assert_eq!(action_id("Shift up"), Some("shifter_up"));
        assert_eq!(action_id("Gear 3"), Some("gear_3"));
        assert_eq!(action_id("Not set"), None);
        assert_eq!(action_display("shifter_up"), "Shift up");
        assert_eq!(action_display("gear_3"), "Gear 3");
        assert_eq!(action_display("profile_next"), "Next profile");
        assert_eq!(action_display("bogus"), "Not set");
        for display in [
            "Shift up",
            "Shift down",
            "Horn",
            "Gear 1",
            "Gear 2",
            "Gear 3",
            "Gear 4",
            "Gear 5",
            "Gear 6",
            "Handbrake",
        ] {
            let id = action_id(display).expect("a chip maps to an id");
            assert_eq!(action_display(id), display);
        }
    }

    #[test]
    fn pretty_game_slug_reads_as_name() {
        assert_eq!(pretty_game_slug("assetto_corsa"), "Assetto Corsa");
        assert_eq!(pretty_game_slug("dirt_rally_2"), "Dirt Rally 2");
    }

    #[test]
    fn user_text_never_says_daemon() {
        // L3: the background service is called "RSH" now.
        assert_eq!(
            user_text("Daemon not running — showing Simulated data"),
            "RSH not running — showing Live data unavailable"
        );
        assert_eq!(
            user_text("the daemon keeps running"),
            "the RSH keeps running"
        );
        assert!(!user_text("Daemon x daemon y").contains("aemon"));
    }

    #[test]
    fn mv_text_dash_for_no_reading() {
        assert_eq!(mv_text(0.0), "—");
        assert_eq!(mv_text(410.0), "410 mV");
        assert_eq!(pct_text(2.0), "2%");
    }

    #[test]
    fn feel_index_round_trips() {
        assert_eq!(PedalCache::feel_index(1.0), 0);
        assert_eq!(PedalCache::feel_index(0.7), 1);
        assert_eq!(PedalCache::feel_index(1.5), 2);
        assert_eq!(PedalCache::feel_exponent(0), 1.0);
        assert_eq!(PedalCache::feel_exponent(1), 0.7);
        assert_eq!(PedalCache::feel_exponent(2), 1.5);
    }

    #[test]
    fn chart_commands_empty_until_two_samples() {
        let mut samples = VecDeque::new();
        assert_eq!(chart_commands(&samples, -180.0, 180.0), "");
        samples.push_back(0.0);
        assert_eq!(chart_commands(&samples, -180.0, 180.0), "");
        samples.push_back(90.0);
        let cmds = chart_commands(&samples, -180.0, 180.0);
        assert!(cmds.starts_with("M 0.0 45.0 L 380.0 25.0"), "{cmds}");
    }

    /// R2 (pedals real-time): the Diagnostics pedals chart carries
    /// throttle, brake and clutch as three subpaths in one string —
    /// empty when no trace has two samples yet.
    #[test]
    fn multi_chart_commands_combines_traces() {
        let empty: VecDeque<f32> = VecDeque::new();
        assert_eq!(multi_chart_commands(&[&empty], 0.0, 1.0), "");
        let one: VecDeque<f32> = [0.0, 1.0].into_iter().collect();
        let two: VecDeque<f32> = [1.0, 0.0].into_iter().collect();
        let combined = multi_chart_commands(&[&one, &two], 0.0, 1.0);
        assert_eq!(combined.matches('M').count(), 2, "{combined}");
        assert!(combined.starts_with("M 0.0 85.0"), "{combined}");
        let short: VecDeque<f32> = [0.5].into_iter().collect();
        assert_eq!(
            multi_chart_commands(&[&short, &one], 0.0, 1.0),
            chart_commands(&one, 0.0, 1.0)
        );
    }

    /// DS-78: the frame command cache reuses the built string while
    /// its inputs are unchanged, rebuilds when an input moves, and
    /// the dashed line is built once.
    #[test]
    fn ds78_frame_cmd_cache_reuses_unchanged_inputs() {
        let mut cache = FrameCmdCache::default();
        let first_curve = cache.curve(10.0, 1.5);
        assert_eq!(cache.curve(10.0, 1.5), first_curve);
        assert_ne!(cache.curve(20.0, 1.5), first_curve);
        assert_eq!(cache.curve(20.0, 1.5), cache.curve(20.0, 1.5));
        let first_cross = cache.crosshair(100.0, 50.0);
        assert_eq!(cache.crosshair(100.0, 50.0), first_cross);
        assert_ne!(cache.crosshair(101.0, 50.0), first_cross);
        let dashed = cache.dashed_line();
        assert_eq!(cache.dashed_line(), dashed);
        assert!(!dashed.is_empty());
    }

    /// DS-82: the frame-time histogram is empty until a frame is
    /// recorded, then reports p50/p99 over the recorded window in
    /// ms (1..=100 ms samples: p50 50 ms, p99 99 ms).
    #[test]
    fn ds82_frame_time_histogram_reports_p50_p99() {
        let (ctx, _dir) = fc2_test_ctx();
        assert_eq!(ctx.frame_time_p50_p99(), None);
        for ms in 1..=100_u64 {
            ctx.record_frame_time(Duration::from_millis(ms));
        }
        let (p50, p99) = ctx.frame_time_p50_p99().expect("measured");
        assert!((p50 - 50.0).abs() < 0.001, "p50 {p50}");
        assert!((p99 - 99.0).abs() < 0.001, "p99 {p99}");
        // The ring caps at 600: 700 more samples push the first
        // 200 out (window 201..=800 ms → p50 500 ms).
        for ms in 101..=800_u64 {
            ctx.record_frame_time(Duration::from_millis(ms));
        }
        assert_eq!(ctx.frame_times.borrow().len(), 600);
        let (p50, _) = ctx.frame_time_p50_p99().expect("measured");
        assert!((p50 - 500.0).abs() < 0.001, "p50 {p50}");
    }

    /// Perf (F-4): an unchanged frame rests — the second
    /// `push_frame` with identical inputs returns `false` and the
    /// frame after that too; a local tuning change wakes it again,
    /// then it rests once more. (`SimApi` is not live here, so the
    /// snapshot is the constant default — the disconnected idle
    /// case the frame rest exists for.)
    #[test]
    fn perf_frame_rest_skips_unchanged_frame_and_wakes_on_tuning_change() {
        let Ok(Ok(win)) = std::panic::catch_unwind(MainWindow::new) else {
            eprintln!("perf frame-rest test skipped: no usable display/backend");
            return;
        };
        let (ctx, dir) = fc2_test_ctx();
        assert!(ctx.push_frame(&win, 0), "first frame always runs");
        assert!(!ctx.push_frame(&win, 1), "identical inputs rest the frame");
        assert!(!ctx.push_frame(&win, 2), "still identical on the next tick");
        ctx.tuning.borrow_mut().set_deadzone(5.0);
        assert!(
            ctx.push_frame(&win, 3),
            "a local tuning change wakes the frame"
        );
        assert!(!ctx.push_frame(&win, 4), "identical again after the change");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Perf (F-7): `persist_notifications` coalesces — repeated
    /// notifies mark one save due but write nothing until the 1 Hz
    /// flush, and that flush writes at most once per second (a
    /// second flush inside the window is a no-op); the exit flush
    /// (`force`) still lands the latest state.
    #[test]
    fn perf_notification_save_is_coalesced_to_one_write_per_second() {
        let Ok(Ok(win)) = std::panic::catch_unwind(MainWindow::new) else {
            eprintln!("perf notif test skipped: no usable display/backend");
            return;
        };
        let (ctx, dir) = fc2_test_ctx();
        let path = dir.join(crate::notifications::NOTIFICATIONS_FILE);
        ctx.notify(&win, "Test", "First", "one");
        ctx.notify(&win, "Test", "Second", "two");
        assert!(!path.exists(), "coalesced: no disk write before the flush");
        ctx.flush_pending_notification_save(false);
        assert!(path.exists(), "the 1 Hz flush writes once");
        let written = std::fs::read_to_string(&path).expect("read notifications");
        assert!(written.contains("First"), "first item persisted");
        assert!(written.contains("Second"), "second item persisted");
        ctx.notify(&win, "Test", "Third", "three");
        ctx.flush_pending_notification_save(false);
        let still = std::fs::read_to_string(&path).expect("read notifications");
        assert_eq!(
            still, written,
            "a flush inside the 1 s window writes nothing"
        );
        ctx.flush_pending_notification_save(true);
        let forced = std::fs::read_to_string(&path).expect("read notifications");
        assert!(forced.contains("Third"), "the forced exit flush lands it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn led_preview_off_is_dark_and_solid_is_accent() {
        let off = led_preview(LedPattern::Off, 100, None);
        assert_eq!(off.len(), 8);
        assert!(off
            .iter()
            .all(|c| *c == Color::from_rgb_u8(0x1D, 0x25, 0x33)));
        let solid = led_preview(LedPattern::Solid, 100, None);
        assert!(solid
            .iter()
            .all(|c| *c == Color::from_rgb_u8(0x2F, 0x6B, 0xFF)));
        let dim = led_preview(LedPattern::Solid, 0, None);
        assert!(dim
            .iter()
            .all(|c| *c == Color::from_rgb_u8(0x0D, 0x11, 0x18)));
    }

    #[test]
    fn led_preview_uses_the_stored_colour() {
        // M2: a stored red shows red, not the historic accent blue.
        let red = led_preview(LedPattern::Solid, 100, Some(0xFF_6B_6B));
        assert!(red
            .iter()
            .all(|c| *c == Color::from_rgb_u8(0xFF, 0x6B, 0x6B)));
        let white = led_preview(LedPattern::Solid, 100, Some(0xFF_FF_FF));
        assert!(white
            .iter()
            .all(|c| *c == Color::from_rgb_u8(0xFF, 0xFF, 0xFF)));
    }

    #[test]
    fn led_horn_slider_mappings() {
        // L1: the period slider spans 100..5000 ms (default 1000 at
        // the 0.184 thumb); HO1: the minimum-honk slider spans
        // 100..1000 ms (default 400 at the 1/3 thumb). Out-of-range
        // thumbs clamp instead of producing an invalid tuning.
        assert_eq!(led_period_ms_from_slider(0.0), 100.0);
        assert_eq!(led_period_ms_from_slider(1.0), 5000.0);
        assert!((led_period_ms_from_slider(900.0 / 4900.0) - 1000.0).abs() < 1e-9);
        assert_eq!(led_period_ms_from_slider(-0.5), 100.0);
        assert_eq!(led_period_ms_from_slider(2.0), 5000.0);
        assert_eq!(horn_min_ms_from_slider(0.0), 100.0);
        assert_eq!(horn_min_ms_from_slider(1.0), 1000.0);
        assert!((horn_min_ms_from_slider(300.0 / 900.0) - 400.0).abs() < 1e-9);
        assert_eq!(horn_min_ms_from_slider(-0.5), 100.0);
        assert_eq!(horn_min_ms_from_slider(2.0), 1000.0);
        // L2: the RGB channels compose the full-RGB protocol value;
        // out-of-range channels clamp to 0..255.
        assert_eq!(led_color_from_channels(1.0, 0.0, 0.0), 0xFF_00_00);
        assert_eq!(led_color_from_channels(0.0, 1.0, 0.0), 0x00_FF_00);
        assert_eq!(led_color_from_channels(0.0, 0.0, 1.0), 0x00_00_FF);
        assert_eq!(led_color_from_channels(1.0, 1.0, 1.0), 0xFF_FF_FF);
        assert_eq!(led_color_from_channels(0.0, 0.0, 0.0), 0x00_00_00);
        assert_eq!(led_color_from_channels(2.0, -1.0, 0.5), 0xFF_00_80);
    }

    #[test]
    fn pedal_key_maps_indices() {
        assert_eq!(pedal_key(0), Some(("accel", "Accelerator")));
        assert_eq!(pedal_key(1), Some(("brake", "Brake")));
        assert_eq!(pedal_key(2), Some(("clutch", "Clutch")));
        assert_eq!(pedal_key(3), None);
        assert_eq!(pedal_key(-1), None);
    }

    #[test]
    fn fc2_busy_state_oldest_job_wins() {
        // B5/FC2: the strip shows one label — the oldest job's.
        let mut jobs: HashMap<JobKind, (String, Instant)> = HashMap::new();
        assert_eq!(busy_state_for(&jobs), (false, String::new()));
        let old = Instant::now();
        let new = old + Duration::from_millis(1);
        jobs.insert(
            JobKind::Transport,
            ("Switching connection…".to_string(), new),
        );
        jobs.insert(JobKind::Reconnect, ("Reconnecting…".to_string(), old));
        assert_eq!(busy_state_for(&jobs), (true, "Reconnecting…".to_string()));
        jobs.remove(&JobKind::Reconnect);
        assert_eq!(
            busy_state_for(&jobs),
            (true, "Switching connection…".to_string())
        );
        jobs.clear();
        assert_eq!(busy_state_for(&jobs), (false, String::new()));
    }

    #[test]
    fn ds54_busy_kind_for_oldest_job() {
        // DS-54: the Slint `busy-kind` prop carries the OLDEST job's
        // JobKind::order value (-1 = idle) so the invoking button can
        // disable + dim while its job runs.
        let mut jobs: HashMap<JobKind, (String, Instant)> = HashMap::new();
        assert_eq!(busy_kind_for(&jobs), -1);
        let old = Instant::now();
        let new = old + Duration::from_millis(1);
        jobs.insert(
            JobKind::Transport,
            ("Switching connection…".to_string(), new),
        );
        jobs.insert(JobKind::Reconnect, ("Reconnecting…".to_string(), old));
        assert_eq!(busy_kind_for(&jobs), 0);
        jobs.remove(&JobKind::Reconnect);
        assert_eq!(busy_kind_for(&jobs), 1);
        jobs.insert(
            JobKind::AppUpdateCheck,
            ("Checking for updates…".to_string(), new),
        );
        // Transport (order 1) is still oldest — AppUpdateCheck (16)
        // does not hide it.
        assert_eq!(busy_kind_for(&jobs), 1);
        jobs.clear();
        jobs.insert(
            JobKind::AppUpdateCheck,
            ("Checking for updates…".to_string(), old),
        );
        assert_eq!(busy_kind_for(&jobs), 16);
    }

    /// DS-54 (consolidation): the full JobKind→busy-kind mapping
    /// the Slint screens disable buttons against — `order()` is the
    /// contract, so every kind is pinned here.
    #[test]
    fn ds54_busy_kind_mapping_matches_slint() {
        let expected = [
            (JobKind::Reconnect, 0),
            (JobKind::Transport, 1),
            (JobKind::Backend, 2),
            (JobKind::FirmwareCheck, 3),
            (JobKind::FirmwareUpdate, 4),
            (JobKind::RawAvailable, 5),
            (JobKind::TuningPush, 7),
            (JobKind::ProfileOp, 8),
            (JobKind::BindingOp, 9),
            (JobKind::CaptureOp, 10),
            (JobKind::LearnOp, 11),
            // R2-35: the toggle has its own strip slot (17, see
            // `JobKind::order`) — Slint disables Learn controls on
            // 11 or 17.
            (JobKind::LearnToggle, 17),
            (JobKind::GameOp, 13),
            (JobKind::SettingsOp, 14),
            (JobKind::ReconnectSeed, 15),
            (JobKind::AppUpdateCheck, 16),
        ];
        for (kind, number) in expected {
            let jobs: HashMap<JobKind, (String, Instant)> =
                [(kind, ("test".to_string(), Instant::now()))]
                    .into_iter()
                    .collect();
            assert_eq!(busy_kind_for(&jobs), number, "{kind:?}");
            assert_eq!(busy_state_for(&jobs), (true, "test".to_string()));
        }
        let empty: HashMap<JobKind, (String, Instant)> = HashMap::new();
        assert_eq!(busy_kind_for(&empty), -1);
        assert_eq!(busy_state_for(&empty), (false, String::new()));
    }

    #[test]
    fn r2_learn_toggle_has_own_job_kind() {
        // R2-35: the Learn click runs on its own kind (order 17,
        // 5 s timeout) so an in-flight LearnOp poll job can never
        // swallow it via the duplicate-start rule.
        assert_eq!(JobKind::LearnToggle.order(), 17);
        assert_eq!(JobKind::LearnToggle.timeout(), Duration::from_secs(5));
        assert_eq!(JobKind::LearnOp.order(), 11);
    }

    #[test]
    fn r2_setup_step_debounce() {
        // R2-36: first click passes; a second click within 300 ms
        // is ignored; a click after the window passes again.
        let (ctx, _dir) = fc2_test_ctx();
        assert!(ctx.setup_step_allowed());
        assert!(!ctx.setup_step_allowed());
        ctx.last_setup_step_at.set(Some(
            Instant::now()
                .checked_sub(Duration::from_millis(400))
                .expect("test instant"),
        ));
        assert!(ctx.setup_step_allowed());
    }

    #[test]
    fn r2_test_flash_active() {
        // R2-17: a future flash instant reads pressed; None and a
        // past instant read released (Horn Test + handbrake test).
        assert!(test_flash_active(Some(
            Instant::now() + Duration::from_millis(400)
        )));
        assert!(!test_flash_active(None));
        assert!(!test_flash_active(Some(
            Instant::now()
                .checked_sub(Duration::from_millis(1))
                .expect("test instant")
        )));
    }

    #[test]
    fn r2_percentile_p50_p99() {
        // R2-46: the shared percentile math behind the frame and
        // paint histograms; empty ring means "—" (None).
        assert_eq!(percentile_p50_p99(&VecDeque::new()), None);
        let times: VecDeque<Duration> = [1, 2, 3, 4, 5]
            .iter()
            .map(|ms| Duration::from_millis(*ms))
            .collect();
        let (p50, p99) = percentile_p50_p99(&times).expect("samples");
        assert!((p50 - 3.0).abs() < 0.01, "p50 {p50}");
        // p99 index is (len-1)*99/100 = 3 for 5 samples, i.e. 4 ms
        // — the DS-82 nearest-rank formula, preserved verbatim
        // from the original `frame_time_p50_p99`.
        assert!((p99 - 4.0).abs() < 0.01, "p99 {p99}");
    }

    #[test]
    fn fc2_board_address_validation() {
        // H4/FC2: same rule as the daemon — private/loopback IPv4
        // literal only (whitespace trimmed).
        for ok in [
            "192.168.4.1",
            "10.0.0.5",
            "172.16.0.9",
            "127.0.0.1",
            " 192.168.4.1 ",
        ] {
            assert!(valid_board_address(ok), "{ok} should be valid");
        }
        for bad in [
            "",
            "   ",
            "rsh.local",
            "8.8.8.8",
            "256.1.1.1",
            "192.168.4",
            "fe80::1",
        ] {
            assert!(!valid_board_address(bad), "{bad} should be invalid");
        }
    }

    #[test]
    fn fc2_daemon_error_text() {
        // M-2/FC2: live clears, no fatal file clears, a fatal detail
        // shows (L3: "daemon" rewritten to "RSH"), an empty detail
        // still shows a plain line.
        assert_eq!(daemon_error_for(true, None), "");
        assert_eq!(
            daemon_error_for(
                true,
                Some((
                    "IpcBindFailed".to_string(),
                    "daemon IPC bind failed".to_string()
                ))
            ),
            ""
        );
        assert_eq!(daemon_error_for(false, None), "");
        assert_eq!(
            daemon_error_for(
                false,
                Some((
                    "IpcBindFailed".to_string(),
                    "daemon IPC bind failed after 5 attempts".to_string()
                ))
            ),
            "RSH IPC bind failed after 5 attempts"
        );
        assert_eq!(
            daemon_error_for(false, Some(("DaemonHung".to_string(), String::new()))),
            "RSH stopped unexpectedly — try Reconnect."
        );
        assert_eq!(
            daemon_error_for(false, Some(("DaemonHung".to_string(), "   ".to_string()))),
            "RSH stopped unexpectedly — try Reconnect."
        );
    }

    #[test]
    fn fc2_clean_profile_name() {
        assert_eq!(
            clean_profile_name("  GT3 Sprint "),
            Some("GT3 Sprint".to_string())
        );
        assert_eq!(clean_profile_name(""), None);
        assert_eq!(clean_profile_name("   "), None);
    }

    /// Minimal [`Ctx`] for the FC2 window test: the sim API, an
    /// isolated temp data dir (notifications + `daemon-status.json`
    /// never touch the real data dir), no health monitor, no frame
    /// timer — the test drives the handlers directly.
    #[allow(clippy::too_many_lines)]
    fn fc2_test_ctx() -> (Rc<Ctx>, PathBuf) {
        let api: Arc<dyn AppApi> = Arc::new(SimApi::new());
        fc2_test_ctx_with_api(&api)
    }

    #[allow(clippy::too_many_lines)]
    fn fc2_test_ctx_with_api(api: &Arc<dyn AppApi>) -> (Rc<Ctx>, PathBuf) {
        let dir = std::env::temp_dir().join(format!("rsh-fc2-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("fc2 temp dir");
        let stored_steering = api.get_tuning("steering");
        let (job_tx, job_rx) = std::sync::mpsc::channel::<JobDone>();
        let ctx = Rc::new(Ctx {
            state: Rc::new(RefCell::new(LiveState {
                api: api.clone(),
                live_api: None,
                live: false,
                connecting: true,
                last_transport: ApiTransport::Wifi,
            })),
            tuning: Rc::new(RefCell::new(Tuning::from_settings(&stored_steering))),
            steering: Rc::new(RefCell::new(stored_steering)),
            pedals: Rc::new(RefCell::new(PedalCache::new())),
            handbrake_record: Rc::new(RefCell::new(TuningSettings::neutral("handbrake"))),
            led_record: Rc::new(RefCell::new(TuningSettings::neutral("led"))),
            horn_record: Rc::new(RefCell::new(TuningSettings::neutral("horn"))),
            calibration: Rc::new(RefCell::new(None)),
            vjoy_polling: Rc::new(Cell::new(false)),
            button_actions: Rc::new(RefCell::new(
                DEFAULT_BUTTON_ACTIONS
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect(),
            )),
            pending_action: Rc::new(RefCell::new(None)),
            long_press_actions: Rc::new(RefCell::new(vec!["Not set".to_string(); 12])),
            pending_long_action: Rc::new(RefCell::new(None)),
            selected_button: Rc::new(Cell::new(5)),
            edit_kind: Rc::new(Cell::new(0)),
            edit_knob: Rc::new(Cell::new(0)),
            knob_row: Rc::new(Cell::new(0)),
            knob_actions: Rc::new(RefCell::new([
                [
                    "Gear 1".to_string(),
                    "Gear 2".to_string(),
                    "Not set".to_string(),
                ],
                [
                    "Not set".to_string(),
                    "Not set".to_string(),
                    "Not set".to_string(),
                ],
            ])),
            profiles_cache: Rc::new(RefCell::new(Vec::new())),
            games_cache: Rc::new(RefCell::new(Vec::new())),
            game_dialog_edit_id: Rc::new(RefCell::new(None)),
            game_dialog_profile_id: Rc::new(RefCell::new(None)),
            presets_games: Rc::new(RefCell::new(Vec::new())),
            games_profiles: Rc::new(RefCell::new(Vec::new())),
            pending_profile_id: Rc::new(RefCell::new(None)),
            learn_target: Rc::new(Cell::new(0)),
            horn_test_until: Rc::new(Cell::new(None)),
            handbrake_test_until: Rc::new(Cell::new(None)),
            status_until: Rc::new(Cell::new(None)),
            steer_hist: Rc::new(RefCell::new(VecDeque::new())),
            pedal_hist: Rc::new(RefCell::new(VecDeque::new())),
            brake_hist: Rc::new(RefCell::new(VecDeque::new())),
            clutch_hist: Rc::new(RefCell::new(VecDeque::new())),
            frame_cmds: Rc::new(RefCell::new(FrameCmdCache::default())),
            frame_times: Rc::new(RefCell::new(VecDeque::new())),
            frame_paint_times: Rc::new(RefCell::new(VecDeque::new())),
            frame_paint_pending: Rc::new(Cell::new(None)),
            unsaved: Rc::new(Cell::new(false)),
            notifications: Rc::new(RefCell::new(NotificationCenter::load(Some(&dir)))),
            data_dir: Some(dir.clone()),
            live_link: Arc::new(Mutex::new(None)),
            hidden: Arc::new(AtomicBool::new(false)),
            quit_requested: Rc::new(Cell::new(false)),
            page_requests: Arc::new(Mutex::new(VecDeque::new())),
            // DS-49/DS-50: no sampler thread in tests — the cached
            // flag starts Dark and the tests drive it directly.
            os_light: Arc::new(AtomicBool::new(false)),
            theme_choice: Rc::new(Cell::new(0)),
            wifi_rejoining: Rc::new(Cell::new(false)),
            wifi_rejoin_since: Rc::new(Cell::new(None)),
            wifi_password_set_known: Rc::new(Cell::new(None)),
            wifi_password_message_for: Rc::new(Cell::new(None)),
            confirm_pending: Rc::new(RefCell::new(None)),
            vjoy_install_started: Rc::new(Cell::new(None)),
            unreachable_notified: Rc::new(Cell::new(false)),
            job_tx,
            job_rx: Rc::new(RefCell::new(job_rx)),
            busy_jobs: Rc::new(RefCell::new(HashMap::new())),
            // F6: generic-job registry.
            next_job_id: Rc::new(Cell::new(0)),
            pending_applies: Rc::new(RefCell::new(HashMap::new())),
            // F5: slow-cache generation the pages last re-synced from.
            last_cache_generation: Rc::new(Cell::new(0)),
            connecting_since: Rc::new(Cell::new(Some(Instant::now()))),
            last_setup_step_at: Rc::new(Cell::new(None)),
            reconnect_in_flight: Rc::new(Cell::new(false)),
            spawn_budget: Arc::new(live_probe::SpawnBudget::new()),
            spawn_latch: Rc::new(RefCell::new(None)),
            last_dead_detail: Rc::new(RefCell::new(None)),
            down_since: Rc::new(Cell::new(None)),
            last_back_at: Rc::new(Cell::new(None)),
            last_flap_at: Rc::new(Cell::new(None)),
            flap_count: Rc::new(Cell::new(0)),
            suppress_connected_notify: Rc::new(Cell::new(false)),
            firmware_check_cache: Rc::new(RefCell::new(None)),
            raw_available: Rc::new(Cell::new(false)),
            presets_count: Rc::new(Cell::new(0)),
            module_states: Rc::new(RefCell::new([true; 4])),
            firmware_update_running: Rc::new(Cell::new(false)),
            last_address_notify: Rc::new(Cell::new(None)),
            steer_push_due: Rc::new(Cell::new(None)),
            pending_field: Rc::new(RefCell::new(None)),
            pending_sensitivity: Rc::new(RefCell::new(None)),
            sensitivity_cached: Rc::new(Cell::new(None)),
            sensitivity_failed_at: Rc::new(Cell::new(None)),
            sensitivity_push_base: Rc::new(Cell::new(None)),
            tuning_push_in_flight: Rc::new(RefCell::new(None)),
            profiles_model_hash: Rc::new(Cell::new(None)),
            games_model_hash: Rc::new(Cell::new(None)),
            presets_model_hash: Rc::new(Cell::new(None)),
            led_preview_key: Rc::new(Cell::new((-1, 255, u32::MAX))),
            diag_buttons_hash: Rc::new(Cell::new(None)),
            diag_events_hash: Rc::new(Cell::new(None)),
            last_frame: Rc::new(RefCell::new(None)),
            // Tests drive handlers directly (no frame timer), so
            // there is no first tick to run the deferred startup
            // syncs — they stay off here.
            startup_sync_pending: Rc::new(Cell::new(false)),
            notif_save_pending: Rc::new(Cell::new(false)),
            notif_last_save: Rc::new(Cell::new(None)),
        });
        (ctx, dir)
    }

    /// Drain finished jobs until none is busy (up to ~3 s) — the
    /// F6 handlers apply on the drain, so tests assert after this,
    /// not right after the invoke.
    fn drain_all_jobs(ctx: &Ctx, win: &MainWindow) {
        for _ in 0..150 {
            ctx.drain_jobs(win);
            if ctx.busy_jobs.borrow().is_empty() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// FC2 end-to-end on a real `MainWindow` (needs a display — run
    /// under Xvfb; skips gracefully headless so the plain suite
    /// stays green). Drives the four new callbacks through the
    /// generated `invoke_*` methods and the busy/daemon-error props
    /// through the real setters, asserting the window props:
    /// - busy on at job start, off after the job drains;
    /// - daemon-error set from a planted fatal file, cleared live;
    /// - the setup wizard opens at the Connect step;
    /// - a bad address reverts the field + notifies;
    /// - keep-tray reverts on a failed persist (sim has no daemon);
    /// - free-text rename notifies (sim cannot persist it);
    /// - a failed Extras toggle reverts the `-on` prop.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn fc2_window_wiring_end_to_end() {
        let Ok(Ok(win)) = std::panic::catch_unwind(MainWindow::new) else {
            eprintln!("FC2 window test skipped: no usable display/backend");
            return;
        };
        let (ctx, dir) = fc2_test_ctx();
        wire_callbacks(&win, &ctx);
        let unread = || ctx.notifications.borrow().unread_count();

        // Props start clean.
        assert!(!win.get_busy());
        assert_eq!(win.get_busy_label().as_str(), "");
        assert_eq!(win.get_daemon_error().as_str(), "");

        // 1. Busy on/off around a real background job.
        let started = ctx.run_job(&win, JobKind::FirmwareCheck, "Checking firmware…", || {
            JobDone::FirmwareCheck(Err(UserError {
                code: "fc2_test".to_string(),
                title: "Test failure".to_string(),
                what_happened: "The test job failed on purpose.".to_string(),
                what_to_do: "Ignore this.".to_string(),
                detail: None,
            }))
        });
        assert!(started, "job should start");
        assert!(win.get_busy(), "strip shows while the job runs");
        assert_eq!(win.get_busy_label().as_str(), "Checking firmware…");
        let mut drained = false;
        for _ in 0..100 {
            ctx.drain_jobs(&win);
            if ctx.busy_jobs.borrow().is_empty() {
                drained = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(drained, "job should finish and drain");
        assert!(!win.get_busy(), "strip hides when the job ends");
        assert_eq!(win.get_busy_label().as_str(), "");

        // 2. Daemon-error set from a fatal file, cleared when live.
        std::fs::write(
            dir.join("daemon-status.json"),
            r#"{"status":"fatal","code":"IpcBindFailed","detail":"daemon IPC bind failed after 5 attempts","pid":123,"at_unix":1}"#,
        )
        .expect("fatal status file");
        ctx.mark_dead(&win);
        assert_eq!(
            win.get_daemon_error().as_str(),
            "RSH IPC bind failed after 5 attempts"
        );
        {
            let mut state = ctx.state.borrow_mut();
            state.live = true;
        }
        ctx.sync_daemon_error(&win);
        assert_eq!(win.get_daemon_error().as_str(), "");
        {
            let mut state = ctx.state.borrow_mut();
            state.live = false;
        }

        // 3. The setup wizard opens at the Connect step (D16).
        ctx.open_setup_wizard(&win);
        assert!(win.get_setup_mode(), "wizard must open");
        assert_eq!(win.get_setup_step(), 0, "wizard opens at Connect");

        // 4. A bad address reverts the field + notifies once. The
        // live address is whatever `sync_overview` derives (the sim
        // reports "rsh.local" as its connection address — the live
        // daemon reports the real board IP); capture it first.
        ctx.sync_overview(&win);
        let live_addr = win.get_setup_address().as_str().to_string();
        win.set_setup_address("8.8.8.8".into());
        let before = unread();
        win.invoke_address_accepted("8.8.8.8".into());
        assert_eq!(
            win.get_setup_address().as_str(),
            live_addr,
            "bad address reverts to the live address"
        );
        assert_eq!(unread(), before + 1, "bad address notifies");
        // A valid address the sim cannot persist also notifies
        // instead of failing silently.
        let before = unread();
        win.invoke_address_accepted("192.168.4.1".into());
        // F6: the persist is a job — drain before asserting.
        drain_all_jobs(&ctx, &win);
        assert_eq!(unread(), before + 1, "unpersistable address notifies");
        assert_eq!(win.get_setup_address().as_str(), live_addr);

        // 5. Keep-tray reverts on a failed persist (sim: no daemon).
        win.set_keep_in_tray(true);
        let before = unread();
        win.invoke_keep_tray_selected(false);
        // F6: the persist is a job — drain before asserting.
        drain_all_jobs(&ctx, &win);
        assert!(
            win.get_keep_in_tray(),
            "failed persist reverts the switch to on"
        );
        assert_eq!(unread(), before + 1, "failed persist notifies");

        // 6. Free-text rename: seeded profile, sim cannot persist —
        // the handler still runs and notifies (empty / same-name
        // branches notify too).
        ctx.profiles_cache.borrow_mut().push(ProfileSummary {
            id: "p1".to_string(),
            name: "GT3 Sprint".to_string(),
            game: Some("assetto_corsa".to_string()),
            active: true,
        });
        let before = unread();
        win.invoke_popover_rename_confirmed(0, "FC2 Rename".into());
        // F6: the rename is a job — drain before asserting.
        drain_all_jobs(&ctx, &win);
        assert_eq!(unread(), before + 1, "rename failure notifies");
        let before = unread();
        win.invoke_popover_rename_confirmed(0, "   ".into());
        assert_eq!(unread(), before + 1, "empty rename notifies");
        let before = unread();
        win.invoke_popover_rename_confirmed(0, "GT3 Sprint".into());
        assert_eq!(unread(), before + 1, "same-name rename notifies");

        // 7. Failed Extras toggle reverts the `-on` prop (sim:
        // set_module_enabled errors without a daemon).
        win.set_extras_handbrake_on(false); // the .slint flip
        win.invoke_extras_action("handbrake-toggle".into());
        // F6: the toggle is a job — drain before asserting.
        drain_all_jobs(&ctx, &win);
        assert!(
            win.get_extras_handbrake_on(),
            "failed toggle reverts the handbrake switch to on"
        );
        // The other `-on` props are Rust-writable too.
        set_extras_on_prop(&win, 3, false);
        assert!(!win.get_extras_leds_on());
        set_extras_on_prop(&win, 3, true);
        assert!(win.get_extras_leds_on());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// R2-10: a pedal deadzone drag moves the window prop at once
    /// (local echo) — it must not wait for the ~1 Hz re-sync, and
    /// the local record agrees so the next re-sync does not snap
    /// it back.
    #[test]
    fn r2_10_pedal_deadzone_drag_updates_prop_locally() {
        let Ok(Ok(win)) = std::panic::catch_unwind(MainWindow::new) else {
            eprintln!("R2-10 window test skipped: no usable display/backend");
            return;
        };
        let (ctx, dir) = fc2_test_ctx();
        wire_callbacks(&win, &ctx);
        ctx.sync_pedals(&win);
        assert_eq!(win.get_throttle_deadzone(), 0.0);
        win.invoke_pedal_deadzone_changed(0, 0.5);
        assert_eq!(win.get_throttle_deadzone(), 0.5);
        assert_eq!(win.get_throttle_deadzone_text().as_str(), "5%");
        assert_eq!(ctx.pedals.borrow().records[0].deadzone, 5.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Workstream C: the new pedal tuning controls sync — curve /
    /// ramp sliders echo locally with the right fraction + text,
    /// saturation derives from the record, and the combined pedals
    /// switch + typed values push through `set_tuning`.
    #[test]
    fn pedal_tuning_controls_sync_to_window() {
        let Ok(Ok(win)) = std::panic::catch_unwind(MainWindow::new) else {
            eprintln!("Workstream C window test skipped: no usable display/backend");
            return;
        };
        let (ctx, dir) = fc2_test_ctx();
        wire_callbacks(&win, &ctx);
        ctx.sync_pedals(&win);
        // Sim neutral = task defaults: throttle curve 1.00, tip-in
        // 100 ms, lift-off 50 ms, saturation 100%, combined off;
        // brake curve 1.30, bite 50 ms; clutch curve 1.00, release
        // 300 ms.
        assert!((win.get_throttle_curve_value() - 1.0_f32 / 3.0).abs() < 0.001);
        assert_eq!(win.get_throttle_curve_text().as_str(), "1.00");
        assert!((win.get_throttle_press_ms_value() - 0.2).abs() < 0.001);
        assert_eq!(win.get_throttle_press_ms_text().as_str(), "100 ms");
        assert!((win.get_throttle_release_ms_value() - 0.1).abs() < 0.001);
        assert_eq!(win.get_throttle_release_ms_text().as_str(), "50 ms");
        assert_eq!(win.get_throttle_saturation_value(), 1.0);
        assert_eq!(win.get_throttle_saturation_text().as_str(), "100%");
        assert!(!win.get_combine_pedals());
        assert!((win.get_brake_curve_value() - 0.8_f32 / 1.5).abs() < 0.001);
        assert_eq!(win.get_brake_curve_text().as_str(), "1.30");
        assert!((win.get_brake_press_ms_value() - 50.0 / 300.0).abs() < 0.001);
        assert_eq!(win.get_brake_press_ms_text().as_str(), "50 ms");
        assert!((win.get_clutch_curve_value() - 1.0_f32 / 3.0).abs() < 0.001);
        assert_eq!(win.get_clutch_curve_text().as_str(), "1.00");
        assert!((win.get_clutch_release_ms_value() - 0.3).abs() < 0.001);
        assert_eq!(win.get_clutch_release_ms_text().as_str(), "300 ms");
        // C2: clutch slip zone defaults to 15% (frac (0.15-0.05)/0.35).
        assert!((win.get_clutch_bite_width() - (0.15_f32 - 0.05) / 0.35).abs() < 0.001);
        assert_eq!(win.get_clutch_bite_width_text().as_str(), "15%");
        // Drags echo locally (frac + text + local record).
        win.invoke_pedal_curve_changed(0, 0.5);
        assert_eq!(win.get_throttle_curve_value(), 0.5);
        assert_eq!(win.get_throttle_curve_text().as_str(), "1.25");
        assert_eq!(ctx.pedals.borrow().records[0].curve, 1.25);
        win.invoke_pedal_press_ms_changed(1, 0.5);
        assert_eq!(win.get_brake_press_ms_value(), 0.5);
        assert_eq!(win.get_brake_press_ms_text().as_str(), "150 ms");
        assert_eq!(ctx.pedals.borrow().records[1].press_ms, Some(150.0));
        win.invoke_pedal_release_ms_changed(2, 0.5);
        assert_eq!(win.get_clutch_release_ms_value(), 0.5);
        assert_eq!(win.get_clutch_release_ms_text().as_str(), "500 ms");
        assert_eq!(ctx.pedals.borrow().records[2].release_ms, Some(500.0));
        // C2: slip-zone drag to full (1.0 of 5..40%) echoes 40%.
        win.invoke_pedal_clutch_bite_width_changed(1.0);
        assert_eq!(win.get_clutch_bite_width(), 1.0);
        assert_eq!(win.get_clutch_bite_width_text().as_str(), "40%");
        assert_eq!(ctx.pedals.borrow().records[2].bite_width, Some(0.4));
        // Saturation 0.5 → 85% of the (uncalibrated) 3300 mV span.
        win.invoke_throttle_saturation_changed(0.5);
        assert_eq!(win.get_throttle_saturation_value(), 0.5);
        assert_eq!(win.get_throttle_saturation_text().as_str(), "85%");
        assert_eq!(ctx.pedals.borrow().records[0].max_mv, 2805.0);
        // Typed values + the toggle push through `set_tuning`
        // (drain the job, then the window re-syncs from the return).
        win.invoke_pedal_curve_edited(1, "1.80".into());
        drain_all_jobs(&ctx, &win);
        assert!((win.get_brake_curve_value() - 1.3_f32 / 1.5).abs() < 0.001);
        assert_eq!(win.get_brake_curve_text().as_str(), "1.80");
        win.invoke_pedal_press_ms_edited(0, "250 ms".into());
        drain_all_jobs(&ctx, &win);
        assert_eq!(win.get_throttle_press_ms_value(), 0.5);
        assert_eq!(win.get_throttle_press_ms_text().as_str(), "250 ms");
        win.invoke_combine_pedals_changed(true);
        drain_all_jobs(&ctx, &win);
        assert!(win.get_combine_pedals());
        assert_eq!(ctx.api().get_tuning("throttle").combine_pedals, Some(true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Workstream G (H1–H3): the handbrake tuning controls sync —
    /// latch + auto-release default off, grab speed defaults to
    /// 0 ms (instant); the grab drag echoes locally and the latch
    /// toggle pushes through `set_tuning`.
    #[test]
    fn handbrake_tuning_controls_sync_to_window() {
        let Ok(Ok(win)) = std::panic::catch_unwind(MainWindow::new) else {
            eprintln!("Workstream G window test skipped: no usable display/backend");
            return;
        };
        let (ctx, dir) = fc2_test_ctx();
        wire_callbacks(&win, &ctx);
        ctx.sync_extras(&win);
        // Sim neutral = task defaults: latch off, auto-release off,
        // grab 0 ms.
        assert!(!win.get_handbrake_latch());
        assert_eq!(win.get_handbrake_grab_ms(), 0.0);
        assert_eq!(win.get_handbrake_grab_ms_text().as_str(), "0 ms");
        assert!(!win.get_handbrake_auto_release());
        // Grab drag echoes locally (frac + text + local record).
        win.invoke_extras_handbrake_grab_ms_changed(0.5);
        assert_eq!(win.get_handbrake_grab_ms(), 0.5);
        assert_eq!(win.get_handbrake_grab_ms_text().as_str(), "100 ms");
        assert_eq!(ctx.handbrake_record.borrow().press_ms, Some(100.0));
        // The latch + auto-release toggles push through `set_tuning`
        // (drain the job, then the window re-syncs from the return).
        win.invoke_extras_handbrake_latch_changed(true);
        drain_all_jobs(&ctx, &win);
        assert!(win.get_handbrake_latch());
        assert_eq!(
            ctx.api().get_tuning("handbrake").handbrake_latch,
            Some(true)
        );
        win.invoke_extras_handbrake_auto_release_changed(true);
        drain_all_jobs(&ctx, &win);
        assert!(win.get_handbrake_auto_release());
        assert_eq!(
            ctx.api().get_tuning("handbrake").handbrake_auto_release,
            Some(true)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// R2-18: a steering push that times out snaps the slider back
    /// to the stored value and tells the user — the UI must never
    /// keep showing an unapplied value.
    #[test]
    fn r2_18_tuning_push_timeout_reverts_steering() {
        let Ok(Ok(win)) = std::panic::catch_unwind(MainWindow::new) else {
            eprintln!("R2-18 window test skipped: no usable display/backend");
            return;
        };
        let (ctx, dir) = fc2_test_ctx();
        wire_callbacks(&win, &ctx);
        // Stored steering deadzone is 0 (sim neutral); drag it to 9.
        ctx.tuning.borrow_mut().set_deadzone(9.0);
        ctx.refresh(&win, 0);
        // Bug 7: the deadzone cap is 30% now, so 9% is 9/30.
        assert_eq!(win.get_deadzone(), 0.3);
        // A push started 6 s ago (past the 5 s TuningPush timeout)
        // and still pending.
        let due = Instant::now()
            .checked_sub(Duration::from_secs(6))
            .expect("6 s ago exists");
        ctx.steer_push_due.set(Some(due));
        let base = ctx.steering.borrow().clone();
        ctx.busy_jobs.borrow_mut().insert(
            JobKind::TuningPush,
            (
                "Saving steering…".to_string(),
                Instant::now()
                    .checked_sub(Duration::from_secs(6))
                    .expect("6 s ago exists"),
            ),
        );
        *ctx.tuning_push_in_flight.borrow_mut() = Some(TuningPushTarget::Steering { due, base });
        ctx.check_job_timeouts(&win);
        assert!(ctx.busy_jobs.borrow().is_empty(), "timed-out job clears");
        assert_eq!(
            ctx.steer_push_due.get(),
            None,
            "pending flag clears on timeout"
        );
        assert_eq!(
            ctx.tuning.borrow().deadzone_pct,
            0.0,
            "slider snaps back to stored"
        );
        assert_eq!(win.get_deadzone(), 0.0);
        assert_eq!(
            win.get_status_title().as_str(),
            "Saving steering… timed out"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Bug 7 (PC-test fix): the Steering Advanced controls sync —
    /// invert toggle, lock slider (snapped to steps), smoothing
    /// slider — from callback to local tuning to window props, and
    /// the deadzone cap is 30% (full-scale drag = 30%).
    /// (S2+S3: the balance and centre-trim sliders sync the same
    /// way — fractions over −20..+20% and −5..+5%.)
    #[test]
    fn steering_advanced_controls_sync_to_window_and_tuning() {
        let Ok(Ok(win)) = std::panic::catch_unwind(MainWindow::new) else {
            eprintln!("Bug 7 window test skipped: no usable display/backend");
            return;
        };
        let (ctx, dir) = fc2_test_ctx();
        wire_callbacks(&win, &ctx);
        ctx.refresh(&win, 0);
        // Sim neutral: not inverted, full lock, no smoothing.
        assert!(!win.get_steer_invert());
        assert_eq!(win.get_steer_lock(), 1.0);
        assert_eq!(win.get_steer_lock_text().as_str(), "900°");
        assert_eq!(win.get_steer_smoothing(), 0.0);
        // S2+S3 defaults: even balance (0.5), no trim (0.5).
        assert_eq!(win.get_steer_balance(), 0.5);
        assert_eq!(win.get_steer_balance_text().as_str(), "0%");
        assert_eq!(win.get_steer_trim(), 0.5);
        assert_eq!(win.get_steer_trim_text().as_str(), "0%");
        // Invert on, lock slider mid-range (585° → snaps to 540°),
        // smoothing to 30%.
        win.invoke_invert_changed(true);
        win.invoke_lock_changed(0.5);
        win.invoke_smoothing_changed(0.3);
        assert!(ctx.tuning.borrow().invert);
        assert_eq!(ctx.tuning.borrow().lock_deg, 540.0);
        assert_eq!(ctx.tuning.borrow().smoothing_pct, 30.0);
        ctx.refresh(&win, 0);
        assert!(win.get_steer_invert());
        assert!((win.get_steer_lock() - (540.0 - 270.0) / 630.0).abs() < 0.001);
        assert_eq!(win.get_steer_lock_text().as_str(), "540°");
        assert_eq!(win.get_steer_smoothing_text().as_str(), "30%");
        // Balance slider to 0.75 (+10%) and trim to 0.2 (−3%):
        // local tuning moves, window props follow on refresh.
        win.invoke_balance_changed(0.75);
        win.invoke_trim_changed(0.2);
        assert_eq!(ctx.tuning.borrow().balance_pct, 10.0);
        assert_eq!(ctx.tuning.borrow().trim_pct, -3.0);
        ctx.refresh(&win, 0);
        assert_eq!(win.get_steer_balance(), 0.75);
        assert_eq!(win.get_steer_balance_text().as_str(), "+10%");
        assert!((win.get_steer_trim() - 0.2).abs() < 0.001);
        assert_eq!(win.get_steer_trim_text().as_str(), "-3%");
        // Text edits parse: "+5%" balance and "-5%" trim.
        win.invoke_balance_edited("+5%".into());
        assert_eq!(ctx.tuning.borrow().balance_pct, 5.0);
        win.invoke_trim_edited("-5%".into());
        assert_eq!(ctx.tuning.borrow().trim_pct, -5.0);
        // Text edits parse and snap: "720°" and "45%".
        win.invoke_lock_edited("720°".into());
        assert_eq!(ctx.tuning.borrow().lock_deg, 720.0);
        win.invoke_smoothing_edited("45%".into());
        assert_eq!(ctx.tuning.borrow().smoothing_pct, 45.0);
        // Deadzone cap is 30: a full-scale drag is 30%.
        win.invoke_deadzone_changed(1.0);
        assert_eq!(ctx.tuning.borrow().deadzone_pct, 30.0);
        ctx.refresh(&win, 0);
        assert_eq!(win.get_deadzone(), 1.0);
        assert_eq!(win.get_deadzone_text().as_str(), "30%");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// R2-11: `refresh_profiles` / `refresh_games` must not rebuild
    /// their list models when the content is unchanged — `ModelRc`
    /// equality is underlying-pointer equality, so an unchanged
    /// model compares equal across two refreshes.
    #[test]
    fn r2_11_refresh_skips_model_rebuild_when_unchanged() {
        let Ok(Ok(win)) = std::panic::catch_unwind(MainWindow::new) else {
            eprintln!("R2-11 window test skipped: no usable display/backend");
            return;
        };
        let (ctx, dir) = fc2_test_ctx();
        wire_callbacks(&win, &ctx);
        ctx.refresh_profiles(&win);
        let first_profiles = win.get_popover_profiles();
        ctx.refresh_profiles(&win);
        assert_eq!(
            win.get_popover_profiles(),
            first_profiles,
            "unchanged profiles must not rebuild the model"
        );
        ctx.refresh_games(&win);
        let first_games = win.get_games();
        ctx.refresh_games(&win);
        assert_eq!(
            win.get_games(),
            first_games,
            "unchanged games must not rebuild the model"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// F6: a generic job shows the busy strip at once, runs its
    /// work off the UI thread (a 2 s sleep must not block the
    /// start), applies on the drain, and a duplicate kind is
    /// ignored while one runs.
    #[test]
    fn generic_job_runs_off_thread_and_applies_on_drain() {
        let Ok(Ok(win)) = std::panic::catch_unwind(MainWindow::new) else {
            eprintln!("generic job test skipped: no usable display/backend");
            return;
        };
        let (ctx, dir) = fc2_test_ctx();
        wire_callbacks(&win, &ctx);
        let started_at = Instant::now();
        let started = ctx.run_generic_job(
            &win,
            JobKind::SettingsOp,
            "Slow test job…",
            move |_api| {
                std::thread::sleep(Duration::from_secs(2));
                JobResult::Unit(Ok(()))
            },
            move |_ctx, _win, _result| {},
        );
        assert!(started, "generic job should start");
        assert!(
            started_at.elapsed() < Duration::from_secs(1),
            "start must not block on the work"
        );
        assert!(win.get_busy(), "strip shows while the job runs");
        assert_eq!(win.get_busy_label().as_str(), "Slow test job…");
        assert!(
            !ctx.run_generic_job(
                &win,
                JobKind::SettingsOp,
                "Slow test job…",
                move |_api| JobResult::Unit(Ok(())),
                move |_ctx, _win, _result| {},
            ),
            "duplicate kind must be ignored while one runs"
        );
        drain_all_jobs(&ctx, &win);
        assert!(!win.get_busy(), "strip hides when the job ends");
        assert_eq!(win.get_busy_label().as_str(), "");
        assert!(
            started_at.elapsed() >= Duration::from_secs(2),
            "the work really ran (2 s sleep)"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// F12/F5: a flap reconnect (down-episode < 10 s) stays silent
    /// — no "Wheel connected" notify — and the count survives; the
    /// next non-flap reconnect folds "flapped ×1" into its notify
    /// and resets the count.
    #[test]
    #[cfg(unix)]
    fn flap_reconnect_suppresses_connected_notify_and_folds_on_stable() {
        let Ok(Ok(win)) = std::panic::catch_unwind(MainWindow::new) else {
            eprintln!("F12 gate test skipped: no usable display/backend");
            return;
        };
        let _guard = crate::live_probe::TEST_IPC_PIPE_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = std::env::temp_dir().join(format!("rsh-f12-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let sock = dir.join("rsh-daemon.sock");
        let api: Arc<dyn AppApi> = Arc::new(SimApi::new());
        let (tx, _rx) = tokio::sync::broadcast::channel(64);
        let server = Arc::new(rsh_ipc::IpcServer::new(api, tx));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
        let sock_clone = sock.clone();
        let handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("server runtime");
            rt.block_on(async move {
                let listener = rsh_ipc::bind_unix_socket(&sock_clone).expect("bind");
                ready_tx.send(()).expect("ready signal");
                if let Ok((stream, _)) = listener.accept().await {
                    server.serve_stream(stream).await;
                }
            });
        });
        ready_rx.recv().expect("server bound");
        let live_api = IpcApi::connect(&sock).expect("connect");
        let (ctx, ctx_dir) = fc2_test_ctx();
        wire_callbacks(&win, &ctx);
        let unread = || ctx.notifications.borrow().unread_count();
        let last_body = || {
            ctx.notifications
                .borrow()
                .to_model_items()
                .into_iter()
                .next_back()
                .map(|item| item.body.as_str().to_string())
                .unwrap_or_default()
        };

        // Flap reconnect: down 5 s ago — silent, count kept.
        ctx.state.borrow_mut().live = true;
        ctx.down_since.set(Some(
            Instant::now()
                .checked_sub(Duration::from_secs(5))
                .expect("5 s ago exists"),
        ));
        let before = unread();
        ctx.swap_to_live(live_api.clone());
        assert!(
            ctx.suppress_connected_notify.get(),
            "flap must arm suppression"
        );
        assert_eq!(ctx.flap_count.get(), 1, "flap must count");
        ctx.apply_reconnect_seed(
            &win,
            live_api.clone(),
            crate::ipc_api::LiveSeed::fetch(&live_api, ApiTransport::Wifi),
        );
        assert_eq!(unread(), before, "flap reconnect must not notify");
        assert_eq!(
            ctx.flap_count.get(),
            1,
            "count survives a suppressed reconnect"
        );

        // Stable reconnect: down 60 s ago — notifies with the fold,
        // count resets.
        ctx.down_since.set(Some(
            Instant::now()
                .checked_sub(Duration::from_secs(60))
                .expect("60 s ago exists"),
        ));
        let before = unread();
        ctx.swap_to_live(live_api.clone());
        assert!(
            !ctx.suppress_connected_notify.get(),
            "long episode must not suppress"
        );
        ctx.apply_reconnect_seed(
            &win,
            live_api.clone(),
            crate::ipc_api::LiveSeed::fetch(&live_api, ApiTransport::Wifi),
        );
        assert_eq!(unread(), before + 1, "stable reconnect must notify once");
        assert!(
            last_body().contains("flapped ×1"),
            "stable reconnect must fold the count in, got: {}",
            last_body()
        );
        assert_eq!(ctx.flap_count.get(), 0, "count resets after folding");

        // Teardown: `mark_dead` drops the live link (F7) so the
        // server thread ends when the client goes away.
        ctx.mark_dead(&win);
        drop(live_api);
        handle.join().expect("server thread");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&ctx_dir);
    }

    /// F26: a seed whose session `connect` failed with
    /// `Unreachable` (no board — the expected disconnected state)
    /// must not notify "Reconnect failed"; a real transport
    /// failure still must. The flap-suppressed setup keeps the
    /// "Wheel connected" notify out of the count, so the only
    /// notify under test is the Reconnect-failed one.
    #[test]
    #[cfg(unix)]
    fn reconnect_seed_unreachable_stays_silent_but_transport_closed_notifies() {
        let Ok(Ok(win)) = std::panic::catch_unwind(MainWindow::new) else {
            eprintln!("F26 gate test skipped: no usable display/backend");
            return;
        };
        let _guard = crate::live_probe::TEST_IPC_PIPE_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = std::env::temp_dir().join(format!("rsh-f26-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let sock = dir.join("rsh-daemon.sock");
        let api: Arc<dyn AppApi> = Arc::new(SimApi::new());
        let (tx, _rx) = tokio::sync::broadcast::channel(64);
        let server = Arc::new(rsh_ipc::IpcServer::new(api, tx));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
        let sock_clone = sock.clone();
        let handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("server runtime");
            rt.block_on(async move {
                let listener = rsh_ipc::bind_unix_socket(&sock_clone).expect("bind");
                ready_tx.send(()).expect("ready signal");
                if let Ok((stream, _)) = listener.accept().await {
                    server.serve_stream(stream).await;
                }
            });
        });
        ready_rx.recv().expect("server bound");
        let live_api = IpcApi::connect(&sock).expect("connect");
        let (ctx, ctx_dir) = fc2_test_ctx();
        wire_callbacks(&win, &ctx);
        let unread = || ctx.notifications.borrow().unread_count();
        let last_title = || {
            ctx.notifications
                .borrow()
                .to_model_items()
                .into_iter()
                .next_back()
                .map(|item| item.title.as_str().to_string())
                .unwrap_or_default()
        };
        let seed_with = |connect: Result<ConnectionInfo, UserError>| crate::ipc_api::LiveSeed {
            backend: "vJoy".to_string(),
            autostart: true,
            connect,
            wifi: WifiStatus {
                ssid: None,
                connected: false,
                password_set: false,
                board_mac: None,
            },
        };

        // Flap setup: down 5 s ago — the "Wheel connected" notify
        // is suppressed, so only a Reconnect-failed notify could
        // move the count.
        ctx.state.borrow_mut().live = true;
        ctx.down_since.set(Some(
            Instant::now()
                .checked_sub(Duration::from_secs(5))
                .expect("5 s ago exists"),
        ));
        let before = unread();
        ctx.swap_to_live(live_api.clone());
        ctx.apply_reconnect_seed(
            &win,
            live_api.clone(),
            seed_with(Err(UserError::from_connection(
                ConnectionError::Unreachable,
                "no board answered",
            ))),
        );
        assert_eq!(
            unread(),
            before,
            "a no-board reconnect must not notify 'Reconnect failed'"
        );

        ctx.apply_reconnect_seed(
            &win,
            live_api.clone(),
            seed_with(Err(UserError::from_connection(
                ConnectionError::TransportClosed,
                "link dropped",
            ))),
        );
        assert_eq!(
            unread(),
            before + 1,
            "a real transport failure must notify once"
        );
        assert_eq!(last_title(), "Reconnect failed");

        // Teardown: `mark_dead` drops the live link (F7) so the
        // server thread ends when the client goes away.
        ctx.mark_dead(&win);
        drop(live_api);
        handle.join().expect("server thread");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&ctx_dir);
    }

    /// B1 (PC-test Bug 1): a reconnect seed whose session `connect`
    /// failed with `Unreachable` (board offline — the expected
    /// disconnected state) must NOT toast "Wheel connected / Live
    /// data is back": the daemon link coming back is not the board
    /// connecting, and the Overview already shows "Wheel not
    /// found". A seed whose `connect` is Ok (board really
    /// connected) must still notify once, titled "Wheel connected"
    /// — the daemon link and the board connection are different
    /// things and must not read as one. Non-flap setup (down 60 s
    /// ago) so the flap suppression cannot hide the result.
    #[test]
    #[cfg(unix)]
    #[allow(clippy::too_many_lines)]
    fn reconnect_seed_unreachable_never_toasts_connected_but_ok_seed_does() {
        let Ok(Ok(win)) = std::panic::catch_unwind(MainWindow::new) else {
            eprintln!("B1 gate test skipped: no usable display/backend");
            return;
        };
        let _guard = crate::live_probe::TEST_IPC_PIPE_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = std::env::temp_dir().join(format!("rsh-b1-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let sock = dir.join("rsh-daemon.sock");
        let api: Arc<dyn AppApi> = Arc::new(SimApi::new());
        let (tx, _rx) = tokio::sync::broadcast::channel(64);
        let server = Arc::new(rsh_ipc::IpcServer::new(api, tx));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
        let sock_clone = sock.clone();
        let handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("server runtime");
            rt.block_on(async move {
                let listener = rsh_ipc::bind_unix_socket(&sock_clone).expect("bind");
                ready_tx.send(()).expect("ready signal");
                if let Ok((stream, _)) = listener.accept().await {
                    server.serve_stream(stream).await;
                }
            });
        });
        ready_rx.recv().expect("server bound");
        let live_api = IpcApi::connect(&sock).expect("connect");
        let (ctx, ctx_dir) = fc2_test_ctx();
        wire_callbacks(&win, &ctx);
        let unread = || ctx.notifications.borrow().unread_count();
        let last_title = || {
            ctx.notifications
                .borrow()
                .to_model_items()
                .into_iter()
                .next_back()
                .map(|item| item.title.as_str().to_string())
                .unwrap_or_default()
        };
        let seed_with = |connect: Result<ConnectionInfo, UserError>| crate::ipc_api::LiveSeed {
            backend: "vJoy".to_string(),
            autostart: true,
            connect,
            wifi: WifiStatus {
                ssid: None,
                connected: false,
                password_set: false,
                board_mac: None,
            },
        };
        let connected_info = ConnectionInfo {
            state: ConnectionState::Active,
            label: "Connected".to_string(),
            transport: ApiTransport::Wifi,
            address: "192.168.4.1".to_string(),
            session_active: true,
            board_reachable: true,
            error: None,
            hint: None,
        };

        // Non-flap setup: down 60 s ago — the connected notify is
        // NOT suppressed, so a wrong toast would show up here.
        ctx.state.borrow_mut().live = true;
        ctx.down_since.set(Some(
            Instant::now()
                .checked_sub(Duration::from_secs(60))
                .expect("60 s ago exists"),
        ));
        let before = unread();
        ctx.swap_to_live(live_api.clone());
        assert!(
            !ctx.suppress_connected_notify.get(),
            "long episode must not suppress"
        );
        ctx.apply_reconnect_seed(
            &win,
            live_api.clone(),
            seed_with(Err(UserError::from_connection(
                ConnectionError::Unreachable,
                "no board answered",
            ))),
        );
        assert_eq!(
            unread(),
            before,
            "a no-board reconnect must not notify 'Wheel connected'"
        );
        assert_eq!(
            win.get_status_text().as_str(),
            "",
            "a no-board reconnect must not show any toast"
        );

        // Board really connected: one notify + toast, titled for
        // the wheel, not the daemon link.
        ctx.apply_reconnect_seed(&win, live_api.clone(), seed_with(Ok(connected_info)));
        assert_eq!(
            unread(),
            before + 1,
            "a board-connected reconnect must notify once"
        );
        assert_eq!(last_title(), "Wheel connected");
        assert!(
            win.get_status_text().as_str().contains("Live data is back"),
            "the connected toast must show, got: {}",
            win.get_status_text()
        );

        // Teardown: `mark_dead` drops the live link (F7) so the
        // server thread ends when the client goes away.
        ctx.mark_dead(&win);
        drop(live_api);
        handle.join().expect("server thread");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&ctx_dir);
    }

    /// F27: a reconnect seed that lands after the window was
    /// hidden to tray must swap the live state but touch no window
    /// prop and push no notification — the close-vs-seed-apply
    /// race `SIGSEGV`ed under Xvfb (F17-FINDING-5). A generic job
    /// that finishes while hidden must likewise never run its
    /// apply. Reopening re-syncs the window
    /// (`resync_after_reopen`): the live flag comes back and no
    /// deferred notify fires twice.
    #[test]
    #[cfg(unix)]
    fn seed_and_generic_job_landing_while_hidden_skip_the_window() {
        let Ok(Ok(win)) = std::panic::catch_unwind(MainWindow::new) else {
            eprintln!("F27 gate test skipped: no usable display/backend");
            return;
        };
        let _guard = crate::live_probe::TEST_IPC_PIPE_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = std::env::temp_dir().join(format!("rsh-f27-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let sock = dir.join("rsh-daemon.sock");
        let api: Arc<dyn AppApi> = Arc::new(SimApi::new());
        let (tx, _rx) = tokio::sync::broadcast::channel(64);
        let server = Arc::new(rsh_ipc::IpcServer::new(api, tx));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
        let sock_clone = sock.clone();
        let handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("server runtime");
            rt.block_on(async move {
                let listener = rsh_ipc::bind_unix_socket(&sock_clone).expect("bind");
                ready_tx.send(()).expect("ready signal");
                if let Ok((stream, _)) = listener.accept().await {
                    server.serve_stream(stream).await;
                }
            });
        });
        ready_rx.recv().expect("server bound");
        let live_api = IpcApi::connect(&sock).expect("connect");
        let (ctx, ctx_dir) = fc2_test_ctx();
        wire_callbacks(&win, &ctx);
        let unread = || ctx.notifications.borrow().unread_count();

        // Go live while visible, then hide (the close).
        ctx.swap_to_live(live_api.clone());
        assert!(!win.get_daemon_live(), "window starts not-live");
        ctx.hidden.store(true, Ordering::SeqCst);
        let before = unread();
        ctx.apply_reconnect_seed(
            &win,
            live_api.clone(),
            crate::ipc_api::LiveSeed::fetch(&live_api, ApiTransport::Wifi),
        );
        assert!(ctx.is_live(), "the state still swaps while hidden");
        assert!(
            !win.get_daemon_live(),
            "no window write while hidden — the close-vs-apply race"
        );
        assert_eq!(unread(), before, "no notify while hidden");

        // A generic job finishing while hidden never runs its apply.
        let applied = std::rc::Rc::new(std::cell::Cell::new(false));
        let applied_in_job = applied.clone();
        assert!(
            ctx.run_generic_job(
                &win,
                JobKind::SettingsOp,
                "Hidden test job…",
                move |_api| JobResult::Unit(Ok(())),
                move |_ctx, _win, _result| {
                    applied_in_job.set(true);
                },
            ),
            "job should start"
        );
        drain_all_jobs(&ctx, &win);
        assert!(
            !applied.get(),
            "a job that finished while hidden must not apply"
        );

        // Reopen: the live flag comes back, with no duplicate notify.
        ctx.hidden.store(false, Ordering::SeqCst);
        ctx.resync_after_reopen(&win);
        assert!(
            win.get_daemon_live(),
            "reopen re-syncs the live flag the hidden apply skipped"
        );
        assert_eq!(unread(), before, "reopen must not re-notify");

        // Teardown: `mark_dead` drops the live link (F7) so the
        // server thread ends when the client goes away.
        ctx.mark_dead(&win);
        drop(live_api);
        handle.join().expect("server thread");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&ctx_dir);
    }

    /// H-4 (FD): a flash whose daemon status goes idle mid-update
    /// (daemon died/restarted — the slot is neither running nor
    /// done) must fail closed: the running flag clears, the user is
    /// told the update was interrupted, and the Settings firmware
    /// line stops saying "Updating firmware…" (B5: never infinite
    /// loading). The sim reports the idle status, which is exactly
    /// what a dead daemon looks like to the poller.
    #[test]
    fn firmware_update_poll_fails_closed_when_status_goes_idle() {
        let Ok(Ok(win)) = std::panic::catch_unwind(MainWindow::new) else {
            eprintln!("firmware poll test skipped: no usable display/backend");
            return;
        };
        let (ctx, dir) = fc2_test_ctx();
        wire_callbacks(&win, &ctx);
        let unread = || ctx.notifications.borrow().unread_count();
        assert!(!ctx.firmware_update_running.get());

        // Mid-flash state: the flag is set and the line shows progress.
        ctx.firmware_update_running.set(true);
        win.set_firmware_text("Updating firmware… uploading 42%".into());
        let before = unread();
        ctx.poll_firmware_update(&win);

        assert!(
            !ctx.firmware_update_running.get(),
            "idle status must clear the running flag"
        );
        assert!(
            unread() > before,
            "the interrupted update must notify the user"
        );
        assert!(
            !win.get_firmware_text().contains("Updating firmware"),
            "firmware line must stop showing progress: {}",
            win.get_firmware_text()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// H-4 (FD): while a flash runs, "Update firmware" again must
    /// not start a second job — it notifies "already running" and
    /// the busy map stays empty (the daemon's own already-running
    /// refusal is the backstop, not the user-facing answer).
    #[test]
    fn firmware_update_start_is_ignored_while_one_runs() {
        let Ok(Ok(win)) = std::panic::catch_unwind(MainWindow::new) else {
            eprintln!("firmware start-guard test skipped: no usable display/backend");
            return;
        };
        let (ctx, dir) = fc2_test_ctx();
        wire_callbacks(&win, &ctx);
        let unread = || ctx.notifications.borrow().unread_count();

        ctx.firmware_update_running.set(true);
        let before = unread();
        ctx.start_firmware_update_job(&win);

        assert!(
            !ctx.busy_jobs
                .borrow()
                .contains_key(&JobKind::FirmwareUpdate),
            "no second firmware-update job may start while one runs"
        );
        assert!(
            unread() > before,
            "the ignored start must tell the user an update is already running"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn is_flap_down_episode_classifies_by_duration() {
        assert!(is_flap_down_episode(Some(Duration::from_secs(5))));
        assert!(is_flap_down_episode(Some(Duration::from_millis(1))));
        assert!(!is_flap_down_episode(Some(Duration::from_secs(10))));
        assert!(!is_flap_down_episode(Some(Duration::from_secs(60))));
        assert!(!is_flap_down_episode(None), "first connect is not a flap");
    }

    /// F7: `mark_dead` swaps `state.api` back to a fresh `SimApi` —
    /// the old handle is dropped, so a dead link's runtime + poller
    /// die with it instead of spinning on a dead socket.
    #[test]
    fn mark_dead_swaps_api_back_to_sim() {
        let Ok(Ok(win)) = std::panic::catch_unwind(MainWindow::new) else {
            eprintln!("F7 test skipped: no usable display/backend");
            return;
        };
        let (ctx, dir) = fc2_test_ctx();
        wire_callbacks(&win, &ctx);
        ctx.state.borrow_mut().live = true;
        let before = ctx.api();
        ctx.mark_dead(&win);
        assert!(!ctx.is_live(), "dead link must not stay live");
        assert!(
            ctx.state.borrow().live_api.is_none(),
            "live link must be dropped"
        );
        assert!(
            !Arc::ptr_eq(&ctx.api(), &before),
            "the dead API handle must be replaced by a fresh SimApi"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// F3: the respawn latch shows its reason in the banner (there
    /// is no fatal file behind a budget exhaustion) and notifies
    /// once; the Reconnect button stays the manual Retry.
    #[test]
    fn mark_spawn_latched_shows_latch_reason_in_banner() {
        let Ok(Ok(win)) = std::panic::catch_unwind(MainWindow::new) else {
            eprintln!("F3 latch test skipped: no usable display/backend");
            return;
        };
        let (ctx, dir) = fc2_test_ctx();
        wire_callbacks(&win, &ctx);
        let unread = || ctx.notifications.borrow().unread_count();
        // Already down (the normal flow: `DaemonDown` ran `mark_dead`
        // first), so the latch itself notifies exactly once.
        ctx.state.borrow_mut().live = false;
        ctx.state.borrow_mut().connecting = false;
        let reason = "RSH keeps failing to start — automatic restarts stopped after 5 tries. Use Reconnect to try again.";
        let before = unread();
        ctx.mark_spawn_latched(&win, "RespawnBudgetExhausted", reason);
        assert_eq!(unread(), before + 1, "latch must notify exactly once");
        assert_eq!(win.get_daemon_error().as_str(), reason);
        assert!(
            ctx.spawn_latch.borrow().is_some(),
            "latch must be recorded until a live daemon clears it"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// F3: a fatal-file reason `mark_dead` already showed is not
    /// notified twice by the latch — the banner still carries it.
    #[test]
    fn mark_spawn_latched_does_not_repeat_fatal_reason() {
        let Ok(Ok(win)) = std::panic::catch_unwind(MainWindow::new) else {
            eprintln!("F3 latch test skipped: no usable display/backend");
            return;
        };
        let (ctx, dir) = fc2_test_ctx();
        wire_callbacks(&win, &ctx);
        let unread = || ctx.notifications.borrow().unread_count();
        std::fs::write(
            dir.join("daemon-status.json"),
            r#"{"status":"fatal","code":"IpcBindFailed","detail":"daemon IPC bind failed after 5 attempts","pid":123,"at_unix":1}"#,
        )
        .expect("fatal status file");
        let before = unread();
        ctx.mark_spawn_latched(
            &win,
            "IpcBindFailed",
            "daemon IPC bind failed after 5 attempts",
        );
        assert_eq!(
            unread(),
            before + 1,
            "fatal detail must be notified once, by mark_dead, not again by the latch"
        );
        assert_eq!(
            win.get_daemon_error().as_str(),
            "RSH IPC bind failed after 5 attempts"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// F3: a `DaemonFatal` health event latches the banner through
    /// the same drain the frame timer uses.
    #[test]
    fn drain_health_fatal_latches_banner() {
        let Ok(Ok(win)) = std::panic::catch_unwind(MainWindow::new) else {
            eprintln!("F3 drain test skipped: no usable display/backend");
            return;
        };
        let (ctx, dir) = fc2_test_ctx();
        wire_callbacks(&win, &ctx);
        let (tx, rx) = std::sync::mpsc::channel::<HealthEvent>();
        tx.send(HealthEvent::DaemonFatal {
            code: "RespawnBudgetExhausted".to_string(),
            detail: "RSH keeps failing to start — automatic restarts stopped.".to_string(),
        })
        .expect("send fatal");
        drain_health(&rx, &win.as_weak(), &ctx);
        assert_eq!(
            win.get_daemon_error().as_str(),
            "RSH keeps failing to start — automatic restarts stopped."
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// F12: a burst of flap deaths collapses into the first entry —
    /// the second death (seconds after a flap reconnect) stays
    /// silent, and the next death after a stable stretch folds the
    /// count ("flapped ×1") into its notification.
    #[test]
    fn flap_burst_collapses_repeated_deaths() {
        let Ok(Ok(win)) = std::panic::catch_unwind(MainWindow::new) else {
            eprintln!("F12 test skipped: no usable display/backend");
            return;
        };
        let (ctx, dir) = fc2_test_ctx();
        wire_callbacks(&win, &ctx);
        let unread = || ctx.notifications.borrow().unread_count();
        let last_body = || {
            ctx.notifications
                .borrow()
                .to_model_items()
                .into_iter()
                .next_back()
                .map(|item| item.body.as_str().to_string())
                .unwrap_or_default()
        };

        // Death 1: fresh entry.
        ctx.state.borrow_mut().live = true;
        let before = unread();
        ctx.mark_dead(&win);
        assert_eq!(unread(), before + 1, "first death notifies");

        // Flap reconnect, then death 2 seconds later: silent.
        ctx.state.borrow_mut().live = true;
        ctx.flap_count.set(1);
        ctx.last_back_at.set(Some(Instant::now()));
        ctx.last_flap_at.set(Some(Instant::now()));
        ctx.mark_dead(&win);
        assert_eq!(unread(), before + 1, "burst death must stay silent");

        // Stable stretch (60 s up), then death 3: notifies with the
        // folded count, and the count resets.
        ctx.state.borrow_mut().live = true;
        ctx.last_back_at.set(Some(
            Instant::now()
                .checked_sub(Duration::from_secs(60))
                .expect("60 s ago exists"),
        ));
        ctx.mark_dead(&win);
        assert_eq!(unread(), before + 2, "post-burst death notifies");
        assert!(
            last_body().contains("×1"),
            "post-burst death must fold the flap count in, got: {}",
            last_body()
        );
        assert_eq!(ctx.flap_count.get(), 0, "count resets after folding");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // F23: the v2.0.2 report — a user who skipped setup in v2.0.1
    // never saw the wizard again after the update install, because
    // Skip and Finish both wrote the bare word "done".
    #[test]
    fn f23_parse_setup_marker() {
        assert_eq!(
            parse_setup_marker("done 2.0.2\n"),
            SetupMarker::Done("2.0.2".to_string())
        );
        assert_eq!(
            parse_setup_marker("skipped 2.0.1"),
            SetupMarker::Skipped("2.0.1".to_string())
        );
        // Pre-v2.0.3 bare marker: Skip vs Finish unknowable.
        assert_eq!(parse_setup_marker("done\n"), SetupMarker::Legacy);
        assert_eq!(parse_setup_marker("done"), SetupMarker::Legacy);
        // Unreadable / corrupt content re-offers once, like legacy.
        assert_eq!(parse_setup_marker(""), SetupMarker::Legacy);
        assert_eq!(parse_setup_marker("garbage\n"), SetupMarker::Legacy);
        assert_eq!(parse_setup_marker("done \n"), SetupMarker::Legacy);
    }

    #[test]
    fn f23_should_show_decision_matrix() {
        // Fresh profile → wizard shows.
        assert!(setup_marker_should_show(&SetupMarker::Missing, "2.0.3"));
        // Finished setup never re-shows, any version.
        assert!(!setup_marker_should_show(
            &SetupMarker::Done("2.0.2".to_string()),
            "2.0.3"
        ));
        assert!(!setup_marker_should_show(
            &SetupMarker::Done("2.0.3".to_string()),
            "2.0.3"
        ));
        // Skipped setup re-shows once the version changes, and only
        // then — same-version skip stays hidden.
        assert!(setup_marker_should_show(
            &SetupMarker::Skipped("2.0.2".to_string()),
            "2.0.3"
        ));
        assert!(!setup_marker_should_show(
            &SetupMarker::Skipped("2.0.3".to_string()),
            "2.0.3"
        ));
        // Legacy bare "done" (the v2.0.2 report) re-shows once.
        assert!(setup_marker_should_show(&SetupMarker::Legacy, "2.0.3"));
    }

    #[test]
    fn f23_marker_round_trip_through_ctx() {
        let (ctx, dir) = fc2_test_ctx();
        let marker = dir.join("setup-done");
        // Fresh profile → wizard shows.
        assert!(ctx.setup_should_show(), "fresh profile must show wizard");
        // Finish writes a versioned marker → never re-shows.
        ctx.mark_setup_done();
        assert_eq!(
            std::fs::read_to_string(&marker).expect("marker written"),
            format!("done {}\n", app_version())
        );
        assert!(!ctx.setup_should_show(), "finished setup must hide wizard");
        // Reopen (A1) clears the marker → wizard shows again.
        ctx.clear_setup_marker();
        assert!(!marker.is_file(), "reopen must delete the marker");
        assert!(ctx.setup_should_show(), "cleared marker must show wizard");
        // Skip writes a versioned skipped marker → hidden on this
        // version (the skip itself is not re-prompted immediately).
        ctx.mark_setup_skipped();
        assert_eq!(
            std::fs::read_to_string(&marker).expect("marker written"),
            format!("skipped {}\n", app_version())
        );
        assert!(
            !ctx.setup_should_show(),
            "same-version skip must hide wizard"
        );
        // The reported case: a v2.0.1 skip left a bare "done" file,
        // and the v2.0.2 update install never showed the wizard.
        // v2.0.3 treats that legacy marker as "show once".
        std::fs::write(&marker, "done\n").expect("legacy marker");
        assert_eq!(ctx.read_setup_marker(), SetupMarker::Legacy);
        assert!(
            ctx.setup_should_show(),
            "legacy bare marker must show wizard once"
        );
        // And a skip recorded under an OLDER version re-offers on
        // the current version (the update-install path).
        std::fs::write(&marker, "skipped 2.0.1\n").expect("old skip marker");
        assert!(
            ctx.setup_should_show(),
            "older-version skip must re-offer wizard on update"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn f23_run_setup_again_reopens_wizard() {
        // A1 reopen, end to end at the callback level: the Settings
        // "Run setup" button fires `run-setup-again`, which must
        // clear the marker and open the wizard at step 0 — even
        // when the marker came from a v2.0.1 skip.
        let Ok(Ok(win)) = std::panic::catch_unwind(MainWindow::new) else {
            eprintln!("F23 window test skipped: no usable display/backend");
            return;
        };
        let (ctx, dir) = fc2_test_ctx();
        wire_callbacks(&win, &ctx);
        let marker = dir.join("setup-done");
        std::fs::write(&marker, "done\n").expect("legacy marker");
        assert!(
            ctx.setup_should_show(),
            "legacy marker must show wizard on next launch"
        );
        win.set_setup_mode(false);
        win.invoke_run_setup_again();
        assert!(win.get_setup_mode(), "reopen must open the wizard");
        assert_eq!(win.get_setup_step(), 0, "reopen starts at step 0");
        assert!(!marker.is_file(), "reopen must delete the marker");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // DS-20: the Settings "Check" button now really checks — the
    // tag parse + version verdict are pure, so they are tested
    // here; the network call itself (`fetch_app_update`) is not
    // exercised in tests (no live GitHub in the suite).
    #[test]
    fn ds20_parse_latest_release_tag() {
        // Real-shaped `releases/latest` payload.
        assert_eq!(
            parse_latest_release_tag(
                r#"{"tag_name":"v2.0.4","name":"RSH v2.0.4","draft":false,"prerelease":false}"#
            ),
            Some("2.0.4".to_string())
        );
        // A tag without the `v` prefix passes through as-is.
        assert_eq!(
            parse_latest_release_tag(r#"{"tag_name":"2.0.4"}"#),
            Some("2.0.4".to_string())
        );
        // Error bodies, missing tags, empty tags, garbage: no
        // usable tag — the caller reports "unreadable", never a
        // guessed version.
        assert_eq!(parse_latest_release_tag(r#"{"message":"Not Found"}"#), None);
        assert_eq!(parse_latest_release_tag(r#"{"name":"RSH"}"#), None);
        assert_eq!(parse_latest_release_tag(r#"{"tag_name":""}"#), None);
        assert_eq!(parse_latest_release_tag(r#"{"tag_name":"v"}"#), None);
        assert_eq!(parse_latest_release_tag("not json"), None);
        assert_eq!(parse_latest_release_tag(""), None);
    }

    #[test]
    fn ds20_compare_app_versions_is_numeric() {
        use std::cmp::Ordering;
        assert_eq!(
            compare_app_versions("2.0.4", "2.0.3"),
            Some(Ordering::Greater)
        );
        assert_eq!(
            compare_app_versions("2.0.3", "2.0.3"),
            Some(Ordering::Equal)
        );
        assert_eq!(compare_app_versions("2.0.2", "2.0.3"), Some(Ordering::Less));
        // Numeric, not lexicographic: 2.0.10 is newer than 2.0.9.
        assert_eq!(
            compare_app_versions("2.0.10", "2.0.9"),
            Some(Ordering::Greater)
        );
        // A leading `v` and a missing patch are tolerated.
        assert_eq!(
            compare_app_versions("v2.1", "2.0.9"),
            Some(Ordering::Greater)
        );
        // Garbage never compares — the caller reports unreadable.
        assert_eq!(compare_app_versions("garbage", "2.0.3"), None);
        assert_eq!(compare_app_versions("2.0.3", "garbage"), None);
    }

    #[test]
    fn ds20_app_update_verdict_matrix() {
        assert_eq!(
            app_update_verdict("2.0.3", "2.0.3"),
            Some(AppUpdateVerdict::UpToDate)
        );
        assert_eq!(
            app_update_verdict("2.0.3", "2.0.4"),
            Some(AppUpdateVerdict::Available("2.0.4".to_string()))
        );
        assert_eq!(
            app_update_verdict("2.0.3", "2.0.2"),
            Some(AppUpdateVerdict::CurrentIsNewer("2.0.2".to_string()))
        );
        assert_eq!(app_update_verdict("2.0.3", "garbage"), None);
    }

    // D17 (was DS-24): a failure shows in the status bar and NEVER
    // opens the drawer — the drawer opens only on bell click. The
    // error lives 8 s, then the 1 Hz expiry clears it. A success
    // shows the same way, with the ok-accent kind.
    #[test]
    fn d17_failure_status_never_opens_drawer() {
        let Ok(Ok(win)) = std::panic::catch_unwind(MainWindow::new) else {
            eprintln!("D17 window test skipped: no usable display/backend");
            return;
        };
        let (ctx, dir) = fc2_test_ctx();
        wire_callbacks(&win, &ctx);
        assert_eq!(win.get_status_text().as_str(), "");

        // A plain failure shows in place but leaves the drawer shut.
        let err = UserError {
            code: "d17_test".to_string(),
            title: "Test failure".to_string(),
            what_happened: "The test action failed on purpose.".to_string(),
            what_to_do: "Ignore this.".to_string(),
            detail: None,
        };
        ctx.notify_error(&win, "Test", &err);
        assert_eq!(win.get_status_title().as_str(), "Test failure");
        assert!(
            win.get_status_text().as_str().contains("failed on purpose"),
            "status shows the failure, got: {}",
            win.get_status_text()
        );
        assert_eq!(win.get_status_kind().as_str(), "error");
        assert!(!win.get_notif_open(), "failure must not open the drawer");

        // Not expired yet: the 1 Hz check leaves it alone.
        ctx.check_status_expiry(&win);
        assert_ne!(win.get_status_text().as_str(), "");

        // Past the 8 s deadline: the expiry clears it (and the kind
        // resets to error).
        ctx.status_until.set(Some(
            Instant::now()
                .checked_sub(Duration::from_secs(1))
                .expect("1 s ago exists"),
        ));
        ctx.check_status_expiry(&win);
        assert_eq!(win.get_status_text().as_str(), "");
        assert_eq!(win.get_status_title().as_str(), "");
        assert_eq!(win.get_status_kind().as_str(), "error");

        // A success shows the same way — ok-accent kind, drawer
        // still shut.
        ctx.notify_success(&win, "Test", "Test success", "The test action worked.");
        assert_eq!(win.get_status_title().as_str(), "Test success");
        assert_eq!(win.get_status_kind().as_str(), "success");
        assert!(!win.get_notif_open(), "success must not open the drawer");

        // B01: the status bar carries the FULL text — a 200-char
        // body is not cut at 120 chars any more (the bar elides
        // the one line visually; the drawer holds the same text).
        let long_happened = "x".repeat(200);
        let long_err = UserError {
            code: "d17_test_long".to_string(),
            title: "Long failure".to_string(),
            what_happened: long_happened.clone(),
            what_to_do: "Retry.".to_string(),
            detail: None,
        };
        ctx.notify_error(&win, "Test", &long_err);
        assert!(
            win.get_status_text().as_str().contains(&long_happened),
            "status carries the full 200-char body, got: {}",
            win.get_status_text()
        );

        // B01: a second message supersedes the first and the +N
        // counter counts it; clearing resets it.
        ctx.notify_success(&win, "Test", "Second", "Second message.");
        assert_eq!(win.get_status_more(), 1);
        ctx.clear_status(&win);
        assert_eq!(win.get_status_more(), 0);
        assert_eq!(win.get_status_text().as_str(), "");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // B02: TransportClosed never renders its sentence twice — the
    // hint is "RSH is reconnecting automatically." now, and
    // `notify_error` refuses to join a `what_to_do` that already
    // says what happened.
    #[test]
    fn b02_transport_closed_no_doubled_sentence() {
        let Ok(Ok(win)) = std::panic::catch_unwind(MainWindow::new) else {
            eprintln!("B02 window test skipped: no usable display/backend");
            return;
        };
        let (ctx, dir) = fc2_test_ctx();
        wire_callbacks(&win, &ctx);
        let err = UserError::from_connection(ConnectionError::TransportClosed, "test detail");
        assert_eq!(err.what_to_do, "RSH is reconnecting automatically.");
        ctx.notify_error(&win, "Test", &err);
        assert_eq!(
            win.get_status_text().as_str(),
            "The connection to the wheel dropped. RSH is reconnecting automatically."
        );
        // And the dedupe guard: a what_to_do that repeats the
        // happened text is not joined at all.
        let dup = UserError {
            code: "dup".to_string(),
            title: "Dup".to_string(),
            what_happened: "The wheel stopped.".to_string(),
            what_to_do: "the wheel stopped".to_string(),
            detail: None,
        };
        ctx.notify_error(&win, "Test", &dup);
        assert_eq!(win.get_status_text().as_str(), "The wheel stopped.");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // Bug 4/8: a failed backend-switch job reverts the Settings
    // radio to the backend that is actually live and toasts an
    // error — never a "Game controller is now …" success. A job
    // that reports success for the WRONG backend (live vJoy after a
    // HIDMaestro request — the old silent fallback) is the same
    // failure, not a success toast. A genuine success (live backend
    // == requested) still toasts success.
    #[test]
    fn backend_job_failure_reverts_radio_and_errors_without_success_toast() {
        let Ok(Ok(win)) = std::panic::catch_unwind(MainWindow::new) else {
            eprintln!("backend job test skipped: no usable display/backend");
            return;
        };
        let (ctx, dir) = fc2_test_ctx();
        wire_callbacks(&win, &ctx);
        // SimApi reports vJoy live; the click flipped the segment
        // to HIDMaestro.
        assert_eq!(ctx.api().output_backend(), "vJoy");
        win.set_controller(0);

        // 1. The switch errors (sidecar/UAC failure): radio back
        // on vJoy, error toast with the real reason, no success.
        let started = ctx.run_job(&win, JobKind::Backend, "Switching controller…", || {
            JobDone::Backend {
                index: 0,
                result: Err(UserError {
                    code: "Unknown".to_string(),
                    title: "Something went wrong".to_string(),
                    what_happened: "Something went wrong.".to_string(),
                    what_to_do: "Try again.".to_string(),
                    detail: Some(
                        "could not switch output backend to HIDMaestro: HIDMaestro sidecar failed to start."
                            .to_string(),
                    ),
                }),
            }
        });
        assert!(started, "backend job should start");
        drain_all_jobs(&ctx, &win);
        assert_eq!(
            win.get_controller(),
            1,
            "radio reverts to the live backend (vJoy)"
        );
        assert_eq!(win.get_status_kind().as_str(), "error");
        assert!(
            win.get_status_text()
                .as_str()
                .contains("sidecar failed to start"),
            "toast shows the real reason, got: {}",
            win.get_status_text()
        );
        assert!(
            !win.get_status_text().as_str().contains("is now"),
            "no success wording on a failed switch, got: {}",
            win.get_status_text()
        );

        // 2. The job "succeeds" for the WRONG backend (live vJoy
        // after a HIDMaestro request — the old silent fallback):
        // the same failure, not a success toast.
        win.set_controller(0);
        let started = ctx.run_job(&win, JobKind::Backend, "Switching controller…", || {
            JobDone::Backend {
                index: 0,
                result: Ok("vJoy".to_string()),
            }
        });
        assert!(started, "second backend job should start");
        drain_all_jobs(&ctx, &win);
        assert_eq!(
            win.get_controller(),
            1,
            "radio reverts to the live backend (vJoy) on a mismatch"
        );
        assert_eq!(win.get_status_kind().as_str(), "error");
        assert!(
            win.get_status_text().as_str().contains("is not active"),
            "mismatch toasts as a failure, got: {}",
            win.get_status_text()
        );
        assert!(
            !win.get_status_text()
                .as_str()
                .contains("Game controller is now"),
            "no success toast on a mismatched switch, got: {}",
            win.get_status_text()
        );

        // 3. A genuine success (live backend == requested) still
        // toasts success and keeps the segment.
        win.set_controller(0);
        let started = ctx.run_job(&win, JobKind::Backend, "Switching controller…", || {
            JobDone::Backend {
                index: 0,
                result: Ok("HIDMaestro".to_string()),
            }
        });
        assert!(started, "third backend job should start");
        drain_all_jobs(&ctx, &win);
        assert_eq!(win.get_controller(), 0, "segment keeps HIDMaestro");
        assert_eq!(win.get_status_kind().as_str(), "success");
        assert!(
            win.get_status_text()
                .as_str()
                .contains("Game controller is now HIDMaestro"),
            "genuine success still toasts, got: {}",
            win.get_status_text()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // D15: the wizard's Connect step unlocks on `board_reachable`
    // (HELLO-or-better), never on Active/pairing. The sim reports
    // a reachable board, so a live sim session unlocks Step 1;
    // with no live link the step stays locked.
    #[test]
    fn d15_step1_next_unlocks_on_board_reachable() {
        let Ok(Ok(win)) = std::panic::catch_unwind(MainWindow::new) else {
            eprintln!("D15 window test skipped: no usable display/backend");
            return;
        };
        let (ctx, dir) = fc2_test_ctx();
        wire_callbacks(&win, &ctx);
        // No live link yet — the step is locked.
        ctx.refresh(&win, 0);
        assert!(
            !win.get_setup_connect_done(),
            "connect-done must be false with no live link"
        );
        // A live link to a reachable board unlocks it.
        ctx.state.borrow_mut().live = true;
        ctx.refresh(&win, 0);
        assert!(
            win.get_setup_connect_done(),
            "connect-done must be true once the board is reachable"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // D19: the Wi-Fi password validates in Rust before any job
    // runs — 8–63 printable ASCII, both fields matching.
    #[test]
    fn d19_wifi_password_validation() {
        assert!(Ctx::wifi_password_error("short", "short").is_some());
        assert!(Ctx::wifi_password_error("abcdefgh", "abcdefgh").is_none());
        assert!(Ctx::wifi_password_error("abcdefgh", "abcdefgi").is_some());
        assert!(Ctx::wifi_password_error("abc defgh", "abc defgh").is_some());
        assert!(
            Ctx::wifi_password_error("a".repeat(64).as_str(), "a".repeat(64).as_str()).is_some()
        );
        assert!(
            Ctx::wifi_password_error("a".repeat(63).as_str(), "a".repeat(63).as_str()).is_none()
        );
    }

    // R3-UI-04: a panic must flush the buffered UI log — the
    // crash context survives instead of stranding the last ≤64
    // lines. The hook is installed here (process-global, chained
    // onto the test harness's hook); the panic is caught, so the
    // test process survives, and the file is read with NO explicit
    // flush — the hook must have done it.
    #[test]
    fn r3_ui04_panic_hook_flushes_buffered_log_lines() {
        install_ui_panic_hook();
        let dir = std::env::temp_dir().join(format!("rsh-ui-panic-hook-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        crate::ui_log::init_ui_file_log(Some(&dir));
        crate::ui_log::ui_file_log("line before the panic");
        let _ = std::panic::catch_unwind(|| panic!("R3-UI-04 test panic"));
        let logs = std::fs::read_dir(dir.join("logs")).expect("logs dir exists");
        let names: Vec<String> = logs
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 1, "one daily file: {names:?}");
        let text = std::fs::read_to_string(dir.join("logs").join(&names[0])).expect("read log");
        assert!(
            text.contains("line before the panic"),
            "buffered line must reach the file via the panic hook: {text:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
        // Point the log nowhere again so later tests do not write
        // into this test's (removed) dir.
        crate::ui_log::init_ui_file_log(None);
    }

    // R3-UI-02: the success message is cleared when the Wi-Fi
    // state moves on, and kept while it still describes the
    // current state. The sim reports `password_set = false`, so a
    // message recorded for `true` (a previous set) is stale and
    // must clear on re-sync, while one recorded for `false`
    // survives the same sync.
    #[test]
    fn r3_ui02_wifi_password_message_cleared_when_state_moves_on() {
        let Ok(Ok(win)) = std::panic::catch_unwind(MainWindow::new) else {
            eprintln!("R3-UI-02 window test skipped: no usable display/backend");
            return;
        };
        let (ctx, dir) = fc2_test_ctx();
        // A message from a previous set (`password_set` = true)
        // is stale against the sim's `false` — the sync clears it.
        win.set_wifi_password_message(
            "Wi-Fi password set — the wheel's hotspot now needs it.".into(),
        );
        ctx.wifi_password_message_for.set(Some(true));
        ctx.sync_wifi_password_state(&win);
        assert_eq!(
            win.get_wifi_password_message().as_str(),
            "",
            "stale success message must clear when the state moves on"
        );
        assert_eq!(ctx.wifi_password_message_for.get(), None);
        // The same message recorded for the current state
        // (`false`) survives the re-sync.
        win.set_wifi_password_message(
            "Wi-Fi password forgotten — the wheel's hotspot is open again.".into(),
        );
        ctx.wifi_password_message_for.set(Some(false));
        ctx.sync_wifi_password_state(&win);
        assert!(
            win.get_wifi_password_message()
                .starts_with("Wi-Fi password forgotten"),
            "message describing the current state must survive re-sync"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // R3-UI-02: the Set button shows "Setting…" while the job
    // runs and un-busies when the job resolves. The sim rejects
    // the set ("needs daemon"), so the error arm is what clears
    // it here — the success arm is the same single clear line.
    #[test]
    fn r3_ui02_wifi_password_busy_set_on_start_and_cleared_on_apply() {
        let Ok(Ok(win)) = std::panic::catch_unwind(MainWindow::new) else {
            eprintln!("R3-UI-02 window test skipped: no usable display/backend");
            return;
        };
        let (ctx, dir) = fc2_test_ctx();
        win.set_wifi_password_new("abcdefgh".into());
        win.set_wifi_password_confirm("abcdefgh".into());
        assert!(!win.get_wifi_password_busy(), "busy starts false");
        ctx.start_set_wifi_password_job(&win);
        assert!(
            win.get_wifi_password_busy(),
            "busy must be true while the set job runs"
        );
        // The job thread fails fast against the sim — drain until
        // the apply clears the flag (bounded wait, never infinite).
        for _ in 0..100 {
            ctx.drain_jobs(&win);
            if !win.get_wifi_password_busy() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            !win.get_wifi_password_busy(),
            "busy must clear when the job resolves"
        );
        assert!(
            !win.get_wifi_password_error().is_empty(),
            "sim set failure must surface as an inline error"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // R3-UI-05: live + foreground refreshes every tick (20 Hz);
    // live-but-unfocused/minimized drops to every 2nd tick
    // (10 Hz, the not-live rate); not-live stays every 2nd tick
    // whatever the focus. The Win32 query itself is
    // Windows-only and fails open (unknown window = foreground),
    // so this table is the whole throttle contract.
    #[test]
    fn r3_ui05_should_refresh_throttles_live_unfocused_to_10hz() {
        // Live + foreground: every tick.
        assert!(should_refresh(true, true, 0));
        assert!(should_refresh(true, true, 1));
        // Live + unfocused/minimized: every 2nd tick.
        assert!(should_refresh(true, false, 0));
        assert!(!should_refresh(true, false, 1));
        assert!(should_refresh(true, false, 2));
        // Not live: every 2nd tick regardless of focus.
        assert!(should_refresh(false, true, 0));
        assert!(!should_refresh(false, true, 1));
        assert!(should_refresh(false, false, 0));
        assert!(!should_refresh(false, false, 1));
    }

    // R3-UI-02: a timed-out SettingsOp drops its apply, so the
    // busy-clear in the apply never runs — `check_job_timeouts`
    // must clear the button itself, or it would stay "Setting…"
    // forever. A stale (11 s old) busy entry stands in for the
    // timed-out job; the 10 s SettingsOp bound makes it due.
    #[test]
    fn r3_ui02_wifi_password_busy_cleared_on_timeout() {
        let Ok(Ok(win)) = std::panic::catch_unwind(MainWindow::new) else {
            eprintln!("R3-UI-02 window test skipped: no usable display/backend");
            return;
        };
        let (ctx, dir) = fc2_test_ctx();
        let started = Instant::now()
            .checked_sub(Duration::from_secs(11))
            .expect("11 s ago exists");
        ctx.busy_jobs.borrow_mut().insert(
            JobKind::SettingsOp,
            ("Saving Wi-Fi password…".to_string(), started),
        );
        win.set_wifi_password_busy(true);
        ctx.check_job_timeouts(&win);
        assert!(
            !win.get_wifi_password_busy(),
            "busy must clear when the SettingsOp job times out"
        );
        assert!(
            !ctx.busy_jobs.borrow().contains_key(&JobKind::SettingsOp),
            "timed-out entry must be gone"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // D17: destructive actions confirm first — opening the confirm
    // dialog holds the pending action and shows the dialog; Yes
    // runs it and closes the dialog, No cancels it. Esc dismisses
    // the dialog (and the toast) but NEVER closes the wizard.
    #[test]
    fn d17_confirm_dialog_and_esc_never_closes_wizard() {
        let Ok(Ok(win)) = std::panic::catch_unwind(MainWindow::new) else {
            eprintln!("D17 window test skipped: no usable display/backend");
            return;
        };
        let (ctx, dir) = fc2_test_ctx();
        wire_callbacks(&win, &ctx);
        win.set_setup_mode(true);
        win.set_setup_step(0);

        // Open the confirm for a game removal — dialog shows, the
        // action is pending, the game is still there.
        ctx.open_confirm(
            &win,
            "Remove game",
            "Remove Test Game? You can add it again later.",
            "Remove",
            ConfirmAction::RemoveGame("game-1".to_string()),
        );
        assert!(win.get_confirm_open(), "confirm must open");
        assert_eq!(win.get_confirm_title().as_str(), "Remove game");
        assert!(ctx.confirm_pending.borrow().is_some());

        // Esc dismisses the dialog and clears the pending action —
        // the wizard stays open.
        win.invoke_keyboard_escape();
        assert!(!win.get_confirm_open(), "Esc must close the confirm");
        assert!(ctx.confirm_pending.borrow().is_none());
        assert!(win.get_setup_mode(), "Esc must NOT close the wizard");

        // Esc with a status showing dismisses it — the wizard
        // still stays open.
        ctx.set_toast(&win, "Test", "Test toast");
        assert_ne!(win.get_status_title().as_str(), "");
        win.invoke_keyboard_escape();
        assert_eq!(win.get_status_title().as_str(), "");
        assert!(win.get_setup_mode(), "Esc must NOT close the wizard");

        // Esc with nothing open does nothing to the wizard either.
        win.invoke_keyboard_escape();
        assert!(win.get_setup_mode(), "Esc must NOT close the wizard");

        // Yes runs the pending action and closes the dialog (the
        // sim has no daemon, so the removal refuses with a toast —
        // the point is the dialog closed and the action ran).
        ctx.open_confirm(
            &win,
            "Remove game",
            "Remove Test Game? You can add it again later.",
            "Remove",
            ConfirmAction::RemoveGame("game-1".to_string()),
        );
        win.invoke_confirm_yes();
        assert!(!win.get_confirm_open(), "Yes must close the confirm");
        assert!(ctx.confirm_pending.borrow().is_none());

        // No cancels without running anything.
        ctx.open_confirm(
            &win,
            "Delete profile",
            "Delete Test? This can't be undone.",
            "Delete",
            ConfirmAction::DeleteProfile(0),
        );
        win.invoke_confirm_no();
        assert!(!win.get_confirm_open(), "No must close the confirm");
        assert!(ctx.confirm_pending.borrow().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ds50_ds54_auto_theme_busy_kind() {
        let Ok(Ok(win)) = std::panic::catch_unwind(MainWindow::new) else {
            eprintln!("DS-50/54 window test skipped: no usable display/backend");
            return;
        };
        let (ctx, dir) = fc2_test_ctx();
        wire_callbacks(&win, &ctx);
        // DS-50: Theme Auto follows the cached OS preference.
        ctx.theme_choice.set(2);
        ctx.os_light.store(true, Ordering::Relaxed);
        win.set_theme(0);
        win.set_theme_choice(2);
        ctx.sync_auto_theme(&win);
        assert_eq!(win.get_theme(), 1);
        // R2-31: the resolved value never overwrites the stored
        // choice — Auto stays visibly selected in Settings.
        assert_eq!(win.get_theme_choice(), 2);
        ctx.os_light.store(false, Ordering::Relaxed);
        ctx.sync_auto_theme(&win);
        assert_eq!(win.get_theme(), 0);
        assert_eq!(win.get_theme_choice(), 2);
        // DS-54: the busy-kind prop carries the running job's kind.
        ctx.busy_jobs.borrow_mut().insert(
            JobKind::Backend,
            ("Switching controller…".to_string(), Instant::now()),
        );
        ctx.sync_busy(&win);
        assert!(win.get_busy());
        assert_eq!(win.get_busy_kind(), 2);
        assert_eq!(win.get_busy_label().as_str(), "Switching controller…");
        ctx.busy_jobs.borrow_mut().clear();
        ctx.sync_busy(&win);
        assert!(!win.get_busy());
        assert_eq!(win.get_busy_kind(), -1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// R2-19: step 0 gates on reachability, not `Active` — and shows
    /// why it cannot advance. With no live daemon the sim's `Active`
    /// must never unlock Next, and the hint must name the down
    /// daemon; with a live `Active` session the step unlocks.
    #[test]
    fn r2_19_step_zero_gate_reachability_and_hint() {
        let Ok(Ok(win)) = std::panic::catch_unwind(MainWindow::new) else {
            eprintln!("R2-19 window test skipped: no usable display/backend");
            return;
        };
        let (ctx, dir) = fc2_test_ctx();
        wire_callbacks(&win, &ctx);
        // SimApi reports Active — but it is not live, so the wizard
        // must stay locked and say why.
        ctx.push_frame(&win, 0);
        assert!(
            !win.get_setup_connect_done(),
            "the sim's Active must never unlock step 0"
        );
        assert!(
            win.get_setup_connect_hint()
                .as_str()
                .contains("RSH isn't running"),
            "step 0 must name the down daemon, got: {}",
            win.get_setup_connect_hint()
        );
        // A live Active session still unlocks (already-paired
        // resume path) and clears the hint.
        ctx.state.borrow_mut().live = true;
        ctx.push_frame(&win, 0);
        assert!(
            win.get_setup_connect_done(),
            "a live Active session must unlock step 0"
        );
        assert_eq!(
            win.get_setup_connect_hint().as_str(),
            "",
            "no hint once the board answers"
        );
        assert!(
            win.get_setup_wifi_joined(),
            "the joined label follows the same reachability"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ds51_r231_theme_persist_sets_both_props() {
        let Ok(Ok(win)) = std::panic::catch_unwind(MainWindow::new) else {
            eprintln!("DS-51/R2-31 window test skipped: no usable display/backend");
            return;
        };
        let (ctx, dir) = fc2_test_ctx();
        wire_callbacks(&win, &ctx);
        // Stored choice is Auto (2): the segment shows Auto while
        // the resolved theme follows the OS (dark here).
        ctx.theme_choice.set(2);
        ctx.os_light.store(false, Ordering::Relaxed);
        win.set_theme_choice(2);
        win.set_theme(0);
        // Change to Light via the Settings segment callback: both
        // props move, and the choice is stored.
        win.invoke_theme_persist(1);
        assert_eq!(ctx.theme_choice.get(), 1);
        assert_eq!(win.get_theme_choice(), 1, "segment shows the stored choice");
        assert_eq!(win.get_theme(), 1, "resolved theme follows the choice");
        // Change to Auto: the segment must show Auto (2) while the
        // resolved theme follows the OS (dark here) — the segment
        // must NOT be overwritten with the resolved value.
        win.invoke_theme_persist(2);
        assert_eq!(ctx.theme_choice.get(), 2);
        assert_eq!(win.get_theme_choice(), 2, "segment keeps showing Auto");
        assert_eq!(win.get_theme(), 0, "resolved theme follows the OS");
        // The choice survives a reload from disk.
        assert_eq!(read_theme(Some(&dir)), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
