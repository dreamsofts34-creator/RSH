//! HMAC control client (Phase C3).
//!
//! Sends the twelve control commands (`GET_CAPS` … `SET_PASSWORD`,
//! opcodes `0x01`–`0x0C`) over any [`ControlTransport`] and verifies the
//! board's replies. Every request frame is built with
//! [`rsh_protocol::control::build_frame`] — tag `HMAC-SHA256(key, nonce ‖
//! counter_LE ‖ cmd ‖ payload)[0:16]` — and every reply is verified with
//! [`ControlFrame::verify`] before its status byte is trusted.
//!
//! Retry rule (§3.2): up to 3 sends at 250 ms intervals; every retry keeps
//! the same `reqId` and uses a **new** counter. Only a timeout retries — a
//! bad tag, a replay, a board error, or a malformed frame fails at once,
//! because resending the same request cannot fix those. The board answers
//! a same-`reqId` retry from its duplicate cache, so idempotent commands
//! are never executed twice.
//!
//! Replay rule (§3.2, RFC 1982 serial arithmetic): a reply counter is a
//! replay when it is `≤` the highest counter accepted so far in this
//! session, so the wrap `0xFFFFFFFF → 0` is a *new* counter, not a replay.
//! Gaps above the maximum are accepted — a lost packet must not wedge
//! the session.
//!
//! The [`Session`] (device key, nonce, counter) is defined here and driven
//! by the C4 pairing worker in Wave 2: pairing supplies the key and nonce,
//! and a new session (new nonce) restarts the counter at 0.

use std::time::Duration;

use rsh_protocol::control::{
    ControlFrame, ErrorCode, GetCfgRequest, LedTestPattern, MAX_PAYLOAD_LEN, MAX_RETRIES, Opcode,
    PinProbeRequest, RETRY_INTERVAL_MS, ReplayTracker, build_set_password_payload,
};
use rsh_protocol::{ConfigError, ControlParseError, GetCapsReply, SetCfgChunk};

use crate::{ControlTransport, TransportError};

/// Overwrite `buf` with zeros in a way the compiler must not optimise
/// away (volatile writes plus `black_box`, same primitive as
/// `session.rs`). Used only by the `Drop` impls below.
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

/// Per-session control-channel context: the 32-byte device key, the
/// 16-byte nonce, and the next counter to use.
///
/// The counter starts at 0 for a fresh session (D30) and advances by one
/// for every send, wrapping `0xFFFFFFFF → 0`. A new session — new nonce,
/// e.g. after a board reboot — restarts the counter at 0; see
/// [`Session::reset`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    /// Per-device HMAC key (32 bytes, never logged or exported).
    pub key: [u8; 32],
    /// Session nonce (16 bytes, from the `HELLO` reply).
    pub nonce: [u8; 16],
    /// Counter for the next send.
    pub counter: u32,
}

impl Session {
    /// Create a session with the counter at 0.
    #[must_use]
    pub const fn new(key: [u8; 32], nonce: [u8; 16]) -> Self {
        Self {
            key,
            nonce,
            counter: 0,
        }
    }

    /// Take the counter for the next send and advance it (wrapping).
    pub fn next_counter(&mut self) -> u32 {
        let counter = self.counter;
        self.counter = self.counter.wrapping_add(1);
        counter
    }

    /// Start a new session with a fresh nonce: counter restarts at 0.
    pub fn reset(&mut self, nonce: [u8; 16]) {
        self.nonce = nonce;
        self.counter = 0;
    }
}

impl Drop for Session {
    /// Zeroize the device key when the session is dropped, so a key
    /// never lingers in memory after the session ends (M1). The nonce
    /// is not key material and is left alone. Callers that take the
    /// session out via [`ControlClient::into_inner`] move the key out
    /// whole, so this drop does not run for the moved-out copy — the
    /// new owner (e.g. [`BoardSession::close`]) zeroizes it instead.
    fn drop(&mut self) {
        zeroize(&mut self.key);
    }
}

impl<T: ControlTransport> Drop for ControlClient<T> {
    /// Zeroize the session key when the client is dropped (M1) — a
    /// dropped client must never leave the device key in memory.
    /// [`ControlClient::into_inner`] moves the session out whole, so
    /// this drop does not run for a taken-out client.
    fn drop(&mut self) {
        zeroize(&mut self.session.key);
    }
}

/// Error from a control command.
#[derive(Debug)]
pub enum ControlError {
    /// No reply within the per-attempt timeout after all attempts.
    Timeout,
    /// A reply failed HMAC tag verification (constant-time compare).
    BadTag,
    /// A reply counter is a replay (RFC 1982: `≤` max-seen, new `reqId`).
    Replay,
    /// The board answered with a non-zero status byte.
    BoardError(ErrorCode),
    /// The request payload (or `SET_CFG` chunk) exceeds its size limit.
    TooBig,
    /// The underlying transport failed.
    Transport(TransportError),
    /// A reply could not be parsed (`TooShort` / `BadMagic` /
    /// `BadLength`, or an empty / unknown-status reply payload).
    Parse(ControlParseError),
    /// A reply arrived for a different command or `reqId` than the one
    /// in flight (only surfaced when no matching reply arrives at all —
    /// stray frames are otherwise skipped while waiting).
    UnexpectedReply,
    /// A caller-supplied argument violates the protocol (unknown opcode,
    /// bad `LED_TEST` pattern id, bad `PIN_PROBE` duration/mode, bad
    /// `SET_PASSWORD` password, out-of-range `SET_CFG` chunk).
    InvalidArgument(&'static str),
}

impl std::fmt::Display for ControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout => write!(f, "control request timed out after all retries"),
            Self::BadTag => write!(f, "control reply HMAC tag verification failed"),
            Self::Replay => write!(f, "control reply counter is a replay"),
            Self::BoardError(code) => write!(f, "board rejected the command: {code:?}"),
            Self::TooBig => write!(f, "control payload exceeds the size limit"),
            Self::Transport(e) => write!(f, "control transport error: {e}"),
            Self::Parse(e) => write!(f, "control reply parse error: {e}"),
            Self::UnexpectedReply => {
                write!(f, "control reply was for a different command or reqId")
            }
            Self::InvalidArgument(msg) => write!(f, "invalid control argument: {msg}"),
        }
    }
}

