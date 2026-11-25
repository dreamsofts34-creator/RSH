//! Per-module processing pipeline (SW-R3.3, D6 / D7 / D9 / D10 / D14).
//!
//! Per tick, per module: map (mV → normalised) → deadzone → curve →
//! One Euro smoothing → pedal ramp fusion → [`OutputState`].
//!
//! Axis slots are canonical (FEATURE-INVENTORY §1.4): 0 steering,
//! 1 accelerator, 2 brake, 3 clutch, 4 handbrake. All axes are
//! millivolts (D6). A disabled module emits nothing: its outputs stay
//! neutral and its filter/ramp state is reset so re-enabling it starts
//! clean.
//!
//! Steering scale is a **divide** (D7): `steer / scale`, so scale 0.4
//! with steer 0.4 is full lock. The result is clamped to `-1..=1`.
//!
//! The hot path is allocation-free: fixed-size arrays only, no `Vec`,
//! no `HashMap`. The pipeline assumes a 200 Hz tick
//! ([`INPUT_FREQ_HZ`]); it does not read frame timestamps.

use core::f32::consts::PI;

use rsh_protocol::stream::{MAX_ENCODERS, StreamFrame};

use crate::calibration::{
    DEFAULT_CENTER_MV, DEFAULT_MAX_MV, DEFAULT_MIN_MV, validate_pedal_range,
    validate_steering_range,
};
use crate::encoder::{EncoderButtons, EncoderTracker, deltas_to_buttons};

/// Input tick rate in Hz (5 ms tick, SW-R3.1).
pub const INPUT_FREQ_HZ: f32 = 200.0;
/// Seconds per tick at [`INPUT_FREQ_HZ`].
pub const DT_S: f32 = 1.0 / INPUT_FREQ_HZ;
/// Steering axis slot.
pub const STEERING_SLOT: usize = 0;
/// Accelerator axis slot.
pub const ACCEL_SLOT: usize = 1;
/// Brake axis slot.
pub const BRAKE_SLOT: usize = 2;
/// Clutch axis slot.
pub const CLUTCH_SLOT: usize = 3;
/// Handbrake axis slot.
pub const HANDBRAKE_SLOT: usize = 4;
/// Bitmask of the H-pattern gear bits 5–10 in the stream `buttons` word.
pub const GEAR_BITS_MASK: u32 = 0x7E0;
/// Stream `buttons` bit for the horn (BUTTON slot 2).
pub const HORN_BIT: u32 = 1 << 2;
/// Stream `buttons` bits for sequential shifter up/down (BUTTON slots 0/1).
pub const SHIFTER_SEQ_BITS: u32 = 0b11;
/// Stream `buttons` bits the BU1 debounce stage skips: 0/1
/// (sequential shifter) and 2 (horn) — the firmware already
/// debounces them (`inputs.cpp:9-10`, `125-127`, `516-530`), so a
/// second stage would only add latency to shifts and the horn.
pub const BUTTON_DEBOUNCE_EXCLUDE_MASK: u32 = SHIFTER_SEQ_BITS | HORN_BIT;

/// Shifter gate mode (per-module tuning, only meaningful for the
/// shifter module; other modules ignore it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ShifterGateMode {
    /// Up/down buttons (BUTTON slots 0/1).
    #[default]
    Sequential,
    /// Six gears on stream `buttons` bits 5–10.
    HPattern,
}

/// Per-module tuning: on/off plus the map/deadzone/curve/smoothing/
/// range parameters for one module.
///
/// Ranges are millivolts (D6). An insane range (span < `MIN_SPAN_MV`,
/// mis-ordered endpoints) is never applied as-is: the mapper falls
/// back to the default range (`300..=3300` mV, centre `1800` mV).
/// [`sanitized`](Self::sanitized) applies the same rules eagerly.
///
/// Five independent flags, one per module behaviour (`enabled`,
/// `invert`, encoder accel, handbrake latch + auto-release) —
/// grouping them would obscure the flat plan shape the pipeline
/// reads per module.
//
// The bool count is the module's independent switches (enabled /
// invert / accel / latch); grouping them would break the plan shape
// the state layer builds field-by-field, so the pedantic bool-count
// lint is allowed here.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModuleTuning {
    /// False = the module emits nothing (neutral output).
    pub enabled: bool,
    /// Deadzone, `0..=0.5` (fraction of full travel). Default `0.02`.
    pub deadzone: f32,
    /// Curve strength, `-1..=1` (0 = linear). Positive softens the
    /// centre (progressive), negative sharpens it.
    pub curve_strength: f32,
    /// One Euro smoothing, `0..=1` (0 = off). Default `0.3`.
    pub smoothing: f32,
    /// Range minimum in millivolts. Default `300`.
    pub min_mv: u16,
    /// Range maximum in millivolts. Default `3300`.
    pub max_mv: u16,
    /// Range centre in millivolts (steering only; pedals ignore it).
    /// Default `1800`.
    pub center_mv: u16,
    /// Invert the normalised value after mapping.
    pub invert: bool,
    /// Steering scale, `0.2..=1.0` (D7 divide). Default `1.0`.
    /// Kept so stored profiles keep their shape and the plan still
    /// mirrors the lock scale here — but steering no longer READS
    /// it: the steering chain divides by `scale_left` /
    /// `scale_right` instead (S2, per-module tuning).
    pub scale: f32,
    // --- Steering per-module tuning ---
    /// Steering left scale, `0.2..=1.0` (D7 divide). Default `1.0`.
    /// Only the steering module reads it (S2: the balance splits
    /// `scale` per side); every other module keeps it `1.0`.
    pub scale_left: f32,
    /// Steering right scale, `0.2..=1.0` (D7 divide). Default `1.0`.
    /// Only the steering module reads it (S2).
    pub scale_right: f32,
    /// Pedal press ramp in milliseconds (0 = no ramp, instant).
    /// Default `150`.
    pub press_ms: f32,
    /// Pedal release ramp in milliseconds (0 = no ramp, instant).
    /// Default `100`.
    pub release_ms: f32,
    /// Shifter gate mode (only the shifter module reads this).
    pub gate_mode: ShifterGateMode,
    // --- Shifter per-module tuning ---
    /// Shift lockout in ms (SH2, shifter only): new gear inputs
    /// within this long of the last accepted shift are ignored,
    /// `0..=200` (default `50`; `0` = off). Kills gate-bounce
    /// double-shifts in H-pattern and double-taps in sequential.
    pub shift_lockout_ms: f32,
    /// Clutch bite point, `0..=1` (clutch / brake only; `0.0` =
    /// none). The pedal output stays `0.0` until the mapped travel
    /// passes the bite point, then ramps `(value - bite) / (1 - bite)`
    /// — the clutch engages at the bite, it does not creep before it.
    pub bite_point: f32,
    // --- Brake per-module tuning ---
    /// Brake force (BRF), `0.5..=1.0`: the travel fraction that
    /// gives 100% brake (brake only; `1.0` = off, full travel).
    /// Applied right after mapping, before the bite point:
    /// `value / saturation`, clamped to `0..=1`. Calibration
    /// (`min_mv` / `max_mv`) is untouched.
    pub saturation: f32,
    // --- Encoder per-module tuning ---
    /// Pulse length in milliseconds (E1, encoders only): each step
    /// holds its virtual button (65–72) for this long, `20..=200`
    /// (default `50`), instead of exactly one 5 ms tick — games
    /// polling slower than 200 Hz no longer miss steps.
    pub pulse_ms: f32,
    /// Counts per click (E2, encoders only): quadrature counts per
    /// emitted step — `1`, `2` or `4` (default `4`: one physical
    /// detent = one step). Anything else sanitizes to `4`.
    pub counts_per_click: u8,
    /// Acceleration (E3, encoders only): fast spins emit extra steps.
    /// Default `false` (off).
    pub accel_enabled: bool,
    /// Acceleration sensitivity (E3, encoders only): extra steps as
    /// a multiple of the spun steps, `0..=2` (default `1.0` — a fast
    /// spin doubles its steps). Only read when `accel_enabled`.
    pub accel_sensitivity: f32,
    /// Per-encoder direction invert (E4, encoders only): bit `i`
    /// inverts encoder `i` (only bits `0..=3` are kept). OR-ed with
    /// the module-level `invert`, which inverts all encoders.
    pub encoder_invert_mask: u8,
    // --- Buttons per-module tuning ---
    /// Button debounce in milliseconds (BU1, buttons only): a
    /// press / release must hold steady this long before the engine
    /// reports it, `0..=50` (default `10`). `0` = pass-through.
    pub debounce_ms: f32,
    /// Long-press threshold in milliseconds (BU2, buttons only):
    /// how long a button must be held before its long-press action
    /// takes over, `300..=1500` (default `800`).
    pub long_press_ms: f32,
    /// Double-press window in milliseconds (BU3, buttons only):
    /// a second press that starts within this long of the previous
    /// release counts as a double-press, `150..=500` (default
    /// `300`).
    pub double_press_ms: f32,
    // --- Handbrake/clutch per-module tuning ---
    /// Latch mode (H1, handbrake only): a rising edge toggles a
    /// held handbrake instead of following the input. Default
    /// `false` (off — the output follows the input).
    pub handbrake_latch: bool,
    /// Auto-release on throttle (H3, handbrake only): with latch
    /// on, accelerator past 20% clears the latch (rally launch).
    /// Default `false` (off). Only read when `handbrake_latch`.
    pub handbrake_auto_release: bool,
    /// Engagement width / slip zone (C2, clutch only), `0.05..=0.40`
    /// as a fraction of travel (default `0.15`). `0.0` = no slip
    /// zone — the bite is the old hard stretch. Only the clutch
    /// plan carries a nonzero value; every other module keeps
    /// `0.0`, so their bite behaviour is unchanged.
    pub bite_width: f32,
    // --- LED/horn per-module tuning ---
    /// Minimum honk duration in milliseconds (HO1, horn only):
    /// a press holds the horn output for at least this long,
    /// `100..=1000` (default `400`), so a quick tap still
    /// registers in the game.
    pub horn_min_ms: f32,
    /// Honk mode (HO2, horn only): `true` = Latch (each press
    /// toggles the horn on/off), `false` = Momentary (default —
    /// the horn sounds only while the button is held, plus the
    /// [`Self::horn_min_ms`] hold).
    pub horn_latch: bool,
}

impl Default for ModuleTuning {
    fn default() -> Self {
        Self {
            enabled: true,
            deadzone: 0.02,
            curve_strength: 0.0,
            smoothing: 0.3,
            min_mv: DEFAULT_MIN_MV,
            max_mv: DEFAULT_MAX_MV,
            center_mv: DEFAULT_CENTER_MV,
            invert: false,
            scale: 1.0,
            // --- Steering per-module tuning ---
            scale_left: 1.0,
            scale_right: 1.0,
            press_ms: 150.0,
            release_ms: 100.0,
            gate_mode: ShifterGateMode::Sequential,
            // --- Shifter per-module tuning ---
            shift_lockout_ms: 50.0,
            bite_point: 0.0,
            // --- Brake per-module tuning ---
            saturation: 1.0,
            // --- Encoder per-module tuning ---
            pulse_ms: 50.0,
            counts_per_click: 4,
            accel_enabled: false,
            accel_sensitivity: 1.0,
            encoder_invert_mask: 0,
            // --- Buttons per-module tuning ---
            debounce_ms: 10.0,
            long_press_ms: 800.0,
            double_press_ms: 300.0,
            // --- Handbrake/clutch per-module tuning ---
            handbrake_latch: false,
            handbrake_auto_release: false,
            bite_width: 0.0,
            // --- LED/horn per-module tuning ---
            horn_min_ms: 400.0,
            horn_latch: false,
        }
    }
}

impl ModuleTuning {
    /// A copy with every field clamped to its documented range
    /// (deadzone `0..=0.5`, curve `-1..=1`, smoothing `0..=1`, scale
    /// `0.2..=1.0` with NaN → `1.0`, ramp `10..=2000` ms with 0 kept
    /// as "off"). An insane mV range is replaced by the default
    /// range; a centre outside the range is reset to its midpoint.
    #[must_use]
    pub fn sanitized(&self) -> Self {
        let mut out = *self;
        out.deadzone = clamp_finite(self.deadzone, 0.0, 0.5, 0.02);
        out.curve_strength = clamp_finite(self.curve_strength, -1.0, 1.0, 0.0);
        out.smoothing = clamp_finite(self.smoothing, 0.0, 1.0, 0.3);
        out.scale = clamp_finite(self.scale, 0.2, 1.0, 1.0);
        // --- Steering per-module tuning ---
        out.scale_left = clamp_finite(self.scale_left, 0.2, 1.0, 1.0);
        out.scale_right = clamp_finite(self.scale_right, 0.2, 1.0, 1.0);
        out.press_ms = if self.press_ms <= 0.0 {
            0.0
        } else {
            clamp_finite(self.press_ms, 10.0, 2000.0, 150.0)
        };
        out.release_ms = if self.release_ms <= 0.0 {
            0.0
        } else {
            clamp_finite(self.release_ms, 10.0, 2000.0, 100.0)
        };
        out.bite_point = clamp_finite(self.bite_point, 0.0, 1.0, 0.0);
        // --- Shifter per-module tuning ---
        out.shift_lockout_ms = clamp_finite(self.shift_lockout_ms, 0.0, 200.0, 50.0);
        // --- Brake per-module tuning ---
        out.saturation = clamp_finite(self.saturation, 0.5, 1.0, 1.0);
        // --- Encoder per-module tuning ---
        out.pulse_ms = clamp_finite(self.pulse_ms, 20.0, 200.0, 50.0);
        out.counts_per_click = match self.counts_per_click {
            1 => 1,
            2 => 2,
            _ => 4,
        };
        out.accel_sensitivity = clamp_finite(self.accel_sensitivity, 0.0, 2.0, 1.0);
        out.encoder_invert_mask = self.encoder_invert_mask & 0x0F;
        // --- Buttons per-module tuning ---
        out.debounce_ms = clamp_finite(self.debounce_ms, 0.0, 50.0, 10.0);
        out.long_press_ms = clamp_finite(self.long_press_ms, 300.0, 1500.0, 800.0);
        out.double_press_ms = clamp_finite(self.double_press_ms, 150.0, 500.0, 300.0);
        // --- Handbrake/clutch per-module tuning ---
        // A slip width of 0 (or less) is "off" — the hard stretch
        // — so it is kept, not clamped up to 0.05.
        out.bite_width = if self.bite_width <= 0.0 {
            0.0
        } else {
            clamp_finite(self.bite_width, 0.05, 0.40, 0.0)
        };
        // --- LED/horn per-module tuning ---
        out.horn_min_ms = clamp_finite(self.horn_min_ms, 100.0, 1000.0, 400.0);
        if validate_pedal_range(self.min_mv, self.max_mv).is_err() {
            out.min_mv = DEFAULT_MIN_MV;
            out.max_mv = DEFAULT_MAX_MV;
            // The range was replaced, so the old centre no longer
            // belongs to it: reset to the new range's midpoint (the
            // mapper's fallback uses the default centre 1800, which
            // is exactly this midpoint).
            out.center_mv = out.min_mv + (out.max_mv - out.min_mv) / 2;
        } else if !(out.min_mv < self.center_mv && self.center_mv < out.max_mv) {
            out.center_mv = out.min_mv + (out.max_mv - out.min_mv) / 2;
        }
        out
    }
}

/// The per-module plan for the pipeline: one tuning per data module
/// plus the combined-pedals switch.
///
/// The clutch module defaults to **disabled** (factory config has no
/// clutch axis; it is enabled only when the config blob carries an
/// `AXIS_ADC` slot-3 record). All other modules default to enabled.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PipelinePlan {
    /// Steering tuning (axis slot 0).
    pub steering: ModuleTuning,
    /// Accelerator tuning (axis slot 1).
    pub accelerator: ModuleTuning,
    /// Brake tuning (axis slot 2).
    pub brake: ModuleTuning,
    /// Clutch tuning (axis slot 3). Disabled by default.
    pub clutch: ModuleTuning,
    /// Handbrake tuning (axis slot 4, `WSlider` output).
    pub handbrake: ModuleTuning,
    /// Shifter tuning (buttons / gear decode).
    pub shifter: ModuleTuning,
    /// Horn tuning (stream `buttons` bit 2).
    pub horn: ModuleTuning,
    /// Wheel-buttons tuning (stream `buttons` word).
    pub buttons: ModuleTuning,
    /// Encoders tuning (cumulative i16 counts → buttons 65–72).
    pub encoders: ModuleTuning,
    /// True = accelerator and brake share one axis
    /// ([`OutputState::pedals_combined`]).
    pub combine_pedals: bool,
}

impl PipelinePlan {
    /// A copy with every module tuning sanitized (L2: the single
    /// source of truth — [`ProcessingPipeline::new`] and
    /// [`ProcessingPipeline::set_plan`] both apply this, so the
    /// pipeline never runs an unsanitized plan and `process` never
    /// has to re-sanitize per frame).
    #[must_use]
    pub fn sanitized(&self) -> Self {
        Self {
            steering: self.steering.sanitized(),
            accelerator: self.accelerator.sanitized(),
            brake: self.brake.sanitized(),
            clutch: self.clutch.sanitized(),
            handbrake: self.handbrake.sanitized(),
            shifter: self.shifter.sanitized(),
            horn: self.horn.sanitized(),
            buttons: self.buttons.sanitized(),
            encoders: self.encoders.sanitized(),
            combine_pedals: self.combine_pedals,
        }
    }
}

impl Default for PipelinePlan {
    fn default() -> Self {
        Self {
            steering: ModuleTuning::default(),
            accelerator: ModuleTuning::default(),
            brake: ModuleTuning::default(),
            clutch: ModuleTuning {
                enabled: false,
                ..ModuleTuning::default()
            },
            handbrake: ModuleTuning::default(),
            shifter: ModuleTuning::default(),
            horn: ModuleTuning::default(),
            buttons: ModuleTuning::default(),
            encoders: ModuleTuning::default(),
            combine_pedals: false,
        }
    }
}

