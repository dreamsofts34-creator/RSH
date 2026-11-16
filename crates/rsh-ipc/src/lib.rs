//! Local IPC (LOGIC-FIX-PLAN §2): named pipe on Windows, Unix
//! socket on Linux, JSON lines, versioned protocol.
//!
//! Wire format — one JSON object per line, UTF-8, `\\n` terminated:
//!
//! * Request (client → server):
//!   `{"v":1,"id":7,"method":"ping","params":{}}`
//! * Response (server → client):
//!   `{"v":1,"id":7,"ok":true,"result":{...}}` or
//!   `{"v":1,"id":7,"ok":false,"error":{"code":"…","message":"…"}}`
//! * Event push (server → client, no `id`):
//!   `{"v":1,"event":{…AppEvent JSON…}}`
//!
//! `v` is the protocol version; a client or server that sees a
//! version it does not understand must fail the call with
//! `UnsupportedVersion` instead of guessing.
//!
//! Access control:
//! * Linux: the socket file is created with mode `0600` and lives in
//!   the user's data dir (`~/.local/share/RSH/`), so only the current
//!   user can connect.
//! * Windows: the named pipe `\\.\pipe\rsh-daemon` is local to this
//!   machine and this session; an explicit current-user-only DACL is
//!   an OPEN Phase-7/Windows item (see the gate doc) — the pipe name
//!   is namespaced per user where the OS supports it.
//!
//! The server is generic over `Arc<dyn AppApi>`; the daemon passes
//! its real implementation, tests pass a stub.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rsh_api::{AppApi, AppEvent};
use serde::{Deserialize, Serialize};

/// IPC protocol version. Bump only with a breaking wire change.
pub const IPC_PROTOCOL_VERSION: u32 = 1;

/// Default socket / pipe name (no path — see [`socket_path`]).
pub const IPC_SOCKET_NAME: &str = "rsh-daemon.sock";
/// Windows named-pipe name.
pub const IPC_PIPE_NAME: &str = r"\\.\pipe\rsh-daemon";

/// Error from an IPC operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IpcError {
    /// The peer closed the connection.
    Closed,
    /// The request/response JSON did not parse, or a line was too long.
    Protocol(String),
    /// The peer speaks a protocol version this side does not support.
    UnsupportedVersion(u32),
    /// The method is unknown on this server.
    UnknownMethod(String),
    /// The daemon API call itself failed (carries the `UserError` code).
    Api(String),
    /// An I/O error (bind, connect, read, write).
    Io(String),
}

impl std::fmt::Display for IpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed => write!(f, "IPC connection closed"),
            Self::Protocol(message) => write!(f, "IPC protocol error: {message}"),
            Self::UnsupportedVersion(v) => {
                write!(
                    f,
                    "IPC protocol version {v} is not supported (want {IPC_PROTOCOL_VERSION})"
                )
            }
            Self::UnknownMethod(method) => write!(f, "IPC unknown method: {method}"),
            Self::Api(code) => write!(f, "daemon API error: {code}"),
            Self::Io(message) => write!(f, "IPC I/O error: {message}"),
        }
    }
}

impl std::error::Error for IpcError {}

/// The socket path for this user (Linux): `<data_dir>/rsh-daemon.sock`.
/// `data_dir` is the RSH data dir (`%AppData%\RSH` on Windows,
/// `~/.local/share/RSH` on Linux).
#[must_use]
pub fn socket_path(data_dir: &Path) -> PathBuf {
    data_dir.join(IPC_SOCKET_NAME)
}

/// One JSON-lines request.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct IpcRequest {
    /// Protocol version (must be [`IPC_PROTOCOL_VERSION`]).
    pub v: u32,
    /// Client-chosen id; echoed in the response.
    pub id: u64,
    /// Method name (`ping`, `version`, `connection`, `connect`,
    /// `disconnect`, `pairing_status`, `begin_pairing`, `snapshot`,
    /// `output_enabled`, `set_output_enabled`, `subscribe`,
    /// `get_tuning`, `set_tuning`, `draft_status`, `save_draft`,
    /// `discard_draft`).
    pub method: String,
    /// Method params (object; may be empty).
    #[serde(default)]
    pub params: serde_json::Value,
}

/// One JSON-lines response.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct IpcResponse {
    /// Protocol version.
    pub v: u32,
    /// Echo of the request id.
    pub id: u64,
    /// True on success.
    pub ok: bool,
    /// Result payload, when `ok`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// Error, when not `ok`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<IpcErrorBody>,
}

/// Error body inside an [`IpcResponse`].
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct IpcErrorBody {
    /// Stable code.
    pub code: String,
    /// Human-readable message (never a secret).
    pub message: String,
}

/// One JSON-lines push event (server → client).
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct IpcEventFrame {
    /// Protocol version.
    pub v: u32,
    /// The pushed event.
    pub event: AppEvent,
}

