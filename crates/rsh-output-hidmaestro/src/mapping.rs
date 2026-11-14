//! `ProcessedState` → `HIDMaestro` frame mapping (pure — no driver).
//!
//! Two submit paths, both pure (mirroring the C#
//! `HidMaestroMapping`, re-expressed for the Rust
//! [`OutputState`](rsh_engine::pipeline::OutputState), whose axes are
//! already floats):
//!
//! 1. **`SubmitState` path** (opt-out): the SDK's documented abstract frame
//!    (`HMGamepadState`). Axes are `[0, 1]` floats — `0.5` = centre on
//!    the signed steering axis, `0.0` = released on unsigned pedals /
//!    slider. Buttons: `HMButton` is 25 named bits (measured by
//!    reflection on the real v2.1.0 DLL: highest bit
//!    `RightTriggerClick = 0x1000000`), so wheel buttons **1–25** travel
//!    this way; buttons 26–32, `buttons_ex` (33–64) and encoder pulses
//!    (65–72) do NOT — documented gap, same as the C# series.
//! 2. **Raw-report path** (default, `use_raw_report`): the custom
//!    `rsh-wheel-custom` profile's real input report, derived from its
//!    124-byte descriptor (measured in this task — see below). It carries
//!    all 32 wheel buttons. `buttons_ex` and encoder pulses still have
//!    no wire slot on this profile (it has 32 buttons, not 128).
//!
//! # The 26-byte assumption is dead (R-9)
//!
//! The C# series built a 26-byte raw report (`5 × u16` axes + 128 button
//! bits) for a 128-button profile that was never built. The real custom
//! profile descriptor (124 B, measured) is:
//! 5 × 16-bit axes + 32 × 1-bit buttons + 1 × 8-bit hat = 120 bits =
//! **15 bytes**. There is no 26-byte report for this profile, and this
//! crate deliberately does **not** implement the legacy 26-byte layout:
//! sending 26 bytes to a 15-byte report would corrupt the frame.
//! The sidecar derives the size live from the descriptor
//! (`describe-profile` command) and the raw path refuses any report
//! whose length is not the derived size.
//!
//! # Axis conversion (vJoy parity)
//!
//! Both backends route through the same vJoy-unit axis values
//! ([`rsh_output::vjoy::steering_to_axis`] / `pedal_to_axis`, which
//! carry the C# `InputMapper` banker's rounding and clamping), so a
//! `HIDMaestro` float and the matching vJoy axis always agree:
//! * steering `s ∈ [-1, 1]` → `0.5 + (v - 16384) / 32768` where `v` is
//!   the vJoy axis value — centre exactly `0.5`, full right exactly
//!   `1.0`, full left `0.00003` (1 LSB above 0 — the C# B-H1 finding:
//!   a straight `(v - 1) / 32767` would put centre at `0.49998`).
//! * pedal `p ∈ [0, 1]` → `(v - 1) / 32767` — released exactly `0.0`,
//!   fully pressed exactly `1.0`.
//! * raw-report u16: `v - 1` (`0..=32767`), centre `16383`.
//!
//! Non-finite inputs (NaN/inf — the pipeline guarantees finite, this is
//! defence in depth) map to the safe value (centre for steering,
//! released for pedals), never to a garbage axis.

use rsh_engine::pipeline::{OutputState, PipelinePlan};
use rsh_output::vjoy::{pedal_to_axis, steering_to_axis};

use crate::HidMaestroError;

/// `HMButton` carries 25 named bits (measured on the real DLL:
/// highest bit `RightTriggerClick = 0x1000000`).
pub const SUBMIT_STATE_MAX_BUTTON: u8 = 25;
/// Mask for the 25 `HMButton` bits (wheel buttons 1–25).
pub const SUBMIT_STATE_BUTTON_MASK: u32 = 0x01FF_FFFF;