/// The processed output of one tick. Consumed by the B1 engine worker
/// and the output backends (`vJoy` / `HIDMaestro`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OutputState {
    /// Steering, `-1..=1` (-1 = full left, +1 = full right).
    pub steering: f32,
    /// Accelerator, `0..=1` (0 = released, 1 = fully pressed).
    pub accelerator: f32,
    /// Brake, `0..=1` (0 = released, 1 = fully pressed).
    pub brake: f32,
    /// Clutch, `0..=1` (0 = released, 1 = fully pressed).
    pub clutch: f32,
    /// Handbrake slider, `0..=1` (`WSlider` axis, HID usage `0x36`).
    pub handbrake: f32,
    /// Combined pedals, `-1..=1` (accelerator minus brake). Only
    /// meaningful when [`PipelinePlan::combine_pedals`] is true;
    /// otherwise `0.0`.
    pub pedals_combined: f32,
    /// Wheel buttons 0–31, bit *n* = button *n*. H-pattern gear bits
    /// 5–10 are stripped (see [`gear`](Self::gear)); the horn bit is
    /// cleared when the horn module is disabled; `0` when the buttons
    /// module is disabled.
    pub buttons: u32,
    /// Wheel buttons currently held past the long-press threshold
    /// (BU2): bit *n* is set while button *n* has been held (after
    /// the BU1 debounce) for at least the buttons module's
    /// `long_press_ms`. The bit clears the tick the button is
    /// released. `0` when the buttons module is disabled. The
    /// daemon's binding layer reads this to switch a button to its
    /// long-press action; the output backends never see it directly.
    pub long_press_buttons: u32,
    /// Wheel buttons currently held as a double-press (BU3): bit
    /// *n* is set while button *n* is held on a press that started
    /// within the buttons module's `double_press_ms` of the
    /// previous release of that button. The bit clears the tick the
    /// button is released. `0` when the buttons module is disabled.
    /// The daemon's binding layer reads this to switch the button
    /// to its long-press (2nd) action — the same action BU2 uses, so
    /// a double-press and a long hold both mean "2nd action".
    pub double_press_buttons: u32,
    /// Wheel buttons 32–63. Always `0` from the pipeline: the stream
    /// packet carries only buttons 0–31, and `btnsEx` arrives via the
    /// status packet, which the engine layer merges — not the pipeline.
    pub buttons_ex: u32,
    /// Momentary encoder-button pulses, buttons 65–72 (see
    /// [`EncoderButtons`]). Each step holds its bit for the
    /// encoders module's `pulse_ms` (E1, default 50 ms) instead of
    /// exactly one 5 ms tick. `0` when the encoders module is disabled.
    pub encoder_buttons: EncoderButtons,
    /// Per-encoder effective step deltas this tick (index `i` =
    /// encoder `i`): the glitch-filtered counts after per-encoder
    /// invert and `counts_per_click` division (E2), so they always
    /// agree with [`Self::encoder_buttons`]. `0` when the encoders
    /// module is disabled.
    pub encoder_deltas: [i16; MAX_ENCODERS],
    /// Decoded H-pattern gear: `0` = neutral, `1..=6` = gear. Always
    /// `0` in sequential mode or when the shifter module is disabled.
    pub gear: u8,
    /// True when the horn button (stream `buttons` bit 2) is pressed
    /// and the horn module is enabled.
    pub horn: bool,
}

impl OutputState {
    /// All-neutral state: every axis at rest, no buttons, no gear.
    /// This is what the watchdog emits when data goes stale.
    #[must_use]
    pub const fn neutral() -> Self {
        Self {
            steering: 0.0,
            accelerator: 0.0,
            brake: 0.0,
            clutch: 0.0,
            handbrake: 0.0,
            pedals_combined: 0.0,
            buttons: 0,
            long_press_buttons: 0,
            double_press_buttons: 0,
            buttons_ex: 0,
            encoder_buttons: EncoderButtons::none(),
            encoder_deltas: [0; MAX_ENCODERS],
            gear: 0,
            horn: false,
        }
    }
}

/// One Euro filter (standard algorithm, SW-R3.5).
///
/// Adaptive low-pass: the cutoff rises with the signal's speed
/// (`min_cutoff + beta * |filtered derivative|`), so slow moves are
/// smoothed heavily and fast moves pass through. `freq_hz` is the
/// input rate; a non-positive or non-finite frequency disables the
/// filter (pass-through) instead of dividing by zero (SW-R3.5 edge
/// guard). The first sample always passes through unfiltered.
#[derive(Clone, Copy, Debug)]
pub struct OneEuroFilter {
    /// Seconds per sample (`1 / freq_hz`).
    dt: f32,
    /// Base cutoff in Hz.
    min_cutoff: f32,
    /// Cutoff slope per unit of signal speed.
    beta: f32,
    /// Cutoff of the derivative low-pass in Hz.
    d_cutoff: f32,
    /// Previous filtered value.
    x_prev: f32,
    /// Previous filtered derivative.
    dx_prev: f32,
    /// True until the first sample arrives.
    first: bool,
    /// True = pass-through (invalid frequency or smoothing off).
    bypass: bool,
}

impl OneEuroFilter {
    /// Build a filter. An invalid frequency (`<= 0` or non-finite)
    /// yields a pass-through filter, never a divide-by-zero.
    /// Non-finite / non-positive cutoffs fall back to `1.0` Hz
    /// (`min_cutoff`, `d_cutoff`) and `0.0` (`beta`).
    #[must_use]
    pub fn new(freq_hz: f32, min_cutoff: f32, beta: f32, d_cutoff: f32) -> Self {
        let bypass = !(freq_hz.is_finite() && freq_hz > 0.0);
        let dt = if bypass { DT_S } else { 1.0 / freq_hz };
        Self {
            dt,
            min_cutoff: sanitize_cutoff(min_cutoff),
            beta: if beta.is_finite() && beta >= 0.0 {
                beta
            } else {
                0.0
            },
            d_cutoff: sanitize_cutoff(d_cutoff),
            x_prev: 0.0,
            dx_prev: 0.0,
            first: true,
            bypass,
        }
    }

    /// Build a filter from a `0..=1` smoothing amount (the
    /// per-module `smoothing` tuning value). `0` (or less) disables
    /// the filter (pass-through); `1` is the heaviest smoothing
    /// (`min_cutoff` `0.5` Hz); the default `0.3` maps to `7.15` Hz.
    /// `beta` is `0.5`, `d_cutoff` is `1.0` Hz.
    #[must_use]
    pub fn from_smoothing(smoothing: f32, freq_hz: f32) -> Self {
        if !(smoothing.is_finite() && smoothing > 0.0) {
            let mut filter = Self::new(freq_hz, 1.0, 0.5, 1.0);
            filter.bypass = true;
            return filter;
        }
        let s = smoothing.clamp(0.0, 1.0);
        Self::new(freq_hz, 0.5 + 9.5 * (1.0 - s), 0.5, 1.0)
    }

    /// True when this filter passes input through unchanged.
    #[must_use]
    pub const fn is_bypass(&self) -> bool {
        self.bypass
    }

    /// Enable or disable filtering. Disabling resets the state, so
    /// re-enabling starts clean from the next sample.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.bypass = !enabled;
        if !enabled {
            self.first = true;
        }
    }

    /// Replace the base cutoff. Invalid values (`<= 0`, non-finite)
    /// are ignored and the previous cutoff is kept.
    pub fn set_min_cutoff(&mut self, min_cutoff: f32) {
        if min_cutoff.is_finite() && min_cutoff > 0.0 {
            self.min_cutoff = min_cutoff;
        }
    }

    /// Configure this filter from a `0..=1` smoothing amount (the
    /// per-module `smoothing` tuning value), the same mapping as
    /// [`from_smoothing`](Self::from_smoothing). Called when the
    /// plan is set ([`ProcessingPipeline::set_plan_live`]), so the
    /// per-tick [`apply_smoothing`] does not have to re-derive the
    /// cutoff on every tick for a value that only changes on a plan
    /// set (bench regression: the per-tick reconfiguration on all
    /// five axes was pure overhead). `0` (or less / non-finite)
    /// disables the filter (pass-through, state reset).
    pub fn configure(&mut self, smoothing: f32) {
        if !(smoothing.is_finite() && smoothing > 0.0) {
            self.set_enabled(false);
            return;
        }
        self.set_enabled(true);
        self.set_min_cutoff(0.5 + 9.5 * (1.0 - smoothing.clamp(0.0, 1.0)));
    }

    /// Forget all state; the next sample passes through unfiltered.
    pub fn reset(&mut self) {
        self.x_prev = 0.0;
        self.dx_prev = 0.0;
        self.first = true;
    }

    /// Filter one sample. Non-finite input is returned unchanged and
    /// never enters the filter state (degenerate input cannot crash
    /// or poison the filter).
    pub fn filter(&mut self, x: f32) -> f32 {
        if self.bypass || !x.is_finite() {
            return x;
        }
        if self.first {
            self.x_prev = x;
            self.dx_prev = 0.0;
            self.first = false;
            return x;
        }
        let dx = (x - self.x_prev) / self.dt;
        let a_d = alpha(self.d_cutoff, self.dt);
        let dx_hat = a_d.mul_add(dx, (1.0 - a_d) * self.dx_prev);
        let cutoff = self.min_cutoff + self.beta * dx_hat.abs();
        let a = alpha(cutoff, self.dt);
        let x_hat = a.mul_add(x, (1.0 - a) * self.x_prev);
        self.x_prev = x_hat;
        self.dx_prev = dx_hat;
        x_hat
    }
}

/// Slew-rate limiter for pedal ramps (press/release fusion).
///
/// Limits how fast the value may rise (`press_ms`) and fall
/// (`release_ms`). A limit of `0` (or less) means unlimited in that
/// direction. The first sample is taken as-is (no ramp from zero).
#[derive(Clone, Copy, Debug, Default)]
pub struct RampLimiter {
    /// Current limited value.
    value: f32,
    /// True once the first sample was taken.
    initialized: bool,
}

impl RampLimiter {
    /// An uninitialized limiter (first sample passes through).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            value: 0.0,
            initialized: false,
        }
    }

    /// Current limited value (`0.0` before the first sample).
    #[must_use]
    pub const fn value(&self) -> f32 {
        self.value
    }

    /// Step toward `target`, limited by the press/release ramps.
    /// `dt` is seconds per tick; `press_ms` / `release_ms` are the
    /// full-travel times in milliseconds (0 = unlimited).
    ///
    /// NOTE: hoisting the `max_step` division out of this fn (cached
    /// per-tick steps configured at plan time, like the filter cutoff
    /// and the encoder pulse ticks) was tried and measured slightly
    /// SLOWER (234–240 ns vs 229.5 ns `pipeline_tick`, two runs), so
    /// the per-tick division stays — do not re-hoist without new
    /// measurements.
    pub fn update(&mut self, target: f32, dt: f32, press_ms: f32, release_ms: f32) -> f32 {
        if !self.initialized {
            self.value = target;
            self.initialized = true;
            return self.value;
        }
        let delta = target - self.value;
        if delta > 0.0 {
            let max_rise = max_step(dt, press_ms);
            self.value += delta.min(max_rise);
        } else if delta < 0.0 {
            let max_fall = max_step(dt, release_ms);
            self.value += delta.max(-max_fall);
        }
        self.value
    }

    /// Forget all state; the next sample passes through.
    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

/// The per-module processing pipeline. Owns the plan plus the
/// per-axis filter, ramp, and encoder state. Allocation-free in the
/// hot path: [`process`](Self::process) uses fixed-size arrays only.
//
// The struct holds exactly 4 bools: two (state, prev-state)
// edge-detection pairs (`handbrake_latched` / `handbrake_prev_pressed`
// and `horn_latch_on` / `horn_prev_pressed`) — private internal runtime
// state, not config. All are built in the single construction site
// [`ProcessingPipeline::new`] (all false), so the lint's confusion
// concern does not apply; grouping them would only churn the 200 Hz
// hot path.
#[allow(clippy::struct_excessive_bools)]
pub struct ProcessingPipeline {
    /// Active per-module plan.
    plan: PipelinePlan,
    /// One Euro filters, one per analog axis.
    steering_filter: OneEuroFilter,
    /// One Euro filter for the accelerator.
    accel_filter: OneEuroFilter,
    /// One Euro filter for the brake.
    brake_filter: OneEuroFilter,
    /// One Euro filter for the clutch.
    clutch_filter: OneEuroFilter,
    /// One Euro filter for the handbrake.
    handbrake_filter: OneEuroFilter,
    /// Press/release ramp for the accelerator.
    accel_ramp: RampLimiter,
    /// Press/release ramp for the brake.
    brake_ramp: RampLimiter,
    /// Press/release ramp for the clutch.
    clutch_ramp: RampLimiter,
    /// Press/release ramp for the handbrake.
    handbrake_ramp: RampLimiter,
    /// Cumulative encoder counts (wrap-aware deltas).
    encoders: EncoderTracker,
    /// E1 pulse hold: ticks left (incl. the current tick) each
    /// virtual-button bit 0..=7 stays held after its step.
    encoder_pulse: [u8; MAX_ENCODERS * 2],
    /// E1 mask of virtual-button bits with a nonzero pulse hold
    /// (`encoder_pulse` bit > 0): [`apply_encoder_pulse`] only
    /// visits held or freshly-stepped bits instead of all 8 every
    /// tick. Kept in sync with `encoder_pulse` wherever that
    /// array is written (reset / disabled clear / the pulse
    /// itself).
    encoder_pulse_mask: u8,
    /// E1 pulse hold in ticks, derived from the encoders module's
    /// `pulse_ms` when the plan is set ([`encoder_pulse_ticks`]):
    /// the float division runs once per plan change, not once per
    /// tick in [`apply_encoder_pulse`](Self::apply_encoder_pulse).
    encoder_pulse_ticks: u8,
    /// E2 sub-click remainder: counts banked per encoder toward the
    /// next whole step (`counts_per_click` counts = one step).
    encoder_remainder: [i32; MAX_ENCODERS],
    /// BU1 debounced wheel-button word (bits 0/1/2 excluded — see
    /// [`BUTTON_DEBOUNCE_EXCLUDE_MASK`]).
    button_stable: u32,
    /// BU1 candidate word: the latest raw value a bit disagreed
    /// with `button_stable` toward.
    button_candidate: u32,
    /// BU1 ticks each bit's candidate has held steady (bit index =
    /// stream button bit).
    button_candidate_ticks: [u8; 32],
    /// BU2 ticks each debounced button has been held continuously
    /// (bit index = stream button bit).
    button_hold_ticks: [u16; 32],
    /// BU2 mask of buttons with a nonzero hold count (bit index =
    /// stream button bit): the set-bit iteration in
    /// [`track_button_holds`](Self::track_button_holds) only visits
    /// pressed or held buttons instead of all 32 every tick. Kept
    /// in sync with `button_hold_ticks` wherever that array is
    /// written (reset / disabled clear / the tracker itself).
    button_held_mask: u32,
    /// BU3 tick counter: incremented once per `process_buttons`
    /// call (one 5 ms tick at 200 Hz). Used only to measure the
    /// gap between a release and the next press.
    button_tick: u32,
    /// BU3 tick each button was last released (bit index = stream
    /// button bit). `0` = never released since the pipeline started
    /// (so the first press is never a double-press).
    button_last_release_tick: [u32; 32],
    /// BU3 debounced button word from the previous tick (for edge
    /// detection).
    button_prev_buttons: u32,
    /// BU3 mask of buttons currently held as a double-press (a
    /// press that started within `double_press_ms` of the previous
    /// release). Clears per-bit on release.
    button_double_hold: u32,
    /// SH2 last accepted H-pattern gear (0 = neutral): the lockout
    /// holds the output at this gear while the window is open.
    shift_last_gear: u8,
    /// SH2 lockout ticks left (incl. the current tick's countdown):
    /// a new gear / up-down edge is ignored while this is nonzero.
    /// One tick is 5 ms at 200 Hz, like the encoder pulse (E1).
    shift_lockout_left: u8,
    /// SH2 previous sequential up/down bits (bits 0/1): a shift is
    /// a rising edge against these, so a press held through the
    /// window emits nothing more.
    shift_prev_seq: u32,
    /// SH2 rejected sequential presses (bits 0/1): a press whose
    /// edge landed inside the window stays suppressed while held —
    /// otherwise the held bit would leak through on the very next
    /// tick (no new edge) and the double-tap would land anyway.
    /// Releasing the press clears its suppression.
    shift_suppressed_seq: u32,
    // --- Handbrake/clutch per-module tuning ---
    /// H1 latch state: true while a latched handbrake press is
    /// held on (output forced to 1.0) after the input was released.
    handbrake_latched: bool,
    /// Previous tick's pressed state (processed handbrake value
    /// above 0.5): a rising edge toggles the latch (H1).
    handbrake_prev_pressed: bool,
    /// P2-1 hold-last-good: the last axis outputs from a frame
    /// WITHOUT `axis_invalid` set, in slot order (steering,
    /// accelerator, brake, clutch, handbrake — handbrake is the
    /// post-latch output). While the flag is set the pipeline
    /// re-emits these instead of driving the outputs from the
    /// untrusted sample (a rail-stuck axis would otherwise drive
    /// full deflection). Buttons/encoders keep processing the
    /// live frame — only the axes hold.
    last_good_axes: [f32; 5],
    /// HO1 hold: ticks left the horn output stays on after the
    /// button is released (re-armed to the full minimum on every
    /// pressed tick, so a tap honks for at least `horn_min_ms`).
    horn_hold_remaining: u16,
    /// HO2 latch: true while a latched honk is on (toggled on each
    /// rising edge while `horn_latch` is set).
    horn_latch_on: bool,
    /// Previous tick's raw horn bit (rising-edge detection for the
    /// HO1 hold and the HO2 latch toggle).
    horn_prev_pressed: bool,
}

impl Default for ProcessingPipeline {
    fn default() -> Self {
        Self::new(PipelinePlan::default())
    }
}

impl ProcessingPipeline {
    /// Build a pipeline from a plan. Filters are built from each
    /// module's `smoothing` value; ramps and encoders start empty.
    /// The plan is sanitized on entry (L2), so an insane tuning can
    /// never reach the mappers.
    #[must_use]
    pub fn new(plan: PipelinePlan) -> Self {
        let plan = plan.sanitized();
        Self {
            steering_filter: OneEuroFilter::from_smoothing(plan.steering.smoothing, INPUT_FREQ_HZ),
            accel_filter: OneEuroFilter::from_smoothing(plan.accelerator.smoothing, INPUT_FREQ_HZ),
            brake_filter: OneEuroFilter::from_smoothing(plan.brake.smoothing, INPUT_FREQ_HZ),
            clutch_filter: OneEuroFilter::from_smoothing(plan.clutch.smoothing, INPUT_FREQ_HZ),
            handbrake_filter: OneEuroFilter::from_smoothing(
                plan.handbrake.smoothing,
                INPUT_FREQ_HZ,
            ),
            accel_ramp: RampLimiter::new(),
            brake_ramp: RampLimiter::new(),
            clutch_ramp: RampLimiter::new(),
            handbrake_ramp: RampLimiter::new(),
            encoders: EncoderTracker::new(),
            encoder_pulse: [0; MAX_ENCODERS * 2],
            encoder_pulse_mask: 0,
            encoder_pulse_ticks: encoder_pulse_ticks(plan.encoders.pulse_ms),
            encoder_remainder: [0; MAX_ENCODERS],
            // --- Buttons per-module tuning ---
            button_stable: 0,
            button_candidate: 0,
            button_candidate_ticks: [0; 32],
            button_hold_ticks: [0; 32],
            button_held_mask: 0,
            button_tick: 0,
            button_last_release_tick: [0; 32],
            button_prev_buttons: 0,
            button_double_hold: 0,
            // --- Shifter per-module tuning ---
            shift_last_gear: 0,
            shift_lockout_left: 0,
            shift_prev_seq: 0,
            shift_suppressed_seq: 0,
            // --- Handbrake/clutch per-module tuning ---
            handbrake_latched: false,
            handbrake_prev_pressed: false,
            last_good_axes: [0.0; 5],
            // --- LED/horn per-module tuning ---
            horn_hold_remaining: 0,
            horn_latch_on: false,
            horn_prev_pressed: false,
            plan,
        }
    }

