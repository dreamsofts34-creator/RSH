//! Diagnostics export + crash-dump support (SW-R7.6).
//!
//! The export bundle is logs + versions + system info — and **never**
//! secrets. Log lines are redacted before they enter the bundle
//! (defence in depth: the logger itself must never write secrets,
//! see the S-11 no-secrets rule), and caller-supplied notes are
//! redacted at build time too. The serialised JSON is deliberately
//! NOT re-processed after serialisation — a line-based redaction
//! pass over JSON would truncate values and corrupt the document.
//! Tests assert the exported file contains none of the planted
//! secret values.
//!
//! Crash dumps: the Rust panic hook is installed here (portable,
//! testable — it appends the panic message + location to a file).
//! The Windows native crash dump (`MiniDumpWriteDump` on an
//! unhandled exception) is a Windows-only design documented in
//! `docs/DIAGNOSTICS.md` §2; it needs the `windows` crate `dbghelp`
//! feature and is a Phase H / Windows-CI item, not compiled here.
//!
//! Startup log: [`append_startup_log`] appends timestamped lines to
//! the daily-rotated `rsh-YYYYMMDD.log` in the log directory, so a
//! startup failure leaves a trace on disk even when the console is
//! gone (the v1.0.0 launch failure left `%AppData%\RSH\logs` empty
//! because startup only ever used `eprintln`).

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Log lines whose value after `=`/`:` is a secret. Matched
/// case-insensitively against the *key* part of a line; the bare word
/// `key` is deliberately NOT in this list (it appears in benign
/// contexts such as "hotkey" / "key binding") — device and signing
/// keys are covered by the `device_key` / `signing` entries instead.
pub const SECRET_KEYS: &[&str] = &[
    "password",
    "passwd",
    "secret",
    "token",
    "api-key",
    "api_key",
    "apikey",
    "hmac",
    "private",
    "credential",
    "bearer",
    "auth",
    "wifi",
    "device_key",
    "device-key",
    "signing",
];

/// Placeholder written in place of a redacted secret value.
pub const REDACTED: &str = "[REDACTED]";

/// Errors from the diagnostics export.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiagnosticsError {
    /// A log directory or output path could not be read/written.
    Io(String),
}

impl std::fmt::Display for DiagnosticsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(message) => write!(f, "diagnostics I/O error: {message}"),
        }
    }
}

impl std::error::Error for DiagnosticsError {}

/// Redact secret values from free text, line by line.
///
/// For each line, when a [`SECRET_KEYS`] word appears before a `=` or
/// `:` separator, everything after that separator is replaced with
/// `[REDACTED]`. Lines with no secret key pass through unchanged. A
/// line that mentions a private key without a separator is redacted
/// in full.
#[must_use]
pub fn redact_secrets(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (index, line) in text.lines().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(&redact_line(line));
    }
    if text.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Redact one line (see [`redact_secrets`]).
///
/// A secret key matches when the first `=`/`:` after it is separated
/// from the key only by blanks, quotes, `_`/`-`, or the word `key`
/// (so `password=…`, `api_key: …`, and `device hmac key: …` all
/// redact, while `author: …` does not — `auth` followed by `or` is
/// not a key).
fn redact_line(line: &str) -> String {
    let lower = line.to_lowercase();
    if lower.contains("private key") && !lower.contains('=') && !lower.contains(':') {
        return REDACTED.to_string();
    }
    // L-3: `Authorization: Basic/Bearer <token>` — the generic
    // key/separator check below misses this (`auth` in
    // `authorization` fails the between-check, and `Bearer` has no
    // separator). Any line naming the Authorization header redacts
    // from the `:` on.
    if lower.contains("authorization") {
        if let Some(colon) = line.find(':') {
            return format!("{} {REDACTED}", &line[..=colon]);
        }
        return REDACTED.to_string();
    }
    // L-3: a bare `Bearer <token>` / `Basic <base64>` anywhere (a
    // header value pasted into a log without its header name). The
    // token must look like a credential (≥ 12 chars, base64/JWT
    // alphabet with a digit or separator) so benign words such as
    // "basic settings" are not redacted.
    for scheme in ["bearer", "basic"] {
        let mut search_from = 0;
        while let Some(at) = lower[search_from..]
            .find(scheme)
            .map(|offset| offset + search_from)
        {
            search_from = at + scheme.len();
            let before_ok = at == 0 || !lower.as_bytes()[at - 1].is_ascii_alphanumeric();
            let after = &line[at + scheme.len()..];
            let Some(rest) = after.strip_prefix([' ', '\t']) else {
                continue;
            };
            let token: &str = rest.split_whitespace().next().unwrap_or("");
            let looks_like_credential = token.len() >= 12
                && token
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || "+/=_.-".contains(ch))
                && token
                    .chars()
                    .any(|ch| ch.is_ascii_digit() || "+/=_.-".contains(ch));
            if before_ok && looks_like_credential {
                return format!("{} {REDACTED}", &line[..at + scheme.len()]);
            }
        }
    }
    for key in SECRET_KEYS {
        let mut search_from = 0;
        while let Some(key_at) = lower[search_from..]
            .find(key)
            .map(|offset| offset + search_from)
        {
            let after = &line[key_at + key.len()..];
            let Some(separator_at) = after.find(['=', ':']) else {
                break;
            };
            // What sits between the key and the separator: only
            // blanks, quotes (JSON `"key": value`), or the word `key`
            // (`device hmac key:`) may appear — anything else means
            // this occurrence is a different word (`auth` in
            // `author:`).
            let between = after[..separator_at]
                .trim()
                .trim_matches(|ch| ch == '"' || ch == '\'')
                .trim()
                .to_lowercase();
            if between.is_empty() || between == "key" {
                let value_start = key_at + key.len() + separator_at + 1;
                return format!("{} {REDACTED}", &line[..value_start]);
            }
            // False positive for this occurrence (e.g. `auth` in
            // `author:`) — keep looking for a later occurrence.
            search_from = key_at + key.len();
        }
    }
    line.to_string()
}

