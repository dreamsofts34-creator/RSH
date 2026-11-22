//! Persisted button→action bindings (v2.0.2, Round 2B).
//!
//! The PC-side binding store: which wheel button triggers which
//! action. This is the store the old `config.buttons` flow could
//! never be — the board only stores three fixed `BUTTON` slot roles
//! (PROTOCOL-V2.1 §3.6: shifter-up, shifter-down, horn), so an
//! arbitrary rebind cannot be staged to the board at all (see
//! `rsh_app_core::commands::config_json_to_blob`). The bindings
//! live here, in `bindings.json` next to `profiles.json`, and the
//! daemon applies them to each engine tick before the output
//! backend sees it (see `AppStateCore::apply_bindings` in
//! `rsh-app-core`).
//!
//! Actions:
//! * Game inputs — what the output backend understands: `horn`,
//!   `shifter_up`, `shifter_down`, `gear_1`…`gear_6`, `handbrake`
//!   (the same vocabulary as
//!   `rsh_app_core::commands::ASSIGNABLE_ACTIONS` — the two lists
//!   must stay in sync).
//! * App actions — things the daemon does itself on a press edge:
//!   `profile_next` / `profile_previous` (cycle the active profile).
//!
//! Buttons are physical wheel buttons 1–32 or encoder virtual
//! buttons 65–72 — the same range
//! `rsh_app_core::commands::validate_button_bindings` accepts.
//! Buttons 33–64 have no binding UI and are rejected.
//!
//! Load policy: a missing file means no bindings; a corrupt,
//! oversize, or newer-schema file loads as *empty* with a warning
//! (see [`BindingStore::load_warnings`]) instead of failing the
//! daemon start — bindings are user preferences the Buttons screen
//! recreates in seconds, not profile data. Nothing is ever
//! silently dropped: every dropped entry leaves one warning line.
//!
//! Save policy: atomic (temp file in the same directory, then
//! rename — a crash mid-save never leaves a half-written
//! `bindings.json`). A failed save leaves the in-memory map
//! untouched and reports [`BindingError`] with the stable code
//! `BindingSaveFailed`.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// On-disk schema version for `bindings.json`.
pub const BINDINGS_SCHEMA_VERSION: u32 = 1;
/// Size cap for `bindings.json` (64 KiB — far above any real map:
/// at most 40 buttons × a short action string).
pub const MAX_BINDINGS_FILE_BYTES: u64 = 65_536;
/// File name of the binding store inside the data directory.
pub const BINDINGS_FILE_NAME: &str = "bindings.json";

/// Game-input actions a button can trigger (what the output
/// backend understands). Must stay in sync with
/// `rsh_app_core::commands::ASSIGNABLE_ACTIONS`.
pub const GAME_ACTIONS: &[&str] = &[
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

/// App actions a button can trigger (the daemon runs these itself
/// on a press edge — they never reach the output backend).
pub const APP_ACTIONS: &[&str] = &["profile_next", "profile_previous"];

/// True when `action` is a game-input action.
#[must_use]
pub fn is_game_action(action: &str) -> bool {
    GAME_ACTIONS.contains(&action)
}

/// True when `action` is an app action.
#[must_use]
pub fn is_app_action(action: &str) -> bool {
    APP_ACTIONS.contains(&action)
}

/// True when `action` is any assignable action (game or app).
#[must_use]
pub fn is_assignable_action(action: &str) -> bool {
    is_game_action(action) || is_app_action(action)
}

/// Validate a button number: physical 1–32 or encoder virtual
/// 65–72. Returns the error message, or [`None`] when valid.
#[must_use]
pub fn validate_button(button: u8) -> Option<String> {
    if (1..=32).contains(&button) || (65..=72).contains(&button) {
        None
    } else {
        Some(format!(
            "button {button} cannot be bound (physical buttons are 1-32, encoder virtual buttons 65-72)"
        ))
    }
}

/// Validate an action id. Returns the error message, or [`None`]
/// when valid.
#[must_use]
pub fn validate_action(action: &str) -> Option<String> {
    if is_assignable_action(action) {
        None
    } else {
        Some(format!("unknown action '{action}'"))
    }
}

/// A binding failure, with a stable code the API layer maps to a
/// `UserError` verbatim — the UI matches on the code, never on
/// the message text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindingError {
    /// Stable code: `InvalidButton`, `UnknownAction`, or
    /// `BindingSaveFailed`.
    pub code: &'static str,
    /// Human-readable reason (never a secret).
    pub message: String,
}

