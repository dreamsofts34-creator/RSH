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
//! flips the steering direction, and `steering_lock_deg` is the
//! steering lock in degrees (one of [`STEERING_LOCK_STEPS_DEG`]).

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
/// Workstream C): the accelerator opens over 100 ms, the brake bites
/// over 50 ms; other modules have no press ramp of their own and
/// fall back to the profile's global `pedal_press_ms` (150 ms).
/// Both spellings of the throttle id are accepted — the UI writes
/// `"throttle"`, the pipeline reads `"accelerator"`.
#[must_use]
pub fn default_press_ms(module: &str) -> Option<f64> {
    match module {
        "accelerator" | "throttle" => Some(100.0),
        "brake" => Some(50.0),
        _ => None,
    }
}

/// Default pedal release-ramp ms for a module (per-module tuning,
/// Workstream C): the accelerator closes over 50 ms, the clutch
/// releases over 300 ms; other modules fall back to the profile's
/// global `pedal_release_ms` (100 ms).
#[must_use]
pub fn default_release_ms(module: &str) -> Option<f64> {
    match module {
        "accelerator" | "throttle" => Some(50.0),
        "clutch" => Some(300.0),
        _ => None,
    }
}

/// Shifter gate type (tuning extension; the C# `ShifterGateMode`
/// string kept as a typed enum here).
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub enum ShifterGate {
    /// Sequential up/down (buttons).
    #[serde(rename = "sequential")]
    Sequential,
    /// H-pattern (6 gear pins).
    #[serde(rename = "h-pattern")]
    HPattern,
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
    /// Pedal bite point, 0..1 (clutch / brake only; `None` elsewhere).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bite_point: Option<f64>,
    /// Shifter gate type (shifter module only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shifter_gate: Option<ShifterGate>,
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
    /// brake 50 ms (see [`default_press_ms`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub press_ms: Option<f64>,
    /// Pedal release ramp in ms (accelerator / brake / clutch only;
    /// `None` = no per-module ramp — the profile's global
    /// `pedal_release_ms` applies). Defaults: accelerator 50 ms,
    /// clutch 300 ms (see [`default_release_ms`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_ms: Option<f64>,
    /// Combined pedals (accelerator + brake as one axis, for old
    /// games): a profile-level switch, carried on the accelerator
    /// record only (`None` = not applicable — other modules never
    /// carry it). Default off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub combine_pedals: Option<bool>,
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
            bite_point: None,
            shifter_gate: None,
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
        }
    }
}

/// Validate a [`TuningSettings`] record against the tuning contract.
/// Returns one message per failed rule (empty = valid). The endpoint
/// span guard (≥ 660 mV) is the same rule the frontend's
/// `clampEndpoints` enforces — the backend re-checks it, never trusting
/// the UI alone.
#[must_use]
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
    if let Some(bite) = tuning.bite_point
        && !(0.0..=1.0).contains(&bite)
    {
        errors.push(format!("bite_point {bite} is outside 0..=1"));
    }
    if let Some(button) = tuning.horn_button
        && !(1..=32).contains(&button)
    {
        errors.push(format!("horn_button {button} is outside 1..=32"));
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
    errors
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
        shifter.shifter_gate = Some(ShifterGate::HPattern);
        shifter.horn_button = None;
        assert_eq!(validate_tuning(&shifter), Vec::<String>::new());
        let json = serde_json::to_string(&shifter).expect("serialize");
        assert!(json.contains("\"shifterGate\":\"h-pattern\""), "{json}");
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
        // Task defaults: accel press 100 / release 50, brake press
        // 50, clutch release 300, combined pedals off.
        assert_eq!(default_press_ms("accelerator"), Some(100.0));
        assert_eq!(default_release_ms("accelerator"), Some(50.0));
        assert_eq!(default_press_ms("throttle"), Some(100.0));
        assert_eq!(default_press_ms("brake"), Some(50.0));
        assert_eq!(default_release_ms("brake"), None);
        assert_eq!(default_press_ms("clutch"), None);
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
