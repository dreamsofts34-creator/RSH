//! `TelemetryWatcher`: per-car / per-game profile auto-switch (Phase F2,
//! SW-R7.2).
//!
//! The watcher polls two cheap sources and asks the
//! [`CarProfileResolver`](crate::resolver::CarProfileResolver) what should
//! be active:
//!
//! * foreground game (400 ms debounce — SW-R7.2 / A8), via a
//!   [`ForegroundDetector`];
//! * current car (on track, same car for 4 consecutive polls spanning at
//!   least 2 s — DATA-INPUT §3 switch trigger), via a [`CarSource`].
//!
//! The actual profile activation is injected as a closure, so the watcher
//! itself never touches the store: tests inject a closure that records
//! activations or fails on demand.
//!
//! # Fail-safe
//!
//! If activation fails, the current profile **stays** — the watcher never
//! leaves the wheel profile-less. The failure is recorded in
//! [`TelemetryWatcher::last_error`] and no switch event is emitted.
//!
//! # Restore on game close — spec deviation, read this
//!
//! DATA-INPUT §3 rule 6 says a game closing does *not* change the profile
//! ("aakhri profile rehti hai") and only the *launcher* restores on exit
//! (A2). Phase F gate decision: rule 6 wins — the watcher does NOT
//! restore on game close by default; the launcher owns restore. The
//! behaviour stays behind the [`TelemetryWatcher::restore_on_game_close`]
//! flag so it can be turned on without touching the switch logic.
//!
//! # Manual override
//!
//! While [`TelemetryWatcher::set_manual_override`] is on, no automatic
//! switch or restore happens (DATA-INPUT §3 rule 5: manual override wins).
//! Tracking state is frozen and re-armed when the override clears, so a
//! stale car can never switch the profile the moment the override lifts.

use std::time::{Duration, Instant};

use crate::resolver::{CarProfileResolver, ProfileId, ProfileSwitchEvent, ProfileSwitchReason};

/// How long a foreground game must be stable before the watcher acts on
/// it (SW-R7.2 / A8 debounce). Flapping (alt-tab flicker) never switches.
pub const GAME_DEBOUNCE: Duration = Duration::from_millis(400);

/// How many consecutive polls must report the same car before a
/// car-profile switch is considered.
pub const CAR_STABLE_POLLS: u32 = 4;

/// Minimum wall time the same car must be reported for (DATA-INPUT §3:
/// 4 consecutive polls at 2 Hz = 2 s).
pub const CAR_STABLE_TIME: Duration = Duration::from_secs(2);

/// One telemetry reading: which game is running and which car is on track.
///
/// Produced by the telemetry readers (Phase F1, `rsh-telemetry`); until
/// that crate exposes its public API, this watcher is driven through the
/// [`CarSource`] trait so the gate wiring is a one-line adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CarReading {
    /// Game id (one of the `GAME_*` constants in
    /// [`crate::resolver`]).
    pub game_id: String,
    /// Car model exactly as the game reports it (may be empty in menus).
    pub car_name: String,
    /// True only when the session state is on track (DATA-INPUT §3
    /// trigger rule 1: a menu/loading car never switches).
    pub on_track: bool,
}

impl CarReading {
    /// Builds a reading.
    #[must_use]
    pub fn new(game_id: impl Into<String>, car_name: impl Into<String>, on_track: bool) -> Self {
        Self {
            game_id: game_id.into(),
            car_name: car_name.into(),
            on_track,
        }
    }
}

/// Source of the current car reading. The F1 telemetry readers implement
/// this at the gate; tests use a stub that returns scripted readings.
pub trait CarSource {
    /// Current car reading, or [`None`] when no game/telemetry is active.
    fn current_car(&self) -> Option<CarReading>;
}

impl<T: CarSource + ?Sized> CarSource for &T {
    fn current_car(&self) -> Option<CarReading> {
        (**self).current_car()
    }
}

/// Detects which game owns the foreground window, if any.
///
/// Returns a game id (one of the `GAME_*` constants in
/// [`crate::resolver`]), or [`None`] when the foreground window is not a
/// known game.
pub trait ForegroundDetector {
    /// Game id of the foreground window's process, or [`None`].
    fn foreground_game(&self) -> Option<String>;
}

impl<T: ForegroundDetector + ?Sized> ForegroundDetector for &T {
    fn foreground_game(&self) -> Option<String> {
        (**self).foreground_game()
    }
}

/// Exe file name (lower-case) to game id, for foreground detection and
/// for the launcher catalogue. `acs.exe` is Assetto Corsa's helper
/// process — it belongs to the same game as `assettocorsa.exe`.
pub const GAME_EXE_NAMES: &[(&str, &str)] = &[
    ("assettocorsa.exe", crate::resolver::GAME_ASSETTO_CORSA),
    ("acs.exe", crate::resolver::GAME_ASSETTO_CORSA),
    ("acc.exe", crate::resolver::GAME_ASSETTO_CORSA_COMPETIZIONE),
    ("iracingsim64dx11.exe", crate::resolver::GAME_IRACING),
    ("dirt2.exe", crate::resolver::GAME_DIRT_RALLY_2),
];

