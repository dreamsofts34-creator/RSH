//! Daemon liveness: try the real IPC connection (Phase 2).
//!
//! One connect + `ping` + `subscribe` against the daemon's socket
//! (`<data_dir>/rsh-daemon.sock`, the path the daemon binds). Success
//! means the UI runs on live daemon data through
//! [`crate::ipc_api::IpcApi`]; any failure means the daemon is not
//! running and the caller falls back to the simulated
//! [`crate::sim_api::SimApi`] — never an error shown to the user.

use std::path::Path;
use std::sync::Arc;

use crate::ipc_api::IpcApi;

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
    let data_dir = data_dir
        .map(std::path::PathBuf::from)
        .or_else(|| dirs::data_dir().map(|d| d.join("RSH")))?;
    let path = rsh_ipc::socket_path(&data_dir);
    let api = IpcApi::connect(&path).ok()?;
    if api.ping() {
        Some(api)
    } else {
        None
    }
}
