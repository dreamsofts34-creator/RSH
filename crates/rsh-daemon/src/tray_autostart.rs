//! Autostart for the v2.0 daemon (REDESIGN-PLAN §7 Phase 7,
//! LOGIC-FIX-PLAN §4.6 D-7).
//!
//! The daemon is the autostart target now — not the old Tauri
//! `RSH.exe`. The entry points at `rsh-daemon.exe` with no
//! arguments: the daemon creates its tray icon on every launch, so
//! no `--tray` flag is needed, and the daemon's own argument parser
//! would reject the v1 `--rsh-version` argument. A stale v1 entry
//! (pointing at the old exe) is migrated by the repair step below.
//!
//! # No login-time UAC surprise (D-7)
//!
//! Autostart is safe at login only because of the lazy-start
//! strategy, which already lives in `rsh-app-core` and the daemon
//! honours by calling it:
//! * the daemon exe itself needs no elevation, so the Run-key launch
//!   never prompts;
//! * `AppStateCore::activate_startup_backend` defers the elevated
//!   `HIDMaestro` backend at startup (it is only activated when the
//!   user turns "Send to game" on — the UAC prompt there is
//!   expected, user-initiated);
//! * a non-elevated backend (vJoy, driver present) *is* activated at
//!   startup, so autostart defaults to vJoy until `HIDMaestro` is
//!   ready — exactly LOGIC-FIX-PLAN §4.6 option (b) + (c).
//! * the optional elevated helper (scheduled task, option (a)) is
//!   NOT implemented: the deferral above already removes the
//!   login-time prompt, and a scheduled task would be a second
//!   autostart mechanism to keep in sync.
//!
//! # Backends
//!
//! * Windows: `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`,
//!   value `RSH`, via the Registry API directly — never `reg.exe`
//!   (the v1.0.2 `reg.exe` console flash). Implemented here as
//!   [`RegRunKeyStore`]; the repair/disable logic itself is the
//!   tested `rsh-app-core::system` code over the [`RunKeyStore`]
//!   trait, so it is unit-testable on Linux with a fake store.
//! * Linux (sandbox parity + Linux installs): a
//!   `~/.config/autostart/rsh-daemon.desktop` file
//!   (`X-GNOME-Autostart-enabled=true`, `Hidden=false`).

use std::path::Path;
#[cfg(not(windows))]
use std::path::PathBuf;

#[cfg(any(windows, test))]
use rsh_app_core::system::RunKeyStore;
#[cfg(any(windows, test))]
use rsh_app_core::system::SystemError;
#[cfg(windows)]
use rsh_app_core::system::{
    AUTOSTART_VALUE_NAME, autostart_command, disable_autostart, repair_autostart,
};

/// The Run-key subkey, relative to `HKEY_CURRENT_USER`.
#[cfg(windows)]
pub const RUN_KEY_SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// Linux autostart desktop file name.
#[cfg(not(windows))]
pub const DESKTOP_FILE_NAME: &str = "rsh-daemon.desktop";

/// What [`sync_autostart`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutostartSync {
    /// An entry now points at `exe`.
    Enabled,
    /// No entry exists (and none was created).
    Disabled,
    /// The entry already pointed at `exe`; nothing changed.
    AlreadyCorrect,
    /// A stale entry was rewritten to `exe`.
    #[cfg(windows)]
    Repaired,
}

/// Whether autostart is currently enabled for `exe`.
///
/// Windows: the `RSH` Run-key value exists and points at `exe`.
/// Linux: the `.desktop` file exists in the autostart dir.
#[must_use]
pub fn autostart_enabled(exe: &Path) -> bool {
    #[cfg(windows)]
    {
        let store = RegRunKeyStore::new();
        store.read_rsh().is_some_and(|command| {
            rsh_app_core::system::same_exe(&rsh_app_core::system::exe_from_command(&command), exe)
        })
    }
    #[cfg(not(windows))]
    {
        let _ = exe;
        autostart_dir().is_some_and(|dir| dir.join(DESKTOP_FILE_NAME).is_file())
    }
}

