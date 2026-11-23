//! Single-instance (LOGIC-FIX-PLAN §2 rule 6): the daemon is
//! single-instance; a second launch signals the running daemon and
//! exits.
//!
//! * Windows: a named mutex (`Global\RSH-Daemon-<user>` — the real
//!   mutex name is built from the current username when available).
//!   A second instance finds the mutex held and exits after
//!   signalling via IPC. R3-06: `Global\` needs
//!   `SeCreateGlobalPrivilege`, which a non-elevated daemon does
//!   not hold — on access-denied the acquire falls back to a
//!   per-session `Local\` mutex (after probing `Global\`, so an
//!   elevated first daemon is still seen as already-running).
//! * Linux: a lock file `<data_dir>/rsh-daemon.pid` holding the
//!   daemon's pid. A second instance reads the pid and checks the
//!   process is a live `rsh-daemon` (its `/proc/<pid>/cmdline` must
//!   name it — a recycled pid belonging to another program, or a
//!   zombie, is stale and reclaimed), signals via IPC, and exits.
//!   A stale pid file (dead pid) is reclaimed.
//!
//! The UI-open hook lands in Phase 3: for now the second instance
//! signals with an IPC `ping` (proving the running daemon answers)
//! and exits — the running daemon logs the signal. Phase 3 adds an
//! `open-ui` IPC method the second instance will call instead.

use std::path::PathBuf;
use std::time::Duration;

/// How long a second instance waits for the lock holder's IPC to
/// answer before deciding (F15, v2.0.3): a slow-starting first daemon
/// (store open + runtime before the pipe exists) takes >1.5–2 s,
/// which the old 3 × 500 ms window misread as hung.
pub const PING_ATTEMPTS: usize = 20;
/// Delay between liveness pings (F15).
pub const PING_INTERVAL: Duration = Duration::from_millis(500);

/// Verdict after probing an already-running daemon (F15).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlreadyRunningVerdict {
    /// IPC answers: the owner is alive.
    Alive,
    /// The IPC endpoint exists but never answered: really hung.
    Hung,
    /// The IPC endpoint never appeared: the owner is still starting
    /// up — NOT hung, and no fatal status may be written for it.
    Starting,
}

/// Classify an already-running daemon from the ping outcome (F15):
/// a daemon whose IPC endpoint never existed is starting, not hung.
#[must_use]
pub fn classify_already_running(ping_ok: bool, endpoint_seen: bool) -> AlreadyRunningVerdict {
    if ping_ok {
        AlreadyRunningVerdict::Alive
    } else if endpoint_seen {
        AlreadyRunningVerdict::Hung
    } else {
        AlreadyRunningVerdict::Starting
    }
}

/// Whether the daemon's IPC endpoint exists right now (F15): the
/// Unix socket file on Linux; on Windows, whether a named-pipe
/// instance exists (a listener waiting, or all instances busy —
/// only "no such pipe" means the daemon has not bound yet).
#[must_use]
pub fn ipc_endpoint_exists(socket: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        socket.exists()
    }
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, GetLastError};
        use windows::Win32::System::Pipes::WaitNamedPipeW;
        use windows::core::PCWSTR;
        let _ = socket;
        let name: Vec<u16> = format!("{}\0", rsh_ipc::IPC_PIPE_NAME)
            .encode_utf16()
            .collect();
        let available = unsafe { WaitNamedPipeW(PCWSTR(name.as_ptr()), 0) };
        if available.as_bool() {
            return true;
        }
        // No free instance — but the pipe may still exist with all
        // instances busy. Only "file not found" means no pipe at all.
        unsafe { GetLastError() != ERROR_FILE_NOT_FOUND }
    }
}

/// File holding the lock owner's pid: `<data_dir>/rsh-daemon.pid`
/// (the same file the Linux path uses as its lock).
fn pid_file_path(data_dir: &std::path::Path) -> PathBuf {
    data_dir.join("rsh-daemon.pid")
}

/// Record our pid in the pid file (F22, Windows First path; also
/// the Linux lock file — see [`acquire`]).
///
/// Only used by the Windows `acquire` and by tests: on Linux the
/// production path uses [`claim_pid_file`] instead, so without the
/// cfg this is dead code there (`-D warnings` fails the gate).
///
/// # Errors
///
/// Returns the write error when the pid file cannot be written.
#[cfg(any(test, windows))]
fn write_pid_file(data_dir: &std::path::Path) -> std::io::Result<PathBuf> {
    let path = pid_file_path(data_dir);
    let _ = std::fs::create_dir_all(data_dir);
    std::fs::write(&path, format!("{}\n", std::process::id()))?;
    Ok(path)
}

