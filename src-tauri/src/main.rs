// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! RSH v1.0.2 — Tauri shell entry point (Phase G0).
//!
//! The window shows the literal text `RSH v1.0.2` (see
//! `frontend/dist/index.html`). The text is literal on purpose: the C#
//! blank-window lesson (v2.0.0–v2.0.4) was that silently-failing dynamic
//! bindings can leave a window alive but blank, so nothing in this shell
//! depends on a resource, binding, or theme brush that can fail silently.
//!
//! Commands are thin wrappers over `rsh_app_core` — all logic and tests
//! live there, because this crate cannot be compiled in the Linux
//! sandbox (Tauri needs `WebKitGTK`); Windows CI compiles it. Every
//! wrapper reads/writes the real `AppState` (registered with
//! `.manage()`): engine metrics + loop-rate tracker + output backend +
//! game launcher + profile store. No wrapper stubs or fakes a result.
//! Command signatures take `AppHandle` / `State` / owned args by value because
//! the `#[tauri::command]` macro requires it, so `clippy::needless_pass_by_value` is
//! allowed on the command fns below (not a skipped check — the signatures
//! cannot take references).
//!
//! Implemented here (56): `ping`, `get_version`, `validate_pin_map`,
//! `validate_firmware_image` (Phase E) + `list_backups`,
//! `get_firewall_state`, `get_autostart`, `set_autostart`,
//! `check_for_update`, `export_diagnostics` (Phase F) +
//! `get_engine_stats`, `get_snapshot`, `get_output_state`,
//! `set_output_state`, `set_output_backend`, `list_output_backends`,
//! `list_profiles`,
//! `get_active_profile`, `activate_profile`, `create_profile`,
//! `duplicate_profile`, `delete_profile`, `import_profile`,
//! `export_profile`, `list_games`, `launch_game`, `stop_game`,
//! `get_game_state`, `get_settings`, `set_settings` (PATCH),
//! `enable_auto_switch`, `led_test`, `key_reset`, `factory_reset`,
//! `import_csharp_profiles`,
//! `import_csharp_settings`, `backup_now`,
//! `restore_backup`, `get_tuning`, `set_tuning`, `get_calibration`,
//! `save_calibration` (Phase G0) + `get_transport_state`,
//! `connect_transport`, `disconnect_transport`, `switch_transport`,
//! `open_config_folder`, `get_startup_notice` (Phase H3: thin
//! wrappers over the `AppStateCore` transport state / store repair)
//! + `get_firmware_version`
//!   (Phase H5: thin wrapper over the `AppStateCore` session
//!   activator — the firmware version reads the activator's
//!   records, which exist only while a session is live)
//! + `set_password`, `get_config`, `stage_config`, `commit_config`,
//!   `confirm_config`, `rollback_config`, `get_config_state`,
//!   `get_pin_map`, `set_pin_map`, `upload_firmware`, `restart_wheel`
//!   (Phase H5-control: thin wrappers over the
//!   run-a-control-command-under-a-fresh-session helper in
//!   `rsh_app_core::state` — every command opens its own fresh session
//!   with a fresh nonce and a signed `GET_CAPS` first, then closes it
//!   and zeroizes the key).
//!
//! Honest limits of the H5-control commands (the behaviour is what it
//! says — see the `rsh-app-core` docs for the full reasoning):
//! - Serial control refuses while the live transport holds the serial
//!   port (disconnect first, then retry); UDP control works alongside
//!   the live forwarder.
//! - Arbitrary button→action rebinding is not a board feature — the
//!   board stores only the three fixed `BUTTON` slot roles, so
//!   `stage_config` rejects a cross-role rebind loudly instead of
//!   pretending to stage it.
//! - `restart_wheel` does not auto-reconnect (the board takes seconds
//!   to reboot and the forwarder has no re-activation retry); the
//!   user reconnects once the board is back. `upload_firmware`'s
//!   post-upload version confirm is the caller's job on reconnect,
//!   via `get_firmware_version`.
//!
//! The exit path (`handle_run_event` → `AppState::shutdown`) runs the
//! real F4 shutdown: bounded engine stop (≤ ~5 s join; the engine's
//! own `finish()` sends a final neutral through the sink), then
//! neutral through the installed backend. With the `NullSink`
//! installed the neutral send is attempted and honestly reported as
//! not sent (D-F3) until Phase G2/G3 install the real backend. The
//! 30 Hz `rsh://snapshot` feed plus `rsh://button-press` events are
//! emitted by the emitter thread owned by `AppState`
//! (`src-tauri/src/app_state.rs`, loop in `rsh_app_core::state`).
//! Startup activates the persisted output backend first (Phase G:
//! `AppStateCore::activate_startup_backend` in `build_app_state` —
//! default `vJoy`, fallback Xbox (ViGEmBus), reason recorded on failure),
//! and `setup` starts the engine task (idle + best-effort UDP).

mod app_state;
mod system;

use std::sync::Arc;

use app_state::AppState;
use rsh_app_core::commands::{
    BackendStatus, BackupInfo, FirewallState, FirmwareValidation, LedPattern, OutputState,
    PinAssignment, PinMapValidation, TuningSettings,
};
use rsh_app_core::diagnostics::{append_startup_log_both, install_panic_hook, log_fatal};
use rsh_app_core::snapshot::{EngineSnapshot, EngineStats};
use rsh_app_core::state::{AppSettings, AppSettingsPatch, ProfileSummary, is_allowed_board_ip};
use rsh_app_core::state::{AppStateCore, Calibration, GameInfo, TransportKind, TransportState};
use rsh_app_core::system::{
    InstanceDecision, UpdateInfo, parse_instance_version_arg, resolve_instance,
};
use tauri::Manager;