impl BindingError {
    /// Button outside 1–32 / 65–72.
    #[must_use]
    pub fn invalid_button(button: u8) -> Self {
        Self {
            code: "InvalidButton",
            message: validate_button(button)
                .unwrap_or_else(|| format!("button {button} cannot be bound")),
        }
    }

    /// Action id that is neither a game input nor an app action.
    #[must_use]
    pub fn unknown_action(action: &str) -> Self {
        Self {
            code: "UnknownAction",
            message: validate_action(action)
                .unwrap_or_else(|| format!("unknown action '{action}'")),
        }
    }

    /// The bindings file could not be written.
    #[must_use]
    pub fn save_failed(message: impl Into<String>) -> Self {
        Self {
            code: "BindingSaveFailed",
            message: message.into(),
        }
    }
}

impl std::fmt::Display for BindingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for BindingError {}

/// An empty knob map (`""` = not set everywhere). A helper (not
/// `[[String::new(); 3]; 2]`) because `String` is not `Copy`.
fn empty_knobs() -> [[String; 3]; 2] {
    std::array::from_fn(|_| [String::new(), String::new(), String::new()])
}

/// What [`BindingStore::read_file`] returns: (bindings, knobs,
/// long-press bindings, load warnings).
type BindingsFileContents = (
    BTreeMap<u8, String>,
    [[String; 3]; 2],
    BTreeMap<u8, String>,
    Vec<String>,
);

/// On-disk shape of `bindings.json` (`schemaVersion` camelCase,
/// like `profiles.json`). `knobs` are the two rotary knobs' actions
/// [knob][row: left, right, press] — `""` = not set (M9, v2.0.2).
/// `long_press_bindings` (BU2) is the per-button second action
/// that fires when the button is held past the long-press
/// threshold — physical buttons 1–32 only (the engine tracks
/// holds for stream buttons, not encoder virtual buttons).
/// Absent in old files (`#[serde(default)]`) = no long-press
/// actions, so old `bindings.json` files load unchanged.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
struct BindingsFile {
    schema_version: u32,
    bindings: BTreeMap<u8, String>,
    knobs: [[String; 3]; 2],
    long_press_bindings: BTreeMap<u8, String>,
}

/// The persisted button→action store.
pub struct BindingStore {
    /// Directory holding `bindings.json`.
    dir: PathBuf,
    /// Button → action id, in button order.
    bindings: BTreeMap<u8, String>,
    /// Knob actions [knob][row: left, right, press] (`""` = not set).
    knobs: [[String; 3]; 2],
    /// Button → long-press action id (BU2), in button order.
    /// Physical buttons 1–32 only.
    long_press_bindings: BTreeMap<u8, String>,
    /// Load-time warnings (dropped entries, corrupt file). Empty
    /// on a clean load; surfaced by the daemon as a startup notice,
    /// never silently swallowed.
    load_warnings: Vec<String>,
}

impl BindingStore {
    /// Open the store in `dir`, loading `bindings.json` when it
    /// exists. A missing file means no bindings; a corrupt,
    /// oversize, or newer-schema file loads as empty with a warning
    /// (see the module docs) — this never fails.
    #[must_use]
    pub fn open(dir: &Path) -> Self {
        let path = dir.join(BINDINGS_FILE_NAME);
        let (bindings, knobs, long_press_bindings, load_warnings) = if path.exists() {
            Self::read_file(&path)
        } else {
            (BTreeMap::new(), empty_knobs(), BTreeMap::new(), Vec::new())
        };
        Self {
            dir: dir.to_path_buf(),
            bindings,
            knobs,
            long_press_bindings,
            load_warnings,
        }
    }

    /// Load-time warnings from [`BindingStore::open`]: dropped
    /// entries, or the whole file ignored as corrupt. Empty on a
    /// clean load.
    #[must_use]
    pub fn load_warnings(&self) -> &[String] {
        &self.load_warnings
    }

    /// The directory this store reads from / writes to.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Path of the bindings file.
    #[must_use]
    pub fn bindings_path(&self) -> PathBuf {
        self.dir.join(BINDINGS_FILE_NAME)
    }

    /// All bindings, in button order.
    #[must_use]
    pub fn list(&self) -> &BTreeMap<u8, String> {
        &self.bindings
    }

    /// Knob actions [knob][row: left, right, press] (`""` = not set).
    #[must_use]
    pub fn knobs(&self) -> [[String; 3]; 2] {
        self.knobs.clone()
    }

