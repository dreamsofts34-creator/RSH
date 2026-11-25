//! Tauri command logic for the commands the frontend calls that have a
//! real backend today (Phase E gate).
//!
//! These functions are the command *logic* — pure Rust over the library
//! crates, fully testable here. The thin `#[tauri::command]` wrappers in
//! `src-tauri/src/main.rs` only forward arguments and results; they cannot
//! be compiled in this Linux sandbox (Tauri needs WebKitGTK), so the logic
//! lives here where `cargo test` covers it.
//!
//! Commands implemented here:
//! - [`crate::commands::validate_pin_map`] — pin-map validation via
//!   `rsh_protocol::validate_blob` (the single authority; the frontend
//!   never re-implements pin rules). [`PinAssignment`] now carries
//!   `pins[]` as well as the legacy single `pin`, so multi-pin
//!   functions (shifter, encoders, button matrix) validate instead of
//!   erroring.
//! - [`crate::commands::validate_firmware_image`] — signed-firmware validation via
//!   `rsh_security::verify_firmware_image` (Ed25519 signature + format).
//! - [`crate::commands::get_engine_stats`] / [`crate::commands::get_snapshot`] —
//!   the snapshot API (see `crate::snapshot`): `EngineMetrics` → the
//!   frontend `EngineStats` contract, plus the 30 Hz `rsh://snapshot`
//!   payload (loop rate measured, never a constant; p99 `null` while
//!   unmeasured; encoder data with derived buttons 65–72).
//! - [`crate::commands::list_backups`] — the 3-generation backup list.
//! - [`crate::commands::get_firewall_state`] — Windows Firewall rule
//!   probe (Windows-only; an honest "unavailable" elsewhere).
//! - [`crate::commands::get_output_state`] / [`crate::commands::set_output_state`] —
//!   output-backend state, including the `enabled` flag.
//! - [`crate::commands::validate_tuning`] — the extended tuning contract
//!   (pedal bite point, shifter gate, horn button, LED brightness/pattern).
//! - Button bindings ([`crate::commands::parse_button_bindings`],
//!   [`crate::commands::validate_button_bindings`]) — the Rust side of
//!   the `ButtonBindings` shape owned by `frontend/src/screens/Buttons/bindings.ts`.
//! - Pin-map / config conversions for the H5-control commands
//!   ([`crate::commands::pin_assignments_to_role_records`],
//!   [`crate::commands::records_to_pin_assignments`],
//!   [`crate::commands::merge_pin_map_into_blob`],
//!   [`crate::commands::config_blob_to_json`],
//!   [`crate::commands::config_json_to_blob`]) — the role-based record
//!   conversion `set_pin_map` stages, the single-pin map `get_pin_map`
//!   reads, and the JSON shape `get_config` / `stage_config` carry.
//!   The board interprets record slots as roles (PROTOCOL-V2.1 §3.6),
//!   so the board write converts by role, never by list position.
//!
//! # Why there is no `#[tauri::command]` in this file
//!
//! The thin `#[tauri::command]` wrappers live in `src-tauri/src/main.rs`
//! (the coordinator registers them at the gate). This crate cannot carry
//! a `tauri` dependency: it would not compile in this Linux sandbox
//! (Tauri needs WebKitGTK), which would kill `cargo test` for every
//! command's logic. The logic functions here have the exact command
//! names and JSON shapes the wrappers forward to; see the report for
//! the registration list.
//!
//! # Anti-downgrade scope (read this before trusting `valid`)
//!
//! [`crate::commands::validate_firmware_image`] verifies the signature and the image
//! format only. It passes current version `(0, 0, 0)` to the verifier,
//! so it never rejects a downgrade: the command does not know the
//! installed board version. The version-aware form is
//! [`crate::commands::validate_firmware_image_against`]: pass the
//! installed version from `get_firmware_version` and it rejects a
//! downgrade (reporting the image version too, so the UI can show
//! both). The frontend's check is a gate, not a trust decision —
//! `upload_firmware` (Phase F) and the [`crate::webota::flash_firmware`]
//! flow re-validate with the same verifier before any upload.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use rsh_api::UserError;
use rsh_engine::metrics::EngineMetrics;
pub use rsh_profiles::store::BackupInfo;
use rsh_profiles::store::list_backups_in;
use rsh_protocol::config::{ConfigBlob, ConfigRecord, record_type};
use rsh_protocol::pin::validate_blob;
use rsh_security::firmware_verify::{
    FirmwareImageInfo, FirmwareVerifyError, verify_firmware_image,
};
use serde::{Deserialize, Serialize};

use crate::snapshot::{EngineSnapshot, EngineStats, SnapshotInput};

/// One pin assignment from the Settings pin-map editor:
/// a module/function name and the GPIO pin(s) assigned to it.
///
/// Single-pin functions use `pin` (the Phase E shape, kept for
/// compatibility); multi-pin functions (shifter, encoders, button
/// matrix) use `pins`. When `pins` is non-empty it wins and `pin` is
/// ignored. The frontend sends the same shape
/// (`PinAssignment` in `frontend/src/lib/tauri.ts`).
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct PinAssignment {
    /// Module/function name, e.g. `"steering"`, `"brake"`, `"led"`.
    pub function: String,
    /// GPIO pin number on the ESP32 (0–39) for single-pin functions.
    pub pin: u8,
    /// GPIO pins for multi-pin functions (shifter: 6 gear pins,
    /// encoders: 2 pins per encoder, button matrix: rows + cols pins).
    /// Empty for single-pin functions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pins: Vec<u8>,
}

/// Result of [`crate::commands::validate_pin_map`]: the validator's errors (if any).
/// `warnings` is always empty for now — `validate_blob` returns errors
/// only, with no warnings split; a later split is a backend-only change
/// (the frontend already renders both lists).
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct PinMapValidation {
    /// True when the map passes every pin rule.
    pub valid: bool,
    /// One message per failed rule (empty when valid).
    pub errors: Vec<String>,
    /// Advisory messages (currently always empty — see the struct docs).
    pub warnings: Vec<String>,
}

/// Result of [`crate::commands::validate_firmware_image`].
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FirmwareValidation {
    /// True when the image is signed, well-formed, and has a version.
    pub valid: bool,
    /// Parsed image version (`"a.b.c"`), when one was found.
    pub version: Option<String>,
    /// Whether the Ed25519 signature verified. False when the signature
    /// could not be checked at all (truncated/unsigned/malformed image).
    pub signature_valid: bool,
    /// One human-readable message per failed check (empty when valid).
    pub errors: Vec<String>,
}

/// Check every pin in `pins` is in range and unique, then return them.
/// Shared by the single-pin and multi-pin paths so the range error is
/// identical either way.
fn checked_pins(function: &str, pins: &[u8]) -> Result<Vec<u8>, String> {
    let mut seen: Vec<u8> = Vec::new();
    for &pin in pins {
        if pin > 39 {
            return Err(format!(
                "pin {pin} out of range (0-39) for function '{function}'"
            ));
        }
        if seen.contains(&pin) {
            return Err(format!("pin {pin} is used twice in function '{function}'"));
        }
        seen.push(pin);
    }
    Ok(seen)
}

/// Map one pin assignment to the config record(s) the validator
/// understands. Single-pin functions map to one record each;
/// multi-pin functions map to one record per module instance:
/// - `shifter` with 6 pins → one `SHIFTER` record (gear pins 1–3 on
///   `pinA`/`pinB`/`pinC`, gears 4–6 on `params[1..4]`, sequential gate
///   `params[0] = 0`);
/// - `encoders` with an even number of pins → one `ENCODER` record per
///   pair (slots 0, 1, …);
/// - `button_matrix` with 8 pins → one `BUTTON_MATRIX` record (4 rows ×
///   4 cols, `params[0] = 4`, `params[1] = 4`).
///
/// `slot` is the first free slot byte; multi-record assignments consume
/// consecutive slots. Unknown functions and wrong pin counts are errors,
/// never silently skipped.
fn assignment_records(assignment: &PinAssignment, slot: u8) -> Result<Vec<ConfigRecord>, String> {
    let pins = if assignment.pins.is_empty() {
        vec![assignment.pin]
    } else {
        assignment.pins.clone()
    };
    let pins = checked_pins(&assignment.function, &pins)?;
    match assignment.function.as_str() {
        "steering" | "accelerator" | "brake" | "clutch" | "handbrake" | "horn" | "buttons"
        | "led" => {
            if pins.len() != 1 {
                return Err(format!(
                    "function '{}' takes exactly 1 pin, got {}",
                    assignment.function,
                    pins.len()
                ));
            }
            Ok(vec![single_pin_record(&assignment.function, pins[0], slot)])
        }
        "shifter" | "encoders" | "button_matrix" => {
            multi_pin_records(&assignment.function, &pins, slot)
        }
        other => Err(format!("unknown function '{other}'")),
    }
}

/// One record for a single-pin function.
fn single_pin_record(function: &str, pin: u8, slot: u8) -> ConfigRecord {
    let rec_type = match function {
        "steering" | "accelerator" | "brake" | "clutch" | "handbrake" => record_type::AXIS_ADC,
        "horn" | "buttons" => record_type::BUTTON,
        _ => record_type::LED_STRIP, // "led" (the only remaining caller)
    };
    ConfigRecord::new(rec_type, slot, pin, 0xFF, 0xFF)
}

