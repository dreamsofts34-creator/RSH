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
    /// Client-generated idempotency key for the add that created
    /// this entry (P1-3, v2.0.11): the UI generates one per
    /// add-dialog session and sends it with `add_game`; a retry of
    /// a call that timed out on the client but succeeded on the
    /// daemon returns the existing entry instead of minting a
    /// duplicate. `None` for entries added before keys existed
    /// (or by callers that send none) — those keep the old
    /// always-new-id behaviour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
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
        self.add_with_key(name, exe_path, bound_profile_id, None)
    }

    /// [`add`](Self::add) with an idempotency key (P1-3, v2.0.11):
    /// when `key` is a non-empty key that already created an entry,
    /// that entry is returned unchanged — no duplicate, no new id,
    /// no write. The key is stored on the entry so the dedupe
    /// survives a daemon restart (the store file round-trips it).
    ///
    /// # Errors
    ///
    /// Same as [`add`](Self::add).
    pub fn add_with_key(
        &mut self,
        name: &str,
        exe_path: &str,
        bound_profile_id: Option<String>,
        key: Option<&str>,
    ) -> Result<ManualGame, String> {
        let key = key.map(str::trim).filter(|key| !key.is_empty());
        // CORE-P3-10 (polish round): a key hit returns the existing
        // entry only when the retried parameters match — a retry
        // carrying edited parameters used to silently return the old
        // entry and lose the edit with a success response. A
        // parameter mismatch falls through to a fresh add (a new id,
        // same as a retry without a key).
        if let Some(key) = key
            && let Some(existing) = self
                .games
                .iter()
                .find(|game| game.idempotency_key.as_deref() == Some(key))
            && existing.name == name.trim()
            && existing.exe_path == exe_path.trim()
            && existing.bound_profile_id.as_deref()
                == bound_profile_id
                    .as_deref()
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
        {
            return Ok(existing.clone());
        }
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
            idempotency_key: key.map(str::to_string),
        };
        self.games.push(game.clone());
        if let Err(error) = self.save() {
            // Don't keep an unsaved game in memory: the UI got an
            // error, so the list must not show it anyway.
            self.games.pop();
            return Err(error);
        }
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
        let Some(pos) = self.games.iter().position(|game| game.id == id) else {
            return Err(format!("unknown game '{id}'"));
        };
        // CORE-P2-3 (polish round): clone-save-swap — a save failure
        // used to leave the edited entry in memory while the UI
        // showed an error, so the change silently vanished on the
        // next restart. Restore the old entry on failure.
        let old = self.games[pos].clone();
        self.games[pos].name = name.to_string();
        self.games[pos].exe_path = exe_path.to_string();
        self.games[pos].bound_profile_id = bound_profile_id.filter(|id| !id.trim().is_empty());
        let game = self.games[pos].clone();
        if let Err(error) = self.save() {
            self.games[pos] = old;
            return Err(error);
        }
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
        // CORE-P2-3 (polish round): restore the removed entry when
        // the save fails, so memory never diverges from disk.
        let removed = self.games.remove(pos);
        if let Err(error) = self.save() {
            self.games.insert(pos, removed);
            return Err(error);
        }
        Ok(())
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
        let Some(pos) = self.games.iter().position(|game| game.id == id) else {
            return Err(format!("unknown game '{id}'"));
        };
        // CORE-P2-3 (polish round): same rollback as `update`.
        let old = self.games[pos].clone();
        self.games[pos].bound_profile_id = bound_profile_id.filter(|id| !id.trim().is_empty());
        let game = self.games[pos].clone();
        if let Err(error) = self.save() {
            self.games[pos] = old;
            return Err(error);
        }
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
                idempotency_key: game.idempotency_key.filter(|key| !key.trim().is_empty()),
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
    fn add_with_key_dedupes_retries_and_survives_reopen() {
        // P1-3: a retry carrying the key of an add that already
        // succeeded returns the existing entry — no duplicate, no
        // new id, no second write.
        let dir = test_dir("idempotent");
        let mut store = ManualGameStore::open(&dir);
        let first = store
            .add_with_key("Race", r"C:\a.exe", None, Some("dialog-1"))
            .expect("add");
        let retry = store
            .add_with_key("Race", r"C:\a.exe", None, Some("dialog-1"))
            .expect("retry");
        assert_eq!(first.id, retry.id, "same key returns the same entry");
        assert_eq!(store.list().len(), 1, "no duplicate is created");
        // A different key is a different add, even for the same name.
        let second = store
            .add_with_key("Race", r"C:\b.exe", None, Some("dialog-2"))
            .expect("add 2");
        assert_ne!(second.id, first.id);
        assert_eq!(store.list().len(), 2);
        // The key survives a reopen: a retry after a daemon restart
        // still dedupes.
        let mut store = ManualGameStore::open(&dir);
        let after_restart = store
            .add_with_key("Race", r"C:\a.exe", None, Some("dialog-1"))
            .expect("retry after reopen");
        assert_eq!(after_restart.id, first.id);
        assert_eq!(store.list().len(), 2);
        // No key (old callers) keeps the old always-new-id behaviour.
        let third = store.add("Race", r"C:\c.exe", None).expect("add 3");
        assert_ne!(third.id, first.id);
        assert_ne!(third.id, second.id);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// CORE-P3-10: a retry carrying the SAME key but EDITED
    /// parameters must not return the old entry — the edit would
    /// be silently lost behind a success response. It falls through
    /// to a fresh add (a new id, same as a retry without a key).
    #[test]
    fn add_with_key_param_mismatch_is_a_fresh_add() {
        let dir = test_dir("idempotent-mismatch");
        let mut store = ManualGameStore::open(&dir);
        let first = store
            .add_with_key("Race", r"C:\a.exe", None, Some("dialog-1"))
            .expect("add");
        // Same key, edited exe path: a NEW entry, not the old one.
        let edited = store
            .add_with_key("Race", r"C:\b.exe", None, Some("dialog-1"))
            .expect("edited retry");
        assert_ne!(edited.id, first.id, "edited retry must not dedupe");
        assert_eq!(edited.exe_path, r"C:\b.exe");
        assert_eq!(store.list().len(), 2);
        // Same key, edited name: also a fresh add.
        let renamed = store
            .add_with_key("Race 2", r"C:\a.exe", None, Some("dialog-1"))
            .expect("renamed retry");
        assert_ne!(renamed.id, first.id);
        assert_eq!(store.list().len(), 3);
        // Exact retry still dedupes.
        let retry = store
            .add_with_key("Race", r"C:\a.exe", None, Some("dialog-1"))
            .expect("exact retry");
        assert_eq!(retry.id, first.id);
        assert_eq!(store.list().len(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// CORE-P2-3: when the save fails, memory must stay exactly as
    /// disk — the old code left the edited entry in memory while the
    /// UI showed an error, so the change silently vanished on the
    /// next restart. The save is broken here by replacing the games
    /// file with a directory (the atomic rename cannot land on it).
    #[test]
    fn save_failure_rolls_back_update_remove_and_set_profile() {
        let dir = test_dir("rollback");
        let mut store = ManualGameStore::open(&dir);
        let game = store.add("Race", r"C:\a.exe", None).expect("add");
        let path = dir.join(MANUAL_GAMES_FILE_NAME);
        std::fs::remove_file(&path).expect("remove games file");
        std::fs::create_dir(&path).expect("games file becomes a dir");
        // update fails and memory keeps the old entry.
        assert!(
            store
                .update(&game.id, "Race 2", r"D:\race2.exe", None)
                .is_err()
        );
        let kept = store.get(&game.id).expect("still there");
        assert_eq!(kept.name, "Race");
        assert_eq!(kept.exe_path, r"C:\a.exe");
        // set_profile fails and memory keeps the old binding.
        assert!(
            store
                .set_profile(&game.id, Some("profile-1".to_string()))
                .is_err()
        );
        assert!(store.get(&game.id).unwrap().bound_profile_id.is_none());
        // remove fails and the entry stays.
        assert!(store.remove(&game.id).is_err());
        assert_eq!(store.list().len(), 1);
        // add fails too and leaves no phantom entry.
        assert!(store.add("Other", r"C:\b.exe", None).is_err());
        assert_eq!(store.list().len(), 1);
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
