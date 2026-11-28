//! vJoy FFI backend (G3 / SW-R6.4) — the always-present fallback.
//!
//! Hand-written `extern "C"` bindings to `vJoyInterface.dll` via
//! `libloading`. No `vjoy` crate (unverified, single maintainer) and no
//! `bindgen`: the ten functions below are the whole surface, declared
//! exactly as the vJoy SDK header declares them (`__cdecl`; on x86-64
//! Windows there is only one calling convention, but the declaration
//! is explicit). All native calls go through the [`VJoyInterface`]
//! trait, so tests run on Linux against a mock DLL — the real DLL does
//! not exist on Linux, and real-DLL verification is Windows-only (see
//! "Windows-only" below).
//!
//! # Driver pin
//!
//! Driver: `BrunnerInnovation` **v2.2.2.0**. The `DriverMatch` version
//! check can only see the `X.Y.Z` nibbles (`2.2.2` — the fourth
//! component is not visible through the API), so the exact-build pin
//! is the DLL's SHA-256. **No SHA-256 for the 2.2.2.0 DLL is recorded
//! in the plans, the research notes, or the C# sources** — none was
//! ever measured there — so [`PINNED_DLL_SHA256_HEX`] is `None` until
//! someone measures the release-machine DLL (`Get-FileHash
//! vJoyInterface.dll -Algorithm SHA256`) and fills it in. Do NOT
//! invent a hash: [`verify_dll_sha256`] exists so the pin can be
//! enforced the moment a real hash is recorded, and
//! [`VJoyOutput::set_expected_dll_sha256`] refuses malformed hashes.
//!
//! # DLL resolution (D18)
//!
//! The DLL is **never bundled** with the app. Resolution order:
//! registry (`HKLM\...\Uninstall\vJoy`, `InstallLocation`, Windows
//! only) → `%ProgramFiles%\vJoy\x64\...` → `%ProgramFiles(x86)%\vJoy\
//! x64\...`. The registry probe is best-effort: any failure falls
//! through to the `ProgramFiles` candidates, which is the path the C#
//! series used.
//!
//! # Missing-driver journey
//!
//! vJoy not installed → the app still runs, output is disabled, and
//! [`VJoyOutput::status`] reports [`VJoyStateKind::NotInstalled`]
//! with a human-readable message and [`VJOY_DOWNLOAD_URL`]. The
//! Dashboard renders that as "vJoy not found — [Install vJoy] [Open
//! Diagnostics]". [`VJoyOutput::try_connect`] returns an error, never
//! a panic and never a fake success.
//!
//! # D32 keepalive — dropped, deliberately
//!
//! There is no keepalive timer in this backend, and there was none in
//! the C# series either (no keepalive code exists in `RSH.Output.VJoy`).
//! The need D32 worried about — a game dropping an idle device — is
//! already covered by the Phase B engine: the engine ticks at 200 Hz
//! and re-sends every tick, the last state while fresh and
//! [`OutputState::neutral()`] while the 250 ms watchdog is stale, and
//! keeps ticking until stopped. `UpdateVJD` is therefore called every
//! 5 ms while connected, so the device is never idle from the game's
//! point of view, and a separate keepalive would only duplicate those
//! calls. What remains unverified is the *observation*, not the code:
//! the 10-minute idle check in the game-compatibility matrix (Phase F,
//! Windows PC).
//!
//! # Mapping (`ProcessedState` -> `JOYSTICK_POSITION`)
//!
//! Pure function [`build_position`], modelled on the C#
//! `VJoyOutput.BuildPosition` (56-test `RSH.VJoy.Tests` suite):
//! steering -> X (+ V3 `wSteering`), accelerator -> Y (+ V3
//! `wAccelerator`), brake -> Z (+ V3 `wBrake`), clutch -> V3 `wClutch`
//! AND Rx (the feed writes the clutch to Rx as well as to `wClutch`; the
//! Rx write is caps-gated, the `wClutch` write stays caps-independent
//! as before), handbrake -> `wSlider` (HID usage `0x36`, caps-gated),
//! POV/hats always neutral (`0xFFFF_FFFF` - this wheel has no hat
//! input). Buttons 1-32 -> `lButtons`; `lButtonsEx1`/`lButtonsEx2`/
//! `lButtonsEx3` are always 0 (v2.0.19: the buttons/encoders cleanup
//! deleted every source for buttons 33-128). D9: a button above 32 is
//! **never folded or clamped into** `lButtons` - each word maps to its
//! own field; [`check_button_number`] rejects a single button number
//! outside 1-32 for the `lButtons` word.
//!
//! # Windows-only (real-DLL verification, not done here)
//!
//! * Loading the real `vJoyInterface.dll` and calling
//!   `AcquireVJD`/`UpdateVJD`/`RelinquishVJD` against a live driver.
//! * The registry probe (compiled `#[cfg(windows)]` only).
//! * Measuring the 2.2.2.0 DLL's SHA-256 and filling in
//!   [`PINNED_DLL_SHA256_HEX`].
//! * The game-compatibility matrix (Phase F) and the 10-minute idle
//!   check.

use std::env;
use std::fmt;
use std::fmt::Write as _;
use std::fs;
use std::mem::{offset_of, size_of};
use std::path::{Path, PathBuf};

use libloading::{Library, Symbol};
use rsh_engine::pipeline::{OutputState, PipelinePlan};
use sha2::{Digest, Sha256};

use crate::{IJoystickOutput, ProcessedState};

/// File name of the vJoy C interface DLL. Never bundled (D18).
pub const VJOY_DLL_FILE_NAME: &str = "vJoyInterface.dll";
/// Pinned driver version as the API reports it (`DriverMatch` nibbles:
/// major.minor.patch only).
pub const PINNED_DRIVER_VERSION: &str = "2.2.2";
/// Pinned driver release, full four-component form.
pub const PINNED_DRIVER_VERSION_FULL: &str = "2.2.2.0";
/// SHA-256 pin of the pinned release's `vJoyInterface.dll`.
///
/// `None`: no hash is recorded anywhere in the plans, research notes,
/// or C# sources — measure the release-machine DLL and fill this in;
/// never invent a value. While `None`, the version check is
/// `DriverMatch` + the `2.2.2` pin only. Use
/// [`VJoyOutput::set_expected_dll_sha256`] to enforce a hash at
/// runtime once one is measured.
pub const PINNED_DLL_SHA256_HEX: Option<&str> = None;
/// Where to send a user who needs the driver (guide + download, cited
/// in `RESEARCH.md` §4).
pub const VJOY_DOWNLOAD_URL: &str = "https://simux.brunner-innovation.swiss/guide/vjoy/";

/// vJoy axis minimum (C# `InputMapper.AxisMin`).
pub const AXIS_MIN: i32 = 1;
/// vJoy axis maximum (C# `InputMapper.AxisMax`).
pub const AXIS_MAX: i32 = 32768;
/// vJoy axis centre (C# `InputMapper.AxisCenter`).
pub const AXIS_CENTER: i32 = 16384;
/// vJoy device ids are 1-based, at most 16 devices.
pub const MAX_DEVICE_ID: u32 = 16;
/// vJoy button count range is 0–128.
pub const MAX_BUTTONS: i32 = 128;
/// HAT neutral: all four hat fields carry this when no hat is pressed.
pub const HAT_NEUTRAL: u32 = 0xFFFF_FFFF;

/// HID Generic Desktop usages (C# `VJoyOutput.Usage*`, `public.h`).
pub const USAGE_X: u16 = 0x30;
/// HID usage: Y.
pub const USAGE_Y: u16 = 0x31;
/// HID usage: Z.
pub const USAGE_Z: u16 = 0x32;
/// HID usage: Slider (`0x36`) — the handbrake axis (Phase G C8).
pub const USAGE_SLIDER: u16 = 0x36;
/// HID usage: Rx (`0x33`) — envelope axis 4.
pub const USAGE_RX: u16 = 0x33;
/// HID usage: Ry (`0x34`) — envelope axis 5.
pub const USAGE_RY: u16 = 0x34;
/// HID usage: Rz (`0x35`) — envelope axis 6.
pub const USAGE_RZ: u16 = 0x35;
/// HID usage: Dial (`0x37`) — envelope axis 8 (Slider1).
pub const USAGE_DIAL: u16 = 0x37;

// ---------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------

/// Everything that can go wrong in the vJoy backend.
///
/// Display strings are the human-readable reason (the C# `out string
/// error`): the Dashboard shows them verbatim, so they name the fix
/// ("install vJoy …"), never just a code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VJoyError {
    /// The DLL could not be found (registry + `ProgramFiles` both
    /// missed). The message names the download link.
    DriverNotFound(String),
    /// The DLL loaded but `vJoyEnabled()` is false.
    DriverDisabled,
    /// `DriverMatch` says the DLL and driver are from different
    /// releases.
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
        /// Pinned version ([`PINNED_DRIVER_VERSION`]).
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
    /// The device is not configured in vJoyConf.
    DeviceMissing(u32),
    /// `GetVJDStatus` returned something unrecognised.
    DeviceUnknown(u32),
    /// `AcquireVJD` refused a free device.
    AcquireFailed(u32),
    /// `UpdateVJD` refused a connected device.
    UpdateFailed(u32),
    /// The device cannot serve the configured plan (D11). The message
    /// names the missing capabilities; the device was relinquished.
    InsufficientCapabilities(String),
    /// `update` called while not connected.
    NotConnected,
    /// Device id outside 1–16.
    InvalidDeviceId(u32),
    /// D9: a single button number outside 1–32 for the `lButtons`
    /// word. Never clamped, never folded.
    ButtonOutOfRange(u8),
    /// `libloading` could not load the DLL or resolve a symbol.
    LoadFailed(String),
}

impl fmt::Display for VJoyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DriverNotFound(msg) | Self::InsufficientCapabilities(msg) => {
                write!(f, "{msg}")
            }
            Self::DriverDisabled => write!(
                f,
                "vJoy driver is not enabled. Install vJoy (BrunnerInnovation {PINNED_DRIVER_VERSION_FULL}) first."
            ),
            Self::VersionMismatch { dll, driver } => write!(
                f,
                "vJoy DLL (v{dll}) and driver (v{driver}) versions do not match. Install both from the same release."
            ),
            Self::UnsupportedVersion { found, pinned } => write!(
                f,
                "vJoy version v{found} is not the supported release (v{pinned}). Install vJoy {PINNED_DRIVER_VERSION_FULL}."
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
            Self::NotConnected => write!(f, "vJoy is not connected."),
            Self::InvalidDeviceId(id) => {
                write!(
                    f,
                    "vJoy device id {id} is invalid (valid range is 1..={MAX_DEVICE_ID})."
                )
            }
            Self::ButtonOutOfRange(button) => write!(
                f,
                "vJoy button {button} is out of range (buttons are 1–32; higher buttons are rejected, never clamped)."
            ),
            Self::LoadFailed(msg) => write!(f, "Could not load vJoyInterface.dll: {msg}"),
        }
    }
}

impl std::error::Error for VJoyError {}

// ---------------------------------------------------------------------
// Dashboard status
// ---------------------------------------------------------------------

/// The backend's state as the Dashboard shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VJoyStateKind {
    /// Never attempted (or cleanly disconnected).
    Idle,
    /// Connected and sending.
    Ready,
    /// Driver/DLL not found — the "vJoy not found" warning journey.
    NotInstalled,
    /// Driver present but not enabled.
    Disabled,
    /// DLL/driver version mismatch, or not the pinned release.
    VersionMismatch,
    /// Device owned by another application.
    Busy,
    /// Device not configured in vJoyConf.
    Missing,
    /// Any other failure (message carries the detail).
    Error,
}

/// Snapshot for the Dashboard: state + human-readable message +
/// versions + install link when relevant.
///
/// The Dashboard renders `NotInstalled` as "vJoy not found —
/// [Install vJoy] [Open Diagnostics]" (`download_url` is the Install
/// target; Diagnostics opens the app's diagnostics view).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VJoyStatus {
    /// Machine-readable state.
    pub kind: VJoyStateKind,
    /// Human-readable message (shown verbatim).
    pub message: String,
    /// DLL version (`"2.2.2"` form), once read.
    pub dll_version: Option<String>,
    /// Driver version (`"2.2.2"` form), once read.
    pub driver_version: Option<String>,
    /// Install/guide link when the driver is missing or mismatched.
    pub download_url: Option<&'static str>,
}

impl VJoyStatus {
    /// Idle: never attempted or cleanly disconnected.
    #[must_use]
    pub fn idle() -> Self {
        Self {
            kind: VJoyStateKind::Idle,
            message: "vJoy backend idle — not connected yet.".to_string(),
            dll_version: None,
            driver_version: None,
            download_url: None,
        }
    }

    /// The missing-driver warning state.
    #[must_use]
    pub fn not_installed() -> Self {
        Self {
            kind: VJoyStateKind::NotInstalled,
            message: format!(
                "vJoy not found — install vJoy (BrunnerInnovation {PINNED_DRIVER_VERSION_FULL}) to enable wheel output."
            ),
            dll_version: None,
            driver_version: None,
            download_url: Some(VJOY_DOWNLOAD_URL),
        }
    }
}

// ---------------------------------------------------------------------
// Device status (vJoyInterface.h `VjdStat`)
// ---------------------------------------------------------------------

/// `GetVJDStatus` result (values from the SDK header).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VjdStatus {
    /// Owned by us already.
    Own = 0,
    /// Free, can be acquired.
    Free = 1,
    /// Owned by another feeder.
    Busy = 2,
    /// Not configured in vJoyConf.
    Missing = 3,
    /// Unrecognised / unknown.
    #[default]
    Unknown = 4,
}

impl VjdStatus {
    /// Map a raw `VjdStat` integer; anything unrecognised is
    /// [`VjdStatus::Unknown`].
    #[must_use]
    pub const fn from_raw(value: i32) -> Self {
        match value {
            0 => Self::Own,
            1 => Self::Free,
            2 => Self::Busy,
            3 => Self::Missing,
            _ => Self::Unknown,
        }
    }
}

// ---------------------------------------------------------------------
// JOYSTICK_POSITION (API V3, inc/public.h — byte-for-byte)
// ---------------------------------------------------------------------

