//! RSH v1.0.0 — Tauri shell entry point (Phase G0).
//!
//! The window shows the literal text `RSH v1.0.0` (see
//! `frontend/dist/index.html`). The text is literal on purpose: the C#
//! blank-window lesson (v2.0.0–v2.0.4) was that silently-failing dynamic
//! bindings can leave a window alive but blank, so nothing in this shell
//! depends on a resource, binding, or theme brush that can fail silently.
//!
//! Commands are thin wrappers over `rsh_app_core` — all logic and tests
//! live there, because this crate cannot be compiled in the Linux
//! sandbox (Tauri needs WebKitGTK); Windows CI compiles it. Every
//! wrapper reads/writes the real `AppState` (registered with
//! `.manage()`): engine metrics + loop-rate tracker + output backend +
//! game launcher + profile store. No wrapper stubs or fakes a result.
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
//! `confirm_pairing_replacement`, `import_csharp_profiles`,
//! `import_csharp_settings`, `backup_now`,
//! `restore_backup`, `get_tuning`, `set_tuning`, `get_calibration`,
//! `save_calibration` (Phase G0) + `get_transport_state`,
//! `connect_transport`, `disconnect_transport`, `switch_transport`,
//! `open_config_folder`, `get_startup_notice` (Phase H3: thin
//! wrappers over the `AppStateCore` transport state / store repair)
//! + `begin_pairing`, `get_pairing_status`, `get_firmware_version`
//! (Phase H5: thin wrappers over the `AppStateCore` session
//! activator — the pairing status and the firmware version read the
//! activator's records, which exist only while a session is live)
//! + `set_password`, `get_config`, `stage_config`, `commit_config`,
//! `confirm_config`, `rollback_config`, `get_config_state`,
//! `get_pin_map`, `set_pin_map`, `upload_firmware`, `restart_wheel`
//! (Phase H5-control: thin wrappers over the
//! run-a-control-command-under-a-fresh-session helper in
//! `rsh_app_core::state` — every command opens its own fresh session
//! with a fresh nonce and a signed `GET_CAPS` first, then closes it
//! and zeroizes the key).
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
//! default `HIDMaestro`, fallback vJoy, reason recorded on failure),
//! and `setup` starts the engine task (idle + best-effort UDP).

mod app_state;
mod system;

use std::sync::Arc;

use app_state::AppState;
use rsh_app_core::commands::{
    BackendStatus, BackupInfo, FirewallState, FirmwareValidation, LedPattern, OutputState,
    PinAssignment, PinMapValidation, TuningSettings,
};
use rsh_app_core::snapshot::{EngineSnapshot, EngineStats};
use rsh_app_core::state::{AppSettings, AppSettingsPatch, ProfileSummary, is_allowed_board_ip};
use rsh_app_core::state::{
    AppStateCore, Calibration, GameInfo, PairingStatus, TransportKind, TransportState,
};
use rsh_app_core::system::{
    InstanceDecision, UpdateInfo, parse_instance_version_arg, resolve_instance,
};
use tauri::Manager;

/// Application version (new Rust series: v1.0.0).
const APP_VERSION: &str = "1.0.0";

/// Liveness probe for the frontend. Returns the literal string `pong`.
#[tauri::command]
fn ping() -> &'static str {
    "pong"
}

/// Returns the application version, `1.0.0`.
#[tauri::command]
fn get_version() -> &'static str {
    APP_VERSION
}

/// Validates a flat pin map against the board pin rules (Settings editor).
/// Thin wrapper over `rsh_app_core::commands::validate_pin_map` — the
/// logic and the tests live there.
#[tauri::command]
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
/// management path (`system::firewall_rule_state`) is used by the
/// startup ensure path, not by this command — the frontend contract
/// is `{ rulePresent, detail }`.
#[tauri::command]
fn get_firewall_state() -> FirewallState {
    rsh_app_core::commands::get_firewall_state()
}

/// Autostart state (via the `tauri-plugin-autostart` manager).
#[tauri::command]
fn get_autostart(app: tauri::AppHandle) -> Result<bool, String> {
    system::get_autostart(&app)
}

/// Enable or disable autostart (via the `tauri-plugin-autostart`
/// manager). Returns the resulting enabled state.
#[tauri::command]
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
/// open gap (deep-scan Round 2, L2 — see docs/SYSTEM-INTEGRATION.md §5).
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
fn get_engine_stats(state: tauri::State<'_, AppState>) -> EngineStats {
    state.core().engine_stats()
}

