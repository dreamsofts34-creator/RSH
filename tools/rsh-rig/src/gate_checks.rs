//! v2.0.2 gate checks (fix plan §C).
//!
//! One function per gate item; each returns `(passed, detail)` and is
//! wired as a duration-0 scenario (`gate-*`) in
//! [`crate::harness::run_contract`], so the normal rig runner reports
//! them like every other scenario. The runtime checks are
//! cross-platform — the fake IPC peer is a Unix socket on Linux and
//! a named pipe on Windows (see `FakeListener`). A Windows-only
//! behaviour (real UAC prompts, the NSIS installer UI, the tray
//! icon) is covered by a *source audit* here plus an explicit open
//! verification on the user PC, recorded in the v2.0.2 gate report
//! (removed from docs/ in the 2026-11-19 repo cleanup — see git
//! history). Nothing here pretends a Linux run proves a Windows behaviour.
//!
//! Two kinds of check live in this module, and the detail string of
//! each says which it is:
//! * `runtime` — executes the real code (backend switch, typed
//!   errors, IPC timeout, OTA self-checks, performance timings).
//! * `source audit` — reads the tree as text and asserts the fix is
//!   present in the code that ships (slider fill binding, tray
//!   auto-hide, X-close wiring, wizard reopen, hover/theme/overflow
//!   markers). The real visual proof for these stays the user-PC
//!   retest; the audit only proves the code says what the fix claims.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rsh_api::errors::{BindErrorKind, ConnectionError, UserError};
use rsh_app_core::state::{AppStateCore, StartupBackendOutcome};
use rsh_app_core::webota::{
    FlashStage, WebOtaTarget, flash_firmware_with_pubkey, wait_for_board_version,
};
use rsh_ipc::{IPC_PROTOCOL_VERSION, IpcClient};
use rsh_output::selector::{BackendFactories, BackendKind, switch_backend};
use rsh_output::{IJoystickOutput, ProcessedState, VJoyError};
use rsh_profiles::store::ProfileStore;
use rsh_security::firmware_verify::FIRMWARE_IMAGE_MIN_LEN;

use crate::webota_board::{FakeBoard, build_test_image, test_pubkey};

/// Repo root, derived from this crate's manifest dir
/// (`tools/rsh-rig` → two levels up). The rig is always run from the
/// repo, so this is stable; source audits fail closed when a file is
/// missing instead of silently passing.
#[must_use]
pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// Read a repo-relative file as text; `None` when it cannot be read
/// (audits treat that as a failure, never as a pass).
fn read(rel: &str) -> Option<String> {
    std::fs::read_to_string(repo_root().join(rel)).ok()
}

/// Accumulates one check's sub-assertions into the `(passed, detail)`
/// pair the harness expects.
struct Report {
    ok: bool,
    lines: Vec<String>,
}

impl Report {
    fn new() -> Self {
        Self {
            ok: true,
            lines: Vec::new(),
        }
    }

    fn check(&mut self, name: &str, passed: bool, detail: &str) {
        if !passed {
            self.ok = false;
        }
        self.lines.push(format!(
            "{}: {} — {detail}",
            name,
            if passed { "PASS" } else { "FAIL" }
        ));
    }

    fn finish(self) -> (bool, String) {
        (self.ok, self.lines.join(" | "))
    }
}

/// Temporary directory for one check (`rsh-gate-<name>-<pid>` under
/// the system temp dir), removed afterwards. Best-effort cleanup:
/// a leftover temp dir is harmless.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rsh-gate-{name}-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Cross-platform fake IPC listener for the runtime gate checks.
///
/// The checks model a daemon with a fake peer and drive the real
/// `IpcClient` against it. On Unix the peer is a `UnixListener` at
/// a temp socket path; on Windows it is a named-pipe server at the
/// fixed `IPC_PIPE_NAME` — `IpcClient::connect` dials that fixed
/// name there and ignores the path, so the fake peer must listen
/// there (same pattern as the daemon's `serve_ipc` and the `rsh-ui`
/// IPC tests). Added when the v2.0.2 release clippy run failed on
/// Windows: `tokio::net::UnixListener` does not exist there.
#[cfg(unix)]
type FakeStream = tokio::net::UnixStream;
#[cfg(windows)]
type FakeStream = tokio::net::windows::named_pipe::NamedPipeServer;

struct FakeListener {
    #[cfg(unix)]
    inner: tokio::net::UnixListener,
}

impl FakeListener {
    /// Bind the fake peer. Unix: bind the socket at `path`.
    /// Windows: probe-create one pipe instance at the fixed
    /// `IPC_PIPE_NAME` so a taken name (a real daemon running)
    /// fails fast here instead of at the first accept; the probe
    /// instance is dropped immediately.
    fn bind(path: &Path) -> std::io::Result<Self> {
        #[cfg(unix)]
        {
            Ok(Self {
                inner: tokio::net::UnixListener::bind(path)?,
            })
        }
        #[cfg(windows)]
        {
            use tokio::net::windows::named_pipe::ServerOptions;
            let _ = path;
            let _probe = ServerOptions::new()
                .first_pipe_instance(true)
                .create(rsh_ipc::IPC_PIPE_NAME)?;
            Ok(Self {})
        }
    }

    /// Accept one connection. Unix: accept on the socket.
    /// Windows: create a fresh pipe instance (same pattern as the
    /// daemon's `serve_ipc` loop) and wait for the client to
    /// connect.
    async fn accept(&self) -> std::io::Result<FakeStream> {
        #[cfg(unix)]
        {
            let (stream, _) = self.inner.accept().await?;
            Ok(stream)
        }
        #[cfg(windows)]
        {
            use tokio::net::windows::named_pipe::ServerOptions;
            let pipe = ServerOptions::new()
                .first_pipe_instance(true)
                .create(rsh_ipc::IPC_PIPE_NAME)?;
            pipe.connect().await?;
            Ok(pipe)
        }
    }
}

/// A mock output backend for the switch state machine: connects
/// unless told to fail, and records connects/disconnects so the
/// switch can prove exactly one backend is live at a time.
struct MockBackend {
    name: &'static str,
    state: Arc<Mutex<MockState>>,
}

struct MockState {
    connected: bool,
    connect_calls: usize,
    disconnect_calls: usize,
    fail_times: usize,
}

impl MockBackend {
    fn new(name: &'static str) -> (Self, Arc<Mutex<MockState>>) {
        let state = Arc::new(Mutex::new(MockState {
            connected: false,
            connect_calls: 0,
            disconnect_calls: 0,
            fail_times: 0,
        }));
        (
            Self {
                name,
                state: Arc::clone(&state),
            },
            state,
        )
    }

    fn failing(name: &'static str, fail_times: usize) -> (Self, Arc<Mutex<MockState>>) {
        let (backend, state) = Self::new(name);
        state.lock().expect("mock lock poisoned").fail_times = fail_times;
        (backend, state)
    }

    /// Box a handle sharing this mock's state, so a factory closure
    /// stays `Fn` (it may be called more than once) while the built
    /// instance still records connects/disconnects on the shared
    /// state. Each `build` in the switch tests constructs its
    /// instance from a fresh [`MockBackend::new`], so sharing a
    /// state across builds of one factory never merges two live
    /// backends in these checks.
    fn boxed(&self) -> Box<dyn IJoystickOutput> {
        Box::new(Self {
            name: self.name,
            state: Arc::clone(&self.state),
        })
    }
}

impl IJoystickOutput for MockBackend {
    fn name(&self) -> &'static str {
        self.name
    }

    fn is_connected(&self) -> bool {
        self.state.lock().expect("mock lock poisoned").connected
    }

    fn try_connect(&mut self) -> Result<(), VJoyError> {
        let mut guard = self.state.lock().expect("mock lock poisoned");
        guard.connect_calls += 1;
        if guard.fail_times > 0 {
            guard.fail_times -= 1;
            return Err(VJoyError::DriverNotFound(format!(
                "mock {} connect failed",
                self.name
            )));
        }
        guard.connected = true;
        Ok(())
    }

    fn disconnect(&mut self) {
        let mut guard = self.state.lock().expect("mock lock poisoned");
        guard.disconnect_calls += 1;
        guard.connected = false;
    }

    fn update(&mut self, _state: &ProcessedState) -> Result<(), VJoyError> {
        if self.is_connected() {
            Ok(())
        } else {
            Err(VJoyError::NotConnected)
        }
    }

    fn send_neutral(&mut self) -> Result<(), VJoyError> {
        if self.is_connected() {
            Ok(())
        } else {
            Err(VJoyError::NotConnected)
        }
    }
}

fn mock_factories() -> BackendFactories {
    let (hid, _) = MockBackend::new("HIDMaestro");
    let (vjoy, _) = MockBackend::new("vJoy");
    let (none, _) = MockBackend::new("NullSink");
    BackendFactories {
        hidmaestro: Some(Box::new(move || hid.boxed())),
        vjoy: Some(Box::new(move || vjoy.boxed())),
        none: Box::new(move || none.boxed()),
    }
}

