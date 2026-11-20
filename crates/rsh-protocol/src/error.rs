//! Shared error types and per-reason reject counters.
//!
//! Every parser rejects with a distinct error variant and bumps a distinct
//! counter in [`RejectCounters`], so a caller can tell *why* a datagram was
//! dropped (PROTOCOL-V2.2.md §1.4: `badMagic`, `badVersion`, `badCrc`,
//! `badFrame`, `rsvNonzero`).

use core::fmt;

/// Distinct reject counters, one per reject reason.
///
/// Parsers that take `&mut RejectCounters` bump exactly one counter per
/// rejected datagram. Counters saturate at `u64::MAX` instead of wrapping.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RejectCounters {
    /// Bad magic bytes (stream / status / control / serial / ping).
    pub bad_magic: u64,
    /// Wrong version byte.
    pub bad_version: u64,
    /// CRC16 (or CRC32) mismatch.
    pub bad_crc: u64,
    /// Structural problem: length mismatch, `nEnc > 4`, out-of-bounds claim.
    pub bad_frame: u64,
    /// Nonzero reserved byte(s).
    pub rsv_nonzero: u64,
    /// Counter/state replay (`REPLAY`, duplicate `reqId`).
    pub replay: u64,
    /// Exact retransmit of a cached `reqId`.
    pub duplicate: u64,
    /// Frame shorter than its header (`TOO_SHORT` / `TooShort`).
    pub too_short: u64,
    /// Header readable but length does not match (`BAD_LENGTH`).
    pub bad_length: u64,
    /// Legacy v1 packet (`0xA5 0x01`) seen — recognised, not parsed as v2.
    pub legacy: u64,
}

impl RejectCounters {
    /// Create an all-zero counter set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Total number of rejected datagrams across all reasons.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.bad_magic
            .saturating_add(self.bad_version)
            .saturating_add(self.bad_crc)
            .saturating_add(self.bad_frame)
            .saturating_add(self.rsv_nonzero)
            .saturating_add(self.replay)
            .saturating_add(self.duplicate)
            .saturating_add(self.too_short)
            .saturating_add(self.bad_length)
            .saturating_add(self.legacy)
    }
}

/// Error returned when a stream datagram (§1.1) cannot be parsed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamError {
    /// Fewer than 4 bytes — the magic cannot even be read.
    TooShort,
    /// First 4 bytes are not `"RSH2"` (and not the legacy `0xA5 0x01`).
    BadMagic,
    /// Legacy v1 packet (`0xA5 0x01`): recognised, deliberately not parsed.
    Legacy,
    /// Version byte is not `0x22`.
    BadVersion,
    /// `nEnc > 4`, or the datagram length does not equal
    /// `20 + 2·popcount(axisMask) + 2·nEnc + 2`.
    BadFrame,
    /// Reserved bytes 18–19 are nonzero, or flags bits 4–7 are nonzero.
    RsvNonzero,
    /// Trailing CRC16 does not match.
    BadCrc,
}

impl fmt::Display for StreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::TooShort => "stream frame too short to read header",
            Self::BadMagic => "stream frame bad magic (not RSH2)",
            Self::Legacy => "legacy v1 packet (0xA5 0x01), not parsed as v2.2",
            Self::BadVersion => "stream frame version is not 0x22",
            Self::BadFrame => "stream frame length or encoder count out of bounds",
            Self::RsvNonzero => "stream frame reserved byte(s) nonzero",
            Self::BadCrc => "stream frame CRC16 mismatch",
        };
        f.write_str(s)
    }
}

impl core::error::Error for StreamError {}

/// Error returned when a status packet (§1.2) cannot be parsed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusError {
    /// Fewer than 2 bytes, or not 34/38 bytes long.
    BadLength,
    /// First byte is not `0xA6`.
    BadMagic,
    /// Version byte is not `0x22`.
    BadVersion,
    /// Trailing CRC16 does not match.
    BadCrc,
}

impl fmt::Display for StatusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::BadLength => "status packet length is not 34 or 38 bytes",
            Self::BadMagic => "status packet magic is not 0xA6",
            Self::BadVersion => "status packet version is not 0x22",
            Self::BadCrc => "status packet CRC16 mismatch",
        };
        f.write_str(s)
    }
}

impl core::error::Error for StatusError {}

/// Structural parse error for a control frame (§3.1).
///
/// `TooShort` is returned **only** when the frame is shorter than the 9-byte
/// header (the `len` field itself cannot be read). Once the header has been
/// read, every length mismatch — `len > 1024`, truncation, extra bytes — is
/// `BadLength`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlParseError {
    /// Frame is shorter than 9 bytes.
    TooShort,
    /// Magic is not `"RSHC"`.
    BadMagic,
    /// Header readable but length does not satisfy `9 + len + 2`,
    /// or `len > 1024`.
    BadLength,
    /// Trailing CRC16 does not match.
    BadCrc,
}

impl fmt::Display for ControlParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::TooShort => "control frame shorter than 9-byte header (TOO_SHORT)",
            Self::BadMagic => "control frame bad magic (not RSHC)",
            Self::BadLength => "control frame length mismatch (BAD_LENGTH)",
            Self::BadCrc => "control frame CRC16 mismatch (BAD_CRC)",
        };
        f.write_str(s)
    }
}

impl core::error::Error for ControlParseError {}