/// The client library the Slint UI (Phase 3) uses.
pub mod client;
/// The methods the IPC server answers, with their params.
pub mod server;

pub use client::IpcClient;
pub use server::IpcServer;
#[cfg(unix)]
pub use server::bind_unix_socket;
#[cfg(unix)]
pub use server::serve_listener;

/// Parse + version-check one request line. Shared by the server and
/// the tests so both sides enforce the version the same way.
///
/// # Errors
///
/// [`IpcError::Protocol`] when the line is not a valid request, and
/// [`IpcError::UnsupportedVersion`] when its `v` is not
/// [`IPC_PROTOCOL_VERSION`].
pub fn parse_request(line: &str) -> Result<IpcRequest, IpcError> {
    let request: IpcRequest =
        serde_json::from_str(line).map_err(|e| IpcError::Protocol(e.to_string()))?;
    if request.v != IPC_PROTOCOL_VERSION {
        return Err(IpcError::UnsupportedVersion(request.v));
    }
    Ok(request)
}

/// Serialise a response to one line (no trailing newline — the
/// writer adds it).
///
/// # Errors
///
/// [`IpcError::Protocol`] when the response does not serialise
/// (unreachable for these types in practice).
pub fn encode_response(response: &IpcResponse) -> Result<String, IpcError> {
    serde_json::to_string(response).map_err(|e| IpcError::Protocol(e.to_string()))
}

/// Dispatch one method against the API. Pure function over
/// [`AppApi`] — the server is a thin transport around this, and the
/// round-trip test drives it through the real socket.
///
/// # Errors
///
/// [`IpcError::UnknownMethod`] for an unknown method, and
/// [`IpcError::Api`] when the daemon API call itself fails.
pub fn dispatch(
    api: &Arc<dyn AppApi>,
    request: &IpcRequest,
) -> Result<serde_json::Value, IpcError> {
    use rsh_api::ApiTransport;
    let params = &request.params;
    match request.method.as_str() {
        "ping" => Ok(serde_json::json!({"pong": true})),
        "version" => Ok(serde_json::json!({"version": api.version()})),
        "connection" => Ok(serde_json::to_value(api.connection())
            .map_err(|e| IpcError::Protocol(e.to_string()))?),
        "connect" => {
            let transport = match params
                .get("transport")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("wifi")
            {
                "usb" => ApiTransport::Usb,
                _ => ApiTransport::Wifi,
            };
            api.connect(transport)
                .map(|info| serde_json::to_value(info).unwrap_or(serde_json::Value::Null))
                .map_err(|e| IpcError::Api(e.code))
        }
        "disconnect" => api
            .disconnect()
            .map(|info| serde_json::to_value(info).unwrap_or(serde_json::Value::Null))
            .map_err(|e| IpcError::Api(e.code)),
        "pairing_status" => Ok(serde_json::to_value(api.pairing_status())
            .map_err(|e| IpcError::Protocol(e.to_string()))?),
        "begin_pairing" => api
            .begin_pairing()
            .map(|info| serde_json::to_value(info).unwrap_or(serde_json::Value::Null))
            .map_err(|e| IpcError::Api(e.code)),
        "snapshot" => {
            Ok(serde_json::to_value(api.snapshot())
                .map_err(|e| IpcError::Protocol(e.to_string()))?)
        }
        "output_enabled" => Ok(serde_json::json!({"enabled": api.output_enabled()})),
        "set_output_enabled" => {
            let enabled = params
                .get("enabled")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            api.set_output_enabled(enabled);
            Ok(serde_json::json!({"enabled": api.output_enabled()}))
        }
        // `subscribe` is handled by the server (it marks the
        // connection as an event subscriber); dispatching it here
        // just acknowledges it.
        "subscribe" => Ok(serde_json::json!({"subscribed": true})),
        // L4 tuning transport: the UI sliders' values reach the
        // daemon's draft + live pipeline plan through these.
        "get_tuning" => {
            let module = params
                .get("module")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("steering");
            Ok(serde_json::to_value(api.get_tuning(module))
                .map_err(|e| IpcError::Protocol(e.to_string()))?)
        }
        "set_tuning" => {
            let tuning: rsh_api::TuningSettings = serde_json::from_value(params.clone())
                .map_err(|e| IpcError::Protocol(e.to_string()))?;
            api.set_tuning(&tuning)
                .map(|stored| serde_json::to_value(stored).unwrap_or(serde_json::Value::Null))
                .map_err(|e| IpcError::Api(e.code))
        }
        "draft_status" => Ok(serde_json::to_value(api.draft_status())
            .map_err(|e| IpcError::Protocol(e.to_string()))?),
        "save_draft" => api
            .save_draft()
            .map(|saved| serde_json::json!({"saved": saved}))
            .map_err(|e| IpcError::Api(e.code)),
        "discard_draft" => Ok(serde_json::json!({"discarded": api.discard_draft()})),
        other => Err(IpcError::UnknownMethod(other.to_string())),
    }
}
