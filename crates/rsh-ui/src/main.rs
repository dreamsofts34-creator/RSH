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
//! min/max mV + smoothing, the sliders supply deadzone/curve, and
//! Save persists the draft via [`AppApi::save_draft`].
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
//! Timing / event logs go to stderr as `RSH-UI ...` lines.
#![allow(
    missing_docs,
    reason = "slint::include_modules! generates undocumented getters/setters"
)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod curve;
mod ipc_api;
mod live_probe;
mod notifications;
mod sim_api;
mod ui_lifecycle;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rsh_api::connection::ConnectionState;
use rsh_api::pairing::countdown_ring_commands;
use rsh_api::{
    ApiTransport, AppApi, AppEvent, CalibrationInfo, ConnectionInfo, FirmwareCheck,
    FirmwareUpdateStatus, GameSummary, LedPattern, PairingInfo, ProfileSummary, ShifterGate,
    SnapshotInfo, TuningSettings, UserError,
};
use slint::{Color, ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel};

use crate::curve::{crosshair_commands, curve_commands, curve_output, dashed_line_commands};
use crate::ipc_api::IpcApi;
use crate::notifications::{sync_to_window, NotificationCenter};
use crate::sim_api::SimApi;

slint::include_modules!();

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

/// LED swatch palette (M2, `extras.slint` order): the six colours
/// the user can pick, as 0xRRGGBB. Index 3 is the historic accent
/// blue — the default when no colour is stored yet.
pub const LED_PALETTE: [u32; 6] = [
    0xFF_6B_6B, 0xF5_B8_4A, 0x3D_D6_8C, 0x2F_6B_FF, 0xB4_6C_FF, 0xFF_FF_FF,
];

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
    /// Centre deadzone, percent (0..10).
    deadzone_pct: f32,
    /// Response-curve exponent (1.0..2.0 — A24: the core contract
    /// caps the curve at 2.0, so the UI slider runs to 2.0 too).
    exponent: f32,
    /// 0 soft, 1 balanced, 2 sharp, 3 custom.
    preset: i32,
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
        let deadzone_pct = settings.deadzone.clamp(0.0, 10.0) as f32;
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
        }
    }

    /// Build the daemon record to push (L4): the cached stored
    /// record supplies min/max mV, smoothing and the extension
    /// fields; the sliders supply deadzone/curve. The core contract
    /// caps the curve at 2.0 (A24) — the slider range matches, so
    /// the clamp here is only a guard.
    fn to_settings(&self, base: &TuningSettings) -> TuningSettings {
        TuningSettings {
            module: "steering".to_string(),
            deadzone: f64::from(self.deadzone_pct),
            curve: f64::from(self.exponent).clamp(0.5, 2.0),
            ..base.clone()
        }
    }

    fn set_deadzone(&mut self, pct: f32) {
        self.deadzone_pct = pct.clamp(0.0, 10.0);
    }

    fn set_exponent(&mut self, exp: f32) {
        self.exponent = exp.clamp(1.0, 2.0);
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
    }
}

struct Args {
    page: i32,
    quit_after: Option<u64>,
    static_mode: bool,
    /// First-time setup wizard instead of the shell layout.
    setup: bool,
    /// Test-only override for the daemon data dir (IPC socket path).
    data_dir: Option<std::path::PathBuf>,
}

