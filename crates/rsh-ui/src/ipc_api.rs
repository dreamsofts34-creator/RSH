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
    app_version, ApiTransport, AppApi, AppEvent, ConnectionError, ConnectionInfo, DraftStatus,
    PairingInfo, SnapshotInfo, TuningSettings, UserError,
};
use rsh_ipc::{IpcClient, IpcError};

/// Live daemon API over IPC.
pub struct IpcApi {
    client: Arc<IpcClient>,
    rt: tokio::runtime::Runtime,
    last_connection: Mutex<Option<ConnectionInfo>>,
    last_snapshot: Mutex<Option<SnapshotInfo>>,
    last_output: Mutex<Option<bool>>,
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
        drop(client);
        handle.join().expect("server thread");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
