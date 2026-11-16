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
    /// True only once a signed `GET_CAPS` passed (SEC-E-14).
    pub session_active: bool,
    /// Typed failure reason, when in `Error`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ConnectionError>,
    /// One-line hint for the error, when in `Error`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// Pairing status, as the API reports it. The typed `error` is the
/// backend part of the Phase 0 §4.3 finding — the UI (Phase 3/6)
/// matches on it for distinct copy instead of one generic
/// "No pairing window is open."
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct PairingInfo {
    /// Seconds left in the pairing window; 0 when no window is open.
    pub seconds_left: u64,
    /// The one-time Wi-Fi password, shown once, or `None`.
    /// Never logged, never put in an event payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// Typed failure reason of the last activation, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ConnectionError>,
    /// Human-readable form of `error` (from the stored reason
    /// string, when the typed value is unavailable).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_detail: Option<String>,
    /// Board MAC of the last HELLO, when one was observed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board_mac: Option<String>,
    /// True when the board already has a stored key and re-pairing
    /// needs explicit user confirmation.
    pub replacement_required: bool,
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
    /// A pairing window opened or closed.
    PairingWindow {
        /// True when a window is open now.
        open: bool,
        /// Seconds left (0 when closed).
        seconds_left: u64,
        /// Board MAC, when known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        board_mac: Option<String>,
    },
    /// A pairing attempt finished (success or typed failure).
    PairingFinished {
        /// True on success.
        ok: bool,
        /// Typed reason on failure.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<ConnectionError>,
    },
    /// A live snapshot (only while a UI is attached).
    Snapshot(SnapshotInfo),
    /// A notification for the bell / toast list (Phase 3 renders it).
    Notification {
        /// Stable kind (`ConnectionLost`, `PairingNeeded`, …).
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
/// caller's view; long work (pairing, firmware upload) runs as an
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

    /// Current pairing status (typed error included).
    fn pairing_status(&self) -> PairingInfo;

    /// Begin pairing (triggers a session-activation attempt).
    ///
    /// # Errors
    ///
    /// [`UserError`] when no transport can be started.
    fn begin_pairing(&self) -> Result<PairingInfo, UserError>;

    /// Latest live snapshot (steering / pedals).
    fn snapshot(&self) -> SnapshotInfo;

    /// Whether "Send to game" output is enabled.
    fn output_enabled(&self) -> bool;

    /// Set "Send to game" on/off.
    fn set_output_enabled(&self, enabled: bool);

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
    ///
    /// NOTE (no backend): there is deliberately NO `set_centre` /
    /// `calibrate_pedal` on this trait. Capturing a centre or pedal
    /// endpoint needs the live raw ADC value in mV, and raw mV never
    /// reaches `AppStateCore`: the engine consumes the frame's
    /// `axes_mv` inside the pipeline and the state only ever sees the
    /// processed output (steering degrees, pedals 0..1). Inverting the
    /// processed values would only reproduce the stored calibration,
    /// not capture a new one — so no capture method is offered until
    /// raw mV is plumbed through (a v2.0.2 engine change).
    fn calibration(&self) -> Result<CalibrationInfo, UserError> {
        Err(UserError::from_connection(
            ConnectionError::Unknown,
            "calibration is not supported by this API",
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
