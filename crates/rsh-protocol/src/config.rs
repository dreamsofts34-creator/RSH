//! Config blob (§3.6) and `SET_CFG` chunk staging (§3.5).
//!
//! Blob layout: header 8 B (`"RSHG" | schema u8 (= 2) | moduleCount u8 |
//! cfgVersion u16 LE`), module records fixed 16 B each
//! (`type | slot | flags | pinA | pinB | pinC | params[10]`), trailer
//! 4 B CRC32 (header + records, CRC-32/ISO-HDLC).
//!
//! `SET_CFG` chunk payload (§3.5): `offset u16 | total u16 | chunkLen u16 |
//! chunkCrc u16 (CRC16 over data only) | data`. Chunk ≤ 1024 B,
//! total ≤ 2048 B. Transaction (D28): stage → COMMIT → CONFIRM (30 s)
//! → permanent; no CONFIRM in 30 s → rollback.

use crate::crc::{crc16, crc32};
use crate::error::ConfigError;

/// Config blob magic.
pub const CONFIG_MAGIC: &[u8; 4] = b"RSHG";
/// Config blob schema version.
pub const CONFIG_SCHEMA: u8 = 2;
/// Config blob header size in bytes.
pub const CONFIG_HEADER_LEN: usize = 8;
/// Config record size in bytes.
pub const CONFIG_RECORD_LEN: usize = 16;
/// Config blob CRC32 trailer size in bytes.
pub const CONFIG_TRAILER_LEN: usize = 4;
/// Maximum config blob size in bytes (§3.5).
pub const MAX_CONFIG_LEN: usize = 2048;
/// Maximum `SET_CFG` chunk data in bytes (§3.5).
pub const MAX_CHUNK_LEN: usize = 1024;
/// CONFIRM window in seconds (D28).
pub const CONFIRM_DEADLINE_SECS: u8 = 30;

/// Config record types (§3.6).
pub mod record_type {
    /// `1` — `AXIS_ADC`.
    pub const AXIS_ADC: u8 = 1;
    /// `2` — `AXIS_ADS1115`.
    pub const AXIS_ADS1115: u8 = 2;
    /// `3` — BUTTON.
    pub const BUTTON: u8 = 3;
    /// `4` — `BUTTON_MATRIX`.
    pub const BUTTON_MATRIX: u8 = 4;
    /// `5` — ENCODER.
    pub const ENCODER: u8 = 5;
    /// `6` — `LED_STRIP`.
    pub const LED_STRIP: u8 = 6;
    /// `7` — BATTERY.
    pub const BATTERY: u8 = 7;
    /// `8` — `EXPANDER_MCP23017`.
    pub const EXPANDER_MCP23017: u8 = 8;
    /// `9` — `EXPANDER_PCF8574`.
    pub const EXPANDER_PCF8574: u8 = 9;
    /// `10` — `AXIS_GENERIC`.
    pub const AXIS_GENERIC: u8 = 10;
    /// `11` — SHIFTER (gate mode + H-pattern gear pins).
    pub const SHIFTER: u8 = 11;
    /// `12` — `AXIS_AS5600` (magnetic encoder axis, I²C addr 0x36 fixed).
    pub const AXIS_AS5600: u8 = 12;
}

/// One 16-byte config module record (§3.6).
///
/// Pin `0xFF` means "no pin". For SHIFTER records: `params[0]` is the
/// gate mode (0 Sequential, 1 H-pattern); `pinA`/`pinB`/`pinC` are gears
/// 1–3 and `params[1..4]` are gears 4–6.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConfigRecord {
    /// Record type (`record_type::*`).
    pub rec_type: u8,
    /// Slot (role within the type — see §3.6 slot table).
    pub slot: u8,
    /// Flags (type-specific).
    pub flags: u8,
    /// First pin (`0xFF` = none).
    pub pin_a: u8,
    /// Second pin (`0xFF` = none).
    pub pin_b: u8,
    /// Third pin (`0xFF` = none).
    pub pin_c: u8,
    /// Type-specific parameters (10 bytes).
    pub params: [u8; 10],
}

