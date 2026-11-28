//! Fixed representative hot-path input for the Phase B5 benches and
//! gates (SW-R8.1).
//!
//! One deterministic input set, shared by `benches/engine.rs`,
//! `tests/alloc_gate.rs` and `tests/soak.rs`, so a number measured in
//! one place is comparable with the same number anywhere else.
//!
//! # The input
//!
//! [`representative_inputs`] returns 1024 [`TickInput`]s, each a
//! [`StreamFrame`](rsh_protocol::StreamFrame) plus its wire encoding:
//!
//! * Axes (mV, default range `300..=3300`, centre `1800`): steering
//!   slot 0, accelerator slot 1, brake slot 2, handbrake slot 4 —
//!   the four analog modules a typical wheel reports. Clutch (slot 3)
//!   is absent, matching the factory default plan (clutch disabled).
//! * Encoders: 2 cumulative `i16` counts stepping `+2` / `-1` per tick
//!   (wrapping) — still sent in the frame (frozen wire shape), but
//!   the pipeline ignores them since v2.0.19; the words only
//!   exercise the parser.
//! * Buttons: wheel buttons plus the horn bit on every 7th tick.
//! * `seq` increments `+1` per frame from 0; `timestamp_us` advances
//!   5000 µs per frame (the 200 Hz tick).
//!
//! # Determinism
//!
//! No randomness, no wall clock, no hash-map iteration order: every
//! value is a closed-form function of the frame index (wrapping
//! integer arithmetic only — a fixed seed in the LCG sense, with the
//! formula documented here instead of a magic constant). The same
//! index always yields the same frame, on every machine, so bench
//! numbers are comparable run to run and the allocation/soak gates
//! measure the same work the benches do.

use rsh_protocol::{StreamFlags, StreamFrame};

/// Number of frames in [`representative_inputs`]. 1024 is a power of
/// two (cheap `%` cycling in the benches) and long enough that a
/// per-frame cost cannot hide behind a single-frame special case.
pub const INPUT_COUNT: usize = 1024;

/// One hot-path tick's input: the parsed frame and its wire bytes.
/// The bytes are what the transport hands the engine; the frame is
/// the same data already parsed (benches that time `process()` alone
/// use it, benches that time the full hot path parse the bytes).
#[derive(Clone, Debug)]
pub struct TickInput {
    /// The parsed stream frame for this tick.
    pub frame: StreamFrame,
    /// The frame's wire encoding (parse input for the hot-path bench).
    pub bytes: Vec<u8>,
}

/// Build the fixed 1024-frame representative input set.
///
/// Steering sweeps `1100..=2500` mV, accelerator / brake / handbrake
/// sweep the full `300..=3300` mV range, encoder words step `+2` / `-1`
/// per tick (sent, ignored by the pipeline since v2.0.19), and the horn
/// bit (button 2) is set on every 7th frame.
/// All arithmetic is wrapping integer arithmetic on the frame index.
///
/// # Panics
///
/// Panics if `INPUT_COUNT` ever exceeds `u16::MAX` (the `try_from`
/// conversions above); today it is 1024, so this cannot trigger.
#[must_use]
pub fn representative_inputs() -> Vec<TickInput> {
    (0..INPUT_COUNT)
        .map(|i| {
            let index = u32::try_from(i).expect("INPUT_COUNT fits u32");
            let seq = u16::try_from(index).expect("INPUT_COUNT fits u16");
            let enc = i16::try_from(index).expect("INPUT_COUNT fits i16");
            let steering = 1100 + (index.wrapping_mul(73) % 1401) as u16;
            let accel = 300 + (index.wrapping_mul(131) % 3001) as u16;
            let brake = 300 + (index.wrapping_mul(97) % 3001) as u16;
            let handbrake = 300 + (index.wrapping_mul(57) % 3001) as u16;
            let enc0 = enc.wrapping_mul(2);
            let enc1 = enc.wrapping_neg();
            let buttons = if i % 7 == 0 { 0b101 } else { 0b001 };
            let frame = StreamFrame::new(
                seq,
                index.wrapping_mul(5000),
                StreamFlags::default(),
                &[(0, steering), (1, accel), (2, brake), (4, handbrake)],
                &[enc0, enc1],
                buttons,
            );
            let bytes = frame.encode();
            TickInput { frame, bytes }
        })
        .collect()
}