/// `JOYSTICK_POSITION` — `BrunnerInnovation` vJoy 2.2.2.0, API V3 layout
/// (`wAccelerator`/`wBrake`/`wClutch`/`wSteering` mid-struct,
/// `wAxisVZ`/`VBR*` at the tail). Total size **124 bytes**.
///
/// The fields this backend uses (X/Y/Z axes, buttons, hats) sit at
/// the same offsets in V2 and V3, but the struct is V3-sized so the
/// driver always reads a full V3 position. Layout is asserted at
/// compile time below — a mismatch fails the build, not a game night.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JoystickPosition {
    /// 1-based device index.
    pub b_device: u8,
    /// Throttle (unused by RSH, centre).
    pub w_throttle: i32,
    /// Rudder (unused, centre).
    pub w_rudder: i32,
    /// Aileron (unused, centre).
    pub w_aileron: i32,
    /// X axis — steering.
    pub w_axis_x: i32,
    /// Y axis — accelerator.
    pub w_axis_y: i32,
    /// Z axis — brake.
    pub w_axis_z: i32,
    /// X rotation (unused, centre).
    pub w_axis_x_rot: i32,
    /// Y rotation (unused, centre).
    pub w_axis_y_rot: i32,
    /// Z rotation (unused, centre).
    pub w_axis_z_rot: i32,
    /// Slider — handbrake (HID usage `0x36`).
    pub w_slider: i32,
    /// Dial (unused, centre).
    pub w_dial: i32,
    /// Wheel (unused, centre).
    pub w_wheel: i32,
    /// V3 driving axis — accelerator mirror.
    pub w_accelerator: i32,
    /// V3 driving axis — brake mirror.
    pub w_brake: i32,
    /// V3 driving axis — clutch.
    pub w_clutch: i32,
    /// V3 driving axis — steering mirror.
    pub w_steering: i32,
    /// X velocity (unused, centre).
    pub w_axis_vx: i32,
    /// Y velocity (unused, centre).
    pub w_axis_vy: i32,
    /// Buttons 1–32 (bit 0 = button 1).
    pub l_buttons: i32,
    /// POV hat 1 (neutral).
    pub b_hats: u32,
    /// POV hat 2 (neutral).
    pub b_hats_ex1: u32,
    /// POV hat 3 (neutral).
    pub b_hats_ex2: u32,
    /// POV hat 4 (neutral).
    pub b_hats_ex3: u32,
    /// Buttons 33–64.
    pub l_buttons_ex1: i32,
    /// Buttons 65–96.
    pub l_buttons_ex2: i32,
    /// Buttons 97–128 (always 0: no wire source).
    pub l_buttons_ex3: i32,
    /// Z velocity (unused, centre).
    pub w_axis_vz: i32,
    /// X rotation velocity (unused, centre).
    pub w_axis_vbrx: i32,
    /// Y rotation velocity (unused, centre).
    pub w_axis_vbry: i32,
    /// Z rotation velocity (unused, centre).
    pub w_axis_vbrz: i32,
}

// Compile-time layout asserts (C# `VJoyLayoutTests` size/offset
// checks, moved to the compiler: a drift fails the build).
const _: () = assert!(size_of::<JoystickPosition>() == 124);
const _: () = assert!(offset_of!(JoystickPosition, b_device) == 0);
const _: () = assert!(offset_of!(JoystickPosition, w_throttle) == 4);
const _: () = assert!(offset_of!(JoystickPosition, w_axis_x) == 16);
const _: () = assert!(offset_of!(JoystickPosition, w_axis_y) == 20);
const _: () = assert!(offset_of!(JoystickPosition, w_axis_z) == 24);
const _: () = assert!(offset_of!(JoystickPosition, w_slider) == 40);
const _: () = assert!(offset_of!(JoystickPosition, w_accelerator) == 52);
const _: () = assert!(offset_of!(JoystickPosition, w_brake) == 56);
const _: () = assert!(offset_of!(JoystickPosition, w_clutch) == 60);
const _: () = assert!(offset_of!(JoystickPosition, w_steering) == 64);
const _: () = assert!(offset_of!(JoystickPosition, l_buttons) == 76);
const _: () = assert!(offset_of!(JoystickPosition, b_hats) == 80);
const _: () = assert!(offset_of!(JoystickPosition, l_buttons_ex1) == 96);
const _: () = assert!(offset_of!(JoystickPosition, l_buttons_ex2) == 100);
const _: () = assert!(offset_of!(JoystickPosition, l_buttons_ex3) == 104);
const _: () = assert!(offset_of!(JoystickPosition, w_axis_vz) == 108);
const _: () = assert!(offset_of!(JoystickPosition, w_axis_vbrz) == 120);

// ---------------------------------------------------------------------
// Capabilities (D11)
// ---------------------------------------------------------------------

/// The envelope shape of a vJoy device (v2.0.14): the four axes [`VJoyCapabilities`] never measured
/// (Rx/Ry/Rz/Dial) and the POV counts (discrete 1, continuous 0 —
/// `-s 1` is the discrete flag, `-p` would be continuous;
/// verified against the upstream vJoyConfig source).
///
/// A device without the envelope still feeds (see
/// [`validate_envelope`]) — these fields only *report* the shape,
/// they never gate feeding. FFB is deliberately NOT here: it is a
/// input works without it (Forza does not send FFB to vJoy).
// Four bools because the envelope IS four axes (Rx/Ry/Rz/Dial);
// grouping them into a bitmask would hide which axis is missing
// from the report, so the pedantic bool-count lint is allowed here.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EnvelopeCaps {
    /// Rx axis present (envelope axis 4). Also the clutch axis:
    /// the feed writes the clutch here (see [`build_position`]).
    pub has_axis_rx: bool,
    /// Ry axis present (envelope axis 5).
    pub has_axis_ry: bool,
    /// Rz axis present (envelope axis 6).
    pub has_axis_rz: bool,
    /// Dial axis present (envelope axis 8, Slider1).
    pub has_axis_dial: bool,
    /// Discrete (4-direction) POV hat count as reported;
    /// `-1` = not read.
    pub discrete_pov_count: i32,
    /// Continuous POV hat count as reported;
    /// `-1` = not read.
    pub continuous_pov_count: i32,
}

impl EnvelopeCaps {
    /// Envelope shape not read yet: extra axes assumed present,
    /// POV counts unknown. [`validate_envelope`] only reports
    /// what was actually measured, so unknown fields never produce
    /// a false "missing" report.
    #[must_use]
    pub const fn unknown() -> Self {
        Self {
            has_axis_rx: true,
            has_axis_ry: true,
            has_axis_rz: true,
            has_axis_dial: true,
            discrete_pov_count: -1,
            continuous_pov_count: -1,
        }
    }
}

/// A vJoy device's capabilities, read by `TryConnect` from the driver.
///
/// `Unknown` means "not read yet" — old behaviour: write everything
/// (no gating). The real device always has caps read at connect, so
/// `Unknown` only appears in pure-mapping tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// Four independent device flags, one per driver query — a bitfield
// would obscure the 1:1 mapping with `GetVJDAxisExist` / button count.
#[allow(clippy::struct_excessive_bools)]
pub struct VJoyCapabilities {
    /// X axis present (steering).
    pub has_axis_x: bool,
    /// Y axis present (accelerator).
    pub has_axis_y: bool,
    /// Z axis present (brake).
    pub has_axis_z: bool,
    /// Button count as the driver reports it (0–128).
    pub button_count: i32,
    /// Slider axis present (handbrake, HID usage `0x36`).
    pub has_axis_slider: bool,
    /// `EmuWheel` envelope shape (see [`EnvelopeCaps`]).
    /// [`EnvelopeCaps::unknown`] unless the caps were built with
    /// [`Self::with_envelope`] — [`Self::new`] predates the
    /// envelope read and cannot measure it.
    pub envelope: EnvelopeCaps,
}

impl VJoyCapabilities {
    /// Capabilities for a known device.
    #[must_use]
    // Positional constructor mirrors the C# `VJoyCapabilities` record;
    // call sites always pass literals in the documented order.
    #[allow(clippy::fn_params_excessive_bools)]
    pub const fn new(
        has_axis_x: bool,
        has_axis_y: bool,
        has_axis_z: bool,
        button_count: i32,
        has_axis_slider: bool,
    ) -> Self {
        Self {
            has_axis_x,
            has_axis_y,
            has_axis_z,
            button_count,
            has_axis_slider,
            envelope: EnvelopeCaps::unknown(),
        }
    }

    /// Attach a measured envelope shape to caps built with
    /// [`Self::new`] (the connect path reads the envelope after
    /// the five classic caps).
    #[must_use]
    pub const fn with_envelope(self, envelope: EnvelopeCaps) -> Self {
        Self { envelope, ..self }
    }

    /// Caps not read yet: assume a full device (no gating).
    #[must_use]
    pub const fn unknown() -> Self {
        Self::new(true, true, true, MAX_BUTTONS, true)
    }

    /// Mask for vJoy buttons 1–32 (`lButtons`).
    #[must_use]
    pub fn button_mask(&self) -> u32 {
        block_mask(self.button_count, 1)
    }
}

/// Bitmask for the 32-button block starting at 1-based `first_button`
/// (1, 33, 65 or 97): only the buttons the device actually has pass.
fn block_mask(button_count: i32, first_button: i32) -> u32 {
    let available = button_count - first_button + 1;
    if available <= 0 {
        0
    } else if available >= 32 {
        u32::MAX
    } else {
        // `available` is 1..=31 here, so the conversion cannot fail.
        (1u32 << u32::try_from(available).unwrap_or(0)) - 1
    }
}

/// Validate a device's capabilities against the configured modules
/// (D11, C# `VJoyOutput.ValidateCapabilities`).
///
/// Only *enabled* modules impose requirements: a steering-only wheel
/// is valid with just X, a disabled module imposes nothing, and there
/// is no fixed "X+Y+Z and ≥2 buttons" rule. Clutch imposes no extra
/// requirement (its `wClutch` field has no HID usage to query).
/// Returns the list of problems; empty means valid.
#[must_use]
pub fn validate_capabilities(plan: &PipelinePlan, caps: &VJoyCapabilities) -> Vec<String> {
    let mut errors = Vec::new();
    if !(0..=MAX_BUTTONS).contains(&caps.button_count) {
        errors.push(format!(
            "ButtonCount {} is out of range (0..={MAX_BUTTONS}).",
            caps.button_count
        ));
    }
    if plan.steering.enabled && !caps.has_axis_x {
        errors.push("Steering module is on but the vJoy device has no X axis.".to_string());
    }
    if plan.accelerator.enabled && !caps.has_axis_y {
        errors.push("Accelerator module is on but the vJoy device has no Y axis.".to_string());
    }
    if plan.brake.enabled && !caps.has_axis_z {
        errors.push("Brake module is on but the vJoy device has no Z axis.".to_string());
    }
    if plan.handbrake.enabled && !caps.has_axis_slider {
        errors.push("Handbrake module is on but the vJoy device has no Slider axis.".to_string());
    }
    if plan.shifter.enabled && caps.button_count < 2 {
        errors.push(
            "Shifter module is on but the vJoy device has fewer than 2 buttons (shifter is buttons 1-2)."
                .to_string(),
        );
    }
    if plan.horn.enabled && caps.button_count < 5 {
        errors.push(
            "Horn module is on but the vJoy device has fewer than 5 buttons (horn is button 5)."
                .to_string(),
        );
    }
    errors
}

/// Validate a device's shape against the vJoy envelope (v2.0.14): 128 buttons, all 8 axes, exactly one
/// discrete POV hat and no continuous POV.
///
/// REPORTS ONLY — it never refuses a connect. A default-shape
/// vJoy device (32 buttons, no FFB) connects and feeds silently
/// today (ETS2 and friends depend on it), and that stays true:
/// the caller stores the returned problems (see
/// [`VJoyOutput::envelope_warnings`]); the envelope warnings
/// are surfaced in diagnostics — it does not fail `try_connect`. The old module docs claimed
/// the connect-time caps read "is the verification" for the
/// envelope; that was false (the caps read never checked buttons
/// beyond range, Rx/Ry/Rz/Dial, POVs, or FFB) — this function is
/// the envelope verification. Unmeasured fields never report: a POV count of `-1`
/// (not read) and extra axes from [`VJoyCapabilities::new`]
/// (assumed present — the pre-envelope constructor cannot measure
/// them) are skipped. FFB is deliberately NOT checked here: it is
/// not checked here: input works without it (Forza does not send FFB to vJoy).
/// Returns the list of problems; empty means the envelope matches
/// (or nothing measurable was missing).
#[must_use]
pub fn validate_envelope(caps: &VJoyCapabilities) -> Vec<String> {
    let mut errors = Vec::new();
    if caps.button_count != MAX_BUTTONS {
        errors.push(format!(
            "vJoy device has {} buttons, expected {MAX_BUTTONS} (EmuWheel envelope).",
            caps.button_count
        ));
    }
    let envelope = &caps.envelope;
    if !envelope.has_axis_rx {
        errors.push(
            "vJoy device has no Rx axis (EmuWheel envelope needs X Y Z Rx Ry Rz Slider Dial)."
                .to_string(),
        );
    }
    if !envelope.has_axis_ry {
        errors.push(
            "vJoy device has no Ry axis (EmuWheel envelope needs X Y Z Rx Ry Rz Slider Dial)."
                .to_string(),
        );
    }
    if !envelope.has_axis_rz {
        errors.push(
            "vJoy device has no Rz axis (EmuWheel envelope needs X Y Z Rx Ry Rz Slider Dial)."
                .to_string(),
        );
    }
    if !envelope.has_axis_dial {
        errors.push(
            "vJoy device has no Dial axis (EmuWheel envelope needs X Y Z Rx Ry Rz Slider Dial)."
                .to_string(),
        );
    }
    if envelope.discrete_pov_count >= 0 && envelope.discrete_pov_count != 1 {
        errors.push(format!(
            "vJoy device has {} discrete POV hats, expected 1 (EmuWheel envelope).",
            envelope.discrete_pov_count
        ));
    }
    if envelope.continuous_pov_count > 0 {
        errors.push(format!(
            "vJoy device has {} continuous POV hat(s), expected 0 (the envelope uses one discrete POV).",
            envelope.continuous_pov_count
        ));
    }
    errors
}

// ---------------------------------------------------------------------
// Axis conversion (C# `InputMapper` parity)
// ---------------------------------------------------------------------

/// Banker's rounding (C# `Math.Round` default, `MidpointRounding.ToEven`).
/// Exact halves go to the even neighbour: 16383.5 → 16384, 0.5 → 0.
fn round_half_even(value: f32) -> f32 {
    let floor = value.floor();
    let diff = value - floor;
    if diff < 0.5 {
        floor
    } else if diff > 0.5 {
        floor + 1.0
    } else if floor % 2.0 == 0.0 {
        floor
    } else {
        floor + 1.0
    }
}

/// Convert a scaled axis value (already in vJoy units, finite) to the
/// `1..=32768` range. The cast is safe: the value is clamped first,
/// so truncation can only produce an in-range integer.
fn axis_from_scaled(scaled: f32) -> i32 {
    const MIN: f32 = 1.0;
    const MAX: f32 = 32768.0;
    let clamped = scaled.clamp(MIN, MAX);
    #[allow(clippy::cast_possible_truncation)]
    {
        clamped as i32
    }
}

/// Steering `-1..=1` → vJoy axis `1..=32768` (`-1` = full left = 1,
/// `0` = centre = 16384, `+1` = full right = 32768).
///
/// Non-finite input (NaN/inf — the pipeline guarantees finite, this is
/// defence in depth) maps to centre, never to a garbage axis value.
#[must_use]
pub fn steering_to_axis(value: f32) -> i32 {
    const CENTRE: f32 = 16384.0;
    let safe = if value.is_finite() { value } else { 0.0 };
    axis_from_scaled(CENTRE + round_half_even(safe.clamp(-1.0, 1.0) * CENTRE))
}

/// Pedal `0..=1` → vJoy axis `1..=32768` (`0` = released = 1,
/// `1` = fully pressed = 32768). Non-finite input maps to released.
#[must_use]
pub fn pedal_to_axis(value: f32) -> i32 {
    const SPAN: f32 = 32767.0; // AXIS_MAX - AXIS_MIN
    let safe = if value.is_finite() { value } else { 0.0 };
    axis_from_scaled(1.0 + round_half_even(safe.clamp(0.0, 1.0) * SPAN))
}

// ---------------------------------------------------------------------
// Pure mapping: ProcessedState → JOYSTICK_POSITION
// ---------------------------------------------------------------------