/// The latest 30 Hz snapshot on demand (the live feed is the
/// `rsh://snapshot` event).
#[tauri::command]
fn get_snapshot(state: tauri::State<'_, AppState>) -> EngineSnapshot {
    state.core().snapshot()
}

/* ------------------------------------------------------------------ */
/* Transport (Phase H3 — thin wrappers over AppStateCore).            */
/* ------------------------------------------------------------------ */

/// The live transport + session state for the Settings / Setup
/// screens. Thin wrapper over `AppStateCore::transport_state`.
#[tauri::command]
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
fn connect_transport(
    state: tauri::State<'_, AppState>,
    transport: String,
    address: String,
) -> Result<TransportState, String> {
    connect_transport_inner(&state, &transport, &address)
}

/// Disconnects the live transport (the engine keeps ticking idle).
#[tauri::command]
fn disconnect_transport(state: tauri::State<'_, AppState>) -> Result<TransportState, String> {
    state.core().disconnect_transport()?;
    Ok(state.core().transport_state())
}

/// Switches transport: disconnects the live one (a failure there
/// never blocks the switch — there may be nothing live), then
/// connects the requested one.
#[tauri::command]
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
fn get_startup_notice(state: tauri::State<'_, AppState>) -> Option<String> {
    state.startup_notice()
}

/* ------------------------------------------------------------------ */
/* Session + pairing (Phase H5 — thin wrappers over the activator).   */
/* ------------------------------------------------------------------ */

/// Begins pairing: triggers a session-activation attempt on the live
/// transport (the forwarder runs `BoardSession::pair` when the board
/// reports itself unpaired with the window open, `resume` otherwise)
/// and returns the pairing status. A no-op returning the current
/// status while a session is already active — it never tears down a
/// live session to re-pair. Thin wrapper over
/// `AppStateCore::begin_pairing`. The command waits up to ~3 s for
/// the attempt to produce a window or a failure (the `HELLO` arrives
/// asynchronously in the forwarder); the frontend polls
/// `get_pairing_status` while the window is open.
#[tauri::command]
fn begin_pairing(state: tauri::State<'_, AppState>) -> Result<PairingStatus, String> {
    state.core().begin_pairing()
}

/// The pairing status: seconds left in the 60 s window, the one-time
/// password while the window is open, and the last activation
/// failure, if any. Thin wrapper over `AppStateCore::pairing_status`.
#[tauri::command]
fn get_pairing_status(state: tauri::State<'_, AppState>) -> PairingStatus {
    state.core().pairing_status()
}

/// The board firmware version from the live session's `GET_CAPS`
/// reply (`"4.0.1"`). Errors honestly with no authenticated session
/// — the version comes from the board, never from a local guess.
/// Thin wrapper over `AppStateCore::firmware_version`.
#[tauri::command]
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
fn set_password(state: tauri::State<'_, AppState>, password: String) -> Result<(), String> {
    state.core().set_password(&password)
}

/// Read the board's live config blob (Buttons screen / pin map).
/// Thin wrapper over `AppStateCore::get_config`: the JSON shape
/// `stage_config` accepts (`cfgVersion`, the full `records` list,
/// and the derived `buttons` view of the three fixed `BUTTON` slot
/// roles).
#[tauri::command]
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
fn commit_config(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.core().commit_config()
}

/// `CONFIRM` the committed config (makes it permanent). Thin wrapper
/// over `AppStateCore::confirm_config`: only legal inside the 30 s
/// window after `commit_config` — an expired window errors without
/// sending `CONFIRM` (the board has already rolled back on its own;
/// run `rollback_config` to verify).
#[tauri::command]
fn confirm_config(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.core().confirm_config()
}

/// Roll back: `REVERT` + read-back verify against the last-good
/// config. Thin wrapper over `AppStateCore::rollback_config`: legal
/// from `Staged` (explicit cancel) and `Committed` (confirm timeout
/// / late confirm / expired window).
#[tauri::command]
fn rollback_config(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.core().rollback_config()
}

/// The config transaction state (`"clean"` / `"staged"` /
/// `"pending_confirm"` / `"rolled_back"`). Thin wrapper over
/// `AppStateCore::config_state`: the recorded state, with no board
/// call.
#[tauri::command]
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
fn set_pin_map(
    state: tauri::State<'_, AppState>,
    pins: Vec<PinAssignment>,
) -> Result<PinMapValidation, String> {
    state.core().set_pin_map(pins)
}

