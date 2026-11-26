//! `HIDMaestro` output backend (G2 / SW-R6.3) — the default output backend.
//!
//! The Rust app never touches the `HIDMaestro` SDK (C# only). All driver
//! interaction lives in the C# `NativeAOT` sidecar
//! (`tools/rsh-hm-sidecar/`), which the Rust side drives over a
//! JSON-lines protocol — stdin/stdout on Linux and in tests, a loopback
//! TCP connect-back on Windows (see [`manager`]).
//!
//! # Design (spike G1 decision — GO, path (b))
//!
//! * Sidecar owns the lifecycle: `HMContext` → `InstallDriver()` → custom
//!   `rsh-wheel-custom` profile → `CreateController` → `SubmitState` /
//!   `SubmitRawReport` → `Dispose`. Rust never touches the SDK.
//! * Profile: custom no-brand wheel, 5 axes (X/Y/Z/Rx/Slider), 32 buttons,
//!   hat. No catalog brand profile is ever used (identity spoofing).
//! * Fallback: any sidecar failure (no admin, install fails, sidecar
//!   crash) → clean fallback to the vJoy backend (G3), never a crash,
//!   never a silent dead wheel. The manager retries **once**, then sets
//!   [`manager::SidecarManager::fallback_needed`].
//!
//! # Elevation (headline caveat — R-1)
//!
//! The `HIDMaestro` path needs an **elevated helper for the whole session**:
//! the input shared section's DACL grants Everyone only `GENERIC_READ`,
//! so only an admin process can write input frames. RSH with this backend
//! shows a **UAC prompt on every launch** (or RSH itself must run
//! elevated). vJoy needs no prompt at runtime. This is surfaced in
//! [`HidMaestroStatus::elevation_required`] and its `uac_note` copy so
//! the Output page (G4) can show it; do NOT work around it with a
//! scheduled task or service storing elevation.
//!
//! # Windows-only
//!
//! The real sidecar + driver are Windows-only. On Linux the crate builds
//! and its full test suite runs against the stub sidecar
//! (`src/bin/hm-stub-sidecar.rs`); the Windows gates H-HID-1–H-HID-8
//! (HVCI install/create/remove, 8-game matrix, E2E SLO + soak,
//! sleep/resume, MSIX path, anti-cheat spot-check, ARM64 unsupported,
//! pinned version in release notes) are owner-PC items — see
//! `README.md` for the honest checklist status.

pub mod backend;
pub mod error;
pub mod manager;
pub mod mapping;
pub mod protocol;
pub mod transport;

pub use backend::{HidMaestroConfig, HidMaestroOutput, HidMaestroStateKind, HidMaestroStatus};
pub use error::HidMaestroError;
pub use manager::{SidecarConfig, SidecarManager};
pub use mapping::{
    CUSTOM_RAW_REPORT_SIZE, GamepadFrame, HidMaestroCapabilities, build_gamepad_frame,
    build_raw_report, check_button_number, neutral_gamepad_frame, neutral_raw_report,
    validate_capabilities,
};
pub use protocol::{
    AxesJson, CapabilitiesJson, Command, Reply, bytes_to_hex, encode_command, parse_reply,
};

/// `HIDMaestro` SDK release pinned for this backend (spike §3.1, re-verified
/// 2026-11-14: still the latest release).
pub const PINNED_VERSION: &str = "2.1.0";
/// SHA-256 of `HIDMaestro-v2.1.0.zip` (release ZIP, 55,148,357 B).
pub const PINNED_ZIP_SHA256: &str =
    "7fe349f163864b8dfc36117af260d51bac94a2263b6cdfa719cb364a808397b4";
/// SHA-256 of `HIDMaestro.Core.dll` (18,163,712 B) from that ZIP.
/// Re-verified against the backup copy in this task (2026-11-14).
pub const PINNED_DLL_SHA256: &str =
    "f13e32591404db0f418af9265b8e240f71257297cfae5a41dcd696fd87e9b4d5";
