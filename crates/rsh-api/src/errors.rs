//! Typed connection errors (LOGIC-FIX-PLAN §4.1/§4.7).
//!
//! Every failure the daemon and the Connection Manager can
//! produce, as a typed value — never a bare `String`. The UI
//! (Phase 3) matches on `code()` to show distinct copy per cause
//! (Phase 0 gate §4.3: "board unreachable" vs "board mismatch"
//! must render differently), and `hint()` is the one-line "what to
//! do" the UI shows next to it. No variant ever carries a password.

use serde::{Deserialize, Serialize};

/// Typed UDP bind failure (C-12): the `io::ErrorKind` behind a
/// failed `UdpTransport::bind`, classified so the UI can hint
/// instead of showing a raw OS string.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum BindErrorKind {
    /// Local port already in use (another RSH, or a stale socket).
    AddrInUse,
    /// Bind denied (missing privilege / restricted interface).
    PermissionDenied,
    /// No such interface / address not available on this host.
    NoInterface,
    /// Any other I/O failure.
    Other,
}

impl BindErrorKind {
    /// Classify an `io::Error` by kind.
    #[must_use]
    pub fn from_io(error: &std::io::Error) -> Self {
        use std::io::ErrorKind;
        match error.kind() {
            ErrorKind::AddrInUse => Self::AddrInUse,
            ErrorKind::PermissionDenied => Self::PermissionDenied,
            ErrorKind::AddrNotAvailable => Self::NoInterface,
            _ => Self::Other,
        }
    }

    /// Short stable code for logs and the wire.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::AddrInUse => "AddrInUse",
            Self::PermissionDenied => "PermissionDenied",
            Self::NoInterface => "NoInterface",
            Self::Other => "BindFailed",
        }
    }

    /// One-line "what to do" for the UI.
    #[must_use]
    pub const fn hint(self) -> &'static str {
        match self {
            Self::AddrInUse => {
                // F21: name the port and the prime suspect (the legacy
                // Tauri v1.0.2 app, installed exe `RSH.exe`, binds
                // the same port at launch) — "close the other RSH"
                // alone never told the user WHICH app to close.
                "Another app is using the wheel port (UDP 4211) — close the old RSH app (RSH.exe) or any other RSH, then press Retry."
            }
            Self::PermissionDenied => {
                "Binding the wheel port was denied — run RSH normally (no admin needed) and retry."
            }
            Self::NoInterface => {
                "The network interface is not available — check the RSH Wi-Fi is joined, then retry."
            }
            Self::Other => {
                "The wheel port could not be opened — retry, then restart RSH if it persists."
            }
        }
    }
}

/// Typed connection / activation failure (no-pairing redesign,
/// protocol v2.2): the board is connected by joining its Wi-Fi
/// (or plugging in USB) and an unsigned `GET_CAPS` — there is no
/// pairing, no window, no device key, so the pairing variants are
/// gone. Kept variants are the transport / board failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum ConnectionError {
    /// No HELLO reply at all: board unreachable on this transport.
    Unreachable,
    /// A control/session step timed out.
    Timeout,
    /// The transport died while waiting (channel closed).
    TransportClosed,
    /// The UDP bind failed (typed kind attached).
    BindFailed(BindErrorKind),
    /// The HELLO board MAC does not match the board this session
    /// is bound to (D11: the app binds to the board it joined —
    /// the `RSH-XXXX` SSID is per-board, so a foreign board's
    /// HELLO is refused).
    BoardMismatch,
    /// The board connected but then stopped streaming frames:
    /// the heartbeat watchdog rebuilt the transport
    /// [`MAX_WATCHDOG_REBUILDS`](crate::connection::MAX_WATCHDOG_REBUILDS)
    /// times with no frame seen and latched (F20, v2.0.3). The fix
    /// is the user's Retry.
    WatchdogStall,
    /// Anything else (detail string carried separately, never a secret).
    Unknown,
}

