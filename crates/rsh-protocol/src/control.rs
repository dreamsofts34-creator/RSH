//! Control frames (§3): `"RSHC"` + HMAC-SHA256 tag, opcodes, error codes,
//! replay / duplicate / retry rules (D3).
//!
//! Frame layout: `magic[4] | cmd u8 | reqId u16 | counter u32 | len u16 |
//! payload | tag[16]`; total `13 + len + 16`, `len ≤ 1024`.
//!
//! Tag (v2.1-frozen, §3.2 — header is **not** in the HMAC):
//! `tag = HMAC-SHA256(key32, nonce ‖ counter_LE ‖ cmd ‖ payload)[0:16]`.
//! Verification uses `Mac::verify_truncated_left` — a constant-time
//! check for a left-truncated tag. (A3 spike finding: `verify_slice`
//! requires the FULL 32-byte tag and always fails on the 16-byte wire
//! tag, so it must never be used here.)
//!
//! Parse errors (§3.1): `TooShort` only when the frame is shorter than the
//! 13-byte header; every other length mismatch is `BadLength`.

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

use crate::error::{ControlParseError, RejectCounters};

/// HMAC-SHA256 type alias.
type HmacSha256 = Hmac<Sha256>;

/// Control frame header size in bytes.
pub const CONTROL_HEADER_LEN: usize = 13;
/// HMAC tag size in bytes (first 16 bytes of HMAC-SHA256).
pub const TAG_LEN: usize = 16;
/// Maximum control payload in bytes.
pub const MAX_PAYLOAD_LEN: usize = 1024;
/// Maximum control frame size in bytes (`13 + 1024 + 16`).
pub const MAX_FRAME_LEN: usize = CONTROL_HEADER_LEN + MAX_PAYLOAD_LEN + TAG_LEN;
/// Maximum retry attempts per request: 3 sends total (§3.2).
pub const MAX_RETRIES: u32 = 3;
/// Retry interval in milliseconds (§3.2).
pub const RETRY_INTERVAL_MS: u64 = 250;
/// Duplicate-cache size: the board caches the last 32 `(reqId, counter)`
/// pairs per session (§3.2).
pub const DUPLICATE_CACHE_SIZE: usize = 32;

/// Control opcodes (§3.3).
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
    /// `0x0B` — `KEY_RESET`.
    KeyReset = 0x0B,
    /// `0x0C` — `SET_PASSWORD`.
    SetPassword = 0x0C,
}

impl Opcode {
    /// Decode an opcode byte; `None` for unknown commands.
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
            0x0B => Self::KeyReset,
            0x0C => Self::SetPassword,
            _ => return None,
        })
    }

    /// `true` when the byte is a known opcode (0x01–0x0C).
    #[must_use]
    pub fn is_known(b: u8) -> bool {
        Self::from_u8(b).is_some()
    }
}

/// Reply status codes (§3.9) — the first byte of every reply payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ErrorCode {
    /// `0` — success.
    Ok = 0,
    /// `1` — HMAC tag verification failed.
    BadTag = 1,
    /// `2` — counter ≤ session max-seen with a new reqId.
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
}

impl ErrorCode {
    /// Decode a status byte; `None` for values > 7.
    #[must_use]
    pub fn from_u8(b: u8) -> Option<Self> {
        Some(match b {
            0 => Self::Ok,
            1 => Self::BadTag,
            2 => Self::Replay,
            3 => Self::BadCrc,
            4 => Self::TooBig,
            5 => Self::PinConflict,
            6 => Self::Unsupported,
            7 => Self::Busy,
            _ => return None,
        })
    }
}

