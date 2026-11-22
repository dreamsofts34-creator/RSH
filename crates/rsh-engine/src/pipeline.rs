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
            bite_point: 0.0,
            // --- Brake per-module tuning ---
            saturation: 1.0,
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
        // --- Brake per-module tuning ---
        out.saturation = clamp_finite(self.saturation, 0.5, 1.0, 1.0);
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
    /// Wheel buttons 32–63. Always `0` from the pipeline: the stream
    /// packet carries only buttons 0–31, and `btnsEx` arrives via the
    /// status packet, which the engine layer merges — not the pipeline.
    pub buttons_ex: u32,
    /// Momentary encoder-button pulses, buttons 65–72 (see
    /// [`EncoderButtons`]). `0` when the encoders module is disabled.
    pub encoder_buttons: EncoderButtons,
    /// Per-encoder wrap-aware deltas observed this tick (index `i` =
    /// encoder `i`). `0` when the encoders module is disabled.
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
    /// per tick (`apply_smoothing`), so kept state stays valid.
    /// Profile switches and engine start use [`Self::set_plan`]
    /// (full reset) instead.
    pub fn set_plan_live(&mut self, plan: PipelinePlan) {
        self.plan = plan.sanitized();
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
    #[must_use]
    pub fn process_with_gap(&mut self, frame: &StreamFrame, skipped: u16) -> OutputState {
        let mut out = OutputState::neutral();

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
        );
        let brake_tuning = self.plan.brake;
        out.brake = process_pedal_axis(
            frame.axes_mv[BRAKE_SLOT],
            &brake_tuning,
            &mut self.brake_filter,
            &mut self.brake_ramp,
        );
        let clutch_tuning = self.plan.clutch;
        out.clutch = process_pedal_axis(
            frame.axes_mv[CLUTCH_SLOT],
            &clutch_tuning,
            &mut self.clutch_filter,
            &mut self.clutch_ramp,
        );
        let handbrake_tuning = self.plan.handbrake;
        out.handbrake = process_pedal_axis(
            frame.axes_mv[HANDBRAKE_SLOT],
            &handbrake_tuning,
            &mut self.handbrake_filter,
            &mut self.handbrake_ramp,
        );

        if self.plan.combine_pedals {
            out.pedals_combined = (out.accelerator - out.brake).clamp(-1.0, 1.0);
        }

        self.process_buttons(frame, skipped, &mut out);
        out
    }

    /// Buttons, gear decode, horn, and encoders.
    fn process_buttons(&mut self, frame: &StreamFrame, skipped: u16, out: &mut OutputState) {
        let mut buttons = frame.buttons;

        // Shifter: H-pattern gear decode (bits 5–10) or sequential
        // up/down (bits 0/1). Gear bits never reach the button word.
        match self.plan.shifter.gate_mode {
            ShifterGateMode::HPattern => {
                out.gear = if self.plan.shifter.enabled {
                    decode_gear(buttons)
                } else {
                    0
                };
                buttons &= !GEAR_BITS_MASK;
            }
            ShifterGateMode::Sequential => {
                if !self.plan.shifter.enabled {
                    buttons &= !SHIFTER_SEQ_BITS;
                }
            }
        }

        // Horn (BUTTON slot 2, bit 2).
        out.horn = self.plan.horn.enabled && (buttons & HORN_BIT) != 0;
        if !self.plan.horn.enabled {
            buttons &= !HORN_BIT;
        }

        // Wheel buttons: a disabled module emits nothing.
        out.buttons = if self.plan.buttons.enabled {
            buttons
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
            out.encoder_deltas = deltas;
            out.encoder_buttons = deltas_to_buttons(&deltas);
        }
    }
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

/// Brake-force (BRF) rescale for a mapped pedal value in `0..=1`:
/// `saturation` is the travel fraction that gives 100% brake, so
/// the output is `value / saturation`, clamped to `0..=1`. A
/// saturation of `1.0` is the identity. Values outside `0.5..=1.0`
/// clamp (a sanitized plan never sends them, but the stage must not
/// amplify an insane input — and `0.0` would divide by zero).
#[must_use]
pub fn apply_saturation(value: f32, saturation: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
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
/// filter's base cutoff follows the documented smoothing mapping
/// (see [`OneEuroFilter::from_smoothing`]).
#[must_use]
pub fn apply_smoothing(filter: &mut OneEuroFilter, value: f32, smoothing: f32) -> f32 {
    if !(smoothing.is_finite() && smoothing > 0.0) {
        filter.set_enabled(false);
        return value;
    }
    filter.set_enabled(true);
    filter.set_min_cutoff(0.5 + 9.5 * (1.0 - smoothing.clamp(0.0, 1.0)));
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
/// curve → smooth → ramp. Returns `0.0` (and resets filter and
/// ramp) when the module is disabled or the axis is missing.
fn process_pedal_axis(
    mv: Option<u16>,
    tuning: &ModuleTuning,
    filter: &mut OneEuroFilter,
    ramp: &mut RampLimiter,
) -> f32 {
    if !tuning.enabled {
        filter.reset();
        ramp.reset();
        return 0.0;
    }
    let Some(mv) = mv else {
        filter.reset();
        ramp.reset();
        return 0.0;
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
    value = apply_bite_point(value, tuning.bite_point);
    value = apply_deadzone_unipolar(value, tuning.deadzone);
    value = apply_curve(value, tuning.curve_strength);
    value = apply_smoothing(filter, value, tuning.smoothing);
    ramp.update(value, DT_S, tuning.press_ms, tuning.release_ms)
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
        apply_steering_scale, decode_gear, map_pedal_mv, map_steering_mv,
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
        let out = pipe.process(&frame(&[], &[32767], 0));
        assert_eq!(out.encoder_deltas[0], 0); // first sample: baseline
        let out = pipe.process(&frame(&[], &[-32768], 0));
        assert_eq!(out.encoder_deltas[0], 1);
        assert!(out.encoder_buttons.clockwise(0));
        let out = pipe.process(&frame(&[], &[32767], 0));
        assert_eq!(out.encoder_deltas[0], -1);
        assert!(out.encoder_buttons.counter_clockwise(0));
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
