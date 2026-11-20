//! Golden vectors — byte-identical match gate (SW-R2.4), protocol v2.2.
//!
//! Source: the v2.2 layout in `NO-PAIRING-PLAN.md` §3 (D1/D2) — HELLO
//! 92 B with zeroed reserved fields, plain `RSHC` frames with a CRC16
//! trailer (`magic | cmd | reqId u16 | len u16 | payload | CRC16`),
//! version byte `0x22`. The hex values below were computed with the
//! same CRC16-CCITT-FALSE as `firmware/tests-host/protocol_v2_vectors.py`
//! (WS-1 regenerates that script for v2.2; these vectors must match its
//! output — if they diverge, the layout decision, not the vectors, is
//! what needs reconciling).
//!
//! Test fixtures (NOT real secrets): `TEST_MAC = aabbccddeeff`,
//! `TEST_PW = "Ab3dEf6hJk9mNp2rSt5vWx8Y"` (only in the old-firmware
//! HELLO vector, where a v2.1 board still discloses it).

use rsh_protocol::{
    ControlFrame, GetCapsReply, HelloReply, SerialFrame, SerialKind, StatusPacket, StreamFlags,
    StreamFrame, build_reply, crc16,
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

#[test]
fn vector_stream_v2_2() {
    // seq 0x1234, ts 0x01020304, axes 0-4 mV, buttons 0x205,
    // enc cumulative +1000,-250, flags 0x01 (config-changed).
    let expected = hex(
        "52 53 48 32 22 01 34 12 04 03 02 01 1f 02 05 02 00 00 00 00 fc 08 e8 03 d0 07 00 00 00 02 e8 03 06 ff 48 ca",
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
fn vector_status_v2_2_base() {
    // 34 B: seqGap 2, txFail 1.
    let expected = hex(
        "a6 22 07 00 40 e2 01 00 cc 56 ac 0f 78 da 02 00 03 00 5c 08 00 01 ef be ad de 05 00 02 00 01 00 cd 82",
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
fn vector_status_v2_2_extended() {
    // 38 B: btnsEx = buttons 32+5 set (bit 5).
    let expected = hex(
        "a6 22 08 00 41 e2 01 00 ce 56 ac 0f 78 da 02 00 03 00 5c 08 00 01 ef be ad de 05 00 20 00 00 00 02 00 01 00 9b 6e",
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
fn vector_hello_v2_2() {
    // flags 0, session echo 0, fw 4.0.3, nonce/key/password all zero
    // (reserved fields).
    let expected = hex(
        "52 53 48 52 02 02 00 00 aa bb cc dd ee ff 04 00 03 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 ba f6",
    );
    assert_eq!(expected.len(), 92);
    let hello = HelloReply::parse(&expected).expect("HELLO vector parses");
    assert_eq!(hello.protocol, (2, 2));
    assert!(!hello.is_old_protocol());
    assert!(!hello.safe_mode());
    assert_eq!(hello.session_echo, 0);
    assert_eq!(hello.board_mac, [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]);
    assert_eq!(hello.firmware, (4, 0, 3));
    assert_eq!(hello.nonce, [0u8; 16]);
    assert_eq!(hello.device_key, [0u8; 32]);
    assert_eq!(hello.wifi_password, [0u8; 24]);
    assert_eq!(hello.encode(), expected);
}

#[test]
fn vector_hello_v2_2_safe_mode() {
    // flags 0x04 (safe_mode — the only defined flag bit in v2.2).
    let expected = hex(
        "52 53 48 52 02 02 04 00 aa bb cc dd ee ff 04 00 03 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 59 41",
    );
    assert_eq!(expected.len(), 92);
    let hello = HelloReply::parse(&expected).expect("safe-mode HELLO parses");
    assert!(hello.safe_mode());
    assert_eq!(hello.encode(), expected);
}

#[test]
fn vector_hello_v2_1_old_firmware_still_parses() {
    // A pre-redesign board: protocol (2,1), flags 0b00001001 =
    // unpaired | window open, key + password disclosed. It still
    // parses — the caller detects it via `protocol == (2,1)` — but
    // its credentials are never acted on.
    let expected = hex(
        "52 53 48 52 02 01 09 01 aa bb cc dd ee ff 04 00 00 00 a0 a1 a2 a3 a4 a5 a6 a7 a8 a9 aa ab ac ad ae af 00 01 02 03 04 05 06 07 08 09 0a 0b 0c 0d 0e 0f 10 11 12 13 14 15 16 17 18 19 1a 1b 1c 1d 1e 1f 41 62 33 64 45 66 36 68 4a 6b 39 6d 4e 70 32 72 53 74 35 76 57 78 38 59 5a 42",
    );
    assert_eq!(expected.len(), 92);
    let hello = HelloReply::parse(&expected).expect("old HELLO parses");
    assert_eq!(hello.protocol, (2, 1));
    assert!(hello.is_old_protocol());
    assert_eq!(hello.session_echo, 1);
    assert_eq!(hello.board_mac, [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]);
    assert_eq!(hello.firmware, (4, 0, 0));
    // The disclosed bytes are present in the struct but reserved:
    // encode() emits v2.2, so this is not a round-trip.
    assert_ne!(hello.encode(), expected);
}

#[test]
fn vector_ctrl_get_caps() {
    // GET_CAPS, reqId 1, empty payload, CRC16 trailer.
    let expected = hex("52 53 48 43 01 01 00 00 00 77 e7");
    assert_eq!(expected.len(), 11);
    let frame = ControlFrame::verify(&expected).expect("GET_CAPS vector verifies");
    assert_eq!(frame.cmd, 0x01);
    assert_eq!(frame.req_id, 1);
    assert_eq!(frame.payload, [] as [u8; 0]);
    // Re-encode byte-identical.
    assert_eq!(frame.encode().expect("encode"), expected);
}

#[test]
fn vector_ctrl_set_cfg() {
    // SET_CFG chunk, reqId 2 (offset 0, total 64, 16 B data).
    let expected = hex(
        "52 53 48 43 03 02 00 18 00 00 00 40 00 10 00 eb 52 52 53 48 47 00 00 00 00 00 00 00 00 00 00 00 00 87 9d",
    );
    assert_eq!(expected.len(), 35);
    let frame = ControlFrame::verify(&expected).expect("SET_CFG vector verifies");
    assert_eq!(frame.cmd, 0x03);
    assert_eq!(frame.req_id, 2);
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
    assert_eq!(frame.encode().expect("encode"), expected);
}

#[test]
fn vector_ctrl_get_caps_reply() {
    // GET_CAPS OK reply, reqId 1; body = MAC + fw/proto/limits.
    let expected = hex(
        "52 53 48 43 81 01 00 12 00 00 aa bb cc dd ee ff 04 00 00 02 02 08 40 04 06 00 00 29 e8",
    );
    assert_eq!(expected.len(), 29);
    let frame = ControlFrame::verify(&expected).expect("GET_CAPS reply verifies");
    assert_eq!(frame.cmd, 0x81);
    assert!(frame.is_reply());
    assert_eq!(frame.request_cmd(), 0x01);
    let caps = GetCapsReply::from_frame(&frame).expect("caps body parses");
    assert_eq!(caps.status, rsh_protocol::ErrorCode::Ok);
    assert_eq!(caps.board_mac, [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]);
    assert_eq!(caps.firmware, (4, 0, 0));
    assert_eq!(caps.protocol, (2, 2));
    assert_eq!(caps.n_axes_max, 8);
    assert_eq!(caps.n_btn_max, 64);
    assert_eq!(caps.n_enc_max, 4);
    assert_eq!(caps.wifi_channel, 6);
    assert_eq!(caps.pin_table, [] as [rsh_protocol::PinTableEntry; 0]);
    // Rebuild the identical reply from parts.
    let rebuilt = build_reply(0x01, 1, rsh_protocol::ErrorCode::Ok, &frame.payload[1..])
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
fn negative_bad_crc() {
    let good = hex("52 53 48 43 01 01 00 00 00 77 e7");
    let mut flipped = good.clone();
    let last = flipped.len() - 1;
    flipped[last] ^= 0xFF;
    assert_eq!(
        ControlFrame::verify(&flipped).unwrap_err(),
        rsh_protocol::ControlParseError::BadCrc
    );
    // A flipped reqId byte also fails the CRC (reqId is CRC-covered
    // in v2.2, unlike the HMAC in v2.1).
    let mut flipped_req = good.clone();
    flipped_req[5] ^= 0xFF; // reqId low byte
    assert_eq!(
        ControlFrame::verify(&flipped_req).unwrap_err(),
        rsh_protocol::ControlParseError::BadCrc
    );
}

#[test]
fn negative_truncated_control_is_bad_length_not_too_short() {
    // 11 B frame cut to 10 B: header readable -> BAD_LENGTH, not TOO_SHORT.
    let good = hex("52 53 48 43 01 01 00 00 00 77 e7");
    let truncated = &good[..10];
    assert_eq!(
        ControlFrame::verify(truncated).unwrap_err(),
        rsh_protocol::ControlParseError::BadLength
    );
    // Under 9 B -> TOO_SHORT.
    assert_eq!(
        ControlFrame::verify(&good[..8]).unwrap_err(),
        rsh_protocol::ControlParseError::TooShort
    );
}

#[test]
fn negative_duplicate_req_id_is_state_not_crc() {
    // A retransmit (same reqId, same bytes) still verifies — the
    // duplicate cache catches it, not the CRC.
    let good = hex("52 53 48 43 01 01 00 00 00 77 e7");
    let frame = ControlFrame::verify(&good).expect("verifies");
    assert_eq!(frame.req_id, 1);
    let mut cache = rsh_protocol::DuplicateCache::new();
    assert!(cache.insert(frame.req_id));
    assert!(!cache.insert(frame.req_id));
    assert!(cache.is_duplicate(frame.req_id));
    assert!(!cache.is_duplicate(99));
}

#[test]
fn negative_bad_magic_control() {
    let bad = hex("58 58 58 58 01 01 00 00 00 77 e7");
    assert_eq!(
        ControlFrame::verify(&bad).unwrap_err(),
        rsh_protocol::ControlParseError::BadMagic
    );
}

#[test]
fn negative_stream_rejects_counted_per_reason() {
    use rsh_protocol::{RejectCounters, StreamError};
    let mut counters = RejectCounters::new();
    // Bad CRC (flip a payload byte of the stream vector).
    let mut bad_crc = hex(
        "52 53 48 32 22 01 34 12 04 03 02 01 1f 02 05 02 00 00 00 00 fc 08 e8 03 d0 07 00 00 00 02 e8 03 06 ff 48 ca",
    );
    bad_crc[20] ^= 0xFF;
    assert_eq!(
        StreamFrame::parse_counted(&bad_crc, &mut counters).unwrap_err(),
        StreamError::BadCrc
    );
    // Nonzero reserved.
    let mut rsv = hex(
        "52 53 48 32 22 01 34 12 04 03 02 01 1f 02 05 02 00 00 00 00 fc 08 e8 03 d0 07 00 00 00 02 e8 03 06 ff 48 ca",
    );
    rsv[18] = 1;
    assert_eq!(
        StreamFrame::parse_counted(&rsv, &mut counters).unwrap_err(),
        StreamError::RsvNonzero
    );
    // nEnc > 4 (header says 5 encoders; length then also mismatches —
    // nEnc is checked first, so BadFrame).
    let mut n_enc = hex(
        "52 53 48 32 22 01 34 12 04 03 02 01 1f 02 05 02 00 00 00 00 fc 08 e8 03 d0 07 00 00 00 02 e8 03 06 ff 48 ca",
    );
    n_enc[13] = 5;
    assert_eq!(
        StreamFrame::parse_counted(&n_enc, &mut counters).unwrap_err(),
        StreamError::BadFrame
    );
    // Bad version (0x21 is now the old version).
    let mut ver = hex(
        "52 53 48 32 22 01 34 12 04 03 02 01 1f 02 05 02 00 00 00 00 fc 08 e8 03 d0 07 00 00 00 02 e8 03 06 ff 48 ca",
    );
    ver[4] = 0x21;
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
