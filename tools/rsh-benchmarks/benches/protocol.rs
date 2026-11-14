//! Phase A6 baseline benches (R-H17) — `rsh-protocol` only.
//!
//! Every input is a golden vector copied verbatim from
//! `crates/rsh-protocol/tests/golden_vectors.rs` (which itself copies
//! `reference/tests-host/protocol_v2_vectors.py`). No bench input was
//! invented. Test fixtures only — `TEST_KEY = 00..1f`,
//! `TEST_NONCE = a0..af` are NOT real secrets.
//!
//! Pipeline tick / engine E2E benches are intentionally absent: the
//! engine is Phase B. See `docs/BASELINE.md` (TBD list).

use criterion::{Criterion, criterion_group, criterion_main};
use rsh_protocol::{
    ConfigBlob, ControlFrame, HelloReply, PinModule, StatusPacket, StreamFrame, build_frame,
    compute_tag, crc16, factory_default_blob, validate, validate_blob, verify_frame, verify_tag,
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

const TEST_KEY: [u8; 32] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
    0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f,
];
const TEST_NONCE: [u8; 16] = [
    0xa0, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xab, 0xac, 0xad, 0xae, 0xaf,
];

/// Golden vectors (verbatim from `tests/golden_vectors.rs`).
const STREAM_V2_1: &str = "52 53 48 32 21 01 34 12 04 03 02 01 1f 02 05 02 00 00 00 00 fc 08 e8 03 d0 07 00 00 00 02 e8 03 06 ff 69 d1";
const STATUS_BASE: &str = "a6 21 07 00 40 e2 01 00 cc 56 ac 0f 78 da 02 00 03 00 5c 08 00 01 ef be ad de 05 00 02 00 01 00 97 00";
const STATUS_EXT: &str = "a6 21 08 00 41 e2 01 00 ce 56 aa 0f 78 da 02 00 03 00 5c 08 00 01 ef be ad de 05 00 20 00 00 00 02 00 01 00 e1 1e";
const HELLO_UNPAIRED: &str = "52 53 48 52 02 01 09 01 aa bb cc dd ee ff 04 00 00 00 a0 a1 a2 a3 a4 a5 a6 a7 a8 a9 aa ab ac ad ae af 00 01 02 03 04 05 06 07 08 09 0a 0b 0c 0d 0e 0f 10 11 12 13 14 15 16 17 18 19 1a 1b 1c 1d 1e 1f 41 62 33 64 45 66 36 68 4a 6b 39 6d 4e 70 32 72 53 74 35 76 57 78 38 59 5a 42";
const CTRL_GET_CAPS: &str =
    "52 53 48 43 01 01 00 00 00 00 00 00 00 42 ee 8d 10 07 93 58 d9 93 6f e7 43 68 f8 b6 61";

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
    let stream = hex(STREAM_V2_1);
    let status_base = hex(STATUS_BASE);
    let status_ext = hex(STATUS_EXT);
    let hello = hex(HELLO_UNPAIRED);
    let ctrl = hex(CTRL_GET_CAPS);
    assert_eq!(stream.len(), 36);
    assert_eq!(status_base.len(), 34);
    assert_eq!(status_ext.len(), 38);
    assert_eq!(hello.len(), 92);
    assert_eq!(ctrl.len(), 29);

    // Parsed once outside the timed loop; encode benches re-encode it.
    let stream_frame = StreamFrame::parse(&stream).expect("golden stream parses");
    let tag = compute_tag(&TEST_KEY, &TEST_NONCE, 0x01, 0, b"");
    assert_eq!(&tag[..], &ctrl[13..29], "golden tag matches compute_tag");
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
    c.bench_function("control_verify_get_caps_29b", |b| {
        b.iter(|| {
            assert!(verify_frame(black_box(&ctrl), &TEST_KEY, &TEST_NONCE));
            ControlFrame::verify(black_box(&ctrl), &TEST_KEY, &TEST_NONCE).expect("verifies")
        });
    });
    c.bench_function("control_build_frame_get_caps", |b| {
        b.iter(|| {
            build_frame(
                &TEST_KEY,
                &TEST_NONCE,
                0x01,
                black_box(1),
                black_box(0),
                b"",
            )
            .expect("builds")
        });
    });
    c.bench_function("hmac_compute_tag_empty_payload", |b| {
        b.iter(|| compute_tag(&TEST_KEY, &TEST_NONCE, 0x01, black_box(0), b""));
    });
    c.bench_function("hmac_verify_tag", |b| {
        b.iter(|| {
            assert!(verify_tag(
                &TEST_KEY,
                &TEST_NONCE,
                0x01,
                black_box(0),
                b"",
                black_box(&tag)
            ));
        });
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
