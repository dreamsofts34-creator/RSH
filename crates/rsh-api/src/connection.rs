//! L1 Connection Manager (LOGIC-FIX-PLAN §4.1): the explicit
//! connection state machine the daemon drives and the UI renders.
//!
//! ```text
//! Idle ─connect→ LinkUp ─HELLO→ Hello ─GET_CAPS ok→ Active
//!   ▲              │ (no HELLO)    │ (no caps)   │
//!   │              └─────────► Error(reason, hint, retry_in_ms) ◄──────┘
//!   └──────────────  Backoff(2s→30s)  ◄── Active ─(no frames 2 s / transport lost)
//! ```
//!
//! Rules enforced here (pure logic — no I/O, no clock of its own;
//! the caller supplies `Instant`s so tests drive it deterministically):
//!
//! * `Active` only after an unsigned `GET_CAPS` passes (protocol
//!   v2.2, no pairing) — the caller reports `CapsOk` only on that
//!   event (C-01/C-02). The "Connected" label is only valid in
//!   `Active`.
//! * A failed activation records `Error(reason, hint, retry_in_ms)` +
//!   backoff (2 s → 30 s). Nothing latches on a pairing reason
//!   anymore (pairing does not exist). F21: `BindFailed(AddrInUse)`
//!   stops auto-retry (`retry_in_ms: None` — the port holder can
//!   only be closed by the user) and surfaces as a plain `Error`.
//! * Heartbeat watchdog (C-04): no stream/status frame for 2 s in
//!   `Active` → transport rebuild + re-activate. F20: capped at
//!   [`MAX_WATCHDOG_REBUILDS`] consecutive frameless rebuilds, then
//!   the manager latches `Error(WatchdogStall)` with no retry timer
//!   (user Retry required) — only a real frame resets the count.
//! * HELLO wait (C-03): 750 ms per attempt (above the firmware's
//!   500 ms reply limiter), 3 attempts.
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
    /// A HELLO arrived; waiting for `GET_CAPS` to start.
    Hello,
    /// Unsigned `GET_CAPS` passed — the only "Connected" state.
    Active,
    /// Failed with a typed reason. `retry_in_ms` is `None` when
    /// auto-retry is stopped (`AddrInUse`, watchdog latch).
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
            Self::LinkUp | Self::Hello => "Connecting",
            Self::Active => "Connected",
            // D10: "Error" scared users when the wheel was simply
            // not there — the sidebar now says what is actually
            // wrong. Red is reserved for real faults (see
            // `is_fault`); the rest read as not-found / dropped.
            Self::Error { reason, .. } => match reason {
                ConnectionError::Unreachable => "Wheel not found",
                ConnectionError::Timeout | ConnectionError::TransportClosed => "Disconnected",
                _ => "Error",
            },
            Self::Backoff { .. } => "Reconnecting",
        }
    }

    /// True when the state is a REAL fault — the sidebar dot is
    /// red. Anything else unconnected (not found, dropped,
    /// retrying) is amber: the wheel being off is not a fault in
    /// the app. Faults: a foreign board answered, the watchdog
    /// latched, the UDP port is held by another app, a malformed
    /// HELLO, a credential-store failure, a rejected control
    /// command, or an unknown failure.
    #[must_use]
    pub fn is_fault(&self) -> bool {
        matches!(
            self,
            Self::Error {
                reason: ConnectionError::BoardMismatch
                    | ConnectionError::WatchdogStall
                    | ConnectionError::BindFailed(_)
                    | ConnectionError::Keystore
                    | ConnectionError::ControlFailed
                    | ConnectionError::MalformedHello
                    | ConnectionError::Unknown,
                ..
            }
        )
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
    /// The user (or autostart) asked to connect. This is also the
    /// retry path (R3-D-06): the former `UserRetry` event had zero
    /// production callers, so `Connect` from `Error` — which resets
    /// the failure and watchdog budgets exactly like a retry — is
    /// the one documented retry event.
    Connect,
    /// The transport bound and the link is up. P1-1 (v2.0.11):
    /// no production caller feeds this — `Connect` and
    /// `RetryTimer` move the manager to `LinkUp` themselves, and
    /// the activator/forwarder own the link. Kept on the enum so
    /// the wire shape and any harness stay compiling; `on_event`
    /// deliberately ignores it (single retry owner: the
    /// activator/forwarder, never the manager).
    LinkUp,
    /// A HELLO arrived (the board is reachable; no pairing
    /// state rides along anymore — v2.2 HELLO carries none).
    HelloReceived,
    /// Unsigned `GET_CAPS` passed — the session is live.
    CapsOk,
    /// Activation failed with a typed reason.
    ActivationFailed(ConnectionError),
    /// One HELLO wait expired with no reply. P1-1 (v2.0.11):
    /// no production caller feeds this — the HELLO wait lives in
    /// the activator (`await_new_session_hello`, 3 × 750 ms) and
    /// its expiry surfaces as `ActivationFailed(Unreachable)`.
    /// Kept on the enum for the same reason as `LinkUp`;
    /// `on_event` deliberately ignores it.
    HelloTimeout,
    /// A stream or status frame arrived (heartbeat).
    FrameSeen,
    /// The transport was lost (source drop / bind death). P1-1
    /// (v2.0.11): no production caller feeds this — transport
    /// loss is owned by the forwarder + the heartbeat watchdog
    /// (`check_watchdog` → `RebuildTransport`), and a dead
    /// transport during activation surfaces as
    /// `ActivationFailed(TransportClosed)`. Kept on the enum for
    /// the same reason as `LinkUp`; `on_event` deliberately
    /// ignores it.
    TransportLost,
    /// The UDP bind failed (typed). P1-1 (v2.0.11): no production
    /// caller feeds this — a bind failure surfaces as
    /// `ActivationFailed(BindFailed(..))`. Kept on the enum for
    /// the same reason as `LinkUp`; `on_event` deliberately
    /// ignores it.
    BindFailed(crate::errors::BindErrorKind),
    /// The backoff/retry timer fired.
    RetryTimer,
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
    /// Start the unsigned `GET_CAPS` (Hello → Active).
    StartActivation,
    /// Rebuild the transport, then re-activate from `LinkUp`.
    RebuildTransport,
    /// Re-activate on the live transport (no rebuild needed).
    Reactivate,
    /// The state changed — the driver must emit
    /// `AppEvent::ConnectionChanged` (the manager also records the
    /// log line; see `take_log_lines`).
    StateChanged,
}