/// Upload a firmware image over the board's WebOTA server (Setup
/// wizard). Thin wrapper over `AppStateCore::upload_firmware`: the
/// real board OTA path — a fresh session reads the live version,
/// the image is verified (Ed25519 signature + format + anti-downgrade
/// vs the live version), the Wi-Fi password comes from the keystore,
/// and the image uploads over HTTP to the board's AP address. On
/// success the board restarts about a second later; the post-upload
/// version confirm is the caller's job on reconnect, via
/// `get_firmware_version`.
#[tauri::command]
fn upload_firmware(state: tauri::State<'_, AppState>, path: String) -> Result<(), String> {
    state.core().upload_firmware(std::path::Path::new(&path))
}

/// Reboot the board (Dashboard). Thin wrapper over
/// `AppStateCore::restart_wheel`: on success the session gate closes
/// and the session records clear — there is NO auto reconnect (the
/// board takes seconds to reboot and the forwarder has no
/// re-activation retry); the user reconnects once the board is back.
#[tauri::command]
fn restart_wheel(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.core().restart_wheel()
}

/// Output backend state, exactly as the backend reports it.
#[tauri::command]
fn get_output_state(state: tauri::State<'_, AppState>) -> OutputState {
    state.core().output_state()
}

/// Output enable/disable (the ONLY place the output toggle lives).
/// Actually actuates the installed backend: disabling sends neutral,
/// enabling resends the last pipeline output. The returned state
/// reports whether the backend accepted the send.
#[tauri::command]
fn set_output_state(state: tauri::State<'_, AppState>, enabled: bool) -> OutputState {
    state.core().set_output_enabled(enabled)
}

/// Selects the output backend by name (`"HIDMaestro"`, `"vJoy"`,
/// `"none"`; case-insensitive) and switches the live slot at runtime —
/// no restart. Thin wrapper over
/// `rsh_app_core::state::AppStateCore::set_output_backend`: the choice
/// is persisted in settings, the default is `HIDMaestro`, a failed
/// `HIDMaestro` connect falls back to vJoy with the reason in the
/// returned state's detail, and a failed connect leaves the previous
/// backend exactly as it was. Errors honestly on an unknown name or a
/// failed switch instead of pretending the switch happened.
#[tauri::command]
fn set_output_backend(
    state: tauri::State<'_, AppState>,
    backend: String,
) -> Result<OutputState, String> {
    state.core().set_output_backend(&backend)
}

/// Every selectable backend with honest per-backend status for the
/// Output page picker: which is selected, which is live, whether each
/// is connected, whether its driver/sidecar file was found, whether it
/// needs elevation (with the UAC note), and the vJoy install link when
/// vJoy is missing. Thin wrapper over
/// `rsh_app_core::state::AppStateCore::list_output_backends`.
#[tauri::command]
fn list_output_backends(state: tauri::State<'_, AppState>) -> Vec<BackendStatus> {
    state.core().list_output_backends()
}

/* ------------------------------------------------------------------ */
/* Profiles + settings + backups (Phase G0 — stateful, via AppState). */
/* ------------------------------------------------------------------ */

/// List-row summaries for the Profiles screen, in store order.
#[tauri::command]
fn list_profiles(state: tauri::State<'_, AppState>) -> Vec<ProfileSummary> {
    state.core().list_profiles()
}

/// The active profile's summary, if one is active.
#[tauri::command]
fn get_active_profile(state: tauri::State<'_, AppState>) -> Option<ProfileSummary> {
    state.core().active_profile()
}

