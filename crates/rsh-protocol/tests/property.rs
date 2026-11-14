//! Property tests (proptest): the parser never panics on arbitrary
//! bytes, round-trips hold, and a single-byte flip rejects.
//!
//! Default proptest configuration: ≥ 256 cases per property.

use proptest::prelude::*;
use rsh_protocol::{
    ConfigBlob, ConfigRecord, ControlFrame, HelloReply, PingFrame, SerialFrame, StatusPacket,
    StreamFlags, StreamFrame,
};

/// Strategy for arbitrary byte vectors up to one max control frame.
fn arb_bytes(max_len: usize) -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 0..=max_len)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Parser never panics on arbitrary bytes: accept or clean reject only.
    #[test]
    fn stream_parser_never_panics(bytes in arb_bytes(64)) {
        let _ = StreamFrame::parse(&bytes);
        let _ = StatusPacket::parse(&bytes);
        let _ = HelloReply::parse(&bytes);
        let _ = PingFrame::parse(&bytes);
        let _ = SerialFrame::parse(&bytes);
        let _ = ConfigBlob::decode(&bytes);
    }

    /// Control parser + tag verifier never panic on arbitrary bytes.
    #[test]
    fn control_parser_never_panics(bytes in arb_bytes(1060)) {
        let key = [0x11u8; 32];
        let nonce = [0x22u8; 16];
        let _ = ControlFrame::parse_structure(&bytes);
        let _ = ControlFrame::verify(&bytes, &key, &nonce);
    }

    /// Round-trip: decode(encode(p)) == p for stream frames.
    #[test]
    fn stream_round_trip(
        seq in any::<u16>(),
        ts in any::<u32>(),
        flags in 0u8..16,
        axes in prop::collection::vec((0usize..8, any::<u16>()), 0..=8),
        enc in prop::collection::vec(any::<i16>(), 0..=4),
        buttons in any::<u32>(),
    ) {
        // Deduplicate axis slots (a slot appears at most once).
        let mut seen = [false; 8];
        let axes: Vec<(usize, u16)> = axes
            .into_iter()
            .filter(|(slot, _)| {
                if seen[*slot] { false } else { seen[*slot] = true; true }
            })
            .collect();
        let flags = StreamFlags::from_byte(flags).expect("flags 0..16 are valid");
        let frame = StreamFrame::new(seq, ts, flags, &axes, &enc, buttons);
        let bytes = frame.encode();
        let decoded = StreamFrame::parse(&bytes).expect("round-trip parses");
        prop_assert_eq!(decoded, frame);
    }

    /// Round-trip: status packets (both lengths).
    #[test]
    fn status_round_trip(
        seq in any::<u16>(),
        uptime in any::<u32>(),
        rssi in any::<i8>(),
        battery in any::<u8>(),
        supply in any::<u16>(),
        heap in any::<u32>(),
        overruns in any::<u16>(),
        max_cycle in any::<u16>(),
        gov in any::<u8>(),
        reset in any::<u8>(),
        cfg_crc in any::<u32>(),
        cfg_ver in any::<u16>(),
        seq_gap in any::<u16>(),
        tx_fail in any::<u16>(),
        btns_ex in prop::option::of(any::<u32>()),
    ) {
        let packet = StatusPacket {
            seq, uptime_ms: uptime, rssi, battery_pct: battery,
            supply_mv: supply, heap, overruns, max_cycle_us: max_cycle,
            governor: gov, reset_reason: reset, cfg_crc, cfg_ver,
            seq_gap, tx_fail, btns_ex,
        };
        let bytes = packet.encode();
        let decoded = StatusPacket::parse(&bytes).expect("round-trip parses");
        prop_assert_eq!(decoded, packet);
    }

    /// Round-trip: control frames (tag included).
    #[test]
    fn control_round_trip(
        cmd in 1u8..=12,
        req_id in any::<u16>(),
        counter in any::<u32>(),
        payload in prop::collection::vec(any::<u8>(), 0..=64),
    ) {
        let key = [0xabu8; 32];
        let nonce = [0xcdu8; 16];
        let frame = ControlFrame { cmd, req_id, counter, payload: payload.clone() };
        let bytes = frame.encode(&key, &nonce).expect("payload <= 1024");
        let decoded = ControlFrame::verify(&bytes, &key, &nonce).expect("round-trip verifies");
        prop_assert_eq!(decoded, frame);
    }

    /// Round-trip: HELLO replies (both paired and unpaired shapes).
    #[test]
    fn hello_round_trip(
        flags in prop::collection::vec(any::<bool>(), 4),
        session_echo in 0u8..=1,
        mac in prop::collection::vec(any::<u8>(), 6),
        nonce in prop::collection::vec(any::<u8>(), 16),
    ) {
        let flags_byte: u8 = flags.iter().enumerate()
            .map(|(i, b)| u8::from(*b) << i).sum();
        let mut mac_arr = [0u8; 6];
        mac_arr.copy_from_slice(&mac);
        let mut nonce_arr = [0u8; 16];
        nonce_arr.copy_from_slice(&nonce);
        let unpaired = flags_byte & 0x01 != 0;
        let window = flags_byte & 0x08 != 0;
        let (key, pw) = if unpaired && window {
            ([0x55u8; 32], [b'A'; 24])
        } else {
            ([0u8; 32], [0u8; 24])
        };
        let reply = HelloReply {
            flags: flags_byte, session_echo, board_mac: mac_arr,
            firmware: (4, 0, 1), nonce: nonce_arr,
            device_key: key, wifi_password: pw,
        };
        let bytes = reply.encode();
        prop_assert_eq!(bytes.len(), 92);
        let decoded = HelloReply::parse(&bytes).expect("round-trip parses");
        prop_assert_eq!(decoded, reply);
    }

    /// Round-trip: ping frames.
    #[test]
    fn ping_round_trip(seq in any::<u16>(), t0 in any::<u64>(), t1 in any::<u64>(), t2 in any::<u64>()) {
        let frame = PingFrame::reply(seq, t0, t1, t2);
        let bytes = frame.encode();
        let decoded = PingFrame::parse(&bytes).expect("round-trip parses");
        prop_assert_eq!(decoded, frame);
    }

    /// Round-trip: config blobs.
    #[test]
    fn config_blob_round_trip(
        cfg_version in any::<u16>(),
        records in prop::collection::vec(
            (any::<u8>(), any::<u8>(), any::<u8>(), any::<u8>(), any::<u8>(), any::<u8>(),
             prop::collection::vec(any::<u8>(), 10)),
            0..=12,
        ),
    ) {
        let records: Vec<ConfigRecord> = records.into_iter().map(|(t, s, f, a, b, c, p)| {
            let mut params = [0u8; 10];
            params.copy_from_slice(&p);
            ConfigRecord { rec_type: t, slot: s, flags: f, pin_a: a, pin_b: b, pin_c: c, params }
        }).collect();
        let blob = ConfigBlob::new(cfg_version, records);
        let bytes = blob.encode().expect("blob encodes");
        let decoded = ConfigBlob::decode(&bytes).expect("round-trip decodes");
        prop_assert_eq!(decoded, blob);
    }

    /// Single-byte flip in a valid stream frame -> reject (never a
    /// silent accept with different contents, never a panic).
    #[test]
    fn stream_single_byte_flip_rejects(
        seq in any::<u16>(),
        ts in any::<u32>(),
        axes in prop::collection::vec((0usize..8, any::<u16>()), 1..=8),
        enc in prop::collection::vec(any::<i16>(), 0..=4),
        idx in 0usize..40,
    ) {
        let mut seen = [false; 8];
        let axes: Vec<(usize, u16)> = axes.into_iter()
            .filter(|(slot, _)| {
                if seen[*slot] { false } else { seen[*slot] = true; true }
            }).collect();
        let frame = StreamFrame::new(seq, ts, StreamFlags::default(), &axes, &enc, 0);
        let mut bytes = frame.encode();
        if idx < bytes.len() {
            bytes[idx] ^= 0xFF;
            // A flip either rejects, or (only for the CRC bytes' own
            // complement edge) still parses — but the decoded frame must
            // never equal the original when any covered byte changed.
            // In practice every flip breaks the CRC, so parse must fail.
            prop_assert!(StreamFrame::parse(&bytes).is_err(),
                "flipped byte {idx} must reject");
        }
    }

    /// Single-byte flip in a valid control frame -> reject (tag or structure).
    /// Exception: bytes 5–6 are `reqId`, which is deliberately NOT in the
    /// HMAC (v2.1-frozen, SEC-D-07a analysis). Flipping `reqId` still
    /// verifies — the changed reqId can only yield DUPLICATE (cached
    /// pair) or REPLAY (old counter), never a new execution — so those
    /// two positions are asserted to verify with a changed reqId instead.
    #[test]
    fn control_single_byte_flip_rejects(idx in 0usize..29) {
        let key = [0x01u8; 32];
        let nonce = [0x02u8; 16];
        let frame = ControlFrame { cmd: 1, req_id: 1, counter: 0, payload: vec![] };
        let mut bytes = frame.encode(&key, &nonce).expect("encode");
        bytes[idx] ^= 0xFF;
        if idx == 5 || idx == 6 {
            let decoded = ControlFrame::verify(&bytes, &key, &nonce)
                .expect("reqId is not covered by the HMAC, so it still verifies");
            prop_assert_ne!(decoded.req_id, 1);
        } else {
            prop_assert!(ControlFrame::verify(&bytes, &key, &nonce).is_err(),
                "flipped byte {idx} must reject");
        }
    }
}
