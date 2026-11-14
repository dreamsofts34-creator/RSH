//! Ed25519 firmware-image verification (SEC-C-06 / SEC-D-04, Phase E3).
//!
//! This is the app-side half of signed OTA: before any firmware upload the
//! image is validated here (the Tauri `validate_firmware_image` command
//! calls [`verify_firmware_image`]), and the board re-verifies on receipt.
//! The format authority is the C++ reference firmware:
//!
//! * `reference/firmware-v4.0.1/sign-image.py` — signed-image layout
//!   (lines 6–9, `SIG_LEN = 64`, `MAGIC = b"RSHS"`, `TRAILER_LEN = 68`):
//!   `[ESP32 app image][Ed25519 signature, 64 B][magic "RSHS", 4 B]`.
//!   The signature is `Ed25519(secret_key, SHA-256(image bytes))` —
//!   hash-then-sign over the image bytes *without* the trailer, so the
//!   signed message is the 32-byte SHA-256 digest itself (signed with
//!   `openssl pkeyutl -sign -rawin`, i.e. plain Ed25519 over the digest,
//!   NOT Ed25519ph).
//! * `reference/firmware-v4.0.1/ota_validate.h` — the board-side rules this
//!   mirrors: 68-byte trailer (`kOtaTrailerSize`, line 83), ESP image magic
//!   `0xE9` (`kEspImageMagic`), size bounds (`kOtaMinBytes = 65536`,
//!   `kOtaMaxBytes = 0x140000`), the version marker `"Version <b>"`
//!   (`kOtaVersionMarker`, line 117) scanned out of the image body, and
//!   anti-downgrade (image version < running version is rejected; equal is
//!   a reinstall and is allowed).
//! * `reference/firmware-v4.0.1/signing_pubkey.h` — the production public
//!   key embedded below (public key only; the private key is offline and
//!   never enters this repo).
//!
//! The image version lives **inside the image body** — there is no version
//! field in the trailer. The board embeds the chip
//! `"Version <b>" + FIRMWARE_VERSION` (`config.h:134`, `"4.0.1"`) in its
//! `WebOTA` page, and the scanner in `ota_validate.h` (`rsh_version_scan_*`,
//! lines 117–165) finds it: marker, then up to 8 non-digit bytes skipped,
//! then a run of up to 16 version characters, parsed strictly as `a.b.c`
//! (each part 1–3 digits, value ≤ 255). This module replicates that scan.
//! The marker is untrusted on its own (an attacker can write any version
//! string) — authenticity comes only from the Ed25519 signature, which is
//! why the signature is verified *before* the version is trusted.
//!
//! # What is never logged
//!
//! No key or signature material appears in an error message. Errors name
//! only the failed check and, for a downgrade, the two version triples.

use std::fmt;

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

/// Production release image-signing public key (Ed25519, 32 bytes).
///
/// Copied from `reference/firmware-v4.0.1/signing_pubkey.h`
/// (`kRshSigningPubkey`). Public key only — safe to embed. The matching
/// private key is offline (`~/workspace/projects/rsh/signing-keys/`, never
/// read, copied, or used by this code).
pub const FIRMWARE_SIGNING_PUBKEY: [u8; 32] = [
    0xba, 0x6f, 0x53, 0xe0, 0xad, 0xaf, 0x98, 0x29, 0x5c, 0x86, 0xfe, 0xf5, 0x81, 0x9e, 0x85, 0x02,
    0x8d, 0xef, 0x02, 0xb3, 0x20, 0xb8, 0x99, 0x55, 0xd9, 0xde, 0xe0, 0x9a, 0xfd, 0xce, 0x2c, 0xe7,
];

/// SHA-256 fingerprint (lowercase hex) of [`FIRMWARE_SIGNING_PUBKEY`],
/// as printed in `reference/firmware-v4.0.1/signing_pubkey.h`.
pub const FIRMWARE_SIGNING_PUBKEY_SHA256_FINGERPRINT: &str =
    "387764a4529337c437b233a148b48f9b2ffe3f3b5015e14c95737cb3a67a6411";

/// Ed25519 signature length in bytes (trailer part 1).
pub const FIRMWARE_SIGNATURE_LEN: usize = 64;

