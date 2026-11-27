//! `rsh-output`: the `IJoystickOutput` trait and the output backends.
//!
//! The engine only ever talks to the trait, never to a concrete backend,
//! so it can be tested with a mock backend and no driver installed.
//!
//! Backends: `HIDMaestro` (default, interop path decided by the Phase G
//! spike — G2) and vJoy (fallback, hand-written FFI — G3, see
//! [`vjoy`]; the DLL is never bundled).
//!
//! The backend selector lives in [`selector`] (G4): [`BackendKind`]
//! (default `HIDMaestro`, fallback vJoy, `None`) +
//! [`switch_backend`] (runtime switch without restart, failed connect
//! rolls back to the previous backend, `HIDMaestro` failure falls back
//! to vJoy). The shared error type is [`OutputError`] (see
//! [`error`] for why the trait itself still returns [`VJoyError`]).

pub mod error;
pub mod selector;
pub mod vjoy;
pub mod vjoy_config;
pub mod wheel_check;

use rsh_engine::pipeline::{OutputState, PipelinePlan};

pub use error::OutputError;
pub use selector::{BackendFactories, BackendKind, SwitchOutcome, switch_backend};
pub use vjoy::{EnvelopeCaps, VJoyError, VJoyOutput, VJoyStateKind, VJoyStatus, validate_envelope};
pub use vjoy_config::{
    STEAM_FH4_MARKER_VALUE, VJOY_FH4_DEVICE_ID, VJoyConfigError, WHEEL_FLAG_BYTES,
    WHEEL_FLAG_OEM_SUBKEY, set_wheel_flag, set_wheel_flag_and_clear_marker,
    steam_fh4_marker_present, vjoy_config_argv, wheel_flag_script,
};
pub use wheel_check::{WheelCheckResult, mapping_entries, run_wheel_check};

/// The processed state the engine hands to an output backend.
///
/// The plans call this type `ProcessedState`; the engine type is
/// [`OutputState`] (`rsh_engine::pipeline`). They are the same type —
/// this alias exists so backend code can use the plan's name.
pub type ProcessedState = OutputState;

/// Joystick output: the one door every backend goes through.
///
/// The engine calls [`update`](IJoystickOutput::update) at 200 Hz and
/// [`send_neutral`](IJoystickOutput::send_neutral) when the watchdog
/// goes stale or the engine stops. An update error never kills the
/// loop: the engine counts it, falls back to neutral, and keeps
/// ticking (same contract as the engine's `OutputSink`).
pub trait IJoystickOutput: Send {
    /// Backend name for logs and the Output page (`"vJoy"`,
    /// `"HIDMaestro"`).
    fn name(&self) -> &'static str;

    /// True after a successful [`try_connect`](IJoystickOutput::try_connect)
    /// until [`disconnect`](IJoystickOutput::disconnect).
    fn is_connected(&self) -> bool;

    /// Acquire the device. On failure the returned error carries the
    /// human-readable reason (the C# `out string error`); the app
    /// never crashes on a missing driver — output is simply disabled
    /// and the Dashboard shows the warning state.
    ///
    /// # Errors
    ///
    /// Returns [`VJoyError`] (or the backend's equivalent) when the
    /// driver is missing, disabled, version-mismatched, busy, or the
    /// device cannot be acquired.
    fn try_connect(&mut self) -> Result<(), VJoyError>;

    /// Connect with the enabled modules validated against the device's
    /// capabilities (D11): a steering-only wheel is valid, a disabled
    /// module imposes no requirement. On validation failure the device
    /// is relinquished and the error names the missing capability.
    /// The default implementation ignores the plan and calls
    /// [`try_connect`](IJoystickOutput::try_connect) (test fakes and
    /// backends without capability data).
    ///
    /// # Errors
    ///
    /// Same as [`try_connect`](IJoystickOutput::try_connect), plus a
    /// capabilities error when the device cannot serve the plan.
    fn try_connect_with_plan(&mut self, plan: &PipelinePlan) -> Result<(), VJoyError> {
        let _ = plan;
        self.try_connect()
    }