/// Real input-report size of the `rsh-wheel-custom` profile, derived
/// from its 124-byte descriptor: 5 × 16-bit axes (80 bits) + 32 × 1-bit
/// buttons (32 bits) + 1 × 8-bit hat (8 bits) = 120 bits = 15 bytes.
/// The sidecar re-derives this live (`describe-profile`); this constant
/// is the Rust side's expectation, asserted by tests.
pub const CUSTOM_RAW_REPORT_SIZE: usize = 15;

/// Neutral value of the raw report's hat byte.
///
/// The descriptor's hat field is 8 bits with the null-state flag and a
/// logical range of 0–3 (4 directions); `0x08` is the first
/// out-of-range value — the conventional "centred" encoding for a
/// null-state hat. RSH has no hat input, so this byte is always
/// neutral. **Assumption — verify on Windows (H-HID-2):** if games
/// show a stuck hat direction, the neutral value is the first suspect.
pub const HAT_NEUTRAL_RAW: u8 = 0x08;

/// A device's capabilities, read from the created profile (connect
/// time). Mirrors the C# `HidMaestroCapabilities`.
// Six boolean flags are the honest shape of a capability set (the
// sibling vJoy backend does the same); a bitflags type would hide
// which axis is missing in diagnostics.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HidMaestroCapabilities {
    /// X axis present (steering).
    pub has_axis_x: bool,
    /// Y axis present (accelerator).
    pub has_axis_y: bool,
    /// Z axis present (brake).
    pub has_axis_z: bool,
    /// Rx axis present (clutch).
    pub has_axis_rx: bool,
    /// Slider axis present (handbrake, HID usage `0x36`).
    pub has_axis_slider: bool,
    /// Button count as the profile reports it (0–128).
    pub button_count: i32,
    /// Hat present.
    pub has_hat: bool,
}

impl HidMaestroCapabilities {
    /// Capabilities of the `rsh-wheel-custom` profile: 5 axes, 32
    /// buttons, hat.
    #[must_use]
    pub const fn custom_wheel() -> Self {
        Self {
            has_axis_x: true,
            has_axis_y: true,
            has_axis_z: true,
            has_axis_rx: true,
            has_axis_slider: true,
            button_count: 32,
            has_hat: true,
        }
    }

    /// Capabilities for a known device.
    // Seven parameters because a capability set is seven facts; the
    // sibling vJoy backend allows the same. `has_axis_x` /
    // `has_axis_rx` are genuinely similar names for different axes.
    #[allow(clippy::fn_params_excessive_bools)]
    #[allow(clippy::similar_names)]
    #[must_use]
    pub const fn new(
        has_axis_x: bool,
        has_axis_y: bool,
        has_axis_z: bool,
        has_axis_rx: bool,
        has_axis_slider: bool,
        button_count: i32,
        has_hat: bool,
    ) -> Self {
        Self {
            has_axis_x,
            has_axis_y,
            has_axis_z,
            has_axis_rx,
            has_axis_slider,
            button_count,
            has_hat,
        }
    }

    /// Caps not read yet: assume a full device (no gating). Same
    /// convention as the C# series and the vJoy backend.
    #[must_use]
    pub const fn unknown() -> Self {
        Self::new(true, true, true, true, true, 128, false)
    }

    /// Mask for wheel buttons 1–32.
    #[must_use]
    pub fn button_mask(&self) -> u32 {
        block_mask(self.button_count, 1)
    }

    /// Mask for wheel buttons 33–64 (no wire slot on the 32-button
    /// custom profile — always 0 there).
    #[must_use]
    pub fn ex1_mask(&self) -> u32 {
        block_mask(self.button_count, 33)
    }

    /// Mask for wheel buttons 65–96 (no wire slot on the 32-button
    /// custom profile — always 0 there).
    #[must_use]
    pub fn ex2_mask(&self) -> u32 {
        block_mask(self.button_count, 65)
    }
}

