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
//!   `runas` launch. The token passed on the command line is echoed in
//!   `hello` and verified.
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

use crate::error::HidMaestroError;
use crate::mapping::HidMaestroCapabilities;
use crate::protocol::{
    CapabilitiesJson, Command, Reply, bytes_to_hex, encode_command, parse_reply, reply_to_error,
    validate_hello,
};
use crate::transport::{LiveTransport, TransportEvent, spawn_stdio};
#[cfg(windows)]
use crate::transport::{accept_loopback, generate_token, listen_loopback};
use crate::{PROFILE_ID, SIDECAR_EXE_NAME, SIDECAR_PATH_ENV};

/// How long to wait for the sidecar's `hello` line after spawn.
pub const DEFAULT_SPAWN_TIMEOUT: Duration = Duration::from_secs(10);
/// How long to wait for one reply (submit path included — the spike
/// measured IPC p99 276 µs at 200 Hz, so 500 ms is generous; a slower
/// reply means a hung sidecar).
pub const DEFAULT_REPLY_TIMEOUT: Duration = Duration::from_millis(500);
/// Heartbeat cadence while connected.
pub const DEFAULT_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);
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
    last_hello: Option<HelloInfo>,
    consecutive_failures: u32,
    fallback_needed: bool,
    healthy_submits: u64,
    last_reply: Option<Instant>,
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
            last_hello: None,
            consecutive_failures: 0,
            fallback_needed: false,
            healthy_submits: 0,
            last_reply: None,
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
    /// counter so the backend gets a fresh initial attempt + retry.
    pub fn reset_fallback(&mut self) {
        self.fallback_needed = false;
        self.consecutive_failures = 0;
        self.healthy_submits = 0;
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
        if self.fallback_needed {
            return Err(HidMaestroError::FallbackNeeded);
        }
        if self.transport.is_some() {
            return Ok(self.last_hello.clone().unwrap_or_default());
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
        self.read_hello(None)
    }

    /// Windows elevated spawn: loopback listener + `runas` launch +
    /// connect-back accept, then `hello` (token-checked).
    #[cfg(windows)]
    fn spawn_windows_elevated(&mut self) -> Result<HelloInfo, HidMaestroError> {
        let (listener, port) = listen_loopback().map_err(HidMaestroError::from)?;
        let token = generate_token();
        spawn_elevated_runas(&self.config.sidecar_path, port, &token)?;
        let transport =
            accept_loopback(&listener, self.config.spawn_timeout).map_err(HidMaestroError::from)?;
        self.token = Some(token.clone());
        self.transport = Some(transport);
        self.read_hello(Some(token.as_str()))
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
                })
            }
            Ok(TransportEvent::Eof) => Err(HidMaestroError::SidecarDied(
                "sidecar exited before hello".to_string(),
            )),
            Err(_) => Err(HidMaestroError::HelloTimeout),
        }
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
        let line = encode_command(cmd)?;
        let Some(transport) = self.transport.as_mut() else {
            return Err(HidMaestroError::NotConnected);
        };
        if let Err(e) = transport.writer.write_line(&line) {
            self.kill_transport();
            self.note_failure();
            return Err(HidMaestroError::SidecarDied(e.to_string()));
        }
        match transport.events.recv_timeout(self.config.reply_timeout) {
            Ok(TransportEvent::Line(line)) => {
                self.last_reply = Some(Instant::now());
                parse_reply(&line)
            }
            Ok(TransportEvent::Eof) => {
                self.kill_transport();
                self.note_failure();
                Err(HidMaestroError::SidecarDied(
                    "sidecar closed the connection".to_string(),
                ))
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
        let reply = self.request(&Command::Ping)?;
        reply_to_error(&reply)?;
        Ok(())
    }

    /// `install-driver` with the `AlreadyCurrent` fast path (the
    /// sidecar skips the install call when the driver is present).
    ///
    /// # Errors
    ///
    /// Propagates request errors plus `NotAdmin` / `DriverInstallFailed`.
    pub fn install_driver(&mut self) -> Result<InstallInfo, HidMaestroError> {
        let reply = self.request_checked(&Command::InstallDriver)?;
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
        let reply = self.request_checked(&Command::Create {
            profile: PROFILE_ID.to_string(),
        })?;
        let caps = reply.capabilities.map_or_else(
            HidMaestroCapabilities::custom_wheel,
            |c: CapabilitiesJson| {
                HidMaestroCapabilities::new(
                    c.has_axis_x,
                    c.has_axis_y,
                    c.has_axis_z,
                    c.has_axis_rx,
                    c.has_axis_slider,
                    c.button_count,
                    c.has_hat,
                )
            },
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
        let reply = self.request(&Command::SubmitRaw { report: encoded })?;
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

    /// Best-effort shutdown: ask the sidecar to exit cleanly (short
    /// timeout), then kill the transport. Never panics, never blocks
    /// long. Idempotent.
    pub fn shutdown(&mut self) {
        if self.transport.is_none() {
            return;
        }
        // Best-effort `exit` with a short fuse — the sidecar disposes
        // the device on its own. Any failure here just falls through
        // to the kill below.
        let _ = self.request(&Command::Exit);
        self.kill_transport();
        self.token = None;
        self.last_reply = None;
    }

    fn kill_transport(&mut self) {
        if let Some(mut transport) = self.transport.take() {
            transport.writer.kill();
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
/// Windows-only. The `windows` crate code here is **not** compiled in
/// the Linux sandbox — it is verified by the Windows CI publish job /
/// owner-PC gate (H-HID-5).
#[cfg(windows)]
fn spawn_elevated_runas(
    path: &std::path::Path,
    port: u16,
    token: &str,
) -> Result<(), HidMaestroError> {
    use windows::Win32::Foundation::{BOOL, GetLastError};
    use windows::Win32::UI::Shell::{SEE_MASK_FLAG_NO_UI, SHELLEXECUTEINFOW, ShellExecuteExW};
    use windows::core::PCWSTR;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    let file = wide(&path.to_string_lossy());
    let params = wide(&format!("--connect-back 127.0.0.1:{port} --token {token}"));
    let verb = wide("runas");
    // SAFETY: all three wide strings are NUL-terminated and live for
    // the whole call; `cbSize` is the struct's own size. No handle is
    // requested (`SEE_MASK_NOCLOSEPROCESS` is not set) — liveness is
    // the TCP connection, and the sidecar exits on disconnect.
    let mut info = SHELLEXECUTEINFOW {
        cbSize: u32::try_from(std::mem::size_of::<SHELLEXECUTEINFOW>()).unwrap_or(0),
        fMask: SEE_MASK_FLAG_NO_UI,
        hwnd: Default::default(),
        lpVerb: PCWSTR::from_raw(verb.as_ptr()),
        lpFile: PCWSTR::from_raw(file.as_ptr()),
        lpParameters: PCWSTR::from_raw(params.as_ptr()),
        lpDirectory: PCWSTR::null(),
        nShow: 0, // SW_HIDE — the sidecar has no window
        hInstApp: Default::default(),
        ..Default::default()
    };
    // SAFETY: `ShellExecuteExW` with valid pointers (see above); the
    // struct is stack-allocated for the call.
    let ok: BOOL = unsafe { ShellExecuteExW(&mut info) };
    if ok.as_bool() {
        Ok(())
    } else {
        let code = unsafe { GetLastError() }.0;
        if code == 1223 {
            // ERROR_CANCELLED — the user declined the UAC prompt.
            return Err(HidMaestroError::NotAdmin);
        }
        Err(HidMaestroError::SpawnFailed(format!(
            "ShellExecuteEx (runas) failed, Win32 error {code}"
        )))
    }
}
