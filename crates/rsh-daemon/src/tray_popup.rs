//! Tray popup (REDESIGN-PLAN §7.7, preview `Tray.html`): the
//! 340×440 window left-click opens next to the tray icon, and the
//! live data it shows.
//!
//! Split deliberately so the part that can be tested on Linux is
//! pure:
//! * [`anchor_popup`] — pure geometry: tray rect + screen work area
//!   + popup size → clamped top-left position. No Win32, no Slint.
//! * [`TrayPopupData::collect`] — pure mapping from the live
//!   [`AppApi`] + profile/game lists into the rows the `.slint`
//!   popup renders. No window, no timer.
//! * The window itself (show/hide/position/refresh timer/
//!   click-outside poll) lives in `main.rs`, on the Slint
//!   event-loop thread; the tray-icon rect comes from
//!   [`crate::tray_geometry`] (Windows-only).

use std::time::Duration;

use rsh_api::AppApi;
use rsh_app_core::state::{GameInfo, ProfileSummary};

/// Tray popup size, logical px (REDESIGN-PLAN §7.7, `Tray.html`).
pub const POPUP_WIDTH: i32 = 340;
/// Tray popup height, logical px.
pub const POPUP_HEIGHT: i32 = 440;
/// Gap between the tray icon and the popup, logical px.
pub const POPUP_GAP: i32 = 8;
/// Grace after the popup is shown during which the foreground poll
/// never dismisses it (A4): showing the popup cannot instantly
/// close it — the first poll after the tray click must not count
/// as "clicked away".
pub const POPUP_FOREGROUND_GRACE: Duration = Duration::from_millis(500);
/// Settle window after the grace during which the poll re-samples
/// its foreground baseline (F9, v2.0.3): Windows settles back to
/// the pre-click app after the tray click, and that settle can
/// finish after the grace expires, so a baseline sampled at show
/// time is stale. The baseline is armed inside
/// `grace .. grace + settle` and never before.
pub const POPUP_BASELINE_SETTLE: Duration = Duration::from_millis(500);
/// Consecutive 100 ms polls showing the same foreground window
/// needed to arm the baseline (~300 ms of stability).
pub const POPUP_BASELINE_STABLE_POLLS: u32 = 3;
/// Consecutive 100 ms polls showing a foreground window different
/// from the armed baseline needed to dismiss (~300 ms — a single
/// fluky `GetForegroundWindow` read never closes the popup).
pub const POPUP_DISMISS_CHANGED_POLLS: u32 = 3;
/// Continuous foreground time after which losing the foreground
/// counts as a click-away (F9): a popup that was foreground for
/// this long was being used, so the loss was caused by the user.
pub const POPUP_FOREGROUND_SUSTAINED: Duration = Duration::from_millis(500);
/// How long the show-time activation is retried on timer ticks
/// (F9): Slint may not have created the popup's native window yet
/// when `show()` runs, so the first activation attempt can find no
/// window to activate.
pub const POPUP_ACTIVATION_TIMEOUT: Duration = Duration::from_secs(1);
/// Double-click guard window for the tray-icon toggle (F9): a
/// second tray click this soon after an accepted toggle is
/// ignored, so a double-click is one show, not show-then-hide.
pub const TRAY_TOGGLE_DEBOUNCE: Duration = Duration::from_millis(400);

/// How long an OS theme sample is trusted (DS-22): the popup
/// refreshes at 10 Hz while visible, so an uncached `reg` spawn
/// per refresh would be 10 child processes a second. One sample
/// a minute is plenty — the main window follows live OS changes
/// (DS-50); the popup re-samples on the next minute tick.
const SYSTEM_LIGHT_TTL: Duration = Duration::from_secs(60);

/// Cached OS light-preference sample (`(sampled_at, is_light)`).
static SYSTEM_LIGHT_CACHE: std::sync::Mutex<Option<(std::time::Instant, bool)>> =
    std::sync::Mutex::new(None);

/// Whether the OS currently prefers a light theme — Windows
/// `AppsUseLightTheme` (mirrors `rsh-ui`'s `system_light`, which
/// lives in the UI binary and cannot be shared here).
/// Best-effort: any failure (no `reg`, no key) means Dark.
#[cfg(windows)]
fn system_light_uncached() -> bool {
    let Ok(out) = std::process::Command::new("reg")
        .args([
            "query",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
            "/v",
            "AppsUseLightTheme",
        ])
        .output()
    else {
        return false;
    };
    String::from_utf8_lossy(&out.stdout).contains("0x1")
}

/// Whether the OS currently prefers a light theme (DS-22):
/// non-Windows has no system light preference wired — Dark.
#[cfg(not(windows))]
fn system_light_uncached() -> bool {
    false
}

/// Cached OS light preference (DS-22): the 10 Hz popup refresh
/// calls [`popup_is_light`] every tick, and only the Auto case
/// needs the OS — so the sample is cached for
/// [`SYSTEM_LIGHT_TTL`] instead of spawning `reg` per refresh.
/// A poisoned/unavailable lock degrades to a fresh sample.
fn system_light() -> bool {
    let now = std::time::Instant::now();
    if let Ok(cache) = SYSTEM_LIGHT_CACHE.lock()
        && let Some((at, value)) = *cache
        && now.duration_since(at) < SYSTEM_LIGHT_TTL
    {
        return value;
    }
    let value = system_light_uncached();
    if let Ok(mut cache) = SYSTEM_LIGHT_CACHE.lock() {
        *cache = Some((now, value));
    }
    value
}

