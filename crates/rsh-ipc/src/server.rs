//! IPC server: accepts one client at a time per connection, reads
//! JSON-lines requests, dispatches them against the [`AppApi`], and
//! writes JSON-lines responses. Connections that sent `subscribe`
//! also receive [`AppEvent`] pushes via the shared broadcast
//! channel the daemon feeds.
//!
//! Transports:
//! * Unix: [`tokio::net::UnixListener`] on the socket path (0600).
//! * Windows: a tokio named-pipe server on `\\.\pipe\rsh-daemon`
//!   (one instance per connection; the pipe is local-only).

use std::sync::Arc;

#[cfg(unix)]
use std::path::Path;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::broadcast;

use rsh_api::{AppApi, AppEvent};

use crate::{
    IPC_PROTOCOL_VERSION, IpcError, IpcErrorBody, IpcEventFrame, IpcResponse, dispatch,
    encode_response, parse_request,
};

/// The IPC server: owns the API handle and the event broadcast
/// channel. One `serve_*` task per listener; each connection gets
/// its own task.
pub struct IpcServer {
    api: Arc<dyn AppApi>,
    events: broadcast::Sender<AppEvent>,
}

impl IpcServer {
    /// Build a server over `api`. `events` is the broadcast channel
    /// the daemon publishes [`AppEvent`]s on (capacity ≥ 64).
    #[must_use]
    pub fn new(api: Arc<dyn AppApi>, events: broadcast::Sender<AppEvent>) -> Self {
        Self { api, events }
    }

