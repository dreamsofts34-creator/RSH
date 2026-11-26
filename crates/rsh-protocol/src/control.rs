//! Control frames (§3): plain `"RSHC"` frames, CRC only (protocol v2.2).
//!
//! Frame layout: `magic[4] | cmd u8 | reqId u16 | len u16 | payload |
//! CRC16[2]`; total `11 + len`, `len ≤ 1024`.
//!
//! There is no HMAC, no device key, no nonce, and no counter in v2.2
//! (D2): the duplicate cache keys on `reqId` alone, and `reqId` is
//! monotonic per session. Integrity is the trailing CRC16-CCITT-FALSE
//! over every byte before the CRC field — the same CRC as the stream /
//! HELLO / serial frames. A frame whose CRC fails is dropped as
//! `BadCrc`, never executed.
//!
//! Parse errors (§3.1): `TooShort` only when the frame is shorter than the
//! 9-byte header; every other length mismatch is `BadLength`.

use crate::crc::{crc16, verify_crc16_trailer};
use crate::error::{ControlParseError, RejectCounters};

/// Control frame header size in bytes (`magic[4] | cmd | reqId u16 |
/// len u16`).
pub const CONTROL_HEADER_LEN: usize = 9;
/// Trailing CRC16 size in bytes.
pub const CRC_LEN: usize = 2;
/// Maximum control payload in bytes.
pub const MAX_PAYLOAD_LEN: usize = 1024;
/// Maximum control frame size in bytes (`9 + 1024 + 2`).
pub const MAX_FRAME_LEN: usize = CONTROL_HEADER_LEN + MAX_PAYLOAD_LEN + CRC_LEN;
/// Maximum retry attempts per request: 3 sends total (§3.2).
pub const MAX_RETRIES: u32 = 3;
/// Retry interval in milliseconds (§3.2).
pub const RETRY_INTERVAL_MS: u64 = 250;
/// Duplicate-cache size: the board caches the last 32 `reqId`s per
/// session (§3.2, D2).
pub const DUPLICATE_CACHE_SIZE: usize = 32;

/// Control opcodes (§3.3). `0x0B` (`KEY_RESET`) is deleted in v2.2 (D3):
/// password-clear is physical-only (BOOT hold), there is no wire opcode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Opcode {
    /// `0x01` — `GET_CAPS`.
    GetCaps = 0x01,
    /// `0x02` — `GET_CFG`.
    GetCfg = 0x02,
    /// `0x03` — `SET_CFG`.
    SetCfg = 0x03,
    /// `0x04` — COMMIT.
    Commit = 0x04,
    /// `0x05` — CONFIRM.
    Confirm = 0x05,
    /// `0x06` — REVERT.
    Revert = 0x06,
    /// `0x07` — `PIN_PROBE`.
    PinProbe = 0x07,
    /// `0x08` — `FACTORY_RESET`.
    FactoryReset = 0x08,
    /// `0x09` — REBOOT.
    Reboot = 0x09,
    /// `0x0A` — `LED_TEST`.
    LedTest = 0x0A,
    /// `0x0C` — `SET_PASSWORD`.
    SetPassword = 0x0C,
}

impl Opcode {
    /// Decode an opcode byte; `None` for unknown commands (including
    /// the deleted `0x0B`).
    #[must_use]
    pub fn from_u8(b: u8) -> Option<Self> {
        Some(match b {
            0x01 => Self::GetCaps,
            0x02 => Self::GetCfg,
            0x03 => Self::SetCfg,
            0x04 => Self::Commit,
            0x05 => Self::Confirm,
            0x06 => Self::Revert,
            0x07 => Self::PinProbe,
            0x08 => Self::FactoryReset,
            0x09 => Self::Reboot,
            0x0A => Self::LedTest,
            0x0C => Self::SetPassword,
            _ => return None,
        })
    }

    /// `true` when the byte is a known opcode (`0x01`–`0x0C` except
    /// the deleted `0x0B`).
    #[must_use]
    pub fn is_known(b: u8) -> bool {
        Self::from_u8(b).is_some()
    }
}