/// Bring the autostart entry in line with `enabled` for `exe`:
/// enable → create/repair the entry; disable → delete it.
/// Best-effort at the call site: the daemon logs the outcome and
/// keeps running when this fails (a failed autostart write must
/// never fail the launch).
///
/// # Errors
///
/// Returns [`String`] when the store cannot be read or written.
pub fn sync_autostart(enabled: bool, exe: &Path) -> Result<AutostartSync, String> {
    #[cfg(windows)]
    {
        let mut store = RegRunKeyStore::new();
        if enabled {
            return match repair_autostart(&mut store, exe).map_err(|error| error.to_string())? {
                rsh_app_core::system::AutostartRepair::AlreadyCorrect => {
                    Ok(AutostartSync::AlreadyCorrect)
                }
                rsh_app_core::system::AutostartRepair::Repaired { .. } => {
                    Ok(AutostartSync::Repaired)
                }
                rsh_app_core::system::AutostartRepair::NotPresent => {
                    store
                        .write_rsh(&autostart_command(exe))
                        .map_err(|error| error.to_string())?;
                    Ok(AutostartSync::Enabled)
                }
            };
        }
        disable_autostart(&mut store).map_err(|error| error.to_string())?;
        Ok(AutostartSync::Disabled)
    }
    #[cfg(not(windows))]
    {
        let Some(dir) = autostart_dir() else {
            return Err("no config dir for the Linux autostart file".to_string());
        };
        let file = dir.join(DESKTOP_FILE_NAME);
        if enabled {
            std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
            let contents = desktop_file_contents(exe);
            if file.is_file()
                && std::fs::read_to_string(&file).map_err(|error| error.to_string())? == contents
            {
                return Ok(AutostartSync::AlreadyCorrect);
            }
            std::fs::write(&file, contents).map_err(|error| error.to_string())?;
            Ok(AutostartSync::Enabled)
        } else {
            if file.is_file() {
                std::fs::remove_file(&file).map_err(|error| error.to_string())?;
            }
            Ok(AutostartSync::Disabled)
        }
    }
}

/// The Linux autostart directory (`~/.config/autostart`).
#[cfg(not(windows))]
fn autostart_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("autostart"))
}

/// Render the Linux autostart `.desktop` file for `exe`.
#[cfg(not(windows))]
#[must_use]
pub fn desktop_file_contents(exe: &Path) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=RSH Wheel\nComment=Real Sim Hub daemon — wheel, tray and auto profiles\nExec=\"{}\"\nIcon=rsh\nTerminal=false\nCategories=Utility;\nX-GNOME-Autostart-enabled=true\nHidden=false\n",
        exe.display()
    )
}

/// In-memory [`RunKeyStore`] for tests (and for the repair logic's
/// Linux test seam): the registry is Windows-only, the logic is not.
#[cfg(test)]
#[derive(Default)]
pub struct MemoryRunKeyStore {
    /// The stored command line, when present.
    pub command: Option<String>,
}

#[cfg(test)]
impl RunKeyStore for MemoryRunKeyStore {
    fn read_rsh(&self) -> Option<String> {
        self.command.clone()
    }

    fn write_rsh(&mut self, command: &str) -> Result<(), SystemError> {
        self.command = Some(command.to_string());
        Ok(())
    }

    fn delete_rsh(&mut self) -> Result<(), SystemError> {
        self.command = None;
        Ok(())
    }
}

/// The Windows Registry backend for the Run key.
///
/// Direct Registry API calls (`RegOpenKeyExW` / `RegSetValueExW` /
/// `RegDeleteValueW` / `RegQueryValueExW`): no `reg.exe` child
/// process, so no console flash on autostart repair (the v1.0.2
/// `reg.exe` flash bug class). `HKCU` needs no elevation.
#[cfg(windows)]
pub struct RegRunKeyStore;

#[cfg(windows)]
impl RegRunKeyStore {
    /// Build the store (stateless — every call opens the key).
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    fn open_read() -> Option<windows::Win32::System::Registry::HKEY> {
        use windows::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, KEY_READ, RegOpenKeyExW};
        use windows::core::HSTRING;
        let mut key = HKEY::default();
        let result = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                &HSTRING::from(RUN_KEY_SUBKEY),
                None,
                KEY_READ,
                &raw mut key,
            )
        };
        if result.0 == 0 { Some(key) } else { None }
    }

    fn open_write() -> Option<windows::Win32::System::Registry::HKEY> {
        use windows::Win32::System::Registry::{
            HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, RegOpenKeyExW,
        };
        use windows::core::HSTRING;
        let mut key = HKEY::default();
        let result = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                &HSTRING::from(RUN_KEY_SUBKEY),
                None,
                KEY_SET_VALUE,
                &raw mut key,
            )
        };
        if result.0 == 0 { Some(key) } else { None }
    }
}

#[cfg(windows)]
impl RunKeyStore for RegRunKeyStore {
    fn read_rsh(&self) -> Option<String> {
        use windows::Win32::System::Registry::{REG_SZ, RegCloseKey, RegQueryValueExW};
        use windows::core::HSTRING;
        let key = Self::open_read()?;
        let mut size_bytes = 0u32;
        let size_result = unsafe {
            RegQueryValueExW(
                key,
                &HSTRING::from(AUTOSTART_VALUE_NAME),
                None,
                None,
                None,
                Some(&raw mut size_bytes),
            )
        };
        if size_result.0 != 0 || size_bytes == 0 {
            unsafe {
                let _ = RegCloseKey(key);
            };
            return None;
        }
        let mut buffer = vec![0u16; (size_bytes as usize).div_ceil(2)];
        let mut value_type = REG_SZ;
        let mut read_bytes = size_bytes;
        let result = unsafe {
            RegQueryValueExW(
                key,
                &HSTRING::from(AUTOSTART_VALUE_NAME),
                None,
                Some(&raw mut value_type),
                Some(buffer.as_mut_ptr().cast::<u8>()),
                Some(&raw mut read_bytes),
            )
        };
        unsafe {
            let _ = RegCloseKey(key);
        };
        if result.0 != 0 || value_type != REG_SZ {
            return None;
        }
        let units = (read_bytes as usize) / 2;
        let text = String::from_utf16_lossy(&buffer[..units]);
        Some(text.trim_end_matches('\0').to_string())
    }