/// A parsed control frame (structure + payload, tag already verified by
/// [`ControlFrame::verify`] / [`verify_frame`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlFrame {
    /// Command byte (request: 0x01–0x0C; reply: `req_cmd | 0x80`).
    pub cmd: u8,
    /// Request id chosen by the PC; identical across retries (D3).
    pub req_id: u16,
    /// Per-session monotonic counter (u32 LE in the HMAC input).
    pub counter: u32,
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

    /// Encode this frame with a fresh HMAC tag.
    ///
    /// # Errors
    ///
    /// Returns [`ControlParseError::BadLength`] when the payload exceeds
    /// 1024 bytes. (Key/nonce length misuse is a caller bug and panics —
    /// see [`compute_tag`].)
    pub fn encode(&self, key: &[u8; 32], nonce: &[u8; 16]) -> Result<Vec<u8>, ControlParseError> {
        build_frame(
            key,
            nonce,
            self.cmd,
            self.req_id,
            self.counter,
            &self.payload,
        )
    }

    /// Parse only the structure (magic + header + length), without
    /// verifying the tag. Use [`ControlFrame::verify`] for a full check.
    ///
    /// # Errors
    ///
    /// Returns [`ControlParseError`] for a short frame, bad magic, or any
    /// length mismatch (`TooShort` only below 13 bytes).
    pub fn parse_structure(data: &[u8]) -> Result<Self, ControlParseError> {
        if data.len() < CONTROL_HEADER_LEN {
            return Err(ControlParseError::TooShort);
        }
        if &data[0..4] != b"RSHC" {
            return Err(ControlParseError::BadMagic);
        }
        let cmd = data[4];
        let req_id = u16::from_le_bytes([data[5], data[6]]);
        let counter = u32::from_le_bytes([data[7], data[8], data[9], data[10]]);
        let len = u16::from_le_bytes([data[11], data[12]]) as usize;
        if len > MAX_PAYLOAD_LEN || data.len() != CONTROL_HEADER_LEN + len + TAG_LEN {
            return Err(ControlParseError::BadLength);
        }
        Ok(Self {
            cmd,
            req_id,
            counter,
            payload: data[CONTROL_HEADER_LEN..CONTROL_HEADER_LEN + len].to_vec(),
        })
    }

    /// Parse and verify the HMAC tag (constant-time compare).
    ///
    /// # Errors
    ///
    /// Returns [`ControlParseError`] for structural problems or a tag
    /// mismatch (`BadTag`).
    pub fn verify(
        data: &[u8],
        key: &[u8; 32],
        nonce: &[u8; 16],
    ) -> Result<Self, ControlParseError> {
        let frame = Self::parse_structure(data)?;
        let tag_off = CONTROL_HEADER_LEN + frame.payload.len();
        let mut tag = [0u8; TAG_LEN];
        tag.copy_from_slice(&data[tag_off..tag_off + TAG_LEN]);
        if !verify_tag(key, nonce, frame.cmd, frame.counter, &frame.payload, &tag) {
            return Err(ControlParseError::BadTag);
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
        key: &[u8; 32],
        nonce: &[u8; 16],
        counters: &mut RejectCounters,
    ) -> Result<Self, ControlParseError> {
        match Self::verify(data, key, nonce) {
            Ok(frame) => Ok(frame),
            Err(e) => {
                match e {
                    ControlParseError::TooShort => counters.too_short += 1,
                    ControlParseError::BadMagic => counters.bad_magic += 1,
                    ControlParseError::BadLength => counters.bad_length += 1,
                    ControlParseError::BadTag => counters.bad_tag += 1,
                }
                Err(e)
            }
        }
    }
}

/// Verify a 16-byte wire tag in constant time.
///
/// The wire tag is the LEFT-truncated HMAC (`[0:16]`), so this uses
/// `Mac::verify_truncated_left`. `verify_slice` must never be used
/// here: it requires the full 32-byte tag and always fails on the
/// 16-byte wire tag (A3 spike finding).
///
/// # Panics
///
/// Panics only if the HMAC key length is invalid — impossible for the
/// fixed `[u8; 32]` key type used by every caller.
#[must_use]
pub fn verify_tag(
    key: &[u8; 32],
    nonce: &[u8; 16],
    cmd: u8,
    counter: u32,
    payload: &[u8],
    tag: &[u8; TAG_LEN],
) -> bool {
    let mut mac = HmacSha256::new_from_slice(key).expect("32-byte HMAC key is valid");
    mac.update(nonce);
    mac.update(&counter.to_le_bytes());
    mac.update(&[cmd]);
    mac.update(payload);
    mac.verify_truncated_left(tag).is_ok()
}

