//! Controllable fake `rsh-daemon` (v2.0.3 F17 test fixture).
//!
//! Speaks the REAL IPC wire protocol (`rsh_ipc::dispatch` over a
//! Unix socket at `<data_dir>/rsh-daemon.sock` on Unix, or the
//! Windows named pipe `\\.\pipe\rsh-daemon` on Windows) with canned
//! disconnected/neutral data, so the real UI treats it as a live
//! daemon. Behaviour is selected by environment:
//!
//! * `RSH_FAKE_DAEMON_MODE` — `live` (default), `crash` (exit 1 at
//!   once), `exit-after-bind` (bind the socket, then exit 0,
//!   leaving a stale socket), `silent` (bind, accept, never reply),
//!   `slow` (ping answers fast, every other method waits
//!   `RSH_FAKE_DAEMON_SLOW_MS`, default 2000), `flap` (answer
//!   `RSH_FAKE_DAEMON_FLAP_PINGS` pings, default 3, then exit 0).
//!   In `slow` mode `ping`, `subscribe` and `poll` stay fast
//!   (liveness channel healthy) and every other method waits the
//!   slow delay, so the UI goes live and then hits the slow data
//!   path — the shape that freezes `reconnect_to_live`.
//! * `RSH_FAKE_DAEMON_LOG` — every invocation appends one line
//!   (`<unix-ms> <pid> <mode>`) here: the spawn-counting harness
//!   for the respawn-bound regression test. The UI spawns this
//!   binary as its sibling `rsh-daemon`, so the log is the spawn
//!   count — no process polling needed.
//!
//! Arguments mirror the real daemon's test surface:
//! `rsh-fake-daemon [--data-dir DIR]`. Unknown args are ignored.
//! Test-only; never shipped.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rsh_api::{
    ApiTransport, AppApi, ConnectionInfo, ConnectionState, PairingInfo, SnapshotInfo,
    TuningSettings, UserError,
};
use rsh_ipc::{IPC_PROTOCOL_VERSION, IpcError, IpcErrorBody, IpcResponse};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Canned disconnected API: the fake daemon never has a board, so
/// every value is the honest disconnected/neutral one — never fake
/// live data.
struct StubApi;

impl AppApi for StubApi {
    fn version(&self) -> String {
        "2.0.2-fake".to_string()
    }

    fn connection(&self) -> ConnectionInfo {
        ConnectionInfo {
            state: ConnectionState::Idle,
            label: "Wheel not connected".to_string(),
            transport: ApiTransport::Wifi,
            address: String::new(),
            session_active: false,
            error: None,
            hint: None,
        }
    }

    fn connect(&self, _transport: ApiTransport) -> Result<ConnectionInfo, UserError> {
        Err(UserError::from_connection(
            rsh_api::errors::ConnectionError::Unreachable,
            "fake daemon: no board",
        ))
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
            key_reset_armed: false,
        }
    }

    fn begin_pairing(&self) -> Result<PairingInfo, UserError> {
        Err(UserError::from_connection(
            rsh_api::errors::ConnectionError::Unreachable,
            "fake daemon: no board",
        ))
    }

    fn snapshot(&self) -> SnapshotInfo {
        SnapshotInfo::default()
    }

    fn output_enabled(&self) -> bool {
        false
    }

    fn set_output_enabled(&self, _enabled: bool) {}

    fn get_tuning(&self, module: &str) -> TuningSettings {
        TuningSettings::neutral(module)
    }

    fn set_tuning(&self, tuning: &TuningSettings) -> Result<TuningSettings, UserError> {
        Ok(tuning.clone())
    }

    fn draft_status(&self) -> Option<rsh_api::DraftStatus> {
        None
    }

    fn save_draft(&self) -> Result<bool, UserError> {
        Ok(false)
    }

    fn discard_draft(&self) -> bool {
        false
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis())
}

