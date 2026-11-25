//! Sidecar process manager: spawn, hello, request/response, heartbeat,
//! EOF/crash detection, one retry, then vJoy fallback.
//!
//! # Transports
//!
//! * Non-Windows (Linux, tests): stdio child process
//!   ([`transport::spawn_stdio`]).
//! * Windows, elevated (production): the sidecar is launched with
//!   `ShellExecuteEx` (`runas` verb — the UAC prompt) and connects back
//!   over a loopback TCP socket ([`transport::listen_loopback`] /
//!   `accept_loopback`), because handle redirection does not survive a
//!   `runas` launch. The one-time token travels on the sidecar's
//!   command line (`--token <nonce>`, v2.0.12) and is echoed in
//!   `hello` and verified; the accepted socket's peer PID is
//!   additionally verified against the spawned process (C-TOK-1).
//!   The token used to travel in the environment
//!   (`RSH_HM_SIDECAR_TOKEN`, v2.0.11 and earlier) — AIS builds a
//!   FRESH environment for a `runas` child, dropping runtime-set
//!   variables, so the sidecar died with exit 2 before connecting
//!   (proven on the owner's PC, v2.0.11 retest). The nonce is
//!   single-use random and briefly visible in the process list /
//!   WMI — accepted; it is never logged anywhere, only its
//!   presence.
//! * Windows, non-elevated (dev only — works when RSH itself runs
//!   elevated): stdio spawn. A manifest elevation failure (OS error
//!   740) is mapped to [`HidMaestroError::NotAdmin`].
//!
//! # Retry policy (spike §6)
//!
//! No silent respawn loop: the manager counts **consecutive failures**
//! (failed spawn, bad hello, mid-session death, hung reply). The first
//! failure is recoverable — the next `connect()` retries once. A second
//! consecutive failure sets [`SidecarManager::fallback_needed`], and
//! further `connect()` calls fail fast with
//! [`HidMaestroError::FallbackNeeded`] until the user explicitly
//! re-selects the backend ([`SidecarManager::reset_fallback`], wired to
//! the G4 selector).
//!
//! A successful reconnect does **not** by itself reset the counter —
//! otherwise a sidecar that dies on every first submit would earn an
//! endless retry loop. The counter re-arms only after a session proves
//! healthy: [`HEALTHY_SUBMITS_TO_REARM`] consecutive successful submits
//! (1 s of 200 Hz output). A reply error (e.g. a refused `create`) is
//! not a transport failure and does not count.
//!
//! # Heartbeat
//!
//! [`SidecarManager::ensure_alive`] sends a `ping` when no reply has
//! arrived for [`SidecarConfig::heartbeat_interval`]. The backend calls
//! it on every `update()` tick (200 Hz), so a dead sidecar is noticed
//! within ~1 s plus the reply timeout — long before the engine's 250 ms
//! watchdog would matter, and a missed heartbeat is a sidecar death,
//! never a silent stall.

use std::path::PathBuf;
use std::time::{Duration, Instant};

/// R3-D-12: `SIDECAR_PATH_ENV` is only read inside
/// `#[cfg(debug_assertions)]` (`resolve_sidecar_path`) — the
/// unconditional import warned as unused in release builds.
#[cfg(debug_assertions)]
use crate::SIDECAR_PATH_ENV;
use crate::error::HidMaestroError;
use crate::mapping::HidMaestroCapabilities;
use crate::protocol::{
    CapabilitiesJson, Command, Reply, bytes_to_hex, encode_command, parse_reply, reply_to_error,
    validate_hello,
};
use crate::transport::{LiveTransport, TransportEvent, spawn_stdio};
#[cfg(windows)]
use crate::transport::{accept_loopback_with_diagnostics, generate_token, listen_loopback};
use crate::{PROFILE_ID, SIDECAR_EXE_NAME};

/// Timeout budget (P1-1, update4-fix): the UI bound must EXCEED the
/// daemon worst case, or a slow-but-healthy switch is reported as
/// failed while the daemon keeps going (and may still succeed).
///
/// Per-stage bounds, each stage getting its own window:
/// * accept (connect-back) + hello: [`DEFAULT_SPAWN_TIMEOUT`] each
///   (30 s — the sidecar is already running by then; hello is sent
///   before `HMContext` creation, so both are normally < 2 s).
/// * `install-driver` + `create`: [`DEFAULT_DRIVER_OP_TIMEOUT`]
///   each (120 s, P2-1 — a Windows driver install / device creation
///   on a slow or loaded machine takes seconds to a couple of
///   minutes; update4's 60 s killed the sidecar mid-install).
/// * first neutral after create: [`DEFAULT_FIRST_NEUTRAL_TIMEOUT`]
///   (10 s, P2-2 — 50x the normal 200 ms reply bound).
///
/// Daemon worst case after the UAC prompt is answered:
/// 30 + 30 + 120 + 120 + 10 = **310 s**. The UI allows 60 s for the
/// user to answer the UAC prompt on top of that (370 s) — the IPC
/// bound is 390 s and the Backend job timeout 420 s, so
/// **job (420 s) > IPC (390 s) > worst case incl. UAC (370 s) >
/// worst case post-UAC (310 s)**. Residual: the UAC dwell itself is
/// unbounded (the user may stare at the prompt), so no finite UI
/// bound can cover a user who takes longer than 60 s to answer —
/// that case still reports failure while the daemon continues, and
/// is documented, not silently fixed.
pub const DEFAULT_SPAWN_TIMEOUT: Duration = Duration::from_secs(30);
/// How long `install-driver` and `create` may each take (P2-1).
/// 120 s: a slow-but-healthy Windows driver install / device
/// creation finishes inside it; a genuinely hung sidecar still
/// fails in 2 minutes instead of hanging forever. Counts toward
/// the P1-1 budget above (two 120 s stages dominate it).
pub const DEFAULT_DRIVER_OP_TIMEOUT: Duration = Duration::from_secs(120);
/// How long the first neutral after `create` may take (P2-2).
/// 10 s: the first driver I/O after device creation can be slow,
/// and a 200 ms bound there used to fail the whole switch after
/// the device was already created.
pub const DEFAULT_FIRST_NEUTRAL_TIMEOUT: Duration = Duration::from_secs(10);
/// How long to wait for one reply. H4: 200 ms, not 500 ms — the spike
/// measured IPC p99 276 µs at 200 Hz, so 200 ms is still generous, and
/// a reply slower than that would already trip the engine's 250 ms
/// watchdog; failing fast here lets the backend fail over instead of
/// stalling the tick.
pub const DEFAULT_REPLY_TIMEOUT: Duration = Duration::from_millis(200);
/// Heartbeat cadence while connected.
pub const DEFAULT_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);
/// How long an `ensure_alive` failure stays cached (R2-26): while
/// inside the cooldown the cached `SidecarDied` error is returned
/// without a Ping round-trip — a dead sidecar used to cost a full
/// synchronous round-trip (up to `reply_timeout`, 200 ms) on EVERY
/// 200 Hz tick until `fail()` retired the backend. 1 s matches the
/// heartbeat cadence: the next heartbeat attempt is one cooldown
/// away, and a recovered sidecar is re-probed promptly.
pub const ALIVE_FAILURE_COOLDOWN: Duration = Duration::from_secs(1);
/// Successful submits in one session that re-arm the retry budget
/// (1 s of 200 Hz output — a session that survives this long is
/// healthy; a later isolated failure gets a fresh retry).
pub const HEALTHY_SUBMITS_TO_REARM: u64 = 200;

