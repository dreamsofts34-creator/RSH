//! Live `AppApi` implementation over the daemon IPC socket (Phase 2).
//!
//! [`IpcApi`] speaks the real `rsh-ipc` v1 JSON-lines protocol to the
//! running daemon (`crates/rsh-daemon`) and exposes it through the
//! same [`rsh_api::AppApi`] trait the simulated implementation uses,
//! so the UI code never knows which one is behind the trait. Method
//! calls go through a small multi-thread Tokio runtime (the client is
//! async; the Slint thread blocks on it).
//!
//! Hang honesty (B2/B3, v2.0.2): every IPC round-trip is bounded to
//! 500 ms ([`IpcClient::call`]) and fails fast once the background
//! poller proves the link dead. The hot values (`connection`,
//! `snapshot`, `output_enabled`) are refreshed ~20 Hz by a runtime
//! task into [`LiveCache`] — one combined `poll` call per cycle
//! (L-4), so the three slots always hold values from the same
//! instant. The UI thread only ever reads the cache, so a hung
//! daemon can never freeze a frame. When the daemon dies
//! mid-session the UI calls [`IpcApi::clear_cached_state`]
//! (no stale connection / snapshot survives) and the health monitor
//! re-probes in the background.
//!
//! The connection subscribes to [`AppEvent`] pushes (`subscribe`)
//! at connect time; [`IpcApi::poll_events`] drains them for the
//! UI's frame loop.

use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

use rsh_api::connection::ConnectionState;
use rsh_api::{
    app_version, ApiTransport, AppApi, AppEvent, ButtonBinding, CalibrationInfo, ConnectionError,
    ConnectionInfo, DiagStats, DraftStatus, FirmwareCheck, FirmwareUpdateStatus, GameSummary,
    PairingInfo, PollState, ProfileSummary, SnapshotInfo, TuningSettings, UserError, VjoyStatus,
};
use rsh_ipc::{IpcClient, IpcError};

/// Background-polled live values (B3): the poller task in
/// [`IpcApi::connect`] refreshes these ~20 Hz, so the UI thread
/// only ever reads the cache. `link_dead` is set by the poller when
/// a round fails and cleared on the next good round; fallible calls
/// fail fast on it instead of waiting out the 500 ms timeout.
struct LiveCache {
    connection: Mutex<Option<ConnectionInfo>>,
    snapshot: Mutex<Option<SnapshotInfo>>,
    output: Mutex<Option<bool>>,
    link_dead: AtomicBool,
}

impl Default for LiveCache {
    fn default() -> Self {
        Self {
            connection: Mutex::new(None),
            snapshot: Mutex::new(None),
            output: Mutex::new(None),
            link_dead: AtomicBool::new(false),
        }
    }
}

/// Live daemon API over IPC.
pub struct IpcApi {
    client: Arc<IpcClient>,
    rt: tokio::runtime::Runtime,
    cache: Arc<LiveCache>,
    last_autostart: Mutex<Option<bool>>,
    last_backend: Mutex<Option<String>>,
}