/// Reply status codes (§3.9) — the first byte of every reply payload.
///
/// Code `1` is `BadPassword` (v2.2, D3): it reuses the freed `BadTag`
/// slot (firmware `RSH_CTRL_BAD_PASSWORD` = 1), so a status byte means
/// the same thing on both sides of the redesign. The remaining codes
/// keep their v2.1 numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ErrorCode {
    /// `0` — success.
    Ok = 0,
    /// `1` — `SET_PASSWORD` current password wrong (v2.2, D3).
    BadPassword = 1,
    /// `2` — duplicate `reqId` (retransmit answered from the cache,
    /// or a `reqId` seen out of order).
    Replay = 2,
    /// `3` — `SET_CFG` chunk or config blob CRC mismatch.
    BadCrc = 3,
    /// `4` — `len > 1024`, chunk `offset + len > total`, or total > 2048.
    TooBig = 4,
    /// `5` — pin validation failed.
    PinConflict = 5,
    /// `6` — unknown command / unsupported feature.
    Unsupported = 6,
    /// `7` — board busy.
    Busy = 7,
    /// `8` — `SET_PASSWORD` synchronous NVS write failed (F-07, firmware
    /// 4.0.6): the password was NOT changed; retry or keep the old one.
    NvsFail = 8,
}

impl ErrorCode {
    /// Decode a status byte; `None` for values > 8.
    #[must_use]
    pub fn from_u8(b: u8) -> Option<Self> {
        Some(match b {
            0 => Self::Ok,
            1 => Self::BadPassword,
            2 => Self::Replay,
            3 => Self::BadCrc,
            4 => Self::TooBig,
            5 => Self::PinConflict,
            6 => Self::Unsupported,
            7 => Self::Busy,
            8 => Self::NvsFail,
            _ => return None,
        })
    }
}

/// A parsed control frame (structure + payload, CRC already verified by
/// [`ControlFrame::verify`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlFrame {
    /// Command byte (request: known opcodes; reply: `req_cmd | 0x80`).
    pub cmd: u8,
    /// Request id chosen by the PC; identical across retries (D2).
    pub req_id: u16,
    /// Payload bytes (`len` ≤ 1024).
    pub payload: Vec<u8>,
}

impl ControlFrame {
    /// `true` when this frame is a reply (`cmd` high bit set).
    #[must_use]
    pub fn is_reply(&self) -> bool {
        self.cmd & 0x80 != 0
    }

    /// The request command this reply answers (`cmd & 0x7F`).
    #[must_use]
    pub fn request_cmd(&self) -> u8 {
        self.cmd & 0x7F
    }

    /// Reply status byte (first payload byte), if the frame is a reply
    /// with a non-empty payload.
    #[must_use]
    pub fn status(&self) -> Option<ErrorCode> {
        if self.is_reply() {
            self.payload.first().and_then(|b| ErrorCode::from_u8(*b))
        } else {
            None
        }
    }

    /// Encode this frame with a trailing CRC16.
    ///
    /// # Errors
    ///
    /// Returns [`ControlParseError::BadLength`] when the payload exceeds
    /// 1024 bytes.
    pub fn encode(&self) -> Result<Vec<u8>, ControlParseError> {
        build_frame(self.cmd, self.req_id, &self.payload)
    }

    /// Parse only the structure (magic + header + length), without
    /// verifying the CRC. Use [`ControlFrame::verify`] for a full check.
    ///
    /// # Errors
    ///
    /// Returns [`ControlParseError`] for a short frame, bad magic, or any
    /// length mismatch (`TooShort` only below 9 bytes).
    pub fn parse_structure(data: &[u8]) -> Result<Self, ControlParseError> {
        if data.len() < CONTROL_HEADER_LEN {
            return Err(ControlParseError::TooShort);
        }
        if &data[0..4] != b"RSHC" {
            return Err(ControlParseError::BadMagic);
        }
        let cmd = data[4];
        let req_id = u16::from_le_bytes([data[5], data[6]]);
        let len = u16::from_le_bytes([data[7], data[8]]) as usize;
        if len > MAX_PAYLOAD_LEN || data.len() != CONTROL_HEADER_LEN + len + CRC_LEN {
            return Err(ControlParseError::BadLength);
        }
        Ok(Self {
            cmd,
            req_id,
            payload: data[CONTROL_HEADER_LEN..CONTROL_HEADER_LEN + len].to_vec(),
        })
    }

