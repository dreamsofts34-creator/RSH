//! Notification center (RSH v2.0 — Phase 6 bell / drawer).
//!
//! [`NotificationCenter`] is the UI-side store for
//! [`rsh_api::AppEvent::Notification`] events pushed by the daemon.
//! The frame timer in `main.rs` feeds it from the event drain (and
//! still logs each event via `log_event`); the Slint drawer renders
//! it through the `notifications` model property on `MainWindow`.
//!
//! Ordering is newest-first, capped at [`MAX_NOTIFICATIONS`] items
//! (oldest dropped). Opening the drawer does NOT mark items read —
//! that is an explicit user action (`Mark all read`).
//!
//! Dedupe (F12, v2.0.3): [`NotificationCenter::push`] is NOT
//! unconditional — a repeat with the same `(kind, title, body)` key
//! inside the cooldown window is suppressed: no new row is stored,
//! the existing item's `suppressed` count grows, and the drawer
//! shows it as `"Title (×N)"`. Cooldown is 60 s for connection /
//! pairing kinds (`Connection`, `ConnectionLost`, `PairingNeeded` —
//! the daemon death→return flap pairs) and 5 s for everything else
//! (user-action errors). The suppressed count survives a save/load
//! round-trip with the item.

use std::collections::VecDeque;
use std::path::Path;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use rsh_api::AppEvent;
use serde::{Deserialize, Serialize};
use slint::{ModelRc, VecModel};

use crate::{MainWindow, NotificationItem};

/// Maximum notifications kept; older ones are dropped.
pub const MAX_NOTIFICATIONS: usize = 50;

/// File the center persists to (`<data_dir>/notifications.json`,
/// M10): notifications survive a restart instead of living only in
/// memory.
pub const NOTIFICATIONS_FILE: &str = "notifications.json";

/// One stored notification.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notification {
    /// Stable kind (`ConnectionLost`, `PairingNeeded`, …).
    pub kind: String,
    /// Short title.
    pub title: String,
    /// One-line body.
    pub body: String,
    /// Receive time as `HH:MM:SS` (UTC).
    pub time: String,
    /// True until the user marks all read.
    pub unread: bool,
    /// Repeats suppressed by dedupe (F12): how many pushes with
    /// this exact `(kind, title, body)` key arrived inside the
    /// cooldown window and were folded into this item instead of
    /// becoming new rows. Shown as `"Title (×N)"` in the drawer.
    /// `0` means this item is a single, unsuppressed push.
    #[serde(default)]
    pub suppressed: u32,
    /// Unix seconds of the last push folded into this item (the
    /// first push, or the latest suppressed repeat). Drives the
    /// cooldown comparison; not shown in the drawer. `0` on items
    /// loaded from a pre-F12 file (no timestamp recorded).
    #[serde(default)]
    pub last_seen_secs: u64,
}

/// Current Unix time in seconds.
fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Current time as `HH:MM:SS` (UTC, from the Unix clock — no extra
/// date/time dependency for one label).
fn now_time_string() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (h, m, s) = ((secs / 3600) % 24, (secs / 60) % 60, secs % 60);
    format!("{h:02}:{m:02}:{s:02}")
}

/// UI-side notification store (newest first).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NotificationCenter {
    items: VecDeque<Notification>,
}

impl NotificationCenter {
    /// Empty center.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Load the persisted center from `<data_dir>/notifications.json`
    /// (M10): a missing or unreadable file is an empty center, never
    /// an error — notifications are a log, not data the app depends
    /// on. Capped at [`MAX_NOTIFICATIONS`] on load.
    #[must_use]
    pub fn load(data_dir: Option<&Path>) -> Self {
        let Some(dir) = data_dir else {
            return Self::new();
        };
        let Ok(text) = std::fs::read_to_string(dir.join(NOTIFICATIONS_FILE)) else {
            return Self::new();
        };
        let Ok(mut center) = serde_json::from_str::<Self>(&text) else {
            return Self::new();
        };
        while center.items.len() > MAX_NOTIFICATIONS {
            center.items.pop_back();
        }
        center
    }

