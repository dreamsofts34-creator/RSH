//! Rust profile + settings store (Phase F).
//!
//! Owns the Rust app's own data files (fresh start, D-R8 — nothing here
//! reads the C# files except the explicit manual import step):
//! `profiles.json` (Profile v2, `schemaVersion` 2) and `settings.json`,
//! both under a directory the caller supplies (normally the app data
//! directory, `%AppData%\RSH` on Windows).
//!
//! Guarantees (the C# lessons, carried over):
//! - Atomic save: write a temp file in the same directory, then rename.
//!   A crash mid-save never leaves a half-written `profiles.json`.
//! - 3-generation backup: every save rotates
//!   `profiles.json` → `.bak1` → `.bak2` (settings get the same
//!   treatment, `settings.json` → `.bak1` → `.bak2`). [`ProfileStore::backup_now`]
//!   forces a rotation; [`ProfileStore::restore_backup`] restores one.
//! - Size caps: profiles at most 1 MiB, settings at most 64 KiB — the
//!   same caps as the C# importer, enforced before parsing.
//! - Import/export is version-checked (`schemaVersion` must be ≤ 2) and
//!   size-capped; one bad entry never silently invents data.
//!
//! Auto-switch resolution ([`ProfileStore::resolve_for_game`], SW-R7.2 /
//! P7.2): a car-specific profile wins first, then a game profile, then
//! the profile named `Default`. Only tuning switches — calibration is
//! board-bound and never changes with a profile switch. A manual
//! override (the user picked a profile by hand) locks auto-switch off
//! until the user re-enables "Auto". On duplicate process names the
//! first profile in list order wins.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::csharp_import::{
    AppSettings, AppSettingsPatch, GameId, MAX_PROFILES_FILE_BYTES, MAX_SETTINGS_FILE_BYTES,
    ProfileFile, WheelProfile, canonical_module_id, read_csharp_profiles_file, sanitize_settings,
};
use crate::resolver::{ProfileId, ProfileSource};

/// The canonical car-model normalizer (H2): one normalizer everywhere.
/// Re-exported from `rsh-telemetry` (`types.rs`) — lowercase with runs
/// of space / `_` / `-` unified to a single `_`, so `ferrari_488_gt3`
/// (AC) and `Ferrari 488 GT3` (ACC) match the same profile key.
pub use rsh_telemetry::normalize_car_model as canonical_normalize_car_model;

/// Store file schema version (Profile v2).
pub const STORE_SCHEMA_VERSION: u32 = 2;
/// Maximum profile name length (SEC-C-15).
pub const MAX_PROFILE_NAME_LEN: usize = 64;
/// Backup generations kept, including the current file.
pub const BACKUP_GENERATIONS: usize = 3;

/// Errors from the profile/settings store.
#[derive(Debug)]
pub enum StoreError {
    /// A filesystem operation failed.
    Io(io::Error),
    /// Stored JSON could not be parsed.
    Json(serde_json::Error),
    /// A file exceeds the size cap.
    TooLarge {
        /// The cap that was exceeded, in bytes.
        limit: u64,
    },
    /// No profile with this id exists.
    NotFound(String),
    /// The data failed validation; one message per failed rule.
    Validation(Vec<String>),
    /// The stored schema version is above [`STORE_SCHEMA_VERSION`].
    Schema(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "profile store I/O error: {error}"),
            Self::Json(error) => write!(f, "profile store JSON error: {error}"),
            Self::TooLarge { limit } => {
                write!(f, "profile store file exceeds the {limit} byte cap")
            }
            Self::NotFound(id) => write!(f, "no profile with id '{id}'"),
            Self::Validation(errors) => {
                write!(f, "profile failed validation: {}", errors.join("; "))
            }
            Self::Schema(message) => write!(f, "profile store schema error: {message}"),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::TooLarge { .. } | Self::NotFound(_) | Self::Validation(_) | Self::Schema(_) => {
                None
            }
        }
    }
}

impl From<io::Error> for StoreError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

/// One backup generation, as the frontend `list_backups` command reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackupGeneration {
    /// The live file (`profiles.json`).
    Current,
    /// The previous generation (`profiles.json.bak1`).
    Bak1,
    /// The generation before that (`profiles.json.bak2`).
    Bak2,
}

impl BackupGeneration {
    /// The string the frontend matches on (`"current"`, `"bak1"`, `"bak2"`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Bak1 => "bak1",
            Self::Bak2 => "bak2",
        }
    }

    /// The file suffix for this generation (empty for the live file).
    #[must_use]
    pub const fn suffix(self) -> &'static str {
        match self {
            Self::Current => "",
            Self::Bak1 => ".bak1",
            Self::Bak2 => ".bak2",
        }
    }

    /// Parse a generation string; `None` for anything else.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "current" => Some(Self::Current),
            "bak1" => Some(Self::Bak1),
            "bak2" => Some(Self::Bak2),
            _ => None,
        }
    }
}

/// One row of the `list_backups` result.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BackupInfo {
    /// Generation (`"current"`, `"bak1"`, `"bak2"`).
    pub generation: String,
    /// File this row describes (`"profiles.json"` or `"settings.json"`).
    pub file: String,
    /// True when the file for this generation exists on disk.
    pub present: bool,
    /// When the generation was last saved (RFC 3339 UTC), or `None`.
    pub saved_at: Option<String>,
}

/// One stored profile: a stable id plus the profile itself.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct StoredProfile {
    /// Stable id (slug of the name, unique in the store).
    pub id: String,
    /// The wheel profile.
    pub profile: WheelProfile,
}

/// List-row summary, matching the frontend `ProfileSummary` shape.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSummary {
    /// Stable profile id.
    pub id: String,
    /// Profile name.
    pub name: String,
    /// Game slug (`"assetto_corsa"`, `"assetto_corsa_competizione"`,
    /// `"iracing"`), or `None` for a game-less profile.
    pub game: Option<String>,
    /// True when this is the active profile.
    pub active: bool,
}

/// On-disk shape of `profiles.json`.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case", default)]
struct ProfilesFile {
    schema_version: u32,
    profiles: Vec<StoredProfile>,
    active_profile_id: Option<String>,
    auto_switch_locked: bool,
}

/// On-disk shape of a single-profile export file.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
struct ProfileExport {
    schema_version: u32,
    profile: StoredProfile,
}

/// The game slug for a [`GameId`], or `None` for [`GameId::Unknown`].
#[must_use]
pub fn game_slug(game: GameId) -> Option<&'static str> {
    match game {
        GameId::Unknown => None,
        GameId::AssettoCorsa => Some("assetto_corsa"),
        GameId::Acc => Some("assetto_corsa_competizione"),
        GameId::IRacing => Some("iracing"),
        GameId::DirtRally2 => Some("dirt_rally_2"),
    }
}

/// Reverse of [`game_slug`] (M1, v2.0.2): the profile-link path
/// takes a slug from the UI and stores the matching [`GameId`].
/// Unknown slugs map to [`GameId::Unknown`] (game-less profile).
#[must_use]
pub fn game_id_from_slug(slug: &str) -> GameId {
    match slug {
        "assetto_corsa" => GameId::AssettoCorsa,
        "assetto_corsa_competizione" => GameId::Acc,
        "iracing" => GameId::IRacing,
        "dirt_rally_2" => GameId::DirtRally2,
        _ => GameId::Unknown,
    }
}

/// Normalize a game process name for matching: trim, strip a trailing
/// `.exe` (case-insensitive), lowercase.
#[must_use]
pub fn normalize_process_name(name: &str) -> String {
    let lowered = name.trim().to_ascii_lowercase();
    lowered
        .strip_suffix(".exe")
        .map(str::to_string)
        .unwrap_or(lowered)
}

/// Normalize a car model for matching: the canonical normalizer
/// (H2 — `rsh-telemetry` `types.rs`): trim, lowercase, and unify runs
/// of space / `_` / `-` to a single `_`, so `ferrari_488_gt3` (AC)
/// and `Ferrari 488 GT3` (ACC) are the same key. Every lookup in this
/// crate (store resolution, watcher, adapter) goes through this one
/// function — never a local variant.
#[must_use]
pub fn normalize_car_model(model: &str) -> String {
    rsh_telemetry::normalize_car_model(model)
}

/// Validate a profile name (SEC-C-15): non-empty after trimming, at most
/// [`MAX_PROFILE_NAME_LEN`] characters, no path separators or control
/// characters.
#[must_use]
pub fn validate_profile_name(name: &str) -> Vec<String> {
    let mut errors = Vec::new();
    let trimmed = name.trim();
    if trimmed.is_empty() {
        errors.push("profile name must not be empty".to_string());
        return errors;
    }
    if trimmed.chars().count() > MAX_PROFILE_NAME_LEN {
        errors.push(format!(
            "profile name is longer than {MAX_PROFILE_NAME_LEN} characters"
        ));
    }
    if trimmed.chars().any(|c| {
        c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
    }) {
        errors.push(
            "profile name must not contain path separators or control characters".to_string(),
        );
    }
    errors
}

