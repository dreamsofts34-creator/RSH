//! Pairing Manager — USB-first pairing (LOGIC-FIX-PLAN §4.2, Phase 6).
//!
//! Pure state machine, no I/O: the daemon driver feeds it board
//! observations (HELLO flags + `GET_DIAG` window-remaining) and it
//! answers what the UI must do and show. It fixes the live v1.0.2
//! bug (Phase 0 gate §4): the old 60 s window ran from AP-up and
//! never reopened, so "Begin pairing" always failed with "No pairing
//! window" on the user's PC.
//!
//! Rules this module enforces:
//!
//! * **USB-first.** When a USB serial link to the board exists,
//!   pairing goes over serial: every serial HELO opens the window
//!   on demand (firmware F-03, 120 s, refreshed per HELO), so there
//!   is no 60 s race at all.
//! * **Wi-Fi path.** The user holds the pair button 3 s (firmware
//!   F-02), which opens a 120 s window — but only on an *unpaired*
//!   board. The app reads the REAL remaining time from the board
//!   (`GET_DIAG`, F-10) and the displayed countdown is clamped to
//!   it: it never shows a longer countdown than the board reports
//!   (C-05 fix; HELLO carries no remaining-seconds field — the
//!   protocol is frozen, F-16 — so a local-only countdown is only
//!   ever a conservative fallback, never the source of truth).
//! * **Paired board re-pair is a separate guided flow** ("Reset
//!   pairing: hold BOOT for 10 s" — the 3 s button does nothing on
//!   a paired board, by firmware design).
//! * **Password policy (Phase 0 §6 + §4.2).** The one-time Wi-Fi
//!   password is shown once and dropped ONLY on the user's explicit
//!   "I saved it" confirm. It is re-showable from the keyring, but
//!   only after a confirmation dialog. This module tracks the
//!   lifecycle state only — it never holds the password value
//!   itself (the core/keyring holds it), so no secret can leak
//!   through this module, its logs, or its tests.
//! * **Success = `Active` only.** "Password saved" is NOT a
//!   success tick: [`PairingManager::is_paired`] is true only after
//!   the driver reports the session reached `Active` (signed
//!   `GET_CAPS` passed, C-02).

use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::errors::ConnectionError;

/// Boot window: the legacy 60 s window from AP-up (firmware
/// `RSH_PAIR_WINDOW_BOOT_MS`). Still the fallback when the board
/// reports a window but no `GET_DIAG` remaining time yet.
pub const BOOT_WINDOW_SECS: u64 = 60;

/// Pair-button / serial window: 120 s (firmware
/// `RSH_PAIR_WINDOW_BUTTON_MS`, F-02/F-03).
pub const BUTTON_WINDOW_SECS: u64 = 120;

/// How long the user must hold the pair button (firmware F-02,
/// `RSH_BOOTHOLD_PAIR_MS`).
pub const PAIR_BUTTON_HOLD_SECS: u64 = 3;

/// How long the user must hold BOOT to reset the pairing key
/// (`KEY_RESET`, 10 s hold).
pub const KEY_RESET_HOLD_SECS: u64 = 10;

/// Transport a pairing attempt runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PairingTransport {
    /// Wired USB-serial — tried first whenever a link exists.
    Usb,
    /// Wi-Fi UDP (pair button opens the window).
    Wifi,
}

impl PairingTransport {
    /// Short id (`"usb"` / `"wifi"`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Usb => "usb",
            Self::Wifi => "wifi",
        }
    }
}

/// One board observation, as the driver last saw it: HELLO flags
/// plus the `GET_DIAG` window-remaining (F-10). `remaining_s` is
/// `None` when no `GET_DIAG` reply has arrived yet — the manager
/// then falls back to a conservative local countdown and never
/// claims more time than the local window allows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardReport {
    /// The board reports itself unpaired.
    pub unpaired: bool,
    /// The pairing window is open right now (HELLO flag).
    pub window_open: bool,
    /// Real seconds left in the window, from `GET_DIAG` (F-10).
    /// `None` = not yet reported.
    pub remaining_s: Option<u64>,
    /// Board MAC of the last HELLO, when known.
    pub board_mac: Option<String>,
    /// The board's key-reset is armed (HELLO flag, M-6).
    pub key_reset_armed: bool,
}

