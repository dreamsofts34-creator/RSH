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

/// R3-08: `.lines()` survives only in the Unix tests below (fake
/// peers) — the production reader uses `read_bounded_line` — so
/// this import is test-only AND Unix-only.
#[cfg(all(test, unix))]
use tokio::io::AsyncBufReadExt;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, mpsc};

use rsh_api::AppEvent;

use crate::{IPC_PROTOCOL_VERSION, IpcError, IpcRequest, IpcResponse};

/// Bound for one IPC round-trip (B3): a daemon that stops
/// answering must never hang the caller — the UI's frame loop and
/// the health monitor both depend on every call returning within
/// this window, even when the daemon process is wedged but its
/// socket is still open.
const CALL_TIMEOUT: Duration = Duration::from_millis(500);

/// Per-method round-trip bound (P2-4, v2.0.11): the 500 ms default
/// is right for cheap reads, but several methods legitimately take
/// seconds on the daemon side — a plain `call()` to one of them
/// used to time out misleadingly (read as link degradation) while
/// the daemon was still working. Callers of these methods get the
/// right bound by default; [`IpcClient::call_with_timeout`] still
/// overrides when the caller knows better. Bounds are grounded in
/// the UI's own explicit bounds where they exist (`set_output_backend`
/// 390 s, game writes 10 s) and in the daemon-side work otherwise
/// (a board control round-trip, a file copy, a driver download).
#[must_use]
pub fn timeout_for_method(method: &str) -> Duration {
    match method {
        // UAC prompt + 60 s sidecar spawn (B-2, v2.0.2) + the
        // 310 s post-UAC switch budget (HID path sweep, v2.0.12):
        // the UI's own long-call bound (390 s), kept here so a
        // plain call agrees. The old 75 s arm was stale (P3-1,
        // v2.0.12 scan) — a healthy 310 s switch timed out at
        // 75 s while the daemon kept going. `set_output_enabled`
        // rides the same arm (CORE-P1-2, polish round): it can run
        // the full backend switch inline, so the UI no longer
        // times out at 500 ms while the daemon keeps switching.
        // `set_g29_spoof_mode` rides it too (Bug 1, v2.0.17): it
        // recreates the HID backend (sidecar respawn + UAC dwell),
        // which the daemon says "can take as long as a backend
        // switch" — at 500 ms the UI timed out, errored, and
        // flipped the toggle back while the daemon kept working.
        "set_output_backend" | "set_output_enabled" | "set_g29_spoof_mode" => {
            Duration::from_secs(390)
        }
        // Game writes: atomic file write + process refresh, which
        // can pass 500 ms on Windows under an AV scan (the UI's
        // `GAME_WRITE_TIMEOUT`). Board control round-trip (LED test
        // on `set_tuning`, config transaction, firmware
        // stat/parse) — same 10 s bound, one arm. `update_firmware`
        // (P2-1, v2.0.12 scan) rides here too: its daemon
        // pre-flight (keystore read, bundled-image stat + version
        // parse, validation) runs synchronously before the flash
        // thread spawns, so the 500 ms default timed out a flash
        // that had already started.
        "add_game"
        | "update_game"
        | "remove_game"
        | "set_tuning"
        | "set_shifter_sensitivity"
        | "firmware_check"
        | "update_firmware"
        // CORE-P3-1 (polish round): launch/stop/state are slow
        // methods too (process spawn under AV, TerminateProcess +
        // wait) — a plain call() used to get 500 ms.
        | "launch_game"
        | "stop_game"
        | "game_state"
        // v2.0.18 scan: `connect` / `disconnect` are session
        // control, not cheap reads. `connect` can block up to ~5 s
        // in `start_engine` (supervisor spawn + ready wait, and the
        // re-await path when a previous spawn reported ready late),
        // and `disconnect` runs an OS serial-port rescan
        // (`rescan_board_port`) with no bound in code — both used
        // to time out at 500 ms on the UI's plain `call_async`
        // while the daemon kept connecting, so the UI reported a
        // link failure for a connect that was actually proceeding.
        // 10 s covers the 5 s ready wait with headroom.
        | "connect"
        | "disconnect" => Duration::from_secs(10),
        // Wi-Fi password set / rejoin (P1-3, v2.0.12 scan): the
        // daemon worst case is ≈150 s (netsh add ≤30 s + connect
        // ≤30 s + confirm ≤90 s, plus board round-trips and a
        // possible rollback), so the bound is 180 s — a client
        // timeout mid-rejoin used to race the rollback and strand
        // board/PC out of sync.
        "set_wifi_password" | "clear_wifi_password" | "rejoin_wifi" | "rejoin_wifi_open" => {
            Duration::from_secs(180)
        }
        // Wi-Fi scan (P2-2, v2.0.12 scan): one netsh call with a
        // 30 s timeout of its own, so the 30 s client bound left
        // zero headroom — 45 s.
        "scan_rsh_networks" => Duration::from_secs(45),
        // Control session + OS WLAN profile write; file copies /
        // restores — same 30 s bound, one arm.
        "backup_now" | "restore_backup" => Duration::from_secs(30),
        // `slow_poll` (P2-3, v2.0.12 scan): the cached static
        // groups are cheap, but on TTL expiry / generation change
        // the daemon re-reads them (incl. `list_games()` with
        // per-game disk probes) — 5 s so a cache miss does not
        // read as link degradation to the health monitor.
        "slow_poll" => Duration::from_secs(5),
        // Driver download + install.
        "install_vjoy" => Duration::from_secs(120),
        _ => CALL_TIMEOUT,
    }
}