/// Bitmask for the 32-button block starting at 1-based `first_button`
/// (1, 33 or 65): only the buttons the device actually has pass.
const fn block_mask(button_count: i32, first_button: i32) -> u32 {
    let available = button_count - first_button + 1;
    if available <= 0 {
        0
    } else if available >= 32 {
        u32::MAX
    } else {
        // `available` is 1..=31 here, so the shift cannot overflow.
        (1u32 << available) - 1
    }
}

/// Validate a device's capabilities against the configured modules
/// (D11, C# `HidMaestroOutput.ValidateCapabilities`).
///
/// Only *enabled* modules impose requirements: a steering-only wheel
/// is valid with just X, a disabled module imposes nothing. Clutch
/// imposes no extra requirement (its `Rx` value is always written;
/// the SDK ignores it when the axis is absent — vJoy `wClutch` parity).
/// Returns the list of problems; empty means valid.
#[must_use]
pub fn validate_capabilities(plan: &PipelinePlan, caps: &HidMaestroCapabilities) -> Vec<String> {
    let mut errors = Vec::new();
    if !(0..=128).contains(&caps.button_count) {
        errors.push(format!(
            "ButtonCount {} is out of range (0..=128).",
            caps.button_count
        ));
    }
    if plan.steering.enabled && !caps.has_axis_x {
        errors.push("Steering module is on but the HIDMaestro profile has no X axis.".to_string());
    }
    if plan.accelerator.enabled && !caps.has_axis_y {
        errors
            .push("Accelerator module is on but the HIDMaestro profile has no Y axis.".to_string());
    }
    if plan.brake.enabled && !caps.has_axis_z {
        errors.push("Brake module is on but the HIDMaestro profile has no Z axis.".to_string());
    }
    if plan.handbrake.enabled && !caps.has_axis_slider {
        errors.push(
            "Handbrake module is on but the HIDMaestro profile has no Slider axis.".to_string(),
        );
    }
    if plan.buttons.enabled && caps.button_count < 1 {
        errors.push("Buttons module is on but the HIDMaestro profile has no buttons.".to_string());
    }
    errors
}

/// Steering `-1..=1` → `SubmitState` float `0..=1` (`-1` = full left ≈ 0,
/// `0` = centre = `0.5`, `+1` = full right = `1.0`).
///
/// Routes through the vJoy axis value so both backends agree exactly
/// (see module docs). Non-finite input maps to centre.
#[must_use]
pub fn steering_to_float(value: f32) -> f32 {
    let axis = steering_to_axis(value);
    // `axis` is 1..=32768; the cast to f32 is exact in that range.
    #[allow(clippy::cast_precision_loss)]
    let axis_f = axis as f32;
    (0.5 + (axis_f - 16384.0) / 32768.0).clamp(0.0, 1.0)
}

/// Pedal / slider `0..=1` → `SubmitState` float `0..=1` (`0` = released =
/// `0.0`, `1` = fully pressed = `1.0`).
///
/// Routes through the vJoy axis value so both backends agree exactly.
/// Non-finite input maps to released.
#[must_use]
pub fn pedal_to_float(value: f32) -> f32 {
    let axis = pedal_to_axis(value);
    // `axis` is 1..=32768; the cast to f32 is exact in that range.
    #[allow(clippy::cast_precision_loss)]
    let axis_f = axis as f32;
    ((axis_f - 1.0) / 32767.0).clamp(0.0, 1.0)
}

/// Raw-report u16 for a vJoy-unit axis value: `value - 1` (`0..=32767`).
/// Centre is `16383`.
#[must_use]
pub fn axis_to_u16(axis_value: i32) -> u16 {
    let v = axis_value.clamp(1, 32768) - 1;
    // `v` is 0..=32767 after clamping, so neither the truncation nor
    // the sign conversion can lose anything.
    #[allow(clippy::cast_possible_truncation)]
    #[allow(clippy::cast_sign_loss)]
    {
        v as u16
    }
}

