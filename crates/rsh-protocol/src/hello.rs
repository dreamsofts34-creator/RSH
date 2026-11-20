//! HELO / HELLO discovery (§2) and the plain link state (protocol v2.2).
//!
//! HELO request (PC → board, port 4210): `"HELO" | proto u8 | sessionFlag u8`
//! — 5 bytes, or 6 bytes with the flag; a 5-byte HELO means `sessionFlag 0`.
//! HELLO reply (board → PC, port 4211): 92 B fixed.
//!
//! Protocol v2.2 (no-pairing redesign, D1/D7): there is no pairing, no
//! device key, no nonce, and no HMAC. The `nonce[16]`, `device_key[32]`,
//! and `wifi_password[24]` fields stay in the 92-byte layout as
//! **zeroed/reserved** — a v2.2 board always sends zeros there, and the
//! PC never reads credentials out of a HELLO. HELLO flags: bit2
//! `safe_mode` is the only defined bit; bit0 (`unpaired`), bit1
//! (`key_reset_armed`), and bit3 (`pairing_window_open`) are reserved 0
//! (the armed state died with D7 — password-clear is a direct physical
//! BOOT hold, no armed window). Bits 4–7 are reserved 0.
//!
//! Old firmware (protocol 2.1, `0x21`) still parses: [`HelloReply::parse`]
//! accepts `(2,1)` and `(2,2)` and records which one arrived in
//! [`HelloReply::protocol`], so the caller can detect an old board and
//! offer the firmware update instead of attempting control (control is
//! unsupported against a 2.1 board). [`HelloReply::encode`] always
//! emits v2.2.

use crate::crc::{crc16, verify_crc16_trailer};
use crate::error::{HelloError, RejectCounters};

/// HELLO reply fixed length in bytes.
pub const HELLO_LEN: usize = 92;
/// Device key field length in bytes (reserved, always zero in v2.2).
pub const DEVICE_KEY_LEN: usize = 32;
/// Nonce field length in bytes (reserved, always zero in v2.2).
pub const NONCE_LEN: usize = 16;
/// Wi-Fi/OTA password field length in bytes (reserved, always zero in v2.2).
pub const WIFI_PASSWORD_LEN: usize = 24;

/// Protocol version of a v2.2 board: `(major, minor)`.
pub const PROTOCOL_VERSION: (u8, u8) = (2, 2);
/// Protocol version of an old (pre-redesign) board: `(major, minor)`.
/// Such a board still parses, but control is unsupported against it.
pub const PROTOCOL_VERSION_OLD: (u8, u8) = (2, 1);

/// A HELO request (§2.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeloRequest {
    /// Protocol byte: `0x01` = v1 app (legacy), `0x02` = v2.x app.
    pub proto: u8,
    /// `0` = resume, `1` = new session. Retained for wire
    /// compatibility; v2.2 boards carry no session material, so the
    /// flag has no effect on the board — a new session is just a new
    /// unsigned `connect()` on the PC side.
    pub session_flag: u8,
}

impl HeloRequest {
    /// Build a v2 HELO: 6 bytes `"HELO" | 0x02 | session_flag`.
    #[must_use]
    pub fn new_v2(session_flag: u8) -> Self {
        Self {
            proto: 0x02,
            session_flag,
        }
    }

    /// Encode to wire bytes.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(6);
        out.extend_from_slice(b"HELO");
        out.push(self.proto);
        out.push(self.session_flag);
        out
    }

    /// Decode a HELO request.
    ///
    /// Accepts 5 bytes (no `sessionFlag` — treated as 0, v2 draft compat)
    /// or 6 bytes. `proto` must be `0x01`/`0x02` and `sessionFlag` 0/1,
    /// otherwise the HELO is ignored (§2.4).
    ///
    /// # Errors
    ///
    /// Returns [`HelloError::BadLength`] for a wrong length, and
    /// [`HelloError::BadVersion`] for a bad `proto`/`sessionFlag`.
    pub fn decode(data: &[u8]) -> Result<Self, HelloError> {
        if data.len() != 5 && data.len() != 6 {
            return Err(HelloError::BadLength);
        }
        if &data[0..4] != b"HELO" {
            return Err(HelloError::BadMagic);
        }
        let proto = data[4];
        if proto != 0x01 && proto != 0x02 {
            return Err(HelloError::BadVersion);
        }
        let session_flag = if data.len() == 6 { data[5] } else { 0 };
        if session_flag > 1 {
            return Err(HelloError::BadVersion);
        }
        Ok(Self {
            proto,
            session_flag,
        })
    }
}

