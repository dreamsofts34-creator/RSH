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
        unsafe { GetWindowThreadProcessId(found, Some(&mut pid as *mut u32)) };
        if pid == own_pid {
            let identifier = NOTIFYICONIDENTIFIER {
                cbSize: std::mem::size_of::<NOTIFYICONIDENTIFIER>() as u32,
                hWnd: found,
                uID: 1,
                guidItem: windows::core::GUID::zeroed(),
            };
            let rect = unsafe { Shell_NotifyIconGetRect(&identifier) }.ok()?;
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
        GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN, SPI_GETWORKAREA, SystemParametersInfoW,
    };
    let mut rect = RECT::default();
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut rect as *mut RECT as *mut std::ffi::c_void),
            Default::default(),
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
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid as *mut u32)) };
    if pid == unsafe { GetCurrentProcessId() } {
        Some(hwnd.0 as isize)
    } else {
        None
    }
}

/// Whether `hwnd` (a raw `HWND` value from [`popup_hwnd`]) is the
/// current foreground window. `None` when Windows cannot tell.
#[cfg(windows)]
#[must_use]
pub fn is_foreground(hwnd_value: isize) -> Option<bool> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    let foreground = unsafe { GetForegroundWindow() };
    Some(foreground == HWND(hwnd_value as *mut std::ffi::c_void))
}
