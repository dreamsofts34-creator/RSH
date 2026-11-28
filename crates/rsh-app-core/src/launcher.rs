//! Manual-game launch (game-scanner removal, v2.0.3+).
//!
//! The automatic game scanner is gone (user decision 2026-11-18):
//! no Steam/iRacing catalogue, no registry reads, no `tasklist`,
//! no filesystem detection of any kind. The user adds games by hand
//! (see [`crate::manual_games`]) and this module only **launches**
//! them and tracks the running one.
//!
//! * Launch spawns the stored exe — or a shortcut (`.lnk`, via
//!   `ShellExecuteExW`) — directly and tracks the child pid.
//!   Exactly one game runs at a time. Stop kills **only** the
//!   tracked pid (SEC-E-10: never kill another same-name process).
//! * The child runs with the **game's own folder** as its working
//!   directory (v2.0.13: the daemon's cwd is never inherited —
//!   most games fail to start, or exit at once, when started from
//!   a foreign folder, which read as a silent no-op launch).
//!   For an exe that is the exe's parent dir; for a `.lnk` the
//!   shortcut target's parent dir (the target is read out of the
//!   shortcut — see [`resolve_shortcut_target`]).
//! * A game that starts and exits at once is a real error, not a
//!   silent "not running" (settle check in [`GameLauncher::launch`]);
//!   a shortcut helper that spawns the real game and exits is not
//!   mistaken for that — the first refreshes after launch keep the
//!   game running (launch grace in [`GameLauncher::refresh`]).
//! * Spawn uses null stdio + detached, no-window creation (DS-47):
//!   a game that inherits the daemon's stdio pipes can block the
//!   parent forever (the NSIS `Exec` hang class fixed in v2.0.2),
//!   and a console window must never flash up for a GUI game.
//!
//! # Windows-only gaps (this Linux sandbox cannot verify them)
//!
//! * The real launch on Windows spawns a Windows exe; the
//!   [`SystemSpawner`] is only exercised here through the fake
//!   spawner in tests. Real-launch verification is a Windows-PC
//!   item (gate).
//! * A shortcut's target helper (e.g. AC's `acs.exe`) spawns as a
//!   *child* of the game: stopping the tracked pid does not
//!   enumerate children (that needs `Toolhelp32Snapshot` on
//!   Windows). If the helper outlives the game, the app re-attaches
//!   on Refresh and stops it separately — the C# behaviour, not yet
//!   implemented here (open item for Windows CI).

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long after a launch the tracked pid is given the benefit
/// of the doubt: a shortcut helper spawns the real game and exits
/// at once, and a game can take a moment to get going — the first
/// refreshes inside this window never clear a just-launched game.
const LAUNCH_GRACE: Duration = Duration::from_secs(5);

/// How long after the launch grace expires a cleared record still
/// counts as "closed immediately" (F2, v2.0.21): a game that is
/// gone within grace + this window failed to start and the UI must
/// say so; a game the user played and then closed is a normal
/// exit and clears silently.
const CLOSED_IMMEDIATELY_WINDOW_SECS: u64 = 30;

/// Unix seconds now (0 on a clock failure — never panics).
fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

/// True when two paths name the same file. Windows paths compare
/// case-insensitively (the pid image query can return different
/// casing than the stored path); elsewhere the comparison is exact.
fn paths_equal(a: &Path, b: &Path) -> bool {
    #[cfg(windows)]
    {
        a.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&b.as_os_str().to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        a == b
    }
}

/// Why a launch failed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LaunchError {
    /// The game has no exe path (empty path stored on the entry).
    NotInstalled(String),
    /// Another game is already running (one game at a time).
    AlreadyRunning(String),
    /// The process could not be spawned.
    SpawnFailed(String),
    /// Launching is not supported on this platform.
    UnsupportedPlatform(String),
}

impl fmt::Display for LaunchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInstalled(game) => write!(f, "game {game} is not installed"),
            Self::AlreadyRunning(game) => {
                write!(f, "game {game} is already running (one game at a time)")
            }
            Self::SpawnFailed(detail) => write!(f, "failed to launch game: {detail}"),
            Self::UnsupportedPlatform(detail) => write!(f, "launch not supported: {detail}"),
        }
    }
}

impl std::error::Error for LaunchError {}

/// A running game tracked by the launcher.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RunningGame {
    /// Game id (the manual entry's slug id).
    pub game_id: String,
    /// OS process id of the tracked game process.
    pub pid: u32,
    /// Full path to the exe (or shortcut) that was launched.
    pub exe_path: String,
    /// The exe image the tracked pid actually runs, resolved at
    /// launch time (R3-D-04): for a `.lnk` launch this is the
    /// shortcut's target exe (`QueryFullProcessImageNameW` on the
    /// spawned pid), which never equals the stored `.lnk` path —
    /// the pid-reuse guard must compare against this, not the
    /// stored path. [`None`] when the image could not be read at
    /// launch (the guard then falls back to `exe_path`, and to
    /// liveness-only for unresolved shortcuts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_exe_path: Option<String>,
    /// The exe image the game itself is expected to run as (F2,
    /// v2.0.21): the stored exe for a plain launch, the shortcut's
    /// target exe for a `.lnk` launch. This is what the post-grace
    /// verification searches for — `resolved_exe_path` is the
    /// image the *tracked pid* runs, which is a helper's image
    /// when the tracked pid is a launcher stub, so it cannot be
    /// the search key. [`None`] on records built before v2.0.21
    /// (the search then falls back to `exe_path` for plain
    /// launches, and is skipped for shortcuts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_exe_path: Option<String>,
    /// Unix seconds when the game was launched.
    pub started_unix_secs: u64,
}

/// Spawns and supervises game processes. The production implementation
/// is [`SystemSpawner`]; tests inject a fake.
pub trait ProcessSpawner {
    /// Spawns an exe (or shortcut) and returns its pid.
    ///
    /// # Errors
    ///
    /// Returns an error message when the process could not be started.
    fn spawn(&mut self, exe: &Path) -> Result<u32, String>;

    /// Spawns an exe (or shortcut) with `cwd` as the child's
    /// working directory and returns its pid (v2.0.13: a game must
    /// start in its own folder, never in the daemon's). [`None`]
    /// inherits the parent's directory. The default delegates to
    /// [`ProcessSpawner::spawn`] (spawners that cannot set a
    /// working directory keep the old behaviour).
    ///
    /// # Errors
    ///
    /// Returns an error message when the process could not be started.
    fn spawn_in(&mut self, exe: &Path, cwd: Option<&Path>) -> Result<u32, String> {
        let _ = cwd;
        self.spawn(exe)
    }

    /// True when the pid is still alive.
    fn is_alive(&self, pid: u32) -> bool;

    /// Terminates a pid.
    ///
    /// # Errors
    ///
    /// Returns an error message when the process could not be terminated.
    fn terminate(&mut self, pid: u32) -> Result<(), String>;

    /// Full path of the exe image for a pid, when the spawner can
    /// determine it (L10: binds the tracked record to the actual
    /// process, so a reused pid is not mistaken for the game).
    /// The default returns `None` — a spawner that cannot answer
    /// leaves the pid-only liveness check unchanged.
    fn exe_path(&self, _pid: u32) -> Option<PathBuf> {
        None
    }

    /// Exit code of a pid that has already died, when the spawner
    /// can determine it (F2, v2.0.21): `None` when the pid is still
    /// alive, the code is unreadable, or the spawner cannot answer.
    /// A nonzero code proves the game itself failed to start — the
    /// settle check uses it when the dead pid's image could not be
    /// read at all (the process died between spawn and the first
    /// image query). The default returns `None` — a spawner that
    /// cannot answer leaves the launch checks unchanged.
    fn exit_code(&self, _pid: u32) -> Option<u32> {
        None
    }

    /// Pid of a live process running the given exe image, when the
    /// spawner can determine it (F2 post-grace verification: the
    /// tracked pid may be a helper that exited while the real game
    /// runs under a different pid — Store apps and launcher stubs
    /// do exactly this). The default returns `None` — a spawner
    /// that cannot answer leaves the tracked-pid-only checks
    /// unchanged.
    fn find_process_with_image(&self, _exe: &Path) -> Option<u32> {
        None
    }
}

#[cfg(windows)]
mod win32 {
    //! Minimal raw Win32 FFI for process liveness / termination and
    //! shortcut launching. (Same rationale as the watcher module: no
    //! `windows`-crate dependency yet; the SW-R7.1 spike may replace
    //! this.)

    /// `HANDLE` as an integer-sized value.
    pub type Handle = isize;