fn parse_args() -> Args {
    let mut a = Args {
        page: 0,
        quit_after: None,
        static_mode: false,
        setup: false,
        data_dir: None,
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
/// screen index: 0 Dark, 1 Light, 2 Auto).
fn write_theme(data_dir: Option<&PathBuf>, settings_idx: i32) {
    let Some(dir) = data_dir else {
        return;
    };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let text = match settings_idx {
        1 => "Light",
        2 => "Auto",
        _ => "Dark",
    };
    match std::fs::write(dir.join("ui-theme"), format!("{text}\n")) {
        Ok(()) => eprintln!("RSH-UI theme persisted: {text}"),
        Err(e) => eprintln!("RSH-UI theme persist failed: {e}"),
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
#[cfg(windows)]
fn system_light() -> bool {
    let Ok(out) = std::process::Command::new("reg")
        .args([
            "query",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
            "/v",
            "AppsUseLightTheme",
        ])
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
    /// LED brightness percent (0..100).
    Brightness,
}

/// A slider drag's latest value, waiting out its debounce (M-7):
/// the slider already moved on screen (the component owns the drag
/// state); only the daemon push waits 150 ms after the last move.
/// The value text / mini-curve lag the drag by at most that long.
#[derive(Clone, Copy)]
struct PendingField {
    module: &'static str,
    kind: FieldKind,
    value: f64,
}

/// Health-monitor verdicts (B2): the background thread in
/// [`spawn_health_monitor`] pings the live daemon every second and
/// re-probes after a death; the frame timer drains these on the UI
/// thread and applies them — the monitor never touches UI state.
enum HealthEvent {
    /// The live daemon stopped answering (or the first probe found
    /// none): show Disconnected, clear stale values.
    DaemonDown,
    /// A (new) live daemon answered: swap it in.
    DaemonBack(Arc<IpcApi>),
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
    /// Firmware update start (the flash itself then runs on the
    /// daemon and is polled separately).
    FirmwareUpdate,
    /// Raw-mV availability probe for the wizard Calibrate step.
    RawAvailable,
    /// Pairing reset.
    ResetPairing,
}

impl JobKind {
    /// How long the job may stay busy before the frame timer times
    /// it out with a visible error (B5: loading never infinite).
    /// Backend is long on purpose — the user may take a minute to
    /// answer the UAC prompt while the sidecar spawn waits.
    fn timeout(self) -> Duration {
        match self {
            JobKind::Reconnect => Duration::from_secs(15),
            JobKind::Backend => Duration::from_secs(80),
            JobKind::FirmwareUpdate => Duration::from_secs(10),
            JobKind::Transport
            | JobKind::FirmwareCheck
            | JobKind::RawAvailable
            | JobKind::ResetPairing => Duration::from_secs(5),
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
            JobKind::ResetPairing => 6,
        }
    }
}

/// Busy-strip state (B5, FC2) for a `busy_jobs` map: `(false, "")`
/// when nothing runs, otherwise `(true, label)` of the OLDEST
/// running job (the strip shows one job at a time — the one that
/// started first; ties break by [`JobKind::order`]).
fn busy_state_for(jobs: &HashMap<JobKind, (String, Instant)>) -> (bool, String) {
    let mut best: Option<(Instant, u8, &str)> = None;
    for (kind, (label, started)) in jobs {
        let candidate = (*started, kind.order(), label.as_str());
        let take = best.is_none_or(|(old_started, old_order, _)| {
            candidate.0 < old_started || (candidate.0 == old_started && candidate.1 < old_order)
        });
        if take {
            best = Some(candidate);
        }
    }
    best.map_or_else(
        || (false, String::new()),
        |(_, _, label)| (true, label.to_string()),
    )
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
fn set_extras_on_prop(win: &MainWindow, index: usize, value: bool) {
    match index {
        0 => win.set_extras_handbrake_on(value),
        1 => win.set_extras_shifter_on(value),
        2 => win.set_extras_horn_on(value),
        _ => win.set_extras_leds_on(value),
    }
}

/// Write the B5 busy props, only when they change (so the 1 Hz
/// safety call in the frame timer is free). A transition logs to
/// stderr — the Xvfb smoke run reads the busy on/off off the log.
fn set_busy_props(win: &MainWindow, busy: bool, label: &str) {
    if win.get_busy() == busy && win.get_busy_label().as_str() == label {
        return;
    }
    win.set_busy(busy);
    win.set_busy_label(label.into());
    if busy {
        eprintln!("RSH-UI busy: {label}");
    } else {
        eprintln!("RSH-UI busy: idle");
    }
}

/// Write the M-2 daemon-error prop, only when it changes.
fn set_daemon_error_prop(win: &MainWindow, text: &str) {
    if win.get_daemon_error().as_str() == text {
        return;
    }
    win.set_daemon_error(text.into());
    if text.is_empty() {
        eprintln!("RSH-UI daemon-error banner cleared");
    } else {
        eprintln!("RSH-UI daemon-error banner: {text}");
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
    /// Firmware update start finished (the flash then runs on the
    /// daemon; progress comes from `firmware_update_status` polls).
    FirmwareUpdate(Result<FirmwareUpdateStatus, UserError>),
    /// Raw-mV availability probe finished (any axis reported).
    RawAvailable(bool),
    /// Pairing reset finished: `was_armed` is whether the board had
    /// BOOT held (`KEY_RESET` armed) when the reset started — armed
    /// means the board rotated its key, unarmed means only the
    /// PC-side state was cleared and the user still has to hold BOOT.
    ResetPairing {
        was_armed: bool,
        result: Result<PairingInfo, UserError>,
    },
}

/// Shared frame context for callbacks: the swappable API state,
/// tuning state, the unsaved flag, the notification center, and the
/// resolved data dir (setup marker + theme file).
struct Ctx {
    state: Rc<RefCell<LiveState>>,
    tuning: Rc<RefCell<Tuning>>,
    /// Cached stored steering record (L4): supplies min/max mV,
    /// smoothing and extension fields for each slider push.
    steering: Rc<RefCell<TuningSettings>>,
    /// Cached pedal records + feel exponents (A1/A2).
    pedals: Rc<RefCell<PedalCache>>,
    /// Last good calibration read (A2/A12/A22): pedal mV, capture
    /// date, centre-set state. `None` = never read / never captured.
    calibration: Rc<RefCell<Option<CalibrationInfo>>>,
    /// True while a vJoy install started by the wizard is polling.
    vjoy_polling: Rc<Cell<bool>>,
    /// 12 wheel-button display actions (A3/A4) + the pending chip
    /// choice not yet saved (None = the tile's stored action).
    button_actions: Rc<RefCell<Vec<String>>>,
    pending_action: Rc<RefCell<Option<String>>>,
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
    /// Profile id waiting on the unsaved-changes prompt (A8).
    pending_profile_id: Rc<RefCell<Option<String>>>,
    /// Learn target: 0 none, 1 Buttons page, 2 horn (Extras).
    learn_target: Rc<Cell<i32>>,
    /// Horn test flash end (Extras Test lights the indicator).
    horn_test_until: Rc<Cell<Option<Instant>>>,
    /// Live chart histories (A20): steering deg + throttle 0..1.
    steer_hist: Rc<RefCell<VecDeque<f32>>>,
    pedal_hist: Rc<RefCell<VecDeque<f32>>>,
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
    /// When the wizard started waiting for a pairing window
    /// (B-1): `begin_pairing` returns immediately now, so the
    /// window may open seconds later. The 1 Hz tick must not expire
    /// a `seconds_left == 0` wait as a closed window — it gives the
    /// board 10 s from this instant, then gives up honestly.
    pairing_wait_since: Rc<RefCell<Option<Instant>>>,
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
    /// When the current `connecting` state started (A9): the frame
    /// timer clears a `connecting` older than 15 s with a message
    /// instead of leaving "Connecting…" forever.
    connecting_since: Rc<Cell<Option<Instant>>>,
    /// True while a Reconnect job is in flight (A9): a second click
    /// is ignored instead of queueing another ~3 s probe.
    reconnect_in_flight: Rc<Cell<bool>>,
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
    fn sync_capture_availability(&self, win: &MainWindow) {
        let available = self.capture_available();
        win.set_centre_available(available);
        win.set_pedal_set_disabled(!available);
        win.set_extras_calibration_disabled(!available);
        // H9: the wizard's Calibrate step gates on REAL raw mV, not
        // just on the daemon being live — `raw_available` is probed
        // by a background job when the step is entered (and on the
        // 5 s refresh while it is open).
        win.set_setup_calibration_available(self.raw_available.get());
        let reason = if available {
            String::new()
        } else {
            Self::capture_unavailable_reason().to_string()
        };
        win.set_centre_reason(reason.clone().into());
        let setup_reason = if self.raw_available.get() {
            String::new()
        } else if !available {
            Self::capture_unavailable_reason().to_string()
        } else if self.busy_jobs.borrow().contains_key(&JobKind::RawAvailable) {
            "Waiting for wheel data…".to_string()
        } else {
            "No wheel data yet — check the wheel is connected.".to_string()
        };
        win.set_setup_calibration_reason(setup_reason.into());
    }

    /// Capture the steering centre over the live daemon (v2.0.2 —
    /// the calls live on [`IpcApi`] now, not a second connection).
    /// Returns the captured mV, or the human reason — never a
    /// silent failure. Only while live: the sim never captures.
    fn capture_centre_ipc(&self) -> Result<u16, String> {
        if !self.is_live() {
            return Err(Self::capture_unavailable_reason().to_string());
        }
        self.api().capture_centre().map_err(|e| e.title.clone())
    }

    /// Capture one pedal endpoint over the live daemon (`endpoint`
    /// is `"min"` for the released point, `"max"` for fully
    /// pressed). Returns the captured mV, or the human reason.
    fn capture_pedal_ipc(&self, pedal: &str, endpoint: &str) -> Result<u16, String> {
        if !self.is_live() {
            return Err(Self::capture_unavailable_reason().to_string());
        }
        let api = self.api();
        let result = if endpoint == "min" {
            api.capture_pedal_min(pedal)
        } else {
            api.capture_pedal_max(pedal)
        };
        result.map_err(|e| e.title.clone())
    }

    /// Refresh the cached calibration read (`calibration()`):
    /// `Ok` stores it, `Err` (sim / never captured) clears it.
    /// Called at startup, on reconnect and on the 5 s refresh.
    fn refresh_calibration(&self) {
        *self.calibration.borrow_mut() = self.api().calibration().ok();
    }

    /// Short date ("2026-11-16") from an RFC 3339 capture timestamp.
    fn calibration_date(&self) -> Option<String> {
        self.calibration
            .borrow()
            .as_ref()
            .and_then(|c| c.calibrated_at.as_ref())
            .map(|s| s.chars().take(10).collect())
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
    fn push_frame(&self, win: &MainWindow, tick: u64) {
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

        win.set_connected(connected);
        win.set_conn_label(if connecting {
            // B4: the first probe is still running.
            "Connecting…".into()
        } else if live {
            conn.label.clone().into()
        } else {
            "Disconnected".into()
        });
        // The wizard's Connect step is done exactly when the session
        // is Active on the LIVE daemon (v2.0.1 hotfix): re-derived on
        // every frame, so a reconnect also advances the wizard. The
        // sim reports Active too — it must never advance the wizard.
        win.set_setup_connect_done(active);
        if active {
            // TR §3c: an already-paired board + Active session means
            // the Pair step is already complete when the wizard opens.
            win.set_setup_paired(true);
            win.set_paired(true);
        } else {
            win.set_paired(false);
        }
        win.set_setup_wifi_joined(active && conn.transport == ApiTransport::Wifi);
        win.set_setup_usb_available(active && conn.transport == ApiTransport::Usb);
        let transport_label = match conn.transport {
            rsh_api::ApiTransport::Wifi => "Wi-Fi",
            rsh_api::ApiTransport::Usb => "USB",
        };
        let sub = if connecting {
            "Starting RSH…".to_string()
        } else if live {
            if conn.address.is_empty() {
                transport_label.to_string()
            } else {
                format!("{transport_label} · {}", conn.address)
            }
        } else {
            "Not connected".to_string()
        };
        win.set_conn_sub(sub.into());
        win.set_send_to_game(api.output_enabled());
        win.set_steer_angle(steer_deg);
        // B1: no live daemon → "—", never a sine angle.
        win.set_steer_text(if live {
            format!("{steer_deg:+.0}°").into()
        } else {
            "—".into()
        });
        win.set_throttle((snap.throttle * 100.0) as f32);
        win.set_brake((snap.brake * 100.0) as f32);
        win.set_clutch((snap.clutch.unwrap_or(0.0) * 100.0) as f32);
        // H10/M-3: handbrake + gear come from the live snapshot now
        // (0 / "—" only without live data — the snapshot defaults).
        win.set_handbrake((snap.handbrake * 100.0) as f32);
        win.set_gear(if snap.gear == 0 {
            "—".into()
        } else {
            snap.gear.to_string().into()
        });
        win.set_input_pct(input_pct);
        win.set_output_pct(output_pct);
        win.set_marker_vx(mx);
        win.set_marker_vy(my);
        win.set_dashed_line(dashed_line_commands().into());
        win.set_crosshair(crosshair_commands(mx, my).into());
        win.set_curve_commands(curve_commands(deadzone, exponent).into());
        win.set_deadzone(deadzone / 10.0);
        win.set_deadzone_text(format!("{deadzone:.0}%").into());
        // A24: slider range is 1.0..2.0 now, so the normalised value
        // is exponent - 1.0 (was / 2.0 for the old 1.0..3.0 range).
        win.set_curve_val(exponent - 1.0);
        win.set_curve_text(format!("{exponent:.2}").into());
        win.set_preset(self.tuning.borrow().preset);
        win.set_unsaved(self.unsaved.get());
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
        win.set_pressed_index(pressed_index);
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
            win.set_throttle_marker_x(markers[0].0);
            win.set_throttle_marker_y(markers[0].1);
            win.set_brake_marker_x(markers[1].0);
            win.set_brake_marker_y(markers[1].1);
            win.set_clutch_marker_x(markers[2].0);
            win.set_clutch_marker_y(markers[2].1);
        }
        // Horn test flash (Extras): lights the indicator briefly.
        let horn_pressed = self
            .horn_test_until
            .get()
            .is_some_and(|until| Instant::now() < until);
        win.set_horn_pressed(horn_pressed);
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
            let mut pedal_hist = self.pedal_hist.borrow_mut();
            pedal_hist.push_back(snap.throttle as f32);
            while pedal_hist.len() > CHART_SAMPLES {
                pedal_hist.pop_front();
            }
            if tick.is_multiple_of(10) {
                win.set_diag_steer_chart(chart_commands(&steer_hist, -180.0, 180.0).into());
                win.set_diag_pedals_chart(chart_commands(&pedal_hist, 0.0, 1.0).into());
                win.set_diag_steer_readout(format!("{steer_deg:+.0}°").into());
                win.set_diag_pedals_readout(
                    format!(
                        "Throttle {:.0}% · Brake {:.0}%",
                        snap.throttle * 100.0,
                        snap.brake * 100.0
                    )
                    .into(),
                );
            }
        } else if tick.is_multiple_of(10) {
            win.set_diag_steer_chart("".into());
            win.set_diag_pedals_chart("".into());
            win.set_diag_steer_readout("—".into());
            win.set_diag_pedals_readout("—".into());
        }
    }

    fn refresh(&self, win: &MainWindow, tick: u64) {
        self.push_frame(win, tick);
        // v2.0.2: capture availability follows the live daemon every
        // frame (see `sync_capture_availability`).
        self.sync_capture_availability(win);
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
    fn flush_steering_push(&self, win: &MainWindow) {
        let Some(due) = self.steer_push_due.get() else {
            return;
        };
        if Instant::now() < due {
            return;
        }
        self.steer_push_due.set(None);
        let settings = {
            let tuning = self.tuning.borrow();
            let base = self.steering.borrow();
            tuning.to_settings(&base)
        };
        match self.api().set_tuning(&settings) {
            Ok(stored) => {
                *self.steering.borrow_mut() = stored;
                self.unsaved.set(true);
            }
            Err(e) => {
                self.notify_error(win, "Steering", &e);
                eprintln!("RSH-UI set_tuning failed: {} ({})", e.code, e.title);
            }
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

    /// Flush a due field push (frame timer, M-7): read the module's
    /// stored record, apply the queued field, push it. On success the
    /// page re-syncs from the daemon; on failure the error is shown
    /// and the page re-syncs anyway (the slider snaps back to the
    /// stored value instead of lying).
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn flush_field_push(&self, win: &MainWindow) {
        let Some((pending, due)) = *self.pending_field.borrow() else {
            return;
        };
        if Instant::now() < due {
            return;
        }
        *self.pending_field.borrow_mut() = None;
        let api = self.api();
        let mut record = api.get_tuning(pending.module);
        match pending.kind {
            FieldKind::Deadzone => record.deadzone = pending.value,
            FieldKind::Bite => record.bite_point = Some(pending.value),
            FieldKind::Brightness => record.led_brightness = Some(pending.value.round() as u8),
        }
        match api.set_tuning(&record) {
            Ok(_) => {
                self.unsaved.set(true);
            }
            Err(e) => {
                self.notify_error(win, "Tuning", &e);
                eprintln!(
                    "RSH-UI field push {} failed: {} ({})",
                    pending.module, e.code, e.title
                );
            }
        }
        match pending.module {
            "throttle" | "brake" | "clutch" => self.sync_pedals(win),
            _ => self.sync_extras(win),
        }
        self.refresh(win, 0);
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
        sync_to_window(&self.notifications.borrow(), win);
    }

    /// Show a typed error gracefully (B6): the title is the plain
    /// headline, the body is what happened + the next step — never a
    /// raw code or stack trace. The daemon maps untyped state errors
    /// to `Unknown` with the real message in `detail`, so a generic
    /// "Something went wrong." is replaced by that detail, and an
    /// "Unknown" title by a plain one.
    fn notify_error(&self, win: &MainWindow, kind: &str, err: &UserError) {
        let title = if err.title == "Unknown" {
            "Couldn't complete that"
        } else {
            err.title.as_str()
        };
        let happened = match (&err.detail, err.what_happened.as_str()) {
            (Some(detail), "Something went wrong.") => detail.clone(),
            _ => err.what_happened.clone(),
        };
        let body = if err.what_to_do.is_empty() {
            happened
        } else {
            format!("{happened} {}", err.what_to_do)
        };
        self.notify(win, kind, title, &body);
    }

    /// Current busy state (B5): whether any job runs + the oldest
    /// running job's label (the strip shows one job at a time).
    fn busy_state(&self) -> (bool, String) {
        busy_state_for(&self.busy_jobs.borrow())
    }

    /// Drive the B5 busy strip from `busy_jobs` (FC2): a running
    /// job shows the strip with its label, no job hides it. Fed on
    /// every job start/finish/timeout (see [`Ctx::run_job`],
    /// [`Ctx::drain_jobs`], [`Ctx::check_job_timeouts`]) plus a 1 Hz
    /// safety call from the frame timer — the write itself is
    /// change-detected, so the safety call costs nothing.
    fn sync_busy(&self, win: &MainWindow) {
        let (busy, label) = self.busy_state();
        set_busy_props(win, busy, &label);
    }

    /// Drive the M-2 daemon-error banner (FC2): the fatal reason
    /// from `daemon-status.json` while the daemon is down, empty
    /// (banner hidden) while it is live or no fatal file exists.
    /// Called on every daemon death / return (`mark_dead`,
    /// `reconnect_to_live`, Reconnect failure) plus a 1 Hz safety
    /// call, so a fatal file that lands without a health event
    /// still reaches the banner.
    fn sync_daemon_error(&self, win: &MainWindow) {
        let text = daemon_error_for(
            self.is_live(),
            live_probe::read_daemon_status(self.data_dir.as_deref()),
        );
        set_daemon_error_prop(win, &text);
    }

    /// Seed the keep-in-tray switch from the API (FC2): the close
    /// handler reads the same value, so the switch never disagrees
    /// with what X will do.
    fn sync_keep_in_tray(&self, win: &MainWindow) {
        win.set_keep_in_tray(self.api().close_to_tray());
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
            eprintln!("RSH-UI job {kind:?} already running — ignoring duplicate start");
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
                    eprintln!("RSH-UI job {kind:?} finished after the UI went away");
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
                        // M-2: the daemon's fatal file names the
                        // reason (IpcBindFailed / DaemonHung) — show
                        // it instead of the generic "not running".
                        if let Some((_code, detail)) =
                            live_probe::read_daemon_status(self.data_dir.as_deref())
                        {
                            self.notify(win, "Connection", "RSH stopped unexpectedly", &detail);
                        } else {
                            self.notify(win, "Connection", "Couldn't reconnect", &reason);
                        }
                        // FC2: a fatal file names the reason — show
                        // it in the Overview banner too, not just in
                        // the notification drawer.
                        self.sync_daemon_error(win);
                        self.refresh(win, 0);
                    }
                }
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
                        eprintln!("RSH-UI transport switch ok: {}", info.label);
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
                match result {
                    Ok(active) => {
                        self.sync_overview(win);
                        // The wizard's Drivers step mirrors the same
                        // backend choice (fallback flag + controller
                        // segment + any install error cleared).
                        win.set_controller(index);
                        if index == 0 {
                            win.set_setup_vjoy_fallback(true);
                            win.set_setup_vjoy_error("".into());
                        }
                        self.notify(
                            win,
                            "Game controller",
                            "Controller switched",
                            &format!("Game controller is now {active}."),
                        );
                    }
                    Err(e) => {
                        // A10: revert the switch the binding already
                        // flipped, and show the typed reason (a UAC
                        // decline says admin is needed, not "Unknown").
                        win.set_controller(1 - index);
                        self.notify_error(win, "Game controller", &e);
                    }
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
                            self.notify(
                                win,
                                "Firmware",
                                "Wheel firmware",
                                "Firmware version unavailable — reconnect to see it.",
                            );
                        } else if check.update_available {
                            let bundled = check.bundled_version.as_deref().unwrap_or("?");
                            self.notify(
                                win,
                                "Firmware",
                                "Firmware update available",
                                &format!(
                                    "Firmware {bundled} is available — the wheel runs {}.",
                                    check.board_version
                                ),
                            );
                        } else if let Some(bundled) = &check.bundled_version {
                            self.notify(
                                win,
                                "Firmware",
                                "Firmware up to date",
                                &format!(
                                    "Wheel firmware {} — up to date (bundled {bundled}).",
                                    check.board_version
                                ),
                            );
                        } else {
                            self.notify(
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
                            self.firmware_update_running.set(true);
                            self.notify(
                                win,
                                "Firmware",
                                "Firmware update started",
                                "Updating the wheel firmware — the wheel will restart during the update.",
                            );
                        } else {
                            self.notify(
                                win,
                                "Firmware",
                                "Firmware update",
                                "The update didn't start — try again.",
                            );
                        }
                    }
                    Err(e) => self.notify_error(win, "Firmware", &e),
                }
            }
            JobDone::RawAvailable(available) => {
                self.busy_jobs.borrow_mut().remove(&JobKind::RawAvailable);
                // H9: the wizard's Calibrate Next gates on this.
                self.raw_available.set(available);
                self.sync_capture_availability(win);
            }
            JobDone::ResetPairing { was_armed, result } => {
                if self
                    .busy_jobs
                    .borrow_mut()
                    .remove(&JobKind::ResetPairing)
                    .is_none()
                {
                    return;
                }
                match result {
                    Ok(_) => {
                        if was_armed {
                            self.notify(
                                win,
                                "Pairing",
                                "Pairing reset",
                                "Pairing reset — the wheel has a new key. Pair again now.",
                            );
                        } else {
                            self.notify(
                                win,
                                "Pairing",
                                "Pairing reset",
                                "This wheel is already paired — hold BOOT for 10 seconds to reset it, then pair again.",
                            );
                        }
                        // The reset cleared the session: the wizard's
                        // Pair step reflects the unpaired state.
                        win.set_setup_paired(false);
                        win.set_setup_reset_required(false);
                    }
                    Err(e) => self.notify_error(win, "Pairing", &e),
                }
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
            if kind == JobKind::Reconnect {
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
            eprintln!("RSH-UI job {kind:?} timed out ({label})");
            self.notify(
                win,
                "Loading timed out",
                &format!("{label} timed out"),
                "The operation took too long — try again.",
            );
            self.refresh(win, 0);
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
                self.notify(
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
                    self.notify(
                        win,
                        "Firmware",
                        "Firmware updated",
                        &format!(
                            "Wheel firmware {version} installed — the wheel restarted with it."
                        ),
                    );
                } else {
                    self.notify(
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
    fn start_reconnect_job(&self, win: &MainWindow) {
        if self.reconnect_in_flight.get() {
            return;
        }
        self.reconnect_in_flight.set(true);
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
        if self.api().connection().transport == transport && self.is_live() {
            // Already on this transport — the switch already shows
            // it; nothing to do.
            return;
        }
        self.notify(
            win,
            "Connection",
            "Switching connection…",
            "The wheel connection is switching — this takes a moment.",
        );
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
            self.notify(
                win,
                "Game controller",
                "Admin permission needed",
                "HIDMaestro needs admin permission — Windows will ask once.",
            );
        } else {
            self.notify(
                win,
                "Game controller",
                "Switching controller…",
                "Switching to vJoy — no permission needed.",
            );
        }
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
        self.notify(
            win,
            "Firmware",
            "Checking firmware…",
            "Reading the wheel and bundled firmware versions…",
        );
        let api = self.api();
        self.run_job(
            win,
            JobKind::FirmwareCheck,
            "Checking firmware…",
            move || JobDone::FirmwareCheck(api.firmware_check()),
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
            self.notify(
                win,
                "Firmware",
                "Firmware update already running",
                "The wheel firmware is already updating — wait for it to finish.",
            );
            return;
        }
        self.notify(
            win,
            "Firmware",
            "Starting firmware update…",
            "Validating the firmware image before the upload…",
        );
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

    /// Start a pairing reset job (B5): the daemon resets the pairing
    /// (key rotation when BOOT was held, PC-side clear otherwise) —
    /// never just a notification, and never an automatic re-pair.
    fn start_reset_pairing_job(&self, win: &MainWindow) {
        self.notify(
            win,
            "Pairing",
            "Resetting pairing…",
            "Clearing the pairing state…",
        );
        let api = self.api();
        self.run_job(
            win,
            JobKind::ResetPairing,
            "Resetting pairing…",
            move || {
                let was_armed = api.pairing_status().key_reset_armed;
                JobDone::ResetPairing {
                    was_armed,
                    result: api.reset_pairing(),
                }
            },
        );
    }

    /// Seed the Buttons page from the daemon's stored bindings (H-3 /
    /// B1 wiring): button 1–12 ← stored action, unbound ← the
    /// display default. Runs on (re)connect — at startup the sim has
    /// no bindings, so the defaults stand until the daemon answers.
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
        match self.api().set_board_address(address) {
            Ok(stored) => {
                self.notify(
                    win,
                    "Settings",
                    "Address saved",
                    &format!("Wheel address is now {stored} — it applies on the next reconnect."),
                );
                self.sync_overview(win);
            }
            Err(e) => {
                self.notify_error(win, "Settings", &e);
                self.sync_overview(win);
            }
        }
    }

    /// Apply the keep-in-tray switch (M-7, FC2): the
    /// `keep-tray-selected` callback carries the NEW state (the
    /// .slint already flipped the prop). Success confirms the
    /// authoritative value; failure reverts the prop to the old
    /// state and explains — the switch never lies about the real
    /// setting.
    fn apply_keep_tray(&self, win: &MainWindow, enabled: bool) {
        match self.api().set_close_to_tray(enabled) {
            Ok(new_state) => {
                win.set_keep_in_tray(new_state);
                self.notify(
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
                self.notify_error(win, "Settings", &e);
            }
        }
    }

    /// Open the setup wizard at the Pair step and start the real
    /// pairing flow (H2): the Overview "Pair now" banner and the
    /// Settings Pair button both land here — never a dead-end on
    /// Settings. The wizard's own Begin button calls
    /// `begin_pairing_flow` directly (it is already in the wizard).
    fn open_pair_wizard(&self, win: &MainWindow) {
        self.clear_setup_marker();
        self.learn_target.set(0);
        win.set_setup_step(2);
        win.set_setup_centre_done(false);
        win.set_setup_pedals_done(false);
        win.set_setup_paired(win.get_paired());
        win.set_setup_reset_required(self.api().pairing_status().replacement_required);
        win.set_setup_mode(true);
        self.sync_vjoy(win);
        begin_pairing_flow(win, self);
    }

    /// Persist the notification center after a mutation (M10).
    fn persist_notifications(&self) {
        self.notifications.borrow().save(self.data_dir.as_deref());
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
    fn mark_dead(&self, win: &MainWindow) {
        let old_api = {
            let mut state = self.state.borrow_mut();
            if !state.live && !state.connecting {
                return;
            }
            state.live = false;
            state.connecting = false;
            let old = state.live_api.take();
            if let Ok(mut link) = self.live_link.lock() {
                *link = None;
            }
            old
        };
        if let Some(api) = old_api {
            api.clear_cached_state();
        }
        self.steer_hist.borrow_mut().clear();
        self.pedal_hist.borrow_mut().clear();
        win.set_daemon_live(false);
        // FC2: the fatal reason (if any) reaches the Overview
        // banner now, not just the notification drawer.
        self.sync_daemon_error(win);
        self.connecting_since.set(None);
        // M-2: the daemon's fatal file names the reason
        // (IpcBindFailed / DaemonHung) — show it instead of the
        // generic "not running". The detail is the daemon's own plain
        // words; the code goes to the log only.
        match live_probe::read_daemon_status(self.data_dir.as_deref()) {
            Some((code, detail)) => {
                eprintln!("RSH-UI daemon fatal: {code}: {detail}");
                self.notify(win, "Connection", "RSH stopped unexpectedly", &detail);
            }
            None => {
                self.notify(
                    win,
                    "Connection",
                    "RSH not running",
                    "Live data unavailable — RSH isn't running.",
                );
            }
        }
        self.refresh(win, 0);
    }

    /// Swap a (re)connected daemon in (B2/H1): the shared path for
    /// the Reconnect button and the health monitor's re-probe —
    /// seed the sliders from the daemon, reconnect the session on
    /// the transport it was on, and re-sync every page. A duplicate
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
        let transport = self.last_transport();
        self.swap_to_live(live_api);
        win.set_daemon_live(true);
        // FC2: live again — the daemon-error banner clears.
        self.sync_daemon_error(win);
        // Seed the controller + autostart switches from the daemon
        // (the startup seeding cannot run — the daemon was not
        // there when the window opened).
        if self.api().output_backend() == "HIDMaestro" {
            win.set_controller(0);
        } else {
            win.set_controller(1);
        }
        win.set_autostart_enabled(self.api().autostart());
        // B1 wiring: bindings + knobs + auto-pause + module on/off +
        // the firmware check cache all seed from the daemon now that
        // it is live (at startup the sim has none of these).
        self.seed_live_state(win);
        self.notify(win, "Connection", "RSH connected", "Live data is back.");
        if let Err(e) = self.api().connect(transport) {
            self.notify(win, "Connection", "Reconnect failed", &e.title);
            eprintln!("RSH-UI reconnect failed: {} ({})", e.code, e.title);
        }
        // An already-open pairing window / Active session at
        // (re)connect seeds the wizard the same way startup used to
        // (TR §3c: Active session = already paired).
        {
            let ps = self.api().pairing_status();
            // H-5: the wizard's reset branch (board already paired
            // with another key) only shows when the daemon reports a
            // replacement is required.
            win.set_setup_reset_required(ps.replacement_required);
            if ps.seconds_left > 0 {
                win.set_setup_pairing_active(true);
                win.set_setup_pair_seconds_left(i32::try_from(ps.seconds_left).unwrap_or(120));
                win.set_setup_pair_total(i32::try_from(ps.seconds_left).unwrap_or(120));
                win.set_setup_pair_status(
                    "Pairing window open — hold the pair button on the wheel.".into(),
                );
                Ctx::sync_pair_ring(win);
            }
            if matches!(self.api().connection().state, ConnectionState::Active) {
                win.set_setup_paired(true);
            }
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

    /// Apply one pedal-tuning edit (deadzone / bite / feel curve) to
    /// the named module: read the stored record, patch it, push it.
    /// Success marks the profile unsaved and re-syncs the pedal
    /// page; failure shows an error notification (never silent).
    fn set_pedal_tuning(
        &self,
        win: &MainWindow,
        module: &str,
        update: impl FnOnce(&mut TuningSettings),
    ) {
        let api = self.api();
        let mut record = api.get_tuning(module);
        update(&mut record);
        match api.set_tuning(&record) {
            Ok(_) => {
                self.unsaved.set(true);
                self.sync_pedals(win);
                self.refresh(win, 0);
            }
            Err(e) => {
                self.notify(win, "Pedals", "Couldn't save pedal tuning", &e.title);
                eprintln!(
                    "RSH-UI pedal set_tuning {module} failed: {} ({})",
                    e.code, e.title
                );
            }
        }
    }

    /// Apply one tuning edit to a non-pedal module (handbrake / LED /
    /// shifter / horn): same push as [`Ctx::set_pedal_tuning`], then
    /// re-sync the Extras page.
    fn set_module_tuning(
        &self,
        win: &MainWindow,
        module: &str,
        update: impl FnOnce(&mut TuningSettings),
    ) {
        let api = self.api();
        let mut record = api.get_tuning(module);
        update(&mut record);
        match api.set_tuning(&record) {
            Ok(_) => {
                self.unsaved.set(true);
                self.sync_extras(win);
                self.refresh(win, 0);
            }
            Err(e) => {
                self.notify(win, "Extras", "Couldn't save setting", &e.title);
                eprintln!(
                    "RSH-UI extras set_tuning {module} failed: {} ({})",
                    e.code, e.title
                );
            }
        }
    }

    /// Refresh the Pedals page from the stored records (A1/A2):
    /// mV from min/max, deadzone slider = pct/10, feel from the
    /// stored curve, mini-curve paths from feel + deadzone, bite for
    /// the clutch. Called at startup, on swap-to-live and after
    /// every pedal edit — the page never shows literals.
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
            let record = api.get_tuning(module);
            cache.feel_exp[i] = record.curve as f32;
            cache.records[i] = record;
        }
        let (t, b, c) = (
            cache.records[0].clone(),
            cache.records[1].clone(),
            cache.records[2].clone(),
        );
        let feels = [
            PedalCache::feel_index(cache.feel_exp[0]),
            PedalCache::feel_index(cache.feel_exp[1]),
            PedalCache::feel_index(cache.feel_exp[2]),
        ];
        drop(cache);
        // A1: slider value = pct/10 (0..10% range), handler does v*10.
        win.set_throttle_up_mv(mv_text(cal_mv("accel", true).unwrap_or(t.min_mv)).into());
        win.set_throttle_down_mv(mv_text(cal_mv("accel", false).unwrap_or(t.max_mv)).into());
        win.set_throttle_feel(feels[0]);
        win.set_throttle_deadzone((t.deadzone / 10.0) as f32);
        win.set_throttle_deadzone_text(pct_text(t.deadzone).into());
        win.set_throttle_curve(
            pedal_curve_commands(t.deadzone as f32, FEEL_EXPONENTS[feels[0] as usize]).into(),
        );
        win.set_throttle_dashed("M 6 104 L 104 6".into());
        win.set_brake_up_mv(mv_text(cal_mv("brake", true).unwrap_or(b.min_mv)).into());
        win.set_brake_down_mv(mv_text(cal_mv("brake", false).unwrap_or(b.max_mv)).into());
        win.set_brake_feel(feels[1]);
        win.set_brake_deadzone((b.deadzone / 10.0) as f32);
        win.set_brake_deadzone_text(pct_text(b.deadzone).into());
        win.set_brake_curve(
            pedal_curve_commands(b.deadzone as f32, FEEL_EXPONENTS[feels[1] as usize]).into(),
        );
        win.set_brake_dashed("M 6 104 L 104 6".into());
        win.set_clutch_up_mv(mv_text(cal_mv("clutch", true).unwrap_or(c.min_mv)).into());
        win.set_clutch_down_mv(mv_text(cal_mv("clutch", false).unwrap_or(c.max_mv)).into());
        win.set_clutch_feel(feels[2]);
        win.set_clutch_deadzone((c.deadzone / 10.0) as f32);
        win.set_clutch_deadzone_text(pct_text(c.deadzone).into());
        let bite = c.bite_point.unwrap_or(0.0);
        win.set_clutch_bite(bite as f32);
        win.set_clutch_bite_text(pct_text(bite * 100.0).into());
        win.set_clutch_curve(
            pedal_curve_commands(c.deadzone as f32, FEEL_EXPONENTS[feels[2] as usize]).into(),
        );
        win.set_clutch_dashed("".into());
        // Set up/down capture the live endpoints (v2.0.2): enabled
        // while the daemon is live, disabled with a reason otherwise.
        win.set_pedal_set_disabled(!self.capture_available());
    }

    /// Refresh the Extras page from the stored records (A6):
    /// handbrake mV/deadzone/feel, shifter gate, horn button, LED
    /// pattern + brightness — all `set_tuning` fields. Controls with
    /// no backend field (sensitivity, LED colour, handbrake mode)
    /// stay disabled / explain themselves in the Slint.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn sync_extras(&self, win: &MainWindow) {
        let api = self.api();
        let hb = api.get_tuning("handbrake");
        let led = api.get_tuning("led");
        let shifter = api.get_tuning("shifter");
        let horn = api.get_tuning("horn");
        win.set_handbrake_up_mv(mv_text(hb.min_mv).into());
        win.set_handbrake_down_mv(mv_text(hb.max_mv).into());
        win.set_handbrake_feel(PedalCache::feel_index(hb.curve as f32));
        win.set_handbrake_deadzone((hb.deadzone / 10.0) as f32);
        win.set_handbrake_deadzone_text(pct_text(hb.deadzone).into());
        win.set_handbrake_curve(
            pedal_curve_commands(
                hb.deadzone as f32,
                FEEL_EXPONENTS[PedalCache::feel_index(hb.curve as f32) as usize],
            )
            .into(),
        );
        win.set_handbrake_dashed("M 6 104 L 104 6".into());
        win.set_handbrake_marker_x(6.0);
        win.set_handbrake_marker_y(104.0);
        // H8: the stored handbrake mode (Axis 0 / Button 1) — never
        // a hardcoded 0.
        win.set_handbrake_mode(i32::from(hb.handbrake_button_mode.unwrap_or(false)));
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
        win.set_extras_calibration_disabled(!self.capture_available());
        win.set_shifter_mode(match shifter.shifter_gate {
            Some(ShifterGate::HPattern) => 1,
            _ => 0,
        });
        win.set_horn_button(match horn.horn_button {
            Some(b) => format!("Button {b}").into(),
            None => "Not set".into(),
        });
        win.set_horn_learning(self.learn_target.get() == 2);
        let pattern = led.led_pattern.unwrap_or(LedPattern::Solid);
        win.set_led_pattern(match pattern {
            LedPattern::Off => 0,
            LedPattern::Solid => 1,
            LedPattern::Blink => 2,
            LedPattern::Chase => 3,
            LedPattern::Rainbow => 4,
        });
        let brightness = led.led_brightness.unwrap_or(80);
        win.set_led_brightness(f32::from(brightness) / 100.0);
        win.set_led_brightness_text(pct_text(f64::from(brightness)).into());
        // M2: the stored colour drives the swatch + the preview —
        // never always-blue. Unknown colours fall back to the accent
        // swatch (index 3).
        let color_index = led
            .led_color
            .and_then(|c| LED_PALETTE.iter().position(|p| *p == c))
            .unwrap_or(3);
        win.set_led_color(i32::try_from(color_index).unwrap_or(3));
        win.set_led_preview(ModelRc::new(Rc::new(VecModel::from(led_preview(
            pattern,
            brightness,
            led.led_color,
        )))));
    }

    /// Refresh the Overview live values (A11/A12/A14/E): firmware
    /// line from the API, delay/jitter/loss from `diag_stats()` ("—"
    /// until the daemon measures), calibration label from
    /// `calibration().calibrated_at`, backend name + readiness from
    /// `output_backend()`.
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
        win.set_setup_address(address.clone().into());
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
        win.set_firmware_line(if firmware == "Unknown" {
            "Firmware —".into()
        } else {
            format!("Firmware {firmware} · {address}{loop_suffix}").into()
        });
        win.set_delay_text(delay.into());
        win.set_jitter_text(jitter.into());
        win.set_lost_text(lost.into());
        // A12: the capture date, or "Not calibrated" (warn state).
        if let Some(date) = self.calibration_date() {
            win.set_calibrated_label(format!("Calibrated {date}").into());
            win.set_calibrated_warn(false);
        } else {
            win.set_calibrated_label("Not calibrated".into());
            win.set_calibrated_warn(true);
        }
        // A22: the "Set" pill only once a centre was captured.
        win.set_centre_set(self.calibration_date().is_some());
        let backend = api.output_backend();
        win.set_backend_name(if backend == "Unknown" {
            "Not ready".into()
        } else {
            backend.clone().into()
        });
        win.set_backend_ready(backend != "Unknown");
        // C3/M-8: Settings firmware text from the cached firmware
        // check (board live version vs bundled OTA image version) —
        // never the hardcoded "4.0.2". No cache yet (startup, before
        // the first check) falls back to the live version alone.
        win.set_firmware_text(match self.firmware_check_cache.borrow().as_ref() {
            Some(check) if check.board_version != "Unknown" => {
                if check.update_available {
                    format!(
                        "Version {} · update {} available",
                        check.board_version,
                        check.bundled_version.as_deref().unwrap_or("?")
                    )
                    .into()
                } else {
                    format!("Version {} · up to date", check.board_version).into()
                }
            }
            _ => {
                if firmware == "Unknown" {
                    "—".into()
                } else {
                    format!("Version {firmware}").into()
                }
            }
        });
        win.set_app_update_text("Auto-update is off — new versions are on GitHub Releases".into());
    }

    /// Refresh the profile lists (A8/C6): the popover model, the
    /// Settings list, the top-bar / Overview profile name and the
    /// auto-profile chip — all from `list_profiles()`, never
    /// hardcoded. Empty lists stay empty (no invented profiles).
    fn refresh_profiles(&self, win: &MainWindow) {
        let profiles = self.api().list_profiles();
        let active = profiles.iter().find(|p| p.active);
        win.set_profile_name(
            active
                .map_or_else(|| "No profile".to_string(), |p| p.name.clone())
                .into(),
        );
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
        win.set_auto_profile_text(auto.unwrap_or_else(|| "No profile yet".to_string()).into());
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
        let presets: Vec<ProfileItem> = preset_games
            .iter()
            .map(|g| ProfileItem {
                name: g.name.clone().into(),
                game: g.name.clone().into(),
                active: false,
            })
            .collect();
        win.set_popover_presets(ModelRc::new(Rc::new(VecModel::from(presets))));
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
        *self.profiles_cache.borrow_mut() = profiles;
    }

    /// Refresh the game tiles (A14) + the wizard Games step (A17)
    /// from `list_games()`. Tiles: running → Stop, installed →
    /// Launch, not installed → "Not installed" with NO action button
    /// (no fake Locate). Empty lists show the empty state, never
    /// invented games.
    fn refresh_games(&self, win: &MainWindow) {
        let games = self.api().list_games();
        let profiles = self.profiles_cache.borrow();
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
        drop(profiles);
        win.set_games(ModelRc::new(Rc::new(VecModel::from(items))));
        if games.is_empty() {
            win.set_setup_games_text("No games found yet.".into());
            win.set_setup_games_done(false);
        } else {
            let names: Vec<String> = games.iter().map(|g| g.name.clone()).collect();
            win.set_setup_games_text(
                format!(
                    "Found {} game{}: {}",
                    games.len(),
                    if games.len() == 1 { "" } else { "s" },
                    names.join(", ")
                )
                .into(),
            );
            win.set_setup_games_done(true);
        }
        *self.games_cache.borrow_mut() = games;
    }

    /// Sync the diagnostics drawer props (v2.0.1 hotfix): connection
    /// label, background-service liveness, app version, firmware
    /// version, live charts from the ring buffers, measured stats
    /// from `diag_stats()` ("—" + "No telemetry yet" while the
    /// daemon does not measure, A20/A21), plus status lines (and
    /// recent notifications) as the drawer's event log.
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
        win.set_diag_conn_label(conn.label.clone().into());
        win.set_diag_daemon_live(self.is_live());
        win.set_diag_app_version(version.clone().into());
        win.set_diag_firmware(firmware.clone().into());
        // A20: measured stats, or "—" (the drawer shows
        // "No telemetry yet" while these are "—").
        if let Ok(stats) = api.diag_stats() {
            {
                win.set_diag_delay_text(
                    stats
                        .e2e_ms
                        .map_or_else(|| "—".to_string(), |v| format!("{v:.0} ms"))
                        .into(),
                );
                win.set_diag_jitter_text(
                    stats
                        .jitter_ms_p99
                        .map_or_else(|| "—".to_string(), |v| format!("{v:.0} ms"))
                        .into(),
                );
                win.set_diag_lost_text(format!("{:.1}%", stats.loss_pct).into());
                win.set_diag_loop_rate_text(
                    stats
                        .loop_hz
                        .map_or_else(|| "—".to_string(), |hz| format!("{hz:.0} Hz"))
                        .into(),
                );
                win.set_diag_tx_failures_text(stats.tx_fail.to_string().into());
                win.set_diag_seq_gaps_text(stats.seq_gap.to_string().into());
            }
        } else {
            win.set_diag_delay_text("—".into());
            win.set_diag_jitter_text("—".into());
            win.set_diag_lost_text("—".into());
            win.set_diag_loop_rate_text("—".into());
            win.set_diag_tx_failures_text("—".into());
            win.set_diag_seq_gaps_text("—".into());
        }
        // H10: the live button grid shows the real pressed buttons
        // (bits 1–12 of the snapshot mask) — empty when not live, so
        // the section hides instead of rendering 12 dead tiles.
        let pressed: Vec<bool> = if self.is_live() {
            let mask = self.api().snapshot().button_mask;
            (0..12).map(|i| mask & (1_u64 << i) != 0).collect()
        } else {
            Vec::new()
        };
        win.set_diag_buttons_pressed(ModelRc::new(Rc::new(VecModel::from(pressed))));
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
        for n in self.notifications.borrow().to_model_items().iter().take(6) {
            lines.push(format!("{} {}", n.time, n.title).into());
        }
        win.set_diag_events(ModelRc::new(Rc::new(VecModel::from(lines))));
    }

    /// Sync the wizard Drivers step (vJoy) from `vjoy_status()`:
    /// installed / installing / version / last install error are all
    /// real. When the call errors (sim / no daemon) the status is
    /// honestly unknown — never a fake "installed" — and the
    /// `HIDMaestro` fallback stays the way forward. The user's
    /// fallback choice is preserved across calls.
    fn sync_vjoy(&self, win: &MainWindow) {
        let fallback = win.get_setup_vjoy_fallback();
        if let Ok(status) = self.api().vjoy_status() {
            win.set_setup_vjoy_installed(status.installed);
            win.set_setup_vjoy_installing(status.installing);
            win.set_setup_vjoy_version(status.version.unwrap_or_default().into());
            win.set_setup_vjoy_error(status.install_error.unwrap_or_default().into());
        } else {
            win.set_setup_vjoy_installed(false);
            win.set_setup_vjoy_installing(false);
            win.set_setup_vjoy_version("".into());
            win.set_setup_vjoy_error("Driver status unavailable — use HIDMaestro instead.".into());
        }
        win.set_setup_vjoy_fallback(fallback);
    }

    /// Drive the wizard countdown ring from seconds-left/total (A16):
    /// `countdown_ring_commands` draws the arc; empty when the
    /// window is closed (0 s left).
    fn sync_pair_ring(win: &MainWindow) {
        let left = win.get_setup_pair_seconds_left();
        let total = win.get_setup_pair_total();
        win.set_setup_ring_commands(
            countdown_ring_commands(
                u64::try_from(left.max(0)).unwrap_or(0),
                u64::try_from(total.max(0)).unwrap_or(0),
            )
            .into(),
        );
    }

    /// Path of the first-run marker (`<data_dir>/setup-done`).
    fn setup_marker_path(&self) -> Option<PathBuf> {
        self.data_dir.as_ref().map(|d| d.join("setup-done"))
    }

    /// True once setup finished or was skipped. No data dir means no
    /// marker can be read — TR §3 edge: that must NOT count as done,
    /// or the wizard could never show on a machine without a data dir.
    fn is_setup_done(&self) -> bool {
        let Some(path) = self.setup_marker_path() else {
            return false;
        };
        path.is_file()
    }

    /// Record that setup finished (or was skipped) so the wizard
    /// never shows again on this machine.
    fn mark_setup_done(&self) {
        let Some(path) = self.setup_marker_path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                return;
            }
        }
        match std::fs::write(&path, "done\n") {
            Ok(()) => eprintln!("RSH-UI setup marker written: {}", path.display()),
            Err(e) => eprintln!("RSH-UI setup marker write failed: {e}"),
        }
    }

    /// Delete the first-run marker so "Run setup again" (A18) shows
    /// the wizard from step 0.
    fn clear_setup_marker(&self) {
        if let Some(path) = self.setup_marker_path() {
            if let Err(e) = std::fs::remove_file(&path) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    eprintln!("RSH-UI setup marker delete failed: {e}");
                }
            }
        }
    }

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
    fn do_activate(&self, win: &MainWindow, p: &ProfileSummary) {
        match self.api().activate_profile(&p.id) {
            Ok(name) => {
                self.unsaved.set(false);
                self.refresh_profiles(win);
                self.refresh_games(win);
                win.set_popover_open(false);
                win.set_popover_show_prompt(false);
                *self.pending_profile_id.borrow_mut() = None;
                self.notify(
                    win,
                    "Profile",
                    "Profile active",
                    &format!("{name} is now active."),
                );
                self.refresh(win, 0);
            }
            Err(e) => {
                self.notify(win, "Profile", "Couldn't switch profile", &e.title);
                eprintln!("RSH-UI activate_profile failed: {} ({})", e.code, e.title);
            }
        }
    }

    /// Resolve the pending profile switch after the prompt.
    fn resolve_pending_switch(&self, win: &MainWindow, save_first: bool) {
        let pending = self.pending_profile_id.borrow().clone();
        let Some(id) = pending else {
            win.set_popover_show_prompt(false);
            return;
        };
        if save_first {
            if let Err(e) = self.api().save_draft() {
                self.notify(win, "Profile", "Couldn't save changes", &e.title);
                win.set_popover_show_prompt(false);
                return;
            }
            self.unsaved.set(false);
        } else if self.api().discard_draft() {
            self.unsaved.set(false);
        }
        win.set_popover_show_prompt(false);
        *self.pending_profile_id.borrow_mut() = None;
        if let Some(p) = self
            .profiles_cache
            .borrow()
            .iter()
            .find(|p| p.id == id)
            .cloned()
        {
            self.do_activate(win, &p);
        }
    }

    /// Duplicate the profile at `index` (A8).
    fn duplicate_profile_index(&self, win: &MainWindow, index: i32) {
        let Some(p) = self.profile_at(index) else {
            return;
        };
        match self.api().duplicate_profile(&p.id) {
            Ok(name) => {
                self.refresh_profiles(win);
                self.notify(
                    win,
                    "Profile",
                    "Profile duplicated",
                    &format!("Duplicated as {name}."),
                );
            }
            Err(e) => self.notify(win, "Profile", "Couldn't duplicate profile", &e.title),
        }
    }

    /// Delete the profile at `index` (A8).
    fn delete_profile_index(&self, win: &MainWindow, index: i32) {
        let Some(p) = self.profile_at(index) else {
            return;
        };
        match self.api().delete_profile(&p.id) {
            Ok(true) => {
                self.refresh_profiles(win);
                self.refresh_games(win);
                self.notify(
                    win,
                    "Profile",
                    "Profile deleted",
                    &format!("{} deleted.", p.name),
                );
            }
            Ok(false) => {
                self.notify(
                    win,
                    "Profile",
                    "Couldn't delete profile",
                    &format!("{} couldn't be deleted.", p.name),
                );
            }
            Err(e) => self.notify(win, "Profile", "Couldn't delete profile", &e.title),
        }
    }

    /// Create a new profile with a fresh unique name (A8).
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
        match self.api().create_profile(&name) {
            Ok(created) => {
                self.refresh_profiles(win);
                self.notify(
                    win,
                    "Profile",
                    "Profile created",
                    &format!("{created} created."),
                );
            }
            Err(e) => self.notify(win, "Profile", "Couldn't create profile", &e.title),
        }
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
        match self.api().create_profile(&name) {
            Ok(created) => {
                self.refresh_profiles(win);
                if let Some(p) = self
                    .profiles_cache
                    .borrow()
                    .iter()
                    .find(|p| p.name == created)
                    .cloned()
                {
                    self.do_activate(win, &p);
                }
                self.notify(
                    win,
                    "Profile",
                    "Profile created",
                    &format!("{created} created from the {} preset.", game.name),
                );
            }
            Err(e) => self.notify_error(win, "Profile", &e),
        }
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
        match self.api().rename_profile(&p.id, &game_name) {
            Ok(name) => {
                self.refresh_profiles(win);
                self.notify(
                    win,
                    "Profile",
                    "Profile renamed",
                    &format!("Renamed to {name}."),
                );
            }
            Err(e) => self.notify_error(win, "Profile", &e),
        }
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
        match self.api().rename_profile(&p.id, &name) {
            Ok(renamed) => {
                self.refresh_profiles(win);
                self.notify(
                    win,
                    "Profile",
                    "Profile renamed",
                    &format!("Renamed to {renamed}."),
                );
            }
            Err(e) => self.notify_error(win, "Profile", &e),
        }
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
                "No games found — install a racing game first.",
            );
            return;
        }
        let current = p.game.as_deref();
        let pos = current.and_then(|slug| games.iter().position(|g| g.id == slug));
        let next = games[pos.map_or(0, |i| i + 1) % games.len()].clone();
        match self.api().link_profile(&p.id, &next.id) {
            Ok(name) => {
                self.refresh_profiles(win);
                self.refresh_games(win);
                self.notify(
                    win,
                    "Profile",
                    "Profile linked",
                    &format!("{name} is now linked to {}.", next.name),
                );
            }
            Err(e) => self.notify_error(win, "Profile", &e),
        }
    }

    /// Poll a running learn (Buttons / horn): a picked button
    /// arrives via `take_learned_button()`; learning ending with no
    /// pick clears the listening state.
    fn poll_learn(&self, win: &MainWindow) {
        let target = self.learn_target.get();
        if target == 0 {
            return;
        }
        let api = self.api();
        if let Some(b) = api.take_learned_button() {
            self.learn_target.set(0);
            win.set_buttons_listening(false);
            win.set_horn_learning(false);
            if target == 2 {
                let mut record = api.get_tuning("horn");
                record.horn_button = Some(b);
                match api.set_tuning(&record) {
                    Ok(_) => {
                        self.unsaved.set(true);
                        self.sync_extras(win);
                        self.notify(
                            win,
                            "Horn",
                            "Horn learned",
                            &format!("Horn is now Button {b}."),
                        );
                    }
                    Err(e) => self.notify(win, "Horn", "Couldn't save horn button", &e.title),
                }
            } else {
                let idx = i32::from(b).saturating_sub(1);
                if (0..12).contains(&idx) {
                    self.selected_button.set(idx);
                    win.set_selected_index(idx);
                    let action =
                        self.button_actions.borrow()[usize::try_from(idx).unwrap_or(0)].clone();
                    *self.pending_action.borrow_mut() = None;
                    win.set_selected_action(action.into());
                }
                self.notify(
                    win,
                    "Buttons",
                    "Button learned",
                    &format!("Learned Button {b}."),
                );
            }
        } else if !api.is_learning() {
            self.learn_target.set(0);
            win.set_buttons_listening(false);
            win.set_horn_learning(false);
        }
    }

    /// Start (or cancel) learning for `target` (1 Buttons, 2 horn):
    /// `start_learn` arms the wheel, the frame timer picks the press
    /// up via [`Ctx::poll_learn`].
    fn toggle_learn(&self, win: &MainWindow, target: i32) {
        if self.learn_target.get() == target {
            match self.api().cancel_learn() {
                Ok(_) => {
                    self.learn_target.set(0);
                    win.set_buttons_listening(false);
                    win.set_horn_learning(false);
                    self.notify(win, "Learn", "Learning cancelled", "Learning cancelled.");
                }
                Err(e) => self.notify(win, "Learn", "Couldn't cancel learning", &e.title),
            }
            return;
        }
        match self.api().start_learn() {
            Ok(true) => {
                self.learn_target.set(target);
                win.set_buttons_listening(target == 1);
                win.set_horn_learning(target == 2);
                self.notify(
                    win,
                    "Learn",
                    "Listening…",
                    "Listening — press a button on the wheel.",
                );
            }
            Ok(false) => {
                self.notify(
                    win,
                    "Learn",
                    "Couldn't start learning",
                    "Learning didn't start.",
                );
            }
            Err(e) => self.notify(win, "Learn", "Couldn't start learning", &e.title),
        }
    }
}

