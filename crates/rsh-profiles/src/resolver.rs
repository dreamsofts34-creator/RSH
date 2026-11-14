//! Per-car / per-game profile resolution (Phase F2, SW-R7.2).
//!
//! Resolution order (DATA-INPUT §3, switch trigger rules):
//!
//! 1. Car-specific profile — game + car model (e.g. ACC + "Ferrari 488 GT3").
//! 2. Game profile — the profile bound to the running game.
//! 3. Default profile — only when one is configured; otherwise no switch.
//!
//! A resolver never invents a profile: when nothing matches it returns
//! [`None`] and the current profile stays active.
//!
//! The resolver reads through the [`ProfileSource`] trait so the real
//! profile store (Phase F, `store.rs`) can be wired in at the gate without
//! changing this module. Tests use the in-memory [`MapProfileSource`].

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Game id for Assetto Corsa (Steam 244210, `assettocorsa.exe`).
pub const GAME_ASSETTO_CORSA: &str = "assetto-corsa";
/// Game id for Assetto Corsa Competizione (Steam 805550, `acc.exe`).
pub const GAME_ASSETTO_CORSA_COMPETIZIONE: &str = "assetto-corsa-competizione";
/// Game id for `iRacing` (`iRacingSim64DX11.exe`, not on Steam).
pub const GAME_IRACING: &str = "iracing";
/// Game id for `DiRT` Rally 2.0 (Steam 690790, `dirt2.exe`).
pub const GAME_DIRT_RALLY_2: &str = "dirt-rally-2.0";

/// Tauri event emitted when the active profile changes
/// (`rsh://profile-switched`, see [`ProfileSwitchEvent`]).
pub const PROFILE_SWITCHED_EVENT: &str = "rsh://profile-switched";

/// Opaque id of a wheel profile (the `id` of a profile in the store).
///
/// A newtype, not a bare `String`, so a profile id can never be confused
/// with a game id or a car name at a call site.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq, Hash)]
#[serde(transparent)]
pub struct ProfileId(pub String);

impl ProfileId {
    /// Wraps a profile id string.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Borrows the id as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ProfileId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for ProfileId {
    fn from(id: &str) -> Self {
        Self(id.to_owned())
    }
}

impl From<String> for ProfileId {
    fn from(id: String) -> Self {
        Self(id)
    }
}

/// Read-only view of the profile bindings the resolver needs.
///
/// The real store implements this at the gate; the resolver only ever
/// reads, never writes. Keys are matched case-insensitively after
/// trimming (car names arrive from telemetry exactly as the game reports
/// them, e.g. `"Ferrari 488 GT3"` vs `"ferrari 488 gt3"`).
pub trait ProfileSource {
    /// Profile bound to a game, if any (`game_id` is one of the
    /// `GAME_*` constants, e.g. [`GAME_ASSETTO_CORSA_COMPETIZIONE`]).
    fn profile_for_game(&self, game_id: &str) -> Option<ProfileId>;

    /// Profile bound to a car name regardless of game, if any.
    ///
    /// The default implementation returns [`None`]; sources that keep
    /// game-scoped car bindings should override
    /// [`profile_for_game_car`](ProfileSource::profile_for_game_car)
    /// instead, which the resolver tries first.
    fn profile_for_car(&self, car_name: &str) -> Option<ProfileId>;

    /// Profile bound to a specific car in a specific game, if any.
    /// This is the first lookup the resolver tries.
    ///
    /// The default implementation falls back to
    /// [`profile_for_car`](ProfileSource::profile_for_car) so sources
    /// with only car-level bindings keep working.
    fn profile_for_game_car(&self, game_id: &str, car_name: &str) -> Option<ProfileId> {
        let _ = game_id;
        self.profile_for_car(car_name)
    }

    /// Default profile, used only when neither a car nor a game binding
    /// matches. [`None`] means "no default — do not switch".
    fn default_profile(&self) -> Option<ProfileId> {
        None
    }
}

/// Normalises a game id / car name key for lookup: trimmed, lower-cased.
#[must_use]
pub fn normalize_key(key: &str) -> String {
    key.trim().to_lowercase()
}

/// In-memory [`ProfileSource`] for tests and for the gate wiring.
///
/// Bindings are stored under normalised keys, so `"Ferrari 488 GT3"`,
/// `" ferrari 488 gt3 "` and `"FERRARI 488 GT3"` all resolve the same way.
#[derive(Clone, Debug, Default)]
pub struct MapProfileSource {
    game_profiles: HashMap<String, ProfileId>,
    /// Keyed by `(normalised game id, normalised car name)`.
    game_car_profiles: HashMap<(String, String), ProfileId>,
    /// Car-only bindings, keyed by normalised car name.
    car_profiles: HashMap<String, ProfileId>,
    default: Option<ProfileId>,
}

