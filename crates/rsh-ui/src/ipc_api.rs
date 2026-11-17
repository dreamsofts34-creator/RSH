//! Live `AppApi` implementation over the daemon IPC socket (Phase 2).
//!
//! [`IpcApi`] speaks the real `rsh-ipc` v1 JSON-lines protocol to the
//! running daemon (`crates/rsh-daemon`) and exposes it through the
//! same [`rsh_api::AppApi`] trait the simulated implementation uses,
//! so the UI code never knows which one is behind the trait. Method
//! calls go through a small multi-thread Tokio runtime (the client is
//! async; the Slint thread blocks on it — each call is one local
//! socket round-trip, sub-millisecond on loopback/Unix socket).
//!
//! Reads that cannot fail at the trait level (`connection`,
//! `snapshot`, `output_enabled`, `pairing_status`) return the last
//! good value on IPC failure, and an explicit `Error`-state
//! [`ConnectionInfo`] when there never was one — the UI then shows
//! "Connection lost" instead of stale data. Fallible calls
//! (`connect`, `disconnect`, `begin_pairing`) surface a typed
//! [`UserError`].
//!
//! The connection subscribes to [`AppEvent`] pushes (`subscribe`)
//! at connect time; [`IpcApi::poll_events`] drains them for the
//! UI's frame loop.

use std::path::Path;
use std::sync::{Arc, Mutex};

use rsh_api::connection::ConnectionState;
use rsh_api::{
    app_version, ApiTransport, AppApi, AppEvent, CalibrationInfo, ConnectionError, ConnectionInfo,
    DiagStats, DraftStatus, GameSummary, PairingInfo, ProfileSummary, SnapshotInfo, TuningSettings,
    UserError, VjoyStatus,
};
use rsh_ipc::{IpcClient, IpcError};

/// Live daemon API over IPC.
pub struct IpcApi {
    client: Arc<IpcClient>,
    rt: tokio::runtime::Runtime,
    last_connection: Mutex<Option<ConnectionInfo>>,
    last_snapshot: Mutex<Option<SnapshotInfo>>,
    last_output: Mutex<Option<bool>>,
    last_autostart: Mutex<Option<bool>>,
    last_backend: Mutex<Option<String>>,
}

impl IpcApi {
    /// Connect to the daemon at `path` and subscribe to push events.
    ///
    /// # Errors
    ///
    /// [`IpcError`] when the socket cannot be reached or the
    /// subscribe handshake fails (daemon not running).
    pub fn connect(path: &Path) -> Result<Arc<Self>, IpcError> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| IpcError::Io(e.to_string()))?;
        let client = rt.block_on(IpcClient::connect(path))?;
        rt.block_on(client.call("subscribe", serde_json::json!({})))?;
        Ok(Arc::new(Self {
            client,
            rt,
            last_connection: Mutex::new(None),
            last_snapshot: Mutex::new(None),
            last_output: Mutex::new(None),
            last_autostart: Mutex::new(None),
            last_backend: Mutex::new(None),
        }))
    }

    /// One `ping` round-trip (liveness check after connect).
    #[must_use]
    pub fn ping(&self) -> bool {
        self.rt.block_on(self.client.ping()).unwrap_or(false)
    }

    /// Drain all pushed [`AppEvent`]s waiting on this connection.
    #[must_use]
    pub fn poll_events(&self) -> Vec<AppEvent> {
        let mut out = Vec::new();
        while let Some(ev) = self.rt.block_on(self.client.try_recv_event()) {
            out.push(ev);
        }
        out
    }

    fn ipc_failed() -> UserError {
        UserError::from_connection(ConnectionError::TransportClosed, "daemon IPC link failed")
    }

    fn lost_connection() -> ConnectionInfo {
        ConnectionInfo {
            state: ConnectionState::Error {
                reason: ConnectionError::TransportClosed,
                hint: ConnectionError::TransportClosed.hint().to_string(),
                retry_in_ms: None,
            },
            label: "Connection lost".to_string(),
            transport: ApiTransport::Wifi,
            address: String::new(),
            session_active: false,
            error: Some(ConnectionError::TransportClosed),
            hint: Some(ConnectionError::TransportClosed.hint().to_string()),
        }
    }
}

impl AppApi for IpcApi {
    fn version(&self) -> String {
        let v = self
            .rt
            .block_on(self.client.version())
            .unwrap_or_else(|_| app_version().to_string());
        v
    }

