//! Shared output error type (G4) - the selector- and diagnostics-facing
//! error for every output backend.
//!
//! # Why the trait still returns [`VJoyError`](crate::vjoy::VJoyError)
//!
//! The [`IJoystickOutput`](crate::IJoystickOutput) trait's error type is
//! vJoy-flavoured today, and every backend maps its own failures onto
//! it lossily (`DriverNotFound` carrying the full human-readable
//! message). The selector therefore records failure **reasons as
//! strings** (see `SwitchOutcome::fallback_reason` in the
//! [`selector`](crate::selector) module) and never branches on a
//! `VJoyError` variant to identify which backend failed.
//!
//! Until a trait migration happens, [`OutputError`] is the unified
//! type for everything *above* the trait: the backend selector
//! ([`selector`](crate::selector)), switch outcomes, and diagnostics.
//! `From<VJoyError>` is exact (variant-for-variant), so no
//! vJoy-origin information is lost in that conversion either.

use std::fmt;

use crate::vjoy::VJoyError;

/// Everything that can go wrong in any output backend, plus the
/// selector-level failures.
///
/// Display strings are the human-readable reason (the C# `out string
/// error`): the Output page shows them verbatim, so they name the fix
/// ("install vJoy ...", "install the `ViGEmBus` driver ..."), never just a code.
/// Every variant mirrors [`VJoyError`] exactly (same wording), so
/// a future trait migration maps variant-for-variant with no
/// message change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutputError {
    /// The driver DLL could not be found (registry + `ProgramFiles`
    /// both missed). The message names the download link.
    DriverNotFound(String),
    /// The driver DLL loaded but the driver is not enabled.
    DriverDisabled,
    /// DLL and driver versions do not match each other.
    VersionMismatch {
        /// DLL version as reported (`"2.2.2"` form).
        dll: String,
        /// Driver version as reported.
        driver: String,
    },
    /// DLL and driver agree with each other, but neither is the pinned
    /// release.
    UnsupportedVersion {
        /// Version found (`"2.2.2"` form).
        found: String,
        /// Pinned version.
        pinned: String,
    },
    /// The loaded DLL's SHA-256 is not the pinned hash.
    DllHashMismatch {
        /// Expected hash (lowercase hex).
        expected: String,
        /// Hash actually measured.
        actual: String,
    },
    /// The DLL's hash could not be measured (unreadable file, …).
    DllHashUnavailable(String),
    /// A SHA-256 hash string is malformed (not 64 hex digits).
    InvalidHex(String),
    /// The device is owned by another application.
    DeviceBusy(u32),
    /// The device is not configured in the driver control panel.
    DeviceMissing(u32),
    /// The device status could not be determined.
    DeviceUnknown(u32),
    /// Acquiring a free device was refused.
    AcquireFailed(u32),
    /// Updating a connected device was refused.
    UpdateFailed(u32),
    /// The device cannot serve the configured plan (D11). The message
    /// names the missing capabilities; the device was relinquished.
    InsufficientCapabilities(String),
    /// An update was attempted while not connected.
    NotConnected,
    /// Device id outside the valid range (vJoy: 1–16).
    InvalidDeviceId(u32),
    /// D9: a single button number outside 1–32. Never clamped.
    ButtonOutOfRange(u8),
    /// The driver DLL could not be loaded or an export is missing.
    LoadFailed(String),
    /// Any other I/O failure talking to a backend.
    Io(String),
    /// The requested backend has no factory (not installed / not
    /// available on this machine). The message names the backend.
    BackendNotAvailable(String),
    /// A backend switch failed (and any fallback failed too). The
    /// message names every attempt, in order; the previous backend is
    /// left exactly as it was — never half-connected.
    SwitchFailed(String),
}

