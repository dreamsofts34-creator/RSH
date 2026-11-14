//! Game detection + launch (Phase F3 / SW-R7.3, Dashboard integration).
//!
//! Catalogue: Assetto Corsa, Assetto Corsa Competizione, iRacing,
//! DiRT Rally 2.0 (FEATURE-INVENTORY §6.1). The other matrix games
//! (rFactor 2, Automobilista 2, F1, Forza) are **not** in the catalogue:
//! their Steam/exe paths are unverified and are only added after a
//! Windows-PC check (see `docs/GAME-MATRIX.md`).
//!
//! * Detection reads Steam library folders (`steamapps/libraryfolders.vdf`)
//!   and checks for the game's exe under `steamapps/common/<dir>/`.
//!   iRacing is not on Steam — it is detected at its default install path.
//!   The library discovery is a pure function over a Steam root, so it is
//!   testable on Linux with fixture folders; only the *default* roots are
//!   Windows-only.
//! * Launch spawns the game exe directly and tracks the child pid.
//!   Exactly one game runs at a time. Stop kills **only** the tracked pid
//!   (SEC-E-10: never kill another same-name process).
//!
//! # Windows-only gaps (this Linux sandbox cannot verify them)
//!
//! * Default Steam roots and the iRacing default path are Windows paths
//!   (`#[cfg(windows)]`); on Linux detection finds nothing by default.
//! * The real launch on Windows spawns a Windows exe; the
//!   [`SystemSpawner`] is only exercised here through the fake spawner in
//!   tests. Real-launch verification is a Windows-PC item (gate).
//! * Assetto Corsa's helper (`acs.exe`) spawns as a *child* of the game:
//!   stopping the tracked pid does not enumerate children (that needs
//!   `Toolhelp32Snapshot` on Windows). If the helper outlives the game,
//!   the app re-attaches on Refresh and stops it separately — the C#
//!   behaviour, not yet implemented here (open item for Windows CI).

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};

use rsh_profiles::resolver::{
    GAME_ASSETTO_CORSA, GAME_ASSETTO_CORSA_COMPETIZIONE, GAME_DIRT_RALLY_2, GAME_IRACING,
};
use serde::{Deserialize, Serialize};

/// One catalogue game.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Game {
    /// Game id (matches the `GAME_*` constants in `rsh-profiles`).
    pub id: &'static str,
    /// Display name.
    pub name: &'static str,
    /// Steam app id, when the game is on Steam (`None` for iRacing).
    pub steam_app_id: Option<u32>,
    /// Exe file names, first one is the main exe.
    pub exe_file_names: &'static [&'static str],
    /// Install directory names under `steamapps/common/` (Steam games).
    pub install_dir_names: &'static [&'static str],
    /// Helper exe spawned by the game, if any (AC: `acs.exe`).
    pub helper_exe: Option<&'static str>,
}

/// The launcher catalogue (FEATURE-INVENTORY §6.1): exactly four games.
pub const KNOWN_GAMES: &[Game] = &[
    Game {
        id: GAME_ASSETTO_CORSA,
        name: "Assetto Corsa",
        steam_app_id: Some(244_210),
        exe_file_names: &["assettocorsa.exe"],
        install_dir_names: &["assettocorsa"],
        helper_exe: Some("acs.exe"),
    },
    Game {
        id: GAME_ASSETTO_CORSA_COMPETIZIONE,
        name: "Assetto Corsa Competizione",
        steam_app_id: Some(805_550),
        exe_file_names: &["acc.exe"],
        install_dir_names: &["Assetto Corsa Competizione"],
        helper_exe: None,
    },
    Game {
        id: GAME_IRACING,
        name: "iRacing",
        steam_app_id: None,
        exe_file_names: &["iRacingSim64DX11.exe"],
        install_dir_names: &[],
        helper_exe: None,
    },
    Game {
        id: GAME_DIRT_RALLY_2,
        name: "DiRT Rally 2.0",
        steam_app_id: Some(690_790),
        exe_file_names: &["dirt2.exe"],
        install_dir_names: &["DiRT Rally 2.0"],
        helper_exe: None,
    },
];

/// Finds a catalogue game by id.
#[must_use]
pub fn game_by_id(id: &str) -> Option<&'static Game> {
    KNOWN_GAMES.iter().find(|game| game.id == id)
}

/// Default Steam install roots.
///
/// Windows: the two standard install locations. Other platforms: empty
/// (Steam library detection is Windows-only; callers pass explicit roots
/// in tests).
#[must_use]
pub fn default_steam_roots() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        vec![
            PathBuf::from(r"C:\Program Files (x86)\Steam"),
            PathBuf::from(r"C:\Program Files\Steam"),
        ]
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

