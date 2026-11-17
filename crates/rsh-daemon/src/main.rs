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

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rsh_api::{AppApi, AppEvent, ConnEvent, ConnectionError, ManagerAction};
use rsh_app_core::diagnostics::{append_startup_log_both, log_fatal};
use rsh_app_core::state::{AppStateCore, BindingAppAction, TransportKind};
#[cfg(unix)]
use rsh_ipc::serve_listener;
use rsh_ipc::{IpcServer, socket_path};
use slint::ComponentHandle;
use tokio::sync::broadcast;

use daemon_api::{DaemonApi, connection_changed_event};
use rejoin::{RSH_SSID, rejoin_wifi};
use single_instance::{AcquireOutcome, acquire};
use tray_popup::{PopupGame, TrayPopupData, anchor_popup, next_profile, should_dismiss_popup};

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
    let mut first_active_seen = false;
    let mut pending_rejoin_password: Option<String> = None;
    let mut logged_lines = 0usize;
    let mut ticker = tokio::time::interval(Duration::from_millis(250));
    loop {
        ticker.tick().await;
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
        // Drain the pairing log into the diagnostics file (one line
        // per transition — the buffer is capped at 200, so a shrink
        // means the cap dropped old lines: restart from the top).
        let lines = api.state.pairing_log_lines();
        if lines.len() < logged_lines {
            logged_lines = 0;
        }
        for line in &lines[logged_lines..] {
            log_line(verbose, line);
        }
        logged_lines = lines.len();
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
                TrayAction::RestartWheel => match api.state.restart_wheel() {
                    Ok(()) => log_line(verbose, "tray: wheel restart requested"),
                    Err(error) => {
                        log_line(verbose, &format!("tray: wheel restart failed: {error}"));
                    }
                },
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
            if pairing_in_flight || !first_active_seen {
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
            first_active_seen = true;
            if let Some(password) = pending_rejoin_password.take() {
                match rejoin_wifi(RSH_SSID, &password) {
                    Ok(()) => log_line(verbose, "pairing: rejoined RSH Wi-Fi after pairing"),
                    Err(error) => {
                        log_line(verbose, &format!("pairing: Wi-Fi rejoin failed: {error}"));
                    }
                }
            }
        }
        if !window_open && !active_now {
            pending_rejoin_password = None;
        }
        last_active = active_now;
        // Heartbeat watchdog (C-04): 2 s silence in Active →
        // rebuild + re-activate.
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
fn spawn_ui(page: Option<&str>) {
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
    if let Err(error) = command.spawn() {
        eprintln!("rsh-daemon: cannot start UI ({}): {error}", exe.display());
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
            spawn_ui(Some(page));
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
            spawn_ui(Some(page));
        } else {
            log_line(
                verbose,
                &format!("tray: UI already running (pid {pid}) — not spawning a second copy"),
            );
        }
        return true;
    }
    // 3. No UI at all → launch it exactly like a double-click.
    spawn_ui(page);
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

/// Serve the IPC listener forever (Unix socket on Linux, named
/// pipe on Windows).
#[cfg(unix)]
async fn serve_ipc(server: Arc<IpcServer>, data_dir: &std::path::Path, verbose: bool) {
    let path = socket_path(data_dir);
    // A bind can fail transiently (the data dir is still being
    // created, a stale socket is being reaped) — retry a few times
    // instead of silently running with no IPC, which would leave
    // the UI on simulated data forever with no further trace.
    for attempt in 1..=5 {
        match rsh_ipc::bind_unix_socket(&path) {
            Ok(listener) => {
                log_line(
                    verbose,
                    &format!("daemon: IPC listening on {}", path.display()),
                );
                serve_listener(server, listener).await;
                return;
            }
            Err(error) => {
                log_line(
                    verbose,
                    &format!("daemon: IPC bind failed (attempt {attempt}/5): {error}"),
                );
                if attempt < 5 {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }
    // M8 (v2.0.2): five failed binds are a VISIBLE fatal state,
    // not a quiet headless run — the daemon exits(1) after leaving
    // the machine-readable fatal status + the dual-location fatal
    // log, so the UI can say the daemon is dead instead of silently
    // running on simulated data.
    let detail = format!(
        "daemon IPC bind failed after 5 attempts (socket was {}) — the daemon cannot serve the UI",
        path.display()
    );
    write_daemon_status(data_dir, "fatal", "IpcBindFailed", &detail);
    let _ = log_fatal(&format!("daemon: {detail}"));
    std::process::exit(1);
}

/// Serve the IPC named pipe forever (Windows).
#[cfg(windows)]
async fn serve_ipc(server: Arc<IpcServer>, data_dir: &std::path::Path, verbose: bool) {
    use tokio::net::windows::named_pipe::ServerOptions;
    log_line(verbose, "daemon: IPC listening on \\\\.\\pipe\\rsh-daemon");
    // A pipe create can fail transiently — retry, and count
    // CONSECUTIVE failures: five in a row are a visible fatal
    // (M8, v2.0.2), exactly like the Unix bind path, instead of
    // the old infinite retry that left the daemon headless with no
    // trace. A successful create resets the count; a failed client
    // connect just loops for the next client.
    let mut create_failures = 0u32;
    loop {
        let pipe = match ServerOptions::new()
            .first_pipe_instance(true)
            .create(rsh_ipc::IPC_PIPE_NAME)
        {
            Ok(pipe) => {
                create_failures = 0;
                pipe
            }
            Err(error) => {
                create_failures += 1;
                log_line(
                    verbose,
                    &format!(
                        "daemon: IPC pipe create failed (attempt {create_failures}/5): {error}"
                    ),
                );
                if create_failures >= 5 {
                    let detail =
                        "daemon IPC bind failed after 5 attempts — the daemon cannot serve the UI";
                    write_daemon_status(data_dir, "fatal", "IpcBindFailed", detail);
                    let _ = log_fatal(&format!("daemon: {detail}"));
                    std::process::exit(1);
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        };
        // NOTE (OPEN, Windows verification): the pipe currently uses
        // the default security descriptor. An explicit
        // current-user-only DACL is a Phase-7 hardening item — see
        // docs/V2-PHASE-2-GATE.md. The pipe is local-only and the
        // name is per-user via the single-instance mutex scope.
        if pipe.connect().await.is_err() {
            continue;
        }
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
    log_line(
        verbose,
        &format!(
            "daemon: starting rsh-daemon v{} (data dir {})",
            rsh_api::app_version(),
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
            // The lock holder may be a daemon that is still starting
            // (IPC not bound yet) — give its ping a few tries before
            // declaring it dead.
            let alive = std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .ok()?;
                Some(runtime.block_on(async move {
                    for _ in 0..3 {
                        if let Ok(client) = rsh_ipc::IpcClient::connect(&socket).await
                            && let Ok(true) = client.ping().await
                        {
                            return true;
                        }
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                    false
                }))
            })
            .join()
            .ok()
            .flatten()
            .unwrap_or(false);
            if alive {
                log_line(
                    verbose,
                    &format!("daemon: already running (pid {pid:?}, IPC answers) — exiting"),
                );
                std::process::exit(0);
            }
            // The lock is held but the daemon does not answer IPC:
            // hung, or a stale lock the single-instance check could
            // not reclaim. Never exit 0 (success) for a broken
            // daemon, and never leave only an invisible stderr line:
            // a UI that spawned us falls back to simulated data, so
            // this failure must be in the dual-location log AND in
            // the machine-readable status file (L8, v2.0.2) — the UI
            // reads the file and tells the user the running daemon
            // is hung instead of silently simulating.
            let detail = format!(
                "already running (pid {pid:?}) but not responding to IPC after 3 tries — the running daemon is hung or its IPC is dead; not taking over"
            );
            write_daemon_status(&data_dir, "fatal", "DaemonHung", &detail);
            let _ = log_fatal(&format!("daemon: {detail}"));
            std::process::exit(1);
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
        runtime.spawn(async move {
            serve_ipc(server, &dir, verbose).await;
        });
    }
    {
        let api = Arc::clone(&api);
        let event_tx = event_tx.clone();
        let tray_actions = Arc::clone(&tray_actions);
        let shutdown = Arc::clone(&shutdown);
        runtime.spawn(async move {
            manager_loop(api, event_tx, tray_actions, shutdown, verbose).await;
        });
    }

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

    // D-7 lazy-start (Phase 7): activate the startup output backend.
    // vJoy activates now; HIDMaestro stays deferred until the user
    // turns "Send to game" on, so autostart never pops a UAC prompt
    // at login. This never fails the launch (see the outcome log).
    let backend = state.activate_startup_backend();
    log_line(
        verbose,
        &format!(
            "daemon: startup backend requested={:?} active={:?} fell_back={} reason={:?}",
            backend.requested, backend.active, backend.fell_back, backend.reason
        ),
    );

    // Autostart sync (Phase 7): `settings.autostart` drives the
    // Windows Run key / Linux .desktop file. A stale v1 entry
    // (pointing at the old `RSH.exe`) is migrated to the daemon exe
    // when autostart is on, and removed when it is off — the old exe
    // no longer exists after the v2.0 install. Best-effort: a failed
    // write is logged, never fatal.
    //
    // Heal a stale persisted `autostart = false` FIRST: the v1
    // default was false, so a machine upgraded from v1.0.2 carries
    // `settings.json` with autostart off — and the plain sync below
    // would DELETE the Run key the installer just wrote, so the
    // daemon would never autostart again (the v2.0.0
    // no-tray-after-reboot report). A Run entry that already points
    // at this exe cannot come from a v2 Settings toggle-off (that
    // deletes the entry immediately), so its presence is newer
    // evidence than the stale persisted flag: adopt it and persist
    // the fix. Only when persisting the adoption fails is the sync
    // skipped this launch, so a stale flag can never delete a live
    // entry. (A fresh-store default flip alone cannot heal this —
    // it only affects stores with no `settings.json` yet.)
    let mut skip_autostart_sync = false;
    if let Ok(exe) = std::env::current_exe() {
        if !state.settings().autostart && tray_autostart::autostart_enabled(&exe) {
            let mut settings = state.settings();
            settings.autostart = true;
            match state.set_settings(settings) {
                Ok(_) => log_line(
                    verbose,
                    "daemon: autostart adopted from the existing Run entry (persisted autostart was stale off) — Run key kept",
                ),
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
            match tray_autostart::sync_autostart(state.settings().autostart, &exe) {
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
    let tray_ok = if args.no_tray {
        log_line(verbose, "daemon: tray disabled (--no-tray)");
        false
    } else {
        let mut tray_ok = false;
        for attempt in 1..=6 {
            match create_tray(&api, &tray_actions, &shutdown, verbose, &data_dir) {
                Ok(()) => {
                    log_line(verbose, "daemon: tray icon registered");
                    tray_ok = true;
                    break;
                }
                Err(error) => {
                    log_line(
                        verbose,
                        &format!("daemon: tray unavailable (attempt {attempt}/6: {error})"),
                    );
                    if attempt < 6 {
                        std::thread::sleep(Duration::from_secs(2));
                    }
                }
            }
        }
        if !tray_ok {
            log_line(
                verbose,
                "daemon: tray unavailable after 6 attempts — running headless",
            );
        }
        tray_ok
    };

    if tray_ok {
        // The event loop runs until Quit is picked (or the process
        // is killed). Tray callbacks run on this thread.
        if let Err(error) = slint::run_event_loop() {
            log_line(verbose, &format!("daemon: event loop error: {error}"));
        }
    } else {
        // No tray, no event loop: park the main thread until killed.
        // (Ctrl-C / SIGTERM end the process; the pid file is
        // reclaimed by the next launch's /proc check.)
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    }

    shutdown.store(true, Ordering::Relaxed);
    let _ = state.stop_engine();
    lock.release();
}

/// The tray popup window + its visibility state. Lives on the Slint
/// event-loop thread only (`Rc` / `Cell` — never sent anywhere).
struct PopupHandle {
    popup: TrayPopup,
    visible: Cell<bool>,
    shown_at: Cell<Option<Instant>>,
    /// The popup was the foreground window at least once since it
    /// was shown (A4): losing the foreground after that is a real
    /// click-away. Never-foreground is NOT a click-away — see
    /// [`tray_popup::should_dismiss_popup`].
    was_foreground: Cell<bool>,
    /// The foreground window when the popup was shown (A4): the
    /// poll dismisses only when the foreground MOVED AWAY from
    /// this window. `None` on Linux (no foreground query).
    foreground_at_show: Cell<Option<isize>>,
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
        // foreground moves away from THIS window.
        self.was_foreground.set(false);
        self.foreground_at_show
            .set(tray_geometry::foreground_hwnd());
        let (x, y) = anchor_popup(tray_geometry::tray_icon_rect(), tray_geometry::work_area());
        self.popup
            .window()
            .set_position(slint::PhysicalPosition::new(x, y));
        self.refresh(api);
        if self.popup.show().is_ok() {
            self.visible.set(true);
            self.shown_at.set(Some(Instant::now()));
            // H-2: activate the popup — a tray-shown window
            // usually never becomes foreground on its own
            // (foreground lock), which is what made the old poll
            // dismiss it ~1 s after the click. Best-effort: a
            // failed activation leaves the popup visible but
            // inactive, and the fixed poll
            // (`should_dismiss_popup`) still keeps it open until
            // a real click-away.
            let _ = tray_geometry::activate_popup_window(tray_geometry::POPUP_WINDOW_TITLE);
        }
    }

    /// Hide the popup (click-outside, Escape, or an action taken).
    fn hide(&self) {
        let _ = self.popup.hide();
        self.visible.set(false);
        self.shown_at.set(None);
        self.was_foreground.set(false);
        self.foreground_at_show.set(None);
    }
}

/// Create + show the tray icon, wiring its callbacks, and create the
/// tray popup (§7.7) alongside it. The tray lives on the Slint
/// event loop (main thread). A popup failure never takes the tray
/// down: left-click then falls back to opening the UI.
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
) -> Result<(), slint::PlatformError> {
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
            was_foreground: Cell::new(false),
            foreground_at_show: Cell::new(None),
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
                handle.hide();
                push(&actions, TrayAction::OpenMain);
            });
        }
        {
            let actions = Arc::clone(tray_actions);
            let popup = &handle.popup;
            let handle = Rc::clone(handle);
            popup.on_restart_wheel(move || {
                handle.hide();
                push(&actions, TrayAction::RestartWheel);
            });
        }
        {
            let actions = Arc::clone(tray_actions);
            let popup = &handle.popup;
            let handle = Rc::clone(handle);
            popup.on_launch_game(move |id| {
                handle.hide();
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
            popup.on_close_popup(move || handle.hide());
        }
        // Refresh timer (10 Hz while visible) + click-outside poll.
        // The timer lives on the event-loop thread; the popup is
        // refreshed from the live `DaemonApi` (no IPC round-trip —
        // the daemon IS the data source). The poll dismisses only
        // on a real click-away (see
        // `tray_popup::should_dismiss_popup`: only when the
        // foreground MOVES AWAY from the window it had at show
        // time, or the popup loses a foreground it had — never
        // merely for not being foreground, which a tray-shown
        // window usually never is). On Linux `popup_hwnd` is
        // always `None`, so the poll never dismisses there and
        // Escape / the popup's own buttons close it instead.
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
                        if let Some(shown) = handle.shown_at.get()
                            && let Some(hwnd) =
                                tray_geometry::popup_hwnd(tray_geometry::POPUP_WINDOW_TITLE)
                        {
                            let is_foreground = tray_geometry::is_foreground(hwnd);
                            if is_foreground {
                                handle.was_foreground.set(true);
                            }
                            if should_dismiss_popup(
                                shown.elapsed(),
                                is_foreground,
                                handle.was_foreground.get(),
                                tray_geometry::foreground_hwnd(),
                                handle.foreground_at_show.get(),
                            ) {
                                handle.hide();
                            }
                        }
                        // `popup_hwnd` None: the window cannot be
                        // found (yet) — stay open, never guess.
                    }
                },
            );
        }
        std::mem::forget(timer);
    }
    {
        // Left-click (§7.7): toggle the popup; without a popup the
        // click opens the UI, as before.
        let actions = Arc::clone(tray_actions);
        let popup = popup_handle.clone();
        let api = Arc::clone(api);
        tray.on_tray_clicked(move || match &popup {
            Some(handle) => {
                if handle.visible.get() {
                    handle.hide();
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
    // Keep the tray alive for the process lifetime: leak it the way
    // the Tauri shell leaked its TrayIcon (it must outlive this fn).
    std::mem::forget(tray);
    Ok(())
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