/// Error returned when a HELLO reply (§2.2) cannot be parsed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelloError {
    /// Length is not exactly 92 bytes.
    BadLength,
    /// Magic is not `"RSHR"`.
    BadMagic,
    /// `protoMajor`/`protoMinor` are not 2/2 (2/1 accepted for an old
    /// board — see `HelloReply::is_old_protocol`).
    BadVersion,
    /// Reserved byte 17 is nonzero.
    RsvNonzero,
    /// Trailing CRC16 does not match.
    BadCrc,
}

impl fmt::Display for HelloError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::BadLength => "HELLO reply length is not 92 bytes",
            Self::BadMagic => "HELLO reply magic is not RSHR",
            Self::BadVersion => "HELLO reply protocol version is not 2.2 (or 2.1 for old firmware)",
            Self::RsvNonzero => "HELLO reply reserved byte nonzero",
            Self::BadCrc => "HELLO reply CRC16 mismatch",
        };
        f.write_str(s)
    }
}

impl core::error::Error for HelloError {}

/// Error returned when a ping frame (§9) cannot be parsed or validated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PingError {
    /// Frame is shorter than 8 bytes.
    TooShort,
    /// Magic is not `"RSHN"`.
    BadMagic,
    /// Version byte is not `0x22`, or `type` is not 0/1.
    BadVersion,
    /// Length is not exactly 34 bytes.
    BadLength,
    /// Trailing CRC16 does not match.
    BadCrc,
    /// Reply `seq` or `t0` does not match the request it answers.
    Mismatch,
    /// `t2 < t1`, `t3 < t0`, or the computed delay is negative.
    InvalidTimestamps,
}

impl fmt::Display for PingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::TooShort => "ping frame too short",
            Self::BadMagic => "ping frame bad magic (not RSHN)",
            Self::BadVersion => "ping frame version/type invalid",
            Self::BadLength => "ping frame length is not 34 bytes",
            Self::BadCrc => "ping frame CRC16 mismatch",
            Self::Mismatch => "ping reply seq/t0 does not match the request",
            Self::InvalidTimestamps => "ping timestamps invalid (t2 < t1 or negative delay)",
        };
        f.write_str(s)
    }
}

impl core::error::Error for PingError {}

/// Error returned when a config blob (§3.6) or a `SET_CFG` chunk (§3.5)
/// cannot be parsed or staged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// Blob is shorter than the 12-byte minimum (8 B header + 4 B trailer).
    TooShort,
    /// Blob magic is not `"RSHG"`.
    BadMagic,
    /// Schema byte is not 2.
    BadSchema,
    /// Declared `moduleCount` does not match the blob length.
    BadLength,
    /// CRC32 trailer does not match header + records.
    BadCrc,
    /// Blob total exceeds 2048 bytes, or a chunk exceeds 1024 bytes.
    TooBig,
    /// A chunk's `offset + chunkLen` exceeds `total`.
    ChunkOutOfRange,
    /// A chunk's CRC16 (over its data only) does not match.
    ChunkBadCrc,
    /// The first chunk of a staging session does not start at offset 0.
    FirstChunkNotZero,
    /// A chunk belongs to a different staging session (`total` changed).
    TotalMismatch,
    /// A record type, slot, or gate-mode value is unknown / out of range.
    BadRecord,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::TooShort => "config blob too short",
            Self::BadMagic => "config blob magic is not RSHG",
            Self::BadSchema => "config blob schema is not 2",
            Self::BadLength => "config blob length does not match module count",
            Self::BadCrc => "config blob CRC32 mismatch",
            Self::TooBig => "config blob or chunk exceeds size limit",
            Self::ChunkOutOfRange => "SET_CFG chunk offset + length exceeds total",
            Self::ChunkBadCrc => "SET_CFG chunk CRC16 mismatch",
            Self::FirstChunkNotZero => "first SET_CFG chunk must start at offset 0",
            Self::TotalMismatch => "SET_CFG chunk total does not match staging session",
            Self::BadRecord => "config record type/slot/gate-mode invalid",
        };
        f.write_str(s)
    }
}

impl core::error::Error for ConfigError {}

/// Error returned when a serial envelope frame (§10.1) cannot be parsed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SerialError {
    /// Fewer than 7 bytes — the header cannot be read.
    TooShort,
    /// Magic is not `"RSHS"`.
    BadMagic,
    /// Header readable but `kind` unknown, `len > 1053`, or the total
    /// length does not match `7 + len + 2`.
    BadLength,
    /// Trailing CRC16 does not match.
    BadCrc,
}

impl fmt::Display for SerialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::TooShort => "serial frame shorter than 7-byte header",
            Self::BadMagic => "serial frame bad magic (not RSHS)",
            Self::BadLength => "serial frame length/kind mismatch",
            Self::BadCrc => "serial frame CRC16 mismatch",
        };
        f.write_str(s)
    }
}

impl core::error::Error for SerialError {}

/// Error returned when a `PIN_PROBE` stream frame (§3.8) cannot be parsed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeError {
    /// Fewer than 13 bytes (header + at least one sample + CRC).
    TooShort,
    /// Magic is not `"RSHP"`.
    BadMagic,
    /// Version byte is not `0x22`, or `mode` is not 0/1.
    BadVersion,
    /// Declared `sampleCount` does not match the frame length.
    BadLength,
    /// Trailing CRC16 does not match.
    BadCrc,
}

impl fmt::Display for ProbeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::TooShort => "probe frame too short",
            Self::BadMagic => "probe frame bad magic (not RSHP)",
            Self::BadVersion => "probe frame version/mode invalid",
            Self::BadLength => "probe frame sample count does not match length",
            Self::BadCrc => "probe frame CRC16 mismatch",
        };
        f.write_str(s)
    }
}

impl core::error::Error for ProbeError {}