/// Trailer magic: the last 4 bytes of a signed image (`"RSHS"`).
pub const FIRMWARE_TRAILER_MAGIC: [u8; 4] = *b"RSHS";

/// Trailer length in bytes: 64-byte signature + 4-byte magic.
pub const FIRMWARE_IMAGE_TRAILER_LEN: usize = FIRMWARE_SIGNATURE_LEN + 4;

/// First byte of an ESP32 app image (`kEspImageMagic`, `ota_validate.h`).
pub const FIRMWARE_ESP_IMAGE_MAGIC: u8 = 0xE9;

/// Smallest acceptable image body (without trailer), in bytes
/// (`kOtaMinBytes`, `ota_validate.h` / `MIN_IMAGE`, `sign-image.py`).
pub const FIRMWARE_IMAGE_MIN_LEN: usize = 65_536;

/// Largest acceptable image body (without trailer), in bytes: the app
/// partition size (`kOtaMaxBytes = 0x140000`, `ota_validate.h`).
pub const FIRMWARE_IMAGE_MAX_LEN: usize = 0x140_000;

/// Version marker inside the image body (`kOtaVersionMarker`,
/// `ota_validate.h` line 117): `"Version <b>"` followed by `a.b.c`.
pub const FIRMWARE_VERSION_MARKER: &[u8] = b"Version <b>";

/// Maximum non-digit bytes skipped between the marker and the version
/// digits (`ota_validate.h`: skip up to 8 bytes).
const VERSION_MARKER_MAX_SKIP: usize = 8;

/// Maximum version characters scanned after the marker
/// (`ota_validate.h`: version 16 chars at most).
const VERSION_MAX_LEN: usize = 16;

/// A parsed firmware version triple: `(major, minor, patch)`.
pub type FirmwareVersion = (u8, u8, u8);

/// The parsed, verified contents of a signed firmware image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FirmwareImageInfo {
    /// Image version parsed from the `"Version <b>a.b.c"` marker in the
    /// image body (equal to or newer than the current version).
    pub version: FirmwareVersion,
    /// Image body size in bytes (without the 68-byte trailer).
    pub image_size: usize,
    /// Total signed-image size in bytes (body + trailer).
    pub total_size: usize,
    /// SHA-256 of the image body — the exact bytes the signature covers.
    pub image_sha256: [u8; 32],
}

/// Every way a signed firmware image can fail validation.
///
/// Messages name only the failed check (and, for a downgrade, the two
/// version triples) — never key or signature material.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FirmwareVerifyError {
    /// Fewer than 68 bytes: no trailer can be present at all.
    TruncatedTrailer,
    /// The last 4 bytes are not `"RSHS"` — the image is unsigned.
    BadTrailerMagic,
    /// Image body is smaller than [`FIRMWARE_IMAGE_MIN_LEN`].
    TooSmall,
    /// Image body is larger than [`FIRMWARE_IMAGE_MAX_LEN`].
    TooLarge,
    /// First image byte is not `0xE9` — not an ESP32 app image
    /// (e.g. a merged/factory image was supplied by mistake).
    BadEspMagic,
    /// The Ed25519 signature does not verify against the production
    /// public key (tampered image, wrong key, or corrupt signature).
    InvalidSignature,
    /// No `"Version <b>a.b.c"` marker was found in the image body, so
    /// the version — and therefore the downgrade check — is unknown.
    /// The board rejects this the same way (`webota.cpp`: "image mein
    /// version nahi mila").
    VersionNotFound,
    /// The image version is older than the running version.
    Downgrade {
        /// Version parsed from the image.
        image: FirmwareVersion,
        /// Running version it was compared against.
        current: FirmwareVersion,
    },
}

