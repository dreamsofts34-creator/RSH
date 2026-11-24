//! Typed daemon API (LOGIC-FIX-PLAN §2): `AppApi` (commands) +
//! `AppEvent` (push events).
//!
//! This replaces the Tauri events / `drain_app_events` queue for
//! daemon ↔ UI communication. The Slint UI (Phase 3) talks to the
//! daemon over the IPC server (`rsh-ipc`), which serialises exactly
//! these types as JSON lines; the tray and the daemon itself call
//! the trait directly. Everything is plain data — no transport,
//! engine, or UI type leaks in.

use serde::{Deserialize, Serialize};

use crate::connection::ConnectionState;
use crate::errors::{ConnectionError, UserError};
use crate::tuning::{DraftStatus, TuningSettings};

/// Single source of truth for the app version: the Cargo workspace
/// version (`Cargo.toml` `[workspace.package] version`). The daemon,
/// the Settings screen, the README badge, and the installer must all
/// read this value — never a second hard-coded copy (A-18).
#[must_use]
pub fn app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Transport kind, as the API reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ApiTransport {
    /// Wi-Fi UDP.
    Wifi,
    /// Wired USB-serial.
    Usb,
}

impl ApiTransport {
    /// Short id (`"wifi"` / `"usb"`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Wifi => "wifi",
            Self::Usb => "usb",
        }
    }
}

/// Live connection, as the API reports it. "Connected" is only ever
/// true in `Active` (C-02).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ConnectionInfo {
    /// Current connection state.
    pub state: ConnectionState,
    /// UI label: "Connected" only when `Active`.
    pub label: String,
    /// Transport in use.
    pub transport: ApiTransport,
    /// Board address (IP or `rsh.local` resolution result).
    pub address: String,
    /// True only once an unsigned `GET_CAPS` passed (SEC-E-14).
    pub session_active: bool,
    /// Sticky "board reachable" latch (D15): set on every HELLO
    /// observed, cleared on disconnect / transport switch /
    /// `NoRoute` / HELLO-timeout. The wizard Step 1 Next unlocks
    /// on this (HELLO-or-better), never on `Active` alone.
    #[serde(default)]
    pub board_reachable: bool,
    /// Typed failure reason, when in `Error`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ConnectionError>,
    /// One-line hint for the error, when in `Error`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// Wi-Fi status, as the API reports it (no-pairing redesign):
/// which board hotspot the PC is on, whether the board is
/// connected, and whether the user has set a Wi-Fi password.
/// Never carries a password.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct WifiStatus {
    /// Board hotspot SSID (`RSH-XXXX`, D4), when a board HELLO
    /// has been observed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssid: Option<String>,
    /// True when the PC is connected to the board's network
    /// (Wi-Fi joined / USB plugged in) and the board answered.
    pub connected: bool,
    /// True when the user has set a Wi-Fi password for this
    /// board (stored in the OS credential store + WLAN profile).
    pub password_set: bool,
    /// Board MAC (`"AA:BB:CC:DD:EE:FF"`) of the last HELLO, when
    /// one was observed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board_mac: Option<String>,
}

/// A live snapshot value (steering / pedals), pushed at ~30 Hz only
/// while a UI is attached (no polling).
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct SnapshotInfo {
    /// Steering angle in degrees.
    pub steering_deg: f64,
    /// Throttle 0.0–1.0.
    pub throttle: f64,
    /// Brake 0.0–1.0.
    pub brake: f64,
    /// Clutch 0.0–1.0, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clutch: Option<f64>,
    /// Live button bitmask (H10, v2.0.2): bit `n - 1` is wheel
    /// button `n` (buttons 1–64, combined low + extended mask).
    /// `0` = no button pressed (or no live data).
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub button_mask: u64,
    /// Handbrake position 0.0–1.0 (M-3, v2.0.2): the pipeline's
    /// handbrake axis. `0.0` = released (or no live data).
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub handbrake: f64,
    /// Decoded H-pattern gear (M-3, v2.0.2): `0` = neutral, `1..=6`
    /// = gear. `0` in sequential mode or with no live data.
    #[serde(default)]
    pub gear: u8,
    /// Monotonic engine snapshot sequence (L-4, v2.0.2): the `seq`
    /// of the engine tick this snapshot was built from (`0` = no
    /// tick yet). A [`PollState`] carries this snapshot, so the UI
    /// can tell whether the snapshot changed since the last poll —
    /// and a snapshot read in the same `poll` as its connection
    /// cannot be from a different instant than that connection.
    #[serde(default)]
    pub seq: u64,
}

/// `skip_serializing_if` for `button_mask`: serde requires the
/// `&u64` signature (the reference is part of serde's API), so the
/// trivially-copy lint is silenced here on purpose.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero_u64(v: &u64) -> bool {
    *v == 0
}

/// `skip_serializing_if` for `handbrake`: same serde signature rule
/// as [`is_zero_u64`].
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero_f64(v: &f64) -> bool {
    *v == 0.0
}

/// One combined live-state read (L-4, v2.0.2): the snapshot, the
/// connection, and the "Send to game" flag in a single call.
///
/// The UI poller used to make three sequential IPC round-trips per
/// cycle (snapshot + connection + output) — on a slow daemon the
/// cache could go ~1.5 s stale and the three values could come from
/// three different instants (tearing: e.g. a snapshot from while
/// connected paired with a connection that has since flipped to
/// `Error`). One `poll` is one round-trip and one server-side
/// dispatch, so the three values are from the same instant. This is
/// the standard aggregated-state-endpoint pattern (REST / gRPC /
/// telemetry loops): 1 RTT instead of 3, no concurrency / ordering
/// / race between parallel clients, less overhead.
///
/// Wire shape is `camelCase`: `{"snapshot": {...}, "connection":
/// {...}, "outputEnabled": true}`.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PollState {
    /// Latest live snapshot (steering / pedals / buttons / gear).
    pub snapshot: SnapshotInfo,
    /// Current connection state.
    pub connection: ConnectionInfo,
    /// Whether "Send to game" output is enabled.
    pub output_enabled: bool,
}

