//! Shared output error type (G4) — the selector- and diagnostics-facing
//! error for every output backend.
//!
//! # Why the trait still returns [`VJoyError`](crate::vjoy::VJoyError)
//!
//! The [`IJoystickOutput`](crate::IJoystickOutput) trait's error type is
//! vJoy-flavoured today, and the `HIDMaestro` backend maps its rich
//! `HidMaestroError` (in `rsh-output-hidmaestro`) onto it
//! lossily (`DriverNotFound` carrying the full human-readable message).
//! That split stays for now, deliberately, and this type does **not**
//! pretend to have fixed it:
//!
//! * Changing the trait's error type would require coordinated edits to
//!   both backends' trait implementations — `vjoy.rs` (this crate) and
//!   `rsh-output-hidmaestro`'s `backend.rs` / `error.rs` — which are
//!   outside the G4 file ownership. A half-migration (trait changed,
//!   one backend left behind) would not compile at all.
//! * A `From<HidMaestroError> for OutputError` impl cannot live in this
//!   crate: `rsh-output-hidmaestro` depends on `rsh-output`, so this
//!   crate cannot depend on it back (circular dependency). It cannot
//!   live in `rsh-app-core` either (orphan rule — neither type is local
//!   there). It belongs in `rsh-output-hidmaestro`'s `error.rs`, next to
//!   the existing `From<HidMaestroError> for VJoyError`, as the first
//!   step of the trait migration below.
//! * Nothing diagnostic is lost at the UI layer in the meantime: the
//!   lossy mapping preserves the full human-readable message verbatim
//!   (`VJoyError::DriverNotFound` displays its payload as-is), and the
//!   authoritative rich error always lives in
//!   `HidMaestroOutput::last_error()`
//!   / `HidMaestroStatus`,
//!   which the selector's diagnostics read — never the trait error's
//!   variant. The selector therefore records failure **reasons as
//!   strings** (see `SwitchOutcome::fallback_reason` in the
//!   [`selector`](crate::selector) module) and never branches on a
//!   never branches on a `VJoyError` variant to identify a
//!   `HIDMaestro` failure.
//!
//! # Migration path (follow-up, not G4)
//!
//! 1. Add `impl From<HidMaestroError> for OutputError` in
//!    `rsh-output-hidmaestro/src/error.rs` (variant-for-variant, using
//!    the `HIDMaestro`-origin variants below).
//! 2. Change the [`IJoystickOutput`](crate::IJoystickOutput) trait's
//!    methods to return `Result<_, OutputError>` and update both
//!    backends' implementations (`vjoy.rs` maps via the
//!    [`From<VJoyError>`] impl below; the `HIDMaestro` backend maps via
//!    step 1).
//! 3. Delete the lossy `From<HidMaestroError> for VJoyError` impl and
//!    the `VJoyError` import from the `HIDMaestro` backend.
//!
//! Until then, [`OutputError`] is the unified type for everything
//! *above* the trait: the backend selector
//! ([`selector`](crate::selector)), switch outcomes, and diagnostics.
//! `From<VJoyError>` is exact (variant-for-variant), so no
//! vJoy-origin information is lost in that conversion either.

use std::fmt;
use std::path::PathBuf;

use crate::vjoy::VJoyError;

/// Everything that can go wrong in any output backend, plus the
/// selector-level failures.
///
/// Display strings are the human-readable reason (the C# `out string
/// error`): the Output page shows them verbatim, so they name the fix
/// ("install vJoy …", "approve the UAC prompt …"), never just a code.
/// The vJoy-origin variants mirror [`VJoyError`] exactly; the
/// `HIDMaestro`-origin variants mirror
/// `HidMaestroError` (in `rsh-output-hidmaestro`) exactly
/// (same wording), so a future trait migration maps variant-for-variant
/// with no message change.
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
    /// The `HIDMaestro` sidecar executable could not be found at the
    /// resolved path.
    SidecarNotFound(PathBuf),
    /// The sidecar process could not be spawned.
    SidecarSpawnFailed(String),
    /// No `hello` line arrived within the spawn timeout.
    SidecarHelloTimeout,
    /// The `hello` line was malformed or failed validation.
    SidecarBadHello(String),
    /// The sidecar died (EOF / crash / heartbeat timeout) mid-session.
    SidecarDied(String),
    /// A sidecar reply did not arrive within the reply timeout.
    SidecarReplyTimeout,
    /// A sidecar reply was not valid JSON or failed validation.
    SidecarBadReply(String),
    /// The operation needs administrator rights and the sidecar is not
    /// elevated (UAC declined, or RSH was launched non-elevated).
    NotAdmin,
    /// The `HIDMaestro` driver is not installed and could not be
    /// installed.
    DriverNotInstalled(String),
    /// `InstallDriver()` failed.
    DriverInstallFailed(String),
    /// The requested profile was refused (brand catalog profile, usbip
    /// composite, own-reports, or not deployable — SEC-H-06).
    ProfileRefused(String),
    /// `CreateController` failed.
    CreateFailed(String),
    /// The sidecar already failed once and the one retry was used up —
    /// fall back to vJoy (G3). Cleared by an explicit user re-select.
    FallbackNeeded,
    /// This backend only runs on Windows (sidecar + driver).
    UnsupportedPlatform,
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

