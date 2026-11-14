//! System-integration logic for the Tauri shell (SW-R7.4 / SW-R7.5 / F4).
//!
//! This module is the *testable* half of system integration: pure Rust
//! over injected traits, with no Tauri dependency, so `cargo test` covers
//! it on Linux. The Tauri/Windows half lives in
//! `src-tauri/src/system.rs` and calls into here.
//!
//! Covered here:
//! - update check (`check_update`, SW-R7.5) — semver comparison only;
//!   the Tauri updater plugin (download/install/signature verify) is
//!   wired in `src-tauri` and its signing keypair is a Phase H (H4)
//!   item — no keys are generated here.
//! - single-instance version rule (SW-R7.4) — the C# rule, decided as
//!   pure logic: an older version terminates, fail-closed.
//! - autostart stale-entry repair (SW-R7.4) — pure logic over a
//!   [`RunKeyStore`]; the Windows registry backend is in `src-tauri`.
//! - tray-hide guard (SW-R7.4) — the C# trap: the window may only be
//!   hidden when a tray icon actually exists.
//! - firewall rule *specification* (SW-R7.4) — the program-scoped rule
//!   itself is applied by the Windows COM code in `src-tauri`
//!   (verified spike path, see `docs/SYSTEM-INTEGRATION.md`); this
//!   module only builds and describes the spec.
//! - shutdown-neutral path (F4) — signal the engine to stop, then a
//!   best-effort neutral send so the wheel never stays deflected after
//!   the app exits.

use std::cmp::Ordering;
use std::fmt;
use std::path::{Path, PathBuf};

use rsh_engine::engine::{OutputSink, SinkError, StopHandle};
use rsh_engine::pipeline::OutputState;
use serde::{Deserialize, Serialize};

/// Application version of this Rust series.
pub const APP_VERSION: &str = "1.0.0";

/// Errors from the system-integration logic in this module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SystemError {
    /// A version string did not parse as semver (`a.b.c[-pre]`).
    InvalidVersion(String),
    /// The autostart store (registry) could not be read or written.
    Store(String),
    /// The window was asked to hide while no tray icon exists.
    TrayMissing,
    /// The operation is Windows-only and this is not Windows.
    UnsupportedPlatform(&'static str),
}

impl fmt::Display for SystemError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidVersion(version) => {
                write!(f, "invalid version '{version}' (expected semver a.b.c)")
            }
            Self::Store(message) => write!(f, "autostart store error: {message}"),
            Self::TrayMissing => {
                write!(f, "cannot hide window: no tray icon exists")
            }
            Self::UnsupportedPlatform(what) => {
                write!(f, "{what} is only supported on Windows")
            }
        }
    }
}

impl std::error::Error for SystemError {}

// ---------------------------------------------------------------------------
// Versions + update check (SW-R7.5)
// ---------------------------------------------------------------------------

/// Parse a version string as semver.
///
/// # Errors
///
/// Returns [`SystemError::InvalidVersion`] when the string is not
/// `a.b.c` (with an optional pre-release suffix).
pub fn parse_version(input: &str) -> Result<semver::Version, SystemError> {
    semver::Version::parse(input).map_err(|_| SystemError::InvalidVersion(input.to_string()))
}

/// Compare two version strings with semver precedence.
///
/// # Errors
///
/// Returns [`SystemError::InvalidVersion`] when either string does not
/// parse. Build metadata is ignored by semver precedence, as usual.
pub fn compare_versions(a: &str, b: &str) -> Result<Ordering, SystemError> {
    Ok(parse_version(a)?.cmp(&parse_version(b)?))
}

/// What an update check found upstream: a newer release, or nothing
/// newer. `download_url` is filled in by the Tauri side when the
/// updater plugin reports one; the pure check leaves it [`None`].
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    /// Upstream version (`a.b.c`).
    pub version: String,
    /// Release notes, when the updater manifest carries them.
    pub notes: Option<String>,
    /// Direct download URL, when the updater plugin reports one.
    pub download_url: Option<String>,
}

/// The result of [`check_update`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateDecision {
    /// The running version is the newest known version.
    UpToDate {
        /// The running version.
        current: String,
    },
    /// A newer version exists upstream.
    Available(UpdateInfo),
    /// The running version is *newer* than the latest upstream
    /// version (e.g. a dev build). Never offered as an update.
    CurrentIsNewer {
        /// The running version.
        current: String,
        /// The latest upstream version.
        latest: String,
    },
}