/// The diagnostics bundle: what `export_diagnostics` writes.
///
/// Deliberately small: versions + OS/arch + a redacted tail of the
/// app logs. No settings file is embedded (it can hold the Wi-Fi
/// password), no keystore content, no full logs — the tail is capped
/// (see [`collect_log_tail`]) so an export stays emailable.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsBundle {
    /// App version (Rust series, e.g. `"1.0.0"`).
    pub app_version: String,
    /// `std::env::consts::OS` (`"windows"` / `"linux"`).
    pub os: String,
    /// `std::env::consts::ARCH` (`"x86_64"`).
    pub arch: String,
    /// Seconds since the Unix epoch when the bundle was built.
    pub generated_unix_secs: u64,
    /// Redacted tail of the app logs, newest last.
    pub log_tail: Vec<String>,
    /// Free-form notes (e.g. "exported from Settings → Diagnostics").
    pub notes: Vec<String>,
}

/// Collect the redacted tail of the app logs in `log_dir`.
///
/// Reads `rsh-*.log` files (the S-11 daily-rotation name) oldest
/// first, keeps the last `max_lines` lines overall, redacts each
/// line. A missing/unreadable directory yields an empty tail, not an
/// error — diagnostics must work on a broken install.
#[must_use]
pub fn collect_log_tail(log_dir: &Path, max_lines: usize) -> Vec<String> {
    let mut names: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = fs::read_dir(log_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("rsh-") && name.ends_with(".log") {
                names.push(entry.path());
            }
        }
    }
    names.sort();
    let mut lines: Vec<String> = Vec::new();
    for path in names {
        if let Ok(contents) = fs::read_to_string(&path) {
            lines.extend(contents.lines().map(str::to_string));
        }
    }
    if lines.len() > max_lines {
        lines.drain(..lines.len() - max_lines);
    }
    lines.iter().map(|line| redact_line(line)).collect()
}

/// Default cap for the exported log tail: enough to diagnose a
/// startup failure, small enough to email.
pub const DEFAULT_LOG_TAIL_LINES: usize = 200;

/// Build a bundle from a log directory (no file is written).
#[must_use]
pub fn build_bundle(log_dir: &Path, app_version: &str) -> DiagnosticsBundle {
    build_bundle_with_notes(log_dir, app_version, &[])
}