    unsafe extern "C" {
        /// Opens a process handle, or 0 on failure.
        pub fn OpenProcess(desired_access: u32, inherit_handle: i32, pid: u32) -> Handle;
        /// Writes the exit code; returns non-zero on success.
        pub fn GetExitCodeProcess(process: Handle, exit_code: *mut u32) -> i32;
        /// Terminates a process; returns non-zero on success.
        pub fn TerminateProcess(process: Handle, exit_code: u32) -> i32;
        /// Writes the full image path; returns non-zero on success.
        pub fn QueryFullProcessImageNameW(
            process: Handle,
            flags: u32,
            exe_name: *mut u16,
            size: *mut u32,
        ) -> i32;
        /// Closes a handle; returns non-zero on success.
        pub fn CloseHandle(handle: Handle) -> i32;
        /// Launches a file with its registered handler (used for
        /// `.lnk` shortcuts, which `CreateProcess` cannot start);
        /// returns non-zero on success.
        pub fn ShellExecuteExW(info: *mut ShellExecuteInfoW) -> i32;
        /// Returns the pid for a process handle, or 0 on failure.
        pub fn GetProcessId(process: Handle) -> u32;
        /// Takes a snapshot of the processes; returns
        /// `INVALID_HANDLE_VALUE` (-1) on failure.
        pub fn CreateToolhelp32Snapshot(dw_flags: u32, th32_process_id: u32) -> Handle;
        /// Reads the first process from a snapshot; returns
        /// non-zero on success.
        pub fn Process32FirstW(snapshot: Handle, entry: *mut ProcessEntry32W) -> i32;
        /// Reads the next process from a snapshot; returns
        /// non-zero on success.
        pub fn Process32NextW(snapshot: Handle, entry: *mut ProcessEntry32W) -> i32;
    }

    /// `TH32CS_SNAPPROCESS` (0x2): snapshot all processes.
    pub const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;
    /// `INVALID_HANDLE_VALUE` (-1): failed snapshot handle.
    pub const INVALID_HANDLE_VALUE: Handle = -1;

    /// `PROCESSENTRY32W` (partial: only `dwSize` and
    /// `th32ProcessID` are read; `szExeFile` is present so the
    /// layout matches the real struct).
    #[repr(C)]
    pub struct ProcessEntry32W {
        /// Size of this struct, in bytes (must be set by the caller).
        pub dw_size: u32,
        /// Usage count (unused).
        pub cnt_usage: u32,
        /// Process id.
        pub th32_process_id: u32,
        /// Default heap id (unused).
        pub th32_default_heap_id: usize,
        /// Module id (unused).
        pub th32_module_id: u32,
        /// Thread count (unused).
        pub cnt_threads: u32,
        /// Parent process id (unused).
        pub th32_parent_process_id: u32,
        /// Base priority (unused).
        pub pc_pri_class_base: i32,
        /// Flags (unused).
        pub dw_flags: u32,
        /// Exe file name (unused — the image is queried per pid).
        pub sz_exe_file: [u16; 260],
    }

    /// `PROCESS_QUERY_LIMITED_INFORMATION` (0x1000).
    pub const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    /// `PROCESS_TERMINATE` (0x0001).
    pub const PROCESS_TERMINATE: u32 = 0x0001;
    /// `STILL_ACTIVE` (259).
    pub const STILL_ACTIVE: u32 = 259;
    /// `SEE_MASK_NOCLOSEPROCESS` (0x40): return the child process
    /// handle in `h_process`, so the launcher can track its pid.
    pub const SEE_MASK_NOCLOSEPROCESS: u32 = 0x0000_0040;
    /// `SW_SHOWNORMAL` (1): show the launched app normally.
    pub const SW_SHOWNORMAL: i32 = 1;

    /// `SHELLEXECUTEINFOW` (partial: fields up to `h_process` are
    /// the ones this module reads; the struct is `repr(C)` and the
    /// size is set from the full layout, so later fields are
    /// zero-initialised padding).
    #[repr(C)]
    pub struct ShellExecuteInfoW {
        /// Size of this struct, in bytes.
        pub cb_size: u32,
        /// Flags (`SEE_MASK_*`).
        pub f_mask: u32,
        /// Parent window handle (`0` = none).
        pub hwnd: Handle,
        /// Verb (`null` = the default "open" verb).
        pub lp_verb: *const u16,
        /// File to open (the shortcut path, UTF-16, NUL-terminated).
        pub lp_file: *const u16,
        /// Parameters (`null`).
        pub lp_parameters: *const u16,
        /// Working directory (`null` = the caller's current
        /// directory — never leave this null for a game launch).
        pub lp_directory: *const u16,
        /// Show command (`SW_SHOWNORMAL`).
        pub n_show: i32,
        /// Receives the `HINSTANCE` result (unused here).
        pub h_inst_app: Handle,
        /// Item-id list (`null`).
        pub lp_id_list: *mut core::ffi::c_void,
        /// Class name (`null`).
        pub lp_class: *const u16,
        /// Class key (`0`).
        pub hkey_class: Handle,
        /// Hot key (`0`).
        pub dw_hot_key: u32,
        /// Icon (`0`).
        pub h_icon: Handle,
        /// Receives the child process handle (with
        /// `SEE_MASK_NOCLOSEPROCESS`).
        pub h_process: Handle,
    }
}

/// True when `path` is a Windows shortcut (`.lnk`, any case).
#[must_use]
pub fn is_shortcut(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("lnk"))
}

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    bytes
        .get(offset..offset + 2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    bytes
        .get(offset..offset + 4)
        .map(|quad| u32::from_le_bytes([quad[0], quad[1], quad[2], quad[3]]))
}

/// Reads a NUL-terminated ANSI string at `offset` (relative to
/// `base`), or [`None`] when the offset is out of bounds.
fn read_cstr(bytes: &[u8], base: usize, offset: u32) -> Option<String> {
    let start = base.checked_add(usize::try_from(offset).ok()?)?;
    let rest = bytes.get(start..)?;
    let end = rest.iter().position(|byte| *byte == 0)?;
    Some(String::from_utf8_lossy(&rest[..end]).into_owned())
}

/// Reads a NUL-terminated UTF-16 string at `offset` (relative to
/// `base`), or [`None`] when the offset is out of bounds.
fn read_wstr(bytes: &[u8], base: usize, offset: u32) -> Option<String> {
    let start = base.checked_add(usize::try_from(offset).ok()?)?;
    let mut units = Vec::new();
    let mut at = start;
    loop {
        let unit = read_u16(bytes, at)?;
        if unit == 0 {
            break;
        }
        units.push(unit);
        at = at.checked_add(2)?;
    }
    Some(String::from_utf16_lossy(&units))
}

/// Resolves a Windows shortcut (`.lnk`) to the target it points at
/// (v2.0.13: the launch working directory is the *target's* folder,
/// and a broken shortcut is a real error, not a silent no-op).
///
/// Pure-Rust reader of the Shell Link binary format (MS-SHLLINK):
/// 76-byte header, optional `LinkTargetIDList`, then `LinkInfo`
/// with `LocalBasePath` (Unicode form preferred when present).
/// Relative targets resolve against the shortcut's own folder.
///
/// # Errors
///
/// Returns a message when the file is not a shortcut, is truncated,
/// or carries no readable target — the caller turns it into a
/// launch error that names the shortcut.
pub fn resolve_shortcut_target(shortcut: &Path) -> Result<PathBuf, String> {
    let bytes = std::fs::read(shortcut)
        .map_err(|error| format!("couldn't read shortcut '{}': {error}", shortcut.display()))?;
    // Header: 76 bytes, signature 0x0000004C, LinkFlags at +20.
    if bytes.len() < 76 {
        return Err(format!(
            "shortcut '{}' is too small to be a shortcut ({} bytes)",
            shortcut.display(),
            bytes.len()
        ));
    }
    if read_u32(&bytes, 0) != Some(0x0000_004C) {
        return Err(format!(
            "shortcut '{}' is not a Windows shortcut (bad header)",
            shortcut.display()
        ));
    }
    let flags = read_u32(&bytes, 20).unwrap_or(0);
    let has_id_list = flags & 0x0000_0001 != 0;
    let has_link_info = flags & 0x0000_0002 != 0;
    let mut offset = 76_usize;
    if has_id_list {
        let id_list_size = usize::from(read_u16(&bytes, offset).unwrap_or(0));
        offset = offset
            .checked_add(2)
            .and_then(|at| at.checked_add(id_list_size))
            .ok_or_else(|| {
                format!(
                    "shortcut '{}' has a corrupt target list",
                    shortcut.display()
                )
            })?;
    }
    if !has_link_info {
        return Err(format!(
            "shortcut '{}' has no target information",
            shortcut.display()
        ));
    }
    let info_size = read_u32(&bytes, offset).unwrap_or(0);
    let header_size = read_u32(&bytes, offset + 4).unwrap_or(0);
    if info_size < 28 || header_size < 28 {
        return Err(format!(
            "shortcut '{}' has a corrupt target block",
            shortcut.display()
        ));
    }
    let end = offset
        .checked_add(usize::try_from(info_size).unwrap_or(usize::MAX))
        .ok_or_else(|| {
            format!(
                "shortcut '{}' has a corrupt target block",
                shortcut.display()
            )
        })?;
    if end > bytes.len() {
        return Err(format!(
            "shortcut '{}' is truncated (target block runs past the end)",
            shortcut.display()
        ));
    }
    linkinfo_target(&bytes, offset, header_size, shortcut)
}

