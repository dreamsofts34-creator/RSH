//! Wired USB-serial transport framing (§10): the `"RSHS"` envelope.
//!
//! Envelope: `magic[4] | kind u8 | len u16 LE | payload | CRC16`.
//! Header 7 B, CRC 2 B, max payload 1053 B (largest control frame),
//! max frame 1062 B. The payload is exactly the bytes that would go
//! over UDP (§1/§2/§3/§9) — serial adds no new wire format.
//!
//! Parse errors follow the control §3.1 rule (B1): `TooShort` only when
//! the 7-byte header itself is incomplete; once the header is readable,
//! every length mismatch (truncation, `len > 1053`, unknown kind, extra
//! bytes) is `BadLength`; CRC mismatch is `BadCrc`.

use crate::crc::{crc16, verify_crc16_trailer};
use crate::error::{RejectCounters, SerialError};

/// Serial envelope header size in bytes.
pub const SERIAL_HEADER_LEN: usize = 7;
/// Maximum serial payload in bytes (largest control frame).
pub const MAX_SERIAL_PAYLOAD: usize = 1053;
/// Maximum serial frame size in bytes (`7 + 1053 + 2`).
pub const MAX_SERIAL_FRAME_LEN: usize = SERIAL_HEADER_LEN + MAX_SERIAL_PAYLOAD + 2;

/// Serial frame kind (§10.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum SerialKind {
    /// `1` — STREAM (board → PC).
    Stream = 1,
    /// `2` — STATUS (board → PC).
    Status = 2,
    /// `3` — HELLO (board → PC).
    Hello = 3,
    /// `4` — CONTROL (both directions).
    Control = 4,
    /// `5` — PING (both directions).
    Ping = 5,
    /// `6` — HELO (PC → board).
    Helo = 6,
    /// `7` — PROBE (both directions).
    Probe = 7,
}

impl SerialKind {
    /// Decode a kind byte; `None` for values outside 1–7.
    #[must_use]
    pub fn from_u8(b: u8) -> Option<Self> {
        Some(match b {
            1 => Self::Stream,
            2 => Self::Status,
            3 => Self::Hello,
            4 => Self::Control,
            5 => Self::Ping,
            6 => Self::Helo,
            7 => Self::Probe,
            _ => return None,
        })
    }
}

/// One serial envelope frame (§10.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SerialFrame {
    /// Frame kind (1–7).
    pub kind: SerialKind,
    /// Payload — exactly the UDP bytes for this kind.
    pub payload: Vec<u8>,
}

impl SerialFrame {
    /// Build a frame.
    #[must_use]
    pub fn new(kind: SerialKind, payload: Vec<u8>) -> Self {
        Self { kind, payload }
    }

    /// Encode to wire bytes (header + payload + CRC16).
    ///
    /// # Errors
    ///
    /// Returns [`SerialError::BadLength`] when the payload exceeds
    /// 1053 bytes.
    pub fn encode(&self) -> Result<Vec<u8>, SerialError> {
        if self.payload.len() > MAX_SERIAL_PAYLOAD {
            return Err(SerialError::BadLength);
        }
        let mut out = Vec::with_capacity(SERIAL_HEADER_LEN + self.payload.len() + 2);
        out.extend_from_slice(b"RSHS");
        out.push(self.kind as u8);
        out.extend_from_slice(
            &(u16::try_from(self.payload.len()).unwrap_or(u16::MAX)).to_le_bytes(),
        );
        out.extend_from_slice(&self.payload);
        let crc = crc16(&out);
        out.extend_from_slice(&crc.to_le_bytes());
        Ok(out)
    }

    /// Parse one complete frame from `data` (which must contain exactly
    /// one frame — use [`SerialDecoder`] for a byte stream).
    ///
    /// # Errors
    ///
    /// Returns [`SerialError`] for a short frame, bad magic, unknown
    /// kind, a length mismatch, or a CRC mismatch.
    pub fn parse(data: &[u8]) -> Result<Self, SerialError> {
        if data.len() < SERIAL_HEADER_LEN {
            return Err(SerialError::TooShort);
        }
        if &data[0..4] != b"RSHS" {
            return Err(SerialError::BadMagic);
        }
        let kind = SerialKind::from_u8(data[4]).ok_or(SerialError::BadLength)?;
        let len = u16::from_le_bytes([data[5], data[6]]) as usize;
        if len > MAX_SERIAL_PAYLOAD || data.len() != SERIAL_HEADER_LEN + len + 2 {
            return Err(SerialError::BadLength);
        }
        if !verify_crc16_trailer(data) {
            return Err(SerialError::BadCrc);
        }
        Ok(Self {
            kind,
            payload: data[SERIAL_HEADER_LEN..SERIAL_HEADER_LEN + len].to_vec(),
        })
    }
}