/// Maps a process exe file name to a game id, if it is a known game.
///
/// The match is case-insensitive and ignores a directory prefix.
#[must_use]
pub fn game_id_for_exe(exe_file_name: &str) -> Option<&'static str> {
    let file_name = exe_file_name
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(exe_file_name)
        .to_lowercase();
    GAME_EXE_NAMES
        .iter()
        .find(|(exe, _)| *exe == file_name)
        .map(|(_, game_id)| *game_id)
}

/// Foreground-game detector.
///
/// * Windows: reads the foreground window's process exe name through raw
///   Win32 FFI (`GetForegroundWindow` → `GetWindowThreadProcessId` →
///   `OpenProcess` → `QueryFullProcessImageNameW`). No `windows`-crate
///   dependency yet — the exact crate choice is the SW-R7.1 spike's call.
///   This path is **not compiled on Linux and has no test coverage here**;
///   it needs Windows CI + a real foreground game (see the ignored test).
/// * Any other OS: stub that always returns [`None`] — foreground
///   detection is Windows-only.
#[derive(Clone, Copy, Debug, Default)]
pub struct WindowsForegroundDetector;

#[cfg(windows)]
mod win32 {
    //! Minimal raw Win32 FFI for foreground-process detection.
    //! Signatures are the documented Win32 ones; keep this module tiny —
    //! a `windows`-crate migration (SW-R7.1 spike) replaces it wholesale.

    /// `HWND` / `HANDLE` as an integer-sized value.
    pub type Handle = isize;

    unsafe extern "C" {
        /// Returns the foreground window handle, or 0.
        pub fn GetForegroundWindow() -> Handle;
        /// Writes the owning process id; returns the thread id.
        pub fn GetWindowThreadProcessId(hwnd: Handle, pid: *mut u32) -> u32;
        /// Opens a process handle, or 0 on failure.
        pub fn OpenProcess(desired_access: u32, inherit_handle: i32, pid: u32) -> Handle;
        /// Writes the full exe path into the buffer; returns non-zero on success.
        pub fn QueryFullProcessImageNameW(
            process: Handle,
            flags: u32,
            exe_name: *mut u16,
            size: *mut u32,
        ) -> i32;
        /// Closes a handle; returns non-zero on success.
        pub fn CloseHandle(handle: Handle) -> i32;
    }

    /// `PROCESS_QUERY_LIMITED_INFORMATION` (0x1000).
    pub const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
}

#[cfg(windows)]
impl ForegroundDetector for WindowsForegroundDetector {
    fn foreground_game(&self) -> Option<String> {
        // SAFETY: every call below is a plain Win32 query with checked
        // return values; the process handle is always closed, and the
        // UTF-16 buffer is stack-owned with its length tracked.
        unsafe {
            let hwnd = win32::GetForegroundWindow();
            if hwnd == 0 {
                return None;
            }
            let mut pid: u32 = 0;
            win32::GetWindowThreadProcessId(hwnd, &mut pid);
            if pid == 0 {
                return None;
            }
            let handle = win32::OpenProcess(win32::PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle == 0 {
                return None;
            }
            let mut buffer = [0_u16; 260];
            let mut size: u32 = 260;
            let ok = win32::QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut size);
            win32::CloseHandle(handle);
            if ok == 0 {
                return None;
            }
            let path = String::from_utf16_lossy(&buffer[..size as usize]);
            game_id_for_exe(&path).map(str::to_owned)
        }
    }
}

#[cfg(not(windows))]
impl ForegroundDetector for WindowsForegroundDetector {
    fn foreground_game(&self) -> Option<String> {
        // Stub: foreground detection is Windows-only (task rule).
        None
    }
}

/// A switch that failed or was blocked by the manual override and must be
/// retried on later polls, never silently dropped. At most one can be
/// pending: a game-profile switch only pends while a game is active, a
/// restore only pends while no game is active.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum PendingSwitch {
    /// Nothing pending.
    #[default]
    None,
    /// The game-profile switch for the active game.
    GameProfile,
    /// The restore of the pre-game profile after the game closed.
    Restore,
}