/// Wire all Slint callbacks to the [`AppApi`] implementation.
fn wire_callbacks(win: &MainWindow, ctx: &Rc<Ctx>) {
    wire_tuning(win, ctx);
    wire_actions(win, ctx);
    wire_keyboard(win);
    wire_notifications(win, ctx);
    wire_setup(win, ctx);
    wire_pedals(win, ctx);
    wire_buttons(win, ctx);
    wire_extras(win, ctx);
    wire_theme(win, ctx);
    wire_popover(win, ctx);
    wire_transport(win, ctx);
    wire_fc2_callbacks(win, ctx);
}

/// Wire the FC2 callbacks (v2.0.2 Slint follow-up): the .slint side
/// already fires these — Rust had no handlers, so the clicks did
/// nothing. Each one lands on a real handler now:
/// - `pair-now-requested` → the Pair wizard (Overview banner).
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
        win.unwrap().on_pair_now_requested(move || {
            if let Some(w) = win.upgrade() {
                ctx.open_pair_wizard(&w);
            }
        });
    }
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

/// Run one pairing attempt from the wizard (Begin / Retry /
/// Reset): calls `begin_pairing`, drives the wizard's countdown +
/// status props, and notifies visibly on success and failure.
fn begin_pairing_flow(w: &MainWindow, ctx: &Ctx) {
    match ctx.api().begin_pairing() {
        Ok(info) => {
            w.set_setup_pair_error("".into());
            if info.seconds_left > 0 {
                w.set_setup_pairing_active(true);
                w.set_setup_pair_seconds_left(i32::try_from(info.seconds_left).unwrap_or(120));
                w.set_setup_pair_total(i32::try_from(info.seconds_left).unwrap_or(120));
                w.set_setup_pair_status(
                    "Pairing window open — hold the pair button on the wheel.".into(),
                );
                Ctx::sync_pair_ring(w);
                ctx.notify(
                    w,
                    "Pairing",
                    "Pairing window open",
                    &format!("Pairing window open — {}s left", info.seconds_left),
                );
            } else {
                // B-1: no window yet — the board may still open one
                // (begin_pairing returns immediately now). Start the
                // 10 s wait clock; the 1 Hz tick expires it, not the
                // first tick.
                *ctx.pairing_wait_since.borrow_mut() = Some(Instant::now());
                w.set_setup_pairing_active(true);
                w.set_setup_pair_status("Waiting for the wheel…".into());
                Ctx::sync_pair_ring(w);
                ctx.notify(
                    w,
                    "Pairing",
                    "Pairing started",
                    "Waiting for the wheel — hold the pair button for 3 seconds.",
                );
            }
            eprintln!("RSH-UI setup begin-pairing ok: {}s left", info.seconds_left);
        }
        Err(e) => {
            w.set_setup_pair_error(e.title.clone().into());
            ctx.notify(w, "Pairing", "Pairing failed", &e.title);
            eprintln!(
                "RSH-UI setup begin-pairing failed: {} ({})",
                e.code, e.title
            );
        }
    }
}

