#![no_main]
use libfuzzer_sys::fuzz_target;
use rsh_protocol::{ConfigBlob, SetCfgChunk, validate_blob};

fuzz_target!(|data: &[u8]| {
    // Blob decode must never panic on arbitrary bytes.
    if let Ok(blob) = ConfigBlob::decode(data) {
        let encoded = blob.encode().expect("decoded blob re-encodes");
        assert_eq!(encoded, data);
        // The blob validator must never panic on a decoded blob.
        let _ = validate_blob(&blob);
    }
    // Chunk decode must never panic either.
    if let Ok(chunk) = SetCfgChunk::decode(data) {
        let encoded = chunk.encode().expect("decoded chunk re-encodes");
        assert_eq!(encoded, data);
    }
});