/// The safe neutral position: wheel straight, pedals released,
/// handbrake released, no buttons, hats neutral.
///
/// Slider neutral is `AXIS_MIN`, not centre - centre would be half a
/// handbrake. Pedals are `AXIS_MIN` for the same reason; only the
/// steering-class axes sit at centre. Rx is the clutch axis (the feed
/// writes the clutch to Rx as well as to `wClutch`), so its neutral is
/// `AXIS_MIN` too: released clutch, not half clutch.
#[must_use]
pub fn neutral_position(device_id: u32) -> JoystickPosition {
    JoystickPosition {
        // Out-of-range ids map to 0 (invalid device — the driver
        // rejects it); callers pass a validated 1..=16 id.
        b_device: u8::try_from(device_id).unwrap_or(0),
        w_throttle: AXIS_CENTER,
        w_rudder: AXIS_CENTER,
        w_aileron: AXIS_CENTER,
        w_axis_x: AXIS_CENTER,
        w_axis_y: AXIS_MIN,
        w_axis_z: AXIS_MIN,
        w_axis_x_rot: AXIS_MIN,
        w_axis_y_rot: AXIS_CENTER,
        w_axis_z_rot: AXIS_CENTER,
        w_slider: AXIS_MIN,
        w_dial: AXIS_CENTER,
        w_wheel: AXIS_CENTER,
        w_accelerator: AXIS_MIN,
        w_brake: AXIS_MIN,
        w_clutch: AXIS_MIN,
        w_steering: AXIS_CENTER,
        w_axis_vx: AXIS_CENTER,
        w_axis_vy: AXIS_CENTER,
        l_buttons: 0,
        b_hats: HAT_NEUTRAL,
        b_hats_ex1: HAT_NEUTRAL,
        b_hats_ex2: HAT_NEUTRAL,
        b_hats_ex3: HAT_NEUTRAL,
        l_buttons_ex1: 0,
        l_buttons_ex2: 0,
        l_buttons_ex3: 0,
        w_axis_vz: AXIS_CENTER,
        w_axis_vbrx: AXIS_CENTER,
        w_axis_vbry: AXIS_CENTER,
        w_axis_vbrz: AXIS_CENTER,
    }
}

/// Reinterpret button bits as the signed `LONG` the driver expects
/// (bit-exact, no sign-extension surprises).
const fn bits_to_i32(bits: u32) -> i32 {
    i32::from_ne_bytes(bits.to_ne_bytes())
}

/// Build a [`JoystickPosition`] from one processed tick, gated by the
/// device's capabilities (C# `VJoyOutput.BuildPosition`).
///
/// Rebuilt from neutral every call: an axis the device lacks stays
/// neutral, buttons beyond the device's count are dropped by the caps
/// mask (D11 — connect-time validation reports that instead of
/// silently dropping when the plan needs them).
///
/// Notes on fields with no source here:
/// * `lButtonsEx1`/`lButtonsEx2`/`lButtonsEx3` (buttons 33-128) are
///   always 0: v2.0.19 deleted every source for them (buttons/encoders
///   cleanup). Same as the C# series.
#[must_use]
pub fn build_position(
    state: &OutputState,
    caps: &VJoyCapabilities,
    device_id: u32,
) -> JoystickPosition {
    let mut p = neutral_position(device_id);
    if caps.has_axis_x {
        let x = steering_to_axis(state.steering);
        p.w_axis_x = x;
        p.w_steering = x;
    }
    if caps.has_axis_y {
        let y = pedal_to_axis(state.accelerator);
        p.w_axis_y = y;
        p.w_accelerator = y;
    }
    if caps.has_axis_z {
        let z = pedal_to_axis(state.brake);
        p.w_axis_z = z;
        p.w_brake = z;
    }
    // Clutch is caps-independent for `wClutch` (it has no HID usage
    // to query, and the driver ignores the field when the axis is
    // absent) - but it ALSO feeds Rx: Rx is the clutch axis, so a
    // wheel game can see the clutch. Rx is caps-gated (the driver
    // ignores the field when the axis is absent, same as the other
    // axes); `wClutch` keeps its old unconditional write for the
    // games that read the V3 driving axis.
    p.w_clutch = pedal_to_axis(state.clutch);
    if caps.envelope.has_axis_rx {
        p.w_axis_x_rot = pedal_to_axis(state.clutch);
    }
    if caps.has_axis_slider {
        p.w_slider = pedal_to_axis(state.handbrake);
    }
    // D9: the 1-32 word maps ONLY to `lButtons`. Nothing is ever
    // folded or clamped from a higher word into `lButtons`.
    // v2.0.19: Ex1/Ex2/Ex3 are always 0 - the buttons/encoders
    // cleanup deleted every source for buttons 33-128.
    p.l_buttons = bits_to_i32(state.buttons & caps.button_mask());
    p.l_buttons_ex1 = 0;
    p.l_buttons_ex2 = 0;
    p.l_buttons_ex3 = 0;
    p
}

/// D9: validate a single physical button number for the 1-32
/// `lButtons` word.
///
/// Buttons 1-32 pass; anything else (0, 33, 128, ...) is rejected with
/// [`VJoyError::ButtonOutOfRange`] - never clamped onto 1-32.
/// Buttons above 32 have no word any more (v2.0.19 deleted every
/// source for them) - see [`build_position`].
///
/// # Errors
///
/// Returns [`VJoyError::ButtonOutOfRange`] when `button` is outside
/// 1–32.
pub fn check_button_number(button: u8) -> Result<(), VJoyError> {
    if (1..=32).contains(&button) {
        Ok(())
    } else {
        Err(VJoyError::ButtonOutOfRange(button))
    }
}

// ---------------------------------------------------------------------
// Version + SHA-256 pin
// ---------------------------------------------------------------------

/// Format a vJoy `USHORT` version (`DriverMatch` nibbles) as
/// `"major.minor.patch"`: `0x222` → `"2.2.2"`.
#[must_use]
pub fn format_version(version: u16) -> String {
    format!(
        "{}.{}.{}",
        (version >> 8) & 0xF,
        (version >> 4) & 0xF,
        version & 0xF
    )
}

/// Decode a 64-hex-digit SHA-256 string (optional `0x` prefix) to
/// bytes. Case-insensitive.
fn decode_hex_sha256(hex: &str) -> Result<[u8; 32], VJoyError> {
    fn nibble(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }
    let hex = hex.strip_prefix("0x").unwrap_or(hex);
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(VJoyError::InvalidHex(hex.to_string()));
    }
    let bytes = hex.as_bytes();
    let mut out = [0u8; 32];
    for (i, pair) in bytes.chunks(2).enumerate() {
        let (Some(hi), Some(lo)) = (nibble(pair[0]), nibble(pair[1])) else {
            return Err(VJoyError::InvalidHex(hex.to_string()));
        };
        out[i] = (hi << 4) | lo;
    }
    Ok(out)
}

/// Check a file's SHA-256 against an expected hash.
///
/// # Errors
///
/// Returns [`VJoyError::InvalidHex`] for a malformed expected hash,
/// [`VJoyError::DllHashUnavailable`] when the file cannot be read.
pub fn verify_dll_sha256(path: &Path, expected_hex: &str) -> Result<bool, VJoyError> {
    let expected = decode_hex_sha256(expected_hex)?;
    let bytes = fs::read(path).map_err(|e| VJoyError::DllHashUnavailable(e.to_string()))?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(hasher.finalize().as_slice() == expected)
}

// ---------------------------------------------------------------------
// DLL resolution (D18): registry → %ProgramFiles% → %ProgramFiles(x86)%
// ---------------------------------------------------------------------

/// Resolve `vJoyInterface.dll` without ever bundling it (D18).
///
/// Order: registry `InstallLocation` (Windows only) →
/// `%ProgramFiles%\vJoy\x64\...` → `%ProgramFiles(x86)%\vJoy\x64\...`.
/// Returns `None` when the driver is not installed.
#[must_use]
pub fn resolve_dll_path() -> Option<PathBuf> {
    #[cfg(windows)]
    if let Some(path) = registry_dll_path() {
        return Some(path);
    }
    resolve_dll_path_from_env(
        env::var("ProgramFiles").ok().as_deref(),
        env::var("ProgramFiles(x86)").ok().as_deref(),
    )
}