/// Activates a profile (a manual choice: auto-switch locks off until
/// re-enabled) and persists the store.
#[tauri::command]
fn activate_profile(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<ProfileSummary, String> {
    state.core().activate_profile(&id)
}

/// Creates a profile with default tuning under `name` and persists.
#[tauri::command]
fn create_profile(
    state: tauri::State<'_, AppState>,
    name: String,
) -> Result<ProfileSummary, String> {
    state.core().create_profile(&name)
}

/// Duplicates a profile (new id, `"copy"` name) and persists.
#[tauri::command]
fn duplicate_profile(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<ProfileSummary, String> {
    state.core().duplicate_profile(&id)
}

/// Deletes a profile and persists.
#[tauri::command]
fn delete_profile(state: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    state.core().delete_profile(&id)
}

/// Imports a profile file (size-capped, schema-checked by the store)
/// and persists.
#[tauri::command]
fn import_profile(
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<ProfileSummary, String> {
    state.core().import_profile(&path)
}

/// Exports a profile to a file.
#[tauri::command]
fn export_profile(
    state: tauri::State<'_, AppState>,
    id: String,
    path: String,
) -> Result<(), String> {
    state.core().export_profile(&id, &path)
}

/// Current app settings.
#[tauri::command]
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
fn enable_auto_switch(state: tauri::State<'_, AppState>) -> Result<AppSettings, String> {
    state.core().enable_auto_switch()
}

/// Runs an LED test pattern on the board at a brightness (C-LED-1):
/// `pattern` is one of `off` / `solid` / `blink` / `chase` /
/// `rainbow`; `brightness` is 0–100 (default: the stored LED tuning,
/// else 100); `period_ms` is 100–5000 (default 1000). The brightness
/// scales the pattern's RGB values before the command is encoded.
#[tauri::command]
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

/// Resets the board's stored pairing key (M-6). Only works while the
/// board has armed a key reset (the HELLO flag); otherwise it refuses
/// loudly. On success the session gate closes and the session
/// records are cleared, so the board can be paired again.
#[tauri::command]
fn key_reset(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.core().key_reset()
}

/// Factory-resets the board (M-6): same session cleanup as
/// `key_reset` on success.
#[tauri::command]
fn factory_reset(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.core().factory_reset()
}

/// Confirms a pairing replacement (M-7): the board already holds a
/// key for a different PC, and the user has seen the warning
/// (board MAC in `get_pairing_status`). Re-triggers activation with
/// the replacement allowed exactly once, and returns the resulting
/// transport state.
#[tauri::command]
fn confirm_pairing_replacement(
    state: tauri::State<'_, AppState>,
) -> Result<TransportState, String> {
    state.core().confirm_pairing_replacement()
}

/// Imports profiles from a C# ApexLink/RSH export file (M-6).
/// Imported profiles are validated on load; the returned summaries
/// are the profiles that were actually added.
#[tauri::command]
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
fn import_csharp_settings(
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<AppSettings, String> {
    state.core().import_csharp_settings(&path)
}

/// Forces a backup rotation now and returns the backup list.
#[tauri::command]
fn backup_now(state: tauri::State<'_, AppState>) -> Result<Vec<BackupInfo>, String> {
    state.core().backup_now()
}

/// Restores one backup generation over the live file (M9: accepts a
/// real path — a bare generation name such as `profiles.json.bak1`
/// resolves against the store directory, an absolute path must lie
/// inside it; settings generations restore the settings too).
#[tauri::command]
fn restore_backup(state: tauri::State<'_, AppState>, path: String) -> Result<(), String> {
    state.core().restore_backup(&path)
}

/* ------------------------------------------------------------------ */
/* Games (Phase G0 — stateful, via AppState).                          */
/* ------------------------------------------------------------------ */

/// Every catalogue game with install state, bound profile, and running
/// state.
#[tauri::command]
fn list_games(state: tauri::State<'_, AppState>) -> Vec<GameInfo> {
    state.core().list_games()
}

/// Launches a game (exactly one at a time; the launcher tracks the pid).
#[tauri::command]
fn launch_game(state: tauri::State<'_, AppState>, id: String) -> Result<GameInfo, String> {
    state.core().launch_game(&id)
}

/// Stops the tracked running game (only the tracked pid).
#[tauri::command]
fn stop_game(state: tauri::State<'_, AppState>, id: String) -> Result<GameInfo, String> {
    state.core().stop_game(&id)
}

/// One game's current state (installed / running / bound profile).
#[tauri::command]
fn get_game_state(state: tauri::State<'_, AppState>, id: String) -> Result<GameInfo, String> {
    state.core().get_game_state(&id)
}

/* ------------------------------------------------------------------ */
/* Tuning + calibration (Phase G0 — stateful, via AppState).          */
/* ------------------------------------------------------------------ */

/// The tuning for one module (session tuning, else the active
/// profile's stored tuning, else the neutral default).
#[tauri::command]
fn get_tuning(state: tauri::State<'_, AppState>, module: String) -> TuningSettings {
    state.core().get_tuning(&module)
}

/// Validates and stores one module's tuning (and writes its core
/// fields through to the active profile).
#[tauri::command]
fn set_tuning(
    state: tauri::State<'_, AppState>,
    tuning: TuningSettings,
) -> Result<TuningSettings, String> {
    state.core().set_tuning(&tuning)
}

/// The active profile's captured calibration. Errors honestly when
/// there is no active profile or nothing was captured yet.
#[tauri::command]
fn get_calibration(state: tauri::State<'_, AppState>) -> Result<Calibration, String> {
    state.core().get_calibration()
}

/// Saves a captured calibration into the active profile.
#[tauri::command]
fn save_calibration(
    state: tauri::State<'_, AppState>,
    calibration: Calibration,
) -> Result<Calibration, String> {
    state.core().save_calibration(calibration)
}

/// Builds the tray icon for the shell.
///
/// The caller must keep the returned icon alive (store it in `AppState` —
/// Phase P2.3 scope): dropping the icon removes it from the tray, and the
/// window must never be hidden without a tray icon present (C# v2.0.1
/// lesson). P2.3 wires this into `setup`; until then it is an explicit
/// skeleton, not dead code by accident.
#[allow(dead_code)]
fn build_tray(app: &tauri::AppHandle) -> tauri::Result<tauri::tray::TrayIcon> {
    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| tauri::Error::Io(std::io::Error::other("no default window icon")))?;
    tauri::tray::TrayIconBuilder::with_id("main")
        .icon(icon)
        .tooltip("RSH — Real Sim Hub")
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
/// `output_backend` choice (default `HIDMaestro`) is activated
/// through the selector, with fallback (vJoy, then the `NullSink`)
/// and the reason recorded — a failed activation is logged loudly and
/// the app keeps running with output disabled, never silently on the
/// `NullSink` and never crashed at startup.
fn build_app_state() -> AppState {
    let Some(dir) = rsh_app_core::data_dir() else {
        eprintln!(
            "[RSH] cannot resolve the data directory; refusing to start without a profile store"
        );
        std::process::exit(1);
    };
    match AppStateCore::open_with_null_sink_repair(&dir) {
        (Ok(core), repair_notice) => {
            let core = Arc::new(core);
            let outcome = core.activate_startup_backend();
            match outcome.active {
                Some(active) => {
                    let mut line = format!(
                        "[RSH] startup output backend: requested {} → live {}",
                        outcome.requested.display_name(),
                        active.display_name()
                    );
                    if outcome.fell_back {
                        line.push_str(&format!(
                            " (fell back: {})",
                            outcome.reason.as_deref().unwrap_or("unknown reason")
                        ));
                    }
                    eprintln!("{line}");
                }
                None => {
                    eprintln!(
                        "[RSH] startup output backend: {} activation failed: {}; output disabled (NullSink)",
                        outcome.requested.display_name(),
                        outcome.reason.as_deref().unwrap_or("unknown reason")
                    );
                }
            }
            let state = AppState::new(core);
            if let Some(notice) = repair_notice {
                state.set_startup_notice(notice);
            }
            state
        }
        (Err(error), _) => {
            eprintln!(
                "[RSH] profile store in {} failed to open: {error}; \
                 refusing to start without profiles (backups are next to it)",
                dir.display()
            );
            std::process::exit(1);
        }
    }
}

fn main() {
    // Repair a stale autostart entry before Settings reads the state.
    // A failure is not silent any more: it joins the startup notice
    // the shell shows once (H3).
    let autostart_repair_error = system::repair_autostart_entry().err();
    if let Some(error) = &autostart_repair_error {
        eprintln!("[RSH] autostart repair failed: {error}");
    }
    let state = build_app_state();
    if let Some(error) = autostart_repair_error {
        let notice = match state.startup_notice() {
            Some(existing) => format!("{existing} Autostart repair failed: {error}."),
            None => format!("Autostart repair failed: {error}."),
        };
        state.set_startup_notice(notice);
    }
    let app = tauri::Builder::default()
        // Single instance (registered FIRST): a second launch focuses the
        // existing window instead of opening another one. L-4: the
        // second instance's argv carries `--rsh-version <a.b.c>`
        // (AUTOSTART_ARGS puts it there for autostart launches), and
        // the version is compared with `resolve_instance` — an older
        // second instance is refused, a newer one is logged loudly,
        // and the same-version case just focuses. `TakeOver` is NEVER
        // a silent kill of this instance: the plugin has already
        // exited the second instance, so exiting here would leave
        // zero instances running.
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
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
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(
                system::AUTOSTART_ARGS
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect(),
            ),
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
            confirm_pairing_replacement,
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
            begin_pairing,
            get_pairing_status,
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
        .expect("error while building RSH");
    app.run(|app, event| {
        system::handle_run_event(event, || app.state::<AppState>().shutdown());
    });
}
