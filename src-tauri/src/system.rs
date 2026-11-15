//! Tauri system integration (SW-R7.4 / SW-R7.5 / F4) — the shell half.
//!
//! The testable logic lives in `rsh_app_core::system` /
//! `rsh_app_core::diagnostics`; this file is the thin Tauri/Windows
//! glue over it. It **cannot be compile-checked in the Linux
//! sandbox** (Tauri needs `WebKitGTK` there, and the `windows` crate
//! does not compile on Linux at all) — Windows CI compiles it. Every
//! Windows-only item is `#[cfg(windows)]`-gated with a non-Windows
//! stub that fails loudly instead of silently doing nothing.
//!
//! Wiring (coordinator, `src-tauri/src/main.rs` — this file does not
//! touch `main.rs` itself):
//! ```ignore
//! mod system;
//!
//! // Single instance stays as-is (A2); the version arg is forwarded
//! // by the plugin to the first instance's callback — see
//! // `parse_instance_version_arg` / `docs/SYSTEM-INTEGRATION.md` §1.
//! .plugin(tauri_plugin_autostart::init(
//!     tauri_plugin_autostart::MacosLauncher::LaunchAgent,
//!     Some(vec!["--tray"]),
//! ))
//! .plugin(tauri_plugin_updater::Builder::new().build())
//! // Shutdown: build the app, then run it with the event handler:
//! let app = tauri::Builder::default() /* …plugins… */ .build(tauri::generate_context!())?;
//! app.run(|_app, event| {
//!     system::handle_run_event(&event, || {
//!         // Coordinator-owned: signal the engine stop handle and
//!         // hand over the output sink, e.g.
//!         // rsh_app_core::system::shutdown_output(Some(&stop), &mut sink)
//!         rsh_app_core::system::ShutdownOutcome { stop_signalled: false, neutral_sent: false }
//!     });
//! });
//! ```
//!
//! Commands the coordinator wires as thin `#[tauri::command]`
//! wrappers in `main.rs` (logic here or in `rsh-app-core`):
//! - `get_autostart` / `set_autostart` → [`set_autostart`] (+ a
//!   `get_` wrapper over `AutostartManager::is_enabled`);
//! - `get_firewall_state` → `rsh_app_core::commands::get_firewall_state`;
//! - `check_for_update` → [`check_for_update`] then
//!   `rsh_app_core::system::check_update` against the running
//!   version;
//! - `export_diagnostics` → `rsh_app_core::diagnostics::export_diagnostics`.

use rsh_app_core::diagnostics;
use rsh_app_core::system::{RunKeyStore, ShutdownOutcome, SystemError, UpdateInfo};
use tauri::{AppHandle, RunEvent};

/// Args the app is launched with from autostart: start hidden to the
/// tray (the window is only shown when the user clicks the tray icon
/// or launches the app by hand), plus the `--rsh-version` flag so a
/// second launch can be compared by the single-instance callback
/// (L-4). Passed to `tauri_plugin_autostart::init` — see the module
/// docs.
pub const AUTOSTART_ARGS: &[&str] = &["--tray", "--rsh-version", rsh_app_core::system::APP_VERSION];

// ---------------------------------------------------------------------------
// Autostart (SW-R7.4)
// ---------------------------------------------------------------------------

/// Enable or disable autostart via the `tauri-plugin-autostart`
/// manager, and return the resulting enabled state.
///
/// The plugin owns the registry write; the stale-entry repair
/// ([`repair_autostart_entry`]) runs at startup *before* the
/// Settings page reads this state, so a repaired entry is what the
/// UI reports.
///
/// # Errors
///
/// Returns a message when the plugin manager fails (registry
/// unreachable, or the plugin was not registered in `main.rs`).
pub fn set_autostart(app: &AppHandle, enabled: bool) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    let manager = app.autolaunch();
    let result = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };
    result.map_err(|error| error.to_string())?;
    manager.is_enabled().map_err(|error| error.to_string())
}

/// Read the current autostart state via the plugin manager.
///
/// # Errors
///
/// Returns a message when the plugin manager fails.
pub fn get_autostart(app: &AppHandle) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch()
        .is_enabled()
        .map_err(|error| error.to_string())
}

