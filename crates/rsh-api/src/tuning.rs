//! Per-module tuning + draft wire types (L4, LOGIC-FIX-PLAN §2/§4.4).
//!
//! These are the tuning shapes the Slint UI sends the daemon over IPC
//! ([`AppApi::set_tuning`](crate::AppApi::set_tuning)) and the draft
//! status the UI renders as the orange unsaved dot (D-1). They live
//! here — not in `rsh-app-core` — because `rsh-app-core` already
//! depends on `rsh-api`; the API trait cannot name a type owned by
//! its own implementor. `rsh-app-core::commands` re-exports them, so
//! the Tauri command logic and the daemon keep their existing paths.
//!
//! Units are the tuning screen's units, not the C# profile's:
//! `deadzone` is percent (0–30), `curve` is the response exponent
//! (0.5–2.0, 1.0 = linear), `min_mv`/`max_mv` are millivolts (span
//! must stay ≥ 660 mV), `smoothing` is percent (0–100), `invert`
//! flips the steering direction, `steering_lock_deg` is the
//! steering lock in degrees (one of [`STEERING_LOCK_STEPS_DEG`]),
//! `steering_balance_pct` is the left/right lock balance in percent
//! (−20..=+20, 0 = even), and `steering_center_trim_pct` is the
//! centre trim in percent (−5..=+5, 0 = no trim).

use serde::{Deserialize, Serialize};

/// Steering lock steps in degrees (Bug 7, PC-test fix plan):
/// 270° is the tightest usable lock — below that the wheel gets
/// twitchy — and 900° is full lock.
pub const STEERING_LOCK_STEPS_DEG: [f64; 5] = [270.0, 360.0, 540.0, 720.0, 900.0];

/// Default steering lock in degrees (full lock).
pub const DEFAULT_STEERING_LOCK_DEG: f64 = 900.0;

/// Snap a steering lock in degrees to the nearest
/// [`STEERING_LOCK_STEPS_DEG`] step (non-finite → full lock).
#[must_use]
pub fn snap_steering_lock_deg(deg: f64) -> f64 {
    if !deg.is_finite() {
        return DEFAULT_STEERING_LOCK_DEG;
    }
    let mut best = DEFAULT_STEERING_LOCK_DEG;
    let mut best_dist = f64::INFINITY;
    for step in STEERING_LOCK_STEPS_DEG {
        let dist = (deg - step).abs();
        if dist < best_dist {
            best_dist = dist;
            best = step;
        }
    }
    best
}

/// Map a steering lock in degrees to the engine `scale`
/// (`lock / 900`, clamped `0.2..=1.0`): 900° → 1.0, 540° → 0.6,
/// 270° → 0.3. The lock is snapped first, so any input lands on a
/// real step.
#[must_use]
pub fn steering_lock_deg_to_scale(deg: f64) -> f64 {
    (snap_steering_lock_deg(deg) / DEFAULT_STEERING_LOCK_DEG).clamp(0.2, 1.0)
}

/// Default pedal press-ramp ms for a module (per-module tuning,
/// Workstream C): the accelerator opens over 100 ms, the brake
/// bites instantly (0 ms, v2.0.21 F7 — the 50 ms press was felt
/// lag on every brake input), the clutch opens over 50 ms, the
/// handbrake grabs instantly (0 ms, H2); other modules have no
/// press ramp of their own and fall back to the profile's global
/// `pedal_press_ms` (150 ms).
/// Both spellings of the throttle id are accepted — the UI writes
/// `"throttle"`, the pipeline reads `"accelerator"`.
#[must_use]
pub fn default_press_ms(module: &str) -> Option<f64> {
    match module {
        "accelerator" | "throttle" => Some(100.0),
        "brake" | "handbrake" => Some(0.0),
        "clutch" => Some(50.0),
        _ => None,
    }
}

/// Default pedal release-ramp ms for a module (per-module tuning,
/// Workstream C): the accelerator closes over 50 ms, the brake
/// releases over 50 ms (v2.0.21 F7 — the old 100 ms global made
/// the brake feel sticky on lift-off), the clutch releases over
/// 300 ms; other modules fall back to the profile's global
/// `pedal_release_ms` (100 ms).
#[must_use]
pub fn default_release_ms(module: &str) -> Option<f64> {
    match module {
        "accelerator" | "throttle" | "brake" => Some(50.0),
        "clutch" => Some(300.0),
        _ => None,
    }
}

/// Shifter gate type (tuning extension; the C# `ShifterGateMode`
/// string kept as a typed enum here).
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub enum ShifterGate {
    /// Sequential up/down (buttons).
    /// (v2.0.19: H-pattern is deleted — sequential is the only gate.)
    #[serde(rename = "sequential")]
    Sequential,
}