/// A decoded HELLO reply (§2.2), 92 B fixed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelloReply {
    /// Protocol version from the reply: `(2,2)` for a current board,
    /// `(2,1)` for an old (pre-redesign) board.
    pub protocol: (u8, u8),
    /// Flags byte: bit2 `safe_mode` is the only defined bit in v2.2;
    /// bits 0, 1, 3–7 are reserved (0).
    pub flags: u8,
    /// Echo of the request's `sessionFlag` (0/1).
    pub session_echo: u8,
    /// Board Wi-Fi MAC (device identity).
    pub board_mac: [u8; 6],
    /// Firmware version (major, minor, patch).
    pub firmware: (u8, u8, u8),
    /// Reserved 16-byte field (was the session nonce pre-v2.2).
    /// Always zero from a v2.2 board; never interpreted.
    pub nonce: [u8; NONCE_LEN],
    /// Reserved 32-byte field (was the per-device key pre-v2.2).
    /// Always zero from a v2.2 board; never interpreted.
    pub device_key: [u8; DEVICE_KEY_LEN],
    /// Reserved 24-byte field (was the Wi-Fi password pre-v2.2).
    /// Always zero from a v2.2 board; never interpreted.
    pub wifi_password: [u8; WIFI_PASSWORD_LEN],
}

impl HelloReply {
    /// Bit 2 — board is in safe mode. The only defined flag bit in v2.2.
    #[must_use]
    pub fn safe_mode(&self) -> bool {
        self.flags & 0x04 != 0
    }

    /// `true` when this HELLO came from an old (protocol 2.1) board.
    /// Control is unsupported against such a board — the caller should
    /// offer the firmware update instead.
    #[must_use]
    pub fn is_old_protocol(&self) -> bool {
        self.protocol == PROTOCOL_VERSION_OLD
    }

    /// Encode to the 92-byte wire form. Always emits protocol v2.2;
    /// the reserved fields are written as stored (callers building a
    /// v2.2 reply must leave them zero).
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HELLO_LEN);
        out.extend_from_slice(b"RSHR");
        out.push(PROTOCOL_VERSION.0);
        out.push(PROTOCOL_VERSION.1);
        out.push(self.flags);
        out.push(self.session_echo);
        out.extend_from_slice(&self.board_mac);
        out.push(self.firmware.0);
        out.push(self.firmware.1);
        out.push(self.firmware.2);
        out.push(0);
        out.extend_from_slice(&self.nonce);
        out.extend_from_slice(&self.device_key);
        out.extend_from_slice(&self.wifi_password);
        let crc = crc16(&out);
        out.extend_from_slice(&crc.to_le_bytes());
        out
    }

    /// Parse a 92-byte HELLO reply.
    ///
    /// Accepts protocol `(2,2)` (current) and `(2,1)` (old firmware —
    /// see [`HelloReply::is_old_protocol`]). Reserved-flag enforcement
    /// depends on the version: a v2.2 reply must have bits 0, 1, 3–7
    /// clear (only `safe_mode` may be set); a v2.1 reply is checked
    /// only for bits 4–7, since its bits 0/1/3 carried pairing meaning.
    /// The reserved `nonce`/`device_key`/`wifi_password` fields are
    /// accepted as-is and never interpreted — a caller must not treat
    /// them as credentials.
    ///
    /// # Errors
    ///
    /// Returns [`HelloError`] for a wrong length, magic, protocol
    /// version, nonzero reserved byte/flag bit, or CRC mismatch.
    pub fn parse(data: &[u8]) -> Result<Self, HelloError> {
        if data.len() != HELLO_LEN {
            return Err(HelloError::BadLength);
        }
        if &data[0..4] != b"RSHR" {
            return Err(HelloError::BadMagic);
        }
        let protocol = (data[4], data[5]);
        if protocol != PROTOCOL_VERSION && protocol != PROTOCOL_VERSION_OLD {
            return Err(HelloError::BadVersion);
        }
        // Reserved flag bits: bits 4–7 must be 0 in either version; a
        // v2.2 reply must additionally have bits 0, 1, 3 clear (only
        // bit2 safe_mode is defined). The session echo is a single
        // bit (0/1) — anything else is a malformed HELLO.
        let reserved_mask = if protocol == PROTOCOL_VERSION {
            0xFB
        } else {
            0xF0
        };
        if data[6] & reserved_mask != 0 {
            return Err(HelloError::RsvNonzero);
        }
        if data[7] > 1 {
            return Err(HelloError::BadVersion);
        }
        if data[17] != 0 {
            return Err(HelloError::RsvNonzero);
        }
        if !verify_crc16_trailer(data) {
            return Err(HelloError::BadCrc);
        }
        let mut board_mac = [0u8; 6];
        board_mac.copy_from_slice(&data[8..14]);
        let mut nonce = [0u8; NONCE_LEN];
        nonce.copy_from_slice(&data[18..34]);
        let mut device_key = [0u8; DEVICE_KEY_LEN];
        device_key.copy_from_slice(&data[34..66]);
        let mut wifi_password = [0u8; WIFI_PASSWORD_LEN];
        wifi_password.copy_from_slice(&data[66..90]);
        Ok(Self {
            protocol,
            flags: data[6],
            session_echo: data[7],
            board_mac,
            firmware: (data[14], data[15], data[16]),
            nonce,
            device_key,
            wifi_password,
        })
    }

    /// Parse, bumping the matching counter in `counters` on rejection.
    ///
    /// # Errors
    ///
    /// Same as [`HelloReply::parse`].
    pub fn parse_counted(data: &[u8], counters: &mut RejectCounters) -> Result<Self, HelloError> {
        match Self::parse(data) {
            Ok(reply) => Ok(reply),
            Err(e) => {
                match e {
                    HelloError::BadLength => counters.bad_frame += 1,
                    HelloError::BadMagic => counters.bad_magic += 1,
                    HelloError::BadVersion => counters.bad_version += 1,
                    HelloError::RsvNonzero => counters.rsv_nonzero += 1,
                    HelloError::BadCrc => counters.bad_crc += 1,
                }
                Err(e)
            }
        }
    }
}

