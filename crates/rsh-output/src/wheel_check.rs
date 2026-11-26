//! Wheel self-check (FH4 fix batch, v2.0.14).
//!
//! The app must be able to tell us — without the user running
//! diagnostics — which piece of the wheel setup is missing on his
//! PC. Forza Horizon 4 still shows no Wheel tab after v2.0.13 even
//! though the backend is vJoy and the PC was restarted, so "no
//! restart" and "wrong backend" are eliminated; what is left is
//! (a) the installer writes never landed or are incomplete,
//! (b) the vJoy device-1 envelope never landed, or (c) FH4 demands
//! a profile match beyond the registry classification. This check
//! measures (a) and (b) directly, so its result distinguishes (c):
//! if every item here is green and FH4 still shows no tab, the
//! mapping + envelope are proven present on the PC and the cause is
//! the profile match — not a missing write.
//!
//! # The complete mapping
//!
//! v2.0.13 wrote only the *required* `RacingWheel` subkeys
//! (Description, `MaxWheelAngle`, Wheel/Throttle/Brake/Handbrake,
//! PreviousGear/NextGear). That is the full required set per the
//! Learn doc — but Microsoft's own example registry file
//! ("Registry data for game controllers", "Example registry file")
//! writes the whole tree, and the doc says Labels "should be
//! present". The research team's #1 suspect is that GameInput/FH4
//! ignores a mapping without the complete tree, so the mapping is
//! now the full tree as far as RSH's layout truthfully allows:
//!
//! * `Labels\Buttons` — the example's generic-wheel labels
//!   (Button0=LetterA, Button1=LetterB, Button2=LetterX,
//!   Button3=LetterY, Button6=Menu, Button7=View,
//!   Button8=RightStickButton, Button9=LeftStickButton). Partial
//!   labelling is the documented norm; these are the example's
//!   positions for a generic wheel, not physical prints on RSH's
//!   buttons.
//! * `Labels\Switches` — `Switch0Up`/ Right/Down/Left = Up/Right/
//!   Down/Left (our 1 discrete POV is switch index 0, exactly as
//!   in the example).
//! * `RacingWheel\DPadUp/Down/Left/Right` — `SwitchIndex` 0,
//!   `IncludeAdjacent` 1, `SwitchPosition` Up/Down/Left/Right.
//! * `RacingWheel\Button1` (index 0, shift-up) and `Button2`
//!   (index 1, shift-down) — the only `ButtonN` entries with a
//!   fixed, firing assignment. Button3–8 are deliberately NOT
//!   mapped: raw indices 2–3 are pipeline-stripped pedal buttons
//!   and 5–10 are stripped H-pattern gear bits — dead buttons a
//!   mapping would point at.
//! * Gears (`FirstGear`..`SixthGear`, `ReverseGear`) are NOT
//!   mapped either: the pipeline strips gear bits 5–10 from the
//!   button word, so no raw button ever fires for a gear.
//! * `RacingWheel\Clutch` — `AxisIndex` 3 (Rx). The vJoy feed now
//!   writes the clutch to Rx as well as to the V3 `wClutch` field
//!   (see `vjoy.rs`), so the mapping is truthful; the HID profile
//!   already fed clutch on Rx.
//!
//! The check verifies the COMPLETE tree: `vjoy_mapping_ok` means
//! every entry below is present with the expected value, and
//! [`first_mismatch`] names the missing piece for `next_step`.
//!
//! # What is measured
//!
//! * Both `GameInput` mappings, entry by entry
//!   ([`mapping_entries`]).
//! * vJoy device-1 envelope, via the DLL: button count (expect
//!   128), all 8 axes (`GetVJDAxisExist` for X/Y/Z/Rx/Ry/Rz/
//!   Slider/Dial), discrete POV count (`GetVJDDiscPovNumber`,
//!   expect 1) and continuous POV count (`GetVJDContPovNumber`,
//!   expect 0 — `-s 1` is the discrete flag, `-p` would be
//!   continuous; verified against the upstream vJoyConfig source,
//!   `apps/vJoyConf/vJoyConfig/vJoyConfig.h`: `-s j` sets "the
//!   number of discrete POV Switches", and the two flags are
//!   mutually exclusive).
//! * FFB: the device-1 HID report descriptor (`HidReportDescriptor`
//!   `REG_BINARY`) contains the Physical Interface Device usage
//!   page (`0x05 0x0F`). This is a DIAGNOSTIC only — FH4 does not
//!   send FFB data to vJoy, so input works without it and a
//!   missing FFB section never blocks the check or the next step.
//!
//! The check is read-only and non-elevated: the `GameInput`
//! mapping keys and the vJoy `Parameters\Device01` key are
//! world-readable, the DLL is loaded only to *ask* it (never to
//! acquire a device), and nothing is written. It runs at most
//! every ~30 s from `AppStateCore::vjoy_status` (cached there),
//! plus once on backend switch, and the result is surfaced as
//! `rsh_api::VjoyStatus.wheel_check` (the `feeding` field there
//! is filled in by the state layer from the live backend — this
//! module does not own the backend slot).
//!
//! The single source of truth for the mapping is
//! [`mapping_entries`]: the installer (`installer/rsh.nsi`), the
//! app-side repair (`vjoy_config::wheel_setup_script`), the
//! self-check, and the smoke test all write / verify the same
//! entries. A test below proves the installer contains every
//! entry.

use std::collections::BTreeMap;

#[cfg(windows)]
use crate::vjoy::RealVJoyDll;
use crate::vjoy::{
    USAGE_DIAL, USAGE_RX, USAGE_RY, USAGE_RZ, USAGE_SLIDER, USAGE_X, USAGE_Y, USAGE_Z,
    VJoyInterface,
};
#[cfg(windows)]
use crate::vjoy_config::{VJOY_DEVICE1_REGISTRY_SUBKEY, VJOY_FH4_DEVICE_ID};

/// `GameInput` mapping key for the vJoy device (VID `1234` /
/// PID `BEAD`, usage page `0001`, usage `0004`).
pub const VJOY_GAMEINPUT_SUBKEY: &str =
    r"SYSTEM\CurrentControlSet\Control\GameInput\Devices\1234BEAD00010004";
/// `GameInput` mapping key for the `HIDMaestro` device (VID `1209` /
/// PID `5253`, usage page `0001`, usage `0004`).
pub const HID_GAMEINPUT_SUBKEY: &str =
    r"SYSTEM\CurrentControlSet\Control\GameInput\Devices\1209525300010004";