/// Methods whose result arrives via events/polling, not the RPC
/// reply (B-1, v2.0.2). No-pairing redesign: the only
/// fire-and-forget method was `begin_pairing` (it only *started*
/// a pairing window); it is deleted, so the list is empty — kept
/// as an empty list so [`IpcClient::call`] keeps its shape for
/// any future method that legitimately waits on a human.
/// The late reply of such a method is discarded by the next
/// call's id check (the response channel holds 64 frames, so it
/// never blocks the reader).
const FIRE_AND_FORGET_METHODS: &[&str] = &[];

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
    /// Pushed events, drained by the caller. A `std` mutex (DS-78):
    /// `try_recv` needs no async runtime at all, so the drain is a
    /// plain sync `mpsc` drain — the frame timer no longer enters
    /// the runtime (`block_on`) once per event (and once per empty
    /// frame). The lock is held only for the `try_recv` call itself;
    /// the reader task never touches this side.
    events: std::sync::Mutex<mpsc::Receiver<AppEvent>>,
    /// The reader task's handle (kept alive for the client's life).
    _reader: tokio::task::JoinHandle<()>,
}

/// Which kind of JSON-lines frame a wire line is (R3-17).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireFrameKind {
    /// A pushed event frame (`{"v":…,"event":{…}}`).
    Event,
    /// A request/response frame (anything else, including an
    /// unparseable line — the reader drops those either way).
    Response,
}

