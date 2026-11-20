#![no_main]
use libfuzzer_sys::fuzz_target;
use rsh_protocol::{ControlFrame, DuplicateCache};

fuzz_target!(|data: &[u8]| {
    // Structure parse must never panic on arbitrary bytes.
    let _ = ControlFrame::parse_structure(data);
    // CRC verification must never panic.
    if let Ok(frame) = ControlFrame::verify(data) {
        // A verified frame re-encodes byte-identically.
        let encoded = frame.encode().expect("payload <= 1024");
        assert_eq!(encoded, data);
        // ReqId dedup cache must never panic on any req_id.
        let mut cache = DuplicateCache::new();
        let _ = cache.insert(frame.req_id);
        let _ = cache.is_duplicate(frame.req_id);
    }
});
