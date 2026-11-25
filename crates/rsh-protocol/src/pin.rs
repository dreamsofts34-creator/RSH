//! PIN-RULES pure validator (WP1/WP2).
//!
//! Rule source: `board-esp32-devkitv1.json`, embedded with
//! `include_str!` — the pin table is **never** hard-coded in Rust.
//! Logic mirrors `reference/tests-host/pin_rules_ref.py` exactly
//! (same checks, same order, same error strings), so the Rust
//! validator and the Python reference agree on every input.
//!
//! Limits (D6): axes ≤ 8, buttons ≤ 128, encoders ≤ 4, LED strips ≤ 1
//! (≤ 60 LEDs/strip), batteries ≤ 1 — and a battery is **not** an axis
//! (T7 fix).

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use serde_json::Value;

use crate::config::{ConfigBlob, record_type};

/// Embedded board description (single source of truth for pin rules).
const BOARD_JSON: &str = include_str!("board-esp32-devkitv1.json");

/// Parsed board JSON, initialised once.
fn board() -> &'static Value {
    static BOARD: OnceLock<Value> = OnceLock::new();
    BOARD.get_or_init(|| serde_json::from_str(BOARD_JSON).expect("embedded board JSON is valid"))
}

/// Read a JSON number as `u32`, falling back to `default` when the
/// value is missing, not a number, or out of range.
fn json_u32(value: &Value, default: u32) -> u32 {
    value
        .as_u64()
        .and_then(|v| u32::try_from(v).ok())
        .unwrap_or(default)
}

/// Read a JSON number as `u8`, falling back to `default` when the
/// value is missing, not a number, or out of range.
fn json_u8(value: &Value, default: u8) -> u8 {
    value
        .as_u64()
        .and_then(|v| u8::try_from(v).ok())
        .unwrap_or(default)
}

/// Board limits from the embedded JSON: `(axes, buttons, encoders,
/// led_strips, leds_per_strip, batteries)`.
#[must_use]
pub fn board_limits() -> (u32, u32, u32, u32, u32, u32) {
    let limits = &board()["limits"];
    (
        json_u32(&limits["axes"], 8),
        json_u32(&limits["buttons"], 128),
        json_u32(&limits["encoders"], 4),
        json_u32(&limits["led_strips"], 1),
        json_u32(&limits["leds_per_strip"], 60),
        json_u32(&limits["batteries"], 1),
    )
}

/// Reserved pins from the embedded JSON: `(i2c_sda, i2c_scl, status_led)`.
#[must_use]
pub fn reserved_pins() -> (u8, u8, u8) {
    let reserved = &board()["reserved"];
    (
        json_u8(&reserved["i2c_sda"], 21),
        json_u8(&reserved["i2c_scl"], 22),
        json_u8(&reserved["status_led"], 2),
    )
}

/// Module kinds, mirroring the Python validator's `KINDS`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PinModuleKind {
    /// Direct ADC axis (ADC1 pin).
    Axis,
    /// ADS1115 I²C ADC axis (channel 0–3, no GPIO).
    AxisAds1115,
    /// Battery sense (ADC1 pin; not counted as an axis).
    Battery,
    /// Direct digital button.
    Button,
    /// Button matrix (`rows × cols` keys on `rows + cols` pins).
    ButtonMatrix,
    /// Encoder (exactly 2 pins, A + B).
    Encoder,
    /// Expander button (MCP23017 / PCF8574, no GPIO).
    ExpanderButton,
    /// LED strip (digital-out pin, 1–60 LEDs).
    Led,
    /// AS5600 magnetic encoder axis (I²C, max 1/board).
    AxisAs5600,
}

impl PinModuleKind {
    /// Kind name as used in the Python reference and the board JSON.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Axis => "axis",
            Self::AxisAds1115 => "axis_ads1115",
            Self::Battery => "battery",
            Self::Button => "button",
            Self::ButtonMatrix => "button_matrix",
            Self::Encoder => "encoder",
            Self::ExpanderButton => "expander_button",
            Self::Led => "led",
            Self::AxisAs5600 => "axis_as5600",
        }
    }

    /// Parse a kind name; `None` for unknown kinds.
    #[must_use]
    pub fn from_name(s: &str) -> Option<Self> {
        Some(match s {
            "axis" => Self::Axis,
            "axis_ads1115" => Self::AxisAds1115,
            "battery" => Self::Battery,
            "button" => Self::Button,
            "button_matrix" => Self::ButtonMatrix,
            "encoder" => Self::Encoder,
            "expander_button" => Self::ExpanderButton,
            "led" => Self::Led,
            "axis_as5600" => Self::AxisAs5600,
            _ => return None,
        })
    }
}

/// One module to validate: a kind plus its pins and kind-specific options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinModule {
    /// Module kind.
    pub kind: PinModuleKind,
    /// ESP32 GPIO pins used (`[]` for I²C kinds).
    pub pins: Vec<u8>,
    /// External pull-up fitted (buttons/encoders on input-only pins).
    pub ext_pullup: bool,
    /// ADS1115 channel (0–3) for `AxisAds1115`.
    pub channel: u8,
    /// I²C address for `AxisAds1115` / `ExpanderButton` (default 0x48 /
    /// 0x20 — see the constructors).
    pub addr: u8,
    /// Expander chip for `ExpanderButton` (`"MCP23017"` / `"PCF8574"`).
    pub expander: String,
    /// Expander pin number for `ExpanderButton`.
    pub expander_pin: Option<u8>,
    /// Matrix rows for `ButtonMatrix` (1–8).
    pub rows: u8,
    /// Matrix columns for `ButtonMatrix` (1–8).
    pub cols: u8,
    /// LED count for `Led` (1–60).
    pub count: Option<u8>,
}

impl PinModule {
    /// Direct ADC axis on one pin.
    #[must_use]
    pub fn axis(pin: u8) -> Self {
        Self::base(PinModuleKind::Axis, vec![pin])
    }

    /// Direct digital button on one pin.
    #[must_use]
    pub fn button(pin: u8) -> Self {
        Self::base(PinModuleKind::Button, vec![pin])
    }

    /// Encoder on two pins (A, B).
    #[must_use]
    pub fn encoder(pin_a: u8, pin_b: u8) -> Self {
        Self::base(PinModuleKind::Encoder, vec![pin_a, pin_b])
    }

    /// LED strip on one digital-out pin.
    #[must_use]
    pub fn led(pin: u8) -> Self {
        Self::base(PinModuleKind::Led, vec![pin])
    }

    /// Battery sense on one ADC1 pin.
    #[must_use]
    pub fn battery(pin: u8) -> Self {
        Self::base(PinModuleKind::Battery, vec![pin])
    }

    /// ADS1115 axis on a channel (0–3); I²C address defaults to 0x48.
    #[must_use]
    pub fn axis_ads1115(channel: u8) -> Self {
        let mut module = Self::base(PinModuleKind::AxisAds1115, vec![]);
        module.channel = channel;
        module.addr = 0x48;
        module
    }

    /// Expander button; defaults to MCP23017 at 0x20.
    #[must_use]
    pub fn expander_button(expander_pin: u8) -> Self {
        let mut module = Self::base(PinModuleKind::ExpanderButton, vec![]);
        module.expander_pin = Some(expander_pin);
        module
    }

    /// Button matrix on `rows + cols` pins (default 4×4).
    #[must_use]
    pub fn button_matrix(pins: Vec<u8>) -> Self {
        let mut module = Self::base(PinModuleKind::ButtonMatrix, pins);
        module.rows = 4;
        module.cols = 4;
        module
    }

    /// AS5600 magnetic encoder axis (I²C, no GPIO).
    #[must_use]
    pub fn axis_as5600() -> Self {
        Self::base(PinModuleKind::AxisAs5600, vec![])
    }

    fn base(kind: PinModuleKind, pins: Vec<u8>) -> Self {
        Self {
            kind,
            pins,
            ext_pullup: false,
            channel: 0,
            addr: 0x20,
            expander: "MCP23017".to_string(),
            expander_pin: None,
            rows: 4,
            cols: 4,
            count: None,
        }
    }

    /// Set the external-pull-up flag (builder style).
    #[must_use]
    pub fn with_ext_pullup(mut self, ext_pullup: bool) -> Self {
        self.ext_pullup = ext_pullup;
        self
    }

    /// Set the LED count (builder style).
    #[must_use]
    pub fn with_count(mut self, count: u8) -> Self {
        self.count = Some(count);
        self
    }

    /// Set matrix rows/cols (builder style).
    #[must_use]
    pub fn with_rows_cols(mut self, rows: u8, cols: u8) -> Self {
        self.rows = rows;
        self.cols = cols;
        self
    }

    /// Set the I²C address (builder style).
    #[must_use]
    pub fn with_addr(mut self, addr: u8) -> Self {
        self.addr = addr;
        self
    }

    /// Set the expander chip name (builder style).
    #[must_use]
    pub fn with_expander(mut self, expander: &str) -> Self {
        self.expander = expander.to_string();
        self
    }
}

/// Shared head of every pin check (C++ `rsh_cfg_pin_check_` barred +
/// duplicate steps): nonexistent / forbidden / avoid / already-used.
/// Empty = the pin's class is usable (kind-specific electrical rules
/// still apply on top). Also the whole of the class-only check
/// (`RSH_PIN_RULE_CLASS_ONLY`) — see [`validate_blob`].
fn pin_class_errors(pin: u8, used: &HashMap<u8, String>) -> Vec<String> {
    let pins = &board()["pins"];
    let Some(info) = pins.get(pin.to_string()) else {
        return vec![format!("GPIO{pin}: does not exist")];
    };
    let class = info.get("class").and_then(Value::as_str).unwrap_or("");
    if class == "forbidden" {
        let reason = info.get("reason").and_then(Value::as_str).unwrap_or("");
        return vec![format!("GPIO{pin}: forbidden ({reason})")];
    }
    if class == "avoid" {
        let reason = info.get("reason").and_then(Value::as_str).unwrap_or("");
        return vec![format!("GPIO{pin}: avoided ({reason})")];
    }
    if let Some(owner) = used.get(&pin) {
        return vec![format!("GPIO{pin}: already used by {owner}")];
    }
    Vec::new()
}