/// Value under [`VJOY_DEVICE1_REGISTRY_SUBKEY`] holding the HID
/// report descriptor (`REG_BINARY`).
pub const VJOY_DESCRIPTOR_VALUE: &str = "HidReportDescriptor";

/// The expected `GameInput` mapping for one device — the values
/// `installer/rsh.nsi` writes. `Invert` is 0 for every axis (RSH's
/// raw values rise with input), so it is baked into
/// [`mapping_entries`] rather than carried per axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExpectedMapping {
    /// `Description` value.
    pub description: &'static str,
    /// `RacingWheel/MaxWheelAngle` (450 = 900° lock-to-lock, one
    /// direction).
    pub max_wheel_angle: u32,
    /// Wheel axis index (X = 0).
    pub wheel_axis: u32,
    /// Throttle axis index (Y = 1).
    pub throttle_axis: u32,
    /// Brake axis index (Z = 2).
    pub brake_axis: u32,
    /// Clutch axis index (Rx = 3). [`Some`] for both devices now:
    /// the vJoy feed writes clutch to Rx as well as `wClutch`.
    pub clutch_axis: Option<u32>,
    /// Handbrake axis index (vJoy Slider = 6, HID Slider = 4).
    pub handbrake_axis: u32,
    /// `PreviousGear` button index (button 2 = index 1).
    pub previous_gear: u32,
    /// `NextGear` button index (button 1 = index 0).
    pub next_gear: u32,
}

/// Expected vJoy mapping (see `installer/rsh.nsi`,
/// `WriteGameInputVJoyMapping`).
pub const VJOY_EXPECTED_MAPPING: ExpectedMapping = ExpectedMapping {
    description: "RSH Wheel (vJoy)",
    max_wheel_angle: 450,
    wheel_axis: 0,
    throttle_axis: 1,
    brake_axis: 2,
    clutch_axis: Some(3),
    handbrake_axis: 6,
    previous_gear: 1,
    next_gear: 0,
};

/// Expected `HIDMaestro` mapping (see `installer/rsh.nsi`,
/// `WriteGameInputHidMapping`).
pub const HID_EXPECTED_MAPPING: ExpectedMapping = ExpectedMapping {
    description: "RSH Wheel (HIDMaestro)",
    max_wheel_angle: 450,
    wheel_axis: 0,
    throttle_axis: 1,
    brake_axis: 2,
    clutch_axis: Some(3),
    handbrake_axis: 4,
    previous_gear: 1,
    next_gear: 0,
};

/// `Labels\Buttons` for a generic wheel, verbatim from the Learn
/// example registry file (partial labelling is the documented
/// norm — Button4/Button5 are the Next/PreviousGear buttons and
/// carry no label in the example).
pub const LABEL_BUTTONS: &[(&str, &str)] = &[
    ("Button0", "LetterA"),
    ("Button1", "LetterB"),
    ("Button2", "LetterX"),
    ("Button3", "LetterY"),
    ("Button6", "Menu"),
    ("Button7", "View"),
    ("Button8", "RightStickButton"),
    ("Button9", "LeftStickButton"),
];

/// `Labels\Switches` for the one discrete POV (switch index 0),
/// verbatim from the Learn example.
pub const LABEL_SWITCHES: &[(&str, &str)] = &[
    ("Switch0Up", "Up"),
    ("Switch0Right", "Right"),
    ("Switch0Down", "Down"),
    ("Switch0Left", "Left"),
];

/// `RacingWheel\DPad*` switch positions (subkey, position),
/// per the Learn example (`SwitchIndex` 0, `IncludeAdjacent` 1).
pub const DPAD_POSITIONS: &[(&str, &str)] = &[
    ("DPadUp", "Up"),
    ("DPadDown", "Down"),
    ("DPadLeft", "Left"),
    ("DPadRight", "Right"),
];

/// One expected registry value in a mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MappingValue {
    /// `REG_SZ`.
    Str(&'static str),
    /// `REG_DWORD`.
    Dword(u32),
}

/// One entry of the complete mapping: the subkey relative to the
/// device key (`""` = the device key itself), the value name, and
/// the expected value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MappingEntry {
    /// Subkey relative to the device key.
    pub subkey: &'static str,
    /// Value name.
    pub name: &'static str,
    /// Expected value.
    pub value: MappingValue,
}

/// The complete mapping for one device, as a flat entry list —
/// the single source of truth (see the module docs). `Invert` is
/// 0 for every axis. `Button1`/`Button2` are the only `ButtonN`
/// entries (fixed, firing assignments — see the module docs);
/// gears are omitted on purpose.
// One table by design (the single source of truth): splitting it
// would scatter the mapping across helpers and hide drift, so the
// pedantic line-count lint is allowed here.
#[allow(clippy::too_many_lines)]
#[must_use]
pub fn mapping_entries(expected: &ExpectedMapping) -> Vec<MappingEntry> {
    let mut entries = vec![
        MappingEntry {
            subkey: "",
            name: "Description",
            value: MappingValue::Str(expected.description),
        },
        MappingEntry {
            subkey: "RacingWheel",
            name: "MaxWheelAngle",
            value: MappingValue::Dword(expected.max_wheel_angle),
        },
    ];
    for (axis, index) in [
        ("Wheel", expected.wheel_axis),
        ("Throttle", expected.throttle_axis),
        ("Brake", expected.brake_axis),
        ("Handbrake", expected.handbrake_axis),
    ] {
        entries.push(MappingEntry {
            subkey: match axis {
                "Wheel" => "RacingWheel\\Wheel",
                "Throttle" => "RacingWheel\\Throttle",
                "Brake" => "RacingWheel\\Brake",
                _ => "RacingWheel\\Handbrake",
            },
            name: "AxisIndex",
            value: MappingValue::Dword(index),
        });
        entries.push(MappingEntry {
            subkey: match axis {
                "Wheel" => "RacingWheel\\Wheel",
                "Throttle" => "RacingWheel\\Throttle",
                "Brake" => "RacingWheel\\Brake",
                _ => "RacingWheel\\Handbrake",
            },
            name: "Invert",
            value: MappingValue::Dword(0),
        });
    }
    if let Some(index) = expected.clutch_axis {
        entries.push(MappingEntry {
            subkey: "RacingWheel\\Clutch",
            name: "AxisIndex",
            value: MappingValue::Dword(index),
        });
        entries.push(MappingEntry {
            subkey: "RacingWheel\\Clutch",
            name: "Invert",
            value: MappingValue::Dword(0),
        });
    }
    for (button, index) in [
        ("PreviousGear", expected.previous_gear),
        ("NextGear", expected.next_gear),
        ("Button1", 0),
        ("Button2", 1),
    ] {
        entries.push(MappingEntry {
            subkey: match button {
                "PreviousGear" => "RacingWheel\\PreviousGear",
                "NextGear" => "RacingWheel\\NextGear",
                "Button1" => "RacingWheel\\Button1",
                _ => "RacingWheel\\Button2",
            },
            name: "ButtonIndex",
            value: MappingValue::Dword(index),
        });
    }
    for (dpad, position) in DPAD_POSITIONS {
        let subkey = match *dpad {
            "DPadUp" => "RacingWheel\\DPadUp",
            "DPadDown" => "RacingWheel\\DPadDown",
            "DPadLeft" => "RacingWheel\\DPadLeft",
            _ => "RacingWheel\\DPadRight",
        };
        entries.push(MappingEntry {
            subkey,
            name: "SwitchIndex",
            value: MappingValue::Dword(0),
        });
        entries.push(MappingEntry {
            subkey,
            name: "IncludeAdjacent",
            value: MappingValue::Dword(1),
        });
        entries.push(MappingEntry {
            subkey,
            name: "SwitchPosition",
            value: MappingValue::Str(position),
        });
    }
    for (name, label) in LABEL_BUTTONS {
        entries.push(MappingEntry {
            subkey: "Labels\\Buttons",
            name,
            value: MappingValue::Str(label),
        });
    }
    for (name, label) in LABEL_SWITCHES {
        entries.push(MappingEntry {
            subkey: "Labels\\Switches",
            name,
            value: MappingValue::Str(label),
        });
    }
    entries
}