    /// Set one knob row's action (`knob` 0/1, `row` 0 left / 1 right /
    /// 2 press, `action` an assignable id or `""` for not set) and
    /// persist. A failed save leaves the in-memory knobs untouched.
    ///
    /// # Errors
    ///
    /// Returns [`BindingError`] with code `InvalidButton` for a
    /// knob/row outside 0/1 / 0..=2, `UnknownAction` for an action
    /// id that is neither assignable nor empty, and
    /// `BindingSaveFailed` when the file cannot be written.
    pub fn set_knob(&mut self, knob: u8, row: u8, action: &str) -> Result<(), BindingError> {
        if knob > 1 || row > 2 {
            return Err(BindingError {
                code: "InvalidButton",
                message: format!("knob {knob} row {row} is out of range (knob 0-1, row 0-2)"),
            });
        }
        if !action.is_empty() && validate_action(action).is_some() {
            return Err(BindingError::unknown_action(action));
        }
        let mut next = self.knobs.clone();
        next[knob as usize][row as usize] = action.to_string();
        self.save_file(
            &self.bindings.clone(),
            &next,
            &self.long_press_bindings.clone(),
        )?;
        self.knobs = next;
        Ok(())
    }

    /// The action bound to `button`, if any.
    #[must_use]
    pub fn get(&self, button: u8) -> Option<&str> {
        self.bindings.get(&button).map(String::as_str)
    }

    /// Bind `button` to `action` and persist. Validates first; a
    /// failed save leaves the in-memory map untouched.
    ///
    /// # Errors
    ///
    /// Returns [`BindingError`] with code `InvalidButton` for a
    /// button outside 1–32 / 65–72, `UnknownAction` for an action
    /// id that is neither a game input nor an app action, and
    /// `BindingSaveFailed` when the file cannot be written.
    pub fn set(&mut self, button: u8, action: &str) -> Result<(), BindingError> {
        if let Some(message) = validate_button(button) {
            return Err(BindingError {
                code: "InvalidButton",
                message,
            });
        }
        if let Some(message) = validate_action(action) {
            return Err(BindingError {
                code: "UnknownAction",
                message,
            });
        }
        let mut next = self.bindings.clone();
        next.insert(button, action.to_string());
        self.save_file(
            &next,
            &self.knobs.clone(),
            &self.long_press_bindings.clone(),
        )?;
        self.bindings = next;
        Ok(())
    }

    /// Remove the binding for `button` and persist. Returns `true`
    /// when a binding was actually removed (`false` = nothing was
    /// bound, nothing written).
    ///
    /// # Errors
    ///
    /// Returns [`BindingError`] with code `BindingSaveFailed` when
    /// the file cannot be written.
    pub fn clear(&mut self, button: u8) -> Result<bool, BindingError> {
        if !self.bindings.contains_key(&button) {
            return Ok(false);
        }
        let mut next = self.bindings.clone();
        next.remove(&button);
        self.save_file(
            &next,
            &self.knobs.clone(),
            &self.long_press_bindings.clone(),
        )?;
        self.bindings = next;
        Ok(true)
    }

    /// All long-press bindings (BU2), in button order.
    #[must_use]
    pub fn list_long_press(&self) -> &BTreeMap<u8, String> {
        &self.long_press_bindings
    }

    /// The long-press action bound to `button`, if any.
    #[must_use]
    pub fn get_long_press(&self, button: u8) -> Option<&str> {
        self.long_press_bindings.get(&button).map(String::as_str)
    }

    /// Bind `button`'s long-press action and persist. Validates
    /// first; a failed save leaves the in-memory map untouched.
    /// Only physical buttons 1–32: the engine tracks holds for
    /// stream buttons, so an encoder virtual button (65–72) could
    /// never fire its long-press action — it is rejected here
    /// instead of stored dead.
    ///
    /// # Errors
    ///
    /// Returns [`BindingError`] with code `InvalidButton` for a
    /// button outside 1–32, `UnknownAction` for an action id that
    /// is neither a game input nor an app action, and
    /// `BindingSaveFailed` when the file cannot be written.
    pub fn set_long_press(&mut self, button: u8, action: &str) -> Result<(), BindingError> {
        if !(1..=32).contains(&button) {
            return Err(BindingError {
                code: "InvalidButton",
                message: format!(
                    "button {button} cannot have a long-press action (long-press actions are for physical wheel buttons 1-32)"
                ),
            });
        }
        if let Some(message) = validate_action(action) {
            return Err(BindingError {
                code: "UnknownAction",
                message,
            });
        }
        let mut next = self.long_press_bindings.clone();
        next.insert(button, action.to_string());
        self.save_file(&self.bindings.clone(), &self.knobs.clone(), &next)?;
        self.long_press_bindings = next;
        Ok(())
    }

