//! IPC client library (used by the Slint UI in Phase 3).
//!
//! `IpcClient` connects to the daemon's Unix socket (Linux) or
//! named pipe (Windows), sends one JSON-lines request at a time,
//! and reads the matching response by `id`. `subscribe()` marks
//! the connection as an event subscriber; pushed
//! [`AppEvent`](rsh_api::AppEvent) frames are delivered on a
//! channel the caller drains.
//!
//! The client is deliberately single-request-at-a-time (one
//! in-flight call): the UI's calls are user-driven and rare, and a
//! mutex around the stream keeps response matching trivial and
//! correct.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, mpsc};

use rsh_api::AppEvent;

use crate::{IPC_PROTOCOL_VERSION, IpcError, IpcRequest, IpcResponse};

/// Bound for one IPC round-trip (B3): a daemon that stops
/// answering must never hang the caller — the UI's frame loop and
/// the health monitor both depend on every call returning within
/// this window, even when the daemon process is wedged but its
/// socket is still open.
const CALL_TIMEOUT: Duration = Duration::from_millis(500);

/// Methods whose result arrives via events/polling, not the RPC
/// reply (B-1, v2.0.2): `begin_pairing` only *starts* a pairing
/// window — the outcome arrives as `PairingWindow` /
/// `PairingFinished` events and via `pairing_status`. Waiting for
/// the RPC reply would time out against a slow board (the daemon
/// used to wait up to 3 s for the window) and the timeout would
/// poison the poller's link state, so [`IpcClient::call`] returns
/// as soon as the request is written for these methods. The late
/// reply is discarded by the next call's id check (the response
/// channel holds 64 frames, so it never blocks the reader).
const FIRE_AND_FORGET_METHODS: &[&str] = &["begin_pairing"];

/// A connected IPC client.
pub struct IpcClient {
    /// Line writer (guarded: one in-flight call at a time, and the
    /// response wait happens under the same guard). Boxed so the
    /// same code serves the Unix socket (Linux) and the named pipe
    /// (Windows) — both are `AsyncRead + AsyncWrite`.
    writer: Mutex<Box<dyn tokio::io::AsyncWrite + Unpin + Send>>,
    /// Response receiver, guarded by the same mutex as the writer
    /// (one in-flight call, so one waiter).
    responses: Mutex<mpsc::Receiver<IpcResponse>>,
    /// Next request id.
    next_id: Mutex<u64>,
    /// Pushed events, drained by the caller.
    events: Mutex<mpsc::Receiver<AppEvent>>,
    /// The reader task's handle (kept alive for the client's life).
    _reader: tokio::task::JoinHandle<()>,
}

