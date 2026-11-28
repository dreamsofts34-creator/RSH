//! `rsh-engine`: the `WheelEngine` 200 Hz `tokio` task plus the
//! `ProcessingPipeline` (map, deadzone, curve, One Euro smoothing, pedal ramp),
//! the 250 ms watchdog (stale data goes `NEUTRAL`), the reboot-safe
//! `SequenceTracker`, per-module tuning, millivolt-domain calibration, and the
//! NTP-style ping metrics.
//!
//! Headless by design: no Tauri/UI dependency, so the engine is testable with
//! mock transports and mock outputs.

pub mod calibration;
pub mod engine;
pub mod metrics;
pub mod ping_client;
pub mod pipeline;
pub mod sequence;
pub mod watchdog;
