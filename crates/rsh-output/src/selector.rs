//! Backend selector (G4 / SW-R6.1) — runtime output-backend switching.
//!
//! The engine and the app state only ever hold a
//! `Box<dyn `[`IJoystickOutput`]`>`; this module owns the *choice* of
//! which backend that box holds, and the switch between them. It never
//! constructs a real backend itself: the caller supplies factories
//! ([`BackendFactories`]), so the selector is testable with mock
//! backends and no driver installed, and so `rsh-output` stays free of
//! a dependency on any concrete backend crate (a reverse edge would
//! be circular).
//!
//! # Rules
//!
//! * **Default is `vJoy`** ([`BackendKind::DEFAULT`]); `Xbox`
//!   (`ViGEmBus`) is the fallback choice for games that do not take
//!   the vJoy wheel; `None` is the no-device sink. vJoy needs no
//!   elevation after its one-time install; Xbox needs the `ViGEmBus`
//!   driver installed (the wizard offers it).
//! * **Switch without restart where possible:** the switch below runs
//!   on the live backend slot — no process restart, no engine restart.
//!   The engine keeps ticking against whatever backend the slot holds
//!   (a failed send is "neutral and keep ticking", per the engine
//!   contract).
//! * **Connect-first, then disconnect:** the new backend is connected
//!   *before* the old one is released. A failed connect therefore
//!   leaves the previous backend exactly as it was — installed and in
//!   its previous connected state — never half-connected and never
//!   dropped. The old backend is only disconnected after the new one
//!   is live (its `disconnect` sends neutral first, per the trait).
//! * **Fallback:** a failed `Xbox` connect falls back to vJoy -
//!   but ONLY when the caller allows it ([`switch_backend`]'s
//!   `allow_fallback`): automatic paths (startup activation,
//!   deferred enable, mid-session failover) allow it, an explicit
//!   user switch does NOT. An explicit switch whose requested
//!   backend fails to connect errors with the real reason instead
//!   of silently landing on vJoy (Bug 4/8: the user asked for
//!   `Xbox`, the UI must never toast "now vJoy"). The switch
//!   outcome records the reason verbatim
//!   ([`SwitchOutcome::fallback_reason`]) when a fallback does
//!   happen. The selector does **not** branch on a `VJoyError`
//!   variant to identify an `Xbox` failure (see
//!   [`error`](crate::error) for why): any `Xbox` connect failure
//!   triggers the fallback attempt. If vJoy also fails, the switch
//!   errors and the previous backend stays installed - the app
//!   keeps running with output as it was.
//! * **One live backend at a time:** after a successful switch exactly
//!   one backend is connected; the previous one is disconnected.
//!   (Both factories may exist in parallel — the switch test builds
//!   both — but only the active one is ever connected.)
//! * **Persistence is the caller's job:** [`switch_backend`] switches
//!   the slot; the caller (`AppStateCore` in `rsh-app-core`) persists
//!   the requested choice in settings. The selector never touches the
//!   settings store.

use rsh_engine::pipeline::PipelinePlan;

use crate::IJoystickOutput;
use crate::error::OutputError;

/// The output backend the user can select (Settings/Output page).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BackendKind {
    /// Xbox via `ViGEmBus` (v2.0.19) - the fallback backend for games
    /// that do not take the vJoy wheel. Needs the `ViGEmBus` driver;
    /// never the default.
    Xbox,
    /// vJoy - the default backend (G3, FFI, no elevation needed).
    VJoy,
    /// No output device — the no-device sink. The wheel sends nothing
    /// to games.
    None,
}

impl BackendKind {
    /// The default backend: `vJoy`.
    pub const DEFAULT: Self = Self::VJoy;