impl BoardReport {
    /// Build from a HELLO with no `GET_DIAG` reading yet.
    #[must_use]
    pub fn from_hello(unpaired: bool, window_open: bool, key_reset_armed: bool) -> Self {
        Self {
            unpaired,
            window_open,
            remaining_s: None,
            board_mac: None,
            key_reset_armed,
        }
    }
}

/// Pairing-window state, as the UI renders it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WindowState {
    /// No board has been seen yet — nothing to show but "looking".
    Unknown,
    /// The window is open; `seconds_left` is already clamped to
    /// the board-reported remaining time (never longer).
    Open {
        /// Seconds left (board-clamped, 0 never reported as open).
        seconds_left: u64,
        /// Transport the open window belongs to.
        transport: PairingTransport,
    },
    /// The window is closed, with the typed reason.
    Closed {
        /// Why there is no window.
        reason: ConnectionError,
    },
}

/// What `begin_pairing` tells the driver/UI to do next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairingAction {
    /// Pair over the USB serial link: the next serial HELO opens
    /// the window on demand (F-03) — no user action needed.
    PairOverUsb,
    /// Wi-Fi: the user must hold the pair button 3 s to open the
    /// 120 s window (F-02). The UI shows the hold instruction.
    HoldPairButton,
    /// The board is already paired: pairing is the wrong operation.
    /// The UI must offer the guided reset flow instead.
    ResetRequired,
    /// No board has been seen on either transport yet.
    NoBoard,
}

/// Lifecycle of the one-time Wi-Fi password in the UI (Phase 0 §6).
/// The manager holds state only — never the password value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum PasswordState {
    /// No password has been disclosed for this attempt.
    Hidden,
    /// Disclosed once, waiting for the user's explicit "I saved it".
    Shown,
    /// User confirmed "I saved it": dropped from the UI. It stays
    /// in the keyring and can be re-shown after a confirmation
    /// dialog ([`PairingManager::request_password_reshow`]).
    Saved,
}

/// Outcome of the pairing attempt. Only `Active` counts as paired.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairingOutcome {
    /// Pairing is still in progress (or not started).
    InProgress,
    /// The session reached `Active` — paired. This is the ONLY
    /// success state; "password saved" never sets it.
    Paired,
    /// The attempt failed with a typed reason.
    Failed(ConnectionError),
}

/// Guided reset instruction shown for a paired board (§4.2: the
/// 3 s pair button only works on an unpaired board).
pub const RESET_INSTRUCTION: &str = "Reset pairing: hold BOOT for 10 s";

/// The Pairing Manager: USB-first selection, board-clamped window
/// countdown, password lifecycle, and the Active-only success gate.
#[derive(Debug)]
pub struct PairingManager {
    /// A USB serial link to the board exists.
    usb_available: bool,
    /// Last board observation, if any.
    board: Option<BoardReport>,
    /// When the last board report arrived.
    board_seen_at: Option<Instant>,
    /// When the current window was first observed open.
    window_opened_at: Option<Instant>,
    /// Total seconds of the current window (60 boot / 120
    /// button-or-serial), for the local fallback countdown.
    window_total_s: u64,
    /// One-time password lifecycle state (value held elsewhere).
    password: PasswordState,
    /// Pairing outcome — `Paired` only after `Active`.
    outcome: PairingOutcome,
    /// The guided reset flow is active (paired board re-pair).
    reset_flow: bool,
    /// Redacted log lines for the diagnostics log (never a secret).
    log_lines: Vec<String>,
}

impl Default for PairingManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PairingManager {
    /// Empty manager: no board seen, no USB link, no password.
    #[must_use]
    pub fn new() -> Self {
        Self {
            usb_available: false,
            board: None,
            board_seen_at: None,
            window_opened_at: None,
            window_total_s: BOOT_WINDOW_SECS,
            password: PasswordState::Hidden,
            outcome: PairingOutcome::InProgress,
            reset_flow: false,
            log_lines: Vec::new(),
        }
    }

