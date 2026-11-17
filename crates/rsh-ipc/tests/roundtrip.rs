//! IPC round-trip test: real Unix socket, real server task, real
//! client — `ping` + `version` + `connection` + an event push
//! delivered to a subscriber. (The Windows named-pipe path is the
//! same dispatch over a different listener; it is verified on
//! Windows — see the gate doc.)
//!
//! Unix-only: the crate doc stays above the `cfg` so the crate is
//! documented even where the tests are compiled out (Windows).
#![cfg(unix)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use rsh_api::{
    ApiTransport, AppApi, AppEvent, ConnectionInfo, DraftStatus, PairingInfo, SlowPollState,
    SnapshotInfo, TuningSettings, UserError, connection::ConnectionState, validate_tuning,
};
use rsh_ipc::{IpcClient, IpcServer, bind_unix_socket, serve_listener};
use tokio::sync::broadcast;

struct StubApi {
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
        "1.0.2".to_string()
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
            key_reset_armed: false,
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

/// L4 tuning round-trip over the socket: set → get → draft → save
/// → clean. Split out of the main round-trip test so it stays under
/// the 100-line clippy limit.
async fn assert_tuning_round_trip(client: &IpcClient) {
    let stored = client
        .call(
            "set_tuning",
            serde_json::json!({"module":"steering","deadzone":5.0,"curve":1.6,"minMv":0.0,"maxMv":3300.0,"smoothing":10.0}),
        )
        .await
        .expect("set_tuning");
    assert_eq!(
        stored.get("deadzone").and_then(serde_json::Value::as_f64),
        Some(5.0)
    );
    let got = client
        .call("get_tuning", serde_json::json!({"module": "steering"}))
        .await
        .expect("get_tuning");
    assert_eq!(
        got.get("curve").and_then(serde_json::Value::as_f64),
        Some(1.6)
    );
    let status = client
        .call("draft_status", serde_json::json!({}))
        .await
        .expect("draft_status");
    assert_eq!(
        status.get("dirty").and_then(serde_json::Value::as_bool),
        Some(true)
    );
    let saved = client
        .call("save_draft", serde_json::json!({}))
        .await
        .expect("save_draft");
    assert_eq!(
        saved.get("saved").and_then(serde_json::Value::as_bool),
        Some(true)
    );
    let status = client
        .call("draft_status", serde_json::json!({}))
        .await
        .expect("draft_status after save");
    assert!(status.is_null(), "draft clean after save, got {status}");
}

#[tokio::test]
async fn ipc_round_trip_ping_version_connection_and_event() {
    let dir = std::env::temp_dir().join(format!("rsh-ipc-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("test dir");
    let socket = dir.join("rsh-daemon.sock");
    let api: Arc<dyn AppApi> = Arc::new(StubApi {
        output: Mutex::new(false),
        tunings: Mutex::new(BTreeMap::new()),
        dirty: Mutex::new(BTreeSet::new()),
    });
    let (event_tx, _rx) = broadcast::channel(64);
    let server = Arc::new(IpcServer::new(Arc::clone(&api), event_tx.clone()));
    let listener = bind_unix_socket(&socket).expect("bind test socket");
    // The socket file must be current-user-only (0600).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&socket)
            .expect("socket meta")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "IPC socket must be 0600, got {mode:o}");
    }
    let serve = tokio::spawn(serve_listener(Arc::clone(&server), listener));

    let client = IpcClient::connect_unix(&socket)
        .await
        .expect("client connects");
    assert!(client.ping().await.expect("ping"));
    assert_eq!(client.version().await.expect("version"), "1.0.2");
    let conn = client
        .call("connection", serde_json::json!({}))
        .await
        .expect("connection");
    assert_eq!(
        conn.get("label").and_then(serde_json::Value::as_str),
        Some("Connected")
    );
    assert_eq!(
        conn.get("session_active")
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );

    // DS-29: one combined slow read over the same socket — the
    // whole slow poller payload in a single round-trip.
    let slow_value = client
        .call("slow_poll", serde_json::json!({}))
        .await
        .expect("slow_poll");
    let slow: SlowPollState =
        serde_json::from_value(slow_value).expect("slow_poll parses as SlowPollState");
    assert_eq!(slow.version, "1.0.2");
    assert_eq!(slow.firmware_version, "Unknown");
    assert_eq!(slow.pairing_status.seconds_left, 0);
    assert_eq!(slow.tunings.len(), 8);
    assert_eq!(slow.modules.len(), 4);
    assert!(slow.modules.values().all(|enabled| *enabled));
    assert!(slow.draft.is_none());

    // Subscribe, then push an event from the server side: the
    // client must receive it on its event channel.
    client
        .call("subscribe", serde_json::json!({}))
        .await
        .expect("subscribe");
    event_tx
        .send(AppEvent::ConnectionChanged {
            state: ConnectionState::Active,
            label: "Connected".to_string(),
            reason: None,
        })
        .expect("event publishes");
    let mut got = None;
    for _ in 0..50 {
        if let Some(event) = client.try_recv_event() {
            got = Some(event);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        matches!(got, Some(AppEvent::ConnectionChanged { .. })),
        "subscriber received the pushed event, got {got:?}"
    );

    // L4 tuning round-trip over the same socket: set → get → draft
    // → save → clean.
    assert_tuning_round_trip(&client).await;

    serve.abort();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn ipc_connect_fails_when_daemon_not_running() {
    let missing = std::env::temp_dir().join("rsh-ipc-test-missing.sock");
    let _ = std::fs::remove_file(&missing);
    let Err(err) = IpcClient::connect_unix(&missing).await else {
        panic!("no daemon, no connect");
    };
    assert!(matches!(err, rsh_ipc::IpcError::Io(_)), "got {err:?}");
}
