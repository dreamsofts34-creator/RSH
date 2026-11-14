//! Golden vectors — byte-identical match gate (SW-R2.4).
//!
//! Source: `reference/tests-host/protocol_v2_vectors.py` (run it to
//! regenerate the reference output). The hex values below were copied
//! verbatim from that script's output; the Python file was **not**
//! modified and no vector was adjusted for Rust. The Rust code must
//! match these bytes.
//!
//! Test fixtures (NOT real secrets): `TEST_KEY = 00..1f`,
//! `TEST_NONCE = a0..af`, `TEST_MAC = aabbccddeeff`,
//! `TEST_PW = "Ab3dEf6hJk9mNp2rSt5vWx8Y"`.

use rsh_protocol::{
    ControlFrame, GetCapsReply, HelloReply, SerialFrame, SerialKind, StatusPacket, StreamFlags,
    StreamFrame, build_reply, compute_tag, crc16, verify_frame,
};

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
const WRONG_KEY: [u8; 32] = [0xFF; 32];

#[test]
fn vector_stream_v2_1() {
    // seq 0x1234, ts 0x01020304, axes 0-4 mV, buttons 0x205,
    // enc cumulative +1000,-250, flags 0x01 (config-changed).
    let expected = hex(
        "52 53 48 32 21 01 34 12 04 03 02 01 1f 02 05 02 00 00 00 00 fc 08 e8 03 d0 07 00 00 00 02 e8 03 06 ff 69 d1",
    );
    assert_eq!(expected.len(), 36);
    let frame = StreamFrame::parse(&expected).expect("stream vector parses");
    assert_eq!(frame.seq, 0x1234);
    assert_eq!(frame.timestamp_us, 0x0102_0304);
    assert!(frame.flags.config_changed);
    assert!(!frame.flags.low_supply);
    assert_eq!(frame.axes_mv[0], Some(2300));
    assert_eq!(frame.axes_mv[1], Some(1000));
    assert_eq!(frame.axes_mv[2], Some(2000));
    assert_eq!(frame.axes_mv[3], Some(0));
    assert_eq!(frame.axes_mv[4], Some(512));
    assert_eq!(frame.axes_mv[5], None);
    assert_eq!(frame.buttons, 0x205);
    assert_eq!(frame.n_enc, 2);
    assert_eq!(frame.encoders[0], 1000);
    assert_eq!(frame.encoders[1], -250);
    // Byte-identical re-encode.
    assert_eq!(frame.encode(), expected);
    // And encode from scratch matches too.
    let rebuilt = StreamFrame::new(
        0x1234,
        0x0102_0304,
        StreamFlags {
            config_changed: true,
            ..StreamFlags::default()
        },
        &[(0, 2300), (1, 1000), (2, 2000), (3, 0), (4, 512)],
        &[1000, -250],
        0x205,
    );
    assert_eq!(rebuilt.encode(), expected);
}

#[test]
fn vector_status_v2_1_base() {
    // 34 B: seqGap 2, txFail 1.
    let expected = hex(
        "a6 21 07 00 40 e2 01 00 cc 56 ac 0f 78 da 02 00 03 00 5c 08 00 01 ef be ad de 05 00 02 00 01 00 97 00",
    );
    assert_eq!(expected.len(), 34);
    let status = StatusPacket::parse(&expected).expect("status vector parses");
    assert_eq!(status.seq, 7);
    assert_eq!(status.uptime_ms, 123_456);
    assert_eq!(status.rssi, -52);
    assert_eq!(status.battery_pct, 86);
    assert_eq!(status.supply_mv, 4012);
    assert_eq!(status.heap, 187_000);
    assert_eq!(status.overruns, 3);
    assert_eq!(status.max_cycle_us, 2140);
    assert_eq!(status.governor, 0);
    assert_eq!(status.reset_reason, 1);
    assert_eq!(status.cfg_crc, 0xDEAD_BEEF);
    assert_eq!(status.cfg_ver, 5);
    assert_eq!(status.seq_gap, 2);
    assert_eq!(status.tx_fail, 1);
    assert_eq!(status.btns_ex, None);
    assert_eq!(status.encode(), expected);
}