/// Validate a [`WheelProfile`] against the documented ranges (D9 gear
/// buttons 1..=32, deadzone 0..=0.5, curve -1..=1, smoothing 0..=1,
/// steering scale 0.2..=1.0, pedal press/release 10..=2000 ms).
/// Returns one message per failed rule (empty = valid).
#[must_use]
pub fn validate_profile(profile: &WheelProfile) -> Vec<String> {
    let mut errors = validate_profile_name(&profile.name);
    if !(0.0..=0.5).contains(&profile.deadzone) {
        errors.push(format!("deadzone {} is outside 0..=0.5", profile.deadzone));
    }
    if !(-1.0..=1.0).contains(&profile.curve_strength) {
        errors.push(format!(
            "curve_strength {} is outside -1..=1",
            profile.curve_strength
        ));
    }
    if !(0.0..=1.0).contains(&profile.smoothing) {
        errors.push(format!("smoothing {} is outside 0..=1", profile.smoothing));
    }
    if !(0.2..=1.0).contains(&profile.steering_scale) {
        errors.push(format!(
            "steering_scale {} is outside 0.2..=1.0",
            profile.steering_scale
        ));
    }
    for (label, button) in [
        ("gear_up_vjoy_button", profile.gear_up_vjoy_button),
        ("gear_down_vjoy_button", profile.gear_down_vjoy_button),
    ] {
        if !(1..=32).contains(&button) {
            errors.push(format!("{label} {button} is outside 1..=32"));
        }
    }
    for (label, ms) in [
        ("pedal_press_ms", profile.pedal_press_ms),
        ("pedal_release_ms", profile.pedal_release_ms),
    ] {
        if !(10.0..=2000.0).contains(&ms) {
            errors.push(format!("{label} {ms} is outside 10..=2000"));
        }
    }
    for (module, tuning) in &profile.module_tunings {
        if !(0.0..=0.5).contains(&tuning.deadzone) {
            errors.push(format!(
                "module '{module}' deadzone {} is outside 0..=0.5",
                tuning.deadzone
            ));
        }
        if !(-1.0..=1.0).contains(&tuning.curve_strength) {
            errors.push(format!(
                "module '{module}' curve_strength {} is outside -1..=1",
                tuning.curve_strength
            ));
        }
        if !(0.0..=1.0).contains(&tuning.smoothing) {
            errors.push(format!(
                "module '{module}' smoothing {} is outside 0..=1",
                tuning.smoothing
            ));
        }
        if !(0.2..=1.0).contains(&tuning.scale) {
            errors.push(format!(
                "module '{module}' scale {} is outside 0.2..=1.0",
                tuning.scale
            ));
        }
    }
    errors
}

/// Format a [`SystemTime`] as RFC 3339 UTC (`2026-11-14T12:00:00Z`).
/// Returns `None` for times before the Unix epoch. Pure-std civil
/// conversion (Howard Hinnant's days-from-civil algorithm); no date
/// crate is in the workspace yet.
#[must_use]
pub fn format_system_time_rfc3339(time: SystemTime) -> Option<String> {
    let secs = time.duration_since(UNIX_EPOCH).ok()?.as_secs();
    let days = secs / 86_400;
    let secs_of_day = secs % 86_400;
    let (hour, minute, second) = (
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60,
    );
    // days since 1970-01-01 -> civil (y, m, d).
    let z = i64::try_from(days).ok()? + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z"
    ))
}

/// The Rust profile + settings store.
pub struct ProfileStore {
    /// Directory holding `profiles.json` / `settings.json`.
    dir: PathBuf,
    /// Profiles in list order (insertion order = resolution order).
    profiles: Vec<StoredProfile>,
    /// Id of the active profile, if any.
    active_profile_id: Option<String>,
    /// App settings (in-memory copy of `settings.json`).
    settings: AppSettings,
    /// True when the user picked a profile by hand: auto-switch stays
    /// off until the user re-enables "Auto".
    auto_switch_locked: bool,
    /// Load-time validation notes (H5): entries rejected, duplicates
    /// dropped, or the Default profile recreated on load. Empty on a
    /// clean load; surfaced by the app as a startup notice, never
    /// silently swallowed.
    load_warnings: Vec<String>,
}

impl ProfileStore {
    /// Open the store in `dir`, loading `profiles.json` and
    /// `settings.json` when they exist. A missing file means defaults
    /// (one `Default` profile, default settings); a corrupt file is an
    /// error, never silently dropped data.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Json`]/[`StoreError::Schema`]/[`StoreError::TooLarge`]
    /// when a present file cannot be parsed, and [`StoreError::Io`] when
    /// a present file cannot be read.
    pub fn open(dir: &Path) -> Result<Self, StoreError> {
        let profiles_path = dir.join("profiles.json");
        let (profiles, active_profile_id, auto_switch_locked) = if profiles_path.exists() {
            Self::read_profiles_file(&profiles_path)?
        } else {
            (vec![StoredProfile::default_profile()], None, false)
        };
        // H5: validate the loaded list — an invalid entry is rejected
        // (never loaded as a live profile), a duplicate id/name is
        // dropped (first wins), and an empty list recreates Default.
        // A corrupt file (bad JSON / oversize / bad schema) is still
        // an error above — validation only ever drops *entries*, never
        // invents data.
        let (profiles, load_warnings) = Self::validate_loaded(profiles);
        let active_profile_id = active_profile_id.filter(|id| profiles.iter().any(|p| &p.id == id));
        let settings_path = dir.join("settings.json");
        let settings = if settings_path.exists() {
            Self::read_settings_file(&settings_path)?
        } else {
            AppSettings::default()
        };
        Ok(Self {
            dir: dir.to_path_buf(),
            profiles,
            active_profile_id,
            settings,
            auto_switch_locked,
            load_warnings,
        })
    }

    /// Load-time validation for a `profiles.json` list (H5).
    ///
    /// Returns the kept profiles plus one warning per dropped entry:
    /// invalid entries (failing [`validate_profile`]) are rejected,
    /// duplicate ids and duplicate names (case-insensitive) keep the
    /// first entry, and an empty result recreates the `Default`
    /// profile so the store is never profile-less. Module-tuning keys
    /// are canonicalized (`"accel"` → `"accelerator"`, C-MOD-1).
    fn validate_loaded(profiles: Vec<StoredProfile>) -> (Vec<StoredProfile>, Vec<String>) {
        let mut warnings = Vec::new();
        let mut kept: Vec<StoredProfile> = Vec::with_capacity(profiles.len());
        let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut seen_names: std::collections::HashSet<String> = std::collections::HashSet::new();
        for mut stored in profiles {
            // Canonicalize module-tuning keys first (C-MOD-1).
            let mut migrated: BTreeMap<String, crate::csharp_import::PerModuleTuning> =
                BTreeMap::new();
            for (key, tuning) in std::mem::take(&mut stored.profile.module_tunings) {
                let canonical = canonical_module_id(&key);
                if canonical != key && migrated.contains_key(&canonical) {
                    warnings.push(format!(
                        "profile '{}': duplicate module tuning '{key}' dropped (canonical '{canonical}' already present)",
                        stored.profile.name
                    ));
                    continue;
                }
                migrated.insert(canonical, tuning);
            }
            stored.profile.module_tunings = migrated;
            let errors = validate_profile(&stored.profile);
            if !errors.is_empty() {
                warnings.push(format!(
                    "profile '{}' (id '{}') rejected on load: {}",
                    stored.profile.name,
                    stored.id,
                    errors.join("; ")
                ));
                continue;
            }
            if !seen_ids.insert(stored.id.clone()) {
                warnings.push(format!(
                    "profile '{}' (id '{}') rejected on load: duplicate id",
                    stored.profile.name, stored.id
                ));
                continue;
            }
            let name_key = stored.profile.name.trim().to_ascii_lowercase();
            if !seen_names.insert(name_key) {
                warnings.push(format!(
                    "profile '{}' (id '{}') rejected on load: duplicate name",
                    stored.profile.name, stored.id
                ));
                continue;
            }
            kept.push(stored);
        }
        if kept.is_empty() {
            warnings
                .push("no valid profiles on load — the Default profile was recreated".to_string());
            kept.push(StoredProfile::default_profile());
        }
        (kept, warnings)
    }

    /// Load-time validation notes from [`ProfileStore::open`] (H5):
    /// rejected entries, dropped duplicates, Default recreation.
    /// Empty on a clean load.
    #[must_use]
    pub fn load_warnings(&self) -> &[String] {
        &self.load_warnings
    }

    /// The directory this store reads from / writes to.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// All profiles, in list order.
    #[must_use]
    pub fn list(&self) -> &[StoredProfile] {
        &self.profiles
    }

    /// List-row summaries for the Profiles screen.
    #[must_use]
    pub fn list_summaries(&self) -> Vec<ProfileSummary> {
        self.profiles
            .iter()
            .map(|stored| ProfileSummary {
                id: stored.id.clone(),
                name: stored.profile.name.clone(),
                game: game_slug(stored.profile.game).map(str::to_string),
                active: Some(&stored.id) == self.active_profile_id.as_ref(),
            })
            .collect()
    }