impl IpcClient {
    /// Build a client over any connected byte stream (Unix socket
    /// or named pipe). Shared by the per-platform `connect_*`
    /// constructors and the round-trip test.
    #[must_use]
    pub fn from_stream<S>(stream: S) -> Arc<Self>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let (reader, writer) = tokio::io::split(stream);
        let (event_tx, event_rx) = mpsc::channel(64);
        let (resp_tx, resp_rx) = mpsc::channel::<IpcResponse>(64);
        // Reader task: demuxes response frames from event pushes.
        // Responses carry `id`; event frames carry `event`.
        let reader_task = tokio::spawn(async move {
            let mut lines = BufReader::new(reader).lines();
            loop {
                let Ok(Some(line)) = lines.next_line().await else {
                    break;
                };
                if line.trim().is_empty() {
                    continue;
                }
                // DS-35 (B-09): classify by STRUCTURE, not
                // substring — the old `line.contains("\"event\"")`
                // misclassified any response whose JSON contained
                // the literal substring `"event"` (e.g. a string
                // value holding it, escaped as `\"event\"`),
                // parsed it as an event frame, failed, and silently
                // dropped the line — the caller then saw only the
                // 500 ms call timeout. A top-level `"event"` key
                // marks an event frame; anything else is a
                // response (an unparseable line is dropped either
                // way, as before).
                let is_event = serde_json::from_str::<serde_json::Value>(&line)
                    .map_or(false, |value| value.get("event").is_some());
                if is_event {
                    if let Ok(frame) = serde_json::from_str::<crate::IpcEventFrame>(&line)
                        && frame.v == IPC_PROTOCOL_VERSION
                        && event_tx.send(frame.event).await.is_err()
                    {
                        break;
                    }
                } else if let Ok(resp) = serde_json::from_str::<IpcResponse>(&line)
                    && resp_tx.send(resp).await.is_err()
                {
                    break;
                }
            }
        });
        Arc::new(Self {
            writer: Mutex::new(Box::new(writer)),
            responses: Mutex::new(resp_rx),
            next_id: Mutex::new(1),
            events: Mutex::new(event_rx),
            _reader: reader_task,
        })
    }

    /// Connect to the daemon: Unix socket at `path` on Linux,
    /// named pipe on Windows (`path` is ignored there — the pipe
    /// name is fixed, [`crate::IPC_PIPE_NAME`]).
    ///
    /// # Errors
    ///
    /// [`IpcError::Io`] when the daemon cannot be reached (daemon
    /// not running).
    pub async fn connect(path: &Path) -> Result<Arc<Self>, IpcError> {
        #[cfg(unix)]
        {
            Self::connect_unix(path).await
        }
        #[cfg(windows)]
        {
            let _ = path;
            Self::connect_named_pipe().await
        }
    }

    /// Connect to the daemon's Unix socket at `path` (Linux).
    ///
    /// # Errors
    ///
    /// [`IpcError::Io`] when the socket cannot be reached (daemon
    /// not running).
    #[cfg(unix)]
    pub async fn connect_unix(path: &Path) -> Result<Arc<Self>, IpcError> {
        let stream = tokio::net::UnixStream::connect(path).await.map_err(|e| {
            IpcError::Io(format!(
                "cannot connect to daemon IPC {}: {e}",
                path.display()
            ))
        })?;
        Ok(Self::from_stream(stream))
    }

    /// Connect to the daemon's named pipe (Windows). The pipe open
    /// itself is synchronous in Tokio's API, so this returns a
    /// ready future instead of an `async fn` (clippy's
    /// `unused_async` fires on an `async fn` with no `.await`).
    ///
    /// # Errors
    ///
    /// [`IpcError::Io`] when the pipe cannot be reached (daemon
    /// not running).
    #[cfg(windows)]
    pub fn connect_named_pipe() -> impl std::future::Future<Output = Result<Arc<Self>, IpcError>> {
        use tokio::net::windows::named_pipe::ClientOptions;
        let result = ClientOptions::new()
            .open(crate::IPC_PIPE_NAME)
            .map_err(|e| IpcError::Io(format!("cannot connect to daemon IPC pipe: {e}")))
            .map(Self::from_stream);
        std::future::ready(result)
    }

    /// Build a client over an already-connected Unix stream (also
    /// the seam the round-trip test uses).
    #[cfg(unix)]
    #[must_use]
    pub fn from_unix_stream(stream: tokio::net::UnixStream) -> Arc<Self> {
        Self::from_stream(stream)
    }

    /// Receive the next pushed event, if one is waiting.
    pub async fn try_recv_event(&self) -> Option<AppEvent> {
        self.events.lock().await.try_recv().ok()
    }

    /// Call one method and await its response.
    ///
    /// The whole round-trip is bounded by [`CALL_TIMEOUT`] (B3): a
    /// daemon that never answers surfaces as [`IpcError::Io`]
    /// ("timed out") instead of hanging the caller forever. The
    /// call is strictly single-flight — the writer lock is held
    /// across the write + response wait, so no second call can
    /// interleave and steal the response.
    ///
    /// # Errors
    ///
    /// [`IpcError`] on transport failure, timeout, version
    /// mismatch, or a daemon-side error response (with the
    /// daemon's code).
    pub async fn call(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, IpcError> {
        if FIRE_AND_FORGET_METHODS.contains(&method) {
            return self.call_send_only(method, params).await;
        }
        self.call_with_timeout(method, params, CALL_TIMEOUT).await
    }

    /// Send one request and return as soon as it is written, without
    /// waiting for the reply (B-1 fire-and-forget start). Only for
    /// methods whose result arrives via events/polling (see
    /// [`FIRE_AND_FORGET_METHODS`]); the reply, when it arrives, is
    /// discarded by the next call's id check. A failed write still
    /// errors, so a dead daemon surfaces immediately.
    ///
    /// # Errors
    ///
    /// [`IpcError`] when the request cannot be serialized or
    /// written.
    async fn call_send_only(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, IpcError> {
        let id = {
            let mut next = self.next_id.lock().await;
            let id = *next;
            *next += 1;
            id
        };
        let request = IpcRequest {
            v: IPC_PROTOCOL_VERSION,
            id,
            method: method.to_string(),
            params,
        };
        let line =
            serde_json::to_string(&request).map_err(|e| IpcError::Protocol(e.to_string()))?;
        let mut writer = self.writer.lock().await;
        writer
            .write_all(line.as_bytes())
            .await
            .map_err(|e| IpcError::Io(e.to_string()))?;
        writer
            .write_all(b"\n")
            .await
            .map_err(|e| IpcError::Io(e.to_string()))?;
        writer
            .flush()
            .await
            .map_err(|e| IpcError::Io(e.to_string()))?;
        Ok(serde_json::Value::Null)
    }

    /// [`call`](Self::call) with an explicit bound (B-2, v2.0.2):
    /// calls whose daemon side legitimately waits on a human —
    /// `set_output_backend` (the UAC prompt + a 60 s sidecar spawn)
    /// — use a longer bound instead of the 500 ms default, so the
    /// UI does not time out while the user is still answering the
    /// prompt. (`begin_pairing` does not use this path: it is
    /// fire-and-forget — see [`FIRE_AND_FORGET_METHODS`].)
    ///
    /// # Errors
    ///
    /// See [`call`](Self::call).
    pub async fn call_with_timeout(
        &self,
        method: &str,
        params: serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, IpcError> {
        let id = {
            let mut next = self.next_id.lock().await;
            let id = *next;
            *next += 1;
            id
        };
        let request = IpcRequest {
            v: IPC_PROTOCOL_VERSION,
            id,
            method: method.to_string(),
            params,
        };
        let line =
            serde_json::to_string(&request).map_err(|e| IpcError::Protocol(e.to_string()))?;
        let deadline = tokio::time::Instant::now() + timeout;
        match tokio::time::timeout_at(deadline, self.call_once(id, line)).await {
            Ok(result) => result,
            Err(_) => Err(IpcError::Io(format!(
                "IPC call '{method}' timed out after {} ms",
                timeout.as_millis()
            ))),
        }
    }

    /// One round-trip without the timeout: write the line, then
    /// wait for the response with this `id`. A reply with another
    /// id is a late answer to a call that already timed out — it
    /// is discarded, never mistaken for this call's reply (ids are
    /// unique per call and responses arrive in request order).
    async fn call_once(&self, id: u64, line: String) -> Result<serde_json::Value, IpcError> {
        // Hold the writer lock across the write + response wait so
        // no second call can interleave and steal the response.
        let mut writer = self.writer.lock().await;
        writer
            .write_all(line.as_bytes())
            .await
            .map_err(|e| IpcError::Io(e.to_string()))?;
        writer
            .write_all(b"\n")
            .await
            .map_err(|e| IpcError::Io(e.to_string()))?;
        writer
            .flush()
            .await
            .map_err(|e| IpcError::Io(e.to_string()))?;
        let mut rx = self.responses.lock().await;
        loop {
            let resp = rx.recv().await.ok_or(IpcError::Closed)?;
            if resp.id != id {
                continue;
            }
            if resp.v != IPC_PROTOCOL_VERSION {
                return Err(IpcError::UnsupportedVersion(resp.v));
            }
            if resp.ok {
                return Ok(resp.result.unwrap_or(serde_json::Value::Null));
            }
            let body = resp.error.unwrap_or(crate::IpcErrorBody {
                code: "Protocol".to_string(),
                message: "daemon returned an error without a body".to_string(),
            });
            return Err(IpcError::Api(format!("{}: {}", body.code, body.message)));
        }
    }

    /// Convenience: `ping` → true.
    ///
    /// # Errors
    ///
    /// See [`IpcClient::call`].
    pub async fn ping(&self) -> Result<bool, IpcError> {
        let result = self.call("ping", serde_json::json!({})).await?;
        Ok(result
            .get("pong")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false))
    }

    /// Convenience: daemon version string.
    ///
    /// # Errors
    ///
    /// See [`IpcClient::call`].
    pub async fn version(&self) -> Result<String, IpcError> {
        let result = self.call("version", serde_json::json!({})).await?;
        result
            .get("version")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| IpcError::Protocol("version response has no version".to_string()))
    }
}

