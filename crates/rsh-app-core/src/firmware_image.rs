//! Bundled OTA image version parsing (H3/M-8, v2.0.2).
//!
//! Reads the version out of the bundled signed firmware image
//! (`<data_dir>/firmware-signed.bin`) WITHOUT signature
//! verification: the "Check firmware" comparison only needs to know
//! which version the bundled image carries, and the OTA worker's
//! flash flow (which DOES verify) reuses this parser for its
//! pre-flight check. The scan mirrors
//! `rsh_security::firmware_verify::scan_image_version` (same marker,
//! same skip/run limits, same strict `a.b.c` parse) — it is
//! duplicated here because that function is private to the
//! signed-verification path.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

/// Version marker in the signed image body (`ota_validate.h`).
const VERSION_MARKER: &[u8] = b"Version <b>";

/// Maximum non-digit bytes skipped between the marker and the
/// version digits (`ota_validate.h`: skip up to 8 bytes).
const VERSION_MARKER_MAX_SKIP: usize = 8;

/// Maximum version characters scanned after the marker
/// (`ota_validate.h`: version 16 chars at most).
const VERSION_MAX_LEN: usize = 16;

/// File name of the bundled signed OTA image inside the data dir.
pub const BUNDLED_FIRMWARE_FILE: &str = "firmware-signed.bin";

/// Parse a firmware version triple out of raw image bytes: find the
/// `Version <b>` marker, skip up to 8 non-digit bytes, then take a
/// run of at most 16 digit/dot characters and parse it strictly as
/// `a.b.c` (each part 1–3 digits, value ≤ 255). Later markers are
/// tried when an earlier one yields no version. `None` when no
/// marker yields a version.
#[must_use]
pub fn parse_firmware_version(bytes: &[u8]) -> Option<(u8, u8, u8)> {
    let mut search_from = 0;
    while let Some(rel) = find_subslice(&bytes[search_from..], VERSION_MARKER) {
        let after_marker = search_from + rel + VERSION_MARKER.len();
        let mut digits_at = after_marker;
        let skip_end = (after_marker + VERSION_MARKER_MAX_SKIP).min(bytes.len());
        while digits_at < skip_end && !bytes[digits_at].is_ascii_digit() {
            digits_at += 1;
        }
        if digits_at < bytes.len() && bytes[digits_at].is_ascii_digit() {
            let run_end = (digits_at + VERSION_MAX_LEN).min(bytes.len());
            let mut end = digits_at;
            while end < run_end && (bytes[end].is_ascii_digit() || bytes[end] == b'.') {
                end += 1;
            }
            if let Some(version) = parse_version(&bytes[digits_at..end]) {
                return Some(version);
            }
        }
        // This marker did not yield a version — keep scanning after it.
        search_from = after_marker;
    }
    None
}

/// Strict `a.b.c` parse (matches `rsh_parse_version`): exactly
/// three parts, each 1–3 digits, each value ≤ 255. A shorter run
/// ("4.0") is NOT a version — the old loop left the missing part at
/// 0 and accepted it; the count check below closes that.
fn parse_version(run: &[u8]) -> Option<(u8, u8, u8)> {
    let text = std::str::from_utf8(run).ok()?;
    let mut parts = text.split('.');
    let mut triple = [0u8; 3];
    let mut count = 0;
    for (i, part) in parts.by_ref().take(3).enumerate() {
        if part.is_empty() || part.len() > 3 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let value: u32 = part.parse().ok()?;
        if value > 255 {
            return None;
        }
        triple[i] = u8::try_from(value).ok()?;
        count = i + 1;
    }
    if count != 3 || parts.next().is_some() {
        return None;
    }
    Some((triple[0], triple[1], triple[2]))
}

/// Find the first occurrence of `needle` in `haystack`.
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Parse a strict `"a.b.c"` version string (each part 0..=255, no
/// extra parts, no surrounding text). The flash flow needs the
/// installed version as a tuple; the image scan returns one too, so
/// both sides share this parser instead of ad-hoc splits.
#[must_use]
pub fn parse_version_str(text: &str) -> Option<(u8, u8, u8)> {
    let mut parts = text.split('.');
    let major: u8 = parts.next()?.parse().ok()?;
    let minor: u8 = parts.next()?.parse().ok()?;
    let patch: u8 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// Where the bundled OTA image lives: `<data_dir>/firmware-signed.bin`
/// when the OTA flow downloaded it there, else the copy the installer
/// bundles beside the daemon executable. `None` when neither exists —
/// the caller reports "no bundled firmware image" instead of flashing
/// a path that is not there.
#[must_use]
pub fn bundled_firmware_path(data_dir: &Path) -> Option<PathBuf> {
    let in_data = data_dir.join(BUNDLED_FIRMWARE_FILE);
    if in_data.is_file() {
        return Some(in_data);
    }
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))?;
    let beside_exe = exe_dir.join(BUNDLED_FIRMWARE_FILE);
    if beside_exe.is_file() {
        return Some(beside_exe);
    }
    None
}

