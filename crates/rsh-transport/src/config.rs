//! Config transaction client (Phase C5).
//!
//! Drives the stage → commit → confirm → rollback transaction
//! (PROTOCOL-V2.1 §3.5/§3.6, D28) over a [`ControlClient`]:
//!
//! ```text
//! Idle → Staged → Committed → Confirmed
//!          │          │
//!          └────→ RolledBack ←────┘
//! ```
//!
//! Safety rule — a bad config must never brick the board. It is
//! enforced in three layers, all client-side:
//!
//! 1. **Validate before sending.** [`ConfigTransaction::stage`] decodes
//!    the blob and runs the PIN-RULES validator
//!    ([`rsh_protocol::validate_blob`]) before a single `SET_CFG` is
//!    sent. A blob the board would reject is refused locally.
//! 2. **Confirm or roll back.** [`ConfigTransaction::apply`] confirms
//!    inside the 30 s window (D28) and, on any confirm failure,
//!    automatically sends `REVERT`.
//! 3. **Last-good read-back.** [`ConfigTransaction::rollback`] reads the
//!    live config back with `GET_CFG` and compares it against the
//!    pre-stage blob saved in memory; a mismatch is a hard error, not a
//!    silent success.
//!
//! Wire notes (PROTOCOL-V2.1 §3.3, firmware `ctrl_handlers.h`):
//!
//! * `GET_CFG` replies `status, totalLen u16, offset u16, data…` — the
//!   board clamps the data to 1019 B per reply, so
//!   [`ConfigTransaction::read_current_blob`] loops over offsets.
//!   [`ControlClient::get_cfg`] returns that reply body verbatim (status
//!   stripped), so the 4-byte header is parsed here, not in C3.
//! * `COMMIT` replies `status, confirmDeadlineSec u8 (= 30)` on success.
//!   On validation failure it replies `status, errCount u8, errCode u8 ×
//!   errCount` (errCode: 1 pin conflict, 2 value out of range, 3 unknown
//!   module/type, 4 chunk/blob CRC mismatch, 5 total size mismatch).
//! * `SET_CFG` chunks carry at most 1016 B of data (8 B chunk header +
//!   data ≤ the 1024 B frame payload cap), total ≤ 2048 B.
//!
//! # `COMMIT` errCodes (fixed in the C3 API)
//!
//! `COMMIT` replies carry `[errCount, errCode…]` with a `PIN_CONFLICT` /
//! `UNSUPPORTED` status. [`ControlClient::execute_with_status`] returns the
//! reply body even on a non-zero status, so
//! [`ConfigTransaction::commit`] parses the codes with
//! [`parse_commit_error_body`] and reports them in
//! [`ConfigTransactionError::PinConflict`] /
//! [`ConfigTransactionError::Unsupported`].
//!
//! # `PIN_PROBE`
//!
//! [`ConfigTransaction::pin_probe`] only sends the probe command and
//! returns its acknowledgement. The samples themselves arrive as
//! `"RSHP"` stream frames on port 4211 (see `rsh_protocol::probe`);
//! this client never receives them.

use std::time::{Duration, Instant};

use rsh_protocol::config::{CONFIRM_DEADLINE_SECS, MAX_CONFIG_LEN};
use rsh_protocol::control::MAX_PAYLOAD_LEN;
use rsh_protocol::{
    ConfigBlob, ConfigError as ProtocolConfigError, ControlParseError, ErrorCode, Opcode,
    PinProbeRequest, SetCfgChunk, validate_blob,
};

use crate::ControlTransport;
use crate::control_client::{ControlClient, ControlError};

/// `GET_CFG` reply body header: `totalLen u16 | offset u16` (§3.3).
const GET_CFG_HEADER_LEN: usize = 4;

/// `GET_CFG` data the board returns per reply (firmware clamps to 1019 B
/// even when more is requested).
const GET_CFG_MAX_DATA: usize = 1019;

/// Bytes requested per `GET_CFG` call (the protocol cap is 1024).
const GET_CFG_WANT_LEN: u16 = 1024;

/// Bytes of config data per `SET_CFG` chunk: the frame payload cap is
/// 1024 B and the chunk header is 8 B, so 1024 B of data (what §3.5
/// allows on paper) would make a 1032 B payload that both the board's
/// dispatcher and [`ControlClient::execute`] reject — 1016 B is the
/// largest chunk that fits the wire. Chunk boundaries are invisible to
/// the board (bitmap staging), so this changes nothing else.
const SET_CFG_DATA_LEN: usize = MAX_PAYLOAD_LEN - 8;

/// One board validation error code from a failed `COMMIT` reply
/// (§3.6: `errCode u8 × errCount`, first 8 kept by the board).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ConfigErrCode {
    /// `1` — two records claim the same pin, or a pin is forbidden /
    /// strapping / otherwise unusable for its module.
    PinConflict = 1,
    /// `2` — a value is out of range (too many axes/encoders, pin > 39).
    ValueOutOfRange = 2,
    /// `3` — unknown module/record type.
    UnknownModule = 3,
    /// `4` — chunk or blob CRC mismatch.
    CrcMismatch = 4,
    /// `5` — total size mismatch.
    SizeMismatch = 5,
    /// Any other code the board sends (forward-compatible).
    Unknown(u8),
}

impl ConfigErrCode {
    /// Decode a raw errCode byte; unknown values stay [`Self::Unknown`].
    #[must_use]
    pub const fn from_u8(byte: u8) -> Self {
        match byte {
            1 => Self::PinConflict,
            2 => Self::ValueOutOfRange,
            3 => Self::UnknownModule,
            4 => Self::CrcMismatch,
            5 => Self::SizeMismatch,
            other => Self::Unknown(other),
        }
    }

    /// The raw errCode byte.
    #[must_use]
    pub const fn as_u8(self) -> u8 {
        match self {
            Self::PinConflict => 1,
            Self::ValueOutOfRange => 2,
            Self::UnknownModule => 3,
            Self::CrcMismatch => 4,
            Self::SizeMismatch => 5,
            Self::Unknown(byte) => byte,
        }
    }
}

impl std::fmt::Display for ConfigErrCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::PinConflict => "pin conflict",
            Self::ValueOutOfRange => "value out of range",
            Self::UnknownModule => "unknown module/type",
            Self::CrcMismatch => "chunk/blob CRC mismatch",
            Self::SizeMismatch => "total size mismatch",
            Self::Unknown(_) => "unknown err code",
        };
        write!(f, "{}={name}", self.as_u8())
    }
}

/// Parse a failed-`COMMIT` reply body: `errCount u8, errCode u8 ×
/// errCount` (§3.6). The board caps the list at 8 codes; a count larger
/// than the bytes present is truncated, never panicked on.
#[must_use]
pub fn parse_commit_error_body(body: &[u8]) -> Vec<ConfigErrCode> {
    let Some((&count, rest)) = body.split_first() else {
        return Vec::new();
    };
    let count = usize::from(count).min(8).min(rest.len());
    rest[..count]
        .iter()
        .map(|&byte| ConfigErrCode::from_u8(byte))
        .collect()
}

/// Transaction state (see the module docs for the diagram).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConfigState {
    /// No blob staged; nothing pending on the board.
    #[default]
    Idle,
    /// A blob is staged (`SET_CFG` done), not yet committed.
    Staged,
    /// `COMMIT` accepted; the board runs the new config and waits for
    /// `CONFIRM` inside the 30 s window.
    Committed,
    /// `CONFIRM` accepted; the new config is permanent.
    Confirmed,
    /// `REVERT` accepted and the last-good config verified by read-back.
    RolledBack,
}

impl std::fmt::Display for ConfigState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::Idle => "idle",
            Self::Staged => "staged",
            Self::Committed => "committed",
            Self::Confirmed => "confirmed",
            Self::RolledBack => "rolled back",
        };
        f.write_str(name)
    }
}