/// Decide whether `latest` is an update over `current`.
///
/// Pure semver comparison: `latest > current` is an update,
/// `latest == current` is up to date, `latest < current` means the
/// running build is newer (dev build) and no update is offered. This
/// function never touches the network and never installs anything —
/// the Tauri updater plugin does the download, the Ed25519 manifest
/// verification, and the install (Phase H owns its signing keypair).
///
/// # Errors
///
/// Returns [`SystemError::InvalidVersion`] when either version string
/// does not parse.
pub fn check_update(current: &str, latest: &UpdateInfo) -> Result<UpdateDecision, SystemError> {
    match compare_versions(&latest.version, current)? {
        Ordering::Greater => Ok(UpdateDecision::Available(latest.clone())),
        Ordering::Equal => Ok(UpdateDecision::UpToDate {
            current: current.to_string(),
        }),
        Ordering::Less => Ok(UpdateDecision::CurrentIsNewer {
            current: current.to_string(),
            latest: latest.version.clone(),
        }),
    }
}

/// Whether the `WebView2` runtime check (SW-R7.5) has anything to do.
///
/// `WebView2` ships with Windows 10/11, so on a supported OS the
/// runtime is present by definition; a missing runtime is reported
/// by the Tauri shell at startup, not by this check. This constant
/// records that the C# ".NET runtime check" has no Rust equivalent —
/// there is no runtime to check for.
pub const RUNTIME_CHECK_NOT_APPLICABLE: &str =
    "Rust has no runtime dependency; WebView2 ships with Windows 10/11";

// ---------------------------------------------------------------------------
// Single instance, version-aware (SW-R7.4, C# rule)
// ---------------------------------------------------------------------------

/// What the *second* instance must do when it finds a running one.
///
/// The C# rule, restated: an older version terminates, fail-closed.
/// The decision is pure logic; the handover itself (the running
/// instance exiting so the newer one can take over) needs a
/// named-event handshake the `tauri-plugin-single-instance` plugin
/// does not provide — see `docs/SYSTEM-INTEGRATION.md` §1. Until
/// that handshake exists, the shell ships the plugin's behaviour
/// (focus the existing window) and treats `TakeOver` as "log and
/// focus", never as a silent kill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstanceDecision {
    /// No other instance is running: proceed as the primary instance.
    TakeOver,
    /// Another instance runs the same version: focus its window and
    /// exit this process.
    FocusExisting,
    /// This process is the older one (or a version could not be
    /// compared): terminate this process. Fail-closed — an unknown
    /// version never fights a known one.
    YieldFailClosed,
}

/// Resolve the single-instance conflict for a second instance.
///
/// `running_version` is the version of the already-running instance
/// ([`None`] when no instance is running); `own_version` is this
/// process's version. Rules:
/// - no running instance → [`InstanceDecision::TakeOver`];
/// - running is newer → [`InstanceDecision::YieldFailClosed`]
///   (this process is the older one);
/// - running is older → [`InstanceDecision::TakeOver`] (the running
///   instance is the older one and must exit — the handover protocol
///   in `docs/SYSTEM-INTEGRATION.md` §1);
/// - same version → [`InstanceDecision::FocusExisting`];
/// - either version unparsable → [`InstanceDecision::YieldFailClosed`]
///   (fail-closed: never guess).
#[must_use]
pub fn resolve_instance(running_version: Option<&str>, own_version: &str) -> InstanceDecision {
    let Some(running) = running_version else {
        return InstanceDecision::TakeOver;
    };
    let Ok(ordering) = compare_versions(running, own_version) else {
        return InstanceDecision::YieldFailClosed;
    };
    match ordering {
        Ordering::Greater => InstanceDecision::YieldFailClosed,
        Ordering::Less => InstanceDecision::TakeOver,
        Ordering::Equal => InstanceDecision::FocusExisting,
    }
}

/// The argument a second instance passes its version in, so the
/// first instance's `single-instance` callback can compare versions
/// (`--rsh-version <a.b.c>`). The plugin forwards the second
/// instance's argv to the first instance's callback.
pub const INSTANCE_VERSION_ARG: &str = "--rsh-version";

/// Pull the `--rsh-version <version>` value out of an argv list.
///
/// Returns [`None`] when the flag is absent or has no following value.
#[must_use]
pub fn parse_instance_version_arg(args: &[String]) -> Option<String> {
    args.windows(2).find_map(|pair| {
        if pair[0] == INSTANCE_VERSION_ARG {
            Some(pair[1].clone())
        } else {
            None
        }
    })
}

