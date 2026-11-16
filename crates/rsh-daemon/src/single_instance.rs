//! Single-instance (LOGIC-FIX-PLAN §2 rule 6): the daemon is
//! single-instance; a second launch signals the running daemon and
//! exits.
//!
//! * Windows: a named mutex (`Global\RSH-Daemon-<user>` — the real
//!   mutex name is built from the current username when available).
//!   A second instance finds the mutex held and exits after
//!   signalling via IPC.
//! * Linux: a lock file `<data_dir>/rsh-daemon.pid` holding the
//!   daemon's pid. A second instance reads the pid, checks the
//!   process is alive (`/proc/<pid>`), signals via IPC, and exits.
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
        // Alive? (/proc/<pid> exists and is not a zombie we own.)
        && std::path::Path::new(&format!("/proc/{pid}")).exists()
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
