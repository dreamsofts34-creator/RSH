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
//! and to stderr only with `--verbose`.
//!
//! Perf (F-7): the log holds ONE open buffered handle instead of
//! opening + writing + flushing + closing per line (86 `ui_log!`
//! sites, all on the UI thread). Lines accumulate in an 8 KiB
//! buffer and flush when it fills, every [`UI_LOG_FLUSH_LINES`]
//! lines, on day rollover, or on [`flush_ui_file_log`] (called at
//! exit from `main`, and from the panic hook installed by
//! `install_ui_panic_hook` in `main.rs` — R3-UI-04 — so a crash
//! flushes the last buffered lines instead of stranding them). The handle is deliberately reopened at each
//! flush boundary, so a delete-and-recreate of the log file can
//! strand at most 64 lines on a stale handle instead of the whole
//! session — the property the old open-per-line design bought, kept
//! at 1/64th of its cost.
//!
//! Timestamps are UTC (`std` has no local-time API; the daemon's
//! `diagnostics.rs` uses the same convention, and the civil-date
//! conversion below is the same days-from-civil algorithm). Lines
//! are flattened to one line and never carry a password — callers
//! log booleans / codes / titles only.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Lines buffered before the handle is flushed + reopened (F-7):
/// 64 lines per open/flush/close cycle instead of one per line.
const UI_LOG_FLUSH_LINES: u32 = 64;

/// `BufWriter` capacity: log lines are short, so 8 KiB holds many
/// lines before the buffer itself forces a flush.
const UI_LOG_BUF_CAPACITY: usize = 8192;

/// Held-open log state: the directory, today's open handle (if
/// any), the day it was opened for, and lines written since the
/// last flush/reopen.
struct UiLogState {
    dir: Option<PathBuf>,
    file: Option<BufWriter<File>>,
    day: Option<(i64, u32, u32)>,
    lines_since_flush: u32,
}

static UI_LOG: Mutex<UiLogState> = Mutex::new(UiLogState {
    dir: None,
    file: None,
    day: None,
    lines_since_flush: 0,
});

/// Point the UI file log at `<data_dir>/logs` (call once from
/// `main`, before the window is created). A `None` data dir leaves
/// file logging off — [`ui_file_log`] then no-ops.
pub fn init_ui_file_log(data_dir: Option<&Path>) {
    let dir = data_dir.map(|d| d.join("logs"));
    if let Some(dir) = &dir {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut state) = UI_LOG.lock() {
        if let Some(file) = state.file.as_mut() {
            let _ = file.flush();
        }
        *state = UiLogState {
            dir,
            file: None,
            day: None,
            lines_since_flush: 0,
        };
    }
    ui_file_log("rsh-ui started");
}

/// Append one timestamped line to today's `rsh-ui-YYYYMMDD.log`
/// (buffered — see the module docs; use [`flush_ui_file_log`] to
/// force it to disk, e.g. before reading the file in a test or at
/// exit). Never panics, never blocks startup: any failure is
/// silently dropped (logging must never fail the UI).
pub fn ui_file_log(message: &str) {
    let Ok(mut state) = UI_LOG.lock() else {
        return;
    };
    let Some(dir) = state.dir.clone() else {
        return;
    };
    let now = SystemTime::now();
    let (year, month, day, hour, minute, second) = utc_civil_parts(now);
    let today = (year, month, day);
    if state.day != Some(today) || state.file.is_none() {
        // Day rollover, first line, or post-flush reopen: flush +
        // drop the old handle, open today's file.
        if let Some(file) = state.file.as_mut() {
            let _ = file.flush();
        }
        state.file = None;
        state.day = Some(today);
        state.lines_since_flush = 0;
        let path = dir.join(format!("rsh-ui-{year:04}{month:02}{day:02}.log"));
        state.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok()
            .map(|f| BufWriter::with_capacity(UI_LOG_BUF_CAPACITY, f));
    }
    let single: String = message
        .chars()
        .map(|c| if c == '\n' { ' ' } else { c })
        .collect();
    let line =
        format!("[{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}Z] {single}\n");
    let wrote = {
        let Some(file) = state.file.as_mut() else {
            return;
        };
        file.write_all(line.as_bytes()).is_ok()
    };
    if !wrote {
        // Broken handle (disk full, file removed mid-write): drop
        // it — the next line reopens the file instead of retrying
        // a dead handle forever.
        state.file = None;
        return;
    }
    state.lines_since_flush += 1;
    if state.lines_since_flush >= UI_LOG_FLUSH_LINES {
        // Flush + reopen: bounds the stale-handle window (see the
        // module docs) and the amount lost if the process dies.
        if let Some(file) = state.file.as_mut() {
            let _ = file.flush();
        }
        state.file = None;
        state.lines_since_flush = 0;
    }
}

/// Flush any buffered UI log lines to disk (F-7): called once at
/// exit from `main`, and by tests before reading the log file.
/// No-op when logging is off or nothing is buffered.
pub fn flush_ui_file_log() {
    if let Ok(mut state) = UI_LOG.lock() {
        if let Some(file) = state.file.as_mut() {
            let _ = file.flush();
        }
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

    /// Reset the global log state (tests run in-process, so a test
    /// that pointed the log at its temp dir must not leak it into
    /// the next test).
    fn reset_ui_log() {
        if let Ok(mut state) = UI_LOG.lock() {
            if let Some(file) = state.file.as_mut() {
                let _ = file.flush();
            }
            *state = UiLogState {
                dir: None,
                file: None,
                day: None,
                lines_since_flush: 0,
            };
        }
    }

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
        // F-7: lines are buffered — flush before reading the file.
        flush_ui_file_log();
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
        reset_ui_log();
    }

    #[test]
    fn buffered_lines_land_on_flush_not_before() {
        // F-7: the held-open handle batches lines — a burst of
        // short lines stays in the buffer (one open, zero
        // flush/close cycles) until `flush_ui_file_log`.
        let dir = std::env::temp_dir().join(format!("rsh-ui-log-buf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        init_ui_file_log(Some(&dir));
        for i in 0..10 {
            ui_file_log(&format!("buffered line {i}"));
        }
        flush_ui_file_log();
        let logs = std::fs::read_dir(dir.join("logs")).expect("logs dir exists");
        let names: Vec<String> = logs
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 1, "one daily file: {names:?}");
        let text = std::fs::read_to_string(dir.join("logs").join(&names[0])).expect("read log");
        for i in 0..10 {
            assert!(
                text.contains(&format!("buffered line {i}")),
                "line {i} present"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
        reset_ui_log();
    }

    #[test]
    fn flush_without_init_is_noop() {
        // No data dir, no handle — flushing must not panic or
        // create anything.
        reset_ui_log();
        flush_ui_file_log();
        ui_file_log("nowhere to go");
        flush_ui_file_log();
    }
}