// ---------------------------------------------------------------------------
// Autostart: Run-key store + stale-entry repair (SW-R7.4)
// ---------------------------------------------------------------------------

/// The value name RSH uses under
/// `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
pub const AUTOSTART_VALUE_NAME: &str = "RSH";

/// Minimal interface over the Windows `Run` key, so the repair logic
/// is testable with a fake store on Linux. The real backend
/// (`RegExeStore`, `src-tauri/src/system.rs`) shells out to `reg.exe`
/// on Windows; the plugin owns enable/disable, this store owns
/// *repair* (the plugin never rewrites a stale entry).
pub trait RunKeyStore {
    /// The current `RSH` Run-key command line, [`None`] when absent.
    fn read_rsh(&self) -> Option<String>;
    /// Write the `RSH` Run-key command line.
    ///
    /// # Errors
    ///
    /// Returns [`SystemError::Store`] when the registry write fails.
    fn write_rsh(&mut self, command: &str) -> Result<(), SystemError>;
    /// Delete the `RSH` Run-key value (no-op when absent).
    ///
    /// # Errors
    ///
    /// Returns [`SystemError::Store`] when the registry delete fails.
    fn delete_rsh(&mut self) -> Result<(), SystemError>;
}

/// Build the Run-key command line for an exe path: the quoted path,
/// no extra arguments (autostart args are appended by the shell, see
/// `AUTOSTART_ARGS` in `src-tauri/src/system.rs`).
#[must_use]
pub fn autostart_command(exe: &Path) -> String {
    format!("\"{}\"", exe.display())
}

/// Extract the exe path from a Run-key command line: the first
/// whitespace-separated token, with surrounding quotes stripped.
/// Arguments after the exe are ignored — the entry is identified by
/// the exe it points at, not by its flags.
#[must_use]
pub fn exe_from_command(command: &str) -> PathBuf {
    let trimmed = command.trim_start();
    if let Some(rest) = trimmed.strip_prefix('"') {
        let end = rest.find('"').unwrap_or(rest.len());
        PathBuf::from(&rest[..end])
    } else {
        let end = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
        PathBuf::from(&trimmed[..end])
    }
}

/// Whether two exe paths name the same file. Windows paths compare
/// case-insensitively; the comparison is done on the string form so
/// it also works for the `C:\...` spellings used in tests on Linux.
#[must_use]
pub fn same_exe(a: &Path, b: &Path) -> bool {
    a.as_os_str()
        .to_string_lossy()
        .eq_ignore_ascii_case(&b.as_os_str().to_string_lossy())
}

/// What [`repair_autostart`] found and did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AutostartRepair {
    /// The entry already points at the expected exe. Nothing changed.
    AlreadyCorrect,
    /// The entry pointed elsewhere (stale folder / old install) and
    /// was rewritten to the expected exe.
    Repaired {
        /// The stale command line that was replaced.
        old_command: String,
    },
    /// No `RSH` entry exists. Nothing changed — enabling autostart
    /// is the plugin's job (`set_autostart`), not repair's.
    NotPresent,
}

/// Repair a stale autostart entry: when the `RSH` Run-key value
/// exists but points at a different exe than `expected_exe` (an old
/// install folder after a move/reinstall), rewrite it to
/// `expected_exe`. A correct entry is left untouched, and an absent
/// entry is left absent.
///
/// # Errors
///
/// Returns [`SystemError::Store`] when the registry read/write fails.
pub fn repair_autostart(
    store: &mut dyn RunKeyStore,
    expected_exe: &Path,
) -> Result<AutostartRepair, SystemError> {
    let Some(command) = store.read_rsh() else {
        return Ok(AutostartRepair::NotPresent);
    };
    if same_exe(&exe_from_command(&command), expected_exe) {
        return Ok(AutostartRepair::AlreadyCorrect);
    }
    let expected = autostart_command(expected_exe);
    store.write_rsh(&expected)?;
    Ok(AutostartRepair::Repaired {
        old_command: command,
    })
}

/// Disable autostart by deleting the `RSH` Run-key value.
///
/// # Errors
///
/// Returns [`SystemError::Store`] when the registry delete fails.
pub fn disable_autostart(store: &mut dyn RunKeyStore) -> Result<(), SystemError> {
    store.delete_rsh()
}

