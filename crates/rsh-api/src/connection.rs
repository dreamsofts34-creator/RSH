//! L1 Connection Manager (LOGIC-FIX-PLAN §4.1): the explicit
//! connection state machine the daemon drives and the UI renders.
//!
//! ```text
//! Idle ─connect→ LinkUp ─HELLO→ Hello ─pair/resume→ Authenticating ─GET_CAPS ok→ Active
//!   ▲              │ (no HELLO)    │ (window closed / bad tag / no key)   │
//!   │              └─────────► Error(reason, hint, retry_in_ms) ◄────────────┘
//!   └──────────────  Backoff(2s→30s)  ◄── Active ─(no frames 2 s / transport lost)
//! ```
//!
//! Rules enforced here (pure logic — no I/O, no clock of its own;
//! the caller supplies `Instant`s so tests drive it deterministically):
//!
//! * `Active` only after a signed `GET_CAPS` passes — the caller
//!   reports `CapsOk` only on that event (C-01/C-02). The "Connected"
//!   label is only valid in `Active`.
//! * A failed activation records `Error(reason, hint, retry_in_ms)` +
//!   backoff (2 s → 30 s). User-action reasons (`NoStoredKey`,
//!   `WindowClosed`, `WindowExpired`, `StoredKeyExists`) stop
//!   auto-retry: `retry_in_ms` is `None` and the UI must surface
//!   "Pairing needed" (C-01). F21: `BindFailed(AddrInUse)` also
//!   stops auto-retry (`retry_in_ms: None` — the port holder can
//!   only be closed by the user) but is NOT a pairing reason, so
//!   it surfaces as a plain `Error`, never "Pairing needed".
//! * Heartbeat watchdog (C-04): no stream/status frame for 2 s in
//!   `Active` → transport rebuild + re-activate. F20: capped at
//!   [`MAX_WATCHDOG_REBUILDS`] consecutive frameless rebuilds, then
//!   the manager latches `Error(WatchdogStall)` with no retry timer
//!   (user Retry required) — only a real frame resets the count.
//! * HELLO wait (C-03): 750 ms per attempt (above the firmware's
//!   500 ms reply limiter), 3 attempts.
//! * Resume fail `BadTag` (C-13): one retry after 10 s with the
//!   "wheel busy" message; a second `BadTag` is a hard error.
//! * Every transition is recorded as one log line
//!   (`take_log_lines`) and as a `ConnectionChanged` event the
//!   caller forwards as `AppEvent` — one line per transition, never
//!   per poll.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::errors::ConnectionError;

/// HELLO wait per attempt (C-03): 750 ms, above the firmware's
/// 500 ms `HELLO`-reply rate limiter (F-04), so a reply is never
/// lost to the limiter. Was 500 ms × 2 attempts in v1.0.2.
pub const HELLO_WAIT: Duration = Duration::from_millis(750);
/// HELLO attempts per activation (C-03): 3 (was 2).
pub const HELLO_ATTEMPTS: u32 = 3;
/// Heartbeat watchdog (C-04): no stream/status frame for this long
/// in `Active` means the board is gone (reboot / Wi-Fi drop) — the
/// transport is rebuilt and the session re-activated.
pub const WATCHDOG_TIMEOUT: Duration = Duration::from_secs(2);
/// Watchdog rebuild cap (F20, v2.0.3): at most this many
/// consecutive watchdog rebuilds with no stream frame seen, then
/// the manager latches `Error(WatchdogStall)` with no retry timer
/// instead of rebuilding forever. Same count as F1's 5-attempt IPC
/// fatal and F3's 5-spawn budget. Only a real frame
/// ([`ConnEvent::FrameSeen`]) resets the count — `CapsOk` must NOT:
/// it resets `consecutive_failures`, which is exactly why the
/// pre-F20 rebuild loop never backed off and never stopped.
pub const MAX_WATCHDOG_REBUILDS: u32 = 5;
/// `BadTag` resume retry (C-13): one retry after this delay — the
/// firmware rejects a new session for 10 s after an authenticated
/// one (F-05).
pub const BAD_TAG_RETRY_DELAY: Duration = Duration::from_secs(10);
/// First backoff delay (M-11, shared with the forwarder policy).
pub const BACKOFF_BASE: Duration = Duration::from_secs(2);
/// Backoff cap (M-11).
pub const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// The connection state the UI renders. `Connected` is only ever
/// shown for [`ConnectionState::Active`].
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum ConnectionState {
    /// No transport, no attempt in flight.
    Idle,
    /// Link is up (transport bound), waiting for a HELLO.
    LinkUp,
    /// A HELLO arrived; waiting for pair/resume to start.
    Hello,
    /// Pair/resume + signed `GET_CAPS` in flight.
    Authenticating,
    /// Signed `GET_CAPS` passed — the only "Connected" state.
    Active,
    /// Failed with a typed reason. `retry_in_ms` is `None` when the
    /// fix needs the user (pairing) — auto-retry is stopped.
    Error {
        /// Typed failure reason.
        reason: ConnectionError,
        /// One-line "what to do" (from the reason).
        hint: String,
        /// Milliseconds until the next automatic retry (`None` = no retry).
        /// Relative (not an `Instant`) so the state stays serialisable.
        retry_in_ms: Option<u64>,
    },
    /// Waiting out a backoff before the next attempt. The absolute
    /// deadline lives on the manager (`retry_due_at`); the state
    /// itself only carries the serialisable wait, so it stays
    /// serialisable over IPC.
    Backoff {
        /// Backoff wait in milliseconds.
        wait_ms: u64,
        /// Which attempt this backoff precedes (1-based).
        attempt: u32,
    },
}

