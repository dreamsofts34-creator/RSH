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

#[cfg(unix)]
use std::path::PathBuf;

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
    use std::io::Write as _;
    let path = data_dir.join("rsh-daemon.pid");
    if let Ok(existing) = std::fs::read_to_string(&path)
        && let Ok(pid) = existing.trim().parse::<u32>()
        && pid_is_live_rsh_daemon(pid)
    {
        return Ok(AcquireOutcome::AlreadyRunning(Some(pid)));
    }
    // Otherwise the pid file is stale (or unreadable): the previous
    // daemon died without cleaning up — reclaim it below.
    let _ = std::fs::create_dir_all(data_dir);
    match std::fs::File::create(&path) {
        Ok(mut file) => {
            let _ = writeln!(file, "{}", std::process::id());
            Ok(AcquireOutcome::First(SingleInstance::PidFile { path }))
        }
        Err(error) => Err(format!(
            "cannot create daemon lock file {}: {error}",
            path.display()
        )),
    }
}

/// Try to become the single daemon instance (Windows named mutex).
///
/// # Errors
///
/// Returns an error message when the mutex cannot be created.
#[cfg(windows)]
pub fn acquire(_data_dir: &std::path::Path) -> Result<AcquireOutcome, String> {
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
        return Ok(AcquireOutcome::AlreadyRunning(None));
    }
    Ok(AcquireOutcome::First(SingleInstance::Mutex { handle }))
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
            // dropping `self` alone would leak it.
            let SingleInstance::Mutex { handle } = self;
            let _ = unsafe { windows::Win32::Foundation::CloseHandle(handle) };
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
