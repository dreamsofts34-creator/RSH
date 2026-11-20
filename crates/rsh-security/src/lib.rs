//! `rsh-security`: the keystore (Wi-Fi / OTA password only, protocol
//! v2.2), Ed25519 firmware-image verify, and log redaction. A password
//! never appears in plaintext in a file, a log, the UI, or an export.
//!
//! The session state machine that *uses* the keystore lives in
//! `rsh-transport` (`session`), next to the control client. The
//! Wi-Fi password entry (`board:<boardMac>:wifi`, service `RSH`) is the
//! `WebOTA` auth source (D8).

pub mod firmware_verify;
pub mod keystore;
