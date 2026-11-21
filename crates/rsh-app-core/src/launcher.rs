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
        /// Working directory (`null` = the shortcut's own).
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
    /// returns the child pid. Windows-only.
    #[cfg(windows)]
    fn spawn_shortcut(exe: &Path) -> Result<u32, String> {
        use std::os::windows::ffi::OsStrExt;
        // SAFETY: plain Win32 call with checked returns; the path
        // buffer is valid for the call, the info struct is fully
        // initialised, and the process handle is always closed.
        unsafe {
            let file_wide: Vec<u16> = exe
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let mut info = win32::ShellExecuteInfoW {
                cb_size: u32::try_from(std::mem::size_of::<win32::ShellExecuteInfoW>())
                    .unwrap_or(u32::MAX),
                f_mask: win32::SEE_MASK_NOCLOSEPROCESS,
                hwnd: 0,
                lp_verb: std::ptr::null(),
                lp_file: file_wide.as_ptr(),
                lp_parameters: std::ptr::null(),
                lp_directory: std::ptr::null(),
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
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            if is_shortcut(exe) {
                return Self::spawn_shortcut(exe);
            }
            // DS-47: null stdio + detached, no-window creation —
            // mirroring the daemon's `spawn_ui` (F8). A game that
            // inherits the daemon's stdio pipes can block the parent
            // forever (the NSIS `Exec` hang class fixed in v2.0.2),
            // and a console window must never flash up for a GUI
            // game. Flags are the raw Win32 values, as elsewhere in
            // this crate (`state.rs`): DETACHED_PROCESS 0x8,
            // CREATE_NO_WINDOW 0x0800_0000.
            std::process::Command::new(exe)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .creation_flags(0x0000_0008 | 0x0800_0000)
                .spawn()
                .map(|child| child.id())
                .map_err(|error| error.to_string())
        }
        #[cfg(not(windows))]
        {
            // The sandbox has no Windows games; spawning a Windows exe
            // here fails at the OS level and that failure is reported,
            // never hidden.
            let _ = exe;
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
}

/// Launches and tracks games: exactly one at a time.
///
/// The launcher owns no profile state — it only reports which game is
/// running. The watcher / app layer binds the game's profile (see
/// [`crate::manual_games::ManualGame::bound_profile_id`]).
pub struct GameLauncher<S: ProcessSpawner> {
    spawner: S,
    running: Option<RunningGame>,
}

impl<S: ProcessSpawner> GameLauncher<S> {
    /// Creates a launcher over a spawner.
    #[must_use]
    pub fn new(spawner: S) -> Self {
        Self {
            spawner,
            running: None,
        }
    }

    /// The currently tracked running game, if any.
    #[must_use]
    pub fn running(&self) -> Option<&RunningGame> {
        self.running.as_ref()
    }

    /// Launches a manually added game.
    ///
    /// # Errors
    ///
    /// [`LaunchError::NotInstalled`] when the exe path is empty,
    /// [`LaunchError::AlreadyRunning`] when another game is tracked as
    /// running, [`LaunchError::SpawnFailed`] when the OS refuses to start it.
    pub fn launch(&mut self, game_id: &str, exe_path: &str) -> Result<RunningGame, LaunchError> {
        let _ = self.refresh();
        if let Some(running) = &self.running {
            return Err(LaunchError::AlreadyRunning(running.game_id.clone()));
        }
        if exe_path.trim().is_empty() {
            return Err(LaunchError::NotInstalled(game_id.to_string()));
        }
        let pid = self
            .spawner
            .spawn(Path::new(exe_path))
            .map_err(LaunchError::SpawnFailed)?;
        let started_unix_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs());
        // R3-D-04: resolve the exe the pid actually runs NOW — for
        // a `.lnk` launch this is the shortcut's target exe, not
        // the `.lnk` path. The pid-reuse guard compares against
        // this; without it every shortcut game was marked "not
        // running" within ~1 s of launch (the resolved image never
        // equals the stored `.lnk` path).
        let resolved_exe_path = self
            .spawner
            .exe_path(pid)
            .map(|path| path.to_string_lossy().into_owned());
        let running = RunningGame {
            game_id: game_id.to_string(),
            pid,
            exe_path: exe_path.to_string(),
            resolved_exe_path,
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
    #[must_use]
    pub fn refresh(&mut self) -> Option<&RunningGame> {
        let dead = match &self.running {
            Some(running) => {
                if !self.spawner.is_alive(running.pid) {
                    true
                } else if is_shortcut(Path::new(&running.exe_path))
                    && running.resolved_exe_path.is_none()
                {
                    // Unresolved shortcut launch: liveness only —
                    // there is no trustworthy image to compare.
                    false
                } else if let Some(exe) = self.spawner.exe_path(running.pid) {
                    // The pid is alive but runs a different exe: the
                    // game exited and the OS reused its pid.
                    let guard_path = running
                        .resolved_exe_path
                        .as_deref()
                        .unwrap_or(running.exe_path.as_str());
                    exe != Path::new(guard_path)
                } else {
                    false
                }
            }
            None => false,
        };
        if dead {
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
    use std::collections::HashMap;

    use super::*;

    /// Fake spawner: records spawns/kills, hands out fake pids.
    struct FakeSpawner {
        next_pid: u32,
        alive: RefCell<HashMap<u32, PathBuf>>,
        spawned: RefCell<Vec<PathBuf>>,
        killed: RefCell<Vec<u32>>,
        fail_terminate: RefCell<bool>,
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
                spawned: RefCell::new(Vec::new()),
                killed: RefCell::new(Vec::new()),
                fail_terminate: RefCell::new(false),
                resolve: RefCell::new(HashMap::new()),
                fail_exe_path: RefCell::new(false),
            }
        }

        fn kill_from_outside(&self, pid: u32) {
            self.alive.borrow_mut().remove(&pid);
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
            self.spawned.borrow_mut().push(exe.to_path_buf());
            Ok(pid)
        }

        fn is_alive(&self, pid: u32) -> bool {
            self.alive.borrow().contains_key(&pid)
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

        let mut launcher = GameLauncher::new(FakeSpawner::new());
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
        let mut launcher = GameLauncher::new(FakeSpawner::new());
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

        let mut launcher = GameLauncher::new(FakeSpawner::new());
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

        let mut launcher = GameLauncher::new(FakeSpawner::new());
        let running = launcher.launch("my-race", &ac).expect("launch");
        launcher.spawner.kill_from_outside(running.pid);
        assert!(launcher.refresh().is_none());
        assert!(launcher.running().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stop_with_nothing_running_is_an_error() {
        let mut launcher = GameLauncher::new(FakeSpawner::new());
        assert!(launcher.stop().is_err());
    }

    /// M1: a failed terminate on a still-alive process keeps the
    /// record, so the stop can be retried.
    #[test]
    fn stop_failed_terminate_keeps_record() {
        let dir = test_dir("stop-fail");
        let ac = fake_exe(&dir, "race.exe");

        let mut launcher = GameLauncher::new(FakeSpawner::new());
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

        let mut launcher = GameLauncher::new(FakeSpawner::new());
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

        let mut launcher = GameLauncher::new(FakeSpawner::new());
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
        let shortcut = dir.join("race.lnk");
        std::fs::write(&shortcut, b"fake shortcut").expect("write fixture lnk");

        let mut launcher = GameLauncher::new(FakeSpawner::new());
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
        let shortcut = dir.join("race.lnk");
        std::fs::write(&shortcut, b"fake shortcut").expect("write fixture lnk");

        let mut launcher = GameLauncher::new(FakeSpawner::new());
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
}