/// One combined slow-state read (DS-29, v2.0.3): everything the
/// UI's ~1 Hz slow poller used to fetch in 30 sequential IPC
/// round-trips (18 named calls + 8 `get_tuning` + 4
/// `module_enabled` + `draft_status`), in a single call — the
/// L-4 pattern that `poll()` already uses for the fast path.
/// The values are read back-to-back in this one call, so they
/// cannot drift the way 30 separate calls can; tearing across
/// the slow (1 Hz) values is acceptable and documented. Fallible
/// sources (`diag_stats`, `calibration`, `firmware_check`,
/// `vjoy_status`) surface as `None` when the daemon reports an
/// error — exactly the "keep the old slot value" behaviour the
/// UI had per call.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlowPollState {
    /// Daemon app version.
    pub version: String,
    /// Board firmware version (or `"Unknown"` when no live session).
    pub firmware_version: String,
    /// Active output backend name.
    pub backend: String,
    /// Whether the app starts with Windows.
    pub autostart: bool,
    /// Board address (Wi-Fi IP, or `""` for USB).
    pub board_address: String,
    /// Engine diagnostics, when the daemon can report them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diag_stats: Option<DiagStats>,
    /// Current Wi-Fi state (SSID / connected / password set).
    pub wifi_status: WifiStatus,
    /// Calibration, when a board session is live.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calibration: Option<CalibrationInfo>,
    /// Latest raw ADC readings in mV, when the daemon can report
    /// them (R2-16: the Advanced-mode live readouts — steering /
    /// pedal raw values at the ~1 Hz slow rate). `None` = no live
    /// frame yet; the UI shows "—", never a fake 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_mv: Option<RawMv>,
    /// Stored profiles.
    pub profiles: Vec<ProfileSummary>,
    /// Catalogue games with install / running state.
    pub games: Vec<GameSummary>,
    /// Firmware update check, when the daemon can report it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firmware_check: Option<FirmwareCheck>,
    /// Background firmware-flash progress.
    pub firmware_update_status: FirmwareUpdateStatus,
    /// vJoy driver status, when the daemon can report it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vjoy_status: Option<VjoyStatus>,
    /// Whether auto-profile switching is paused.
    pub auto_pause: bool,
    /// Whether closing the window keeps RSH in the tray.
    pub close_to_tray: bool,
    /// Button bindings.
    pub bindings: Vec<ButtonBinding>,
    /// Knob actions: `[knob][row]` action names.
    pub knobs: [[String; 3]; 2],
    /// Draft status, when an unsaved draft exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<DraftStatus>,
    /// Per-module tunings, keyed by module name.
    pub tunings: std::collections::BTreeMap<String, TuningSettings>,
    /// Module on/off for the four toggleable modules
    /// (`handbrake`, `shifter`, `horn`, `led`).
    pub modules: std::collections::BTreeMap<String, bool>,
}

/// Firmware update check (H3, v2.0.2): the board's live version
/// against the bundled OTA image's version (parsed from the signed
/// image's `Version` marker — no signature check here; the OTA flow
/// verifies the signature before flashing).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FirmwareCheck {
    /// Board firmware version, or `"Unknown"` when no live session.
    pub board_version: String,
    /// Version parsed from the bundled OTA image, when one is present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundled_version: Option<String>,
    /// True when a bundled image exists and is newer than the board.
    pub update_available: bool,
}

/// Status of a firmware flash started by [`AppApi::update_firmware`]
/// (H-4, v2.0.2). The flash runs on a daemon background thread (it
/// takes minutes: upload + board restart + confirm), so the UI starts
/// it once and polls [`AppApi::firmware_update_status`] for progress —
/// the IPC call itself never blocks on the flash.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FirmwareUpdateStatus {
    /// True while the flash thread is running.
    pub running: bool,
    /// Current stage (`"validating"` / `"uploading"` /
    /// `"waitingForRestart"` / `"confirming"`), while running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    /// Upload percent (0..100) while uploading, `None` otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percent: Option<u8>,
    /// True once the flash finished (success or failure — see
    /// `success` / `error`). False while running or never started.
    pub done: bool,
    /// True only when the board confirmed the new version.
    pub success: bool,
    /// The confirmed new firmware version, on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The failure reason, on a finished failure (plain words).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A profile list-row, as the API reports it (v2.0.1 hotfix).
/// Mirrors the store's `ProfileSummary` (`rsh-profiles`) and the
/// core's `GameInfo` shape: `rsh-api` cannot depend on either crate,
/// so this is the wire copy — same fields, same `camelCase` JSON.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSummary {
    /// Stable profile id.
    pub id: String,
    /// Profile name.
    pub name: String,
    /// Game slug (`"assetto_corsa"`, …), or `None` for a game-less
    /// profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game: Option<String>,
    /// True when this is the active profile.
    pub active: bool,
}

/// A catalogue game, as the API reports it (v2.0.1 hotfix). Mirrors
/// the core's `GameInfo` (`rsh-app-core`): same fields, same
/// `camelCase` JSON.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameSummary {
    /// Game id (e.g. `"assetto_corsa"`).
    pub id: String,
    /// Display name.
    pub name: String,
    /// True when this game is the currently tracked running game.
    pub running: bool,
    /// True when the game exe was found on disk.
    pub installed: bool,
    /// Id of the profile bound to this game, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound_profile_id: Option<String>,
}

/// The active profile's captured calibration, as the API reports it
/// (v2.0.1 hotfix BACKEND-2). Mirrors the core's `Calibration`
/// (`rsh-app-core`): steering centre in mV, pedal min/max in mV
/// keyed by pedal (`"accel"`, `"brake"`, `"clutch"`), and when it
/// was captured — same fields, same `camelCase` JSON. `rsh-api`
/// cannot depend on `rsh-app-core`, so this is the wire copy.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalibrationInfo {
    /// Steering centre in mV.
    pub steering_center_mv: f64,
    /// Pedal minimums in mV, keyed by pedal.
    pub pedal_min_mv: std::collections::BTreeMap<String, f64>,
    /// Pedal maximums in mV, keyed by pedal.
    pub pedal_max_mv: std::collections::BTreeMap<String, f64>,
    /// When the calibration was captured (RFC 3339), if any capture
    /// was ever saved. [`None`] = never calibrated — the UI must show
    /// "not yet", never a fake date.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calibrated_at: Option<String>,
}

/// Latest raw ADC readings in mV, as the API reports them (v2.0.2).
/// One field per axis slot (protocol §1.3: 0 steering, 1 accel,
/// 2 brake, 3 clutch, 4 handbrake). [`None`] = that slot was absent
/// from the latest frame, or no frame has arrived yet — never a
/// fake 0. `age_ms` is the age of the sample ([`None`] = no sample
/// ever seen); a capture refuses a stale sample instead of saving
/// it, so the UI must never treat an old reading as live.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RawMv {
    /// Steering raw mV (axis slot 0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steering_mv: Option<u16>,
    /// Accelerator raw mV (axis slot 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accel_mv: Option<u16>,
    /// Brake raw mV (axis slot 2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brake_mv: Option<u16>,
    /// Clutch raw mV (axis slot 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clutch_mv: Option<u16>,
    /// Handbrake raw mV (axis slot 4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handbrake_mv: Option<u16>,
    /// Age of the sample in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub age_ms: Option<u64>,
}

/// One persisted button→action binding, as the API reports it
/// (v2.0.2). `button` is a physical wheel button (1–32) or an
/// encoder virtual button (65–72); `action` is a game input
/// (`horn`, `shifter_up`, `shifter_down`, `gear_1`…`gear_6`,
/// `handbrake`) or an app action (`profile_next`,
/// `profile_previous`).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ButtonBinding {
    /// Wheel button number.
    pub button: u8,
    /// Action id.
    pub action: String,
}

/// vJoy driver status, as the API reports it (v2.0.1 hotfix
/// BACKEND-2). Detection is a file probe
/// (`vJoyInterface.dll` resolvable); `version` is [`None`] until a
/// version read exists — the DLL is never loaded just to ask its
/// version, and an unmeasured version is never invented.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VjoyStatus {
    /// True when the vJoy driver DLL was found.
    pub installed: bool,
    /// True while a background install started by
    /// [`AppApi::install_vjoy`] is still running.
    pub installing: bool,
    /// The last install failure, when the last install failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_error: Option<String>,
    /// Resolved `vJoyInterface.dll` path, when found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dll_path: Option<String>,
    /// Installed driver version, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// Live engine / link diagnostics, as the API reports it (v2.0.1