/// Configuration for [`SidecarManager`].
#[derive(Clone, Debug)]
pub struct SidecarConfig {
    /// Path of the sidecar executable.
    pub sidecar_path: PathBuf,
    /// Request elevation on Windows (production default `true`).
    /// On Windows the sidecar is always launched elevated via `runas`
    /// — this flag selects the runas+TCP path (`true`) or the plain
    /// stdio path (`false`, dev only, works when RSH itself runs
    /// elevated). Ignored on non-Windows.
    pub elevated: bool,
    /// Timeout for spawn + `hello`.
    pub spawn_timeout: Duration,
    /// Timeout for the slow driver operations (`install-driver`,
    /// `create`) — see [`DEFAULT_DRIVER_OP_TIMEOUT`].
    pub driver_op_timeout: Duration,
    /// Timeout for the first neutral after `create` — see
    /// [`DEFAULT_FIRST_NEUTRAL_TIMEOUT`].
    pub first_neutral_timeout: Duration,
    /// Timeout for a single reply.
    pub reply_timeout: Duration,
    /// Heartbeat cadence while connected.
    pub heartbeat_interval: Duration,
    /// Extra environment variables for the sidecar child (stub fault
    /// injection in tests; empty in production).
    pub extra_env: Vec<(String, String)>,
    /// Extra arguments for the sidecar child. Empty in production
    /// (the sidecar is a native exe). Used to run the sidecar via
    /// `dotnet <dll>` in dev/sandbox tests.
    pub args: Vec<String>,
}

impl Default for SidecarConfig {
    fn default() -> Self {
        Self {
            sidecar_path: resolve_sidecar_path(),
            elevated: true,
            spawn_timeout: DEFAULT_SPAWN_TIMEOUT,
            driver_op_timeout: DEFAULT_DRIVER_OP_TIMEOUT,
            first_neutral_timeout: DEFAULT_FIRST_NEUTRAL_TIMEOUT,
            reply_timeout: DEFAULT_REPLY_TIMEOUT,
            heartbeat_interval: DEFAULT_HEARTBEAT_INTERVAL,
            extra_env: Vec::new(),
            args: Vec::new(),
        }
    }
}

impl SidecarConfig {
    /// Configuration pointing at an explicit sidecar path (tests, dev).
    #[must_use]
    pub fn with_path(path: PathBuf) -> Self {
        Self {
            sidecar_path: path,
            ..Self::default()
        }
    }
}

/// Resolve the sidecar path: the executable's own directory → the
/// current directory. The DLL is never bundled with RSH; the sidecar
/// loads `HIDMaestro.Core.dll` from beside itself at runtime.
///
/// `$RSH_HM_SIDECAR` overrides the path **in debug builds only**
/// (dev/tests): in a release build the variable is ignored, so
/// anyone who can influence RSH's environment cannot redirect the
/// elevated (`runas`) sidecar launch at an arbitrary executable
/// (H1 — the UAC prompt gates elevation, but the path itself must
/// not be environment-controlled in production).
#[must_use]
pub fn resolve_sidecar_path() -> PathBuf {
    #[cfg(debug_assertions)]
    if let Ok(path) = std::env::var(SIDECAR_PATH_ENV)
        && !path.is_empty()
    {
        return PathBuf::from(path);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        return dir.join(SIDECAR_EXE_NAME);
    }
    PathBuf::from(SIDECAR_EXE_NAME)
}

/// What `connect()` learned from the sidecar's `hello`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HelloInfo {
    /// SDK version the sidecar reports (pinned `2.1.0`).
    pub sdk_version: Option<String>,
    /// True when the sidecar runs elevated.
    pub admin: bool,
    /// True when the driver is installed.
    pub driver_installed: bool,
    /// Transport in use (`"stdio"` / `"tcp-loopback"`).
    pub transport: Option<String>,
    /// SHA-256 the sidecar measured for `HIDMaestro.Core.dll`
    /// beside itself (`None` when the DLL was missing — the
    /// v2.0.8 retest failure mode). Kept so the hello verdict is
    /// preserved structurally, not only inside an error string.
    pub dll_sha256: Option<String>,
    /// Whether the measured DLL hash matched the pin.
    pub dll_sha256_ok: Option<bool>,
}

/// What `create()` learned about the created controller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateInfo {
    /// Transport index of the created controller.
    pub transport_index: Option<i32>,
    /// Driver version (`"2.1.0"` form).
    pub driver_version: Option<String>,
    /// Capabilities the controller reports.
    pub capabilities: HidMaestroCapabilities,
}

/// What `install_driver()` learned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InstallInfo {
    /// Whether the driver was already installed (fast path).
    pub was_installed: bool,
    /// Whether the driver is installed now.
    pub now_installed: bool,
    /// `InstallDriver()` outcome (`"AlreadyCurrent"`, …).
    pub outcome: Option<&'static str>,
}

/// The sidecar process manager.
pub struct SidecarManager {
    config: SidecarConfig,
    transport: Option<LiveTransport>,
    token: Option<String>,
    /// Windows elevated spawn: the sidecar's process handle address
    /// (`SEE_MASK_NOCLOSEPROCESS`, M4) — `kill_transport` terminates
    /// it, so a hung sidecar cannot outlive the manager. Stored as a
    /// raw `usize`, not a `HANDLE`: `HANDLE` is not `Send`, and the
    /// backend must stay `Send` for the engine slot.
    #[cfg(windows)]
    elevated_process: Option<usize>,
    /// Windows elevated spawn: the sidecar's PID (P0, v2.0.11 — the
    /// spawn PID per switch attempt, carried in the
    /// `SidecarDied` message so the daemon log names the process
    /// that died before hello).
    #[cfg(windows)]
    last_spawn_pid: Option<u32>,
    /// When the current spawn + hello attempt started (P0, v2.0.11):
    /// the accept→EOF elapsed ms in the `SidecarDied` message tells
    /// an instant native crash apart from a slow clean exit. P1-1
    /// (update4-fix): the clock starts AFTER the spawn returns —
    /// `ShellExecuteEx` (`runas`) blocks on the UAC prompt, so a
    /// clock started before the spawn inflated the elapsed ms with
    /// the user's UAC dwell. Set at accept start on the elevated
    /// path, right after the stdio spawn on the stdio path.
    hello_started: Option<Instant>,
    last_hello: Option<HelloInfo>,
    consecutive_failures: u32,
    fallback_needed: bool,
    /// Whether the user declined the UAC prompt this session
    /// (B-2/A10, v2.0.2): while set, [`connect`](Self::connect)
    /// fails fast with [`NotAdmin`](HidMaestroError::NotAdmin)
    /// instead of spawning (and prompting) again. Only an explicit
    /// re-select ([`reset_fallback`](Self::reset_fallback)) clears
    /// it. This is the manager-level latch; the app-core
    /// `elevation_declined` flag is the primary guard (it survives
    /// the fresh-factory re-arm that replaces this manager).
    elevation_declined: bool,
    healthy_submits: u64,
    last_reply: Option<Instant>,
    /// When the last `ensure_alive` heartbeat failed (R2-26): while
    /// this is within [`ALIVE_FAILURE_COOLDOWN`] of now, the failure
    /// is returned from cache instead of another synchronous Ping
    /// round-trip on the 200 Hz tick. Cleared on the next successful
    /// heartbeat.
    last_alive_failure: Option<Instant>,
    last_submit_latency_us: Option<f64>,
    submit_count: u64,
    transport_index: Option<i32>,
    driver_version: Option<String>,
    sdk_version: Option<String>,
}

impl SidecarManager {
    /// Create a manager. Nothing is spawned yet — call
    /// [`connect`](Self::connect).
    #[must_use]
    pub fn new(config: SidecarConfig) -> Self {
        Self {
            config,
            transport: None,
            token: None,
            #[cfg(windows)]
            elevated_process: None,
            #[cfg(windows)]
            last_spawn_pid: None,
            hello_started: None,
            last_hello: None,
            consecutive_failures: 0,
            fallback_needed: false,
            elevation_declined: false,
            healthy_submits: 0,
            last_reply: None,
            last_alive_failure: None,
            last_submit_latency_us: None,
            submit_count: 0,
            transport_index: None,
            driver_version: None,
            sdk_version: None,
        }
    }

