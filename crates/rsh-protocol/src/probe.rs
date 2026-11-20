//! `PIN_PROBE` stream frames (§3.8).
//!
//! During a probe the board sends one frame per 100 ms on port 4211:
//! `"RSHP" | ver 0x22 | seq u16 | pin u8 | mode u8 | sampleCount u16 |
//! samples | CRC16`. Mode 0 samples are `u8` levels (0/1); mode 1
//! samples are `u16` LE millivolts. `sampleCount` is 1–64.

use crate::crc::{crc16, verify_crc16_trailer};
use crate::error::ProbeError;
use crate::stream::VERSION;

/// Maximum samples per probe frame.
pub const MAX_PROBE_SAMPLES: usize = 64;

/// Probe samples: digital levels or ADC millivolts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeSamples {
    /// Mode 0 — digital levels, one byte per sample.
    Digital(Vec<u8>),
    /// Mode 1 — ADC millivolts, one `u16` per sample.
    Analog(Vec<u16>),
}

impl ProbeSamples {
    /// Number of samples.
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::Digital(v) => v.len(),
            Self::Analog(v) => v.len(),
        }
    }

    /// `true` when there are no samples.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A decoded `PIN_PROBE` stream frame (§3.8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeFrame {
    /// Probe sequence (0-based within this probe).
    pub seq: u16,
    /// GPIO pin being probed.
    pub pin: u8,
    /// Mode: 0 digital, 1 ADC.
    pub mode: u8,
    /// Samples in this frame (1–64).
    pub samples: ProbeSamples,
}

impl ProbeFrame {
    /// Encode to wire bytes.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(13 + self.samples.len() * 2);
        out.extend_from_slice(b"RSHP");
        out.push(VERSION);
        out.extend_from_slice(&self.seq.to_le_bytes());
        out.push(self.pin);
        out.push(self.mode);
        out.extend_from_slice(
            &(u16::try_from(self.samples.len()).unwrap_or(u16::MAX)).to_le_bytes(),
        );
        match &self.samples {
            ProbeSamples::Digital(levels) => out.extend_from_slice(levels),
            ProbeSamples::Analog(values) => {
                for v in values {
                    out.extend_from_slice(&v.to_le_bytes());
                }
            }
        }
        let crc = crc16(&out);
        out.extend_from_slice(&crc.to_le_bytes());
        out
    }

    /// Parse a probe frame.
    ///
    /// # Errors
    ///
    /// Returns [`ProbeError`] for a short frame, bad magic, bad
    /// version/mode, a sample count that does not match the length,
    /// or a CRC mismatch.
    pub fn parse(data: &[u8]) -> Result<Self, ProbeError> {
        if data.len() < 13 {
            return Err(ProbeError::TooShort);
        }
        if &data[0..4] != b"RSHP" {
            return Err(ProbeError::BadMagic);
        }
        if data[4] != VERSION || data[8] > 1 {
            return Err(ProbeError::BadVersion);
        }
        let sample_count = u16::from_le_bytes([data[9], data[10]]) as usize;
        if sample_count == 0 || sample_count > MAX_PROBE_SAMPLES {
            return Err(ProbeError::BadLength);
        }
        let bytes_per_sample = if data[8] == 0 { 1 } else { 2 };
        if data.len() != 11 + sample_count * bytes_per_sample + 2 {
            return Err(ProbeError::BadLength);
        }
        if !verify_crc16_trailer(data) {
            return Err(ProbeError::BadCrc);
        }
        let samples = if data[8] == 0 {
            ProbeSamples::Digital(data[11..11 + sample_count].to_vec())
        } else {
            let mut values = Vec::with_capacity(sample_count);
            for i in 0..sample_count {
                let off = 11 + i * 2;
                values.push(u16::from_le_bytes([data[off], data[off + 1]]));
            }
            ProbeSamples::Analog(values)
        };
        Ok(Self {
            seq: u16::from_le_bytes([data[5], data[6]]),
            pin: data[7],
            mode: data[8],
            samples,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{ProbeFrame, ProbeSamples};

    #[test]
    fn probe_round_trip_both_modes() {
        let digital = ProbeFrame {
            seq: 3,
            pin: 13,
            mode: 0,
            samples: ProbeSamples::Digital(vec![1, 0, 1, 1]),
        };
        let bytes = digital.encode();
        assert_eq!(bytes.len(), 11 + 4 + 2);
        assert_eq!(ProbeFrame::parse(&bytes).expect("digital probe"), digital);

        let analog = ProbeFrame {
            seq: 4,
            pin: 34,
            mode: 1,
            samples: ProbeSamples::Analog(vec![2300, 2310, 2290]),
        };
        let bytes = analog.encode();
        assert_eq!(bytes.len(), 11 + 6 + 2);
        assert_eq!(ProbeFrame::parse(&bytes).expect("analog probe"), analog);
    }
}
