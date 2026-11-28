//! `rsh-output`: the `IJoystickOutput` trait and the output backends.
//!
//! The engine only ever talks to the trait, never to a concrete backend,
//! so it can be tested with a mock backend and no driver installed.
//!
//! Backends (v2.0.19): vJoy (default, hand-written FFI — see
//! [`vjoy`]; the DLL is never bundled) and Xbox via `ViGEmBus`
//! (fallback for games that do not accept a vJoy wheel — see
//! [`vigem`]; the `ViGEmBus` driver is installed separately, the
//! client DLL ships beside the daemon).
//!
//! The backend selector lives in [`selector`]: [`BackendKind`]
//! (default vJoy, fallback Xbox, `None`) +
//! [`switch_backend`] (runtime switch without restart, failed connect
//! rolls back to the previous backend, Xbox failure falls back
//! to vJoy). The shared error type is [`OutputError`] (see
//! [`error`] for why the trait itself still returns [`VJoyError`]).

pub mod error;
pub mod selector;
pub mod vigem;
pub mod vjoy;
pub mod vjoy_config;

use rsh_engine::pipeline::{OutputState, PipelinePlan};

pub use error::OutputError;
pub use selector::{BackendFactories, BackendKind, SwitchOutcome, switch_backend};
pub use vigem::{VigemError, VigemOutput};
pub use vjoy::{EnvelopeCaps, VJoyError, VJoyOutput, VJoyStateKind, VJoyStatus, validate_envelope};
pub use vjoy_config::{
    STALE_OEMDATA_SUBKEY, STALE_OEMDATA_VALUE_NAME, VJOY_DEVICE_ID, VJOY_ENVELOPE_AXES,
    VJOY_ENVELOPE_BUTTONS, VJOY_ENVELOPE_DISCRETE_POVS, VJOY_ENVELOPE_FFB_EFFECTS, VJoyConfigError,
    delete_stale_oemdata_hkcu, vjoy_config_argv,
};

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
    /// `"Xbox"`).
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
    /// it. Only an explicit re-select calls this; ordinary attempts
    /// reuse the latched state and fail fast.
    fn reset_fallback(&mut self) {}
}