/// Read the pid recorded by the lock owner, if any (F22).
/// `None` when the file is missing or unparseable — the caller
/// treats that as "owner unknown", never as an error.
#[must_use]
pub fn recorded_pid(data_dir: &std::path::Path) -> Option<u32> {
    std::fs::read_to_string(pid_file_path(data_dir))
        .ok()?
        .trim()
        .parse::<u32>()
        .ok()
}

/// Which namespace the Windows single-instance mutex lives in
/// (R3-06): `Global\` is cross-session but needs
/// `SeCreateGlobalPrivilege` (administrators / services only — a
/// non-elevated daemon does not hold it); `Local\` is per-session
/// and needs no privilege.
#[cfg(windows)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MutexScope {
    /// Cross-session `Global\` mutex.
    Global,
    /// Per-session `Local\` fallback.
    Local,
}

/// The running-daemon lock, held for the daemon's lifetime.
pub enum SingleInstance {
    /// Linux pid-file lock (path + our pid).
    #[cfg(unix)]
    PidFile {
        /// Lock file path.
        path: PathBuf,
    },
    /// Windows named mutex (guard kept alive).
    #[cfg(windows)]
    Mutex {
        /// The mutex handle (kept alive; dropping it releases).
        #[allow(dead_code)]
        handle: windows::Win32::Foundation::HANDLE,
        /// Pid file recording the lock owner's pid (F22, v2.0.3):
        /// the mutex itself carries no pid, so a second instance
        /// could never name the owner (`AlreadyRunning(None)`
        /// always — the user log proves it: every `DaemonHung`
        /// fatal says `pid None`). Informational only — the mutex
        /// is authoritative; a stale pid file is overwritten by the
        /// next First and removed on clean release.
        pid_path: PathBuf,
        /// Which namespace the mutex was created in (R3-06).
        scope: MutexScope,
    },
}

/// Outcome of trying to become the single instance.
pub enum AcquireOutcome {
    /// We are the first instance; the lock is held in `SingleInstance`.
    First(SingleInstance),
    /// Another daemon is already running (its pid, when known).
    AlreadyRunning(Option<u32>),
}

/// Try to become the single daemon instance.
///
/// # Errors
///
/// Returns an error message when the lock file / mutex cannot be
/// created or read (not when another instance holds it — that is
/// [`AcquireOutcome::AlreadyRunning`]).
///
/// # F17 (v2.0.3) — `O_EXCL` claim, no check-then-create
///
/// The old code read the pid file, decided it was stale, and then
/// truncated it with [`write_pid_file`]: two daemons launched at
/// once could both pass the check and both become "First" (the
/// duplicate-daemon half of the restart loop). The claim is now
/// atomic ([`claim_pid_file`]: `O_EXCL` create — exactly one
/// caller in the system wins) and every loser re-reads the file:
/// a live winner means [`AcquireOutcome::AlreadyRunning`], a stale
/// file is removed and the claim retried (bounded, so a
/// pathological directory fails loudly instead of spinning).
#[cfg(unix)]
pub fn acquire(data_dir: &std::path::Path) -> Result<AcquireOutcome, String> {
    let _ = std::fs::create_dir_all(data_dir);
    for _ in 0..16 {
        // Live holder (whoever won the claim) — never steal it.
        if let Some(pid) = recorded_pid(data_dir)
            && pid_is_live_rsh_daemon(pid)
        {
            return Ok(AcquireOutcome::AlreadyRunning(Some(pid)));
        }
        match claim_pid_file(data_dir) {
            Ok(path) => {
                return Ok(AcquireOutcome::First(SingleInstance::PidFile { path }));
            }
            Err(ClaimError::Held) => {
                // Lost the claim race. The winner may still be
                // writing its pid (create_new done, writeln
                // pending) — the old code waited once (10 ms) and
                // then REMOVED the file when the re-check saw no
                // live pid, so a winner stalled >10 ms between
                // create and write lost its claim and the loser
                // became First too (tun-racefix: the rig's UI race
                // hit this in the UI claim 3/3, and the daemon
                // claim the same way once — two live daemons, both
                // bound IPC, in the quiet control run). Wait for
                // the write instead, bounded
                // (`PID_WRITE_GRACE_POLLS`): only remove when the
                // file names a dead pid, vanishes, or is still
                // empty after the full grace (an owner that died
                // mid-write). Never remove a file that might belong
                // to a live winner. Bounded — a file that can never
                // be claimed fails loudly below instead of spinning.
                let mut waited: u32 = 0;
                loop {
                    match daemon_pid_file_state(data_dir) {
                        DaemonPidFileState::Live(pid) => {
                            return Ok(AcquireOutcome::AlreadyRunning(Some(pid)));
                        }
                        DaemonPidFileState::Dead | DaemonPidFileState::Missing => {
                            let _ = std::fs::remove_file(pid_file_path(data_dir));
                            break;
                        }
                        DaemonPidFileState::Empty => {
                            waited += 1;
                            if waited >= PID_WRITE_GRACE_POLLS {
                                // Owner died mid-write (or the file is
                                // garbage): reclaim it and retry the claim.
                                let _ = std::fs::remove_file(pid_file_path(data_dir));
                                break;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(10));
                        }
                    }
                }
            }
            Err(ClaimError::Io(error)) => {
                return Err(format!(
                    "cannot create daemon lock file {}: {error}",
                    pid_file_path(data_dir).display()
                ));
            }
        }
    }
    Err(format!(
        "cannot acquire daemon lock file {}: claim race did not settle",
        pid_file_path(data_dir).display()
    ))
}

