//! Tray-icon geometry on Windows (REDESIGN-PLAN §7.7): where the
//! tray icon is, so the popup can open next to it.
//!
//! Verified against the Slint 1.18.1 source this daemon is built
//! with (`i-slint-core-1.18.1/items/system_tray/windows.rs`):
//! Slint's Windows tray backend creates one hidden message-only
//! window of class `SlintSystemTrayWindow` per tray icon and
//! registers the icon with `Shell_NotifyIconW(NIM_ADD, …)` and
//! `uID = 1` (`TRAY_UID`), no GUID. So the icon's screen rect is
//! reachable from our own process with exactly the API the brief
//! names:
//! 1. `FindWindowExW(HWND_MESSAGE, … "SlintSystemTrayWindow" …)` —
//!    enumerate message-only windows of that class and keep the one
//!    whose process id is ours (another Slint app's tray must never
//!    be mistaken for ours);
//! 2. `Shell_NotifyIconGetRect(NOTIFYICONIDENTIFIER { hWnd, uID: 1 })`.
//!
//! Both steps can fail (tray host hidden, icon in the overflow
//! flyout, Explorer restarted): the caller then falls back to the
//! bottom-right dock in [`crate::tray_popup::anchor_popup`], never
//! to a guessed position. Linux has no tray geometry to query —
//! `tray_icon_rect` returns `None` there and the same fallback
//! applies (Phase 2 documented the missing tray host on headless
//! Linux; the popup still works, docked bottom-right).

use crate::tray_popup::Rect;

/// The tray icon's screen rect, when Windows can report it.
#[cfg(windows)]
#[must_use]
#[allow(clippy::cast_possible_truncation)]
// cbSize: NOTIFYICONIDENTIFIER is a small fixed struct (tens of
// bytes); its size always fits in a u32.
pub fn tray_icon_rect() -> Option<Rect> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::Shell::{NOTIFYICONIDENTIFIER, Shell_NotifyIconGetRect};
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowExW, GetWindowThreadProcessId, HWND_MESSAGE,
    };
    use windows::core::w;

    // Slint's tray window class (see module docs). Message-only
    // windows are enumerated with HWND_MESSAGE as the parent.
    let class_name = w!("SlintSystemTrayWindow");
    let own_pid = unsafe { GetCurrentProcessId() };
    let mut candidate = HWND::default();
    loop {
        let found = unsafe { FindWindowExW(Some(HWND_MESSAGE), Some(candidate), class_name, None) }
            .unwrap_or_default();
        if found.is_invalid() {
            break;
        }
        candidate = found;
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(found, Some(&raw mut pid)) };
        if pid == own_pid {
            let identifier = NOTIFYICONIDENTIFIER {
                cbSize: std::mem::size_of::<NOTIFYICONIDENTIFIER>() as u32,
                hWnd: found,
                uID: 1,
                guidItem: windows::core::GUID::zeroed(),
            };
            let rect = unsafe { Shell_NotifyIconGetRect(&raw const identifier) }.ok()?;
            return Some(Rect {
                left: rect.left,
                top: rect.top,
                right: rect.right,
                bottom: rect.bottom,
            });
        }
    }
    None
}

/// No tray geometry on Linux: the popup docks bottom-right.
#[cfg(not(windows))]
#[must_use]
pub fn tray_icon_rect() -> Option<Rect> {
    None
}

/// The screen work area (screen minus taskbar), when Windows can
/// report it. Falls back to the full primary screen (1920×1080 is
/// NOT assumed — the fallback is the primary monitor's size from
/// `GetSystemMetrics`, so a smaller display still clamps sanely).
#[cfg(windows)]
#[must_use]
pub fn work_area() -> Rect {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN, SPI_GETWORKAREA,
        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    };
    let mut rect = RECT::default();
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some((&raw mut rect).cast::<std::ffi::c_void>()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS::default(),
        )
    };
    if ok.is_ok() && rect.right > rect.left && rect.bottom > rect.top {
        return Rect {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        };
    }
    let width = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    let height = unsafe { GetSystemMetrics(SM_CYSCREEN) };
    Rect {
        left: 0,
        top: 0,
        right: width.max(0),
        bottom: height.max(0),
    }
}

/// Linux fallback work area: no Win32 to ask, so the popup docks to
/// the bottom-right of a 1920×1080 area and `anchor_popup` clamps it
/// to the real window size once the popup is shown.
#[cfg(not(windows))]
#[must_use]
pub fn work_area() -> Rect {
    Rect {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    }
}

