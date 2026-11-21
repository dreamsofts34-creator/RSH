//! Windows Application Restart + Recovery registration (Round 3 WS-6, R3-AK-04).
//!
//! One call at startup in both binaries (`rsh-daemon`, `rsh-ui`):
//! `RegisterApplicationRestart(<restart command line>,
//! RESTART_NO_PATCH | RESTART_NO_REBOOT)` plus a recovery callback that
//! leaves a one-line trace in the diagnostics log dir. This is the OS's
//! own crash/hang recovery path — it covers the case the UI respawn
//! cannot: the *UI itself* crashing while the daemon is alive (the
//! daemon never respawns the UI except from tray actions).
//!
//! # Honest limits (Microsoft docs — do not promise more)
//!
//! WER restarts the app **only if it has been running >= 60 seconds**
//! before the crash/hang, and **only with the user's consent** — on
//! crash/hang the user is shown a dialog and asked whether to restart.
//! It is **not** a silent auto-restart, and it is **not** executed if
//! the user manually terminates the program (e.g. Task Manager).
//! The silent path stays the UI monitor + the Task Scheduler
//! supervisor (R3-AK-01). Source: `RegisterApplicationRestart` docs
//! and Raymond Chen, "How do I add support for Windows Restart
//! Manager…" / "How can I register a program to auto-relaunch if it
//! crashes…" (`devblogs.microsoft.com/oldnewthing/20230608-00/`).
//!
//! # Crash-loop guard
//!
//! The 60 s rule already stops rapid nagging, but a process that
//! lives > 60 s and then crashes on every run would still offer the
//! user a restart dialog on every crash. So the registered command
//! line carries a sentinel arg ([`WER_RESTART_ARG`]); a launch that
//! sees it *and* finds a restart marker written <
//! [`RAPID_RESTART_WINDOW_SECS`] ago does **not** re-register (the
//! marker is only written on a (re-)registration, so a normal launch
//! never trips this).
//!
//! # Platform
//!
//! Windows-only API (`windows` crate, `Win32_System_Recovery` +
//! `Win32_System_WindowsProgramming`). On any other OS
//! [`register_application_restart`] is a no-op returning
//! [`RestartOutcome::UnsupportedPlatform`], so the call sites stay
//! unconditional and the pure helpers ([`quote_arg`],
//! [`should_register_restart`], [`was_restarted_by_wer`]) are
//! unit-testable on Linux.

use std::path::Path;
#[cfg(any(windows, test))]
use std::path::PathBuf;

/// Sentinel arg appended to the WER restart command line, so a
/// restarted process can tell it was relaunched by WER (see the
/// module docs — crash-loop guard). Accepted-and-ignored by both
/// binaries' arg parsers.
pub const WER_RESTART_ARG: &str = "--wer-restart";

/// Marker file in the data dir, holding the unix time of the last
/// (re-)registration as ASCII.
#[cfg(any(windows, test))]
const RESTART_MARKER_FILE: &str = "wer-restart-marker";

/// A WER-restarted launch this soon after the last registration is a
/// rapid re-crash: do not re-register (no more restart dialogs).
/// WER itself only restarts after >= 60 s uptime, so 120 s covers a
/// crash ~60 s after the restart plus scheduling slack.
const RAPID_RESTART_WINDOW_SECS: u64 = 120;

/// Recovery-callback trace file, under `<data_dir>/logs/`.
#[cfg(windows)]
const RECOVERY_LOG_FILE: &str = "rsh-recovery.log";

/// What [`register_application_restart`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestartOutcome {
    /// Restart + recovery callback are registered with WER.
    Registered,
    /// A WER-restarted launch that crashed again too fast: the
    /// registration was deliberately skipped (see the module docs).
    SkippedRapidRestart,
    /// Not Windows — nothing was registered (not an error).
    UnsupportedPlatform,
}

/// Whether `args` (normally `std::env::args().skip(1)`) contains the
/// WER-restart sentinel ([`WER_RESTART_ARG`]).
#[must_use]
pub fn was_restarted_by_wer(args: &[String]) -> bool {
    args.iter().any(|arg| arg == WER_RESTART_ARG)
}