/// Error from a config transaction step.
#[derive(Debug)]
pub enum ConfigTransactionError {
    /// The blob failed to decode (bad magic/schema/length/CRC, too big).
    InvalidBlob(ProtocolConfigError),
    /// Client-side PIN-RULES validation failed; the strings are the
    /// validator's messages. Nothing was sent to the board.
    PinValidation(Vec<String>),
    /// The board rejected `COMMIT` with `PIN_CONFLICT` (5). The `codes`
    /// are the board's errCodes when the control client exposes them
    /// (see the module docs — currently empty on the live path). Never
    /// retried: resending a conflicting commit cannot succeed.
    PinConflict {
        /// Board errCodes (`1` pin conflict, `2` out of range, …).
        codes: Vec<ConfigErrCode>,
    },
    /// The board rejected `COMMIT` with `UNSUPPORTED` (6) — e.g. an
    /// unknown module/type. `codes` as in [`Self::PinConflict`].
    Unsupported {
        /// Board errCodes (`1` pin conflict, `2` out of range, …).
        codes: Vec<ConfigErrCode>,
    },
    /// The underlying control command failed (timeout, bad tag, replay,
    /// transport, parse, or any other board status).
    Control(ControlError),
    /// The requested step is illegal in the current state (e.g.
    /// `commit` twice, `confirm` without `commit`). Never a panic.
    InvalidState {
        /// State the transaction was in.
        state: ConfigState,
        /// Operation that was attempted.
        operation: &'static str,
    },
    /// `CONFIRM` was not acknowledged inside the confirm window. The
    /// board's 30 s timer keeps running; call [`ConfigTransaction::rollback`]
    /// (or use [`ConfigTransaction::apply`], which rolls back by itself).
    ConfirmTimeout,
    /// Rollback could not be verified: `REVERT` failed, or the config
    /// read back after `REVERT` does not match the saved last-good blob.
    RollbackFailed,
    /// Rollback was refused before anything was sent: there is no
    /// saved last-good blob to verify against (M8 — e.g. a transaction
    /// resumed from a snapshot that carries no `last_good_blob`), so
    /// sending `REVERT` would leave the board in an unverifiable
    /// state. Distinct from [`Self::RollbackFailed`], which means a
    /// `REVERT` was sent and its read-back did not match.
    NoLastGoodBlob,
}

impl std::fmt::Display for ConfigTransactionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidBlob(e) => write!(f, "config blob is invalid: {e}"),
            Self::PinValidation(errs) => {
                if errs.len() == 1 {
                    write!(f, "config failed client-side pin validation: {}", errs[0])
                } else {
                    write!(
                        f,
                        "config failed client-side pin validation ({} errors): {}",
                        errs.len(),
                        errs.join("; ")
                    )
                }
            }
            Self::PinConflict { codes } => {
                write!(
                    f,
                    "board rejected COMMIT with PIN_CONFLICT{}",
                    describe_codes(codes)
                )
            }
            Self::Unsupported { codes } => {
                write!(
                    f,
                    "board rejected COMMIT with UNSUPPORTED{}",
                    describe_codes(codes)
                )
            }
            Self::Control(e) => write!(f, "control command failed: {e}"),
            Self::InvalidState { state, operation } => {
                write!(f, "cannot {operation} while the transaction is {state}")
            }
            Self::ConfirmTimeout => {
                write!(f, "CONFIRM was not acknowledged inside the confirm window")
            }
            Self::RollbackFailed => write!(
                f,
                "rollback could not be verified: REVERT failed or the config read back \
                 after REVERT does not match the saved last-good blob"
            ),
            Self::NoLastGoodBlob => write!(
                f,
                "rollback refused: no last-good config is saved to verify against, \
                 so REVERT was not sent"
            ),
        }
    }
}

impl std::error::Error for ConfigTransactionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidBlob(e) => Some(e),
            Self::Control(e) => Some(e),
            _ => None,
        }
    }
}

impl From<ProtocolConfigError> for ConfigTransactionError {
    fn from(e: ProtocolConfigError) -> Self {
        Self::InvalidBlob(e)
    }
}

impl From<ControlError> for ConfigTransactionError {
    fn from(e: ControlError) -> Self {
        Self::Control(e)
    }
}

/// Render an errCode list for a [`ConfigTransactionError`] message.
fn describe_codes(codes: &[ConfigErrCode]) -> String {
    if codes.is_empty() {
        " (no err codes reported)".to_string()
    } else {
        let list: Vec<String> = codes.iter().map(ToString::to_string).collect();
        format!(" (err codes: {})", list.join(", "))
    }
}

/// A `BadLength` control parse error, for malformed `GET_CFG` replies.
fn bad_length() -> ConfigTransactionError {
    ConfigTransactionError::Control(ControlError::Parse(ControlParseError::BadLength))
}

/// Convert a length to `u16`; lengths here are ≤ 2048 by construction
/// (the blob decoder enforces it), so this only fires defensively.
fn u16_len(value: usize) -> Result<u16, ConfigTransactionError> {
    u16::try_from(value).map_err(|_| bad_length())
}

/// Parse one `GET_CFG` reply body (`totalLen u16 | offset u16 | data…`,
/// §3.3) and check it answers the requested offset.
fn parse_get_cfg_body(
    body: &[u8],
    expected_offset: usize,
) -> Result<(usize, Vec<u8>), ConfigTransactionError> {
    if body.len() < GET_CFG_HEADER_LEN {
        return Err(bad_length());
    }
    let total = usize::from(u16::from_le_bytes([body[0], body[1]]));
    let offset = usize::from(u16::from_le_bytes([body[2], body[3]]));
    // L12: a board reporting a zero-length live config is malformed —
    // a real blob is never empty, and accepting total == 0 would let
    // read_current_blob return an empty blob as "the live config".
    if total == 0 || offset != expected_offset || total > MAX_CONFIG_LEN || offset > total {
        return Err(bad_length());
    }
    let data = &body[GET_CFG_HEADER_LEN..];
    if data.len() > total - offset {
        return Err(bad_length());
    }
    Ok((total, data.to_vec()))
}

/// A resumable snapshot of a [`ConfigTransaction`]'s state (H5-control).
///
/// One board transaction spans several Tauri commands
/// (`stage_config` → `commit_config` → `confirm_config` /
/// `rollback_config`), and every command runs under its own fresh
/// session with a fresh `reqId` counter — the transaction client itself
/// cannot survive between commands. The snapshot is the part that
/// does survive: it is kept in the app state between commands and
/// handed to [`ConfigTransaction::resume`] over the next command's
/// fresh client, so the board-side staged buffer and the client-side
/// record stay in step. It holds no key material and no transport:
/// only the state machine position, the staged / last-good blobs, the
/// board-reported confirm deadline, and when `COMMIT` was accepted
/// (for the 30 s wall-clock confirm window, D28). A snapshot survives
/// only in memory: an app restart loses it, and the board then
/// self-rolls-back within 30 s — the next `rollback_config` (or a
/// fresh `stage_config`) re-syncs the record.
#[derive(Clone, Debug)]
pub struct ConfigTxnSnapshot {
    /// The transaction state machine position.
    pub state: ConfigState,
    /// The last fully staged blob (the one `COMMIT` would apply).
    pub staged_blob: Option<Vec<u8>>,
    /// The live config read back before staging (the rollback target).
    pub last_good_blob: Option<Vec<u8>>,
    /// The confirm window the board reported on `COMMIT` (30 s, D28).
    pub commit_deadline: Duration,
    /// When `COMMIT` was accepted (for the wall-clock confirm window).
    pub committed_at: Option<Instant>,
}

impl Default for ConfigTxnSnapshot {
    /// An idle snapshot: no staged blob, no last-good blob, the
    /// default 30 s confirm deadline (D28), nothing committed.
    fn default() -> Self {
        Self {
            state: ConfigState::Idle,
            staged_blob: None,
            last_good_blob: None,
            commit_deadline: Duration::from_secs(u64::from(CONFIRM_DEADLINE_SECS)),
            committed_at: None,
        }
    }
}

/// Config transaction client: stage → commit → confirm → rollback,
/// generic over the control transport, wrapping a [`ControlClient`].
///
/// The client owns the state machine; illegal transitions return
/// [`ConfigTransactionError::InvalidState`], never a panic. The
/// high-level [`ConfigTransaction::apply`] runs the whole flow
/// (stage → commit → confirm-or-rollback) in one call; the low-level
/// [`ConfigTransaction::stage`] / [`ConfigTransaction::commit`] /
/// [`ConfigTransaction::confirm`] / [`ConfigTransaction::rollback`]
/// methods exist for the UI and tests that drive the flow manually.
///
/// A transaction that spans several fresh sessions (H5-control: one
/// Tauri command per step, each under its own session) is resumed with
/// [`ConfigTransaction::resume`] from the [`ConfigTxnSnapshot`] the
/// previous step saved with [`ConfigTransaction::snapshot`].
pub struct ConfigTransaction<T: ControlTransport> {
    client: ControlClient<T>,
    state: ConfigState,
    /// Last fully staged blob (the one `COMMIT` would apply).
    staged_blob: Option<Vec<u8>>,
    /// Live config read back before staging (the rollback target).
    last_good_blob: Option<Vec<u8>>,
    /// Confirm window from the board's `COMMIT` reply (always 30 s, D28).
    commit_deadline: Duration,
    /// When `COMMIT` was accepted (for the wall-clock confirm window).
    committed_at: Option<Instant>,
}

