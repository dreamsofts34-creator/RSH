//! Manager tests against the stub sidecar (Linux).
//!
//! These are the process-manager cases of the 78-test equivalent
//! suite: spawn/hello, request/response, install fast path, create,
//! crash/EOF detection, heartbeat, one retry, then vJoy fallback.
//! The real sidecar + driver are Windows-only; the stub speaks the
//! same JSON-lines protocol.

use std::path::PathBuf;
use std::time::Duration;

use rsh_output_hidmaestro::manager::{HelloInfo, SidecarConfig, SidecarManager};
use rsh_output_hidmaestro::protocol::Command;
use rsh_output_hidmaestro::{
    CUSTOM_RAW_REPORT_SIZE, GamepadFrame, HidMaestroError, PINNED_DLL_SHA256, PINNED_VERSION,
    PROFILE_PID, PROFILE_VID,
};

fn stub_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_hm-stub-sidecar"))
}

fn config(envs: &[(&str, &str)]) -> SidecarConfig {
    let mut cfg = SidecarConfig::with_path(stub_path());
    // Same as backend_tests: the stub speaks stdio only — on Windows
    // the default config would take the elevated `runas` + TCP
    // connect-back path, which the stub can never serve. These tests
    // assert `transport == "stdio"`, so select the stdio path here.
    cfg.elevated = false;
    cfg.spawn_timeout = Duration::from_secs(5);
    cfg.reply_timeout = Duration::from_secs(2);
    cfg.heartbeat_interval = Duration::from_secs(60);
    cfg.extra_env = envs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    cfg
}

fn manager(envs: &[(&str, &str)]) -> SidecarManager {
    SidecarManager::new(config(envs))
}

#[test]
fn spawn_and_hello_ok() {
    let mut m = manager(&[]);
    let info: HelloInfo = m.connect().expect("connect");
    assert_eq!(info.sdk_version.as_deref(), Some(PINNED_VERSION));
    assert!(info.admin);
    assert!(!info.driver_installed);
    assert_eq!(info.transport.as_deref(), Some("stdio"));
    assert!(m.is_running());
    m.shutdown();
    assert!(!m.is_running());
}

#[test]
fn connect_twice_is_noop_success() {
    let mut m = manager(&[]);
    m.connect().unwrap();
    m.connect().unwrap();
    assert!(m.is_running());
    m.shutdown();
}

#[test]
fn ping_round_trip() {
    let mut m = manager(&[]);
    m.connect().unwrap();
    let reply = m.request(&Command::Ping).unwrap();
    assert!(reply.ok);
    assert_eq!(reply.pong, Some(true));
    m.shutdown();
}

#[test]
fn version_round_trip() {
    let mut m = manager(&[]);
    m.connect().unwrap();
    let reply = m.request(&Command::Version).unwrap();
    assert!(reply.ok);
    assert_eq!(reply.sdk.as_deref(), Some(PINNED_VERSION));
    assert_eq!(reply.dll_sha256_ok, Some(true));
    assert_eq!(reply.dll_sha256.as_deref(), Some(PINNED_DLL_SHA256));
    m.shutdown();
}

#[test]
fn describe_profile_reports_15_byte_report() {
    let mut m = manager(&[]);
    m.connect().unwrap();
    let reply = m.request(&Command::DescribeProfile).unwrap();
    assert!(reply.ok);
    assert_eq!(reply.descriptor_len, Some(124));
    assert_eq!(reply.input_report_size, Some(15));
    // The C# 26-byte assumption is recorded, not used.
    assert_eq!(reply.input_report_size_legacy_assumption, Some(26));
    assert_eq!(reply.vid, Some(u32::from(PROFILE_VID)));
    assert_eq!(reply.pid_value, Some(u32::from(PROFILE_PID)));
    assert_eq!(reply.vid_pid_provisional, Some(true));
    // The descriptor is 124 bytes = 248 hex chars.
    assert_eq!(reply.descriptor_hex.as_ref().map(String::len), Some(248));
    m.shutdown();
}