    /// Create a manager with the default config (resolved sidecar path).
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(SidecarConfig::default())
    }

    /// True while a sidecar transport is live (not proof the driver is
    /// up — use [`ensure_alive`](Self::ensure_alive) for that).
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.transport.is_some()
    }

    /// True once the one retry was used up — the backend must fall back
    /// to vJoy until the user re-selects `HIDMaestro`.
    #[must_use]
    pub fn fallback_needed(&self) -> bool {
        self.fallback_needed
    }

    /// Consecutive failures since the last success (0–2).
    #[must_use]
    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures
    }

    /// Clear the fallback latch (user explicitly re-selected the
    /// backend in Output settings — G4). Also resets the failure
    /// counter so the backend gets a fresh initial attempt + retry,
    /// and clears the UAC-decline latch (B-2): an explicit re-select
    /// is a new action, so one fresh prompt is allowed.
    pub fn reset_fallback(&mut self) {
        self.fallback_needed = false;
        self.elevation_declined = false;
        self.consecutive_failures = 0;
        self.healthy_submits = 0;
    }

    /// Whether the user declined the UAC prompt this session.
    #[must_use]
    pub fn elevation_declined(&self) -> bool {
        self.elevation_declined
    }

    /// Last submit latency the sidecar reported (`submit_us`), if any.
    #[must_use]
    pub fn last_submit_latency_us(&self) -> Option<f64> {
        self.last_submit_latency_us
    }

    /// Number of successful submits this session.
    #[must_use]
    pub fn submit_count(&self) -> u64 {
        self.submit_count
    }

    /// Transport index of the created controller, if any.
    #[must_use]
    pub fn transport_index(&self) -> Option<i32> {
        self.transport_index
    }

    /// Driver version the sidecar reported, if any.
    #[must_use]
    pub fn driver_version(&self) -> Option<String> {
        self.driver_version.clone()
    }

    /// SDK version the sidecar reported, if any.
    #[must_use]
    pub fn sdk_version(&self) -> Option<String> {
        self.sdk_version.clone()
    }

    /// Recent stderr lines from the sidecar (diagnostics; empty on the
    /// TCP path).
    #[must_use]
    pub fn last_stderr(&self) -> Vec<String> {
        self.transport.as_ref().map_or_else(Vec::new, |t| {
            t.stderr_lines
                .lock()
                .map_or_else(|_| Vec::new(), |g| g.iter().cloned().collect())
        })
    }

    /// Spawn the sidecar (if needed) and validate its `hello`.
    ///
    /// A second call while running is a no-op success. After the one
    /// retry is used up this fails fast with
    /// [`HidMaestroError::FallbackNeeded`] without spawning.
    ///
    /// # Errors
    ///
    /// [`HidMaestroError::FallbackNeeded`] once the retry budget is
    /// spent; `SidecarNotFound` / `SpawnFailed` / `NotAdmin` /
    /// `HelloTimeout` / `BadHello` / `DllHashMismatch` /
    /// `VersionMismatch` / `SidecarDied` on spawn or hello failure.
    pub fn connect(&mut self) -> Result<HelloInfo, HidMaestroError> {
        // B-2/A10: a declined UAC prompt fails fast — no second
        // spawn, no second prompt, no retry-budget burn.
        if self.elevation_declined {
            return Err(HidMaestroError::NotAdmin);
        }
        if self.fallback_needed {
            return Err(HidMaestroError::FallbackNeeded);
        }
        if self.transport.is_some() {
            // M8: a transport that exists is not proof the sidecar is
            // alive — drain the event channel without blocking. A
            // queued EOF means the sidecar died since the last call:
            // drop the transport and respawn (one fresh attempt) so a
            // dead sidecar is never reported as connected.
            match self.check_liveness() {
                Ok(()) => return Ok(self.last_hello.clone().unwrap_or_default()),
                Err(err) => {
                    self.kill_transport();
                    self.note_failure();
                    if self.fallback_needed {
                        return Err(HidMaestroError::FallbackNeeded);
                    }
                    // Fall through to spawn_and_hello for the retry;
                    // its own failure path counts the second failure.
                    let _ = err;
                }
            }
        }
        match self.spawn_and_hello() {
            Ok(info) => {
                // A fresh session starts unproven: the failure counter
                // is NOT reset here (see the retry-policy docs) — only
                // HEALTHY_SUBMITS_TO_REARM successful submits re-arm it.
                self.healthy_submits = 0;
                // `clone_from` reuses the stored string's allocation;
                // the stored hello still needs its own copy below.
                self.sdk_version.clone_from(&info.sdk_version);
                self.last_hello = Some(info.clone());
                Ok(info)
            }
            Err(err) => {
                self.kill_transport();
                self.note_failure();
                Err(err)
            }
        }
    }

    fn spawn_and_hello(&mut self) -> Result<HelloInfo, HidMaestroError> {
        // P1-1: `hello_started` is NOT set here — the elevated spawn
        // below blocks on the UAC prompt, and the clock must start
        // after the spawn returns (set in each path below).
        if !self.config.sidecar_path.is_file() {
            return Err(HidMaestroError::SidecarNotFound(
                self.config.sidecar_path.clone(),
            ));
        }
        #[cfg(windows)]
        {
            if self.config.elevated {
                return self.spawn_windows_elevated();
            }
        }
        let transport = spawn_stdio(
            &self.config.sidecar_path,
            &self.config.args,
            &self.config.extra_env,
        )
        .map_err(|e| map_spawn_error(&e))?;
        self.token = None;
        self.transport = Some(transport);
        // P1-1: spawn has returned — start the hello clock now.
        self.hello_started = Some(Instant::now());
        self.read_hello(None)
    }

    /// Non-blocking liveness check on an existing transport (M8):
    /// drain whatever the reader thread already queued. `Eof` means
    /// the sidecar died; stray lines (a reply with no request in
    /// flight — e.g. a late reply to a timed-out request) are
    /// discarded so they cannot desync the next request.
    ///
    /// # Errors
    ///
    /// [`HidMaestroError::SidecarDied`] when an EOF is queued.
    fn check_liveness(&mut self) -> Result<(), HidMaestroError> {
        let Some(transport) = self.transport.as_mut() else {
            return Ok(());
        };
        loop {
            match transport.events.try_recv() {
                Ok(TransportEvent::Line(_)) => {} // stray line: discard, keep draining
                Ok(TransportEvent::Eof) => {
                    return Err(HidMaestroError::SidecarDied(
                        "sidecar exited (EOF queued)".to_string(),
                    ));
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return Ok(()),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return Err(HidMaestroError::SidecarDied(
                        "sidecar reader thread is gone".to_string(),
                    ));
                }
            }
        }
    }

    /// Windows elevated spawn: loopback listener + `runas` launch +
    /// connect-back accept, then `hello` (token-checked).
    #[cfg(windows)]
    fn spawn_windows_elevated(&mut self) -> Result<HelloInfo, HidMaestroError> {
        let (listener, port) = listen_loopback().map_err(HidMaestroError::from)?;
        let token = generate_token().map_err(HidMaestroError::from)?;
        // v2.0.12 (H1 — PROVEN on the owner's PC): the token travels
        // on the sidecar's command line (`--token <nonce>`), NOT in
        // its environment. The old env transport
        // (`RSH_HM_SIDECAR_TOKEN`, set just for the spawn) is dead:
        // a non-elevated daemon launches the elevated sidecar via
        // `ShellExecuteEx "runas"`, and AIS builds a FRESH
        // environment for the elevated child, dropping the
        // runtime-set variable — the sidecar logged `bad-args:
        // --connect-back needs RSH_HM_SIDECAR_TOKEN...` and exited 2
        // before connecting. The old comment's claim ("the child
        // inherits a copy") was FALSE through AIS. The nonce is
        // single-use random (128-bit hex) and briefly visible in the
        // process list / WMI — accepted; it is NEVER logged anywhere
        // (presence only, never the value).
        let spawned = spawn_elevated_runas(&self.config.sidecar_path, port, &token);
        // B-2/A10: a declined UAC prompt latches the decline — the
        // next `connect()` fails fast with NotAdmin instead of
        // prompting again.
        let process = match spawned {
            Ok(process) => process,
            Err(error) => {
                if matches!(error, HidMaestroError::NotAdmin) {
                    self.elevation_declined = true;
                }
                return Err(error);
            }
        };
        // Keep the process handle BEFORE waiting for the connect-back:
        // when the accept fails (slow UAC, foreign peer, I/O error) the
        // elevated sidecar must be terminated and its handle closed,
        // not leaked (a retry would otherwise spawn a second one).
        // Stored before the PID check so the pid-0 fail-fast below
        // can terminate it via `kill_transport`.
        self.elevated_process = Some(process);
        let pid = {
            use windows::Win32::Foundation::HANDLE;
            use windows::Win32::System::Threading::GetProcessId;
            // SAFETY: `process` is a live handle from `ShellExecuteEx`
            // with `SEE_MASK_NOCLOSEPROCESS`.
            unsafe { GetProcessId(HANDLE(process as *mut std::ffi::c_void)) }
        };
        // (2b, v2.0.12) H4 pid-0 hole: `GetProcessId` returns 0 for
        // an invalid handle — fail fast with `SpawnFailed`, never a
        // silent 30 s accept timeout on a process that was never
        // really spawned.
        if pid == 0 {
            self.kill_transport();
            return Err(HidMaestroError::SpawnFailed(
                "elevated sidecar spawn returned PID 0 (no live process — refusing to wait out the 30 s accept timeout)"
                    .to_string(),
            ));
        }
        // P0 (v2.0.11): keep the spawn PID for the `SidecarDied`
        // message — the daemon log then names the process that died
        // before hello, per switch attempt.
        self.last_spawn_pid = Some(pid);
        // P1-1: the spawn (and the UAC prompt) has returned — start
        // the hello clock now, at accept start, so the accept→EOF
        // elapsed ms never includes the UAC dwell.
        self.hello_started = Some(Instant::now());
        let mut accept_diagnostics = Vec::new();
        let transport = match accept_loopback_with_diagnostics(
            &listener,
            self.config.spawn_timeout,
            Some(pid),
            &mut accept_diagnostics,
        ) {
            Ok(transport) => transport,
            Err(error) => {
                // (2a, v2.0.12) P0: the accept-failure diagnostic —
                // PID + exit code + elapsed ms — is captured BEFORE
                // `kill_transport` (which takes the handle the exit
                // code is read from) and folded into the returned
                // error, which lands in the daemon log via the
                // switch-failure line (`daemon_api::set_output_backend`
                // logs `set_output_backend(HIDMaestro) failed:
                // <reason>` — the same channel the v2.0.11 P0
                // `SidecarDied` enrichment already uses). Exit 2 =
                // the sidecar exited fast on its own (bad-args — see
                // its file log); 259 = still running but never
                // connected back; anything else = crash code.
                let message = self.describe_accept_failure(&error, &accept_diagnostics);
                self.kill_transport();
                return Err(HidMaestroError::Io(message));
            }
        };
        self.token = Some(token.clone());
        self.transport = Some(transport);
        self.read_hello(Some(token.as_str()))
    }

    /// Accept-failure diagnostic (2a, v2.0.12): `{source} (pid …,
    /// exit code …, N ms since spawn; …)` plus any foreign-peer
    /// drops (2c). Windows-only: the exit code comes from
    /// `GetExitCodeProcess` on the stored elevated-process handle,
    /// so this MUST be called before `kill_transport` (which closes
    /// the handle). The pure formatting lives in
    /// [`format_accept_failure`] so it is unit-testable off Windows.
    #[cfg(windows)]
    fn describe_accept_failure(&self, source: &std::io::Error, foreign_drops: &[String]) -> String {
        let elapsed_ms = self
            .hello_started
            .map_or(0, |started| started.elapsed().as_millis());
        format_accept_failure(
            &source.to_string(),
            self.last_spawn_pid,
            self.sidecar_exit_code(),
            elapsed_ms,
            foreign_drops,
        )
    }

    fn read_hello(&mut self, expected_token: Option<&str>) -> Result<HelloInfo, HidMaestroError> {
        let Some(transport) = self.transport.as_mut() else {
            return Err(HidMaestroError::SidecarDied(
                "no transport while reading hello".to_string(),
            ));
        };
        match transport.events.recv_timeout(self.config.spawn_timeout) {
            Ok(TransportEvent::Line(line)) => {
                let reply = parse_reply(&line)?;
                validate_hello(&reply, expected_token)?;
                self.last_reply = Some(Instant::now());
                Ok(HelloInfo {
                    sdk_version: reply.sdk.clone(),
                    admin: reply.admin.unwrap_or(false),
                    driver_installed: reply.driver_installed.unwrap_or(false),
                    transport: reply.transport.clone(),
                    dll_sha256: reply.dll_sha256.clone(),
                    dll_sha256_ok: reply.dll_sha256_ok,
                })
            }
            Ok(TransportEvent::Eof) => {
                // P0 (v2.0.11): the v2.0.10 failure mode — the sidecar
                // died between connect and hello — used to report
                // only "sidecar exited before hello", with no PID, no
                // exit code, and no timing, so a native crash
                // (0xC0000005, instant) was indistinguishable from a
                // clean exit. The enriched message lands in the
                // daemon log via the switch-failure line, and the
                // sidecar's own file log
                // (%PROGRAMDATA%\RSH\logs\rsh-sidecar-YYYYMMDD.log)
                // carries its startup milestones.
                let elapsed_ms = self
                    .hello_started
                    .map_or(0, |started| started.elapsed().as_millis());
                Err(HidMaestroError::SidecarDied(
                    self.describe_hello_eof(elapsed_ms),
                ))
            }
            Err(_) => Err(HidMaestroError::HelloTimeout),
        }
    }

    /// Describe a sidecar death before hello (P0, v2.0.11): PID +
    /// exit code + accept→EOF elapsed ms. On Windows the exit code
    /// comes from `GetExitCodeProcess` on the stored elevated-process
    /// handle — `0xC0000005` is the native access-violation signature
    /// (the suspected `new HMContext()` crash), `2` is the sidecar's
    /// own clean fast-exit code, `259` (`STILL_ACTIVE`) means the
    /// socket closed while the process was still alive. Off Windows
    /// (stdio path) there is no process handle, so only the timing
    /// is reported.
    ///
    /// `self` is only used on Windows (PID + exit code); off
    /// Windows the method is a plain formatter — the `allow` keeps
    /// clippy's `unused_self` quiet on non-Windows targets instead
    /// of splitting the method per platform.
    #[allow(clippy::unused_self)]
    fn describe_hello_eof(&self, elapsed_ms: u128) -> String {
        self.describe_eof("sidecar exited before hello", elapsed_ms, "after spawn")
    }

    /// Describe a sidecar death AFTER hello (P1-3, update4-fix):
    /// the same PID + exit-code + timing enrichment as the
    /// pre-hello path — the old bare "sidecar closed the
    /// connection" could not tell a native crash from a clean exit
    /// or say how long the session had lived. Elapsed is since the
    /// last reply (or since spawn when no reply ever arrived).
    /// Must be called BEFORE `kill_transport`: killing takes the
    /// elevated-process handle, after which the exit code is
    /// unknowable (see [`request_with_timeout`](Self::request_with_timeout)).
    fn describe_post_hello_eof(&self) -> String {
        let elapsed_ms = self.last_reply.map_or_else(
            || {
                self.hello_started
                    .map_or(0, |started| started.elapsed().as_millis())
            },
            |last| last.elapsed().as_millis(),
        );
        self.describe_eof(
            "sidecar closed the connection",
            elapsed_ms,
            "since last reply",
        )
    }

    /// Shared death description: `{what} (pid …, exit code …, N ms
    /// {elapsed_label}; sidecar log: …)`. `what` names the phase
    /// ("exited before hello" / "closed the connection") and
    /// `elapsed_label` names the clock ("after spawn" /
    /// "since last reply").
    #[allow(clippy::unused_self)]
    fn describe_eof(&self, what: &str, elapsed_ms: u128, elapsed_label: &str) -> String {
        #[cfg(windows)]
        {
            let pid = self
                .last_spawn_pid
                .map_or("?".to_string(), |pid| pid.to_string());
            match self.sidecar_exit_code() {
                Some(259) => format!(
                    "{what} (pid {pid}, process still running, {elapsed_ms} ms {elapsed_label}; sidecar log: %PROGRAMDATA%\\RSH\\logs\\rsh-sidecar-YYYYMMDD.log)"
                ),
                Some(code) => format!(
                    "{what} (pid {pid}, exit code 0x{code:08X}, {elapsed_ms} ms {elapsed_label}; sidecar log: %PROGRAMDATA%\\RSH\\logs\\rsh-sidecar-YYYYMMDD.log)"
                ),
                None => format!(
                    "{what} (pid {pid}, exit code unknown, {elapsed_ms} ms {elapsed_label}; sidecar log: %PROGRAMDATA%\\RSH\\logs\\rsh-sidecar-YYYYMMDD.log)"
                ),
            }
        }
        #[cfg(not(windows))]
        {
            format!("{what} ({elapsed_ms} ms {elapsed_label})")
        }
    }

    /// The sidecar's exit code via `GetExitCodeProcess` on the stored
    /// elevated-process handle (P0, v2.0.11). `None` when no handle
    /// was stored or the query fails — the caller reports "unknown"
    /// instead of guessing.
    #[cfg(windows)]
    fn sidecar_exit_code(&self) -> Option<u32> {
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::Threading::GetExitCodeProcess;
        let process = self.elevated_process?;
        let handle = HANDLE(process as *mut std::ffi::c_void);
        let mut code = 0u32;
        // SAFETY: `handle` is the live `SEE_MASK_NOCLOSEPROCESS`
        // handle stored at spawn; `kill_transport` closes it exactly
        // once, after this read.
        unsafe { GetExitCodeProcess(handle, &raw mut code).ok()? };
        Some(code)
    }

    /// Send one command and wait for its reply (one in flight at a
    /// time — the sidecar answers in order).
    ///
    /// A transport death or a reply timeout kills the transport and
    /// counts as a failure against the retry budget.
    ///
    /// # Errors
    ///
    /// `NotConnected` when no sidecar is running; `SidecarDied` /
    /// `ReplyTimeout` / `BadReply` on transport failure; the reply's
    /// own error is *not* mapped here — use
    /// [`request_checked`](Self::request_checked) for that.
    pub fn request(&mut self, cmd: &Command) -> Result<Reply, HidMaestroError> {
        self.request_with_timeout(cmd, self.config.reply_timeout)
    }

    /// [`request`](Self::request) with an explicit reply bound, for
    /// the slow driver operations (`install-driver`, `create`): the
    /// 200 ms default is meant for submit / ping, and a driver
    /// install takes seconds — it used to time out, kill the elevated
    /// sidecar mid-install and latch the vJoy fallback.
    ///
    /// # Errors
    ///
    /// Same as [`request`](Self::request).
    pub fn request_with_timeout(
        &mut self,
        cmd: &Command,
        timeout: Duration,
    ) -> Result<Reply, HidMaestroError> {
        let line = encode_command(cmd)?;
        let Some(transport) = self.transport.as_mut() else {
            return Err(HidMaestroError::NotConnected);
        };
        if let Err(e) = transport.writer.write_line(&line) {
            self.kill_transport();
            self.note_failure();
            return Err(HidMaestroError::SidecarDied(e.to_string()));
        }
        match transport.events.recv_timeout(timeout) {
            Ok(TransportEvent::Line(line)) => {
                self.last_reply = Some(Instant::now());
                parse_reply(&line)
            }
            Ok(TransportEvent::Eof) => {
                // P1-3: the post-hello death carries the same
                // enrichment as the pre-hello one (PID + exit code +
                // ms since last reply) — captured BEFORE
                // `kill_transport`, which takes the elevated-process
                // handle the exit code is read from.
                let message = self.describe_post_hello_eof();
                self.kill_transport();
                self.note_failure();
                Err(HidMaestroError::SidecarDied(message))
            }
            Err(_) => {
                self.kill_transport();
                self.note_failure();
                Err(HidMaestroError::ReplyTimeout)
            }
        }
    }

    /// [`request`](Self::request) plus mapping a failed reply onto its
    /// error.
    ///
    /// # Errors
    ///
    /// Same as [`request`](Self::request), plus the reply's mapped
    /// error (`NotAdmin`, `CreateFailed`, …).
    pub fn request_checked(&mut self, cmd: &Command) -> Result<Reply, HidMaestroError> {
        let reply = self.request(cmd)?;
        reply_to_error(&reply)?;
        Ok(reply)
    }

    /// Heartbeat: send a `ping` when no reply has arrived for the
    /// heartbeat interval. No-op when nothing is running or when the
    /// last reply is fresh. A failed heartbeat is a sidecar death
    /// (handled inside [`request`](Self::request)).
    ///
    /// # Errors
    ///
    /// Propagates a heartbeat failure (`SidecarDied` / `ReplyTimeout`).
    pub fn ensure_alive(&mut self) -> Result<(), HidMaestroError> {
        if self.transport.is_none() {
            return Ok(());
        }
        let due = self
            .last_reply
            .is_none_or(|t| t.elapsed() >= self.config.heartbeat_interval);
        if !due {
            return Ok(());
        }
        // R2-26: a recent heartbeat failure is returned from cache
        // for one cooldown — without this, a dead sidecar costs a
        // full synchronous Ping round-trip (up to `reply_timeout`,
        // 200 ms) on EVERY 200 Hz tick until `fail()` retires the
        // backend.
        if let Some(failed_at) = self.last_alive_failure
            && failed_at.elapsed() < ALIVE_FAILURE_COOLDOWN
        {
            return Err(HidMaestroError::SidecarDied(
                "sidecar heartbeat failed recently (cached)".to_string(),
            ));
        }
        let outcome = self
            .request(&Command::Ping)
            .and_then(|reply| reply_to_error(&reply));
        match outcome {
            Ok(()) => {
                self.last_alive_failure = None;
                Ok(())
            }
            Err(error) => {
                self.last_alive_failure = Some(Instant::now());
                Err(error)
            }
        }
    }

    /// `install-driver` with the `AlreadyCurrent` fast path (the
    /// sidecar skips the install call when the driver is present).
    ///
    /// # Errors
    ///
    /// Propagates request errors plus `NotAdmin` / `DriverInstallFailed`.
    pub fn install_driver(&mut self) -> Result<InstallInfo, HidMaestroError> {
        let reply =
            self.request_with_timeout(&Command::InstallDriver, self.config.driver_op_timeout)?;
        reply_to_error(&reply)?;
        Ok(InstallInfo {
            was_installed: reply.was_installed.unwrap_or(false),
            now_installed: reply.now_installed.unwrap_or(false),
            outcome: reply.outcome.as_deref().map(outcome_static),
        })
    }

    /// `create` for the custom wheel profile. Only
    /// [`PROFILE_ID`] is ever requested.
    ///
    /// # Errors
    ///
    /// Propagates request errors plus `ProfileRefused` / `CreateFailed`.
    pub fn create(&mut self) -> Result<CreateInfo, HidMaestroError> {
        let reply = self.request_with_timeout(
            &Command::Create {
                profile: PROFILE_ID.to_string(),
            },
            self.config.driver_op_timeout,
        )?;
        reply_to_error(&reply)?;
        // C2: parse the REAL sidecar shape — `axes` is a count
        // (`axes == 5` ⇒ all five axes). No `capabilities` object
        // (older sidecar) falls back to the custom-wheel caps.
        let caps = reply.capabilities.map_or_else(
            HidMaestroCapabilities::custom_wheel,
            CapabilitiesJson::to_capabilities,
        );
        self.transport_index = reply.transport_index;
        // `clone_from` reuses the stored string's allocation.
        self.driver_version.clone_from(&reply.driver);
        Ok(CreateInfo {
            transport_index: reply.transport_index,
            driver_version: reply.driver.clone(),
            capabilities: caps,
        })
    }

    /// Submit one frame (`SubmitState` path). Returns the sidecar's
    /// `submit_us` sample.
    ///
    /// # Errors
    ///
    /// Propagates request errors; a failed reply maps to
    /// [`HidMaestroError::UpdateFailed`] (the sidecar has no
    /// controller, or the driver refused the frame).
    pub fn submit(&mut self, frame: crate::GamepadFrame) -> Result<f64, HidMaestroError> {
        let reply = self.request(&Command::Submit {
            axes: frame.into(),
            buttons: frame.buttons,
        })?;
        if !reply.ok {
            let msg = reply
                .error
                .clone()
                .unwrap_or_else(|| "submit refused".to_string());
            return Err(HidMaestroError::UpdateFailed(msg));
        }
        self.record_submit(&reply);
        Ok(reply.submit_us.unwrap_or(0.0))
    }

    /// Submit one raw report (`SubmitRawReport` path). The report is sent
    /// hex-encoded (see `PROTOCOL.md`); it must be exactly the derived
    /// size (the sidecar refuses anything else).
    ///
    /// # Errors
    ///
    /// Same as [`submit`](Self::submit). A wrong-length report is
    /// refused by the sidecar before any driver call.
    pub fn submit_raw(&mut self, report: &[u8]) -> Result<f64, HidMaestroError> {
        let encoded = bytes_to_hex(report);
        let reply = self.request(&Command::SubmitRaw { bytes: encoded })?;
        if !reply.ok {
            let msg = reply
                .error
                .clone()
                .unwrap_or_else(|| "raw submit refused".to_string());
            return Err(HidMaestroError::UpdateFailed(msg));
        }
        self.record_submit(&reply);
        Ok(reply.submit_us.unwrap_or(0.0))
    }

    fn record_submit(&mut self, reply: &Reply) {
        if let Some(us) = reply.submit_us {
            self.last_submit_latency_us = Some(us);
        }
        if let Some(us) = reply.on_submit_latency_us {
            // The SDK's own per-submit hook, when wired — prefer it
            // over the wall-clock sample for diagnostics. Latency
            // microseconds are far below 2^53, so the cast is exact.
            #[allow(clippy::cast_precision_loss)]
            let us_f = us as f64;
            self.last_submit_latency_us = Some(us_f);
        }
        self.submit_count += 1;
        // A session that survives this many submits is healthy: the
        // retry budget re-arms, so a later isolated failure gets a
        // fresh retry instead of an immediate fallback.
        self.healthy_submits += 1;
        if self.healthy_submits >= HEALTHY_SUBMITS_TO_REARM {
            self.consecutive_failures = 0;
            self.healthy_submits = 0;
        }
    }

    /// Send the sidecar's `neutral` command (L2 — Rust USES it for
    /// watchdog / shutdown neutral, instead of re-submitting a neutral
    /// frame). `raw: true` sends the 15-byte neutral raw report.
    /// A successful neutral counts as a healthy submit (it proves the
    /// sidecar + driver are alive) and re-arms the retry budget the
    /// same way a submit does.
    ///
    /// # Errors
    ///
    /// Propagates request errors; a failed reply maps to
    /// [`HidMaestroError::UpdateFailed`].
    pub fn neutral(&mut self, raw: bool) -> Result<(), HidMaestroError> {
        self.neutral_with_timeout(raw, self.config.reply_timeout)
    }

    /// The first neutral after `create` (P2-2): the same command
    /// with the longer [`DEFAULT_FIRST_NEUTRAL_TIMEOUT`] bound —
    /// the first driver I/O after device creation can be slow, and
    /// the 200 ms bound used to fail the whole switch after the
    /// device was already created. Counts as a healthy submit like
    /// [`neutral`](Self::neutral).
    ///
    /// # Errors
    ///
    /// Same as [`neutral`](Self::neutral).
    pub fn neutral_first(&mut self, raw: bool) -> Result<(), HidMaestroError> {
        self.neutral_with_timeout(raw, self.config.first_neutral_timeout)
    }

    /// [`neutral`](Self::neutral) with an explicit reply bound.
    ///
    /// # Errors
    ///
    /// Propagates request errors; a failed reply maps to
    /// [`HidMaestroError::UpdateFailed`].
    pub fn neutral_with_timeout(
        &mut self,
        raw: bool,
        timeout: Duration,
    ) -> Result<(), HidMaestroError> {
        let reply = self.request_with_timeout(&Command::Neutral { raw }, timeout)?;
        if !reply.ok {
            let msg = reply
                .error
                .clone()
                .unwrap_or_else(|| "neutral refused".to_string());
            return Err(HidMaestroError::UpdateFailed(msg));
        }
        self.record_submit(&reply);
        Ok(())
    }

    /// Send the sidecar's `dispose` command (L2 — Rust USES it on
    /// shutdown, before `exit`). Best-effort callers should use
    /// [`shutdown`](Self::shutdown) instead, which never waits for a
    /// reply.
    ///
    /// # Errors
    ///
    /// Propagates request errors plus the reply's mapped error.
    pub fn dispose(&mut self) -> Result<Reply, HidMaestroError> {
        self.request_checked(&Command::Dispose)
    }

    /// Best-effort shutdown (H4): fire `dispose` + `exit` into the
    /// writer queue and kill the transport immediately — NEVER wait
    /// for an `exit` reply. The sidecar disposes the device on stdin
    /// EOF / socket close as well, so killing the transport is itself
    /// the clean-shutdown signal; waiting for the reply used to stall
    /// the caller for up to the reply timeout. Never panics, never
    /// blocks long. Idempotent.
    pub fn shutdown(&mut self) {
        if self.transport.is_none() {
            return;
        }
        // Best-effort `dispose` + `exit`, queued only — no reply is
        // awaited (see above). Any failure falls through to the kill.
        if let Some(transport) = self.transport.as_mut() {
            for cmd in [Command::Dispose, Command::Exit] {
                if let Ok(line) = encode_command(&cmd) {
                    let _ = transport.writer.write_line(&line);
                }
            }
        }
        self.kill_transport();
        self.token = None;
        self.last_reply = None;
    }

    fn kill_transport(&mut self) {
        if let Some(mut transport) = self.transport.take() {
            transport.writer.kill();
        }
        // M4: terminate the elevated sidecar process itself (the
        // handle was kept via `SEE_MASK_NOCLOSEPROCESS`) — closing
        // the pipe alone leaves a hung elevated process holding the
        // device.
        #[cfg(windows)]
        if let Some(process) = self.elevated_process.take() {
            use windows::Win32::Foundation::{CloseHandle, HANDLE};
            use windows::Win32::System::Threading::TerminateProcess;
            // SAFETY: `process` is the live handle address stored at
            // spawn; it is reconstructed and closed exactly once, here.
            let handle = HANDLE(process as *mut std::ffi::c_void);
            unsafe {
                let _ = TerminateProcess(handle, 1);
                let _ = CloseHandle(handle);
            }
        }
    }

    fn note_failure(&mut self) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        if self.consecutive_failures >= 2 {
            self.fallback_needed = true;
        }
    }
}

