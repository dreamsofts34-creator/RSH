//! Native Windows file picker for the add/edit game dialog (PC-test
//! Bug 6): the dialog used to be a manual text form because `rfd`
//! could not resolve offline on the Linux VM — but that was never a
//! Windows limitation. The picker is the Win32 `IFileOpenDialog`
//! straight from the `windows` crate (no new crate, no offline
//! resolution risk): Add opens the small Explorer picker, Cancel
//! leaves the dialog exactly as it was, and a selection fills the
//! exe path and auto-fills the game name from the file stem.
//!
//! Windows-only behaviour lives behind `#[cfg(windows)]`; other
//! platforms get a stub that reports the picker as unavailable, so
//! the crate still builds on the Linux rig. The real proof is CI
//! `windows-check` + the user-PC test — a Linux build cannot show a
//! Windows Explorer window, and this module never pretends it can.

use std::path::{Path, PathBuf};

/// Derive a game display name from a picked file path: the file
/// stem with `_` as word separators ("`assetto_corsa.exe`" →
/// "assetto corsa", "Forza Horizon 4.lnk" → "Forza Horizon 4").
/// No title-casing — it would mangle names like "iRacing".
/// Empty when the path has no usable stem, so the caller leaves the
/// name field alone and the user types one.
#[must_use]
pub fn game_name_from_path(path: &Path) -> String {
    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
        return String::new();
    };
    stem.replace('_', " ")
        .split_whitespace()
        .collect::<Vec<&str>>()
        .join(" ")
}

/// Open the native Windows file picker for a game exe or shortcut
/// (`.exe` / `.lnk`, single select, file must exist).
///
/// Returns `Ok(Some(path))` on a selection, `Ok(None)` when the
/// user cancels (the dialog stays exactly as it was — no error,
/// no toast), and `Err(detail)` when the picker itself could not
/// be shown or read. `detail` is a log line, never user-facing
/// wording: the caller shows a plain message instead.
///
/// # Errors
///
/// Returns `Err` when COM cannot start the dialog, the dialog
/// fails to open, or the picked path cannot be read. Cancelling
/// is not an error — it returns `Ok(None)`.
#[cfg(windows)]
pub fn pick_game_exe() -> Result<Option<PathBuf>, String> {
    use windows::Win32::Foundation::S_OK;
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};

    // The Slint UI thread is not guaranteed to have COM up already:
    // initialise STA for this call and uninitialise only when this
    // call owned it (`S_OK`). `S_FALSE` (already initialised) and
    // `RPC_E_CHANGED_MODE` (already initialised another way) both
    // mean "COM is up, not ours" — the dialog attempt below still
    // runs and reports its own failure if COM is really unusable.
    let init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let owns_com = init == S_OK;
    let picked = pick_game_exe_inner();
    if owns_com {
        unsafe { CoUninitialize() };
    }
    picked
}

