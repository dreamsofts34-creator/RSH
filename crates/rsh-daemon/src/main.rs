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
use std::time::{Duration, Instant};

use rsh_api::{AppApi, AppEvent, ConnEvent, ConnectionError, ManagerAction};
use rsh_app_core::diagnostics::append_startup_log_both;
use rsh_app_core::state::{AppStateCore, TransportKind};
use rsh_ipc::{IpcServer, serve_listener, socket_path};
use slint::ComponentHandle;
use tokio::sync::broadcast;

use daemon_api::{DaemonApi, connection_changed_event};
use rejoin::{RSH_SSID, rejoin_wifi};
use single_instance::{AcquireOutcome, acquire};
use tray_popup::{PopupGame, TrayPopupData, anchor_popup, next_profile};

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
                eprintln!("rsh-daemon: unknown argument: {other}");
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
                    spawn_ui(None);
                    log_line(verbose, "tray: Open RSH — spawned rsh-ui");
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
                TrayAction::OpenSettings => {
                    spawn_ui(Some("settings"));
                    log_line(verbose, "tray: Settings — spawned rsh-ui on Settings");
                }
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
        last_window_open = window_open;
        // Pairing rejoin (Phase 6): stash the Wi-Fi password while the
        // pairing window is open — it is cleared on activation — and
        // rejoin the board AP once when the session reaches Active.
        // The password never reaches a log line (see rejoin.rs).
        if window_open && let Some(password) = api.state.pairing_status_peek().password {
            pending_rejoin_password = Some(password);
        }
        let active_now = api.manager_state().is_active();
        if active_now
            && !last_active
            && let Some(password) = pending_rejoin_password.take()
        {
            match rejoin_wifi(RSH_SSID, &password) {
                Ok(()) => log_line(verbose, "pairing: rejoined RSH Wi-Fi after pairing"),
                Err(error) => log_line(verbose, &format!("pairing: Wi-Fi rejoin failed: {error}")),
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
    match rsh_ipc::bind_unix_socket(&path) {
        Ok(listener) => {
            log_line(
                verbose,
                &format!("daemon: IPC listening on {}", path.display()),
            );
            serve_listener(server, listener).await;
        }
        Err(error) => {
            log_line(verbose, &format!("daemon: IPC bind failed: {error}"));
        }
    }
}

/// Serve the IPC named pipe forever (Windows).
#[cfg(windows)]
async fn serve_ipc(server: Arc<IpcServer>, _data_dir: &std::path::Path, verbose: bool) {
    use tokio::net::windows::named_pipe::ServerOptions;
    log_line(verbose, "daemon: IPC listening on \\\\.\\pipe\\rsh-daemon");
    loop {
        let pipe = match ServerOptions::new()
            .first_pipe_instance(true)
            .create(rsh_ipc::IPC_PIPE_NAME)
        {
            Ok(pipe) => pipe,
            Err(error) => {
                log_line(verbose, &format!("daemon: IPC pipe create failed: {error}"));
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
    let data_dir = args
        .data_dir
        .clone()
        .or_else(rsh_app_core::data_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    log_line(
        verbose,
        &format!(
            "daemon: starting rsh-daemon v{} (data dir {})",
            rsh_api::app_version(),
            data_dir.display()
        ),
    );

    // Single-instance: a second launch signals the running daemon
    // (IPC ping — the UI open hook lands in Phase 3) and exits.
    let lock = match acquire(&data_dir) {
        Ok(AcquireOutcome::First(lock)) => lock,
        Ok(AcquireOutcome::AlreadyRunning(pid)) => {
            let socket = socket_path(&data_dir);
            let alive = std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .ok()?;
                runtime.block_on(async move {
                    let client = rsh_ipc::IpcClient::connect_unix(&socket).await.ok()?;
                    client.ping().await.ok()
                })
            })
            .join()
            .ok()
            .flatten()
            .unwrap_or(false);
            eprintln!("rsh-daemon: already running (pid {pid:?}, IPC answers: {alive}) — exiting");
            std::process::exit(0);
        }
        Err(error) => {
            eprintln!("rsh-daemon: cannot acquire single-instance lock: {error}");
            log_line(
                verbose,
                &format!("daemon: FATAL single-instance lock: {error}"),
            );
            std::process::exit(1);
        }
    };

    // Open the app state (repairs a corrupt store from backups).
    let state = match AppStateCore::open_with_null_sink_repair(&data_dir) {
        (Ok(state), repair_note) => {
            if let Some(note) = repair_note {
                log_line(verbose, &format!("daemon: store repaired: {note}"));
            }
            Arc::new(state)
        }
        (Err(error), _) => {
            eprintln!("rsh-daemon: cannot open profile store: {error}");
            log_line(verbose, &format!("daemon: FATAL store open: {error}"));
            std::process::exit(1);
        }
    };
    let api = DaemonApi::new(Arc::clone(&state));
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
            eprintln!("rsh-daemon: cannot build runtime: {error}");
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
    if let Ok(exe) = std::env::current_exe() {
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

    // --check: verify IPC answers, then exit 0 (no tray needed).
    if args.check {
        let socket = socket_path(&data_dir);
        let ok = runtime.block_on(async move {
            // Give the listener a moment to bind.
            for _ in 0..20 {
                if let Ok(client) = rsh_ipc::IpcClient::connect_unix(&socket).await
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
    let tray_ok = if args.no_tray {
        log_line(verbose, "daemon: tray disabled (--no-tray)");
        false
    } else {
        match create_tray(&api, &tray_actions, &shutdown, verbose) {
            Ok(()) => {
                log_line(verbose, "daemon: tray icon registered");
                true
            }
            Err(error) => {
                // Linux without a tray host (Xvfb / headless) fails
                // here — expected (spike §3). The daemon keeps
                // running headless; Windows is the target.
                log_line(
                    verbose,
                    &format!("daemon: tray unavailable ({error}) — running headless"),
                );
                false
            }
        }
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
        let (x, y) = anchor_popup(tray_geometry::tray_icon_rect(), tray_geometry::work_area());
        self.popup
            .window()
            .set_position(slint::PhysicalPosition::new(x, y));
        self.refresh(api);
        if self.popup.show().is_ok() {
            self.visible.set(true);
            self.shown_at.set(Some(Instant::now()));
        }
    }

    /// Hide the popup (click-outside, Escape, or an action taken).
    fn hide(&self) {
        let _ = self.popup.hide();
        self.visible.set(false);
        self.shown_at.set(None);
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
) -> Result<(), slint::PlatformError> {
    let tray = RshTray::new()?;
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
        // the daemon IS the data source). On Windows the popup
        // closes when another window takes the foreground (500 ms
        // grace so showing the popup cannot instantly close it);
        // on Linux there is no foreground query, so Escape and the
        // popup's own buttons close it there.
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
                        #[cfg(windows)]
                        {
                            if handle
                                .shown_at
                                .get()
                                .is_some_and(|shown| shown.elapsed() > Duration::from_millis(500))
                                && let Some(hwnd) = tray_geometry::popup_hwnd("RSH Wheel — Tray")
                                && tray_geometry::is_foreground(hwnd) == Some(false)
                            {
                                handle.hide();
                            }
                        }
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
        tray.on_open_settings(move || push(&actions, TrayAction::OpenSettings));
    }
    {
        let shutdown = Arc::clone(shutdown);
        tray.on_quit_app(move || {
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