    /// The active plan.
    #[must_use]
    pub const fn plan(&self) -> &PipelinePlan {
        &self.plan
    }

    /// Encoder glitch jumps dropped by the filter so far (M4).
    #[must_use]
    pub const fn encoder_glitch_dropped(&self) -> u64 {
        self.encoders.glitch_dropped()
    }

    /// Replace the plan. All filter, ramp, and encoder state is reset:
    /// a new plan must never inherit stale smoothing or a stale
    /// encoder baseline.
    pub fn set_plan(&mut self, plan: PipelinePlan) {
        *self = Self::new(plan);
    }

    /// Replace the plan for a live tuning change (L4 pipeline push),
    /// WITHOUT resetting filter, ramp, or encoder state: a slider drag
    /// must take effect on the next tick, not restart the smoothing
    /// and ramps (which would step the output). The plan is still
    /// sanitized on entry. Filter cutoffs track the new smoothing
    /// here ([`OneEuroFilter::configure`]), so kept state stays
    /// valid and the per-tick path does not re-derive them.
    /// Profile switches and engine start use [`Self::set_plan`]
    /// (full reset) instead.
    pub fn set_plan_live(&mut self, plan: PipelinePlan) {
        self.plan = plan.sanitized();
        self.encoder_pulse_ticks = encoder_pulse_ticks(self.plan.encoders.pulse_ms);
        self.steering_filter.configure(self.plan.steering.smoothing);
        self.accel_filter.configure(self.plan.accelerator.smoothing);
        self.brake_filter.configure(self.plan.brake.smoothing);
        self.clutch_filter.configure(self.plan.clutch.smoothing);
        self.handbrake_filter
            .configure(self.plan.handbrake.smoothing);
    }

    /// Forget all filter, ramp, and encoder state (same plan).
    pub fn reset(&mut self) {
        self.steering_filter.reset();
        self.accel_filter.reset();
        self.brake_filter.reset();
        self.clutch_filter.reset();
        self.handbrake_filter.reset();
        self.accel_ramp.reset();
        self.brake_ramp.reset();
        self.clutch_ramp.reset();
        self.handbrake_ramp.reset();
        self.encoders.reset();
        self.encoder_pulse = [0; MAX_ENCODERS * 2];
        self.encoder_pulse_mask = 0;
        self.encoder_remainder = [0; MAX_ENCODERS];
        // --- Buttons per-module tuning ---
        self.button_stable = 0;
        self.button_candidate = 0;
        self.button_candidate_ticks = [0; 32];
        self.button_hold_ticks = [0; 32];
        self.button_held_mask = 0;
        self.button_last_release_tick = [0; 32];
        self.button_prev_buttons = 0;
        self.button_double_hold = 0;
        // --- Shifter per-module tuning ---
        self.shift_last_gear = 0;
        self.shift_lockout_left = 0;
        self.shift_prev_seq = 0;
        self.shift_suppressed_seq = 0;
        // --- Handbrake/clutch per-module tuning ---
        self.handbrake_latched = false;
        self.handbrake_prev_pressed = false;
        self.last_good_axes = [0.0; 5];
        // --- LED/horn per-module tuning ---
        // A held/latched honk must not survive a reset (or a
        // re-enable) — the next press starts from silence.
        self.horn_hold_remaining = 0;
        self.horn_latch_on = false;
        self.horn_prev_pressed = false;
    }

    /// Process one stream frame into an [`OutputState`].
    ///
    /// Stage order per module: map → deadzone → curve → (steering:
    /// divide by scale) → One Euro smoothing → (pedals/handbrake:
    /// ramp fusion). A disabled module's outputs stay neutral and its
    /// state is reset. A missing axis (`None`) is neutral. An insane
    /// calibration range falls back to the default range.
    ///
    /// D14 (button-mode analog skip): an axis the board does not
    /// sample arrives as `None` and is neutral here — there is no
    /// analog value to skip.
    #[must_use]
    pub fn process(&mut self, frame: &StreamFrame) -> OutputState {
        self.process_with_gap(frame, 0)
    }

    /// Process one stream frame with the sequence gap since the
    /// previous frame (`skipped` stream frames were lost in between).
    /// The gap scales the encoder glitch filter (M4) so legitimate
    /// motion during the gap is not dropped as a glitch.
    ///
    /// P2-1: when `frame.flags.axis_invalid` is set, the axis
    /// sample is untrusted (the board flags a rail-stuck / failed
    /// axis, whose value sits at full deflection) — the axes hold
    /// their last good outputs instead of being driven from it.
    /// Filters/ramps are not fed the suspect sample, so resuming
    /// after the flag clears continues from the held state. Buttons
    /// and encoders still process the live frame (the flag is about
    /// axes only). Before any good frame the hold is neutral.
    #[must_use]
    pub fn process_with_gap(&mut self, frame: &StreamFrame, skipped: u16) -> OutputState {
        let mut out = OutputState::neutral();

        // NOTE (bench-gate fix attempt): reading the tunings by
        // reference instead of copying them was measured SLOWER
        // (+21 ns/tick on criterion) — the local copies scalarize
        // into registers, while the reference re-loads tuning
        // fields from memory on every access. Copies stay.
        if frame.flags.axis_invalid {
            // P2-1 hold-last-good (see the doc comment above).
            out.steering = self.last_good_axes[0];
            out.accelerator = self.last_good_axes[1];
            out.brake = self.last_good_axes[2];
            out.clutch = self.last_good_axes[3];
            out.handbrake = self.last_good_axes[4];
        } else {
            let steering_tuning = self.plan.steering;
            out.steering = process_steering_axis(
                frame.axes_mv[STEERING_SLOT],
                &steering_tuning,
                &mut self.steering_filter,
            );
            let accel_tuning = self.plan.accelerator;
            out.accelerator = process_pedal_axis(
                frame.axes_mv[ACCEL_SLOT],
                &accel_tuning,
                &mut self.accel_filter,
                &mut self.accel_ramp,
            )
            .0;
            let brake_tuning = self.plan.brake;
            out.brake = process_pedal_axis(
                frame.axes_mv[BRAKE_SLOT],
                &brake_tuning,
                &mut self.brake_filter,
                &mut self.brake_ramp,
            )
            .0;
            let clutch_tuning = self.plan.clutch;
            out.clutch = process_pedal_axis(
                frame.axes_mv[CLUTCH_SLOT],
                &clutch_tuning,
                &mut self.clutch_filter,
                &mut self.clutch_ramp,
            )
            .0;
            let handbrake_tuning = self.plan.handbrake;
            let (handbrake_value, handbrake_pressed) = process_pedal_axis(
                frame.axes_mv[HANDBRAKE_SLOT],
                &handbrake_tuning,
                &mut self.handbrake_filter,
                &mut self.handbrake_ramp,
            );
            // H1/H3: the latch holds the output at full after the input
            // is released (and auto-release drops it on throttle).
            // The accelerator is already processed above, so the
            // 20% throttle test reads this tick's value.
            out.handbrake = self.apply_handbrake_latch(
                handbrake_value,
                handbrake_pressed,
                out.accelerator,
                frame.axes_mv[HANDBRAKE_SLOT].is_some(),
            );
            self.last_good_axes = [
                out.steering,
                out.accelerator,
                out.brake,
                out.clutch,
                out.handbrake,
            ];
        }

        if self.plan.combine_pedals {
            out.pedals_combined = (out.accelerator - out.brake).clamp(-1.0, 1.0);
        }

        self.process_buttons(frame, skipped, &mut out);
        out
    }

    /// Buttons, gear decode, horn, and encoders.
    ///
    /// Never inlined into `process_with_gap`: the button/encoder
    /// block is large, and inlining it balloons the axis hot path
    /// (register pressure / icache) — measured on criterion.
    #[inline(never)]
    fn process_buttons(&mut self, frame: &StreamFrame, skipped: u16, out: &mut OutputState) {
        let mut buttons = frame.buttons;

        // Shifter: H-pattern gear decode (bits 5–10) or sequential
        // up/down (bits 0/1). Gear bits never reach the button word.
        // SH2: the lockout ignores a new gear / up-down edge within
        // `shift_lockout_ms` of the last accepted shift (gate-bounce
        // double-shifts, sequential double-taps). The window counts
        // in 5 ms ticks like the encoder pulse (E1) and ticks down
        // once per processed frame — a gap (`skipped`) does not
        // fast-forward it, same as the pulse. A gear still engaged
        // after the window ends is accepted on the next tick, so
        // only transient bounces are killed, never a real shift.
        match self.plan.shifter.gate_mode {
            ShifterGateMode::HPattern => {
                if self.plan.shifter.enabled {
                    let gear = decode_gear(buttons);
                    out.gear = self.apply_shift_lockout_h(gear);
                } else {
                    out.gear = 0;
                    self.shift_last_gear = 0;
                    self.shift_lockout_left = 0;
                }
                buttons &= !GEAR_BITS_MASK;
            }
            ShifterGateMode::Sequential => {
                if self.plan.shifter.enabled {
                    buttons = self.apply_shift_lockout_seq(buttons);
                } else {
                    buttons &= !SHIFTER_SEQ_BITS;
                    self.shift_prev_seq = 0;
                    self.shift_suppressed_seq = 0;
                    self.shift_lockout_left = 0;
                }
            }
        }

        // BU1: wheel-button debounce — the firmware debounces only
        // gear / horn / pedal / handbrake inputs, so a bouncy wheel
        // button reaches the stream raw. The stability stage holds
        // each wheel-button bit until it has been steady for
        // `debounce_ms` (5 ms ticks at 200 Hz); bits 0/1/2 skip it
        // (firmware-debounced already). P3-5: the gap (`skipped`
        // lost frames) fast-forwards the debounce — a press that
        // was already steady for the whole gap counts it, instead
        // of the gap being invisible (pulse E1 and lockout SH2
        // deliberately do NOT fast-forward; their windows are
        // documented as approximate over a loss).
        // Skipped while the buttons module is disabled — the disabled branch below clears the
        // state instead, so a stale hold can never survive a
        // re-enable.
        if self.plan.buttons.enabled {
            buttons = self.apply_button_debounce(buttons, skipped);
        }

        // Horn (BUTTON slot 2, bit 2).
        out.horn = self.plan.horn.enabled && (buttons & HORN_BIT) != 0;
        // Horn (BUTTON slot 2, bit 2) — HO1 minimum honk + HO2
        // latch. Momentary: the output follows the button, and a
        // press re-arms the hold to the full minimum on every
        // pressed tick, so the honk lasts at least `horn_min_ms`
        // after release. Latch: each rising edge toggles the horn
        // on/off (the hold does not apply — a latched honk stays
        // on until toggled off, it must not tail after switch-off).
        // The held/latched horn keeps bit 2 set in the button word
        // too, so the game sees button 3 held — the `horn` flag
        // alone would not honk in-game. Disabled clears the hold,
        // the latch and the bit, like the encoder pulse clear.
        let raw_horn = (buttons & HORN_BIT) != 0;
        let horn_rising = raw_horn && !self.horn_prev_pressed;
        self.horn_prev_pressed = raw_horn;
        if !self.plan.horn.enabled {
            out.horn = false;
            buttons &= !HORN_BIT;
            self.horn_hold_remaining = 0;
            self.horn_latch_on = false;
        } else if self.plan.horn.horn_latch {
            if horn_rising {
                self.horn_latch_on = !self.horn_latch_on;
            }
            out.horn = self.horn_latch_on;
            if self.horn_latch_on {
                buttons |= HORN_BIT;
            } else {
                buttons &= !HORN_BIT;
            }
        } else {
            if raw_horn {
                self.horn_hold_remaining = horn_min_ticks(self.plan.horn.horn_min_ms);
            } else {
                self.horn_hold_remaining = self.horn_hold_remaining.saturating_sub(1);
            }
            let held = raw_horn || self.horn_hold_remaining > 0;
            out.horn = held;
            if held {
                buttons |= HORN_BIT;
            } else {
                buttons &= !HORN_BIT;
            }
        }

        // Wheel buttons: a disabled module emits nothing.
        out.buttons = if self.plan.buttons.enabled {
            buttons
        } else {
            0
        };
        // BU2: long-press tracking on the debounced word — the mask
        // of buttons held for at least `long_press_ms`. A disabled
        // module clears the debounce + hold state (same pattern as
        // the encoder pulse clear below).
        out.long_press_buttons = if self.plan.buttons.enabled {
            self.track_button_holds(out.buttons)
        } else {
            self.button_stable = 0;
            self.button_candidate = 0;
            self.button_candidate_ticks = [0; 32];
            self.button_hold_ticks = [0; 32];
            self.button_held_mask = 0;
            self.button_last_release_tick = [0; 32];
            self.button_prev_buttons = 0;
            self.button_double_hold = 0;
            0
        };
        // BU3: double-press tracking on the debounced word — the
        // mask of buttons held on a press that started within
        // `double_press_ms` of the previous release. Clears with the
        // same disabled branch above (which zeroes the BU3 state).
        out.double_press_buttons = if self.plan.buttons.enabled {
            self.track_button_double_press(out.buttons)
        } else {
            0
        };
        // D9: buttons > 32 are rejected, never clamped. The stream
        // packet carries only buttons 0–31, so there is nothing to
        // reject here; the output backends enforce D9 when mapping
        // to vJoy/HIDMaestro buttons 1–32.
        out.buttons_ex = 0;

        // Encoders: the baseline keeps tracking even while disabled
        // (no output), so re-enabling cannot emit a stale jump.
        let deltas = self.encoders.update_all_with_gap(
            &frame.encoders,
            frame.n_enc.min(MAX_ENCODERS),
            skipped,
        );
        if self.plan.encoders.enabled {
            out.encoder_deltas = self.apply_encoder_tuning(deltas);
            out.encoder_buttons = self.apply_encoder_pulse(out.encoder_deltas);
        } else {
            // Disabled: a held pulse must not survive to re-enable.
            self.encoder_pulse = [0; MAX_ENCODERS * 2];
            self.encoder_pulse_mask = 0;
        }
    }

    /// SH2, H-pattern side: accept `gear` unless it differs from
    /// the last accepted gear while the lockout window is still
    /// open — then the output holds the last accepted gear. The
    /// window ticks down once per processed frame; an accepted
    /// change restarts it. A level (not an edge) is decoded, so a
    /// gear still engaged after the window ends is accepted on the
    /// next tick.
    fn apply_shift_lockout_h(&mut self, gear: u8) -> u8 {
        self.shift_lockout_left = self.shift_lockout_left.saturating_sub(1);
        if gear != self.shift_last_gear {
            if self.shift_lockout_left > 0 {
                return self.shift_last_gear;
            }
            self.shift_last_gear = gear;
            self.shift_lockout_left = shift_lockout_ticks(self.plan.shifter.shift_lockout_ms);
        }
        self.shift_last_gear
    }

    /// SH2, sequential side: a shift is a rising edge on bit 0/1.
    /// An edge inside the lockout window is rejected — and the
    /// rejected press stays suppressed while held (a held bit would
    /// otherwise leak through on the very next tick, when there is
    /// no new edge, and the double-tap would land anyway). Releasing
    /// the press clears its suppression, so the next real tap can
    /// fire. An accepted edge restarts the window.
    fn apply_shift_lockout_seq(&mut self, buttons: u32) -> u32 {
        self.shift_lockout_left = self.shift_lockout_left.saturating_sub(1);
        let seq = buttons & SHIFTER_SEQ_BITS;
        let edges = seq & !self.shift_prev_seq;
        self.shift_prev_seq = seq;
        // A rejected press stays suppressed while held; releasing it
        // clears the suppression.
        self.shift_suppressed_seq &= seq;
        if edges != 0 {
            if self.shift_lockout_left > 0 {
                self.shift_suppressed_seq |= edges;
            } else {
                self.shift_lockout_left = shift_lockout_ticks(self.plan.shifter.shift_lockout_ms);
            }
        }
        buttons & !self.shift_suppressed_seq
    }

    /// E2 (+E3/E4): turn raw glitch-filtered deltas into emitted steps.
    ///
    /// Per encoder: the module-level `invert` or that encoder's bit
    /// in `encoder_invert_mask` flips the direction (the encoder path
    /// never read the module `invert` before — steering/pedals did —
    /// so an encoders `invert` was silently dead until E2 wired it
    /// in), then `counts_per_click` counts make one step; the sub-click
    /// remainder is banked across ticks, so turning back unwinds it
    /// instead of emitting a phantom step. Division truncates toward
    /// zero, which keeps the remainder's sign correct for reversals.
    /// E3: with acceleration on, a fast spin (`|step| >= 2` in one
    /// 5 ms tick — 2+ detents per tick) emits extra steps,
    /// `round(step * accel_sensitivity)` more, same direction.
    /// The `i32` step is clamped to the `i16` range before the cast.
    /// The `i32 → f32` cast in the accel extra cannot lose precision
    /// in practice: a step is at most ~32768 (glitch filter), far
    /// below the 2^24 integers `f32` represents exactly.
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    fn apply_encoder_tuning(&mut self, raw: [i16; MAX_ENCODERS]) -> [i16; MAX_ENCODERS] {
        let tuning = self.plan.encoders;
        let divisor = i32::from(tuning.counts_per_click);
        let mut steps = [0i16; MAX_ENCODERS];
        for (i, &delta) in raw.iter().enumerate() {
            let mut delta = i32::from(delta);
            if tuning.invert || tuning.encoder_invert_mask & (1u8 << i) != 0 {
                delta = -delta;
            }
            self.encoder_remainder[i] += delta;
            // Idle encoder (no movement, nothing banked): the step
            // is 0 whatever the invert / divisor / accel settings
            // say, so the division and accel test are skipped. The
            // common case — a wheel reports 2 encoders, slots 2–3
            // never move — so half the loop is cut on every tick.
            if delta == 0 && self.encoder_remainder[i] == 0 {
                continue;
            }
            let mut step = div_counts(self.encoder_remainder[i], divisor);
            self.encoder_remainder[i] -= step * divisor;
            if tuning.accel_enabled && step.unsigned_abs() >= 2 {
                let extra = (step as f32 * tuning.accel_sensitivity).round() as i32;
                step += extra;
            }
            steps[i] = step.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
        }
        steps
    }

