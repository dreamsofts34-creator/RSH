//! RSH v2.0 — Phase 3 shell + components (REDESIGN-PLAN §3-§6).
//!
//! Binary driver for the Slint main window. Data flows through the real
//! `rsh_api::AppApi` trait behind `Arc<dyn AppApi>`: the live
//! [`crate::ipc_api::IpcApi`] when the Phase 2 daemon answers on its
//! IPC socket, the simulated [`crate::sim_api::SimApi`] otherwise
//! (see `live_probe.rs`). Steering slider moves push live to the
//! daemon through [`AppApi::set_tuning`] (L4): the cached stored
//! record supplies min/max mV + smoothing, the sliders supply
//! deadzone/curve, and Save persists the draft via
//! [`AppApi::save_draft`].
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
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rsh_api::{AppApi, AppEvent, TuningSettings};
use slint::{ComponentHandle, Timer, TimerMode};

use crate::curve::{crosshair_commands, curve_commands, curve_output, dashed_line_commands};
use crate::notifications::{sync_to_window, NotificationCenter};
use crate::sim_api::SimApi;

slint::include_modules!();

/// View-box geometry of the steering curve (see `curve.rs`).
const VX0: f32 = 30.0;
/// View-box geometry of the steering curve.
const VX1: f32 = 290.0;
/// View-box geometry of the steering curve.
const VY1: f32 = 190.0;

