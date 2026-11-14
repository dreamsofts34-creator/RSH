//! `GET_CAPS` reply body (§3.4) — board capabilities + pin table.
//!
//! Reply payload (after the 1-byte `status`): `boardMac[6] | fwMajor |
//! fwMinor | fwPatch | protoMajor | protoMinor | nAxesMax | nBtnMax |
//! nEncMax | wifiChannel | capsFlags | pinCount | pin table (8 B × N)`.
//!
//! Pin table entry (8 B): `pin | kind | module | caps | reserved[4]`.
//! Kind codes (§3.4.1): 0 NONE, 1 `ADC_NATIVE`, 2 `ADC_ADS1115`,
//! 3 `DIGITAL_IN`, 4 `DIGITAL_OUT`, 5 `LED_STRIP`, 6 `I2C_SDA`, 7 `I2C_SCL`,
//! 8 `EXPANDER_MCP23017`, 9 `EXPANDER_PCF8574`, 10 `AXIS_GENERIC`,
//! 11 `RESERVED_PIN`, 12 `AXIS_AS5600`.
//! Module codes: 0 NONE, 1 STEERING, 2 ACCELERATOR, 3 BRAKE, 4 CLUTCH,
//! 5 HANDBRAKE, 6 SHIFTER, 7 HORN, 8 BUTTONS, 9 ENCODERS, 10 LED,
//! 11 BATTERY.

use crate::control::{ControlFrame, ErrorCode};
use crate::error::ControlParseError;

/// Maximum pin table entries in a `GET_CAPS` reply.
pub const MAX_PIN_ENTRIES: usize = 40;

/// One 8-byte pin table entry (§3.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PinTableEntry {
    /// GPIO number (0–39).
    pub pin: u8,
    /// Kind code (§3.4.1).
    pub kind: u8,
    /// Module code (§3.4.1, 0 = NONE / unassigned).
    pub module: u8,
    /// Capability bits: bit0 input, bit1 output, bit2 adc, bit3 pwm,
    /// bit4 pullup; bits 5–7 reserved (0).
    pub caps: u8,
}

impl PinTableEntry {
    /// `true` when the pin can be an input.
    #[must_use]
    pub fn is_input(&self) -> bool {
        self.caps & 0x01 != 0
    }

    /// `true` when the pin can drive an output.
    #[must_use]
    pub fn is_output(&self) -> bool {
        self.caps & 0x02 != 0
    }

    /// `true` when the pin has an ADC.
    #[must_use]
    pub fn is_adc(&self) -> bool {
        self.caps & 0x04 != 0
    }

    /// Encode the 8-byte entry (reserved bytes are always 0).
    #[must_use]
    pub fn encode(&self) -> [u8; 8] {
        [self.pin, self.kind, self.module, self.caps, 0, 0, 0, 0]
    }

    /// Decode an 8-byte entry.
    ///
    /// # Errors
    ///
    /// Returns [`ControlParseError::BadLength`] for a wrong length or a
    /// nonzero reserved byte.
    pub fn decode(data: &[u8]) -> Result<Self, ControlParseError> {
        if data.len() != 8 || data[4..8] != [0, 0, 0, 0] {
            return Err(ControlParseError::BadLength);
        }
        Ok(Self {
            pin: data[0],
            kind: data[1],
            module: data[2],
            caps: data[3],
        })
    }
}

/// Decoded `GET_CAPS` reply body (§3.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GetCapsReply {
    /// Reply status byte.
    pub status: ErrorCode,
    /// Board Wi-Fi MAC (= HELLO reply `boardMac`).
    pub board_mac: [u8; 6],
    /// Firmware version (major, minor, patch).
    pub firmware: (u8, u8, u8),
    /// Protocol version (major, minor).
    pub protocol: (u8, u8),
    /// Maximum axes (8).
    pub n_axes_max: u8,
    /// Maximum buttons on the wire (64; config ceiling is 128, §1.1 N1).
    pub n_btn_max: u8,
    /// Maximum encoders (4).
    pub n_enc_max: u8,
    /// Current `SoftAP` channel (1–13); 0 = unknown / no AP.
    pub wifi_channel: u8,
    /// Caps flags: bit0 slowAxis, bit1 safeMode, bit2 unpaired,
    /// bit3 keyResetArmed; bits 4–7 reserved (0).
    pub caps_flags: u8,
    /// Pin table entries.
    pub pin_table: Vec<PinTableEntry>,
}