impl Drop for SidecarManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Pure accept-failure formatter (2a/2c, v2.0.12): the source error
/// plus spawn PID, sidecar exit code, accept→deadline elapsed ms,
/// and any foreign-peer drops — never the token (it is not an
/// argument here by design; the caller must not pass it). `exit_code`
/// `Some(259)` (`STILL_ACTIVE`) means the process was alive but
/// never connected back; `Some(2)` is the sidecar's own fast-exit
/// code (bad-args — the sidecar file log names it); `None` means the
/// code could not be read. Kept free of Windows APIs so the
/// formatting is unit-testable on Linux; the Windows-only
/// [`SidecarManager::describe_accept_failure`] gathers the values.
/// Windows-only in non-test builds (its only production caller is
/// Windows-only); compiled on every target under `test` for the
/// unit tests below.
#[cfg(any(windows, test))]
fn format_accept_failure(
    source: &str,
    pid: Option<u32>,
    exit_code: Option<u32>,
    elapsed_ms: u128,
    foreign_drops: &[String],
) -> String {
    let pid = pid.map_or("?".to_string(), |pid| pid.to_string());
    let exit = match exit_code {
        Some(259) => "process still running".to_string(),
        Some(code) => format!("exit code 0x{code:08X}"),
        None => "exit code unknown".to_string(),
    };
    let mut message = format!(
        "{source} (pid {pid}, {exit}, {elapsed_ms} ms since spawn; exit 2 = sidecar exited fast on its own — see its file log; 259 = still running but never connected back; sidecar log: %PROGRAMDATA%\\RSH\\logs\\rsh-sidecar-YYYYMMDD.log)"
    );
    if !foreign_drops.is_empty() {
        message.push_str("; foreign loopback peers dropped: ");
        message.push_str(&foreign_drops.join("; "));
    }
    message
}

