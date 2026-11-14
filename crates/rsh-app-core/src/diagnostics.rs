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

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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
        let path = log_dir.join(PANIC_LOG_NAME);
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = file.write_all(line.as_bytes());
        }
    }));
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
}
