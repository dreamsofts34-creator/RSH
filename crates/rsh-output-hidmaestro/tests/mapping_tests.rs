//! Mapping + parity integration tests.
//!
//! The vJoy-parity cases of the 78-test equivalent suite: the
//! `HIDMaestro` floats and the vJoy axis ints are two views of the same
//! `OutputState` (both route through the C# `InputMapper` math), and
//! the known gaps (`SubmitState` 25 buttons, raw 32 buttons, 33+ no wire
//! slot on the custom profile) are asserted, not hidden.

use rsh_engine::pipeline::OutputState;
use rsh_output::vjoy::{build_position, neutral_position, pedal_to_axis, steering_to_axis};
use rsh_output_hidmaestro::mapping::{
    CUSTOM_RAW_REPORT_SIZE, HAT_NEUTRAL_RAW, SUBMIT_STATE_BUTTON_MASK, SUBMIT_STATE_MAX_BUTTON,
    build_gamepad_frame, build_raw_report, is_raw_button_pressed, neutral_gamepad_frame,
    neutral_raw_report, pedal_to_float, read_raw_axis, steering_to_float,
};
use rsh_output_hidmaestro::{
    HidMaestroCapabilities, PINNED_DLL_SHA256, PINNED_VERSION, PINNED_ZIP_SHA256, PROFILE_PID,
    PROFILE_VID,
};

fn state(
    steering: f32,
    accel: f32,
    brake: f32,
    clutch: f32,
    handbrake: f32,
    buttons: u32,
) -> OutputState {
    let mut s = OutputState::neutral();
    s.steering = steering;
    s.accelerator = accel;
    s.brake = brake;
    s.clutch = clutch;
    s.handbrake = handbrake;
    s.buttons = buttons;
    s
}

#[test]
fn csharp_vectors_steering() {
    // C# ToUnitSigned: 0.5 + (v-16384)/32768, v = InputMapper axis.
    assert!((steering_to_float(-1.0) - 0.000_030_5).abs() < 0.000_001);
    assert_eq!(steering_to_float(0.0), 0.5);
    assert_eq!(steering_to_float(1.0), 1.0);
}

#[test]
fn csharp_vectors_pedals() {
    // C# ToUnit: (v-1)/32767 — both ends exact.
    assert_eq!(pedal_to_float(0.0), 0.0);
    assert_eq!(pedal_to_float(1.0), 1.0);
    assert!((pedal_to_float(0.5) - 0.5).abs() < 0.000_1);
}

#[test]
#[allow(clippy::cast_precision_loss)]
fn axes_parity_vjoy_vs_hidmaestro() {
    let caps = HidMaestroCapabilities::custom_wheel();
    let vjoy_caps = rsh_output::vjoy::VJoyCapabilities::unknown();
    for (steering, accel, brake, clutch, handbrake) in [
        (-1.0, 0.0, 0.0, 0.0, 0.0),
        (0.0, 0.0, 0.0, 0.0, 0.0),
        (1.0, 1.0, 1.0, 1.0, 1.0),
        (0.25, 0.5, 0.75, 0.1, 0.9),
        (-0.5, 1.0, 0.0, 0.5, 0.25),
    ] {
        let s = state(steering, accel, brake, clutch, handbrake, 0);
        let pos = build_position(&s, &vjoy_caps, 1);
        let frame = build_gamepad_frame(&s, &caps);
        assert_eq!(pos.w_axis_x, steering_to_axis(steering));
        assert_eq!(pos.w_axis_y, pedal_to_axis(accel));
        assert_eq!(pos.w_axis_z, pedal_to_axis(brake));
        assert_eq!(pos.w_clutch, pedal_to_axis(clutch));
        assert_eq!(pos.w_slider, pedal_to_axis(handbrake));
        assert_eq!(frame.x, steering_to_float(steering));
        assert_eq!(frame.y, pedal_to_float(accel));
        assert_eq!(frame.z, pedal_to_float(brake));
        assert_eq!(frame.rx, pedal_to_float(clutch));
        assert_eq!(frame.slider, pedal_to_float(handbrake));
        // The two views agree through the shared axis ints.
        assert_eq!(frame.x, 0.5 + (pos.w_axis_x as f32 - 16384.0) / 32768.0);
        assert_eq!(frame.y, (pos.w_axis_y as f32 - 1.0) / 32767.0);
    }
}

#[test]
fn neutral_parity_both_backends() {
    let vjoy = neutral_position(1);
    let hm = neutral_gamepad_frame(&HidMaestroCapabilities::custom_wheel());
    assert_eq!(vjoy.w_axis_x, 16384);
    assert_eq!(hm.x, 0.5);
    assert_eq!(vjoy.w_axis_y, 1);
    assert_eq!(hm.y, 0.0);
    assert_eq!(vjoy.w_axis_z, 1);
    assert_eq!(hm.z, 0.0);
    assert_eq!(vjoy.w_slider, 1);
    assert_eq!(hm.slider, 0.0);
    assert_eq!(vjoy.l_buttons, 0);
    assert_eq!(hm.buttons, 0);
    let mut raw = [0u8; CUSTOM_RAW_REPORT_SIZE];
    neutral_raw_report(&HidMaestroCapabilities::custom_wheel(), &mut raw);
    assert_eq!(read_raw_axis(&raw, 0), Some(16383));
    assert_eq!(raw[14], HAT_NEUTRAL_RAW);
}