    /// The profile with this id, if any.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&StoredProfile> {
        self.profiles.iter().find(|p| p.id == id)
    }

    /// The active profile, if one is set.
    #[must_use]
    pub fn active(&self) -> Option<&StoredProfile> {
        self.active_profile_id
            .as_deref()
            .and_then(|id| self.get(id))
    }

    /// The current settings.
    #[must_use]
    pub fn settings(&self) -> &AppSettings {
        &self.settings
    }

    /// Whether a manual profile choice is locking auto-switch off.
    #[must_use]
    pub const fn auto_switch_locked(&self) -> bool {
        self.auto_switch_locked
    }

    /// Create a profile with default tuning under `name`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Validation`] when the name is invalid or
    /// already taken (case-insensitive — L3: two profiles must never
    /// share a name, or resolution and the UI cannot tell them apart).
    pub fn create(&mut self, name: &str) -> Result<StoredProfile, StoreError> {
        let errors = validate_profile_name(name);
        if !errors.is_empty() {
            return Err(StoreError::Validation(errors));
        }
        let trimmed = name.trim();
        if self
            .profiles
            .iter()
            .any(|p| p.profile.name.eq_ignore_ascii_case(trimmed))
        {
            return Err(StoreError::Validation(vec![format!(
                "a profile named '{trimmed}' already exists"
            )]));
        }
        let profile = WheelProfile {
            name: trimmed.to_string(),
            ..WheelProfile::default()
        };
        let id = self.unique_id(&profile.name);
        let stored = StoredProfile { id, profile };
        self.profiles.push(stored.clone());
        Ok(stored)
    }

    /// Duplicate a profile under `"<name> copy"`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] for an unknown id. The copy's
    /// name is truncated so the `"<name> copy"` / `" (2)"` suffix
    /// never pushes it past [`MAX_PROFILE_NAME_LEN`] (L3: a 64-char
    /// name duplicated cleanly instead of failing validation).
    pub fn duplicate(&mut self, id: &str) -> Result<StoredProfile, StoreError> {
        let source = self
            .get(id)
            .ok_or_else(|| StoreError::NotFound(id.to_string()))?;
        let mut profile = source.profile.clone();
        let base = source.profile.name.clone();
        let mut candidate = Self::copy_name(&base);
        let mut n = 2;
        while self.profiles.iter().any(|p| p.profile.name == candidate) {
            candidate = Self::copy_name_numbered(&base, n);
            n += 1;
        }
        profile.name = candidate;
        let new_id = self.unique_id(&profile.name);
        let stored = StoredProfile {
            id: new_id,
            profile,
        };
        self.profiles.push(stored.clone());
        Ok(stored)
    }

    /// Name for a duplicate of `name`: `"<name> copy"`, or
    /// `"<name> (2)"` when that is taken / too long — always within
    /// [`MAX_PROFILE_NAME_LEN`] characters.
    fn copy_name(name: &str) -> String {
        Self::copy_name_numbered(name, 0)
    }

    /// Numbered duplicate name: `"<name> copy"` for `n == 0`, else
    /// `"<name> (n+1)"` — truncated to fit [`MAX_PROFILE_NAME_LEN`].
    fn copy_name_numbered(name: &str, n: u32) -> String {
        fn fit(base: &str, suffix: &str) -> String {
            let room = MAX_PROFILE_NAME_LEN.saturating_sub(suffix.chars().count());
            let truncated: String = base.chars().take(room).collect();
            format!("{}{suffix}", truncated.trim_end())
        }
        if n == 0 {
            let copy = fit(name, " copy");
            if validate_profile_name(&copy).is_empty() {
                return copy;
            }
            return fit(name, " (2)");
        }
        fit(name, &format!(" ({})", n + 1))
    }

    /// Delete a profile. Deleting the active profile clears the active id.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] for an unknown id.
    pub fn delete(&mut self, id: &str) -> Result<(), StoreError> {
        let before = self.profiles.len();
        self.profiles.retain(|p| p.id != id);
        if self.profiles.len() == before {
            return Err(StoreError::NotFound(id.to_string()));
        }
        if self.active_profile_id.as_deref() == Some(id) {
            self.active_profile_id = None;
        }
        Ok(())
    }

    /// Replace a profile's contents (validated first; the id is unchanged).
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] for an unknown id and
    /// [`StoreError::Validation`] when the new profile is invalid.
    pub fn update(&mut self, id: &str, profile: WheelProfile) -> Result<(), StoreError> {
        let errors = validate_profile(&profile);
        if !errors.is_empty() {
            return Err(StoreError::Validation(errors));
        }
        let stored = self
            .profiles
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| StoreError::NotFound(id.to_string()))?;
        stored.profile = profile;
        Ok(())
    }

    /// Activate a profile. A manual activation locks auto-switch off
    /// (manual override wins — see the module docs); pass
    /// `manual: false` when the activation itself came from auto-switch
    /// — an automatic activation clears the lock (H3: the lock is the
    /// manual override, so an auto-switch must not leave it set).
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] for an unknown id.
    pub fn activate(&mut self, id: &str, manual: bool) -> Result<(), StoreError> {
        if self.get(id).is_none() {
            return Err(StoreError::NotFound(id.to_string()));
        }
        self.active_profile_id = Some(id.to_string());
        self.auto_switch_locked = manual;
        Ok(())
    }

    /// Re-enable automatic profile switching after a manual override.
    pub fn enable_auto_switch(&mut self) {
        self.auto_switch_locked = false;
    }

    /// Clears the manual-override lock without changing the
    /// `auto_switch_profiles` setting (H3): a settings write that
    /// turns auto-switch on uses this instead of
    /// [`enable_auto_switch`](Self::enable_auto_switch) when the
    /// setting itself was already on.
    pub fn clear_auto_switch_lock(&mut self) {
        self.auto_switch_locked = false;
    }

    /// Resolve the profile for a running game + car (SW-R7.2 / P7.2).
    ///
    /// Priority: (1) car-specific profile — same game process AND same
    /// car model; (2) game profile — same game process, no car bound;
    /// (3) the profile named `Default`. On duplicate process names the
    /// first profile in list order wins. Returns `None` when auto-switch
    /// is locked by a manual override, or when no game profile and no
    /// `Default` profile exist.
    ///
    /// Only tuning switches with a profile — calibration is board-bound
    /// and is never changed by this resolution; the caller applies the
    /// returned profile's tuning only.
    #[must_use]
    pub fn resolve_for_game(
        &self,
        game_process_name: &str,
        car_model: &str,
    ) -> Option<&StoredProfile> {
        if self.auto_switch_locked {
            return None;
        }
        let game = normalize_process_name(game_process_name);
        let car = normalize_car_model(car_model);
        if game.is_empty() {
            return None;
        }
        // (1) Car-specific: same game AND same (non-empty) car.
        if !car.is_empty() {
            for stored in &self.profiles {
                if normalize_process_name(&stored.profile.game_process_name) == game
                    && normalize_car_model(&stored.profile.car_model) == car
                {
                    return Some(stored);
                }
            }
        }
        // (2) Game profile: same game, no car bound.
        for stored in &self.profiles {
            if normalize_process_name(&stored.profile.game_process_name) == game
                && stored.profile.car_model.trim().is_empty()
            {
                return Some(stored);
            }
        }
        // (3) Default profile.
        self.profiles
            .iter()
            .find(|stored| stored.profile.name.eq_ignore_ascii_case("Default"))
    }

    /// Replace the in-memory settings, sanitized first (M10: the
    /// allowlists / ranges are enforced on the Rust write path too,
    /// not only on C# import — a hand-edited or frontend-sent bad
    /// value can never be persisted).
    pub fn set_settings(&mut self, settings: AppSettings) {
        let mut settings = settings;
        sanitize_settings(&mut settings);
        self.settings = settings;
    }

    /// Apply a partial settings update (C-SET-1, PATCH semantics):
    /// only the provided fields overwrite; absent fields keep their
    /// existing values. The result is sanitized and returned.
    pub fn set_settings_patch(&mut self, patch: AppSettingsPatch) -> AppSettings {
        self.settings.apply_patch(patch);
        self.settings.clone()
    }

    /// Path of the profiles file.
    #[must_use]
    pub fn profiles_path(&self) -> PathBuf {
        self.dir.join("profiles.json")
    }

    /// Path of the settings file.
    #[must_use]
    pub fn settings_path(&self) -> PathBuf {
        self.dir.join("settings.json")
    }

    /// Path of one backup generation's file.
    #[must_use]
    pub fn backup_path(&self, generation: BackupGeneration) -> PathBuf {
        self.dir
            .join(format!("profiles.json{}", generation.suffix()))
    }

    /// Save profiles + settings (atomic, with backup rotation).
    ///
    /// All-or-nothing is impossible across two files with rotation,
    /// so a half failure is reported honestly instead (H6): when the
    /// profiles save succeeds but the settings save fails, the error
    /// names the half that was already written, so the caller knows
    /// the store is in a partial state instead of assuming a clean
    /// failure.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Io`] when a file cannot be written.
    pub fn save(&self) -> Result<(), StoreError> {
        self.save_profiles()?;
        self.save_settings().map_err(|error| {
            StoreError::Io(io::Error::other(format!(
                "settings save failed after profiles were already saved (partial state — profiles are new, settings are old): {error}"
            )))
        })
    }

    /// Save `profiles.json` atomically, rotating the 3 generations first.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Io`] when a file cannot be written or a
    /// backup rotation fails.
    pub fn save_profiles(&self) -> Result<(), StoreError> {
        let file = ProfilesFile {
            schema_version: STORE_SCHEMA_VERSION,
            profiles: self.profiles.clone(),
            active_profile_id: self.active_profile_id.clone(),
            auto_switch_locked: self.auto_switch_locked,
        };
        let json = serde_json::to_string_pretty(&file)?;
        fs::create_dir_all(&self.dir)?;
        rotate_backups(&self.profiles_path())?;
        write_atomic(&self.profiles_path(), json.as_bytes())
    }

    /// Save `settings.json` atomically, rotating its 3 generations first.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Io`] when a file cannot be written or a
    /// backup rotation fails.
    pub fn save_settings(&self) -> Result<(), StoreError> {
        let json = serde_json::to_string_pretty(&self.settings)?;
        fs::create_dir_all(&self.dir)?;
        rotate_backups(&self.settings_path())?;
        write_atomic(&self.settings_path(), json.as_bytes())
    }

    /// Force a backup rotation now (the "Backup now" button): the
    /// current generations shift by one without changing any content.
    /// Returns the backup list afterwards.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Io`] when a backup file cannot be moved.
    pub fn backup_now(&self) -> Result<Vec<BackupInfo>, StoreError> {
        fs::create_dir_all(&self.dir)?;
        rotate_backups(&self.profiles_path())?;
        rotate_backups(&self.settings_path())?;
        Ok(self.list_backups())
    }

    /// The backup list: 3 generations for `profiles.json` plus 3 for
    /// `settings.json` (M5 — the Profiles backup card shows both
    /// files; each row names its `file`).
    #[must_use]
    pub fn list_backups(&self) -> Vec<BackupInfo> {
        list_backups_in(&self.dir)
    }

    /// Restore one backup generation over the live `profiles.json` and
    /// reload the in-memory state from it.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] when the generation file does
    /// not exist, and the read/parse errors when it cannot be restored.
    pub fn restore_backup(&mut self, generation: BackupGeneration) -> Result<(), StoreError> {
        if generation == BackupGeneration::Current {
            return Ok(());
        }
        self.restore_backup_path(&self.backup_path(generation))
    }

    /// Restore from a real backup path (M9): `path` must name one of
    /// the six store files in this store's directory —
    /// `profiles.json[.bak1|.bak2]` or `settings.json[.bak1|.bak2]`.
    /// A profiles file reloads the profiles + active id + lock; a
    /// settings file reloads (and sanitizes) the settings. Restoring
    /// the live file itself is a no-op. Any other path is rejected —
    /// the caller passes the path the backup list reported, never an
    /// arbitrary file.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] when the path names no known
    /// store file or the file does not exist, and the read/parse
    /// errors when it cannot be restored.
    pub fn restore_backup_path(&mut self, path: &Path) -> Result<(), StoreError> {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        let (is_profiles, generation) = match file_name {
            "profiles.json" => (true, BackupGeneration::Current),
            "profiles.json.bak1" => (true, BackupGeneration::Bak1),
            "profiles.json.bak2" => (true, BackupGeneration::Bak2),
            "settings.json" => (false, BackupGeneration::Current),
            "settings.json.bak1" => (false, BackupGeneration::Bak1),
            "settings.json.bak2" => (false, BackupGeneration::Bak2),
            _ => {
                return Err(StoreError::NotFound(format!(
                    "cannot restore backup from '{path}': expected a profiles.json / settings.json generation in the store directory",
                    path = path.display()
                )));
            }
        };
        if generation == BackupGeneration::Current {
            return Ok(());
        }
        // The path must name a file inside this store's directory: a
        // same-named file elsewhere is never a backup of this store.
        // A bare file name (or a relative path) resolves against the
        // store directory, which is what the backup list reports.
        let full_path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.dir.join(path)
        };
        let in_store_dir = full_path.parent().is_some_and(|parent| parent == self.dir);
        if !in_store_dir {
            return Err(StoreError::NotFound(format!(
                "cannot restore backup from '{}': not in this store's directory",
                path.display()
            )));
        }
        let path = full_path.as_path();
        if !path.exists() {
            return Err(StoreError::NotFound(format!(
                "backup generation '{}' does not exist",
                generation.as_str()
            )));
        }
        if is_profiles {
            let (profiles, active_profile_id, auto_switch_locked) = Self::read_profiles_file(path)?;
            // The restored content becomes the live file (atomically, and
            // the pre-restore live file rotates into .bak1 first).
            let file = ProfilesFile {
                schema_version: STORE_SCHEMA_VERSION,
                profiles: profiles.clone(),
                active_profile_id: active_profile_id.clone(),
                auto_switch_locked,
            };
            let json = serde_json::to_string_pretty(&file)?;
            rotate_backups(&self.profiles_path())?;
            write_atomic(&self.profiles_path(), json.as_bytes())?;
            let (profiles, warnings) = Self::validate_loaded(profiles);
            self.load_warnings = warnings;
            self.profiles = profiles;
            self.active_profile_id =
                active_profile_id.filter(|id| self.profiles.iter().any(|p| &p.id == id));
            self.auto_switch_locked = auto_switch_locked;
            Ok(())
        } else {
            let settings = Self::read_settings_file(path)?;
            let json = serde_json::to_string_pretty(&settings)?;
            rotate_backups(&self.settings_path())?;
            write_atomic(&self.settings_path(), json.as_bytes())?;
            self.settings = settings;
            Ok(())
        }
    }

    /// Export one profile to `path` (single-profile export format,
    /// version-checked on import).
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] for an unknown id and
    /// [`StoreError::Io`] when the file cannot be written.
    pub fn export_profile(&self, id: &str, path: &Path) -> Result<(), StoreError> {
        let stored = self
            .get(id)
            .ok_or_else(|| StoreError::NotFound(id.to_string()))?;
        let export = ProfileExport {
            schema_version: STORE_SCHEMA_VERSION,
            profile: stored.clone(),
        };
        let json = serde_json::to_string_pretty(&export)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        write_atomic(path, json.as_bytes())
    }

    /// Import one profile from an export file (size-capped at 1 MiB,
    /// `schemaVersion` must be ≤ 2). A name clash gets a numeric suffix.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::TooLarge`], [`StoreError::Schema`] or
    /// [`StoreError::Json`] for a bad file, and
    /// [`StoreError::Validation`] for an invalid profile.
    pub fn import_profile(&mut self, path: &Path) -> Result<StoredProfile, StoreError> {
        let metadata = fs::metadata(path)?;
        if metadata.len() > MAX_PROFILES_FILE_BYTES {
            return Err(StoreError::TooLarge {
                limit: MAX_PROFILES_FILE_BYTES,
            });
        }
        let text = fs::read_to_string(path)?;
        if text.len() as u64 > MAX_PROFILES_FILE_BYTES {
            return Err(StoreError::TooLarge {
                limit: MAX_PROFILES_FILE_BYTES,
            });
        }
        let export: ProfileExport = serde_json::from_str(&text)?;
        if export.schema_version > STORE_SCHEMA_VERSION {
            return Err(StoreError::Schema(format!(
                "schemaVersion {} is above {STORE_SCHEMA_VERSION}",
                export.schema_version
            )));
        }
        let errors = validate_profile(&export.profile.profile);
        if !errors.is_empty() {
            return Err(StoreError::Validation(errors));
        }
        Ok(self.insert_imported(export.profile.profile))
    }

    /// Import every profile from a C# `profiles.json` (manual import
    /// step only — never automatic). Names that already exist get a
    /// numeric suffix; bad entries are skipped and counted in the
    /// returned report, mirroring the importer's F3 tolerance.
    ///
    /// # Errors
    ///
    /// Returns the importer's errors when the file cannot be read or
    /// parsed.
    pub fn import_csharp_profiles(
        &mut self,
        path: &Path,
    ) -> Result<crate::csharp_import::ProfileFile, StoreError> {
        let file = read_csharp_profiles_file(path).map_err(|error| match error {
            crate::csharp_import::ImportError::Io(error) => StoreError::Io(error),
            crate::csharp_import::ImportError::Json(error) => StoreError::Json(error),
            crate::csharp_import::ImportError::TooLarge { limit } => StoreError::TooLarge { limit },
            crate::csharp_import::ImportError::Schema(message) => StoreError::Schema(message),
        })?;
        for profile in &file.profiles {
            self.insert_imported(profile.clone());
        }
        // The active C# profile becomes active when it was imported.
        if let Some(active) = self
            .profiles
            .iter()
            .find(|stored| stored.profile.name == file.active_profile_name)
        {
            let id = active.id.clone();
            self.active_profile_id = Some(id);
        }
        Ok(file)
    }

    /// Import settings from a C# `settings.json` (manual import step
    /// only). The Wi-Fi password is never carried over — the importer
    /// ignores it (see `csharp_import`). The import is a MERGE (M6):
    /// only the fields the C# file actually contains overwrite the
    /// current settings; absent fields keep their Rust values — a
    /// partial C# file can never wipe settings it does not mention.
    ///
    /// # Errors
    ///
    /// Returns the importer's errors when the file cannot be read or parsed.
    pub fn import_csharp_settings(&mut self, path: &Path) -> Result<AppSettings, StoreError> {
        let text = fs::read_to_string(path)?;
        if text.len() as u64 > MAX_SETTINGS_FILE_BYTES {
            return Err(StoreError::TooLarge {
                limit: MAX_SETTINGS_FILE_BYTES,
            });
        }
        let value: serde_json::Value = serde_json::from_str(&text)?;
        // Only the keys present in the C# file are applied: normalize
        // the PascalCase keys, parse into a patch (unknown keys such
        // as the Wi-Fi password are ignored, values are sanitized by
        // apply_patch), never a full replace.
        let normalized = crate::csharp_import::normalize_keys(value);
        let settings_patch: AppSettingsPatch =
            serde_json::from_value(normalized).map_err(StoreError::Json)?;
        Ok(self.set_settings_patch(settings_patch))
    }

    /// Read + validate a `profiles.json`-shaped file.
    fn read_profiles_file(
        path: &Path,
    ) -> Result<(Vec<StoredProfile>, Option<String>, bool), StoreError> {
        let metadata = fs::metadata(path)?;
        if metadata.len() > MAX_PROFILES_FILE_BYTES {
            return Err(StoreError::TooLarge {
                limit: MAX_PROFILES_FILE_BYTES,
            });
        }
        let text = fs::read_to_string(path)?;
        if text.len() as u64 > MAX_PROFILES_FILE_BYTES {
            return Err(StoreError::TooLarge {
                limit: MAX_PROFILES_FILE_BYTES,
            });
        }
        let file: ProfilesFile = serde_json::from_str(&text)?;
        if file.schema_version > STORE_SCHEMA_VERSION {
            return Err(StoreError::Schema(format!(
                "schemaVersion {} is above {STORE_SCHEMA_VERSION}",
                file.schema_version
            )));
        }
        Ok((
            file.profiles,
            file.active_profile_id,
            file.auto_switch_locked,
        ))
    }

    /// Read + validate a `settings.json` file (M10: sanitized on the
    /// Rust load path too — a hand-edited bad value can never be
    /// loaded into memory unsanitized).
    fn read_settings_file(path: &Path) -> Result<AppSettings, StoreError> {
        let text = fs::read_to_string(path)?;
        if text.len() as u64 > MAX_SETTINGS_FILE_BYTES {
            return Err(StoreError::TooLarge {
                limit: MAX_SETTINGS_FILE_BYTES,
            });
        }
        let mut settings: AppSettings = serde_json::from_str(&text)?;
        sanitize_settings(&mut settings);
        Ok(settings)
    }

    /// Insert an imported profile, de-duplicating the name and id.
    fn insert_imported(&mut self, mut profile: WheelProfile) -> StoredProfile {
        if !validate_profile_name(&profile.name).is_empty() {
            profile.name = "Imported".to_string();
        }
        let base = profile.name.clone();
        let mut candidate = base.clone();
        let mut suffix = 2;
        while self.profiles.iter().any(|p| p.profile.name == candidate) {
            candidate = format!("{base} ({suffix})");
            suffix += 1;
        }
        profile.name = candidate;
        let id = self.unique_id(&profile.name);
        let stored = StoredProfile { id, profile };
        self.profiles.push(stored.clone());
        stored
    }

    /// A unique id for `name`: slugified, with a numeric suffix on clash.
    fn unique_id(&self, name: &str) -> String {
        let slug: String = name
            .trim()
            .to_ascii_lowercase()
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c
                } else if c == ' ' || c == '-' || c == '_' {
                    '-'
                } else {
                    '\0'
                }
            })
            .filter(|&c| c != '\0')
            .collect();
        let slug = slug.trim_matches('-').to_string();
        let base = if slug.is_empty() {
            "profile".to_string()
        } else {
            slug
        };
        let mut candidate = base.clone();
        let mut suffix = 2;
        while self.profiles.iter().any(|p| p.id == candidate) {
            candidate = format!("{base}-{suffix}");
            suffix += 1;
        }
        candidate
    }
}