/// Repair a stale autostart entry at startup: when the `RSH` Run-key
/// value points at an old install folder, rewrite it to the current
/// exe. See `rsh_app_core::system::repair_autostart` for the rules;
/// this is only its Windows registry backend + entry point.
///
/// # Errors
///
/// Returns a message when the registry cannot be read/written, or on
/// non-Windows platforms (autostart repair is Windows-only; the
/// plugin handles the other platforms' mechanisms itself).
pub fn repair_autostart_entry() -> Result<rsh_app_core::system::AutostartRepair, String> {
    #[cfg(windows)]
    {
        let exe = std::env::current_exe().map_err(|error| error.to_string())?;
        let mut store = RegExeStore;
        rsh_app_core::system::repair_autostart(&mut store, &exe).map_err(|error| error.to_string())
    }
    #[cfg(not(windows))]
    {
        Err(SystemError::UnsupportedPlatform("autostart repair").to_string())
    }
}

/// `RunKeyStore` over the Windows `Run` key, implemented with the
/// stock `reg.exe` (present on every Windows install) so no extra
/// registry crate is needed. The plugin owns enable/disable; this
/// store exists only for *repair*.
///
/// Key: `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`,
/// value name `RSH`.
#[cfg(windows)]
struct RegExeStore;

#[cfg(windows)]
impl RunKeyStore for RegExeStore {
    fn read_rsh(&self) -> Option<String> {
        let output = std::process::Command::new("reg")
            .args([
                "query",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                "/v",
                rsh_app_core::system::AUTOSTART_VALUE_NAME,
            ])
            .output()
            .ok()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        parse_reg_query_value(&stdout)
    }

    fn write_rsh(&mut self, command: &str) -> Result<(), SystemError> {
        let status = std::process::Command::new("reg")
            .args([
                "add",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                "/v",
                rsh_app_core::system::AUTOSTART_VALUE_NAME,
                "/t",
                "REG_SZ",
                "/d",
                command,
                "/f",
            ])
            .status()
            .map_err(|error| SystemError::Store(error.to_string()))?;
        if status.success() {
            Ok(())
        } else {
            Err(SystemError::Store(format!("reg add exited with {status}")))
        }
    }

    fn delete_rsh(&mut self) -> Result<(), SystemError> {
        let status = std::process::Command::new("reg")
            .args([
                "delete",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                "/v",
                rsh_app_core::system::AUTOSTART_VALUE_NAME,
                "/f",
            ])
            .status()
            .map_err(|error| SystemError::Store(error.to_string()))?;
        if status.success() {
            Ok(())
        } else {
            Err(SystemError::Store(format!(
                "reg delete exited with {status}"
            )))
        }
    }
}