    /// Parse and verify the trailing CRC16.
    ///
    /// # Errors
    ///
    /// Returns [`ControlParseError`] for structural problems or a CRC
    /// mismatch (`BadCrc`).
    pub fn verify(data: &[u8]) -> Result<Self, ControlParseError> {
        let frame = Self::parse_structure(data)?;
        if !verify_crc16_trailer(data) {
            return Err(ControlParseError::BadCrc);
        }
        Ok(frame)
    }

    /// Parse + verify, bumping the matching counter in `counters`.
    ///
    /// # Errors
    ///
    /// Same as [`ControlFrame::verify`].
    pub fn verify_counted(
        data: &[u8],
        counters: &mut RejectCounters,
    ) -> Result<Self, ControlParseError> {
        match Self::verify(data) {
            Ok(frame) => Ok(frame),
            Err(e) => {
                match e {
                    ControlParseError::TooShort => counters.too_short += 1,
                    ControlParseError::BadMagic => counters.bad_magic += 1,
                    ControlParseError::BadLength => counters.bad_length += 1,
                    ControlParseError::BadCrc => counters.bad_crc += 1,
                }
                Err(e)
            }
        }
    }
}

/// Build a complete control frame (header + payload + CRC16).
///
/// # Errors
///
/// Returns [`ControlParseError::BadLength`] when `payload.len() > 1024`.
pub fn build_frame(cmd: u8, req_id: u16, payload: &[u8]) -> Result<Vec<u8>, ControlParseError> {
    if payload.len() > MAX_PAYLOAD_LEN {
        return Err(ControlParseError::BadLength);
    }
    let mut out = Vec::with_capacity(CONTROL_HEADER_LEN + payload.len() + CRC_LEN);
    out.extend_from_slice(b"RSHC");
    out.push(cmd);
    out.extend_from_slice(&req_id.to_le_bytes());
    out.extend_from_slice(&(u16::try_from(payload.len()).unwrap_or(u16::MAX)).to_le_bytes());
    out.extend_from_slice(payload);
    let crc = crc16(&out);
    out.extend_from_slice(&crc.to_le_bytes());
    Ok(out)
}

/// Build a reply frame: `cmd = req_cmd | 0x80`, payload is
/// `status ‖ body`.
///
/// # Errors
///
/// Returns [`ControlParseError::BadLength`] when the reply payload
/// (`1 + body.len()`) exceeds 1024 bytes.
pub fn build_reply(
    req_cmd: u8,
    req_id: u16,
    status: ErrorCode,
    body: &[u8],
) -> Result<Vec<u8>, ControlParseError> {
    let mut payload = Vec::with_capacity(1 + body.len());
    payload.push(status as u8);
    payload.extend_from_slice(body);
    build_frame(req_cmd | 0x80, req_id, &payload)
}

/// Exact-retransmit cache: the last 32 `reqId`s (§3.2, D2).
/// A byte-identical retransmit is answered from the cached reply and never
/// re-executed.
#[derive(Clone, Debug, Default)]
pub struct DuplicateCache {
    entries: [Option<u16>; DUPLICATE_CACHE_SIZE],
    next: usize,
}

impl DuplicateCache {
    /// Create an empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `true` when this exact `req_id` was already seen.
    #[must_use]
    pub fn is_duplicate(&self, req_id: u16) -> bool {
        self.entries.contains(&Some(req_id))
    }