impl<T: ControlTransport> ConfigTransaction<T> {
    /// Create a transaction client around a [`ControlClient`]. Starts
    /// [`ConfigState::Idle`] with the default 30 s confirm deadline.
    #[must_use]
    pub fn new(client: ControlClient<T>) -> Self {
        Self {
            client,
            state: ConfigState::Idle,
            staged_blob: None,
            last_good_blob: None,
            commit_deadline: Duration::from_secs(u64::from(CONFIRM_DEADLINE_SECS)),
            committed_at: None,
        }
    }

    /// Resume a transaction over a fresh [`ControlClient`] (a new
    /// session, H5-control) from the snapshot a previous step saved
    /// with [`ConfigTransaction::snapshot`]. The board-side staged
    /// buffer is untouched by resuming — resume only restores the
    /// client-side record, so the caller must resume the snapshot the
    /// previous step actually saved, never a stale one.
    #[must_use]
    pub fn resume(client: ControlClient<T>, snapshot: ConfigTxnSnapshot) -> Self {
        Self {
            client,
            state: snapshot.state,
            staged_blob: snapshot.staged_blob,
            last_good_blob: snapshot.last_good_blob,
            commit_deadline: snapshot.commit_deadline,
            committed_at: snapshot.committed_at,
        }
    }

    /// Save the resumable state for the next step's
    /// [`ConfigTransaction::resume`]. Call after every step — including
    /// a failed one, so a failed stage (state back to `Idle`) or a
    /// timed-out confirm (state still `Committed`) is recorded too.
    #[must_use]
    pub fn snapshot(&self) -> ConfigTxnSnapshot {
        ConfigTxnSnapshot {
            state: self.state,
            staged_blob: self.staged_blob.clone(),
            last_good_blob: self.last_good_blob.clone(),
            commit_deadline: self.commit_deadline,
            committed_at: self.committed_at,
        }
    }

    /// The current transaction state.
    #[must_use]
    pub fn state(&self) -> ConfigState {
        self.state
    }

    /// The last fully staged blob, if any.
    #[must_use]
    pub fn staged_blob(&self) -> Option<&[u8]> {
        self.staged_blob.as_deref()
    }

    /// The last-good blob saved before staging, if any (the rollback
    /// target — the live config the board ran before this transaction).
    #[must_use]
    pub fn last_good_blob(&self) -> Option<&[u8]> {
        self.last_good_blob.as_deref()
    }

    /// The confirm window the board reported on `COMMIT` (30 s, D28;
    /// the default before any commit).
    #[must_use]
    pub fn confirm_deadline(&self) -> Duration {
        self.commit_deadline
    }

    /// The underlying control client.
    #[must_use]
    pub fn client(&self) -> &ControlClient<T> {
        &self.client
    }

    /// Mutable access to the underlying control client.
    pub fn client_mut(&mut self) -> &mut ControlClient<T> {
        &mut self.client
    }

    /// Consume the transaction client, returning the control client.
    #[must_use]
    pub fn into_inner(self) -> ControlClient<T> {
        self.client
    }

    /// Read the board's whole live config blob with chunked `GET_CFG`
    /// (§3.3: each reply is `totalLen | offset | data…`, data clamped
    /// to 1019 B). Does not change the transaction state.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigTransactionError::Control`] when a `GET_CFG`
    /// fails or a reply is malformed (short, wrong offset, total >
    /// 2048 B, or more data than the total allows).
    pub async fn read_current_blob(&mut self) -> Result<Vec<u8>, ConfigTransactionError> {
        let first = self.client.get_cfg(0, GET_CFG_WANT_LEN).await?;
        let (total, mut blob) = parse_get_cfg_body(&first, 0)?;
        while blob.len() < total {
            let offset = u16_len(blob.len())?;
            let remaining = total - blob.len();
            let want = if remaining > GET_CFG_MAX_DATA {
                GET_CFG_WANT_LEN
            } else {
                u16_len(remaining)?
            };
            let body = self.client.get_cfg(offset, want).await?;
            let (total_now, chunk) = parse_get_cfg_body(&body, blob.len())?;
            if total_now != total || chunk.is_empty() {
                return Err(bad_length());
            }
            blob.extend_from_slice(&chunk);
        }
        Ok(blob)
    }

    /// Stage a config blob: validate it client-side, save the live
    /// config, then write it with chunked `SET_CFG` (1016 B of data per
    /// chunk — 8 B header + data must fit the 1024 B frame payload cap —
    /// total ≤ 2048 B).
    ///
    /// Legal from [`ConfigState::Idle`], [`ConfigState::Staged`]
    /// (re-stage replaces the staged blob — a new offset-0 chunk opens
    /// a fresh session on the board), and [`ConfigState::RolledBack`].
    /// A bad pin map is refused before anything is sent — no `SET_CFG`
    /// (and no `GET_CFG`) reaches the board.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigTransactionError::InvalidState`] from
    /// `Committed`/`Confirmed`, [`ConfigTransactionError::InvalidBlob`]
    /// when the blob does not decode,
    /// [`ConfigTransactionError::PinValidation`] when the PIN-RULES
    /// validator rejects it, and [`ConfigTransactionError::Control`]
    /// when reading the live config or a `SET_CFG` chunk fails. On a
    /// chunk failure the state drops back to `Idle` (the board's staged
    /// buffer is partial/unknown) and the staged blob is forgotten.
    pub async fn stage(&mut self, blob: &[u8]) -> Result<(), ConfigTransactionError> {
        self.require_state(
            &[
                ConfigState::Idle,
                ConfigState::Staged,
                ConfigState::RolledBack,
            ],
            "stage",
        )?;
        // 1. Decode + PIN-RULES validation BEFORE anything is sent: a
        // blob the board would reject must never reach the wire.
        let decoded = ConfigBlob::decode(blob)?;
        let pin_errors = validate_blob(&decoded);
        if !pin_errors.is_empty() {
            return Err(ConfigTransactionError::PinValidation(pin_errors));
        }
        // 2. Remember the live config so rollback can verify it later.
        let last_good = self.read_current_blob().await?;
        // 3. Chunked SET_CFG (1016 B of data per chunk: 8 B header +
        // data must fit the 1024 B frame payload cap).
        let total = u16_len(blob.len())?;
        for (index, data) in blob.chunks(SET_CFG_DATA_LEN).enumerate() {
            let offset = u16_len(index * SET_CFG_DATA_LEN)?;
            let chunk = SetCfgChunk::new(offset, total, data.to_vec());
            if let Err(e) = self.client.set_cfg(&chunk).await {
                self.state = ConfigState::Idle;
                self.staged_blob = None;
                return Err(e.into());
            }
        }
        self.last_good_blob = Some(last_good);
        self.staged_blob = Some(blob.to_vec());
        self.state = ConfigState::Staged;
        Ok(())
    }

    /// Send `COMMIT`: the board validates the staged blob and, on
    /// success, live-applies it and starts the 30 s confirm window
    /// (the deadline comes from the board's reply body).
    ///
    /// Legal only from [`ConfigState::Staged`]. On `PIN_CONFLICT`
    /// returns [`ConfigTransactionError::PinConflict`] with the board's
    /// errCodes (see the module docs) and does **not** retry — a
    /// conflicting commit cannot succeed by resending it. The state
    /// stays `Staged`, so the caller can fix the blob and re-stage, or
    /// roll back to discard it.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigTransactionError::InvalidState`] when not
    /// `Staged`, [`ConfigTransactionError::PinConflict`] /
    /// [`ConfigTransactionError::Unsupported`] on a board validation
    /// failure, and [`ConfigTransactionError::Control`] otherwise.
    pub async fn commit(&mut self) -> Result<(), ConfigTransactionError> {
        self.require_state(&[ConfigState::Staged], "commit")?;
        if self.staged_blob.is_none() {
            return Err(ConfigTransactionError::InvalidState {
                state: self.state,
                operation: "commit (no staged blob)",
            });
        }
        // NOTE: `ControlClient::commit` is deliberately NOT used here:
        // it discards the reply body, and the COMMIT reply carries the
        // confirm deadline (success) or the errCodes (validation
        // failure). `execute_with_status` returns the body either way.
        match self
            .client
            .execute_with_status(Opcode::Commit as u8, &[])
            .await
        {
            Ok((ErrorCode::Ok, body)) => {
                let secs = match body.first() {
                    Some(&secs) if secs > 0 => secs,
                    _ => CONFIRM_DEADLINE_SECS,
                };
                self.commit_deadline = Duration::from_secs(u64::from(secs));
                self.committed_at = Some(Instant::now());
                self.state = ConfigState::Committed;
                Ok(())
            }
            Ok((ErrorCode::PinConflict, body)) => Err(ConfigTransactionError::PinConflict {
                codes: parse_commit_error_body(&body),
            }),
            Ok((ErrorCode::Unsupported, body)) => Err(ConfigTransactionError::Unsupported {
                codes: parse_commit_error_body(&body),
            }),
            Ok((status, _)) => Err(ControlError::BoardError(status).into()),
            Err(e) => Err(e.into()),
        }
    }

