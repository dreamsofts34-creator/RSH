//! The Tauri-managed app state (Phase G0, engine wiring Phase G).
//!
//! A thin shell over [`AppStateCore`](rsh_app_core::state::AppStateCore):
//! it owns the snapshot emitter thread, which samples the core at 30 Hz
//! and emits `rsh://snapshot` / `rsh://button-press` into the WebView,
//! starts the engine task at launch (idle + best-effort UDP connect),
//! and it runs the real shutdown-neutral path on exit. All state logic
//! and tests live in `rsh-app-core`; this file cannot be compiled in
//! the Linux sandbox (Tauri needs WebKitGTK) — Windows CI compiles it.

use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicBool, Ordering},
};
use std::thread::JoinHandle;

use rsh_app_core::state::TransportKind;
use rsh_app_core::state::{AppStateCore, BUTTON_PRESS_EVENT, SNAPSHOT_EVENT, SnapshotEmit};
use rsh_app_core::system::ShutdownOutcome;
use tauri::{AppHandle, Emitter};

/// Locks a mutex, recovering the guard if a previous holder panicked
/// while holding it (same rationale as `rsh_app_core::state`'s helper:
/// a poisoned lock must not turn one failure into a panic in every
/// later command).
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The Tauri-managed app state: the testable core plus the snapshot
/// emitter thread.
pub struct AppState {
    /// Engine metrics + loop-rate tracker + output backend + game
    /// launcher + profile store (see `rsh_app_core::state`).
    core: Arc<AppStateCore>,
    /// Set on shutdown so the emitter thread stops emitting.
    emitter_stop: Arc<AtomicBool>,
    /// The emitter thread, once started.
    emitter_thread: Mutex<Option<JoinHandle<()>>>,
    /// Startup notice for the UI (H3): a corrupt-store repair note
    /// or an autostart-repair failure, shown once in the shell as a
    /// dismissible banner. `None` on a clean start.
    startup_notice: Mutex<Option<String>>,
}

impl AppState {
    /// Wraps a core state. The emitter thread is not started yet —
    /// call [`AppState::start_snapshot_emitter`] from `setup`.
    #[must_use]
    pub fn new(core: Arc<AppStateCore>) -> Self {
        Self {
            core,
            emitter_stop: Arc::new(AtomicBool::new(false)),
            emitter_thread: Mutex::new(None),
            startup_notice: Mutex::new(None),
        }
    }

    /// The testable core — what every command wrapper reads/writes.
    #[must_use]
    pub fn core(&self) -> &Arc<AppStateCore> {
        &self.core
    }

    /// Records the startup notice (corrupt-store repair / autostart
    /// repair outcome) for the UI to show once.
    pub fn set_startup_notice(&self, notice: String) {
        *lock(&self.startup_notice) = Some(notice);
    }

    /// The startup notice, if startup had anything to report.
    #[must_use]
    pub fn startup_notice(&self) -> Option<String> {
        lock(&self.startup_notice).clone()
    }

    /// Starts the 30 Hz snapshot emitter thread (once; later calls are
    /// no-ops). The thread samples the core's latest snapshot and
    /// drains its button-press queue, emitting both into the WebView;
    /// a failed emit is ignored (the frontend re-reads via
    /// `get_snapshot` — a dropped event is never retried into a
    /// burst). If the thread cannot be spawned, this logs loudly and
    /// carries on without live snapshots (`get_snapshot` still works).
    pub fn start_snapshot_emitter(&self, app: &AppHandle) {
        if lock(&self.emitter_thread).is_some() {
            return;
        }
        let core = Arc::clone(&self.core);
        let stop = Arc::clone(&self.emitter_stop);
        let handle = app.clone();
        match std::thread::Builder::new()
            .name("rsh-snapshot-emitter".to_string())
            .spawn(move || {
                AppStateCore::run_snapshot_emitter(
                    &core,
                    |emit| match emit {
                        SnapshotEmit::Snapshot(snapshot) => {
                            let _ = handle.emit(SNAPSHOT_EVENT, snapshot);
                        }
                        SnapshotEmit::ButtonPress(event) => {
                            let _ = handle.emit(BUTTON_PRESS_EVENT, event);
                        }
                        SnapshotEmit::AppEvent(name, payload) => {
                            let _ = handle.emit(name.as_str(), payload);
                        }
                    },
                    &stop,
                );
            }) {
            Ok(thread) => *lock(&self.emitter_thread) = Some(thread),
            Err(error) => eprintln!(
                "[RSH] failed to spawn snapshot emitter thread: {error}; \
                 live snapshots unavailable, get_snapshot still works"
            ),
        }
    }

    /// Starts the engine task (idle) and best-effort connects the UDP
    /// transport (board at the configured `esp_ip`, Phase G gate
    /// wiring). The UDP connect runs the H5 session activator on the
    /// forwarder (flag-1 HELO → `HELLO` → `pair`/`resume` with a
    /// signed `GET_CAPS` first → session gate opens only on success),
    /// so the startup path is a session-activation path, not just a
    /// transport path. Neither step can fail startup: a failure is
    /// logged loudly and the app keeps running — snapshots are still
    /// served and the output backend is still switchable, wheel input
    /// just does not flow until a transport connects and a session
    /// authenticates. The engine status detail names the reason (see
    /// `AppStateCore::engine_status`).
    pub fn start_engine(&self) {
        if let Err(error) = self.core.start_engine() {
            eprintln!(
                "[RSH] engine failed to start: {error}; snapshots and output switching still work, wheel input will not flow"
            );
            return;
        }
        if let Err(error) = self.core.connect_transport(TransportKind::Udp) {
            eprintln!(
                "[RSH] UDP transport connect failed: {error}; engine idle until a transport connects"
            );
        }
    }

    /// Real shutdown: stops the emitter, stops the engine with a
    /// bounded wait (≤ ~5 s — the engine's own `finish()` sends a
    /// final neutral through the sink), and sends neutral through the
    /// installed backend. The outcome reports honestly whether neutral
    /// was sent — with the `NullSink` installed: attempted, not sent
    /// (D-F3). The emitter thread is not joined: the process is
    /// exiting, and the thread wakes within one 33 ms tick of the stop
    /// flag; joining here could delay exit. A timed-out engine stop
    /// never blocks exit past the bound (see
    /// `AppStateCore::stop_engine`).
    #[must_use]
    pub fn shutdown(&self) -> ShutdownOutcome {
        self.emitter_stop.store(true, Ordering::Relaxed);
        let _ = self.core.stop_engine();
        self.core.shutdown()
    }
}