#[test]
fn install_driver_fast_path_when_already_installed() {
    let mut m = manager(&[("HM_STUB_WAS_INSTALLED", "1")]);
    m.connect().unwrap();
    let info = m.install_driver().unwrap();
    assert!(info.was_installed);
    assert!(info.now_installed);
    assert_eq!(info.outcome, Some("AlreadyCurrent"));
    m.shutdown();
}

#[test]
fn install_driver_installs_when_missing() {
    let mut m = manager(&[]);
    m.connect().unwrap();
    let info = m.install_driver().unwrap();
    assert!(!info.was_installed);
    assert!(info.now_installed);
    assert_eq!(info.outcome, Some("Installed"));
    m.shutdown();
}

#[test]
fn install_driver_not_admin() {
    let mut m = manager(&[("HM_STUB_ADMIN", "0")]);
    m.connect().unwrap();
    let err = m.install_driver().unwrap_err();
    assert!(matches!(err, HidMaestroError::NotAdmin));
    m.shutdown();
}

#[test]
fn create_ok_reports_custom_caps() {
    let mut m = manager(&[]);
    m.connect().unwrap();
    let created = m.create().unwrap();
    assert_eq!(created.transport_index, Some(0));
    assert_eq!(created.driver_version.as_deref(), Some(PINNED_VERSION));
    assert_eq!(
        created.capabilities,
        rsh_output_hidmaestro::HidMaestroCapabilities::custom_wheel()
    );
    m.shutdown();
}

#[test]
fn create_refused_is_one_failure_not_fallback_yet() {
    let mut m = manager(&[("HM_STUB_FAIL_CREATE", "1")]);
    m.connect().unwrap();
    let err = m.create().unwrap_err();
    assert!(matches!(err, HidMaestroError::CreateFailed(_)));
    assert_eq!(
        m.consecutive_failures(),
        0,
        "reply errors are not transport failures"
    );
    assert!(!m.fallback_needed());
    m.shutdown();
}

#[test]
fn missing_sidecar_file_is_not_found() {
    let mut cfg = SidecarConfig::with_path(PathBuf::from("/nonexistent/rsh-hm-sidecar"));
    cfg.spawn_timeout = Duration::from_secs(2);
    let mut m = SidecarManager::new(cfg);
    let err = m.connect().unwrap_err();
    assert!(matches!(err, HidMaestroError::SidecarNotFound(_)));
    assert!(!m.fallback_needed());
}

#[test]
fn no_hello_eof_is_sidecar_died() {
    let mut m = manager(&[("HM_STUB_NO_HELLO", "1")]);
    let err = m.connect().unwrap_err();
    assert!(matches!(err, HidMaestroError::SidecarDied(_)));
    assert_eq!(m.consecutive_failures(), 1);
    assert!(!m.fallback_needed());
}

#[test]
fn silent_sidecar_hello_times_out() {
    let mut cfg = config(&[("HM_STUB_SILENT", "1")]);
    cfg.spawn_timeout = Duration::from_millis(200);
    let mut m = SidecarManager::new(cfg);
    let err = m.connect().unwrap_err();
    assert!(matches!(err, HidMaestroError::HelloTimeout));
    m.shutdown(); // kills the lingering silent stub
}

#[test]
fn bad_hello_is_bad_reply() {
    let mut m = manager(&[("HM_STUB_BAD_HELLO", "1")]);
    let err = m.connect().unwrap_err();
    assert!(matches!(err, HidMaestroError::BadReply(_)));
}

#[test]
fn bad_dll_hash_is_refused() {
    let mut m = manager(&[("HM_STUB_DLL_OK", "0")]);
    let err = m.connect().unwrap_err();
    assert!(matches!(err, HidMaestroError::DllHashMismatch { .. }));
}

#[test]
fn submit_round_trip_reports_latency() {
    let mut m = manager(&[]);
    m.connect().unwrap();
    m.create().unwrap();
    let us = m.submit(GamepadFrame::neutral()).unwrap();
    assert_eq!(us, 12.0);
    assert_eq!(m.submit_count(), 1);
    // The SDK hook sample (9 us) is preferred over the wall sample.
    assert_eq!(m.last_submit_latency_us(), Some(9.0));
    m.shutdown();
}