impl OutputError {
    /// True when this error means "the `HIDMaestro` retry is used up —
    /// fall back to vJoy". Only constructible from the rich
    /// `HIDMaestro` error (see the module docs); the lossy trait error
    /// never produces it, so the selector falls back on **any**
    /// `HIDMaestro` connect failure, not only on this variant.
    #[must_use]
    pub const fn fallback_needed(&self) -> bool {
        matches!(self, Self::FallbackNeeded)
    }
}

impl fmt::Display for OutputError {
    // One arm per variant (34 variants) is longer than clippy's
    // 100-line default for a function; splitting the Display impl
    // would only hide the mapping, so the lint is allowed here.
    #[allow(clippy::too_many_lines)]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DriverNotFound(msg)
            | Self::InsufficientCapabilities(msg)
            | Self::ProfileRefused(msg)
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
            Self::SidecarNotFound(path) => write!(
                f,
                "HIDMaestro sidecar not found at {}. Reinstall RSH or switch the output backend to vJoy.",
                path.display()
            ),
            Self::SidecarSpawnFailed(msg) => {
                write!(f, "Could not start the HIDMaestro sidecar: {msg}")
            }
            Self::SidecarHelloTimeout => write!(
                f,
                "The HIDMaestro sidecar did not answer in time. Try again or switch the output backend to vJoy."
            ),
            Self::SidecarBadHello(msg) => {
                write!(f, "The HIDMaestro sidecar sent an invalid hello: {msg}")
            }
            Self::SidecarDied(msg) => write!(
                f,
                "The HIDMaestro sidecar stopped unexpectedly ({msg}). Falling back to vJoy."
            ),
            Self::SidecarReplyTimeout => write!(
                f,
                "The HIDMaestro sidecar stopped answering. Falling back to vJoy."
            ),
            Self::SidecarBadReply(msg) => {
                write!(f, "The HIDMaestro sidecar sent an invalid reply: {msg}")
            }
            Self::NotAdmin => write!(
                f,
                "HIDMaestro needs administrator rights: approve the UAC prompt when RSH starts (the elevated helper runs for the whole session), or run RSH as administrator. vJoy needs no prompt."
            ),
            Self::DriverNotInstalled(msg) => {
                write!(f, "HIDMaestro driver is not installed: {msg}")
            }
            Self::DriverInstallFailed(msg) => {
                write!(f, "HIDMaestro driver install failed: {msg}")
            }
            Self::CreateFailed(msg) => {
                write!(f, "HIDMaestro device could not be created: {msg}")
            }
            Self::FallbackNeeded => write!(
                f,
                "HIDMaestro already failed once and the retry was used up — falling back to vJoy. Re-select HIDMaestro in Output settings to try again."
            ),
            Self::UnsupportedPlatform => write!(
                f,
                "HIDMaestro only runs on Windows (sidecar + driver). Use the vJoy backend on this machine."
            ),
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
        // The lossy HIDMaestro mapping rides on this variant: the
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

    #[test]
    fn hidmaestro_origin_variants_carry_the_documented_wording() {
        assert!(OutputError::FallbackNeeded.fallback_needed());
        assert!(!OutputError::NotConnected.fallback_needed());
        assert!(
            OutputError::NotAdmin
                .to_string()
                .contains("approve the UAC prompt")
        );
        assert!(
            OutputError::FallbackNeeded
                .to_string()
                .contains("falling back to vJoy")
        );
        assert!(
            OutputError::UnsupportedPlatform
                .to_string()
                .contains("only runs on Windows")
        );
    }
}