    /// Send `CONFIRM` inside the 30 s window: the staged config
    /// becomes permanent. Legal only from [`ConfigState::Committed`].
    ///
    /// A transport timeout becomes [`ConfigTransactionError::ConfirmTimeout`];
    /// the state stays `Committed` (the board's timer keeps running),
    /// so the caller must [`ConfigTransaction::rollback`].
    ///
    /// # Errors
    ///
    /// Returns [`ConfigTransactionError::InvalidState`] when not
    /// `Committed`, [`ConfigTransactionError::ConfirmTimeout`] when the
    /// confirm is not acknowledged, and
    /// [`ConfigTransactionError::Control`] otherwise (including a late
    /// confirm the board answers `UNSUPPORTED` — it has already rolled
    /// back by then).
    pub async fn confirm(&mut self) -> Result<(), ConfigTransactionError> {
        self.require_state(&[ConfigState::Committed], "confirm")?;
        match self.client.confirm().await {
            Ok(()) => {
                // The staged blob is now the last-good config.
                self.last_good_blob = self.staged_blob.take();
                self.committed_at = None;
                self.state = ConfigState::Confirmed;
                Ok(())
            }
            Err(ControlError::Timeout) => Err(ConfigTransactionError::ConfirmTimeout),
            Err(e) => Err(e.into()),
        }
    }

    /// Roll back: send `REVERT` (discards the staged or
    /// committed-but-unconfirmed config) and verify the board reports
    /// the last-good config — read back via `GET_CFG` and compare
    /// against the pre-stage blob saved in memory.
    ///
    /// Legal from [`ConfigState::Staged`] (explicit cancel) and
    /// [`ConfigState::Committed`] (confirm timeout / late confirm).
    /// `REVERT` is idempotent on the board, so rolling back when
    /// nothing is pending is still a verified no-op.
    ///
    /// M8: the saved last-good blob is checked *before* `REVERT` is
    /// sent — with no blob to verify against, the rollback is refused
    /// with [`ConfigTransactionError::NoLastGoodBlob`] and nothing is
    /// sent, instead of sending `REVERT` and then failing verification
    /// with a misleading [`ConfigTransactionError::RollbackFailed`].
    ///
    /// # Errors
    ///
    /// Returns [`ConfigTransactionError::InvalidState`] when not
    /// `Staged`/`Committed`, [`ConfigTransactionError::NoLastGoodBlob`]
    /// when no last-good blob is saved,
    /// [`ConfigTransactionError::Control`] when `REVERT` or the
    /// read-back fails, and
    /// [`ConfigTransactionError::RollbackFailed`] when the read-back
    /// does not match the saved last-good blob.
    pub async fn rollback(&mut self) -> Result<(), ConfigTransactionError> {
        self.require_state(&[ConfigState::Staged, ConfigState::Committed], "rollback")?;
        let Some(last_good) = self.last_good_blob.clone() else {
            return Err(ConfigTransactionError::NoLastGoodBlob);
        };
        self.client.revert().await?;
        let current = self.read_current_blob().await?;
        if last_good == current {
            self.staged_blob = None;
            self.committed_at = None;
            self.state = ConfigState::RolledBack;
            Ok(())
        } else {
            Err(ConfigTransactionError::RollbackFailed)
        }
    }

    /// Run the whole transaction in one call: stage → commit →
    /// confirm-or-rollback.
    ///
    /// On a confirm failure (timeout, late confirm, or any other
    /// confirm error) this automatically rolls back and returns the
    /// original confirm error when the rollback verifies — so a bad
    /// config can never be left live unconfirmed. Exception (H2): on
    /// a confirm *timeout* the live config is read back first, and if
    /// the staged blob is already live (the confirm executed but its
    /// reply was lost) the transaction is confirmed instead of rolled
    /// back. If the rollback itself cannot be verified, the rollback
    /// error is returned instead (the board may be in an unknown
    /// state — surface it, do not hide it behind the confirm error).
    /// A failed `commit` needs no rollback: the board never
    /// live-applied the blob, so the error is returned as-is.
    ///
    /// # Errors
    ///
    /// Same as [`ConfigTransaction::stage`],
    /// [`ConfigTransaction::commit`], and [`ConfigTransaction::confirm`];
    /// [`ConfigTransactionError::RollbackFailed`] when the automatic
    /// rollback could not be verified.
    pub async fn apply(&mut self, blob: &[u8]) -> Result<(), ConfigTransactionError> {
        self.stage(blob).await?;
        self.commit().await?;
        // Confirm inside the board-reported window (D28: always 30 s).
        // The wall clock is enforced here, not just by the board: if
        // the window is already gone, do not waste a CONFIRM the board
        // would answer UNSUPPORTED — go straight to rollback.
        let elapsed = self.committed_at.map_or(Duration::ZERO, |at| at.elapsed());
        let remaining = self.commit_deadline.saturating_sub(elapsed);
        let confirm_result = if remaining.is_zero() {
            Err(ConfigTransactionError::ConfirmTimeout)
        } else {
            match tokio::time::timeout(remaining, self.confirm()).await {
                Ok(result) => result,
                Err(_) => Err(ConfigTransactionError::ConfirmTimeout),
            }
        };
        match confirm_result {
            Ok(()) => Ok(()),
            Err(confirm_err) => {
                // H2: a confirm timeout is ambiguous — the board may
                // have applied the new config and only its reply was
                // lost (dropped datagram / late retry), in which case
                // blindly reverting would throw away a good config the
                // board is already running. Read the live config back
                // first: if it is already the staged (new) blob, the
                // confirm really happened and the transaction is
                // confirmed; only when the live config is still the
                // old one (or the read-back itself fails) do we roll
                // back. This check runs only for a timeout — any other
                // confirm error (board rejection, late confirm) still
                // rolls back immediately.
                if matches!(confirm_err, ConfigTransactionError::ConfirmTimeout)
                    && self.confirm_applied_via_readback().await
                {
                    self.last_good_blob = self.staged_blob.take();
                    self.committed_at = None;
                    self.state = ConfigState::Confirmed;
                    return Ok(());
                }
                match self.rollback().await {
                    Ok(()) => Err(confirm_err),
                    Err(rollback_err) => Err(rollback_err),
                }
            }
        }
    }

    /// H2 helper: `true` when a `GET_CFG` read-back shows the staged
    /// blob is already the board's live config (the confirm executed
    /// but its reply was lost). `false` on any read-back failure or
    /// when the live config is anything else.
    async fn confirm_applied_via_readback(&mut self) -> bool {
        match self.read_current_blob().await {
            Ok(live) => Some(&live) == self.staged_blob.as_ref(),
            Err(_) => false,
        }
    }

    /// `PIN_PROBE` passthrough: send the probe command and return its
    /// acknowledgement. The samples themselves arrive as `"RSHP"`
    /// stream frames on port 4211 (see `rsh_protocol::probe`) — this
    /// client never receives them. Independent of the transaction
    /// state; duration 100–10000 ms, mode 0 digital / 1 ADC mV.
    ///
    /// # Errors
    ///
    /// Same as [`ControlClient::pin_probe`] (invalid duration/mode,
    /// timeout, board error, transport error).
    pub async fn pin_probe(
        &mut self,
        request: &PinProbeRequest,
    ) -> Result<(), ConfigTransactionError> {
        self.client.pin_probe(request).await?;
        Ok(())
    }

