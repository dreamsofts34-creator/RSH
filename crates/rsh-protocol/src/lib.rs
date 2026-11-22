//! `rsh-protocol`: RSH protocol v2.2 — pure parse/encode.
//!
//! Wire contract: `PROTOCOL-V2.2.md` (authority, WS-5). Byte-identical with the
//! C++/Python implementations; golden vectors live in
//! `tests/golden_vectors.rs` (values computed from the v2.2 layout in
//! `NO-PAIRING-PLAN.md` §3 D1/D2 — HELLO 92 B with zeroed reserved fields,
//! plain `RSHC` frames with a CRC16 trailer, version byte `0x22`).
//!
//! Modules:
//!
//! * [`crc`] — CRC16-CCITT-FALSE (check `0x29B1`) and CRC-32/ISO-HDLC.
//! * [`stream`] — stream (`"RSH2"`) and status (`0xA6`) packets, legacy
//!   detection.
//! * [`control`] — `"RSHC"` frames (CRC only, no HMAC), opcodes, error
//!   codes, reqId-duplicate/retry rules.
//! * [`hello`] — HELO/HELLO discovery and the plain link state.
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
    PinProbeRequest, build_frame, build_reply,
};
pub use crc::{crc16, crc32, verify_crc16_trailer, with_crc16};
pub use error::{
    ConfigError, ControlParseError, HelloError, PingError, ProbeError, RejectCounters, SerialError,
    StatusError, StreamError,
};
pub use hello::{HelloReply, HeloRequest, LinkState};
pub use pin::{
    PinModule, PinModuleKind, SHIFTER_SENSITIVITY_PARAM, free_pins, validate, validate_blob,
};
pub use ping::{PingFrame, PingMeasurement, PingType, measure};
pub use probe::{ProbeFrame, ProbeSamples};
pub use serial::{SerialDecoder, SerialFrame, SerialKind};
pub use stream::{
    DatagramKind, StatusPacket, StreamFlags, StreamFrame, classify_datagram, is_legacy,
};
