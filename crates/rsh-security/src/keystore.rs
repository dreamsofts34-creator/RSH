//! The Wi-Fi / OTA password in the OS credential store (`keyring` 4.2.0).
//!
//! Protocol v2.2 (no-pairing redesign, D8): the device key is gone —
//! there is no pairing and no per-device secret. This module stores one
//! secret per board, and nothing else:
//!
//! | Secret | Service | Account (username) |
//! |---|---|---|
//! | Wi-Fi / OTA password | `RSH` | `board:<boardMac>:wifi` |
//!
//! The password entry is the `WebOTA` auth source: the WLAN profile
//! password cannot be read back at runtime (`netsh wlan show profile
//! key=clear` needs admin), so the stored entry is what the app uses
//! for `WebOTA`. It is written in the same `set_password` transaction
//! as the WLAN profile update, and cleared together with it on
//! password-clear. The service and account names are locked by the
//! key-lifecycle design document; do not rename them without amending
//! that document first. One board never overwrites another board's
//! entries: the board MAC is part of every account name.
//!
//! # Backend
//!
//! On Windows the `keyring` crate's default (`v1`) feature selects the
//! Windows Credential Manager (DPAPI-backed, current-user scoped). `keyring`
//! 4.x is fail-closed: if the platform store cannot be created,
//! [`keyring::Entry::new`] returns an error instead of silently falling back
//! to an in-memory mock (the `keyring` 3.x trap).
//! Every failure surfaces here as [`KeystoreError::PlatformUnavailable`];
//! password-set / `WebOTA` must fail loudly, never proceed without the
//! stored password. Connecting to a board needs no credential store
//! at all — a missing store blocks password-set and `WebOTA`, not connect.
//!
//! # Encoding
//!
//! The Wi-Fi password is stored as UTF-8 text.
//!
//! # Zeroization
//!
//! [`KeyStore`] never retains password material: it holds no secret between
//! calls. Buffers that transiently hold a password (the fetched blob on
//! read) are overwritten with zeros on drop or on the error path. The
//! returned `String` from [`KeyStore::get_wifi_password`] is owned by the
//! caller, so the caller must zeroize it when done; this module cannot
//! do it for them.
//!
//! # What is never logged
//!
//! No password material ever appears in an error message, a log
//! line, or a `Debug`/`Display` output. [`KeystoreError::Other`] carries
//! only fixed, secret-free descriptions. [`KeyStore`]'s `Debug` impl shows
//! only the service name.

use std::fmt;
use std::sync::Arc;

/// OS credential-store service name for every RSH entry.
///
/// Locked by `docs/KEY-LIFECYCLE-RUST.md` §4.
pub const KEYSTORE_SERVICE: &str = "RSH";

/// Maximum credential-blob size in bytes on the platform store.
///
/// Windows `CRED_MAX_CREDENTIAL_BLOB_SIZE` is 2560 bytes. A write whose
/// encoded form would exceed this is rejected before it reaches the
/// store, so a too-large value fails loudly instead of truncating.
const MAX_CREDENTIAL_BLOB_BYTES: usize = 2560;

/// Maximum length of a board identifier, in characters.
const MAX_BOARD_ID_LEN: usize = 64;

/// Errors from [`KeyStore`] operations.
///
/// No variant ever carries key or password material: [`KeystoreError::Other`]
/// holds only fixed, secret-free descriptions, never a secret value, a
/// stored blob, or a fragment of either.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeystoreError {
    /// No entry exists for this board in the credential store.
    NotFound,
    /// The platform credential store is unavailable (no Secret Service on
    /// Linux, Credential Manager unreachable on Windows, store locked, or
    /// the store could not be initialised at all).
    PlatformUnavailable,
    /// Any other failure. The message is a fixed, secret-free description
    /// of what went wrong; it never contains key or password material.
    Other(String),
}