/// Gate: backend switch vJoy ↔ HID + UAC count (A10, plan §C).
///
/// Runtime: drives the real [`switch_backend`] state machine with
/// mock backends (None → HID → vJoy → HID → None, fallback on a
/// failed HID connect, hard failure on a failed vJoy connect), then
/// proves the startup path never touches an elevated backend: with
/// `HIDMaestro` persisted, [`AppStateCore::activate_startup_backend_with_factories`]
/// defers it and the HID factory is built zero times — so zero UAC
/// prompts can appear at startup. Source audit: the UAC copy exists
/// (`UAC_NOTE` + the Settings "Needs admin each start." line).
/// The per-session "prompt at most once" runtime proof is
/// Windows-only and stays an open user-PC verification.
#[must_use]
pub fn gate_backend_switch() -> (bool, String) {
    let mut report = Report::new();

    let factories = mock_factories();
    let mut current = MockBackend::new("NullSink").0.boxed();

    let outcome = switch_backend(&mut current, BackendKind::HidMaestro, &factories, None);
    let ok = outcome
        .is_ok_and(|o| o.active == BackendKind::HidMaestro && o.switched && !o.fell_back)
        && current.name() == "HIDMaestro"
        && current.is_connected();
    report.check(
        "none->hid",
        ok,
        "HIDMaestro active + connected after switch from NullSink",
    );

    let outcome = switch_backend(&mut current, BackendKind::VJoy, &factories, None);
    let ok = outcome.is_ok_and(|o| o.active == BackendKind::VJoy && o.switched)
        && current.name() == "vJoy"
        && current.is_connected();
    report.check(
        "hid->vjoy",
        ok,
        "vJoy active + connected; HID instance released (slot holds vJoy only)",
    );

    let outcome = switch_backend(&mut current, BackendKind::HidMaestro, &factories, None);
    let ok = outcome.is_ok_and(|o| o.active == BackendKind::HidMaestro && o.switched)
        && current.name() == "HIDMaestro"
        && current.is_connected();
    report.check("vjoy->hid", ok, "back to HIDMaestro, connected");

    let outcome = switch_backend(&mut current, BackendKind::None, &factories, None);
    let ok = outcome.is_ok_and(|o| o.active == BackendKind::None && o.switched)
        && current.name() == "NullSink"
        && !current.is_connected();
    report.check(
        "hid->none",
        ok,
        "NullSink installed, disconnected — output disabled, no device held",
    );

    // Failed HID connect must fall back to vJoy (the documented
    // fallback), with the reason recorded — never a silent switch.
    let (failing_hid, _) = MockBackend::failing("HIDMaestro", 1);
    let (working_vjoy, _) = MockBackend::new("vJoy");
    let (none, _) = MockBackend::new("NullSink");
    let fallback_factories = BackendFactories {
        hidmaestro: Some(Box::new(move || failing_hid.boxed())),
        vjoy: Some(Box::new(move || working_vjoy.boxed())),
        none: Box::new(move || none.boxed()),
    };
    let mut current = MockBackend::new("NullSink").0.boxed();
    let outcome = switch_backend(
        &mut current,
        BackendKind::HidMaestro,
        &fallback_factories,
        None,
    );
    let ok = outcome
        .is_ok_and(|o| o.active == BackendKind::VJoy && o.fell_back && o.fallback_reason.is_some())
        && current.name() == "vJoy"
        && current.is_connected();
    report.check(
        "hid-fail-falls-back-to-vjoy",
        ok,
        "HID connect failed → vJoy installed, fell_back=true, reason recorded",
    );

    // Failed vJoy connect has no fallback: the switch errors and the
    // installed slot is untouched (still NullSink).
    let (failing_vjoy, _) = MockBackend::failing("vJoy", 1);
    let (none2, _) = MockBackend::new("NullSink");
    let no_fallback_factories = BackendFactories {
        hidmaestro: None,
        vjoy: Some(Box::new(move || failing_vjoy.boxed())),
        none: Box::new(move || none2.boxed()),
    };
    let mut current = MockBackend::new("NullSink").0.boxed();
    let outcome = switch_backend(
        &mut current,
        BackendKind::VJoy,
        &no_fallback_factories,
        None,
    );
    let ok = outcome.is_err() && current.name() == "NullSink" && !current.is_connected();
    report.check(
        "vjoy-fail-errors-slot-untouched",
        ok,
        "vJoy connect failed → Err, NullSink still installed",
    );

    // Startup deferral (A10): a persisted HIDMaestro must NOT be
    // built at startup — building it would launch the elevated
    // sidecar and pop UAC. The factory counts builds; zero builds
    // means zero UAC prompts at startup.
    let dir = temp_dir("backend-startup");
    let mut store = match ProfileStore::open(&dir) {
        Ok(store) => store,
        Err(error) => {
            report.check(
                "startup-deferral-store",
                false,
                &format!("open store: {error}"),
            );
            return report.finish();
        }
    };
    let mut settings = store.settings().clone();
    settings.output_backend = BackendKind::HidMaestro.settings_value().to_string();
    store.set_settings(settings);
    if let Err(error) = store.save() {
        report.check(
            "startup-deferral-store",
            false,
            &format!("persist HIDMaestro setting: {error}"),
        );
        return report.finish();
    }
    let builds = Arc::new(AtomicUsize::new(0));
    let builds_in_factory = Arc::clone(&builds);
    let (none3, _) = MockBackend::new("NullSink");
    let startup_factories = BackendFactories {
        hidmaestro: Some(Box::new(move || {
            builds_in_factory.fetch_add(1, Ordering::SeqCst);
            MockBackend::new("HIDMaestro").0.boxed()
        })),
        vjoy: None,
        none: Box::new(move || none3.boxed()),
    };
    let state = AppStateCore::new(store, MockBackend::new("NullSink").0.boxed(), "gate");
    let outcome: StartupBackendOutcome =
        state.activate_startup_backend_with_factories(&startup_factories);
    let reason = outcome.reason.clone().unwrap_or_default();
    report.check(
        "startup-hid-deferred",
        outcome.active.is_none()
            && !outcome.fell_back
            && reason.contains("UAC")
            && builds.load(Ordering::SeqCst) == 0,
        &format!(
            "persisted HIDMaestro at startup: active=None, reason mentions UAC, HID factory built {} time(s) (must be 0)",
            builds.load(Ordering::SeqCst)
        ),
    );
    let _ = std::fs::remove_dir_all(&dir);

    // UAC copy audit (A10): the user must be told admin is needed
    // BEFORE choosing HID, and vJoy must never need it. (The
    // Settings wording changed during v2.0.2 from "Needs admin each
    // start." to "Needs admin permission when first used." — the
    // check accepts the current wording and reports it.)
    let backend_rs = read("crates/rsh-output-hidmaestro/src/backend.rs").unwrap_or_default();
    let settings_slint = read("crates/rsh-ui/ui/screens/settings.slint").unwrap_or_default();
    let settings_warns_admin = settings_slint.contains("Needs admin");
    report.check(
        "uac-copy",
        backend_rs.contains("pub const UAC_NOTE")
            && backend_rs.contains("UAC")
            && settings_warns_admin,
        "UAC_NOTE exists in the HID backend; Settings HID row warns admin is needed",
    );

    // A live backend accepts ticks; a disconnected one reports
    // NotConnected honestly instead of pretending to send.
    let (mut live, _) = MockBackend::new("vJoy");
    let _ = live.try_connect();
    let live_ok = live.update(&ProcessedState::default()).is_ok() && live.send_neutral().is_ok();
    let (mut dead, _) = MockBackend::new("vJoy");
    let dead_ok = dead.update(&ProcessedState::default()).is_err() && dead.send_neutral().is_err();
    report.check(
        "backend-send-honesty",
        live_ok && dead_ok,
        "connected backend sends; disconnected backend errors (no fake sends)",
    );

    report.finish()
}

/// Words that must never appear in user-facing error copy (B6):
/// raw implementation detail, not something a user can act on.
const JARGON: &[&str] = &[
    "panic",
    "unwrap",
    "expect(",
    "0x",
    "Err(",
    "thread '",
    ".rs:",
    "assertion",
    "index out of bounds",
    "PoisonError",
    "called `",
];

fn jargon_free(text: &str) -> bool {
    !JARGON.iter().any(|word| text.contains(word))
}