// ---------------------------------------------------------------------------
// Tray-hide guard (SW-R7.4, C# v2.0.1 trap)
// ---------------------------------------------------------------------------

/// Guards "hide to tray": the window may only be hidden when a tray
/// icon actually exists. Dropping the icon (or never creating it) and
/// hiding anyway strands the user with no way back — the C# v2.0.1
/// blank-window lesson. Every hide path in the shell goes through
/// this guard; the icon's lifetime is owned by the shell state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrayGuard {
    /// Whether a tray icon is currently present.
    icon_present: bool,
}

impl TrayGuard {
    /// Build a guard with the icon's current state.
    #[must_use]
    pub const fn new(icon_present: bool) -> Self {
        Self { icon_present }
    }

    /// Record that the tray icon was created (`true`) or removed
    /// (`false`). Call this from the icon's lifecycle, not from the
    /// hide path.
    pub const fn set_icon_present(&mut self, present: bool) {
        self.icon_present = present;
    }

    /// Whether the window may be hidden right now.
    #[must_use]
    pub const fn can_hide_window(&self) -> bool {
        self.icon_present
    }

    /// Authorise a hide, or refuse it when no tray icon exists.
    ///
    /// # Errors
    ///
    /// Returns [`SystemError::TrayMissing`] when no tray icon exists.
    /// The caller must show (not hide) the window in that case.
    pub const fn authorise_hide(&self) -> Result<(), SystemError> {
        if self.icon_present {
            Ok(())
        } else {
            Err(SystemError::TrayMissing)
        }
    }
}

// ---------------------------------------------------------------------------
// Firewall rule specification (SW-R7.4 spike)
// ---------------------------------------------------------------------------

/// Name of the RSH program-scoped firewall rule.
pub const FIREWALL_RULE_NAME: &str = "RSH";

/// A program-scoped Windows Firewall rule for one exe.
///
/// The rule allows **inbound** traffic to the RSH exe on **all
/// profiles** (the board talks to the PC over UDP on the LAN, and the
/// profile the LAN ends up on is not predictable). No port is named:
/// the rule is scoped to the *program*, not to a port — a port-scoped
/// rule would also allow other programs on that port. Outbound is
/// left alone (Windows allows outbound by default). The rule is
/// applied by the Windows COM code in `src-tauri/src/system.rs`
/// (`INetFwPolicy2` / `INetFwRule`, verified against the
/// `windows` 0.62.2 crate source — see `docs/SYSTEM-INTEGRATION.md`
/// §4). Applying it needs elevation; a non-elevated apply fails
/// loudly and the app keeps working without the rule (the user is
/// told to allow it in the firewall prompt instead).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirewallRuleSpec {
    /// Rule name ([`FIREWALL_RULE_NAME`]).
    pub name: String,
    /// Full path of the exe the rule is scoped to.
    pub exe_path: PathBuf,
}

impl FirewallRuleSpec {
    /// The inbound-allow, program-scoped rule for `exe`.
    #[must_use]
    pub fn inbound_allow(exe: &Path) -> Self {
        Self {
            name: FIREWALL_RULE_NAME.to_string(),
            exe_path: exe.to_path_buf(),
        }
    }

    /// Human-readable description (Settings → Firewall state).
    #[must_use]
    pub fn describe(&self) -> String {
        format!(
            "Rule '{}': allow inbound to program '{}' on all profiles (no port scope)",
            self.name,
            self.exe_path.display()
        )
    }
}

/// The firewall state the Settings page shows for RSH.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FirewallState {
    /// Whether a rule named [`FIREWALL_RULE_NAME`] exists.
    pub rule_present: bool,
    /// Whether that rule is enabled (false when absent).
    pub rule_enabled: bool,
    /// The exe the existing rule is scoped to, when one exists.
    pub exe_path: Option<String>,
}

