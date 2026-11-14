//! Board → PC stream (§1.1) and status (§1.2) packets.
//!
//! Stream layout (`"RSH2"`, variable length):
//! `magic[4] | ver 0x21 | flags | seq u16 | timestamp u32 | axisMask |
//! nEnc | buttons u32 | reserved u16=0 | axes u16 × popcount(mask) (mV) |
//! encoders i16 × nEnc (cumulative) | CRC16`.
//!
//! Status layout (magic `0xA6`): 34 B base, 38 B extended (`btnsEx`).
//! `seqGap`/`txFail` sit immediately before the trailing CRC16 (§1.2).

use crate::crc::{crc16, verify_crc16_trailer};
use crate::error::{RejectCounters, StatusError, StreamError};

/// Wire version byte for protocol v2.1.
pub const VERSION: u8 = 0x21;
/// Maximum axes (D6).
pub const MAX_AXES: usize = 8;
/// Maximum encoders (D6).
pub const MAX_ENCODERS: usize = 4;
/// Maximum buttons in a config (D6 ceiling; wire carries 0–63).
pub const MAX_BUTTONS: usize = 128;
/// Stream header size in bytes.
pub const STREAM_HEADER_LEN: usize = 20;

/// Stream flags byte (§1.3, D4). Bits 4–7 are reserved and must be 0.
///
/// The four fields map 1:1 to the four defined wire bits; a bitflags
/// integer would hide that mapping, so the struct form is deliberate.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StreamFlags {
    /// Bit 0 — board config changed; PC should `GET_CFG`.
    pub config_changed: bool,
    /// Bit 1 — supply voltage low.
    pub low_supply: bool,
    /// Bit 2 — one or more ticks were skipped before this packet.
    pub tick_skipped: bool,
    /// Bit 3 — some axis is invalid (D12 criteria).
    pub axis_invalid: bool,
}

impl StreamFlags {
    /// Decode the flags byte. Returns `None` when a reserved bit (4–7)
    /// is set — the packet must be dropped and counted (`rsvNonzero`).
    #[must_use]
    pub fn from_byte(b: u8) -> Option<Self> {
        if b & 0xF0 != 0 {
            return None;
        }
        Some(Self {
            config_changed: b & 0x01 != 0,
            low_supply: b & 0x02 != 0,
            tick_skipped: b & 0x04 != 0,
            axis_invalid: b & 0x08 != 0,
        })
    }

    /// Encode back to the wire byte.
    #[must_use]
    pub fn to_byte(self) -> u8 {
        (u8::from(self.config_changed))
            | (u8::from(self.low_supply) << 1)
            | (u8::from(self.tick_skipped) << 2)
            | (u8::from(self.axis_invalid) << 3)
    }
}

/// A decoded v2.1 stream packet (§1.1).
///
/// Axes are millivolts (D6 — no per-axis unit flag). Axis values are stored
/// per slot index; a slot absent from the packet is `None`. Encoders are
/// cumulative i16 counts (wrap allowed — the consumer takes
/// `new.wrapping_sub(old)` deltas).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamFrame {
    /// Stream sequence, +1 per packet, wraps `0xFFFF → 0`.
    pub seq: u16,
    /// Timestamp in µs, taken after the ADC read (F4/D13).
    pub timestamp_us: u32,
    /// Flags byte (§1.3).
    pub flags: StreamFlags,
    /// Axis values in mV by slot 0–7 (`None` = slot not present).
    pub axes_mv: [Option<u16>; MAX_AXES],
    /// Cumulative encoder counts, `n_enc` entries used.
    pub encoders: [i16; MAX_ENCODERS],
    /// Number of encoders present (0–4).
    pub n_enc: usize,
    /// Buttons 0–31, bit *n* = button *n*.
    pub buttons: u32,
}