impl ConnectionState {
    /// The UI label: only `Active` is "Connected".
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Idle => "Disconnected",
            Self::LinkUp | Self::Hello | Self::Authenticating => "Connecting",
            Self::Active => "Connected",
            Self::Error { .. } => "Error",
            Self::Backoff { .. } => "Reconnecting",
        }
    }

    /// True only in `Active`.
    #[must_use]
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Active)
    }

    /// Short state name for logs / the wire.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::LinkUp => "LinkUp",
            Self::Hello => "Hello",
            Self::Authenticating => "Authenticating",
            Self::Active => "Active",
            Self::Error { .. } => "Error",
            Self::Backoff { .. } => "Backoff",
        }
    }
}

/// Events the driver feeds the manager. The manager never does I/O:
/// it answers with [`ManagerAction`]s the driver executes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnEvent {
    /// The user (or autostart) asked to connect.
    Connect,
    /// The transport bound and the link is up.
    LinkUp,
    /// A HELLO arrived.
    HelloReceived {
        /// Board reports itself unpaired.
        unpaired: bool,
        /// Pairing window is open.
        window_open: bool,
    },
    /// Pair/resume started (signed `GET_CAPS` in flight).
    Authenticating,
    /// Signed `GET_CAPS` passed — the session is live.
    CapsOk,
    /// Activation failed with a typed reason.
    ActivationFailed(ConnectionError),
    /// One HELLO wait expired with no reply.
    HelloTimeout,
    /// A stream or status frame arrived (heartbeat).
    FrameSeen,
    /// The transport was lost (source drop / bind death).
    TransportLost,
    /// The UDP bind failed (typed).
    BindFailed(crate::errors::BindErrorKind),
    /// The backoff/retry timer fired.
    RetryTimer,
    /// The user explicitly retried (from an `Error` state).
    UserRetry,
    /// The user disconnected.
    Disconnect,
}

/// What the driver must do after feeding an event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ManagerAction {
    /// Nothing to do.
    None,
    /// Send one flag-1 HELO (fresh nonce) and wait `HELLO_WAIT`.
    RequestHello,
    /// Start pair/resume + signed `GET_CAPS`.
    StartAuthentication,
    /// Rebuild the transport, then re-activate from `LinkUp`.
    RebuildTransport,
    /// Re-activate on the live transport (no rebuild needed).
    Reactivate,
    /// Stop auto-retry; surface "Pairing needed" in the UI.
    NeedsPairing(ConnectionError),
    /// The state changed — the driver must emit
    /// `AppEvent::ConnectionChanged` (the manager also records the
    /// log line; see `take_log_lines`).
    StateChanged,
}

