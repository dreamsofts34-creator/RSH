//! Fuzz the IPC JSON-lines wire format (R3-17).
//!
//! The other fuzz targets cover the board protocol / config path;
//! none covered the IPC wire format: `parse_request` version
//! checking, the server's `handle_line` error mapping, or the
//! client's structure-based event/response classification. The
//! DS-35 misclassification bug (a `"event"` substring inside a
//! string value silently dropping a response) is exactly the class
//! a line-fuzzer catches — and it was found by hand, not by fuzz.
//!
//! The target feeds arbitrary lines to `parse_request` +
//! `handle_line` (over a stub [`AppApi`]) and to the client's
//! frame classifier, asserting:
//!   * nothing panics on any input;
//!   * a well-formed request always gets a response with the same
//!     id (never a silent drop);
//!   * a well-formed RESPONSE never classifies as an event frame,
//!     whatever its string values hold (DS-35).
//!
//! CI's `fuzz-quick` job picks this target up automatically — it
//! loops over `fuzz/fuzz_targets/*.rs`, 60 s per target, so no
//! workflow change was needed.

#![no_main]
use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use rsh_api::{
    ApiTransport, AppApi, ConnectionError, ConnectionInfo, ConnectionState, SnapshotInfo,
    TuningSettings, UserError, WifiStatus, validate_tuning,
};
use rsh_ipc::{IpcServer, WireFrameKind, classify_wire_line, parse_request};

/// Minimal stub API: every required method answers with canned
/// values, so the fuzzed surface is the wire format (parsing,
/// version check, error mapping, classification) — not the API.
struct StubApi;

impl AppApi for StubApi {
    fn version(&self) -> String {
        "2.0.6".to_string()
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

    fn wifi_status(&self) -> WifiStatus {
        WifiStatus {
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
        false
    }

    fn set_output_enabled(&self, _enabled: bool) {}

    fn get_tuning(&self, module: &str) -> TuningSettings {
        TuningSettings::neutral(module)
    }

    fn set_tuning(&self, tuning: &TuningSettings) -> Result<TuningSettings, UserError> {
        let errors = validate_tuning(tuning);
        if errors.is_empty() {
            Ok(tuning.clone())
        } else {
            Err(UserError::from_connection(
                ConnectionError::Unknown,
                format!("invalid tuning: {}", errors.join("; ")),
            ))
        }
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

fn server() -> IpcServer {
    let (events, _rx) = tokio::sync::broadcast::channel(64);
    IpcServer::new(Arc::new(StubApi), events)
}

fuzz_target!(|data: &[u8]| {
    let line = String::from_utf8_lossy(data);

    // 1. parse_request must never panic on any line.
    let parsed = parse_request(&line);

    // 2. handle_line must never panic, and a well-formed request
    // always gets a response with the same id — never a silent
    // drop. (An Err here is the "unparseable line" path, which the
    // connection loop drops by design — but it must be an Err,
    // not a panic.) Nested `if let`s, not a let-chain: the fuzz
    // crate is edition 2021, and let-chains need Rust 2024.
    let server = server();
    if let Ok(response) = server.handle_line(&line) {
        if let Ok(request) = parsed {
            assert_eq!(
                response.id, request.id,
                "response id must echo the request id"
            );
            assert_eq!(response.v, rsh_ipc::IPC_PROTOCOL_VERSION);
            if request.method == "ping" {
                assert!(response.ok, "ping must succeed against the stub API");
            }
        }
    }

    // 3. The client frame classifier must never panic, and a
    // well-formed RESPONSE must never classify as an event — the
    // DS-35 regression (a string value containing "event"
    // silently dropping the response).
    let kind = classify_wire_line(&line);
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) {
        if value.get("event").is_none() {
            if serde_json::from_str::<rsh_ipc::IpcResponse>(&line).is_ok() {
                assert_eq!(
                    kind,
                    WireFrameKind::Response,
                    "well-formed response classified as event: {line}"
                );
            }
        } else {
            assert_eq!(kind, WireFrameKind::Event);
        }
    }
});