impl ConfigRecord {
    /// Build a record with all pins/params defaulting to `0xFF`/`0`
    /// except the fields given.
    #[must_use]
    pub fn new(rec_type: u8, slot: u8, pin_a: u8, pin_b: u8, pin_c: u8) -> Self {
        Self {
            rec_type,
            slot,
            flags: 0,
            pin_a,
            pin_b,
            pin_c,
            params: [0; 10],
        }
    }

    /// Encode the 16-byte record.
    #[must_use]
    pub fn encode(&self) -> [u8; CONFIG_RECORD_LEN] {
        let mut out = [0u8; CONFIG_RECORD_LEN];
        out[0] = self.rec_type;
        out[1] = self.slot;
        out[2] = self.flags;
        out[3] = self.pin_a;
        out[4] = self.pin_b;
        out[5] = self.pin_c;
        out[6..16].copy_from_slice(&self.params);
        out
    }

    /// Decode a 16-byte record.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::BadLength`] when `data` is not 16 bytes.
    pub fn decode(data: &[u8]) -> Result<Self, ConfigError> {
        if data.len() != CONFIG_RECORD_LEN {
            return Err(ConfigError::BadLength);
        }
        let mut params = [0u8; 10];
        params.copy_from_slice(&data[6..16]);
        Ok(Self {
            rec_type: data[0],
            slot: data[1],
            flags: data[2],
            pin_a: data[3],
            pin_b: data[4],
            pin_c: data[5],
            params,
        })
    }
}

/// A whole config blob (§3.6): header + records + CRC32 trailer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigBlob {
    /// Config version (increments on each committed change).
    pub cfg_version: u16,
    /// Module records (order preserved on encode).
    pub records: Vec<ConfigRecord>,
}

impl ConfigBlob {
    /// Build a blob from records and a config version.
    #[must_use]
    pub fn new(cfg_version: u16, records: Vec<ConfigRecord>) -> Self {
        Self {
            cfg_version,
            records,
        }
    }