/// Quote one argv element per `CommandLineToArgvW` rules: no quoting
/// when the arg has no space/tab/quote; otherwise wrap in double
/// quotes, escaping `"` and trailing `\` (a `\` before a closing
/// quote would escape the quote itself).
#[must_use]
pub fn quote_arg(arg: &str) -> String {
    if arg.is_empty() {
        return "\"\"".to_string();
    }
    if !arg.chars().any(|c| c == ' ' || c == '\t' || c == '"') {
        return arg.to_string();
    }
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    let mut backslashes = 0;
    for c in arg.chars() {
        if c == '\\' {
            backslashes += 1;
        } else {
            // The backslashes seen so far precede a real character:
            // emit them verbatim (a `"` after them gets one extra
            // escape backslash of its own).
            for _ in 0..backslashes {
                out.push('\\');
            }
            backslashes = 0;
            if c == '"' {
                out.push('\\');
            }
            out.push(c);
        }
    }
    // Trailing backslashes must be DOUBLED, or they would escape
    // the closing quote (one input backslash → two in the output).
    for _ in 0..(backslashes * 2) {
        out.push('\\');
    }
    out.push('"');
    out
}

/// Build the restart command line: the current exe plus the args this
/// process was started with (minus any stale sentinel), plus the
/// sentinel ([`WER_RESTART_ARG`]) so a WER-restarted launch can be
/// detected. The restart must pass the same args the process was
/// started with (no `--tray` quirks) — the daemon's `--data-dir` and
/// the UI's `--page` survive a restart this way.
///
/// # Errors
///
/// Returns [`String`] when the current exe path cannot be resolved.
pub fn restart_command_line() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let mut parts = vec![quote_arg(&exe.to_string_lossy())];
    for arg in std::env::args().skip(1) {
        if arg != WER_RESTART_ARG {
            parts.push(quote_arg(&arg));
        }
    }
    parts.push(WER_RESTART_ARG.to_string());
    Ok(parts.join(" "))
}

/// Pure decision: should this launch (re-)register for restart?
///
/// * Not a WER restart → always register.
/// * WER restart with no marker (or an unreadable one) → register
///   (the marker is best-effort; a missing marker must not disable
///   recovery).
/// * WER restart with a marker younger than
///   [`RAPID_RESTART_WINDOW_SECS`] → skip (rapid re-crash).
#[must_use]
pub fn should_register_restart(
    was_wer_restart: bool,
    marker_unix: Option<u64>,
    now_unix: u64,
) -> bool {
    if !was_wer_restart {
        return true;
    }
    match marker_unix {
        Some(marker) => now_unix.saturating_sub(marker) >= RAPID_RESTART_WINDOW_SECS,
        None => true,
    }
}

/// Current unix time (0 when the clock is unavailable — the marker
/// then reads as ancient, so registration proceeds).
#[cfg(windows)]
fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

