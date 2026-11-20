//! Connect-play session state machine (protocol v2.2, no-pairing redesign).
//!
//! [`BoardSession`] drives the board through its link lifecycle —
//! `Idle` → `Active` — on top of the
//! [`crate::control_client::ControlClient`] and the transport. There is
//! no pairing, no device key, no nonce, and no HMAC: a session is
//! established by a single unsigned `connect(hello)` — the HELLO proves
//! the board is reachable, and an unsigned `GET_CAPS` is sent **first**
//! (SEC-E-14: the board streams only after a plain `GET_CAPS`) before
//! the session is marked active.
//!
//! The keystore sits behind the small [`KeyStoreLike`] trait so tests can
//! substitute an in-memory store; production code passes the real
//! [`rsh_security::keystore::KeyStore`]. The keystore holds only the
//! Wi-Fi password (D8 — the `WebOTA` auth source); connecting to a board
//! needs no credential store at all. A missing store blocks
//! password-set / `WebOTA`, never connect.
//!
//! Password-clear (`key_reset`, D7) is physical-only on the board (BOOT
//! held ≥ 10 s) — there is no wire opcode anymore. The PC-side
//! [`BoardSession::key_reset`] only deletes the locally stored Wi-Fi
//! password (and closes any live session); the board-side clear needs
//! the BOOT hold, which the Settings UI must say.

use std::time::Duration;

use rsh_protocol::{GetCapsReply, HelloError, HelloReply};
use rsh_security::keystore::{KeyStore, KeystoreError};

use crate::ControlTransport;
use crate::control_client::{ControlClient, ControlError, Session};

/// The one operation the session state machine needs from a key store.
///
/// The real [`rsh_security::keystore::KeyStore`] implements this
/// trait; tests substitute an in-memory store. The trait is deliberately
/// narrow — only the Wi-Fi password store/delete used by `key_reset`
/// (password-clear) — so a substitute cannot silently widen what the
/// session touches. Connecting never touches the store.
pub trait KeyStoreLike: Send {
    /// Store (or replace) the Wi-Fi / OTA password for `board_id`.
    ///
    /// # Errors
    ///
    /// Returns [`KeystoreError`] when the board identifier is invalid,
    /// the password is empty or too large, or the platform credential
    /// store cannot be reached.
    fn store_wifi_password(&self, board_id: &str, password: &str) -> Result<(), KeystoreError>;

    /// Read the stored Wi-Fi / OTA password for `board_id` (v2.2, D3:
    /// `SET_PASSWORD` must carry the current password when one is set
    /// — the stored password IS the current one).
    ///
    /// # Errors
    ///
    /// Returns [`KeystoreError::NotFound`] when no password is stored, or
    /// [`KeystoreError::PlatformUnavailable`] when the platform store
    /// cannot be reached.
    fn get_wifi_password(&self, board_id: &str) -> Result<String, KeystoreError>;

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
    fn store_wifi_password(&self, board_id: &str, password: &str) -> Result<(), KeystoreError> {
        KeyStore::store_wifi_password(self, board_id, password)
    }

    fn get_wifi_password(&self, board_id: &str) -> Result<String, KeystoreError> {
        KeyStore::get_wifi_password(self, board_id)
    }

    fn delete_wifi_password(&self, board_id: &str) -> Result<(), KeystoreError> {
        KeyStore::delete_wifi_password(self, board_id)
    }
}

/// Link state of one board.
///
/// The transitions are: `Idle` → `Active` (unsigned `GET_CAPS`
/// accepted). The machine returns to `Idle` on `close()` and on
/// `key_reset()` (password-clear).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    /// No live session for this board.
    Idle,
    /// A live unsigned session exists: a plain `GET_CAPS` was accepted
    /// and the board may now emit the 200 Hz stream (SEC-E-14).
    Active,
}