/// UI-side tuning state (deadzone / curve / preset).
///
/// This is deliberately separate from the `AppApi` handle: in Phase 3
/// the sliders edit local state and mark the profile unsaved; pushing
/// the values into the engine lands in Phase 4.
struct Tuning {
    /// Centre deadzone, percent (0..10).
    deadzone_pct: f32,
    /// Response-curve exponent (1.0..3.0).
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
        let exponent = settings.curve.clamp(1.0, 3.0) as f32;
        let preset = match (deadzone_pct, exponent) {
            (3.0, 1.0) => 0,
            (2.0, 1.4) => 1,
            (1.0, 2.2) => 2,
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
    /// caps the curve at 2.0 while the UI slider runs to 3.0, so the
    /// exponent is clamped here (the drawn curve still uses the
    /// unclamped UI value).
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
        self.exponent = exp.clamp(1.0, 3.0);
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
                self.exponent = 2.2;
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

/// Push the current API + tuning state into the window.
// Casts are safe here: snapshot values are bounded (0..100, ±180°),
// so no truncation can occur in practice.
#[allow(clippy::cast_possible_truncation)]
fn push_frame(
    win: &MainWindow,
    api: &Arc<dyn AppApi>,
    tuning: &Tuning,
    unsaved: bool,
    live: bool,
    tick: u64,
) {
    let conn = api.connection();
    let snap = api.snapshot();
    let deadzone = tuning.deadzone_pct;
    let exponent = tuning.exponent;
    let steer_deg = snap.steering_deg as f32;
    let input_pct = (steer_deg.abs() / 180.0 * 100.0).clamp(0.0, 100.0);
    let output_pct = curve_output(input_pct, deadzone, exponent);
    let mx = VX0 + input_pct / 100.0 * (VX1 - VX0);
    let my = VY1 - output_pct / 100.0 * (VY1 - crate::curve::VY0);

    win.set_connected(conn.label == "Connected");
    win.set_conn_label(conn.label.into());
    // Live mode has no latency in the API — show transport + address.
    // Sim mode keeps the preview's "Wi-Fi · 4 ms" label.
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
        "Wi-Fi · 4 ms".to_string()
    };
    win.set_conn_sub(sub.into());
    win.set_send_to_game(api.output_enabled());
    win.set_steer_angle(steer_deg);
    win.set_steer_text(format!("{steer_deg:+.0}°").into());
    win.set_throttle((snap.throttle * 100.0) as f32);
    win.set_brake((snap.brake * 100.0) as f32);
    win.set_clutch((snap.clutch.unwrap_or(0.0) * 100.0) as f32);
    win.set_handbrake((snap.brake * 100.0 * 0.6) as f32);
    win.set_input_pct(input_pct);
    win.set_output_pct(output_pct);
    win.set_marker_vx(mx);
    win.set_marker_vy(my);
    win.set_dashed_line(dashed_line_commands().into());
    win.set_crosshair(crosshair_commands(mx, my).into());
    win.set_curve_commands(curve_commands(deadzone, exponent).into());
    win.set_deadzone(deadzone / 10.0);
    win.set_deadzone_text(format!("{deadzone:.0}%").into());
    win.set_curve_val((exponent - 1.0) / 2.0);
    win.set_curve_text(format!("{exponent:.2}").into());
    win.set_preset(tuning.preset);
    win.set_unsaved(unsaved);
    win.set_pressed_index((tick / 40 % 12) as i32);
}

/// Shared frame context for callbacks: the API handle, tuning state,
/// the unsaved flag, and the notification center.
struct Ctx {
    api: Arc<dyn AppApi>,
    tuning: Rc<RefCell<Tuning>>,
    /// Cached stored steering record (L4): supplies min/max mV,
    /// smoothing and extension fields for each slider push.
    steering: Rc<RefCell<TuningSettings>>,
    unsaved: Rc<Cell<bool>>,
    live: bool,
    notifications: Rc<RefCell<NotificationCenter>>,
}

impl Ctx {
    fn refresh(&self, win: &MainWindow, tick: u64) {
        let tuning = self.tuning.borrow();
        push_frame(win, &self.api, &tuning, self.unsaved.get(), self.live, tick);
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
        match self.api.set_tuning(&settings) {
            Ok(stored) => {
                *self.steering.borrow_mut() = stored;
                self.unsaved.set(true);
            }
            Err(e) => eprintln!("RSH-UI set_tuning failed: {} ({})", e.code, e.title),
        }
    }
}

/// Wire all Slint callbacks to the [`AppApi`] implementation.
fn wire_callbacks(win: &MainWindow, ctx: &Rc<Ctx>) {
    wire_tuning(win, ctx);
    wire_actions(win, ctx);
    wire_keyboard(win);
    wire_notifications(win, ctx);
    wire_setup(win);
}

/// Wire the setup-wizard callbacks (compare-fix).
///
/// Back/Next move through the wizard steps (0..=3); Next on the last
/// step and Skip leave setup and show the main pages. Pairing,
/// connect, calibrate and game search are stubs until the Pairing
/// Manager / `AppApi` surface lands — they log, they never pretend to
/// have paired anything.
fn wire_setup(win: &MainWindow) {
    {
        let win = win.as_weak();
        win.unwrap().on_setup_back(move || {
            if let Some(w) = win.upgrade() {
                w.set_setup_step((w.get_setup_step() - 1).max(0));
            }
        });
    }
    {
        let win = win.as_weak();
        win.unwrap().on_setup_next(move || {
            if let Some(w) = win.upgrade() {
                if w.get_setup_step() >= 3 {
                    w.set_setup_mode(false);
                    eprintln!("RSH-UI setup finished (wizard closed)");
                } else {
                    w.set_setup_step(w.get_setup_step() + 1);
                }
            }
        });
    }
    {
        let win = win.as_weak();
        win.unwrap().on_setup_skip(move || {
            if let Some(w) = win.upgrade() {
                w.set_setup_mode(false);
                eprintln!("RSH-UI setup skipped (wizard closed)");
            }
        });
    }
    win.on_setup_begin_pairing(|| {
        eprintln!("RSH-UI setup begin-pairing (Pairing Manager lands in Phase 6)");
    });
    win.on_setup_use_usb(|| {
        eprintln!("RSH-UI setup use-usb (transport switch lands in Phase 6)");
    });
    win.on_setup_retry_pairing(|| {
        eprintln!("RSH-UI setup retry-pairing (Pairing Manager lands in Phase 6)");
    });
    win.on_setup_reset_pairing(|| {
        eprintln!("RSH-UI setup reset-pairing (Pairing Manager lands in Phase 6)");
    });
    win.on_setup_connect(|t| {
        eprintln!("RSH-UI setup connect {t} (transport switch lands in Phase 6)");
    });
    win.on_setup_calibrate(|a| {
        eprintln!("RSH-UI setup calibrate {a} (calibration push lands in Phase 6)");
    });
    win.on_setup_find_games(|| {
        eprintln!("RSH-UI setup find-games (game launcher lands in Phase 6)");
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
            ctx.tuning.borrow_mut().set_exponent(1.0 + v * 2.0);
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
/// - `keyboard-escape`: closes the notifications drawer when it is open
///   (it is an overlay child of the root keyboard scope, so Esc reaches
///   here from anywhere); the diagnostics drawer and profile popover land
///   in Phase 5 and close themselves via their own `FocusScope` Esc handlers.
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
                if w.get_notif_open() {
                    w.set_notif_open(false);
                    eprintln!("RSH-UI keyboard-escape closed notifications drawer");
                } else {
                    eprintln!("RSH-UI keyboard-escape (no drawer or popover open)");
                }
            }
        });
    }
    // Focus the root keyboard scope so keys work before the first Tab.
    win.invoke_focus_keyboard();
}

