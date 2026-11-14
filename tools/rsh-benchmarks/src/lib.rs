//! `rsh-benchmarks` — criterion bench harness for the RSH Rust recreation.
//!
//! Benches live in `benches/`: `protocol.rs` is the Phase A6 baseline
//! (`rsh-protocol` parse/encode/HMAC/CRC/config), `engine.rs` is the
//! Phase B5 hot-path bench (`rsh-engine` pipeline tick, full hot path,
//! sequence + watchdog overhead). The shared helpers live here:
//!
//! * [`alloc`] — the counting allocator behind the 0 B/tick gate.
//! * [`fixture`] — the fixed representative input both benches and
//!   gates measure, so their numbers are comparable.
//! * [`soak`] — the 60 s headless soak runner ([`run_soak`](soak::run_soak)),
//!   executed by the `rsh-soak` helper binary (no libtest) and
//!   asserted on by `tests/soak.rs`.
//!
//! Thresholds are enforced outside the bench binaries by
//! `check-thresholds.py` (reads `thresholds.json` + criterion's
//! `target/criterion/<bench>/new/estimates.json`); the allocation gate
//! is enforced inside the run by `tests/alloc_gate.rs` and by the
//! `alloc_probe_*` benches in `benches/engine.rs`, which panic the
//! bench binary if a single allocation sneaks into the hot path.

pub mod alloc;
pub mod fixture;
pub mod soak;