/// The popup window's native handle on Windows, found by its title
/// (`TrayPopup` sets `title: "RSH Wheel — Tray"`) and verified to
/// belong to this process. Used only by the click-outside poll:
/// when the popup is visible and the foreground window is some other
/// window, the popup closes. Returns `None` on Linux and while the
/// popup is not shown yet.
#[cfg(windows)]
#[must_use]
pub fn popup_hwnd(title: &str) -> Option<isize> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, GetWindowThreadProcessId};
    use windows::core::HSTRING;

    let hwnd = unsafe { FindWindowW(None, &HSTRING::from(title)) }.unwrap_or_default();
    if hwnd == HWND::default() {
        return None;
    }
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&raw mut pid)) };
    if pid == unsafe { GetCurrentProcessId() } {
        Some(hwnd.0 as isize)
    } else {
        None
    }
}

/// No popup window to find on Linux: the click-outside poll in
/// `main.rs` always sees `None` here and never dismisses.
#[cfg(not(windows))]
#[must_use]
pub fn popup_hwnd(_title: &str) -> Option<isize> {
    None
}

/// Bring the tray popup to the foreground when it is shown (H-2,
/// v2.0.2) — the second half of the A4 fix.
///
/// Root cause, restated: a window shown from a tray click usually
/// NEVER becomes the foreground window, because the tray-click
/// input went to Explorer/the taskbar, not to the daemon, so
/// Windows' foreground lock refuses the popup activation and the
/// old foreground poll dismissed it ~1 s later. The fixed poll
/// (`tray_popup::should_dismiss_popup`) already keeps such a
/// popup open, but a popup that never activates also never takes
/// the `was_foreground` path, so a click-away is only caught via
/// the foreground-change rule. Activating here makes the popup a
/// real foreground window in the common case, so both dismiss
/// rules work as designed.
///
/// `SetForegroundWindow` alone usually fails under the lock, so
/// the window is also briefly raised `TOPMOST` (which forces it
/// visible on top) and immediately dropped back to `NOTOPMOST`,
/// so it ends up top of the Z-order but NOT always-on-top. All
/// three calls are best-effort — a failure leaves the popup
/// visible but inactive, and the fixed poll still keeps it open
/// until a real click-away. Returns `true` when the popup window
/// was found (activation attempted), `false` on Linux (no Win32)
/// and while the popup window does not exist yet.
#[cfg(windows)]
#[must_use]
pub fn activate_popup_window(title: &str) -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SetForegroundWindow,
        SetWindowPos,
    };

    let Some(hwnd_value) = popup_hwnd(title) else {
        return false;
    };
    let hwnd = HWND(hwnd_value as *mut std::ffi::c_void);
    let _ = unsafe { SetForegroundWindow(hwnd) };
    let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW;
    let _ = unsafe { SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, flags) };
    let _ = unsafe { SetWindowPos(hwnd, Some(HWND_NOTOPMOST), 0, 0, 0, 0, flags) };
    true
}

/// No popup activation on Linux (no Win32 to ask).
#[cfg(not(windows))]
#[must_use]
pub fn activate_popup_window(_title: &str) -> bool {
    false
}

/// The current foreground window, if Windows can report one.
/// `None` when there is no foreground window. Used by the tray
/// popup's click-outside poll (A4): the poll compares this against
/// the foreground window recorded when the popup was shown — a
/// *change* means the user clicked away, while "still the same"
/// means they did nothing and the popup must stay open.
#[cfg(windows)]
#[must_use]
pub fn foreground_hwnd() -> Option<isize> {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_invalid() {
        None
    } else {
        Some(foreground.0 as isize)
    }
}

/// No foreground query on Linux: the popup never auto-dismisses
/// there (Escape / its own buttons close it).
#[cfg(not(windows))]
#[must_use]
pub fn foreground_hwnd() -> Option<isize> {
    None
}

/// Cursor position in screen pixels (Phase 2, F-12 / U-1): the
/// click-outside check for the tray popup — a fresh left-button
/// press counts as a click-away only when the cursor is outside
/// the popup rect.
#[cfg(windows)]
#[must_use]
pub fn cursor_position() -> Option<(i32, i32)> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut point = POINT::default();
    unsafe { GetCursorPos(&raw mut point) }.ok()?;
    Some((point.x, point.y))
}

/// No cursor query on Linux: the click-outside path is inert there.
#[cfg(not(windows))]
#[must_use]
pub fn cursor_position() -> Option<(i32, i32)> {
    None
}

/// Whether the left mouse button was pressed since the last call
/// (Phase 2, F-12 / U-1): `GetAsyncKeyState` bit 0 is the
/// "pressed since last read" latch, so a quick press is caught
/// even if it is released between 100 ms ticks. Each call clears
/// the latch — the tick calls this once per cycle, so the tray
/// click that showed the popup is consumed by the first tick and
/// cannot dismiss its own popup. Bit 15 (currently down) is NOT
/// used: a held button must not dismiss on every tick.
#[cfg(windows)]
#[must_use]
pub fn left_button_pressed_since_last_poll() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
    // Parentheses are load-bearing: `&` binds looser than `!=`,
    // so an unparenthesised `x & 1 != 0` reads as `x & (1 != 0)`.
    (unsafe { GetAsyncKeyState(i32::from(VK_LBUTTON.0)) } & 1) != 0
}