    /// Fail unless the state is one of `allowed`.
    fn require_state(
        &self,
        allowed: &[ConfigState],
        operation: &'static str,
    ) -> Result<(), ConfigTransactionError> {
        if allowed.contains(&self.state) {
            Ok(())
        } else {
            Err(ConfigTransactionError::InvalidState {
                state: self.state,
                operation,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use rsh_protocol::config::record_type;
    use rsh_protocol::{
        ConfigBlob, ConfigError, ConfigRecord, ControlFrame, ControlParseError, ErrorCode, Opcode,
        PinProbeRequest, SetCfgChunk, build_reply, factory_default_blob,
    };

    use super::{
        ConfigErrCode, ConfigState, ConfigTransaction, ConfigTransactionError,
        parse_commit_error_body,
    };
    use crate::control_client::{ControlClient, ControlError, Session};
    use crate::{ControlTransport, TransportError};

    /// How the fake board answers `CONFIRM`.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    enum ConfirmBehavior {
        /// Answer normally.
        #[default]
        Answer,
        /// Cache the answer for retries but deliver nothing this time
        /// (forces one client retry, then success).
        DropNext,
        /// Never answer and never execute: the board never sees the
        /// confirm, so the config is never applied.
        Silent,
    }

    /// What the fake board does with an executed command.
    enum BoardReply {
        /// Answer now (and cache for same-`reqId` retries).
        Answer(Vec<u8>),
        /// Cache the answer for retries but deliver nothing this time.
        DropOnce(Vec<u8>),
        /// Deliver nothing and cache nothing (every retry re-executes).
        Silent,
    }

    /// A scripted fake board: verifies CRCs, answers same-`reqId`
    /// retries from a cache without re-executing, keeps a live config
    /// (`stored`), a staged buffer, and a pending-commit flag — the
    /// wire-faithful shapes from PROTOCOL-V2.2 §3.3/§3.5/§3.6
    /// (`GET_CFG` → `totalLen|offset|data`, `COMMIT` OK → `[30]`,
    /// `COMMIT` fail → `[errCount, errCode…]`).
    struct FakeBoard {
        pending: Option<Vec<u8>>,
        sent: Vec<Vec<u8>>,
        stored: Vec<u8>,
        staged: Vec<u8>,
        staged_received: Vec<bool>,
        staged_total: usize,
        committed: bool,
        commit_status: ErrorCode,
        commit_err_body: Vec<u8>,
        commit_deadline: u8,
        confirm_behavior: ConfirmBehavior,
        corrupt_on_revert: bool,
        /// H2 test hook: `CONFIRM` executes (the new config goes
        /// live) but its reply is dropped on EVERY attempt, so the
        /// client always times out even though the board applied it.
        apply_silent_confirm: bool,
        cached: HashMap<u16, Vec<u8>>,
    }

    impl FakeBoard {
        fn new(stored: Vec<u8>) -> Self {
            Self {
                pending: None,
                sent: Vec::new(),
                stored,
                staged: Vec::new(),
                staged_received: Vec::new(),
                staged_total: 0,
                committed: false,
                commit_status: ErrorCode::Ok,
                commit_err_body: Vec::new(),
                commit_deadline: 30,
                confirm_behavior: ConfirmBehavior::Answer,
                corrupt_on_revert: false,
                apply_silent_confirm: false,
                cached: HashMap::new(),
            }
        }

        fn reply(frame: &ControlFrame, status: ErrorCode, body: &[u8]) -> Vec<u8> {
            build_reply(frame.cmd, frame.req_id, status, body).expect("fake-board reply builds")
        }

        fn staged_bytes(&self) -> u16 {
            u16::try_from(
                self.staged_received
                    .iter()
                    .filter(|received| **received)
                    .count(),
            )
            .unwrap_or(u16::MAX)
        }

        fn staged_total(&self) -> u16 {
            u16::try_from(self.staged_total).unwrap_or(u16::MAX)
        }

        /// `SET_CFG` reply body: `stagedBytes u16 | totalExpected u16`.
        fn staged_body(&self) -> Vec<u8> {
            let mut body = Vec::with_capacity(4);
            body.extend_from_slice(&self.staged_bytes().to_le_bytes());
            body.extend_from_slice(&self.staged_total().to_le_bytes());
            body
        }

        /// `GET_CFG` handler: reply `totalLen | offset | data…`, data
        /// clamped to 1019 B.
        fn reply_get_cfg(&self, frame: &ControlFrame) -> BoardReply {
            if frame.payload.len() != 4 {
                return BoardReply::Answer(Self::reply(frame, ErrorCode::Unsupported, &[]));
            }
            let offset = usize::from(u16::from_le_bytes([frame.payload[0], frame.payload[1]]));
            let want = usize::from(u16::from_le_bytes([frame.payload[2], frame.payload[3]]));
            let take = want.min(1019).min(self.stored.len().saturating_sub(offset));
            let mut body = Vec::with_capacity(4 + take);
            body.extend_from_slice(
                &u16::try_from(self.stored.len())
                    .unwrap_or(u16::MAX)
                    .to_le_bytes(),
            );
            body.extend_from_slice(&u16::try_from(offset).unwrap_or(u16::MAX).to_le_bytes());
            body.extend_from_slice(&self.stored[offset..offset + take]);
            BoardReply::Answer(Self::reply(frame, ErrorCode::Ok, &body))
        }

        /// `SET_CFG` handler: stage one chunk, reply
        /// `stagedBytes | totalExpected`.
        fn reply_set_cfg(&mut self, frame: &ControlFrame) -> BoardReply {
            let chunk = match SetCfgChunk::decode(&frame.payload) {
                Err(e) => {
                    let status = if matches!(e, ConfigError::TooBig) {
                        ErrorCode::TooBig
                    } else {
                        ErrorCode::BadCrc
                    };
                    let body = self.staged_body();
                    return BoardReply::Answer(Self::reply(frame, status, &body));
                }
                Ok(chunk) => chunk,
            };
            if chunk.offset == 0 {
                self.staged_total = usize::from(chunk.total);
                self.staged = vec![0u8; self.staged_total];
                self.staged_received = vec![false; self.staged_total];
            } else if self.staged_total == 0 || usize::from(chunk.total) != self.staged_total {
                let body = self.staged_body();
                return BoardReply::Answer(Self::reply(frame, ErrorCode::TooBig, &body));
            }
            let off = usize::from(chunk.offset);
            let len = chunk.data.len();
            self.staged[off..off + len].copy_from_slice(&chunk.data);
            for slot in &mut self.staged_received[off..off + len] {
                *slot = true;
            }
            let body = self.staged_body();
            BoardReply::Answer(Self::reply(frame, ErrorCode::Ok, &body))
        }

        fn execute_command(&mut self, frame: &ControlFrame) -> BoardReply {
            match frame.cmd {
                cmd if cmd == Opcode::GetCfg as u8 => self.reply_get_cfg(frame),
                cmd if cmd == Opcode::SetCfg as u8 => self.reply_set_cfg(frame),
                cmd if cmd == Opcode::Commit as u8 => {
                    if self.commit_status != ErrorCode::Ok {
                        let body = self.commit_err_body.clone();
                        return BoardReply::Answer(Self::reply(frame, self.commit_status, &body));
                    }
                    let complete = self.staged_received.iter().all(|received| *received);
                    if !complete || ConfigBlob::decode(&self.staged).is_err() {
                        return BoardReply::Answer(Self::reply(frame, ErrorCode::BadCrc, &[1, 4]));
                    }
                    self.committed = true;
                    BoardReply::Answer(Self::reply(frame, ErrorCode::Ok, &[self.commit_deadline]))
                }
                cmd if cmd == Opcode::Confirm as u8 => {
                    if self.confirm_behavior == ConfirmBehavior::Silent {
                        return BoardReply::Silent;
                    }
                    let (status, body) = if self.committed {
                        self.stored = self.staged.clone();
                        self.committed = false;
                        (ErrorCode::Ok, Vec::new())
                    } else {
                        (ErrorCode::Unsupported, Vec::new())
                    };
                    let reply = Self::reply(frame, status, &body);
                    if self.confirm_behavior == ConfirmBehavior::DropNext {
                        self.confirm_behavior = ConfirmBehavior::Answer;
                        BoardReply::DropOnce(reply)
                    } else {
                        BoardReply::Answer(reply)
                    }
                }
                cmd if cmd == Opcode::Revert as u8 => {
                    self.committed = false;
                    self.staged = Vec::new();
                    self.staged_received = Vec::new();
                    self.staged_total = 0;
                    if self.corrupt_on_revert {
                        self.stored = vec![0xAA; 12];
                    }
                    BoardReply::Answer(Self::reply(frame, ErrorCode::Ok, &[]))
                }
                cmd if cmd == Opcode::PinProbe as u8 => {
                    let accepted = if frame.payload.len() == 4 {
                        u16::from_le_bytes([frame.payload[1], frame.payload[2]])
                    } else {
                        0
                    };
                    BoardReply::Answer(Self::reply(frame, ErrorCode::Ok, &accepted.to_le_bytes()))
                }
                _ => BoardReply::Answer(Self::reply(frame, ErrorCode::Unsupported, &[])),
            }
        }

        /// Commands sent so far, decoded from the raw frames.
        fn sent_cmds(&self) -> Vec<u8> {
            self.sent
                .iter()
                .map(|bytes| ControlFrame::parse_structure(bytes).map_or(0, |frame| frame.cmd))
                .collect()
        }

        /// How many times `cmd` was sent.
        fn count_cmd(&self, cmd: u8) -> usize {
            self.sent_cmds()
                .iter()
                .fold(0, |count, sent| count + usize::from(*sent == cmd))
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
                // CRC check first: a bad-CRC frame is dropped silently,
                // never executed and never answered.
                if ControlFrame::verify(&owned).is_err() {
                    self.pending = None;
                    return Ok(());
                }
                // Duplicate cache: same reqId re-answered, never re-executed.
                if let Some(cached) = self.cached.get(&frame.req_id) {
                    // H2 hook: a confirm whose reply is lost every time
                    // is never delivered, even from the cache.
                    if !(self.apply_silent_confirm && frame.cmd == Opcode::Confirm as u8) {
                        self.pending = Some(cached.clone());
                    }
                    return Ok(());
                }
                match self.execute_command(&frame) {
                    BoardReply::Answer(reply) => {
                        self.cached.insert(frame.req_id, reply.clone());
                        // H2 hook: see the duplicate-cache branch above.
                        if !(self.apply_silent_confirm && frame.cmd == Opcode::Confirm as u8) {
                            self.pending = Some(reply);
                        }
                    }
                    BoardReply::DropOnce(reply) => {
                        self.cached.insert(frame.req_id, reply);
                        self.pending = None;
                    }
                    BoardReply::Silent => {
                        self.pending = None;
                    }
                }
                Ok(())
            }
        }

        async fn recv_control(&mut self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
            if let Some(bytes) = self.pending.take() {
                return Ok(bytes);
            }
            // Silence lasts the whole requested timeout, like a real
            // transport waiting for a reply that never comes.
            tokio::time::sleep(timeout).await;
            Err(TransportError::Timeout)
        }
    }

    /// Factory default blob (188 B, cfgVersion 1) as the board's live config.
    fn good_blob() -> Vec<u8> {
        factory_default_blob()
            .encode()
            .expect("factory blob encodes")
    }

    /// Factory blob with cfgVersion 2 (the "new" config under test).
    fn good_blob_v2() -> Vec<u8> {
        let mut blob = factory_default_blob();
        blob.cfg_version = 2;
        blob.encode().expect("v2 blob encodes")
    }

    /// Factory blob with cfgVersion 3.
    fn good_blob_v3() -> Vec<u8> {
        let mut blob = factory_default_blob();
        blob.cfg_version = 3;
        blob.encode().expect("v3 blob encodes")
    }

    /// Factory blob where BUTTON slot 2 reuses GPIO34 (already the
    /// steering axis pin) — a duplicate-pin map the validator refuses.
    fn bad_pin_blob() -> Vec<u8> {
        let mut blob = factory_default_blob();
        blob.records[7].pin_a = 34;
        blob.encode().expect("bad-pin blob encodes")
    }

    /// Factory blob with the steering axis moved to GPIO25 (ADC2 —
    /// unusable for analog with Wi-Fi).
    fn adc2_blob() -> Vec<u8> {
        let mut blob = factory_default_blob();
        blob.records[0].pin_a = 25;
        blob.encode().expect("ADC2 blob encodes")
    }

    /// A valid 1788 B blob (factory records + 100 unassigned `LED_STRIP`
    /// records, `pinA == 0xFF`): stages in two `SET_CFG` chunks
    /// (1016 + 772) and reads back in two `GET_CFG` replies (1019 + 769).
    /// Pinless LED records are the filler because the C++ validator
    /// counts EVERY record toward the limits — 100 pinless BUTTON
    /// records would be `n_btn = 103 > 64` (firmware UNSUPPORTED) —
    /// while a pinless LED record is an explicit no-op
    /// (`rsh_cfg_blob_validate`: only a pinned LED record increments
    /// `n_led`).
    fn big_blob() -> Vec<u8> {
        let mut blob = factory_default_blob();
        for slot in 3..103u8 {
            blob.records.push(ConfigRecord::new(
                record_type::LED_STRIP,
                slot,
                0xFF,
                0xFF,
                0xFF,
            ));
        }
        let encoded = blob.encode().expect("big blob encodes");
        assert_eq!(encoded.len(), 1788);
        encoded
    }

    /// A transaction client over a fake board whose live config is the
    /// factory blob; 10 ms per-attempt timeout so a full 3-attempt
    /// timeout stays fast in tests.
    fn txn(board: FakeBoard) -> ConfigTransaction<FakeBoard> {
        let client =
            ControlClient::new(board, Session::new()).with_timeout(Duration::from_millis(10));
        ConfigTransaction::new(client)
    }

    /// A transaction client over a fake board with a custom live config.
    fn txn_with(stored: Vec<u8>) -> ConfigTransaction<FakeBoard> {
        txn(FakeBoard::new(stored))
    }

    #[tokio::test]
    async fn apply_happy_path_makes_config_live() {
        let mut txn = txn_with(good_blob());
        txn.apply(&good_blob_v2()).await.expect("apply succeeds");
        assert_eq!(txn.state(), ConfigState::Confirmed);
        assert_eq!(txn.client().transport().stored, good_blob_v2());
        assert_eq!(txn.last_good_blob(), Some(good_blob_v2().as_slice()));
        assert_eq!(txn.staged_blob(), None);
        // One GET_CFG (last-good read), one SET_CFG (188 B, one chunk),
        // one COMMIT, one CONFIRM.
        let board = txn.client().transport();
        assert_eq!(board.count_cmd(Opcode::GetCfg as u8), 1);
        assert_eq!(board.count_cmd(Opcode::SetCfg as u8), 1);
        assert_eq!(board.count_cmd(Opcode::Commit as u8), 1);
        assert_eq!(board.count_cmd(Opcode::Confirm as u8), 1);
        assert_eq!(board.count_cmd(Opcode::Revert as u8), 0);
    }

    #[tokio::test]
    async fn manual_stage_commit_confirm_flow() {
        let mut txn = txn_with(good_blob());
        assert_eq!(txn.state(), ConfigState::Idle);
        assert_eq!(txn.confirm_deadline(), Duration::from_secs(30));
        txn.stage(&good_blob_v2()).await.expect("stage succeeds");
        assert_eq!(txn.state(), ConfigState::Staged);
        assert_eq!(txn.staged_blob(), Some(good_blob_v2().as_slice()));
        assert_eq!(txn.last_good_blob(), Some(good_blob().as_slice()));
        txn.commit().await.expect("commit succeeds");
        assert_eq!(txn.state(), ConfigState::Committed);
        // The board's COMMIT reply body [30] is the confirm deadline.
        assert_eq!(txn.confirm_deadline(), Duration::from_secs(30));
        txn.confirm().await.expect("confirm succeeds");
        assert_eq!(txn.state(), ConfigState::Confirmed);
        assert_eq!(txn.client().transport().stored, good_blob_v2());
        assert_eq!(txn.last_good_blob(), Some(good_blob_v2().as_slice()));
    }

    #[tokio::test]
    async fn bad_pin_map_refused_before_sending() {
        let mut txn = txn_with(good_blob());
        let err = txn
            .stage(&bad_pin_blob())
            .await
            .expect_err("duplicate pin must be refused");
        assert!(
            matches!(err, ConfigTransactionError::PinValidation(ref errs) if errs.iter().any(|e| e.contains("already used"))),
            "got {err:?}"
        );
        assert_eq!(txn.state(), ConfigState::Idle);
        // Nothing at all was sent — no GET_CFG, no SET_CFG.
        assert_eq!(txn.client().transport().sent.len(), 0);
    }

    #[tokio::test]
    async fn adc2_axis_refused_before_sending() {
        let mut txn = txn_with(good_blob());
        let err = txn
            .stage(&adc2_blob())
            .await
            .expect_err("ADC2 axis must be refused");
        assert!(
            matches!(err, ConfigTransactionError::PinValidation(ref errs) if errs.iter().any(|e| e.contains("ADC1"))),
            "got {err:?}"
        );
        assert_eq!(txn.client().transport().sent.len(), 0);
    }

    #[tokio::test]
    async fn corrupt_blob_refused_before_sending() {
        let mut blob = good_blob_v2();
        blob[20] ^= 0xFF; // Break the CRC32 trailer's coverage.
        let mut txn = txn_with(good_blob());
        let err = txn
            .stage(&blob)
            .await
            .expect_err("CRC-broken blob must be refused");
        assert!(
            matches!(
                err,
                ConfigTransactionError::InvalidBlob(ConfigError::BadCrc)
            ),
            "got {err:?}"
        );
        assert_eq!(txn.client().transport().sent.len(), 0);
    }

    #[tokio::test]
    async fn oversize_blob_refused_before_sending() {
        // 128 records → 2060 B, over the 2048 B cap (length matches
        // moduleCount, so the TooBig check is the one that fires).
        let mut blob = vec![0u8; 8 + 128 * 16 + 4];
        blob[0..4].copy_from_slice(b"RSHG");
        blob[4] = 2;
        blob[5] = 128;
        let mut txn = txn_with(good_blob());
        let err = txn
            .stage(&blob)
            .await
            .expect_err("2060-byte blob must be refused");
        assert!(
            matches!(
                err,
                ConfigTransactionError::InvalidBlob(ConfigError::TooBig)
            ),
            "got {err:?}"
        );
        assert_eq!(txn.client().transport().sent.len(), 0);
    }

    #[tokio::test]
    async fn multi_chunk_blob_stages_in_two_chunks() {
        let big = big_blob();
        let mut txn = txn_with(good_blob());
        txn.stage(&big).await.expect("big blob stages");
        let board = txn.client().transport();
        assert_eq!(board.count_cmd(Opcode::SetCfg as u8), 2);
        let chunks: Vec<SetCfgChunk> = board
            .sent
            .iter()
            .filter_map(|bytes| ControlFrame::parse_structure(bytes).ok())
            .filter(|frame| frame.cmd == Opcode::SetCfg as u8)
            .map(|frame| SetCfgChunk::decode(&frame.payload).expect("chunk decodes"))
            .collect();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].offset, 0);
        assert_eq!(chunks[0].data.len(), 1016);
        assert_eq!(chunks[0].total, 1788);
        assert_eq!(chunks[1].offset, 1016);
        assert_eq!(chunks[1].data.len(), 772);
        assert_eq!(chunks[1].total, 1788);
        txn.commit().await.expect("commit succeeds");
        txn.confirm().await.expect("confirm succeeds");
        assert_eq!(txn.client().transport().stored, big);
    }

