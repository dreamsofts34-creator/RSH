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

use rsh_protocol::stream::StreamFrame;

use crate::calibration::{
    DEFAULT_CENTER_MV, DEFAULT_MAX_MV, DEFAULT_MIN_MV, validate_pedal_range,
    validate_steering_range,
};

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
/// Stream `buttons` bit for the horn.
///
/// HID P1-1 (polish round): the firmware sends horn on bit 4
/// (`firmware/modules.h` `RSH_BTN2_HORN 4`, asserted by the
/// firmware host tests) while the pipeline used to read bit 2 —
/// the firmware's accel-button bit — so a horn press did nothing
/// and a pedal press honked. The canonical assignment is pinned
/// in `docs/PROTOCOL-V2.2.md` §1.1: bit 0 shift-up, 1 shift-down,
/// 2 accel-button, 3 brake-button, 4 horn, 5–10 H-pattern gears.
/// The debounce-exclusion mask below follows this constant, so it
/// stays correct if the assignment ever moves.
pub const HORN_BIT: u32 = 1 << 4;
/// Stream `buttons` bits for the pedal buttons (firmware fixed
/// pins 25/26): bit 2 accel-button, bit 3 brake-button. HID P1-2
/// (polish round): these are pedal inputs, never wheel buttons —
/// the pipeline strips them from the button word (the firmware
/// drives the pedal axis from them in button mode), so they can
/// never reach the game as buttons 3/4 or trip the horn logic.
pub const PEDAL_BUTTON_BITS: u32 = (1 << 2) | (1 << 3);
/// Stream `buttons` bits for sequential shifter up/down (BUTTON slots 0/1).
pub const SHIFTER_SEQ_BITS: u32 = 0b11;

/// Shifter gate mode (per-module tuning, only meaningful for the
/// shifter module; other modules ignore it). v2.0.19: sequential-only -
/// the H-pattern mode was deleted with the buttons/encoders cleanup.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ShifterGateMode {
    /// Up/down buttons (BUTTON slots 0/1).
    #[default]
    Sequential,
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
/// `invert`, handbrake latch + auto-release) —
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
    /// One Euro smoothing, `0..=1` (0 = off). Default `0.0`
    /// (v2.0.21 P2 — the old `0.3` lagged the primary axis; the
    /// 0.1 µF cap + trimmed mean handle the noise instead).
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
    /// double-taps in sequential.
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
            // v2.0.21 P2: smoothing off by default (was 0.3). The
            // per-module press/release ramps stay at the legacy
            // 150/100 ms here — this struct default is
            // module-agnostic, so the per-module values live in
            // `rsh_api::{default_press_ms, default_release_ms}`
            // (throttle 100/50, brake 0/50, clutch 50/300), which
            // `get_tuning` applies before the plan is built.
            smoothing: 0.0,
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
        out.smoothing = clamp_finite(self.smoothing, 0.0, 1.0, 0.0);
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
    /// Horn tuning (stream `buttons` bit 4 — see [`HORN_BIT`]).
    pub horn: ModuleTuning,
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
            combine_pedals: false,
        }
    }
}