    /// E1: hold each fresh step's virtual-button bit for
    /// `pulse_ms` (4–40 ticks at 5 ms/tick) instead of one tick.
    /// A fresh step re-arms its bit to the full hold; the hold
    /// counts down once per tick, including the fresh tick.
    ///
    /// Only held or freshly-stepped bits are visited (set bits of
    /// `fresh | encoder_pulse_mask`): a bit with a zero counter
    /// contributes nothing, so skipping it is exactly the 8-bit
    /// loop with the no-op iterations cut. `encoder_pulse_mask`
    /// is the set of bits with a nonzero counter on entry, so a
    /// bit is in this tick's mask exactly when the old loop would
    /// have set it (fresh, or counter > 0), and the next mask is
    /// the bits still nonzero after the countdown — the hold
    /// length is unchanged.
    fn apply_encoder_pulse(&mut self, steps: [i16; MAX_ENCODERS]) -> EncoderButtons {
        let ticks = self.encoder_pulse_ticks;
        let fresh = deltas_to_buttons(&steps);
        let mask = fresh.0 | self.encoder_pulse_mask;
        let mut next_mask = 0u8;
        let mut active = mask;
        while active != 0 {
            let bit = active.trailing_zeros();
            active &= active - 1;
            let bit_mask = 1u8 << bit;
            let remaining = &mut self.encoder_pulse[bit as usize];
            if fresh.0 & bit_mask != 0 {
                *remaining = ticks;
            }
            *remaining = remaining.saturating_sub(1);
            if *remaining > 0 {
                next_mask |= bit_mask;
            }
        }
        self.encoder_pulse_mask = next_mask;
        EncoderButtons(mask)
    }

    /// BU1: per-bit stability stage for the wheel-button word. A
    /// bit flips in `button_stable` only after its raw value has
    /// held steady for `debounce_ms` (0 = pass-through). Bits in
    /// [`BUTTON_DEBOUNCE_EXCLUDE_MASK`] pass through untouched.
    /// A bounce shorter than the hold never reaches the output; a
    /// genuine press is delayed by at most `debounce_ms`.
    ///
    /// P3-5: `skipped` lost frames (from `process_with_gap`)
    /// count toward the hold — this tick advances each candidate
    /// by `1 + skipped`, so a press steady across a gap is not
    /// held back by frames that never arrived.
    ///
    /// Fast path: when every wheel bit already agrees with stable,
    /// the per-bit loop would only clear pending candidates — the
    /// same clearing in one shot, then the raw word unchanged.
    /// (A bit that disagreed on an earlier tick has its candidate
    /// and count cleared here exactly as the loop's settled branch
    /// would; a bit that disagrees on a later tick re-arms from
    /// zero either way, so the debounce timing is unchanged.)
    fn apply_button_debounce(&mut self, raw: u32, skipped: u16) -> u32 {
        let needed = button_debounce_ticks(self.plan.buttons.debounce_ms);
        let wheel_raw = raw & !BUTTON_DEBOUNCE_EXCLUDE_MASK;
        if needed == 0 {
            self.button_stable = wheel_raw;
            self.button_candidate = wheel_raw;
            self.button_candidate_ticks = [0; 32];
            return raw;
        }
        if wheel_raw == self.button_stable {
            self.button_candidate = 0;
            self.button_candidate_ticks = [0; 32];
            return raw;
        }
        // P3-5: one processed tick plus each skipped (lost) tick.
        let step: u8 = 1u8.saturating_add(u8::try_from(skipped).unwrap_or(u8::MAX));
        for bit in 0..32 {
            let mask = 1u32 << bit;
            if mask & BUTTON_DEBOUNCE_EXCLUDE_MASK != 0 {
                continue;
            }
            let raw_bit = wheel_raw & mask != 0;
            let stable_bit = self.button_stable & mask != 0;
            if raw_bit == stable_bit {
                // Settled: forget any candidate.
                self.button_candidate &= !mask;
                self.button_candidate_ticks[bit] = 0;
            } else {
                // Disagreeing with stable: (re)arm the candidate,
                // then count this tick toward the switch.
                if (self.button_candidate & mask != 0) != raw_bit {
                    if raw_bit {
                        self.button_candidate |= mask;
                    } else {
                        self.button_candidate &= !mask;
                    }
                    self.button_candidate_ticks[bit] = 0;
                }
                self.button_candidate_ticks[bit] =
                    self.button_candidate_ticks[bit].saturating_add(step);
                if self.button_candidate_ticks[bit] >= needed {
                    if raw_bit {
                        self.button_stable |= mask;
                    } else {
                        self.button_stable &= !mask;
                    }
                    self.button_candidate_ticks[bit] = 0;
                }
            }
        }
        (raw & BUTTON_DEBOUNCE_EXCLUDE_MASK) | (self.button_stable & !BUTTON_DEBOUNCE_EXCLUDE_MASK)
    }

    /// BU2: per-button hold tracking on the debounced word.
    /// Returns the mask of buttons held for at least
    /// `long_press_ms` this tick; a released button clears at
    /// once, so the mask is only set while the button is down.
    ///
    /// Only pressed or currently-held buttons are visited (set
    /// bits of `buttons | button_held_mask`): every other bit has
    /// a zero hold count and contributes nothing, so skipping it
    /// is exactly the 32-bit loop with the no-op iterations cut.
    fn track_button_holds(&mut self, buttons: u32) -> u32 {
        let needed = button_long_press_ticks(self.plan.buttons.long_press_ms);
        let mut long = 0u32;
        let mut remaining = buttons | self.button_held_mask;
        while remaining != 0 {
            let bit = remaining.trailing_zeros();
            remaining &= remaining - 1;
            let mask = 1u32 << bit;
            let held = &mut self.button_hold_ticks[bit as usize];
            if buttons & mask != 0 {
                *held = held.saturating_add(1);
                self.button_held_mask |= mask;
                if *held >= needed {
                    long |= mask;
                }
            } else {
                *held = 0;
                self.button_held_mask &= !mask;
            }
        }
        long
    }

    /// BU3: double-press detection on the debounced word. A press
    /// that starts within `double_press_ms` of the previous release
    /// of the same button sets that button's bit in
    /// `button_double_hold` for the whole hold; the bit clears on
    /// release. The gap is measured in pipeline ticks (5 ms at
    /// 200 Hz); a `process_with_gap` loss makes the window
    /// approximate by the lost frames, which is acceptable for a
    /// human double-press. Returns the current double-press mask.
    ///
    /// Only bits whose pressed state changed this tick are visited:
    /// an unchanged bit has no edge, so neither its release tick
    /// nor the double-press hold can move. The tick counter still
    /// advances on every call, so the release→press gap is measured
    /// in real ticks even on ticks with no edge.
    fn track_button_double_press(&mut self, buttons: u32) -> u32 {
        let window = button_double_press_ticks(self.plan.buttons.double_press_ms);
        self.button_tick = self.button_tick.wrapping_add(1);
        let tick = self.button_tick;
        let mut changed = buttons ^ self.button_prev_buttons;
        while changed != 0 {
            let bit = changed.trailing_zeros();
            changed &= changed - 1;
            let mask = 1u32 << bit;
            let last_release = &mut self.button_last_release_tick[bit as usize];
            let pressed = buttons & mask != 0;
            if pressed {
                // Rising edge: double-press if the previous release
                // was recent enough (and this is not the first
                // press ever — `last_release == 0` means "never").
                if *last_release != 0 && tick.wrapping_sub(*last_release) <= u32::from(window) {
                    self.button_double_hold |= mask;
                }
            } else {
                // Falling edge: remember the release tick, clear
                // the double-press hold for this button.
                *last_release = tick;
                self.button_double_hold &= !mask;
            }
        }
        self.button_prev_buttons = buttons;
        self.button_double_hold
    }
    // --- Handbrake/clutch per-module tuning ---
    /// H1/H3: handbrake latch + auto-release on throttle.
    ///
    /// `value` is the processed handbrake output (post-ramp,
    /// `0..=1`); `pressed_value` is the pre-ramp value (P2-3) the
    /// latch edge test reads — a button-mode tap held for less
    /// than the press ramp must still toggle the latch, and the
    /// post-ramp value would never reach 0.5 in that time
    /// (default press ramp 150 ms caps the rise at 0.033/tick);
    /// `accel` is this tick's accelerator output;
    /// `input_present` is false when the axis is missing (the
    /// board does not sample it). "Pressed" is `pressed_value >
    /// 0.5`: a full pull (axis) or a debounced press (button mode,
    /// which the firmware sends as 4095/0 on the same axis).
    /// A rising edge toggles the latch — press once = stays at
    /// full, press again = release. While latched the output is
    /// `1.0` whatever the input; on release the output follows
    /// the input again (so unlatching while still holding gives
    /// full until the lever is let go). With latch off, or the
    /// module disabled / the axis missing, the latch clears and
    /// the output follows the input. H3: with latch and
    /// auto-release both on, accelerator past 20% clears the
    /// latch (rally launch) — before the edge test, so a
    /// throttle press never re-latches in the same tick.
    fn apply_handbrake_latch(
        &mut self,
        value: f32,
        pressed_value: f32,
        accel: f32,
        input_present: bool,
    ) -> f32 {
        let tuning = self.plan.handbrake;
        if !tuning.enabled || !input_present {
            self.handbrake_latched = false;
            self.handbrake_prev_pressed = false;
            return 0.0;
        }
        if !tuning.handbrake_latch {
            self.handbrake_latched = false;
            self.handbrake_prev_pressed = pressed_value > 0.5;
            return value;
        }
        if tuning.handbrake_auto_release && self.handbrake_latched && accel > 0.2 {
            self.handbrake_latched = false;
        }
        let pressed = pressed_value > 0.5;
        if pressed && !self.handbrake_prev_pressed {
            self.handbrake_latched = !self.handbrake_latched;
        }
        self.handbrake_prev_pressed = pressed;
        if self.handbrake_latched { 1.0 } else { value }
    }
}

/// Truncating division of `value` by a `counts_per_click` divisor.
/// The sanitized divisors (`1`, `2`, `4`) are powers of two, so they
/// use the bias-and-shift idiom instead of a hardware `idiv` — the
/// remainder banking runs this 4 times per tick on the 200 Hz hot
/// path, and integer division is the slowest op in the tick (bench
/// regression: the E2 division was ~40 ns/tick of the +42 ns
/// encoder cost). The bias (`(value >> 31) & (d - 1)`) rounds toward
/// zero exactly like `/` for negative values; it cannot overflow
/// (a negative `value` gains at most 3, a positive one gains 0).
/// Any other divisor — unreachable after sanitize, but the plan is
/// also constructible directly — falls back to `/`.
#[must_use]
fn div_counts(value: i32, divisor: i32) -> i32 {
    match divisor {
        1 => value,
        2 => (value + ((value >> 31) & 1)) >> 1,
        4 => (value + ((value >> 31) & 3)) >> 2,
        d => value / d,
    }
}

/// Shift-lockout window in ticks (SH2): one tick is 5 ms at
/// 200 Hz, so the 0–200 ms range is 0–40 ticks (`0` = off). The
/// clamp to `0..=40` makes both casts safe.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn shift_lockout_ticks(lockout_ms: f32) -> u8 {
    ((lockout_ms / 5.0).round().clamp(0.0, 40.0)) as u8
}

/// Encoder pulse hold in ticks for a `pulse_ms` setting (E1): one
/// tick is 5 ms at 200 Hz, so the 20–200 ms range is 4–40 ticks
/// (1 tick minimum — a pulse shorter than one tick is still one tick).
/// The clamp to `1..=40` makes both casts safe.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn encoder_pulse_ticks(pulse_ms: f32) -> u8 {
    ((pulse_ms / 5.0).round().clamp(1.0, 40.0)) as u8
}

/// BU1: debounce hold in ticks for a `debounce_ms` setting (0–50
/// ms → 0–10 ticks at 5 ms/tick; 0 = pass-through, no hold).
/// The clamp to `0..=10` makes both casts safe.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn button_debounce_ticks(debounce_ms: f32) -> u8 {
    ((debounce_ms / 5.0).round().clamp(0.0, 10.0)) as u8
}

/// BU2: long-press hold in ticks for a `long_press_ms` setting
/// (300–1500 ms → 60–300 ticks at 5 ms/tick). The clamp to
/// `60..=300` makes both casts safe.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn button_long_press_ticks(long_press_ms: f32) -> u16 {
    ((long_press_ms / 5.0).round().clamp(60.0, 300.0)) as u16
}

/// BU3: double-press window in ticks for a `double_press_ms`
/// setting (150–500 ms → 30–100 ticks at 5 ms/tick). The clamp to
/// `30..=100` makes both casts safe.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn button_double_press_ticks(double_press_ms: f32) -> u16 {
    ((double_press_ms / 5.0).round().clamp(30.0, 100.0)) as u16
}

/// Horn minimum-honk hold in ticks for a `horn_min_ms` setting
/// (HO1): one tick is 5 ms at 200 Hz, so the 100–1000 ms range is
/// 20–200 ticks. The clamp to `20..=200` makes the cast safe.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn horn_min_ticks(min_ms: f32) -> u16 {
    ((min_ms / 5.0).round().clamp(20.0, 200.0)) as u16
}

/// Map a steering sample in mV to `-1..=1` using a
/// `min / centre / max` range. Out-of-range samples clamp to the ends.
/// Returns `0.0` for a degenerate range (`centre <= min` or
/// `max <= centre`).
#[must_use]
pub fn map_steering_mv(mv: u16, min_mv: u16, center_mv: u16, max_mv: u16) -> f32 {
    if center_mv <= min_mv || max_mv <= center_mv {
        return 0.0;
    }
    if mv <= center_mv {
        (-f32::from(center_mv - mv) / f32::from(center_mv - min_mv)).clamp(-1.0, 1.0)
    } else {
        (f32::from(mv - center_mv) / f32::from(max_mv - center_mv)).clamp(-1.0, 1.0)
    }
}

/// Bite-point remap for a mapped pedal value in `0..=1`: below the
/// bite the output is `0.0`; above it the remaining travel is
/// stretched to `0..=1`. A bite of `0.0` is the identity. Values
/// outside `0..=1` clamp (a sanitized plan never sends them, but the
/// stage must not amplify an insane input).
#[must_use]
pub fn apply_bite_point(value: f32, bite: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    let bite = bite.clamp(0.0, 1.0);
    if bite <= 0.0 {
        return value;
    }
    if value <= bite {
        return 0.0;
    }
    ((value - bite) / (1.0 - bite)).clamp(0.0, 1.0)
}

/// Bite-point remap with a slip zone for a mapped pedal value in
/// `0..=1`: the clutch engages over a travel width `width` around
/// the bite instead of the hard stretch. Below `bite - width/2`
/// the output is `0.0`; above `bite + width/2` it is the hard
/// [`apply_bite_point`] stretch; between the two it ramps linearly
/// from `0.0` to the hard value at the top of the zone, so the
/// curve is continuous and the knee is soft instead of sharp. A
/// width of `0.0` (or less), a bite of `0.0`, or a bite of `1.0`
/// is exactly the hard remap — the clutch default is `0.15`, every
/// other module carries `0.0`, so only the clutch ever softens.
#[must_use]
pub fn apply_bite_point_width(value: f32, bite: f32, width: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    let bite = bite.clamp(0.0, 1.0);
    if width <= 0.0 || bite <= 0.0 || bite >= 1.0 {
        return apply_bite_point(value, bite);
    }
    let lo = (bite - width / 2.0).max(0.0);
    let hi = (bite + width / 2.0).min(1.0);
    if hi <= lo {
        return apply_bite_point(value, bite);
    }
    if value <= lo {
        return 0.0;
    }
    let hard_at_hi = (hi - bite) / (1.0 - bite);
    if value >= hi {
        return apply_bite_point(value, bite);
    }
    ((value - lo) / (hi - lo) * hard_at_hi).clamp(0.0, 1.0)
}

/// Brake-force (BRF) rescale for a mapped pedal value in `0..=1`:
/// `saturation` is the travel fraction that gives 100% brake, so
/// the output is `value / saturation`, clamped to `0..=1`. A
/// saturation of `1.0` is the identity. Values outside `0.5..=1.0`
/// clamp (a sanitized plan never sends them, but the stage must not
/// amplify an insane input — and `0.0` would divide by zero).
/// A saturation at/above `1.0` (the default) or non-finite (which
/// sanitizes to `1.0`) skips the division: `value / 1.0` is `value`.
#[must_use]
pub fn apply_saturation(value: f32, saturation: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    if !(saturation.is_finite() && saturation < 1.0) {
        return value;
    }
    let saturation = clamp_finite(saturation, 0.5, 1.0, 1.0);
    (value / saturation).clamp(0.0, 1.0)
}

/// Map a pedal sample in mV to `0..=1` using a `min / max` range.
/// Out-of-range samples clamp to the ends. Returns `0.0` for a
/// degenerate range (`max <= min`).
#[must_use]
pub fn map_pedal_mv(mv: u16, min_mv: u16, max_mv: u16) -> f32 {
    if max_mv <= min_mv {
        return 0.0;
    }
    (f32::from(mv.saturating_sub(min_mv)) / f32::from(max_mv - min_mv)).clamp(0.0, 1.0)
}

/// Bipolar deadzone with rescale: inside the deadzone the output is
/// `0`; outside, the remaining travel is stretched back to full
/// scale. `deadzone` is clamped to `0..=0.5`.
#[must_use]
pub fn apply_deadzone_bipolar(value: f32, deadzone: f32) -> f32 {
    let dz = clamp_finite(deadzone, 0.0, 0.5, 0.0);
    if value.abs() <= dz {
        0.0
    } else {
        value.signum() * (value.abs() - dz) / (1.0 - dz)
    }
}

/// Unipolar deadzone with rescale (same rule as
/// [`apply_deadzone_bipolar`] on `0..=1`).
#[must_use]
pub fn apply_deadzone_unipolar(value: f32, deadzone: f32) -> f32 {
    let dz = clamp_finite(deadzone, 0.0, 0.5, 0.0);
    if value <= dz {
        0.0
    } else {
        (value - dz) / (1.0 - dz)
    }
}

