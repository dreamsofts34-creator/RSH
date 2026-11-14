//! `rsh-protocol`: RSH protocol v2.1 — pure parse/encode.
//!
//! Wire contract: `PROTOCOL-V2.1.md` (authority). Byte-identical with the
//! C#/C++/Python implementations; golden vectors live in
//! `tests/golden_vectors.rs` (values copied from
//! `reference/tests-host/protocol_v2_vectors.py` — the Rust code must
//! match *them*, never the other way round).
//!
//! Modules:
//!
//! * [`crc`] — CRC16-CCITT-FALSE (check `0x29B1`) and CRC-32/ISO-HDLC.
//! * [`stream`] — stream (`"RSH2"`) and status (`0xA6`) packets, legacy
//!   detection.
//! * [`control`] — `"RSHC"` frames, HMAC-SHA256 tags, opcodes, error
//!   codes, replay/duplicate/retry rules.
//! * [`hello`] — HELO/HELLO pairing and the session state machine (D30).
//! * [`ping`] — `"RSHN"` NTP-style ping and delay/offset formulas.
//! * [`config`] — `"RSHG"` config blob and `SET_CFG` chunk staging.
//! * [`caps`] — `GET_CAPS` reply body and pin table.
//! * [`probe`] — `"RSHP"` `PIN_PROBE` stream frames.
//! * [`serial`] — `"RSHS"` wired USB-serial envelope + decoder.
//! * [`pin`] — PIN-RULES validator (board JSON embedded, never hard-coded).
//! * [`error`] — error types and per-reason reject counters.
//!
//! No `tokio`, no `std`-net, no I/O of any kind: this crate is pure so
//! it can be fuzzed and checked against the golden vectors byte-for-byte.

pub mod caps;
pub mod config;
pub mod control;
pub mod crc;
pub mod error;
pub mod hello;
pub mod pin;
pub mod ping;
pub mod probe;
pub mod serial;
pub mod stream;

pub use caps::{GetCapsReply, PinTableEntry};
pub use config::{ConfigBlob, ConfigRecord, ConfigStaging, SetCfgChunk, factory_default_blob};
pub use control::{
    ControlFrame, ControlRetry, DuplicateCache, ErrorCode, GetCfgRequest, LedTestPattern, Opcode,
    PinProbeRequest, ReplayTracker, build_frame, build_reply, compute_tag, verify_frame,
    verify_tag,
};
pub use crc::{crc16, crc32, verify_crc16_trailer, with_crc16};
pub use error::{
    ConfigError, ControlParseError, HelloError, PingError, ProbeError, RejectCounters, SerialError,
    StatusError, StreamError,
};
pub use hello::{HelloReply, HeloRequest, SessionState};
pub use pin::{PinModule, PinModuleKind, free_pins, validate, validate_blob};
pub use ping::{PingFrame, PingMeasurement, PingType, measure};
pub use probe::{ProbeFrame, ProbeSamples};
pub use serial::{SerialDecoder, SerialFrame, SerialKind};
pub use stream::{
    DatagramKind, StatusPacket, StreamFlags, StreamFrame, classify_datagram, is_legacy,
};