    /// Record whether a USB serial link to the board exists.
    /// USB-first selection keys off this flag.
    pub fn set_usb_available(&mut self, available: bool) {
        if self.usb_available != available {
            self.usb_available = available;
            self.log_lines.push(format!(
                "pairing: usb link {}",
                if available { "available" } else { "lost" }
            ));
        }
    }

    /// Whether a USB serial link is currently known.
    #[must_use]
    pub const fn usb_available(&self) -> bool {
        self.usb_available
    }

    /// USB-first transport selection (§4.2): serial whenever a USB
    /// link exists, Wi-Fi otherwise.
    #[must_use]
    pub const fn select_transport(&self) -> PairingTransport {
        if self.usb_available {
            PairingTransport::Usb
        } else {
            PairingTransport::Wifi
        }
    }

    /// Feed one board observation (HELLO flags + `GET_DIAG`
    /// remaining). A window that transitions closed → open starts
    /// the local fallback countdown; a fresh `remaining_s` always
    /// wins over the local countdown.
    pub fn note_board_report(&mut self, report: BoardReport) {
        let now = Instant::now();
        let was_open = self.board.as_ref().is_some_and(|b| b.window_open);
        if report.window_open && !was_open {
            self.window_opened_at = Some(now);
            // A serial HELO or a pair-button press opens 120 s; the
            // boot window is 60 s. Without a GET_DIAG reading the
            // local fallback must assume the shorter window.
            self.window_total_s =
                if self.usb_available || report.remaining_s.is_some_and(|s| s > BOOT_WINDOW_SECS) {
                    BUTTON_WINDOW_SECS
                } else {
                    BOOT_WINDOW_SECS
                };
            self.log_lines.push(format!(
                "pairing: window opened (transport={}, board_remaining_s={})",
                self.select_transport().as_str(),
                report
                    .remaining_s
                    .map_or_else(|| "-".to_string(), |s| s.to_string()),
            ));
        } else if !report.window_open && was_open {
            self.log_lines.push("pairing: window closed".to_string());
        }
        self.board = Some(report);
        self.board_seen_at = Some(now);
    }

    /// Feed a fresh `GET_DIAG` remaining-seconds reading without a
    /// full HELLO (F-10). This is what the countdown clamps to.
    pub fn note_window_remaining(&mut self, remaining_s: u64) {
        let now = Instant::now();
        if let Some(board) = self.board.as_mut() {
            board.remaining_s = Some(remaining_s);
            board.window_open = remaining_s > 0 && board.unpaired;
            self.board_seen_at = Some(now);
            if board.window_open && self.window_opened_at.is_none() {
                self.window_opened_at = Some(now);
                self.window_total_s = BUTTON_WINDOW_SECS;
            }
        }
    }

    /// Seconds left in the pairing window, clamped to the
    /// board-reported remaining time (C-05 fix). Returns 0 when no
    /// window is open, no board has been seen, or the board is
    /// already paired. The value shown in the UI must always come
    /// from here — never from a local 60 s assumption.
    #[must_use]
    pub fn seconds_left(&self) -> u64 {
        let Some(board) = &self.board else {
            return 0;
        };
        if !board.unpaired || !board.window_open {
            return 0;
        }
        let elapsed_since_report = self.board_seen_at.map_or(0, |at| at.elapsed().as_secs());
        // Board-reported remaining, aged since the report arrived.
        let board_left = board
            .remaining_s
            .map(|s| s.saturating_sub(elapsed_since_report));
        // Conservative local fallback: time since the window was
        // first observed open, against its total length.
        let local_left = self.window_opened_at.map_or(self.window_total_s, |at| {
            self.window_total_s.saturating_sub(at.elapsed().as_secs())
        });
        // Never show more than the board reports: when a board
        // reading exists it caps the local countdown.
        match board_left {
            Some(left) => left.min(local_left),
            None => local_left,
        }
    }