/// Build a bundle with extra notes (each note is redacted — see
/// [`redact_secrets`] — before it enters the bundle).
#[must_use]
pub fn build_bundle_with_notes(
    log_dir: &Path,
    app_version: &str,
    notes: &[String],
) -> DiagnosticsBundle {
    let generated_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    DiagnosticsBundle {
        app_version: app_version.to_string(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        generated_unix_secs,
        log_tail: collect_log_tail(log_dir, DEFAULT_LOG_TAIL_LINES),
        notes: notes.iter().map(|note| redact_secrets(note)).collect(),
    }
}

/// Export a diagnostics bundle as pretty JSON to `out_path`.
///
/// Every string that enters the bundle is already redacted at build
/// time (log tail in [`collect_log_tail`], notes in
/// [`build_bundle_with_notes`]); the serialised JSON is deliberately
/// NOT re-processed — a line-based redaction pass over JSON would
/// truncate values and corrupt the document (a redaction that cuts
/// a closing quote breaks the string it sits in).
/// Returns the path written.
///
/// # Errors
///
/// Returns [`DiagnosticsError::Io`] when the output file cannot be
/// written.
pub fn export_diagnostics(
    log_dir: &Path,
    out_path: &Path,
    app_version: &str,
) -> Result<PathBuf, DiagnosticsError> {
    let bundle = build_bundle(log_dir, app_version);
    let json = serde_json::to_string_pretty(&bundle)
        .map_err(|error| DiagnosticsError::Io(error.to_string()))?;
    fs::write(out_path, json).map_err(|error| DiagnosticsError::Io(error.to_string()))?;
    Ok(out_path.to_path_buf())
}

/// File the panic hook appends Rust panic reports to.
pub const PANIC_LOG_NAME: &str = "diagnostics-panic.log";

/// Install a panic hook that appends `panic at <location>:
/// <payload>` lines to `log_dir/diagnostics-panic.log`.
///
/// This is the portable half of crash capture: a Rust panic (a bug,
/// never a normal exit) leaves a trace next to the logs even when
/// the process dies before flushing anything else. The hook never
/// panics itself — a failed write is silently dropped — and it never
/// writes secret values (panic payloads are code strings, but the
/// line is redacted anyway). Native Windows crash dumps
/// (`MiniDumpWriteDump`) are documented in `docs/DIAGNOSTICS.md` §2.
///
/// v1.0.2: the report is written to BOTH log locations — the given
/// `log_dir` and the `%TEMP%` fallback ([`temp_log_dir`]) — so a
/// panic still leaves evidence when the data dir is unwritable or
/// resolves under a different user (the v1.0.1 launch failure left
/// `%AppData%\RSH\logs` empty).
pub fn install_panic_hook(log_dir: PathBuf) {
    std::panic::set_hook(Box::new(move |info| {
        let location = info.location().map_or_else(
            || "unknown location".to_string(),
            std::string::ToString::to_string,
        );
        let payload = if let Some(message) = info.payload().downcast_ref::<&str>() {
            (*message).to_string()
        } else if let Some(message) = info.payload().downcast_ref::<String>() {
            message.clone()
        } else {
            "non-string panic payload".to_string()
        };
        let line = redact_secrets(&format!("panic at {location}: {payload}\n"));
        append_panic_line(&log_dir.join(PANIC_LOG_NAME), &line);
        let fallback_path = temp_log_dir().join(PANIC_LOG_NAME);
        if fallback_path != log_dir.join(PANIC_LOG_NAME) {
            append_panic_line(&fallback_path, &line);
        }
    }));
}

/// One best-effort append for the panic hook: the line is flushed
/// before returning (the process may die right after the hook runs,
/// and a buffered line lost in a crash is no evidence at all), and
/// a failed write is silently dropped — the hook never panics.
fn append_panic_line(path: &Path, line: &str) {
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        // Flush before returning: the process may die right after
        // the hook runs, and a buffered line lost in a crash is
        // no evidence at all (v1.0.1 acceptance: the file log is
        // the evidence for the next run).
        let _ = file.write_all(line.as_bytes());
        let _ = file.flush();
    }
}

/// Daily startup-log file name: `rsh-YYYYMMDD.log` (the S-11
/// daily-rotation name [`collect_log_tail`] already reads, so the
/// diagnostics export picks these lines up).
///
/// Timestamps are UTC: `std` has no local-time API and this crate
/// takes no `chrono` dependency, so the file name and the line
/// timestamp use the same UTC civil date.
fn startup_log_path_for(log_dir: &Path, now: SystemTime) -> PathBuf {
    let (year, month, day, _, _, _) = utc_civil_parts(now);
    log_dir.join(format!("rsh-{year:04}{month:02}{day:02}.log"))
}

/// `(year, month, day, hour, minute, second)` in UTC for `now`
/// (Howard Hinnant's days-from-civil algorithm; std-only).
fn utc_civil_parts(now: SystemTime) -> (i64, u32, u32, u32, u32, u32) {
    let secs = i64::try_from(
        now.duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs()),
    )
    .unwrap_or(i64::MAX);
    let days = secs.div_euclid(86_400);
    let secs_of_day = secs.rem_euclid(86_400);
    let hour = u32::try_from(secs_of_day / 3_600).unwrap_or(23);
    let minute = u32::try_from((secs_of_day % 3_600) / 60).unwrap_or(59);
    let second = u32::try_from(secs_of_day % 60).unwrap_or(59);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = u32::try_from(z.rem_euclid(146_097)).unwrap_or(0);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = i64::from(yoe) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    (year, month, day, hour, minute, second)
}