/// Wire the remaining callbacks (send-to-game, tiles, games, settings).
fn wire_actions(win: &MainWindow, ctx: &Rc<Ctx>) {
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap()
            .on_reconnect(move || match ctx.api.connect(rsh_api::ApiTransport::Wifi) {
                Ok(info) => {
                    eprintln!("RSH-UI reconnect ok: {}", info.label);
                    if let Some(w) = win.upgrade() {
                        ctx.refresh(&w, 0);
                    }
                }
                Err(e) => eprintln!("RSH-UI reconnect failed: {} ({})", e.code, e.title),
            });
    }
    win.on_set_centre(|| {
        eprintln!("RSH-UI set-centre (simulated — engine push lands in Phase 4)");
    });
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_save_profile(move || {
            match ctx.api.save_draft() {
                Ok(saved) => {
                    eprintln!("RSH-UI save-profile saved={saved}");
                    ctx.unsaved.set(false);
                }
                Err(e) => {
                    eprintln!("RSH-UI save-profile failed: {} ({})", e.code, e.title);
                }
            }
            if let Some(w) = win.upgrade() {
                ctx.refresh(&w, 0);
            }
        });
    }
    win.on_profile_clicked(|| {
        eprintln!("RSH-UI profile popover lands in Phase 5");
    });
    {
        let win = win.as_weak();
        let ctx = ctx.clone();
        win.unwrap().on_toggled_send(move || {
            let next = !ctx.api.output_enabled();
            ctx.api.set_output_enabled(next);
            eprintln!("RSH-UI send-to-game={next}");
            if let Some(w) = win.upgrade() {
                w.set_send_to_game(next);
            }
        });
    }
    {
        let win = win.as_weak();
        win.unwrap().on_tile_selected(move |i| {
            if let Some(w) = win.upgrade() {
                w.set_selected_index(i);
            }
            eprintln!("RSH-UI button tile {i} selected (learning lands in Phase 4)");
        });
    }
    win.on_launch_game(|g| {
        eprintln!("RSH-UI launch-game {g} (game launcher lands in Phase 4)");
    });
    win.on_pedal_action(|a| {
        eprintln!("RSH-UI pedal-action {a} (calibration push lands in Phase 4)");
    });
    win.on_extras_action(|a| {
        eprintln!("RSH-UI extras-action {a} (Extras editing lands in Phase 5)");
    });
    win.on_settings_action(|a| {
        eprintln!("RSH-UI settings-action {a} (Settings editing lands in Phase 5)");
    });
    win.on_action_chip(|a| {
        eprintln!("RSH-UI action-chip {a} (button mapping lands in Phase 4)");
    });
    win.on_open_diagnostics(|| {
        eprintln!("RSH-UI diagnostics drawer lands in Phase 5");
    });
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

fn main() -> Result<(), slint::PlatformError> {
    let t0 = Instant::now();
    let args = parse_args();

    // Data source: live daemon API when it answers, simulated otherwise.
    let live_api = live_probe::try_live_api(args.data_dir.as_deref());
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
            "simulated data until Phase 2 daemon runs"
        }
    );

    // L4 startup: seed the slider state from the stored steering
    // record (live daemon or sim) and the unsaved dot from the draft.
    let stored_steering = api.get_tuning("steering");
    let initial_unsaved = api.draft_status().is_some_and(|s| s.dirty);
    let ctx = Rc::new(Ctx {
        api: api.clone(),
        tuning: Rc::new(RefCell::new(Tuning::from_settings(&stored_steering))),
        steering: Rc::new(RefCell::new(stored_steering)),
        unsaved: Rc::new(Cell::new(initial_unsaved)),
        live,
        notifications: Rc::new(RefCell::new(NotificationCenter::new())),
    });

    let win = MainWindow::new()?;
    win.set_page(args.page);
    win.set_setup_mode(args.setup);
    win.set_app_version(api.version().into());
    win.set_daemon_live(live);
    win.set_gear("N".into());
    win.set_notif_open(false);
    sync_to_window(&ctx.notifications.borrow(), &win);
    push_frame(&win, &api, &ctx.tuning.borrow(), initial_unsaved, live, 0);
    eprintln!("RSH-UI window shown in {} ms", t0.elapsed().as_millis());

    wire_callbacks(&win, &ctx);

    // ---- frame timer (20 Hz) + --quit-after
    if !args.static_mode {
        let win = win.as_weak();
        let ctx = ctx.clone();
        let sim_api = sim_api.clone();
        let live_api = live_api.clone();
        let quit_after = args.quit_after;
        let tick = Rc::new(Cell::new(0_u64));
        let timer = Timer::default();
        timer.start(TimerMode::Repeated, Duration::from_millis(50), move || {
            let t = tick.get();
            tick.set(t + 1);
            if let Some(sim) = &sim_api {
                sim.advance(0.05);
            }
            if let Some(live) = &live_api {
                for ev in live.poll_events() {
                    log_event(&ev);
                    // Feed Notification events into the bell / drawer
                    // center (log kept above); other events are ignored
                    // by the center.
                    let pushed = ctx.notifications.borrow_mut().push_event(&ev);
                    if pushed {
                        if let Some(w) = win.upgrade() {
                            sync_to_window(&ctx.notifications.borrow(), &w);
                        }
                    }
                }
            }
            if let Some(w) = win.upgrade() {
                ctx.refresh(&w, t);
            }
            if let Some(secs) = quit_after {
                if t0.elapsed() >= Duration::from_secs(secs) {
                    slint::quit_event_loop().ok();
                }
            }
        });
        // Keep the timer alive for the run.
        std::mem::forget(timer);
    } else if let Some(secs) = args.quit_after {
        let timer = Timer::default();
        timer.start(TimerMode::SingleShot, Duration::from_secs(secs), || {
            slint::quit_event_loop().ok();
        });
        std::mem::forget(timer);
    }

    win.run()
}
