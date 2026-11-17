//! Typed connection errors (LOGIC-FIX-PLAN §4.1/§4.7).
//!
//! Every failure the daemon, the Connection Manager, and the pairing
//! path can produce, as a typed value — never a bare `String`. The UI
//! (Phase 3) matches on `code()` to show distinct copy per cause
//! (Phase 0 gate §4.3: "board unreachable" vs "window closed" vs
//! "already paired" must render differently), and `hint()` is the
//! one-line "what to do" the UI shows next to it. No variant ever
//! carries key, password, nonce, or HMAC material.

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

/// Typed connection / pairing / activation failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum ConnectionError {
    /// No HELLO reply at all: board unreachable on this transport.
    Unreachable,
    /// HELLO arrived but the pairing window is closed (board paired
    /// already, or the 60 s window expired).
    WindowClosed,
    /// The pairing window expired mid-attempt.
    WindowExpired,
    /// No device key is stored for this board — pairing is required.
    NoStoredKey,
    /// A device key is already stored and the disclosed key differs —
    /// re-pairing needs the user's explicit confirmation.
    StoredKeyExists,
    /// HMAC tag verification failed on a control reply (C-13: the
    /// board rejects a new session for 10 s after an authenticated
    /// one — one retry after 10 s, message "wheel busy").
    BadTag,
    /// A control/session step timed out.
    Timeout,
    /// The transport died while waiting (channel closed).
    TransportClosed,
    /// The UDP bind failed (typed kind attached).
    BindFailed(BindErrorKind),
    /// The board reports itself already paired; pairing is the wrong
    /// operation (resume instead).
    AlreadyPaired,
    /// The board is unpaired, so there is no stored key to resume with.
    BoardUnpaired,
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
            Self::WindowClosed => "WindowClosed",
            Self::WindowExpired => "WindowExpired",
            Self::NoStoredKey => "NoStoredKey",
            Self::StoredKeyExists => "StoredKeyExists",
            Self::BadTag => "BadTag",
            Self::Timeout => "Timeout",
            Self::TransportClosed => "TransportClosed",
            Self::BindFailed(kind) => kind.code(),
            Self::AlreadyPaired => "AlreadyPaired",
            Self::BoardUnpaired => "BoardUnpaired",
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
            "WindowClosed" => Self::WindowClosed,
            "WindowExpired" => Self::WindowExpired,
            "NoStoredKey" => Self::NoStoredKey,
            "StoredKeyExists" => Self::StoredKeyExists,
            "BadTag" => Self::BadTag,
            "Timeout" => Self::Timeout,
            "TransportClosed" => Self::TransportClosed,
            "AddrInUse" => Self::BindFailed(BindErrorKind::AddrInUse),
            "PermissionDenied" => Self::BindFailed(BindErrorKind::PermissionDenied),
            "NoInterface" => Self::BindFailed(BindErrorKind::NoInterface),
            "BindFailed" => Self::BindFailed(BindErrorKind::Other),
            "AlreadyPaired" => Self::AlreadyPaired,
            "BoardUnpaired" => Self::BoardUnpaired,
            "Unknown" => Self::Unknown,
            _ => return None,
        })
    }

    /// One-line "what to do" for the UI.
    #[must_use]
    pub const fn hint(self) -> &'static str {
        match self {
            Self::Unreachable => {
                "Wheel not found — check the RSH Wi-Fi is joined (or the USB cable is plugged in), then retry."
            }
            Self::WindowClosed => {
                "Pairing window closed — restart the wheel and pair within 60 s of it turning on."
            }
            Self::WindowExpired => {
                "Pairing window expired — restart the wheel and pair within 60 s of it turning on."
            }
            Self::NoStoredKey => "Pairing needed — pair the wheel to store its key.",
            Self::StoredKeyExists => {
                "This wheel is already paired with a different key — confirm to replace the pairing."
            }
            Self::BadTag => "Wheel is busy finishing the last session — retrying in 10 s.",
            Self::Timeout => "The wheel did not answer in time — check the connection and retry.",
            Self::TransportClosed => "Connection to the wheel dropped — reconnecting.",
            Self::BindFailed(kind) => kind.hint(),
            Self::AlreadyPaired => "Wheel is already paired — no need to pair again.",
            Self::BoardUnpaired => "Wheel is not paired yet — pair it first.",
            Self::Unknown => "Something went wrong — retry, then check the diagnostics log.",
        }
    }

    /// True when the fix is a user action, not a retry: auto-retry
    /// must stop and the UI must surface "Pairing needed" (C-01).
    #[must_use]
    pub const fn needs_user_action(self) -> bool {
        matches!(
            self,
            Self::NoStoredKey | Self::WindowClosed | Self::WindowExpired | Self::StoredKeyExists
        )
    }
}

impl std::fmt::Display for ConnectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.code())
    }
}

/// The typed pairing error the API exposes so the UI can show
/// distinct copy per cause (Phase 0 gate §4.3 — backend part; the
/// UI copy itself lands in Phase 3/6). Alias kept short for callers.
pub type PairingError = ConnectionError;

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
                ConnectionError::WindowClosed | ConnectionError::WindowExpired => {
                    "The pairing window is closed.".to_string()
                }
                ConnectionError::NoStoredKey => {
                    "No pairing key is stored for this wheel.".to_string()
                }
                ConnectionError::StoredKeyExists => {
                    "This wheel is already paired with a different key.".to_string()
                }
                ConnectionError::BadTag => {
                    "The wheel is busy finishing the last session.".to_string()
                }
                ConnectionError::Timeout => "The wheel did not answer in time.".to_string(),
                ConnectionError::TransportClosed => {
                    "The connection to the wheel dropped.".to_string()
                }
                ConnectionError::BindFailed(_) => "The wheel port could not be opened.".to_string(),
                ConnectionError::AlreadyPaired => "The wheel is already paired.".to_string(),
                ConnectionError::BoardUnpaired => "The wheel is not paired yet.".to_string(),
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
            ConnectionError::WindowClosed,
            ConnectionError::WindowExpired,
            ConnectionError::NoStoredKey,
            ConnectionError::StoredKeyExists,
            ConnectionError::BadTag,
            ConnectionError::Timeout,
            ConnectionError::TransportClosed,
            ConnectionError::BindFailed(BindErrorKind::AddrInUse),
            ConnectionError::BindFailed(BindErrorKind::PermissionDenied),
            ConnectionError::BindFailed(BindErrorKind::NoInterface),
            ConnectionError::BindFailed(BindErrorKind::Other),
            ConnectionError::AlreadyPaired,
            ConnectionError::BoardUnpaired,
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
