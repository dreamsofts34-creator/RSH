//! Read-only importer for C# RSH v2 data files (`profiles.json`,
//! `settings.json`).
//!
//! * Reads only — never writes, deletes, or modifies the C# files. The file
//!   readers below use `fs::read_to_string` and nothing else.
//! * Parses PascalCase JSON case-insensitively (`DATA-INPUT.md` §1.2):
//!   object keys are converted to `snake_case` before deserializing, so
//!   `SchemaVersion`, `schemaVersion`, and `SCHEMAVERSION` all match.
//! * Enforces the C# size caps (profiles at most 1 MiB, settings at most
//!   64 KiB) before parsing, so an oversized file is rejected without being
//!   fully loaded. The cap is checked twice — once on the file metadata and
//!   once on the text — so a file that grows between the two checks is still
//!   rejected.
//! * Calibration values are kept verbatim. A calibration whose
//!   `is_millivolts` flag is false is unit-less and is reported by
//!   [`WheelProfile::computed_needs_recalibration`] — it is never silently
//!   treated as millivolts (the P9 lesson, `DATA-INPUT.md` §1.3).
//! * One bad profile entry does not kill the whole file: it is skipped and
//!   counted in [`ProfileFile::skipped_profiles`] (F3 tolerance,
//!   `DATA-INPUT.md` §1.4).
//! * The app itself starts fresh (D-R8): nothing in this module is called
//!   automatically at startup. It exists for a manual/future import step.
//!
//! Deliberately NOT in this module (later phases own them): value
//! sanitizing/clamping beyond the settings allowlists, 3-generation backup,
//! atomic save, the `ProfileStore` load chain, and any key/password
//! migration. A plaintext Wi-Fi password found in a C# `settings.json` is
//! ignored here and never carried into Rust storage.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io;
use std::net::Ipv4Addr;
use std::path::Path;

use serde::de::Deserializer;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Maximum size of a C# `profiles.json` file: 1 MiB (`DATA-INPUT.md` §1.4).
pub const MAX_PROFILES_FILE_BYTES: u64 = 1_048_576;
/// Maximum size of a C# `settings.json` file: 64 KiB (`DATA-INPUT.md` §2.1).
pub const MAX_SETTINGS_FILE_BYTES: u64 = 65_536;
/// Minimum sane calibration span: 20% of the 0–3300 mV full scale (D10).
pub const MIN_CALIBRATION_SPAN_MV: i32 = 660;

/// Errors from the read-only C# importer.
#[derive(Debug)]
pub enum ImportError {
    /// The file could not be read.
    Io(io::Error),
    /// The JSON could not be parsed.
    Json(serde_json::Error),
    /// The file exceeds the size cap.
    TooLarge {
        /// The cap that was exceeded, in bytes.
        limit: u64,
    },
    /// The file parsed but failed schema validation.
    Schema(String),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "could not read C# data file: {error}"),
            Self::Json(error) => write!(f, "could not parse C# data file: {error}"),
            Self::TooLarge { limit } => {
                write!(f, "C# data file exceeds the {limit} byte cap")
            }
            Self::Schema(message) => {
                write!(f, "C# data file failed schema validation: {message}")
            }
        }
    }
}

impl std::error::Error for ImportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::TooLarge { .. } | Self::Schema(_) => None,
        }
    }
}

impl From<io::Error> for ImportError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for ImportError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

/// The game a profile belongs to (`DATA-INPUT.md` §1.2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GameId {
    /// Unknown game (0). Unknown numeric values also map here.
    #[default]
    Unknown = 0,
    /// Assetto Corsa (1).
    AssettoCorsa = 1,
    /// Assetto Corsa Competizione (2).
    Acc = 2,
    /// iRacing (3).
    IRacing = 3,
    /// Dirt Rally 2.0 (4, v2.0.2 — M1 link target; old files never
    /// contain 4, so this is backward compatible).
    DirtRally2 = 4,
}

impl GameId {
    /// Maps a raw numeric game id, defaulting to [`GameId::Unknown`].
    const fn from_u32(value: u32) -> Self {
        match value {
            1 => Self::AssettoCorsa,
            2 => Self::Acc,
            3 => Self::IRacing,
            4 => Self::DirtRally2,
            _ => Self::Unknown,
        }
    }
}

impl<'de> Deserialize<'de> for GameId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = u32::deserialize(deserializer)?;
        Ok(Self::from_u32(value))
    }
}

impl Serialize for GameId {
    /// Serializes as the C# numeric game id (0 Unknown, 1 AC, 2 ACC, 3 iRacing).
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(*self as u32)
    }
}

/// Steering calibration: three points, millivolt domain when `is_millivolts`.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", default)]
pub struct SteeringCalibration {
    /// Minimum raw value (millivolts when `is_millivolts` is true).
    pub raw_min: i32,
    /// Centre raw value.
    pub raw_center: i32,
    /// Maximum raw value.
    pub raw_max: i32,
    /// Whether calibration has been captured at all.
    pub is_set: bool,
    /// Whether the values are millivolts (`false` = unit-less legacy values).
    pub is_millivolts: bool,
}

/// Pedal calibration: two points, millivolt domain when `is_millivolts`.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", default)]
pub struct PedalCalibration {
    /// Minimum raw value (millivolts when `is_millivolts` is true).
    pub raw_min: i32,
    /// Maximum raw value.
    pub raw_max: i32,
    /// Whether calibration has been captured at all.
    pub is_set: bool,
    /// Whether the values are millivolts (`false` = unit-less legacy values).
    pub is_millivolts: bool,
}