/// LED pattern for the WS2812 strip (`rsh-led`: OFF / SOLID / BLINK /
/// CHASE / RAINBOW).
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum LedPattern {
    /// Strip off.
    Off,
    /// Solid colour.
    Solid,
    /// Blinking.
    Blink,
    /// Chase / running light.
    Chase,
    /// Rainbow cycle.
    Rainbow,
}

impl LedPattern {
    /// The protocol pattern id (`LedTestPattern.pattern_id`): 0 OFF,
    /// 1 SOLID, 2 BLINK, 3 CHASE, 4 RAINBOW.
    #[must_use]
    pub const fn pattern_id(self) -> u8 {
        match self {
            Self::Off => 0,
            Self::Solid => 1,
            Self::Blink => 2,
            Self::Chase => 3,
            Self::Rainbow => 4,
        }
    }

    /// The lowercase name as stored in profiles (`"off"`, `"solid"`,
    /// …) — the same spelling the serde form uses.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Solid => "solid",
            Self::Blink => "blink",
            Self::Chase => "chase",
            Self::Rainbow => "rainbow",
        }
    }

    /// Parses a stored pattern name (case-insensitive); [`None`] for
    /// an unknown name — the caller keeps `None` instead of guessing.
    #[must_use]
    pub fn parse_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "off" => Some(Self::Off),
            "solid" => Some(Self::Solid),
            "blink" => Some(Self::Blink),
            "chase" => Some(Self::Chase),
            "rainbow" => Some(Self::Rainbow),
            _ => None,
        }
    }
}

/// Per-module tuning (`TuningSettings` in `frontend/src/lib/tauri.ts`).
///
/// Units are the frontend's units, not the C# profile's: `deadzone` is
/// percent (0–30, displayed as %), `curve` is the response exponent
/// (0.5–2.0, 1.0 = linear), `min_mv`/`max_mv` are millivolts (span must
/// stay ≥ 660 mV), `smoothing` is percent (0–100). The C# profile's
/// fraction-based deadzone (0–0.5) is a different scale — do not mix
/// the two; the profile store owns the C# shape, this struct owns the
/// tuning screen's shape.
///
/// The extension fields are all optional: a module that has no bite
/// point, gate, horn button, or LED simply omits them (`None` =
/// "not applicable", never a fake 0).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TuningSettings {
    /// Module id, e.g. `"steering"`, `"brake"`, `"shifter"`, `"led"`.
    pub module: String,
    /// Deadzone in percent, 0–30.
    pub deadzone: f64,
    /// Response curve exponent, 0.5–2.0 (1.0 = linear).
    pub curve: f64,
    /// Raw minimum in mV; span `max_mv - min_mv` must stay ≥ 660 mV.
    pub min_mv: f64,
    /// Raw maximum in mV.
    pub max_mv: f64,
    /// Smoothing in percent, 0–100 (One Euro factor).
    pub smoothing: f64,
    /// Invert the steering direction (steering module only;
    /// `None` = not applicable — other modules never carry it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invert: Option<bool>,
    /// Steering lock in degrees (steering module only; one of
    /// [`STEERING_LOCK_STEPS_DEG`]; `None` = not applicable).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steering_lock_deg: Option<f64>,
    // --- Steering per-module tuning ---
    /// Left/right lock balance in percent, −20..=+20 (steering
    /// module only; `None` = not applicable). `0` = even. Positive
    /// favours the left: the left side's scale shrinks, so full
    /// left lock arrives sooner — for a pot that travels less to
    /// the left than to the right. The engine maps it onto
    /// `scale_left` / `scale_right` (S2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steering_balance_pct: Option<f64>,
    /// Centre trim in percent, −5..=+5 (steering module only;
    /// `None` = not applicable). Shifts the effective centre
    /// (`center_mv`) by that percent of the range's half-span,
    /// without a full centre re-capture. `0` = no trim (S3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steering_center_trim_pct: Option<f64>,
    /// Pedal bite point, 0..1 (clutch / brake only; `None` elsewhere).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bite_point: Option<f64>,
    // --- Brake per-module tuning ---
    /// Brake force (BRF), 0.5..=1.0 as a fraction: the travel that
    /// gives 100% brake (brake module only; `None` = 100% / not
    /// applicable — old profiles load as `None`, i.e. full force).
    /// Calibration (`min_mv` / `max_mv`) stays untouched: this only
    /// rescales the mapped travel in the engine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saturation: Option<f64>,
    /// Shifter gate type (shifter module only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shifter_gate: Option<ShifterGate>,
    // --- Shifter per-module tuning ---
    /// Shift lockout in ms (SH2, shifter module only): new gear
    /// inputs within this long of the last accepted shift are
    /// ignored — 0–200 (default 50). `0` = off. Kills double-taps
    /// in sequential shifting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shift_lockout_ms: Option<f64>,
    /// Button number (1–32) the horn is bound to (horn module only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub horn_button: Option<u8>,
    /// LED strip brightness in percent, 0–100 (LED module only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub led_brightness: Option<u8>,
    /// LED strip pattern (LED module only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub led_pattern: Option<LedPattern>,
    /// LED strip colour as `0xRRGGBB` (M2, v2.0.2 — LED module only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub led_color: Option<u32>,
    /// Handbrake "Button" mode (H8, v2.0.2 — handbrake module only):
    /// `Some(true)` = the handbrake input acts as a button,
    /// `Some(false)` = axis; `None` = module default (axis).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handbrake_button_mode: Option<bool>,
    // --- Pedal ramps/curves per-module tuning ---
    /// Pedal press ramp in ms (accelerator / brake / clutch only;
    /// `None` = no per-module ramp — the profile's global
    /// `pedal_press_ms` applies). Defaults: accelerator 100 ms,
    /// brake 0 ms (instant, v2.0.21 F7), clutch 50 ms (see
    /// [`default_press_ms`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub press_ms: Option<f64>,
    /// Pedal release ramp in ms (accelerator / brake / clutch only;
    /// `None` = no per-module ramp — the profile's global
    /// `pedal_release_ms` applies). Defaults: accelerator 50 ms,
    /// brake 50 ms (v2.0.21 F7), clutch 300 ms (see
    /// [`default_release_ms`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_ms: Option<f64>,
    /// Combined pedals (accelerator + brake as one axis, for old
    /// games): a profile-level switch, carried on the accelerator
    /// record only (`None` = not applicable — other modules never
    /// carry it). Default off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub combine_pedals: Option<bool>,
    // --- Handbrake/clutch per-module tuning ---
    /// Latch mode (H1, handbrake module only): press once = the
    /// handbrake stays on, press again = release. `None` = not
    /// applicable — other modules never carry it. Default off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handbrake_latch: Option<bool>,
    /// Auto-release on throttle (H3, handbrake module only): only
    /// with latch on, pressing the throttle past 20% releases the
    /// handbrake (rally launch). `None` = not applicable. Default off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handbrake_auto_release: Option<bool>,
    /// Engagement width / slip zone (C2, clutch module only): the
    /// travel width, as a fraction 0.05..=0.40 (default 0.15), over
    /// which the clutch engages around the bite point instead of
    /// the hard stretch. `None` = not applicable — other modules
    /// never carry it (their bite stays a hard stretch).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bite_width: Option<f64>,
    // --- LED/horn per-module tuning ---
    /// Blink / Chase / Rainbow cycle period in ms (L1, LED module
    /// only), 100–5000 (default 1000): one full pattern cycle per
    /// period on the board. Solid / Off ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub led_period_ms: Option<f64>,
    /// Minimum honk duration in ms (HO1, horn module only),
    /// 100–1000 (default 400): a quick tap holds the horn output
    /// for at least this long, so the game registers it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub horn_min_ms: Option<f64>,
    /// Honk mode (HO2, horn module only): `Some(true)` = Latch
    /// (press once = on, again = off), `Some(false)` = Momentary
    /// (default — the horn sounds only while the button is held).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub horn_latch: Option<bool>,
}