    /// Current window state for the UI.
    #[must_use]
    pub fn window_state(&self) -> WindowState {
        let Some(board) = &self.board else {
            return WindowState::Unknown;
        };
        if !board.unpaired {
            return WindowState::Closed {
                reason: ConnectionError::AlreadyPaired,
            };
        }
        let left = self.seconds_left();
        if board.window_open && left > 0 {
            WindowState::Open {
                seconds_left: left,
                transport: self.select_transport(),
            }
        } else if board.window_open {
            // Flag said open but the (clamped) countdown hit zero:
            // the window expired mid-attempt.
            WindowState::Closed {
                reason: ConnectionError::WindowExpired,
            }
        } else {
            WindowState::Closed {
                reason: ConnectionError::WindowClosed,
            }
        }
    }

    /// Decide the next pairing step from transport + board state.
    /// USB-first: a USB link pairs over serial (F-03 opens the
    /// window on demand); Wi-Fi needs the 3 s pair-button hold
    /// (F-02); a paired board needs the guided reset flow.
    #[must_use]
    pub fn begin_pairing(&mut self) -> PairingAction {
        let action = match &self.board {
            None => PairingAction::NoBoard,
            Some(board) if !board.unpaired => PairingAction::ResetRequired,
            Some(_) if self.usb_available => PairingAction::PairOverUsb,
            Some(_) => PairingAction::HoldPairButton,
        };
        self.log_lines.push(format!(
            "pairing: begin_pairing action={action:?} transport={}",
            self.select_transport().as_str()
        ));
        action
    }

    /// Start the guided re-pair flow for a paired board. Returns
    /// the instruction to show, or `None` when the board is not
    /// known-paired (the flow only applies then).
    #[must_use]
    pub fn begin_reset_flow(&mut self) -> Option<&'static str> {
        let paired = self.board.as_ref().is_some_and(|b| !b.unpaired);
        if !paired {
            return None;
        }
        self.reset_flow = true;
        self.log_lines
            .push("pairing: reset flow started (hold BOOT 10 s)".to_string());
        Some(RESET_INSTRUCTION)
    }

    /// Whether the guided reset flow is active.
    #[must_use]
    pub const fn reset_flow_active(&self) -> bool {
        self.reset_flow
    }

    /// Record the HELLO key-reset-armed flag (M-6): true once the
    /// user held BOOT long enough that a reset will actually work.
    /// Ends the reset flow's waiting state when armed.
    pub fn note_key_reset_armed(&mut self, armed: bool) {
        if armed && self.reset_flow {
            self.log_lines
                .push("pairing: key reset armed (BOOT hold seen)".to_string());
        }
    }

    /// The one-time Wi-Fi password was disclosed to the UI.
    /// Lifecycle: `Hidden` → `Shown`. The value itself is never
    /// passed here — only the fact of disclosure.
    pub fn note_password_disclosed(&mut self) {
        if self.password == PasswordState::Hidden {
            self.password = PasswordState::Shown;
            self.log_lines
                .push("pairing: one-time password disclosed to UI (single-use)".to_string());
        }
    }

    /// Current password lifecycle state.
    #[must_use]
    pub const fn password_state(&self) -> PasswordState {
        self.password
    }

    /// The user explicitly confirmed "I saved it": drop the
    /// password from the UI. This is the ONLY way the password
    /// leaves `Shown` — never on a poll, a re-render, or a window
    /// close. Returns true when a shown password was dropped.
    /// Note: this is NOT pairing success (see [`Self::is_paired`]).
    pub fn confirm_password_saved(&mut self) -> bool {
        if self.password == PasswordState::Shown {
            self.password = PasswordState::Saved;
            self.log_lines
                .push("pairing: one-time password dropped on explicit user confirm".to_string());
            true
        } else {
            false
        }
    }

    /// Re-show the password from the keyring after the user
    /// confirmed the re-show dialog. `confirmed` must be true
    /// (the dialog's answer) — otherwise nothing changes.
    /// Returns true when the password is shown again.
    pub fn request_password_reshow(&mut self, confirmed: bool) -> bool {
        if confirmed && self.password == PasswordState::Saved {
            self.password = PasswordState::Shown;
            self.log_lines
                .push("pairing: one-time password re-shown from keyring after confirm".to_string());
            true
        } else {
            false
        }
    }

    /// The session reached `Active` (signed `GET_CAPS` passed) —
    /// the ONLY success signal. Call this from the driver when the
    /// Connection Manager reports `Active`.
    pub fn note_active(&mut self) {
        self.outcome = PairingOutcome::Paired;
        self.reset_flow = false;
        self.log_lines.push("pairing: Active — paired".to_string());
    }

    /// The attempt failed with a typed reason.
    pub fn note_failed(&mut self, reason: ConnectionError) {
        self.outcome = PairingOutcome::Failed(reason);
        self.log_lines
            .push(format!("pairing: failed reason={}", reason.code()));
    }

    /// Current outcome.
    #[must_use]
    pub const fn outcome(&self) -> PairingOutcome {
        self.outcome
    }

    /// True only after the session reached `Active`. "Password
    /// saved" never makes this true.
    #[must_use]
    pub const fn is_paired(&self) -> bool {
        matches!(self.outcome, PairingOutcome::Paired)
    }

    /// Board MAC of the last HELLO, when one was observed.
    #[must_use]
    pub fn board_mac(&self) -> Option<&str> {
        self.board.as_ref().and_then(|b| b.board_mac.as_deref())
    }

    /// Drain the redacted pairing log lines (Phase 0 gate §6.2).
    /// Never contains a password, key, nonce, or HMAC value.
    pub fn take_log_lines(&mut self) -> Vec<String> {
        std::mem::take(&mut self.log_lines)
    }
}

