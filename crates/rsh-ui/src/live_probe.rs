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
use std::sync::Arc;
use std::time::Duration;

use crate::ipc_api::IpcApi;

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
#[must_use]
pub fn try_live_api_with_spawn(data_dir: Option<&Path>) -> Option<Arc<IpcApi>> {
    if let Some(api) = try_live_api(data_dir) {
        eprintln!("RSH-UI daemon already running (live data)");
        return Some(api);
    }
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
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: the daemon must never flash a console.
        cmd.creation_flags(0x0800_0000);
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
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(500));
        if let Some(api) = try_live_api(data_dir) {
            eprintln!("RSH-UI daemon answered after spawn (live data)");
            return Some(api);
        }
    }
    eprintln!("RSH-UI daemon did not answer within ~10 s of spawn");
    None
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
        let dir = std::env::temp_dir().join(format!("rsh-no-daemon-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        assert!(try_live_api(Some(&dir)).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