/// Whether the tray popup should render Light (DS-22, U-04):
/// reads the persisted UI theme choice from `<data_dir>/ui-theme`
/// — the same file the main window reads and writes (`rsh-ui`
/// `read_theme`/`write_theme`: `"Dark"`/`"Light"`/`"Auto"`).
/// "Light" → light, "Dark" → dark, "Auto" → the OS preference.
/// A missing / unreadable file (or no data dir) → Dark — the
/// fresh-install default, the same default `read_theme` uses
/// (R2-37): the preview only approves the dark theme
/// (REDESIGN-PLAN: "Light theme: abhi design nahi hui"), so a
/// fresh install must not render Light on a light OS while the
/// main window opens Dark. Pure over the given dir, so it is
/// unit-testable with a temp dir.
#[must_use]
pub fn popup_is_light(data_dir: Option<&std::path::Path>) -> bool {
    let Some(dir) = data_dir else {
        return false;
    };
    match std::fs::read_to_string(dir.join("ui-theme")) {
        Ok(text) if text.trim() == "Light" => true,
        Ok(text) if text.trim() == "Auto" => system_light(),
        _ => false,
    }
}

/// Whether the visible tray popup should auto-dismiss, decided by
/// the daemon's 100 ms click-outside poll in `main.rs` — kept here
/// as pure state so it is unit-testable on Linux.
///
/// Root cause of the v2.0.1 bug: the old poll dismissed the popup
/// whenever it was not the foreground window after the 500 ms
/// grace. But a window shown from a tray click is usually NEVER
/// the foreground window — Windows' foreground lock stops a
/// background process (the daemon, whose foreground owner at click
/// time is Explorer/the taskbar) from stealing the foreground, so
/// `SetForegroundWindow`-style activation of the popup fails and
/// the first poll after the grace hid the popup the user had just
/// opened (~0.5–1 s after the click — exactly the reported
/// symptom). "Not foreground" on its own therefore says nothing
/// about the user clicking away.
///
/// The fixed rule (A4, v2.0.2) dismisses only on a *change* the
/// user caused — and F9 (v2.0.3) hardens it against the transient
/// the A4 fix still missed: `foreground_at_show` is sampled
/// during the tray-click transition, and Windows settles back to
/// the pre-click app after the grace expires, so rule (d) fired at
/// ~0.6–1 s on a settle the user never caused. This state machine
/// therefore:
///
/// * never dismisses inside the grace;
/// * never dismisses while the popup IS foreground;
/// * re-samples the baseline at grace expiry and only arms it
///   once the foreground has been the same window for
///   [`POPUP_BASELINE_STABLE_POLLS`] consecutive polls (or at the
///   end of the settle window, taking the latest value) — a
///   transient settle (e.g. taskbar → other app → taskbar across
///   300–700 ms) never becomes the baseline, and a change that
///   lands during the settle window is absorbed as the settle,
///   not treated as a click-away;
/// * after arming, dismisses only on
///   [`POPUP_DISMISS_CHANGED_POLLS`] consecutive polls showing a
///   different foreground window (a real, sustained click-away);
/// * dismisses immediately when the popup loses a foreground it
///   held continuously for [`POPUP_FOREGROUND_SUSTAINED`] — but
///   never on a brief foreground blip (a transient activation
///   does not arm the loss rule, which was the `was_foreground`
///   trap: the old sticky flag fired on any foreground sighting,
///   however short);
/// * never dismisses when the foreground is unknown (`None`).
///
/// Known limit: when the popup was shown while the main UI window
/// was already foreground, clicking back into that same UI window
/// leaves the foreground unchanged, so the popup stays open
/// (Escape / tray-click / "Open RSH" still dismiss it). Detecting
/// that click would need a mouse hook, which is out of scope.
///
/// A second honest limit: a click-away that lands inside the
/// settle window (roughly the first second after showing) is
/// absorbed as settling and does NOT dismiss — the popup stays
/// open until the next click-away / Escape / tray-click. The
/// ~1 s false dismiss is the worse failure, so the poll errs
/// towards staying open.
#[derive(Clone, Debug, Default)]
pub struct PopupDismissPoll {
    /// Foreground window sampled at show time (diagnostics only —
    /// the armed baseline comes from the settle window, not this).
    foreground_at_show: Option<isize>,
    /// Baseline armed in the settle window; `None` until armed.
    baseline: Option<isize>,
    /// Foreground value currently being stability-tested for the
    /// baseline, and how many consecutive polls showed it.
    candidate: Option<isize>,
    /// Consecutive polls showing [`Self::candidate`].
    candidate_streak: u32,
    /// Consecutive polls (after arming) showing a foreground
    /// window different from [`Self::baseline`].
    changed_streak: u32,
    /// Elapsed time when the popup first became foreground in the
    /// current continuous run (`None` while it is not foreground).
    foreground_since: Option<Duration>,
    /// The popup held the foreground continuously for at least
    /// [`POPUP_FOREGROUND_SUSTAINED`] at some point since it was
    /// shown (sticky until the next show).
    saw_sustained_foreground: bool,
}