/// Records for a multi-pin function (`shifter`, `encoders`,
/// `button_matrix`). See [`assignment_records`] for the layouts.
fn multi_pin_records(function: &str, pins: &[u8], slot: u8) -> Result<Vec<ConfigRecord>, String> {
    match function {
        "shifter" => {
            if pins.len() != 6 {
                return Err(format!(
                    "function 'shifter' needs 6 gear pins, got {}",
                    pins.len()
                ));
            }
            let mut record =
                ConfigRecord::new(record_type::SHIFTER, slot, pins[0], pins[1], pins[2]);
            record.params[0] = 0; // Sequential gate.
            record.params[1] = pins[3];
            record.params[2] = pins[4];
            record.params[3] = pins[5];
            Ok(vec![record])
        }
        "encoders" => {
            if !pins.len().is_multiple_of(2) || pins.is_empty() || pins.len() > 8 {
                return Err(format!(
                    "function 'encoders' needs 2 pins per encoder (2, 4, 6 or 8 pins), got {}",
                    pins.len()
                ));
            }
            let mut records = Vec::new();
            for (index, pair) in pins.as_chunks::<2>().0.iter().enumerate() {
                let encoder_slot = slot.wrapping_add(u8::try_from(index).unwrap_or(u8::MAX));
                records.push(ConfigRecord::new(
                    record_type::ENCODER,
                    encoder_slot,
                    pair[0],
                    pair[1],
                    0xFF,
                ));
            }
            Ok(records)
        }
        "button_matrix" => {
            if pins.len() != 8 {
                return Err(format!(
                    "function 'button_matrix' needs 8 pins (4 rows + 4 cols), got {}",
                    pins.len()
                ));
            }
            let mut record =
                ConfigRecord::new(record_type::BUTTON_MATRIX, slot, pins[0], pins[1], pins[2]);
            record.params[0] = 4; // Rows.
            record.params[1] = 4; // Cols.
            record.params[2] = pins[3];
            record.params[3] = pins[4];
            record.params[4] = pins[5];
            record.params[5] = pins[6];
            record.params[6] = pins[7];
            // Unused param slots must be 0xFF: the record→module
            // conversion keeps every param that is not 0xFF as a pin.
            record.params[7] = 0xFF;
            record.params[8] = 0xFF;
            record.params[9] = 0xFF;
            Ok(vec![record])
        }
        other => Err(format!("unknown function '{other}'")),
    }
}

/// Validate a pin map (Settings editor) against the board pin rules.
///
/// Builds a [`ConfigBlob`] from the assignments and runs
/// [`validate_blob`] — the same validator the board runs at COMMIT, so
/// the UI rejects exactly what the board would reject (and never rejects
/// what the board would accept, except for the deliberately stricter
/// BUG-5/BUG-6c cases).
/// A bad map is never sent to the board: the frontend gates Apply on
/// `valid`, and this command is the defence in depth behind it.
///
/// Multi-pin functions (`shifter`, `encoders`, `button_matrix`) and
/// unknown functions are reported as errors, not silently skipped — a
/// `valid: true` verdict must cover every pin the caller sent.
#[must_use]
pub fn validate_pin_map(pins: &[PinAssignment]) -> PinMapValidation {
    let mut errors: Vec<String> = Vec::new();
    let mut records: Vec<ConfigRecord> = Vec::new();
    let mut slot: u8 = 0;
    for assignment in pins {
        // Slot is only an identifier byte; a list longer than the blob
        // can hold (127 records) is rejected by the limits below anyway.
        match assignment_records(assignment, slot) {
            Ok(mut new_records) => {
                slot = slot.wrapping_add(u8::try_from(new_records.len()).unwrap_or(u8::MAX));
                records.append(&mut new_records);
            }
            Err(error) => errors.push(error),
        }
    }
    if !errors.is_empty() {
        return PinMapValidation {
            valid: false,
            errors,
            warnings: Vec::new(),
        };
    }
    let blob = ConfigBlob::new(1, records);
    let mut blob_errors = validate_blob(&blob);
    errors.append(&mut blob_errors);
    PinMapValidation {
        valid: errors.is_empty(),
        errors,
        warnings: Vec::new(),
    }
}

/* ------------------------------------------------------------------ */
/* Pin map <-> config records, role-based (H5-control).                */
/*                                                                     */
/* `validate_pin_map` above converts assignments with a running slot */
/* counter: the counter is only a record identifier, and `validate_blob`*/
/* ignores slots entirely, so that conversion is correct *for          */
/* validation* — but the board interprets slots as roles (PROTOCOL-   */
/* V2.1 §3.6: AXIS_ADC slot 3 is the clutch, slot 4 the handbrake;     */
/* BUTTON slot 0 is shifter-up, 1 shifter-down, 2 horn). A board write */
/* built from the running counter would silently re-role pins (a       */
/* handbrake submitted without a clutch would land in the clutch slot). */
/* The functions below convert by ROLE, so a board write means exactly */
/* what the editor submitted.                                          */
/*                                                                     */
/* The generic `buttons` function has no board consumer in protocol    */
/* v2.1 (firmware `pinmap.h`: BUTTON slot > 2 has no consumer, and the */
/* PC builder never sends slot 3) — it is rejected here instead of     */
/* being staged into a dead slot.                                      */
/* ------------------------------------------------------------------ */

/// The board role of a `BUTTON` slot (PROTOCOL-V2.1 §3.6): slot 0 is
/// shifter-up, 1 shifter-down, 2 horn. Slots above 2 have no board
/// consumer.
#[must_use]
pub const fn button_action_for_slot(slot: u8) -> Option<&'static str> {
    match slot {
        0 => Some("shifter_up"),
        1 => Some("shifter_down"),
        2 => Some("horn"),
        _ => None,
    }
}

/// The `BUTTON` slot for a button action (inverse of
/// [`button_action_for_slot`]).
#[must_use]
pub fn button_slot_for_action(action: &str) -> Option<u8> {
    match action {
        "shifter_up" => Some(0),
        "shifter_down" => Some(1),
        "horn" => Some(2),
        _ => None,
    }
}

/// The board slot for a single-pin function (`None` for functions
/// that need multi-pin records or have no board role).
fn role_slot(function: &str) -> Option<u8> {
    match function {
        "steering" | "led" => Some(0),
        "accelerator" => Some(1),
        "brake" | "horn" => Some(2),
        "clutch" => Some(3),
        "handbrake" => Some(4),
        _ => None,
    }
}

/// Convert pin assignments to config records by board role (H5-control,
/// the conversion `set_pin_map` stages to the board).
///
/// Single-pin functions map to their protocol slot
/// (`steering` → `AXIS_ADC` 0, `accelerator` → 1, `brake` → 2,
/// `clutch` → 3, `handbrake` → 4, `horn` → `BUTTON` 2, `led` →
/// `LED_STRIP` 0); multi-pin functions map exactly like
/// [`assignment_records`] (shifter → one `SHIFTER` record, encoders →
/// one `ENCODER` record per pair starting at slot 0, button matrix →
/// one `BUTTON_MATRIX` record at slot 0).
///
/// # Errors
///
/// Returns a message for an unknown function, a wrong pin count, an
/// out-of-range or duplicate pin, or the generic `buttons` function
/// (no board consumer in protocol v2.1 — see the section docs).
/// Nothing is silently skipped.
pub fn pin_assignments_to_role_records(
    pins: &[PinAssignment],
) -> Result<Vec<ConfigRecord>, String> {
    let mut records: Vec<ConfigRecord> = Vec::new();
    for assignment in pins {
        if assignment.function == "buttons" {
            return Err(
                "function 'buttons' has no board pin in protocol v2.1 — BUTTON slots are \
                 0 shifter-up, 1 shifter-down, 2 horn (assign the horn pin via 'horn')"
                    .to_string(),
            );
        }
        let pins_list = if assignment.pins.is_empty() {
            vec![assignment.pin]
        } else {
            assignment.pins.clone()
        };
        let pins_list = checked_pins(&assignment.function, &pins_list)?;
        match assignment.function.as_str() {
            "steering" | "accelerator" | "brake" | "clutch" | "handbrake" | "horn" | "led" => {
                if pins_list.len() != 1 {
                    return Err(format!(
                        "function '{}' takes exactly 1 pin, got {}",
                        assignment.function,
                        pins_list.len()
                    ));
                }
                let slot = role_slot(&assignment.function)
                    .ok_or_else(|| format!("unknown function '{}'", assignment.function))?;
                records.push(single_pin_record(&assignment.function, pins_list[0], slot));
            }
            "shifter" | "encoders" | "button_matrix" => {
                // Multi-pin records carry their own slots (shifter 0,
                // encoders from 0, button matrix 0) — the running
                // counter of `assignment_records` is not used here.
                let mut new_records = multi_pin_records(&assignment.function, &pins_list, 0)?;
                records.append(&mut new_records);
            }
            other => {
                return Err(format!("unknown function '{other}'"));
            }
        }
    }
    Ok(records)
}

/// Convert config records back to the single-pin assignments the
/// Settings editor can round-trip (H5-control, the conversion
/// `get_pin_map` reads from the board).
///
/// Only single-pin functions are returned: axes by slot
/// (0 steering, 1 accelerator, 2 brake, 3 clutch, 4 handbrake),
/// `BUTTON` slot 2 (horn), `LED_STRIP` slot 0 (led). Everything else
/// is skipped deliberately, never guessed: `BUTTON` slots 0/1 are the
/// sequential shifter up/down inputs (no editor function), multi-pin
/// records (shifter, encoders, button matrix) cannot round-trip
/// through the editor's single-select rows, and battery / I²C /
/// expander records are not pin-map functions. Skipped records are
/// preserved untouched by
/// [`merge_pin_map_into_blob`], so reading the map never loses them.
#[must_use]
pub fn records_to_pin_assignments(records: &[ConfigRecord]) -> Vec<PinAssignment> {
    let mut out: Vec<PinAssignment> = Vec::new();
    for record in records {
        if record.pin_a == 0xFF {
            continue;
        }
        let function = match (record.rec_type, record.slot) {
            (record_type::AXIS_ADC, 0) => Some("steering"),
            (record_type::AXIS_ADC, 1) => Some("accelerator"),
            (record_type::AXIS_ADC, 2) => Some("brake"),
            (record_type::AXIS_ADC, 3) => Some("clutch"),
            (record_type::AXIS_ADC, 4) => Some("handbrake"),
            (record_type::BUTTON, 2) => Some("horn"),
            (record_type::LED_STRIP, 0) => Some("led"),
            _ => None,
        };
        if let Some(function) = function {
            out.push(PinAssignment {
                function: function.to_string(),
                pin: record.pin_a,
                pins: Vec::new(),
            });
        }
    }
    out
}

