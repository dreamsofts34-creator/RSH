// Copyright (c) 2026 RSH contributors. MIT licensed.
//! `rsh-daemon`: the always-on RSH daemon (REDESIGN-PLAN §10,
//! LOGIC-FIX-PLAN §2).
//!
//! Owns the engine + transport + output + tray + auto profile
//! switch. No web engine. Runs headless; the Slint UI (Phase 3)
//! attaches over local IPC (`rsh-ipc`). The tray icon is created
//! on every launch (A-09), not only `--tray`.
//!
//! Threads:
//! * main thread — Slint event loop (tray only; no window yet —
//!   the main window lands in Phase 3).
//! * `rsh-daemon-rt` — tokio multi-thread runtime: IPC server +
//!   manager loop.
//! * `rsh-engine-200hz` — the 200 Hz engine on its own
//!   thread/runtime (A-02, owned by `rsh-app-core`).
//! * supervisor thread — forwarder/activator network runtime
//!   (owned by `rsh-app-core`).
//!
//! Usage: `rsh-daemon [--no-tray] [--no-connect] [--check]
//! [--data-dir DIR] [--verbose]`

// The Slint-generated tray module (OUT_DIR/tray.rs) has no doc
// comments; the workspace `missing_docs` lint would flag every
// generated item. The daemon's own code is documented; the
// generated code cannot be.
#![allow(missing_docs)]
// No console window in release builds (v1.0.0 lesson): the daemon
// is a tray app — a console-subsystem binary would flash a terminal
// on every launch and on every autostart login.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod daemon_api;
mod rejoin;
mod single_instance;
mod tray_autostart;
mod tray_geometry;
mod tray_popup;

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rsh_api::{AppApi, AppEvent, ConnEvent, ConnectionError, ManagerAction};
use rsh_app_core::diagnostics::{append_startup_log_both, install_panic_hook, log_fatal};
use rsh_app_core::state::{AppStateCore, BindingAppAction, TransportKind};
#[cfg(unix)]
use rsh_ipc::serve_listener;
use rsh_ipc::{IpcServer, socket_path};
use slint::ComponentHandle;
use tokio::sync::broadcast;

use daemon_api::{DaemonApi, connection_changed_event};
use rejoin::{RSH_SSID, rejoin_wifi};
use single_instance::{AcquireOutcome, acquire};
use tray_popup::{
    POPUP_ACTIVATION_TIMEOUT, PopupDismissPoll, PopupGame, TrayPopupData, TrayToggleGuard,
    anchor_popup, next_profile, popup_is_light,
};

slint::include_modules!();

/// Tray actions queued by the Slint callbacks (which run on the
/// event-loop thread) and drained by the manager loop.
#[derive(Clone, Debug)]
enum TrayAction {
    /// "Open RSH": spawn the Slint UI (`rsh-ui` beside the daemon).
    OpenMain,
    /// "Send to game" toggled from the tray menu / popup switch.
    ToggleSend,
    /// A profile was picked from the tray menu (by display name).
    ProfileSelected(String),
    /// Cycle to the next profile (popup profile row).
    CycleProfile,
    /// A game launch was picked (menu: display name; popup: game id).
    LaunchGame(String),
    /// "Restart wheel" from the tray menu / popup footer.
    RestartWheel,
    /// "Run setup" from the tray menu: spawn the UI on the Setup
    /// wizard (M-5, v2.0.2).
    OpenSetup,
    /// "Settings" from the tray menu: spawn the UI on Settings.
    OpenSettings,
}

/// CLI arguments.
// Four plain flags + one option: a struct of bools is the honest
// shape for a hand-rolled parser (no clap in this crate).
#[allow(clippy::struct_excessive_bools)]
struct Args {
    /// Do not create the tray icon (headless/CI).
    no_tray: bool,
    /// Do not start the engine / connect on startup.
    no_connect: bool,
    /// Smoke check: start, verify IPC answers, exit 0.
    check: bool,
    /// Override the data dir (tests).
    data_dir: Option<PathBuf>,
    /// Also log to stderr.
    verbose: bool,
}

fn parse_args() -> Args {
    let mut args = Args {
        no_tray: false,
        no_connect: false,
        check: false,
        data_dir: None,
        verbose: false,
    };
    let mut rest = std::env::args().skip(1);
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--no-tray" => args.no_tray = true,
            "--no-connect" => args.no_connect = true,
            "--check" => args.check = true,
            "--verbose" => args.verbose = true,
            "--data-dir" => args.data_dir = rest.next().map(PathBuf::from),
            "--help" | "-h" => {
                eprintln!(
                    "Usage: rsh-daemon [--no-tray] [--no-connect] [--check] [--data-dir DIR] [--verbose]"
                );
                std::process::exit(0);
            }
            other => {
                // Log first: under `windows_subsystem = "windows"`
                // stderr is invisible, so an eprintln-only exit here
                // would leave no trace of why the daemon died.
                let _ = log_fatal(&format!("rsh-daemon: unknown argument: {other}"));
                std::process::exit(2);
            }
        }
    }
    args
}

/// Log one line: to the diagnostics log file (both locations,
/// redacted) and to stderr when `--verbose`.
fn log_line(verbose: bool, message: &str) {
    if verbose {
        eprintln!("[rsh-daemon] {message}");
    }
    let _ = append_startup_log_both(message);
}

