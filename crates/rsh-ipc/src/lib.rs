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
    /// `poll`, `output_enabled`, `set_output_enabled`, `subscribe`,
    /// `get_tuning`, `set_tuning`, `draft_status`, `save_draft`,
    /// `discard_draft`, `autostart`, `set_autostart`,
    /// `firmware_version`, `output_backend`, `set_output_backend`,
    /// `backup_now`, `list_profiles`, `activate_profile`,
    /// `create_profile`, `duplicate_profile`, `delete_profile`,
    /// `list_games`, `launch_game`, `stop_game`, `game_state`,
    /// `start_learn`, `cancel_learn`, `is_learning`,
    /// `take_learned_button`, `calibration`, `vjoy_status`,
    /// `install_vjoy`, `diag_stats`, `raw_mv`, `capture_centre`,
    /// `capture_pedal_min`, `capture_pedal_max`, `list_bindings`,
    /// `set_binding`, `clear_binding`, `reset_pairing`,
    /// `firmware_check`, `update_firmware`,
    /// `firmware_update_status`, `board_address`,
    /// `set_board_address`, `module_enabled`, `set_module_enabled`,
    /// `auto_pause`, `set_auto_pause`, `close_to_tray`,
    /// `set_close_to_tray`, `restore_backup`, `rename_profile`,
    /// `link_profile`, `knob_actions`, `set_knob_action`).
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
#[allow(clippy::too_many_lines)]
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
        // L-4 (v2.0.2): one combined live-state read — snapshot +
        // connection + output in a single round-trip, from one
        // server-side dispatch, so the three values are from the
        // same instant (no tearing between three sequential calls).
        "poll" => {
            Ok(serde_json::to_value(api.poll()).map_err(|e| IpcError::Protocol(e.to_string()))?)
        }
        // DS-29 (v2.0.3): one combined slow-state read — everything
        // the UI's ~1 Hz slow poller fetched in 30 sequential
        // round-trips, in a single dispatch (see `SlowPollState`).
        "slow_poll" => Ok(
            serde_json::to_value(api.slow_poll()).map_err(|e| IpcError::Protocol(e.to_string()))?,
        ),
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
        // v2.0.1 hotfix: Settings toggles/buttons (autostart,
        // firmware check, backend radio, backup) had no API before.
        // Dispatched by the helper below so `dispatch` stays under
        // the clippy line limit.
        "autostart" | "set_autostart" | "firmware_version" | "output_backend"
        | "set_output_backend" | "backup_now" => dispatch_settings(api, request),
        // v2.0.1 hotfix: profiles / games / learn — the UI rows and
        // buttons that were hardcoded / notification-only before.
        // Dispatched by the helper below so `dispatch` stays under
        // the clippy line limit.
        "list_profiles"
        | "activate_profile"
        | "create_profile"
        | "duplicate_profile"
        | "delete_profile"
        | "list_games"
        | "launch_game"
        | "stop_game"
        | "game_state"
        | "start_learn"
        | "cancel_learn"
        | "is_learning"
        | "take_learned_button" => dispatch_profiles_games_learn(api, request),
        // v2.0.1 hotfix BACKEND-2: calibration read, vJoy
        // status/install, diagnostics.
        "calibration" | "vjoy_status" | "install_vjoy" | "diag_stats" => {
            dispatch_calibration_vjoy_diag(api, request)
        }
        // v2.0.2: raw-mV read + capture (centre / pedal min / max).
        "raw_mv" | "capture_centre" | "capture_pedal_min" | "capture_pedal_max" => {
            dispatch_capture(api, request)
        }
        // v2.0.2: button→action bindings (list / set / clear).
        "list_bindings" | "set_binding" | "clear_binding" => dispatch_bindings(api, request),
        // v2.0.2 Round 3: pairing reset, firmware check/update,
        // board address, module toggles, auto-pause, tray, backup,
        // profile rename/link, knob actions.
        "reset_pairing"
        | "firmware_check"
        | "update_firmware"
        | "firmware_update_status"
        | "board_address"
        | "set_board_address"
        | "module_enabled"
        | "set_module_enabled"
        | "auto_pause"
        | "set_auto_pause"
        | "close_to_tray"
        | "set_close_to_tray"
        | "restore_backup"
        | "rename_profile"
        | "link_profile"
        | "knob_actions"
        | "set_knob_action" => dispatch_round3(api, request),
        other => Err(IpcError::UnknownMethod(other.to_string())),
    }
}