#[test]
fn vector_status_v2_1_extended() {
    // 38 B: btnsEx = buttons 32+5 set (bit 5).
    let expected = hex(
        "a6 21 08 00 41 e2 01 00 ce 56 aa 0f 78 da 02 00 03 00 5c 08 00 01 ef be ad de 05 00 20 00 00 00 02 00 01 00 e1 1e",
    );
    assert_eq!(expected.len(), 38);
    let status = StatusPacket::parse(&expected).expect("extended status parses");
    assert_eq!(status.seq, 8);
    assert_eq!(status.seq_gap, 2);
    assert_eq!(status.tx_fail, 1);
    assert_eq!(status.btns_ex, Some(0x20));
    assert_eq!(status.encode(), expected);
}

#[test]
fn vector_hello_unpaired() {
    // flags 0b00001001 = unpaired | window open, session echo 1,
    // key + password present.
    let expected = hex(
        "52 53 48 52 02 01 09 01 aa bb cc dd ee ff 04 00 00 00 a0 a1 a2 a3 a4 a5 a6 a7 a8 a9 aa ab ac ad ae af 00 01 02 03 04 05 06 07 08 09 0a 0b 0c 0d 0e 0f 10 11 12 13 14 15 16 17 18 19 1a 1b 1c 1d 1e 1f 41 62 33 64 45 66 36 68 4a 6b 39 6d 4e 70 32 72 53 74 35 76 57 78 38 59 5a 42",
    );
    assert_eq!(expected.len(), 92);
    let hello = HelloReply::parse(&expected).expect("HELLO vector parses");
    assert!(hello.unpaired());
    assert!(hello.pairing_window_open());
    assert!(hello.credentials_present());
    assert_eq!(hello.session_echo, 1);
    assert_eq!(hello.board_mac, [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]);
    assert_eq!(hello.firmware, (4, 0, 0));
    assert_eq!(hello.nonce, TEST_NONCE);
    assert_eq!(hello.device_key, TEST_KEY);
    assert_eq!(hello.wifi_password_str(), Some("Ab3dEf6hJk9mNp2rSt5vWx8Y"));
    assert_eq!(hello.encode(), expected);
}

#[test]
fn vector_hello_paired() {
    // flags 0, key/password all zero.
    let expected = hex(
        "52 53 48 52 02 01 00 00 aa bb cc dd ee ff 04 00 00 00 a0 a1 a2 a3 a4 a5 a6 a7 a8 a9 aa ab ac ad ae af 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 24 45",
    );
    assert_eq!(expected.len(), 92);
    let hello = HelloReply::parse(&expected).expect("paired HELLO parses");
    assert!(!hello.unpaired());
    assert!(!hello.credentials_present());
    assert_eq!(hello.device_key, [0u8; 32]);
    assert_eq!(hello.wifi_password, [0u8; 24]);
    assert_eq!(hello.wifi_password_str(), None);
    assert_eq!(hello.encode(), expected);
}

#[test]
fn vector_ctrl_get_caps() {
    // GET_CAPS, reqId 1, counter 0, empty payload.
    let expected = hex(
        "52 53 48 43 01 01 00 00 00 00 00 00 00 42 ee 8d 10 07 93 58 d9 93 6f e7 43 68 f8 b6 61",
    );
    assert_eq!(expected.len(), 29);
    assert!(verify_frame(&expected, &TEST_KEY, &TEST_NONCE));
    let frame =
        ControlFrame::verify(&expected, &TEST_KEY, &TEST_NONCE).expect("GET_CAPS vector verifies");
    assert_eq!(frame.cmd, 0x01);
    assert_eq!(frame.req_id, 1);
    assert_eq!(frame.counter, 0);
    assert_eq!(frame.payload, [] as [u8; 0]);
    // Re-encode byte-identical.
    assert_eq!(
        frame.encode(&TEST_KEY, &TEST_NONCE).expect("encode"),
        expected
    );
    // Tag formula cross-check: HMAC over nonce || counter_LE || cmd || payload.
    let tag = compute_tag(&TEST_KEY, &TEST_NONCE, 0x01, 0, b"");
    assert_eq!(&tag[..], &expected[13..29]);
}