/// Compute the 16-byte tag: first 16 bytes of
/// `HMAC-SHA256(key, nonce ‖ counter_LE ‖ cmd ‖ payload)`.
///
/// # Panics
///
/// Panics only if the HMAC key length is invalid — impossible for the
/// fixed `[u8; 32]` key type used by every caller.
#[must_use]
pub fn compute_tag(
    key: &[u8; 32],
    nonce: &[u8; 16],
    cmd: u8,
    counter: u32,
    payload: &[u8],
) -> [u8; TAG_LEN] {
    let mut mac = HmacSha256::new_from_slice(key).expect("32-byte HMAC key is valid");
    mac.update(nonce);
    mac.update(&counter.to_le_bytes());
    mac.update(&[cmd]);
    mac.update(payload);
    let digest = mac.finalize().into_bytes();
    let mut tag = [0u8; TAG_LEN];
    tag.copy_from_slice(&digest[..TAG_LEN]);
    tag
}

/// Build a complete control frame (header + payload + tag).
///
/// # Errors
///
/// Returns [`ControlParseError::BadLength`] when `payload.len() > 1024`.
pub fn build_frame(
    key: &[u8; 32],
    nonce: &[u8; 16],
    cmd: u8,
    req_id: u16,
    counter: u32,
    payload: &[u8],
) -> Result<Vec<u8>, ControlParseError> {
    if payload.len() > MAX_PAYLOAD_LEN {
        return Err(ControlParseError::BadLength);
    }
    let tag = compute_tag(key, nonce, cmd, counter, payload);
    let mut out = Vec::with_capacity(CONTROL_HEADER_LEN + payload.len() + TAG_LEN);
    out.extend_from_slice(b"RSHC");
    out.push(cmd);
    out.extend_from_slice(&req_id.to_le_bytes());
    out.extend_from_slice(&counter.to_le_bytes());
    out.extend_from_slice(&(u16::try_from(payload.len()).unwrap_or(u16::MAX)).to_le_bytes());
    out.extend_from_slice(payload);
    out.extend_from_slice(&tag);
    Ok(out)
}

/// Build a reply frame: `cmd = req_cmd | 0x80`, payload is
/// `status ‖ body`, counter echoes the request counter (§3.2).
///
/// # Errors
///
/// Returns [`ControlParseError::BadLength`] when the reply payload
/// (`1 + body.len()`) exceeds 1024 bytes.
pub fn build_reply(
    key: &[u8; 32],
    nonce: &[u8; 16],
    req_cmd: u8,
    req_id: u16,
    counter: u32,
    status: ErrorCode,
    body: &[u8],
) -> Result<Vec<u8>, ControlParseError> {
    let mut payload = Vec::with_capacity(1 + body.len());
    payload.push(status as u8);
    payload.extend_from_slice(body);
    build_frame(key, nonce, req_cmd | 0x80, req_id, counter, &payload)
}

/// Verify a received frame's tag without allocating the parsed frame.
/// Returns `false` for any structural problem or tag mismatch.
#[must_use]
pub fn verify_frame(data: &[u8], key: &[u8; 32], nonce: &[u8; 16]) -> bool {
    ControlFrame::verify(data, key, nonce).is_ok()
}

/// RFC 1982 serial-number arithmetic for the per-session counter (§3.2).
///
/// A counter is a replay when `(counter − max_seen)` interpreted as a
/// signed 32-bit difference is `≤ 0`. This makes the wrap
/// `0xFFFFFFFF → 0` a *new* counter, not a replay. Any gap above
/// `max_seen` is accepted (a lost packet must not wedge the session).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReplayTracker {
    /// Highest counter accepted so far in this session (`None` = no frame yet).
    max_seen: Option<u32>,
}