/// The manager loop (tokio task): drives the L1 Connection Manager
/// from live `AppStateCore` observations, drains the pairing log
/// into the diagnostics file, runs the heartbeat watchdog, fires
/// backoff retries, and publishes `AppEvent`s.
// One loop: observations → manager events → published AppEvents is
// a single pipeline; splitting it would scatter the pairing-log
// drain, the watchdog, and the retry timer across tasks.
#[allow(clippy::too_many_lines)]
async fn manager_loop(
    api: Arc<DaemonApi>,
    event_tx: broadcast::Sender<AppEvent>,
    tray_actions: Arc<Mutex<Vec<TrayAction>>>,
    shutdown: Arc<AtomicBool>,
    verbose: bool,
) {
    let mut last_gate = false;
    let mut last_error: Option<ConnectionError> = None;
    let mut last_window_open = false;
    let mut last_active = false;
    // Pairing completion tracking (v2.0.1 hotfix): the wizard's Pair
    // step only completes on `PairingFinished`, and nothing in the
    // daemon ever published it — so the wizard could never advance
    // past Pair except via Skip. A pairing is in flight while its
    // window is open; reaching Active with a window open — or the
    // first Active since daemon start (a resumed paired session) —
    // finishes it successfully. A window closing without Active
    // finishes it as failed.
    let mut pairing_in_flight = false;
    let mut pending_rejoin_password: Option<String> = None;
    let mut ticker = tokio::time::interval(Duration::from_millis(250));
    loop {
        ticker.tick().await;
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
        // Drain the pairing log into the diagnostics file (DS-32:
        // drain-once — each line is logged exactly once, no
        // re-clone of the whole buffer and no index bookkeeping).
        for line in api.state.take_pairing_log_lines() {
            log_line(verbose, &line);
        }
        // Drain the manager's own transition lines.
        for line in api.take_manager_log_lines() {
            log_line(verbose, &line);
        }
        // Drain tray actions.
        let actions: Vec<TrayAction> = {
            let mut guard = tray_actions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            std::mem::take(&mut *guard)
        };
        for action in actions {
            match action {
                TrayAction::OpenMain => {
                    if open_ui(None, verbose) {
                        log_line(verbose, "tray: Open RSH — restored the running UI window");
                    } else {
                        log_line(verbose, "tray: Open RSH — spawned rsh-ui");
                    }
                }
                TrayAction::ToggleSend => {
                    let enabled = !api.output_enabled();
                    api.set_output_enabled(enabled);
                    log_line(verbose, &format!("tray: Send to game = {enabled}"));
                }
                TrayAction::ProfileSelected(name) => match resolve_profile_id(&api, &name) {
                    Some(id) => match api.state.activate_profile(&id) {
                        Ok(summary) => log_line(
                            verbose,
                            &format!("tray: profile activated: {}", summary.name),
                        ),
                        Err(error) => log_line(
                            verbose,
                            &format!("tray: profile activation failed: {error}"),
                        ),
                    },
                    None => log_line(
                        verbose,
                        &format!("tray: unknown profile '{name}' — ignored"),
                    ),
                },
                TrayAction::CycleProfile => {
                    let profiles = api.state.list_profiles();
                    let current = api.state.active_profile().map(|summary| summary.id.clone());
                    match next_profile(&profiles, current.as_deref()) {
                        Some(next) => match api.state.activate_profile(&next.id) {
                            Ok(summary) => log_line(
                                verbose,
                                &format!("tray: profile cycled to: {}", summary.name),
                            ),
                            Err(error) => {
                                log_line(verbose, &format!("tray: profile cycle failed: {error}"));
                            }
                        },
                        None => log_line(verbose, "tray: only one profile — nothing to cycle to"),
                    }
                }
                TrayAction::LaunchGame(key) => match resolve_game_id(&api, &key) {
                    Some(id) => match api.state.launch_game(&id) {
                        Ok(game) => {
                            log_line(verbose, &format!("tray: game launched: {}", game.name));
                        }
                        Err(error) => {
                            log_line(verbose, &format!("tray: game launch failed: {error}"));
                        }
                    },
                    None => log_line(verbose, &format!("tray: unknown game '{key}' — ignored")),
                },
                TrayAction::RestartWheel => {
                    // DS-02 (P0): `restart_wheel` is a sync H5 command
                    // built on `block_on_control` (fresh runtime +
                    // `block_on`) — calling it here, inside this tokio
                    // task, panics ("Cannot start a runtime from
                    // within a runtime") and kills the manager loop
                    // silently (proven with a scratch tokio probe).
                    // Run it on the blocking pool instead, where
                    // there is no runtime context; the loop awaits
                    // the outcome and logs it like every other
                    // tray action.
                    let state = Arc::clone(&api.state);
                    match tokio::task::spawn_blocking(move || state.restart_wheel()).await {
                        Ok(Ok(())) => log_line(verbose, "tray: wheel restart requested"),
                        Ok(Err(error)) => {
                            log_line(verbose, &format!("tray: wheel restart failed: {error}"));
                        }
                        Err(error) => {
                            log_line(
                                verbose,
                                &format!("tray: wheel restart task failed: {error}"),
                            );
                        }
                    }
                }
                TrayAction::OpenSetup => {
                    if open_ui(Some("setup"), verbose) {
                        log_line(
                            verbose,
                            "tray: Run setup — restored the running UI window (setup page needs the UI's own single-instance hook)",
                        );
                    } else {
                        log_line(verbose, "tray: Run setup — spawned rsh-ui on Setup");
                    }
                }
                TrayAction::OpenSettings => {
                    if open_ui(Some("settings"), verbose) {
                        log_line(
                            verbose,
                            "tray: Settings — restored the running UI window (page switch needs the UI's own single-instance hook)",
                        );
                    } else {
                        log_line(verbose, "tray: Settings — spawned rsh-ui on Settings");
                    }
                }
            }
        }
        // Bound-button app actions (v2.0.2, Round 2B): a wheel
        // button bound to profile_next / profile_previous queues
        // here; the loop runs the cycle (activation saves + pushes
        // the new profile's plan — that work never happens on the
        // 200 Hz engine tick). Game-input bindings are already
        // applied inline, in the tick itself.
        for action in api.state.drain_binding_actions() {
            let forward = matches!(action, BindingAppAction::ProfileNext);
            match api.state.cycle_profile(forward) {
                Ok(summary) => log_line(
                    verbose,
                    &format!("binding: profile cycled to: {}", summary.name),
                ),
                Err(error) => log_line(verbose, &format!("binding: profile cycle failed: {error}")),
            }
        }
        // Observations → manager events.
        let gate = api.state.session_gate_open();
        if gate && !last_gate {
            let (action, changed) = api.manager_event(ConnEvent::CapsOk);
            if changed {
                publish(&event_tx, connection_changed_event(&api));
            }
            let _ = action;
        }
        last_gate = gate;
        let error = api.state.session_last_error_typed();
        if error != last_error {
            if let Some(reason) = error {
                let (action, changed) = api.manager_event(ConnEvent::ActivationFailed(reason));
                if changed {
                    publish(&event_tx, connection_changed_event(&api));
                }
                if matches!(action, ManagerAction::NeedsPairing(_)) {
                    publish(
                        &event_tx,
                        AppEvent::Notification {
                            kind: "PairingNeeded".to_string(),
                            title: "Pairing needed".to_string(),
                            body: reason.hint().to_string(),
                        },
                    );
                }
            }
            last_error = error;
        }
        let window_open = api.state.pairing_status_peek().seconds_left > 0;
        if window_open && !last_window_open {
            pairing_in_flight = true;
            let (action, changed) = api.manager_event(ConnEvent::HelloReceived {
                unpaired: true,
                window_open: true,
            });
            if changed {
                publish(&event_tx, connection_changed_event(&api));
            }
            if matches!(action, ManagerAction::StartAuthentication) {
                api.manager_event(ConnEvent::Authenticating);
            }
            publish(
                &event_tx,
                AppEvent::PairingWindow {
                    open: true,
                    seconds_left: api.state.pairing_status_peek().seconds_left,
                    board_mac: api.state.pairing_status_peek().board_mac,
                },
            );
        }
        if !window_open && last_window_open && pairing_in_flight && !api.manager_state().is_active()
        {
            publish(
                &event_tx,
                AppEvent::PairingFinished {
                    ok: false,
                    error: api.state.session_last_error_typed(),
                },
            );
            log_line(
                verbose,
                "pairing: finished — failed (window closed without Active)",
            );
            pairing_in_flight = false;
        }
        last_window_open = window_open;
        // Pairing rejoin (Phase 6): stash the Wi-Fi password while the
        // pairing window is open — it is cleared on activation — and
        // rejoin the board AP once when the session reaches Active.
        // The password never reaches a log line (see rejoin.rs).
        if window_open && let Some(password) = api.state.pairing_status_peek().password {
            pending_rejoin_password = Some(password);
        }
        let active_now = api.manager_state().is_active();
        if active_now && !last_active {
            // DS-34: publish the success ONLY when a pairing was
            // actually in flight. The old `!first_active_seen`
            // arm published `PairingFinished { ok: true }` on the
            // first Active since daemon start even when no pairing
            // ever happened (a restart with an already-paired
            // board auto-resuming) — a wizard open at that moment
            // could advance past Pair on a phantom success.
            if pairing_in_flight {
                publish(
                    &event_tx,
                    AppEvent::PairingFinished {
                        ok: true,
                        error: None,
                    },
                );
                log_line(verbose, "pairing: finished — board paired (session Active)");
            }
            pairing_in_flight = false;
            if let Some(password) = pending_rejoin_password.take() {
                // DS-15 (F14): `rejoin_wifi` runs blocking `netsh`
                // calls — never on the manager-loop task (a hung
                // Wi-Fi stack would stall the 250 ms ticker:
                // watchdog, retries, tray drain, pairing events).
                // A detached task runs it on the blocking pool
                // (bounded by a hard timeout inside `rejoin_wifi`)
                // and logs the outcome; the loop never waits on it.
                tokio::spawn(async move {
                    match tokio::task::spawn_blocking(move || rejoin_wifi(RSH_SSID, &password))
                        .await
                    {
                        Ok(Ok(())) => {
                            log_line(verbose, "pairing: rejoined RSH Wi-Fi after pairing");
                        }
                        Ok(Err(error)) => {
                            log_line(verbose, &format!("pairing: Wi-Fi rejoin failed: {error}"));
                        }
                        Err(error) => {
                            log_line(
                                verbose,
                                &format!("pairing: Wi-Fi rejoin task failed: {error}"),
                            );
                        }
                    }
                });
            }
        }
        if !window_open && !active_now {
            pending_rejoin_password = None;
        }
        last_active = active_now;
        // Heartbeat watchdog (C-04): 2 s silence in Active →
        // rebuild + re-activate. F20: the rebuild is capped inside
        // `check_watchdog` (5 frameless rebuilds latch
        // `Error(WatchdogStall)`, which returns `StateChanged`,
        // not `RebuildTransport`, so this branch simply does not
        // run for a latched manager) and the tick is fed real frame
        // freshness by `DaemonApi::watchdog_tick` — a healthy
        // session never reaches this branch at all.
        if gate && api.watchdog_tick() == ManagerAction::RebuildTransport {
            log_line(
                verbose,
                "connection: heartbeat watchdog fired — rebuilding transport",
            );
            let _ = api.state.disconnect_transport();
            let _ = api.state.connect_transport(TransportKind::Udp);
            publish(&event_tx, connection_changed_event(&api));
        }
        // Backoff retry timer.
        if let Some(due) = api.manager_retry_due_at()
            && Instant::now() >= due
        {
            let (action, changed) = api.manager_event(ConnEvent::RetryTimer);
            if changed {
                publish(&event_tx, connection_changed_event(&api));
            }
            match action {
                ManagerAction::RequestHello | ManagerAction::Reactivate => {
                    let _ = api.state.activate_session();
                }
                _ => {}
            }
        }
    }
}

/// Publish one event (a missing subscriber is fine).
fn publish(event_tx: &broadcast::Sender<AppEvent>, event: AppEvent) {
    let _ = event_tx.send(event);
}

/// Spawn the Slint UI (`rsh-ui` beside this daemon exe), optionally
/// on a page (`--page settings`). Fire-and-forget: the UI is its own
/// process (Phase 3 owns its lifecycle); a missing binary is logged,
/// never fatal to the daemon.
///
/// Lifetime (A11/A12, v2.0.2): the spawned UI is deliberately NOT
/// tied to the daemon — the `Child` handle is dropped without
/// waiting and no job object / kill-on-drop exists, so the daemon
/// keeps running after the UI exits (and vice versa: the UI spawns
/// the daemon the same way, see `rsh-ui/src/live_probe.rs`). The
/// daemon therefore survives an X-close of the UI; what it must
/// NOT do is spawn a second UI while one is already running — that
/// is [`open_ui`]'s job.
///
/// Detached spawn (F8, v2.0.3): stdio is nulled and, on Windows,
/// the child is detached (`DETACHED_PROCESS |
/// CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW`) — the same bug
/// class as the NSIS `Exec` hang fixed in v2.0.2 (b8b9fd4), where a
/// spawned child inheriting the parent's stdio pipes could block
/// the parent forever.
fn spawn_ui(page: Option<&str>, verbose: bool) {
    let Some(exe) = std::env::current_exe().ok().and_then(|exe| {
        exe.parent().map(|dir| {
            dir.join(if cfg!(windows) {
                "rsh-ui.exe"
            } else {
                "rsh-ui"
            })
        })
    }) else {
        eprintln!("rsh-daemon: cannot locate rsh-ui (no current exe)");
        return;
    };
    let mut command = std::process::Command::new(&exe);
    if let Some(page) = page {
        command.arg("--page").arg(page);
    }
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows::Win32::System::Threading::{
            CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, DETACHED_PROCESS,
        };
        command
            .creation_flags(DETACHED_PROCESS.0 | CREATE_NEW_PROCESS_GROUP.0 | CREATE_NO_WINDOW.0);
    }
    match command.spawn() {
        Ok(child) => log_line(verbose, &format!("daemon: spawned UI (pid {})", child.id())),
        Err(error) => {
            eprintln!("rsh-daemon: cannot start UI ({}): {error}", exe.display());
        }
    }
}

/// Open the main UI (A11/A12, v2.0.2): restore the already-running
/// UI window when one exists, and only spawn a fresh UI when none
/// is running.
///
/// Root cause of the v2.0.1 report: every tray "Open RSH" called
/// [`spawn_ui`] unconditionally, so each open started a brand-new
/// `rsh-ui` process — full startup, fresh IPC probe, fresh load —
/// even when a UI was already running (hidden to the tray, or just
/// minimised). That is the "fresh load" the user saw, and the
/// fresh process's probe window is also where a fake-data flash
/// can come from: a new UI starts on its `SimApi` fallback until
/// its background probe answers, while the restored UI is already
/// live-connected to this daemon and shows real state.
///
/// Returns `true` when an already-running UI was restored (or was
/// already starting — no second copy spawned), `false` when a fresh
/// UI was spawned (or the spawn was attempted). A `page` request
/// (`--page settings`) is forwarded to a running UI via a second
/// `rsh-ui` instance: the UI's single-instance hook (pid file +
/// `show-ui.request`, polled by the frame timer) makes the second
/// instance forward the page to the first and exit, so the running
/// UI switches page instead of staying put (M-5, v2.0.2).
fn open_ui(page: Option<&str>, verbose: bool) -> bool {
    // 1. A UI window exists → restore/show it (instant, live). A page
    // request is forwarded as well (see above) — restore alone would
    // leave the window on its old page.
    if tray_geometry::restore_ui_window() {
        if let Some(page) = page {
            log_line(
                verbose,
                &format!("tray: forwarding page '{page}' to running UI"),
            );
            spawn_ui(Some(page), verbose);
        }
        return true;
    }
    // 2. A UI process exists but has no window yet (still starting
    // up) — spawning a second UI would duplicate it, so leave it
    // alone; it appears on its own. On Linux there is no window
    // raise, so a running UI is likewise left alone here (its
    // taskbar/dock entry brings it forward). Exception: a page
    // request is forwarded via a second instance (single-instance
    // hook), which exits after forwarding — no duplicate window.
    if let Some(pid) = tray_geometry::ui_process_id() {
        if let Some(page) = page {
            log_line(
                verbose,
                &format!(
                    "tray: UI already running (pid {pid}) — forwarding page '{page}' via second instance"
                ),
            );
            spawn_ui(Some(page), verbose);
        } else {
            log_line(
                verbose,
                &format!("tray: UI already running (pid {pid}) — not spawning a second copy"),
            );
        }
        return true;
    }
    // 3. No UI at all → launch it exactly like a double-click.
    spawn_ui(page, verbose);
    false
}

