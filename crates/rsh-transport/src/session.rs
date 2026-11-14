//! Pairing + session state machine (Phase C4).
//!
//! [`BoardSession`] drives the board through its pairing lifecycle —
//! `Unpaired` → `Pairing` → `Paired` → `SessionActive` — on top of the
//! [`crate::control_client::ControlClient`] (Phase C3) and
//! the [`crate::udp::UdpTransport`] (Phase C1). It is the
//! app-side half of `docs/KEY-LIFECYCLE-RUST.md` §3/§5–§7:
//!
//! * Pairing only happens while the board reports itself unpaired **and**
//!   the 60 s pairing window is open. The disclosed device key + Wi-Fi
//!   password are stored in the OS credential store immediately, and a
//!   signed `GET_CAPS` is sent **first** (SEC-E-14: the board streams only
//!   after an HMAC-verified control command) before the session is marked
//!   active.
//! * Resuming loads the stored key and starts a **new** session with a
//!   **fresh** nonce (D30 — the nonce rotates only on reboot / new
//!   session); the counter restarts at 0. A frame signed under an old
//!   nonce fails with `BAD_TAG`.
//! * `KEY_RESET` erases the old key on the board and locally: the old key
//!   is invalid immediately, with no grace period.
//! * Re-pairing always uses a **new** key — the board discloses a fresh
//!   first-boot random 32-byte key per `HELLO`; a stored key is never
//!   reused for a re-pair.
//!
//! The keystore sits behind the small [`KeyStoreLike`] trait so tests can
//! substitute an in-memory store; production code passes the real
//! [`rsh_security::keystore::KeyStore`]. The platform store is
//! fail-closed: a [`KeystoreError::PlatformUnavailable`] aborts pairing
//! loudly — pairing never proceeds without a stored key.
//!
//! # Zeroization
//!
//! Every owned `[u8; 32]` key this module holds is overwritten with zeros
//! (volatile writes + [`std::hint::black_box`]) at session end and on
//! every error path that discards it. No key or password material ever
//! appears in a log line, an error message, or a `Debug` output.

use std::time::{Duration, Instant};

use rsh_protocol::hello::PAIRING_WINDOW_SECS;
use rsh_protocol::{GetCapsReply, HelloError, HelloReply};
use rsh_security::keystore::{KeyStore, KeystoreError};

use crate::ControlTransport;
use crate::control_client::{ControlClient, ControlError, Session};

/// Pairing window length, in seconds (protocol v2.1 §2.3: SoftAP-up +
/// 60 s).
pub const PAIRING_WINDOW: Duration = Duration::from_secs(PAIRING_WINDOW_SECS);

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

/// The one operation the pairing state machine needs from a key store.
///
/// The real [`rsh_security::keystore::KeyStore`] implements this
/// trait; tests substitute an in-memory store. The trait is deliberately
/// narrow — only the three device-key operations and the Wi-Fi password
/// store/delete used by pairing and `KEY_RESET` — so a substitute cannot
/// silently widen what pairing touches.
pub trait KeyStoreLike: Send {
    /// Store (or replace) the 32-byte device key for `board_id`.
    ///
    /// # Errors
    ///
    /// Returns [`KeystoreError`] when the board identifier is invalid or
    /// the platform credential store cannot be reached.
    fn store_device_key(&self, board_id: &str, key: &[u8; 32]) -> Result<(), KeystoreError>;

    /// Fetch the 32-byte device key stored for `board_id`.
    ///
    /// # Errors
    ///
    /// Returns [`KeystoreError::NotFound`] when no key is stored,
    /// [`KeystoreError::PlatformUnavailable`] when the platform store
    /// cannot be reached, or [`KeystoreError::Other`] for an invalid
    /// board identifier or a corrupt stored entry.
    fn get_device_key(&self, board_id: &str) -> Result<[u8; 32], KeystoreError>;

    /// Delete the stored device key for `board_id`.
    ///
    /// # Errors
    ///
    /// Returns [`KeystoreError::NotFound`] when no key is stored, or
    /// [`KeystoreError::PlatformUnavailable`] when the platform store
    /// cannot be reached.
    fn delete_device_key(&self, board_id: &str) -> Result<(), KeystoreError>;

    /// Store (or replace) the Wi-Fi / OTA password for `board_id`.
    ///
    /// # Errors
    ///
    /// Returns [`KeystoreError`] when the board identifier is invalid,
    /// the password is empty or too large, or the platform credential
    /// store cannot be reached.
    fn store_wifi_password(&self, board_id: &str, password: &str) -> Result<(), KeystoreError>;

    /// Delete the stored Wi-Fi / OTA password for `board_id`.
    ///
    /// # Errors
    ///
    /// Returns [`KeystoreError::NotFound`] when no password is stored, or
    /// [`KeystoreError::PlatformUnavailable`] when the platform store
    /// cannot be reached.
    fn delete_wifi_password(&self, board_id: &str) -> Result<(), KeystoreError>;
}

impl KeyStoreLike for KeyStore {
    fn store_device_key(&self, board_id: &str, key: &[u8; 32]) -> Result<(), KeystoreError> {
        KeyStore::store_device_key(self, board_id, key)
    }

    fn get_device_key(&self, board_id: &str) -> Result<[u8; 32], KeystoreError> {
        KeyStore::get_device_key(self, board_id)
    }

    fn delete_device_key(&self, board_id: &str) -> Result<(), KeystoreError> {
        KeyStore::delete_device_key(self, board_id)
    }

    fn store_wifi_password(&self, board_id: &str, password: &str) -> Result<(), KeystoreError> {
        KeyStore::store_wifi_password(self, board_id, password)
    }

    fn delete_wifi_password(&self, board_id: &str) -> Result<(), KeystoreError> {
        KeyStore::delete_wifi_password(self, board_id)
    }
}

/// Pairing + session state of one board.
///
/// The transitions are: `Unpaired` → `Pairing` (a `HELLO` opens the
/// pairing window and discloses credentials) → `Paired` (key stored, no
/// live session) → `SessionActive` (signed `GET_CAPS` accepted). The
/// machine returns to `Unpaired` on `KEY_RESET` and on pairing-window
/// expiry, and to `Paired` when a live session ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    /// No key is stored for this board; pairing has not started.
    Unpaired,
    /// The pairing window is open and credentials were disclosed, but
    /// pairing has not completed yet.
    Pairing,
    /// A device key is stored for this board, but no session is live.
    Paired,
    /// A live HMAC session exists: a signed `GET_CAPS` was accepted and
    /// the board may now emit the 200 Hz stream (SEC-E-14).
    SessionActive,
}

/// Errors from the pairing + session state machine.
///
/// No variant ever carries key or password material: messages are fixed,
/// secret-free descriptions.
#[derive(Debug)]
pub enum SessionError {
    /// The board is unpaired but the 60 s pairing window is closed.
    WindowClosed,
    /// The 60 s pairing window expired before pairing completed.
    WindowExpired,
    /// The board reports itself paired, so it discloses no key; pairing
    /// is the wrong operation (use [`BoardSession::resume`]).
    AlreadyPaired,
    /// The board reports itself unpaired, so there is no stored key to
    /// resume with (pair it instead).
    BoardUnpaired,
    /// The `HELLO` board MAC does not match this session's board.
    BoardMismatch,
    /// No device key is stored for this board.
    NoStoredKey,
    /// A device key is already stored for this board and the
    /// disclosed key differs (SEC-R2-06 board-binding gate): an evil
    /// twin inside the pairing window must not silently overwrite the
    /// stored key. The caller must get explicit user confirmation and
    /// retry via [`BoardSession::pair_with_replacement`]; nothing was
    /// stored and the state is unchanged.
    StoredKeyExists,
    /// The operation needs a live session, but none exists.
    NotActive,
    /// A live session already exists.
    AlreadyActive,
    /// The `HELLO` reply is structurally usable but its credential
    /// fields are unusable (all-zero key, non-UTF-8 password). The
    /// message is a fixed, secret-free description.
    InvalidHello(&'static str),
    /// The raw `HELLO` bytes failed to parse (length, magic, version,
    /// reserved byte, or CRC).
    HelloParse(HelloError),
    /// The platform credential store failed.
    Keystore(KeystoreError),
    /// A control command failed.
    Control(ControlError),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WindowClosed => write!(f, "pairing window is closed"),
            Self::WindowExpired => write!(f, "pairing window expired before pairing"),
            Self::AlreadyPaired => {
                write!(f, "board is already paired; pairing discloses no key")
            }
            Self::BoardUnpaired => {
                write!(
                    f,
                    "board is unpaired; there is no stored key to resume with"
                )
            }
            Self::BoardMismatch => {
                write!(f, "HELLO board MAC does not match this session's board")
            }
            Self::NoStoredKey => write!(f, "no device key stored for this board"),
            Self::StoredKeyExists => write!(
                f,
                "a device key is already stored for this board; re-pairing with a different key needs user confirmation"
            ),
            Self::NotActive => write!(f, "no live session for this board"),
            Self::AlreadyActive => write!(f, "a session is already active for this board"),
            Self::InvalidHello(msg) => write!(f, "HELLO credentials unusable: {msg}"),
            Self::HelloParse(e) => write!(f, "HELLO reply failed to parse: {e}"),
            Self::Keystore(e) => write!(f, "credential store error: {e}"),
            Self::Control(e) => write!(f, "control command failed: {e}"),
        }
    }
}

impl std::error::Error for SessionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::HelloParse(e) => Some(e),
            Self::Keystore(e) => Some(e),
            Self::Control(e) => Some(e),
            _ => None,
        }
    }
}

impl From<HelloError> for SessionError {
    fn from(e: HelloError) -> Self {
        Self::HelloParse(e)
    }
}

