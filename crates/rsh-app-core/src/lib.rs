//! `rsh-app-core`: the Tauri command handlers and the app state (`AppState`)
//! that join the engine, transports, security, and profiles crates together.
//! The web frontend only ever talks to this crate.
//!
//! A2 scope is deliberately small: only the data-directory helper and the
//! re-exported read-only C# importer. Command handlers, engine wiring, and
//! app state arrive in later phases.

use std::path::PathBuf;

/// Tauri command logic with a real backend (Phase E gate): pin-map and
/// firmware-image validation. The thin `#[tauri::command]` wrappers in
/// `src-tauri/src/main.rs` forward to these functions.
pub mod commands;

/// Snapshot API: `EngineMetrics` → frontend `EngineStats` contract,
/// loop-rate measurement, and the 30 Hz `rsh://snapshot` payload.
pub mod snapshot;

/// App state (`AppStateCore`): engine metrics + loop-rate tracker +
/// output backend + game launcher + profile store (Phase G0 / SW-R6.1
/// entry item). The Tauri shell (`src-tauri/src/app_state.rs`) wraps
/// this in the Tauri-managed `AppState` and owns the snapshot emitter
/// thread; all logic and tests live here.
pub mod state;

/// Profile draft / dirty model (LOGIC-FIX-PLAN §4.4, D-1): the
/// in-memory draft `set_tuning` edits, saved to disk only on Save.
pub mod draft;

/// Game detection + launch (Phase F2 / SW-R7.3).
pub mod launcher;

/// Board firmware upload over the board's WebOTA HTTP server
/// (H5-control `upload_firmware`): std-only HTTP client, CSRF token
/// fetch + multipart `POST /update`. The image is verified by the
/// caller (`rsh_security::verify_firmware_image`) before this module
/// ever moves a byte.
pub mod webota;

/// System integration: single instance, autostart, tray, firewall,
/// update check, shutdown-neutral (Phase F-sys / SW-R7.4, SW-R7.5, F4).
pub mod system;

/// Diagnostics export + crash-dump support (Phase F-sys / SW-R7.6).
pub mod diagnostics;

/// Zeroizing secret strings (Security L-2): Wi-Fi passwords and other
/// short-lived secrets are zeroed on drop instead of lingering in
/// heap memory.
pub mod secrets;

/// Telemetry adapter: F1 readers → F2 watcher `CarSource` (Phase F gate).
pub mod telemetry_adapter;

/// Re-export of the read-only C# v2 importer (owned by `rsh-profiles`).
pub use rsh_profiles::csharp_import;

/// Returns the RSH data directory: `%AppData%\RSH` on Windows
/// (`dirs::data_dir()` resolves to `%AppData%` there), `~/.local/share/RSH`
/// on Linux.
///
/// # Note — A3 open item, NOT decided here
/// Whether Tauri's `app_data_dir()` should replace `dirs::data_dir()` is an
/// A3-spike open item (`DATA-INPUT.md` §0/§8). A2 keeps the `dirs` crate,
/// matching the C# layout, and records the question instead of answering it.
#[must_use]
pub fn data_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|dir| dir.join("RSH"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_dir_is_named_rsh() {
        let dir = data_dir().expect("data dir resolves on this platform");
        assert_eq!(dir.file_name().and_then(|name| name.to_str()), Some("RSH"));
    }
}