/// Map a spawn I/O error: Windows elevation-required (OS error 740)
/// becomes [`HidMaestroError::NotAdmin`]; anything else is
/// `SpawnFailed` with the OS message.
fn map_spawn_error(err: &std::io::Error) -> HidMaestroError {
    if err.raw_os_error() == Some(740) {
        HidMaestroError::NotAdmin
    } else {
        HidMaestroError::SpawnFailed(err.to_string())
    }
}

/// `install-driver` outcome strings are a closed set; leak the
/// `&str` out of the reply by mapping to a static copy.
fn outcome_static(outcome: &str) -> &'static str {
    match outcome {
        "AlreadyCurrent" => "AlreadyCurrent",
        "Installed" => "Installed",
        "Updated" => "Updated",
        _ => "Unknown",
    }
}

/// Launch the sidecar elevated via `ShellExecuteEx` (`runas` verb).
/// The sidecar then connects back over loopback TCP (see
/// [`transport::listen_loopback`]).
///
/// M4 / C-TOK-1: `SEE_MASK_NOCLOSEPROCESS` is set and the returned
/// process handle is kept by the manager, so a hung sidecar can be
/// terminated on kill. The one-time loopback token travels on the
/// command line (`--token <nonce>`, v2.0.12 — see
/// [`SidecarManager::spawn_windows_elevated`]): AIS builds a fresh
/// environment for a `runas` child, so the old env transport dropped
/// the token and the sidecar died with exit 2 before connecting. The
/// nonce is single-use random and briefly visible in the process
/// list / WMI — accepted; it is never logged (presence only).
///
/// Windows-only. The `windows` crate code here is **not** compiled in
/// the Linux sandbox — it is verified by the Windows target check
/// (`cargo check --target x86_64-pc-windows-msvc`) and the Windows CI
/// publish job / owner-PC gate (H-HID-5).
#[cfg(windows)]
fn spawn_elevated_runas(
    path: &std::path::Path,
    port: u16,
    token: &str,
) -> Result<usize, HidMaestroError> {
    use windows::Win32::Foundation::{ERROR_CANCELLED, HINSTANCE, HWND, WIN32_ERROR};
    use windows::Win32::UI::Shell::{
        SEE_MASK_FLAG_NO_UI, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };
    use windows::core::PCWSTR;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    let file = wide(&path.to_string_lossy());
    // The token is 32 lowercase hex chars (see
    // `transport::generate_token`), so it needs no quoting — it
    // contains no spaces or shell metacharacters.
    let params = wide(&format!(
        "--connect-back 127.0.0.1:{port} {} {token}",
        crate::SIDECAR_TOKEN_ARG
    ));
    let verb = wide("runas");
    // SAFETY: all three wide strings are NUL-terminated and live for
    // the whole call; `cbSize` is the struct's own size.
    // `SEE_MASK_NOCLOSEPROCESS` requests the child process handle in
    // `hProcess` (M4) — liveness is the TCP connection AND the handle:
    // the manager terminates the process on kill, so a hung sidecar
    // cannot outlive RSH.
    let mut info = SHELLEXECUTEINFOW {
        cbSize: u32::try_from(std::mem::size_of::<SHELLEXECUTEINFOW>()).unwrap_or(0),
        fMask: SEE_MASK_FLAG_NO_UI | SEE_MASK_NOCLOSEPROCESS,
        hwnd: HWND::default(),
        lpVerb: PCWSTR::from_raw(verb.as_ptr()),
        lpFile: PCWSTR::from_raw(file.as_ptr()),
        lpParameters: PCWSTR::from_raw(params.as_ptr()),
        lpDirectory: PCWSTR::null(),
        nShow: 0, // SW_HIDE — the sidecar has no window
        hInstApp: HINSTANCE::default(),
        ..Default::default()
    };
    // SAFETY: `ShellExecuteExW` with valid pointers (see above); the
    // struct is stack-allocated for the call. windows 0.62 returns
    // `Result<()>`; the Win32 error is recovered from the `Error`
    // (FACILITY_WIN32 HRESULT) via `WIN32_ERROR::from_error`.
    match unsafe { ShellExecuteExW(&raw mut info) } {
        // The handle address (not the `HANDLE` itself — it is not
        // `Send`, and the backend must stay `Send`); the manager
        // reconstructs it only to terminate + close it.
        // (2b, v2.0.12): a success with no process handle is a
        // spawn failure, not a process to wait on — fail fast with
        // `SpawnFailed` instead of a silent 30 s accept timeout.
        Ok(()) => {
            if info.hProcess.0.is_null() {
                return Err(HidMaestroError::SpawnFailed(
                    "ShellExecuteEx (runas) succeeded but returned no process handle (hProcess is NULL) — refusing to wait out the accept timeout"
                        .to_string(),
                ));
            }
            Ok(info.hProcess.0 as usize)
        }
        Err(e) => {
            let win32 = WIN32_ERROR::from_error(&e);
            if win32 == Some(ERROR_CANCELLED) {
                // ERROR_CANCELLED — the user declined the UAC prompt.
                return Err(HidMaestroError::NotAdmin);
            }
            Err(HidMaestroError::SpawnFailed(format!(
                "ShellExecuteEx (runas) failed, Win32 error {}",
                win32.map_or_else(|| e.code().0.cast_unsigned(), |w| w.0)
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A manager whose UAC prompt was declined (latch set directly —
    /// same-module test seam; the real path is Windows-only, the
    /// `runas` spawn).
    fn declined_manager() -> SidecarManager {
        let mut config = SidecarConfig::with_path(std::path::PathBuf::from(
            "/nonexistent/rsh-hidmaestro-sidecar",
        ));
        config.spawn_timeout = Duration::from_millis(200);
        config.reply_timeout = Duration::from_millis(200);
        let mut manager = SidecarManager::new(config);
        manager.elevation_declined = true;
        manager
    }

    #[test]
    fn elevation_decline_fails_fast_without_spawn() {
        let mut manager = declined_manager();
        let error = manager.connect().expect_err("declined UAC must fail fast");
        assert!(matches!(error, HidMaestroError::NotAdmin));
        assert!(manager.elevation_declined());
        // An explicit re-select clears the latch: the next connect
        // attempts a real spawn — on Linux the sidecar binary does
        // not exist, so it fails with anything BUT NotAdmin.
        manager.reset_fallback();
        assert!(!manager.elevation_declined());
        let error = manager.connect().expect_err("no sidecar binary on Linux");
        assert!(!matches!(error, HidMaestroError::NotAdmin));
    }

    /// P3 (v2.0.11): the v2.0.10 failure mode, on the TCP
    /// connect-back path — a sidecar that connects and dies before
    /// hello must make the daemon report `SidecarDied` (with the
    /// accept→EOF elapsed ms), never hang. A stub TCP client plays
    /// the dying sidecar: connect, then close without writing.
    #[test]
    fn tcp_connect_then_die_before_hello_is_sidecar_died() {
        use crate::transport::{accept_loopback, listen_loopback};
        use std::net::TcpStream;

        let (listener, port) = listen_loopback().expect("loopback listener");
        std::thread::spawn(move || {
            let _stream = TcpStream::connect(("127.0.0.1", port)).expect("connect back");
            // Dropped without writing: EOF before hello.
        });
        let transport = accept_loopback(&listener, Duration::from_secs(5), None).expect("accept");
        let mut manager = SidecarManager::new(SidecarConfig::with_path(PathBuf::from(
            "/nonexistent/rsh-hidmaestro-sidecar",
        )));
        manager.transport = Some(transport);
        manager.hello_started = Some(Instant::now());
        let err = manager
            .read_hello(None)
            .expect_err("EOF before hello must fail");
        assert!(matches!(err, HidMaestroError::SidecarDied(_)));
        let message = err.to_string();
        assert!(message.contains("exited before hello"), "{message}");
        assert!(message.contains("ms after spawn"), "{message}");
    }

    /// H4 (v2.0.8 retest Bug 1): `read_hello` preserves the
    /// DLL-hash verdict in `HelloInfo` — the daemon/UI log and
    /// Diagnostics need the measurement, not just pass/fail.
    #[test]
    fn read_hello_preserves_dll_verdict() {
        use crate::transport::TransportWriter;
        use std::collections::VecDeque;
        use std::sync::{Arc, Mutex};

        struct StubWriter;
        impl TransportWriter for StubWriter {
            fn write_line(&mut self, _line: &str) -> std::io::Result<()> {
                Ok(())
            }
            fn kill(&mut self) {}
        }

        let (tx, rx) = std::sync::mpsc::channel();
        let transport = LiveTransport {
            writer: Box::new(StubWriter),
            events: rx,
            stderr_lines: Arc::new(Mutex::new(VecDeque::new())),
        };
        let mut manager = SidecarManager::new(SidecarConfig::with_path(std::path::PathBuf::from(
            "/nonexistent/rsh-hidmaestro-sidecar",
        )));
        manager.transport = Some(transport);
        tx.send(TransportEvent::Line(
            r#"{"ok":true,"hello":"rsh-hm-sidecar","sidecar":"rsh-hm-sidecar","sdk":"2.1.0","protocol":1,"dll_sha256":"f13e32591404db0f418af9265b8e240f71257297cfae5a41dcd696fd87e9b4d5","dll_sha256_ok":true,"admin":true}"#
                .to_string(),
        ))
        .unwrap();
        let info = manager.read_hello(None).expect("good hello must parse");
        assert_eq!(info.sdk_version.as_deref(), Some("2.1.0"));
        assert_eq!(
            info.dll_sha256.as_deref(),
            Some("f13e32591404db0f418af9265b8e240f71257297cfae5a41dcd696fd87e9b4d5")
        );
        assert_eq!(info.dll_sha256_ok, Some(true));
    }

    /// (2a, v2.0.12): the accept-failure diagnostic names the spawn
    /// PID, the sidecar exit code, and the accept→deadline elapsed
    /// ms — the three facts the v2.0.11 retest was missing. Exit 2
    /// is the sidecar's own fast-exit code (bad-args).
    #[test]
    fn accept_failure_names_pid_exit_code_and_elapsed() {
        let message = format_accept_failure(
            "sidecar did not connect back in time",
            Some(4848),
            Some(2),
            32,
            &[],
        );
        assert!(
            message.contains("sidecar did not connect back in time"),
            "{message}"
        );
        assert!(message.contains("pid 4848"), "{message}");
        assert!(message.contains("exit code 0x00000002"), "{message}");
        assert!(message.contains("32 ms since spawn"), "{message}");
    }

    /// (2a, v2.0.12): exit 259 (`STILL_ACTIVE`) reads as "process
    /// still running", not as a crash code — a live-but-stuck sidecar
    /// is a different failure from a dead one.
    #[test]
    fn accept_failure_still_active_reads_as_running() {
        let message = format_accept_failure(
            "sidecar did not connect back in time",
            Some(1234),
            Some(259),
            30_000,
            &[],
        );
        assert!(message.contains("pid 1234"), "{message}");
        assert!(message.contains("process still running"), "{message}");
        assert!(!message.contains("exit code 0x00000103"), "{message}");
    }

    /// (2a, v2.0.12): an unreadable exit code reports "unknown",
    /// never a guessed code; a missing PID reports "?", never 0
    /// (0 is not a PID — it is the H4 hole).
    #[test]
    fn accept_failure_unknown_exit_code_is_not_guessed() {
        let message =
            format_accept_failure("sidecar did not connect back in time", None, None, 5, &[]);
        assert!(message.contains("pid ?"), "{message}");
        assert!(message.contains("exit code unknown"), "{message}");
        assert!(!message.contains("pid 0"), "{message}");
    }

    /// (2c, v2.0.12): foreign-peer drops ride in the diagnostic
    /// with expected-vs-actual PID — and the token is never part of
    /// the message (it is not even an argument to the formatter).
    #[test]
    fn accept_failure_carries_foreign_peer_drops() {
        let drops = vec!["loopback peer is PID 9999, not the spawned sidecar PID 4848".to_string()];
        let message = format_accept_failure(
            "sidecar did not connect back in time",
            Some(4848),
            Some(2),
            30_000,
            &drops,
        );
        assert!(
            message.contains("foreign loopback peers dropped"),
            "{message}"
        );
        assert!(message.contains("loopback peer is PID 9999"), "{message}");
        assert!(message.contains("spawned sidecar PID 4848"), "{message}");
        assert!(!message.contains("abcdef0123456789"), "{message}");
    }

    /// P1-3 (update4-fix): a sidecar death AFTER hello reports the
    /// same enrichment as a pre-hello death — the phase ("closed the
    /// connection") plus the ms since the last reply — not the bare
    /// "sidecar closed the connection" with no timing. A stub
    /// transport plays a sidecar that says hello, then dies.
    #[test]
    fn post_hello_eof_is_enriched_like_pre_hello_eof() {
        use crate::transport::TransportWriter;
        use std::collections::VecDeque;
        use std::sync::{Arc, Mutex};

        struct StubWriter;
        impl TransportWriter for StubWriter {
            fn write_line(&mut self, _line: &str) -> std::io::Result<()> {
                Ok(())
            }
            fn kill(&mut self) {}
        }

        let (tx, rx) = std::sync::mpsc::channel();
        let transport = LiveTransport {
            writer: Box::new(StubWriter),
            events: rx,
            stderr_lines: Arc::new(Mutex::new(VecDeque::new())),
        };
        let mut manager = SidecarManager::new(SidecarConfig::with_path(std::path::PathBuf::from(
            "/nonexistent/rsh-hidmaestro-sidecar",
        )));
        manager.transport = Some(transport);
        manager.hello_started = Some(Instant::now());
        tx.send(TransportEvent::Line(
            r#"{"ok":true,"hello":"rsh-hm-sidecar","sidecar":"rsh-hm-sidecar","sdk":"2.1.0","protocol":1,"dll_sha256":"f13e32591404db0f418af9265b8e240f71257297cfae5a41dcd696fd87e9b4d5","dll_sha256_ok":true,"admin":true}"#
                .to_string(),
        ))
        .unwrap();
        manager.read_hello(None).expect("good hello must parse");
        // The sidecar dies mid-session: EOF instead of a reply.
        tx.send(TransportEvent::Eof).unwrap();
        let err = manager
            .request(&Command::Ping)
            .expect_err("EOF after hello must fail");
        assert!(matches!(err, HidMaestroError::SidecarDied(_)));
        let message = err.to_string();
        assert!(message.contains("closed the connection"), "{message}");
        assert!(message.contains("ms since last reply"), "{message}");
    }
}