/// A registry value as read back: [`None`] per key when the
/// subkey / value is missing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegValue {
    /// `REG_SZ`.
    Str(String),
    /// `REG_DWORD`.
    Dword(u32),
}

/// Whether every expected entry is present in `actual` with the
/// expected value (pure — `actual` is keyed by
/// `(subkey-relative-to-the-device-key, value-name)`).
#[must_use]
pub fn entries_match(
    entries: &[MappingEntry],
    actual: &BTreeMap<(String, String), RegValue>,
) -> bool {
    first_mismatch(entries, actual).is_none()
}

/// The first expected entry that is missing or wrong in `actual`,
/// as a `subkey\name` path for `next_step` (pure). [`None`] when
/// the mapping is complete.
#[must_use]
pub fn first_mismatch(
    entries: &[MappingEntry],
    actual: &BTreeMap<(String, String), RegValue>,
) -> Option<String> {
    for entry in entries {
        let key = (entry.subkey.to_string(), entry.name.to_string());
        let found = match actual.get(&key) {
            Some(RegValue::Str(found)) => match entry.value {
                MappingValue::Str(expected) => found == expected,
                MappingValue::Dword(_) => false,
            },
            Some(RegValue::Dword(found)) => match entry.value {
                MappingValue::Dword(expected) => *found == expected,
                MappingValue::Str(_) => false,
            },
            None => false,
        };
        if !found {
            if entry.subkey.is_empty() {
                return Some(entry.name.to_string());
            }
            return Some(format!("{}\\{}", entry.subkey, entry.name));
        }
    }
    None
}

/// Whether a HID report descriptor carries the FFB section: the
/// Physical Interface Device usage page (`0x05 0x0F`, usage page
/// `0x0F` = PID) anywhere in the blob (pure — a byte scan, no
/// descriptor parsing; the two-byte prefix is the standard
/// short-item encoding of a usage page).
#[must_use]
pub fn descriptor_has_ffb(descriptor: &[u8]) -> bool {
    descriptor.windows(2).any(|pair| pair == [0x05, 0x0F])
}

/// Whether the live device-1 shape is the `EmuWheel` envelope
/// (pure): 128 buttons, all 8 axes, exactly one discrete POV hat
/// and no continuous POV. FFB is deliberately NOT part of this:
/// it is a diagnostic only (FH4 does not send FFB to vJoy).
#[must_use]
pub fn envelope_shape_ok(
    button_count: i32,
    all_eight_axes: bool,
    discrete_pov_count: i32,
    continuous_pov_count: i32,
) -> bool {
    button_count == 128 && all_eight_axes && discrete_pov_count == 1 && continuous_pov_count == 0
}

/// The plain-words next step for a check result (pure). Priority
/// is the order a user must fix things in: no vJoy at all first,
/// then the vJoy mapping (naming the missing piece), then the
/// envelope, then the HID mapping. `""` when everything is OK.
/// `vjoy_present` is false when the DLL could not be loaded or
/// the driver is disabled — then the button count is `-1` and the
/// envelope cannot be judged, so the step names vJoy itself.
/// Missing FFB never produces a step (diagnostic only).
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn next_step_for(
    vjoy_mapping_issue: Option<&str>,
    hid_mapping_issue: Option<&str>,
    envelope_ok: bool,
    button_count: i32,
    discrete_pov_count: i32,
    continuous_pov_count: i32,
    vjoy_present: bool,
) -> String {
    if !vjoy_present {
        return "vJoy is not installed or not enabled. Install vJoy, then re-run the RSH installer so device 1 is set up as a wheel.".to_string();
    }
    if let Some(piece) = vjoy_mapping_issue {
        return format!(
            "GameInput mapping incomplete — {piece} missing or wrong. Re-run the RSH installer."
        );
    }
    if !envelope_ok {
        if button_count != 128 {
            return format!(
                "vJoy device 1 has {button_count} buttons instead of 128. Re-run the RSH installer to set it up as a wheel."
            );
        }
        if discrete_pov_count != 1 {
            return format!(
                "vJoy device 1 has {discrete_pov_count} discrete POV hats instead of 1. Re-run the RSH installer to set it up as a wheel."
            );
        }
        if continuous_pov_count != 0 {
            return "vJoy device 1 has a continuous POV hat instead of a discrete one. Re-run the RSH installer to set it up as a wheel.".to_string();
        }
        return "vJoy device 1 is not set up as a wheel (8 axes, 128 buttons, 1 discrete POV hat, all force-feedback effects). Re-run the RSH installer.".to_string();
    }
    if let Some(piece) = hid_mapping_issue {
        return format!(
            "GameInput mapping incomplete for the HID wheel — {piece} missing or wrong. Re-run the RSH installer."
        );
    }
    String::new()
}