/// Dispatch the v2.0.2 capture methods. Split out of [`dispatch`]
/// so neither function trips `too_many_lines`.
fn dispatch_capture(
    api: &Arc<dyn AppApi>,
    request: &IpcRequest,
) -> Result<serde_json::Value, IpcError> {
    let params = &request.params;
    match request.method.as_str() {
        "raw_mv" => api
            .raw_mv()
            .map(|reading| serde_json::to_value(reading).unwrap_or(serde_json::Value::Null))
            .map_err(|error| detailed_api_error(&error)),
        "capture_centre" => api
            .capture_centre()
            .map(|mv| serde_json::json!({"mv": mv}))
            .map_err(|error| detailed_api_error(&error)),
        "capture_pedal_min" | "capture_pedal_max" => {
            let pedal = params
                .get("pedal")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let captured = if request.method == "capture_pedal_min" {
                api.capture_pedal_min(pedal)
            } else {
                api.capture_pedal_max(pedal)
            };
            captured
                .map(|mv| serde_json::json!({"mv": mv}))
                .map_err(|error| detailed_api_error(&error))
        }
        other => Err(IpcError::UnknownMethod(other.to_string())),
    }
}

/// Dispatch the v2.0.2 binding methods. Split out of [`dispatch`]
/// so neither function trips `too_many_lines`.
///
/// Wire shapes: `list_bindings` → `{"bindings": [{"button": 5,
/// "action": "horn"}]}`; `set_binding` takes `{"button": 5,
/// "action": "horn"}` and returns `{"binding": {...}}`;
/// `clear_binding` takes `{"button": 5}` and returns `{"cleared":
/// true}`. A non-numeric / out-of-range `button` param is an
/// `InvalidButton` error, never a silent default: the UI must not
/// bind the wrong button because it sent a bad number.
fn dispatch_bindings(
    api: &Arc<dyn AppApi>,
    request: &IpcRequest,
) -> Result<serde_json::Value, IpcError> {
    let params = &request.params;
    match request.method.as_str() {
        "list_bindings" => {
            let bindings = api.list_bindings();
            Ok(serde_json::json!({"bindings": bindings}))
        }
        "set_binding" => {
            let button = parse_binding_button(params)?;
            let action = params
                .get("action")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            api.set_binding(button, action)
                .map(|binding| serde_json::json!({"binding": binding}))
                .map_err(|error| detailed_api_error(&error))
        }
        "clear_binding" => {
            let button = parse_binding_button(params)?;
            api.clear_binding(button)
                .map(|cleared| serde_json::json!({"cleared": cleared}))
                .map_err(|error| detailed_api_error(&error))
        }
        other => Err(IpcError::UnknownMethod(other.to_string())),
    }
}

/// Parse the `button` param of a binding request: a JSON number
/// that fits in a `u8` (physical 1–32, encoder virtual 65–72 —
/// the range itself is the API's job, so a `u8` that is out of
/// range still reaches it and fails there with `InvalidButton`).
/// A missing / non-numeric / oversized value fails here with the
/// same stable code.
fn parse_binding_button(params: &serde_json::Value) -> Result<u8, IpcError> {
    params
        .get("button")
        .and_then(serde_json::Value::as_u64)
        .and_then(|n| u8::try_from(n).ok())
        .ok_or_else(|| {
            IpcError::Api(
                "InvalidButton|button must be a number (physical 1-32, encoder virtual 65-72)"
                    .to_string(),
            )
        })
}

/// Map a [`UserError`](rsh_api::UserError) to an IPC error that
/// keeps the reason: [`IpcError::Api`] otherwise carries only the
/// code, so a failed call would reach the UI as a bare "Unknown"
/// with the real reason lost. The payload is `code|detail` (the
/// detail never contains `|`); the UI splits it back out. The
/// capture and binding methods use this — every other method keeps
/// the existing code-only mapping.
fn detailed_api_error(error: &rsh_api::UserError) -> IpcError {
    let mut body = error.code.clone();
    body.push('|');
    body.push_str(error.detail.as_deref().unwrap_or(""));
    IpcError::Api(body)
}