impl ReplayTracker {
    /// Create a fresh tracker for a new session (counter restarts at 0).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Highest counter seen so far, if any.
    #[must_use]
    pub fn max_seen(&self) -> Option<u32> {
        self.max_seen
    }

    /// `true` when `counter` is a replay against the current `max_seen`.
    /// The very first counter of a session is never a replay.
    #[must_use]
    pub fn is_replay(&self, counter: u32) -> bool {
        match self.max_seen {
            None => false,
            Some(max) => (counter.wrapping_sub(max).cast_signed()) <= 0,
        }
    }

    /// Record a counter. Returns `false` (and does not move `max_seen`)
    /// when the counter is a replay.
    pub fn observe(&mut self, counter: u32) -> bool {
        if self.is_replay(counter) {
            return false;
        }
        self.max_seen = Some(counter);
        true
    }

    /// Reset for a new session (new nonce ⇒ counter restarts at 0, D30).
    pub fn reset(&mut self) {
        self.max_seen = None;
    }
}

/// Exact-retransmit cache: the last 32 `(reqId, counter)` pairs (§3.2).
/// A byte-identical retransmit is answered from the cached reply and never
/// re-executed.
#[derive(Clone, Debug, Default)]
pub struct DuplicateCache {
    entries: [Option<(u16, u32)>; DUPLICATE_CACHE_SIZE],
    next: usize,
}

impl DuplicateCache {
    /// Create an empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `true` when this exact `(req_id, counter)` pair was already seen.
    #[must_use]
    pub fn is_duplicate(&self, req_id: u16, counter: u32) -> bool {
        self.entries.contains(&Some((req_id, counter)))
    }

    /// Record a pair. Returns `false` when it was already present
    /// (a duplicate retransmit).
    pub fn insert(&mut self, req_id: u16, counter: u32) -> bool {
        if self.is_duplicate(req_id, counter) {
            return false;
        }
        self.entries[self.next] = Some((req_id, counter));
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
/// `req_id` and uses a **new** counter. After the third send the request
/// fails and the UI is told.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControlRetry {
    /// Request id, identical across retries.
    pub req_id: u16,
    /// Counter to use for the next send.
    pub next_counter: u32,
    /// Sends already made (1 after the first send).
    pub attempts: u32,
}

impl ControlRetry {
    /// Start a request: first send uses `first_counter`, `req_id` fixed.
    #[must_use]
    pub fn new(req_id: u16, first_counter: u32) -> Self {
        Self {
            req_id,
            next_counter: first_counter,
            attempts: 0,
        }
    }