    fn connection(&self) -> ConnectionInfo {
        let got: Result<ConnectionInfo, IpcError> = self.rt.block_on(async {
            let value = self
                .client
                .call("connection", serde_json::json!({}))
                .await?;
            serde_json::from_value(value).map_err(|e| IpcError::Protocol(e.to_string()))
        });
        match got {
            Ok(info) => {
                if let Ok(mut slot) = self.last_connection.lock() {
                    *slot = Some(info.clone());
                }
                info
            }
            Err(_) => self
                .last_connection
                .lock()
                .ok()
                .and_then(|slot| slot.clone())
                .unwrap_or_else(Self::lost_connection),
        }
    }

    fn connect(&self, transport: ApiTransport) -> Result<ConnectionInfo, UserError> {
        let got: Result<ConnectionInfo, IpcError> = self.rt.block_on(async {
            let value = self
                .client
                .call(
                    "connect",
                    serde_json::json!({"transport": transport.as_str()}),
                )
                .await?;
            serde_json::from_value(value).map_err(|e| IpcError::Protocol(e.to_string()))
        });
        got.map_err(|_| Self::ipc_failed())
    }

    fn disconnect(&self) -> Result<ConnectionInfo, UserError> {
        let got: Result<ConnectionInfo, IpcError> = self.rt.block_on(async {
            let value = self
                .client
                .call("disconnect", serde_json::json!({}))
                .await?;
            serde_json::from_value(value).map_err(|e| IpcError::Protocol(e.to_string()))
        });
        got.map_err(|_| Self::ipc_failed())
    }

    fn pairing_status(&self) -> PairingInfo {
        let got: Result<PairingInfo, IpcError> = self.rt.block_on(async {
            let value = self
                .client
                .call("pairing_status", serde_json::json!({}))
                .await?;
            serde_json::from_value(value).map_err(|e| IpcError::Protocol(e.to_string()))
        });
        got.unwrap_or(PairingInfo {
            seconds_left: 0,
            password: None,
            error: None,
            error_detail: None,
            board_mac: None,
            replacement_required: false,
        })
    }

    fn begin_pairing(&self) -> Result<PairingInfo, UserError> {
        let got: Result<PairingInfo, IpcError> = self.rt.block_on(async {
            let value = self
                .client
                .call("begin_pairing", serde_json::json!({}))
                .await?;
            serde_json::from_value(value).map_err(|e| IpcError::Protocol(e.to_string()))
        });
        got.map_err(|_| Self::ipc_failed())
    }

    fn snapshot(&self) -> SnapshotInfo {
        let got: Result<SnapshotInfo, IpcError> = self.rt.block_on(async {
            let value = self.client.call("snapshot", serde_json::json!({})).await?;
            serde_json::from_value(value).map_err(|e| IpcError::Protocol(e.to_string()))
        });
        match got {
            Ok(snap) => {
                if let Ok(mut slot) = self.last_snapshot.lock() {
                    *slot = Some(snap.clone());
                }
                snap
            }
            Err(_) => self
                .last_snapshot
                .lock()
                .ok()
                .and_then(|slot| slot.clone())
                .unwrap_or_default(),
        }
    }

    fn output_enabled(&self) -> bool {
        let got: Result<bool, IpcError> = self.rt.block_on(async {
            let value = self
                .client
                .call("output_enabled", serde_json::json!({}))
                .await?;
            value
                .get("enabled")
                .and_then(serde_json::Value::as_bool)
                .ok_or_else(|| IpcError::Protocol("output_enabled has no enabled".to_string()))
        });
        match got {
            Ok(enabled) => {
                if let Ok(mut slot) = self.last_output.lock() {
                    *slot = Some(enabled);
                }
                enabled
            }
            Err(_) => self
                .last_output
                .lock()
                .ok()
                .and_then(|slot| *slot)
                .unwrap_or(true),
        }
    }

    fn set_output_enabled(&self, enabled: bool) {
        let _ = self.rt.block_on(self.client.call(
            "set_output_enabled",
            serde_json::json!({"enabled": enabled}),
        ));
        if let Ok(mut slot) = self.last_output.lock() {
            *slot = Some(enabled);
        }
    }

    fn get_tuning(&self, module: &str) -> TuningSettings {
        let got: Result<TuningSettings, IpcError> = self.rt.block_on(async {
            let value = self
                .client
                .call("get_tuning", serde_json::json!({"module": module}))
                .await?;
            serde_json::from_value(value).map_err(|e| IpcError::Protocol(e.to_string()))
        });
        got.unwrap_or_else(|_| TuningSettings::neutral(module))
    }