/// Classify one wire line by STRUCTURE, not substring (DS-35,
/// B-09) — the old `line.contains("\"event\"")` misclassified
/// any response whose JSON contained the literal substring
/// `"event"` (e.g. a string value holding it, escaped as
/// `\"event\"`), parsed it as an event frame, failed, and
/// silently dropped the line — the caller then saw only the
/// 500 ms call timeout. A top-level `"event"` key marks an event
/// frame; anything else is a response. Split out of the reader
/// task so the fuzz target (`fuzz/fuzz_targets/ipc_wire.rs`,
/// R3-17) can drive it directly: a well-formed response must
/// never classify as an event, whatever its string values hold.
#[must_use]
pub fn classify_wire_line(line: &str) -> WireFrameKind {
    let is_event = serde_json::from_str::<serde_json::Value>(line)
        .is_ok_and(|value| value.get("event").is_some());
    if is_event {
        WireFrameKind::Event
    } else {
        WireFrameKind::Response
    }
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
        // R3-07: the event channel matches the daemon-side
        // broadcast (256) — a burst of events between UI frame
        // drains must not stall IPC responses.
        let (event_tx, event_rx) = mpsc::channel(256);
        let (resp_tx, resp_rx) = mpsc::channel::<IpcResponse>(64);
        // Reader task: demuxes response frames from event pushes.
        // Responses carry `id`; event frames carry `event`.
        let reader_task = tokio::spawn(async move {
            let mut lines = BufReader::new(reader);
            loop {
                // R3-08: bounded line read — a newline-free peer
                // used to grow the reader's buffer without bound.
                let Ok(Some(line)) = crate::read_bounded_line(&mut lines).await else {
                    break;
                };
                if line.trim().is_empty() {
                    continue;
                }
                // DS-35 (B-09): classify by STRUCTURE, not
                // substring — see [`classify_wire_line`].
                let is_event = classify_wire_line(&line) == WireFrameKind::Event;
                if is_event {
                    if let Ok(frame) = serde_json::from_str::<crate::IpcEventFrame>(&line)
                        && frame.v == IPC_PROTOCOL_VERSION
                    {
                        // R3-07: `try_send`, never `send().await` —
                        // awaiting here parked the reader (and with
                        // it every in-flight response) whenever the
                        // event channel filled between UI drains.
                        // Events are loss-tolerant (the UI re-reads
                        // state on its next poll), so a full channel
                        // drops the NEWEST event; a closed channel
                        // means the client is gone, so the task ends.
                        if let Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) =
                            event_tx.try_send(frame.event)
                        {
                            break;
                        }
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
            events: std::sync::Mutex::new(event_rx),
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

    /// Receive the next pushed event, if one is waiting (DS-78:
    /// synchronous — no runtime is needed to drain the channel).
    pub fn try_recv_event(&self) -> Option<AppEvent> {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .try_recv()
            .ok()
    }

    /// Call one method and await its response.
    ///
    /// The whole round-trip is bounded by
    /// [`timeout_for_method`] (P2-4): 500 ms by default (B3), longer
    /// for the methods whose daemon side legitimately takes seconds
    /// (backend switch, game writes, tuning with LED test, Wi-Fi
    /// password set, vJoy install, …). A daemon that never answers
    /// surfaces as [`IpcError::Io`] ("timed out") instead of hanging
    /// the caller forever. The call is strictly single-flight — the
    /// writer lock is held across the write + response wait, so no
    /// second call can interleave and steal the response.
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
        self.call_with_timeout(method, params, timeout_for_method(method))
            .await
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
    /// prompt.
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
    // The P2-4 timeout-table test below is platform-independent
    // (pure function, no socket), so it runs on Windows too — the
    // `super::*` import must NOT be unix-gated, or the Windows
    // target check cannot compile that test (`timeout_for_method`
    // / `Duration` not in scope). The other tests bind a real Unix
    // socket and stay `#[cfg(unix)]`.
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

    /// R3-07: a burst of events far larger than the event channel
    /// must not head-of-line-block IPC responses. The old reader
    /// awaited the event send, so >64 queued events parked it and
    /// every in-flight call timed out at 500 ms. The reader now
    /// `try_send`s (dropping the newest event when full), so a call
    /// issued after the burst still gets its response in time.
    #[test]
    #[cfg(unix)]
    fn event_burst_does_not_block_responses() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        rt.block_on(async {
            let (client_side, server_side) = tokio::io::duplex(1 << 20);
            let client = IpcClient::from_stream(client_side);
            tokio::spawn(async move {
                let (reader, mut writer) = tokio::io::split(server_side);
                let mut lines = BufReader::new(reader).lines();
                // Flood 600 events (2x the 256-slot channel) before
                // ever reading the request — the client drains
                // nothing during this window.
                for _ in 0..600 {
                    let frame = serde_json::json!({
                        "v": IPC_PROTOCOL_VERSION,
                        "event": {"Notification": {"kind": "Test", "title": "t", "body": "b"}},
                    });
                    let text = serde_json::to_string(&frame).expect("encode");
                    let _ = writer.write_all(text.as_bytes()).await;
                    let _ = writer.write_all(b"\n").await;
                }
                let _ = writer.flush().await;
                if let Ok(Some(line)) = lines.next_line().await {
                    let id = serde_json::from_str::<serde_json::Value>(&line)
                        .ok()
                        .and_then(|value| value.get("id").and_then(serde_json::Value::as_u64))
                        .unwrap_or(0);
                    let response = serde_json::json!({
                        "v": IPC_PROTOCOL_VERSION,
                        "id": id,
                        "ok": true,
                        "result": { "pong": true },
                        "error": null,
                    });
                    let text = serde_json::to_string(&response).expect("encode");
                    let _ = writer.write_all(text.as_bytes()).await;
                    let _ = writer.write_all(b"\n").await;
                    let _ = writer.flush().await;
                }
            });
            let start = Instant::now();
            let result = client
                .call("ping", serde_json::json!({}))
                .await
                .expect("the response must arrive despite the event burst");
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "response must not time out behind the burst, took {:?}",
                start.elapsed()
            );
            assert_eq!(
                result.get("pong"),
                Some(&serde_json::Value::Bool(true)),
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

    /// P2-4: the per-method timeout table gives the known slow
    /// methods room by default (the old code gave every plain
    /// `call()` 500 ms, which mis-reported slow-but-working calls
    /// as timeouts).
    #[test]
    fn timeout_for_method_gives_slow_methods_room() {
        assert_eq!(timeout_for_method("ping"), Duration::from_millis(500));
        // P3-1 (v2.0.12 scan): the UI's long-call bound is 390 s,
        // not the stale 75 s.
        assert_eq!(
            timeout_for_method("set_output_backend"),
            Duration::from_secs(390)
        );
        // Bug 1 (v2.0.17): the G29 toggle recreates the HID backend
        // (sidecar respawn + UAC dwell), so it needs the 390 s arm
        // too — at the 500 ms default the UI timed out while the
        // daemon was still switching.
        assert_eq!(
            timeout_for_method("set_g29_spoof_mode"),
            Duration::from_secs(390)
        );
        assert_eq!(timeout_for_method("add_game"), Duration::from_secs(10));
        assert_eq!(timeout_for_method("set_tuning"), Duration::from_secs(10));
        assert_eq!(
            timeout_for_method("firmware_check"),
            Duration::from_secs(10)
        );
        // P2-1 (v2.0.12 scan): firmware pre-flight runs before
        // the flash thread spawns, so it needs the 10 s arm too.
        assert_eq!(
            timeout_for_method("update_firmware"),
            Duration::from_secs(10)
        );
        assert_eq!(timeout_for_method("backup_now"), Duration::from_secs(30));
        assert_eq!(timeout_for_method("install_vjoy"), Duration::from_secs(120));
        // P1-3 (v2.0.12 scan): Wi-Fi password/rejoin worst case
        // ≈150 s, so the bound is 180 s.
        assert_eq!(
            timeout_for_method("set_wifi_password"),
            Duration::from_secs(180)
        );
        assert_eq!(
            timeout_for_method("clear_wifi_password"),
            Duration::from_secs(180)
        );
        assert_eq!(timeout_for_method("rejoin_wifi"), Duration::from_secs(180));
        assert_eq!(
            timeout_for_method("rejoin_wifi_open"),
            Duration::from_secs(180)
        );
        // P2-2 (v2.0.12 scan): the scan's netsh call alone takes
        // up to 30 s, so the client bound is 45 s.
        assert_eq!(
            timeout_for_method("scan_rsh_networks"),
            Duration::from_secs(45)
        );
        // P2-3 (v2.0.12 scan): a slow_poll cache miss re-reads
        // the static groups, so it gets 5 s, not the 500 ms
        // default.
        assert_eq!(timeout_for_method("slow_poll"), Duration::from_secs(5));
        // v2.0.18 scan: `connect` can block up to ~5 s in
        // `start_engine` (supervisor spawn + ready wait), so the
        // 500 ms default timed out a connect that was proceeding;
        // `disconnect` runs an unbounded OS port rescan.
        assert_eq!(timeout_for_method("connect"), Duration::from_secs(10));
        assert_eq!(timeout_for_method("disconnect"), Duration::from_secs(10));
        assert_eq!(
            timeout_for_method("unknown_method"),
            Duration::from_millis(500)
        );
    }

    /// P2-4: a `set_tuning` reply arriving after 800 ms succeeds
    /// through the plain `call()` path — under the old 500 ms
    /// default it would have timed out while the daemon kept
    /// running.
    #[test]
    #[cfg(unix)]
    fn slow_set_tuning_succeeds_through_plain_call() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        rt.block_on(async {
            let (client_side, server_side) = tokio::io::duplex(8192);
            let client = IpcClient::from_stream(client_side);
            tokio::spawn(async move {
                let (reader, mut writer) = tokio::io::split(server_side);
                let mut lines = BufReader::new(reader).lines();
                if let Ok(Some(line)) = lines.next_line().await {
                    let id = serde_json::from_str::<serde_json::Value>(&line)
                        .ok()
                        .and_then(|value| value.get("id").and_then(serde_json::Value::as_u64))
                        .unwrap_or(0);
                    // Slower than the 500 ms default, inside the
                    // 10 s `set_tuning` bound.
                    tokio::time::sleep(Duration::from_millis(800)).await;
                    let response = serde_json::json!({
                        "v": IPC_PROTOCOL_VERSION,
                        "id": id,
                        "ok": true,
                        "result": { "ok": true },
                        "error": null,
                    });
                    let text = serde_json::to_string(&response).expect("encode");
                    let _ = writer.write_all(text.as_bytes()).await;
                    let _ = writer.write_all(b"\n").await;
                    let _ = writer.flush().await;
                }
            });
            let result = client
                .call("set_tuning", serde_json::json!({"module": "steering"}))
                .await
                .expect("plain call must wait out the 10 s method bound");
            assert_eq!(result.get("ok"), Some(&serde_json::Value::Bool(true)));
        });
    }
}