/// Current UTC time as an ISO-8601 `Z` timestamp
/// (`2026-11-16T06:30:00Z`) — the timestamp on pairing/connection
/// log lines (Phase 0 gate §6.2).
#[must_use]
pub fn utc_now_iso() -> String {
    let (year, month, day, hour, minute, second) = utc_civil_parts(SystemTime::now());
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Append one timestamped line to today's startup log
/// (`log_dir/rsh-YYYYMMDD.log`):
/// `[YYYY-MM-DD HH:MM:SS] <message>`.
///
/// The log directory is created when missing. A message is redacted
/// ([`redact_secrets`]) and flattened to one line before it is
/// written, so a secret can never reach the file through this
/// helper. Any failure (unwritable directory, disk full, …) is
/// silently ignored — startup must never fail because logging did.
///
/// Returns the path written, or [`None`] when nothing could be
/// written.
#[must_use]
pub fn append_startup_log(log_dir: &Path, message: &str) -> Option<PathBuf> {
    append_startup_log_at(log_dir, message, SystemTime::now())
}

/// [`append_startup_log`] with an injectable clock (the test seam:
/// tests pin the time to prove the date rollover).
fn append_startup_log_at(log_dir: &Path, message: &str, now: SystemTime) -> Option<PathBuf> {
    let line = format_startup_log_line(message, now);
    write_log_line(log_dir, &line, now)
}

/// Format one startup-log line (DS-77): flatten + redact + UTC
/// timestamp, exactly once. The formatted line is written to every
/// location by `write_log_line`, so redaction no longer runs once
/// per location.
fn format_startup_log_line(message: &str, now: SystemTime) -> String {
    let (year, month, day, hour, minute, second) = utc_civil_parts(now);
    let single_line: String = message
        .chars()
        .map(|ch| if ch == '\n' || ch == '\r' { ' ' } else { ch })
        .collect();
    format!(
        "[{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}] {}\n",
        redact_secrets(&single_line)
    )
}

/// Identity of an open log file, for staleness checks (DS-77).
#[derive(Clone, Copy, PartialEq, Eq)]
struct FileIdentity(u64, u64);

#[cfg(unix)]
fn identity_of(meta: &fs::Metadata) -> FileIdentity {
    use std::os::unix::fs::MetadataExt;
    FileIdentity(meta.dev(), meta.ino())
}

#[cfg(windows)]
fn identity_of(meta: &fs::Metadata) -> FileIdentity {
    // `MetadataExt::file_index` is unstable (`windows_by_handle`)
    // on this toolchain, so the identity is (length, mtime-nanos):
    // a deleted-and-recreated log gets a new mtime and a truncated
    // log a new length, so both are detected like Unix dev+ino.
    use std::time::UNIX_EPOCH;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX));
    FileIdentity(meta.len(), mtime)
}

#[cfg(not(any(unix, windows)))]
fn identity_of(_meta: &fs::Metadata) -> FileIdentity {
    FileIdentity(0, 0)
}

/// One held-open log file (DS-77): the file is opened once and
/// reused across appends, so a log line costs one `write_all`
/// instead of `create_dir_all` + open + write + flush + close.
/// `identity` is the file's identity at open time (Unix dev+ino,
/// Windows length+mtime); it is checked against the live path on
/// every write, so a deleted-and-recreated log (or a test that
/// wipes its temp dir) is detected and the handle reopened.
struct OpenLog {
    file: File,
    identity: FileIdentity,
    /// When the on-disk identity was last re-checked (R2-45):
    /// the per-line `fs::metadata` staleness check costs a syscall
    /// on every log line (× 2 locations for fatal lines), so the
    /// identity is re-checked at most once per
    /// [`LOG_IDENTITY_CHECK_INTERVAL`]; between checks the cached
    /// handle is trusted.
    last_checked: Instant,
}

/// Re-check a held-open log file's on-disk identity at most this
/// often (R2-45): 60 s is far shorter than any log rotation, and
/// long enough that the per-line metadata syscalls disappear from
/// the steady-state log path.
const LOG_IDENTITY_CHECK_INTERVAL: Duration = Duration::from_secs(60);

/// Held-open log handles, keyed by full log path (DS-77).
static LOG_HANDLES: std::sync::LazyLock<std::sync::Mutex<HashMap<PathBuf, OpenLog>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

/// Log directories already created this process (DS-77): the
/// `create_dir_all` syscall is cached per directory.
static LOG_DIRS_CREATED: std::sync::LazyLock<std::sync::Mutex<HashSet<PathBuf>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(HashSet::new()));

/// Create `log_dir` once (DS-77): the `create_dir_all` syscall is
/// cached per directory for the process lifetime, so a log line no
/// longer stats/creates the directory every time. A later open
/// failure recreates it on the spot (see `open_log_file`).
fn ensure_log_dir(log_dir: &Path) {
    let mut created = LOG_DIRS_CREATED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if created.contains(log_dir) {
        return;
    }
    if fs::create_dir_all(log_dir).is_ok() {
        created.insert(log_dir.to_path_buf());
    }
}

/// Open (or create) a log file for append (DS-77). On failure the
/// directory may have vanished after the cached create (a wipe
/// raced us): recreate it and retry once. The cache entry is left
/// alone — the directory exists again after the retry, so it is
/// true again.
fn open_log_file(path: &Path, log_dir: &Path) -> Option<File> {
    if let Ok(file) = OpenOptions::new().create(true).append(true).open(path) {
        Some(file)
    } else {
        let _ = fs::create_dir_all(log_dir);
        OpenOptions::new().create(true).append(true).open(path).ok()
    }
}