impl GetCapsReply {
    /// Bit 0 — a slow (ADS1115) axis exists (≤ 50 Hz).
    #[must_use]
    pub fn slow_axis(&self) -> bool {
        self.caps_flags & 0x01 != 0
    }

    /// Bit 1 — safe mode.
    #[must_use]
    pub fn safe_mode(&self) -> bool {
        self.caps_flags & 0x02 != 0
    }

    /// Bit 2 — unpaired.
    #[must_use]
    pub fn unpaired(&self) -> bool {
        self.caps_flags & 0x04 != 0
    }

    /// Bit 3 — `KEY_RESET` armed.
    #[must_use]
    pub fn key_reset_armed(&self) -> bool {
        self.caps_flags & 0x08 != 0
    }

    /// Parse the reply payload of a verified `GET_CAPS` control frame
    /// (the payload *after* the control frame has been verified — the
    /// frame's `cmd` must be `0x81`).
    ///
    /// # Errors
    ///
    /// Returns [`ControlParseError::BadLength`] for a payload that is
    /// too short, has a wrong `cmd`, a nonzero reserved caps bit, or a
    /// pin count that does not match the payload length.
    pub fn from_frame(frame: &ControlFrame) -> Result<Self, ControlParseError> {
        if frame.cmd != 0x81 {
            return Err(ControlParseError::BadLength);
        }
        Self::from_payload(&frame.payload)
    }

    /// Parse a `GET_CAPS` reply payload (status byte first).
    ///
    /// # Errors
    ///
    /// Same as [`GetCapsReply::from_frame`].
    pub fn from_payload(payload: &[u8]) -> Result<Self, ControlParseError> {
        if payload.len() < 18 {
            return Err(ControlParseError::BadLength);
        }
        let status = ErrorCode::from_u8(payload[0]).ok_or(ControlParseError::BadLength)?;
        let pin_count = payload[17] as usize;
        if pin_count > MAX_PIN_ENTRIES || payload.len() != 18 + pin_count * 8 {
            return Err(ControlParseError::BadLength);
        }
        let caps_flags = payload[16];
        if caps_flags & 0xF0 != 0 {
            return Err(ControlParseError::BadLength);
        }
        let mut board_mac = [0u8; 6];
        board_mac.copy_from_slice(&payload[1..7]);
        let mut pin_table = Vec::with_capacity(pin_count);
        for i in 0..pin_count {
            let off = 18 + i * 8;
            pin_table.push(PinTableEntry::decode(&payload[off..off + 8])?);
        }
        Ok(Self {
            status,
            board_mac,
            firmware: (payload[7], payload[8], payload[9]),
            protocol: (payload[10], payload[11]),
            n_axes_max: payload[12],
            n_btn_max: payload[13],
            n_enc_max: payload[14],
            wifi_channel: payload[15],
            caps_flags,
            pin_table,
        })
    }

    /// Encode the reply body (status byte + fields + pin table).
    #[must_use]
    pub fn encode_body(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(18 + self.pin_table.len() * 8);
        out.push(self.status as u8);
        out.extend_from_slice(&self.board_mac);
        out.push(self.firmware.0);
        out.push(self.firmware.1);
        out.push(self.firmware.2);
        out.push(self.protocol.0);
        out.push(self.protocol.1);
        out.push(self.n_axes_max);
        out.push(self.n_btn_max);
        out.push(self.n_enc_max);
        out.push(self.wifi_channel);
        out.push(self.caps_flags);
        out.push(u8::try_from(self.pin_table.len()).unwrap_or(u8::MAX));
        for entry in &self.pin_table {
            out.extend_from_slice(&entry.encode());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::{GetCapsReply, PinTableEntry};
    use crate::control::ErrorCode;

    #[test]
    fn caps_round_trip() {
        let reply = GetCapsReply {
            status: ErrorCode::Ok,
            board_mac: [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff],
            firmware: (4, 0, 1),
            protocol: (2, 1),
            n_axes_max: 8,
            n_btn_max: 64,
            n_enc_max: 4,
            wifi_channel: 6,
            caps_flags: 0,
            pin_table: vec![PinTableEntry {
                pin: 34,
                kind: 1,
                module: 1,
                caps: 0x05,
            }],
        };
        let body = reply.encode_body();
        assert_eq!(body.len(), 26);
        let decoded = GetCapsReply::from_payload(&body).expect("caps round-trip");
        assert_eq!(decoded, reply);
    }
}