/// Single-pin (type, slot) pairs the pin-map editor can express.
/// A live record on one of these pairs is replaced by the submitted
/// map — and removed when the submission omits it (the editor always
/// submits its full draft, so omission means "unassigned"). Every
/// other live record is preserved untouched.
fn expressible_single_pin_slots() -> Vec<(u8, u8)> {
    vec![
        (record_type::AXIS_ADC, 0),
        (record_type::AXIS_ADC, 1),
        (record_type::AXIS_ADC, 2),
        (record_type::AXIS_ADC, 3),
        (record_type::AXIS_ADC, 4),
        (record_type::BUTTON, 2),
        (record_type::LED_STRIP, 0),
    ]
}

/// Merge a submitted pin map into the board's live config blob
/// (H5-control, the merge `set_pin_map` stages).
///
/// The submission is the COMPLETE pin map (the Settings editor always
/// submits its full draft): every expressible single-pin slot is
/// replaced by the submission, and an expressible slot the submission
/// omits is removed from the blob (replace semantics — a function with
/// no record is not sampled by the board). Multi-pin records
/// (shifter, encoders, button matrix) are replaced only when the
/// submission carries them; otherwise the live ones are preserved.
/// All other live records (`BUTTON` slots 0/1, battery, I²C/expander
/// kinds) are preserved untouched — the pin map cannot express them,
/// so it must not delete them.
///
/// # Errors
///
/// Returns a message when the submission fails role conversion (see
/// [`pin_assignments_to_role_records`]) or the merged blob fails
/// [`validate_blob`] (e.g. a submitted pin duplicates a preserved
/// record's pin). A failed merge stages nothing.
pub fn merge_pin_map_into_blob(
    live: &ConfigBlob,
    pins: &[PinAssignment],
) -> Result<ConfigBlob, String> {
    let new_records = pin_assignments_to_role_records(pins)?;
    let expressible = expressible_single_pin_slots();
    let mut merged: Vec<ConfigRecord> = live
        .records
        .iter()
        .copied()
        .filter(|record| {
            // Keep every record the submission cannot express, plus
            // multi-pin records the submission does not replace.
            if expressible.contains(&(record.rec_type, record.slot)) {
                return false;
            }
            if matches!(
                record.rec_type,
                record_type::SHIFTER | record_type::ENCODER | record_type::BUTTON_MATRIX
            ) {
                return !new_records
                    .iter()
                    .any(|new| new.rec_type == record.rec_type && new.slot == record.slot);
            }
            true
        })
        .collect();
    merged.extend_from_slice(&new_records);
    let blob = ConfigBlob::new(live.cfg_version, merged);
    let errors = validate_blob(&blob);
    if !errors.is_empty() {
        return Err(format!(
            "merged pin map failed board validation: {}",
            errors.join("; ")
        ));
    }
    blob.encode()
        .map_err(|error| format!("merged pin map could not be encoded: {error}"))?;
    Ok(blob)
}

/* ------------------------------------------------------------------ */
/* Config JSON (H5-control) — the `get_config` / `stage_config` shape.*/
/*                                                                     */
/* ```json                                                            */
/* {                                                                 */
/*   "cfgVersion": 1,                                                */
/*   "records": [                                                    */
/*     {"type": 1, "slot": 0, "flags": 0, "pinA": 34, "pinB": 255,   */
/*      "pinC": 255, "params": [0, 0, ...]}, ...                     */
/*   ],                                                              */
/*   "buttons": {"1": "shifter_up", "2": "shifter_down", "3": "horn"}*/
/* }                                                                 */
/* ```                                                               */
/*                                                                     */
/* `records` is the authoritative blob content, round-tripped verbatim.*/
/* `buttons` is a derived view of the three fixed `BUTTON` slot roles */
/* (PROTOCOL-V2.1 §3.6) — the only button→action mapping the board    */
/* stores. Arbitrary button→action rebinding is NOT a board feature:  */
/* the stream button bits are the slots, so a rebind cannot be staged */
/* (see `config_json_to_blob`); that needs a PC-side binding store,   */
/* which is an open design item, not a silent board write.            */
/* ------------------------------------------------------------------ */

/// Serialize one config record to its JSON shape.
fn config_record_to_json(record: &ConfigRecord) -> serde_json::Value {
    serde_json::json!({
        "type": record.rec_type,
        "slot": record.slot,
        "flags": record.flags,
        "pinA": record.pin_a,
        "pinB": record.pin_b,
        "pinC": record.pin_c,
        "params": record.params,
    })
}

/// Serialize a config blob to the `get_config` JSON shape: the
/// `cfgVersion`, the full `records` list, and the derived `buttons`
/// view (only the `BUTTON` slots present in the blob, by fixed role).
#[must_use]
pub fn config_blob_to_json(blob: &ConfigBlob) -> serde_json::Value {
    let records: Vec<serde_json::Value> = blob.records.iter().map(config_record_to_json).collect();
    let mut buttons = serde_json::Map::new();
    for record in &blob.records {
        if record.rec_type == record_type::BUTTON
            && record.pin_a != 0xFF
            && let Some(action) = button_action_for_slot(record.slot)
        {
            buttons.insert(
                (u16::from(record.slot) + 1).to_string(),
                serde_json::Value::String(action.to_string()),
            );
        }
    }
    serde_json::json!({
        "cfgVersion": blob.cfg_version,
        "records": records,
        "buttons": buttons,
    })
}

/// Parse one JSON record object back to a [`ConfigRecord`]. `params`
/// defaults to zeros when absent; a present `params` must have exactly
/// 10 entries. Every other field is required and must fit in a byte.
fn config_record_from_json(value: &serde_json::Value) -> Result<ConfigRecord, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "config record must be an object".to_string())?;
    let byte = |name: &str| -> Result<u8, String> {
        object
            .get(name)
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| u8::try_from(n).ok())
            .ok_or_else(|| format!("config record field '{name}' must be a byte (0-255)"))
    };
    let mut params = [0u8; 10];
    if let Some(raw_params) = object.get("params") {
        let list = raw_params
            .as_array()
            .ok_or_else(|| "config record field 'params' must be an array".to_string())?;
        if list.len() != 10 {
            return Err("config record field 'params' must have exactly 10 entries".to_string());
        }
        for (index, entry) in list.iter().enumerate() {
            params[index] = entry
                .as_u64()
                .and_then(|n| u8::try_from(n).ok())
                .ok_or_else(|| format!("config record params[{index}] must be a byte (0-255)"))?;
        }
    }
    Ok(ConfigRecord {
        rec_type: byte("type")?,
        slot: byte("slot")?,
        flags: byte("flags")?,
        pin_a: byte("pinA")?,
        pin_b: byte("pinB")?,
        pin_c: byte("pinC")?,
        params,
    })
}

/// Strictly parse a `buttons` map: every key must be a button number
/// and every value a non-empty action string. Unlike
/// [`parse_button_bindings`] (lenient, for reading), staging must
/// never silently drop a corrupt entry.
fn parse_button_bindings_strict(raw: &serde_json::Value) -> Result<ButtonBindings, String> {
    let map = raw
        .as_object()
        .ok_or_else(|| "config 'buttons' must be an object".to_string())?;
    let mut bindings: ButtonBindings = BTreeMap::new();
    for (key, value) in map {
        let button: u8 = key
            .parse()
            .map_err(|_| format!("config 'buttons' has a non-numeric button key '{key}'"))?;
        if button < 1 {
            return Err("config 'buttons' has button 0 (buttons start at 1)".to_string());
        }
        let action = value.as_str().filter(|s| !s.is_empty()).ok_or_else(|| {
            format!("config 'buttons' entry for button {button} must be a non-empty action string")
        })?;
        bindings.insert(button, action.to_string());
    }
    Ok(bindings)
}

/// Rebuild a config blob from the `get_config` JSON shape.
///
/// `records` is authoritative; `cfgVersion` is required and carried
/// verbatim. When the `buttons` key is present it is the full binding
/// map for the three fixed `BUTTON` slots, reconciled against the
/// records: an entry whose action is not the slot's fixed role is an
/// error (a button input cannot be re-roled on the board — pin changes
/// go through the pin map), an entry for a button with no `BUTTON`
/// record is an error (no pin assigned for that role), an entry for
/// any other button is an error (only buttons 1–3 are board inputs),
/// and a missing entry unbinds that slot (its `BUTTON` record is
/// removed — replace semantics). When `buttons` is absent the records
/// are used exactly as given.
///
/// # Errors
///
/// Returns a message for a malformed shape, a blob that fails to
/// decode/encode or [`validate_blob`], or any `buttons` inconsistency
/// above. A failed rebuild stages nothing.
pub fn config_json_to_blob(value: &serde_json::Value) -> Result<ConfigBlob, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "config must be a JSON object".to_string())?;
    let cfg_version: u16 = object
        .get("cfgVersion")
        .and_then(serde_json::Value::as_u64)
        .and_then(|n| u16::try_from(n).ok())
        .ok_or_else(|| "config field 'cfgVersion' must be a number (0-65535)".to_string())?;
    let raw_records = object
        .get("records")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "config field 'records' must be an array".to_string())?;
    let mut records: Vec<ConfigRecord> = Vec::with_capacity(raw_records.len());
    for raw in raw_records {
        records.push(config_record_from_json(raw)?);
    }
    if let Some(raw_buttons) = object.get("buttons") {
        let bindings = parse_button_bindings_strict(raw_buttons)?;
        let binding_errors = validate_button_bindings(&bindings);
        if !binding_errors.is_empty() {
            return Err(format!(
                "config 'buttons' is invalid: {}",
                binding_errors.join("; ")
            ));
        }
        for (&button, action) in &bindings {
            let Some(slot) = button_slot_for_action(action) else {
                return Err(format!(
                    "button {button} is bound to '{action}', which is not a board button role \
                     (board roles are shifter_up, shifter_down, horn)"
                ));
            };
            if u16::from(button) != u16::from(slot) + 1 {
                return Err(format!(
                    "button {button} is bound to '{action}', but '{action}' is the board's \
                     button {} — a button input cannot be re-roled on the board; move pins \
                     in the pin map instead",
                    u16::from(slot) + 1
                ));
            }
            if !records
                .iter()
                .any(|record| record.rec_type == record_type::BUTTON && record.slot == slot)
            {
                return Err(format!(
                    "button {button} is bound to '{action}' but the board has no pin for it — \
                     assign one in the pin map first"
                ));
            }
        }
        // A missing entry unbinds its slot: remove that BUTTON record.
        // (Entries for buttons 4..32 / 65..72 were already rejected
        // above — only buttons 1–3 have board slots at all.)
        records.retain(|record| {
            if record.rec_type != record_type::BUTTON {
                return true;
            }
            let Some(action) = button_action_for_slot(record.slot) else {
                return true;
            };
            let button = record.slot + 1;
            bindings.get(&button).is_some_and(|bound| bound == action)
        });
    }
    let blob = ConfigBlob::new(cfg_version, records);
    // Encode + decode proves the blob is well-formed (size, count,
    // CRC) before the pin rules run.
    let encoded = blob
        .encode()
        .map_err(|error| format!("config blob could not be encoded: {error}"))?;
    let decoded = ConfigBlob::decode(&encoded)
        .map_err(|error| format!("config blob failed to decode: {error}"))?;
    let errors = validate_blob(&decoded);
    if !errors.is_empty() {
        return Err(format!(
            "config failed board validation: {}",
            errors.join("; ")
        ));
    }
    Ok(decoded)
}