/// One `SubmitState` frame: five `[0, 1]` axis floats plus the 25
/// `HMButton` bits (wheel buttons 1–25).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GamepadFrame {
    /// X — steering (`0.5` = centre).
    pub x: f32,
    /// Y — accelerator (`0.0` = released).
    pub y: f32,
    /// Z — brake (`0.0` = released).
    pub z: f32,
    /// Rx — clutch (`0.0` = released).
    pub rx: f32,
    /// Slider — handbrake (`0.0` = released).
    pub slider: f32,
    /// Wheel buttons 1–25 (bit 0 = button 1). Higher bits are dropped
    /// on this path — see [`build_gamepad_frame`].
    pub buttons: u32,
}

impl GamepadFrame {
    /// The safe neutral frame: wheel straight, pedals released,
    /// handbrake released, no buttons.
    #[must_use]
    pub const fn neutral() -> Self {
        Self {
            x: 0.5,
            y: 0.0,
            z: 0.0,
            rx: 0.0,
            slider: 0.0,
            buttons: 0,
        }
    }
}

/// Build a [`GamepadFrame`] from one processed tick, gated by the
/// device's capabilities.
///
/// An axis the profile lacks is left at its neutral value; buttons
/// 1–25 map to the `HMButton` bits. **Buttons 26–32, `buttons_ex`
/// (33–64) and encoder pulses (65–72) are dropped on this path** —
/// `HMButton` has no bits for them (documented gap; use the raw-report
/// path for buttons 26–32, and note that 33+ have no wire slot on the
/// custom profile at all).
#[must_use]
pub fn build_gamepad_frame(state: &OutputState, caps: &HidMaestroCapabilities) -> GamepadFrame {
    let mut frame = GamepadFrame::neutral();
    if caps.has_axis_x {
        frame.x = steering_to_float(state.steering);
    }
    if caps.has_axis_y {
        frame.y = pedal_to_float(state.accelerator);
    }
    if caps.has_axis_z {
        frame.z = pedal_to_float(state.brake);
    }
    // Clutch is caps-independent for the *value* (Rx is always written;
    // the SDK ignores it when the axis is absent) — but the frame only
    // carries it when the profile has Rx, so a missing Rx reads as
    // released rather than stale. Same rule as the C# series.
    if caps.has_axis_rx {
        frame.rx = pedal_to_float(state.clutch);
    }
    if caps.has_axis_slider {
        frame.slider = pedal_to_float(state.handbrake);
    }
    // L1: the `SubmitState` mask (buttons 1–25) AND the device's own
    // button mask both apply — a 10-button device must never report
    // buttons 11–25 as pressed on this path.
    frame.buttons = state.buttons & SUBMIT_STATE_BUTTON_MASK & caps.button_mask();
    frame
}

/// Neutral [`GamepadFrame`] for the given caps (watchdog / shutdown).
#[must_use]
pub fn neutral_gamepad_frame(caps: &HidMaestroCapabilities) -> GamepadFrame {
    let _ = caps;
    GamepadFrame::neutral()
}

/// Build the custom profile's 15-byte raw HID input report
/// (`SubmitRawReport` path). Layout:
/// `[0..2]` X steering, `[2..4]` Y accel, `[4..6]` Z brake, `[6..8]`
/// Rx clutch, `[8..10]` Slider handbrake (all u16 LE, value = axis
/// − 1); `[10..14]` buttons 1–32 (bit 0 = button 1); `[14]` hat
/// ([`HAT_NEUTRAL_RAW`] — RSH has no hat input).
///
/// An axis the profile lacks stays neutral; buttons are masked by the
/// device's button count. `buttons_ex` (33–64) and encoder pulses
/// (65–72) have no wire slot on the 32-button custom profile and are
/// dropped — documented, not silently folded anywhere.
pub fn build_raw_report(
    state: &OutputState,
    caps: &HidMaestroCapabilities,
    out: &mut [u8; CUSTOM_RAW_REPORT_SIZE],
) {
    out.fill(0);
    let x = if caps.has_axis_x {
        steering_to_axis(state.steering)
    } else {
        16384
    };
    let y = if caps.has_axis_y {
        pedal_to_axis(state.accelerator)
    } else {
        1
    };
    let z = if caps.has_axis_z {
        pedal_to_axis(state.brake)
    } else {
        1
    };
    let rx = if caps.has_axis_rx {
        pedal_to_axis(state.clutch)
    } else {
        1
    };
    let slider = if caps.has_axis_slider {
        pedal_to_axis(state.handbrake)
    } else {
        1
    };
    write_u16(out, 0, axis_to_u16(x));
    write_u16(out, 2, axis_to_u16(y));
    write_u16(out, 4, axis_to_u16(z));
    write_u16(out, 6, axis_to_u16(rx));
    write_u16(out, 8, axis_to_u16(slider));
    write_u32(out, 10, state.buttons & caps.button_mask());
    out[14] = HAT_NEUTRAL_RAW;
}