/// The processed output of one tick. Consumed by the B1 engine worker
/// and the output backends (`vJoy` / `Xbox`).
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
    /// Wheel buttons, bit *n* = button *n*. v2.0.19: only bits 0/1
    /// (sequential shifter up/down) and 4 (horn) ever survive - the
    /// pipeline strips every other bit (pedal buttons 2/3, the
    /// deleted H-pattern gear bits 5.10, and all other wheel
    /// buttons). Gating moved to the shifter/horn modules when the
    /// buttons module was deleted; `0` when both are disabled.
    pub buttons: u32,
    /// True when the horn button (stream `buttons` bit 4) is pressed
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
    /// per-tick steps configured at plan time, like the filter cutoff) was tried and measured slightly
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
/// per-axis filter and ramp state. Allocation-free in the
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
    /// SH2 lockout ticks left (incl. the current tick's countdown):
    /// a new gear / up-down edge is ignored while this is nonzero.
    /// One tick is 5 ms at 200 Hz.
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
    /// full deflection). Buttons keep processing the
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
    /// module's `smoothing` value; ramps start empty.
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

    /// Replace the plan. All filter and ramp state is reset:
    /// a new plan must never inherit stale smoothing.
    pub fn set_plan(&mut self, plan: PipelinePlan) {
        *self = Self::new(plan);
    }

    /// Replace the plan for a live tuning change (L4 pipeline push),
    /// WITHOUT resetting filter or ramp state: a slider drag
    /// must take effect on the next tick, not restart the smoothing
    /// and ramps (which would step the output). The plan is still
    /// sanitized on entry. Filter cutoffs track the new smoothing
    /// here ([`OneEuroFilter::configure`]), so kept state stays
    /// valid and the per-tick path does not re-derive them.
    /// Profile switches and engine start use [`Self::set_plan`]
    /// (full reset) instead.
    pub fn set_plan_live(&mut self, plan: PipelinePlan) {
        self.plan = plan.sanitized();
        self.steering_filter.configure(self.plan.steering.smoothing);
        self.accel_filter.configure(self.plan.accelerator.smoothing);
        self.brake_filter.configure(self.plan.brake.smoothing);
        self.clutch_filter.configure(self.plan.clutch.smoothing);
        self.handbrake_filter
            .configure(self.plan.handbrake.smoothing);
    }

    /// Forget all filter and ramp state (same plan).
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
    /// (v2.0.19: the gap no longer feeds any stage - the encoder
    /// glitch filter it used to scale was deleted with the
    /// encoders, so `skipped` is accepted and ignored.)
    ///
    /// P2-1: when `frame.flags.axis_invalid` is set, the suspect
    /// axis sample is untrusted (the board flags a rail-stuck /
    /// failed axis, whose value sits at full deflection) — that axis
    /// holds its last good output instead of being driven from it.
    ///
    /// v2.0.20 (RC-1): the hold is PER-AXIS. The wire carries only
    /// one bit (protocol v2.2 frozen — no per-axis mask), so the
    /// suspect is identified from the frame itself: an axis whose
    /// sample sits at rail (0 or `DEFAULT_MAX_MV` mV) is the one the
    /// board's rail-stuck criterion fires on, and only it is held;
    /// every other axis is processed live — a released handbrake at
    /// rail 0 used to freeze the steering too. When NO axis is at
    /// rail the flag must come from jitter or an I2C failure, which
    /// no frame value can identify, so every present axis is held
    /// (the old behaviour) rather than trusting an unknown suspect. A missing
    /// axis (`None`) is never held — there is no sample to distrust,
    /// and the axis processors already emit neutral for it.
    /// Filters/ramps are not fed a held sample, so resuming after
    /// the flag clears continues from the held state. Buttons still
    /// process the live frame (the flag is about axes only). Before
    /// any good frame the hold is neutral.
    #[must_use]
    pub fn process_with_gap(&mut self, frame: &StreamFrame, skipped: u16) -> OutputState {
        let mut out = OutputState::neutral();

        // NOTE (bench-gate fix attempt): reading the tunings by
        // reference instead of copying them was measured SLOWER
        // (+21 ns/tick on criterion) — the local copies scalarize
        // into registers, while the reference re-loads tuning
        // fields from memory on every access. Copies stay.
        //
        // v2.0.20 (RC-1): per-axis P2-1 hold — `held[i]` is true
        // only for the suspect axis (see `invalid_hold_axes`).
        // A held axis keeps its last good output and its filter /
        // ramp are not fed; every other axis is processed live and
        // refreshes its own last-good slot.
        let held = invalid_hold_axes(frame);
        let steering_tuning = self.plan.steering;
        if held[STEERING_SLOT] {
            out.steering = self.last_good_axes[STEERING_SLOT];
        } else {
            out.steering = process_steering_axis(
                frame.axes_mv[STEERING_SLOT],
                &steering_tuning,
                &mut self.steering_filter,
            );
            self.last_good_axes[STEERING_SLOT] = out.steering;
        }
        let accel_tuning = self.plan.accelerator;
        if held[ACCEL_SLOT] {
            out.accelerator = self.last_good_axes[ACCEL_SLOT];
        } else {
            out.accelerator = process_pedal_axis(
                frame.axes_mv[ACCEL_SLOT],
                &accel_tuning,
                &mut self.accel_filter,
                &mut self.accel_ramp,
            )
            .0;
            self.last_good_axes[ACCEL_SLOT] = out.accelerator;
        }
        let brake_tuning = self.plan.brake;
        if held[BRAKE_SLOT] {
            out.brake = self.last_good_axes[BRAKE_SLOT];
        } else {
            out.brake = process_pedal_axis(
                frame.axes_mv[BRAKE_SLOT],
                &brake_tuning,
                &mut self.brake_filter,
                &mut self.brake_ramp,
            )
            .0;
            self.last_good_axes[BRAKE_SLOT] = out.brake;
        }
        let clutch_tuning = self.plan.clutch;
        if held[CLUTCH_SLOT] {
            out.clutch = self.last_good_axes[CLUTCH_SLOT];
        } else {
            out.clutch = process_pedal_axis(
                frame.axes_mv[CLUTCH_SLOT],
                &clutch_tuning,
                &mut self.clutch_filter,
                &mut self.clutch_ramp,
            )
            .0;
            self.last_good_axes[CLUTCH_SLOT] = out.clutch;
        }
        let handbrake_tuning = self.plan.handbrake;
        // A held handbrake feeds the latch its held output for both
        // value and pressed (same 0..=1 domain) — the latch state
        // freezes with the output instead of toggling on a suspect
        // sample. The last-good slot takes the LATCHED output below
        // (as before), never the raw pedal value.
        let (handbrake_value, handbrake_pressed) = if held[HANDBRAKE_SLOT] {
            let held_value = self.last_good_axes[HANDBRAKE_SLOT];
            (held_value, held_value)
        } else {
            process_pedal_axis(
                frame.axes_mv[HANDBRAKE_SLOT],
                &handbrake_tuning,
                &mut self.handbrake_filter,
                &mut self.handbrake_ramp,
            )
        };
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
        if !held[HANDBRAKE_SLOT] {
            self.last_good_axes[HANDBRAKE_SLOT] = out.handbrake;
        }

        if self.plan.combine_pedals {
            out.pedals_combined = (out.accelerator - out.brake).clamp(-1.0, 1.0);
        }

        self.process_buttons(frame, skipped, &mut out);
        out
    }

    /// Shifter (sequential up/down, bits 0/1) and horn (bit 4).
    ///
    /// v2.0.19: the buttons/encoders cleanup deleted every other
    /// button source - the word that leaves here carries ONLY bits
    /// 0/1 (shifter) and 4 (horn). Pedal-button bits 2/3 are
    /// stripped (never wheel buttons), gear bits 5-10 and all other
    /// wheel-button bits are stripped with the deleted modules, and
    /// the BU1/BU2/BU3 stages are gone (bits 0/1/4 were already
    /// firmware-debounced and skipped them). The horn HO1 minimum
    /// honk + HO2 latch still hold bit 4 in the word while honking,
    /// so the game sees button 5 held.
    ///
    /// Never inlined into `process_with_gap`: the button block is
    /// large, and inlining it balloons the axis hot path
    /// (register pressure / icache) - measured on criterion.
    #[inline(never)]
    fn process_buttons(&mut self, frame: &StreamFrame, _skipped: u16, out: &mut OutputState) {
        let mut buttons = frame.buttons;

        // Shifter: sequential up/down (bits 0/1) only - H-pattern
        // was deleted in v2.0.19. SH2: the lockout ignores a new
        // up-down edge within `shift_lockout_ms` of the last
        // accepted shift (sequential double-taps). The window counts
        // in 5 ms ticks and ticks down once per processed frame.
        if self.plan.shifter.enabled {
            buttons = self.apply_shift_lockout_seq(buttons);
        } else {
            buttons &= !SHIFTER_SEQ_BITS;
            self.shift_prev_seq = 0;
            self.shift_suppressed_seq = 0;
            self.shift_lockout_left = 0;
        }

        // HID P1-2 (polish round): pedal-button bits are not wheel
        // buttons - strip them before output so a digital pedal
        // press never reaches the game as button 3/4 (and, before
        // the HORN_BIT fix, never honked). The firmware drives the
        // pedal axis from these bits in button mode.
        buttons &= !PEDAL_BUTTON_BITS;

        // Horn (BUTTON slot 2, bit 4 - see [`HORN_BIT`]) - HO1
        // minimum honk + HO2 latch. Momentary: the output follows
        // the button, and a press re-arms the hold to the full
        // minimum on every pressed tick, so the honk lasts at least
        // `horn_min_ms` after release. Latch: each rising edge
        // toggles the horn on/off (the hold does not apply - a
        // latched honk stays on until toggled off, it must not tail
        // after switch-off). The held/latched horn keeps bit 4 set
        // in the button word too, so the game sees button 5 held -
        // the `horn` flag alone would not honk in-game. Disabled
        // clears the hold, the latch and the bit.
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

        // Only bits 0/1 (shifter) and 4 (horn) survive: every other
        // bit is a deleted input (pedal buttons 2/3, gear bits
        // 5-10, wheel buttons) and is stripped here, so a stale
        // firmware bit can never reach a game as a phantom button.
        out.buttons = buttons & (SHIFTER_SEQ_BITS | HORN_BIT);
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

/// Shift-lockout window in ticks (SH2): one tick is 5 ms at
/// 200 Hz, so the 0–200 ms range is 0–40 ticks (`0` = off). The
/// clamp to `0..=40` makes both casts safe.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn shift_lockout_ticks(lockout_ms: f32) -> u8 {
    ((lockout_ms / 5.0).round().clamp(0.0, 40.0)) as u8
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

/// v2.0.20 (RC-1): which axes P2-1 holds on this frame, in slot
/// order (steering, accel, brake, clutch, handbrake). No flag, no
/// hold. Flag set: only the axes whose sample sits at rail (0 or
/// `DEFAULT_MAX_MV` mV) are held — the board's rail-stuck
/// criterion fires on exactly those. No axis at rail means the flag
/// came from jitter or an I2C failure, which no frame value can
/// identify, so every PRESENT axis is held (the pre-v2.0.20
/// behaviour for present axes). A
/// missing axis (`None`) is never held — there is no sample to
/// distrust, and the axis processors already emit neutral for it;
/// that also covers the fallback: a missing axis stays neutral
/// instead of holding a stale last-good value.
fn invalid_hold_axes(frame: &StreamFrame) -> [bool; 5] {
    if !frame.flags.axis_invalid {
        return [false; 5];
    }
    let slots = [
        STEERING_SLOT,
        ACCEL_SLOT,
        BRAKE_SLOT,
        CLUTCH_SLOT,
        HANDBRAKE_SLOT,
    ];
    let mut held = [false; 5];
    for (i, slot) in slots.iter().enumerate() {
        held[i] = matches!(frame.axes_mv[*slot], Some(v) if v == 0 || v >= DEFAULT_MAX_MV);
    }
    if !held.iter().any(|held| *held) {
        // No axis at rail: hold every PRESENT axis (a missing one
        // stays neutral — see above).
        for (i, slot) in slots.iter().enumerate() {
            held[i] = frame.axes_mv[*slot].is_some();
        }
    }
    held
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
        apply_curve, apply_deadzone_bipolar, apply_deadzone_unipolar, apply_steering_scale,
        map_pedal_mv, map_steering_mv, shift_lockout_ticks,
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
        // v2.0.21 P2: a non-finite smoothing falls back to the new
        // factory default 0.0 (off), not the old 0.3.
        assert_eq!(clean.smoothing, 0.0);
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
        plan.shifter.enabled = false;
        plan.horn.enabled = false;
        let mut pipe = ProcessingPipeline::new(plan);
        // Every button bit set: only shifter 0/1 + horn 4 could ever
        // survive, and both modules are disabled here - the word is 0.
        let out = pipe.process(&frame(&[(0, 3300)], &[100], 0xFFFF_FFFF));
        assert!(approx(out.steering, 0.0));
        assert_eq!(out.buttons, 0);
        assert!(!out.horn);
        // But the accelerator (still enabled) passes through.
        let out = pipe.process(&frame(&[(1, 3300)], &[], 0));
        assert!(approx(out.accelerator, 1.0));
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
        // 0 ms = off: rapid sequential taps all pass through.
        let mut plan = plain_plan();
        plan.shifter.shift_lockout_ms = 0.0;
        let mut pipe = ProcessingPipeline::new(plan);
        let out = pipe.process(&frame(&[], &[], 0b01));
        assert_eq!(out.buttons, 0b01);
        let out = pipe.process(&frame(&[], &[], 0b10));
        assert_eq!(out.buttons, 0b10);
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
        let out = pipe.process(&frame(&[], &[], 0b10011));
        assert_eq!(out.buttons, 0b10011);
        assert!(out.horn);

        let mut plan = plain_plan();
        plan.shifter.enabled = false;
        plan.horn.enabled = false;
        let mut pipe = ProcessingPipeline::new(plan);
        let out = pipe.process(&frame(&[], &[], 0b10011));
        assert_eq!(out.buttons, 0);
        assert!(!out.horn);
    }

    #[test]
    fn horn_min_hold_keeps_honk_after_a_quick_tap() {
        // HO1: a one-tick tap (5 ms) with a 400 ms minimum holds
        // the horn — flag + button bit — for 80 ticks after release,
        // then goes quiet. The game polls button 5, so the bit must
        // stay set for the whole hold, not just the `horn` flag.
        let mut plan = plain_plan();
        plan.horn.horn_min_ms = 400.0;
        let mut pipe = ProcessingPipeline::new(plan);
        let out = pipe.process(&frame(&[], &[], 0b1_0000));
        assert!(out.horn);
        assert_eq!(out.buttons & 0b1_0000, 0b1_0000);
        // 79 released ticks: still honking (the release tick counts
        // as the first of the 80-tick hold).
        for _ in 0..79 {
            let out = pipe.process(&frame(&[], &[], 0));
            assert!(out.horn);
            assert_eq!(out.buttons & 0b1_0000, 0b1_0000);
        }
        let out = pipe.process(&frame(&[], &[], 0));
        assert!(!out.horn);
        assert_eq!(out.buttons & 0b1_0000, 0);
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
    fn horn_bit4_canonical_and_pedal_bits_stripped() {
        // HID P1-1/P1-2 (polish round): the canonical stream
        // assignment is horn = bit 4, accel pedal = bit 2, brake
        // pedal = bit 3 (firmware modules.h + PROTOCOL-V2.2 §1.1).
        // A horn press honks and keeps bit 4 in the button word;
        // pedal bits never honk and never reach the game as
        // buttons 3/4 — the firmware drives the pedal axis from
        // them in button mode.
        let mut pipe = ProcessingPipeline::new(plain_plan());
        let out = pipe.process(&frame(&[], &[], 1 << 4));
        assert!(out.horn);
        assert_eq!(out.buttons, 1 << 4);
        // Fresh pipeline per probe: the horn min-hold keeps honking
        // for 80 ticks after a horn press, so a reused pipeline
        // would report `horn` on the pedal frames too.
        let mut pipe = ProcessingPipeline::new(plain_plan());
        let out = pipe.process(&frame(&[], &[], 1 << 2));
        assert!(!out.horn, "accel pedal bit must not honk");
        assert_eq!(out.buttons, 0, "accel pedal bit must not leak as a button");
        let mut pipe = ProcessingPipeline::new(plain_plan());
        let out = pipe.process(&frame(&[], &[], 1 << 3));
        assert!(!out.horn, "brake pedal bit must not honk");
        assert_eq!(out.buttons, 0, "brake pedal bit must not leak as a button");
        // All three at once: horn survives, pedal bits are gone.
        let mut pipe = ProcessingPipeline::new(plain_plan());
        let out = pipe.process(&frame(&[], &[], (1 << 2) | (1 << 3) | (1 << 4)));
        assert!(out.horn);
        assert_eq!(out.buttons, 1 << 4);
    }

    #[test]
    fn horn_latch_toggles_on_each_press() {
        // HO2: press once = on (stays on with no button held),
        // press again = off. A held press does not re-toggle —
        // only the rising edge counts.
        let mut plan = plain_plan();
        plan.horn.horn_latch = true;
        let mut pipe = ProcessingPipeline::new(plan);
        let out = pipe.process(&frame(&[], &[], 0b1_0000));
        assert!(out.horn);
        let out = pipe.process(&frame(&[], &[], 0b1_0000));
        assert!(out.horn, "holding the button must not toggle off");
        let out = pipe.process(&frame(&[], &[], 0));
        assert!(out.horn, "latched honk stays on after release");
        assert_eq!(out.buttons & 0b1_0000, 0b1_0000);
        let out = pipe.process(&frame(&[], &[], 0b1_0000));
        assert!(!out.horn, "second press toggles off");
        assert_eq!(out.buttons & 0b1_0000, 0);
        let out = pipe.process(&frame(&[], &[], 0));
        assert!(!out.horn, "no hold tail after a latch switch-off");
        // Live-disable clears a latched honk; re-enable stays quiet.
        let out = pipe.process(&frame(&[], &[], 0b1_0000));
        assert!(out.horn);
        plan.horn.enabled = false;
        pipe.set_plan_live(plan);
        let out = pipe.process(&frame(&[], &[], 0b1_0000));
        assert!(!out.horn);
        assert_eq!(out.buttons & 0b1_0000, 0);
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
        let out = pipe.process(&frame(&[(0, 3300)], &[], 0));
        assert!(approx(out.steering, 1.0));
        pipe.set_plan(plain_plan());
        // Filter state was reset: a neutral sample reads neutral.
        let out = pipe.process(&frame(&[(0, 1800)], &[], 0));
        assert!(approx(out.steering, 0.0));
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
        // Buttons still live: horn bit (1 << 4) on an invalid frame honks.
        let bad_horn = StreamFrame::new(3, 5000, flags, &[(0, 300)], &[], 1 << 4);
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
    fn axis_invalid_flag_holds_only_the_railed_axis() {
        // v2.0.20 (RC-1): handbrake at rail (0, released) + steering
        // moving + axis_invalid set — steering must keep flowing live
        // (pre-fix it froze at its last good value); only the
        // handbrake is held. With NO axis at rail the flag falls
        // back to holding every present axis (jitter / I2C cause).
        let mut pipe = ProcessingPipeline::new(plain_plan());
        let out = pipe.process(&frame(&[(0, 3300), (1, 1800), (4, 1650)], &[], 0));
        assert!(approx(out.steering, 1.0));
        assert!(approx(out.accelerator, 0.5));
        // Handbrake 1650 mV: (1650-300)/(3300-300) = 0.45.
        assert!(
            approx(out.handbrake, 0.45),
            "handbrake baseline: {}",
            out.handbrake
        );
        let flags = StreamFlags {
            axis_invalid: true,
            ..Default::default()
        };
        // Steering moves to 2550 (0.5), handbrake drops to rail 0.
        let bad = StreamFrame::new(2, 5000, flags, &[(0, 2550), (1, 1800), (4, 0)], &[], 0);
        let out = pipe.process(&bad);
        assert!(
            approx(out.steering, 0.5),
            "steering must follow live: {}",
            out.steering
        );
        assert!(
            approx(out.accelerator, 0.5),
            "accel must follow live: {}",
            out.accelerator
        );
        assert!(
            approx(out.handbrake, 0.45),
            "railed handbrake held: {}",
            out.handbrake
        );
        // No axis at rail: flag set, all values mid-range — everything holds.
        let bad2 = StreamFrame::new(3, 5000, flags, &[(0, 300), (1, 300), (4, 300)], &[], 0);
        let out = pipe.process(&bad2);
        assert!(
            approx(out.steering, 0.5),
            "fallback holds steering: {}",
            out.steering
        );
        assert!(
            approx(out.accelerator, 0.5),
            "fallback holds accel: {}",
            out.accelerator
        );
        assert!(
            approx(out.handbrake, 0.45),
            "fallback holds handbrake: {}",
            out.handbrake
        );
        // Flag clears: new values flow again.
        let out = pipe.process(&frame(&[(0, 300), (4, 3300)], &[], 0));
        assert!(approx(out.steering, -1.0));
        assert!(approx(out.handbrake, 1.0));
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