impl IpcApi {
    /// Connect to the daemon at `path` and subscribe to push events.
    ///
    /// Also starts the background poller task: one combined `poll`
    /// call per cycle refreshes snapshot / connection / output
    /// ~20 Hz into the shared cache (B3 + L-4), so the UI thread
    /// never blocks on the daemon for them. The task holds only the
    /// client + cache (never the `IpcApi`), so it ends when the
    /// runtime shuts down with the last `Arc<IpcApi>`.
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
        let client = rt.block_on(async {
            tokio::time::timeout(Duration::from_secs(2), IpcClient::connect(path))
                .await
                .map_err(|_| IpcError::Io("daemon IPC connect timed out after 2 s".to_string()))?
        })?;
        rt.block_on(client.call("subscribe", serde_json::json!({})))?;
        let cache = Arc::new(LiveCache::default());
        let api = Arc::new(Self {
            client: client.clone(),
            rt,
            cache: cache.clone(),
            last_autostart: Mutex::new(None),
            last_backend: Mutex::new(None),
        });
        {
            let poll_client = client;
            let poll_cache = cache;
            api.rt.spawn(async move {
                loop {
                    // L-4 (v2.0.2): ONE combined `poll` round-trip
                    // per cycle — snapshot + connection + output
                    // from one server-side dispatch, so the three
                    // cache slots always hold values from the same
                    // instant. The old three sequential calls could
                    // tear (a snapshot from while connected paired
                    // with a connection that had since flipped) and
                    // went ~1.5 s stale on a slow daemon. A failed
                    // call or a failed parse counts as a failed
                    // round, same as before: the cache keeps its
                    // last good values and `link_dead` trips below.
                    let ok = match poll_client.call("poll", serde_json::json!({})).await {
                        Ok(value) => match serde_json::from_value::<PollState>(value) {
                            Ok(polled) => {
                                if let Ok(mut slot) = poll_cache.snapshot.lock() {
                                    *slot = Some(polled.snapshot);
                                }
                                if let Ok(mut slot) = poll_cache.connection.lock() {
                                    *slot = Some(polled.connection);
                                }
                                if let Ok(mut slot) = poll_cache.output.lock() {
                                    *slot = Some(polled.output_enabled);
                                }
                                true
                            }
                            Err(_) => false,
                        },
                        Err(_) => false,
                    };
                    poll_cache.link_dead.store(!ok, Ordering::Relaxed);
                    // A9/B7: a failed round backs off to 500 ms — a
                    // wedged daemon must not let the poller hog the
                    // single-flight IPC lock (and the CPU) at 20 Hz,
                    // starving the UI thread's own calls.
                    let delay_ms = if ok { 50 } else { 500 };
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                }
            });
        }
        Ok(api)
    }

    /// One IPC round-trip for a trait method (B3): fails fast with
    /// [`IpcError::Closed`] when the poller already proved the link
    /// dead, otherwise one `client.call` — itself bounded to 500 ms
    /// (see [`IpcClient::call`]). Async so the existing `block_on`
    /// call sites only change the callee; [`IpcApi::ping`] bypasses
    /// it on purpose (the health monitor needs a real attempt).
    async fn call_async(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, IpcError> {
        if self.cache.link_dead.load(Ordering::Relaxed) {
            return Err(IpcError::Closed);
        }
        self.client.call(method, params).await
    }

    /// [`call_async`] with an explicit bound (B-2, v2.0.2): the
    /// backend switch waits out a UAC prompt + a 60 s sidecar spawn
    /// (75 s), so the UI does not time out while the user is still
    /// answering the prompt. Same fail-fast on a proven-dead link.
    async fn call_async_long(
        &self,
        method: &str,
        params: serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, IpcError> {
        if self.cache.link_dead.load(Ordering::Relaxed) {
            return Err(IpcError::Closed);
        }
        self.client.call_with_timeout(method, params, timeout).await
    }

    /// One `ping` round-trip (liveness check after connect, and the
    /// health monitor's 1 s probe — always a real attempt, never
    /// fail-fast, bounded by the 500 ms client timeout).
    #[must_use]
    pub fn ping(&self) -> bool {
        self.rt.block_on(self.client.ping()).unwrap_or(false)
    }

    /// Drop every cached live value and mark the link dead (B2):
    /// the UI calls this when the health monitor proves the daemon
    /// died, so no frame ever shows a stale connection / snapshot /
    /// output again. A discarded link's poller keeps running only
    /// until its `IpcApi` is dropped with the runtime.
    pub fn clear_cached_state(&self) {
        if let Ok(mut slot) = self.cache.connection.lock() {
            *slot = None;
        }
        if let Ok(mut slot) = self.cache.snapshot.lock() {
            *slot = None;
        }
        if let Ok(mut slot) = self.cache.output.lock() {
            *slot = None;
        }
        if let Ok(mut slot) = self.last_autostart.lock() {
            *slot = None;
        }
        if let Ok(mut slot) = self.last_backend.lock() {
            *slot = None;
        }
        self.cache.link_dead.store(true, Ordering::Relaxed);
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
        let got: Result<String, IpcError> = self.rt.block_on(async {
            let value = self.call_async("version", serde_json::json!({})).await?;
            value
                .get("version")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| IpcError::Protocol("version response has no version".to_string()))
        });
        got.unwrap_or_else(|_| app_version().to_string())
    }

    /// Cached connection (B3): the poller refreshes it ~20 Hz, so
    /// this never touches the socket. No cache yet (or cleared by
    /// [`IpcApi::clear_cached_state`]) means "Connection lost" —
    /// never a stale last-good value.
    fn connection(&self) -> ConnectionInfo {
        self.cache
            .connection
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
            .unwrap_or_else(Self::lost_connection)
    }

    fn connect(&self, transport: ApiTransport) -> Result<ConnectionInfo, UserError> {
        let got: Result<ConnectionInfo, IpcError> = self.rt.block_on(async {
            let value = self
                .call_async(
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
            let value = self.call_async("disconnect", serde_json::json!({})).await?;
            serde_json::from_value(value).map_err(|e| IpcError::Protocol(e.to_string()))
        });
        got.map_err(|_| Self::ipc_failed())
    }

    fn pairing_status(&self) -> PairingInfo {
        let got: Result<PairingInfo, IpcError> = self.rt.block_on(async {
            let value = self
                .call_async("pairing_status", serde_json::json!({}))
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
            key_reset_armed: false,
        })
    }

    fn begin_pairing(&self) -> Result<PairingInfo, UserError> {
        // B-1: fire-and-forget start — the client returns as soon as
        // the request is written (a slow board must not time the
        // call out), so the reply is `null`; the real status arrives
        // via `PairingWindow` events + `pairing_status` polling.
        let got: Result<PairingInfo, IpcError> = self.rt.block_on(async {
            let value = self
                .call_async("begin_pairing", serde_json::json!({}))
                .await?;
            Ok(serde_json::from_value(value).unwrap_or(PairingInfo {
                seconds_left: 0,
                password: None,
                error: None,
                error_detail: None,
                board_mac: None,
                replacement_required: false,
                key_reset_armed: false,
            }))
        });
        got.map_err(|_| Self::ipc_failed())
    }

    /// Cached snapshot (B3): the poller refreshes it ~20 Hz, so
    /// this never touches the socket. No cache yet (or cleared by
    /// [`IpcApi::clear_cached_state`]) means zeroed values — never
    /// a stale reading shown as live.
    fn snapshot(&self) -> SnapshotInfo {
        self.cache
            .snapshot
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
            .unwrap_or_default()
    }

    /// Cached output flag (B3): same poller cache as the snapshot.
    /// No cache yet means on (the pre-poller default).
    fn output_enabled(&self) -> bool {
        self.cache
            .output
            .lock()
            .ok()
            .and_then(|slot| *slot)
            .unwrap_or(true)
    }

    fn set_output_enabled(&self, enabled: bool) {
        let _ = self.rt.block_on(self.call_async(
            "set_output_enabled",
            serde_json::json!({"enabled": enabled}),
        ));
        if let Ok(mut slot) = self.cache.output.lock() {
            *slot = Some(enabled);
        }
    }

    fn get_tuning(&self, module: &str) -> TuningSettings {
        let got: Result<TuningSettings, IpcError> = self.rt.block_on(async {
            let value = self
                .call_async("get_tuning", serde_json::json!({"module": module}))
                .await?;
            serde_json::from_value(value).map_err(|e| IpcError::Protocol(e.to_string()))
        });
        got.unwrap_or_else(|_| TuningSettings::neutral(module))
    }

    fn set_tuning(&self, tuning: &TuningSettings) -> Result<TuningSettings, UserError> {
        let got: Result<TuningSettings, IpcError> = self.rt.block_on(async {
            let value = self
                .call_async(
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
                .call_async("draft_status", serde_json::json!({}))
                .await?;
            serde_json::from_value(value).map_err(|e| IpcError::Protocol(e.to_string()))
        });
        got.unwrap_or(None)
    }

    fn save_draft(&self) -> Result<bool, UserError> {
        let got: Result<bool, IpcError> = self.rt.block_on(async {
            let value = self.call_async("save_draft", serde_json::json!({})).await?;
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
                .call_async("discard_draft", serde_json::json!({}))
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
            let value = self.call_async("autostart", serde_json::json!({})).await?;
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
                .call_async("set_autostart", serde_json::json!({"enabled": enabled}))
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
                .call_async("firmware_version", serde_json::json!({}))
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
                .call_async("output_backend", serde_json::json!({}))
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
                .call_async_long(
                    "set_output_backend",
                    serde_json::json!({"name": name}),
                    // B-2: the switch waits out the UAC prompt (the
                    // user may take seconds to answer) + the 60 s
                    // sidecar spawn — a 500 ms bound timed out while
                    // the prompt was still on screen, reverted the
                    // toggle, and looped the prompt. 75 s covers the
                    // 60 s spawn bound with margin.
                    Duration::from_secs(75),
                )
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
            let value = self.call_async("backup_now", serde_json::json!({})).await?;
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
                .call_async("list_profiles", serde_json::json!({}))
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
                .call_async("delete_profile", serde_json::json!({"id": id}))
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
            let value = self.call_async("list_games", serde_json::json!({})).await?;
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
                .call_async("is_learning", serde_json::json!({}))
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
                .call_async("take_learned_button", serde_json::json!({}))
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
                .call_async("calibration", serde_json::json!({}))
                .await?;
            serde_json::from_value(value).map_err(|e| IpcError::Protocol(e.to_string()))
        });
        got.map_err(|_| Self::ipc_failed())
    }

    fn vjoy_status(&self) -> Result<VjoyStatus, UserError> {
        let got: Result<VjoyStatus, IpcError> = self.rt.block_on(async {
            let value = self
                .call_async("vjoy_status", serde_json::json!({}))
                .await?;
            serde_json::from_value(value).map_err(|e| IpcError::Protocol(e.to_string()))
        });
        got.map_err(|_| Self::ipc_failed())
    }

    fn install_vjoy(&self) -> Result<String, UserError> {
        let got: Result<String, IpcError> = self.rt.block_on(async {
            let value = self
                .call_async("install_vjoy", serde_json::json!({}))
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
            let value = self.call_async("diag_stats", serde_json::json!({})).await?;
            serde_json::from_value(value).map_err(|e| IpcError::Protocol(e.to_string()))
        });
        got.map_err(|_| Self::ipc_failed())
    }

    // v2.0.2 capture: the daemon-side methods, on this connection
    // (the UI's second connection, `CaptureLink`, is gone — see
    // `main.rs`). A failed capture surfaces the daemon's reason as
    // the [`UserError`] title, never a bare "Unknown".
    fn capture_centre(&self) -> Result<u16, UserError> {
        self.capture_mv("capture_centre", serde_json::json!({}))
    }

    fn capture_pedal_min(&self, pedal: &str) -> Result<u16, UserError> {
        self.capture_mv("capture_pedal_min", serde_json::json!({"pedal": pedal}))
    }

    fn capture_pedal_max(&self, pedal: &str) -> Result<u16, UserError> {
        self.capture_mv("capture_pedal_max", serde_json::json!({"pedal": pedal}))
    }

    // v2.0.2 Round 3: pairing reset, firmware check/update, board
    // address, module toggles, auto-pause, tray, backup, profile
    // rename/link, knob actions, bindings. Failures surface the
    // daemon's typed reason ([`Self::api_error`]), never a bare
    // "Unknown" — the UI shows the reason + the next step.
    fn reset_pairing(&self) -> Result<PairingInfo, UserError> {
        let got: Result<PairingInfo, IpcError> = self.rt.block_on(async {
            let value = self
                .call_async("reset_pairing", serde_json::json!({}))
                .await?;
            serde_json::from_value(value).map_err(|e| IpcError::Protocol(e.to_string()))
        });
        got.map_err(|e| Self::api_error(&e))
    }

    fn list_bindings(&self) -> Vec<ButtonBinding> {
        let got: Result<Vec<ButtonBinding>, IpcError> = self.rt.block_on(async {
            let value = self
                .call_async("list_bindings", serde_json::json!({}))
                .await?;
            value
                .get("bindings")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(|e| IpcError::Protocol(e.to_string()))?
                .ok_or_else(|| IpcError::Protocol("list_bindings has no bindings".to_string()))
        });
        got.unwrap_or_default()
    }

    fn set_binding(&self, button: u8, action: &str) -> Result<ButtonBinding, UserError> {
        let got: Result<ButtonBinding, IpcError> = self.rt.block_on(async {
            let value = self
                .call_async(
                    "set_binding",
                    serde_json::json!({"button": button, "action": action}),
                )
                .await?;
            value
                .get("binding")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(|e| IpcError::Protocol(e.to_string()))?
                .ok_or_else(|| IpcError::Protocol("set_binding has no binding".to_string()))
        });
        got.map_err(|e| Self::api_error(&e))
    }

    fn clear_binding(&self, button: u8) -> Result<bool, UserError> {
        let got: Result<bool, IpcError> = self.rt.block_on(async {
            let value = self
                .call_async("clear_binding", serde_json::json!({"button": button}))
                .await?;
            value
                .get("cleared")
                .and_then(serde_json::Value::as_bool)
                .ok_or_else(|| IpcError::Protocol("clear_binding has no cleared".to_string()))
        });
        got.map_err(|e| Self::api_error(&e))
    }

    fn firmware_check(&self) -> Result<FirmwareCheck, UserError> {
        let got: Result<FirmwareCheck, IpcError> = self.rt.block_on(async {
            let value = self
                .call_async("firmware_check", serde_json::json!({}))
                .await?;
            serde_json::from_value(value).map_err(|e| IpcError::Protocol(e.to_string()))
        });
        got.map_err(|e| Self::api_error(&e))
    }

    fn update_firmware(&self) -> Result<FirmwareUpdateStatus, UserError> {
        // Start only: the daemon validates + spawns the flash thread
        // and returns the running status at once, so the 500 ms
        // bound is plenty — the flash itself never blocks this call.
        let got: Result<FirmwareUpdateStatus, IpcError> = self.rt.block_on(async {
            let value = self
                .call_async("update_firmware", serde_json::json!({}))
                .await?;
            serde_json::from_value(value).map_err(|e| IpcError::Protocol(e.to_string()))
        });
        got.map_err(|e| Self::api_error(&e))
    }

    fn firmware_update_status(&self) -> FirmwareUpdateStatus {
        let got: Result<FirmwareUpdateStatus, IpcError> = self.rt.block_on(async {
            let value = self
                .call_async("firmware_update_status", serde_json::json!({}))
                .await?;
            serde_json::from_value(value).map_err(|e| IpcError::Protocol(e.to_string()))
        });
        got.unwrap_or_default()
    }

    fn board_address(&self) -> String {
        let got: Result<String, IpcError> = self.rt.block_on(async {
            let value = self
                .call_async("board_address", serde_json::json!({}))
                .await?;
            value
                .get("address")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| IpcError::Protocol("board_address has no address".to_string()))
        });
        got.unwrap_or_else(|_| "192.168.4.1".to_string())
    }

    fn set_board_address(&self, address: &str) -> Result<String, UserError> {
        let got: Result<String, IpcError> = self.rt.block_on(async {
            let value = self
                .call_async("set_board_address", serde_json::json!({"address": address}))
                .await?;
            value
                .get("address")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| IpcError::Protocol("set_board_address has no address".to_string()))
        });
        got.map_err(|e| Self::api_error(&e))
    }

    fn module_enabled(&self, module: &str) -> bool {
        let got: Result<bool, IpcError> = self.rt.block_on(async {
            let value = self
                .call_async("module_enabled", serde_json::json!({"module": module}))
                .await?;
            value
                .get("enabled")
                .and_then(serde_json::Value::as_bool)
                .ok_or_else(|| IpcError::Protocol("module_enabled has no enabled".to_string()))
        });
        got.unwrap_or(true)
    }

    fn set_module_enabled(&self, module: &str, enabled: bool) -> Result<bool, UserError> {
        let got: Result<bool, IpcError> = self.rt.block_on(async {
            let value = self
                .call_async(
                    "set_module_enabled",
                    serde_json::json!({"module": module, "enabled": enabled}),
                )
                .await?;
            value
                .get("enabled")
                .and_then(serde_json::Value::as_bool)
                .ok_or_else(|| IpcError::Protocol("set_module_enabled has no enabled".to_string()))
        });
        got.map_err(|e| Self::api_error(&e))
    }

    fn auto_pause(&self) -> bool {
        let got: Result<bool, IpcError> = self.rt.block_on(async {
            let value = self.call_async("auto_pause", serde_json::json!({})).await?;
            value
                .get("paused")
                .and_then(serde_json::Value::as_bool)
                .ok_or_else(|| IpcError::Protocol("auto_pause has no paused".to_string()))
        });
        got.unwrap_or(false)
    }

    fn set_auto_pause(&self, paused: bool) -> Result<bool, UserError> {
        let got: Result<bool, IpcError> = self.rt.block_on(async {
            let value = self
                .call_async("set_auto_pause", serde_json::json!({"paused": paused}))
                .await?;
            value
                .get("paused")
                .and_then(serde_json::Value::as_bool)
                .ok_or_else(|| IpcError::Protocol("set_auto_pause has no paused".to_string()))
        });
        got.map_err(|e| Self::api_error(&e))
    }

    fn close_to_tray(&self) -> bool {
        let got: Result<bool, IpcError> = self.rt.block_on(async {
            let value = self
                .call_async("close_to_tray", serde_json::json!({}))
                .await?;
            value
                .get("close_to_tray")
                .and_then(serde_json::Value::as_bool)
                .ok_or_else(|| IpcError::Protocol("close_to_tray has no close_to_tray".to_string()))
        });
        got.unwrap_or(true)
    }

    fn set_close_to_tray(&self, enabled: bool) -> Result<bool, UserError> {
        let got: Result<bool, IpcError> = self.rt.block_on(async {
            let value = self
                .call_async("set_close_to_tray", serde_json::json!({"enabled": enabled}))
                .await?;
            value
                .get("close_to_tray")
                .and_then(serde_json::Value::as_bool)
                .ok_or_else(|| {
                    IpcError::Protocol("set_close_to_tray has no close_to_tray".to_string())
                })
        });
        got.map_err(|e| Self::api_error(&e))
    }

    fn restore_backup(&self) -> Result<String, UserError> {
        let got: Result<String, IpcError> = self.rt.block_on(async {
            let value = self
                .call_async("restore_backup", serde_json::json!({}))
                .await?;
            value
                .get("message")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| IpcError::Protocol("restore_backup has no message".to_string()))
        });
        got.map_err(|e| Self::api_error(&e))
    }

    fn rename_profile(&self, id: &str, name: &str) -> Result<String, UserError> {
        self.profile_two_param_call("rename_profile", "id", id, "name", name)
    }

    fn link_profile(&self, id: &str, game_slug: &str) -> Result<String, UserError> {
        self.profile_two_param_call("link_profile", "id", id, "game_slug", game_slug)
    }

    fn knob_actions(&self) -> [[String; 3]; 2] {
        let got: Result<[[String; 3]; 2], IpcError> = self.rt.block_on(async {
            let value = self
                .call_async("knob_actions", serde_json::json!({}))
                .await?;
            value
                .get("knobs")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(|e| IpcError::Protocol(e.to_string()))?
                .ok_or_else(|| IpcError::Protocol("knob_actions has no knobs".to_string()))
        });
        got.unwrap_or_default()
    }

    fn set_knob_action(&self, knob: u8, row: u8, action: &str) -> Result<(), UserError> {
        let got: Result<(), IpcError> = self.rt.block_on(async {
            self.call_async(
                "set_knob_action",
                serde_json::json!({"knob": knob, "row": row, "action": action}),
            )
            .await?;
            Ok(())
        });
        got.map_err(|e| Self::api_error(&e))
    }
}