    fn set_tuning(&self, tuning: &TuningSettings) -> Result<TuningSettings, UserError> {
        let got: Result<TuningSettings, IpcError> = self.rt.block_on(async {
            let value = self
                .client
                .call(
                    "set_tuning",
                    serde_json::to_value(tuning).map_err(|e| IpcError::Protocol(e.to_string()))?,
                )
                .await?;
            serde_json::from_value(value).map_err(|e| IpcError::Protocol(e.to_string()))
        });
        got.map_err(|_| Self::ipc_failed())
    }

    fn draft_status(&self) -> Option<DraftStatus> {
        let got: Result<Option<DraftStatus>, IpcError> = self.rt.block_on(async {
            let value = self
                .client
                .call("draft_status", serde_json::json!({}))
                .await?;
            serde_json::from_value(value).map_err(|e| IpcError::Protocol(e.to_string()))
        });
        got.unwrap_or(None)
    }

    fn save_draft(&self) -> Result<bool, UserError> {
        let got: Result<bool, IpcError> = self.rt.block_on(async {
            let value = self
                .client
                .call("save_draft", serde_json::json!({}))
                .await?;
            value
                .get("saved")
                .and_then(serde_json::Value::as_bool)
                .ok_or_else(|| IpcError::Protocol("save_draft has no saved".to_string()))
        });
        got.map_err(|_| Self::ipc_failed())
    }

    fn discard_draft(&self) -> bool {
        let got: Result<bool, IpcError> = self.rt.block_on(async {
            let value = self
                .client
                .call("discard_draft", serde_json::json!({}))
                .await?;
            value
                .get("discarded")
                .and_then(serde_json::Value::as_bool)
                .ok_or_else(|| IpcError::Protocol("discard_draft has no discarded".to_string()))
        });
        got.unwrap_or(false)
    }

    fn autostart(&self) -> bool {
        let got: Result<bool, IpcError> = self.rt.block_on(async {
            let value = self.client.call("autostart", serde_json::json!({})).await?;
            value
                .get("autostart")
                .and_then(serde_json::Value::as_bool)
                .ok_or_else(|| IpcError::Protocol("autostart has no autostart".to_string()))
        });
        match got {
            Ok(enabled) => {
                if let Ok(mut slot) = self.last_autostart.lock() {
                    *slot = Some(enabled);
                }
                enabled
            }
            Err(_) => self
                .last_autostart
                .lock()
                .ok()
                .and_then(|slot| *slot)
                .unwrap_or(false),
        }
    }

    fn set_autostart(&self, enabled: bool) -> Result<bool, UserError> {
        let got: Result<bool, IpcError> = self.rt.block_on(async {
            let value = self
                .client
                .call("set_autostart", serde_json::json!({"enabled": enabled}))
                .await?;
            value
                .get("autostart")
                .and_then(serde_json::Value::as_bool)
                .ok_or_else(|| IpcError::Protocol("set_autostart has no autostart".to_string()))
        });
        match got {
            Ok(new_state) => {
                if let Ok(mut slot) = self.last_autostart.lock() {
                    *slot = Some(new_state);
                }
                Ok(new_state)
            }
            Err(_) => Err(Self::ipc_failed()),
        }
    }