    /// The vJoy driver version (`"2.2.2"` form) this backend
    /// measured at connect, when this backend IS vJoy and has
    /// connected. The default is [`None`] — only vJoy measures a
    /// vJoy driver version (P3-9, v2.0.12 scan:
    /// `vjoy_status().version` was always `None` because nothing
    /// ever surfaced the measured version). The app core caches
    /// the last measured value so the version survives a switch
    /// away to another backend.
    fn vjoy_driver_version(&self) -> Option<String> {
        None
    }

    /// Best-effort neutral send, then release the device. Never
    /// panics; calling it while disconnected is a no-op.
    fn disconnect(&mut self);

    /// Send one processed tick (200 Hz).
    ///
    /// # Errors
    ///
    /// Returns an error when not connected or when the driver refuses
    /// the update. The caller (engine) treats this as "send neutral
    /// and keep ticking", never as fatal.
    fn update(&mut self, state: &ProcessedState) -> Result<(), VJoyError>;

    /// Send the safe neutral state (watchdog / shutdown). A no-op
    /// while disconnected; best-effort while connected (a failed
    /// neutral send is swallowed, never fatal).
    ///
    /// # Errors
    ///
    /// Backends report errors here only for programming mistakes
    /// (e.g. misuse after a failed connect); driver failures during
    /// the neutral send itself are swallowed by design.
    fn send_neutral(&mut self) -> Result<(), VJoyError>;

    /// Clear a latched failure state on an explicit user re-select
    /// (P2, v2.0.11). Default: no-op — backends without a latch keep
    /// it. `HIDMaestro` overrides it to clear its sidecar manager's
    /// fallback / UAC-decline latches IN PLACE, so the re-select
    /// retries with the SAME manager instead of a fresh factory build
    /// (a fresh build used to defeat the manager's 2-failure latch
    /// and clear `elevation_declined` on every attempt — the retry
    /// storm's enabler). Only an explicit re-select calls this;
    /// ordinary attempts reuse the latched manager and fail fast.
    fn reset_fallback(&mut self) {}

    /// Steam FH4 wheel mode (FH4 fix, v2.0.15): when on, the
    /// backend dedupes idle updates (100 ms keepalive) and applies
    /// a 32-count per-axis hysteresis to idle noise, so a wheel
    /// game no longer sees a constantly-twitching controller. The
    /// default is a no-op — only vJoy implements it (the feed that
    /// needs quieting is the vJoy one); `HIDMaestro` ignores it.
    /// `AppStateCore` pushes the persisted mode into the live
    /// backend on every connect and on every toggle, so a backend
    /// that (re)connects picks the current mode up without the
    /// caller remembering to re-push it.
    fn set_steam_fh4_mode(&mut self, _enabled: bool) {}

    /// `G29` spoof mode (v2.0.16): when on, the backend presents the
    /// `HIDMaestro` built-in `logitech-g29` catalog profile (VID
    /// `0x046D` / PID `0xC24F`) instead of the custom no-brand wheel.
    /// The default is a no-op — only `HIDMaestro` implements it
    /// (`vJoy` has no `HIDMaestro` profile to switch; the daemon refuses
    /// enabling on any other backend with `g29_spoof_hid_only`).
    /// `HIDMaestro` recreates the controller with the right profile
    /// when the mode changes while connected; a disconnected backend
    /// just records the flag for its next connect. See
    /// `HidMaestroOutput::set_g29_spoof_mode` for the restart
    /// semantics. RISK: presenting Logitech's VID/PID violates
    /// USB-IF policy as written (no enforcement found for private
    /// hobby use); FH4 end-to-end is unproven — do not promise
    /// auto-recognition.
    fn set_g29_spoof_mode(&mut self, _enabled: bool) {}
}