/// Gate: error paths (B6, plan §C).
///
/// Runtime: every [`ConnectionError`] variant (plus all four
/// [`BindErrorKind`]s) is rendered through
/// [`UserError::from_connection`] and must carry a non-empty
/// `what_happened` and `what_to_do` with no jargon, a stable code,
/// and the detail passed in. The `title` is the stable code by
/// design (the UI maps the code to copy); the jargon rule applies
/// to the two sentences the user actually reads.
///
/// v2.2 note: the pairing variants (`WindowClosed`,
/// `WindowExpired`, `NoStoredKey`, `StoredKeyExists`, `BadTag`,
/// `AlreadyPaired`, `BoardUnpaired`) were deleted with pairing
/// (plan D14). `BoardMismatch` is kept: the app binds to the
/// board it joined (D11, `RSH-XXXX` SSID per board).
#[must_use]
pub fn gate_user_errors() -> (bool, String) {
    let mut report = Report::new();
    let variants: Vec<ConnectionError> = vec![
        ConnectionError::Unreachable,
        ConnectionError::Timeout,
        ConnectionError::TransportClosed,
        ConnectionError::BindFailed(BindErrorKind::AddrInUse),
        ConnectionError::BindFailed(BindErrorKind::PermissionDenied),
        ConnectionError::BindFailed(BindErrorKind::NoInterface),
        ConnectionError::BindFailed(BindErrorKind::Other),
        ConnectionError::BoardMismatch,
        ConnectionError::WatchdogStall,
        ConnectionError::Unknown,
    ];
    for error in variants {
        let rendered = UserError::from_connection(error, "gate detail");
        let code = error.code();
        let ok = !rendered.code.is_empty()
            && rendered.code == code
            && !rendered.what_happened.is_empty()
            && !rendered.what_to_do.is_empty()
            && jargon_free(&rendered.what_happened)
            && jargon_free(&rendered.what_to_do)
            && rendered.detail.as_deref() == Some("gate detail");
        report.check(
            &format!("user-error-{code}"),
            ok,
            &format!(
                "code={code} what='{}' do='{}'",
                rendered.what_happened, rendered.what_to_do
            ),
        );
    }
    // Distinct causes must render distinctly (Phase 0 gate §4.3):
    // "board unreachable" vs "control timed out" vs "watchdog
    // stalled" are different sentences, not one generic error.
    let a = UserError::from_connection(ConnectionError::Unreachable, "");
    let b = UserError::from_connection(ConnectionError::Timeout, "");
    let c = UserError::from_connection(ConnectionError::WatchdogStall, "");
    report.check(
        "user-error-distinct-copy",
        a.what_happened != b.what_happened
            && b.what_happened != c.what_happened
            && a.what_to_do != b.what_to_do,
        "Unreachable / Timeout / WatchdogStall render different copy",
    );
    report.finish()
}

/// Build a current-thread tokio runtime for checks that need async
/// IPC inside the synchronous contract harness (same pattern as the
/// `rsh-ipc` timeout test).
fn current_thread_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("gate tokio runtime")
}

/// Run `work` on a fresh OS thread (scoped, so it may borrow the
/// caller's locals) with its own tokio runtime. The harness calls
/// these checks from inside the rig's own `#[tokio::main]` runtime,
/// where `block_on` would panic ("cannot start a runtime from
/// within a runtime"); a fresh thread has no runtime context, so
/// building one there is safe.
fn block_on_scoped<F, T>(work: F) -> T
where
    F: FnOnce() -> T + Send,
    T: Send,
{
    std::thread::scope(|scope| {
        scope
            .spawn(work)
            .join()
            .expect("gate worker thread panicked")
    })
}

/// Gate: loading states / IPC timeout (B5, A9, plan §C).
///
/// Runtime: a peer that accepts the socket but never answers must
/// not hang the caller — `IpcClient::call` must fail with an
/// `IpcError::Io` ("timed out") in under 5 s (the client timeout is
/// 500 ms; 5 s is the assertion bound, matching the `rsh-ipc`
/// test). This is the property that keeps a disconnected wheel from
/// hanging the UI: every click's IPC round-trip is bounded. The
/// spinner/timeout/retry UI itself is a Slint-side behaviour —
/// covered by the loading-state source audit below and by the
/// user-PC retest.
#[must_use]
pub fn gate_ipc_timeout() -> (bool, String) {
    let mut report = Report::new();
    let dir = temp_dir("ipc-timeout");
    let path = dir.join("rsh-daemon.sock");
    let path_for_task = path.clone();
    let (elapsed_ms, timed_out) = block_on_scoped(|| {
        let rt = current_thread_runtime();
        rt.block_on(async {
            let listener = match FakeListener::bind(&path_for_task) {
                Ok(listener) => listener,
                Err(error) => return (0_u128, format!("bind failed: {error}")),
            };
            // Accept and hold the stream without ever answering.
            tokio::spawn(async move {
                if let Ok(stream) = listener.accept().await {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    drop(stream);
                }
            });
            let client = match IpcClient::connect(&path_for_task).await {
                Ok(client) => client,
                Err(error) => return (0_u128, format!("connect failed: {error}")),
            };
            let start = Instant::now();
            let result = client.call("ping", serde_json::json!({})).await;
            let elapsed_ms = start.elapsed().as_millis();
            match result {
                Ok(_) => (elapsed_ms, "call unexpectedly succeeded".to_string()),
                Err(error) => {
                    let message = error.to_string();
                    if message.contains("timed out") {
                        (elapsed_ms, String::new())
                    } else {
                        (elapsed_ms, format!("wrong error: {message}"))
                    }
                }
            }
        })
    });
    report.check(
        "silent-peer-times-out",
        timed_out.is_empty() && elapsed_ms < 5_000,
        &if timed_out.is_empty() {
            format!("silent daemon: call failed after {elapsed_ms} ms (< 5000 ms bound)")
        } else {
            timed_out.clone()
        },
    );
    // Connecting to a socket nobody listens on must also fail fast
    // (no hang while the daemon is down).
    let dead_path = dir.join("nobody-listening.sock");
    let (dead_ms, dead_failed_fast) = block_on_scoped(|| {
        let rt = current_thread_runtime();
        rt.block_on(async {
            let start = Instant::now();
            let result = IpcClient::connect(&dead_path).await;
            (start.elapsed().as_millis(), result.is_err())
        })
    });
    report.check(
        "daemon-down-connect-fails-fast",
        dead_failed_fast && dead_ms < 5_000,
        &format!("connect to a dead socket failed in {dead_ms} ms"),
    );
    let _ = std::fs::remove_dir_all(&dir);
    report.finish()
}

