//! HELO / HELLO pairing (§2) and the session state machine (D30).
//!
//! HELO request (PC → board, port 4210): `"HELO" | proto u8 | sessionFlag u8`
//! — 5 bytes, or 6 bytes with the flag; a 5-byte HELO means `sessionFlag 0`.
//! HELLO reply (board → PC, port 4211): 92 B fixed.
//!
//! Nonce/counter rules (D30): the nonce rotates **only** on board reboot
//! or a new session (HELO `sessionFlag 1`); routine 1 Hz HELOs (flag 0)
//! get the old nonce back. The counter is per-session and starts at 0
//! with a new nonce. Key/password fields are present **only** when the
//! board is unpaired *and* the 60 s pairing window is open — otherwise
//! they are all zero.

use crate::crc::{crc16, verify_crc16_trailer};
use crate::error::{HelloError, RejectCounters};

/// HELLO reply fixed length in bytes.
pub const HELLO_LEN: usize = 92;
/// Pairing window length in seconds (SoftAP-up + 60 s, §2.3).
pub const PAIRING_WINDOW_SECS: u64 = 60;
/// `KEY_RESET` armed window length in seconds (§4.4).
pub const KEY_RESET_ARMED_SECS: u64 = 120;
/// Device key length in bytes (per-device random, §4.1).
pub const DEVICE_KEY_LEN: usize = 32;
/// Nonce length in bytes.
pub const NONCE_LEN: usize = 16;
/// Wi-Fi/OTA password length in bytes (§5.1).
pub const WIFI_PASSWORD_LEN: usize = 24;

/// A HELO request (§2.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeloRequest {
    /// Protocol byte: `0x01` = v1 app (legacy), `0x02` = v2.x app.
    pub proto: u8,
    /// `0` = resume (old nonce back), `1` = new session (new nonce,
    /// counter from 0).
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
    /// Flags byte: bit0 unpaired, bit1 keyResetArmed, bit2 safeMode,
    /// bit3 pairingWindowOpen; bits 4–7 reserved (0).
    pub flags: u8,
    /// Echo of the request's `sessionFlag` (0/1).
    pub session_echo: u8,
    /// Board Wi-Fi MAC (device identity — never a key/password seed).
    pub board_mac: [u8; 6],
    /// Firmware version (major, minor, patch) — `4.0.0` for v4.0.x.
    pub firmware: (u8, u8, u8),
    /// 16-byte session nonce.
    pub nonce: [u8; NONCE_LEN],
    /// Per-device key — valid only when unpaired + window open.
    pub device_key: [u8; DEVICE_KEY_LEN],
    /// Wi-Fi/OTA password (24 ASCII bytes) — valid only when unpaired +
    /// window open.
    pub wifi_password: [u8; WIFI_PASSWORD_LEN],
}

impl HelloReply {
    /// Bit 0 — board is unpaired.
    #[must_use]
    pub fn unpaired(&self) -> bool {
        self.flags & 0x01 != 0
    }

    /// Bit 1 — `KEY_RESET` armed (BOOT held ≥ 10 s, 120 s window).
    #[must_use]
    pub fn key_reset_armed(&self) -> bool {
        self.flags & 0x02 != 0
    }

    /// Bit 2 — board is in safe mode.
    #[must_use]
    pub fn safe_mode(&self) -> bool {
        self.flags & 0x04 != 0
    }

    /// Bit 3 — pairing window is open (SoftAP-up + 60 s).
    #[must_use]
    pub fn pairing_window_open(&self) -> bool {
        self.flags & 0x08 != 0
    }

    /// `true` when the key/password fields carry real credentials:
    /// unpaired **and** pairing window open (§2.2). Otherwise both
    /// fields must be all zero.
    #[must_use]
    pub fn credentials_present(&self) -> bool {
        self.unpaired() && self.pairing_window_open()
    }

    /// The Wi-Fi password as a string, only when credentials are present
    /// and the bytes are valid ASCII. Returns `None` otherwise.
    #[must_use]
    pub fn wifi_password_str(&self) -> Option<&str> {
        if !self.credentials_present() {
            return None;
        }
        core::str::from_utf8(&self.wifi_password).ok()
    }

