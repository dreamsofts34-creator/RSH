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
//! must stay ≥ 660 mV), `smoothing` is percent (0–100).

use serde::{Deserialize, Serialize};

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
            curve: 1.0,
            min_mv: 0.0,
            max_mv: 3300.0,
            smoothing: 0.0,
            bite_point: None,
            shifter_gate: None,
            horn_button: None,
            led_brightness: None,
            led_pattern: None,
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