/// Resolve a tray-menu profile pick (display name) to a profile id.
fn resolve_profile_id(api: &DaemonApi, name: &str) -> Option<String> {
    api.state
        .list_profiles()
        .into_iter()
        .find(|summary| summary.name == name)
        .map(|summary| summary.id)
}

/// Resolve a game pick to a game id: the popup passes the id
/// already; the tray menu passes the display name.
fn resolve_game_id(api: &DaemonApi, key: &str) -> Option<String> {
    let games = api.state.list_games();
    games
        .iter()
        .find(|game| game.id == key)
        .or_else(|| games.iter().find(|game| game.name == key))
        .map(|game| game.id.clone())
}

/// Signal a fatal IPC bind failure (F2, v2.0.3): write the
/// machine-readable fatal status + the dual-location fatal log,
/// then ask the MAIN thread to quit cleanly (shutdown flag +
/// `slint::quit_event_loop()`) instead of `process::exit(1)` from
/// this background task. Exiting from here used to kill the process
/// mid-flight — the tray icon was never removed (`NIM_DELETE`) and
/// the single-instance lock never released — so every death left a
/// ghost icon and a stale lock behind. The main thread does the
/// teardown (tray drop, engine stop, lock release) after the event
/// loop returns; with no event loop running (headless park,
/// `--check`) the quit call is a harmless no-op and the main thread
/// exits on the shutdown flag instead.
///
/// F24: the signal also sets `ipc_fatal`, so `main` can exit(1)
/// AFTER that teardown — the `DaemonHung` path exits 1, and a fatal
/// bind must too (the rig's `gate-ipc-bind-failure` asserts it).
/// A clean SIGTERM/quit never sets it, so a clean shutdown still
/// exits 0. The exit-code decision lives in `main`, never in a
/// background task (F2's rule stands).
fn signal_ipc_fatal_shutdown(
    data_dir: &std::path::Path,
    detail: &str,
    shutdown: &Arc<AtomicBool>,
    ipc_fatal: &Arc<AtomicBool>,
) {
    write_daemon_status(data_dir, "fatal", "IpcBindFailed", detail);
    let _ = log_fatal(&format!("daemon: {detail}"));
    ipc_fatal.store(true, Ordering::Relaxed);
    shutdown.store(true, Ordering::Relaxed);
    let _ = slint::quit_event_loop();
}

/// Whether a Windows named-pipe create failure means "an instance
/// is already in use" (transient — a live peer holds the pipe) rather
/// than a real bind error. Log-proven (user log 2026-11-16, 1199
/// failures): after the UI connects, every further create fails
/// `Access is denied. (os error 5)`, and treating that as a bind
/// error killed the daemon ~5 s after every UI connect (v2.0.2 loop
/// killer). The raw OS codes are Win32 (`ERROR_ACCESS_DENIED` 5,
/// `ERROR_PIPE_BUSY` 231); this helper is only called on the Windows
/// path, so the raw-code arm never misfires on Linux.
// Only called on the Windows IPC path; kept available in test
// builds so the Linux suite can cover the classifier (F1).
#[cfg(any(windows, test))]
fn pipe_create_is_in_use(error: &std::io::Error) -> bool {
    use std::io::ErrorKind;
    matches!(
        error.kind(),
        ErrorKind::PermissionDenied | ErrorKind::AlreadyExists | ErrorKind::AddrInUse
    ) || matches!(error.raw_os_error(), Some(5 | 231))
}

/// Backoff between retries of an in-use pipe create (F1): 100 ms,
/// doubling, capped at 1 s. DS-14: the retry itself is now CAPPED —
/// an in-use pipe is a live peer, not a broken bind, but a squatted
/// pipe used to leave the daemon IPC-dead forever (one log line per
/// second, never fatal). After [`IN_USE_FATAL_AFTER`] of continuous
/// in-use failures the tracker reports fatal (see
/// [`IpcBindTracker::record_failure`]).
fn pipe_in_use_backoff(failures: u32) -> Duration {
    // 100 ms, 200 ms, 400 ms, 800 ms, then 1 s forever.
    let shift = failures.saturating_sub(1).min(4);
    Duration::from_millis((100u64 << shift).min(1000))
}

/// Total continuous in-use time after which the daemon gives up
/// (DS-14): ~90 s — long enough for any transient peer hold, short
/// enough that a squatted pipe becomes a VISIBLE fatal instead of
/// a silent IPC-dead loop.
const IN_USE_FATAL_AFTER: Duration = Duration::from_secs(90);

/// Outcome of recording one IPC bind/create failure (F1/F2).
enum IpcBindRetry {
    /// Try again after this delay.
    RetryAfter(Duration),
    /// Five consecutive real bind failures: fatal.
    Fatal,
}

/// Counts CONSECUTIVE IPC bind/create failures (F1/F2): five real
/// failures in a row are fatal; "pipe in use" failures never count
/// toward the five (a live peer holds the pipe — that is the v2.0.2
/// loop killer, where every post-connect create failed
/// `ERROR_ACCESS_DENIED` and the daemon killed itself ~5 s after the
/// UI connected); a success resets both counters.
struct IpcBindTracker {
    failures: u32,
    in_use_failures: u32,
    /// DS-14: when the current run of consecutive in-use failures
    /// started — `None` outside such a run. After
    /// [`IN_USE_FATAL_AFTER`] the run is fatal instead of looping
    /// forever.
    in_use_since: Option<Instant>,
}

impl IpcBindTracker {
    fn new() -> Self {
        Self {
            failures: 0,
            in_use_failures: 0,
            in_use_since: None,
        }
    }

    fn record_success(&mut self) {
        self.failures = 0;
        self.in_use_failures = 0;
        self.in_use_since = None;
    }

    fn record_failure(&mut self, in_use: bool) -> IpcBindRetry {
        if in_use {
            self.in_use_failures += 1;
            if self.in_use_since.is_none() {
                self.in_use_since = Some(Instant::now());
            }
            // DS-14: a squatted pipe (in-use forever) becomes a
            // visible fatal after ~90 s instead of looping 1/s
            // forever with no IPC and no exit.
            if self
                .in_use_since
                .is_some_and(|since| since.elapsed() >= IN_USE_FATAL_AFTER)
            {
                return IpcBindRetry::Fatal;
            }
            return IpcBindRetry::RetryAfter(pipe_in_use_backoff(self.in_use_failures));
        }
        self.failures += 1;
        if self.failures >= 5 {
            IpcBindRetry::Fatal
        } else {
            IpcBindRetry::RetryAfter(Duration::from_secs(1))
        }
    }
}

/// Serve the IPC listener forever (Unix socket on Linux, named
/// pipe on Windows).
#[cfg(unix)]
async fn serve_ipc(
    server: Arc<IpcServer>,
    data_dir: &std::path::Path,
    verbose: bool,
    shutdown: Arc<AtomicBool>,
    ipc_fatal: Arc<AtomicBool>,
) {
    let path = socket_path(data_dir);
    // A bind can fail transiently (the data dir is still being
    // created, a stale socket is being reaped) — retry a few times
    // instead of silently running with no IPC, which would leave
    // the UI on simulated data forever with no further trace.
    let mut tracker = IpcBindTracker::new();
    loop {
        match rsh_ipc::bind_unix_socket(&path) {
            Ok(listener) => {
                tracker.record_success();
                log_line(
                    verbose,
                    &format!("daemon: IPC listening on {}", path.display()),
                );
                // DS-36: a broken accept loop (`true`) is a VISIBLE
                // fatal like the bind path — the old code returned
                // silently here and the daemon ran on with no IPC.
                if serve_listener(server, listener).await {
                    signal_ipc_fatal_shutdown(
                        data_dir,
                        "daemon IPC accept loop failed 50 times in a row — the listener is broken, the daemon cannot serve the UI",
                        &shutdown,
                        &ipc_fatal,
                    );
                }
                return;
            }
            Err(error) => match tracker.record_failure(false) {
                IpcBindRetry::RetryAfter(delay) => {
                    log_line(
                        verbose,
                        &format!(
                            "daemon: IPC bind failed (attempt {}/5): {error}",
                            tracker.failures
                        ),
                    );
                    tokio::time::sleep(delay).await;
                }
                IpcBindRetry::Fatal => {
                    // M8 (v2.0.2): five failed binds are a VISIBLE
                    // fatal state, not a quiet headless run — but
                    // (F2) the fatal is signalled to the main thread
                    // instead of `process::exit(1)` from this task,
                    // so the tray is removed and the lock released
                    // cleanly before the process ends.
                    let detail = format!(
                        "daemon IPC bind failed after 5 attempts (socket was {}) — the daemon cannot serve the UI",
                        path.display()
                    );
                    signal_ipc_fatal_shutdown(data_dir, &detail, &shutdown, &ipc_fatal);
                    return;
                }
            },
        }
    }
}

/// Create ONE Windows named-pipe listening instance, retrying
/// transient failures (F1). Returns `None` only when a fatal bind
/// failure was signalled (the tracker hit 5) or shutdown was
/// requested while waiting — the caller then returns.
#[cfg(windows)]
async fn create_pipe_listener(
    first_create: bool,
    data_dir: &std::path::Path,
    verbose: bool,
    shutdown: &Arc<AtomicBool>,
    ipc_fatal: &Arc<AtomicBool>,
    tracker: &mut IpcBindTracker,
) -> Option<tokio::net::windows::named_pipe::NamedPipeServer> {
    use tokio::net::windows::named_pipe::ServerOptions;
    loop {
        if shutdown.load(Ordering::Relaxed) {
            return None;
        }
        match ServerOptions::new()
            .first_pipe_instance(first_create)
            .create(rsh_ipc::IPC_PIPE_NAME)
        {
            Ok(pipe) => {
                tracker.record_success();
                return Some(pipe);
            }
            Err(error) => {
                let in_use = pipe_create_is_in_use(&error);
                match tracker.record_failure(in_use) {
                    IpcBindRetry::RetryAfter(delay) => {
                        if in_use {
                            // DS-14: quiet after the first few
                            // lines — at the 1 s cap an in-use
                            // squat would otherwise log once per
                            // second for the full ~90 s cap window.
                            // First 3 + every 30th (~30 s) is enough
                            // to see it is still stuck.
                            if tracker.in_use_failures <= 3
                                || tracker.in_use_failures.is_multiple_of(30)
                            {
                                log_line(
                                    verbose,
                                    &format!(
                                        "daemon: IPC pipe create failed (in use, retry {}): {error} (kind={:?}, os_error={:?})",
                                        tracker.in_use_failures,
                                        error.kind(),
                                        error.raw_os_error()
                                    ),
                                );
                            }
                        } else {
                            log_line(
                                verbose,
                                &format!(
                                    "daemon: IPC pipe create failed (attempt {}/5): {error} (kind={:?}, os_error={:?})",
                                    tracker.failures,
                                    error.kind(),
                                    error.raw_os_error()
                                ),
                            );
                        }
                        tokio::time::sleep(delay).await;
                    }
                    IpcBindRetry::Fatal => {
                        // DS-14: name which cap tripped — the 5
                        // real-failure fatal, or the ~90 s in-use
                        // squat fatal (no real failures at all).
                        let detail = if in_use && tracker.failures == 0 {
                            format!(
                                "daemon IPC pipe held in use for {} s ({} retries) — the daemon cannot serve the UI",
                                IN_USE_FATAL_AFTER.as_secs(),
                                tracker.in_use_failures
                            )
                        } else {
                            "daemon IPC bind failed after 5 attempts — the daemon cannot serve the UI".to_string()
                        };
                        signal_ipc_fatal_shutdown(data_dir, &detail, shutdown, ipc_fatal);
                        return None;
                    }
                }
            }
        }
    }
}