    /// Parse a user- or settings-supplied backend name
    /// (case-insensitive; `"null"` / `"NullSink"` also mean [`None`](BackendKind::None)).
    /// Returns `None` for an unknown name — the caller errors honestly
    /// instead of guessing.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "xbox" | "xbox360" | "xbox (vigembus)" => Some(Self::Xbox),
            "vjoy" | "v-joy" => Some(Self::VJoy),
            "none" | "null" | "nullsink" => Some(Self::None),
            _ => None,
        }
    }

    /// Identify the kind of an installed backend from the name its
    /// [`IJoystickOutput::name`] reports (`"Xbox"`, `"vJoy"`,
    /// `"NullSink"`). Returns `None` for a backend this selector does
    /// not know (e.g. a test recording backend) — the switch still
    /// works, it just cannot take the same-kind shortcut for it.
    #[must_use]
    pub fn from_backend_name(name: &str) -> Option<Self> {
        Self::parse(name)
    }

    /// Short id for logs, diagnostics, and the frontend (`"xbox"`
    /// / `"vjoy"` / `"none"`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Xbox => "xbox",
            Self::VJoy => "vjoy",
            Self::None => "none",
        }
    }

    /// Display name exactly as the backend reports it.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Xbox => "Xbox (ViGEmBus)",
            Self::VJoy => "vJoy",
            Self::None => "None",
        }
    }

    /// The value persisted in `AppSettings.output_backend`
    /// (`"Xbox"` / `"VJoy"` / `"None"` - the spellings the settings
    /// store has always used, not the backends' own `name()` strings).
    /// A persisted `"HIDMaestro"` (deleted in v2.0.19) parses to
    /// `None`, and the caller falls back to [`BackendKind::DEFAULT`].
    #[must_use]
    pub const fn settings_value(self) -> &'static str {
        match self {
            Self::Xbox => "Xbox",
            Self::VJoy => "VJoy",
            Self::None => "None",
        }
    }

    /// True for the default backend ([`BackendKind::DEFAULT`]).
    #[must_use]
    pub const fn is_default(self) -> bool {
        matches!(self, Self::VJoy)
    }

    /// Fallback target when this backend's connect fails:
    /// `Xbox` -> vJoy; anything else -> no fallback.
    #[must_use]
    pub const fn fallback(self) -> Option<Self> {
        match self {
            Self::Xbox => Some(Self::VJoy),
            Self::VJoy | Self::None => None,
        }
    }
}

/// Factories for the three backends. The caller supplies them; the
/// selector only calls them.
///
/// `None`'s factory must always be present (the no-device sink is the
/// safe parking state); a missing `Xbox` / vJoy factory means
/// that backend is not installed on this machine, and requesting it
/// errors honestly instead of building a fake.
pub struct BackendFactories {
    /// Factory for the `Xbox` (`ViGEmBus`) backend, when installed.
    pub xbox: Option<Box<dyn Fn() -> Box<dyn IJoystickOutput> + Send>>,
    /// Factory for the vJoy backend, when installed.
    pub vjoy: Option<Box<dyn Fn() -> Box<dyn IJoystickOutput> + Send>>,
    /// Factory for the no-device sink. Always present.
    pub none: Box<dyn Fn() -> Box<dyn IJoystickOutput> + Send>,
}

impl BackendFactories {
    /// Build a fresh backend of `kind`, or `None` when no factory for
    /// it is installed.
    #[must_use]
    pub fn build(&self, kind: BackendKind) -> Option<Box<dyn IJoystickOutput>> {
        match kind {
            BackendKind::Xbox => self.xbox.as_ref().map(|build| build()),
            BackendKind::VJoy => self.vjoy.as_ref().map(|build| build()),
            BackendKind::None => Some((self.none)()),
        }
    }
}

/// The outcome of one [`switch_backend`] call: what is live now, and
/// how it got there. The frontend renders `active` (never `requested`)
/// as the live backend, and shows [`SwitchOutcome::fallback_reason`]
/// when [`SwitchOutcome::fell_back`] is set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwitchOutcome {
    /// The backend that was requested.
    pub requested: BackendKind,
    /// The backend that is actually live now (differs from `requested`
    /// after a fallback).
    pub active: BackendKind,
    /// True when the installed backend identity changed.
    pub switched: bool,
    /// True when the requested backend failed and the fallback backend
    /// was installed instead.
    pub fell_back: bool,
    /// The requested backend's connect failure, verbatim, when a
    /// fallback happened.
    pub fallback_reason: Option<String>,
    /// Human-readable summary of the switch (shown in diagnostics).
    pub detail: String,
}

/// Connect one backend, with the plan's capabilities validated when a
/// plan is given (D11). The trait error is converted to the shared
/// [`OutputError`] exactly (see [`error`](crate::error)).
///
/// # Errors
///
/// Returns the connect failure as [`OutputError`].
fn connect(
    backend: &mut dyn IJoystickOutput,
    plan: Option<&PipelinePlan>,
) -> Result<(), OutputError> {
    match plan {
        Some(plan) => backend
            .try_connect_with_plan(plan)
            .map_err(OutputError::from),
        None => backend.try_connect().map_err(OutputError::from),
    }
}