/// Read the restart marker (unix seconds as ASCII); `None` when the
/// file is missing or unparseable.
#[cfg(any(windows, test))]
fn read_restart_marker(data_dir: &Path) -> Option<u64> {
    std::fs::read_to_string(data_dir.join(RESTART_MARKER_FILE))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Write the restart marker (best-effort — a failure must never fail
/// the launch; registration still proceeds).
#[cfg(any(windows, test))]
fn write_restart_marker(data_dir: &Path, now: u64) {
    let _ = std::fs::write(data_dir.join(RESTART_MARKER_FILE), now.to_string());
}

/// Register this process for Windows Application Restart (+ recovery
/// callback). Best-effort at the call site: on `Err` log the reason
/// and keep running — a failed registration must never fail the
/// launch, and the UI monitor + Task Scheduler supervisor still
/// cover respawn without it.
///
/// # Errors
///
/// Returns [`String`] when the exe path cannot be resolved or the
/// `RegisterApplicationRestart` / `RegisterApplicationRecoveryCallback`
/// call fails.
pub fn register_application_restart(
    data_dir: &Path,
    was_wer_restart: bool,
) -> Result<RestartOutcome, String> {
    #[cfg(not(windows))]
    {
        let _ = (data_dir, was_wer_restart);
        Ok(RestartOutcome::UnsupportedPlatform)
    }
    #[cfg(windows)]
    {
        register_application_restart_windows(data_dir, was_wer_restart)
    }
}

#[cfg(windows)]
fn register_application_restart_windows(
    data_dir: &Path,
    was_wer_restart: bool,
) -> Result<RestartOutcome, String> {
    use windows::Win32::System::Recovery::{
        RESTART_NO_PATCH, RESTART_NO_REBOOT, RegisterApplicationRecoveryCallback,
        RegisterApplicationRestart,
    };
    use windows::core::PCWSTR;

    let now = now_unix();
    if !should_register_restart(was_wer_restart, read_restart_marker(data_dir), now) {
        return Ok(RestartOutcome::SkippedRapidRestart);
    }
    write_restart_marker(data_dir, now);
    let command_line = restart_command_line()?;
    let wide: Vec<u16> = command_line.encode_utf16().chain([0]).collect();
    unsafe {
        RegisterApplicationRestart(PCWSTR(wide.as_ptr()), RESTART_NO_PATCH | RESTART_NO_REBOOT)
            .map_err(|error| error.to_string())?;
        // Recovery callback: WER runs it on its own thread before
        // terminating the app, so the app can flush state. RSH keeps
        // its state on disk already (profiles/settings/status file);
        // the callback only leaves a one-line trace so a recovery is
        // visible in the diagnostics log dir. The path is leaked
        // deliberately: the registration lives for the process
        // lifetime, and the callback must never dangle.
        let log_path = data_dir.join("logs").join(RECOVERY_LOG_FILE);
        let param =
            std::ptr::from_mut::<PathBuf>(Box::leak(Box::new(log_path))).cast::<std::ffi::c_void>();
        RegisterApplicationRecoveryCallback(Some(recovery_callback), Some(param), 5000, 0)
            .map_err(|error| error.to_string())?;
    }
    Ok(RestartOutcome::Registered)
}

/// WER recovery callback: ping WER (so recovery is not killed for
/// silence), append one trace line, report finished. Never panics
/// and never fails the process — every step is best-effort.
#[cfg(windows)]
unsafe extern "system" fn recovery_callback(pvparameter: *mut std::ffi::c_void) -> u32 {
    use windows::Win32::System::Recovery::{
        ApplicationRecoveryFinished, ApplicationRecoveryInProgress,
    };
    let _ = std::panic::catch_unwind(|| unsafe {
        let _ = ApplicationRecoveryInProgress();
        if !pvparameter.is_null() {
            let path = &*(pvparameter as *const PathBuf);
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let line = format!(
                "recovery: pid {} at {} — WER recovery callback ran (state already on disk; nothing else to flush)\n",
                std::process::id(),
                now_unix()
            );
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                use std::io::Write;
                let _ = file.write_all(line.as_bytes());
            }
        }
        ApplicationRecoveryFinished(true);
    });
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rsh-restart-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn quote_arg_plain_is_unchanged() {
        assert_eq!(quote_arg("rsh-daemon.exe"), "rsh-daemon.exe");
        assert_eq!(quote_arg("--no-tray"), "--no-tray");
    }

    #[test]
    fn quote_arg_empty_and_spaced() {
        assert_eq!(quote_arg(""), "\"\"");
        assert_eq!(
            quote_arg(r"C:\Program Files\RSH\rsh-daemon.exe"),
            r#""C:\Program Files\RSH\rsh-daemon.exe""#
        );
    }

    #[test]
    fn quote_arg_escapes_quotes_and_trailing_backslashes() {
        assert_eq!(quote_arg("a\"b"), "\"a\\\"b\"");
        // No space/tab/quote → returned unchanged, no quoting needed.
        assert_eq!(quote_arg(r"C:\dir\"), r"C:\dir\");
        // With a space, the arg IS quoted — and the trailing
        // backslash must be doubled, or it would escape the closing
        // quote.
        assert_eq!(quote_arg(r"C:\my dir\"), r#""C:\my dir\\""#);
    }

    #[test]
    fn was_restarted_by_wer_detects_sentinel() {
        assert!(was_restarted_by_wer(&[
            "--data-dir".to_string(),
            "C:\\x".to_string(),
            WER_RESTART_ARG.to_string(),
        ]));
        assert!(!was_restarted_by_wer(&["--no-tray".to_string()]));
        assert!(!was_restarted_by_wer(&[]));
    }

    #[test]
    fn should_register_normal_launch_always() {
        assert!(should_register_restart(false, None, 1_000));
        assert!(should_register_restart(false, Some(999), 1_000));
    }

    #[test]
    fn should_register_wer_restart_without_marker() {
        assert!(should_register_restart(true, None, 1_000));
    }

    #[test]
    fn should_skip_rapid_wer_recrash() {
        // Marker 60 s ago: a crash ~60 s after the WER restart.
        assert!(!should_register_restart(true, Some(940), 1_000));
        // Marker at the window edge: register again.
        assert!(should_register_restart(true, Some(880), 1_000));
        // Ancient marker: register again.
        assert!(should_register_restart(true, Some(100), 1_000));
    }

    #[test]
    fn restart_marker_round_trip() {
        let dir = temp_dir("marker");
        assert_eq!(read_restart_marker(&dir), None);
        write_restart_marker(&dir, 1_700_000_000);
        assert_eq!(read_restart_marker(&dir), Some(1_700_000_000));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn restart_command_line_ends_with_sentinel() {
        let line = restart_command_line().expect("command line");
        assert!(line.ends_with(WER_RESTART_ARG), "line: {line}");
        assert_eq!(
            line.matches(WER_RESTART_ARG).count(),
            1,
            "sentinel must appear exactly once: {line}"
        );
        let exe = std::env::current_exe().expect("current exe");
        let name = exe.file_name().expect("exe name").to_string_lossy();
        assert!(line.contains(name.as_ref()), "line: {line}");
    }

    #[test]
    fn register_is_noop_off_windows() {
        let dir = temp_dir("noop");
        let outcome = register_application_restart(&dir, false).expect("register must not fail");
        #[cfg(not(windows))]
        assert_eq!(outcome, RestartOutcome::UnsupportedPlatform);
        #[cfg(windows)]
        {
            use windows::Win32::System::Recovery::{
                UnregisterApplicationRecoveryCallback, UnregisterApplicationRestart,
            };
            assert_eq!(outcome, RestartOutcome::Registered);
            unsafe {
                let _ = UnregisterApplicationRestart();
                let _ = UnregisterApplicationRecoveryCallback();
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Windows-only: register, then read the settings back with
    /// `GetApplicationRestartSettings` — the flags must be
    /// `RESTART_NO_PATCH | RESTART_NO_REBOOT` and the command line
    /// must carry the sentinel. Unregisters afterwards so the test
    /// process leaves no registration behind. Runs in CI's
    /// `test-windows` job (`cargo test --workspace` on
    /// windows-latest); on Linux this test does not exist.
    #[cfg(windows)]
    #[test]
    fn register_restart_round_trips_through_get_settings() {
        use windows::Win32::System::Recovery::{
            GetApplicationRestartSettings, RESTART_NO_PATCH, RESTART_NO_REBOOT,
            UnregisterApplicationRecoveryCallback, UnregisterApplicationRestart,
        };
        use windows::Win32::System::Threading::GetCurrentProcess;
        use windows::core::PWSTR;

        let dir = temp_dir("round-trip");
        let outcome = register_application_restart(&dir, false).expect("register");
        assert_eq!(outcome, RestartOutcome::Registered);
        let mut buf = vec![0u16; 4096];
        let mut size = u32::try_from(buf.len()).unwrap_or(u32::MAX);
        let mut flags = 0u32;
        unsafe {
            GetApplicationRestartSettings(
                GetCurrentProcess(),
                Some(PWSTR(buf.as_mut_ptr())),
                &raw mut size,
                Some(&raw mut flags),
            )
            .expect("get settings");
        }
        let expected = RESTART_NO_PATCH.0 | RESTART_NO_REBOOT.0;
        assert_eq!(flags & expected, expected, "flags: {flags:#x}");
        // `size` counts chars written including the NUL terminator.
        let len = (size as usize).saturating_sub(1).min(buf.len());
        let command_line = String::from_utf16_lossy(&buf[..len]);
        assert!(
            command_line.contains(WER_RESTART_ARG),
            "command line: {command_line}"
        );
        unsafe {
            let _ = UnregisterApplicationRestart();
            let _ = UnregisterApplicationRecoveryCallback();
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