/// Connect the wizard to one transport and drive its props
/// (`setup-transport`; `setup-connect-done` follows from
/// `connection()` Active on the next frame via `push_frame`).
/// A9: the connect runs on a background job — the click returns
/// immediately; the wizard's Connect step is done when the session
/// goes Active (or the job drain notifies the failure).
fn setup_connect_to(w: &MainWindow, ctx: &Ctx, transport: ApiTransport) {
    w.set_setup_transport(i32::from(transport == ApiTransport::Usb));
    ctx.start_transport_job(w, transport, None);
}

/// Wire the setup-wizard callbacks (v2.0.1 hotfix).
///
/// Back/Next move through the wizard steps (0..=4); Next on the
/// last step and Skip leave setup, write the `setup-done` marker,
/// and show the main pages. Pairing / connect / calibrate / game
/// search are wired to the real `AppApi` now: `connect-done` comes
/// from `connection()` Active (set at startup + every frame),
/// `paired` from `PairingFinished` events, calibrate needs the
/// calibration backend (disabled + reason while it is missing —
/// never a fake done), games come from `list_games()`.
fn wire_setup(win: &MainWindow, ctx: &Rc<Ctx>) {
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_setup_back(move || {
            if let Some(w) = win.upgrade() {
                let next = (w.get_setup_step() - 1).max(0);
                w.set_setup_step(next);
                if next == 1 {
                    ctx.sync_vjoy(&w);
                }
                if next == 3 {
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
                if w.get_setup_step() >= 4 {
                    w.set_setup_mode(false);
                    ctx.mark_setup_done();
                    eprintln!("RSH-UI setup finished (wizard closed)");
                } else {
                    let next = w.get_setup_step() + 1;
                    w.set_setup_step(next);
                    if next == 1 {
                        ctx.sync_vjoy(&w);
                    }
                    if next == 3 {
                        // H9: entering Calibrate probes raw mV in the
                        // background — Next gates on real data, not
                        // just on the daemon being live.
                        ctx.start_raw_available_job(&w);
                    }
                    if next == 4 {
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
                ctx.mark_setup_done();
                eprintln!("RSH-UI setup skipped (wizard closed)");
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_setup_begin_pairing(move || {
            if let Some(w) = win.upgrade() {
                begin_pairing_flow(&w, &ctx);
            }
        });
    }
    wire_setup_pairing(win, ctx);
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
                match ctx.api().install_vjoy() {
                    Ok(_) => {
                        // Background install: the 1 Hz tick polls
                        // vjoy_status() until it lands or fails.
                        ctx.vjoy_polling.set(true);
                        w.set_setup_vjoy_installing(true);
                        w.set_setup_vjoy_error("".into());
                        ctx.notify(
                            &w,
                            "Drivers",
                            "vJoy install started",
                            "Installing vJoy in the background — this takes a minute.",
                        );
                    }
                    Err(e) => {
                        w.set_setup_vjoy_error(user_text(&e.title).into());
                        ctx.notify(&w, "Drivers", "vJoy install failed", &e.title);
                        eprintln!("RSH-UI install_vjoy failed: {} ({})", e.code, e.title);
                    }
                }
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

/// Wire the wizard's pairing / transport / calibrate / games
/// callbacks (split from [`wire_setup`] for readability).
fn wire_setup_pairing(win: &MainWindow, ctx: &Rc<Ctx>) {
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_setup_use_usb(move || {
            if let Some(w) = win.upgrade() {
                setup_connect_to(&w, &ctx, ApiTransport::Usb);
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_setup_retry_pairing(move || {
            if let Some(w) = win.upgrade() {
                begin_pairing_flow(&w, &ctx);
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_setup_reset_pairing(move || {
            if let Some(w) = win.upgrade() {
                // B5: the reset REALLY resets (daemon pairing reset +
                // board re-pair state cleared) — never just a
                // notification, and never an automatic re-pair.
                ctx.start_reset_pairing_job(&w);
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_setup_connect(move |t| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let transport = match t.as_str() {
                "usb" => ApiTransport::Usb,
                "wifi" => ApiTransport::Wifi,
                other => {
                    eprintln!("RSH-UI setup connect: unknown transport {other}");
                    return;
                }
            };
            setup_connect_to(&w, &ctx, transport);
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_setup_find_games(move || {
            if let Some(w) = win.upgrade() {
                ctx.refresh_games(&w);
                let count = ctx.games_cache.borrow().len();
                if count == 0 {
                    ctx.notify(
                        &w,
                        "Games",
                        "No games found",
                        "No games found — install a racing game and try again.",
                    );
                } else {
                    ctx.notify(
                        &w,
                        "Games",
                        "Games found",
                        &format!("Found {count} game{}.", if count == 1 { "" } else { "s" }),
                    );
                }
            }
        });
    }
}

/// Wire the wizard's Calibrate step (v2.0.2): "centre" captures the
/// live steering centre; "pedals" captures the fully-pressed
/// endpoint of accelerator + brake (the wizard says "press each
/// pedal fully, then set" — the clutch is optional and is set on
/// the Pedals page instead).
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
                "centre" => match ctx.capture_centre_ipc() {
                    Ok(mv) => {
                        w.set_setup_centre_done(true);
                        ctx.refresh_calibration();
                        ctx.notify(
                            &w,
                            "Calibration",
                            "Centre set",
                            &format!("Centre set at {mv} mV."),
                        );
                    }
                    Err(reason) => {
                        ctx.notify(&w, "Calibration", "Couldn't set centre", &reason);
                        eprintln!("RSH-UI setup calibrate centre failed: {reason}");
                    }
                },
                "pedals" => {
                    let mut captured: Vec<String> = Vec::new();
                    let mut failed: Option<String> = None;
                    for (key, name) in [("accel", "Accelerator"), ("brake", "Brake")] {
                        match ctx.capture_pedal_ipc(key, "max") {
                            Ok(mv) => captured.push(format!("{name} {mv} mV")),
                            Err(reason) => {
                                failed = Some(reason);
                                break;
                            }
                        }
                    }
                    match failed {
                        None => {
                            w.set_setup_pedals_done(true);
                            ctx.refresh_calibration();
                            ctx.notify(
                                &w,
                                "Calibration",
                                "Pedals set",
                                &format!("Pedals set: {}.", captured.join(", ")),
                            );
                        }
                        Some(reason) => {
                            ctx.notify(&w, "Calibration", "Couldn't set pedals", &reason);
                            eprintln!("RSH-UI setup calibrate pedals failed: {reason}");
                        }
                    }
                }
                other => eprintln!("RSH-UI setup calibrate: unknown action {other}"),
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
            if win.upgrade().is_none() {
                return;
            }
            let Ok(idx) = usize::try_from(p) else {
                return;
            };
            let Some(module) = MODULES.get(idx) else {
                return;
            };
            // A1: v is 0..1 of 10% → deadzone percent is v * 10.
            // M-7: a drag queues one debounced push, not one per move.
            let deadzone = (f64::from(v) * 10.0).clamp(0.0, 10.0);
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
    wire_pedals_set(win, ctx);
}

/// Wire the clutch bite-point and feel-preset callbacks (split
/// from [`wire_pedals`]; Set up/down live in [`wire_pedals_set`]).
fn wire_pedals_rest(win: &MainWindow, ctx: &Rc<Ctx>) {
    const MODULES: [&str; 3] = ["throttle", "brake", "clutch"];
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_pedal_bite_changed(move |v| {
            if win.upgrade().is_none() {
                return;
            }
            let bite = f64::from(v).clamp(0.0, 1.0);
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
                match ctx.capture_pedal_ipc(key, "min") {
                    Ok(mv) => {
                        ctx.refresh_calibration();
                        ctx.sync_pedals(&w);
                        ctx.notify(
                            &w,
                            "Pedals",
                            &format!("{name} released point set"),
                            &format!("{name} up (released) captured at {mv} mV."),
                        );
                    }
                    Err(reason) => {
                        ctx.notify(&w, "Pedals", &format!("Couldn't set {name} up"), &reason);
                        eprintln!("RSH-UI pedal set up {key} failed: {reason}");
                    }
                }
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
                match ctx.capture_pedal_ipc(key, "max") {
                    Ok(mv) => {
                        ctx.refresh_calibration();
                        ctx.sync_pedals(&w);
                        ctx.notify(
                            &w,
                            "Pedals",
                            &format!("{name} pressed point set"),
                            &format!("{name} down (pressed) captured at {mv} mV."),
                        );
                    }
                    Err(reason) => {
                        ctx.notify(&w, "Pedals", &format!("Couldn't set {name} down"), &reason);
                        eprintln!("RSH-UI pedal set down {key} failed: {reason}");
                    }
                }
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
            let idx = usize::try_from(i).unwrap_or(0);
            let action = ctx
                .button_actions
                .borrow()
                .get(idx)
                .cloned()
                .unwrap_or_else(|| "Not set".to_string());
            w.set_selected_index(i);
            w.set_selected_action(action.into());
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
            if ctx.edit_kind.get() == 1 {
                let knob = ctx.edit_knob.get();
                let row = ctx.knob_row.get();
                let display = ctx.knob_actions.borrow()[usize::try_from(knob).unwrap_or(0)]
                    [usize::try_from(row).unwrap_or(0)]
                .clone();
                let action = action_id(&display).unwrap_or("");
                let saved = ctx.api().set_knob_action(
                    u8::try_from(knob).unwrap_or(0),
                    u8::try_from(row).unwrap_or(0),
                    action,
                );
                match saved {
                    Ok(()) => {
                        ctx.unsaved
                            .set(ctx.api().draft_status().is_some_and(|s| s.dirty));
                        ctx.load_knobs(&w);
                        ctx.notify(
                            &w,
                            "Buttons",
                            "Knob mapping saved",
                            &format!("Knob {} → {display}.", knob + 1),
                        );
                    }
                    Err(e) => {
                        ctx.notify_error(&w, "Buttons", &e);
                        // Revert the staged choice to the stored one.
                        ctx.load_knobs(&w);
                    }
                }
                return;
            }
            // Button edit: persist the pending chip choice (H-3) —
            // "Not set" clears the binding, any other chip sets it.
            // The tile refreshes from the daemon immediately after.
            if let Some(action) = ctx.pending_action.borrow().clone() {
                let button = ctx.selected_button.get() + 1;
                let button_u8 = u8::try_from(button).unwrap_or(0);
                let saved = match action_id(&action) {
                    Some(id) => ctx.api().set_binding(button_u8, id).map(|_| ()),
                    None => ctx.api().clear_binding(button_u8).map(|_| ()),
                };
                match saved {
                    Ok(()) => {
                        *ctx.pending_action.borrow_mut() = None;
                        ctx.unsaved
                            .set(ctx.api().draft_status().is_some_and(|s| s.dirty));
                        ctx.load_bindings(&w);
                        ctx.notify(
                            &w,
                            "Buttons",
                            "Button mapping saved",
                            &format!("Button {button} → {action}."),
                        );
                    }
                    Err(e) => {
                        ctx.notify_error(&w, "Buttons", &e);
                        // Revert the tile to the stored action.
                        ctx.load_bindings(&w);
                    }
                }
                return;
            }
            match ctx.api().save_draft() {
                Ok(true) => {
                    ctx.unsaved.set(false);
                    ctx.notify(&w, "Buttons", "Changes saved", "Button changes saved.");
                    ctx.refresh(&w, 0);
                }
                Ok(false) => {
                    ctx.notify(&w, "Buttons", "Nothing to save", "No changes to save.");
                }
                Err(e) => {
                    ctx.notify_error(&w, "Buttons", &e);
                    eprintln!("RSH-UI buttons edit-save failed: {} ({})", e.code, e.title);
                }
            }
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
            let idx = usize::try_from(ctx.selected_button.get()).unwrap_or(0);
            let action = ctx
                .button_actions
                .borrow()
                .get(idx)
                .cloned()
                .unwrap_or_else(|| "Not set".to_string());
            w.set_selected_action(action.into());
            if ctx.api().discard_draft() {
                ctx.unsaved.set(false);
                ctx.notify(&w, "Buttons", "Changes discarded", "Changes discarded.");
                ctx.refresh(&w, 0);
            } else {
                ctx.notify(
                    &w,
                    "Buttons",
                    "Nothing to discard",
                    "No changes to discard.",
                );
            }
        });
    }
}

/// Wire the Extras page callbacks (A6): LED pattern/brightness and
/// handbrake feel/deadzone push through `set_tuning`; horn Learn
/// arms `start_learn`; Test flashes the indicator; the shifter link
/// opens the Buttons page. Handbrake Set up/down capture the live
/// endpoints (v2.0.2). Controls with no backend (handbrake mode,
/// sensitivity, LED colour) explain themselves instead of
/// pretending to work.
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
                    match ctx.api().set_module_enabled(module, new_state) {
                        Ok(enabled) => {
                            ctx.module_states.borrow_mut()[index] = enabled;
                            // FC2: confirm the prop with the
                            // authoritative value (`sync_extras` in
                            // refresh re-seeds it too).
                            set_extras_on_prop(&w, index, enabled);
                            ctx.refresh(&w, 0);
                        }
                        Err(e) => {
                            // FC2: the .slint already flipped the
                            // prop — flip it back so the switch
                            // never lies about the real module state.
                            set_extras_on_prop(&w, index, old_state);
                            ctx.notify_error(&w, "Extras", &e);
                        }
                    }
                }
                "handbrake-up" | "handbrake-down" => {
                    // v2.0.2: capture the handbrake endpoint from
                    // the live raw reading (up = released, down =
                    // fully pressed).
                    let endpoint = if a.as_str() == "handbrake-up" {
                        "min"
                    } else {
                        "max"
                    };
                    match ctx.capture_pedal_ipc("handbrake", endpoint) {
                        Ok(mv) => {
                            ctx.refresh_calibration();
                            ctx.sync_extras(&w);
                            ctx.notify(
                                &w,
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
                        Err(reason) => {
                            ctx.notify(&w, "Extras", "Couldn't set handbrake", &reason);
                            eprintln!("RSH-UI extras handbrake capture failed: {reason}");
                        }
                    }
                }
                "horn-learn" => ctx.toggle_learn(&w, 2),
                "horn-test" => {
                    ctx.horn_test_until
                        .set(Some(Instant::now() + Duration::from_millis(400)));
                    ctx.refresh(&w, 0);
                }
                "shifter-change-in-buttons" => w.set_page(3),
                "shifter-sensitivity" => {
                    ctx.notify(
                        &w,
                        "Extras",
                        "Shifter sensitivity",
                        "Sensitivity has no wheel setting yet.",
                    );
                }
                other => eprintln!("RSH-UI unknown extras-action: {other}"),
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
            if win.upgrade().is_none() {
                return;
            }
            let deadzone = (f64::from(v) * 10.0).clamp(0.0, 10.0);
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
            if win.upgrade().is_none() {
                return;
            }
            let brightness = (v.clamp(0.0, 1.0) * 100.0).round();
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
}

/// Wire the theme-persist callback (v2.0.1 hotfix, A8 v2.0.2): the
/// Settings screen reports its theme index (0 Dark, 1 Light,
/// 2 Auto) and Rust persists it AND resolves it to the window theme
/// (0 Dark / 1 Light — Auto follows the OS preference, so the
/// Settings segment then shows the resolved value, not "Auto").
fn wire_theme(win: &MainWindow, ctx: &Rc<Ctx>) {
    let win = win.as_weak();
    let ctx = ctx.clone();
    win.unwrap().on_theme_persist(move |i| {
        write_theme(ctx.data_dir.as_ref(), i);
        if let Some(w) = win.upgrade() {
            w.set_theme(resolve_theme_mode(i, system_light()));
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
                w.set_notif_open(!w.get_notif_open());
            }
        });
    }
    {
        let win = win.as_weak();
        win.unwrap().on_notif_close(move || {
            if let Some(w) = win.upgrade() {
                w.set_notif_open(false);
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
}

/// Wire the tuning callbacks (deadzone / curve / preset / reset / save).
/// Tuning changes mark the profile unsaved (REDESIGN-PLAN §3.6).
/// A24: the curve slider is 1.0..2.0 (exponent = 1.0 + v * 1.0).
fn wire_tuning(win: &MainWindow, ctx: &Rc<Ctx>) {
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_deadzone_changed(move |v| {
            ctx.tuning.borrow_mut().set_deadzone(v * 10.0);
            ctx.tuning.borrow_mut().set_preset(3);
            ctx.push_steering();
            if let Some(w) = win.upgrade() {
                ctx.refresh(&w, 0);
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_curve_changed(move |v| {
            ctx.tuning.borrow_mut().set_exponent(1.0 + v * 1.0);
            ctx.tuning.borrow_mut().set_preset(3);
            ctx.push_steering();
            if let Some(w) = win.upgrade() {
                ctx.refresh(&w, 0);
            }
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
            if let Some(w) = win.upgrade() {
                ctx.refresh(&w, 0);
            }
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
            if let Some(w) = win.upgrade() {
                ctx.refresh(&w, 0);
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_preset_selected(move |p| {
            ctx.tuning.borrow_mut().set_preset(p);
            ctx.push_steering();
            if let Some(w) = win.upgrade() {
                ctx.refresh(&w, 0);
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_reset_defaults(move || {
            ctx.tuning.borrow_mut().reset_defaults();
            ctx.push_steering();
            if let Some(w) = win.upgrade() {
                ctx.refresh(&w, 0);
            }
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
/// - `keyboard-escape`: closes the profile popover first, then the
///   notifications drawer, then the diagnostics drawer (v2.0.1 hotfix).
/// - `focus-keyboard`: focuses the root keyboard scope once at startup so
///   keys work before the user Tabs anywhere.
fn wire_keyboard(win: &MainWindow) {
    {
        let win = win.as_weak();
        win.unwrap().on_keyboard_save(move || {
            if let Some(w) = win.upgrade() {
                let page = w.get_page();
                if (1..=4).contains(&page) {
                    w.invoke_save_profile();
                } else {
                    eprintln!("RSH-UI keyboard-save ignored on page {page} (no Save there)");
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
                eprintln!("RSH-UI keyboard-nav page={next}");
            }
        });
    }
    {
        let win = win.as_weak();
        win.unwrap().on_keyboard_escape(move || {
            if let Some(w) = win.upgrade() {
                if w.get_popover_open() {
                    w.set_popover_open(false);
                    eprintln!("RSH-UI keyboard-escape closed profile popover");
                } else if w.get_notif_open() {
                    w.set_notif_open(false);
                    eprintln!("RSH-UI keyboard-escape closed notifications drawer");
                } else if w.get_diag_open() {
                    w.set_diag_open(false);
                    eprintln!("RSH-UI keyboard-escape closed diagnostics drawer");
                } else {
                    eprintln!("RSH-UI keyboard-escape (no drawer or popover open)");
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
fn wire_action_notifications(win: &MainWindow, ctx: &Rc<Ctx>) {
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_set_centre(move || {
            if let Some(w) = win.upgrade() {
                // v2.0.2: capture the live centre over IPC.
                match ctx.capture_centre_ipc() {
                    Ok(mv) => {
                        ctx.refresh_calibration();
                        ctx.refresh(&w, 0);
                        ctx.notify(
                            &w,
                            "Steering",
                            "Centre set",
                            &format!("Centre set at {mv} mV."),
                        );
                    }
                    Err(reason) => {
                        ctx.notify(&w, "Steering", "Couldn't set centre", &reason);
                        eprintln!("RSH-UI set centre failed: {reason}");
                    }
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
            let api = ctx.api();
            let result = if game.running {
                api.stop_game(&game.id)
            } else {
                api.launch_game(&game.id)
            };
            match result {
                Ok(updated) => {
                    ctx.refresh_games(&w);
                    let body = if updated.running {
                        format!("{} launched.", updated.name)
                    } else {
                        format!("{} stopped.", updated.name)
                    };
                    ctx.notify(
                        &w,
                        "Game",
                        if updated.running {
                            "Game launched"
                        } else {
                            "Game stopped"
                        },
                        &body,
                    );
                }
                Err(e) => {
                    ctx.notify(&w, "Game", "Couldn't launch game", &e.title);
                    eprintln!("RSH-UI launch-game failed: {} ({})", e.code, e.title);
                }
            }
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
                } else {
                    ctx.refresh_profiles(&w);
                    w.set_popover_open(true);
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
                    ctx.delete_profile_index(&w, my);
                } else {
                    ctx.notify(
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
            match ctx.api().set_autostart(enabled) {
                Ok(new_state) => {
                    w.set_autostart_enabled(new_state);
                    ctx.notify(
                        &w,
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
                    ctx.notify(&w, "Startup", "Autostart failed", &e.title);
                    eprintln!("RSH-UI set_autostart failed: {} ({})", e.code, e.title);
                }
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_run_setup_again(move || {
            if let Some(w) = win.upgrade() {
                // A18: delete the marker and open the wizard at step 0.
                ctx.clear_setup_marker();
                ctx.learn_target.set(0);
                w.set_setup_step(0);
                w.set_setup_centre_done(false);
                w.set_setup_pedals_done(false);
                w.set_setup_paired(w.get_paired());
                // H-5: sync the reset branch on wizard (re)open too.
                w.set_setup_reset_required(ctx.api().pairing_status().replacement_required);
                w.set_setup_mode(true);
                ctx.sync_vjoy(&w);
                eprintln!("RSH-UI run setup again (wizard reopened)");
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

/// Run one pairing attempt from Settings (H2): open the wizard at
/// the Pair step and start the real pairing flow — never a dead-end
/// notification on Settings.
fn settings_pair(w: &MainWindow, ctx: &Ctx) {
    ctx.open_pair_wizard(w);
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
            let api = ctx.api();
            match a.as_str() {
                "pair" => settings_pair(&w, &ctx),
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
                "update-check" => ctx.notify(
                    &w,
                    "Update",
                    "Auto-update is off",
                    "Auto-update is off — new versions are on GitHub Releases",
                ),
                "backup" => match api.backup_now() {
                    Ok(path) => {
                        // C3: the backup line updates after a backup.
                        w.set_backup_text("Saved just now".into());
                        ctx.notify(
                            &w,
                            "Backup",
                            "Backup saved",
                            &format!("Backup saved to {path}"),
                        );
                    }
                    Err(e) => ctx.notify_error(&w, "Backup", &e),
                },
                "restore" => match api.restore_backup() {
                    Ok(message) => {
                        ctx.notify(&w, "Backup", "Backup restored", &message);
                        ctx.refresh_profiles(&w);
                        ctx.refresh(&w, 0);
                    }
                    Err(e) => ctx.notify_error(&w, "Backup", &e),
                },
                other => eprintln!("RSH-UI unknown settings-action: {other}"),
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
            match ctx.api().save_draft() {
                Ok(true) => {
                    ctx.unsaved.set(false);
                    ctx.notify(&w, "Profile", "Changes saved", "Changes saved.");
                }
                Ok(false) => {
                    ctx.notify(&w, "Profile", "Nothing to save", "No changes to save.");
                }
                Err(e) => {
                    ctx.notify_error(&w, "Profile", &e);
                    eprintln!("RSH-UI save-profile failed: {} ({})", e.code, e.title);
                }
            }
            ctx.refresh(&w, 0);
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
            // output path). The old code already called it; the bug
            // was only the auto-pause toggle next to it.
            let next = !ctx.api().output_enabled();
            ctx.api().set_output_enabled(next);
            eprintln!("RSH-UI send-to-game={next}");
            if let Some(w) = win.upgrade() {
                w.set_send_to_game(next);
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_auto_pause_toggled(move || {
            if let Some(w) = win.upgrade() {
                // M6: auto-pause persists in settings and applies
                // (the daemon stops auto-switching while paused) —
                // never a local-only prop flip.
                let next = !w.get_auto_paused();
                match ctx.api().set_auto_pause(next) {
                    Ok(stored) => {
                        w.set_auto_paused(stored);
                        ctx.notify(
                            &w,
                            "Profile",
                            "Auto-switch",
                            if stored {
                                "Auto-switch paused — the active profile stays put."
                            } else {
                                "Auto-switch resumed."
                            },
                        );
                        eprintln!("RSH-UI auto-profile paused={stored}");
                    }
                    Err(e) => {
                        w.set_auto_paused(!next);
                        ctx.notify_error(&w, "Profile", &e);
                    }
                }
            }
        });
    }
}

/// Log one drained push event. Notification events are ALSO fed to
/// the [`NotificationCenter`] by the timer drain (see `main`); this
/// function only logs, it never stores.
fn log_event(ev: &AppEvent) {
    match ev {
        AppEvent::ConnectionChanged {
            state,
            label,
            reason,
        } => {
            eprintln!(
                "RSH-UI event ConnectionChanged state={state:?} label={label} reason={reason:?}"
            );
        }
        AppEvent::PairingWindow {
            open,
            seconds_left,
            board_mac,
        } => {
            eprintln!(
                "RSH-UI event PairingWindow open={open} seconds_left={seconds_left} board_mac={board_mac:?}"
            );
        }
        AppEvent::PairingFinished { ok, error } => {
            eprintln!("RSH-UI event PairingFinished ok={ok} error={error:?}");
        }
        AppEvent::Snapshot(_) => {}
        AppEvent::Notification { kind, title, body } => {
            eprintln!("RSH-UI event Notification kind={kind} title={title} body={body}");
        }
        AppEvent::JobProgress {
            id,
            state,
            progress,
            error,
        } => {
            eprintln!(
                "RSH-UI event JobProgress id={id} state={state} progress={progress:?} error={error:?}"
            );
        }
    }
}

/// React to one drained live event: pairing events drive the
/// wizard (a finished pairing marks the Pair step done, a window
/// event drives its countdown + ring); Notification events feed
/// the bell / drawer center (other events are ignored by it).
fn handle_live_event(win: &slint::Weak<MainWindow>, ctx: &Ctx, ev: &AppEvent) {
    match ev {
        AppEvent::PairingFinished { ok, error } => {
            if let Some(w) = win.upgrade() {
                if *ok {
                    w.set_setup_paired(true);
                    w.set_setup_pairing_active(false);
                    w.set_setup_pair_status("Paired — wheel connected.".into());
                    Ctx::sync_pair_ring(&w);
                    ctx.notify(
                        &w,
                        "Pairing",
                        "Wheel paired",
                        "Pairing complete — wheel connected.",
                    );
                } else {
                    let detail = error.as_ref().map_or_else(
                        || "Pairing failed".to_string(),
                        |e| format!("{} ({})", e.hint(), e.code()),
                    );
                    w.set_setup_pair_error(detail.clone().into());
                    ctx.notify(&w, "Pairing", "Pairing failed", &detail);
                }
            }
        }
        AppEvent::PairingWindow {
            open, seconds_left, ..
        } => {
            if let Some(w) = win.upgrade() {
                // B-1: the window opened — the wait is over (the
                // countdown now ticks from seconds_left).
                if *open {
                    *ctx.pairing_wait_since.borrow_mut() = None;
                }
                w.set_setup_pairing_active(*open);
                w.set_setup_pair_seconds_left(i32::try_from(*seconds_left).unwrap_or(120));
                if *open {
                    w.set_setup_pair_status(
                        "Pairing window open — hold the pair button on the wheel.".into(),
                    );
                }
                Ctx::sync_pair_ring(&w);
            }
        }
        _ => {}
    }
    let pushed = ctx.notifications.borrow_mut().push_event(ev);
    if pushed {
        // M10: daemon events persist like any other notification.
        ctx.persist_notifications();
        if let Some(w) = win.upgrade() {
            sync_to_window(&ctx.notifications.borrow(), &w);
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
        }
    }
}

/// Spawn the health-monitor thread (B2/B4): ping the live daemon
/// every second; on a death clear the shared link and report it,
/// then re-probe (spawning the daemon when needed) until it answers
/// again. The thread only talks to the UI through `tx` — the frame
/// timer applies the verdicts on the UI thread — and it owns no
/// `Rc` state, only the shared link. It exits when the UI drops
/// the receiver (app quit).
fn spawn_health_monitor(
    data_dir: Option<PathBuf>,
    live_link: Arc<Mutex<Option<Arc<IpcApi>>>>,
    tx: std::sync::mpsc::Sender<HealthEvent>,
    hidden: Arc<AtomicBool>,
) {
    let _ = std::thread::Builder::new()
        .name("rsh-health".to_string())
        .spawn(move || {
            let mut down_notified = false;
            loop {
                let current = live_link
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                if let Some(api) = current {
                    if !api.ping() {
                        // Only clear the link when it still
                        // holds the api that died — the user may
                        // have reconnected while the ping was in
                        // flight.
                        if let Ok(mut slot) = live_link.lock() {
                            if let Some(cur) = slot.as_ref() {
                                if Arc::ptr_eq(cur, &api) {
                                    *slot = None;
                                }
                            }
                        }
                        if !down_notified {
                            if tx.send(HealthEvent::DaemonDown).is_err() {
                                return;
                            }
                            down_notified = true;
                        }
                    }
                } else {
                    // H-1/A11: while the window is hidden to tray
                    // the monitor only probes for an EXISTING daemon
                    // — it must never spawn one, or a tray Quit
                    // (daemon gone on purpose) would resurrect it.
                    let found = if hidden.load(Ordering::Relaxed) {
                        live_probe::try_live_api(data_dir.as_deref())
                    } else {
                        live_probe::try_live_api_with_spawn(data_dir.as_deref())
                    };
                    if let Some(api) = found {
                        // Only publish when the slot is still
                        // empty — Reconnect may have beaten us.
                        let mut published = false;
                        if let Ok(mut slot) = live_link.lock() {
                            if slot.is_none() {
                                *slot = Some(api.clone());
                                published = true;
                            }
                        }
                        down_notified = false;
                        if published && tx.send(HealthEvent::DaemonBack(api)).is_err() {
                            return;
                        }
                    } else if !down_notified {
                        if tx.send(HealthEvent::DaemonDown).is_err() {
                            return;
                        }
                        down_notified = true;
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
    #[cfg(windows)]
    ui_lifecycle::restore_and_foreground();
    ctx.hidden.store(false, Ordering::SeqCst);
    if page == "setup" {
        ctx.clear_setup_marker();
        ctx.learn_target.set(0);
        win.set_setup_step(0);
        win.set_setup_centre_done(false);
        win.set_setup_pedals_done(false);
        win.set_setup_paired(win.get_paired());
        win.set_setup_reset_required(ctx.api().pairing_status().replacement_required);
        win.set_setup_mode(true);
        ctx.sync_vjoy(win);
        eprintln!("RSH-UI page request: wizard reopened");
    } else {
        win.set_setup_mode(false);
        win.set_page(ui_lifecycle::page_index(page));
        eprintln!("RSH-UI page request: {page}");
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
        eprintln!("RSH-UI tray quit requested");
        slint::quit_event_loop().ok();
    } else if ctx.quit_requested.get() {
        ctx.quit_requested.set(false);
        slint::quit_event_loop().ok();
    }
}

/// Start the 20 Hz frame timer: drain health verdicts, drain live
/// events only while a live API is attached, refresh the frame, and
/// honour `--quit-after`. The data source comes from the shared
/// state, so a Reconnect swap takes effect on the next tick. 1 Hz
/// tasks (pairing countdown tick-down, learn poll) run every 20th
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
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(50), move || {
        let t = tick.get();
        tick.set(t + 1);
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
            ctx.refresh(&w, t);
            if t.is_multiple_of(20) {
                // 1 Hz: job timeouts + the `connecting` guard (A9 —
                // never infinite) + firmware-update poll (H-4) +
                // local pairing countdown tick-down (A16 — the ring
                // follows seconds-left/total) + learn poll.
                // FC2 safety net: the busy strip + daemon-error
                // banner re-derive here too (both writes are
                // change-detected, so this is free when nothing
                // changed) — a job or fatal file that lands outside
                // the drain/timeout paths still reaches the UI.
                ctx.sync_busy(&w);
                ctx.sync_daemon_error(&w);
                ctx.check_job_timeouts(&w);
                ctx.check_connecting_timeout(&w);
                ctx.poll_firmware_update(&w);
                if w.get_setup_pairing_active() && !w.get_setup_paired() {
                    let left = w.get_setup_pair_seconds_left();
                    match left {
                        2.. => {
                            w.set_setup_pair_seconds_left(left - 1);
                        }
                        1 => {
                            // The window had 1 s left — now closed.
                            w.set_setup_pair_seconds_left(0);
                            w.set_setup_pairing_active(false);
                            w.set_setup_pair_status("Pairing window closed — try again.".into());
                        }
                        _ => {
                            // B-1: left == 0 means no window yet (still
                            // waiting), NOT a closed window. Give the
                            // board 10 s from the wait start, then give
                            // up honestly instead of expiring on the
                            // first tick.
                            let waited = ctx
                                .pairing_wait_since
                                .borrow()
                                .is_some_and(|since| since.elapsed() >= Duration::from_secs(10));
                            if waited {
                                *ctx.pairing_wait_since.borrow_mut() = None;
                                w.set_setup_pairing_active(false);
                                w.set_setup_pair_status(
                                    "No pairing window — check the wheel is on and try again."
                                        .into(),
                                );
                                ctx.notify(
                                    &w,
                                    "Pairing",
                                    "No pairing window",
                                    "The wheel did not open a pairing window — check it is on and try again.",
                                );
                            }
                        }
                    }
                    Ctx::sync_pair_ring(&w);
                }
                ctx.poll_learn(&w);
                // vJoy install polling: until installed or failed.
                if ctx.vjoy_polling.get() {
                    ctx.sync_vjoy(&w);
                    if w.get_setup_vjoy_installed() {
                        ctx.vjoy_polling.set(false);
                        ctx.notify(
                            &w,
                            "Drivers",
                            "vJoy installed",
                            "vJoy is installed — games can see your wheel.",
                        );
                    } else if !w.get_setup_vjoy_installing() && w.get_setup_vjoy_error() != "" {
                        ctx.vjoy_polling.set(false);
                        ctx.notify(
                            &w,
                            "Drivers",
                            "vJoy install failed",
                            &w.get_setup_vjoy_error(),
                        );
                    }
                } else if w.get_setup_mode() && w.get_setup_step() == 1 {
                    ctx.sync_vjoy(&w);
                }
            }
            if t.is_multiple_of(100) {
                ctx.refresh_calibration();
                ctx.refresh_profiles(&w);
                ctx.refresh_games(&w);
                // H9: while the wizard sits on Calibrate, re-probe
                // raw mV so Next enables when wheel data arrives.
                if w.get_setup_mode() && w.get_setup_step() == 3 {
                    ctx.start_raw_available_job(&w);
                }
                if w.get_diag_open() {
                    ctx.sync_diag(&w);
                }
            }
        }
        if let Some(secs) = quit_after {
            if t0.elapsed() >= Duration::from_secs(secs) {
                slint::quit_event_loop().ok();
            }
        }
    });
    // Keep the timer alive for the run.
    std::mem::forget(timer);
}

#[allow(clippy::too_many_lines)]
fn main() -> Result<(), slint::PlatformError> {
    let t0 = Instant::now();
    let args = parse_args();
    let data_dir = live_probe::resolve_data_dir(args.data_dir.as_deref());

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
        calibration: Rc::new(RefCell::new(None)),
        vjoy_polling: Rc::new(Cell::new(false)),
        button_actions: Rc::new(RefCell::new(
            DEFAULT_BUTTON_ACTIONS
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
        )),
        pending_action: Rc::new(RefCell::new(None)),
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
        pending_profile_id: Rc::new(RefCell::new(None)),
        learn_target: Rc::new(Cell::new(0)),
        horn_test_until: Rc::new(Cell::new(None)),
        steer_hist: Rc::new(RefCell::new(VecDeque::new())),
        pedal_hist: Rc::new(RefCell::new(VecDeque::new())),
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
        pairing_wait_since: Rc::new(RefCell::new(None)),
        job_tx,
        job_rx: Rc::new(RefCell::new(job_rx)),
        busy_jobs: Rc::new(RefCell::new(HashMap::new())),
        // A9: the window starts in "Connecting…" — the clock starts
        // now, so a daemon that never answers is reported after 15 s
        // instead of leaving the Overview on "Connecting…" forever.
        connecting_since: Rc::new(Cell::new(Some(t0))),
        reconnect_in_flight: Rc::new(Cell::new(false)),
        firmware_check_cache: Rc::new(RefCell::new(None)),
        raw_available: Rc::new(Cell::new(false)),
        presets_count: Rc::new(Cell::new(0)),
        module_states: Rc::new(RefCell::new([true; 4])),
        firmware_update_running: Rc::new(Cell::new(false)),
        last_address_notify: Rc::new(Cell::new(None)),
        steer_push_due: Rc::new(Cell::new(None)),
        pending_field: Rc::new(RefCell::new(None)),
    });

    // The health monitor owns the first probe (B4): it starts now,
    // on its own thread, while the window below opens.
    spawn_health_monitor(data_dir.clone(), live_link, health_tx, ctx.hidden.clone());

    let win = MainWindow::new()?;
    // H-1/A11: X hides to tray — the UI process stays alive (the
    // daemon keeps running, tray "Open RSH" restores it instantly).
    // Real quit is only via tray "Quit RSH" (quit file) or with
    // "Keep running in the tray" off (then the frame timer quits on
    // the next tick — quitting from inside this callback would be
    // re-entrant).
    {
        let ctx_close = ctx.clone();
        win.window().on_close_requested(move || {
            let to_tray = ctx_close.api().close_to_tray();
            ctx_close.hidden.store(true, Ordering::SeqCst);
            if !to_tray {
                ctx_close.quit_requested.set(true);
            }
            slint::CloseRequestResponse::HideWindow
        });
    }
    // M-5/A1: listen for page forwards from second instances
    // (Windows named pipe; Linux uses the request file).
    ui_lifecycle::listen_for_page_requests(ctx.page_requests.clone());
    // First-run wizard: --page setup, or no setup-done marker yet.
    // A19: upgrade users see the wizard once (the marker is new) —
    // kept on purpose now that the Drivers step is in it.
    let setup = args.setup || !ctx.is_setup_done();
    win.set_page(args.page);
    win.set_setup_mode(setup);
    win.set_setup_step(0);
    win.set_theme(resolve_theme_mode(
        read_theme(data_dir.as_ref()),
        system_light(),
    ));
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
    sync_to_window(&ctx.notifications.borrow(), &win);
    ctx.sync_pedals(&win);
    ctx.sync_extras(&win);
    ctx.sync_overview(&win);
    ctx.refresh_profiles(&win);
    ctx.refresh_games(&win);
    ctx.sync_vjoy(&win);
    ctx.sync_diag(&win);
    ctx.refresh(&win, 0);
    // No startup pairing seeding here any more (B4): the live API
    // does not exist yet — `reconnect_to_live` seeds the wizard's
    // pairing state when the daemon answers. The "not running"
    // notification comes from the monitor's first verdict instead.
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
                // FC2: same 1 Hz safety net as the frame timer —
                // the daemon-error banner re-derives (change-
                // detected, so free when nothing changed).
                ctx_static.sync_daemon_error(&w);
                ctx_static.poll_firmware_update(&w);
                ctx_static.flush_steering_push(&w);
                ctx_static.flush_field_push(&w);
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

    win.run()
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn countdown_ring_drives_from_seconds() {
        assert_eq!(countdown_ring_commands(0, 120), "");
        let full = countdown_ring_commands(120, 120);
        assert!(full.contains("A 58 58"), "{full}");
        let half = countdown_ring_commands(60, 120);
        assert!(half.contains("66.0 124.0"), "{half}");
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
    fn fc2_test_ctx() -> (Rc<Ctx>, PathBuf) {
        let dir = std::env::temp_dir().join(format!("rsh-fc2-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("fc2 temp dir");
        let api: Arc<dyn AppApi> = Arc::new(SimApi::new());
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
            calibration: Rc::new(RefCell::new(None)),
            vjoy_polling: Rc::new(Cell::new(false)),
            button_actions: Rc::new(RefCell::new(
                DEFAULT_BUTTON_ACTIONS
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect(),
            )),
            pending_action: Rc::new(RefCell::new(None)),
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
            pending_profile_id: Rc::new(RefCell::new(None)),
            learn_target: Rc::new(Cell::new(0)),
            horn_test_until: Rc::new(Cell::new(None)),
            steer_hist: Rc::new(RefCell::new(VecDeque::new())),
            pedal_hist: Rc::new(RefCell::new(VecDeque::new())),
            unsaved: Rc::new(Cell::new(false)),
            notifications: Rc::new(RefCell::new(NotificationCenter::load(Some(&dir)))),
            data_dir: Some(dir.clone()),
            live_link: Arc::new(Mutex::new(None)),
            hidden: Arc::new(AtomicBool::new(false)),
            quit_requested: Rc::new(Cell::new(false)),
            page_requests: Arc::new(Mutex::new(VecDeque::new())),
            pairing_wait_since: Rc::new(RefCell::new(None)),
            job_tx,
            job_rx: Rc::new(RefCell::new(job_rx)),
            busy_jobs: Rc::new(RefCell::new(HashMap::new())),
            connecting_since: Rc::new(Cell::new(Some(Instant::now()))),
            reconnect_in_flight: Rc::new(Cell::new(false)),
            firmware_check_cache: Rc::new(RefCell::new(None)),
            raw_available: Rc::new(Cell::new(false)),
            presets_count: Rc::new(Cell::new(0)),
            module_states: Rc::new(RefCell::new([true; 4])),
            firmware_update_running: Rc::new(Cell::new(false)),
            last_address_notify: Rc::new(Cell::new(None)),
            steer_push_due: Rc::new(Cell::new(None)),
            pending_field: Rc::new(RefCell::new(None)),
        });
        (ctx, dir)
    }

    /// FC2 end-to-end on a real `MainWindow` (needs a display — run
    /// under Xvfb; skips gracefully headless so the plain suite
    /// stays green). Drives the four new callbacks through the
    /// generated `invoke_*` methods and the busy/daemon-error props
    /// through the real setters, asserting the window props:
    /// - busy on at job start, off after the job drains;
    /// - daemon-error set from a planted fatal file, cleared live;
    /// - pair-now opens the wizard at the Pair step;
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

        // 3. Pair-now opens the wizard at the Pair step.
        win.invoke_pair_now_requested();
        assert!(win.get_setup_mode(), "pair-now opens the wizard");
        assert_eq!(win.get_setup_step(), 2, "wizard opens at Pair");

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
        assert_eq!(unread(), before + 1, "unpersistable address notifies");
        assert_eq!(win.get_setup_address().as_str(), live_addr);

        // 5. Keep-tray reverts on a failed persist (sim: no daemon).
        win.set_keep_in_tray(true);
        let before = unread();
        win.invoke_keep_tray_selected(false);
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
}