/// Application version (new Rust series: v1.0.2).
const APP_VERSION: &str = "1.0.2";

/// The startup-log directory: `<data-dir>/logs` (`%AppData%\RSH\logs`
/// on Windows). `None` when the data directory does not resolve.
fn startup_log_dir() -> Option<std::path::PathBuf> {
    rsh_app_core::data_dir().map(|dir| dir.join("logs"))
}

/// Append one line to the daily startup log (`rsh-YYYYMMDD.log`) in
/// BOTH log locations — the data-dir logs and the `%TEMP%\RSH\logs`
/// fallback (v1.0.2: the v1.0.1 launch failure left the data-dir logs
/// empty, so a single-location log is no evidence at all). Never
/// fails startup: an unwritable directory is silently ignored
/// inside `append_startup_log_both`, and every line is flushed to
/// disk before the call returns, so the log survives a crash right
/// after the write.
fn log_startup(message: &str) {
    let _ = append_startup_log_both(message);
}

/// Install the panic hook and log process start (v1.0.2). Called first
/// in `main()`, before anything else can panic — the file log is the
/// evidence for the next run when the window never paints.
fn init_startup_logging() {
    if let Some(log_dir) = startup_log_dir() {
        install_panic_hook(log_dir);
    }
    // Dual-location on purpose (see `log_startup`): even with no
    // data dir at all (review bug D) the start line reaches the
    // %TEMP% fallback — a run with no trace anywhere must never
    // happen again.
    let _ = append_startup_log_both(&format!("RSH v{APP_VERSION} starting"));
}

// Raw `user32` binding for the fatal dialog (see `show_fatal_dialog`).
// Module level on purpose: an `extern` block after statements trips
// `clippy::items-after-statements`. Plain `//` comments here: rustdoc
// does not document `extern` blocks, so `///` would be unused.
#[cfg(windows)]
#[link(name = "user32")]
unsafe extern "system" {
    fn MessageBoxW(
        hwnd: *mut std::ffi::c_void,
        text: *const u16,
        caption: *const u16,
        utype: u32,
    ) -> i32;
}

/// Shows a native Windows error dialog (`MessageBoxW`, `MB_OK |
/// MB_ICONERROR`) so a fatal startup failure is VISIBLE even when no
/// console exists and the user never finds the logs — the v1.0.1
/// launch failure showed nothing at all. Windows-only; a no-op on
/// other platforms (where the console + logs are the evidence).
///
/// Implemented as a raw `extern "system"` call on purpose: the
/// `windows` crate features enabled for this shell do not include
/// `Win32_UI_WindowsAndMessaging`, and a last-resort dialog must not
/// depend on a new dependency. Windows CI compile is the gate for
/// this function (it cannot compile in the Linux sandbox).
#[cfg(windows)]
fn show_fatal_dialog(title: &str, message: &str) {
    use std::os::windows::ffi::OsStrExt;
    fn to_wide_nul(text: &str) -> Vec<u16> {
        std::ffi::OsStr::new(text)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }
    let text = to_wide_nul(message);
    let caption = to_wide_nul(title);
    // 0x10 = MB_ICONERROR (MB_OK is 0x0).
    unsafe {
        MessageBoxW(std::ptr::null_mut(), text.as_ptr(), caption.as_ptr(), 0x10);
    }
}

/// Non-Windows stub: no native dialog, the console + logs carry it.
#[cfg(not(windows))]
fn show_fatal_dialog(_title: &str, _message: &str) {}

/// Last-resort startup failure: write the reason to both log
/// locations ([`log_fatal`]), show it in a native dialog so the
/// failure is visible even when no console exists and the logs
/// cannot be found, then exit(1).
///
/// The dialog names the log files actually written, so the next
/// report can quote a path instead of "logs folder khaali hai" —
/// when the data dir is unwritable the `%TEMP%\RSH\logs` fallback
/// is what is shown (review bugs C/D).
fn fatal_startup(reason: &str) -> ! {
    use std::fmt::Write;
    let written = log_fatal(reason);
    let mut detail = reason.to_string();
    if written.is_empty() {
        detail.push_str(
            "\n\n(No log file could be written — neither the data directory nor the %TEMP% fallback was writable.)",
        );
    } else {
        detail.push_str("\n\nDetails were written to:");
        for path in &written {
            let _ = write!(detail, "\n{}", path.display());
        }
    }
    show_fatal_dialog("RSH — Startup Error", &detail);
    std::process::exit(1);
}

/// Liveness probe for the frontend. Returns the literal string `pong`.
#[tauri::command]
fn ping() -> &'static str {
    "pong"
}

/// Returns the application version, `1.0.2`.
#[tauri::command]
fn get_version() -> &'static str {
    APP_VERSION
}

/// Validates a flat pin map against the board pin rules (Settings editor).
/// Thin wrapper over `rsh_app_core::commands::validate_pin_map` — the
/// logic and the tests live there.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn validate_pin_map(pins: Vec<PinAssignment>) -> PinMapValidation {
    rsh_app_core::commands::validate_pin_map(&pins)
}

/// Validates a signed firmware image file before any upload (Setup
/// wizard). Thin wrapper over
/// `rsh_app_core::commands::validate_firmware_image` — the logic and the
/// tests live there. Verifies the Ed25519 signature and the image format
/// only; anti-downgrade is the caller's job (see the `rsh-app-core`
/// command docs) — the wizard compares the returned `version` against
/// the installed version from `get_firmware_version`.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn validate_firmware_image(path: String) -> FirmwareValidation {
    rsh_app_core::commands::validate_firmware_image(std::path::Path::new(&path))
}