#[test]
fn submit_raw_ok_and_wrong_size_refused() {
    let mut m = manager(&[]);
    m.connect().unwrap();
    m.create().unwrap();
    let ok = m.submit_raw(&[0u8; CUSTOM_RAW_REPORT_SIZE]).unwrap();
    assert_eq!(ok, 12.0);
    let err = m.submit_raw(&[0u8; 26]).unwrap_err();
    assert!(matches!(err, HidMaestroError::UpdateFailed(_)));
    assert!(err.to_string().contains("15-byte"));
    m.shutdown();
}

#[test]
fn crash_mid_session_is_detected() {
    let mut m = manager(&[("HM_STUB_CRASH_AFTER_SUBMITS", "1")]);
    m.connect().unwrap();
    m.create().unwrap();
    m.submit(GamepadFrame::neutral()).unwrap();
    let err = m.submit(GamepadFrame::neutral()).unwrap_err();
    assert!(matches!(err, HidMaestroError::SidecarDied(_)));
    assert!(!m.is_running());
    assert_eq!(m.consecutive_failures(), 1);
    assert!(!m.fallback_needed());
}

#[test]
fn crash_twice_uses_the_one_retry_then_fallback() {
    // Every spawn crashes on its first submit: connect#1 dies (failure
    // 1), the retry connect#2 dies too (failure 2) → fallback latch.
    let mut m = manager(&[("HM_STUB_CRASH_AFTER_SUBMITS", "0")]);
    m.connect().unwrap();
    m.create().unwrap();
    assert!(m.submit(GamepadFrame::neutral()).is_err());
    assert!(!m.fallback_needed());
    m.connect().unwrap(); // the one retry
    m.create().unwrap();
    assert!(m.submit(GamepadFrame::neutral()).is_err());
    assert!(m.fallback_needed());
    let err = m.connect().unwrap_err();
    assert!(matches!(err, HidMaestroError::FallbackNeeded));
}

#[test]
fn reset_fallback_clears_latch() {
    let mut m = manager(&[("HM_STUB_CRASH_AFTER_SUBMITS", "0")]);
    m.connect().unwrap();
    m.create().unwrap();
    assert!(m.submit(GamepadFrame::neutral()).is_err());
    m.connect().unwrap();
    m.create().unwrap();
    assert!(m.submit(GamepadFrame::neutral()).is_err());
    assert!(m.fallback_needed());
    m.reset_fallback();
    assert!(!m.fallback_needed());
    assert_eq!(m.consecutive_failures(), 0);
    // A fresh attempt is allowed again: the spawn + hello succeed,
    // and the crash that follows is a first failure again — not a
    // FallbackNeeded.
    m.connect().expect("fresh attempt allowed after reset");
    m.create().unwrap();
    let err = m.submit(GamepadFrame::neutral()).unwrap_err();
    assert!(matches!(err, HidMaestroError::SidecarDied(_)));
    assert!(!m.fallback_needed());
}

#[test]
fn heartbeat_detects_dead_sidecar() {
    let mut cfg = config(&[("HM_STUB_EXIT_AFTER", "1")]);
    cfg.heartbeat_interval = Duration::from_millis(50);
    let mut m = SidecarManager::new(cfg);
    m.connect().unwrap();
    // The stub exits after its first command (the ping below): the
    // ping itself still succeeds — the death is only visible on the
    // NEXT heartbeat, once the interval elapses again.
    std::thread::sleep(Duration::from_millis(120));
    assert!(m.ensure_alive().is_ok(), "first heartbeat ping still lands");
    std::thread::sleep(Duration::from_millis(120));
    let err = m.ensure_alive().unwrap_err();
    assert!(matches!(err, HidMaestroError::SidecarDied(_)));
    assert!(!m.is_running());
}

#[test]
fn heartbeat_quiet_when_fresh() {
    let mut m = manager(&[]);
    m.connect().unwrap();
    // Last reply is fresh (60 s interval): no ping is sent.
    assert!(m.ensure_alive().is_ok());
    m.shutdown();
}