impl TuningSettings {
    /// The neutral default for a module (deadzone 0, linear curve,
    /// full 0–3300 mV span, no smoothing) — the same default
    /// `AppStateCore::get_tuning` documents when nothing is stored.
    /// Nothing here is invented data: it is the documented neutral
    /// tuning, used by the simulated API and IPC fallbacks.
    #[must_use]
    pub fn neutral(module: &str) -> Self {
        Self {
            module: module.to_string(),
            deadzone: 0.0,
            // --- Pedal ramps/curves per-module tuning ---
            // Brake defaults to a progressive 1.3 curve (B3); the
            // other pedals are linear 1.0.
            curve: if module == "brake" { 1.3 } else { 1.0 },
            min_mv: 0.0,
            max_mv: 3300.0,
            smoothing: 0.0,
            invert: None,
            steering_lock_deg: None,
            // --- Steering per-module tuning ---
            steering_balance_pct: None,
            steering_center_trim_pct: None,
            bite_point: None,
            // --- Brake per-module tuning ---
            saturation: None,
            shifter_gate: None,
            // --- Shifter per-module tuning ---
            shift_lockout_ms: None,
            horn_button: None,
            led_brightness: None,
            led_pattern: None,
            led_color: None,
            handbrake_button_mode: None,
            // --- Pedal ramps/curves per-module tuning ---
            press_ms: default_press_ms(module),
            release_ms: default_release_ms(module),
            combine_pedals: if module == "accelerator" || module == "throttle" {
                Some(false)
            } else {
                None
            },
            // --- Handbrake/clutch per-module tuning ---
            // Latch + auto-release live on the handbrake record
            // only (both off); the slip zone on the clutch record
            // only (15%). Other modules never carry them.
            handbrake_latch: if module == "handbrake" {
                Some(false)
            } else {
                None
            },
            handbrake_auto_release: if module == "handbrake" {
                Some(false)
            } else {
                None
            },
            bite_width: if module == "clutch" { Some(0.15) } else { None },
            // --- LED/horn per-module tuning ---
            led_period_ms: None,
            horn_min_ms: None,
            horn_latch: None,
        }
    }
}