/// Lists the 3-generation backups (profiles + settings).
/// Thin wrapper over `rsh_app_core::commands::list_backups`.
#[tauri::command]
fn list_backups() -> Vec<BackupInfo> {
    match rsh_app_core::data_dir() {
        Some(dir) => rsh_app_core::commands::list_backups(&dir),
        None => Vec::new(),
    }
}

/// Windows Firewall rule state (read-only probe; see
/// `rsh_app_core::commands::get_firewall_state`). The COM-based rule
/// `rsh_app_core::commands::get_firewall_state`). The frontend contract
/// is `{ rulePresent, detail }`.
#[tauri::command]
fn get_firewall_state() -> FirewallState {
    rsh_app_core::commands::get_firewall_state()
}

/// Autostart state (via the `tauri-plugin-autostart` manager).
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn get_autostart(app: tauri::AppHandle) -> Result<bool, String> {
    system::get_autostart(&app)
}

/// Enable or disable autostart (via the `tauri-plugin-autostart`
/// manager). Returns the resulting enabled state.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn set_autostart(app: tauri::AppHandle, enabled: bool) -> Result<bool, String> {
    system::set_autostart(&app, enabled)
}

/// Asks the Tauri updater plugin whether a newer release exists.
/// Check-only: download/install is a Windows-gate action, not a code
/// change — the minisign keypair already exists (pubkey baked into
/// `tauri.conf.json`, private key in the `RSH_UPDATER_PRIVATE_KEY`
/// secret) and the `active: false` in the config is a Tauri v1 leftover
/// the updater plugin ignores (its `Config` has no `active` field).
/// What keeps updates off is check-only app code + no `latest.json`
/// published yet; the bad-signature-rejects-update test is still an
/// open gap (deep-scan Round 2, L2).
#[tauri::command]
async fn check_for_update(app: tauri::AppHandle) -> Result<Option<UpdateInfo>, String> {
    system::check_for_update(&app).await
}

/// Exports a diagnostics bundle (logs + versions + system info, no
/// secrets) to `<data-dir>/logs/diagnostics-export.json`. Returns the
/// written path.
#[tauri::command]
fn export_diagnostics() -> Result<String, String> {
    let out_path = system::default_log_dir()
        .map(|dir| dir.join("diagnostics-export.json"))
        .ok_or_else(|| "data directory not found".to_string())?;
    system::export_diagnostics_to(&out_path).map(|path| path.to_string_lossy().into_owned())
}

/* ------------------------------------------------------------------ */
/* Engine + output (Phase G0 — stateful, via AppState).               */
/* ------------------------------------------------------------------ */

/// Current engine stats (loop rate measured, p99s `null` while
/// unmeasured — never a fake 0).
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn get_engine_stats(state: tauri::State<'_, AppState>) -> EngineStats {
    state.core().engine_stats()
}

/// The latest 30 Hz snapshot on demand (the live feed is the
/// `rsh://snapshot` event).
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn get_snapshot(state: tauri::State<'_, AppState>) -> EngineSnapshot {
    state.core().snapshot()
}

/* ------------------------------------------------------------------ */
/* Transport (Phase H3 — thin wrappers over AppStateCore).            */
/* ------------------------------------------------------------------ */

/// The live transport + session state for the Settings / Setup
/// screens. Thin wrapper over `AppStateCore::transport_state`.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn get_transport_state(state: tauri::State<'_, AppState>) -> TransportState {
    state.core().transport_state()
}

/// Persists a typed Wi-Fi address as `esp_ip` when it parses as an
/// IP literal. A hostname such as `rsh.local` is left alone — the
/// forwarder only ever dials the IP literal, so persisting a hostname
/// would silently change the board the app dials. M-1: a public
/// (non-private, non-loopback) IP literal is refused loudly instead
/// of persisted — the board only ever lives on the local network, so
/// a public address is a typo or a spoofed hint, never a real board.
fn persist_esp_ip(state: &AppState, address: &str) -> Result<(), String> {
    let Ok(ip) = address.parse::<std::net::IpAddr>() else {
        return Ok(());
    };
    let std::net::IpAddr::V4(ipv4) = ip else {
        return Err(format!(
            "refusing to persist non-IPv4 board address '{address}'"
        ));
    };
    if !is_allowed_board_ip(&ipv4) {
        return Err(format!(
            "refusing to persist public board IP '{address}' — the board \
             only ever lives on the local network"
        ));
    }
    let mut settings = state.core().settings();
    if settings.esp_ip != ip.to_string() {
        settings.esp_ip = ip.to_string();
        state.core().set_settings(settings)?;
    }
    Ok(())
}

/// Shared connect path for `connect_transport` / `switch_transport`:
/// record the request, persist a Wi-Fi IP literal, then connect.
fn connect_transport_inner(
    state: &AppState,
    transport: &str,
    address: &str,
) -> Result<TransportState, String> {
    let kind = TransportKind::parse(transport)
        .ok_or_else(|| format!("unknown transport '{transport}'"))?;
    let address = address.trim().to_string();
    if kind == TransportKind::Udp {
        persist_esp_ip(state, &address)?;
    }
    state.core().set_transport_request(kind, address);
    state.core().connect_transport(kind)?;
    Ok(state.core().transport_state())
}

/// Connects a transport to the running engine (starting it idle when
/// needed). For Wi-Fi an IP-literal address is persisted as `esp_ip`
/// first; for USB the address is only a hint — the board port is
/// auto-detected, and a missing board errors honestly.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn connect_transport(
    state: tauri::State<'_, AppState>,
    transport: String,
    address: String,
) -> Result<TransportState, String> {
    connect_transport_inner(&state, &transport, &address)
}

/// Disconnects the live transport (the engine keeps ticking idle).
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn disconnect_transport(state: tauri::State<'_, AppState>) -> Result<TransportState, String> {
    state.core().disconnect_transport()?;
    Ok(state.core().transport_state())
}