impl IpcApi {
    /// One capture round-trip: `{"mv": n}` out, or the daemon's
    /// reason as the [`UserError`] title (see
    /// [`capture_code_detail`]).
    fn capture_mv(&self, method: &str, params: serde_json::Value) -> Result<u16, UserError> {
        let got: Result<u16, IpcError> = self.rt.block_on(async {
            let value = self.call_async(method, params).await?;
            value
                .get("mv")
                .and_then(serde_json::Value::as_u64)
                .and_then(|mv| u16::try_from(mv).ok())
                .ok_or_else(|| IpcError::Protocol(format!("{method} has no mv")))
        });
        got.map_err(|e| Self::capture_error(&e))
    }

    /// Build the [`UserError`] for a failed capture: the daemon's
    /// `code|detail` payload (see [`capture_code_detail`])
    /// becomes code + title, so the UI notification shows the real
    /// reason ("no live wheel data yet") instead of a bare
    /// "Unknown".
    fn capture_error(error: &IpcError) -> UserError {
        let (code, detail) = capture_code_detail(error);
        UserError {
            code,
            title: detail.clone(),
            what_happened: detail.clone(),
            what_to_do: "Reconnect and try again.".to_string(),
            detail: Some(detail),
        }
    }

    /// Build the [`UserError`] for a failed Round 3 call (B6): the
    /// daemon's `code|detail` payload (see `detailed_api_error`)
    /// keeps its code + reason; anything else (link down, timeout,
    /// protocol) is the generic link failure. The UI's
    /// `notify_error` maps known codes to a plain title + next step.
    fn api_error(error: &IpcError) -> UserError {
        if let IpcError::Api(payload) = error {
            if let Some((code, detail)) = payload.split_once('|') {
                if !detail.is_empty() {
                    return UserError {
                        code: code.to_string(),
                        title: detail.to_string(),
                        what_happened: detail.to_string(),
                        what_to_do: "Try again.".to_string(),
                        detail: Some(detail.to_string()),
                    };
                }
            }
            return UserError::from_connection(ConnectionError::Unknown, payload.clone());
        }
        Self::ipc_failed()
    }