    /// Counter for the next send, or `None` when all 3 attempts are used.
    /// Each call consumes one attempt and advances the counter.
    pub fn next_send(&mut self) -> Option<u32> {
        if self.attempts >= MAX_RETRIES {
            return None;
        }
        let counter = self.next_counter;
        self.next_counter = self.next_counter.wrapping_add(1);
        self.attempts += 1;
        Some(counter)
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

/// Validate a `SET_PASSWORD` request payload (§5.2): `newLen u8 (8..63)` +
/// printable-ASCII password bytes (`0x21–0x7E`, no space).
///
/// Returns the password bytes on success, `None` on any violation
/// (length or charset — the board replies `UNSUPPORTED`).
#[must_use]
pub fn validate_set_password_payload(payload: &[u8]) -> Option<&[u8]> {
    if payload.is_empty() {
        return None;
    }
    let new_len = payload[0] as usize;
    if !(8..=63).contains(&new_len) || payload.len() != 1 + new_len {
        return None;
    }
    let password = &payload[1..];
    if password.iter().any(|b| !(0x21..=0x7E).contains(b)) {
        return None;
    }
    Some(password)
}

/// Build a `SET_PASSWORD` request payload.
///
/// Returns `None` when the password violates the §5.2 rules.
#[must_use]
pub fn build_set_password_payload(password: &[u8]) -> Option<Vec<u8>> {
    if !(8..=63).contains(&password.len()) {
        return None;
    }
    if password.iter().any(|b| !(0x21..=0x7E).contains(b)) {
        return None;
    }
    let mut out = Vec::with_capacity(1 + password.len());
    out.push(u8::try_from(password.len()).unwrap_or(u8::MAX));
    out.extend_from_slice(password);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::{ControlRetry, ErrorCode, Opcode, ReplayTracker, validate_set_password_payload};

    #[test]
    fn opcode_round_trip() {
        for b in 1u8..=12 {
            assert!(Opcode::from_u8(b).is_some());
            assert!(Opcode::is_known(b));
        }
        assert_eq!(Opcode::from_u8(0), None);
        assert_eq!(Opcode::from_u8(0x0D), None);
        assert_eq!(Opcode::from_u8(0x81), None);
    }

    #[test]
    fn error_code_round_trip() {
        for b in 0u8..=7 {
            assert!(ErrorCode::from_u8(b).is_some());
        }
        assert_eq!(ErrorCode::from_u8(8), None);
    }

    #[test]
    fn replay_wrap_is_new() {
        let mut tracker = ReplayTracker::new();
        assert!(tracker.observe(0xFFFF_FFFF));
        // Counter wrap 0xFFFFFFFF -> 0 is NEW, not a replay (RFC 1982).
        assert!(!tracker.is_replay(0));
        assert!(tracker.observe(0));
        assert!(tracker.is_replay(0));
        assert!(tracker.is_replay(0xFFFF_FFFF));
    }

    #[test]
    fn replay_gap_accepted() {
        let mut tracker = ReplayTracker::new();
        assert!(tracker.observe(5));
        assert!(!tracker.is_replay(100)); // gaps never wedge the session
        assert!(tracker.observe(100));
        assert!(tracker.is_replay(6));
    }

    #[test]
    fn retry_keeps_req_id_new_counter_three_times() {
        let mut retry = ControlRetry::new(42, 7);
        assert_eq!(retry.next_send(), Some(7));
        assert_eq!(retry.next_send(), Some(8));
        assert_eq!(retry.next_send(), Some(9));
        assert_eq!(retry.next_send(), None);
        assert!(retry.exhausted());
    }

    #[test]
    fn verify_tag_truncated_left_semantics() {
        // A3 spike: verify_slice needs the FULL 32-byte tag and always
        // fails on the 16-byte wire tag. verify_tag (verify_truncated_left)
        // accepts the 16-byte tag and rejects a flipped byte / wrong key.
        use super::{compute_tag, verify_tag};
        let key = [0x42u8; 32];
        let nonce = [0x24u8; 16];
        let tag = compute_tag(&key, &nonce, 0x01, 0, b"");
        assert!(verify_tag(&key, &nonce, 0x01, 0, b"", &tag));
        let mut bad = tag;
        bad[0] ^= 0xFF;
        assert!(!verify_tag(&key, &nonce, 0x01, 0, b"", &bad));
        assert!(!verify_tag(&[0xFFu8; 32], &nonce, 0x01, 0, b"", &tag));
        assert!(!verify_tag(&key, &nonce, 0x01, 1, b"", &tag));
    }

    #[test]
    fn set_password_validation() {
        assert!(validate_set_password_payload(&[]).is_none());
        let mut good = vec![8u8];
        good.extend_from_slice(b"Abcdef12");
        assert_eq!(
            validate_set_password_payload(&good),
            Some(b"Abcdef12".as_slice())
        );
        let mut short = vec![7u8];
        short.extend_from_slice(b"Abcdef1");
        assert!(validate_set_password_payload(&short).is_none());
        let mut space = vec![8u8];
        space.extend_from_slice(b"Abc def1");
        assert!(validate_set_password_payload(&space).is_none());
    }
}
