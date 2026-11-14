//! Backend tests against the stub sidecar (Linux).
//!
//! These are the `IJoystickOutput`-level cases of the 78-test
//! equivalent suite: the trait as defined in `rsh-output` (Result-based
//! `try_connect` / `update` / `send_neutral` / `disconnect`),
//! plan validation (D11), crash → disconnect → one retry → fallback,
//! and the G4 selector hook.

use std::path::PathBuf;
use std::time::Duration;

use rsh_engine::pipeline::{OutputState, PipelinePlan};
use rsh_output::vjoy::VJoyError;
use rsh_output::{IJoystickOutput, ProcessedState};
use rsh_output_hidmaestro::{
    HidMaestroConfig, HidMaestroError, HidMaestroOutput, HidMaestroStateKind,
};

fn stub_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_hm-stub-sidecar"))
}

fn config(envs: &[(&str, &str)]) -> HidMaestroConfig {
    let mut cfg = HidMaestroConfig::with_sidecar_path(stub_path());
    cfg.sidecar.spawn_timeout = Duration::from_secs(5);
    cfg.sidecar.reply_timeout = Duration::from_secs(2);
    cfg.sidecar.heartbeat_interval = Duration::from_secs(60);
    cfg.sidecar.extra_env = envs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    cfg
}

fn backend(envs: &[(&str, &str)]) -> HidMaestroOutput {
    HidMaestroOutput::with_config(config(envs))
}

fn full_state() -> OutputState {
    let mut s = OutputState::neutral();
    s.steering = 0.5;
    s.accelerator = 0.8;
    s.brake = 0.2;
    s.clutch = 0.1;
    s.handbrake = 0.0;
    s.buttons = 0b101;
    s
}

#[test]
fn try_connect_happy_path() {
    let mut b = backend(&[]);
    assert_eq!(b.name(), "HIDMaestro");
    assert!(!b.is_connected());
    b.try_connect().expect("connect");
    assert!(b.is_connected());
    let status = b.status();
    assert_eq!(status.kind, HidMaestroStateKind::Ready);
    assert_eq!(status.driver_version.as_deref(), Some("2.1.0"));
    assert_eq!(status.transport_index, Some(0));
    assert!(status.message.contains("rsh-wheel-custom"));
    assert_eq!(
        b.capabilities(),
        rsh_output_hidmaestro::HidMaestroCapabilities::custom_wheel()
    );
    b.disconnect();
}

#[test]
fn try_connect_twice_is_noop() {
    let mut b = backend(&[]);
    b.try_connect().unwrap();
    b.try_connect().unwrap();
    assert!(b.is_connected());
    b.disconnect();
}

#[test]
fn update_submits_frame() {
    let mut b = backend(&[]);
    b.try_connect().unwrap();
    b.update(&full_state()).expect("update");
    let status = b.status();
    // Neutral at connect + this update = 2 submits.
    assert_eq!(status.submit_count, 2);
    assert_eq!(status.last_submit_latency_us, Some(9.0));
    b.disconnect();
}

#[test]
fn update_full_deflection_ok() {
    let mut b = backend(&[]);
    b.try_connect().unwrap();
    let mut s = OutputState::neutral();
    s.steering = 1.0;
    s.accelerator = 1.0;
    s.brake = 1.0;
    s.clutch = 1.0;
    s.handbrake = 1.0;
    s.buttons = u32::MAX;
    b.update(&s).unwrap();
    b.disconnect();
}

#[test]
fn update_not_connected_errors() {
    let mut b = backend(&[]);
    let err = b.update(&OutputState::neutral()).unwrap_err();
    assert_eq!(err, VJoyError::NotConnected);
}

#[test]
fn send_neutral_not_connected_is_noop() {
    let mut b = backend(&[]);
    assert!(b.send_neutral().is_ok());
}

#[test]
fn send_neutral_connected_ok() {
    let mut b = backend(&[]);
    b.try_connect().unwrap();
    assert!(b.send_neutral().is_ok());
    b.disconnect();
}

#[test]
fn disconnect_is_idempotent_and_allows_reconnect() {
    let mut b = backend(&[]);
    b.try_connect().unwrap();
    b.disconnect();
    b.disconnect();
    assert!(!b.is_connected());
    assert_eq!(b.status().kind, HidMaestroStateKind::Idle);
    b.try_connect().expect("reconnect after disconnect");
    assert!(b.is_connected());
    b.disconnect();
}