impl std::error::Error for ControlError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transport(e) => Some(e),
            Self::Parse(e) => Some(e),
            _ => None,
        }
    }
}

impl From<TransportError> for ControlError {
    fn from(e: TransportError) -> Self {
        Self::Transport(e)
    }
}

impl From<ControlParseError> for ControlError {
    fn from(e: ControlParseError) -> Self {
        Self::Parse(e)
    }
}

/// HMAC control client, generic over the transport.
///
/// The client is generic — never `&dyn` — because [`ControlTransport`]
/// uses desugared `impl Future` methods, which are not dyn-compatible.
/// One client owns one [`Session`]; the `reqId` increments per command
/// (wrapping) and the session counter increments per send.
pub struct ControlClient<T: ControlTransport> {
    transport: T,
    session: Session,
    next_req_id: u16,
    replay: ReplayTracker,
    timeout: Duration,
    max_attempts: u32,
}

impl<T: ControlTransport> ControlClient<T> {
    /// Create a client: 3 attempts × 250 ms per request (protocol §3.2).
    #[must_use]
    pub fn new(transport: T, session: Session) -> Self {
        Self {
            transport,
            session,
            next_req_id: 0,
            replay: ReplayTracker::new(),
            timeout: Duration::from_millis(RETRY_INTERVAL_MS),
            max_attempts: MAX_RETRIES,
        }
    }

    /// Override the per-attempt reply timeout (tests use a few ms so a
    /// full 3-attempt timeout stays fast; production keeps 250 ms).
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Override the number of sends per request (tests use 1–2 to prove
    /// the attempt count; production keeps 3). Clamped to at least 1.
    #[must_use]
    pub fn with_max_attempts(mut self, max_attempts: u32) -> Self {
        self.max_attempts = max_attempts.max(1);
        self
    }

    /// The session (key, nonce, next counter).
    #[must_use]
    pub fn session(&self) -> &Session {
        &self.session
    }

    /// Mutable access to the session (C4 pairing rotates key/nonce here).
    #[must_use]
    pub fn session_mut(&mut self) -> &mut Session {
        &mut self.session
    }

    /// The underlying transport.
    #[must_use]
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// Mutable access to the underlying transport.
    #[must_use]
    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }

    /// Per-attempt reply timeout.
    #[must_use]
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Number of sends per request.
    #[must_use]
    pub fn max_attempts(&self) -> u32 {
        self.max_attempts
    }

    /// Consume the client, returning the transport and the session.
    /// The session (and its key) moves out whole, so the client's
    /// `Drop` impl does not run for it — the new owner zeroizes it.
    #[must_use]
    pub fn into_inner(self) -> (T, Session) {
        let this = std::mem::ManuallyDrop::new(self);
        // SAFETY: `this` is never dropped (it is `ManuallyDrop`), so
        // reading the fields out by pointer copy is the only access
        // and no value is used twice.
        unsafe {
            (
                std::ptr::read(std::ptr::addr_of!(this.transport)),
                std::ptr::read(std::ptr::addr_of!(this.session)),
            )
        }
    }

    /// Send one command and return the reply body (status byte stripped).
    ///
    /// Retries only on timeout: same `reqId`, new counter, up to
    /// [`ControlClient::max_attempts`] sends. A bad tag, a replay, a
    /// board error, or a malformed reply fails immediately.
    ///
    /// # Errors
    ///
    /// Returns [`ControlError::TooBig`] when `payload.len() > 1024`,
    /// [`ControlError::InvalidArgument`] for an unknown opcode,
    /// [`ControlError::Timeout`] after all attempts, [`ControlError::BadTag`]
    /// / [`ControlError::Replay`] / [`ControlError::Parse`] for a bad
    /// reply, [`ControlError::BoardError`] for a non-zero board status,
    /// and [`ControlError::Transport`] when the transport fails.
    pub async fn execute(&mut self, cmd: u8, payload: &[u8]) -> Result<Vec<u8>, ControlError> {
        let frame = self.execute_frame(cmd, payload).await?;
        Self::frame_body(&frame)
    }

    /// Send one command and return the reply status plus the body (status
    /// byte stripped) — even when the board reports a non-zero status.
    ///
    /// [`ControlClient::execute`] discards the body on board errors, but
    /// some replies carry data with the error status (notably `COMMIT`:
    /// `[errCount, errCode…]` with `PIN_CONFLICT` / `UNSUPPORTED`). Use
    /// this method when the error body matters; it retries exactly like
    /// `execute` (timeout only, same `reqId`, new counter).
    ///
    /// # Errors
    ///
    /// Same as [`ControlClient::execute`], except a non-zero board status
    /// is returned as `Ok((status, body))` instead of
    /// [`ControlError::BoardError`].
    pub async fn execute_with_status(
        &mut self,
        cmd: u8,
        payload: &[u8],
    ) -> Result<(ErrorCode, Vec<u8>), ControlError> {
        let frame = self.execute_frame(cmd, payload).await?;
        let status = Self::frame_status(&frame)?;
        Ok((status, frame.payload[1..].to_vec()))
    }

    /// `GET_CAPS` — board capabilities, pin table, firmware/protocol
    /// versions, Wi-Fi channel, caps flags.
    ///
    /// # Errors
    ///
    /// Same as [`ControlClient::execute`], plus [`ControlError::Parse`]
    /// when the caps body does not match the §3.4 layout.
    pub async fn get_caps(&mut self) -> Result<GetCapsReply, ControlError> {
        let frame = self.execute_frame(Opcode::GetCaps as u8, &[]).await?;
        let status = Self::frame_status(&frame)?;
        if status != ErrorCode::Ok {
            return Err(ControlError::BoardError(status));
        }
        GetCapsReply::from_payload(&frame.payload).map_err(ControlError::Parse)
    }

    /// `GET_CFG` — read `want_len` bytes of the stored config blob at
    /// `offset`. Returns the raw chunk bytes (status byte stripped).
    ///
    /// # Errors
    ///
    /// Same as [`ControlClient::execute`]; [`ControlError::TooBig`] when
    /// `want_len > 1024`.
    pub async fn get_cfg(&mut self, offset: u16, want_len: u16) -> Result<Vec<u8>, ControlError> {
        if usize::from(want_len) > MAX_PAYLOAD_LEN {
            return Err(ControlError::TooBig);
        }
        let request = GetCfgRequest { offset, want_len };
        self.execute(Opcode::GetCfg as u8, &request.encode()).await
    }