impl fmt::Display for FirmwareVerifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TruncatedTrailer => {
                write!(
                    f,
                    "firmware image is truncated: no 68-byte signature trailer"
                )
            }
            Self::BadTrailerMagic => {
                write!(
                    f,
                    "firmware image is not signed: trailer magic is not \"RSHS\""
                )
            }
            Self::TooSmall => write!(
                f,
                "firmware image is too small (minimum {FIRMWARE_IMAGE_MIN_LEN} bytes)"
            ),
            Self::TooLarge => write!(
                f,
                "firmware image is too large (maximum {FIRMWARE_IMAGE_MAX_LEN} bytes)"
            ),
            Self::BadEspMagic => {
                write!(
                    f,
                    "firmware image is not an ESP32 app image (first byte is not 0xE9)"
                )
            }
            Self::InvalidSignature => {
                write!(f, "firmware signature verification failed")
            }
            Self::VersionNotFound => {
                write!(f, "firmware image contains no version marker")
            }
            Self::Downgrade { image, current } => write!(
                f,
                "firmware downgrade rejected: image {}.{}.{} is older than running {}.{}.{}",
                image.0, image.1, image.2, current.0, current.1, current.2
            ),
        }
    }
}

impl std::error::Error for FirmwareVerifyError {}

/// SHA-256 fingerprint (lowercase hex) of the embedded production public
/// key. Must equal [`FIRMWARE_SIGNING_PUBKEY_SHA256_FINGERPRINT`]; the
/// test `production_pubkey_fingerprint_matches` asserts this, so a
/// corrupted or swapped key fails the suite instead of silently
/// verifying against the wrong key.
#[must_use]
pub fn signing_pubkey_fingerprint_hex() -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(FIRMWARE_SIGNING_PUBKEY);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

/// Verify a signed firmware image against the production release key.
///
/// `image` is the whole signed file (image body + 68-byte trailer) and
/// `current_version` is the running firmware version `(major, minor,
/// patch)`. Checks, in order: trailer present with `"RSHS"` magic, body
/// size bounds, ESP `0xE9` magic, Ed25519 signature over
/// `SHA-256(body)`, version marker present, and anti-downgrade (image
/// version must be equal to or newer than `current_version`; equal is a
/// reinstall and is allowed, matching the board rule in
/// `ota_validate.h`).
///
/// # Errors
///
/// Returns [`FirmwareVerifyError`] for any failed check; see its variants.
pub fn verify_firmware_image(
    image: &[u8],
    current_version: FirmwareVersion,
) -> Result<FirmwareImageInfo, FirmwareVerifyError> {
    verify_firmware_image_with_pubkey(image, current_version, &FIRMWARE_SIGNING_PUBKEY)
}

/// Verify a signed firmware image against an explicit public key.
///
/// Same checks as [`verify_firmware_image`], but the caller supplies the
/// key. Production code always uses [`verify_firmware_image`] (the
/// embedded release key); this entry point exists so tests can verify
/// images built with a throwaway keypair without ever touching the
/// production signing key, and so a future key-migration path can verify
/// against a second key.
///
/// # Errors
///
/// Returns [`FirmwareVerifyError`] for any failed check; see its variants.
pub fn verify_firmware_image_with_pubkey(
    image: &[u8],
    current_version: FirmwareVersion,
    pubkey: &[u8; 32],
) -> Result<FirmwareImageInfo, FirmwareVerifyError> {
    if image.len() < FIRMWARE_IMAGE_TRAILER_LEN {
        return Err(FirmwareVerifyError::TruncatedTrailer);
    }
    let (body, trailer) = image.split_at(image.len() - FIRMWARE_IMAGE_TRAILER_LEN);
    if trailer[FIRMWARE_SIGNATURE_LEN..] != FIRMWARE_TRAILER_MAGIC {
        return Err(FirmwareVerifyError::BadTrailerMagic);
    }
    if body.len() < FIRMWARE_IMAGE_MIN_LEN {
        return Err(FirmwareVerifyError::TooSmall);
    }
    if body.len() > FIRMWARE_IMAGE_MAX_LEN {
        return Err(FirmwareVerifyError::TooLarge);
    }
    if body.first() != Some(&FIRMWARE_ESP_IMAGE_MAGIC) {
        return Err(FirmwareVerifyError::BadEspMagic);
    }
    let signature = Signature::from_slice(&trailer[..FIRMWARE_SIGNATURE_LEN])
        .map_err(|_| FirmwareVerifyError::InvalidSignature)?;
    let digest: [u8; 32] = Sha256::digest(body).into();
    let verifying_key =
        VerifyingKey::from_bytes(pubkey).map_err(|_| FirmwareVerifyError::InvalidSignature)?;
    verifying_key
        .verify(&digest, &signature)
        .map_err(|_| FirmwareVerifyError::InvalidSignature)?;

    // The signature is valid, so the body is authentic — only now is the
    // version marker trusted (it is untrusted input before this point).
    let version = scan_image_version(body).ok_or(FirmwareVerifyError::VersionNotFound)?;
    if version < current_version {
        return Err(FirmwareVerifyError::Downgrade {
            image: version,
            current: current_version,
        });
    }
    Ok(FirmwareImageInfo {
        version,
        image_size: body.len(),
        total_size: image.len(),
        image_sha256: digest,
    })
}