    #[tokio::test]
    async fn read_current_blob_round_trips_big_blob() {
        let big = big_blob();
        let mut txn = txn_with(big.clone());
        let read = txn.read_current_blob().await.expect("read-back succeeds");
        assert_eq!(read, big);
        // 1019 B first (board clamp), then the remaining 769 B.
        assert_eq!(txn.client().transport().count_cmd(Opcode::GetCfg as u8), 2);
    }

    #[tokio::test]
    async fn commit_pin_conflict_is_typed_and_not_retried() {
        let mut board = FakeBoard::new(good_blob());
        board.commit_status = ErrorCode::PinConflict;
        board.commit_err_body = vec![2, 1, 2];
        let mut txn = txn(board);
        txn.stage(&good_blob_v2()).await.expect("stage succeeds");
        let err = txn
            .commit()
            .await
            .expect_err("conflicting commit must fail");
        // Typed error with the board's errCodes on the live path
        // (`execute_with_status` returns the error body): [2, 1, 2] means
        // two codes — PinConflict (1) and ValueOutOfRange (2).
        assert!(
            matches!(
                err,
                ConfigTransactionError::PinConflict {
                    codes: ref c
                } if *c == vec![ConfigErrCode::PinConflict, ConfigErrCode::ValueOutOfRange]
            ),
            "got {err:?}"
        );
        // No retry: a conflicting commit cannot succeed by resending it.
        assert_eq!(txn.client().transport().count_cmd(Opcode::Commit as u8), 1);
        // The staged blob stays on the board; the live config is untouched.
        assert_eq!(txn.state(), ConfigState::Staged);
        assert_eq!(txn.client().transport().stored, good_blob());
    }