/// Dispatch the v2.0.2 Round 3 methods. Split out of [`dispatch`]
/// so neither function trips `too_many_lines`.
///
/// Wire shapes: `reset_pairing` → the pairing status;
/// `firmware_check` → the [`FirmwareCheck`](rsh_api::FirmwareCheck);
/// `update_firmware` starts the flash and returns the running
/// status; `firmware_update_status` polls it; `board_address` →
/// `{"address"}`; `set_board_address` takes `{"address"}`;
/// `module_enabled` / `set_module_enabled` take `{"module"}` (+
/// `{"enabled"}` to set) and return `{"enabled"}`; `auto_pause` /
/// `set_auto_pause` use `{"paused"}`; `close_to_tray` /
/// `set_close_to_tray` use `{"close_to_tray"}` / `{"enabled"}`;
/// `restore_backup` → `{"message"}`; `rename_profile` takes
/// `{"id", "name"}`; `link_profile` takes `{"id", "game_slug"}`;
/// `knob_actions` → `{"knobs": [[...], [...]]}`; `set_knob_action`
/// takes `{"knob", "row", "action"}`.
#[allow(clippy::too_many_lines)]
fn dispatch_round3(
    api: &Arc<dyn AppApi>,
    request: &IpcRequest,
) -> Result<serde_json::Value, IpcError> {
    let params = &request.params;
    match request.method.as_str() {
        "reset_pairing" => api
            .reset_pairing()
            .map(|info| serde_json::to_value(info).unwrap_or(serde_json::Value::Null))
            .map_err(|error| detailed_api_error(&error)),
        "firmware_check" => api
            .firmware_check()
            .map(|check| serde_json::to_value(check).unwrap_or(serde_json::Value::Null))
            .map_err(|error| detailed_api_error(&error)),
        "update_firmware" => api
            .update_firmware()
            .map(|status| serde_json::to_value(status).unwrap_or(serde_json::Value::Null))
            .map_err(|error| detailed_api_error(&error)),
        "firmware_update_status" => Ok(serde_json::to_value(api.firmware_update_status())
            .map_err(|e| IpcError::Protocol(e.to_string()))?),
        "board_address" => Ok(serde_json::json!({"address": api.board_address()})),
        "set_board_address" => {
            let address = params
                .get("address")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            api.set_board_address(address)
                .map(|stored| serde_json::json!({"address": stored}))
                .map_err(|error| detailed_api_error(&error))
        }
        "module_enabled" => {
            let module = params
                .get("module")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            Ok(serde_json::json!({"enabled": api.module_enabled(module)}))
        }
        "set_module_enabled" => {
            let module = params
                .get("module")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let enabled = params
                .get("enabled")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            api.set_module_enabled(module, enabled)
                .map(|new_state| serde_json::json!({"enabled": new_state}))
                .map_err(|error| detailed_api_error(&error))
        }
        "auto_pause" => Ok(serde_json::json!({"paused": api.auto_pause()})),
        "set_auto_pause" => {
            let paused = params
                .get("paused")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            api.set_auto_pause(paused)
                .map(|new_state| serde_json::json!({"paused": new_state}))
                .map_err(|error| detailed_api_error(&error))
        }
        "close_to_tray" => Ok(serde_json::json!({"close_to_tray": api.close_to_tray()})),
        "set_close_to_tray" => {
            let enabled = params
                .get("enabled")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            api.set_close_to_tray(enabled)
                .map(|new_state| serde_json::json!({"close_to_tray": new_state}))
                .map_err(|error| detailed_api_error(&error))
        }
        "restore_backup" => api
            .restore_backup()
            .map(|message| serde_json::json!({"message": message}))
            .map_err(|error| detailed_api_error(&error)),
        "rename_profile" => {
            let id = params
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let name = params
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            api.rename_profile(id, name)
                .map(|stored| serde_json::json!({"name": stored}))
                .map_err(|error| detailed_api_error(&error))
        }
        "link_profile" => {
            let id = params
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let game_slug = params
                .get("game_slug")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            api.link_profile(id, game_slug)
                .map(|stored| serde_json::json!({"name": stored}))
                .map_err(|error| detailed_api_error(&error))
        }
        "knob_actions" => Ok(serde_json::json!({"knobs": api.knob_actions()})),
        "set_knob_action" => {
            let knob = parse_u8_param(params, "knob")?;
            let row = parse_u8_param(params, "row")?;
            let action = params
                .get("action")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            api.set_knob_action(knob, row, action)
                .map(|()| serde_json::json!({"ok": true}))
                .map_err(|error| detailed_api_error(&error))
        }
        other => Err(IpcError::UnknownMethod(other.to_string())),
    }
}

/// Parse a small numeric param (`knob`, `row`) from a request: a
/// JSON number that fits in a `u8`. A missing / non-numeric /
/// oversized value fails here with `InvalidButton` — the range
/// itself is the API's job, so an in-range `u8` that is out of range
/// still reaches it and fails there.
fn parse_u8_param(params: &serde_json::Value, name: &str) -> Result<u8, IpcError> {
    params
        .get(name)
        .and_then(serde_json::Value::as_u64)
        .and_then(|n| u8::try_from(n).ok())
        .ok_or_else(|| IpcError::Api(format!("InvalidButton|{name} must be a number")))
}