    /// Shared two-string-param profile call (`rename_profile`,
    /// `link_profile`): two params in, the profile name out.
    fn profile_two_param_call(
        &self,
        method: &str,
        param_a: &str,
        value_a: &str,
        param_b: &str,
        value_b: &str,
    ) -> Result<String, UserError> {
        let mut params = serde_json::Map::new();
        params.insert(
            param_a.to_string(),
            serde_json::Value::String(value_a.to_string()),
        );
        params.insert(
            param_b.to_string(),
            serde_json::Value::String(value_b.to_string()),
        );
        let got: Result<String, IpcError> = self.rt.block_on(async {
            let result = self
                .call_async(method, serde_json::Value::Object(params))
                .await?;
            result
                .get("name")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| IpcError::Protocol(format!("{method} has no name")))
        });
        got.map_err(|e| Self::api_error(&e))
    }

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
                .call_async(method, serde_json::Value::Object(params))
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
                .call_async(method, serde_json::json!({"id": id}))
                .await?;
            serde_json::from_value(value).map_err(|e| IpcError::Protocol(e.to_string()))
        });
        got.map_err(|_| Self::ipc_failed())
    }

    /// Shared no-param learn call: `{"learning": bool}` out.
    fn learn_call(&self, method: &str) -> Result<bool, UserError> {
        let got: Result<bool, IpcError> = self.rt.block_on(async {
            let value = self.call_async(method, serde_json::json!({})).await?;
            value
                .get("learning")
                .and_then(serde_json::Value::as_bool)
                .ok_or_else(|| IpcError::Protocol(format!("{method} has no learning")))
        });
        got.map_err(|_| Self::ipc_failed())
    }
}

