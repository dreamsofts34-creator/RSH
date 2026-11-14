//! Shared types for the telemetry readers: game identity, session state,
//! the snapshot a reader returns, and car-name normalisation.
//!
//! Everything in this module is platform-independent and total: no I/O,
//! no panics on untrusted input.

use std::time::Instant;

/// The game a telemetry snapshot came from.
///
/// Discriminants match `rsh-profiles::csharp_import::GameId` (0 = Unknown,
/// 1 = Assetto Corsa, 2 = ACC, 3 = iRacing) so the resolver (Phase F2) can
/// map between the two without a translation table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum GameId {
    /// Unknown / no game detected.
    #[default]
    Unknown = 0,
    /// Assetto Corsa (`acs.exe`).
    AssettoCorsa = 1,
    /// Assetto Corsa Competizione (`acc.exe`).
    Acc = 2,
    /// iRacing (`iRacingSim64DX11.exe`).
    IRacing = 3,
}

impl GameId {
    /// Maps a raw numeric game id, defaulting to [`GameId::Unknown`].
    #[must_use]
    pub const fn from_u32(value: u32) -> Self {
        match value {
            1 => Self::AssettoCorsa,
            2 => Self::Acc,
            3 => Self::IRacing,
            _ => Self::Unknown,
        }
    }
}

/// What the game is doing, as far as profile switching cares.
///
/// Only [`SessionState::OnTrack`] ever triggers an auto-switch (Phase F2);
/// every other state means "do not switch".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SessionState {
    /// State could not be determined (map missing, var absent, bad value).
    #[default]
    Unknown,
    /// Game is in menus / not in a session (`AC_STATUS` OFF).
    Menu,
    /// Session is loading / getting in the car / warmup.
    Loading,
    /// Car is on track and driving (live, replay excluded).
    OnTrack,
    /// A replay is playing.
    Replay,
    /// Session paused.
    Paused,
}

/// One identity snapshot: which game, which car, which track, which state.
///
/// Identity only (v1 scope): no physics fields (speed/rpm/gear) are read —
/// those belong to a future LED/dash feature. `car_model` is the
/// normalised profile key (see [`normalize_car_model`]); `car_display` is
/// the name exactly as the game reports it, for UI toasts.
#[derive(Clone, Debug)]
pub struct TelemetrySnapshot {
    /// Which game produced this snapshot.
    pub game: GameId,
    /// Normalised car key: lowercase, separators unified to `_`.
    pub car_model: String,
    /// Car name as reported by the game, trimmed (e.g. `Ferrari 488 GT3 Evo`).
    pub car_display: String,
    /// Track name as reported by the game, trimmed (may be empty).
    pub track_name: String,
    /// Session state at read time.
    pub state: SessionState,
    /// Monotonic read time, for the watcher's debounce (Phase F2).
    pub seen_at: Instant,
}

impl TelemetrySnapshot {
    /// Builds a snapshot, normalising `car_model` from `car_display`.
    #[must_use]
    pub fn new(game: GameId, car_display: &str, track_name: &str, state: SessionState) -> Self {
        Self {
            game,
            car_model: normalize_car_model(car_display),
            car_display: car_display.trim().to_string(),
            track_name: track_name.trim().to_string(),
            state,
            seen_at: Instant::now(),
        }
    }

    /// Whether this snapshot may trigger a profile switch.
    #[must_use]
    pub const fn is_on_track(&self) -> bool {
        matches!(self.state, SessionState::OnTrack)
    }

    /// Whether a car was actually identified.
    #[must_use]
    pub fn has_car(&self) -> bool {
        !self.car_model.is_empty()
    }
}

/// Normalises a car name into a profile key.
///
/// Lowercases, and treats runs of space / `_` / `-` as one `_`, with no
/// leading or trailing `_`. This is what makes `ferrari_488_gt3` (AC) and
/// `Ferrari 488 GT3` (ACC) match the same profile key
/// (`ferrari_488_gt3`); see `TELEMETRY-READER.md` §5.
#[must_use]
pub fn normalize_car_model(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    // Starts "after a separator" so leading separators are dropped.
    let mut last_was_sep = true;
    for ch in raw.chars().flat_map(char::to_lowercase) {
        if ch == ' ' || ch == '_' || ch == '-' {
            if !last_was_sep {
                out.push('_');
                last_was_sep = true;
            }
        } else {
            out.push(ch);
            last_was_sep = false;
        }
    }
    if last_was_sep {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_ac_underscore_name_unchanged() {
        assert_eq!(normalize_car_model("ferrari_488_gt3"), "ferrari_488_gt3");
    }

    #[test]
    fn normalize_acc_display_name_matches_ac_key() {
        assert_eq!(
            normalize_car_model("Ferrari 488 GT3 Evo"),
            "ferrari_488_gt3_evo"
        );
    }

    #[test]
    fn normalize_collapses_mixed_separators_and_trims() {
        assert_eq!(
            normalize_car_model("  Ferrari--488__GT3  "),
            "ferrari_488_gt3"
        );
    }

    #[test]
    fn normalize_empty_and_separators_only() {
        assert_eq!(normalize_car_model(""), "");
        assert_eq!(normalize_car_model("  --__  "), "");
    }

    #[test]
    fn game_id_round_trips_unknown() {
        assert_eq!(GameId::from_u32(1), GameId::AssettoCorsa);
        assert_eq!(GameId::from_u32(2), GameId::Acc);
        assert_eq!(GameId::from_u32(3), GameId::IRacing);
        assert_eq!(GameId::from_u32(0), GameId::Unknown);
        assert_eq!(GameId::from_u32(99), GameId::Unknown);
    }

    #[test]
    fn snapshot_derives_normalized_key_and_display() {
        let snap = TelemetrySnapshot::new(
            GameId::Acc,
            "Ferrari 488 GT3 Evo",
            "spa",
            SessionState::OnTrack,
        );
        assert_eq!(snap.car_model, "ferrari_488_gt3_evo");
        assert_eq!(snap.car_display, "Ferrari 488 GT3 Evo");
        assert!(snap.is_on_track());
        assert!(snap.has_car());
    }
}