    /// Persist the center to `<data_dir>/notifications.json` (M10):
    /// best-effort — a write failure is silently ignored (the drawer
    /// still works in memory; the next save retries). Callers save
    /// after every mutation (push / mark-all-read / clear) — via
    /// `Ctx::persist_notifications` in `main.rs`, which coalesces
    /// bursts to at most one disk write per second (perf F-7); this
    /// method itself still writes immediately (tests + the flush
    /// path use it directly).
    pub fn save(&self, data_dir: Option<&Path>) {
        let Some(dir) = data_dir else {
            return;
        };
        if std::fs::create_dir_all(dir).is_err() {
            return;
        }
        if let Ok(text) = serde_json::to_string(&self) {
            let _ = std::fs::write(dir.join(NOTIFICATIONS_FILE), text);
        }
    }

    /// Cooldown (seconds) for a kind (F12): connection / pairing
    /// notifications repeat on every daemon death→return flap, so
    /// they get the long window; user-action errors are rarer and
    /// more specific, so they get the short one.
    fn cooldown_secs_for(kind: &str) -> u64 {
        match kind {
            "PairingNeeded" | "Connection" | "ConnectionLost" => 60,
            _ => 5,
        }
    }

    /// Parse an `HH:MM:SS` time label back to seconds since
    /// midnight (the fake clock for [`NotificationCenter::push_with_time`]
    /// tests; `0` when the label does not parse).
    #[cfg(test)]
    fn time_string_to_secs(time: &str) -> u64 {
        let mut parts = time.split(':');
        let parse = |part: Option<&str>| part.and_then(|p| p.parse::<u64>().ok());
        match (
            parse(parts.next()),
            parse(parts.next()),
            parse(parts.next()),
        ) {
            (Some(h), Some(m), Some(s)) if h < 24 && m < 60 && s < 60 => h * 3600 + m * 60 + s,
            _ => 0,
        }
    }

    /// Push a notification (newest first, capped at
    /// [`MAX_NOTIFICATIONS`]). Returns the stored item — or, when
    /// the push was a repeat inside the cooldown window, the
    /// existing item with its `suppressed` count bumped (F12: no
    /// new row, the drawer shows `"Title (×N)"` instead).
    pub fn push(&mut self, kind: &str, title: &str, body: &str) -> Notification {
        self.push_at(kind, title, body, now_unix_secs(), now_time_string())
    }

    /// Push with an explicit clock (F12 seam): `now_secs` is the
    /// dedupe clock, `time` is the display label. Production code
    /// uses [`NotificationCenter::push`]; tests drive this (via
    /// [`NotificationCenter::push_with_time`]) with a fake clock.
    fn push_at(
        &mut self,
        kind: &str,
        title: &str,
        body: &str,
        now_secs: u64,
        time: String,
    ) -> Notification {
        let cooldown = Self::cooldown_secs_for(kind);
        let existing = self
            .items
            .iter_mut()
            .find(|n| n.kind == kind && n.title == title && n.body == body);
        if let Some(item) = existing {
            if now_secs.saturating_sub(item.last_seen_secs) < cooldown {
                item.suppressed = item.suppressed.saturating_add(1);
                item.last_seen_secs = now_secs;
                return item.clone();
            }
        }
        let item = Notification {
            kind: kind.to_string(),
            title: title.to_string(),
            body: body.to_string(),
            time,
            unread: true,
            suppressed: 0,
            last_seen_secs: now_secs,
        };
        self.push_item(item.clone());
        item
    }

    /// Push with an explicit time string (tests only; production
    /// code uses [`NotificationCenter::push`]). The time string
    /// doubles as the fake dedupe clock (parsed as `HH:MM:SS`).
    #[cfg(test)]
    pub fn push_with_time(
        &mut self,
        kind: &str,
        title: &str,
        body: &str,
        time: &str,
    ) -> Notification {
        let now_secs = Self::time_string_to_secs(time);
        self.push_at(kind, title, body, now_secs, time.to_string())
    }

    fn push_item(&mut self, item: Notification) {
        self.items.push_front(item);
        while self.items.len() > MAX_NOTIFICATIONS {
            self.items.pop_back();
        }
    }