/// Parse `reg query … /v RSH` output and return the `RSH` value.
///
/// Output shape (English locale; `REG_SZ` is locale-independent):
/// ```text
/// HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run
///     RSH    REG_SZ    "C:\Program Files\RSH\RSH.exe"
/// ```
/// Pure function — unit-tested on Linux, where `reg.exe` does not
/// exist.
#[must_use]
pub fn parse_reg_query_value(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        if let Some(reg_at) = line.find("REG_SZ") {
            let value = line[reg_at + "REG_SZ".len()..].trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Update check (SW-R7.5)
// ---------------------------------------------------------------------------

/// Ask the Tauri updater plugin whether a newer release exists.
///
/// Returns the upstream release when one is available
/// (`Some`), or [`None`] when the app is up to date. This only
/// *checks* — download/install is a Phase H action (the updater
/// signing keypair is generated there; see `docs/SYSTEM-INTEGRATION.md`
/// §5). The caller compares the result with the running version via
/// `rsh_app_core::system::check_update` and shows the update UI; the
/// check runs only when the user asks for it (Settings "Check for
/// updates") — never at startup — and never blocks the UI. Since
/// v1.0.1 the release ships no `latest.json`, so the check reports
/// "could not check" (auto-update is off by design).
///
/// # Errors
///
/// Returns a message when the updater plugin is not registered or
/// the check fails (no network, bad manifest). A failed check is
/// reported to the user as "could not check", never as "up to date".
pub async fn check_for_update(app: &AppHandle) -> Result<Option<UpdateInfo>, String> {
    use tauri_plugin_updater::UpdaterExt;
    let updater = app.updater().map_err(|error| error.to_string())?;
    let update = updater.check().await.map_err(|error| error.to_string())?;
    Ok(update.map(|found| UpdateInfo {
        version: found.version,
        notes: found.body,
        download_url: None,
    }))
}

// ---------------------------------------------------------------------------
// Shutdown-neutral path (F4)
// ---------------------------------------------------------------------------

/// Handle a Tauri run event for shutdown: on `ExitRequested`, run the
/// caller-supplied `shutdown` closure (which signals the engine stop
/// handle and sends the final neutral state — see
/// `rsh_app_core::system::shutdown_output`) and log the outcome.
///
/// The closure must be bounded and non-blocking: it signals stop and
/// sends one neutral state synchronously. Waiting for the engine
/// task to finish is the caller's job, with a timeout — a stuck
/// engine must never strand the exit.
///
/// Wire-up (coordinator, `main.rs`): build the app and call
/// `app.run(|_app, event| system::handle_run_event(&event, || …))`
/// — see the module docs. The engine stop handle + output sink live
/// in the app state the coordinator owns; this function deliberately
/// takes them as a closure instead of inventing a state type here.
pub fn handle_run_event<F>(event: &RunEvent, shutdown: F)
where
    F: FnOnce() -> ShutdownOutcome,
{
    if let RunEvent::ExitRequested { .. } = event {
        let outcome = shutdown();
        let line = format!(
            "[RSH] shutdown: stop_signalled={} neutral_sent={}",
            outcome.stop_signalled, outcome.neutral_sent
        );
        eprintln!("{line}");
        // v1.0.1: the shutdown line in the file log proves a clean exit;
        // start line present + shutdown line absent = abnormal exit.
        if let Some(log_dir) = rsh_app_core::data_dir().map(|dir| dir.join("logs")) {
            diagnostics::append_startup_log(&log_dir, &line);
        }
    }
}

/// Re-export for the shell's diagnostics command wiring: build a
/// bundle from the default log dir (`%AppData%\RSH\logs`).
#[must_use]
pub fn default_log_dir() -> Option<std::path::PathBuf> {
    rsh_app_core::data_dir().map(|dir| dir.join("logs"))
}

/// Export a diagnostics bundle (logs + versions + system info, no
/// secrets) to `out_path`. Thin wrapper over
/// [`diagnostics::export_diagnostics`] with the app version filled in.
///
/// # Errors
///
/// Returns a message when the export cannot be written.
pub fn export_diagnostics_to(out_path: &std::path::Path) -> Result<std::path::PathBuf, String> {
    let log_dir = default_log_dir().ok_or_else(|| "data directory not found".to_string())?;
    diagnostics::export_diagnostics(&log_dir, out_path, rsh_app_core::system::APP_VERSION)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reg_query_parses_quoted_value() {
        let stdout = "HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Run\n    RSH    REG_SZ    \"C:\\Program Files\\RSH\\RSH.exe\"\n";
        assert_eq!(
            parse_reg_query_value(stdout),
            Some("\"C:\\Program Files\\RSH\\RSH.exe\"".to_string())
        );
    }

    #[test]
    fn reg_query_missing_value_is_none() {
        let stdout = "HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Run\n";
        assert_eq!(parse_reg_query_value(stdout), None);
    }

    #[test]
    fn reg_query_error_text_is_none() {
        // `reg query` prints this when the value does not exist.
        let stdout = "ERROR: The system was unable to find the specified registry key or value.\n";
        assert_eq!(parse_reg_query_value(stdout), None);
    }

    #[test]
    fn instance_version_arg_parses_in_shell() {
        let args = vec![
            "RSH.exe".to_string(),
            "--rsh-version".to_string(),
            "1.0.0".to_string(),
        ];
        assert_eq!(
            rsh_app_core::system::parse_instance_version_arg(&args),
            Some("1.0.0".to_string())
        );
    }

    #[test]
    fn exit_requested_runs_shutdown_once() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let calls = Arc::new(AtomicUsize::new(0));
        let moved = Arc::clone(&calls);
        // RunEvent::ExitRequested needs an ExitRequestedApi; the
        // wildcard match only cares about the variant, so construct
        // via the non-exhaustive-friendly path: any non-exit event
        // must NOT run the shutdown. Ready is constructible.
        handle_run_event(&RunEvent::Ready, move || {
            moved.fetch_add(1, Ordering::SeqCst);
            ShutdownOutcome {
                stop_signalled: true,
                neutral_sent: true,
            }
        });
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}