/// Per-module tuning, as stored in a C# profile's `ModuleTunings` map.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case", default)]
pub struct PerModuleTuning {
    /// Deadzone, 0..0.5.
    pub deadzone: f64,
    /// Curve strength, -1..1 (0 = linear).
    pub curve_strength: f64,
    /// Smoothing, 0..1 (0 = off).
    pub smoothing: f64,
    /// Whether the axis is inverted.
    pub invert: bool,
    /// Scale, 0.2..1.0.
    #[serde(default = "default_scale")]
    pub scale: f64,
    /// Whether the module is enabled (a disabled module never reaches the pipeline).
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Shifter gate mode as written in the C# file.
    ///
    /// Kept as a string on purpose: the exact C# `ShifterGateMode` variants
    /// were not available to this phase (`DATA-INPUT.md` §1.2), so no variants
    /// are invented here. A typed enum is a later-phase (SW-R4) item.
    #[serde(default = "default_gate_mode")]
    pub gate_mode: String,
    /// Raw minimum in mV for this module's axis, when the tuning screen
    /// captured one (`None` = never captured, never a fake 0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_mv: Option<f64>,
    /// Raw maximum in mV for this module's axis, when captured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_mv: Option<f64>,
    /// Pedal bite point, 0..1 (clutch / brake only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bite_point: Option<f64>,
    // --- Brake per-module tuning ---
    /// Brake force (BRF), 0.5..=1.0 as a fraction: the travel that
    /// gives 100% brake (brake module only; `None` = 100% — old
    /// profiles load as `None`, i.e. full force, calibration intact).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saturation: Option<f64>,
    /// Button number (1–32) the horn is bound to (horn module only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub horn_button: Option<u8>,
    /// LED strip brightness in percent, 0–100 (LED module only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub led_brightness: Option<u8>,
    /// LED strip pattern name (LED module only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub led_pattern: Option<String>,
    /// LED strip colour as `"#RRGGBB"` (M2, v2.0.2 — LED module only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub led_color: Option<String>,
    /// Handbrake "Button" mode (H8, v2.0.2 — handbrake module only):
    /// `Some(true)` = the handbrake input acts as a button,
    /// `Some(false)` = axis; `None` = module default (axis).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handbrake_button_mode: Option<bool>,
    // --- Pedal ramps/curves per-module tuning ---
    /// Pedal press ramp in ms (accelerator / brake / clutch only).
    /// `None` = never set — the profile's global `pedal_press_ms`
    /// applies (old profiles load with `None`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub press_ms: Option<f64>,
    /// Pedal release ramp in ms (accelerator / brake / clutch only).
    /// `None` = never set — the profile's global `pedal_release_ms`
    /// applies (old profiles load with `None`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_ms: Option<f64>,
    // --- Shifter per-module tuning ---
    /// Shift lockout in ms (SH2, shifter module only), 0–200.
    /// `None` = never set (old profile) — the 50 ms default applies.
    /// `Some(0.0)` = off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shift_lockout_ms: Option<f64>,
    // --- Handbrake/clutch per-module tuning ---
    /// Latch mode (H1, handbrake module only). `None` = never set
    /// (old profile) — off applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handbrake_latch: Option<bool>,
    /// Auto-release on throttle (H3, handbrake module only).
    /// `None` = never set (old profile) — off applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handbrake_auto_release: Option<bool>,
    /// Engagement width / slip zone (C2, clutch module only),
    /// 0.05..=0.40. `None` = never set (old profile) — 0.15 applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bite_width: Option<f64>,
    // --- LED/horn per-module tuning ---
    /// Blink / Chase / Rainbow cycle period in ms (L1, LED module
    /// only), 100–5000. `None` = never set (old profile) — the
    /// 1000 ms default applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub led_period_ms: Option<f64>,
    /// Minimum honk duration in ms (HO1, horn module only),
    /// 100–1000. `None` = never set — the 400 ms default applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub horn_min_ms: Option<f64>,
    /// Honk latch mode (HO2, horn module only): `Some(true)` =
    /// Latch, `Some(false)` / `None` = Momentary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub horn_latch: Option<bool>,
}

/// One wheel profile from a C# `profiles.json` (Profile v2).
//
// The bool count is dictated by the external C# JSON schema (one flag per
// invert/combine/recalibration field); grouping them would break the import
// mapping, so the pedantic bool-count lint is allowed here.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case", default)]
pub struct WheelProfile {
    /// Profile name.
    #[serde(default = "default_profile_name")]
    pub name: String,
    /// Per-profile `schemaVersion`; absent means v1 (0).
    pub schema_version: u32,
    /// Steering calibration (three points).
    pub steering_calibration: Option<SteeringCalibration>,
    /// Accelerator calibration (two points).
    pub accel_calibration: Option<PedalCalibration>,
    /// Brake calibration (two points).
    pub brake_calibration: Option<PedalCalibration>,
    /// Clutch calibration (two points).
    pub clutch_calibration: Option<PedalCalibration>,
    /// Deadzone, 0..0.5.
    #[serde(default = "default_deadzone")]
    pub deadzone: f64,
    /// Curve strength, -1..1 (0 = linear).
    pub curve_strength: f64,
    /// Smoothing, 0..1 (0 = off).
    #[serde(default = "default_smoothing")]
    pub smoothing: f64,
    /// Whether steering is inverted.
    pub invert_steering: bool,
    /// Whether the accelerator is inverted.
    pub invert_accel: bool,
    /// Whether the brake is inverted.
    pub invert_brake: bool,
    /// Whether both pedals share one axis (older games).
    pub combine_pedals: bool,
    /// Gear-up output button, 1..32 (D9: above 32 is rejected, never clamped).
    #[serde(default = "default_gear_up_button")]
    pub gear_up_vjoy_button: u32,
    /// Gear-down output button, 1..32.
    #[serde(default = "default_gear_down_button")]
    pub gear_down_vjoy_button: u32,
    /// Pedal press time in milliseconds, 10..2000.
    #[serde(default = "default_pedal_press_ms")]
    pub pedal_press_ms: f64,
    /// Pedal release time in milliseconds, 10..2000.
    #[serde(default = "default_pedal_release_ms")]
    pub pedal_release_ms: f64,
    /// Steering scale, 0.2..1.0 (1.0 = 900°, 0.6 = 540°, 0.4 = 360°).
    #[serde(default = "default_scale")]
    pub steering_scale: f64,
    // --- Steering per-module tuning ---
    /// Steering left/right lock balance in percent, −20..=+20
    /// (0 = even; positive favours the left side). Absent in old
    /// profiles → 0 (even), never a guessed value.
    #[serde(default)]
    pub steering_balance_pct: f64,
    /// Steering centre trim in percent, −5..=+5 (0 = no trim).
    /// Absent in old profiles → 0.
    #[serde(default)]
    pub steering_center_trim_pct: f64,
    /// Game process name for auto-switch (`.exe` suffix stripped by the store).
    pub game_process_name: String,
    /// Car model for auto-switch (normalized lowercase by the store).
    pub car_model: String,
    /// Game this profile belongs to.
    pub game: GameId,
    /// Per-module tunings, keyed by module name (normalized to `snake_case`
    /// by the importer, e.g. `steering`); unknown keys are dropped by the store.
    pub module_tunings: BTreeMap<String, PerModuleTuning>,
    /// Board MAC this profile was made for, canonical `AA:BB:CC:DD:EE:FF` (D21).
    pub board_mac: Option<String>,
    /// Persisted recalibration flag. Recomputed on load — see
    /// [`WheelProfile::computed_needs_recalibration`].
    pub needs_recalibration: bool,
    /// Pin-map fingerprint the calibration was captured against.
    pub calibration_pin_map_fingerprint: Option<String>,
    /// Firmware version the calibration was captured against.
    pub calibration_firmware_version: Option<String>,
    /// When the calibration was captured (RFC 3339). Kept as a string: the
    /// date/time crate choice is still unverified (`DATA-INPUT.md` §0).
    pub calibrated_at_utc: Option<String>,
}