#[test]
fn try_connect_not_admin_reports_uac() {
    let mut b = backend(&[("HM_STUB_ADMIN", "0")]);
    let err = b.try_connect().unwrap_err();
    assert!(matches!(err, VJoyError::DriverNotFound(_)));
    assert_eq!(b.status().kind, HidMaestroStateKind::NotAdmin);
    assert!(b.status().message.contains("administrator"));
    assert!(b.status().uac_note.contains("UAC"));
    assert!(b.status().elevation_required);
    assert!(matches!(b.last_error(), Some(HidMaestroError::NotAdmin)));
}

#[test]
fn try_connect_sidecar_missing() {
    let mut cfg = HidMaestroConfig::with_sidecar_path(PathBuf::from("/nonexistent/rsh-hm-sidecar"));
    cfg.sidecar.spawn_timeout = Duration::from_secs(2);
    let mut b = HidMaestroOutput::with_config(cfg);
    let err = b.try_connect().unwrap_err();
    assert!(matches!(err, VJoyError::DriverNotFound(_)));
    assert_eq!(b.status().kind, HidMaestroStateKind::SidecarMissing);
    assert!(b.status().message.contains("not found"));
}

#[test]
fn try_connect_create_failed_then_fallback() {
    let mut b = backend(&[("HM_STUB_FAIL_CREATE", "1")]);
    let err = b.try_connect().unwrap_err();
    assert!(matches!(err, VJoyError::DriverNotFound(_)));
    assert_eq!(b.status().kind, HidMaestroStateKind::CreateFailed);
    assert!(matches!(
        b.last_error(),
        Some(HidMaestroError::CreateFailed(_))
    ));
    assert!(
        !b.fallback_needed(),
        "reply errors do not consume the retry"
    );
    // The retry: same failure again — still a reply error, still no
    // fallback latch (transport never died).
    let _ = b.try_connect();
    assert!(!b.fallback_needed());
}

#[test]
fn try_connect_with_plan_steering_only_minimal_caps() {
    // C2: real sidecar shape — `axes == 1` is steering (X) only.
    let caps = r#"{"axes":1,"buttons":0,"has_hat":false,"input_report_size":15,"submit_state_max_button":25,"submit_state_button_mask":33554431}"#;
    let mut b = backend(&[("HM_STUB_CAPS_JSON", caps)]);
    let mut plan = PipelinePlan::default();
    plan.accelerator.enabled = false;
    plan.brake.enabled = false;
    plan.clutch.enabled = false;
    plan.handbrake.enabled = false;
    plan.buttons.enabled = false;
    b.try_connect_with_plan(&plan).expect("steering-only plan");
    assert!(b.is_connected());
    b.disconnect();
}

#[test]
fn try_connect_with_plan_missing_x_fails_and_relinquishes() {
    // C2: real sidecar shape — `axes == 0` means no steering axis at
    // all (a count cannot express "every axis except X", and the real
    // sidecar never sends that anyway: it is 5 or nothing).
    let caps = r#"{"axes":0,"buttons":32,"has_hat":true,"input_report_size":15,"submit_state_max_button":25,"submit_state_button_mask":33554431}"#;
    let mut b = backend(&[("HM_STUB_CAPS_JSON", caps)]);
    let err = b
        .try_connect_with_plan(&PipelinePlan::default())
        .unwrap_err();
    assert!(matches!(err, VJoyError::DriverNotFound(_)));
    assert!(!b.is_connected());
    assert_eq!(b.status().kind, HidMaestroStateKind::CreateFailed);
    assert!(b.status().message.contains("Steering"));
}

#[test]
fn raw_report_path_is_the_default() {
    // M3: raw reports ship by default, so buttons 26–32 + encoder
    // pulses reach the game without any setting change.
    let b = HidMaestroOutput::new();
    assert!(b.use_raw_report());
    assert!(HidMaestroConfig::default().use_raw_report);
    let b2 = backend(&[]);
    assert!(b2.use_raw_report());
}

