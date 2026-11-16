//! RSH v2.0 — Phase 3 shell + components (REDESIGN-PLAN §3-§6),
//! v2.0.1 hotfix: UI core (daemon self-heal, first-run wizard,
//! wired Settings / Pedals / Buttons, diagnostics drawer, theme
//! persist) + UI-FINISH deep-scan fixes (see the fix notes on each
//! function: A1–A25, B1–B7, C1–C8, D, E).
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

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rsh_api::connection::ConnectionState;
use rsh_api::pairing::countdown_ring_commands;
use rsh_api::{
    ApiTransport, AppApi, AppEvent, CalibrationInfo, GameSummary, LedPattern, ProfileSummary,
    ShifterGate, TuningSettings,
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

/// Rewrite user-facing "daemon" wording (C1): the background service
/// is called by its plain name, never "daemon" / "Simulated data".
#[must_use]
pub fn user_text(s: &str) -> String {
    s.replace("Daemon", "Background service")
        .replace("daemon", "background service")
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

/// Tile code letters from a game name: first letters of the first
/// two words ("Assetto Corsa" → "AC", "iRacing" → "IR").
#[must_use]
pub fn game_code(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    if words.len() >= 2 {
        words
            .iter()
            .take(2)
            .filter_map(|w| w.chars().next())
            .collect::<String>()
            .to_uppercase()
    } else {
        name.chars().take(2).collect::<String>().to_uppercase()
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

/// Static LED strip preview (8 circles) from pattern + brightness:
/// Off is dark; any lit pattern shows the accent at the set
/// brightness (a still preview, not a live strip — the strip itself
/// is not readable through the API).
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn led_preview(pattern: LedPattern, brightness: u8) -> Vec<Color> {
    if pattern == LedPattern::Off {
        return vec![Color::from_rgb_u8(0x1D, 0x25, 0x33); 8];
    }
    let b = f32::from(brightness) / 100.0;
    let (r, g, bl) = (47.0, 107.0, 255.0);
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

/// Read the persisted UI theme (0 Dark, 1 Auto) from
/// `<data_dir>/ui-theme` ("Dark"/"Auto", default Dark). Auto
/// renders as Dark — no light tokens exist.
fn read_theme(data_dir: Option<&PathBuf>) -> i32 {
    let Some(dir) = data_dir else {
        return 0;
    };
    match std::fs::read_to_string(dir.join("ui-theme")) {
        Ok(text) if text.trim() == "Auto" => 1,
        _ => 0,
    }
}

/// Persist the UI theme choice (`settings_idx` is the Settings
/// screen index: 0 Dark, 1 Light, 2 Auto — Light has no tokens, so
/// it persists as Dark).
fn write_theme(data_dir: Option<&PathBuf>, settings_idx: i32) {
    let Some(dir) = data_dir else {
        return;
    };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let text = if settings_idx == 2 { "Auto" } else { "Dark" };
    match std::fs::write(dir.join("ui-theme"), format!("{text}\n")) {
        Ok(()) => eprintln!("RSH-UI theme persisted: {text}"),
        Err(e) => eprintln!("RSH-UI theme persist failed: {e}"),
    }
}

/// Swappable data source (v2.0.1 hotfix): the active [`AppApi`]
/// plus both backends. Reconnect re-probes (spawning the daemon when
/// needed) and swaps `api` to live without restarting; the
/// sim stays stored — frozen, not advanced — as the fallback.
struct LiveState {
    api: Arc<dyn AppApi>,
    live_api: Option<Arc<IpcApi>>,
    sim_api: Option<Arc<SimApi>>,
    live: bool,
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

    /// SEAM (BACKEND-2, verified 2026-11-16): `calibration`,
    /// `vjoy_status`, `install_vjoy` and `diag_stats` DO exist on
    /// `AppApi` and are wired below. `set_centre` / `calibrate_pedal`
    /// / `set_button_action` deliberately do NOT exist — BACKEND-2:
    /// raw mV never reaches the state (capture needs a v2.0.2 engine
    /// change) and button→action rebinding is not a board feature.
    /// So every capture control stays disabled with that reason.
    /// This is the capture gate: always false until a capture
    /// method lands.
    fn capture_available() -> bool {
        false
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
    #[allow(clippy::cast_possible_truncation, clippy::too_many_lines)]
    fn push_frame(&self, win: &MainWindow, tick: u64) {
        let (api, live) = {
            let state = self.state.borrow();
            (state.api.clone(), state.live)
        };
        let conn = api.connection();
        let snap = api.snapshot();
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
        win.set_conn_label(if live {
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
        let sub = if live {
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
        win.set_steer_text(format!("{steer_deg:+.0}°").into());
        win.set_throttle((snap.throttle * 100.0) as f32);
        win.set_brake((snap.brake * 100.0) as f32);
        win.set_clutch((snap.clutch.unwrap_or(0.0) * 100.0) as f32);
        // A7: no handbrake / gear source in SnapshotInfo — 0 / "—".
        win.set_handbrake(0.0);
        win.set_gear("—".into());
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
        // A5: pressed-index comes ONLY from live button state — the
        // snapshot has none, so it is always -1 (no fake cycle).
        win.set_pressed_index(-1);
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
        {
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
        }
    }

    fn refresh(&self, win: &MainWindow, tick: u64) {
        self.push_frame(win, tick);
    }

    /// Push the current steering tuning to the daemon (L4): builds a
    /// `TuningSettings` from the local slider state over the cached
    /// stored record and calls `set_tuning`. On success the cached
    /// record updates and the profile is marked unsaved; on failure
    /// the error is logged and the local state stays (the slider
    /// still moves visually — the push is retried on the next edit).
    fn push_steering(&self) {
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
            Err(e) => eprintln!("RSH-UI set_tuning failed: {} ({})", e.code, e.title),
        }
    }

    /// Push a VISIBLE notification and sync the drawer model. Every
    /// user-facing handler uses this — never silent, never
    /// eprintln-only (stderr is invisible under
    /// `windows_subsystem = "windows"`). "daemon" wording is
    /// rewritten to "background service" (C1).
    fn notify(&self, win: &MainWindow, kind: &str, title: &str, body: &str) {
        let title = user_text(title);
        let body = user_text(body);
        self.notifications.borrow_mut().push(kind, &title, &body);
        sync_to_window(&self.notifications.borrow(), win);
    }

    /// Swap the shared API state to a live daemon connection (the
    /// Reconnect path): the daemon becomes the API, the sim stays
    /// stored (frozen) as fallback, and the steering sliders
    /// re-seed from the daemon's stored record. Caller re-syncs the
    /// pedal / extras / overview props afterwards.
    fn swap_to_live(&self, live_api: Arc<IpcApi>) {
        let api = live_api.clone() as Arc<dyn AppApi>;
        let stored = api.get_tuning("steering");
        *self.tuning.borrow_mut() = Tuning::from_settings(&stored);
        *self.steering.borrow_mut() = stored;
        self.unsaved
            .set(api.draft_status().is_some_and(|s| s.dirty));
        let mut state = self.state.borrow_mut();
        state.api = api;
        state.live_api = Some(live_api);
        state.live = true;
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
        // Set up/down need the capture backend (A2 seam — no backend:
        // raw mV never reaches the state).
        win.set_pedal_set_disabled(!Self::capture_available());
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
        win.set_handbrake_mode(0);
        win.set_extras_calibration_disabled(!Self::capture_available());
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
        win.set_led_preview(ModelRc::new(Rc::new(VecModel::from(led_preview(
            pattern, brightness,
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
        let address = if conn.address.is_empty() {
            "rsh.local".to_string()
        } else {
            conn.address.clone()
        };
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
        // C3: Settings texts from the API, not literals.
        win.set_firmware_text(if firmware == "Unknown" {
            "—".into()
        } else if firmware == "4.0.2" {
            "Version 4.0.2 · up to date".into()
        } else {
            format!("Version {firmware}").into()
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
        win.set_popover_presets(ModelRc::new(Rc::new(VecModel::from(
            Vec::<ProfileItem>::new(),
        ))));
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
        let firmware = api.firmware_version();
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
        win.set_diag_buttons_pressed(ModelRc::new(Rc::new(VecModel::from(vec![false; 12]))));
        let mut lines: Vec<SharedString> = vec![
            format!("App {version}").into(),
            format!(
                "Background service: {}",
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
fn setup_connect_to(w: &MainWindow, ctx: &Ctx, transport: ApiTransport) {
    match ctx.api().connect(transport) {
        Ok(info) => {
            w.set_setup_transport(i32::from(transport == ApiTransport::Usb));
            if matches!(info.state, ConnectionState::Active) {
                w.set_setup_connect_done(true);
            }
            eprintln!("RSH-UI setup connect ok: {}", info.label);
            ctx.refresh(w, 0);
        }
        Err(e) => {
            ctx.notify(w, "Connection", "Connect failed", &e.title);
            eprintln!("RSH-UI setup connect failed: {} ({})", e.code, e.title);
        }
    }
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
                match ctx.api().set_output_backend("HIDMaestro") {
                    Ok(_) => {
                        w.set_setup_vjoy_fallback(true);
                        w.set_controller(0);
                        w.set_setup_vjoy_error("".into());
                        ctx.sync_overview(&w);
                        ctx.notify(
                            &w,
                            "Drivers",
                            "HIDMaestro selected",
                            "HIDMaestro selected — RSH will ask for admin permission on start.",
                        );
                    }
                    Err(e) => {
                        ctx.notify(&w, "Drivers", "Couldn't switch controller", &e.title);
                    }
                }
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
                ctx.notify(
                    &w,
                    "Pairing",
                    "Reset pairing",
                    "This wheel is already paired — hold BOOT for 10 seconds to reset it, then pair again.",
                );
                begin_pairing_flow(&w, &ctx);
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
        // A17: Calibrate calls the real calibration backend — centre
        // via set_centre(), pedals via calibrate_pedal(). Both are
        // BACKEND-2 seam methods; until they land the buttons are
        // disabled in the wizard and this handler only explains why
        // (never a fake done).
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_setup_calibrate(move |a| {
            let Some(w) = win.upgrade() else {
                return;
            };
            match a.as_str() {
                "centre" | "pedals" => {
                    ctx.notify(
                        &w,
                        "Calibration",
                        "Calibration unavailable",
                        "Calibration capture needs a later wheel update.",
                    );
                }
                other => eprintln!("RSH-UI setup calibrate: unknown action {other}"),
            }
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

/// Wire the Pedals page callbacks (v2.0.1 hotfix + UI-FINISH A1/A2).
///
/// Pedal index order (verified in `ui/screens/pedals.slint`):
/// 0 throttle, 1 brake, 2 clutch. A1: the deadzone slider is 0..1
/// of 10% — slider value = pct/10, handler does v*10 (the old
/// `v * 100` read 20% as "2%"). Typed deadzone values are percents
/// ("2%"); the bite slider is 0..1 and typed bite values are
/// percents ("40%" → 0.40). Feel chips push the curve (Linear 1.0 /
/// Soft 0.7 / Sharp 1.5) through `set_tuning`. Set up/down need
/// `calibrate_pedal` (BACKEND-2 seam) and stay disabled until then.
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
            ctx.set_pedal_tuning(&w, module, |t| t.deadzone = deadzone);
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
}

/// Wire the clutch bite-point, feel-preset and Set up/down
/// callbacks (split from [`wire_pedals`]).
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
            ctx.set_pedal_tuning(&w, "clutch", |t| t.bite_point = Some(bite));
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
    {
        // A2: SEAM — calibrate_pedal(module, "up"/"down") once
        // BACKEND-2 lands it. The buttons are disabled until then.
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_pedal_set_up(move |_| {
            if let Some(w) = win.upgrade() {
                ctx.notify(
                    &w,
                    "Pedals",
                    "Pedal calibration",
                    "Calibration capture needs a later wheel update.",
                );
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_pedal_set_down(move |_| {
            if let Some(w) = win.upgrade() {
                ctx.notify(
                    &w,
                    "Pedals",
                    "Pedal calibration",
                    "Calibration capture needs a later wheel update.",
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
            // A3: SEAM — set_button_action(button, action) once
            // BACKEND-2 lands it. Until then a pending mapping
            // cannot be saved to the wheel: say so, keep it pending.
            if ctx.edit_kind.get() == 1 {
                ctx.notify(
                    &w,
                    "Buttons",
                    "Knob mapping isn't supported yet",
                    "Knob mapping isn't a wheel feature.",
                );
                return;
            }
            if let Some(action) = ctx.pending_action.borrow().clone() {
                let button = ctx.selected_button.get() + 1;
                ctx.notify(
                    &w,
                    "Buttons",
                    "Couldn't save button mapping",
                    &format!(
                        "Button {button} → {action} isn't a wheel feature — your choice stays on this screen."
                    ),
                );
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
                    ctx.notify(&w, "Buttons", "Couldn't save changes", &e.title);
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
/// opens the Buttons page. Controls with no backend (handbrake mode,
/// sensitivity, LED colour, Set up/down) explain themselves instead
/// of pretending to work.
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
                    // Module on/off has no backend field — the card
                    // greying out IS the visible effect.
                    eprintln!("RSH-UI extras toggle {a}");
                }
                "handbrake-up" | "handbrake-down" => {
                    // SEAM: calibrate_pedal("handbrake", …).
                    ctx.notify(
                        &w,
                        "Extras",
                        "Handbrake calibration",
                        "Calibration capture needs a later wheel update.",
                    );
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
        win.unwrap().on_extras_handbrake_mode(move |_| {
            if let Some(w) = win.upgrade() {
                // No backend for the handbrake mode (firmware F-15):
                // stay on Axis and explain.
                w.set_handbrake_mode(0);
                ctx.notify(
                    &w,
                    "Extras",
                    "Handbrake mode",
                    "Handbrake button mode needs a wheel setting — staying on Axis.",
                );
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
            if let Some(w) = win.upgrade() {
                let deadzone = (f64::from(v) * 10.0).clamp(0.0, 10.0);
                ctx.set_module_tuning(&w, "handbrake", |t| t.deadzone = deadzone);
            }
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
            if let Some(w) = win.upgrade() {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let brightness = (v.clamp(0.0, 1.0) * 100.0).round() as u8;
                ctx.set_module_tuning(&w, "led", |t| t.led_brightness = Some(brightness));
            }
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
        win.unwrap().on_extras_led_color(move |_| {
            if let Some(w) = win.upgrade() {
                ctx.notify(
                    &w,
                    "Extras",
                    "LED colour",
                    "LED colour isn't stored on the wheel yet.",
                );
            }
        });
    }
}

/// Wire the theme-persist callback (v2.0.1 hotfix): the Settings
/// screen reports its theme index (0 Dark, 1 Light, 2 Auto) and
/// Rust writes `<data_dir>/ui-theme`.
fn wire_theme(win: &MainWindow, ctx: &Rc<Ctx>) {
    let win = win.as_weak();
    let ctx = ctx.clone();
    win.unwrap().on_theme_persist(move |i| {
        write_theme(ctx.data_dir.as_ref(), i);
        if let Some(w) = win.upgrade() {
            w.set_theme(i32::from(i == 2));
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

/// Wire Reconnect (v2.0.1 hotfix): re-probes via
/// `try_live_api_with_spawn` (spawning the daemon when it is not
/// running) and swaps to live data on success, with a visible
/// notification either way.
fn wire_reconnect(win: &MainWindow, ctx: &Rc<Ctx>) {
    let win = win.as_weak();
    let ctx = ctx.clone();
    win.unwrap().on_reconnect(move || {
        let Some(w) = win.upgrade() else {
            return;
        };
        match live_probe::try_live_api_with_spawn(ctx.data_dir.as_deref()) {
            Some(live_api) => {
                ctx.swap_to_live(live_api);
                w.set_daemon_live(true);
                ctx.notify(
                    &w,
                    "Connection",
                    "Background service connected",
                    "Live data is back.",
                );
                if let Err(e) = ctx.api().connect(ApiTransport::Wifi) {
                    ctx.notify(&w, "Connection", "Reconnect failed", &e.title);
                    eprintln!("RSH-UI reconnect failed: {} ({})", e.code, e.title);
                }
                ctx.refresh_calibration();
                ctx.sync_pedals(&w);
                ctx.sync_extras(&w);
                ctx.sync_overview(&w);
                ctx.refresh_profiles(&w);
                ctx.refresh_games(&w);
                ctx.sync_diag(&w);
                ctx.refresh(&w, 0);
            }
            None => {
                ctx.notify(
                    &w,
                    "Connection",
                    "Background service not running",
                    "Live data unavailable — the background service isn't running.",
                );
            }
        }
    });
}

/// Wire Set centre + Launch game (A14/A22): Set centre needs the
/// calibration backend (`set_centre` seam) — the button is disabled
/// until then. Launch game works off the real games list: running
/// → Stop, installed → Launch, not installed → honest refusal.
fn wire_action_notifications(win: &MainWindow, ctx: &Rc<Ctx>) {
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_set_centre(move || {
            if let Some(w) = win.upgrade() {
                // SEAM: api.set_centre() once BACKEND-2 lands it.
                ctx.notify(
                    &w,
                    "Steering",
                    "Set centre",
                    "Calibration capture needs a later wheel update.",
                );
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
        win.unwrap().on_popover_use_preset(move |_| {
            if let Some(w) = win.upgrade() {
                ctx.notify(
                    &w,
                    "Profile",
                    "Presets",
                    "There are no presets — create a profile with New.",
                );
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
                ctx.duplicate_profile_index(&w, i);
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_popover_rename(move |_| {
            if let Some(w) = win.upgrade() {
                ctx.notify(&w, "Profile", "Rename", "Renaming isn't supported yet.");
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_popover_delete(move |i| {
            if let Some(w) = win.upgrade() {
                ctx.delete_profile_index(&w, i);
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_popover_link(move |_| {
            if let Some(w) = win.upgrade() {
                ctx.notify(
                    &w,
                    "Profile",
                    "Link to game",
                    "Linking a profile to a game isn't supported yet.",
                );
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
            match ctx.api().connect(transport) {
                Ok(info) => {
                    eprintln!("RSH-UI transport switch ok: {}", info.label);
                    ctx.refresh(&w, 0);
                }
                Err(e) => {
                    // Revert: the binding already flipped the switch.
                    w.set_transport(1 - i);
                    ctx.notify(&w, "Connection", "Couldn't switch connection", &e.title);
                    eprintln!("RSH-UI transport switch failed: {} ({})", e.code, e.title);
                }
            }
        });
    }
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_output_backend_selected(move |i| {
            let Some(w) = win.upgrade() else {
                return;
            };
            let name = if i == 1 { "vJoy" } else { "HIDMaestro" };
            match ctx.api().set_output_backend(name) {
                Ok(active) => {
                    ctx.sync_overview(&w);
                    ctx.notify(
                        &w,
                        "Game controller",
                        "Controller switched",
                        &format!("Game controller is now {active}."),
                    );
                }
                Err(e) => {
                    w.set_controller(1 - i);
                    ctx.notify(
                        &w,
                        "Game controller",
                        "Couldn't switch controller",
                        &e.title,
                    );
                    eprintln!(
                        "RSH-UI output-backend switch failed: {} ({})",
                        e.code, e.title
                    );
                }
            }
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

/// Run one pairing attempt from Settings and notify the result.
fn settings_pair(w: &MainWindow, ctx: &Ctx) {
    match ctx.api().begin_pairing() {
        Ok(info) => {
            if info.seconds_left > 0 {
                ctx.notify(
                    w,
                    "Pairing",
                    "Pairing window open",
                    &format!("Pairing window open — {}s left", info.seconds_left),
                );
            } else {
                ctx.notify(
                    w,
                    "Pairing",
                    "Pairing started",
                    "Waiting for the wheel — hold the pair button for 3 seconds.",
                );
            }
        }
        Err(e) => ctx.notify(w, "Pairing", "Pairing failed", &e.title),
    }
}

/// Report the wheel firmware version from Settings.
fn settings_check_firmware(w: &MainWindow, ctx: &Ctx) {
    let v = ctx.api().firmware_version();
    if v == "4.0.2" {
        ctx.notify(
            w,
            "Firmware",
            "Firmware up to date",
            &format!("Wheel firmware {v} — up to date"),
        );
    } else {
        ctx.notify(
            w,
            "Firmware",
            "Wheel firmware",
            &format!("Wheel firmware {v}"),
        );
    }
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
                "address" => ctx.notify(
                    &w,
                    "Settings",
                    "Address",
                    "Address applies when you reconnect",
                ),
                "tray" => ctx.notify(
                    &w,
                    "Settings",
                    "Keep in tray",
                    "RSH keeps running in the tray.",
                ),
                "new-profile" => ctx.create_new_profile(&w),
                "check-firmware" => settings_check_firmware(&w, &ctx),
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
                    Err(e) => ctx.notify(&w, "Backup", "Backup failed", &e.title),
                },
                "restore" => ctx.notify(&w, "Backup", "Restore", "Restore isn't supported yet."),
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
                    ctx.notify(&w, "Profile", "Couldn't save changes", &e.title);
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
        win.unwrap().on_auto_pause_toggled(move || {
            if let Some(w) = win.upgrade() {
                let next = !w.get_auto_paused();
                w.set_auto_paused(next);
                eprintln!("RSH-UI auto-profile paused={next}");
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
        if let Some(w) = win.upgrade() {
            sync_to_window(&ctx.notifications.borrow(), &w);
        }
    }
}

/// Start the 20 Hz frame timer: advance the sim only while it is
/// the active source, drain live events only while a live API is
/// attached, refresh the frame, and honour `--quit-after`. The data
/// source comes from the shared state, so a Reconnect swap takes
/// effect on the next tick. 1 Hz tasks (pairing countdown tick-down,
/// learn poll) run every 20th tick; 5 s tasks (profiles/games
/// refresh, diagnostics re-sync while open) every 100th tick.
fn start_frame_timer(win: &MainWindow, ctx: &Rc<Ctx>, quit_after: Option<u64>, t0: Instant) {
    let win = win.as_weak();
    let ctx = ctx.clone();
    let tick = Rc::new(Cell::new(0_u64));
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(50), move || {
        let t = tick.get();
        tick.set(t + 1);
        // Data source comes from the shared state so a Reconnect
        // swap takes effect on the next tick: advance the sim
        // only while it is the active source, drain live events
        // only while a live API is attached.
        let (live_api, sim_api) = {
            let state = ctx.state.borrow();
            (state.live_api.clone(), state.sim_api.clone())
        };
        if let Some(sim) = &sim_api {
            if !ctx.is_live() {
                sim.advance(0.05);
            }
        }
        if let Some(live) = &live_api {
            for ev in live.poll_events() {
                log_event(&ev);
                handle_live_event(&win, &ctx, &ev);
            }
        }
        if let Some(w) = win.upgrade() {
            ctx.refresh(&w, t);
            if t.is_multiple_of(20) {
                // 1 Hz: local pairing countdown tick-down (A16 —
                // the ring follows seconds-left/total) + learn poll.
                if w.get_setup_pairing_active() && !w.get_setup_paired() {
                    let left = w.get_setup_pair_seconds_left();
                    if left > 1 {
                        w.set_setup_pair_seconds_left(left - 1);
                    } else {
                        w.set_setup_pair_seconds_left(0);
                        w.set_setup_pairing_active(false);
                        w.set_setup_pair_status("Pairing window closed — try again.".into());
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

    // Data source: live daemon API when it answers (spawning the
    // sibling daemon first when it is not running), simulated
    // otherwise. The sim fallback posts one visible notification so
    // the user knows the data is not live.
    let live_api = live_probe::try_live_api_with_spawn(data_dir.as_deref());
    let live = live_api.is_some();
    let sim_api: Option<Arc<SimApi>> = if live {
        None
    } else {
        Some(Arc::new(SimApi::new()))
    };
    let api: Arc<dyn AppApi> = match (&live_api, &sim_api) {
        (Some(live), _) => live.clone() as Arc<dyn AppApi>,
        (_, Some(sim)) => sim.clone() as Arc<dyn AppApi>,
        _ => unreachable!("one data source always exists"),
    };
    eprintln!(
        "RSH-UI daemon-live={live} ({})",
        if live {
            "live daemon data via rsh-ipc"
        } else {
            "simulated data until the daemon answers"
        }
    );

    // L4 startup: seed the slider state from the stored steering
    // record (live daemon or sim) and the unsaved dot from the draft.
    let stored_steering = api.get_tuning("steering");
    let initial_unsaved = api.draft_status().is_some_and(|s| s.dirty);
    let ctx = Rc::new(Ctx {
        state: Rc::new(RefCell::new(LiveState {
            api: api.clone(),
            live_api: live_api.clone(),
            sim_api: sim_api.clone(),
            live,
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
        notifications: Rc::new(RefCell::new(NotificationCenter::new())),
        data_dir: data_dir.clone(),
    });

    let win = MainWindow::new()?;
    // First-run wizard: --page setup, or no setup-done marker yet.
    // A19: upgrade users see the wizard once (the marker is new) —
    // kept on purpose now that the Drivers step is in it.
    let setup = args.setup || !ctx.is_setup_done();
    win.set_page(args.page);
    win.set_setup_mode(setup);
    win.set_setup_step(0);
    win.set_theme(read_theme(data_dir.as_ref()));
    win.set_app_version(api.version().into());
    win.set_daemon_live(live);
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
    // Default game controller is vJoy (user order 2026-11-16); when
    // live, seed from the stored backend instead.
    if live {
        if api.output_backend() == "HIDMaestro" {
            win.set_controller(0);
        } else {
            // vJoy (the default) and anything unknown: vJoy.
            win.set_controller(1);
        }
    }
    // Capture gate (no capture backend — BACKEND-2): centre /
    // pedal Set / wizard Calibrate stay disabled with a reason.
    win.set_centre_available(Ctx::capture_available());
    win.set_pedal_set_disabled(!Ctx::capture_available());
    win.set_extras_calibration_disabled(!Ctx::capture_available());
    win.set_setup_calibration_available(Ctx::capture_available());
    win.set_centre_reason("Calibration capture needs a later wheel update.".into());
    win.set_setup_calibration_reason("Calibration capture needs a later wheel update.".into());
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
    // One-time pairing status: an already-open window shows its countdown.
    {
        let ps = api.pairing_status();
        if ps.seconds_left > 0 {
            win.set_setup_pairing_active(true);
            win.set_setup_pair_seconds_left(i32::try_from(ps.seconds_left).unwrap_or(120));
            win.set_setup_pair_total(i32::try_from(ps.seconds_left).unwrap_or(120));
            win.set_setup_pair_status(
                "Pairing window open — hold the pair button on the wheel.".into(),
            );
            Ctx::sync_pair_ring(&win);
        }
        // TR §3c: Active session at startup = already paired.
        if matches!(api.connection().state, ConnectionState::Active) {
            win.set_setup_paired(true);
        }
    }
    // Sim fallback is visible, once: the bell carries the reason.
    if !live {
        ctx.notify(
            &win,
            "Connection",
            "Background service not running",
            "Live data unavailable — the background service isn't running.",
        );
    }
    eprintln!("RSH-UI window shown in {} ms", t0.elapsed().as_millis());

    wire_callbacks(&win, &ctx);

    // ---- frame timer (20 Hz) + --quit-after
    if !args.static_mode {
        start_frame_timer(&win, &ctx, args.quit_after, t0);
    } else if let Some(secs) = args.quit_after {
        let timer = Timer::default();
        timer.start(TimerMode::SingleShot, Duration::from_secs(secs), || {
            slint::quit_event_loop().ok();
        });
        std::mem::forget(timer);
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
    fn game_code_from_names() {
        assert_eq!(game_code("Assetto Corsa"), "AC");
        assert_eq!(game_code("AC Competizione"), "AC");
        assert_eq!(game_code("iRacing"), "IR");
        assert_eq!(game_code("Dirt Rally 2.0"), "DR");
    }

    #[test]
    fn pretty_game_slug_reads_as_name() {
        assert_eq!(pretty_game_slug("assetto_corsa"), "Assetto Corsa");
        assert_eq!(pretty_game_slug("dirt_rally_2"), "Dirt Rally 2");
    }

    #[test]
    fn user_text_never_says_daemon() {
        assert_eq!(
            user_text("Daemon not running — showing Simulated data"),
            "Background service not running — showing Live data unavailable"
        );
        assert_eq!(
            user_text("the daemon keeps running"),
            "the background service keeps running"
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
        let off = led_preview(LedPattern::Off, 100);
        assert_eq!(off.len(), 8);
        assert!(off
            .iter()
            .all(|c| *c == Color::from_rgb_u8(0x1D, 0x25, 0x33)));
        let solid = led_preview(LedPattern::Solid, 100);
        assert!(solid
            .iter()
            .all(|c| *c == Color::from_rgb_u8(0x2F, 0x6B, 0xFF)));
        let dim = led_preview(LedPattern::Solid, 0);
        assert!(dim
            .iter()
            .all(|c| *c == Color::from_rgb_u8(0x0D, 0x11, 0x18)));
    }
}