/// Switches transport: disconnects the live one (a failure there
/// never blocks the switch — there may be nothing live), then
/// connects the requested one.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn switch_transport(
    state: tauri::State<'_, AppState>,
    transport: String,
    address: String,
) -> Result<TransportState, String> {
    let _ = state.core().disconnect_transport();
    connect_transport_inner(&state, &transport, &address)
}

/// Opens the app data directory in the OS file manager (the ONLY
/// place "Open config folder" lives — UI plan golden rule 2). No
/// plugin needed: `explorer` / `open` / `xdg-open` on the data dir.
#[tauri::command]
fn open_config_folder() -> Result<(), String> {
    let dir = rsh_app_core::data_dir().ok_or_else(|| "data directory not found".to_string())?;
    #[cfg(windows)]
    let result = std::process::Command::new("explorer").arg(&dir).spawn();
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(&dir).spawn();
    #[cfg(not(any(windows, target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(&dir).spawn();
    result
        .map(|_| ())
        .map_err(|error| format!("could not open {}: {error}", dir.display()))
}

/// The startup notice (corrupt-store repair / autostart-repair
/// outcome), if startup had anything to report. The shell shows it
/// once as a dismissible banner; `None` on a clean start.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn get_startup_notice(state: tauri::State<'_, AppState>) -> Option<String> {
    state.startup_notice()
}

/* ------------------------------------------------------------------ */
/* ------------------------------------------------------------------ */
/* Session (Phase H5 — thin wrappers over the activator).           */
/* ------------------------------------------------------------------ */

/// The board firmware version from the live session's `GET_CAPS`
/// reply (`"4.0.1"`). Errors honestly with no authenticated session
/// — the version comes from the board, never from a local guess.
/// Thin wrapper over `AppStateCore::firmware_version`.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn get_firmware_version(state: tauri::State<'_, AppState>) -> Result<String, String> {
    state.core().firmware_version()
}

/* ------------------------------------------------------------------ */
/* Live-session control (Phase H5-control — thin wrappers over the   */
/* run-a-control-command-under-a-fresh-session helper). Each command  */
/* runs under its own fresh session (flag-1 HELO → fresh nonce →     */
/* signed `GET_CAPS` first, fail-closed, session closed + key        */
/* zeroized afterwards); the logic and the tests live in            */
/* `rsh_app_core::state`.                                            */
/* ------------------------------------------------------------------ */

/// Change the board AP password (Setup wizard). Thin wrapper over
/// `AppStateCore::set_password`: runs `SET_PASSWORD` under a fresh
/// session, then stores the new password as the Wi-Fi / OTA password
/// (one credential for both). On success the board restarts its AP —
/// the session gate closes and the user must rejoin the board's AP
/// with the new password (the OS Wi-Fi profile still holds the old
/// one).
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn set_password(state: tauri::State<'_, AppState>, password: String) -> Result<(), String> {
    state.core().set_password(&password)
}

/// Read the board's live config blob (Buttons screen / pin map).
/// Thin wrapper over `AppStateCore::get_config`: the JSON shape
/// `stage_config` accepts (`cfgVersion`, the full `records` list,
/// and the derived `buttons` view of the three fixed `BUTTON` slot
/// roles).
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn get_config(state: tauri::State<'_, AppState>) -> Result<serde_json::Value, String> {
    state.core().get_config()
}

/// Stage a config blob (Buttons screen save / pin map apply). Thin
/// wrapper over `AppStateCore::stage_config`: rebuilds the blob from
/// the JSON shape (records authoritative; `buttons` reconciled
/// against the three fixed `BUTTON` slot roles — a cross-role rebind
/// is rejected, see the `rsh-app-core` command docs), then stages it
/// under a fresh session. The staged blob is NOT live until
/// `commit_config` + `confirm_config`.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn stage_config(
    state: tauri::State<'_, AppState>,
    config: serde_json::Value,
) -> Result<(), String> {
    state.core().stage_config(&config)
}

/// `COMMIT` the staged config. Thin wrapper over
/// `AppStateCore::commit_config`: the board validates the staged
/// blob, live-applies it, and starts the 30 s confirm window. Only
/// legal after `stage_config`.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn commit_config(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.core().commit_config()
}

/// `CONFIRM` the committed config (makes it permanent). Thin wrapper
/// over `AppStateCore::confirm_config`: only legal inside the 30 s
/// window after `commit_config` — an expired window errors without
/// sending `CONFIRM` (the board has already rolled back on its own;
/// run `rollback_config` to verify).
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn confirm_config(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.core().confirm_config()
}

/// Roll back: `REVERT` + read-back verify against the last-good
/// config. Thin wrapper over `AppStateCore::rollback_config`: legal
/// from `Staged` (explicit cancel) and `Committed` (confirm timeout
/// / late confirm / expired window).
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn rollback_config(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.core().rollback_config()
}

/// The config transaction state (`"clean"` / `"staged"` /
/// `"pending_confirm"` / `"rolled_back"`). Thin wrapper over
/// `AppStateCore::config_state`: the recorded state, with no board
/// call.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn get_config_state(state: tauri::State<'_, AppState>) -> &'static str {
    state.core().config_state()
}

/// Read the pin map (Settings editor). Thin wrapper over
/// `AppStateCore::get_pin_map`: the single-pin functions the editor
/// can round-trip (steering, accelerator, brake, clutch, handbrake,
/// horn, led). Multi-pin records (shifter, encoders, button matrix),
/// the shifter up/down `BUTTON` slots, and battery / I²C records are
/// omitted — `set_pin_map` preserves them untouched.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn get_pin_map(state: tauri::State<'_, AppState>) -> Result<Vec<PinAssignment>, String> {
    state.core().get_pin_map()
}