impl ConnectionError {
    /// Short stable code for logs, the wire, and the Phase 0 §6.2
    /// `activation failed: reason=<code>` line.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Unreachable => "Unreachable",
            Self::Timeout => "Timeout",
            Self::TransportClosed => "TransportClosed",
            Self::BindFailed(kind) => kind.code(),
            Self::BoardMismatch => "BoardMismatch",
            Self::WatchdogStall => "WatchdogStall",
            Self::Unknown => "Unknown",
        }
    }

    /// Parse a [`ConnectionError::code`] back into the typed error
    /// (F26): the daemon's IPC error carries only the code string,
    /// so the UI recovers the kind from it instead of collapsing
    /// every failure into `TransportClosed`. `None` for codes this
    /// type never produces (feature codes, typos). `BindFailed`
    /// codes are the [`BindErrorKind`] codes (`AddrInUse`,
    /// `PermissionDenied`, `NoInterface`, `BindFailed`).
    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        Some(match code {
            "Unreachable" => Self::Unreachable,
            "Timeout" => Self::Timeout,
            "TransportClosed" => Self::TransportClosed,
            "AddrInUse" => Self::BindFailed(BindErrorKind::AddrInUse),
            "PermissionDenied" => Self::BindFailed(BindErrorKind::PermissionDenied),
            "NoInterface" => Self::BindFailed(BindErrorKind::NoInterface),
            "BindFailed" => Self::BindFailed(BindErrorKind::Other),
            "BoardMismatch" => Self::BoardMismatch,
            "WatchdogStall" => Self::WatchdogStall,
            "Unknown" => Self::Unknown,
            _ => return None,
        })
    }

    /// One-line "what to do" for the UI.
    #[must_use]
    pub const fn hint(self) -> &'static str {
        match self {
            Self::Unreachable => {
                "Wheel not found — join the RSH-XXXX Wi-Fi network (or plug in the USB cable), then retry."
            }
            Self::Timeout => "The wheel did not answer in time — check the connection and retry.",
            Self::TransportClosed => "Connection to the wheel dropped — reconnecting.",
            Self::BindFailed(kind) => kind.hint(),
            Self::BoardMismatch => {
                "A different wheel answered than the one this session is bound to — check you joined the right RSH-XXXX network, then retry."
            }
            Self::WatchdogStall => {
                "The wheel connected but stopped sending data — rebuilding did not help. Check the wheel, then press Retry."
            }
            Self::Unknown => "Something went wrong — retry, then check the diagnostics log.",
        }
    }

    /// True when the fix is a user action, not a retry. No
    /// connection error needs this anymore: pairing is gone, so
    /// nothing latches on a pairing reason — every remaining
    /// failure retries with backoff (except `AddrInUse` and the
    /// watchdog latch, which the manager handles directly).
    /// Kept so callers keep compiling; always `false`.
    #[must_use]
    pub const fn needs_user_action(self) -> bool {
        let _ = self;
        false
    }
}

impl std::fmt::Display for ConnectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.code())
    }
}

/// A typed user-facing error (LOGIC-FIX-PLAN §4.7): what happened
/// and what to do, as data the UI renders directly — never a raw
/// `String` error.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct UserError {
    /// Stable code (`ConnectionError::code`, or a feature code).
    pub code: String,
    /// Short title.
    pub title: String,
    /// What happened, in one line.
    pub what_happened: String,
    /// What the user should do, in one line.
    pub what_to_do: String,
    /// Free-form detail (never a secret).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl UserError {
    /// Build from a typed connection error.
    #[must_use]
    pub fn from_connection(error: ConnectionError, detail: impl Into<String>) -> Self {
        Self {
            code: error.code().to_string(),
            title: error.code().to_string(),
            what_happened: match error {
                ConnectionError::Unreachable => "The wheel did not answer.".to_string(),
                ConnectionError::Timeout => "The wheel did not answer in time.".to_string(),
                ConnectionError::TransportClosed => {
                    "The connection to the wheel dropped.".to_string()
                }
                ConnectionError::BindFailed(_) => "The wheel port could not be opened.".to_string(),
                ConnectionError::BoardMismatch => {
                    "A different wheel answered than the one expected.".to_string()
                }
                ConnectionError::WatchdogStall => {
                    "The wheel connected but stopped sending data.".to_string()
                }
                ConnectionError::Unknown => "Something went wrong.".to_string(),
            },
            what_to_do: error.hint().to_string(),
            detail: Some(detail.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_error_code_round_trips_through_from_code() {
        // F26: every code `code()` produces must parse back to the
        // same error — the UI recovers the kind from the daemon's
        // bare code string over IPC.
        for error in [
            ConnectionError::Unreachable,
            ConnectionError::Timeout,
            ConnectionError::TransportClosed,
            ConnectionError::BindFailed(BindErrorKind::AddrInUse),
            ConnectionError::BindFailed(BindErrorKind::PermissionDenied),
            ConnectionError::BindFailed(BindErrorKind::NoInterface),
            ConnectionError::BindFailed(BindErrorKind::Other),
            ConnectionError::BoardMismatch,
            ConnectionError::WatchdogStall,
            ConnectionError::Unknown,
        ] {
            assert_eq!(
                ConnectionError::from_code(error.code()),
                Some(error),
                "code {} must round-trip",
                error.code()
            );
        }
        assert_eq!(ConnectionError::from_code("NoSuchCode"), None);
        assert_eq!(ConnectionError::from_code(""), None);
    }
}