/// Neutral 15-byte raw report (watchdog / shutdown).
pub fn neutral_raw_report(caps: &HidMaestroCapabilities, out: &mut [u8; CUSTOM_RAW_REPORT_SIZE]) {
    build_raw_report(&OutputState::neutral(), caps, out);
}

fn write_u16(out: &mut [u8; CUSTOM_RAW_REPORT_SIZE], offset: usize, value: u16) {
    let bytes = value.to_le_bytes();
    out[offset] = bytes[0];
    out[offset + 1] = bytes[1];
}

fn write_u32(out: &mut [u8; CUSTOM_RAW_REPORT_SIZE], offset: usize, value: u32) {
    let bytes = value.to_le_bytes();
    out[offset] = bytes[0];
    out[offset + 1] = bytes[1];
    out[offset + 2] = bytes[2];
    out[offset + 3] = bytes[3];
}

/// Read one axis back out of a raw report (test/verify): index 0 = X,
/// 1 = Y, 2 = Z, 3 = Rx, 4 = Slider. Returns `None` for an out-of-range
/// index.
#[must_use]
pub fn read_raw_axis(report: &[u8; CUSTOM_RAW_REPORT_SIZE], axis_index: usize) -> Option<u16> {
    if axis_index > 4 {
        return None;
    }
    let offset = axis_index * 2;
    Some(u16::from_le_bytes([report[offset], report[offset + 1]]))
}

/// True when wheel button `button` (1–32) is pressed in a raw report.
/// Out-of-range buttons (0, 33+) return false — they have no wire slot
/// on the custom profile.
#[must_use]
pub fn is_raw_button_pressed(report: &[u8; CUSTOM_RAW_REPORT_SIZE], button: u8) -> bool {
    if !(1..=32).contains(&button) {
        return false;
    }
    let bit = usize::from(button) - 1;
    report[10 + bit / 8] & (1u8 << (bit % 8)) != 0
}