/// Set the pin map (Settings editor apply). Thin wrapper over
/// `AppStateCore::set_pin_map`: the submission is the COMPLETE map
/// (the editor always submits its full draft). It is validated first
/// (an invalid map touches no board), then merged into the live blob
/// by role and staged + committed under one fresh session, so the
/// editor's 30 s confirm bar follows immediately. A merge failure is
/// a validation verdict (`valid: false` with the reason), not an
/// error — nothing is staged.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn set_pin_map(
    state: tauri::State<'_, AppState>,
    pins: Vec<PinAssignment>,
) -> Result<PinMapValidation, String> {
    state.core().set_pin_map(pins)
}

/// Upload a firmware image over the board's `WebOTA` server (Setup
/// wizard). Thin wrapper over `AppStateCore::upload_firmware`: the
/// real board OTA path — a fresh session reads the live version,
/// the image is verified (Ed25519 signature + format + anti-downgrade
/// vs the live version), the Wi-Fi password comes from the keystore,
/// and the image uploads over HTTP to the board's AP address. On
/// success the board restarts about a second later; the post-upload
/// version confirm is the caller's job on reconnect, via
/// `get_firmware_version`.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn upload_firmware(state: tauri::State<'_, AppState>, path: String) -> Result<(), String> {
    state.core().upload_firmware(std::path::Path::new(&path))
}

/// Reboot the board (Dashboard). Thin wrapper over
/// `AppStateCore::restart_wheel`: on success the session gate closes
/// and the session records clear — there is NO auto reconnect (the
/// board takes seconds to reboot and the forwarder has no
/// re-activation retry); the user reconnects once the board is back.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn restart_wheel(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.core().restart_wheel()
}

/// Output backend state, exactly as the backend reports it.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn get_output_state(state: tauri::State<'_, AppState>) -> OutputState {
    state.core().output_state()
}

/// Output enable/disable (the ONLY place the output toggle lives).
/// Actually actuates the installed backend: disabling sends neutral,
/// enabling resends the last pipeline output. The returned state
/// reports whether the backend accepted the send.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn set_output_state(state: tauri::State<'_, AppState>, enabled: bool) -> OutputState {
    state.core().set_output_enabled(enabled)
}

/// Selects the output backend by name (`"vJoy"`, `"Xbox"`,
/// `"none"`; case-insensitive) and switches the live slot at runtime —
/// no restart. Thin wrapper over
/// `rsh_app_core::state::AppStateCore::set_output_backend`: the choice
/// is persisted in settings, the default is `vJoy`, a failed
/// connect never falls back, and a failed connect leaves the previous
/// backend exactly as it was. Errors honestly on an unknown name or a
/// failed switch instead of pretending the switch happened.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn set_output_backend(
    state: tauri::State<'_, AppState>,
    backend: String,
) -> Result<OutputState, String> {
    state.core().set_output_backend(&backend)
}

/// Every selectable backend with honest per-backend status for the
/// Output page picker: which is selected, which is live, whether each
/// is connected, whether its driver file was found, and the vJoy
/// install link when vJoy is missing. Thin wrapper over
/// `rsh_app_core::state::AppStateCore::list_output_backends`.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn list_output_backends(state: tauri::State<'_, AppState>) -> Vec<BackendStatus> {
    state.core().list_output_backends()
}

/* ------------------------------------------------------------------ */
/* Profiles + settings + backups (Phase G0 — stateful, via AppState). */
/* ------------------------------------------------------------------ */

/// List-row summaries for the Profiles screen, in store order.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn list_profiles(state: tauri::State<'_, AppState>) -> Vec<ProfileSummary> {
    state.core().list_profiles()
}

/// The active profile's summary, if one is active.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn get_active_profile(state: tauri::State<'_, AppState>) -> Option<ProfileSummary> {
    state.core().active_profile()
}

/// Activates a profile (a manual choice: auto-switch locks off until
/// re-enabled) and persists the store.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn activate_profile(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<ProfileSummary, String> {
    state.core().activate_profile(&id)
}

/// Creates a profile with default tuning under `name` and persists.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn create_profile(
    state: tauri::State<'_, AppState>,
    name: String,
) -> Result<ProfileSummary, String> {
    state.core().create_profile(&name)
}

/// Duplicates a profile (new id, `"copy"` name) and persists.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn duplicate_profile(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<ProfileSummary, String> {
    state.core().duplicate_profile(&id)
}

/// Deletes a profile and persists.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn delete_profile(state: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    state.core().delete_profile(&id)
}

/// Imports a profile file (size-capped, schema-checked by the store)
/// and persists.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn import_profile(
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<ProfileSummary, String> {
    state.core().import_profile(&path)
}

/// Exports a profile to a file.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn export_profile(
    state: tauri::State<'_, AppState>,
    id: String,
    path: String,
) -> Result<(), String> {
    state.core().export_profile(&id, &path)
}

/// Current app settings.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn get_settings(state: tauri::State<'_, AppState>) -> AppSettings {
    state.core().settings()
}

/// Applies a settings PATCH and persists it (C-SET-1): only the
/// fields the frontend provides overwrite; absent fields keep their
/// stored values, so a partial save can never wipe the rest of the
/// settings. Flipping the output flag actuates the backend through
/// the same path as the toggle; flipping `auto_switch_profiles` on
/// starts the auto-switch watcher.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn set_settings(
    state: tauri::State<'_, AppState>,
    settings: AppSettingsPatch,
) -> Result<AppSettings, String> {
    state.core().set_settings_patch(settings)
}