/// Dispatch the v2.0.1 BACKEND-2 calibration / vJoy / diagnostics
/// methods. Split out of [`dispatch`] so neither function trips
/// `too_many_lines`.
fn dispatch_calibration_vjoy_diag(
    api: &Arc<dyn AppApi>,
    request: &IpcRequest,
) -> Result<serde_json::Value, IpcError> {
    match request.method.as_str() {
        "calibration" => api
            .calibration()
            .map(|info| serde_json::to_value(info).unwrap_or(serde_json::Value::Null))
            .map_err(|e| IpcError::Api(e.code)),
        "vjoy_status" => api
            .vjoy_status()
            .map(|status| serde_json::to_value(status).unwrap_or(serde_json::Value::Null))
            .map_err(|e| IpcError::Api(e.code)),
        "install_vjoy" => api
            .install_vjoy()
            .map(|message| serde_json::json!({"message": message}))
            .map_err(|e| IpcError::Api(e.code)),
        "diag_stats" => api
            .diag_stats()
            .map(|stats| serde_json::to_value(stats).unwrap_or(serde_json::Value::Null))
            .map_err(|e| IpcError::Api(e.code)),
        other => Err(IpcError::UnknownMethod(other.to_string())),
    }
}

/// Dispatch the v2.0.1 Settings toggles/buttons (autostart,
/// firmware check, backend radio, backup). Split out of [`dispatch`]
/// so neither function trips `too_many_lines`.
fn dispatch_settings(
    api: &Arc<dyn AppApi>,
    request: &IpcRequest,
) -> Result<serde_json::Value, IpcError> {
    let params = &request.params;
    match request.method.as_str() {
        "autostart" => Ok(serde_json::json!({"autostart": api.autostart()})),
        "set_autostart" => {
            let enabled = params
                .get("enabled")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            api.set_autostart(enabled)
                .map(|new_state| serde_json::json!({"autostart": new_state}))
                .map_err(|e| IpcError::Api(e.code))
        }
        "firmware_version" => Ok(serde_json::json!({"firmware_version": api.firmware_version()})),
        "output_backend" => Ok(serde_json::json!({"output_backend": api.output_backend()})),
        "set_output_backend" => {
            let name = params
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            api.set_output_backend(name)
                .map(|backend| serde_json::json!({"output_backend": backend}))
                .map_err(|e| IpcError::Api(e.code))
        }
        "backup_now" => api
            .backup_now()
            .map(|message| serde_json::json!({"message": message}))
            .map_err(|e| IpcError::Api(e.code)),
        other => Err(IpcError::UnknownMethod(other.to_string())),
    }
}

/// Dispatch the v2.0.1 profiles / games / learn methods. Split out
/// of [`dispatch`] so neither function trips `too_many_lines`.
fn dispatch_profiles_games_learn(
    api: &Arc<dyn AppApi>,
    request: &IpcRequest,
) -> Result<serde_json::Value, IpcError> {
    let params = &request.params;
    match request.method.as_str() {
        "list_profiles" => Ok(serde_json::to_value(api.list_profiles())
            .map_err(|e| IpcError::Protocol(e.to_string()))?),
        "activate_profile" => {
            let id = params
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            api.activate_profile(id)
                .map(|name| serde_json::json!({"name": name}))
                .map_err(|e| IpcError::Api(e.code))
        }
        "create_profile" => {
            let name = params
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            api.create_profile(name)
                .map(|name| serde_json::json!({"name": name}))
                .map_err(|e| IpcError::Api(e.code))
        }
        "duplicate_profile" => {
            let id = params
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            api.duplicate_profile(id)
                .map(|name| serde_json::json!({"name": name}))
                .map_err(|e| IpcError::Api(e.code))
        }
        "delete_profile" => {
            let id = params
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            api.delete_profile(id)
                .map(|deleted| serde_json::json!({"deleted": deleted}))
                .map_err(|e| IpcError::Api(e.code))
        }
        "list_games" => Ok(serde_json::to_value(api.list_games())
            .map_err(|e| IpcError::Protocol(e.to_string()))?),
        "launch_game" => {
            let id = params
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            api.launch_game(id)
                .map(|game| serde_json::to_value(game).unwrap_or(serde_json::Value::Null))
                .map_err(|e| IpcError::Api(e.code))
        }
        "stop_game" => {
            let id = params
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            api.stop_game(id)
                .map(|game| serde_json::to_value(game).unwrap_or(serde_json::Value::Null))
                .map_err(|e| IpcError::Api(e.code))
        }
        "game_state" => {
            let id = params
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            api.game_state(id)
                .map(|game| serde_json::to_value(game).unwrap_or(serde_json::Value::Null))
                .map_err(|e| IpcError::Api(e.code))
        }
        "start_learn" => api
            .start_learn()
            .map(|learning| serde_json::json!({"learning": learning}))
            .map_err(|e| IpcError::Api(e.code)),
        "cancel_learn" => api
            .cancel_learn()
            .map(|learning| serde_json::json!({"learning": learning}))
            .map_err(|e| IpcError::Api(e.code)),
        "is_learning" => Ok(serde_json::json!({"learning": api.is_learning()})),
        "take_learned_button" => Ok(serde_json::json!({"button": api.take_learned_button()})),
        other => Err(IpcError::UnknownMethod(other.to_string())),
    }
}