    /// Remove the long-press binding for `button` and persist.
    /// Returns `true` when one was actually removed (`false` =
    /// nothing was bound, nothing written).
    ///
    /// # Errors
    ///
    /// Returns [`BindingError`] with code `BindingSaveFailed` when
    /// the removal cannot be persisted.
    pub fn clear_long_press(&mut self, button: u8) -> Result<bool, BindingError> {
        if !self.long_press_bindings.contains_key(&button) {
            return Ok(false);
        }
        let mut next = self.long_press_bindings.clone();
        next.remove(&button);
        self.save_file(&self.bindings.clone(), &self.knobs.clone(), &next)?;
        self.long_press_bindings = next;
        Ok(true)
    }

    /// Read + validate a `bindings.json`-shaped file. Invalid
    /// entries are dropped with one warning each; a corrupt file
    /// (bad JSON / oversize / newer schema) yields an empty map
    /// with one warning — never an error, never invented data. Knob
    /// entries follow the same rule (`""` or an assignable action,
    /// else `""` with a warning).
    fn read_file(path: &Path) -> BindingsFileContents {
        let mut warnings = Vec::new();
        let empty_knobs = empty_knobs();
        let metadata = match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) => {
                warnings.push(format!(
                    "bindings file '{}' could not be read ({error}) — starting with no bindings",
                    path.display()
                ));
                return (BTreeMap::new(), empty_knobs, BTreeMap::new(), warnings);
            }
        };
        if metadata.len() > MAX_BINDINGS_FILE_BYTES {
            warnings.push(format!(
                "bindings file '{}' exceeds the {MAX_BINDINGS_FILE_BYTES} byte cap — starting with no bindings",
                path.display()
            ));
            return (BTreeMap::new(), empty_knobs, BTreeMap::new(), warnings);
        }
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) => {
                warnings.push(format!(
                    "bindings file '{}' could not be read ({error}) — starting with no bindings",
                    path.display()
                ));
                return (BTreeMap::new(), empty_knobs, BTreeMap::new(), warnings);
            }
        };
        let file: BindingsFile = match serde_json::from_str(&text) {
            Ok(file) => file,
            Err(error) => {
                warnings.push(format!(
                    "bindings file '{}' is not valid JSON ({error}) — starting with no bindings",
                    path.display()
                ));
                return (BTreeMap::new(), empty_knobs, BTreeMap::new(), warnings);
            }
        };
        if file.schema_version > BINDINGS_SCHEMA_VERSION {
            warnings.push(format!(
                "bindings file '{}' has schemaVersion {} (above {BINDINGS_SCHEMA_VERSION}) — starting with no bindings",
                path.display(),
                file.schema_version
            ));
            return (BTreeMap::new(), empty_knobs, BTreeMap::new(), warnings);
        }
        let mut bindings = BTreeMap::new();
        for (button, action) in file.bindings {
            if let Some(message) = validate_button(button) {
                warnings.push(format!(
                    "binding for button {button} dropped on load: {message}"
                ));
                continue;
            }
            if let Some(message) = validate_action(&action) {
                warnings.push(format!(
                    "binding for button {button} dropped on load: {message}"
                ));
                continue;
            }
            bindings.insert(button, action);
        }
        let mut knobs = empty_knobs.clone();
        for (knob, rows) in file.knobs.iter().enumerate() {
            for (row, action) in rows.iter().enumerate() {
                if action.is_empty() || is_assignable_action(action) {
                    knobs[knob][row].clone_from(action);
                } else {
                    warnings.push(format!(
                        "knob {knob} row {row} action dropped on load: unknown action '{action}'"
                    ));
                }
            }
        }
        // BU2: long-press entries follow the same rule, except the
        // button must be physical (1–32) — an encoder virtual
        // button could never fire, so it is dropped with a warning
        // instead of stored dead.
        let mut long_press_bindings = BTreeMap::new();
        for (button, action) in file.long_press_bindings {
            if !(1..=32).contains(&button) {
                warnings.push(format!(
                    "long-press binding for button {button} dropped on load: long-press actions are for physical wheel buttons 1-32"
                ));
                continue;
            }
            if let Some(message) = validate_action(&action) {
                warnings.push(format!(
                    "long-press binding for button {button} dropped on load: {message}"
                ));
                continue;
            }
            long_press_bindings.insert(button, action);
        }
        (bindings, knobs, long_press_bindings, warnings)
    }

    /// Write `bindings` + `knobs` + `long_press_bindings` to
    /// `bindings.json` atomically (temp file in the same directory,
    /// then rename).
    fn save_file(
        &self,
        bindings: &BTreeMap<u8, String>,
        knobs: &[[String; 3]; 2],
        long_press_bindings: &BTreeMap<u8, String>,
    ) -> Result<(), BindingError> {
        let file = BindingsFile {
            schema_version: BINDINGS_SCHEMA_VERSION,
            bindings: bindings.clone(),
            knobs: knobs.clone(),
            long_press_bindings: long_press_bindings.clone(),
        };
        let json = serde_json::to_string_pretty(&file)
            .map_err(|error| BindingError::save_failed(error.to_string()))?;
        fs::create_dir_all(&self.dir).map_err(|error| {
            BindingError::save_failed(format!(
                "cannot create bindings directory '{}': {error}",
                self.dir.display()
            ))
        })?;
        write_atomic(&self.bindings_path(), json.as_bytes())
            .map_err(|error| BindingError::save_failed(error.to_string()))
    }
}

