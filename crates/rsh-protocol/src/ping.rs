//! NTP-style ping (§9): `"RSHN"` 34 B fixed, E2E latency measurement.
//!
//! Layout: `magic[4] | ver 0x21 | type u8 (0 request, 1 reply) | seq u16 |
//! t0 u64 | t1 u64 | t2 u64 | CRC16`. All timestamps are µs; `t0` is the
//! PC monotonic send time, `t1`/`t2` are board monotonic receive/send
//! times. `t3` (PC receive time) is never on the wire — the PC measures
//! it when the reply arrives.
//!
//! Formulas (§9.2): `delay = (t3 − t0) − (t2 − t1)`,
//! `offset = ((t1 − t0) + (t2 − t3)) / 2`, `board processing = t2 − t1`.
//! A reply is accepted only when magic/version/type/length/CRC are valid,
//! `seq` and `t0` match the request, and `t2 ≥ t1`, `t3 ≥ t0`,
//! `delay ≥ 0`. Ping timestamps are untrusted measurement only — they
//! must never drive an auth / replay / expiry decision (SEC-C-25).

use crate::crc::{crc16, verify_crc16_trailer};
use crate::error::{PingError, RejectCounters};
use crate::stream::VERSION;

/// Ping frame fixed length in bytes.
pub const PING_LEN: usize = 34;
/// Ping reply timeout in milliseconds (no retry — next ping 1 s later).
pub const PING_TIMEOUT_MS: u64 = 500;
/// Minimum gap between pings in milliseconds (sender 1–4 Hz, §9.5).
pub const PING_MIN_INTERVAL_MS: u64 = 250;

/// Ping frame type (§9.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PingType {
    /// `0` — request (PC → board).
    Request = 0,
    /// `1` — reply (board → PC).
    Reply = 1,
}

impl PingType {
    /// Decode a type byte; `None` for any value other than 0/1.
    #[must_use]
    pub fn from_u8(b: u8) -> Option<Self> {
        match b {
            0 => Some(Self::Request),
            1 => Some(Self::Reply),
            _ => None,
        }
    }
}

/// A ping frame (§9.1). Request and reply share the layout; a request
/// carries `t1 = t2 = 0` and the reply echoes `t0`/`seq`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PingFrame {
    /// Request or reply.
    pub frame_type: PingType,
    /// Sequence chosen by the PC, +1 per ping (wrap allowed).
    pub seq: u16,
    /// PC send time, PC monotonic µs.
    pub t0: u64,
    /// Board receive time, board monotonic µs (0 in a request).
    pub t1: u64,
    /// Board send time, board monotonic µs (0 in a request).
    pub t2: u64,
}

impl PingFrame {
    /// Build a request frame (`t1 = t2 = 0`).
    #[must_use]
    pub fn request(seq: u16, t0: u64) -> Self {
        Self {
            frame_type: PingType::Request,
            seq,
            t0,
            t1: 0,
            t2: 0,
        }
    }

    /// Build a reply frame echoing the request's `seq`/`t0`.
    #[must_use]
    pub fn reply(seq: u16, t0: u64, t1: u64, t2: u64) -> Self {
        Self {
            frame_type: PingType::Reply,
            seq,
            t0,
            t1,
            t2,
        }
    }

    /// Encode to the 34-byte wire form.
    #[must_use]
    pub fn encode(&self) -> [u8; PING_LEN] {
        let mut out = [0u8; PING_LEN];
        out[0..4].copy_from_slice(b"RSHN");
        out[4] = VERSION;
        out[5] = self.frame_type as u8;
        out[6..8].copy_from_slice(&self.seq.to_le_bytes());
        out[8..16].copy_from_slice(&self.t0.to_le_bytes());
        out[16..24].copy_from_slice(&self.t1.to_le_bytes());
        out[24..32].copy_from_slice(&self.t2.to_le_bytes());
        let crc = crc16(&out[..32]);
        out[32..34].copy_from_slice(&crc.to_le_bytes());
        out
    }

    /// Parse a 34-byte ping frame.
    ///
    /// # Errors
    ///
    /// Returns [`PingError`] for a short frame, bad magic, bad
    /// version/type, wrong length, or CRC mismatch.
    pub fn parse(data: &[u8]) -> Result<Self, PingError> {
        if data.len() < 8 {
            return Err(PingError::TooShort);
        }
        if &data[0..4] != b"RSHN" {
            return Err(PingError::BadMagic);
        }
        if data[4] != VERSION {
            return Err(PingError::BadVersion);
        }
        let frame_type = PingType::from_u8(data[5]).ok_or(PingError::BadVersion)?;
        if data.len() != PING_LEN {
            return Err(PingError::BadLength);
        }
        if !verify_crc16_trailer(data) {
            return Err(PingError::BadCrc);
        }
        Ok(Self {
            frame_type,
            seq: u16::from_le_bytes([data[6], data[7]]),
            t0: u64::from_le_bytes([
                data[8], data[9], data[10], data[11], data[12], data[13], data[14], data[15],
            ]),
            t1: u64::from_le_bytes([
                data[16], data[17], data[18], data[19], data[20], data[21], data[22], data[23],
            ]),
            t2: u64::from_le_bytes([
                data[24], data[25], data[26], data[27], data[28], data[29], data[30], data[31],
            ]),
        })
    }