impl MapProfileSource {
    /// Creates an empty source with no bindings and no default.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Binds a profile to a game. Returns `self` for chaining.
    #[must_use]
    pub fn with_game_profile(mut self, game_id: &str, profile: ProfileId) -> Self {
        self.game_profiles.insert(normalize_key(game_id), profile);
        self
    }

    /// Binds a profile to a car in a specific game. Returns `self` for chaining.
    #[must_use]
    pub fn with_car_profile(mut self, game_id: &str, car_name: &str, profile: ProfileId) -> Self {
        self.game_car_profiles
            .insert((normalize_key(game_id), normalize_key(car_name)), profile);
        self
    }

    /// Binds a profile to a car name in any game. Returns `self` for chaining.
    #[must_use]
    pub fn with_car_fallback_profile(mut self, car_name: &str, profile: ProfileId) -> Self {
        self.car_profiles.insert(normalize_key(car_name), profile);
        self
    }

    /// Sets the default profile (used when nothing else matches).
    /// Returns `self` for chaining.
    #[must_use]
    pub fn with_default(mut self, profile: ProfileId) -> Self {
        self.default = Some(profile);
        self
    }
}

impl ProfileSource for MapProfileSource {
    fn profile_for_game(&self, game_id: &str) -> Option<ProfileId> {
        self.game_profiles.get(&normalize_key(game_id)).cloned()
    }

    fn profile_for_car(&self, car_name: &str) -> Option<ProfileId> {
        self.car_profiles.get(&normalize_key(car_name)).cloned()
    }

    fn profile_for_game_car(&self, game_id: &str, car_name: &str) -> Option<ProfileId> {
        let key = (normalize_key(game_id), normalize_key(car_name));
        self.game_car_profiles
            .get(&key)
            .cloned()
            .or_else(|| self.profile_for_car(car_name))
    }

    fn default_profile(&self) -> Option<ProfileId> {
        self.default.clone()
    }
}

/// Why a profile switch happened (payload of [`ProfileSwitchEvent`]).
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ProfileSwitchReason {
    /// A game was detected in the foreground (after the 400 ms debounce).
    GameDetected,
    /// A car stayed stable on track long enough (4 polls / 2 s).
    CarDetected,
    /// The game closed; the pre-game profile was restored.
    GameClosedRestore,
}

/// Notification payload for a profile switch.
///
/// The app layer emits this as the Tauri event [`PROFILE_SWITCHED_EVENT`]
/// (`rsh://profile-switched`); the frontend shows it as a toast. This
/// module only *builds* the payload — emitting it is the app layer's job.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSwitchEvent {
    /// Profile that was active before the switch (`None` = none active).
    pub previous_profile_id: Option<ProfileId>,
    /// Profile that is active after the switch.
    pub new_profile_id: ProfileId,
    /// Game that triggered the switch, if the switch is game-related.
    pub game_id: Option<String>,
    /// Car that triggered the switch, if the switch is car-related.
    pub car_name: Option<String>,
    /// Why the switch happened.
    pub reason: ProfileSwitchReason,
}

impl ProfileSwitchEvent {
    /// Builds a switch event.
    #[must_use]
    pub fn new(
        previous_profile_id: Option<ProfileId>,
        new_profile_id: ProfileId,
        game_id: Option<String>,
        car_name: Option<String>,
        reason: ProfileSwitchReason,
    ) -> Self {
        Self {
            previous_profile_id,
            new_profile_id,
            game_id,
            car_name,
            reason,
        }
    }
}

/// Resolves "which profile should be active?" for a game and car.
///
/// Pure lookup over a [`ProfileSource`]; it never applies a switch and
/// never touches the store. The [`crate::watcher::TelemetryWatcher`]
/// owns the timing (debounce, car stability) and the fail-safe apply.
pub struct CarProfileResolver<'a> {
    source: &'a dyn ProfileSource,
}

impl<'a> CarProfileResolver<'a> {
    /// Creates a resolver over a profile source.
    #[must_use]
    pub fn new(source: &'a dyn ProfileSource) -> Self {
        Self { source }
    }