#[test]
fn shutdown_is_idempotent() {
    let mut m = manager(&[]);
    m.connect().unwrap();
    m.shutdown();
    m.shutdown();
    assert!(!m.is_running());
}

#[test]
fn request_without_connect_is_not_connected() {
    let mut m = manager(&[]);
    let err = m.request(&Command::Ping).unwrap_err();
    assert!(matches!(err, HidMaestroError::NotConnected));
}

#[test]
fn unknown_command_is_bad_reply() {
    let mut m = manager(&[]);
    m.connect().unwrap();
    let reply = m.request(&Command::DescribeProfile).unwrap();
    assert!(reply.ok);
    // Unknown commands come back ok:false; request_checked maps them.
    let err = m
        .request_checked(&Command::SubmitRaw {
            bytes: "zz".to_string(),
        })
        .unwrap_err();
    assert!(matches!(err, HidMaestroError::BadReply(_)));
    m.shutdown();
}

/// C2 contract test: the verbatim real-sidecar `create` reply
/// (PROTOCOL.md capabilities example) flows through `parse_reply` →
/// `manager.create()` and yields the custom-wheel caps — the exact
/// path a real sidecar's reply takes.
#[test]
fn create_verbatim_real_sidecar_caps_parse_to_custom_wheel() {
    let caps = r#"{"axes":5,"buttons":32,"has_hat":true,"input_report_size":15,"submit_state_max_button":25,"submit_state_button_mask":33554431}"#;
    let mut m = manager(&[("HM_STUB_CAPS_JSON", caps)]);
    m.connect().unwrap();
    let created = m.create().unwrap();
    assert_eq!(
        created.capabilities,
        rsh_output_hidmaestro::HidMaestroCapabilities::custom_wheel()
    );
    m.shutdown();
}

/// C2: a partial axis count derives partial caps (`axes == 1` is
/// steering only) — the manager no longer falls back to the full
/// custom wheel when the sidecar reports fewer axes.
#[test]
fn create_partial_axis_count_derives_partial_caps() {
    let caps = r#"{"axes":1,"buttons":0,"has_hat":false,"input_report_size":15,"submit_state_max_button":25,"submit_state_button_mask":33554431}"#;
    let mut m = manager(&[("HM_STUB_CAPS_JSON", caps)]);
    m.connect().unwrap();
    let created = m.create().unwrap();
    assert_eq!(
        created.capabilities,
        rsh_output_hidmaestro::HidMaestroCapabilities::new(
            true, false, false, false, false, 0, false
        )
    );
    m.shutdown();
}