#[test]
fn raw_report_path_submits_15_bytes() {
    let mut cfg = config(&[]);
    cfg.use_raw_report = true;
    let mut b = HidMaestroOutput::with_config(cfg);
    assert!(b.use_raw_report());
    b.try_connect().unwrap();
    b.update(&full_state()).unwrap();
    assert_eq!(b.status().submit_count, 2, "neutral + one update");
    b.disconnect();
}

#[test]
fn submit_state_path_still_available_when_opted_out() {
    // Opting out of raw keeps the `SubmitState` path working (the
    // neutral goes through the sidecar's `neutral` command either way).
    let mut cfg = config(&[]);
    cfg.use_raw_report = false;
    let mut b = HidMaestroOutput::with_config(cfg);
    assert!(!b.use_raw_report());
    b.try_connect().unwrap();
    b.update(&full_state()).unwrap();
    assert_eq!(b.status().submit_count, 2, "neutral + one update");
    b.disconnect();
}

#[test]
fn crash_mid_session_marks_disconnected_then_fallback() {
    let mut b = backend(&[("HM_STUB_CRASH_AFTER_SUBMITS", "1")]);
    b.try_connect().unwrap(); // neutral (Neutral cmd) lands
    let err = b.update(&full_state()).unwrap_err(); // submit #1 crashes
    assert!(matches!(err, VJoyError::DriverNotFound(_)));
    assert!(!b.is_connected());
    assert_eq!(b.status().kind, HidMaestroStateKind::SidecarFailed);
    assert!(!b.fallback_needed(), "first death: one retry remains");
    // The one retry: reconnect, then the next update dies again.
    b.try_connect().expect("retry connect");
    assert!(b.is_connected());
    assert!(b.update(&full_state()).is_err());
    assert!(!b.is_connected());
    assert!(b.fallback_needed(), "second death: fallback latch set");
    let err = b.try_connect().unwrap_err();
    assert!(matches!(err, VJoyError::DriverNotFound(_)));
    assert_eq!(b.status().kind, HidMaestroStateKind::FallbackToVJoy);
    // The user re-selects the backend: the latch clears.
    b.reset_fallback();
    assert!(!b.fallback_needed());
    assert_eq!(b.status().kind, HidMaestroStateKind::Idle);
}

#[test]
fn update_after_crash_is_not_connected() {
    let mut b = backend(&[("HM_STUB_CRASH_AFTER_SUBMITS", "1")]);
    b.try_connect().unwrap();
    assert!(b.update(&full_state()).is_err());
    let err = b.update(&full_state()).unwrap_err();
    assert_eq!(err, VJoyError::NotConnected);
}

#[test]
fn backend_info_selector_hook() {
    let info = HidMaestroOutput::backend_info();
    assert_eq!(info.name, "HIDMaestro");
    assert!(info.is_default);
    assert!(info.requires_elevation);
    assert_eq!(info.fallback_backend, "vJoy");
    assert_eq!(info.sdk_version, "2.1.0");
    assert_eq!(info.profile_id, "rsh-wheel-custom");
    assert_eq!(info.identity_key, "rsh-wheel");
    assert!(info.uac_note.contains("UAC"));
}

#[test]
fn status_carries_uac_note_and_elevation_flag() {
    let b = backend(&[]);
    let status = b.status();
    assert!(status.elevation_required);
    assert!(status.uac_note.contains("every launch"));
    assert!(status.uac_note.contains("vJoy needs no prompt"));
}

#[test]
fn trait_error_preserves_hidmaestro_message() {
    let mut cfg = HidMaestroConfig::with_sidecar_path(PathBuf::from("/nonexistent/rsh-hm-sidecar"));
    cfg.sidecar.spawn_timeout = Duration::from_secs(2);
    let mut b = HidMaestroOutput::with_config(cfg);
    let err = b.try_connect().unwrap_err();
    match err {
        VJoyError::DriverNotFound(msg) => {
            assert!(
                msg.contains("rsh-hm-sidecar"),
                "message names the sidecar: {msg}"
            );
        }
        other => panic!("expected DriverNotFound, got {other:?}"),
    }
}

#[test]
fn processed_state_alias_is_output_state() {
    fn takes_processed(_: &ProcessedState) {}
    let state = OutputState::neutral();
    takes_processed(&state);
    let mut b = backend(&[]);
    b.try_connect().unwrap();
    b.update(&state).unwrap();
    b.disconnect();
}