/// No button query on Linux: the click-outside path is inert there.
#[cfg(not(windows))]
#[must_use]
pub fn left_button_pressed_since_last_poll() -> bool {
    false
}

/// Title of the main UI window (`rsh-ui`, `ui/main.slint`).
#[cfg(windows)]
pub const UI_WINDOW_TITLE: &str = "RSH — Real Sim Hub";

/// Title of the tray popup window (`TrayPopup` in `ui/tray.slint`).
/// Used by the click-outside poll and the show-time activation so
/// the title lives in exactly one place.
pub const POPUP_WINDOW_TITLE: &str = "RSH Wheel — Tray";

/// File name of the UI executable (Windows: `rsh-ui.exe`).
#[cfg(windows)]
const UI_EXE_NAME: &str = "rsh-ui.exe";

/// The main UI window's native handle on Windows, found by its
/// title and verified to belong to an `rsh-ui` process (a
/// same-title window owned by another program must never be
/// mistaken for ours). `None` on Linux and while the UI is not
/// running (or its window is not created yet — the UI is still
/// starting up).
#[cfg(windows)]
#[must_use]
pub fn ui_window_hwnd() -> Option<isize> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, GetWindowThreadProcessId};
    use windows::core::HSTRING;

    let hwnd = unsafe { FindWindowW(None, &HSTRING::from(UI_WINDOW_TITLE)) }.unwrap_or_default();
    if hwnd == HWND::default() {
        return None;
    }
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&raw mut pid)) };
    if process_exe_name(pid).is_some_and(|name| name.eq_ignore_ascii_case(UI_EXE_NAME)) {
        Some(hwnd.0 as isize)
    } else {
        None
    }
}

/// No UI window lookup on Linux (no Win32 to ask). Only the
/// Windows restore path and the tests use this; the Linux daemon
/// never calls it — kept so the lookup API is the same on both
/// platforms.
#[cfg(not(windows))]
#[allow(dead_code)]
#[must_use]
pub fn ui_window_hwnd() -> Option<isize> {
    None
}

/// The pid of a running `rsh-ui` process, if one exists.
///
/// Windows: `ToolHelp` snapshot, matched on the exe file name
/// (`rsh-ui.exe`) — the same pattern as
/// `rsh-telemetry::windows::is_process_running`, so a UI launched
/// by double-click / the installer (not by the daemon) is found
/// too. Linux: `/proc` scan for a cmdline naming `rsh-ui`
/// (zombies skipped — a dead UI must not block a fresh spawn).
/// `None` when no UI process is running.
#[cfg(windows)]
#[must_use]
pub fn ui_process_id() -> Option<u32> {
    use windows::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }.ok()?;
    if snapshot == INVALID_HANDLE_VALUE {
        return None;
    }
    let mut entry = PROCESSENTRY32W {
        dwSize: u32::try_from(std::mem::size_of::<PROCESSENTRY32W>()).unwrap_or(u32::MAX),
        ..Default::default()
    };
    let mut found = None;
    let mut next = unsafe { Process32FirstW(snapshot, &raw mut entry) };
    while next.is_ok() {
        let end = entry
            .szExeFile
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(entry.szExeFile.len());
        let name = String::from_utf16_lossy(&entry.szExeFile[..end]);
        if name.eq_ignore_ascii_case(UI_EXE_NAME) {
            found = Some(entry.th32ProcessID);
            break;
        }
        next = unsafe { Process32NextW(snapshot, &raw mut entry) };
    }
    let _ = unsafe { CloseHandle(snapshot) };
    found
}

/// Linux `rsh-ui` process scan (see the Windows docs above).
#[cfg(not(windows))]
#[must_use]
pub fn ui_process_id() -> Option<u32> {
    let own_pid = std::process::id();
    let entries = std::fs::read_dir("/proc").ok()?;
    for entry in entries.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        if pid == own_pid {
            continue;
        }
        // Zombie: dead, but /proc still lists it.
        if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat"))
            && let Some(state) = stat
                .rsplit(") ")
                .next()
                .and_then(|rest| rest.chars().next())
            && state == 'Z'
        {
            continue;
        }
        // Match ONLY argv[0]'s file name (the exe name): a loose
        // "any arg contains rsh-ui" match also fires on unrelated
        // processes whose command line merely mentions rsh-ui —
        // e.g. the very `cargo test -p rsh-ui` that runs this
        // suite, or a terminal running an rsh-ui command — and
        // then `open_ui` wrongly believes a UI is running and
        // never spawns one. A real UI has argv[0] = its exe path
        // (basename `rsh-ui`); the test fake sets argv[0] to
        // `rsh-ui` via `exec -a`.
        if let Ok(cmdline) = std::fs::read(format!("/proc/{pid}/cmdline"))
            && let Some(argv0) = String::from_utf8_lossy(&cmdline).split('\0').next()
            && std::path::Path::new(argv0)
                .file_name()
                .is_some_and(|name| name == "rsh-ui" || name == "rsh-ui.exe")
        {
            return Some(pid);
        }
    }
    None
}