/// Errors from the connect-play session state machine.
///
/// No variant ever carries password material: messages are fixed,
/// secret-free descriptions.
#[derive(Debug)]
pub enum SessionError {
    /// The `HELLO` board MAC does not match this session's board.
    BoardMismatch,
    /// The operation needs a live session, but none exists.
    NotActive,
    /// A live session already exists.
    AlreadyActive,
    /// The `HELLO` reply is structurally usable but cannot start a
    /// session (for example an old protocol-2.1 board, which speaks no
    /// v2.2 control). The message is a fixed, secret-free description.
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
            Self::BoardMismatch => {
                write!(f, "HELLO board MAC does not match this session's board")
            }
            Self::NotActive => write!(f, "no live session for this board"),
            Self::AlreadyActive => write!(f, "a session is already active for this board"),
            Self::InvalidHello(msg) => write!(f, "HELLO cannot start a session: {msg}"),
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

/// The connect-play session state machine for one board.
///
/// Generic over the key store (`K`) and the control transport (`T`), like
/// [`crate::control_client::ControlClient`] is generic over
/// the transport. One `BoardSession` owns one board's link lifecycle:
/// the transport is held idle until a session activates, then owned by
/// the control client until the session ends.
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
    caps: Option<GetCapsReply>,
    control_timeout: Duration,
}

impl<K: KeyStoreLike, T: ControlTransport> BoardSession<K, T> {
    /// Create a session machine for `board_id`, with no live session.
    ///
    /// The transport is held idle until [`BoardSession::connect`]
    /// activates it. Creation never touches the credential store and
    /// never fails.
    #[must_use]
    pub fn new(board_id: &str, keystore: K, transport: T) -> Self {
        Self {
            board_id: board_id.trim().to_ascii_uppercase(),
            keystore,
            transport: Some(transport),
            client: None,
            state: SessionState::Idle,
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

    /// The current link state.
    #[must_use]
    pub fn state(&self) -> SessionState {
        self.state
    }

    /// `true` while a live session exists.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.state == SessionState::Active
    }

    /// `true` once the board may emit the 200 Hz stream: a plain
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
    /// session closes. Taking the client does not change the session
    /// state: the session stays active, and a session whose client was
    /// taken but never restored still ends safely — the transport is
    /// only handed back by [`BoardSession::close`] /
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
    /// Call only after a take, before [`BoardSession::close`].
    /// Restoring while a client is already present would drop that
    /// client, so this is a debug assertion in tests and a plain
    /// replace in release — the only caller (the H5-control helper)
    /// always takes first.
    pub fn restore_control_client(&mut self, client: ControlClient<T>) {
        debug_assert!(
            self.client.is_none(),
            "restore_control_client called while a client is live"
        );
        self.client = Some(client);
    }

    /// The key store behind this session (tests inspect the stored
    /// Wi-Fi password).
    #[must_use]
    pub fn keystore(&self) -> &K {
        &self.keystore
    }

    /// Connect to the board described by `hello`: send an unsigned
    /// `GET_CAPS` **first** (SEC-E-14) and mark the session active only
    /// on its success. The returned value is that `GET_CAPS` reply.
    ///
    /// No credential store is touched: connecting needs no password and
    /// no key. A `GET_CAPS` failure leaves the state at `Idle` and the
    /// transport idle, so a fresh `HELLO` may retry.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::BoardMismatch`] for a foreign board MAC,
    /// [`SessionError::InvalidHello`] for a HELLO that cannot start a
    /// session (an old protocol-2.1 board — update its firmware
    /// instead), [`SessionError::AlreadyActive`] when a session is
    /// already live, and [`SessionError::Control`] when the confirming
    /// `GET_CAPS` fails.
    pub async fn connect(&mut self, hello: &HelloReply) -> Result<GetCapsReply, SessionError> {
        if self.client.is_some() {
            return Err(SessionError::AlreadyActive);
        }
        if !self.board_mac_matches(hello) {
            return Err(SessionError::BoardMismatch);
        }
        if hello.is_old_protocol() {
            return Err(SessionError::InvalidHello(
                "board speaks protocol 2.1; update the board firmware first",
            ));
        }
        let session = Session::new();
        match self.activate(session).await {
            Ok(caps) => {
                self.state = SessionState::Active;
                Ok(caps)
            }
            Err(error) => {
                self.state = SessionState::Idle;
                Err(error)
            }
        }
    }

    /// Connect from raw 92-byte `HELLO` reply bytes (parsed first, then
    /// [`BoardSession::connect`]).
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::HelloParse`] for malformed bytes, plus the
    /// errors of [`BoardSession::connect`].
    pub async fn connect_bytes(&mut self, data: &[u8]) -> Result<GetCapsReply, SessionError> {
        let hello = HelloReply::parse(data)?;
        self.connect(&hello).await
    }

    /// Clear the Wi-Fi password (D7 password-clear, PC side only).
    ///
    /// Closes any live session first, then deletes the locally stored
    /// Wi-Fi password for this board and returns to `Idle`. No wire
    /// command is sent — there is no `KEY_RESET` opcode anymore; the
    /// board-side clear is physical (hold BOOT ≥ 10 s), and the
    /// Settings UI must tell the user that. A missing stored password
    /// is a successful clear.
    ///
    /// A credential-store failure is reported loudly — the session is
    /// still closed and the state is still `Idle`, but the stored
    /// password may survive, so the caller must surface the error.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::Keystore`] when the local delete fails
    /// for a reason other than "already gone".
    pub fn key_reset(&mut self) -> Result<(), SessionError> {
        self.close();
        match self.keystore.delete_wifi_password(&self.board_id) {
            Ok(()) | Err(KeystoreError::NotFound) => Ok(()),
            Err(error) => Err(SessionError::Keystore(error)),
        }
    }

