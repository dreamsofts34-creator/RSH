//! The reader contract and the three per-game readers.
//!
//! Every reader is stateless, `Send`, and total: [`TelemetryReader::read`]
//! returns `None` whenever the game is not running, the map is missing,
//! or the bytes do not parse — it never panics and never propagates an
//! error into the caller. That is the fail-safe contract the engine
//! depends on: a reader runs in its own task (owned by the watcher,
//! Phase F2), and a reader failure can only ever yield "no snapshot",
//! never a crash.
//!
//! On non-Windows targets `read()` always returns `None` (there is no
//! game shared memory here); the pure entry points
//! [`AcReader::read_from_maps`] / [`IRacingReader::read_from_map`] work
//! everywhere and are what the fixture tests exercise.

use crate::ac::read_kunos_snapshot;
use crate::access::MapAccessor;
use crate::iracing::read_iracing_snapshot;
use crate::types::{GameId, TelemetrySnapshot};

/// The reader contract (C# `ITelemetryReader` equivalent).
///
/// Implementations must be total: `read()` never panics, never throws,
/// and returns `None` for every failure mode (game absent, map absent,
/// torn or corrupt bytes).
pub trait TelemetryReader {
    /// Which game this reader reads.
    fn game(&self) -> GameId;

    /// The game process this reader gates on (`TELEMETRY-READER.md` §2
    /// rule 3 / SEC-H-05b).
    fn process_name(&self) -> &'static str;

    /// Reads one identity snapshot from the live game, or `None`.
    fn read(&self) -> Option<TelemetrySnapshot>;

    /// Convenience: just the normalised car key, or `None`.
    fn read_car(&self) -> Option<String> {
        self.read().map(|snapshot| snapshot.car_model)
    }
}

/// Assetto Corsa reader: `acs.exe`, `acpmf_static` + `acpmf_graphics`.
#[derive(Clone, Copy, Debug, Default)]
pub struct AcReader;

impl AcReader {
    /// Creates the reader.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Pure entry point: parses a snapshot from two map accessors.
    ///
    /// Platform-independent — the fixture tests call this with
    /// [`SliceMap`](crate::access::SliceMap)s; the Windows `read()`
    /// calls it with live mapped views.
    #[must_use]
    pub fn read_from_maps(
        static_map: &dyn MapAccessor,
        graphics_map: &dyn MapAccessor,
    ) -> Option<TelemetrySnapshot> {
        read_kunos_snapshot(static_map, graphics_map, GameId::AssettoCorsa)
    }
}

impl TelemetryReader for AcReader {
    fn game(&self) -> GameId {
        GameId::AssettoCorsa
    }

    fn process_name(&self) -> &'static str {
        "acs.exe"
    }

    fn read(&self) -> Option<TelemetrySnapshot> {
        #[cfg(windows)]
        {
            if !crate::windows::is_process_running(self.process_name()) {
                return None;
            }
            let static_map = crate::windows::SharedMemoryMap::open(
                crate::ac::STATIC_MAP_NAME,
                crate::ac::STATIC_MIN_LEN,
            )?;
            let graphics_map = crate::windows::SharedMemoryMap::open(
                crate::ac::GRAPHICS_MAP_NAME,
                crate::ac::GRAPHICS_MIN_LEN,
            )?;
            Self::read_from_maps(&static_map, &graphics_map)
        }
        #[cfg(not(windows))]
        {
            None
        }
    }
}

/// Assetto Corsa Competizione reader: `acc.exe`, same `acpmf_*` maps as
/// AC (the readers differ by process gate, not by map name).
#[derive(Clone, Copy, Debug, Default)]
pub struct AccReader;

impl AccReader {
    /// Creates the reader.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Pure entry point: parses a snapshot from two map accessors.
    #[must_use]
    pub fn read_from_maps(
        static_map: &dyn MapAccessor,
        graphics_map: &dyn MapAccessor,
    ) -> Option<TelemetrySnapshot> {
        read_kunos_snapshot(static_map, graphics_map, GameId::Acc)
    }
}

impl TelemetryReader for AccReader {
    fn game(&self) -> GameId {
        GameId::Acc
    }

    fn process_name(&self) -> &'static str {
        "acc.exe"
    }

    fn read(&self) -> Option<TelemetrySnapshot> {
        #[cfg(windows)]
        {
            if !crate::windows::is_process_running(self.process_name()) {
                return None;
            }
            let static_map = crate::windows::SharedMemoryMap::open(
                crate::ac::STATIC_MAP_NAME,
                crate::ac::STATIC_MIN_LEN,
            )?;
            let graphics_map = crate::windows::SharedMemoryMap::open(
                crate::ac::GRAPHICS_MAP_NAME,
                crate::ac::GRAPHICS_MIN_LEN,
            )?;
            Self::read_from_maps(&static_map, &graphics_map)
        }
        #[cfg(not(windows))]
        {
            None
        }
    }
}

/// iRacing reader: `iRacingSim64DX11.exe`, `Local\IRSDKMemMapFileName`.
#[derive(Clone, Copy, Debug, Default)]
pub struct IRacingReader;

impl IRacingReader {
    /// Creates the reader.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Pure entry point: parses a snapshot from one map accessor.
    #[must_use]
    pub fn read_from_map(map: &dyn MapAccessor) -> Option<TelemetrySnapshot> {
        read_iracing_snapshot(map)
    }
}

impl TelemetryReader for IRacingReader {
    fn game(&self) -> GameId {
        GameId::IRacing
    }

    fn process_name(&self) -> &'static str {
        "iRacingSim64DX11.exe"
    }

    fn read(&self) -> Option<TelemetrySnapshot> {
        #[cfg(windows)]
        {
            if !crate::windows::is_process_running(self.process_name()) {
                return None;
            }
            let map = crate::windows::SharedMemoryMap::open(
                crate::iracing::MAP_NAME,
                crate::iracing::MAP_SIZE,
            )?;
            Self::read_from_map(&map)
        }
        #[cfg(not(windows))]
        {
            None
        }
    }
}

/// All three readers, in a fixed order: AC, ACC, iRacing.
///
/// The watcher (Phase F2) polls these in its own task at 2 Hz.
#[must_use]
pub fn all_readers() -> [Box<dyn TelemetryReader + Send>; 3] {
    [
        Box::new(AcReader::new()),
        Box::new(AccReader::new()),
        Box::new(IRacingReader::new()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_readers_covers_three_games() {
        let readers = all_readers();
        assert_eq!(readers[0].game(), GameId::AssettoCorsa);
        assert_eq!(readers[1].game(), GameId::Acc);
        assert_eq!(readers[2].game(), GameId::IRacing);
        assert_eq!(readers[0].process_name(), "acs.exe");
        assert_eq!(readers[1].process_name(), "acc.exe");
        assert_eq!(readers[2].process_name(), "iRacingSim64DX11.exe");
    }

    #[test]
    fn live_read_returns_none_without_windows() {
        // There is no game shared memory in this sandbox; on Windows
        // this would attempt a real (gated) read instead.
        for reader in all_readers() {
            assert!(reader.read().is_none());
            assert_eq!(reader.read_car(), None);
        }
    }
}
