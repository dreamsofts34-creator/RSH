//! `rsh-api`: the typed RSH v2.0 daemon API (LOGIC-FIX-PLAN §2).
//!
//! * [`AppApi`] — the command trait the daemon implements and the
//!   Slint UI (Phase 3) calls over IPC.
//! * [`AppEvent`] — push events; the UI subscribes, it never polls.
//! * [`ConnectionManager`] — the L1 connection state machine
//!   (§4.1): `Idle → LinkUp → Hello → Authenticating → Active`,
//!   with `Error(reason, hint, retry_at)` + backoff (2 s → 30 s),
//!   the 2 s heartbeat watchdog, the 750 ms × 3 HELLO wait, and the
//!   one 10 s `BadTag` retry.
//! * [`ConnectionError`] / [`UserError`] — typed failures the UI
//!   renders directly (never a bare `String` error).
//! * [`app_version`] — the one version (Cargo workspace version,
//!   A-18).

pub mod api;
pub mod connection;
pub mod errors;
pub mod pairing;
pub mod tuning;

pub use api::{
    ApiTransport, AppApi, AppEvent, ConnectionInfo, PairingInfo, SnapshotInfo, app_version,
};
pub use connection::{
    BACKOFF_BASE, BACKOFF_MAX, BAD_TAG_RETRY_DELAY, ConnEvent, ConnectionManager, ConnectionState,
    HELLO_ATTEMPTS, HELLO_WAIT, ManagerAction, WATCHDOG_TIMEOUT,
};
pub use errors::{BindErrorKind, ConnectionError, PairingError, UserError};
pub use pairing::{PairingManager, PairingOutcome, PairingTransport, countdown_ring_commands};
pub use tuning::{DraftStatus, LedPattern, ShifterGate, TuningSettings, validate_tuning};
