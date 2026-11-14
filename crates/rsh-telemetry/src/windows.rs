//! The Windows-only live glue: opening a game's named file mapping
//! read-only and checking the game process is running (SEC-H-05b).
//!
//! This module is `#[cfg(windows)]` only. The `windows` crate does not
//! compile on Linux, so nothing here is ever built on Linux — the
//! readers' `read()` methods return `None` there instead (see
//! `readers.rs`). Verified API surface (`windows` 0.62.2, generated
//! source): `OpenFileMappingW`, `MapViewOfFile`, `UnmapViewOfFile`,
//! `CloseHandle`, `CreateToolhelp32Snapshot`, `Process32FirstW` /
//! `Process32NextW`, `PROCESSENTRY32W.szExeFile: [u16; 260]`.

use std::ffi::c_void;
use std::ptr::copy_nonoverlapping;

use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Memory::{
    FILE_MAP_READ, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, OpenFileMappingW, UnmapViewOfFile,
};
use windows::core::PCWSTR;

use crate::access::MapAccessor;

/// A read-only view of one named file mapping.
///
/// Opens with `OpenFileMappingW(FILE_MAP_READ)` and maps exactly `len`
/// bytes (a partial view is legal — the view may be smaller than the
/// mapping). The game memory is never written. `Drop` unmaps the view
/// and closes the handle.
pub(crate) struct SharedMemoryMap {
    handle: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    len: usize,
}

impl SharedMemoryMap {
    /// Opens the named file mapping read-only and maps `len` bytes.
    ///
    /// Returns `None` when the mapping does not exist (game not running
    /// / session not loaded) or the view cannot be mapped.
    pub(crate) fn open(name: &str, len: usize) -> Option<Self> {
        if len == 0 {
            return None;
        }
        let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
        let handle =
            unsafe { OpenFileMappingW(FILE_MAP_READ.0, false, PCWSTR::from_raw(wide.as_ptr())) }
                .ok()?;
        let view = unsafe { MapViewOfFile(handle, FILE_MAP_READ, 0, 0, len) };
        if view.Value.is_null() {
            let _ = unsafe { CloseHandle(handle) };
            return None;
        }
        Some(Self { handle, view, len })
    }

    /// Base pointer of the mapped view.
    fn ptr(&self) -> *const c_void {
        self.view.Value
    }
}

impl MapAccessor for SharedMemoryMap {
    fn len(&self) -> usize {
        self.len
    }

    fn read_into(&self, offset: usize, buf: &mut [u8]) -> bool {
        let Some(end) = offset.checked_add(buf.len()) else {
            return false;
        };
        if end > self.len {
            return false;
        }
        // SAFETY: `end <= self.len` was just checked, so
        // `[offset, end)` lies inside the mapped view, which lives as
        // long as `self`; the view is read-only and `buf` is a valid
        // write target of exactly `buf.len()` bytes. The game may write
        // the source concurrently — that is why callers double-read —
        // but a racing source byte is still a readable byte.
        unsafe {
            copy_nonoverlapping(
                self.ptr().cast::<u8>().add(offset),
                buf.as_mut_ptr(),
                buf.len(),
            );
        }
        true
    }
}

impl Drop for SharedMemoryMap {
    fn drop(&mut self) {
        let _ = unsafe { UnmapViewOfFile(self.view) };
        let _ = unsafe { CloseHandle(self.handle) };
    }
}

/// Closes a `ToolHelp` snapshot handle on drop.
struct SnapshotGuard(HANDLE);

impl Drop for SnapshotGuard {
    fn drop(&mut self) {
        if self.0 != INVALID_HANDLE_VALUE {
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
}

/// Whether a process with the given exe file name is running
/// (case-insensitive, SEC-H-05b process-ownership gate).
///
/// Compares the `ToolHelp` `szExeFile` file name — the main module's exe
/// name, not a full path. This is defense-in-depth only (the C# doc
/// notes no complete user-mode proof of map ownership exists); the
/// damage a spoofed map can do is limited to a tuning-profile switch.
/// Returns `false` on any error.
pub(crate) fn is_process_running(exe_name: &str) -> bool {
    let snapshot = match unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) } {
        Ok(handle) => SnapshotGuard(handle),
        Err(_) => return false,
    };
    let mut entry = PROCESSENTRY32W {
        dwSize: u32::try_from(std::mem::size_of::<PROCESSENTRY32W>()).unwrap_or(u32::MAX),
        ..Default::default()
    };
    let mut first = unsafe { Process32FirstW(snapshot.0, &raw mut entry) };
    while first.is_ok() {
        let end = entry
            .szExeFile
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(entry.szExeFile.len());
        let name = String::from_utf16_lossy(&entry.szExeFile[..end]);
        if name.eq_ignore_ascii_case(exe_name) {
            return true;
        }
        first = unsafe { Process32NextW(snapshot.0, &raw mut entry) };
    }
    false
}