/// Reads the target out of a validated `LinkInfo` block starting
/// at `offset` (`header_size` from the block itself).
fn linkinfo_target(
    bytes: &[u8],
    offset: usize,
    header_size: u32,
    shortcut: &Path,
) -> Result<PathBuf, String> {
    // LinkInfo offsets (relative to the LinkInfo start): +12
    // VolumeIDOffset, +16 LocalBasePath, +20
    // CommonNetworkRelativeLinkOffset, +24 CommonPathSuffix, +28/+32
    // the Unicode forms (header size >= 36).
    let unicode_base = if header_size >= 36 {
        read_u32(bytes, offset + 28)
            .filter(|at| *at != 0)
            .and_then(|at| read_wstr(bytes, offset, at))
            .filter(|path| !path.is_empty())
    } else {
        None
    };
    let base = unicode_base.or_else(|| {
        read_u32(bytes, offset + 16)
            .filter(|at| *at != 0)
            .and_then(|at| read_cstr(bytes, offset, at))
            .filter(|path| !path.is_empty())
    });
    if let Some(target) = base {
        let target = PathBuf::from(target);
        if target.is_absolute() {
            return Ok(target);
        }
        if let Some(parent) = shortcut.parent() {
            return Ok(parent.join(target));
        }
        return Ok(target);
    }
    // No base path — a suffix alone is relative to the shortcut.
    let suffix = if header_size >= 36 {
        read_u32(bytes, offset + 32)
            .filter(|at| *at != 0)
            .and_then(|at| read_wstr(bytes, offset, at))
    } else {
        None
    }
    .or_else(|| {
        read_u32(bytes, offset + 24)
            .filter(|at| *at != 0)
            .and_then(|at| read_cstr(bytes, offset, at))
    })
    .filter(|path| !path.is_empty());
    if let Some(suffix) = suffix {
        if let Some(parent) = shortcut.parent() {
            return Ok(parent.join(suffix));
        }
        return Ok(PathBuf::from(suffix));
    }
    Err(format!(
        "shortcut '{}' has no target path in it",
        shortcut.display()
    ))
}

/// Spawns real OS processes.
///
/// * Spawn uses [`std::process::Command`] (works on Windows and Linux).
///   Shortcuts (`.lnk`) go through `ShellExecuteExW` instead —
///   `CreateProcess` cannot start a shortcut — with
///   `SEE_MASK_NOCLOSEPROCESS`, so the child pid is tracked exactly
///   like a direct exe launch.
/// * Liveness/termination use raw Win32 FFI on Windows and `/proc` +
///   the `kill` command elsewhere.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemSpawner;

impl SystemSpawner {
    /// Launches a shortcut (`.lnk`) via `ShellExecuteExW` and
    /// returns the child pid. Windows-only. `cwd` is the child's
    /// working directory (v2.0.13: the shortcut target's folder —
    /// `lp_directory: null` left the game in the daemon's folder,
    /// where most games fail to start or exit at once).
    #[cfg(windows)]
    fn spawn_shortcut(exe: &Path, cwd: Option<&Path>) -> Result<u32, String> {
        use std::os::windows::ffi::OsStrExt;
        // SAFETY: plain Win32 call with checked returns; the path
        // and directory buffers are valid for the call, the info
        // struct is fully initialised, and the process handle is
        // always closed.
        unsafe {
            let file_wide: Vec<u16> = exe
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let dir_wide: Option<Vec<u16>> = cwd.map(|dir| {
                dir.as_os_str()
                    .encode_wide()
                    .chain(std::iter::once(0))
                    .collect()
            });
            let mut info = win32::ShellExecuteInfoW {
                cb_size: u32::try_from(std::mem::size_of::<win32::ShellExecuteInfoW>())
                    .unwrap_or(u32::MAX),
                f_mask: win32::SEE_MASK_NOCLOSEPROCESS,
                hwnd: 0,
                lp_verb: std::ptr::null(),
                lp_file: file_wide.as_ptr(),
                lp_parameters: std::ptr::null(),
                lp_directory: dir_wide
                    .as_ref()
                    .map_or(std::ptr::null(), std::vec::Vec::as_ptr),
                n_show: win32::SW_SHOWNORMAL,
                h_inst_app: 0,
                lp_id_list: std::ptr::null_mut(),
                lp_class: std::ptr::null(),
                hkey_class: 0,
                dw_hot_key: 0,
                h_icon: 0,
                h_process: 0,
            };
            let ok = win32::ShellExecuteExW(&raw mut info);
            if ok == 0 {
                return Err(format!("failed to launch shortcut '{}'", exe.display()));
            }
            if info.h_process == 0 {
                return Err(format!(
                    "shortcut '{}' launched but returned no process",
                    exe.display()
                ));
            }
            let pid = win32::GetProcessId(info.h_process);
            win32::CloseHandle(info.h_process);
            if pid == 0 {
                return Err(format!(
                    "shortcut '{}' launched but its pid could not be read",
                    exe.display()
                ));
            }
            Ok(pid)
        }
    }
}

impl ProcessSpawner for SystemSpawner {
    fn spawn(&mut self, exe: &Path) -> Result<u32, String> {
        self.spawn_in(exe, None)
    }

