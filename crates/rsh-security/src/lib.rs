//! `rsh-security`: the keystore (per-device 32-byte key, nonce/counter
//! session), Ed25519 firmware-image verify, and log redaction. A key or
//! password never appears in plaintext in a file, a log, the UI, or an export.
//!
//! The pairing/session state machine that *uses* the keystore lives in
//! `rsh-transport` (`session`), next to the control client. The key lifecycle
//! design itself is in `docs/KEY-LIFECYCLE-RUST.md` (Phase A5).

pub mod firmware_verify;
pub mod keystore;