/// Pure half of [`resolve_dll_path`]: the `ProgramFiles` candidates
/// for explicit base directories (testable on any OS — no env reads).
#[must_use]
pub fn resolve_dll_path_from_env(
    program_files: Option<&str>,
    program_files_x86: Option<&str>,
) -> Option<PathBuf> {
    for base in [program_files, program_files_x86].into_iter().flatten() {
        let candidate = Path::new(base)
            .join("vJoy")
            .join("x64")
            .join(VJOY_DLL_FILE_NAME);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Registry probe (Windows only): the vJoy uninstall key's
/// `InstallLocation`, `…\x64\vJoyInterface.dll` when the file exists.
///
/// Best-effort by design — any failure (key missing, value missing,
/// file missing) yields `None` and resolution falls through to the
/// `ProgramFiles` candidates. The exact key is the standard NSIS
/// uninstall location; if a future installer stops writing it, the
/// `ProgramFiles` fallback still finds the DLL.
#[cfg(windows)]
fn registry_dll_path() -> Option<PathBuf> {
    use windows::Win32::System::Registry::{
        HKEY, HKEY_LOCAL_MACHINE, KEY_READ, REG_SZ, RegCloseKey, RegOpenKeyExW, RegQueryValueExW,
    };
    use windows::core::PCWSTR;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn install_location(subkey: &str) -> Option<String> {
        let subkey_w = wide(subkey);
        let value_w = wide("InstallLocation");
        let mut key = HKEY::default();
        // SAFETY: `RegOpenKeyExW` with valid NUL-terminated wide strings;
        // `key` is closed below on every path that opened it.
        let opened = unsafe {
            RegOpenKeyExW(
                HKEY_LOCAL_MACHINE,
                PCWSTR::from_raw(subkey_w.as_ptr()),
                Some(0),
                KEY_READ,
                &raw mut key,
            )
        };
        if opened.0 != 0 {
            return None;
        }
        let mut buf = vec![0u16; 512];
        let mut byte_len = u32::try_from(buf.len() * 2).unwrap_or(0);
        let mut value_type = REG_SZ;
        // SAFETY: `key` is open; `buf` is a live `u16` buffer of the
        // declared byte length; the call only writes into it.
        let queried = unsafe {
            RegQueryValueExW(
                key,
                PCWSTR::from_raw(value_w.as_ptr()),
                None,
                Some(&raw mut value_type),
                Some(buf.as_mut_ptr().cast::<u8>()),
                Some(&raw mut byte_len),
            )
        };
        // SAFETY: closing an open key.
        let _ = unsafe { RegCloseKey(key) };
        if queried.0 != 0 || value_type != REG_SZ {
            return None;
        }
        let chars = usize::try_from(byte_len / 2).unwrap_or(0).min(buf.len());
        let nul = buf[..chars].iter().position(|c| *c == 0).unwrap_or(chars);
        String::from_utf16(&buf[..nul]).ok()
    }

    for subkey in [
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\vJoy",
        r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\vJoy",
    ] {
        if let Some(dir) = install_location(subkey) {
            let candidate = Path::new(&dir).join("x64").join(VJOY_DLL_FILE_NAME);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------
// FFI seam: every native call goes through this trait
// ---------------------------------------------------------------------

/// The vJoy C interface, as a trait.
///
/// This is the mock-DLL seam: the real implementation
/// ([`RealVJoyDll`]) loads `vJoyInterface.dll` with `libloading`;
/// tests use a mock that records calls and returns scripted answers,
/// so the whole backend (except the literal `Library::new`) runs on
/// Linux. All functions are `__cdecl` in the SDK header; on x86-64
/// Windows the declaration is `extern "C"`.
///
/// C `BOOL` is `i32` (not Rust `bool`) — every boolean-returning
/// export is declared `-> i32` and compared against 0 here.
pub trait VJoyInterface: Send {
    /// `vJoyEnabled()`: the driver is loaded and enabled.
    #[must_use]
    fn vjoy_enabled(&self) -> bool;
    /// `GetvJoyVersion()`: DLL version as a nibble `USHORT`.
    #[must_use]
    fn get_vjoy_version(&self) -> u16;
    /// `DriverMatch(&dll, &driver)`: (match, dll version, driver version).
    #[must_use]
    fn driver_match(&self) -> (bool, u16, u16);
    /// `GetVJDStatus(id)`.
    #[must_use]
    fn get_vjd_status(&self, device_id: u32) -> VjdStatus;
    /// `AcquireVJD(id)`.
    fn acquire_vjd(&mut self, device_id: u32) -> bool;
    /// `RelinquishVJD(id)` — returns `void` in the header.
    fn relinquish_vjd(&mut self, device_id: u32);
    /// `ResetVJD(id)`.
    fn reset_vjd(&mut self, device_id: u32) -> bool;
    /// `UpdateVJD(id, &position)` — the 200 Hz path.
    fn update_vjd(&mut self, device_id: u32, position: &JoystickPosition) -> bool;
    /// `GetVJDAxisExist(id, usage)`.
    #[must_use]
    fn get_vjd_axis_exist(&self, device_id: u32, usage: u16) -> bool;
    /// `GetVJDButtonNumber(id)`.
    #[must_use]
    fn get_vjd_button_number(&self, device_id: u32) -> i32;
    /// `GetVJDDiscPovNumber(id)` — discrete (4-direction) POV hats.
    #[must_use]
    fn get_vjd_disc_pov_number(&self, device_id: u32) -> i32;
    /// `GetVJDContPovNumber(id)` — continuous POV hats.
    #[must_use]
    fn get_vjd_cont_pov_number(&self, device_id: u32) -> i32;
}

// Raw `extern "C"` function-pointer types, one per export. The
// signatures mirror the SDK header (`vJoyInterface.h`).
type VJoyEnabledFn = unsafe extern "C" fn() -> i32;
type GetVJoyVersionFn = unsafe extern "C" fn() -> i16;
type DriverMatchFn = unsafe extern "C" fn(*mut u16, *mut u16) -> i32;
type GetVjdStatusFn = unsafe extern "C" fn(u32) -> i32;
type AcquireVjdFn = unsafe extern "C" fn(u32) -> i32;
type RelinquishVjdFn = unsafe extern "C" fn(u32);
type ResetVjdFn = unsafe extern "C" fn(u32) -> i32;
type UpdateVjdFn = unsafe extern "C" fn(u32, *const JoystickPosition) -> i32;
type GetVjdAxisExistFn = unsafe extern "C" fn(u32, u32) -> i32;
type GetVjdButtonNumberFn = unsafe extern "C" fn(u32) -> i32;
type GetVjdDiscPovNumberFn = unsafe extern "C" fn(u32) -> i32;
type GetVjdContPovNumberFn = unsafe extern "C" fn(u32) -> i32;

/// The real DLL, loaded with `libloading`.
///
/// The function pointers are copied out of the `libloading` symbols
/// at load time; the `Library` itself is kept alive in the struct
/// (declared **last**, so it is dropped last — the pointers must
/// never outlive the loaded DLL).
pub struct RealVJoyDll {
    vjoy_enabled: VJoyEnabledFn,
    get_vjoy_version: GetVJoyVersionFn,
    driver_match: DriverMatchFn,
    get_vjd_status: GetVjdStatusFn,
    acquire_vjd: AcquireVjdFn,
    relinquish_vjd: RelinquishVjdFn,
    reset_vjd: ResetVjdFn,
    update_vjd: UpdateVjdFn,
    get_vjd_axis_exist: GetVjdAxisExistFn,
    get_vjd_button_number: GetVjdButtonNumberFn,
    get_vjd_disc_pov_number: GetVjdDiscPovNumberFn,
    get_vjd_cont_pov_number: GetVjdContPovNumberFn,
    /// The loaded library. Last field = dropped last.
    _library: Library,
}

impl RealVJoyDll {
    /// Load the DLL from the resolved path ([`resolve_dll_path`]).
    ///
    /// # Errors
    ///
    /// [`VJoyError::DriverNotFound`] when the driver is not installed,
    /// [`VJoyError::LoadFailed`] when the file cannot be loaded or a
    /// required export is missing.
    pub fn load() -> Result<Self, VJoyError> {
        let path = resolve_dll_path().ok_or_else(|| {
            VJoyError::DriverNotFound(format!(
                "vJoy not found — vJoyInterface.dll could not be located. Install vJoy (BrunnerInnovation {PINNED_DRIVER_VERSION_FULL}): {VJOY_DOWNLOAD_URL}"
            ))
        })?;
        Self::load_from(&path)
    }

    /// Load the DLL from an explicit path (used by [`load`](Self::load)
    /// after resolution; also the injection point for a
    /// release-machine DLL path in diagnostics).
    ///
    /// # Errors
    ///
    /// [`VJoyError::LoadFailed`] when the file cannot be loaded or a
    /// required export is missing (wrong architecture, truncated file,
    /// …).
    pub fn load_from(path: &Path) -> Result<Self, VJoyError> {
        // SAFETY: `Library::new`/`get` are `unsafe` in libloading 0.7
        // because loading foreign code is inherently unsafe. The
        // resolved path is the vJoy install location (never a
        // user-writable app directory), and every symbol is resolved
        // here, before any call, so a missing export fails the load
        // instead of crashing a later update.
        unsafe {
            let library = Library::new(path)
                .map_err(|e| VJoyError::LoadFailed(format!("{}: {e}", path.display())))?;
            let vjoy_enabled: Symbol<'_, VJoyEnabledFn> = library
                .get(b"vJoyEnabled\0")
                .map_err(|e| VJoyError::LoadFailed(format!("vJoyEnabled: {e}")))?;
            let get_vjoy_version: Symbol<'_, GetVJoyVersionFn> =
                library
                    .get(b"GetvJoyVersion\0")
                    .map_err(|e| VJoyError::LoadFailed(format!("GetvJoyVersion: {e}")))?;
            let driver_match: Symbol<'_, DriverMatchFn> = library
                .get(b"DriverMatch\0")
                .map_err(|e| VJoyError::LoadFailed(format!("DriverMatch: {e}")))?;
            let get_vjd_status: Symbol<'_, GetVjdStatusFn> = library
                .get(b"GetVJDStatus\0")
                .map_err(|e| VJoyError::LoadFailed(format!("GetVJDStatus: {e}")))?;
            let acquire_vjd: Symbol<'_, AcquireVjdFn> = library
                .get(b"AcquireVJD\0")
                .map_err(|e| VJoyError::LoadFailed(format!("AcquireVJD: {e}")))?;
            let relinquish_vjd: Symbol<'_, RelinquishVjdFn> = library
                .get(b"RelinquishVJD\0")
                .map_err(|e| VJoyError::LoadFailed(format!("RelinquishVJD: {e}")))?;
            let reset_vjd: Symbol<'_, ResetVjdFn> = library
                .get(b"ResetVJD\0")
                .map_err(|e| VJoyError::LoadFailed(format!("ResetVJD: {e}")))?;
            let update_vjd: Symbol<'_, UpdateVjdFn> = library
                .get(b"UpdateVJD\0")
                .map_err(|e| VJoyError::LoadFailed(format!("UpdateVJD: {e}")))?;
            let get_vjd_axis_exist: Symbol<'_, GetVjdAxisExistFn> = library
                .get(b"GetVJDAxisExist\0")
                .map_err(|e| VJoyError::LoadFailed(format!("GetVJDAxisExist: {e}")))?;
            let get_vjd_button_number: Symbol<'_, GetVjdButtonNumberFn> = library
                .get(b"GetVJDButtonNumber\0")
                .map_err(|e| VJoyError::LoadFailed(format!("GetVJDButtonNumber: {e}")))?;
            let get_vjd_disc_pov_number: Symbol<'_, GetVjdDiscPovNumberFn> = library
                .get(b"GetVJDDiscPovNumber\0")
                .map_err(|e| VJoyError::LoadFailed(format!("GetVJDDiscPovNumber: {e}")))?;
            let get_vjd_cont_pov_number: Symbol<'_, GetVjdContPovNumberFn> = library
                .get(b"GetVJDContPovNumber\0")
                .map_err(|e| VJoyError::LoadFailed(format!("GetVJDContPovNumber: {e}")))?;
            Ok(Self {
                vjoy_enabled: *vjoy_enabled,
                get_vjoy_version: *get_vjoy_version,
                driver_match: *driver_match,
                get_vjd_status: *get_vjd_status,
                acquire_vjd: *acquire_vjd,
                relinquish_vjd: *relinquish_vjd,
                reset_vjd: *reset_vjd,
                update_vjd: *update_vjd,
                get_vjd_axis_exist: *get_vjd_axis_exist,
                get_vjd_button_number: *get_vjd_button_number,
                get_vjd_disc_pov_number: *get_vjd_disc_pov_number,
                get_vjd_cont_pov_number: *get_vjd_cont_pov_number,
                _library: library,
            })
        }
    }
}

impl VJoyInterface for RealVJoyDll {
    fn vjoy_enabled(&self) -> bool {
        // SAFETY: the function pointer came from the loaded DLL (see
        // `load_from`) and the `Library` is kept alive in `self`.
        unsafe { (self.vjoy_enabled)() != 0 }
    }

    fn get_vjoy_version(&self) -> u16 {
        // SAFETY: as above. The SHORT is a nibble version; reinterpret
        // the bits as unsigned.
        unsafe { u16::from_ne_bytes((self.get_vjoy_version)().to_ne_bytes()) }
    }

    fn driver_match(&self) -> (bool, u16, u16) {
        let (mut dll, mut driver) = (0u16, 0u16);
        // SAFETY: as above; the two `USHORT*` out-params point at live
        // stack slots for the duration of the call.
        let matched = unsafe { (self.driver_match)(&raw mut dll, &raw mut driver) != 0 };
        (matched, dll, driver)
    }

    fn get_vjd_status(&self, device_id: u32) -> VjdStatus {
        // SAFETY: as above.
        VjdStatus::from_raw(unsafe { (self.get_vjd_status)(device_id) })
    }

    fn acquire_vjd(&mut self, device_id: u32) -> bool {
        // SAFETY: as above.
        unsafe { (self.acquire_vjd)(device_id) != 0 }
    }

    fn relinquish_vjd(&mut self, device_id: u32) {
        // SAFETY: as above. `RelinquishVJD` returns void by header.
        unsafe { (self.relinquish_vjd)(device_id) }
    }

    fn reset_vjd(&mut self, device_id: u32) -> bool {
        // SAFETY: as above.
        unsafe { (self.reset_vjd)(device_id) != 0 }
    }

    fn update_vjd(&mut self, device_id: u32, position: &JoystickPosition) -> bool {
        // SAFETY: as above. `UpdateVJD` reads the position (it never
        // writes back); the pointer is valid for the call.
        unsafe { (self.update_vjd)(device_id, position) != 0 }
    }

    fn get_vjd_axis_exist(&self, device_id: u32, usage: u16) -> bool {
        // SAFETY: as above. M10: the SDK header declares
        // `GetVJDAxisExist(UINT, UINT)` — both params are `u32`;
        // the `u16` usage widens here, it was never a 16-bit param.
        unsafe { (self.get_vjd_axis_exist)(device_id, u32::from(usage)) != 0 }
    }

    fn get_vjd_button_number(&self, device_id: u32) -> i32 {
        // SAFETY: as above.
        unsafe { (self.get_vjd_button_number)(device_id) }
    }

    fn get_vjd_disc_pov_number(&self, device_id: u32) -> i32 {
        // SAFETY: as above.
        unsafe { (self.get_vjd_disc_pov_number)(device_id) }
    }

    fn get_vjd_cont_pov_number(&self, device_id: u32) -> i32 {
        // SAFETY: as above.
        unsafe { (self.get_vjd_cont_pov_number)(device_id) }
    }
}

// ---------------------------------------------------------------------
// VJoyOutput — the backend
// ---------------------------------------------------------------------

/// vJoy output backend (G3): the always-present fallback.
///
/// Created with [`VJoyOutput::new`] (lazy DLL load on connect) or
/// [`VJoyOutput::with_dll`] (injected [`VJoyInterface`], used by
/// tests and by the backend selector). Connect flow mirrors the C#
/// `VJoyOutput.TryConnect`: enabled check → `DriverMatch` + pinned
/// version → optional SHA-256 → device status → acquire → read caps
/// → optional plan validation → `ResetVJD` + neutral.
///
/// A failed connect leaves the backend disconnected with
/// [`status`](VJoyOutput::status) explaining why — the app keeps
/// running with output disabled.
pub struct VJoyOutput {
    device_id: u32,
    dll: Option<Box<dyn VJoyInterface>>,
    dll_path: Option<PathBuf>,
    expected_dll_sha256: Option<String>,
    connected: bool,
    caps_known: bool,
    capabilities: VJoyCapabilities,
    status: VJoyStatus,
    /// Envelope-shape problems measured at connect (see
    /// [`validate_envelope`]): REPORTS only — a non-empty list
    /// never refused the connect, feeding keeps working on a
    /// default-shape device. Empty when the envelope matches (or
    /// when never connected).
    envelope_warnings: Vec<String>,
    /// Consecutive failed `UpdateVJD` calls (M2). Reset on every
    /// successful update; at [`MAX_CONSECUTIVE_UPDATE_FAILURES`] the
    /// backend marks itself disconnected (status `Error`) instead of
    /// reporting success-then-failure forever on a lost device.
    consecutive_update_failures: u32,
}

/// Consecutive failed updates after which vJoy marks itself
/// disconnected (M2). One or two failures are transient (the old
/// behaviour: stay connected, report the error); three in a row means
/// the device is gone.
pub const MAX_CONSECUTIVE_UPDATE_FAILURES: u32 = 3;

impl VJoyOutput {
    /// Create the backend for a vJoy device id (1–16; the default
    /// wheel uses 1). Nothing is loaded yet — the DLL is resolved on
    /// the first [`try_connect`](IJoystickOutput::try_connect).
    #[must_use]
    pub fn new(device_id: u32) -> Self {
        Self {
            device_id,
            dll: None,
            dll_path: None,
            expected_dll_sha256: None,
            connected: false,
            caps_known: false,
            capabilities: VJoyCapabilities::unknown(),
            status: VJoyStatus::idle(),
            envelope_warnings: Vec::new(),
            consecutive_update_failures: 0,
        }
    }

    /// Create the backend with an injected DLL implementation (mock in
    /// tests, [`RealVJoyDll`] in production via [`new`](Self::new)).
    #[must_use]
    pub fn with_dll(device_id: u32, dll: Box<dyn VJoyInterface>) -> Self {
        let mut out = Self::new(device_id);
        out.dll = Some(dll);
        out
    }

    /// Pin the expected SHA-256 of `vJoyInterface.dll` (lowercase or
    /// uppercase hex, optional `0x` prefix). Enforced on connect when
    /// set: a mismatch refuses the DLL. Until
    /// [`PINNED_DLL_SHA256_HEX`] is measured, callers (release
    /// tooling, diagnostics) pass the measured hash here.
    ///
    /// # Errors
    ///
    /// [`VJoyError::InvalidHex`] when `hex` is not 64 hex digits.
    pub fn set_expected_dll_sha256(&mut self, hex: &str) -> Result<(), VJoyError> {
        let decoded = decode_hex_sha256(hex)?;
        let _ = decoded;
        self.expected_dll_sha256 = Some(hex.trim_start_matches("0x").to_lowercase());
        Ok(())
    }

    /// Device id this backend drives (1–16).
    #[must_use]
    pub const fn device_id(&self) -> u32 {
        self.device_id
    }

    /// The Dashboard snapshot: state + message + versions + install
    /// link. Updated by every connect attempt and by disconnect.
    #[must_use]
    pub fn status(&self) -> VJoyStatus {
        self.status.clone()
    }

    /// Capabilities read at connect (unknown before the first
    /// successful connect).
    #[must_use]
    pub const fn capabilities(&self) -> VJoyCapabilities {
        self.capabilities
    }

    /// Envelope-shape problems measured at the last successful
    /// connect ([`validate_envelope`]) — empty when the envelope
    /// matches or when never connected. Reporting only: a
    /// non-empty list never refused the connect.
    #[must_use]
    pub fn envelope_warnings(&self) -> &[String] {
        &self.envelope_warnings
    }

    /// Connect, validating the device against the enabled modules of
    /// `plan` when one is given (D11). `None` skips validation.
    ///
    /// # Errors
    ///
    /// See [`VJoyError`]: driver missing/disabled, version mismatch
    /// or unsupported release, SHA-256 mismatch, device busy/missing,
    /// acquire failure, or a capabilities error (device relinquished
    /// in that case).
    ///
    /// # Panics
    ///
    /// Panics only on an internal invariant violation (DLL expected
    /// loaded but absent after a successful load) — never on driver
    /// state.
    pub fn connect_with_plan(&mut self, plan: Option<&PipelinePlan>) -> Result<(), VJoyError> {
        if self.connected {
            return Ok(());
        }
        if !(1..=MAX_DEVICE_ID).contains(&self.device_id) {
            let err = VJoyError::InvalidDeviceId(self.device_id);
            self.fail(&err);
            return Err(err);
        }
        // Load the DLL (lazy on first connect; injected DLLs skip this).
        // M1 + L5: the path is resolved ONCE, the SHA-256 (when a pin
        // is set) is verified against that SAME path BEFORE the DLL is
        // loaded — the old code loaded first and verified afterwards
        // (and resolved the path twice, so the verified file and the
        // loaded file could differ — load-before-verify + TOCTOU).
        if self.dll.is_none() {
            let Some(path) = resolve_dll_path() else {
                let err = VJoyError::DriverNotFound(format!(
                    "vJoy not found — vJoyInterface.dll could not be located. Install vJoy (BrunnerInnovation {PINNED_DRIVER_VERSION_FULL}): {VJOY_DOWNLOAD_URL}"
                ));
                self.fail(&err);
                return Err(err);
            };
            let expected = self
                .expected_dll_sha256
                .clone()
                .or_else(|| PINNED_DLL_SHA256_HEX.map(str::to_string));
            if let Some(expected) = expected {
                match verify_dll_sha256(&path, &expected) {
                    Ok(true) => {}
                    Ok(false) => {
                        let actual = sha256_hex(&path).unwrap_or_else(|_| "?".to_string());
                        let err = VJoyError::DllHashMismatch { expected, actual };
                        self.fail(&err);
                        return Err(err);
                    }
                    Err(err) => {
                        self.fail(&err);
                        return Err(err);
                    }
                }
            }
            match RealVJoyDll::load_from(&path) {
                Ok(dll) => {
                    self.dll_path = Some(path);
                    self.dll = Some(Box::new(dll));
                }
                Err(err) => {
                    self.fail(&err);
                    return Err(err);
                }
            }
        }
        // The borrow of `self.dll` must end before the `self.status`
        // writes below, so the whole driver conversation happens in
        // this scope and only plain results escape it.
        let outcome = {
            let device_id = self.device_id;
            let dll = self.dll.as_mut().expect("DLL loaded above").as_mut();
            connect_via(device_id, dll, plan)
        };
        match outcome {
            Ok((dll_version, driver_version, caps, envelope_warnings)) => {
                self.capabilities = caps;
                self.caps_known = true;
                self.envelope_warnings = envelope_warnings;
                self.connected = true;
                self.consecutive_update_failures = 0;
                self.status = VJoyStatus {
                    kind: VJoyStateKind::Ready,
                    message: format!(
                        "vJoy device {} ready (driver v{dll_version}, {} buttons).",
                        self.device_id, caps.button_count
                    ),
                    dll_version: Some(dll_version),
                    driver_version: Some(driver_version),
                    download_url: None,
                };
                Ok(())
            }
            Err(err) => {
                self.fail(&err);
                Err(err)
            }
        }
    }

    /// Record a failed connect in the Dashboard status.
    fn fail(&mut self, err: &VJoyError) {
        let (kind, download_url) = match err {
            VJoyError::DriverNotFound(_) => (VJoyStateKind::NotInstalled, Some(VJOY_DOWNLOAD_URL)),
            VJoyError::DriverDisabled => (VJoyStateKind::Disabled, Some(VJOY_DOWNLOAD_URL)),
            VJoyError::VersionMismatch { .. }
            | VJoyError::UnsupportedVersion { .. }
            | VJoyError::DllHashMismatch { .. }
            | VJoyError::DllHashUnavailable(_) => {
                (VJoyStateKind::VersionMismatch, Some(VJOY_DOWNLOAD_URL))
            }
            VJoyError::DeviceBusy(_) => (VJoyStateKind::Busy, None),
            VJoyError::DeviceMissing(_) => (VJoyStateKind::Missing, None),
            _ => (VJoyStateKind::Error, None),
        };
        self.status = VJoyStatus {
            kind,
            message: err.to_string(),
            dll_version: None,
            driver_version: None,
            download_url,
        };
    }

    /// Best-effort neutral send + relinquish. Never panics; safe to
    /// call from [`Drop`].
    fn disconnect_inner(&mut self) {
        if !self.connected {
            return;
        }
        self.connected = false;
        self.consecutive_update_failures = 0;
        if let Some(dll) = self.dll.as_mut() {
            let neutral = neutral_position(self.device_id);
            let _ = dll.update_vjd(self.device_id, &neutral);
            dll.relinquish_vjd(self.device_id);
        }
        self.status = VJoyStatus::idle();
    }
}

/// The driver conversation: enabled → versions → status → acquire →
/// caps → plan validation → reset + neutral. Returns the versions,
/// caps, and envelope warnings on success. The device is
/// relinquished on any failure after acquisition. A free function
/// (not a method) so it can borrow the injected DLL without
/// borrowing the whole backend.
///
/// The envelope read is REPORTS ONLY (see [`validate_envelope`]):
/// its warnings ride along in the return value and never fail the
/// connect — a default-shape device still feeds.
fn connect_via(
    device_id: u32,
    dll: &mut dyn VJoyInterface,
    plan: Option<&PipelinePlan>,
) -> Result<(String, String, VJoyCapabilities, Vec<String>), VJoyError> {
    if !dll.vjoy_enabled() {
        return Err(VJoyError::DriverDisabled);
    }
    let (matched, dll_raw, driver_raw) = dll.driver_match();
    let dll_version = format_version(dll_raw);
    let driver_version = format_version(driver_raw);
    if !matched {
        return Err(VJoyError::VersionMismatch {
            dll: dll_version,
            driver: driver_version,
        });
    }
    if dll_version != PINNED_DRIVER_VERSION {
        return Err(VJoyError::UnsupportedVersion {
            found: dll_version,
            pinned: PINNED_DRIVER_VERSION.to_string(),
        });
    }
    match dll.get_vjd_status(device_id) {
        VjdStatus::Own => {}
        VjdStatus::Free => {
            if !dll.acquire_vjd(device_id) {
                return Err(VJoyError::AcquireFailed(device_id));
            }
        }
        VjdStatus::Busy => return Err(VJoyError::DeviceBusy(device_id)),
        VjdStatus::Missing => return Err(VJoyError::DeviceMissing(device_id)),
        VjdStatus::Unknown => return Err(VJoyError::DeviceUnknown(device_id)),
    }
    let caps = VJoyCapabilities::new(
        dll.get_vjd_axis_exist(device_id, USAGE_X),
        dll.get_vjd_axis_exist(device_id, USAGE_Y),
        dll.get_vjd_axis_exist(device_id, USAGE_Z),
        dll.get_vjd_button_number(device_id),
        dll.get_vjd_axis_exist(device_id, USAGE_SLIDER),
    )
    .with_envelope(EnvelopeCaps {
        has_axis_rx: dll.get_vjd_axis_exist(device_id, USAGE_RX),
        has_axis_ry: dll.get_vjd_axis_exist(device_id, USAGE_RY),
        has_axis_rz: dll.get_vjd_axis_exist(device_id, USAGE_RZ),
        has_axis_dial: dll.get_vjd_axis_exist(device_id, USAGE_DIAL),
        discrete_pov_count: dll.get_vjd_disc_pov_number(device_id),
        continuous_pov_count: dll.get_vjd_cont_pov_number(device_id),
    });
    // Envelope-shape validation REPORTS only: a wrong-shaped
    // device still connects and feeds (ETS2 etc. depend on it).
    // The warnings ride along for the backend status and the wheel
    // check; the plan validation below is the only gate, and it
    // only checks the enabled modules' axes/buttons.
    let envelope_warnings = validate_envelope(&caps);
    if let Some(plan) = plan {
        let errors = validate_capabilities(plan, &caps);
        if !errors.is_empty() {
            dll.relinquish_vjd(device_id);
            return Err(VJoyError::InsufficientCapabilities(format!(
                "vJoy device cannot serve this plan: {}",
                errors.join(" ")
            )));
        }
    }
    // Reset, then neutral — the device starts from a known state
    // (C# parity: both best-effort, connect still succeeds).
    let _ = dll.reset_vjd(device_id);
    let neutral = neutral_position(device_id);
    let _ = dll.update_vjd(device_id, &neutral);
    Ok((dll_version, driver_version, caps, envelope_warnings))
}

impl Drop for VJoyOutput {
    fn drop(&mut self) {
        self.disconnect_inner();
    }
}

impl IJoystickOutput for VJoyOutput {
    fn name(&self) -> &'static str {
        "vJoy"
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    fn try_connect(&mut self) -> Result<(), VJoyError> {
        self.connect_with_plan(None)
    }

    fn try_connect_with_plan(&mut self, plan: &PipelinePlan) -> Result<(), VJoyError> {
        self.connect_with_plan(Some(plan))
    }

    /// The driver version `connect_with_plan` measured (`status`
    /// carries it after a successful connect; `None` before the
    /// first connect — an unmeasured version is never invented).
    fn vjoy_driver_version(&self) -> Option<String> {
        self.status.driver_version.clone()
    }

    fn disconnect(&mut self) {
        self.disconnect_inner();
    }

    /// # Panics
    ///
    /// Panics only on an internal invariant violation (connected
    /// without a DLL) — never on driver state.
    fn update(&mut self, state: &ProcessedState) -> Result<(), VJoyError> {
        if !self.connected {
            return Err(VJoyError::NotConnected);
        }
        let caps = if self.caps_known {
            self.capabilities
        } else {
            VJoyCapabilities::unknown()
        };
        let position = build_position(state, &caps, self.device_id);
        let dll = self.dll.as_mut().expect("connected implies a DLL");
        let updated = dll.update_vjd(self.device_id, &position);
        if updated {
            self.consecutive_update_failures = 0;
            Ok(())
        } else {
            // M2: N consecutive failures = the device is gone — mark
            // disconnected (status `Error`, device relinquished) so
            // the selector / engine stops treating a dead device as
            // live. Fewer than N failures keep the old behaviour
            // (stay connected, report the error).
            self.consecutive_update_failures += 1;
            if self.consecutive_update_failures >= MAX_CONSECUTIVE_UPDATE_FAILURES {
                self.connected = false;
                if let Some(dll) = self.dll.as_mut() {
                    dll.relinquish_vjd(self.device_id);
                }
                self.status = VJoyStatus {
                    kind: VJoyStateKind::Error,
                    message: format!(
                        "vJoy device {} lost after {MAX_CONSECUTIVE_UPDATE_FAILURES} consecutive failed updates — disconnected.",
                        self.device_id
                    ),
                    dll_version: None,
                    driver_version: None,
                    download_url: None,
                };
            }
            Err(VJoyError::UpdateFailed(self.device_id))
        }
    }

    /// # Panics
    ///
    /// Panics only on an internal invariant violation (connected
    /// without a DLL) — never on driver state.
    fn send_neutral(&mut self) -> Result<(), VJoyError> {
        if !self.connected {
            return Ok(());
        }
        let neutral = neutral_position(self.device_id);
        let dll = self.dll.as_mut().expect("connected implies a DLL");
        // Best-effort by design (C# parity): a failed neutral send is
        // swallowed, never fatal — disconnect still relinquishes.
        let _ = dll.update_vjd(self.device_id, &neutral);
        Ok(())
    }
}

/// SHA-256 of a file as lowercase hex (for the mismatch error detail).
fn sha256_hex(path: &Path) -> Result<String, VJoyError> {
    let bytes = fs::read(path).map_err(|e| VJoyError::DllHashUnavailable(e.to_string()))?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let mut hex = String::with_capacity(64);
    for byte in hasher.finalize() {
        let _ = write!(hex, "{byte:02x}");
    }
    Ok(hex)
}

// ---------------------------------------------------------------------
// Tests — modelled on the C# `RSH.VJoy.Tests` suite (56 tests).
//
// Pure mapping/validation tests run anywhere with no driver. Lifecycle
// tests run against `MockDll` (the `VJoyInterface` seam) — the real
// DLL does not exist on Linux. Real-DLL verification is Windows-only.
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::IJoystickOutput;

    // ---- helpers -----------------------------------------------------

    fn full_caps() -> VJoyCapabilities {
        VJoyCapabilities::new(true, true, true, 32, true)
    }

    fn state(steering: f32, accel: f32, brake: f32) -> OutputState {
        OutputState {
            steering,
            accelerator: accel,
            brake,
            ..OutputState::default()
        }
    }

    #[allow(clippy::fn_params_excessive_bools)] // test helper mirroring C# PlanWith
    fn plan_with(
        steering: bool,
        accel: bool,
        brake: bool,
        handbrake: bool,
        horn: bool,
    ) -> PipelinePlan {
        let mut plan = PipelinePlan::default();
        plan.steering.enabled = steering;
        plan.accelerator.enabled = accel;
        plan.brake.enabled = brake;
        plan.handbrake.enabled = handbrake;
        plan.horn.enabled = horn;
        // Shifter is not a plan_with parameter: keep it off so the
        // shifter button check does not fire in unrelated tests.
        plan.shifter.enabled = false;
        plan
    }

    fn steering_only_plan() -> PipelinePlan {
        plan_with(true, false, false, false, false)
    }

    // ---- mock DLL ----------------------------------------------------

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum MockCall {
        Acquire(u32),
        Relinquish(u32),
        Reset(u32),
        Update(u32),
    }

    #[derive(Debug)]
    // Four independent scripted flags — the mock's whole point.
    #[allow(clippy::struct_excessive_bools)]
    struct MockState {
        enabled: bool,
        driver_match: bool,
        dll_version: u16,
        driver_version: u16,
        status: VjdStatus,
        acquire_ok: bool,
        update_ok: bool,
        /// X, Y, Z, Slider.
        axes: [bool; 4],
        /// Rx, Ry, Rz, Dial (envelope axes).
        extra_axes: [bool; 4],
        /// Discrete POV hats (envelope: 1).
        disc_pov_count: i32,
        /// Continuous POV hats (envelope: 0).
        cont_pov_count: i32,
        button_count: i32,
        calls: Vec<MockCall>,
        positions: Vec<JoystickPosition>,
    }

    impl Default for MockState {
        fn default() -> Self {
            Self {
                enabled: true,
                driver_match: true,
                dll_version: 0x222,
                driver_version: 0x222,
                status: VjdStatus::Free,
                acquire_ok: true,
                update_ok: true,
                axes: [true; 4],
                extra_axes: [true; 4],
                disc_pov_count: 1,
                cont_pov_count: 0,
                button_count: 32,
                calls: Vec::new(),
                positions: Vec::new(),
            }
        }
    }

    #[derive(Clone, Debug)]
    struct MockDll {
        state: Arc<Mutex<MockState>>,
    }

    #[derive(Clone, Debug)]
    struct MockHandle {
        state: Arc<Mutex<MockState>>,
    }

    impl MockDll {
        fn with_defaults() -> (Self, MockHandle) {
            Self::with_state(MockState::default())
        }

        fn with_state(state: MockState) -> (Self, MockHandle) {
            let shared = Arc::new(Mutex::new(state));
            (
                Self {
                    state: Arc::clone(&shared),
                },
                MockHandle { state: shared },
            )
        }
    }

    impl MockHandle {
        fn calls(&self) -> Vec<MockCall> {
            self.state.lock().expect("mock lock").calls.clone()
        }

        fn positions(&self) -> Vec<JoystickPosition> {
            self.state.lock().expect("mock lock").positions.clone()
        }
    }

    impl VJoyInterface for MockDll {
        fn vjoy_enabled(&self) -> bool {
            self.state.lock().expect("mock lock").enabled
        }

        fn get_vjoy_version(&self) -> u16 {
            self.state.lock().expect("mock lock").dll_version
        }

        fn driver_match(&self) -> (bool, u16, u16) {
            let s = self.state.lock().expect("mock lock");
            (s.driver_match, s.dll_version, s.driver_version)
        }

        fn get_vjd_status(&self, device_id: u32) -> VjdStatus {
            let _ = device_id;
            self.state.lock().expect("mock lock").status
        }

        fn acquire_vjd(&mut self, device_id: u32) -> bool {
            let mut s = self.state.lock().expect("mock lock");
            s.calls.push(MockCall::Acquire(device_id));
            s.acquire_ok
        }

        fn relinquish_vjd(&mut self, device_id: u32) {
            self.state
                .lock()
                .expect("mock lock")
                .calls
                .push(MockCall::Relinquish(device_id));
        }

        fn reset_vjd(&mut self, device_id: u32) -> bool {
            self.state
                .lock()
                .expect("mock lock")
                .calls
                .push(MockCall::Reset(device_id));
            true
        }

        fn update_vjd(&mut self, device_id: u32, position: &JoystickPosition) -> bool {
            let mut s = self.state.lock().expect("mock lock");
            s.calls.push(MockCall::Update(device_id));
            s.positions.push(*position);
            s.update_ok
        }

        fn get_vjd_axis_exist(&self, device_id: u32, usage: u16) -> bool {
            let _ = device_id;
            let s = self.state.lock().expect("mock lock");
            match usage {
                USAGE_X => s.axes[0],
                USAGE_Y => s.axes[1],
                USAGE_Z => s.axes[2],
                USAGE_SLIDER => s.axes[3],
                USAGE_RX => s.extra_axes[0],
                USAGE_RY => s.extra_axes[1],
                USAGE_RZ => s.extra_axes[2],
                USAGE_DIAL => s.extra_axes[3],
                _ => false,
            }
        }

        fn get_vjd_button_number(&self, device_id: u32) -> i32 {
            let _ = device_id;
            self.state.lock().expect("mock lock").button_count
        }

        fn get_vjd_disc_pov_number(&self, device_id: u32) -> i32 {
            let _ = device_id;
            self.state.lock().expect("mock lock").disc_pov_count
        }

        fn get_vjd_cont_pov_number(&self, device_id: u32) -> i32 {
            let _ = device_id;
            self.state.lock().expect("mock lock").cont_pov_count
        }
    }

    fn connected_output() -> (VJoyOutput, MockHandle) {
        let (mock, handle) = MockDll::with_defaults();
        let mut out = VJoyOutput::with_dll(1, Box::new(mock));
        out.try_connect().expect("mock connect succeeds");
        (out, handle)
    }

    // ---- layout ------------------------------------------------------

    #[test]
    fn struct_size_is_124() {
        assert_eq!(size_of::<JoystickPosition>(), 124);
    }

    #[test]
    fn struct_offsets_match_header() {
        assert_eq!(offset_of!(JoystickPosition, b_device), 0);
        assert_eq!(offset_of!(JoystickPosition, w_throttle), 4);
        assert_eq!(offset_of!(JoystickPosition, w_axis_x), 16);
        assert_eq!(offset_of!(JoystickPosition, w_axis_y), 20);
        assert_eq!(offset_of!(JoystickPosition, w_axis_z), 24);
        assert_eq!(offset_of!(JoystickPosition, w_slider), 40);
        assert_eq!(offset_of!(JoystickPosition, w_accelerator), 52);
        assert_eq!(offset_of!(JoystickPosition, w_steering), 64);
        assert_eq!(offset_of!(JoystickPosition, l_buttons), 76);
        assert_eq!(offset_of!(JoystickPosition, b_hats), 80);
        assert_eq!(offset_of!(JoystickPosition, l_buttons_ex1), 96);
        assert_eq!(offset_of!(JoystickPosition, w_axis_vz), 108);
        assert_eq!(offset_of!(JoystickPosition, w_axis_vbrz), 120);
    }

    #[test]
    fn neutral_position_values() {
        let p = neutral_position(1);
        assert_eq!(p.b_device, 1);
        assert_eq!(p.w_axis_x, 16384); // steering centre
        assert_eq!(p.w_axis_y, 1); // pedal released
        assert_eq!(p.w_axis_z, 1);
        assert_eq!(p.w_slider, 1); // handbrake released (min, not centre)
        assert_eq!(p.w_axis_x_rot, 1); // clutch released (Rx is the clutch axis)
        assert_eq!(p.w_steering, 16384);
        assert_eq!(p.w_accelerator, 1);
        assert_eq!(p.w_brake, 1);
        assert_eq!(p.w_clutch, 1);
        assert_eq!(p.l_buttons, 0);
        assert_eq!(p.b_hats, 0xFFFF_FFFF);
        assert_eq!(p.b_hats_ex1, 0xFFFF_FFFF);
        assert_eq!(p.l_buttons_ex1, 0);
        assert_eq!(p.l_buttons_ex2, 0);
        assert_eq!(p.l_buttons_ex3, 0);
    }

    #[test]
    fn neutral_position_carries_device_id() {
        assert_eq!(neutral_position(7).b_device, 7);
    }

    #[test]
    fn vjd_status_from_raw() {
        assert_eq!(VjdStatus::from_raw(0), VjdStatus::Own);
        assert_eq!(VjdStatus::from_raw(1), VjdStatus::Free);
        assert_eq!(VjdStatus::from_raw(2), VjdStatus::Busy);
        assert_eq!(VjdStatus::from_raw(3), VjdStatus::Missing);
        assert_eq!(VjdStatus::from_raw(4), VjdStatus::Unknown);
        assert_eq!(VjdStatus::from_raw(99), VjdStatus::Unknown);
    }

    #[test]
    fn format_version_nibbles() {
        assert_eq!(format_version(0x222), "2.2.2");
        assert_eq!(format_version(0x219), "2.1.9");
        assert_eq!(format_version(0x000), "0.0.0");
    }

    // ---- axis conversion ---------------------------------------------

    #[test]
    fn axis_conversion_steering_edges() {
        assert_eq!(steering_to_axis(-1.0), 1);
        assert_eq!(steering_to_axis(-0.5), 8192);
        assert_eq!(steering_to_axis(0.0), 16384);
        assert_eq!(steering_to_axis(0.5), 24576);
        assert_eq!(steering_to_axis(1.0), 32768);
        assert_eq!(steering_to_axis(2.0), 32768); // clamped, not wrapped
        assert_eq!(steering_to_axis(-2.0), 1);
    }

    #[test]
    fn axis_conversion_pedal_edges() {
        assert_eq!(pedal_to_axis(0.0), 1);
        assert_eq!(pedal_to_axis(0.5), 16385);
        assert_eq!(pedal_to_axis(1.0), 32768);
        assert_eq!(pedal_to_axis(1.5), 32768);
        assert_eq!(pedal_to_axis(-0.5), 1);
    }

    #[test]
    fn axis_conversion_half_even_rounding() {
        // Exact halves go to the even neighbour (C# Math.Round parity):
        // 0.5 -> 0, 1.5 -> 2, 2.5 -> 2.
        let half = 0.5 / 16384.0;
        assert_eq!(steering_to_axis(half), 16384);
        assert_eq!(steering_to_axis(1.5 / 16384.0), 16386);
        assert_eq!(steering_to_axis(2.5 / 16384.0), 16386);
        assert_eq!(steering_to_axis(-half), 16384);
    }

    #[test]
    fn axis_conversion_non_finite_goes_neutral() {
        // Non-finite input maps to neutral: steering centre, pedal released.
        assert_eq!(steering_to_axis(f32::NAN), 16384);
        assert_eq!(steering_to_axis(f32::INFINITY), 16384);
        assert_eq!(steering_to_axis(f32::NEG_INFINITY), 16384);
        assert_eq!(pedal_to_axis(f32::NAN), 1);
        assert_eq!(pedal_to_axis(f32::INFINITY), 1);
        assert_eq!(pedal_to_axis(f32::NEG_INFINITY), 1);
    }

    // ---- mapping: axes -----------------------------------------------

    #[test]
    fn mapping_steering_to_x_and_v3_steering() {
        let p = build_position(&state(-0.94, 0.0, 0.0), &full_caps(), 1);
        // -0.94 * 16384 = -15400.96 -> -15401 -> 16384 - 15401 = 983
        assert_eq!(p.w_axis_x, 983);
        assert_eq!(p.w_steering, 983);
        assert_eq!(p.w_axis_y, 1); // pedals untouched
        assert_eq!(p.w_axis_z, 1);
    }

    #[test]
    fn mapping_pedals_to_y_z_and_v3_accel_brake() {
        let p = build_position(&state(0.0, 0.61, 0.92), &full_caps(), 1);
        assert_eq!(p.w_axis_y, pedal_to_axis(0.61));
        assert_eq!(p.w_accelerator, pedal_to_axis(0.61));
        assert_eq!(p.w_axis_z, pedal_to_axis(0.92));
        assert_eq!(p.w_brake, pedal_to_axis(0.92));
        assert_eq!(p.w_axis_x, 16384); // steering centre untouched
        assert_eq!(p.w_steering, 16384);
    }

    #[test]
    fn mapping_full_deflection_edges_exact() {
        let p = build_position(&state(1.0, 1.0, 1.0), &full_caps(), 1);
        assert_eq!(p.w_axis_x, 32768);
        assert_eq!(p.w_axis_y, 32768);
        assert_eq!(p.w_axis_z, 32768);
        assert_eq!(p.w_steering, 32768);
        assert_eq!(p.w_accelerator, 32768);
        assert_eq!(p.w_brake, 32768);

        let q = build_position(&state(-1.0, 0.0, 0.0), &full_caps(), 1);
        assert_eq!(q.w_axis_x, 1);
        assert_eq!(q.w_axis_y, 1);
        assert_eq!(q.w_axis_z, 1);
    }

    #[test]
    fn mapping_neutral_state_matches_neutral_position() {
        let p = build_position(&OutputState::neutral(), &full_caps(), 1);
        let n = neutral_position(1);
        assert_eq!(p, n);
    }

    // ---- mapping: buttons (D9) ---------------------------------------

    #[test]
    fn mapping_buttons_1_to_32_go_to_lbuttons() {
        for button in 1..=32u32 {
            let s = OutputState {
                buttons: 1u32 << (button - 1),
                ..OutputState::default()
            };
            let p = build_position(&s, &full_caps(), 1);
            assert_eq!(
                p.l_buttons,
                bits_to_i32(1u32 << (button - 1)),
                "button {button}"
            );
        }
    }

    #[test]
    fn mapping_all_buttons_1_to_32_set_together() {
        let s = OutputState {
            buttons: u32::MAX,
            ..OutputState::default()
        };
        assert_eq!(build_position(&s, &full_caps(), 1).l_buttons, -1);
    }

    #[test]
    fn mapping_unknown_caps_writes_everything() {
        let s = OutputState {
            steering: 1.0,
            accelerator: 1.0,
            brake: 1.0,
            buttons: 0b1111,
            ..OutputState::default()
        };
        let p = build_position(&s, &VJoyCapabilities::unknown(), 1);
        assert_eq!(p.w_axis_x, 32768);
        assert_eq!(p.w_axis_y, 32768);
        assert_eq!(p.w_axis_z, 32768);
        assert_eq!(p.w_slider, 1); // handbrake released (min)
        assert_eq!(p.l_buttons, 0b1111);
    }

    #[test]
    fn d9_check_button_number() {
        assert!(check_button_number(1).is_ok());
        assert!(check_button_number(32).is_ok());
        assert_eq!(check_button_number(0), Err(VJoyError::ButtonOutOfRange(0)));
        assert_eq!(
            check_button_number(33),
            Err(VJoyError::ButtonOutOfRange(33))
        );
        assert_eq!(
            check_button_number(128),
            Err(VJoyError::ButtonOutOfRange(128))
        );
    }

    // ---- mapping: clutch / handbrake --------------------------------

    #[test]
    fn mapping_clutch_to_wclutch() {
        let s = OutputState {
            clutch: 1.0,
            ..OutputState::default()
        };
        assert_eq!(build_position(&s, &full_caps(), 1).w_clutch, 32768);
        assert_eq!(
            build_position(&OutputState::neutral(), &full_caps(), 1).w_clutch,
            1
        );
    }

    #[test]
    fn mapping_clutch_written_without_xyz_caps() {
        // WClutch is caps-independent (no HID usage to query).
        let s = OutputState {
            clutch: 0.61,
            ..OutputState::default()
        };
        let caps = VJoyCapabilities::new(false, false, false, 0, false);
        let p = build_position(&s, &caps, 1);
        assert_eq!(p.w_clutch, pedal_to_axis(0.61));
        assert_eq!(p.w_axis_x, 16384);
    }

    #[test]
    fn mapping_handbrake_full_and_half_to_slider() {
        let full = OutputState {
            handbrake: 1.0,
            ..OutputState::default()
        };
        assert_eq!(build_position(&full, &full_caps(), 1).w_slider, 32768);
        let half = OutputState {
            handbrake: 0.5,
            ..OutputState::default()
        };
        assert_eq!(build_position(&half, &full_caps(), 1).w_slider, 16385);
    }

    #[test]
    fn mapping_handbrake_released_slider_is_min() {
        let s = OutputState {
            handbrake: 0.0,
            ..OutputState::default()
        };
        assert_eq!(build_position(&s, &full_caps(), 1).w_slider, 1);
    }

    #[test]
    fn mapping_no_slider_caps_handbrake_stays_neutral() {
        let s = OutputState {
            handbrake: 1.0,
            ..OutputState::default()
        };
        let caps = VJoyCapabilities::new(true, true, true, 32, false);
        let p = build_position(&s, &caps, 1);
        assert_eq!(p.w_slider, 1);
        assert_eq!(p.w_axis_x, 16384);
        assert_eq!(p.w_axis_y, 1);
        assert_eq!(p.w_axis_z, 1);
    }

    #[test]
    fn mapping_unknown_caps_handbrake_written() {
        let s = OutputState {
            handbrake: 1.0,
            ..OutputState::default()
        };
        assert_eq!(
            build_position(&s, &VJoyCapabilities::unknown(), 1).w_slider,
            32768
        );
    }

    #[test]
    fn mapping_hats_always_neutral() {
        let s = state(1.0, 1.0, 1.0);
        let p = build_position(&s, &full_caps(), 1);
        assert_eq!(p.b_hats, HAT_NEUTRAL);
        assert_eq!(p.b_hats_ex1, HAT_NEUTRAL);
        assert_eq!(p.b_hats_ex2, HAT_NEUTRAL);
        assert_eq!(p.b_hats_ex3, HAT_NEUTRAL);
    }

    // ---- capabilities masks ------------------------------------------

    // ---- validation (D11) --------------------------------------------

    #[test]
    fn validation_steering_only_valid() {
        let errors = validate_capabilities(
            &steering_only_plan(),
            &VJoyCapabilities::new(true, false, false, 0, false),
        );
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn validation_steering_only_no_x_invalid() {
        let errors = validate_capabilities(
            &steering_only_plan(),
            &VJoyCapabilities::new(false, false, false, 0, false),
        );
        assert!(
            errors
                .iter()
                .any(|e| e.contains("Steering") && e.contains('X'))
        );
    }

    #[test]
    fn validation_default_plan_full_caps_valid() {
        let errors = validate_capabilities(
            &PipelinePlan::default(),
            &VJoyCapabilities::new(true, true, true, 32, true),
        );
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn validation_default_plan_missing_axes_reports_each() {
        let errors = validate_capabilities(
            &PipelinePlan::default(),
            &VJoyCapabilities::new(true, false, false, 8, false),
        );
        assert!(
            errors
                .iter()
                .any(|e| e.contains("Accelerator") && e.contains('Y'))
        );
        assert!(
            errors
                .iter()
                .any(|e| e.contains("Brake") && e.contains('Z'))
        );
        assert!(
            errors
                .iter()
                .any(|e| e.contains("Handbrake") && e.contains("Slider"))
        );
        assert!(!errors.iter().any(|e| e.contains("Steering")));
        assert!(!errors.iter().any(|e| e.contains("Horn")));
    }

    #[test]
    fn validation_horn_disabled_no_buttons_valid() {
        let plan = plan_with(true, true, true, false, false);
        let errors =
            validate_capabilities(&plan, &VJoyCapabilities::new(true, true, true, 0, false));
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn validation_horn_enabled_no_buttons_invalid() {
        let plan = plan_with(true, false, false, false, true);
        let errors =
            validate_capabilities(&plan, &VJoyCapabilities::new(true, true, true, 0, false));
        assert!(errors.iter().any(|e| e.contains("Horn")));
    }

    #[test]
    fn validation_brake_disabled_no_z_valid() {
        let plan = plan_with(true, true, false, false, false);
        let errors =
            validate_capabilities(&plan, &VJoyCapabilities::new(true, true, false, 0, false));
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn validation_button_count_out_of_range() {
        for count in [-1, 129] {
            let errors = validate_capabilities(
                &PipelinePlan::default(),
                &VJoyCapabilities::new(true, true, true, count, true),
            );
            assert!(
                errors.iter().any(|e| e.contains("ButtonCount")),
                "count {count}: {errors:?}"
            );
        }
    }

    #[test]
    fn validation_button_count_one_with_horn_invalid() {
        // Horn is button 5: one button is not enough.
        let plan = plan_with(true, false, false, false, true);
        let errors =
            validate_capabilities(&plan, &VJoyCapabilities::new(true, false, false, 1, false));
        assert!(errors.iter().any(|e| e.contains("Horn")));
    }

    #[test]
    fn validation_button_count_five_with_horn_valid() {
        let plan = plan_with(true, false, false, false, true);
        let errors =
            validate_capabilities(&plan, &VJoyCapabilities::new(true, false, false, 5, false));
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn validation_clutch_enabled_no_extra_axis() {
        let mut plan = PipelinePlan::default();
        plan.steering.enabled = false;
        plan.accelerator.enabled = false;
        plan.brake.enabled = false;
        plan.horn.enabled = false;
        plan.handbrake.enabled = false;
        plan.shifter.enabled = false;
        plan.clutch.enabled = true;
        let errors =
            validate_capabilities(&plan, &VJoyCapabilities::new(false, false, false, 0, false));
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn validation_handbrake_enabled_with_slider_valid() {
        let plan = plan_with(false, false, false, true, false);
        let errors =
            validate_capabilities(&plan, &VJoyCapabilities::new(true, true, true, 32, true));
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn validation_handbrake_enabled_without_slider_invalid() {
        let plan = plan_with(false, false, false, true, false);
        let errors =
            validate_capabilities(&plan, &VJoyCapabilities::new(true, true, true, 32, false));
        assert!(
            errors
                .iter()
                .any(|e| e.contains("Handbrake") && e.contains("Slider"))
        );
    }

    #[test]
    fn validation_handbrake_disabled_without_slider_valid() {
        let plan = plan_with(false, false, false, false, false);
        let errors =
            validate_capabilities(&plan, &VJoyCapabilities::new(true, true, true, 32, false));
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn validation_default_plan_needs_slider() {
        let errors = validate_capabilities(
            &PipelinePlan::default(),
            &VJoyCapabilities::new(true, true, true, 32, false),
        );
        assert!(
            errors
                .iter()
                .any(|e| e.contains("Handbrake") && e.contains("Slider"))
        );
        let ok = validate_capabilities(
            &PipelinePlan::default(),
            &VJoyCapabilities::new(true, true, true, 32, true),
        );
        assert!(ok.is_empty(), "{ok:?}");
    }

    // ---- envelope validation (v2.0.14) --------------------------------

    /// Caps with a measured full envelope (128 buttons, all extra
    /// axes, 1 discrete POV, 0 continuous).
    fn envelope_caps() -> VJoyCapabilities {
        VJoyCapabilities::new(true, true, true, 128, true).with_envelope(EnvelopeCaps {
            has_axis_rx: true,
            has_axis_ry: true,
            has_axis_rz: true,
            has_axis_dial: true,
            discrete_pov_count: 1,
            continuous_pov_count: 0,
        })
    }

    #[test]
    fn envelope_full_shape_validates_clean() {
        assert_eq!(validate_envelope(&envelope_caps()), Vec::<String>::new());
    }

    #[test]
    fn envelope_reports_each_missing_piece() {
        let mut caps = envelope_caps();
        caps.button_count = 32;
        assert!(
            validate_envelope(&caps)
                .iter()
                .any(|e| e.contains("32 buttons")),
            "32 buttons reported"
        );
        let mut caps = envelope_caps();
        caps.envelope.has_axis_rx = false;
        assert!(
            validate_envelope(&caps).iter().any(|e| e.contains("Rx")),
            "missing Rx reported"
        );
        let mut caps = envelope_caps();
        caps.envelope.has_axis_dial = false;
        assert!(
            validate_envelope(&caps).iter().any(|e| e.contains("Dial")),
            "missing Dial reported"
        );
        let mut caps = envelope_caps();
        caps.envelope.discrete_pov_count = 0;
        assert!(
            validate_envelope(&caps)
                .iter()
                .any(|e| e.contains("discrete POV")),
            "no discrete POV reported"
        );
        let mut caps = envelope_caps();
        caps.envelope.continuous_pov_count = 1;
        assert!(
            validate_envelope(&caps)
                .iter()
                .any(|e| e.contains("continuous POV")),
            "continuous POV reported"
        );
    }

    #[test]
    fn envelope_unknown_pov_never_reports() {
        // `new()` caps never measured the POVs (-1) — a 128-button
        // device built the old way validates clean instead of
        // crying about an unread count.
        let caps = VJoyCapabilities::new(true, true, true, 128, true);
        assert_eq!(caps.envelope.discrete_pov_count, -1);
        assert!(validate_envelope(&caps).is_empty(), "unknown POV skipped");
    }

    #[test]
    fn connect_reports_envelope_warnings_without_refusing() {
        // Mock defaults are a default-shape device (32 buttons):
        // the connect SUCCEEDS (feeding keeps working) and the
        // warnings name the gap.
        let (mock, _handle) = MockDll::with_defaults();
        let mut out = VJoyOutput::with_dll(1, Box::new(mock));
        out.try_connect().expect("default-shape device connects");
        assert!(out.is_connected());
        assert!(
            out.envelope_warnings()
                .iter()
                .any(|w| w.contains("32 buttons")),
            "{:?}",
            out.envelope_warnings()
        );
    }

    #[test]
    fn connect_with_full_envelope_has_no_warnings() {
        let (mock, _handle) = MockDll::with_state(MockState {
            button_count: 128,
            ..MockState::default()
        });
        let mut out = VJoyOutput::with_dll(1, Box::new(mock));
        out.try_connect().expect("full-envelope device connects");
        assert!(
            out.envelope_warnings().is_empty(),
            "{:?}",
            out.envelope_warnings()
        );
    }

    #[test]
    fn clutch_feeds_rx_when_present_and_stays_neutral_without_it() {
        let s = OutputState {
            clutch: 1.0,
            ..state(0.0, 0.0, 0.0)
        };
        let p = build_position(&s, &envelope_caps(), 1);
        assert_eq!(p.w_axis_x_rot, 32768, "clutch fully pressed on Rx");
        assert_eq!(p.w_clutch, 32768, "wClutch still fed too");
        let mut caps = envelope_caps();
        caps.envelope.has_axis_rx = false;
        let p = build_position(&s, &caps, 1);
        assert_eq!(p.w_axis_x_rot, 1, "no Rx axis: Rx stays released");
        assert_eq!(p.w_clutch, 32768, "wClutch is caps-independent");
    }

    #[test]
    fn build_position_x_only_caps_other_axes_stay_neutral() {
        let s = state(1.0, 1.0, 1.0);
        let s = OutputState {
            buttons: 0b1111,
            ..s
        };
        let p = build_position(&s, &VJoyCapabilities::new(true, false, false, 0, false), 1);
        assert_eq!(p.w_axis_x, 32768);
        assert_eq!(p.w_steering, 32768);
        assert_eq!(p.w_axis_y, AXIS_MIN);
        assert_eq!(p.w_axis_z, AXIS_MIN);
        assert_eq!(p.w_accelerator, AXIS_MIN);
        assert_eq!(p.w_brake, AXIS_MIN);
        assert_eq!(p.w_slider, AXIS_MIN);
        assert_eq!(p.l_buttons, 0);
    }

    #[test]
    fn build_position_buttons_masked_to_button_count() {
        let s = OutputState {
            buttons: 0b1111,
            ..OutputState::default()
        };
        let p = build_position(&s, &VJoyCapabilities::new(true, true, true, 2, true), 1);
        assert_eq!(p.l_buttons, 0b11);
        let none = build_position(&s, &VJoyCapabilities::new(true, true, true, 0, true), 1);
        assert_eq!(none.l_buttons, 0);
    }

    // ---- lifecycle (mock DLL) ----------------------------------------

    #[test]
    fn connect_success_acquires_resets_and_sends_neutral() {
        let (mock, handle) = MockDll::with_defaults();
        let mut out = VJoyOutput::with_dll(1, Box::new(mock));
        assert_eq!(out.name(), "vJoy");
        assert!(!out.is_connected());
        out.try_connect().expect("mock connect succeeds");
        assert!(out.is_connected());
        assert_eq!(
            handle.calls(),
            vec![
                MockCall::Acquire(1),
                MockCall::Reset(1),
                MockCall::Update(1),
            ]
        );
        // The connect-time update is the neutral position.
        assert_eq!(handle.positions(), vec![neutral_position(1)]);
        let status = out.status();
        assert_eq!(status.kind, VJoyStateKind::Ready);
        assert_eq!(status.dll_version.as_deref(), Some("2.2.2"));
        assert_eq!(status.driver_version.as_deref(), Some("2.2.2"));
        assert_eq!(out.capabilities().button_count, 32);
        assert!(out.capabilities().has_axis_slider);
    }

    /// P3-9 (v2.0.12 scan): the trait surfaces the driver version
    /// `connect_with_plan` measured, so `vjoy_status()` can report
    /// it instead of always `None`.
    #[test]
    fn vjoy_driver_version_is_measured_at_connect() {
        let (mock, _) = MockDll::with_defaults();
        let mut out = VJoyOutput::with_dll(1, Box::new(mock));
        assert_eq!(out.vjoy_driver_version(), None, "nothing measured yet");
        out.try_connect().expect("mock connect succeeds");
        assert_eq!(
            out.vjoy_driver_version().as_deref(),
            Some("2.2.2"),
            "the measured driver version is surfaced"
        );
    }

    #[test]
    fn connect_when_already_owned_skips_acquire() {
        let (mock, handle) = MockDll::with_state(MockState {
            status: VjdStatus::Own,
            ..MockState::default()
        });
        let mut out = VJoyOutput::with_dll(1, Box::new(mock));
        out.try_connect().expect("owned device connects");
        assert!(out.is_connected());
        assert_eq!(
            handle.calls(),
            vec![MockCall::Reset(1), MockCall::Update(1)]
        );
    }

    #[test]
    fn connect_device_busy_errors_without_acquire() {
        let (mock, handle) = MockDll::with_state(MockState {
            status: VjdStatus::Busy,
            ..MockState::default()
        });
        let mut out = VJoyOutput::with_dll(1, Box::new(mock));
        let err = out.try_connect().expect_err("busy device must fail");
        assert_eq!(err, VJoyError::DeviceBusy(1));
        assert!(!out.is_connected());
        assert_eq!(handle.calls().len(), 0);
        assert_eq!(out.status().kind, VJoyStateKind::Busy);
    }

    #[test]
    fn connect_device_missing_errors() {
        let (mock, handle) = MockDll::with_state(MockState {
            status: VjdStatus::Missing,
            ..MockState::default()
        });
        let mut out = VJoyOutput::with_dll(1, Box::new(mock));
        let err = out.try_connect().expect_err("missing device must fail");
        assert_eq!(err, VJoyError::DeviceMissing(1));
        assert!(!out.is_connected());
        assert_eq!(handle.calls().len(), 0);
        assert_eq!(out.status().kind, VJoyStateKind::Missing);
    }

    #[test]
    fn connect_device_unknown_errors() {
        let (mock, _) = MockDll::with_state(MockState {
            status: VjdStatus::Unknown,
            ..MockState::default()
        });
        let mut out = VJoyOutput::with_dll(1, Box::new(mock));
        let err = out.try_connect().expect_err("unknown status must fail");
        assert_eq!(err, VJoyError::DeviceUnknown(1));
        assert!(!out.is_connected());
    }

    #[test]
    fn connect_acquire_failure_errors() {
        let (mock, handle) = MockDll::with_state(MockState {
            acquire_ok: false,
            ..MockState::default()
        });
        let mut out = VJoyOutput::with_dll(1, Box::new(mock));
        let err = out.try_connect().expect_err("failed acquire must fail");
        assert_eq!(err, VJoyError::AcquireFailed(1));
        assert!(!out.is_connected());
        assert_eq!(handle.calls(), vec![MockCall::Acquire(1)]);
    }

    #[test]
    fn connect_driver_disabled_errors() {
        let (mock, _) = MockDll::with_state(MockState {
            enabled: false,
            ..MockState::default()
        });
        let mut out = VJoyOutput::with_dll(1, Box::new(mock));
        let err = out.try_connect().expect_err("disabled driver must fail");
        assert_eq!(err, VJoyError::DriverDisabled);
        assert_eq!(out.status().kind, VJoyStateKind::Disabled);
        assert!(!out.is_connected());
    }

    #[test]
    fn connect_version_mismatch_errors() {
        let (mock, handle) = MockDll::with_state(MockState {
            driver_match: false,
            dll_version: 0x222,
            driver_version: 0x219,
            ..MockState::default()
        });
        let mut out = VJoyOutput::with_dll(1, Box::new(mock));
        let err = out
            .try_connect()
            .expect_err("mismatched versions must fail");
        assert_eq!(
            err,
            VJoyError::VersionMismatch {
                dll: "2.2.2".to_string(),
                driver: "2.1.9".to_string(),
            }
        );
        assert!(!out.is_connected());
        assert_eq!(handle.calls().len(), 0);
        assert_eq!(out.status().kind, VJoyStateKind::VersionMismatch);
    }

    #[test]
    fn connect_unsupported_version_errors() {
        let (mock, _) = MockDll::with_state(MockState {
            driver_match: true,
            dll_version: 0x219,
            driver_version: 0x219,
            ..MockState::default()
        });
        let mut out = VJoyOutput::with_dll(1, Box::new(mock));
        let err = out.try_connect().expect_err("old version must fail");
        assert_eq!(
            err,
            VJoyError::UnsupportedVersion {
                found: "2.1.9".to_string(),
                pinned: PINNED_DRIVER_VERSION.to_string(),
            }
        );
        assert!(!out.is_connected());
    }

    #[test]
    fn connect_with_plan_caps_fail_relinquishes() {
        let (mock, handle) = MockDll::with_state(MockState {
            axes: [true, false, false, false],
            button_count: 0,
            ..MockState::default()
        });
        let mut out = VJoyOutput::with_dll(1, Box::new(mock));
        let plan = PipelinePlan::default();
        let err = out
            .try_connect_with_plan(&plan)
            .expect_err("undersized device must fail the plan");
        assert!(matches!(err, VJoyError::InsufficientCapabilities(_)));
        assert!(err.to_string().contains("Accelerator"));
        assert!(!out.is_connected());
        // Acquired, then handed back — never left owned.
        assert_eq!(
            handle.calls(),
            vec![MockCall::Acquire(1), MockCall::Relinquish(1)]
        );
    }

    #[test]
    fn connect_with_plan_caps_ok() {
        let (mock, _) = MockDll::with_defaults();
        let mut out = VJoyOutput::with_dll(1, Box::new(mock));
        out.try_connect_with_plan(&PipelinePlan::default())
            .expect("full caps serve the default plan");
        assert!(out.is_connected());
    }

    #[test]
    fn connect_is_idempotent() {
        let (mut out, handle) = connected_output();
        out.try_connect().expect("second connect is a no-op");
        assert_eq!(
            handle.calls(),
            vec![
                MockCall::Acquire(1),
                MockCall::Reset(1),
                MockCall::Update(1),
            ]
        );
    }

    #[test]
    fn connect_invalid_device_id_rejected_before_any_call() {
        for bad_id in [0, 17] {
            let (mock, handle) = MockDll::with_defaults();
            let mut out = VJoyOutput::with_dll(bad_id, Box::new(mock));
            let err = out.try_connect().expect_err("bad device id must fail");
            assert_eq!(err, VJoyError::InvalidDeviceId(bad_id));
            assert!(handle.calls().is_empty(), "device {bad_id}");
            assert!(!out.is_connected());
        }
    }

    #[test]
    fn disconnect_sends_neutral_then_relinquishes_and_is_idempotent() {
        let (mut out, handle) = connected_output();
        out.disconnect();
        assert!(!out.is_connected());
        assert_eq!(out.status().kind, VJoyStateKind::Idle);
        let calls = handle.calls();
        assert_eq!(
            calls,
            vec![
                MockCall::Acquire(1),
                MockCall::Reset(1),
                MockCall::Update(1), // connect neutral
                MockCall::Update(1), // disconnect neutral
                MockCall::Relinquish(1),
            ]
        );
        assert_eq!(
            handle.positions().last(),
            Some(&neutral_position(1)),
            "last position sent is neutral"
        );
        out.disconnect(); // second disconnect: no more calls
        assert_eq!(handle.calls(), calls);
    }

    #[test]
    fn drop_relinquishes_device() {
        let (mock, handle) = MockDll::with_defaults();
        {
            let mut out = VJoyOutput::with_dll(1, Box::new(mock));
            out.try_connect().expect("mock connect succeeds");
            assert!(out.is_connected());
        } // `out` drops here
        assert!(
            handle.calls().contains(&MockCall::Relinquish(1)),
            "drop must hand the device back: {:?}",
            handle.calls()
        );
    }

    #[test]
    fn update_not_connected_errors() {
        let (mock, handle) = MockDll::with_defaults();
        let mut out = VJoyOutput::with_dll(1, Box::new(mock));
        let err = out
            .update(&OutputState::neutral())
            .expect_err("update before connect must fail");
        assert_eq!(err, VJoyError::NotConnected);
        assert_eq!(handle.calls().len(), 0);
    }

    #[test]
    fn update_connected_sends_mapped_position() {
        let (mut out, handle) = connected_output();
        out.update(&state(1.0, 1.0, 1.0)).expect("update succeeds");
        let positions = handle.positions();
        assert_eq!(positions.len(), 2); // connect neutral + this update
        let p = positions[1];
        assert_eq!(p.w_axis_x, 32768);
        assert_eq!(p.w_axis_y, 32768);
        assert_eq!(p.w_axis_z, 32768);
        assert_eq!(p.w_steering, 32768);
    }

    #[test]
    fn update_failure_returns_error_and_stays_connected() {
        let (mock, handle) = MockDll::with_state(MockState {
            update_ok: false,
            ..MockState::default()
        });
        let mut out = VJoyOutput::with_dll(1, Box::new(mock));
        out.try_connect()
            .expect("connect succeeds (neutral is best-effort)");
        assert!(out.is_connected());
        let err = out
            .update(&state(1.0, 0.0, 0.0))
            .expect_err("failed UpdateVJD must surface");
        assert_eq!(err, VJoyError::UpdateFailed(1));
        assert!(out.is_connected(), "a failed update must not disconnect");
        assert_eq!(handle.positions().len(), 2);
    }

    /// M2: three consecutive failed updates mark the backend
    /// disconnected (status `Error`, device relinquished); one or two
    /// failures keep the old behaviour (stay connected).
    #[test]
    fn three_consecutive_update_failures_disconnect() {
        let (mock, handle) = MockDll::with_state(MockState {
            update_ok: false,
            ..MockState::default()
        });
        let mut out = VJoyOutput::with_dll(1, Box::new(mock));
        out.try_connect()
            .expect("connect succeeds (neutral is best-effort)");
        assert!(out.is_connected());
        for _ in 0..2 {
            let err = out
                .update(&state(1.0, 0.0, 0.0))
                .expect_err("failed UpdateVJD must surface");
            assert_eq!(err, VJoyError::UpdateFailed(1));
            assert!(out.is_connected(), "1–2 failures stay connected");
        }
        let err = out
            .update(&state(1.0, 0.0, 0.0))
            .expect_err("third failure must surface");
        assert_eq!(err, VJoyError::UpdateFailed(1));
        assert!(!out.is_connected(), "3 failures disconnect");
        assert_eq!(out.status().kind, VJoyStateKind::Error);
        assert!(
            out.status().message.contains("3 consecutive"),
            "status names the cause: {}",
            out.status().message
        );
        assert!(
            handle.calls().contains(&MockCall::Relinquish(1)),
            "device relinquished on disconnect: {:?}",
            handle.calls()
        );
        assert_eq!(
            out.update(&state(0.0, 0.0, 0.0)).unwrap_err(),
            VJoyError::NotConnected
        );
    }

    /// M2: a successful update resets the consecutive-failure count —
    /// two failures, a success, then two more failures still leave
    /// the backend connected.
    #[test]
    fn successful_update_resets_consecutive_failure_count() {
        let (mock, handle) = MockDll::with_state(MockState {
            update_ok: false,
            ..MockState::default()
        });
        let mut out = VJoyOutput::with_dll(1, Box::new(mock));
        out.try_connect().expect("mock connect succeeds");
        for _ in 0..2 {
            assert!(out.update(&state(1.0, 0.0, 0.0)).is_err());
        }
        assert!(out.is_connected());
        handle.state.lock().expect("mock lock").update_ok = true;
        out.update(&state(0.0, 0.0, 0.0)).expect("update succeeds");
        handle.state.lock().expect("mock lock").update_ok = false;
        for _ in 0..2 {
            assert!(out.update(&state(1.0, 0.0, 0.0)).is_err());
        }
        assert!(out.is_connected(), "counter reset by the success");
    }

    #[test]
    fn send_neutral_when_disconnected_is_noop() {
        let (mock, handle) = MockDll::with_defaults();
        let mut out = VJoyOutput::with_dll(1, Box::new(mock));
        out.send_neutral()
            .expect("neutral while disconnected is Ok");
        assert_eq!(handle.calls().len(), 0);
        assert_eq!(handle.positions().len(), 0);
    }

    #[test]
    fn send_neutral_connected_sends_neutral_position() {
        let (mut out, handle) = connected_output();
        out.update(&state(1.0, 1.0, 1.0)).expect("update succeeds");
        assert_eq!(handle.positions().len(), 2);
        out.send_neutral().expect("neutral send succeeds");
        let positions = handle.positions();
        assert_eq!(positions.len(), 3);
        assert_eq!(positions[2], neutral_position(1));
    }

    // ---- missing-driver journey (no DLL on this machine) -------------

    #[test]
    #[cfg(unix)]
    fn driver_missing_is_a_clean_error_not_a_crash() {
        // No vJoy DLL exists on Linux: connect must fail with the
        // NotInstalled warning state, and a retry must fail the same
        // way (no panic, no fake success). Takes the env guard so a
        // parallel M1 test cannot make a fake DLL resolvable here.
        let _guard = ENV_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut out = VJoyOutput::new(1);
        let err = out.try_connect().expect_err("no driver on Linux");
        assert!(
            matches!(err, VJoyError::DriverNotFound(_)),
            "unexpected error: {err}"
        );
        assert!(err.to_string().contains("vJoyInterface.dll"));
        assert!(!out.is_connected());
        let status = out.status();
        assert_eq!(status.kind, VJoyStateKind::NotInstalled);
        assert_eq!(status.download_url, Some(VJOY_DOWNLOAD_URL));
        assert!(status.message.contains("vJoy not found"));
        let err2 = out.try_connect().expect_err("retry must fail the same way");
        assert!(matches!(err2, VJoyError::DriverNotFound(_)));
    }

    // ---- SHA-256 pin ---------------------------------------------------

    #[test]
    fn pinned_sha256_is_unmeasured_not_invented() {
        // No hash is recorded in any source. This test pins the fact:
        // fill `PINNED_DLL_SHA256_HEX` in only after measuring the
        // release-machine DLL — never guess one.
        assert!(
            PINNED_DLL_SHA256_HEX.is_none(),
            "a hash was recorded; keep this test in sync with the measurement"
        );
    }

    #[test]
    fn set_expected_sha256_validates_hex() {
        let (mock, _) = MockDll::with_defaults();
        let mut out = VJoyOutput::with_dll(1, Box::new(mock));
        assert!(out.set_expected_dll_sha256("xyz").is_err());
        assert!(out.set_expected_dll_sha256("ab").is_err());
        assert!(
            out.set_expected_dll_sha256(
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
            )
            .is_ok()
        );
    }

    #[test]
    #[cfg(unix)]
    fn expected_sha256_without_dll_path_cannot_verify() {
        // M1: hash verification is a LOAD-path concern (verify the
        // file before loading it). An injected DLL is never loaded
        // from disk, so there is nothing to verify — the pin simply
        // does not apply, and connect proceeds on the injected DLL.
        // (The old test expected `DllHashUnavailable` here; that was
        // the load-first ordering, which M1 removed.)
        let (mock, _) = MockDll::with_defaults();
        let mut out = VJoyOutput::with_dll(1, Box::new(mock));
        out.set_expected_dll_sha256(
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        )
        .expect("valid hash");
        out.try_connect()
            .expect("injected DLL skips the load-path hash check");
        assert!(out.is_connected());
    }

    fn temp_file_with(content: &[u8], name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("rsh-vjoy-test-{}-{name}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("vJoyInterface.dll");
        std::fs::write(&path, content).expect("write temp DLL stand-in");
        path
    }

    // Serialises the two tests that mutate the process environment
    // (`ProgramFiles`): the test harness runs tests in parallel, and
    // a stray `ProgramFiles` would change what the missing-driver
    // test resolves.
    #[cfg(unix)]
    static ENV_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// M1: the DLL hash is verified BEFORE the DLL is loaded, on the
    /// same resolved path. The fixture file is not a DLL at all — with
    /// a wrong pin the failure must be `DllHashMismatch` (a
    /// load-first implementation would report `LoadFailed` instead),
    /// and with the right pin the failure moves to the load step.
    #[test]
    #[cfg(unix)]
    fn dll_hash_is_verified_before_load() {
        let _guard = ENV_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let base = std::env::temp_dir().join(format!("rsh-vjoy-m1-{}", std::process::id()));
        let dll_dir = base.join("vJoy").join("x64");
        std::fs::create_dir_all(&dll_dir).expect("create fake install dir");
        let dll_path = dll_dir.join(VJOY_DLL_FILE_NAME);
        std::fs::write(&dll_path, b"not a real dll").expect("write fake DLL");
        let actual = sha256_hex(&dll_path).expect("hash the fake DLL");
        let wrong = format!(
            "{}{}",
            if actual.starts_with('0') { '1' } else { '0' },
            &actual[1..]
        );
        // SAFETY: edition-2024 env mutation is `unsafe`; the guard
        // above serialises it against the other env-mutating test,
        // and the variable is removed before this test returns.
        unsafe {
            std::env::set_var("ProgramFiles", base.to_str().expect("temp path is UTF-8"));
        }
        let mut out = VJoyOutput::new(1);
        out.set_expected_dll_sha256(&wrong)
            .expect("wrong pin is valid hex");
        let err = out.try_connect().expect_err("wrong pin must fail");
        assert!(
            matches!(err, VJoyError::DllHashMismatch { .. }),
            "M1: expected DllHashMismatch (verify-before-load), got {err}"
        );
        assert!(!out.is_connected());
        let mut out2 = VJoyOutput::new(1);
        out2.set_expected_dll_sha256(&actual)
            .expect("right pin is valid hex");
        let err2 = out2.try_connect().expect_err("fake DLL must fail to load");
        assert!(
            matches!(err2, VJoyError::LoadFailed(_)),
            "M1: right pin passes verification, load fails second: got {err2}"
        );
        unsafe {
            std::env::remove_var("ProgramFiles");
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn verify_sha256_known_vector() {
        // SHA-256("abc") — FIPS 180-4 test vector.
        let path = temp_file_with(b"abc", "sha-ok");
        let ok = verify_dll_sha256(
            &path,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        )
        .expect("verification runs");
        assert!(ok);
        // Uppercase hex + 0x prefix are accepted too.
        let ok2 = verify_dll_sha256(
            &path,
            "0xBA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD",
        )
        .expect("verification runs");
        assert!(ok2);
        let _ = std::fs::remove_dir_all(path.parent().expect("temp parent"));
    }

    #[test]
    fn verify_sha256_mismatch_is_false_not_error() {
        let path = temp_file_with(b"abc", "sha-mismatch");
        let ok = verify_dll_sha256(
            &path,
            "0000000000000000000000000000000000000000000000000000000000000000",
        )
        .expect("verification runs");
        assert!(!ok);
        let _ = std::fs::remove_dir_all(path.parent().expect("temp parent"));
    }

    #[test]
    fn verify_sha256_missing_file_and_bad_hex_error() {
        let missing = std::env::temp_dir().join("rsh-vjoy-test-no-such-file.dll");
        assert!(matches!(
            verify_dll_sha256(
                &missing,
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
            ),
            Err(VJoyError::DllHashUnavailable(_))
        ));
        let path = temp_file_with(b"abc", "sha-badhex");
        assert!(matches!(
            verify_dll_sha256(&path, "not-hex"),
            Err(VJoyError::InvalidHex(_))
        ));
        let _ = std::fs::remove_dir_all(path.parent().expect("temp parent"));
    }

    // ---- DLL resolution ------------------------------------------------

    #[test]
    fn resolve_from_env_finds_program_files() {
        let base =
            std::env::temp_dir().join(format!("rsh-vjoy-test-{}-resolve", std::process::id()));
        let dll_dir = base.join("vJoy").join("x64");
        std::fs::create_dir_all(&dll_dir).expect("create fake install dir");
        let dll = dll_dir.join(VJOY_DLL_FILE_NAME);
        std::fs::write(&dll, b"fake").expect("write fake DLL");
        let base_str = base.to_str().expect("temp path is UTF-8");
        let found = resolve_dll_path_from_env(Some(base_str), None);
        assert_eq!(found, Some(dll));
        // x86 fallback when the x64 ProgramFiles has no DLL.
        let x86 = base.join("x86");
        let x86_dll_dir = x86.join("vJoy").join("x64");
        std::fs::create_dir_all(&x86_dll_dir).expect("create fake x86 dir");
        let x86_dll = x86_dll_dir.join(VJOY_DLL_FILE_NAME);
        std::fs::write(&x86_dll, b"fake").expect("write fake x86 DLL");
        let empty = base.join("empty");
        let empty_str = empty.to_str().expect("temp path is UTF-8");
        let x86_str = x86.to_str().expect("temp path is UTF-8");
        assert_eq!(
            resolve_dll_path_from_env(Some(empty_str), Some(x86_str)),
            Some(x86_dll)
        );
        assert_eq!(resolve_dll_path_from_env(None, None), None);
        assert_eq!(
            resolve_dll_path_from_env(Some(empty_str), Some(empty_str)),
            None
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