/// Find the `"Version <b>a.b.c"` marker in the image body and parse the
/// version after it, replicating `rsh_version_scan_feed`
/// (`ota_validate.h` lines 117–165): after the marker, skip up to 8
/// non-digit bytes, then take a run of up to 16 digit/dot characters and
/// parse it strictly as `a.b.c`. A marker whose run does not parse is
/// skipped and scanning continues after it. Returns `None` when no
/// marker yields a valid version.
fn scan_image_version(body: &[u8]) -> Option<FirmwareVersion> {
    let mut search_from = 0;
    while let Some(rel) = find_subslice(&body[search_from..], FIRMWARE_VERSION_MARKER) {
        let after_marker = search_from + rel + FIRMWARE_VERSION_MARKER.len();
        // Skip up to 8 non-digit bytes (markup between marker and digits).
        let mut digits_at = after_marker;
        let skip_end = (after_marker + VERSION_MARKER_MAX_SKIP).min(body.len());
        while digits_at < skip_end && !body[digits_at].is_ascii_digit() {
            digits_at += 1;
        }
        if digits_at < body.len() && body[digits_at].is_ascii_digit() {
            let run_end = (digits_at + VERSION_MAX_LEN).min(body.len());
            let mut end = digits_at;
            while end < run_end && (body[end].is_ascii_digit() || body[end] == b'.') {
                end += 1;
            }
            if let Some(version) = parse_version(&body[digits_at..end]) {
                return Some(version);
            }
        }
        // This marker did not yield a version — keep scanning after it.
        search_from = after_marker;
    }
    None
}

/// Strict `a.b.c` parse, matching `rsh_parse_version` (`ota_validate.h`
/// lines 50–73): exactly three parts, each 1–3 digits, each value ≤ 255,
/// and the whole run consumed.
fn parse_version(run: &[u8]) -> Option<FirmwareVersion> {
    if run.is_empty() || run.len() > 11 {
        return None;
    }
    let mut parts = [0u16; 3];
    let mut part = 0usize;
    let mut digits = 0usize;
    for &byte in run {
        if byte.is_ascii_digit() {
            digits += 1;
            if digits > 3 {
                return None;
            }
            parts[part] = parts[part] * 10 + u16::from(byte - b'0');
            if parts[part] > 255 {
                return None;
            }
        } else if byte == b'.' {
            if digits == 0 || part >= 2 {
                return None;
            }
            part += 1;
            digits = 0;
        } else {
            return None;
        }
    }
    if part != 2 || digits == 0 {
        return None;
    }
    Some((
        u8::try_from(parts[0]).ok()?,
        u8::try_from(parts[1]).ok()?,
        u8::try_from(parts[2]).ok()?,
    ))
}