/// SVG path commands for the Setup wizard's countdown ring
/// (132×132 viewBox, r=58, centre 66,66 — same geometry as the
/// `Setup.html` preview). The arc starts at the top and sweeps
/// clockwise for the fraction `seconds_left / total_s`.
///
/// Returns an empty string when nothing is left to show (the ring
/// is then just its track), and a full circle (two arcs) when the
/// window is full.
#[must_use]
pub fn countdown_ring_commands(seconds_left: u64, total_s: u64) -> String {
    if total_s == 0 || seconds_left == 0 {
        return String::new();
    }
    // Seconds are small (window totals are 60/120 s); saturate
    // through u32 so the f64 conversion is exact and clippy-clean.
    let seconds_left = u32::try_from(seconds_left).unwrap_or(u32::MAX);
    let total_s = u32::try_from(total_s).unwrap_or(u32::MAX);
    let fraction = (f64::from(seconds_left) / f64::from(total_s)).clamp(0.0, 1.0);
    if fraction >= 1.0 {
        // A single SVG arc cannot draw a full circle: two halves.
        return "M 66 8 A 58 58 0 1 1 66 124 A 58 58 0 1 1 66 8".to_string();
    }
    // Clockwise from the top (12 o'clock): angle measured from
    // straight up, x = cx + r·sin, y = cy − r·cos.
    let angle = fraction * 2.0 * std::f64::consts::PI;
    let x = 66.0 + 58.0 * angle.sin();
    let y = 66.0 - 58.0 * angle.cos();
    let large = i32::from(fraction > 0.5);
    format!("M 66 8 A 58 58 0 {large} 1 {x:.1} {y:.1}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_unpaired(remaining: Option<u64>) -> BoardReport {
        BoardReport {
            unpaired: true,
            window_open: true,
            remaining_s: remaining,
            board_mac: Some("AA:BB:CC:DD:EE:FF".to_string()),
            key_reset_armed: false,
        }
    }

    #[test]
    fn usb_first_when_link_present() {
        let mut mgr = PairingManager::new();
        assert_eq!(mgr.select_transport(), PairingTransport::Wifi);
        mgr.set_usb_available(true);
        assert_eq!(mgr.select_transport(), PairingTransport::Usb);
        mgr.set_usb_available(false);
        assert_eq!(mgr.select_transport(), PairingTransport::Wifi);
    }

    #[test]
    fn begin_pairing_usb_first_over_serial() {
        let mut mgr = PairingManager::new();
        mgr.set_usb_available(true);
        mgr.note_board_report(open_unpaired(None));
        assert_eq!(mgr.begin_pairing(), PairingAction::PairOverUsb);
    }

    #[test]
    fn begin_pairing_wifi_needs_pair_button_hold() {
        let mut mgr = PairingManager::new();
        mgr.note_board_report(open_unpaired(None));
        assert_eq!(mgr.begin_pairing(), PairingAction::HoldPairButton);
    }

    #[test]
    fn begin_pairing_no_board_seen() {
        let mut mgr = PairingManager::new();
        assert_eq!(mgr.begin_pairing(), PairingAction::NoBoard);
    }

    #[test]
    fn begin_pairing_paired_board_needs_reset() {
        let mut mgr = PairingManager::new();
        mgr.note_board_report(BoardReport {
            unpaired: false,
            window_open: false,
            remaining_s: None,
            board_mac: None,
            key_reset_armed: false,
        });
        assert_eq!(mgr.begin_pairing(), PairingAction::ResetRequired);
    }

    #[test]
    fn countdown_never_exceeds_board_report() {
        // The C-05 fix: board says 23 s left (GET_DIAG) — the UI
        // must show at most 23 s, even though the local window
        // total is 60 s or 120 s.
        let mut mgr = PairingManager::new();
        mgr.note_board_report(open_unpaired(Some(23)));
        assert!(mgr.seconds_left() <= 23);
        assert!(mgr.seconds_left() > 0);
        match mgr.window_state() {
            WindowState::Open { seconds_left, .. } => assert!(seconds_left <= 23),
            other => panic!("expected Open window, got {other:?}"),
        }
    }

    #[test]
    fn countdown_falls_back_to_local_window_without_diag() {
        // No GET_DIAG yet: conservative local countdown, bounded
        // by the boot window (60 s) — never more.
        let mut mgr = PairingManager::new();
        mgr.note_board_report(open_unpaired(None));
        let left = mgr.seconds_left();
        assert!(left > 0 && left <= BOOT_WINDOW_SECS);
    }

    #[test]
    fn window_closed_when_board_paired() {
        let mut mgr = PairingManager::new();
        mgr.note_board_report(BoardReport {
            unpaired: false,
            window_open: false,
            remaining_s: None,
            board_mac: None,
            key_reset_armed: false,
        });
        assert_eq!(
            mgr.window_state(),
            WindowState::Closed {
                reason: ConnectionError::AlreadyPaired
            }
        );
        assert_eq!(mgr.seconds_left(), 0);
    }

    #[test]
    fn window_unknown_before_any_board() {
        let mgr = PairingManager::new();
        assert_eq!(mgr.window_state(), WindowState::Unknown);
        assert_eq!(mgr.seconds_left(), 0);
    }

    #[test]
    fn window_closed_reason_when_window_shut() {
        let mut mgr = PairingManager::new();
        mgr.note_board_report(BoardReport {
            unpaired: true,
            window_open: false,
            remaining_s: None,
            board_mac: None,
            key_reset_armed: false,
        });
        assert_eq!(
            mgr.window_state(),
            WindowState::Closed {
                reason: ConnectionError::WindowClosed
            }
        );
    }

    #[test]
    fn note_window_remaining_updates_countdown() {
        let mut mgr = PairingManager::new();
        mgr.note_board_report(open_unpaired(None));
        mgr.note_window_remaining(90);
        let left = mgr.seconds_left();
        assert!(left > 0 && left <= 90);
    }

    #[test]
    fn password_dropped_only_on_explicit_confirm() {
        let mut mgr = PairingManager::new();
        // Not disclosed yet: nothing to drop.
        assert!(!mgr.confirm_password_saved());
        assert_eq!(mgr.password_state(), PasswordState::Hidden);
        mgr.note_password_disclosed();
        assert_eq!(mgr.password_state(), PasswordState::Shown);
        // A second disclosure does not re-arm or drop anything.
        mgr.note_password_disclosed();
        assert_eq!(mgr.password_state(), PasswordState::Shown);
        assert!(mgr.confirm_password_saved());
        assert_eq!(mgr.password_state(), PasswordState::Saved);
        // Confirming again is a no-op.
        assert!(!mgr.confirm_password_saved());
    }

    #[test]
    fn password_reshow_needs_confirmation_dialog() {
        let mut mgr = PairingManager::new();
        mgr.note_password_disclosed();
        mgr.confirm_password_saved();
        // No confirmation → stays dropped.
        assert!(!mgr.request_password_reshow(false));
        assert_eq!(mgr.password_state(), PasswordState::Saved);
        // Confirmed → shown again from the keyring.
        assert!(mgr.request_password_reshow(true));
        assert_eq!(mgr.password_state(), PasswordState::Shown);
    }

    #[test]
    fn password_saved_is_not_pairing_success() {
        // The §4.2 success criterion: "Password saved" is NOT a
        // success tick — only Active is.
        let mut mgr = PairingManager::new();
        mgr.note_password_disclosed();
        assert!(mgr.confirm_password_saved());
        assert!(!mgr.is_paired());
        assert_eq!(mgr.outcome(), PairingOutcome::InProgress);
        mgr.note_active();
        assert!(mgr.is_paired());
    }

    #[test]
    fn failure_and_active_outcomes() {
        let mut mgr = PairingManager::new();
        mgr.note_failed(ConnectionError::WindowExpired);
        assert_eq!(
            mgr.outcome(),
            PairingOutcome::Failed(ConnectionError::WindowExpired)
        );
        assert!(!mgr.is_paired());
        mgr.note_active();
        assert!(mgr.is_paired());
    }

    #[test]
    fn reset_flow_only_for_paired_board() {
        let mut mgr = PairingManager::new();
        // Unpaired board: no reset flow.
        mgr.note_board_report(open_unpaired(None));
        assert_eq!(mgr.begin_reset_flow(), None);
        assert!(!mgr.reset_flow_active());
        // Paired board: guided flow with the BOOT-hold instruction.
        mgr.note_board_report(BoardReport {
            unpaired: false,
            window_open: false,
            remaining_s: None,
            board_mac: None,
            key_reset_armed: false,
        });
        assert_eq!(mgr.begin_reset_flow(), Some(RESET_INSTRUCTION));
        assert!(mgr.reset_flow_active());
        assert!(RESET_INSTRUCTION.contains("10 s"));
        // Active clears the reset flow.
        mgr.note_active();
        assert!(!mgr.reset_flow_active());
    }

    #[test]
    fn log_lines_never_carry_secrets() {
        // The manager never holds the password value, so no log
        // line it emits can contain one — assert the shape: only
        // transport / state words, no key material.
        let mut mgr = PairingManager::new();
        mgr.set_usb_available(true);
        mgr.note_board_report(open_unpaired(Some(42)));
        mgr.note_password_disclosed();
        mgr.confirm_password_saved();
        for line in mgr.take_log_lines() {
            assert!(!line.contains("AA:BB"), "MAC must not be logged: {line}");
            assert!(line.starts_with("pairing: "), "{line}");
        }
    }

    #[test]
    fn ring_empty_at_zero_and_full_at_total() {
        assert_eq!(countdown_ring_commands(0, 120), "");
        assert_eq!(countdown_ring_commands(5, 0), "");
        let full = countdown_ring_commands(120, 120);
        assert!(full.contains("A 58 58"), "{full}");
        // Two arcs for a full circle.
        assert_eq!(full.matches('A').count(), 2);
    }

    #[test]
    fn ring_half_circle_ends_at_bottom() {
        // 60 of 120 s = half the ring: top (66,8) to bottom (66,124).
        // At exactly half, large-arc is 0 (either value draws a
        // semicircle); the endpoint is what matters.
        let cmds = countdown_ring_commands(60, 120);
        assert!(cmds.starts_with("M 66 8 A 58 58 0 0 1 "), "{cmds}");
        assert!(cmds.contains("66.0 124.0"), "{cmds}");
    }

    #[test]
    fn ring_quarter_circle_ends_at_right() {
        // 30 of 120 s = quarter: top to right (124,66).
        let cmds = countdown_ring_commands(30, 120);
        assert!(cmds.contains("124.0 66.0"), "{cmds}");
        assert!(cmds.contains("0 0 1"), "{cmds}");
    }
}
