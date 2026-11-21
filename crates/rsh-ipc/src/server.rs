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
        // B-1 (v2.0.2): responses from blocking handlers travel on
        // their own channel. Dispatch runs on `spawn_blocking`
        // (below), so one slow handler — a backend switch waiting
        // out a UAC prompt, a Wi-Fi password set — never stalls this
        // connection's event forwarding or its next request. The
        // channel is bounded (64): `blocking_send` in the handler
        // task applies backpressure instead of piling up responses
        // for a client that stopped reading. Responses can arrive
        // out of order; the client matches them by `id`.
        let (resp_tx, mut resp_rx) = tokio::sync::mpsc::channel::<String>(64);
        // DS-28: bound in-flight blocking dispatches per connection
        // — a client that floods the server gets a `Busy` error
        // instead of unbounded queued work. Cheap reads bypass this
        // entirely (they never take a permit).
        let dispatch_permits = Arc::new(tokio::sync::Semaphore::new(8));
        loop {
            tokio::select! {
                line = lines.next_line() => {
                    let Ok(Some(line)) = line else {
                        break; // client closed / read error
                    };
                    if line.trim().is_empty() {
                        continue;
                    }
                    // Cheap parse here (no I/O, never blocks): the
                    // subscribe flag is set on request, since the
                    // response now arrives asynchronously.
                    let parsed = parse_request(&line);
                    if parsed
                        .as_ref()
                        .is_ok_and(|request| request.method == "subscribe")
                    {
                        subscribed = true;
                    }
                    // DS-28: cheap reads run inline on this
                    // connection task — no `spawn_blocking` hop, no
                    // thread-pool wakeup. The response still travels
                    // on `resp_tx` so writes stay ordered with
                    // events. R2-49: `try_send`, never `send().await`
                    // here — awaiting the send inside the read arm
                    // wedged the connection task when the 64-slot
                    // response channel filled (the old path used
                    // `blocking_send` on a pool thread). A dropped
                    // cheap response is superseded by the next poll.
                    if parsed
                        .as_ref()
                        .is_ok_and(|request| is_cheap_method(&request.method))
                    {
                        if let Some(text) = self.response_text(&line) {
                            let _ = resp_tx.try_send(text);
                        }
                        continue;
                    }
                    let server = Arc::clone(&self);
                    let tx = resp_tx.clone();
                    let Ok(permit) = dispatch_permits.clone().try_acquire_owned() else {
                        // DS-28: too many dispatches in flight on
                        // this connection — answer `Busy` instead of
                        // queueing unbounded work. R2-49: `try_send`
                        // for the same reason as the cheap path —
                        // never await a send inside the read arm.
                        if let Some(text) = busy_response_text(&line) {
                            let _ = resp_tx.try_send(text);
                        }
                        continue;
                    };
                    tokio::task::spawn_blocking(move || {
                        // The permit is held until this handler
                        // finishes, then released.
                        let _permit = permit;
                        if let Some(text) = server.response_text(&line) {
                            let _ = tx.blocking_send(text);
                        }
                    });
                }
                resp = resp_rx.recv() => {
                    let Some(text) = resp else {
                        break; // all handler tasks are gone
                    };
                    if writer.write_all(text.as_bytes()).await.is_err()
                        || writer.write_all(b"\n").await.is_err()
                    {
                        break;
                    }
                }
                event = event_rx.recv(), if subscribed => {
                    if !Self::forward_event(&mut writer, event, &mut event_rx).await {
                        break;
                    }
                }
            }
        }
    }

    /// Forward one received broadcast outcome to the client.
    /// Returns `false` when the connection must end (channel closed,
    /// or a write failed).
    ///
    /// DS-17 (B-05): `Lagged` means this subscriber fell behind the
    /// 256-capacity channel — skip to the newest event instead of
    /// tearing the whole connection down (the old code treated it
    /// like `Closed`, so a burst of events during a UI stall forced
    /// a reconnect + re-subscribe). Only `Closed` ends the
    /// connection.
    ///
    /// R2-30: on `Lagged`, drain everything queued and forward the
    /// one-shot events the UI waits on (`ConnectionChanged`) in
    /// order, plus the newest event — a lagged burst must never
    /// silently drop a connection change (the old code kept only
    /// the newest event).
    async fn forward_event<W>(
        writer: &mut W,
        result: Result<AppEvent, broadcast::error::RecvError>,
        event_rx: &mut broadcast::Receiver<AppEvent>,
    ) -> bool
    where
        W: tokio::io::AsyncWrite + Unpin,
    {
        let events: Vec<AppEvent> = match result {
            Ok(event) => vec![event],
            Err(broadcast::error::RecvError::Lagged(_)) => {
                let mut drained = Vec::new();
                while let Ok(event) = event_rx.try_recv() {
                    drained.push(event);
                }
                let Some(latest) = drained.pop() else {
                    return true;
                };
                let mut out: Vec<AppEvent> = drained
                    .into_iter()
                    .filter(|event| matches!(event, AppEvent::ConnectionChanged { .. }))
                    .collect();
                if !out.contains(&latest) {
                    out.push(latest);
                }
                out
            }
            Err(broadcast::error::RecvError::Closed) => return false,
        };
        for event in events {
            let frame = IpcEventFrame {
                v: IPC_PROTOCOL_VERSION,
                event,
            };
            match serde_json::to_string(&frame) {
                Ok(text) => {
                    if writer.write_all(text.as_bytes()).await.is_err()
                        || writer.write_all(b"\n").await.is_err()
                    {
                        return false;
                    }
                }
                Err(_) => return false,
            }
        }
        true
    }

    /// Encode one request line's response (DS-28): shared by the
    /// inline cheap path and the `spawn_blocking` path. Returns
    /// `None` when the response itself cannot be encoded.
    fn response_text(&self, line: &str) -> Option<String> {
        // DS-82: the single instrumentation point — every dispatch
        // (inline or blocking) records its latency per method.
        let started = std::time::Instant::now();
        let method = parse_request(line).map_or_else(|_| "?".to_string(), |request| request.method);
        let out = match self.handle_line(line) {
            Ok(resp) => encode_response(&resp).ok(),
            Err(error) => {
                // Unparseable request: answer with a Protocol error
                // instead of dropping the connection silently.
                let id = request_id(line);
                encode_response(&IpcResponse {
                    v: IPC_PROTOCOL_VERSION,
                    id,
                    ok: false,
                    result: None,
                    error: Some(IpcErrorBody {
                        code: "Protocol".to_string(),
                        message: error.to_string(),
                    }),
                })
                .ok()
            }
        };
        crate::record_ipc_latency(&method, started.elapsed());
        out
    }

    /// Handle one request line → response (version check, dispatch,
    /// error mapping). Split out for unit tests. A version mismatch
    /// is answered as an error *response* (with the request's id
    /// when it can be read), never as a dropped connection — the
    /// client must learn the versions disagree.
    ///
    /// Public so the fuzz target (`fuzz/fuzz_targets/ipc_wire.rs`,
    /// R3-17) can drive the wire format directly; the server's
    /// connection loop calls this same function.
    ///
    /// # Errors
    ///
    /// Returns `Err(IpcError::Protocol)` when the line is not a
    /// parseable request (bad JSON, or a line too long). A version
    /// mismatch, an unknown method, and a daemon API failure are NOT
    /// errors here — they are answered as error *responses*
    /// (`ok: false`) so the client learns what happened.
    pub fn handle_line(&self, line: &str) -> Result<IpcResponse, IpcError> {
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

/// Cheap read methods (DS-28) that run inline on the connection
/// task instead of `spawn_blocking`: pure lock reads, no I/O, no
/// waits. `slow_poll` is deliberately NOT here — it scans the
/// filesystem (`list_games`). `poll` / `connection` read the cached
/// board port (DS-27), so they only ever probe hardware on a 5 s
/// cache miss.
fn is_cheap_method(method: &str) -> bool {
    matches!(
        method,
        "ping" | "version" | "poll" | "snapshot" | "connection"
    )
}

/// The request id carried by a raw line, for error responses to
/// unparseable requests (0 when the line has no readable id).
fn request_id(line: &str) -> u64 {
    serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .and_then(|value| value.get("id").and_then(serde_json::Value::as_u64))
        .unwrap_or(0)
}

/// A `Busy` error response (DS-28): the per-connection dispatch
/// bound is exhausted. The client surfaces this as
/// `IpcError::Api("Busy: …")` and retries or backs off.
fn busy_response_text(line: &str) -> Option<String> {
    encode_response(&IpcResponse {
        v: IPC_PROTOCOL_VERSION,
        id: request_id(line),
        ok: false,
        result: None,
        error: Some(IpcErrorBody {
            code: "Busy".to_string(),
            message: "server busy: too many requests in flight — retry".to_string(),
        }),
    })
    .ok()
}

/// Serve a Unix listener forever (Linux): accept connections and
/// spawn one task per client. Returns `true` only when the accept
/// loop itself broke (too many consecutive accept errors) — the
/// caller (the daemon) must surface that as a fatal, like the bind
/// path; otherwise it never returns.
///
/// DS-36 (B-11): the old loop slept 100 ms on every accept error
/// with no log, no counter, and no exit — a permanently broken
/// listener (fd exhaustion, deleted socket dir) spun at 10 Hz
/// forever with no trace. Now each error is logged (first + every
/// 10th), the sleep ramps 100 ms → 1 s, and after 50 consecutive
/// failures the listener is declared broken.
#[cfg(unix)]
pub async fn serve_listener(server: Arc<IpcServer>, listener: tokio::net::UnixListener) -> bool {
    let mut consecutive_failures = 0u32;
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                consecutive_failures = 0;
                let server = Arc::clone(&server);
                tokio::spawn(async move {
                    server.serve_stream(stream).await;
                });
            }
            Err(error) => {
                consecutive_failures += 1;
                if consecutive_failures == 1 || consecutive_failures.is_multiple_of(10) {
                    eprintln!(
                        "rsh-ipc: accept failed ({consecutive_failures} consecutive): {error}"
                    );
                }
                if consecutive_failures >= 50 {
                    eprintln!("rsh-ipc: accept failed 50 times in a row — listener broken");
                    return true;
                }
                tokio::time::sleep(accept_error_delay(consecutive_failures)).await;
            }
        }
    }
}