/// Default iRacing install paths (Windows only; iRacing is not on Steam).
#[must_use]
pub fn default_iracing_paths() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        vec![
            PathBuf::from(r"C:\Program Files (x86)\iRacing\iRacingSim64DX11.exe"),
            PathBuf::from(r"C:\Program Files\iRacing\iRacingSim64DX11.exe"),
        ]
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

/// Discovers Steam library folders for one Steam install root.
///
/// Reads `<root>/steamapps/libraryfolders.vdf` and returns the root
/// itself plus every `"path"` listed in the file (deduped, existing dirs
/// only). A missing or unreadable VDF is not an error — the root alone is
/// still a library. The VDF parse is deliberately minimal: it only looks
/// for `"path"` keys and their quoted value on the same line.
///
/// # Errors
///
/// Never errors: unreadable input yields fewer libraries, never a failure.
#[must_use]
pub fn discover_steam_libraries(steam_root: &Path) -> Vec<PathBuf> {
    let mut libraries: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let push = |path: PathBuf, libraries: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>| {
        if path.is_dir() && seen.insert(path.clone()) {
            libraries.push(path);
        }
    };
    push(steam_root.to_path_buf(), &mut libraries, &mut seen);

    let vdf = steam_root.join("steamapps").join("libraryfolders.vdf");
    if let Ok(text) = std::fs::read_to_string(vdf) {
        for path in parse_libraryfolders_vdf(&text) {
            push(path, &mut libraries, &mut seen);
        }
    }
    libraries
}

/// Extracts the `"path"` values from a `libraryfolders.vdf` document.
fn parse_libraryfolders_vdf(text: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for line in text.lines() {
        // Split on quotes, dropping empty and whitespace-only parts:
        // `"path"  "C:\Games"` yields ["path", "C:\Games"].
        let mut quoted = line.split('"').filter(|part| !part.trim().is_empty());
        let mut in_path_value = false;
        for part in quoted.by_ref() {
            if part.trim() == "path" {
                in_path_value = true;
                break;
            }
        }
        if !in_path_value {
            continue;
        }
        if let Some(value) = quoted.next() {
            // VDF escapes backslashes: `C:\\Games\\Steam`.
            let unescaped = value.replace("\\\\", "\\");
            paths.push(PathBuf::from(unescaped));
        }
    }
    paths
}