/// How long a claim loser waits for the winner's pid to appear in
/// the pid file before concluding the file is stale (tun-racefix):
/// 200 polls x 10 ms = 2 s. The winner writes its pid within
/// microseconds of creating the file, so 2 s is already
/// pathological; it only covers a winner stalled between
/// `create_new` and `writeln!` by scheduler / filesystem stalls
/// under load.
#[cfg(unix)]
const PID_WRITE_GRACE_POLLS: u32 = 200;

/// What the daemon pid file says right now (see [`acquire`]).
/// `Empty` covers a file the winner created but has not written
/// yet (create_new done, writeln pending) as well as garbage
/// content — both mean "no pid to check", never "no owner".
#[cfg(unix)]
enum DaemonPidFileState {
    /// Names a live `rsh-daemon` other than this process.
    Live(u32),
    /// Names a dead pid (stale — safe to remove).
    Dead,
    /// Exists but holds no parsable pid (winner mid-write).
    Empty,
    /// No file (removed by a rival, or never created).
    Missing,
}

/// Classify the pid file's current content.
#[cfg(unix)]
fn daemon_pid_file_state(data_dir: &std::path::Path) -> DaemonPidFileState {
    let path = pid_file_path(data_dir);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return DaemonPidFileState::Missing;
        }
        Err(_) => return DaemonPidFileState::Empty,
    };
    let Ok(pid) = text.trim().parse::<u32>() else {
        return DaemonPidFileState::Empty;
    };
    if pid == std::process::id() {
        // Our own pid (a stale file naming a recycled pid): we
        // have not claimed yet, so the file cannot be our claim
        // — it is stale and safe to remove at once.
        return DaemonPidFileState::Dead;
    }
    if pid_is_live_rsh_daemon(pid) {
        DaemonPidFileState::Live(pid)
    } else {
        DaemonPidFileState::Dead
    }
}

/// Why [`claim_pid_file`] did not claim the lock.
#[cfg(unix)]
enum ClaimError {
    /// Another caller owns the path (lost the `O_EXCL` race).
    Held,
    /// A real I/O failure (not a lost race).
    Io(std::io::Error),
}

/// Atomically claim the pid file for this process (F17):
/// `O_EXCL` create, so exactly one caller in the whole system
/// wins. Returns the path for the winner; [`ClaimError::Held`]
/// when another caller already owns the path. Split out of
/// [`acquire`] so the atomicity itself is unit-testable without a
/// live daemon process.
#[cfg(unix)]
fn claim_pid_file(data_dir: &std::path::Path) -> Result<PathBuf, ClaimError> {
    use std::io::Write as _;
    let path = pid_file_path(data_dir);
    let _ = std::fs::create_dir_all(data_dir);
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut file) => {
            writeln!(file, "{}", std::process::id()).map_err(ClaimError::Io)?;
            Ok(path)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Err(ClaimError::Held),
        Err(error) => Err(ClaimError::Io(error)),
    }
}