impl PopupDismissPoll {
    /// Start polling for a freshly shown popup.
    #[must_use]
    pub fn new(foreground_at_show: Option<isize>) -> Self {
        Self {
            foreground_at_show,
            ..Self::default()
        }
    }

    /// Foreground window sampled at show time (for the F10 log
    /// line — the armed baseline may differ after settling).
    #[must_use]
    pub fn foreground_at_show(&self) -> Option<isize> {
        self.foreground_at_show
    }

    /// Armed baseline, if the settle window has produced one.
    #[must_use]
    pub fn baseline(&self) -> Option<isize> {
        self.baseline
    }

    /// Whether the popup held the foreground continuously for at
    /// least [`POPUP_FOREGROUND_SUSTAINED`] since it was shown.
    #[must_use]
    pub fn was_foreground_sustained(&self) -> bool {
        self.saw_sustained_foreground
    }

    /// One 100 ms poll tick. `elapsed` is the time since the popup
    /// was shown; `foreground_now` is the tick's single
    /// `GetForegroundWindow()` read (`None` when Windows cannot
    /// tell); `popup_hwnd` is the popup's native window, if it
    /// exists yet. Returns `true` when the popup should hide.
    pub fn poll(
        &mut self,
        elapsed: Duration,
        foreground_now: Option<isize>,
        popup_hwnd: Option<isize>,
    ) -> bool {
        let popup_foreground = popup_hwnd.is_some_and(|hwnd| foreground_now == Some(hwnd));
        // Track continuous foreground first — grace samples count:
        // a popup that becomes foreground during the grace and
        // stays there is being used.
        if popup_foreground {
            if self.foreground_since.is_none() {
                self.foreground_since = Some(elapsed);
            } else if let Some(since) = self.foreground_since
                && elapsed.saturating_sub(since) >= POPUP_FOREGROUND_SUSTAINED
            {
                self.saw_sustained_foreground = true;
            }
        } else {
            self.foreground_since = None;
        }
        if elapsed <= POPUP_FOREGROUND_GRACE {
            return false;
        }
        if popup_foreground {
            return false;
        }
        // The popup held the foreground for a sustained interval
        // and just lost it to a known window: a real click-away.
        // A brief foreground blip never sets the flag, and an
        // unknown foreground never dismisses.
        if self.saw_sustained_foreground && foreground_now.is_some() {
            return true;
        }
        // Baseline arming: re-sampled in the settle window, only
        // once the foreground has been stable for
        // `POPUP_BASELINE_STABLE_POLLS` consecutive polls; at the
        // settle deadline the latest known value wins so the poll
        // can still work on a flapping foreground.
        let Some(baseline) = self.baseline else {
            match foreground_now {
                Some(now) => {
                    if self.candidate == Some(now) {
                        self.candidate_streak += 1;
                    } else {
                        self.candidate = Some(now);
                        self.candidate_streak = 1;
                    }
                    if self.candidate_streak >= POPUP_BASELINE_STABLE_POLLS
                        || elapsed > POPUP_FOREGROUND_GRACE + POPUP_BASELINE_SETTLE
                    {
                        self.baseline = Some(now);
                        self.changed_streak = 0;
                    }
                }
                None => {
                    if elapsed > POPUP_FOREGROUND_GRACE + POPUP_BASELINE_SETTLE {
                        self.baseline = self.foreground_at_show;
                    }
                }
            }
            return false;
        };
        // Armed: dismiss only on a sustained change — N
        // consecutive polls away from the baseline.
        match foreground_now {
            Some(now) if now != baseline => {
                self.changed_streak += 1;
                self.changed_streak >= POPUP_DISMISS_CHANGED_POLLS
            }
            _ => {
                self.changed_streak = 0;
                false
            }
        }
    }
}

/// Double-click guard for the tray-icon toggle (F9, v2.0.3): the
/// tray backend can deliver a double-click as two toggles, which
/// shows the popup and instantly hides it again. The guard keeps
/// the time of the last accepted toggle; a click inside
/// [`TRAY_TOGGLE_DEBOUNCE`] of it is ignored. Times are elapsed
/// durations on one monotonic clock (the caller passes
/// `started.elapsed()` from a fixed `Instant`); the guard itself
/// stays pure so the timeline is unit-testable.
#[derive(Clone, Debug, Default)]
pub struct TrayToggleGuard {
    last_accepted: Option<Duration>,
}

impl TrayToggleGuard {
    /// A fresh guard (no toggle accepted yet).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` when this click is a real toggle (and
    /// records it), `false` when it is inside the debounce window
    /// of the last accepted toggle.
    pub fn accept(&mut self, now: Duration) -> bool {
        match self.last_accepted {
            Some(last) if now.saturating_sub(last) < TRAY_TOGGLE_DEBOUNCE => false,
            _ => {
                self.last_accepted = Some(now);
                true
            }
        }
    }
}

/// Preview game order in the popup (AC, ACC, iRacing first).
const PREFERRED_GAME_IDS: [&str; 3] = ["assetto_corsa", "assetto_corsa_competizione", "iracing"];

