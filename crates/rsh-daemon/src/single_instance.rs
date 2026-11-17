//! Single-instance (LOGIC-FIX-PLAN §2 rule 6): the daemon is
//! single-instance; a second launch signals the running daemon and
//! exits.
//!
//! * Windows: a named mutex (`Global\RSH-Daemon-<user>` — the real
//!   mutex name is built from the current username when available).
//!   A second instance finds the mutex held and exits after
//!   signalling via IPC.
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
/// # Errors
///
/// Returns the write error when the pid file cannot be written.
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
#[cfg(unix)]
pub fn acquire(data_dir: &std::path::Path) -> Result<AcquireOutcome, String> {
    if let Some(pid) = recorded_pid(data_dir)
        && pid_is_live_rsh_daemon(pid)
    {
        return Ok(AcquireOutcome::AlreadyRunning(Some(pid)));
    }
    // Otherwise the pid file is stale (or unreadable): the previous
    // daemon died without cleaning up — reclaim it below.
    match write_pid_file(data_dir) {
        Ok(path) => Ok(AcquireOutcome::First(SingleInstance::PidFile { path })),
        Err(error) => Err(format!(
            "cannot create daemon lock file {}: {error}",
            pid_file_path(data_dir).display()
        )),
    }
}

/// Try to become the single daemon instance (Windows named mutex).
///
/// # Errors
///
/// Returns an error message when the mutex cannot be created.
#[cfg(windows)]
pub fn acquire(data_dir: &std::path::Path) -> Result<AcquireOutcome, String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::core::PCWSTR;
    // Per-user mutex name: the username keeps two users' daemons
    // independent on a shared machine.
    let user = std::env::var("USERNAME").unwrap_or_else(|_| "user".to_string());
    let name: Vec<u16> = format!("Global\\RSH-Daemon-{user}\0")
        .encode_utf16()
        .collect();
    let handle = unsafe {
        CreateMutexW(None, true, PCWSTR(name.as_ptr()))
            .map_err(|e| format!("cannot create daemon mutex: {e}"))?
    };
    let already = unsafe { windows::Win32::Foundation::GetLastError() }
        == windows::Win32::Foundation::ERROR_ALREADY_EXISTS;
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
            let SingleInstance::Mutex { handle, pid_path } = self;
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
    fn pid_of_a_process_named_rsh_daemon_is_already_running() {
        let dir = temp_dir("live");
        // `exec -a` renames argv[0], so the child's cmdline names
        // rsh-daemon without running the real daemon.
        let mut child = std::process::Command::new("bash")
            .args(["-c", "exec -a rsh-daemon sleep 30"])
            .spawn()
            .expect("spawn bash");
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