/// Incremental byte-at-a-time serial decoder (§10.2).
///
/// Feeds bytes as they arrive from the UART, resynchronises on garbage
/// (boot log / bootloader output), and delivers only complete,
/// CRC-valid frames. A bad frame is dropped silently and counted —
/// never delivered half-formed. The buffer is bounded: it never holds
/// more than one maximum frame plus one magic scan window.
#[derive(Clone, Debug, Default)]
pub struct SerialDecoder {
    buf: Vec<u8>,
    /// Dropped frames by reason (bad magic bytes are counted per byte
    /// skipped while scanning; bad CRC / bad length per frame).
    pub dropped_bad_magic: u64,
    /// Frames dropped for a CRC mismatch.
    pub dropped_bad_crc: u64,
    /// Frames dropped for a length/kind problem.
    pub dropped_bad_length: u64,
}

impl SerialDecoder {
    /// Create an empty decoder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Reset the decoder, discarding any partial frame (reconnect).
    pub fn reset(&mut self) {
        self.buf.clear();
    }

    /// Feed bytes; returns every complete, valid frame decoded from
    /// the stream so far. Invalid frames are dropped and counted.
    pub fn feed(&mut self, data: &[u8]) -> Vec<SerialFrame> {
        self.buf.extend_from_slice(data);
        let mut frames = Vec::new();
        loop {
            // Scan for the magic.
            let Some(start) = find_magic(&self.buf) else {
                // Keep at most the last 3 bytes — a magic split across feeds.
                let keep = self.buf.len().min(3);
                let dropped = self.buf.len() - keep;
                self.dropped_bad_magic += dropped as u64;
                self.buf.drain(..dropped);
                break;
            };
            if start > 0 {
                self.dropped_bad_magic += start as u64;
                self.buf.drain(..start);
            }
            if self.buf.len() < SERIAL_HEADER_LEN {
                break; // header incomplete — wait for more bytes
            }
            let Some(kind) = SerialKind::from_u8(self.buf[4]) else {
                self.dropped_bad_length += 1;
                self.buf.drain(..1);
                continue;
            };
            let len = u16::from_le_bytes([self.buf[5], self.buf[6]]) as usize;
            if len > MAX_SERIAL_PAYLOAD {
                self.dropped_bad_length += 1;
                self.buf.drain(..1);
                continue;
            }
            let total = SERIAL_HEADER_LEN + len + 2;
            if self.buf.len() < total {
                // Guard against an unbounded buffer: a declared frame can
                // never exceed MAX_SERIAL_FRAME_LEN.
                if self.buf.len() > MAX_SERIAL_FRAME_LEN {
                    self.dropped_bad_length += 1;
                    self.buf.drain(..1);
                    continue;
                }
                break; // frame incomplete — wait for more bytes
            }
            let frame_bytes = &self.buf[..total];
            if !verify_crc16_trailer(frame_bytes) {
                self.dropped_bad_crc += 1;
                self.buf.drain(..1);
                continue;
            }
            frames.push(SerialFrame {
                kind,
                payload: frame_bytes[SERIAL_HEADER_LEN..SERIAL_HEADER_LEN + len].to_vec(),
            });
            self.buf.drain(..total);
        }
        frames
    }

    /// Total dropped frames/bytes across all reasons.
    #[must_use]
    pub fn dropped_total(&self) -> u64 {
        self.dropped_bad_magic + self.dropped_bad_crc + self.dropped_bad_length
    }
}

/// Find the first occurrence of `"RSHS"` in `buf`.
fn find_magic(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"RSHS")
}

/// Parse a serial frame, bumping the matching counter in `counters`.
///
/// # Errors
///
/// Same as [`SerialFrame::parse`].
pub fn parse_serial_counted(
    data: &[u8],
    counters: &mut RejectCounters,
) -> Result<SerialFrame, SerialError> {
    match SerialFrame::parse(data) {
        Ok(frame) => Ok(frame),
        Err(e) => {
            match e {
                SerialError::TooShort => counters.too_short += 1,
                SerialError::BadMagic => counters.bad_magic += 1,
                SerialError::BadLength => counters.bad_length += 1,
                SerialError::BadCrc => counters.bad_crc += 1,
            }
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SerialDecoder, SerialFrame, SerialKind};

    #[test]
    fn decoder_chunked_and_garbage_resync() {
        let frame = SerialFrame::new(SerialKind::Helo, b"HELO\x02\x00".to_vec());
        let bytes = frame.encode().expect("serial frame encodes");
        let mut decoder = SerialDecoder::new();
        // Garbage (boot log) before the frame, fed byte-at-a-time.
        let mut out = Vec::new();
        for chunk in [b"boot log\r\n".as_slice(), &bytes[..5], &bytes[5..]] {
            out.extend(decoder.feed(chunk));
        }
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], frame);
        assert!(decoder.dropped_bad_magic > 0);
    }

    #[test]
    fn decoder_drops_bad_crc_frame() {
        let frame = SerialFrame::new(SerialKind::Stream, vec![1, 2, 3, 4, 5, 6, 7, 8]);
        let mut bytes = frame.encode().expect("serial frame encodes");
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        let mut decoder = SerialDecoder::new();
        let out = decoder.feed(&bytes);
        assert_eq!(out.len(), 0);
        assert_eq!(decoder.dropped_bad_crc, 1);
    }
}