#[test]
fn buttons_1_to_25_parity_both_backends() {
    let caps = HidMaestroCapabilities::custom_wheel();
    let vjoy_caps = rsh_output::vjoy::VJoyCapabilities::unknown();
    for button in 1..=25u8 {
        let s = state(0.0, 0.0, 0.0, 0.0, 0.0, 1u32 << (u32::from(button) - 1));
        let frame = build_gamepad_frame(&s, &caps);
        assert_eq!(frame.buttons, 1u32 << (u32::from(button) - 1));
        let pos = build_position(&s, &vjoy_caps, 1);
        assert_eq!(pos.l_buttons, 1i32 << (u32::from(button) - 1));
    }
}

#[test]
fn buttons_26_to_32_known_gap_submit_state_drops_raw_keeps() {
    let caps = HidMaestroCapabilities::custom_wheel();
    for button in 26..=32u8 {
        let s = state(0.0, 0.0, 0.0, 0.0, 0.0, 1u32 << (u32::from(button) - 1));
        let frame = build_gamepad_frame(&s, &caps);
        assert_eq!(
            frame.buttons, 0,
            "button {button} must not leak into HMButton"
        );
        let mut raw = [0u8; CUSTOM_RAW_REPORT_SIZE];
        build_raw_report(&s, &caps, &mut raw);
        assert!(
            is_raw_button_pressed(&raw, button),
            "button {button} kept in raw"
        );
    }
}

#[test]
fn buttons_ex_and_encoder_pulses_have_no_hidmaestro_wire_slot() {
    let caps = HidMaestroCapabilities::custom_wheel();
    let vjoy_caps = rsh_output::vjoy::VJoyCapabilities::unknown();
    let mut s = state(0.0, 0.0, 0.0, 0.0, 0.0, u32::MAX);
    s.buttons_ex = u32::MAX;
    let frame = build_gamepad_frame(&s, &caps);
    assert_eq!(frame.buttons, SUBMIT_STATE_BUTTON_MASK);
    let mut raw = [0u8; CUSTOM_RAW_REPORT_SIZE];
    build_raw_report(&s, &caps, &mut raw);
    assert_eq!(raw[10..14], [0xFF, 0xFF, 0xFF, 0xFF]);
    // vJoy DOES carry them (Ex1/Ex2) — the gap is HIDMaestro-profile
    // shaped, not a mapping bug.
    let pos = build_position(&s, &vjoy_caps, 1);
    assert_ne!(pos.l_buttons_ex1, 0);
}

#[test]
fn submit_state_button_limit_is_25() {
    assert_eq!(SUBMIT_STATE_MAX_BUTTON, 25);
    assert_eq!(SUBMIT_STATE_BUTTON_MASK, 0x01FF_FFFF);
}

#[test]
fn raw_report_layout_is_15_bytes_from_real_descriptor() {
    // 5x16-bit axes + 32x1-bit buttons + 1x8-bit hat = 120 bits.
    assert_eq!(CUSTOM_RAW_REPORT_SIZE, 15);
    assert_eq!(5 * 16 + 32 + 8, CUSTOM_RAW_REPORT_SIZE * 8);
}

#[test]
fn raw_report_round_trip_axes() {
    let caps = HidMaestroCapabilities::custom_wheel();
    for (steering, accel, brake) in [(-1.0, 0.0, 0.0), (0.0, 0.5, 0.5), (1.0, 1.0, 1.0)] {
        let s = state(steering, accel, brake, 0.0, 0.0, 0);
        let mut raw = [0u8; CUSTOM_RAW_REPORT_SIZE];
        build_raw_report(&s, &caps, &mut raw);
        assert_eq!(
            read_raw_axis(&raw, 0),
            Some(u16::try_from(steering_to_axis(steering) - 1).unwrap_or(0))
        );
        assert_eq!(
            read_raw_axis(&raw, 1),
            Some(u16::try_from(pedal_to_axis(accel) - 1).unwrap_or(0))
        );
    }
}

#[test]
fn vid_pid_are_not_the_spike_placeholder() {
    assert_eq!(PROFILE_VID, 0x1209);
    assert_eq!(PROFILE_PID, 0x5253);
    assert_ne!(PROFILE_PID, 0x0001, "0x0001 is pid.codes' own Test PID");
}

#[test]
fn pinned_hashes_are_well_formed_and_match_the_spike() {
    assert_eq!(PINNED_VERSION, "2.1.0");
    for hash in [PINNED_ZIP_SHA256, PINNED_DLL_SHA256] {
        assert_eq!(hash.len(), 64);
        assert!(hash.bytes().all(|b| b.is_ascii_hexdigit()));
    }
    assert_eq!(
        PINNED_ZIP_SHA256,
        "7fe349f163864b8dfc36117af260d51bac94a2263b6cdfa719cb364a808397b4"
    );
    assert_eq!(
        PINNED_DLL_SHA256,
        "f13e32591404db0f418af9265b8e240f71257297cfae5a41dcd696fd87e9b4d5"
    );
}