impl From<KeystoreError> for SessionError {
    fn from(e: KeystoreError) -> Self {
        Self::Keystore(e)
    }
}

impl From<ControlError> for SessionError {
    fn from(e: ControlError) -> Self {
        Self::Control(e)
    }
}

/// Credentials disclosed by one `HELLO` reply, held only until pairing
/// completes or the window expires.
///
/// The buffers are zeroized on drop, so an early return or an expired
/// window can never leave a disclosed key in memory.
struct PendingCredentials {
    /// Disclosed 32-byte device key.
    key: [u8; 32],
    /// Disclosed 24-byte Wi-Fi / OTA password.
    wifi_password: [u8; 24],
}

impl PendingCredentials {
    /// Zeroize both buffers now (the `Drop` impl does the same).
    fn zeroize(&mut self) {
        zeroize(&mut self.key);
        zeroize(&mut self.wifi_password);
    }
}

impl Drop for PendingCredentials {
    fn drop(&mut self) {
        self.zeroize();
    }
}

/// The pairing + session state machine for one board.
///
/// Generic over the key store (`K`) and the control transport (`T`), like
/// [`crate::control_client::ControlClient`] is generic over
/// the transport. One `BoardSession` owns one board's pairing lifecycle:
/// the transport is held idle until a session activates, then owned by
/// the control client until the session ends or `KEY_RESET` runs.
///
/// The `board_id` is the board MAC as reported by `HELLO` (for example
/// `"AA:BB:CC:DD:EE:FF"`); it is trimmed and upper-cased on construction,
/// matching the keystore's account naming.
pub struct BoardSession<K: KeyStoreLike, T: ControlTransport> {
    board_id: String,
    keystore: K,
    transport: Option<T>,
    client: Option<ControlClient<T>>,
    state: SessionState,
    pending: Option<PendingCredentials>,
    window_opened_at: Option<Instant>,
    window_nonce: Option<[u8; 16]>,
    expired_window_nonce: Option<[u8; 16]>,
    last_nonce: Option<[u8; 16]>,
    caps: Option<GetCapsReply>,
    control_timeout: Duration,
}

impl<K: KeyStoreLike, T: ControlTransport> BoardSession<K, T> {
    /// Create a session machine for `board_id`, with no key stored and no
    /// live session.
    ///
    /// The transport is held idle until [`BoardSession::pair`] or
    /// [`BoardSession::resume`] activates it. Creation never touches the
    /// credential store and never fails.
    #[must_use]
    pub fn new(board_id: &str, keystore: K, transport: T) -> Self {
        Self {
            board_id: board_id.trim().to_ascii_uppercase(),
            keystore,
            transport: Some(transport),
            client: None,
            state: SessionState::Unpaired,
            pending: None,
            window_opened_at: None,
            window_nonce: None,
            expired_window_nonce: None,
            last_nonce: None,
            caps: None,
            control_timeout: Duration::from_millis(250),
        }
    }

    /// Override the per-attempt control reply timeout (production keeps
    /// 250 ms; tests use a few ms so a full 3-attempt timeout stays fast).
    #[must_use]
    pub fn with_control_timeout(mut self, timeout: Duration) -> Self {
        self.control_timeout = timeout;
        self
    }

    /// The board this session belongs to (MAC, trimmed and upper-cased).
    #[must_use]
    pub fn board_id(&self) -> &str {
        &self.board_id
    }

    /// The current pairing / session state.
    #[must_use]
    pub fn state(&self) -> SessionState {
        self.state
    }