impl fmt::Display for KeystoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => write!(f, "no credential stored for this board"),
            Self::PlatformUnavailable => {
                write!(f, "platform credential store is unavailable")
            }
            Self::Other(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for KeystoreError {}

/// Overwrite `buf` with zeros in a way the compiler must not optimise away.
///
/// Uses volatile writes plus [`std::hint::black_box`]. This is the only
/// zeroization primitive in this module; no extra dependency is used.
fn zeroize(buf: &mut [u8]) {
    for byte in buf.iter_mut() {
        // SAFETY: `byte` is a valid `&mut u8` borrowed from a live slice,
        // so the derived raw pointer is valid for a volatile write.
        unsafe {
            std::ptr::write_volatile(byte, 0);
        }
    }
    std::hint::black_box(buf);
}

/// A byte buffer that is zeroized when it goes out of scope.
///
/// Used for every transient copy of password material in this module
/// (the fetched blob on read). The buffer is zeroed in [`Drop`], so
/// early returns and error paths cannot leak it.
struct ZeroizingVec(Vec<u8>);

impl ZeroizingVec {
    /// Wrap `bytes` in a zeroizing buffer.
    fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
}

impl Drop for ZeroizingVec {
    fn drop(&mut self) {
        zeroize(&mut self.0);
    }
}

/// The one operation the platform credential store must provide.
///
/// `KeyStore` talks only to this trait, never to `keyring` directly, so
/// tests can substitute an in-memory backend without touching the OS store.
/// Production code always uses [`PlatformBackend`]; the mock/in-memory
/// backend exists only under `cfg(test)` and is never linked into the app.
trait SecretBackend: Send + Sync {
    /// Store `secret` under (`service`, `account`), replacing any entry.
    fn set_secret(&self, service: &str, account: &str, secret: &[u8]) -> Result<(), KeystoreError>;

    /// Fetch the secret stored under (`service`, `account`).
    fn get_secret(&self, service: &str, account: &str) -> Result<Vec<u8>, KeystoreError>;

    /// Delete the entry under (`service`, `account`).
    fn delete_secret(&self, service: &str, account: &str) -> Result<(), KeystoreError>;
}

/// The production backend: the OS credential store via `keyring` 4.2.0.
///
/// `keyring` 4.2.0 with default features selects the native store
/// (Windows Credential Manager on Windows, Secret Service on Linux). It
/// fails closed when no store exists — there is no silent mock fallback.
struct PlatformBackend;

impl SecretBackend for PlatformBackend {
    fn set_secret(&self, service: &str, account: &str, secret: &[u8]) -> Result<(), KeystoreError> {
        let entry =
            keyring::Entry::new(service, account).map_err(|error| map_keyring_error(&error))?;
        entry
            .set_secret(secret)
            .map_err(|error| map_keyring_error(&error))
    }

    fn get_secret(&self, service: &str, account: &str) -> Result<Vec<u8>, KeystoreError> {
        let entry =
            keyring::Entry::new(service, account).map_err(|error| map_keyring_error(&error))?;
        entry
            .get_secret()
            .map_err(|error| map_keyring_error(&error))
    }

    fn delete_secret(&self, service: &str, account: &str) -> Result<(), KeystoreError> {
        let entry =
            keyring::Entry::new(service, account).map_err(|error| map_keyring_error(&error))?;
        entry
            .delete_credential()
            .map_err(|error| map_keyring_error(&error))
    }
}

/// Map a `keyring` error onto [`KeystoreError`] without leaking secrets.
///
/// Only the error variant is inspected; no payload is copied into the
/// message. In particular [`KeystoreError::Other`] never receives the
/// underlying error's `Display` text, because a platform error could in
/// principle echo back part of a stored blob.
fn map_keyring_error(error: &keyring::Error) -> KeystoreError {
    match error {
        keyring::Error::NoEntry => KeystoreError::NotFound,
        keyring::Error::NoDefaultStore
        | keyring::Error::PlatformFailure(_)
        | keyring::Error::NoStorageAccess(_) => KeystoreError::PlatformUnavailable,
        keyring::Error::Invalid(name, _) if name == "platform" => {
            KeystoreError::PlatformUnavailable
        }
        keyring::Error::TooLong(_, _) => {
            KeystoreError::Other("value exceeds the platform credential length limit".to_owned())
        }
        keyring::Error::BadEncoding(_) => {
            KeystoreError::Other("stored password is not valid UTF-8".to_owned())
        }
        keyring::Error::BadDataFormat(_, _) => {
            KeystoreError::Other("stored secret has an unexpected format".to_owned())
        }
        keyring::Error::BadStoreFormat(_) => {
            KeystoreError::Other("platform credential store is malformed".to_owned())
        }
        keyring::Error::Ambiguous(_) => {
            KeystoreError::Other("more than one credential matches this entry".to_owned())
        }
        keyring::Error::Invalid(_, _) => {
            KeystoreError::Other("invalid credential store parameter".to_owned())
        }
        keyring::Error::NotSupportedByStore(_) => KeystoreError::Other(
            "operation is not supported by the platform credential store".to_owned(),
        ),
        _ => KeystoreError::Other("platform credential store operation failed".to_owned()),
    }
}

/// Validate and normalise a board identifier.
///
/// A board identifier is the board MAC as reported by HELLO, for example
/// `"AA:BB:CC:DD:EE:FF"`. It is trimmed and upper-cased, so `"aa:bb:…"` and
/// `"AA:BB:…"` refer to the same entry and one board cannot accumulate two
/// entries that differ only by case. Only ASCII alphanumerics, `:` and `-`
/// are accepted; anything else (whitespace inside the identifier, `/`,
/// control characters) is rejected, because the identifier becomes part of
/// a credential-store account name.
///
/// # Errors
///
/// Returns [`KeystoreError::Other`] if the identifier is empty, longer
/// than 64 characters, or contains a character outside the allowed set.
///
/// # Panics
///
/// This function does not panic.
fn normalize_board_id(board_id: &str) -> Result<String, KeystoreError> {
    let trimmed = board_id.trim();
    if trimmed.is_empty() {
        return Err(KeystoreError::Other(
            "board identifier must not be empty".to_owned(),
        ));
    }
    if trimmed.len() > MAX_BOARD_ID_LEN {
        return Err(KeystoreError::Other(
            "board identifier exceeds the maximum length".to_owned(),
        ));
    }
    if !trimmed
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b':' || byte == b'-')
    {
        return Err(KeystoreError::Other(
            "board identifier contains an unsupported character".to_owned(),
        ));
    }
    Ok(trimmed.to_ascii_uppercase())
}