    fn firmware_version(&self) -> String {
        let got: Result<String, IpcError> = self.rt.block_on(async {
            let value = self
                .client
                .call("firmware_version", serde_json::json!({}))
                .await?;
            value
                .get("firmware_version")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| {
                    IpcError::Protocol("firmware_version has no firmware_version".to_string())
                })
        });
        got.unwrap_or_else(|_| "Unknown".to_string())
    }

    fn output_backend(&self) -> String {
        let got: Result<String, IpcError> = self.rt.block_on(async {
            let value = self
                .client
                .call("output_backend", serde_json::json!({}))
                .await?;
            value
                .get("output_backend")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| {
                    IpcError::Protocol("output_backend has no output_backend".to_string())
                })
        });
        match got {
            Ok(backend) => {
                if let Ok(mut slot) = self.last_backend.lock() {
                    *slot = Some(backend.clone());
                }
                backend
            }
            Err(_) => self
                .last_backend
                .lock()
                .ok()
                .and_then(|slot| slot.clone())
                .unwrap_or_else(|| "Unknown".to_string()),
        }
    }

    fn set_output_backend(&self, name: &str) -> Result<String, UserError> {
        let got: Result<String, IpcError> = self.rt.block_on(async {
            let value = self
                .client
                .call("set_output_backend", serde_json::json!({"name": name}))
                .await?;
            value
                .get("output_backend")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| {
                    IpcError::Protocol("set_output_backend has no output_backend".to_string())
                })
        });
        match got {
            Ok(backend) => {
                if let Ok(mut slot) = self.last_backend.lock() {
                    *slot = Some(backend.clone());
                }
                Ok(backend)
            }
            Err(_) => Err(Self::ipc_failed()),
        }
    }

    fn backup_now(&self) -> Result<String, UserError> {
        let got: Result<String, IpcError> = self.rt.block_on(async {
            let value = self
                .client
                .call("backup_now", serde_json::json!({}))
                .await?;
            value
                .get("message")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| IpcError::Protocol("backup_now has no message".to_string()))
        });
        got.map_err(|_| Self::ipc_failed())
    }

    // v2.0.1 hotfix: profiles / games / learn over the same socket.
    // Lists are empty (never invented) when the daemon cannot be
    // reached; fallible calls surface the IPC failure.
    fn list_profiles(&self) -> Vec<ProfileSummary> {
        let got: Result<Vec<ProfileSummary>, IpcError> = self.rt.block_on(async {
            let value = self
                .client
                .call("list_profiles", serde_json::json!({}))
                .await?;
            serde_json::from_value(value).map_err(|e| IpcError::Protocol(e.to_string()))
        });
        got.unwrap_or_default()
    }

    fn activate_profile(&self, id: &str) -> Result<String, UserError> {
        self.profile_name_call("activate_profile", "id", id)
    }

    fn create_profile(&self, name: &str) -> Result<String, UserError> {
        self.profile_name_call("create_profile", "name", name)
    }

    fn duplicate_profile(&self, id: &str) -> Result<String, UserError> {
        self.profile_name_call("duplicate_profile", "id", id)
    }

    fn delete_profile(&self, id: &str) -> Result<bool, UserError> {
        let got: Result<bool, IpcError> = self.rt.block_on(async {
            let value = self
                .client
                .call("delete_profile", serde_json::json!({"id": id}))
                .await?;
            value
                .get("deleted")
                .and_then(serde_json::Value::as_bool)
                .ok_or_else(|| IpcError::Protocol("delete_profile has no deleted".to_string()))
        });
        got.map_err(|_| Self::ipc_failed())
    }

    fn list_games(&self) -> Vec<GameSummary> {
        let got: Result<Vec<GameSummary>, IpcError> = self.rt.block_on(async {
            let value = self
                .client
                .call("list_games", serde_json::json!({}))
                .await?;
            serde_json::from_value(value).map_err(|e| IpcError::Protocol(e.to_string()))
        });
        got.unwrap_or_default()
    }

    fn launch_game(&self, id: &str) -> Result<GameSummary, UserError> {
        self.game_call("launch_game", id)
    }

    fn stop_game(&self, id: &str) -> Result<GameSummary, UserError> {
        self.game_call("stop_game", id)
    }

    fn game_state(&self, id: &str) -> Result<GameSummary, UserError> {
        self.game_call("game_state", id)
    }

    fn start_learn(&self) -> Result<bool, UserError> {
        self.learn_call("start_learn")
    }

    fn cancel_learn(&self) -> Result<bool, UserError> {
        self.learn_call("cancel_learn")
    }

    fn is_learning(&self) -> bool {
        let got: Result<bool, IpcError> = self.rt.block_on(async {
            let value = self
                .client
                .call("is_learning", serde_json::json!({}))
                .await?;
            value
                .get("learning")
                .and_then(serde_json::Value::as_bool)
                .ok_or_else(|| IpcError::Protocol("is_learning has no learning".to_string()))
        });
        got.unwrap_or(false)
    }

    fn take_learned_button(&self) -> Option<u8> {
        let got: Result<Option<u8>, IpcError> = self.rt.block_on(async {
            let value = self
                .client
                .call("take_learned_button", serde_json::json!({}))
                .await?;
            match value.get("button") {
                Some(serde_json::Value::Null) | None => Ok(None),
                Some(v) => v
                    .as_u64()
                    .and_then(|n| u8::try_from(n).ok())
                    .map(Some)
                    .ok_or_else(|| {
                        IpcError::Protocol("take_learned_button has no button".to_string())
                    }),
            }
        });
        got.unwrap_or(None)
    }

    // v2.0.1 hotfix BACKEND-2: calibration read, vJoy status/install,
    // diagnostics over the same socket.
    fn calibration(&self) -> Result<CalibrationInfo, UserError> {
        let got: Result<CalibrationInfo, IpcError> = self.rt.block_on(async {
            let value = self
                .client
                .call("calibration", serde_json::json!({}))
                .await?;
            serde_json::from_value(value).map_err(|e| IpcError::Protocol(e.to_string()))
        });
        got.map_err(|_| Self::ipc_failed())
    }

    fn vjoy_status(&self) -> Result<VjoyStatus, UserError> {
        let got: Result<VjoyStatus, IpcError> = self.rt.block_on(async {
            let value = self
                .client
                .call("vjoy_status", serde_json::json!({}))
                .await?;
            serde_json::from_value(value).map_err(|e| IpcError::Protocol(e.to_string()))
        });
        got.map_err(|_| Self::ipc_failed())
    }

    fn install_vjoy(&self) -> Result<String, UserError> {
        let got: Result<String, IpcError> = self.rt.block_on(async {
            let value = self
                .client
                .call("install_vjoy", serde_json::json!({}))
                .await?;
            value
                .get("message")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| IpcError::Protocol("install_vjoy has no message".to_string()))
        });
        got.map_err(|_| Self::ipc_failed())
    }

    fn diag_stats(&self) -> Result<DiagStats, UserError> {
        let got: Result<DiagStats, IpcError> = self.rt.block_on(async {
            let value = self
                .client
                .call("diag_stats", serde_json::json!({}))
                .await?;
            serde_json::from_value(value).map_err(|e| IpcError::Protocol(e.to_string()))
        });
        got.map_err(|_| Self::ipc_failed())
    }
}