impl StreamFrame {
    /// Build a frame from axis values given as `(slot, mV)` pairs.
    ///
    /// # Panics
    ///
    /// Panics if a slot is `>= 8` or more than 4 encoders are given.
    /// Callers that take untrusted input must check bounds first; the
    /// parser never calls this with out-of-range data.
    #[must_use]
    pub fn new(
        seq: u16,
        timestamp_us: u32,
        flags: StreamFlags,
        axes: &[(usize, u16)],
        encoders: &[i16],
        buttons: u32,
    ) -> Self {
        assert!(axes.len() <= MAX_AXES, "at most 8 axes");
        assert!(encoders.len() <= MAX_ENCODERS, "at most 4 encoders");
        let mut axes_mv = [None; MAX_AXES];
        for &(slot, mv) in axes {
            assert!(slot < MAX_AXES, "axis slot out of range");
            axes_mv[slot] = Some(mv);
        }
        let mut enc = [0i16; MAX_ENCODERS];
        enc[..encoders.len()].copy_from_slice(encoders);
        Self {
            seq,
            timestamp_us,
            flags,
            axes_mv,
            encoders: enc,
            n_enc: encoders.len(),
            buttons,
        }
    }

    /// Bitmask of the axes present in this frame.
    #[must_use]
    pub fn axis_mask(&self) -> u8 {
        let mut mask = 0u8;
        for (i, a) in self.axes_mv.iter().enumerate() {
            if a.is_some() {
                mask |= 1u8 << i;
            }
        }
        mask
    }

    /// Encode this frame to wire bytes (header + axes + encoders + CRC16).
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let n_axes = self.axes_mv.iter().filter(|a| a.is_some()).count();
        let mut out = Vec::with_capacity(20 + 2 * n_axes + 2 * self.n_enc + 2);
        out.extend_from_slice(b"RSH2");
        out.push(VERSION);
        out.push(self.flags.to_byte());
        out.extend_from_slice(&self.seq.to_le_bytes());
        out.extend_from_slice(&self.timestamp_us.to_le_bytes());
        out.push(self.axis_mask());
        out.push(u8::try_from(self.n_enc).unwrap_or(u8::MAX));
        out.extend_from_slice(&self.buttons.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        for a in self.axes_mv.iter().flatten() {
            out.extend_from_slice(&a.to_le_bytes());
        }
        for e in &self.encoders[..self.n_enc] {
            out.extend_from_slice(&e.to_le_bytes());
        }
        let crc = crc16(&out);
        out.extend_from_slice(&crc.to_le_bytes());
        out
    }

    /// Parse a datagram into a [`StreamFrame`].
    ///
    /// Reject order (§1.4): legacy → magic → header length → version →
    /// flags-reserved → `nEnc`/length (`BadFrame`) → reserved bytes →
    /// CRC. Each failure maps to a distinct [`StreamError`].
    ///
    /// # Errors
    ///
    /// Returns the matching [`StreamError`] for any malformed datagram.
    pub fn parse(data: &[u8]) -> Result<Self, StreamError> {
        if is_legacy(data) {
            return Err(StreamError::Legacy);
        }
        if data.len() < 4 {
            return Err(StreamError::TooShort);
        }
        if &data[0..4] != b"RSH2" {
            return Err(StreamError::BadMagic);
        }
        if data.len() < STREAM_HEADER_LEN {
            return Err(StreamError::TooShort);
        }
        if data[4] != VERSION {
            return Err(StreamError::BadVersion);
        }
        let flags = StreamFlags::from_byte(data[5]).ok_or(StreamError::RsvNonzero)?;
        let seq = u16::from_le_bytes([data[6], data[7]]);
        let timestamp_us = u32::from_le_bytes([data[8], data[9], data[10], data[11]]);
        let axis_mask = data[12];
        let n_enc = data[13] as usize;
        if n_enc > MAX_ENCODERS {
            return Err(StreamError::BadFrame);
        }
        let n_axes = axis_mask.count_ones() as usize;
        let expected = STREAM_HEADER_LEN + 2 * n_axes + 2 * n_enc + 2;
        if data.len() != expected {
            return Err(StreamError::BadFrame);
        }
        let buttons = u32::from_le_bytes([data[14], data[15], data[16], data[17]]);
        if data[18] != 0 || data[19] != 0 {
            return Err(StreamError::RsvNonzero);
        }
        if !verify_crc16_trailer(data) {
            return Err(StreamError::BadCrc);
        }
        let mut axes_mv = [None; MAX_AXES];
        let mut off = STREAM_HEADER_LEN;
        for (slot, axis) in axes_mv.iter_mut().enumerate() {
            if axis_mask & (1u8 << slot) != 0 {
                *axis = Some(u16::from_le_bytes([data[off], data[off + 1]]));
                off += 2;
            }
        }
        let mut encoders = [0i16; MAX_ENCODERS];
        for e in encoders.iter_mut().take(n_enc) {
            *e = i16::from_le_bytes([data[off], data[off + 1]]);
            off += 2;
        }
        Ok(Self {
            seq,
            timestamp_us,
            flags,
            axes_mv,
            encoders,
            n_enc,
            buttons,
        })
    }

