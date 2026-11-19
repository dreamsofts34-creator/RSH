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
use std::sync::Mutex;
use std::time::SystemTime;

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

/// Cached bundled-image version (R2-20): [`bundled_firmware_version`]
/// used to `fs::read` the ~960 KB image on every call, and
/// `slow_poll` calls it ~1 Hz — a ~1 MB disk read per slow cycle
/// for a value that only changes when the image file changes. The
/// parsed version is cached keyed by the resolved path + the file's
/// metadata (mtime + length); a cache hit costs one `stat` and no
/// read. A replacement with identical mtime and length would read
/// stale — the flows that write a new image call
/// [`invalidate_bundled_firmware_version_cache`] after writing, so
/// that path never relies on the stat. A missing / unreadable file
/// is never cached: the next call retries the read.
struct BundledVersionEntry {
    path: PathBuf,
    modified: SystemTime,
    len: u64,
    version: Option<String>,
}

static BUNDLED_VERSION_CACHE: Mutex<Option<BundledVersionEntry>> = Mutex::new(None);

/// Drop the cached bundled version (R2-20): call after writing a
/// new bundled image (OTA download / installer refresh), so the
/// next read parses the fresh file even if its mtime and length
/// happen to match the old one.
pub fn invalidate_bundled_firmware_version_cache() {
    *BUNDLED_VERSION_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}

/// The bundled OTA image's version (`"a.b.c"`), or `None` when the
/// image is absent, unreadable, or carries no version marker. No
/// signature check happens here — see the module docs.
#[must_use]
pub fn bundled_firmware_version(data_dir: &Path) -> Option<String> {
    let path = bundled_firmware_path(data_dir)?;
    let metadata = std::fs::metadata(&path).ok()?;
    let modified = metadata.modified().ok()?;
    let len = metadata.len();
    let mut cache = BUNDLED_VERSION_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(entry) = cache.as_ref()
        && entry.path == path
        && entry.modified == modified
        && entry.len == len
    {
        return entry.version.clone();
    }
    let bytes = std::fs::read(&path).ok()?;
    // Cache the `None` too: a present-but-unparseable image must
    // not be re-read on every slow cycle either.
    let version = parse_firmware_version(&bytes)
        .map(|(major, minor, patch_num)| format!("{major}.{minor}.{patch_num}"));
    *cache = Some(BundledVersionEntry {
        path,
        modified,
        len,
        version: version.clone(),
    });
    version
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
    use std::sync::Mutex;

    /// Serialises the cache-touching tests below: `bundled_firmware_version`
    /// shares one process-global entry and the harness runs tests in
    /// parallel, so a sibling test's refill / invalidate would otherwise
    /// evict the entry under test — green alone, flaky together.
    static CACHE_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn lock_cache_tests() -> std::sync::MutexGuard<'static, ()> {
        CACHE_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

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
        let _cache_guard = lock_cache_tests();
        let dir =
            std::env::temp_dir().join(format!("rsh-firmware-image-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(BUNDLED_FIRMWARE_FILE), image_with("4.0.2")).unwrap();
        assert_eq!(bundled_firmware_version(&dir).as_deref(), Some("4.0.2"));
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(bundled_firmware_version(&dir), None);
    }

    #[test]
    fn bundled_version_is_cached_until_the_file_changes() {
        let _cache_guard = lock_cache_tests();
        // R2-20: a second call with unchanged metadata must not
        // re-read the file — rewrite the bytes (same length, mtime
        // restored) and the cached version still answers.
        let dir = std::env::temp_dir().join(format!(
            "rsh-firmware-image-cache-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(BUNDLED_FIRMWARE_FILE);
        std::fs::write(&path, image_with("4.0.2")).unwrap();
        assert_eq!(bundled_firmware_version(&dir).as_deref(), Some("4.0.2"));
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::fs::write(&path, image_with("4.0.3")).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(modified))
            .unwrap();
        assert_eq!(
            bundled_firmware_version(&dir).as_deref(),
            Some("4.0.2"),
            "same mtime + length answers the cached version without re-reading"
        );
        // A real change (new mtime) re-reads the fresh version.
        std::fs::write(&path, image_with("4.0.3")).unwrap();
        assert_eq!(bundled_firmware_version(&dir).as_deref(), Some("4.0.3"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn bundled_version_invalidate_forces_a_fresh_read() {
        let _cache_guard = lock_cache_tests();
        // R2-20: the explicit invalidate covers a rewrite the stat
        // cannot see (same mtime + length).
        let dir = std::env::temp_dir().join(format!(
            "rsh-firmware-image-invalidate-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(BUNDLED_FIRMWARE_FILE);
        std::fs::write(&path, image_with("4.0.2")).unwrap();
        assert_eq!(bundled_firmware_version(&dir).as_deref(), Some("4.0.2"));
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::fs::write(&path, image_with("4.0.3")).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(modified))
            .unwrap();
        assert_eq!(bundled_firmware_version(&dir).as_deref(), Some("4.0.2"));
        invalidate_bundled_firmware_version_cache();
        assert_eq!(bundled_firmware_version(&dir).as_deref(), Some("4.0.3"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
