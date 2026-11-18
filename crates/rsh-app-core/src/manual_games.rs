//! Manually added games (game-scanner removal, v2.0.3+).
//!
//! The automatic game scanner is gone (user decision 2026-11-18):
//! no Steam/iRacing detection, no registry reads, no `tasklist`.
//! The user adds games by hand — a display name plus an exe or
//! shortcut (`.lnk`) path — and they are stored here, in
//! `manual-games.json` next to `profiles.json` / `bindings.json`.
//!
//! Persistence mirrors `rsh_profiles::bindings`: a missing file
//! means no games; a corrupt, oversize, or newer-schema file loads
//! as empty with warnings (never fails the state open). Saves are
//! atomic (temp file in the same directory, then rename).
//!
//! Each entry has a stable slug id (derived from the name at add
//! time, never re-slugged on rename — the id is what profiles bind
//! to and what the launcher tracks). `bound_profile_id` is the
//! profile bound to this entry, if any; profile existence is
//! validated by the caller (`AppStateCore`), not here.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// On-disk schema version for `manual-games.json`.
const MANUAL_GAMES_SCHEMA_VERSION: u32 = 1;

/// Size cap for `manual-games.json` (64 KiB — far above any real
/// list: a few dozen entries of short strings).
const MANUAL_GAMES_MAX_BYTES: u64 = 64 * 1024;

/// Maximum number of manual games (keeps the list — and the
/// Overview tiles — sane).
const MAX_MANUAL_GAMES: usize = 64;

/// File name for the manual-games store, inside the data dir.
pub const MANUAL_GAMES_FILE_NAME: &str = "manual-games.json";

/// One manually added game.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ManualGame {
    /// Stable slug id (derived from the name at add time, e.g.
    /// `"my-racing-game"`). Never changes after creation.
    pub id: String,
    /// Display name, as the user typed it.
    pub name: String,
    /// Full path to the game exe or shortcut (`.lnk`), as the user
    /// picked or typed it.
    pub exe_path: String,
    /// Id of the profile bound to this game, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound_profile_id: Option<String>,
}

/// On-disk shape of `manual-games.json` (`schemaVersion` camelCase,
/// like `bindings.json`).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ManualGamesFile {
    schema_version: u32,
    #[serde(default)]
    games: Vec<ManualGame>,
}

/// The persisted manual-games store.
pub struct ManualGameStore {
    /// Directory holding `manual-games.json`.
    dir: PathBuf,
    /// Entries in add order.
    games: Vec<ManualGame>,
    /// Load-time warnings (dropped entries, corrupt file). Empty
    /// on a clean load; surfaced by the daemon as a startup notice,
    /// never silently swallowed.
    load_warnings: Vec<String>,
}

impl ManualGameStore {
    /// Open the store in `dir`, loading `manual-games.json` when it
    /// exists. A missing file means no games; a corrupt, oversize,
    /// or newer-schema file loads as empty with a warning (see the
    /// module docs) — this never fails.
    #[must_use]
    pub fn open(dir: &Path) -> Self {
        let path = dir.join(MANUAL_GAMES_FILE_NAME);
        let (games, load_warnings) = if path.exists() {
            Self::read_file(&path)
        } else {
            (Vec::new(), Vec::new())
        };
        Self {
            dir: dir.to_path_buf(),
            games,
            load_warnings,
        }
    }

    /// Load-time warnings (empty on a clean load).
    #[must_use]
    pub fn load_warnings(&self) -> &[String] {
        &self.load_warnings
    }

    /// Entries in add order.
    #[must_use]
    pub fn list(&self) -> &[ManualGame] {
        &self.games
    }