/// Shifter sensitivity (`SHIFTER` record `params[4]`, 0..100) of a
/// `get_config` JSON blob — [`None`] when the board has no SHIFTER
/// record (no shifter configured; the firmware default 50 then
/// applies on the board, but there is nothing to edit). Mirrors
/// [`rsh_protocol::pin::shifter_sensitivity`] on the decoded blob;
/// this is the JSON-side read for the daemon API, which only ever
/// sees the `get_config` shape. A stored value above 100 (a blob
/// that could never validate) reads as [`None`], never as a lie.
#[must_use]
pub fn shifter_sensitivity_from_config(config: &serde_json::Value) -> Option<u8> {
    let records = config.get("records")?.as_array()?;
    let record = records.iter().find(|record| {
        record.get("type").and_then(serde_json::Value::as_u64)
            == Some(u64::from(record_type::SHIFTER))
    })?;
    let params = record.get("params")?.as_array()?;
    let sensitivity = params
        .get(rsh_protocol::SHIFTER_SENSITIVITY_PARAM)?
        .as_u64()?;
    u8::try_from(sensitivity)
        .ok()
        .filter(|sensitivity| *sensitivity <= 100)
}

/// Set the shifter sensitivity (`SHIFTER` record `params[4]`) in a
/// `get_config` JSON blob, returning a new value with everything
/// else verbatim (records, buttons, `cfgVersion`) — the shape
/// `stage_config` accepts. This is the SH1 write: the UI slider's
/// 0–100 value lands in the board config blob, and the firmware
/// applies the new debounce (`rsh_shifter_debounce_ms`) on COMMIT
/// with no reboot (`inputs_apply_config_blob` re-reads it before
/// its pinmap early-return).
///
/// # Errors
///
/// Returns a message when the sensitivity is outside 0..=100 or
/// the blob has no SHIFTER record — a board with no shifter
/// configured has nowhere to store it, so the slider stays
/// disabled instead of inventing a record with unknown pins.
pub fn set_shifter_sensitivity_in_config(
    config: &serde_json::Value,
    sensitivity: u8,
) -> Result<serde_json::Value, String> {
    if sensitivity > 100 {
        return Err(format!(
            "shifter sensitivity {sensitivity} is outside 0..=100"
        ));
    }
    let mut updated = config.clone();
    let Some(records) = updated
        .get_mut("records")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return Err("config has no 'records' list".to_string());
    };
    let Some(record) = records.iter_mut().find(|record| {
        record.get("type").and_then(serde_json::Value::as_u64)
            == Some(u64::from(record_type::SHIFTER))
    }) else {
        return Err(
            "no shifter is configured on the board — assign shifter pins in the pin map first"
                .to_string(),
        );
    };
    let Some(params) = record
        .get_mut("params")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return Err("shifter record has no 'params' list".to_string());
    };
    let Some(slot) = params.get_mut(rsh_protocol::SHIFTER_SENSITIVITY_PARAM) else {
        return Err("shifter record params are shorter than 5 entries".to_string());
    };
    *slot = serde_json::Value::from(sensitivity);
    Ok(updated)
}

/// Validate a signed firmware image file (Setup wizard upload gate).
///
/// Reads the file and runs [`verify_firmware_image`] with current
/// version `(0, 0, 0)` — see the module docs for why anti-downgrade is
/// the caller's job, not this command's. `signature_valid` is true only
/// when the Ed25519 check actually ran and passed (or the image passed
/// every check); a truncated, unsigned, or malformed image reports
/// `signature_valid: false` because no signature could be verified.
#[must_use]
pub fn validate_firmware_image(path: &Path) -> FirmwareValidation {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            return FirmwareValidation {
                valid: false,
                version: None,
                signature_valid: false,
                errors: vec![format!(
                    "cannot read firmware image '{}': {error}",
                    path.display()
                )],
            };
        }
    };
    validate_firmware_image_bytes(&bytes, (0, 0, 0))
}

/// Validate a firmware image file against the production key, with the
/// anti-downgrade check vs the installed board version `current`
/// (from `get_firmware_version`, the live session's `GET_CAPS`) — the
/// version-aware form of [`validate_firmware_image`]. An older image
/// is rejected; an equal version is a reinstall and is allowed,
/// matching the board rule in `ota_validate.h`. The Setup wizard and
/// the [`crate::webota::flash_firmware`] flow call this (via the
/// verifier) before any upload; the plain command above cannot know
/// the installed version and never rejects a downgrade.
#[must_use]
pub fn validate_firmware_image_against(path: &Path, current: (u8, u8, u8)) -> FirmwareValidation {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            return FirmwareValidation {
                valid: false,
                version: None,
                signature_valid: false,
                errors: vec![format!(
                    "cannot read firmware image '{}': {error}",
                    path.display()
                )],
            };
        }
    };
    validate_firmware_image_bytes(&bytes, current)
}

/// Map a failed firmware validation to the typed [`UserError`] the UI
/// shows (B6): plain words + the next step, never the raw verifier
/// string. Returns [`None`] when the image is valid (nothing to
/// report). The flash flow ([`crate::webota::flash_firmware`],
/// called from [`crate::state::AppStateCore::start_firmware_update`])
/// validates again before any upload — this is the user-facing
/// wording for the same rejection, shared so the two paths never
/// drift apart.
#[must_use]
pub fn firmware_validation_user_error(validation: &FirmwareValidation) -> Option<UserError> {
    if validation.valid {
        return None;
    }
    let detail = validation.errors.first().cloned();
    if validation.signature_valid {
        // Signature verified, so the image is authentic — the only
        // remaining rejection is a downgrade (see
        // `firmware_validation_from_result`).
        Some(UserError {
            code: "firmware_rejected".to_string(),
            title: "Firmware update refused".to_string(),
            what_happened: format!(
                "The firmware file is older than the wheel's version{} — the wheel keeps its current firmware.",
                validation
                    .version
                    .as_ref()
                    .map_or_else(String::new, |v| format!(" ({v})"))
            ),
            what_to_do: "Check for a newer firmware version on GitHub Releases.".to_string(),
            detail,
        })
    } else {
        Some(UserError {
            code: "firmware_invalid".to_string(),
            title: "Firmware image isn't valid".to_string(),
            what_happened:
                "The firmware file failed its signature check, so it can't be installed."
                    .to_string(),
            what_to_do: "Download the firmware again from GitHub Releases and try again."
                .to_string(),
            detail,
        })
    }
}

/// Shared body of [`validate_firmware_image`] and
/// [`validate_firmware_image_against`]: verify `bytes` against
/// `current` and map the outcome to [`FirmwareValidation`].
fn validate_firmware_image_bytes(bytes: &[u8], current: (u8, u8, u8)) -> FirmwareValidation {
    firmware_validation_from_result(verify_firmware_image(bytes, current))
}

/// Map a verifier outcome to [`FirmwareValidation`]. On a downgrade
/// the image version is still reported (the signature verified, so
/// the version is authentic) — the UI can show both versions instead
/// of just "rejected".
fn firmware_validation_from_result(
    result: Result<FirmwareImageInfo, FirmwareVerifyError>,
) -> FirmwareValidation {
    match result {
        Ok(info) => FirmwareValidation {
            valid: true,
            version: Some(format!(
                "{}.{}.{}",
                info.version.0, info.version.1, info.version.2
            )),
            signature_valid: true,
            errors: Vec::new(),
        },
        Err(error) => {
            // The signature runs before the version scan, so only these
            // two failures imply a verified signature.
            let signature_valid = matches!(
                error,
                FirmwareVerifyError::VersionNotFound | FirmwareVerifyError::Downgrade { .. }
            );
            let version = if let FirmwareVerifyError::Downgrade { image, .. } = error {
                Some(format!("{}.{}.{}", image.0, image.1, image.2))
            } else {
                None
            };
            let mut message = error.to_string();
            if let FirmwareVerifyError::Downgrade { image, current } = error {
                let _ = write!(
                    message,
                    " (image {}.{}.{}, current {}.{}.{} — the caller must \
                     compare against the installed version)",
                    image.0, image.1, image.2, current.0, current.1, current.2
                );
            }
            FirmwareValidation {
                valid: false,
                version,
                signature_valid,
                errors: vec![message],
            }
        }
    }
}

/* ------------------------------------------------------------------ */
/* Snapshot API (E5 contract gap) — see `crate::snapshot`.            */
/* ------------------------------------------------------------------ */

/// Map [`EngineMetrics`] to the frontend `EngineStats` contract
/// (`get_engine_stats` command logic).
///
/// `loop_hz` comes from [`crate::snapshot::LoopRateTracker::rate_hz`] —
/// pass `None` while the tracker has not measured a rate yet. µs→ms,
/// `None` p99→`null`, loss fraction→percent.
#[must_use]
pub fn get_engine_stats(metrics: &EngineMetrics, loop_hz: Option<f64>) -> EngineStats {
    crate::snapshot::build_engine_stats(metrics, loop_hz)
}

/// Build one 30 Hz snapshot payload (`get_snapshot` command logic, and
/// the payload the Tauri shell emits on `rsh://snapshot`).
#[must_use]
pub fn get_snapshot(input: SnapshotInput) -> EngineSnapshot {
    crate::snapshot::build_snapshot(input)
}