    /// Encode the blob (header + records + CRC32 trailer).
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::TooBig`] when the blob would exceed
    /// 2048 bytes, and [`ConfigError::BadRecord`] when there are more
    /// than 255 records (the header count is a single byte).
    pub fn encode(&self) -> Result<Vec<u8>, ConfigError> {
        if self.records.len() > u8::MAX as usize {
            return Err(ConfigError::BadRecord);
        }
        let total = CONFIG_HEADER_LEN + self.records.len() * CONFIG_RECORD_LEN + CONFIG_TRAILER_LEN;
        if total > MAX_CONFIG_LEN {
            return Err(ConfigError::TooBig);
        }
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(CONFIG_MAGIC);
        out.push(CONFIG_SCHEMA);
        out.push(u8::try_from(self.records.len()).unwrap_or(u8::MAX));
        out.extend_from_slice(&self.cfg_version.to_le_bytes());
        for record in &self.records {
            out.extend_from_slice(&record.encode());
        }
        let crc = crc32(&out);
        out.extend_from_slice(&crc.to_le_bytes());
        Ok(out)
    }

    /// Decode a blob, verifying magic, schema, length, and CRC32.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] for a short blob, bad magic/schema,
    /// a length that does not match `moduleCount`, or a CRC32 mismatch.
    /// Record-level semantic validation (pin rules, SHIFTER gate mode)
    /// is [`crate::pin::validate_blob`], not this function.
    pub fn decode(data: &[u8]) -> Result<Self, ConfigError> {
        if data.len() < CONFIG_HEADER_LEN + CONFIG_TRAILER_LEN {
            return Err(ConfigError::TooShort);
        }
        if &data[0..4] != CONFIG_MAGIC {
            return Err(ConfigError::BadMagic);
        }
        if data[4] != CONFIG_SCHEMA {
            return Err(ConfigError::BadSchema);
        }
        let module_count = data[5] as usize;
        let expected = CONFIG_HEADER_LEN + module_count * CONFIG_RECORD_LEN + CONFIG_TRAILER_LEN;
        if data.len() != expected {
            return Err(ConfigError::BadLength);
        }
        if data.len() > MAX_CONFIG_LEN {
            return Err(ConfigError::TooBig);
        }
        let end = data.len();
        let stored_crc = u32::from_le_bytes([
            data[end - CONFIG_TRAILER_LEN],
            data[end - 3],
            data[end - 2],
            data[end - 1],
        ]);
        if crc32(&data[..data.len() - CONFIG_TRAILER_LEN]) != stored_crc {
            return Err(ConfigError::BadCrc);
        }
        let cfg_version = u16::from_le_bytes([data[6], data[7]]);
        let mut records = Vec::with_capacity(module_count);
        for i in 0..module_count {
            let off = CONFIG_HEADER_LEN + i * CONFIG_RECORD_LEN;
            records.push(ConfigRecord::decode(&data[off..off + CONFIG_RECORD_LEN])?);
        }
        Ok(Self {
            cfg_version,
            records,
        })
    }
}

/// The factory default blob (§3.6, Phase G C3): 188 B, 11 records,
/// `cfgVersion` 1. Axes 0:34, 1:35, 2:36, 4:32; battery 0:39;
/// buttons 0:27, 1:33, 2:4; encoders 0:23/19, 1:16/17; LED strip 0:21.
/// Clutch is OFF by default (no `AXIS_ADC` slot 3 record) and the
/// shifter is sequential (no SHIFTER record).
#[must_use]
pub fn factory_default_blob() -> ConfigBlob {
    use record_type::{AXIS_ADC, BATTERY, BUTTON, ENCODER, LED_STRIP};
    ConfigBlob::new(
        1,
        vec![
            ConfigRecord::new(AXIS_ADC, 0, 34, 0xFF, 0xFF),
            ConfigRecord::new(AXIS_ADC, 1, 35, 0xFF, 0xFF),
            ConfigRecord::new(AXIS_ADC, 2, 36, 0xFF, 0xFF),
            ConfigRecord::new(AXIS_ADC, 4, 32, 0xFF, 0xFF),
            ConfigRecord::new(BATTERY, 0, 39, 0xFF, 0xFF),
            ConfigRecord::new(BUTTON, 0, 27, 0xFF, 0xFF),
            ConfigRecord::new(BUTTON, 1, 33, 0xFF, 0xFF),
            ConfigRecord::new(BUTTON, 2, 4, 0xFF, 0xFF),
            ConfigRecord::new(ENCODER, 0, 23, 19, 0xFF),
            ConfigRecord::new(ENCODER, 1, 16, 17, 0xFF),
            ConfigRecord::new(LED_STRIP, 0, 21, 0xFF, 0xFF),
        ],
    )
}

/// One `SET_CFG` chunk (§3.5): `offset | total | chunkLen | chunkCrc |
/// data`. `chunkCrc` is CRC16 over `data` only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetCfgChunk {
    /// Offset of this chunk within the config blob.
    pub offset: u16,
    /// Total length of the whole config blob (same in every chunk).
    pub total: u16,
    /// Chunk data bytes.
    pub data: Vec<u8>,
}

impl SetCfgChunk {
    /// Build a chunk. The CRC is computed on encode.
    #[must_use]
    pub fn new(offset: u16, total: u16, data: Vec<u8>) -> Self {
        Self {
            offset,
            total,
            data,
        }
    }