    /// `SET_CFG` — stage one config chunk (≤ 1024 B data, total ≤ 2048 B,
    /// chunk CRC16 over the data — all enforced by [`SetCfgChunk`]).
    /// The full blob is sent chunk by chunk; the C5 transaction client
    /// drives `COMMIT` / `CONFIRM` afterwards.
    ///
    /// # Errors
    ///
    /// Same as [`ControlClient::execute`]; [`ControlError::TooBig`] for
    /// an oversize chunk/total, [`ControlError::InvalidArgument`] when
    /// `offset + data.len() > total`.
    pub async fn set_cfg(&mut self, chunk: &SetCfgChunk) -> Result<(), ControlError> {
        let payload = chunk.encode().map_err(|e| match e {
            ConfigError::TooBig => ControlError::TooBig,
            _ => ControlError::InvalidArgument("SET_CFG chunk offset or length out of range"),
        })?;
        self.execute(Opcode::SetCfg as u8, &payload).await?;
        Ok(())
    }

    /// `COMMIT` — stage the uploaded config for the 30 s confirm window.
    ///
    /// # Errors
    ///
    /// Same as [`ControlClient::execute`].
    pub async fn commit(&mut self) -> Result<(), ControlError> {
        self.execute(Opcode::Commit as u8, &[]).await?;
        Ok(())
    }

    /// `CONFIRM` — make the staged config permanent (within 30 s of
    /// `COMMIT`, else the board rolls back).
    ///
    /// # Errors
    ///
    /// Same as [`ControlClient::execute`].
    pub async fn confirm(&mut self) -> Result<(), ControlError> {
        self.execute(Opcode::Confirm as u8, &[]).await?;
        Ok(())
    }

    /// `REVERT` — discard the staged config, keep the last-good one.
    ///
    /// # Errors
    ///
    /// Same as [`ControlClient::execute`].
    pub async fn revert(&mut self) -> Result<(), ControlError> {
        self.execute(Opcode::Revert as u8, &[]).await?;
        Ok(())
    }

    /// `PIN_PROBE` — start a probe; the board then streams `"RSHP"`
    /// samples on port 4211 (see `rsh_protocol::probe`). This call only
    /// returns the command acknowledgement, not the samples.
    ///
    /// # Errors
    ///
    /// Same as [`ControlClient::execute`];
    /// [`ControlError::InvalidArgument`] when `duration_ms` is outside
    /// 100–10000 or `mode` is not 0 (digital) / 1 (ADC mV).
    pub async fn pin_probe(&mut self, request: &PinProbeRequest) -> Result<(), ControlError> {
        if !(100..=10_000).contains(&request.duration_ms) || request.mode > 1 {
            return Err(ControlError::InvalidArgument(
                "PIN_PROBE duration must be 100..10000 ms and mode 0 or 1",
            ));
        }
        self.execute(Opcode::PinProbe as u8, &request.encode())
            .await?;
        Ok(())
    }

    /// `FACTORY_RESET` — reset config to factory defaults. The device
    /// key is *not* touched by this command.
    ///
    /// # Errors
    ///
    /// Same as [`ControlClient::execute`].
    pub async fn factory_reset(&mut self) -> Result<(), ControlError> {
        self.execute(Opcode::FactoryReset as u8, &[]).await?;
        Ok(())
    }

    /// `REBOOT` — reboot the board (the reply arrives before the reboot).
    ///
    /// # Errors
    ///
    /// Same as [`ControlClient::execute`].
    pub async fn reboot(&mut self) -> Result<(), ControlError> {
        self.execute(Opcode::Reboot as u8, &[]).await?;
        Ok(())
    }

    /// `LED_TEST` — run an LED pattern (8-byte payload, board caps the
    /// run at 60 s, SEC-H-10).
    ///
    /// # Errors
    ///
    /// Same as [`ControlClient::execute`];
    /// [`ControlError::InvalidArgument`] for an unknown `pattern_id`
    /// (0 OFF, 1 SOLID, 2 BLINK, 3 CHASE, 4 RAINBOW).
    pub async fn led_test(&mut self, pattern: &LedTestPattern) -> Result<(), ControlError> {
        if pattern.pattern_id > 4 {
            return Err(ControlError::InvalidArgument(
                "LED_TEST pattern_id must be 0..=4",
            ));
        }
        self.execute(Opcode::LedTest as u8, &pattern.encode())
            .await?;
        Ok(())
    }

    /// `KEY_RESET` — rotate the device key (only while key-reset is
    /// armed on the board; the new key arrives via a fresh `HELLO`).
    ///
    /// # Errors
    ///
    /// Same as [`ControlClient::execute`].
    pub async fn key_reset(&mut self) -> Result<(), ControlError> {
        self.execute(Opcode::KeyReset as u8, &[]).await?;
        Ok(())
    }

    /// `SET_PASSWORD` — change the board AP password (8–63 printable
    /// ASCII bytes, no space; the board restarts its AP after replying).
    ///
    /// # Errors
    ///
    /// Same as [`ControlClient::execute`];
    /// [`ControlError::InvalidArgument`] when the password violates the
    /// length/charset rules.
    pub async fn set_password(&mut self, password: &[u8]) -> Result<(), ControlError> {
        let payload = build_set_password_payload(password).ok_or(ControlError::InvalidArgument(
            "password must be 8..=63 printable ASCII bytes (0x21..=0x7E)",
        ))?;
        self.execute(Opcode::SetPassword as u8, &payload).await?;
        Ok(())
    }