    /// One entry by id, if it exists.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&ManualGame> {
        self.games.iter().find(|game| game.id == id)
    }

    /// Add a game: validates the name / exe path, derives a unique
    /// slug id from the name, persists, and returns the entry.
    ///
    /// # Errors
    ///
    /// Returns a message when the name or exe path is empty, the
    /// list is full, or the save fails.
    pub fn add(
        &mut self,
        name: &str,
        exe_path: &str,
        bound_profile_id: Option<String>,
    ) -> Result<ManualGame, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("game name must not be empty".to_string());
        }
        let exe_path = exe_path.trim();
        if exe_path.is_empty() {
            return Err("game exe path must not be empty".to_string());
        }
        if self.games.len() >= MAX_MANUAL_GAMES {
            return Err(format!("too many games (maximum {MAX_MANUAL_GAMES})"));
        }
        let id = self.unique_id(&slugify(name));
        let game = ManualGame {
            id,
            name: name.to_string(),
            exe_path: exe_path.to_string(),
            bound_profile_id: bound_profile_id.filter(|id| !id.trim().is_empty()),
        };
        self.games.push(game.clone());
        self.save()?;
        Ok(game)
    }

    /// Update a game's name / exe path / bound profile. The id is
    /// stable and never changes. Persists and returns the entry.
    ///
    /// # Errors
    ///
    /// Returns a message for an unknown id, an empty name or exe
    /// path, or a save failure.
    pub fn update(
        &mut self,
        id: &str,
        name: &str,
        exe_path: &str,
        bound_profile_id: Option<String>,
    ) -> Result<ManualGame, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("game name must not be empty".to_string());
        }
        let exe_path = exe_path.trim();
        if exe_path.is_empty() {
            return Err("game exe path must not be empty".to_string());
        }
        let Some(game) = self.games.iter_mut().find(|game| game.id == id) else {
            return Err(format!("unknown game '{id}'"));
        };
        game.name = name.to_string();
        game.exe_path = exe_path.to_string();
        game.bound_profile_id = bound_profile_id.filter(|id| !id.trim().is_empty());
        let game = game.clone();
        self.save()?;
        Ok(game)
    }

    /// Remove a game by id and persist.
    ///
    /// # Errors
    ///
    /// Returns a message for an unknown id or a save failure.
    pub fn remove(&mut self, id: &str) -> Result<(), String> {
        let Some(pos) = self.games.iter().position(|game| game.id == id) else {
            return Err(format!("unknown game '{id}'"));
        };
        self.games.remove(pos);
        self.save()
    }

    /// Bind (`Some`) or unbind (`None`) a profile for a game.
    /// Persists and returns the entry.
    ///
    /// # Errors
    ///
    /// Returns a message for an unknown id or a save failure.
    pub fn set_profile(
        &mut self,
        id: &str,
        bound_profile_id: Option<String>,
    ) -> Result<ManualGame, String> {
        let Some(game) = self.games.iter_mut().find(|game| game.id == id) else {
            return Err(format!("unknown game '{id}'"));
        };
        game.bound_profile_id = bound_profile_id.filter(|id| !id.trim().is_empty());
        let game = game.clone();
        self.save()?;
        Ok(game)
    }

    /// A unique id for a slug base: the base itself when free,
    /// otherwise `base-2`, `base-3`, ….
    fn unique_id(&self, base: &str) -> String {
        if self.get(base).is_none() {
            return base.to_string();
        }
        let mut n = 2;
        loop {
            let candidate = format!("{base}-{n}");
            if self.get(&candidate).is_none() {
                return candidate;
            }
            n += 1;
        }
    }

    /// Read + validate a `manual-games.json`-shaped file. Invalid
    /// entries are dropped with a warning; a corrupt, oversize, or
    /// newer-schema file loads as empty with a warning.
    fn read_file(path: &Path) -> (Vec<ManualGame>, Vec<String>) {
        let mut warnings = Vec::new();
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) => {
                warnings.push(format!(
                    "manual games file unreadable ({error}): starting with no games"
                ));
                return (Vec::new(), warnings);
            }
        };
        if bytes.len() as u64 > MANUAL_GAMES_MAX_BYTES {
            warnings.push(format!(
                "manual games file over the {} KiB cap: starting with no games",
                MANUAL_GAMES_MAX_BYTES / 1024
            ));
            return (Vec::new(), warnings);
        }
        let file: ManualGamesFile = match serde_json::from_slice(&bytes) {
            Ok(file) => file,
            Err(error) => {
                warnings.push(format!(
                    "manual games file corrupt ({error}): starting with no games"
                ));
                return (Vec::new(), warnings);
            }
        };
        if file.schema_version > MANUAL_GAMES_SCHEMA_VERSION {
            warnings.push(format!(
                "manual games file schema v{} is newer than supported v{MANUAL_GAMES_SCHEMA_VERSION}: starting with no games",
                file.schema_version
            ));
            return (Vec::new(), warnings);
        }
        let mut games = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for game in file.games {
            let id = game.id.trim();
            let name = game.name.trim();
            let exe_path = game.exe_path.trim();
            if id.is_empty() || name.is_empty() || exe_path.is_empty() {
                warnings
                    .push("dropped a manual game with an empty id, name, or exe path".to_string());
                continue;
            }
            if !seen.insert(id.to_string()) {
                warnings.push(format!("dropped duplicate manual game id '{id}'"));
                continue;
            }
            games.push(ManualGame {
                id: id.to_string(),
                name: name.to_string(),
                exe_path: exe_path.to_string(),
                bound_profile_id: game.bound_profile_id.filter(|id| !id.trim().is_empty()),
            });
            if games.len() >= MAX_MANUAL_GAMES {
                warnings.push(format!(
                    "manual games file holds more than {MAX_MANUAL_GAMES} entries: kept the first {MAX_MANUAL_GAMES}"
                ));
                break;
            }
        }
        (games, warnings)
    }

    /// Write the store to `manual-games.json` atomically (temp file
    /// in the same directory, then rename).
    fn save(&self) -> Result<(), String> {
        let file = ManualGamesFile {
            schema_version: MANUAL_GAMES_SCHEMA_VERSION,
            games: self.games.clone(),
        };
        let json = serde_json::to_string_pretty(&file)
            .map_err(|error| format!("cannot serialize manual games: {error}"))?;
        fs::create_dir_all(&self.dir).map_err(|error| {
            format!(
                "cannot create manual games directory '{}': {error}",
                self.dir.display()
            )
        })?;
        let path = self.dir.join(MANUAL_GAMES_FILE_NAME);
        write_atomic(&path, json.as_bytes())
            .map_err(|error| format!("cannot save manual games: {error}"))
    }
}