/// Normalizes a resolver game id for store comparison: the
/// resolver's `GAME_*` constants are hyphenated
/// (`"assetto-corsa-competizione"`) while the store's slugs are
/// underscored (`"assetto_corsa_competizione"`).
fn game_id_to_slug(game_id: &str) -> String {
    game_id.replace('-', "_")
}

impl ProfileSource for ProfileStore {
    /// Profile bound to a game: the first profile in list order whose
    /// game matches `game_id` and which binds no car.
    fn profile_for_game(&self, game_id: &str) -> Option<ProfileId> {
        let wanted = game_id_to_slug(game_id);
        self.profiles
            .iter()
            .find(|stored| {
                game_slug(stored.profile.game).is_some_and(|slug| slug == wanted)
                    && stored.profile.car_model.trim().is_empty()
            })
            .map(|stored| ProfileId::new(stored.id.clone()))
    }

    /// Profile bound to a car name in any game.
    fn profile_for_car(&self, car_name: &str) -> Option<ProfileId> {
        let wanted = normalize_car_model(car_name);
        if wanted.is_empty() {
            return None;
        }
        self.profiles
            .iter()
            .find(|stored| normalize_car_model(&stored.profile.car_model) == wanted)
            .map(|stored| ProfileId::new(stored.id.clone()))
    }