impl FirewallState {
    /// No rule is known to exist (the pre-check / non-Windows state).
    #[must_use]
    pub const fn absent() -> Self {
        Self {
            rule_present: false,
            rule_enabled: false,
            exe_path: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Shutdown-neutral path (F4)
// ---------------------------------------------------------------------------

/// What the shutdown path did. Both steps are best-effort and
/// reported, never panicking: a failing backend at shutdown must not
/// turn an exit into a crash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShutdownOutcome {
    /// Whether the engine was signalled to stop.
    pub stop_signalled: bool,
    /// Whether a final neutral state reached the output backend.
    pub neutral_sent: bool,
}

/// Best-effort final neutral send to an output backend.
///
/// Returns `true` when the backend accepted the neutral state. The
/// engine's own `finish()` also sends neutral; this send covers the
/// shell path (engine task already gone, or the sink held by the
/// shell rather than the engine). Sending neutral twice is harmless
/// — the state is idempotent.
#[must_use]
pub fn send_neutral(sink: &mut dyn OutputSink) -> bool {
    sink.send(&OutputState::neutral()).is_ok()
}

/// Run the F4 shutdown path: signal the engine to stop (when a stop
/// handle is given), then send a final neutral state to the output
/// backend.///
/// Ordering contract (the caller owns the rest):
/// 1. call this — stop is signalled, neutral is sent;
/// 2. await the engine task with a bounded timeout (the engine
///    returns within one 5 ms tick of the stop signal, well inside
///    the 5 s bound — its own `finish()` sends neutral again);
/// 3. drop/release the output backend (device released).
///
/// The caller must not block shutdown on the engine task: if the
/// wait times out, the process still exits — a stuck engine must
/// never strand the app, and the neutral send in step 1 already
/// parked the wheel.
#[must_use]
pub fn shutdown_output(stop: Option<&StopHandle>, sink: &mut dyn OutputSink) -> ShutdownOutcome {
    let stop_signalled = match stop {
        Some(handle) => {
            handle.stop();
            true
        }
        None => false,
    };
    let neutral_sent = send_neutral(sink);
    ShutdownOutcome {
        stop_signalled,
        neutral_sent,
    }
}

/// Sink that accepts nothing: used on the exit path before Phase G
/// wires the real output backend into `AppState`. The neutral send is
/// attempted and honestly reported as not sent — never silently
/// claimed as delivered.
pub struct NullSink;

impl OutputSink for NullSink {
    fn send(&mut self, _state: &OutputState) -> Result<(), SinkError> {
        Err(SinkError::default())
    }
}

/// Exit-path shutdown with no output backend yet (Phase F gate).
/// Phase G wires the real backend + engine stop handle into `AppState`
/// and replaces this call; until then the shutdown path is wired and
/// its outcome is reported honestly (`neutral_sent: false`).
#[must_use]
pub fn shutdown_with_no_backend() -> ShutdownOutcome {
    let mut sink = NullSink;
    shutdown_output(None, &mut sink)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsh_engine::engine::SinkError;

    // -- versions / update check -------------------------------------------

    #[test]
    fn version_compare_orders_releases() {
        assert_eq!(compare_versions("1.0.0", "1.0.0").unwrap(), Ordering::Equal);
        assert_eq!(
            compare_versions("1.0.1", "1.0.0").unwrap(),
            Ordering::Greater
        );
        assert_eq!(compare_versions("1.0.0", "2.0.0").unwrap(), Ordering::Less);
        // Pre-release sorts before its release (semver rule).
        assert_eq!(
            compare_versions("1.0.0-rc.1", "1.0.0").unwrap(),
            Ordering::Less
        );
    }

    #[test]
    fn version_compare_rejects_garbage() {
        assert_eq!(
            compare_versions("banana", "1.0.0"),
            Err(SystemError::InvalidVersion("banana".to_string()))
        );
        assert_eq!(
            compare_versions("1.0", "1.0.0"),
            Err(SystemError::InvalidVersion("1.0".to_string()))
        );
    }

    fn update_info(version: &str) -> UpdateInfo {
        UpdateInfo {
            version: version.to_string(),
            notes: None,
            download_url: None,
        }
    }

    #[test]
    fn update_available_when_newer() {
        let decision = check_update("1.0.0", &update_info("1.1.0")).unwrap();
        assert_eq!(decision, UpdateDecision::Available(update_info("1.1.0")));
    }

    #[test]
    fn update_up_to_date_when_equal() {
        let decision = check_update("1.0.0", &update_info("1.0.0")).unwrap();
        assert_eq!(
            decision,
            UpdateDecision::UpToDate {
                current: "1.0.0".to_string()
            }
        );
    }

    #[test]
    fn update_never_offered_when_running_is_newer() {
        let decision = check_update("1.2.0", &update_info("1.1.0")).unwrap();
        assert_eq!(
            decision,
            UpdateDecision::CurrentIsNewer {
                current: "1.2.0".to_string(),
                latest: "1.1.0".to_string()
            }
        );
    }

    #[test]
    fn update_rejects_bad_version() {
        assert!(check_update("1.0.0", &update_info("not-a-version")).is_err());
    }

    // -- single instance -----------------------------------------------------

    #[test]
    fn instance_no_running_instance_takes_over() {
        assert_eq!(resolve_instance(None, "1.0.0"), InstanceDecision::TakeOver);
    }

    #[test]
    fn instance_same_version_focuses_existing() {
        assert_eq!(
            resolve_instance(Some("1.0.0"), "1.0.0"),
            InstanceDecision::FocusExisting
        );
    }

    #[test]
    fn instance_older_self_yields_fail_closed() {
        // Running 1.1.0, we are 1.0.0: we are the older version.
        assert_eq!(
            resolve_instance(Some("1.1.0"), "1.0.0"),
            InstanceDecision::YieldFailClosed
        );
    }

    #[test]
    fn instance_running_is_older_we_take_over() {
        // Running 1.0.0, we are 1.1.0: the running one is older.
        assert_eq!(
            resolve_instance(Some("1.0.0"), "1.1.0"),
            InstanceDecision::TakeOver
        );
    }

    #[test]
    fn instance_unparsable_version_yields_fail_closed() {
        assert_eq!(
            resolve_instance(Some("???"), "1.0.0"),
            InstanceDecision::YieldFailClosed
        );
        assert_eq!(
            resolve_instance(Some("1.0.0"), "???"),
            InstanceDecision::YieldFailClosed
        );
    }

    #[test]
    fn instance_version_arg_parses() {
        let args = vec![
            "RSH.exe".to_string(),
            "--rsh-version".to_string(),
            "1.2.3".to_string(),
        ];
        assert_eq!(parse_instance_version_arg(&args), Some("1.2.3".to_string()));
        let no_flag = vec!["RSH.exe".to_string()];
        assert_eq!(parse_instance_version_arg(&no_flag), None);
        let dangling = vec!["RSH.exe".to_string(), "--rsh-version".to_string()];
        assert_eq!(parse_instance_version_arg(&dangling), None);
    }

    // -- autostart repair ----------------------------------------------------

    /// Fake Run-key store for tests.
    #[derive(Default)]
    struct FakeStore {
        value: Option<String>,
    }

    impl RunKeyStore for FakeStore {
        fn read_rsh(&self) -> Option<String> {
            self.value.clone()
        }

        fn write_rsh(&mut self, command: &str) -> Result<(), SystemError> {
            self.value = Some(command.to_string());
            Ok(())
        }

        fn delete_rsh(&mut self) -> Result<(), SystemError> {
            self.value = None;
            Ok(())
        }
    }

    fn old_exe() -> PathBuf {
        PathBuf::from(r"C:\Old\RSH\RSH.exe")
    }

    fn new_exe() -> PathBuf {
        PathBuf::from(r"C:\Program Files\RSH\RSH.exe")
    }

    #[test]
    fn autostart_stale_entry_is_repaired() {
        let mut store = FakeStore {
            value: Some("\"C:\\Old\\RSH\\RSH.exe\"".to_string()),
        };
        let outcome = repair_autostart(&mut store, &new_exe()).unwrap();
        assert_eq!(
            outcome,
            AutostartRepair::Repaired {
                old_command: "\"C:\\Old\\RSH\\RSH.exe\"".to_string()
            }
        );
        assert_eq!(
            store.value.as_deref(),
            Some("\"C:\\Program Files\\RSH\\RSH.exe\"")
        );
    }

    #[test]
    fn autostart_correct_entry_is_untouched() {
        let mut store = FakeStore {
            value: Some("\"C:\\Program Files\\RSH\\RSH.exe\" --tray".to_string()),
        };
        let outcome = repair_autostart(&mut store, &new_exe()).unwrap();
        assert_eq!(outcome, AutostartRepair::AlreadyCorrect);
        assert_eq!(
            store.value.as_deref(),
            Some("\"C:\\Program Files\\RSH\\RSH.exe\" --tray")
        );
    }

    #[test]
    fn autostart_absent_entry_is_left_absent() {
        let mut store = FakeStore::default();
        let outcome = repair_autostart(&mut store, &new_exe()).unwrap();
        assert_eq!(outcome, AutostartRepair::NotPresent);
        assert_eq!(store.value, None);
    }

    #[test]
    fn autostart_case_insensitive_path_match() {
        let mut store = FakeStore {
            value: Some("\"c:\\program files\\rsh\\rsh.exe\"".to_string()),
        };
        let outcome = repair_autostart(&mut store, &new_exe()).unwrap();
        assert_eq!(outcome, AutostartRepair::AlreadyCorrect);
    }

    #[test]
    fn autostart_unquoted_old_path_is_repaired() {
        let mut store = FakeStore {
            value: Some("C:\\Old\\RSH\\RSH.exe".to_string()),
        };
        let outcome = repair_autostart(&mut store, &new_exe()).unwrap();
        assert!(matches!(outcome, AutostartRepair::Repaired { .. }));
    }

    #[test]
    fn autostart_disable_deletes_entry() {
        let mut store = FakeStore {
            value: Some("\"C:\\Program Files\\RSH\\RSH.exe\"".to_string()),
        };
        disable_autostart(&mut store).unwrap();
        assert_eq!(store.value, None);
    }

    #[test]
    fn exe_from_command_handles_quoted_and_bare() {
        assert_eq!(
            exe_from_command("\"C:\\Program Files\\RSH\\RSH.exe\" --tray"),
            PathBuf::from(r"C:\Program Files\RSH\RSH.exe")
        );
        assert_eq!(
            exe_from_command("C:\\RSH\\RSH.exe --tray"),
            PathBuf::from(r"C:\RSH\RSH.exe")
        );
        assert_eq!(exe_from_command(&old_exe().to_string_lossy()), old_exe());
    }

    // -- tray guard ------------------------------------------------------------

    #[test]
    fn tray_guard_refuses_hide_without_icon() {
        let guard = TrayGuard::new(false);
        assert!(!guard.can_hide_window());
        assert_eq!(guard.authorise_hide(), Err(SystemError::TrayMissing));
    }

    #[test]
    fn tray_guard_allows_hide_with_icon() {
        let mut guard = TrayGuard::new(false);
        guard.set_icon_present(true);
        assert!(guard.can_hide_window());
        assert_eq!(guard.authorise_hide(), Ok(()));
        guard.set_icon_present(false);
        assert_eq!(guard.authorise_hide(), Err(SystemError::TrayMissing));
    }

    // -- firewall spec ---------------------------------------------------------

    #[test]
    fn firewall_spec_describes_program_scope() {
        let spec = FirewallRuleSpec::inbound_allow(&new_exe());
        assert_eq!(spec.name, "RSH");
        assert_eq!(spec.exe_path, new_exe());
        let description = spec.describe();
        assert!(description.contains("RSH"));
        assert!(description.contains("all profiles"));
        assert!(description.contains("no port scope"));
    }

    #[test]
    fn firewall_absent_state_is_all_false() {
        let state = FirewallState::absent();
        assert!(!state.rule_present);
        assert!(!state.rule_enabled);
        assert_eq!(state.exe_path, None);
    }

    // -- shutdown-neutral ------------------------------------------------------

    /// Recording sink: remembers every state it was sent.
    #[derive(Default)]
    struct RecordingSink {
        sent: Vec<OutputState>,
        fail: bool,
    }

    impl OutputSink for RecordingSink {
        fn send(&mut self, state: &OutputState) -> Result<(), SinkError> {
            if self.fail {
                return Err(SinkError::new("sink refused"));
            }
            self.sent.push(*state);
            Ok(())
        }
    }

    #[test]
    fn shutdown_sends_neutral_after_stop() {
        let mut sink = RecordingSink::default();
        // A non-neutral state sent before shutdown, to prove the last
        // state after shutdown is neutral.
        let mut live = OutputState::neutral();
        live.steering = 0.8;
        live.accelerator = 1.0;
        live.buttons = 0b101;
        sink.send(&live).unwrap();
        assert_ne!(*sink.sent.last().unwrap(), OutputState::neutral());

        let outcome = shutdown_output(None, &mut sink);
        assert!(!outcome.stop_signalled);
        assert!(outcome.neutral_sent);
        assert_eq!(*sink.sent.last().unwrap(), OutputState::neutral());
    }

    #[test]
    fn shutdown_neutral_send_failure_is_reported_not_panicking() {
        let mut sink = RecordingSink {
            fail: true,
            ..RecordingSink::default()
        };
        assert!(!send_neutral(&mut sink));
        let outcome = shutdown_output(None, &mut sink);
        assert!(!outcome.neutral_sent);
    }
}
