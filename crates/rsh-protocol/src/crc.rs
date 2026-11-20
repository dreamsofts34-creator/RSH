//! CRC16-CCITT-FALSE and CRC-32/ISO-HDLC, table-driven.
//!
//! CRC16: poly `0x1021`, init `0xFFFF`, no reflection, xorout `0x0000`.
//! Check value: `crc16(b"123456789") == 0x29B1` (PROTOCOL-V2.2.md §0).
//! CRC16 is always last in a frame and covers every byte before it.
//!
//! CRC32: poly `0xEDB88320` (reflected), init `0xFFFFFFFF`,
//! xorout `0xFFFFFFFF` — the config-blob trailer (§3.6).

/// CRC-16/CCITT-FALSE lookup table, generated at compile time.
const CRC16_TABLE: [u16; 256] = {
    let mut table = [0u16; 256];
    let mut i: u16 = 0;
    while i < 256 {
        let mut crc = i << 8;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
            bit += 1;
        }
        table[i as usize] = crc;
        i += 1;
    }
    table
};

/// CRC-32/ISO-HDLC lookup table (reflected poly `0xEDB88320`).
const CRC32_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i: u32 = 0;
    while i < 256 {
        let mut crc = i;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
            bit += 1;
        }
        table[i as usize] = crc;
        i += 1;
    }
    table
};

/// Compute CRC-16/CCITT-FALSE over `data`.
///
/// # Examples
///
/// ```
/// assert_eq!(rsh_protocol::crc16(b"123456789"), 0x29B1);
/// ```
#[must_use]
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in data {
        let idx = ((crc >> 8) ^ u16::from(b)) as usize & 0xFF;
        crc = (crc << 8) ^ CRC16_TABLE[idx];
    }
    crc
}

/// Compute CRC-32/ISO-HDLC over `data` (config-blob trailer, §3.6).
#[must_use]
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        let idx = ((crc ^ u32::from(b)) & 0xFF) as usize;
        crc = (crc >> 8) ^ CRC32_TABLE[idx];
    }
    crc ^ 0xFFFF_FFFF
}

/// Verify that the trailing little-endian CRC16 of `frame` matches the
/// bytes before it. Returns `false` for frames shorter than 2 bytes.
#[must_use]
pub fn verify_crc16_trailer(frame: &[u8]) -> bool {
    if frame.len() < 2 {
        return false;
    }
    let (body, trailer) = frame.split_at(frame.len() - 2);
    crc16(body) == u16::from_le_bytes([trailer[0], trailer[1]])
}

/// Append the little-endian CRC16 of `body` to a copy of `body`.
#[must_use]
pub fn with_crc16(body: &[u8]) -> Vec<u8> {
    let mut out = body.to_vec();
    out.extend_from_slice(&crc16(body).to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::{crc16, crc32};

    #[test]
    fn crc16_check_value() {
        assert_eq!(crc16(b"123456789"), 0x29B1);
    }

    #[test]
    fn crc16_empty_is_init() {
        assert_eq!(crc16(b""), 0xFFFF);
    }

    #[test]
    fn crc16_v1_golden_packet() {
        // Appendix A golden: a5 01 34 12 ... CRC 0xBD75 over the first 16 B.
        let v1: [u8; 18] = [
            0xa5, 0x01, 0x34, 0x12, 0x04, 0x03, 0x02, 0x01, 0xfc, 0x08, 0xe8, 0x03, 0xd0, 0x07,
            0x05, 0x01, 0x75, 0xbd,
        ];
        assert_eq!(crc16(&v1[..16]), 0xBD75);
    }

    #[test]
    fn crc32_known_value() {
        // CRC-32/ISO-HDLC check value for "123456789" is 0xCBF43926.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn crc32_empty() {
        assert_eq!(crc32(b""), 0);
    }
}