    /// Profile bound to a specific car in a specific game (the first
    /// lookup the resolver tries). Falls back to the car-only binding.
    fn profile_for_game_car(&self, game_id: &str, car_name: &str) -> Option<ProfileId> {
        let wanted = normalize_car_model(car_name);
        let wanted_game = game_id_to_slug(game_id);
        if !wanted.is_empty()
            && let Some(stored) = self.profiles.iter().find(|stored| {
                game_slug(stored.profile.game).is_some_and(|slug| slug == wanted_game)
                    && normalize_car_model(&stored.profile.car_model) == wanted
            })
        {
            return Some(ProfileId::new(stored.id.clone()));
        }
        self.profile_for_car(car_name)
    }

    /// The profile named `Default`, when one exists.
    fn default_profile(&self) -> Option<ProfileId> {
        self.profiles
            .iter()
            .find(|stored| stored.profile.name.eq_ignore_ascii_case("Default"))
            .map(|stored| ProfileId::new(stored.id.clone()))
    }
}

impl StoredProfile {
    /// The factory `Default` profile.
    #[must_use]
    pub fn default_profile() -> Self {
        let profile = WheelProfile {
            name: "Default".to_string(),
            ..WheelProfile::default()
        };
        Self {
            id: "default".to_string(),
            profile,
        }
    }
}

/// The backup list for `profiles.json` + `settings.json` in `dir`
/// (M5 — 6 rows: 3 generations per file), without opening the store
/// (so `list_backups` never fails on a corrupt `profiles.json` — a
/// missing generation simply reports `present: false`).
#[must_use]
pub fn list_backups_in(dir: &Path) -> Vec<BackupInfo> {
    let mut out = Vec::with_capacity(6);
    for file in ["profiles.json", "settings.json"] {
        for generation in [
            BackupGeneration::Current,
            BackupGeneration::Bak1,
            BackupGeneration::Bak2,
        ] {
            let path = dir.join(format!("{file}{}", generation.suffix()));
            let (present, saved_at) = match fs::metadata(&path) {
                Ok(metadata) => (
                    true,
                    metadata
                        .modified()
                        .ok()
                        .and_then(format_system_time_rfc3339),
                ),
                Err(_) => (false, None),
            };
            out.push(BackupInfo {
                generation: generation.as_str().to_string(),
                file: file.to_string(),
                present,
                saved_at,
            });
        }
    }
    out
}

/// Repair a corrupt store from its 3-generation backups (H3, Phase G
/// condition 16: corrupt-store startup was fail-closed with no repair
/// path at all).
///
/// Tries `bak1`, then `bak2`. For each generation it first restores
/// only `profiles.json` (the file that corrupts in practice — a crash
/// mid-save) and retries the open; only if that still fails does it
/// also restore `settings.json`, so a healthy settings file is never
/// clobbered by an older backup for no reason. Every live file that
/// gets overwritten is quarantined first (`<name>.corrupt`, or
/// `<name>.corrupt-<unix-seconds>` when one already exists), so the
/// corrupt bytes are never destroyed; if no generation repairs the
/// store the quarantined originals are put back, best effort, and the
/// error names the generations that were tried.
///
/// Returns `Ok(None)` when the store already opens (nothing to
/// repair), `Ok(Some(notice))` with a human-readable repair note when
/// a generation fixed it, and `Err(message)` when the store is still
/// unusable after both generations.
///
/// # Errors
///
/// Returns a message when the store cannot be opened even after both
/// backup generations were tried, or when a quarantine move fails.
pub fn try_repair_store_from_backup(dir: &Path) -> Result<Option<String>, String> {
    /// Quarantine `path` (if it exists) and record the move so the
    /// original can be put back when repair fails outright. The
    /// quarantine name is unique per attempt (unix seconds + process
    /// id + attempt counter, L7): two quarantines in the same second
    /// — or of the profiles and the settings file — can never collide
    /// and clobber each other.
    fn quarantine(
        path: &Path,
        moved: &mut Vec<(PathBuf, PathBuf)>,
        attempt: &mut u32,
    ) -> Result<(), String> {
        if !path.exists() {
            return Ok(());
        }
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let mut target = PathBuf::from(format!(
            "{}.corrupt-{}-{}-{}",
            path.display(),
            secs,
            std::process::id(),
            *attempt
        ));
        *attempt += 1;
        // Extremely defensive: if even that name exists, keep bumping.
        while target.exists() {
            target = PathBuf::from(format!(
                "{}.corrupt-{}-{}-{}",
                path.display(),
                secs,
                std::process::id(),
                *attempt
            ));
            *attempt += 1;
        }
        fs::rename(path, &target)
            .map_err(|error| format!("could not quarantine {}: {error}", path.display()))?;
        moved.push((target, path.to_path_buf()));
        Ok(())
    }

    if ProfileStore::open(dir).is_ok() {
        return Ok(None);
    }
    let open_error = ProfileStore::open(dir).err().map_or_else(
        || "unknown store error".to_string(),
        |error| error.to_string(),
    );

    let profiles = dir.join("profiles.json");
    let settings = dir.join("settings.json");
    let mut quarantined: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut attempt: u32 = 0;

    for generation in [BackupGeneration::Bak1, BackupGeneration::Bak2] {
        let profiles_backup = dir.join(format!("profiles.json{}", generation.suffix()));
        let settings_backup = dir.join(format!("settings.json{}", generation.suffix()));
        // Attempt 1: profiles only.
        if profiles_backup.exists() {
            quarantine(&profiles, &mut quarantined, &mut attempt)?;
            if fs::copy(&profiles_backup, &profiles).is_ok() && ProfileStore::open(dir).is_ok() {
                return Ok(Some(format!(
                    "Store repaired from backup '{}': profiles.json restored; \
                     the unreadable file was quarantined (kept as {}.corrupt-*)",
                    generation.as_str(),
                    profiles.display()
                )));
            }
        }
        // Attempt 2: profiles + settings.
        if settings_backup.exists() {
            quarantine(&settings, &mut quarantined, &mut attempt)?;
            if fs::copy(&settings_backup, &settings).is_ok() && ProfileStore::open(dir).is_ok() {
                return Ok(Some(format!(
                    "Store repaired from backup '{}': profiles.json and \
                     settings.json restored; the unreadable files were \
                     quarantined (kept as *.corrupt-*)",
                    generation.as_str()
                )));
            }
        }
    }

    // Nothing worked: put the quarantined originals back, best effort,
    // so a failed repair never destroys evidence.
    for (quarantine_path, original) in quarantined.iter().rev() {
        let _ = fs::remove_file(original);
        let _ = fs::rename(quarantine_path, original);
    }
    Err(format!(
        "profile store is corrupt ({open_error}); backups 'bak1' and 'bak2' \
         were tried and did not repair it — the original files were left in place"
    ))
}