fn log_spawn(mode: &str) {
    use std::io::Write as _;
    if let Ok(path) = std::env::var("RSH_FAKE_DAEMON_LOG") {
        let line = format!("{} {} {mode}\n", now_ms(), std::process::id());
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = file.write_all(line.as_bytes());
        }
    }
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// One client connection: read JSON lines, dispatch, reply.
/// `silent` never replies; `slow` delays every non-`ping` reply;
/// `flap` exits the process after `flap_pings` answered pings.
async fn serve_conn<S>(
    stream: S,
    api: Arc<dyn AppApi>,
    mode: &str,
    slow_ms: u64,
    flap_pings: usize,
    pings: Arc<AtomicU64>,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (reader, mut writer) = tokio::io::split(stream);
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let request = match rsh_ipc::parse_request(&line) {
            Ok(request) => request,
            Err(error) => {
                let id = serde_json::from_str::<serde_json::Value>(&line)
                    .ok()
                    .and_then(|v| v.get("id").and_then(serde_json::Value::as_u64))
                    .unwrap_or(0);
                let resp = IpcResponse {
                    v: IPC_PROTOCOL_VERSION,
                    id,
                    ok: false,
                    result: None,
                    error: Some(IpcErrorBody {
                        code: "Protocol".to_string(),
                        message: error.to_string(),
                    }),
                };
                if let Ok(text) = rsh_ipc::encode_response(&resp) {
                    let _ = writer.write_all(text.as_bytes()).await;
                    let _ = writer.write_all(b"\n").await;
                }
                continue;
            }
        };
        if mode == "silent" {
            // Accept + read, never answer: the client's 500 ms call
            // timeout is the only thing that saves it.
            continue;
        }
        let is_ping = request.method == "ping";
        // `subscribe` must stay fast: it is part of `IpcApi::connect`,
        // so delaying it past the 500 ms client bound would make the
        // UI treat the daemon as dead (and respawn forever) instead
        // of going live. `poll` must stay fast too: the `IpcApi`
        // poller marks the link dead after one failed `poll` round,
        // and then every data call fails fast (`Closed`) instead of
        // blocking — a slow `poll` would hide the freeze under test
        // behind the fail-fast instead of producing it. The
        // slow-daemon case under test is a daemon whose liveness
        // channel is healthy but whose data calls are slow (a loaded
        // daemon / slow IPC under load): `poll` fast, data slow.
        // That is what makes `reconnect_to_live`'s ~33 sequential
        // blocking calls freeze the UI thread.
        let is_fast = matches!(request.method.as_str(), "ping" | "subscribe" | "poll");
        if mode == "slow" && !is_fast {
            tokio::time::sleep(Duration::from_millis(slow_ms)).await;
        }
        let result = rsh_ipc::dispatch(&api, &request);
        let resp = match result {
            Ok(value) => IpcResponse {
                v: IPC_PROTOCOL_VERSION,
                id: request.id,
                ok: true,
                result: Some(value),
                error: None,
            },
            Err(IpcError::Api(code)) => IpcResponse {
                v: IPC_PROTOCOL_VERSION,
                id: request.id,
                ok: false,
                result: None,
                error: Some(IpcErrorBody {
                    // Match the real daemon server's message
                    // (`"daemon API error: {code}"`) — the rig's
                    // fake daemon must encode errors the way the
                    // real one does (an empty message here used to
                    // collapse `Unreachable` into `TransportClosed`
                    // on the UI side).
                    message: format!("daemon API error: {code}"),
                    code,
                }),
            },
            Err(error) => IpcResponse {
                v: IPC_PROTOCOL_VERSION,
                id: request.id,
                ok: false,
                result: None,
                error: Some(IpcErrorBody {
                    code: "Protocol".to_string(),
                    message: error.to_string(),
                }),
            },
        };
        if let Ok(text) = rsh_ipc::encode_response(&resp) {
            if writer.write_all(text.as_bytes()).await.is_err() {
                return;
            }
            if writer.write_all(b"\n").await.is_err() {
                return;
            }
        }
        if mode == "flap" && is_ping {
            let answered = pings.fetch_add(1, Ordering::SeqCst) + 1;
            if usize::try_from(answered).map_or(true, |n| n >= flap_pings) {
                // Let the reply flush, then die mid-session: the
                // UI sees a live daemon flap to dead.
                tokio::time::sleep(Duration::from_millis(100)).await;
                std::process::exit(0);
            }
        }
    }
}