/// Enables car-profile auto-switching (H3): clears any lock, persists
/// the setting, and starts the 2 Hz watcher thread when it is not
/// already running. Returns the updated settings.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn enable_auto_switch(state: tauri::State<'_, AppState>) -> Result<AppSettings, String> {
    state.core().enable_auto_switch()
}

/// Runs an LED test pattern on the board at a brightness (C-LED-1):
/// `pattern` is one of `off` / `solid` / `blink` / `chase` /
/// `rainbow`; `brightness` is 0–100 (default: the stored LED tuning,
/// else 100); `period_ms` is 100–5000 (default 1000). The brightness
/// scales the pattern's RGB values before the command is encoded.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn led_test(
    state: tauri::State<'_, AppState>,
    pattern: String,
    brightness: Option<u8>,
    period_ms: Option<u16>,
) -> Result<(), String> {
    let pattern = LedPattern::parse_name(&pattern)
        .ok_or_else(|| format!("unknown LED pattern '{pattern}'"))?;
    state.core().led_test(pattern, brightness, period_ms)
}

/// Forgets the Wi-Fi password on the PC side (M-6, D7): deletes
/// the stored `:wifi` password for the current board, closes the
/// session gate, and clears the session records. There is no wire
/// opcode anymore — the board-side password is cleared only by
/// holding BOOT ≥ 10 s on the board itself.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn key_reset(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.core().key_reset()
}

/// Factory-resets the board (M-6): same session cleanup as
/// `key_reset` on success.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn factory_reset(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.core().factory_reset()
}

/// Imports profiles from a C# ApexLink/RSH export file (M-6).
/// Imported profiles are validated on load; the returned summaries
/// are the profiles that were actually added.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn import_csharp_profiles(
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<Vec<ProfileSummary>, String> {
    state.core().import_csharp_profiles(&path)
}

/// Imports settings from a C# ApexLink/RSH settings file as a MERGE
/// (M-6): only the fields present in the file overwrite; the Wi-Fi
/// password is ignored (it is never stored in a C# export).
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn import_csharp_settings(
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<AppSettings, String> {
    state.core().import_csharp_settings(&path)
}

/// Forces a backup rotation now and returns the backup list.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn backup_now(state: tauri::State<'_, AppState>) -> Result<Vec<BackupInfo>, String> {
    state.core().backup_now()
}

/// Restores one backup generation over the live file (M9: accepts a
/// real path — a bare generation name such as `profiles.json.bak1`
/// resolves against the store directory, an absolute path must lie
/// inside it; settings generations restore the settings too).
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn restore_backup(state: tauri::State<'_, AppState>, path: String) -> Result<(), String> {
    state.core().restore_backup(&path)
}

/* ------------------------------------------------------------------ */
/* Games (Phase G0 — stateful, via AppState).                          */
/* ------------------------------------------------------------------ */

/// Every catalogue game with install state, bound profile, and running
/// state.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn list_games(state: tauri::State<'_, AppState>) -> Vec<GameInfo> {
    state.core().list_games()
}

/// Launches a game (exactly one at a time; the launcher tracks the pid).
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn launch_game(state: tauri::State<'_, AppState>, id: String) -> Result<GameInfo, String> {
    state.core().launch_game(&id)
}

/// Stops the tracked running game (only the tracked pid).
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn stop_game(state: tauri::State<'_, AppState>, id: String) -> Result<GameInfo, String> {
    state.core().stop_game(&id)
}

/// One game's current state (installed / running / bound profile).
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn get_game_state(state: tauri::State<'_, AppState>, id: String) -> Result<GameInfo, String> {
    state.core().get_game_state(&id)
}

/* ------------------------------------------------------------------ */
/* Tuning + calibration (Phase G0 — stateful, via AppState).          */
/* ------------------------------------------------------------------ */

/// The tuning for one module (session tuning, else the active
/// profile's stored tuning, else the neutral default).
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn get_tuning(state: tauri::State<'_, AppState>, module: String) -> TuningSettings {
    state.core().get_tuning(&module)
}

/// Validates and stores one module's tuning (and writes its core
/// fields through to the active profile).
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn set_tuning(
    state: tauri::State<'_, AppState>,
    tuning: TuningSettings,
) -> Result<TuningSettings, String> {
    state.core().set_tuning(&tuning)
}

/// The active profile's captured calibration. Errors honestly when
/// there is no active profile or nothing was captured yet.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn get_calibration(state: tauri::State<'_, AppState>) -> Result<Calibration, String> {
    state.core().get_calibration()
}

/// Saves a captured calibration into the active profile.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn save_calibration(
    state: tauri::State<'_, AppState>,
    calibration: Calibration,
) -> Result<Calibration, String> {
    state.core().save_calibration(calibration)
}

/// Builds the tray icon for the shell.
///
/// Left-clicking the icon shows the (possibly hidden/minimized)
/// window again — the way back from a `--tray` autostart launch,
/// besides launching the app a second time (see
/// `on_second_instance`).
///
/// The caller must keep the returned icon alive for as long as the
/// tray should be shown: dropping the icon removes it from the
/// tray, and the window must never be hidden without a tray icon
/// present (C# v2.0.1 lesson). The icon is `!Send`, so it cannot
/// live in managed state — `setup` leaks it for the process
/// lifetime (see the `--tray` block there).
fn build_tray(app: &tauri::AppHandle) -> tauri::Result<tauri::tray::TrayIcon> {
    use tauri::tray::{MouseButton, TrayIconEvent};
    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| tauri::Error::Io(std::io::Error::other("no default window icon")))?;
    tauri::tray::TrayIconBuilder::with_id("main")
        .icon(icon)
        .tooltip("RSH — Real Sim Hub")
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button, .. } = event
                && button == MouseButton::Left
                && let Some(window) = tray.app_handle().get_webview_window("main")
            {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        })
        .build(app)
}