/// Try to become the single daemon instance (Windows named mutex).
///
/// R3-06: the `Global\` create needs `SeCreateGlobalPrivilege`,
/// which a non-elevated daemon (the production autostart case)
/// does not hold — the create fails access-denied. The old code
/// treated that as fatal with no fallback, so a standard-user /
/// non-elevated first daemon could never start. Now: on
/// access-denied, fall back to a per-session `Local\` mutex (no
/// privilege needed), after probing the `Global\` mutex — an
/// elevated daemon may already hold it, and a second daemon must
/// not become "First" in the Local scope while the Global holder
/// is alive. The scope taken is recorded on [`SingleInstance`]
/// (see [`SingleInstance::mutex_scope`]).
///
/// # Errors
///
/// Returns an error message when the mutex cannot be created.
#[cfg(windows)]
pub fn acquire(data_dir: &std::path::Path) -> Result<AcquireOutcome, String> {
    use windows::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, GetLastError};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::core::PCWSTR;
    // Per-user mutex name: the username keeps two users' daemons
    // independent on a shared machine.
    let user = std::env::var("USERNAME").unwrap_or_else(|_| "user".to_string());
    let name: Vec<u16> = format!("Global\\RSH-Daemon-{user}\0")
        .encode_utf16()
        .collect();
    // SAFETY: plain Win32 call with a valid NUL-terminated name.
    match unsafe { CreateMutexW(None, true, PCWSTR(name.as_ptr())) } {
        Ok(handle) => {
            let already =
                unsafe { GetLastError() } == windows::Win32::Foundation::ERROR_ALREADY_EXISTS;
            if already {
                let _ = unsafe { CloseHandle(handle) };
                // F22: the mutex carries no pid — read the pid file the
                // owner wrote, so the second instance can name it.
                return Ok(AcquireOutcome::AlreadyRunning(recorded_pid(data_dir)));
            }
            let pid_path = write_pid_file(data_dir).unwrap_or_else(|_| pid_file_path(data_dir));
            Ok(AcquireOutcome::First(SingleInstance::Mutex {
                handle,
                pid_path,
                scope: MutexScope::Global,
            }))
        }
        Err(error) => {
            if unsafe { GetLastError() } == ERROR_ACCESS_DENIED {
                return acquire_local_mutex(data_dir, &user);
            }
            Err(format!("cannot create daemon mutex: {error}"))
        }
    }
}

/// Per-session `Local\` fallback for [`acquire`] (R3-06): no
/// privilege needed. Before creating it, probe the `Global\`
/// mutex: an elevated daemon may already hold it (its first
/// create succeeded where ours failed with access-denied), and a
/// second daemon must not become "First" in the Local scope while
/// the Global holder is alive. A probe failure means no Global
/// holder we can open — either none exists (fresh non-elevated
/// first daemon) or a foreign user's daemon holds it (in which
/// case a per-user Local daemon is the correct outcome) — so any
/// probe failure falls through to the Local create.
#[cfg(windows)]
fn acquire_local_mutex(data_dir: &std::path::Path, user: &str) -> Result<AcquireOutcome, String> {
    use windows::Win32::Foundation::{CloseHandle, GetLastError};
    use windows::Win32::System::Threading::{
        CreateMutexW, OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE,
    };
    use windows::core::PCWSTR;
    let global_name: Vec<u16> = format!("Global\\RSH-Daemon-{user}\0")
        .encode_utf16()
        .collect();
    // SAFETY: plain Win32 probe with a valid NUL-terminated name.
    if let Ok(handle) = unsafe {
        OpenMutexW(
            SYNCHRONIZATION_SYNCHRONIZE,
            false,
            PCWSTR(global_name.as_ptr()),
        )
    } {
        // A daemon already holds the Global\ mutex (elevated first
        // daemon) — we are second, whatever our own scope would be.
        let _ = unsafe { CloseHandle(handle) };
        return Ok(AcquireOutcome::AlreadyRunning(recorded_pid(data_dir)));
    }
    let name: Vec<u16> = format!("Local\\RSH-Daemon-{user}\0")
        .encode_utf16()
        .collect();
    // SAFETY: plain Win32 call with a valid NUL-terminated name.
    let handle = unsafe {
        CreateMutexW(None, true, PCWSTR(name.as_ptr()))
            .map_err(|e| format!("cannot create daemon mutex (Local fallback): {e}"))?
    };
    let already = unsafe { GetLastError() } == windows::Win32::Foundation::ERROR_ALREADY_EXISTS;
    if already {
        let _ = unsafe { CloseHandle(handle) };
        return Ok(AcquireOutcome::AlreadyRunning(recorded_pid(data_dir)));
    }
    let pid_path = write_pid_file(data_dir).unwrap_or_else(|_| pid_file_path(data_dir));
    Ok(AcquireOutcome::First(SingleInstance::Mutex {
        handle,
        pid_path,
        scope: MutexScope::Local,
    }))
}