/// Windows accept loop: named-pipe listener on
/// [`rsh_ipc::IPC_PIPE_NAME`], mirroring the real daemon's
/// `create_pipe_listener` / `serve_ipc` F1/F2 pattern
/// (`crates/rsh-daemon/src/main.rs`):
///
/// * `first_pipe_instance(true)` ONLY on the first create in the
///   process — after the UI connects, instance #1 stays alive, so
///   a second `first_pipe_instance(true)` create fails
///   `ERROR_ACCESS_DENIED` (the v2.0.2 loop killer: 1199
///   pipe-create failures in the user log).
/// * The NEXT listener is pre-created with
///   `first_pipe_instance(false)` BEFORE the accepted pipe is
///   handed to `serve_conn`, so a listener is always available.
///   A pre-create failure is NOT fatal — the loop-top create
///   retries it, and the accepted client is served regardless.
/// * `NamedPipeServer` has `connect().await`, not `accept()`.
#[cfg(windows)]
async fn run_windows(
    api: Arc<dyn AppApi>,
    mode: String,
    slow_ms: u64,
    flap_pings: usize,
    pings: Arc<AtomicU64>,
) {
    use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
    let mut first_create = true;
    // The listening instance carried between iterations (see the
    // doc comment above): pre-created before serving each
    // connection.
    let mut listener: Option<NamedPipeServer> = None;
    loop {
        let pipe = match listener.take() {
            Some(pipe) => pipe,
            None => match ServerOptions::new()
                .first_pipe_instance(first_create)
                .create(rsh_ipc::IPC_PIPE_NAME)
            {
                Ok(pipe) => pipe,
                Err(error) => {
                    eprintln!(
                        "fake-daemon: cannot bind {}: {error}",
                        rsh_ipc::IPC_PIPE_NAME
                    );
                    std::process::exit(1);
                }
            },
        };
        if first_create {
            first_create = false;
            if mode == "exit-after-bind" {
                std::process::exit(0);
            }
        }
        if pipe.connect().await.is_err() {
            continue;
        }
        // Pre-create the next listener BEFORE handing this pipe to
        // `serve_conn`. One best-effort attempt: a failure here is
        // NOT fatal — the loop-top create retries it, and the
        // accepted client is served regardless.
        listener = match ServerOptions::new()
            .first_pipe_instance(false)
            .create(rsh_ipc::IPC_PIPE_NAME)
        {
            Ok(next) => Some(next),
            Err(error) => {
                eprintln!("fake-daemon: IPC next-pipe create failed (will retry): {error}");
                None
            }
        };
        let api = Arc::clone(&api);
        let pings = Arc::clone(&pings);
        let mode = mode.clone();
        tokio::spawn(async move {
            serve_conn(pipe, api, &mode, slow_ms, flap_pings, pings).await;
        });
    }
}

#[tokio::main]
async fn main() {
    let mut data_dir: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--data-dir" {
            data_dir = args.next().map(PathBuf::from);
        }
    }
    let data_dir = data_dir
        .or_else(|| dirs::data_dir().map(|d| d.join("RSH")))
        .unwrap_or_else(|| PathBuf::from("."));
    let mode = std::env::var("RSH_FAKE_DAEMON_MODE").unwrap_or_else(|_| "live".to_string());
    let slow_ms = env_usize("RSH_FAKE_DAEMON_SLOW_MS", 2000) as u64;
    let flap_pings = env_usize("RSH_FAKE_DAEMON_FLAP_PINGS", 3);
    log_spawn(&mode);
    if mode == "crash" {
        std::process::exit(1);
    }
    let api: Arc<dyn AppApi> = Arc::new(StubApi);
    let pings = Arc::new(AtomicU64::new(0));
    #[cfg(unix)]
    {
        let path = rsh_ipc::socket_path(&data_dir);
        let listener = match rsh_ipc::bind_unix_socket(&path) {
            Ok(listener) => listener,
            Err(error) => {
                eprintln!("fake-daemon: cannot bind {}: {error}", path.display());
                std::process::exit(1);
            }
        };
        if mode == "exit-after-bind" {
            std::process::exit(0);
        }
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let api = Arc::clone(&api);
            let pings = Arc::clone(&pings);
            let mode = mode.clone();
            tokio::spawn(async move {
                serve_conn(stream, api, &mode, slow_ms, flap_pings, pings).await;
            });
        }
    }
    #[cfg(windows)]
    {
        // Windows has no Unix socket: the pipe name is fixed
        // (`rsh_ipc::IPC_PIPE_NAME`), so `--data-dir` is unused.
        let _ = data_dir;
        run_windows(api, mode, slow_ms, flap_pings, pings).await;
    }
}