/// PC-side plain link state: what the PC knows about the board link.
///
/// This is the v2.2 replacement for the old keyed `SessionState`: there
/// is no key, no nonce, and no auth flag — just the board identity from
/// the latest HELLO and whether an unsigned `GET_CAPS` has been
/// accepted on this link (the stream-emit gate, SEC-E-14: the board
/// sends the 200 Hz stream only after a plain `GET_CAPS`, normally the
/// first command of a `connect()`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkState {
    /// Board MAC from the latest HELLO reply.
    pub board_mac: [u8; 6],
    /// Firmware version from the latest HELLO reply.
    pub firmware: (u8, u8, u8),
    /// Protocol version from the latest HELLO reply.
    pub protocol: (u8, u8),
    /// Board is in safe mode.
    pub safe_mode: bool,
    /// At least one plain `GET_CAPS` has been accepted on this link.
    pub caps_ok: bool,
}

impl LinkState {
    /// Start a fresh link state from a HELLO reply (`caps_ok` false).
    #[must_use]
    pub fn from_hello(hello: &HelloReply) -> Self {
        Self {
            board_mac: hello.board_mac,
            firmware: hello.firmware,
            protocol: hello.protocol,
            safe_mode: hello.safe_mode(),
            caps_ok: false,
        }
    }

    /// Apply a HELLO reply to the link state. A HELLO from a different
    /// board MAC clears `caps_ok` — the gate belongs to the board that
    /// answered `GET_CAPS`, not to whatever board is heard now.
    pub fn apply_hello(&mut self, hello: &HelloReply) {
        if hello.board_mac != self.board_mac {
            self.caps_ok = false;
        }
        self.board_mac = hello.board_mac;
        self.firmware = hello.firmware;
        self.protocol = hello.protocol;
        self.safe_mode = hello.safe_mode();
    }

    /// Mark the link gate open (a plain `GET_CAPS` was accepted) — the
    /// board may now emit the 200 Hz stream.
    pub fn mark_caps_ok(&mut self) {
        self.caps_ok = true;
    }
}