/* ------------------------------------------------------------------ */
/* Tuning contract extensions (frontend `TuningSettings`).            */
/* ------------------------------------------------------------------ */

/// Moved to `rsh_api::tuning` (L4): the `AppApi` trait must name
/// these types, and `rsh-app-core` already depends on `rsh-api`,
/// so the API crate owns them. Re-exported here so the Tauri
/// command logic keeps its existing paths.
pub use rsh_api::tuning::{LedPattern, ShifterGate, TuningSettings, validate_tuning};

/* Button bindings — Rust side of `ButtonBindings`.                    */
/*                                                                     */
/* The shape is owned by `frontend/src/screens/Buttons/bindings.ts`:  */
/* `Record<number, string>` (button number → action id), read/written */
/* through the config staging flow (`config.buttons`). There is no     */
/* dedicated button-binding command; these helpers parse and validate  */
/* that map so the Rust side never invents a binding.                  */
/* ------------------------------------------------------------------ */

/// Actions a button can trigger (mirrors `ASSIGNABLE_ACTIONS` in
/// `frontend/src/screens/Buttons/bindings.ts`).
pub const ASSIGNABLE_ACTIONS: &[&str] = &[
    "horn",
    "shifter_up",
    "shifter_down",
    "gear_1",
    "gear_2",
    "gear_3",
    "gear_4",
    "gear_5",
    "gear_6",
    "handbrake",
];

/// Button number → action id. Absent = unbound. Serializes as a JSON
/// object with string keys (`{"1": "horn"}`), exactly the shape the
/// frontend's `ButtonBindings` parses.
pub type ButtonBindings = BTreeMap<u8, String>;

/// Parse `config.buttons` defensively, mirroring the frontend's
/// `parseBindings`: only integer keys (≥ 1) with non-empty string values
/// count; anything else is ignored. A corrupt entry never invents a
/// binding or crashes the caller. Range/action checks are
/// [`validate_button_bindings`]'s job, not this parser's.
#[must_use]
pub fn parse_button_bindings(config: &serde_json::Value) -> ButtonBindings {
    let mut bindings: ButtonBindings = BTreeMap::new();
    let Some(raw) = config.get("buttons") else {
        return bindings;
    };
    let Some(map) = raw.as_object() else {
        return bindings;
    };
    for (key, value) in map {
        let Ok(button) = key.parse::<u8>() else {
            continue;
        };
        if button < 1 {
            continue;
        }
        let Some(action) = value.as_str() else {
            continue;
        };
        if action.is_empty() {
            continue;
        }
        bindings.insert(button, action.to_string());
    }
    bindings
}

/// Validate parsed bindings: buttons must be physical (1–32) or encoder
/// virtual buttons (65–72), and actions must be assignable. Buttons
/// 33–64 have no binding UI (and no C# precedent) and are rejected.
/// Returns one message per failed rule (empty = valid).
#[must_use]
pub fn validate_button_bindings(bindings: &ButtonBindings) -> Vec<String> {
    let mut errors = Vec::new();
    for (&button, action) in bindings {
        if !((1..=32).contains(&button) || (65..=72).contains(&button)) {
            errors.push(format!(
                "button {button} cannot be bound (physical buttons are 1-32, encoder virtual buttons 65-72)"
            ));
        }
        if !ASSIGNABLE_ACTIONS.contains(&action.as_str()) {
            errors.push(format!("button {button} has unknown action '{action}'"));
        }
    }
    errors
}

/// Build a `config` JSON value with one binding replaced (or removed
/// when `action` is `None`), mirroring the frontend's `withBinding`.
/// The input config is left untouched.
#[must_use]
pub fn with_button_binding(
    config: &serde_json::Value,
    button: u8,
    action: Option<&str>,
) -> serde_json::Value {
    let mut bindings = parse_button_bindings(config);
    match action {
        Some(action) => {
            bindings.insert(button, action.to_string());
        }
        None => {
            bindings.remove(&button);
        }
    }
    let mut next = config.clone();
    let as_strings: serde_json::Map<String, serde_json::Value> = bindings
        .iter()
        .map(|(&button, action)| {
            (
                button.to_string(),
                serde_json::Value::String(action.clone()),
            )
        })
        .collect();
    if let Some(object) = next.as_object_mut() {
        object.insert("buttons".to_string(), serde_json::Value::Object(as_strings));
    }
    next
}

/* ------------------------------------------------------------------ */
/* Output backend state.                                               */
/* ------------------------------------------------------------------ */

/// Output backend state, exactly as the backend reports it (never
/// hardcoded). Mirrors the frontend `OutputState`: `enabled` is
/// `None` until the backend reports it.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OutputState {
    /// Backend name exactly as the backend reports it.
    pub backend: String,
    /// Whether the backend is connected.
    pub connected: bool,
    /// Human-readable detail (e.g. device name or last error).
    pub detail: String,
    /// Whether output is enabled; absent until the backend reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

/// One output backend as the Output page lists it (G4): the picker
/// shows every backend with honest per-backend status — never a fake
/// "installed" or "connected". `installed` is `None` only when the
/// probe itself is unavailable on this platform; both probes below run
/// on Linux and Windows.
// Five independent status flags, one per fact the picker shows —
// splitting them into nested structs would obscure the flat
// frontend contract.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BackendStatus {
    /// Short id: `"hidmaestro"` / `"vjoy"` / `"none"`.
    pub id: String,
    /// Display name exactly as the backend reports it.
    pub name: String,
    /// True for the default backend (`HIDMaestro`).
    pub is_default: bool,
    /// True when this backend is the one the user selected in
    /// settings (may differ from `active` after a fallback).
    pub selected: bool,
    /// True when this backend is the live installed backend.
    pub active: bool,
    /// True when the live backend is connected.
    pub connected: bool,
    /// Whether the backend's driver/sidecar file was found by the
    /// probe (`Some(false)` = probed, not present).
    pub installed: Option<bool>,
    /// True when the backend needs an elevated helper for the session
    /// (`HIDMaestro` only).
    pub elevation_required: bool,
    /// The UAC copy, when elevation is required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uac_note: Option<String>,
    /// Human-readable status line (shown verbatim).
    pub detail: String,
    /// Install/guide link, when the backend is missing (vJoy only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_url: Option<String>,
}

/// Build the `get_output_state` result from the backend's current
/// values. This is the state constructor the Tauri wrapper calls with
/// the live backend state — it never invents a backend name or a
/// connection state.
#[must_use]
pub fn get_output_state(
    backend: &str,
    connected: bool,
    detail: &str,
    enabled: Option<bool>,
) -> OutputState {
    OutputState {
        backend: backend.to_string(),
        connected,
        detail: detail.to_string(),
        enabled,
    }
}

/// Apply the `set_output_state` enable/disable toggle to a state
/// snapshot: returns the state with `enabled` set. The actual
/// enable/disable actuation (starting/stopping the output backend)
/// lives in the output backend / `AppState` — the Tauri wrapper applies
/// this transition there and drives the backend; this function is the
/// pure, testable state transition.
#[must_use]
pub fn set_output_state(enabled: bool, current: &OutputState) -> OutputState {
    let mut next = current.clone();
    next.enabled = Some(enabled);
    next
}

/* ------------------------------------------------------------------ */
/* Backups + firewall.                                                 */
/* ------------------------------------------------------------------ */

/// The 3-generation backup list for `profiles.json` and
/// `settings.json` in `data_dir` (M5: six rows — three generations ×
/// two files) for the `list_backups` command. Never fails on a
/// corrupt `profiles.json`: a missing generation reports
/// `present: false`. Re-exported from `rsh_profiles::store` — this
/// wrapper exists so the command surface is one import
/// (`rsh_app_core::commands::*`).
#[must_use]
pub fn list_backups(data_dir: &Path) -> Vec<BackupInfo> {
    list_backups_in(data_dir)
}

/// Windows Firewall rule state (`get_firewall_state` command logic).
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FirewallState {
    /// True when an "RSH" inbound firewall rule exists.
    pub rule_present: bool,
    /// Human-readable detail (rule name / why the check is unavailable).
    pub detail: String,
}

/// Probe the Windows Firewall for the "RSH" rule (Windows only).
///
/// Windows implementation: read-only `netsh advfirewall firewall show
/// rule name=RSH`. NOTE (SW-R7.4 / A11): the verified path for managing
/// the rule is the Windows Firewall API, not `netsh` — this probe only
/// *reads* rule presence, and creating/repairing the rule is a later
/// spike item. A `netsh` failure is reported honestly in `detail`,
/// never as a fake "rule present".
///
/// M8: presence is decided by the exit code + the rule name in the
/// output — never by the English string `"No rules match"`, which a
/// localized Windows never prints (a German install used to report a
/// missing rule as present).
#[cfg(windows)]
#[must_use]
pub fn get_firewall_state() -> FirewallState {
    use std::process::Command;
    let output = Command::new("netsh")
        .args(["advfirewall", "firewall", "show", "rule", "name=RSH"])
        .output();
    match output {
        Ok(output) => {
            let text = String::from_utf8_lossy(&output.stdout);
            parse_netsh_rule_output(output.status.success(), &text)
        }
        Err(error) => FirewallState {
            rule_present: false,
            detail: format!("could not run the firewall rule check: {error}"),
        },
    }
}

/// Parses `netsh advfirewall firewall show rule name=RSH` output
/// (M8, pure — testable on any platform).
///
/// `netsh` exits 0 and prints the rule block (`Rule Name: RSH`) when
/// the rule exists, and exits non-zero with a localized "no rules
/// match" message when it does not. The decision uses only the exit
/// code and the rule name — no locale-dependent string.
#[must_use]
pub fn parse_netsh_rule_output(success: bool, text: &str) -> FirewallState {
    // The rule block prints the rule's name; the no-match message
    // never contains it — and neither string depends on the Windows
    // display language (even `Rule Name:` itself is localized).
    let names_rule = text.contains("RSH");
    if success && names_rule {
        FirewallState {
            rule_present: true,
            detail: "firewall rule \"RSH\" exists".to_string(),
        }
    } else if !success {
        FirewallState {
            rule_present: false,
            detail: "no firewall rule named \"RSH\"".to_string(),
        }
    } else {
        FirewallState {
            rule_present: false,
            detail: "firewall rule check returned an unexpected result (exit 0, no rule block)"
                .to_string(),
        }
    }
}

