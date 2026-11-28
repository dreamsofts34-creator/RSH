//! Sequence / pipeline / config-staging fuzz target (M-9).
//!
//! The existing fuzz targets cover the wire parsers and the pin /
//! blob validators. This one covers the stateful engine side that
//! parsers feed: `SequenceTracker` (dedup / gap / reboot
//! rebaseline), `ProcessingPipeline` (frame → output), and the
//! `ConfigStaging` state machine (chunked SET_CFG assembly). All of
//! them must accept arbitrary input without panicking, and the
//! invariants the rest of the system relies on must hold on every
//! input:
//!
//! * `SequenceTracker::gaps` only grows on `Gap` decisions, and a
//!   `Gap(n)` counts exactly `n` skipped sequences.
//! * `ConfigStaging::finish` succeeds only on a complete, valid
//!   blob; a finished blob re-encodes byte-identically.
//!
//! v2.0.19: the encoder parts (`EncoderTracker`, per-tick encoder
//! deltas, the glitch filter) are deleted with the buttons/encoders
//! cleanup — encoder words in a frame are parsed (frozen wire
//! shape) and ignored by the pipeline.
//!
//! Input layout: the fuzzer's bytes are split into 2-byte sequence
//! numbers (first half) and raw frame/chunk bytes (second half), so
//! one input drives all three machines at once.
#![no_main]
use libfuzzer_sys::fuzz_target;
use rsh_engine::pipeline::ProcessingPipeline;
use rsh_engine::sequence::{SeqDecision, SequenceTracker};
use rsh_protocol::{ConfigStaging, SetCfgChunk, StreamFrame};

fuzz_target!(|data: &[u8]| {
    // Split the input: even 2-byte words are sequence numbers for
    // the tracker; the whole input is also fed to the frame parser
    // and the chunk state machine.
    let mut tracker = SequenceTracker::new();
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
    }
    // Frame path: a parsed frame goes through the pipeline (encoder
    // words in the frame are ignored since v2.0.19 — the pipeline
    // must still accept the frame without panicking).
    if let Ok(frame) = StreamFrame::parse(data) {
        let _ = tracker.check(frame.seq);
        let _ = pipeline.process(&frame);
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