    /// End the live session: return the transport to idle and fall back
    /// to `Idle`. A no-op without a live session.
    pub fn close(&mut self) {
        if let Some(client) = self.client.take() {
            let (transport, _session) = client.into_inner();
            self.transport = Some(transport);
            self.caps = None;
            self.state = SessionState::Idle;
        }
    }

    /// Take the idle transport back out of the session machine (H5).
    ///
    /// After [`BoardSession::close`] — or after a failed
    /// [`BoardSession::connect`], which already returned the transport
    /// to idle — the transport is idle again and the caller (the app's
    /// transport forwarder) needs it back to stream frames. The session
    /// itself does not survive the take. A later control operation
    /// starts a fresh session with a fresh `reqId`
    /// ([`BoardSession::connect`]).
    ///
    /// Returns [`None`] when the transport is live inside a session
    /// (call `close` first) or was already taken.
    #[must_use]
    pub fn take_transport(&mut self) -> Option<T> {
        self.transport.take()
    }

    /// Build the control client, send the unsigned `GET_CAPS` first
    /// (SEC-E-14), and keep the client on success. On failure the
    /// transport is returned to idle.
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
                let (transport, _session) = client.into_inner();
                self.transport = Some(transport);
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
}

impl<K: KeyStoreLike, T: ControlTransport> std::fmt::Debug for BoardSession<K, T> {
    /// Debug output shows only the state — never the board id, and never
    /// password material (this type holds no password in memory).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoardSession")
            .field("state", &self.state)
            .field("active", &self.client.is_some())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, VecDeque};
    use std::sync::Mutex;
    use std::time::Duration;

    use rsh_protocol::control::{ErrorCode, Opcode, build_reply};
    use rsh_protocol::{ControlFrame, GetCapsReply, PinTableEntry};

    use super::{BoardSession, KeyStoreLike, KeystoreError, SessionError, SessionState};
    use crate::{ControlTransport, TransportError};

    /// Board MAC used by the tests (and the matching board id).
    const BOARD_ID: &str = "AA:BB:CC:DD:EE:FF";
    /// Board MAC bytes.
    const BOARD_MAC: [u8; 6] = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
    /// Wi-Fi password stored for the board.
    const WIFI_PASSWORD: &str = "Ab3dEf6hJk9mNp2rSt5vXy8";

    /// In-memory [`KeyStoreLike`] for tests only (never touches the OS
    /// store). Can be put into a failing mode to prove `key_reset` fails
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

        fn get_wifi_password(&self, board_id: &str) -> Result<String, KeystoreError> {
            if let Some(error) = self.fail() {
                return Err(error);
            }
            let account = format!("board:{}:wifi", board_id.trim().to_ascii_uppercase());
            self.entries
                .lock()
                .expect("test store lock")
                .get(&("RSH".to_owned(), account))
                .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                .ok_or(KeystoreError::NotFound)
        }
    }

    /// A fake board: CRC-verifies requests, answers `GET_CAPS` (and
    /// every other command) with a valid reply, records every frame
    /// sent, and can drop every reply (to force client retries until
    /// the 3-attempt timeout).
    struct FakeBoard {
        sent: Vec<Vec<u8>>,
        pending: VecDeque<Vec<u8>>,
        /// When `true`, every reply is dropped (forces client retries
        /// until the 3-attempt timeout).
        drop_all: bool,
    }

    impl FakeBoard {
        fn new() -> Self {
            Self {
                sent: Vec::new(),
                pending: VecDeque::new(),
                drop_all: false,
            }
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
                let Ok(frame) = ControlFrame::verify(&owned) else {
                    return Ok(());
                };
                let body = if frame.cmd == Opcode::GetCaps as u8 {
                    caps_body()
                } else {
                    Vec::new()
                };
                let reply = build_reply(frame.cmd, frame.req_id, ErrorCode::Ok, &body)
                    .expect("fake-board reply builds");
                if !self.drop_all {
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

    /// A `GET_CAPS` body *after* the status byte: fw 4.0.1, proto 2.2,
    /// 8 axes / 64 buttons / 4 encoders, Wi-Fi channel 6, one pin-table
    /// entry (pin 34). `encode_body` includes the status byte first, so
    /// it is stripped here.
    fn caps_body() -> Vec<u8> {
        let mut body = GetCapsReply {
            status: ErrorCode::Ok,
            board_mac: BOARD_MAC,
            firmware: (4, 0, 1),
            protocol: (2, 2),
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

    /// A `HELLO` reply for this board (protocol 2.2, flags 0).
    fn hello() -> rsh_protocol::HelloReply {
        rsh_protocol::HelloReply {
            protocol: (2, 2),
            flags: 0,
            session_echo: 0,
            board_mac: BOARD_MAC,
            firmware: (4, 0, 1),
            nonce: [0u8; 16],
            device_key: [0u8; 32],
            wifi_password: [0u8; 24],
        }
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
    async fn connect_happy_path_sends_unsigned_get_caps_first() {
        let store = MemKeyStore::new();
        let mut session = machine(store, FakeBoard::new());
        assert_eq!(session.state(), SessionState::Idle);
        let caps = session.connect(&hello()).await.expect("connect succeeds");
        assert_eq!(caps.firmware, (4, 0, 1));
        assert_eq!(caps.protocol, (2, 2));
        assert_eq!(session.state(), SessionState::Active);
        assert!(session.stream_allowed());
        assert!(session.is_active());
        // No credential store was touched: nothing is stored.
        assert_eq!(
            session
                .keystore()
                .entries
                .lock()
                .expect("test store lock")
                .len(),
            0
        );
        // Unsigned GET_CAPS is the first (and here only) control command.
        let frames = sent_frames(&session);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].cmd, Opcode::GetCaps as u8);
        assert_eq!(frames[0].req_id, 0);
        // The live session's next reqId advanced past the GET_CAPS.
        let live = session.control_client().expect("client live").session();
        assert_eq!(live.next_req_id, 1);
    }

    #[tokio::test]
    async fn connect_bytes_parses_hello_then_connects() {
        let mut session = machine(MemKeyStore::new(), FakeBoard::new());
        let bytes = hello().encode();
        assert_eq!(bytes.len(), 92);
        let caps = session
            .connect_bytes(&bytes)
            .await
            .expect("connect_bytes succeeds");
        assert_eq!(caps.firmware, (4, 0, 1));
        assert_eq!(session.state(), SessionState::Active);
    }

    #[tokio::test]
    async fn connect_bytes_rejects_malformed_hello() {
        let mut session = machine(MemKeyStore::new(), FakeBoard::new());
        let err = session
            .connect_bytes(&[0u8; 10])
            .await
            .expect_err("10-byte HELLO must fail");
        assert!(matches!(err, SessionError::HelloParse(_)), "got {err:?}");
        assert_eq!(session.state(), SessionState::Idle);
    }

    #[tokio::test]
    async fn connect_wrong_board_mac_refuses() {
        let mut session = machine(MemKeyStore::new(), FakeBoard::new());
        let mut foreign = hello();
        foreign.board_mac = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66];
        let err = session
            .connect(&foreign)
            .await
            .expect_err("foreign MAC must fail");
        assert!(matches!(err, SessionError::BoardMismatch), "got {err:?}");
        assert_eq!(session.state(), SessionState::Idle);
    }

    #[tokio::test]
    async fn connect_old_protocol_hello_refuses() {
        // A pre-redesign board (protocol 2.1) still parses, but control
        // is unsupported against it — connect refuses with InvalidHello
        // instead of attempting v2.2 control.
        let mut session = machine(MemKeyStore::new(), FakeBoard::new());
        let mut old = hello();
        old.protocol = (2, 1);
        let err = session
            .connect(&old)
            .await
            .expect_err("old-protocol HELLO must fail");
        assert!(matches!(err, SessionError::InvalidHello(_)), "got {err:?}");
        assert_eq!(session.state(), SessionState::Idle);
        assert!(session.control_client().is_none());
    }

    #[tokio::test]
    async fn connect_twice_while_active_is_already_active() {
        let mut session = machine(MemKeyStore::new(), FakeBoard::new());
        session
            .connect(&hello())
            .await
            .expect("first connect succeeds");
        let err = session
            .connect(&hello())
            .await
            .expect_err("second connect must fail");
        assert!(matches!(err, SessionError::AlreadyActive), "got {err:?}");
        assert_eq!(session.state(), SessionState::Active);
    }

    #[tokio::test]
    async fn connect_after_close_succeeds_again() {
        let mut session = machine(MemKeyStore::new(), FakeBoard::new());
        session
            .connect(&hello())
            .await
            .expect("first connect succeeds");
        session.close();
        assert_eq!(session.state(), SessionState::Idle);
        assert!(session.control_client().is_none());
        let caps = session
            .connect(&hello())
            .await
            .expect("reconnect after close succeeds");
        assert_eq!(caps.firmware, (4, 0, 1));
        assert_eq!(session.state(), SessionState::Active);
        // A fresh session: reqId restarted at 0 for the new GET_CAPS
        // (the board's send log holds both sessions' frames).
        let frames = sent_frames(&session);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[1].req_id, 0);
        assert_eq!(frames[1].cmd, Opcode::GetCaps as u8);
    }

    #[tokio::test]
    async fn get_caps_failure_leaves_idle_state() {
        let mut board = FakeBoard::new();
        board.drop_all = true; // Drop every reply: GET_CAPS times out.
        let mut session = machine(MemKeyStore::new(), board);
        let err = session
            .connect(&hello())
            .await
            .expect_err("dropped GET_CAPS must fail");
        assert!(
            matches!(
                err,
                SessionError::Control(crate::control_client::ControlError::Timeout)
            ),
            "got {err:?}"
        );
        assert_eq!(session.state(), SessionState::Idle);
        assert!(session.control_client().is_none());
        // The transport is idle again: the forwarder gets it back.
        assert!(session.take_transport().is_some());
    }

    #[tokio::test]
    async fn close_returns_to_idle() {
        let mut session = machine(MemKeyStore::new(), FakeBoard::new());
        session.connect(&hello()).await.expect("connect succeeds");
        session.close();
        assert_eq!(session.state(), SessionState::Idle);
        assert!(session.control_client().is_none());
        assert!(!session.stream_allowed());
        assert!(session.caps().is_none());
        // A fresh session connects again.
        session
            .connect(&hello())
            .await
            .expect("connect after close succeeds");
    }

    #[tokio::test]
    async fn take_transport_returns_idle_transport_after_close() {
        // H5: the release path the forwarder activator uses — the
        // transport goes back out after the session ends, so the same
        // transport can stream frames.
        let mut session = machine(MemKeyStore::new(), FakeBoard::new());
        session.connect(&hello()).await.expect("connect succeeds");
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
    async fn take_transport_returns_idle_transport_after_failed_connect() {
        // A failed activation already returned the transport to idle,
        // so the forwarder gets it back without a close() first.
        let mut board = FakeBoard::new();
        board.drop_all = true; // Drop every reply: GET_CAPS times out.
        let mut session = machine(MemKeyStore::new(), board);
        let _err = session
            .connect(&hello())
            .await
            .expect_err("dropped GET_CAPS must fail");
        assert!(session.take_transport().is_some());
    }

    #[tokio::test]
    async fn take_and_restore_control_client_round_trip() {
        // H5-control: a transaction client that must own the control
        // client borrows it out and hands it back; the session still
        // closes cleanly afterwards (transport returned).
        let mut session = machine(MemKeyStore::new(), FakeBoard::new());
        assert!(session.take_control_client().is_none(), "no client yet");
        session.connect(&hello()).await.expect("connect succeeds");
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
        assert_eq!(session.state(), SessionState::Idle);
        assert!(session.control_client().is_none());
        assert!(session.take_transport().is_some(), "transport returned");
    }

    #[tokio::test]
    async fn key_reset_deletes_stored_wifi_password_and_closes_session() {
        let store = MemKeyStore::new();
        store
            .store_wifi_password(BOARD_ID, WIFI_PASSWORD)
            .expect("pre-store password");
        let mut session = machine(store, FakeBoard::new());
        session.connect(&hello()).await.expect("connect succeeds");
        let sent_before = session
            .control_client()
            .expect("client live")
            .transport()
            .sent
            .len();
        session.key_reset().expect("key_reset succeeds");
        assert_eq!(session.state(), SessionState::Idle);
        assert!(!session.is_active());
        assert!(session.control_client().is_none());
        // The stored Wi-Fi password is gone; no wire command was sent
        // for the reset (there is no KEY_RESET opcode anymore).
        let entries = session.keystore().entries.lock().expect("test store lock");
        assert!(entries.is_empty(), "password entry must be deleted");
        drop(entries);
        let transport = session
            .take_transport()
            .expect("transport back after key_reset");
        assert_eq!(transport.sent.len(), sent_before, "no reset frame sent");
    }

    #[tokio::test]
    async fn key_reset_without_session_still_clears_password() {
        let store = MemKeyStore::new();
        store
            .store_wifi_password(BOARD_ID, WIFI_PASSWORD)
            .expect("pre-store password");
        let mut session = machine(store, FakeBoard::new());
        session
            .key_reset()
            .expect("key_reset without a session succeeds");
        assert_eq!(session.state(), SessionState::Idle);
        let entries = session.keystore().entries.lock().expect("test store lock");
        assert!(entries.is_empty(), "password entry must be deleted");
    }

    #[tokio::test]
    async fn key_reset_with_missing_password_is_ok() {
        // Nothing stored: clearing is already done, not an error.
        let mut session = machine(MemKeyStore::new(), FakeBoard::new());
        session.key_reset().expect("missing password clears fine");
    }

    #[tokio::test]
    async fn key_reset_store_failure_is_loud() {
        let failing = MemKeyStore::failing(KeystoreError::PlatformUnavailable);
        let mut session = machine(failing, FakeBoard::new());
        session
            .connect(&hello())
            .await
            .expect("connect needs no store");
        let err = session
            .key_reset()
            .expect_err("unavailable store must fail key_reset");
        assert!(
            matches!(
                err,
                SessionError::Keystore(KeystoreError::PlatformUnavailable)
            ),
            "got {err:?}"
        );
        // The session is still closed even when the delete fails.
        assert_eq!(session.state(), SessionState::Idle);
        assert!(session.control_client().is_none());
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn connect_does_not_need_credential_store() {
        // The real platform store does not exist on this Linux sandbox,
        // but connect touches no store — so it succeeds. Only
        // password-clear (which deletes the stored password) fails
        // loudly with PlatformUnavailable.
        use rsh_security::keystore::KeyStore;
        assert!(!KeyStore::store_available());
        let mut session = BoardSession::new(BOARD_ID, KeyStore::new(), FakeBoard::new())
            .with_control_timeout(Duration::from_millis(10));
        session
            .connect(&hello())
            .await
            .expect("connect succeeds without a credential store");
        assert_eq!(session.state(), SessionState::Active);
        let err = session
            .key_reset()
            .expect_err("key_reset needs the store and must fail loudly");
        assert!(
            matches!(
                err,
                SessionError::Keystore(KeystoreError::PlatformUnavailable)
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn session_error_display_is_secret_free() {
        assert_eq!(
            SessionError::BoardMismatch.to_string(),
            "HELLO board MAC does not match this session's board"
        );
        assert_eq!(
            SessionError::NotActive.to_string(),
            "no live session for this board"
        );
        assert_eq!(
            SessionError::Keystore(KeystoreError::PlatformUnavailable).to_string(),
            "credential store error: platform credential store is unavailable"
        );
        let debug = format!(
            "{:?}",
            BoardSession::new(BOARD_ID, MemKeyStore::new(), FakeBoard::new())
        );
        assert!(
            !debug.contains(BOARD_ID),
            "Debug leaked the board id: {debug}"
        );
    }

    #[test]
    fn session_state_transitions() {
        assert_ne!(SessionState::Idle, SessionState::Active);
    }
}
