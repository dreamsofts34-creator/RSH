//! Adapts the F1 telemetry readers ([`rsh_telemetry`]) to the F2
//! watcher's [`CarSource`] trait ([`rsh_profiles::watcher`]).
//!
//! Phase F gate wiring: the watcher only needs
//! `current_car() -> Option<CarReading>`, so this adapter polls every
//! reader and returns the first live snapshot. Fail-safe by
//! construction: a reader returning `None` is skipped, never a panic.
//! `GameId::Unknown` snapshots are skipped — the watcher must never
//! switch on an unidentified game.
//!
//! The adapter is deliberately thin: car-name normalisation lives in
//! the telemetry crate, profile resolution lives in the watcher.

use rsh_profiles::resolver::{GAME_ASSETTO_CORSA, GAME_ASSETTO_CORSA_COMPETIZIONE, GAME_IRACING};
use rsh_profiles::watcher::{CarReading, CarSource};
use rsh_telemetry::readers::{TelemetryReader, all_readers};
use rsh_telemetry::types::{GameId, SessionState};

/// `CarSource` backed by the live F1 telemetry readers (AC / ACC / iRacing).
pub struct TelemetryCarSource {
    readers: Vec<Box<dyn TelemetryReader + Send>>,
}

impl TelemetryCarSource {
    /// Build with the default reader set ([`all_readers`]).
    #[must_use]
    pub fn new() -> Self {
        Self {
            readers: all_readers().into_iter().collect(),
        }
    }

    /// Build with an explicit reader set (test seam).
    #[must_use]
    pub fn with_readers(readers: Vec<Box<dyn TelemetryReader + Send>>) -> Self {
        Self { readers }
    }
}

impl Default for TelemetryCarSource {
    fn default() -> Self {
        Self::new()
    }
}

impl CarSource for TelemetryCarSource {
    fn current_car(&self) -> Option<CarReading> {
        for reader in &self.readers {
            let Some(snapshot) = reader.read() else {
                continue;
            };
            let game_id = match snapshot.game {
                GameId::AssettoCorsa => GAME_ASSETTO_CORSA,
                GameId::Acc => GAME_ASSETTO_CORSA_COMPETIZIONE,
                GameId::IRacing => GAME_IRACING,
                // Dirt Rally 2.0 has no telemetry reader yet (F1 scope is
                // AC/ACC/iRacing); never invent a mapping for it.
                GameId::Unknown => continue,
            };
            let on_track = snapshot.state == SessionState::OnTrack;
            // H2: the canonical model (normalized by the reader), not
            // the raw display string — the resolver matches on the
            // canonical form.
            return Some(CarReading::new(game_id, snapshot.car_model, on_track));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsh_telemetry::readers::TelemetryReader;
    use rsh_telemetry::types::{GameId, SessionState, TelemetrySnapshot};

    /// Fake reader returning one fixed snapshot (or nothing).
    struct FakeReader {
        snapshot: Option<TelemetrySnapshot>,
    }

    impl TelemetryReader for FakeReader {
        fn game(&self) -> GameId {
            self.snapshot
                .as_ref()
                .map_or(GameId::Unknown, |snapshot| snapshot.game)
        }

        fn process_name(&self) -> &'static str {
            "fake.exe"
        }

        fn read(&self) -> Option<TelemetrySnapshot> {
            self.snapshot.clone()
        }
    }

    fn snapshot(game: GameId, car: &str, state: SessionState) -> TelemetrySnapshot {
        TelemetrySnapshot::new(game, car, "Test Track", state)
    }

    #[test]
    fn first_live_reader_wins() {
        let source = TelemetryCarSource::with_readers(vec![
            Box::new(FakeReader { snapshot: None }),
            Box::new(FakeReader {
                snapshot: Some(snapshot(
                    GameId::Acc,
                    "Ferrari 488 GT3 Evo",
                    SessionState::OnTrack,
                )),
            }),
        ]);
        let reading = source.current_car().expect("ACC snapshot read");
        assert_eq!(reading.game_id, GAME_ASSETTO_CORSA_COMPETIZIONE);
        // H2: the adapter passes the canonical model, not the raw
        // display string — the resolver matches on this form.
        assert_eq!(reading.car_name, "ferrari_488_gt3_evo");
        assert!(reading.on_track);
    }

    #[test]
    fn unknown_game_is_skipped() {
        let source = TelemetryCarSource::with_readers(vec![Box::new(FakeReader {
            snapshot: Some(snapshot(
                GameId::Unknown,
                "Mystery Car",
                SessionState::OnTrack,
            )),
        })]);
        assert!(source.current_car().is_none());
    }

    #[test]
    fn menu_state_is_not_on_track() {
        let source = TelemetryCarSource::with_readers(vec![Box::new(FakeReader {
            snapshot: Some(snapshot(
                GameId::AssettoCorsa,
                "Porsche 911 GT3 R",
                SessionState::Menu,
            )),
        })]);
        let reading = source.current_car().expect("AC snapshot read");
        assert_eq!(reading.game_id, GAME_ASSETTO_CORSA);
        assert!(!reading.on_track);
    }

    #[test]
    fn no_readers_means_no_car() {
        let source = TelemetryCarSource::with_readers(vec![]);
        assert!(source.current_car().is_none());
    }
}
