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

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, mpsc};

use rsh_api::AppEvent;

use crate::{IPC_PROTOCOL_VERSION, IpcError, IpcRequest, IpcResponse};

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
                if line.contains("\"event\"") {
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
    /// # Errors
    ///
    /// [`IpcError`] on transport failure, version mismatch, or a
    /// daemon-side error response (with the daemon's code).
    pub async fn call(
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
        drop(writer);
        let mut rx = self.responses.lock().await;
        let resp = rx.recv().await.ok_or(IpcError::Closed)?;
        if resp.v != IPC_PROTOCOL_VERSION {
            return Err(IpcError::UnsupportedVersion(resp.v));
        }
        if resp.id != id {
            return Err(IpcError::Protocol(format!(
                "response id {} does not match request id {id}",
                resp.id
            )));
        }
        if resp.ok {
            Ok(resp.result.unwrap_or(serde_json::Value::Null))
        } else {
            let body = resp.error.unwrap_or(crate::IpcErrorBody {
                code: "Protocol".to_string(),
                message: "daemon returned an error without a body".to_string(),
            });
            Err(IpcError::Api(format!("{}: {}", body.code, body.message)))
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