/// First index of `needle` in `haystack`, or `None`.
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    /// Fixed, non-secret seed for the throwaway test keypair. This key
    /// signs test images only; it is NOT the production signing key and
    /// must never be used to sign a real firmware image.
    const TEST_SEED: [u8; 32] = [
        0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
        0x00, 0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80, 0x90, 0xa0, 0xb0, 0xc0, 0xd0, 0xe0,
        0xf0, 0x0f,
    ];

    fn test_signing_key() -> SigningKey {
        SigningKey::from_bytes(&TEST_SEED)
    }

    fn test_pubkey() -> [u8; 32] {
        test_signing_key().verifying_key().to_bytes()
    }

    /// Build a signed test image: `body_len` bytes starting with `0xE9`,
    /// the version marker embedded at offset 1024, signed over
    /// `SHA-256(body)` with the throwaway test key.
    fn build_test_image(version: &str, body_len: usize) -> Vec<u8> {
        let mut body = vec![0u8; body_len];
        body[0] = FIRMWARE_ESP_IMAGE_MAGIC;
        let marker = format!("Version <b>{version}");
        // Small bodies (e.g. the too-small test) cannot hold the marker at
        // the usual offset — place it where it fits.
        let at = if body_len > 2048 { 1024 } else { 16 };
        body[at..at + marker.len()].copy_from_slice(marker.as_bytes());
        sign_body(&body)
    }

    /// Build a signed test image with an arbitrary body (caller controls
    /// the marker / magic bytes).
    fn build_custom_image(body: &[u8]) -> Vec<u8> {
        sign_body(body)
    }

    fn sign_body(body: &[u8]) -> Vec<u8> {
        let digest: [u8; 32] = Sha256::digest(body).into();
        let signature = test_signing_key().sign(&digest);
        let mut image = body.to_vec();
        image.extend_from_slice(&signature.to_bytes());
        image.extend_from_slice(&FIRMWARE_TRAILER_MAGIC);
        image
    }

    fn verify_test(
        image: &[u8],
        current: FirmwareVersion,
    ) -> Result<FirmwareImageInfo, FirmwareVerifyError> {
        verify_firmware_image_with_pubkey(image, current, &test_pubkey())
    }

    #[test]
    fn valid_image_verifies() {
        let image = build_test_image("4.0.1", FIRMWARE_IMAGE_MIN_LEN);
        let info = verify_test(&image, (4, 0, 0)).expect("valid image must verify");
        assert_eq!(info.version, (4, 0, 1));
        assert_eq!(info.image_size, FIRMWARE_IMAGE_MIN_LEN);
        assert_eq!(
            info.total_size,
            FIRMWARE_IMAGE_MIN_LEN + FIRMWARE_IMAGE_TRAILER_LEN
        );
        let expected_sha256: [u8; 32] = Sha256::digest(&image[..FIRMWARE_IMAGE_MIN_LEN]).into();
        assert_eq!(info.image_sha256, expected_sha256);
    }

    #[test]
    fn production_pubkey_fingerprint_matches() {
        assert_eq!(
            signing_pubkey_fingerprint_hex(),
            FIRMWARE_SIGNING_PUBKEY_SHA256_FINGERPRINT
        );
        assert_eq!(
            FIRMWARE_SIGNING_PUBKEY_SHA256_FINGERPRINT,
            "387764a4529337c437b233a148b48f9b2ffe3f3b5015e14c95737cb3a67a6411"
        );
    }

    #[test]
    fn tampered_body_byte_fails_signature() {
        let mut image = build_test_image("4.0.1", FIRMWARE_IMAGE_MIN_LEN);
        // Flip a body byte far from the version marker: the digest changes,
        // so the signature must fail (not the version parse).
        image[2000] ^= 0x01;
        assert_eq!(
            verify_test(&image, (4, 0, 0)),
            Err(FirmwareVerifyError::InvalidSignature)
        );
    }

    #[test]
    fn tampered_signature_fails() {
        let mut image = build_test_image("4.0.1", FIRMWARE_IMAGE_MIN_LEN);
        let sig_at = FIRMWARE_IMAGE_MIN_LEN + 10;
        image[sig_at] ^= 0x01;
        assert_eq!(
            verify_test(&image, (4, 0, 0)),
            Err(FirmwareVerifyError::InvalidSignature)
        );
    }

    #[test]
    fn bad_trailer_magic_fails() {
        let mut image = build_test_image("4.0.1", FIRMWARE_IMAGE_MIN_LEN);
        let magic_at = image.len() - 4;
        image[magic_at..].copy_from_slice(b"XXXX");
        assert_eq!(
            verify_test(&image, (4, 0, 0)),
            Err(FirmwareVerifyError::BadTrailerMagic)
        );
    }

    #[test]
    fn truncated_trailer_fails() {
        // Fewer than 68 bytes total: no trailer can exist.
        let image = build_test_image("4.0.1", FIRMWARE_IMAGE_MIN_LEN);
        assert_eq!(
            verify_test(&image[..10], (4, 0, 0)),
            Err(FirmwareVerifyError::TruncatedTrailer)
        );
        // Trailer cut short (magic gone): the magic check fires.
        assert_eq!(
            verify_test(&image[..image.len() - 10], (4, 0, 0)),
            Err(FirmwareVerifyError::BadTrailerMagic)
        );
    }

    #[test]
    fn too_small_body_fails() {
        let image = build_test_image("4.0.1", 1024);
        assert_eq!(
            verify_test(&image, (4, 0, 0)),
            Err(FirmwareVerifyError::TooSmall)
        );
    }

    #[test]
    fn too_large_body_fails() {
        let image = build_test_image("4.0.1", FIRMWARE_IMAGE_MAX_LEN + 1);
        assert_eq!(
            verify_test(&image, (4, 0, 0)),
            Err(FirmwareVerifyError::TooLarge)
        );
    }

    #[test]
    fn bad_esp_magic_fails() {
        let mut body = vec![0u8; FIRMWARE_IMAGE_MIN_LEN];
        body[0] = 0xE8;
        let marker = b"Version <b>4.0.1";
        body[1024..1024 + marker.len()].copy_from_slice(marker);
        let image = build_custom_image(&body);
        assert_eq!(
            verify_test(&image, (4, 0, 0)),
            Err(FirmwareVerifyError::BadEspMagic)
        );
    }

    #[test]
    fn version_not_found_fails() {
        let mut body = vec![0u8; FIRMWARE_IMAGE_MIN_LEN];
        body[0] = FIRMWARE_ESP_IMAGE_MAGIC;
        // No version marker anywhere in the body.
        let image = build_custom_image(&body);
        assert_eq!(
            verify_test(&image, (4, 0, 0)),
            Err(FirmwareVerifyError::VersionNotFound)
        );
    }

    #[test]
    fn downgrade_rejected_same_and_newer_accepted() {
        let older = build_test_image("4.0.0", FIRMWARE_IMAGE_MIN_LEN);
        assert_eq!(
            verify_test(&older, (4, 0, 1)),
            Err(FirmwareVerifyError::Downgrade {
                image: (4, 0, 0),
                current: (4, 0, 1),
            })
        );
        // Equal version = reinstall, allowed (board rule, ota_validate.h).
        let same = build_test_image("4.0.1", FIRMWARE_IMAGE_MIN_LEN);
        assert_eq!(
            verify_test(&same, (4, 0, 1))
                .expect("reinstall allowed")
                .version,
            (4, 0, 1)
        );
        // Newer version, allowed.
        let newer = build_test_image("4.0.2", FIRMWARE_IMAGE_MIN_LEN);
        assert_eq!(
            verify_test(&newer, (4, 0, 1))
                .expect("upgrade allowed")
                .version,
            (4, 0, 2)
        );
    }

    #[test]
    fn production_key_rejects_throwaway_signed_image() {
        // The production entry point must use the embedded release key,
        // not the test key: a throwaway-signed image fails the signature.
        let image = build_test_image("4.0.1", FIRMWARE_IMAGE_MIN_LEN);
        assert_eq!(
            verify_firmware_image(&image, (4, 0, 0)),
            Err(FirmwareVerifyError::InvalidSignature)
        );
    }

    #[test]
    fn version_marker_with_markup_gap_parses() {
        // Marker followed by closing markup before the digits, as in the
        // real WebOTA chip ("Version <b>4.0.1</b>"): the scanner skips
        // non-digit bytes (up to 8) after the marker.
        let mut body = vec![0u8; FIRMWARE_IMAGE_MIN_LEN];
        body[0] = FIRMWARE_ESP_IMAGE_MAGIC;
        let marker = b"Version <b></b>4.1.0";
        body[2048..2048 + marker.len()].copy_from_slice(marker);
        let image = build_custom_image(&body);
        assert_eq!(
            verify_test(&image, (4, 0, 0))
                .expect("markup gap must parse")
                .version,
            (4, 1, 0)
        );
    }
}