    #[test]
    fn commit_error_body_parses_err_codes() {
        assert_eq!(
            parse_commit_error_body(&[2, 1, 2]),
            vec![ConfigErrCode::PinConflict, ConfigErrCode::ValueOutOfRange]
        );
        assert_eq!(
            parse_commit_error_body(&[1, 4]),
            vec![ConfigErrCode::CrcMismatch]
        );
        assert_eq!(
            parse_commit_error_body(&[3, 3, 5, 9]),
            vec![
                ConfigErrCode::UnknownModule,
                ConfigErrCode::SizeMismatch,
                ConfigErrCode::Unknown(9),
            ]
        );
        // Empty body, zero count, count larger than the bytes present.
        assert_eq!(parse_commit_error_body(&[]), Vec::<ConfigErrCode>::new());
        assert_eq!(parse_commit_error_body(&[0]), Vec::<ConfigErrCode>::new());
        assert_eq!(
            parse_commit_error_body(&[5, 1]),
            vec![ConfigErrCode::PinConflict]
        );
        // The board caps the list at 8 codes.
        let body = [8, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2];
        assert_eq!(parse_commit_error_body(&body).len(), 8);
        assert_eq!(
            parse_commit_error_body(&body),
            vec![ConfigErrCode::PinConflict; 8]
        );
    }

    #[tokio::test]
    async fn confirm_timeout_rolls_back_automatically() {
        let mut board = FakeBoard::new(good_blob());
        board.confirm_behavior = ConfirmBehavior::Silent;
        let mut txn = txn(board);
        let err = txn
            .apply(&good_blob_v2())
            .await
            .expect_err("silent confirm must fail");
        assert!(
            matches!(err, ConfigTransactionError::ConfirmTimeout),
            "got {err:?}"
        );
        // Automatic rollback: REVERT sent, last-good verified by read-back.
        assert_eq!(txn.state(), ConfigState::RolledBack);
        assert_eq!(txn.client().transport().stored, good_blob());
        assert_eq!(txn.last_good_blob(), Some(good_blob().as_slice()));
        let board = txn.client().transport();
        assert_eq!(board.count_cmd(Opcode::Revert as u8), 1);
        // Three GET_CFGs: one reading last-good at stage, one H2
        // read-back after the confirm timeout (live config still the
        // old blob, so the rollback proceeds), one verifying the
        // rollback. Three CONFIRMs: the client retried twice.
        assert_eq!(board.count_cmd(Opcode::GetCfg as u8), 3);
        assert_eq!(board.count_cmd(Opcode::Confirm as u8), 3);
    }

    #[tokio::test]
    async fn confirm_timeout_but_new_config_live_is_confirmed_not_rolled_back() {
        // H2 regression: the confirm executed on the board (the new
        // config is live) but every reply was lost, so the client
        // times out. apply() must read the live config back, see the
        // staged blob is already live, and confirm — never REVERT a
        // good config that is already running.
        let mut board = FakeBoard::new(good_blob());
        board.apply_silent_confirm = true;
        let mut txn = txn(board);
        txn.apply(&good_blob_v2())
            .await
            .expect("confirm timeout with live new config is a success");
        assert_eq!(txn.state(), ConfigState::Confirmed);
        assert_eq!(txn.client().transport().stored, good_blob_v2());
        assert_eq!(txn.last_good_blob(), Some(good_blob_v2().as_slice()));
        let board = txn.client().transport();
        assert_eq!(board.count_cmd(Opcode::Revert as u8), 0, "no REVERT sent");
        assert_eq!(board.count_cmd(Opcode::Confirm as u8), 3);
    }

    #[tokio::test]
    async fn rollback_without_last_good_blob_sends_no_revert() {
        // M8: a transaction resumed from a snapshot with no saved
        // last-good blob refuses to roll back BEFORE sending REVERT,
        // with the distinct NoLastGoodBlob error.
        use super::ConfigTxnSnapshot;
        let board = FakeBoard::new(good_blob());
        let client =
            ControlClient::new(board, Session::new()).with_timeout(Duration::from_millis(10));
        let snapshot = ConfigTxnSnapshot {
            state: ConfigState::Committed,
            staged_blob: Some(good_blob_v2()),
            last_good_blob: None,
            commit_deadline: Duration::from_secs(30),
            committed_at: Some(std::time::Instant::now()),
        };
        let mut txn = ConfigTransaction::resume(client, snapshot);
        let err = txn
            .rollback()
            .await
            .expect_err("rollback without last-good must fail");
        assert!(
            matches!(err, ConfigTransactionError::NoLastGoodBlob),
            "got {err:?}"
        );
        assert_eq!(
            txn.client().transport().count_cmd(Opcode::Revert as u8),
            0,
            "REVERT must not be sent without a blob to verify against"
        );
        assert_eq!(txn.state(), ConfigState::Committed);
    }