/// hotfix BACKEND-2). Every field is measured, never a constant:
/// `loop_hz` / jitter / latencies are [`None`] until ticks are
/// measured, `loss_pct` is always a number (0 % loss is real), and
/// `seq_gap` / `tx_fail` are the board's counters from the latest
/// status packet.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagStats {
    /// Engine loop rate in Hz (target 200); [`None`] until measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_hz: Option<f64>,
    /// Packet loss in percent, sliding window.
    pub loss_pct: f64,
    /// Inter-arrival jitter p99 in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jitter_ms_p99: Option<f64>,
    /// Output latency p99 in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_latency_ms: Option<f64>,
    /// End-to-end latency p99 in milliseconds (ping-derived).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub e2e_ms: Option<f64>,
    /// `seqGap` counter from the latest status packet.
    pub seq_gap: u64,
    /// `txFail` counter from the latest status packet.
    pub tx_fail: u64,
    /// IPC dispatch latency p50 in milliseconds, across methods
    /// (DS-82): the daemon-side cost of one round-trip. [`None`]
    /// until the daemon has measured a dispatch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipc_p50_ms: Option<f64>,
    /// IPC dispatch latency p99 in milliseconds (DS-82).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipc_p99_ms: Option<f64>,
    /// Per-method IPC dispatch latency (R2-46 / DS-82): one entry
    /// per method the daemon has served, with its call count and
    /// p50/p99 in milliseconds — the global aggregate above cannot
    /// say *which* method is slow. Empty until the daemon has
    /// measured a dispatch; the core leaves it empty (only the
    /// daemon's `DaemonApi::diag_stats` fills it in).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ipc_per_method_ms: Vec<IpcMethodLatency>,
    /// Action → visible-result latency p50 in milliseconds
    /// (R3-D-13, R2-46 remainder): from a mutating IPC dispatch
    /// completing (connect, launch game, toggle output, …) to the
    /// first `poll` / `slow_poll` dispatch after it — the poll
    /// that shows the action's result. The daemon measures from
    /// mutation completion, not from the UI click itself (the
    /// click timestamp lives in the UI); the outbound IPC leg is
    /// in `ipc_p50_ms` above. [`None`] until an action + a later
    /// poll have both been observed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_p50_ms: Option<f64>,
    /// Action → visible-result latency p99 in milliseconds (R3-D-13).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_p99_ms: Option<f64>,
}

/// One method's IPC dispatch latency (R2-46): the per-method detail
/// behind [`DiagStats::ipc_per_method_ms`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct IpcMethodLatency {
    /// IPC method name (`"poll"`, `"slow_poll"`, …).
    pub method: String,
    /// Total dispatches ever recorded for this method.
    pub calls: u64,
    /// p50 dispatch latency in milliseconds.
    pub p50_ms: f64,
    /// p99 dispatch latency in milliseconds.
    pub p99_ms: f64,
}

/// Push events from the daemon to the UI. The UI never polls for
/// these (LOGIC-FIX-PLAN §2 rule 4): it subscribes once and reacts.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "PascalCase")]
pub enum AppEvent {
    /// The connection state changed (one event per transition).
    ConnectionChanged {
        /// New state.
        state: ConnectionState,
        /// UI label for the new state.
        label: String,
        /// Typed reason, when the new state is `Error`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<ConnectionError>,
    },
    /// A live snapshot (only while a UI is attached).
    Snapshot(SnapshotInfo),
    /// A notification for the bell / toast list (Phase 3 renders it).
    Notification {
        /// Stable kind (`ConnectionLost`, …).
        kind: String,
        /// Short title.
        title: String,
        /// One-line body.
        body: String,
    },
    /// A background job progressed (`Job{id, state, progress, error}`
    /// — LOGIC-FIX-PLAN §2 rule 2: pairing, firmware upload, config
    /// push, restart never block the UI thread).
    JobProgress {
        /// Job id.
        id: String,
        /// `running` | `done` | `failed`.
        state: String,
        /// 0.0–1.0, when known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        progress: Option<f64>,
        /// Typed error on failure.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<UserError>,
    },
}

/// The typed daemon API. Every method is synchronous from the
/// caller's view; long work (firmware upload) runs as an
/// async job and reports through [`AppEvent::JobProgress`] — never
/// by blocking the caller (LOGIC-FIX-PLAN §2 rule 2, A-01).
///
/// Errors are [`UserError`] (typed, renderable), never bare strings.
pub trait AppApi: Send + Sync {
    /// App version (workspace version, A-18).
    fn version(&self) -> String;

    /// Current connection state.
    fn connection(&self) -> ConnectionInfo;

    /// Ask to connect (Wi-Fi by default, or the requested transport).
    /// Returns the connection state right after the request.
    ///
    /// # Errors
    ///
    /// [`UserError`] when no transport can be started.
    fn connect(&self, transport: ApiTransport) -> Result<ConnectionInfo, UserError>;

    /// Disconnect the live transport.
    ///
    /// # Errors
    ///
    /// [`UserError`] when the engine is not running.
    fn disconnect(&self) -> Result<ConnectionInfo, UserError>;

    /// Current Wi-Fi status (SSID / connected / password set).
    fn wifi_status(&self) -> WifiStatus;