/// A rectangle in physical screen coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    /// Left edge.
    pub left: i32,
    /// Top edge.
    pub top: i32,
    /// Right edge (exclusive).
    pub right: i32,
    /// Bottom edge (exclusive).
    pub bottom: i32,
}

/// Anchor the 340×440 popup next to the tray icon.
///
/// The taskbar is normally at the bottom: the popup sits *above* the
/// icon, right-aligned to it. When the tray is at the top (taskbar on
/// top) the popup sits *below* the icon instead. The result is
/// always clamped inside `work_area`, so the popup can never end up
/// half off-screen on a small / scaled display. With no tray rect
/// (`None` — Linux, or `Shell_NotifyIconGetRect` failed) the popup
/// docks to the bottom-right corner of the work area, which is where
/// the tray lives on a default Windows desktop.
#[must_use]
pub fn anchor_popup(tray: Option<Rect>, work_area: Rect) -> (i32, i32) {
    let (mut x, mut y) = match tray {
        Some(icon) => {
            let x = icon.right - POPUP_WIDTH;
            // Icon in the bottom half of the screen → taskbar at the
            // bottom → popup above the icon; otherwise below it.
            let icon_centre_y = i32::midpoint(icon.top, icon.bottom);
            let work_centre_y = i32::midpoint(work_area.top, work_area.bottom);
            let y = if icon_centre_y >= work_centre_y {
                icon.top - POPUP_GAP - POPUP_HEIGHT
            } else {
                icon.bottom + POPUP_GAP
            };
            (x, y)
        }
        None => (
            work_area.right - POPUP_WIDTH,
            work_area.bottom - POPUP_HEIGHT,
        ),
    };
    // Clamp inside the work area (a narrow work area can be smaller
    // than the popup — then the top-left corner wins).
    x = x.clamp(
        work_area.left,
        (work_area.right - POPUP_WIDTH).max(work_area.left),
    );
    y = y.clamp(
        work_area.top,
        (work_area.bottom - POPUP_HEIGHT).max(work_area.top),
    );
    (x, y)
}

/// One game row in the popup (max 3, preview order: Assetto Corsa,
/// AC Competizione, iRacing first when present).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PopupGame {
    /// Game id (what `launch_game` takes).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Short initials for the tile (`AC`, `ACC`, `iR`).
    pub initials: String,
    /// Right-hand state text: `Running`, `Launch`, `Not found`.
    pub state_text: String,
    /// Whether the Launch affordance is shown.
    pub can_launch: bool,
}

/// The popup's live content, refreshed from the daemon API.
#[derive(Clone, Debug, PartialEq)]
pub struct TrayPopupData {
    /// Connection label (`Connected`, `Connecting…`, …).
    pub conn_label: String,
    /// Sub-line (`Wi-Fi · 192.168.4.1`).
    pub conn_sub: String,
    /// Whether the wheel is connected.
    pub connected: bool,
    /// "Send to game" switch state.
    pub send_to_game: bool,
    /// Steering angle, degrees.
    pub steering_deg: f64,
    /// Throttle 0–100.
    pub throttle_pct: f64,
    /// Brake 0–100.
    pub brake_pct: f64,
    /// Clutch 0–100.
    pub clutch_pct: f64,
    /// Pre-formatted bar labels (`"72%"` — the `.slint` side never
    /// formats floats).
    pub throttle_text: String,
    /// Pre-formatted brake label.
    pub brake_text: String,
    /// Pre-formatted clutch label.
    pub clutch_text: String,
    /// Active profile name.
    pub profile_name: String,
    /// Profile sub-line (`Auto · Assetto Corsa` / game slug).
    pub profile_sub: String,
    /// Game rows (at most 3).
    pub games: Vec<PopupGame>,
}