impl WheelProfile {
    /// Recomputes whether this profile needs recalibration, from the live
    /// rules (`DATA-INPUT.md` §1.3): a set calibration is insane when it is
    /// unit-less (`is_millivolts` false), when the span is below 660 mV, or —
    /// for steering — when the three points are not strictly ordered.
    /// A calibration that was never set is normal first-run flow, not a trigger.
    #[must_use]
    pub fn computed_needs_recalibration(&self) -> bool {
        if let Some(calibration) = &self.steering_calibration
            && !steering_calibration_sane(calibration)
        {
            return true;
        }
        for calibration in [
            &self.accel_calibration,
            &self.brake_calibration,
            &self.clutch_calibration,
        ] {
            if let Some(calibration) = calibration
                && !pedal_calibration_sane(calibration)
            {
                return true;
            }
        }
        false
    }
}

/// A parsed C# `profiles.json` file (Profile v2).
#[derive(Clone, Debug, Default)]
pub struct ProfileFile {
    /// File-level `schemaVersion` (2 for C# v2 files; 0/1 means a v1 file).
    pub schema_version: u32,
    /// Profiles that parsed successfully.
    pub profiles: Vec<WheelProfile>,
    /// The active profile name from the file (it may name a missing profile).
    pub active_profile_name: String,
    /// Entries in the file that failed to parse and were skipped (F3 tolerance).
    pub skipped_profiles: usize,
}

/// App settings from a C# `settings.json`.
//
// The bool count is dictated by the external C# JSON schema; grouping them
// would break the import mapping, so the pedantic bool-count lint is allowed
// here.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case", default)]
pub struct AppSettings {
    /// Theme preference: `System`, `Dark`, `Light`, or `HighContrast`.
    pub theme: String,
    /// Language preference: `System`, `en`, `ur`, or `hi`.
    pub language: String,
    /// Whether the app starts with Windows.
    pub autostart: bool,
    /// Whether the app starts minimized.
    pub start_minimized: bool,
    /// Whether closing the window hides to the tray instead of exiting.
    #[serde(default = "default_true")]
    pub close_to_tray: bool,
    /// Whether the output backend is enabled.
    #[serde(default = "default_true")]
    pub output_enabled: bool,
    /// Wheel address: a private IPv4 literal only (anything else → default).
    #[serde(default = "default_esp_ip")]
    pub esp_ip: String,
    /// vJoy device id, clamped to 1..=16 on parse.
    #[serde(default = "default_vjoy_device_id")]
    pub vjoy_device_id: u32,
    /// Output backend: `VJoy` (default), `Xbox` (ViGEmBus
    /// fallback), or `None` (no output device). A persisted
    /// `"HIDMaestro"` value sanitizes to the vJoy default (see
    /// [`sanitize_settings`]) — the HIDMaestro backend is gone.
    #[serde(default = "default_output_backend")]
    pub output_backend: String,
    /// Log level: `Debug`, `Information`, `Warning`, or `Error`.
    #[serde(default = "default_log_level")]
    pub log_level: String,
    /// Whether the setup wizard has been completed.
    pub wizard_completed: bool,
    /// Whether the first run has finished.
    pub first_run_done: bool,
    /// Whether profiles auto-switch per game/car.
    #[serde(default = "default_true")]
    pub auto_switch_profiles: bool,
    /// Whether auto-switching is paused (M6, v2.0.2): paused means
    /// the watcher does not switch, even when `auto_switch_profiles`
    /// is on. Default `false` (not paused).
    #[serde(default)]
    pub auto_switch_paused: bool,
    /// Per-module on/off switches, keyed by canonical module id
    /// (`"steering"`, `"accelerator"`, `"brake"`, `"clutch"`,
    /// `"handbrake"`, `"shifter"`, `"horn"`, `"led"`). Absent keys
    /// mean "on" (see
    /// [`sanitize_settings`]); a disabled module never reaches the
    /// pipeline. Unknown keys are preserved verbatim.
    #[serde(default = "default_modules")]
    pub modules: HashMap<String, bool>,
}

