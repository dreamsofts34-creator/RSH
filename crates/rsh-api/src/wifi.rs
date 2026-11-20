//! Wi-Fi / password helpers (no-pairing redesign, protocol v2.2).
//!
//! The board's hotspot SSID is `RSH-XXXX` (D4): the last 4 hex
//! digits of the board Wi-Fi MAC, uppercase, fixed per board —
//! e.g. board `20:9B:A9:70:9A:55` → `RSH-9A55`. The SSID never
//! changes on `KEY_RESET` or password-set. The daemon derives the
//! same SSID from the HELLO `board_mac`; the firmware derives it
//! from its own Wi-Fi MAC.
//!
//! Also home to [`KEY_RESET_HOLD_SECS`]: holding BOOT ≥ 10 s
//! clears the board-side Wi-Fi password in firmware and reboots
//! into an open AP (D7). There is no wire `KEY_RESET` opcode and no
//! armed window anymore.

/// How long the user must hold BOOT to clear the board-side Wi-Fi
/// password (D7): 10 s, with LED indication during the hold; the
/// clear happens at the threshold and the board reboots to open AP.
pub const KEY_RESET_HOLD_SECS: u64 = 10;

/// Derive the board hotspot SSID from a board MAC (`"AA:BB:CC:DD:EE:FF"`
/// or `"AABBCCDDEEFF"` — separators and case are ignored).
/// Returns `None` when the MAC has fewer than 4 hex digits.
#[must_use]
pub fn ssid_for_board_mac(mac: &str) -> Option<String> {
    let hex: String = mac.chars().filter(char::is_ascii_hexdigit).collect();
    if hex.len() < 4 {
        return None;
    }
    let suffix = hex[hex.len() - 4..].to_ascii_uppercase();
    Some(format!("RSH-{suffix}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssid_derives_last_four_hex_uppercase() {
        assert_eq!(
            ssid_for_board_mac("20:9B:A9:70:9A:55").as_deref(),
            Some("RSH-9A55")
        );
        assert_eq!(
            ssid_for_board_mac("aa:bb:cc:dd:ee:ff").as_deref(),
            Some("RSH-EEFF")
        );
        assert_eq!(
            ssid_for_board_mac("AABBCCDDEEFF").as_deref(),
            Some("RSH-EEFF")
        );
    }

    #[test]
    fn ssid_rejects_short_mac() {
        assert_eq!(ssid_for_board_mac("AA:B"), None);
        assert_eq!(ssid_for_board_mac(""), None);
    }
}