    /// Feed one drained daemon event into the center. Returns `true`
    /// only for [`AppEvent::Notification`]; every other event is
    /// ignored (it is still logged by `log_event` in `main.rs`).
    pub fn push_event(&mut self, ev: &AppEvent) -> bool {
        if let AppEvent::Notification { kind, title, body } = ev {
            self.push(kind, title, body);
            true
        } else {
            false
        }
    }

    /// Number of unread notifications.
    #[must_use]
    pub fn unread_count(&self) -> usize {
        self.items.iter().filter(|n| n.unread).count()
    }

    /// Suppressed-repeat count (F12) for one dedupe key: how many
    /// pushes with this exact `(kind, title, body)` were folded
    /// into the stored item instead of becoming new rows (`0` when
    /// no such item exists, or it was never repeated).
    /// Tests only (the drawer reads the `suppressed` field off the
    /// stored item instead).
    #[cfg(test)]
    #[must_use]
    pub fn suppressed_count(&self, kind: &str, title: &str, body: &str) -> u32 {
        self.items
            .iter()
            .find(|n| n.kind == kind && n.title == title && n.body == body)
            .map_or(0, |n| n.suppressed)
    }

    /// Mark every stored notification read (items are kept).
    pub fn mark_all_read(&mut self) {
        for n in &mut self.items {
            n.unread = false;
        }
    }

    /// Drop every stored notification.
    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// Slint model rows for the drawer (`notifications` property).
    /// A folded item renders its suppressed count in the title
    /// (F12): `"Pairing needed (×3)"` instead of three rows.
    #[must_use]
    pub fn to_model_items(&self) -> Vec<NotificationItem> {
        self.items
            .iter()
            .map(|n| NotificationItem {
                kind: n.kind.clone().into(),
                title: if n.suppressed > 0 {
                    format!("{} (×{})", n.title, n.suppressed).into()
                } else {
                    n.title.clone().into()
                },
                body: n.body.clone().into(),
                time: n.time.clone().into(),
                unread: n.unread,
            })
            .collect()
    }
}