/// Rotate the 3 generations for `path`: `path.bak1` → `path.bak2`,
/// then `path` → `path.bak1`. Missing files are skipped; a failed
/// rename is an error the caller must see (H6 — a silent rotation
/// failure used to leave the generations stale while the save
/// reported success). A failed `remove_file` of a stale `.bak2` is
/// also an error: the rotation cannot proceed safely without it.
fn rotate_backups(path: &Path) -> Result<(), StoreError> {
    let bak1 = PathBuf::from(format!("{}.bak1", path.display()));
    let bak2 = PathBuf::from(format!("{}.bak2", path.display()));
    if bak1.exists() {
        if bak2.exists() {
            fs::remove_file(&bak2)?;
        }
        fs::rename(&bak1, &bak2)?;
    }
    if path.exists() {
        fs::rename(path, &bak1)?;
    }
    Ok(())
}

/// Write `bytes` to `path` atomically: temp file in the same directory,
/// then rename over the target. The temp file is `fsync`ed before the
/// rename and the directory is `fsync`ed after it (H6 — without both,
/// a power cut can lose the rename itself, not just the bytes).
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    use std::io::Write as _;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let temp = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    let result = (|| -> Result<(), StoreError> {
        let mut file = fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)?;
        // Directory sync: makes the rename itself durable.
        if let Ok(dir) = fs::File::open(parent) {
            let _ = dir.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Re-export of the parsed C# profiles file type, for import reports.
pub type CsharpProfileFile = ProfileFile;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rsh-profiles-store-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn cleanup(dir: &Path) {
        let _ = fs::remove_dir_all(dir);
    }

    fn profile_with_game(name: &str, game_process: &str, car: &str) -> WheelProfile {
        WheelProfile {
            name: name.to_string(),
            game_process_name: game_process.to_string(),
            car_model: car.to_string(),
            ..WheelProfile::default()
        }
    }

    #[test]
    fn rfc3339_known_values() {
        assert_eq!(
            format_system_time_rfc3339(UNIX_EPOCH).as_deref(),
            Some("1970-01-01T00:00:00Z")
        );
        assert_eq!(
            format_system_time_rfc3339(UNIX_EPOCH + Duration::from_secs(1_000_000_000)).as_deref(),
            Some("2001-09-09T01:46:40Z")
        );
        assert_eq!(
            format_system_time_rfc3339(UNIX_EPOCH + Duration::from_secs(1_792_000_000)).as_deref(),
            Some("2026-10-14T17:46:40Z")
        );
        assert_eq!(
            format_system_time_rfc3339(UNIX_EPOCH - Duration::from_secs(1)),
            None
        );
    }

    #[test]
    fn normalize_process_and_car() {
        assert_eq!(normalize_process_name("acs.exe"), "acs");
        assert_eq!(normalize_process_name("  ACC.EXE "), "acc");
        assert_eq!(
            normalize_process_name("iRacingSim64DX11.exe"),
            "iracingsim64dx11"
        );
        assert_eq!(normalize_car_model("  Ferrari 488 GT3 "), "ferrari_488_gt3");
    }

    #[test]
    fn open_missing_files_gives_default_profile() {
        let dir = temp_dir();
        let store = ProfileStore::open(&dir).expect("open empty dir");
        assert_eq!(store.list().len(), 1);
        assert_eq!(store.list()[0].profile.name, "Default");
        assert_eq!(store.list()[0].id, "default");
        assert!(store.active().is_none());
        cleanup(&dir);
    }

    #[test]
    fn create_activate_save_reload_round_trip() {
        let dir = temp_dir();
        let mut store = ProfileStore::open(&dir).expect("open");
        let created = store.create("GT3 Setup").expect("create");
        assert_eq!(created.id, "gt3-setup");
        store.activate(&created.id, false).expect("activate");
        store.save().expect("save");
        assert!(store.profiles_path().exists());
        assert!(store.settings_path().exists());

        let reloaded = ProfileStore::open(&dir).expect("reload");
        assert_eq!(reloaded.list().len(), 2);
        assert_eq!(
            reloaded.active().map(|p| p.profile.name.as_str()),
            Some("GT3 Setup")
        );
        assert_eq!(reloaded.settings().theme, "System");
        cleanup(&dir);
    }

    #[test]
    fn create_rejects_bad_names() {
        let dir = temp_dir();
        let mut store = ProfileStore::open(&dir).expect("open");
        assert!(store.create("").is_err());
        assert!(store.create("   ").is_err());
        assert!(store.create(&"x".repeat(65)).is_err());
        assert!(store.create("bad/name").is_err());
        assert!(store.create("ok-name_1.0").is_ok());
        cleanup(&dir);
    }

    #[test]
    fn duplicate_and_delete() {
        let dir = temp_dir();
        let mut store = ProfileStore::open(&dir).expect("open");
        let created = store.create("Base").expect("create");
        store.activate(&created.id, false).expect("activate");
        let dup = store.duplicate(&created.id).expect("duplicate");
        assert_eq!(dup.profile.name, "Base copy");
        assert_ne!(dup.id, created.id);
        assert_eq!(store.list().len(), 3);
        store.delete(&created.id).expect("delete active");
        assert!(store.active().is_none());
        assert!(matches!(store.delete("nope"), Err(StoreError::NotFound(_))));
        cleanup(&dir);
    }

    #[test]
    fn update_validates_ranges() {
        let dir = temp_dir();
        let mut store = ProfileStore::open(&dir).expect("open");
        let created = store.create("Tune").expect("create");
        let bad = WheelProfile {
            name: "Tune".to_string(),
            gear_up_vjoy_button: 99, // D9: above 32 is rejected, never clamped
            ..WheelProfile::default()
        };
        assert!(store.update(&created.id, bad).is_err());
        let good = WheelProfile {
            name: "Tune".to_string(),
            gear_up_vjoy_button: 5,
            ..WheelProfile::default()
        };
        store.update(&created.id, good).expect("valid update");
        assert_eq!(
            store
                .get(&created.id)
                .expect("get")
                .profile
                .gear_up_vjoy_button,
            5
        );
        cleanup(&dir);
    }

    #[test]
    fn resolve_car_then_game_then_default() {
        let dir = temp_dir();
        let mut store = ProfileStore::open(&dir).expect("open");
        // Default profile already exists (game-less, no process).
        let mut game = profile_with_game("AC Game", "acs.exe", "");
        game.game = GameId::AssettoCorsa;
        let game_id = store.insert_imported(game).id;
        let mut car = profile_with_game("AC Ferrari", "acs.exe", "Ferrari 488 GT3");
        car.game = GameId::AssettoCorsa;
        let car_id = store.insert_imported(car).id;
        // Car-specific wins over game-only.
        assert_eq!(
            store
                .resolve_for_game("acs.exe", "Ferrari 488 GT3")
                .map(|p| p.id.as_str()),
            Some(car_id.as_str())
        );
        // Other car falls back to the game profile.
        assert_eq!(
            store
                .resolve_for_game("acs.exe", "Porsche 911")
                .map(|p| p.id.as_str()),
            Some(game_id.as_str())
        );
        // Unknown game falls back to Default.
        assert_eq!(
            store
                .resolve_for_game("unknown.exe", "")
                .map(|p| p.profile.name.as_str()),
            Some("Default")
        );
        // Case-insensitive process + car match.
        assert_eq!(
            store
                .resolve_for_game("ACS.EXE", "ferrari 488 gt3")
                .map(|p| p.id.as_str()),
            Some(car_id.as_str())
        );
        cleanup(&dir);
    }

    #[test]
    fn resolve_first_duplicate_process_wins_and_manual_lock_blocks() {
        let dir = temp_dir();
        let mut store = ProfileStore::open(&dir).expect("open");
        let first = store.insert_imported(profile_with_game("First", "acs.exe", ""));
        let _second = store.insert_imported(profile_with_game("Second", "acs.exe", ""));
        assert_eq!(
            store.resolve_for_game("acs.exe", "").map(|p| p.id.as_str()),
            Some(first.id.as_str())
        );
        store.activate(&first.id, true).expect("manual activate");
        assert!(store.auto_switch_locked());
        assert_eq!(store.resolve_for_game("acs.exe", ""), None);
        store.enable_auto_switch();
        assert!(!store.auto_switch_locked());
        assert_eq!(
            store.resolve_for_game("acs.exe", "").map(|p| p.id.as_str()),
            Some(first.id.as_str())
        );
        cleanup(&dir);
    }

    #[test]
    fn backup_rotation_three_generations_and_restore() {
        let dir = temp_dir();
        let mut store = ProfileStore::open(&dir).expect("open");
        store.create("V1").expect("v1");
        store.save().expect("save v1");
        store.create("V2").expect("v2");
        store.save().expect("save v2");
        store.create("V3").expect("v3");
        store.save().expect("save v3");

        let backups = store.list_backups();
        // M5: 3 generations per file — profiles first, then settings.
        assert_eq!(backups.len(), 6);
        assert!(backups[0].present);
        assert!(backups[1].present);
        assert!(backups[2].present);
        assert!(backups[0].saved_at.is_some());
        assert_eq!(backups[0].generation, "current");
        assert_eq!(backups[0].file, "profiles.json");
        assert_eq!(backups[1].generation, "bak1");
        assert_eq!(backups[3].file, "settings.json");
        assert_eq!(backups[3].generation, "current");

        // bak1 holds V2 (2 profiles + Default), bak2 holds V1.
        let bak1 = ProfileStore::read_profiles_file(&store.backup_path(BackupGeneration::Bak1))
            .expect("read bak1");
        assert_eq!(bak1.0.len(), 3); // Default + V1 + V2
        let bak2 = ProfileStore::read_profiles_file(&store.backup_path(BackupGeneration::Bak2))
            .expect("read bak2");
        assert_eq!(bak2.0.len(), 2); // Default + V1

        // Restore bak2 -> live file holds V1 only.
        store
            .restore_backup(BackupGeneration::Bak2)
            .expect("restore bak2");
        assert_eq!(store.list().len(), 2);
        assert!(store.list().iter().any(|p| p.profile.name == "V1"));
        assert!(!store.list().iter().any(|p| p.profile.name == "V3"));
        cleanup(&dir);
    }

    #[test]
    fn backup_now_rotates_without_changing_content() {
        let dir = temp_dir();
        let mut store = ProfileStore::open(&dir).expect("open");
        store.create("Only").expect("create");
        store.save().expect("save");
        let backups = store.backup_now().expect("backup now");
        assert!(backups[1].present); // current rotated into bak1
        assert!(!backups[2].present);
        // Content unchanged: still 2 profiles live.
        assert_eq!(store.list().len(), 2);
        cleanup(&dir);
    }

    #[test]
    fn restore_missing_generation_is_not_found() {
        let dir = temp_dir();
        let mut store = ProfileStore::open(&dir).expect("open");
        assert!(matches!(
            store.restore_backup(BackupGeneration::Bak2),
            Err(StoreError::NotFound(_))
        ));
        cleanup(&dir);
    }

    #[test]
    fn settings_persist_and_rotate_backups() {
        let dir = temp_dir();
        let mut store = ProfileStore::open(&dir).expect("open");
        let settings = AppSettings {
            theme: "Dark".to_string(),
            esp_ip: "10.0.0.5".to_string(),
            ..AppSettings::default()
        };
        store.set_settings(settings);
        store.save_settings().expect("save settings");
        let reloaded = ProfileStore::open(&dir).expect("reload");
        assert_eq!(reloaded.settings().theme, "Dark");
        assert_eq!(reloaded.settings().esp_ip, "10.0.0.5");
        // Second save rotates the first settings file into .bak1.
        store.save_settings().expect("save settings again");
        assert!(dir.join("settings.json.bak1").exists());
        cleanup(&dir);
    }

    #[test]
    fn export_import_round_trip_and_name_clash_suffix() {
        let dir = temp_dir();
        let mut store = ProfileStore::open(&dir).expect("open");
        let created = store.create("Exported").expect("create");
        let export_path = dir.join("exported.json");
        store
            .export_profile(&created.id, &export_path)
            .expect("export");
        assert!(export_path.exists());
        let imported = store.import_profile(&export_path).expect("import");
        assert_eq!(imported.profile.name, "Exported (2)");
        assert_ne!(imported.id, created.id);
        // Re-import into a fresh store keeps the original name.
        let dir2 = temp_dir();
        let mut store2 = ProfileStore::open(&dir2).expect("open2");
        store2.delete("default").expect("delete default");
        let imported2 = store2.import_profile(&export_path).expect("import2");
        assert_eq!(imported2.profile.name, "Exported");
        cleanup(&dir);
        cleanup(&dir2);
    }

    #[test]
    fn import_rejects_oversized_and_future_schema() {
        let dir = temp_dir();
        let mut store = ProfileStore::open(&dir).expect("open");
        let big_path = dir.join("big.json");
        fs::write(&big_path, "x".repeat(1_048_577)).expect("write big");
        assert!(matches!(
            store.import_profile(&big_path),
            Err(StoreError::TooLarge { .. })
        ));
        let future_path = dir.join("future.json");
        fs::write(
            &future_path,
            r#"{"schema_version": 99, "profile": {"id": "x", "profile": {"name": "X"}}}"#,
        )
        .expect("write future");
        assert!(matches!(
            store.import_profile(&future_path),
            Err(StoreError::Schema(_))
        ));
        cleanup(&dir);
    }

    #[test]
    fn import_csharp_profiles_and_settings() {
        let dir = temp_dir();
        let profiles_path = dir.join("csharp-profiles.json");
        fs::write(
            &profiles_path,
            r#"{
              "SchemaVersion": 2,
              "Profiles": [
                {"Name": "CSharpOne", "Game": 1, "GameProcessName": "acs.exe", "CarModel": "", "Deadzone": 0.02}
              ],
              "ActiveProfileName": "CSharpOne"
            }"#,
        )
        .expect("write csharp profiles");
        let mut store = ProfileStore::open(&dir).expect("open");
        let file = store
            .import_csharp_profiles(&profiles_path)
            .expect("import csharp");
        assert_eq!(file.profiles.len(), 1);
        assert_eq!(file.skipped_profiles, 0);
        assert_eq!(
            store.active().map(|p| p.profile.name.as_str()),
            Some("CSharpOne")
        );
        assert_eq!(
            store
                .resolve_for_game("acs.exe", "")
                .map(|p| p.profile.name.as_str()),
            Some("CSharpOne")
        );

        let settings_path = dir.join("csharp-settings.json");
        fs::write(
            &settings_path,
            r#"{"Theme": "Dark", "Language": "en", "EspIp": "10.0.0.5"}"#,
        )
        .expect("write csharp settings");
        let settings = store
            .import_csharp_settings(&settings_path)
            .expect("import settings");
        assert_eq!(settings.theme, "Dark");
        assert_eq!(settings.esp_ip, "10.0.0.5");
        cleanup(&dir);
    }

    #[test]
    fn stored_profile_json_round_trip() {
        let stored = StoredProfile::default_profile();
        let json = serde_json::to_string(&stored).expect("serialize");
        assert!(json.contains("\"name\":\"Default\""), "{json}");
        let back: StoredProfile = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, stored);
        // GameId serializes as a number (C# shape).
        let profile = WheelProfile {
            game: GameId::Acc,
            ..WheelProfile::default()
        };
        let json = serde_json::to_string(&profile).expect("serialize profile");
        assert!(json.contains("\"game\":2"), "{json}");
        let back: WheelProfile = serde_json::from_str(&json).expect("deserialize profile");
        assert_eq!(back.game, GameId::Acc);
    }

    #[test]
    fn corrupt_profiles_file_is_an_error_not_silent_defaults() {
        let dir = temp_dir();
        fs::write(dir.join("profiles.json"), "{not json").expect("write corrupt");
        assert!(matches!(ProfileStore::open(&dir), Err(StoreError::Json(_))));
        cleanup(&dir);
    }

    #[test]
    fn backup_info_serializes_to_frontend_shape() {
        let info = BackupInfo {
            generation: "bak1".to_string(),
            file: "profiles.json".to_string(),
            present: true,
            saved_at: Some("2026-11-14T12:00:00Z".to_string()),
        };
        let json = serde_json::to_string(&info).expect("serialize");
        assert!(json.contains("\"generation\":\"bak1\""), "{json}");
        assert!(json.contains("\"file\":\"profiles.json\""), "{json}");
        assert!(
            json.contains("\"savedAt\":\"2026-11-14T12:00:00Z\""),
            "{json}"
        );
    }

    /// A healthy store needs no repair (H3 corrupt-store repair).
    #[test]
    fn repair_healthy_store_is_noop() {
        let dir = temp_dir();
        let mut store = ProfileStore::open(&dir).expect("open empty dir");
        store.create("GT3 Setup").expect("create profile");
        store.save().expect("save");
        assert_eq!(try_repair_store_from_backup(&dir), Ok(None));
        cleanup(&dir);
    }

    /// Corrupt `profiles.json` + good `bak1` repairs, quarantines the
    /// corrupt bytes, and the repaired store holds the backup's data.
    #[test]
    fn repair_corrupt_profiles_from_bak1() {
        let dir = temp_dir();
        let mut store = ProfileStore::open(&dir).expect("open empty dir");
        store.create("GT3 Setup").expect("create profile");
        store.save().expect("save");
        fs::copy(dir.join("profiles.json"), dir.join("profiles.json.bak1")).expect("stage bak1");
        fs::write(dir.join("profiles.json"), "{not json").expect("corrupt");
        assert!(ProfileStore::open(&dir).is_err());
        let notice = try_repair_store_from_backup(&dir)
            .expect("repair succeeds")
            .expect("repair reports a notice");
        assert!(notice.contains("bak1"), "{notice}");
        // Quarantine name is unique per attempt (L7):
        // `profiles.json.corrupt-<secs>-<pid>-<n>`.
        let quarantined = fs::read_dir(&dir)
            .expect("read dir")
            .flatten()
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("profiles.json.corrupt-")
            });
        assert!(quarantined, "corrupt file must be quarantined");
        let repaired = ProfileStore::open(&dir).expect("repaired store opens");
        assert!(
            repaired
                .list()
                .iter()
                .any(|p| p.profile.name == "GT3 Setup")
        );
        cleanup(&dir);
    }

    /// Corrupt `profiles.json` + corrupt `bak1` + good `bak2` falls
    /// through to `bak2` instead of giving up.
    #[test]
    fn repair_falls_through_to_bak2() {
        let dir = temp_dir();
        let mut store = ProfileStore::open(&dir).expect("open empty dir");
        store.create("GT3 Setup").expect("create profile");
        store.save().expect("save");
        fs::copy(dir.join("profiles.json"), dir.join("profiles.json.bak2")).expect("stage bak2");
        fs::write(dir.join("profiles.json.bak1"), "{also not json").expect("corrupt bak1");
        fs::write(dir.join("profiles.json"), "{not json").expect("corrupt");
        let notice = try_repair_store_from_backup(&dir)
            .expect("repair succeeds")
            .expect("repair reports a notice");
        assert!(notice.contains("bak2"), "{notice}");
        let repaired = ProfileStore::open(&dir).expect("repaired store opens");
        assert!(
            repaired
                .list()
                .iter()
                .any(|p| p.profile.name == "GT3 Setup")
        );
        cleanup(&dir);
    }

    /// No usable backup: repair fails honestly and the corrupt original
    /// is put back byte-identical (evidence is never destroyed).
    #[test]
    fn repair_without_backups_restores_original() {
        let dir = temp_dir();
        fs::write(dir.join("profiles.json"), "{not json").expect("corrupt");
        let error = try_repair_store_from_backup(&dir).expect_err("no backup");
        assert!(error.contains("bak1"), "{error}");
        assert_eq!(
            fs::read_to_string(dir.join("profiles.json")).expect("original back"),
            "{not json"
        );
        cleanup(&dir);
    }

    // -- Deep-scan Phase 2 regressions -----------------------------------

    /// H5: an invalid entry is rejected on load (with a warning), a
    /// duplicate id/name is dropped (first wins), and an all-invalid
    /// list recreates the Default profile instead of opening empty.
    #[test]
    fn load_validation_rejects_invalid_and_duplicates_recreates_default() {
        let dir = temp_dir();
        let good = profile_with_game("Good", "acs.exe", "");
        let mut bad = profile_with_game("Bad", "acs.exe", "");
        bad.deadzone = 9.9; // outside 0..=0.5
        let mut dup_name = profile_with_game("good", "acc.exe", "");
        dup_name.deadzone = 0.1;
        let file = serde_json::json!({
            "schema_version": 2,
            "profiles": [
                {"id": "good", "profile": good},
                {"id": "bad", "profile": bad},
                {"id": "good-2", "profile": dup_name},
                {"id": "good", "profile": profile_with_game("Also Good", "acs.exe", "")},
            ],
            "active_profile_id": "bad",
            "auto_switch_locked": false,
        });
        fs::write(
            dir.join("profiles.json"),
            serde_json::to_string(&file).expect("fixture json"),
        )
        .expect("write fixture");
        let store = ProfileStore::open(&dir).expect("open validates, never errors on entries");
        assert_eq!(store.list().len(), 1, "only the valid entry survives");
        assert_eq!(store.list()[0].profile.name, "Good");
        assert!(
            store.active().is_none(),
            "active id pointed at a rejected entry"
        );
        assert!(
            store.load_warnings().len() >= 3,
            "warnings: {:?}",
            store.load_warnings()
        );

        // All-invalid list recreates Default.
        let dir2 = temp_dir();
        let mut only_bad = profile_with_game("Bad", "acs.exe", "");
        only_bad.deadzone = 9.9;
        let file2 = serde_json::json!({
            "schema_version": 2,
            "profiles": [{"id": "bad", "profile": only_bad}],
            "active_profile_id": null,
            "auto_switch_locked": false,
        });
        fs::write(
            dir2.join("profiles.json"),
            serde_json::to_string(&file2).expect("fixture json"),
        )
        .expect("write fixture");
        let store2 = ProfileStore::open(&dir2).expect("open recreates Default");
        assert_eq!(store2.list().len(), 1);
        assert_eq!(store2.list()[0].profile.name, "Default");
        assert!(
            store2
                .load_warnings()
                .iter()
                .any(|w| w.contains("Default profile was recreated")),
            "{:?}",
            store2.load_warnings()
        );
        cleanup(&dir);
        cleanup(&dir2);
    }

    /// L3: creating a second profile with the same name (any case) is
    /// rejected, and duplicating a 64-char name stays within the cap.
    #[test]
    fn create_rejects_duplicate_name_and_duplicate_fits_64_chars() {
        let dir = temp_dir();
        let mut store = ProfileStore::open(&dir).expect("open");
        store.create("GT3 Setup").expect("create");
        assert!(matches!(
            store.create("gt3 setup"),
            Err(StoreError::Validation(_))
        ));
        let long = "x".repeat(64);
        let created = store.create(&long).expect("64-char name creates");
        let dup = store.duplicate(&created.id).expect("duplicate fits");
        assert!(dup.profile.name.chars().count() <= MAX_PROFILE_NAME_LEN);
        assert!(dup.profile.name.ends_with("copy") || dup.profile.name.ends_with(')'));
        cleanup(&dir);
    }

    /// C-SET-1: a patch overwrites only the provided fields; absent
    /// fields keep their values (a partial save never wipes settings).
    #[test]
    fn settings_patch_merges_without_wiping() {
        let dir = temp_dir();
        let mut store = ProfileStore::open(&dir).expect("open");
        store.set_settings(AppSettings {
            theme: "Dark".to_string(),
            esp_ip: "10.0.0.5".to_string(),
            ..AppSettings::default()
        });
        let patch = AppSettingsPatch {
            theme: Some("light".to_string()),
            ..AppSettingsPatch::default()
        };
        let merged = store.set_settings_patch(patch);
        assert_eq!(merged.theme, "Light", "lowercase theme maps to canonical");
        assert_eq!(merged.esp_ip, "10.0.0.5", "absent esp_ip is kept");
        assert_eq!(merged.output_backend, "VJoy");
        cleanup(&dir);
    }

    /// M10: the Rust load path sanitizes — a hand-edited public
    /// `esp_ip` and a lowercase theme are corrected on open.
    #[test]
    fn rust_load_sanitizes_settings() {
        let dir = temp_dir();
        fs::write(
            dir.join("settings.json"),
            r#"{"theme": "dark", "esp_ip": "8.8.8.8", "vjoy_device_id": 99}"#,
        )
        .expect("write settings");
        let store = ProfileStore::open(&dir).expect("open sanitizes");
        assert_eq!(store.settings().theme, "Dark");
        assert_eq!(store.settings().esp_ip, "192.168.4.1");
        assert_eq!(store.settings().vjoy_device_id, 16);
        cleanup(&dir);
    }

    /// C1: the store implements `ProfileSource` — car-in-game beats
    /// game-only beats Default, with the canonical normalizer.
    #[test]
    fn store_implements_profile_source() {
        use crate::resolver::{
            CarProfileResolver, GAME_ASSETTO_CORSA_COMPETIZIONE, MapProfileSource, ProfileId,
            ProfileSource as _,
        };
        let dir = temp_dir();
        let mut store = ProfileStore::open(&dir).expect("open");
        let mut game = profile_with_game("ACC Game", "acc.exe", "");
        game.game = GameId::Acc;
        store.insert_imported(game);
        let mut car = profile_with_game("ACC Ferrari", "acc.exe", "Ferrari 488 GT3");
        car.game = GameId::Acc;
        let car_id = store.insert_imported(car).id;
        let _ = MapProfileSource::new();
        assert_eq!(
            store.profile_for_game_car(GAME_ASSETTO_CORSA_COMPETIZIONE, "Ferrari 488 GT3 Evo"),
            None,
            "different car must not match the Ferrari binding"
        );
        assert_eq!(
            store.profile_for_game_car(GAME_ASSETTO_CORSA_COMPETIZIONE, "Ferrari 488 GT3"),
            Some(ProfileId::new(car_id.clone()))
        );
        assert_eq!(
            store.profile_for_car("  FERRARI--488__GT3 "),
            Some(ProfileId::new(car_id)),
            "canonical normalizer unifies separators"
        );
        assert!(
            store
                .profile_for_game(GAME_ASSETTO_CORSA_COMPETIZIONE)
                .is_some()
        );
        assert_eq!(
            store.default_profile(),
            Some(ProfileId::new("default".to_string()))
        );
        // The resolver drives the same source end to end.
        let resolver = CarProfileResolver::new(&store);
        assert_eq!(
            resolver.resolve(
                Some(GAME_ASSETTO_CORSA_COMPETIZIONE),
                Some("Ferrari 488 GT3")
            ),
            store.profile_for_game_car(GAME_ASSETTO_CORSA_COMPETIZIONE, "Ferrari 488 GT3")
        );
        cleanup(&dir);
    }

    /// M5/M9: the backup list covers settings generations too, and a
    /// real settings backup path restores the settings.
    #[test]
    fn backups_cover_settings_and_real_path_restore_works() {
        let dir = temp_dir();
        let mut store = ProfileStore::open(&dir).expect("open");
        store.set_settings(AppSettings {
            theme: "Dark".to_string(),
            ..AppSettings::default()
        });
        store.save().expect("save");
        store.set_settings(AppSettings {
            theme: "Light".to_string(),
            ..AppSettings::default()
        });
        store.save().expect("save again");
        let backups = store.list_backups();
        assert_eq!(backups.len(), 6);
        assert!(
            backups
                .iter()
                .any(|b| b.file == "settings.json" && b.present)
        );
        let settings_bak1 = dir.join("settings.json.bak1");
        assert!(settings_bak1.exists());
        store
            .restore_backup_path(&settings_bak1)
            .expect("restore settings bak1");
        assert_eq!(store.settings().theme, "Dark");
        // A path outside the store directory is refused.
        let outside = std::env::temp_dir().join("rsh-outside-settings.json.bak1");
        assert!(matches!(
            store.restore_backup_path(&outside),
            Err(StoreError::NotFound(_))
        ));
        cleanup(&dir);
    }

    /// M6: a C# settings import merges — only present fields win.
    #[test]
    fn csharp_settings_import_merges() {
        let dir = temp_dir();
        let mut store = ProfileStore::open(&dir).expect("open");
        store.set_settings(AppSettings {
            theme: "Dark".to_string(),
            esp_ip: "10.0.0.5".to_string(),
            ..AppSettings::default()
        });
        let csharp = dir.join("csharp-settings.json");
        fs::write(&csharp, r#"{"Theme": "Light", "VJoyDeviceId": 4}"#).expect("write csharp");
        let merged = store
            .import_csharp_settings(&csharp)
            .expect("import merges");
        assert_eq!(merged.theme, "Light");
        assert_eq!(merged.vjoy_device_id, 4);
        assert_eq!(merged.esp_ip, "10.0.0.5", "absent field is kept");
        cleanup(&dir);
    }

    /// H3: an automatic activation clears the manual-override lock.
    #[test]
    fn auto_activation_clears_manual_lock() {
        let dir = temp_dir();
        let mut store = ProfileStore::open(&dir).expect("open");
        let created = store.create("Manual").expect("create");
        store.activate(&created.id, true).expect("manual");
        assert!(store.auto_switch_locked());
        store.activate(&created.id, false).expect("auto");
        assert!(!store.auto_switch_locked());
        cleanup(&dir);
    }
}