    /// Resolves the profile for a game and optional car, in order:
    /// car-in-game, then car-only, then game, then default.
    ///
    /// Returns [`None`] when nothing matches — the caller must then
    /// leave the current profile untouched.
    #[must_use]
    pub fn resolve(&self, game_id: Option<&str>, car_name: Option<&str>) -> Option<ProfileId> {
        let car_name = car_name.map(str::trim).filter(|name| !name.is_empty());
        let game_id = game_id.map(str::trim).filter(|id| !id.is_empty());

        if let (Some(game), Some(car)) = (game_id, car_name)
            && let Some(profile) = self.source.profile_for_game_car(game, car)
        {
            return Some(profile);
        }
        if let Some(car) = car_name
            && let Some(profile) = self.source.profile_for_car(car)
        {
            return Some(profile);
        }
        if let Some(game) = game_id
            && let Some(profile) = self.source.profile_for_game(game)
        {
            return Some(profile);
        }
        self.source.default_profile()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> MapProfileSource {
        MapProfileSource::new()
            .with_game_profile(
                GAME_ASSETTO_CORSA_COMPETIZIONE,
                ProfileId::new("acc-default"),
            )
            .with_car_profile(
                GAME_ASSETTO_CORSA_COMPETIZIONE,
                "Ferrari 488 GT3",
                ProfileId::new("acc-ferrari"),
            )
            .with_car_fallback_profile("Porsche 911 GT3 R", ProfileId::new("any-porsche"))
            .with_default(ProfileId::new("global-default"))
    }

    #[test]
    fn car_in_game_beats_game_profile() {
        let src = source();
        let resolver = CarProfileResolver::new(&src);
        assert_eq!(
            resolver.resolve(
                Some(GAME_ASSETTO_CORSA_COMPETIZIONE),
                Some("Ferrari 488 GT3")
            ),
            Some(ProfileId::new("acc-ferrari"))
        );
    }

    #[test]
    fn car_lookup_is_case_and_space_insensitive() {
        let src = source();
        let resolver = CarProfileResolver::new(&src);
        assert_eq!(
            resolver.resolve(
                Some(GAME_ASSETTO_CORSA_COMPETIZIONE),
                Some("  FERRARI 488 gt3 ")
            ),
            Some(ProfileId::new("acc-ferrari"))
        );
    }

    #[test]
    fn car_fallback_applies_in_any_game() {
        let src = source();
        let resolver = CarProfileResolver::new(&src);
        assert_eq!(
            resolver.resolve(Some(GAME_IRACING), Some("Porsche 911 GT3 R")),
            Some(ProfileId::new("any-porsche"))
        );
    }

    #[test]
    fn game_profile_used_when_no_car_matches() {
        let src = source();
        let resolver = CarProfileResolver::new(&src);
        assert_eq!(
            resolver.resolve(Some(GAME_ASSETTO_CORSA_COMPETIZIONE), Some("Unknown Car")),
            Some(ProfileId::new("acc-default"))
        );
    }

    #[test]
    fn default_used_when_neither_game_nor_car_matches() {
        let src = source();
        let resolver = CarProfileResolver::new(&src);
        assert_eq!(
            resolver.resolve(Some(GAME_IRACING), Some("Unknown Car")),
            Some(ProfileId::new("global-default"))
        );
    }

    #[test]
    fn no_match_and_no_default_resolves_to_none() {
        let empty = MapProfileSource::new();
        let resolver = CarProfileResolver::new(&empty);
        assert_eq!(
            resolver.resolve(Some(GAME_IRACING), Some("Unknown Car")),
            None
        );
        assert_eq!(resolver.resolve(None, None), None);
    }

    #[test]
    fn empty_car_name_is_treated_as_no_car() {
        let src = source();
        let resolver = CarProfileResolver::new(&src);
        assert_eq!(
            resolver.resolve(Some(GAME_ASSETTO_CORSA_COMPETIZIONE), Some("   ")),
            Some(ProfileId::new("acc-default"))
        );
    }

    #[test]
    fn switch_event_serializes_camel_case() {
        let event = ProfileSwitchEvent::new(
            Some(ProfileId::new("old")),
            ProfileId::new("new"),
            Some(GAME_IRACING.to_owned()),
            Some("Dallara P217".to_owned()),
            ProfileSwitchReason::CarDetected,
        );
        let json = serde_json::to_string(&event).expect("event serializes");
        assert!(json.contains("\"newProfileId\":\"new\""));
        assert!(json.contains("\"reason\":\"car-detected\""));
        assert_eq!(PROFILE_SWITCHED_EVENT, "rsh://profile-switched");
    }
}