/// C1: the stub rejects the old `report` field at the process level
/// — spawn it, feed the old wire line, read the refusal. A Rust
/// build that still sends `report` fails loudly here instead of
/// silently dropping raw reports against the real sidecar.
#[test]
fn submit_raw_old_report_field_is_rejected_by_stub() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command as Proc, Stdio};
    let mut child = Proc::new(stub_path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("stub spawns");
    let mut stdin = child.stdin.take().expect("piped stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
    let mut line = String::new();
    stdout.read_line(&mut line).expect("hello line");
    assert!(line.contains("rsh-hm-sidecar"), "hello: {line}");
    writeln!(stdin, r#"{{"cmd":"submit-raw","report":"00ff"}}"#).unwrap();
    stdin.flush().unwrap();
    line.clear();
    stdout.read_line(&mut line).expect("refusal line");
    assert!(
        line.contains(r#""ok":false"#) && line.contains("bad-command"),
        "old `report` field must be refused as bad-command, got: {line}"
    );
    drop(stdin);
    let _ = child.wait();
}

/// M8: a second `connect()` notices a sidecar that died since the
/// last call (EOF already queued) and respawns instead of reporting
/// the dead transport as connected.
#[test]
fn connect_after_sidecar_exit_respawns() {
    let mut m = manager(&[("HM_STUB_EXIT_AFTER", "1")]);
    m.connect().unwrap();
    // First command: the stub answers, then exits (EOF queued).
    let reply = m.request(&Command::Ping).unwrap();
    assert!(reply.ok);
    // Second connect: liveness drain sees the EOF and respawns a
    // fresh stub — this must succeed, not return the dead session.
    let info = m.connect().expect("respawn after sidecar exit");
    assert_eq!(info.sdk_version.as_deref(), Some(PINNED_VERSION));
    assert!(m.is_running());
    m.shutdown();
}

/// H4: shutdown never waits for the `exit` reply — a sidecar that
/// answers `exit` slowly (or never) must not stall the caller.
#[test]
fn shutdown_does_not_wait_for_slow_exit_reply() {
    let mut cfg = config(&[("HM_STUB_SLOW_MS", "300")]);
    cfg.reply_timeout = Duration::from_millis(50);
    let mut m = SidecarManager::new(cfg);
    m.connect().unwrap();
    let start = std::time::Instant::now();
    m.shutdown();
    assert!(
        start.elapsed() < Duration::from_millis(250),
        "shutdown took {:?} — it waited for the exit reply",
        start.elapsed()
    );
    assert!(!m.is_running());
}

#[test]
fn slow_sidecar_reply_times_out() {
    let mut cfg = config(&[("HM_STUB_SLOW_MS", "300")]);
    cfg.reply_timeout = Duration::from_millis(50);
    let mut m = SidecarManager::new(cfg);
    m.connect().unwrap();
    let err = m.request(&Command::Ping).unwrap_err();
    assert!(matches!(err, HidMaestroError::ReplyTimeout));
    assert!(!m.is_running());
}

#[test]
fn stderr_ring_is_available() {
    let m = manager(&[]);
    assert!(m.last_stderr().is_empty(), "stub writes no stderr");
}

/// Smoke test: the Rust manager against the REAL production sidecar
/// (built as a plain .NET app, run via `dotnet <dll>` over stdio).
/// Runs only when `HM_REAL_SIDECAR_DLL` points at the built dll;
/// otherwise it skips (the DLL is never committed, and the real
/// sidecar + driver are Windows-only in production).
///
/// Run it with:
///   HM_REAL_SIDECAR_DLL=tools/rsh-hm-sidecar/bin/Release/net10.0-windows/rsh-hm-sidecar.dll \
///     cargo test -p `rsh-output-hidmaestro` `real_sidecar_stdio_smoke`
#[test]
fn real_sidecar_stdio_smoke() {
    let Ok(dll) = std::env::var("HM_REAL_SIDECAR_DLL") else {
        return;
    };
    let dll = PathBuf::from(dll);
    if !dll.exists() {
        return;
    }
    let dotnet = std::env::var("HM_DOTNET").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_default();
        format!("{home}/.dotnet/dotnet")
    });
    let mut cfg = SidecarConfig::with_path(PathBuf::from(dotnet));
    cfg.args = vec![dll.to_string_lossy().into_owned()];
    cfg.elevated = false;
    cfg.spawn_timeout = Duration::from_secs(15);
    cfg.reply_timeout = Duration::from_secs(5);
    cfg.heartbeat_interval = Duration::from_secs(60);
    let mut m = SidecarManager::new(cfg);
    let hello: HelloInfo = m.connect().expect("real sidecar hello");
    assert_eq!(hello.sdk_version.as_deref(), Some(PINNED_VERSION));
    let version = m.request(&Command::Version).expect("real sidecar version");
    assert_eq!(version.dll_sha256.as_deref(), Some(PINNED_DLL_SHA256));
    assert_eq!(version.dll_sha256_ok, Some(true));
    let reply = m
        .request(&Command::DescribeProfile)
        .expect("real sidecar describe-profile");
    assert!(reply.ok);
    assert_eq!(reply.input_report_size, Some(CUSTOM_RAW_REPORT_SIZE));
    assert_eq!(reply.descriptor_len, Some(124));
    assert_eq!(reply.vid, Some(u32::from(PROFILE_VID)));
    assert_eq!(reply.pid_value, Some(u32::from(PROFILE_PID)));
    // The driver is not installed on Linux: install must refuse
    // with not-admin rather than crash the sidecar.
    let err = m.install_driver().unwrap_err();
    assert!(matches!(err, HidMaestroError::NotAdmin));
    m.shutdown();
}
