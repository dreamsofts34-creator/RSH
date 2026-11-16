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
}
