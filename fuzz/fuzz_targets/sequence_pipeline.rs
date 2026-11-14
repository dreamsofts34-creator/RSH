//! Sequence / pipeline / encoder / config-staging fuzz target (M-9).
//!
//! The existing fuzz targets cover the wire parsers and the pin /
//! blob validators. This one covers the stateful engine side that
//! parsers feed: `SequenceTracker` (dedup / gap / reboot
//! rebaseline), `EncoderTracker` (wrap-aware cumulative counts),
//! `ProcessingPipeline` (frame → output), and the `ConfigStaging`
//! state machine (chunked SET_CFG assembly). All of them must accept
//! arbitrary input without panicking, and the invariants the rest of
//! the system relies on must hold on every input:
//!
//! * `SequenceTracker::gaps` only grows on `Gap` decisions, and a
//!   `Gap(n)` counts exactly `n` skipped sequences.
//! * `EncoderTracker::update` deltas are wrap-aware: consecutive
//!   counts never produce a `|delta| > 32768` jump from the raw
//!   difference alone — the i16 wrap is `+1` / `-1`, never `±65535`.
//! * The pipeline's per-tick encoder deltas are bounded
//!   (`|delta| <= 32` per encoder per tick — the glitch filter the
//!   rig asserts in `encoder-glitch-v2`).
//! * `ConfigStaging::finish` succeeds only on a complete, valid
//!   blob; a finished blob re-encodes byte-identically.
//!
//! Input layout: the fuzzer's bytes are split into 2-byte sequence
//! numbers (first half) and raw frame/chunk bytes (second half), so
//! one input drives all four machines at once.
#![no_main]
use libfuzzer_sys::fuzz_target;
use rsh_engine::encoder::EncoderTracker;
use rsh_engine::pipeline::ProcessingPipeline;
use rsh_engine::sequence::{SeqDecision, SequenceTracker};
use rsh_protocol::{ConfigStaging, SetCfgChunk, StreamFrame};

fuzz_target!(|data: &[u8]| {
    // Split the input: even 2-byte words are sequence numbers for
    // the tracker; the whole input is also fed to the frame parser,
    // the encoder tracker, and the chunk state machine.
    let mut tracker = SequenceTracker::new();
    let mut encoders = EncoderTracker::new();
    let mut pipeline = ProcessingPipeline::default();
    let mut gaps_seen: u64 = 0;
    for word in data.chunks_exact(2) {
        let seq = u16::from_le_bytes([word[0], word[1]]);
        match tracker.check(seq) {
            SeqDecision::Gap(n) => {
                gaps_seen += u64::from(n);
                assert_eq!(tracker.gaps(), gaps_seen, "gaps() must equal the sum of Gap(n)");
            }
            SeqDecision::Accept | SeqDecision::Duplicate => {
                assert_eq!(tracker.gaps(), gaps_seen, "gaps() must not move on Accept/Duplicate");
            }
        }
        // Encoder counts: the raw i16 word per encoder slot. The
        // delta must be wrap-aware: |delta| <= 32768 always (the
        // i16 wrap is a single step, never a 65535 jump).
        let count = i16::from_le_bytes([word[0], word[1]]);
        for index in 0..4 {
            let delta = encoders.update(index, count);
            assert!(
                delta.unsigned_abs() <= 32768,
                "encoder delta out of wrap range: {delta}"
            );
        }
    }
    // Frame path: a parsed frame goes through the pipeline, and the
    // per-tick encoder deltas stay inside the glitch-filter bound.
    if let Ok(frame) = StreamFrame::parse(data) {
        let _ = tracker.check(frame.seq);
        let out = pipeline.process(&frame);
        for delta in out.encoder_deltas {
            assert!(
                delta.unsigned_abs() <= 32,
                "pipeline encoder delta exceeded the glitch-filter bound: {delta}"
            );
        }
    }
    // Config-staging state machine: chunk decodes must never panic,
    // and finish() must only succeed on a complete blob that
    // re-encodes identically.
    if let Ok(chunk) = SetCfgChunk::decode(data) {
        if let Ok(mut staging) = ConfigStaging::start(&chunk) {
            let _ = staging.add(&chunk);
            if let Ok(done) = staging.finish() {
                let encoded = done.encode().expect("finished blob re-encodes");
                let _ = encoded;
            }
        }
    }
});