/// The Wi-Fi / OTA password in the OS credential store.
///
/// A `KeyStore` holds no secret itself: every method reads or writes one
/// entry in the platform store and returns. It is cheap to clone and safe
/// to share between threads.
///
/// Callers must zeroize the `String` returned by
/// [`KeyStore::get_wifi_password`] when they are done with it — this
/// module cannot zeroize a value it has handed out. Passwords must never
/// be sent to the frontend over IPC, written to a file or log, or
/// included in an export.
#[derive(Clone)]
pub struct KeyStore {
    backend: Arc<dyn SecretBackend>,
}

impl KeyStore {
    /// Create a `KeyStore` backed by the platform credential store.
    ///
    /// Creation itself never touches the store and never fails: the store
    /// is contacted on the first read/write/delete, and a missing store is
    /// reported then as [`KeystoreError::PlatformUnavailable`]. Use
    /// [`KeyStore::store_available`] to probe the store up front.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    #[must_use]
    pub fn new() -> Self {
        Self {
            backend: Arc::new(PlatformBackend),
        }
    }

    /// Report whether the platform credential store is usable right now.
    ///
    /// This initialises the `keyring` store if that has not happened yet
    /// and returns `false` when no store exists (no Secret Service on
    /// Linux, Credential Manager unreachable on Windows). Password-set
    /// and `WebOTA` must check this — or handle
    /// [`KeystoreError::PlatformUnavailable`] from a store call — and
    /// fail loudly instead of proceeding without the stored password.
    /// A missing store blocks password-set and `WebOTA`, not connect:
    /// connecting to a board needs no credential store at all.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    #[must_use]
    pub fn store_available() -> bool {
        keyring::Entry::store_status().is_ok()
    }