    /// Serve one connection: read requests, dispatch, write
    /// responses, and forward broadcast events to subscribers. Ends
    /// when the client disconnects. Generic over the stream so the
    /// Unix socket (Linux) and the named pipe (Windows) share this
    /// code — both are `AsyncRead + AsyncWrite`.
    pub async fn serve_stream<S>(self: Arc<Self>, stream: S)
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let (reader, mut writer) = tokio::io::split(stream);
        let mut lines = BufReader::new(reader).lines();
        let mut subscribed = false;
        let mut event_rx = self.events.subscribe();
        loop {
            tokio::select! {
                line = lines.next_line() => {
                    let Ok(Some(line)) = line else {
                        break; // client closed / read error
                    };
                    if line.trim().is_empty() {
                        continue;
                    }
                    let response = self.handle_line(&line);
                    if let Ok(resp) = &response
                        && let Some(result) = &resp.result
                        && result
                            .get("subscribed")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false)
                    {
                        subscribed = true;
                    }
                    match response.and_then(|r| encode_response(&r)) {
                        Ok(text) => {
                            if writer.write_all(text.as_bytes()).await.is_err()
                                || writer.write_all(b"\n").await.is_err()
                            {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                event = event_rx.recv(), if subscribed => {
                    let Ok(event) = event else { break };
                    let frame = IpcEventFrame { v: IPC_PROTOCOL_VERSION, event };
                    match serde_json::to_string(&frame) {
                        Ok(text) => {
                            if writer.write_all(text.as_bytes()).await.is_err()
                                || writer.write_all(b"\n").await.is_err()
                            {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
        }
    }

    /// Handle one request line → response (version check, dispatch,
    /// error mapping). Split out for unit tests. A version mismatch
    /// is answered as an error *response* (with the request's id
    /// when it can be read), never as a dropped connection — the
    /// client must learn the versions disagree.
    fn handle_line(&self, line: &str) -> Result<IpcResponse, IpcError> {
        let request = match parse_request(line) {
            Ok(request) => request,
            Err(IpcError::UnsupportedVersion(v)) => {
                let id = serde_json::from_str::<serde_json::Value>(line)
                    .ok()
                    .and_then(|value| value.get("id").and_then(serde_json::Value::as_u64))
                    .unwrap_or(0);
                return Ok(IpcResponse {
                    v: IPC_PROTOCOL_VERSION,
                    id,
                    ok: false,
                    result: None,
                    error: Some(IpcErrorBody {
                        code: "UnsupportedVersion".to_string(),
                        message: format!("protocol version {v} not supported"),
                    }),
                });
            }
            Err(other) => return Err(other),
        };
        let id = request.id;
        match dispatch(&self.api, &request) {
            Ok(result) => Ok(IpcResponse {
                v: IPC_PROTOCOL_VERSION,
                id,
                ok: true,
                result: Some(result),
                error: None,
            }),
            Err(IpcError::UnknownMethod(method)) => Ok(IpcResponse {
                v: IPC_PROTOCOL_VERSION,
                id,
                ok: false,
                result: None,
                error: Some(IpcErrorBody {
                    code: "UnknownMethod".to_string(),
                    message: format!("unknown method: {method}"),
                }),
            }),
            Err(IpcError::Api(code)) => Ok(IpcResponse {
                v: IPC_PROTOCOL_VERSION,
                id,
                ok: false,
                result: None,
                error: Some(IpcErrorBody {
                    code: code.clone(),
                    message: format!("daemon API error: {code}"),
                }),
            }),
            Err(IpcError::UnsupportedVersion(v)) => Ok(IpcResponse {
                v: IPC_PROTOCOL_VERSION,
                id,
                ok: false,
                result: None,
                error: Some(IpcErrorBody {
                    code: "UnsupportedVersion".to_string(),
                    message: format!("protocol version {v} not supported"),
                }),
            }),
            Err(other) => Ok(IpcResponse {
                v: IPC_PROTOCOL_VERSION,
                id,
                ok: false,
                result: None,
                error: Some(IpcErrorBody {
                    code: "Protocol".to_string(),
                    message: other.to_string(),
                }),
            }),
        }
    }
}

/// Serve a Unix listener forever (Linux): accept connections and
/// spawn one task per client. Returns only on listener error.
/// Unix-only: `tokio::net::UnixListener` does not exist on Windows
/// (the daemon serves a named pipe there instead).
#[cfg(unix)]
pub async fn serve_listener(server: Arc<IpcServer>, listener: tokio::net::UnixListener) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let server = Arc::clone(&server);
                tokio::spawn(async move {
                    server.serve_stream(stream).await;
                });
            }
            Err(_) => {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
}

/// Bind a Unix socket at `path` with mode `0600` (current user
/// only). A stale socket file from a previous run is removed first.
///
/// # Errors
///
/// Returns [`IpcError::Io`] when the socket cannot be bound.
#[cfg(unix)]
pub fn bind_unix_socket(path: &Path) -> Result<tokio::net::UnixListener, IpcError> {
    use std::os::unix::fs::PermissionsExt;
    if path.exists() {
        let _ = std::fs::remove_file(path);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| IpcError::Io(format!("cannot create IPC dir: {e}")))?;
    }
    let listener = tokio::net::UnixListener::bind(path)
        .map_err(|e| IpcError::Io(format!("cannot bind IPC socket {}: {e}", path.display())))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| IpcError::Io(format!("cannot chmod IPC socket: {e}")))?;
    Ok(listener)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsh_api::connection::ConnectionState;
    use rsh_api::{
        ApiTransport, ConnectionInfo, DraftStatus, PairingInfo, SnapshotInfo, TuningSettings,
        UserError, validate_tuning,
    };
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Mutex;

    /// Minimal stub API for the server tests.
    struct StubApi {
        version: String,
        output: Mutex<bool>,
        tunings: Mutex<BTreeMap<String, TuningSettings>>,
        dirty: Mutex<BTreeSet<String>>,
    }

    impl StubApi {
        fn clear_tunings(&self) {
            if let Ok(mut tunings) = self.tunings.lock() {
                tunings.clear();
            }
        }
    }

    impl AppApi for StubApi {
        fn version(&self) -> String {
            self.version.clone()
        }
        fn connection(&self) -> ConnectionInfo {
            ConnectionInfo {
                state: ConnectionState::Active,
                label: "Connected".to_string(),
                transport: ApiTransport::Wifi,
                address: "192.168.4.1".to_string(),
                session_active: true,
                error: None,
                hint: None,
            }
        }
        fn connect(&self, _transport: ApiTransport) -> Result<ConnectionInfo, UserError> {
            Ok(self.connection())
        }
        fn disconnect(&self) -> Result<ConnectionInfo, UserError> {
            Ok(self.connection())
        }
        fn pairing_status(&self) -> PairingInfo {
            PairingInfo {
                seconds_left: 0,
                password: None,
                error: None,
                error_detail: None,
                board_mac: None,
                replacement_required: false,
            }
        }
        fn begin_pairing(&self) -> Result<PairingInfo, UserError> {
            Ok(self.pairing_status())
        }
        fn snapshot(&self) -> SnapshotInfo {
            SnapshotInfo::default()
        }
        fn output_enabled(&self) -> bool {
            *self
                .output
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        }
        fn set_output_enabled(&self, enabled: bool) {
            *self
                .output
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = enabled;
        }
        fn get_tuning(&self, module: &str) -> TuningSettings {
            self.tunings
                .lock()
                .ok()
                .and_then(|tunings| tunings.get(module).cloned())
                .unwrap_or_else(|| TuningSettings::neutral(module))
        }
        fn set_tuning(&self, tuning: &TuningSettings) -> Result<TuningSettings, UserError> {
            let errors = validate_tuning(tuning);
            if !errors.is_empty() {
                return Err(UserError::from_connection(
                    rsh_api::ConnectionError::Unknown,
                    format!("invalid tuning: {}", errors.join("; ")),
                ));
            }
            if let Ok(mut tunings) = self.tunings.lock() {
                tunings.insert(tuning.module.clone(), tuning.clone());
            }
            if let Ok(mut dirty) = self.dirty.lock() {
                dirty.insert(tuning.module.clone());
            }
            Ok(tuning.clone())
        }
        fn draft_status(&self) -> Option<DraftStatus> {
            let dirty_modules: Vec<String> = self
                .dirty
                .lock()
                .ok()
                .map(|dirty| dirty.iter().cloned().collect())
                .unwrap_or_default();
            if dirty_modules.is_empty() {
                return None;
            }
            Some(DraftStatus {
                profile_id: "stub".to_string(),
                profile_name: "Stub".to_string(),
                dirty: true,
                dirty_modules,
            })
        }
        fn save_draft(&self) -> Result<bool, UserError> {
            Ok(self.dirty.lock().is_ok_and(|mut dirty| {
                let had = !dirty.is_empty();
                dirty.clear();
                had
            }))
        }
        fn discard_draft(&self) -> bool {
            let had = self.dirty.lock().is_ok_and(|mut dirty| {
                let had = !dirty.is_empty();
                dirty.clear();
                had
            });
            if had {
                self.clear_tunings();
            }
            had
        }
    }

    fn stub_server() -> Arc<IpcServer> {
        let api: Arc<dyn AppApi> = Arc::new(StubApi {
            version: "1.0.2".to_string(),
            output: Mutex::new(false),
            tunings: Mutex::new(BTreeMap::new()),
            dirty: Mutex::new(BTreeSet::new()),
        });
        let (tx, _rx) = broadcast::channel(64);
        Arc::new(IpcServer::new(api, tx))
    }

    #[test]
    fn ping_and_version_round_trip_through_dispatch() {
        let server = stub_server();
        let resp = server
            .handle_line(r#"{"v":1,"id":3,"method":"ping","params":{}}"#)
            .expect("ping parses");
        assert!(resp.ok);
        assert_eq!(resp.id, 3);
        assert_eq!(
            resp.result.expect("result").get("pong"),
            Some(&serde_json::Value::Bool(true))
        );
        let resp = server
            .handle_line(r#"{"v":1,"id":4,"method":"version","params":{}}"#)
            .expect("version parses");
        assert_eq!(
            resp.result.expect("result").get("version"),
            Some(&serde_json::Value::String("1.0.2".to_string()))
        );
    }

    #[test]
    fn wrong_version_is_rejected() {
        let server = stub_server();
        let resp = server
            .handle_line(r#"{"v":99,"id":1,"method":"ping","params":{}}"#)
            .expect("version error is a response, not a transport failure");
        assert!(!resp.ok);
        assert_eq!(resp.error.expect("error").code, "UnsupportedVersion");
    }

    #[test]
    fn unknown_method_is_an_error_response() {
        let server = stub_server();
        let resp = server
            .handle_line(r#"{"v":1,"id":2,"method":"explode","params":{}}"#)
            .expect("unknown method is a response");
        assert!(!resp.ok);
        assert_eq!(resp.error.expect("error").code, "UnknownMethod");
    }

    #[test]
    fn tuning_methods_round_trip_through_dispatch() {
        let server = stub_server();
        // get_tuning: neutral default before any edit.
        let resp = server
            .handle_line(r#"{"v":1,"id":10,"method":"get_tuning","params":{"module":"steering"}}"#)
            .expect("get_tuning parses");
        assert!(resp.ok);
        let got = resp.result.expect("result");
        assert_eq!(got.get("module"), Some(&serde_json::json!("steering")));
        assert_eq!(got.get("curve"), Some(&serde_json::json!(1.0)));
        // set_tuning: valid record is stored + echoed.
        let resp = server
            .handle_line(r#"{"v":1,"id":11,"method":"set_tuning","params":{"module":"steering","deadzone":5.0,"curve":1.6,"minMv":0.0,"maxMv":3300.0,"smoothing":10.0}}"#)
            .expect("set_tuning parses");
        assert!(resp.ok, "resp: {resp:?}");
        let got = resp.result.expect("result");
        assert_eq!(got.get("deadzone"), Some(&serde_json::json!(5.0)));
        assert_eq!(got.get("curve"), Some(&serde_json::json!(1.6)));
        // draft_status: dirty after the edit.
        let resp = server
            .handle_line(r#"{"v":1,"id":12,"method":"draft_status","params":{}}"#)
            .expect("draft_status parses");
        assert!(resp.ok);
        let got = resp.result.expect("result");
        assert_eq!(got.get("dirty"), Some(&serde_json::Value::Bool(true)));
        assert_eq!(
            got.get("dirtyModules"),
            Some(&serde_json::json!(["steering"]))
        );
        // set_tuning: invalid record is an API error, not a crash.
        let resp = server
            .handle_line(r#"{"v":1,"id":13,"method":"set_tuning","params":{"module":"steering","deadzone":5.0,"curve":9.0,"minMv":0.0,"maxMv":3300.0,"smoothing":10.0}}"#)
            .expect("invalid set_tuning parses");
        assert!(!resp.ok);
        assert_eq!(resp.error.expect("error").code, "Unknown");
        // save_draft: saves, then the draft is clean (null status).
        let resp = server
            .handle_line(r#"{"v":1,"id":14,"method":"save_draft","params":{}}"#)
            .expect("save_draft parses");
        assert!(resp.ok);
        assert_eq!(
            resp.result.expect("result").get("saved"),
            Some(&serde_json::Value::Bool(true))
        );
        let resp = server
            .handle_line(r#"{"v":1,"id":15,"method":"draft_status","params":{}}"#)
            .expect("draft_status parses");
        assert!(resp.ok);
        assert!(resp.result.expect("result").is_null());
        // discard_draft: nothing to discard when clean.
        let resp = server
            .handle_line(r#"{"v":1,"id":16,"method":"discard_draft","params":{}}"#)
            .expect("discard_draft parses");
        assert!(resp.ok);
        assert_eq!(
            resp.result.expect("result").get("discarded"),
            Some(&serde_json::Value::Bool(false))
        );
        // Edit again, then discard reverts to neutral.
        let _ = server
            .handle_line(r#"{"v":1,"id":17,"method":"set_tuning","params":{"module":"steering","deadzone":8.0,"curve":1.6,"minMv":0.0,"maxMv":3300.0,"smoothing":10.0}}"#)
            .expect("set_tuning parses");
        let resp = server
            .handle_line(r#"{"v":1,"id":18,"method":"discard_draft","params":{}}"#)
            .expect("discard_draft parses");
        assert_eq!(
            resp.result.expect("result").get("discarded"),
            Some(&serde_json::Value::Bool(true))
        );
        let resp = server
            .handle_line(r#"{"v":1,"id":19,"method":"get_tuning","params":{"module":"steering"}}"#)
            .expect("get_tuning parses");
        assert_eq!(
            resp.result.expect("result").get("deadzone"),
            Some(&serde_json::json!(0.0))
        );
    }
}
