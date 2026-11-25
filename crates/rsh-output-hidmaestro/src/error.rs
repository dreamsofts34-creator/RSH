//! Errors for the `HIDMaestro` backend.
//!
//! Display strings are the human-readable reason (the C# `out string
//! error`): the Output page shows them verbatim, so they name the fix
//! ("run as administrator …", "install the driver …"), never just a code.

use std::fmt;
use std::path::PathBuf;

use rsh_output::vjoy::VJoyError;

/// Everything that can go wrong in the `HIDMaestro` backend.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HidMaestroError {
    /// The sidecar executable could not be found at the resolved path.
    SidecarNotFound(PathBuf),
    /// The sidecar process could not be spawned.
    SpawnFailed(String),
    /// No `hello` line arrived within the spawn timeout.
    HelloTimeout,
    /// The `hello` line was malformed or failed validation.
    BadHello(String),
    /// The sidecar's `HIDMaestro.Core.dll` hash is not the pinned hash.
    DllHashMismatch {
        /// Expected hash ([`crate::PINNED_DLL_SHA256`]).
        expected: String,
        /// Hash the sidecar measured.
        actual: String,
    },
    /// The sidecar's SDK version is not the pinned release.
    VersionMismatch {
        /// Version the sidecar reported.
        found: String,
        /// Pinned version ([`crate::PINNED_VERSION`]).
        pinned: String,
    },
    /// The operation needs administrator rights and the sidecar is not
    /// elevated (UAC declined, or RSH was launched non-elevated).
    NotAdmin,
    /// The `HIDMaestro` driver is not installed and could not be installed.
    DriverNotInstalled(String),
    /// `InstallDriver()` failed.
    DriverInstallFailed(String),
    /// The sidecar is alive but its `HIDMaestro` context could not be
    /// created (P1, v2.0.11 — the sidecar reports
    /// `context-unavailable` when `new HMContext()` throws; a native
    /// crash there instead surfaces as [`SidecarDied`](Self::SidecarDied)).
    /// Distinct from [`DriverInstallFailed`](Self::DriverInstallFailed):
    /// the driver install was never attempted — the context itself is
    /// unavailable on this PC. The sidecar log
    /// (`%PROGRAMDATA%\RSH\logs\rsh-sidecar-YYYYMMDD.log`) names the
    /// failure.
    ContextUnavailable(String),
    /// The requested profile was refused (brand catalog profile, usbip
    /// composite, own-reports, or not deployable — SEC-H-06).
    ProfileRefused(String),
    /// `CreateController` failed.
    CreateFailed(String),
    /// The sidecar died (EOF / crash / heartbeat timeout) mid-session.
    SidecarDied(String),
    /// A sidecar reply did not arrive within the reply timeout. The
    /// payload is the enriched diagnostic (HID path sweep, v2.0.12):
    /// which bound was hit, the spawn PID, the sidecar exit code
    /// (`STILL_ACTIVE` = hung, not dead), and the ms since the last
    /// reply — the bare unit variant could not tell a 120 s
    /// install/create timeout (hung driver install, sidecar alive)
    /// from a crashed sidecar, and the daemon log + Output page
    /// showed only "stopped answering" either way.
    ReplyTimeout(String),
    /// A sidecar reply was not valid JSON or failed validation.
    BadReply(String),
    /// `update` called while not connected.
    NotConnected,
    /// A submit was refused by the sidecar / driver.
    UpdateFailed(String),
    /// D9: a single button number outside 1–32. Never clamped.
    InvalidButton(u8),
    /// The sidecar already failed once and the one retry was used up —
    /// fall back to vJoy (G3). Cleared by an explicit user re-select
    /// ([`crate::SidecarManager::reset_fallback`]).
    FallbackNeeded,
    /// This backend only runs on Windows (sidecar + driver).
    UnsupportedPlatform,
    /// Any other I/O failure talking to the sidecar.
    Io(String),
}