    /// Parse, bumping the matching counter in `counters` on rejection.
    ///
    /// # Errors
    ///
    /// Same as [`PingFrame::parse`].
    pub fn parse_counted(data: &[u8], counters: &mut RejectCounters) -> Result<Self, PingError> {
        match Self::parse(data) {
            Ok(frame) => Ok(frame),
            Err(e) => {
                match e {
                    PingError::TooShort => counters.too_short += 1,
                    PingError::BadMagic => counters.bad_magic += 1,
                    PingError::BadVersion => counters.bad_version += 1,
                    PingError::BadLength => counters.bad_length += 1,
                    PingError::BadCrc => counters.bad_crc += 1,
                    PingError::Mismatch | PingError::InvalidTimestamps => {
                        counters.bad_frame += 1;
                    }
                }
                Err(e)
            }
        }
    }
}

/// A validated ping measurement (§9.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PingMeasurement {
    /// Round-trip minus board processing, in µs — the E2E network delay.
    pub delay_us: u64,
    /// Board clock − PC clock, in µs (display only, SEC-C-25).
    /// Signed: positive means the board clock is ahead.
    pub offset_us: i64,
    /// Board processing time (`t2 − t1`), in µs.
    pub board_processing_us: u64,
}

/// Validate a reply against its request and compute the measurement.
///
/// `t3` is the PC receive time in PC monotonic µs (measured locally,
/// never on the wire).
///
/// # Errors
///
/// Returns [`PingError::Mismatch`] when the reply is not a reply or its
/// `seq`/`t0` differ from the request, and
/// [`PingError::InvalidTimestamps`] when `t2 < t1`, `t3 < t0`, or the
/// computed delay would be negative. Invalid replies are counted, never
/// fed into a metric.
pub fn measure(
    request: &PingFrame,
    reply: &PingFrame,
    t3: u64,
) -> Result<PingMeasurement, PingError> {
    if reply.frame_type != PingType::Reply || reply.seq != request.seq || reply.t0 != request.t0 {
        return Err(PingError::Mismatch);
    }
    if reply.t2 < reply.t1 || t3 < request.t0 {
        return Err(PingError::InvalidTimestamps);
    }
    let network = i128::from(t3 - request.t0);
    let processing = i128::from(reply.t2 - reply.t1);
    let delay = network - processing;
    if delay < 0 {
        return Err(PingError::InvalidTimestamps);
    }
    // offset = ((t1 − t0) + (t2 − t3)) / 2, in signed µs.
    let offset = i128::midpoint(
        i128::from(reply.t1) - i128::from(request.t0),
        i128::from(reply.t2) - i128::from(t3),
    );
    Ok(PingMeasurement {
        delay_us: u64::try_from(delay).unwrap_or(0),
        offset_us: i64::try_from(offset).unwrap_or(if offset < 0 { i64::MIN } else { i64::MAX }),
        board_processing_us: reply.t2 - reply.t1,
    })
}

#[cfg(test)]
mod tests {
    use super::{PingFrame, measure};

    #[test]
    fn delay_and_offset_formulas() {
        // t0=1000, t1=1100, t2=1150, t3=1300 (µs, same epoch for the test).
        let req = PingFrame::request(7, 1000);
        let rep = PingFrame::reply(7, 1000, 1100, 1150);
        let m = measure(&req, &rep, 1300).expect("valid reply");
        assert_eq!(m.delay_us, 250); // (1300-1000) - (1150-1100)
        assert_eq!(m.board_processing_us, 50);
        assert_eq!(m.offset_us, -25); // ((1100-1000) + (1150-1300)) / 2
    }

    #[test]
    fn reply_mismatch_rejected() {
        let req = PingFrame::request(7, 1000);
        let wrong_seq = PingFrame::reply(8, 1000, 1100, 1150);
        assert!(measure(&req, &wrong_seq, 1300).is_err());
        let wrong_t0 = PingFrame::reply(7, 999, 1100, 1150);
        assert!(measure(&req, &wrong_t0, 1300).is_err());
    }

    #[test]
    fn negative_delay_rejected() {
        // Board processing (500) exceeds the round trip (300) — impossible.
        let req = PingFrame::request(7, 1000);
        let rep = PingFrame::reply(7, 1000, 1100, 1600);
        assert!(measure(&req, &rep, 1300).is_err());
    }

    #[test]
    fn t2_before_t1_rejected() {
        let req = PingFrame::request(7, 1000);
        let rep = PingFrame::reply(7, 1000, 1200, 1100);
        assert!(measure(&req, &rep, 1300).is_err());
    }
}