/// The dialog itself, after COM is up (see [`pick_game_exe`]).
/// Every failure path returns a log-ready detail string.
#[cfg(windows)]
fn pick_game_exe_inner() -> Result<Option<PathBuf>, String> {
    use windows::core::{HRESULT, HSTRING, PCWSTR};
    use windows::Win32::Foundation::ERROR_CANCELLED;
    use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER};
    use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
    use windows::Win32::UI::Shell::{
        FileOpenDialog, IFileOpenDialog, FOS_DONTADDTORECENT, FOS_FILEMUSTEXIST,
        FOS_FORCEFILESYSTEM, FOS_NOCHANGEDIR, FOS_PATHMUSTEXIST, SIGDN_FILESYSPATH,
    };
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    /// A NUL-terminated UTF-16 copy of `s` for a `PCWSTR` field.
    /// The caller keeps the `Vec` alive until the dialog closes.
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    let dialog: IFileOpenDialog =
        unsafe { CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) }
            .map_err(|e| format!("IFileOpenDialog CoCreateInstance failed: {e}"))?;
    // The filter strings live in these locals until the dialog
    // closes — the dialog reads them through the pointers below.
    let game_name = wide("Game programs");
    let game_spec = wide("*.exe;*.lnk");
    let exe_name = wide("Programs");
    let exe_spec = wide("*.exe");
    let lnk_name = wide("Shortcuts");
    let lnk_spec = wide("*.lnk");
    let all_name = wide("All files");
    let all_spec = wide("*.*");
    let filters = [
        COMDLG_FILTERSPEC {
            pszName: PCWSTR(game_name.as_ptr()),
            pszSpec: PCWSTR(game_spec.as_ptr()),
        },
        COMDLG_FILTERSPEC {
            pszName: PCWSTR(exe_name.as_ptr()),
            pszSpec: PCWSTR(exe_spec.as_ptr()),
        },
        COMDLG_FILTERSPEC {
            pszName: PCWSTR(lnk_name.as_ptr()),
            pszSpec: PCWSTR(lnk_spec.as_ptr()),
        },
        COMDLG_FILTERSPEC {
            pszName: PCWSTR(all_name.as_ptr()),
            pszSpec: PCWSTR(all_spec.as_ptr()),
        },
    ];
    unsafe {
        dialog
            .SetFileTypes(&filters)
            .map_err(|e| format!("IFileDialog SetFileTypes failed: {e}"))?;
        dialog
            .SetFileTypeIndex(1)
            .map_err(|e| format!("IFileDialog SetFileTypeIndex failed: {e}"))?;
        dialog
            .SetOptions(
                FOS_FILEMUSTEXIST
                    | FOS_PATHMUSTEXIST
                    | FOS_FORCEFILESYSTEM
                    | FOS_NOCHANGEDIR
                    | FOS_DONTADDTORECENT,
            )
            .map_err(|e| format!("IFileDialog SetOptions failed: {e}"))?;
        dialog
            .SetTitle(&HSTRING::from("Choose a game"))
            .map_err(|e| format!("IFileDialog SetTitle failed: {e}"))?;
    }
    // Own the dialog with the foreground window (the RSH window the
    // user just clicked Browse in) so the picker opens above it
    // instead of as an unowned top-level window.
    let owner = unsafe { GetForegroundWindow() };
    let owner = if owner.0.is_null() { None } else { Some(owner) };
    match unsafe { dialog.Show(owner) } {
        Ok(()) => {}
        // Cancel / Esc / close is the normal "no selection" path —
        // not an error, and the game dialog stays as it was.
        Err(e) if e.code() == HRESULT::from_win32(ERROR_CANCELLED.0) => return Ok(None),
        Err(e) => return Err(format!("IFileDialog Show failed: {e}")),
    }
    let item =
        unsafe { dialog.GetResult() }.map_err(|e| format!("IFileDialog GetResult failed: {e}"))?;
    let raw_path = unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) }
        .map_err(|e| format!("IShellItem GetDisplayName failed: {e}"))?;
    // `GetDisplayName` hands us a CoTaskMem string: copy it out,
    // then free it (the `PWSTR` wrapper does not free itself).
    let path_str = unsafe { raw_path.to_string() }
        .map_err(|_| "picked file path was not valid Unicode".to_string());
    unsafe { CoTaskMemFree(Some(raw_path.as_ptr().cast::<core::ffi::c_void>())) };
    Ok(Some(PathBuf::from(path_str?)))
}

/// No native picker off Windows: the RSH app ships on Windows, and
/// the Linux build is the dev rig. The caller shows the plain
/// "only on Windows" message — the Browse button never pretends
/// a picker opened.
///
/// # Errors
///
/// Always returns `Err`: there is no native picker on this
/// platform.
#[cfg(not(windows))]
pub fn pick_game_exe() -> Result<Option<PathBuf>, String> {
    Err("the file picker is only available on Windows".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // NOTE: `Path` is platform-dependent — backslash is a separator
    // only on Windows. The forward-slash cases below run everywhere;
    // the real `C:\Games\...` paths are covered by the
    // `#[cfg(windows)]` test, which runs in CI `test (Windows)`.
    #[test]
    fn game_name_from_exe_stem() {
        assert_eq!(game_name_from_path(Path::new("games/acc.exe")), "acc");
        assert_eq!(
            game_name_from_path(Path::new("/home/user/games/iRacing.exe")),
            "iRacing"
        );
    }

    #[test]
    fn game_name_from_path_underscores_become_spaces() {
        assert_eq!(
            game_name_from_path(Path::new("D:/Games/assetto_corsa.exe")),
            "assetto corsa"
        );
        assert_eq!(
            game_name_from_path(Path::new("my__game___name.exe")),
            "my game name"
        );
    }

    #[test]
    fn game_name_from_lnk_stem_keeps_spaces() {
        assert_eq!(
            game_name_from_path(Path::new("/home/user/Games/Forza Horizon 4.lnk")),
            "Forza Horizon 4"
        );
    }

    #[test]
    fn game_name_from_path_empty_when_no_stem() {
        assert_eq!(game_name_from_path(Path::new("")), "");
        assert_eq!(game_name_from_path(Path::new("/")), "");
    }

    #[test]
    fn game_name_from_path_case_preserved() {
        assert_eq!(
            game_name_from_path(Path::new("AC_Competizione.EXE")),
            "AC Competizione"
        );
    }

    /// The paths the picker actually returns on a user PC.
    #[test]
    #[cfg(windows)]
    fn game_name_from_windows_paths() {
        assert_eq!(
            game_name_from_path(Path::new(r"C:\Games\Assetto Corsa Competizione\acc.exe")),
            "acc"
        );
        assert_eq!(
            game_name_from_path(Path::new(r"D:\Games\assetto_corsa.exe")),
            "assetto corsa"
        );
        assert_eq!(
            game_name_from_path(Path::new(r"C:\Games\Forza Horizon 4.lnk")),
            "Forza Horizon 4"
        );
    }
}