/// Result of one [`run_wheel_check`] — the same fields as
/// `rsh_api::WheelCheck` except `feeding`, which the state layer
/// fills in from the live backend (this crate does not own the
/// backend slot).
// Five bools because the check IS five pass/fail items, mirroring
// the API struct field by field; grouping them would break the
// mapping, so the pedantic bool-count lint is allowed here.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WheelCheckResult {
    /// vJoy `GameInput` mapping complete with expected values.
    pub vjoy_mapping_ok: bool,
    /// HID `GameInput` mapping complete with expected values.
    pub hid_mapping_ok: bool,
    /// vJoy device 1 has the `EmuWheel` envelope.
    pub envelope_ok: bool,
    /// vJoy device-1 button count as reported (`-1` when unknown).
    pub button_count: i32,
    /// FFB section present in the device-1 descriptor
    /// (diagnostic only — never blocks the check).
    pub ffb_present: bool,
    /// Plain-words next step (`""` when everything is OK).
    pub next_step: String,
}

/// The live envelope shape read from a vJoy DLL handle: button
/// count, whether all 8 envelope axes exist, and the POV counts.
/// No device is acquired — these are read-only queries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EnvelopeRead {
    /// Button count as reported.
    pub button_count: i32,
    /// All 8 envelope axes present (X/Y/Z/Rx/Ry/Rz/Slider/Dial).
    pub all_eight_axes: bool,
    /// Discrete (4-direction) POV hat count as reported.
    pub discrete_pov_count: i32,
    /// Continuous POV hat count as reported.
    pub continuous_pov_count: i32,
}

/// Read the live envelope shape through a [`VJoyInterface`]
/// (pure over the trait — tests use a fake DLL, production passes
/// the real one).
#[must_use]
pub fn check_envelope(dll: &dyn VJoyInterface, device_id: u32) -> EnvelopeRead {
    let all_eight_axes = [
        USAGE_X,
        USAGE_Y,
        USAGE_Z,
        USAGE_RX,
        USAGE_RY,
        USAGE_RZ,
        USAGE_SLIDER,
        USAGE_DIAL,
    ]
    .iter()
    .all(|usage| dll.get_vjd_axis_exist(device_id, *usage));
    EnvelopeRead {
        button_count: dll.get_vjd_button_number(device_id),
        all_eight_axes,
        discrete_pov_count: dll.get_vjd_disc_pov_number(device_id),
        continuous_pov_count: dll.get_vjd_cont_pov_number(device_id),
    }
}