/// Validate a [`TuningSettings`] record against the tuning contract.
/// Returns one message per failed rule (empty = valid). The endpoint
/// span guard (≥ 660 mV) is the same rule the frontend's
/// `clampEndpoints` enforces — the backend re-checks it, never trusting
/// the UI alone.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn validate_tuning(tuning: &TuningSettings) -> Vec<String> {
    let mut errors = Vec::new();
    if tuning.module.trim().is_empty() {
        errors.push("tuning module must not be empty".to_string());
    }
    if !(0.0..=30.0).contains(&tuning.deadzone) {
        errors.push(format!(
            "deadzone {} is outside 0..=30 (%)",
            tuning.deadzone
        ));
    }
    if !(0.5..=2.0).contains(&tuning.curve) {
        errors.push(format!("curve {} is outside 0.5..=2.0", tuning.curve));
    }
    if !(0.0..=3300.0).contains(&tuning.min_mv) {
        errors.push(format!("min_mv {} is outside 0..=3300 mV", tuning.min_mv));
    }
    if !(0.0..=3300.0).contains(&tuning.max_mv) {
        errors.push(format!("max_mv {} is outside 0..=3300 mV", tuning.max_mv));
    }
    if tuning.max_mv - tuning.min_mv < 660.0 {
        errors.push(format!(
            "endpoint span {} mV is below the 660 mV minimum",
            tuning.max_mv - tuning.min_mv
        ));
    }
    if !(0.0..=100.0).contains(&tuning.smoothing) {
        errors.push(format!(
            "smoothing {} is outside 0..=100 (%)",
            tuning.smoothing
        ));
    }
    if let Some(lock_deg) = tuning.steering_lock_deg
        && !STEERING_LOCK_STEPS_DEG.contains(&lock_deg)
    {
        errors.push(format!(
            "steering_lock_deg {lock_deg} is not one of {STEERING_LOCK_STEPS_DEG:?}"
        ));
    }
    // --- Steering per-module tuning ---
    if let Some(balance) = tuning.steering_balance_pct
        && !(-20.0..=20.0).contains(&balance)
    {
        errors.push(format!(
            "steering_balance_pct {balance} is outside -20..=20 (%)"
        ));
    }
    if let Some(trim) = tuning.steering_center_trim_pct
        && !(-5.0..=5.0).contains(&trim)
    {
        errors.push(format!(
            "steering_center_trim_pct {trim} is outside -5..=5 (%)"
        ));
    }
    if let Some(bite) = tuning.bite_point
        && !(0.0..=1.0).contains(&bite)
    {
        errors.push(format!("bite_point {bite} is outside 0..=1"));
    }
    // --- Brake per-module tuning ---
    if let Some(saturation) = tuning.saturation
        && !(0.5..=1.0).contains(&saturation)
    {
        errors.push(format!("saturation {saturation} is outside 0.5..=1.0"));
    }
    if let Some(button) = tuning.horn_button
        && !(1..=32).contains(&button)
    {
        errors.push(format!("horn_button {button} is outside 1..=32"));
    }
    // --- Shifter per-module tuning ---
    if let Some(lockout_ms) = tuning.shift_lockout_ms
        && !(0.0..=200.0).contains(&lockout_ms)
    {
        errors.push(format!(
            "shift_lockout_ms {lockout_ms} is outside 0..=200 (ms)"
        ));
    }
    if let Some(brightness) = tuning.led_brightness
        && brightness > 100
    {
        errors.push(format!(
            "led_brightness {brightness} is outside 0..=100 (%)"
        ));
    }
    // --- Pedal ramps/curves per-module tuning ---
    // The engine sanitizes ramps to 0 (off) or 10..=2000 ms; the
    // API contract accepts that whole range and the UI enforces the
    // per-module slider ranges (accel 0..500, brake press 0..300,
    // clutch release 0..1000).
    if let Some(press_ms) = tuning.press_ms
        && !(0.0..=2000.0).contains(&press_ms)
    {
        errors.push(format!("press_ms {press_ms} is outside 0..=2000"));
    }
    if let Some(release_ms) = tuning.release_ms
        && !(0.0..=2000.0).contains(&release_ms)
    {
        errors.push(format!("release_ms {release_ms} is outside 0..=2000"));
    }
    // --- Handbrake/clutch per-module tuning ---
    // (v2.0.19: this check used to live in the deleted
    // `validate_encoder_tuning` helper alongside the encoder/buttons
    // checks — it survives because `bite_width` is a clutch field.)
    if let Some(width) = tuning.bite_width
        && !(0.05..=0.40).contains(&width)
    {
        errors.push(format!("bite_width {width} is outside 0.05..=0.40"));
    }
    validate_led_horn_tuning(tuning, &mut errors);
    errors
}