/// Write `bytes` to `path` atomically: a temp file in the same
/// directory, then rename. A crash mid-save never leaves a
/// half-written `bindings.json`. (Same pattern as the profile
/// store's `write_atomic` — duplicated here so this module stays
/// self-contained; the profile store's helper is private to
/// `store.rs`.)
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temp = path.with_extension(format!("json.tmp-{}", std::process::id()));
    fs::write(&temp, bytes)?;
    match fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&temp);
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rsh-bindings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn empty_store_opens_with_no_warnings() {
        let dir = temp_dir("empty");
        let store = BindingStore::open(&dir);
        assert!(store.list().is_empty());
        assert_eq!(store.load_warnings().len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_persists_and_reopens() {
        let dir = temp_dir("persist");
        let mut store = BindingStore::open(&dir);
        store.set(5, "horn").expect("bind button 5");
        store.set(65, "profile_next").expect("bind encoder button");
        assert_eq!(store.get(5), Some("horn"));
        assert_eq!(store.get(65), Some("profile_next"));
        // Reopen: the bindings survive, in button order.
        let reopened = BindingStore::open(&dir);
        assert_eq!(reopened.load_warnings().len(), 0);
        let listed: Vec<(u8, &str)> = reopened
            .list()
            .iter()
            .map(|(&button, action)| (button, action.as_str()))
            .collect();
        assert_eq!(listed, vec![(5, "horn"), (65, "profile_next")]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_rejects_bad_button_and_unknown_action() {
        let dir = temp_dir("reject");
        let mut store = BindingStore::open(&dir);
        let error = store.set(0, "horn").expect_err("button 0");
        assert_eq!(error.code, "InvalidButton");
        let error = store.set(33, "horn").expect_err("button 33");
        assert_eq!(error.code, "InvalidButton");
        let error = store.set(64, "horn").expect_err("button 64");
        assert_eq!(error.code, "InvalidButton");
        let error = store.set(5, "warp_drive").expect_err("unknown action");
        assert_eq!(error.code, "UnknownAction");
        assert!(store.list().is_empty(), "rejected sets store nothing");
        assert!(!dir.join(BINDINGS_FILE_NAME).exists(), "no file written");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clear_removes_and_reports_absence() {
        let dir = temp_dir("clear");
        let mut store = BindingStore::open(&dir);
        assert!(!store.clear(5).expect("clear absent"));
        store.set(5, "horn").expect("bind");
        assert!(store.clear(5).expect("clear present"));
        assert_eq!(store.get(5), None);
        // Reopen: the removal persisted.
        assert!(BindingStore::open(&dir).list().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn long_press_persists_and_reopens() {
        // BU2: the second action survives a reopen, alongside the
        // normal bindings and knobs.
        let dir = temp_dir("long-press");
        let mut store = BindingStore::open(&dir);
        store.set(5, "gear_1").expect("bind button 5");
        store
            .set_long_press(5, "gear_2")
            .expect("bind button 5 long-press");
        assert_eq!(store.get_long_press(5), Some("gear_2"));
        assert_eq!(store.get(5), Some("gear_1"));
        let reopened = BindingStore::open(&dir);
        assert_eq!(reopened.load_warnings().len(), 0);
        assert_eq!(reopened.get_long_press(5), Some("gear_2"));
        assert_eq!(reopened.get(5), Some("gear_1"));
        // Clear only the long-press: the normal binding stands.
        let mut reopened = reopened;
        assert!(reopened.clear_long_press(5).expect("clear long-press"));
        assert_eq!(reopened.get_long_press(5), None);
        assert_eq!(reopened.get(5), Some("gear_1"));
        assert!(!reopened.clear_long_press(5).expect("clear absent"));
        assert_eq!(BindingStore::open(&dir).get(5), Some("gear_1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn long_press_rejects_encoder_buttons_and_unknown_actions() {
        // BU2: the engine tracks holds for stream buttons only, so
        // an encoder virtual button (65–72) is rejected, not stored
        // dead; unknown actions are rejected like normal bindings.
        let dir = temp_dir("long-press-reject");
        let mut store = BindingStore::open(&dir);
        let error = store
            .set_long_press(65, "horn")
            .expect_err("encoder button");
        assert_eq!(error.code, "InvalidButton");
        let error = store.set_long_press(0, "horn").expect_err("button 0");
        assert_eq!(error.code, "InvalidButton");
        let error = store
            .set_long_press(5, "warp_drive")
            .expect_err("unknown action");
        assert_eq!(error.code, "UnknownAction");
        assert!(store.list_long_press().is_empty());
        assert!(!dir.join(BINDINGS_FILE_NAME).exists(), "no file written");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_file_without_long_press_loads_empty() {
        // BU2: a bindings.json written before long-press existed
        // (no `longPressBindings` key) loads with no long-press
        // actions and no warning; an invalid long-press entry is
        // dropped with a warning and the valid ones kept.
        let dir = temp_dir("long-press-old");
        std::fs::write(
            dir.join(BINDINGS_FILE_NAME),
            r#"{"schemaVersion":1,"bindings":{"5":"horn"},"knobs":[["","",""],["","",""]],"longPressBindings":{"70":"horn","5":"gear_2","6":"warp_drive"}}"#,
        )
        .expect("write");
        let store = BindingStore::open(&dir);
        assert_eq!(store.get(5), Some("horn"));
        assert_eq!(store.get_long_press(5), Some("gear_2"));
        assert_eq!(store.get_long_press(70), None);
        assert_eq!(store.get_long_press(6), None);
        assert_eq!(
            store.load_warnings().len(),
            2,
            "{:?}",
            store.load_warnings()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_file_loads_empty_with_a_warning() {
        let dir = temp_dir("corrupt");
        std::fs::write(dir.join(BINDINGS_FILE_NAME), "{not json").expect("write");
        let store = BindingStore::open(&dir);
        assert!(store.list().is_empty());
        assert_eq!(store.load_warnings().len(), 1);
        assert!(
            store.load_warnings()[0].contains("not valid JSON"),
            "warning: {}",
            store.load_warnings()[0]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_entries_are_dropped_with_warnings_and_valid_ones_kept() {
        let dir = temp_dir("partial");
        std::fs::write(
            dir.join(BINDINGS_FILE_NAME),
            r#"{"schemaVersion": 1, "bindings": {"5": "horn", "33": "horn", "7": "warp_drive", "65": "profile_next"}}"#,
        )
        .expect("write");
        let store = BindingStore::open(&dir);
        assert_eq!(store.list().len(), 2);
        assert_eq!(store.get(5), Some("horn"));
        assert_eq!(store.get(65), Some("profile_next"));
        assert_eq!(
            store.load_warnings().len(),
            2,
            "one warning per dropped entry"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn newer_schema_loads_empty_with_a_warning() {
        let dir = temp_dir("schema");
        std::fs::write(
            dir.join(BINDINGS_FILE_NAME),
            r#"{"schemaVersion": 99, "bindings": {"5": "horn"}}"#,
        )
        .expect("write");
        let store = BindingStore::open(&dir);
        assert!(store.list().is_empty());
        assert_eq!(store.load_warnings().len(), 1);
        assert!(store.load_warnings()[0].contains("schemaVersion 99"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn action_vocabulary_splits_game_and_app() {
        assert!(is_game_action("horn"));
        assert!(is_game_action("gear_6"));
        assert!(is_game_action("handbrake"));
        assert!(!is_game_action("profile_next"));
        assert!(is_app_action("profile_next"));
        assert!(is_app_action("profile_previous"));
        assert!(!is_app_action("horn"));
        assert!(is_assignable_action("shifter_down"));
        assert!(!is_assignable_action("warp_drive"));
    }
}