/// Watches the foreground game + telemetry car and auto-switches the
/// active profile.
///
/// Generic over the car source, the foreground detector, and the
/// activation closure so every scenario is testable without a game, a
/// window, or a profile store. Poll at 2 Hz (the DATA-INPUT §3 rate);
/// pass the poll time in so tests control the clock exactly.
pub struct TelemetryWatcher<'a, C, F, A>
where
    C: CarSource,
    F: ForegroundDetector,
    A: FnMut(&ProfileId) -> Result<(), String>,
{
    resolver: CarProfileResolver<'a>,
    car_source: C,
    foreground: F,
    apply: A,
    current_profile: Option<ProfileId>,
    /// Profile active when the current game session started; restored
    /// when the game closes (see the module docs for the spec deviation).
    restore_target: Option<ProfileId>,
    /// Last raw foreground value seen (pre-debounce).
    observed_game: Option<String>,
    /// When `observed_game` last changed.
    observed_since: Option<Instant>,
    /// Debounced game the watcher has acted on.
    active_game: Option<String>,
    /// Car currently being stability-tracked.
    pending_car: Option<String>,
    /// When `pending_car` was first seen.
    pending_car_since: Option<Instant>,
    /// Consecutive polls reporting `pending_car`.
    pending_car_polls: u32,
    /// Car whose profile is already active (prevents re-switching).
    active_car: Option<String>,
    /// Manual override on: no auto-switch, no restore (rule 5).
    manual_override: bool,
    /// Switch that failed or was blocked; retried on later polls.
    pending: PendingSwitch,
    /// When false, closing a game leaves the last profile active
    /// (DATA-INPUT §3 rule 6). Default true per the Phase F task.
    restore_on_game_close: bool,
    /// Last activation failure, if any.
    last_error: Option<String>,
    /// Successful switches (debounce acceptance: flapping must not
    /// produce extra switches).
    switch_count: u32,
}