/// Backoff between accept retries after consecutive failures
/// (DS-36): 100 ms per failure, capped at 1 s. Pure: unit-tested.
#[cfg(unix)]
fn accept_error_delay(consecutive_failures: u32) -> std::time::Duration {
    std::time::Duration::from_millis(
        (100u64.saturating_mul(u64::from(consecutive_failures.min(10)))).min(1000),
    )
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
        ApiTransport, ButtonBinding, CalibrationInfo, ConnectionError, ConnectionInfo, DiagStats,
        DraftStatus, RawMv, SnapshotInfo, TuningSettings, UserError, VjoyStatus, validate_tuning,
    };
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Mutex;

    /// Minimal stub API for the server tests.
    struct StubApi {
        version: String,
        output: Mutex<bool>,
        tunings: Mutex<BTreeMap<String, TuningSettings>>,
        dirty: Mutex<BTreeSet<String>>,
        bindings: Mutex<BTreeMap<u8, String>>,
        /// Test-only: `list_profiles` sleeps this long (DS-28 — a
        /// controllable blocking handler). Zero in normal tests.
        list_profiles_delay: std::time::Duration,
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
                board_reachable: false,
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
        fn wifi_status(&self) -> rsh_api::WifiStatus {
            rsh_api::WifiStatus {
                ssid: Some("RSH-9A55".to_string()),
                connected: true,
                password_set: false,
                board_mac: Some("AA:BB:CC:DD:EE:FF".to_string()),
            }
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

        // v2.0.1 hotfix BACKEND-2: canned values so the dispatch
        // round-trip tests prove the wire shape (the real daemon
        // overrides these with measured values).
        fn calibration(&self) -> Result<CalibrationInfo, UserError> {
            let mut pedal_min_mv = BTreeMap::new();
            pedal_min_mv.insert("accel".to_string(), 300.0);
            let mut pedal_max_mv = BTreeMap::new();
            pedal_max_mv.insert("accel".to_string(), 3300.0);
            Ok(CalibrationInfo {
                steering_center_mv: 1800.0,
                pedal_min_mv,
                pedal_max_mv,
                calibrated_at: Some("2026-11-16T00:00:00Z".to_string()),
            })
        }

        fn vjoy_status(&self) -> Result<VjoyStatus, UserError> {
            Ok(VjoyStatus {
                installed: true,
                installing: false,
                install_error: None,
                dll_path: Some("C:\\Program Files\\vJoy\\x64\\vJoyInterface.dll".to_string()),
                version: Some("2.2.2".to_string()),
            })
        }

        fn install_vjoy(&self) -> Result<String, UserError> {
            Ok("vJoy install started".to_string())
        }

        fn diag_stats(&self) -> Result<DiagStats, UserError> {
            Ok(DiagStats {
                loop_hz: Some(200.0),
                loss_pct: 0.5,
                jitter_ms_p99: Some(3.4),
                output_latency_ms: None,
                e2e_ms: None,
                seq_gap: 7,
                tx_fail: 3,
                ipc_p50_ms: None,
                ipc_p99_ms: None,
                ipc_per_method_ms: Vec::new(),
            })
        }

        // v2.0.2 capture: canned values so the dispatch round-trip
        // tests prove the wire shape (the real daemon overrides
        // these with the live reading).
        fn raw_mv(&self) -> Result<RawMv, UserError> {
            Ok(RawMv {
                steering_mv: Some(1812),
                accel_mv: Some(320),
                brake_mv: Some(300),
                clutch_mv: None,
                handbrake_mv: Some(250),
                age_ms: Some(5),
            })
        }

        fn capture_centre(&self) -> Result<u16, UserError> {
            Ok(1812)
        }

        fn capture_pedal_min(&self, pedal: &str) -> Result<u16, UserError> {
            match pedal {
                "accel" => Ok(320),
                _ => Err(UserError::from_connection(
                    ConnectionError::Unknown,
                    format!("unknown pedal '{pedal}' (want accel, brake, clutch, or handbrake)"),
                )),
            }
        }

        fn capture_pedal_max(&self, pedal: &str) -> Result<u16, UserError> {
            match pedal {
                "accel" => Ok(3200),
                _ => Err(UserError::from_connection(
                    ConnectionError::Unknown,
                    format!("unknown pedal '{pedal}' (want accel, brake, clutch, or handbrake)"),
                )),
            }
        }

        // v2.0.2 bindings: an in-memory map with the same button
        // range the real store enforces, so the dispatch round-trip
        // tests prove the wire shape and the error path.
        // DS-28 test hook: a controllable blocking handler.
        // `list_profiles` sleeps `list_profiles_delay` (zero in
        // normal tests) so the dispatch-bound test can hold all 8
        // permits at once.
        fn list_profiles(&self) -> Vec<rsh_api::ProfileSummary> {
            if !self.list_profiles_delay.is_zero() {
                std::thread::sleep(self.list_profiles_delay);
            }
            Vec::new()
        }

        fn list_bindings(&self) -> Vec<ButtonBinding> {
            self.bindings
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .map(|(&button, action)| ButtonBinding {
                    button,
                    action: action.clone(),
                })
                .collect()
        }

        fn set_binding(&self, button: u8, action: &str) -> Result<ButtonBinding, UserError> {
            if !((1..=32).contains(&button) || (65..=72).contains(&button)) {
                return Err(UserError {
                    code: "InvalidButton".to_string(),
                    title: "Button cannot be bound".to_string(),
                    what_happened: format!("button {button} cannot be bound"),
                    what_to_do: "Bind a physical button (1-32) or an encoder button (65-72)."
                        .to_string(),
                    detail: Some(format!("button {button} cannot be bound")),
                });
            }
            self.bindings
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(button, action.to_string());
            Ok(ButtonBinding {
                button,
                action: action.to_string(),
            })
        }

        fn clear_binding(&self, button: u8) -> Result<bool, UserError> {
            Ok(self
                .bindings
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&button)
                .is_some())
        }
    }

    fn stub_server() -> Arc<IpcServer> {
        stub_server_with_events().0
    }

    /// The stub server plus its event sender, so tests can flood
    /// the broadcast channel (DS-17: the Lagged path).
    fn stub_server_with_events() -> (Arc<IpcServer>, broadcast::Sender<AppEvent>) {
        let api: Arc<dyn AppApi> = Arc::new(StubApi {
            version: "1.0.2".to_string(),
            output: Mutex::new(false),
            tunings: Mutex::new(BTreeMap::new()),
            dirty: Mutex::new(BTreeSet::new()),
            bindings: Mutex::new(BTreeMap::new()),
            list_profiles_delay: std::time::Duration::ZERO,
        });
        let (tx, _rx) = broadcast::channel(64);
        (Arc::new(IpcServer::new(api, tx.clone())), tx)
    }

    /// R2-30: a lagged burst must still deliver the one-shot
    /// `ConnectionChanged` — the old code kept only the newest
    /// event, so a connection change buried in a lag burst was
    /// silently dropped and the UI waited forever. (No-pairing
    /// redesign: the test used to cover `PairingFinished`; the
    /// one-shot that matters now is `ConnectionChanged`.)
    #[test]
    fn lagged_burst_still_delivers_connection_changed() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        rt.block_on(async {
            let (server, events) = stub_server_with_events();
            let (client_side, server_side) = tokio::io::duplex(65536);
            let serve = tokio::spawn(async move {
                server.serve_stream(server_side).await;
            });
            let (reader, mut writer) = tokio::io::split(client_side);
            let mut lines = BufReader::new(reader).lines();
            writer
                .write_all(b"{\"v\":2,\"id\":1,\"method\":\"subscribe\",\"params\":{}} \n")
                .await
                .expect("write subscribe");
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            // Flood the 64-capacity channel, then the connection
            // change: even if the subscriber lags, the change must
            // be forwarded (not just the newest event).
            for _ in 0..100 {
                let _ = events.send(AppEvent::ConnectionChanged {
                    state: ConnectionState::LinkUp,
                    label: "Link up".to_string(),
                    reason: None,
                });
            }
            let _ = events.send(AppEvent::ConnectionChanged {
                state: ConnectionState::Active,
                label: "Connected".to_string(),
                reason: None,
            });
            let mut delivered = false;
            for _ in 0..300 {
                let line =
                    tokio::time::timeout(std::time::Duration::from_secs(2), lines.next_line())
                        .await
                        .expect("read timeout")
                        .expect("read line")
                        .expect("a line");
                if line.trim().is_empty() {
                    continue;
                }
                let value: serde_json::Value =
                    serde_json::from_str(&line).expect("server writes JSON lines");
                if let Some(event) = value.get("event")
                    && event.get("type") == Some(&serde_json::json!("ConnectionChanged"))
                    && event.get("state") == Some(&serde_json::json!("Active"))
                {
                    delivered = true;
                    break;
                }
            }
            assert!(delivered, "ConnectionChanged must survive a lagged burst");
            assert!(
                !serve.is_finished(),
                "the connection task must still be alive"
            );
            serve.abort();
        });
    }

    #[test]
    fn lagged_broadcast_does_not_drop_the_connection() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        rt.block_on(async {
            let (server, events) = stub_server_with_events();
            let (client_side, server_side) = tokio::io::duplex(65536);
            let serve = tokio::spawn(async move {
                server.serve_stream(server_side).await;
            });
            let (reader, mut writer) = tokio::io::split(client_side);
            let mut lines = BufReader::new(reader).lines();
            // Subscribe first, so the connection forwards events.
            writer
                .write_all(b"{\"v\":2,\"id\":1,\"method\":\"subscribe\",\"params\":{}}\n")
                .await
                .expect("write subscribe");
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            // Flood 100 events into the 64-capacity channel: the
            // subscriber lags — pre-fix the connection died here.
            for _ in 0..100 {
                let _ = events.send(AppEvent::ConnectionChanged {
                    state: ConnectionState::Active,
                    label: "Connected".to_string(),
                    reason: None,
                });
            }
            // The connection must still answer a ping afterwards.
            writer
                .write_all(b"{\"v\":2,\"id\":2,\"method\":\"ping\",\"params\":{}}\n")
                .await
                .expect("write ping");
            let mut answered = false;
            for _ in 0..200 {
                let line =
                    tokio::time::timeout(std::time::Duration::from_secs(2), lines.next_line())
                        .await
                        .expect("read timeout")
                        .expect("read line")
                        .expect("a line");
                if line.trim().is_empty() {
                    continue;
                }
                let value: serde_json::Value =
                    serde_json::from_str(&line).expect("server writes JSON lines");
                if value.get("event").is_some() {
                    continue; // a forwarded event, not the ping reply
                }
                if value.get("id") == Some(&serde_json::json!(2)) {
                    assert_eq!(
                        value.get("ok"),
                        Some(&serde_json::Value::Bool(true)),
                        "ping reply: {value}"
                    );
                    answered = true;
                    break;
                }
            }
            assert!(answered, "ping must be answered after a lag burst");
            assert!(
                !serve.is_finished(),
                "the connection task must still be alive"
            );
            serve.abort();
        });
    }

    /// DS-36: the accept-error backoff ramps 100 ms per failure
    /// and caps at 1 s (a permanently broken listener used to spin
    /// at a fixed 100 ms with no log and no exit).
    #[test]
    #[cfg(unix)]
    fn accept_error_delay_ramps_then_caps() {
        assert_eq!(accept_error_delay(1), std::time::Duration::from_millis(100));
        assert_eq!(accept_error_delay(5), std::time::Duration::from_millis(500));
        assert_eq!(accept_error_delay(10), std::time::Duration::from_secs(1));
        assert_eq!(accept_error_delay(100), std::time::Duration::from_secs(1));
    }

    #[test]
    fn ping_and_version_round_trip_through_dispatch() {
        let server = stub_server();
        let resp = server
            .handle_line(r#"{"v":2,"id":3,"method":"ping","params":{}}"#)
            .expect("ping parses");
        assert!(resp.ok);
        assert_eq!(resp.id, 3);
        assert_eq!(
            resp.result.expect("result").get("pong"),
            Some(&serde_json::Value::Bool(true))
        );
        let resp = server
            .handle_line(r#"{"v":2,"id":4,"method":"version","params":{}}"#)
            .expect("version parses");
        assert_eq!(
            resp.result.expect("result").get("version"),
            Some(&serde_json::Value::String("1.0.2".to_string()))
        );
    }

    /// DS-82: `response_text` records every dispatch's latency —
    /// the snapshot holds the methods it served, with sane
    /// percentiles (the global aggregate is `Some` once anything
    /// was recorded; counts are lower bounds — the snapshot is
    /// process-global, so other tests in this binary add to it).
    #[test]
    fn ds82_response_text_records_dispatch_latency() {
        let server = stub_server();
        assert!(
            server
                .response_text(r#"{"v":2,"id":60,"method":"ping","params":{}}"#)
                .is_some()
        );
        assert!(
            server
                .response_text(r#"{"v":2,"id":61,"method":"poll","params":{}}"#)
                .is_some()
        );
        let snapshot = crate::ipc_latency_snapshot();
        let ping = snapshot
            .iter()
            .find(|(method, _, _, _)| method == "ping")
            .expect("ping recorded");
        assert!(ping.1 >= 1, "ping count: {ping:?}");
        assert!(ping.2 <= ping.3, "p50 <= p99: {ping:?}");
        let poll = snapshot
            .iter()
            .find(|(method, _, _, _)| method == "poll")
            .expect("poll recorded");
        assert!(poll.1 >= 1, "poll count: {poll:?}");
        let (p50, p99) = crate::ipc_latency_global_p50_p99_ms().expect("global aggregate");
        assert!(p50 <= p99, "global p50 {p50} <= p99 {p99}");
        assert!(p50 >= 0.0 && p99 < 500.0, "p50 {p50} p99 {p99} ms");
    }

    #[test]
    fn poll_round_trip_through_dispatch() {
        // L-4 (v2.0.2): one `poll` call carries snapshot +
        // connection + output (the trait default, over the stub).
        // Wire shape is camelCase: `outputEnabled`.
        let server = stub_server();
        let resp = server
            .handle_line(r#"{"v":2,"id":5,"method":"poll","params":{}}"#)
            .expect("poll parses");
        assert!(resp.ok, "resp: {resp:?}");
        let got = resp.result.expect("result");
        let snap = got.get("snapshot").expect("poll has a snapshot");
        assert_eq!(snap.get("seq"), Some(&serde_json::json!(0)));
        assert_eq!(snap.get("throttle"), Some(&serde_json::json!(0.0)));
        let conn = got.get("connection").expect("poll has a connection");
        assert_eq!(
            conn.get("label"),
            Some(&serde_json::Value::String("Connected".to_string()))
        );
        assert_eq!(
            conn.get("session_active"),
            Some(&serde_json::Value::Bool(true))
        );
        assert_eq!(
            got.get("outputEnabled"),
            Some(&serde_json::Value::Bool(false)),
            "the stub's output flag is off: {got}"
        );
        // The same values the three separate calls return — one
        // call, not three round-trips.
        let snap_resp = server
            .handle_line(r#"{"v":2,"id":6,"method":"snapshot","params":{}}"#)
            .expect("snapshot parses");
        assert_eq!(snap_resp.result.expect("result"), *snap);
        let conn_resp = server
            .handle_line(r#"{"v":2,"id":7,"method":"connection","params":{}}"#)
            .expect("connection parses");
        assert_eq!(conn_resp.result.expect("result"), *conn);
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
            .handle_line(r#"{"v":2,"id":2,"method":"explode","params":{}}"#)
            .expect("unknown method is a response");
        assert!(!resp.ok);
        assert_eq!(resp.error.expect("error").code, "UnknownMethod");
    }

    #[test]
    fn tuning_methods_round_trip_through_dispatch() {
        let server = stub_server();
        // get_tuning: neutral default before any edit.
        let resp = server
            .handle_line(r#"{"v":2,"id":10,"method":"get_tuning","params":{"module":"steering"}}"#)
            .expect("get_tuning parses");
        assert!(resp.ok);
        let got = resp.result.expect("result");
        assert_eq!(got.get("module"), Some(&serde_json::json!("steering")));
        assert_eq!(got.get("curve"), Some(&serde_json::json!(1.0)));
        // set_tuning: valid record is stored + echoed.
        let resp = server
            .handle_line(r#"{"v":2,"id":11,"method":"set_tuning","params":{"module":"steering","deadzone":5.0,"curve":1.6,"minMv":0.0,"maxMv":3300.0,"smoothing":10.0}}"#)
            .expect("set_tuning parses");
        assert!(resp.ok, "resp: {resp:?}");
        let got = resp.result.expect("result");
        assert_eq!(got.get("deadzone"), Some(&serde_json::json!(5.0)));
        assert_eq!(got.get("curve"), Some(&serde_json::json!(1.6)));
        // draft_status: dirty after the edit.
        let resp = server
            .handle_line(r#"{"v":2,"id":12,"method":"draft_status","params":{}}"#)
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
            .handle_line(r#"{"v":2,"id":13,"method":"set_tuning","params":{"module":"steering","deadzone":5.0,"curve":9.0,"minMv":0.0,"maxMv":3300.0,"smoothing":10.0}}"#)
            .expect("invalid set_tuning parses");
        assert!(!resp.ok);
        assert_eq!(resp.error.expect("error").code, "Unknown");
        // save_draft: saves, then the draft is clean (null status).
        let resp = server
            .handle_line(r#"{"v":2,"id":14,"method":"save_draft","params":{}}"#)
            .expect("save_draft parses");
        assert!(resp.ok);
        assert_eq!(
            resp.result.expect("result").get("saved"),
            Some(&serde_json::Value::Bool(true))
        );
        let resp = server
            .handle_line(r#"{"v":2,"id":15,"method":"draft_status","params":{}}"#)
            .expect("draft_status parses");
        assert!(resp.ok);
        assert!(resp.result.expect("result").is_null());
        // discard_draft: nothing to discard when clean.
        let resp = server
            .handle_line(r#"{"v":2,"id":16,"method":"discard_draft","params":{}}"#)
            .expect("discard_draft parses");
        assert!(resp.ok);
        assert_eq!(
            resp.result.expect("result").get("discarded"),
            Some(&serde_json::Value::Bool(false))
        );
        // Edit again, then discard reverts to neutral.
        let _ = server
            .handle_line(r#"{"v":2,"id":17,"method":"set_tuning","params":{"module":"steering","deadzone":8.0,"curve":1.6,"minMv":0.0,"maxMv":3300.0,"smoothing":10.0}}"#)
            .expect("set_tuning parses");
        let resp = server
            .handle_line(r#"{"v":2,"id":18,"method":"discard_draft","params":{}}"#)
            .expect("discard_draft parses");
        assert_eq!(
            resp.result.expect("result").get("discarded"),
            Some(&serde_json::Value::Bool(true))
        );
        let resp = server
            .handle_line(r#"{"v":2,"id":19,"method":"get_tuning","params":{"module":"steering"}}"#)
            .expect("get_tuning parses");
        assert_eq!(
            resp.result.expect("result").get("deadzone"),
            Some(&serde_json::json!(0.0))
        );
    }

    #[test]
    fn profiles_games_learn_round_trip_through_dispatch() {
        // The stub uses the trait defaults: empty lists, honest
        // errors, learn never armed, no pick to take.
        let server = stub_server();
        let resp = server
            .handle_line(r#"{"v":2,"id":30,"method":"list_profiles","params":{}}"#)
            .expect("list_profiles parses");
        assert!(resp.ok);
        assert_eq!(
            resp.result.expect("result"),
            serde_json::Value::Array(vec![])
        );
        let resp = server
            .handle_line(r#"{"v":2,"id":31,"method":"activate_profile","params":{"id":"x"}}"#)
            .expect("activate_profile parses");
        assert!(!resp.ok);
        assert_eq!(resp.error.expect("error").code, "Unknown");
        let resp = server
            .handle_line(r#"{"v":2,"id":32,"method":"create_profile","params":{"name":"New"}}"#)
            .expect("create_profile parses");
        assert!(!resp.ok);
        assert_eq!(resp.error.expect("error").code, "Unknown");
        let resp = server
            .handle_line(r#"{"v":2,"id":33,"method":"duplicate_profile","params":{"id":"x"}}"#)
            .expect("duplicate_profile parses");
        assert!(!resp.ok);
        let resp = server
            .handle_line(r#"{"v":2,"id":34,"method":"delete_profile","params":{"id":"x"}}"#)
            .expect("delete_profile parses");
        assert!(!resp.ok);
        let resp = server
            .handle_line(r#"{"v":2,"id":35,"method":"list_games","params":{}}"#)
            .expect("list_games parses");
        assert!(resp.ok);
        assert_eq!(
            resp.result.expect("result"),
            serde_json::Value::Array(vec![])
        );
        let resp = server
            .handle_line(
                r#"{"v":2,"id":36,"method":"launch_game","params":{"id":"assetto_corsa"}}"#,
            )
            .expect("launch_game parses");
        assert!(!resp.ok);
        assert_eq!(resp.error.expect("error").code, "Unknown");
        let resp = server
            .handle_line(r#"{"v":2,"id":37,"method":"stop_game","params":{"id":"assetto_corsa"}}"#)
            .expect("stop_game parses");
        assert!(!resp.ok);
        let resp = server
            .handle_line(r#"{"v":2,"id":38,"method":"game_state","params":{"id":"assetto_corsa"}}"#)
            .expect("game_state parses");
        assert!(!resp.ok);
        let resp = server
            .handle_line(r#"{"v":2,"id":39,"method":"start_learn","params":{}}"#)
            .expect("start_learn parses");
        assert!(!resp.ok);
        assert_eq!(resp.error.expect("error").code, "Unknown");
        let resp = server
            .handle_line(r#"{"v":2,"id":40,"method":"cancel_learn","params":{}}"#)
            .expect("cancel_learn parses");
        assert!(!resp.ok);
        let resp = server
            .handle_line(r#"{"v":2,"id":41,"method":"is_learning","params":{}}"#)
            .expect("is_learning parses");
        assert!(resp.ok);
        assert_eq!(
            resp.result.expect("result").get("learning"),
            Some(&serde_json::Value::Bool(false))
        );
        let resp = server
            .handle_line(r#"{"v":2,"id":42,"method":"take_learned_button","params":{}}"#)
            .expect("take_learned_button parses");
        assert!(resp.ok);
        assert_eq!(
            resp.result.expect("result").get("button"),
            Some(&serde_json::Value::Null)
        );
    }

    #[test]
    fn settings_methods_round_trip_through_dispatch() {
        // The stub uses the trait defaults: autostart off,
        // firmware/backend unknown, set/backup unsupported.
        let server = stub_server();
        let resp = server
            .handle_line(r#"{"v":2,"id":20,"method":"autostart","params":{}}"#)
            .expect("autostart parses");
        assert!(resp.ok);
        assert_eq!(
            resp.result.expect("result").get("autostart"),
            Some(&serde_json::Value::Bool(false))
        );
        let resp = server
            .handle_line(r#"{"v":2,"id":21,"method":"set_autostart","params":{"enabled":true}}"#)
            .expect("set_autostart parses");
        assert!(!resp.ok);
        assert_eq!(resp.error.expect("error").code, "Unknown");
        let resp = server
            .handle_line(r#"{"v":2,"id":22,"method":"firmware_version","params":{}}"#)
            .expect("firmware_version parses");
        assert!(resp.ok);
        assert_eq!(
            resp.result.expect("result").get("firmware_version"),
            Some(&serde_json::Value::String("Unknown".to_string()))
        );
        let resp = server
            .handle_line(r#"{"v":2,"id":23,"method":"output_backend","params":{}}"#)
            .expect("output_backend parses");
        assert!(resp.ok);
        assert_eq!(
            resp.result.expect("result").get("output_backend"),
            Some(&serde_json::Value::String("Unknown".to_string()))
        );
        let resp = server
            .handle_line(
                r#"{"v":2,"id":24,"method":"set_output_backend","params":{"name":"vJoy"}}"#,
            )
            .expect("set_output_backend parses");
        assert!(!resp.ok);
        assert_eq!(resp.error.expect("error").code, "Unknown");
        let resp = server
            .handle_line(r#"{"v":2,"id":25,"method":"backup_now","params":{}}"#)
            .expect("backup_now parses");
        assert!(!resp.ok);
        assert_eq!(resp.error.expect("error").code, "Unknown");
    }

    #[test]
    fn calibration_vjoy_diag_round_trip_through_dispatch() {
        let server = stub_server();
        // calibration: wire shape is camelCase, values round-trip.
        let resp = server
            .handle_line(r#"{"v":2,"id":30,"method":"calibration","params":{}}"#)
            .expect("calibration parses");
        assert!(resp.ok, "resp: {resp:?}");
        let got = resp.result.expect("result");
        assert_eq!(
            got.get("steeringCenterMv"),
            Some(&serde_json::json!(1800.0))
        );
        assert_eq!(
            got.get("calibratedAt"),
            Some(&serde_json::json!("2026-11-16T00:00:00Z"))
        );
        // vjoy_status: installed + DLL path round-trip.
        let resp = server
            .handle_line(r#"{"v":2,"id":31,"method":"vjoy_status","params":{}}"#)
            .expect("vjoy_status parses");
        assert!(resp.ok, "resp: {resp:?}");
        let got = resp.result.expect("result");
        assert_eq!(got.get("installed"), Some(&serde_json::Value::Bool(true)));
        assert_eq!(
            got.get("dllPath"),
            Some(&serde_json::json!(
                "C:\\Program Files\\vJoy\\x64\\vJoyInterface.dll"
            ))
        );
        // install_vjoy: starts, returns the message.
        let resp = server
            .handle_line(r#"{"v":2,"id":32,"method":"install_vjoy","params":{}}"#)
            .expect("install_vjoy parses");
        assert!(resp.ok, "resp: {resp:?}");
        assert_eq!(
            resp.result.expect("result").get("message"),
            Some(&serde_json::Value::String(
                "vJoy install started".to_string()
            ))
        );
        // diag_stats: measured values round-trip.
        let resp = server
            .handle_line(r#"{"v":2,"id":33,"method":"diag_stats","params":{}}"#)
            .expect("diag_stats parses");
        assert!(resp.ok, "resp: {resp:?}");
        let got = resp.result.expect("result");
        assert_eq!(got.get("loopHz"), Some(&serde_json::json!(200.0)));
        assert_eq!(got.get("lossPct"), Some(&serde_json::json!(0.5)));
        assert_eq!(got.get("seqGap"), Some(&serde_json::json!(7)));
        assert_eq!(got.get("txFail"), Some(&serde_json::json!(3)));
    }

    #[test]
    fn capture_methods_round_trip_through_dispatch() {
        let server = stub_server();
        // raw_mv: camelCase wire shape, absent slots omitted.
        let resp = server
            .handle_line(r#"{"v":2,"id":40,"method":"raw_mv","params":{}}"#)
            .expect("raw_mv parses");
        assert!(resp.ok, "resp: {resp:?}");
        let got = resp.result.expect("result");
        assert_eq!(got.get("steeringMv"), Some(&serde_json::json!(1812)));
        assert_eq!(got.get("accelMv"), Some(&serde_json::json!(320)));
        assert_eq!(got.get("clutchMv"), None, "absent slot is omitted");
        assert_eq!(got.get("ageMs"), Some(&serde_json::json!(5)));
        // capture_centre: the captured mV comes back.
        let resp = server
            .handle_line(r#"{"v":2,"id":41,"method":"capture_centre","params":{}}"#)
            .expect("capture_centre parses");
        assert!(resp.ok, "resp: {resp:?}");
        assert_eq!(
            resp.result.expect("result").get("mv"),
            Some(&serde_json::json!(1812))
        );
        // capture_pedal_min/max: pedal param in, captured mV out.
        let resp = server
            .handle_line(
                r#"{"v":2,"id":42,"method":"capture_pedal_min","params":{"pedal":"accel"}}"#,
            )
            .expect("capture_pedal_min parses");
        assert!(resp.ok, "resp: {resp:?}");
        assert_eq!(
            resp.result.expect("result").get("mv"),
            Some(&serde_json::json!(320))
        );
        let resp = server
            .handle_line(
                r#"{"v":2,"id":43,"method":"capture_pedal_max","params":{"pedal":"accel"}}"#,
            )
            .expect("capture_pedal_max parses");
        assert!(resp.ok, "resp: {resp:?}");
        assert_eq!(
            resp.result.expect("result").get("mv"),
            Some(&serde_json::json!(3200))
        );
        // An unknown pedal fails — and the error keeps the reason
        // (`code|detail`), not a bare "Unknown".
        let resp = server
            .handle_line(
                r#"{"v":2,"id":44,"method":"capture_pedal_min","params":{"pedal":"gear"}}"#,
            )
            .expect("capture_pedal_min parses");
        assert!(!resp.ok, "resp: {resp:?}");
        let error = resp.error.expect("error body");
        assert!(
            error.code.contains("unknown pedal 'gear'"),
            "error code must carry the reason: {}",
            error.code
        );
        assert!(error.message.contains("unknown pedal 'gear'"));
    }

    #[test]
    fn binding_methods_round_trip_through_dispatch() {
        let server = stub_server();
        // list_bindings starts empty.
        let resp = server
            .handle_line(r#"{"v":2,"id":50,"method":"list_bindings","params":{}}"#)
            .expect("list_bindings parses");
        assert!(resp.ok, "resp: {resp:?}");
        assert_eq!(
            resp.result.expect("result").get("bindings"),
            Some(&serde_json::json!([]))
        );
        // set_binding: button + action in, the stored binding out.
        let resp = server
            .handle_line(
                r#"{"v":2,"id":51,"method":"set_binding","params":{"button":5,"action":"horn"}}"#,
            )
            .expect("set_binding parses");
        assert!(resp.ok, "resp: {resp:?}");
        assert_eq!(
            resp.result.expect("result").get("binding"),
            Some(&serde_json::json!({"button": 5, "action": "horn"}))
        );
        // list_bindings now shows it (camelCase wire shape).
        let resp = server
            .handle_line(r#"{"v":2,"id":52,"method":"list_bindings","params":{}}"#)
            .expect("list_bindings parses");
        assert!(resp.ok, "resp: {resp:?}");
        assert_eq!(
            resp.result.expect("result").get("bindings"),
            Some(&serde_json::json!([{"button": 5, "action": "horn"}]))
        );
        // clear_binding: true when bound, false when absent.
        let resp = server
            .handle_line(r#"{"v":2,"id":53,"method":"clear_binding","params":{"button":5}}"#)
            .expect("clear_binding parses");
        assert!(resp.ok, "resp: {resp:?}");
        assert_eq!(
            resp.result.expect("result").get("cleared"),
            Some(&serde_json::json!(true))
        );
        let resp = server
            .handle_line(r#"{"v":2,"id":54,"method":"clear_binding","params":{"button":5}}"#)
            .expect("clear_binding parses");
        assert!(resp.ok, "resp: {resp:?}");
        assert_eq!(
            resp.result.expect("result").get("cleared"),
            Some(&serde_json::json!(false))
        );
        // An out-of-range button fails — and the error keeps the
        // reason (`code|detail`), not a bare "Unknown".
        let resp = server
            .handle_line(
                r#"{"v":2,"id":55,"method":"set_binding","params":{"button":33,"action":"horn"}}"#,
            )
            .expect("set_binding parses");
        assert!(!resp.ok, "resp: {resp:?}");
        let error = resp.error.expect("error body");
        assert!(
            error.code.contains("InvalidButton"),
            "error code must carry the reason: {}",
            error.code
        );
        assert!(error.message.contains("button 33 cannot be bound"));
        // A missing button param fails the same way, never silently
        // binds button 0.
        let resp = server
            .handle_line(r#"{"v":2,"id":56,"method":"set_binding","params":{"action":"horn"}}"#)
            .expect("set_binding parses");
        assert!(!resp.ok, "resp: {resp:?}");
        assert!(
            resp.error
                .expect("error body")
                .code
                .contains("InvalidButton"),
            "missing button must fail as InvalidButton"
        );
    }

    /// DS-28: 12 concurrent blocking dispatches on one connection —
    /// the first 8 run, the rest answer `Busy`; and a cheap `poll`
    /// sent while the pool is full still answers inline at once.
    #[tokio::test]
    #[cfg(unix)]
    async fn ds28_blocking_dispatch_bound_answers_busy_and_cheap_stays_inline() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        use tokio::net::UnixStream;

        let dir = std::env::temp_dir().join(format!("rsh-ipc-ds28-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("test dir");
        let socket = dir.join("rsh-daemon.sock");
        // `list_profiles` blocks 1 s per call on this server, so 12
        // concurrent calls hold all 8 permits at once.
        let api: Arc<dyn AppApi> = Arc::new(StubApi {
            version: "1.0.2".to_string(),
            output: Mutex::new(false),
            tunings: Mutex::new(BTreeMap::new()),
            dirty: Mutex::new(BTreeSet::new()),
            bindings: Mutex::new(BTreeMap::new()),
            list_profiles_delay: std::time::Duration::from_secs(1),
        });
        let (tx, _rx) = broadcast::channel(64);
        let server = Arc::new(IpcServer::new(api, tx));
        let listener = bind_unix_socket(&socket).expect("bind test socket");
        let serve = tokio::spawn(serve_listener(Arc::clone(&server), listener));

        let stream = UnixStream::connect(&socket).await.expect("connect");
        let (reader, mut writer) = tokio::io::split(stream);
        let mut lines = tokio::io::BufReader::new(reader).lines();
        // Fire 12 blocking calls without waiting for any reply.
        for id in 1..=12_u64 {
            let req = serde_json::json!({
                "v": IPC_PROTOCOL_VERSION,
                "id": id,
                "method": "list_profiles",
                "params": {}
            });
            writer
                .write_all(format!("{req}\n").as_bytes())
                .await
                .expect("write request");
        }
        // A cheap poll while the pool is full: inline, no permit.
        let poll_req = serde_json::json!({
            "v": IPC_PROTOCOL_VERSION,
            "id": 13,
            "method": "poll",
            "params": {}
        });
        writer
            .write_all(format!("{poll_req}\n").as_bytes())
            .await
            .expect("write poll");
        let mut ok = 0;
        let mut busy = 0;
        let mut poll_ok = false;
        let started = std::time::Instant::now();
        for _ in 0..13 {
            let line = tokio::time::timeout(std::time::Duration::from_secs(5), lines.next_line())
                .await
                .expect("response arrives in time")
                .expect("line read")
                .expect("a line");
            let value: serde_json::Value = serde_json::from_str(&line).expect("response parses");
            let id = value
                .get("id")
                .and_then(serde_json::Value::as_u64)
                .expect("id");
            if id == 13 {
                assert!(
                    value
                        .get("ok")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false),
                    "poll answers while pool is full: {line}"
                );
                poll_ok = true;
                // The inline poll must not have waited out the 1 s
                // blocking calls.
                assert!(
                    started.elapsed() < std::time::Duration::from_millis(900),
                    "poll waited for blocking handlers: {:?}",
                    started.elapsed()
                );
            } else if value
                .get("ok")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
            {
                ok += 1;
            } else {
                let code = value
                    .get("error")
                    .and_then(|e| e.get("code"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                assert_eq!(code, "Busy", "non-ok reply must be Busy: {line}");
                busy += 1;
            }
        }
        assert_eq!(ok, 8, "first 8 blocking calls run");
        assert_eq!(busy, 4, "the rest answer Busy");
        assert!(poll_ok, "poll answered");
        serve.abort();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