    /// Send one command with retries and return the verified reply frame
    /// (status byte *not* interpreted — callers check it).
    async fn execute_frame(
        &mut self,
        cmd: u8,
        payload: &[u8],
    ) -> Result<ControlFrame, ControlError> {
        if !Opcode::is_known(cmd) {
            return Err(ControlError::InvalidArgument("unknown control opcode"));
        }
        if payload.len() > MAX_PAYLOAD_LEN {
            return Err(ControlError::TooBig);
        }
        let req_id = self.next_req_id;
        self.next_req_id = self.next_req_id.wrapping_add(1);
        let mut attempts = 0u32;
        while attempts < self.max_attempts {
            attempts += 1;
            let counter = self.session.next_counter();
            let bytes = ControlFrame {
                cmd,
                req_id,
                counter,
                payload: payload.to_vec(),
            }
            .encode(&self.session.key, &self.session.nonce)
            .map_err(ControlError::Parse)?;
            self.transport
                .send_control(&bytes)
                .await
                .map_err(ControlError::Transport)?;
            if let Some(frame) = self.wait_for_reply(cmd, req_id).await? {
                return Ok(frame);
            }
            // Attempt timed out: retry with the same reqId, new counter.
        }
        Err(ControlError::Timeout)
    }

    /// Wait up to the per-attempt timeout for the reply to `req_id`.
    /// Returns `Ok(None)` on timeout (caller retries), `Ok(Some(frame))`
    /// for a verified, matching, non-replay reply. Stray frames for other
    /// commands/`reqId`s are skipped while time remains.
    ///
    /// H1/M-8: match first, verify second. The frame is parsed
    /// structurally and matched on `cmd`/`reqId` *before* any HMAC
    /// check; a stray frame (another command, another `reqId`, or a
    /// late reply to a previous command) is skipped without verifying
    /// its tag — so a stray frame with a corrupted tag can never kill
    /// the in-flight command with a spurious `BadTag`. Only the
    /// matching frame's tag is verified, and a bad tag on *that*
    /// frame still fails at once.
    async fn wait_for_reply(
        &mut self,
        cmd: u8,
        req_id: u16,
    ) -> Result<Option<ControlFrame>, ControlError> {
        let deadline = tokio::time::Instant::now() + self.timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }
            let received =
                tokio::time::timeout(remaining, self.transport.recv_control(remaining)).await;
            let raw = match received {
                Err(_) | Ok(Err(TransportError::Timeout)) => return Ok(None),
                Ok(Err(e)) => return Err(ControlError::Transport(e)),
                Ok(Ok(raw)) => raw,
            };
            // Structure first: a frame too short / bad magic / bad
            // length has no trustworthy cmd/reqId to match on, so it
            // fails like before. A structurally sound frame is
            // matched before its tag is touched.
            let frame = match ControlFrame::parse_structure(&raw) {
                Ok(frame) => frame,
                Err(e) => return Err(ControlError::Parse(e)),
            };
            if frame.cmd != (cmd | 0x80) || frame.req_id != req_id {
                continue;
            }
            if let Err(e) = frame.verify_tag(&self.session.key, &self.session.nonce, &raw) {
                return Err(match e {
                    ControlParseError::BadTag => ControlError::BadTag,
                    other => ControlError::Parse(other),
                });
            }
            if self.replay.is_replay(frame.counter) {
                return Err(ControlError::Replay);
            }
            self.replay.observe(frame.counter);
            return Ok(Some(frame));
        }
    }

    /// Reply status byte of a verified frame.
    fn frame_status(frame: &ControlFrame) -> Result<ErrorCode, ControlError> {
        let Some(status_byte) = frame.payload.first() else {
            return Err(ControlError::Parse(ControlParseError::BadLength));
        };
        ErrorCode::from_u8(*status_byte).ok_or(ControlError::Parse(ControlParseError::BadLength))
    }

    /// Reply body: status byte checked, `Ok` → bytes after the status.
    fn frame_body(frame: &ControlFrame) -> Result<Vec<u8>, ControlError> {
        let status = Self::frame_status(frame)?;
        if status == ErrorCode::Ok {
            Ok(frame.payload[1..].to_vec())
        } else {
            Err(ControlError::BoardError(status))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, VecDeque};
    use std::time::Duration;

    use rsh_protocol::control::{ErrorCode, LedTestPattern, Opcode, build_frame, build_reply};
    use rsh_protocol::{ConfigError, GetCapsReply, PinProbeRequest, PinTableEntry, SetCfgChunk};
    use rsh_protocol::{ControlFrame, ControlParseError};

    use super::{ControlClient, ControlError, Session};
    use crate::{ControlTransport, TransportError};

    /// Test device key / nonce (shared by client and test doubles).
    const KEY: [u8; 32] = [0x42; 32];
    /// Test session nonce.
    const NONCE: [u8; 16] = [0x24; 16];

    /// Build a client with a 10 ms per-attempt timeout so a full
    /// 3-attempt timeout stays fast in tests.
    fn client<T: ControlTransport>(transport: T) -> ControlClient<T> {
        ControlClient::new(transport, Session::new(KEY, NONCE))
            .with_timeout(Duration::from_millis(10))
    }

    /// Build a verified reply frame for `req_id` / `counter`.
    fn reply(cmd: u8, req_id: u16, counter: u32, status: ErrorCode, body: &[u8]) -> Vec<u8> {
        build_reply(&KEY, &NONCE, cmd, req_id, counter, status, body).expect("test reply builds")
    }

    /// A reply whose tag is corrupted (flip the last tag byte).
    fn reply_bad_tag(
        cmd: u8,
        req_id: u16,
        counter: u32,
        status: ErrorCode,
        body: &[u8],
    ) -> Vec<u8> {
        let mut bytes = reply(cmd, req_id, counter, status, body);
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        bytes
    }

    /// One scripted receive outcome for [`ScriptedTransport`].
    enum ScriptedRecv {
        /// Deliver these raw bytes.
        Bytes(Vec<u8>),
        /// Fail the receive with this transport error.
        Err(TransportError),
    }

    /// Transport that plays back scripted replies and records sends.
    struct ScriptedTransport {
        replies: VecDeque<ScriptedRecv>,
        sent: Vec<Vec<u8>>,
        fail_send: bool,
    }

    impl ScriptedTransport {
        fn new(replies: Vec<ScriptedRecv>) -> Self {
            Self {
                replies: replies.into(),
                sent: Vec::new(),
                fail_send: false,
            }
        }

        fn with_send_failure() -> Self {
            Self {
                replies: VecDeque::new(),
                sent: Vec::new(),
                fail_send: true,
            }
        }
    }

    impl ControlTransport for ScriptedTransport {
        fn send_control(
            &mut self,
            bytes: &[u8],
        ) -> impl Future<Output = Result<(), TransportError>> + Send {
            let owned = bytes.to_vec();
            async move {
                if self.fail_send {
                    return Err(TransportError::Closed);
                }
                self.sent.push(owned);
                Ok(())
            }
        }

        fn recv_control(
            &mut self,
            _timeout: Duration,
        ) -> impl Future<Output = Result<Vec<u8>, TransportError>> + Send {
            std::future::ready(match self.replies.pop_front() {
                Some(ScriptedRecv::Bytes(bytes)) => Ok(bytes),
                Some(ScriptedRecv::Err(e)) => Err(e),
                None => Err(TransportError::Timeout),
            })
        }
    }

    /// A fake board: verifies tags, answers same-`reqId` retries from a
    /// cache without re-executing, counts executions per `reqId`, and can
    /// drop the next reply (to force a client retry).
    struct FakeBoard {
        pending: Option<Vec<u8>>,
        sent: Vec<Vec<u8>>,
        drop_next: bool,
        executions: HashMap<u16, u32>,
        cached: HashMap<u16, Vec<u8>>,
        status: ErrorCode,
        body: Vec<u8>,
        corrupt_tag: bool,
    }

    impl FakeBoard {
        fn new(status: ErrorCode, body: Vec<u8>) -> Self {
            Self {
                pending: None,
                sent: Vec::new(),
                drop_next: false,
                executions: HashMap::new(),
                cached: HashMap::new(),
                status,
                body,
                corrupt_tag: false,
            }
        }

        fn executions_for(&self, req_id: u16) -> u32 {
            self.executions.get(&req_id).copied().unwrap_or(0)
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
                    self.pending = None;
                    return Ok(());
                };
                // Tag check first: a bad tag is answered BAD_TAG, never executed.
                if ControlFrame::verify(&owned, &KEY, &NONCE).is_err() {
                    let reply = build_reply(
                        &KEY,
                        &NONCE,
                        frame.cmd,
                        frame.req_id,
                        frame.counter,
                        ErrorCode::BadTag,
                        &[],
                    )
                    .expect("fake-board BAD_TAG reply builds");
                    self.pending = Some(reply);
                    return Ok(());
                }
                // Duplicate cache: same reqId re-answered, never re-executed.
                if let Some(cached) = self.cached.get(&frame.req_id) {
                    self.pending = Some(cached.clone());
                    return Ok(());
                }
                *self.executions.entry(frame.req_id).or_insert(0) += 1;
                let mut reply = build_reply(
                    &KEY,
                    &NONCE,
                    frame.cmd,
                    frame.req_id,
                    frame.counter,
                    self.status,
                    &self.body,
                )
                .expect("fake-board reply builds");
                if self.corrupt_tag {
                    let last = reply.len() - 1;
                    reply[last] ^= 0xFF;
                }
                self.cached.insert(frame.req_id, reply.clone());
                if self.drop_next {
                    self.drop_next = false;
                    self.pending = None;
                } else {
                    self.pending = Some(reply);
                }
                Ok(())
            }
        }

        fn recv_control(
            &mut self,
            _timeout: Duration,
        ) -> impl Future<Output = Result<Vec<u8>, TransportError>> + Send {
            std::future::ready(self.pending.take().ok_or(TransportError::Timeout))
        }
    }

    /// A `GET_CAPS` body *after* the status byte (what `build_reply`
    /// appends): fw 4.0.1, proto 2.1, 8 axes / 64 buttons / 4 encoders,
    /// Wi-Fi channel 6, one pin-table entry (pin 34). `encode_body`
    /// includes the status byte first, so it is stripped here.
    fn caps_body() -> Vec<u8> {
        let mut body = GetCapsReply {
            status: ErrorCode::Ok,
            board_mac: [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF],
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

    #[tokio::test]
    async fn get_caps_happy_path_fake_board() {
        let board = FakeBoard::new(ErrorCode::Ok, caps_body());
        let mut c = client(board);
        let caps = c.get_caps().await.expect("GET_CAPS succeeds");
        assert_eq!(caps.firmware, (4, 0, 1));
        assert_eq!(caps.protocol, (2, 1));
        assert_eq!(caps.n_axes_max, 8);
        assert_eq!(caps.n_btn_max, 64);
        assert_eq!(caps.n_enc_max, 4);
        assert_eq!(caps.wifi_channel, 6);
        assert_eq!(caps.caps_flags, 0);
        assert_eq!(caps.board_mac, [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
        assert_eq!(caps.pin_table.len(), 1);
        assert_eq!(caps.pin_table[0].pin, 34);
        assert_eq!(caps.pin_table[0].kind, 1);
        assert_eq!(caps.pin_table[0].module, 1);
        assert!(caps.pin_table[0].is_adc());
    }

    #[tokio::test]
    async fn get_caps_happy_path_scripted() {
        let transport = ScriptedTransport::new(vec![ScriptedRecv::Bytes(reply(
            Opcode::GetCaps as u8,
            0,
            0,
            ErrorCode::Ok,
            &caps_body(),
        ))]);
        let mut c = client(transport);
        let caps = c.get_caps().await.expect("GET_CAPS succeeds");
        assert_eq!(caps.firmware, (4, 0, 1));
        assert_eq!(caps.pin_table.len(), 1);
    }

    #[tokio::test]
    async fn bad_tag_reply_is_bad_tag_error() {
        let transport = ScriptedTransport::new(vec![ScriptedRecv::Bytes(reply_bad_tag(
            Opcode::GetCaps as u8,
            0,
            0,
            ErrorCode::Ok,
            &caps_body(),
        ))]);
        let mut c = client(transport);
        let err = c.get_caps().await.expect_err("bad tag must fail");
        assert!(matches!(err, ControlError::BadTag), "got {err:?}");
        // No retry on a bad tag: exactly one send.
        assert_eq!(c.transport().sent.len(), 1);
    }

    #[tokio::test]
    async fn replayed_reply_counter_is_replay_error() {
        // First command succeeds (counter 0 accepted, max_seen = 0).
        let transport = ScriptedTransport::new(vec![
            ScriptedRecv::Bytes(reply(
                Opcode::GetCaps as u8,
                0,
                0,
                ErrorCode::Ok,
                &caps_body(),
            )),
            // Second command sends counter 1, but the reply echoes the
            // old counter 0 with a valid tag: a replay.
            ScriptedRecv::Bytes(reply(
                Opcode::GetCaps as u8,
                1,
                0,
                ErrorCode::Ok,
                &caps_body(),
            )),
        ]);
        let mut c = client(transport);
        c.get_caps().await.expect("first GET_CAPS succeeds");
        let err = c.get_caps().await.expect_err("replayed counter must fail");
        assert!(matches!(err, ControlError::Replay), "got {err:?}");
    }

    #[tokio::test]
    async fn counter_wrap_is_new_not_replay() {
        let session = Session {
            counter: 0xFFFF_FFFF,
            ..Session::new(KEY, NONCE)
        };
        let transport = ScriptedTransport::new(vec![
            ScriptedRecv::Bytes(reply(
                Opcode::GetCaps as u8,
                0,
                0xFFFF_FFFF,
                ErrorCode::Ok,
                &caps_body(),
            )),
            // Counter wraps 0xFFFFFFFF -> 0: new, not a replay (RFC 1982).
            ScriptedRecv::Bytes(reply(
                Opcode::GetCaps as u8,
                1,
                0,
                ErrorCode::Ok,
                &caps_body(),
            )),
        ]);
        let mut c = ControlClient::new(transport, session).with_timeout(Duration::from_millis(10));
        c.get_caps().await.expect("counter 0xFFFFFFFF accepted");
        c.get_caps().await.expect("wrapped counter 0 accepted");
    }

    #[tokio::test]
    async fn board_error_codes_mapped() {
        for (code, name) in [
            (ErrorCode::BadTag, "BAD_TAG"),
            (ErrorCode::Replay, "REPLAY"),
            (ErrorCode::BadCrc, "BAD_CRC"),
            (ErrorCode::TooBig, "TOO_BIG"),
            (ErrorCode::PinConflict, "PIN_CONFLICT"),
            (ErrorCode::Unsupported, "UNSUPPORTED"),
            (ErrorCode::Busy, "BUSY"),
        ] {
            let transport = ScriptedTransport::new(vec![ScriptedRecv::Bytes(reply(
                Opcode::Commit as u8,
                0,
                0,
                code,
                &[],
            ))]);
            let mut c = client(transport);
            let err = c.commit().await.expect_err("board error must fail");
            assert!(
                matches!(err, ControlError::BoardError(got) if got == code),
                "{name}: got {err:?}"
            );
        }
    }

    #[tokio::test]
    async fn get_caps_busy_is_board_error() {
        let board = FakeBoard::new(ErrorCode::Busy, Vec::new());
        let mut c = client(board);
        let err = c.get_caps().await.expect_err("BUSY must fail");
        assert!(
            matches!(err, ControlError::BoardError(ErrorCode::Busy)),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn timeout_after_three_attempts_same_req_id_new_counter() {
        let transport = ScriptedTransport::new(vec![]);
        let mut c = client(transport);
        let err = c
            .execute(Opcode::GetCaps as u8, &[])
            .await
            .expect_err("silence must time out");
        assert!(matches!(err, ControlError::Timeout), "got {err:?}");
        let sent = &c.transport().sent;
        assert_eq!(sent.len(), 3);
        let frames: Vec<ControlFrame> = sent
            .iter()
            .map(|bytes| ControlFrame::parse_structure(bytes).expect("sent frame parses"))
            .collect();
        assert!(frames.iter().all(|f| f.req_id == frames[0].req_id));
        assert_eq!(frames[0].counter, 0);
        assert_eq!(frames[1].counter, 1);
        assert_eq!(frames[2].counter, 2);
    }

    #[tokio::test]
    async fn max_attempts_is_configurable() {
        let transport = ScriptedTransport::new(vec![]);
        let mut c = client(transport).with_max_attempts(2);
        let err = c
            .execute(Opcode::GetCaps as u8, &[])
            .await
            .expect_err("silence must time out");
        assert!(matches!(err, ControlError::Timeout), "got {err:?}");
        assert_eq!(c.transport().sent.len(), 2);
    }

    #[tokio::test]
    async fn retry_after_dropped_reply_does_not_reexecute() {
        let mut board = FakeBoard::new(ErrorCode::Ok, caps_body());
        board.drop_next = true;
        let mut c = client(board);
        let caps = c.get_caps().await.expect("retry succeeds");
        assert_eq!(caps.firmware, (4, 0, 1));
        // Same reqId (0) on both sends, new counter on the retry.
        let sent = &c.transport().sent;
        assert_eq!(sent.len(), 2);
        let first = ControlFrame::parse_structure(&sent[0]).expect("first send parses");
        let second = ControlFrame::parse_structure(&sent[1]).expect("second send parses");
        assert_eq!(first.req_id, second.req_id);
        assert_ne!(first.counter, second.counter);
        // The board executed the command exactly once.
        assert_eq!(c.transport().executions_for(0), 1);
    }

    #[tokio::test]
    async fn truncated_reply_is_parse_error_not_panic() {
        let full = reply(Opcode::GetCaps as u8, 0, 0, ErrorCode::Ok, &caps_body());
        let transport = ScriptedTransport::new(vec![ScriptedRecv::Bytes(full[..5].to_vec())]);
        let mut c = client(transport);
        let err = c.get_caps().await.expect_err("5-byte reply must fail");
        assert!(
            matches!(err, ControlError::Parse(ControlParseError::TooShort)),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn bad_length_reply_is_parse_error() {
        let full = reply(Opcode::GetCaps as u8, 0, 0, ErrorCode::Ok, &caps_body());
        // 20 bytes: header readable, but 13 + len + 16 != 20.
        let transport = ScriptedTransport::new(vec![ScriptedRecv::Bytes(full[..20].to_vec())]);
        let mut c = client(transport);
        let err = c
            .get_caps()
            .await
            .expect_err("length-mismatch reply must fail");
        assert!(
            matches!(err, ControlError::Parse(ControlParseError::BadLength)),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn bad_magic_reply_is_parse_error() {
        let mut bytes = reply(Opcode::GetCaps as u8, 0, 0, ErrorCode::Ok, &caps_body());
        bytes[0] = b'X';
        let transport = ScriptedTransport::new(vec![ScriptedRecv::Bytes(bytes)]);
        let mut c = client(transport);
        let err = c.get_caps().await.expect_err("bad-magic reply must fail");
        assert!(
            matches!(err, ControlError::Parse(ControlParseError::BadMagic)),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn empty_reply_payload_is_parse_error() {
        let bytes = build_frame(&KEY, &NONCE, 0x81, 0, 0, &[]).expect("empty reply builds");
        let transport = ScriptedTransport::new(vec![ScriptedRecv::Bytes(bytes)]);
        let mut c = client(transport);
        let err = c.get_caps().await.expect_err("empty reply must fail");
        assert!(
            matches!(err, ControlError::Parse(ControlParseError::BadLength)),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn unknown_status_byte_is_parse_error() {
        // Status 0x09 is not in the 0..=7 error table.
        let bytes = build_frame(&KEY, &NONCE, 0x81, 0, 0, &[0x09]).expect("reply builds");
        let transport = ScriptedTransport::new(vec![ScriptedRecv::Bytes(bytes)]);
        let mut c = client(transport);
        let err = c.get_caps().await.expect_err("unknown status must fail");
        assert!(
            matches!(err, ControlError::Parse(ControlParseError::BadLength)),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn stray_reply_for_other_req_id_is_skipped_then_timeout() {
        let transport = ScriptedTransport::new(vec![ScriptedRecv::Bytes(reply(
            Opcode::GetCaps as u8,
            999,
            0,
            ErrorCode::Ok,
            &caps_body(),
        ))]);
        let mut c = client(transport);
        let err = c
            .execute(Opcode::GetCaps as u8, &[])
            .await
            .expect_err("stray reply must not satisfy the request");
        assert!(matches!(err, ControlError::Timeout), "got {err:?}");
        assert_eq!(c.transport().sent.len(), 3);
    }

    #[tokio::test]
    async fn stray_corrupt_tag_frame_does_not_kill_in_flight_command() {
        // H1/M-8 regression: a stray frame for another reqId whose tag
        // is corrupted must be skipped WITHOUT verification — the old
        // code verified first and failed the in-flight command with
        // BadTag. Here the stray frame arrives first, then the valid
        // reply for reqId 0, and GET_CAPS must still succeed.
        let transport = ScriptedTransport::new(vec![
            ScriptedRecv::Bytes(reply_bad_tag(
                Opcode::GetCaps as u8,
                999,
                0,
                ErrorCode::Ok,
                &caps_body(),
            )),
            ScriptedRecv::Bytes(reply(
                Opcode::GetCaps as u8,
                0,
                0,
                ErrorCode::Ok,
                &caps_body(),
            )),
        ]);
        let mut c = client(transport);
        let caps = c.get_caps().await.expect("valid reply still wins");
        assert_eq!(caps.firmware, (4, 0, 1));
        assert_eq!(c.transport().sent.len(), 1, "no retry was needed");
    }

    #[tokio::test]
    async fn matching_reply_with_bad_tag_still_fails() {
        // The H1 split only skips non-matching frames: a bad tag on
        // the MATCHING reply is still a hard BadTag, no retry.
        let transport = ScriptedTransport::new(vec![ScriptedRecv::Bytes(reply_bad_tag(
            Opcode::GetCaps as u8,
            0,
            0,
            ErrorCode::Ok,
            &caps_body(),
        ))]);
        let mut c = client(transport);
        let err = c.get_caps().await.expect_err("matching bad tag must fail");
        assert!(matches!(err, ControlError::BadTag), "got {err:?}");
        assert_eq!(c.transport().sent.len(), 1);
    }

    #[tokio::test]
    async fn oversize_payload_is_too_big_without_sending() {
        let transport = ScriptedTransport::new(vec![]);
        let mut c = client(transport);
        let err = c
            .execute(Opcode::SetCfg as u8, &[0u8; 1025])
            .await
            .expect_err("1025-byte payload must fail");
        assert!(matches!(err, ControlError::TooBig), "got {err:?}");
        assert_eq!(c.transport().sent.len(), 0);
    }

    #[tokio::test]
    async fn max_size_payload_is_accepted() {
        let transport = ScriptedTransport::new(vec![ScriptedRecv::Bytes(reply(
            Opcode::SetCfg as u8,
            0,
            0,
            ErrorCode::Ok,
            &[],
        ))]);
        let mut c = client(transport);
        c.execute(Opcode::SetCfg as u8, &[0u8; 1024])
            .await
            .expect("1024-byte payload succeeds");
    }

    #[tokio::test]
    async fn unknown_opcode_is_invalid_argument() {
        let transport = ScriptedTransport::new(vec![]);
        let mut c = client(transport);
        let err = c
            .execute(0x0D, &[])
            .await
            .expect_err("unknown opcode must fail");
        assert!(
            matches!(err, ControlError::InvalidArgument(_)),
            "got {err:?}"
        );
        assert_eq!(c.transport().sent.len(), 0);
    }

    #[tokio::test]
    async fn send_failure_is_transport_error() {
        let transport = ScriptedTransport::with_send_failure();
        let mut c = client(transport);
        let err = c.get_caps().await.expect_err("closed transport must fail");
        assert!(
            matches!(err, ControlError::Transport(TransportError::Closed)),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn recv_closed_is_transport_error() {
        let transport = ScriptedTransport::new(vec![ScriptedRecv::Err(TransportError::Closed)]);
        let mut c = client(transport);
        let err = c.get_caps().await.expect_err("closed transport must fail");
        assert!(
            matches!(err, ControlError::Transport(TransportError::Closed)),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn get_cfg_returns_chunk_bytes() {
        let board = FakeBoard::new(ErrorCode::Ok, b"chunk-data".to_vec());
        let mut c = client(board);
        let chunk = c.get_cfg(0, 10).await.expect("GET_CFG succeeds");
        assert_eq!(chunk, b"chunk-data");
        // Request payload is offset 0, wantLen 10.
        let sent = ControlFrame::parse_structure(&c.transport().sent[0]).expect("send parses");
        assert_eq!(sent.payload, [0, 0, 10, 0]);
    }

    #[tokio::test]
    async fn get_cfg_too_big_want_len() {
        let transport = ScriptedTransport::new(vec![]);
        let mut c = client(transport);
        let err = c
            .get_cfg(0, 1025)
            .await
            .expect_err("want_len 1025 must fail");
        assert!(matches!(err, ControlError::TooBig), "got {err:?}");
        assert_eq!(c.transport().sent.len(), 0);
    }

    #[tokio::test]
    async fn set_cfg_chunk_round_trip() {
        let board = FakeBoard::new(ErrorCode::Ok, Vec::new());
        let mut c = client(board);
        let chunk = SetCfgChunk::new(0, 3, vec![1, 2, 3]);
        c.set_cfg(&chunk).await.expect("SET_CFG succeeds");
    }

    #[tokio::test]
    async fn set_cfg_oversize_chunk_is_too_big() {
        let transport = ScriptedTransport::new(vec![]);
        let mut c = client(transport);
        let chunk = SetCfgChunk::new(0, 2048, vec![0u8; 1025]);
        let err = c
            .set_cfg(&chunk)
            .await
            .expect_err("oversize chunk must fail");
        assert!(matches!(err, ControlError::TooBig), "got {err:?}");
        assert_eq!(c.transport().sent.len(), 0);
    }

    #[tokio::test]
    async fn set_cfg_chunk_out_of_range_is_invalid_argument() {
        let transport = ScriptedTransport::new(vec![]);
        let mut c = client(transport);
        // offset 10 + 3 bytes > total 3.
        let chunk = SetCfgChunk::new(10, 3, vec![1, 2, 3]);
        let err = c
            .set_cfg(&chunk)
            .await
            .expect_err("out-of-range chunk must fail");
        assert!(
            matches!(err, ControlError::InvalidArgument(_)),
            "got {err:?}"
        );
        // The encode error is ChunkOutOfRange, not TooBig.
        assert_eq!(
            chunk.encode(),
            Err(ConfigError::ChunkOutOfRange),
            "test fixture sanity"
        );
    }

    #[tokio::test]
    async fn lifecycle_commands_ok() {
        let board = FakeBoard::new(ErrorCode::Ok, Vec::new());
        let mut c = client(board);
        c.commit().await.expect("COMMIT succeeds");
        c.confirm().await.expect("CONFIRM succeeds");
        c.revert().await.expect("REVERT succeeds");
        c.factory_reset().await.expect("FACTORY_RESET succeeds");
        c.reboot().await.expect("REBOOT succeeds");
        c.key_reset().await.expect("KEY_RESET succeeds");
    }

    #[tokio::test]
    async fn pin_probe_ok_and_validation() {
        let board = FakeBoard::new(ErrorCode::Ok, Vec::new());
        let mut c = client(board);
        c.pin_probe(&PinProbeRequest {
            pin: 34,
            duration_ms: 1000,
            mode: 0,
        })
        .await
        .expect("PIN_PROBE succeeds");
        let err = c
            .pin_probe(&PinProbeRequest {
                pin: 34,
                duration_ms: 50,
                mode: 0,
            })
            .await
            .expect_err("duration 50 ms must fail");
        assert!(
            matches!(err, ControlError::InvalidArgument(_)),
            "got {err:?}"
        );
        let err = c
            .pin_probe(&PinProbeRequest {
                pin: 34,
                duration_ms: 1000,
                mode: 2,
            })
            .await
            .expect_err("mode 2 must fail");
        assert!(
            matches!(err, ControlError::InvalidArgument(_)),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn led_test_ok_and_bad_pattern_id() {
        let board = FakeBoard::new(ErrorCode::Ok, Vec::new());
        let mut c = client(board);
        c.led_test(&LedTestPattern {
            pattern_id: 1,
            r: 255,
            g: 0,
            b: 0,
            period_ms: 500,
            repeats: 0,
        })
        .await
        .expect("LED_TEST succeeds");
        let err = c
            .led_test(&LedTestPattern {
                pattern_id: 9,
                r: 0,
                g: 0,
                b: 0,
                period_ms: 500,
                repeats: 0,
            })
            .await
            .expect_err("pattern 9 must fail");
        assert!(
            matches!(err, ControlError::InvalidArgument(_)),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn set_password_ok_and_validation() {
        let board = FakeBoard::new(ErrorCode::Ok, Vec::new());
        let mut c = client(board);
        c.set_password(b"Abcdef12")
            .await
            .expect("SET_PASSWORD succeeds");
        let err = c
            .set_password(b"short")
            .await
            .expect_err("7-char password must fail");
        assert!(
            matches!(err, ControlError::InvalidArgument(_)),
            "got {err:?}"
        );
        let err = c
            .set_password(b"Abc def1")
            .await
            .expect_err("password with a space must fail");
        assert!(
            matches!(err, ControlError::InvalidArgument(_)),
            "got {err:?}"
        );
        // Nothing was sent for the invalid passwords: 1 send total.
        assert_eq!(c.transport().sent.len(), 1);
    }

    #[tokio::test]
    async fn session_counter_advances_per_send() {
        let transport = ScriptedTransport::new(vec![
            ScriptedRecv::Bytes(reply(
                Opcode::GetCaps as u8,
                0,
                0,
                ErrorCode::Ok,
                &caps_body(),
            )),
            ScriptedRecv::Bytes(reply(
                Opcode::GetCaps as u8,
                1,
                1,
                ErrorCode::Ok,
                &caps_body(),
            )),
        ]);
        let mut c = client(transport);
        assert_eq!(c.session().counter, 0);
        c.get_caps().await.expect("first succeeds");
        assert_eq!(c.session().counter, 1);
        c.get_caps().await.expect("second succeeds");
        assert_eq!(c.session().counter, 2);
    }

    #[tokio::test]
    async fn session_reset_restarts_counter() {
        let mut session = Session::new(KEY, NONCE);
        assert_eq!(session.next_counter(), 0);
        assert_eq!(session.next_counter(), 1);
        session.reset([0x11; 16]);
        assert_eq!(session.nonce, [0x11; 16]);
        assert_eq!(session.counter, 0);
        assert_eq!(session.next_counter(), 0);
    }
}