impl<'a, C, F, A> TelemetryWatcher<'a, C, F, A>
where
    C: CarSource,
    F: ForegroundDetector,
    A: FnMut(&ProfileId) -> Result<(), String>,
{
    /// Creates a watcher. `current_profile` is the profile active right
    /// now (the watcher never activates anything at construction).
    #[must_use]
    pub fn new(
        resolver: CarProfileResolver<'a>,
        car_source: C,
        foreground: F,
        apply: A,
        current_profile: Option<ProfileId>,
    ) -> Self {
        Self {
            resolver,
            car_source,
            foreground,
            apply,
            current_profile,
            restore_target: None,
            observed_game: None,
            observed_since: None,
            active_game: None,
            pending_car: None,
            pending_car_since: None,
            pending_car_polls: 0,
            active_car: None,
            manual_override: false,
            restore_on_game_close: false,
            pending: PendingSwitch::None,
            last_error: None,
            switch_count: 0,
        }
    }

    /// Profile currently active (as far as the watcher knows).
    #[must_use]
    pub fn current_profile(&self) -> Option<&ProfileId> {
        self.current_profile.as_ref()
    }

    /// Syncs the watcher when something else activated a profile
    /// (manual activation in the UI, launcher, …). Does not switch.
    /// When the externally set profile differs from the current one,
    /// a pending automatic switch is dropped, and a pending restore is
    /// dropped too unless the new profile already is the restore target
    /// (i.e. the restore already happened elsewhere).
    pub fn set_current_profile(&mut self, profile: Option<ProfileId>) {
        if profile != self.current_profile {
            self.pending = PendingSwitch::None;
            if profile.as_ref() != self.restore_target.as_ref() {
                self.restore_target = None;
            }
        }
        self.current_profile = profile;
    }

    /// Debounced game the watcher has acted on, if any.
    #[must_use]
    pub fn active_game(&self) -> Option<&str> {
        self.active_game.as_deref()
    }

    /// Profile that will be restored when the game closes, if any.
    #[must_use]
    pub fn restore_target(&self) -> Option<&ProfileId> {
        self.restore_target.as_ref()
    }

    /// Last activation failure, if any (fail-safe record).
    #[must_use]
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Number of successful switches so far.
    #[must_use]
    pub fn switch_count(&self) -> u32 {
        self.switch_count
    }

    /// Whether the manual override is on.
    #[must_use]
    pub fn manual_override(&self) -> bool {
        self.manual_override
    }

    /// Turns the manual override on/off. Turning it on re-arms the car
    /// tracking so no stale car switches when it clears.
    pub fn set_manual_override(&mut self, on: bool) {
        self.manual_override = on;
        if on {
            self.reset_car_tracking();
        }
    }

    /// Turns restore-on-game-close on/off (default on; see module docs).
    pub fn set_restore_on_game_close(&mut self, on: bool) {
        self.restore_on_game_close = on;
    }

    /// One poll. Call at 2 Hz with the current time; returns the switch
    /// events that happened (usually none, at most one per poll).
    ///
    /// The returned events are the notification payloads — the app layer
    /// emits each as [`PROFILE_SWITCHED_EVENT`](crate::resolver::PROFILE_SWITCHED_EVENT).
    pub fn poll(&mut self, now: Instant) -> Vec<ProfileSwitchEvent> {
        let mut events = Vec::new();

        // --- Foreground game, debounced (400 ms) ---------------------
        let raw_game = self.foreground.foreground_game();
        if raw_game != self.observed_game {
            self.observed_game = raw_game;
            self.observed_since = Some(now);
        }
        let stable_since = self.observed_since.unwrap_or(now);
        if self.observed_game != self.active_game
            && now.duration_since(stable_since) >= GAME_DEBOUNCE
        {
            let previous_game = self.active_game.clone();
            self.active_game = self.observed_game.clone();
            self.reset_car_tracking();
            if let Some(event) = self.on_game_transition(previous_game.as_deref(), now) {
                events.push(event);
            }
        }

        // --- Car stability (only while a game is active) --------------
        if let Some(game) = self.active_game.clone()
            && !self.manual_override
            && let Some(event) = self.on_car_poll(&game, now)
        {
            events.push(event);
        }

        // --- Retries: a failed or override-blocked switch is never dropped.
        // A pending game switch only retries while no car profile is in
        // effect (the car profile is more specific and wins).
        if !self.manual_override {
            match self.pending {
                PendingSwitch::GameProfile => {
                    if let Some(game) = self.active_game.clone()
                        && self.active_car.is_none()
                    {
                        match self.resolver.resolve(Some(game.as_str()), None) {
                            Some(target) if Some(&target) != self.current_profile.as_ref() => {
                                if let Some(event) = self.try_switch(
                                    target,
                                    Some(game),
                                    None,
                                    ProfileSwitchReason::GameDetected,
                                ) {
                                    self.pending = PendingSwitch::None;
                                    events.push(event);
                                }
                            }
                            _ => self.pending = PendingSwitch::None,
                        }
                    }
                }
                PendingSwitch::Restore => {
                    if self.active_game.is_none() {
                        match self.restore_target.clone() {
                            Some(target) if Some(&target) != self.current_profile.as_ref() => {
                                if let Some(event) = self.try_switch(
                                    target,
                                    None,
                                    None,
                                    ProfileSwitchReason::GameClosedRestore,
                                ) {
                                    self.pending = PendingSwitch::None;
                                    self.restore_target = None;
                                    events.push(event);
                                }
                            }
                            Some(_) => {
                                self.pending = PendingSwitch::None;
                                self.restore_target = None;
                            }
                            None => self.pending = PendingSwitch::None,
                        }
                    }
                }
                PendingSwitch::None => {}
            }
        }

        events
    }

    /// Handles a committed (debounced) game transition. Returns a switch
    /// event when a switch happened.
    fn on_game_transition(
        &mut self,
        previous_game: Option<&str>,
        _now: Instant,
    ) -> Option<ProfileSwitchEvent> {
        // Clone first: the arms below call &mut self methods, so the match
        // must not borrow a field of self.
        let new_game = self.active_game.clone();
        match (&previous_game, &new_game) {
            // Game closed: restore the pre-game profile (fail-safe:
            // restore is just an activation like any other). A failure
            // or a manual override keeps the target for a later retry
            // instead of dropping it.
            (Some(_), None) => {
                if !self.restore_on_game_close {
                    self.restore_target = None;
                    self.pending = PendingSwitch::None;
                    return None;
                }
                if self.manual_override {
                    self.pending = PendingSwitch::Restore;
                    return None;
                }
                let target = self.restore_target.clone()?;
                if Some(&target) == self.current_profile.as_ref() {
                    self.restore_target = None;
                    return None;
                }
                if let Some(event) =
                    self.try_switch(target, None, None, ProfileSwitchReason::GameClosedRestore)
                {
                    self.restore_target = None;
                    self.pending = PendingSwitch::None;
                    Some(event)
                } else {
                    self.pending = PendingSwitch::Restore;
                    None
                }
            }
            // New session (or game changed): remember the pre-game
            // profile once per session, then switch to the game profile.
            // A failure or a manual override marks the switch pending
            // so a later poll retries it instead of dropping it.
            (_, Some(game)) => {
                if previous_game.is_none() && self.restore_target.is_none() {
                    self.restore_target = self.current_profile.clone();
                }
                // A new session supersedes a pending restore from the
                // previous one (the stored target is still the right
                // pre-game profile for this session too).
                self.pending = PendingSwitch::None;
                if self.manual_override {
                    self.pending = PendingSwitch::GameProfile;
                    return None;
                }
                let game = game.clone();
                let target = self.resolver.resolve(Some(game.as_str()), None)?;
                if Some(&target) == self.current_profile.as_ref() {
                    return None;
                }
                if let Some(event) =
                    self.try_switch(target, Some(game), None, ProfileSwitchReason::GameDetected)
                {
                    Some(event)
                } else {
                    self.pending = PendingSwitch::GameProfile;
                    None
                }
            }
            // No game before, no game now: nothing to do.
            (None, None) => None,
        }
    }

    /// Handles one car poll for the active game. Returns a switch event
    /// when the car stayed stable long enough and a switch happened.
    fn on_car_poll(&mut self, game: &str, now: Instant) -> Option<ProfileSwitchEvent> {
        let reading = self.car_source.current_car()?;
        let car_name = reading.car_name.trim();
        let usable = reading.on_track
            && !car_name.is_empty()
            && crate::resolver::normalize_key(&reading.game_id)
                == crate::resolver::normalize_key(game);
        if !usable {
            self.reset_car_tracking();
            return None;
        }
        let car_name = car_name.to_owned();
        if self.active_car.as_deref() == Some(car_name.as_str()) {
            return None;
        }
        if self.pending_car.as_deref() == Some(car_name.as_str()) {
            self.pending_car_polls += 1;
        } else {
            self.pending_car = Some(car_name.clone());
            self.pending_car_since = Some(now);
            self.pending_car_polls = 1;
        }
        let since = self.pending_car_since.unwrap_or(now);
        if self.pending_car_polls >= CAR_STABLE_POLLS
            && now.duration_since(since) >= CAR_STABLE_TIME
        {
            let target = self.resolver.resolve(Some(game), Some(car_name.as_str()))?;
            if Some(&target) == self.current_profile.as_ref() {
                // Already on that profile (e.g. manual activation): mark
                // the car handled so we do not re-resolve every poll.
                self.active_car = Some(car_name);
                self.reset_car_tracking_keep_active();
                return None;
            }
            let event = self.try_switch(
                target,
                Some(game.to_owned()),
                Some(car_name.clone()),
                ProfileSwitchReason::CarDetected,
            );
            if event.is_some() {
                // The car profile supersedes the game profile.
                self.active_car = Some(car_name);
                self.pending = PendingSwitch::None;
            }
            self.reset_car_tracking_keep_active();
            event
        } else {
            None
        }
    }

    /// Attempts an activation. On failure the current profile stays and
    /// the error is recorded; on success the event is returned.
    fn try_switch(
        &mut self,
        target: ProfileId,
        game_id: Option<String>,
        car_name: Option<String>,
        reason: ProfileSwitchReason,
    ) -> Option<ProfileSwitchEvent> {
        if self.current_profile.as_ref() == Some(&target) {
            return None;
        }
        match (self.apply)(&target) {
            Ok(()) => {
                self.last_error = None;
                self.switch_count += 1;
                let event = ProfileSwitchEvent::new(
                    self.current_profile.clone(),
                    target.clone(),
                    game_id,
                    car_name,
                    reason,
                );
                self.current_profile = Some(target);
                Some(event)
            }
            Err(error) => {
                // FAIL-SAFE: current profile stays, nothing emitted.
                self.last_error = Some(error);
                None
            }
        }
    }

    /// Clears car tracking (game changed, car left the track, bad reading).
    fn reset_car_tracking(&mut self) {
        self.pending_car = None;
        self.pending_car_since = None;
        self.pending_car_polls = 0;
        self.active_car = None;
    }

    /// Clears pending-car tracking after a car switch, keeping
    /// `active_car` so the same car does not re-trigger a switch.
    fn reset_car_tracking_keep_active(&mut self) {
        self.pending_car = None;
        self.pending_car_since = None;
        self.pending_car_polls = 0;
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use super::*;
    use crate::resolver::{GAME_ASSETTO_CORSA_COMPETIZIONE, GAME_IRACING, MapProfileSource};

    /// Stub car source: tests set the reading directly.
    struct StubCarSource {
        reading: RefCell<Option<CarReading>>,
    }

    impl StubCarSource {
        fn new() -> Self {
            Self {
                reading: RefCell::new(None),
            }
        }

        fn set(&self, reading: Option<CarReading>) {
            *self.reading.borrow_mut() = reading;
        }
    }

    impl CarSource for StubCarSource {
        fn current_car(&self) -> Option<CarReading> {
            self.reading.borrow().clone()
        }
    }

    /// Stub foreground detector: tests set the foreground game directly.
    struct StubForeground {
        game: RefCell<Option<String>>,
    }

    impl StubForeground {
        fn new() -> Self {
            Self {
                game: RefCell::new(None),
            }
        }

        fn set(&self, game: Option<&str>) {
            *self.game.borrow_mut() = game.map(str::to_owned);
        }
    }

    impl ForegroundDetector for StubForeground {
        fn foreground_game(&self) -> Option<String> {
            self.game.borrow().clone()
        }
    }

    /// Builds a watcher over a source with an ACC game profile, an ACC
    /// Ferrari car profile, and a global default.
    fn harness() -> (
        MapProfileSource,
        StubCarSource,
        StubForeground,
        RefCell<Vec<ProfileId>>,
        Cell<bool>,
    ) {
        let source = MapProfileSource::new()
            .with_game_profile(
                GAME_ASSETTO_CORSA_COMPETIZIONE,
                ProfileId::new("acc-default"),
            )
            .with_car_profile(
                GAME_ASSETTO_CORSA_COMPETIZIONE,
                "Ferrari 488 GT3",
                ProfileId::new("acc-ferrari"),
            )
            .with_default(ProfileId::new("global-default"));
        (
            source,
            StubCarSource::new(),
            StubForeground::new(),
            RefCell::new(Vec::new()),
            Cell::new(false),
        )
    }

    /// Polls `watcher` at 2 Hz for `polls` polls starting at `start`.
    fn poll_2hz<C, F, A>(
        watcher: &mut TelemetryWatcher<'_, C, F, A>,
        start: Instant,
        polls: u32,
    ) -> Vec<ProfileSwitchEvent>
    where
        C: CarSource,
        F: ForegroundDetector,
        A: FnMut(&ProfileId) -> Result<(), String>,
    {
        let mut all = Vec::new();
        for i in 0..polls {
            let now = start + Duration::from_millis(u64::from(i) * 500);
            all.extend(watcher.poll(now));
        }
        all
    }

    #[test]
    fn game_detected_then_car_stable_switches_profile() {
        let (source, cars, fg, applied, fail) = harness();
        let resolver = CarProfileResolver::new(&source);
        let mut watcher = TelemetryWatcher::new(
            resolver,
            &cars,
            &fg,
            |id: &ProfileId| {
                if fail.get() {
                    return Err("apply failed".to_owned());
                }
                applied.borrow_mut().push(id.clone());
                Ok(())
            },
            Some(ProfileId::new("global-default")),
        );

        let t0 = Instant::now();
        fg.set(Some(GAME_ASSETTO_CORSA_COMPETIZIONE));
        // Not yet debounced: no switch.
        assert_eq!(watcher.poll(t0).len(), 0);
        // Debounced (400 ms): game profile switches.
        let events = watcher.poll(t0 + Duration::from_millis(500));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].new_profile_id, ProfileId::new("acc-default"));
        assert_eq!(events[0].reason, ProfileSwitchReason::GameDetected);
        assert_eq!(
            watcher.current_profile(),
            Some(&ProfileId::new("acc-default"))
        );

        // Car on track, stable for 4 polls spanning 2 s: car profile switches.
        cars.set(Some(CarReading::new(
            GAME_ASSETTO_CORSA_COMPETIZIONE,
            "Ferrari 488 GT3",
            true,
        )));
        // 5 polls at 2 Hz: the 4th poll is only 1.5 s in (too early), the
        // 5th crosses 2 s — exactly one switch.
        let events = poll_2hz(&mut watcher, t0 + Duration::from_secs(1), 5);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].new_profile_id, ProfileId::new("acc-ferrari"));
        assert_eq!(events[0].reason, ProfileSwitchReason::CarDetected);
        assert_eq!(applied.borrow().len(), 2);
        assert_eq!(watcher.switch_count(), 2);
    }

    #[test]
    fn switch_failure_keeps_current_profile() {
        let (source, cars, fg, applied, fail) = harness();
        let resolver = CarProfileResolver::new(&source);
        let mut watcher = TelemetryWatcher::new(
            resolver,
            &cars,
            &fg,
            |id: &ProfileId| {
                if fail.get() {
                    return Err("store write failed".to_owned());
                }
                applied.borrow_mut().push(id.clone());
                Ok(())
            },
            Some(ProfileId::new("global-default")),
        );

        fail.set(true);
        fg.set(Some(GAME_ASSETTO_CORSA_COMPETIZIONE));
        let t0 = Instant::now();
        watcher.poll(t0);
        let events = watcher.poll(t0 + Duration::from_millis(500));
        assert!(events.is_empty(), "failed switch must emit nothing");
        assert_eq!(
            watcher.current_profile(),
            Some(&ProfileId::new("global-default")),
            "FAIL-SAFE: profile unchanged after failed switch"
        );
        assert!(watcher.last_error().is_some());
        assert_eq!(applied.borrow().len(), 0);

        // Recovery: the next poll retries and succeeds.
        fail.set(false);
        let events = watcher.poll(t0 + Duration::from_millis(1000));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].new_profile_id, ProfileId::new("acc-default"));
        assert!(watcher.last_error().is_none());
    }

    #[test]
    fn game_closed_restores_previous_profile() {
        let (source, cars, fg, _applied, _fail) = harness();
        let resolver = CarProfileResolver::new(&source);
        let mut watcher = TelemetryWatcher::new(
            resolver,
            &cars,
            &fg,
            |_: &ProfileId| Ok(()),
            Some(ProfileId::new("global-default")),
        );
        // Opt-in behaviour (Phase F gate: default is OFF per DATA-INPUT
        // §3 rule 6; the launcher owns restore).
        watcher.set_restore_on_game_close(true);

        let t0 = Instant::now();
        fg.set(Some(GAME_ASSETTO_CORSA_COMPETIZIONE));
        watcher.poll(t0);
        watcher.poll(t0 + Duration::from_millis(500));
        assert_eq!(
            watcher.current_profile(),
            Some(&ProfileId::new("acc-default"))
        );
        assert_eq!(
            watcher.restore_target(),
            Some(&ProfileId::new("global-default"))
        );

        // Game closes; restore only fires after the 400 ms debounce.
        fg.set(None);
        assert_eq!(watcher.poll(t0 + Duration::from_secs(1)).len(), 0);
        let events = watcher.poll(t0 + Duration::from_millis(1500));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].reason, ProfileSwitchReason::GameClosedRestore);
        assert_eq!(events[0].new_profile_id, ProfileId::new("global-default"));
        assert_eq!(
            watcher.current_profile(),
            Some(&ProfileId::new("global-default"))
        );
        assert!(watcher.restore_target().is_none());
    }

    #[test]
    fn restore_can_be_turned_off() {
        let (source, cars, fg, _applied, _fail) = harness();
        let resolver = CarProfileResolver::new(&source);
        let mut watcher = TelemetryWatcher::new(
            resolver,
            &cars,
            &fg,
            |_: &ProfileId| Ok(()),
            Some(ProfileId::new("global-default")),
        );
        watcher.set_restore_on_game_close(false);

        let t0 = Instant::now();
        fg.set(Some(GAME_ASSETTO_CORSA_COMPETIZIONE));
        watcher.poll(t0);
        watcher.poll(t0 + Duration::from_millis(500));
        fg.set(None);
        watcher.poll(t0 + Duration::from_secs(1));
        assert_eq!(watcher.poll(t0 + Duration::from_millis(1500)).len(), 0);
        assert_eq!(
            watcher.current_profile(),
            Some(&ProfileId::new("acc-default")),
            "with restore off, the last profile stays (DATA-INPUT §3 rule 6)"
        );
    }

    #[test]
    fn debounce_suppresses_foreground_flapping() {
        let (source, cars, fg, _applied, _fail) = harness();
        let resolver = CarProfileResolver::new(&source);
        let mut watcher = TelemetryWatcher::new(
            resolver,
            &cars,
            &fg,
            |_: &ProfileId| Ok(()),
            Some(ProfileId::new("global-default")),
        );

        // AC appears, vanishes, reappears — all inside 400 ms windows.
        let t0 = Instant::now();
        fg.set(Some(GAME_ASSETTO_CORSA_COMPETIZIONE));
        assert_eq!(watcher.poll(t0).len(), 0);
        fg.set(None);
        assert_eq!(watcher.poll(t0 + Duration::from_millis(200)).len(), 0);
        fg.set(Some(GAME_ASSETTO_CORSA_COMPETIZIONE));
        assert_eq!(watcher.poll(t0 + Duration::from_millis(300)).len(), 0);
        // Stable for 400 ms now: exactly one switch.
        let events = watcher.poll(t0 + Duration::from_millis(800));
        assert_eq!(events.len(), 1);
        assert_eq!(watcher.switch_count(), 1);
        // Repeated polls with nothing changed: no further switches.
        assert_eq!(watcher.poll(t0 + Duration::from_secs(2)).len(), 0);
        assert_eq!(watcher.switch_count(), 1);
    }

    #[test]
    fn car_must_be_stable_for_four_polls_and_two_seconds() {
        let (source, cars, fg, _applied, _fail) = harness();
        let resolver = CarProfileResolver::new(&source);
        let mut watcher = TelemetryWatcher::new(
            resolver,
            &cars,
            &fg,
            |_: &ProfileId| Ok(()),
            Some(ProfileId::new("global-default")),
        );

        let t0 = Instant::now();
        fg.set(Some(GAME_ASSETTO_CORSA_COMPETIZIONE));
        watcher.poll(t0);
        watcher.poll(t0 + Duration::from_millis(500));
        assert_eq!(
            watcher.current_profile(),
            Some(&ProfileId::new("acc-default"))
        );

        // Car changes every poll: never stable, never switches.
        for (i, car) in [
            "Ferrari 488 GT3",
            "Porsche 911 GT3 R",
            "Lamborghini Huracan GT3",
        ]
        .iter()
        .enumerate()
        {
            cars.set(Some(CarReading::new(
                GAME_ASSETTO_CORSA_COMPETIZIONE,
                *car,
                true,
            )));
            let now = t0 + Duration::from_millis(1000 + u64::try_from(i).unwrap_or(0) * 500);
            assert_eq!(watcher.poll(now).len(), 0);
        }
        assert_eq!(
            watcher.current_profile(),
            Some(&ProfileId::new("acc-default"))
        );

        // Same car for 4 polls but only 1.5 s elapsed: still no switch.
        cars.set(Some(CarReading::new(
            GAME_ASSETTO_CORSA_COMPETIZIONE,
            "Ferrari 488 GT3",
            true,
        )));
        for i in 0..4_u64 {
            let now = t0 + Duration::from_millis(3000 + i * 500);
            assert!(
                watcher.poll(now).is_empty(),
                "4 polls in 1.5 s must not switch yet"
            );
        }
        // Fifth poll crosses 2 s: switches.
        let events = watcher.poll(t0 + Duration::from_millis(5000));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].new_profile_id, ProfileId::new("acc-ferrari"));
    }

    #[test]
    fn car_in_menu_or_wrong_game_never_switches() {
        let (source, cars, fg, _applied, _fail) = harness();
        let resolver = CarProfileResolver::new(&source);
        let mut watcher = TelemetryWatcher::new(
            resolver,
            &cars,
            &fg,
            |_: &ProfileId| Ok(()),
            Some(ProfileId::new("global-default")),
        );

        let t0 = Instant::now();
        fg.set(Some(GAME_ASSETTO_CORSA_COMPETIZIONE));
        watcher.poll(t0);
        watcher.poll(t0 + Duration::from_millis(500));

        // In the menu (not on track): no switch.
        cars.set(Some(CarReading::new(
            GAME_ASSETTO_CORSA_COMPETIZIONE,
            "Ferrari 488 GT3",
            false,
        )));
        assert_eq!(
            poll_2hz(&mut watcher, t0 + Duration::from_secs(1), 5).len(),
            0
        );

        // Telemetry for a different game than the foreground one: no switch.
        cars.set(Some(CarReading::new(GAME_IRACING, "Ferrari 488 GT3", true)));
        assert_eq!(
            poll_2hz(&mut watcher, t0 + Duration::from_secs(4), 5).len(),
            0
        );
        assert_eq!(
            watcher.current_profile(),
            Some(&ProfileId::new("acc-default"))
        );
    }

    #[test]
    fn manual_override_blocks_switch_and_restore() {
        let (source, cars, fg, _applied, _fail) = harness();
        let resolver = CarProfileResolver::new(&source);
        let mut watcher = TelemetryWatcher::new(
            resolver,
            &cars,
            &fg,
            |_: &ProfileId| Ok(()),
            Some(ProfileId::new("global-default")),
        );
        watcher.set_manual_override(true);

        let t0 = Instant::now();
        fg.set(Some(GAME_ASSETTO_CORSA_COMPETIZIONE));
        watcher.poll(t0);
        assert_eq!(watcher.poll(t0 + Duration::from_millis(500)).len(), 0);
        assert_eq!(
            watcher.current_profile(),
            Some(&ProfileId::new("global-default"))
        );

        // Override clears: the next poll switches to the game profile.
        watcher.set_manual_override(false);
        let events = watcher.poll(t0 + Duration::from_secs(1));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].new_profile_id, ProfileId::new("acc-default"));
    }

    #[test]
    fn unknown_game_and_no_match_switches_nothing() {
        let (source, cars, fg, _applied, _fail) = harness();
        let resolver = CarProfileResolver::new(&source);
        let mut watcher = TelemetryWatcher::new(resolver, &cars, &fg, |_: &ProfileId| Ok(()), None);

        let t0 = Instant::now();
        fg.set(Some("some-other-game"));
        watcher.poll(t0);
        // Unknown game falls back to the default profile (configured).
        let events = watcher.poll(t0 + Duration::from_millis(500));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].new_profile_id, ProfileId::new("global-default"));

        // With no default either, nothing switches at all.
        let empty = MapProfileSource::new();
        let resolver = CarProfileResolver::new(&empty);
        let mut watcher = TelemetryWatcher::new(
            resolver,
            &cars,
            &fg,
            |_: &ProfileId| Ok(()),
            Some(ProfileId::new("keep-me")),
        );
        fg.set(Some("some-other-game"));
        watcher.poll(t0);
        assert_eq!(watcher.poll(t0 + Duration::from_millis(500)).len(), 0);
        assert_eq!(watcher.current_profile(), Some(&ProfileId::new("keep-me")));
    }

    #[test]
    fn exe_name_maps_to_game_id() {
        assert_eq!(
            game_id_for_exe("assettocorsa.exe"),
            Some(crate::resolver::GAME_ASSETTO_CORSA)
        );
        assert_eq!(
            game_id_for_exe(r"C:\Games\Steam\steamapps\common\assettocorsa\acs.exe"),
            Some(crate::resolver::GAME_ASSETTO_CORSA)
        );
        assert_eq!(
            game_id_for_exe("ACC.EXE"),
            Some(GAME_ASSETTO_CORSA_COMPETIZIONE)
        );
        assert_eq!(game_id_for_exe("notepad.exe"), None);
    }

    /// Windows-only: the real foreground detector needs a Windows desktop
    /// with a game in the foreground. This sandbox has neither.
    #[test]
    #[ignore = "Windows-only: needs a real foreground game window on a Windows PC (Windows CI)"]
    fn windows_foreground_detector_needs_windows_ci() {
        let detector = WindowsForegroundDetector;
        // On Linux this is the stub (always None); on Windows CI this
        // test must be run with a known game in the foreground and the
        // assertion below replaced by the expected game id.
        assert_eq!(detector.foreground_game(), None);
    }
}