/// Whether the vJoy device-1 descriptor carries FFB
/// ([`descriptor_has_ffb`]). [`None`] when the descriptor could
/// not be read (non-Windows, vJoy not installed, key missing) —
/// "not read" is not "no FFB".
#[must_use]
pub fn device1_ffb_present() -> Option<bool> {
    #[cfg(windows)]
    {
        read_descriptor_bytes().map(|bytes| descriptor_has_ffb(&bytes))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Run the whole wheel check (Windows: registry + DLL reads;
/// elsewhere: everything unknown). Read-only, non-elevated, no
/// device acquired — safe to run from the ~1 Hz status poll
/// (cached by the caller).
#[must_use]
pub fn run_wheel_check() -> WheelCheckResult {
    #[cfg(windows)]
    {
        let vjoy_entries = mapping_entries(&VJOY_EXPECTED_MAPPING);
        let hid_entries = mapping_entries(&HID_EXPECTED_MAPPING);
        let vjoy_values = read_mapping_values(VJOY_GAMEINPUT_SUBKEY, &vjoy_entries);
        let hid_values = read_mapping_values(HID_GAMEINPUT_SUBKEY, &hid_entries);
        let vjoy_issue = first_mismatch(&vjoy_entries, &vjoy_values);
        let hid_issue = first_mismatch(&hid_entries, &hid_values);
        let ffb_present = device1_ffb_present().unwrap_or(false);
        let (button_count, envelope_axes, disc_pov_count, cont_pov_count, vjoy_present) =
            read_envelope_via_dll();
        let envelope_ok =
            envelope_shape_ok(button_count, envelope_axes, disc_pov_count, cont_pov_count);
        let next_step = next_step_for(
            vjoy_issue.as_deref(),
            hid_issue.as_deref(),
            envelope_ok,
            button_count,
            disc_pov_count,
            cont_pov_count,
            vjoy_present,
        );
        WheelCheckResult {
            vjoy_mapping_ok: vjoy_issue.is_none(),
            hid_mapping_ok: hid_issue.is_none(),
            envelope_ok,
            button_count,
            ffb_present,
            next_step,
        }
    }
    #[cfg(not(windows))]
    {
        WheelCheckResult {
            vjoy_mapping_ok: false,
            hid_mapping_ok: false,
            envelope_ok: false,
            button_count: -1,
            ffb_present: false,
            next_step: "Wheel check needs Windows.".to_string(),
        }
    }
}

/// The envelope shape via the real DLL: `(button_count,
/// all_eight_axes, discrete_pov_count, continuous_pov_count,
/// vjoy_present)`. A missing DLL or a disabled driver reports
/// "not present" with `button_count` `-1` — the caller must not
/// mistake that for a measured 0-button device.
#[cfg(windows)]
fn read_envelope_via_dll() -> (i32, bool, i32, i32, bool) {
    let Ok(dll) = RealVJoyDll::load() else {
        return (-1, false, -1, -1, false);
    };
    if !dll.vjoy_enabled() {
        return (-1, false, -1, -1, false);
    }
    let read = check_envelope(&dll, VJOY_FH4_DEVICE_ID);
    (
        read.button_count,
        read.all_eight_axes,
        read.discrete_pov_count,
        read.continuous_pov_count,
        true,
    )
}

/// Read every value of a mapping from the registry (Windows-only):
/// each missing key / value is simply absent from the map, which
/// never matches — a half-written mapping fails the check instead
/// of passing on the fields that happen to be there.
#[cfg(windows)]
fn read_mapping_values(
    device_subkey: &str,
    entries: &[MappingEntry],
) -> BTreeMap<(String, String), RegValue> {
    let mut values = BTreeMap::new();
    for entry in entries {
        let full_subkey = if entry.subkey.is_empty() {
            device_subkey.to_string()
        } else {
            format!("{device_subkey}\\{}", entry.subkey)
        };
        let read = match entry.value {
            MappingValue::Str(_) => read_string(&full_subkey, entry.name).map(RegValue::Str),
            MappingValue::Dword(_) => read_dword(&full_subkey, entry.name).map(RegValue::Dword),
        };
        if let Some(value) = read {
            values.insert((entry.subkey.to_string(), entry.name.to_string()), value);
        }
    }
    values
}

/// The device-1 HID report descriptor bytes (Windows-only).
#[cfg(windows)]
fn read_descriptor_bytes() -> Option<Vec<u8>> {
    read_binary(VJOY_DEVICE1_REGISTRY_SUBKEY, VJOY_DESCRIPTOR_VALUE)
}

// ---------------------------------------------------------------------
// Registry reads (Windows-only; same `windows`-crate pattern as
// `registry_dll_path` in `vjoy.rs` — KEY_READ, never a write).
// ---------------------------------------------------------------------

#[cfg(windows)]
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Open a HKLM subkey for reading.
#[cfg(windows)]
fn open_key(subkey: &str) -> Option<windows::Win32::System::Registry::HKEY> {
    use windows::Win32::System::Registry::{HKEY, HKEY_LOCAL_MACHINE, KEY_READ, RegOpenKeyExW};
    use windows::core::PCWSTR;
    let subkey_w = wide(subkey);
    let mut key = HKEY::default();
    // SAFETY: `RegOpenKeyExW` with a valid NUL-terminated wide
    // string; `key` is closed by the caller on every path that
    // opened it.
    let opened = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR::from_raw(subkey_w.as_ptr()),
            Some(0),
            KEY_READ,
            &raw mut key,
        )
    };
    if opened.0 != 0 {
        return None;
    }
    Some(key)
}

/// Read one value's raw bytes + type from an open key.
#[cfg(windows)]
fn query_value(
    key: windows::Win32::System::Registry::HKEY,
    value_name: &str,
) -> Option<(windows::Win32::System::Registry::REG_VALUE_TYPE, Vec<u8>)> {
    use windows::Win32::System::Registry::{REG_VALUE_TYPE, RegQueryValueExW};
    use windows::core::PCWSTR;
    let name_w = wide(value_name);
    let mut byte_len: u32 = 0;
    let mut value_type = REG_VALUE_TYPE::default();
    // SAFETY: size probe — `lpData` is null, so only `byte_len`
    // (and the type) are written.
    let probed = unsafe {
        RegQueryValueExW(
            key,
            PCWSTR::from_raw(name_w.as_ptr()),
            None,
            Some(&raw mut value_type),
            None,
            Some(&raw mut byte_len),
        )
    };
    if probed.0 != 0 || byte_len == 0 {
        return None;
    }
    let mut buf = vec![0u8; usize::try_from(byte_len).unwrap_or(0)];
    // SAFETY: `key` is open; `buf` is a live byte buffer of the
    // declared byte length; the call only writes into it.
    let queried = unsafe {
        RegQueryValueExW(
            key,
            PCWSTR::from_raw(name_w.as_ptr()),
            None,
            Some(&raw mut value_type),
            Some(buf.as_mut_ptr()),
            Some(&raw mut byte_len),
        )
    };
    if queried.0 != 0 {
        return None;
    }
    buf.truncate(usize::try_from(byte_len).unwrap_or(0));
    Some((value_type, buf))
}

/// Read a `REG_DWORD` value as `u32` (Windows-only).
#[cfg(windows)]
fn read_dword(subkey: &str, value_name: &str) -> Option<u32> {
    use windows::Win32::System::Registry::{REG_DWORD, RegCloseKey};
    let key = open_key(subkey)?;
    let result = query_value(key, value_name).and_then(|(value_type, bytes)| {
        if value_type != REG_DWORD || bytes.len() < 4 {
            return None;
        }
        Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    });
    // SAFETY: closing an open key.
    let _ = unsafe { RegCloseKey(key) };
    result
}

/// Read a `REG_SZ` value as `String` (Windows-only).
#[cfg(windows)]
fn read_string(subkey: &str, value_name: &str) -> Option<String> {
    use windows::Win32::System::Registry::{REG_SZ, RegCloseKey};
    let key = open_key(subkey)?;
    let result = query_value(key, value_name).and_then(|(value_type, bytes)| {
        if value_type != REG_SZ || bytes.len() % 2 != 0 {
            return None;
        }
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        let nul = units.iter().position(|c| *c == 0).unwrap_or(units.len());
        String::from_utf16(&units[..nul]).ok()
    });
    // SAFETY: closing an open key.
    let _ = unsafe { RegCloseKey(key) };
    result
}

/// Read a `REG_BINARY` value as bytes (Windows-only).
#[cfg(windows)]
fn read_binary(subkey: &str, value_name: &str) -> Option<Vec<u8>> {
    use windows::Win32::System::Registry::{REG_BINARY, RegCloseKey};
    let key = open_key(subkey)?;
    let result = query_value(key, value_name).and_then(|(value_type, bytes)| {
        if value_type != REG_BINARY {
            return None;
        }
        Some(bytes)
    });
    // SAFETY: closing an open key.
    let _ = unsafe { RegCloseKey(key) };
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vjoy::{JoystickPosition, VjdStatus};

    /// A complete vJoy mapping read, as the registry would return
    /// it — every entry of [`mapping_entries`] present.
    fn complete_vjoy_values() -> BTreeMap<(String, String), RegValue> {
        let mut values = BTreeMap::new();
        for entry in mapping_entries(&VJOY_EXPECTED_MAPPING) {
            let value = match entry.value {
                MappingValue::Str(text) => RegValue::Str(text.to_string()),
                MappingValue::Dword(number) => RegValue::Dword(number),
            };
            values.insert((entry.subkey.to_string(), entry.name.to_string()), value);
        }
        values
    }

    #[test]
    fn mapping_entries_cover_the_complete_tree() {
        let vjoy = mapping_entries(&VJOY_EXPECTED_MAPPING);
        let hid = mapping_entries(&HID_EXPECTED_MAPPING);
        // 1 Description + 1 MaxWheelAngle + 5 axes × 2 (Index+Invert)
        // + 4 buttons + 4 DPad × 3 + 8 button labels + 4 switch labels.
        assert_eq!(vjoy.len(), 40, "vJoy entries: {vjoy:?}");
        assert_eq!(hid.len(), 40, "HID entries: {hid:?}");
        // Both devices map Clutch to Rx (axis 3) now.
        let clutch = vjoy
            .iter()
            .find(|e| e.subkey == "RacingWheel\\Clutch" && e.name == "AxisIndex")
            .expect("vJoy Clutch entry");
        assert_eq!(clutch.value, MappingValue::Dword(3));
        // Handbrake differs: vJoy Slider 6, HID Slider 4.
        let handbrake = |entries: &[MappingEntry]| {
            entries
                .iter()
                .find(|e| e.subkey == "RacingWheel\\Handbrake" && e.name == "AxisIndex")
                .expect("Handbrake entry")
                .value
        };
        assert_eq!(handbrake(&vjoy), MappingValue::Dword(6));
        assert_eq!(handbrake(&hid), MappingValue::Dword(4));
        // Gears are deliberately NOT mapped (dead buttons).
        // PreviousGear/NextGear ARE present — they are the shifter
        // buttons, not gear buttons, so the check is by subkey
        // suffix, not by the "Gear" substring.
        assert!(
            !vjoy.iter().any(|e| [
                "FirstGear",
                "SecondGear",
                "ThirdGear",
                "FourthGear",
                "FifthGear",
                "SixthGear",
                "SeventhGear",
                "ReverseGear",
            ]
            .iter()
            .any(|gear| e.subkey.ends_with(gear))),
            "no gear entries"
        );
        // Button3–8 are deliberately NOT mapped either.
        for n in 3..=8 {
            assert!(
                !vjoy
                    .iter()
                    .any(|e| e.subkey.ends_with(&format!("Button{n}"))),
                "Button{n} must not be mapped"
            );
        }
        // Labels follow the Learn example exactly.
        let label = |entries: &[MappingEntry], name: &str| {
            entries
                .iter()
                .find(|e| e.subkey == "Labels\\Buttons" && e.name == name)
                .expect("label entry")
                .value
        };
        assert_eq!(label(&vjoy, "Button0"), MappingValue::Str("LetterA"));
        assert_eq!(label(&vjoy, "Button6"), MappingValue::Str("Menu"));
        assert_eq!(
            label(&vjoy, "Button9"),
            MappingValue::Str("LeftStickButton")
        );
    }

    /// Value-by-value check against the Microsoft Learn example
    /// registry file ("Registry data for game controllers",
    /// device 1234567800010004). The example is a *different*
    /// device, so only its device-independent values must match
    /// exactly — this test fails if any of them drifts. Its
    /// device-specific values differ by design and are asserted
    /// separately, with the reason:
    /// * `Invert`: the example inverts Throttle/Brake/Clutch (1)
    ///   because ITS hardware reads that way; RSH pedals feed
    ///   increasing-with-press (1 released → 32768 pressed), so
    ///   RSH inverts nothing (0 everywhere).
    /// * `PreviousGear`/`NextGear`: the example's shifter is raw
    ///   buttons 5/4; RSH's is raw buttons 1/0 (shift-down/up).
    /// * `Handbrake`: the example has NO Handbrake subkey; RSH
    ///   adds one (vJoy Slider 6, HID Slider 4) from its own
    ///   layout. `Description` is per-device ("Example Wheel
    ///   Device" vs ours).
    // Value-by-value table test (the whole Learn example in one
    // place): splitting it would hide the table, so the pedantic
    // line-count lint is allowed here.
    #[allow(clippy::too_many_lines)]
    #[test]
    fn mapping_entries_match_learn_example_value_by_value() {
        let entries = mapping_entries(&VJOY_EXPECTED_MAPPING);
        let find = |subkey: &str, name: &str| {
            entries
                .iter()
                .find(|e| e.subkey == subkey && e.name == name)
                .unwrap_or_else(|| panic!("missing entry {subkey}\\{name}"))
                .value
        };
        // Device-independent values, copied from the Learn example.
        let learn: &[(&str, &str, MappingValue)] = &[
            ("RacingWheel", "MaxWheelAngle", MappingValue::Dword(450)),
            ("RacingWheel\\Wheel", "AxisIndex", MappingValue::Dword(0)),
            ("RacingWheel\\Throttle", "AxisIndex", MappingValue::Dword(1)),
            ("RacingWheel\\Brake", "AxisIndex", MappingValue::Dword(2)),
            ("RacingWheel\\Clutch", "AxisIndex", MappingValue::Dword(3)),
            (
                "RacingWheel\\Button1",
                "ButtonIndex",
                MappingValue::Dword(0),
            ),
            (
                "RacingWheel\\Button2",
                "ButtonIndex",
                MappingValue::Dword(1),
            ),
            ("RacingWheel\\DPadUp", "SwitchIndex", MappingValue::Dword(0)),
            (
                "RacingWheel\\DPadUp",
                "IncludeAdjacent",
                MappingValue::Dword(1),
            ),
            (
                "RacingWheel\\DPadUp",
                "SwitchPosition",
                MappingValue::Str("Up"),
            ),
            (
                "RacingWheel\\DPadDown",
                "SwitchIndex",
                MappingValue::Dword(0),
            ),
            (
                "RacingWheel\\DPadDown",
                "IncludeAdjacent",
                MappingValue::Dword(1),
            ),
            (
                "RacingWheel\\DPadDown",
                "SwitchPosition",
                MappingValue::Str("Down"),
            ),
            (
                "RacingWheel\\DPadLeft",
                "SwitchIndex",
                MappingValue::Dword(0),
            ),
            (
                "RacingWheel\\DPadLeft",
                "IncludeAdjacent",
                MappingValue::Dword(1),
            ),
            (
                "RacingWheel\\DPadLeft",
                "SwitchPosition",
                MappingValue::Str("Left"),
            ),
            (
                "RacingWheel\\DPadRight",
                "SwitchIndex",
                MappingValue::Dword(0),
            ),
            (
                "RacingWheel\\DPadRight",
                "IncludeAdjacent",
                MappingValue::Dword(1),
            ),
            (
                "RacingWheel\\DPadRight",
                "SwitchPosition",
                MappingValue::Str("Right"),
            ),
            ("Labels\\Buttons", "Button0", MappingValue::Str("LetterA")),
            ("Labels\\Buttons", "Button1", MappingValue::Str("LetterB")),
            ("Labels\\Buttons", "Button2", MappingValue::Str("LetterX")),
            ("Labels\\Buttons", "Button3", MappingValue::Str("LetterY")),
            ("Labels\\Buttons", "Button6", MappingValue::Str("Menu")),
            ("Labels\\Buttons", "Button7", MappingValue::Str("View")),
            (
                "Labels\\Buttons",
                "Button8",
                MappingValue::Str("RightStickButton"),
            ),
            (
                "Labels\\Buttons",
                "Button9",
                MappingValue::Str("LeftStickButton"),
            ),
            ("Labels\\Switches", "Switch0Up", MappingValue::Str("Up")),
            (
                "Labels\\Switches",
                "Switch0Right",
                MappingValue::Str("Right"),
            ),
            ("Labels\\Switches", "Switch0Down", MappingValue::Str("Down")),
            ("Labels\\Switches", "Switch0Left", MappingValue::Str("Left")),
        ];
        for (subkey, name, value) in learn {
            assert_eq!(
                find(subkey, name),
                *value,
                "Learn example value drifted: {subkey}\\{name}"
            );
        }
        // Device-specific values: RSH's own layout, asserted with
        // the reason above.
        for axis in ["Wheel", "Throttle", "Brake", "Clutch", "Handbrake"] {
            assert_eq!(
                find(&format!("RacingWheel\\{axis}"), "Invert"),
                MappingValue::Dword(0),
                "RSH inverts no axis"
            );
        }
        assert_eq!(
            find("RacingWheel\\PreviousGear", "ButtonIndex"),
            MappingValue::Dword(1),
            "shift-down is raw button 2 (index 1)"
        );
        assert_eq!(
            find("RacingWheel\\NextGear", "ButtonIndex"),
            MappingValue::Dword(0),
            "shift-up is raw button 1 (index 0)"
        );
    }

    #[test]
    fn entries_match_when_complete_and_fail_on_any_gap() {
        let entries = mapping_entries(&VJOY_EXPECTED_MAPPING);
        let values = complete_vjoy_values();
        assert!(entries_match(&entries, &values));
        // Each category, one mutation at a time.
        let mut wrong = values.clone();
        wrong.insert(
            ("Labels\\Buttons".to_string(), "Button0".to_string()),
            RegValue::Str("LetterB".to_string()),
        );
        assert!(!entries_match(&entries, &wrong));
        let mut wrong = values.clone();
        wrong.remove(&("Labels\\Switches".to_string(), "Switch0Up".to_string()));
        assert!(!entries_match(&entries, &wrong));
        let mut wrong = values.clone();
        wrong.insert(
            (
                "RacingWheel\\DPadUp".to_string(),
                "SwitchPosition".to_string(),
            ),
            RegValue::Str("Down".to_string()),
        );
        assert!(!entries_match(&entries, &wrong));
        let mut wrong = values.clone();
        wrong.insert(
            ("RacingWheel\\Clutch".to_string(), "AxisIndex".to_string()),
            RegValue::Dword(4),
        );
        assert!(!entries_match(&entries, &wrong));
        let mut wrong = values.clone();
        wrong.remove(&(
            "RacingWheel\\Button1".to_string(),
            "ButtonIndex".to_string(),
        ));
        assert!(!entries_match(&entries, &wrong));
        assert!(
            !entries_match(&entries, &BTreeMap::new()),
            "empty never matches"
        );
    }

    #[test]
    fn first_mismatch_names_the_missing_piece() {
        let entries = mapping_entries(&VJOY_EXPECTED_MAPPING);
        assert_eq!(first_mismatch(&entries, &complete_vjoy_values()), None);
        let mut values = complete_vjoy_values();
        values.remove(&("Labels\\Buttons".to_string(), "Button0".to_string()));
        assert_eq!(
            first_mismatch(&entries, &values).as_deref(),
            Some("Labels\\Buttons\\Button0")
        );
        let mut values = complete_vjoy_values();
        values.remove(&(String::new(), "Description".to_string()));
        assert_eq!(
            first_mismatch(&entries, &values).as_deref(),
            Some("Description")
        );
    }

    #[test]
    fn descriptor_ffb_scan_finds_pid_usage_page() {
        // Real descriptor fragment: Generic Desktop usage page,
        // then the PID page opening the FFB section.
        let with_ffb: Vec<u8> = vec![0x05, 0x01, 0x09, 0x04, 0xA1, 0x01, 0x05, 0x0F, 0x09, 0x01];
        assert!(descriptor_has_ffb(&with_ffb));
        // Same shape, no PID page.
        let without_ffb: Vec<u8> = vec![0x05, 0x01, 0x09, 0x04, 0xA1, 0x01, 0x05, 0x0D, 0x09, 0x01];
        assert!(!descriptor_has_ffb(&without_ffb));
        assert!(!descriptor_has_ffb(&[]));
        // A lone 0x0F byte is not a usage page.
        assert!(!descriptor_has_ffb(&[0x0F]));
        assert!(!descriptor_has_ffb(&[0x05]));
    }

    #[test]
    fn envelope_shape_needs_buttons_axes_and_discrete_pov() {
        assert!(envelope_shape_ok(128, true, 1, 0));
        assert!(!envelope_shape_ok(32, true, 1, 0), "32 buttons");
        assert!(!envelope_shape_ok(128, false, 1, 0), "axis missing");
        assert!(!envelope_shape_ok(128, true, 0, 0), "no discrete POV");
        assert!(!envelope_shape_ok(128, true, 2, 0), "two discrete POVs");
        assert!(
            !envelope_shape_ok(128, true, 1, 1),
            "continuous POV present"
        );
        assert!(!envelope_shape_ok(-1, false, -1, -1), "unknown");
    }

    #[test]
    fn next_step_names_the_piece_and_ignores_ffb() {
        assert_eq!(
            next_step_for(None, None, true, 128, 1, 0, true),
            "",
            "everything OK"
        );
        assert!(
            next_step_for(None, None, true, 128, 1, 0, false).contains("vJoy is not installed"),
            "no vJoy names vJoy"
        );
        assert!(
            next_step_for(
                Some("Labels\\Buttons\\Button0"),
                None,
                true,
                128,
                1,
                0,
                true
            )
            .contains("Labels\\Buttons\\Button0"),
            "mapping gap names the piece"
        );
        assert!(
            next_step_for(None, None, false, 32, 1, 0, true).contains("32 buttons"),
            "wrong button count names the count"
        );
        assert!(
            next_step_for(None, None, false, 128, 0, 0, true).contains("discrete POV"),
            "missing discrete POV names it"
        );
        assert!(
            next_step_for(None, None, false, 128, 1, 1, true).contains("continuous POV"),
            "continuous POV names it"
        );
        assert!(
            next_step_for(
                None,
                Some("RacingWheel\\DPadUp\\SwitchPosition"),
                true,
                128,
                1,
                0,
                true
            )
            .contains("HID wheel"),
            "HID gap names the HID wheel"
        );
    }

    /// Fake DLL for [`check_envelope`]: scripted axes / buttons /
    /// POVs, no driver involved.
    struct FakeDll {
        axes: [bool; 8],
        button_count: i32,
        disc_pov_count: i32,
        cont_pov_count: i32,
    }

    impl VJoyInterface for FakeDll {
        fn vjoy_enabled(&self) -> bool {
            true
        }
        fn get_vjoy_version(&self) -> u16 {
            0x222
        }
        fn driver_match(&self) -> (bool, u16, u16) {
            (true, 0x222, 0x222)
        }
        fn get_vjd_status(&self, _device_id: u32) -> VjdStatus {
            VjdStatus::Free
        }
        fn acquire_vjd(&mut self, _device_id: u32) -> bool {
            true
        }
        fn relinquish_vjd(&mut self, _device_id: u32) {}
        fn reset_vjd(&mut self, _device_id: u32) -> bool {
            true
        }
        fn update_vjd(&mut self, _device_id: u32, _position: &JoystickPosition) -> bool {
            true
        }
        fn get_vjd_axis_exist(&self, _device_id: u32, usage: u16) -> bool {
            match usage {
                USAGE_X => self.axes[0],
                USAGE_Y => self.axes[1],
                USAGE_Z => self.axes[2],
                USAGE_RX => self.axes[3],
                USAGE_RY => self.axes[4],
                USAGE_RZ => self.axes[5],
                USAGE_SLIDER => self.axes[6],
                USAGE_DIAL => self.axes[7],
                _ => false,
            }
        }
        fn get_vjd_button_number(&self, _device_id: u32) -> i32 {
            self.button_count
        }
        fn get_vjd_disc_pov_number(&self, _device_id: u32) -> i32 {
            self.disc_pov_count
        }
        fn get_vjd_cont_pov_number(&self, _device_id: u32) -> i32 {
            self.cont_pov_count
        }
    }

    #[test]
    fn check_envelope_reads_axes_buttons_and_both_pov_counts() {
        let full = FakeDll {
            axes: [true; 8],
            button_count: 128,
            disc_pov_count: 1,
            cont_pov_count: 0,
        };
        let read = check_envelope(&full, 1);
        assert_eq!(
            read,
            EnvelopeRead {
                button_count: 128,
                all_eight_axes: true,
                discrete_pov_count: 1,
                continuous_pov_count: 0,
            }
        );
        assert!(envelope_shape_ok(
            read.button_count,
            read.all_eight_axes,
            read.discrete_pov_count,
            read.continuous_pov_count,
        ));
        // Default-shape device: 32 buttons, no Dial, no discrete
        // POV, one continuous POV.
        let mut default_axes = [true; 8];
        default_axes[7] = false;
        let default = FakeDll {
            axes: default_axes,
            button_count: 32,
            disc_pov_count: 0,
            cont_pov_count: 1,
        };
        let read = check_envelope(&default, 1);
        assert!(!read.all_eight_axes);
        assert_eq!(read.button_count, 32);
        assert_eq!(read.discrete_pov_count, 0);
        assert_eq!(read.continuous_pov_count, 1);
        assert!(!envelope_shape_ok(
            read.button_count,
            read.all_eight_axes,
            read.discrete_pov_count,
            read.continuous_pov_count,
        ));
    }

    #[test]
    #[cfg(not(windows))]
    fn run_wheel_check_reports_unknown_off_windows() {
        let result = run_wheel_check();
        assert!(!result.vjoy_mapping_ok);
        assert!(!result.envelope_ok);
        assert_eq!(result.button_count, -1);
        assert!(!result.ffb_present);
        assert_ne!(result.next_step, "");
    }

    #[test]
    fn device1_ffb_is_unread_off_windows() {
        #[cfg(not(windows))]
        assert_eq!(device1_ffb_present(), None);
        #[cfg(windows)]
        let _ = device1_ffb_present();
    }

    /// The installer must write exactly the entries
    /// [`mapping_entries`] defines — no drift between the Rust
    /// source of truth and `installer/rsh.nsi`. Parses the
    /// `WriteRegStr` / `WriteRegDWORD` lines for both device keys
    /// and compares them entry by entry.
    #[test]
    fn installer_writes_the_complete_mapping() {
        let nsi = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../installer/rsh.nsi");
        let text = std::fs::read_to_string(&nsi)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", nsi.display()));
        for (key, expected) in [
            ("1234BEAD00010004", VJOY_EXPECTED_MAPPING),
            ("1209525300010004", HID_EXPECTED_MAPPING),
        ] {
            let mut actual: BTreeMap<(String, String), RegValue> = BTreeMap::new();
            for line in text.lines() {
                let line = line.trim();
                let (is_str, rest) = if let Some(rest) = line.strip_prefix("WriteRegStr ") {
                    (true, rest)
                } else if let Some(rest) = line.strip_prefix("WriteRegDWORD ") {
                    (false, rest)
                } else {
                    continue;
                };
                // WriteRegStr HKLM "<full key>" "<name>" "<value>"
                // WriteRegDWORD HKLM "<full key>" "<name>" <number>
                let mut parts = rest.split('"');
                let _hkml = parts.next();
                let full_key = parts.next().unwrap_or("");
                let _gap = parts.next();
                let name = parts.next().unwrap_or("");
                // STR values are quoted (`... "Name" "Value"`), so
                // one more split part separates name from value;
                // DWORD values are unquoted (`... "Name" 450`), so
                // the remainder after the name IS the value.
                let value_text = if is_str {
                    let _gap = parts.next();
                    parts.next().unwrap_or("").trim()
                } else {
                    parts.next().unwrap_or("").trim()
                };
                let prefix =
                    format!("SYSTEM\\CurrentControlSet\\Control\\GameInput\\Devices\\{key}");
                let Some(rel) = full_key.strip_prefix(&prefix) else {
                    continue;
                };
                let rel = rel.strip_prefix('\\').unwrap_or(rel);
                let value = if is_str {
                    RegValue::Str(value_text.to_string())
                } else {
                    RegValue::Dword(value_text.parse().unwrap_or(u32::MAX))
                };
                actual.insert((rel.to_string(), name.to_string()), value);
            }
            let entries = mapping_entries(&expected);
            assert!(
                entries_match(&entries, &actual),
                "installer mapping for {key} is incomplete/drifted — first gap: {:?}",
                first_mismatch(&entries, &actual)
            );
        }
    }
}