impl IpcApi {
    /// Shared `{"name": …}` profile call (`activate` / `create` /
    /// `duplicate`): one string param in, the profile name out.
    fn profile_name_call(
        &self,
        method: &str,
        param: &str,
        value: &str,
    ) -> Result<String, UserError> {
        let mut params = serde_json::Map::new();
        params.insert(
            param.to_string(),
            serde_json::Value::String(value.to_string()),
        );
        let got: Result<String, IpcError> = self.rt.block_on(async {
            let result = self
                .client
                .call(method, serde_json::Value::Object(params))
                .await?;
            result
                .get("name")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| IpcError::Protocol(format!("{method} has no name")))
        });
        got.map_err(|_| Self::ipc_failed())
    }

    /// Shared `{"id": …}` game call: one game id in, its
    /// [`GameSummary`] out.
    fn game_call(&self, method: &str, id: &str) -> Result<GameSummary, UserError> {
        let got: Result<GameSummary, IpcError> = self.rt.block_on(async {
            let value = self
                .client
                .call(method, serde_json::json!({"id": id}))
                .await?;
            serde_json::from_value(value).map_err(|e| IpcError::Protocol(e.to_string()))
        });
        got.map_err(|_| Self::ipc_failed())
    }

    /// Shared no-param learn call: `{"learning": bool}` out.
    fn learn_call(&self, method: &str) -> Result<bool, UserError> {
        let got: Result<bool, IpcError> = self.rt.block_on(async {
            let value = self.client.call(method, serde_json::json!({})).await?;
            value
                .get("learning")
                .and_then(serde_json::Value::as_bool)
                .ok_or_else(|| IpcError::Protocol(format!("{method} has no learning")))
        });
        got.map_err(|_| Self::ipc_failed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim_api::SimApi;
    use rsh_ipc::IpcServer;
    use tokio::sync::broadcast;

    /// L4 end-to-end: `IpcApi` → real Unix socket → `dispatch` →
    /// `SimApi` (as the daemon stand-in) → back. Exercises the whole
    /// tuning transport, not just one layer.
    ///
    /// Plain `#[test]` (not `#[tokio::test]`): `IpcApi` builds its
    /// own Tokio runtime and blocks on it, which panics inside an
    /// async context — so the server runs on a background std thread
    /// with its own current-thread runtime instead.
    #[test]
    fn tuning_round_trip_over_real_socket() {
        // Serialised with the probe test: on Windows both share the
        // one global named pipe (see live_probe::TEST_IPC_PIPE_GUARD).
        // The guard is held until the server thread is joined below,
        // so the pipe is destroyed before the probe test can run.
        let _guard = crate::live_probe::TEST_IPC_PIPE_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = std::env::temp_dir().join(format!("rsh-ipc-api-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let sock = dir.join("rsh-daemon.sock");
        let api: Arc<dyn AppApi> = Arc::new(SimApi::new());
        let (tx, _rx) = broadcast::channel(64);
        let server = Arc::new(IpcServer::new(api, tx));
        // The server thread owns its runtime: it binds the socket
        // (needs a reactor) and signals readiness before accepting.
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
        #[cfg(unix)]
        let sock_clone = sock.clone();
        let handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("server runtime");
            rt.block_on(async move {
                #[cfg(unix)]
                {
                    let listener = rsh_ipc::bind_unix_socket(&sock_clone).expect("bind");
                    ready_tx.send(()).expect("ready signal");
                    if let Ok((stream, _)) = listener.accept().await {
                        server.serve_stream(stream).await;
                    }
                }
                #[cfg(windows)]
                {
                    use tokio::net::windows::named_pipe::ServerOptions;
                    let pipe = ServerOptions::new()
                        .first_pipe_instance(true)
                        .create(rsh_ipc::IPC_PIPE_NAME)
                        .expect("pipe");
                    ready_tx.send(()).expect("ready signal");
                    if pipe.connect().await.is_ok() {
                        server.serve_stream(pipe).await;
                    }
                }
            });
        });
        ready_rx.recv().expect("server bound");
        let client = IpcApi::connect(&sock).expect("connect");
        assert!(client.ping());
        // Neutral default before any edit.
        let got = client.get_tuning("steering");
        assert_eq!(got.module, "steering");
        assert_eq!(got.curve, 1.0);
        // Set + read back.
        let mut upd = TuningSettings::neutral("steering");
        upd.deadzone = 5.0;
        upd.curve = 1.6;
        let stored = client.set_tuning(&upd).expect("set_tuning");
        assert_eq!(stored.deadzone, 5.0);
        assert_eq!(client.get_tuning("steering").curve, 1.6);
        // Draft dirty, then save clears it.
        let status = client.draft_status().expect("dirty draft");
        assert!(status.dirty);
        assert_eq!(status.dirty_modules, vec!["steering"]);
        assert!(client.save_draft().expect("save"));
        assert!(client.draft_status().is_none());
        // Invalid record is rejected by the daemon side.
        let mut bad = TuningSettings::neutral("steering");
        bad.curve = 9.0;
        assert!(client.set_tuning(&bad).is_err());
        // Discard reverts to neutral.
        client.set_tuning(&upd).expect("edit again");
        assert!(client.discard_draft());
        assert_eq!(client.get_tuning("steering").deadzone, 0.0);
        // v2.0.1 settings methods over the same socket.
        assert!(client.autostart());
        assert!(!client.set_autostart(false).expect("disable autostart"));
        assert!(!client.autostart());
        assert!(client.set_autostart(true).expect("enable autostart"));
        assert_eq!(client.firmware_version(), "4.0.2");
        assert_eq!(client.output_backend(), "vJoy");
        assert_eq!(client.set_output_backend("vJoy").expect("vJoy"), "vJoy");
        assert_eq!(client.output_backend(), "vJoy");
        assert!(client.set_output_backend("bogus").is_err());
        assert_eq!(client.output_backend(), "vJoy");
        // Sim never pretends a backup happened.
        assert!(client.backup_now().is_err());
        // v2.0.1 BACKEND-2 over the same socket: the sim never fakes
        // a calibration, a vJoy install, or diagnostics.
        assert!(client.calibration().is_err());
        assert!(client.vjoy_status().is_err());
        assert!(client.install_vjoy().is_err());
        assert!(client.diag_stats().is_err());
        // v2.0.1 profiles / games / learn over the same socket.
        // The sim invents nothing: empty lists, honest errors.
        assert_eq!(client.list_profiles().len(), 0);
        assert_eq!(client.list_games().len(), 0);
        assert!(client.activate_profile("x").is_err());
        assert!(client.create_profile("New").is_err());
        assert!(client.duplicate_profile("x").is_err());
        assert!(client.delete_profile("x").is_err());
        assert!(client.launch_game("assetto_corsa").is_err());
        assert!(client.stop_game("assetto_corsa").is_err());
        assert!(client.game_state("assetto_corsa").is_err());
        assert!(client.start_learn().is_err());
        assert!(client.cancel_learn().is_err());
        assert!(!client.is_learning());
        assert_eq!(client.take_learned_button(), None);
        drop(client);
        handle.join().expect("server thread");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