/// Slugify a display name into a stable id: lowercase, runs of
/// non-alphanumeric characters become one `-`, leading/trailing
/// `-` trimmed. Empty input (or input with no alphanumeric
/// characters) becomes `"game"`.
fn slugify(name: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = true; // trims leading dashes
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        "game".to_string()
    } else {
        slug
    }
}

/// Write `bytes` to `path` atomically: a temp file in the same
/// directory, then rename. A crash mid-save never leaves a
/// half-written `manual-games.json`. (Same pattern as the profile
/// store and the bindings store.)
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
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

    fn test_dir(name: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "rsh-manual-games-test-{}-{name}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create test dir");
        dir
    }

    #[test]
    fn empty_store_adds_and_persists() {
        let dir = test_dir("add");
        let mut store = ManualGameStore::open(&dir);
        assert_eq!(store.list(), []);
        let game = store
            .add("My Racing Game", r"C:\Games\race.exe", None)
            .expect("add");
        assert_eq!(game.id, "my-racing-game");
        assert_eq!(game.name, "My Racing Game");
        assert!(dir.join(MANUAL_GAMES_FILE_NAME).exists());
        // Re-open: the entry survives.
        let store = ManualGameStore::open(&dir);
        assert_eq!(store.list().len(), 1);
        assert_eq!(
            store.get("my-racing-game").unwrap().exe_path,
            r"C:\Games\race.exe"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn duplicate_names_get_unique_ids() {
        let dir = test_dir("dupe");
        let mut store = ManualGameStore::open(&dir);
        let first = store.add("Race", r"C:\a.exe", None).expect("add 1");
        let second = store.add("Race", r"C:\b.exe", None).expect("add 2");
        let third = store.add("Race!", r"C:\c.exe", None).expect("add 3");
        assert_eq!(first.id, "race");
        assert_eq!(second.id, "race-2");
        assert_eq!(third.id, "race-3");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn validation_rejects_empty_name_and_path() {
        let dir = test_dir("valid");
        let mut store = ManualGameStore::open(&dir);
        assert!(store.add("", r"C:\a.exe", None).is_err());
        assert!(store.add("  ", r"C:\a.exe", None).is_err());
        assert!(store.add("Race", "", None).is_err());
        assert!(store.add("Race", "   ", None).is_err());
        assert_eq!(store.list(), []);
        assert!(store.update("nope", "Race", r"C:\a.exe", None).is_err());
        assert!(store.remove("nope").is_err());
        assert!(store.set_profile("nope", None).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn update_keeps_stable_id_and_binds_profile() {
        let dir = test_dir("update");
        let mut store = ManualGameStore::open(&dir);
        let game = store.add("Race", r"C:\a.exe", None).expect("add");
        let updated = store
            .update(
                &game.id,
                "Race 2",
                r"D:\race2.exe",
                Some("profile-1".to_string()),
            )
            .expect("update");
        assert_eq!(updated.id, game.id, "id never changes on update");
        assert_eq!(updated.name, "Race 2");
        assert_eq!(updated.bound_profile_id.as_deref(), Some("profile-1"));
        let cleared = store.set_profile(&game.id, None).expect("unbind");
        assert!(cleared.bound_profile_id.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_file_loads_empty_with_warning() {
        let dir = test_dir("corrupt");
        std::fs::write(dir.join(MANUAL_GAMES_FILE_NAME), "{not json").expect("write");
        let store = ManualGameStore::open(&dir);
        assert_eq!(store.list(), []);
        assert_eq!(store.load_warnings().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn slugify_cases() {
        assert_eq!(slugify("Assetto Corsa"), "assetto-corsa");
        assert_eq!(slugify("  DiRT Rally 2.0!! "), "dirt-rally-2-0");
        assert_eq!(slugify("!!!"), "game");
        assert_eq!(slugify(""), "game");
    }
}