impl fmt::Display for OutputError {
    // One arm per variant (20 variants) is longer than clippy's
    // 100-line default for a function; splitting the Display impl
    // would only hide the mapping, so the lint is allowed here.
    #[allow(clippy::too_many_lines)]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DriverNotFound(msg)
            | Self::InsufficientCapabilities(msg)
            | Self::BackendNotAvailable(msg)
            | Self::SwitchFailed(msg) => write!(f, "{msg}"),
            Self::DriverDisabled => write!(
                f,
                "vJoy driver is not enabled. Install vJoy (BrunnerInnovation 2.2.2.0) first."
            ),
            Self::VersionMismatch { dll, driver } => write!(
                f,
                "vJoy DLL (v{dll}) and driver (v{driver}) versions do not match. Install both from the same release."
            ),
            Self::UnsupportedVersion { found, pinned } => write!(
                f,
                "vJoy version v{found} is not the supported release (v{pinned}). Install vJoy 2.2.2.0."
            ),
            Self::DllHashMismatch { expected, actual } => write!(
                f,
                "vJoyInterface.dll SHA-256 mismatch: expected {expected}, measured {actual}. Refusing an untrusted DLL."
            ),
            Self::DllHashUnavailable(msg) => {
                write!(f, "Could not verify vJoyInterface.dll SHA-256: {msg}")
            }
            Self::InvalidHex(hex) => {
                write!(f, "Invalid SHA-256 hash (expected 64 hex digits): {hex}")
            }
            Self::DeviceBusy(id) => write!(
                f,
                "vJoy device {id} is owned by another application (BUSY). Close that application and retry."
            ),
            Self::DeviceMissing(id) => write!(
                f,
                "vJoy device {id} does not exist. Enable device {id} in vJoyConf (X, Y, Z + Slider axes)."
            ),
            Self::DeviceUnknown(id) => {
                write!(
                    f,
                    "vJoy device {id} status could not be determined (Unknown)."
                )
            }
            Self::AcquireFailed(id) => write!(f, "vJoy device {id} could not be acquired."),
            Self::UpdateFailed(id) => write!(
                f,
                "UpdateVJD failed for vJoy device {id} — the device appears to have been lost."
            ),
            Self::NotConnected => write!(f, "Output backend is not connected."),
            Self::InvalidDeviceId(id) => {
                write!(
                    f,
                    "Output device id {id} is invalid (valid range is 1..=16)."
                )
            }
            Self::ButtonOutOfRange(button) => write!(
                f,
                "Button {button} is out of range (buttons are 1–32; higher buttons are rejected, never clamped)."
            ),
            Self::LoadFailed(msg) => write!(f, "Could not load vJoyInterface.dll: {msg}"),
            Self::Io(msg) => write!(f, "Output backend I/O error: {msg}"),
        }
    }
}

impl std::error::Error for OutputError {}

/// Exact conversion from the vJoy backend's error type: every variant
/// maps 1:1, so no vJoy-origin information is lost above the trait.
impl From<VJoyError> for OutputError {
    fn from(err: VJoyError) -> Self {
        match err {
            VJoyError::DriverNotFound(msg) => Self::DriverNotFound(msg),
            VJoyError::DriverDisabled => Self::DriverDisabled,
            VJoyError::VersionMismatch { dll, driver } => Self::VersionMismatch { dll, driver },
            VJoyError::UnsupportedVersion { found, pinned } => {
                Self::UnsupportedVersion { found, pinned }
            }
            VJoyError::DllHashMismatch { expected, actual } => {
                Self::DllHashMismatch { expected, actual }
            }
            VJoyError::DllHashUnavailable(msg) => Self::DllHashUnavailable(msg),
            VJoyError::InvalidHex(hex) => Self::InvalidHex(hex),
            VJoyError::DeviceBusy(id) => Self::DeviceBusy(id),
            VJoyError::DeviceMissing(id) => Self::DeviceMissing(id),
            VJoyError::DeviceUnknown(id) => Self::DeviceUnknown(id),
            VJoyError::AcquireFailed(id) => Self::AcquireFailed(id),
            VJoyError::UpdateFailed(id) => Self::UpdateFailed(id),
            VJoyError::InsufficientCapabilities(msg) => Self::InsufficientCapabilities(msg),
            VJoyError::NotConnected => Self::NotConnected,
            VJoyError::InvalidDeviceId(id) => Self::InvalidDeviceId(id),
            VJoyError::ButtonOutOfRange(button) => Self::ButtonOutOfRange(button),
            VJoyError::LoadFailed(msg) => Self::LoadFailed(msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vjoy_error_converts_exactly_and_keeps_its_message() {
        let err = OutputError::from(VJoyError::DeviceBusy(1));
        assert_eq!(err, OutputError::DeviceBusy(1));
        assert!(err.to_string().contains("owned by another application"));

        let err = OutputError::from(VJoyError::DriverNotFound("vJoy not found".to_string()));
        assert_eq!(
            err,
            OutputError::DriverNotFound("vJoy not found".to_string())
        );
        // A backend's lossy mapping rides on this variant: the
        // payload is displayed verbatim, so the real reason survives.
        assert_eq!(err.to_string(), "vJoy not found");

        let err = OutputError::from(VJoyError::VersionMismatch {
            dll: "2.2.2".to_string(),
            driver: "2.1.9".to_string(),
        });
        assert_eq!(
            err,
            OutputError::VersionMismatch {
                dll: "2.2.2".to_string(),
                driver: "2.1.9".to_string(),
            }
        );
    }
}