    fn write_rsh(&mut self, command: &str) -> Result<(), SystemError> {
        use windows::Win32::System::Registry::{REG_SZ, RegCloseKey, RegSetValueExW};
        use windows::core::HSTRING;
        let Some(key) = Self::open_write() else {
            return Err(SystemError::Store(
                "cannot open the Run key for writing".to_string(),
            ));
        };
        let wide: Vec<u16> = command.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes: &[u8] =
            unsafe { std::slice::from_raw_parts(wide.as_ptr().cast::<u8>(), wide.len() * 2) };
        let result = unsafe {
            RegSetValueExW(
                key,
                &HSTRING::from(AUTOSTART_VALUE_NAME),
                None,
                REG_SZ,
                Some(bytes),
            )
        };
        unsafe {
            let _ = RegCloseKey(key);
        };
        if result.0 == 0 {
            Ok(())
        } else {
            Err(SystemError::Store(format!(
                "cannot write the Run key value (Win32 error {})",
                result.0
            )))
        }
    }

    fn delete_rsh(&mut self) -> Result<(), SystemError> {
        use windows::Win32::System::Registry::{RegCloseKey, RegDeleteValueW};
        use windows::core::HSTRING;
        let Some(key) = Self::open_write() else {
            return Err(SystemError::Store(
                "cannot open the Run key for writing".to_string(),
            ));
        };
        let result = unsafe { RegDeleteValueW(key, &HSTRING::from(AUTOSTART_VALUE_NAME)) };
        unsafe {
            let _ = RegCloseKey(key);
        };
        // 2 = ERROR_FILE_NOT_FOUND: already absent is a successful
        // delete (the app-core contract: delete is a no-op when
        // absent).
        if result.0 == 0 || result.0 == 2 {
            Ok(())
        } else {
            Err(SystemError::Store(format!(
                "cannot delete the Run key value (Win32 error {})",
                result.0
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsh_app_core::system::{
        AutostartRepair, autostart_command, disable_autostart, repair_autostart,
    };

    #[test]
    #[cfg(not(windows))]
    fn desktop_file_renders_exec_and_autostart_flag() {
        let contents = desktop_file_contents(Path::new("/usr/bin/rsh-daemon"));
        assert!(contents.contains("Exec=\"/usr/bin/rsh-daemon\""));
        assert!(contents.contains("X-GNOME-Autostart-enabled=true"));
        assert!(contents.contains("Terminal=false"));
        assert!(contents.starts_with("[Desktop Entry]\nType=Application\n"));
    }

    #[test]
    fn memory_store_repair_rewrites_stale_v1_entry() {
        let mut store = MemoryRunKeyStore {
            command: Some("\"C:\\Program Files\\RSH\\RSH.exe\" --tray".to_string()),
        };
        let outcome = repair_autostart(
            &mut store,
            Path::new("C:\\Program Files\\RSH\\rsh-daemon.exe"),
        )
        .unwrap();
        assert!(matches!(outcome, AutostartRepair::Repaired { .. }));
        assert_eq!(
            store.command.as_deref(),
            Some("\"C:\\Program Files\\RSH\\rsh-daemon.exe\"")
        );
    }

    #[test]
    fn memory_store_repair_leaves_correct_entry_alone() {
        let mut store = MemoryRunKeyStore {
            command: Some("\"C:\\Program Files\\RSH\\rsh-daemon.exe\"".to_string()),
        };
        let outcome = repair_autostart(
            &mut store,
            Path::new("C:\\Program Files\\RSH\\rsh-daemon.exe"),
        )
        .unwrap();
        assert_eq!(outcome, AutostartRepair::AlreadyCorrect);
    }

    #[test]
    fn memory_store_disable_deletes_entry() {
        let mut store = MemoryRunKeyStore {
            command: Some("\"C:\\Program Files\\RSH\\rsh-daemon.exe\"".to_string()),
        };
        disable_autostart(&mut store).unwrap();
        assert_eq!(store.command, None);
    }

    #[test]
    fn autostart_command_quotes_the_exe_path() {
        assert_eq!(
            autostart_command(Path::new("C:\\Program Files\\RSH\\rsh-daemon.exe")),
            "\"C:\\Program Files\\RSH\\rsh-daemon.exe\""
        );
    }
}