fn default_profile_name() -> String {
    "Default".to_owned()
}

fn default_deadzone() -> f64 {
    0.02
}

fn default_smoothing() -> f64 {
    0.3
}

fn default_gear_up_button() -> u32 {
    1
}

fn default_gear_down_button() -> u32 {
    2
}

fn default_pedal_press_ms() -> f64 {
    150.0
}

fn default_pedal_release_ms() -> f64 {
    100.0
}

fn default_scale() -> f64 {
    1.0
}

fn default_true() -> bool {
    true
}

fn default_gate_mode() -> String {
    "Sequential".to_owned()
}

fn default_esp_ip() -> String {
    "192.168.4.1".to_owned()
}

fn default_vjoy_device_id() -> u32 {
    1
}

fn default_output_backend() -> String {
    // v2.0.1 hotfix: the default output backend is vJoy — HIDMaestro
    // was the default until v2.0.0 (architecture Phase G), but its
    // elevated sidecar pops a UAC prompt on every launch, so it is
    // now an explicit choice only; "None" is the no-device choice.
    "VJoy".to_owned()
}

fn default_log_level() -> String {
    "Warning".to_owned()
}

/// Manual `Default` impls (Phase F): the derived `Default` gave all-zero
/// values — `steering_scale` 0, gear buttons 0, `pedal_press_ms` 0 —
/// which fail the store's own validation, and `AppSettings::default()`
/// had an empty `theme`/`esp_ip`. `Default` must match the documented
/// serde defaults above, so a freshly created profile/settings is
/// valid without a JSON round-trip. (Struct-update syntax
/// `..Default::default()` would recurse, so every field is listed.)
impl Default for PerModuleTuning {
    fn default() -> Self {
        Self {
            deadzone: 0.0,
            curve_strength: 0.0,
            smoothing: 0.0,
            invert: false,
            scale: default_scale(),
            enabled: true,
            gate_mode: default_gate_mode(),
            min_mv: None,
            max_mv: None,
            bite_point: None,
            // --- Brake per-module tuning ---
            saturation: None,
            horn_button: None,
            led_brightness: None,
            led_pattern: None,
            led_color: None,
            handbrake_button_mode: None,
            // --- Pedal ramps/curves per-module tuning ---
            press_ms: None,
            release_ms: None,
            // --- Shifter per-module tuning ---
            shift_lockout_ms: None,
            // --- Handbrake/clutch per-module tuning ---
            handbrake_latch: None,
            handbrake_auto_release: None,
            bite_width: None,
            // --- LED/horn per-module tuning ---
            led_period_ms: None,
            horn_min_ms: None,
            horn_latch: None,
        }
    }
}

impl Default for WheelProfile {
    fn default() -> Self {
        Self {
            name: default_profile_name(),
            schema_version: 0,
            steering_calibration: None,
            accel_calibration: None,
            brake_calibration: None,
            clutch_calibration: None,
            deadzone: default_deadzone(),
            curve_strength: 0.0,
            smoothing: default_smoothing(),
            invert_steering: false,
            invert_accel: false,
            invert_brake: false,
            combine_pedals: false,
            gear_up_vjoy_button: default_gear_up_button(),
            gear_down_vjoy_button: default_gear_down_button(),
            pedal_press_ms: default_pedal_press_ms(),
            pedal_release_ms: default_pedal_release_ms(),
            steering_scale: default_scale(),
            // --- Steering per-module tuning ---
            steering_balance_pct: 0.0,
            steering_center_trim_pct: 0.0,
            game_process_name: String::new(),
            car_model: String::new(),
            game: GameId::default(),
            module_tunings: BTreeMap::new(),
            board_mac: None,
            needs_recalibration: false,
            calibration_pin_map_fingerprint: None,
            calibration_firmware_version: None,
            calibrated_at_utc: None,
        }
    }
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: "System".to_string(),
            language: "System".to_string(),
            // v2.0.1: the v2 daemon is always-on by design
            // (REDESIGN-PLAN §10), so a fresh store enables
            // autostart — the installer writes the Run key and the
            // daemon must not delete it on first launch.
            autostart: true,
            start_minimized: false,
            close_to_tray: true,
            output_enabled: true,
            esp_ip: default_esp_ip(),
            vjoy_device_id: default_vjoy_device_id(),
            output_backend: default_output_backend(),
            log_level: default_log_level(),
            wizard_completed: false,
            first_run_done: false,
            auto_switch_profiles: true,
            auto_switch_paused: false,
            modules: default_modules(),
        }
    }
}

/// The canonical module ids (C-MOD-1): `"accelerator"` is canonical,
/// never `"accel"`. Every tuning / module-tuning / settings lookup
/// normalizes through [`canonical_module_id`].
pub const KNOWN_MODULES: &[&str] = &[
    "steering",
    "accelerator",
    "brake",
    "clutch",
    "handbrake",
    "shifter",
    "horn",
    "led",
];

/// Default per-module switches: every known module on.
#[must_use]
pub fn default_modules() -> HashMap<String, bool> {
    KNOWN_MODULES
        .iter()
        .map(|module| ((*module).to_string(), true))
        .collect()
}

/// Canonicalizes a module id: trims, lowercases, and maps the legacy
/// spellings to the canonical `"accelerator"` (C-MOD-1): `"accel"`
/// and `"throttle"` (the Pedals page's spelling) both read the
/// accelerator record — without this the UI's throttle edits landed
/// under a `"throttle"` key the pipeline never reads (Workstream C
/// probe: a `"throttle"` deadzone never reached `plan.accelerator`).
/// Unknown ids pass through unchanged (lowercased + trimmed).
#[must_use]
pub fn canonical_module_id(id: &str) -> String {
    match id.trim().to_ascii_lowercase().as_str() {
        "accel" | "throttle" => "accelerator".to_string(),
        other => other.to_string(),
    }
}

