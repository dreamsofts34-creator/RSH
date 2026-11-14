//! `rsh-profiles`: Profile v2 (`serde` JSON, `schemaVersion` required),
//! 3-generation backup, and per-car/per-game profile resolution.
//!
//! This crate also owns the read-only C# v2 importer ([`csharp_import`]):
//! it parses C# `%AppData%\RSH\profiles.json` / `settings.json` without ever
//! writing to them. The Rust app itself starts fresh (D-R8) — the importer
//! exists only for a manual/future import step, never for auto-migration.

/// Read-only importer for C# v2 data files.
pub mod csharp_import;

/// Profile + settings persistence: the Rust store (3-generation
/// backup, per-game/per-car resolution, import/export).
pub mod store;

/// Per-car / per-game profile resolution (Phase F2, SW-R7.2).
pub mod resolver;

/// Telemetry watcher: profile auto-switch (Phase F2, SW-R7.2).
pub mod watcher;