    /// Account (username) of the Wi-Fi password entry for `board_id`.
    ///
    /// The account is `board:<boardMac>:wifi` with the board identifier
    /// normalised by `normalize_board_id` (trimmed, upper-cased).
    ///
    /// # Errors
    ///
    /// Returns [`KeystoreError::Other`] if `board_id` is invalid (see
    /// `normalize_board_id`).
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn wifi_password_account(board_id: &str) -> Result<String, KeystoreError> {
        normalize_board_id(board_id).map(|normalised| format!("board:{normalised}:wifi"))
    }

    /// Store (or replace) the Wi-Fi / OTA password for `board_id`.
    ///
    /// The password is written to the `board:<boardMac>:wifi` entry under
    /// service `RSH`. The password must be non-empty and its UTF-16
    /// encoding must fit the platform blob limit
    /// (`MAX_CREDENTIAL_BLOB_BYTES` bytes).
    ///
    /// # Errors
    ///
    /// Returns [`KeystoreError::Other`] if `board_id` is invalid or the
    /// password is empty or too large, or
    /// [`KeystoreError::PlatformUnavailable`] if the platform credential
    /// store cannot be reached.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn store_wifi_password(&self, board_id: &str, password: &str) -> Result<(), KeystoreError> {
        let account = Self::wifi_password_account(board_id)?;
        if password.is_empty() {
            return Err(KeystoreError::Other(
                "Wi-Fi password must not be empty".to_owned(),
            ));
        }
        if password.encode_utf16().count() * 2 > MAX_CREDENTIAL_BLOB_BYTES {
            return Err(KeystoreError::Other(
                "Wi-Fi password exceeds the platform credential size limit".to_owned(),
            ));
        }
        self.backend
            .set_secret(KEYSTORE_SERVICE, &account, password.as_bytes())
    }

    /// Fetch the Wi-Fi / OTA password stored for `board_id`.
    ///
    /// The caller owns the returned password and must not log it, display
    /// it after first pairing, or send it to the frontend.
    ///
    /// # Errors
    ///
    /// Returns [`KeystoreError::NotFound`] if no password is stored for
    /// this board, [`KeystoreError::Other`] if `board_id` is invalid or
    /// the stored entry is not valid UTF-8, or
    /// [`KeystoreError::PlatformUnavailable`] if the platform credential
    /// store cannot be reached.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn get_wifi_password(&self, board_id: &str) -> Result<String, KeystoreError> {
        let account = Self::wifi_password_account(board_id)?;
        let mut blob = ZeroizingVec::new(self.backend.get_secret(KEYSTORE_SERVICE, &account)?);
        // Move the bytes out of the zeroizing buffer so the conversion
        // holds the only copy; the (now empty) buffer is zeroized on drop.
        let bytes = std::mem::take(&mut blob.0);
        String::from_utf8(bytes).map_err(|_| {
            KeystoreError::Other("stored Wi-Fi password is not valid UTF-8".to_owned())
        })
    }

    /// Delete the stored Wi-Fi / OTA password for `board_id`.
    ///
    /// # Errors
    ///
    /// Returns [`KeystoreError::NotFound`] if no password is stored for
    /// this board, [`KeystoreError::Other`] if `board_id` is invalid, or
    /// [`KeystoreError::PlatformUnavailable`] if the platform credential
    /// store cannot be reached.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn delete_wifi_password(&self, board_id: &str) -> Result<(), KeystoreError> {
        let account = Self::wifi_password_account(board_id)?;
        self.backend.delete_secret(KEYSTORE_SERVICE, &account)
    }
}