#[test]
fn vector_ctrl_set_cfg() {
    // SET_CFG chunk, reqId 2, counter 1 (offset 0, total 64, 16 B data).
    let expected = hex(
        "52 53 48 43 03 02 00 01 00 00 00 18 00 00 00 40 00 10 00 eb 52 52 53 48 47 00 00 00 00 00 00 00 00 00 00 00 00 5e 0c 65 66 c9 dd 5e 79 1b e0 ae 3e 77 66 2d 4c",
    );
    assert_eq!(expected.len(), 53);
    assert!(verify_frame(&expected, &TEST_KEY, &TEST_NONCE));
    let frame =
        ControlFrame::verify(&expected, &TEST_KEY, &TEST_NONCE).expect("SET_CFG vector verifies");
    assert_eq!(frame.cmd, 0x03);
    assert_eq!(frame.req_id, 2);
    assert_eq!(frame.counter, 1);
    assert_eq!(frame.payload.len(), 24);
    // Chunk header: offset 0, total 64, chunkLen 16, chunkCrc over data.
    let payload = &frame.payload;
    assert_eq!(u16::from_le_bytes([payload[0], payload[1]]), 0);
    assert_eq!(u16::from_le_bytes([payload[2], payload[3]]), 64);
    assert_eq!(u16::from_le_bytes([payload[4], payload[5]]), 16);
    assert_eq!(
        u16::from_le_bytes([payload[6], payload[7]]),
        crc16(&payload[8..])
    );
    assert_eq!(&payload[8..12], b"RSHG");
    assert_eq!(
        frame.encode(&TEST_KEY, &TEST_NONCE).expect("encode"),
        expected
    );
}

#[test]
fn vector_ctrl_get_caps_reply() {
    // GET_CAPS OK reply, reqId 1, counter 0; body = MAC + fw/proto/limits.
    let expected = hex(
        "52 53 48 43 81 01 00 00 00 00 00 12 00 00 aa bb cc dd ee ff 04 00 00 02 01 08 40 04 06 00 00 04 b2 21 24 07 63 4f fd 85 52 f4 91 ba 7d cb c0",
    );
    assert_eq!(expected.len(), 47);
    assert!(verify_frame(&expected, &TEST_KEY, &TEST_NONCE));
    let frame =
        ControlFrame::verify(&expected, &TEST_KEY, &TEST_NONCE).expect("GET_CAPS reply verifies");
    assert_eq!(frame.cmd, 0x81);
    assert!(frame.is_reply());
    assert_eq!(frame.request_cmd(), 0x01);
    let caps = GetCapsReply::from_frame(&frame).expect("caps body parses");
    assert_eq!(caps.status, rsh_protocol::ErrorCode::Ok);
    assert_eq!(caps.board_mac, [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]);
    assert_eq!(caps.firmware, (4, 0, 0));
    assert_eq!(caps.protocol, (2, 1));
    assert_eq!(caps.n_axes_max, 8);
    assert_eq!(caps.n_btn_max, 64);
    assert_eq!(caps.n_enc_max, 4);
    assert_eq!(caps.wifi_channel, 6);
    assert_eq!(caps.pin_table, [] as [rsh_protocol::PinTableEntry; 0]);
    // Rebuild the identical reply from parts.
    let rebuilt = build_reply(
        &TEST_KEY,
        &TEST_NONCE,
        0x01,
        1,
        0,
        rsh_protocol::ErrorCode::Ok,
        &frame.payload[1..],
    )
    .expect("reply rebuilds");
    assert_eq!(rebuilt, expected);
}