impl TrayPopupData {
    /// Build from the live API + the daemon's profile/game lists.
    /// Percentages are clamped to 0–100 (a corrupt snapshot must
    /// never draw a bar past its track).
    #[must_use]
    pub fn collect(api: &dyn AppApi, profiles: &[ProfileSummary], games: &[GameInfo]) -> Self {
        let conn = api.connection();
        let snap = api.snapshot();
        let active = profiles.iter().find(|profile| profile.active);
        let (profile_name, profile_sub) = match active {
            Some(profile) => (
                profile.name.clone(),
                match &profile.game {
                    Some(game) => format!("Auto · {game}"),
                    None => "Auto".to_string(),
                },
            ),
            None => ("Default".to_string(), "Auto".to_string()),
        };
        let transport_label = match conn.transport {
            rsh_api::ApiTransport::Wifi => "Wi-Fi",
            rsh_api::ApiTransport::Usb => "USB",
        };
        let conn_sub = if conn.address.is_empty() {
            transport_label.to_string()
        } else {
            format!("{transport_label} · {}", conn.address)
        };
        // Preview order first (AC, ACC, iRacing), then any others,
        // capped at 3 rows.
        let mut ordered: Vec<&GameInfo> = Vec::new();
        for wanted in PREFERRED_GAME_IDS {
            if let Some(game) = games.iter().find(|game| game.id == wanted) {
                ordered.push(game);
            }
        }
        for game in games {
            if ordered.len() >= 3 {
                break;
            }
            if !ordered.iter().any(|g| g.id == game.id) {
                ordered.push(game);
            }
        }
        let popup_games = ordered
            .into_iter()
            .map(|game| {
                let initials = game
                    .name
                    .split_whitespace()
                    .filter_map(|word| word.chars().next())
                    .take(2)
                    .collect::<String>();
                let (state_text, can_launch) = if game.running {
                    ("Running".to_string(), false)
                } else if game.installed {
                    ("Launch".to_string(), true)
                } else {
                    ("Not found".to_string(), false)
                };
                PopupGame {
                    id: game.id.clone(),
                    name: game.name.clone(),
                    initials,
                    state_text,
                    can_launch,
                }
            })
            .collect();
        let throttle_pct = (snap.throttle * 100.0).clamp(0.0, 100.0);
        let brake_pct = (snap.brake * 100.0).clamp(0.0, 100.0);
        let clutch_pct = (snap.clutch.unwrap_or(0.0) * 100.0).clamp(0.0, 100.0);
        Self {
            conn_label: conn.label.clone(),
            conn_sub,
            connected: conn.label == "Connected",
            send_to_game: api.output_enabled(),
            steering_deg: snap.steering_deg,
            throttle_pct,
            brake_pct,
            clutch_pct,
            throttle_text: format!("{throttle_pct:.0}%"),
            brake_text: format!("{brake_pct:.0}%"),
            clutch_text: format!("{clutch_pct:.0}%"),
            profile_name,
            profile_sub,
            games: popup_games,
        }
    }
}