/// Per-GPIO checks shared by every kind that uses ESP32 pins.
/// Returns the error list for this pin (empty = usable for this kind).
/// Order matches the Python reference: forbidden/avoid first, then
/// already-used, then kind-specific electrical rules.
fn pin_errors(
    pin: u8,
    kind: PinModuleKind,
    ext_pullup: bool,
    used: &HashMap<u8, String>,
) -> Vec<String> {
    let class_errs = pin_class_errors(pin, used);
    if !class_errs.is_empty() {
        return class_errs;
    }
    let pins = &board()["pins"];
    let info = pins
        .get(pin.to_string())
        .expect("pin_class_errors proved the pin exists");
    let mut errs = Vec::new();
    let has_adc = info.get("adc").is_some_and(|v| !v.is_null());
    let has_pullup = info.get("pullup").and_then(Value::as_bool).unwrap_or(false);
    let can_drive = info
        .get("digital_out")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if matches!(kind, PinModuleKind::Axis | PinModuleKind::Battery) && !has_adc {
        errs.push(format!(
            "GPIO{pin}: not ADC1 — unusable for analog with Wi-Fi"
        ));
    }
    if matches!(
        kind,
        PinModuleKind::Button | PinModuleKind::Encoder | PinModuleKind::ButtonMatrix
    ) && !has_pullup
        && !ext_pullup
    {
        errs.push(format!(
            "GPIO{pin}: no internal pull-up (set external pull-up)"
        ));
    }
    if kind == PinModuleKind::Led && !can_drive {
        errs.push(format!("GPIO{pin}: cannot drive output"));
    }
    errs
}

/// Check every GPIO pin of one module against the board rules,
/// recording valid pins in `used`.
fn check_module_pins(module: &PinModule, used: &mut HashMap<u8, String>, errs: &mut Vec<String>) {
    for &pin in &module.pins {
        let perr = pin_errors(pin, module.kind, module.ext_pullup, used);
        errs.extend(perr.iter().cloned());
        if perr.is_empty() {
            used.insert(pin, module.kind.as_str().to_string());
        }
    }
}

/// Validate a list of modules. Returns the error list (empty = valid).
///
/// `expanders` must be true when any I²C kind is used — it reserves
/// GPIO21/22 for the I²C bus. Mirrors `pin_rules_ref.validate`.
#[must_use]
/// Running counts of validated modules, for the D6 limit checks.
#[derive(Clone, Copy, Debug, Default)]
struct ModuleCounts {
    axis: u32,
    button: u32,
    encoder: u32,
    led: u32,
    battery: u32,
}

/// Validate one I²C kind (`axis_ads1115` / `axis_as5600` /
/// `expander_button`): no GPIO pins, I²C bus required, channel/pin
/// uniqueness. Returns its errors and bumps `counts`.
fn validate_i2c_module(
    module: &PinModule,
    expanders: bool,
    ads_used: &mut HashSet<(u8, u8)>,
    exp_used: &mut HashSet<(String, u8, u8)>,
    as5600_used: &mut bool,
    counts: &mut ModuleCounts,
) -> Vec<String> {
    let mut errs = Vec::new();
    match module.kind {
        PinModuleKind::AxisAds1115 => {
            if !module.pins.is_empty() {
                errs.push("axis_ads1115: uses I2C, no GPIO pin".to_string());
            }
            if !expanders {
                errs.push("axis_ads1115: requires I2C expander (pass expanders=True)".to_string());
            }
            if module.channel > 3 {
                errs.push(format!(
                    "axis_ads1115: channel {} out of range (0-3)",
                    module.channel
                ));
            } else if !ads_used.insert((module.addr, module.channel)) {
                errs.push(format!(
                    "axis_ads1115: channel {} on addr {} already used",
                    module.channel, module.addr
                ));
            }
            counts.axis += 1;
        }
        PinModuleKind::AxisAs5600 => {
            if !module.pins.is_empty() {
                errs.push("axis_as5600: uses I2C, no GPIO pin".to_string());
            }
            if !expanders {
                errs.push("axis_as5600: requires I2C expander (pass expanders=True)".to_string());
            }
            if *as5600_used {
                errs.push(
                    "axis_as5600: only one AS5600 per board (fixed I2C address 0x36)".to_string(),
                );
            } else {
                *as5600_used = true;
            }
            counts.axis += 1;
        }
        PinModuleKind::ExpanderButton => {
            if !module.pins.is_empty() {
                errs.push("expander_button: uses no ESP32 GPIO pin".to_string());
            }
            if !expanders {
                errs.push(
                    "expander_button: requires I2C expander (pass expanders=True)".to_string(),
                );
            }
            if let Some(ep) = module.expander_pin {
                let hi: u8 = if module.expander == "PCF8574" { 7 } else { 15 };
                if ep > hi {
                    errs.push(format!(
                        "expander_button: pin {ep} out of range for {kind} (0-{hi})",
                        kind = module.expander
                    ));
                } else if !exp_used.insert((module.expander.clone(), module.addr, ep)) {
                    errs.push(format!(
                        "expander_button: {} addr {} pin {ep} already used",
                        module.expander, module.addr
                    ));
                }
            }
            counts.button += 1;
        }
        _ => {}
    }
    errs
}

/// Append the D6 limit errors for the final counts.
fn append_limit_errors(counts: &ModuleCounts, errs: &mut Vec<String>) {
    let (axes_lim, buttons_lim, encoders_lim, led_strips_lim, _, batteries_lim) = board_limits();
    if counts.axis > axes_lim {
        errs.push(format!("too many axes ({} > {axes_lim})", counts.axis));
    }
    if counts.button > buttons_lim {
        errs.push(format!(
            "too many buttons ({} > {buttons_lim})",
            counts.button
        ));
    }
    if counts.encoder > encoders_lim {
        errs.push(format!(
            "too many encoders ({} > {encoders_lim})",
            counts.encoder
        ));
    }
    if counts.led > led_strips_lim {
        errs.push(format!(
            "too many LED strips ({} > {led_strips_lim})",
            counts.led
        ));
    }
    if counts.battery > batteries_lim {
        errs.push(format!(
            "too many battery modules ({} > {batteries_lim})",
            counts.battery
        ));
    }
}

/// Validate a list of modules. Returns the error list (empty = valid).
///
/// `expanders` must be true when any I²C kind is used — it reserves
/// GPIO21/22 for the I²C bus. Mirrors `pin_rules_ref.validate`.
#[must_use]
pub fn validate(modules: &[PinModule], expanders: bool) -> Vec<String> {
    let (_, _, _, _, leds_per_strip, _) = board_limits();
    let (i2c_sda, i2c_scl, status_led) = reserved_pins();
    let mut errs: Vec<String> = Vec::new();
    let mut used: HashMap<u8, String> = HashMap::new();
    if modules.is_empty() {
        errs.push("at least one input module required".to_string());
    }
    if expanders {
        used.insert(i2c_sda, "i2c".to_string());
        used.insert(i2c_scl, "i2c".to_string());
    }
    used.insert(status_led, "status LED".to_string());
    let mut counts = ModuleCounts::default();
    let mut ads_used: HashSet<(u8, u8)> = HashSet::new();
    let mut exp_used: HashSet<(String, u8, u8)> = HashSet::new();
    let mut as5600_used = false;

    for module in modules {
        let kind = module.kind;
        if matches!(
            kind,
            PinModuleKind::AxisAds1115 | PinModuleKind::AxisAs5600 | PinModuleKind::ExpanderButton
        ) {
            errs.extend(validate_i2c_module(
                module,
                expanders,
                &mut ads_used,
                &mut exp_used,
                &mut as5600_used,
                &mut counts,
            ));
            continue;
        }
        if kind == PinModuleKind::ButtonMatrix {
            let rows = module.rows;
            let cols = module.cols;
            if !(1..=8).contains(&rows) || !(1..=8).contains(&cols) {
                errs.push(format!(
                    "button_matrix: rows/cols must be 1-8 (got {rows}x{cols})"
                ));
                continue;
            }
            if module.pins.len() != (rows as usize + cols as usize) {
                errs.push(format!(
                    "button_matrix: needs {} pins (rows+cols), got {}",
                    rows as usize + cols as usize,
                    module.pins.len()
                ));
                continue;
            }
            check_module_pins(module, &mut used, &mut errs);
            counts.button += u32::from(rows) * u32::from(cols);
            continue;
        }
        if kind == PinModuleKind::Encoder && module.pins.len() != 2 {
            errs.push(format!(
                "encoder: needs exactly 2 pins (A, B), got {}",
                module.pins.len()
            ));
            continue;
        }
        if matches!(
            kind,
            PinModuleKind::Axis
                | PinModuleKind::Battery
                | PinModuleKind::Button
                | PinModuleKind::Led
        ) && module.pins.len() != 1
        {
            errs.push(format!(
                "{}: needs exactly 1 pin, got {}",
                kind.as_str(),
                module.pins.len()
            ));
            continue;
        }
        check_module_pins(module, &mut used, &mut errs);
        match kind {
            PinModuleKind::Axis => counts.axis += 1,
            PinModuleKind::Battery => counts.battery += 1,
            PinModuleKind::Button => counts.button += 1,
            PinModuleKind::Encoder => counts.encoder += 1,
            PinModuleKind::Led => {
                counts.led += 1;
                if let Some(count) = module.count
                    && (count < 1 || u32::from(count) > leds_per_strip)
                {
                    errs.push(format!(
                        "led: count {count} out of range (1-{leds_per_strip})"
                    ));
                }
            }
            _ => {}
        }
    }

    append_limit_errors(&counts, &mut errs);
    errs
}