    /// Record a `req_id`. Returns `false` when it was already present
    /// (a duplicate retransmit).
    pub fn insert(&mut self, req_id: u16) -> bool {
        if self.is_duplicate(req_id) {
            return false;
        }
        self.entries[self.next] = Some(req_id);
        self.next = (self.next + 1) % DUPLICATE_CACHE_SIZE;
        true
    }

    /// Clear the cache (new session).
    pub fn clear(&mut self) {
        self.entries = [None; DUPLICATE_CACHE_SIZE];
        self.next = 0;
    }
}

/// Client-side retry state for one control request (§3.2).
///
/// Rule: up to 3 sends at 250 ms intervals; every retry keeps the same
/// `req_id` and re-sends the byte-identical frame (there is no counter
/// in v2.2 — a retry is an exact retransmit). After the third send the
/// request fails and the UI is told.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControlRetry {
    /// Request id, identical across retries.
    pub req_id: u16,
    /// Sends already made (1 after the first send).
    pub attempts: u32,
}

impl ControlRetry {
    /// Start a request.
    #[must_use]
    pub fn new(req_id: u16) -> Self {
        Self {
            req_id,
            attempts: 0,
        }
    }

    /// `true` when another send is allowed; each call consumes one
    /// attempt. The retry re-sends the same `req_id` (same bytes).
    pub fn next_send(&mut self) -> bool {
        if self.attempts >= MAX_RETRIES {
            return false;
        }
        self.attempts += 1;
        true
    }

    /// `true` when no attempts remain.
    #[must_use]
    pub fn exhausted(&self) -> bool {
        self.attempts >= MAX_RETRIES
    }
}

/// `GET_CFG` request payload (§3.3): `offset u16, wantLen u16 (≤ 1024)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GetCfgRequest {
    /// Offset into the stored config blob.
    pub offset: u16,
    /// Bytes requested (≤ 1024).
    pub want_len: u16,
}

impl GetCfgRequest {
    /// Encode the 4-byte request payload.
    #[must_use]
    pub fn encode(&self) -> [u8; 4] {
        let mut out = [0u8; 4];
        out[0..2].copy_from_slice(&self.offset.to_le_bytes());
        out[2..4].copy_from_slice(&self.want_len.to_le_bytes());
        out
    }

    /// Decode a 4-byte request payload.
    ///
    /// # Errors
    ///
    /// Returns [`ControlParseError::BadLength`] for a wrong length or
    /// `want_len > 1024`.
    pub fn decode(data: &[u8]) -> Result<Self, ControlParseError> {
        if data.len() != 4 {
            return Err(ControlParseError::BadLength);
        }
        let want_len = u16::from_le_bytes([data[2], data[3]]);
        if usize::from(want_len) > MAX_PAYLOAD_LEN {
            return Err(ControlParseError::BadLength);
        }
        Ok(Self {
            offset: u16::from_le_bytes([data[0], data[1]]),
            want_len,
        })
    }
}

/// `LED_TEST` pattern payload (§3.7), 8 bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LedTestPattern {
    /// Pattern id: 0 OFF, 1 SOLID, 2 BLINK, 3 CHASE, 4 RAINBOW.
    pub pattern_id: u8,
    /// Red 0–255.
    pub r: u8,
    /// Green 0–255.
    pub g: u8,
    /// Blue 0–255.
    pub b: u8,
    /// Blink/chase period in ms (100–5000; ignored for SOLID/RAINBOW).
    pub period_ms: u16,
    /// Repeats; `0` = run 5 s then stop. Total run is capped at 60 s
    /// board-side (SEC-H-10).
    pub repeats: u8,
}

impl LedTestPattern {
    /// Encode the 8-byte payload (reserved byte is always 0).
    #[must_use]
    pub fn encode(&self) -> [u8; 8] {
        let mut out = [0u8; 8];
        out[0] = self.pattern_id;
        out[1] = self.r;
        out[2] = self.g;
        out[3] = self.b;
        out[4..6].copy_from_slice(&self.period_ms.to_le_bytes());
        out[6] = self.repeats;
        out
    }