/// Finds a game's exe inside the given Steam libraries, if installed.
///
/// Checks `<library>/steamapps/common/<install dir>/<exe>` for every
/// install dir / exe combination. iRacing has no Steam install dirs and
/// is never found here — use [`detect_game`] for it.
#[must_use]
pub fn find_game_exe(libraries: &[PathBuf], game: &Game) -> Option<PathBuf> {
    for library in libraries {
        for dir in game.install_dir_names {
            for exe in game.exe_file_names {
                let candidate = library.join("steamapps").join("common").join(dir).join(exe);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

/// Detection result for one catalogue game.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DetectedGame {
    /// Game id (one of the `GAME_*` constants).
    pub game_id: String,
    /// Display name.
    pub name: String,
    /// Steam app id, when the game is on Steam.
    pub steam_app_id: Option<u32>,
    /// True when the exe was found on disk.
    pub installed: bool,
    /// Full path to the game exe, when installed.
    pub exe_path: Option<String>,
    /// Profile bound to this game, if the user bound one (`None` = the
    /// game profile / default applies; set by the caller, not by detection).
    pub bound_profile_id: Option<String>,
}

impl DetectedGame {
    /// Builds a not-installed entry for a game.
    #[must_use]
    pub fn not_installed(game: &Game) -> Self {
        Self {
            game_id: game.id.to_owned(),
            name: game.name.to_owned(),
            steam_app_id: game.steam_app_id,
            installed: false,
            exe_path: None,
            bound_profile_id: None,
        }
    }

    /// Binds a profile to this game. Returns `self` for chaining.
    #[must_use]
    pub fn with_bound_profile(mut self, profile_id: impl Into<String>) -> Self {
        self.bound_profile_id = Some(profile_id.into());
        self
    }
}

/// Detects one game: Steam exe search, plus the iRacing default path
/// when the game is iRacing.
#[must_use]
pub fn detect_game(game: &Game, steam_roots: &[PathBuf]) -> DetectedGame {
    let mut libraries: Vec<PathBuf> = Vec::new();
    for root in steam_roots {
        libraries.extend(discover_steam_libraries(root));
    }
    if let Some(exe) = find_game_exe(&libraries, game) {
        return DetectedGame {
            game_id: game.id.to_owned(),
            name: game.name.to_owned(),
            steam_app_id: game.steam_app_id,
            installed: true,
            exe_path: Some(exe.to_string_lossy().into_owned()),
            bound_profile_id: None,
        };
    }
    if game.id == GAME_IRACING {
        for candidate in default_iracing_paths() {
            if candidate.is_file() {
                return DetectedGame {
                    game_id: game.id.to_owned(),
                    name: game.name.to_owned(),
                    steam_app_id: None,
                    installed: true,
                    exe_path: Some(candidate.to_string_lossy().into_owned()),
                    bound_profile_id: None,
                };
            }
        }
    }
    DetectedGame::not_installed(game)
}

/// Detects every catalogue game for the given Steam roots.
#[must_use]
pub fn detect_all_games(steam_roots: &[PathBuf]) -> Vec<DetectedGame> {
    KNOWN_GAMES
        .iter()
        .map(|game| detect_game(game, steam_roots))
        .collect()
}

/// Why a launch failed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LaunchError {
    /// The game is not installed (no exe found).
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
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RunningGame {
    /// Game id.
    pub game_id: String,
    /// OS process id of the tracked game process.
    pub pid: u32,
    /// Full path to the exe that was launched.
    pub exe_path: String,
    /// Unix seconds when the game was launched.
    pub started_unix_secs: u64,
}

/// Spawns and supervises game processes. The production implementation
/// is [`SystemSpawner`]; tests inject a fake.
pub trait ProcessSpawner {
    /// Spawns an exe and returns its pid.
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
}

#[cfg(windows)]
mod win32 {
    //! Minimal raw Win32 FFI for process liveness / termination.
    //! (Same rationale as the watcher module: no `windows`-crate
    //! dependency yet; the SW-R7.1 spike may replace this.)

    /// `HANDLE` as an integer-sized value.
    pub type Handle = isize;

    unsafe extern "C" {
        /// Opens a process handle, or 0 on failure.
        pub fn OpenProcess(desired_access: u32, inherit_handle: i32, pid: u32) -> Handle;
        /// Writes the exit code; returns non-zero on success.
        pub fn GetExitCodeProcess(process: Handle, exit_code: *mut u32) -> i32;
        /// Terminates a process; returns non-zero on success.
        pub fn TerminateProcess(process: Handle, exit_code: u32) -> i32;
        /// Closes a handle; returns non-zero on success.
        pub fn CloseHandle(handle: Handle) -> i32;
    }

    /// `PROCESS_QUERY_LIMITED_INFORMATION` (0x1000).
    pub const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    /// `PROCESS_TERMINATE` (0x0001).
    pub const PROCESS_TERMINATE: u32 = 0x0001;
    /// `STILL_ACTIVE` (259).
    pub const STILL_ACTIVE: u32 = 259;
}

/// Spawns real OS processes.
///
/// * Spawn uses [`std::process::Command`] (works on Windows and Linux).
/// * Liveness/termination use raw Win32 FFI on Windows and `/proc` +
///   the `kill` command elsewhere.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemSpawner;

impl ProcessSpawner for SystemSpawner {
    fn spawn(&mut self, exe: &Path) -> Result<u32, String> {
        #[cfg(windows)]
        {
            std::process::Command::new(exe)
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
                let ok = win32::GetExitCodeProcess(handle, &mut exit_code);
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
            // SAFETY: plain Win32 call with a checked return; the handle
            // is always closed.
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
}

/// Launches and tracks games: exactly one at a time.
///
/// The launcher owns no profile state — it only reports which game is
/// running. The watcher / app layer binds the game's profile (see
/// [`DetectedGame::bound_profile_id`]).
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

    /// Launches a detected game.
    ///
    /// # Errors
    ///
    /// [`LaunchError::NotInstalled`] when the game has no exe path,
    /// [`LaunchError::AlreadyRunning`] when another game is tracked as
    /// running, [`LaunchError::SpawnFailed`] when the OS refuses to start it.
    pub fn launch(&mut self, game: &DetectedGame) -> Result<RunningGame, LaunchError> {
        let _ = self.refresh();
        if let Some(running) = &self.running {
            return Err(LaunchError::AlreadyRunning(running.game_id.clone()));
        }
        let exe_path = game
            .exe_path
            .as_ref()
            .ok_or_else(|| LaunchError::NotInstalled(game.game_id.clone()))?;
        let pid = self
            .spawner
            .spawn(Path::new(exe_path))
            .map_err(LaunchError::SpawnFailed)?;
        let started_unix_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs());
        let running = RunningGame {
            game_id: game.game_id.clone(),
            pid,
            exe_path: exe_path.clone(),
            started_unix_secs,
        };
        self.running = Some(running.clone());
        Ok(running)
    }

    /// Re-checks whether the tracked game is still alive; clears the
    /// record when it exited on its own. Returns the current record.
    #[must_use]
    pub fn refresh(&mut self) -> Option<&RunningGame> {
        let dead = match &self.running {
            Some(running) => !self.spawner.is_alive(running.pid),
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
    /// # Errors
    ///
    /// Returns an error message when nothing is running or the tracked
    /// process could not be terminated. The record is cleared either way
    /// when termination was attempted and the process is gone.
    pub fn stop(&mut self) -> Result<RunningGame, String> {
        let running = self
            .running
            .take()
            .ok_or_else(|| "no game is running".to_owned())?;
        self.spawner.terminate(running.pid)?;
        Ok(running)
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
    }

    impl FakeSpawner {
        fn new() -> Self {
            Self {
                next_pid: 1000,
                alive: RefCell::new(HashMap::new()),
                spawned: RefCell::new(Vec::new()),
                killed: RefCell::new(Vec::new()),
            }
        }

        fn kill_from_outside(&self, pid: u32) {
            self.alive.borrow_mut().remove(&pid);
        }
    }

    impl ProcessSpawner for FakeSpawner {
        fn spawn(&mut self, exe: &Path) -> Result<u32, String> {
            let pid = self.next_pid;
            self.next_pid += 1;
            self.alive.borrow_mut().insert(pid, exe.to_path_buf());
            self.spawned.borrow_mut().push(exe.to_path_buf());
            Ok(pid)
        }

        fn is_alive(&self, pid: u32) -> bool {
            self.alive.borrow().contains_key(&pid)
        }

        fn terminate(&mut self, pid: u32) -> Result<(), String> {
            if self.alive.borrow_mut().remove(&pid).is_some() {
                self.killed.borrow_mut().push(pid);
                Ok(())
            } else {
                Err(format!("pid {pid} is not alive"))
            }
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

    /// Builds a fake Steam root with one extra library and an AC exe.
    fn steam_fixture() -> (PathBuf, PathBuf) {
        let root = test_dir("steam");
        let lib2 = root.join("lib2");
        for dir in [
            root.join("steamapps").join("common").join("assettocorsa"),
            lib2.join("steamapps").join("common").join("DiRT Rally 2.0"),
        ] {
            std::fs::create_dir_all(&dir).expect("create fixture dir");
        }
        std::fs::write(
            root.join("steamapps")
                .join("common")
                .join("assettocorsa")
                .join("assettocorsa.exe"),
            b"fake exe",
        )
        .expect("write fixture exe");
        std::fs::write(
            lib2.join("steamapps")
                .join("common")
                .join("DiRT Rally 2.0")
                .join("dirt2.exe"),
            b"fake exe",
        )
        .expect("write fixture exe");
        std::fs::write(
            root.join("steamapps").join("libraryfolders.vdf"),
            format!(
                "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}\n",
                lib2.to_string_lossy().replace('\\', "\\\\")
            ),
        )
        .expect("write fixture vdf");
        (root, lib2)
    }

    #[test]
    fn vdf_discovers_extra_library_and_exe_is_found() {
        let (root, lib2) = steam_fixture();
        let libraries = discover_steam_libraries(&root);
        assert!(libraries.contains(&root), "root is itself a library");
        assert!(libraries.contains(&lib2), "VDF path is discovered");

        let ac = game_by_id(GAME_ASSETTO_CORSA).expect("AC in catalogue");
        let found = find_game_exe(&libraries, ac).expect("AC exe found");
        assert!(found.ends_with("assettocorsa.exe"));

        let dirt = game_by_id(GAME_DIRT_RALLY_2).expect("DR2 in catalogue");
        let found = find_game_exe(&libraries, dirt).expect("DR2 exe found in lib2");
        assert!(found.ends_with("dirt2.exe"));

        let acc = game_by_id(GAME_ASSETTO_CORSA_COMPETIZIONE).expect("ACC in catalogue");
        assert_eq!(find_game_exe(&libraries, acc), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn detect_all_games_reports_installed_flags() {
        let (root, _lib2) = steam_fixture();
        let games = detect_all_games(std::slice::from_ref(&root));
        assert_eq!(games.len(), 4);
        let by_id = |id: &str| games.iter().find(|game| game.game_id == id).unwrap();
        assert!(by_id(GAME_ASSETTO_CORSA).installed);
        assert!(by_id(GAME_DIRT_RALLY_2).installed);
        assert!(!by_id(GAME_ASSETTO_CORSA_COMPETIZIONE).installed);
        assert!(!by_id(GAME_IRACING).installed, "iRacing: no Steam exe");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_vdf_still_detects_root_library() {
        let root = test_dir("no-vdf");
        std::fs::create_dir_all(
            root.join("steamapps")
                .join("common")
                .join("Assetto Corsa Competizione"),
        )
        .expect("create fixture dir");
        std::fs::write(
            root.join("steamapps")
                .join("common")
                .join("Assetto Corsa Competizione")
                .join("acc.exe"),
            b"fake exe",
        )
        .expect("write fixture exe");
        let libraries = discover_steam_libraries(&root);
        assert_eq!(libraries, vec![root.clone()]);
        let acc = game_by_id(GAME_ASSETTO_CORSA_COMPETIZIONE).expect("ACC in catalogue");
        assert!(find_game_exe(&libraries, acc).is_some());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn launch_tracks_running_and_rejects_second_game() {
        let (root, _lib2) = steam_fixture();
        let games = detect_all_games(std::slice::from_ref(&root));
        let ac = games
            .iter()
            .find(|game| game.game_id == GAME_ASSETTO_CORSA)
            .unwrap()
            .clone();
        let dirt = games
            .iter()
            .find(|game| game.game_id == GAME_DIRT_RALLY_2)
            .unwrap()
            .clone();

        let mut launcher = GameLauncher::new(FakeSpawner::new());
        let running = launcher.launch(&ac).expect("AC launches");
        assert_eq!(running.game_id, GAME_ASSETTO_CORSA);
        assert_eq!(
            launcher.running().map(|game| game.game_id.as_str()),
            Some(GAME_ASSETTO_CORSA)
        );

        let second = launcher.launch(&dirt);
        assert_eq!(
            second,
            Err(LaunchError::AlreadyRunning(GAME_ASSETTO_CORSA.to_owned()))
        );

        let stopped = launcher.stop().expect("stop works");
        assert_eq!(stopped.game_id, GAME_ASSETTO_CORSA);
        assert!(launcher.running().is_none());

        // After stopping, another game can launch.
        assert!(launcher.launch(&dirt).is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn launch_not_installed_is_an_error() {
        let acc = DetectedGame::not_installed(
            game_by_id(GAME_ASSETTO_CORSA_COMPETIZIONE).expect("ACC in catalogue"),
        );
        let mut launcher = GameLauncher::new(FakeSpawner::new());
        assert_eq!(
            launcher.launch(&acc),
            Err(LaunchError::NotInstalled(
                GAME_ASSETTO_CORSA_COMPETIZIONE.to_owned()
            ))
        );
    }

    #[test]
    fn stop_kills_only_the_tracked_pid() {
        let (root, _lib2) = steam_fixture();
        let games = detect_all_games(std::slice::from_ref(&root));
        let ac = games
            .iter()
            .find(|game| game.game_id == GAME_ASSETTO_CORSA)
            .unwrap()
            .clone();

        let mut launcher = GameLauncher::new(FakeSpawner::new());
        // A second, unrelated process with pid 2000 is alive in the fake.
        launcher
            .spawner
            .alive
            .borrow_mut()
            .insert(2000, PathBuf::from("other-assettocorsa.exe"));
        let running = launcher.launch(&ac).expect("AC launches");
        launcher.stop().expect("stop works");
        assert!(
            launcher.spawner.alive.borrow().contains_key(&2000),
            "SEC-E-10: an unrelated pid must survive the stop"
        );
        assert_eq!(launcher.spawner.killed.borrow().as_slice(), [running.pid]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn refresh_clears_a_game_that_exited_on_its_own() {
        let (root, _lib2) = steam_fixture();
        let games = detect_all_games(std::slice::from_ref(&root));
        let ac = games
            .iter()
            .find(|game| game.game_id == GAME_ASSETTO_CORSA)
            .unwrap()
            .clone();

        let mut launcher = GameLauncher::new(FakeSpawner::new());
        let running = launcher.launch(&ac).expect("AC launches");
        launcher.spawner.kill_from_outside(running.pid);
        assert!(launcher.refresh().is_none());
        assert!(launcher.running().is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn stop_with_nothing_running_is_an_error() {
        let mut launcher = GameLauncher::new(FakeSpawner::new());
        assert!(launcher.stop().is_err());
    }
}