/// D9: validate a single physical button number for the 1–32 word.
///
/// Buttons 1–32 pass; anything else (0, 33, 128, …) is rejected with
/// [`HidMaestroError::InvalidButton`] — never clamped onto 1–32.
/// Buttons 33–64 and encoder pulses 65–72 are not "button 33 in the
/// 1–32 word": they have no wire slot on the custom profile at all —
/// see [`build_raw_report`].
///
/// # Errors
///
/// Returns [`HidMaestroError::InvalidButton`] when `button` is outside
/// 1–32.
pub fn check_button_number(button: u8) -> Result<(), HidMaestroError> {
    if (1..=32).contains(&button) {
        Ok(())
    } else {
        Err(HidMaestroError::InvalidButton(button))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_state() -> OutputState {
        let mut s = OutputState::neutral();
        s.steering = 1.0;
        s.accelerator = 1.0;
        s.brake = 1.0;
        s.clutch = 1.0;
        s.handbrake = 1.0;
        s.buttons = u32::MAX;
        s
    }

    #[test]
    fn steering_float_edges_exact() {
        assert_eq!(steering_to_float(-1.0), 0.5 + (1.0 - 16384.0) / 32768.0);
        assert_eq!(steering_to_float(0.0), 0.5);
        assert_eq!(steering_to_float(1.0), 1.0);
        assert!(steering_to_float(-1.0) > 0.0);
    }

    #[test]
    fn steering_float_non_finite_is_centre() {
        assert_eq!(steering_to_float(f32::NAN), 0.5);
        assert_eq!(steering_to_float(f32::INFINITY), 0.5);
        assert_eq!(steering_to_float(f32::NEG_INFINITY), 0.5);
    }

    #[test]
    fn pedal_float_edges_exact() {
        assert_eq!(pedal_to_float(0.0), 0.0);
        assert_eq!(pedal_to_float(1.0), 1.0);
        // 0.5 * 32767 = 16383.5 → banker's rounding to the even
        // neighbour 16384 → axis 16385 → (16385-1)/32767.
        assert_eq!(pedal_to_float(0.5), (16385.0 - 1.0) / 32767.0);
    }

    #[test]
    fn pedal_float_non_finite_is_released() {
        assert_eq!(pedal_to_float(f32::NAN), 0.0);
        assert_eq!(pedal_to_float(f32::INFINITY), 0.0);
    }

    #[test]
    fn pedal_float_out_of_range_clamped() {
        assert_eq!(pedal_to_float(-0.5), 0.0);
        assert_eq!(pedal_to_float(2.0), 1.0);
        assert_eq!(steering_to_float(-2.0), steering_to_float(-1.0));
        assert_eq!(steering_to_float(2.0), 1.0);
    }

    #[test]
    fn gamepad_frame_full_deflection() {
        let frame = build_gamepad_frame(&full_state(), &HidMaestroCapabilities::custom_wheel());
        assert_eq!(frame.x, 1.0);
        assert_eq!(frame.y, 1.0);
        assert_eq!(frame.z, 1.0);
        assert_eq!(frame.rx, 1.0);
        assert_eq!(frame.slider, 1.0);
        assert_eq!(frame.buttons, SUBMIT_STATE_BUTTON_MASK);
    }

    #[test]
    fn gamepad_frame_neutral_matches_expected() {
        let frame = build_gamepad_frame(
            &OutputState::neutral(),
            &HidMaestroCapabilities::custom_wheel(),
        );
        assert_eq!(frame, GamepadFrame::neutral());
    }

    #[test]
    fn gamepad_frame_buttons_26_to_32_dropped() {
        let mut s = OutputState::neutral();
        s.buttons = 1u32 << 25 | 1u32 << 31; // buttons 26 and 32
        let frame = build_gamepad_frame(&s, &HidMaestroCapabilities::custom_wheel());
        assert_eq!(frame.buttons, 0);
    }

    /// L1: the device button mask applies on the `SubmitState` path
    /// too — a 10-button device reports only buttons 1–10, even
    /// though `HMButton` has bits up to 25.
    #[test]
    fn gamepad_frame_buttons_masked_by_device_button_count() {
        let caps = HidMaestroCapabilities::new(true, true, true, true, true, 10, true);
        let mut s = OutputState::neutral();
        s.buttons = u32::MAX;
        let frame = build_gamepad_frame(&s, &caps);
        assert_eq!(frame.buttons, 0x3FF); // buttons 1–10 only
        let caps0 = HidMaestroCapabilities::new(true, true, true, true, true, 0, false);
        let frame0 = build_gamepad_frame(&s, &caps0);
        assert_eq!(frame0.buttons, 0);
    }

    #[test]
    fn gamepad_frame_buttons_ex_and_encoder_dropped() {
        let mut s = OutputState::neutral();
        s.buttons = u32::MAX;
        s.buttons_ex = u32::MAX;
        let frame = build_gamepad_frame(&s, &HidMaestroCapabilities::custom_wheel());
        assert_eq!(frame.buttons, SUBMIT_STATE_BUTTON_MASK);
    }

    #[test]
    fn gamepad_frame_caps_gating_missing_axis_stays_neutral() {
        let caps = HidMaestroCapabilities::new(false, false, false, false, false, 32, true);
        let mut state = full_state();
        state.buttons = 0; // buttons are not axis-gated; test axes only
        let frame = build_gamepad_frame(&state, &caps);
        assert_eq!(frame, GamepadFrame::neutral());
    }

    #[test]
    fn raw_report_size_is_15_not_26() {
        // 5x16-bit axes + 32x1-bit buttons + 1x8-bit hat = 120 bits.
        assert_eq!(CUSTOM_RAW_REPORT_SIZE, 15);
        assert_eq!(5 * 16 + 32 + 8, CUSTOM_RAW_REPORT_SIZE * 8);
    }

    #[test]
    fn raw_report_neutral_matches_expected() {
        let mut out = [0u8; CUSTOM_RAW_REPORT_SIZE];
        neutral_raw_report(&HidMaestroCapabilities::custom_wheel(), &mut out);
        assert_eq!(read_raw_axis(&out, 0), Some(16383));
        assert_eq!(read_raw_axis(&out, 1), Some(0));
        assert_eq!(read_raw_axis(&out, 2), Some(0));
        assert_eq!(read_raw_axis(&out, 3), Some(0));
        assert_eq!(read_raw_axis(&out, 4), Some(0));
        assert_eq!(out[10..14], [0, 0, 0, 0]);
        assert_eq!(out[14], HAT_NEUTRAL_RAW);
    }

    #[test]
    fn raw_report_full_deflection_edges_exact() {
        let mut out = [0u8; CUSTOM_RAW_REPORT_SIZE];
        build_raw_report(
            &full_state(),
            &HidMaestroCapabilities::custom_wheel(),
            &mut out,
        );
        assert_eq!(read_raw_axis(&out, 0), Some(32767));
        assert_eq!(read_raw_axis(&out, 1), Some(32767));
        assert_eq!(read_raw_axis(&out, 2), Some(32767));
        assert_eq!(read_raw_axis(&out, 3), Some(32767));
        assert_eq!(read_raw_axis(&out, 4), Some(32767));
        assert_eq!(out[10..14], [0xFF, 0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn raw_report_buttons_1_to_32_all_set() {
        for button in 1..=32u8 {
            let mut s = OutputState::neutral();
            s.buttons = 1u32 << (u32::from(button) - 1);
            let mut out = [0u8; CUSTOM_RAW_REPORT_SIZE];
            build_raw_report(&s, &HidMaestroCapabilities::custom_wheel(), &mut out);
            assert!(
                is_raw_button_pressed(&out, button),
                "button {button} not set"
            );
            assert_eq!(out[10..14].iter().filter(|b| **b != 0).count(), 1);
        }
    }

    #[test]
    fn raw_report_buttons_33_plus_have_no_wire_slot() {
        let mut s = OutputState::neutral();
        s.buttons_ex = u32::MAX;
        let mut out = [0u8; CUSTOM_RAW_REPORT_SIZE];
        build_raw_report(&s, &HidMaestroCapabilities::custom_wheel(), &mut out);
        assert_eq!(out[10..14], [0, 0, 0, 0]);
        assert!(!is_raw_button_pressed(&out, 33));
        assert!(!is_raw_button_pressed(&out, 0));
    }

    #[test]
    fn raw_report_caps_gating_missing_axis_stays_neutral() {
        let caps = HidMaestroCapabilities::new(false, false, false, false, false, 0, false);
        let mut out = [0u8; CUSTOM_RAW_REPORT_SIZE];
        build_raw_report(&full_state(), &caps, &mut out);
        assert_eq!(read_raw_axis(&out, 0), Some(16383));
        assert_eq!(read_raw_axis(&out, 1), Some(0));
        assert_eq!(out[10..14], [0, 0, 0, 0]);
    }

    #[test]
    fn read_raw_axis_out_of_range_is_none() {
        let out = [0u8; CUSTOM_RAW_REPORT_SIZE];
        assert_eq!(read_raw_axis(&out, 5), None);
        assert_eq!(read_raw_axis(&out, 99), None);
    }

    #[test]
    fn check_button_number_range() {
        assert!(check_button_number(1).is_ok());
        assert!(check_button_number(32).is_ok());
        assert!(check_button_number(0).is_err());
        assert!(check_button_number(33).is_err());
        assert!(check_button_number(128).is_err());
        assert!(matches!(
            check_button_number(33),
            Err(HidMaestroError::InvalidButton(33))
        ));
    }

    #[test]
    fn caps_masks_follow_button_count() {
        assert_eq!(
            HidMaestroCapabilities::custom_wheel().button_mask(),
            u32::MAX
        );
        assert_eq!(HidMaestroCapabilities::custom_wheel().ex1_mask(), 0);
        assert_eq!(HidMaestroCapabilities::custom_wheel().ex2_mask(), 0);
        let caps = HidMaestroCapabilities::new(true, true, true, true, true, 0, false);
        assert_eq!(caps.button_mask(), 0);
        let caps = HidMaestroCapabilities::new(true, true, true, true, true, 1, false);
        assert_eq!(caps.button_mask(), 1);
    }

    #[test]
    fn validate_capabilities_full_device_is_valid() {
        let plan = PipelinePlan::default();
        let caps = HidMaestroCapabilities::custom_wheel();
        assert_eq!(validate_capabilities(&plan, &caps).len(), 0);
    }

    #[test]
    fn validate_capabilities_steering_only_minimal_caps_is_valid() {
        let mut plan = PipelinePlan::default();
        plan.accelerator.enabled = false;
        plan.brake.enabled = false;
        plan.clutch.enabled = false;
        plan.handbrake.enabled = false;
        plan.buttons.enabled = false;
        let caps = HidMaestroCapabilities::new(true, false, false, false, false, 0, false);
        assert_eq!(validate_capabilities(&plan, &caps).len(), 0);
    }

    #[test]
    fn validate_capabilities_missing_axes_reported() {
        let plan = PipelinePlan::default();
        let caps = HidMaestroCapabilities::new(false, false, false, false, false, 32, false);
        let errors = validate_capabilities(&plan, &caps);
        assert_eq!(errors.len(), 4); // X, Y, Z, Slider (clutch needs none)
        assert!(errors.iter().any(|e| e.contains("Steering")));
        assert!(errors.iter().any(|e| e.contains("Handbrake")));
    }

    #[test]
    fn validate_capabilities_buttons_need_at_least_one() {
        let plan = PipelinePlan::default();
        let caps = HidMaestroCapabilities::new(true, true, true, true, true, 0, false);
        let errors = validate_capabilities(&plan, &caps);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("Buttons"));
    }

    #[test]
    fn validate_capabilities_button_count_out_of_range() {
        let plan = PipelinePlan::default();
        let caps = HidMaestroCapabilities::new(true, true, true, true, true, 129, false);
        assert_ne!(validate_capabilities(&plan, &caps).len(), 0);
        let caps = HidMaestroCapabilities::new(true, true, true, true, true, -1, false);
        assert_ne!(validate_capabilities(&plan, &caps).len(), 0);
    }

    #[test]
    fn validate_capabilities_disabled_module_imposes_nothing() {
        let mut plan = PipelinePlan::default();
        plan.handbrake.enabled = false;
        plan.buttons.enabled = false;
        let caps = HidMaestroCapabilities::new(true, true, true, true, false, 0, false);
        assert_eq!(validate_capabilities(&plan, &caps).len(), 0);
    }

    #[test]
    fn validate_capabilities_clutch_needs_no_extra_axis() {
        let mut plan = PipelinePlan::default();
        plan.clutch.enabled = true;
        let caps = HidMaestroCapabilities::new(true, true, true, false, true, 32, false);
        assert_eq!(validate_capabilities(&plan, &caps).len(), 0);
    }
}
