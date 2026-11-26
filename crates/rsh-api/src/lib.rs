//! `rsh-api`: the typed RSH v2.0 daemon API (LOGIC-FIX-PLAN §2).
//!
//! * [`AppApi`] — the command trait the daemon implements and the
//!   Slint UI (Phase 3) calls over IPC.
//! * [`AppEvent`] — push events; the UI subscribes, it never polls.
//! * [`ConnectionManager`] — the L1 connection state machine
//!   (§4.1): `Idle → LinkUp → Hello → Active`, with
//!   `Error(reason, hint, retry_at)` + backoff (2 s → 30 s), the
//!   2 s heartbeat watchdog, and the 750 ms × 3 HELLO wait. No
//!   pairing: `Active` is an unsigned `GET_CAPS` OK (protocol
//!   v2.2), and [`ConnectionInfo::board_reachable`] is the sticky
//!   HELLO latch the wizard Step 1 gate reads (D15).
//! * [`ConnectionError`] / [`UserError`] — typed failures the UI
//!   renders directly (never a bare `String` error).
//! * [`WifiStatus`] — the board hotspot state (SSID / connected /
//!   password set); the password itself never crosses the API.
//! * [`app_version`] — the one version (Cargo workspace version,
//!   A-18).

pub mod api;
pub mod connection;
pub mod errors;
pub mod restart;
pub mod tuning;
pub mod wifi;

pub use api::{
    ApiTransport, AppApi, AppEvent, ButtonBinding, CalibrationInfo, ConnectionInfo, DiagStats,
    FirmwareCheck, FirmwareUpdateStatus, GameSummary, IpcMethodLatency, PollState, ProfileSummary,
    RawMv, SlowPollState, SnapshotInfo, VjoyStatus, WheelCheck, WifiStatus, app_version,
};
pub use connection::{
    BACKOFF_BASE, BACKOFF_MAX, ConnEvent, ConnectionManager, ConnectionState, HELLO_ATTEMPTS,
    HELLO_WAIT, ManagerAction, WATCHDOG_TIMEOUT,
};
pub use errors::{BindErrorKind, ConnectionError, UserError};
pub use tuning::{DraftStatus, LedPattern, ShifterGate, TuningSettings, validate_tuning};
pub use wifi::{KEY_RESET_HOLD_SECS, ssid_for_board_mac};