/// Non-Windows builds cannot query the Windows Firewall: report the
/// check as unavailable instead of inventing a rule state.
#[cfg(not(windows))]
#[must_use]
pub fn get_firewall_state() -> FirewallState {
    FirewallState {
        rule_present: false,
        detail:
            "firewall check is Windows-only (netsh advfirewall); not available on this platform"
                .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn assignment(function: &str, pin: u8) -> PinAssignment {
        PinAssignment {
            function: function.to_string(),
            pin,
            pins: Vec::new(),
        }
    }

    fn multi_assignment(function: &str, pins: &[u8]) -> PinAssignment {
        PinAssignment {
            function: function.to_string(),
            pin: 0xFF,
            pins: pins.to_vec(),
        }
    }

    /// The factory pin map, as the Settings editor would send it.
    fn factory_assignments() -> Vec<PinAssignment> {
        vec![
            assignment("steering", 34),
            assignment("accelerator", 35),
            assignment("brake", 36),
            assignment("clutch", 32),
            assignment("horn", 27),
            assignment("buttons", 33),
            assignment("led", 21),
        ]
    }

    #[test]
    fn factory_pin_map_is_valid() {
        let result = validate_pin_map(&factory_assignments());
        assert!(result.valid, "errors: {:?}", result.errors);
    }

    #[test]
    fn duplicate_pin_is_rejected() {
        let mut pins = factory_assignments();
        pins.push(assignment("horn", 34));
        let result = validate_pin_map(&pins);
        assert!(!result.valid);
        assert!(
            result.errors.iter().any(|e| e.contains("already used")),
            "errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn flash_pin_is_rejected() {
        let result = validate_pin_map(&[assignment("steering", 6)]);
        assert!(!result.valid);
        assert!(
            result.errors.iter().any(|e| e.contains("forbidden")),
            "errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn adc2_axis_pin_is_rejected() {
        let result = validate_pin_map(&[assignment("steering", 25)]);
        assert!(!result.valid);
        assert!(
            result.errors.iter().any(|e| e.contains("ADC1")),
            "errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn shifter_is_an_error_not_a_silent_skip() {
        let result = validate_pin_map(&[assignment("shifter", 21)]);
        assert!(!result.valid);
        assert!(
            result.errors.iter().any(|e| e.contains("6 gear pins")),
            "errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn encoders_is_an_error_not_a_silent_skip() {
        let result = validate_pin_map(&[assignment("encoders", 23)]);
        assert!(!result.valid);
        assert!(
            result.errors.iter().any(|e| e.contains("2 pins")),
            "errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn unknown_function_is_an_error() {
        let result = validate_pin_map(&[assignment("warp-drive", 4)]);
        assert!(!result.valid);
        assert!(
            result.errors.iter().any(|e| e.contains("unknown function")),
            "errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn pin_out_of_range_is_an_error() {
        let result = validate_pin_map(&[assignment("steering", 40)]);
        assert!(!result.valid);
        assert!(
            result.errors.iter().any(|e| e.contains("out of range")),
            "errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn empty_map_is_invalid() {
        let result = validate_pin_map(&[]);
        assert!(!result.valid);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.contains("at least one input module")),
            "errors: {:?}",
            result.errors
        );
    }

    /// Build a signed test image with a throwaway keypair (never the
    /// production key): body + "Version <b>a.b.c" marker, signed over
    /// SHA-256(body), 64 B signature + "RSHS" trailer.
    fn signed_test_image(version: &str, body_len: usize) -> (Vec<u8>, [u8; 32]) {
        use ed25519_dalek::{Signer, SigningKey};
        use sha2::{Digest, Sha256};

        let mut body = vec![0xE9u8; body_len];
        let marker = format!("Version <b>{version}");
        body[100..100 + marker.len()].copy_from_slice(marker.as_bytes());
        let digest: [u8; 32] = Sha256::digest(&body).into();
        let key = SigningKey::from_bytes(&[0x42; 32]);
        let signature = key.sign(&digest);
        let mut image = body;
        image.extend_from_slice(&signature.to_bytes());
        image.extend_from_slice(b"RSHS");
        (image, key.verifying_key().to_bytes())
    }

    #[test]
    fn firmware_image_throwaway_key_fails_signature() {
        // The app-core command verifies against the PRODUCTION key, so a
        // throwaway-signed image must fail with an invalid signature —
        // and the failure must name the signature, not the format.
        let (image, _pubkey) = signed_test_image("4.0.2", 70_000);
        let dir = std::env::temp_dir().join(format!("rsh-app-core-fw-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("firmware-signed.bin");
        {
            let mut file = std::fs::File::create(&path).expect("write test image");
            file.write_all(&image).expect("write test image");
        }
        let result = validate_firmware_image(&path);
        assert!(!result.valid);
        assert!(!result.signature_valid);
        assert!(
            result.errors.iter().any(|e| e.contains("signature")),
            "errors: {:?}",
            result.errors
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn firmware_image_missing_file_is_an_error() {
        let result = validate_firmware_image(Path::new("/definitely/not/here/firmware.bin"));
        assert!(!result.valid);
        assert!(!result.signature_valid);
        assert_eq!(result.version, None);
        assert!(
            result.errors.iter().any(|e| e.contains("cannot read")),
            "errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn firmware_image_truncated_reports_unsigned() {
        let dir = std::env::temp_dir().join(format!("rsh-app-core-fw-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("truncated.bin");
        std::fs::write(&path, [0xE9u8; 100]).expect("write test image");
        let result = validate_firmware_image(&path);
        assert!(!result.valid);
        assert!(!result.signature_valid);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn firmware_image_against_rejects_throwaway_key() {
        // The version-aware form still verifies against the PRODUCTION
        // key, so a throwaway-signed image fails the signature — the
        // installed version never even matters here.
        let (image, _pubkey) = signed_test_image("4.0.2", 70_000);
        let dir = std::env::temp_dir().join(format!("rsh-app-core-fw-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("against-throwaway.bin");
        std::fs::write(&path, &image).expect("write test image");
        let result = validate_firmware_image_against(&path, (4, 0, 1));
        assert!(!result.valid);
        assert!(!result.signature_valid);
        assert!(
            result.errors.iter().any(|e| e.contains("signature")),
            "errors: {:?}",
            result.errors
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn firmware_image_against_missing_file_is_an_error() {
        let result = validate_firmware_image_against(
            Path::new("/definitely/not/here/firmware.bin"),
            (4, 0, 1),
        );
        assert!(!result.valid);
        assert!(!result.signature_valid);
        assert_eq!(result.version, None);
    }

    #[test]
    fn firmware_validation_maps_downgrade_with_version() {
        // The downgrade mapping is the one outcome no file-based test
        // can reach (it needs a production-signed older image), so the
        // mapper is tested directly: version reported, signature valid.
        let result = firmware_validation_from_result(Err(FirmwareVerifyError::Downgrade {
            image: (4, 0, 0),
            current: (4, 0, 1),
        }));
        assert!(!result.valid);
        assert!(result.signature_valid);
        assert_eq!(result.version.as_deref(), Some("4.0.0"));
        assert!(
            result.errors.iter().any(|e| e.contains("downgrade")),
            "errors: {:?}",
            result.errors
        );
        assert!(
            result.errors.iter().any(|e| e.contains("4.0.1")),
            "errors name the installed version: {:?}",
            result.errors
        );
    }

    #[test]
    fn firmware_validation_maps_a_valid_image() {
        let result = firmware_validation_from_result(Ok(FirmwareImageInfo {
            version: (4, 0, 2),
            image_size: 70_000,
            total_size: 70_068,
            image_sha256: [0u8; 32],
        }));
        assert!(result.valid);
        assert!(result.signature_valid);
        assert_eq!(result.version.as_deref(), Some("4.0.2"));
        assert_eq!(result.errors, Vec::<String>::new());
    }

    #[test]
    fn firmware_validation_user_error_is_none_when_valid() {
        let valid = firmware_validation_from_result(Ok(FirmwareImageInfo {
            version: (4, 0, 2),
            image_size: 70_000,
            total_size: 70_068,
            image_sha256: [0u8; 32],
        }));
        assert!(firmware_validation_user_error(&valid).is_none());
    }

    #[test]
    fn firmware_validation_user_error_names_downgrade_plainly() {
        // B6: a downgrade is a typed error with plain words + the next
        // step — never the raw verifier string.
        let rejected = firmware_validation_from_result(Err(FirmwareVerifyError::Downgrade {
            image: (4, 0, 0),
            current: (4, 0, 1),
        }));
        let err = firmware_validation_user_error(&rejected).expect("a UserError");
        assert_eq!(err.code, "firmware_rejected");
        assert!(err.what_happened.contains("4.0.0"));
        assert!(err.what_to_do.contains("GitHub Releases"));
    }

    #[test]
    fn firmware_validation_user_error_names_bad_signature_plainly() {
        let invalid = validate_firmware_image(Path::new("/definitely/not/here/firmware.bin"));
        let err = firmware_validation_user_error(&invalid).expect("a UserError");
        assert_eq!(err.code, "firmware_invalid");
        assert!(err.what_happened.contains("signature"));
        assert!(err.what_to_do.contains("Download"));
    }

    #[test]
    fn shifter_with_six_pins_is_valid() {
        let mut pins = factory_assignments();
        pins.push(multi_assignment("shifter", &[5, 18, 19, 23, 4, 15]));
        let result = validate_pin_map(&pins);
        assert!(result.valid, "errors: {:?}", result.errors);
    }

    #[test]
    fn shifter_with_wrong_pin_count_is_an_error() {
        let result = validate_pin_map(&[multi_assignment("shifter", &[5, 18, 19])]);
        assert!(!result.valid);
        assert!(
            result.errors.iter().any(|e| e.contains("6 gear pins")),
            "errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn encoders_with_two_pairs_are_valid() {
        let mut pins = factory_assignments();
        pins.push(multi_assignment("encoders", &[23, 19, 16, 17]));
        let result = validate_pin_map(&pins);
        assert!(result.valid, "errors: {:?}", result.errors);
    }

    #[test]
    fn encoders_with_odd_pin_count_is_an_error() {
        let result = validate_pin_map(&[multi_assignment("encoders", &[23, 19, 16])]);
        assert!(!result.valid);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.contains("2 pins per encoder")),
            "errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn button_matrix_is_rejected_as_unsupported() {
        // P1-3 (update4): BUTTON_MATRIX has no firmware consumer — COMMIT
        // rejects it fail-closed (same shape as the expander reject), so a
        // matrix pin map must not validate.
        let mut pins = factory_assignments();
        pins.push(multi_assignment(
            "button_matrix",
            &[4, 13, 14, 16, 17, 18, 19, 23],
        ));
        let result = validate_pin_map(&pins);
        assert!(!result.valid);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.contains("button_matrix") && e.contains("not supported")),
            "errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn duplicate_pin_inside_multi_assignment_is_an_error() {
        let result = validate_pin_map(&[multi_assignment("shifter", &[5, 5, 19, 23, 4, 15])]);
        assert!(!result.valid);
        assert!(
            result.errors.iter().any(|e| e.contains("used twice")),
            "errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn single_pin_function_with_two_pins_is_an_error() {
        let result = validate_pin_map(&[multi_assignment("steering", &[34, 35])]);
        assert!(!result.valid);
        assert!(
            result.errors.iter().any(|e| e.contains("exactly 1 pin")),
            "errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn pin_assignment_pins_field_round_trips() {
        let assignment = multi_assignment("encoders", &[23, 19]);
        let json = serde_json::to_string(&assignment).expect("serialize");
        assert!(json.contains("\"pins\":[23,19]"), "{json}");
        let back: PinAssignment = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, assignment);
        // The legacy single-pin shape (no `pins` key) still parses.
        let legacy: PinAssignment =
            serde_json::from_str(r#"{"function":"steering","pin":34}"#).expect("legacy parse");
        assert_eq!(legacy.pins, Vec::<u8>::new());
        assert_eq!(legacy.pin, 34);
    }

    #[test]
    fn engine_stats_command_maps_metrics() {
        use rsh_engine::metrics::EngineMetrics;
        let mut metrics = EngineMetrics::new();
        metrics.record_inter_arrival_jitter(3400);
        metrics.record_output_latency(1200);
        metrics.record_e2e_latency(4800);
        for _ in 0..99 {
            metrics.record_packet(true);
        }
        metrics.record_packet(false);
        let stats = get_engine_stats(&metrics, Some(199.5));
        assert_eq!(stats.loop_hz, Some(199.5));
        assert_eq!(stats.loss_pct, 1.0);
        assert_eq!(stats.jitter_ms_p99, Some(3.4));
        assert_eq!(stats.output_latency_ms, Some(1.2));
        assert_eq!(stats.e2e_ms, Some(4.8));
        // Unmeasured metrics stay null.
        let empty = get_engine_stats(&EngineMetrics::new(), None);
        assert_eq!(empty.loop_hz, None);
        assert_eq!(empty.jitter_ms_p99, None);
        assert_eq!(empty.loss_pct, 0.0);
    }

    #[test]
    fn snapshot_command_builds_payload_with_encoders() {
        use crate::snapshot::{ConnectionState, EncoderSnapshot, PedalSnapshot, SnapshotInput};
        let stats = get_engine_stats(&rsh_engine::metrics::EngineMetrics::new(), Some(200.0));
        let snapshot = get_snapshot(SnapshotInput {
            steering_deg: -45.0,
            pedals: PedalSnapshot {
                accel: 0.8,
                brake: 0.0,
                clutch: None,
            },
            connection: ConnectionState::Connected,
            stats,
            active_profile_id: Some("default".to_string()),
            seq: 7,
            encoders: vec![EncoderSnapshot {
                index: 0,
                count: 5,
                delta: 5,
                cw_button: 65,
                ccw_button: 66,
                cw_pressed: true,
                ccw_pressed: false,
            }],
            seq_gap: 0,
            tx_fail: 0,
            handbrake: 0.0,
            gear: 0,
        });
        assert_eq!(snapshot.encoder_buttons, vec![65]);
        assert_eq!(snapshot.encoders.len(), 1);
        let json = serde_json::to_string(&snapshot).expect("serialize");
        assert!(json.contains("\"steeringDeg\":-45.0"), "{json}");
        assert!(json.contains("\"loopHz\":200.0"), "{json}");
    }

    #[test]
    fn button_bindings_parse_is_tolerant_and_validation_is_strict() {
        let config = serde_json::json!({
            "buttons": {"1": "horn", "5": "shifter_up", "65": "gear_1", "bogus": "horn", "7": 42, "9": "", "40": "horn", "3": "dance"}
        });
        let bindings = parse_button_bindings(&config);
        // Corrupt entries (non-integer key, non-string value, empty
        // string) are dropped; out-of-range / unknown-action entries
        // parse but fail validation.
        assert_eq!(bindings.len(), 5);
        assert_eq!(bindings.get(&1).map(String::as_str), Some("horn"));
        assert_eq!(bindings.get(&65).map(String::as_str), Some("gear_1"));
        let errors = validate_button_bindings(&bindings);
        assert_eq!(errors.len(), 2, "errors: {errors:?}");
        assert!(errors.iter().any(|e| e.contains("button 40")));
        assert!(errors.iter().any(|e| e.contains("button 3")));
        assert!(parse_button_bindings(&serde_json::json!({})).is_empty());
        assert!(parse_button_bindings(&serde_json::json!({"buttons": "horn"})).is_empty());
    }

    #[test]
    fn with_button_binding_adds_and_removes() {
        let config = serde_json::json!({"buttons": {"1": "horn"}});
        let added = with_button_binding(&config, 5, Some("shifter_up"));
        let bindings = parse_button_bindings(&added);
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings.get(&5).map(String::as_str), Some("shifter_up"));
        let removed = with_button_binding(&added, 1, None);
        let bindings = parse_button_bindings(&removed);
        assert_eq!(bindings.len(), 1);
        assert!(!bindings.contains_key(&1));
        // The input config is untouched.
        assert_eq!(parse_button_bindings(&config).len(), 1);
    }

    #[test]
    fn output_state_get_and_set_enabled() {
        let state = get_output_state("vJoy", true, "vJoy device 1", None);
        assert_eq!(state.enabled, None);
        let json = serde_json::to_string(&state).expect("serialize");
        assert!(!json.contains("enabled"), "{json}");
        let enabled = set_output_state(true, &state);
        assert_eq!(enabled.enabled, Some(true));
        assert_eq!(enabled.backend, "vJoy");
        assert!(enabled.connected);
        let disabled = set_output_state(false, &enabled);
        assert_eq!(disabled.enabled, Some(false));
        let json = serde_json::to_string(&disabled).expect("serialize");
        assert!(json.contains("\"enabled\":false"), "{json}");
    }

    #[test]
    fn list_backups_reports_missing_generations() {
        let dir =
            std::env::temp_dir().join(format!("rsh-app-core-backups-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let backups = list_backups(&dir);
        // M5: 3 generations per file — profiles first, then settings.
        assert_eq!(backups.len(), 6);
        assert!(backups.iter().all(|b| !b.present));
        assert_eq!(backups[0].generation, "current");
        assert_eq!(backups[0].file, "profiles.json");
        assert_eq!(backups[3].file, "settings.json");
        std::fs::write(dir.join("profiles.json"), "{}").expect("write profiles");
        let backups = list_backups(&dir);
        assert!(backups[0].present);
        assert!(backups[0].saved_at.is_some());
        assert!(!backups[1].present);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn firewall_parser_uses_exit_code_not_locale() {
        // M8: a localized "no rules" message (German here) must not
        // read as "rule present" — the old code matched the English
        // string "No rules match".
        let missing_de = parse_netsh_rule_output(
            false,
            "Es gibt keine Regeln, die den angegebenen Kriterien entsprechen.\n",
        );
        assert!(!missing_de.rule_present);
        let present = parse_netsh_rule_output(
            true,
            "Rule Name:                            RSH\nEnabled:                              Yes\n",
        );
        assert!(present.rule_present);
        let unexpected = parse_netsh_rule_output(true, "\n");
        assert!(!unexpected.rule_present);
        assert!(
            unexpected.detail.contains("unexpected"),
            "{}",
            unexpected.detail
        );
    }

    #[test]
    #[cfg(not(windows))]
    fn firewall_state_is_honest_on_non_windows() {
        let state = get_firewall_state();
        assert!(!state.rule_present);
        assert!(
            state.detail.contains("Windows-only"),
            "detail: {}",
            state.detail
        );
    }

    /* H5-control: role-based pin conversion + config JSON. */

    #[test]
    fn role_records_use_protocol_slots_not_list_position() {
        // Handbrake submitted WITHOUT a clutch must still land in
        // AXIS_ADC slot 4 (the running counter of validate_pin_map
        // would put it in slot 3 — the clutch — for validation only).
        let records = pin_assignments_to_role_records(&[
            assignment("steering", 34),
            assignment("accelerator", 35),
            assignment("brake", 36),
            assignment("handbrake", 32),
            assignment("horn", 4),
            assignment("led", 21),
        ])
        .expect("role conversion succeeds");
        assert_eq!(records.len(), 6);
        let find = |rec_type: u8, slot: u8| {
            records
                .iter()
                .find(|r| r.rec_type == rec_type && r.slot == slot)
                .map(|r| r.pin_a)
        };
        assert_eq!(find(record_type::AXIS_ADC, 0), Some(34));
        assert_eq!(find(record_type::AXIS_ADC, 1), Some(35));
        assert_eq!(find(record_type::AXIS_ADC, 2), Some(36));
        assert_eq!(find(record_type::AXIS_ADC, 3), None, "no clutch record");
        assert_eq!(
            find(record_type::AXIS_ADC, 4),
            Some(32),
            "handbrake is slot 4"
        );
        assert_eq!(
            find(record_type::BUTTON, 2),
            Some(4),
            "horn is BUTTON slot 2"
        );
        assert_eq!(find(record_type::LED_STRIP, 0), Some(21));
    }

    #[test]
    fn role_records_reject_generic_buttons_and_unknown_functions() {
        let err = pin_assignments_to_role_records(&[assignment("buttons", 5)])
            .expect_err("generic 'buttons' must be rejected");
        assert!(err.contains("no board pin"), "{err}");
        let err = pin_assignments_to_role_records(&[assignment("warp-drive", 4)])
            .expect_err("unknown function must be rejected");
        assert!(err.contains("unknown function"), "{err}");
    }

    #[test]
    fn role_records_multi_pin_functions() {
        let records = pin_assignments_to_role_records(&[
            multi_assignment("shifter", &[10, 11, 12, 13, 14, 15]),
            multi_assignment("encoders", &[23, 19, 16, 17]),
        ])
        .expect("multi-pin conversion succeeds");
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].rec_type, record_type::SHIFTER);
        assert_eq!(records[0].pin_a, 10);
        assert_eq!(records[1].rec_type, record_type::ENCODER);
        assert_eq!(records[1].slot, 0);
        assert_eq!(records[2].slot, 1);
    }

    #[test]
    fn records_to_pin_assignments_reads_the_factory_blob() {
        let blob = rsh_protocol::factory_default_blob();
        let pins = records_to_pin_assignments(&blob.records);
        // BUTTON slots 0/1 (shifter up/down) and the battery record are
        // deliberately absent — the editor has no function for them.
        assert_eq!(
            pins,
            vec![
                assignment("steering", 34),
                assignment("accelerator", 35),
                assignment("brake", 36),
                assignment("handbrake", 32),
                assignment("horn", 4),
                assignment("led", 21),
            ]
        );
    }

    #[test]
    fn merge_pin_map_preserves_unexpressible_records() {
        let live = rsh_protocol::factory_default_blob();
        // Full editor map, no clutch: handbrake must stay slot 4, the
        // shifter up/down BUTTON records and the battery must survive.
        let merged = merge_pin_map_into_blob(
            &live,
            &[
                assignment("steering", 34),
                assignment("accelerator", 35),
                assignment("brake", 36),
                assignment("handbrake", 32),
                assignment("horn", 4),
                assignment("led", 21),
            ],
        )
        .expect("merge succeeds");
        let find = |rec_type: u8, slot: u8| {
            merged
                .records
                .iter()
                .find(|r| r.rec_type == rec_type && r.slot == slot)
                .map(|r| r.pin_a)
        };
        assert_eq!(find(record_type::AXIS_ADC, 3), None, "no clutch added");
        assert_eq!(find(record_type::AXIS_ADC, 4), Some(32));
        assert_eq!(find(record_type::BUTTON, 0), Some(27), "shifter-up kept");
        assert_eq!(find(record_type::BUTTON, 1), Some(33), "shifter-down kept");
        assert_eq!(find(record_type::BUTTON, 2), Some(4));
        assert_eq!(find(record_type::BATTERY, 0), Some(39), "battery kept");
        assert_eq!(find(record_type::ENCODER, 0), Some(23), "encoders kept");
        assert_eq!(find(record_type::ENCODER, 1), Some(16));
        assert_eq!(merged.cfg_version, live.cfg_version);
    }

    #[test]
    fn merge_pin_map_omission_removes_the_function() {
        let live = rsh_protocol::factory_default_blob();
        // Horn omitted: its BUTTON slot-2 record goes away, slots 0/1 stay.
        let merged = merge_pin_map_into_blob(
            &live,
            &[
                assignment("steering", 34),
                assignment("accelerator", 35),
                assignment("brake", 36),
                assignment("handbrake", 32),
                assignment("led", 21),
            ],
        )
        .expect("merge succeeds");
        assert!(
            merged
                .records
                .iter()
                .all(|r| !(r.rec_type == record_type::BUTTON && r.slot == 2)),
            "horn record removed"
        );
        assert!(
            merged
                .records
                .iter()
                .any(|r| r.rec_type == record_type::BUTTON && r.slot == 0),
            "shifter-up kept"
        );
    }

    #[test]
    fn merge_pin_map_rejects_a_pin_that_dupes_a_preserved_record() {
        let live = rsh_protocol::factory_default_blob();
        // Steering onto pin 27 duplicates the preserved BUTTON slot-0 pin.
        let err = merge_pin_map_into_blob(&live, &[assignment("steering", 27)])
            .expect_err("duplicate with a preserved record must fail");
        assert!(err.contains("already used"), "{err}");
    }

    #[test]
    fn config_json_round_trips_the_factory_blob() {
        let blob = rsh_protocol::factory_default_blob();
        let json = config_blob_to_json(&blob);
        assert_eq!(json["cfgVersion"], 1);
        assert_eq!(
            json["buttons"],
            serde_json::json!({"1": "shifter_up", "2": "shifter_down", "3": "horn"})
        );
        let back = config_json_to_blob(&json).expect("round-trip succeeds");
        assert_eq!(back, blob);
    }

    #[test]
    fn shifter_sensitivity_config_helpers_round_trip() {
        // The factory blob has no SHIFTER record: no sensitivity
        // to read, and the write refuses instead of inventing one.
        let factory = config_blob_to_json(&rsh_protocol::factory_default_blob());
        assert_eq!(shifter_sensitivity_from_config(&factory), None);
        let err = set_shifter_sensitivity_in_config(&factory, 70)
            .expect_err("no SHIFTER record must refuse");
        assert!(err.contains("no shifter is configured"), "{err}");
        // With a sequential SHIFTER record (params[4] = 50), the
        // read returns 50 and the write changes only params[4].
        let mut blob = rsh_protocol::factory_default_blob();
        let mut shifter = ConfigRecord::new(record_type::SHIFTER, 0, 25, 26, 14);
        shifter.params[0] = 0; // sequential
        shifter.params[4] = 50;
        blob.records.push(shifter);
        let json = config_blob_to_json(&blob);
        assert_eq!(shifter_sensitivity_from_config(&json), Some(50));
        let updated = set_shifter_sensitivity_in_config(&json, 70).expect("write 70");
        assert_eq!(shifter_sensitivity_from_config(&updated), Some(70));
        // Everything else is verbatim: same version, same records
        // except the one params entry, same buttons view.
        assert_eq!(updated["cfgVersion"], json["cfgVersion"]);
        assert_eq!(updated["buttons"], json["buttons"]);
        assert_eq!(
            updated["records"].as_array().map(Vec::len),
            json["records"].as_array().map(Vec::len)
        );
        for (before, after) in json["records"]
            .as_array()
            .expect("records")
            .iter()
            .zip(updated["records"].as_array().expect("records").iter())
        {
            if before["type"] == record_type::SHIFTER {
                assert_eq!(after["params"][4], 70);
                assert_eq!(after["params"][0], before["params"][0]);
            } else {
                assert_eq!(after, before);
            }
        }
        // Out-of-range values are rejected, never clamped.
        let err = set_shifter_sensitivity_in_config(&json, 101).expect_err("101 must be rejected");
        assert!(err.contains("outside 0..=100"), "{err}");
        // The mutated JSON still rebuilds to a valid blob.
        let rebuilt = config_json_to_blob(&updated).expect("mutated config rebuilds");
        assert_eq!(rsh_protocol::pin::shifter_sensitivity(&rebuilt), 70);
    }

    #[test]
    fn config_json_rejects_a_cross_role_rebind() {
        let blob = rsh_protocol::factory_default_blob();
        let mut json = config_blob_to_json(&blob);
        json["buttons"]["1"] = serde_json::Value::String("horn".to_string());
        let err = config_json_to_blob(&json).expect_err("button 1 -> horn must be rejected");
        assert!(err.contains("cannot be re-roled"), "{err}");
    }

    #[test]
    fn config_json_unbind_removes_the_button_record() {
        let blob = rsh_protocol::factory_default_blob();
        let mut json = config_blob_to_json(&blob);
        let buttons = json["buttons"].as_object_mut().expect("buttons object");
        buttons.remove("3");
        let back = config_json_to_blob(&json).expect("unbind succeeds");
        assert!(
            back.records
                .iter()
                .all(|r| !(r.rec_type == record_type::BUTTON && r.slot == 2)),
            "horn record removed"
        );
        assert!(
            back.records
                .iter()
                .any(|r| r.rec_type == record_type::BUTTON && r.slot == 0),
            "shifter-up kept"
        );
    }

    #[test]
    fn config_json_rejects_buttons_beyond_the_board_slots() {
        let blob = rsh_protocol::factory_default_blob();
        let mut json = config_blob_to_json(&blob);
        json["buttons"]["5"] = serde_json::Value::String("shifter_up".to_string());
        let err = config_json_to_blob(&json).expect_err("button 5 has no board slot");
        assert!(err.contains("button 5"), "{err}");
        let mut json = config_blob_to_json(&blob);
        json["buttons"]["65"] = serde_json::Value::String("gear_1".to_string());
        let err = config_json_to_blob(&json).expect_err("encoder button 65 has no board slot");
        assert!(err.contains("button 65"), "{err}");
    }

    #[test]
    fn config_json_without_buttons_keeps_records_verbatim() {
        let blob = rsh_protocol::factory_default_blob();
        let mut json = config_blob_to_json(&blob);
        json.as_object_mut()
            .expect("config object")
            .remove("buttons");
        let back = config_json_to_blob(&json).expect("no-buttons succeeds");
        assert_eq!(back, blob);
    }

    #[test]
    fn config_json_rejects_a_missing_version_and_a_bad_record() {
        let blob = rsh_protocol::factory_default_blob();
        let mut json = config_blob_to_json(&blob);
        json.as_object_mut()
            .expect("config object")
            .remove("cfgVersion");
        let err = config_json_to_blob(&json).expect_err("missing cfgVersion must fail");
        assert!(err.contains("cfgVersion"), "{err}");
        let mut json = config_blob_to_json(&blob);
        json["records"][0]["pinA"] = serde_json::json!(300);
        let err = config_json_to_blob(&json).expect_err("pin 300 must fail");
        assert!(err.contains("pinA"), "{err}");
    }
}
