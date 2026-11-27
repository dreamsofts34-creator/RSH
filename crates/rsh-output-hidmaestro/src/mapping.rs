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

// ---------------------------------------------------------------------------
// Logitech G29 spoof profile (`logitech-g29`, HIDMaestro catalog).
//
// RISK DISCLOSURE (research 2026-11-27, verified against the real
// v2.1.0 DLL's embedded profile JSON): presenting Logitech's VID/PID
// violates USB-IF policy as written (no enforcement found for private
// hobby use); no Forza ToS text on spoofing was found (risk low, not
// zero); the user's pid.codes `0x1209`/`0x5253` claim is UNAFFECTED
// (simply unused while G29 is active). Even a REAL G29 needs a custom
// control scheme in FH4 (Forza Known Issues) — do not promise
// auto-recognition anywhere. FH4 end-to-end is UNPROVEN: no report
// of HIDMaestro-as-G29 in any game was found; the realistic goal is
// usable steering/pedals via FH4's custom wheel profile.
//
// The 123-byte descriptor (no Report ID) is byte-for-byte the
// catalog profile's, verified from the DLL resource AND the research
// report (they match). The sidecar loads the catalog profile via the
// SDK (`GetProfile`) and validates these bytes — the descriptor is
// NEVER hand-rolled here. Input report is 12 bytes:
// hat (4 bits) + buttons 1–25 (25 bits) + 3 pad bits, then X steering
// u16 LE (0–65535, centre 32768), Z / Rz / Y pedals u8, then vendor
// 0xFF00 input (2 bytes) + vendor input (1 byte), both neutral 0.
// The 16-byte 0xFF00 Output report (FFB channel) is kept in the
// profile; the sidecar subscribes to `OutputReceived` and DISCARDS
// the writes (no motor) — see `tools/rsh-hm-sidecar/Program.cs`.

/// Raw-report size of the G29 profile: 4 bytes hat+buttons + 2 bytes
/// X + 3 bytes pedals + 3 bytes vendor input = 12 bytes.
pub const G29_RAW_REPORT_SIZE: usize = 12;
/// G29 VID (Logitech).
pub const G29_VID: u16 = 0x046D;
/// G29 PID (PC mode, `0xC24F` — NOT the PS4-mode `0xC260`, whose
/// 160-byte descriptor must never be paired with this PID).
pub const G29_PID: u16 = 0xC24F;
/// G29 button count (25).
pub const G29_BUTTON_COUNT: i32 = 25;
/// The catalog profile's 123-byte descriptor, lowercase hex. The
/// sidecar validates the loaded profile's descriptor bytes against
/// this exact string; the manager validates the `describe-profile`
/// reply the same way.
pub const G29_DESCRIPTOR_HEX: &str = "05010904a1010939150025073500463b0165147504950181420509190129196500250145017501951981027501950381030501093027ffff000047ffff000075109501810209320935093126ff0046ff007508950381020600ff0901950281020600ff0901950181020600ff090226ff0046ff00951075089102c0";
/// G29 handbrake mapping: the G29 has no handbrake axis, so the
/// handbrake is a BUTTON — button 8 (L2) by default — pressed when
/// the handbrake value exceeds [`G29_HANDBRAKE_THRESHOLD`]. WHY L2:
/// the face buttons (1–4) are menu buttons, R1/L1 (5–6) are the
/// shift paddles, and buttons 13–19 are the H-pattern gears +
/// reverse — L2 is the least-loaded trigger-style button, and a
/// handbrake IS a trigger pull in shape. The button is configurable
/// at runtime via the persisted `g29_handbrake_button` setting
/// (1–25, default 8 — this constant); [`build_g29_raw_report`]
/// takes the live button as a parameter, and a physical button 8
/// held at the same time still reports (the OR is the collision
/// the setting exists to avoid).
pub const G29_HANDBRAKE_BUTTON: u8 = 8;
/// Handbrake value above which the G29 handbrake button is pressed.
pub const G29_HANDBRAKE_THRESHOLD: f32 = 0.5;