/// Builds the `AppState` over the profile store in the data directory.
///
/// A corrupt store file is repaired from its 3-generation backups
/// first (H3, Phase G condition 16 — `open_with_null_sink_repair`);
/// the repair note is kept on the `AppState` as the startup notice so
/// the UI shows it once instead of silently restoring. Only when no
/// backup repairs the store does startup exit here, with a message
/// naming the file — starting without profiles would mean running
/// with no profiles at all.
///
/// Startup backend activation (Phase G) runs here too: the persisted
/// `output_backend` choice (default `vJoy`) is activated
/// through the selector, with fallback (Xbox, then the `NullSink`)
/// and the reason recorded — a failed activation is logged loudly and
/// the app keeps running with output disabled, never silently on the
/// `NullSink` and never crashed at startup.
fn build_app_state() -> AppState {
    let Some(dir) = rsh_app_core::data_dir() else {
        // Provenance verdict (v1.0.2, hypothesis #1): this silent
        // `exit(1)` is the exact match for the v1.0.1 launch
        // failure — `%APPDATA%` not resolving means no log, no
        // window, no trace. It now logs to the %TEMP% fallback and
        // shows a dialog naming the cause.
        fatal_startup(
            "cannot resolve the data directory (%APPDATA% did not resolve, so no profile store path exists); refusing to start without a profile store",
        );
    };
    // Breadcrumb (v1.0.2): the next run states what the data dir
    // resolved to, so a wrong-user / redirected %APPDATA% shows up
    // in the log instead of needing a user-side check.
    log_startup(&format!("data dir resolved: {}", dir.display()));
    match AppStateCore::open_with_null_sink_repair(&dir) {
        (Ok(core), repair_notice) => {
            let core = Arc::new(core);
            let outcome = core.activate_startup_backend();
            if let Some(active) = outcome.active {
                let mut line = format!(
                    "[RSH] startup output backend: requested {} → live {}",
                    outcome.requested.display_name(),
                    active.display_name()
                );
                if outcome.fell_back {
                    line.push_str(" (fell back: ");
                    line.push_str(outcome.reason.as_deref().unwrap_or("unknown reason"));
                    line.push(')');
                }
                eprintln!("{line}");
                log_startup(&line);
            } else {
                let reason = outcome.reason.as_deref().unwrap_or("unknown reason");
                let line = {
                    format!(
                        "[RSH] startup output backend: {} activation failed: {}; output disabled (NullSink)",
                        outcome.requested.display_name(),
                        reason
                    )
                };
                eprintln!("{line}");
                log_startup(&line);
            }
            let state = AppState::new(core);
            if let Some(notice) = repair_notice {
                state.set_startup_notice(notice);
            }
            state
        }
        (Err(error), _) => {
            fatal_startup(&format!(
                "profile store in {} failed to open: {error}; \
                 refusing to start without profiles (backups are next to it)",
                dir.display()
            ));
        }
    }
}