    /// Decode an 8-byte payload.
    ///
    /// # Errors
    ///
    /// Returns [`ControlParseError::BadLength`] for a wrong length, an
    /// unknown `pattern_id`, or a reserved byte that is nonzero.
    pub fn decode(data: &[u8]) -> Result<Self, ControlParseError> {
        if data.len() != 8 || data[7] != 0 {
            return Err(ControlParseError::BadLength);
        }
        if data[0] > 4 {
            return Err(ControlParseError::BadLength);
        }
        Ok(Self {
            pattern_id: data[0],
            r: data[1],
            g: data[2],
            b: data[3],
            period_ms: u16::from_le_bytes([data[4], data[5]]),
            repeats: data[6],
        })
    }
}

/// `PIN_PROBE` request payload (§3.8), 4 bytes:
/// `pin u8, durationMs u16 (100–10000), mode u8` (0 digital, 1 ADC mV).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PinProbeRequest {
    /// GPIO pin to probe.
    pub pin: u8,
    /// Probe duration in ms.
    pub duration_ms: u16,
    /// Mode: 0 = digital level, 1 = ADC mV.
    pub mode: u8,
}

impl PinProbeRequest {
    /// Encode the 4-byte request payload.
    #[must_use]
    pub fn encode(&self) -> [u8; 4] {
        let mut out = [0u8; 4];
        out[0] = self.pin;
        out[1..3].copy_from_slice(&self.duration_ms.to_le_bytes());
        out[3] = self.mode;
        out
    }

    /// Decode a 4-byte request payload.
    ///
    /// # Errors
    ///
    /// Returns [`ControlParseError::BadLength`] for a wrong length, a
    /// `duration_ms` outside 100–10000, or a `mode` other than 0/1.
    pub fn decode(data: &[u8]) -> Result<Self, ControlParseError> {
        if data.len() != 4 {
            return Err(ControlParseError::BadLength);
        }
        let duration_ms = u16::from_le_bytes([data[1], data[2]]);
        if !(100..=10_000).contains(&duration_ms) || data[3] > 1 {
            return Err(ControlParseError::BadLength);
        }
        Ok(Self {
            pin: data[0],
            duration_ms,
            mode: data[3],
        })
    }
}

/// Validate a `SET_PASSWORD` request payload (§5.2, v2.2 layout D3):
/// `curLen u8` + current password (`curLen` bytes, `0` = none set
/// yet) + `newLen u8 (8..63)` + new password bytes, printable ASCII
/// (`0x21–0x7E`, no space) in both parts.
///
/// Returns `(current, new)` on success — `current` is `None` when
/// `curLen` is `0` — and `None` on any violation (length, charset,
/// or trailing bytes; the board replies `UNSUPPORTED`).
#[must_use]
pub fn validate_set_password_payload(payload: &[u8]) -> Option<(Option<&[u8]>, &[u8])> {
    if payload.len() < 2 {
        return None;
    }
    let cur_len = payload[0] as usize;
    if cur_len > 63 || payload.len() < 1 + cur_len + 1 {
        return None;
    }
    let current = &payload[1..=cur_len];
    if current.iter().any(|b| !(0x21..=0x7E).contains(b)) {
        return None;
    }
    let rest = &payload[1 + cur_len..];
    let new_len = rest[0] as usize;
    if !(8..=63).contains(&new_len) || rest.len() != 1 + new_len {
        return None;
    }
    let new = &rest[1..];
    if new.iter().any(|b| !(0x21..=0x7E).contains(b)) {
        return None;
    }
    Some((if cur_len == 0 { None } else { Some(current) }, new))
}