/// Write one pre-formatted line to the daily log in `log_dir`
/// (DS-77): the directory is created once (cached), the file is
/// opened once and held open, and the line is a single `write_all`
/// — no open/close per line, and no flush-loss risk (the handle is
/// unbuffered, so the bytes are in the kernel as soon as the call
/// returns — a `process::exit` after this cannot lose them, the way
/// a `BufWriter` without a flush could).
///
/// Staleness: the handle is checked against the live path on every
/// write (identity mismatch, or the path gone) and — on Unix — the
/// open file's link count (a deleted file has `nlink == 0`, which
/// also catches a delete-and-recreate that reused the same inode).
/// A stale handle is dropped and reopened. At most 8 handles are
/// held at once (a long-running daemon rolls the file daily, and
/// tests churn temp dirs); past that the map is cleared and the
/// live paths reopen on demand.
///
/// Returns the path written, or [`None`] when nothing could be
/// written.
fn write_log_line(log_dir: &Path, line: &str, now: SystemTime) -> Option<PathBuf> {
    let path = startup_log_path_for(log_dir, now);
    ensure_log_dir(log_dir);
    let mut handles = LOG_HANDLES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let stale = match handles.get(&path) {
        Some(open) => {
            // R2-45: the unlink check runs every line — it is a
            // single `fstat` on the already-open fd (cheap, and it
            // is what catches a delete+recreate / external rotation
            // immediately; see the DS-77 test). Only the *path*
            // `fs::metadata` identity comparison is throttled to
            // once per `LOG_IDENTITY_CHECK_INTERVAL`: the path lookup
            // on every line (× 2 locations) was the steady-state
            // cost, and a replace-with-a-different-file is caught
            // within 60 s. (On non-Unix the unlink check is a stub,
            // so the throttled identity check is the only guard
            // there — deleting an open file is impossible on Windows,
            // so the window only delays replace detection.)
            if file_is_unlinked(&open.file) {
                true
            } else if open.last_checked.elapsed() < LOG_IDENTITY_CHECK_INTERVAL {
                false
            } else {
                match fs::metadata(&path) {
                    Ok(meta) => identity_of(&meta) != open.identity,
                    Err(_) => true,
                }
            }
        }
        None => true,
    };
    if stale {
        handles.remove(&path);
        if handles.len() >= 8 {
            handles.clear();
        }
        let file = open_log_file(&path, log_dir)?;
        let identity = file
            .metadata()
            .ok()
            .map_or(FileIdentity(0, 0), |meta| identity_of(&meta));
        handles.insert(
            path.clone(),
            OpenLog {
                file,
                identity,
                last_checked: Instant::now(),
            },
        );
    }
    let open = handles.get_mut(&path)?;
    // R2-45: record the check time on the re-check path — the
    // `stale` branch above runs only when the interval elapsed, so
    // updating here keeps the 60 s cadence without a second clock
    // read on the hot path.
    if open.last_checked.elapsed() >= LOG_IDENTITY_CHECK_INTERVAL {
        open.last_checked = Instant::now();
    }
    open.file.write_all(line.as_bytes()).ok()?;
    Some(path)
}

/// Whether the open file has been unlinked from its directory
/// (DS-77): a deleted file has link count 0, while a live file has
/// at least 1. Unix-only — on Windows the identity check in
/// `write_log_line` covers the common cases (delete without
/// recreate, replace with a different file index).
#[cfg(unix)]
fn file_is_unlinked(file: &File) -> bool {
    use std::os::unix::fs::MetadataExt;
    file.metadata().is_ok_and(|meta| meta.nlink() == 0)
}

#[cfg(not(unix))]
fn file_is_unlinked(_file: &File) -> bool {
    false
}

/// The fallback log directory: `%TEMP%\RSH\logs` on Windows
/// (`std::env::temp_dir()` + `RSH/logs` everywhere else).
///
/// The data directory is the primary log home, but the v1.0.1 launch
/// failure left `%AppData%\RSH\logs` EMPTY — either the data dir was
/// not writable (review bug C) or it resolved under a different user
/// (bug D). A fatal log that only ever tries the data dir can vanish
/// exactly when it matters most, so every fatal/startup write goes to
/// BOTH locations (see [`append_startup_log_both`] / [`log_fatal`]).
#[must_use]
pub fn temp_log_dir() -> PathBuf {
    std::env::temp_dir().join("RSH").join("logs")
}

/// Append one line to today's startup log in BOTH log locations —
/// the data-dir logs and the `%TEMP%` fallback ([`temp_log_dir`]).
/// Best-effort and never panicking: a location that cannot be
/// written is skipped, and the write is done once (not duplicated)
/// when both locations resolve to the same directory. Returns the
/// files actually written.
#[must_use]
pub fn append_startup_log_both(message: &str) -> Vec<PathBuf> {
    let mut written = Vec::new();
    // DS-77: the line is formatted (flatten + redact + timestamp)
    // once and written to both locations — redaction no longer runs
    // once per location, and both files carry the same timestamp.
    let now = SystemTime::now();
    let line = format_startup_log_line(message, now);
    let primary = crate::data_dir().map(|dir| dir.join("logs"));
    if let Some(dir) = &primary
        && let Some(path) = write_log_line(dir, &line, now)
    {
        written.push(path);
    }
    let fallback = temp_log_dir();
    if primary.as_deref() != Some(fallback.as_path())
        && let Some(path) = write_log_line(&fallback, &line, now)
    {
        written.push(path);
    }
    written
}