/// Second-instance callback (single-instance plugin): a second launch
/// focuses the existing window instead of opening another one. L-4:
/// the second instance's argv carries `--rsh-version <a.b.c>`
/// (`AUTOSTART_ARGS` puts it there for autostart launches), and the
/// version is compared with `resolve_instance` — an older second
/// instance is refused, a newer one is logged loudly, and the
/// same-version case just focuses. `TakeOver` is NEVER a silent kill
/// of this instance: the plugin has already exited the second
/// instance, so exiting here would leave zero instances running.
#[allow(clippy::needless_pass_by_value)]
fn on_second_instance(app: &tauri::AppHandle, args: Vec<String>, _cwd: String) {
    let second_version = parse_instance_version_arg(&args);
    match second_version.as_deref() {
        Some(version) => match resolve_instance(Some(APP_VERSION), version) {
            InstanceDecision::TakeOver => {
                eprintln!(
                    "[RSH] a newer instance (v{version}) tried to launch; \
                     single-instance handover is unsupported — keeping this \
                     instance (v{APP_VERSION}) focused"
                );
            }
            InstanceDecision::YieldFailClosed => {
                eprintln!(
                    "[RSH] an older or unparseable instance (v{version}) tried \
                     to launch; refused, keeping this instance (v{APP_VERSION}) focused"
                );
            }
            InstanceDecision::FocusExisting => {}
        },
        None => {
            eprintln!(
                "[RSH] a second instance launched without --rsh-version; \
                 focusing this instance (v{APP_VERSION})"
            );
        }
    }
    if let Some(window) = app.get_webview_window("main") {
        // v1.0.2 (review bug 3): a minimized window ignores
        // `set_focus` — the second launch appeared to do nothing
        // when the first instance was minimized. Unminimize + show
        // first, then focus.
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Folds an autostart-repair failure into the startup notice (H3):
/// a failure is not silent any more, the shell shows it once.
fn fold_autostart_error(state: &AppState, error: Option<String>) {
    if let Some(error) = error {
        let notice = match state.startup_notice() {
            Some(existing) => format!("{existing} Autostart repair failed: {error}."),
            None => format!("Autostart repair failed: {error}."),
        };
        state.set_startup_notice(notice);
    }
}

/// `setup` for the Tauri shell — extracted from `main()` so `main()`
/// stays under clippy's 100-line limit.
///
/// Bug 5 (v1.0.2): `--tray` (see `AUTOSTART_ARGS`) used to be parsed
/// by nobody — autostart launches showed the window anyway and
/// `build_tray` was dead code. An autostart launch now builds the
/// tray icon and starts hidden; a left-click on the icon (or a second
/// launch — see `on_second_instance`) shows the window again. The
/// tray build MUST succeed before the window is hidden: a failed tray
/// with a hidden window would leave the user with no way to reach the
/// app at all.
fn setup_shell(app: &mut tauri::App) {
    if std::env::args().any(|arg| arg == "--tray") {
        match build_tray(app.handle()) {
            Ok(icon) => {
                // `TrayIcon` is reference-counted and removed
                // from the tray when dropped — and it is
                // `!Send` (its inner handle is `Rc`), so it
                // cannot live in managed state. It is
                // intentionally leaked here: its lifetime IS
                // the process lifetime.
                std::mem::forget(icon);
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.hide();
                }
                log_startup("--tray launch: window hidden, tray icon active");
            }
            Err(error) => {
                let line = format!(
                    "[RSH] --tray launch: tray icon failed to build: {error}; starting with the window visible"
                );
                eprintln!("{line}");
                log_startup(&line);
            }
        }
    }
    let state = app.state::<AppState>();
    state.start_snapshot_emitter(app.handle());
    // Phase G gate wiring: start the engine task (idle + the
    // best-effort UDP connect). The UDP connect runs the H5
    // session activator on the forwarder: flag-1 HELO →
    // `HELLO` → `pair`/`resume` with a signed `GET_CAPS`
    // first → the session gate opens only on success. A
    // failure here never fails startup — `start_engine` logs
    // it and the app keeps running without wheel input.
    state.start_engine();
    // C1: when auto-switch is persisted on, start the 2 Hz
    // car-profile watcher thread (no-op when already running).
    if state.core().settings().auto_switch_profiles {
        state.core().start_auto_switch();
    }
    // v1.0.2: this line proves startup reached the end of setup.
    log_startup("setup complete");
}

fn main() {
    // v1.0.2: panic hook + process-start log first, before anything
    // else can panic (see `init_startup_logging`).
    init_startup_logging();
    // Repair a stale autostart entry before Settings reads the state.
    // A failure is not silent any more: it joins the startup notice
    // the shell shows once (H3).
    let autostart_repair_error = system::repair_autostart_entry().err();
    if let Some(error) = &autostart_repair_error {
        eprintln!("[RSH] autostart repair failed: {error}");
    }
    let state = build_app_state();
    log_startup("app state built");
    fold_autostart_error(&state, autostart_repair_error);
    log_startup("building tauri app");
    let app = match tauri::Builder::default()
        // Single instance (registered FIRST): see `on_second_instance`.
        .plugin(tauri_plugin_single_instance::init(on_second_instance))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            // `init` takes `Option<Vec<&'static str>>` (CI Windows,
            // 2026-11-14: E0277 `Vec<&str>` cannot be built from
            // `String`s) — `AUTOSTART_ARGS` already is `&[&str]`, so
            // copy it; mapping through `to_string` built `Vec<String>`.
            Some(system::AUTOSTART_ARGS.to_vec()),
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
        // M13 (deep-scan Round 2): native file picker for the Setup
        // firmware step (`@tauri-apps/plugin-dialog` 2.8.4). Without
        // this registration the frontend's `open()` rejects and the
        // step falls back to the typed path. Capability:
        // `dialog:default` in `capabilities/default.json` — the
        // frontend only ever calls `open()` (single `.bin` file).
        .plugin(tauri_plugin_dialog::init())
        // The stateful core: engine metrics + output backend + game
        // launcher + profile store. Registered before `setup` starts
        // the snapshot emitter thread.
        .manage(state)
        .setup(|app| {
            setup_shell(app);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ping,
            get_version,
            validate_pin_map,
            validate_firmware_image,
            list_backups,
            get_firewall_state,
            get_autostart,
            set_autostart,
            check_for_update,
            export_diagnostics,
            get_engine_stats,
            get_snapshot,
            get_output_state,
            set_output_state,
            set_output_backend,
            list_output_backends,
            list_profiles,
            get_active_profile,
            activate_profile,
            create_profile,
            duplicate_profile,
            delete_profile,
            import_profile,
            export_profile,
            get_settings,
            set_settings,
            enable_auto_switch,
            led_test,
            key_reset,
            factory_reset,
            import_csharp_profiles,
            import_csharp_settings,
            backup_now,
            restore_backup,
            list_games,
            launch_game,
            stop_game,
            get_game_state,
            get_tuning,
            set_tuning,
            get_calibration,
            save_calibration,
            get_transport_state,
            connect_transport,
            disconnect_transport,
            switch_transport,
            open_config_folder,
            get_startup_notice,
            get_firmware_version,
            set_password,
            get_config,
            stage_config,
            commit_config,
            confirm_config,
            rollback_config,
            get_config_state,
            get_pin_map,
            set_pin_map,
            upload_firmware,
            restart_wheel
        ])
        .build(tauri::generate_context!())
    {
        Ok(app) => app,
        // v1.0.2 (review bug 2): `.expect()` panicked here — a
        // panic before the event loop with no window and (on a GUI
        // subsystem binary) no console. A build failure now logs to
        // both locations and shows a dialog naming the cause.
        Err(error) => fatal_startup(&format!("could not build the RSH app window: {error}")),
    };
    log_startup("tauri app built");
    run_app(app);
}

/// Runs the Tauri event loop — extracted from `main()` so `main()`
/// stays under clippy's 100-line limit. The `Ready` breadcrumb
/// separates "died during startup" from "died after the loop started".
fn run_app(app: tauri::App) {
    app.run(|app, event| {
        if matches!(event, tauri::RunEvent::Ready) {
            // The event loop is live: if the log ends here, the
            // process died after the loop started (not during
            // startup) — the breadcrumb separates the two.
            log_startup("event loop ready");
        }
        system::handle_run_event(&event, || app.state::<AppState>().shutdown());
    });
}