/// Next profile in the list after `current_id` (wraps around), for
/// the popup's profile row: one click cycles to the next profile.
/// Returns `None` when there is nothing to cycle to.
#[must_use]
pub fn next_profile<'a>(
    profiles: &'a [ProfileSummary],
    current_id: Option<&str>,
) -> Option<&'a ProfileSummary> {
    if profiles.len() < 2 {
        return None;
    }
    let current = current_id
        .and_then(|id| profiles.iter().position(|profile| profile.id == id))
        .unwrap_or(usize::MAX);
    let next = if current == usize::MAX {
        0
    } else {
        (current + 1) % profiles.len()
    };
    profiles.get(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsh_api::errors::UserError;
    use rsh_api::{
        ApiTransport, AppApi, ConnectionInfo, ConnectionState, PairingInfo, SnapshotInfo,
    };

    struct FakeApi {
        label: &'static str,
        snap: SnapshotInfo,
        send: bool,
    }

    impl AppApi for FakeApi {
        fn version(&self) -> String {
            "2.0.0".to_string()
        }

        fn connection(&self) -> ConnectionInfo {
            ConnectionInfo {
                state: ConnectionState::Active,
                label: self.label.to_string(),
                transport: ApiTransport::Wifi,
                address: "192.168.4.1".to_string(),
                session_active: true,
                error: None,
                hint: None,
            }
        }

        fn connect(&self, _transport: ApiTransport) -> Result<ConnectionInfo, UserError> {
            Ok(self.connection())
        }

        fn disconnect(&self) -> Result<ConnectionInfo, UserError> {
            Ok(self.connection())
        }

        fn pairing_status(&self) -> PairingInfo {
            PairingInfo {
                seconds_left: 0,
                password: None,
                error: None,
                error_detail: None,
                board_mac: None,
                replacement_required: false,
                key_reset_armed: false,
                attempt_log: Vec::new(),
            }
        }

        fn begin_pairing(&self) -> Result<PairingInfo, UserError> {
            Ok(self.pairing_status())
        }

        fn snapshot(&self) -> SnapshotInfo {
            self.snap.clone()
        }

        fn output_enabled(&self) -> bool {
            self.send
        }

        fn set_output_enabled(&self, _enabled: bool) {}

        fn get_tuning(&self, module: &str) -> rsh_api::TuningSettings {
            rsh_api::TuningSettings::neutral(module)
        }

        fn set_tuning(
            &self,
            tuning: &rsh_api::TuningSettings,
        ) -> Result<rsh_api::TuningSettings, UserError> {
            Ok(tuning.clone())
        }

        fn draft_status(&self) -> Option<rsh_api::DraftStatus> {
            None
        }

        fn save_draft(&self) -> Result<bool, UserError> {
            Ok(false)
        }

        fn discard_draft(&self) -> bool {
            false
        }
    }

    fn work_area() -> Rect {
        Rect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        }
    }

    fn bottom_tray() -> Rect {
        Rect {
            left: 1850,
            top: 1000,
            right: 1890,
            bottom: 1040,
        }
    }

    #[test]
    fn popup_anchors_above_bottom_tray_right_aligned() {
        let (x, y) = anchor_popup(Some(bottom_tray()), work_area());
        assert_eq!(x, 1890 - POPUP_WIDTH);
        assert_eq!(y, 1000 - POPUP_GAP - POPUP_HEIGHT);
    }

    #[test]
    fn popup_anchors_below_top_tray() {
        let top = Rect {
            left: 1850,
            top: 0,
            right: 1890,
            bottom: 40,
        };
        let (x, y) = anchor_popup(Some(top), work_area());
        assert_eq!(x, 1890 - POPUP_WIDTH);
        assert_eq!(y, 40 + POPUP_GAP);
    }

    #[test]
    fn popup_without_tray_rect_docks_bottom_right() {
        let (x, y) = anchor_popup(None, work_area());
        assert_eq!(x, 1920 - POPUP_WIDTH);
        assert_eq!(y, 1040 - POPUP_HEIGHT);
    }

    #[test]
    fn popup_is_clamped_inside_small_work_area() {
        let small = Rect {
            left: 0,
            top: 0,
            right: 800,
            bottom: 600,
        };
        let (x, y) = anchor_popup(Some(bottom_tray()), small);
        assert!(x >= 0 && x + POPUP_WIDTH <= 800);
        assert!(y >= 0 && y + POPUP_HEIGHT <= 600);
    }

    fn profile(id: &str, name: &str, active: bool) -> ProfileSummary {
        ProfileSummary {
            id: id.to_string(),
            name: name.to_string(),
            game: Some("assetto_corsa".to_string()),
            active,
        }
    }

    fn game(id: &str, name: &str, installed: bool, running: bool) -> GameInfo {
        GameInfo {
            id: id.to_string(),
            name: name.to_string(),
            bound_profile_id: None,
            running,
            installed,
        }
    }

    #[test]
    fn collect_maps_live_data_and_clamps_bars() {
        let api = FakeApi {
            label: "Connected",
            snap: SnapshotInfo {
                steering_deg: -18.0,
                throttle: 1.4,
                brake: 0.0,
                clutch: Some(-0.2),
                button_mask: 0,
                handbrake: 0.0,
                gear: 0,
                seq: 0,
            },
            send: true,
        };
        let data = TrayPopupData::collect(
            &api,
            &[profile("p1", "GT3 Sprint", true)],
            &[
                game("assetto_corsa", "Assetto Corsa", true, false),
                game("iracing", "iRacing", false, false),
            ],
        );
        assert!(data.connected);
        assert_eq!(data.conn_sub, "Wi-Fi · 192.168.4.1");
        assert_eq!(data.steering_deg, -18.0);
        assert_eq!(data.throttle_pct, 100.0);
        assert_eq!(data.clutch_pct, 0.0);
        assert_eq!(data.profile_name, "GT3 Sprint");
        assert_eq!(data.games.len(), 2);
        assert_eq!(data.games[0].state_text, "Launch");
        assert!(data.games[0].can_launch);
        assert_eq!(data.games[1].state_text, "Not found");
    }

    #[test]
    fn collect_prefers_preview_game_order_and_caps_at_three() {
        let api = FakeApi {
            label: "Disconnected",
            snap: SnapshotInfo::default(),
            send: false,
        };
        let data = TrayPopupData::collect(
            &api,
            &[],
            &[
                game("dirt_rally_2", "Dirt Rally 2.0", true, false),
                game("iracing", "iRacing", true, true),
                game("assetto_corsa", "Assetto Corsa", true, false),
                game("assetto_corsa_competizione", "AC Competizione", true, false),
            ],
        );
        assert_eq!(data.games.len(), 3);
        assert_eq!(data.games[0].id, "assetto_corsa");
        assert_eq!(data.games[1].id, "assetto_corsa_competizione");
        assert_eq!(data.games[2].id, "iracing");
        assert_eq!(data.games[2].state_text, "Running");
        assert_eq!(data.profile_name, "Default");
    }

    #[test]
    fn next_profile_cycles_and_wraps() {
        let profiles = vec![
            profile("p1", "A", true),
            profile("p2", "B", false),
            profile("p3", "C", false),
        ];
        assert_eq!(next_profile(&profiles, Some("p1")).unwrap().id, "p2");
        assert_eq!(next_profile(&profiles, Some("p3")).unwrap().id, "p1");
        assert_eq!(next_profile(&profiles, None).unwrap().id, "p1");
        assert!(next_profile(&profiles[..1], Some("p1")).is_none());
    }

    // A4 (v2.0.2) + F9 (v2.0.3): the ~1 s auto-dismiss. The
    // foreground window at show time is the taskbar (Explorer,
    // fake hwnd 100 here); the popup itself is hwnd 200 when it
    // ever becomes foreground; another app is 300.
    //
    // Timelines are sequences of (elapsed_ms, foreground_now)
    // samples at the daemon's 100 ms tick. `run_timeline` feeds
    // them to a fresh `PopupDismissPoll` and returns the elapsed
    // time of the first dismiss, or `None` when the popup stays
    // open through the whole timeline.
    fn run_timeline(
        samples: &[(u64, Option<isize>)],
        popup_hwnd: Option<isize>,
    ) -> Option<Duration> {
        let mut poll = PopupDismissPoll::new(Some(100));
        for &(t_ms, foreground_now) in samples {
            let elapsed = Duration::from_millis(t_ms);
            if poll.poll(elapsed, foreground_now, popup_hwnd) {
                return Some(elapsed);
            }
        }
        None
    }

    /// One sample every 100 ms from 0 to `end_ms`, all with the
    /// same foreground window, except the overrides.
    fn steady(end_ms: u64, fg: isize, overrides: &[(u64, isize)]) -> Vec<(u64, Option<isize>)> {
        (0..=end_ms)
            .step_by(100)
            .map(|t| {
                let value = overrides
                    .iter()
                    .find(|(at, _)| *at == t)
                    .map_or(fg, |(_, value)| *value);
                (t, Some(value))
            })
            .collect()
    }

    fn after_grace() -> Duration {
        POPUP_FOREGROUND_GRACE + Duration::from_millis(100)
    }

    #[test]
    fn dismiss_never_inside_the_grace() {
        // Foreground already moved (300) — still inside the grace,
        // so no dismiss.
        let samples = steady(500, 300, &[]);
        assert_eq!(run_timeline(&samples, Some(200)), None);
        assert!(after_grace() > POPUP_FOREGROUND_GRACE);
    }

    #[test]
    fn dismiss_never_when_popup_never_foreground_and_user_did_nothing() {
        // THE v2.0.1 BUG: the old poll hid the popup here (not
        // foreground after the grace), even though the foreground
        // never moved — the user just opened it from the tray.
        let samples = steady(2000, 100, &[]);
        assert_eq!(run_timeline(&samples, Some(200)), None);
    }

    #[test]
    fn dismiss_when_foreground_moves_after_baseline_arms() {
        // Baseline arms on the settled taskbar (100) at ~800 ms;
        // the user then clicks another window (300) and stays
        // there: 3 consecutive changed polls dismiss at 1200 ms.
        let samples = steady(
            2000,
            100,
            &[(1000, 300), (1100, 300), (1200, 300), (1300, 300)],
        );
        assert_eq!(
            run_timeline(&samples, Some(200)),
            Some(Duration::from_millis(1200))
        );
    }

    #[test]
    fn dismiss_when_popup_was_foreground_sustained_and_lost_it() {
        // Popup foreground from 600 ms, user clicks away at 1600 ms
        // (foreground 300): sustained foreground (600–1500 ms)
        // makes the loss a real click-away — dismiss at once.
        let mut samples = steady(2000, 100, &[]);
        for sample in &mut samples {
            if (600..=1500).contains(&sample.0) {
                sample.1 = Some(200);
            } else if sample.0 >= 1600 {
                sample.1 = Some(300);
            }
        }
        assert_eq!(
            run_timeline(&samples, Some(200)),
            Some(Duration::from_millis(1600))
        );
    }

    #[test]
    fn dismiss_never_when_popup_is_foreground() {
        let samples = steady(2000, 200, &[]);
        assert_eq!(run_timeline(&samples, Some(200)), None);
    }

    #[test]
    fn dismiss_never_when_foreground_is_unknown() {
        // Cannot tell what the user did — stay open, never guess.
        let mut samples = steady(2000, 100, &[]);
        for sample in &mut samples {
            if sample.0 >= 600 {
                sample.1 = None;
            }
        }
        assert_eq!(run_timeline(&samples, Some(200)), None);
    }

    // F9 (v2.0.3): the transient-baseline hole the A4 fix missed.
    // Windows settles back to the pre-click app after the grace
    // expires; the settle must not look like a click-away.

    #[test]
    fn transient_baseline_switch_stays_open() {
        // Foreground: taskbar (100) → other app (300) at 300 ms →
        // back to taskbar (100) by 700 ms. The 300 ms value never
        // stabilises into a baseline (the 100 ms value does, at
        // ~900 ms), so the popup stays open.
        let samples = steady(2000, 100, &[(300, 300), (400, 300), (500, 300)]);
        assert_eq!(run_timeline(&samples, Some(200)), None);
    }

    #[test]
    fn transient_baseline_switch_back_by_700ms_stays_open() {
        // F17 matrix: the transient lasts longer — other app (300)
        // from 300 ms until 600 ms, back to the taskbar (100) at
        // 700 ms. The 300 ms value still never stabilises into a
        // baseline (3 stable polls of 100 arm it at ~900 ms), so
        // the popup stays open through 2 s.
        let samples = steady(2000, 100, &[(300, 300), (400, 300), (500, 300), (600, 300)]);
        assert_eq!(run_timeline(&samples, Some(200)), None);
    }

    #[test]
    fn transient_activation_200_to_250ms_stays_open() {
        // F17 matrix: the popup is briefly foreground at 200 ms
        // and still foreground at 250 ms (a transient activation,
        // e.g. the tray click focusing it for a moment), then the
        // taskbar is foreground again. 250 ms of foreground is far
        // below the 500 ms sustained threshold, so the loss rule
        // never arms and the popup stays open.
        let samples: Vec<(u64, Option<isize>)> = (0..=2000)
            .step_by(100)
            .map(|t| (t, Some(if t == 200 { 200 } else { 100 })))
            .collect();
        let mut samples = samples;
        samples.push((250, Some(200)));
        samples.sort_by_key(|(t, _)| *t);
        assert_eq!(run_timeline(&samples, Some(200)), None);
    }

    #[test]
    fn sustained_foreground_loss_at_1500ms_dismisses_immediately() {
        // F17 matrix: the popup is foreground from 900 ms and
        // holds it continuously — by 1400 ms it has held it for
        // the 500 ms sustained threshold. The user clicks away at
        // exactly 1500 ms (foreground 300): a real click-away, so
        // the popup dismisses at once, at 1500 ms.
        let mut samples = steady(2000, 100, &[]);
        for sample in &mut samples {
            if (900..=1400).contains(&sample.0) {
                sample.1 = Some(200);
            } else if sample.0 >= 1500 {
                sample.1 = Some(300);
            }
        }
        assert_eq!(
            run_timeline(&samples, Some(200)),
            Some(Duration::from_millis(1500))
        );
    }

    #[test]
    fn transient_activation_blip_does_not_arm_foreground_loss() {
        // Popup briefly foreground at the 200 ms sample (a
        // transient activation), never again: the old sticky
        // `was_foreground` flag would have armed the loss rule on
        // this blip — the sustained rule must not.
        let samples = steady(2000, 100, &[(200, 200)]);
        assert_eq!(run_timeline(&samples, Some(200)), None);
    }

    #[test]
    fn single_changed_poll_never_dismisses() {
        // One fluky read (300) after arming, then back to the
        // baseline: the changed streak resets, popup stays open.
        let samples = steady(2000, 100, &[(1000, 300)]);
        assert_eq!(run_timeline(&samples, Some(200)), None);
    }

    #[test]
    fn two_changed_polls_never_dismiss_but_three_do() {
        // Two changed polls: still open. A third consecutive one
        // dismisses.
        let two = steady(2000, 100, &[(1000, 300), (1100, 300)]);
        assert_eq!(run_timeline(&two, Some(200)), None);
        let three = steady(2000, 100, &[(1000, 300), (1100, 300), (1200, 300)]);
        assert_eq!(
            run_timeline(&three, Some(200)),
            Some(Duration::from_millis(1200))
        );
    }

    #[test]
    fn click_away_inside_settle_window_is_absorbed() {
        // Honest limit, documented on `PopupDismissPoll`: a real
        // click-away at 700 ms — before the baseline arms — is
        // absorbed as settling (the 300 ms value becomes the
        // baseline) and the popup stays open. The next click-away
        // after arming still dismisses.
        let samples: Vec<(u64, Option<isize>)> = (0..=2000)
            .step_by(100)
            .map(|t| (t, Some(if t >= 700 { 300 } else { 100 })))
            .collect();
        assert_eq!(run_timeline(&samples, Some(200)), None);
    }

    #[test]
    fn baseline_arms_on_latest_value_at_settle_deadline() {
        // Foreground flaps every poll through the settle window,
        // so no value stabilises: at the deadline the latest value
        // (300 at 1100 ms) arms as the baseline, and a later
        // sustained move away from it dismisses.
        let mut samples = Vec::new();
        for t in (0..=2000).step_by(100) {
            let fg = if (600..=1000).contains(&t) {
                if (t / 100) % 2 == 0 { 300 } else { 100 }
            } else if t >= 1500 {
                400
            } else if t >= 1100 {
                300
            } else {
                100
            };
            samples.push((t, Some(fg)));
        }
        // 1500/1600/1700 ms all show 400 ≠ baseline 300.
        assert_eq!(
            run_timeline(&samples, Some(200)),
            Some(Duration::from_millis(1700))
        );
    }

    #[test]
    fn toggle_guard_accepts_first_click_and_ignores_double_click() {
        let mut guard = TrayToggleGuard::new();
        let ms = Duration::from_millis;
        assert!(guard.accept(ms(0))); // first click: show
        assert!(!guard.accept(ms(200))); // double-click: ignored
        assert!(!guard.accept(ms(399))); // still inside 400 ms
        assert!(guard.accept(ms(400))); // debounce expired: hide
        assert!(!guard.accept(ms(600))); // double-click again
        assert!(guard.accept(ms(1000))); // next real toggle
    }

    /// Temp dir for `popup_is_light` tests (no `tempfile` dev-dep
    /// in this crate): unique per test name + pid, removed after.
    fn theme_test_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("rsh-popup-theme-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("theme test dir");
        dir
    }

    #[test]
    fn popup_is_light_follows_persisted_theme_file() {
        // DS-22: the daemon reads the same `<data_dir>/ui-theme`
        // file the main window writes ("Dark"/"Light"/"Auto").
        let dir = theme_test_dir("file");
        std::fs::write(dir.join("ui-theme"), "Light\n").expect("write theme");
        assert!(popup_is_light(Some(&dir)), "Light must render light");
        std::fs::write(dir.join("ui-theme"), "Dark\n").expect("write theme");
        assert!(!popup_is_light(Some(&dir)), "Dark must render dark");
        // R2-37: "Auto" still follows the OS preference, but a
        // missing file, garbage, and no data dir at all are Dark —
        // the fresh-install default, matching `read_theme`.
        // (The OS value itself is runner-dependent, so the Auto
        // assertion compares against the sampler, not a theme.)
        std::fs::write(dir.join("ui-theme"), "Auto\n").expect("write theme");
        assert_eq!(popup_is_light(Some(&dir)), system_light());
        let _ = std::fs::remove_file(dir.join("ui-theme"));
        assert!(!popup_is_light(Some(&dir)), "missing file must render dark");
        std::fs::write(dir.join("ui-theme"), "garbage\n").expect("write theme");
        assert!(!popup_is_light(Some(&dir)), "garbage must render dark");
        assert!(!popup_is_light(None), "no data dir must render dark");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
