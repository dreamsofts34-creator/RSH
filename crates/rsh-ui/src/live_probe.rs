//! Daemon liveness: try the real IPC connection (Phase 2) and
//! self-heal by spawning the daemon when it is not running (v2.0.1
//! hotfix).
//!
//! One connect + `ping` + `subscribe` against the daemon's socket
//! (`<data_dir>/rsh-daemon.sock`, the path the daemon binds). Success
//! means the UI runs on live daemon data through
//! [`crate::ipc_api::IpcApi`]; any failure means the daemon is not
//! running and the caller falls back to the simulated
//! [`crate::sim_api::SimApi`] — never an error shown to the user.
//!
//! The installer never starts/registers the daemon, so the UI no
//! longer just probes once: [`try_live_api_with_spawn`] spawns the
//! sibling `rsh-daemon` binary (next to the UI exe, no console flash
//! on Windows) and retries the probe for ~10 s before giving up.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ipc_api::IpcApi;

/// Serialises the tests that touch the daemon IPC endpoint.
///
/// On Windows the endpoint is ONE global named pipe
/// (`rsh_ipc::IPC_PIPE_NAME`) — the data dir does not isolate it,
/// so parallel tests in this binary race on it: the probe test can
/// connect to the round-trip test's server (and steal its single
/// pipe instance, failing both tests with "all pipe instances are
/// busy"). On Linux the Unix socket is per-dir, so this is only
/// belt-and-braces there. The round-trip test joins its server
/// thread before returning, so once the guard is released the pipe
/// is gone.
#[cfg(test)]
pub(crate) static TEST_IPC_PIPE_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// F3 (v2.0.3): death debounce — the health monitor only declares
/// the daemon dead after this many consecutive failed pings (~3 s
/// at its 1 Hz cadence). One failed ping is noise: a slow poll
/// cycle, or a daemon that recovers on the next tick.
pub const PING_DEBOUNCE: u32 = 3;

/// F3 (v2.0.3): a fatal `daemon-status.json` this fresh blocks
/// auto-spawn — the daemon died on purpose (`IpcBindFailed` /
/// `DaemonHung`) and spawning another one just re-runs the death
/// loop. Older than this, the fatal condition may have cleared, so
/// one spawn attempt is allowed (a fresh fatal file will block the
/// next one if it dies again).
pub const FATAL_FRESHNESS: Duration = Duration::from_secs(600);

/// F3 (v2.0.3): consecutive failed auto-spawn attempts before the
/// respawn latch trips — the UI stops auto-spawning and shows the
/// fatal state with a manual Retry (the Reconnect button) instead.
pub const MAX_FAILED_SPAWNS: u32 = 5;

/// F3 (v2.0.3): exponential backoff between auto-spawn attempts:
/// 1 s, 5 s, 30 s, then 60 s capped. `failed_spawns` is how many
/// attempts have already failed in a row.
#[must_use]
pub fn backoff_delay(failed_spawns: u32) -> Duration {
    match failed_spawns {
        0 => Duration::from_secs(1),
        1 => Duration::from_secs(5),
        2 => Duration::from_secs(30),
        _ => Duration::from_secs(60),
    }
}

/// F4 (v2.0.3): at most one daemon spawn attempt in flight
/// process-wide. The health monitor and the Reconnect job both
/// spawn through [`try_live_api_with_spawn_n`], so the guard
/// serialises them — a second attempt never spawns a second daemon
/// while one is starting up.
static SPAWN_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// RAII holder for [`SPAWN_IN_FLIGHT`]: released on drop, so a
/// spawn attempt that panics or returns early cannot wedge the
/// guard shut.
struct SpawnAttemptGuard;

impl SpawnAttemptGuard {
    fn try_acquire() -> Option<Self> {
        SPAWN_IN_FLIGHT
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .ok()
            .map(|_| Self)
    }
}

impl Drop for SpawnAttemptGuard {
    fn drop(&mut self) {
        SPAWN_IN_FLIGHT.store(false, Ordering::Release);
    }
}

/// F3 (v2.0.3): the auto-respawn budget, shared by the health
/// monitor (which spends it) and the UI thread (whose Reconnect
/// click resets it once for a single manual retry). The monitor
/// never spawns while [`SpawnBudget::auto_spawn_allowed`] is false:
/// latched after [`MAX_FAILED_SPAWNS`] consecutive failures, or
/// while a manual retry is in flight (the retry is the one attempt
/// — it must not re-arm the auto loop).
pub struct SpawnBudget {
    inner: Mutex<SpawnBudgetInner>,
}