/// `HIDMaestro` license (release ZIP `LICENSE` + `THIRD-PARTY-NOTICES.txt`).
pub const PINNED_LICENSE: &str = "MIT";

/// The custom no-brand wheel profile id (built at runtime in the sidecar
/// with `HMProfileBuilder` + `HidDescriptorBuilder`).
pub const PROFILE_ID: &str = "rsh-wheel-custom";
/// Identity key for the created controller (same device back on restart).
pub const IDENTITY_KEY: &str = "rsh-wheel";
/// Profile vendor string (no brand identity).
pub const PROFILE_VENDOR: &str = "RSH";
/// Profile product string (no brand identity).
pub const PROFILE_PRODUCT: &str = "RSH Wheel";

/// USB Vendor ID: pid.codes open-source pool (`0x1209`).
///
/// pid.codes assigns this VID to open-source hardware projects; RSH is
/// MIT-licensed, so it qualifies. See [`PROFILE_PID`] for the PID story.
pub const PROFILE_VID: u16 = 0x1209;
/// USB Product ID: `0x5253` — ASCII `"RS"` (R = 0x52, S = 0x53).
///
/// **PROVISIONAL — release gate, not a final allocation.** The spike's
/// placeholder `0x0001` is unusable: pid.codes lists `0x0001` as its own
/// "pid.codes Test PID", and PIDs `0x0000–0x0FFF` are reserved for common
/// tasks/testing while `0x1000–0x1FFF` belong to `InterBiometrics` (verified
/// against the live <https://pid.codes/1209/> list, 2026-11-14).
/// `0x5253` is absent from that list, is not a brand VID/PID, and carries
/// no brand identity — but a PID is only really RSH's once pid.codes
/// accepts the allocation PR. **Before v1.0.0 ships, a PR to
/// `pid.codes` claiming `0x5253` for "RSH — Real Sim Hub Wheel" must be
/// merged; until then this constant is provisional** (and the sidecar
/// reports it as such in `version`/`describe-profile` replies).
/// If the PR is refused or the PID is taken first, change this one
/// constant (both crates read it from here / the sidecar constant) and
/// re-verify the game matrix — the VID/PID is a device identity, not
/// wire state.
pub const PROFILE_PID: u16 = 0x5253;

/// Sidecar executable file name (shipped beside the Tauri app).
#[cfg(windows)]
pub const SIDECAR_EXE_NAME: &str = "RshCoreHiDcr.exe";
/// Sidecar executable file name (dev/test builds on non-Windows).
#[cfg(not(windows))]
pub const SIDECAR_EXE_NAME: &str = "RshCoreHiDcr";

/// JSON-lines protocol version spoken by Rust and the sidecar.
/// Bump only with a matching sidecar change; the manager refuses a
/// hello whose `protocol` is newer than this.
pub const PROTOCOL_VERSION: u32 = 1;

/// The loopback token is NOT carried in an environment variable
/// (v2.0.12, H1 — PROVEN on the owner's PC): a non-elevated daemon
/// launches the elevated sidecar via `runas`, and AIS builds a FRESH
/// environment for the elevated child, dropping any runtime-set
/// variable — so the sidecar logged `bad-args` and exited 2 before
/// connecting. The manager now passes the one-time nonce on the
/// command line (`--token <nonce>`, see [`manager`]); it is
/// single-use random and briefly visible in the process list / WMI —
/// accepted. It is never logged anywhere (presence only, never the
/// value). There is deliberately no `SIDECAR_TOKEN_ENV` constant
/// anymore — do not reintroduce an env transport.
pub const SIDECAR_TOKEN_ARG: &str = "--token";

/// Environment variable overriding the sidecar path (dev/tests only).
pub const SIDECAR_PATH_ENV: &str = "RSH_HM_SIDECAR";