/// One of the G29's three 8-bit pedal axes (descriptor order Z, Rz, Y).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum G29PedalAxis {
    /// Descriptor byte 6.
    Z,
    /// Descriptor byte 7.
    Rz,
    /// Descriptor byte 8.
    Y,
}

/// Which G29 pedal axis carries each RSH pedal.
///
/// PEDAL-ROLE CONFLICT (unresolved, research §1): the `HIDMaestro`
/// profile layout says Z = accelerator, Rz = brake, Y = clutch;
/// `PCGamingWiki` says Y = accelerator (with Rz = brake). The
/// descriptor order (X, Z, Rz, Y) is agreed; the roles are NOT.
/// The assignment is therefore a runtime setting, not hardcoded
/// logic: the persisted `g29_pedal_layout` setting
/// (`"hidmaestro"` | `"pcgamingwiki"`, see
/// [`G29PedalMap::from_layout`]) picks the map, and the PC test
/// (`joy.cpl` — move each pedal, watch which axis moves) decides
/// which layout is right for a given game. Z carries clutch in the
/// `PCGamingWiki` layout because the G29 descriptor has no Slider
/// axis — there is no fourth pedal slot, so clutch takes Z and
/// nothing is left unassigned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct G29PedalMap {
    /// Accelerator axis.
    pub accelerator: G29PedalAxis,
    /// Brake axis.
    pub brake: G29PedalAxis,
    /// Clutch axis.
    pub clutch: G29PedalAxis,
}

impl G29PedalMap {
    /// Persisted layout value for the `HIDMaestro` profile layout.
    pub const LAYOUT_HIDMAESTRO: &'static str = "hidmaestro";
    /// Persisted layout value for the `PCGamingWiki` layout.
    pub const LAYOUT_PCGAMINGWIKI: &'static str = "pcgamingwiki";

    /// The `HIDMaestro` profile layout: Z = accelerator, Rz = brake,
    /// Y = clutch. Default until the PC test proves otherwise.
    pub const PROFILE: Self = Self {
        accelerator: G29PedalAxis::Z,
        brake: G29PedalAxis::Rz,
        clutch: G29PedalAxis::Y,
    };
    /// Alternate layout for the `PCGamingWiki` reading (Y =
    /// accelerator, Z = clutch; brake stays Rz in both sources).
    pub const ACCEL_CLUTCH_SWAPPED: Self = Self {
        accelerator: G29PedalAxis::Y,
        brake: G29PedalAxis::Rz,
        clutch: G29PedalAxis::Z,
    };