struct SpawnBudgetInner {
    failed_spawns: u32,
    latched: bool,
    manual_retry_pending: bool,
    /// Whether the latch was set when the manual retry was armed:
    /// a failed retry re-latches only then — an ordinary Reconnect
    /// failure while auto-spawn was healthy costs the monitor
    /// nothing (its budget is simply fresh again).
    was_latched: bool,
}

impl SpawnBudget {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(SpawnBudgetInner {
                failed_spawns: 0,
                latched: false,
                manual_retry_pending: false,
                was_latched: false,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, SpawnBudgetInner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Delay before the next auto-spawn attempt, or `None` when the
    /// monitor must not spawn: budget exhausted (latched) or a
    /// manual retry in flight.
    #[must_use]
    pub fn next_delay(&self) -> Option<Duration> {
        let inner = self.lock();
        if inner.latched || inner.manual_retry_pending || inner.failed_spawns >= MAX_FAILED_SPAWNS
        {
            None
        } else {
            Some(backoff_delay(inner.failed_spawns))
        }
    }

    /// Whether the monitor may auto-spawn right now.
    #[must_use]
    pub fn auto_spawn_allowed(&self) -> bool {
        self.next_delay().is_some()
    }

    /// True once the budget is exhausted (or a failed manual retry
    /// re-latched it) — the UI latches the visible fatal state.
    #[must_use]
    pub fn is_latched(&self) -> bool {
        self.lock().latched
    }

    /// One auto-spawn attempt failed: count it, latching at
    /// [`MAX_FAILED_SPAWNS`].
    pub fn record_spawn_failure(&self) {
        let mut inner = self.lock();
        inner.failed_spawns = inner.failed_spawns.saturating_add(1);
        if inner.failed_spawns >= MAX_FAILED_SPAWNS {
            inner.latched = true;
        }
    }

    /// A live daemon answered: the budget is fresh again.
    pub fn record_success(&self) {
        let mut inner = self.lock();
        inner.failed_spawns = 0;
        inner.latched = false;
        inner.manual_retry_pending = false;
        inner.was_latched = false;
    }

    /// A Reconnect click resets the budget once: the manual retry
    /// is the single attempt, and the monitor stays out until it
    /// resolves (success clears, failure re-latches if it was
    /// latched before).
    pub fn reset_for_manual_retry(&self) {
        let mut inner = self.lock();
        inner.was_latched = inner.latched;
        inner.failed_spawns = 0;
        inner.latched = false;
        inner.manual_retry_pending = true;
    }

    /// The manual retry finished without a live daemon: re-latch
    /// only if it was latched before (otherwise the fresh budget
    /// stands and the monitor resumes). No-op unless a manual retry
    /// was pending — a late/timeout duplicate cannot latch twice.
    pub fn record_manual_failure(&self) {
        let mut inner = self.lock();
        if !inner.manual_retry_pending {
            return;
        }
        inner.manual_retry_pending = false;
        if inner.was_latched {
            inner.latched = true;
            inner.failed_spawns = MAX_FAILED_SPAWNS;
        } else {
            inner.failed_spawns = 0;
            inner.latched = false;
        }
        inner.was_latched = false;
    }
}

impl Default for SpawnBudget {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolve the RSH data dir: the explicit override wins, otherwise
/// the platform data dir + `RSH` (`dirs::data_dir().join("RSH")`).
/// `None` when neither is available.
#[must_use]
pub fn resolve_data_dir(data_dir: Option<&Path>) -> Option<PathBuf> {
    data_dir
        .map(PathBuf::from)
        .or_else(|| dirs::data_dir().map(|d| d.join("RSH")))
}

/// Try to open the live daemon API.
///
/// `data_dir` overrides the standard RSH data dir (test flag
/// `--data-dir`); `None` uses the standard dir.
///
/// Returns `Some` only when the daemon answers `ping` on its IPC
/// socket; `None` covers every failure mode (no socket, timeout,
/// version mismatch, daemon error).
#[must_use]
pub fn try_live_api(data_dir: Option<&Path>) -> Option<Arc<IpcApi>> {
    let data_dir = resolve_data_dir(data_dir)?;
    let path = rsh_ipc::socket_path(&data_dir);
    let api = IpcApi::connect(&path).ok()?;
    if api.ping() {
        Some(api)
    } else {
        None
    }
}

/// Try the live API, spawning the sibling daemon first when it is
/// not already running.
///
/// Probe once; if the daemon is not answering, spawn `rsh-daemon`
/// (sibling of the current exe — `rsh-daemon.exe` preferred on
/// Windows, `rsh-daemon` elsewhere; `CREATE_NO_WINDOW` on Windows
/// so no console flashes) with the same `--data-dir` when one was
/// given, then retry the probe up to 20 × 500 ms (~10 s). Returns
/// the live API, or `None` when the daemon never answered.
///
/// A9 (v2.0.2): this is SLOW (up to ~10 s of sleeps + probes) — it
/// must never run on the UI thread. Callers on the UI thread use
/// [`try_live_api_with_spawn_n`] on a background thread instead.
#[must_use]
pub fn try_live_api_with_spawn(data_dir: Option<&Path>) -> Option<Arc<IpcApi>> {
    try_live_api_with_spawn_n(data_dir, 20)
}

/// [`try_live_api_with_spawn`] with a bounded retry count (A9): the
/// UI's Reconnect path uses a short budget (6 retries ≈ 3 s) on its
/// background thread, so a dead daemon resolves fast; the health
/// monitor keeps the full 20-retry budget on its own thread.
///
/// F4 (v2.0.3): at most one spawn attempt runs at a time — when
/// another attempt is already in flight this probes only (no second
/// daemon is spawned); the in-flight attempt may still bring the
/// daemon up, so the probe loop still waits the full budget.
#[must_use]
pub fn try_live_api_with_spawn_n(data_dir: Option<&Path>, retries: usize) -> Option<Arc<IpcApi>> {
    if let Some(api) = try_live_api(data_dir) {
        eprintln!("RSH-UI daemon already running (live data)");
        return Some(api);
    }
    let Some(_spawn_guard) = SpawnAttemptGuard::try_acquire() else {
        eprintln!("RSH-UI daemon spawn already in flight — probing only");
        for _ in 0..retries {
            std::thread::sleep(Duration::from_millis(500));
            if let Some(api) = try_live_api(data_dir) {
                eprintln!("RSH-UI daemon answered during in-flight spawn (live data)");
                return Some(api);
            }
        }
        return None;
    };
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            eprintln!("RSH-UI cannot locate own exe to spawn daemon: {e}");
            return None;
        }
    };
    let Some(dir) = exe.parent() else {
        eprintln!("RSH-UI own exe has no parent dir; cannot spawn daemon");
        return None;
    };
    let daemon = ["rsh-daemon.exe", "rsh-daemon"]
        .iter()
        .map(|name| dir.join(name))
        .find(|p| p.is_file());
    let Some(daemon) = daemon else {
        eprintln!(
            "RSH-UI no sibling rsh-daemon found next to {}",
            exe.display()
        );
        return None;
    };
    let mut cmd = Command::new(&daemon);
    if let Some(d) = data_dir {
        cmd.arg("--data-dir").arg(d);
    }
    // F8 (v2.0.3): fully detached — the daemon must not inherit our
    // stdio (the same hang class as the NSIS `Exec` fix in b8b9fd4:
    // an inherited pipe keeps a child alive / blocks the parent's
    // exit) and on Windows must not flash a console, join our
    // process group, or die with us.
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW.
        cmd.creation_flags(0x0000_0008 | 0x0000_0200 | 0x0800_0000);
    }
    match cmd.spawn() {
        Ok(_child) => {
            // Detached on purpose: the daemon outlives the UI probe.
            // (`Child` is dropped without waiting; the daemon keeps
            // running. No `.wait()` here — that would block the UI.)
            eprintln!("RSH-UI spawned daemon: {}", daemon.display());
        }
        Err(e) => {
            eprintln!("RSH-UI failed to spawn daemon {}: {e}", daemon.display());
            return None;
        }
    }
    // The guard is held for the whole probe loop too: a second
    // attempt cannot spawn a second daemon while this one is still
    // starting up.
    for _ in 0..retries {
        std::thread::sleep(Duration::from_millis(500));
        if let Some(api) = try_live_api(data_dir) {
            eprintln!("RSH-UI daemon answered after spawn (live data)");
            return Some(api);
        }
    }
    eprintln!("RSH-UI daemon did not answer within ~10 s of spawn");
    None
}