/// Serve the IPC named pipe forever (Windows).
#[cfg(windows)]
async fn serve_ipc(
    server: Arc<IpcServer>,
    data_dir: &std::path::Path,
    verbose: bool,
    shutdown: Arc<AtomicBool>,
    ipc_fatal: Arc<AtomicBool>,
) {
    use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
    let mut tracker = IpcBindTracker::new();
    // `first_pipe_instance(true)` ONLY on the first create in the
    // process (F1): after the UI connects, instance #1 stays alive,
    // so a second `first_pipe_instance(true)` create fails
    // `ERROR_ACCESS_DENIED` — the v2.0.2 loop killer (user log,
    // 2026-11-16: 1199 pipe-create failures, daemon dead ~5 s after
    // every UI connect).
    let mut first_create = true;
    // The listening instance carried between iterations: the NEXT
    // listener is created BEFORE the accepted pipe is handed to
    // `serve_stream` (tokio NamedPipeServer doc pattern), so a
    // listener is always available.
    let mut listener: Option<NamedPipeServer> = None;
    // NOTE (F10): the "IPC listening" line is logged only after the
    // first successful create — the old unconditional line, printed
    // before the loop, claimed the pipe was listening while every
    // create was in fact failing.
    let mut listening_logged = false;
    loop {
        if shutdown.load(Ordering::Relaxed) {
            return;
        }
        let pipe = match listener.take() {
            Some(pipe) => pipe,
            None => {
                match create_pipe_listener(
                    first_create,
                    data_dir,
                    verbose,
                    &shutdown,
                    &ipc_fatal,
                    &mut tracker,
                )
                .await
                {
                    Some(pipe) => pipe,
                    // Fatal signalled (or shutdown) inside.
                    None => return,
                }
            }
        };
        if first_create {
            first_create = false;
            if !listening_logged {
                log_line(verbose, "daemon: IPC listening on \\\\.\\pipe\\rsh-daemon");
                listening_logged = true;
            }
        }
        // NOTE (OPEN, Windows verification): the pipe currently uses
        // the default security descriptor. An explicit
        // current-user-only DACL is a Phase-7 hardening item — see
        // docs/V2-PHASE-2-GATE.md. The pipe is local-only and the
        // name is per-user via the single-instance mutex scope.
        if let Err(error) = pipe.connect().await {
            // DS-37 (B-12): a failed connect used to restart the
            // loop silently with no log and no delay — a
            // fast-failing connect hot-spun pipe creates. Log it
            // and back off briefly before retrying.
            log_line(
                verbose,
                &format!("daemon: IPC pipe connect failed: {error} — retrying"),
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        }
        // Pre-create the next listener BEFORE handing this pipe to
        // `serve_stream`. One best-effort attempt: a failure here is
        // NOT fatal — the loop-top create retries it with the full
        // backoff/fatal policy, and the accepted client is served
        // regardless.
        listener = match ServerOptions::new()
            .first_pipe_instance(false)
            .create(rsh_ipc::IPC_PIPE_NAME)
        {
            Ok(next) => {
                tracker.record_success();
                Some(next)
            }
            Err(error) => {
                log_line(
                    verbose,
                    &format!(
                        "daemon: IPC next-pipe create failed (will retry): {error} (kind={:?}, os_error={:?})",
                        error.kind(),
                        error.raw_os_error()
                    ),
                );
                None
            }
        };
        let server = Arc::clone(&server);
        tokio::spawn(async move {
            server.serve_stream(pipe).await;
        });
    }
}

/// Name of the machine-readable daemon status file in the data dir
/// (M8/L8, v2.0.2): the UI reads this when IPC is dead — it is the
/// only channel left when the daemon cannot serve IPC at all, so a
/// daemon that dies before (or without) IPC must leave a trace the
/// UI can show instead of silently falling back to simulated data.
const DAEMON_STATUS_FILE: &str = "daemon-status.json";

/// Write `daemon-status.json` (M8/L8): `{"status": "fatal" |
/// "running", "code": ..., "detail": ..., "pid": ..., "at_unix":
/// ...}`. Atomic (temp file in the same directory, then rename).
/// Best effort — a failure is swallowed, never fatal: the
/// dual-location fatal log is the other channel, and a status
/// write must never mask the fatal exit itself.
fn write_daemon_status(data_dir: &std::path::Path, status: &str, code: &str, detail: &str) {
    let at_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    let json = serde_json::json!({
        "status": status,
        "code": code,
        "detail": detail,
        "pid": std::process::id(),
        "at_unix": at_unix,
    });
    let path = data_dir.join(DAEMON_STATUS_FILE);
    let temp = path.with_extension(format!("json.tmp-{}", std::process::id()));
    if let Ok(text) = serde_json::to_string_pretty(&json)
        && std::fs::write(&temp, text).is_ok()
    {
        let _ = std::fs::rename(&temp, &path);
    }
}

/// Remove a stale `daemon-status.json` (a previous run's fatal
/// state). Only the daemon that holds the single-instance lock
/// clears it — a second daemon that finds a hung first daemon
/// writes its own fatal state and exits instead.
fn clear_daemon_status(data_dir: &std::path::Path) {
    let _ = std::fs::remove_file(data_dir.join(DAEMON_STATUS_FILE));
}

/// Write the UI quit request (`<data_dir>/rsh-ui.quit`) that the UI
/// polls in its frame timer (`rsh-ui` `ui_lifecycle::take_quit_request`,
/// consumed once per file). Called from tray "Quit RSH" so a UI hidden
/// to the tray quits together with the daemon.
fn write_ui_quit_request(data_dir: &std::path::Path) -> std::io::Result<()> {
    std::fs::write(data_dir.join("rsh-ui.quit"), b"quit")
}

/// Quarantine the unreadable store files (`profiles.json`,
/// `settings.json`) aside so a fresh-default open can succeed: each
/// existing file is renamed to `<name>.corrupt-<unix-seconds>-<pid>-<n>`
/// (a unique suffix per attempt, so two quarantines never collide).
/// Returns the quarantine paths (empty when neither file existed).
/// The corrupt bytes are never deleted — only moved — so the
/// originals can be put back or inspected by hand.
fn quarantine_corrupt_store_files(data_dir: &std::path::Path) -> Vec<PathBuf> {
    let mut quarantined = Vec::new();
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    for name in ["profiles.json", "settings.json"] {
        let path = data_dir.join(name);
        if !path.exists() {
            continue;
        }
        let mut attempt = 0u32;
        let mut target = PathBuf::from(format!(
            "{}.corrupt-{secs}-{}-{attempt}",
            path.display(),
            std::process::id()
        ));
        while target.exists() {
            attempt += 1;
            target = PathBuf::from(format!(
                "{}.corrupt-{secs}-{}-{attempt}",
                path.display(),
                std::process::id()
            ));
        }
        if std::fs::rename(&path, &target).is_ok() {
            quarantined.push(target);
        }
    }
    quarantined
}

/// Single-instance check, runtime + IPC + manager loop, engine
/// start, tray creation, Slint event loop (tray only until Phase 3
/// adds the window), shutdown.
// One function: daemon startup is a single linear pipeline
// (lock → runtime → IPC → engine → tray → event loop); splitting
// it would scatter shared handles across helpers.
#[allow(clippy::too_many_lines)]
fn main() {
    let args = parse_args();
    let verbose = args.verbose || args.check;
    // A missing platform data dir falls back to the process working
    // directory — the store, the pid file and (on Linux) the IPC
    // socket then live in the CWD instead of `%APPDATA%\RSH`, where
    // the UI looks for them. That must never happen silently: the
    // fallback is logged loudly below. (`dirs::data_dir()` is
    // `SHGetKnownFolderPath(RoamingAppData)` on Windows — it only
    // fails when the user profile is not loaded, e.g. a service /
    // SYSTEM launch or a login race — and `$HOME` unset on Linux.)
    let (data_dir, data_dir_fell_back) = match args.data_dir.clone().or_else(rsh_app_core::data_dir)
    {
        Some(dir) => (dir, false),
        None => (PathBuf::from("."), true),
    };
    // F16: install the panic hook FIRST, before anything that can
    // panic leaves no trace — a silent panic used to die with no
    // log at all. The hook appends to
    // `<data_dir>/logs/diagnostics-panic.log` (plus the `%TEMP%`
    // fallback inside `install_panic_hook`).
    install_panic_hook(data_dir.join("logs"));
    log_line(
        verbose,
        &format!(
            "daemon: starting rsh-daemon v{} (pid {}, data dir {})",
            rsh_api::app_version(),
            std::process::id(),
            data_dir.display()
        ),
    );
    if data_dir_fell_back {
        log_line(
            verbose,
            &format!(
                "daemon: WARNING data dir could not be resolved — using CWD {} instead of the RSH data dir; profiles, pid file and IPC socket live here now (the UI looks in the platform data dir)",
                std::env::current_dir()
                    .map_or_else(|_| "?".to_string(), |dir| dir.display().to_string())
            ),
        );
    }

    // Single-instance: a second launch signals the running daemon
    // (IPC ping — the UI open hook lands in Phase 3) and exits.
    let lock = match acquire(&data_dir) {
        Ok(AcquireOutcome::First(lock)) => lock,
        Ok(AcquireOutcome::AlreadyRunning(pid)) => {
            let socket = socket_path(&data_dir);
            // F15: the lock holder may be a daemon that is still
            // starting (IPC not bound yet — store open + runtime
            // happen before the pipe exists) — give its ping up to
            // `PING_ATTEMPTS` × `PING_INTERVAL` (~10 s, not the old
            // 3 × 500 ms), and track whether its IPC endpoint ever
            // existed: no endpoint yet means "still starting", which
            // must NOT be reported as hung.
            let (alive, endpoint_seen) = std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .ok()?;
                Some(runtime.block_on(async move {
                    let mut endpoint_seen = false;
                    for _ in 0..single_instance::PING_ATTEMPTS {
                        if single_instance::ipc_endpoint_exists(&socket) {
                            endpoint_seen = true;
                        }
                        if let Ok(client) = rsh_ipc::IpcClient::connect(&socket).await
                            && let Ok(true) = client.ping().await
                        {
                            return (true, endpoint_seen);
                        }
                        tokio::time::sleep(single_instance::PING_INTERVAL).await;
                    }
                    (false, endpoint_seen)
                }))
            })
            .join()
            .ok()
            .flatten()
            .unwrap_or((false, false));
            match single_instance::classify_already_running(alive, endpoint_seen) {
                single_instance::AlreadyRunningVerdict::Alive => {
                    log_line(
                        verbose,
                        &format!("daemon: already running (pid {pid:?}, IPC answers) — exiting"),
                    );
                    std::process::exit(0);
                }
                single_instance::AlreadyRunningVerdict::Hung => {
                    // The lock is held, the endpoint exists, but the
                    // daemon does not answer IPC: hung, or a stale
                    // lock the single-instance check could not
                    // reclaim. Never exit 0 (success) for a broken
                    // daemon, and never leave only an invisible
                    // stderr line: a UI that spawned us falls back to
                    // simulated data, so this failure must be in the
                    // dual-location log AND in the machine-readable
                    // status file (L8, v2.0.2) — the UI reads the
                    // file and tells the user the running daemon is
                    // hung instead of silently simulating.
                    let detail = format!(
                        "already running (pid {pid:?}) but not responding to IPC after {} tries (endpoint exists) — the running daemon is hung or its IPC is dead; not taking over",
                        single_instance::PING_ATTEMPTS
                    );
                    write_daemon_status(&data_dir, "fatal", "DaemonHung", &detail);
                    let _ = log_fatal(&format!("daemon: {detail}"));
                    std::process::exit(1);
                }
                single_instance::AlreadyRunningVerdict::Starting => {
                    // No fatal status: the first daemon is still
                    // starting (its IPC endpoint never appeared in
                    // ~10 s). Exiting 1 without writing `DaemonHung`,
                    // so the UI does not latch a false fatal for a
                    // daemon that is still coming up — the first
                    // daemon will either answer or leave its own
                    // trace.
                    log_line(
                        verbose,
                        &format!(
                            "daemon: already running (pid {pid:?}) but its IPC endpoint never appeared after {} tries — the first daemon is still starting; not declaring it hung, not taking over",
                            single_instance::PING_ATTEMPTS
                        ),
                    );
                    std::process::exit(1);
                }
            }
        }
        Err(error) => {
            let _ = log_fatal(&format!(
                "daemon: cannot acquire single-instance lock: {error}"
            ));
            std::process::exit(1);
        }
    };

    // This process holds the single-instance lock: a previous run's
    // fatal status is stale now (the daemon it describes is gone),
    // so clear it before anything else can misread it.
    clear_daemon_status(&data_dir);

    // Open the app state (repairs a corrupt store from backups).
    // Last resort when even the backups cannot repair it: quarantine
    // the unreadable store files aside (bytes preserved for manual
    // recovery) and open with fresh defaults, instead of dying
    // before the tray — a daemon that exits here leaves no tray, no
    // IPC, and a UI on simulated data with no visible reason. Only
    // when even a fresh open fails (e.g. the data dir is unwritable)
    // does the daemon exit, and it logs the fatal reason to both
    // log locations first.
    let state = match AppStateCore::open_with_null_sink_repair(&data_dir) {
        (Ok(state), repair_note) => {
            if let Some(note) = repair_note {
                log_line(verbose, &format!("daemon: store repaired: {note}"));
            }
            Arc::new(state)
        }
        (Err(error), _) => {
            log_line(
                verbose,
                &format!(
                    "daemon: store open failed ({error}) — quarantining corrupt store files and retrying with defaults"
                ),
            );
            let quarantined = quarantine_corrupt_store_files(&data_dir);
            log_line(
                verbose,
                &format!("daemon: store files quarantined: {quarantined:?}"),
            );
            match AppStateCore::open_with_null_sink_repair(&data_dir) {
                (Ok(state), _) => {
                    log_line(
                        verbose,
                        &format!(
                            "daemon: store reset to defaults after quarantine (corrupt files kept as {quarantined:?}); profiles/settings were unreadable and have been reset"
                        ),
                    );
                    Arc::new(state)
                }
                (Err(retry_error), _) => {
                    let _ = log_fatal(&format!(
                        "daemon: cannot open profile store even after quarantine: {retry_error} (first error: {error}; quarantined: {quarantined:?})"
                    ));
                    std::process::exit(1);
                }
            }
        }
    };
    let api = DaemonApi::new(Arc::clone(&state));
    // BACKEND-2 migration: log the persisted-backend note (if any)
    // so a HIDMaestro→vJoy switch is visible in the startup log.
    if let Some(note) = state.backend_migration_note() {
        log_line(verbose, &format!("daemon: backend migration: {note}"));
    }
    // Binding store: log the load warnings (dropped entries,
    // corrupt file) so a binding that did not survive is never
    // silent.
    for warning in state.binding_load_warnings() {
        log_line(verbose, &format!("daemon: bindings: {warning}"));
    }
    let (event_tx, _event_rx) = broadcast::channel::<AppEvent>(256);
    let tray_actions: Arc<Mutex<Vec<TrayAction>>> = Arc::new(Mutex::new(Vec::new()));
    let shutdown = Arc::new(AtomicBool::new(false));
    // F24: set only by `signal_ipc_fatal_shutdown` (a fatal IPC
    // bind) — `main` exits 1 for it after the teardown below, so a
    // fatal bind reports like `DaemonHung` instead of exiting 0.
    // A clean quit never sets it.
    let ipc_fatal = Arc::new(AtomicBool::new(false));

    // Runtime thread: IPC server + manager loop.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("rsh-daemon-rt")
        .enable_all()
        .build()
        .unwrap_or_else(|error| {
            let _ = log_fatal(&format!("rsh-daemon: cannot build runtime: {error}"));
            std::process::exit(1);
        });
    let ipc_server = Arc::new(IpcServer::new(
        Arc::clone(&api) as Arc<dyn AppApi>,
        event_tx.clone(),
    ));
    {
        let server = Arc::clone(&ipc_server);
        let dir = data_dir.clone();
        let shutdown = Arc::clone(&shutdown);
        let ipc_fatal = Arc::clone(&ipc_fatal);
        runtime.spawn(async move {
            serve_ipc(server, &dir, verbose, shutdown, ipc_fatal).await;
        });
    }
    // DS-02 defence-in-depth: keep the manager-loop handle. A
    // panicking tokio task just dies — the old code discarded both
    // handles, so a dead loop left the daemon looking alive (no
    // watchdog, no retries, no tray drain). `main` watches this
    // handle in the headless park loop and restarts the loop (and
    // logs fatally after the event loop) when it ends without a
    // shutdown request.
    let spawn_manager_loop = {
        let api = Arc::clone(&api);
        let tray_actions = Arc::clone(&tray_actions);
        let shutdown = Arc::clone(&shutdown);
        // A cloned `Handle` (not a borrow of `runtime`): the
        // closure must stay callable after `main` uses `runtime`
        // again (`--check`'s `block_on`), and `Handle::spawn`
        // is the same spawn.
        let handle = runtime.handle().clone();
        move || {
            let api = Arc::clone(&api);
            let event_tx = event_tx.clone();
            let tray_actions = Arc::clone(&tray_actions);
            let shutdown = Arc::clone(&shutdown);
            handle.spawn(async move {
                manager_loop(api, event_tx, tray_actions, shutdown, verbose).await;
            })
        }
    };
    let mut manager_handle = spawn_manager_loop();

    // Engine + transport (unless --no-connect): the daemon owns the
    // engine from startup, like the Tauri shell did.
    if !args.no_connect {
        match state.start_engine() {
            Ok(()) => {
                let (action, changed) = api.manager_event(ConnEvent::Connect);
                let _ = (action, changed);
                match state.connect_transport(TransportKind::Udp) {
                    Ok(()) => log_line(verbose, "daemon: engine started, UDP transport connecting"),
                    Err(error) => log_line(
                        verbose,
                        &format!("daemon: transport connect failed: {error}"),
                    ),
                }
            }
            Err(error) => log_line(verbose, &format!("daemon: engine start failed: {error}")),
        }
    }

    // DS-81: backend activation + autostart sync run on a
    // background thread, concurrently with tray creation below —
    // neither may block startup (activation probes drivers,
    // autostart sync touches the registry / .desktop file, and the
    // tray loop already waits up to 12 s on a headless host). IPC
    // is already serving (runtime spawned above), so this changes
    // no IPC ordering. The thread is best-effort and detached:
    // `--check` exits without waiting for it.
    let exe = std::env::current_exe().ok();
    {
        let state = Arc::clone(&state);
        let _ = std::thread::Builder::new()
            .name("rsh-daemon-startup".to_string())
            .spawn(move || {
                // D-7 lazy-start (Phase 7): activate the startup
                // output backend. vJoy activates now; HIDMaestro
                // stays deferred until the user turns "Send to
                // game" on, so autostart never pops a UAC prompt at
                // login. This never fails the launch (see the
                // outcome log).
                let backend = state.activate_startup_backend();
                log_line(
                    verbose,
                    &format!(
                        "daemon: startup backend requested={:?} active={:?} fell_back={} reason={:?}",
                        backend.requested, backend.active, backend.fell_back, backend.reason
                    ),
                );

                // Autostart sync (Phase 7): `settings.autostart`
                // drives the Windows Run key / Linux .desktop file.
                // Best-effort: a failed write is logged, never
                // fatal.
                let Some(exe) = exe else { return };
                // DS-81: the settings are read ONCE (one `Arc`
                // clone) — the old code read them three times.
                let settings = state.settings_arc();
                let mut autostart = settings.autostart;
                let mut skip_autostart_sync = false;
                // Heal a stale persisted `autostart = false` FIRST:
                // the v1 default was false, so a machine upgraded
                // from v1.0.2 carries `settings.json` with autostart
                // off — and the plain sync below would DELETE the
                // Run key the installer just wrote. A Run entry that
                // already points at this exe cannot come from a v2
                // Settings toggle-off (that deletes the entry
                // immediately), so its presence is newer evidence
                // than the stale persisted flag: adopt it and
                // persist the fix. Only when persisting the adoption
                // fails is the sync skipped this launch, so a stale
                // flag can never delete a live entry.
                if !autostart && tray_autostart::autostart_enabled(&exe) {
                    let mut adopted = (*settings).clone();
                    adopted.autostart = true;
                    match state.set_settings(adopted) {
                        Ok(_) => {
                            autostart = true;
                            log_line(
                                verbose,
                                "daemon: autostart adopted from the existing Run entry (persisted autostart was stale off) — Run key kept",
                            );
                        }
                        Err(error) => {
                            log_line(
                                verbose,
                                &format!(
                                    "daemon: autostart adoption failed ({error}) — leaving the Run entry untouched this launch"
                                ),
                            );
                            skip_autostart_sync = true;
                        }
                    }
                }
                if !skip_autostart_sync {
                    match tray_autostart::sync_autostart(autostart, &exe) {
                        Ok(sync) => log_line(
                            verbose,
                            &format!(
                                "daemon: autostart sync: {sync:?} (enabled now: {})",
                                tray_autostart::autostart_enabled(&exe)
                            ),
                        ),
                        Err(error) => log_line(verbose, &format!("daemon: autostart sync failed: {error}")),
                    }
                }
            });
    }

    // --check: verify IPC answers, then exit 0 (no tray needed).
    if args.check {
        let socket = socket_path(&data_dir);
        let ok = runtime.block_on(async move {
            // Give the listener a moment to bind.
            for _ in 0..20 {
                if let Ok(client) = rsh_ipc::IpcClient::connect(&socket).await
                    && let Ok(true) = client.ping().await
                {
                    let version = client.version().await.unwrap_or_default();
                    println!("DAEMON CHECK OK: ping=true version={version}");
                    return true;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            false
        });
        shutdown.store(true, Ordering::Relaxed);
        let _ = state.stop_engine();
        lock.release();
        std::process::exit(i32::from(!ok));
    }

    // Tray on the Slint event loop (main thread). The icon is
    // created on EVERY launch (A-09) — not only --tray.
    // The tray can fail transiently at login (Explorer / the
    // notification area is not ready yet) — a single attempt would
    // leave the daemon headless for the whole session with no way
    // back to the UI. Retry for ~10 s before giving up; a headless
    // Linux host fails every attempt (expected, spike §3) and the
    // daemon keeps running headless.
    //
    // F11: the tray handle is OWNED here for the process lifetime —
    // dropping it after the event loop sends `NIM_DELETE` and the
    // icon goes away on a clean quit (the old `mem::forget` leaked
    // it, so ghosts piled up).
    let tray: Option<RshTray> = if args.no_tray {
        log_line(verbose, "daemon: tray disabled (--no-tray)");
        None
    } else {
        let mut tray: Option<RshTray> = None;
        for attempt in 1..=6 {
            match create_tray(&api, &tray_actions, &shutdown, verbose, &data_dir) {
                Ok(handle) => {
                    log_line(verbose, "daemon: tray icon registered");
                    tray = Some(handle);
                    break;
                }
                Err(error) => {
                    log_line(
                        verbose,
                        &format!("daemon: tray unavailable (attempt {attempt}/6: {error})"),
                    );
                    // DS-48: fail fast on a headless host — with no
                    // display server a tray can never appear, so
                    // retrying 6×2 s on the main thread only delays
                    // startup by ~10 s (and blocks shutdown/quit
                    // handling the whole time). Windows keeps the
                    // retries: Explorer may just not be ready yet.
                    if attempt == 1 && tray_headless() {
                        log_line(
                            verbose,
                            "daemon: no display server — skipping tray retries (headless)",
                        );
                        break;
                    }
                    if attempt < 6 {
                        std::thread::sleep(Duration::from_secs(2));
                    }
                }
            }
        }
        if tray.is_none() {
            log_line(
                verbose,
                "daemon: tray unavailable after 6 attempts — running headless",
            );
        }
        tray
    };
    let tray_ok = tray.is_some();

    if tray_ok {
        // The event loop runs until Quit is picked (or the process
        // is killed). Tray callbacks run on this thread.
        if let Err(error) = slint::run_event_loop() {
            log_line(verbose, &format!("daemon: event loop error: {error}"));
        }
    } else {
        // No tray, no event loop: park the main thread until killed
        // or until a background task signals shutdown (F2: a fatal
        // IPC bind ends the process this way instead of
        // `process::exit(1)` from the task).
        // (Ctrl-C / SIGTERM end the process; the pid file is
        // reclaimed by the next launch's /proc check.)
        //
        // DS-02: watch the manager-loop handle while parked — a
        // loop that ends without a shutdown request (panic) is
        // restarted here instead of leaving the daemon half-alive.
        while !shutdown.load(Ordering::Relaxed) {
            if manager_handle.is_finished() {
                let _ = log_fatal("daemon: manager loop ended unexpectedly — restarting it");
                log_line(
                    verbose,
                    "daemon: manager loop ended unexpectedly — restarting it",
                );
                manager_handle = spawn_manager_loop();
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    // F11/F10: dropping the tray FIRST removes the icon
    // (`NIM_DELETE`) before the lock is released, then log the exit
    // with the pid so a start/exit pair brackets every run in the log.
    drop(tray);
    // DS-02: if the manager loop already ended while the event loop
    // was running — and no shutdown was requested (tray Quit sets
    // shutdown first, so a loop ended by Quit is normal) — that is
    // a fatal trace: the loop never ends on its own otherwise.
    if !shutdown.load(Ordering::Relaxed) && manager_handle.is_finished() {
        let _ = log_fatal("daemon: manager loop ended unexpectedly before shutdown");
        log_line(
            verbose,
            "daemon: manager loop ended unexpectedly before shutdown",
        );
    }
    log_line(
        verbose,
        &format!("daemon: exiting (pid {})", std::process::id()),
    );
    shutdown.store(true, Ordering::Relaxed);
    let _ = state.stop_engine();
    lock.release();
    // F24: a fatal IPC bind exits 1 — but only AFTER the teardown
    // above (tray removed, engine stopped, lock released), so F2's
    // rule (no `process::exit(1)` from a background task) still
    // holds and the exit code is decided here, in `main`. A clean
    // SIGTERM/quit leaves `ipc_fatal` unset and exits 0.
    if ipc_fatal.load(Ordering::Relaxed) {
        std::process::exit(1);
    }
}

/// The tray popup window + its visibility state. Lives on the Slint
/// event-loop thread only (`Rc` / `Cell` — never sent anywhere).
struct PopupHandle {
    popup: TrayPopup,
    visible: Cell<bool>,
    shown_at: Cell<Option<Instant>>,
    /// `--verbose` passthrough, so the popup show/hide log lines
    /// (F10) also reach stderr.
    verbose: bool,
    /// Click-outside poll state (F9): the foreground baseline is
    /// re-sampled in the settle window after the grace, and a
    /// dismiss needs a sustained change — see
    /// [`tray_popup::PopupDismissPoll`].
    poll: RefCell<PopupDismissPoll>,
    /// Show-time activation finished — either the popup became
    /// foreground or the retry window
    /// ([`tray_popup::POPUP_ACTIVATION_TIMEOUT`]) expired. While
    /// false, the timer tick retries activation (F9).
    activation_done: Cell<bool>,
    /// The "no popup window yet" retry note was already logged once
    /// (bounds the activation-retry logging to one line per show).
    activation_note_logged: Cell<bool>,
}

impl PopupHandle {
    /// Push the current live data into the popup's properties.
    // Casts are safe here: snapshot values are bounded (0..100, ±180°),
    // so no truncation can occur in practice.
    #[allow(clippy::cast_possible_truncation)]
    fn refresh(&self, api: &DaemonApi) {
        let profiles = api.state.list_profiles();
        let games = api.state.list_games();
        let data = TrayPopupData::collect(api as &dyn AppApi, &profiles, &games);
        self.popup.set_conn_label(data.conn_label.into());
        self.popup.set_conn_sub(data.conn_sub.into());
        self.popup.set_connected(data.connected);
        // DS-22: the popup follows the persisted UI theme (the same
        // `<data_dir>/ui-theme` file the main window reads and
        // writes). Re-read every refresh so a theme change in
        // Settings repaints the popup on its next show — the file
        // read is cheap, and the Auto OS sample is cached 60 s
        // inside `popup_is_light`, so the 10 Hz refresh never
        // spawns `reg` more than once a minute.
        self.popup
            .set_light_theme(popup_is_light(rsh_app_core::data_dir().as_deref()));
        self.popup.set_send_to_game(data.send_to_game);
        self.popup.set_steer_angle(data.steering_deg as f32);
        self.popup
            .set_steer_text(format!("{:+.0}°", data.steering_deg).into());
        self.popup.set_throttle(data.throttle_pct as f32);
        self.popup.set_brake(data.brake_pct as f32);
        self.popup.set_clutch(data.clutch_pct as f32);
        self.popup.set_throttle_text(data.throttle_text.into());
        self.popup.set_brake_text(data.brake_text.into());
        self.popup.set_clutch_text(data.clutch_text.into());
        self.popup.set_profile_name(data.profile_name.into());
        self.popup.set_profile_sub(data.profile_sub.into());
        self.set_game_row(0, data.games.first());
        self.set_game_row(1, data.games.get(1));
        self.set_game_row(2, data.games.get(2));
    }

    /// One game row (empty strings when the daemon knows fewer than
    /// `index + 1` games — the row renders blank; see the Phase 7
    /// notes in docs/RELEASE-CHECKLIST.md).
    fn set_game_row(&self, index: usize, game: Option<&PopupGame>) {
        let (id, name, initials, state, can_launch) = match game {
            Some(game) => (
                game.id.as_str(),
                game.name.as_str(),
                game.initials.as_str(),
                game.state_text.as_str(),
                game.can_launch,
            ),
            None => ("", "", "", "", false),
        };
        match index {
            0 => {
                self.popup.set_game0_id(id.into());
                self.popup.set_game0_name(name.into());
                self.popup.set_game0_initials(initials.into());
                self.popup.set_game0_state(state.into());
                self.popup.set_game0_can_launch(can_launch);
            }
            1 => {
                self.popup.set_game1_id(id.into());
                self.popup.set_game1_name(name.into());
                self.popup.set_game1_initials(initials.into());
                self.popup.set_game1_state(state.into());
                self.popup.set_game1_can_launch(can_launch);
            }
            _ => {
                self.popup.set_game2_id(id.into());
                self.popup.set_game2_name(name.into());
                self.popup.set_game2_initials(initials.into());
                self.popup.set_game2_state(state.into());
                self.popup.set_game2_can_launch(can_launch);
            }
        }
    }

    /// Show the popup next to the tray icon (or docked bottom-right
    /// when the icon rect is unknown — Linux / hidden tray).
    fn show(&self, api: &DaemonApi) {
        // A4/H-2: record the foreground window BEFORE showing —
        // the click-outside poll dismisses only when the
        // foreground moves away from the baseline it re-samples
        // after the grace (F9 — the show-time value is stale by
        // the time Windows settles, so it is diagnostics only).
        let foreground_at_show = tray_geometry::foreground_hwnd();
        *self.poll.borrow_mut() = PopupDismissPoll::new(foreground_at_show);
        let (x, y) = anchor_popup(tray_geometry::tray_icon_rect(), tray_geometry::work_area());
        self.popup
            .window()
            .set_position(slint::PhysicalPosition::new(x, y));
        self.refresh(api);
        match self.popup.show() {
            Ok(()) => {
                self.visible.set(true);
                self.shown_at.set(Some(Instant::now()));
                self.activation_done.set(false);
                self.activation_note_logged.set(false);
                // H-2/F9: activate the popup — a tray-shown window
                // usually never becomes foreground on its own
                // (foreground lock), which is what made the old poll
                // dismiss it ~1 s after the click. The attempt is
                // logged (F10) and retried on timer ticks until
                // the popup is foreground or the timeout expires
                // (see `tick`): Slint may not have created the
                // native window yet, so a first attempt that finds
                // no window is NOT a failure, only a retry.
                let attempted =
                    tray_geometry::activate_popup_window(tray_geometry::POPUP_WINDOW_TITLE);
                log_line(
                    self.verbose,
                    &format!(
                        "tray popup: shown pid={} foreground_at_show={foreground_at_show:?} activation_attempted={attempted}",
                        std::process::id()
                    ),
                );
            }
            Err(error) => {
                log_line(
                    self.verbose,
                    &format!(
                        "tray popup: show failed pid={} foreground_at_show={foreground_at_show:?} error={error}",
                        std::process::id()
                    ),
                );
            }
        }
    }

    /// Hide the popup, logging why (F10): every hide carries the
    /// reason + daemon pid + foreground values + elapsed time, so
    /// a poll dismiss and a process death are distinguishable in
    /// the log — previously `show()`/`hide()` logged nothing and
    /// the two looked identical. No log line when the popup was
    /// not visible (a no-op hide).
    fn hide(&self, reason: &str) {
        if self.visible.get() {
            let poll = self.poll.borrow();
            log_line(
                self.verbose,
                &format!(
                    "tray popup: hidden reason={reason} pid={} elapsed_ms={} foreground_at_show={:?} baseline={:?} foreground_now={:?} was_foreground_sustained={}",
                    std::process::id(),
                    self.shown_at
                        .get()
                        .map_or(0, |shown| shown.elapsed().as_millis()),
                    poll.foreground_at_show(),
                    poll.baseline(),
                    tray_geometry::foreground_hwnd(),
                    poll.was_foreground_sustained(),
                ),
            );
        }
        let _ = self.popup.hide();
        self.visible.set(false);
        self.shown_at.set(None);
        self.activation_done.set(true);
    }

    /// One 100 ms timer tick while the popup is visible (F9).
    /// `foreground_now` is this tick's single
    /// `GetForegroundWindow()` read — both the foreground check
    /// and the dismiss poll derive from it (no double read).
    /// Returns `true` when the popup should hide (click-away).
    fn tick(
        &self,
        shown: Instant,
        foreground_now: Option<isize>,
        popup_hwnd: Option<isize>,
    ) -> bool {
        let elapsed = shown.elapsed();
        let popup_foreground = popup_hwnd.is_some_and(|hwnd| foreground_now == Some(hwnd));
        // (c) Activation retry: the show-time attempt runs before
        // Slint has created the native window, so retry on ticks
        // until the popup is foreground or the timeout expires.
        // Success and timeout are logged once each; a missing
        // window is logged once (it is expected early on).
        if !self.activation_done.get() {
            if popup_foreground {
                self.activation_done.set(true);
                log_line(
                    self.verbose,
                    &format!(
                        "tray popup: activated pid={} after {}ms",
                        std::process::id(),
                        elapsed.as_millis()
                    ),
                );
            } else if elapsed <= POPUP_ACTIVATION_TIMEOUT {
                let attempted =
                    tray_geometry::activate_popup_window(tray_geometry::POPUP_WINDOW_TITLE);
                if !attempted && !self.activation_note_logged.get() {
                    self.activation_note_logged.set(true);
                    log_line(
                        self.verbose,
                        &format!(
                            "tray popup: activation retry — no popup window yet pid={} elapsed_ms={}",
                            std::process::id(),
                            elapsed.as_millis()
                        ),
                    );
                }
            } else {
                self.activation_done.set(true);
                log_line(
                    self.verbose,
                    &format!(
                        "tray popup: activation not achieved within 1 s pid={} — popup visible but inactive, poll keeps it open until click-away",
                        std::process::id()
                    ),
                );
            }
        }
        // Dismiss poll — only once the native window exists (no
        // window yet: stay open, never guess).
        if popup_hwnd.is_none() {
            return false;
        }
        self.poll
            .borrow_mut()
            .poll(elapsed, foreground_now, popup_hwnd)
    }
}

/// Create + show the tray icon, wiring its callbacks, and create the
/// tray popup (§7.7) alongside it. The tray lives on the Slint
/// event loop (main thread). A popup failure never takes the tray
/// down: left-click then falls back to opening the UI.
///
/// Returns the tray handle: the caller owns it for the process
/// lifetime (F11) — dropping it sends `NIM_DELETE` (Slint
/// `PlatformTray::drop`, verified in slint 1.18.1
/// `i-slint-core/items/system_tray/windows.rs`), which is what
/// removes the icon on a clean quit.
/// DS-48: `true` when no tray can ever appear — a host with no
/// display server (no X11 `DISPLAY`, no Wayland
/// `WAYLAND_DISPLAY`). The tray retry loop fails fast after the
/// first attempt there instead of sleeping ~10 s on the main
/// thread. Windows has no display-server variable to check and
/// Explorer may just be slow at login, so it always retries.
#[cfg(unix)]
fn tray_headless() -> bool {
    std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none()
}

/// DS-48: Windows always retries the tray (see the Unix arm).
#[cfg(windows)]
fn tray_headless() -> bool {
    false
}

// One function: tray + popup creation is a single wiring pass
// (icon → menu → popup → timer); splitting it would scatter the
// shared handles across helpers.
#[allow(clippy::too_many_lines)]
fn create_tray(
    api: &Arc<DaemonApi>,
    tray_actions: &Arc<Mutex<Vec<TrayAction>>>,
    shutdown: &Arc<AtomicBool>,
    verbose: bool,
    data_dir: &std::path::Path,
) -> Result<RshTray, slint::PlatformError> {
    let tray = RshTray::new()?;
    // A25: fill the native menu from the live store (real profiles
    // + installed games only — never hardcoded names). The menu is
    // built once at tray creation; profiles created later in the UI
    // appear after the next daemon start.
    let profiles = api.state.list_profiles();
    tray.set_profile_items(slint::ModelRc::new(slint::VecModel::from(
        profiles
            .iter()
            .map(|summary| ProfileMenuItem {
                name: summary.name.clone().into(),
                active: summary.active,
            })
            .collect::<Vec<_>>(),
    )));
    let games = api.state.list_games();
    tray.set_game_items(slint::ModelRc::new(slint::VecModel::from(
        games
            .iter()
            .filter(|game| game.installed)
            .map(|game| GameMenuItem {
                id: game.id.clone().into(),
                name: game.name.clone().into(),
            })
            .collect::<Vec<_>>(),
    )));
    tray.set_send_to_game(api.output_enabled());
    tray.set_status_text("RSH — Real Sim Hub".into());
    let push = |tray_actions: &Arc<Mutex<Vec<TrayAction>>>, action: TrayAction| {
        tray_actions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(action);
    };
    // Popup (§7.7): created with the tray, hidden until left-click.
    let popup_handle: Option<Rc<PopupHandle>> = match TrayPopup::new() {
        Ok(popup) => Some(Rc::new(PopupHandle {
            popup,
            visible: Cell::new(false),
            shown_at: Cell::new(None),
            verbose,
            poll: RefCell::new(PopupDismissPoll::new(None)),
            activation_done: Cell::new(true),
            activation_note_logged: Cell::new(false),
        })),
        Err(error) => {
            log_line(
                verbose,
                &format!("daemon: tray popup unavailable ({error}) — menu only"),
            );
            None
        }
    };
    if let Some(handle) = &popup_handle {
        {
            let actions = Arc::clone(tray_actions);
            handle
                .popup
                .on_toggled_send(move || push(&actions, TrayAction::ToggleSend));
        }
        {
            let actions = Arc::clone(tray_actions);
            let popup = &handle.popup;
            let handle = Rc::clone(handle);
            popup.on_open_main(move || {
                handle.hide("open-main");
                push(&actions, TrayAction::OpenMain);
            });
        }
        {
            let actions = Arc::clone(tray_actions);
            let popup = &handle.popup;
            let handle = Rc::clone(handle);
            popup.on_restart_wheel(move || {
                handle.hide("restart-wheel");
                push(&actions, TrayAction::RestartWheel);
            });
        }
        {
            let actions = Arc::clone(tray_actions);
            let popup = &handle.popup;
            let handle = Rc::clone(handle);
            popup.on_launch_game(move |id| {
                handle.hide("launch-game");
                push(&actions, TrayAction::LaunchGame(id.to_string()));
            });
        }
        {
            let actions = Arc::clone(tray_actions);
            handle
                .popup
                .on_cycle_profile(move || push(&actions, TrayAction::CycleProfile));
        }
        {
            let popup = &handle.popup;
            let handle = Rc::clone(handle);
            popup.on_close_popup(move || handle.hide("close-button"));
        }
        // Refresh timer (10 Hz while visible) + click-outside poll.
        // The timer lives on the event-loop thread; the popup is
        // refreshed from the live `DaemonApi` (no IPC round-trip —
        // the daemon IS the data source). The poll dismisses only
        // on a real click-away (see `tray_popup::PopupDismissPoll`:
        // the foreground baseline is re-sampled after the grace
        // and must move away for 3 consecutive polls, or the popup
        // must lose a foreground it held for 500 ms — never merely
        // for not being foreground, which a tray-shown window
        // usually never is). One `GetForegroundWindow()` read per
        // tick feeds both the foreground check and the poll (F9).
        // On Linux `popup_hwnd` is always `None`, so the poll
        // never dismisses there and Escape / the popup's own
        // buttons close it instead.
        let timer = slint::Timer::default();
        {
            let handle = Rc::clone(handle);
            let api = Arc::clone(api);
            timer.start(
                slint::TimerMode::Repeated,
                Duration::from_millis(100),
                move || {
                    if handle.visible.get() {
                        handle.refresh(&api);
                        if let Some(shown) = handle.shown_at.get() {
                            let foreground_now = tray_geometry::foreground_hwnd();
                            let hwnd = tray_geometry::popup_hwnd(tray_geometry::POPUP_WINDOW_TITLE);
                            if handle.tick(shown, foreground_now, hwnd) {
                                handle.hide("click-away");
                            }
                        }
                        // `popup_hwnd` None: the window cannot be
                        // found (yet) — the tick still retries
                        // activation, but the poll never dismisses.
                        // Stay open, never guess.
                    }
                },
            );
        }
        std::mem::forget(timer);
    }
    {
        // Left-click (§7.7): toggle the popup; without a popup the
        // click opens the UI, as before.
        // (d) Double-click guard (F9): a second tray click within
        // ~400 ms of an accepted toggle is ignored, so a
        // double-click is one show, not show-then-hide.
        let actions = Arc::clone(tray_actions);
        let popup = popup_handle.clone();
        let api = Arc::clone(api);
        let toggle_started = Instant::now();
        let toggle_guard = Rc::new(RefCell::new(TrayToggleGuard::new()));
        tray.on_tray_clicked(move || match &popup {
            Some(handle) => {
                if !toggle_guard.borrow_mut().accept(toggle_started.elapsed()) {
                    log_line(
                        verbose,
                        &format!(
                            "tray popup: tray click ignored (double-click guard) pid={}",
                            std::process::id()
                        ),
                    );
                    return;
                }
                if handle.visible.get() {
                    handle.hide("tray-click toggle");
                } else {
                    handle.show(&api);
                }
            }
            None => push(&actions, TrayAction::OpenMain),
        });
    }
    {
        let actions = Arc::clone(tray_actions);
        tray.on_open_main(move || push(&actions, TrayAction::OpenMain));
    }
    {
        let actions = Arc::clone(tray_actions);
        tray.on_toggled_send(move || push(&actions, TrayAction::ToggleSend));
    }
    {
        let actions = Arc::clone(tray_actions);
        tray.on_profile_selected(move |name| {
            push(&actions, TrayAction::ProfileSelected(name.to_string()));
        });
    }
    {
        let actions = Arc::clone(tray_actions);
        tray.on_launch_game(move |name| push(&actions, TrayAction::LaunchGame(name.to_string())));
    }
    {
        let actions = Arc::clone(tray_actions);
        tray.on_restart_wheel(move || push(&actions, TrayAction::RestartWheel));
    }
    {
        let actions = Arc::clone(tray_actions);
        tray.on_open_setup(move || push(&actions, TrayAction::OpenSetup));
    }
    {
        let actions = Arc::clone(tray_actions);
        tray.on_open_settings(move || push(&actions, TrayAction::OpenSettings));
    }
    {
        let shutdown = Arc::clone(shutdown);
        let data_dir = data_dir.to_path_buf();
        tray.on_quit_app(move || {
            // Tell the UI to quit too: it polls `<data_dir>/rsh-ui.quit`
            // in its frame timer (rsh-ui ui_lifecycle::take_quit_request)
            // and quits on the next tick. Without this a UI hidden to
            // the tray would keep running after "Quit RSH".
            if let Err(error) = write_ui_quit_request(&data_dir) {
                eprintln!("rsh-daemon: cannot write rsh-ui.quit: {error}");
            }
            shutdown.store(true, Ordering::Relaxed);
            let _ = slint::quit_event_loop();
        });
    }
    tray.show()?;
    // F11: NO `mem::forget` here any more. The old code leaked the
    // tray deliberately ("the Tauri shell leaked its TrayIcon"),
    // which meant `Drop` never ran and no `NIM_DELETE` was ever
    // sent — every dead daemon left a ghost icon until hover. Slint
    // event-loop ownership does NOT require the leak: the caller
    // (`main`) holds the returned handle across `run_event_loop()`
    // and drops it after the loop returns, which is exactly when
    // the icon must go away. The popup's refresh `Timer` is still
    // forgotten (its closure owns the popup handle); that leak is
    // separate from the tray icon and untouched here.
    Ok(tray)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rsh-daemon-quarantine-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn pipe_create_in_use_classification() {
        use std::io::{Error, ErrorKind};
        assert!(pipe_create_is_in_use(&Error::new(
            ErrorKind::PermissionDenied,
            "access denied"
        )));
        assert!(pipe_create_is_in_use(&Error::new(
            ErrorKind::AlreadyExists,
            "already exists"
        )));
        assert!(pipe_create_is_in_use(&Error::new(
            ErrorKind::AddrInUse,
            "pipe busy"
        )));
        assert!(!pipe_create_is_in_use(&Error::new(
            ErrorKind::NotFound,
            "no such pipe"
        )));
        assert!(!pipe_create_is_in_use(&Error::new(
            ErrorKind::InvalidInput,
            "bad argument"
        )));
    }

    #[test]
    fn pipe_in_use_backoff_ramps_then_caps() {
        assert_eq!(pipe_in_use_backoff(1), Duration::from_millis(100));
        assert_eq!(pipe_in_use_backoff(2), Duration::from_millis(200));
        assert_eq!(pipe_in_use_backoff(3), Duration::from_millis(400));
        assert_eq!(pipe_in_use_backoff(4), Duration::from_millis(800));
        assert_eq!(pipe_in_use_backoff(5), Duration::from_secs(1));
        assert_eq!(pipe_in_use_backoff(100), Duration::from_secs(1));
    }

    #[test]
    fn in_use_retry_is_capped_then_fatal() {
        // DS-14: a squatted pipe used to retry 1/s forever, never
        // fatal. The first in-use failure still retries; ~90 s of
        // continuous in-use failures is fatal; a success in
        // between resets the clock.
        let mut tracker = IpcBindTracker::new();
        assert!(
            matches!(tracker.record_failure(true), IpcBindRetry::RetryAfter(_)),
            "first in-use failure retries"
        );
        assert_eq!(tracker.in_use_failures, 1);
        // 89 s of continuous in-use: still retrying.
        tracker.in_use_since = Some(
            Instant::now()
                .checked_sub(Duration::from_secs(89))
                .expect("test clock is past boot + 89 s"),
        );
        assert!(
            matches!(tracker.record_failure(true), IpcBindRetry::RetryAfter(_)),
            "89 s in-use still retries"
        );
        // 91 s: fatal.
        tracker.in_use_since = Some(
            Instant::now()
                .checked_sub(Duration::from_secs(91))
                .expect("test clock is past boot + 91 s"),
        );
        assert!(
            matches!(tracker.record_failure(true), IpcBindRetry::Fatal),
            "91 s of continuous in-use must be fatal"
        );
        // A success resets the in-use clock: the next squat gets
        // a fresh ~90 s.
        tracker.record_success();
        assert!(tracker.in_use_since.is_none());
        assert!(
            matches!(tracker.record_failure(true), IpcBindRetry::RetryAfter(_)),
            "after a success the next in-use failure retries again"
        );
        // Real (non-in-use) failures keep the old 5-strike rule.
        let mut tracker = IpcBindTracker::new();
        for _ in 0..4 {
            assert!(matches!(
                tracker.record_failure(false),
                IpcBindRetry::RetryAfter(_)
            ));
        }
        assert!(matches!(tracker.record_failure(false), IpcBindRetry::Fatal));
    }

    #[test]
    fn ipc_bind_tracker_fatal_after_five_real_failures_only() {
        let mut tracker = IpcBindTracker::new();
        // "In use" never counts toward the five, however often it
        // fires (F1: the v2.0.2 loop killer — every post-connect
        // create failed in-use, and the old counter killed the
        // daemon for it).
        for _ in 0..50 {
            assert!(matches!(
                tracker.record_failure(true),
                IpcBindRetry::RetryAfter(_)
            ));
        }
        for _ in 1..5 {
            assert!(matches!(
                tracker.record_failure(false),
                IpcBindRetry::RetryAfter(_)
            ));
        }
        assert!(matches!(tracker.record_failure(false), IpcBindRetry::Fatal));
        // A success resets the count.
        tracker.record_success();
        assert!(matches!(
            tracker.record_failure(false),
            IpcBindRetry::RetryAfter(_)
        ));
    }

    #[test]
    fn ipc_fatal_shutdown_signals_both_flags_and_writes_fatal_status() {
        // F24: the fatal-bind signal must set BOTH flags — the
        // shutdown flag (main quits cleanly, F2) and the fatal flag
        // (main exits 1 after teardown, like DaemonHung) — and
        // write the machine-readable `IpcBindFailed` status. A
        // test that only checked the shutdown flag would still pass
        // with the old exit-0 behaviour.
        let dir = temp_dir("ipc-fatal");
        let shutdown = Arc::new(AtomicBool::new(false));
        let ipc_fatal = Arc::new(AtomicBool::new(false));
        signal_ipc_fatal_shutdown(&dir, "test detail", &shutdown, &ipc_fatal);
        assert!(
            shutdown.load(Ordering::Relaxed),
            "main must be asked to quit"
        );
        assert!(
            ipc_fatal.load(Ordering::Relaxed),
            "F24: main must exit(1) for a fatal bind, not 0"
        );
        let text = std::fs::read_to_string(dir.join(DAEMON_STATUS_FILE)).expect("read status");
        let value: serde_json::Value = serde_json::from_str(&text).expect("status is valid JSON");
        assert_eq!(value["status"], "fatal");
        assert_eq!(value["code"], "IpcBindFailed");
        assert_eq!(value["detail"], "test detail");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn quarantine_moves_corrupt_store_files_and_keeps_other_files() {
        let dir = temp_dir("moves");
        std::fs::write(dir.join("profiles.json"), "{corrupt").expect("write");
        std::fs::write(dir.join("settings.json"), "{corrupt").expect("write");
        std::fs::write(dir.join("setup-done"), "done\n").expect("write");
        let quarantined = quarantine_corrupt_store_files(&dir);
        assert_eq!(quarantined.len(), 2);
        assert!(!dir.join("profiles.json").exists());
        assert!(!dir.join("settings.json").exists());
        assert!(dir.join("setup-done").is_file());
        for path in &quarantined {
            assert!(path.is_file());
            assert!(
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.contains(".corrupt-"))
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn quarantine_is_a_noop_without_store_files() {
        let dir = temp_dir("noop");
        assert_eq!(quarantine_corrupt_store_files(&dir).len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn daemon_status_write_and_clear_round_trip() {
        let dir = temp_dir("status");
        write_daemon_status(&dir, "fatal", "IpcBindFailed", "test detail");
        let path = dir.join(DAEMON_STATUS_FILE);
        assert!(path.is_file(), "the status file is written");
        let text = std::fs::read_to_string(&path).expect("read status");
        let value: serde_json::Value = serde_json::from_str(&text).expect("status is valid JSON");
        assert_eq!(value["status"], "fatal");
        assert_eq!(value["code"], "IpcBindFailed");
        assert_eq!(value["detail"], "test detail");
        assert_eq!(
            value["pid"],
            serde_json::json!(std::process::id()),
            "the status names the dying process"
        );
        assert!(
            value["at_unix"].as_u64().is_some_and(|at| at > 0),
            "the status carries a timestamp: {value}"
        );
        clear_daemon_status(&dir);
        assert!(!path.exists(), "clear removes the file");
        clear_daemon_status(&dir); // clearing an absent file is fine
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ui_quit_request_file_is_written() {
        let dir = temp_dir("ui-quit");
        write_ui_quit_request(&dir).expect("write rsh-ui.quit");
        assert!(dir.join("rsh-ui.quit").exists(), "quit file exists");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn daemon_status_write_is_atomic_and_overwrites() {
        let dir = temp_dir("status-overwrite");
        write_daemon_status(&dir, "fatal", "DaemonHung", "first");
        write_daemon_status(&dir, "fatal", "IpcBindFailed", "second");
        let text = std::fs::read_to_string(dir.join(DAEMON_STATUS_FILE)).expect("read status");
        let value: serde_json::Value = serde_json::from_str(&text).expect("status is valid JSON");
        assert_eq!(value["code"], "IpcBindFailed");
        assert_eq!(value["detail"], "second");
        // No temp file is left behind.
        assert!(
            !std::fs::read_dir(&dir).expect("read dir").any(|entry| entry
                .ok()
                .and_then(|entry| entry.file_name().into_string().ok())
                .is_some_and(|name| name.contains(".tmp-"))),
            "no temp file left behind"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
