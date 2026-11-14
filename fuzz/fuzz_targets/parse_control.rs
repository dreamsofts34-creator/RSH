#![no_main]
use libfuzzer_sys::fuzz_target;
use rsh_protocol::{ControlFrame, DuplicateCache, ReplayTracker};

fuzz_target!(|data: &[u8]| {
    // Structure parse must never panic on arbitrary bytes.
    let _ = ControlFrame::parse_structure(data);
    // Tag verification with fixed test key/nonce must never panic.
    let key = [0x11u8; 32];
    let nonce = [0x22u8; 16];
    if let Ok(frame) = ControlFrame::verify(data, &key, &nonce) {
        // A verified frame re-encodes byte-identically (same key/nonce).
        let encoded = frame.encode(&key, &nonce).expect("payload <= 1024");
        assert_eq!(encoded, data);
        // Counter-state helpers must never panic on any counter.
        let mut tracker = ReplayTracker::new();
        let _ = tracker.observe(frame.counter);
        let _ = tracker.is_replay(frame.counter);
        let mut cache = DuplicateCache::new();
        let _ = cache.insert(frame.req_id, frame.counter);
    }
});