/// Response curve, cubic blend: `y = x + c * (x^3 - x)` with `c`
/// clamped to `-1..=1`. `c = 0` is linear; positive `c` softens the
/// centre (progressive), negative `c` sharpens it. Sign-preserving,
/// so it works for bipolar (`-1..=1`) and unipolar (`0..=1`) inputs.
#[must_use]
pub fn apply_curve(value: f32, strength: f32) -> f32 {
    let c = clamp_finite(strength, -1.0, 1.0, 0.0);
    value + c * (value * value * value - value)
}

/// Steering scale divide (D7): `value / scale`, clamped to `-1..=1`.
/// `scale` is clamped to `0.2..=1.0` (NaN → `1.0`).
#[must_use]
pub fn apply_steering_scale(value: f32, scale: f32) -> f32 {
    let s = clamp_finite(scale, 0.2, 1.0, 1.0);
    (value / s).clamp(-1.0, 1.0)
}

/// Decode the H-pattern gear from stream `buttons` bits 5–10:
/// exactly one gear bit set → gear `1..=6` (bit 5 = gear 1, …,
/// bit 10 = gear 6); zero or more than one bit → neutral (`0`).
#[must_use]
pub fn decode_gear(buttons: u32) -> u8 {
    let gear_bits = buttons & GEAR_BITS_MASK;
    if gear_bits.count_ones() != 1 {
        return 0;
    }
    let bit = gear_bits.trailing_zeros();
    // Bits 5..=10 map to gears 1..=6.
    u8::try_from(bit - 5 + 1).unwrap_or(0)
}

/// Apply One Euro smoothing for one tick. `smoothing <= 0` bypasses
/// the filter (state reset, value passes through); otherwise the
/// filter runs with the cutoff configured when the plan was set
/// ([`OneEuroFilter::configure`] in [`ProcessingPipeline::new`] /
/// [`set_plan_live`](ProcessingPipeline::set_plan_live)) — the
/// cutoff is NOT re-derived here per tick (bench regression: the
/// per-tick `set_min_cutoff` on all five axes recomputed the same
/// cutoff every tick). The enable flag is still applied per tick so
/// a filter used without a plan-set configuration behaves as before.
#[must_use]
pub fn apply_smoothing(filter: &mut OneEuroFilter, value: f32, smoothing: f32) -> f32 {
    if !(smoothing.is_finite() && smoothing > 0.0) {
        filter.set_enabled(false);
        return value;
    }
    filter.set_enabled(true);
    filter.filter(value)
}

/// Steering chain for one tick: map → deadzone → curve → divide →
/// smooth. Returns `0.0` (and resets the filter) when the module is
/// disabled or the axis is missing.
fn process_steering_axis(
    mv: Option<u16>,
    tuning: &ModuleTuning,
    filter: &mut OneEuroFilter,
) -> f32 {
    if !tuning.enabled {
        filter.reset();
        return 0.0;
    }
    let Some(mv) = mv else {
        filter.reset();
        return 0.0;
    };
    let (min_mv, center_mv, max_mv) = steering_range(tuning);
    let mut value = map_steering_mv(mv, min_mv, center_mv, max_mv);
    if tuning.invert {
        value = -value;
    }
    value = apply_deadzone_bipolar(value, tuning.deadzone);
    value = apply_curve(value, tuning.curve_strength);
    // --- Steering per-module tuning (S2) ---
    // The balance lives in the side scales: negative output (left)
    // divides by `scale_left`, positive (right) by `scale_right`.
    // The sign is read AFTER the invert, so "left" always means
    // left in-game, whatever the pot wiring. At balance 0 both
    // sides equal `scale`, so the old single-scale behaviour is
    // exactly preserved.
    let side_scale = if value < 0.0 {
        tuning.scale_left
    } else {
        tuning.scale_right
    };
    value = apply_steering_scale(value, side_scale);
    apply_smoothing(filter, value, tuning.smoothing)
}

/// Pedal chain for one tick: map → saturation → bite → deadzone →
/// curve → smooth → ramp. Returns `(ramped, pre_ramp)` — the ramped
/// output first, and the pre-ramp (post-smoothing) value second: the
/// handbrake latch edge test reads the pre-ramp value (P2-3), so a
/// short button-mode tap still toggles the latch even though the
/// default 150 ms press ramp has not risen past 0.5 yet. Returns
/// `(0.0, 0.0)` (and resets filter and ramp) when the module is
/// disabled or the axis is missing.
fn process_pedal_axis(
    mv: Option<u16>,
    tuning: &ModuleTuning,
    filter: &mut OneEuroFilter,
    ramp: &mut RampLimiter,
) -> (f32, f32) {
    if !tuning.enabled {
        filter.reset();
        ramp.reset();
        return (0.0, 0.0);
    }
    let Some(mv) = mv else {
        filter.reset();
        ramp.reset();
        return (0.0, 0.0);
    };
    let (min_mv, max_mv) = pedal_range(tuning);
    let mut value = map_pedal_mv(mv, min_mv, max_mv);
    // --- Brake per-module tuning ---
    // B1 brake force: rescale travel so `saturation` travel is full
    // brake (1.0 = off). Runs before the bite point, so the bite
    // and deadzone stages work in rescaled-travel units.
    value = apply_saturation(value, tuning.saturation);
    if tuning.invert {
        value = 1.0 - value;
    }
    // L4 clutch bite point: engages at the bite, silent before it.
    // C2: the clutch plan carries a slip width (0.15) that softens
    // the stretch into a zone around the bite; every other module
    // carries 0.0, so their bite is the hard stretch as before.
    // A zero bite with a zero width is the identity (the mapped
    // value is already `0..=1`), so the stage is skipped — the
    // default for every pedal.
    if tuning.bite_point != 0.0 || tuning.bite_width != 0.0 {
        value = apply_bite_point_width(value, tuning.bite_point, tuning.bite_width);
    }
    value = apply_deadzone_unipolar(value, tuning.deadzone);
    value = apply_curve(value, tuning.curve_strength);
    value = apply_smoothing(filter, value, tuning.smoothing);
    let pre_ramp = value;
    (
        ramp.update(value, DT_S, tuning.press_ms, tuning.release_ms),
        pre_ramp,
    )
}

/// Effective steering range for a tuning: the tuning's own range when
/// sane (D10), otherwise the default range.
#[must_use]
fn steering_range(tuning: &ModuleTuning) -> (u16, u16, u16) {
    if validate_steering_range(tuning.min_mv, tuning.center_mv, tuning.max_mv).is_ok() {
        (tuning.min_mv, tuning.center_mv, tuning.max_mv)
    } else {
        (DEFAULT_MIN_MV, DEFAULT_CENTER_MV, DEFAULT_MAX_MV)
    }
}

/// Effective pedal range for a tuning: the tuning's own range when
/// sane (D10), otherwise the default range.
#[must_use]
fn pedal_range(tuning: &ModuleTuning) -> (u16, u16) {
    if validate_pedal_range(tuning.min_mv, tuning.max_mv).is_ok() {
        (tuning.min_mv, tuning.max_mv)
    } else {
        (DEFAULT_MIN_MV, DEFAULT_MAX_MV)
    }
}

/// One Euro smoothing factor for a cutoff at the pipeline tick rate.
fn alpha(cutoff: f32, dt: f32) -> f32 {
    let tau = 1.0 / (2.0 * PI * cutoff);
    1.0 / (1.0 + tau / dt)
}

/// Sanitize a cutoff: finite and positive values pass through,
/// anything else falls back to `1.0` Hz.
fn sanitize_cutoff(cutoff: f32) -> f32 {
    if cutoff.is_finite() && cutoff > 0.0 {
        cutoff
    } else {
        1.0
    }
}

/// Maximum value change per tick for a ramp time in milliseconds
/// (`0` or less = unlimited, i.e. `INFINITY`).
fn max_step(dt: f32, ramp_ms: f32) -> f32 {
    if ramp_ms.is_finite() && ramp_ms > 0.0 {
        dt * 1000.0 / ramp_ms
    } else {
        f32::INFINITY
    }
}