/// Whether `pid` is a live `rsh-daemon` process (Linux pid-file
/// check). `/proc/<pid>` merely existing is not enough: pids are
/// recycled, so a stale file can name an unrelated live process
/// (then the new daemon would exit believing a daemon is running,
/// with no tray and no IPC), and a zombie still has a `/proc` entry
/// after its daemon died. Both are stale. The command line must name
/// `rsh-daemon`; an unreadable command line is treated as live
/// (conservative — never steal a lock we cannot identify).
#[cfg(unix)]
fn pid_is_live_rsh_daemon(pid: u32) -> bool {
    use std::path::Path;
    if !Path::new(&format!("/proc/{pid}")).exists() {
        return false;
    }
    // Zombie: dead, but /proc still lists it. The state is the
    // character after the final `) ` of the comm field.
    if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        && let Some(state) = stat
            .rsplit(") ")
            .next()
            .and_then(|rest| rest.chars().next())
        && state == 'Z'
    {
        return false;
    }
    match std::fs::read(format!("/proc/{pid}/cmdline")) {
        Ok(cmdline) => String::from_utf8_lossy(&cmdline)
            .split('\0')
            .any(|arg| arg.contains("rsh-daemon")),
        Err(_) => true,
    }
}

impl SingleInstance {
    /// Which mutex namespace this lock lives in (R3-06). Windows-only
    /// by construction (the Mutex variant exists only on Windows),
    /// so this always has a value — no `Option` wrapper.
    #[cfg(windows)]
    #[must_use]
    pub fn mutex_scope(&self) -> MutexScope {
        match self {
            Self::Mutex { scope, .. } => *scope,
        }
    }

    /// Release the lock (pid file removed / mutex dropped). Called
    /// on clean shutdown; a crash leaves the pid file behind, and
    /// the next launch reclaims it via the `/proc` check.
    pub fn release(self) {
        #[cfg(unix)]
        {
            // On unix only the PidFile variant exists (the Mutex
            // variant is Windows-only), so this is a plain `let`.
            let SingleInstance::PidFile { path } = self;
            let _ = std::fs::remove_file(path);
        }
        #[cfg(windows)]
        {
            // Close the mutex handle: HANDLE has no Drop impl, so
            // dropping `self` alone would leak it. The pid file is
            // removed too (F22) — a stale file is harmless (the next
            // First overwrites it), but a clean release leaves no
            // trace of a dead owner behind.
            let SingleInstance::Mutex {
                handle, pid_path, ..
            } = self;
            let _ = unsafe { windows::Win32::Foundation::CloseHandle(handle) };
            let _ = std::fs::remove_file(pid_path);
        }
    }
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use std::os::unix::process::CommandExt;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("rsh-single-instance-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn write_pid(dir: &std::path::Path, pid: u32) {
        let mut file = std::fs::File::create(dir.join("rsh-daemon.pid")).expect("pid file");
        writeln!(file, "{pid}").expect("write pid");
    }

    #[test]
    fn classify_already_running_matrix() {
        assert_eq!(
            classify_already_running(true, false),
            AlreadyRunningVerdict::Alive
        );
        assert_eq!(
            classify_already_running(true, true),
            AlreadyRunningVerdict::Alive
        );
        assert_eq!(
            classify_already_running(false, true),
            AlreadyRunningVerdict::Hung
        );
        assert_eq!(
            classify_already_running(false, false),
            AlreadyRunningVerdict::Starting
        );
    }

    #[test]
    fn ping_window_covers_slow_start() {
        // 20 × 500 ms = 10 s: well past the 1.5–2 s a slow first
        // daemon needs before its pipe exists (F15).
        assert!(PING_ATTEMPTS as u128 * PING_INTERVAL.as_millis() >= 10_000);
    }

