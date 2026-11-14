#![no_main]
use libfuzzer_sys::fuzz_target;
use rsh_protocol::{HelloReply, PingFrame, SerialFrame, StatusPacket, StreamFrame};

fuzz_target!(|data: &[u8]| {
    // Every parser must accept or cleanly reject arbitrary bytes —
    // never panic, never allocate unboundedly.
    let _ = StreamFrame::parse(data);
    let _ = StatusPacket::parse(data);
    let _ = HelloReply::parse(data);
    let _ = PingFrame::parse(data);
    let _ = SerialFrame::parse(data);
    // Stream round-trip: a parsed frame must re-encode to the same bytes.
    if let Ok(frame) = StreamFrame::parse(data) {
        let encoded = frame.encode();
        assert_eq!(StreamFrame::parse(&encoded).expect("re-encoded parses"), frame);
        assert_eq!(encoded, data);
    }
});