/// The exe file name of `pid` on Windows (`None` when the process
/// is gone or its entry cannot be read).
#[cfg(windows)]
fn process_exe_name(pid: u32) -> Option<String> {
    use windows::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }.ok()?;
    if snapshot == INVALID_HANDLE_VALUE {
        return None;
    }
    let mut entry = PROCESSENTRY32W {
        dwSize: u32::try_from(std::mem::size_of::<PROCESSENTRY32W>()).unwrap_or(u32::MAX),
        ..Default::default()
    };
    let mut found = None;
    let mut next = unsafe { Process32FirstW(snapshot, &raw mut entry) };
    while next.is_ok() {
        if entry.th32ProcessID == pid {
            let end = entry
                .szExeFile
                .iter()
                .position(|unit| *unit == 0)
                .unwrap_or(entry.szExeFile.len());
            found = Some(String::from_utf16_lossy(&entry.szExeFile[..end]));
            break;
        }
        next = unsafe { Process32NextW(snapshot, &raw mut entry) };
    }
    let _ = unsafe { CloseHandle(snapshot) };
    found
}

/// Restore / show the already-running UI window (A11, v2.0.2):
/// a window hidden to the tray is shown again, a minimised one is
/// restored, and the window is brought to the foreground. Returns
/// `true` when a UI window was found and shown — the caller must
/// NOT spawn a second UI then. `false` when no UI window exists
/// (UI not running, or still starting up with no window yet).
///
/// Windows only: `SetForegroundWindow` can still fail under the
/// foreground lock (the daemon is a background process), but the
/// window is visible and restored regardless — the user can click
/// it. On Linux there is no portable window raise, so this always
/// returns `false` there; the caller falls back to the
/// `ui_process_id` check (a running UI is left alone, a dead one
/// is spawned fresh) instead of duplicating the UI.
#[cfg(windows)]
#[must_use]
pub fn restore_ui_window() -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        IsWindowVisible, SW_RESTORE, SW_SHOW, SetForegroundWindow, ShowWindow,
    };

    let Some(hwnd_value) = ui_window_hwnd() else {
        return false;
    };
    let hwnd = HWND(hwnd_value as *mut std::ffi::c_void);
    // Hidden (hide-to-tray) → show; minimised / visible → restore.
    let command = if unsafe { IsWindowVisible(hwnd) }.as_bool() {
        SW_RESTORE
    } else {
        SW_SHOW
    };
    let _ = unsafe { ShowWindow(hwnd, command) };
    let _ = unsafe { SetForegroundWindow(hwnd) };
    true
}

/// No window raise on Linux (see the Windows docs above).
#[cfg(not(windows))]
#[must_use]
pub fn restore_ui_window() -> bool {
    false
}

#[cfg(test)]
#[cfg(not(windows))]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt;

    #[test]
    fn linux_ui_process_scan_finds_rsh_ui_and_ignores_other_programs() {
        // argv[0] set directly — no shell: `sh` here is dash,
        // which has no `exec -a`, so a `sh -c "exec -a ..."` fake
        // dies instantly and the scan finds nothing.
        let mut ui = std::process::Command::new("sleep")
            .arg("30")
            .arg0("rsh-ui")
            .spawn()
            .expect("spawn fake ui");
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(ui_process_id(), Some(ui.id()));
        let _ = ui.kill();
        let _ = ui.wait();
        // H-1 (v2.0.2): once the UI process is dead the scan must
        // report nothing — that is what sends `open_ui` down the
        // spawn-a-fresh-UI branch instead of restoring a dead
        // window after an X-close.
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert_eq!(ui_process_id(), None);

        let mut other = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert_ne!(ui_process_id(), Some(other.id()));
        let _ = other.kill();
        let _ = other.wait();
    }

    #[test]
    fn linux_restore_ui_window_is_always_false() {
        assert!(!restore_ui_window());
        assert!(ui_window_hwnd().is_none());
        assert!(foreground_hwnd().is_none());
    }
}