/// Switch the live backend slot to `requested`, without a restart.
///
/// Connect-first, then disconnect (see the module docs): a failed
/// connect leaves the previous backend installed and in its previous
/// state. When `allow_fallback` is true, a failed `Xbox` connect
/// falls back to vJoy; when it is false (an explicit user switch),
/// the failure errors with the real reason and the slot is untouched
/// - never a silent fallback. If vJoy also fails, the error names
///   both attempts and the previous backend is untouched.
///
/// # Errors
///
/// Returns [`OutputError::BackendNotAvailable`] when the requested
/// backend has no factory, or [`OutputError::SwitchFailed`] when the
/// requested backend (and its fallback, if allowed) could not
/// connect. On error the slot is unchanged.
pub fn switch_backend(
    current: &mut Box<dyn IJoystickOutput>,
    requested: BackendKind,
    factories: &BackendFactories,
    plan: Option<&PipelinePlan>,
    allow_fallback: bool,
) -> Result<SwitchOutcome, OutputError> {
    let current_kind = BackendKind::from_backend_name(current.name());

    // Requesting "none" while the sink is installed: nothing to do,
    // and the sink has no connect step (its `try_connect` always
    // fails honestly — there is no device to acquire).
    if requested == BackendKind::None && current_kind == Some(BackendKind::None) {
        return Ok(SwitchOutcome {
            requested,
            active: BackendKind::None,
            switched: false,
            fell_back: false,
            fallback_reason: None,
            detail: "No output backend selected — output stays disabled.".to_string(),
        });
    }

    // Same backend, disconnected: reconnect the installed instance
    // (no rebuild — the device, caps, and sidecar session survive).
    if current_kind == Some(requested) {
        if current.is_connected() {
            return Ok(SwitchOutcome {
                requested,
                active: requested,
                switched: false,
                fell_back: false,
                fallback_reason: None,
                detail: format!(
                    "{} is already the active backend.",
                    requested.display_name()
                ),
            });
        }
        return reconnect_or_fallback(current, requested, factories, plan, allow_fallback);
    }

    // A fresh backend: build it, connect it, and only then release the
    // old one. Any failure below leaves `current` untouched.
    if requested == BackendKind::None {
        let next = factories
            .build(BackendKind::None)
            .ok_or_else(|| OutputError::BackendNotAvailable("no-device sink".to_string()))?;
        current.disconnect();
        *current = next;
        return Ok(SwitchOutcome {
            requested,
            active: BackendKind::None,
            switched: true,
            fell_back: false,
            fallback_reason: None,
            detail: "Output backend removed — output is disabled.".to_string(),
        });
    }
    let mut next = factories.build(requested).ok_or_else(|| {
        OutputError::BackendNotAvailable(format!(
            "{} is not installed on this machine",
            requested.display_name()
        ))
    })?;
    match connect(next.as_mut(), plan) {
        Ok(()) => {
            current.disconnect();
            *current = next;
            Ok(SwitchOutcome {
                requested,
                active: requested,
                switched: true,
                fell_back: false,
                fallback_reason: None,
                detail: format!("Output backend switched to {}.", requested.display_name()),
            })
        }
        Err(error) => {
            // The new backend failed to connect: it is dropped
            // (disconnected, never installed). With fallback
            // allowed the fallback runs; without it (an explicit
            // user switch) the failure is the error — the slot
            // stays exactly as it was, never silently on vJoy.
            next.disconnect();
            drop(next);
            if allow_fallback {
                try_fallback(current, requested, factories, plan, &error)
            } else {
                Err(OutputError::SwitchFailed(format!(
                    "{} could not connect: {error}",
                    requested.display_name()
                )))
            }
        }
    }
}

/// Reconnect the installed backend (same-kind switch while
/// disconnected), falling back when it is `Xbox`, the
/// reconnect fails, and `allow_fallback` is true. With fallback
/// disallowed the reconnect failure errors and the slot is
/// unchanged.
///
/// # Errors
///
/// Returns [`OutputError::SwitchFailed`] when the reconnect and its
/// fallback (if allowed) both fail; the slot is unchanged then.
fn reconnect_or_fallback(
    current: &mut Box<dyn IJoystickOutput>,
    requested: BackendKind,
    factories: &BackendFactories,
    plan: Option<&PipelinePlan>,
    allow_fallback: bool,
) -> Result<SwitchOutcome, OutputError> {
    match connect(current.as_mut(), plan) {
        Ok(()) => Ok(SwitchOutcome {
            requested,
            active: requested,
            switched: false,
            fell_back: false,
            fallback_reason: None,
            detail: format!("{} reconnected.", requested.display_name()),
        }),
        Err(error) => {
            if allow_fallback {
                try_fallback(current, requested, factories, plan, &error)
            } else {
                Err(OutputError::SwitchFailed(format!(
                    "{} could not connect: {error}",
                    requested.display_name()
                )))
            }
        }
    }
}