/// Build a `SET_PASSWORD` request payload (v2.2 layout, D3 §3.6):
/// `curLen u8` + current password + `newLen u8` + new password.
///
/// `current` is `None` (or empty) when no password is set yet —
/// first setup on an open AP. When one is set, the current password
/// must be carried, or the board replies `BadPassword` (1).
///
/// Returns `None` when either password violates the §5.2 rules
/// (new: 8–63 printable ASCII; current: 0–63 printable ASCII).
#[must_use]
pub fn build_set_password_payload(current: Option<&[u8]>, password: &[u8]) -> Option<Vec<u8>> {
    let current = current.unwrap_or(&[]);
    if current.len() > 63 || current.iter().any(|b| !(0x21..=0x7E).contains(b)) {
        return None;
    }
    if !(8..=63).contains(&password.len()) {
        return None;
    }
    if password.iter().any(|b| !(0x21..=0x7E).contains(b)) {
        return None;
    }
    let mut out = Vec::with_capacity(1 + current.len() + 1 + password.len());
    out.push(u8::try_from(current.len()).unwrap_or(u8::MAX));
    out.extend_from_slice(current);
    out.push(u8::try_from(password.len()).unwrap_or(u8::MAX));
    out.extend_from_slice(password);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::{
        ControlFrame, ControlRetry, DuplicateCache, ErrorCode, Opcode,
        validate_set_password_payload,
    };
    use crate::error::ControlParseError;

    #[test]
    fn opcode_round_trip() {
        // Known opcodes: 0x01–0x0C except the deleted 0x0B KEY_RESET.
        for b in [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 12] {
            assert!(Opcode::from_u8(b).is_some());
            assert!(Opcode::is_known(b));
        }
        assert_eq!(Opcode::from_u8(0), None);
        assert_eq!(Opcode::from_u8(0x0B), None, "KEY_RESET is deleted in v2.2");
        assert_eq!(Opcode::from_u8(0x0D), None);
        assert_eq!(Opcode::from_u8(0x81), None);
    }

    #[test]
    fn error_code_round_trip() {
        // Code 1 is BadPassword (v2.2, D3 — reuses the freed BadTag
        // slot); code 8 is NvsFail (firmware 4.0.6, F-07). Every
        // code 0..=8 decodes, 9+ does not.
        for b in [0u8, 1, 2, 3, 4, 5, 6, 7, 8] {
            assert!(ErrorCode::from_u8(b).is_some());
        }
        assert_eq!(ErrorCode::from_u8(1), Some(ErrorCode::BadPassword));
        assert_eq!(ErrorCode::from_u8(8), Some(ErrorCode::NvsFail));
        assert_eq!(ErrorCode::from_u8(9), None);
    }

    #[test]
    fn duplicate_cache_keys_on_req_id_only() {
        let mut cache = DuplicateCache::new();
        assert!(cache.insert(1));
        assert!(!cache.insert(1));
        assert!(cache.is_duplicate(1));
        assert!(!cache.is_duplicate(2));
        assert!(cache.insert(2));
        cache.clear();
        assert!(!cache.is_duplicate(1));
        assert!(cache.insert(1));
    }

    #[test]
    fn retry_keeps_req_id_three_times() {
        let mut retry = ControlRetry::new(42);
        assert_eq!(retry.req_id, 42);
        assert!(retry.next_send());
        assert!(retry.next_send());
        assert!(retry.next_send());
        assert!(!retry.next_send());
        assert!(retry.exhausted());
    }

    #[test]
    fn crc_verify_rejects_flipped_bytes() {
        // CRC16 trailer: a flipped payload byte, a flipped CRC byte,
        // and a wrong command byte all fail verification.
        let frame = ControlFrame {
            cmd: 0x01,
            req_id: 1,
            payload: b"hello".to_vec(),
        };
        let bytes = frame.encode().expect("encode");
        assert_eq!(ControlFrame::verify(&bytes).expect("verifies"), frame);
        let mut bad = bytes.clone();
        bad[10] ^= 0xFF; // payload byte
        assert_eq!(
            ControlFrame::verify(&bad).unwrap_err(),
            ControlParseError::BadCrc
        );
        let mut bad_crc = bytes.clone();
        let last = bad_crc.len() - 1;
        bad_crc[last] ^= 0xFF;
        assert_eq!(
            ControlFrame::verify(&bad_crc).unwrap_err(),
            ControlParseError::BadCrc
        );
        // Structure-only parse still succeeds on a CRC-bad frame
        // (and shows the flipped payload — structure is not integrity).
        let parsed = ControlFrame::parse_structure(&bad).expect("structure parses");
        assert_ne!(parsed.payload, frame.payload);
    }

    #[test]
    fn frame_too_short_vs_bad_length() {
        // Under 9 B -> TooShort; header readable but length mismatch ->
        // BadLength.
        let bytes = super::build_frame(0x01, 1, b"").expect("encode");
        assert_eq!(
            ControlFrame::verify(&bytes[..8]).unwrap_err(),
            ControlParseError::TooShort
        );
        assert_eq!(
            ControlFrame::verify(&bytes[..10]).unwrap_err(),
            ControlParseError::BadLength
        );
    }

    #[test]
    fn set_password_validation() {
        assert!(validate_set_password_payload(&[]).is_none());
        // No current password yet: curLen 0.
        let mut good = vec![0u8, 8u8];
        good.extend_from_slice(b"Abcdef12");
        assert_eq!(
            validate_set_password_payload(&good),
            Some((None, b"Abcdef12".as_slice()))
        );
        // With a current password: curLen 8 + current + newLen + new.
        let mut with_current = vec![8u8];
        with_current.extend_from_slice(b"oldpass1");
        with_current.push(8u8);
        with_current.extend_from_slice(b"Abcdef12");
        assert_eq!(
            validate_set_password_payload(&with_current),
            Some((Some(b"oldpass1".as_slice()), b"Abcdef12".as_slice()))
        );
        // New password too short.
        let mut short = vec![0u8, 7u8];
        short.extend_from_slice(b"Abcdef1");
        assert!(validate_set_password_payload(&short).is_none());
        // Space in the new password.
        let mut space = vec![0u8, 8u8];
        space.extend_from_slice(b"Abc def1");
        assert!(validate_set_password_payload(&space).is_none());
        // Space in the current password.
        let mut bad_current = vec![8u8];
        bad_current.extend_from_slice(b"old pas1");
        bad_current.push(8u8);
        bad_current.extend_from_slice(b"Abcdef12");
        assert!(validate_set_password_payload(&bad_current).is_none());
        // Truncated (declared newLen longer than the payload).
        let mut truncated = vec![0u8, 8u8];
        truncated.extend_from_slice(b"Abcdef");
        assert!(validate_set_password_payload(&truncated).is_none());
    }

    #[test]
    fn set_password_payload_round_trip() {
        // First setup (no current password): [0][newLen][new].
        let built = super::build_set_password_payload(None, b"Abcdef12")
            .expect("valid new password builds");
        assert_eq!(
            built,
            [0u8, 8u8, b'A', b'b', b'c', b'd', b'e', b'f', b'1', b'2']
        );
        assert_eq!(
            validate_set_password_payload(&built),
            Some((None, b"Abcdef12".as_slice()))
        );
        // Password change: [curLen][current][newLen][new].
        let built = super::build_set_password_payload(Some(b"oldpass1"), b"Abcdef12")
            .expect("valid current + new builds");
        assert_eq!(
            built,
            [
                8u8, b'o', b'l', b'd', b'p', b'a', b's', b's', b'1', 8u8, b'A', b'b', b'c', b'd',
                b'e', b'f', b'1', b'2'
            ]
        );
        assert_eq!(
            validate_set_password_payload(&built),
            Some((Some(b"oldpass1".as_slice()), b"Abcdef12".as_slice()))
        );
        // Bad current (space / too long) is refused.
        assert!(super::build_set_password_payload(Some(b"old pas1"), b"Abcdef12").is_none());
        assert!(super::build_set_password_payload(Some(&[b'a'; 64]), b"Abcdef12").is_none());
    }
}