/// The L1 Connection Manager: pure state machine + backoff +
/// watchdog + HELLO attempts.
#[derive(Debug)]
pub struct ConnectionManager {
    state: ConnectionState,
    /// Consecutive failed attempts (drives the 2 s → 30 s backoff).
    consecutive_failures: u32,
    /// HELLO attempts used in the current activation.
    hello_attempts: u32,
    /// Last stream/status frame seen (heartbeat, C-04).
    last_frame_at: Option<Instant>,
    /// Consecutive watchdog rebuilds with no frame seen (F20):
    /// reset only by [`ConnEvent::FrameSeen`], a fresh
    /// [`ConnEvent::Connect`], or [`ConnEvent::Disconnect`].
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
                    self.watchdog_rebuilds = 0;
                    self.transition(ConnectionState::LinkUp, now);
                    return ManagerAction::RequestHello;
                }
                ManagerAction::None
            }
            ConnEvent::LinkUp => {
                // P1-1 (v2.0.11): dead arm, deliberately a no-op.
                // No production caller feeds `LinkUp` (`Connect`
                // and `RetryTimer` already land the manager in
                // `LinkUp`); the link itself is owned by the
                // activator/forwarder. Returning an action here
                // would make the manager a second retry owner.
                ManagerAction::None
            }
            ConnEvent::HelloReceived => {
                // R3-D-01: accept from `Backoff` too. After a
                // watchdog rebuild the manager sits in `Backoff`
                // while the forwarder re-activates on its own —
                // the HELLO that re-opens the session is real
                // evidence the board is reachable, so the manager
                // must follow it into `Hello` instead of waiting
                // for its own `RetryTimer` (which the driver now
                // skips while the forwarder owns the retry). The
                // backoff is over once activation starts: disarm
                // its timer.
                // P1-2 (v2.0.11): accept from `Error` too. A
                // failed activation clears the core's
                // `board_reachable` latch, so the next HELLO is a
                // fresh rising edge the daemon loop feeds — and
                // without this arm the manager would drop it in
                // `Error` and never re-activate (the old code only
                // accepted it from `LinkUp`/`Hello`/`Backoff`).
                // `CapsOk` was already accepted from `Error`
                // (R2-01); accepting the HELLO that precedes it
                // is the same shape, not a shortcut around
                // `GET_CAPS`.
                if matches!(
                    self.state,
                    ConnectionState::LinkUp
                        | ConnectionState::Hello
                        | ConnectionState::Backoff { .. }
                        | ConnectionState::Error { .. }
                ) {
                    self.hello_attempts = 0;
                    self.backoff_until = None;
                    self.transition(ConnectionState::Hello, now);
                    // Any HELLO means the board is reachable: one
                    // unsigned GET_CAPS decides Active. There is no
                    // unpaired/window fail-closed anymore.
                    return ManagerAction::StartActivation;
                }
                ManagerAction::None
            }
            ConnEvent::CapsOk => {
                // R2-01 + R3-D-01: the forwarder may feed `CapsOk`
                // without a preceding `HelloReceived` (the manager
                // sits in `LinkUp` or in `Error` when the session
                // gate opens) — and, after a watchdog rebuild, in
                // `Backoff`: the gate re-opens while the manager
                // still waits out its backoff, and dropping the
                // event stranded it in "Connecting" on a live
                // session (no further gate edge ever arrives).
                // `CapsOk` is only ever fed on the gate-open rising
                // edge, and the gate only opens after an unsigned
                // `GET_CAPS` passed (`activate_session_on`,
                // `Ok(caps)` arm) — so accepting it from `LinkUp` /
                // `Error` / `Backoff` is still "Active only after
                // GET_CAPS" (C-01/C-02), never a shortcut around
                // it. Leaving `Backoff` disarms its timer.
                if matches!(
                    self.state,
                    ConnectionState::Hello
                        | ConnectionState::LinkUp
                        | ConnectionState::Error { .. }
                        | ConnectionState::Backoff { .. }
                ) {
                    self.consecutive_failures = 0;
                    self.backoff_until = None;
                    self.last_frame_at = Some(now);
                    self.transition(ConnectionState::Active, now);
                    return ManagerAction::StateChanged;
                }
                ManagerAction::None
            }
            ConnEvent::ActivationFailed(reason) => self.fail(reason, now),
            // P1-1 (v2.0.11): dead arms, deliberately no-ops (see
            // the `ConnEvent` docs). `HelloTimeout` is owned by
            // the activator, `TransportLost` by the forwarder +
            // watchdog, `BindFailed` arrives as
            // `ActivationFailed(BindFailed(..))` — giving any of
            // them behaviour here would make the manager a second
            // retry owner next to the activator/forwarder. The
            // variants stay on the enum so the wire shape and any
            // harness keep compiling; feeding one must change
            // nothing (covered by
            // `dead_manager_arms_are_noops` below).
            ConnEvent::HelloTimeout | ConnEvent::TransportLost | ConnEvent::BindFailed(_) => {
                ManagerAction::None
            }
            ConnEvent::RetryTimer => {
                if let ConnectionState::Backoff { attempt, .. } = self.state {
                    self.backoff_until = None;
                    self.consecutive_failures = attempt;
                    self.transition(ConnectionState::LinkUp, now);
                    return ManagerAction::RequestHello;
                }
                // R2-04: a generic failure lands in `Error` with
                // a promised retry (`retry_in_ms: Some`) and an
                // armed `backoff_until` — when the timer fires,
                // the retry is the same re-activation as from
                // `Backoff`: back to `LinkUp` + `RequestHello`.
                // Without this arm the loop would deliver a no-op
                // `RetryTimer` every 250 ms forever and the
                // promised retry would never happen. The
                // `retry_in_ms: None` states (user action,
                // `AddrInUse`, watchdog latch) never arm the timer,
                // so they never reach this branch.
                // `consecutive_failures` is deliberately NOT reset
                // here (unlike `Connect`): `fail()`
                // already counted this failure, so the next
                // failure backs off further (2 s → 30 s).
                if let ConnectionState::Error {
                    retry_in_ms: Some(_),
                    ..
                } = self.state
                {
                    self.backoff_until = None;
                    self.transition(ConnectionState::LinkUp, now);
                    return ManagerAction::RequestHello;
                }
                ManagerAction::None
            }
            ConnEvent::Disconnect => {
                self.consecutive_failures = 0;
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

    /// Record a failure: everything backs off 2 s → 30 s, except
    /// F21: a UDP `AddrInUse` is NOT retried at all — the port
    /// holder (usually the still-running legacy RSH app) can only
    /// be closed by the user, so retrying just re-logs the same
    /// failure forever (user log: 117× `AddrInUse` over ~35 min).
    /// It lands in `Error` with `retry_in_ms: None` and returns
    /// `StateChanged`.
    fn fail(&mut self, reason: ConnectionError, now: Instant) -> ManagerAction {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        // F21: `AddrInUse` never self-heals — stop here. No retry
        // timer is armed (`backoff_until` stays `None`), so the
        // driver's retry tick never re-fires `connect_session`
        // into another doomed bind. The explicit clear also drops
        // a stale backoff (R2-04): without it the loop would keep
        // delivering a no-op `RetryTimer` every 250 ms against
        // this latched `Error`.
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
        // R2-04: the `Error` above promises a retry in `delay`,
        // so arm the timer the manager loop polls
        // (`manager_retry_due_at`) — without this the retry tick
        // never fires and the manager latches in `Error` with a
        // `retry_in_ms` nobody honours.
        self.backoff_until = Some(now + delay);
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
        m.on_event(ConnEvent::HelloReceived, t);
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
        // CapsOk before HELLO must not activate (C-01/C-02).
        let mut m2 = ConnectionManager::new();
        m2.on_event(ConnEvent::CapsOk, t);
        assert!(!m2.state().is_active());
        assert_eq!(
            m.on_event(ConnEvent::HelloReceived, t),
            ManagerAction::StartActivation
        );
        assert!(!m.state().is_active());
        m.on_event(ConnEvent::CapsOk, t);
        assert!(m.state().is_active());
        assert_eq!(m.state().label(), "Connected");
    }

    #[test]
    fn error_label_names_not_found_instead_of_error() {
        // D10: the sidebar must not scare the user with "Error"
        // when the wheel is simply not there — and the dot is
        // amber, not red, for a missing wheel.
        let t = now();
        let mut m = ConnectionManager::new();
        m.on_event(ConnEvent::Connect, t);
        let _ = m.on_event(ConnEvent::ActivationFailed(ConnectionError::Unreachable), t);
        assert_eq!(m.state().label(), "Wheel not found");
        assert!(!m.state().is_fault());
        // A dropped transport reads as Disconnected, not Error.
        let mut m = ConnectionManager::new();
        m.on_event(ConnEvent::Connect, t);
        let _ = m.on_event(
            ConnEvent::ActivationFailed(ConnectionError::TransportClosed),
            t,
        );
        assert_eq!(m.state().label(), "Disconnected");
        assert!(!m.state().is_fault());
        // A real fault keeps "Error" + the red dot.
        for reason in [
            ConnectionError::BoardMismatch,
            ConnectionError::WatchdogStall,
            ConnectionError::Unknown,
            ConnectionError::BindFailed(crate::errors::BindErrorKind::AddrInUse),
        ] {
            let mut m = ConnectionManager::new();
            m.on_event(ConnEvent::Connect, t);
            let _ = m.on_event(ConnEvent::ActivationFailed(reason), t);
            assert_eq!(m.state().label(), "Error", "{reason:?}");
            assert!(m.state().is_fault(), "{reason:?}");
        }
    }

    #[test]
    fn hello_received_starts_activation() {
        // No-pairing redesign: any HELLO means the board is
        // reachable — there is no unpaired/window fail-closed.
        let mut m = ConnectionManager::new();
        let t = now();
        m.on_event(ConnEvent::Connect, t);
        assert_eq!(
            m.on_event(ConnEvent::HelloReceived, t),
            ManagerAction::StartActivation
        );
        assert!(matches!(m.state(), ConnectionState::Hello));
    }

    #[test]
    fn dead_manager_arms_are_noops() {
        // P1-1 (v2.0.11): `HelloTimeout` / `TransportLost` /
        // `BindFailed` / `LinkUp` have no production caller — the
        // HELLO wait lives in the activator, transport loss in the
        // forwarder + watchdog, bind failure arrives as
        // `ActivationFailed(BindFailed(..))`. Feeding any of them
        // must change nothing: the manager must not become a second
        // retry owner. (`LinkUp` is covered too: `Connect` and
        // `RetryTimer` already land the manager there.)
        for event in [
            ConnEvent::HelloTimeout,
            ConnEvent::TransportLost,
            ConnEvent::BindFailed(crate::errors::BindErrorKind::AddrInUse),
            ConnEvent::LinkUp,
        ] {
            // From `LinkUp` (a live attempt) and from `Active` (a
            // live session): neither may move, retry, or rebuild.
            for setup in [false, true] {
                let mut m = ConnectionManager::new();
                let t = now();
                m.on_event(ConnEvent::Connect, t);
                if setup {
                    m.on_event(ConnEvent::HelloReceived, t);
                    m.on_event(ConnEvent::CapsOk, t);
                    assert!(m.state().is_active());
                }
                let before = m.state().clone();
                assert_eq!(m.on_event(event, t), ManagerAction::None, "{event:?}");
                assert_eq!(*m.state(), before, "{event:?} must not move the state");
                assert_eq!(m.retry_due_at(), None, "{event:?} must not arm a retry");
            }
        }
    }

    #[test]
    fn hello_received_from_error_restarts_activation() {
        // P1-2 (v2.0.11): after a failed activation the manager
        // sits in `Error`; the next HELLO (a fresh rising edge —
        // the core clears the reachable latch on the failure) must
        // restart activation instead of being dropped.
        let mut m = ConnectionManager::new();
        let t = now();
        m.on_event(ConnEvent::Connect, t);
        let _ = m.on_event(ConnEvent::ActivationFailed(ConnectionError::Unreachable), t);
        assert!(matches!(m.state(), ConnectionState::Error { .. }));
        assert_eq!(
            m.on_event(ConnEvent::HelloReceived, t),
            ManagerAction::StartActivation
        );
        assert!(matches!(m.state(), ConnectionState::Hello));
        m.on_event(ConnEvent::CapsOk, t);
        assert!(m.state().is_active());
    }

    #[test]
    fn generic_failures_back_off_with_retry() {
        // No-pairing redesign: no failure latches on a pairing
        // reason — every activation failure backs off 2 s → 30 s
        // with a retry timer armed.
        for reason in [
            ConnectionError::Unreachable,
            ConnectionError::Timeout,
            ConnectionError::TransportClosed,
            ConnectionError::BoardMismatch,
            ConnectionError::Unknown,
        ] {
            let mut m = ConnectionManager::new();
            let t = now();
            m.on_event(ConnEvent::Connect, t);
            let action = m.on_event(ConnEvent::ActivationFailed(reason), t);
            assert_eq!(action, ManagerAction::StateChanged, "{reason:?}");
            assert!(matches!(
                m.state(),
                ConnectionState::Error {
                    retry_in_ms: Some(_),
                    ..
                }
            ));
            assert!(m.retry_due_at().is_some(), "{reason:?}");
        }
    }

    #[test]
    fn bind_failed_addr_in_use_stops_auto_retry() {
        // F21: the port holder (usually the still-running legacy
        // RSH app) can only be closed by the user — retrying the
        // bind just re-logs the same failure forever (user log:
        // 117× `AddrInUse` over ~35 min). So no retry is scheduled
        // and the state is a plain `Error`.
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
    fn caps_ok_from_link_up_reaches_active() {
        // R2-01: the forwarder may feed `CapsOk` without a
        // preceding `HelloReceived` — the manager sits in `LinkUp`
        // (where startup `Connect` left it) when the session gate
        // opens. The gate only opens after an unsigned `GET_CAPS`
        // passed, so reaching `Active` from here is still "Active
        // only after GET_CAPS".
        let mut m = ConnectionManager::new();
        let t = now();
        m.on_event(ConnEvent::Connect, t);
        assert!(matches!(m.state(), ConnectionState::LinkUp));
        assert_eq!(
            m.on_event(ConnEvent::CapsOk, t),
            ManagerAction::StateChanged
        );
        assert!(m.state().is_active());
    }

    #[test]
    fn caps_ok_from_error_reaches_active() {
        // R2-01: same resume path, but a failed attempt latched
        // the manager in `Error` first and the forwarder then
        // re-activated successfully — the gate opening must still
        // bring the manager to `Active` instead of stranding it.
        let mut m = ConnectionManager::new();
        let t = now();
        m.on_event(ConnEvent::Connect, t);
        let _ = m.on_event(ConnEvent::ActivationFailed(ConnectionError::Unreachable), t);
        assert!(matches!(m.state(), ConnectionState::Error { .. }));
        assert_eq!(
            m.on_event(ConnEvent::CapsOk, t),
            ManagerAction::StateChanged
        );
        assert!(m.state().is_active());
    }

    #[test]
    fn generic_failure_arms_retry_timer_and_retries() {
        // R2-04: a generic (retryable) failure promises a retry
        // in `retry_in_ms` — the timer must actually be armed,
        // and firing it must re-activate instead of no-op-ing.
        let mut m = ConnectionManager::new();
        let t = now();
        m.on_event(ConnEvent::Connect, t);
        let action = m.on_event(ConnEvent::ActivationFailed(ConnectionError::Unreachable), t);
        assert_eq!(action, ManagerAction::StateChanged);
        assert!(matches!(
            m.state(),
            ConnectionState::Error {
                retry_in_ms: Some(2000),
                ..
            }
        ));
        assert_eq!(m.retry_due_at(), Some(t + Duration::from_secs(2)));
        // Firing early is harmless (the daemon loop only fires
        // at/after the deadline) but must not re-activate yet:
        // the loop gates on the deadline, the manager on state.
        assert_eq!(
            m.on_event(ConnEvent::RetryTimer, t + Duration::from_secs(2)),
            ManagerAction::RequestHello
        );
        assert!(matches!(m.state(), ConnectionState::LinkUp));
        assert_eq!(m.retry_due_at(), None, "fired timer is disarmed");
        // A second failure backs off further (2 s -> 4 s).
        let _ = m.on_event(ConnEvent::ActivationFailed(ConnectionError::Unreachable), t);
        assert!(matches!(
            m.state(),
            ConnectionState::Error {
                retry_in_ms: Some(4000),
                ..
            }
        ));
        assert_eq!(m.retry_due_at(), Some(t + Duration::from_secs(4)));
    }

    #[test]
    fn retry_timer_is_noop_for_latched_addr_in_use_error() {
        // R2-04: `retry_in_ms: None` states never arm the timer,
        // so a stray `RetryTimer` must not re-activate them.
        let mut m = ConnectionManager::new();
        let t = now();
        m.on_event(ConnEvent::Connect, t);
        let _ = m.on_event(
            ConnEvent::ActivationFailed(ConnectionError::BindFailed(
                crate::errors::BindErrorKind::AddrInUse,
            )),
            t,
        );
        assert_eq!(m.retry_due_at(), None);
        assert_eq!(m.on_event(ConnEvent::RetryTimer, t), ManagerAction::None);
        assert!(matches!(
            m.state(),
            ConnectionState::Error {
                reason: ConnectionError::BindFailed(crate::errors::BindErrorKind::AddrInUse),
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
        m.on_event(ConnEvent::HelloReceived, t);
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
        // A manual retry gets a fresh budget. (R3-D-06: the
        // former `UserRetry` event is deleted — `Connect` from
        // `Error` is the retry path and resets the budgets.)
        assert_eq!(
            m.on_event(ConnEvent::Connect, t),
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
    fn caps_ok_from_backoff_reaches_active() {
        // R3-D-01: after a watchdog rebuild the manager sits in
        // `Backoff` while the forwarder re-activates on its own —
        // the gate opening must still bring the manager to
        // `Active` instead of stranding it in "Connecting" on a
        // live session. The gate only opens after an unsigned
        // `GET_CAPS` passed, so this is still "Active only after
        // GET_CAPS".
        let mut m = active_manager();
        let t = now();
        m.on_event(ConnEvent::FrameSeen, t);
        assert_eq!(
            m.check_watchdog(t + Duration::from_millis(2001)),
            ManagerAction::RebuildTransport
        );
        assert!(matches!(m.state(), ConnectionState::Backoff { .. }));
        assert!(m.retry_due_at().is_some(), "backoff arms a timer");
        assert_eq!(
            m.on_event(ConnEvent::CapsOk, t),
            ManagerAction::StateChanged
        );
        assert!(m.state().is_active());
        assert_eq!(
            m.retry_due_at(),
            None,
            "leaving Backoff via CapsOk disarms its timer"
        );
    }

    #[test]
    fn hello_received_from_backoff_starts_activation() {
        // R3-D-01: same watchdog-rebuild shape, but the HELLO
        // arrives before the gate opens — the manager must follow
        // it into `Hello` instead of dropping it in `Backoff`.
        let mut m = active_manager();
        let t = now();
        m.on_event(ConnEvent::FrameSeen, t);
        assert_eq!(
            m.check_watchdog(t + Duration::from_millis(2001)),
            ManagerAction::RebuildTransport
        );
        assert!(matches!(m.state(), ConnectionState::Backoff { .. }));
        assert_eq!(
            m.on_event(ConnEvent::HelloReceived, t),
            ManagerAction::StartActivation
        );
        assert!(matches!(m.state(), ConnectionState::Hello));
        assert_eq!(
            m.retry_due_at(),
            None,
            "leaving Backoff via HelloReceived disarms its timer"
        );
        m.on_event(ConnEvent::CapsOk, t);
        assert!(m.state().is_active());
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