/// Write a fatal startup failure to both log locations and stderr.
///
/// The message is prefixed `FATAL: ` so it stands out in the log
/// tail. Both writes are best-effort (see [`append_startup_log_both`])
/// — this function never panics and never fails the caller, so it is
/// safe to call on the exit path right before `process::exit`.
/// Returns the files actually written (empty when neither location
/// was writable — the stderr line is still emitted).
#[must_use]
pub fn log_fatal(message: &str) -> Vec<PathBuf> {
    let line = format!("FATAL: {message}");
    eprintln!("[RSH] {line}");
    append_startup_log_both(&line)
}

/// [`log_fatal`] with injectable log directories (the test seam:
/// tests pin both locations to temp dirs so the real data dir is
/// never touched).
#[must_use]
pub fn log_fatal_to(
    message: &str,
    primary_log_dir: Option<&Path>,
    fallback_log_dir: &Path,
) -> Vec<PathBuf> {
    let line = format!("FATAL: {message}");
    eprintln!("[RSH] {line}");
    let mut written = Vec::new();
    // DS-77: format once, write to both locations.
    let now = SystemTime::now();
    let formatted = format_startup_log_line(&line, now);
    if let Some(dir) = primary_log_dir
        && let Some(path) = write_log_line(dir, &formatted, now)
    {
        written.push(path);
    }
    if primary_log_dir != Some(fallback_log_dir)
        && let Some(path) = write_log_line(fallback_log_dir, &formatted, now)
    {
        written.push(path);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // The panic-hook test installs a global hook; serialise it so no
    // other test observes a swapped hook mid-run.
    static HOOK_MUTEX: Mutex<()> = Mutex::new(());

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rsh-diagnostics-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn redact_password_assignment() {
        assert_eq!(
            redact_secrets("wifi_password=hunter2-secret"),
            "wifi_password= [REDACTED]"
        );
        assert_eq!(
            redact_secrets("password: hunter2-secret"),
            "password: [REDACTED]"
        );
    }

    #[test]
    fn redact_json_secret_field() {
        let redacted = redact_secrets(r#"{"api_key": "sk-live-12345"}"#);
        assert!(!redacted.contains("sk-live-12345"), "{redacted}");
        assert!(redacted.contains(REDACTED), "{redacted}");
    }

    #[test]
    fn redact_token_and_hmac_lines() {
        let text = "token=abc123\ndevice hmac key: deadbeef\nnormal line stays";
        let redacted = redact_secrets(text);
        assert!(!redacted.contains("abc123"), "{redacted}");
        assert!(!redacted.contains("deadbeef"), "{redacted}");
        assert!(redacted.contains("normal line stays"), "{redacted}");
    }

    #[test]
    fn redact_leaves_benign_lines_alone() {
        let text = "author: Jane Doe\nkey binding: F13\nconnected to board";
        let redacted = redact_secrets(text);
        assert_eq!(redacted, text);
    }

    #[test]
    fn redact_authorization_header_and_bare_bearer() {
        // L-3: the old redactor missed both of these.
        let redacted = redact_secrets("Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.payload.sig");
        assert!(!redacted.contains("eyJhbGci"), "{redacted}");
        assert_eq!(redacted, "Authorization: [REDACTED]");
        let redacted = redact_secrets("Authorization: Basic dXNlcjpwYXNzd29yZA==");
        assert!(!redacted.contains("dXNlcjpw"), "{redacted}");
        assert_eq!(redacted, "Authorization: [REDACTED]");
        let redacted = redact_secrets("retrying with Bearer abcdefgh1234.token");
        assert!(!redacted.contains("abcdefgh1234"), "{redacted}");
        assert!(redacted.contains("Bearer [REDACTED]"), "{redacted}");
        // Benign "basic" stays untouched.
        assert_eq!(
            redact_secrets("basic settings loaded"),
            "basic settings loaded"
        );
    }

    #[test]
    fn log_tail_caps_lines_and_redacts() {
        let dir = temp_dir("tail");
        let mut lines: Vec<String> = (0..10).map(|index| format!("line {index}")).collect();
        lines.push("wifi_password=planted-secret-value".to_string());
        let contents = lines.join("\n") + "\n";
        fs::write(dir.join("rsh-20261114.log"), contents).expect("write log");
        let tail = collect_log_tail(&dir, 3);
        assert_eq!(tail.len(), 3);
        assert!(tail.iter().any(|line| line.contains(REDACTED)), "{tail:?}");
        assert!(
            !tail
                .iter()
                .any(|line| line.contains("planted-secret-value")),
            "{tail:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn log_tail_missing_dir_is_empty_not_error() {
        let tail = collect_log_tail(Path::new("/definitely/not/here/logs"), 10);
        assert_eq!(tail.len(), 0);
    }

    #[test]
    fn export_bundle_has_no_secrets() {
        let dir = temp_dir("export");
        fs::write(
            dir.join("rsh-20261114.log"),
            "engine started\nwifi_password=planted-secret-value\napi_key=planted-api-key\ntoken: planted-token\n",
        )
        .expect("write log");
        let out_path = dir.join("diagnostics.json");
        let written = export_diagnostics(&dir, &out_path, "1.0.0").expect("export");
        assert_eq!(written, out_path);
        let json = fs::read_to_string(&out_path).expect("read export");
        for planted in ["planted-secret-value", "planted-api-key", "planted-token"] {
            assert!(!json.contains(planted), "export leaked {planted}: {json}");
        }
        assert!(json.contains(REDACTED), "{json}");
        // And the file is valid JSON with the expected shape.
        let parsed: DiagnosticsBundle = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(parsed.app_version, "1.0.0");
        assert_eq!(parsed.os, std::env::consts::OS);
        assert!(
            parsed
                .log_tail
                .iter()
                .any(|line| line.contains("engine started"))
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_to_unwritable_path_is_an_error() {
        let dir = temp_dir("export-bad");
        let result = export_diagnostics(
            &dir,
            Path::new("/definitely/not/here/diagnostics.json"),
            "1.0.0",
        );
        assert!(matches!(result, Err(DiagnosticsError::Io(_))));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn panic_hook_writes_redacted_report() {
        let _guard = HOOK_MUTEX
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = temp_dir("panic");
        let previous = std::panic::take_hook();
        install_panic_hook(dir.clone());
        let result = std::panic::catch_unwind(|| {
            panic!("test panic with password=planted-panic-secret");
        });
        assert!(result.is_err());
        std::panic::set_hook(previous);
        let log = fs::read_to_string(dir.join(PANIC_LOG_NAME)).expect("panic log written");
        assert!(log.contains("test panic"), "{log}");
        assert!(!log.contains("planted-panic-secret"), "{log}");
        assert!(log.contains(REDACTED), "{log}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn utc_civil_parts_known_dates() {
        assert_eq!(utc_civil_parts(UNIX_EPOCH), (1970, 1, 1, 0, 0, 0));
        // 2026-11-15 00:00:00 UTC (verified with `date -u -d @1794700800`).
        assert_eq!(
            utc_civil_parts(UNIX_EPOCH + std::time::Duration::from_secs(1_794_700_800)),
            (2026, 11, 15, 0, 0, 0)
        );
        // 2026-11-15 23:59:59 UTC.
        assert_eq!(
            utc_civil_parts(UNIX_EPOCH + std::time::Duration::from_secs(1_794_787_199)),
            (2026, 11, 15, 23, 59, 59)
        );
    }

    fn fixed_time(secs: u64) -> SystemTime {
        UNIX_EPOCH + std::time::Duration::from_secs(secs)
    }

    #[test]
    fn startup_log_creates_file_and_appends_timestamped_lines() {
        let dir = temp_dir("startup");
        let log_dir = dir.join("logs");
        let first =
            append_startup_log_at(&log_dir, "RSH v1.0.0 starting", fixed_time(1_794_701_220));
        let second = append_startup_log_at(&log_dir, "setup complete", fixed_time(1_794_701_225));
        assert_eq!(first, second);
        let path = first.expect("log written");
        assert_eq!(path, log_dir.join("rsh-20261115.log"));
        let contents = fs::read_to_string(&path).expect("read log");
        assert_eq!(
            contents,
            "[2026-11-15 00:07:00] RSH v1.0.0 starting\n\
             [2026-11-15 00:07:05] setup complete\n",
            "{contents}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn startup_log_creates_missing_dir() {
        let dir = temp_dir("startup-missing");
        let log_dir = dir.join("deep").join("logs");
        assert!(!log_dir.exists());
        let written = append_startup_log_at(&log_dir, "first line", fixed_time(1_794_700_800));
        assert_eq!(written, Some(log_dir.join("rsh-20261115.log")));
        assert!(log_dir.join("rsh-20261115.log").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn startup_log_rolls_over_at_midnight() {
        let dir = temp_dir("startup-rollover");
        // 2026-11-15 23:59:59 UTC, then 2026-11-16 00:00:00 UTC.
        let before = append_startup_log_at(&dir, "last line of the day", fixed_time(1_794_787_199));
        let after =
            append_startup_log_at(&dir, "first line of the new day", fixed_time(1_794_787_200));
        assert_eq!(before, Some(dir.join("rsh-20261115.log")));
        assert_eq!(after, Some(dir.join("rsh-20261116.log")));
        let old = fs::read_to_string(dir.join("rsh-20261115.log")).expect("old log");
        let new = fs::read_to_string(dir.join("rsh-20261116.log")).expect("new log");
        assert!(
            old.contains("[2026-11-15 23:59:59] last line of the day"),
            "{old}"
        );
        assert!(!old.contains("new day"), "{old}");
        assert!(
            new.contains("[2026-11-16 00:00:00] first line of the new day"),
            "{new}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn startup_log_never_writes_secrets_and_flattens_lines() {
        let dir = temp_dir("startup-secrets");
        let written = append_startup_log_at(
            &dir,
            "password=hunter2-secret\nsecond line stays one line",
            fixed_time(1_794_700_800),
        );
        let path = written.expect("log written");
        let contents = fs::read_to_string(&path).expect("read log");
        assert!(!contents.contains("hunter2-secret"), "{contents}");
        assert!(contents.contains(REDACTED), "{contents}");
        assert_eq!(contents.lines().count(), 1, "{contents}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn startup_log_failure_is_silent() {
        let dir = temp_dir("startup-fail");
        let file_as_dir = dir.join("not-a-dir");
        fs::write(&file_as_dir, "in the way").expect("write blocker file");
        let written =
            append_startup_log_at(&file_as_dir, "cannot be written", fixed_time(1_794_700_800));
        assert_eq!(written, None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn temp_log_dir_is_named_rsh_logs() {
        let dir = temp_log_dir();
        assert_eq!(dir.file_name().and_then(|name| name.to_str()), Some("logs"));
        assert_eq!(
            dir.parent()
                .and_then(|parent| parent.file_name())
                .and_then(|name| name.to_str()),
            Some("RSH")
        );
    }

    #[test]
    fn log_fatal_writes_both_locations() {
        let dir = temp_dir("fatal-both");
        let primary = dir.join("primary");
        let fallback = dir.join("fallback");
        let written = log_fatal_to("data dir missing", Some(&primary), &fallback);
        assert_eq!(written.len(), 2, "{written:?}");
        for path in &written {
            let contents = fs::read_to_string(path).expect("read fatal log");
            assert!(contents.contains("FATAL: data dir missing"), "{contents}");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn log_fatal_falls_back_when_primary_unwritable() {
        // Bug C: the data-dir logs are unwritable — the %TEMP%
        // fallback must still carry the trace.
        let dir = temp_dir("fatal-fallback");
        let blocker = dir.join("not-a-dir");
        fs::write(&blocker, "in the way").expect("write blocker file");
        let fallback = dir.join("fallback");
        let written = log_fatal_to("profile store failed", Some(&blocker), &fallback);
        assert_eq!(written.len(), 1, "{written:?}");
        assert!(written[0].starts_with(&fallback), "{written:?}");
        let contents = fs::read_to_string(&written[0]).expect("read fallback log");
        assert!(
            contents.contains("FATAL: profile store failed"),
            "{contents}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn log_fatal_without_primary_writes_fallback_only() {
        // Bug D: no data dir resolves at all — the fallback is the
        // only trace.
        let dir = temp_dir("fatal-noprimary");
        let fallback = dir.join("fallback");
        let written = log_fatal_to("no data dir", None, &fallback);
        assert_eq!(written.len(), 1, "{written:?}");
        assert!(written[0].starts_with(&fallback), "{written:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn log_fatal_same_dir_writes_once() {
        let dir = temp_dir("fatal-same");
        let written = log_fatal_to("dedup check", Some(&dir), &dir);
        assert_eq!(written.len(), 1, "{written:?}");
        let contents = fs::read_to_string(&written[0]).expect("read fatal log");
        assert_eq!(contents.lines().count(), 1, "{contents}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// DS-77: a held-open handle is detected as stale when its file
    /// is deleted and recreated — the next write reopens the new
    /// file instead of writing into the unlinked inode (the
    /// delete-and-recreate a test teardown does, or an external log
    /// rotation).
    #[test]
    fn ds77_stale_handle_reopens_after_delete_and_recreate() {
        let dir = temp_dir("ws-e-stale");
        let log_dir = dir.join("logs");
        let path = append_startup_log_at(&log_dir, "first line", fixed_time(1_794_700_800))
            .expect("first write");
        assert_eq!(fs::read_to_string(&path).expect("read").lines().count(), 1);
        // Wipe the file (and its directory, as a test teardown
        // would): the held handle now points at an unlinked inode.
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&log_dir).expect("recreate log dir");
        let path2 = append_startup_log_at(&log_dir, "second line", fixed_time(1_794_700_805))
            .expect("second write");
        assert_eq!(path, path2);
        let contents = fs::read_to_string(&path2).expect("read after recreate");
        assert!(contents.contains("second line"), "{contents}");
        assert_eq!(contents.lines().count(), 1, "{contents}");
        let _ = fs::remove_dir_all(&dir);
    }
}