/// Split a capture [`IpcError::Api`] payload into its stable code +
/// human reason (v2.0.2). The daemon's capture errors travel as
/// `code|detail` inside [`IpcError::Api`] (see `rsh_ipc::dispatch`):
/// the code is the stable class, the detail the reason. The client
/// formats the payload as `"{code}: {message}"` where both halves
/// carry the `code|detail` payload, so the first half is enough.
/// Anything else (link down, protocol) means RSH is not answering
/// — the same visible reason, never a silent failure. Only the
/// capture methods use the `code|detail` payload — every other
/// method keeps the code-only mapping.
fn capture_code_detail(error: &IpcError) -> (String, String) {
    const FALLBACK: &str = "RSH isn't running — reconnect to capture.";
    if let IpcError::Api(payload) = error {
        let head = payload
            .split(": daemon API error: ")
            .next()
            .unwrap_or(payload);
        if let Some((code, detail)) = head.split_once('|') {
            if !detail.is_empty() {
                return (code.to_string(), detail.to_string());
            }
        }
    }
    ("TransportClosed".to_string(), FALLBACK.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim_api::SimApi;
    use rsh_ipc::IpcServer;
    #[cfg(unix)]
    use std::sync::atomic::AtomicUsize;
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
        // H5: the server-side sim reports no firmware version.
        assert_eq!(client.firmware_version(), "Unknown");
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

    #[test]
    fn capture_ipc_error_keeps_the_daemon_reason() {
        // The daemon's `code|detail` payload, as the client formats
        // it (`"{code}: {message}"`, both halves carrying it).
        let detail = |error: &IpcError| capture_code_detail(error).1;
        let error = IpcError::Api(
            "NoLiveData|no live wheel data yet — connect the wheel: daemon API error: NoLiveData|no live wheel data yet — connect the wheel"
                .to_string(),
        );
        assert_eq!(detail(&error), "no live wheel data yet — connect the wheel");
        let bad_range = IpcError::Api(
            "BadRange|pedal 'accel' range 3000..=3300 mV spans only 300 mV: daemon API error: BadRange|pedal 'accel' range 3000..=3300 mV spans only 300 mV"
                .to_string(),
        );
        assert!(
            detail(&bad_range).contains("spans only 300 mV"),
            "the range reason must survive: {}",
            detail(&bad_range)
        );
        // A dead link (not a daemon error) is the reconnect reason,
        // never a silent empty string.
        assert_eq!(
            detail(&IpcError::Closed),
            "RSH isn't running — reconnect to capture."
        );
        assert_eq!(
            detail(&IpcError::Io("socket gone".to_string())),
            "RSH isn't running — reconnect to capture."
        );
    }

    /// Minimal `AppApi` for the capture test: only the capture
    /// methods answer (canned values); everything else is an unused
    /// one-liner (the trait's defaults cover the rest).
    /// Unix-only: the test that uses it binds a real Unix socket.
    #[cfg(unix)]
    struct CaptureStubApi;

    #[cfg(unix)]
    impl AppApi for CaptureStubApi {
        fn version(&self) -> String {
            "test".to_string()
        }

        fn connection(&self) -> rsh_api::ConnectionInfo {
            rsh_api::ConnectionInfo {
                state: ConnectionState::Idle,
                label: "Disconnected".to_string(),
                transport: ApiTransport::Wifi,
                address: String::new(),
                session_active: false,
                error: None,
                hint: None,
            }
        }

        fn connect(
            &self,
            _transport: ApiTransport,
        ) -> Result<rsh_api::ConnectionInfo, rsh_api::UserError> {
            Ok(self.connection())
        }

        fn disconnect(&self) -> Result<rsh_api::ConnectionInfo, rsh_api::UserError> {
            Ok(self.connection())
        }

        fn pairing_status(&self) -> rsh_api::PairingInfo {
            rsh_api::PairingInfo {
                seconds_left: 0,
                password: None,
                error: None,
                error_detail: None,
                board_mac: None,
                replacement_required: false,
                key_reset_armed: false,
            }
        }

        fn begin_pairing(&self) -> Result<rsh_api::PairingInfo, rsh_api::UserError> {
            Ok(self.pairing_status())
        }

        fn snapshot(&self) -> rsh_api::SnapshotInfo {
            rsh_api::SnapshotInfo::default()
        }

        fn output_enabled(&self) -> bool {
            false
        }

        fn set_output_enabled(&self, _enabled: bool) {}

        fn get_tuning(&self, module: &str) -> TuningSettings {
            TuningSettings::neutral(module)
        }

        fn set_tuning(
            &self,
            tuning: &TuningSettings,
        ) -> Result<TuningSettings, rsh_api::UserError> {
            Ok(tuning.clone())
        }

        fn draft_status(&self) -> Option<rsh_api::DraftStatus> {
            None
        }

        fn save_draft(&self) -> Result<bool, rsh_api::UserError> {
            Ok(false)
        }

        fn discard_draft(&self) -> bool {
            false
        }

        fn capture_centre(&self) -> Result<u16, rsh_api::UserError> {
            Ok(1812)
        }

        fn capture_pedal_min(&self, pedal: &str) -> Result<u16, rsh_api::UserError> {
            match pedal {
                "accel" => Ok(320),
                _ => Err(rsh_api::UserError::from_connection(
                    rsh_api::ConnectionError::Unknown,
                    format!("unknown pedal '{pedal}' (want accel, brake, clutch, or handbrake)"),
                )),
            }
        }

        fn capture_pedal_max(&self, pedal: &str) -> Result<u16, rsh_api::UserError> {
            match pedal {
                "accel" => Ok(3200),
                _ => Err(rsh_api::UserError::from_connection(
                    rsh_api::ConnectionError::Unknown,
                    format!("unknown pedal '{pedal}' (want accel, brake, clutch, or handbrake)"),
                )),
            }
        }
    }

    /// v2.0.2: capture calls go through this connection now (no
    /// second `CaptureLink`) — round-trip through a real socket,
    /// and a daemon-side failure arrives with its reason as the
    /// error title.
    #[test]
    #[cfg(unix)]
    fn capture_round_trips_over_real_socket() {
        // Serialised with the other socket tests: on Windows all
        // share the one global named pipe (see live_probe guard).
        let _guard = crate::live_probe::TEST_IPC_PIPE_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = std::env::temp_dir().join(format!("rsh-capture-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let socket = rsh_ipc::socket_path(&dir);
        let api: Arc<dyn AppApi> = Arc::new(CaptureStubApi);
        let (tx, _rx) = broadcast::channel(64);
        let server = Arc::new(IpcServer::new(api.clone(), tx));
        let socket_for_thread = socket.clone();
        // The server thread is intentionally never joined: it serves
        // until the test process exits.
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("server runtime");
            rt.block_on(async move {
                let listener = rsh_ipc::bind_unix_socket(&socket_for_thread).expect("bind socket");
                rsh_ipc::serve_listener(server, listener).await;
            });
        });
        for _ in 0..100 {
            if socket.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(socket.exists(), "server did not bind its socket");
        let client = IpcApi::connect(&socket).expect("connect");
        assert!(client.ping());
        assert_eq!(client.capture_centre().expect("capture centre"), 1812);
        assert_eq!(client.capture_pedal_min("accel").expect("capture min"), 320);
        assert_eq!(
            client.capture_pedal_max("accel").expect("capture max"),
            3200
        );
        let error = client
            .capture_pedal_min("gear")
            .expect_err("unknown pedal must fail");
        assert!(
            error.title.contains("unknown pedal 'gear'"),
            "the daemon's reason must survive IPC: {}",
            error.title
        );
        drop(client);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// B2/B3: the poller fills the connection + snapshot cache
    /// (the UI never calls the daemon for them), and
    /// [`IpcApi::clear_cached_state`] empties it again — no stale
    /// values survive a daemon death.
    #[test]
    #[cfg(unix)]
    fn poller_caches_live_values_and_clear_empties_them() {
        let _guard = crate::live_probe::TEST_IPC_PIPE_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = std::env::temp_dir().join(format!("rsh-poller-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let sock = dir.join("rsh-daemon.sock");
        let api: Arc<dyn AppApi> = Arc::new(SimApi::new());
        let (tx, _rx) = broadcast::channel(64);
        let server = Arc::new(IpcServer::new(api, tx));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
        let sock_clone = sock.clone();
        let handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("server runtime");
            rt.block_on(async move {
                let listener = rsh_ipc::bind_unix_socket(&sock_clone).expect("bind");
                ready_tx.send(()).expect("ready signal");
                if let Ok((stream, _)) = listener.accept().await {
                    server.serve_stream(stream).await;
                }
            });
        });
        ready_rx.recv().expect("server bound");
        let client = IpcApi::connect(&sock).expect("connect");
        // The poller fills the cache within a few rounds (50 ms).
        // The server-side sim never advances, so its snapshot is
        // deterministic: steering 0°, throttle 0.5.
        let mut label = String::new();
        for _ in 0..100 {
            label = client.connection().label;
            if label == "Connected" {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(label, "Connected", "poller never filled the cache");
        assert_eq!(client.snapshot().throttle, 0.5);
        // Clear + assert at once: the poller repopulates within
        // ~50 ms, so a repopulated round is retried, not flaked on.
        for _ in 0..5 {
            client.clear_cached_state();
            if client.connection().label == "Connection lost"
                && client.snapshot() == SnapshotInfo::default()
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(60));
        }
        assert_eq!(client.connection().label, "Connection lost");
        assert_eq!(client.snapshot(), SnapshotInfo::default());
        drop(client);
        handle.join().expect("server thread");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Counting `AppApi` for the L-4 poller test: delegates to a
    /// `SimApi` and counts how many times the server dispatches
    /// each hot method. `poll()` reads the inner sim directly (one
    /// dispatch, no `snapshot` / `connection` / `output_enabled`
    /// dispatch inside it) — the same single-read pattern as the
    /// daemon's override.
    /// Unix-only: the test that uses it binds a real Unix socket.
    #[cfg(unix)]
    struct CountingApi {
        inner: SimApi,
        poll_calls: AtomicUsize,
        snapshot_calls: AtomicUsize,
        connection_calls: AtomicUsize,
        output_calls: AtomicUsize,
    }

    #[cfg(unix)]
    impl CountingApi {
        fn new() -> Self {
            Self {
                inner: SimApi::new(),
                poll_calls: AtomicUsize::new(0),
                snapshot_calls: AtomicUsize::new(0),
                connection_calls: AtomicUsize::new(0),
                output_calls: AtomicUsize::new(0),
            }
        }
    }

    #[cfg(unix)]
    impl AppApi for CountingApi {
        fn version(&self) -> String {
            self.inner.version()
        }

        fn connection(&self) -> ConnectionInfo {
            self.connection_calls.fetch_add(1, Ordering::Relaxed);
            self.inner.connection()
        }

        fn connect(&self, transport: ApiTransport) -> Result<ConnectionInfo, UserError> {
            self.inner.connect(transport)
        }

        fn disconnect(&self) -> Result<ConnectionInfo, UserError> {
            self.inner.disconnect()
        }

        fn pairing_status(&self) -> PairingInfo {
            self.inner.pairing_status()
        }

        fn begin_pairing(&self) -> Result<PairingInfo, UserError> {
            self.inner.begin_pairing()
        }

        fn snapshot(&self) -> SnapshotInfo {
            self.snapshot_calls.fetch_add(1, Ordering::Relaxed);
            self.inner.snapshot()
        }

        fn output_enabled(&self) -> bool {
            self.output_calls.fetch_add(1, Ordering::Relaxed);
            self.inner.output_enabled()
        }

        fn set_output_enabled(&self, enabled: bool) {
            self.inner.set_output_enabled(enabled);
        }

        fn get_tuning(&self, module: &str) -> TuningSettings {
            self.inner.get_tuning(module)
        }

        fn set_tuning(&self, tuning: &TuningSettings) -> Result<TuningSettings, UserError> {
            self.inner.set_tuning(tuning)
        }

        fn draft_status(&self) -> Option<DraftStatus> {
            self.inner.draft_status()
        }

        fn save_draft(&self) -> Result<bool, UserError> {
            self.inner.save_draft()
        }

        fn discard_draft(&self) -> bool {
            self.inner.discard_draft()
        }

        fn poll(&self) -> rsh_api::PollState {
            self.poll_calls.fetch_add(1, Ordering::Relaxed);
            rsh_api::PollState {
                snapshot: self.inner.snapshot(),
                connection: self.inner.connection(),
                output_enabled: self.inner.output_enabled(),
            }
        }
    }

    /// L-4 (v2.0.2): the poller does exactly ONE round-trip per
    /// cycle — the combined `poll` — never the three old separate
    /// calls. Proved by counting server-side dispatches while the
    /// real `IpcApi` poller runs against a real socket: after ≥ 2
    /// poll cycles, `poll` was dispatched ≥ 2 times and
    /// `snapshot` / `connection` / `output_enabled` were dispatched
    /// 0 times, while the cache still holds live values.
    #[test]
    #[cfg(unix)]
    fn poller_uses_one_combined_poll_per_cycle() {
        let _guard = crate::live_probe::TEST_IPC_PIPE_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = std::env::temp_dir().join(format!("rsh-poll-count-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let sock = dir.join("rsh-daemon.sock");
        let counting = Arc::new(CountingApi::new());
        let api: Arc<dyn AppApi> = counting.clone();
        let (tx, _rx) = broadcast::channel(64);
        let server = Arc::new(IpcServer::new(api, tx));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
        let sock_clone = sock.clone();
        let handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("server runtime");
            rt.block_on(async move {
                let listener = rsh_ipc::bind_unix_socket(&sock_clone).expect("bind");
                ready_tx.send(()).expect("ready signal");
                if let Ok((stream, _)) = listener.accept().await {
                    server.serve_stream(stream).await;
                }
            });
        });
        ready_rx.recv().expect("server bound");
        let client = IpcApi::connect(&sock).expect("connect");
        // The poller runs a cycle every ~50 ms; wait for ≥ 2.
        let mut polls = 0;
        for _ in 0..100 {
            polls = counting.poll_calls.load(Ordering::Relaxed);
            if polls >= 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(polls >= 2, "poller never ran a poll cycle");
        assert_eq!(
            counting.snapshot_calls.load(Ordering::Relaxed),
            0,
            "poller must not dispatch snapshot any more"
        );
        assert_eq!(
            counting.connection_calls.load(Ordering::Relaxed),
            0,
            "poller must not dispatch connection any more"
        );
        assert_eq!(
            counting.output_calls.load(Ordering::Relaxed),
            0,
            "poller must not dispatch output_enabled any more"
        );
        // The cache still got live values — from the one poll.
        assert_eq!(client.connection().label, "Connected");
        assert_eq!(client.snapshot().throttle, 0.5);
        assert!(client.output_enabled());
        drop(client);
        handle.join().expect("server thread");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