#[test]
fn vector_serial_stream() {
    // kind=Stream(1), payload 01..08 -> CRC 0x5DD9.
    let expected = hex("525348530108000102030405060708d95d");
    assert_eq!(expected.len(), 17);
    let frame = SerialFrame::parse(&expected).expect("serial stream parses");
    assert_eq!(frame.kind, SerialKind::Stream);
    assert_eq!(frame.payload, vec![1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(frame.encode().expect("encode"), expected);
}

#[test]
fn vector_serial_helo() {
    // kind=Helo(6), payload "HELO" 02 00 -> CRC 0x4947.
    let expected = hex("5253485306060048454c4f02004749");
    assert_eq!(expected.len(), 15);
    let frame = SerialFrame::parse(&expected).expect("serial HELO parses");
    assert_eq!(frame.kind, SerialKind::Helo);
    assert_eq!(frame.payload, b"HELO\x02\x00".to_vec());
    assert_eq!(frame.encode().expect("encode"), expected);
}

#[test]
fn vector_crc16_v1_golden() {
    // Appendix A: v1 packet CRC over the first 16 bytes is 0xBD75.
    let v1 = hex("a5013412040302 01fc08e803d007050175bd");
    assert_eq!(crc16(&v1[..16]), 0xBD75);
    assert_eq!(crc16(b"123456789"), 0x29B1);
}

// ---- negative vectors (T2): each must FAIL verification / be rejected ----

#[test]
fn negative_bad_tag() {
    let good = hex(
        "52 53 48 43 01 01 00 00 00 00 00 00 00 42 ee 8d 10 07 93 58 d9 93 6f e7 43 68 f8 b6 61",
    );
    let mut flipped = good.clone();
    let last = flipped.len() - 1;
    flipped[last] ^= 0xFF;
    assert!(!verify_frame(&flipped, &TEST_KEY, &TEST_NONCE));
    assert_eq!(
        ControlFrame::verify(&flipped, &TEST_KEY, &TEST_NONCE).unwrap_err(),
        rsh_protocol::ControlParseError::BadTag
    );
}

#[test]
fn negative_wrong_key() {
    let good = hex(
        "52 53 48 43 01 01 00 00 00 00 00 00 00 42 ee 8d 10 07 93 58 d9 93 6f e7 43 68 f8 b6 61",
    );
    assert!(!verify_frame(&good, &WRONG_KEY, &TEST_NONCE));
}

#[test]
fn negative_truncated_control_is_bad_length_not_too_short() {
    // 29 B frame cut to 21 B: header readable -> BAD_LENGTH, not TOO_SHORT.
    let good = hex(
        "52 53 48 43 01 01 00 00 00 00 00 00 00 42 ee 8d 10 07 93 58 d9 93 6f e7 43 68 f8 b6 61",
    );
    let truncated = &good[..21];
    assert_eq!(
        ControlFrame::verify(truncated, &TEST_KEY, &TEST_NONCE).unwrap_err(),
        rsh_protocol::ControlParseError::BadLength
    );
    // Under 13 B -> TOO_SHORT.
    assert_eq!(
        ControlFrame::verify(&good[..12], &TEST_KEY, &TEST_NONCE).unwrap_err(),
        rsh_protocol::ControlParseError::TooShort
    );
}

#[test]
fn negative_replay_and_duplicate_are_state_rules_not_tag_rules() {
    // Replay frame (counter 0 reused, new reqId 99) verifies
    // cryptographically — the receiver rejects it by counter state.
    let replay = hex(
        "52 53 48 43 01 63 00 00 00 00 00 00 00 42 ee 8d 10 07 93 58 d9 93 6f e7 43 68 f8 b6 61",
    );
    assert!(verify_frame(&replay, &TEST_KEY, &TEST_NONCE));
    let frame = ControlFrame::verify(&replay, &TEST_KEY, &TEST_NONCE).expect("verifies");
    assert_eq!(frame.req_id, 99);
    assert_eq!(frame.counter, 0);
    let mut tracker = rsh_protocol::ReplayTracker::new();
    assert!(tracker.observe(0));
    assert!(tracker.is_replay(frame.counter));

    // Exact duplicate retransmit verifies too; the duplicate cache catches it.
    let good = hex(
        "52 53 48 43 01 01 00 00 00 00 00 00 00 42 ee 8d 10 07 93 58 d9 93 6f e7 43 68 f8 b6 61",
    );
    assert!(verify_frame(&good, &TEST_KEY, &TEST_NONCE));
    let mut cache = rsh_protocol::DuplicateCache::new();
    assert!(cache.insert(1, 0));
    assert!(!cache.insert(1, 0));
    assert!(cache.is_duplicate(1, 0));
}

#[test]
fn negative_bad_magic_control() {
    let bad = hex(
        "58 58 58 58 01 01 00 00 00 00 00 00 00 42 ee 8d 10 07 93 58 d9 93 6f e7 43 68 f8 b6 61",
    );
    assert!(!verify_frame(&bad, &TEST_KEY, &TEST_NONCE));
    assert_eq!(
        ControlFrame::verify(&bad, &TEST_KEY, &TEST_NONCE).unwrap_err(),
        rsh_protocol::ControlParseError::BadMagic
    );
}

#[test]
fn negative_stream_rejects_counted_per_reason() {
    use rsh_protocol::{RejectCounters, StreamError};
    let mut counters = RejectCounters::new();
    // Bad CRC (flip a payload byte of the stream vector).
    let mut bad_crc = hex(
        "52 53 48 32 21 01 34 12 04 03 02 01 1f 02 05 02 00 00 00 00 fc 08 e8 03 d0 07 00 00 00 02 e8 03 06 ff 69 d1",
    );
    bad_crc[20] ^= 0xFF;
    assert_eq!(
        StreamFrame::parse_counted(&bad_crc, &mut counters).unwrap_err(),
        StreamError::BadCrc
    );
    // Nonzero reserved.
    let mut rsv = hex(
        "52 53 48 32 21 01 34 12 04 03 02 01 1f 02 05 02 00 00 00 00 fc 08 e8 03 d0 07 00 00 00 02 e8 03 06 ff 69 d1",
    );
    rsv[18] = 1;
    assert_eq!(
        StreamFrame::parse_counted(&rsv, &mut counters).unwrap_err(),
        StreamError::RsvNonzero
    );
    // nEnc > 4 (header says 5 encoders; length then also mismatches —
    // nEnc is checked first, so BadFrame).
    let mut n_enc = hex(
        "52 53 48 32 21 01 34 12 04 03 02 01 1f 02 05 02 00 00 00 00 fc 08 e8 03 d0 07 00 00 00 02 e8 03 06 ff 69 d1",
    );
    n_enc[13] = 5;
    assert_eq!(
        StreamFrame::parse_counted(&n_enc, &mut counters).unwrap_err(),
        StreamError::BadFrame
    );
    // Bad version.
    let mut ver = hex(
        "52 53 48 32 21 01 34 12 04 03 02 01 1f 02 05 02 00 00 00 00 fc 08 e8 03 d0 07 00 00 00 02 e8 03 06 ff 69 d1",
    );
    ver[4] = 0x20;
    assert_eq!(
        StreamFrame::parse_counted(&ver, &mut counters).unwrap_err(),
        StreamError::BadVersion
    );
    // Bad magic.
    let bad_magic = hex("58585858");
    assert_eq!(
        StreamFrame::parse_counted(&bad_magic, &mut counters).unwrap_err(),
        StreamError::BadMagic
    );
    // Legacy detected, not parsed.
    let legacy = hex("a5013412040302 01fc08e803d007050175bd");
    assert_eq!(
        StreamFrame::parse_counted(&legacy, &mut counters).unwrap_err(),
        StreamError::Legacy
    );
    assert_eq!(counters.bad_crc, 1);
    assert_eq!(counters.rsv_nonzero, 1);
    assert_eq!(counters.bad_frame, 1);
    assert_eq!(counters.bad_version, 1);
    assert_eq!(counters.bad_magic, 1);
    assert_eq!(counters.legacy, 1);
    assert_eq!(counters.total(), 6);
}

#[test]
fn negative_serial_crc_flip_rejected() {
    let frame = hex("525348530108000102030405060708d95d");
    let mut flipped = frame.clone();
    let last = flipped.len() - 1;
    flipped[last] ^= 0xFF;
    assert_eq!(
        SerialFrame::parse(&flipped).unwrap_err(),
        rsh_protocol::SerialError::BadCrc
    );
}