/// The L1 Connection Manager: pure state machine + backoff +
/// watchdog + HELLO attempts + one `BadTag` retry.
#[derive(Debug)]
pub struct ConnectionManager {
    state: ConnectionState,
    /// Consecutive failed attempts (drives the 2 s → 30 s backoff).
    consecutive_failures: u32,
    /// HELLO attempts used in the current activation.
    hello_attempts: u32,
    /// Whether the one C-13 `BadTag` retry was already spent.
    bad_tag_retried: bool,
    /// Last stream/status frame seen (heartbeat, C-04).
    last_frame_at: Option<Instant>,
    /// Consecutive watchdog rebuilds with no frame seen (F20):
    /// reset only by [`ConnEvent::FrameSeen`], a fresh
    /// [`ConnEvent::Connect`]/[`ConnEvent::UserRetry`], or
    /// [`ConnEvent::Disconnect`].
    watchdog_rebuilds: u32,
    /// Absolute deadline of the current backoff (`None` when not backing off).
    backoff_until: Option<Instant>,
    /// Transition log lines, drained by the driver (one per
    /// transition — never per poll).
    log_lines: Vec<String>,
    /// State changes since the last drain, for event emission.
    changed: bool,
}

impl ConnectionManager {
    /// A fresh manager in `Idle`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: ConnectionState::Idle,
            consecutive_failures: 0,
            hello_attempts: 0,
            bad_tag_retried: false,
            last_frame_at: None,
            watchdog_rebuilds: 0,
            backoff_until: None,
            log_lines: Vec::new(),
            changed: false,
        }
    }

    /// Current state.
    #[must_use]
    pub fn state(&self) -> &ConnectionState {
        &self.state
    }

    /// Consecutive failed attempts (test visibility).
    #[must_use]
    pub const fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures
    }

    /// Consecutive watchdog rebuilds with no frame seen (F20, test
    /// visibility): reaches [`MAX_WATCHDOG_REBUILDS`] exactly when
    /// the manager latches `Error(WatchdogStall)`.
    #[must_use]
    pub const fn watchdog_rebuilds(&self) -> u32 {
        self.watchdog_rebuilds
    }

    /// Absolute instant the current backoff ends (`None` when not
    /// backing off). The driver polls this to fire `RetryTimer`.
    #[must_use]
    pub const fn retry_due_at(&self) -> Option<Instant> {
        self.backoff_until
    }

    /// Drain the transition log lines recorded since the last drain.
    pub fn take_log_lines(&mut self) -> Vec<String> {
        std::mem::take(&mut self.log_lines)
    }

    /// True when the state changed since the last `take_changed`.
    pub fn take_changed(&mut self) -> bool {
        std::mem::replace(&mut self.changed, false)
    }

    /// A `Duration` as whole milliseconds, saturating at `u64::MAX`
    /// (the waits here are seconds-scale, so saturation is
    /// unreachable in practice — it only silences the truncating
    /// cast).
    fn ms_u64(duration: Duration) -> u64 {
        u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
    }

    /// Backoff delay after `failures` consecutive failures (1-based):
    /// base doubled per failure after the first, capped at 30 s.
    #[must_use]
    pub fn backoff_delay(failures: u32) -> Duration {
        let doublings = failures.saturating_sub(1).min(30);
        BACKOFF_BASE
            .saturating_mul(1u32 << doublings)
            .min(BACKOFF_MAX)
    }

    /// Feed one event at `now`; returns what the driver must do.
    // One arm per event: splitting the dispatch would scatter the
    // machine's transitions without helping review (same shape as
    // the rig harness's `run_contract` dispatch).
    #[allow(clippy::too_many_lines)]
    pub fn on_event(&mut self, event: ConnEvent, now: Instant) -> ManagerAction {
        match event {
            ConnEvent::FrameSeen => {
                self.last_frame_at = Some(now);
                // F20: a real frame is the only proof the session is
                // alive — it clears the frameless-rebuild count, so
                // a session that streams (even with transient
                // silences) never latches.
                self.watchdog_rebuilds = 0;
                ManagerAction::None
            }
            ConnEvent::Connect => {
                if matches!(
                    self.state,
                    ConnectionState::Idle | ConnectionState::Error { .. }
                ) {
                    self.consecutive_failures = 0;
                    self.bad_tag_retried = false;
                    self.watchdog_rebuilds = 0;
                    self.transition(ConnectionState::LinkUp, now);
                    return ManagerAction::RequestHello;
                }
                ManagerAction::None
            }
            ConnEvent::LinkUp => {
                if matches!(self.state, ConnectionState::Backoff { .. }) {
                    self.transition(ConnectionState::LinkUp, now);
                    return ManagerAction::RequestHello;
                }
                ManagerAction::None
            }
            ConnEvent::HelloReceived {
                unpaired,
                window_open,
            } => {
                if matches!(self.state, ConnectionState::LinkUp | ConnectionState::Hello) {
                    self.hello_attempts = 0;
                    self.transition(ConnectionState::Hello, now);
                    // An unpaired board with a closed window cannot
                    // pair: fail closed with the typed reason now,
                    // instead of attempting a doomed GET_CAPS.
                    if unpaired && !window_open {
                        return self.fail(ConnectionError::WindowClosed, now);
                    }
                    return ManagerAction::StartAuthentication;
                }
                ManagerAction::None
            }
            ConnEvent::Authenticating => {
                if matches!(
                    self.state,
                    ConnectionState::Hello | ConnectionState::Authenticating
                ) {
                    self.transition(ConnectionState::Authenticating, now);
                }
                ManagerAction::None
            }
            ConnEvent::CapsOk => {
                if matches!(
                    self.state,
                    ConnectionState::Authenticating | ConnectionState::Hello
                ) {
                    self.consecutive_failures = 0;
                    self.bad_tag_retried = false;
                    self.last_frame_at = Some(now);
                    self.transition(ConnectionState::Active, now);
                    return ManagerAction::StateChanged;
                }
                ManagerAction::None
            }
            ConnEvent::ActivationFailed(reason) => self.fail(reason, now),
            ConnEvent::HelloTimeout => {
                if matches!(self.state, ConnectionState::LinkUp | ConnectionState::Hello) {
                    self.hello_attempts += 1;
                    if self.hello_attempts < HELLO_ATTEMPTS {
                        return ManagerAction::RequestHello;
                    }
                    return self.fail(ConnectionError::Unreachable, now);
                }
                ManagerAction::None
            }
            ConnEvent::TransportLost => {
                if self.state.is_active() {
                    let wait = Self::backoff_delay(1);
                    self.backoff_until = Some(now + wait);
                    self.transition(
                        ConnectionState::Backoff {
                            wait_ms: Self::ms_u64(wait),
                            attempt: 1,
                        },
                        now,
                    );
                    return ManagerAction::RebuildTransport;
                }
                if !matches!(self.state, ConnectionState::Idle) {
                    return self.fail(ConnectionError::TransportClosed, now);
                }
                ManagerAction::None
            }
            ConnEvent::BindFailed(kind) => self.fail(ConnectionError::BindFailed(kind), now),
            ConnEvent::RetryTimer => {
                if let ConnectionState::Backoff { attempt, .. } = self.state {
                    self.backoff_until = None;
                    self.consecutive_failures = attempt;
                    self.transition(ConnectionState::LinkUp, now);
                    return ManagerAction::RequestHello;
                }
                ManagerAction::None
            }
            ConnEvent::UserRetry => {
                if matches!(self.state, ConnectionState::Error { .. }) {
                    self.consecutive_failures = 0;
                    self.bad_tag_retried = false;
                    self.watchdog_rebuilds = 0;
                    self.transition(ConnectionState::LinkUp, now);
                    return ManagerAction::RequestHello;
                }
                ManagerAction::None
            }
            ConnEvent::Disconnect => {
                self.consecutive_failures = 0;
                self.bad_tag_retried = false;
                self.watchdog_rebuilds = 0;
                self.last_frame_at = None;
                self.backoff_until = None;
                self.transition(ConnectionState::Idle, now);
                ManagerAction::StateChanged
            }
        }
    }

    /// Heartbeat watchdog (C-04): call periodically with `now`. In
    /// `Active` with no frame for `WATCHDOG_TIMEOUT`, the transport
    /// is rebuilt and the session re-activated. Returns the action
    /// (or `None` when healthy).
    ///
    /// F20: the rebuild is capped — after
    /// [`MAX_WATCHDOG_REBUILDS`] consecutive rebuilds with no frame
    /// seen, the manager latches `Error(WatchdogStall)` (no retry
    /// timer, user Retry required) instead of rebuilding forever.
    /// `CapsOk` resets `consecutive_failures` but deliberately NOT
    /// `watchdog_rebuilds`: a board that authenticates yet never
    /// streams would otherwise rebuild at a fixed 2 s cadence
    /// forever, and — worse — so would a healthy session, because
    /// nothing fed [`ConnEvent::FrameSeen`] before F20 (the daemon
    /// now feeds it from the core's stream-frame clock).
    pub fn check_watchdog(&mut self, now: Instant) -> ManagerAction {
        if !self.state.is_active() {
            return ManagerAction::None;
        }
        let silent_for = self
            .last_frame_at
            .map_or(WATCHDOG_TIMEOUT + Duration::from_secs(1), |at| {
                now.saturating_duration_since(at)
            });
        if silent_for >= WATCHDOG_TIMEOUT {
            self.watchdog_rebuilds = self.watchdog_rebuilds.saturating_add(1);
            if self.watchdog_rebuilds >= MAX_WATCHDOG_REBUILDS {
                self.log_lines.push(format!(
                    "[WARN connection] heartbeat watchdog: {} consecutive rebuilds with no stream frame — latching WatchdogStall, user retry required",
                    self.watchdog_rebuilds
                ));
                self.backoff_until = None;
                self.transition(
                    ConnectionState::Error {
                        reason: ConnectionError::WatchdogStall,
                        hint: ConnectionError::WatchdogStall.hint().to_string(),
                        retry_in_ms: None,
                    },
                    now,
                );
                return ManagerAction::StateChanged;
            }
            self.log_lines.push(format!(
                "[WARN connection] heartbeat watchdog: no stream/status for {} ms in Active — rebuilding transport + re-activating (frameless rebuild {}/{})",
                silent_for.as_millis(),
                self.watchdog_rebuilds,
                MAX_WATCHDOG_REBUILDS
            ));
            self.consecutive_failures = self.consecutive_failures.saturating_add(1);
            let wait = Self::backoff_delay(self.consecutive_failures.max(1));
            self.backoff_until = Some(now + wait);
            self.transition(
                ConnectionState::Backoff {
                    wait_ms: Self::ms_u64(wait),
                    attempt: self.consecutive_failures.max(1),
                },
                now,
            );
            return ManagerAction::RebuildTransport;
        }
        ManagerAction::None
    }

    /// Record a failure: `BadTag` gets one 10 s retry (C-13);
    /// user-action reasons stop auto-retry with `retry_in_ms: None`
    /// (C-01); everything else backs off 2 s → 30 s. F21: a UDP
    /// `AddrInUse` is NOT retried at all — the port holder (usually
    /// the still-running legacy RSH app) can only be closed by the
    /// user, so retrying just re-logs the same failure forever
    /// (user log: 117× `AddrInUse` over ~35 min). It lands in
    /// `Error` with `retry_in_ms: None` like a user-action reason,
    /// but returns `StateChanged` (not `NeedsPairing`): this is not
    /// a pairing problem and must not surface a "Pairing needed"
    /// notification.
    fn fail(&mut self, reason: ConnectionError, now: Instant) -> ManagerAction {
        // C-13: one BadTag retry after 10 s — the board rejects a new
        // session for 10 s after an authenticated one ("wheel busy").
        if reason == ConnectionError::BadTag && !self.bad_tag_retried {
            self.bad_tag_retried = true;
            self.log_lines.push(
                "[INFO connection] activation failed: reason=BadTag — wheel busy, one retry in 10 s"
                    .to_string(),
            );
            self.backoff_until = Some(now + BAD_TAG_RETRY_DELAY);
            self.transition(
                ConnectionState::Backoff {
                    wait_ms: Self::ms_u64(BAD_TAG_RETRY_DELAY),
                    attempt: 1,
                },
                now,
            );
            return ManagerAction::Reactivate;
        }
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        if reason.needs_user_action() {
            self.transition(
                ConnectionState::Error {
                    reason,
                    hint: reason.hint().to_string(),
                    retry_in_ms: None,
                },
                now,
            );
            return ManagerAction::NeedsPairing(reason);
        }
        // F21: `AddrInUse` never self-heals — stop here. No retry
        // timer is armed (`backoff_until` stays `None`), so the
        // driver's retry tick never re-fires `activate_session`
        // into another doomed bind.
        if matches!(
            reason,
            ConnectionError::BindFailed(crate::errors::BindErrorKind::AddrInUse)
        ) {
            self.backoff_until = None;
            self.transition(
                ConnectionState::Error {
                    reason,
                    hint: reason.hint().to_string(),
                    retry_in_ms: None,
                },
                now,
            );
            return ManagerAction::StateChanged;
        }
        let delay = Self::backoff_delay(self.consecutive_failures);
        self.transition(
            ConnectionState::Error {
                reason,
                hint: reason.hint().to_string(),
                retry_in_ms: Some(Self::ms_u64(delay)),
            },
            now,
        );
        ManagerAction::StateChanged
    }

    /// Move to `next`, recording one log line + the changed flag.
    fn transition(&mut self, next: ConnectionState, _now: Instant) {
        let from = self.state.name();
        let to = next.name();
        if from != to {
            self.log_lines
                .push(format!("[INFO connection] state: {from} -> {to}"));
            self.changed = true;
        }
        // Error/Backoff carry data: record the reason too.
        if let ConnectionState::Error {
            reason,
            retry_in_ms,
            ..
        } = &next
        {
            let retry = retry_in_ms.map_or("none (user action required)".to_string(), |ms| {
                format!("retry in {ms} ms")
            });
            self.log_lines.push(format!(
                "[INFO connection] error: reason={} hint=\"{}\" {retry}",
                reason.code(),
                reason.hint()
            ));
        }
        self.state = next;
    }
}