/// Whether an IPv4 address may be used as the board address
/// (`esp_ip`, M-1): a private-range address, or loopback (local
/// testing / the loopback forwarder tests). Anything else — a public
/// address, a hostname, unparseable text — is refused and the caller
/// falls back to the default `192.168.4.1`.
#[must_use]
pub fn is_allowed_board_ip(ip: &Ipv4Addr) -> bool {
    ip.is_private() || ip.is_loopback()
}

/// Canonical theme for a theme string, accepting both vocabularies:
/// the backend canonical `"System"` | `"Dark"` | `"Light"` |
/// `"HighContrast"` and the frontend lowercase spellings (`"system"`,
/// `"dark"`, `"light"`, `"highcontrast"` / `"high-contrast"`).
/// Returns [`None`] for anything else (the caller falls back to
/// `"System"`).
#[must_use]
pub fn canonical_theme(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "system" => Some("System"),
        "dark" => Some("Dark"),
        "light" => Some("Light"),
        "highcontrast" | "high-contrast" | "high_contrast" => Some("HighContrast"),
        _ => None,
    }
}


/// A partial settings update (C-SET-1, PATCH semantics): only the
/// fields that are `Some` overwrite the stored settings; absent fields
/// keep their existing values. This is what the `set_settings` Tauri
/// command accepts — the frontend sends only the fields it changed,
/// and a partial save can never wipe the settings it did not mention.
/// `modules` merges per key (a provided key overwrites that module's
/// switch; absent keys keep their values).
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case", default)]
pub struct AppSettingsPatch {
    /// Theme preference (either vocabulary, see [`canonical_theme`]).
    pub theme: Option<String>,
    /// Language preference.
    pub language: Option<String>,
    /// Whether the app starts with Windows.
    pub autostart: Option<bool>,
    /// Whether the app starts minimized.
    pub start_minimized: Option<bool>,
    /// Whether closing the window hides to the tray.
    pub close_to_tray: Option<bool>,
    /// Whether the output backend is enabled.
    pub output_enabled: Option<bool>,
    /// Wheel address (private IPv4 literal only).
    pub esp_ip: Option<String>,
    /// vJoy device id, clamped to 1..=16.
    pub vjoy_device_id: Option<u32>,
    /// Output backend (`VJoy` / `Xbox` / `None`).
    pub output_backend: Option<String>,
    /// Log level.
    pub log_level: Option<String>,
    /// Whether the setup wizard has been completed.
    pub wizard_completed: Option<bool>,
    /// Whether the first run has finished.
    pub first_run_done: Option<bool>,
    /// Whether profiles auto-switch per game/car.
    pub auto_switch_profiles: Option<bool>,
    /// Whether auto-switching is paused (M6, v2.0.2).
    pub auto_switch_paused: Option<bool>,
    /// Per-module switches, merged per key.
    pub modules: Option<HashMap<String, bool>>,
}

impl AppSettings {
    /// Applies a [`AppSettingsPatch`]: only `Some` fields overwrite.
    /// The result is sanitized ([`sanitize_settings`]) before it is
    /// returned, so a patch can never store an invalid theme, a
    /// public `esp_ip`, or an out-of-range vJoy device id.
    pub fn apply_patch(&mut self, patch: AppSettingsPatch) {
        if let Some(theme) = patch.theme {
            self.theme = theme;
        }
        if let Some(language) = patch.language {
            self.language = language;
        }
        if let Some(autostart) = patch.autostart {
            self.autostart = autostart;
        }
        if let Some(start_minimized) = patch.start_minimized {
            self.start_minimized = start_minimized;
        }
        if let Some(close_to_tray) = patch.close_to_tray {
            self.close_to_tray = close_to_tray;
        }
        if let Some(output_enabled) = patch.output_enabled {
            self.output_enabled = output_enabled;
        }
        if let Some(esp_ip) = patch.esp_ip {
            self.esp_ip = esp_ip;
        }
        if let Some(vjoy_device_id) = patch.vjoy_device_id {
            self.vjoy_device_id = vjoy_device_id;
        }
        if let Some(output_backend) = patch.output_backend {
            self.output_backend = output_backend;
        }
        if let Some(log_level) = patch.log_level {
            self.log_level = log_level;
        }
        if let Some(wizard_completed) = patch.wizard_completed {
            self.wizard_completed = wizard_completed;
        }
        if let Some(first_run_done) = patch.first_run_done {
            self.first_run_done = first_run_done;
        }
        if let Some(auto_switch_profiles) = patch.auto_switch_profiles {
            self.auto_switch_profiles = auto_switch_profiles;
        }
        if let Some(auto_switch_paused) = patch.auto_switch_paused {
            self.auto_switch_paused = auto_switch_paused;
        }
        if let Some(modules) = patch.modules {
            for (module, enabled) in modules {
                self.modules.insert(canonical_module_id(&module), enabled);
            }
        }
        sanitize_settings(self);
    }
}

/// Raw shape of the `profiles.json` file, before per-entry tolerance is applied.
#[derive(Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
struct ProfileFileRaw {
    schema_version: u32,
    profiles: Vec<Value>,
    active_profile_name: String,
}

/// Converts every object key from C# `PascalCase` to Rust `snake_case`,
/// recursively, so C# JSON deserializes into the `snake_case`-renamed Rust
/// structs. `VJoy` is one word in Rust field names (`vjoy_device_id`), so
/// that brand-name sequence is folded to `Vjoy` before conversion; other
/// acronym boundaries follow the usual rule (`EspIp` -> `esp_ip`).
///
/// `pub(crate)` so the store can parse a C# settings file into a
/// *patch* (merge import, M6) instead of a full replace.
pub(crate) fn normalize_keys(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let normalized = map
                .into_iter()
                .map(|(key, val)| (pascal_to_snake(&key), normalize_keys(val)))
                .collect();
            Value::Object(normalized)
        }
        Value::Array(items) => Value::Array(items.into_iter().map(normalize_keys).collect()),
        other => other,
    }
}

