//! `rsh-telemetry`: the Assetto Corsa / ACC / iRacing shared-memory readers.
//!
//! Identity only (v1): each reader resolves the **car model, track and
//! session state** the game is currently running — no physics fields.
//! The watcher (Phase F2) polls these readers in a separate task at
//! 2 Hz and resolves the car to a wheel profile; the engine never
//! depends on a reader succeeding.
//!
//! # Fail-safe contract
//!
//! A reader never panics and never propagates an error: every failure
//! (game not running, map missing, torn or corrupt bytes, unsupported
//! version) is `None`. A reader crash/failure can therefore never
//! affect the engine — the worst case is "no snapshot this poll".
//!
//! # Platform
//!
//! The readers are Windows-only by nature (named file mappings +
//! `ToolHelp` process check, `windows` 0.62.2, `cfg(windows)`-gated —
//! the crate does not even compile that dependency on Linux). The
//! parsing itself is pure and platform-independent: every reader has a
//! `read_from_*` entry point that takes a [`MapAccessor`] and is tested
//! on Linux against recorded shared-memory fixtures in
//! `tests/fixtures/`. See `docs/TELEMETRY-RUST.md` for the spike
//! verdict, the layouts used, the fixture format and the Windows-only
//! gaps.

pub mod ac;
pub mod access;
pub mod iracing;
pub mod readers;
pub mod types;
#[cfg(windows)]
pub(crate) mod windows;

pub use access::{MapAccessor, SliceMap, read_consistent};
pub use readers::{AcReader, AccReader, IRacingReader, TelemetryReader, all_readers};
pub use types::{GameId, SessionState, TelemetrySnapshot, normalize_car_model};