    fn spawn_in(&mut self, exe: &Path, cwd: Option<&Path>) -> Result<u32, String> {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            if is_shortcut(exe) {
                return Self::spawn_shortcut(exe, cwd);
            }
            // DS-47: null stdio + detached, no-window creation —
            // mirroring the daemon's `spawn_ui` (F8). A game that
            // inherits the daemon's stdio pipes can block the parent
            // forever (the NSIS `Exec` hang class fixed in v2.0.2),
            // and a console window must never flash up for a GUI
            // game. Flags are the raw Win32 values, as elsewhere in
            // this crate (`state.rs`): DETACHED_PROCESS 0x8,
            // CREATE_NO_WINDOW 0x0800_0000.
            // v2.0.13: the child runs in the game's own folder
            // (`current_dir`) — without it most games fail to start
            // or exit at once, which read as a silent no-op launch.
            let mut command = std::process::Command::new(exe);
            command
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .creation_flags(0x0000_0008 | 0x0800_0000);
            if let Some(dir) = cwd {
                command.current_dir(dir);
            }
            command
                .spawn()
                .map(|child| child.id())
                .map_err(|error| error.to_string())
        }
        #[cfg(not(windows))]
        {
            // The sandbox has no Windows games; spawning a Windows exe
            // here fails at the OS level and that failure is reported,
            // never hidden.
            let _ = (exe, cwd);
            Err("game launch is Windows-only".to_owned())
        }
    }

    fn is_alive(&self, pid: u32) -> bool {
        #[cfg(windows)]
        {
            // SAFETY: plain Win32 queries with checked returns; the
            // handle is always closed.
            unsafe {
                let handle = win32::OpenProcess(win32::PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
                if handle == 0 {
                    return false;
                }
                let mut exit_code: u32 = 0;
                let ok = win32::GetExitCodeProcess(handle, &raw mut exit_code);
                win32::CloseHandle(handle);
                ok != 0 && exit_code == win32::STILL_ACTIVE
            }
        }
        #[cfg(not(windows))]
        {
            Path::new("/proc").join(pid.to_string()).is_dir()
        }
    }

    fn terminate(&mut self, pid: u32) -> Result<(), String> {
        #[cfg(windows)]
        {
            // SAFETY: plain Win32 call with a checked return; the
            // handle is always closed.
            unsafe {
                let handle = win32::OpenProcess(win32::PROCESS_TERMINATE, 0, pid);
                if handle == 0 {
                    return Err(format!("cannot open process {pid} to terminate it"));
                }
                let ok = win32::TerminateProcess(handle, 0);
                win32::CloseHandle(handle);
                if ok == 0 {
                    return Err(format!("failed to terminate process {pid}"));
                }
                Ok(())
            }
        }
        #[cfg(not(windows))]
        {
            let status = std::process::Command::new("kill")
                .arg(pid.to_string())
                .status()
                .map_err(|error| error.to_string())?;
            if status.success() {
                Ok(())
            } else {
                Err(format!("kill {pid} failed"))
            }
        }
    }

    fn exe_path(&self, pid: u32) -> Option<PathBuf> {
        #[cfg(windows)]
        {
            // SAFETY: plain Win32 query with checked returns; the
            // handle and the stack buffer are always valid.
            unsafe {
                let handle = win32::OpenProcess(win32::PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
                if handle == 0 {
                    return None;
                }
                let mut buffer = [0_u16; 4096];
                let mut size = u32::try_from(buffer.len()).unwrap_or(u32::MAX);
                let ok = win32::QueryFullProcessImageNameW(
                    handle,
                    0,
                    buffer.as_mut_ptr(),
                    &raw mut size,
                );
                win32::CloseHandle(handle);
                if ok == 0 {
                    return None;
                }
                let len = usize::try_from(size).unwrap_or(0);
                Some(PathBuf::from(String::from_utf16_lossy(
                    &buffer[..len.min(buffer.len())],
                )))
            }
        }
        #[cfg(not(windows))]
        {
            std::fs::read_link(Path::new("/proc").join(pid.to_string()).join("exe")).ok()
        }
    }

    fn exit_code(&self, pid: u32) -> Option<u32> {
        #[cfg(windows)]
        {
            // SAFETY: plain Win32 query with checked returns; the
            // handle is always closed. `STILL_ACTIVE` (259) means
            // the process is alive — that is "no code", not a code.
            unsafe {
                let handle = win32::OpenProcess(win32::PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
                if handle == 0 {
                    return None;
                }
                let mut code: u32 = 0;
                let ok = win32::GetExitCodeProcess(handle, &raw mut code);
                win32::CloseHandle(handle);
                if ok == 0 || code == win32::STILL_ACTIVE {
                    None
                } else {
                    Some(code)
                }
            }
        }
        #[cfg(not(windows))]
        {
            // `/proc/<pid>/stat`: `pid (comm) state … exit_code`
            // (field 52, last). Only a zombie (`Z`) has a real
            // code — a live process reports 0, which is "no code".
            let stat =
                std::fs::read_to_string(Path::new("/proc").join(pid.to_string()).join("stat"))
                    .ok()?;
            let after_comm = stat.rfind(')')?;
            let fields: Vec<&str> = stat[after_comm + 1..].split_whitespace().collect();
            // fields[0] is the state (field 3); exit_code is
            // field 52, i.e. index 49 after the closing paren.
            if fields.first() != Some(&"Z") {
                return None;
            }
            let code: i32 = fields.get(49)?.parse().ok()?;
            u32::try_from(code).ok()
        }
    }

    fn find_process_with_image(&self, exe: &Path) -> Option<u32> {
        #[cfg(windows)]
        {
            // SAFETY: plain Win32 snapshot enumeration with checked
            // returns; every handle is closed. The snapshot lists
            // pids; each pid's image is queried and compared
            // case-insensitively (see `paths_equal`).
            unsafe {
                let snapshot = win32::CreateToolhelp32Snapshot(win32::TH32CS_SNAPPROCESS, 0);
                if snapshot == win32::INVALID_HANDLE_VALUE {
                    return None;
                }
                let mut entry = win32::ProcessEntry32W {
                    dw_size: u32::try_from(std::mem::size_of::<win32::ProcessEntry32W>())
                        .unwrap_or(u32::MAX),
                    cnt_usage: 0,
                    th32_process_id: 0,
                    th32_default_heap_id: 0,
                    th32_module_id: 0,
                    cnt_threads: 0,
                    th32_parent_process_id: 0,
                    pc_pri_class_base: 0,
                    dw_flags: 0,
                    sz_exe_file: [0; 260],
                };
                let mut found = None;
                let mut ok = win32::Process32FirstW(snapshot, &raw mut entry);
                while ok != 0 {
                    let pid = entry.th32_process_id;
                    if pid != 0 {
                        if let Some(image) = self.exe_path(pid) {
                            if paths_equal(&image, exe) {
                                found = Some(pid);
                                break;
                            }
                        }
                    }
                    ok = win32::Process32NextW(snapshot, &raw mut entry);
                }
                win32::CloseHandle(snapshot);
                found
            }
        }
        #[cfg(not(windows))]
        {
            // `/proc` scan: the exe symlink of each numeric pid.
            let entries = std::fs::read_dir("/proc").ok()?;
            for entry in entries.flatten() {
                let name = entry.file_name();
                let Ok(pid) = name.to_string_lossy().parse::<u32>() else {
                    continue;
                };
                if let Some(image) = self.exe_path(pid)
                    && paths_equal(&image, exe)
                {
                    return Some(pid);
                }
            }
            None
        }
    }
}

/// Outcome of one post-grace verification poll (F2, v2.0.21) —
/// see [`GameLauncher::post_grace_poll`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PostGrace {
    /// The game runs: the tracked pid is alive, or the record
    /// re-attached to the real game's pid.
    Running,
    /// Still inside the launch grace: the tracked pid is dead but
    /// the real game may still appear — poll again.
    Waiting,
    /// The grace expired and no live process runs the expected
    /// image (carries the detail for the `game_closed_immediately`
    /// error).
    ClosedImmediately(String),
}

/// Launches and tracks games: exactly one at a time.
///
/// The launcher owns no profile state — it only reports which game is
/// running. The watcher / app layer binds the game's profile (see
/// [`crate::manual_games::ManualGame::bound_profile_id`]).
pub struct GameLauncher<S: ProcessSpawner> {
    spawner: S,
    running: Option<RunningGame>,
    /// How long to watch a just-spawned pid before reporting the
    /// launch as a success (v2.0.13): a game that starts and exits
    /// at once is a real error, never a silent "not running".
    settle: Duration,
    /// How long after a launch a dead pid is given the benefit of
    /// the doubt (see [`LAUNCH_GRACE`]).
    grace: Duration,
    /// A game the post-grace check found closed at once (F2,
    /// v2.0.21): `(game_id, detail)` — drained by the app layer
    /// via [`Self::take_closed_immediately`] so the UI can show
    /// the `game_closed_immediately` error instead of the tile
    /// silently flipping back to "not running".
    closed_immediately: Option<(String, String)>,
    /// Human-readable notes from the last launch / refresh (F2):
    /// exit codes, re-attachments. Drained via
    /// [`Self::take_launch_notes`] into the daemon log.
    launch_notes: Vec<String>,
}

impl<S: ProcessSpawner> GameLauncher<S> {
    /// Creates a launcher over a spawner (1.5 s settle, 5 s grace).
    #[must_use]
    pub fn new(spawner: S) -> Self {
        Self::with_timing(spawner, Duration::from_millis(1500), LAUNCH_GRACE)
    }

    /// Creates a launcher with an explicit settle / grace (tests
    /// use zero for both when they check launch / stop / refresh
    /// semantics without waiting).
    #[must_use]
    pub fn with_timing(spawner: S, settle: Duration, grace: Duration) -> Self {
        Self {
            spawner,
            running: None,
            settle,
            grace,
            closed_immediately: None,
            launch_notes: Vec::new(),
        }
    }

    /// Creates a launcher with an explicit settle and the default
    /// grace (tests that check the immediate-exit error).
    #[must_use]
    pub fn with_settle(spawner: S, settle: Duration) -> Self {
        Self::with_timing(spawner, settle, LAUNCH_GRACE)
    }

    /// The currently tracked running game, if any.
    #[must_use]
    pub fn running(&self) -> Option<&RunningGame> {
        self.running.as_ref()
    }

    /// Drains the pending closed-immediately report (F2): the
    /// `(game_id, detail)` the post-grace check recorded when a
    /// just-launched game turned out to have no live process at
    /// all. [`None`] when nothing was recorded since the last drain.
    #[must_use]
    pub fn take_closed_immediately(&mut self) -> Option<(String, String)> {
        self.closed_immediately.take()
    }

    /// Drains the launch / refresh notes since the last drain (F2):
    /// exit codes, re-attachments — the app layer logs these.
    #[must_use]
    pub fn take_launch_notes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.launch_notes)
    }

    /// True when a game is tracked AND its pid is already dead
    /// (F2): the launch succeeded only because the pid is a helper
    /// or its image was unreadable — the app layer waits out the
    /// grace for the real game to appear instead of reporting
    /// success for a game that never runs.
    #[must_use]
    pub fn tracked_pid_dead(&self) -> bool {
        self.running
            .as_ref()
            .is_some_and(|running| !self.spawner.is_alive(running.pid))
    }

    /// One post-grace verification poll (F2): no sleeping, so the
    /// caller can poll on its own cadence without holding the
    /// launcher lock across the wait. Inside the grace window a
    /// dead tracked pid is still [`PostGrace::Waiting`]; once the
    /// grace expires this reuses [`Self::refresh`]'s verification
    /// (re-attach to the real game, or clear + record
    /// closed-immediately).
    #[must_use]
    pub fn post_grace_poll(&mut self) -> PostGrace {
        let Some(running) = self.running.as_ref() else {
            return PostGrace::ClosedImmediately("no game is running".to_string());
        };
        if self.spawner.is_alive(running.pid) {
            return PostGrace::Running;
        }
        let now = now_unix_secs();
        if now
            < running
                .started_unix_secs
                .saturating_add(self.grace.as_secs())
        {
            return PostGrace::Waiting;
        }
        match self.refresh() {
            Some(_) => PostGrace::Running,
            None => match self.take_closed_immediately() {
                Some((_, detail)) => PostGrace::ClosedImmediately(detail),
                None => {
                    PostGrace::ClosedImmediately("game closed immediately after launch".to_string())
                }
            },
        }
    }

    /// Launches a manually added game.
    ///
    /// The child runs with the game's own folder as its working
    /// directory (v2.0.13 — the root cause of the silent no-op
    /// launch: neither spawn path set one, so the game started in
    /// the daemon's folder and died at once). For an exe that is
    /// the exe's parent dir; for a `.lnk` the shortcut target is
    /// resolved first and its parent dir is used — a shortcut
    /// whose target cannot be read is a [`LaunchError::SpawnFailed`],
    /// never a launch into the void.
    ///
    /// After spawning, the pid is watched for `settle`: when the
    /// spawned process *is* the game (its image matches the exe /
    /// shortcut target) and it dies inside the settle window, the
    /// launch fails with a real "started but exited immediately"
    /// error. When the image is a helper (or unreadable) the pid
    /// may legitimately exit at once — it spawned the real game —
    /// so the launch succeeds and the grace window in
    /// [`GameLauncher::refresh`] keeps it running.
    ///
    /// # Errors
    ///
    /// [`LaunchError::NotInstalled`] when the exe path is empty,
    /// [`LaunchError::AlreadyRunning`] when another game is tracked as
    /// running, [`LaunchError::SpawnFailed`] when the OS refuses to start it
    /// or it exits immediately.
    pub fn launch(&mut self, game_id: &str, exe_path: &str) -> Result<RunningGame, LaunchError> {
        let _ = self.refresh();
        if let Some(running) = &self.running {
            return Err(LaunchError::AlreadyRunning(running.game_id.clone()));
        }
        if exe_path.trim().is_empty() {
            return Err(LaunchError::NotInstalled(game_id.to_string()));
        }
        let exe = Path::new(exe_path);
        // v2.0.13: the working directory is the game's own folder.
        // (An empty parent — a bare file name with no folder — is
        // no directory at all, never an empty `current_dir`.)
        let parent_dir = |path: &Path| {
            path.parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .map(Path::to_path_buf)
        };
        let (cwd, expected): (Option<PathBuf>, PathBuf) = if is_shortcut(exe) {
            match resolve_shortcut_target(exe) {
                Ok(target) => (parent_dir(&target), target),
                Err(detail) => {
                    return Err(LaunchError::SpawnFailed(format!(
                        "couldn't read the shortcut's target ({detail}). The shortcut may be broken — re-create it, or add the game's exe directly."
                    )));
                }
            }
        } else {
            (parent_dir(exe), exe.to_path_buf())
        };
        let pid = self
            .spawner
            .spawn_in(exe, cwd.as_deref())
            .map_err(LaunchError::SpawnFailed)?;
        let started_unix_secs = now_unix_secs();
        // R3-D-04: resolve the exe the pid actually runs NOW — for
        // a `.lnk` launch this is the shortcut's target exe, not
        // the `.lnk` path. The pid-reuse guard compares against
        // this; without it every shortcut game was marked "not
        // running" within ~1 s of launch (the resolved image never
        // equals the stored `.lnk` path).
        let mut resolved_exe_path = self
            .spawner
            .exe_path(pid)
            .map(|path| path.to_string_lossy().into_owned());
        // v2.0.13 settle: a game that starts and dies at once must
        // be a visible error, not a silent "not running" — but only
        // when the dead pid *is* the game. A helper that spawned the
        // real game and exited is the normal case for some
        // shortcuts; its image never matches the expected target.
        //
        // F2 (v2.0.21): the v2.0.13 check queried the image ONCE,
        // immediately after spawn — a process that died in that
        // gap answered `None`, read as "helper", and the launch
        // recorded success while nothing was running. So the image
        // is re-queried on every 100 ms poll while the pid is
        // alive, and again the moment it dies; and when no image
        // can be read at all, a nonzero exit code proves the game
        // itself failed (an exit of 0 / unknown still leaves the
        // helper case to the post-grace check in `refresh`).
        if !self.settle.is_zero() {
            let deadline = std::time::Instant::now() + self.settle;
            loop {
                if resolved_exe_path.is_none() {
                    resolved_exe_path = self
                        .spawner
                        .exe_path(pid)
                        .map(|path| path.to_string_lossy().into_owned());
                }
                if !self.spawner.is_alive(pid) {
                    // One last image query: the process may have
                    // died between the poll above and this check.
                    if resolved_exe_path.is_none() {
                        resolved_exe_path = self
                            .spawner
                            .exe_path(pid)
                            .map(|path| path.to_string_lossy().into_owned());
                    }
                    let is_game = resolved_exe_path
                        .as_deref()
                        .is_some_and(|image| paths_equal(Path::new(image), &expected));
                    if is_game {
                        return Err(LaunchError::SpawnFailed(format!(
                            "game started but exited immediately ({}). It may need files from its own folder, or administrator rights. Try starting it from File Explorer — if it closes there too, the game itself is failing to start.",
                            expected.display()
                        )));
                    }
                    if resolved_exe_path.is_none()
                        && let Some(code) = self.spawner.exit_code(pid)
                        && code != 0
                    {
                        return Err(LaunchError::SpawnFailed(format!(
                            "game started but exited immediately ({}) with exit code 0x{code:08X} ({code}). It may need files from its own folder, or administrator rights. Try starting it from File Explorer — if it closes there too, the game itself is failing to start.",
                            expected.display()
                        )));
                    }
                    break;
                }
                if std::time::Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        let running = RunningGame {
            game_id: game_id.to_string(),
            pid,
            exe_path: exe_path.to_string(),
            resolved_exe_path,
            expected_exe_path: Some(expected.to_string_lossy().into_owned()),
            started_unix_secs,
        };
        self.running = Some(running.clone());
        Ok(running)
    }

    /// Re-checks whether the tracked game is still alive; clears the
    /// record when it exited on its own, or when the pid now belongs to
    /// a different exe (L10 pid-reuse guard). Returns the current record.
    ///
    /// R3-D-04: the guard compares against the launch-time resolved
    /// image (`resolved_exe_path`), not the stored launch path — a
    /// `.lnk` launch's stored path never equals its target exe. When
    /// the image could not be resolved at launch AND the launch was
    /// a shortcut, the exe-path comparison is skipped (liveness
    /// only): a stale comparison there is worse than no comparison.
    ///
    /// v2.0.13 launch grace: a dead pid inside the grace window
    /// after launch does NOT clear the record — a shortcut helper
    /// spawns the real game and exits at once, and the first
    /// refresh must not report the game "not running" while the
    /// real game is starting. The pid-reuse guard still applies
    /// inside the grace (a live pid running another exe clears).
    ///
    /// F2 (v2.0.21) post-grace verification: once the grace has
    /// expired and the tracked pid is dead, the record is NOT
    /// cleared blindly — first the spawner is asked whether ANY
    /// live process runs the expected image, and the record
    /// re-attaches to that pid when one exists (the tracked pid
    /// was a helper). When no such process exists and the launch
    /// was recent, the clear is recorded via
    /// [`Self::take_closed_immediately`] so the app layer can
    /// surface the `game_closed_immediately` error instead of a
    /// silent flip back to "not running".
    ///
    /// # Panics
    ///
    /// Panics if the running record disappears between the liveness
    /// check and the post-grace re-attach — unreachable in practice
    /// (`refresh` holds `&mut self` and the record is only cleared
    /// in this function, after the check), but the `expect` calls
    /// below name it.
    #[must_use]
    pub fn refresh(&mut self) -> Option<&RunningGame> {
        let now = now_unix_secs();
        let grace_secs = self.grace.as_secs();
        let running = self.running.as_ref()?;
        let (pid_dead, pid_reused) = if !self.spawner.is_alive(running.pid) {
            (
                now >= running.started_unix_secs.saturating_add(grace_secs),
                false,
            )
        } else if is_shortcut(Path::new(&running.exe_path)) && running.resolved_exe_path.is_none() {
            // Unresolved shortcut launch: liveness only —
            // there is no trustworthy image to compare.
            (false, false)
        } else if let Some(exe) = self.spawner.exe_path(running.pid) {
            // The pid is alive but runs a different exe: the
            // game exited and the OS reused its pid.
            let guard_path = running
                .resolved_exe_path
                .as_deref()
                .unwrap_or(running.exe_path.as_str());
            (false, exe != Path::new(guard_path))
        } else {
            (false, false)
        };
        if pid_dead {
            // F2 (v2.0.21) post-grace verification: the tracked pid
            // may have been a helper/launcher stub that exited
            // while the real game runs under a different pid
            // (Store apps, launcher stubs). Before clearing the
            // record, look for ANY live process with the expected
            // image and re-attach to it.
            let (old_pid, game_id, started, expected) = {
                let running = self.running.as_ref().expect("checked above");
                // F2: the search key is the image the GAME is
                // expected to run as — never `resolved_exe_path`,
                // which is the image the *tracked pid* runs (a
                // helper's image when the tracked pid is a launcher
                // stub). Fallback for pre-v2.0.21 records without
                // the field: the stored exe for plain launches,
                // no search for shortcuts.
                let expected = running.expected_exe_path.clone().or_else(|| {
                    if is_shortcut(Path::new(&running.exe_path)) {
                        None
                    } else {
                        Some(running.exe_path.clone())
                    }
                });
                (
                    running.pid,
                    running.game_id.clone(),
                    running.started_unix_secs,
                    expected,
                )
            };
            if let Some(expected) = &expected
                && let Some(found) = self.spawner.find_process_with_image(Path::new(expected))
                && found != old_pid
                && self.spawner.is_alive(found)
            {
                let running = self.running.as_mut().expect("checked above");
                running.pid = found;
                running.resolved_exe_path = Some(expected.clone());
                self.launch_notes.push(format!(
                    "game '{game_id}' re-attached to pid {found} running {expected} (tracked pid {old_pid} exited — it was a launcher/helper)"
                ));
                return self.running.as_ref();
            }
            // No live process with the expected image: the game is
            // gone. Record it as closed-immediately — but only
            // when the launch was recent (grace + 30 s): a game
            // the user played for an hour and then closed is a
            // normal exit, not a failed launch.
            let exit_code = self.spawner.exit_code(old_pid);
            if let Some(code) = exit_code {
                self.launch_notes.push(format!(
                    "game '{game_id}' tracked pid {old_pid} exited with code 0x{code:08X} ({code})"
                ));
            }
            self.running = None;
            if now
                <= started
                    .saturating_add(grace_secs)
                    .saturating_add(CLOSED_IMMEDIATELY_WINDOW_SECS)
            {
                let exit_note = exit_code
                    .map(|code| format!(" (exit code 0x{code:08X})"))
                    .unwrap_or_default();
                let detail = format!(
                    "game '{game_id}' closed immediately after launch{exit_note} — no running process was found after the launch grace period. Try starting it from File Explorer — if it closes there too, the game itself is failing to start."
                );
                self.closed_immediately = Some((game_id, detail));
            }
            return None;
        }
        if pid_reused {
            self.running = None;
        }
        self.running.as_ref()
    }

    /// Stops the tracked game (only the tracked pid — never any other
    /// same-name process) and returns its record.
    ///
    /// M1: the record is cleared only on a successful terminate, or
    /// when a refresh confirms the process is already dead. A failed
    /// terminate on a still-alive process keeps the record, so the
    /// stop can be retried instead of losing the game.
    ///
    /// # Errors
    ///
    /// Returns an error message when nothing is running or the tracked
    /// process could not be terminated.
    pub fn stop(&mut self) -> Result<RunningGame, String> {
        let running = self
            .running
            .clone()
            .ok_or_else(|| "no game is running".to_owned())?;
        match self.spawner.terminate(running.pid) {
            Ok(()) => {
                self.running = None;
                Ok(running)
            }
            Err(error) => {
                if self.spawner.is_alive(running.pid) {
                    Err(error)
                } else {
                    // It exited between the last refresh and the
                    // terminate — the stop still happened.
                    self.running = None;
                    Ok(running)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::{HashMap, HashSet};

    use super::*;

    /// Fake spawner: records spawns/kills, hands out fake pids.
    struct FakeSpawner {
        next_pid: u32,
        alive: RefCell<HashMap<u32, PathBuf>>,
        /// Pids that "exited at once" after spawn (v2.0.13 settle /
        /// grace tests): dead for `is_alive`, but their image is
        /// still readable via `exe_path`, like a real just-exited
        /// process whose image was resolved at launch.
        dead: RefCell<HashSet<u32>>,
        spawned: RefCell<Vec<PathBuf>>,
        /// Working directory each spawn was asked for, in order.
        spawned_cwd: RefCell<Vec<Option<PathBuf>>>,
        killed: RefCell<Vec<u32>>,
        fail_terminate: RefCell<bool>,
        /// When set, every spawned pid dies at once.
        die_after_spawn: RefCell<bool>,
        /// Exit code a pid that died at once reports (F2): 1 by
        /// default, so an instantly-exiting game fails fast with
        /// its code; tests set 0 for the clean-exit helper case.
        die_exit_code: RefCell<u32>,
        /// Shortcut → target resolution (R3-D-04): when a spawn
        /// path is listed here, the pid's image is the target, not
        /// the spawned path — what `QueryFullProcessImageNameW`
        /// returns for a real `.lnk` launch on Windows.
        resolve: RefCell<HashMap<PathBuf, PathBuf>>,
        /// When set, `exe_path` returns [`None`] (image unreadable).
        fail_exe_path: RefCell<bool>,
    }

    impl FakeSpawner {
        fn new() -> Self {
            Self {
                next_pid: 1000,
                alive: RefCell::new(HashMap::new()),
                dead: RefCell::new(HashSet::new()),
                spawned: RefCell::new(Vec::new()),
                spawned_cwd: RefCell::new(Vec::new()),
                killed: RefCell::new(Vec::new()),
                fail_terminate: RefCell::new(false),
                die_after_spawn: RefCell::new(false),
                die_exit_code: RefCell::new(1),
                resolve: RefCell::new(HashMap::new()),
                fail_exe_path: RefCell::new(false),
            }
        }

        fn kill_from_outside(&self, pid: u32) {
            self.alive.borrow_mut().remove(&pid);
        }

        /// Register a live process with `image` under `pid` (F2):
        /// the "real game" a helper spawned under its own pid —
        /// the post-grace re-attach target.
        fn spawn_live(&self, pid: u32, image: &Path) {
            self.alive.borrow_mut().insert(pid, image.to_path_buf());
        }

        fn set_die_exit_code(&self, code: u32) {
            *self.die_exit_code.borrow_mut() = code;
        }

        fn set_fail_terminate(&self, fail: bool) {
            *self.fail_terminate.borrow_mut() = fail;
        }

        /// Register a shortcut → target resolution for later spawns.
        fn map_shortcut(&self, shortcut: &Path, target: &Path) {
            self.resolve
                .borrow_mut()
                .insert(shortcut.to_path_buf(), target.to_path_buf());
        }

        fn set_fail_exe_path(&self, fail: bool) {
            *self.fail_exe_path.borrow_mut() = fail;
        }

        fn set_die_after_spawn(&self, die: bool) {
            *self.die_after_spawn.borrow_mut() = die;
        }
    }

    impl ProcessSpawner for FakeSpawner {
        fn spawn(&mut self, exe: &Path) -> Result<u32, String> {
            let pid = self.next_pid;
            self.next_pid += 1;
            // R3-D-04: a registered shortcut resolves to its target
            // image at spawn, like the real Windows launch.
            let image = self
                .resolve
                .borrow()
                .get(exe)
                .cloned()
                .unwrap_or_else(|| exe.to_path_buf());
            self.alive.borrow_mut().insert(pid, image);
            if *self.die_after_spawn.borrow() {
                self.dead.borrow_mut().insert(pid);
            }
            self.spawned.borrow_mut().push(exe.to_path_buf());
            Ok(pid)
        }

        fn spawn_in(&mut self, exe: &Path, cwd: Option<&Path>) -> Result<u32, String> {
            self.spawned_cwd
                .borrow_mut()
                .push(cwd.map(Path::to_path_buf));
            self.spawn(exe)
        }

        fn is_alive(&self, pid: u32) -> bool {
            self.alive.borrow().contains_key(&pid) && !self.dead.borrow().contains(&pid)
        }

        fn terminate(&mut self, pid: u32) -> Result<(), String> {
            if *self.fail_terminate.borrow() {
                return Err("simulated terminate failure".to_owned());
            }
            if self.alive.borrow_mut().remove(&pid).is_some() {
                self.killed.borrow_mut().push(pid);
                Ok(())
            } else {
                Err(format!("pid {pid} is not alive"))
            }
        }

        fn exe_path(&self, pid: u32) -> Option<PathBuf> {
            if *self.fail_exe_path.borrow() {
                return None;
            }
            self.alive.borrow().get(&pid).cloned()
        }

        fn exit_code(&self, pid: u32) -> Option<u32> {
            if self.dead.borrow().contains(&pid) {
                Some(*self.die_exit_code.borrow())
            } else {
                None
            }
        }

        fn find_process_with_image(&self, exe: &Path) -> Option<u32> {
            let dead = self.dead.borrow();
            self.alive
                .borrow()
                .iter()
                .find(|(pid, image)| !dead.contains(pid) && paths_equal(image, exe))
                .map(|(pid, _)| *pid)
        }
    }

    /// Unique temp dir per test call (tests run in parallel in one
    /// process, so the name must be unique per call, not just per test).
    /// Removed afterwards.
    fn test_dir(name: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "rsh-launcher-test-{}-{name}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create test dir");
        dir
    }

    /// A fake exe path inside a fresh temp dir.
    fn fake_exe(dir: &Path, name: &str) -> String {
        let exe = dir.join(name);
        std::fs::write(&exe, b"fake exe").expect("write fixture exe");
        exe.to_string_lossy().into_owned()
    }

    /// Launcher with zero settle / grace: existing semantics tests
    /// check launch / stop / refresh without waiting out the
    /// production settle window or the launch grace.
    fn launcher() -> GameLauncher<FakeSpawner> {
        GameLauncher::with_timing(
            FakeSpawner::new(),
            std::time::Duration::from_millis(0),
            std::time::Duration::from_millis(0),
        )
    }

    /// Writes a minimal but structurally valid `.lnk` pointing at
    /// `target` (76-byte header + `LinkInfo` with `LocalBasePath`),
    /// so [`resolve_shortcut_target`] reads the target back.
    fn write_lnk(dir: &Path, name: &str, target: &Path) -> PathBuf {
        let target_bytes = target
            .as_os_str()
            .to_string_lossy()
            .into_owned()
            .into_bytes();
        let suffix = target
            .file_name()
            .map(|file| file.to_string_lossy().into_owned().into_bytes())
            .unwrap_or_default();
        let mut header = vec![0_u8; 76];
        header[0..4].copy_from_slice(&0x0000_004C_u32.to_le_bytes());
        // ShellLink GUID {00021401-0000-0000-C000-000000000046}.
        header[4..20].copy_from_slice(&[
            0x01, 0x14, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x46,
        ]);
        header[20..24].copy_from_slice(&0x0000_0002_u32.to_le_bytes()); // HasLinkInfo
        let volume_size = 16_u32;
        let local_base_off = 28 + volume_size;
        let suffix_off = local_base_off + u32::try_from(target_bytes.len()).unwrap_or(u32::MAX) + 1;
        let linkinfo_size = suffix_off + u32::try_from(suffix.len()).unwrap_or(u32::MAX) + 1;
        let mut info = Vec::new();
        info.extend_from_slice(&linkinfo_size.to_le_bytes());
        info.extend_from_slice(&28_u32.to_le_bytes()); // LinkInfoHeaderSize
        info.extend_from_slice(&0x0000_0001_u32.to_le_bytes()); // VolumeIDAndLocalBasePath
        info.extend_from_slice(&28_u32.to_le_bytes()); // VolumeIDOffset
        info.extend_from_slice(&local_base_off.to_le_bytes());
        info.extend_from_slice(&0_u32.to_le_bytes()); // CommonNetworkRelativeLinkOffset
        info.extend_from_slice(&suffix_off.to_le_bytes());
        info.extend_from_slice(&volume_size.to_le_bytes()); // VolumeIDSize
        info.extend_from_slice(&3_u32.to_le_bytes()); // DRIVE_FIXED
        info.extend_from_slice(&0_u32.to_le_bytes()); // DriveSerialNumber
        info.extend_from_slice(&16_u32.to_le_bytes()); // VolumeLabelOffset (empty label)
        info.extend_from_slice(&target_bytes);
        info.push(0);
        info.extend_from_slice(&suffix);
        info.push(0);
        assert_eq!(
            u32::try_from(info.len()).expect("fixture link fits in u32"),
            linkinfo_size,
            "fixture link size"
        );
        let mut bytes = header;
        bytes.extend_from_slice(&info);
        let path = dir.join(name);
        std::fs::write(&path, bytes).expect("write fixture lnk");
        path
    }

    #[test]
    fn is_shortcut_matches_lnk_case_insensitively() {
        assert!(is_shortcut(Path::new(r"C:\Games\race.lnk")));
        assert!(is_shortcut(Path::new(r"C:\Games\race.LNK")));
        assert!(!is_shortcut(Path::new(r"C:\Games\race.exe")));
        assert!(!is_shortcut(Path::new(r"C:\Games\race")));
    }

    #[test]
    fn launch_tracks_running_and_rejects_second_game() {
        let dir = test_dir("launch");
        let ac = fake_exe(&dir, "race.exe");
        let dirt = fake_exe(&dir, "dirt2.exe");

        let mut launcher = launcher();
        let running = launcher.launch("my-race", &ac).expect("launch");
        assert_eq!(running.game_id, "my-race");
        assert_eq!(
            launcher.running().map(|game| game.game_id.as_str()),
            Some("my-race")
        );

        let second = launcher.launch("dirt", &dirt);
        assert_eq!(
            second,
            Err(LaunchError::AlreadyRunning("my-race".to_owned()))
        );

        let stopped = launcher.stop().expect("stop works");
        assert_eq!(stopped.game_id, "my-race");
        assert!(launcher.running().is_none());

        // After stopping, another game can launch.
        assert!(launcher.launch("dirt", &dirt).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn launch_empty_exe_path_is_not_installed() {
        let mut launcher = launcher();
        assert_eq!(
            launcher.launch("my-race", ""),
            Err(LaunchError::NotInstalled("my-race".to_owned()))
        );
        assert_eq!(
            launcher.launch("my-race", "   "),
            Err(LaunchError::NotInstalled("my-race".to_owned()))
        );
    }

    #[test]
    fn stop_kills_only_the_tracked_pid() {
        let dir = test_dir("stop-only-tracked");
        let ac = fake_exe(&dir, "race.exe");

        let mut launcher = launcher();
        // A second, unrelated process with pid 2000 is alive in the fake.
        launcher
            .spawner
            .alive
            .borrow_mut()
            .insert(2000, PathBuf::from("other-race.exe"));
        let running = launcher.launch("my-race", &ac).expect("launch");
        launcher.stop().expect("stop works");
        assert!(
            launcher.spawner.alive.borrow().contains_key(&2000),
            "SEC-E-10: an unrelated pid must survive the stop"
        );
        assert_eq!(launcher.spawner.killed.borrow().as_slice(), [running.pid]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refresh_clears_a_game_that_exited_on_its_own() {
        let dir = test_dir("refresh-dead");
        let ac = fake_exe(&dir, "race.exe");

        let mut launcher = launcher();
        let running = launcher.launch("my-race", &ac).expect("launch");
        launcher.spawner.kill_from_outside(running.pid);
        assert!(launcher.refresh().is_none());
        assert!(launcher.running().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stop_with_nothing_running_is_an_error() {
        let mut launcher = launcher();
        assert!(launcher.stop().is_err());
    }

    /// M1: a failed terminate on a still-alive process keeps the
    /// record, so the stop can be retried.
    #[test]
    fn stop_failed_terminate_keeps_record() {
        let dir = test_dir("stop-fail");
        let ac = fake_exe(&dir, "race.exe");

        let mut launcher = launcher();
        launcher.launch("my-race", &ac).expect("launch");
        launcher.spawner.set_fail_terminate(true);
        assert!(launcher.stop().is_err());
        assert!(
            launcher.running().is_some(),
            "record must survive a failed stop"
        );
        launcher.spawner.set_fail_terminate(false);
        assert!(
            launcher.stop().is_ok(),
            "retry succeeds once terminate works"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// M1: when the process already died, stop still succeeds and
    /// clears the record (the game is gone either way).
    #[test]
    fn stop_when_process_already_dead_clears_record() {
        let dir = test_dir("stop-dead");
        let ac = fake_exe(&dir, "race.exe");

        let mut launcher = launcher();
        let running = launcher.launch("my-race", &ac).expect("launch");
        launcher.spawner.kill_from_outside(running.pid);
        assert!(launcher.stop().is_ok());
        assert!(launcher.running().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// L10: a pid reused by a different exe is not mistaken for the game.
    #[test]
    fn refresh_clears_on_exe_path_mismatch() {
        let dir = test_dir("refresh-mismatch");
        let ac = fake_exe(&dir, "race.exe");

        let mut launcher = launcher();
        let running = launcher.launch("my-race", &ac).expect("launch");
        launcher
            .spawner
            .alive
            .borrow_mut()
            .insert(running.pid, PathBuf::from("other-game.exe"));
        assert!(launcher.refresh().is_none(), "pid now runs another exe");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// R3-D-04: a `.lnk` launch resolves to its target exe at
    /// launch time, and the pid-reuse guard compares against the
    /// resolved image — the game must NOT be marked "not running"
    /// just because the stored path is the shortcut.
    #[test]
    fn shortcut_launch_stays_running_against_resolved_target() {
        let dir = test_dir("refresh-lnk");
        let target = fake_exe(&dir, "race.exe");
        let shortcut = write_lnk(&dir, "race.lnk", Path::new(&target));

        let mut launcher = launcher();
        launcher.spawner.map_shortcut(&shortcut, Path::new(&target));
        let running = launcher
            .launch("my-race", &shortcut.to_string_lossy())
            .expect("launch shortcut");
        assert_eq!(running.exe_path, shortcut.to_string_lossy());
        assert_eq!(
            running.resolved_exe_path.as_deref(),
            Some(target.as_str()),
            "launch must resolve the shortcut target"
        );
        // The guard runs on every ~1 Hz slow poll: it must keep
        // the game running.
        assert!(
            launcher.refresh().is_some(),
            "shortcut game must stay running"
        );
        assert_eq!(
            launcher.running().map(|game| game.game_id.as_str()),
            Some("my-race")
        );
        // ...but the guard still fires when the pid really is
        // reused by another exe.
        launcher
            .spawner
            .alive
            .borrow_mut()
            .insert(running.pid, PathBuf::from("other-game.exe"));
        assert!(launcher.refresh().is_none(), "pid now runs another exe");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// R3-D-04: when the image cannot be read at launch, a
    /// shortcut launch falls back to liveness-only — never to a
    /// comparison against the `.lnk` path (which can never match).
    #[test]
    fn unresolved_shortcut_launch_is_liveness_only() {
        let dir = test_dir("refresh-lnk-unresolved");
        let shortcut = write_lnk(&dir, "race.lnk", Path::new(r"C:\Games\race.exe"));

        let mut launcher = launcher();
        launcher.spawner.set_fail_exe_path(true);
        let running = launcher
            .launch("my-race", &shortcut.to_string_lossy())
            .expect("launch shortcut");
        assert_eq!(running.resolved_exe_path, None);
        assert!(
            launcher.refresh().is_some(),
            "unresolved shortcut must stay running while alive"
        );
        launcher.spawner.kill_from_outside(running.pid);
        assert!(
            launcher.refresh().is_none(),
            "a dead shortcut game still clears on liveness"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- v2.0.13 game-launch gate tests ---

    /// The `.lnk` fixture round-trips through the parser, and
    /// garbage / truncated files are rejected, never silently
    /// launched into the daemon's folder.
    #[test]
    fn shortcut_target_resolution_round_trips() {
        let dir = test_dir("lnk-parse");
        let target = fake_exe(&dir, "race.exe");
        let shortcut = write_lnk(&dir, "race.lnk", Path::new(&target));
        assert_eq!(
            resolve_shortcut_target(&shortcut).expect("parse fixture lnk"),
            Path::new(&target)
        );

        let garbage = dir.join("garbage.lnk");
        std::fs::write(&garbage, b"not a shortcut at all").expect("write garbage lnk");
        assert!(
            resolve_shortcut_target(&garbage).is_err(),
            "garbage must not resolve to a target"
        );
        let tiny = dir.join("tiny.lnk");
        std::fs::write(&tiny, b"tiny").expect("write tiny lnk");
        assert!(resolve_shortcut_target(&tiny).is_err(), "truncated lnk");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// v2.0.13 root cause: an exe launch must run with the exe's
    /// own folder as the working directory — never the daemon's.
    #[test]
    fn launch_exe_uses_exe_parent_as_cwd() {
        let dir = test_dir("cwd-exe");
        let game_dir = dir.join("game");
        std::fs::create_dir_all(&game_dir).expect("create game dir");
        let exe = fake_exe(&game_dir, "race.exe");

        let mut launcher = launcher();
        launcher.launch("my-race", &exe).expect("launch");
        assert_eq!(
            launcher.spawner.spawned_cwd.borrow().as_slice(),
            [Some(game_dir)],
            "the game must start in its own folder"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A shortcut launch must run with the *target's* folder as
    /// the working directory — not the shortcut's folder (Desktop)
    /// and never the daemon's.
    #[test]
    fn launch_shortcut_uses_target_parent_as_cwd() {
        let dir = test_dir("cwd-lnk");
        let game_dir = dir.join("game");
        let desk_dir = dir.join("desktop");
        std::fs::create_dir_all(&game_dir).expect("create game dir");
        std::fs::create_dir_all(&desk_dir).expect("create desk dir");
        let target = fake_exe(&game_dir, "race.exe");
        let shortcut = write_lnk(&desk_dir, "race.lnk", Path::new(&target));

        let mut launcher = launcher();
        launcher.spawner.map_shortcut(&shortcut, Path::new(&target));
        let running = launcher
            .launch("my-race", &shortcut.to_string_lossy())
            .expect("launch shortcut");
        assert_eq!(
            launcher.spawner.spawned_cwd.borrow().as_slice(),
            [Some(game_dir)],
            "the shortcut launch must start in the target's folder"
        );
        assert_eq!(running.resolved_exe_path.as_deref(), Some(target.as_str()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A shortcut whose target cannot be read is a real error
    /// naming the shortcut — never a launch into the void.
    #[test]
    fn launch_broken_shortcut_is_a_real_error() {
        let dir = test_dir("cwd-lnk-broken");
        let shortcut = dir.join("broken.lnk");
        std::fs::write(&shortcut, b"not a shortcut at all").expect("write broken lnk");

        let mut launcher = launcher();
        let error = launcher
            .launch("my-race", &shortcut.to_string_lossy())
            .expect_err("a broken shortcut must not launch");
        assert!(
            matches!(error, LaunchError::SpawnFailed(_)),
            "broken shortcut must be SpawnFailed, got {error:?}"
        );
        assert!(
            error.to_string().contains("shortcut"),
            "the error must name the shortcut: {error}"
        );
        assert!(launcher.running().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A game that starts and exits at once is a visible error,
    /// not a silent "not running" tile.
    #[test]
    fn launch_game_that_exits_immediately_is_an_error() {
        let dir = test_dir("settle-exit");
        let exe = fake_exe(&dir, "race.exe");

        let mut launcher =
            GameLauncher::with_settle(FakeSpawner::new(), std::time::Duration::from_millis(500));
        launcher.spawner.set_die_after_spawn(true);
        let error = launcher
            .launch("my-race", &exe)
            .expect_err("an instantly-exiting game must fail the launch");
        assert!(
            matches!(error, LaunchError::SpawnFailed(_)),
            "instant exit must be SpawnFailed, got {error:?}"
        );
        assert!(
            error.to_string().contains("exited immediately"),
            "the error must say the game exited immediately: {error}"
        );
        assert!(launcher.running().is_none(), "no stale running record");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ...but a shortcut helper that spawns the real game and exits
    /// at once is NOT that error: the launch succeeds and the first
    /// refreshes keep the game running (launch grace).
    #[test]
    fn launch_helper_that_exits_immediately_stays_running() {
        let dir = test_dir("settle-helper");
        let game_dir = dir.join("game");
        std::fs::create_dir_all(&game_dir).expect("create game dir");
        let target = fake_exe(&game_dir, "race.exe");
        let helper = fake_exe(&dir, "helper.exe");
        let shortcut = write_lnk(&dir, "race.lnk", Path::new(&target));

        let mut launcher =
            GameLauncher::with_settle(FakeSpawner::new(), std::time::Duration::from_millis(500));
        launcher.spawner.map_shortcut(&shortcut, Path::new(&helper));
        launcher.spawner.set_die_after_spawn(true);
        let running = launcher
            .launch("my-race", &shortcut.to_string_lossy())
            .expect("a helper exit must not fail the launch");
        assert_eq!(running.game_id, "my-race");
        assert!(
            launcher.refresh().is_some(),
            "the first refreshes after launch must keep the game running"
        );
        assert_eq!(
            launcher.running().map(|game| game.game_id.as_str()),
            Some("my-race")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// F2 (v2.0.21): the dead pid's image could not be read at all
    /// (it died between spawn and the first query) — a nonzero
    /// exit code still proves the game itself failed, so the
    /// launch fails fast with the code in the message. This is the
    /// case v2.0.13 missed: image `None` used to read as "helper".
    #[test]
    fn launch_dead_pid_with_no_image_and_nonzero_exit_is_an_error() {
        let dir = test_dir("settle-noimage-nonzero");
        let exe = fake_exe(&dir, "race.exe");

        let mut launcher = GameLauncher::with_timing(
            FakeSpawner::new(),
            std::time::Duration::from_millis(500),
            std::time::Duration::from_millis(0),
        );
        launcher.spawner.set_die_after_spawn(true);
        launcher.spawner.set_fail_exe_path(true);
        let error = launcher
            .launch("my-race", &exe)
            .expect_err("a nonzero exit with no image must fail the launch");
        assert!(
            matches!(error, LaunchError::SpawnFailed(_)),
            "must be SpawnFailed, got {error:?}"
        );
        assert!(
            error.to_string().contains("0x00000001"),
            "the error must carry the exit code: {error}"
        );
        assert!(launcher.running().is_none(), "no stale running record");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// F2: the same unreadable image with a CLEAN exit (0) is the
    /// helper case — the launch succeeds, and the post-grace
    /// refresh then reports the game closed immediately (no live
    /// process with the expected image) instead of silently
    /// flipping the tile back to "not running".
    #[test]
    fn launch_clean_exit_with_no_image_reports_closed_immediately() {
        let dir = test_dir("settle-noimage-zero");
        let exe = fake_exe(&dir, "race.exe");

        let mut launcher = GameLauncher::with_timing(
            FakeSpawner::new(),
            std::time::Duration::from_millis(500),
            std::time::Duration::from_millis(0),
        );
        launcher.spawner.set_die_after_spawn(true);
        launcher.spawner.set_fail_exe_path(true);
        launcher.spawner.set_die_exit_code(0);
        let running = launcher
            .launch("my-race", &exe)
            .expect("a clean exit with no image must not fail the launch");
        assert_eq!(running.game_id, "my-race");
        assert!(
            launcher.refresh().is_none(),
            "the post-grace refresh must clear the record"
        );
        let (game_id, detail) = launcher
            .take_closed_immediately()
            .expect("the clear must record closed-immediately");
        assert_eq!(game_id, "my-race");
        assert!(
            detail.contains("closed immediately"),
            "the detail must say the game closed immediately: {detail}"
        );
        assert!(
            detail.contains("0x00000000"),
            "the detail must carry the exit code: {detail}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// F2: the tracked pid was a helper that exited, but the real
    /// game runs under another pid — the post-grace refresh
    /// re-attaches to it instead of clearing the record.
    #[test]
    fn refresh_reattaches_to_real_game_when_helper_pid_dies() {
        let dir = test_dir("refresh-reattach");
        let game_dir = dir.join("game");
        std::fs::create_dir_all(&game_dir).expect("create game dir");
        let target = fake_exe(&game_dir, "race.exe");
        let helper = fake_exe(&dir, "helper.exe");
        let shortcut = write_lnk(&dir, "race.lnk", Path::new(&target));

        let mut launcher = GameLauncher::with_timing(
            FakeSpawner::new(),
            std::time::Duration::from_millis(500),
            std::time::Duration::from_millis(0),
        );
        launcher.spawner.map_shortcut(&shortcut, Path::new(&helper));
        launcher.spawner.set_die_after_spawn(true);
        let running = launcher
            .launch("my-race", &shortcut.to_string_lossy())
            .expect("a helper exit must not fail the launch");
        assert_eq!(running.pid, 1000);
        // The real game the helper spawned, under its own pid.
        launcher.spawner.spawn_live(2000, Path::new(&target));
        let refreshed = launcher
            .refresh()
            .expect("the refresh must re-attach to the real game");
        assert_eq!(refreshed.pid, 2000);
        assert_eq!(
            refreshed.resolved_exe_path.as_deref(),
            Some(target.as_str()),
            "the re-attached record binds the real image"
        );
        assert!(
            launcher.take_closed_immediately().is_none(),
            "a re-attach is not a closed-immediately"
        );
        assert!(
            launcher
                .take_launch_notes()
                .iter()
                .any(|note| note.contains("re-attached")),
            "the re-attach must be noted for the daemon log"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