    /// Encode the chunk payload (header + data).
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::TooBig`] when the chunk exceeds 1024 bytes
    /// or `total` exceeds 2048, and [`ConfigError::ChunkOutOfRange`]
    /// when `offset + data.len() > total`.
    pub fn encode(&self) -> Result<Vec<u8>, ConfigError> {
        if self.data.len() > MAX_CHUNK_LEN || self.total as usize > MAX_CONFIG_LEN {
            return Err(ConfigError::TooBig);
        }
        if self.offset as usize + self.data.len() > self.total as usize {
            return Err(ConfigError::ChunkOutOfRange);
        }
        let mut out = Vec::with_capacity(8 + self.data.len());
        out.extend_from_slice(&self.offset.to_le_bytes());
        out.extend_from_slice(&self.total.to_le_bytes());
        out.extend_from_slice(&(u16::try_from(self.data.len()).unwrap_or(u16::MAX)).to_le_bytes());
        out.extend_from_slice(&crc16(&self.data).to_le_bytes());
        out.extend_from_slice(&self.data);
        Ok(out)
    }

    /// Decode a chunk payload, verifying the chunk CRC16.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] for a short payload, a declared length
    /// that does not match, a chunk CRC mismatch, or size violations.
    pub fn decode(data: &[u8]) -> Result<Self, ConfigError> {
        if data.len() < 8 {
            return Err(ConfigError::TooShort);
        }
        let offset = u16::from_le_bytes([data[0], data[1]]);
        let total = u16::from_le_bytes([data[2], data[3]]);
        let chunk_len = u16::from_le_bytes([data[4], data[5]]) as usize;
        let chunk_crc = u16::from_le_bytes([data[6], data[7]]);
        if data.len() != 8 + chunk_len {
            return Err(ConfigError::BadLength);
        }
        if chunk_len > MAX_CHUNK_LEN || total as usize > MAX_CONFIG_LEN {
            return Err(ConfigError::TooBig);
        }
        if offset as usize + chunk_len > total as usize {
            return Err(ConfigError::ChunkOutOfRange);
        }
        let chunk_data = &data[8..];
        if crc16(chunk_data) != chunk_crc {
            return Err(ConfigError::ChunkBadCrc);
        }
        Ok(Self {
            offset,
            total,
            data: chunk_data.to_vec(),
        })
    }
}

/// Staging state for a `SET_CFG` upload session (§3.5–§3.6).
///
/// The first chunk must start at offset 0 (it opens the session);
/// later chunks may arrive in any order. When `staged == total` the
/// blob CRC32 is verified — a failure surfaces as `BadCrc` on the next
/// COMMIT, exactly as the board reports it.
#[derive(Clone, Debug)]
pub struct ConfigStaging {
    /// Expected total blob length for this session.
    pub total: usize,
    /// Staged bytes (zero-filled holes until written).
    staged: Vec<u8>,
    /// Which byte ranges have been received.
    received: Vec<bool>,
}

impl ConfigStaging {
    /// Start a staging session from a first chunk (offset 0 required).
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::FirstChunkNotZero`] when the chunk does
    /// not start at offset 0, and the chunk's own errors otherwise.
    pub fn start(chunk: &SetCfgChunk) -> Result<Self, ConfigError> {
        if chunk.offset != 0 {
            return Err(ConfigError::FirstChunkNotZero);
        }
        let total = chunk.total as usize;
        let mut staging = Self {
            total,
            staged: vec![0u8; total],
            received: vec![false; total],
        };
        staging.add(chunk)?;
        Ok(staging)
    }

    /// Add a chunk to the session.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::TotalMismatch`] when the chunk's `total`
    /// differs from the session's, and [`ConfigError::ChunkOutOfRange`]
    /// when the chunk overruns the blob.
    pub fn add(&mut self, chunk: &SetCfgChunk) -> Result<(), ConfigError> {
        if chunk.total as usize != self.total {
            return Err(ConfigError::TotalMismatch);
        }
        let off = chunk.offset as usize;
        if off + chunk.data.len() > self.total {
            return Err(ConfigError::ChunkOutOfRange);
        }
        self.staged[off..off + chunk.data.len()].copy_from_slice(&chunk.data);
        for slot in &mut self.received[off..off + chunk.data.len()] {
            *slot = true;
        }
        Ok(())
    }

    /// `true` when every byte of the blob has been received.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.received.iter().all(|b| *b)
    }

    /// Number of bytes received so far.
    #[must_use]
    pub fn staged_bytes(&self) -> usize {
        self.received.iter().filter(|b| **b).count()
    }

    /// Finish staging: decode and CRC-verify the complete blob.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::BadLength`] when the blob is incomplete,
    /// and the blob's own decode errors (including `BadCrc`) otherwise.
    pub fn finish(&self) -> Result<ConfigBlob, ConfigError> {
        if !self.is_complete() {
            return Err(ConfigError::BadLength);
        }
        ConfigBlob::decode(&self.staged)
    }
}

#[cfg(test)]
mod tests {
    use super::{ConfigBlob, factory_default_blob};

    #[test]
    fn factory_blob_is_188_bytes() {
        let blob = factory_default_blob();
        let encoded = blob.encode().expect("factory blob encodes");
        assert_eq!(encoded.len(), 188);
        assert_eq!(encoded[0..4], *b"RSHG");
        assert_eq!(encoded[4], 2);
        assert_eq!(encoded[5], 11);
        let decoded = ConfigBlob::decode(&encoded).expect("factory blob decodes");
        assert_eq!(decoded, blob);
    }
}