/// After `requested` failed to connect with `error`: try its fallback
/// backend (vJoy for `Xbox`). On fallback success the fallback
/// backend is installed and the old backend released, with the reason
/// recorded. On fallback failure the error names both attempts and the
/// slot is unchanged.
///
/// # Errors
///
/// Returns [`OutputError::SwitchFailed`] when the fallback (or the
/// original backend, when it has none) could not connect.
fn try_fallback(
    current: &mut Box<dyn IJoystickOutput>,
    requested: BackendKind,
    factories: &BackendFactories,
    plan: Option<&PipelinePlan>,
    error: &OutputError,
) -> Result<SwitchOutcome, OutputError> {
    let Some(fallback_kind) = requested.fallback() else {
        return Err(OutputError::SwitchFailed(format!(
            "{} could not connect: {error}",
            requested.display_name()
        )));
    };
    let reason = error.to_string();

    // The fallback backend is already the installed one (e.g. the user
    // asked for Xbox while vJoy is live): reconnect it in place
    // instead of building a second vJoy device that would collide with
    // the first one's acquire.
    if BackendKind::from_backend_name(current.name()) == Some(fallback_kind) {
        return match connect(current.as_mut(), plan) {
            Ok(()) => Ok(SwitchOutcome {
                requested,
                active: fallback_kind,
                switched: false,
                fell_back: true,
                fallback_reason: Some(reason),
                detail: format!(
                    "{} could not connect, fell back to {}.",
                    requested.display_name(),
                    fallback_kind.display_name()
                ),
            }),
            Err(fallback_error) => Err(OutputError::SwitchFailed(format!(
                "{} could not connect: {reason}; vJoy fallback also failed: {fallback_error}",
                requested.display_name()
            ))),
        };
    }

    let Some(mut fallback) = factories.build(fallback_kind) else {
        return Err(OutputError::SwitchFailed(format!(
            "{} could not connect: {reason}; vJoy fallback is not installed",
            requested.display_name()
        )));
    };
    match connect(fallback.as_mut(), plan) {
        Ok(()) => {
            current.disconnect();
            *current = fallback;
            Ok(SwitchOutcome {
                requested,
                active: fallback_kind,
                switched: true,
                fell_back: true,
                fallback_reason: Some(reason),
                detail: format!(
                    "{} could not connect, fell back to {}.",
                    requested.display_name(),
                    fallback_kind.display_name()
                ),
            })
        }
        Err(fallback_error) => {
            fallback.disconnect();
            Err(OutputError::SwitchFailed(format!(
                "{} could not connect: {reason}; vJoy fallback also failed: {fallback_error}",
                requested.display_name()
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use crate::ProcessedState;
    use crate::vjoy::VJoyError;

    /// Scripted mock backend: `fail_times` connect attempts fail with
    /// the scripted message, later attempts succeed. Every call is
    /// counted through the shared state so tests can assert exactly
    /// which backend was connected / disconnected, and when.
    #[derive(Debug)]
    struct MockState {
        connected: bool,
        connect_calls: usize,
        disconnect_calls: usize,
        fail_times: usize,
        fail_message: String,
    }

    #[derive(Clone, Debug)]
    struct MockBackend {
        name: &'static str,
        state: Arc<Mutex<MockState>>,
    }

    impl MockBackend {
        fn new(name: &'static str) -> (Self, Arc<Mutex<MockState>>) {
            let state = Arc::new(Mutex::new(MockState {
                connected: false,
                connect_calls: 0,
                disconnect_calls: 0,
                fail_times: 0,
                fail_message: "mock connect failed".to_string(),
            }));
            (
                Self {
                    name,
                    state: Arc::clone(&state),
                },
                state,
            )
        }

        fn with_failures(
            name: &'static str,
            fail_times: usize,
            fail_message: &str,
        ) -> (Self, Arc<Mutex<MockState>>) {
            let (backend, state) = Self::new(name);
            {
                let mut guard = state.lock().expect("mock lock poisoned");
                guard.fail_times = fail_times;
                guard.fail_message = fail_message.to_string();
            }
            (backend, state)
        }

        fn boxed(self) -> Box<dyn IJoystickOutput> {
            Box::new(self)
        }
    }

    impl IJoystickOutput for MockBackend {
        fn name(&self) -> &'static str {
            self.name
        }

        fn is_connected(&self) -> bool {
            self.state.lock().expect("mock lock poisoned").connected
        }

        fn try_connect(&mut self) -> Result<(), VJoyError> {
            let mut guard = self.state.lock().expect("mock lock poisoned");
            guard.connect_calls += 1;
            if guard.fail_times > 0 {
                guard.fail_times -= 1;
                return Err(VJoyError::DriverNotFound(guard.fail_message.clone()));
            }
            guard.connected = true;
            Ok(())
        }

        fn disconnect(&mut self) {
            let mut guard = self.state.lock().expect("mock lock poisoned");
            guard.disconnect_calls += 1;
            guard.connected = false;
        }

        fn update(&mut self, _state: &ProcessedState) -> Result<(), VJoyError> {
            if self.is_connected() {
                Ok(())
            } else {
                Err(VJoyError::NotConnected)
            }
        }

        fn send_neutral(&mut self) -> Result<(), VJoyError> {
            Ok(())
        }
    }

    /// A factory that builds a fresh mock backend per call and counts
    /// builds (the selector must not rebuild the installed backend on
    /// a same-kind reconnect).
    struct MockFactory {
        name: &'static str,
        builds: Arc<AtomicUsize>,
        failures: Arc<Mutex<VecDeque<(usize, String)>>>,
    }

    impl MockFactory {
        fn new(name: &'static str) -> Self {
            Self {
                name,
                builds: Arc::new(AtomicUsize::new(0)),
                failures: Arc::new(Mutex::new(VecDeque::new())),
            }
        }

        fn fail_next(&self, times: usize, message: &str) {
            self.failures
                .lock()
                .expect("factory lock poisoned")
                .push_back((times, message.to_string()));
        }

        fn factory(&self) -> Box<dyn Fn() -> Box<dyn IJoystickOutput> + Send> {
            let name = self.name;
            let builds = Arc::clone(&self.builds);
            let failures = Arc::clone(&self.failures);
            Box::new(move || {
                builds.fetch_add(1, Ordering::SeqCst);
                let scripted = failures.lock().expect("factory lock poisoned").pop_front();
                match scripted {
                    Some((times, message)) => {
                        MockBackend::with_failures(name, times, &message).0.boxed()
                    }
                    None => MockBackend::new(name).0.boxed(),
                }
            })
        }
    }

    fn factories_with(xbox: &MockFactory, vjoy: &MockFactory) -> BackendFactories {
        BackendFactories {
            xbox: Some(xbox.factory()),
            vjoy: Some(vjoy.factory()),
            none: Box::new(|| MockBackend::new("NullSink").0.boxed()),
        }
    }

    fn state_of(state: &Arc<Mutex<MockState>>) -> (bool, usize, usize) {
        let guard = state.lock().expect("mock lock poisoned");
        (guard.connected, guard.connect_calls, guard.disconnect_calls)
    }

    #[test]
    fn default_backend_is_vjoy() {
        assert_eq!(BackendKind::DEFAULT, BackendKind::VJoy);
        assert!(BackendKind::VJoy.is_default());
        assert!(!BackendKind::Xbox.is_default());
        assert_eq!(BackendKind::Xbox.fallback(), Some(BackendKind::VJoy));
        assert_eq!(BackendKind::VJoy.fallback(), None);
        assert_eq!(BackendKind::None.fallback(), None);
        assert_eq!(BackendKind::Xbox.settings_value(), "Xbox");
        assert_eq!(BackendKind::VJoy.settings_value(), "VJoy");
        assert_eq!(BackendKind::None.settings_value(), "None");
    }

    #[test]
    fn backend_kind_parses_user_and_settings_names() {
        assert_eq!(BackendKind::parse("Xbox"), Some(BackendKind::Xbox));
        assert_eq!(BackendKind::parse("xbox"), Some(BackendKind::Xbox));
        assert_eq!(BackendKind::parse("vJoy"), Some(BackendKind::VJoy));
        assert_eq!(BackendKind::parse("VJOY"), Some(BackendKind::VJoy));
        assert_eq!(BackendKind::parse("none"), Some(BackendKind::None));
        assert_eq!(BackendKind::parse("null"), Some(BackendKind::None));
        assert_eq!(BackendKind::parse("NullSink"), Some(BackendKind::None));
        assert_eq!(BackendKind::parse("Sideways"), None);
        assert_eq!(BackendKind::parse(""), None);
        // v2.0.19 migration: the deleted backend no longer parses -
        // the caller falls back to the default (vJoy) instead.
        assert_eq!(BackendKind::parse("HIDMaestro"), None);
        assert_eq!(BackendKind::parse("hidmaestro"), None);
        assert_eq!(
            BackendKind::from_backend_name("Xbox"),
            Some(BackendKind::Xbox)
        );
        assert_eq!(BackendKind::from_backend_name("RecordingBackend"), None);
    }

    #[test]
    fn switch_none_to_xbox_and_back() {
        let hm = MockFactory::new("Xbox");
        let vjoy = MockFactory::new("vJoy");
        let factories = factories_with(&hm, &vjoy);
        let mut current = MockBackend::new("NullSink").0.boxed();

        let outcome = switch_backend(&mut current, BackendKind::Xbox, &factories, None, true)
            .expect("Xbox switch succeeds");
        assert_eq!(outcome.active, BackendKind::Xbox);
        assert!(outcome.switched);
        assert!(!outcome.fell_back);
        assert!(current.is_connected());
        assert_eq!(current.name(), "Xbox");

        let outcome = switch_backend(&mut current, BackendKind::None, &factories, None, true)
            .expect("switch to none succeeds");
        assert_eq!(outcome.active, BackendKind::None);
        assert!(outcome.switched);
        assert!(!current.is_connected());
    }

    #[test]
    fn switch_both_backends_in_parallel_exactly_one_connected() {
        // Both factories exist at once (both backends built in
        // parallel); the switch still leaves exactly one connected.
        let hm = MockFactory::new("Xbox");
        let vjoy = MockFactory::new("vJoy");
        let factories = factories_with(&hm, &vjoy);
        let mut current = MockBackend::new("NullSink").0.boxed();

        let outcome = switch_backend(&mut current, BackendKind::Xbox, &factories, None, true)
            .expect("Xbox switch succeeds");
        assert_eq!(outcome.active, BackendKind::Xbox);

        // Xbox -> vJoy: the Xbox instance is disconnected,
        // the vJoy instance connected. Hold the counts via fresh mocks
        // is impossible after boxing, so assert through the slot: the
        // live backend is vJoy and connected, and a second switch back
        // proves the vJoy instance was released (its factory builds a
        // fresh one — builds count both directions).
        let outcome = switch_backend(&mut current, BackendKind::VJoy, &factories, None, true)
            .expect("vJoy switch succeeds");
        assert_eq!(outcome.active, BackendKind::VJoy);
        assert!(outcome.switched);
        assert!(current.is_connected());
        assert_eq!(current.name(), "vJoy");

        let outcome = switch_backend(&mut current, BackendKind::Xbox, &factories, None, true)
            .expect("switch back to Xbox succeeds");
        assert_eq!(outcome.active, BackendKind::Xbox);
        assert!(current.is_connected());
        assert_eq!(hm.builds.load(Ordering::SeqCst), 2);
        assert_eq!(vjoy.builds.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn switch_disconnects_the_previous_backend_exactly_once() {
        let hm = MockFactory::new("Xbox");
        let vjoy = MockFactory::new("vJoy");
        let factories = factories_with(&hm, &vjoy);
        let (old, old_state) = MockBackend::new("Xbox");
        let mut current = old.boxed();
        current.try_connect().expect("old connects");
        assert!(state_of(&old_state).0);

        let outcome = switch_backend(&mut current, BackendKind::VJoy, &factories, None, true)
            .expect("vJoy switch succeeds");
        assert_eq!(outcome.active, BackendKind::VJoy);
        let (connected, _, disconnects) = state_of(&old_state);
        assert!(!connected, "previous backend must be disconnected");
        assert_eq!(disconnects, 1, "previous backend disconnected exactly once");
        assert!(current.is_connected());
    }

    #[test]
    fn failed_xbox_connect_falls_back_to_vjoy_with_reason() {
        let hm = MockFactory::new("Xbox");
        // The ViGEmBus failure arrives through the trait as a
        // DriverNotFound carrying this wording - the selector must
        // still fall back, without branching on the variant.
        hm.fail_next(
            1,
            "ViGEmBus driver not found (0xE0000001) - install the ViGEmBus driver and retry.",
        );
        let vjoy = MockFactory::new("vJoy");
        let factories = factories_with(&hm, &vjoy);
        let mut current = MockBackend::new("NullSink").0.boxed();

        let outcome = switch_backend(&mut current, BackendKind::Xbox, &factories, None, true)
            .expect("fallback to vJoy succeeds");
        assert_eq!(outcome.requested, BackendKind::Xbox);
        assert_eq!(outcome.active, BackendKind::VJoy);
        assert!(outcome.fell_back);
        assert!(outcome.switched);
        let reason = outcome
            .fallback_reason
            .expect("fallback reason is recorded");
        // The reason names the ViGEmBus failure; the fallback
        // itself is stated by the outcome detail, where it really happened.
        assert!(reason.contains("ViGEmBus"), "{reason}");
        assert!(
            outcome.detail.contains("fell back to vJoy"),
            "{}",
            outcome.detail
        );
        assert!(current.is_connected());
        assert_eq!(current.name(), "vJoy");
    }

    #[test]
    fn failed_connect_rolls_back_to_the_previous_backend() {
        let hm = MockFactory::new("Xbox");
        let vjoy = MockFactory::new("vJoy");
        // vJoy connect fails: the requested switch must error and the
        // installed Xbox backend must stay exactly as it was —
        // still installed, still connected, never disconnected.
        vjoy.fail_next(1, "vJoy device 1 is owned by another application (BUSY).");
        let factories = factories_with(&hm, &vjoy);
        let (old, old_state) = MockBackend::new("Xbox");
        let mut current = old.boxed();
        current.try_connect().expect("old connects");

        let error = switch_backend(&mut current, BackendKind::VJoy, &factories, None, true)
            .expect_err("failed vJoy switch must error");
        assert!(error.to_string().contains("BUSY"), "{error}");
        assert_eq!(current.name(), "Xbox");
        let (connected, _, disconnects) = state_of(&old_state);
        assert!(connected, "previous backend stays connected after rollback");
        assert_eq!(disconnects, 0, "previous backend is never disconnected");
    }

    #[test]
    fn xbox_and_fallback_both_failing_leaves_the_slot_unchanged() {
        let hm = MockFactory::new("Xbox");
        hm.fail_next(1, "ViGEmBus driver not found.");
        let vjoy = MockFactory::new("vJoy");
        vjoy.fail_next(1, "vJoy not found.");
        let factories = factories_with(&hm, &vjoy);
        let mut current = MockBackend::new("NullSink").0.boxed();

        let error = switch_backend(&mut current, BackendKind::Xbox, &factories, None, true)
            .expect_err("both backends failing must error");
        let message = error.to_string();
        assert!(message.contains("driver not found"), "{message}");
        assert!(message.contains("vJoy not found"), "{message}");
        assert_eq!(current.name(), "NullSink");
        assert!(!current.is_connected());
    }

    #[test]
    fn xbox_failure_while_vjoy_is_live_reconnects_vjoy_in_place() {
        // Asking for Xbox while vJoy is live, with Xbox
        // failing: the fallback must reconnect the installed vJoy
        // backend, not build a second one that would collide on
        // AcquireVJD with the first.
        let hm = MockFactory::new("Xbox");
        hm.fail_next(1, "ViGEmBus driver install failed.");
        let vjoy = MockFactory::new("vJoy");
        let factories = factories_with(&hm, &vjoy);
        let (old, old_state) = MockBackend::new("vJoy");
        let mut current = old.boxed();
        current.try_connect().expect("old vJoy connects");

        let outcome = switch_backend(&mut current, BackendKind::Xbox, &factories, None, true)
            .expect("fallback reconnects the live vJoy backend");
        assert_eq!(outcome.active, BackendKind::VJoy);
        assert!(outcome.fell_back);
        assert!(!outcome.switched, "same backend instance stays installed");
        assert_eq!(
            vjoy.builds.load(Ordering::SeqCst),
            0,
            "no second vJoy built"
        );
        let (connected, connect_calls, _) = state_of(&old_state);
        assert!(connected);
        assert_eq!(connect_calls, 2, "installed vJoy reconnected in place");
    }

    #[test]
    fn same_backend_disconnected_reconnects_without_rebuilding() {
        let hm = MockFactory::new("Xbox");
        let vjoy = MockFactory::new("vJoy");
        let factories = factories_with(&hm, &vjoy);
        let (backend, backend_state) = MockBackend::new("Xbox");
        let mut current = backend.boxed();

        let outcome = switch_backend(&mut current, BackendKind::Xbox, &factories, None, true)
            .expect("same-kind reconnect succeeds");
        assert_eq!(outcome.active, BackendKind::Xbox);
        assert!(!outcome.switched, "same backend instance stays installed");
        assert_eq!(
            hm.builds.load(Ordering::SeqCst),
            0,
            "no rebuild on reconnect"
        );
        let (connected, connect_calls, _) = state_of(&backend_state);
        assert!(connected);
        assert_eq!(connect_calls, 1);
    }

    #[test]
    fn same_backend_connected_is_a_no_op() {
        let hm = MockFactory::new("Xbox");
        let vjoy = MockFactory::new("vJoy");
        let factories = factories_with(&hm, &vjoy);
        let (backend, backend_state) = MockBackend::new("Xbox");
        let mut current = backend.boxed();
        current.try_connect().expect("backend connects");

        let outcome = switch_backend(&mut current, BackendKind::Xbox, &factories, None, true)
            .expect("no-op switch succeeds");
        assert!(!outcome.switched);
        assert!(!outcome.fell_back);
        let (_, connect_calls, disconnect_calls) = state_of(&backend_state);
        assert_eq!(connect_calls, 1, "no reconnect when already connected");
        assert_eq!(disconnect_calls, 0);
    }

    #[test]
    fn switch_to_none_never_attempts_a_connect() {
        let hm = MockFactory::new("Xbox");
        let vjoy = MockFactory::new("vJoy");
        let factories = factories_with(&hm, &vjoy);
        let (old, old_state) = MockBackend::new("Xbox");
        let mut current = old.boxed();
        current.try_connect().expect("old connects");

        let outcome = switch_backend(&mut current, BackendKind::None, &factories, None, true)
            .expect("switch to none succeeds");
        assert_eq!(outcome.active, BackendKind::None);
        assert!(!current.is_connected());
        assert_eq!(state_of(&old_state).2, 1, "old backend disconnected once");
    }

    #[test]
    fn explicit_xbox_failure_errors_without_fallback() {
        // Bug 4/8: an explicit user switch to Xbox whose
        // connect fails (non-UAC reason) must ERROR with the real
        // reason — never silently land on vJoy with a success
        // outcome. The slot stays exactly as it was.
        let hm = MockFactory::new("Xbox");
        hm.fail_next(1, "ViGEmBus target failed to plug in.");
        let vjoy = MockFactory::new("vJoy");
        let factories = factories_with(&hm, &vjoy);
        let (old, old_state) = MockBackend::new("vJoy");
        let mut current = old.boxed();
        current.try_connect().expect("old vJoy connects");

        let error = switch_backend(&mut current, BackendKind::Xbox, &factories, None, false)
            .expect_err("explicit Xbox failure must error, not fall back");
        let message = error.to_string();
        assert!(message.contains("failed to plug in"), "{message}");
        assert!(!message.contains("fell back"), "{message}");
        assert_eq!(current.name(), "vJoy");
        let (connected, _, disconnects) = state_of(&old_state);
        assert!(connected, "live vJoy stays connected");
        assert_eq!(disconnects, 0, "live vJoy is never disconnected");
        assert_eq!(
            vjoy.builds.load(Ordering::SeqCst),
            0,
            "no second vJoy is built for a disallowed fallback"
        );
    }

    #[test]
    fn explicit_xbox_failure_from_sink_errors_without_fallback() {
        // Same rule from the no-device sink: the error names the
        // Xbox failure, the sink stays installed.
        let hm = MockFactory::new("Xbox");
        hm.fail_next(1, "ViGEmBus driver install failed.");
        let vjoy = MockFactory::new("vJoy");
        let factories = factories_with(&hm, &vjoy);
        let mut current = MockBackend::new("NullSink").0.boxed();

        let error = switch_backend(&mut current, BackendKind::Xbox, &factories, None, false)
            .expect_err("explicit Xbox failure must error");
        assert!(error.to_string().contains("driver install failed"));
        assert_eq!(current.name(), "NullSink");
        assert!(!current.is_connected());
    }

    #[test]
    fn explicit_same_kind_reconnect_failure_errors_without_fallback() {
        // The same-kind reconnect path honours the flag too: a
        // disconnected Xbox whose reconnect fails errors
        // instead of falling back to vJoy.
        let hm = MockFactory::new("Xbox");
        let vjoy = MockFactory::new("vJoy");
        let factories = factories_with(&hm, &vjoy);
        let (backend, backend_state) =
            MockBackend::with_failures("Xbox", 2, "ViGEmBus target failed to plug in.");
        let mut current = backend.boxed();

        let error = switch_backend(&mut current, BackendKind::Xbox, &factories, None, false)
            .expect_err("explicit reconnect failure must error");
        assert!(error.to_string().contains("failed to plug in"));
        assert_eq!(current.name(), "Xbox");
        let (connected, _, disconnects) = state_of(&backend_state);
        assert!(!connected);
        assert_eq!(disconnects, 0, "failed instance is not disconnected");
        // …while the automatic path still falls back, for the same
        // installed backend.
        let outcome = switch_backend(&mut current, BackendKind::Xbox, &factories, None, true)
            .expect("automatic reconnect falls back to vJoy");
        assert!(outcome.fell_back);
        assert_eq!(outcome.active, BackendKind::VJoy);
    }

    #[test]
    fn missing_factory_errors_without_touching_the_slot() {
        let factories = BackendFactories {
            xbox: None,
            vjoy: None,
            none: Box::new(|| MockBackend::new("NullSink").0.boxed()),
        };
        let (old, old_state) = MockBackend::new("Xbox");
        let mut current = old.boxed();
        current.try_connect().expect("old connects");

        // Requesting the *other* backend (vJoy) with no factory for it
        // must error — the same-kind request would be a no-op that
        // needs no factory at all.
        let error = switch_backend(&mut current, BackendKind::VJoy, &factories, None, true)
            .expect_err("missing vJoy factory must error");
        assert!(matches!(error, OutputError::BackendNotAvailable(_)));
        assert!(error.to_string().contains("not installed"), "{error}");
        let (connected, _, disconnects) = state_of(&old_state);
        assert!(connected);
        assert_eq!(disconnects, 0);
    }
}