/// Push the center's current state into the window: the `notifications`
/// model and the `notif-unread` badge count.
pub fn sync_to_window(center: &NotificationCenter, win: &MainWindow) {
    win.set_notifications(ModelRc::new(Rc::new(VecModel::from(
        center.to_model_items(),
    ))));
    win.set_notif_unread(i32::try_from(center.unread_count()).unwrap_or(i32::MAX));
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsh_api::{ConnectionState, SnapshotInfo};

    #[test]
    fn push_stores_item_and_counts_unread() {
        let mut c = NotificationCenter::new();
        assert_eq!(c.unread_count(), 0);
        c.push_with_time(
            "PairingNeeded",
            "Pair your board",
            "Press Begin pairing",
            "10:00:01",
        );
        let rows = c.to_model_items();
        assert_eq!(rows.len(), 1);
        assert_eq!(c.unread_count(), 1);
        let n = &rows[0];
        assert_eq!(n.kind, "PairingNeeded");
        assert_eq!(n.title, "Pair your board");
        assert_eq!(n.body, "Press Begin pairing");
        assert_eq!(n.time, "10:00:01");
        assert!(n.unread);
    }

    #[test]
    fn push_orders_newest_first() {
        let mut c = NotificationCenter::new();
        c.push_with_time("A", "First", "one", "10:00:01");
        c.push_with_time("B", "Second", "two", "10:00:02");
        let rows = c.to_model_items();
        assert_eq!(rows[0].title, "Second");
        assert_eq!(rows[1].title, "First");
    }

    #[test]
    fn push_is_capped_at_max_and_drops_oldest() {
        let mut c = NotificationCenter::new();
        for i in 0..MAX_NOTIFICATIONS + 5 {
            c.push_with_time("K", &format!("item {i}"), "b", "10:00:00");
        }
        let rows = c.to_model_items();
        assert_eq!(rows.len(), MAX_NOTIFICATIONS);
        assert_eq!(rows[0].title, format!("item {}", MAX_NOTIFICATIONS + 4));
        // Oldest ("item 0".."item 4") were dropped.
        assert!(rows.iter().all(|n| n.title != "item 0"));
    }

    #[test]
    fn dedupe_identical_pushes_within_cooldown_fold_into_one_item() {
        // F12: 5 identical pushes on a frozen fake clock → 1 stored
        // item, 4 suppressed repeats, drawer shows "(×4)".
        let mut c = NotificationCenter::new();
        for _ in 0..5 {
            c.push_with_time(
                "PairingNeeded",
                "Pair your board",
                "Press Begin pairing",
                "10:00:00",
            );
        }
        let rows = c.to_model_items();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            c.suppressed_count("PairingNeeded", "Pair your board", "Press Begin pairing"),
            4
        );
        assert_eq!(rows[0].title, "Pair your board (×4)");
        // One row, one unread — the flap does not inflate the badge.
        assert_eq!(c.unread_count(), 1);
    }

    #[test]
    fn dedupe_ten_identical_pushes_in_60s_fold_into_one_item() {
        // F17: the flap regression — 10 identical ConnectionLost
        // pushes spread over 60 s (one every ~6 s, the daemon
        // down/up flap cadence) must store exactly ONE row with 9
        // suppressed repeats, not 10 rows. Pre-F12 this stored 10.
        let mut c = NotificationCenter::new();
        for i in 0..10 {
            c.push_with_time(
                "ConnectionLost",
                "Connection lost",
                "Board unreachable",
                &format!("10:00:{:02}", i * 6),
            );
        }
        let rows = c.to_model_items();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            c.suppressed_count("ConnectionLost", "Connection lost", "Board unreachable"),
            9
        );
        assert_eq!(rows[0].title, "Connection lost (×9)");
        assert_eq!(c.unread_count(), 1);
    }

    #[test]
    fn dedupe_cooldown_expiry_allows_second_item() {
        // F12: PairingNeeded cooldown is 60 s. A repeat at +59 s is
        // still folded; a repeat 60 s after the last seen push is a
        // new row (a genuinely new flap, not the same one).
        let mut c = NotificationCenter::new();
        c.push_with_time(
            "PairingNeeded",
            "Pair your board",
            "Press Begin pairing",
            "10:00:00",
        );
        c.push_with_time(
            "PairingNeeded",
            "Pair your board",
            "Press Begin pairing",
            "10:00:59",
        );
        assert_eq!(c.to_model_items().len(), 1);
        assert_eq!(
            c.suppressed_count("PairingNeeded", "Pair your board", "Press Begin pairing"),
            1
        );
        c.push_with_time(
            "PairingNeeded",
            "Pair your board",
            "Press Begin pairing",
            "10:01:59",
        );
        assert_eq!(c.to_model_items().len(), 2);
    }

    #[test]
    fn dedupe_different_body_is_new_item_immediately() {
        // F12: the key is (kind, title, body) — a different body is
        // a different notification, even inside the cooldown.
        let mut c = NotificationCenter::new();
        c.push_with_time(
            "PairingNeeded",
            "Pair your board",
            "Press Begin pairing",
            "10:00:00",
        );
        c.push_with_time(
            "PairingNeeded",
            "Pair your board",
            "A different hint",
            "10:00:00",
        );
        assert_eq!(c.to_model_items().len(), 2);
        assert_eq!(
            c.suppressed_count("PairingNeeded", "Pair your board", "Press Begin pairing"),
            0
        );
    }

    #[test]
    fn dedupe_user_action_cooldown_is_5s() {
        // F12: user-action kinds (anything but Connection /
        // ConnectionLost / PairingNeeded) use the 5 s window.
        let mut c = NotificationCenter::new();
        c.push_with_time(
            "Profile",
            "Couldn't switch profile",
            "Try again",
            "10:00:00",
        );
        c.push_with_time(
            "Profile",
            "Couldn't switch profile",
            "Try again",
            "10:00:04",
        );
        assert_eq!(c.to_model_items().len(), 1);
        assert_eq!(
            c.suppressed_count("Profile", "Couldn't switch profile", "Try again"),
            1
        );
        // 6 s after the last seen push: past the window, new row.
        c.push_with_time(
            "Profile",
            "Couldn't switch profile",
            "Try again",
            "10:00:10",
        );
        assert_eq!(c.to_model_items().len(), 2);
    }

    #[test]
    fn dedupe_suppressed_count_survives_save_load() {
        // F12 + M10: the folded count is persisted with the item,
        // so a restart does not lose the "(×N)" or re-inflate rows.
        let dir = std::env::temp_dir().join(format!("rsh-notif-dedupe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut c = NotificationCenter::new();
        for _ in 0..3 {
            c.push_with_time(
                "ConnectionLost",
                "Connection lost",
                "Board unreachable",
                "10:00:00",
            );
        }
        c.save(Some(&dir));
        let loaded = NotificationCenter::load(Some(&dir));
        assert_eq!(loaded.to_model_items().len(), 1);
        assert_eq!(
            loaded.suppressed_count("ConnectionLost", "Connection lost", "Board unreachable"),
            2
        );
        assert_eq!(loaded.to_model_items()[0].title, "Connection lost (×2)");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mark_all_read_keeps_items_clears_unread() {
        let mut c = NotificationCenter::new();
        c.push("A", "One", "1");
        c.push("B", "Two", "2");
        assert_eq!(c.unread_count(), 2);
        c.mark_all_read();
        assert_eq!(c.unread_count(), 0);
        assert_eq!(c.to_model_items().len(), 2);
        assert!(c.to_model_items().iter().all(|n| !n.unread));
    }

    #[test]
    fn clear_empties_center() {
        let mut c = NotificationCenter::new();
        c.push("A", "One", "1");
        c.clear();
        assert_eq!(c.to_model_items().len(), 0);
        assert_eq!(c.unread_count(), 0);
    }

    #[test]
    fn save_and_load_round_trips() {
        // M10: notifications survive a restart (file, not memory).
        let dir = std::env::temp_dir().join(format!("rsh-notif-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut c = NotificationCenter::new();
        c.push_with_time("A", "First", "one", "10:00:01");
        c.push_with_time("B", "Second", "two", "10:00:02");
        c.mark_all_read();
        c.save(Some(&dir));
        let loaded = NotificationCenter::load(Some(&dir));
        let rows = loaded.to_model_items();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].title, "Second");
        assert_eq!(rows[1].title, "First");
        assert_eq!(loaded.unread_count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_missing_file_is_empty_center() {
        let dir = std::env::temp_dir().join(format!("rsh-notif-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            NotificationCenter::load(Some(&dir)).to_model_items().len(),
            0
        );
        assert_eq!(NotificationCenter::load(None).to_model_items().len(), 0);
    }

    #[test]
    fn push_event_only_accepts_notification_events() {
        let mut c = NotificationCenter::new();
        let ev = AppEvent::Notification {
            kind: "ConnectionLost".to_string(),
            title: "Connection lost".to_string(),
            body: "Board unreachable".to_string(),
        };
        assert!(c.push_event(&ev));
        assert_eq!(c.to_model_items().len(), 1);
        assert_eq!(c.to_model_items()[0].kind, "ConnectionLost");

        let other = AppEvent::ConnectionChanged {
            state: ConnectionState::Active,
            label: "Connected".to_string(),
            reason: None,
        };
        assert!(!c.push_event(&other));
        assert_eq!(c.to_model_items().len(), 1);
        assert!(!c.push_event(&AppEvent::Snapshot(SnapshotInfo::default())));
        assert_eq!(c.to_model_items().len(), 1);
    }

    #[test]
    fn to_model_items_mirrors_store() {
        let mut c = NotificationCenter::new();
        c.push_with_time("A", "Title", "Body", "10:00:01");
        c.mark_all_read();
        c.push_with_time("B", "Title 2", "Body 2", "10:00:02");
        let rows = c.to_model_items();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].title, "Title 2");
        assert!(rows[0].unread);
        assert_eq!(rows[1].title, "Title");
        assert!(!rows[1].unread);
        assert_eq!(rows[1].time, "10:00:01");
    }

    #[test]
    fn now_time_string_is_hh_mm_ss() {
        let t = now_time_string();
        assert_eq!(t.len(), 8);
        assert_eq!(t.as_bytes()[2], b':');
        assert_eq!(t.as_bytes()[5], b':');
    }
}