#[cfg(test)]
mod tests {
    // Unix-only: the module's only test binds a real Unix socket.
    #[cfg(unix)]
    use super::*;
    #[cfg(unix)]
    use std::time::Instant;

    /// DS-35: a response whose JSON contains the substring
    /// `"event"` (here, inside a string value) must still be
    /// delivered as a response — the old substring check parsed it
    /// as an event frame, failed, and silently dropped it, so the
    /// call timed out.
    #[test]
    #[cfg(unix)]
    fn response_containing_event_substring_is_not_dropped() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        rt.block_on(async {
            let (client_side, server_side) = tokio::io::duplex(8192);
            let client = IpcClient::from_stream(client_side);
            // Fake server: read one request line, answer it with a
            // result whose string value contains `"event"`.
            tokio::spawn(async move {
                let (reader, mut writer) = tokio::io::split(server_side);
                let mut lines = BufReader::new(reader).lines();
                if let Ok(Some(line)) = lines.next_line().await {
                    let id = serde_json::from_str::<serde_json::Value>(&line)
                        .ok()
                        .and_then(|value| value.get("id").and_then(serde_json::Value::as_u64))
                        .unwrap_or(0);
                    let response = serde_json::json!({
                        "v": IPC_PROTOCOL_VERSION,
                        "id": id,
                        "ok": true,
                        "result": { "version": "1.0.2 \"event\" inside a string" },
                        "error": null,
                    });
                    let text = serde_json::to_string(&response).expect("encode");
                    let _ = writer.write_all(text.as_bytes()).await;
                    let _ = writer.write_all(b"\n").await;
                    let _ = writer.flush().await;
                }
            });
            let result = client
                .call("version", serde_json::json!({}))
                .await
                .expect("the response must be delivered, not dropped as an event");
            assert_eq!(
                result.get("version"),
                Some(&serde_json::Value::String(
                    "1.0.2 \"event\" inside a string".to_string()
                )),
                "result: {result}"
            );
        });
    }

    /// B3: a peer that accepts the socket but never answers must
    /// not hang the call — it times out within ~500 ms.
    ///
    /// Plain `#[test]` (not `#[tokio::test]`): this crate's tokio
    /// has no `macros` feature, so the test builds its own
    /// current-thread runtime.
    #[test]
    #[cfg(unix)]
    fn call_times_out_when_daemon_never_answers() {
        let dir = std::env::temp_dir().join(format!("rsh-ipc-timeout-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("rsh-daemon.sock");
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        rt.block_on(async {
            let listener = tokio::net::UnixListener::bind(&path).expect("bind");
            // Accept and hold the stream without ever answering.
            tokio::spawn(async move {
                if let Ok((stream, _)) = listener.accept().await {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    drop(stream);
                }
            });
            let client = IpcClient::connect(&path).await.expect("connect");
            let start = Instant::now();
            let err = client
                .call("ping", serde_json::json!({}))
                .await
                .expect_err("a silent daemon must time out");
            let elapsed = start.elapsed();
            assert!(
                elapsed < Duration::from_secs(5),
                "timeout must bound the call, took {elapsed:?}"
            );
            assert!(
                matches!(&err, IpcError::Io(message) if message.contains("timed out")),
                "timeout surfaces as an Io error: {err}"
            );
        });
        let _ = std::fs::remove_dir_all(&dir);
    }
}
