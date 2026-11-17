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

/// Whether the visible tray popup should auto-dismiss on this
/// foreground poll (A4, v2.0.2) — the pure decision behind the
/// daemon's 100 ms click-outside poll in `main.rs`, kept here so
/// it is unit-testable on Linux.
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
/// The fixed rule dismisses only on a *change* the user caused:
/// * inside the grace — never (showing cannot instantly close);
/// * the popup IS foreground — never (the user is in it; the
///   caller records this in `was_foreground`);
/// * the popup WAS foreground and lost it — yes (clicked away
///   after interacting with it);
/// * the popup was never foreground, but the foreground window
///   moved away from what it was when the popup was shown
///   (`foreground_now != foreground_at_show`, both known) — yes
///   (clicked another window / the desktop while the popup sat
///   inactive);
/// * otherwise — no: the foreground is still whatever it was at
///   show time (normally the taskbar), i.e. the user did nothing,
///   so the popup stays until they dismiss it, click away, or
///   press Escape.
///
/// Known limit: when the popup was shown while the main UI window
/// was already foreground, clicking back into that same UI window
/// leaves the foreground unchanged, so the popup stays open
/// (Escape / tray-click / "Open RSH" still dismiss it). Detecting
/// that click would need a mouse hook, which is out of scope.
#[must_use]
pub fn should_dismiss_popup(
    elapsed: Duration,
    popup_foreground: bool,
    was_foreground: bool,
    foreground_now: Option<isize>,
    foreground_at_show: Option<isize>,
) -> bool {
    if elapsed <= POPUP_FOREGROUND_GRACE {
        return false;
    }
    if popup_foreground {
        return false;
    }
    if was_foreground {
        return true;
    }
    matches!((foreground_now, foreground_at_show), (Some(now), Some(at_show)) if now != at_show)
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

    // A4 (v2.0.2): the ~1 s auto-dismiss. The foreground window at
    // show time is the taskbar (Explorer, fake hwnd 100 here); the
    // popup itself is hwnd 200 when it ever becomes foreground.
    fn after_grace() -> Duration {
        POPUP_FOREGROUND_GRACE + Duration::from_millis(100)
    }

    #[test]
    fn dismiss_never_inside_the_grace() {
        assert!(!should_dismiss_popup(
            POPUP_FOREGROUND_GRACE,
            false,
            false,
            Some(300),
            Some(100)
        ));
    }

    #[test]
    fn dismiss_never_when_popup_never_foreground_and_user_did_nothing() {
        // THE v2.0.1 BUG: the old poll hid the popup here (not
        // foreground after the grace), even though the foreground
        // never moved — the user just opened it from the tray.
        assert!(!should_dismiss_popup(
            after_grace(),
            false,
            false,
            Some(100),
            Some(100)
        ));
    }

    #[test]
    fn dismiss_when_foreground_moved_while_popup_never_foreground() {
        // User clicked another window (300) while the popup sat
        // inactive: that IS a click-away.
        assert!(should_dismiss_popup(
            after_grace(),
            false,
            false,
            Some(300),
            Some(100)
        ));
    }

    #[test]
    fn dismiss_when_popup_was_foreground_and_lost_it() {
        assert!(should_dismiss_popup(
            after_grace(),
            false,
            true,
            Some(100),
            Some(100)
        ));
    }

    #[test]
    fn dismiss_never_when_popup_is_foreground() {
        assert!(!should_dismiss_popup(
            after_grace(),
            true,
            false,
            Some(200),
            Some(100)
        ));
    }

    #[test]
    fn dismiss_never_when_foreground_is_unknown() {
        // Cannot tell what the user did — stay open, never guess.
        assert!(!should_dismiss_popup(
            after_grace(),
            false,
            false,
            None,
            Some(100)
        ));
        assert!(!should_dismiss_popup(
            after_grace(),
            false,
            false,
            Some(300),
            None
        ));
    }
}