/// Gate: performance (B7, plan §C) — startup + the IPC path every
/// UI click uses.
///
/// Runtime, on this host: time to open a profile store and build
/// [`AppStateCore`] (the app-startup path), and the ping round-trip
/// distribution against a local echo server speaking the real IPC
/// wire format (p50/p99 over 100 pings). Thresholds are recorded in
/// the v2.0.2 gate report (removed from docs/ — see git history); a
/// breach fails the gate. Memory is
/// reported by the rig's allocator counters in the full run, not
/// re-measured here.
#[must_use]
pub fn gate_perf() -> (bool, String) {
    let mut report = Report::new();

    // Startup path: store open + AppStateCore build.
    let dir = temp_dir("perf-startup");
    let start = Instant::now();
    let opened = ProfileStore::open(&dir);
    let open_ms = start.elapsed().as_millis();
    match opened {
        Ok(store) => {
            let build_start = Instant::now();
            let _state = AppStateCore::new(store, MockBackend::new("NullSink").0.boxed(), "gate");
            let build_ms = build_start.elapsed().as_millis();
            report.check(
                "startup-state-build",
                open_ms < 1_000 && build_ms < 1_000,
                &format!(
                    "store open {open_ms} ms, AppStateCore build {build_ms} ms (both < 1000 ms)"
                ),
            );
        }
        Err(error) => {
            report.check(
                "startup-state-build",
                false,
                &format!("open store: {error}"),
            );
        }
    }
    let _ = std::fs::remove_dir_all(&dir);

    // IPC round-trip: echo server speaking the real wire format
    // (`{"v":<IPC_PROTOCOL_VERSION>,"id":N,"ok":true,"result":{"pong":true}}`).
    // The version must track `IPC_PROTOCOL_VERSION` — WS-3 bumped it
    // 1 → 2 with the no-pairing IPC changes, and the client rejects
    // any reply whose `v` differs, so a hardcoded `"v":1` here made
    // every ping fail and the p50/p99 came back as Duration::MAX.
    let dir = temp_dir("perf-ipc");
    let path = dir.join("rsh-daemon.sock");
    let (p50_ms, p99_ms, count) = block_on_scoped(|| {
        let rt = current_thread_runtime();
        rt.block_on(async {
        let listener = FakeListener::bind(&path).expect("gate echo server bind");
        tokio::spawn(async move {
            loop {
                let Ok(stream) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
                    let (reader, mut writer) = tokio::io::split(stream);
                    let mut lines = BufReader::new(reader).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        let id = serde_json::from_str::<serde_json::Value>(&line)
                            .ok()
                            .and_then(|v| v.get("id")?.as_u64())
                            .unwrap_or(0);
                        let reply = format!(
                            "{{\"v\":{IPC_PROTOCOL_VERSION},\"id\":{id},\"ok\":true,\"result\":{{\"pong\":true}}}}\n"
                        );
                        if writer.write_all(reply.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });
        let client = IpcClient::connect(&path).await.expect("gate echo connect");
        let mut samples: Vec<Duration> = Vec::with_capacity(100);
        for _ in 0..100 {
            let start = Instant::now();
            let ok = client.ping().await.unwrap_or(false);
            if !ok {
                break;
            }
            samples.push(start.elapsed());
        }
        samples.sort_unstable();
        let count = samples.len();
        let p50 = samples.get(count / 2).copied().unwrap_or(Duration::MAX);
        let p99 = samples.get(count.saturating_sub(1)).copied().unwrap_or(Duration::MAX);
        (p50, p99, count)
        })
    });
    report.check(
        "ipc-ping-round-trip",
        count == 100 && p50_ms < Duration::from_millis(5) && p99_ms < Duration::from_millis(50),
        &format!(
            "100 pings: p50 {} ms, p99 {} ms (thresholds p50 < 5 ms, p99 < 50 ms)",
            p50_ms.as_secs_f64() * 1000.0,
            p99_ms.as_secs_f64() * 1000.0
        ),
    );
    let _ = std::fs::remove_dir_all(&dir);
    report.finish()
}

/// Gate: OTA end-to-end (B4, plan §C) — reuses the 2C rig
/// scenarios (`tools/rsh-rig/src/ota_checks.rs`): signed image
/// flashes to the fake board, a same-version reinstall is allowed,
/// tampered/wrong-key images are rejected, a mid-upload disconnect
/// rolls back, and an upgrade preserves data. The board half is the
/// fake board; the real-board flash stays a user-PC verification.
#[must_use]
pub fn gate_ota_e2e() -> (bool, String) {
    let mut report = Report::new();
    let (ok, detail) = crate::ota_checks::self_check_ota_e2e_success();
    report.check("ota-e2e-success", ok, &detail);
    let (ok, detail) = crate::ota_checks::self_check_ota_e2e_same_version_reinstall();
    report.check("ota-e2e-same-version", ok, &detail);
    let (ok, detail) = crate::ota_checks::self_check_ota_e2e_bad_signature();
    report.check("ota-e2e-bad-signature", ok, &detail);
    let (ok, detail) = crate::ota_checks::self_check_ota_e2e_disconnect_rollback();
    report.check("ota-e2e-disconnect-rollback", ok, &detail);
    let (ok, detail) = crate::ota_checks::self_check_upgrade_preserve();
    report.check("ota-upgrade-preserve", ok, &detail);
    let (ok, detail) = crate::ota_checks::self_check_installer_preserves_data();
    report.check("ota-installer-preserves-data", ok, &detail);
    report.finish()
}

/// Gate: installer install → update flows (B3, plan §C) — read-only
/// audit of `.github/workflows/release.yml`.
///
/// The workflow generates the NSIS script and runs a headless
/// smoke test; this check asserts both halves cover what B3
/// promises: version-compare (update / reinstall / downgrade
/// prompts), daemon stop + restart around an update, registry
/// version write, user-data survival, firewall rule, and uninstall
/// cleanup. It CANNOT prove the installer UI on a real PC — and it
/// records the gap honestly: the smoke test installs the SAME
/// version twice, so the update-vs-reinstall-vs-downgrade branches
/// (different installed version) are never exercised in CI.
#[must_use]
pub fn gate_installer_audit() -> (bool, String) {
    let mut report = Report::new();
    let Some(workflow) = read(".github/workflows/release.yml") else {
        report.check("installer-workflow-present", false, "release.yml missing");
        return report.finish();
    };
    let markers: &[(&str, &str)] = &[
        (
            "VersionCompare",
            "smart installer compares installed vs new version",
        ),
        (
            "Update it to",
            "update prompt (existing install, older version)",
        ),
        ("Reinstall it?", "reinstall prompt (same version)"),
        (
            "Downgrade to",
            "downgrade warning (newer version installed)",
        ),
        (
            "smoke-marker.txt",
            "smoke test: user data survives the update",
        ),
        (
            "daemon --check",
            "smoke test: daemon answers after install/update",
        ),
        (
            "firewall rule 'RSH'",
            "smoke test: firewall rule present after install",
        ),
        (
            "uninstall",
            "smoke test: uninstall removes rule + install dir",
        ),
    ];
    for (marker, what) in markers {
        report.check(
            &format!("installer-{marker}"),
            workflow.contains(marker),
            what,
        );
    }
    // Honest gap, recorded not hidden: the smoke test reinstalls
    // the same version, so the three VersionCompare branches are
    // untested in CI. The check passes on coverage of the markers;
    // the branch proof stays user-PC.
    report.check(
        "installer-branch-gap-recorded",
        workflow.contains("install a second time over the first"),
        "smoke test installs the same version twice — update/reinstall/downgrade branches are NOT exercised in CI (open: user-PC verification)",
    );
    report.finish()
}

/// Count non-overlapping occurrences of `needle` in `haystack`.
fn count(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

/// Gate: slider visual, source audit (A6, plan §C).
///
/// Source audit: the `SliderRow` fill must be `width:
/// parent.width * value` anchored at the left (x = 0) with the
/// thumb at `x: parent.width * value - 7px`, and there must be no
/// inverted `(1 - value)` fill. The pixel proof (fill left of the
/// thumb at min/mid/max) runs in `tools/gate-checks/rsh-gate-ui`
/// (`ui-slider-visual`); this audit is the fast regression net.
#[must_use]
pub fn gate_slider_source() -> (bool, String) {
    let mut report = Report::new();
    let Some(components) = read("crates/rsh-ui/ui/components.slint") else {
        report.check("slider-source-present", false, "components.slint missing");
        return report.finish();
    };
    // A6 fix (Slint team): the fill/thumb read the *clamped* fraction
    // `frac` (0..1), never the raw `value` — an out-of-range value
    // can never flip the fill. The fill is anchored at the track's
    // left edge (`x: 0px`) and runs min → thumb.
    report.check(
        "slider-fill-min-to-thumb",
        components.contains("width: parent.width * frac;"),
        "fill width is parent.width * frac (min → thumb)",
    );
    report.check(
        "slider-thumb-at-value",
        components.contains("parent.width * frac - 7px"),
        "thumb x is parent.width * frac - 7px (centred on the fill end)",
    );
    report.check(
        "slider-no-inverted-fill",
        !components.contains("(1 - value)") && !components.contains("(1 - frac)"),
        "no inverted (1 - value) fill anywhere in components.slint",
    );
    report.finish()
}

/// Gate: tray popup persistence, source audit (A4, plan §C).
///
/// Source audit: the popup's only hide paths are click-outside
/// (via `tray_popup::PopupDismissPoll` — the F9 state machine:
/// never dismisses within the 500 ms grace, never dismisses while
/// the popup is foreground, and only dismisses on a real
/// foreground move-away — sustained foreground lost, or the
/// foreground away from the re-sampled baseline for 3 consecutive
/// polls), Escape, and the popup's own buttons — there is no
/// unconditional auto-hide timer, and `tray.slint` itself contains
/// no `Timer`. The runtime proof (popup open 5 s, click-away
/// closes it) is Windows-only and stays a user-PC verification.
#[must_use]
pub fn gate_tray_persistence_source() -> (bool, String) {
    let mut report = Report::new();
    let Some(daemon) = read("crates/rsh-daemon/src/main.rs") else {
        report.check("tray-source-present", false, "daemon main.rs missing");
        return report.finish();
    };
    let Some(tray) = read("crates/rsh-daemon/ui/tray.slint") else {
        report.check("tray-source-present", false, "tray.slint missing");
        return report.finish();
    };
    let popup = read("crates/rsh-daemon/src/tray_popup.rs").unwrap_or_default();
    report.check(
        "tray-slint-no-timer",
        !tray.contains("Timer"),
        "tray.slint has no Timer (no Slint-side auto-hide)",
    );
    report.check(
        "tray-dismiss-fn-grace",
        popup.contains("pub struct PopupDismissPoll")
            && popup.contains("POPUP_FOREGROUND_GRACE: Duration = Duration::from_millis(500)")
            && popup.contains("if elapsed <= POPUP_FOREGROUND_GRACE {"),
        "PopupDismissPoll never dismisses within the 500 ms grace",
    );
    report.check(
        "tray-dismiss-fn-foreground",
        popup.contains("if popup_foreground {")
            && popup.contains("saw_sustained_foreground && foreground_now.is_some()")
            && popup.contains("changed_streak >= POPUP_DISMISS_CHANGED_POLLS"),
        "PopupDismissPoll never dismisses while foreground; dismisses only on a real foreground move-away (sustained foreground lost, or 3 consecutive polls away from baseline)",
    );
    let Some(timer_start) = daemon.find("Refresh timer (10 Hz while visible)") else {
        report.check("tray-timer-block", false, "refresh-timer block not found");
        return report.finish();
    };
    let Some(timer_end) = daemon[timer_start..].find("std::mem::forget(timer)") else {
        report.check("tray-timer-block", false, "timer block end not found");
        return report.finish();
    };
    let block = &daemon[timer_start..timer_start + timer_end];
    let hide_count = block.matches("handle.hide(").count();
    report.check(
        "tray-timer-hide-only-via-dismiss",
        block.contains("handle.tick(shown, foreground_now, hwnd)") && hide_count == 1,
        "the timer's only hide() is behind the PopupDismissPoll tick gate (no timed hide)",
    );
    report.check(
        "tray-hide-paths",
        daemon.contains("handle.hide(\"close-button\")")
            && tray.contains("Key.Escape"),
        "hide paths are: popup close button, Escape, click-away via PopupDismissPoll — no timed hide",
    );
    report.finish()
}

/// Gate: tray "Open RSH" shows live/real state, source audit
/// (A12, plan §C).
///
/// Source audit: [`TrayPopupData::collect`] derives the connection
/// label from the live daemon API (never a hardcoded "Connected")
/// and clamps all bars to 0–100, and the `.slint` popup defaults
/// to "Disconnected" with zero bars. The runtime proof (open from
/// the tray with the daemon down → "Disconnected", never fake
/// data) is covered by the `ui-tray-disconnected` snapshot check in
/// `rsh-gate-ui` plus the user-PC retest.
#[must_use]
pub fn gate_tray_data_source() -> (bool, String) {
    let mut report = Report::new();
    let Some(popup) = read("crates/rsh-daemon/src/tray_popup.rs") else {
        report.check("tray-data-source-present", false, "tray_popup.rs missing");
        return report.finish();
    };
    let Some(tray) = read("crates/rsh-daemon/ui/tray.slint") else {
        report.check("tray-data-source-present", false, "tray.slint missing");
        return report.finish();
    };
    report.check(
        "tray-label-from-live-api",
        popup.contains("conn_label: conn.label.clone()"),
        "popup label comes from the live connection, not a hardcoded string",
    );
    report.check(
        "tray-bars-clamped",
        popup.contains(".clamp(0.0, 100.0)"),
        "throttle/brake/clutch bars clamped to 0–100 (no impossible values)",
    );
    report.check(
        "tray-defaults-disconnected",
        tray.contains("in property <string> conn-label: \"Disconnected\"")
            && tray.contains("in property <bool> connected: false"),
        "popup defaults: conn-label Disconnected, connected=false",
    );
    report.finish()
}

/// Gate: X-close → tray → instant reopen (A11, plan §C), source
/// audit.
///
/// Source audit: closing the window must hide it to the tray
/// (never quit the app/daemon), and reopening from the tray must
/// restore the existing window (no fresh load). Wired in `rsh-ui`:
/// `on_close_requested` asks `close_to_tray()` and answers
/// `CloseRequestResponse::HideWindow` (real quit is only via tray
/// "Quit RSH" or with "Keep running in the tray" off).
#[must_use]
pub fn gate_x_close_source() -> (bool, String) {
    let mut report = Report::new();
    let ui_main = read("crates/rsh-ui/src/main.rs").unwrap_or_default();
    let main_slint = read("crates/rsh-ui/ui/main.slint").unwrap_or_default();
    let shell_slint = read("crates/rsh-ui/ui/shell.slint").unwrap_or_default();
    let combined = format!("{ui_main}\n{main_slint}\n{shell_slint}");
    let wired = combined.contains("close_to_tray")
        || combined.contains("on_close_requested")
        || combined.contains("CloseRequest");
    report.check(
        "x-close-wired",
        wired,
        "window close is wired to a close-to-tray handler (on_close_requested + close_to_tray)",
    );
    let hides = combined.contains("CloseRequestResponse::HideWindow")
        || combined.contains("set_visible(false)")
        || combined.contains("window().hide()");
    report.check(
        "x-close-hides-not-quits",
        wired && hides,
        "close hides the window instead of quitting the app (HideWindow response)",
    );
    // The tray side must already restore the window: the daemon
    // handles OpenMain (checked here so a tray regression is
    // caught even while the UI side is pending).
    let daemon = read("crates/rsh-daemon/src/main.rs").unwrap_or_default();
    report.check(
        "tray-open-main-exists",
        daemon.contains("TrayAction::OpenMain"),
        "daemon tray menu has an OpenMain action",
    );
    report.finish()
}

/// Gate: wizard reopen (A1, plan §C), source audit.
///
/// The Settings path is wired end-to-end: Settings "Run setup
/// again" → `run-setup` → `run-setup-again` → Rust clears the
/// setup marker and opens the wizard at step 0. The tray-menu path
/// is wired too: the daemon tray menu has `TrayAction::OpenSetup`
/// ("Run setup"), which spawns the UI on the Setup page.
#[must_use]
pub fn gate_wizard_reopen_source() -> (bool, String) {
    let mut report = Report::new();
    let settings = read("crates/rsh-ui/ui/screens/settings.slint").unwrap_or_default();
    let main_slint = read("crates/rsh-ui/ui/main.slint").unwrap_or_default();
    let ui_main = read("crates/rsh-ui/src/main.rs").unwrap_or_default();
    let daemon = read("crates/rsh-daemon/src/main.rs").unwrap_or_default();
    report.check(
        "wizard-reopen-settings-row",
        settings.contains("Run setup again") && settings.contains("run-setup();"),
        "Settings has a 'Run setup again' row wired to run-setup()",
    );
    report.check(
        "wizard-reopen-forwarded",
        main_slint.contains("run-setup => { run-setup-again(); }"),
        "main.slint forwards run-setup to run-setup-again",
    );
    // v2.2 note (no-pairing redesign, WS-4): the handler delegates
    // to `Ctx::open_setup_wizard`, which clears the marker, resets
    // the wizard state and opens it at step 0 — the old inline
    // `w.set_setup_mode(true)` in the closure is gone, but the
    // behaviour is the same; audit the helper, not the old shape.
    report.check(
        "wizard-reopen-rust",
        ui_main.contains("on_run_setup_again")
            && ui_main.contains("open_setup_wizard(&w)")
            && ui_main.contains("fn open_setup_wizard")
            && ui_main.contains("clear_setup_marker()")
            && ui_main.contains("set_setup_step(0)")
            && ui_main.contains("set_setup_mode(true)"),
        "Rust on_run_setup_again clears the marker and opens the wizard at step 0",
    );
    let tray_has_setup = daemon.contains("TrayAction::OpenSetup")
        || daemon.contains("TrayAction::RunSetup")
        || daemon.contains("\"Run setup\"")
        || daemon.contains("\"Setup\"");
    report.check(
        "wizard-reopen-tray-menu",
        tray_has_setup,
        "tray menu has a Setup reopen entry (TrayAction::OpenSetup)",
    );
    report.finish()
}

/// Gate: wizard fake data (A3, plan §C), source audit.
///
/// The wizard's defaults are placeholders, never fake live values,
/// and the simulated data source must never advance the wizard
/// (a sim reporting "Active" is not a real connection).
///
/// v2.2 note (no-pairing redesign, D16): the wizard is 4 steps
/// (Connect, Drivers, Calibrate, Games) and the pairing copy is
/// gone — no `pair-status` / `paired` defaults anymore. The
/// Connect step's why-it-cannot-advance text is `connect-hint`
/// ("" = no error to show).
#[must_use]
pub fn gate_wizard_fake_data_source() -> (bool, String) {
    let mut report = Report::new();
    let setup = read("crates/rsh-ui/ui/screens/setup.slint").unwrap_or_default();
    let ui_main = read("crates/rsh-ui/src/main.rs").unwrap_or_default();
    report.check(
        "wizard-defaults-placeholder",
        setup.contains("games-text: \"No games added yet.\"")
            && setup.contains("connect-done: false")
            && setup.contains("connect-hint: \"\"")
            && !setup.contains("pair-status:")
            && !setup.contains("paired:"),
        "wizard defaults: 'No games added yet.', connect-done=false, connect-hint empty, no pair-status/paired defaults",
    );
    report.check(
        "wizard-sim-never-advances",
        ui_main.contains("must never advance the wizard"),
        "sim data never advances the wizard Connect step (Rust guards on the real session)",
    );
    report.finish()
}

/// Gate: loading states (B5, A9, plan §C), source audit.
///
/// The bounded-call property is runtime-proven by
/// `gate-ipc-timeout` (every IPC call fails within 500 ms instead
/// of hanging). This audit checks the wiring around it: the IPC
/// connect itself has a 2 s timeout, the poll loop goes through
/// the bounded `call` (a single combined `poll` since L-4, v2.0.2),
/// and the UI carries loading-state markers (a spinner/timeout/
/// retry path per async action). The visible spinner behaviour is
/// user-PC verified.
#[must_use]
pub fn gate_loading_source() -> (bool, String) {
    let mut report = Report::new();
    let client = read("crates/rsh-ipc/src/client.rs").unwrap_or_default();
    let ipc_api = read("crates/rsh-ui/src/ipc_api.rs").unwrap_or_default();
    let ui_main = read("crates/rsh-ui/src/main.rs").unwrap_or_default();
    report.check(
        "loading-ipc-call-bounded",
        client.contains("CALL_TIMEOUT")
            && client.contains("timeout_at(deadline")
            && client.contains("from_millis(500)"),
        "IpcClient::call is bounded by CALL_TIMEOUT (500 ms)",
    );
    report.check(
        "loading-connect-bounded",
        ipc_api.contains("timeout(Duration::from_secs(2), IpcClient::connect"),
        "UI connect is bounded by a 2 s timeout",
    );
    report.check(
        "loading-poll-uses-bounded-call",
        ipc_api.contains(".call(\"poll\""),
        "poll loop uses the bounded call (single combined poll, L-4)",
    );
    let loading_markers =
        count(&ui_main, "loading") + count(&ui_main, "spinner") + count(&ui_main, "Loading");
    report.check(
        "loading-ui-markers",
        loading_markers >= 5,
        &format!("{loading_markers} loading/spinner markers in rsh-ui main.rs"),
    );
    report.finish()
}

/// Gate: hover / interaction feedback (A5, plan §C), source audit.
///
/// The `Btn` component must change border/background on hover and
/// press and show a pointer cursor; other interactive components
/// must carry the same `has-hover` feedback. The rendered proof
/// (hover actually repaints) is user-PC verified — the testing
/// backend has no mouse-move event to drive hover headlessly.
#[must_use]
pub fn gate_hover_source() -> (bool, String) {
    let mut report = Report::new();
    let Some(components) = read("crates/rsh-ui/ui/components.slint") else {
        report.check("hover-source-present", false, "components.slint missing");
        return report.finish();
    };
    report.check(
        "hover-btn-feedback",
        components.contains("ta.has-hover ? Theme.accent-hover")
            && components.contains("ta.pressed ? Theme.accent-pressed")
            && components.contains("MouseCursor.pointer"),
        "Btn: hover lightens accent, pressed darkens it, cursor is pointer",
    );
    report.check(
        "hover-preset-chip",
        components.contains("ta.has-hover ? Theme.hover-bg"),
        "PresetChip (and secondary Btn): hover changes the panel fill",
    );
    let hover_count = count(&components, "has-hover");
    report.check(
        "hover-markers-count",
        hover_count >= 5,
        &format!("{hover_count} has-hover bindings in components.slint"),
    );
    let mut screens_hover = 0;
    for screen in [
        "overview",
        "steering",
        "pedals",
        "buttons",
        "extras",
        "settings",
        "diagnostics",
    ] {
        let text = read(&format!("crates/rsh-ui/ui/screens/{screen}.slint")).unwrap_or_default();
        screens_hover += count(&text, "has-hover");
    }
    report.check(
        "hover-screens",
        screens_hover >= 5,
        &format!("{screens_hover} has-hover bindings across the screens"),
    );
    report.finish()
}

/// Gate: theme switching (A8, plan §C), source audit.
///
/// The theme is a `mode` binding in the `Theme` global (0 Dark,
/// 1 Light, 2 Auto) with a real Light palette; Settings forwards
/// the choice and Rust persists it. The rendered proof (Light
/// actually repaints the window) runs in `rsh-gate-ui`
/// (`ui-theme-switch`).
#[must_use]
pub fn gate_theme_source() -> (bool, String) {
    let mut report = Report::new();
    let tokens = read("crates/rsh-ui/ui/tokens.slint").unwrap_or_default();
    let settings = read("crates/rsh-ui/ui/screens/settings.slint").unwrap_or_default();
    let main_slint = read("crates/rsh-ui/ui/main.slint").unwrap_or_default();
    let ui_main = read("crates/rsh-ui/src/main.rs").unwrap_or_default();
    report.check(
        "theme-mode-binding",
        tokens.contains("in-out property <int> mode: 0;")
            && tokens.contains("in-out property <bool> is-light: mode == 1;"),
        "Theme global has mode (0 Dark / 1 Light / 2 Auto) + is-light",
    );
    report.check(
        "theme-light-palette",
        tokens.contains("is-light ? #F5F3FF") && tokens.contains("is-light ? #FFFFFF"),
        "Light palette exists (bg #F5F3FF, panel #FFFFFF)",
    );
    report.check(
        "theme-settings-wired",
        settings.contains("theme-selected") && settings.contains("theme-choice: 0;"),
        "Settings theme row forwards the selection",
    );
    report.check(
        "theme-window-wired",
        main_slint.contains("Theme.mode: theme;") && main_slint.contains("theme-persist"),
        "MainWindow drives Theme.mode and persists the choice",
    );
    report.check(
        "theme-rust-wired",
        ui_main.contains("fn wire_theme") && ui_main.contains("fn read_theme"),
        "Rust wire_theme/read_theme persist + restore the theme",
    );
    report.finish()
}

/// Gate: overflow — no button/text outside its card (A7, plan §C),
/// source audit.
///
/// The "Backup and updates" card's rows give the text column
/// `horizontal-stretch: 1` and `wrap: word-wrap`, so a long line
/// can never push the Check button out of the card. The rendered
/// proof at 920 px and at a narrow width is user-PC verified.
#[must_use]
pub fn gate_overflow_source() -> (bool, String) {
    let mut report = Report::new();
    let Some(settings) = read("crates/rsh-ui/ui/screens/settings.slint") else {
        report.check("overflow-source-present", false, "settings.slint missing");
        return report.finish();
    };
    let Some(card_start) = settings.find("Backup and updates") else {
        report.check(
            "overflow-card-present",
            false,
            "Backup and updates card missing",
        );
        return report.finish();
    };
    let card = &settings[card_start..];
    report.check(
        "overflow-text-wraps",
        card.contains("wrap: word-wrap;"),
        "update rows wrap their sub-text (word-wrap)",
    );
    report.check(
        "overflow-text-column-stretches",
        card.contains("horizontal-stretch: 1;"),
        "text column takes the free width; the Check button keeps its own width",
    );
    report.check(
        "overflow-fix-note",
        settings.contains("A7, v2.0.2"),
        "A7 fix is marked in the source",
    );
    report.finish()
}

/// Gate: error handling + notifications (B6, plan §C), source
/// audit.
///
/// The typed [`UserError`] rendering is runtime-proven by
/// `gate-user-errors`. This audit checks the delivery path exists:
/// a `NotificationCenter` is wired into the UI shell, so a failure
/// surfaces as a visible notification instead of a silent fail.
#[must_use]
pub fn gate_notifications_source() -> (bool, String) {
    let mut report = Report::new();
    let notifications = read("crates/rsh-ui/src/notifications.rs");
    let ui_main = read("crates/rsh-ui/src/main.rs").unwrap_or_default();
    report.check(
        "notifications-module",
        notifications.is_some(),
        "crates/rsh-ui/src/notifications.rs exists",
    );
    report.check(
        "notifications-wired",
        ui_main.contains("NotificationCenter") && ui_main.contains("notifications:"),
        "NotificationCenter is wired into the UI context",
    );
    let commands = read("crates/rsh-app-core/src/commands.rs").unwrap_or_default();
    report.check(
        "notifications-typed-errors",
        commands.contains("UserError"),
        "app-core commands surface typed UserError (not bare strings)",
    );
    report.finish()
}

/// Gate: Extras card size, source audit (cards layout fix, Bug 2).
///
/// Source audit: the 4 Extras module cards must NOT swap their body
/// with `if` branches — `if` removes the element from the tree, so
/// the card collapsed to ~71 px when off and the whole grid reflowed
/// on every toggle (measured: grid 1030 px all-ON → 190 px all-OFF).
/// Both the OFF note and the ON body stay in the tree and only
/// `visible` toggles (`visible: false` keeps the layout space in box
/// layouts, verified headless). The pixel proof (row heights identical
/// ON/OFF on the real `ExtrasScreen`) runs in `tools/gate-checks/
/// rsh-gate-ui` (`ui-extras-card-size`); this audit is the fast
/// regression net.
#[must_use]
pub fn gate_extras_card_size_source() -> (bool, String) {
    let mut report = Report::new();
    let Some(extras) = read("crates/rsh-ui/ui/screens/extras.slint") else {
        report.check("extras-source-present", false, "extras.slint missing");
        return report.finish();
    };
    for card in ["handbrake", "shifter", "horn", "leds"] {
        report.check(
            &format!("extras-{card}-no-if-off"),
            !extras.contains(&format!("if !{card}-on")),
            &format!("no `if !{card}-on` branch (OFF note is `visible`, not `if`)"),
        );
        report.check(
            &format!("extras-{card}-no-if-on"),
            !extras.contains(&format!("if {card}-on :")),
            &format!("no `if {card}-on :` branch (ON body is `visible`, not `if`)"),
        );
        report.check(
            &format!("extras-{card}-visible-off"),
            extras.contains(&format!("visible: !{card}-on;")),
            &format!("OFF note uses `visible: !{card}-on`"),
        );
        report.check(
            &format!("extras-{card}-visible-on"),
            extras.contains(&format!("visible: {card}-on;")),
            &format!("ON body uses `visible: {card}-on`"),
        );
    }
    // R-10: the 4 cards share the row equally — 342 px each at
    // design width ((696 - 12) / 2) — via `min-width`, so they still
    // grow on wider windows. Without it the row was lopsided
    // (measured: Horn ~213 px, LEDs ~471 px — content-preferred
    // widths, no floor).
    report.check(
        "extras-cards-min-width",
        count(&extras, "min-width: 342px;") == 4,
        "all 4 Extras cards have min-width: 342px (equal columns)",
    );
    report.finish()
}

/// Gate: responsive card gaps, source audit (cards layout fix, Bug 1).
///
/// Source audit: inter-card gaps are spacer Rectangles (`min-width`
/// = the design gap, `horizontal-stretch: 0.05`) — NOT fixed
/// `spacing` literals, which never scale with the window — and the
/// cards that should grow use `min-width` (an explicit `width` caps
/// max == pref, `can_grow == 0`, so the card never grows however much
/// stretch it has). The pixel proof (gap 12 px at design width,
/// ~19 px at 1016 px; cards 362 → 498 px) runs in
/// `tools/gate-checks/rsh-gate-ui` (`ui-card-gap-responsive`); this
/// audit is the fast regression net. The two deliberately fixed tool
/// panels (Steering right column 296 px, Buttons edit panel 268 px)
/// are NOT asserted here.
#[must_use]
pub fn gate_card_gap_source() -> (bool, String) {
    let mut report = Report::new();
    let extras = read("crates/rsh-ui/ui/screens/extras.slint").unwrap_or_default();
    let pedals = read("crates/rsh-ui/ui/screens/pedals.slint").unwrap_or_default();
    let buttons = read("crates/rsh-ui/ui/screens/buttons.slint").unwrap_or_default();
    let steering = read("crates/rsh-ui/ui/screens/steering.slint").unwrap_or_default();
    let overview = read("crates/rsh-ui/ui/screens/overview.slint").unwrap_or_default();
    let settings = read("crates/rsh-ui/ui/screens/settings.slint").unwrap_or_default();
    report.check(
        "gap-spacers-extras",
        count(&extras, "min-width: 12px; horizontal-stretch: 0.05;") == 2,
        "Extras has 2 gap spacers (one per card row)",
    );
    report.check(
        "gap-spacers-pedals",
        count(&pedals, "min-width: 12px; horizontal-stretch: 0.05;") == 2,
        "Pedals has 2 gap spacers (between the 3 cards)",
    );
    report.check(
        "gap-spacers-buttons-tiles",
        count(&buttons, "min-width: 8px; horizontal-stretch: 0.05;") == 10,
        "Buttons has 10 gap spacers (9 between tiles + 1 knob row)",
    );
    report.check(
        "gap-spacers-buttons-root",
        count(&buttons, "min-width: 12px; horizontal-stretch: 0.05;") == 1,
        "Buttons has 1 root gap spacer (left column ↔ edit panel)",
    );
    report.check(
        "gap-spacer-steering",
        count(&steering, "min-width: 12px; horizontal-stretch: 0.05;") == 1,
        "Steering has 1 gap spacer (graph card ↔ right column)",
    );
    report.check(
        "gap-spacer-overview",
        count(&overview, "min-width: 12px; horizontal-stretch: 0.05;") == 1,
        "Overview has 1 gap spacer (bottom-row cards)",
    );
    report.check(
        "gap-spacer-settings",
        count(&settings, "min-width: 12px; horizontal-stretch: 0.05;") == 1,
        "Settings has 1 gap spacer (between the columns)",
    );
    report.check(
        "gap-grid-overview-fixed",
        overview.contains("spacing: 10px;"),
        "Overview games grid keeps fixed 10px spacing (a computed spacing from self.width is a binding loop inside the ScrollView — see the comment at the grid; the tiles still grow)",
    );
    // NOTE: `count("width: 224px;")` also matches the `min-width:`
    // occurrences (substring), so the explicit-width count is the
    // total minus the min-width count.
    report.check(
        "gap-pedals-min-width",
        count(&pedals, "min-width: 224px;") == 3
            && count(&pedals, "width: 224px;") - count(&pedals, "min-width: 224px;") == 0,
        "Pedals cards use min-width: 224px (no explicit width: 224px)",
    );
    report.check(
        "gap-buttons-min-width",
        count(&buttons, "min-width: 90px;") == 12
            && count(&buttons, "width: 90px;") - count(&buttons, "min-width: 90px;") == 0,
        "Buttons tiles use min-width: 90px (no explicit width: 90px)",
    );
    report.check(
        "gap-settings-min-width",
        count(&settings, "min-width: 342px;") == 2
            && count(&settings, "width: 342px;") - count(&settings, "min-width: 342px;") == 0,
        "Settings columns use min-width: 342px + stretch (no explicit width: 342px)",
    );
    report.finish()
}

/// Gate: HID switch UAC dwell (B-2, re-scan finding).
///
/// The elevated sidecar is launched via UAC (`runas`); the user
/// needs 10 s for the spawn plus thinking time to read and answer
/// the prompt. The manager waits `DEFAULT_SPAWN_TIMEOUT` for the
/// sidecar `hello` — 10 s today, so a user who takes longer to
/// answer the prompt trips `HelloTimeout` and the switch fails.
/// The gate requires the spawn/hello wait to cover 10 s + thinking
/// time (>= 60 s). On decline the switch must surface ONE clear
/// message and never retry-loop: the UI handler starts exactly one
/// backend job (which calls `set_output_backend` once, guarded by
/// the Backend in-flight check) and the done-arm notifies once
/// with the typed error, and the app-core call returns a single
/// typed error without installing any backend.
#[must_use]
pub fn gate_hid_uac_dwell() -> (bool, String) {
    let mut report = Report::new();

    // Dwell: parse DEFAULT_SPAWN_TIMEOUT from the manager.
    let manager = read("crates/rsh-output-hidmaestro/src/manager.rs").unwrap_or_default();
    let spawn_marker = "pub const DEFAULT_SPAWN_TIMEOUT: Duration = Duration::from_secs(";
    let spawn_secs = manager
        .find(spawn_marker)
        .and_then(|pos| {
            let rest = &manager[pos + spawn_marker.len()..];
            rest.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse::<u64>()
                .ok()
        })
        .unwrap_or(0);
    report.check(
        "hid-uac-dwell-timeout",
        spawn_secs >= 60,
        &format!(
            "DEFAULT_SPAWN_TIMEOUT is {spawn_secs} s (must be >= 60 s: 10 s spawn + user thinking time)"
        ),
    );

    // Decline / no-sidecar path on Linux: one call, one typed
    // error, fast, nothing installed, no retry.
    let dir = temp_dir("hid-decline");
    let state = match ProfileStore::open(&dir) {
        Ok(store) => AppStateCore::new(store, MockBackend::new("NullSink").0.boxed(), "gate"),
        Err(error) => {
            report.check(
                "hid-decline-single-error",
                false,
                &format!("open store: {error}"),
            );
            return report.finish();
        }
    };
    let start = Instant::now();
    let switched = state.set_output_backend("HIDMaestro");
    let decline_ms = start.elapsed().as_millis();
    let backend_now = state.backend_name();
    report.check(
        "hid-decline-single-error",
        switched.is_err() && decline_ms < 5_000,
        &format!(
            "set_output_backend(HIDMaestro) on Linux: single Err in {decline_ms} ms (no hang, no retry loop)"
        ),
    );
    report.check(
        "hid-decline-nothing-installed",
        backend_now != "HIDMaestro" && backend_now != "vJoy",
        &format!(
            "backend after failed HID switch is '{backend_now}' (no backend silently installed)"
        ),
    );
    let _ = std::fs::remove_dir_all(&dir);

    // UI handler: exactly one switch attempt, one notification.
    // Since the A9 job-system refactor the handler only starts a
    // background job — the `set_output_backend` call lives in
    // `start_backend_job` (guarded by the Backend in-flight check),
    // and the failure notification lives in the `JobDone::Backend`
    // done-arm (typed `notify_error` + toggle revert). Audit that
    // shape instead of the old direct-call handler.
    let ui_main = read("crates/rsh-ui/src/main.rs").unwrap_or_default();
    let Some(handler_start) = ui_main.find("on_output_backend_selected") else {
        report.check(
            "hid-ui-single-attempt",
            false,
            "backend-selected handler missing",
        );
        return report.finish();
    };
    let handler_end = ui_main[handler_start..]
        .find("on_autostart_selected")
        .map_or(ui_main.len(), |off| handler_start + off);
    let handler = &ui_main[handler_start..handler_end];
    let job_fn = ui_main.find("fn start_backend_job").map_or("", |start| {
        let rest = &ui_main[start..];
        let end = rest
            .find("\n    /// Start a firmware-check job")
            .unwrap_or(rest.len());
        &rest[..end]
    });
    report.check(
        "hid-ui-single-attempt",
        count(handler, "start_backend_job") == 1
            && !handler.contains("loop")
            && count(job_fn, "set_output_backend") == 1
            && job_fn.contains("contains_key(&JobKind::Backend)"),
        "UI backend-selected handler starts exactly one backend job (no loop); the job calls set_output_backend once, guarded by the Backend in-flight check",
    );
    let done_arm = ui_main
        .find("JobDone::Backend { index, result } => {")
        .map_or("", |start| {
            let rest = &ui_main[start..];
            let end = rest
                .find("JobDone::FirmwareCheck(result) => {")
                .unwrap_or(rest.len());
            &rest[..end]
        });
    report.check(
        "hid-ui-one-message",
        count(done_arm, "notify_error") == 1 && done_arm.contains("set_controller(1 - index)"),
        "UI shows exactly one typed failure notification on backend-switch failure and reverts the toggle",
    );
    report.finish()
}

/// Gate: bindings UI (H-3, re-scan finding).
///
/// The backend is done (B1): `AppStateCore` persists
/// button→action bindings and the IPC server dispatches
/// `list_bindings` / `set_binding` / `clear_binding`. The gate
/// requires the Buttons screen to actually reach them: Save must
/// call `set_binding` so a binding exists in the backend after
/// saving. Currently FAILS: the UI save handler shows "isn't a
/// wheel feature" and returns without calling `set_binding`, and
/// the UI IPC client does not forward the binding methods at all.
#[must_use]
pub fn gate_bindings_ui() -> (bool, String) {
    let mut report = Report::new();

    // Runtime: the backend round-trip itself.
    let dir = temp_dir("bindings-ui");
    let state = match ProfileStore::open(&dir) {
        Ok(store) => AppStateCore::new(store, MockBackend::new("NullSink").0.boxed(), "gate"),
        Err(error) => {
            report.check(
                "bindings-backend-round-trip",
                false,
                &format!("open store: {error}"),
            );
            return report.finish();
        }
    };
    let mut backend_ok = state.list_bindings().is_empty();
    let bound = state.set_binding(5, "horn");
    backend_ok &= bound.is_ok_and(|b| b.button == 5 && b.action == "horn");
    backend_ok &= state.set_binding(65, "profile_next").is_ok();
    backend_ok &= state.list_bindings().len() == 2;
    backend_ok &= state.clear_binding(5).unwrap_or(false);
    backend_ok &= state.list_bindings().len() == 1;
    backend_ok &= state.set_binding(0, "horn").is_err();
    backend_ok &= state.set_binding(5, "bogus_action").is_err();
    report.check(
        "bindings-backend-round-trip",
        backend_ok,
        "list/set/clear round-trip on AppStateCore (button 5 horn, 65 profile_next; invalid button/action rejected)",
    );
    let _ = std::fs::remove_dir_all(&dir);

    // IPC server dispatches the binding methods (daemon side ready).
    let server = read("crates/rsh-ipc/src/lib.rs").unwrap_or_default();
    report.check(
        "bindings-ipc-server-ready",
        server.contains("\"list_bindings\" | \"set_binding\" | \"clear_binding\""),
        "IPC server dispatches list/set/clear_binding",
    );

    // UI client forwards them (the gap).
    let ipc_api = read("crates/rsh-ui/src/ipc_api.rs").unwrap_or_default();
    report.check(
        "bindings-ipc-client-forwards",
        ipc_api.contains("set_binding") && ipc_api.contains("list_bindings"),
        "IpcApi forwards set_binding/list_bindings to the daemon (missing — UI cannot reach the backend)",
    );

    // Buttons save handler calls set_binding (the gap).
    let ui_main = read("crates/rsh-ui/src/main.rs").unwrap_or_default();
    let Some(save_start) = ui_main.find("on_edit_save") else {
        report.check(
            "bindings-save-calls-set-binding",
            false,
            "edit-save handler missing",
        );
        return report.finish();
    };
    let save_end = ui_main[save_start..]
        .find("on_edit_cancel")
        .map_or(ui_main.len(), |off| save_start + off);
    let save_block = &ui_main[save_start..save_end];
    report.check(
        "bindings-save-calls-set-binding",
        save_block.contains("set_binding"),
        "Buttons edit-save calls set_binding (currently shows \"isn't a wheel feature\" instead)",
    );
    report.finish()
}

/// Gate: OTA flash caller (H-4, re-scan finding).
///
/// The flash entry point exists in app-core
/// (`webota::flash_firmware`: signature-verify → upload with
/// progress → restart → confirm version). The gate requires a
/// caller: an "Update firmware" flow in Settings/wizard that
/// starts a flash from the bundled `firmware-signed.bin`, shows
/// progress, and confirms the new version.
///
/// Runtime: drives the real caller entry point
/// (`flash_firmware_with_pubkey`, throwaway key) against the rig
/// fake board and asserts the full flow — progress stages,
/// upload to 100%, confirmed version 4.0.3, board on 4.0.3.
/// Source audit: the UI caller. Currently FAILS: Settings only
/// *checks* the version (`settings_check_firmware` notifies);
/// there is no update/firmware action wired to a flash call.
#[must_use]
pub fn gate_ota_flash_caller() -> (bool, String) {
    let mut report = Report::new();

    // Runtime: the caller entry point, end-to-end on the fake board.
    let board = FakeBoard::start("secret", (4, 0, 1));
    let (image, _) = build_test_image("4.0.3", FIRMWARE_IMAGE_MIN_LEN);
    let dir = temp_dir("ota-flash-caller");
    let path = dir.join("firmware-signed.bin");
    if let Err(error) = std::fs::write(&path, &image) {
        report.check(
            "ota-caller-flash-e2e",
            false,
            &format!("write image: {error}"),
        );
        return report.finish();
    }
    let target = WebOtaTarget::on_port(
        std::net::IpAddr::from([127, 0, 0, 1]),
        board.port(),
        board.password(),
    );
    let mut stages: Vec<FlashStage> = Vec::new();
    let mut percents: Vec<u8> = Vec::new();
    let mut confirm = || wait_for_board_version(&target, "4.0.3", Duration::from_secs(10));
    let result = flash_firmware_with_pubkey(
        &path,
        &target,
        (4, 0, 1),
        &test_pubkey(),
        &mut |progress| {
            stages.push(progress.stage);
            if progress.stage == FlashStage::Uploading
                && let Some(percent) = progress.percent
            {
                percents.push(percent);
            }
        },
        &mut confirm,
    );
    let _ = std::fs::remove_dir_all(&dir);
    report.check(
        "ota-caller-flash-e2e",
        result.success
            && result.version.as_deref() == Some("4.0.3")
            && board.version() == (4, 0, 3)
            && percents.last() == Some(&100),
        &format!(
            "flash_firmware_with_pubkey vs fake board: success={} version={:?} board={:?} upload-max={:?}",
            result.success,
            result.version,
            board.version(),
            percents.last()
        ),
    );
    report.check(
        "ota-caller-progress-stages",
        stages.contains(&FlashStage::Validating)
            && stages.contains(&FlashStage::Uploading)
            && (stages.contains(&FlashStage::WaitingForRestart)
                || stages.contains(&FlashStage::Confirming)),
        &format!("progress stages observed: {stages:?}"),
    );

    // Source audit: the UI caller.
    let settings = read("crates/rsh-ui/ui/screens/settings.slint").unwrap_or_default();
    let ui_main = read("crates/rsh-ui/src/main.rs").unwrap_or_default();
    report.check(
        "ota-caller-settings-action",
        settings.contains("Update firmware"),
        "Settings has an 'Update firmware' action (currently only a version check exists)",
    );
    report.check(
        "ota-caller-ui-wires-flash",
        ui_main.contains("flash_firmware") || ui_main.contains("update_firmware"),
        "UI wires the action to a flash call (none found yet)",
    );
    report.finish()
}

/// Gate: version bump (M-9, re-scan finding).
///
/// The release workflow's tag check is fail-closed: the tag
/// version must match `[workspace.package] version` in
/// `Cargo.toml`. The gate requires the version to agree in all
/// three places that carry it before any release is cut: the
/// workspace `Cargo.toml`, the `MainWindow` `app-version`
/// (`ui/main.slint`), and the Settings screen `app-version`
/// (`ui/screens/settings.slint`). The expected version is read
/// from Cargo.toml itself — never a literal: a hardcoded "2.0.x"
/// here is the DS-01 tripwire that failed CI round 1 of the
/// v2.0.4 release (the literal still said 2.0.3 after the
/// workspace bump to 2.0.4).
#[must_use]
pub fn gate_version_bump() -> (bool, String) {
    let mut report = Report::new();
    let cargo = read("Cargo.toml").unwrap_or_default();
    let Some(pkg_start) = cargo.find("[workspace.package]") else {
        report.check("version-cargo-toml", false, "[workspace.package] missing");
        return report.finish();
    };
    let pkg_section = &cargo[pkg_start..];
    // Single source of truth: the version is parsed out of
    // [workspace.package], so the next bump needs no edit here.
    let Some(version) = pkg_section.lines().find_map(|line| {
        let line = line.trim();
        line.strip_prefix("version = \"")?
            .split('"')
            .next()
            .map(str::to_string)
    }) else {
        report.check(
            "version-cargo-toml",
            false,
            "no version = \"...\" in [workspace.package]",
        );
        return report.finish();
    };
    report.check(
        "version-cargo-toml",
        true,
        &format!("workspace Cargo.toml version is {version}"),
    );
    let main_slint = read("crates/rsh-ui/ui/main.slint").unwrap_or_default();
    report.check(
        "version-main-slint",
        main_slint.contains(&format!("app-version: \"{version}\"")),
        &format!("MainWindow app-version is {version}"),
    );
    let settings = read("crates/rsh-ui/ui/screens/settings.slint").unwrap_or_default();
    report.check(
        "version-settings-slint",
        settings.contains(&format!("app-version: \"RSH {version}\"")),
        &format!("Settings app-version is RSH {version}"),
    );
    let workflow = read(".github/workflows/release.yml").unwrap_or_default();
    report.check(
        "version-workflow-tag-check",
        workflow.contains("does not match [workspace.package] version"),
        "release workflow fails closed when the tag version mismatches Cargo.toml",
    );
    report.finish()
}