    /// `true` while a live HMAC session exists.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.state == SessionState::SessionActive
    }

    /// `true` once the board may emit the 200 Hz stream: a signed
    /// `GET_CAPS` was accepted this session (SEC-E-14). Identical to
    /// [`BoardSession::is_active`] — the stream gate *is* the active
    /// session.
    #[must_use]
    pub fn stream_allowed(&self) -> bool {
        self.is_active()
    }

    /// The `GET_CAPS` reply accepted when the session activated, if any.
    #[must_use]
    pub fn caps(&self) -> Option<&GetCapsReply> {
        self.caps.as_ref()
    }

    /// `true` while disclosed credentials are held pending pairing.
    #[must_use]
    pub fn has_pending_credentials(&self) -> bool {
        self.pending.is_some()
    }

    /// The live control client, if a session is active.
    #[must_use]
    pub fn control_client(&self) -> Option<&ControlClient<T>> {
        self.client.as_ref()
    }

    /// Mutable access to the live control client, if a session is active.
    #[must_use]
    pub fn control_client_mut(&mut self) -> Option<&mut ControlClient<T>> {
        self.client.as_mut()
    }

    /// Take the live control client out of the session temporarily
    /// (H5-control).
    ///
    /// Some clients must *own* the control client — notably the
    /// [`ConfigTransaction`](crate::config::ConfigTransaction), which
    /// drives stage → commit → confirm → rollback across several
    /// commands, each under its own fresh session. The transaction is
    /// built over the taken client, runs its step, and hands the client
    /// back via [`BoardSession::restore_control_client`] before the
    /// session closes, so the key is still zeroized exactly once, by
    /// [`BoardSession::close`]. Taking the client does not change the
    /// session state: the session stays active, and a session whose
    /// client was taken but never restored still ends safely — the
    /// transport is only handed back by [`BoardSession::close`] /
    /// [`BoardSession::take_transport`] while a client is present, so
    /// a caller that forgets the restore loses the transport instead
    /// of leaking a live session (fail-closed, never a half-open
    /// session). Returns [`None`] without a live session.
    #[must_use]
    pub fn take_control_client(&mut self) -> Option<ControlClient<T>> {
        self.client.take()
    }

    /// Hand a control client taken by
    /// [`BoardSession::take_control_client`] back to the session.
    /// Call only after a take, before [`BoardSession::close`]: the
    /// session must hold its client when it closes, so the transport
    /// is returned and the session key is zeroized exactly once.
    /// Restoring while a client is already present would drop that
    /// client without zeroizing its key, so this is a debug assertion
    /// in tests and a plain replace in release — the only caller (the
    /// H5-control helper) always takes first.
    pub fn restore_control_client(&mut self, client: ControlClient<T>) {
        debug_assert!(
            self.client.is_none(),
            "restore_control_client called while a client is live — the live client would be dropped without zeroizing its key"
        );
        self.client = Some(client);
    }

    /// The key store behind this session (tests inspect the stored key).
    #[must_use]
    pub fn keystore(&self) -> &K {
        &self.keystore
    }

    /// Record a `HELLO` reply: update the pairing-window tracking and,
    /// when the board discloses credentials, hold them pending pairing.
    ///
    /// A `HELLO` that discloses no credentials clears any pending
    /// credentials (they are no longer valid). A `HELLO` with a fresh
    /// nonce and an open window clears a latched expired window, so a
    /// new window can pair again.
    pub fn observe_hello(&mut self, hello: &HelloReply) {
        if hello.credentials_present() {
            if self.expired_window_nonce == Some(hello.nonce) {
                return;
            }
            if self.window_nonce != Some(hello.nonce) {
                self.expired_window_nonce = None;
                // M2: the window opens only on a NEW nonce. A repeat
                // HELLO for the same window must not refresh
                // `window_opened_at` — otherwise a board re-sending
                // HELLO every second would keep the 60 s window open
                // forever.
                self.window_opened_at = Some(Instant::now());
                self.window_nonce = Some(hello.nonce);
            }
            self.pending = Some(PendingCredentials {
                key: hello.device_key,
                wifi_password: hello.wifi_password,
            });
            if self.state == SessionState::Unpaired {
                self.state = SessionState::Pairing;
            }
        } else {
            self.discard_pending();
            if self.state == SessionState::Pairing {
                self.state = SessionState::Unpaired;
            }
        }
    }

    /// Expire the pairing window now: any held key/password material is
    /// zeroed and the state machine returns to `Unpaired`.
    ///
    /// Returns `true` when a window was open. Pairing is refused until a
    /// new `HELLO` opens a new window (a fresh nonce); a retry with the
    /// expired window's `HELLO` still fails with
    /// [`SessionError::WindowExpired`].
    pub fn expire_window(&mut self) -> bool {
        let had_window = self.window_opened_at.is_some();
        self.expired_window_nonce = self.window_nonce;
        self.window_opened_at = None;
        self.window_nonce = None;
        self.discard_pending();
        if self.state == SessionState::Pairing || self.state == SessionState::Unpaired {
            self.state = SessionState::Unpaired;
        }
        had_window
    }

    /// Expire the window if 60 s have passed since it opened.
    ///
    /// Returns `true` when the window just expired. Callers that poll
    /// (for example the 1 Hz `HELO` loop) use this to retire a stale
    /// window without waiting for the next `HELLO`.
    pub fn check_window_expiry(&mut self) -> bool {
        match self.window_opened_at {
            Some(opened_at) if opened_at.elapsed() >= PAIRING_WINDOW => self.expire_window(),
            _ => false,
        }
    }

    /// Pair with the board described by `hello`.
    ///
    /// The board must report itself unpaired with the pairing window open;
    /// otherwise nothing is stored and the matching error is returned
    /// ([`SessionError::WindowClosed`] or [`SessionError::AlreadyPaired`]).
    /// The disclosed key + Wi-Fi password are stored in the credential
    /// store, then a signed `GET_CAPS` is sent **first** (SEC-E-14) and
    /// only on its success is the session marked active. The returned
    /// value is that `GET_CAPS` reply.
    ///
    /// A credential-store failure fails loudly — the in-memory copies are
    /// zeroized, nothing is stored (a half-stored key is rolled back),
    /// and no control command is sent. A `GET_CAPS` failure leaves the
    /// stored key in place (the board disclosed it; a fresh `HELLO`
    /// inside the window may retry) and the state at `Paired`.
    ///
    /// Board binding (SEC-R2-06): when a **different** key is already
    /// stored for this board, pairing refuses with
    /// [`SessionError::StoredKeyExists`] and stores nothing — a foreign
    /// board (or an evil twin spoofing this board's MAC) cannot silently
    /// overwrite the stored key. The user must confirm the replacement
    /// explicitly (confirm dialog in the pairing UI) and the caller
    /// retries via [`BoardSession::pair_with_replacement`]. A stored key
    /// identical to the disclosed one is accepted (idempotent
    /// re-disclosure). After a proper `KEY_RESET` the local key is
    /// already deleted, so the normal re-pairing flow never hits this.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::WindowExpired`] for an expired window,
    /// [`SessionError::WindowClosed`] / [`SessionError::AlreadyPaired`]
    /// for a `HELLO` that discloses nothing, [`SessionError::BoardMismatch`]
    /// for a foreign board MAC, [`SessionError::InvalidHello`] for
    /// unusable credential fields, [`SessionError::AlreadyActive`] when a
    /// session is already live, [`SessionError::StoredKeyExists`] when a
    /// different key is already stored, [`SessionError::Keystore`] when the
    /// credential store fails, and [`SessionError::Control`] when the
    /// confirming `GET_CAPS` fails.
    pub async fn pair(&mut self, hello: &HelloReply) -> Result<GetCapsReply, SessionError> {
        self.pair_inner(hello, false).await
    }

    /// Pair with the board described by `hello`, replacing a stored
    /// key that differs from the disclosed one.
    ///
    /// This is the confirmed path behind [`SessionError::StoredKeyExists`]:
    /// call it only after the user explicitly confirmed the replacement
    /// (the pairing UI's confirm dialog). Everything else is identical
    /// to [`BoardSession::pair`].
    ///
    /// # Errors
    ///
    /// Same as [`BoardSession::pair`], except a differing stored key
    /// is replaced instead of refused.
    pub async fn pair_with_replacement(
        &mut self,
        hello: &HelloReply,
    ) -> Result<GetCapsReply, SessionError> {
        self.pair_inner(hello, true).await
    }

    async fn pair_inner(
        &mut self,
        hello: &HelloReply,
        allow_replacement: bool,
    ) -> Result<GetCapsReply, SessionError> {
        if self.client.is_some() {
            return Err(SessionError::AlreadyActive);
        }
        if self.window_expired_for(hello.nonce) {
            return Err(SessionError::WindowExpired);
        }
        if !hello.credentials_present() {
            if hello.unpaired() {
                return Err(SessionError::WindowClosed);
            }
            return Err(SessionError::AlreadyPaired);
        }
        if !self.board_mac_matches(hello) {
            return Err(SessionError::BoardMismatch);
        }
        if hello.device_key == [0u8; 32] {
            return Err(SessionError::InvalidHello(
                "disclosed device key is all zero",
            ));
        }
        let password = {
            // M3: the 24-byte field is NUL-padded — trim trailing
            // 0x00 before storing, so the stored password is the
            // password the board meant, not the password plus padding.
            // An empty / all-zero password is not a password at all.
            let end = hello
                .wifi_password
                .iter()
                .rposition(|&byte| byte != 0)
                .map_or(0, |last| last + 1);
            if end == 0 {
                return Err(SessionError::InvalidHello(
                    "disclosed Wi-Fi password is empty or all zero",
                ));
            }
            std::str::from_utf8(&hello.wifi_password[..end]).map_err(|_| {
                SessionError::InvalidHello("disclosed Wi-Fi password is not valid UTF-8")
            })?
        };
        // SEC-R2-06 board-binding gate: a stored key that differs from
        // the disclosed one is refused unless the caller confirmed the
        // replacement. Checked before anything is stored or any state
        // changes, so a refusal leaves store and state untouched.
        if !allow_replacement {
            match self.keystore.get_device_key(&self.board_id) {
                Ok(stored) => {
                    let mut stored = stored;
                    let differs = stored != hello.device_key;
                    zeroize(&mut stored);
                    if differs {
                        return Err(SessionError::StoredKeyExists);
                    }
                }
                Err(KeystoreError::NotFound) => {}
                Err(error) => return Err(SessionError::Keystore(error)),
            }
        }
        self.observe_hello(hello);
        let mut key = hello.device_key;
        if let Err(error) = self.keystore.store_device_key(&self.board_id, &key) {
            zeroize(&mut key);
            self.discard_pending();
            self.state = SessionState::Unpaired;
            return Err(SessionError::Keystore(error));
        }
        if let Err(error) = self.keystore.store_wifi_password(&self.board_id, password) {
            zeroize(&mut key);
            let _ = self.keystore.delete_device_key(&self.board_id);
            self.discard_pending();
            self.state = SessionState::Unpaired;
            return Err(SessionError::Keystore(error));
        }
        let session = Session::new(key, hello.nonce);
        zeroize(&mut key);
        match self.activate(session).await {
            Ok(caps) => {
                self.discard_pending();
                self.state = SessionState::SessionActive;
                self.last_nonce = Some(hello.nonce);
                Ok(caps)
            }
            Err(error) => {
                self.discard_pending();
                self.state = SessionState::Paired;
                Err(error)
            }
        }
    }

    /// Pair from raw 92-byte `HELLO` reply bytes (parsed first, then
    /// [`BoardSession::pair`]).
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::HelloParse`] for malformed bytes, plus the
    /// errors of [`BoardSession::pair`].
    pub async fn pair_bytes(&mut self, data: &[u8]) -> Result<GetCapsReply, SessionError> {
        let hello = HelloReply::parse(data)?;
        self.pair(&hello).await
    }

    /// Pair from raw 92-byte `HELLO` reply bytes with replacement
    /// (parsed first, then [`BoardSession::pair_with_replacement`]).
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::HelloParse`] for malformed bytes, plus the
    /// errors of [`BoardSession::pair_with_replacement`].
    pub async fn pair_bytes_with_replacement(
        &mut self,
        data: &[u8],
    ) -> Result<GetCapsReply, SessionError> {
        let hello = HelloReply::parse(data)?;
        self.pair_with_replacement(&hello).await
    }

    /// Resume a stored pairing: load the stored device key, start a new
    /// session with a **fresh** nonce from `hello` (D30 — the nonce
    /// rotates only on reboot / new session, so the caller sends `HELO`
    /// with `sessionFlag 1`), send a signed `GET_CAPS` first (SEC-E-14),
    /// and mark the session active.
    ///
    /// The board must report itself paired. A `HELLO` carrying a stale
    /// nonce fails here with [`ControlError::BadTag`] from the confirming
    /// `GET_CAPS` — the board's tag no longer verifies under the old
    /// nonce — and the state stays `Paired` with the in-memory key
    /// zeroized, so the caller can retry with a fresh `HELLO`.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::BoardUnpaired`] when the board reports
    /// itself unpaired, [`SessionError::BoardMismatch`] for a foreign
    /// board MAC, [`SessionError::NoStoredKey`] when no key is stored,
    /// [`SessionError::AlreadyActive`] when a session is already live,
    /// [`SessionError::Keystore`] when the credential store fails, and
    /// [`SessionError::Control`] when the confirming `GET_CAPS` fails
    /// (including `BadTag` for a stale nonce).
    pub async fn resume(&mut self, hello: &HelloReply) -> Result<GetCapsReply, SessionError> {
        if self.client.is_some() {
            return Err(SessionError::AlreadyActive);
        }
        if hello.unpaired() {
            return Err(SessionError::BoardUnpaired);
        }
        if !self.board_mac_matches(hello) {
            return Err(SessionError::BoardMismatch);
        }
        let mut key = match self.keystore.get_device_key(&self.board_id) {
            Ok(key) => key,
            Err(KeystoreError::NotFound) => return Err(SessionError::NoStoredKey),
            Err(error) => return Err(SessionError::Keystore(error)),
        };
        let session = Session::new(key, hello.nonce);
        zeroize(&mut key);
        match self.activate(session).await {
            Ok(caps) => {
                self.state = SessionState::SessionActive;
                self.last_nonce = Some(hello.nonce);
                Ok(caps)
            }
            Err(error) => {
                self.state = SessionState::Paired;
                Err(error)
            }
        }
    }

    /// Resume from raw 92-byte `HELLO` reply bytes (parsed first, then
    /// [`BoardSession::resume`]).
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::HelloParse`] for malformed bytes, plus the
    /// errors of [`BoardSession::resume`].
    pub async fn resume_bytes(&mut self, data: &[u8]) -> Result<GetCapsReply, SessionError> {
        let hello = HelloReply::parse(data)?;
        self.resume(&hello).await
    }

    /// Send `KEY_RESET` (only while a session is live — the board
    /// requires the armed state, BOOT held ≥ 10 s, 120 s window), then
    /// delete the local device key + Wi-Fi password, zeroize all
    /// in-memory key material, and return to `Unpaired`.
    ///
    /// The old key is invalid immediately after success: the board erased
    /// it and generated a new one, and this PC no longer holds it.
    /// Re-pairing (a fresh `HELLO` in the new 60 s window) is the only
    /// way back, and it always uses the new key.
    ///
    /// A failed `KEY_RESET` command leaves the session, the stored key,
    /// and the state untouched, so the caller can retry.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::NotActive`] without a live session,
    /// [`SessionError::Control`] when the board rejects `KEY_RESET`, and
    /// [`SessionError::Keystore`] when a local delete fails (the state is
    /// still `Unpaired` and the memory is still zeroized — the delete
    /// failure is reported loudly, not hidden).
    pub async fn key_reset(&mut self) -> Result<(), SessionError> {
        let Some(client) = self.client.as_mut() else {
            return Err(SessionError::NotActive);
        };
        client.key_reset().await?;
        let Some(client) = self.client.take() else {
            return Err(SessionError::NotActive);
        };
        let (transport, mut session) = client.into_inner();
        self.transport = Some(transport);
        zeroize(&mut session.key);
        self.caps = None;
        self.last_nonce = None;
        self.discard_pending();
        self.state = SessionState::Unpaired;
        let key_delete = self.keystore.delete_device_key(&self.board_id);
        let wifi_delete = self.keystore.delete_wifi_password(&self.board_id);
        Self::first_delete_error(key_delete, wifi_delete).map_err(SessionError::Keystore)
    }

    /// End the live session: zeroize the in-memory key, return the
    /// transport to idle, and fall back to `Paired` (the stored key
    /// survives, so [`BoardSession::resume`] can start a new session).
    /// A no-op without a live session (pending credentials are still
    /// discarded, and a half-open pairing window returns to `Unpaired`).
    pub fn close(&mut self) {
        if let Some(client) = self.client.take() {
            let (transport, mut session) = client.into_inner();
            self.transport = Some(transport);
            zeroize(&mut session.key);
            self.state = SessionState::Paired;
        } else if self.state == SessionState::Pairing {
            self.state = SessionState::Unpaired;
        }
        self.discard_pending();
    }

    /// Take the idle transport back out of the session machine (H5).
    ///
    /// After [`BoardSession::close`] — or after a failed
    /// [`BoardSession::pair`] / [`BoardSession::resume`], which already
    /// returned the transport to idle — the transport is idle again and
    /// the caller (the app's transport forwarder) needs it back to
    /// stream frames. The session itself does not survive the take:
    /// the in-memory key was zeroized by `close` (or never left the
    /// keystore), and a later control operation starts a fresh session
    /// with a fresh nonce ([`BoardSession::resume`]), exactly as D30
    /// requires. This is the release path that makes in-forwarder
    /// activation feasible without restructuring the transports
    /// (H1 §2 had rejected it for want of exactly this method: the
    /// session took the transport by value and never gave it back).
    ///
    /// Returns [`None`] when the transport is live inside a session
    /// (call `close` first) or was already taken.
    #[must_use]
    pub fn take_transport(&mut self) -> Option<T> {
        self.transport.take()
    }

    /// The nonce of the last active session, if any.
    #[must_use]
    pub fn last_nonce(&self) -> Option<[u8; 16]> {
        self.last_nonce
    }

    /// Build the control client for `session`, send the signed `GET_CAPS`
    /// first (SEC-E-14), and keep the client on success. On failure the
    /// transport is returned to idle and the session key is zeroized.
    async fn activate(&mut self, session: Session) -> Result<GetCapsReply, SessionError> {
        let Some(transport) = self.transport.take() else {
            return Err(SessionError::AlreadyActive);
        };
        let mut client = ControlClient::new(transport, session).with_timeout(self.control_timeout);
        match client.get_caps().await {
            Ok(caps) => {
                self.caps = Some(caps.clone());
                self.client = Some(client);
                Ok(caps)
            }
            Err(error) => {
                let (transport, mut session) = client.into_inner();
                self.transport = Some(transport);
                zeroize(&mut session.key);
                Err(SessionError::Control(error))
            }
        }
    }

    /// `true` when `hello`'s board MAC matches this session's board.
    fn board_mac_matches(&self, hello: &HelloReply) -> bool {
        let mac = hello
            .board_mac
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<Vec<String>>()
            .join(":");
        mac == self.board_id
    }

    /// Expire the window when `nonce` is the window's nonce and 60 s have
    /// passed since it opened. A different nonce means a new window: the
    /// stale one lapses silently (pending material is still discarded)
    /// and pairing may proceed.
    fn window_expired_for(&mut self, nonce: [u8; 16]) -> bool {
        match (self.window_opened_at, self.window_nonce) {
            (Some(opened_at), Some(window_nonce)) if window_nonce == nonce => {
                if opened_at.elapsed() >= PAIRING_WINDOW {
                    self.expire_window();
                    true
                } else {
                    false
                }
            }
            (Some(_), _) => {
                self.discard_pending();
                self.window_opened_at = None;
                self.window_nonce = None;
                false
            }
            _ => self
                .expired_window_nonce
                .is_some_and(|expired| expired == nonce),
        }
    }

    /// Drop any pending credentials (zeroized by their `Drop` impl) and
    /// clear the window-open tracking (L9 — a closed/discarded window
    /// must not leave a stale `window_opened_at`/`window_nonce`
    /// behind for a later HELLO to be measured against).
    fn discard_pending(&mut self) {
        self.pending = None;
        self.window_opened_at = None;
        self.window_nonce = None;
    }

    /// The first delete error that is not "already gone": a missing entry
    /// is a successful delete for pairing purposes.
    fn first_delete_error(
        first: Result<(), KeystoreError>,
        second: Result<(), KeystoreError>,
    ) -> Result<(), KeystoreError> {
        for result in [first, second] {
            match result {
                Ok(()) | Err(KeystoreError::NotFound) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

impl<K: KeyStoreLike, T: ControlTransport> std::fmt::Debug for BoardSession<K, T> {
    /// Debug output shows only the state — never the board id, and never
    /// key or password material (this type holds the key only inside the
    /// live control client, which is not shown either).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoardSession")
            .field("state", &self.state)
            .field("active", &self.client.is_some())
            .finish_non_exhaustive()
    }
}

impl<K: KeyStoreLike, T: ControlTransport> Drop for BoardSession<K, T> {
    /// Zeroize the live session key if the machine is dropped mid-session.
    /// The pending credentials zeroize themselves via their own `Drop`.
    fn drop(&mut self) {
        if let Some(client) = self.client.as_mut() {
            zeroize(&mut client.session_mut().key);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, VecDeque};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use rsh_protocol::control::{ErrorCode, Opcode, build_reply};
    use rsh_protocol::{ControlFrame, GetCapsReply, PinTableEntry};

    use super::{BoardSession, KeyStoreLike, KeystoreError, SessionError, SessionState, zeroize};
    use crate::control_client::{ControlClient, Session};
    use crate::{ControlTransport, TransportError};

    /// Board MAC used by the tests (and the matching board id).
    const BOARD_ID: &str = "AA:BB:CC:DD:EE:FF";
    /// Board MAC bytes.
    const BOARD_MAC: [u8; 6] = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
    /// First-boot device key disclosed by the board.
    const KEY_ONE: [u8; 32] = [0x11; 32];
    /// Fresh key disclosed after `KEY_RESET`.
    const KEY_TWO: [u8; 32] = [0x22; 32];
    /// First session nonce.
    const NONCE_ONE: [u8; 16] = [0xA1; 16];
    /// Fresh nonce after a reboot / new session.
    const NONCE_TWO: [u8; 16] = [0xA2; 16];
    /// Wi-Fi password disclosed on first pairing (24 ASCII bytes).
    const WIFI_PASSWORD: &[u8; 24] = b"Ab3dEf6hJk9mNp2rSt5vXy8z";

    /// In-memory [`KeyStoreLike`] for tests only (never touches the OS
    /// store). Can be put into a failing mode to prove pairing fails
    /// loudly when the platform store is unavailable.
    pub(crate) struct MemKeyStore {
        entries: Mutex<HashMap<(String, String), Vec<u8>>>,
        fail_with: Mutex<Option<KeystoreError>>,
    }

    impl MemKeyStore {
        fn new() -> Self {
            Self {
                entries: Mutex::new(HashMap::new()),
                fail_with: Mutex::new(None),
            }
        }

        fn failing(error: KeystoreError) -> Self {
            Self {
                entries: Mutex::new(HashMap::new()),
                fail_with: Mutex::new(Some(error)),
            }
        }

        fn fail(&self) -> Option<KeystoreError> {
            self.fail_with.lock().expect("test store lock").clone()
        }
    }

    impl KeyStoreLike for MemKeyStore {
        fn store_device_key(&self, board_id: &str, key: &[u8; 32]) -> Result<(), KeystoreError> {
            if let Some(error) = self.fail() {
                return Err(error);
            }
            let account = format!("board:{}", board_id.trim().to_ascii_uppercase());
            self.entries
                .lock()
                .expect("test store lock")
                .insert(("RSH".to_owned(), account), key.to_vec());
            Ok(())
        }

        fn get_device_key(&self, board_id: &str) -> Result<[u8; 32], KeystoreError> {
            if let Some(error) = self.fail() {
                return Err(error);
            }
            let account = format!("board:{}", board_id.trim().to_ascii_uppercase());
            let entries = self.entries.lock().expect("test store lock");
            let blob = entries
                .get(&("RSH".to_owned(), account))
                .ok_or(KeystoreError::NotFound)?;
            let mut key = [0u8; 32];
            key.copy_from_slice(blob);
            Ok(key)
        }

        fn delete_device_key(&self, board_id: &str) -> Result<(), KeystoreError> {
            if let Some(error) = self.fail() {
                return Err(error);
            }
            let account = format!("board:{}", board_id.trim().to_ascii_uppercase());
            self.entries
                .lock()
                .expect("test store lock")
                .remove(&("RSH".to_owned(), account))
                .map_or(Err(KeystoreError::NotFound), |_| Ok(()))
        }

        fn store_wifi_password(&self, board_id: &str, password: &str) -> Result<(), KeystoreError> {
            if let Some(error) = self.fail() {
                return Err(error);
            }
            let account = format!("board:{}:wifi", board_id.trim().to_ascii_uppercase());
            self.entries
                .lock()
                .expect("test store lock")
                .insert(("RSH".to_owned(), account), password.as_bytes().to_vec());
            Ok(())
        }

        fn delete_wifi_password(&self, board_id: &str) -> Result<(), KeystoreError> {
            if let Some(error) = self.fail() {
                return Err(error);
            }
            let account = format!("board:{}:wifi", board_id.trim().to_ascii_uppercase());
            self.entries
                .lock()
                .expect("test store lock")
                .remove(&("RSH".to_owned(), account))
                .map_or(Err(KeystoreError::NotFound), |_| Ok(()))
        }
    }

    impl Drop for MemKeyStore {
        fn drop(&mut self) {
            if let Ok(mut entries) = self.entries.lock() {
                for value in entries.values_mut() {
                    zeroize(value);
                }
            }
        }
    }

    /// A fake board: verifies request tags with its own key/nonce,
    /// answers `GET_CAPS` (and every other command) with a valid reply,
    /// records every frame sent, and rotates its key when `KEY_RESET`
    /// executes (the old key stops verifying immediately).
    struct FakeBoard {
        key: [u8; 32],
        nonce: [u8; 16],
        sent: Vec<Vec<u8>>,
        pending: VecDeque<Vec<u8>>,
        rotated_key: Option<[u8; 32]>,
        /// When `true`, every reply is dropped (forces client retries
        /// until the 3-attempt timeout).
        drop_all: bool,
    }

    impl FakeBoard {
        fn new(key: [u8; 32], nonce: [u8; 16]) -> Self {
            Self {
                key,
                nonce,
                sent: Vec::new(),
                pending: VecDeque::new(),
                rotated_key: None,
                drop_all: false,
            }
        }

        /// After the next `KEY_RESET`, the board expects `new_key`.
        fn rotate_key_on_reset(&mut self, new_key: [u8; 32]) {
            self.rotated_key = Some(new_key);
        }
    }

    impl ControlTransport for FakeBoard {
        fn send_control(
            &mut self,
            bytes: &[u8],
        ) -> impl Future<Output = Result<(), TransportError>> + Send {
            let owned = bytes.to_vec();
            async move {
                self.sent.push(owned.clone());
                let Ok(frame) = ControlFrame::parse_structure(&owned) else {
                    return Ok(());
                };
                // A tag that fails under the board's key/nonce is
                // answered BAD_TAG (tagged with the board's key/nonce,
                // so the client sees a tag failure too).
                if ControlFrame::verify(&owned, &self.key, &self.nonce).is_err() {
                    let reply = build_reply(
                        &self.key,
                        &self.nonce,
                        frame.cmd,
                        frame.req_id,
                        frame.counter,
                        ErrorCode::BadTag,
                        &[],
                    )
                    .expect("fake-board BAD_TAG reply builds");
                    self.pending.push_back(reply);
                    return Ok(());
                }
                if frame.cmd == Opcode::KeyReset as u8 {
                    // The reply is tagged with the OLD key (the session
                    // in flight); the key rotates only after the reply
                    // is built, so the next command needs the new key.
                    let reply = build_reply(
                        &self.key,
                        &self.nonce,
                        frame.cmd,
                        frame.req_id,
                        frame.counter,
                        ErrorCode::Ok,
                        &[],
                    )
                    .expect("fake-board reply builds");
                    if let Some(new_key) = self.rotated_key {
                        self.key = new_key;
                    }
                    if !self.drop_all {
                        self.pending.push_back(reply);
                    }
                    return Ok(());
                }
                let body = if frame.cmd == Opcode::GetCaps as u8 {
                    caps_body()
                } else {
                    Vec::new()
                };
                let reply = build_reply(
                    &self.key,
                    &self.nonce,
                    frame.cmd,
                    frame.req_id,
                    frame.counter,
                    ErrorCode::Ok,
                    &body,
                )
                .expect("fake-board reply builds");
                if self.drop_all {
                    // Reply dropped: the client retries, then times out.
                } else {
                    self.pending.push_back(reply);
                }
                Ok(())
            }
        }

        fn recv_control(
            &mut self,
            _timeout: Duration,
        ) -> impl Future<Output = Result<Vec<u8>, TransportError>> + Send {
            std::future::ready(self.pending.pop_front().ok_or(TransportError::Timeout))
        }
    }

    /// A `GET_CAPS` body *after* the status byte: fw 4.0.1, proto 2.1,
    /// 8 axes / 64 buttons / 4 encoders, Wi-Fi channel 6, one pin-table
    /// entry (pin 34). `encode_body` includes the status byte first, so
    /// it is stripped here.
    fn caps_body() -> Vec<u8> {
        let mut body = GetCapsReply {
            status: ErrorCode::Ok,
            board_mac: BOARD_MAC,
            firmware: (4, 0, 1),
            protocol: (2, 1),
            n_axes_max: 8,
            n_btn_max: 64,
            n_enc_max: 4,
            wifi_channel: 6,
            caps_flags: 0,
            pin_table: vec![PinTableEntry {
                pin: 34,
                kind: 1,
                module: 1,
                caps: 0x05,
            }],
        }
        .encode_body();
        body.remove(0);
        body
    }

    /// A `HELLO` reply. `flags` bit0 unpaired, bit3 window open.
    fn hello(
        flags: u8,
        nonce: [u8; 16],
        key: [u8; 32],
        wifi: [u8; 24],
    ) -> rsh_protocol::HelloReply {
        rsh_protocol::HelloReply {
            flags,
            session_echo: 1,
            board_mac: BOARD_MAC,
            firmware: (4, 0, 1),
            nonce,
            device_key: key,
            wifi_password: wifi,
        }
    }

    /// Unpaired board, window open, disclosing `key`.
    fn pairing_hello(key: [u8; 32], nonce: [u8; 16]) -> rsh_protocol::HelloReply {
        hello(0x09, nonce, key, *WIFI_PASSWORD)
    }

    /// Paired board (no credentials disclosed).
    fn paired_hello(nonce: [u8; 16]) -> rsh_protocol::HelloReply {
        hello(0x00, nonce, [0u8; 32], [0u8; 24])
    }

    /// Build a session machine with a 10 ms control timeout so a full
    /// 3-attempt timeout stays fast in tests.
    fn machine(keystore: MemKeyStore, board: FakeBoard) -> BoardSession<MemKeyStore, FakeBoard> {
        BoardSession::new(BOARD_ID, keystore, board).with_control_timeout(Duration::from_millis(10))
    }

    /// The fake board's recorded sends, via the live control client.
    fn sent_frames(session: &BoardSession<MemKeyStore, FakeBoard>) -> Vec<ControlFrame> {
        session
            .control_client()
            .expect("session must be active")
            .transport()
            .sent
            .iter()
            .map(|bytes| ControlFrame::parse_structure(bytes).expect("sent frame parses"))
            .collect()
    }

    #[tokio::test]
    async fn pair_happy_path_stores_key_and_sends_get_caps_first() {
        let store = MemKeyStore::new();
        let mut session = machine(store, FakeBoard::new(KEY_ONE, NONCE_ONE));
        assert_eq!(session.state(), SessionState::Unpaired);
        let caps = session
            .pair(&pairing_hello(KEY_ONE, NONCE_ONE))
            .await
            .expect("pairing succeeds");
        assert_eq!(caps.firmware, (4, 0, 1));
        assert_eq!(session.state(), SessionState::SessionActive);
        assert!(session.stream_allowed());
        assert!(session.is_active());
        assert_eq!(session.last_nonce(), Some(NONCE_ONE));
        // The key and the Wi-Fi password are stored.
        assert_eq!(
            session
                .keystore()
                .get_device_key(BOARD_ID)
                .expect("key stored"),
            KEY_ONE
        );
        // Signed GET_CAPS is the first (and here only) control command.
        let frames = sent_frames(&session);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].cmd, Opcode::GetCaps as u8);
        assert_eq!(frames[0].counter, 0);
        // The live session uses the disclosed key and the HELLO nonce.
        let live = session.control_client().expect("client live").session();
        assert_eq!(live.key, KEY_ONE);
        assert_eq!(live.nonce, NONCE_ONE);
        assert_eq!(live.counter, 1);
    }

    #[tokio::test]
    async fn pair_bytes_parses_hello_then_pairs() {
        let mut session = machine(MemKeyStore::new(), FakeBoard::new(KEY_ONE, NONCE_ONE));
        let bytes = pairing_hello(KEY_ONE, NONCE_ONE).encode();
        assert_eq!(bytes.len(), 92);
        let caps = session
            .pair_bytes(&bytes)
            .await
            .expect("pair_bytes succeeds");
        assert_eq!(caps.firmware, (4, 0, 1));
        assert_eq!(session.state(), SessionState::SessionActive);
    }

    #[tokio::test]
    async fn pair_bytes_rejects_malformed_hello() {
        let mut session = machine(MemKeyStore::new(), FakeBoard::new(KEY_ONE, NONCE_ONE));
        let err = session
            .pair_bytes(&[0u8; 10])
            .await
            .expect_err("10-byte HELLO must fail");
        assert!(matches!(err, SessionError::HelloParse(_)), "got {err:?}");
        assert_eq!(session.state(), SessionState::Unpaired);
        assert_eq!(
            session.keystore().get_device_key(BOARD_ID),
            Err(KeystoreError::NotFound)
        );
    }

    #[tokio::test]
    async fn pair_window_closed_refuses_and_stores_nothing() {
        let mut session = machine(MemKeyStore::new(), FakeBoard::new(KEY_ONE, NONCE_ONE));
        // Unpaired, window closed: credentials_present() is false.
        let closed = hello(0x01, NONCE_ONE, KEY_ONE, *WIFI_PASSWORD);
        let err = session
            .pair(&closed)
            .await
            .expect_err("closed window must fail");
        assert!(matches!(err, SessionError::WindowClosed), "got {err:?}");
        assert_eq!(session.state(), SessionState::Unpaired);
        assert_eq!(
            session.keystore().get_device_key(BOARD_ID),
            Err(KeystoreError::NotFound)
        );
    }

    #[tokio::test]
    async fn pair_already_paired_board_refuses_and_stores_nothing() {
        let mut session = machine(MemKeyStore::new(), FakeBoard::new(KEY_ONE, NONCE_ONE));
        let err = session
            .pair(&paired_hello(NONCE_ONE))
            .await
            .expect_err("paired board must fail");
        assert!(matches!(err, SessionError::AlreadyPaired), "got {err:?}");
        assert_eq!(session.state(), SessionState::Unpaired);
        assert_eq!(
            session.keystore().get_device_key(BOARD_ID),
            Err(KeystoreError::NotFound)
        );
    }

    #[tokio::test]
    async fn pair_wrong_board_mac_refuses() {
        let mut session = machine(MemKeyStore::new(), FakeBoard::new(KEY_ONE, NONCE_ONE));
        let mut foreign = pairing_hello(KEY_ONE, NONCE_ONE);
        foreign.board_mac = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66];
        let err = session
            .pair(&foreign)
            .await
            .expect_err("foreign MAC must fail");
        assert!(matches!(err, SessionError::BoardMismatch), "got {err:?}");
        assert_eq!(
            session.keystore().get_device_key(BOARD_ID),
            Err(KeystoreError::NotFound)
        );
    }

    #[tokio::test]
    async fn pair_all_zero_key_is_invalid_hello() {
        let mut session = machine(MemKeyStore::new(), FakeBoard::new(KEY_ONE, NONCE_ONE));
        let bad = pairing_hello([0u8; 32], NONCE_ONE);
        let err = session
            .pair(&bad)
            .await
            .expect_err("all-zero key must fail");
        assert!(matches!(err, SessionError::InvalidHello(_)), "got {err:?}");
        assert_eq!(
            session.keystore().get_device_key(BOARD_ID),
            Err(KeystoreError::NotFound)
        );
    }

    #[tokio::test]
    async fn pair_twice_while_active_is_already_active() {
        let mut session = machine(MemKeyStore::new(), FakeBoard::new(KEY_ONE, NONCE_ONE));
        session
            .pair(&pairing_hello(KEY_ONE, NONCE_ONE))
            .await
            .expect("first pair succeeds");
        let err = session
            .pair(&pairing_hello(KEY_TWO, NONCE_TWO))
            .await
            .expect_err("second pair must fail");
        assert!(matches!(err, SessionError::AlreadyActive), "got {err:?}");
        // The first session is untouched.
        assert_eq!(session.state(), SessionState::SessionActive);
        assert_eq!(
            session
                .keystore()
                .get_device_key(BOARD_ID)
                .expect("key stored"),
            KEY_ONE
        );
    }

    /// SEC-R2-06: a HELLO disclosing a *different* key for a board
    /// that already has a stored key is refused — nothing is stored,
    /// the old key survives, and the state is untouched.
    #[tokio::test]
    async fn pair_refuses_different_key_when_one_is_stored() {
        let mut session = machine(MemKeyStore::new(), FakeBoard::new(KEY_ONE, NONCE_ONE));
        session
            .pair(&pairing_hello(KEY_ONE, NONCE_ONE))
            .await
            .expect("first pair succeeds");
        session.close();
        assert_eq!(session.state(), SessionState::Paired);
        // The board was KEY_RESET elsewhere (or an evil twin spoofs
        // this MAC): it now discloses a different key. Refusal happens
        // before any control frame, so the fake board's key is
        // irrelevant here.
        let err = session
            .pair(&pairing_hello(KEY_TWO, NONCE_TWO))
            .await
            .expect_err("different stored key must refuse");
        assert!(matches!(err, SessionError::StoredKeyExists), "got {err:?}");
        assert_eq!(
            session
                .keystore()
                .get_device_key(BOARD_ID)
                .expect("old key survives"),
            KEY_ONE
        );
        assert_eq!(session.state(), SessionState::Paired);
        assert!(!session.is_active());
    }

    /// SEC-R2-06 confirmed path: after explicit user confirmation the
    /// replacement proceeds and the new key is stored.
    #[tokio::test]
    async fn pair_with_replacement_overwrites_after_confirmation() {
        let mut session = machine(MemKeyStore::new(), FakeBoard::new(KEY_ONE, NONCE_ONE));
        session
            .pair(&pairing_hello(KEY_ONE, NONCE_ONE))
            .await
            .expect("first pair succeeds");
        session.close();
        // The board rotated its key; the fake board now verifies with
        // the new key and a fresh nonce.
        session
            .transport
            .as_mut()
            .expect("transport idle after close")
            .key = KEY_TWO;
        session
            .transport
            .as_mut()
            .expect("transport idle after close")
            .nonce = NONCE_TWO;
        let caps = session
            .pair_with_replacement(&pairing_hello(KEY_TWO, NONCE_TWO))
            .await
            .expect("confirmed replacement succeeds");
        assert_eq!(caps.firmware, (4, 0, 1));
        assert_eq!(session.state(), SessionState::SessionActive);
        assert_eq!(
            session
                .keystore()
                .get_device_key(BOARD_ID)
                .expect("new key stored"),
            KEY_TWO
        );
    }

    /// SEC-R2-06: re-disclosing the *same* key is idempotent — the
    /// gate only fires when the disclosed key differs.
    #[tokio::test]
    async fn pair_allows_same_key_redisclosure() {
        let mut session = machine(MemKeyStore::new(), FakeBoard::new(KEY_ONE, NONCE_ONE));
        session
            .pair(&pairing_hello(KEY_ONE, NONCE_ONE))
            .await
            .expect("first pair succeeds");
        session.close();
        session
            .transport
            .as_mut()
            .expect("transport idle after close")
            .nonce = NONCE_TWO;
        let caps = session
            .pair(&pairing_hello(KEY_ONE, NONCE_TWO))
            .await
            .expect("same-key re-disclosure succeeds");
        assert_eq!(caps.firmware, (4, 0, 1));
        assert_eq!(session.state(), SessionState::SessionActive);
    }

    #[tokio::test]
    async fn resume_happy_path_with_fresh_nonce() {
        let mut session = machine(MemKeyStore::new(), FakeBoard::new(KEY_ONE, NONCE_ONE));
        session
            .pair(&pairing_hello(KEY_ONE, NONCE_ONE))
            .await
            .expect("pair succeeds");
        session.close();
        assert_eq!(session.state(), SessionState::Paired);
        assert!(!session.is_active());
        assert!(session.control_client().is_none());
        // Board rebooted: fresh nonce, same key.
        session
            .transport
            .as_mut()
            .expect("transport idle after close")
            .nonce = NONCE_TWO;
        let caps = session
            .resume(&paired_hello(NONCE_TWO))
            .await
            .expect("resume with a fresh nonce succeeds");
        assert_eq!(caps.firmware, (4, 0, 1));
        assert_eq!(session.state(), SessionState::SessionActive);
        assert_eq!(session.last_nonce(), Some(NONCE_TWO));
        // New session: counter restarted at 0. Two GET_CAPS sends in
        // total on this transport: one from pairing, one from resume.
        let live = session.control_client().expect("client live").session();
        assert_eq!(live.key, KEY_ONE);
        assert_eq!(live.nonce, NONCE_TWO);
        assert_eq!(live.counter, 1);
        let frames = sent_frames(&session);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[1].cmd, Opcode::GetCaps as u8);
        assert_eq!(frames[1].counter, 0);
    }

    #[tokio::test]
    async fn resume_with_no_stored_key_is_no_stored_key() {
        let mut session = machine(MemKeyStore::new(), FakeBoard::new(KEY_ONE, NONCE_ONE));
        let err = session
            .resume(&paired_hello(NONCE_ONE))
            .await
            .expect_err("resume without a key must fail");
        assert!(matches!(err, SessionError::NoStoredKey), "got {err:?}");
        assert_eq!(session.state(), SessionState::Unpaired);
    }

    #[tokio::test]
    async fn resume_unpaired_board_is_board_unpaired() {
        let mut session = machine(MemKeyStore::new(), FakeBoard::new(KEY_ONE, NONCE_ONE));
        session
            .pair(&pairing_hello(KEY_ONE, NONCE_ONE))
            .await
            .expect("pair succeeds");
        session.close();
        // The board lost its pairing (KEY_RESET elsewhere): resume is
        // the wrong operation now.
        let err = session
            .resume(&pairing_hello(KEY_TWO, NONCE_TWO))
            .await
            .expect_err("resume of an unpaired board must fail");
        assert!(matches!(err, SessionError::BoardUnpaired), "got {err:?}");
        assert_eq!(session.state(), SessionState::Paired);
    }

    #[tokio::test]
    async fn resume_with_old_nonce_is_bad_tag() {
        // Board rebooted and rotated its nonce to NONCE_TWO, but the app
        // resumes with a HELLO carrying the stale NONCE_ONE: the board
        // answers BAD_TAG and the confirming GET_CAPS fails.
        let store = MemKeyStore::new();
        store
            .store_device_key(BOARD_ID, &KEY_ONE)
            .expect("pre-store key");
        let mut stale = machine(store, FakeBoard::new(KEY_ONE, NONCE_TWO));
        let err = stale
            .resume(&paired_hello(NONCE_ONE))
            .await
            .expect_err("stale nonce must fail");
        assert!(
            matches!(
                err,
                SessionError::Control(crate::control_client::ControlError::BadTag)
            ),
            "got {err:?}"
        );
        assert_eq!(stale.state(), SessionState::Paired);
        assert!(stale.control_client().is_none());
        // The stored key survives a BadTag: a fresh HELLO retries.
        assert_eq!(
            stale.keystore().get_device_key(BOARD_ID).expect("key kept"),
            KEY_ONE
        );
    }

    #[tokio::test]
    async fn resume_after_bad_tag_with_fresh_nonce_succeeds() {
        let store = MemKeyStore::new();
        store
            .store_device_key(BOARD_ID, &KEY_ONE)
            .expect("pre-store key");
        let mut session = machine(store, FakeBoard::new(KEY_ONE, NONCE_TWO));
        let err = session
            .resume(&paired_hello(NONCE_ONE))
            .await
            .expect_err("stale nonce must fail");
        assert!(matches!(
            err,
            SessionError::Control(crate::control_client::ControlError::BadTag)
        ));
        session
            .resume(&paired_hello(NONCE_TWO))
            .await
            .expect("fresh nonce resumes");
        assert_eq!(session.state(), SessionState::SessionActive);
    }

    #[tokio::test]
    async fn key_reset_deletes_store_and_invalidates_old_key() {
        let mut board = FakeBoard::new(KEY_ONE, NONCE_ONE);
        board.rotate_key_on_reset(KEY_TWO);
        let mut session = machine(MemKeyStore::new(), board);
        session
            .pair(&pairing_hello(KEY_ONE, NONCE_ONE))
            .await
            .expect("pair succeeds");
        session.key_reset().await.expect("KEY_RESET succeeds");
        assert_eq!(session.state(), SessionState::Unpaired);
        assert!(!session.is_active());
        assert!(session.control_client().is_none());
        assert_eq!(session.last_nonce(), None);
        // The local store is empty: key and Wi-Fi password both gone.
        assert_eq!(
            session.keystore().get_device_key(BOARD_ID),
            Err(KeystoreError::NotFound)
        );
        // Resume is impossible without a stored key.
        let err = session
            .resume(&paired_hello(NONCE_TWO))
            .await
            .expect_err("resume after KEY_RESET must fail");
        assert!(matches!(err, SessionError::NoStoredKey), "got {err:?}");
        // The old key no longer signs a valid command: a fresh client
        // with the old session against a board that rotated its key
        // gets BAD_TAG.
        let mut old_client = ControlClient::new(
            FakeBoard::new(KEY_TWO, NONCE_TWO),
            Session::new(KEY_ONE, NONCE_TWO),
        )
        .with_timeout(Duration::from_millis(10));
        let err = old_client
            .get_caps()
            .await
            .expect_err("old key must fail after KEY_RESET");
        assert!(
            matches!(err, crate::control_client::ControlError::BadTag),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn key_reset_without_session_is_not_active() {
        let mut session = machine(MemKeyStore::new(), FakeBoard::new(KEY_ONE, NONCE_ONE));
        let err = session
            .key_reset()
            .await
            .expect_err("KEY_RESET without a session must fail");
        assert!(matches!(err, SessionError::NotActive), "got {err:?}");
    }

    #[tokio::test]
    async fn key_reset_failure_keeps_session_and_key() {
        let mut session = machine(MemKeyStore::new(), FakeBoard::new(KEY_ONE, NONCE_ONE));
        session
            .pair(&pairing_hello(KEY_ONE, NONCE_ONE))
            .await
            .expect("pair succeeds");
        // Drop every reply from here on: KEY_RESET times out.
        session
            .control_client_mut()
            .expect("client live")
            .transport_mut()
            .drop_all = true;
        let err = session
            .key_reset()
            .await
            .expect_err("dropped KEY_RESET must fail");
        assert!(
            matches!(
                err,
                SessionError::Control(crate::control_client::ControlError::Timeout)
            ),
            "got {err:?}"
        );
        // Session, stored key, and state are untouched: retry is safe.
        assert_eq!(session.state(), SessionState::SessionActive);
        assert_eq!(
            session
                .keystore()
                .get_device_key(BOARD_ID)
                .expect("key kept"),
            KEY_ONE
        );
    }

    #[tokio::test]
    async fn re_pairing_always_uses_a_new_key() {
        let mut board = FakeBoard::new(KEY_ONE, NONCE_ONE);
        board.rotate_key_on_reset(KEY_TWO);
        let mut session = machine(MemKeyStore::new(), board);
        session
            .pair(&pairing_hello(KEY_ONE, NONCE_ONE))
            .await
            .expect("first pair succeeds");
        session.key_reset().await.expect("KEY_RESET succeeds");
        // New window, new nonce, new key disclosed: re-pairing stores
        // the NEW key, never the old one.
        session
            .transport
            .as_mut()
            .expect("transport idle after KEY_RESET")
            .nonce = NONCE_TWO;
        session
            .pair(&pairing_hello(KEY_TWO, NONCE_TWO))
            .await
            .expect("re-pair succeeds");
        assert_eq!(session.state(), SessionState::SessionActive);
        assert_eq!(
            session
                .keystore()
                .get_device_key(BOARD_ID)
                .expect("new key stored"),
            KEY_TWO
        );
        let live = session.control_client().expect("client live").session();
        assert_eq!(live.key, KEY_TWO);
        assert_eq!(live.nonce, NONCE_TWO);
    }

    #[tokio::test]
    async fn window_expiry_zeroes_pending_and_refuses_pair() {
        let mut session = machine(MemKeyStore::new(), FakeBoard::new(KEY_ONE, NONCE_ONE));
        let hello = pairing_hello(KEY_ONE, NONCE_ONE);
        session.observe_hello(&hello);
        assert_eq!(session.state(), SessionState::Pairing);
        assert!(session.has_pending_credentials());
        // The pending copy holds the disclosed key.
        assert_eq!(session.pending.as_ref().expect("pending held").key, KEY_ONE);
        assert!(session.expire_window());
        assert_eq!(session.state(), SessionState::Unpaired);
        assert!(!session.has_pending_credentials());
        assert!(session.pending.is_none());
        // The same HELLO (same window nonce) can no longer pair.
        let err = session
            .pair(&hello)
            .await
            .expect_err("expired window must fail");
        assert!(matches!(err, SessionError::WindowExpired), "got {err:?}");
        assert_eq!(
            session.keystore().get_device_key(BOARD_ID),
            Err(KeystoreError::NotFound)
        );
        // A fresh HELLO (new nonce, new window) pairs again.
        session.transport.as_mut().expect("transport idle").nonce = NONCE_TWO;
        session
            .pair(&pairing_hello(KEY_ONE, NONCE_TWO))
            .await
            .expect("new window pairs");
        assert_eq!(session.state(), SessionState::SessionActive);
    }

    #[tokio::test]
    async fn window_expiry_by_elapsed_time() {
        let mut session = machine(MemKeyStore::new(), FakeBoard::new(KEY_ONE, NONCE_ONE));
        let hello = pairing_hello(KEY_ONE, NONCE_ONE);
        session.observe_hello(&hello);
        assert!(session.has_pending_credentials());
        // Pretend 61 s passed since the window opened.
        session.window_opened_at = Some(
            Instant::now()
                .checked_sub(Duration::from_secs(61))
                .expect("test clock supports subtracting 61 s"),
        );
        assert!(session.check_window_expiry());
        assert!(!session.has_pending_credentials());
        assert_eq!(session.state(), SessionState::Unpaired);
        let err = session
            .pair(&hello)
            .await
            .expect_err("elapsed window must fail");
        assert!(matches!(err, SessionError::WindowExpired), "got {err:?}");
    }

    #[tokio::test]
    async fn check_window_expiry_is_false_inside_window() {
        let mut session = machine(MemKeyStore::new(), FakeBoard::new(KEY_ONE, NONCE_ONE));
        session.observe_hello(&pairing_hello(KEY_ONE, NONCE_ONE));
        assert!(!session.check_window_expiry());
        assert!(session.has_pending_credentials());
        session
            .pair(&pairing_hello(KEY_ONE, NONCE_ONE))
            .await
            .expect("pair inside the window succeeds");
    }

    #[tokio::test]
    async fn close_zeroizes_live_key_and_returns_to_paired() {
        let mut session = machine(MemKeyStore::new(), FakeBoard::new(KEY_ONE, NONCE_ONE));
        session
            .pair(&pairing_hello(KEY_ONE, NONCE_ONE))
            .await
            .expect("pair succeeds");
        session.close();
        assert_eq!(session.state(), SessionState::Paired);
        assert!(session.control_client().is_none());
        assert!(!session.stream_allowed());
        // The stored key survives close; a fresh session resumes.
        assert_eq!(
            session
                .keystore()
                .get_device_key(BOARD_ID)
                .expect("key kept"),
            KEY_ONE
        );
        session
            .transport
            .as_mut()
            .expect("transport idle after close")
            .nonce = NONCE_TWO;
        session
            .resume(&paired_hello(NONCE_TWO))
            .await
            .expect("resume after close succeeds");
    }

    #[tokio::test]
    async fn take_transport_returns_idle_transport_after_close() {
        // H5: the release path the forwarder activator uses — the
        // transport goes back out after the session ends, so the same
        // transport can stream frames.
        let mut session = machine(MemKeyStore::new(), FakeBoard::new(KEY_ONE, NONCE_ONE));
        session
            .pair(&pairing_hello(KEY_ONE, NONCE_ONE))
            .await
            .expect("pair succeeds");
        // Live session: the transport is owned by the control client.
        assert!(session.take_transport().is_none());
        session.close();
        let transport = session.take_transport();
        assert!(transport.is_some(), "idle transport comes back out");
        // Second take is empty: the machine holds nothing anymore.
        assert!(session.take_transport().is_none());
        // The taken transport is the board: it recorded the GET_CAPS.
        let sent = transport.expect("taken transport").sent;
        assert_eq!(sent.len(), 1);
        assert_eq!(
            ControlFrame::parse_structure(&sent[0])
                .expect("sent frame parses")
                .cmd,
            Opcode::GetCaps as u8
        );
    }

    #[tokio::test]
    async fn take_transport_returns_idle_transport_after_failed_pair() {
        // A failed activation already returned the transport to idle,
        // so the forwarder gets it back without a close() first.
        let mut board = FakeBoard::new(KEY_ONE, NONCE_ONE);
        board.drop_all = true; // Drop every reply: GET_CAPS times out.
        let mut session = machine(MemKeyStore::new(), board);
        let _err = session
            .pair(&pairing_hello(KEY_ONE, NONCE_ONE))
            .await
            .expect_err("dropped GET_CAPS must fail");
        assert!(session.take_transport().is_some());
    }

    #[tokio::test]
    async fn take_and_restore_control_client_round_trip() {
        // H5-control: a transaction client that must own the control
        // client borrows it out and hands it back; the session still
        // closes cleanly afterwards (key zeroized, transport returned).
        let mut session = machine(MemKeyStore::new(), FakeBoard::new(KEY_ONE, NONCE_ONE));
        assert!(session.take_control_client().is_none(), "no client yet");
        session
            .pair(&pairing_hello(KEY_ONE, NONCE_ONE))
            .await
            .expect("pair succeeds");
        assert!(session.control_client().is_some());
        let client = session
            .take_control_client()
            .expect("live client comes out");
        assert!(session.control_client().is_none());
        assert!(
            session.is_active(),
            "taking the client keeps the session active"
        );
        // The taken client still works: one more command through it.
        let mut client = client;
        client
            .reboot()
            .await
            .expect("REBOOT through the taken client succeeds");
        session.restore_control_client(client);
        assert!(session.control_client().is_some());
        session.close();
        assert_eq!(session.state(), SessionState::Paired);
        assert!(session.control_client().is_none());
        assert!(session.take_transport().is_some(), "transport returned");
    }

    #[tokio::test]
    async fn get_caps_failure_leaves_stored_key_and_paired_state() {
        let mut board = FakeBoard::new(KEY_ONE, NONCE_ONE);
        board.drop_all = true; // Drop every reply: GET_CAPS times out.
        let mut session = machine(MemKeyStore::new(), board);
        let err = session
            .pair(&pairing_hello(KEY_ONE, NONCE_ONE))
            .await
            .expect_err("dropped GET_CAPS must fail");
        assert!(
            matches!(
                err,
                SessionError::Control(crate::control_client::ControlError::Timeout)
            ),
            "got {err:?}"
        );
        // Key stays stored (the board disclosed it); state is Paired,
        // so a fresh HELLO can retry via resume.
        assert_eq!(session.state(), SessionState::Paired);
        assert_eq!(
            session
                .keystore()
                .get_device_key(BOARD_ID)
                .expect("key kept"),
            KEY_ONE
        );
        assert!(session.control_client().is_none());
    }

    #[tokio::test]
    async fn platform_unavailable_fails_pairing_loudly() {
        let failing = MemKeyStore::failing(KeystoreError::PlatformUnavailable);
        let mut session = machine(failing, FakeBoard::new(KEY_ONE, NONCE_ONE));
        let err = session
            .pair(&pairing_hello(KEY_ONE, NONCE_ONE))
            .await
            .expect_err("unavailable store must fail pairing");
        assert!(
            matches!(
                err,
                SessionError::Keystore(KeystoreError::PlatformUnavailable)
            ),
            "got {err:?}"
        );
        assert_eq!(session.state(), SessionState::Unpaired);
        // Never silently unpaired-with-a-session: no control command
        // was sent at all.
        assert!(session.control_client().is_none());
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn real_keystore_fails_closed_on_linux_sandbox() {
        // The real platform store does not exist on this Linux sandbox,
        // so pairing must fail loudly with PlatformUnavailable — never
        // proceed without a stored key.
        use rsh_security::keystore::KeyStore;
        assert!(!KeyStore::store_available());
        let mut session = BoardSession::new(
            BOARD_ID,
            KeyStore::new(),
            FakeBoard::new(KEY_ONE, NONCE_ONE),
        )
        .with_control_timeout(Duration::from_millis(10));
        let err = session
            .pair(&pairing_hello(KEY_ONE, NONCE_ONE))
            .await
            .expect_err("real store unavailable must fail pairing");
        assert!(
            matches!(
                err,
                SessionError::Keystore(KeystoreError::PlatformUnavailable)
            ),
            "got {err:?}"
        );
        assert_eq!(session.state(), SessionState::Unpaired);
        assert!(session.control_client().is_none());
    }

    #[test]
    fn zeroize_overwrites_every_byte() {
        let mut buf = [0xAAu8; 32];
        zeroize(&mut buf);
        assert_eq!(buf, [0u8; 32]);
    }

    #[test]
    fn pending_credentials_zeroize_on_drop() {
        let mut pending = super::PendingCredentials {
            key: KEY_ONE,
            wifi_password: *WIFI_PASSWORD,
        };
        pending.zeroize();
        assert_eq!(pending.key, [0u8; 32]);
        assert_eq!(pending.wifi_password, [0u8; 24]);
    }

    #[test]
    fn session_error_display_is_secret_free() {
        assert_eq!(
            SessionError::WindowClosed.to_string(),
            "pairing window is closed"
        );
        assert_eq!(
            SessionError::NoStoredKey.to_string(),
            "no device key stored for this board"
        );
        assert_eq!(
            SessionError::Keystore(KeystoreError::PlatformUnavailable).to_string(),
            "credential store error: platform credential store is unavailable"
        );
        let debug = format!(
            "{:?}",
            BoardSession::new(
                BOARD_ID,
                MemKeyStore::new(),
                FakeBoard::new(KEY_ONE, NONCE_ONE)
            )
        );
        assert!(
            !debug.contains(BOARD_ID),
            "Debug leaked the board id: {debug}"
        );
    }

    #[test]
    fn session_state_transitions() {
        assert_ne!(SessionState::Unpaired, SessionState::Pairing);
        assert_ne!(SessionState::Paired, SessionState::SessionActive);
    }

    #[test]
    fn repeat_hello_same_nonce_does_not_refresh_window() {
        // M2: the 60 s window opens on a new nonce only. A repeat
        // HELLO for the same window leaves window_opened_at alone.
        let mut session = machine(MemKeyStore::new(), FakeBoard::new(KEY_ONE, NONCE_ONE));
        session.observe_hello(&pairing_hello(KEY_ONE, NONCE_ONE));
        let opened = session.window_opened_at.expect("window opened");
        std::thread::sleep(Duration::from_millis(2));
        session.observe_hello(&pairing_hello(KEY_ONE, NONCE_ONE));
        assert_eq!(
            session.window_opened_at,
            Some(opened),
            "repeat HELLO must not refresh"
        );
        assert_eq!(session.window_nonce, Some(NONCE_ONE));
        // A fresh nonce opens a new window (refreshed).
        std::thread::sleep(Duration::from_millis(2));
        session.observe_hello(&pairing_hello(KEY_ONE, NONCE_TWO));
        let reopened = session.window_opened_at.expect("new window opened");
        assert!(reopened > opened, "new nonce must open a new window");
        assert_eq!(session.window_nonce, Some(NONCE_TWO));
    }

    #[test]
    fn close_clears_window_tracking() {
        // L9: close() (via discard_pending) clears window_opened_at
        // and window_nonce, not just the pending credentials.
        let mut session = machine(MemKeyStore::new(), FakeBoard::new(KEY_ONE, NONCE_ONE));
        session.observe_hello(&pairing_hello(KEY_ONE, NONCE_ONE));
        assert!(session.window_opened_at.is_some());
        assert!(session.window_nonce.is_some());
        session.close();
        assert_eq!(session.window_opened_at, None);
        assert_eq!(session.window_nonce, None);
        assert!(!session.has_pending_credentials());
    }

    #[tokio::test]
    async fn pair_rejects_all_zero_wifi_password() {
        // M3: an all-zero Wi-Fi password is not a password.
        let mut session = machine(MemKeyStore::new(), FakeBoard::new(KEY_ONE, NONCE_ONE));
        let hello = hello(0x09, NONCE_ONE, KEY_ONE, [0u8; 24]);
        let err = session
            .pair(&hello)
            .await
            .expect_err("all-zero password must fail");
        assert!(matches!(err, SessionError::InvalidHello(_)), "got {err:?}");
        assert_eq!(session.state(), SessionState::Unpaired);
    }

    #[tokio::test]
    async fn pair_trims_trailing_nul_wifi_password() {
        // M3: the 24-byte field is NUL-padded — "secret" + padding
        // stores as "secret", not "secret\0\0…".
        let store = MemKeyStore::new();
        let mut session = machine(store, FakeBoard::new(KEY_ONE, NONCE_ONE));
        let mut wifi = [0u8; 24];
        wifi[..6].copy_from_slice(b"secret");
        let hello = hello(0x09, NONCE_ONE, KEY_ONE, wifi);
        session
            .pair(&hello)
            .await
            .expect("pairing with padded password succeeds");
        let entries = session.keystore().entries.lock().expect("test store lock");
        let stored = entries
            .get(&("RSH".to_owned(), format!("board:{BOARD_ID}:wifi")))
            .expect("wifi password stored");
        assert_eq!(stored, b"secret", "trailing NULs must be trimmed");
    }
}