impl Default for LinkState {
    fn default() -> Self {
        Self {
            board_mac: [0u8; 6],
            firmware: (0, 0, 0),
            protocol: PROTOCOL_VERSION,
            safe_mode: false,
            caps_ok: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{HelloReply, LinkState};

    fn reply() -> HelloReply {
        HelloReply {
            protocol: (2, 2),
            flags: 0,
            session_echo: 0,
            board_mac: [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff],
            firmware: (4, 0, 3),
            nonce: [0; 16],
            device_key: [0; 32],
            wifi_password: [0; 24],
        }
    }

    #[test]
    fn encode_writes_protocol_2_2() {
        let bytes = reply().encode();
        assert_eq!(bytes.len(), 92);
        assert_eq!((bytes[4], bytes[5]), (2, 2));
        let decoded = HelloReply::parse(&bytes).expect("v2.2 HELLO parses");
        assert_eq!(decoded.protocol, (2, 2));
        assert!(!decoded.is_old_protocol());
        assert_eq!(decoded, reply());
    }

    #[test]
    fn old_protocol_hello_still_parses() {
        // A pre-redesign (2,1) board: flags 0x09 (unpaired | window
        // open) and disclosed key/password bytes still parse — the
        // caller detects the old board via `protocol`, it does not get
        // a parse error.
        let mut bytes = reply().encode();
        bytes[4] = 2;
        bytes[5] = 1;
        bytes[6] = 0x09;
        bytes[7] = 1;
        bytes[18..34].copy_from_slice(&[0xa0; 16]);
        bytes[34..66].copy_from_slice(&[
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b,
            0x1c, 0x1d, 0x1e, 0x1f,
        ]);
        bytes[66..90].copy_from_slice(b"Ab3dEf6hJk9mNp2rSt5vWx8Y");
        // Recompute the trailing CRC over the edited body.
        let crc = crate::crc::crc16(&bytes[..90]);
        bytes[90..92].copy_from_slice(&crc.to_le_bytes());
        let decoded = HelloReply::parse(&bytes).expect("old HELLO parses");
        assert_eq!(decoded.protocol, (2, 1));
        assert!(decoded.is_old_protocol());
        assert_eq!(decoded.firmware, (4, 0, 3));
        // Encode always emits v2.2 — the old bytes are not round-tripped.
        assert_ne!(decoded.encode(), bytes);
        assert_eq!((decoded.encode()[4], decoded.encode()[5]), (2, 2));
    }

    #[test]
    fn reserved_flag_bits_rejected_in_v2_2() {
        use super::HelloError;
        // v2.2: only bit2 (safe_mode) is defined — bits 0, 1, 3, 4–7
        // must be 0.
        for flags in [0x01u8, 0x02, 0x08, 0x10, 0x20, 0x80, 0xF0, 0x0B] {
            let mut r = reply();
            r.flags = flags;
            let bytes = r.encode();
            assert_eq!(
                HelloReply::parse(&bytes),
                Err(HelloError::RsvNonzero),
                "flags {flags:#04x} must be rejected"
            );
        }
        // Defined bit (2) still parses.
        let mut r = reply();
        r.flags = 0x04;
        let decoded = HelloReply::parse(&r.encode()).expect("safe_mode parses");
        assert!(decoded.safe_mode());
    }

    #[test]
    fn session_echo_above_one_rejected_as_bad_version() {
        use super::HelloError;
        for echo in [2u8, 3, 255] {
            let mut r = reply();
            r.session_echo = echo;
            let bytes = r.encode();
            assert_eq!(
                HelloReply::parse(&bytes),
                Err(HelloError::BadVersion),
                "session_echo {echo} must be rejected"
            );
        }
        for echo in [0u8, 1] {
            let mut r = reply();
            r.session_echo = echo;
            assert!(HelloReply::parse(&r.encode()).is_ok());
        }
    }

    #[test]
    fn link_state_from_hello_and_apply() {
        let mut link = LinkState::from_hello(&reply());
        assert!(!link.caps_ok);
        assert_eq!(link.firmware, (4, 0, 3));
        link.mark_caps_ok();
        assert!(link.caps_ok);
        // Same board HELLO keeps the gate.
        link.apply_hello(&reply());
        assert!(link.caps_ok);
        // A different board clears the gate.
        let mut other = reply();
        other.board_mac = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66];
        link.apply_hello(&other);
        assert!(!link.caps_ok);
        assert_eq!(link.board_mac, other.board_mac);
    }
}