    #[tokio::test]
    async fn read_current_blob_rejects_zero_total() {
        // L12: a board reporting totalLen 0 has no live config to
        // read — BadLength, never an empty "live" blob.
        let mut txn = txn_with(Vec::new());
        let err = txn
            .read_current_blob()
            .await
            .expect_err("zero-total GET_CFG must fail");
        assert!(
            matches!(
                err,
                ConfigTransactionError::Control(ControlError::Parse(ControlParseError::BadLength))
            ),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn confirm_retry_after_dropped_reply_succeeds() {
        let mut board = FakeBoard::new(good_blob());
        board.confirm_behavior = ConfirmBehavior::DropNext;
        let mut txn = txn(board);
        txn.stage(&good_blob_v2()).await.expect("stage succeeds");
        txn.commit().await.expect("commit succeeds");
        txn.confirm()
            .await
            .expect("retry after a dropped reply succeeds");
        assert_eq!(txn.state(), ConfigState::Confirmed);
        assert_eq!(txn.client().transport().stored, good_blob_v2());
        assert_eq!(txn.client().transport().count_cmd(Opcode::Confirm as u8), 2);
    }

    #[tokio::test]
    async fn double_commit_is_typed_error() {
        let mut txn = txn_with(good_blob());
        txn.stage(&good_blob_v2()).await.expect("stage succeeds");
        txn.commit().await.expect("first commit succeeds");
        let err = txn.commit().await.expect_err("second commit must fail");
        assert!(
            matches!(
                err,
                ConfigTransactionError::InvalidState {
                    state: ConfigState::Committed,
                    operation: "commit"
                }
            ),
            "got {err:?}"
        );
        assert_eq!(txn.client().transport().count_cmd(Opcode::Commit as u8), 1);
    }

    #[tokio::test]
    async fn confirm_without_commit_is_typed_error() {
        let mut txn = txn_with(good_blob());
        let err = txn
            .confirm()
            .await
            .expect_err("confirm from Idle must fail");
        assert!(
            matches!(
                err,
                ConfigTransactionError::InvalidState {
                    state: ConfigState::Idle,
                    operation: "confirm"
                }
            ),
            "got {err:?}"
        );
        assert_eq!(txn.client().transport().sent.len(), 0);
        txn.stage(&good_blob_v2()).await.expect("stage succeeds");
        let err = txn
            .confirm()
            .await
            .expect_err("confirm from Staged must fail");
        assert!(
            matches!(
                err,
                ConfigTransactionError::InvalidState {
                    state: ConfigState::Staged,
                    operation: "confirm"
                }
            ),
            "got {err:?}"
        );
        assert_eq!(txn.client().transport().count_cmd(Opcode::Confirm as u8), 0);
    }

    #[tokio::test]
    async fn rollback_from_staged_discards_staged() {
        let mut txn = txn_with(good_blob());
        txn.stage(&good_blob_v2()).await.expect("stage succeeds");
        txn.rollback().await.expect("rollback from Staged succeeds");
        assert_eq!(txn.state(), ConfigState::RolledBack);
        assert_eq!(txn.client().transport().stored, good_blob());
        assert_eq!(txn.staged_blob(), None);
        // Nothing is staged any more: commit is now illegal.
        let err = txn
            .commit()
            .await
            .expect_err("commit after rollback must fail");
        assert!(
            matches!(
                err,
                ConfigTransactionError::InvalidState {
                    state: ConfigState::RolledBack,
                    ..
                }
            ),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn rollback_mismatch_is_rollback_failed() {
        let mut board = FakeBoard::new(good_blob());
        board.corrupt_on_revert = true;
        let mut txn = txn(board);
        txn.stage(&good_blob_v2()).await.expect("stage succeeds");
        txn.commit().await.expect("commit succeeds");
        let err = txn
            .rollback()
            .await
            .expect_err("mismatched read-back must fail");
        assert!(
            matches!(err, ConfigTransactionError::RollbackFailed),
            "got {err:?}"
        );
        // REVERT was sent, but the verification failed, so the state
        // does not claim a successful rollback.
        assert_eq!(txn.state(), ConfigState::Committed);
    }

    #[tokio::test]
    async fn stage_twice_replaces_staged_blob() {
        let mut txn = txn_with(good_blob());
        txn.stage(&good_blob_v2())
            .await
            .expect("first stage succeeds");
        txn.stage(&good_blob_v3()).await.expect("re-stage succeeds");
        assert_eq!(txn.state(), ConfigState::Staged);
        assert_eq!(txn.staged_blob(), Some(good_blob_v3().as_slice()));
        txn.commit().await.expect("commit succeeds");
        txn.confirm().await.expect("confirm succeeds");
        assert_eq!(txn.client().transport().stored, good_blob_v3());
    }

    #[tokio::test]
    async fn pin_probe_passthrough_and_validation() {
        let mut txn = txn_with(good_blob());
        txn.pin_probe(&PinProbeRequest {
            pin: 34,
            duration_ms: 1000,
            mode: 0,
        })
        .await
        .expect("PIN_PROBE succeeds");
        assert_eq!(
            txn.client().transport().count_cmd(Opcode::PinProbe as u8),
            1
        );
        // Probe is independent of the transaction state.
        assert_eq!(txn.state(), ConfigState::Idle);
        let err = txn
            .pin_probe(&PinProbeRequest {
                pin: 34,
                duration_ms: 50,
                mode: 0,
            })
            .await
            .expect_err("duration 50 ms must fail");
        assert!(
            matches!(
                err,
                ConfigTransactionError::Control(ControlError::InvalidArgument(_))
            ),
            "got {err:?}"
        );
        // Nothing was sent for the invalid probe.
        assert_eq!(
            txn.client().transport().count_cmd(Opcode::PinProbe as u8),
            1
        );
    }

    /// A transaction split across two fresh sessions (H5-control):
    /// stage under one client, snapshot, resume over a new client
    /// (fresh reqId, same board), commit, snapshot, resume again,
    /// confirm — the board ends up on the new config. This is the
    /// exact shape the Tauri commands use (one command per step).
    #[tokio::test]
    async fn resume_across_fresh_sessions_completes_the_flow() {
        let board = FakeBoard::new(good_blob());
        let mut first = txn(board);
        first.stage(&good_blob_v2()).await.expect("stage succeeds");
        let snapshot = first.snapshot();
        assert_eq!(snapshot.state, ConfigState::Staged);
        assert_eq!(snapshot.staged_blob, Some(good_blob_v2()));
        assert_eq!(snapshot.last_good_blob, Some(good_blob()));
        // The first client is dropped; the second client starts a fresh
        // session over the same board. A fresh session resets the
        // board-side reply cache (`net_new_session()` in firmware
        // `net.cpp`), so the cached replies are cleared here — the
        // staged buffer itself survives the session change.
        let mut board = first.into_inner().into_inner().0;
        board.cached.clear();
        let client =
            ControlClient::new(board, Session::new()).with_timeout(Duration::from_millis(10));
        let mut second = ConfigTransaction::resume(client, snapshot);
        assert_eq!(second.state(), ConfigState::Staged);
        second.commit().await.expect("commit succeeds");
        let snapshot = second.snapshot();
        assert_eq!(snapshot.state, ConfigState::Committed);
        assert!(snapshot.committed_at.is_some());
        let mut board = second.into_inner().into_inner().0;
        board.cached.clear();
        let client =
            ControlClient::new(board, Session::new()).with_timeout(Duration::from_millis(10));
        let mut third = ConfigTransaction::resume(client, snapshot);
        third.confirm().await.expect("confirm succeeds");
        assert_eq!(third.state(), ConfigState::Confirmed);
        assert_eq!(third.client().transport().stored, good_blob_v2());
    }

    /// A default snapshot resumes to an idle transaction.
    #[tokio::test]
    async fn default_snapshot_resumes_idle() {
        let board = FakeBoard::new(good_blob());
        let client =
            ControlClient::new(board, Session::new()).with_timeout(Duration::from_millis(10));
        let txn = ConfigTransaction::resume(client, super::ConfigTxnSnapshot::default());
        assert_eq!(txn.state(), ConfigState::Idle);
        assert_eq!(txn.confirm_deadline(), Duration::from_secs(30));
    }
}