/// Clamp `value` to `lo..=hi`, returning `default` for non-finite
/// input (NaN/infinite tuning values never reach the pipeline math).
fn clamp_finite(value: f32, lo: f32, hi: f32, default: f32) -> f32 {
    if value.is_finite() {
        value.clamp(lo, hi)
    } else {
        default
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DT_S, INPUT_FREQ_HZ, OneEuroFilter, OutputState, PipelinePlan, ProcessingPipeline,
        ShifterGateMode, apply_curve, apply_deadzone_bipolar, apply_deadzone_unipolar,
        apply_steering_scale, decode_gear, map_pedal_mv, map_steering_mv, shift_lockout_ticks,
    };
    use rsh_protocol::{StreamFlags, StreamFrame};

    /// True when two floats agree to within `1e-6`.
    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-6
    }

    /// A plan with all shaping off, for exact golden vectors:
    /// deadzone 0, curve 0, smoothing 0, ramps off, scale 1.0,
    /// everything enabled (including clutch).
    fn plain_plan() -> PipelinePlan {
        let mut plan = PipelinePlan::default();
        for tuning in [
            &mut plan.steering,
            &mut plan.accelerator,
            &mut plan.brake,
            &mut plan.clutch,
            &mut plan.handbrake,
            &mut plan.shifter,
            &mut plan.horn,
            &mut plan.buttons,
            &mut plan.encoders,
        ] {
            tuning.deadzone = 0.0;
            tuning.curve_strength = 0.0;
            tuning.smoothing = 0.0;
            tuning.press_ms = 0.0;
            tuning.release_ms = 0.0;
            tuning.scale = 1.0;
            tuning.enabled = true;
        }
        plan
    }

    /// One frame with the given axes/encoders/buttons.
    fn frame(axes: &[(usize, u16)], encoders: &[i16], buttons: u32) -> StreamFrame {
        StreamFrame::new(1, 5000, StreamFlags::default(), axes, encoders, buttons)
    }

    #[test]
    fn golden_steering_endpoints_and_centre() {
        // Default range 300..1800..3300 mV, no shaping.
        assert!(approx(map_steering_mv(1800, 300, 1800, 3300), 0.0));
        assert!(approx(map_steering_mv(300, 300, 1800, 3300), -1.0));
        assert!(approx(map_steering_mv(3300, 300, 1800, 3300), 1.0));
        assert!(approx(map_steering_mv(1050, 300, 1800, 3300), -0.5));
        assert!(approx(map_steering_mv(2550, 300, 1800, 3300), 0.5));
        // Out-of-range clamps; degenerate range is 0.
        assert!(approx(map_steering_mv(0, 300, 1800, 3300), -1.0));
        assert!(approx(map_steering_mv(3300, 1800, 1800, 3300), 0.0));

        let mut pipe = ProcessingPipeline::new(plain_plan());
        let out = pipe.process(&frame(&[(0, 1800)], &[], 0));
        assert!(approx(out.steering, 0.0));
        let out = pipe.process(&frame(&[(0, 3300)], &[], 0));
        assert!(approx(out.steering, 1.0));
        let out = pipe.process(&frame(&[(0, 300)], &[], 0));
        assert!(approx(out.steering, -1.0));
    }

    #[test]
    fn golden_deadzone_edge() {
        // Deadzone 0.10: 0.09 and exactly 0.10 are 0; 0.20 rescales
        // to (0.20 - 0.10) / 0.90.
        assert!(approx(apply_deadzone_bipolar(0.09, 0.10), 0.0));
        assert!(approx(apply_deadzone_bipolar(0.10, 0.10), 0.0));
        assert!(approx(apply_deadzone_bipolar(-0.10, 0.10), 0.0));
        assert!(approx(
            apply_deadzone_bipolar(0.20, 0.10),
            (0.20 - 0.10) / 0.90
        ));
        assert!(approx(apply_deadzone_unipolar(0.05, 0.10), 0.0));
        assert!(approx(
            apply_deadzone_unipolar(0.55, 0.10),
            (0.55 - 0.10) / 0.90
        ));

        let mut plan = plain_plan();
        plan.steering.deadzone = 0.10;
        let mut pipe = ProcessingPipeline::new(plan);
        // Normalised 0.09 (mv 1935) -> 0; 0.20 (mv 2100) -> rescaled.
        let out = pipe.process(&frame(&[(0, 1935)], &[], 0));
        assert!(approx(out.steering, 0.0));
        let out = pipe.process(&frame(&[(0, 2100)], &[], 0));
        assert!(approx(out.steering, (0.20 - 0.10) / 0.90));
    }

    #[test]
    fn golden_steering_divide_d7() {
        // D7: steer / scale — scale 0.4 + steer 0.4 = full lock.
        assert!(approx(apply_steering_scale(0.4, 0.4), 1.0));
        assert!(approx(apply_steering_scale(0.2, 0.4), 0.5));
        assert!(approx(apply_steering_scale(0.5, 1.0), 0.5));
        assert!(approx(apply_steering_scale(0.9, 0.4), 1.0));

        let mut plan = plain_plan();
        // S2: the single `scale` is replaced for steering — the
        // divide now reads the side scales (both 0.4 here).
        plan.steering.scale_left = 0.4;
        plan.steering.scale_right = 0.4;
        let mut pipe = ProcessingPipeline::new(plan);
        // mv 2400 -> normalised 0.4 -> / 0.4 = 1.0.
        let out = pipe.process(&frame(&[(0, 2400)], &[], 0));
        assert!(approx(out.steering, 1.0));
    }

    #[test]
    fn steering_invert_flips_sign_and_lock_scales_output() {
        // Bug 7: invert flips the mapped value; the lock scale
        // (lock/900) divides it — 900° → 1.0, 450° → 0.5, 270° → 0.3.
        let mut plan = plain_plan();
        plan.steering.invert = true;
        let mut pipe = ProcessingPipeline::new(plan);
        // mv 2550 -> +0.5 -> inverted -0.5; mv 1050 -> -0.5 -> +0.5.
        let out = pipe.process(&frame(&[(0, 2550)], &[], 0));
        assert!(approx(out.steering, -0.5));
        let out = pipe.process(&frame(&[(0, 1050)], &[], 0));
        assert!(approx(out.steering, 0.5));

        // Lock mapping through the pipeline: normalised 0.4
        // (mv 2400) at full lock stays 0.4, at 450° doubles to 0.8,
        // at 270° clamps at full lock. (S2: the lock scale lands on
        // both side scales now — the single `scale` is no longer
        // read by the steering chain.)
        for (scale, expected) in [(1.0, 0.4), (0.5, 0.8), (0.3, 1.0)] {
            let mut plan = plain_plan();
            plan.steering.scale_left = scale;
            plan.steering.scale_right = scale;
            let mut pipe = ProcessingPipeline::new(plan);
            let out = pipe.process(&frame(&[(0, 2400)], &[], 0));
            assert!(approx(out.steering, expected), "scale {scale}");
        }
    }

    #[test]
    fn steering_balance_splits_scale_per_side() {
        // S2: balance +20% favours left — left scale 0.8, right
        // scale 1.0 at full lock (base 1.0). A left sample
        // (mv 1050 -> -0.5) divides by 0.8 -> -0.625; the mirror
        // right sample (mv 2550 -> +0.5) divides by 1.0 -> +0.5.
        // At balance 0 both sides are the base scale again.
        let mut plan = plain_plan();
        plan.steering.scale_left = 0.8;
        plan.steering.scale_right = 1.0;
        let mut pipe = ProcessingPipeline::new(plan);
        let out = pipe.process(&frame(&[(0, 1050)], &[], 0));
        assert!(approx(out.steering, -0.625));
        let out = pipe.process(&frame(&[(0, 2550)], &[], 0));
        assert!(approx(out.steering, 0.5));
        // Invert keeps the meaning in-game: with invert on, the
        // physical-left sample (mv 1050) flips to +0.5 and now
        // divides by the RIGHT scale (1.0) — the balance follows
        // the output direction, not the pot wiring.
        let mut plan = plain_plan();
        plan.steering.invert = true;
        plan.steering.scale_left = 0.8;
        plan.steering.scale_right = 1.0;
        let mut pipe = ProcessingPipeline::new(plan);
        let out = pipe.process(&frame(&[(0, 1050)], &[], 0));
        assert!(approx(out.steering, 0.5));
        // Side scales sanitize like `scale`: out-of-range clamps,
        // NaN falls back to 1.0.
        let tuning = super::ModuleTuning {
            scale_left: 0.1,
            scale_right: f32::NAN,
            ..super::ModuleTuning::default()
        };
        let clean = tuning.sanitized();
        assert_eq!(clean.scale_left, 0.2);
        assert_eq!(clean.scale_right, 1.0);
    }

    #[test]
    fn steering_smoothing_and_scale_are_clamped_by_sanitize() {
        // Bug 7: smoothing is a 0..=1 range — out-of-range input
        // never reaches the filter; same for the lock scale.
        let tuning = super::ModuleTuning {
            smoothing: 1.5,
            scale: 0.1,
            ..super::ModuleTuning::default()
        };
        let clean = tuning.sanitized();
        assert_eq!(clean.smoothing, 1.0);
        assert_eq!(clean.scale, 0.2);
        let tuning = super::ModuleTuning {
            smoothing: f32::NAN,
            scale: f32::NAN,
            ..super::ModuleTuning::default()
        };
        let clean = tuning.sanitized();
        assert_eq!(clean.smoothing, 0.3);
        assert_eq!(clean.scale, 1.0);
    }

    #[test]
    fn golden_pedal_midpoint_and_curve() {
        assert!(approx(map_pedal_mv(1800, 300, 3300), 0.5));
        assert!(approx(map_pedal_mv(300, 300, 3300), 0.0));
        assert!(approx(map_pedal_mv(3300, 300, 3300), 1.0));
        // Curve c=1 at x=0.5: 0.5 + (0.125 - 0.5) = 0.125.
        assert!(approx(apply_curve(0.5, 1.0), 0.125));
        assert!(approx(apply_curve(0.5, 0.0), 0.5));
        assert!(approx(apply_curve(-0.5, 1.0), -0.125));

        let mut pipe = ProcessingPipeline::new(plain_plan());
        let out = pipe.process(&frame(&[(1, 1800), (2, 300), (4, 3300)], &[], 0));
        assert!(approx(out.accelerator, 0.5));
        assert!(approx(out.brake, 0.0));
        assert!(approx(out.handbrake, 1.0));
    }

    #[test]
    fn golden_disabled_module_emits_nothing() {
        let mut plan = plain_plan();
        plan.steering.enabled = false;
        plan.buttons.enabled = false;
        plan.encoders.enabled = false;
        plan.horn.enabled = false;
        let mut pipe = ProcessingPipeline::new(plan);
        let out = pipe.process(&frame(&[(0, 3300)], &[100], 0xFFFF_FFFF));
        assert!(approx(out.steering, 0.0));
        assert_eq!(out.buttons, 0);
        assert_eq!(out.encoder_deltas, [0, 0, 0, 0]);
        assert!(!out.encoder_buttons.any());
        assert!(!out.horn);
        // But the accelerator (still enabled) passes through.
        let out = pipe.process(&frame(&[(1, 3300)], &[], 0));
        assert!(approx(out.accelerator, 1.0));
    }

    #[test]
    fn golden_encoder_wrap() {
        let mut pipe = ProcessingPipeline::new(plain_plan());
        // Default counts-per-click is 4: one step is 4 counts, so
        // the wrap is crossed in 4-count steps.
        let out = pipe.process(&frame(&[], &[32764], 0));
        assert_eq!(out.encoder_deltas[0], 0); // first sample: baseline
        let out = pipe.process(&frame(&[], &[-32768], 0));
        assert_eq!(out.encoder_deltas[0], 1);
        assert!(out.encoder_buttons.clockwise(0));
        let out = pipe.process(&frame(&[], &[32764], 0));
        assert_eq!(out.encoder_deltas[0], -1);
        assert!(out.encoder_buttons.counter_clockwise(0));
    }

    #[test]
    fn encoder_pulse_holds_button_for_pulse_ms() {
        use super::encoder_pulse_ticks;
        assert_eq!(encoder_pulse_ticks(50.0), 10);
        assert_eq!(encoder_pulse_ticks(20.0), 4);
        assert_eq!(encoder_pulse_ticks(200.0), 40);
        assert_eq!(encoder_pulse_ticks(5.0), 1);
        // E1: 50 ms = 10 ticks at 200 Hz (counts-per-click 1, so
        // the pulse is isolated from the division).
        let mut plan = plain_plan();
        plan.encoders.counts_per_click = 1;
        plan.encoders.pulse_ms = 50.0;
        let mut pipe = ProcessingPipeline::new(plan);
        let _ = pipe.process(&frame(&[], &[0], 0)); // baseline
        let out = pipe.process(&frame(&[], &[1], 0));
        assert_eq!(out.encoder_deltas[0], 1);
        assert!(out.encoder_buttons.clockwise(0));
        // Held for 10 ticks total (fresh tick + 9 held), with no
        // new counts and no new deltas.
        for _ in 0..9 {
            let out = pipe.process(&frame(&[], &[1], 0));
            assert_eq!(out.encoder_deltas[0], 0);
            assert!(out.encoder_buttons.clockwise(0));
        }
        // 11th tick: the hold expired.
        let out = pipe.process(&frame(&[], &[1], 0));
        assert!(!out.encoder_buttons.any());
        // A fresh step while held re-arms the full hold.
        let out = pipe.process(&frame(&[], &[2], 0));
        assert!(out.encoder_buttons.clockwise(0));
        for _ in 0..9 {
            let out = pipe.process(&frame(&[], &[2], 0));
            assert!(out.encoder_buttons.clockwise(0));
        }
        let out = pipe.process(&frame(&[], &[2], 0));
        assert!(!out.encoder_buttons.any());
        // E1 floor: 20 ms = 4 ticks.
        let mut plan = plain_plan();
        plan.encoders.counts_per_click = 1;
        plan.encoders.pulse_ms = 20.0;
        let mut pipe = ProcessingPipeline::new(plan);
        let _ = pipe.process(&frame(&[], &[0], 0));
        let out = pipe.process(&frame(&[], &[1], 0));
        assert!(out.encoder_buttons.clockwise(0));
        for _ in 0..3 {
            assert!(
                pipe.process(&frame(&[], &[1], 0))
                    .encoder_buttons
                    .clockwise(0)
            );
        }
        assert!(!pipe.process(&frame(&[], &[1], 0)).encoder_buttons.any());
    }

    #[test]
    fn encoder_counts_per_click_divides_and_banks_remainder() {
        // E2: 4 counts = 1 step; the sub-click remainder is banked
        // across ticks, and turning back unwinds it.
        let mut plan = plain_plan();
        plan.encoders.counts_per_click = 4;
        let mut pipe = ProcessingPipeline::new(plan);
        let _ = pipe.process(&frame(&[], &[0], 0)); // baseline
        for count in [1, 2, 3] {
            let out = pipe.process(&frame(&[], &[count], 0));
            assert_eq!(out.encoder_deltas[0], 0);
            assert!(!out.encoder_buttons.any());
        }
        let out = pipe.process(&frame(&[], &[4], 0));
        assert_eq!(out.encoder_deltas[0], 1);
        assert!(out.encoder_buttons.clockwise(0));
        // One more count banks a remainder of 1; turning back one
        // count unwinds it instead of emitting a phantom step.
        let out = pipe.process(&frame(&[], &[5], 0));
        assert_eq!(out.encoder_deltas[0], 0);
        let out = pipe.process(&frame(&[], &[4], 0));
        assert_eq!(out.encoder_deltas[0], 0);
        // Four back is one CCW step.
        let out = pipe.process(&frame(&[], &[0], 0));
        assert_eq!(out.encoder_deltas[0], -1);
        assert!(out.encoder_buttons.counter_clockwise(0));
        // Two detents in one tick are two steps.
        let out = pipe.process(&frame(&[], &[8], 0));
        assert_eq!(out.encoder_deltas[0], 2);
        assert!(out.encoder_buttons.clockwise(0));
    }

    #[test]
    fn div_counts_matches_truncating_division() {
        // The bias-and-shift idiom must agree with `/` exactly,
        // including negatives (truncation toward zero, not floor)
        // and the i32 extremes.
        use super::div_counts;
        for divisor in [1, 2, 4] {
            for value in -200..=200 {
                assert_eq!(div_counts(value, divisor), value / divisor);
            }
            for value in [i32::MIN, i32::MIN + 1, i32::MAX, -1, 0, 1] {
                assert_eq!(div_counts(value, divisor), value / divisor);
            }
        }
        // Non-power-of-two divisors fall back to `/`.
        for divisor in [3, 5, 7] {
            for value in -100..=100 {
                assert_eq!(div_counts(value, divisor), value / divisor);
            }
        }
    }

    #[test]
    fn encoder_counts_per_click_one_is_identity() {
        let mut plan = plain_plan();
        plan.encoders.counts_per_click = 1;
        let mut pipe = ProcessingPipeline::new(plan);
        let _ = pipe.process(&frame(&[], &[0], 0));
        let out = pipe.process(&frame(&[], &[3], 0));
        assert_eq!(out.encoder_deltas[0], 3);
        assert!(out.encoder_buttons.clockwise(0));
    }

    #[test]
    fn encoder_tuning_sanitizes() {
        use super::ModuleTuning;
        let base = ModuleTuning::default();
        let sane = ModuleTuning {
            pulse_ms: 5.0,
            counts_per_click: 0,
            ..base
        }
        .sanitized();
        assert_eq!(sane.pulse_ms, 20.0);
        assert_eq!(sane.counts_per_click, 4);
        let sane = ModuleTuning {
            pulse_ms: 500.0,
            counts_per_click: 3,
            ..base
        }
        .sanitized();
        assert_eq!(sane.pulse_ms, 200.0);
        assert_eq!(sane.counts_per_click, 4);
        let sane = ModuleTuning {
            pulse_ms: f32::NAN,
            counts_per_click: 2,
            ..base
        }
        .sanitized();
        assert_eq!(sane.pulse_ms, 50.0);
        assert_eq!(sane.counts_per_click, 2);
        let sane = ModuleTuning {
            counts_per_click: 1,
            ..base
        }
        .sanitized();
        assert_eq!(sane.counts_per_click, 1);
    }

    #[test]
    fn encoder_module_invert_flips_direction() {
        // The encoder path never read the module-level `invert`
        // before (steering/pedals did) — it was silently dead for
        // encoders. E2 wires it in: invert flips CW/CCW.
        let mut plan = plain_plan();
        plan.encoders.counts_per_click = 1;
        plan.encoders.invert = true;
        let mut pipe = ProcessingPipeline::new(plan);
        let _ = pipe.process(&frame(&[], &[0], 0));
        let out = pipe.process(&frame(&[], &[4], 0));
        assert_eq!(out.encoder_deltas[0], -4);
        assert!(out.encoder_buttons.counter_clockwise(0));
    }

    #[test]
    fn encoder_acceleration_adds_steps_only_on_fast_spins() {
        // E3: |step| >= 2 in one tick is a fast spin — extra steps
        // are round(step * sensitivity); slow spins are untouched.
        let mut plan = plain_plan();
        plan.encoders.counts_per_click = 1;
        plan.encoders.accel_enabled = true;
        plan.encoders.accel_sensitivity = 1.0;
        let mut pipe = ProcessingPipeline::new(plan);
        let _ = pipe.process(&frame(&[], &[0], 0));
        // Slow: 1 count in a tick — no extra.
        let out = pipe.process(&frame(&[], &[1], 0));
        assert_eq!(out.encoder_deltas[0], 1);
        // Fast: 2 steps in a tick at sensitivity 1.0 doubles to 4.
        let out = pipe.process(&frame(&[], &[3], 0));
        assert_eq!(out.encoder_deltas[0], 4);
        assert!(out.encoder_buttons.clockwise(0));
        // Fast the other way: -2 steps -> -4 (same direction).
        let out = pipe.process(&frame(&[], &[1], 0));
        assert_eq!(out.encoder_deltas[0], -4);
        assert!(out.encoder_buttons.counter_clockwise(0));
        // Sensitivity 0 adds nothing even on a fast spin.
        let mut plan = plain_plan();
        plan.encoders.counts_per_click = 1;
        plan.encoders.accel_enabled = true;
        plan.encoders.accel_sensitivity = 0.0;
        let mut pipe = ProcessingPipeline::new(plan);
        let _ = pipe.process(&frame(&[], &[0], 0));
        let out = pipe.process(&frame(&[], &[3], 0));
        assert_eq!(out.encoder_deltas[0], 3);
        // Sensitivity 2.0 triples a fast spin (2 + 4).
        let mut plan = plain_plan();
        plan.encoders.counts_per_click = 1;
        plan.encoders.accel_enabled = true;
        plan.encoders.accel_sensitivity = 2.0;
        let mut pipe = ProcessingPipeline::new(plan);
        let _ = pipe.process(&frame(&[], &[0], 0));
        let out = pipe.process(&frame(&[], &[2], 0));
        assert_eq!(out.encoder_deltas[0], 6);
        // Off: a fast spin stays exactly what was spun.
        let mut plan = plain_plan();
        plan.encoders.counts_per_click = 1;
        let mut pipe = ProcessingPipeline::new(plan);
        let _ = pipe.process(&frame(&[], &[0], 0));
        let out = pipe.process(&frame(&[], &[2], 0));
        assert_eq!(out.encoder_deltas[0], 2);
    }

    #[test]
    fn encoder_per_encoder_invert_is_independent() {
        // E4: the mask inverts one encoder only; the other keeps
        // its direction. Bit 1 = encoder 1.
        let mut plan = plain_plan();
        plan.encoders.counts_per_click = 1;
        plan.encoders.encoder_invert_mask = 0b0010;
        let mut pipe = ProcessingPipeline::new(plan);
        let _ = pipe.process(&frame(&[], &[0, 0], 0));
        let out = pipe.process(&frame(&[], &[4, 4], 0));
        assert_eq!(out.encoder_deltas, [4, -4, 0, 0]);
        assert!(out.encoder_buttons.clockwise(0));
        assert!(out.encoder_buttons.counter_clockwise(1));
        // Out-of-range bits sanitize away (only 0..=3 kept).
        let mut plan = plain_plan();
        plan.encoders.encoder_invert_mask = 0b1111_0000;
        assert_eq!(plan.sanitized().encoders.encoder_invert_mask, 0);
        // Sensitivity sanitizes to 0..=2, default 1.0.
        let mut plan = plain_plan();
        plan.encoders.accel_sensitivity = 5.0;
        assert_eq!(plan.sanitized().encoders.accel_sensitivity, 2.0);
        plan.encoders.accel_sensitivity = f32::NAN;
        assert_eq!(plan.sanitized().encoders.accel_sensitivity, 1.0);
    }

    #[test]
    fn encoder_disabled_clears_held_pulse() {
        let mut plan = plain_plan();
        plan.encoders.counts_per_click = 1;
        plan.encoders.pulse_ms = 200.0;
        let mut pipe = ProcessingPipeline::new(plan);
        let _ = pipe.process(&frame(&[], &[0], 0));
        let out = pipe.process(&frame(&[], &[1], 0));
        assert!(out.encoder_buttons.clockwise(0));
        // Live-disable (no state reset): the held pulse clears.
        plan.encoders.enabled = false;
        pipe.set_plan_live(plan);
        let out = pipe.process(&frame(&[], &[1], 0));
        assert!(!out.encoder_buttons.any());
        // Re-enable: no stale pulse comes back.
        plan.encoders.enabled = true;
        pipe.set_plan_live(plan);
        let out = pipe.process(&frame(&[], &[1], 0));
        assert!(!out.encoder_buttons.any());
    }

    #[test]
    fn golden_h_pattern_gear_decode() {
        assert_eq!(decode_gear(1 << 5), 1);
        assert_eq!(decode_gear(1 << 10), 6);
        assert_eq!(decode_gear((1 << 5) | (1 << 6)), 0); // two bits: neutral
        assert_eq!(decode_gear(0), 0);
        assert_eq!(decode_gear(1 << 4), 0); // not a gear bit

        let mut plan = plain_plan();
        plan.shifter.gate_mode = ShifterGateMode::HPattern;
        // Pure decode here — the lockout (SH2) is off, so a gear
        // change on the very next tick is not held.
        plan.shifter.shift_lockout_ms = 0.0;
        let mut pipe = ProcessingPipeline::new(plan);
        // Gear bit 7 (gear 3) plus a wheel button (bit 0): gear
        // decodes, gear bits are stripped, bit 0 survives.
        let out = pipe.process(&frame(&[], &[], (1 << 7) | 1));
        assert_eq!(out.gear, 3);
        assert_eq!(out.buttons, 1);
        // Two gear bits -> neutral, both stripped.
        let out = pipe.process(&frame(&[], &[], (1 << 5) | (1 << 6)));
        assert_eq!(out.gear, 0);
        assert_eq!(out.buttons, 0);
    }

    #[test]
    fn shifter_lockout_kills_h_pattern_bounce() {
        let mut plan = plain_plan();
        plan.shifter.gate_mode = ShifterGateMode::HPattern;
        plan.shifter.shift_lockout_ms = 50.0;
        let mut pipe = ProcessingPipeline::new(plan);
        // Gear 3 engages: accepted at once.
        let out = pipe.process(&frame(&[], &[], 1 << 7));
        assert_eq!(out.gear, 3);
        // A bounce to gear 4 inside the 50 ms window is ignored…
        for _ in 0..9 {
            let out = pipe.process(&frame(&[], &[], 1 << 8));
            assert_eq!(out.gear, 3);
        }
        // …but the same gear, still engaged after the window, is accepted.
        let out = pipe.process(&frame(&[], &[], 1 << 8));
        assert_eq!(out.gear, 4);
        // And the window restarts on the accepted shift: a bounce
        // back to gear 3 is ignored again until it expires.
        let out = pipe.process(&frame(&[], &[], 1 << 7));
        assert_eq!(out.gear, 4);
    }

    #[test]
    fn shifter_lockout_kills_sequential_double_tap() {
        let mut plan = plain_plan();
        plan.shifter.shift_lockout_ms = 50.0;
        let mut pipe = ProcessingPipeline::new(plan);
        // Shift up: passes.
        let out = pipe.process(&frame(&[], &[], 0b01));
        assert_eq!(out.buttons, 0b01);
        // Release, then a second tap inside the window: dropped.
        let out = pipe.process(&frame(&[], &[], 0));
        assert_eq!(out.buttons, 0);
        let out = pipe.process(&frame(&[], &[], 0b01));
        assert_eq!(out.buttons, 0);
        // Held through the window, the rejected press stays
        // suppressed — it never leaks through as a late shift.
        for _ in 0..9 {
            let out = pipe.process(&frame(&[], &[], 0b01));
            assert_eq!(out.buttons, 0);
        }
        // Release clears the suppression; a fresh tap past the
        // window passes.
        let out = pipe.process(&frame(&[], &[], 0));
        assert_eq!(out.buttons, 0);
        let out = pipe.process(&frame(&[], &[], 0b01));
        assert_eq!(out.buttons, 0b01);
    }

    #[test]
    fn shifter_lockout_zero_is_off_and_sanitizes() {
        // 0 ms = off: rapid gear changes all pass through.
        let mut plan = plain_plan();
        plan.shifter.gate_mode = ShifterGateMode::HPattern;
        plan.shifter.shift_lockout_ms = 0.0;
        let mut pipe = ProcessingPipeline::new(plan);
        let out = pipe.process(&frame(&[], &[], 1 << 7));
        assert_eq!(out.gear, 3);
        let out = pipe.process(&frame(&[], &[], 1 << 8));
        assert_eq!(out.gear, 4);
        // Sanitizer: 500 clamps to 200, NaN falls back to 50.
        let mut plan = plain_plan();
        plan.shifter.shift_lockout_ms = 500.0;
        assert_eq!(plan.sanitized().shifter.shift_lockout_ms, 200.0);
        plan.shifter.shift_lockout_ms = f32::NAN;
        assert_eq!(plan.sanitized().shifter.shift_lockout_ms, 50.0);
        assert_eq!(shift_lockout_ticks(50.0), 10);
        assert_eq!(shift_lockout_ticks(0.0), 0);
        assert_eq!(shift_lockout_ticks(200.0), 40);
    }

    #[test]
    fn golden_sequential_shifter_and_horn() {
        let mut pipe = ProcessingPipeline::new(plain_plan());
        // Sequential: up/down/horn bits pass through as buttons.
        let out = pipe.process(&frame(&[], &[], 0b111));
        assert_eq!(out.gear, 0);
        assert_eq!(out.buttons, 0b111);
        assert!(out.horn);

        let mut plan = plain_plan();
        plan.shifter.enabled = false;
        plan.horn.enabled = false;
        let mut pipe = ProcessingPipeline::new(plan);
        let out = pipe.process(&frame(&[], &[], 0b111));
        assert_eq!(out.buttons, 0);
        assert!(!out.horn);
    }

    #[test]
    fn horn_min_hold_keeps_honk_after_a_quick_tap() {
        // HO1: a one-tick tap (5 ms) with a 400 ms minimum holds
        // the horn — flag + button bit — for 80 ticks after release,
        // then goes quiet. The game polls button 3, so the bit must
        // stay set for the whole hold, not just the `horn` flag.
        let mut plan = plain_plan();
        plan.horn.horn_min_ms = 400.0;
        let mut pipe = ProcessingPipeline::new(plan);
        let out = pipe.process(&frame(&[], &[], 0b100));
        assert!(out.horn);
        assert_eq!(out.buttons & 0b100, 0b100);
        // 79 released ticks: still honking (the release tick counts
        // as the first of the 80-tick hold).
        for _ in 0..79 {
            let out = pipe.process(&frame(&[], &[], 0));
            assert!(out.horn);
            assert_eq!(out.buttons & 0b100, 0b100);
        }
        let out = pipe.process(&frame(&[], &[], 0));
        assert!(!out.horn);
        assert_eq!(out.buttons & 0b100, 0);
        // A 100 ms minimum is 20 ticks; out-of-range sanitizes to
        // 100..=1000 (default 400).
        assert_eq!(super::horn_min_ticks(100.0), 20);
        assert_eq!(super::horn_min_ticks(1000.0), 200);
        assert_eq!(super::horn_min_ticks(50.0), 20);
        assert_eq!(super::horn_min_ticks(5000.0), 200);
        let mut plan = plain_plan();
        plan.horn.horn_min_ms = 50.0;
        assert_eq!(plan.sanitized().horn.horn_min_ms, 100.0);
        plan.horn.horn_min_ms = f32::NAN;
        assert_eq!(plan.sanitized().horn.horn_min_ms, 400.0);
    }

    #[test]
    fn horn_latch_toggles_on_each_press() {
        // HO2: press once = on (stays on with no button held),
        // press again = off. A held press does not re-toggle —
        // only the rising edge counts.
        let mut plan = plain_plan();
        plan.horn.horn_latch = true;
        let mut pipe = ProcessingPipeline::new(plan);
        let out = pipe.process(&frame(&[], &[], 0b100));
        assert!(out.horn);
        let out = pipe.process(&frame(&[], &[], 0b100));
        assert!(out.horn, "holding the button must not toggle off");
        let out = pipe.process(&frame(&[], &[], 0));
        assert!(out.horn, "latched honk stays on after release");
        assert_eq!(out.buttons & 0b100, 0b100);
        let out = pipe.process(&frame(&[], &[], 0b100));
        assert!(!out.horn, "second press toggles off");
        assert_eq!(out.buttons & 0b100, 0);
        let out = pipe.process(&frame(&[], &[], 0));
        assert!(!out.horn, "no hold tail after a latch switch-off");
        // Live-disable clears a latched honk; re-enable stays quiet.
        let out = pipe.process(&frame(&[], &[], 0b100));
        assert!(out.horn);
        plan.horn.enabled = false;
        pipe.set_plan_live(plan);
        let out = pipe.process(&frame(&[], &[], 0b100));
        assert!(!out.horn);
        assert_eq!(out.buttons & 0b100, 0);
        plan.horn.enabled = true;
        pipe.set_plan_live(plan);
        let out = pipe.process(&frame(&[], &[], 0));
        assert!(!out.horn, "no stale latch comes back on re-enable");
    }

    #[test]
    fn golden_insane_calibration_falls_back_to_default() {
        // Span 200 mV < 660 mV (D10): the default range is used, so
        // mv 1800 still maps to steering centre 0.0.
        let mut plan = plain_plan();
        plan.steering.min_mv = 1000;
        plan.steering.center_mv = 1100;
        plan.steering.max_mv = 1200;
        let mut pipe = ProcessingPipeline::new(plan);
        let out = pipe.process(&frame(&[(0, 1800)], &[], 0));
        assert!(approx(out.steering, 0.0));
        let out = pipe.process(&frame(&[(0, 3300)], &[], 0));
        assert!(approx(out.steering, 1.0));
    }

    #[test]
    fn golden_missing_axis_is_neutral() {
        let mut pipe = ProcessingPipeline::new(plain_plan());
        let out = pipe.process(&frame(&[], &[], 0));
        assert_eq!(out, OutputState::neutral());
    }

    #[test]
    fn golden_pedal_ramp_fusion() {
        // Press 100 ms at 200 Hz: +0.05 per tick. Release 50 ms:
        // -0.10 per tick.
        let mut plan = plain_plan();
        plan.accelerator.press_ms = 100.0;
        plan.accelerator.release_ms = 50.0;
        let mut pipe = ProcessingPipeline::new(plan);
        let out = pipe.process(&frame(&[(1, 300)], &[], 0));
        assert!(approx(out.accelerator, 0.0)); // first sample: baseline
        let mut last = 0.0;
        for _ in 0..20 {
            let out = pipe.process(&frame(&[(1, 3300)], &[], 0));
            last = out.accelerator;
        }
        assert!(approx(last, 1.0));
        // Release: 10 ticks at -0.10 each reaches 0.
        for _ in 0..10 {
            let out = pipe.process(&frame(&[(1, 300)], &[], 0));
            last = out.accelerator;
        }
        assert!(approx(last, 0.0));
    }

    #[test]
    fn golden_combined_pedals() {
        let mut plan = plain_plan();
        plan.combine_pedals = true;
        let mut pipe = ProcessingPipeline::new(plan);
        let out = pipe.process(&frame(&[(1, 2700), (2, 900)], &[], 0));
        // Accel (2700-300)/3000 = 0.8; brake (900-300)/3000 = 0.2.
        assert!(approx(out.accelerator, 0.8));
        assert!(approx(out.brake, 0.2));
        assert!(approx(out.pedals_combined, 0.6));
    }

    #[test]
    fn one_euro_constant_input_converges_and_guards_hold() {
        // Constant input converges to itself.
        let mut filter = OneEuroFilter::new(INPUT_FREQ_HZ, 1.0, 0.0, 1.0);
        let first = filter.filter(0.5);
        assert!(approx(first, 0.5));
        let mut last = 0.0;
        for _ in 0..200 {
            last = filter.filter(0.5);
        }
        assert!(approx(last, 0.5));
        // freq = 0 is a pass-through guard, not a crash.
        let mut guard = OneEuroFilter::new(0.0, 1.0, 0.0, 1.0);
        assert!(guard.is_bypass());
        assert!(approx(guard.filter(0.7), 0.7));
        // Non-finite input never poisons the state.
        let mut filter = OneEuroFilter::new(INPUT_FREQ_HZ, 1.0, 0.0, 1.0);
        assert!(approx(filter.filter(0.5), 0.5));
        assert!(filter.filter(f32::NAN).is_nan());
        assert!(approx(filter.filter(0.5), 0.5));
        // Smoothing 0 disables; default 0.3 enables.
        assert!(OneEuroFilter::from_smoothing(0.0, INPUT_FREQ_HZ).is_bypass());
        assert!(!OneEuroFilter::from_smoothing(0.3, INPUT_FREQ_HZ).is_bypass());
    }

    #[test]
    fn tuning_sanitized_clamps_and_fixes_ranges() {
        use super::ModuleTuning;
        let tuning = ModuleTuning {
            deadzone: 0.9,
            curve_strength: 5.0,
            smoothing: -1.0,
            scale: f32::NAN,
            min_mv: 2000,
            max_mv: 2100, // span 100 mV: insane
            center_mv: 9999,
            ..ModuleTuning::default()
        };
        let sane = tuning.sanitized();
        assert!(approx(sane.deadzone, 0.5));
        assert!(approx(sane.curve_strength, 1.0));
        assert!(approx(sane.smoothing, 0.0));
        assert!(approx(sane.scale, 1.0));
        assert_eq!((sane.min_mv, sane.max_mv), (300, 3300));
        assert_eq!(sane.center_mv, 1800);
    }

    #[test]
    fn set_plan_sanitizes_insane_tuning() {
        // L2: set_plan (and new) sanitize — one source of truth, so
        // process() never sees an insane tuning.
        let mut pipe = ProcessingPipeline::new(PipelinePlan::default());
        let mut plan = PipelinePlan::default();
        plan.steering.deadzone = 2.0; // clamps to 0.5
        plan.steering.scale = f32::NAN; // NaN -> 1.0
        plan.brake.min_mv = 3000;
        plan.brake.max_mv = 300; // insane range -> default 300..3300
        pipe.set_plan(plan);
        assert_eq!(pipe.plan().steering.deadzone, 0.5);
        assert_eq!(pipe.plan().steering.scale, 1.0);
        assert_eq!(pipe.plan().brake.min_mv, 300);
        assert_eq!(pipe.plan().brake.max_mv, 3300);
        // And the stored plan is what process() uses: a full-right
        // steering sample still maps to ~+1 with the sanitized plan.
        let frame = StreamFrame::new(0, 0, StreamFlags::default(), &[(0, 3300)], &[], 0);
        let out = pipe.process(&frame);
        assert!(
            out.steering > 0.9,
            "sanitized plan still maps: {}",
            out.steering
        );
    }

    #[test]
    fn set_plan_resets_state() {
        let mut pipe = ProcessingPipeline::new(plain_plan());
        let _ = pipe.process(&frame(&[], &[1000], 0));
        pipe.set_plan(plain_plan());
        // Encoder baseline was reset: next sample is a first sample.
        let out = pipe.process(&frame(&[], &[1005], 0));
        assert_eq!(out.encoder_deltas[0], 0);
        assert_eq!(*pipe.plan(), plain_plan());
    }

    #[test]
    fn bite_point_remap_is_silent_below_and_stretched_above() {
        use super::apply_bite_point;
        // Bite 0 is the identity.
        assert!(approx(apply_bite_point(0.3, 0.0), 0.3));
        assert!(approx(apply_bite_point(1.0, 0.0), 1.0));
        // Bite 0.4: below/at the bite is 0, the rest stretches.
        assert!(approx(apply_bite_point(0.2, 0.4), 0.0));
        assert!(approx(apply_bite_point(0.4, 0.4), 0.0));
        assert!(approx(apply_bite_point(0.7, 0.4), 0.5));
        assert!(approx(apply_bite_point(1.0, 0.4), 1.0));
        // Insane inputs clamp, never amplify.
        assert!(approx(apply_bite_point(2.0, 0.4), 1.0));
        assert!(approx(apply_bite_point(0.5, 2.0), 0.0));
    }

    #[test]
    fn bite_point_clamps_in_sanitize_and_reaches_clutch_output() {
        use super::ModuleTuning;
        let tuning = ModuleTuning {
            bite_point: 1.5,
            ..ModuleTuning::default()
        };
        assert!(approx(tuning.sanitized().bite_point, 1.0));
        // Clutch at 30% travel (1200 mV in the 300..3300 default
        // range) with bite 0.4 stays 0; at 90% travel it is
        // (0.9 - 0.4) / 0.6 = 0.833.
        let mut plan = plain_plan();
        plan.clutch.bite_point = 0.4;
        let mut pipe = ProcessingPipeline::new(plan);
        let out = pipe.process(&frame(&[(3, 1200)], &[], 0));
        assert!(approx(out.clutch, 0.0));
        let out = pipe.process(&frame(&[(3, 3000)], &[], 0));
        assert!(
            approx(out.clutch, 0.833_333_3),
            "clutch at 90% with bite 0.4: {}",
            out.clutch
        );
    }

    #[test]
    fn bite_width_softens_the_bite_into_a_slip_zone() {
        use super::{ModuleTuning, apply_bite_point_width};
        // Width 0 is exactly the hard remap.
        assert!(approx(apply_bite_point_width(0.7, 0.4, 0.0), 0.5));
        assert!(approx(apply_bite_point_width(0.2, 0.4, 0.0), 0.0));
        // Bite 0.35, width 0.15: the zone is 0.275..0.425 and the
        // hard value at the top of the zone is
        // (0.425 - 0.35) / 0.65 = 0.1154.
        assert!(approx(apply_bite_point_width(0.2, 0.35, 0.15), 0.0));
        assert!(approx(apply_bite_point_width(0.275, 0.35, 0.15), 0.0));
        assert!(approx(
            apply_bite_point_width(0.35, 0.35, 0.15),
            0.057_692_31
        ));
        assert!(approx(
            apply_bite_point_width(0.425, 0.35, 0.15),
            0.115_384_62
        ));
        // Above the zone it is the hard stretch again.
        assert!(approx(
            apply_bite_point_width(0.7, 0.35, 0.15),
            0.538_461_55
        ));
        assert!(approx(apply_bite_point_width(1.0, 0.35, 0.15), 1.0));
        // Sanitize: 0 stays off (hard), over-range clamps, NaN
        // falls back to off — never to a fake width.
        assert!(approx(
            ModuleTuning {
                bite_width: 0.5,
                ..ModuleTuning::default()
            }
            .sanitized()
            .bite_width,
            0.40
        ));
        assert!(approx(
            ModuleTuning {
                bite_width: -0.1,
                ..ModuleTuning::default()
            }
            .sanitized()
            .bite_width,
            0.0
        ));
        assert!(approx(
            ModuleTuning {
                bite_width: f32::NAN,
                ..ModuleTuning::default()
            }
            .sanitized()
            .bite_width,
            0.0
        ));
        // Through the pipeline: clutch at 35% travel (1350 mV)
        // with bite 0.35 + width 0.15 is mid-zone (0.0577), not
        // the hard 0.0; at 70% travel it is the hard 0.5385.
        let mut plan = plain_plan();
        plan.clutch.bite_point = 0.35;
        plan.clutch.bite_width = 0.15;
        let mut pipe = ProcessingPipeline::new(plan);
        let out = pipe.process(&frame(&[(3, 900)], &[], 0));
        assert!(approx(out.clutch, 0.0));
        let out = pipe.process(&frame(&[(3, 1350)], &[], 0));
        assert!(
            approx(out.clutch, 0.057_692_31),
            "clutch mid-zone: {}",
            out.clutch
        );
        let out = pipe.process(&frame(&[(3, 2400)], &[], 0));
        assert!(
            approx(out.clutch, 0.538_461_55),
            "clutch above zone: {}",
            out.clutch
        );
        // The brake carries the same bite but width 0.0, so its
        // bite stays the hard stretch: 35% travel is exactly 0.
        let mut plan = plain_plan();
        plan.brake.bite_point = 0.35;
        let mut pipe = ProcessingPipeline::new(plan);
        let out = pipe.process(&frame(&[(2, 1350)], &[], 0));
        assert!(approx(out.brake, 0.0));
    }

    #[test]
    fn axis_invalid_flag_holds_last_good_axes() {
        // P2-1: axis_invalid set ho to axes aakhri achhi values par
        // hold hote hain (rail-stuck sample full deflection nahi
        // deta); buttons/encoders live frame se chalte rehte hain;
        // flag clear hone par nayi values phir flow karti hain.
        let mut pipe = ProcessingPipeline::new(plain_plan());
        let out = pipe.process(&frame(&[(0, 3300), (1, 1800)], &[], 0));
        assert!(approx(out.steering, 1.0));
        assert!(approx(out.accelerator, 0.5));
        let flags = StreamFlags {
            axis_invalid: true,
            ..Default::default()
        };
        let bad = StreamFrame::new(2, 5000, flags, &[(0, 300), (1, 300)], &[], 0);
        let out = pipe.process(&bad);
        assert!(approx(out.steering, 1.0), "held steering: {}", out.steering);
        assert!(
            approx(out.accelerator, 0.5),
            "held accel: {}",
            out.accelerator
        );
        // Buttons still live: horn bit (1 << 2) on an invalid frame honks.
        let bad_horn = StreamFrame::new(3, 5000, flags, &[(0, 300)], &[], 1 << 2);
        let out = pipe.process(&bad_horn);
        assert!(out.horn, "horn still live on invalid frame");
        assert!(approx(out.steering, 1.0));
        // Flag clears: new values flow again.
        let out = pipe.process(&frame(&[(0, 300)], &[], 0));
        assert!(approx(out.steering, -1.0));
        // No good frame yet: hold is neutral.
        let mut pipe = ProcessingPipeline::new(plain_plan());
        let out = pipe.process(&bad);
        assert!(approx(out.steering, 0.0));
        assert!(approx(out.accelerator, 0.0));
    }

    #[test]
    fn handbrake_latch_holds_and_releases_on_second_press() {
        // H1: press once = stays on after release, press again =
        // release. Latch off = the output follows the input.
        let mut plan = plain_plan();
        plan.handbrake.handbrake_latch = true;
        let mut pipe = ProcessingPipeline::new(plan);
        let out = pipe.process(&frame(&[(4, 3300)], &[], 0));
        assert!(approx(out.handbrake, 1.0));
        let out = pipe.process(&frame(&[(4, 300)], &[], 0));
        assert!(approx(out.handbrake, 1.0), "latched after release");
        let out = pipe.process(&frame(&[(4, 300)], &[], 0));
        assert!(approx(out.handbrake, 1.0), "still latched");
        let out = pipe.process(&frame(&[(4, 3300)], &[], 0));
        assert!(approx(out.handbrake, 1.0), "unlatching press still holds");
        let out = pipe.process(&frame(&[(4, 300)], &[], 0));
        assert!(approx(out.handbrake, 0.0), "released after unlatch");
        // Latch off: the same sequence follows the input.
        let mut pipe = ProcessingPipeline::new(plain_plan());
        let out = pipe.process(&frame(&[(4, 3300)], &[], 0));
        assert!(approx(out.handbrake, 1.0));
        let out = pipe.process(&frame(&[(4, 300)], &[], 0));
        assert!(approx(out.handbrake, 0.0));
        // A missing axis clears a latch (no stale hold): latch,
        // release, drop the axis, then press again latches fresh
        // instead of toggling the old latch off.
        let mut plan = plain_plan();
        plan.handbrake.handbrake_latch = true;
        let mut pipe = ProcessingPipeline::new(plan);
        let _ = pipe.process(&frame(&[(4, 3300)], &[], 0));
        let out = pipe.process(&frame(&[(4, 300)], &[], 0));
        assert!(approx(out.handbrake, 1.0));
        let out = pipe.process(&frame(&[], &[], 0));
        assert!(approx(out.handbrake, 0.0), "missing axis clears latch");
        let out = pipe.process(&frame(&[(4, 3300)], &[], 0));
        assert!(approx(out.handbrake, 1.0), "fresh latch after clear");
    }

    #[test]
    fn handbrake_latch_toggles_on_short_button_tap_despite_press_ramp() {
        // P2-3: button-mode tap (3300 for 5 ticks, phir release) —
        // default press ramp 150 ms se post-ramp value 5 ticks mein
        // sirf ~0.165 tak pahunchti hai (0.5 kabhi nahi) — latch edge
        // pre-ramp value par hai, is liye pehle hi tick par toggle hoti hai.
        let mut plan = plain_plan();
        plan.handbrake.handbrake_latch = true;
        plan.handbrake.press_ms = 150.0;
        plan.handbrake.release_ms = 100.0;
        let mut pipe = ProcessingPipeline::new(plan);
        let out = pipe.process(&frame(&[(4, 300)], &[], 0)); // rest
        assert!(approx(out.handbrake, 0.0));
        // 5-tick tap: pre-ramp 1.0 pehle tick par — latch on.
        for _ in 0..5 {
            let out = pipe.process(&frame(&[(4, 3300)], &[], 0));
            assert!(approx(out.handbrake, 1.0));
        }
        // Release: latched rehta hai (output 1.0, ramp ke bawajood).
        let out = pipe.process(&frame(&[(4, 300)], &[], 0));
        assert!(approx(out.handbrake, 1.0), "latched after short tap");
        let out = pipe.process(&frame(&[(4, 300)], &[], 0));
        assert!(approx(out.handbrake, 1.0), "still latched");
        // Doosra tap: unlatch.
        for _ in 0..5 {
            let _ = pipe.process(&frame(&[(4, 3300)], &[], 0));
        }
        // Release: latch off hai — ramp neeche aane tak output
        // input follow karta hai (0 tak girta hai).
        let mut out = pipe.process(&frame(&[(4, 300)], &[], 0));
        for _ in 0..10 {
            if approx(out.handbrake, 0.0) {
                break;
            }
            out = pipe.process(&frame(&[(4, 300)], &[], 0));
        }
        assert!(approx(out.handbrake, 0.0), "released after second tap");
    }

    #[test]
    fn handbrake_auto_release_clears_latch_on_throttle() {
        // H3: with latch + auto-release on, throttle past 20%
        // releases the handbrake (rally launch); 20% exactly does
        // not. Without auto-release the latch survives full throttle.
        let mut plan = plain_plan();
        plan.handbrake.handbrake_latch = true;
        plan.handbrake.handbrake_auto_release = true;
        let mut pipe = ProcessingPipeline::new(plan);
        let _ = pipe.process(&frame(&[(4, 3300)], &[], 0));
        let out = pipe.process(&frame(&[(4, 300)], &[], 0));
        assert!(approx(out.handbrake, 1.0));
        // Throttle at exactly 20% (900 mV): still latched.
        let out = pipe.process(&frame(&[(1, 900), (4, 300)], &[], 0));
        assert!(approx(out.accelerator, 0.2));
        assert!(approx(out.handbrake, 1.0), "20% throttle keeps latch");
        // Throttle at 50% (1800 mV): latch clears.
        let out = pipe.process(&frame(&[(1, 1800), (4, 300)], &[], 0));
        assert!(approx(out.accelerator, 0.5));
        assert!(approx(out.handbrake, 0.0), "throttle released the latch");
        // Auto-release off: full throttle leaves the latch on.
        let mut plan = plain_plan();
        plan.handbrake.handbrake_latch = true;
        let mut pipe = ProcessingPipeline::new(plan);
        let _ = pipe.process(&frame(&[(4, 3300)], &[], 0));
        let out = pipe.process(&frame(&[(1, 3300), (4, 300)], &[], 0));
        assert!(approx(out.handbrake, 1.0), "no auto-release, latch holds");
    }

    #[test]
    fn handbrake_grab_speed_ramps_through_press_ms() {
        // H2: grab speed IS the handbrake press ramp — 200 ms
        // takes 40 ticks to full travel, so the second tick after
        // a full pull is only 5/200 = 0.025 (the first tick is the
        // ramp's passthrough at rest).
        let mut plan = plain_plan();
        plan.handbrake.press_ms = 200.0;
        let mut pipe = ProcessingPipeline::new(plan);
        let out = pipe.process(&frame(&[(4, 300)], &[], 0));
        assert!(approx(out.handbrake, 0.0));
        let out = pipe.process(&frame(&[(4, 3300)], &[], 0));
        assert!(
            approx(out.handbrake, 0.025),
            "grab 200 ms: {}",
            out.handbrake
        );
    }

    #[test]
    fn brake_saturation_rescales_travel_and_clamps_in_sanitize() {
        use super::{ModuleTuning, apply_saturation};
        // Saturation 1.0 is the identity; 0.5 doubles the travel.
        assert!(approx(apply_saturation(0.3, 1.0), 0.3));
        assert!(approx(apply_saturation(1.0, 1.0), 1.0));
        assert!(approx(apply_saturation(0.25, 0.5), 0.5));
        assert!(approx(apply_saturation(0.5, 0.5), 1.0));
        assert!(approx(apply_saturation(0.75, 0.5), 1.0));
        // Insane inputs clamp, never amplify or divide by zero.
        assert!(approx(apply_saturation(0.3, 0.0), 0.6));
        assert!(approx(apply_saturation(0.3, 2.0), 0.3));
        assert!(approx(apply_saturation(2.0, 0.5), 1.0));
        assert!(approx(apply_saturation(0.3, f32::NAN), 0.3));
        // sanitize() clamps to 0.5..=1.0 (NaN → 1.0).
        let tuning = ModuleTuning {
            saturation: 0.2,
            ..ModuleTuning::default()
        };
        assert!(approx(tuning.sanitized().saturation, 0.5));
        let tuning = ModuleTuning {
            saturation: f32::NAN,
            ..ModuleTuning::default()
        };
        assert!(approx(tuning.sanitized().saturation, 1.0));
        // Through the pipeline: brake at 25% travel (1050 mV in the
        // 300..3300 default range) with saturation 0.5 reads 50%;
        // at 50% travel it reads full brake. Calibration is
        // untouched — only the mapped travel is rescaled.
        let mut plan = plain_plan();
        plan.brake.saturation = 0.5;
        let mut pipe = ProcessingPipeline::new(plan);
        let out = pipe.process(&frame(&[(2, 1050)], &[], 0));
        assert!(
            approx(out.brake, 0.5),
            "brake at 25% with BRF 50%: {}",
            out.brake
        );
        let out = pipe.process(&frame(&[(2, 1800)], &[], 0));
        assert!(
            approx(out.brake, 1.0),
            "brake at 50% with BRF 50%: {}",
            out.brake
        );
        // Default plan: saturation 1.0 — no behaviour change.
        let mut pipe = ProcessingPipeline::new(plain_plan());
        let out = pipe.process(&frame(&[(2, 1050)], &[], 0));
        assert!(approx(out.brake, 0.25));
    }

    #[test]
    fn set_plan_live_swaps_plan_without_resetting_state() {
        // L4: a live tuning push must not restart smoothing. Drive
        // the accelerator filter to ~1.0, swap the plan live, then
        // drop the input to 0: with kept state the output decays
        // gradually; a reset filter would jump straight to 0.0.
        let mut plan = plain_plan();
        plan.accelerator.smoothing = 0.5;
        let mut pipe = ProcessingPipeline::new(plan);
        for _ in 0..40 {
            let _ = pipe.process(&frame(&[(1, 3300)], &[], 0));
        }
        let before = pipe.process(&frame(&[(1, 3300)], &[], 0));
        assert!(before.accelerator > 0.95, "filtered up: {before:?}");
        let mut next = plain_plan();
        next.accelerator.smoothing = 0.5;
        next.accelerator.deadzone = 0.02;
        pipe.set_plan_live(next);
        let after = pipe.process(&frame(&[(1, 300)], &[], 0));
        assert!(
            after.accelerator > 0.5,
            "live swap must keep filter state (decays, not jumps): {after:?}"
        );
        assert_eq!(pipe.plan().accelerator.deadzone, 0.02);
        // …while the live path still sanitizes.
        let mut insane = plain_plan();
        insane.accelerator.deadzone = 2.0;
        pipe.set_plan_live(insane);
        assert_eq!(pipe.plan().accelerator.deadzone, 0.5);
    }

    #[test]
    fn button_debounce_ticks_convert() {
        use super::{button_debounce_ticks, button_double_press_ticks, button_long_press_ticks};
        assert_eq!(button_debounce_ticks(0.0), 0);
        assert_eq!(button_debounce_ticks(10.0), 2);
        assert_eq!(button_debounce_ticks(50.0), 10);
        assert_eq!(button_debounce_ticks(60.0), 10);
        assert_eq!(button_long_press_ticks(800.0), 160);
        assert_eq!(button_long_press_ticks(300.0), 60);
        assert_eq!(button_long_press_ticks(1500.0), 300);
        assert_eq!(button_long_press_ticks(100.0), 60);
        assert_eq!(button_double_press_ticks(300.0), 60);
        assert_eq!(button_double_press_ticks(150.0), 30);
        assert_eq!(button_double_press_ticks(500.0), 100);
        assert_eq!(button_double_press_ticks(100.0), 30);
        assert_eq!(button_double_press_ticks(600.0), 100);
    }

    #[test]
    fn button_debounce_filters_bounce_and_passes_steady_press() {
        // BU1: 10 ms default = 2 ticks. Bit 3 is a wheel button
        // (bits 0/1/2 are shifter/horn and skip the stage).
        let mut pipe = ProcessingPipeline::new(plain_plan());
        let _ = pipe.process(&frame(&[], &[], 0)); // baseline
        // A 1-tick blip never reaches the output.
        let out = pipe.process(&frame(&[], &[], 1 << 3));
        assert_eq!(out.buttons, 0);
        let out = pipe.process(&frame(&[], &[], 0));
        assert_eq!(out.buttons, 0);
        // Two steady ticks: the press lands on the second.
        let out = pipe.process(&frame(&[], &[], 1 << 3));
        assert_eq!(out.buttons, 0);
        let out = pipe.process(&frame(&[], &[], 1 << 3));
        assert_eq!(out.buttons, 1 << 3);
        // A 1-tick drop while held is a bounce, not a release.
        let out = pipe.process(&frame(&[], &[], 0));
        assert_eq!(out.buttons, 1 << 3);
        let out = pipe.process(&frame(&[], &[], 1 << 3));
        assert_eq!(out.buttons, 1 << 3);
        // Two steady ticks released: the release lands.
        let out = pipe.process(&frame(&[], &[], 0));
        assert_eq!(out.buttons, 1 << 3);
        let out = pipe.process(&frame(&[], &[], 0));
        assert_eq!(out.buttons, 0);
    }

    #[test]
    fn button_debounce_counts_skipped_frames() {
        // P3-5: BU1 debounce gap ko fast-forward karta hai — 10 ms
        // debounce (2 ticks) + 5 lost frames ke baad press foran stable hai.
        let mut plan = plain_plan();
        plan.buttons.debounce_ms = 10.0;
        let mut pipe = ProcessingPipeline::new(plan);
        // Bit 3 (wheel button) — pehla frame: candidate 1 tick,
        // abhi stable nahi.
        let out = pipe.process(&frame(&[], &[], 1 << 3));
        assert_eq!(out.buttons, 0);
        // Gap frame (5 lost): candidate 1 + 1 + 5 = 7 >= 2 -> stable.
        let out = pipe.process_with_gap(&frame(&[], &[], 1 << 3), 5);
        assert_eq!(out.buttons, 1 << 3, "gap fast-forwards debounce");
        // Gap ke baghair: doosra steady frame bhi chahiye hota.
        let mut pipe = ProcessingPipeline::new(plan);
        let out = pipe.process(&frame(&[], &[], 1 << 3));
        assert_eq!(out.buttons, 0);
        let out = pipe.process(&frame(&[], &[], 1 << 3));
        assert_eq!(out.buttons, 1 << 3);
    }

    #[test]
    fn button_debounce_zero_is_passthrough_and_excluded_bits_skip() {
        // BU1: 0 ms passes wheel buttons through on the first tick.
        let mut plan = plain_plan();
        plan.buttons.debounce_ms = 0.0;
        let mut pipe = ProcessingPipeline::new(plan);
        let out = pipe.process(&frame(&[], &[], 1 << 3));
        assert_eq!(out.buttons, 1 << 3);
        // Excluded bits (shifter 0/1, horn 2) pass through even at
        // the 50 ms maximum — the firmware already debounces them.
        let mut plan = plain_plan();
        plan.buttons.debounce_ms = 50.0;
        let mut pipe = ProcessingPipeline::new(plan);
        let out = pipe.process(&frame(&[], &[], 0b111));
        assert_eq!(out.buttons, 0b111);
        assert!(out.horn);
    }

    #[test]
    fn button_long_press_sets_after_threshold_and_clears_on_release() {
        // BU2: default 800 ms = 160 ticks. A short hold never sets
        // the long-press bit; holding past the threshold does; the
        // release clears it on the next tick.
        use super::button_long_press_ticks;
        let mut plan = plain_plan();
        plan.buttons.debounce_ms = 0.0;
        plan.buttons.long_press_ms = 300.0; // 60 ticks, fast test
        assert_eq!(button_long_press_ticks(300.0), 60);
        let mut pipe = ProcessingPipeline::new(plan);
        let _ = pipe.process(&frame(&[], &[], 0));
        for _ in 0..59 {
            let out = pipe.process(&frame(&[], &[], 1 << 4));
            assert_eq!(out.buttons, 1 << 4);
            assert_eq!(out.long_press_buttons, 0);
        }
        let out = pipe.process(&frame(&[], &[], 1 << 4));
        assert_eq!(out.long_press_buttons, 1 << 4);
        // Still held: the bit stays set.
        let out = pipe.process(&frame(&[], &[], 1 << 4));
        assert_eq!(out.long_press_buttons, 1 << 4);
        // Released: both bits clear at once.
        let out = pipe.process(&frame(&[], &[], 0));
        assert_eq!(out.buttons, 0);
        assert_eq!(out.long_press_buttons, 0);
    }

    #[test]
    fn button_disabled_clears_long_press_and_debounce_state() {
        // BU1+BU2: live-disable while held clears the held press
        // and the long-press bit; re-enabling does not resurrect
        // either (same pattern as the encoder pulse clear).
        let mut plan = plain_plan();
        plan.buttons.debounce_ms = 0.0;
        plan.buttons.long_press_ms = 300.0;
        let mut pipe = ProcessingPipeline::new(plan);
        let _ = pipe.process(&frame(&[], &[], 0));
        for _ in 0..60 {
            let out = pipe.process(&frame(&[], &[], 1 << 4));
            assert_eq!(out.buttons, 1 << 4);
        }
        let out = pipe.process(&frame(&[], &[], 1 << 4));
        assert_eq!(out.long_press_buttons, 1 << 4);
        plan.buttons.enabled = false;
        pipe.set_plan_live(plan);
        let out = pipe.process(&frame(&[], &[], 1 << 4));
        assert_eq!(out.buttons, 0);
        assert_eq!(out.long_press_buttons, 0);
        plan.buttons.enabled = true;
        pipe.set_plan_live(plan);
        let out = pipe.process(&frame(&[], &[], 1 << 4));
        assert_eq!(out.long_press_buttons, 0);
    }

    #[test]
    fn button_tuning_sanitizes() {
        let mut plan = plain_plan();
        plan.buttons.debounce_ms = 60.0;
        plan.buttons.long_press_ms = 100.0;
        plan.buttons.double_press_ms = 600.0;
        let pipe = ProcessingPipeline::new(plan);
        assert_eq!(pipe.plan().buttons.debounce_ms, 50.0);
        assert_eq!(pipe.plan().buttons.long_press_ms, 300.0);
        assert_eq!(pipe.plan().buttons.double_press_ms, 500.0);
        let mut plan = plain_plan();
        plan.buttons.debounce_ms = f32::NAN;
        plan.buttons.long_press_ms = f32::NAN;
        plan.buttons.double_press_ms = f32::NAN;
        let pipe = ProcessingPipeline::new(plan);
        assert_eq!(pipe.plan().buttons.debounce_ms, 10.0);
        assert_eq!(pipe.plan().buttons.long_press_ms, 800.0);
        assert_eq!(pipe.plan().buttons.double_press_ms, 300.0);
    }

    #[test]
    fn button_double_press_sets_within_window_and_clears_on_release() {
        // BU3: default 300 ms = 60 ticks. The first press is never
        // a double-press; a second press starting within the window
        // of the previous release sets the bit for the whole hold;
        // the release clears it.
        let mut plan = plain_plan();
        plan.buttons.debounce_ms = 0.0;
        plan.buttons.double_press_ms = 300.0;
        let mut pipe = ProcessingPipeline::new(plan);
        let _ = pipe.process(&frame(&[], &[], 0)); // baseline
        // First press: single.
        let out = pipe.process(&frame(&[], &[], 1 << 4));
        assert_eq!(out.buttons, 1 << 4);
        assert_eq!(out.double_press_buttons, 0);
        // Release, then 20 ticks of rest (inside the 60-tick window).
        let _ = pipe.process(&frame(&[], &[], 0));
        for _ in 0..20 {
            let _ = pipe.process(&frame(&[], &[], 0));
        }
        // Second press: double, and it stays set while held.
        let out = pipe.process(&frame(&[], &[], 1 << 4));
        assert_eq!(out.buttons, 1 << 4);
        assert_eq!(out.double_press_buttons, 1 << 4);
        let out = pipe.process(&frame(&[], &[], 1 << 4));
        assert_eq!(out.double_press_buttons, 1 << 4);
        // Release clears both.
        let out = pipe.process(&frame(&[], &[], 0));
        assert_eq!(out.buttons, 0);
        assert_eq!(out.double_press_buttons, 0);
    }

    #[test]
    fn button_double_press_ignored_outside_window() {
        // BU3: a second press starting after the window is a fresh
        // single press.
        let mut plan = plain_plan();
        plan.buttons.debounce_ms = 0.0;
        plan.buttons.double_press_ms = 300.0; // 60 ticks
        let mut pipe = ProcessingPipeline::new(plan);
        let _ = pipe.process(&frame(&[], &[], 0));
        let _ = pipe.process(&frame(&[], &[], 1 << 4));
        let _ = pipe.process(&frame(&[], &[], 0));
        // 61 ticks of rest: the next press starts 62 ticks after
        // the release, outside the 60-tick window.
        for _ in 0..61 {
            let _ = pipe.process(&frame(&[], &[], 0));
        }
        let out = pipe.process(&frame(&[], &[], 1 << 4));
        assert_eq!(out.buttons, 1 << 4);
        assert_eq!(out.double_press_buttons, 0);
    }

    #[test]
    fn default_plan_has_clutch_disabled() {
        let plan = PipelinePlan::default();
        assert!(!plan.clutch.enabled);
        assert!(plan.steering.enabled);
        assert!(!plan.combine_pedals);
        // Clutch mV is ignored while disabled.
        let mut pipe = ProcessingPipeline::new(plan);
        let out = pipe.process(&frame(&[(3, 3300)], &[], 0));
        assert!(approx(out.clutch, 0.0));
        // DT_S sanity: 200 Hz tick.
        assert!(approx(DT_S, 0.005));
    }
}