/// The bundled OTA image's version (`"a.b.c"`), or `None` when the
/// image is absent, unreadable, or carries no version marker. No
/// signature check happens here — see the module docs.
#[must_use]
pub fn bundled_firmware_version(data_dir: &Path) -> Option<String> {
    let path = bundled_firmware_path(data_dir)?;
    let bytes = std::fs::read(path).ok()?;
    let (major, minor, patch_num) = parse_firmware_version(&bytes)?;
    Some(format!("{major}.{minor}.{patch_num}"))
}

/// Compare two `"a.b.c"` version strings numerically (semver
/// order). `None` when either side does not parse.
#[must_use]
pub fn compare_versions(left: &str, right: &str) -> Option<Ordering> {
    let parse = |text: &str| -> Option<(u32, u32, u32)> {
        let mut parts = text.split('.');
        let major: u32 = parts.next()?.parse().ok()?;
        let minor: u32 = parts.next()?.parse().ok()?;
        let patch: u32 = parts.next()?.parse().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some((major, minor, patch))
    };
    Some(parse(left)?.cmp(&parse(right)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image_with(version_text: &str) -> Vec<u8> {
        let mut bytes = b"firmware body \x00\x01\x02".to_vec();
        bytes.extend_from_slice(b"Version <b>");
        bytes.extend_from_slice(version_text.as_bytes());
        bytes.extend_from_slice(b"</b> tail");
        bytes
    }

    #[test]
    fn parses_version_after_marker() {
        assert_eq!(
            parse_firmware_version(&image_with("4.0.2")),
            Some((4, 0, 2))
        );
    }

    #[test]
    fn skips_markup_between_marker_and_digits() {
        // "</span>" is 7 non-digit bytes — inside the 8-byte skip.
        assert_eq!(
            parse_firmware_version(&image_with("</span>4.1.0")),
            Some((4, 1, 0))
        );
    }

    #[test]
    fn no_marker_means_no_version() {
        assert_eq!(parse_firmware_version(b"no version here"), None);
    }

    #[test]
    fn bad_versions_are_rejected() {
        assert_eq!(parse_firmware_version(&image_with("4.0")), None);
        assert_eq!(parse_firmware_version(&image_with("4.0.2.1")), None);
        assert_eq!(parse_firmware_version(&image_with("4.0.999")), None);
        assert_eq!(parse_firmware_version(&image_with("4.0.x")), None);
    }

    #[test]
    fn compare_versions_is_numeric_not_lexical() {
        assert_eq!(compare_versions("4.0.2", "4.0.10"), Some(Ordering::Less));
        assert_eq!(compare_versions("4.0.2", "4.0.2"), Some(Ordering::Equal));
        assert_eq!(compare_versions("4.10.0", "4.2.0"), Some(Ordering::Greater));
        assert_eq!(compare_versions("4.0.2", "bogus"), None);
    }

    #[test]
    fn parse_version_str_strict() {
        assert_eq!(parse_version_str("4.0.2"), Some((4, 0, 2)));
        assert_eq!(parse_version_str("255.255.255"), Some((255, 255, 255)));
        assert_eq!(parse_version_str("4.0"), None);
        assert_eq!(parse_version_str("4.0.2.1"), None);
        assert_eq!(parse_version_str("4.0.256"), None);
        assert_eq!(parse_version_str("v4.0.2"), None);
        assert_eq!(parse_version_str(""), None);
    }

    #[test]
    fn bundled_version_reads_the_file() {
        let dir =
            std::env::temp_dir().join(format!("rsh-firmware-image-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(BUNDLED_FIRMWARE_FILE), image_with("4.0.2")).unwrap();
        assert_eq!(bundled_firmware_version(&dir).as_deref(), Some("4.0.2"));
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(bundled_firmware_version(&dir), None);
    }
}