    #[test]
    fn ipc_endpoint_exists_tracks_socket_file() {
        let dir = temp_dir("endpoint");
        let socket = dir.join("rsh-daemon.sock");
        assert!(!ipc_endpoint_exists(&socket));
        std::fs::write(&socket, b"").expect("write socket stand-in");
        assert!(ipc_endpoint_exists(&socket));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recorded_pid_round_trip_and_garbage() {
        let dir = temp_dir("recorded-pid");
        assert_eq!(recorded_pid(&dir), None);
        std::fs::write(dir.join("rsh-daemon.pid"), "not-a-pid\n").expect("write");
        assert_eq!(recorded_pid(&dir), None);
        let path = write_pid_file(&dir).expect("pid file written");
        assert_eq!(recorded_pid(&dir), Some(std::process::id()));
        assert_eq!(path, dir.join("rsh-daemon.pid"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn claim_pid_file_exactly_one_winner_under_race() {
        // F17: the O_EXCL claim itself, 8 threads × 20 rounds.
        // With the old check-then-create (`File::create` truncate)
        // every thread "won"; with `create_new` exactly one wins
        // per round, deterministically — no live daemon needed.
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        for round in 0..20 {
            let dir = temp_dir(&format!("claim-race-{round}"));
            let wins = Arc::new(AtomicUsize::new(0));
            let mut handles = Vec::new();
            for _ in 0..8 {
                let dir = dir.clone();
                let wins = Arc::clone(&wins);
                handles.push(std::thread::spawn(move || {
                    if claim_pid_file(&dir).is_ok() {
                        wins.fetch_add(1, Ordering::SeqCst);
                    }
                }));
            }
            for handle in handles {
                handle.join().expect("claim thread panicked");
            }
            assert_eq!(
                wins.load(Ordering::SeqCst),
                1,
                "round {round}: exactly one O_EXCL claim winner"
            );
            assert_eq!(
                recorded_pid(&dir),
                Some(std::process::id()),
                "round {round}: the winner's pid is recorded"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn stale_pid_file_for_a_dead_process_is_reclaimed() {
        let dir = temp_dir("stale");
        // A pid that can never exist (past the kernel pid limit).
        write_pid(&dir, u32::MAX);
        assert!(matches!(acquire(&dir), Ok(AcquireOutcome::First(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recycled_pid_belonging_to_another_program_is_reclaimed() {
        let dir = temp_dir("recycled");
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");
        let pid = child.id();
        write_pid(&dir, pid);
        assert!(matches!(acquire(&dir), Ok(AcquireOutcome::First(_))));
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_pid_file_with_late_writer_is_not_stolen() {
        // tun-racefix: same shape as the UI claim — the winner's pid
        // file is empty between `create_new` and `writeln!`; a loser
        // must wait for the write, not remove the file and become a
        // second First. The writer lands 200 ms after the empty file
        // appears — past the old single 10 ms re-check, inside the
        // 2 s grace. FAILS on the old code (First), passes on the
        // fix (AlreadyRunning).
        let dir = temp_dir("late-writer");
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .arg0("rsh-daemon")
            .spawn()
            .expect("spawn fake rsh-daemon");
        let pid = child.id();
        std::fs::write(dir.join("rsh-daemon.pid"), "").expect("empty pid file");
        let dir2 = dir.clone();
        std::thread::spawn(move || {
            // 200 ms: the fake process has exec'd by then (its
            // cmdline names rsh-daemon), and 200 ms is past the old
            // 10 ms re-check but far inside the 2 s grace.
            std::thread::sleep(std::time::Duration::from_millis(200));
            std::fs::write(dir2.join("rsh-daemon.pid"), pid.to_string()).expect("write pid");
        });
        assert!(matches!(
            acquire(&dir),
            Ok(AcquireOutcome::AlreadyRunning(Some(p))) if p == pid
        ));
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_pid_file_with_no_writer_is_reclaimed() {
        // An owner that died between create and write leaves an empty
        // file: the claim must still settle (bounded wait), not hang.
        // Takes the ~2 s grace.
        let dir = temp_dir("empty-reclaim");
        std::fs::write(dir.join("rsh-daemon.pid"), "").expect("empty pid file");
        assert!(matches!(acquire(&dir), Ok(AcquireOutcome::First(_))));
        assert_eq!(recorded_pid(&dir), Some(std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pid_of_a_process_named_rsh_daemon_is_already_running() {
        let dir = temp_dir("live");
        // argv[0] set directly — no shell: `sh` here is dash,
        // which has no `exec -a`, so a `sh -c "exec -a ..."` fake
        // dies instantly and the pid is reclaimed as First.
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .arg0("rsh-daemon")
            .spawn()
            .expect("spawn fake rsh-daemon");
        let pid = child.id();
        // Give the exec a moment to happen before we read cmdline.
        std::thread::sleep(std::time::Duration::from_millis(200));
        write_pid(&dir, pid);
        assert!(matches!(
            acquire(&dir),
            Ok(AcquireOutcome::AlreadyRunning(Some(p))) if p == pid
        ));
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