/// Free `ok`-class pins for a module list (mirrors `free_pins`).
///
/// Sorted numerically. With `expanders`, GPIO21/22 are already reserved
/// for I²C; without expanders they are free digital pins.
#[must_use]
pub fn free_pins(modules: &[PinModule], expanders: bool) -> Vec<u8> {
    let (i2c_sda, i2c_scl, status_led) = reserved_pins();
    let mut used: HashSet<u8> = HashSet::new();
    used.insert(status_led);
    if expanders {
        used.insert(i2c_sda);
        used.insert(i2c_scl);
    }
    for module in modules {
        used.extend(module.pins.iter().copied());
    }
    let pins = &board()["pins"];
    let mut free: Vec<u8> = pins
        .as_object()
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| {
                    let pin: u8 = k.parse().ok()?;
                    if v.get("class").and_then(Value::as_str) == Some("ok") && !used.contains(&pin)
                    {
                        Some(pin)
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    free.sort_unstable();
    free
}

/// Handbrake button-mode flag (v4.0.2 F-15): `AXIS_ADC`/`AXIS_GENERIC`
/// record with `slot == 4` and this bit set is a digital button
/// (pull-up rule), not an analog axis (ADC1 rule).
const HANDBRAKE_BUTTON_FLAG: u8 = 0x01;

/// Handbrake slot (axis slot 4 — see §3.6 slot table).
const HANDBRAKE_SLOT: u8 = 4;

/// Shifter sensitivity: `SHIFTER` record `params[4]` (v4.0.2 D-4),
/// 0..100; default 50 when no SHIFTER record exists.
pub const SHIFTER_SENSITIVITY_PARAM: usize = 4;
/// Default shifter sensitivity (→ 13 ms debounce, see
/// [`shifter_debounce_ms`]).
const SHIFTER_SENSITIVITY_DEFAULT: u8 = 50;

/// Map one config record to its [`PinModule`] (or `None` when the
/// record carries no pin, e.g. `pinA == 0xFF`). Sets `*expanders` when
/// the record is an I²C kind. Returns an error string for unknown
/// record types. SHIFTER records are handled separately by
/// [`shifter_errors`] and map to no module here.
// One arm per record type: splitting it would scatter the §3.6
// record → module mapping across helpers.
#[allow(clippy::too_many_lines)]
fn blob_record_module(
    record: &crate::config::ConfigRecord,
    expanders: &mut bool,
) -> Result<Option<PinModule>, String> {
    use crate::config::record_type;
    match record.rec_type {
        record_type::AXIS_ADC | record_type::AXIS_GENERIC => {
            // F-15 (v4.0.2): handbrake AXIS slot 4 — flags bit0 is
            // button mode (digital button, pull-up rule); any other
            // flag bit is rejected. A button-mode record maps to a
            // Button module so the pull-up rule applies instead of
            // the ADC1 rule; the axis limit count is unaffected
            // (`blob_limit_errors` counts records, not modules).
            if record.slot == HANDBRAKE_SLOT {
                if record.flags & !HANDBRAKE_BUTTON_FLAG != 0 {
                    return Err(format!(
                        "handbrake axis: flags 0x{:02x} invalid (only bit0 button mode allowed)",
                        record.flags
                    ));
                }
                if record.flags & HANDBRAKE_BUTTON_FLAG != 0 {
                    return Ok(if record.pin_a == 0xFF {
                        None
                    } else {
                        Some(PinModule::button(record.pin_a))
                    });
                }
            }
            Ok(if record.pin_a == 0xFF {
                None
            } else {
                Some(PinModule::axis(record.pin_a))
            })
        }
        record_type::AXIS_ADS1115 => {
            *expanders = true;
            // G1: C++ (`rsh_cfg_blob_validate`, I2C-kinds branch) rejects
            // any GPIO pin on an I2C kind — pins must all be 0xFF.
            if record.pin_a != 0xFF || record.pin_b != 0xFF || record.pin_c != 0xFF {
                return Err("axis_ads1115: uses I2C, no GPIO pin".to_string());
            }
            let mut module = PinModule::axis_ads1115(record.params[0]);
            module.addr = 0x48;
            Ok(Some(module))
        }
        record_type::BUTTON => Ok(if record.pin_a == 0xFF {
            None
        } else {
            Some(PinModule::button(record.pin_a))
        }),
        record_type::BUTTON_MATRIX => {
            let rows = if record.params[0] == 0 {
                4
            } else {
                record.params[0]
            };
            let cols = if record.params[1] == 0 {
                4
            } else {
                record.params[1]
            };
            let mut pins = vec![record.pin_a, record.pin_b, record.pin_c];
            pins.extend_from_slice(&record.params[2..]);
            pins.retain(|p| *p != 0xFF);
            Ok(Some(
                PinModule::button_matrix(pins).with_rows_cols(rows, cols),
            ))
        }
        record_type::ENCODER => Ok(match (record.pin_a != 0xFF, record.pin_b != 0xFF) {
            (true, true) => Some(PinModule::encoder(record.pin_a, record.pin_b)),
            // BUG-2: a 1-pin encoder used to be silently dropped (`None`),
            // so 5 of them vanished and the blob only got the misleading
            // "at least one input module required". Keep the one pin as a
            // module instead: `validate()` rejects it ("needs exactly 2
            // pins"), and `validate_blob` counts the record toward the
            // encoder limit below, exactly like C++ `n_enc++`.
            (true, false) => Some(PinModule::base(PinModuleKind::Encoder, vec![record.pin_a])),
            (false, true) => Some(PinModule::base(PinModuleKind::Encoder, vec![record.pin_b])),
            (false, false) => None,
        }),
        record_type::LED_STRIP => Ok(if record.pin_a == 0xFF {
            None
        } else {
            let mut module = PinModule::led(record.pin_a);
            if record.params[0] != 0 {
                module = module.with_count(record.params[0]);
            }
            Some(module)
        }),
        record_type::BATTERY => Ok(if record.pin_a == 0xFF {
            None
        } else {
            Some(PinModule::battery(record.pin_a))
        }),
        record_type::EXPANDER_MCP23017 | record_type::EXPANDER_PCF8574 => {
            *expanders = true;
            // G1: same I2C-kinds rule as ADS1115 — no GPIO pins.
            if record.pin_a != 0xFF || record.pin_b != 0xFF || record.pin_c != 0xFF {
                return Err("expander_button: uses I2C, no GPIO pin".to_string());
            }
            let expander = if record.rec_type == record_type::EXPANDER_MCP23017 {
                "MCP23017"
            } else {
                "PCF8574"
            };
            let mut module = PinModule::expander_button(record.params[0]);
            module.expander = expander.to_string();
            Ok(Some(module))
        }
        record_type::AXIS_AS5600 => {
            *expanders = true;
            // BUG-3: C++ (`rsh_cfg_blob_validate`, AS5600 branch) rejects
            // flags other than bit0 (multiturn), rotation (`params[1]`)
            // above 2, and any GPIO pin set — Rust used to ignore all
            // three. Check in C++ order: flags, rotation, pins.
            if record.flags & !0x01 != 0 {
                return Err(format!(
                    "axis_as5600: flags 0x{:02x} invalid (only bit0 multiturn allowed)",
                    record.flags
                ));
            }
            if record.params[1] > 2 {
                return Err(format!(
                    "axis_as5600: rotation {} out of range (0-2)",
                    record.params[1]
                ));
            }
            if record.pin_a != 0xFF || record.pin_b != 0xFF || record.pin_c != 0xFF {
                return Err("axis_as5600: uses I2C, no GPIO pin".to_string());
            }
            Ok(Some(PinModule::axis_as5600()))
        }
        record_type::SHIFTER => Ok(None),
        record_type::LED_CONFIG => {
            // F-14 (v4.0.2): persistent LED config — no module (the
            // strip pin comes from the type-6 LED_STRIP record), but
            // the fields are validated exactly like C++
            // (`rsh_cfg_blob_validate`, LED_CONFIG branch): slot 0,
            // flags 0, no pins, pattern 0..4, brightness 0..100.
            // The max-1-record count is enforced by
            // `blob_limit_errors`.
            if record.slot != 0 {
                return Err(format!(
                    "LED_CONFIG: slot {} invalid (must be 0)",
                    record.slot
                ));
            }
            if record.flags != 0 {
                return Err(format!(
                    "LED_CONFIG: flags 0x{:02x} invalid (must be 0)",
                    record.flags
                ));
            }
            if record.pin_a != 0xFF || record.pin_b != 0xFF || record.pin_c != 0xFF {
                return Err("LED_CONFIG: uses no GPIO pin".to_string());
            }
            if record.params[0] > 4 {
                return Err(format!(
                    "LED_CONFIG: pattern {} out of range (0-4)",
                    record.params[0]
                ));
            }
            if record.params[4] > 100 {
                return Err(format!(
                    "LED_CONFIG: brightness {} out of range (0-100)",
                    record.params[4]
                ));
            }
            Ok(None)
        }
        other => Err(format!("unknown record type {other}")),
    }
}

/// Validate one SHIFTER record (§3.6): gate mode 0/1, H-pattern needs
/// 6 unique gear pins, H-pattern conflicts with any ENCODER record.
/// Valid gear pins are recorded in `used` under `"SHIFTER"`.
fn shifter_errors(
    record: &crate::config::ConfigRecord,
    has_encoder: bool,
    used: &mut HashMap<u8, String>,
) -> Vec<String> {
    let mut errs = Vec::new();
    let mode = record.params[0];
    // D-4 (v4.0.2): sensitivity `params[4]` is checked before the
    // mode — C++ reports it and skips the rest of the record.
    if record.params[SHIFTER_SENSITIVITY_PARAM] > 100 {
        errs.push(format!(
            "SHIFTER: sensitivity {} out of range (0-100)",
            record.params[SHIFTER_SENSITIVITY_PARAM]
        ));
        return errs;
    }
    if mode > 1 {
        errs.push(format!(
            "SHIFTER: gate mode {mode} invalid (0 Sequential, 1 H-pattern)"
        ));
        return errs;
    }
    if mode == 0 {
        // G2: Sequential — C++ falls through to the generic loop, so
        // pinA/B/C get the class-only check (barred / duplicate /
        // I2C-reserved). Passing pins join `used` so later records see
        // them; the validate_blob post-pass catches later module pins
        // duplicating them (see `seq_pins`).
        for pin in [record.pin_a, record.pin_b, record.pin_c] {
            if pin == 0xFF {
                continue;
            }
            let perr = pin_class_errors(pin, used);
            errs.extend(perr.iter().cloned());
            if perr.is_empty() {
                used.insert(pin, "SHIFTER".to_string());
            }
        }
        return errs;
    }
    if has_encoder {
        errs.push("SHIFTER: H-pattern conflicts with ENCODER records".to_string());
    }
    let gear_pins = [
        record.pin_a,
        record.pin_b,
        record.pin_c,
        record.params[1],
        record.params[2],
        record.params[3],
    ];
    let present: Vec<u8> = gear_pins.into_iter().filter(|p| *p != 0xFF).collect();
    if present.len() != 6 {
        errs.push(format!(
            "SHIFTER: H-pattern needs 6 gear pins, got {}",
            present.len()
        ));
    }
    let mut seen: HashSet<u8> = HashSet::new();
    for pin in &present {
        if !seen.insert(*pin) {
            errs.push(format!("GPIO{pin}: already used by SHIFTER"));
            continue;
        }
        let perr = pin_errors(*pin, PinModuleKind::Button, false, used);
        errs.extend(perr.iter().cloned());
        if perr.is_empty() {
            used.insert(*pin, "SHIFTER".to_string());
        }
    }
    errs
}

/// Record-type name for error messages.
fn record_type_name(rec_type: u8) -> &'static str {
    match rec_type {
        record_type::AXIS_ADC => "AXIS_ADC",
        record_type::AXIS_ADS1115 => "AXIS_ADS1115",
        record_type::BUTTON => "BUTTON",
        record_type::BUTTON_MATRIX => "BUTTON_MATRIX",
        record_type::ENCODER => "ENCODER",
        record_type::LED_STRIP => "LED_STRIP",
        record_type::BATTERY => "BATTERY",
        record_type::EXPANDER_MCP23017 => "EXPANDER_MCP23017",
        record_type::EXPANDER_PCF8574 => "EXPANDER_PCF8574",
        record_type::AXIS_GENERIC => "AXIS_GENERIC",
        record_type::SHIFTER => "SHIFTER",
        record_type::AXIS_AS5600 => "AXIS_AS5600",
        record_type::LED_CONFIG => "LED_CONFIG",
        _ => "UNKNOWN",
    }
}

/// Pins of a record that the module mapping drops but the C++ generic
/// loop still checks with `RSH_PIN_RULE_CLASS_ONLY` (barred / duplicate
/// / I2C-reserved, no electrical rule) — BUG-1/BUG-2: pinB/pinC of the
/// single-pin kinds (`AXIS_ADC` / `AXIS_GENERIC` / `BUTTON` / `BATTERY`
/// / `LED_STRIP`) and pinC of `ENCODER` (`BUTTON_MATRIX` pins are all
/// in its module, with the C++ pull-up rule). Returns `(pin, role)`
/// pairs; `0xFF` pins are absent and never listed.
fn class_only_pins(record: &crate::config::ConfigRecord) -> Vec<(u8, &'static str)> {
    let mut out = Vec::new();
    let is_single_pin = matches!(
        record.rec_type,
        record_type::AXIS_ADC
            | record_type::AXIS_GENERIC
            | record_type::BUTTON
            | record_type::BATTERY
            | record_type::LED_STRIP
    );
    if is_single_pin && record.pin_b != 0xFF {
        out.push((record.pin_b, "pinB"));
    }
    if (is_single_pin || record.rec_type == record_type::ENCODER) && record.pin_c != 0xFF {
        out.push((record.pin_c, "pinC"));
    }
    out
}

/// C++ record-level limit errors for a blob (BUG-2/BUG-6, G3). Replaces
/// [`validate`]'s encoder/button/axes/battery limit errors — the firmware
/// counts RECORDS, not modules: `n_enc++` per `ENCODER` record (a 1-pin
/// encoder still counts), `n_btn += 16` per `BUTTON_MATRIX` record
/// (fixed 16, not rows x cols), `n_axes++` per axis record (even a
/// pinless one — it maps to no module), `n_bat++` per `BATTERY` record,
/// and 65–128 buttons have no wire slot. (LED strips are the exception:
/// C++ counts only pinned LED records, exactly like the module path, so
/// no record count is needed for them.) Expander records count (BUG-6c —
/// kept strict by coordinator decision, matching the Python reference).
fn blob_limit_errors(blob: &ConfigBlob) -> Vec<String> {
    let mut errs = Vec::new();
    let n_axes = blob
        .records
        .iter()
        .filter(|r| {
            matches!(
                r.rec_type,
                record_type::AXIS_ADC
                    | record_type::AXIS_ADS1115
                    | record_type::AXIS_GENERIC
                    | record_type::AXIS_AS5600
            )
        })
        .count();
    if n_axes > 8 {
        errs.push(format!("too many axes ({n_axes} > 8)"));
    }
    let n_bat = blob
        .records
        .iter()
        .filter(|r| r.rec_type == record_type::BATTERY)
        .count();
    if n_bat > 1 {
        errs.push(format!("too many battery modules ({n_bat} > 1)"));
    }
    // F-14 (v4.0.2): at most one persistent LED config record.
    let n_ledcfg = blob
        .records
        .iter()
        .filter(|r| r.rec_type == record_type::LED_CONFIG)
        .count();
    if n_ledcfg > 1 {
        errs.push(format!("too many LED config records ({n_ledcfg} > 1)"));
    }
    let n_enc = blob
        .records
        .iter()
        .filter(|r| r.rec_type == record_type::ENCODER)
        .count();
    if n_enc > 4 {
        errs.push(format!("too many encoders ({n_enc} > 4)"));
    }
    let n_btn: u32 = blob
        .records
        .iter()
        .map(|r| match r.rec_type {
            record_type::BUTTON
            | record_type::EXPANDER_MCP23017
            | record_type::EXPANDER_PCF8574 => 1,
            record_type::BUTTON_MATRIX => 16,
            _ => 0,
        })
        .sum();
    if n_btn > 128 {
        errs.push(format!("too many buttons ({n_btn} > 128)"));
    } else if n_btn > 64 {
        // BUG-6b: C++ COMMIT returns UNSUPPORTED (N1) — there is no wire
        // slot for buttons 65-128 (65-72 are encoder virtual buttons,
        // not config records).
        errs.push(format!(
            "too many buttons ({n_btn} > 64): buttons 65-128 have no wire slot (firmware UNSUPPORTED)"
        ));
    }
    errs
}

/// `(pin, record index)` pairs for one SHIFTER mode's pins.
type ShifterPins = Vec<(u8, usize)>;

/// SHIFTER pins of a blob with their record index, for the
/// later-module cross-check: H-pattern gear pins (mode 1: pinA/B/C +
/// params[1..3]) and sequential pins (mode 0: pinA/B/C — G2). `0xFF`
/// pins are absent and never listed.
fn shifter_pins(blob: &ConfigBlob) -> (ShifterPins, ShifterPins) {
    let gear: ShifterPins = blob
        .records
        .iter()
        .enumerate()
        .filter(|(_, r)| r.rec_type == record_type::SHIFTER && r.params[0] == 1)
        .flat_map(|(idx, r)| {
            [
                r.pin_a,
                r.pin_b,
                r.pin_c,
                r.params[1],
                r.params[2],
                r.params[3],
            ]
            .into_iter()
            .map(move |p| (p, idx))
        })
        .filter(|(p, _)| *p != 0xFF)
        .collect();
    let seq: ShifterPins = blob
        .records
        .iter()
        .enumerate()
        .filter(|(_, r)| r.rec_type == record_type::SHIFTER && r.params[0] == 0)
        .flat_map(|(idx, r)| {
            [r.pin_a, r.pin_b, r.pin_c]
                .into_iter()
                .map(move |p| (p, idx))
        })
        .filter(|(p, _)| *p != 0xFF)
        .collect();
    (gear, seq)
}

/// Shifter sensitivity (v4.0.2 D-4) of a blob: the first SHIFTER
/// record's `params[4]` (0..100), or the default 50 when the blob
/// has no SHIFTER record. (A record with `params[4] == 0` means 0,
/// not "default" — the firmware returns the field as-is.)
#[must_use]
pub fn shifter_sensitivity(blob: &ConfigBlob) -> u8 {
    blob.records
        .iter()
        .find(|r| r.rec_type == record_type::SHIFTER)
        .map_or(SHIFTER_SENSITIVITY_DEFAULT, |r| {
            r.params[SHIFTER_SENSITIVITY_PARAM]
        })
}

/// Shifter sensitivity (0..100) → gear debounce / H-pattern
/// stability time in ms, mirroring firmware
/// `rsh_shifter_debounce_ms`: 0 → 20 ms, 50 → 13 ms (default),
/// 100 → 5 ms. Out-of-range input is clamped to 100 first.
#[must_use]
pub fn shifter_debounce_ms(sensitivity: u8) -> u8 {
    let sens = u16::from(sensitivity.min(100));
    u8::try_from(20 - sens * 15 / 100).unwrap_or(0)
}

/// Validate a config blob's pin assignments against the board rules.
///
/// Record → module mapping (§3.6): `AXIS_ADC` → axis, `AXIS_ADS1115` →
/// ADS1115 axis (channel `params[0]`, address `0x48`), `BUTTON` →
/// button, `BUTTON_MATRIX` → matrix (`params[0]` rows, `params[1]`
/// cols, pins `pinA`/`pinB`/`pinC` + `params[2..]`), `ENCODER` →
/// encoder, `LED_STRIP` → LED (count `params[0]` when nonzero),
/// `BATTERY` → battery, expander records → expander buttons
/// (`params[0]` expander pin), `AXIS_AS5600` → AS5600 axis.
/// `SHIFTER` records are validated separately (gate mode, gear pins,
/// H-pattern/encoder conflict; sensitivity `params[4]` 0..100).
/// `LED_CONFIG` records carry no pin and map to no module; their
/// fields are validated (slot 0, flags 0, no pins, pattern 0..4,
/// brightness 0..100, at most one record). A handbrake `AXIS_ADC` /
/// `AXIS_GENERIC` record (slot 4) with flags bit0 set maps to a
/// Button module (pull-up rule, not the ADC1 rule); any other flag
/// bit on a slot-4 record is rejected. Records with a `0xFF` pin are
/// treated as unassigned and skipped. `expanders` is auto-detected
/// from the records (any I²C kind sets it).
///
/// Blob-level counts follow the C++ firmware validator
/// (`rsh_cfg_blob_validate`), not the module-level [`validate`]:
/// every `ENCODER` record counts toward the encoder limit (even a
/// 1-pin one), every `BUTTON_MATRIX` record counts 16 buttons, every
/// axis record counts toward the axes limit (even a pinless one),
/// every `BATTERY` record counts toward the battery limit, and
/// 65–128 buttons are rejected — the firmware has no wire slot for
/// them. [`validate`]'s own encoder/button/axes/battery limit errors
/// are replaced by these record counts so a blob is never rejected
/// twice for the same limit.
#[must_use]
// Validator ek hi pass mein har record kind ka guard lagata hai
// (expander / matrix / slot-range) — is liye 100-line pedantic
// limit se upar hai; split karne se checks bikhar jayenge.
#[allow(clippy::too_many_lines)]
pub fn validate_blob(blob: &ConfigBlob) -> Vec<String> {
    let mut errs: Vec<String> = Vec::new();
    let mut modules: Vec<PinModule> = Vec::new();
    // C++ pre-scan (R-L9): any I²C kind (ADS1115 / expander / AS5600)
    // reserves GPIO21/22 before a single pin is checked, so a direct
    // pin 21/22 — including a SHIFTER gear pin (BUG-4) or a dropped
    // pinB/pinC (BUG-1) — is rejected wherever the I²C record sits.
    let has_i2c = blob.records.iter().any(|r| {
        matches!(
            r.rec_type,
            record_type::AXIS_ADS1115
                | record_type::EXPANDER_MCP23017
                | record_type::EXPANDER_PCF8574
                | record_type::AXIS_AS5600
        )
    });
    let mut expanders = has_i2c;
    // Blob-level `used` map — the C++ validator's single `used_pin[]`
    // array: status LED + I²C reservation, every module pin, every
    // SHIFTER gear pin, and every class-only pin checked. `validate()`
    // below keeps its own map; this one exists so the dropped pins
    // (BUG-1/BUG-2) and gear pins (BUG-4) take part in duplicate
    // detection against pins of records checked earlier — and, via the
    // post-passes, of records checked later.
    let mut used: HashMap<u8, String> = HashMap::new();
    let (i2c_sda, i2c_scl, status_led) = reserved_pins();
    used.insert(status_led, "status LED".to_string());
    if has_i2c {
        used.insert(i2c_sda, "i2c".to_string());
        used.insert(i2c_scl, "i2c".to_string());
    }
    // Order-independent pre-scan: H-pattern conflicts with ANY encoder
    // record in the blob, wherever it sits (§3.6).
    let has_encoder = blob
        .records
        .iter()
        .any(|r| r.rec_type == record_type::ENCODER);
    // SHIFTER pins (H-pattern gear pins + sequential pins) with their
    // record index, for cross-checks against every other module pin.
    let (gear_pins, seq_pins) = shifter_pins(blob);
    // Class-only pins that passed their check, with record index and
    // owner, for the cross-check against later module pins.
    let mut extra_pins: Vec<(u8, usize, String)> = Vec::new();
    // Every module pin with its record index, for the same cross-check.
    let mut module_pins: Vec<(u8, usize)> = Vec::new();
    // Round 3 (v4.0.4): expander sampling is not implemented — one
    // error per blob, not per record (see the loop below).
    let mut saw_expander = false;
    // P1-3: same shape for button matrices — no firmware consumer,
    // one error per blob.
    let mut saw_matrix = false;

    for (idx, record) in blob.records.iter().enumerate() {
        if record.rec_type == record_type::SHIFTER {
            errs.extend(shifter_errors(record, has_encoder, &mut used));
            continue;
        }
        if matches!(
            record.rec_type,
            record_type::EXPANDER_MCP23017 | record_type::EXPANDER_PCF8574
        ) {
            // The board rejects these records at COMMIT, so reject
            // here first with a plain message instead of a silently
            // dead button. Skipped entirely — no module is built, so
            // no range/uniqueness checks run on dead records. Axes
            // (ADS1115/AS5600) are unaffected — only kinds 8/9.
            if !saw_expander {
                errs.push(
                    "expander_button: MCP23017/PCF8574 expander buttons are not supported yet — expander sampling is not implemented, so these buttons would never work. Remove the expander buttons from the configuration and try again."
                        .to_string(),
                );
                saw_expander = true;
            }
            continue;
        }
        if record.rec_type == record_type::BUTTON_MATRIX {
            // P1-3: the board rejects matrix records at COMMIT — the
            // firmware has no matrix scanner, so these buttons would
            // never work. Reject here first with a plain message
            // (same shape as the expander reject above). Skipped
            // entirely — no module is built, so no range/uniqueness
            // checks run on a dead record.
            if !saw_matrix {
                errs.push(
                    "button_matrix: button matrices are not supported yet — the firmware has no matrix scanner, so these buttons would never work. Remove the button matrix from the configuration and try again."
                        .to_string(),
                );
                saw_matrix = true;
            }
            continue;
        }
        match blob_record_module(record, &mut expanders) {
            Ok(Some(module)) => {
                for &pin in &module.pins {
                    used.entry(pin)
                        .or_insert_with(|| module.kind.as_str().to_string());
                    module_pins.push((pin, idx));
                }
                modules.push(module);
            }
            Ok(None) => {}
            Err(e) => {
                errs.push(e);
                continue;
            }
        }
        // Class-only pins (BUG-1/BUG-2). Module pins were just recorded
        // in `used`, so a pinB/pinC duplicating this record's own pinA
        // — or any earlier record's pin — is caught here; a duplicate
        // against a LATER record's pin is caught by the post-pass below.
        for (pin, role) in class_only_pins(record) {
            let owner = format!("{} {role}", record_type_name(record.rec_type));
            let perr = pin_class_errors(pin, &used);
            errs.extend(perr.iter().cloned());
            if perr.is_empty() {
                used.insert(pin, owner.clone());
                extra_pins.push((pin, idx, owner));
            }
        }
    }
    // Gear pins must not collide with any other module pin. Only the
    // gear-first order needs this pass: when the SHIFTER record comes
    // later, `shifter_errors` already saw the module pin in `used`.
    // Same for sequential SHIFTER pins (G2).
    for (pin, j) in &module_pins {
        if gear_pins
            .iter()
            .chain(seq_pins.iter())
            .any(|(gp, i)| gp == pin && i < j)
        {
            errs.push(format!("GPIO{pin}: already used by SHIFTER"));
        }
    }
    // Same ordering reason for class-only pins vs later module pins.
    for (pin, j) in &module_pins {
        if let Some((_, _, owner)) = extra_pins.iter().find(|(ep, i, _)| ep == pin && i < j) {
            errs.push(format!("GPIO{pin}: already used by {owner}"));
        }
    }
    // Module validation (pin classes, electrical rules, axes/LED/
    // battery limits), with its encoder/button/axes/battery limit errors
    // replaced by the C++ record counts above.
    let mut validated = validate(&modules, expanders);
    validated.retain(|e| {
        !e.starts_with("too many encoders")
            && !e.starts_with("too many buttons")
            && !e.starts_with("too many axes")
            && !e.starts_with("too many battery")
    });
    errs.extend(validated);
    errs.extend(blob_limit_errors(blob));
    errs
}

#[cfg(test)]
mod tests {
    use super::{PinModule, PinModuleKind, free_pins, validate, validate_blob};
    use crate::config::{ConfigBlob, ConfigRecord, record_type};

    /// Build a blob from exact 16-byte record hex strings (the FW-A2
    /// regression bytes, verbatim — spaces allowed).
    fn blob_from_hex(records_hex: &[&str]) -> ConfigBlob {
        let records = records_hex
            .iter()
            .map(|hex| {
                let compact: String = hex.chars().filter(|c| !c.is_whitespace()).collect();
                let bytes: Vec<u8> = compact
                    .as_bytes()
                    .chunks(2)
                    .map(|pair| {
                        let s = std::str::from_utf8(pair).expect("test hex is ASCII");
                        u8::from_str_radix(s, 16).expect("test hex is valid")
                    })
                    .collect();
                ConfigRecord::decode(&bytes).expect("test record is 16 bytes")
            })
            .collect();
        ConfigBlob::new(1, records)
    }

    fn full_rig() -> Vec<PinModule> {
        vec![
            PinModule::axis(32),
            PinModule::axis(33),
            PinModule::axis(34),
            PinModule::axis(35),
            PinModule::axis(36),
            PinModule::button(4),
            PinModule::button(13),
            PinModule::button(14),
            PinModule::button(16),
            PinModule::button(17),
            PinModule::button(18),
            PinModule::button(19),
            PinModule::encoder(23, 25),
            PinModule::encoder(26, 27),
            PinModule::battery(39),
        ]
    }

    #[test]
    fn default_map_valid() {
        assert_eq!(
            validate(
                &[
                    PinModule::axis(34),
                    PinModule::axis(35),
                    PinModule::axis(36),
                    PinModule::button(25),
                    PinModule::button(26),
                    PinModule::button(27),
                    PinModule::button(33),
                ],
                false
            ),
            Vec::<String>::new()
        );
    }

    #[test]
    fn analog_on_adc2_rejected() {
        assert!(
            validate(&[PinModule::axis(25)], false)
                .iter()
                .any(|e| e.contains("ADC1"))
        );
    }

    #[test]
    fn flash_pin_rejected() {
        assert!(
            validate(&[PinModule::button(6)], false)
                .iter()
                .any(|e| e.contains("forbidden"))
        );
    }

    #[test]
    fn duplicate_pin_rejected() {
        assert!(
            validate(
                &[
                    PinModule::axis(34),
                    PinModule::button(34).with_ext_pullup(true)
                ],
                false
            )
            .iter()
            .any(|e| e.contains("already used"))
        );
    }

    #[test]
    fn full_rig_valid_and_free_pins_exact() {
        assert_eq!(validate(&full_rig(), false), Vec::<String>::new());
        assert_eq!(free_pins(&full_rig(), false), vec![21, 22]);
        assert_eq!(free_pins(&full_rig(), true), Vec::<u8>::new());
        let mut rig = full_rig();
        rig.push(PinModule::led(21));
        assert_eq!(validate(&rig, false), Vec::<String>::new());
        assert_eq!(free_pins(&rig, false), vec![22]);
    }

    #[test]
    fn limits_enforced() {
        // 9 axes rejected, 8 valid.
        let mut nine = vec![
            PinModule::axis(32),
            PinModule::axis(33),
            PinModule::axis(34),
            PinModule::axis(35),
            PinModule::axis(36),
            PinModule::axis(39),
            PinModule::axis_ads1115(0),
            PinModule::axis_ads1115(1),
            PinModule::axis_ads1115(2),
        ];
        assert!(
            validate(&nine, true)
                .iter()
                .any(|e| e.contains("too many axes"))
        );
        nine.pop();
        assert_eq!(validate(&nine, true), Vec::<String>::new());
        // 5 encoders rejected, 4 valid.
        let five = vec![
            PinModule::encoder(4, 13),
            PinModule::encoder(14, 16),
            PinModule::encoder(17, 18),
            PinModule::encoder(19, 23),
            PinModule::encoder(25, 26),
        ];
        assert!(
            validate(&five, false)
                .iter()
                .any(|e| e.contains("too many encoders"))
        );
        assert_eq!(validate(&five[..4], false), Vec::<String>::new());
        // 129 expander buttons rejected, 128 valid.
        let many: Vec<PinModule> = (0..129).map(|_| PinModule::expander_button(0)).collect();
        assert!(
            validate(&many, true)
                .iter()
                .any(|e| e.contains("too many buttons"))
        );
        let ok128: Vec<PinModule> = (0..128)
            .map(|i| {
                PinModule::expander_button(u8::try_from(i % 16).unwrap_or(0))
                    .with_addr(0x20 + u8::try_from(i / 16).unwrap_or(0))
            })
            .collect();
        assert_eq!(validate(&ok128, true), Vec::<String>::new());
    }

    #[test]
    fn battery_not_counted_as_axis() {
        let modules = vec![
            PinModule::axis(32),
            PinModule::axis(33),
            PinModule::axis(34),
            PinModule::axis(35),
            PinModule::axis(36),
            PinModule::axis_ads1115(0),
            PinModule::axis_ads1115(1),
            PinModule::axis_ads1115(2),
            PinModule::battery(39),
        ];
        assert_eq!(validate(&modules, true), Vec::<String>::new());
    }

    #[test]
    fn embedded_presets_are_valid() {
        let board_json: serde_json::Value =
            serde_json::from_str(super::BOARD_JSON).expect("board JSON parses");
        for (name, preset) in board_json["presets"].as_object().expect("presets object") {
            let expanders = preset["expanders"].as_bool().unwrap_or(false);
            let mut modules = Vec::new();
            for m in preset["modules"].as_array().expect("modules array") {
                let kind = PinModuleKind::from_name(m["kind"].as_str().unwrap_or(""))
                    .unwrap_or_else(|| panic!("preset {name}: unknown kind"));
                let pins: Vec<u8> = m["pins"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_u64().and_then(|n| u8::try_from(n).ok()))
                            .collect()
                    })
                    .unwrap_or_default();
                let mut module = match kind {
                    PinModuleKind::Encoder => PinModule::encoder(pins[0], pins[1]),
                    PinModuleKind::ExpanderButton => {
                        let mut module = PinModule::expander_button(
                            u8::try_from(m["expander_pin"].as_u64().unwrap_or(0)).unwrap_or(0),
                        );
                        module.expander = m["expander"].as_str().unwrap_or("MCP23017").to_string();
                        module.addr =
                            u8::try_from(m["addr"].as_u64().unwrap_or(0x20)).unwrap_or(0x20);
                        module
                    }
                    _ => PinModule {
                        kind,
                        pins,
                        ext_pullup: false,
                        channel: 0,
                        addr: 0x20,
                        expander: "MCP23017".to_string(),
                        expander_pin: None,
                        rows: 4,
                        cols: 4,
                        count: None,
                    },
                };
                let _ = &mut module;
                modules.push(module);
            }
            assert_eq!(
                validate(&modules, expanders),
                Vec::<String>::new(),
                "preset {name} must be valid"
            );
            let expected: Vec<u8> = preset["expected_free_pins"]
                .as_array()
                .expect("expected_free_pins")
                .iter()
                .filter_map(|v| v.as_u64().and_then(|n| u8::try_from(n).ok()))
                .collect();
            assert_eq!(
                free_pins(&modules, expanders),
                expected,
                "preset {name} free pins"
            );
        }
    }

    // --- FW-A2 §6 regressions (BUG-1–BUG-4, BUG-6a, BUG-6b) ---
    // Each test uses the exact recorded regression bytes
    // and asserts the error Rust previously missed (board COMMIT
    // rejects these with PIN_CONFLICT / UNSUPPORTED).

    #[test]
    fn bug1_axis_adc_pinb_flash_rejected() {
        // AXIS_ADC pinA 34, pinB 6 (flash) — C++ err 2.
        let errs = validate_blob(&blob_from_hex(&[
            "01 00 00 22 06 ff 00 00 00 00 00 00 00 00 00 00",
        ]));
        assert!(
            errs.iter()
                .any(|e| e.contains("GPIO6") && e.contains("forbidden")),
            "expected GPIO6 forbidden, got {errs:?}"
        );
    }

    #[test]
    fn bug1_button_pinb_flash_rejected() {
        // BUTTON pinA 27, pinB 6 (flash) — C++ err 2.
        let errs = validate_blob(&blob_from_hex(&[
            "03 00 00 1b 06 ff 00 00 00 00 00 00 00 00 00 00",
        ]));
        assert!(
            errs.iter()
                .any(|e| e.contains("GPIO6") && e.contains("forbidden")),
            "expected GPIO6 forbidden, got {errs:?}"
        );
    }

    #[test]
    fn bug1_axis_adc_pinc_flash_rejected() {
        // AXIS_ADC pinA 34, pinC 6 (flash) — same class-only check.
        let errs = validate_blob(&blob_from_hex(&[
            "01 00 00 22 ff 06 00 00 00 00 00 00 00 00 00 00",
        ]));
        assert!(
            errs.iter()
                .any(|e| e.contains("GPIO6") && e.contains("forbidden")),
            "expected GPIO6 forbidden, got {errs:?}"
        );
    }

    #[test]
    fn bug1_pinb_duplicate_of_pina_rejected() {
        // BUTTON pinB 34 duplicates the AXIS_ADC pinA 34 — C++ dup err 1.
        let errs = validate_blob(&blob_from_hex(&[
            "01 00 00 22 ff ff 00 00 00 00 00 00 00 00 00 00",
            "03 00 00 1b 22 ff 00 00 00 00 00 00 00 00 00 00",
        ]));
        assert!(
            errs.iter()
                .any(|e| e.contains("GPIO34") && e.contains("already used")),
            "expected GPIO34 already used, got {errs:?}"
        );
    }

    #[test]
    fn bug1_pinb_i2c_reserved_rejected() {
        // AXIS_ADC pinB 21 with an ADS1115 record anywhere in the blob
        // (here after it — the I2C pre-scan is order-independent).
        let errs = validate_blob(&blob_from_hex(&[
            "01 00 00 22 15 ff 00 00 00 00 00 00 00 00 00 00",
            "02 00 00 ff ff ff 00 00 00 00 00 00 00 00 00 00",
        ]));
        assert!(
            errs.iter()
                .any(|e| e.contains("GPIO21") && e.contains("already used by i2c")),
            "expected GPIO21 already used by i2c, got {errs:?}"
        );
    }

    #[test]
    fn bug1_led_strip_pinb_flash_rejected() {
        // LED_STRIP pinA 21, pinB 6 (flash) — the C++ generic loop
        // class-only-checks LED pinB/pinC too.
        let errs = validate_blob(&blob_from_hex(&[
            "06 00 00 15 06 ff 00 00 00 00 00 00 00 00 00 00",
        ]));
        assert!(
            errs.iter()
                .any(|e| e.contains("GPIO6") && e.contains("forbidden")),
            "expected GPIO6 forbidden, got {errs:?}"
        );
    }

    #[test]
    fn bug2_encoder_pinc_flash_rejected() {
        // ENCODER 23/19, pinC 6 (flash) — C++ err 2.
        let errs = validate_blob(&blob_from_hex(&[
            "05 00 00 17 13 06 00 00 00 00 00 00 00 00 00 00",
        ]));
        assert!(
            errs.iter()
                .any(|e| e.contains("GPIO6") && e.contains("forbidden")),
            "expected GPIO6 forbidden, got {errs:?}"
        );
    }

    #[test]
    fn bug2_single_one_pin_encoder_errors() {
        // One 1-pin encoder errors (not silently dropped): the module
        // keeps its single pin, so validate() rejects it.
        let errs = validate_blob(&blob_from_hex(&[
            "05 00 00 17 ff ff 00 00 00 00 00 00 00 00 00 00",
        ]));
        assert!(
            errs.iter().any(|e| e.contains("needs exactly 2 pins")),
            "expected encoder 2-pin error, got {errs:?}"
        );
        assert!(
            !errs
                .iter()
                .any(|e| e.contains("at least one input module required")),
            "1-pin encoder must not vanish into the empty-blob error: {errs:?}"
        );
    }

    #[test]
    fn bug2_five_one_pin_encoders_hit_limit() {
        // 5x 1-pin encoders — C++ n_enc = 5 > 4, err 2.
        let records: Vec<ConfigRecord> = [23, 19, 16, 17, 25]
            .iter()
            .enumerate()
            .map(|(i, &pin)| {
                ConfigRecord::new(
                    record_type::ENCODER,
                    u8::try_from(i).unwrap_or(0),
                    pin,
                    0xFF,
                    0xFF,
                )
            })
            .collect();
        let errs = validate_blob(&ConfigBlob::new(1, records));
        assert!(
            errs.iter().any(|e| e.contains("too many encoders (5 > 4)")),
            "expected encoder-limit error, got {errs:?}"
        );
        assert!(
            !errs
                .iter()
                .any(|e| e.contains("at least one input module required")),
            "must not report the misleading empty-blob error: {errs:?}"
        );
    }

    #[test]
    fn bug3_as5600_gpio_pin_rejected() {
        // AS5600 pinA 32 — C++ err 2 (pins must be 0xFF).
        let errs = validate_blob(&blob_from_hex(&[
            "0c 00 00 20 ff ff 00 00 00 00 00 00 00 00 00 00",
        ]));
        assert!(
            errs.iter()
                .any(|e| e.contains("axis_as5600") && e.contains("no GPIO pin")),
            "expected AS5600 no-GPIO error, got {errs:?}"
        );
    }

    #[test]
    fn bug3_as5600_flags_rejected() {
        // AS5600 flags 0x02 — C++ err 2 (only bit0 multiturn allowed).
        let errs = validate_blob(&blob_from_hex(&[
            "0c 00 02 ff ff ff 00 00 00 00 00 00 00 00 00 00",
        ]));
        assert!(
            errs.iter()
                .any(|e| e.contains("axis_as5600") && e.contains("flags 0x02")),
            "expected AS5600 flags error, got {errs:?}"
        );
    }

    #[test]
    fn bug3_as5600_rotation_rejected() {
        // AS5600 rotation (params[1]) 3 — C++ err 2 (0-2 only).
        let errs = validate_blob(&blob_from_hex(&[
            "0c 00 00 ff ff ff 00 03 00 00 00 00 00 00 00 00",
        ]));
        assert!(
            errs.iter()
                .any(|e| e.contains("axis_as5600") && e.contains("rotation 3")),
            "expected AS5600 rotation error, got {errs:?}"
        );
    }

    #[test]
    fn bug4_shifter_gear_pin_21_with_i2c_rejected() {
        // H-pattern SHIFTER (gear pins 21,13,14,16,17,18) + ADS1115 —
        // C++ err 1 (gear pin 21/22 with I2C present).
        let errs = validate_blob(&blob_from_hex(&[
            "0b 00 00 15 0d 0e 01 10 11 12 00 00 00 00 00 00",
            "02 00 00 ff ff ff 00 00 00 00 00 00 00 00 00 00",
        ]));
        assert!(
            errs.iter()
                .any(|e| e.contains("GPIO21") && e.contains("already used by i2c")),
            "expected GPIO21 already used by i2c, got {errs:?}"
        );
    }

    #[test]
    fn bug6a_nine_small_matrices_hit_button_limit() {
        // 9x 2x3 matrices: C++ counts 16 each = 144 > 128 (reject);
        // Rust used to count rows x cols = 54 (accept).
        let pool = [4, 13, 14, 16, 17, 18, 19, 23, 25, 26, 27];
        let records: Vec<ConfigRecord> = (0..9)
            .map(|i| {
                let mut rec = ConfigRecord::new(
                    record_type::BUTTON_MATRIX,
                    u8::try_from(i).unwrap_or(0),
                    pool[(i * 5) % pool.len()],
                    pool[(i * 5 + 1) % pool.len()],
                    pool[(i * 5 + 2) % pool.len()],
                );
                rec.params[0] = 2;
                rec.params[1] = 3;
                rec.params[2] = pool[(i * 5 + 3) % pool.len()];
                rec.params[3] = pool[(i * 5 + 4) % pool.len()];
                rec
            })
            .collect();
        let errs = validate_blob(&ConfigBlob::new(1, records));
        assert!(
            errs.iter()
                .any(|e| e.contains("too many buttons (144 > 128)")),
            "expected 144-button limit error, got {errs:?}"
        );
    }

    #[test]
    fn bug6b_65_buttons_no_wire_slot() {
        // 65 buttons: C++ COMMIT returns UNSUPPORTED (N1) — no wire slot.
        let records: Vec<ConfigRecord> = (0..65)
            .map(|i| {
                let mut rec = ConfigRecord::new(
                    record_type::EXPANDER_MCP23017,
                    u8::try_from(i).unwrap_or(0),
                    0xFF,
                    0xFF,
                    0xFF,
                );
                rec.params[0] = u8::try_from(i % 16).unwrap_or(0);
                rec
            })
            .collect();
        let errs = validate_blob(&ConfigBlob::new(1, records));
        assert!(
            errs.iter().any(|e| e.contains("too many buttons (65 > 64)")
                && e.contains("wire slot")
                && e.contains("65-128")),
            "expected 65-128 wire-slot error, got {errs:?}"
        );
        assert!(
            !errs.iter().any(|e| e.contains("> 128")),
            "65 buttons must not report the >128 error: {errs:?}"
        );
    }

    #[test]
    fn bug6b_64_buttons_wire_slot_ok() {
        // 64 buttons is the wire-slot boundary: no wire-slot error.
        let records: Vec<ConfigRecord> = (0..64)
            .map(|i| {
                let mut rec = ConfigRecord::new(
                    record_type::EXPANDER_MCP23017,
                    u8::try_from(i).unwrap_or(0),
                    0xFF,
                    0xFF,
                    0xFF,
                );
                rec.params[0] = u8::try_from(i % 16).unwrap_or(0);
                rec
            })
            .collect();
        let errs = validate_blob(&ConfigBlob::new(1, records));
        assert!(
            !errs.iter().any(|e| e.contains("wire slot")),
            "64 buttons must not report a wire-slot error: {errs:?}"
        );
    }

    #[test]
    fn expander_records_rejected_at_validate_blob() {
        // Round 3 (v4.0.4): expander sampling is not implemented — a
        // config with expander records must fail with a plain message
        // instead of COMMIT OK + silently dead buttons. Axes (ADS1115 /
        // AS5600) are unaffected — only kinds 8/9 are rejected.
        for rec_type in [
            record_type::EXPANDER_MCP23017,
            record_type::EXPANDER_PCF8574,
        ] {
            let rec = ConfigRecord::new(rec_type, 0, 0xFF, 0xFF, 0xFF);
            let errs = validate_blob(&ConfigBlob::new(1, vec![rec]));
            assert!(
                errs.iter().any(|e| e.contains("not supported yet")),
                "expander type {rec_type} must be rejected, got {errs:?}"
            );
        }
        // Expander + valid axis: still rejected (guard fires regardless
        // of the other records).
        let records = vec![
            ConfigRecord::new(record_type::AXIS_ADC, 0, 34, 0xFF, 0xFF),
            ConfigRecord::new(record_type::EXPANDER_MCP23017, 0, 0xFF, 0xFF, 0xFF),
        ];
        let errs = validate_blob(&ConfigBlob::new(1, records));
        assert!(
            errs.iter().any(|e| e.contains("not supported yet")),
            "expander + axis must be rejected, got {errs:?}"
        );
        // Many expander records: exactly one expander error, not one
        // per record.
        let many: Vec<ConfigRecord> = (0..16)
            .map(|i| {
                ConfigRecord::new(
                    record_type::EXPANDER_MCP23017,
                    u8::try_from(i).unwrap_or(0),
                    0xFF,
                    0xFF,
                    0xFF,
                )
            })
            .collect();
        let errs = validate_blob(&ConfigBlob::new(1, many));
        assert_eq!(
            errs.iter()
                .filter(|e| e.contains("not supported yet"))
                .count(),
            1,
            "one expander error per blob, got {errs:?}"
        );
        // I2C axes only: no expander error (FW-01 sampling is live).
        let mut ads = ConfigRecord::new(record_type::AXIS_ADS1115, 0, 0xFF, 0xFF, 0xFF);
        ads.params[0] = 0;
        let as5600 = ConfigRecord::new(record_type::AXIS_AS5600, 1, 0xFF, 0xFF, 0xFF);
        let errs = validate_blob(&ConfigBlob::new(1, vec![ads, as5600]));
        assert!(
            !errs.iter().any(|e| e.contains("not supported yet")),
            "I2C axes must not trip the expander guard, got {errs:?}"
        );
    }

    #[test]
    fn button_matrix_records_rejected_at_validate_blob() {
        // P1-3 (update4): BUTTON_MATRIX has no firmware consumer —
        // like expanders, COMMIT must fail with a plain message
        // instead of accepting a silently dead record.
        let rec = ConfigRecord::new(record_type::BUTTON_MATRIX, 0, 4, 13, 14);
        let errs = validate_blob(&ConfigBlob::new(1, vec![rec]));
        assert!(
            errs.iter()
                .any(|e| e.contains("button_matrix") && e.contains("not supported")),
            "button_matrix must be rejected, got {errs:?}"
        );
        // Matrix + valid axis: still rejected (guard fires regardless
        // of the other records).
        let records = vec![
            ConfigRecord::new(record_type::AXIS_ADC, 0, 34, 0xFF, 0xFF),
            ConfigRecord::new(record_type::BUTTON_MATRIX, 0, 4, 13, 14),
        ];
        let errs = validate_blob(&ConfigBlob::new(1, records));
        assert!(
            errs.iter()
                .any(|e| e.contains("button_matrix") && e.contains("not supported")),
            "matrix + axis must be rejected, got {errs:?}"
        );
    }

    #[test]
    fn factory_blob_still_valid() {
        // Guard: the fixes must not reject the shipped factory blob.
        assert_eq!(
            validate_blob(&crate::config::factory_default_blob()),
            Vec::<String>::new()
        );
    }

    // G1 — I2C kinds (ADS1115 / expander) must have no GPIO pins (C++
    // `rsh_cfg_blob_validate`, I2C-kinds branch: any of r[3..5] != 0xFF
    // is OUT_OF_RANGE → PIN_CONFLICT).
    #[test]
    fn g1_ads1115_gpio_pin_rejected() {
        let errs = validate_blob(&blob_from_hex(&[
            "02 00 00 15 ff ff 00 00 00 00 00 00 00 00 00 00",
        ]));
        assert!(
            errs.iter().any(|e| e.contains("I2C")),
            "expected I2C pin error, got {errs:?}"
        );
    }

    #[test]
    fn g1_expander_gpio_pin_rejected() {
        // Round 3 (v4.0.4): the expander guard fires first — the record
        // is rejected as unsupported, so the G1 pin error never runs.
        // (The ADS1115/AS5600 G1 pin errors are unaffected; only kinds
        // 8/9 are guarded.)
        let errs = validate_blob(&blob_from_hex(&[
            "08 00 00 15 ff ff 00 00 00 00 00 00 00 00 00 00",
        ]));
        assert!(
            errs.iter().any(|e| e.contains("not supported yet")),
            "expected expander unsupported error, got {errs:?}"
        );
    }

    // G2 — Sequential SHIFTER (mode 0) pins get the class-only check
    // (C++ falls through to the generic loop for mode 0).
    #[test]
    fn g2_sequential_shifter_flash_pin_rejected() {
        let errs = validate_blob(&blob_from_hex(&[
            "0b 00 00 06 ff ff 00 00 00 00 00 00 00 00 00 00",
        ]));
        assert!(
            errs.iter().any(|e| e.contains("forbidden")),
            "expected forbidden-pin error, got {errs:?}"
        );
    }

    #[test]
    fn g2_sequential_shifter_pin_blocks_later_button() {
        // SHIFTER first (mode 0, pinA 25), BUTTON later on the same pin:
        // the post-pass must report the SHIFTER collision.
        let errs = validate_blob(&blob_from_hex(&[
            "01 00 00 22 ff ff 00 00 00 00 00 00 00 00 00 00",
            "0b 00 00 19 ff ff 00 00 00 00 00 00 00 00 00 00",
            "03 00 00 19 ff ff 00 00 00 00 00 00 00 00 00 00",
        ]));
        assert!(
            errs.iter().any(|e| e.contains("already used by SHIFTER")),
            "expected SHIFTER collision, got {errs:?}"
        );
    }

    #[test]
    fn g2_sequential_shifter_valid_pins_ok() {
        let errs = validate_blob(&blob_from_hex(&[
            "01 00 00 22 ff ff 00 00 00 00 00 00 00 00 00 00",
            "03 00 00 19 ff ff 00 00 00 00 00 00 00 00 00 00",
            "0b 00 00 1a 1b ff 00 00 00 00 00 00 00 00 00 00",
        ]));
        assert_eq!(errs, Vec::<String>::new(), "got {errs:?}");
    }

    // G3 — C++ counts EVERY axis/battery record toward the limits,
    // including pinless ones (which map to no module).
    #[test]
    fn g3_nine_pinless_axes_hit_limit() {
        let pinless = "01 00 00 ff ff ff 00 00 00 00 00 00 00 00 00 00";
        let errs = validate_blob(&blob_from_hex(&[pinless; 9]));
        assert!(
            errs.iter().any(|e| e == "too many axes (9 > 8)"),
            "expected axes limit error, got {errs:?}"
        );
    }

    #[test]
    fn g3_eight_pinless_axes_no_limit_error() {
        let pinless = "01 00 00 ff ff ff 00 00 00 00 00 00 00 00 00 00";
        let errs = validate_blob(&blob_from_hex(&[pinless; 8]));
        assert!(
            !errs.iter().any(|e| e.starts_with("too many axes")),
            "unexpected axes limit error, got {errs:?}"
        );
    }

    #[test]
    fn g3_two_pinless_batteries_hit_limit() {
        let pinless = "07 00 00 ff ff ff 00 00 00 00 00 00 00 00 00 00";
        let errs = validate_blob(&blob_from_hex(&[pinless, pinless]));
        assert!(
            errs.iter().any(|e| e == "too many battery modules (2 > 1)"),
            "expected battery limit error, got {errs:?}"
        );
    }

    // --- v4.0.2 regressions (F-14 LED_CONFIG, F-15 handbrake button
    // mode, D-4 shifter sensitivity) ---
    // Record bytes follow the firmware layouts in
    // `firmware/config_transaction.h` (§3.6, type 13 / slot 4 /
    // SHIFTER params[4]).

    /// One valid `LED_CONFIG` record next to a valid axis: accepted,
    /// no errors (previously: "unknown record type 13").
    #[test]
    fn led_config_valid_accepted() {
        let errs = validate_blob(&blob_from_hex(&[
            "01 00 00 22 ff ff 00 00 00 00 00 00 00 00 00 00",
            "0d 00 00 ff ff ff 01 0a 14 1e 32 00 00 00 00 00",
        ]));
        assert_eq!(errs, Vec::<String>::new(), "got {errs:?}");
    }

    #[test]
    fn led_config_pattern_rejected() {
        let errs = validate_blob(&blob_from_hex(&[
            "0d 00 00 ff ff ff 05 0a 14 1e 32 00 00 00 00 00",
        ]));
        assert!(
            errs.iter()
                .any(|e| e.contains("LED_CONFIG") && e.contains("pattern 5")),
            "expected LED_CONFIG pattern error, got {errs:?}"
        );
    }

    #[test]
    fn led_config_brightness_rejected() {
        let errs = validate_blob(&blob_from_hex(&[
            "0d 00 00 ff ff ff 01 0a 14 1e 65 00 00 00 00 00",
        ]));
        assert!(
            errs.iter()
                .any(|e| e.contains("LED_CONFIG") && e.contains("brightness 101")),
            "expected LED_CONFIG brightness error, got {errs:?}"
        );
    }

    #[test]
    fn led_config_flags_rejected() {
        let errs = validate_blob(&blob_from_hex(&[
            "0d 00 01 ff ff ff 01 0a 14 1e 32 00 00 00 00 00",
        ]));
        assert!(
            errs.iter()
                .any(|e| e.contains("LED_CONFIG") && e.contains("flags")),
            "expected LED_CONFIG flags error, got {errs:?}"
        );
    }

    #[test]
    fn led_config_pin_set_rejected() {
        let errs = validate_blob(&blob_from_hex(&[
            "0d 00 00 19 ff ff 01 0a 14 1e 32 00 00 00 00 00",
        ]));
        assert!(
            errs.iter()
                .any(|e| e.contains("LED_CONFIG") && e.contains("no GPIO pin")),
            "expected LED_CONFIG pin error, got {errs:?}"
        );
    }

    #[test]
    fn led_config_slot_rejected() {
        let errs = validate_blob(&blob_from_hex(&[
            "0d 01 00 ff ff ff 01 0a 14 1e 32 00 00 00 00 00",
        ]));
        assert!(
            errs.iter()
                .any(|e| e.contains("LED_CONFIG") && e.contains("slot 1")),
            "expected LED_CONFIG slot error, got {errs:?}"
        );
    }

    #[test]
    fn led_config_duplicate_rejected() {
        let led = "0d 00 00 ff ff ff 01 0a 14 1e 32 00 00 00 00 00";
        let errs = validate_blob(&blob_from_hex(&[
            "01 00 00 22 ff ff 00 00 00 00 00 00 00 00 00 00",
            led,
            led,
        ]));
        assert!(
            errs.iter()
                .any(|e| e == "too many LED config records (2 > 1)"),
            "expected LED_CONFIG duplicate error, got {errs:?}"
        );
    }

    /// Handbrake `AXIS_ADC` slot 4, flags bit0, pin 25 (pull-up pin):
    /// button mode — valid, no errors.
    #[test]
    fn handbrake_button_mode_valid() {
        let errs = validate_blob(&blob_from_hex(&[
            "01 04 01 19 ff ff 00 00 00 00 00 00 00 00 00 00",
        ]));
        assert_eq!(errs, Vec::<String>::new(), "got {errs:?}");
    }

    /// Same record via `AXIS_GENERIC` (type 10): also button mode.
    #[test]
    fn handbrake_generic_button_mode_valid() {
        let errs = validate_blob(&blob_from_hex(&[
            "0a 04 01 19 ff ff 00 00 00 00 00 00 00 00 00 00",
        ]));
        assert_eq!(errs, Vec::<String>::new(), "got {errs:?}");
    }

    #[test]
    fn handbrake_button_mode_flags_rejected() {
        let errs = validate_blob(&blob_from_hex(&[
            "01 04 03 19 ff ff 00 00 00 00 00 00 00 00 00 00",
        ]));
        assert!(
            errs.iter()
                .any(|e| e.contains("handbrake axis") && e.contains("flags 0x03")),
            "expected handbrake flags error, got {errs:?}"
        );
    }

    /// Button mode on GPIO34 (input-only, no pull-up): the pull-up
    /// rule rejects it — the ADC1 rule must NOT save it.
    #[test]
    fn handbrake_button_mode_no_pullup_rejected() {
        let errs = validate_blob(&blob_from_hex(&[
            "01 04 01 22 ff ff 00 00 00 00 00 00 00 00 00 00",
        ]));
        assert!(
            errs.iter()
                .any(|e| e.contains("GPIO34") && e.contains("pull-up")),
            "expected GPIO34 pull-up error, got {errs:?}"
        );
    }

    /// Handbrake slot 4, flags 0, pin 32: plain axis mode, unchanged.
    #[test]
    fn handbrake_axis_mode_unchanged() {
        let errs = validate_blob(&blob_from_hex(&[
            "01 04 00 20 ff ff 00 00 00 00 00 00 00 00 00 00",
        ]));
        assert_eq!(errs, Vec::<String>::new(), "got {errs:?}");
    }

    /// Shifter sensitivity boundary: 100 accepted (sequential mode,
    /// no gear pins).
    #[test]
    fn shifter_sensitivity_boundary_accepted() {
        let errs = validate_blob(&blob_from_hex(&[
            "01 00 00 22 ff ff 00 00 00 00 00 00 00 00 00 00",
            "0b 00 00 ff ff ff 00 00 00 00 64 00 00 00 00 00",
        ]));
        assert_eq!(errs, Vec::<String>::new(), "got {errs:?}");
    }

    #[test]
    fn shifter_sensitivity_rejected() {
        let errs = validate_blob(&blob_from_hex(&[
            "0b 00 00 ff ff ff 00 00 00 00 65 00 00 00 00 00",
        ]));
        assert!(
            errs.iter()
                .any(|e| e.contains("SHIFTER") && e.contains("sensitivity 101")),
            "expected SHIFTER sensitivity error, got {errs:?}"
        );
    }

    /// Sensitivity helper: default 50 with no SHIFTER record, the
    /// record's field when present (0 means 0, not default), and the
    /// debounce map from the firmware gate doc (0→20, 25→17, 50→13,
    /// 75→9, 100→5).
    #[test]
    fn shifter_sensitivity_default_and_debounce() {
        use super::{shifter_debounce_ms, shifter_sensitivity};
        assert_eq!(shifter_sensitivity(&ConfigBlob::new(1, vec![])), 50);
        let with_75 = blob_from_hex(&["0b 00 00 ff ff ff 00 00 00 00 4b 00 00 00 00 00"]);
        assert_eq!(shifter_sensitivity(&with_75), 75);
        let with_0 = blob_from_hex(&["0b 00 00 ff ff ff 00 00 00 00 00 00 00 00 00 00"]);
        assert_eq!(shifter_sensitivity(&with_0), 0);
        assert_eq!(shifter_debounce_ms(0), 20);
        assert_eq!(shifter_debounce_ms(25), 17);
        assert_eq!(shifter_debounce_ms(50), 13);
        assert_eq!(shifter_debounce_ms(75), 9);
        assert_eq!(shifter_debounce_ms(100), 5);
    }
}
