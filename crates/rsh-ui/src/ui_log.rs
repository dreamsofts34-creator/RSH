//! UI file log (Phase 2, L-1): `rsh-ui` writes a log file to
//! `<data_dir>/logs/rsh-ui-YYYYMMDD.log` — the same directory as the
//! daemon's `rsh-YYYYMMDD.log`, so the user's PC has one place to
//! look. Before this, the UI only wrote to stderr via `ui_log!`
//! (and only with `--verbose`), and the UI is launched by
//! double-click / daemon self-spawn with no terminal — so nothing
//! was ever recorded anywhere ("Ui log ni hai").
//!
//! Always-on for key events (wizard transitions, confirm/replace
//! clicks + IPC results, popup open/close, errors/toasts): the
//! `ui_log!` macro in `main.rs` writes to this file unconditionally
//! and to stderr only with `--verbose`. The file is opened once per
//! line (append + close): key events are rare (user actions, job
//! results), so the per-line open costs nothing and there is no
//! held handle to go stale across a delete-and-recreate.
//!
//! Timestamps are UTC (`std` has no local-time API; the daemon's
//! `diagnostics.rs` uses the same convention, and the civil-date
//! conversion below is the same days-from-civil algorithm). Lines
//! are flattened to one line and never carry a password — callers
//! log booleans / codes / titles only.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

static UI_LOG_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Point the UI file log at `<data_dir>/logs` (call once from
/// `main`, before the window is created). A `None` data dir leaves
/// file logging off — [`ui_file_log`] then no-ops.
pub fn init_ui_file_log(data_dir: Option<&Path>) {
    let dir = data_dir.map(|d| d.join("logs"));
    if let Some(dir) = &dir {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut slot) = UI_LOG_DIR.lock() {
        *slot = dir;
    }
    ui_file_log("rsh-ui started");
}

/// Append one timestamped line to today's `rsh-ui-YYYYMMDD.log`.
/// Never panics, never blocks startup: any failure is silently
/// dropped (logging must never fail the UI).
pub fn ui_file_log(message: &str) {
    let Some(dir) = UI_LOG_DIR.lock().ok().and_then(|slot| slot.clone()) else {
        return;
    };
    let now = SystemTime::now();
    let (year, month, day, hour, minute, second) = utc_civil_parts(now);
    let path = dir.join(format!("rsh-ui-{year:04}{month:02}{day:02}.log"));
    let single: String = message
        .chars()
        .map(|c| if c == '\n' { ' ' } else { c })
        .collect();
    let line =
        format!("[{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}Z] {single}\n");
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = file.write_all(line.as_bytes());
        let _ = file.flush();
    }
}

/// `(year, month, day, hour, minute, second)` in UTC for `now`
/// (Howard Hinnant's days-from-civil algorithm, std-only — the
/// same conversion `rsh-app-core`'s `diagnostics.rs` uses, kept
/// local here so `rsh-ui` needs no `rsh-app-core` dependency just
/// to timestamp a log line).
fn utc_civil_parts(now: SystemTime) -> (i64, u32, u32, u32, u32, u32) {
    let secs = i64::try_from(now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()))
        .unwrap_or(i64::MAX);
    let days = secs.div_euclid(86_400);
    let secs_of_day = secs.rem_euclid(86_400);
    let hour = u32::try_from(secs_of_day / 3_600).unwrap_or(23);
    let minute = u32::try_from((secs_of_day % 3_600) / 60).unwrap_or(59);
    let second = u32::try_from(secs_of_day % 60).unwrap_or(59);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = u32::try_from(z.rem_euclid(146_097)).unwrap_or(0);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = i64::from(yoe) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    (year, month, day, hour, minute, second)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_civil_parts_matches_known_date() {
        // 2026-11-19 15:46:28 UTC.
        let now = UNIX_EPOCH + std::time::Duration::from_secs(1_795_103_188);
        assert_eq!(utc_civil_parts(now), (2026, 11, 19, 15, 46, 28));
    }

    #[test]
    fn file_log_writes_daily_file() {
        let dir = std::env::temp_dir().join(format!("rsh-ui-log-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        init_ui_file_log(Some(&dir));
        ui_file_log("test event: wizard step 0 -> 1");
        let logs = std::fs::read_dir(dir.join("logs")).expect("logs dir exists");
        let names: Vec<String> = logs
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 1, "one daily file: {names:?}");
        assert!(names[0].starts_with("rsh-ui-"));
        assert!(
            std::path::Path::new(&names[0])
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("log")),
            "daily file ends in .log: {names:?}"
        );
        let text = std::fs::read_to_string(dir.join("logs").join(&names[0])).expect("read log");
        assert!(text.contains("wizard step 0 -> 1"), "event line present");
        assert!(text.contains("rsh-ui started"), "startup line present");
        let _ = std::fs::remove_dir_all(&dir);
        // Leave the static dir pointing at the removed dir: tests
        // run in-process, so reset it for any later test.
        if let Ok(mut slot) = UI_LOG_DIR.lock() {
            *slot = None;
        }
    }
}