/// Read the daemon's fatal status file (`<data_dir>/daemon-status.json`,
/// item 27 / M-2): the daemon writes it when it dies fatally
/// (`{"status": "fatal", "code": "IpcBindFailed"|"DaemonHung",
/// "detail": "...", ...}`) and clears it on a healthy start, so a
/// present fatal file IS the last known reason the daemon is down.
/// Returns `(code, detail)` only for `status == "fatal"` — any other
/// shape (missing file, healthy status, unparseable bytes) is `None`,
/// and the UI falls back to its generic "not running" message. The
/// detail is the daemon's own words, already plain — the UI shows it
/// verbatim, never a raw error.
///
/// Read at startup (probe failed), in `mark_dead`, and on Reconnect
/// failure — never on the hot path (it is a file read per call).
#[must_use]
pub fn read_daemon_status(data_dir: Option<&Path>) -> Option<(String, String)> {
    let dir = resolve_data_dir(data_dir)?;
    let text = std::fs::read_to_string(dir.join("daemon-status.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    fatal_status_of(&value)
}

/// `(code, detail)` for a parsed `daemon-status.json` value, only
/// when `status == "fatal"`.
fn fatal_status_of(value: &serde_json::Value) -> Option<(String, String)> {
    if value.get("status").and_then(serde_json::Value::as_str) != Some("fatal") {
        return None;
    }
    let code = value
        .get("code")
        .and_then(serde_json::Value::as_str)?
        .to_string();
    let detail = value
        .get("detail")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    Some((code, detail))
}

/// Like [`read_daemon_status`], but only when the fatal file is
/// fresh (F3): the daemon's `at_unix` (or the file mtime when it is
/// missing) no older than `max_age` — use [`FATAL_FRESHNESS`]. A
/// stale fatal file means the daemon died long ago and the fatal
/// condition may have cleared, so one spawn attempt is allowed (a
/// fresh fatal file will block the next one if it dies again). A
/// file with no usable timestamp counts as fresh — fail-safe, never
/// spawn into a known-fatal loop on a guess.
#[must_use]
pub fn read_daemon_status_fresh(
    data_dir: Option<&Path>,
    max_age: Duration,
) -> Option<(String, String)> {
    let dir = resolve_data_dir(data_dir)?;
    let path = dir.join("daemon-status.json");
    let text = std::fs::read_to_string(&path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let status = fatal_status_of(&value)?;
    let fresh = match value.get("at_unix").and_then(serde_json::Value::as_u64) {
        Some(0) | None => std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .map_or(true, |mtime| {
                mtime.elapsed().map_or(true, |age| age <= max_age)
            }),
        Some(at_unix) => {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            now.saturating_sub(at_unix) <= max_age.as_secs()
        }
    };
    if fresh {
        Some(status)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_data_dir_prefers_explicit_override() {
        let dir = Path::new("/tmp/rsh-test-data");
        assert_eq!(resolve_data_dir(Some(dir)), Some(PathBuf::from(dir)));
    }

    #[test]
    fn resolve_data_dir_defaults_to_platform_rsh_dir() {
        let resolved = resolve_data_dir(None);
        assert!(resolved.is_some_and(|d| d.ends_with("RSH")));
    }

    #[test]
    fn try_live_api_returns_none_when_no_daemon() {
        // Serialised with the round-trip test: on Windows both share
        // the one global named pipe (see TEST_IPC_PIPE_GUARD).
        let _guard = TEST_IPC_PIPE_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = std::env::temp_dir().join(format!("rsh-no-daemon-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        assert!(try_live_api(Some(&dir)).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_daemon_status_returns_fatal_code_and_detail() {
        let dir = std::env::temp_dir().join(format!("rsh-status-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(
            dir.join("daemon-status.json"),
            r#"{"status":"fatal","code":"IpcBindFailed","detail":"daemon IPC bind failed after 5 attempts","pid":123,"at_unix":1}"#,
        )
        .expect("status file");
        assert_eq!(
            read_daemon_status(Some(&dir)),
            Some((
                "IpcBindFailed".to_string(),
                "daemon IPC bind failed after 5 attempts".to_string()
            ))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_daemon_status_ignores_missing_and_non_fatal() {
        let dir = std::env::temp_dir().join(format!("rsh-status-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        assert_eq!(read_daemon_status(Some(&dir)), None);
        std::fs::write(
            dir.join("daemon-status.json"),
            r#"{"status":"ok","code":"","detail":""}"#,
        )
        .expect("status file");
        assert_eq!(read_daemon_status(Some(&dir)), None);
        std::fs::write(dir.join("daemon-status.json"), "not json at all").expect("status file");
        assert_eq!(read_daemon_status(Some(&dir)), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn backoff_delay_grows_then_caps_at_60s() {
        assert_eq!(backoff_delay(0), Duration::from_secs(1));
        assert_eq!(backoff_delay(1), Duration::from_secs(5));
        assert_eq!(backoff_delay(2), Duration::from_secs(30));
        assert_eq!(backoff_delay(3), Duration::from_secs(60));
        assert_eq!(backoff_delay(100), Duration::from_secs(60));
    }

    #[test]
    fn spawn_budget_latches_after_five_failures_and_resets_on_success() {
        let budget = SpawnBudget::new();
        assert!(budget.auto_spawn_allowed());
        assert_eq!(budget.next_delay(), Some(Duration::from_secs(1)));
        for _ in 0..4 {
            budget.record_spawn_failure();
            assert!(budget.auto_spawn_allowed());
        }
        assert_eq!(budget.next_delay(), Some(Duration::from_secs(60)));
        budget.record_spawn_failure();
        assert!(budget.is_latched());
        assert!(!budget.auto_spawn_allowed());
        assert_eq!(budget.next_delay(), None);
        budget.record_success();
        assert!(!budget.is_latched());
        assert!(budget.auto_spawn_allowed());
        assert_eq!(budget.next_delay(), Some(Duration::from_secs(1)));
    }

    #[test]
    fn spawn_budget_manual_retry_latches_only_when_it_was_latched() {
        // Latched + manual retry that fails: re-latched.
        let budget = SpawnBudget::new();
        for _ in 0..MAX_FAILED_SPAWNS {
            budget.record_spawn_failure();
        }
        assert!(budget.is_latched());
        budget.reset_for_manual_retry();
        assert!(!budget.is_latched());
        assert!(!budget.auto_spawn_allowed(), "monitor stays out during retry");
        budget.record_manual_failure();
        assert!(budget.is_latched());
        assert!(!budget.auto_spawn_allowed());

        // Healthy + manual retry that fails: fresh budget, no latch.
        let budget = SpawnBudget::new();
        budget.reset_for_manual_retry();
        assert!(!budget.auto_spawn_allowed(), "monitor stays out during retry");
        budget.record_manual_failure();
        assert!(!budget.is_latched());
        assert!(budget.auto_spawn_allowed());

        // A duplicate failure report (timeout + late job result)
        // cannot latch: the retry is no longer pending.
        budget.record_manual_failure();
        assert!(!budget.is_latched());
    }

    #[test]
    fn spawn_attempt_guard_is_exclusive_and_releases_on_drop() {
        let first = SpawnAttemptGuard::try_acquire();
        assert!(first.is_some());
        assert!(SpawnAttemptGuard::try_acquire().is_none());
        drop(first);
        assert!(SpawnAttemptGuard::try_acquire().is_some());
    }

    #[test]
    fn read_daemon_status_fresh_rejects_stale_fatal() {
        let dir = std::env::temp_dir().join(format!("rsh-status-fresh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        // Fresh (60 s ago) — blocks spawning.
        std::fs::write(
            dir.join("daemon-status.json"),
            format!(
                r#"{{"status":"fatal","code":"IpcBindFailed","detail":"bind failed","pid":123,"at_unix":{}}}"#,
                now - 60
            ),
        )
        .expect("status file");
        assert_eq!(
            read_daemon_status_fresh(Some(&dir), FATAL_FRESHNESS),
            Some(("IpcBindFailed".to_string(), "bind failed".to_string()))
        );
        // Stale (one hour ago) — the fatal condition may have
        // cleared, so spawning is allowed again.
        std::fs::write(
            dir.join("daemon-status.json"),
            format!(
                r#"{{"status":"fatal","code":"IpcBindFailed","detail":"bind failed","pid":123,"at_unix":{}}}"#,
                now - 3600
            ),
        )
        .expect("status file");
        assert_eq!(read_daemon_status_fresh(Some(&dir), FATAL_FRESHNESS), None);
        // No timestamp at all — fail-safe: counts as fresh.
        std::fs::write(
            dir.join("daemon-status.json"),
            r#"{"status":"fatal","code":"DaemonHung","detail":"hung"}"#,
        )
        .expect("status file");
        assert_eq!(
            read_daemon_status_fresh(Some(&dir), FATAL_FRESHNESS),
            Some(("DaemonHung".to_string(), "hung".to_string()))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
