//! Phase A6 baseline benches (R-H17) — `rsh-protocol` only.
//!
//! Every input is a golden vector copied verbatim from
//! `crates/rsh-protocol/tests/golden_vectors.rs` (protocol v2.2,
//! which itself copies `reference/tests-host/protocol_v2_vectors.py`).
//! No bench input was invented. Protocol v2.2 has no HMAC: control
//! frames are plain (`RSHC` + header + payload + CRC16 trailer) and
//! the HELLO carries zeroed reserved fields — so there are no HMAC
//! benches and no key/nonce fixtures here anymore.
//!
//! Pipeline tick / engine E2E benches are intentionally absent: the
//! engine is Phase B.

use criterion::{Criterion, criterion_group, criterion_main};
use rsh_protocol::{
    ConfigBlob, ControlFrame, HelloReply, PinModule, StatusPacket, StreamFrame, build_frame, crc16,
    factory_default_blob, validate, validate_blob,
};
use std::hint::black_box;

/// Decode a hex string (whitespace-tolerant) into bytes.
fn hex(s: &str) -> Vec<u8> {
    let compact: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(compact.len().is_multiple_of(2), "odd hex length");
    (0..compact.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&compact[i..i + 2], 16).expect("valid hex"))
        .collect()
}

/// Golden vectors (verbatim from `tests/golden_vectors.rs`).
const STREAM_V2_2: &str = "52 53 48 32 22 01 34 12 04 03 02 01 1f 02 05 02 00 00 00 00 fc 08 e8 03 d0 07 00 00 00 02 e8 03 06 ff 48 ca";
const STATUS_BASE: &str = "a6 22 07 00 40 e2 01 00 cc 56 ac 0f 78 da 02 00 03 00 5c 08 00 01 ef be ad de 05 00 02 00 01 00 cd 82";
const STATUS_EXT: &str = "a6 22 08 00 41 e2 01 00 ce 56 ac 0f 78 da 02 00 03 00 5c 08 00 01 ef be ad de 05 00 20 00 00 00 02 00 01 00 9b 6e";
const HELLO_V2_2: &str = "52 53 48 52 02 02 00 00 aa bb cc dd ee ff 04 00 03 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 ba f6";
const CTRL_GET_CAPS: &str = "52 53 48 43 01 01 00 00 00 77 e7";

/// Factory-default pin modules (same pins as `factory_default_blob`).
fn factory_modules() -> Vec<PinModule> {
    vec![
        PinModule::axis(34),
        PinModule::axis(35),
        PinModule::axis(36),
        PinModule::axis(32),
        PinModule::battery(39),
        PinModule::button(27),
        PinModule::button(33),
        PinModule::button(4),
        PinModule::encoder(23, 19),
        PinModule::encoder(16, 17),
        PinModule::led(21),
    ]
}

fn benches(c: &mut Criterion) {
    let stream = hex(STREAM_V2_2);
    let status_base = hex(STATUS_BASE);
    let status_ext = hex(STATUS_EXT);
    let hello = hex(HELLO_V2_2);
    let ctrl = hex(CTRL_GET_CAPS);
    assert_eq!(stream.len(), 36);
    assert_eq!(status_base.len(), 34);
    assert_eq!(status_ext.len(), 38);
    assert_eq!(hello.len(), 92);
    assert_eq!(ctrl.len(), 11);

    // Parsed once outside the timed loop; encode benches re-encode it.
    let stream_frame = StreamFrame::parse(&stream).expect("golden stream parses");
    // The control vector is the plain v2.2 frame: verify it once here
    // so a bad vector fails fast instead of inside a timed loop.
    ControlFrame::verify(&ctrl).expect("golden control verifies");
    let blob = factory_default_blob();
    let blob_bytes = blob.encode().expect("factory blob encodes");
    let modules = factory_modules();

    c.bench_function("stream_parse_36b", |b| {
        b.iter(|| StreamFrame::parse(black_box(&stream)).expect("parses"));
    });
    c.bench_function("stream_encode_36b", |b| {
        b.iter(|| black_box(stream_frame.encode()));
    });
    c.bench_function("status_parse_34b", |b| {
        b.iter(|| StatusPacket::parse(black_box(&status_base)).expect("parses"));
    });
    c.bench_function("status_parse_38b", |b| {
        b.iter(|| StatusPacket::parse(black_box(&status_ext)).expect("parses"));
    });
    c.bench_function("hello_parse_92b", |b| {
        b.iter(|| HelloReply::parse(black_box(&hello)).expect("parses"));
    });
    c.bench_function("control_verify_get_caps_11b", |b| {
        b.iter(|| ControlFrame::verify(black_box(&ctrl)).expect("verifies"));
    });
    c.bench_function("control_build_frame_get_caps", |b| {
        b.iter(|| build_frame(0x01, black_box(1), b"").expect("builds"));
    });
    c.bench_function("crc16_stream_36b", |b| {
        b.iter(|| crc16(black_box(&stream)));
    });
    c.bench_function("crc16_check_9b", |b| {
        b.iter(|| crc16(black_box(b"123456789")));
    });
    c.bench_function("config_blob_encode_factory", |b| {
        b.iter(|| black_box(blob.encode()).expect("encodes"));
    });
    c.bench_function("config_blob_decode_factory", |b| {
        b.iter(|| ConfigBlob::decode(black_box(&blob_bytes)).expect("decodes"));
    });
    c.bench_function("pin_validate_factory_modules", |b| {
        b.iter(|| black_box(validate(black_box(&modules), false)));
    });
    c.bench_function("pin_validate_blob_factory", |b| {
        b.iter(|| black_box(validate_blob(black_box(&blob))));
    });
}

criterion_group!(benches_group, benches);
criterion_main!(benches_group);