    /// Parse, bumping the matching counter in `counters` on rejection.
    ///
    /// # Errors
    ///
    /// Same as [`StreamFrame::parse`].
    pub fn parse_counted(data: &[u8], counters: &mut RejectCounters) -> Result<Self, StreamError> {
        match Self::parse(data) {
            Ok(frame) => Ok(frame),
            Err(e) => {
                match e {
                    StreamError::TooShort => counters.too_short += 1,
                    StreamError::BadMagic => counters.bad_magic += 1,
                    StreamError::Legacy => counters.legacy += 1,
                    StreamError::BadVersion => counters.bad_version += 1,
                    StreamError::BadFrame => counters.bad_frame += 1,
                    StreamError::RsvNonzero => counters.rsv_nonzero += 1,
                    StreamError::BadCrc => counters.bad_crc += 1,
                }
                Err(e)
            }
        }
    }
}

/// Detect a legacy v1 packet: first byte `0xA5` and second byte `0x01`
/// (Appendix A). Such packets are recognised and routed to the legacy path —
/// never parsed as v2.1.
#[must_use]
pub fn is_legacy(data: &[u8]) -> bool {
    data.len() >= 2 && data[0] == 0xA5 && data[1] == 0x01
}

/// Classify a UDP datagram arriving on port 4211 by its first byte(s),
/// without parsing it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DatagramKind {
    /// `"RSH2"` stream packet.
    Stream,
    /// `0xA6` status packet.
    Status,
    /// Legacy v1 packet (`0xA5 0x01`).
    Legacy,
    /// Anything else (bad magic).
    Unknown,
}

/// Classify a datagram by magic / legacy marker.
#[must_use]
pub fn classify_datagram(data: &[u8]) -> DatagramKind {
    if is_legacy(data) {
        DatagramKind::Legacy
    } else if data.len() >= 4 && &data[0..4] == b"RSH2" {
        DatagramKind::Stream
    } else if !data.is_empty() && data[0] == 0xA6 {
        DatagramKind::Status
    } else {
        DatagramKind::Unknown
    }
}

/// A decoded v2.1 status packet (§1.2): 34 B base, 38 B extended.
///
/// Extended packets carry `btns_ex` (buttons 32–63) and are recognised by
/// datagram length only. `seq_gap`/`tx_fail` sit immediately before the
/// trailing CRC16 in both layouts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatusPacket {
    /// Status sequence (separate counter from the data sequence).
    pub seq: u16,
    /// Milliseconds since boot.
    pub uptime_ms: u32,
    /// RSSI in dBm (PC link).
    pub rssi: i8,
    /// Battery percent; `255` = no battery module.
    pub battery_pct: u8,
    /// Supply in mV; `0` = not measured.
    pub supply_mv: u16,
    /// Free heap bytes.
    pub heap: u32,
    /// Overruns since boot.
    pub overruns: u16,
    /// Max cycle µs (whole loop iteration, D15).
    pub max_cycle_us: u16,
    /// Governor level.
    pub governor: u8,
    /// Reset reason: 1 power-on, 2 brown-out, 3 watchdog, 4 software.
    pub reset_reason: u8,
    /// Config CRC32.
    pub cfg_crc: u32,
    /// Config version.
    pub cfg_ver: u16,
    /// Stream sequence numbers skipped since boot (D5).
    pub seq_gap: u16,
    /// Failed UDP transmissions since boot (D5).
    pub tx_fail: u16,
    /// Buttons 32–63 (`Some` only for 38 B extended packets).
    pub btns_ex: Option<u32>,
}