    /// Set (or change) the board Wi-Fi password (D19): the new
    /// password + its confirmation. Validation is 8–63 printable
    /// ASCII; the two must match. On success the board restarts
    /// its AP WPA2-PSK and the PC reconnects with the new
    /// password. Default: error — only the daemon sets it.
    ///
    /// # Errors
    ///
    /// [`UserError`] when validation fails, no board answers, or
    /// the password could not be stored / applied.
    fn set_wifi_password(&self, _password: &str, _confirm: &str) -> Result<WifiStatus, UserError> {
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "setting the Wi-Fi password is not supported by this API",
        ))
    }

    /// Forget the Wi-Fi password ("Forget password / reset to open
    /// AP", D19): clears the PC-side stores (credential-store
    /// `:wifi` entry + WLAN profile). The board-side password is
    /// cleared only by holding BOOT ≥ 10 s (D7) — the reply says
    /// so when the board still has a password. Default: error —
    /// only the daemon clears it.
    ///
    /// # Errors
    ///
    /// [`UserError`] when the password could not be cleared.
    fn clear_wifi_password(&self) -> Result<WifiStatus, UserError> {
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "clearing the Wi-Fi password is not supported by this API",
        ))
    }

    /// Scan for nearby board hotspots (D4): SSIDs starting with
    /// `RSH-` (`RSH-XXXX`, last 4 hex of the board Wi-Fi MAC).
    /// Default: empty — only the daemon scans.
    fn scan_rsh_networks(&self) -> Vec<String> {
        Vec::new()
    }

    /// Join a board hotspot that has no password (open AP, D5):
    /// writes the open WLAN profile and connects. Default: error —
    /// only the daemon joins.
    ///
    /// # Errors
    ///
    /// [`UserError`] when the network cannot be joined.
    fn rejoin_wifi_open(&self, _ssid: &str) -> Result<WifiStatus, UserError> {
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "joining the open Wi-Fi network is not supported by this API",
        ))
    }

    /// Join a board hotspot with its stored password (D5/D8):
    /// writes the WLAN profile with the password and connects.
    /// Default: error — only the daemon joins.
    ///
    /// # Errors
    ///
    /// [`UserError`] when the network cannot be joined.
    fn rejoin_wifi(&self, _ssid: &str, _password: &str) -> Result<WifiStatus, UserError> {
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "joining the Wi-Fi network is not supported by this API",
        ))
    }

    /// Latest live snapshot (steering / pedals).
    fn snapshot(&self) -> SnapshotInfo;

    /// Whether "Send to game" output is enabled.
    fn output_enabled(&self) -> bool;

    /// Set "Send to game" on/off.
    fn set_output_enabled(&self, enabled: bool);

    /// One combined live-state read (L-4, v2.0.2): the snapshot,
    /// the connection, and the output flag in a single call (see
    /// [`PollState`]). The UI poller calls this once per cycle
    /// instead of three sequential IPC round-trips.
    ///
    /// Default: reads the three getters back-to-back in this one
    /// call — no IPC round-trip (and no await / sleep) between
    /// them, so the values cannot drift the way three separate
    /// calls can. That is enough for simple / immutable
    /// implementations (sim, stubs). A stateful implementation
    /// whose state can change between two reads (the daemon: the
    /// manager and the engine tick on different locks) MUST
    /// override this and read each source exactly once — the
    /// daemon's override reads one core snapshot, one transport
    /// state, one manager state, and one settings copy, then
    /// derives all three fields from those four reads.
    fn poll(&self) -> PollState {
        PollState {
            snapshot: self.snapshot(),
            connection: self.connection(),
            output_enabled: self.output_enabled(),
        }
    }

    /// One combined slow-state read (DS-29, v2.0.3): everything the
    /// UI's ~1 Hz slow poller fetches, in a single call (see
    /// [`SlowPollState`]) — the L-4 pattern, extended from `poll()`.
    ///
    /// Default: reads the getters back-to-back in this one call —
    /// no IPC round-trip (and no await / sleep) between them. That
    /// is enough for simple / immutable implementations (sim, stubs)
    /// and for the daemon: one IPC round-trip instead of 30 is the
    /// win, and the daemon's getters are already cheap reads off its
    /// locks. No override is needed anywhere.
    fn slow_poll(&self) -> SlowPollState {
        let mut tunings = std::collections::BTreeMap::new();
        for module in [
            "steering",
            "throttle",
            "brake",
            "clutch",
            "handbrake",
            "led",
            "shifter",
            "horn",
        ] {
            tunings.insert(module.to_string(), self.get_tuning(module));
        }
        let mut modules = std::collections::BTreeMap::new();
        for module in ["handbrake", "shifter", "horn", "led"] {
            modules.insert(module.to_string(), self.module_enabled(module));
        }
        SlowPollState {
            version: self.version(),
            firmware_version: self.firmware_version(),
            backend: self.output_backend(),
            autostart: self.autostart(),
            board_address: self.board_address(),
            diag_stats: self.diag_stats().ok(),
            wifi_status: self.wifi_status(),
            calibration: self.calibration().ok(),
            raw_mv: self.raw_mv().ok(),
            profiles: self.list_profiles(),
            games: self.list_games(),
            firmware_check: self.firmware_check().ok(),
            firmware_update_status: self.firmware_update_status(),
            vjoy_status: self.vjoy_status().ok(),
            auto_pause: self.auto_pause(),
            close_to_tray: self.close_to_tray(),
            bindings: self.list_bindings(),
            knobs: self.knob_actions(),
            draft: self.draft_status(),
            tunings,
            modules,
        }
    }

    /// Slow-cache generation (F5, v2.0.3): bumps whenever the
    /// implementation's background cache refreshes a slow value.
    /// The UI re-syncs the pages when it changes — that is how a
    /// daemon-side change reaches the UI without any UI-thread IPC.
    ///
    /// Default: `0` forever — implementations without a slow cache
    /// (sim, stubs, the daemon itself) never trigger a resync.
    fn cache_generation(&self) -> u64 {
        0
    }

    /// Whether auto-profile switching is paused (M6, v2.0.2).
    /// Default: not paused — only the daemon overrides it.
    fn auto_pause(&self) -> bool {
        false
    }

    /// Pause / resume auto-profile switching; returns the new state.
    /// Persists the choice and stops (or restarts) the watcher.
    ///
    /// # Errors
    ///
    /// [`UserError`] when the setting cannot be persisted.
    fn set_auto_pause(&self, _paused: bool) -> Result<bool, UserError> {
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "auto-pause is not supported by this API",
        ))
    }

    /// Whether closing the window keeps RSH in the tray (M7, v2.0.2).
    /// Default: true — only the daemon overrides it.
    fn close_to_tray(&self) -> bool {
        true
    }

    /// Set "keep running in the tray" on/off; returns the new state.
    ///
    /// # Errors
    ///
    /// [`UserError`] when the setting cannot be persisted.
    fn set_close_to_tray(&self, _enabled: bool) -> Result<bool, UserError> {
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "close-to-tray is not supported by this API",
        ))
    }

    /// Current tuning for one module (stored value, or the neutral
    /// default when nothing is stored).
    fn get_tuning(&self, module: &str) -> TuningSettings;

    /// Validate + store one module's tuning for this session, push
    /// the rebuilt pipeline plan live to the running engine, and
    /// mark the draft dirty (L4, D-1). Returns the stored tuning.
    ///
    /// # Errors
    ///
    /// [`UserError`] when the tuning fails validation or there is no
    /// active profile to draft against.
    fn set_tuning(&self, tuning: &TuningSettings) -> Result<TuningSettings, UserError>;

    /// Shifter sensitivity (SH1, 0..100) from the board config blob
    /// — the `SHIFTER` record's `params[4]` — or [`None`] when the
    /// board has no shifter configured. This is a board-config
    /// value, NOT a `set_tuning` field: the firmware reads it from
    /// the config blob (`rsh_shifter_debounce_ms`: 0 → 20 ms,
    /// 50 → 13 ms, 100 → 5 ms). Default: `Ok(None)` — only the
    /// daemon reads it.
    ///
    /// # Errors
    ///
    /// [`UserError`] when the board cannot be reached.
    fn shifter_sensitivity(&self) -> Result<Option<u8>, UserError> {
        Ok(None)
    }

    /// Write the shifter sensitivity (SH1, 0..100) into the board
    /// config blob: read → set `params[4]` → stage → commit →
    /// confirm (the config transaction path). The firmware applies
    /// the new debounce on COMMIT, with no reboot. Returns the
    /// applied sensitivity. Default: error — only the daemon writes
    /// it.
    ///
    /// # Errors
    ///
    /// [`UserError`] when the sensitivity is outside 0..=100, no
    /// shifter is configured on the board, or the board cannot be
    /// reached / rejects the transaction.
    fn set_shifter_sensitivity(&self, _sensitivity: u8) -> Result<u8, UserError> {
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "setting the shifter sensitivity is not supported by this API",
        ))
    }

    /// Draft status for the Save / Discard prompt; [`None`] when no
    /// draft exists (nothing edited since the last save or switch).
    fn draft_status(&self) -> Option<DraftStatus>;

    /// Persist the draft to disk. Returns `true` when something was
    /// actually saved, `false` when there was no dirty draft.
    ///
    /// # Errors
    ///
    /// [`UserError`] when there is no active profile or the store
    /// save fails.
    fn save_draft(&self) -> Result<bool, UserError>;

    /// Discard the draft (session tunings revert to the stored
    /// profile, live plan pushed again). Returns `true` when a dirty
    /// draft was discarded.
    fn discard_draft(&self) -> bool;

    /// Whether a module is enabled (H7, v2.0.2): per-module on/off
    /// from the stored tuning (`PerModuleTuning.enabled`). Default:
    /// on — only the daemon overrides it.
    fn module_enabled(&self, _module: &str) -> bool {
        true
    }

    /// Enable / disable a module (H7): persists the flag in the
    /// stored tuning and rebuilds the live pipeline plan, so a
    /// disabled module stops feeding the engine. Returns the new state.
    ///
    /// # Errors
    ///
    /// [`UserError`] when the module is unknown or cannot be saved.
    fn set_module_enabled(&self, _module: &str, _enabled: bool) -> Result<bool, UserError> {
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "module toggles are not supported by this API",
        ))
    }

    /// Whether the daemon starts with Windows / at login (v2.0.1
    /// hotfix: the Settings toggle had nothing to call before this).
    /// Default: off — only the daemon and the sim override it.
    fn autostart(&self) -> bool {
        false
    }

    /// Set autostart on/off; returns the new state.
    ///
    /// # Errors
    ///
    /// [`UserError`] when the setting cannot be persisted or the
    /// autostart entry cannot be synced.
    fn set_autostart(&self, _enabled: bool) -> Result<bool, UserError> {
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "autostart is not supported by this API",
        ))
    }

    /// Board firmware version (`"4.0.2"`), or `"Unknown"` when no
    /// authenticated session is live yet. Default: `"Unknown"`.
    fn firmware_version(&self) -> String {
        "Unknown".to_string()
    }

    /// Firmware update check (H3, v2.0.2): board live version vs the
    /// bundled OTA image version. Default: board version only, no
    /// bundled image, no update — only the daemon checks the bundle.
    ///
    /// # Errors
    ///
    /// [`UserError`] when the board version cannot be read.
    fn firmware_check(&self) -> Result<FirmwareCheck, UserError> {
        Ok(FirmwareCheck {
            board_version: self.firmware_version(),
            bundled_version: None,
            update_available: false,
        })
    }

    /// Start a firmware flash from the bundled OTA image (H-4,
    /// v2.0.2): validates the image + live board, then flashes on a
    /// background thread and returns the running status immediately —
    /// the caller polls [`AppApi::firmware_update_status`] for
    /// progress and the confirmed version. A second start while one
    /// is running is an error, never a second flash. Default:
    /// unsupported (sim / test impls).
    ///
    /// # Errors
    ///
    /// [`UserError`] when no update can start (already running, no
    /// live wheel, no bundled image, or the image fails validation).
    fn update_firmware(&self) -> Result<FirmwareUpdateStatus, UserError> {
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "firmware update is not supported by this API",
        ))
    }

    /// Current firmware-flash status (H-4): the running progress, or
    /// the finished outcome, or the idle default when no flash ever
    /// started. Default: idle.
    fn firmware_update_status(&self) -> FirmwareUpdateStatus {
        FirmwareUpdateStatus::default()
    }

    /// Live output backend name (`"HIDMaestro"` / `"vJoy"`).
    /// Default: `"Unknown"` — only the daemon and the sim override it.
    fn output_backend(&self) -> String {
        "Unknown".to_string()
    }

    /// Switch the output backend; returns the live backend name
    /// after the switch.
    ///
    /// # Errors
    ///
    /// [`UserError`] when the name is unknown or the switch fails.
    fn set_output_backend(&self, _name: &str) -> Result<String, UserError> {
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "output backend switching is not supported by this API",
        ))
    }

    /// Board address the transport connects to (H4, v2.0.2): the
    /// persisted `esp_ip` (private/loopback IPv4 literal, default
    /// `192.168.4.1`). Default: the default address.
    fn board_address(&self) -> String {
        "192.168.4.1".to_string()
    }

    /// Set the board address (H4): validates (IPv4 literal, private
    /// or loopback — `rsh.local` is not an address) and persists it;
    /// returns the stored address. The transport uses it on the next
    /// (re)connect.
    ///
    /// # Errors
    ///
    /// [`UserError`] when the address is invalid or cannot be saved.
    fn set_board_address(&self, _address: &str) -> Result<String, UserError> {
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "board address is not supported by this API",
        ))
    }

    /// Force a backup rotation now; returns a one-line summary.
    ///
    /// # Errors
    ///
    /// [`UserError`] when no backup can be written (the sim never
    /// pretends a backup happened — it always errors here).
    fn backup_now(&self) -> Result<String, UserError> {
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "backup is not supported by this API",
        ))
    }

    /// All profiles, in store order (v2.0.1 hotfix: the Profiles
    /// screen showed hardcoded names before this). Default: empty —
    /// only the daemon and the sim override it, and the sim returns
    /// empty rather than inventing profiles.
    fn list_profiles(&self) -> Vec<ProfileSummary> {
        Vec::new()
    }

    /// Activate a profile; returns the active profile's name.
    ///
    /// # Errors
    ///
    /// [`UserError`] for an unknown id or a store save failure.
    fn activate_profile(&self, _id: &str) -> Result<String, UserError> {
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "profiles are not supported by this API",
        ))
    }

    /// Create a profile with default tuning under `name`; returns
    /// the created profile's name.
    ///
    /// # Errors
    ///
    /// [`UserError`] when the name is invalid or saving fails.
    fn create_profile(&self, _name: &str) -> Result<String, UserError> {
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "profiles are not supported by this API",
        ))
    }

    /// Duplicate a profile (new id, `"copy"` name); returns the new
    /// profile's name.
    ///
    /// # Errors
    ///
    /// [`UserError`] for an unknown id or a store save failure.
    fn duplicate_profile(&self, _id: &str) -> Result<String, UserError> {
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "profiles are not supported by this API",
        ))
    }

    /// Delete a profile; returns `true` when it was deleted.
    ///
    /// # Errors
    ///
    /// [`UserError`] for an unknown id or a store save failure.
    fn delete_profile(&self, _id: &str) -> Result<bool, UserError> {
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "profiles are not supported by this API",
        ))
    }

    /// Rename a profile; returns the new name. The active profile
    /// (and its unsaved draft, when one exists) is renamed with it.
    ///
    /// # Errors
    ///
    /// [`UserError`] when the name is invalid, the id is unknown,
    /// or saving fails.
    fn rename_profile(&self, _id: &str, _name: &str) -> Result<String, UserError> {
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "profiles are not supported by this API",
        ))
    }

    /// Link a profile to a game (`game_slug`, e.g.
    /// `"assetto_corsa"`); returns the linked game's display name.
    ///
    /// # Errors
    ///
    /// [`UserError`] for an unknown id, an unknown game slug, or a
    /// store save failure.
    fn link_profile(&self, _id: &str, _game_slug: &str) -> Result<String, UserError> {
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "profiles are not supported by this API",
        ))
    }

    /// Restore the newest present profiles backup (Bak1, else Bak2)
    /// over the live profiles; returns a one-line summary.
    ///
    /// # Errors
    ///
    /// [`UserError`] when no profiles backup exists or the restore
    /// fails.
    fn restore_backup(&self) -> Result<String, UserError> {
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "backup restore is not supported by this API",
        ))
    }

    /// Every catalogue game with install / running / bound-profile
    /// state (v2.0.1 hotfix: the Overview game rows were hardcoded
    /// before this). Default: empty — never invented game states.
    fn list_games(&self) -> Vec<GameSummary> {
        Vec::new()
    }

    /// Launch a game (exactly one at a time); returns its state
    /// after the launch.
    ///
    /// # Errors
    ///
    /// [`UserError`] when the game is unknown, not installed,
    /// another game is already running, or the OS refuses to start it.
    fn launch_game(&self, _id: &str) -> Result<GameSummary, UserError> {
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "games are not supported by this API",
        ))
    }

    /// Stop the tracked running game; returns its state after the
    /// stop (the pre-game profile is restored by the backend).
    ///
    /// # Errors
    ///
    /// [`UserError`] when nothing is running, another game is
    /// running, or the tracked process could not be terminated.
    fn stop_game(&self, _id: &str) -> Result<GameSummary, UserError> {
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "games are not supported by this API",
        ))
    }

    /// One game's current state (installed / running / bound profile).
    ///
    /// # Errors
    ///
    /// [`UserError`] for an unknown game id.
    fn game_state(&self, _id: &str) -> Result<GameSummary, UserError> {
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "games are not supported by this API",
        ))
    }

    /// Add a manually added game (name + exe/shortcut path,
    /// optional bound profile id); returns the new entry.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty name or exe path, an unknown
    /// bound profile, or a save failure.
    fn add_game(
        &self,
        _name: &str,
        _exe_path: &str,
        _profile_id: Option<String>,
    ) -> Result<GameSummary, UserError> {
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "games are not supported by this API",
        ))
    }

    /// Update a manually added game's name / exe path / bound
    /// profile; returns the updated entry.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown id, an empty name or exe
    /// path, an unknown bound profile, or a save failure.
    fn update_game(
        &self,
        _id: &str,
        _name: &str,
        _exe_path: &str,
        _profile_id: Option<String>,
    ) -> Result<GameSummary, UserError> {
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "games are not supported by this API",
        ))
    }

    /// Remove a manually added game; returns `true` when removed.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown id or a running game.
    fn remove_game(&self, _id: &str) -> Result<bool, UserError> {
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "games are not supported by this API",
        ))
    }

    /// Arm learn mode: the next physical wheel-button press (1–32)
    /// is captured for the binding editor. Returns the learning
    /// state after the call (`true` when armed).
    ///
    /// # Errors
    ///
    /// [`UserError`] — the daemon never fails this; the default
    /// (stubs / sim) errors because there is no wheel to learn from.
    fn start_learn(&self) -> Result<bool, UserError> {
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "learn mode is not supported by this API",
        ))
    }

    /// Cancel learn mode without picking a button. Returns the
    /// learning state after the call (`false` when disarmed).
    ///
    /// # Errors
    ///
    /// [`UserError`] — see [`AppApi::start_learn`].
    fn cancel_learn(&self) -> Result<bool, UserError> {
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "learn mode is not supported by this API",
        ))
    }

    /// Whether learn mode is armed. Default: `false`.
    fn is_learning(&self) -> bool {
        false
    }

    /// Take the button learn mode picked ([`None`] when nothing was
    /// picked yet). Consuming: a second read returns [`None`] until
    /// the next pick. Default: [`None`] — there is no pick to take
    /// without a daemon. NOTE: there is no backend that assigns the
    /// picked button to an action yet (no `set_binding` in
    /// `AppStateCore`); the UI can show the picked button number,
    /// but binding it is a follow-up, not part of this hotfix.
    fn take_learned_button(&self) -> Option<u8> {
        None
    }

    /// The active profile's captured calibration (v2.0.1 hotfix
    /// BACKEND-2). Default: error — only the daemon reads the store.
    ///
    /// # Errors
    ///
    /// [`UserError`] when there is no active profile or no steering
    /// calibration was captured yet.
    fn calibration(&self) -> Result<CalibrationInfo, UserError> {
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "calibration is not supported by this API",
        ))
    }

    /// Latest raw ADC readings in mV (v2.0.2): the live values the
    /// capture methods sample, straight from the latest authenticated
    /// stream frame — never derived by inverting processed values.
    /// Default: error — only the daemon sees live frames.
    ///
    /// # Errors
    ///
    /// [`UserError`] when there is no daemon behind this API.
    fn raw_mv(&self) -> Result<RawMv, UserError> {
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "raw readings are not supported by this API",
        ))
    }

    /// Capture the steering centre from the live raw reading (v2.0.2):
    /// the wheel must be held straight; the current steering mV is
    /// saved as the centre of the active profile (persisted) and the
    /// live pipeline plan is rebuilt from it. Returns the captured
    /// mV. Default: error — only the daemon captures.
    ///
    /// # Errors
    ///
    /// [`UserError`] when there is no live wheel data, the reading is
    /// out of range, there is no active profile, or the save fails.
    fn capture_centre(&self) -> Result<u16, UserError> {
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "centre capture is not supported by this API",
        ))
    }

    /// Capture one pedal's released ("up") endpoint from the live raw
    /// reading (v2.0.2): `pedal` is `"accel"` (or `"accelerator"`),
    /// `"brake"`, `"clutch"`, or `"handbrake"`. The current mV is
    /// saved as that pedal's minimum (persisted, and pushed into the
    /// live pipeline plan), and the captured mV is returned. Default:
    /// error — only the daemon captures.
    ///
    /// # Errors
    ///
    /// [`UserError`] when the pedal is unknown, there is no live
    /// wheel data (or that axis is absent), the captured range would
    /// be invalid, there is no active profile, or the save fails.
    fn capture_pedal_min(&self, pedal: &str) -> Result<u16, UserError> {
        let _ = pedal;
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "pedal capture is not supported by this API",
        ))
    }

    /// Capture one pedal's fully-pressed ("down") endpoint from the
    /// live raw reading (v2.0.2): same pedals as
    /// [`AppApi::capture_pedal_min`], saved as that pedal's maximum.
    /// Default: error — only the daemon captures.
    ///
    /// # Errors
    ///
    /// [`UserError`] — see [`AppApi::capture_pedal_min`].
    fn capture_pedal_max(&self, pedal: &str) -> Result<u16, UserError> {
        let _ = pedal;
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "pedal capture is not supported by this API",
        ))
    }

    /// Capture the steering full-left endpoint from the live raw
    /// reading (S1, per-module tuning): the wheel must be turned
    /// fully left. The reading lands on whichever endpoint side
    /// of the centre it falls on (below → minimum, above →
    /// maximum), so a reversed pot still captures. Default: error
    /// — only the daemon captures.
    ///
    /// # Errors
    ///
    /// [`UserError`] when there is no live wheel data, the reading
    /// sits at the centre, the captured range would be invalid,
    /// there is no active profile, or the save fails.
    fn capture_steering_left(&self) -> Result<u16, UserError> {
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "steering capture is not supported by this API",
        ))
    }

    /// Capture the steering full-right endpoint from the live raw
    /// reading (S1): the wheel must be turned fully right. Same
    /// side-of-centre storage as [`AppApi::capture_steering_left`].
    /// Default: error — only the daemon captures.
    ///
    /// # Errors
    ///
    /// [`UserError`] — see [`AppApi::capture_steering_left`].
    fn capture_steering_right(&self) -> Result<u16, UserError> {
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "steering capture is not supported by this API",
        ))
    }

    /// All persisted button→action bindings (v2.0.2), in button
    /// order. Default: empty — only the daemon owns the store.
    fn list_bindings(&self) -> Vec<ButtonBinding> {
        Vec::new()
    }

    /// Bind `button` (physical 1–32, encoder virtual 65–72) to
    /// `action` — a game input (`horn`, `shifter_up`, `shifter_down`,
    /// `gear_1`…`gear_6`, `handbrake`) or an app action
    /// (`profile_next`, `profile_previous`) — and persist it. The
    /// daemon applies the binding to every engine tick from the next
    /// one on. Default: error — only the daemon binds.
    ///
    /// # Errors
    ///
    /// [`UserError`] with code `InvalidButton`, `UnknownAction`, or
    /// `BindingSaveFailed`.
    fn set_binding(&self, button: u8, action: &str) -> Result<ButtonBinding, UserError> {
        let _ = (button, action);
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "button bindings are not supported by this API",
        ))
    }

    /// Remove the binding for `button` (v2.0.2). Returns `true`
    /// when a binding was actually removed. Default: error — only
    /// the daemon binds.
    ///
    /// # Errors
    ///
    /// [`UserError`] with code `BindingSaveFailed` when the removal
    /// cannot be persisted.
    fn clear_binding(&self, button: u8) -> Result<bool, UserError> {
        let _ = button;
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "button bindings are not supported by this API",
        ))
    }

    /// All persisted long-press button→action bindings (BU2), in
    /// button order. Default: empty — only the daemon owns the store.
    fn list_long_press_bindings(&self) -> Vec<ButtonBinding> {
        Vec::new()
    }

    /// Bind `button`'s long-press action (BU2, physical buttons
    /// 1–32) — the second action that takes over once the button
    /// has been held past the long-press threshold — and persist
    /// it. Default: error — only the daemon binds.
    ///
    /// # Errors
    ///
    /// [`UserError`] with code `InvalidButton`, `UnknownAction`, or
    /// `BindingSaveFailed`.
    fn set_long_press_binding(&self, button: u8, action: &str) -> Result<ButtonBinding, UserError> {
        let _ = (button, action);
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "long-press bindings are not supported by this API",
        ))
    }

    /// Remove the long-press binding for `button` (BU2). Returns
    /// `true` when one was actually removed. Default: error — only
    /// the daemon binds.
    ///
    /// # Errors
    ///
    /// [`UserError`] with code `BindingSaveFailed` when the removal
    /// cannot be persisted.
    fn clear_long_press_binding(&self, button: u8) -> Result<bool, UserError> {
        let _ = button;
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "long-press bindings are not supported by this API",
        ))
    }

    /// Persisted knob actions (M9, v2.0.2): two knobs × three rows
    /// of action ids (`""` = not set). Default: all unset — only the
    /// daemon overrides it.
    fn knob_actions(&self) -> [[String; 3]; 2] {
        [
            [String::new(), String::new(), String::new()],
            [String::new(), String::new(), String::new()],
        ]
    }

    /// Set one knob row's action (M9): `knob` 0–1, `row` 0–2,
    /// `action` an assignable action id or `""` to clear. Persists
    /// the knob table. Returns `true` when stored.
    ///
    /// # Errors
    ///
    /// [`UserError`] for a bad knob/row or an unknown action id.
    fn set_knob_action(&self, _knob: u8, _row: u8, _action: &str) -> Result<(), UserError> {
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "knob actions are not supported by this API",
        ))
    }

    /// vJoy driver status (v2.0.1 hotfix BACKEND-2): installed /
    /// installing / last install error / DLL path / version.
    /// Default: error — only the daemon probes the driver.
    ///
    /// # Errors
    ///
    /// [`UserError`] — the default (stubs / sim) always errors
    /// because there is no daemon to probe with.
    fn vjoy_status(&self) -> Result<VjoyStatus, UserError> {
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "vJoy status is not supported by this API",
        ))
    }

    /// Start a vJoy driver install in the background and return
    /// immediately (`"started"`); progress and failure surface
    /// through [`AppApi::vjoy_status`] polling — never by blocking
    /// the caller. The installer needs admin once (its own manifest
    /// triggers the UAC prompt); that is expected and unlike
    /// `HIDMaestro`, which prompts on every launch. Default: error —
    /// only the daemon installs.
    ///
    /// # Errors
    ///
    /// [`UserError`] on non-Windows (vJoy is Windows-only), when an
    /// install is already running, or when the download source is
    /// unverified.
    fn install_vjoy(&self) -> Result<String, UserError> {
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "vJoy install is not supported by this API",
        ))
    }

    /// Live engine / link diagnostics (v2.0.1 hotfix BACKEND-2):
    /// loop rate, loss, jitter / latency p99s, board `seqGap` /
    /// `txFail` counters. Default: error — only the daemon measures.
    ///
    /// # Errors
    ///
    /// [`UserError`] — the default (stubs / sim) always errors
    /// because there is no engine to measure.
    fn diag_stats(&self) -> Result<DiagStats, UserError> {
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "diagnostics are not supported by this API",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// DS-01 (deep scan, U-01): the release gate — `app_version()`
    /// must equal the release tag. The workspace version was still
    /// "2.0.2" in the v2.0.3 build, so the app displayed "RSH 2.0.2",
    /// wrote `skipped 2.0.2` markers, and the F23 wizard re-offer
    /// (`Skipped(v) => v != current_version`) could never fire for
    /// v2.0.2 skippers. This literal is the tripwire: bump it in the
    /// same commit that bumps `[workspace.package] version`, or the
    /// suite fails before the tag is cut. The release workflow
    /// (`release.yml`, "Tag matches Cargo workspace version") checks
    /// tag == workspace version; the local gate
    /// (`tools/gate-checks/run-gate.sh`, `version-check`) checks the
    /// Slint default copy too.
    #[test]
    fn app_version_matches_release_tag() {
        assert_eq!(app_version(), "2.0.11");
    }

    /// L-4: a stub whose state flips between two reads — the
    /// transition that, over IPC, happens between the snapshot
    /// round-trip and the connection round-trip of the old
    /// three-call poller.
    ///
    /// Generation `0` = connected (`Active` / "Connected",
    /// steering `0.0`); generation `1` = disconnected (`Idle` /
    /// "Disconnected"). `snapshot()` reads its generation and then
    /// advances to the next one, so a following `connection()` sees
    /// the *new* generation: the two separate reads disagree
    /// (tearing). `poll()` reads the generation exactly once under
    /// one lock and derives all three fields from it, so it never
    /// tears — the same single-read pattern the daemon's override
    /// uses (one core snapshot, one transport state, one manager
    /// state, one settings copy).
    struct FlippingApi {
        generation: Mutex<u64>,
    }

    impl FlippingApi {
        fn new() -> Self {
            Self {
                generation: Mutex::new(0),
            }
        }

        fn connection_for(generation: u64) -> ConnectionInfo {
            if generation.is_multiple_of(2) {
                ConnectionInfo {
                    state: ConnectionState::Active,
                    label: "Connected".to_string(),
                    transport: ApiTransport::Wifi,
                    address: "192.168.4.1".to_string(),
                    session_active: true,
                    board_reachable: true,
                    error: None,
                    hint: None,
                }
            } else {
                ConnectionInfo {
                    state: ConnectionState::Idle,
                    label: "Disconnected".to_string(),
                    transport: ApiTransport::Wifi,
                    address: String::new(),
                    session_active: false,
                    board_reachable: false,
                    error: None,
                    hint: None,
                }
            }
        }

        // The `u64 → f64` cast cannot lose precision for real
        // generations: the stub advances by one per read, far below
        // 2^53.
        #[allow(clippy::cast_precision_loss)]
        fn snapshot_for(generation: u64) -> SnapshotInfo {
            SnapshotInfo {
                steering_deg: generation as f64,
                throttle: 0.5,
                brake: 0.0,
                clutch: None,
                button_mask: 0,
                handbrake: 0.0,
                gear: 0,
                seq: generation,
            }
        }
    }

    impl AppApi for FlippingApi {
        fn version(&self) -> String {
            "test".to_string()
        }

        fn connection(&self) -> ConnectionInfo {
            let generation = *self
                .generation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Self::connection_for(generation)
        }

        fn connect(&self, _transport: ApiTransport) -> Result<ConnectionInfo, UserError> {
            Ok(self.connection())
        }

        fn disconnect(&self) -> Result<ConnectionInfo, UserError> {
            Ok(self.connection())
        }

        fn wifi_status(&self) -> WifiStatus {
            WifiStatus {
                ssid: None,
                connected: false,
                password_set: false,
                board_mac: None,
            }
        }

        fn snapshot(&self) -> SnapshotInfo {
            let mut generation = self
                .generation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let snap = Self::snapshot_for(*generation);
            // The transition: the state moves on after the snapshot
            // read, before any later read in the same cycle.
            *generation += 1;
            snap
        }

        fn output_enabled(&self) -> bool {
            true
        }

        fn set_output_enabled(&self, _enabled: bool) {}

        fn get_tuning(&self, module: &str) -> TuningSettings {
            TuningSettings::neutral(module)
        }

        fn set_tuning(&self, tuning: &TuningSettings) -> Result<TuningSettings, UserError> {
            Ok(tuning.clone())
        }

        fn draft_status(&self) -> Option<DraftStatus> {
            None
        }

        fn save_draft(&self) -> Result<bool, UserError> {
            Ok(false)
        }

        fn discard_draft(&self) -> bool {
            false
        }

        fn poll(&self) -> PollState {
            // One read, one lock: every field comes from the same
            // generation — no transition can interleave.
            let generation = *self
                .generation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            PollState {
                snapshot: Self::snapshot_for(generation),
                connection: Self::connection_for(generation),
                output_enabled: true,
            }
        }
    }

    #[test]
    fn separate_snapshot_and_connection_reads_tear() {
        // The bug L-4 fixes, reproduced: the snapshot says
        // generation 0 (connected) while the connection read that
        // follows it says generation 1 (disconnected).
        let api = FlippingApi::new();
        let snap = api.snapshot();
        assert_eq!(snap.seq, 0);
        assert_eq!(snap.steering_deg, 0.0);
        let conn = api.connection();
        assert_eq!(conn.state, ConnectionState::Idle);
        assert_eq!(conn.label, "Disconnected");
        assert!(
            !conn.session_active,
            "snapshot seq 0 implies connected, but the later connection read says disconnected"
        );
    }

    #[test]
    fn poll_returns_all_three_from_one_generation() {
        let api = FlippingApi::new();
        let polled = api.poll();
        // (a) one call carries snapshot + connection + output.
        assert_eq!(polled.snapshot.seq, 0);
        assert_eq!(polled.snapshot.steering_deg, 0.0);
        assert_eq!(polled.snapshot.throttle, 0.5);
        assert_eq!(polled.connection.state, ConnectionState::Active);
        assert_eq!(polled.connection.label, "Connected");
        assert!(polled.connection.session_active);
        assert!(polled.output_enabled);
        // (b) consistent: the snapshot's generation and the
        // connection's generation are the same instant — the
        // snapshot seq (0) is the connected generation, and the
        // connection agrees. A second poll is identical: `poll`
        // itself never advances the state.
        let again = api.poll();
        assert_eq!(again, polled);
        // And the wire shape round-trips (camelCase).
        let json = serde_json::to_string(&polled).expect("poll serializes");
        assert!(json.contains("\"outputEnabled\":true"), "{json}");
        let back: PollState = serde_json::from_str(&json).expect("poll round-trips");
        assert_eq!(back, polled);
    }

    /// DS-29: the default `slow_poll()` composes every slow getter
    /// into one value (the 30-call slow poller, in a single call):
    /// fallible sources become `None`, per-module tunings and module
    /// flags are all present, and the wire shape round-trips.
    #[test]
    fn slow_poll_composes_all_slow_getters() {
        let api = FlippingApi::new();
        let slow = api.slow_poll();
        assert_eq!(slow.version, "test");
        assert_eq!(slow.firmware_version, "Unknown");
        assert!(!slow.autostart);
        assert!(slow.close_to_tray);
        assert!(!slow.auto_pause);
        assert!(!slow.wifi_status.connected);
        assert!(!slow.wifi_status.password_set);
        assert!(slow.wifi_status.ssid.is_none());
        // Fallible sources: the stub errors, so the slots are None
        // (the UI keeps its old value — same as a failed per-call
        // fetch before).
        assert!(slow.diag_stats.is_none());
        assert!(slow.calibration.is_none());
        assert!(slow.raw_mv.is_none());
        assert!(slow.firmware_check.is_some());
        assert!(slow.vjoy_status.is_none());
        assert_eq!(slow.profiles, Vec::new());
        assert_eq!(slow.games, Vec::new());
        assert_eq!(slow.bindings, Vec::new());
        assert!(slow.draft.is_none());
        // All 8 module tunings + all 4 module flags present.
        assert_eq!(slow.tunings.len(), 8);
        for module in [
            "steering",
            "throttle",
            "brake",
            "clutch",
            "handbrake",
            "led",
            "shifter",
            "horn",
        ] {
            assert!(slow.tunings.contains_key(module), "{module}");
        }
        assert_eq!(slow.modules.len(), 4);
        assert!(slow.modules.values().all(|enabled| *enabled));
        // Knobs: default all-unset.
        assert!(slow.knobs.iter().flatten().all(String::is_empty));
        // And the wire shape round-trips (camelCase).
        let json = serde_json::to_string(&slow).expect("slow_poll serializes");
        assert!(json.contains("\"firmwareVersion\":\"Unknown\""), "{json}");
        assert!(json.contains("\"closeToTray\":true"), "{json}");
        let back: SlowPollState = serde_json::from_str(&json).expect("slow_poll round-trips");
        assert_eq!(back, slow);
    }
}