    /// Encode to the 92-byte wire form.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HELLO_LEN);
        out.extend_from_slice(b"RSHR");
        out.push(2);
        out.push(1);
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
    /// # Errors
    ///
    /// Returns [`HelloError`] for a wrong length, magic, protocol
    /// version, nonzero reserved byte, or CRC mismatch. A paired reply
    /// whose key/password fields are nonzero is *not* rejected here —
    /// callers must treat those fields as absent unless
    /// [`HelloReply::credentials_present`] is true.
    pub fn parse(data: &[u8]) -> Result<Self, HelloError> {
        if data.len() != HELLO_LEN {
            return Err(HelloError::BadLength);
        }
        if &data[0..4] != b"RSHR" {
            return Err(HelloError::BadMagic);
        }
        if data[4] != 2 || data[5] != 1 {
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

/// PC-side session state (D30 + §2.3).
///
/// Holds the current nonce, the per-session counter, the per-device key
/// (when paired), and whether at least one HMAC-verified control command
/// has been accepted this session (the stream-emit gate, SEC-E-14: the
/// board sends the 200 Hz stream only after such a command, normally
/// `GET_CAPS`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionState {
    /// Current 16-byte nonce from the latest HELLO reply.
    pub nonce: [u8; NONCE_LEN],
    /// Next counter value to use for a control request.
    pub next_counter: u32,
    /// Per-device key, once pairing delivered it (or it was restored
    /// from secure storage). `None` while unpaired.
    pub device_key: Option<[u8; DEVICE_KEY_LEN]>,
    /// Board MAC from the latest HELLO reply.
    pub board_mac: [u8; 6],
    /// Board is unpaired (pairing not yet complete).
    pub unpaired: bool,
    /// Pairing window is open (60 s from SoftAP-up).
    pub pairing_window_open: bool,
    /// At least one HMAC-verified control command accepted this session.
    pub authenticated: bool,
    /// Firmware version from the latest HELLO reply.
    pub firmware: (u8, u8, u8),
}

impl SessionState {
    /// Apply a HELLO reply to the session.
    ///
    /// A **new nonce** (reboot or `sessionFlag 1`) resets the counter to
    /// 0 and clears `authenticated` — the old nonce/counter must never
    /// be reused (§2.3). The same nonce keeps the counter running.
    pub fn apply_hello(&mut self, hello: &HelloReply) {
        if hello.nonce != self.nonce {
            self.nonce = hello.nonce;
            self.next_counter = 0;
            self.authenticated = false;
        }
        self.board_mac = hello.board_mac;
        self.unpaired = hello.unpaired();
        self.pairing_window_open = hello.pairing_window_open();
        self.firmware = hello.firmware;
        if hello.credentials_present() {
            self.device_key = Some(hello.device_key);
        }
    }

    /// Start a fresh session from a HELLO reply (counter 0, unauthenticated).
    #[must_use]
    pub fn from_hello(hello: &HelloReply) -> Self {
        Self {
            nonce: hello.nonce,
            next_counter: 0,
            device_key: if hello.credentials_present() {
                Some(hello.device_key)
            } else {
                None
            },
            board_mac: hello.board_mac,
            unpaired: hello.unpaired(),
            pairing_window_open: hello.pairing_window_open(),
            authenticated: false,
            firmware: hello.firmware,
        }
    }

    /// Allocate the counter for the next control request (increments).
    #[must_use]
    pub fn take_counter(&mut self) -> u32 {
        let counter = self.next_counter;
        self.next_counter = self.next_counter.wrapping_add(1);
        counter
    }

    /// Mark the session authenticated (an HMAC-verified control command
    /// was accepted) — the board may now emit the 200 Hz stream.
    pub fn mark_authenticated(&mut self) {
        self.authenticated = true;
    }
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            nonce: [0u8; NONCE_LEN],
            next_counter: 0,
            device_key: None,
            board_mac: [0u8; 6],
            unpaired: true,
            pairing_window_open: false,
            authenticated: false,
            firmware: (0, 0, 0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{HelloReply, SessionState};

    fn paired_reply() -> HelloReply {
        HelloReply {
            flags: 0,
            session_echo: 0,
            board_mac: [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff],
            firmware: (4, 0, 0),
            nonce: [0xa0; 16],
            device_key: [0; 32],
            wifi_password: [0; 24],
        }
    }

    #[test]
    fn new_nonce_resets_counter_and_auth() {
        let mut session = SessionState::from_hello(&paired_reply());
        let _ = session.take_counter();
        let _ = session.take_counter();
        session.mark_authenticated();
        assert_eq!(session.next_counter, 2);
        assert!(session.authenticated);

        let mut rotated = paired_reply();
        rotated.nonce = [0xb0; 16];
        session.apply_hello(&rotated);
        assert_eq!(session.next_counter, 0);
        assert!(!session.authenticated);
    }

    #[test]
    fn same_nonce_keeps_counter() {
        let mut session = SessionState::from_hello(&paired_reply());
        let _ = session.take_counter();
        session.apply_hello(&paired_reply());
        assert_eq!(session.next_counter, 1);
    }

    #[test]
    fn credentials_only_when_unpaired_and_window_open() {
        let mut reply = paired_reply();
        reply.flags = 0x09; // unpaired | window open
        assert!(reply.credentials_present());
        reply.flags = 0x01; // unpaired, window closed
        assert!(!reply.credentials_present());
        reply.flags = 0x08; // paired, window open
        assert!(!reply.credentials_present());
    }
}