impl StatusPacket {
    /// Encode to wire bytes (34 B base, 38 B when `btns_ex` is set).
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(if self.btns_ex.is_some() { 38 } else { 34 });
        out.push(0xA6);
        out.push(VERSION);
        out.extend_from_slice(&self.seq.to_le_bytes());
        out.extend_from_slice(&self.uptime_ms.to_le_bytes());
        out.push(self.rssi.cast_unsigned());
        out.push(self.battery_pct);
        out.extend_from_slice(&self.supply_mv.to_le_bytes());
        out.extend_from_slice(&self.heap.to_le_bytes());
        out.extend_from_slice(&self.overruns.to_le_bytes());
        out.extend_from_slice(&self.max_cycle_us.to_le_bytes());
        out.push(self.governor);
        out.push(self.reset_reason);
        out.extend_from_slice(&self.cfg_crc.to_le_bytes());
        out.extend_from_slice(&self.cfg_ver.to_le_bytes());
        if let Some(btns_ex) = self.btns_ex {
            out.extend_from_slice(&btns_ex.to_le_bytes());
        }
        out.extend_from_slice(&self.seq_gap.to_le_bytes());
        out.extend_from_slice(&self.tx_fail.to_le_bytes());
        let crc = crc16(&out);
        out.extend_from_slice(&crc.to_le_bytes());
        out
    }

    /// Parse a status datagram (34 B base or 38 B extended).
    ///
    /// # Errors
    ///
    /// Returns [`StatusError`] for a wrong length, magic, version, or CRC.
    pub fn parse(data: &[u8]) -> Result<Self, StatusError> {
        if data.len() != 34 && data.len() != 38 {
            return Err(StatusError::BadLength);
        }
        if data[0] != 0xA6 {
            return Err(StatusError::BadMagic);
        }
        if data[1] != VERSION {
            return Err(StatusError::BadVersion);
        }
        if !verify_crc16_trailer(data) {
            return Err(StatusError::BadCrc);
        }
        let extended = data.len() == 38;
        let (btns_ex, gap_off) = if extended {
            (
                Some(u32::from_le_bytes([data[28], data[29], data[30], data[31]])),
                32,
            )
        } else {
            (None, 28)
        };
        Ok(Self {
            seq: u16::from_le_bytes([data[2], data[3]]),
            uptime_ms: u32::from_le_bytes([data[4], data[5], data[6], data[7]]),
            rssi: data[8].cast_signed(),
            battery_pct: data[9],
            supply_mv: u16::from_le_bytes([data[10], data[11]]),
            heap: u32::from_le_bytes([data[12], data[13], data[14], data[15]]),
            overruns: u16::from_le_bytes([data[16], data[17]]),
            max_cycle_us: u16::from_le_bytes([data[18], data[19]]),
            governor: data[20],
            reset_reason: data[21],
            cfg_crc: u32::from_le_bytes([data[22], data[23], data[24], data[25]]),
            cfg_ver: u16::from_le_bytes([data[26], data[27]]),
            seq_gap: u16::from_le_bytes([data[gap_off], data[gap_off + 1]]),
            tx_fail: u16::from_le_bytes([data[gap_off + 2], data[gap_off + 3]]),
            btns_ex,
        })
    }

    /// Parse, bumping the matching counter in `counters` on rejection.
    ///
    /// # Errors
    ///
    /// Same as [`StatusPacket::parse`].
    pub fn parse_counted(data: &[u8], counters: &mut RejectCounters) -> Result<Self, StatusError> {
        match Self::parse(data) {
            Ok(packet) => Ok(packet),
            Err(e) => {
                match e {
                    StatusError::BadLength => counters.bad_frame += 1,
                    StatusError::BadMagic => counters.bad_magic += 1,
                    StatusError::BadVersion => counters.bad_version += 1,
                    StatusError::BadCrc => counters.bad_crc += 1,
                }
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{StreamFlags, is_legacy};

    #[test]
    fn flags_round_trip() {
        let f = StreamFlags {
            config_changed: true,
            low_supply: false,
            tick_skipped: true,
            axis_invalid: false,
        };
        assert_eq!(StreamFlags::from_byte(f.to_byte()), Some(f));
    }

    #[test]
    fn flags_reserved_bits_rejected() {
        assert_eq!(StreamFlags::from_byte(0x10), None);
        assert_eq!(StreamFlags::from_byte(0xF0), None);
        assert_eq!(
            StreamFlags::from_byte(0x0F),
            Some(StreamFlags {
                config_changed: true,
                low_supply: true,
                tick_skipped: true,
                axis_invalid: true,
            })
        );
    }

    #[test]
    fn legacy_detection() {
        assert!(is_legacy(&[0xA5, 0x01, 0x34]));
        assert!(!is_legacy(&[0xA5, 0x02]));
        assert!(!is_legacy(&[0xA5]));
        assert!(!is_legacy(b"RSH2"));
    }
}