/// Validate the LED/horn extension fields (L1/HO1): one message
/// per failed rule, appended to `errors`. Split out of
/// [`validate_tuning`] so that function stays under Clippy's
/// 100-line limit.
fn validate_led_horn_tuning(tuning: &TuningSettings, errors: &mut Vec<String>) {
    // --- LED/horn per-module tuning ---
    if let Some(period_ms) = tuning.led_period_ms
        && !(100.0..=5000.0).contains(&period_ms)
    {
        errors.push(format!(
            "led_period_ms {period_ms} is outside 100..=5000 (ms)"
        ));
    }
    if let Some(min_ms) = tuning.horn_min_ms
        && !(100.0..=1000.0).contains(&min_ms)
    {
        errors.push(format!("horn_min_ms {min_ms} is outside 100..=1000 (ms)"));
    }
}

/// Draft status, as the UI prompt (and the `AppApi` surface)
/// consumes it. Plain data: no store or engine type leaks in.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DraftStatus {
    /// Store id of the profile the draft belongs to.
    pub profile_id: String,
    /// Display name of the profile the draft belongs to.
    pub profile_name: String,
    /// True when unsaved tuning edits exist.
    pub dirty: bool,
    /// Module ids with unsaved edits, sorted.
    pub dirty_modules: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tuning(module: &str) -> TuningSettings {
        TuningSettings::neutral(module)
    }

    #[test]
    fn steering_lock_snaps_to_steps_and_maps_to_scale() {
        // Bug 7: lock steps 270/360/540/720/900°; scale = lock/900.
        assert_eq!(snap_steering_lock_deg(900.0), 900.0);
        assert_eq!(snap_steering_lock_deg(500.0), 540.0);
        assert_eq!(snap_steering_lock_deg(200.0), 270.0);
        assert_eq!(snap_steering_lock_deg(1000.0), 900.0);
        assert_eq!(snap_steering_lock_deg(f64::NAN), 900.0);
        assert_eq!(steering_lock_deg_to_scale(900.0), 1.0);
        assert_eq!(steering_lock_deg_to_scale(360.0), 0.4);
        assert!((steering_lock_deg_to_scale(540.0) - 0.6).abs() < f64::EPSILON);
        assert!((steering_lock_deg_to_scale(270.0) - 0.3).abs() < f64::EPSILON);
    }

    #[test]
    fn steering_invert_and_lock_round_trip_and_validate() {
        let mut settings = tuning("steering");
        settings.invert = Some(true);
        settings.steering_lock_deg = Some(540.0);
        assert_eq!(validate_tuning(&settings), Vec::<String>::new());
        let json = serde_json::to_string(&settings).expect("serialize");
        assert!(json.contains("\"invert\":true"), "{json}");
        assert!(json.contains("\"steeringLockDeg\":540.0"), "{json}");
        let back: TuningSettings = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, settings);
        // Other modules never carry them: omitted from the JSON.
        let json = serde_json::to_string(&tuning("brake")).expect("serialize");
        assert!(!json.contains("invert"), "{json}");
        assert!(!json.contains("steeringLockDeg"), "{json}");
        // An off-step lock is rejected, never silently snapped here.
        let mut bad = tuning("steering");
        bad.steering_lock_deg = Some(450.0);
        let errors = validate_tuning(&bad);
        assert_eq!(errors.len(), 1, "errors: {errors:?}");
        assert!(errors[0].contains("steering_lock_deg"), "{}", errors[0]);
    }

    /// Steering per-module tuning (S2+S3): balance and centre trim
    /// round-trip through JSON, validate in range, and are absent
    /// for other modules (old profiles load them as `None`).
    #[test]
    fn steering_balance_and_trim_round_trip_and_validate() {
        let mut settings = tuning("steering");
        settings.steering_balance_pct = Some(-20.0);
        settings.steering_center_trim_pct = Some(5.0);
        assert_eq!(validate_tuning(&settings), Vec::<String>::new());
        let json = serde_json::to_string(&settings).expect("serialize");
        assert!(json.contains("\"steeringBalancePct\":-20.0"), "{json}");
        assert!(json.contains("\"steeringCenterTrimPct\":5.0"), "{json}");
        let back: TuningSettings = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, settings);
        // Other modules never carry them: omitted from the JSON.
        let json = serde_json::to_string(&tuning("brake")).expect("serialize");
        assert!(!json.contains("steeringBalancePct"), "{json}");
        assert!(!json.contains("steeringCenterTrimPct"), "{json}");
        // Out-of-range values are rejected, never silently clamped.
        let mut bad = tuning("steering");
        bad.steering_balance_pct = Some(21.0);
        bad.steering_center_trim_pct = Some(-6.0);
        let errors = validate_tuning(&bad);
        assert_eq!(errors.len(), 2, "errors: {errors:?}");
        assert!(errors.iter().any(|e| e.contains("steering_balance_pct")));
        assert!(
            errors
                .iter()
                .any(|e| e.contains("steering_center_trim_pct"))
        );
    }

    #[test]
    fn tuning_valid_and_extension_fields_round_trip() {
        let mut settings = tuning("brake");
        settings.bite_point = Some(0.35);
        assert_eq!(validate_tuning(&settings), Vec::<String>::new());
        let mut led = tuning("led");
        led.led_brightness = Some(80);
        led.led_pattern = Some(LedPattern::Rainbow);
        assert_eq!(validate_tuning(&led), Vec::<String>::new());
        let json = serde_json::to_string(&led).expect("serialize");
        assert!(json.contains("\"ledBrightness\":80"), "{json}");
        assert!(json.contains("\"ledPattern\":\"rainbow\""), "{json}");
        assert!(!json.contains("\"shifterGate\""), "{json}");
        let back: TuningSettings = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, led);
        let mut shifter = tuning("shifter");
        shifter.shifter_gate = Some(ShifterGate::Sequential);
        shifter.horn_button = None;
        assert_eq!(validate_tuning(&shifter), Vec::<String>::new());
        let json = serde_json::to_string(&shifter).expect("serialize");
        assert!(json.contains("\"shifterGate\":\"sequential\""), "{json}");
    }

    #[test]
    fn brake_saturation_round_trips_and_validates() {
        // B1: brake force 50–100% (0.5..=1.0 fraction); defaults to
        // None (= 100%) so old profiles load without the field.
        let mut settings = tuning("brake");
        assert_eq!(settings.saturation, None);
        assert_eq!(validate_tuning(&settings), Vec::<String>::new());
        settings.saturation = Some(0.75);
        assert_eq!(validate_tuning(&settings), Vec::<String>::new());
        let json = serde_json::to_string(&settings).expect("serialize");
        assert!(json.contains("\"saturation\":0.75"), "{json}");
        let back: TuningSettings = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, settings);
        // Other modules never carry it: omitted from the JSON.
        let json = serde_json::to_string(&tuning("clutch")).expect("serialize");
        assert!(!json.contains("saturation"), "{json}");
        // Old JSON without the field loads as None (100%).
        let old = r#"{"module":"brake","deadzone":0.0,"curve":1.0,"minMv":0.0,"maxMv":3300.0,"smoothing":0.0,"bitePoint":0.2}"#;
        let back: TuningSettings = serde_json::from_str(old).expect("deserialize old");
        assert_eq!(back.saturation, None);
        assert_eq!(back.bite_point, Some(0.2));
        // Out-of-range saturation is rejected, never silently clamped.
        let mut bad = tuning("brake");
        bad.saturation = Some(0.4);
        let errors = validate_tuning(&bad);
        assert_eq!(errors.len(), 1, "errors: {errors:?}");
        assert!(errors[0].contains("saturation"), "{}", errors[0]);
    }

    #[test]
    fn tuning_rejects_bad_span_bite_point_and_horn_button() {
        let mut settings = tuning("clutch");
        settings.min_mv = 3000.0;
        settings.max_mv = 3300.0; // span 300 < 660
        settings.bite_point = Some(1.5);
        settings.horn_button = Some(40);
        let errors = validate_tuning(&settings);
        assert_eq!(errors.len(), 3, "errors: {errors:?}");
        assert!(errors.iter().any(|e| e.contains("span")));
        assert!(errors.iter().any(|e| e.contains("bite_point")));
        assert!(errors.iter().any(|e| e.contains("horn_button")));
    }

    /// LED/horn per-module tuning (L1/HO1/HO2): period, minimum
    /// honk duration and latch mode round-trip through JSON,
    /// validate in range, and are absent for other modules (old
    /// profiles load them as `None`).
    #[test]
    fn led_horn_fields_round_trip_and_validate() {
        let mut led = tuning("led");
        led.led_period_ms = Some(2500.0);
        assert_eq!(validate_tuning(&led), Vec::<String>::new());
        let json = serde_json::to_string(&led).expect("serialize");
        assert!(json.contains("\"ledPeriodMs\":2500.0"), "{json}");
        let back: TuningSettings = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, led);
        let mut horn = tuning("horn");
        horn.horn_min_ms = Some(100.0);
        horn.horn_latch = Some(true);
        assert_eq!(validate_tuning(&horn), Vec::<String>::new());
        let json = serde_json::to_string(&horn).expect("serialize");
        assert!(json.contains("\"hornMinMs\":100.0"), "{json}");
        assert!(json.contains("\"hornLatch\":true"), "{json}");
        let back: TuningSettings = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, horn);
        // Other modules never carry them: omitted from the JSON.
        let json = serde_json::to_string(&tuning("brake")).expect("serialize");
        assert!(!json.contains("ledPeriodMs"), "{json}");
        assert!(!json.contains("hornMinMs"), "{json}");
        assert!(!json.contains("hornLatch"), "{json}");
        // Old JSON without the fields loads as None (1000 ms period,
        // 400 ms minimum honk, Momentary mode — the defaults apply
        // in `get_tuning`, never a silent 0).
        let old: TuningSettings =
            serde_json::from_str(r#"{"module":"led","deadzone":0.0,"curve":1.0,"minMv":0.0,"maxMv":3300.0,"smoothing":0.0,"ledPattern":"blink"}"#)
                .expect("deserialize old");
        assert_eq!(old.led_period_ms, None);
        assert_eq!(old.horn_min_ms, None);
        assert_eq!(old.horn_latch, None);
        // Out-of-range values are rejected, never silently clamped.
        let mut bad = tuning("led");
        bad.led_period_ms = Some(50.0);
        let mut bad_horn = tuning("horn");
        bad_horn.horn_min_ms = Some(1500.0);
        let errors = validate_tuning(&bad);
        assert_eq!(errors.len(), 1, "errors: {errors:?}");
        assert!(errors[0].contains("led_period_ms"), "{}", errors[0]);
        let errors = validate_tuning(&bad_horn);
        assert_eq!(errors.len(), 1, "errors: {errors:?}");
        assert!(errors[0].contains("horn_min_ms"), "{}", errors[0]);
    }

    #[test]
    fn shifter_lockout_round_trips_and_validates() {
        let mut settings = tuning("shifter");
        settings.shift_lockout_ms = Some(120.0);
        assert_eq!(validate_tuning(&settings), Vec::<String>::new());
        let json = serde_json::to_string(&settings).expect("serialize");
        assert!(json.contains("\"shiftLockoutMs\":120.0"), "{json}");
        let back: TuningSettings = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, settings);
        // Other modules never carry it: omitted from the JSON.
        let json = serde_json::to_string(&tuning("brake")).expect("serialize");
        assert!(!json.contains("shiftLockoutMs"), "{json}");
        // Out-of-range values are rejected, never silently clamped
        // here; 0 (off) and 200 (max) are valid.
        let mut bad = tuning("shifter");
        bad.shift_lockout_ms = Some(201.0);
        let errors = validate_tuning(&bad);
        assert_eq!(errors.len(), 1, "errors: {errors:?}");
        assert!(errors[0].contains("shift_lockout_ms"), "{}", errors[0]);
        let mut off = tuning("shifter");
        off.shift_lockout_ms = Some(0.0);
        assert_eq!(validate_tuning(&off), Vec::<String>::new());
        let mut max = tuning("shifter");
        max.shift_lockout_ms = Some(200.0);
        assert_eq!(validate_tuning(&max), Vec::<String>::new());
    }

    #[test]
    fn handbrake_clutch_fields_round_trip_and_validate() {
        // H1/H3 + C2: latch off, auto-release off, grab 0 ms
        // (press_ms), slip zone 15% — old profiles load with
        // exactly these defaults (None in the stored JSON).
        assert_eq!(default_press_ms("handbrake"), Some(0.0));
        let mut hb = tuning("handbrake");
        assert_eq!(hb.handbrake_latch, Some(false));
        assert_eq!(hb.handbrake_auto_release, Some(false));
        assert_eq!(hb.press_ms, Some(0.0));
        assert_eq!(hb.bite_width, None);
        assert_eq!(validate_tuning(&hb), Vec::<String>::new());
        hb.handbrake_latch = Some(true);
        hb.handbrake_auto_release = Some(true);
        hb.press_ms = Some(120.0);
        assert_eq!(validate_tuning(&hb), Vec::<String>::new());
        let json = serde_json::to_string(&hb).expect("serialize");
        assert!(json.contains("\"handbrakeLatch\":true"), "{json}");
        assert!(json.contains("\"handbrakeAutoRelease\":true"), "{json}");
        assert!(json.contains("\"pressMs\":120.0"), "{json}");
        let back: TuningSettings = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, hb);
        // Other modules never carry them: omitted from the JSON.
        let json = serde_json::to_string(&tuning("brake")).expect("serialize");
        assert!(!json.contains("handbrakeLatch"), "{json}");
        assert!(!json.contains("handbrakeAutoRelease"), "{json}");
        assert!(!json.contains("biteWidth"), "{json}");

        let mut clutch = tuning("clutch");
        assert_eq!(clutch.bite_width, Some(0.15));
        assert_eq!(validate_tuning(&clutch), Vec::<String>::new());
        clutch.bite_width = Some(0.4);
        assert_eq!(validate_tuning(&clutch), Vec::<String>::new());
        let json = serde_json::to_string(&clutch).expect("serialize");
        assert!(json.contains("\"biteWidth\":0.4"), "{json}");
        let back: TuningSettings = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, clutch);
        // Old JSON without the fields loads with the defaults
        // (None here — `get_tuning` fills them in; see state.rs).
        let old = r#"{"module":"clutch","deadzone":0.0,"curve":1.0,"minMv":0.0,"maxMv":3300.0,"smoothing":0.0,"bitePoint":0.35}"#;
        let back: TuningSettings = serde_json::from_str(old).expect("deserialize old");
        assert_eq!(back.bite_width, None);
        assert_eq!(back.bite_point, Some(0.35));
        // Out-of-range slip width is rejected, never silently clamped.
        let mut bad = tuning("clutch");
        bad.bite_width = Some(0.04);
        let errors = validate_tuning(&bad);
        assert_eq!(errors.len(), 1, "errors: {errors:?}");
        assert!(errors[0].contains("bite_width"), "{}", errors[0]);
    }

    #[test]
    fn neutral_default_is_valid_and_round_trips() {
        let settings = TuningSettings::neutral("steering");
        assert_eq!(settings.deadzone, 0.0);
        assert_eq!(settings.curve, 1.0);
        assert_eq!(settings.max_mv - settings.min_mv, 3300.0);
        assert_eq!(validate_tuning(&settings), Vec::<String>::new());
        let json = serde_json::to_string(&settings).expect("serialize");
        assert!(json.contains("\"module\":\"steering\""), "{json}");
        let back: TuningSettings = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, settings);
    }

    #[test]
    fn pedal_ramp_defaults_and_round_trip() {
        // --- Pedal ramps/curves per-module tuning ---
        // Task defaults (v2.0.21 F7/P2): accel press 100 /
        // release 50, brake press 0 (instant) / release 50,
        // clutch press 50 / release 300, combined pedals off.
        assert_eq!(default_press_ms("accelerator"), Some(100.0));
        assert_eq!(default_release_ms("accelerator"), Some(50.0));
        assert_eq!(default_press_ms("throttle"), Some(100.0));
        assert_eq!(default_press_ms("brake"), Some(0.0));
        assert_eq!(default_release_ms("brake"), Some(50.0));
        assert_eq!(default_press_ms("clutch"), Some(50.0));
        assert_eq!(default_release_ms("clutch"), Some(300.0));
        assert_eq!(default_press_ms("steering"), None);
        assert_eq!(default_release_ms("steering"), None);
        // B3: the brake curve defaults to progressive 1.3, the
        // other pedals to linear 1.0.
        assert_eq!(TuningSettings::neutral("brake").curve, 1.3);
        assert_eq!(TuningSettings::neutral("clutch").curve, 1.0);
        let accel = TuningSettings::neutral("accelerator");
        assert_eq!(accel.press_ms, Some(100.0));
        assert_eq!(accel.release_ms, Some(50.0));
        assert_eq!(accel.combine_pedals, Some(false));
        assert_eq!(validate_tuning(&accel), Vec::<String>::new());
        let json = serde_json::to_string(&accel).expect("serialize");
        assert!(json.contains("\"pressMs\":100.0"), "{json}");
        assert!(json.contains("\"releaseMs\":50.0"), "{json}");
        assert!(json.contains("\"combinePedals\":false"), "{json}");
        let back: TuningSettings = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, accel);
        // Old JSON without the fields loads with None (old profiles
        // keep the global ramps until edited).
        let old: TuningSettings =
            serde_json::from_str(r#"{"module":"brake","deadzone":0.0,"curve":1.3,"minMv":0.0,"maxMv":3300.0,"smoothing":0.0}"#)
                .expect("deserialize old");
        assert_eq!(old.press_ms, None);
        assert_eq!(old.release_ms, None);
        assert_eq!(old.combine_pedals, None);
        // Out-of-range ramps are rejected, never silently clamped.
        let mut bad = TuningSettings::neutral("brake");
        bad.press_ms = Some(2500.0);
        bad.release_ms = Some(-5.0);
        let errors = validate_tuning(&bad);
        assert_eq!(errors.len(), 2, "errors: {errors:?}");
        assert!(errors.iter().any(|e| e.contains("press_ms")));
        assert!(errors.iter().any(|e| e.contains("release_ms")));
    }

    #[test]
    fn draft_status_serializes_camel_case() {
        let status = DraftStatus {
            profile_id: "p1".to_string(),
            profile_name: "GT3 Sprint".to_string(),
            dirty: true,
            dirty_modules: vec!["clutch".to_string(), "steering".to_string()],
        };
        let json = serde_json::to_string(&status).expect("serialize");
        assert!(json.contains("\"profileName\":\"GT3 Sprint\""), "{json}");
        assert!(
            json.contains("\"dirtyModules\":[\"clutch\",\"steering\"]"),
            "{json}"
        );
        let back: DraftStatus = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, status);
    }
}