    /// Parse a persisted `g29_pedal_layout` value (case-insensitive,
    /// surrounding whitespace ignored). `None` for anything else —
    /// the settings layer canonicalizes unknown values back to the
    /// default instead of guessing, and the backend keeps its
    /// current map when a live push carries an unknown value.
    #[must_use]
    pub fn from_layout(layout: &str) -> Option<Self> {
        match layout.trim().to_ascii_lowercase().as_str() {
            Self::LAYOUT_HIDMAESTRO => Some(Self::PROFILE),
            Self::LAYOUT_PCGAMINGWIKI => Some(Self::ACCEL_CLUTCH_SWAPPED),
            _ => None,
        }
    }
}

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

    /// Capabilities of the `logitech-g29` catalog profile: X steering
    /// (16-bit), 3 pedals (8-bit each), 25 buttons, hat — NO Rx, NO
    /// Slider. The `has_axis_*` slots are ROLE slots here, not the
    /// custom profile's axis names: `has_axis_y` = accelerator
    /// present, `has_axis_z` = brake present, `has_axis_rx` = clutch
    /// present, `has_axis_slider` = false (the G29 has no Slider —
    /// the handbrake is button-mapped, see [`G29_HANDBRAKE_BUTTON`]).
    #[must_use]
    pub const fn g29_wheel() -> Self {
        Self {
            has_axis_x: true,
            has_axis_y: true,
            has_axis_z: true,
            has_axis_rx: true,
            has_axis_slider: false,
            button_count: G29_BUTTON_COUNT,
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

/// Capabilities of the `logitech-g29` catalog profile (free-function
/// form of [`HidMaestroCapabilities::g29_wheel`]).
#[must_use]
pub const fn g29_wheel() -> HidMaestroCapabilities {
    HidMaestroCapabilities::g29_wheel()
}

/// Validate a device's capabilities against the configured modules
/// (D11, C# `HidMaestroOutput.ValidateCapabilities`).
///
/// Only *enabled* modules impose requirements: a steering-only wheel
/// is valid with just X, a disabled module imposes nothing. Clutch
/// imposes no extra requirement (its `Rx` value is always written;
/// the SDK ignores it when the axis is absent — vJoy `wClutch` parity).
/// Returns the list of problems; empty means valid.
///
/// This is the custom-profile form — see
/// [`validate_capabilities_for_profile`] for the G29 form, where the
/// handbrake does NOT require a Slider axis (it is button-mapped).
#[must_use]
pub fn validate_capabilities(plan: &PipelinePlan, caps: &HidMaestroCapabilities) -> Vec<String> {
    validate_capabilities_for_profile(plan, caps, false)
}

/// Profile-aware form of [`validate_capabilities`].
///
/// `is_g29` true means the device is the `logitech-g29` profile:
/// the handbrake module then imposes NO Slider requirement — under
/// G29 the handbrake is a button ([`G29_HANDBRAKE_BUTTON`], pressed
/// past [`G29_HANDBRAKE_THRESHOLD`]), so a G29 device with the
/// handbrake module on is valid despite having no Slider axis. Every
/// other requirement is identical to the custom profile.
#[must_use]
pub fn validate_capabilities_for_profile(
    plan: &PipelinePlan,
    caps: &HidMaestroCapabilities,
    is_g29: bool,
) -> Vec<String> {
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
    // G29: the handbrake is button-mapped (L2), never a Slider axis —
    // requiring one here would make every G29 connect fail validation
    // with the handbrake module on. Documented, not silently skipped:
    // the button mapping lives in `build_g29_raw_report`.
    if plan.handbrake.enabled && !caps.has_axis_slider && !is_g29 {
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

/// G29 steering `-1..=1` → full-range u16 `0..=65535` (the G29 X axis
/// is 16-bit, 0–65535 — NOT the custom profile's 0–32767 range).
/// Endpoints exact: full left `0`, centre `32768`, full right `65535`.
/// Non-finite input maps to centre.
#[must_use]
pub fn g29_steering_u16(value: f32) -> u16 {
    if !value.is_finite() {
        return 32768;
    }
    let v = value.clamp(-1.0, 1.0);
    // `(v + 1) * 32767.5`: -1 → 0, 0 → 32767.5 → 32768 (round half
    // away from zero), +1 → 65535.0. The cast is exact: the value is
    // 0..=65535 after rounding.
    #[allow(clippy::cast_possible_truncation)]
    #[allow(clippy::cast_sign_loss)]
    {
        ((v + 1.0) * 32767.5).round() as u16
    }
}

/// G29 pedal `0..=1` → u8 `0..=255` (the G29 pedals are 8-bit).
/// Endpoints exact: released `0`, fully pressed `255`.
/// Non-finite input maps to released.
#[must_use]
pub fn g29_pedal_u8(value: f32) -> u8 {
    if !value.is_finite() {
        return 0;
    }
    let v = value.clamp(0.0, 1.0);
    #[allow(clippy::cast_possible_truncation)]
    #[allow(clippy::cast_sign_loss)]
    {
        (v * 255.0).round() as u8
    }
}

/// One G29 pedal byte: the scaled value when the pedal's role axis
/// is present on the profile, released (0) when it is absent — never
/// stale. Presence is read off the caps' role slots (see
/// [`HidMaestroCapabilities::g29_wheel`]): accelerator ↔ Y slot,
/// brake ↔ Z slot, clutch ↔ Rx slot.
fn g29_pedal_byte(value: f32, present: bool) -> u8 {
    if present { g29_pedal_u8(value) } else { 0 }
}

/// Index of a [`G29PedalAxis`] in descriptor order (Z = byte 6,
/// Rz = byte 7, Y = byte 8).
const fn g29_pedal_index(axis: G29PedalAxis) -> usize {
    match axis {
        G29PedalAxis::Z => 0,
        G29PedalAxis::Rz => 1,
        G29PedalAxis::Y => 2,
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

/// Build the G29 profile's 12-byte raw HID input report
/// (`SubmitRawReport` path — the ONLY submit path for G29; the
/// `SubmitState` frame layout does not match this profile and must
/// never be used for it, enforced on both sides). Layout, in
/// descriptor order:
/// Bytes 0–3: hat (low 4 bits, always [`HAT_NEUTRAL_RAW`] — see
/// below) + buttons 1–25 (bits 4–28, bit 0 of the field =
/// button 1) + 3 pad bits. Bytes 4–5: X steering u16 LE (0–65535,
/// centre 32768). Bytes 6–8: Z, Rz, Y pedals u8 (roles per `map` —
/// see [`G29PedalMap`]). Bytes 9–10: vendor 0xFF00 input (2 bytes,
/// neutral 0). Byte 11: vendor input (1 byte, neutral 0).
///
/// Hat: there is NO D-pad signal anywhere in the RSH input path —
/// the firmware stream packet, the pipeline [`OutputState`], and
/// the vJoy feed all carry no hat/D-pad input (the v2.0.14
/// `GameInput` `RacingWheel\DPad*` entries are Windows-side mapping
/// declarations for switch 0, not a fed input), so the hat is
/// always neutral (0x08). A real D-pad would need a firmware
/// protocol change (protocol v2.2 is frozen) plus a pipeline field,
/// so it is not built here — the neutral hat is the honest report,
/// not a dropped input.
///
/// Handbrake: no axis — mapped to button `handbrake_button`
/// (default [`G29_HANDBRAKE_BUTTON`], L2) when `state.handbrake`
/// exceeds [`G29_HANDBRAKE_THRESHOLD`].
///
/// Buttons 26–32, `buttons_ex` (33–64) and encoder pulses (65–72)
/// have no wire slot on the 25-button G29 profile and are dropped —
/// documented, not silently folded anywhere. All 25 G29 buttons
/// are already used by RSH buttons 1–25, so there is no free slot
/// to overflow into: no overflow mapping exists, and adding one
/// would need a user-facing choice of which G29 button to steal
/// (a new mapping UI), not a silent remap. Buttons are masked by
/// the device's button count (25 on a G29).
pub fn build_g29_raw_report(
    state: &OutputState,
    caps: &HidMaestroCapabilities,
    map: G29PedalMap,
    handbrake_button: u8,
    out: &mut [u8; G29_RAW_REPORT_SIZE],
) {
    out.fill(0);
    // Buttons 1–25 packed into bits 4–28 of the first u32, hat
    // neutral in the low nibble. `SUBMIT_STATE_BUTTON_MASK` is the
    // 25-bit mask (buttons 1–25) — reused here, same width.
    let mut word: u32 = u32::from(HAT_NEUTRAL_RAW);
    word |= (state.buttons & SUBMIT_STATE_BUTTON_MASK & caps.button_mask()) << 4;
    if state.handbrake > G29_HANDBRAKE_THRESHOLD
        && (1..=G29_BUTTON_COUNT).contains(&i32::from(handbrake_button))
    {
        word |= 1u32 << (4 + u32::from(handbrake_button) - 1);
    }
    out[0..4].copy_from_slice(&word.to_le_bytes());
    let x = if caps.has_axis_x {
        g29_steering_u16(state.steering)
    } else {
        32768
    };
    out[4..6].copy_from_slice(&x.to_le_bytes());
    // Pedals land on their mapped descriptor bytes (Z = 6, Rz = 7,
    // Y = 8); the vendor bytes (9–11) stay 0 (neutral).
    let mut pedals = [0u8; 3];
    pedals[g29_pedal_index(map.accelerator)] = g29_pedal_byte(state.accelerator, caps.has_axis_y);
    pedals[g29_pedal_index(map.brake)] = g29_pedal_byte(state.brake, caps.has_axis_z);
    pedals[g29_pedal_index(map.clutch)] = g29_pedal_byte(state.clutch, caps.has_axis_rx);
    out[6] = pedals[0];
    out[7] = pedals[1];
    out[8] = pedals[2];
}

/// Neutral 12-byte G29 raw report (watchdog / shutdown): steering
/// centred (32768), pedals released, no buttons, hat neutral.
pub fn neutral_g29_raw_report(caps: &HidMaestroCapabilities, out: &mut [u8; G29_RAW_REPORT_SIZE]) {
    build_g29_raw_report(
        &OutputState::neutral(),
        caps,
        G29PedalMap::PROFILE,
        G29_HANDBRAKE_BUTTON,
        out,
    );
}

/// True when G29 wheel button `button` (1–25) is pressed in a G29
/// raw report. Out-of-range buttons (0, 26+) return false — they
/// have no wire slot on the G29 profile.
#[must_use]
pub fn is_g29_button_pressed(report: &[u8; G29_RAW_REPORT_SIZE], button: u8) -> bool {
    if !(1..=25).contains(&button) {
        return false;
    }
    let bit = 4 + usize::from(button) - 1;
    report[bit / 8] & (1u8 << (bit % 8)) != 0
}

/// Read the G29 steering value back out of a raw report (test/verify).
#[must_use]
pub fn read_g29_steering(report: &[u8; G29_RAW_REPORT_SIZE]) -> u16 {
    u16::from_le_bytes([report[4], report[5]])
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

    // --- G29 spoof profile (v2.0.16) ---

    #[test]
    fn g29_descriptor_hex_is_123_bytes_and_matches_task() {
        let bytes = (0..G29_DESCRIPTOR_HEX.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&G29_DESCRIPTOR_HEX[i..i + 2], 16).expect("hex"))
            .collect::<Vec<u8>>();
        assert_eq!(bytes.len(), 123);
        assert_eq!(G29_DESCRIPTOR_HEX.len(), 246);
        // Spot-check the head and tail against the task's verbatim hex.
        assert!(G29_DESCRIPTOR_HEX.starts_with("05010904a101093915002507"));
        assert!(G29_DESCRIPTOR_HEX.ends_with("75089102c0"));
    }

    #[test]
    fn g29_raw_report_size_is_12() {
        // Hat+buttons (32 bits) + X (16) + 3 pedals (24) + vendor (24)
        // = 96 bits = 12 bytes, no Report ID.
        assert_eq!(G29_RAW_REPORT_SIZE, 12);
        assert_eq!(32 + 16 + 24 + 24, G29_RAW_REPORT_SIZE * 8);
        assert_eq!(G29_BUTTON_COUNT, 25);
    }

    #[test]
    fn g29_steering_endpoints_exact() {
        assert_eq!(g29_steering_u16(-1.0), 0);
        assert_eq!(g29_steering_u16(0.0), 32768);
        assert_eq!(g29_steering_u16(1.0), 65535);
        assert_eq!(g29_steering_u16(f32::NAN), 32768);
        assert_eq!(g29_steering_u16(f32::INFINITY), 32768);
        assert_eq!(g29_steering_u16(-2.0), 0);
        assert_eq!(g29_steering_u16(2.0), 65535);
        // Quarter deflection: (-0.5 + 1) * 32767.5 = 16383.75 → 16384.
        assert_eq!(g29_steering_u16(-0.5), 16384);
    }

    #[test]
    fn g29_pedal_endpoints_exact() {
        assert_eq!(g29_pedal_u8(0.0), 0);
        assert_eq!(g29_pedal_u8(1.0), 255);
        assert_eq!(g29_pedal_u8(0.5), 128);
        assert_eq!(g29_pedal_u8(f32::NAN), 0);
        assert_eq!(g29_pedal_u8(-0.5), 0);
        assert_eq!(g29_pedal_u8(2.0), 255);
    }

    #[test]
    fn g29_neutral_report_bytes_exact() {
        let mut out = [0u8; G29_RAW_REPORT_SIZE];
        neutral_g29_raw_report(&HidMaestroCapabilities::g29_wheel(), &mut out);
        // Bytes 0-3: hat neutral 0x08, no buttons.
        assert_eq!(out[0], 0x08);
        assert_eq!(out[1..4], [0, 0, 0]);
        // X = 32768 LE.
        assert_eq!(read_g29_steering(&out), 32768);
        assert_eq!(out[4..6], [0x00, 0x80]);
        // Pedals 0, vendor bytes 0.
        assert_eq!(out[6..12], [0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn g29_full_deflection_endpoints_exact() {
        let mut out = [0u8; G29_RAW_REPORT_SIZE];
        build_g29_raw_report(
            &full_state(),
            &HidMaestroCapabilities::g29_wheel(),
            G29PedalMap::PROFILE,
            G29_HANDBRAKE_BUTTON,
            &mut out,
        );
        assert_eq!(read_g29_steering(&out), 65535);
        // PROFILE map: Z (byte 6) = accel 255, Rz (byte 7) = brake
        // 255, Y (byte 8) = clutch 255. Handbrake 1.0 > 0.5 sets L2.
        assert_eq!(out[6], 255);
        assert_eq!(out[7], 255);
        assert_eq!(out[8], 255);
        assert_eq!(out[9..12], [0, 0, 0]);
        // Buttons 1-25 all set: bits 4-28 → word 0x1FFFFFF8 LE.
        assert_eq!(out[0..4], [0xF8, 0xFF, 0xFF, 0x1F]);
        // Hat nibble is overwritten by button 1's bit only in the
        // sense that 0xF8 keeps the low nibble 0x08 (neutral).
        assert_eq!(out[0] & 0x0F, 0x08);
    }

    #[test]
    fn g29_buttons_1_to_25_pack_one_to_one() {
        for button in 1..=25u8 {
            let mut s = OutputState::neutral();
            s.buttons = 1u32 << (u32::from(button) - 1);
            let mut out = [0u8; G29_RAW_REPORT_SIZE];
            build_g29_raw_report(
                &s,
                &HidMaestroCapabilities::g29_wheel(),
                G29PedalMap::PROFILE,
                G29_HANDBRAKE_BUTTON,
                &mut out,
            );
            assert!(
                is_g29_button_pressed(&out, button),
                "button {button} not set"
            );
            // Exactly one button bit set (plus the hat nibble 0x08).
            let word = u32::from_le_bytes([out[0], out[1], out[2], out[3]]);
            assert_eq!(word & !0x0F, 1u32 << (4 + u32::from(button) - 1));
            assert_eq!(word & 0x0F, 0x08);
        }
    }

    #[test]
    fn g29_buttons_26_plus_have_no_wire_slot() {
        let mut s = OutputState::neutral();
        s.buttons = 1u32 << 25 | 1u32 << 31; // buttons 26 and 32
        s.buttons_ex = u32::MAX;
        // Encoder pulses 65–72 (all 8 bits) — also no wire slot.
        s.encoder_buttons = rsh_engine::encoder::EncoderButtons(u8::MAX);
        let mut out = [0u8; G29_RAW_REPORT_SIZE];
        build_g29_raw_report(
            &s,
            &HidMaestroCapabilities::g29_wheel(),
            G29PedalMap::PROFILE,
            G29_HANDBRAKE_BUTTON,
            &mut out,
        );
        assert_eq!(out[0..4], [0x08, 0, 0, 0]);
        assert!(!is_g29_button_pressed(&out, 26));
        assert!(!is_g29_button_pressed(&out, 0));
    }

    #[test]
    fn g29_hat_always_neutral() {
        // v2.0.17: the hat is neutral because there is NO D-pad
        // signal in the RSH input path (no firmware packet field,
        // no pipeline field) — not because a D-pad is mapped and
        // centred. A real D-pad needs a protocol change (v2.2 is
        // frozen), so this stays neutral by construction.
        let mut out = [0u8; G29_RAW_REPORT_SIZE];
        build_g29_raw_report(
            &full_state(),
            &HidMaestroCapabilities::g29_wheel(),
            G29PedalMap::PROFILE,
            G29_HANDBRAKE_BUTTON,
            &mut out,
        );
        assert_eq!(out[0] & 0x0F, 0x08);
    }

    #[test]
    fn g29_handbrake_button_threshold() {
        let caps = HidMaestroCapabilities::g29_wheel();
        // At/below 0.5: no L2 (button 8).
        for value in [0.0, 0.25, 0.5] {
            let mut s = OutputState::neutral();
            s.handbrake = value;
            let mut out = [0u8; G29_RAW_REPORT_SIZE];
            build_g29_raw_report(
                &s,
                &caps,
                G29PedalMap::PROFILE,
                G29_HANDBRAKE_BUTTON,
                &mut out,
            );
            assert!(
                !is_g29_button_pressed(&out, G29_HANDBRAKE_BUTTON),
                "handbrake {value} must not press L2"
            );
        }
        // Above 0.5: L2 pressed, and only L2.
        let mut s = OutputState::neutral();
        s.handbrake = 0.51;
        let mut out = [0u8; G29_RAW_REPORT_SIZE];
        build_g29_raw_report(
            &s,
            &caps,
            G29PedalMap::PROFILE,
            G29_HANDBRAKE_BUTTON,
            &mut out,
        );
        assert!(is_g29_button_pressed(&out, G29_HANDBRAKE_BUTTON));
        let word = u32::from_le_bytes([out[0], out[1], out[2], out[3]]);
        assert_eq!(word, 0x08 | (1u32 << (4 + 8 - 1)));
    }

    #[test]
    fn g29_pedal_map_swapped_moves_accel_to_y() {
        let mut s = OutputState::neutral();
        s.accelerator = 1.0;
        s.clutch = 0.0;
        let caps = HidMaestroCapabilities::g29_wheel();
        let mut out = [0u8; G29_RAW_REPORT_SIZE];
        build_g29_raw_report(
            &s,
            &caps,
            G29PedalMap::PROFILE,
            G29_HANDBRAKE_BUTTON,
            &mut out,
        );
        assert_eq!((out[6], out[7], out[8]), (255, 0, 0)); // Z = accel
        build_g29_raw_report(
            &s,
            &caps,
            G29PedalMap::ACCEL_CLUTCH_SWAPPED,
            G29_HANDBRAKE_BUTTON,
            &mut out,
        );
        assert_eq!((out[6], out[7], out[8]), (0, 0, 255)); // Y = accel
    }

    #[test]
    fn g29_pedal_layout_from_layout_parses_both_layouts() {
        // v2.0.17: the persisted `g29_pedal_layout` string selects
        // the map — case-insensitive, surrounding whitespace
        // ignored; anything else is rejected (no guessing).
        assert_eq!(
            G29PedalMap::from_layout("hidmaestro"),
            Some(G29PedalMap::PROFILE)
        );
        assert_eq!(
            G29PedalMap::from_layout("PCGamingWiki"),
            Some(G29PedalMap::ACCEL_CLUTCH_SWAPPED)
        );
        assert_eq!(
            G29PedalMap::from_layout("  pcgamingwiki  "),
            Some(G29PedalMap::ACCEL_CLUTCH_SWAPPED)
        );
        assert_eq!(G29PedalMap::from_layout(""), None);
        assert_eq!(G29PedalMap::from_layout("logitech"), None);
        assert_eq!(G29PedalMap::from_layout("hidmaestro2"), None);
    }

    #[test]
    fn g29_pedal_layout_string_drives_pedal_bytes() {
        // v2.0.17: the layout string alone decides which descriptor
        // byte carries the accelerator — one pedal pressed, so the
        // other two bytes must stay 0 (the all-pressed test above
        // cannot tell a swapped map from a duplicated one).
        let mut s = OutputState::neutral();
        s.accelerator = 1.0;
        let caps = HidMaestroCapabilities::g29_wheel();
        let mut out = [0u8; G29_RAW_REPORT_SIZE];
        let map = G29PedalMap::from_layout("hidmaestro").expect("hidmaestro parses");
        build_g29_raw_report(&s, &caps, map, G29_HANDBRAKE_BUTTON, &mut out);
        assert_eq!((out[6], out[7], out[8]), (255, 0, 0)); // Z = accel
        let map = G29PedalMap::from_layout("pcgamingwiki").expect("pcgamingwiki parses");
        build_g29_raw_report(&s, &caps, map, G29_HANDBRAKE_BUTTON, &mut out);
        assert_eq!((out[6], out[7], out[8]), (0, 0, 255)); // Y = accel
        // And the clutch goes to the other end in each layout.
        let mut s = OutputState::neutral();
        s.clutch = 1.0;
        let map = G29PedalMap::from_layout("hidmaestro").expect("hidmaestro parses");
        build_g29_raw_report(&s, &caps, map, G29_HANDBRAKE_BUTTON, &mut out);
        assert_eq!((out[6], out[7], out[8]), (0, 0, 255)); // Y = clutch
        let map = G29PedalMap::from_layout("pcgamingwiki").expect("pcgamingwiki parses");
        build_g29_raw_report(&s, &caps, map, G29_HANDBRAKE_BUTTON, &mut out);
        assert_eq!((out[6], out[7], out[8]), (255, 0, 0)); // Z = clutch
    }

    #[test]
    fn g29_handbrake_on_configured_button_does_not_touch_button_8() {
        // v2.0.17: the handbrake button is configurable (default 8);
        // on button 12 the handbrake must press 12 and leave 8
        // alone — the collision the setting exists to avoid.
        let caps = HidMaestroCapabilities::g29_wheel();
        let mut s = OutputState::neutral();
        s.handbrake = 1.0;
        let mut out = [0u8; G29_RAW_REPORT_SIZE];
        build_g29_raw_report(&s, &caps, G29PedalMap::PROFILE, 12, &mut out);
        assert!(is_g29_button_pressed(&out, 12));
        assert!(!is_g29_button_pressed(&out, 8));
        // A physical button 8 held at the same time still reports
        // on 8 (the two sources OR onto different bits now).
        s.buttons = 1u32 << 7; // physical button 8
        build_g29_raw_report(&s, &caps, G29PedalMap::PROFILE, 12, &mut out);
        assert!(is_g29_button_pressed(&out, 8));
        assert!(is_g29_button_pressed(&out, 12));
        // Default 8 behaviour unchanged: handbrake alone presses 8.
        let mut s = OutputState::neutral();
        s.handbrake = 1.0;
        build_g29_raw_report(
            &s,
            &caps,
            G29PedalMap::PROFILE,
            G29_HANDBRAKE_BUTTON,
            &mut out,
        );
        assert!(is_g29_button_pressed(&out, 8));
        assert!(!is_g29_button_pressed(&out, 12));
    }

    #[test]
    fn g29_handbrake_out_of_range_button_presses_nothing() {
        // v2.0.17: a handbrake button outside 1–25 has no wire slot
        // — it must press nothing, never wrap onto button 1 or 25.
        let caps = HidMaestroCapabilities::g29_wheel();
        let mut s = OutputState::neutral();
        s.handbrake = 1.0;
        for button in [0u8, 26, 255] {
            let mut out = [0u8; G29_RAW_REPORT_SIZE];
            build_g29_raw_report(&s, &caps, G29PedalMap::PROFILE, button, &mut out);
            let word = u32::from_le_bytes([out[0], out[1], out[2], out[3]]);
            assert_eq!(word, 0x08, "button {button} must press nothing");
        }
    }

    #[test]
    fn g29_caps_have_no_rx_no_slider() {
        let caps = HidMaestroCapabilities::g29_wheel();
        assert!(caps.has_axis_x && caps.has_hat);
        assert!(!caps.has_axis_slider);
        assert_eq!(caps.button_count, 25);
    }

    #[test]
    fn validate_g29_handbrake_needs_no_slider_but_other_axes_still_checked() {
        let plan = PipelinePlan::default();
        let caps = HidMaestroCapabilities::g29_wheel();
        // Full G29 caps + handbrake on: valid (button-mapped).
        assert_eq!(
            validate_capabilities_for_profile(&plan, &caps, true).len(),
            0
        );
        // Same caps under the custom profile: handbrake fails (no Slider).
        let errors = validate_capabilities(&plan, &caps);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("Handbrake"));
        // G29 with steering missing still fails for steering.
        let caps_no_x = HidMaestroCapabilities::new(false, true, true, true, false, 25, true);
        let errors = validate_capabilities_for_profile(&plan, &caps_no_x, true);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("Steering"));
    }
}
