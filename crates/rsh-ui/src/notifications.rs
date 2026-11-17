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
}

impl Notification {
    /// New unread notification stamped with the current time.
    #[must_use]
    pub fn new(kind: &str, title: &str, body: &str) -> Self {
        Self {
            kind: kind.to_string(),
            title: title.to_string(),
            body: body.to_string(),
            time: now_time_string(),
            unread: true,
        }
    }
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
    /// after every mutation (push / mark-all-read / clear).
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

    /// Push a notification (newest first, capped at
    /// [`MAX_NOTIFICATIONS`]). Returns the stored item.
    pub fn push(&mut self, kind: &str, title: &str, body: &str) -> Notification {
        let item = Notification::new(kind, title, body);
        self.push_item(item.clone());
        item
    }

    /// Push with an explicit time string (tests only; production
    /// code uses [`NotificationCenter::push`]).
    #[cfg(test)]
    pub fn push_with_time(
        &mut self,
        kind: &str,
        title: &str,
        body: &str,
        time: &str,
    ) -> Notification {
        let item = Notification {
            kind: kind.to_string(),
            title: title.to_string(),
            body: body.to_string(),
            time: time.to_string(),
            unread: true,
        };
        self.push_item(item.clone());
        item
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
    #[must_use]
    pub fn to_model_items(&self) -> Vec<NotificationItem> {
        self.items
            .iter()
            .map(|n| NotificationItem {
                kind: n.kind.clone().into(),
                title: n.title.clone().into(),
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