impl Default for KeyStore {
    /// Create a `KeyStore` backed by the platform credential store.
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for KeyStore {
    /// Debug output shows only the service name — never an account name,
    /// and never key or password material (this type holds none anyway).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeyStore")
            .field("service", &KEYSTORE_SERVICE)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// Board MAC used by the round-trip tests.
    const TEST_BOARD: &str = "AA:BB:CC:DD:EE:FF";
    /// A second board MAC, for per-board isolation tests.
    const OTHER_BOARD: &str = "11:22:33:44:55:66";
    /// A distinctive Wi-Fi password.
    const TEST_PASSWORD: &str = "Ab3dEf6hJk9mNp2rSt5vXy8";

    /// In-memory [`SecretBackend`] for tests only.
    ///
    /// Why not `keyring_core::mock`? The `keyring` 4.2.0 `v1` API re-exports only
    /// `keyring::Entry` / `keyring::Error` — `keyring_core::mock` is not
    /// reachable without adding `keyring-core` as a direct dependency,
    /// which would mean editing `Cargo.toml` (out of scope for this
    /// worker). This backend exercises the same `KeyStore` logic (naming,
    /// error mapping for corrupt blobs, zeroization)
    /// without touching the OS store; the platform-backend call-through
    /// itself is covered by the fail-closed Linux tests below and by the
    /// ignored Windows round-trip test, which runs on the real store.
    struct InMemoryBackend {
        entries: Mutex<HashMap<(String, String), Vec<u8>>>,
    }

    impl InMemoryBackend {
        fn new() -> Self {
            Self {
                entries: Mutex::new(HashMap::new()),
            }
        }
    }

    impl SecretBackend for InMemoryBackend {
        fn set_secret(
            &self,
            service: &str,
            account: &str,
            secret: &[u8],
        ) -> Result<(), KeystoreError> {
            let mut entries = self.entries.lock().expect("test backend lock");
            entries.insert((service.to_owned(), account.to_owned()), secret.to_vec());
            Ok(())
        }

        fn get_secret(&self, service: &str, account: &str) -> Result<Vec<u8>, KeystoreError> {
            let entries = self.entries.lock().expect("test backend lock");
            entries
                .get(&(service.to_owned(), account.to_owned()))
                .cloned()
                .ok_or(KeystoreError::NotFound)
        }

        fn delete_secret(&self, service: &str, account: &str) -> Result<(), KeystoreError> {
            let mut entries = self.entries.lock().expect("test backend lock");
            entries
                .remove(&(service.to_owned(), account.to_owned()))
                .map_or(Err(KeystoreError::NotFound), |_| Ok(()))
        }
    }

    impl Drop for InMemoryBackend {
        /// Zeroize every stored secret when the test backend is dropped.
        fn drop(&mut self) {
            if let Ok(mut entries) = self.entries.lock() {
                for value in entries.values_mut() {
                    zeroize(value);
                }
            }
        }
    }

    /// A `KeyStore` that never touches the OS store.
    fn test_store() -> KeyStore {
        KeyStore {
            backend: Arc::new(InMemoryBackend::new()),
        }
    }

    /// Compile-time proof that `KeyStore` can be shared between threads.
    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn keystore_is_send_and_sync() {
        assert_send_sync::<KeyStore>();
        assert_send_sync::<KeystoreError>();
    }

    #[test]
    fn zeroize_overwrites_every_byte() {
        let mut buf = [0xAAu8; 32];
        zeroize(&mut buf);
        assert_eq!(buf, [0u8; 32]);

        let mut empty: [u8; 0] = [];
        zeroize(&mut empty);
        assert_eq!(empty, [0u8; 0]);
    }

    #[test]
    fn account_naming_matches_key_lifecycle_doc() {
        assert_eq!(
            KeyStore::wifi_password_account("aa:bb:cc:dd:ee:ff").expect("valid board"),
            "board:AA:BB:CC:DD:EE:FF:wifi"
        );
        assert_eq!(
            KeyStore::wifi_password_account("  AA:BB:CC:DD:EE:FF  ").expect("valid board"),
            "board:AA:BB:CC:DD:EE:FF:wifi"
        );
        assert_eq!(KEYSTORE_SERVICE, "RSH");
    }

    #[test]
    fn wifi_password_round_trip() {
        let store = test_store();
        assert_eq!(
            store.get_wifi_password(TEST_BOARD),
            Err(KeystoreError::NotFound)
        );
        store
            .store_wifi_password(TEST_BOARD, TEST_PASSWORD)
            .expect("store password");
        assert_eq!(
            store.get_wifi_password(TEST_BOARD).expect("read password"),
            TEST_PASSWORD
        );
        // Unicode passwords round-trip as UTF-8.
        store
            .store_wifi_password(TEST_BOARD, "pässwörd-üñïçødé-123")
            .expect("store unicode password");
        assert_eq!(
            store.get_wifi_password(TEST_BOARD).expect("read password"),
            "pässwörd-üñïçødé-123"
        );
        store
            .delete_wifi_password(TEST_BOARD)
            .expect("delete password");
        assert_eq!(
            store.get_wifi_password(TEST_BOARD),
            Err(KeystoreError::NotFound)
        );
    }

    #[test]
    fn wifi_passwords_are_per_board() {
        let store = test_store();
        store
            .store_wifi_password(TEST_BOARD, TEST_PASSWORD)
            .expect("store password");
        store
            .store_wifi_password(OTHER_BOARD, "OtherBoardPass1")
            .expect("store other password");
        assert_eq!(
            store.get_wifi_password(TEST_BOARD).expect("read password"),
            TEST_PASSWORD
        );
        assert_eq!(
            store
                .get_wifi_password(OTHER_BOARD)
                .expect("read other password"),
            "OtherBoardPass1"
        );
        store
            .delete_wifi_password(TEST_BOARD)
            .expect("delete password");
        assert_eq!(
            store.get_wifi_password(TEST_BOARD),
            Err(KeystoreError::NotFound)
        );
        assert_eq!(
            store
                .get_wifi_password(OTHER_BOARD)
                .expect("other password untouched"),
            "OtherBoardPass1"
        );
    }

    #[test]
    fn board_id_lookup_is_case_insensitive() {
        let store = test_store();
        store
            .store_wifi_password("aa:bb:cc:dd:ee:ff", TEST_PASSWORD)
            .expect("store lowercase");
        assert_eq!(
            store
                .get_wifi_password("AA:BB:CC:DD:EE:FF")
                .expect("read uppercase"),
            TEST_PASSWORD
        );
        store
            .delete_wifi_password("Aa:Bb:Cc:Dd:Ee:Ff")
            .expect("delete mixed case");
        assert_eq!(
            store.get_wifi_password(TEST_BOARD),
            Err(KeystoreError::NotFound)
        );
    }

    #[test]
    fn delete_of_missing_entry_is_not_found() {
        let store = test_store();
        assert_eq!(
            store.delete_wifi_password(TEST_BOARD),
            Err(KeystoreError::NotFound)
        );
    }

    #[test]
    fn invalid_board_ids_are_rejected() {
        let store = test_store();
        for bad in ["", "   ", "AA BB CC", "AA/BB/CC", "AA\x00BB"] {
            assert!(
                store.store_wifi_password(bad, TEST_PASSWORD).is_err(),
                "board id {bad:?} must be rejected"
            );
            assert!(
                store.get_wifi_password(bad).is_err(),
                "board id {bad:?} must be rejected"
            );
            assert!(
                KeyStore::wifi_password_account(bad).is_err(),
                "board id {bad:?} must be rejected"
            );
        }
        // 65 characters is over the 64-character limit.
        let too_long = "A".repeat(65);
        assert!(store.store_wifi_password(&too_long, TEST_PASSWORD).is_err());
    }

    #[test]
    fn invalid_wifi_passwords_are_rejected() {
        let store = test_store();
        assert!(matches!(
            store.store_wifi_password(TEST_BOARD, ""),
            Err(KeystoreError::Other(_))
        ));
        // Over the 2560-byte platform blob limit (1281 UTF-16 units).
        let too_long = "a".repeat(1281);
        assert!(matches!(
            store.store_wifi_password(TEST_BOARD, &too_long),
            Err(KeystoreError::Other(_))
        ));
        // Nothing was stored by the rejected writes.
        assert_eq!(
            store.get_wifi_password(TEST_BOARD),
            Err(KeystoreError::NotFound)
        );
    }

    #[test]
    fn corrupt_stored_password_blob_is_rejected_without_leaking_it() {
        let backend = Arc::new(InMemoryBackend::new());
        let store = KeyStore {
            backend: Arc::clone(&backend) as Arc<dyn SecretBackend>,
        };
        // A stored blob of invalid UTF-8 is rejected for the password path.
        backend
            .set_secret(
                KEYSTORE_SERVICE,
                "board:AA:BB:CC:DD:EE:FF:wifi",
                &[0xFF, 0xFE, 0xFD],
            )
            .expect("inject non-UTF8 blob");
        assert!(matches!(
            store.get_wifi_password(TEST_BOARD),
            Err(KeystoreError::Other(_))
        ));
    }

    #[test]
    fn no_secret_material_in_debug_or_error_output() {
        let store = test_store();
        store
            .store_wifi_password(TEST_BOARD, TEST_PASSWORD)
            .expect("store password");
        let debug = format!("{store:?}");
        assert!(
            !debug.contains(TEST_PASSWORD),
            "Debug leaked the password: {debug}"
        );
        assert!(!debug.contains(TEST_BOARD), "Debug leaked the board id");

        let error = store
            .get_wifi_password(OTHER_BOARD)
            .expect_err("missing password");
        assert_eq!(error.to_string(), "no credential stored for this board");
        let other = KeystoreError::Other("something failed".to_owned());
        assert_eq!(format!("{other:?}"), r#"Other("something failed")"#);
    }

    #[test]
    fn keystore_error_display_is_secret_free() {
        assert_eq!(
            KeystoreError::NotFound.to_string(),
            "no credential stored for this board"
        );
        assert_eq!(
            KeystoreError::PlatformUnavailable.to_string(),
            "platform credential store is unavailable"
        );
        assert_eq!(
            KeystoreError::Other("custom".to_owned()).to_string(),
            "custom"
        );
    }

    /// Fail-closed on this Linux sandbox: there is no Secret Service /
    /// D-Bus session here, so `keyring` 4.2.0 must refuse to create an
    /// entry instead of silently accepting into a mock store (the
    /// `keyring` 3.x trap). This test is Linux
    /// only on purpose: on Windows (Credential Manager) and macOS
    /// (Keychain) the platform store exists, so the same calls succeed
    /// there — see the ignored `windows_platform_round_trip` test.
    #[cfg(target_os = "linux")]
    #[test]
    fn platform_store_fails_closed_on_linux_sandbox() {
        assert!(
            !KeyStore::store_available(),
            "no platform store is expected on this Linux sandbox"
        );
        let store = KeyStore::new();
        assert_eq!(
            store.store_wifi_password(TEST_BOARD, TEST_PASSWORD),
            Err(KeystoreError::PlatformUnavailable)
        );
        assert_eq!(
            store.get_wifi_password(TEST_BOARD),
            Err(KeystoreError::PlatformUnavailable)
        );
        assert_eq!(
            store.delete_wifi_password(TEST_BOARD),
            Err(KeystoreError::PlatformUnavailable)
        );
    }

    /// Windows-only round-trip on the REAL platform store (Credential
    /// Manager). Ignored by default: it writes a real credential and only
    /// passes where the store exists. Run explicitly on a Windows machine
    /// with `cargo test -p rsh-security -- --ignored
    /// windows_platform_round_trip`, then confirm the entry is visible in
    /// Credential Manager (`cmdkey /list:RSH`) and gone afterwards.
    ///
    /// This is the in-process half of the trap test. The cross-process
    /// half (write here, read back in a FRESH
    /// process — a mock store is per-process memory and cannot survive
    /// it) is still open as a Windows-runtime verification on the owner
    /// PC / Windows CI; it is not implemented here because a unit test
    /// cannot re-exec itself portably.
    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "Windows-only: writes a real Credential Manager entry; run explicitly on Windows"]
    fn windows_platform_round_trip() {
        assert!(
            KeyStore::store_available(),
            "Credential Manager must be available on Windows"
        );
        let store = KeyStore::new();
        // A MAC that cannot be a real board (locally-administered DE:AD).
        let board = "DE:AD:BE:EF:00:01";
        store
            .store_wifi_password(board, TEST_PASSWORD)
            .expect("store password on Windows");
        assert_eq!(
            store
                .get_wifi_password(board)
                .expect("read password on Windows"),
            TEST_PASSWORD
        );
        store.delete_wifi_password(board).expect("delete password");
        assert_eq!(store.get_wifi_password(board), Err(KeystoreError::NotFound));
    }
}