impl Default for ConnectionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> Instant {
        Instant::now()
    }

    fn active_manager() -> ConnectionManager {
        let mut m = ConnectionManager::new();
        let t = now();
        m.on_event(ConnEvent::Connect, t);
        m.on_event(
            ConnEvent::HelloReceived {
                unpaired: false,
                window_open: false,
            },
            t,
        );
        m.on_event(ConnEvent::Authenticating, t);
        m.on_event(ConnEvent::CapsOk, t);
        assert!(m.state().is_active());
        m
    }

    #[test]
    fn happy_path_reaches_active_only_after_caps_ok() {
        let mut m = ConnectionManager::new();
        let t = now();
        assert_eq!(m.state().label(), "Disconnected");
        m.on_event(ConnEvent::Connect, t);
        assert_eq!(m.state().label(), "Connecting");
        // CapsOk before HELLO/auth must not activate (C-01/C-02).
        let mut m2 = ConnectionManager::new();
        m2.on_event(ConnEvent::CapsOk, t);
        assert!(!m2.state().is_active());
        m.on_event(
            ConnEvent::HelloReceived {
                unpaired: false,
                window_open: false,
            },
            t,
        );
        m.on_event(ConnEvent::Authenticating, t);
        assert!(!m.state().is_active());
        m.on_event(ConnEvent::CapsOk, t);
        assert!(m.state().is_active());
        assert_eq!(m.state().label(), "Connected");
    }

    #[test]
    fn hello_timeout_retries_three_times_then_unreachable() {
        let mut m = ConnectionManager::new();
        let t = now();
        m.on_event(ConnEvent::Connect, t);
        assert_eq!(
            m.on_event(ConnEvent::HelloTimeout, t),
            ManagerAction::RequestHello
        );
        assert_eq!(
            m.on_event(ConnEvent::HelloTimeout, t),
            ManagerAction::RequestHello
        );
        let action = m.on_event(ConnEvent::HelloTimeout, t);
        assert_eq!(action, ManagerAction::StateChanged);
        assert!(matches!(
            m.state(),
            ConnectionState::Error {
                reason: ConnectionError::Unreachable,
                ..
            }
        ));
    }

    #[test]
    fn user_action_reasons_stop_auto_retry() {
        for reason in [
            ConnectionError::NoStoredKey,
            ConnectionError::WindowClosed,
            ConnectionError::StoredKeyExists,
        ] {
            let mut m = ConnectionManager::new();
            let t = now();
            m.on_event(ConnEvent::Connect, t);
            let action = m.on_event(ConnEvent::ActivationFailed(reason), t);
            assert_eq!(action, ManagerAction::NeedsPairing(reason));
            assert!(matches!(
                m.state(),
                ConnectionState::Error {
                    retry_in_ms: None,
                    ..
                }
            ));
        }
    }

    #[test]
    fn bind_failed_addr_in_use_stops_auto_retry() {
        // F21: the port holder (usually the still-running legacy
        // RSH app) can only be closed by the user — retrying the
        // bind just re-logs the same failure forever (user log:
        // 117× `AddrInUse` over ~35 min). So no retry is scheduled
        // and no "Pairing needed" is raised: plain `Error`.
        let mut m = ConnectionManager::new();
        let t = now();
        m.on_event(ConnEvent::Connect, t);
        let action = m.on_event(
            ConnEvent::ActivationFailed(ConnectionError::BindFailed(
                crate::errors::BindErrorKind::AddrInUse,
            )),
            t,
        );
        assert_eq!(action, ManagerAction::StateChanged);
        assert!(matches!(
            m.state(),
            ConnectionState::Error {
                reason: ConnectionError::BindFailed(crate::errors::BindErrorKind::AddrInUse),
                retry_in_ms: None,
                ..
            }
        ));
        assert_eq!(m.retry_due_at(), None, "no retry timer is armed");
        assert!(
            matches!(m.state(), ConnectionState::Error { hint, .. } if hint.contains("4211") && hint.contains("RSH.exe")),
            "hint names the port and the app to close: {:?}",
            m.state()
        );
    }

    #[test]
    fn backoff_doubles_to_30s_cap() {
        assert_eq!(ConnectionManager::backoff_delay(1), Duration::from_secs(2));
        assert_eq!(ConnectionManager::backoff_delay(2), Duration::from_secs(4));
        assert_eq!(ConnectionManager::backoff_delay(3), Duration::from_secs(8));
        assert_eq!(ConnectionManager::backoff_delay(5), Duration::from_secs(30));
        assert_eq!(
            ConnectionManager::backoff_delay(99),
            Duration::from_secs(30)
        );
    }

    #[test]
    fn bad_tag_retries_once_after_10s_then_fails() {
        let mut m = ConnectionManager::new();
        let t = now();
        m.on_event(ConnEvent::Connect, t);
        assert_eq!(
            m.on_event(ConnEvent::ActivationFailed(ConnectionError::BadTag), t),
            ManagerAction::Reactivate
        );
        assert!(matches!(m.state(), ConnectionState::Backoff { .. }));
        // Second BadTag: no more free retries — hard error with backoff.
        let action = m.on_event(ConnEvent::ActivationFailed(ConnectionError::BadTag), t);
        assert_eq!(action, ManagerAction::StateChanged);
        assert!(matches!(
            m.state(),
            ConnectionState::Error {
                reason: ConnectionError::BadTag,
                ..
            }
        ));
    }

    #[test]
    fn watchdog_fires_after_2s_silence_in_active() {
        let mut m = active_manager();
        let t = now();
        // Fresh frame: healthy.
        m.on_event(ConnEvent::FrameSeen, t);
        assert_eq!(
            m.check_watchdog(t + Duration::from_millis(1500)),
            ManagerAction::None
        );
        // 2 s silence: rebuild.
        assert_eq!(
            m.check_watchdog(t + Duration::from_millis(2001)),
            ManagerAction::RebuildTransport
        );
        assert!(matches!(m.state(), ConnectionState::Backoff { .. }));
    }

    /// Drive one watchdog rebuild + the daemon driver's
    /// re-activation (`RetryTimer` → `LinkUp` → `HELLO` → `CapsOk` →
    /// `Active`) at `t`. `t` must be ≥2 s after the last `CapsOk` /
    /// frame or the watchdog will not have fired yet.
    fn reactivate_after_watchdog(m: &mut ConnectionManager, t: Instant) {
        assert_eq!(
            m.on_event(ConnEvent::RetryTimer, t),
            ManagerAction::RequestHello,
            "backoff must end in a re-activation attempt"
        );
        m.on_event(
            ConnEvent::HelloReceived {
                unpaired: false,
                window_open: false,
            },
            t,
        );
        m.on_event(ConnEvent::CapsOk, t);
        assert!(m.state().is_active(), "re-activation must reach Active");
    }

    #[test]
    fn watchdog_latches_after_five_frameless_rebuilds() {
        // F20: pre-fix, this loop ran forever — every re-activation
        // passes through CapsOk, which resets `consecutive_failures`,
        // so the backoff never grew past 2 s and nothing ever
        // stopped the rebuild cycle (transport rebind + HELLO +
        // GET_CAPS every ~4 s, healthy session included, because
        // nothing fed FrameSeen at all).
        let mut m = active_manager();
        let mut t = now();
        for rebuild in 1..=MAX_WATCHDOG_REBUILDS {
            t += Duration::from_secs(3);
            let action = m.check_watchdog(t);
            if rebuild < MAX_WATCHDOG_REBUILDS {
                assert_eq!(
                    action,
                    ManagerAction::RebuildTransport,
                    "rebuild {rebuild} must still rebuild"
                );
                assert_eq!(m.watchdog_rebuilds(), rebuild);
                reactivate_after_watchdog(&mut m, t);
            } else {
                assert_eq!(
                    action,
                    ManagerAction::StateChanged,
                    "the 5th frameless rebuild must latch, not rebuild"
                );
                assert_eq!(m.watchdog_rebuilds(), MAX_WATCHDOG_REBUILDS);
                assert!(
                    matches!(
                        m.state(),
                        ConnectionState::Error {
                            reason: ConnectionError::WatchdogStall,
                            retry_in_ms: None,
                            ..
                        }
                    ),
                    "latched state must be Error(WatchdogStall) with no auto-retry: {:?}",
                    m.state()
                );
                assert_eq!(m.retry_due_at(), None, "latched: no retry timer is armed");
            }
        }
        // Latched: further silence fires nothing, so the driver
        // never rebuilds again on its own.
        t += Duration::from_secs(30);
        assert_eq!(m.check_watchdog(t), ManagerAction::None);
        // A manual retry gets a fresh budget.
        assert_eq!(
            m.on_event(ConnEvent::UserRetry, t),
            ManagerAction::RequestHello
        );
        assert_eq!(m.watchdog_rebuilds(), 0);
    }

    #[test]
    fn frame_seen_resets_watchdog_rebuild_count() {
        let mut m = active_manager();
        let mut t = now();
        // 4 frameless rebuilds — one short of the latch.
        for _ in 0..(MAX_WATCHDOG_REBUILDS - 1) {
            t += Duration::from_secs(3);
            assert_eq!(m.check_watchdog(t), ManagerAction::RebuildTransport);
            reactivate_after_watchdog(&mut m, t);
        }
        assert_eq!(m.watchdog_rebuilds(), MAX_WATCHDOG_REBUILDS - 1);
        // One real frame clears the count: the next 4 rebuilds must
        // NOT latch (without the reset, the very next one would).
        m.on_event(ConnEvent::FrameSeen, t);
        assert_eq!(m.watchdog_rebuilds(), 0);
        for _ in 0..(MAX_WATCHDOG_REBUILDS - 1) {
            t += Duration::from_secs(3);
            assert_eq!(m.check_watchdog(t), ManagerAction::RebuildTransport);
            reactivate_after_watchdog(&mut m, t);
        }
        // ... and the 5th rebuild after the reset latches.
        t += Duration::from_secs(3);
        assert_eq!(m.check_watchdog(t), ManagerAction::StateChanged);
        assert!(matches!(
            m.state(),
            ConnectionState::Error {
                reason: ConnectionError::WatchdogStall,
                ..
            }
        ));
    }

    #[test]
    fn window_closed_hello_fails_closed_without_auth() {
        let mut m = ConnectionManager::new();
        let t = now();
        m.on_event(ConnEvent::Connect, t);
        let action = m.on_event(
            ConnEvent::HelloReceived {
                unpaired: true,
                window_open: false,
            },
            t,
        );
        assert_eq!(
            action,
            ManagerAction::NeedsPairing(ConnectionError::WindowClosed)
        );
        assert!(!m.state().is_active());
    }

    #[test]
    fn transport_loss_in_active_rebuilds() {
        let mut m = active_manager();
        let t = now();
        assert_eq!(
            m.on_event(ConnEvent::TransportLost, t),
            ManagerAction::RebuildTransport
        );
    }

    #[test]
    fn one_log_line_per_transition() {
        let mut m = ConnectionManager::new();
        let t = now();
        m.on_event(ConnEvent::Connect, t);
        m.on_event(ConnEvent::FrameSeen, t);
        m.on_event(ConnEvent::FrameSeen, t);
        let lines = m.take_log_lines();
        // Connect produced exactly one transition line; FrameSeen none.
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("Idle -> LinkUp"));
    }
}