/// Converts one `PascalCase` (or already lowercase) key to `snake_case`.
/// An underscore is inserted before an uppercase letter when the previous
/// character is lowercase or a digit, or when the previous character is
/// uppercase and the next is lowercase (end of an acronym run).
fn pascal_to_snake(key: &str) -> String {
    let folded = key.replace("VJoy", "Vjoy");
    let chars: Vec<char> = folded.chars().collect();
    let mut out = String::with_capacity(folded.len() + 4);
    for (i, &ch) in chars.iter().enumerate() {
        if ch.is_ascii_uppercase() {
            let prev = if i == 0 { None } else { Some(chars[i - 1]) };
            let next = chars.get(i + 1).copied();
            let boundary = match prev {
                None => false,
                Some(p) => {
                    p.is_ascii_lowercase()
                        || p.is_ascii_digit()
                        || (p.is_ascii_uppercase() && next.is_some_and(|n| n.is_ascii_lowercase()))
                }
            };
            if boundary {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// Whether a set steering calibration is sane (see [`WheelProfile::computed_needs_recalibration`]).
fn steering_calibration_sane(calibration: &SteeringCalibration) -> bool {
    if !calibration.is_set {
        return true;
    }
    if !calibration.is_millivolts {
        return false;
    }
    if !(calibration.raw_min < calibration.raw_center
        && calibration.raw_center < calibration.raw_max)
    {
        return false;
    }
    calibration
        .raw_max
        .checked_sub(calibration.raw_min)
        .is_some_and(|span| span >= MIN_CALIBRATION_SPAN_MV)
}

/// Whether a set pedal calibration is sane (see [`WheelProfile::computed_needs_recalibration`]).
fn pedal_calibration_sane(calibration: &PedalCalibration) -> bool {
    if !calibration.is_set {
        return true;
    }
    if !calibration.is_millivolts {
        return false;
    }
    calibration
        .raw_max
        .checked_sub(calibration.raw_min)
        .is_some_and(|span| span >= MIN_CALIBRATION_SPAN_MV)
}

/// Applies the C# load-time sanitizing for settings allowlists and ranges
/// (`DATA-INPUT.md` §2.1): unknown enum strings fall back to their defaults,
/// a non-private/non-loopback `esp_ip` falls back to `192.168.4.1`, and the
/// vJoy device id is clamped to 1..=16. Both theme vocabularies are
/// accepted (backend canonical + frontend lowercase, see
/// [`canonical_theme`]), and absent known-module switches are filled
/// in as on. Applied on every load path (C# import, Rust load) and on
/// every write path (`set_settings`, patch apply) — M10.
pub fn sanitize_settings(settings: &mut AppSettings) {
    settings.theme = canonical_theme(&settings.theme)
        .unwrap_or("System")
        .to_string();
    if !matches!(settings.language.as_str(), "System" | "en" | "ur" | "hi") {
        "System".clone_into(&mut settings.language);
    }
    let esp_ok = settings
        .esp_ip
        .parse::<Ipv4Addr>()
        .is_ok_and(|ip| is_allowed_board_ip(&ip));
    if !esp_ok {
        settings.esp_ip = default_esp_ip();
    }
    settings.vjoy_device_id = settings.vjoy_device_id.clamp(1, 16);
    // v2.0.19: the HIDMaestro backend is gone — a persisted
    // "HIDMaestro" value falls back to the vJoy default, same as
    // any other unknown value.
    if !matches!(
        settings.output_backend.as_str(),
        "VJoy" | "Xbox" | "None"
    ) {
        settings.output_backend = default_output_backend();
    }
    if !matches!(
        settings.log_level.as_str(),
        "Debug" | "Information" | "Warning" | "Error"
    ) {
        settings.log_level = default_log_level();
    }
    for module in KNOWN_MODULES {
        settings
            .modules
            .entry((*module).to_string())
            .or_insert(true);
    }
    // Legacy "accel" module keys migrate to the canonical id (C-MOD-1).
    if let Some(enabled) = settings.modules.remove("accel") {
        settings.modules.insert("accelerator".to_string(), enabled);
    }
}

/// Parses C# `profiles.json` text (Profile v2, `PascalCase` JSON).
///
/// Read-only: the input is only parsed, never written anywhere.
///
/// # Errors
///
/// Returns [`ImportError::TooLarge`] if the text exceeds
/// [`MAX_PROFILES_FILE_BYTES`], [`ImportError::Json`] if it is not valid
/// JSON or a profile entry cannot be represented, and [`ImportError::Schema`]
/// if the file-level `schemaVersion` is above 2.
pub fn parse_csharp_profiles(json: &str) -> Result<ProfileFile, ImportError> {
    if json.len() as u64 > MAX_PROFILES_FILE_BYTES {
        return Err(ImportError::TooLarge {
            limit: MAX_PROFILES_FILE_BYTES,
        });
    }
    let value: Value = serde_json::from_str(json)?;
    let raw = serde_json::from_value::<ProfileFileRaw>(normalize_keys(value))?;
    let schema_version = raw.schema_version;
    if schema_version > 2 {
        return Err(ImportError::Schema(format!(
            "schemaVersion {schema_version} is above 2"
        )));
    }
    let mut profiles = Vec::new();
    let mut skipped_profiles = 0;
    for entry in raw.profiles {
        match serde_json::from_value::<WheelProfile>(entry) {
            Ok(profile) => profiles.push(profile),
            Err(_) => skipped_profiles += 1,
        }
    }
    Ok(ProfileFile {
        schema_version,
        profiles,
        active_profile_name: raw.active_profile_name,
        skipped_profiles,
    })
}

/// Parses C# `settings.json` text (`PascalCase` JSON).
///
/// Read-only: the input is only parsed, never written anywhere. Settings
/// allowlists and ranges are sanitized on parse (see [`sanitize_settings`]).
///
/// # Errors
///
/// Returns [`ImportError::TooLarge`] if the text exceeds
/// [`MAX_SETTINGS_FILE_BYTES`] and [`ImportError::Json`] if it is not valid
/// JSON.
pub fn parse_csharp_settings(json: &str) -> Result<AppSettings, ImportError> {
    if json.len() as u64 > MAX_SETTINGS_FILE_BYTES {
        return Err(ImportError::TooLarge {
            limit: MAX_SETTINGS_FILE_BYTES,
        });
    }
    let value: Value = serde_json::from_str(json)?;
    let mut settings = serde_json::from_value::<AppSettings>(normalize_keys(value))?;
    sanitize_settings(&mut settings);
    Ok(settings)
}

/// Reads a C# `profiles.json` file without modifying it.
///
/// The file is only read (`fs::metadata` + `fs::read_to_string`); nothing in
/// this function writes, deletes, or renames anything.
///
/// # Errors
///
/// Returns [`ImportError::Io`] if the file cannot be read,
/// [`ImportError::TooLarge`] if it exceeds [`MAX_PROFILES_FILE_BYTES`], and
/// the [`parse_csharp_profiles`] errors otherwise.
pub fn read_csharp_profiles_file(path: &Path) -> Result<ProfileFile, ImportError> {
    let metadata = fs::metadata(path)?;
    if metadata.len() > MAX_PROFILES_FILE_BYTES {
        return Err(ImportError::TooLarge {
            limit: MAX_PROFILES_FILE_BYTES,
        });
    }
    let text = fs::read_to_string(path)?;
    parse_csharp_profiles(&text)
}

/// Reads a C# `settings.json` file without modifying it.
///
/// The file is only read (`fs::metadata` + `fs::read_to_string`); nothing in
/// this function writes, deletes, or renames anything.
///
/// # Errors
///
/// Returns [`ImportError::Io`] if the file cannot be read,
/// [`ImportError::TooLarge`] if it exceeds [`MAX_SETTINGS_FILE_BYTES`], and
/// the [`parse_csharp_settings`] errors otherwise.
pub fn read_csharp_settings_file(path: &Path) -> Result<AppSettings, ImportError> {
    let metadata = fs::metadata(path)?;
    if metadata.len() > MAX_SETTINGS_FILE_BYTES {
        return Err(ImportError::TooLarge {
            limit: MAX_SETTINGS_FILE_BYTES,
        });
    }
    let text = fs::read_to_string(path)?;
    parse_csharp_settings(&text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::hash::{Hash, Hasher};

    /// Sample C# v2 `profiles.json`, built from the `DATA-INPUT.md` §1.2
    /// schema: one valid millivolt profile, one v1 unit-less profile, and
    /// one broken entry (a string where a number belongs).
    const SAMPLE_PROFILES_JSON: &str = r#"{
      "SchemaVersion": 2,
      "Profiles": [
        {
          "Name": "Default",
          "SchemaVersion": 2,
          "SteeringCalibration": {"RawMin": 300, "RawCenter": 1650, "RawMax": 3000, "IsSet": true, "IsMillivolts": true},
          "AccelCalibration": {"RawMin": 300, "RawMax": 3100, "IsSet": true, "IsMillivolts": true},
          "BrakeCalibration": {"RawMin": 300, "RawMax": 3100, "IsSet": false, "IsMillivolts": true},
          "Deadzone": 0.02,
          "CurveStrength": 0.0,
          "Smoothing": 0.3,
          "InvertSteering": false,
          "InvertAccel": false,
          "InvertBrake": false,
          "CombinePedals": false,
          "GearUpVJoyButton": 1,
          "GearDownVJoyButton": 2,
          "PedalPressMs": 150,
          "PedalReleaseMs": 100,
          "SteeringScale": 1.0,
          "GameProcessName": "",
          "CarModel": "",
          "Game": 0,
          "ModuleTunings": {
            "Steering": {"Deadzone": 0.02, "CurveStrength": 0.0, "Smoothing": 0.3, "Invert": false, "Scale": 1.0, "Enabled": true, "GateMode": "Sequential"}
          },
          "BoardMac": "AA:BB:CC:DD:EE:FF",
          "NeedsRecalibration": false,
          "CalibratedAtUtc": "2026-11-13T10:00:00Z"
        },
        {
          "Name": "OldWheel",
          "SchemaVersion": 1,
          "SteeringCalibration": {"RawMin": 100, "RawCenter": 2048, "RawMax": 4000, "IsSet": true, "IsMillivolts": false},
          "Deadzone": 0.02,
          "CurveStrength": 0.0,
          "Smoothing": 0.3,
          "Game": 99
        },
        {
          "Name": "Broken",
          "Deadzone": "wide"
        }
      ],
      "ActiveProfileName": "Default"
    }"#;

    /// Sample C# `settings.json` (`PascalCase`), from `DATA-INPUT.md` §2.1.
    const SAMPLE_SETTINGS_JSON: &str = r#"{
      "Theme": "Dark",
      "Language": "en",
      "Autostart": false,
      "StartMinimized": false,
      "CloseToTray": true,
      "OutputEnabled": true,
      "EspIp": "10.0.0.5",
      "VJoyDeviceId": 1,
      "OutputBackend": "HIDMaestro",
      "LogLevel": "Warning",
      "WizardCompleted": true,
      "FirstRunDone": true,
      "AutoSwitchProfiles": true
    }"#;

    fn hash_bytes(bytes: &[u8]) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        bytes.hash(&mut hasher);
        hasher.finish()
    }

    #[test]
    fn parses_pascal_case_v2_profiles_with_f3_tolerance() {
        let file =
            parse_csharp_profiles(SAMPLE_PROFILES_JSON).expect("sample C# v2 profiles parse");
        assert_eq!(file.schema_version, 2);
        assert_eq!(file.active_profile_name, "Default");
        // Two valid entries; the broken one is skipped, not fatal.
        assert_eq!(file.profiles.len(), 2);
        assert_eq!(file.skipped_profiles, 1);

        let default = &file.profiles[0];
        assert_eq!(default.name, "Default");
        assert_eq!(default.game, GameId::Unknown);
        assert_eq!(default.board_mac.as_deref(), Some("AA:BB:CC:DD:EE:FF"));
        assert_eq!(
            default.calibrated_at_utc.as_deref(),
            Some("2026-11-13T10:00:00Z")
        );
        assert!(!default.computed_needs_recalibration());
        let steering = default.steering_calibration.expect("steering set");
        assert_eq!(steering.raw_max - steering.raw_min, 2700);
        assert!(default.module_tunings.contains_key("steering"));

        // v1 unit-less calibration is never silently treated as millivolts.
        let old = &file.profiles[1];
        assert_eq!(old.name, "OldWheel");
        assert_eq!(old.schema_version, 1);
        assert_eq!(old.game, GameId::Unknown); // 99 maps to Unknown
        assert!(old.computed_needs_recalibration());
    }

    #[test]
    fn parses_camel_case_keys_the_same_way() {
        // Case-insensitive parse (DATA-INPUT §1.2): PascalCase and camelCase
        // keys both normalize to snake_case. Fully lower-cased keys without
        // word boundaries ("schemaversion") cannot be recovered and are not
        // a format the C# app writes, so they are out of scope here.
        let json = r#"{
          "schemaVersion": 2,
          "profiles": [
            {"name": "Lower", "deadzone": 0.05, "game": 1}
          ],
          "activeProfileName": "Lower"
        }"#;
        let file = parse_csharp_profiles(json).expect("camelCase keys parse");
        assert_eq!(file.schema_version, 2);
        assert_eq!(file.profiles.len(), 1);
        assert_eq!(file.profiles[0].name, "Lower");
        assert_eq!(file.profiles[0].game, GameId::AssettoCorsa);
        assert_eq!(file.active_profile_name, "Lower");
    }

    #[test]
    fn parses_settings_and_keeps_private_ip() {
        let settings = parse_csharp_settings(SAMPLE_SETTINGS_JSON).expect("sample settings parse");
        assert_eq!(settings.theme, "Dark");
        assert_eq!(settings.esp_ip, "10.0.0.5");
        // v2.0.19: the sample's persisted "HIDMaestro" migrates
        // to the vJoy default — the HIDMaestro backend is gone.
        assert_eq!(settings.output_backend, "VJoy");
        assert!(settings.close_to_tray);
        assert!(settings.auto_switch_profiles);
    }

    #[test]
    fn sanitizes_bad_settings_values_to_defaults() {
        let json = r#"{
          "Theme": "Banana",
          "Language": "xx",
          "EspIp": "https://evil.example/x",
          "VJoyDeviceId": 99,
          "OutputBackend": "Sideways",
          "LogLevel": "Chatty"
        }"#;
        let settings = parse_csharp_settings(json).expect("bad settings still parse");
        assert_eq!(settings.theme, "System");
        assert_eq!(settings.language, "System");
        assert_eq!(settings.esp_ip, "192.168.4.1");
        assert_eq!(settings.vjoy_device_id, 16);
        assert_eq!(settings.output_backend, "VJoy");
        assert_eq!(settings.log_level, "Warning");
    }

    #[test]
    fn rejects_schema_version_above_two() {
        let json = r#"{"SchemaVersion": 3, "Profiles": [], "ActiveProfileName": ""}"#;
        let error = parse_csharp_profiles(json).expect_err("schema v3 must reject");
        assert!(matches!(error, ImportError::Schema(_)));
    }

    #[test]
    fn rejects_oversized_inputs() {
        let big_profiles = "x"
            .repeat(usize::try_from(MAX_PROFILES_FILE_BYTES).expect("1 MiB cap fits in usize") + 1);
        assert!(matches!(
            parse_csharp_profiles(&big_profiles).expect_err("profiles cap"),
            ImportError::TooLarge { .. }
        ));
        let big_settings = "x".repeat(
            usize::try_from(MAX_SETTINGS_FILE_BYTES).expect("64 KiB cap fits in usize") + 1,
        );
        assert!(matches!(
            parse_csharp_settings(&big_settings).expect_err("settings cap"),
            ImportError::TooLarge { .. }
        ));
    }

    #[test]
    fn file_reader_does_not_modify_the_csharp_file() {
        let path =
            std::env::temp_dir().join(format!("rsh-a2-import-test-{}.json", std::process::id()));
        fs::write(&path, SAMPLE_PROFILES_JSON).expect("write temp sample");
        let before = fs::read(&path).expect("read temp sample before");
        let before_hash = hash_bytes(&before);

        let file = read_csharp_profiles_file(&path).expect("read temp sample");
        assert_eq!(file.profiles.len(), 2);

        let after = fs::read(&path).expect("read temp sample after");
        assert_eq!(hash_bytes(&after), before_hash);
        assert_eq!(after, before);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn missing_file_reports_io_error() {
        let path = std::env::temp_dir().join("rsh-a2-does-not-exist-000000.json");
        let error = read_csharp_profiles_file(&path).expect_err("missing file errors");
        assert!(matches!(error, ImportError::Io(_)));
    }

    #[test]
    fn default_settings_enable_autostart() {
        // v2.0.1: a fresh store autostarts (always-on daemon,
        // REDESIGN-PLAN §10) — the installer writes the Run key
        // and the daemon must not delete it on first launch.
        assert!(AppSettings::default().autostart);
    }
}