impl fmt::Display for HidMaestroError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SidecarNotFound(path) => write!(
                f,
                "HIDMaestro sidecar not found at {}. Reinstall RSH or switch the output backend to vJoy.",
                path.display()
            ),
            Self::SpawnFailed(msg) => write!(f, "Could not start the HIDMaestro sidecar: {msg}"),
            Self::HelloTimeout => write!(
                f,
                "The HIDMaestro sidecar did not answer in time. Try again or switch the output backend to vJoy."
            ),
            Self::BadHello(msg) => write!(f, "The HIDMaestro sidecar sent an invalid hello: {msg}"),
            Self::DllHashMismatch { expected, actual } => write!(
                f,
                "HIDMaestro.Core.dll SHA-256 mismatch: expected {expected}, measured {actual}. Refusing an untrusted DLL."
            ),
            Self::VersionMismatch { found, pinned } => write!(
                f,
                "HIDMaestro SDK version v{found} is not the supported release (v{pinned}). Reinstall RSH."
            ),
            Self::NotAdmin => write!(
                f,
                "HIDMaestro needs administrator rights: approve the UAC prompt when RSH starts (the elevated helper runs for the whole session), or run RSH as administrator. vJoy needs no prompt."
            ),
            Self::DriverNotInstalled(msg) => write!(f, "HIDMaestro driver is not installed: {msg}"),
            Self::DriverInstallFailed(msg) => {
                write!(f, "HIDMaestro driver install failed: {msg}")
            }
            Self::ContextUnavailable(msg) => write!(
                f,
                "HIDMaestro context is unavailable: {msg} — the sidecar is running but HIDMaestro could not start on this PC. Check the sidecar log (%PROGRAMDATA%\\RSH\\logs\\rsh-sidecar-YYYYMMDD.log) or switch the output backend to vJoy."
            ),
            Self::ProfileRefused(msg) => write!(f, "{msg}"),
            Self::CreateFailed(msg) => {
                write!(f, "HIDMaestro device could not be created: {msg}")
            }
            Self::SidecarDied(msg) => write!(
                f,
                "The HIDMaestro sidecar stopped unexpectedly ({msg}). Falling back to vJoy."
            ),
            Self::ReplyTimeout(detail) => write!(
                f,
                "The HIDMaestro sidecar stopped answering: {detail}. Falling back to vJoy."
            ),
            Self::BadReply(msg) => write!(f, "The HIDMaestro sidecar sent an invalid reply: {msg}"),
            Self::NotConnected => write!(f, "HIDMaestro is not connected."),
            Self::UpdateFailed(msg) => write!(f, "HIDMaestro submit failed: {msg}"),
            Self::InvalidButton(button) => write!(
                f,
                "HIDMaestro button {button} is out of range (buttons are 1–32; higher buttons are rejected, never clamped)."
            ),
            Self::FallbackNeeded => write!(
                f,
                "HIDMaestro already failed once and the retry was used up — falling back to vJoy. Re-select HIDMaestro in Output settings to try again."
            ),
            Self::UnsupportedPlatform => write!(
                f,
                "HIDMaestro only runs on Windows (sidecar + driver). Use the vJoy backend on this machine."
            ),
            Self::Io(msg) => write!(f, "HIDMaestro sidecar I/O error: {msg}"),
        }
    }
}

impl std::error::Error for HidMaestroError {}

impl From<std::io::Error> for HidMaestroError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

/// Map a backend error onto the [`IJoystickOutput`](rsh_output::IJoystickOutput)
/// trait's error type.
///
/// Known wart: the trait's error type is vJoy-flavoured
/// ([`VJoyError`]) — there is no shared `OutputError` yet (G0 owns the
/// trait; a shared error type is a G4 follow-up). The mapping keeps the
/// full human-readable message: [`VJoyError::DriverNotFound`] displays
/// its payload verbatim, so whatever the engine logs or the UI shows
/// from the trait error still names the real `HIDMaestro` reason. The
/// authoritative reason always lives in
/// [`HidMaestroStatus`](crate::HidMaestroStatus) / the backend's
/// `last_error()`, which the Output page (G4) reads — never rely on the
/// trait error's variant for `HIDMaestro` diagnostics.
impl From<HidMaestroError> for VJoyError {
    fn from(err: HidMaestroError) -> Self {
        match err {
            HidMaestroError::NotConnected => VJoyError::NotConnected,
            other => VJoyError::DriverNotFound(other.to_string()),
        }
    }
}
