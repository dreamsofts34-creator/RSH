//! App state (Phase G0 — the Phase G entry item, SW-R6.1).
//!
//! [`AppStateCore`] is the testable half of the Tauri app state: it owns
//! the engine metrics + [`LoopRateTracker`](crate::snapshot::LoopRateTracker)
//! + [`EncoderState`](crate::snapshot::EncoderState), the installed output
//! backend (a boxed [`IJoystickOutput`](rsh_output::IJoystickOutput)), the
//! game launcher, and the profile store. The Tauri shell
//! (`src-tauri/src/app_state.rs`) wraps this in the Tauri-managed
//! `AppState`, registers it with `.manage()`, and owns the snapshot
//! emitter thread; all logic and tests live here, because this crate
//! compiles in the Linux sandbox and `src-tauri` does not.
//!
//! # The installed backend
//!
//! The slot holds G3's [`IJoystickOutput`](rsh_output::IJoystickOutput)
//! trait object (`try_connect` / `try_connect_with_plan` / `disconnect` /
//! `update` / `send_neutral`, all `Result`-based;
//! `ProcessedState = rsh_engine::pipeline::OutputState`). The G4
//! selector swaps the backend in this slot at runtime — vJoy
//! (default, G3), `HIDMaestro` (explicit choice, G2), or the [`NullSink`](crate::system::NullSink)
//! (no device) — via `AppStateCore::set_output_backend`. At launch the
//! shell calls [`AppStateCore::activate_startup_backend`], which
//! activates the persisted `AppSettings.output_backend` choice through
//! the same selector — except an elevated backend (`HIDMaestro`), which
//! is NEVER activated at startup (hotfix v1.0.1: its `runas` launch
//! would pop a UAC prompt on every launch). A persisted `HIDMaestro`
//! choice is deferred instead: the [`NullSink`] stays installed, the
//! outcome records `active: None` with the reason, and the user
//! activates it by enabling output or picking it on the Output page
//! (user-initiated, so the UAC prompt there is expected). The
//! [`NullSink`] is only the fallback base when activation fails, and
//! the failure reason is recorded in the backend detail and in the
//! startup outcome — never silently. Until activation runs the installed backend is the
//! [`NullSink`](crate::system::NullSink) — the no-device sink — speaking the trait via the
//! [`IJoystickOutput` implementation below](NullSink): `try_connect`
//! fails honestly (no driver to acquire), `update` fails as not
//! connected, and `send_neutral` is the trait's disconnected no-op, so
//! every send is attempted and honestly reported as not delivered
//! (D-F3) — never claimed as sent. "Sent" is only ever reported when
//! the backend was connected (`is_connected`) and the call succeeded:
//! `send_neutral` returning `Ok` while disconnected means "nothing to
//! send", not "neutral delivered".
//!
//! # The engine task (Phase G gate wiring)
//!
//! [`AppStateCore::start_engine`] spawns the engine supervisor thread,
//! which runs one [`WheelEngine`](rsh_engine::engine::WheelEngine) task
//! (200 Hz, 250 ms watchdog → NEUTRAL) on a current-thread tokio
//! runtime. The engine's source is a fixed channel-backed
//! [`PacketSource`](rsh_engine::engine::PacketSource): transports feed
//! frames into the channel, so the engine task itself never restarts
//! when a transport connects, drops, or reconnects. With no transport
//! connected the engine still exists and ticks in a clean idle state
//! (no frames, watchdog stale → neutral, nothing faked). The engine's
//! output sink is the state itself: every tick feeds
//! [`AppStateCore::ingest_frame`] (snapshot + loop-rate) and the
//! installed backend's `update`.
//!
//! Honest gaps in this wiring (not stubs — the behaviour is what it
//! says, the missing pieces are named):
//! - **Live engine metrics are measured per tick.** The engine hands
//!   its live [`EngineMetrics`](rsh_engine::metrics::EngineMetrics) to
//!   the sink on every tick ([`OutputSink::send_with_metrics`](rsh_engine::engine::OutputSink::send_with_metrics),
//!   H1 condition 11), so the sink feeds [`ingest_frame`](AppStateCore::ingest_frame)
//!   with real metrics: jitter / output-latency / E2E p99s are `Some`
//!   once measured (still `null` while unmeasured — never a fake 0)
//!   and `seqGap` / `txFail` come from the status pump below. The loop
//!   rate is measured too (one tick per sink send, ~200 Hz).
//! - **Status packets are forwarded.** The forwarder drains the
//!   transport's status channel after every stream frame and pumps
//!   the packets into the engine's `status_sender` (H1 condition 12),
//!   so `btnsEx` and the board loss counters reach the engine. While
//!   no stream frame arrives the pump waits with it (status only
//!   moves while frames flow); the engine still drains its own
//!   status channel on every tick.
//! - **The HMAC session gate is enforced (SEC-E-14, H1 condition 13).**
//!   Stream frames are HELD at the forwarder until the session gate
//!   is open: the gate opens only when a `BoardSession` has completed
//!   its unsigned `GET_CAPS` and the session
//!   owner calls [`AppStateCore::open_session_gate`]. The gate closes
//!   again on transport loss or disconnect — a new transport is a new
//!   session and must reconnect. The session owner is the
//!   **activator (H5)**: [`activate_session_on`] runs inside the
//!   forwarder, on the forwarder's own transport, before the frame
//!   loop — it sends a flag-1 HELO, runs `BoardSession::connect`
//!   (unsigned `GET_CAPS` first), opens
//!   the gate on success, and hands the transport back via
//!   [`BoardSession::take_transport`](rsh_transport::session::BoardSession::take_transport)
//!   so the same transport streams frames. A failed activation leaves
//!   the gate exactly as it was (closed in production) and records
//!   the reason — never silently opened. The board enforces the same
//!   rule (`sessionAuthenticated` in firmware v4.0.1 `net.cpp`), so
//!   this is defence in depth against a rogue board streaming
//!   without auth.
//! - **The pipeline runs the live plan (L4).** `set_tuning` builds a
//!   [`PipelinePlan`] from the draft profile and pushes it to the
//!   running engine over a `watch` channel (latest-wins, applied at
//!   the next tick without resetting filter/ramp state); the engine
//!   starts on the stored profile's plan. Before L4 the engine ran
//!   [`PipelinePlan::default`](rsh_engine::pipeline::PipelinePlan::default)
//!   forever (H1 condition 14).
//!
//! # What is NOT here (honest gaps, not stubs)
//!
//! - **Live control commands run under a fresh session (H5-control).**
//!   [`run_control_session`] opens a fresh session the same way the
//!   activator does (flag-1 HELO → fresh nonce → signed `GET_CAPS`
//!   first, fail-closed), runs one control operation, then closes
//!   the session and zeroizes the key. The 11 remaining commands
//!   (`set_password`, config staging, pin-map get/set, firmware
//!   upload, `restart_wheel`) are thin sync wrappers over it — logic
//!   lives in the crates, never in `src-tauri`.
//!
//! # Honest gaps that remain (not stubs — the behaviour is what it
//! # says, the missing pieces are named)
//!
//! - **Serial control while streaming needs disconnect-first.** A
//!   control command over serial refuses while the live transport
//!   holds the port (a second open fails on Windows and would split
//!   reads between two readers on Linux) — the user disconnects the
//!   transport first, then retries. UDP control works alongside the
//!   live forwarder (ephemeral local port; the stream is fixed to
//!   port 4211 and unauthenticated, so the fresh session cannot
//!   disturb it).
//! - **Arbitrary button→action rebinding is not a board feature.**
//!   The board stores only the three fixed `BUTTON` slot roles
//!   (1 shifter-up, 2 shifter-down, 3 horn); `stage_config` rejects a
//!   cross-role rebind loudly instead of pretending to stage it. The
//!   PC-side binding store + output remap landed in v2.0.2 Round 2B
//!   ([`BindingStore`](rsh_profiles::bindings::BindingStore),
//!   [`AppStateCore::apply_bindings`]) — bindings live in
//!   `bindings.json` and are applied to each engine tick before the
//!   backend sees it.
//! - **No re-activation retry after a reboot.** `restart_wheel`
//!   reboots the board and closes the gate; the user reconnects once
//!   the board is back (the forwarder has no re-activation retry —
//!   a pre-existing H5-activator limitation).
//! - **Pin map get/set** reads/writes the board's config blob (there
//!   is no local pin-map store), so both need a live board session;
//!   with no board they fail honestly instead of returning a guess.

use std::collections::{BTreeMap, VecDeque};
use std::fmt::Write as _;
use std::future::Future;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver as StdReceiver, Sender as StdSender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant, SystemTime};

use sha2::{Digest, Sha256};
#[cfg(windows)]
use std::os::windows::process::CommandExt;

use rsh_api::ButtonBinding;
use rsh_api::RawMv;
use rsh_api::connection::{HELLO_ATTEMPTS, HELLO_WAIT};
use rsh_api::errors::ConnectionError;
use rsh_api::tuning::{snap_steering_lock_deg, steering_lock_deg_to_scale};
use rsh_api::{FirmwareCheck, FirmwareUpdateStatus};
use rsh_engine::calibration::{CalibrationError, MIN_SPAN_MV, validate_pedal_range};
use rsh_engine::engine::{
    EngineReport, OutputSink, PacketSource, SinkError, StopHandle, WheelEngine,
};
use rsh_engine::metrics::EngineMetrics;
use rsh_engine::pipeline::OutputState as PipelineOutputState;
use rsh_engine::pipeline::{PipelinePlan, ProcessingPipeline};
use rsh_output::selector::{BackendFactories, BackendKind, SwitchOutcome, switch_backend};
use rsh_output::vjoy::{VJOY_DOWNLOAD_URL, VJoyOutput, resolve_dll_path};
use rsh_output::{IJoystickOutput, ProcessedState, VJoyError};
use rsh_output_hidmaestro::HidMaestroOutput;
use rsh_output_hidmaestro::backend::UAC_NOTE;
use rsh_output_hidmaestro::manager::resolve_sidecar_path;
use rsh_profiles::bindings::{BindingError, BindingStore, is_app_action};
use rsh_profiles::csharp_import::{
    GameId, PedalCalibration, PerModuleTuning, SteeringCalibration, WheelProfile,
    canonical_module_id,
};
use rsh_profiles::resolver::{
    CarProfileResolver, PROFILE_SWITCHED_EVENT, ProfileId, ProfileSource, ProfileSwitchEvent,
};
use rsh_profiles::store::{
    BackupGeneration, BackupInfo, ProfileStore, format_system_time_rfc3339, game_id_from_slug,
    validate_profile_name,
};
use rsh_profiles::watcher::{
    CarSource, ForegroundDetector, TelemetryWatcher, WindowsForegroundDetector,
};
use rsh_protocol::config::ConfigBlob;
use rsh_protocol::control::LedTestPattern;
use rsh_protocol::stream::MAX_AXES;
use rsh_protocol::stream::MAX_ENCODERS;
use rsh_protocol::stream::StatusPacket;
use rsh_protocol::stream::StreamFrame;
use rsh_protocol::{GetCapsReply, HelloReply};
use rsh_security::firmware_verify::verify_firmware_image;
use rsh_security::keystore::{KeyStore, KeystoreError};
use rsh_transport::ControlTransport;
use rsh_transport::config::{ConfigState, ConfigTransaction, ConfigTxnSnapshot};
use rsh_transport::discovery::{DEFAULT_BOARD_IP, Discovery};
use rsh_transport::port_conflict::{PortHolder, diagnose_udp_port_holder};
use rsh_transport::serial::{
    SerialConfig, SerialTransport, detect_board_port, detect_board_port_metadata_only,
};
use rsh_transport::session::{BoardSession, KeyStoreLike, SessionError};
use rsh_transport::udp::{HELO_PORT, UdpBindError, UdpConfig, UdpTransport};
use serde::{Deserialize, Serialize};

use crate::commands::{
    BackendStatus, LedPattern, OutputState as CommandOutputState, PinAssignment, PinMapValidation,
    ShifterGate, TuningSettings, config_blob_to_json, config_json_to_blob,
    firmware_validation_user_error, get_snapshot as build_snapshot_command,
    merge_pin_map_into_blob, records_to_pin_assignments, validate_firmware_image_against,
    validate_pin_map, validate_tuning,
};
use crate::diagnostics::{redact_secrets, utc_now_iso};
use crate::draft::{DraftStatus, ProfileDraft};
use crate::firmware_image;
use crate::launcher::{GameLauncher, SystemSpawner};
use crate::manual_games::ManualGameStore;
use crate::snapshot::{
    ConnectionState, EncoderState, EngineSnapshot, EngineStats, LoopRateTracker, PedalSnapshot,
    SnapshotInput, build_engine_stats,
};
use crate::system::{NullSink, ShutdownOutcome};
use crate::telemetry_adapter::TelemetryCarSource;
use crate::webota::upload_firmware_image;
use crate::webota::{WebOtaTarget, flash_firmware, wait_for_board_version};

/// App settings, re-exported for the Tauri shell (`src-tauri` does not
/// depend on `rsh-profiles` directly).
pub use rsh_profiles::csharp_import::{AppSettings, AppSettingsPatch, is_allowed_board_ip};
/// Profile list-row summaries, re-exported for the Tauri shell.
pub use rsh_profiles::store::ProfileSummary;

/// Locks a mutex, recovering the guard if a previous holder panicked
/// while holding it. The critical sections in this module are short
/// and contain no panicking code (except a backend's `send`, which is
/// foreign code), so poisoning means "a backend panicked mid-send":
/// the state survives that instead of turning one backend panic into
/// a panic in every later command.
///
/// M-12 audit (deep-scan Round 2, 2026-11-14): every `Mutex` in this
/// module is a `std::sync::Mutex`, and NO guard is ever held across
/// an `.await`. All 17 async fns were checked: only 4 `lock()` sites
/// sit inside an async fn (`engine_supervisor_run` × 2,
/// `activate_session_on` × 2), and each is either a statement-local
/// temporary (dropped at the `;`) or — the one `let`-bound guard in
/// `activate_session_on` — scoped inside a block that ends before
/// the following `.await`. Every other `lock()` site is in a sync
/// fn, where no `.await` exists. No `tokio::sync::Mutex` migration
/// was needed; re-run this audit if an async fn gains a `lock()`
/// call (the `let`-bound-guard-then-`.await` shape is the one that
/// would block the executor).
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The existing [`NullSink`](crate::system::NullSink) as an
/// [`IJoystickOutput`]: the no-device backend installed until a G2/G3
/// backend replaces this slot.
///
/// `NullSink` itself is the engine's [`OutputSink`](rsh_engine::engine::OutputSink)
/// (defined in `crate::system`, local to this crate, so this impl is
/// legal here); this impl is its voice on the output trait. It never
/// claims a device: `try_connect` fails with the honest reason (no
/// backend installed), `update` fails as not connected, and
/// `send_neutral` is the trait's disconnected no-op (`Ok`, meaning
/// "nothing to send" — callers must check `is_connected` before
/// reporting a neutral send as delivered; see
/// [`AppStateCore::shutdown`]).
impl IJoystickOutput for NullSink {
    fn name(&self) -> &'static str {
        "NullSink"
    }

    fn is_connected(&self) -> bool {
        false
    }

    fn try_connect(&mut self) -> Result<(), VJoyError> {
        Err(VJoyError::DriverNotFound(
            "no output backend installed yet — sends are attempted and reported as not delivered (Phase G2/G3 install the real backend)".to_string(),
        ))
    }

    fn disconnect(&mut self) {}

    fn update(&mut self, _state: &ProcessedState) -> Result<(), VJoyError> {
        Err(VJoyError::NotConnected)
    }

    fn send_neutral(&mut self) -> Result<(), VJoyError> {
        Ok(())
    }
}

/// Event name for the 30 Hz engine snapshot feed (UI plan §6.1).
pub const SNAPSHOT_EVENT: &str = "rsh://snapshot";

/// Event name for button-press highlight events (target < 50 ms).
pub const BUTTON_PRESS_EVENT: &str = "rsh://button-press";

/// Event name for game launch/stop state changes (C-EVT-1).
pub const GAME_STATE_EVENT: &str = "rsh://game-state";

/// Event name for config rollback completions (C-EVT-1).
pub const CONFIG_ROLLBACK_EVENT: &str = "rsh://config-rollback";

/// Snapshot emission interval: 30 Hz ≈ 33 ms between emissions.
pub const SNAPSHOT_EMIT_INTERVAL: Duration = Duration::from_millis(SNAPSHOT_EMIT_INTERVAL_MS);

/// Snapshot emission interval in milliseconds (30 Hz).
pub const SNAPSHOT_EMIT_INTERVAL_MS: u64 = 33;

/// One button-press event, as the frontend's `subscribeButtonPress`
/// handler consumes it (`{ button, pressed }`).
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ButtonPressEvent {
    /// Button number: 1–32 physical, 33–64 extended, 65–72 encoder virtual.
    pub button: u8,
    /// True on press, false on release. Encoder virtual buttons are
    /// momentary pulses: only the `pressed: true` edge is emitted.
    pub pressed: bool,
}

/// One queued app action from a bound button press (v2.0.2,
/// Round 2B): the daemon manager loop drains these and runs them —
/// they never reach the output backend. Game-input bindings are
/// applied inline by [`AppStateCore::apply_bindings`] instead; only
/// app actions need a queue, because they run outside the engine
/// tick (profile activation saves + pushes a pipeline plan).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindingAppAction {
    /// Cycle to the next profile.
    ProfileNext,
    /// Cycle to the previous profile.
    ProfilePrevious,
}

/// One manually added game, as the frontend's `GameInfo` shape consumes it.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GameInfo {
    /// Game id (the manual entry's slug id, e.g. `"my-racing-game"`).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Id of the profile bound to this game, if any.
    pub bound_profile_id: Option<String>,
    /// True when this game is the currently tracked running game.
    pub running: bool,
    /// True when the game exe was found on disk.
    pub installed: bool,
}

/// Calibration, as the frontend's `Calibration` shape consumes it.
///
/// Steering carries only its centre here (the frontend shape has no
/// steering min/max); pedal entries carry min/max per pedal
/// (`"accel"`, `"brake"`, `"clutch"` keys). Values are millivolts.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Calibration {
    /// Steering centre in mV.
    pub steering_center_mv: f64,
    /// Pedal minimums in mV, keyed by pedal (`"accel"`, `"brake"`, `"clutch"`).
    pub pedal_min_mv: BTreeMap<String, f64>,
    /// Pedal maximums in mV, keyed by pedal.
    pub pedal_max_mv: BTreeMap<String, f64>,
    /// When the calibration was captured (RFC 3339, L4), if any
    /// capture was ever saved. [`None`] = never calibrated — the
    /// Overview "Pedals calibrated" check must show "not yet", never
    /// a fake date.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calibrated_at: Option<String>,
}

/// The background vJoy install's progress, as
/// [`AppStateCore::vjoy_status`] reports it (v2.0.1 hotfix
/// BACKEND-2). The install runs on its own thread
/// ([`AppStateCore::install_vjoy`]); this is the only shared state
/// between that thread and the status reader.
#[derive(Clone, Debug, Default)]
struct VjoyInstallState {
    /// True while the download / install thread is running.
    installing: bool,
    /// The last install failure, when the last install failed.
    error: Option<String>,
}

/// Cached vJoy DLL probe (R2-20): [`AppStateCore::vjoy_status`]
/// called `resolve_dll_path()` twice per call — a registry read
/// plus two `ProgramFiles` file probes on Windows — and `slow_poll`
/// calls it ~1 Hz. The DLL only appears / disappears when the
/// driver is installed / uninstalled, so the probe result is cached
/// and re-run only when it is invalidated (install completion,
/// backend switch) or when [`VJOY_DLL_CACHE_TTL`] has passed (a
/// driver installed by hand while the daemon runs still shows up
/// within a minute, without a restart).
#[derive(Default)]
struct VjoyDllCache {
    /// When the cached probe ran ([`None`] = never probed, or
    /// invalidated — the next read re-probes immediately).
    probed_at: Option<Instant>,
    /// The probe result: the resolved DLL path, or [`None`] when
    /// the driver was not found.
    dll: Option<PathBuf>,
}

/// Time after which the cached vJoy DLL probe is re-run (R2-20):
/// long enough that the ~1 Hz slow poll never probes, short enough
/// that a hand-installed driver shows up without a restart.
const VJOY_DLL_CACHE_TTL: Duration = Duration::from_secs(60);

/// Cached Wi-Fi password-set flag (R3-D-08): [`AppStateCore::wifi_status`]
/// did one Credential Manager read per call, and the daemon's slow
/// poll calls it ~1 Hz forever. The flag only changes when the
/// password is set or cleared, so it is cached like the vJoy probe
/// above (same 60 s TTL) and invalidated on set/clear.
#[derive(Default)]
struct WifiPasswordCache {
    /// When the cached probe ran ([`None`] = never probed, or
    /// invalidated — the next read re-probes immediately).
    probed_at: Option<Instant>,
    /// The board MAC the cached flag belongs to (a different board
    /// re-probes — one board's flag must never answer for another).
    board_mac: Option<String>,
    /// The cached probe result: a Wi-Fi password is stored for
    /// this board.
    password_set: bool,
}

impl WifiPasswordCache {
    /// The cached flag for `mac`, or [`None`] when the cache is
    /// stale for it (never probed, invalidated, a different board,
    /// or the TTL passed). The caller re-probes on [`None`] and
    /// stores the result with [`Self::store`].
    fn get(&self, mac: &str, now: Instant) -> Option<bool> {
        let probed_at = self.probed_at?;
        if self.board_mac.as_deref() != Some(mac) {
            return None;
        }
        if now.duration_since(probed_at) >= WIFI_PASSWORD_CACHE_TTL {
            return None;
        }
        Some(self.password_set)
    }

    /// Store a fresh probe result for `mac`.
    fn store(&mut self, mac: &str, password_set: bool, now: Instant) {
        self.probed_at = Some(now);
        self.board_mac = Some(mac.to_string());
        self.password_set = password_set;
    }

    /// Drop the cached flag (password set / cleared — the next
    /// read re-probes immediately).
    fn invalidate(&mut self) {
        self.probed_at = None;
    }
}

/// Time after which the cached Wi-Fi password-set flag is re-read
/// (R3-D-08): same 60 s as the vJoy probe — a password set by hand
/// in the credential store while the daemon runs still shows up
/// within a minute, without a restart.
const WIFI_PASSWORD_CACHE_TTL: Duration = Duration::from_secs(60);

/// vJoy installer download (v2.0.1 hotfix BACKEND-2).
///
/// Verification status (checked 2026-11-16): the pinned driver is
/// `BrunnerInnovation` vJoy **2.2.2.0** — the repo
/// (`BrunnerInnovation/vJoy`) and the `v2.2.2.0` release (19 Dec
/// 2023, EV + attestation signed, Win10/11) were verified on
/// github.com itself, and the pinned `PINNED_DRIVER_VERSION_FULL`
/// (`2.2.2.0`) in `rsh-output` already names this build. The asset
/// file name and the SHA-256 / size below are corroborated by
/// vjoy.pro (which states its hashes were verified from the actual
/// upstream files); the asset listing itself was not visible in the
/// text fetch of the GitHub release page, so the file name is
/// corroborated, not directly listed. The hash + size check in
/// [`run_vjoy_install`] is therefore load-bearing: a downloaded file
/// that does not match is deleted and never executed. The whole
/// flow still needs a user-PC verify (elevation + silent install
/// cannot be tested on headless Linux).
///
/// The code's existing [`VJOY_DOWNLOAD_URL`] is Brunner's own guide
/// page (`simux.brunner-innovation.swiss/guide/vjoy/`), NOT a
/// downloadable file — it is an HTML page, so it cannot be the
/// download source. The guide itself confirms the model: Simux
/// downloads the installer, verifies it, then runs it.
pub const VJOY_INSTALLER_URL: &str = "https://github.com/BrunnerInnovation/vJoy/releases/download/v2.2.2.0/vJoySetup_v2.2.2.0_Win10_Win11.exe";
/// File name of the pinned installer (matches the URL's last segment).
pub const VJOY_INSTALLER_FILE_NAME: &str = "vJoySetup_v2.2.2.0_Win10_Win11.exe";
/// SHA-256 of the pinned installer (vjoy.pro, upstream-verified).
pub const VJOY_INSTALLER_SHA256_HEX: &str =
    "ef569a3105cd301b89580f18f60c66b339e95296acf2c0dfcaf4b4bbf8ab68fe";
/// Size in bytes of the pinned installer (vjoy.pro).
pub const VJOY_INSTALLER_SIZE_BYTES: u64 = 5_302_312;

/// vJoy installer silent flags (v2.0.1 hotfix BACKEND-2). The vJoy
/// installer is built with Inno Setup 6.0 (per the vJoy README's
/// build instructions), so the documented Inno Setup silent flags
/// apply: `/VERYSILENT` (no wizard, no progress window),
/// `/SUPPRESSMSGBOXES` (no message boxes),
/// `/NORESTART` (never reboot unattended — the driver may still need
/// a restart, which the re-check reports honestly), `/SP-` (skip the
/// "do you wish to continue?" prompt). The flags themselves are
/// documented Inno Setup behaviour; the Brunner 2.2.2.0 installer
/// honouring them still needs a user-PC verify.
pub const VJOY_INSTALLER_ARGS: &[&str] =
    &["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/SP-"];

/// One emission from the snapshot emitter loop: either the latest
/// snapshot (30 Hz) or a queued button-press event. The snapshot is
/// boxed: the payload is ~216 bytes and the button event is 2, and an
/// unboxed enum would pay the large variant's size on every event.
#[derive(Clone, Debug, PartialEq)]
pub enum SnapshotEmit {
    /// The latest snapshot, sampled at 30 Hz.
    Snapshot(Box<EngineSnapshot>),
    /// A button-press edge from the engine pipeline.
    ButtonPress(ButtonPressEvent),
    /// A backend event queued by the core (C-EVT-1): the first field
    /// is the `rsh://…` event name, the second its JSON payload.
    AppEvent(String, serde_json::Value),
}

/// Inputs for one engine tick, as [`AppStateCore::ingest_frame`] consumes
/// them. The caller is the engine task (or a test driving it): steering
/// and pedals are the pipeline's processed values, encoder counts are
/// the board's cumulative `i16` counts, `output` is the pipeline's
/// [`PipelineOutputState`] for this tick (button edges are diffed from
/// it), and `metrics` is the engine's current metrics snapshot.
#[derive(Clone, Copy, Debug)]
pub struct FrameInput {
    /// Steering angle in degrees, -450..450.
    pub steering_deg: f64,
    /// Pedal positions 0..1.
    pub pedals: PedalSnapshot,
    /// Cumulative encoder counts from the latest stream frame.
    pub encoder_counts: [i16; MAX_ENCODERS],
    /// How many encoders the frame carries (`n_enc <= 4`).
    pub n_enc: usize,
    /// The pipeline's processed output for this tick.
    pub output: PipelineOutputState,
    /// The engine's current metrics.
    pub metrics: EngineMetrics,
}

/// Which live transport feeds the engine (Phase G gate wiring).
///
/// There is no persisted transport choice in settings yet, so the
/// shell starts the engine on UDP (Wi-Fi `SoftAP`, the primary path)
/// and serial is connected on demand through
/// [`AppStateCore::connect_transport`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportKind {
    /// Wi-Fi UDP transport (`UdpTransport`, board at the configured
    /// `esp_ip`, default `192.168.4.1`).
    Udp,
    /// Wired USB-serial transport (`SerialTransport`, board port
    /// auto-detected).
    Serial,
}

impl TransportKind {
    /// Short id for logs and diagnostics (`"udp"` / `"serial"`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Udp => "udp",
            Self::Serial => "serial",
        }
    }

    /// Parse a transport name (`"udp"` / `"wifi"`, `"serial"` /
    /// `"wired"`; case-insensitive). Returns [`None`] for an unknown
    /// name — the caller errors honestly instead of guessing.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "udp" | "wifi" | "wi-fi" => Some(Self::Udp),
            "serial" | "wired" | "usb" => Some(Self::Serial),
            _ => None,
        }
    }
}

/// What the engine task is doing (Phase G gate wiring).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineTaskStatus {
    /// The engine task has never been started.
    Idle,
    /// The engine task is running. `transport` is [`None`] while it
    /// ticks with no transport (clean idle: watchdog stale → neutral,
    /// no fake frames) and [`Some`] once a transport is feeding it.
    Running {
        /// The live transport, when one is connected.
        transport: Option<TransportKind>,
    },
    /// The engine task stopped (bounded stop joined, or it was never
    /// started and a stop was requested).
    Stopped,
    /// The engine task failed to start or ended abnormally. The
    /// reason is in [`AppStateCore::engine_status_detail`].
    Failed {
        /// What went wrong.
        reason: String,
    },
}

/// Live transport + session state, as the `get_transport_state`
/// command reports it (H1 condition 15): which transport is live,
/// whether its frames reach the engine, and why not. `session_active`
/// is the SEC-E-14 gate: `false` means stream frames are being held
/// (counted in `frames_held_by_gate`), never silently dropped. The
/// two gate fields extend the Phase G shape (the frontend
/// `TransportState` carries the first four fields; the gate fields
/// are extra JSON fields the frontend reads when present).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TransportState {
    /// `"wifi"` (UDP) or `"usb"` (serial).
    pub transport: String,
    /// Live connection state.
    pub state: ConnectionState,
    /// Board address actually used (Wi-Fi: persisted `esp_ip`).
    pub address: String,
    /// Failure reason, when the state is `error`.
    pub error: Option<String>,
    /// True only once a `BoardSession` completed its unsigned
    /// `GET_CAPS` and the session owner opened the gate (SEC-E-14).
    pub session_active: bool,
    /// Sticky reachable latch (D15): set on every HELLO observed,
    /// cleared on disconnect / transport switch / `NoRoute` /
    /// HELLO-timeout. Surfaced in `ConnectionInfo` via
    /// `DaemonApi::connection_from`.
    pub board_reachable: bool,
    /// Stream frames held by the closed session gate since engine start.
    pub frames_held_by_gate: u64,
}

impl TransportState {
    /// The UI-facing id for a kind (`"wifi"` / `"usb"`).
    #[must_use]
    pub const fn kind_id(kind: TransportKind) -> &'static str {
        match kind {
            TransportKind::Udp => "wifi",
            TransportKind::Serial => "usb",
        }
    }
}

/// The outcome of one successful session activation (H5): the board
/// the session belongs to plus the `GET_CAPS` reply that authenticated
/// it. Recorded on the state by the activator so the Wi-Fi-status
/// and firmware-version commands have a real backend to read — the
/// record exists only while a session is live (disconnect, transport
/// loss, and engine stop clear it; a later reconnect is a new
/// session and must reconnect).
#[derive(Clone, Debug)]
pub struct SessionActivation {
    /// Board MAC as reported by `HELLO` (`"AA:BB:CC:DD:EE:FF"`).
    pub board_id: String,
    /// Board firmware version from `GET_CAPS` (major, minor, patch).
    pub firmware: (u8, u8, u8),
    /// Board protocol version from `GET_CAPS` (major, minor).
    pub protocol: (u8, u8),
    /// The full `GET_CAPS` reply that authenticated the session.
    pub caps: GetCapsReply,
}

/// One `HELLO` reply as the activator last observed it (H5):
/// the board MAC + when it was seen — enough to report the board
/// identity and to derive the `RSH-XXXX` SSID without re-asking
/// the board. No password, no window, no key ever lives here.
/// Only the MAC is kept: the firmware rides on the session
/// activation once connected (and on the connection-log line),
/// and nothing reads a HELLO timestamp — storing either would
/// be dead code.
#[derive(Clone, Debug)]
struct ObservedHello {
    /// Board MAC (`"AA:BB:CC:DD:EE:FF"`) that sent this `HELLO`.
    board_mac: String,
}

/// The outcome of one [`AppStateCore::activate_startup_backend`] call:
/// what was requested (the persisted choice), what is actually live,
/// and — when nothing is live — why. Recorded on the state (see
/// [`AppStateCore::startup_backend_outcome`]) so a silent stay on the
/// [`NullSink`](crate::system::NullSink) is impossible: `active` is
/// [`None`] exactly when activation failed, and `reason` names the
/// failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartupBackendOutcome {
    /// The backend the persisted settings asked for.
    pub requested: BackendKind,
    /// The backend that is actually live now ([`None`] when activation
    /// failed outright and the [`NullSink`](crate::system::NullSink)
    /// stayed installed).
    pub active: Option<BackendKind>,
    /// True when the requested backend failed and the fallback backend
    /// was installed instead.
    pub fell_back: bool,
    /// The activation failure, verbatim, when the requested backend
    /// (and its fallback) could not connect — or the fallback reason
    /// when [`fell_back`](Self::fell_back) is set.
    pub reason: Option<String>,
}

/// The outcome of one [`AppStateCore::stop_engine`] call: whether the
/// engine task joined within the ~5 s bound, and its final
/// [`EngineReport`] when one was delivered.
#[derive(Clone, Copy, Debug)]
pub struct EngineStopOutcome {
    /// True when the engine task joined within the bound.
    pub joined: bool,
    /// The engine's final report, when the task delivered one.
    pub report: Option<EngineReport>,
}

/// A handle to the installed output backend (PERF F-15): a
/// shared pointer to the backend instance's own mutex. The slot
/// in [`AppStateCore`] holds one of these; cloning it is cheap,
/// and a slow backend call (`update()`) holds only the instance's
/// lock — never the slot lock — so a backend switch (which swaps
/// the slot handle) and an engine tick (which cloned the old one)
/// never block each other.
type BackendHandle = Arc<Mutex<Box<dyn IJoystickOutput>>>;

/// The testable app state (Phase G0).
///
/// Owns the engine metrics + loop-rate tracker + encoder state, the
/// installed output backend, the game launcher, and the profile store.
/// Every command the Tauri shell exposes reads/writes this state — no
/// command invents data, and no command succeeds without its backing
/// store or backend having done the work.
///
/// Locking: every field is a short-held `Mutex`; the emitter thread
/// only ever takes one lock at a time (snapshot clone, event drain),
/// so the engine never blocks on the UI — it hands
/// [`FrameInput`] to [`AppStateCore::ingest_frame`] and returns.
pub struct AppStateCore {
    /// Engine metrics, as last reported by the engine task.
    metrics: Mutex<EngineMetrics>,
    /// Measured engine loop rate (sliding 1 s window).
    loop_rate: Mutex<LoopRateTracker>,
    /// Encoder baselines + virtual-button derivation.
    encoders: Mutex<EncoderState>,
    /// The installed output backend ([`IJoystickOutput`]). Starts as
    /// the [`NullSink`]; the G4 selector swaps this slot at runtime
    /// (vJoy default, `HIDMaestro` explicit choice, none) — see
    /// [`AppStateCore::set_output_backend`].
    ///
    /// PERF F-15: the slot holds an `Arc` handle to a per-instance
    /// mutex, not the backend directly. A caller clones the handle
    /// under this slot lock (microseconds — see
    /// [`AppStateCore::backend_handle`]) and runs slow backend calls
    /// (`update()`, up to ~200 ms on `HIDMaestro`'s sidecar
    /// round-trip) on the handle's own lock, so the 200 Hz engine
    /// tick never stalls a backend switch and a switch never stalls
    /// a tick: a switch swaps the slot `Arc`, the tick keeps sending
    /// to the instance it cloned, and the next tick picks up the new
    /// backend. The old shape — `Mutex<Box<...>>` with `update()`
    /// under the slot lock — stalled every switch and every
    /// `output_state()` read for the whole update.
    backend: Mutex<BackendHandle>,
    /// Backend name, exactly as the backend reports it (`name()` at
    /// construction; `"NullSink"` until the selector installs a real
    /// backend). Behind a lock because the selector changes it.
    backend_name: Mutex<String>,
    /// Human-readable backend detail (what the backend is / is not).
    /// Behind a lock because the selector changes it.
    backend_detail: Mutex<String>,
    /// The backend the user selected in settings (persisted in
    /// `AppSettings.output_backend`). May differ from the live backend
    /// after a fallback — the Output page shows both.
    selected_backend: Mutex<BackendKind>,
    /// Whether output is enabled (the Output screen toggle).
    output_enabled: Mutex<bool>,
    /// Last pipeline output (resent on re-enable; neutral on disable).
    last_output: Mutex<PipelineOutputState>,
    /// Last button bitmask seen (`buttons | buttons_ex << 32`), for edge diffing.
    last_buttons: Mutex<u64>,
    /// Outcome of the most recent backend send (`None` = never attempted).
    last_send_ok: Mutex<Option<bool>>,
    /// Human-readable note on the most recent backend send.
    last_send_note: Mutex<String>,
    /// Latest snapshot (built by `ingest_frame`, sampled by the emitter).
    /// `Arc` (DS-30): readers clone the pointer, not the struct.
    latest_snapshot: Mutex<Arc<EngineSnapshot>>,
    /// Monotonic snapshot sequence number.
    seq: Mutex<u64>,
    /// Transport connection state, as last reported.
    connection: Mutex<ConnectionState>,
    /// The transport the UI last asked for (kind + typed address).
    /// The live state is derived from the engine status, never from
    /// this record alone — this only remembers what was requested, so
    /// `transport_state()` can report the kind and address even while
    /// the engine is idle.
    transport_choice: Mutex<(TransportKind, String)>,
    /// Cached USB board port (DS-27): `detect_board_port()` enumerates
    /// ports and probes each one (up to 500 ms per port), so it must
    /// never run on the 20 Hz poll path. The first `transport_state()`
    /// call fills this cache; later calls reuse it for up to
    /// `BOARD_PORT_CACHE_TTL`, then re-detect. A disconnect or a
    /// transport change clears it so the next poll re-detects.
    ///
    /// R2-25: the cache is now filled ONLY by the background
    /// refresher thread ([`AppStateCore::start_board_port_refresher`])
    /// — `cached_board_port()` is a pure last-known read and never
    /// probes inline, so `transport_state()` on the poll path can
    /// never stall on port enumeration.
    cached_serial_port: Mutex<Option<(Option<String>, Instant)>>,
    /// Whether the background board-port refresher thread (R2-25)
    /// is already running: `start_board_port_refresher` is
    /// idempotent, so a second start (e.g. from `connect_transport`)
    /// never spawns a duplicate thread.
    board_port_refresher_started: AtomicBool,
    /// Wake channel for the board-port refresher (PERF F-5): the
    /// refresher waits on this condvar instead of waking every 5 s
    /// to re-scan. `rescan_board_port()` sets the flag and notifies
    /// (a transport change or disconnect wakes the thread at once,
    /// even mid-backoff); the refresher takes the flag when it
    /// wakes and probes. The flag is only ever set here — the
    /// refresher never sets it itself.
    board_port_wake: (Mutex<bool>, Condvar),
    /// Generation of the slow-poll state (PERF F-6): bumped by
    /// [`AppStateCore::note_slow_state_changed`] on every mutation
    /// the ~1 Hz `slow_poll` reports (profile / game / tuning /
    /// binding / knob / draft / calibration / firmware changes —
    /// including ones made directly on the state, e.g. tray actions
    /// in the daemon's manager loop, which bypass the IPC layer).
    /// The daemon's `slow_poll` serves its cached static groups
    /// until this generation moves or their TTL expires, so a
    /// mutation is visible on the next poll instead of waiting out
    /// a TTL.
    slow_state_generation: AtomicU64,
    /// Queued button-press edges, drained by the emitter.
    button_events: Mutex<VecDeque<ButtonPressEvent>>,
    /// Profile + settings store.
    store: Mutex<ProfileStore>,
    /// Cached app settings (DS-30): `AppSettings` carries several
    /// `String`s + a `HashMap`, so cloning the struct on every poll
    /// is measurable on the 20 Hz path. Readers clone this `Arc`
    /// instead (see `settings_arc`); writers refresh it after every
    /// write (see `refresh_settings_cache`) — copy-on-write.
    settings_cache: Mutex<Arc<AppSettings>>,
    /// Game launcher (one game at a time, tracked pid only).
    launcher: Mutex<GameLauncher<SystemSpawner>>,
    /// Validated per-module tunings for this session (see
    /// [`AppStateCore::set_tuning`]).
    tunings: Mutex<BTreeMap<String, TuningSettings>>,
    /// In-memory draft of the active profile (L4, D-1): tuning edits
    /// land here and mark it dirty; disk is touched only by
    /// [`AppStateCore::save_draft`]. [`None`] until the first tuning
    /// edit (a clean draft is indistinguishable from the stored
    /// profile, so none is kept).
    draft: Mutex<Option<ProfileDraft>>,
    /// Sender for live pipeline plans (L4 pipeline push): the
    /// supervisor installs it when the engine starts; [`None`]
    /// before start / after stop, in which case a tuning edit still
    /// updates the draft but pushes nowhere (the next engine start
    /// pushes the stored profile's plan).
    plan_tx: Mutex<Option<tokio::sync::watch::Sender<PipelinePlan>>>,
    /// Learn mode (L4): armed while the Buttons editor waits for a
    /// wheel press; the next physical button press (1–32) is
    /// captured into `learned_button` and disarms it.
    learn_armed: Mutex<bool>,
    /// Button picked by learn mode, until the UI consumes it with
    /// [`AppStateCore::take_learned_button`].
    learned_button: Mutex<Option<u8>>,
    /// Persisted button→action bindings (v2.0.2, Round 2B).
    bindings: Mutex<BindingStore>,
    /// Load-time warnings from the bindings store (see
    /// [`AppStateCore::binding_load_warnings`]).
    binding_load_warnings: Vec<String>,
    /// Manually added games (game-scanner removal, v2.0.3+):
    /// the `manual-games.json` store — name + exe/shortcut path
    /// per entry, no detection of any kind.
    manual_games: Mutex<ManualGameStore>,
    /// Load-time warnings from the manual-games store (see
    /// [`AppStateCore::manual_game_load_warnings`]).
    manual_game_load_warnings: Vec<String>,
    /// Last pressed set for buttons 1–64 (`buttons | buttons_ex <<
    /// 32`), for app-action edge detection in
    /// [`AppStateCore::apply_bindings`].
    last_bound_pressed: Mutex<u64>,
    /// Last encoder-button bits seen (`encoder_buttons.0`, bits
    /// 0–7 = virtual buttons 65–72), for app-action edge detection
    /// in [`AppStateCore::apply_bindings`] (R2-08): the engine
    /// repeats `last_output` on ticks with no new frame, so a
    /// momentary encoder pulse would otherwise re-fire its app
    /// action on every tick until the next frame recomputes it.
    last_bound_encoder: Mutex<u8>,
    /// Queued app actions from bound button presses, drained by the
    /// daemon manager loop ([`AppStateCore::drain_binding_actions`]).
    binding_actions: Mutex<VecDeque<BindingAppAction>>,
    /// Engine stop handle, when the engine task is running.
    stop_handle: Mutex<Option<StopHandle>>,
    /// Engine task status (Phase G gate wiring): `Idle` until
    /// [`AppStateCore::start_engine`] runs.
    engine_status: Mutex<EngineTaskStatus>,
    /// Human-readable engine status detail (transport state, retry
    /// notes, failure reasons — never silently empty on failure).
    engine_status_detail: Mutex<String>,
    /// Command channel into the engine supervisor thread, while the
    /// engine task is running ([`None`] before start / after stop).
    engine_cmd_tx: Mutex<Option<tokio::sync::mpsc::Sender<EngineCommand>>>,
    /// Report channel from the engine supervisor thread, while the
    /// engine task is running ([`None`] before start / after stop).
    engine_report_rx: Mutex<Option<StdReceiver<EngineReport>>>,
    /// The engine supervisor std thread, while the engine task is
    /// running ([`None`] before start / after stop).
    engine_thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// Frame channel into the running engine (the forwarder and test
    /// feeders send here), while the engine task is running.
    engine_frame_tx: Mutex<Option<tokio::sync::mpsc::Sender<StreamFrame>>>,
    /// The last [`EngineReport`] a stopped engine delivered, if any.
    engine_last_report: Mutex<Option<EngineReport>>,
    /// Cumulative encoder counts reconstructed from the engine's
    /// per-tick deltas (see [`EngineSink`]).
    engine_encoders: Mutex<EngineEncoderAccum>,
    /// Latest raw axis readings in mV (see [`RawAxesSample`]): the
    /// capture methods sample this slot. Reported per frame by the
    /// frame feeder ([`AppStateCore::note_raw_axes`]).
    raw_axes: Mutex<RawAxesSample>,
    /// Outcome of the most recent
    /// [`AppStateCore::activate_startup_backend`] call ([`None`] until
    /// the shell activates the persisted backend at launch).
    startup_backend: Mutex<Option<StartupBackendOutcome>>,
    /// One-time v2.0.0 → v2.0.1 backend-default migration note
    /// ([`migrate_persisted_backend_default`]): [`Some`] with the
    /// human-readable note when this launch migrated a persisted
    /// `HIDMaestro` default to vJoy, [`None`] otherwise. The daemon's
    /// startup log should print it — a persisted choice must never
    /// change silently.
    backend_migration_note: Mutex<Option<String>>,
    /// Background vJoy install progress (see [`VjoyInstallState`]).
    vjoy_install: Mutex<VjoyInstallState>,
    /// Cached vJoy DLL probe (R2-20, see [`VjoyDllCache`]): the
    /// probe result `vjoy_status()` reports, so the ~1 Hz slow poll
    /// does not re-run the registry / file probes every cycle.
    /// `installing` / `install_error` are NOT cached — they are
    /// cheap lock reads and must stay live.
    vjoy_dll_cache: Mutex<VjoyDllCache>,
    /// Cached Wi-Fi password-set flag (R3-D-08, see
    /// [`WifiPasswordCache`]): the Credential Manager read behind
    /// `wifi_status()` is cached the same way, so the ~1 Hz slow
    /// poll does not hit the store every cycle.
    wifi_password_cache: Mutex<WifiPasswordCache>,
    /// HMAC session gate (SEC-E-14, H1 condition 13): `true` only
    /// once a `BoardSession` has completed its HMAC-verified
    /// `GET_CAPS` and the session owner opened the gate. Starts
    /// closed; the forwarder holds stream frames while it is closed.
    session_gate_open: Mutex<bool>,
    /// Stream frames held by the closed session gate, since the
    /// engine started (diagnostics: proves the gate is enforcing
    /// rather than silently dropping).
    session_gate_held_frames: Mutex<u64>,
    /// The last successful session activation (H5): board id + the
    /// `GET_CAPS` reply that authenticated it. [`None`] while no
    /// session is live — disconnect, transport loss, and engine stop
    /// clear it, so a dead session is never presented as live.
    session_activation: Mutex<Option<SessionActivation>>,
    /// The last session-activation failure, if any (H5): the
    /// fail-closed reason the connection status reports. Cleared on the
    /// next successful activation and on disconnect.
    session_last_error: Mutex<Option<String>>,
    /// The last `HELLO` the activator observed (H5): board MAC +
    /// firmware. Cleared on disconnect.
    session_last_hello: Mutex<Option<ObservedHello>>,
    /// The config-transaction record (H5-control): the resumable
    /// snapshot of the stage → commit → confirm → rollback flow
    /// (`stage_config` → `commit_config` → `confirm_config` /
    /// `rollback_config`), kept between commands because every
    /// command runs under its own fresh session (D30) and the
    /// transaction client itself cannot survive. In memory only: an
    /// app restart loses it, and the board then self-rolls-back
    /// within 30 s — the next `rollback_config` (or a fresh
    /// `stage_config`) re-syncs the record.
    config_txn: Mutex<ConfigTxnSnapshot>,
    /// App-level events queued for the frontend (C-EVT-1):
    /// `rsh://profile-switched`, `rsh://game-state`,
    /// `rsh://config-rollback`. Drained by
    /// the emitter thread (`drain_app_events`), which emits each
    /// through the Tauri window.
    pending_events: Mutex<Vec<(String, serde_json::Value)>>,
    /// The installed auto-switch poller (C1): a
    /// [`TelemetryWatcher`](rsh_profiles::watcher::TelemetryWatcher)
    /// over a [`StoreProfileSource`] (the store as a
    /// [`ProfileSource`]), installed by [`AppStateCore::start_auto_switch`].
    /// [`None`] until auto-switch is enabled.
    auto_switch: Mutex<Option<Box<dyn AutoSwitchPoller + Send>>>,
    /// Whether the auto-switch thread is running (C1). Set by
    /// `start_auto_switch`, cleared by `stop_auto_switch`; the
    /// thread polls while this is true.
    auto_switch_running: AtomicBool,
    /// The active profile before the currently running game launched
    /// (H4): `stop_game` restores it. [`None`] when no game is running.
    pre_game_profile: Mutex<Option<String>>,
    /// Whether the mid-session failover already fired (C-FAIL-1):
    /// the backend auto-switches to the fallback at most once until
    /// an explicit user re-select calls [`AppStateCore::reset_fallback`].
    fallback_used: AtomicBool,
    /// When the last failover attempt ran (C-FAIL-1 rate limit: at
    /// most one attempt per 30 s, even after a reset).
    fallback_last_attempt: Mutex<Option<Instant>>,
    /// Whether the user declined the `HIDMaestro` UAC prompt this
    /// session (B-2/A10, v2.0.2): while set, an automatic
    /// (re-)activation of `HIDMaestro` fails fast with the `NotAdmin`
    /// message instead of prompting again. Only an explicit user
    /// re-select ([`AppStateCore::set_output_backend`]) clears it —
    /// a fresh factory build (the H2 re-arm in
    /// [`Self::apply_backend_switch`]) must NOT clear it, or the
    /// prompt would loop again.
    elevation_declined: AtomicBool,
    /// Serialises backend switches (B-2): a switch already in
    /// flight (UAC prompt on screen / sidecar spawning, up to ~10 s)
    /// rejects a second overlapping switch instead of spawning a
    /// second elevated sidecar. Held across the whole
    /// [`Self::apply_backend_switch`] by `try_lock`.
    switch_in_flight: Mutex<()>,
    /// Firmware-flash status (H-4, v2.0.2): the background flash
    /// thread's progress / outcome, polled by the UI while a flash
    /// runs. Idle (`running: false`, `done: false`) until the first
    /// flash starts.
    firmware_update: Mutex<FirmwareUpdateStatus>,
    /// Board IP the live session connected against (M-1):
    /// `upload_firmware`'s HTTP leg is bound to this IP, never to a
    /// stale settings value.
    session_board_ip: Mutex<Option<IpAddr>>,
    /// The last session-activation failure, typed (Phase 2, C-01):
    /// the [`ConnectionError`] behind [`Self::session_last_error`].
    /// The daemon's Connection Manager and the API's typed session
    /// error read this — the UI matches on it instead of parsing a
    /// string (Phase 0 gate §4.3).
    session_last_error_typed: Mutex<Option<ConnectionError>>,
    /// The last *control-session* failure (R2-05): a failed LED
    /// test / config / firmware step under a transient control
    /// session. Deliberately separate from
    /// [`Self::session_last_error_typed`] — a control op failing
    /// while the board streams must never knock the live session's
    /// manager state into `Error`. Read via
    /// [`AppStateCore::control_last_error`]; the IPC caller of the
    /// failed op still gets the error directly as its reply.
    control_last_error: Mutex<Option<String>>,
    /// Whether a forwarder activation exchange (flag-1 HELO →
    /// unsigned `GET_CAPS`) is currently in flight (R2-28): set by
    /// `activate_session_on`, cleared on every exit path. While set,
    /// [`AppStateCore::connect_session`] no-ops instead of aborting
    /// the forwarder mid-activation via `connect_transport`.
    activation_in_flight: AtomicBool,
    /// How many forwarder retry waits are currently armed (R3-D-02):
    /// a count, not a bool, because the wait is armed in two nested
    /// places — the frame loop's retry deadline and
    /// `forwarder_retry_after_failure`'s backoff sleep — and a bool
    /// cleared by the inner guard would lie while the outer wait
    /// still holds. While > 0 the forwarder owns the retry: a
    /// manager `RetryTimer` or a UI Connect must NOT abort it and
    /// spawn a fresh forwarder (the old behaviour reset the
    /// forwarder's breaker streak to 0 on every manager retry, so
    /// the 10-failure circuit breaker could never trip). Read via
    /// [`AppStateCore::forwarder_retry_waiting`].
    forwarder_retry_wait: AtomicU32,
    /// Latched when a forwarder run ends with its circuit breaker
    /// open (R3-D-02): the run is over and no automatic retry may
    /// start a fresh one — the user must reconnect. Cleared by an
    /// explicit [`AppStateCore::connect_transport`] (user Connect /
    /// watchdog rebuild / startup) or
    /// [`AppStateCore::disconnect_transport`]. Read via
    /// [`AppStateCore::forwarder_breaker_open`].
    forwarder_breaker_open: AtomicBool,
    /// Sticky "board reachable" latch (D15): set on every HELLO
    /// observed ([`Self::note_observed_hello`]), cleared on
    /// disconnect / transport switch / `NoRoute` / HELLO-timeout.
    /// The daemon reports it as `ConnectionInfo.board_reachable`;
    /// the wizard Step 1 Next unlocks on it (HELLO-or-better),
    /// never on `Active` alone.
    board_reachable: AtomicBool,
    /// Connection log lines (Phase 0 gate §6): the redacted,
    /// timestamped trail of connection attempts — one line per
    /// transition, never per poll. The daemon drains this into the
    /// diagnostics log file; tests read it directly. Capped at 200
    /// lines (oldest dropped).
    connection_log: Mutex<VecDeque<String>>,
    /// Retained tail of the connection log (R2-23/R2-42): the last
    /// [`CONNECTION_LOG_TAIL_LINES`] lines, kept for diagnostics
    /// readers. The manager loop drains `connection_log` every
    /// 250 ms for the diagnostics file (DS-32) — without this
    /// second buffer the 1 Hz readers almost always saw an empty
    /// log. This buffer is NEVER drained, only capped, so a drain
    /// can no longer starve the reader.
    connection_log_tail: Mutex<VecDeque<String>>,
    /// When the last stream frame arrived (C-04 heartbeat watchdog):
    /// the daemon's Connection Manager checks this — no frame for
    /// 2 s in `Active` means rebuild + re-activate.
    last_stream_frame_at: Mutex<Option<Instant>>,
}

/// One auto-switch poller (C1): the [`TelemetryWatcher`] behind a
/// trait object, so the state can hold the real watcher (live car
/// source + foreground detector) or a test watcher (fake source) in
/// the same slot. `poll` runs one 2 Hz tick and returns the switch
/// events that tick produced.
pub trait AutoSwitchPoller {
    /// Runs one poll tick at `now`; returns the switch events.
    fn poll(&mut self, now: Instant) -> Vec<ProfileSwitchEvent>;
}

impl<C, F, A> AutoSwitchPoller for TelemetryWatcher<'static, C, F, A>
where
    C: CarSource,
    F: ForegroundDetector,
    A: FnMut(&ProfileId) -> Result<(), String>,
{
    fn poll(&mut self, now: Instant) -> Vec<ProfileSwitchEvent> {
        TelemetryWatcher::poll(self, now)
    }
}

/// The profile store as a [`ProfileSource`] (C1): the auto-switch
/// watcher resolves profiles by reading the live store through a
/// [`Weak`] to the state, so the watcher thread never keeps the state
/// alive and never deadlocks the store (each call locks, reads, and
/// releases). When the state is gone the source resolves nothing.
pub struct StoreProfileSource {
    state: Weak<AppStateCore>,
}

impl StoreProfileSource {
    /// Runs `f` against the live store, or returns [`None`] when the
    /// state is gone.
    fn with_store<T>(&self, f: impl FnOnce(&ProfileStore) -> Option<T>) -> Option<T> {
        let state = self.state.upgrade()?;
        f(&lock(&state.store))
    }
}

impl ProfileSource for StoreProfileSource {
    fn profile_for_game(&self, game_id: &str) -> Option<ProfileId> {
        self.with_store(|store| store.profile_for_game(game_id))
    }

    fn profile_for_car(&self, car_name: &str) -> Option<ProfileId> {
        self.with_store(|store| store.profile_for_car(car_name))
    }

    fn profile_for_game_car(&self, game_id: &str, car_name: &str) -> Option<ProfileId> {
        self.with_store(|store| store.profile_for_game_car(game_id, car_name))
    }

    fn default_profile(&self) -> Option<ProfileId> {
        self.with_store(ProfileSource::default_profile)
    }
}

impl AppStateCore {
    /// Builds the state over an installed backend. The backend's name
    /// is taken from the backend itself (`name()`); `backend_detail`
    /// is the state's own human-readable note about the backend
    /// (never invented by the backend — see `with_null_sink`).
    ///
    /// Runs the one-time v2.0.0 → v2.0.1 backend-default migration
    /// ([`migrate_persisted_backend_default`]) on the store first, so
    /// `selected_backend` already reflects the migrated choice.
    #[must_use]
    pub fn new(
        mut store: ProfileStore,
        backend: Box<dyn IJoystickOutput>,
        backend_detail: impl Into<String>,
    ) -> Self {
        let backend_name = backend.name().to_string();
        let output_enabled = store.settings().output_enabled;
        let backend_migration_note = migrate_persisted_backend_default(&mut store);
        let selected_backend =
            BackendKind::parse(&store.settings().output_backend).unwrap_or(BackendKind::DEFAULT);
        let esp_ip = store.settings().esp_ip.clone();
        // DS-30: the settings cache starts from the post-migration
        // store (the migration above may have rewritten the backend).
        let settings_cache = Arc::new(store.settings().clone());
        // The binding store lives beside the profile store; a
        // corrupt bindings file loads as empty with warnings (never
        // fails the state open — see `rsh_profiles::bindings`).
        let bindings = BindingStore::open(store.dir());
        let binding_load_warnings = bindings.load_warnings().to_vec();
        // The manual-games store lives beside the profile store too
        // (same load-never-fails contract — see `manual_games`).
        let manual_games = ManualGameStore::open(store.dir());
        let manual_game_load_warnings = manual_games.load_warnings().to_vec();
        Self {
            metrics: Mutex::new(EngineMetrics::new()),
            loop_rate: Mutex::new(LoopRateTracker::new()),
            encoders: Mutex::new(EncoderState::new()),
            backend: Mutex::new(Arc::new(Mutex::new(backend))),
            backend_name: Mutex::new(backend_name),
            backend_detail: Mutex::new(backend_detail.into()),
            selected_backend: Mutex::new(selected_backend),
            output_enabled: Mutex::new(output_enabled),
            last_output: Mutex::new(PipelineOutputState::neutral()),
            last_buttons: Mutex::new(0),
            last_send_ok: Mutex::new(None),
            last_send_note: Mutex::new("not attempted yet".to_string()),
            latest_snapshot: Mutex::new(Arc::new(EngineSnapshot::default())),
            seq: Mutex::new(0),
            connection: Mutex::new(ConnectionState::Disconnected),
            transport_choice: Mutex::new((TransportKind::Udp, esp_ip)),
            cached_serial_port: Mutex::new(None),
            board_port_refresher_started: AtomicBool::new(false),
            board_port_wake: (Mutex::new(false), Condvar::new()),
            slow_state_generation: AtomicU64::new(0),
            button_events: Mutex::new(VecDeque::new()),
            store: Mutex::new(store),
            settings_cache: Mutex::new(settings_cache),
            launcher: Mutex::new(GameLauncher::new(SystemSpawner)),
            tunings: Mutex::new(BTreeMap::new()),
            draft: Mutex::new(None),
            plan_tx: Mutex::new(None),
            learn_armed: Mutex::new(false),
            learned_button: Mutex::new(None),
            bindings: Mutex::new(bindings),
            binding_load_warnings,
            manual_games: Mutex::new(manual_games),
            manual_game_load_warnings,
            last_bound_pressed: Mutex::new(0),
            last_bound_encoder: Mutex::new(0),
            binding_actions: Mutex::new(VecDeque::new()),
            stop_handle: Mutex::new(None),
            engine_status: Mutex::new(EngineTaskStatus::Idle),
            engine_status_detail: Mutex::new("engine not started yet".to_string()),
            engine_cmd_tx: Mutex::new(None),
            engine_report_rx: Mutex::new(None),
            engine_thread: Mutex::new(None),
            engine_frame_tx: Mutex::new(None),
            engine_last_report: Mutex::new(None),
            engine_encoders: Mutex::new(EngineEncoderAccum::default()),
            raw_axes: Mutex::new(RawAxesSample::default()),
            startup_backend: Mutex::new(None),
            backend_migration_note: Mutex::new(backend_migration_note),
            vjoy_install: Mutex::new(VjoyInstallState::default()),
            vjoy_dll_cache: Mutex::new(VjoyDllCache::default()),
            wifi_password_cache: Mutex::new(WifiPasswordCache::default()),
            session_gate_open: Mutex::new(false),
            session_gate_held_frames: Mutex::new(0),
            session_activation: Mutex::new(None),
            session_last_error: Mutex::new(None),
            session_last_hello: Mutex::new(None),
            config_txn: Mutex::new(ConfigTxnSnapshot::default()),
            pending_events: Mutex::new(Vec::new()),
            auto_switch: Mutex::new(None),
            auto_switch_running: AtomicBool::new(false),
            pre_game_profile: Mutex::new(None),
            fallback_used: AtomicBool::new(false),
            fallback_last_attempt: Mutex::new(None),
            elevation_declined: AtomicBool::new(false),
            switch_in_flight: Mutex::new(()),
            firmware_update: Mutex::new(FirmwareUpdateStatus::default()),
            board_reachable: AtomicBool::new(false),
            session_board_ip: Mutex::new(None),
            session_last_error_typed: Mutex::new(None),
            control_last_error: Mutex::new(None),
            activation_in_flight: AtomicBool::new(false),
            forwarder_retry_wait: AtomicU32::new(0),
            forwarder_breaker_open: AtomicBool::new(false),
            connection_log: Mutex::new(VecDeque::new()),
            connection_log_tail: Mutex::new(VecDeque::new()),
            last_stream_frame_at: Mutex::new(None),
        }
    }

    /// Builds the state with the [`NullSink`] installed: no output
    /// device. Every send is attempted and reported as not delivered
    /// (D-F3) until the G4 selector installs a real backend.
    #[must_use]
    pub fn with_null_sink(store: ProfileStore) -> Self {
        Self::new(
            store,
            Box::new(NullSink),
            "no output backend installed yet — sends are attempted and reported as not delivered (select a backend on the Output page)",
        )
    }

    /// Opens the profile store in `dir` and builds the state with the
    /// [`NullSink`] installed. This is the constructor the Tauri shell
    /// uses — and the [`NullSink`] is the *fallback base*, not the
    /// startup default: the shell calls
    /// [`AppStateCore::activate_startup_backend`] right after opening,
    /// which activates the persisted backend choice through the
    /// selector (vJoy when selected; a persisted `HIDMaestro` choice
    /// is deferred at startup — no UAC prompt — until the user enables
    /// output or picks it on the Output page), with the reason
    /// recorded on failure. A missing store file means defaults (one `Default`
    /// profile, default settings); a corrupt file is an error, never
    /// silently dropped data.
    ///
    /// # Errors
    ///
    /// Returns a message when a present store file cannot be parsed or
    /// read.
    pub fn open_with_null_sink(dir: &Path) -> Result<Self, String> {
        let store = ProfileStore::open(dir).map_err(|error| error.to_string())?;
        Ok(Self::with_null_sink(store))
    }

    /// Opens the store like [`AppStateCore::open_with_null_sink`], but
    /// a corrupt store is repaired from its 3-generation backups first
    /// (H3, Phase G condition 16) instead of exiting with no recourse.
    ///
    /// Returns the opened state and, when a repair happened, the
    /// human-readable repair note for the startup notice — the shell
    /// shows it in the UI so a silent restore never happens. When the
    /// store opens cleanly the note is [`None`]; when repair fails
    /// the error names the file and the generations that were tried.
    pub fn open_with_null_sink_repair(dir: &Path) -> (Result<Self, String>, Option<String>) {
        match Self::open_with_null_sink(dir) {
            Ok(state) => (Ok(state), None),
            Err(first_error) => {
                match rsh_profiles::store::try_repair_store_from_backup(dir) {
                    Ok(Some(notice)) => match Self::open_with_null_sink(dir) {
                        Ok(state) => (Ok(state), Some(notice)),
                        Err(error) => (
                            Err(format!(
                                "store repair reported success but the store still \
                                 will not open: {error}"
                            )),
                            Some(notice),
                        ),
                    },
                    Ok(None) => {
                        // The store opened during the repair check
                        // (transient failure) — retry the open for real.
                        match Self::open_with_null_sink(dir) {
                            Ok(state) => (Ok(state), None),
                            Err(_) => (Err(first_error), None),
                        }
                    }
                    Err(repair_error) => (Err(repair_error), None),
                }
            }
        }
    }

    /// The installed backend's name, exactly as reported.
    #[must_use]
    pub fn backend_name(&self) -> String {
        lock(&self.backend_name).clone()
    }

    /// Clone the live backend handle (PERF F-15): the slot lock is
    /// held only for the `Arc` clone. The caller then locks the
    /// handle's own mutex for its backend call — a slow `update()`
    /// (up to ~200 ms on `HIDMaestro`'s sidecar round-trip) never
    /// holds the slot lock, so it cannot stall a backend switch,
    /// and a switch (which swaps the slot handle) cannot stall a
    /// tick: the tick keeps sending to the instance it cloned and
    /// the next tick picks up the new backend.
    fn backend_handle(&self) -> BackendHandle {
        lock(&self.backend).clone()
    }

    /// The backend the user selected in settings (persisted choice;
    /// may differ from the live backend after a fallback).
    #[must_use]
    pub fn selected_backend(&self) -> BackendKind {
        *lock(&self.selected_backend)
    }

    /// The one-time backend-default migration note for this launch
    /// ([`Some`] when [`AppStateCore::new`] migrated a persisted
    /// `HIDMaestro` default to vJoy). The daemon's startup log
    /// should print it so the change is visible, never silent.
    #[must_use]
    pub fn backend_migration_note(&self) -> Option<String> {
        lock(&self.backend_migration_note).clone()
    }

    /// Registers the engine's stop handle (called by whoever spawns
    /// the engine task; `None` until then — shutdown still sends
    /// neutral, it just has no engine to signal).
    pub fn set_stop_handle(&self, handle: StopHandle) {
        *lock(&self.stop_handle) = Some(handle);
    }

    /// Reports the transport connection state (called by the transport
    /// layer when it exists; `Disconnected` until then).
    pub fn set_connection(&self, connection: ConnectionState) {
        *lock(&self.connection) = connection;
    }

    /* ------------------------------------------------------------------ */
    /* HMAC session gate (SEC-E-14, H1 condition 13)                      */
    /* ------------------------------------------------------------------ */

    /// Whether the session gate is open: a `BoardSession` completed
    /// its HMAC-verified `GET_CAPS` and the session owner opened the
    /// gate. While closed, the forwarder holds stream frames instead
    /// of feeding them to the engine.
    #[must_use]
    pub fn session_gate_open(&self) -> bool {
        *lock(&self.session_gate_open)
    }

    /// Stream frames held by the closed session gate since the engine
    /// started (diagnostics counter).
    #[must_use]
    pub fn session_gate_held_frames(&self) -> u64 {
        *lock(&self.session_gate_held_frames)
    }

    /// Opens the session gate: call only after a `BoardSession` has
    /// completed its unsigned `GET_CAPS` for
    /// the live transport. From here on the forwarder feeds stream
    /// frames to the engine. The gate closes again on transport loss
    /// or disconnect — a new transport is a new session and must
    /// re-authenticate.
    pub fn open_session_gate(&self) {
        if !*lock(&self.session_gate_open) {
            self.push_connection_log("INFO", "session gate: closed -> open");
        }
        *lock(&self.session_gate_open) = true;
        self.note_engine_detail(
            "session authenticated (SEC-E-14) — stream frames flowing".to_string(),
        );
    }

    /// Closes the session gate: stream frames are held again until a
    /// session re-authenticates. Called by the forwarder on transport
    /// loss and by disconnect handling; idempotent.
    pub fn close_session_gate(&self) {
        if *lock(&self.session_gate_open) {
            self.push_connection_log("INFO", "session gate: open -> closed");
        }
        *lock(&self.session_gate_open) = false;
    }

    /// Counts one stream frame held by the closed session gate.
    /// Called by the forwarder only (never by tests driving the frame
    /// channel directly — that channel bypasses the transport gate).
    fn note_gate_held_frame(&self) {
        *lock(&self.session_gate_held_frames) += 1;
    }

    /* ------------------------------------------------------------------ */
    /* Session activation records (H5 — the session-owner activator)     */
    /* ------------------------------------------------------------------ */

    /// The last successful session activation, if a session is live.
    #[must_use]
    pub fn session_activation(&self) -> Option<SessionActivation> {
        lock(&self.session_activation).clone()
    }

    /// The last session-activation failure, if any.
    #[must_use]
    pub fn session_last_error(&self) -> Option<String> {
        lock(&self.session_last_error).clone()
    }

    /// The last session-activation failure, typed (Phase 2): the
    /// [`ConnectionError`] behind [`Self::session_last_error`]. The
    /// daemon's Connection Manager and the API's typed connection
    /// error read this — never parse the string.
    #[must_use]
    pub fn session_last_error_typed(&self) -> Option<ConnectionError> {
        *lock(&self.session_last_error_typed)
    }

    /// The typed connection error for the API (Phase 0 gate §4.3,
    /// backend part): the typed reason when one was recorded, else
    /// the stored reason string classified from its text.
    #[must_use]
    pub fn session_error_typed(&self) -> Option<ConnectionError> {
        if let Some(typed) = self.session_last_error_typed() {
            return Some(typed);
        }
        lock(&self.session_last_error)
            .as_deref()
            .map(Self::classify_error_message)
    }

    /// The connection log lines (Phase 0 gate §6), oldest
    /// first. The daemon drains these into the diagnostics log
    /// file; a connection attempt must leave lines here
    /// (regression test: connect with no board).
    #[must_use]
    pub fn connection_log_lines(&self) -> Vec<String> {
        lock(&self.connection_log).iter().cloned().collect()
    }

    /// The retained connection-log tail (R2-23/R2-42): the last
    /// [`CONNECTION_LOG_TAIL_LINES`] lines, oldest first. It is
    /// never drained, so the manager loop's 250 ms file drain
    /// cannot starve a 1 Hz diagnostics read, and the IPC shape
    /// stays small (≤20 lines).
    #[must_use]
    pub fn connection_log_tail_lines(&self) -> Vec<String> {
        lock(&self.connection_log_tail).iter().cloned().collect()
    }

    /// Drains the connection log lines (DS-32): returns all
    /// lines queued since the last drain and clears the buffer — the
    /// `take_*` pattern `take_manager_log_lines()` already uses in
    /// `DaemonApi`. The manager loop drains with this instead of
    /// re-cloning the whole buffer and slicing by index every
    /// 250 ms. `connection_log_lines()` stays for tests and one-shot
    /// readers.
    #[must_use]
    pub fn take_connection_log_lines(&self) -> Vec<String> {
        lock(&self.connection_log).drain(..).collect()
    }

    /// Push one redacted, timestamped connection log line
    /// (Phase 0 gate §6.2 format: `[<ISO> <LEVEL> connection]
    /// <message>`). Every line goes through [`redact_secrets`]
    /// first — a secret must never reach the log, even if a caller
    /// passes one in. Capped at 200 lines (oldest dropped); one
    /// line per transition, never per poll. The same line is also
    /// kept in the retained tail (R2-23/R2-42, capped at
    /// [`CONNECTION_LOG_TAIL_LINES`]) for diagnostics readers.
    fn push_connection_log(&self, level: &str, message: &str) {
        let line = format!("[{} {level} connection] {message}", utc_now_iso());
        let line = redact_secrets(&line);
        let mut log = lock(&self.connection_log);
        if log.len() >= 200 {
            log.pop_front();
        }
        log.push_back(line.clone());
        let mut tail = lock(&self.connection_log_tail);
        if tail.len() >= CONNECTION_LOG_TAIL_LINES {
            tail.pop_front();
        }
        tail.push_back(line);
    }

    /// Classify a [`SessionError`] into its typed
    /// [`ConnectionError`] (no secret ever rides along — the detail
    /// string stays separate).
    fn classify_session_error(error: &SessionError) -> ConnectionError {
        use rsh_transport::control_client::ControlError;
        match error {
            // Pairing variants (`WindowClosed`, `StoredKeyExists`,
            // …) no longer exist in v2.2 — the wildcard below maps
            // anything unrecognised (including a stale variant a
            // caller still produces) to `Unknown`, so this match
            // stays exhaustive across the WS-2 SessionError rewrite.
            SessionError::BoardMismatch => ConnectionError::BoardMismatch,
            SessionError::Control(ControlError::Timeout) => ConnectionError::Timeout,
            // Every other `SessionError` (control failures,
            // keystore failures, malformed HELLOs, …) maps to
            // `Unknown` — the wildcard keeps this exhaustive
            // across the WS-2 `SessionError` rewrite without
            // naming variants that no longer exist.
            _ => ConnectionError::Unknown,
        }
    }

    /// Classify a stored reason string (legacy callers that only
    /// have a message) by its stable substrings.
    fn classify_error_message(message: &str) -> ConnectionError {
        if message.contains("no HELLO reply") || message.contains("board not responding") {
            ConnectionError::Unreachable
        } else if message.contains("transport closed") {
            ConnectionError::TransportClosed
        } else {
            ConnectionError::Unknown
        }
    }

    /// Record a stream frame arrival (C-04 heartbeat): the
    /// daemon's watchdog reads this via
    /// [`Self::seconds_since_last_stream_frame`].
    pub fn note_stream_frame(&self) {
        *lock(&self.last_stream_frame_at) = Some(Instant::now());
    }

    /// Seconds since the last stream frame arrived (`None` = no
    /// frame ever seen). The daemon's Connection Manager calls this
    /// on its watchdog tick.
    #[must_use]
    pub fn seconds_since_last_stream_frame(&self) -> Option<f64> {
        lock(&self.last_stream_frame_at).map(|at| at.elapsed().as_secs_f64())
    }

    /// Records a successful activation: the board id + the
    /// `GET_CAPS` reply. Clears the last activation error.
    fn record_session_activation(&self, activation: SessionActivation) {
        let (firmware, protocol, board_mac) = (
            activation.firmware,
            activation.protocol,
            activation.board_id.clone(),
        );
        *lock(&self.session_activation) = Some(activation);
        *lock(&self.session_last_error) = None;
        *lock(&self.session_last_error_typed) = None;
        // Phase 0 gate §6.2 format 6 (success).
        self.push_connection_log(
            "INFO",
            &format!(
                "activation ok: firmware={}.{}.{} protocol={}.{} board_mac={board_mac}",
                firmware.0, firmware.1, firmware.2, protocol.0, protocol.1
            ),
        );
    }

    /// Records a failed activation: the fail-closed reason the
    /// connection status reports, plus the engine detail (frames
    /// stay held until a session activates). The gate is left
    /// exactly as it was — a failed `GET_CAPS` never opens it.
    /// Record a failed activation with its typed reason (Phase 2):
    /// sets both the string and the typed records, the engine
    /// detail, and the Phase 0 gate §6.2 format-6 failure line
    /// (`activation failed: reason=<code> detail=<hint>`).
    fn note_session_error_typed(&self, typed: ConnectionError, reason: &str) {
        *lock(&self.session_last_error) = Some(reason.to_string());
        *lock(&self.session_last_error_typed) = Some(typed);
        self.push_connection_log(
            "ERROR",
            &format!(
                "activation failed: reason={} detail={}",
                typed.code(),
                typed.hint()
            ),
        );
        self.note_engine_detail(format!(
            "{reason} — stream frames held until a session activates (SEC-E-14)"
        ));
    }

    /// Records a failed *control* operation (R2-05): a transient
    /// control session's failure (LED test, config, firmware step,
    /// restart) goes here — never into
    /// [`Self::note_session_error`]. The manager loop feeds only
    /// `session_last_error_typed` as `ActivationFailed`, so a
    /// control failure can no longer knock a live, streaming session
    /// into `Error`. The connection log still carries the failure (with
    /// its own prefix, so it cannot be mistaken for an activation
    /// failure), and the IPC caller gets the error as its reply.
    fn note_control_error(&self, reason: &str) {
        *lock(&self.control_last_error) = Some(reason.to_string());
        self.push_connection_log("ERROR", &format!("control op failed: {reason}"));
    }

    /// The last control-session failure, if any (R2-05) — for
    /// diagnostics; the connection status never reads this.
    #[must_use]
    pub fn control_last_error(&self) -> Option<String> {
        lock(&self.control_last_error).clone()
    }

    /// Records one `HELLO` as observed by the activator: the
    /// board MAC + firmware, and sets the sticky
    /// [`Self::board_reachable`] latch (D15) — the wizard Step 1
    /// gate reads it. No password, no window, no key is ever
    /// recorded here: a v2.2 `HELLO` discloses none of them.
    fn note_observed_hello(&self, hello: &HelloReply) {
        let board_mac = hello
            .board_mac
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<Vec<String>>()
            .join(":");
        *lock(&self.session_last_hello) = Some(ObservedHello {
            board_mac: board_mac.clone(),
        });
        self.board_reachable.store(true, Ordering::SeqCst);
        // Phase 0 gate §6.2 format 3 (HELLO received — MAC +
        // firmware only, never a secret).
        self.push_connection_log(
            "INFO",
            &format!(
                "HELLO received: board_mac={board_mac} firmware={}.{}.{}",
                hello.firmware.0, hello.firmware.1, hello.firmware.2
            ),
        );
    }

    /// Whether a board `HELLO` has been observed since the last
    /// disconnect (D15 latch). The daemon reports it as
    /// `ConnectionInfo.board_reachable`.
    #[must_use]
    pub fn board_reachable(&self) -> bool {
        self.board_reachable.load(Ordering::SeqCst)
    }

    /// Clears the [`Self::board_reachable`] latch (D15): on
    /// disconnect / transport switch / `NoRoute` / HELLO-timeout.
    pub fn clear_board_reachable(&self) {
        self.board_reachable.store(false, Ordering::SeqCst);
    }

    /// Board MAC of the last observed `HELLO`, when one was seen.
    /// The daemon uses it for the `RSH-XXXX` SSID derivation and
    /// the device-key migration (D6).
    #[must_use]
    pub fn last_observed_board_mac(&self) -> Option<String> {
        lock(&self.session_last_hello)
            .as_ref()
            .map(|observed| observed.board_mac.clone())
    }

    /// Current Wi-Fi status (D20): the board hotspot SSID derived
    /// from the last observed `HELLO` (`RSH-XXXX`, D4), whether
    /// the board is connected (live session), and whether a Wi-Fi
    /// password is stored for this board. Never carries a password.
    ///
    /// R3-D-08: the `password_set` Credential Manager read is
    /// cached (60 s TTL, per board — see [`WifiPasswordCache`]),
    /// so the daemon's ~1 Hz slow poll does not hit the store
    /// every cycle. Set / clear invalidates it (see
    /// [`Self::invalidate_wifi_password_cache`]).
    #[must_use]
    pub fn wifi_status(&self) -> rsh_api::WifiStatus {
        use rsh_api::ssid_for_board_mac;
        let board_mac = self.last_observed_board_mac();
        let ssid = board_mac.as_deref().and_then(ssid_for_board_mac);
        let password_set = board_mac.as_deref().is_some_and(|mac| {
            let now = Instant::now();
            if let Some(cached) = lock(&self.wifi_password_cache).get(mac, now) {
                return cached;
            }
            let probed = KeyStore::new().get_wifi_password(mac).is_ok();
            lock(&self.wifi_password_cache).store(mac, probed, now);
            probed
        });
        rsh_api::WifiStatus {
            ssid,
            connected: self.session_gate_open(),
            password_set,
            board_mac,
        }
    }

    /// Drop the cached Wi-Fi password-set flag (R3-D-08): the
    /// password was set or cleared, so the next `wifi_status()`
    /// re-probes the credential store instead of serving the stale
    /// flag for up to 60 s.
    fn invalidate_wifi_password_cache(&self) {
        lock(&self.wifi_password_cache).invalidate();
    }

    /// Clears the live session records: the activation (caps /
    /// firmware), the last observed `HELLO`, and the last activation
    /// error. Called on
    /// disconnect, transport loss, and engine stop — a later
    /// reconnect is a new session and must reconnect (SEC-E-14);
    /// stale records must never present a dead session as live.
    /// Also clears the board-IP binding (M-1) and the replacement
    /// requirement (M-7).
    fn clear_session_records(&self) {
        *lock(&self.session_activation) = None;
        *lock(&self.session_last_hello) = None;
        *lock(&self.session_last_error) = None;
        *lock(&self.session_last_error_typed) = None;
        *lock(&self.session_board_ip) = None;
        self.board_reachable.store(false, Ordering::SeqCst);
    }

    /// Live button bitmask (H10, v2.0.2): the raw combined mask
    /// (`buttons | buttons_ex << 32`) from the last button edge
    /// frame — bit `n - 1` is wheel button `n` (1–64). `0` = none
    /// pressed / no frame yet.
    #[must_use]
    pub fn pressed_button_mask(&self) -> u64 {
        *lock(&self.last_buttons)
    }

    /// The board firmware version from the live session's `GET_CAPS`
    /// reply (`"4.0.1"`), for the `get_firmware_version` command.
    ///
    /// # Errors
    ///
    /// Returns a message when no session is live — the version comes
    /// from the authenticated board, never from a local guess, so
    /// there is nothing truthful to return until the activator has
    /// run.
    pub fn firmware_version(&self) -> Result<String, String> {
        match self.session_activation() {
            Some(activation) => Ok(format!(
                "{}.{}.{}",
                activation.firmware.0, activation.firmware.1, activation.firmware.2
            )),
            None => Err(
                "no authenticated session yet — connect the wheel and let the session \
                 activate (SEC-E-14) before reading the firmware version"
                    .to_string(),
            ),
        }
    }

    /// True while a forwarder run is waiting out its own retry
    /// backoff (R3-D-02): either the frame loop's retry deadline
    /// after a failed activation, or the backoff sleep in
    /// `forwarder_retry_after_failure` after a transport loss. The
    /// forwarder owns the retry in that window — the manager loop
    /// skips its `RetryTimer` and `connect_session` /
    /// `connect_transport` no-op instead of aborting the forwarder
    /// and resetting its breaker streak.
    #[must_use]
    pub fn forwarder_retry_waiting(&self) -> bool {
        self.forwarder_retry_wait.load(Ordering::SeqCst) > 0
    }

    /// True once a forwarder run has ended with its circuit breaker
    /// open (R3-D-02): no automatic retry may start a fresh
    /// forwarder — only an explicit user Connect (via
    /// `connect_transport`, which clears the latch) may.
    #[must_use]
    pub fn forwarder_breaker_open(&self) -> bool {
        self.forwarder_breaker_open.load(Ordering::SeqCst)
    }

    /// Retries session activation on the live transport (the explicit
    /// user action: connect / retry). Re-issues the connect for the
    /// currently requested transport kind — the supervisor aborts the
    /// old forwarder and spawns a new one, which runs the activator
    /// again — and returns the current transport state. Activation
    /// itself is asynchronous (the forwarder runs it); the caller
    /// polls `transport_state` for the outcome — `session_active`
    /// turns true only once an unsigned `GET_CAPS` succeeds.
    ///
    /// R2-28: this no-ops (returns the current state) when the
    /// session gate is already open — a live session needs no
    /// re-activation — and when a forwarder activation is already
    /// in flight. Without the second guard, a manager `RetryTimer`
    /// firing mid-activation tore the forwarder down through
    /// `connect_transport` (supervisor abort + respawn) and
    /// restarted the HELO → unsigned `GET_CAPS` exchange from
    /// scratch, wasting attempts. The forwarder owns its own retry
    /// timing while its activation runs; the manager's timer only
    /// matters once that attempt has finished.
    ///
    /// R3-D-02: this also no-ops while the forwarder is waiting out
    /// its own retry backoff (`forwarder_retry_waiting`) or its
    /// circuit breaker is open (`forwarder_breaker_open`). The old
    /// code ran `connect_transport` in both windows: the supervisor
    /// unconditionally aborted the running forwarder and spawned a
    /// fresh one with streak 0, so the 10-failure breaker could
    /// never trip while the manager retried. One loop owns the
    /// retry now — the forwarder's — and a tripped breaker stays
    /// tripped until the user reconnects explicitly (via
    /// `connect_transport`, which clears the latch).
    ///
    /// # Errors
    ///
    /// Returns a message when no transport can be (re)connected (for
    /// example no serial board port is detected).
    pub fn connect_session(self: &Arc<Self>) -> Result<TransportState, String> {
        if self.session_gate_open()
            || self.activation_in_flight.load(Ordering::SeqCst)
            || self.forwarder_retry_waiting()
            || self.forwarder_breaker_open()
        {
            return Ok(self.transport_state());
        }
        let (kind, _) = self.transport_request();
        self.connect_transport(kind)?;
        Ok(self.transport_state())
    }

    /* ------------------------------------------------------------------ */
    /* Auto-switch (C1): watcher + store source + 2 Hz poll task        */
    /* ------------------------------------------------------------------ */

    /// Queues an app-level event for the frontend (C-EVT-1). The
    /// emitter thread drains the queue (`drain_app_events`) and emits
    /// each through the Tauri window. Queueing never fails: a full
    /// queue drops the oldest event rather than blocking the engine.
    pub fn queue_app_event(&self, name: &str, payload: serde_json::Value) {
        let mut events = lock(&self.pending_events);
        if events.len() >= 64 {
            events.remove(0);
        }
        events.push((name.to_string(), payload));
    }

    /// Drains the queued app-level events (C-EVT-1), oldest first.
    /// The emitter thread calls this and emits each entry.
    #[must_use]
    pub fn drain_app_events(&self) -> Vec<(String, serde_json::Value)> {
        std::mem::take(&mut *lock(&self.pending_events))
    }

    /// Applies one auto-switch decision from the watcher: activates
    /// the profile as an *automatic* switch (`manual = false`, so the
    /// manual-override lock clears) and persists the store. This is
    /// the watcher's apply closure — it never queues the event itself;
    /// the watcher returns the [`ProfileSwitchEvent`] and
    /// `poll_auto_switch_once` queues it.
    fn apply_auto_switch(&self, profile_id: &ProfileId) -> Result<(), String> {
        {
            let mut store = lock(&self.store);
            store
                .activate(profile_id.as_str(), false)
                .map_err(|error| error.to_string())?;
            store.save().map_err(|error| error.to_string())?;
        }
        // As with a manual switch: the old draft is gone and the new
        // profile's plan goes live.
        *lock(&self.draft) = None;
        self.push_pipeline_plan();
        Ok(())
    }

    /// Installs an auto-switch poller (test seam + the real wiring
    /// below). Replaces any installed poller. Used by
    /// `start_auto_switch` and by tests, which install a watcher over
    /// a fake source.
    pub fn install_auto_switch_poller(&self, poller: Box<dyn AutoSwitchPoller + Send>) {
        *lock(&self.auto_switch) = Some(poller);
    }

    /// Polls the installed auto-switch watcher once and queues any
    /// switch events for the frontend (C1 / C-EVT-1). A no-op when no
    /// poller is installed. Returns the number of events queued.
    pub fn poll_auto_switch_once(&self) -> usize {
        let events = {
            let mut slot = lock(&self.auto_switch);
            let Some(poller) = slot.as_mut() else {
                return 0;
            };
            poller.poll(Instant::now())
        };
        let count = events.len();
        for event in events {
            let payload = serde_json::to_value(&event).unwrap_or(serde_json::Value::Null);
            self.queue_app_event(PROFILE_SWITCHED_EVENT, payload);
        }
        count
    }

    /// Installs the real auto-switch watcher (C1) and spawns its 2 Hz
    /// poll thread: a [`TelemetryWatcher`] over a
    /// [`StoreProfileSource`] (the profile store as a
    /// [`ProfileSource`]), fed by the live [`TelemetryCarSource`] and
    /// the Windows foreground detector. The apply closure activates
    /// through this state (`manual = false`). The thread holds only a
    /// [`Weak`] to the state, so dropping the state ends the thread
    /// (it also stops when `stop_auto_switch` clears the flag).
    /// A no-op when a poller is already installed.
    pub fn start_auto_switch(self: &Arc<Self>) {
        if lock(&self.auto_switch).is_some() {
            return;
        }
        let weak = Arc::downgrade(self);
        let source: Box<dyn ProfileSource + Send> = Box::new(StoreProfileSource {
            state: weak.clone(),
        });
        let resolver = CarProfileResolver::new_owned(source);
        let current = lock(&self.store)
            .active()
            .map(|active| ProfileId::new(active.id.clone()));
        let apply_weak = weak.clone();
        let watcher = TelemetryWatcher::new(
            resolver,
            TelemetryCarSource::new(),
            WindowsForegroundDetector,
            move |profile_id: &ProfileId| {
                let Some(state) = apply_weak.upgrade() else {
                    return Err("app state dropped".to_string());
                };
                state.apply_auto_switch(profile_id)
            },
            current,
        );
        self.install_auto_switch_poller(Box::new(watcher));
        self.auto_switch_running.store(true, Ordering::SeqCst);
        let thread_weak = weak;
        std::thread::Builder::new()
            .name("rsh-auto-switch".to_string())
            .spawn(move || {
                loop {
                    std::thread::sleep(Duration::from_millis(500));
                    let Some(state) = thread_weak.upgrade() else {
                        break;
                    };
                    if !state.auto_switch_running.load(Ordering::SeqCst) {
                        break;
                    }
                    state.poll_auto_switch_once();
                }
            })
            .ok();
    }

    /// Stops the auto-switch poll thread (C1). The installed poller
    /// stays, so a later `start_auto_switch` resumes polling without
    /// rebuilding the watcher.
    pub fn stop_auto_switch(&self) {
        self.auto_switch_running.store(false, Ordering::SeqCst);
    }

    /// Whether the auto-switch poll thread is running.
    #[must_use]
    pub fn auto_switch_running(&self) -> bool {
        self.auto_switch_running.load(Ordering::SeqCst)
    }

    /// Enables auto-switch (H3): turns the setting on, clears the
    /// manual-override lock, persists, and starts the watcher thread
    /// when it is not running.
    ///
    /// # Errors
    ///
    /// Returns a message when the store save fails.
    pub fn enable_auto_switch(self: &Arc<Self>) -> Result<AppSettings, String> {
        {
            let mut store = lock(&self.store);
            store.enable_auto_switch();
            store.save().map_err(|error| error.to_string())?;
        }
        self.start_auto_switch();
        Ok(self.settings())
    }

    /* ------------------------------------------------------------------ */
    /* H5-control: the 11 remaining commands (live-session control)      */
    /*                                                                     */
    /* Every command below runs under a fresh session via                */
    /* [`run_control_session`] (flag-1 HELO → unsigned `GET_CAPS`      */
    /* first, fail-closed, session closed afterwards). They are         */
    /* synchronous: each                                                */
    /* builds a short-lived current-thread runtime and blocks on the    */
    /* helper — call only outside a tokio runtime (Tauri sync commands   */
    /* satisfy this). The transport comes from the requested kind        */
    /* (`transport_request().0`, default UDP): UDP uses an ephemeral     */
    /* local port (coexists with the forwarder's stream-port socket);    */
    /* serial refuses while the live transport holds the port (see       */
    /* [`build_serial_control_transport`](Self::build_serial_control_transport)). */
    /* ------------------------------------------------------------------ */

    /// Build a fresh UDP transport for one control session: the
    /// configured board IP, standard ports, local port 0 (ephemeral —
    /// coexists with the forwarder's stream-port socket; HELO/control
    /// replies return to the sender's port).
    fn build_udp_control_transport(&self) -> Result<UdpTransport, String> {
        let mut config = udp_config_for(self);
        config.local_port = 0;
        // M-1: bind the board IP this control session talks to, so
        // `upload_firmware`'s HTTP leg uses the authenticated board's
        // IP instead of a stale settings value.
        *lock(&self.session_board_ip) = Some(config.board_ip);
        UdpTransport::bind(config).map_err(|error| error.to_string())
    }

    /// Build a fresh serial transport for one control session.
    /// Refuses while the live transport holds the serial port: a
    /// second open fails on Windows and would split reads between two
    /// readers on Linux — fail-closed, deterministic,
    /// platform-independent. The user disconnects the transport
    /// first, then retries.
    fn build_serial_control_transport(&self) -> Result<SerialTransport, String> {
        if matches!(
            self.engine_status(),
            EngineTaskStatus::Running {
                transport: Some(TransportKind::Serial)
            }
        ) {
            return Err(
                "the serial port is held by the live transport — disconnect the transport \
                 first, then retry"
                    .to_string(),
            );
        }
        let port = detect_board_port().ok_or_else(|| {
            "no board serial port detected — connect the wheel over USB and retry".to_string()
        })?;
        SerialTransport::open(SerialConfig::new(port)).map_err(|error| error.to_string())
    }

    /// The recorded config-transaction snapshot, with a completed
    /// (`Confirmed`) transaction normalized to `Idle`: a confirmed
    /// transaction is complete, so the next stage starts a new cycle.
    /// (Stage is illegal from `Committed` — the caller must confirm
    /// or roll back first — and legal from `Idle` / `Staged` /
    /// `RolledBack`.)
    fn config_snapshot_for_stage(&self) -> ConfigTxnSnapshot {
        let mut snapshot = lock(&self.config_txn).clone();
        if snapshot.state == ConfigState::Confirmed {
            snapshot.state = ConfigState::Idle;
            snapshot.staged_blob = None;
            snapshot.committed_at = None;
        }
        snapshot
    }

    /// Change the board AP password (`set_password`): the new
    /// password is stored as the Wi-Fi / OTA password too (one
    /// credential for both). On success the session gate closes and
    /// the session records clear — the board restarts its AP after
    /// replying, so the session is dead. The daemon updates the
    /// Windows WLAN profile with the new password in the same
    /// `set_wifi_password` call (D8) and rejoins, so the PC
    /// reconnects with the new password.
    ///
    /// # Errors
    ///
    /// Returns a message when no board answers, the session fails to
    /// activate, the password violates the 8–63 printable-ASCII
    /// rules, or the new password could not be stored (in the last
    /// case the board password DID change — the error says so).
    pub fn set_password(self: &Arc<Self>, password: &str) -> Result<(), String> {
        // Owned here so the control future borrows only the session
        // (its other inputs are moved in — see `run_control_session`).
        let password = password.to_string();
        let result = match self.transport_request().0 {
            TransportKind::Udp => block_on_control(run_control_session(
                self,
                self.build_udp_control_transport()?,
                KeyStore::new(),
                |session| Box::pin(op_set_password(session, password)),
            )),
            TransportKind::Serial => block_on_control(run_control_session(
                self,
                self.build_serial_control_transport()?,
                KeyStore::new(),
                |session| Box::pin(op_set_password(session, password)),
            )),
        };
        if result.is_ok() {
            self.close_session_gate();
            self.clear_session_records();
            // R3-D-08: the stored password changed — the cached
            // `password_set` flag must not serve the old value.
            self.invalidate_wifi_password_cache();
        }
        result
    }

    /// Runs an LED test pattern on the board strip (C-LED-1): the
    /// pattern + brightness from the LED tuning screen (or the
    /// settings-apply path). Brightness defaults to the stored LED
    /// tuning's brightness, else 100; the period defaults to 1000 ms
    /// and must be 100–5000 ms. The brightness scales the RGB values
    /// before the payload is encoded (see [`build_led_test_pattern`]).
    ///
    /// # Errors
    ///
    /// Returns a message when the period is out of range, no board
    /// answers, or the session fails to activate.
    pub fn led_test(
        self: &Arc<Self>,
        pattern: LedPattern,
        brightness: Option<u8>,
        period_ms: Option<u16>,
    ) -> Result<(), String> {
        self.led_test_with_color(pattern, brightness, period_ms, None)
    }

    /// [`Self::led_test`] with an explicit flash colour (M2, v2.0.2):
    /// `Some(0xRRGGBB)` tints the flash; `None` reads the stored LED
    /// colour from the active profile when one is stored, else white.
    /// Used by `set_tuning` so a colour change flashes its own colour.
    ///
    /// # Errors
    ///
    /// Returns an error when the test pattern is invalid (period
    /// outside 100..=5000 ms) or the board control session cannot be
    /// built / run.
    pub fn led_test_with_color(
        self: &Arc<Self>,
        pattern: LedPattern,
        brightness: Option<u8>,
        period_ms: Option<u16>,
        color: Option<u32>,
    ) -> Result<(), String> {
        let brightness = brightness.unwrap_or_else(|| {
            lock(&self.store)
                .active()
                .and_then(|active| {
                    active
                        .profile
                        .module_tunings
                        .get("led")
                        .and_then(|tuning| tuning.led_brightness)
                })
                .unwrap_or(100)
        });
        let color = color.or_else(|| {
            lock(&self.store).active().and_then(|active| {
                active
                    .profile
                    .module_tunings
                    .get("led")
                    .and_then(|tuning| tuning.led_color.as_deref())
                    .and_then(parse_hex_color)
            })
        });
        let payload = build_led_test_pattern_with_color(pattern, brightness, period_ms, color)?;
        match self.transport_request().0 {
            TransportKind::Udp => block_on_control(run_control_session(
                self,
                self.build_udp_control_transport()?,
                KeyStore::new(),
                |session| Box::pin(op_led_test(session, payload)),
            )),
            TransportKind::Serial => block_on_control(run_control_session(
                self,
                self.build_serial_control_transport()?,
                KeyStore::new(),
                |session| Box::pin(op_led_test(session, payload)),
            )),
        }
    }

    /// Forget the Wi-Fi password on the PC side (`key_reset`,
    /// D7): deletes the stored `:wifi` password for the current
    /// board, closes the session gate, and clears the session
    /// records. There is NO wire opcode anymore — the board-side
    /// password is cleared only by holding BOOT ≥ 10 s on the
    /// board itself (firmware clears it and reboots to open AP),
    /// so the caller must tell the user that if the board still
    /// has a password. A missing stored password is already-reset
    /// (`NotFound` is fine).
    ///
    /// # Errors
    ///
    /// Returns a message when the stored password cannot be deleted
    /// for a reason other than "not stored".
    pub fn key_reset(self: &Arc<Self>) -> Result<(), String> {
        if let Some(board_id) = self
            .session_activation()
            .map(|activation| activation.board_id)
            .or_else(|| self.last_observed_board_mac())
        {
            match KeyStore::new().delete_wifi_password(&board_id) {
                Ok(()) | Err(KeystoreError::NotFound) => {}
                Err(error) => {
                    return Err(format!(
                        "could not delete the stored Wi-Fi password for {board_id}: {error}"
                    ));
                }
            }
        }
        self.close_session_gate();
        self.clear_session_records();
        Ok(())
    }

    /// Factory-resets the board (`factory_reset`, M-6): clears the
    /// board's config and Wi-Fi password. On success the gate
    /// closes and the session records clear — the board reboots to
    /// an open AP afterwards (set a fresh password in Settings).
    ///
    /// # Errors
    ///
    /// Returns a message when no board answers or the session fails
    /// to activate.
    pub fn factory_reset(self: &Arc<Self>) -> Result<(), String> {
        let result = match self.transport_request().0 {
            TransportKind::Udp => block_on_control(run_control_session(
                self,
                self.build_udp_control_transport()?,
                KeyStore::new(),
                |session| Box::pin(op_factory_reset(session)),
            )),
            TransportKind::Serial => block_on_control(run_control_session(
                self,
                self.build_serial_control_transport()?,
                KeyStore::new(),
                |session| Box::pin(op_factory_reset(session)),
            )),
        };
        if result.is_ok() {
            self.close_session_gate();
            self.clear_session_records();
        }
        result
    }

    /// Read the board's live config blob (`get_config`) as the JSON
    /// shape `stage_config` accepts: `cfgVersion`, the full
    /// `records` list, and the derived `buttons` view of the three
    /// fixed `BUTTON` slot roles.
    ///
    /// # Errors
    ///
    /// Returns a message when no board answers, the session fails to
    /// activate, or the board returns a blob that does not decode.
    pub fn get_config(self: &Arc<Self>) -> Result<serde_json::Value, String> {
        let blob = match self.transport_request().0 {
            TransportKind::Udp => block_on_control(run_control_session(
                self,
                self.build_udp_control_transport()?,
                KeyStore::new(),
                |session| Box::pin(op_read_config_blob(session)),
            )),
            TransportKind::Serial => block_on_control(run_control_session(
                self,
                self.build_serial_control_transport()?,
                KeyStore::new(),
                |session| Box::pin(op_read_config_blob(session)),
            )),
        }?;
        let decoded = ConfigBlob::decode(&blob).map_err(|error| {
            format!("board returned a config blob that does not decode: {error}")
        })?;
        Ok(config_blob_to_json(&decoded))
    }

    /// Stage a config blob (`stage_config`): rebuild the blob from
    /// the JSON shape (records authoritative; `buttons` reconciled
    /// against the three fixed `BUTTON` slot roles — a cross-role
    /// rebind is rejected, see [`config_json_to_blob`]), then stage
    /// it under a fresh session (client-side PIN-RULES validation
    /// first, live config saved for rollback). The staged blob is
    /// NOT live until `commit_config` + `confirm_config`.
    ///
    /// # Errors
    ///
    /// Returns a message for a malformed config, a blob that fails
    /// board validation, a `buttons` inconsistency, no board answer,
    /// or a failed session activation.
    pub fn stage_config(self: &Arc<Self>, config: &serde_json::Value) -> Result<(), String> {
        let blob = config_json_to_blob(config)?;
        let blob_bytes = blob
            .encode()
            .map_err(|error| format!("config blob could not be encoded: {error}"))?;
        let snapshot = self.config_snapshot_for_stage();
        let (result, new_snapshot) = match self.transport_request().0 {
            TransportKind::Udp => block_on_control(run_control_session(
                self,
                self.build_udp_control_transport()?,
                KeyStore::new(),
                |session| Box::pin(op_stage_config(session, snapshot, blob_bytes)),
            ))?,
            TransportKind::Serial => block_on_control(run_control_session(
                self,
                self.build_serial_control_transport()?,
                KeyStore::new(),
                |session| Box::pin(op_stage_config(session, snapshot, blob_bytes)),
            ))?,
        };
        *lock(&self.config_txn) = new_snapshot;
        result
    }

    /// `COMMIT` the staged config (`commit_config`): the board
    /// validates the staged blob, live-applies it, and starts the
    /// 30 s confirm window. Only legal after `stage_config`.
    ///
    /// # Errors
    ///
    /// Returns a message when nothing is staged, the board rejects
    /// the commit (pin conflict / unsupported, with its errCodes),
    /// no board answers, or the session fails to activate.
    pub fn commit_config(self: &Arc<Self>) -> Result<(), String> {
        let snapshot = lock(&self.config_txn).clone();
        let (result, new_snapshot) = match self.transport_request().0 {
            TransportKind::Udp => block_on_control(run_control_session(
                self,
                self.build_udp_control_transport()?,
                KeyStore::new(),
                |session| Box::pin(op_commit_config(session, snapshot)),
            ))?,
            TransportKind::Serial => block_on_control(run_control_session(
                self,
                self.build_serial_control_transport()?,
                KeyStore::new(),
                |session| Box::pin(op_commit_config(session, snapshot)),
            ))?,
        };
        *lock(&self.config_txn) = new_snapshot;
        result
    }

    /// `CONFIRM` the committed config (`confirm_config`): makes the
    /// new config permanent. Only legal inside the 30 s window after
    /// `commit_config`. The wall clock is checked first: an expired
    /// window errors WITHOUT sending `CONFIRM` (the board has already
    /// rolled back on its own) — the state stays `Committed` until
    /// `rollback_config` verifies the board is back on the previous
    /// config.
    ///
    /// # Errors
    ///
    /// Returns a message when nothing is committed, the window has
    /// expired, the confirm is not acknowledged, no board answers,
    /// or the session fails to activate.
    pub fn confirm_config(self: &Arc<Self>) -> Result<(), String> {
        let snapshot = lock(&self.config_txn).clone();
        if snapshot.state == ConfigState::Committed
            && let Some(committed_at) = snapshot.committed_at
            && committed_at.elapsed() >= snapshot.commit_deadline
        {
            return Err(
                "the 30 s confirm window has expired — the board has already rolled back on \
                 its own; run rollback_config to verify the board is back on the previous config"
                    .to_string(),
            );
        }
        let (result, new_snapshot) = match self.transport_request().0 {
            TransportKind::Udp => block_on_control(run_control_session(
                self,
                self.build_udp_control_transport()?,
                KeyStore::new(),
                |session| Box::pin(op_confirm_config(session, snapshot)),
            ))?,
            TransportKind::Serial => block_on_control(run_control_session(
                self,
                self.build_serial_control_transport()?,
                KeyStore::new(),
                |session| Box::pin(op_confirm_config(session, snapshot)),
            ))?,
        };
        *lock(&self.config_txn) = new_snapshot;
        result
    }

    /// Roll back (`rollback_config`): send `REVERT` and verify the
    /// board reports the last-good config (read back via `GET_CFG`
    /// and compared against the pre-stage blob). Legal from `Staged`
    /// (explicit cancel) and `Committed` (confirm timeout / late
    /// confirm / expired window). On success the
    /// `rsh://config-rollback` event is queued (C-EVT-1).
    ///
    /// # Errors
    ///
    /// Returns a message when nothing is staged or committed, the
    /// read-back does not match the last-good blob, no board answers,
    /// or the session fails to activate.
    pub fn rollback_config(self: &Arc<Self>) -> Result<(), String> {
        let snapshot = lock(&self.config_txn).clone();
        let (result, new_snapshot) = match self.transport_request().0 {
            TransportKind::Udp => block_on_control(run_control_session(
                self,
                self.build_udp_control_transport()?,
                KeyStore::new(),
                |session| Box::pin(op_rollback_config(session, snapshot)),
            ))?,
            TransportKind::Serial => block_on_control(run_control_session(
                self,
                self.build_serial_control_transport()?,
                KeyStore::new(),
                |session| Box::pin(op_rollback_config(session, snapshot)),
            ))?,
        };
        *lock(&self.config_txn) = new_snapshot;
        if result.is_ok() {
            self.queue_app_event(
                CONFIG_ROLLBACK_EVENT,
                serde_json::json!({ "state": "rolled_back" }),
            );
        }
        result
    }

    /// The config transaction state (`get_config_state`) — the
    /// recorded state, with no board call. Frontend names: `Idle` /
    /// `Confirmed` → `"clean"`, `Staged` → `"staged"`, `Committed` →
    /// `"pending_confirm"`, `RolledBack` → `"rolled_back"`. After the
    /// 30 s window expires the board has self-rolled-back but the
    /// record still says `"pending_confirm"` until `rollback_config`
    /// verifies — the record is the last verified state, not a guess
    /// about the board.
    #[must_use]
    pub fn config_state(&self) -> &'static str {
        match lock(&self.config_txn).state {
            ConfigState::Idle | ConfigState::Confirmed => "clean",
            ConfigState::Staged => "staged",
            ConfigState::Committed => "pending_confirm",
            ConfigState::RolledBack => "rolled_back",
        }
    }

    /// Read the pin map (`get_pin_map`): the single-pin functions
    /// the Settings editor can round-trip (steering, accelerator,
    /// brake, clutch, handbrake, horn, led). Multi-pin records
    /// (shifter, encoders, button matrix), the shifter up/down
    /// `BUTTON` slots, and battery / I²C records are omitted — the
    /// editor cannot express them, and `set_pin_map` preserves them
    /// untouched.
    ///
    /// # Errors
    ///
    /// Returns a message when no board answers, the session fails to
    /// activate, or the board returns a blob that does not decode.
    pub fn get_pin_map(self: &Arc<Self>) -> Result<Vec<PinAssignment>, String> {
        let blob = match self.transport_request().0 {
            TransportKind::Udp => block_on_control(run_control_session(
                self,
                self.build_udp_control_transport()?,
                KeyStore::new(),
                |session| Box::pin(op_read_config_blob(session)),
            )),
            TransportKind::Serial => block_on_control(run_control_session(
                self,
                self.build_serial_control_transport()?,
                KeyStore::new(),
                |session| Box::pin(op_read_config_blob(session)),
            )),
        }?;
        let decoded = ConfigBlob::decode(&blob).map_err(|error| {
            format!("board returned a config blob that does not decode: {error}")
        })?;
        Ok(records_to_pin_assignments(&decoded.records))
    }

    /// Set the pin map (`set_pin_map`): the submission is the
    /// COMPLETE map (the Settings editor always submits its full
    /// draft). It is validated first (an invalid map touches no
    /// board), then merged into the live blob by role and staged +
    /// committed under one fresh session, so the editor's 30 s
    /// confirm bar follows immediately. A merge failure is a
    /// validation verdict (`valid: false` with the reason), not an
    /// error — nothing is staged. Omitted expressible functions are
    /// removed from the blob (replace semantics); multi-pin and
    /// other unexpressible live records are preserved.
    ///
    /// # Errors
    ///
    /// Returns a message when no board answers, the session fails to
    /// activate, or stage/commit fails on the board.
    pub fn set_pin_map(
        self: &Arc<Self>,
        pins: Vec<PinAssignment>,
    ) -> Result<PinMapValidation, String> {
        let validation = validate_pin_map(&pins);
        if !validation.valid {
            return Ok(validation);
        }
        let snapshot = self.config_snapshot_for_stage();
        let (result, new_snapshot) = match self.transport_request().0 {
            TransportKind::Udp => block_on_control(run_control_session(
                self,
                self.build_udp_control_transport()?,
                KeyStore::new(),
                |session| Box::pin(op_stage_and_commit_pin_map(session, snapshot, pins)),
            ))?,
            TransportKind::Serial => block_on_control(run_control_session(
                self,
                self.build_serial_control_transport()?,
                KeyStore::new(),
                |session| Box::pin(op_stage_and_commit_pin_map(session, snapshot, pins)),
            ))?,
        };
        *lock(&self.config_txn) = new_snapshot;
        result
    }

    /// Upload a firmware image (`upload_firmware`) over the board's
    /// `WebOTA` server — the real board OTA path, never a fake upload:
    ///
    /// 1. A fresh session reads the live firmware version from the
    ///    session's `GET_CAPS` (either transport — the version check
    ///    works over serial too).
    /// 2. The image is verified: Ed25519 signature + format +
    ///    anti-downgrade vs the live version (an older image is
    ///    rejected; an equal version is a reinstall).
    /// 3. The Wi-Fi password comes from the keystore (missing →
    ///    "set a Wi-Fi password in Settings first").
    /// 4. The image uploads over HTTP to the board's AP address
    ///    (settings `esp_ip`, default `192.168.4.1`) — the PC must be
    ///    joined to the board's Wi-Fi network for this leg; over
    ///    serial the session works but the HTTP leg fails honestly
    ///    when the PC is not on the board AP.
    /// 5. On success the gate closes and the session records clear —
    ///    the board restarts about a second later. The post-upload
    ///    version confirm is the caller's job on reconnect, via
    ///    `get_firmware_version` (per SIGNED-OTA §3: HTTP 200 +
    ///    `"OK"` alone is not a version confirm).
    ///
    /// # Errors
    ///
    /// Returns a message when the image file cannot be read, no
    /// board answers, the image fails verification, no Wi-Fi
    /// password is stored, or the board rejects the upload.
    pub fn upload_firmware(self: &Arc<Self>, path: &Path) -> Result<(), String> {
        // M-1: the HTTP upload leg needs the board's Wi-Fi interface.
        // Over USB-serial the PC may not reach the board's WebOTA
        // server at all, so the serial leg is refused outright — the
        // user connects over Wi-Fi (board AP) and retries.
        if self.transport_request().0 == TransportKind::Serial {
            return Err(
                "firmware upload over USB-serial is refused: the HTTP upload leg needs the \
                 board's Wi-Fi interface — connect over Wi-Fi (board AP) and retry"
                    .to_string(),
            );
        }
        let bytes = std::fs::read(path).map_err(|error| {
            format!(
                "could not read the firmware image at {}: {error}",
                path.display()
            )
        })?;
        let info: LiveBoardInfo = block_on_control(run_control_session(
            self,
            self.build_udp_control_transport()?,
            KeyStore::new(),
            |session| Box::pin(std::future::ready(op_live_board_info(session))),
        ))?;
        verify_firmware_image(&bytes, info.firmware)
            .map_err(|error| format!("firmware image failed verification: {error}"))?;
        let password = match KeyStore::new().get_wifi_password(&info.board_id) {
            Ok(password) => password,
            Err(KeystoreError::NotFound) => {
                return Err(
                    "no Wi-Fi password stored for this board — set a Wi-Fi password in \
                     Settings first, then retry the upload"
                        .to_string(),
                );
            }
            Err(error) => {
                return Err(format!("could not read the stored Wi-Fi password: {error}"));
            }
        };
        // M-1: the HTTP host is the IP the live session authenticated
        // against (recorded when the control transport was built), not
        // a settings value that may have changed since.
        let host = lock(&self.session_board_ip).unwrap_or_else(|| udp_config_for(self).board_ip);
        upload_firmware_image(host, &password, &bytes)?;
        self.close_session_gate();
        self.clear_session_records();
        Ok(())
    }

    /// Reboot the board (`restart_wheel`). On success the gate closes
    /// and the session records clear — the board takes seconds to
    /// reboot, and the forwarder has no re-activation retry, so there
    /// is NO auto reconnect: the user reconnects once the board is
    /// back (a pre-existing H5-activator limitation, not a gap in
    /// this command).
    ///
    /// # Errors
    ///
    /// Returns a message when no board answers or the session fails
    /// to activate.
    pub fn restart_wheel(self: &Arc<Self>) -> Result<(), String> {
        let result = match self.transport_request().0 {
            TransportKind::Udp => block_on_control(run_control_session(
                self,
                self.build_udp_control_transport()?,
                KeyStore::new(),
                |session| Box::pin(op_reboot(session)),
            )),
            TransportKind::Serial => block_on_control(run_control_session(
                self,
                self.build_serial_control_transport()?,
                KeyStore::new(),
                |session| Box::pin(op_reboot(session)),
            )),
        };
        if result.is_ok() {
            self.close_session_gate();
            self.clear_session_records();
        }
        result
    }

    /// Live transport + session state for the `get_transport_state`
    /// command: which transport is live, whether frames reach the
    /// engine, and how many the gate has held. While idle the
    /// transport reports the last requested kind (startup default
    /// `"wifi"`) with the persisted `esp_ip` as its address, so the
    /// shape is always complete; `state` is `disconnected` there. A
    /// failed engine reports `error` with the failure reason verbatim
    /// instead of a silent `disconnected`.
    /// USB board-port cache TTL (DS-27). A plugged/unplugged board is
    /// picked up within this bound; the poll path never re-detects
    /// more often than this.
    const BOARD_PORT_CACHE_TTL: Duration = Duration::from_secs(5);

    /// Cached USB board port (R2-25): a pure last-known read — it
    /// NEVER probes. The background refresher thread (see
    /// [`Self::start_board_port_refresher`]) fills this cache — on
    /// Serial only, every [`Self::BOARD_PORT_CACHE_TTL`] while the
    /// board is found, backing off to 30 s while it keeps missing
    /// (PERF F-5); `transport_state()` on the 20 Hz poll path reads
    /// it without ever enumerating ports or waiting out a 500 ms
    /// HELO probe per candidate. [`None`] until the first refresh
    /// (or after [`Self::rescan_board_port`]).
    #[must_use]
    pub fn cached_board_port(&self) -> Option<String> {
        lock(&self.cached_serial_port)
            .clone()
            .and_then(|(port, _)| port)
    }

    /// Current slow-poll generation (PERF F-6): the daemon's
    /// `slow_poll` cache key — see [`Self::note_slow_state_changed`].
    #[must_use]
    pub fn slow_state_generation(&self) -> u64 {
        self.slow_state_generation.load(Ordering::SeqCst)
    }

    /// Bumps the slow-poll generation: call (only) after a
    /// successful mutation of state the ~1 Hz `slow_poll` reports
    /// — profiles, games, tunings, modules, bindings, knobs,
    /// drafts, calibration, vJoy / firmware status — so the
    /// daemon's cached static groups are re-read on the next poll
    /// instead of waiting out their TTL. Cheap (one atomic add);
    /// callers must only bump on success, never on a failed
    /// attempt, or the cache refetches for nothing.
    fn note_slow_state_changed(&self) {
        self.slow_state_generation.fetch_add(1, Ordering::SeqCst);
    }

    /// Backoff cap for the board-port refresher (PERF F-5): when
    /// the board is repeatedly not found on Serial, the refresher
    /// backs off to this interval instead of re-scanning every 5 s;
    /// off Serial it waits this long between (probeless) wakeups.
    const BOARD_PORT_BACKOFF_MAX: Duration = Duration::from_secs(30);

    /// Wait between board-port refreshes (PERF F-5): pure, so the
    /// cadence is unit-testable without hardware.
    ///
    /// - Not Serial (the default Wi-Fi transport): the refresher
    ///   never probes at all — it waits out the backoff cap and
    ///   only wakes early on a rescan request (transport change /
    ///   disconnect). A USB board that is never used costs zero
    ///   serial-stack wakeups.
    /// - Serial with a board found (`misses == 0`): every
    ///   `BOARD_PORT_CACHE_TTL` (5 s), as before — a plugged /
    ///   unplugged board is picked up within that bound.
    /// - Serial with `misses` consecutive misses: 5 s × 2^misses,
    ///   capped at 30 s — a missing board is re-checked less and
    ///   less often instead of churning the serial stack forever.
    fn board_port_refresh_delay(serial: bool, misses: u32) -> Duration {
        if !serial {
            return Self::BOARD_PORT_BACKOFF_MAX;
        }
        if misses == 0 {
            return Self::BOARD_PORT_CACHE_TTL;
        }
        let shift = misses.min(3);
        let secs = Self::BOARD_PORT_CACHE_TTL.as_secs() << shift;
        Duration::from_secs(secs.min(Self::BOARD_PORT_BACKOFF_MAX.as_secs()))
    }

    /// Starts the background board-port refresher thread (R2-25,
    /// PERF F-5): it probes (`detect_board_port` — full enumeration
    /// plus a HELO probe) on a detached thread and stores the result in
    /// the cache — the poll path never probes. Idempotent: a second
    /// call is a no-op. The thread holds only a `Weak` reference
    /// and exits when the state is dropped.
    ///
    /// PERF F-5: the probe runs ONLY while the requested transport
    /// is Serial (or a rescan was requested). On the default Wi-Fi
    /// transport the thread sleeps on its condvar for 30 s at a
    /// time and probes nothing — the old code enumerated + probed
    /// every 5 s even when no USB board could ever be used. While
    /// Serial and the board keeps missing, the wait backs off
    /// 5 s → 10 s → 20 s → 30 s (see
    /// [`Self::board_port_refresh_delay`]); a rescan request wakes
    /// the thread at once, so switching to Serial still finds a
    /// plugged board promptly.
    pub fn start_board_port_refresher(self: &Arc<Self>) {
        if self
            .board_port_refresher_started
            .swap(true, Ordering::SeqCst)
        {
            return;
        }
        let weak = Arc::downgrade(self);
        let _ = std::thread::Builder::new()
            .name("rsh-board-port-refresh".to_string())
            .spawn(move || {
                let mut misses: u32 = 0;
                loop {
                    let Some(state) = weak.upgrade() else {
                        return;
                    };
                    let serial = state.transport_request().0 == TransportKind::Serial;
                    // Take a pending rescan request (set by
                    // `rescan_board_port`): it forces one probe even
                    // when the transport is not Serial — e.g. the
                    // user just picked USB and the cache was cleared.
                    let rescan = {
                        let (flag, _) = &state.board_port_wake;
                        std::mem::replace(&mut *lock(flag), false)
                    };
                    if serial || rescan {
                        let port = detect_board_port();
                        misses = if port.is_some() {
                            0
                        } else {
                            misses.saturating_add(1)
                        };
                        *lock(&state.cached_serial_port) = Some((port, Instant::now()));
                    }
                    let delay = Self::board_port_refresh_delay(serial, misses);
                    // Wait out the delay, or until a rescan wakes
                    // the thread early (transport change /
                    // disconnect). A spurious wake just re-loops
                    // and re-waits.
                    {
                        let (flag, condvar) = &state.board_port_wake;
                        let guard = lock(flag);
                        let _ = condvar
                            .wait_timeout(guard, delay)
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                    }
                    drop(state);
                }
            });
    }

    /// Clears the cached board port and wakes the background
    /// refresher so the next refresh re-detects at once (R2-25,
    /// PERF F-5): call after a disconnect or a transport change.
    /// Clearing only drops the last-known value — it never probes
    /// inline; the probe happens on the refresher thread, which
    /// this wakes even mid-backoff.
    pub fn rescan_board_port(&self) {
        *lock(&self.cached_serial_port) = None;
        let (flag, condvar) = &self.board_port_wake;
        *lock(flag) = true;
        condvar.notify_one();
    }

    /// Current transport state (kind + address + connection), for
    /// the API and the UI. See [`Self::transport_state_with_settings`]
    /// when the settings are already in hand.
    #[must_use]
    pub fn transport_state(&self) -> TransportState {
        self.transport_state_with_settings(&self.settings_arc())
    }

    /// `transport_state()` with the settings already in hand
    /// (DS-30): the UDP arm needs only `esp_ip`, so a caller that
    /// already holds the settings `Arc` (e.g. `poll()`) avoids a
    /// second store lock + struct clone.
    #[must_use]
    pub fn transport_state_with_settings(&self, settings: &AppSettings) -> TransportState {
        let status = self.engine_status();
        let live = match status {
            EngineTaskStatus::Running { transport } => transport,
            _ => None,
        };
        let kind = live.unwrap_or_else(|| self.transport_request().0);
        let address = match kind {
            TransportKind::Udp => settings.esp_ip.clone(),
            TransportKind::Serial => self.cached_board_port().unwrap_or_default(),
        };
        let mut state = *lock(&self.connection);
        // C-02 defence in depth: "Connected" is only ever reported
        // when the session gate is open (unsigned GET_CAPS passed).
        // A transport that is up but unauthenticated reports
        // Connecting, never Connected.
        if state == ConnectionState::Connected && !self.session_gate_open() {
            state = ConnectionState::Connecting;
        }
        let mut error = None;
        if let EngineTaskStatus::Failed { .. } = status {
            state = ConnectionState::Error;
            error = Some(self.engine_status_detail());
        } else if state == ConnectionState::Error {
            error = Some(self.engine_status_detail());
        }
        TransportState {
            transport: TransportState::kind_id(kind).to_string(),
            state,
            address,
            error,
            session_active: self.session_gate_open(),
            board_reachable: self.board_reachable(),
            frames_held_by_gate: self.session_gate_held_frames(),
        }
    }

    /* ------------------------------------------------------------------ */
    /* Engine stats + snapshots                                            */
    /* ------------------------------------------------------------------ */

    /// Current engine stats, from the stored metrics + the measured
    /// loop rate (`None` p99s while unmeasured — never a fake 0).
    #[must_use]
    pub fn engine_stats(&self) -> EngineStats {
        let metrics = *lock(&self.metrics);
        let loop_hz = lock(&self.loop_rate).rate_hz();
        build_engine_stats(&metrics, loop_hz)
    }

    /// The latest snapshot (built by the most recent `ingest_frame`;
    /// the default all-zero snapshot until the first engine tick).
    #[must_use]
    pub fn snapshot(&self) -> EngineSnapshot {
        lock(&self.latest_snapshot).as_ref().clone()
    }

    /// The latest snapshot as a shared pointer (DS-30): clones the
    /// `Arc`, not the struct — the 20 Hz `poll()` path takes this.
    #[must_use]
    pub fn snapshot_arc(&self) -> Arc<EngineSnapshot> {
        lock(&self.latest_snapshot).clone()
    }

    /// Feeds one engine tick into the state: ticks the loop-rate
    /// tracker, stores the metrics, updates the encoders, diffs the
    /// button bits into `rsh://button-press` edges, and builds + stores
    /// the next snapshot. Returns the snapshot built for this tick.
    /// Short-held locks only — the engine never waits on the UI.
    #[must_use]
    pub fn ingest_frame(&self, input: FrameInput) -> EngineSnapshot {
        lock(&self.loop_rate).tick();
        *lock(&self.metrics) = input.metrics;

        let encoders = lock(&self.encoders).update(&input.encoder_counts, input.n_enc);
        let seq = {
            let mut guard = lock(&self.seq);
            *guard += 1;
            *guard
        };

        let stats = self.engine_stats();
        let connection = *lock(&self.connection);
        let active_profile_id = lock(&self.store).active().map(|stored| stored.id.clone());
        let snapshot = build_snapshot_command(SnapshotInput {
            steering_deg: input.steering_deg,
            pedals: input.pedals,
            connection,
            stats,
            active_profile_id,
            seq,
            encoders,
            seq_gap: input.metrics.seq_gap,
            tx_fail: input.metrics.tx_fail,
            // M-3: handbrake + gear come from the pipeline output, so
            // the Overview always shows the live values, never stale
            // or hardcoded ones.
            handbrake: f64::from(input.output.handbrake),
            gear: input.output.gear,
        });
        self.push_button_edges(&input.output, &snapshot);
        // Bindings apply AFTER the raw edges are queued (learn mode
        // + the UI highlight see the physical button) and BEFORE the
        // output is stored: `last_output` — and the backend send,
        // via `ingest_engine_output` — carry the bound actions.
        *lock(&self.last_output) = self.apply_bindings(&input.output);
        *lock(&self.latest_snapshot) = Arc::new(snapshot.clone());
        snapshot
    }

    /// Apply the persisted button→action bindings to one pipeline
    /// tick (v2.0.2, Round 2B). A pure per-tick override — no state
    /// is kept except the app-action edge set below — so releasing
    /// a button restores the pipeline's own value on the next tick.
    ///
    /// * Game action: the source button's bit is cleared and the
    ///   target is set while the button is held — `horn` sets bit 2
    ///   (button 3, plus the `horn` flag), `shifter_up` /
    ///   `shifter_down` set bits 0/1, `gear_N` sets the decoded
    ///   `gear` field, `handbrake` drives the handbrake axis to
    ///   1.0. The game never sees the raw button as well.
    /// * App action: on a press *edge* only (a held button must not
    ///   repeat at 200 Hz), the action is queued for the daemon
    ///   manager loop ([`AppStateCore::drain_binding_actions`]); the
    ///   output is untouched. Encoder pulses (65–72) are edge-detected
    ///   against the previous tick's encoder bits (R2-08) — the
    ///   engine repeats `last_output` on ticks with no new frame, so
    ///   a momentary pulse repeated across ticks is one edge, not one
    ///   action per tick.
    /// * No binding for a button: the tick passes through unchanged
    ///   (the pre-binding default behaviour).
    /// * A binding to `horn` bypasses the horn module gate on
    ///   purpose: the user explicitly bound the button, so the horn
    ///   sounds even when the horn module is off. (A binding added
    ///   while its button is already held fires its app action
    ///   once on the next tick — the edge baseline only tracks
    ///   ticks that pass through here.)
    fn apply_bindings(&self, output: &PipelineOutputState) -> PipelineOutputState {
        let bindings = lock(&self.bindings).list().clone();
        let pressed64 = u64::from(output.buttons) | (u64::from(output.buttons_ex) << 32);
        if bindings.is_empty() {
            *lock(&self.last_bound_pressed) = pressed64;
            *lock(&self.last_bound_encoder) = output.encoder_buttons.0;
            return *output;
        }
        let mut out = *output;
        let mut queued: Vec<BindingAppAction> = Vec::new();
        {
            let mut last_pressed = lock(&self.last_bound_pressed);
            let mut last_encoder = lock(&self.last_bound_encoder);
            for (&button, action) in &bindings {
                if !Self::binding_button_pressed(button, output, pressed64) {
                    continue;
                }
                if is_app_action(action) {
                    // Edge only — see the method docs. Encoder
                    // buttons (65–72) are edge-detected against the
                    // previous tick's encoder bits (R2-08), exactly
                    // like the physical-button path below.
                    let edge = if button >= 65 {
                        let bit = 1_u8 << (button - 65);
                        *last_encoder & bit == 0
                    } else {
                        let bit = 1_u64 << (u64::from(button) - 1);
                        *last_pressed & bit == 0
                    };
                    if edge {
                        match action.as_str() {
                            "profile_next" => queued.push(BindingAppAction::ProfileNext),
                            "profile_previous" => {
                                queued.push(BindingAppAction::ProfilePrevious);
                            }
                            _ => {}
                        }
                    }
                    continue;
                }
                // Game action: clear the source bit, set the target.
                Self::clear_bound_button(&mut out, button);
                match action.as_str() {
                    "horn" => {
                        out.buttons |= 1 << 2;
                        out.horn = true;
                    }
                    "shifter_up" => out.buttons |= 1 << 0,
                    "shifter_down" => out.buttons |= 1 << 1,
                    "gear_1" => out.gear = 1,
                    "gear_2" => out.gear = 2,
                    "gear_3" => out.gear = 3,
                    "gear_4" => out.gear = 4,
                    "gear_5" => out.gear = 5,
                    "gear_6" => out.gear = 6,
                    "handbrake" => out.handbrake = 1.0,
                    // An unknown action can never be stored (the
                    // store validates on set and on load) — ignore
                    // it rather than inventing an output.
                    _ => {}
                }
            }
            *last_pressed = pressed64;
            *last_encoder = output.encoder_buttons.0;
        }
        if !queued.is_empty() {
            lock(&self.binding_actions).extend(queued);
        }
        out
    }

    /// True when wheel button `button` is pressed in this tick's
    /// output: bits of `buttons` are buttons 1–32, bits of
    /// `buttons_ex` are buttons 33–64 (pre-combined in
    /// `pressed64`), and `encoder_buttons` bit `n` is virtual
    /// button `65 + n`.
    fn binding_button_pressed(button: u8, output: &PipelineOutputState, pressed64: u64) -> bool {
        if button >= 65 {
            let bit = button - 65;
            bit < 8 && output.encoder_buttons.0 & (1 << bit) != 0
        } else if button >= 1 {
            pressed64 & (1_u64 << (u64::from(button) - 1)) != 0
        } else {
            false
        }
    }

    /// Clear one wheel button's bit from a tick's output (the
    /// source bit of a bound game action — the game must not see
    /// the raw button as well as the bound target).
    fn clear_bound_button(out: &mut PipelineOutputState, button: u8) {
        match button {
            1..=32 => out.buttons &= !(1 << (u64::from(button) - 1)),
            33..=64 => out.buttons_ex &= !(1 << (u64::from(button) - 33)),
            65..=72 => {
                out.encoder_buttons.0 &= !(1 << (button - 65));
            }
            _ => {}
        }
    }

    /// Diffs `output`'s button bits against the previous tick and
    /// queues the edges; encoder virtual-button pulses (65–72) from
    /// the snapshot are momentary, so only the press edge is queued.
    fn push_button_edges(&self, output: &PipelineOutputState, snapshot: &EngineSnapshot) {
        let combined = u64::from(output.buttons) | (u64::from(output.buttons_ex) << 32);
        let mut last = lock(&self.last_buttons);
        let changed = combined ^ *last;
        *last = combined;
        drop(last);
        if changed != 0 {
            let mut events = lock(&self.button_events);
            // L4 learn mode: the first physical press (1–32) in this
            // edge batch is the pick. Collected here, applied after
            // the events lock is dropped (no nested locking).
            let mut learned: Option<u8> = None;
            for button in 1..=64_u8 {
                let bit = 1_u64 << (u64::from(button) - 1);
                if changed & bit != 0 {
                    let pressed = combined & bit != 0;
                    events.push_back(ButtonPressEvent { button, pressed });
                    if pressed && (1..=32).contains(&button) && learned.is_none() {
                        learned = Some(button);
                    }
                }
            }
            drop(events);
            if let Some(button) = learned
                && *lock(&self.learn_armed)
            {
                *lock(&self.learned_button) = Some(button);
                *lock(&self.learn_armed) = false;
            }
        }
        let mut events = lock(&self.button_events);
        for &button in &snapshot.encoder_buttons {
            events.push_back(ButtonPressEvent {
                button,
                pressed: true,
            });
        }
    }

    /// Drains the queued button-press edges (oldest first). The
    /// snapshot emitter calls this after each snapshot emission.
    #[must_use]
    pub fn drain_button_presses(&self) -> Vec<ButtonPressEvent> {
        lock(&self.button_events).drain(..).collect()
    }

    /// Runs the 30 Hz snapshot emitter loop on the calling thread:
    /// emits the latest snapshot every [`SNAPSHOT_EMIT_INTERVAL`],
    /// then drains + emits queued button-press events, until `stop`
    /// is set. Missed ticks are skipped, never burst-caught-up, so a
    /// slow emit can never flood the frontend. The engine is never
    /// involved here — this only samples what `ingest_frame` stored.
    pub fn run_snapshot_emitter(
        state: &Arc<Self>,
        emit: impl Fn(SnapshotEmit) + Send,
        stop: &AtomicBool,
    ) {
        let mut next = Instant::now();
        loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            emit(SnapshotEmit::Snapshot(Box::new(state.snapshot())));
            for event in state.drain_button_presses() {
                emit(SnapshotEmit::ButtonPress(event));
            }
            for (name, payload) in state.drain_app_events() {
                emit(SnapshotEmit::AppEvent(name, payload));
            }
            next += SNAPSHOT_EMIT_INTERVAL;
            let now = Instant::now();
            if next > now {
                std::thread::sleep(next - now);
            } else {
                // Behind schedule: skip the missed ticks instead of
                // bursting to catch up.
                next = now + SNAPSHOT_EMIT_INTERVAL;
            }
        }
    }

    /* ------------------------------------------------------------------ */
    /* Output backend                                                      */
    /* ------------------------------------------------------------------ */

    /// Records the outcome of one backend interaction honestly:
    /// `accepted` is true only when the backend actually took the
    /// state. `note` names the interaction for the human-readable
    /// detail in [`AppStateCore::output_state`].
    fn record_send(&self, accepted: bool, note: String) {
        *lock(&self.last_send_ok) = Some(accepted);
        *lock(&self.last_send_note) = note;
    }

    /// Current output state, exactly as the backend reports it: the
    /// installed backend's name, `connected` from the backend's live
    /// `is_connected()`, and a detail that names the backend plus the
    /// last send's outcome. `enabled` is the toggle state. When the
    /// live backend is `HIDMaestro` the detail carries the UAC note
    /// (elevated helper for the whole session); after a fallback it
    /// also names the requested backend and the failure reason.
    #[must_use]
    pub fn output_state(&self) -> CommandOutputState {
        let backend = self.backend_handle();
        let connected = lock(&backend).is_connected();
        let enabled = *lock(&self.output_enabled);
        let detail = format!(
            "{} | last send: {}",
            lock(&self.backend_detail),
            lock(&self.last_send_note)
        );
        crate::commands::get_output_state(&self.backend_name(), connected, &detail, Some(enabled))
    }

    /// Applies the Output screen toggle — and actually actuates it:
    /// enabling acquires the device (`try_connect`, idempotent when
    /// already connected) and resends the last pipeline output;
    /// disabling sends neutral and releases the device. The returned
    /// state reports whether the backend accepted the send (with the
    /// [`NullSink`] installed: the connect fails honestly, nothing is
    /// delivered — D-F3).
    #[must_use]
    pub fn set_output_enabled(&self, enabled: bool) -> CommandOutputState {
        let vjoy_device_id = lock(&self.store).settings().vjoy_device_id;
        let factories = real_backend_factories(vjoy_device_id);
        self.set_output_enabled_with_factories(enabled, &factories)
    }

    /// [`AppStateCore::set_output_enabled`] with injectable factories
    /// (the test seam: tests drive the deferred-activation path with
    /// mock backends here, the shell and settings paths use the real
    /// factories through [`AppStateCore::set_output_enabled`]).
    ///
    /// When output is enabled while the [`NullSink`] is installed and
    /// a real backend is selected, this is the user-initiated moment
    /// the deferred startup backend is activated: startup never
    /// activates an elevated backend (hotfix v1.0.1 — no UAC prompt at
    /// launch), so without this step the toggle would only
    /// `try_connect` the [`NullSink`] and output would stay dead until
    /// the user re-picked the backend on the Output page. The switch
    /// runs here because enabling output is an explicit user action —
    /// the UAC prompt for `HIDMaestro` at this point is expected, not
    /// a startup surprise. A failed switch is recorded honestly in the
    /// returned state's detail and the [`NullSink`] stays installed.
    #[must_use]
    pub fn set_output_enabled_with_factories(
        &self,
        enabled: bool,
        factories: &BackendFactories,
    ) -> CommandOutputState {
        *lock(&self.output_enabled) = enabled;
        if enabled {
            if BackendKind::from_backend_name(&self.backend_name()) == Some(BackendKind::None)
                && self.selected_backend() != BackendKind::None
            {
                let requested = self.selected_backend();
                // Deferred startup activation (automatic path): the
                // fallback stays allowed here — enabling output is
                // not an explicit backend switch (Bug 4/8 bans the
                // fallback only for `set_output_backend`).
                if let Err(error) = self.apply_backend_switch(requested, factories, true) {
                    self.record_send(
                        false,
                        format!("output enabled: backend activation failed ({error})"),
                    );
                    return self.output_state();
                }
            }
            // R2-09: connect outside the backend lock (up to 60 s
            // for HIDMaestro's sidecar spawn + UAC dwell). Take the
            // backend out of the slot (a disconnected NullSink
            // stands in, so engine ticks keep flowing instead of
            // blocking on the mutex), connect + resend on the local
            // box, then swap it back. Serialised against switches
            // by `switch_in_flight` so two extractions can never
            // clobber each other's restore; a switch already in
            // flight fails fast here instead of blocking the caller.
            let Ok(_switch_guard) = self.switch_in_flight.try_lock() else {
                self.record_send(
                    false,
                    "output enabled: a backend switch is already in progress — try again"
                        .to_string(),
                );
                return self.output_state();
            };
            let backend = if let Some(placeholder) = factories.build(BackendKind::None) {
                let mut slot = lock(&self.backend);
                std::mem::replace(&mut *slot, Arc::new(Mutex::new(placeholder)))
            } else {
                // No sink factory (test-only path — the real
                // factories always provide one): connect on the
                // live handle, as before.
                let handle = self.backend_handle();
                let mut backend = lock(&handle);
                return self.connect_and_resend(&mut backend);
            };
            // The extracted handle is owned by this scope: the slow
            // connect + resend run on its own lock, never on the
            // slot lock, so engine ticks keep flowing on the
            // placeholder installed above. The connect result is
            // bound first — the scrutinee guard of a `match` would
            // still be held inside the arm and deadlock the resend
            // on the same handle.
            let connected = lock(&backend).try_connect();
            match connected {
                Ok(()) => {
                    let last = *lock(&self.last_output);
                    match lock(&backend).update(&last) {
                        Ok(()) => {
                            self.record_send(true, "output enabled: sent to backend".to_string());
                        }
                        Err(error) => {
                            self.record_send(
                                false,
                                format!("output enabled: update failed ({error})"),
                            );
                        }
                    }
                }
                Err(error) => {
                    self.record_send(false, format!("output enabled: connect failed ({error})"));
                }
            }
            *lock(&self.backend) = backend;
        } else {
            // R2-09: same extract pattern for the neutral +
            // disconnect — a sidecar shutdown must not hold the
            // backend lock either. If a switch is in flight the
            // flag above is already false, so the engine sends
            // nothing (see `send_engine_output`) and the switch
            // result is left alone.
            let Ok(_switch_guard) = self.switch_in_flight.try_lock() else {
                self.record_send(
                    false,
                    "output disabled: a backend switch is in progress — output is off".to_string(),
                );
                return self.output_state();
            };
            let backend = if let Some(placeholder) = factories.build(BackendKind::None) {
                let mut slot = lock(&self.backend);
                std::mem::replace(&mut *slot, Arc::new(Mutex::new(placeholder)))
            } else {
                let handle = self.backend_handle();
                let mut backend = lock(&handle);
                return self.disable_backend(&mut backend);
            };
            let (was_connected, neutral) = {
                let mut backend = lock(&backend);
                let was_connected = backend.is_connected();
                let neutral = backend.send_neutral();
                backend.disconnect();
                (was_connected, neutral)
            };
            *lock(&self.backend) = backend;
            match (was_connected, neutral) {
                (true, Ok(())) => {
                    self.record_send(true, "output disabled: neutral sent to backend".to_string());
                }
                (true, Err(error)) => {
                    self.record_send(false, format!("output disabled: neutral failed ({error})"));
                }
                (false, _) => {
                    self.record_send(
                        false,
                        "output disabled: backend not connected, nothing sent".to_string(),
                    );
                }
            }
        }
        self.output_state()
    }

    /// Connect the given backend and resend the last output (the
    /// enable half of [`Self::set_output_enabled_with_factories`],
    /// split out so the no-sink-factory fallback can run it under
    /// the lock without duplicating the block).
    fn connect_and_resend(&self, backend: &mut Box<dyn IJoystickOutput>) -> CommandOutputState {
        match backend.try_connect() {
            Ok(()) => {
                let last = *lock(&self.last_output);
                match backend.update(&last) {
                    Ok(()) => {
                        self.record_send(true, "output enabled: sent to backend".to_string());
                    }
                    Err(error) => {
                        self.record_send(false, format!("output enabled: update failed ({error})"));
                    }
                }
            }
            Err(error) => {
                self.record_send(false, format!("output enabled: connect failed ({error})"));
            }
        }
        self.output_state()
    }

    /// Send neutral + disconnect the given backend (the disable half
    /// of [`Self::set_output_enabled_with_factories`], split out so
    /// the no-sink-factory fallback can run it under the lock
    /// without duplicating the block).
    fn disable_backend(&self, backend: &mut Box<dyn IJoystickOutput>) -> CommandOutputState {
        let was_connected = backend.is_connected();
        let neutral = backend.send_neutral();
        backend.disconnect();
        match (was_connected, neutral) {
            (true, Ok(())) => {
                self.record_send(true, "output disabled: neutral sent to backend".to_string());
            }
            (true, Err(error)) => {
                self.record_send(false, format!("output disabled: neutral failed ({error})"));
            }
            (false, _) => {
                self.record_send(
                    false,
                    "output disabled: backend not connected, nothing sent".to_string(),
                );
            }
        }
        self.output_state()
    }

    /// Selects the output backend by name (`"HIDMaestro"`, `"vJoy"`,
    /// `"none"` / `"null"` / `"NullSink"`; case-insensitive) and
    /// switches the live slot at runtime — no restart. Default is
    /// `vJoy`. This is an EXPLICIT user switch: a failed `HIDMaestro`
    /// connect NEVER falls back to vJoy (Bug 4/8) — it errors with
    /// the real reason, the live backend stays exactly as it was,
    /// and the persisted choice reverts to the previous backend, so
    /// the next launch does not retry a switch the user saw fail.
    /// (Automatic paths — startup activation, the deferred enable,
    /// mid-session failover — still allow the fallback; see
    /// [`Self::apply_backend_switch`].) A failed connect of any kind
    /// leaves the previous backend exactly as it was (see
    /// [`switch_backend`]).
    ///
    /// The requested choice is persisted in settings first (a save
    /// failure errors out before anything switches); on a failed
    /// switch the persisted choice is written back to the previous
    /// backend.
    ///
    /// # Errors
    ///
    /// Returns a message when the name is unknown, the settings save
    /// fails, or the requested backend could not connect.
    pub fn set_output_backend(&self, name: &str) -> Result<CommandOutputState, String> {
        let vjoy_device_id = lock(&self.store).settings().vjoy_device_id;
        let factories = real_backend_factories(vjoy_device_id);
        self.set_output_backend_with_factories(name, &factories)
    }

    /// [`AppStateCore::set_output_backend`] with injectable factories
    /// (the test seam: tests drive the explicit switch with mock
    /// backends here, the shell uses the real factories through
    /// [`AppStateCore::set_output_backend`]).
    ///
    /// # Errors
    ///
    /// Returns a message when the name is unknown, the settings save
    /// fails, or the requested backend could not connect.
    pub fn set_output_backend_with_factories(
        &self,
        name: &str,
        factories: &BackendFactories,
    ) -> Result<CommandOutputState, String> {
        let requested = BackendKind::parse(name).ok_or_else(|| {
            format!("unknown output backend '{name}' (expected HIDMaestro, vJoy, or none)")
        })?;
        // R2-20: a backend switch re-probes the vJoy DLL on the next
        // status read instead of trusting the cached probe.
        self.invalidate_vjoy_dll_cache();
        // C-FAIL-1: an explicit user re-select re-arms the
        // mid-session failover, so the next failure can fall over
        // once more.
        self.reset_fallback();
        // B-2/A10: an explicit re-select also clears the UAC-decline
        // latch — the user is choosing HIDMaestro again on purpose,
        // so one fresh prompt is allowed. Automatic (re-)activation
        // (`set_output_enabled`, startup) never clears it.
        self.elevation_declined.store(false, Ordering::SeqCst);
        // Persist the choice first: a save failure errors out before
        // anything switches. Remember the previous choice: on a
        // failed explicit switch the persisted choice reverts to it
        // (Bug 4/8 — a failed HIDMaestro switch must not leave
        // HIDMaestro persisted, or the next launch defers/retries a
        // switch the user already saw fail).
        let previous_output_backend = lock(&self.store).settings().output_backend.clone();
        {
            let mut settings = lock(&self.store).settings().clone();
            settings.output_backend = requested.settings_value().to_string();
            let mut store = lock(&self.store);
            store.set_settings(settings);
            store.save().map_err(|error| error.to_string())?;
        }
        // DS-30: the write above changed the store — refresh the
        // `Arc` cache so `settings_arc()` readers see it at once.
        self.refresh_settings_cache();
        let result = self
            .apply_backend_switch(requested, factories, false)
            .map_err(|error| {
                format!(
                    "could not switch output backend to {}: {error}",
                    requested.display_name()
                )
            })
            .map(|_| self.output_state());
        if result.is_err() {
            // Revert the persisted choice (best effort: the switch
            // error above is the one reported even if this save
            // fails — the in-memory store is reverted either way).
            // The store guard lives only inside this block (like the
            // persist above): `refresh_settings_cache` locks the
            // store itself, so it must run after the guard drops —
            // holding it across that call deadlocks the thread.
            {
                let mut settings = lock(&self.store).settings().clone();
                settings.output_backend = previous_output_backend;
                let mut store = lock(&self.store);
                store.set_settings(settings);
                let _ = store.save();
            }
            self.refresh_settings_cache();
        }
        result
    }

    /// Re-arms the mid-session failover (C-FAIL-1): after an explicit
    /// user re-select the next backend failure may fall over to the
    /// fallback once more. Called by `set_output_backend`; the
    /// 30 s rate limit still applies to the next attempt.
    ///
    /// This clears only the app-core flag. The backend's own latch
    /// (the `HIDMaestro` `SidecarManager::fallback_needed` latch — the
    /// "H2 latch") lives behind the [`IJoystickOutput`] trait, which
    /// exposes no reset hook, so this method cannot call the
    /// backend's `reset_fallback` directly. The backend side of the
    /// re-arm happens in [`Self::apply_backend_switch`]: a
    /// disconnected same-kind re-select replaces the installed
    /// instance with a fresh factory build, whose manager latch is
    /// clear — the same state `HidMaestroBackend::reset_fallback`
    /// produces. Without that step the selector's same-kind path
    /// would reconnect the latched instance, fail fast with
    /// `FallbackNeeded`, and the re-select could never succeed
    /// (deep-scan Round 2, item 5).
    pub fn reset_fallback(&self) {
        self.fallback_used.store(false, Ordering::SeqCst);
    }

    /// Whether the mid-session failover already fired since the last
    /// explicit re-select (C-FAIL-1).
    #[must_use]
    pub fn fallback_used(&self) -> bool {
        self.fallback_used.load(Ordering::SeqCst)
    }

    /// UAC decline message (B-2/A10, v2.0.2): shown once when the user
    /// declines the `HIDMaestro` elevation prompt. The switch fails with
    /// this (no silent fallback to vJoy) and the decline latch blocks
    /// automatic re-prompts until an explicit re-select.
    pub const HID_NOT_ADMIN: &str = "HIDMaestro needs administrator rights — approve the UAC prompt to switch to it, or run RSH as administrator. vJoy needs no prompt.";

    /// Applies a backend switch through the selector with the given
    /// factories, and updates the live name / detail / selected choice
    /// on success. Shared by [`AppStateCore::set_output_backend`]
    /// (explicit — `allow_fallback: false`, so a failed requested
    /// backend errors instead of silently landing on vJoy, Bug 4/8)
    /// and the automatic paths ([`AppStateCore::activate_startup_backend`]
    /// and the deferred enable in
    /// [`AppStateCore::set_output_enabled_with_factories`], plus the
    /// mid-session failover — `allow_fallback: true`). On failure
    /// the slot is untouched (see [`switch_backend`]) and the error
    /// is returned; the caller records it.
    ///
    /// # Errors
    ///
    /// Returns the selector's failure (no factory, or the requested
    /// backend and its fallback, when allowed, both failed to
    /// connect).
    fn apply_backend_switch(
        &self,
        requested: BackendKind,
        factories: &BackendFactories,
        allow_fallback: bool,
    ) -> Result<SwitchOutcome, String> {
        // B-2: serialise switches — a second overlapping switch
        // (user double-clicking, a poll-driven re-activation) would
        // spawn a second elevated sidecar and a second UAC prompt.
        let _switch_guard = self.switch_in_flight.try_lock().map_err(|_| {
            "a backend switch is already in progress — wait for it to finish".to_string()
        })?;
        // B-2/A10: the user declined the UAC prompt this session.
        // Fail fast with the NotAdmin message — no new prompt, no
        // silent fallback. Only an explicit re-select clears this
        // (see `set_output_backend`).
        if requested == BackendKind::HidMaestro && self.elevation_declined.load(Ordering::SeqCst) {
            return Err(Self::HID_NOT_ADMIN.to_string());
        }
        let vjoy_device_id = lock(&self.store).settings().vjoy_device_id;
        // H2 latch, deep-scan Round 2 item 5: the selector's
        // same-kind path reconnects the INSTALLED instance, so a
        // re-select of a disconnected backend whose manager latch is
        // set (`fallback_needed`, after its retry budget is spent)
        // would fail fast with `FallbackNeeded` forever — the
        // same-kind re-select trap. The trait exposes no reset hook,
        // so re-arm the backend by replacing a DISCONNECTED same-kind
        // instance with a fresh factory build before the selector
        // runs: a fresh `HidMaestroBackend` has a fresh
        // `SidecarManager` (latch clear), which is exactly what the
        // backend's own `reset_fallback` does. A connected instance
        // is never replaced (its live sidecar session survives), and
        // a missing factory falls through to the selector's old
        // behaviour. `set_output_backend` calls `reset_fallback`
        // first, so by the time this runs the app-core flag is clear
        // too — both latches re-arm on one explicit re-select.
        let live = self.backend_handle();
        if requested != BackendKind::None
            && BackendKind::from_backend_name(&self.backend_name()) == Some(requested)
            && !lock(&live).is_connected()
            && let Some(fresh) = factories.build(requested)
        {
            *lock(&self.backend) = Arc::new(Mutex::new(fresh));
        }
        // R2-09: the selector's connect (up to 60 s of sidecar
        // spawn + UAC dwell for HIDMaestro) must NOT run under the
        // backend lock — while it did, every engine tick blocked on
        // the mutex and output stopped entirely for as long as the
        // user took to answer UAC. Take the installed backend OUT of
        // the slot (leaving a disconnected NullSink behind, so
        // engine ticks keep flowing instead of blocking), run the
        // switch on the local box with no lock held, then swap the
        // result back under a short lock. The slot is restored on
        // every path: a failed switch leaves the previous backend
        // installed, exactly as the selector guarantees.
        let outcome: Result<SwitchOutcome, String> = if let Some(placeholder) =
            factories.build(BackendKind::None)
        {
            let handle = {
                let mut slot = lock(&self.backend);
                std::mem::replace(&mut *slot, Arc::new(Mutex::new(placeholder)))
            };
            // The extracted handle is owned by this scope: the
            // switch (up to 60 s of sidecar spawn + UAC dwell)
            // runs on its own lock, never on the slot lock.
            let mut current = lock(&handle);
            let outcome = switch_backend(&mut current, requested, factories, None, allow_fallback)
                .map_err(|error| error.to_string());
            drop(current);
            *lock(&self.backend) = handle;
            outcome
        } else {
            // No sink factory (test-only path — the real
            // factories always provide one): switch on the live
            // handle, as before.
            let handle = self.backend_handle();
            let mut backend = lock(&handle);
            switch_backend(&mut backend, requested, factories, None, allow_fallback)
                .map_err(|error| error.to_string())
        };
        self.finish_backend_switch(outcome, requested, vjoy_device_id, allow_fallback)
    }

    /// Interprets a completed backend switch (R2-09): the UAC-decline
    /// handling + live-name/detail bookkeeping that used to sit
    /// inline in [`Self::apply_backend_switch`], split out so the
    /// switch itself can run outside the backend lock. The backend
    /// slot already holds the switch result when this runs, so the
    /// live name is read from the slot (never from a stale local).
    fn finish_backend_switch(
        &self,
        outcome: Result<SwitchOutcome, String>,
        requested: BackendKind,
        vjoy_device_id: u32,
        allow_fallback: bool,
    ) -> Result<SwitchOutcome, String> {
        // B-2/A10: a UAC decline surfaces as a NotAdmin failure
        // (or a fallback whose reason names it). Record the decline
        // and FAIL the switch with the plain message — never silently
        // fall back to vJoy (the user asked for HIDMaestro and must
        // know it is not active) and never re-prompt automatically.
        let outcome = match outcome {
            Ok(outcome) => {
                if outcome.fell_back
                    && outcome
                        .fallback_reason
                        .as_deref()
                        .is_some_and(|reason| reason.contains("administrator rights"))
                {
                    self.elevation_declined.store(true, Ordering::SeqCst);
                    return Err(Self::HID_NOT_ADMIN.to_string());
                }
                // Bug 4/8 defence: an explicit switch must never
                // report a fallback as success — the selector does
                // not produce one when `allow_fallback` is false,
                // but if one ever arrived it is a failure with the
                // real reason, not a silent "now vJoy".
                if outcome.fell_back && !allow_fallback {
                    return Err(outcome.fallback_reason.clone().unwrap_or_else(|| {
                        format!(
                            "{} could not connect (fell back to {})",
                            requested.display_name(),
                            outcome.active.display_name()
                        )
                    }));
                }
                outcome
            }
            Err(error) => {
                if requested == BackendKind::HidMaestro && error.contains("administrator rights") {
                    self.elevation_declined.store(true, Ordering::SeqCst);
                    return Err(Self::HID_NOT_ADMIN.to_string());
                }
                return Err(error);
            }
        };
        *lock(&self.selected_backend) = requested;
        let live = self.backend_handle();
        let live_name = lock(&live).name().to_string();
        let mut detail = live_backend_detail(outcome.active, vjoy_device_id);
        if outcome.fell_back {
            detail.push(' ');
            detail.push_str(&outcome.detail);
            if let Some(reason) = &outcome.fallback_reason {
                detail.push_str(" Reason: ");
                detail.push_str(reason);
            }
        }
        *lock(&self.backend_name) = live_name;
        *lock(&self.backend_detail) = detail;
        Ok(outcome)
    }

    /// Activates the persisted output backend choice at launch
    /// (coordinator decision under delegation): reads
    /// `AppSettings.output_backend` (default `vJoy`) and switches
    /// the live slot through the selector — EXCEPT an elevated backend,
    /// which is NEVER activated at startup (hotfix v1.0.1, coordinator
    /// Option A — defer): activating `HIDMaestro` would launch its
    /// sidecar elevated via `runas` and pop a UAC consent prompt on
    /// every launch. A persisted `HIDMaestro` choice is therefore
    /// deferred: the outcome records `active: None` with a reason
    /// naming the deferral, the [`NullSink`](crate::system::NullSink)
    /// stays installed, and the backend detail shows the reason on the
    /// Output page. The user activates it by enabling output
    /// ([`AppStateCore::set_output_enabled`], which switches the
    /// deferred backend in on the explicit toggle) or by picking it on
    /// the Output page ([`AppStateCore::set_output_backend`]) — both
    /// user-initiated, so the UAC prompt there is expected. A
    /// non-elevated choice (vJoy, driver present) IS activated at
    /// startup through the selector, with the usual fallback to the
    /// [`NullSink`](crate::system::NullSink) and the reason recorded.
    /// This never fails the launch: when activation fails, the outcome
    /// records `active: None` and the reason, the
    /// [`NullSink`](crate::system::NullSink) stays installed, and the
    /// backend detail names the failure — the app keeps running with
    /// output disabled instead of crashing or silently pretending a
    /// backend is live. The choice is NOT re-persisted here (it is
    /// already the persisted choice); the next launch defers / retries
    /// it the same way.
    #[must_use]
    pub fn activate_startup_backend(&self) -> StartupBackendOutcome {
        let vjoy_device_id = lock(&self.store).settings().vjoy_device_id;
        let factories = real_backend_factories(vjoy_device_id);
        self.activate_startup_backend_with_factories(&factories)
    }

    /// [`AppStateCore::activate_startup_backend`] with injectable
    /// factories (the test seam: tests build mock backends here, the
    /// shell uses the real factories through
    /// [`AppStateCore::activate_startup_backend`]).
    #[must_use]
    pub fn activate_startup_backend_with_factories(
        &self,
        factories: &BackendFactories,
    ) -> StartupBackendOutcome {
        let requested = self.selected_backend();
        // UAC deferral (hotfix v1.0.1): only HIDMaestro needs
        // elevation — its sidecar is launched elevated via `runas`
        // (`rsh-output-hidmaestro` manager `spawn_elevated_runas`)
        // and its status reports `elevation_required: true`; vJoy
        // needs none. Never touch a factory for an elevated backend
        // here: even building it must not start the elevated launch,
        // and no UAC prompt may appear at startup.
        if requested == BackendKind::HidMaestro {
            let reason = "HIDMaestro deferred at startup — it needs elevation (UAC). Enable output to activate it."
                .to_string();
            *lock(&self.backend_detail) = format!(
                "{reason} Selected backend stays HIDMaestro; no UAC prompt was shown at startup."
            );
            let result = StartupBackendOutcome {
                requested,
                active: None,
                fell_back: false,
                reason: Some(reason),
            };
            *lock(&self.startup_backend) = Some(result.clone());
            return result;
        }
        match self.apply_backend_switch(requested, factories, true) {
            Ok(outcome) => {
                let result = StartupBackendOutcome {
                    requested,
                    active: Some(outcome.active),
                    fell_back: outcome.fell_back,
                    reason: outcome.fallback_reason.clone(),
                };
                *lock(&self.startup_backend) = Some(result.clone());
                result
            }
            Err(reason) => {
                // Both backends failed: the slot is untouched (still
                // the NullSink), but the failure must be visible in
                // diagnostics — record it in the backend detail.
                *lock(&self.backend_detail) = format!(
                    "startup backend activation failed for {}: {reason} — output disabled (NullSink); no backend was silently installed",
                    requested.display_name()
                );
                let result = StartupBackendOutcome {
                    requested,
                    active: None,
                    fell_back: false,
                    reason: Some(reason),
                };
                *lock(&self.startup_backend) = Some(result.clone());
                result
            }
        }
    }

    /// The outcome of the most recent
    /// [`AppStateCore::activate_startup_backend`] call ([`None`] until
    /// the shell activates the persisted backend at launch).
    #[must_use]
    pub fn startup_backend_outcome(&self) -> Option<StartupBackendOutcome> {
        lock(&self.startup_backend).clone()
    }

    /// Every selectable backend with honest per-backend status for the
    /// Output page picker: which is selected, which is live, whether
    /// each is connected, whether its driver/sidecar file was found,
    /// whether it needs elevation (with the UAC note), and the vJoy
    /// install link when vJoy is missing. Nothing here is invented:
    /// `installed` is a file probe, `connected` is the live backend's
    /// `is_connected()`, and non-live backends report `connected:
    /// false`.
    #[must_use]
    pub fn list_output_backends(&self) -> Vec<BackendStatus> {
        let live_kind =
            BackendKind::from_backend_name(&self.backend_name()).unwrap_or(BackendKind::None);
        let live = self.backend_handle();
        let connected = lock(&live).is_connected();
        let selected = self.selected_backend();
        let vjoy_device_id = lock(&self.store).settings().vjoy_device_id;
        let live_detail = lock(&self.backend_detail).clone();
        let vjoy_installed = self.vjoy_dll_path().is_some();
        let hidmaestro_installed = resolve_sidecar_path().is_file();
        vec![
            BackendStatus {
                id: BackendKind::HidMaestro.as_str().to_string(),
                name: BackendKind::HidMaestro.display_name().to_string(),
                is_default: false,
                selected: selected == BackendKind::HidMaestro,
                active: live_kind == BackendKind::HidMaestro,
                connected: live_kind == BackendKind::HidMaestro && connected,
                installed: Some(hidmaestro_installed),
                elevation_required: true,
                uac_note: Some(UAC_NOTE.to_string()),
                detail: if live_kind == BackendKind::HidMaestro {
                    live_detail.clone()
                } else if hidmaestro_installed {
                    "HIDMaestro sidecar found. Explicit-choice backend — activation is deferred at startup (no UAC prompt on launch); enable output to activate it, and Windows will ask (UAC) then.".to_string()
                } else {
                    "HIDMaestro sidecar not found — reinstall RSH to use this backend.".to_string()
                },
                download_url: None,
            },
            BackendStatus {
                id: BackendKind::VJoy.as_str().to_string(),
                name: BackendKind::VJoy.display_name().to_string(),
                is_default: true,
                selected: selected == BackendKind::VJoy,
                active: live_kind == BackendKind::VJoy,
                connected: live_kind == BackendKind::VJoy && connected,
                installed: Some(vjoy_installed),
                elevation_required: false,
                uac_note: None,
                detail: if live_kind == BackendKind::VJoy {
                    live_detail.clone()
                } else if vjoy_installed {
                    format!(
                        "vJoy driver found (device {vjoy_device_id}). Default backend — needs no UAC prompt."
                    )
                } else {
                    "vJoy not found — install vJoy 2.2.2.0 to enable the default backend."
                        .to_string()
                },
                download_url: if vjoy_installed {
                    None
                } else {
                    Some(VJOY_DOWNLOAD_URL.to_string())
                },
            },
            BackendStatus {
                id: BackendKind::None.as_str().to_string(),
                name: BackendKind::None.display_name().to_string(),
                is_default: false,
                selected: selected == BackendKind::None,
                active: live_kind == BackendKind::None,
                connected: false,
                installed: Some(true),
                elevation_required: false,
                uac_note: None,
                detail: if live_kind == BackendKind::None {
                    live_detail
                } else {
                    "No output device — the wheel sends nothing to games.".to_string()
                },
                download_url: None,
            },
        ]
    }

    /// Cached `resolve_dll_path()` (R2-20): one probe per
    /// [`VJOY_DLL_CACHE_TTL`] window instead of two per
    /// `vjoy_status()` call. Callers that change driver presence —
    /// install completion ([`run_vjoy_install`]), backend switch
    /// ([`Self::set_output_backend`]) — call
    /// [`Self::invalidate_vjoy_dll_cache`] instead of waiting out
    /// the TTL.
    fn vjoy_dll_path(&self) -> Option<PathBuf> {
        {
            let cache = lock(&self.vjoy_dll_cache);
            if let Some(probed_at) = cache.probed_at
                && probed_at.elapsed() < VJOY_DLL_CACHE_TTL
            {
                return cache.dll.clone();
            }
        }
        let dll = resolve_dll_path();
        let mut cache = lock(&self.vjoy_dll_cache);
        cache.probed_at = Some(Instant::now());
        cache.dll.clone_from(&dll);
        dll
    }

    /// Drop the cached vJoy DLL probe (R2-20): the next
    /// [`Self::vjoy_dll_path`] re-probes immediately.
    fn invalidate_vjoy_dll_cache(&self) {
        let mut cache = lock(&self.vjoy_dll_cache);
        cache.probed_at = None;
        cache.dll = None;
    }

    /// vJoy driver status (v2.0.1 hotfix BACKEND-2): whether the DLL
    /// was found (cached file probe — see
    /// [`Self::vjoy_dll_path`]), whether a background install is
    /// running, the last install failure, and the resolved DLL path.
    /// `version` is [`None`]: there is no version read without
    /// loading the DLL, and an unmeasured version is never invented.
    #[must_use]
    pub fn vjoy_status(&self) -> rsh_api::VjoyStatus {
        // Probe before taking the install lock: the probe takes the
        // cache lock, and no other path takes both locks in the
        // opposite order.
        let dll = self.vjoy_dll_path();
        let install = lock(&self.vjoy_install);
        rsh_api::VjoyStatus {
            installed: dll.is_some(),
            installing: install.installing,
            install_error: install.error.clone(),
            dll_path: dll.map(|path| path.to_string_lossy().into_owned()),
            version: None,
        }
    }

    /// Start a vJoy driver install in the background (v2.0.1 hotfix
    /// BACKEND-2) and return immediately: the download + silent
    /// install run on their own thread ([`run_vjoy_install`]) and
    /// progress / failure surface through
    /// [`AppStateCore::vjoy_status`] polling — never by blocking the
    /// caller. The installer needs admin once (its own manifest
    /// triggers the UAC prompt); that is expected and unlike
    /// `HIDMaestro`, which prompts on every launch.
    ///
    /// # Errors
    ///
    /// Returns a message on non-Windows (vJoy is Windows-only) or
    /// when the install cannot be started. An already-installed
    /// driver or an install already in progress is not an error —
    /// the returned message says so.
    pub fn install_vjoy(self: &Arc<Self>) -> Result<String, String> {
        if !cfg!(windows) {
            return Err("vJoy is Windows-only".to_string());
        }
        if resolve_dll_path().is_some() {
            return Ok("vJoy is already installed".to_string());
        }
        {
            let mut install = lock(&self.vjoy_install);
            if install.installing {
                return Ok("vJoy install already in progress".to_string());
            }
            install.installing = true;
            install.error = None;
        }
        let state = Arc::clone(self);
        // DS-40: a failed thread spawn used to leave `installing`
        // latched `true` forever (the flag was set before the
        // spawn, and only the thread itself cleared it) — every
        // later attempt then answered "already in progress".
        // Reset the flag on the spawn-failure path.
        match std::thread::Builder::new()
            .name("rsh-vjoy-install".to_string())
            .spawn(move || run_vjoy_install(&state))
        {
            Ok(_) => {
                self.note_slow_state_changed();
                Ok("vJoy install started".to_string())
            }
            Err(error) => {
                let mut install = lock(&self.vjoy_install);
                install.installing = false;
                Err(format!("could not start the vJoy install thread: {error}"))
            }
        }
    }

    /// Live engine / link diagnostics (v2.0.1 hotfix BACKEND-2): the
    /// measured engine stats (loop rate, loss, jitter / latency
    /// p99s) plus the board's `seqGap` / `txFail` counters from the
    /// latest snapshot. Every field is measured — an unmeasured rate
    /// is [`None`], never a fake 200 Hz.
    #[must_use]
    pub fn diag_stats(&self) -> rsh_api::DiagStats {
        let stats = self.engine_stats();
        let snapshot = self.snapshot_arc();
        rsh_api::DiagStats {
            loop_hz: stats.loop_hz,
            loss_pct: stats.loss_pct,
            jitter_ms_p99: stats.jitter_ms_p99,
            output_latency_ms: stats.output_latency_ms,
            e2e_ms: stats.e2e_ms,
            seq_gap: snapshot.seq_gap,
            tx_fail: snapshot.tx_fail,
            // DS-82: the core does not know the IPC latency — only
            // the daemon's `DaemonApi::diag_stats` fills these in.
            ipc_p50_ms: None,
            ipc_p99_ms: None,
            ipc_per_method_ms: Vec::new(),
            // R3-D-13: same — the action→poll latency is measured
            // by the IPC server, filled in by the daemon.
            action_p50_ms: None,
            action_p99_ms: None,
        }
    }

    /// Real shutdown-neutral actuation (D-F3 / F4): signals the engine
    /// to stop when a stop handle is registered, then sends neutral
    /// through the installed backend. Same ordering contract as
    /// `system::shutdown_output`, for the output trait instead of the
    /// engine sink. The outcome reports honestly whether neutral was
    /// sent — with the [`NullSink`] installed: attempted,
    /// `neutral_sent: false`. Note `send_neutral` returning `Ok`
    /// while disconnected is the trait's no-op, not a delivery: only
    /// a backend that was connected can have delivered neutral.
    #[must_use]
    pub fn shutdown(&self) -> ShutdownOutcome {
        let stop = lock(&self.stop_handle).clone();
        let stop_signalled = match stop.as_ref() {
            Some(handle) => {
                handle.stop();
                true
            }
            None => false,
        };
        let neutral_sent = {
            // PERF F-15: the shutdown neutral + disconnect run on
            // the cloned handle's own lock (one-off); the slot lock
            // stays free, so a switch mid-shutdown swaps the slot
            // instead of blocking on the disconnect.
            let handle = self.backend_handle();
            let mut backend = lock(&handle);
            let was_connected = backend.is_connected();
            let neutral = backend.send_neutral();
            backend.disconnect();
            was_connected && neutral.is_ok()
        };
        *lock(&self.output_enabled) = false;
        *lock(&self.last_send_ok) = Some(neutral_sent);
        *lock(&self.last_send_note) = if neutral_sent {
            "shutdown: neutral sent to backend".to_string()
        } else {
            "shutdown: neutral not sent (backend not connected)".to_string()
        };
        ShutdownOutcome {
            stop_signalled,
            neutral_sent,
        }
    }
}

impl AppStateCore {
    /* ------------------------------------------------------------------ */
    /* Engine task + transports (Phase G gate wiring)                      */
    /* ------------------------------------------------------------------ */

    /// Current engine task status (see [`EngineTaskStatus`]).
    #[must_use]
    pub fn engine_status(&self) -> EngineTaskStatus {
        lock(&self.engine_status).clone()
    }

    /// Human-readable engine status detail: the transport state,
    /// retry notes, and failure reasons. Never silently empty when
    /// the status is [`EngineTaskStatus::Failed`].
    #[must_use]
    pub fn engine_status_detail(&self) -> String {
        lock(&self.engine_status_detail).clone()
    }

    /// The last [`EngineReport`] a stopped engine delivered, if any.
    #[must_use]
    pub fn engine_last_report(&self) -> Option<EngineReport> {
        *lock(&self.engine_last_report)
    }

    /// The frame channel into the running engine, when the engine task
    /// is running. Transports feed frames here through the supervisor's
    /// forwarder; tests drive it directly with a fake frame stream.
    /// Whoever feeds frames must also call
    /// [`AppStateCore::note_frame_encoders`] per frame, or the
    /// snapshot's encoder counts stay at their defaults.
    /// [`None`] until [`AppStateCore::start_engine`] runs.
    #[must_use]
    pub fn engine_frame_sender(&self) -> Option<tokio::sync::mpsc::Sender<StreamFrame>> {
        lock(&self.engine_frame_tx).clone()
    }

    /// Records one frame's encoder counts, as reported by whoever
    /// feeds frames into the engine (the transport forwarder calls
    /// this per frame; a test feeder driving
    /// [`AppStateCore::engine_frame_sender`] directly calls it too).
    /// The engine's sink only ever sees pipeline *deltas*, and the
    /// engine repeats the last output — deltas included — on ticks
    /// with no new frame, so deltas must never be accumulated in the
    /// sink (that over-counts). The board's cumulative counts are the
    /// truth: the snapshot tracker diffs them into deltas itself, and
    /// repeats diff to zero. `n_enc` is clamped to `MAX_ENCODERS`.
    pub fn note_frame_encoders(&self, counts: [i16; MAX_ENCODERS], n_enc: usize) {
        let mut accum = lock(&self.engine_encoders);
        accum.counts = counts;
        accum.n_enc = n_enc.min(MAX_ENCODERS);
    }

    /// Records one frame's raw axis readings in mV, as reported by
    /// whoever feeds frames into the engine (the transport forwarder
    /// calls this per authenticated frame, next to
    /// [`AppStateCore::note_frame_encoders`]; a test feeder driving
    /// [`AppStateCore::engine_frame_sender`] directly calls it too).
    /// This is the v2.0.2 capture plumbing: the pipeline only ever
    /// sees the frame inside the engine, so without this slot the raw
    /// values would never reach the state and no centre / pedal
    /// endpoint could be captured from live data. Only the latest
    /// frame is kept — capture samples "now", never a history.
    pub fn note_raw_axes(&self, axes: [Option<u16>; MAX_AXES]) {
        let mut sample = lock(&self.raw_axes);
        sample.axes = axes;
        sample.at = Some(Instant::now());
    }

    /// The latest raw ADC readings in mV ([`RawMv`](rsh_api::RawMv)):
    /// one field per axis slot, [`None`] where the slot was absent
    /// from the latest frame or no frame has arrived yet. The age is
    /// measured from the sample's arrival — never invented.
    #[must_use]
    pub fn raw_mv(&self) -> RawMv {
        let sample = lock(&self.raw_axes);
        RawMv {
            steering_mv: sample.axes[SLOT_STEERING],
            accel_mv: sample.axes[1],
            brake_mv: sample.axes[2],
            clutch_mv: sample.axes[3],
            handbrake_mv: sample.axes[4],
            age_ms: sample
                .at
                .map(|at| u64::try_from(at.elapsed().as_millis()).unwrap_or(u64::MAX)),
        }
    }

    /// The latest raw axes, but only when the sample is fresh at
    /// `now` (see [`RAW_MV_FRESH_FOR`]). Takes the clock so tests can
    /// prove staleness without sleeping. Errors honestly when no
    /// frame was ever seen or the sample is stale — capture must
    /// never save a dead wheel's last reading.
    fn fresh_raw_axes_at(&self, now: Instant) -> Result<[Option<u16>; MAX_AXES], String> {
        let sample = lock(&self.raw_axes);
        let Some(at) = sample.at else {
            return Err(
                "no live wheel data yet — connect the wheel and wait for live data".to_string(),
            );
        };
        let age = now.checked_duration_since(at).unwrap_or(Duration::ZERO);
        if age > RAW_MV_FRESH_FOR {
            return Err(format!(
                "live wheel data is stale ({} ms since the last frame) — reconnect the wheel",
                age.as_millis()
            ));
        }
        Ok(sample.axes)
    }

    /// Capture the steering centre from the live raw reading: the
    /// wheel must be held straight. The current steering mV is saved
    /// as the active profile's centre (persisted to disk, and into
    /// the draft when one exists so the live plan sees it at once),
    /// the capture time is stamped, and the pipeline plan is pushed
    /// live to the running engine. The existing steering min/max are
    /// kept; with no steering calibration yet the centre is stored
    /// with min = max = centre (same rule as
    /// [`Self::save_calibration`]). Returns the captured mV.
    ///
    /// # Errors
    ///
    /// Returns a message when there is no fresh live reading, the
    /// reading is out of range, there is no active profile, or the
    /// save fails.
    pub fn capture_centre(&self) -> Result<u16, String> {
        let axes = self.fresh_raw_axes_at(Instant::now())?;
        let centre = axes[SLOT_STEERING]
            .ok_or_else(|| "no steering reading in the latest frame".to_string())?;
        if centre > 3300 {
            return Err(format!(
                "steering reading {centre} mV is outside 0..=3300 mV"
            ));
        }
        let mut store = lock(&self.store);
        let active = store
            .active()
            .ok_or_else(|| "no active profile".to_string())?;
        let mut profile = active.profile.clone();
        let id = active.id.clone();
        profile.calibrated_at_utc = format_system_time_rfc3339(SystemTime::now());
        profile.steering_calibration = Some(match profile.steering_calibration {
            Some(existing) => SteeringCalibration {
                raw_min: existing.raw_min,
                raw_center: i32::from(centre),
                raw_max: existing.raw_max,
                is_set: true,
                is_millivolts: true,
            },
            None => SteeringCalibration {
                raw_min: i32::from(centre),
                raw_center: i32::from(centre),
                raw_max: i32::from(centre),
                is_set: true,
                is_millivolts: true,
            },
        });
        store
            .update(&id, profile.clone())
            .map_err(|error| error.to_string())?;
        store.save().map_err(|error| error.to_string())?;
        drop(store);
        if let Some(draft) = lock(&self.draft).as_mut() {
            draft.profile.steering_calibration = profile.steering_calibration;
            draft
                .profile
                .calibrated_at_utc
                .clone_from(&profile.calibrated_at_utc);
        }
        self.push_pipeline_plan();
        self.note_slow_state_changed();
        Ok(centre)
    }

    /// Capture one pedal's released ("up") endpoint from the live raw
    /// reading. See [`Self::capture_pedal_max`].
    ///
    /// # Errors
    ///
    /// Returns a message for an unknown pedal, no fresh live reading
    /// (or that axis absent), an invalid resulting range, no active
    /// profile, or a save failure.
    pub fn capture_pedal_min(&self, pedal: &str) -> Result<u16, String> {
        self.capture_pedal_endpoint(pedal, true)
    }

    /// Capture one pedal's fully-pressed ("down") endpoint from the
    /// live raw reading. `pedal` is `"accel"` (or `"accelerator"`),
    /// `"brake"`, `"clutch"`, or `"handbrake"` (case-insensitive).
    /// The current mV is saved as that pedal's minimum / maximum:
    /// into the pedal's calibration record (accel / brake / clutch)
    /// AND into the module tuning's min/max, so the value reaches the
    /// live pipeline plan (the plan reads pedal ranges from the
    /// tuning, not from the calibration record). The handbrake has no
    /// calibration record — only its module tuning is updated. The
    /// profile is persisted to disk, the draft (when one exists) gets
    /// the same values without being marked dirty (the capture is
    /// already saved), the session tuning is updated so the Pedals
    /// page shows the new endpoint, and the plan is pushed live.
    /// Returns the captured mV.
    ///
    /// # Errors
    ///
    /// Returns a message for an unknown pedal, no fresh live reading
    /// (or that axis absent), an invalid resulting range, no active
    /// profile, or a save failure.
    pub fn capture_pedal_max(&self, pedal: &str) -> Result<u16, String> {
        self.capture_pedal_endpoint(pedal, false)
    }

    /// Canonical pedal key, axis slot, and tuning module for a
    /// capture pedal name. Unknown names are refused, never guessed.
    fn pedal_capture_slot(pedal: &str) -> Result<(&'static str, usize, &'static str), String> {
        match pedal.trim().to_ascii_lowercase().as_str() {
            "accel" | "accelerator" => Ok(("accel", 1, "accelerator")),
            "brake" => Ok(("brake", 2, "brake")),
            "clutch" => Ok(("clutch", 3, "clutch")),
            "handbrake" => Ok(("handbrake", 4, "handbrake")),
            other => Err(format!(
                "unknown pedal '{other}' (want accel, brake, clutch, or handbrake)"
            )),
        }
    }

    /// Shared half of [`Self::capture_pedal_min`] /
    /// [`Self::capture_pedal_max`]: `is_min` captures the released
    /// endpoint, otherwise the fully-pressed one. The other endpoint
    /// comes from the captured record when one exists, else the live
    /// tuning, else the range default — and the pair is validated with
    /// the engine's own [`validate_pedal_range`] (min < max, span ≥
    /// 660 mV), so a half-pressed capture is refused with the reason
    /// instead of saving a useless range.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn capture_pedal_endpoint(&self, pedal: &str, is_min: bool) -> Result<u16, String> {
        let (key, slot, module) = Self::pedal_capture_slot(pedal)?;
        let axes = self.fresh_raw_axes_at(Instant::now())?;
        let mv = axes[slot].ok_or_else(|| {
            format!("no live reading for pedal '{key}' (axis not present in the latest frame)")
        })?;
        if mv > 3300 {
            return Err(format!(
                "pedal '{key}' reading {mv} mV is outside 0..=3300 mV"
            ));
        }
        let tuning = self.get_tuning(module);
        let mut store = lock(&self.store);
        let active = store
            .active()
            .ok_or_else(|| "no active profile".to_string())?;
        let mut profile = active.profile.clone();
        let id = active.id.clone();
        // The handbrake has no calibration record — only its module
        // tuning carries the captured endpoints.
        let record = match key {
            "accel" => profile.accel_calibration.as_ref(),
            "brake" => profile.brake_calibration.as_ref(),
            "clutch" => profile.clutch_calibration.as_ref(),
            _ => None,
        };
        let (current_min, current_max) = match record.filter(|c| c.is_set) {
            Some(calibration) => (
                u16::try_from(calibration.raw_min).unwrap_or(0),
                u16::try_from(calibration.raw_max).unwrap_or(3300),
            ),
            None => (
                tuning.min_mv.clamp(0.0, 3300.0) as u16,
                tuning.max_mv.clamp(0.0, 3300.0) as u16,
            ),
        };
        let (min_mv, max_mv) = if is_min {
            (mv, current_max)
        } else {
            (current_min, mv)
        };
        validate_pedal_range(min_mv, max_mv).map_err(|error| match error {
            CalibrationError::BadOrder => format!(
                "pedal '{key}' minimum {min_mv} mV is not below its maximum {max_mv} mV — {} first, then capture the other end",
                if is_min { "capture the maximum" } else { "capture the minimum" }
            ),
            CalibrationError::SpanTooNarrow { span_mv } => format!(
                "pedal '{key}' range {min_mv}..={max_mv} mV spans only {span_mv} mV (need at least {MIN_SPAN_MV} mV) — move the pedal through its full travel and capture again"
            ),
            _ => format!("pedal '{key}' range {min_mv}..={max_mv} mV is invalid"),
        })?;
        profile.calibrated_at_utc = format_system_time_rfc3339(SystemTime::now());
        if key != "handbrake" {
            let slot = match key {
                "accel" => &mut profile.accel_calibration,
                "brake" => &mut profile.brake_calibration,
                _ => &mut profile.clutch_calibration,
            };
            *slot = Some(PedalCalibration {
                raw_min: i32::from(min_mv),
                raw_max: i32::from(max_mv),
                is_set: true,
                is_millivolts: true,
            });
        }
        // The pipeline reads pedal ranges from the module tuning, so
        // the captured endpoint lands there too (store + draft +
        // session tuning below).
        let mut module_tuning = tuning.clone();
        module_tuning.min_mv = f64::from(min_mv);
        module_tuning.max_mv = f64::from(max_mv);
        Self::apply_tuning_to_profile(&mut profile, &module_tuning);
        store
            .update(&id, profile.clone())
            .map_err(|error| error.to_string())?;
        store.save().map_err(|error| error.to_string())?;
        drop(store);
        if let Some(draft) = lock(&self.draft).as_mut() {
            Self::apply_tuning_to_profile(&mut draft.profile, &module_tuning);
            if key != "handbrake" {
                let slot = match key {
                    "accel" => &mut draft.profile.accel_calibration,
                    "brake" => &mut draft.profile.brake_calibration,
                    _ => &mut draft.profile.clutch_calibration,
                };
                *slot = Some(PedalCalibration {
                    raw_min: i32::from(min_mv),
                    raw_max: i32::from(max_mv),
                    is_set: true,
                    is_millivolts: true,
                });
            }
            draft
                .profile
                .calibrated_at_utc
                .clone_from(&profile.calibrated_at_utc);
        }
        lock(&self.tunings).insert(module_tuning.module.clone(), module_tuning);
        self.push_pipeline_plan();
        self.note_slow_state_changed();
        Ok(mv)
    }

    /// Starts the engine task (idempotent: a second call while the
    /// task is running is a no-op `Ok`). Spawns the supervisor std
    /// thread, which runs one [`WheelEngine`](rsh_engine::engine::WheelEngine)
    /// (200 Hz, 250 ms watchdog → NEUTRAL) on a current-thread tokio
    /// runtime, and waits until the supervisor reports the engine
    /// task running (bounded: 5 s — normally a few ms). The engine
    /// starts with **no transport**: it ticks in a clean idle state
    /// (watchdog stale → neutral, no fake frames) until
    /// [`AppStateCore::connect_transport`] feeds it. The engine
    /// never blocks on the UI: the sink hands each tick to
    /// [`AppStateCore::ingest_frame`] and the installed backend under
    /// short-held locks and returns.
    ///
    /// # Errors
    ///
    /// Returns a message when the supervisor thread cannot be spawned
    /// or does not report ready within 5 s.
    pub fn start_engine(self: &Arc<Self>) -> Result<(), String> {
        // DS-16: a stored handle is not proof the supervisor is
        // alive — a thread that panicked or exited early (e.g. the
        // 5 s ready-timeout path, which records `Failed` but keeps
        // the handle) used to make every later `start_engine` a
        // silent no-op while the engine was dead, and
        // `connect_transport` then failed only with "engine command
        // channel is gone". A finished handle is treated as absent:
        // reap it, drop the stale channels, and spawn fresh below.
        let dead_handle = {
            let mut thread = lock(&self.engine_thread);
            if thread
                .as_ref()
                .is_some_and(std::thread::JoinHandle::is_finished)
            {
                thread.take()
            } else {
                None
            }
        };
        if let Some(handle) = dead_handle {
            // Finished, so this joins immediately; a panicked
            // supervisor reports its panic here instead of leaving
            // the status as a stale `Running`.
            if handle.join().is_err() {
                self.set_engine_status(
                    EngineTaskStatus::Failed {
                        reason: "engine supervisor thread panicked".to_string(),
                    },
                    "engine supervisor thread panicked — restarting it".to_string(),
                );
            }
            *lock(&self.engine_cmd_tx) = None;
            *lock(&self.engine_report_rx) = None;
        }
        if lock(&self.engine_thread).is_some() {
            return Ok(());
        }
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel(ENGINE_COMMAND_CAPACITY);
        let (report_tx, report_rx) = std::sync::mpsc::channel();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        *lock(&self.engine_cmd_tx) = Some(cmd_tx);
        *lock(&self.engine_report_rx) = Some(report_rx);
        let state = Arc::clone(self);
        match std::thread::Builder::new()
            .name("rsh-engine-supervisor".to_string())
            .spawn(move || engine_supervisor_main(state, cmd_rx, report_tx, ready_tx))
        {
            Ok(thread) => {
                *lock(&self.engine_thread) = Some(thread);
                if let Ok(()) = ready_rx.recv_timeout(Duration::from_secs(5)) {
                    self.set_engine_status(
                        EngineTaskStatus::Running { transport: None },
                        "engine running, no transport — idle (watchdog neutral)".to_string(),
                    );
                    Ok(())
                } else {
                    let reason = "engine supervisor did not report ready within 5 s".to_string();
                    self.set_engine_status(
                        EngineTaskStatus::Failed {
                            reason: reason.clone(),
                        },
                        reason.clone(),
                    );
                    Err(reason)
                }
            }
            Err(error) => {
                *lock(&self.engine_cmd_tx) = None;
                *lock(&self.engine_report_rx) = None;
                self.set_engine_status(
                    EngineTaskStatus::Failed {
                        reason: format!("could not spawn engine supervisor thread: {error}"),
                    },
                    format!("could not spawn engine supervisor thread: {error}"),
                );
                Err(format!("could not spawn engine supervisor thread: {error}"))
            }
        }
    }

    /// Records what the UI asked for (kind + typed address) so
    /// the Tauri shell can report it even while the engine is idle.
    /// The live connection state is never taken from this record —
    /// only the requested kind/address are (see `transport_state`,
    /// which merges this with the live session-gate state).
    pub fn set_transport_request(&self, kind: TransportKind, address: String) {
        let changed = lock(&self.transport_choice).0 != kind;
        *lock(&self.transport_choice) = (kind, address);
        if changed {
            // DS-27: a transport change invalidates the cached port.
            self.rescan_board_port();
        }
    }

    /// What the UI last asked for (kind + typed address).
    #[must_use]
    pub fn transport_request(&self) -> (TransportKind, String) {
        lock(&self.transport_choice).clone()
    }

    /// Connects a transport to the running engine, starting the engine
    /// first (idle) when it is not running yet. The transport's
    /// forwarder feeds the engine's frame channel; on source loss it
    /// retries every 2 s (the engine keeps ticking neutral meanwhile)
    /// until [`AppStateCore::disconnect_transport`] or the engine
    /// stops. UDP is the startup default (board at the configured
    /// `esp_ip`); serial auto-detects the board port — when no port
    /// is detected this errors honestly and the engine stays idle.
    ///
    /// Connecting a transport does NOT open the session gate: stream
    /// frames are held (SEC-E-14) until a `BoardSession` completes
    /// its HMAC-verified `GET_CAPS` and the session owner calls
    /// [`AppStateCore::open_session_gate`]. Use
    /// [`AppStateCore::transport_state`] to report that honestly.
    ///
    /// # Errors
    ///
    /// Returns a message when no serial board port is detected, or
    /// when the engine command channel is gone. A UDP bind failure is
    /// NOT an error here: the forwarder retries every 2 s and the
    /// reason is recorded in [`AppStateCore::engine_status_detail`].
    ///
    /// R3-D-05: this no-ops (`Ok`, session untouched — the gate is
    /// NOT closed, the latch is NOT cleared) while a forwarder
    /// activation is in flight or the forwarder is waiting out its
    /// own retry backoff — the same guards `connect_session` has.
    /// Without them a UI Connect during `Backoff` / mid-activation
    /// queued a second `EngineCommand::Connect` and the supervisor
    /// aborted the in-flight forwarder, restarting the HELO →
    /// `GET_CAPS` exchange from scratch, while the manager treated
    /// the `Connect` event itself as a no-op. A no-op here is an
    /// explicit "a (re)connect is already running" — the caller
    /// reports the live state (see `DaemonApi::connect`), it does
    /// not start a second activation. An explicit (re)connect that
    /// DOES proceed clears a latched breaker (`forwarder_breaker_open`):
    /// a tripped breaker stays tripped against automatic retries,
    /// not against the user pressing Connect.
    pub fn connect_transport(self: &Arc<Self>, kind: TransportKind) -> Result<(), String> {
        // R3-D-05: never tear an in-flight activation / retry-wait
        // down. The supervisor aborts the running forwarder on every
        // `EngineCommand::Connect`, so queueing one here would
        // restart the exchange the live forwarder is already
        // running — and reset its breaker streak (R3-D-02).
        if self.activation_in_flight.load(Ordering::SeqCst) || self.forwarder_retry_waiting() {
            return Ok(());
        }
        // An explicit (re)connect that proceeds clears a latched
        // breaker: the user asked for a fresh run (R3-D-02).
        self.forwarder_breaker_open.store(false, Ordering::SeqCst);
        // R2-25: the pre-check uses the metadata-only probe (open
        // only, no 500 ms HELO round-trip — the session layer
        // confirms the board with its own HELO/HELLO exchange
        // afterwards), and the background refresher keeps the cached
        // port fresh; the poll path never probes at all.
        if kind == TransportKind::Serial && detect_board_port_metadata_only().is_none() {
            let reason =
                "no board serial port detected — connect a board over USB and retry".to_string();
            self.note_engine_detail(reason.clone());
            return Err(reason);
        }
        if lock(&self.engine_cmd_tx).is_none() {
            self.start_engine()?;
        }
        let tx = lock(&self.engine_cmd_tx)
            .clone()
            .ok_or_else(|| "engine command channel is gone".to_string())?;
        tx.try_send(EngineCommand::Connect(kind)).map_err(|error| {
            format!(
                "could not queue transport connect for {}: {error}",
                kind.as_str()
            )
        })?;
        // R2-02: a (re)connect is a new session — the old
        // session is dead, so the gate closes here (SEC-E-14,
        // same as `disconnect_transport`) and the reachable latch
        // clears (D15: a transport switch clears it; the next
        // HELLO re-sets it). The fresh forwarder
        // reconnects and re-opens it, which is the rising
        // edge the daemon's manager loop needs to feed `CapsOk`.
        // Without this, a reconnect while the gate was still
        // open (e.g. after a watchdog latch left the manager in
        // `Error`) silently re-activated and the manager never
        // learned the session was live again. This closes the
        // gate synchronously on the calling thread (after the
        // command queued successfully), NOT in the supervisor's
        // async `Connect` handler: a late-processed handler
        // close would race a legitimate gate-open that happened
        // after the queue (the tests stand in for the session
        // owner that way). Idempotent at startup / after a
        // failure (gate already closed); frames stay held until
        // the fresh `GET_CAPS` passes.
        self.close_session_gate();
        self.clear_board_reachable();
        Ok(())
    }

    /// Disconnects the live transport (aborts its forwarder). The
    /// engine keeps ticking in its clean idle state (watchdog stale →
    /// neutral) — disconnecting never stops the engine. The session
    /// gate closes too, and the session records are cleared: a later
    /// reconnect is a new session and must reconnect (SEC-E-14).
    ///
    /// # Errors
    ///
    /// Returns a message when the engine is not running.
    pub fn disconnect_transport(&self) -> Result<(), String> {
        self.close_session_gate();
        self.clear_session_records();
        // R3-D-02: a deliberate teardown ends any forwarder retry
        // wait / breaker latch with it — the next explicit connect
        // starts a fresh run. (The aborted forwarder's guards drop
        // too, but clearing here keeps the flags correct even if the
        // supervisor has not processed the abort yet.)
        self.forwarder_retry_wait.store(0, Ordering::SeqCst);
        self.forwarder_breaker_open.store(false, Ordering::SeqCst);
        let tx = lock(&self.engine_cmd_tx)
            .clone()
            .ok_or_else(|| "engine is not running".to_string())?;
        tx.try_send(EngineCommand::Disconnect)
            .map_err(|error| format!("could not queue transport disconnect: {error}"))?;
        self.set_connection(ConnectionState::Disconnected);
        // DS-27: a disconnect invalidates the cached board port.
        self.rescan_board_port();
        Ok(())
    }

    /// Stops the engine task with a bounded wait: signals the stop
    /// handle, then waits at most ~5 s for the engine's final report
    /// and joins the supervisor thread. The engine itself returns
    /// within one 5 ms tick of the stop signal (its `finish()` sends
    /// a final neutral), so the bound is only ever approached when
    /// something is already wrong. When the engine was never started
    /// this is a no-op returning the last delivered report, if any.
    /// A timed-out stop never blocks the caller past the bound: the
    /// thread is detached and the outcome reports `joined: false`.
    #[must_use]
    pub fn stop_engine(&self) -> EngineStopOutcome {
        let stop = lock(&self.stop_handle).clone();
        let Some(handle) = stop else {
            return EngineStopOutcome {
                joined: false,
                report: self.engine_last_report(),
            };
        };
        handle.stop();
        // Stopping the engine ends the session: the gate closes and
        // the session records are cleared with it.
        self.close_session_gate();
        self.clear_session_records();
        let report_rx = lock(&self.engine_report_rx).take();
        let outcome = match report_rx {
            Some(rx) => {
                if let Ok(report) = rx.recv_timeout(ENGINE_STOP_TIMEOUT) {
                    *lock(&self.engine_last_report) = Some(report);
                    EngineStopOutcome {
                        joined: true,
                        report: Some(report),
                    }
                } else {
                    let reason =
                        "engine stop timed out after 5 s — thread detached, outcome unknown"
                            .to_string();
                    self.set_engine_status(
                        EngineTaskStatus::Failed {
                            reason: reason.clone(),
                        },
                        reason,
                    );
                    // Detach the stuck thread so a later start can
                    // retry instead of no-op-ing on a dead handle.
                    let _ = lock(&self.engine_thread).take();
                    EngineStopOutcome {
                        joined: false,
                        report: self.engine_last_report(),
                    }
                }
            }
            None => EngineStopOutcome {
                joined: false,
                report: self.engine_last_report(),
            },
        };
        if outcome.joined {
            // The report arrives as the supervisor thread's last act,
            // so the join below returns promptly; a panicked thread
            // still reports `joined: false` instead of hanging exit.
            let thread = lock(&self.engine_thread).take();
            if let Some(thread) = thread {
                if thread.join().is_ok() {
                    self.set_engine_status(EngineTaskStatus::Stopped, "engine stopped".to_string());
                } else {
                    let reason = "engine supervisor thread panicked".to_string();
                    self.set_engine_status(
                        EngineTaskStatus::Failed {
                            reason: reason.clone(),
                        },
                        reason,
                    );
                    return EngineStopOutcome {
                        joined: false,
                        report: outcome.report,
                    };
                }
            } else {
                self.set_engine_status(EngineTaskStatus::Stopped, "engine stopped".to_string());
            }
        }
        *lock(&self.stop_handle) = None;
        *lock(&self.engine_cmd_tx) = None;
        *lock(&self.engine_frame_tx) = None;
        outcome
    }

    /// Records the engine status + detail together.
    fn set_engine_status(&self, status: EngineTaskStatus, detail: String) {
        *lock(&self.engine_status) = status;
        *lock(&self.engine_status_detail) = detail;
    }

    /// Records an engine note in the status detail without changing
    /// the status itself (transport retries, disconnects). Public
    /// so the daemon API layer can surface a failure it observes
    /// (DS-41: a swallowed `set_output_enabled` failure) in the
    /// same detail the diagnostics already show.
    pub fn note_engine_detail(&self, detail: String) {
        *lock(&self.engine_status_detail) = detail;
    }

    /// Records an engine failure: status + detail both name the
    /// reason, so a failed engine is never silent.
    fn note_engine_failure(&self, reason: String) {
        self.set_engine_status(
            EngineTaskStatus::Failed {
                reason: reason.clone(),
            },
            reason,
        );
    }

    /// Records a transport failure / retry note and, when the engine
    /// is running, keeps the status `Running` (a transport loss is
    /// not an engine failure — the loop keeps ticking neutral).
    fn note_engine_transport_error(&self, kind: TransportKind, reason: &str) {
        // Status stays Running: the engine is alive, only the
        // transport is down — a transport loss is not an engine
        // failure, and the loop keeps ticking neutral.
        *lock(&self.engine_status_detail) = format!(
            "transport {} unavailable: {reason} — retrying every 2 s (engine keeps ticking, watchdog neutral)",
            kind.as_str()
        );
    }

    /// Feeds one engine tick into the state: the sink's view of the
    /// pipeline output becomes a snapshot tick (steering in degrees,
    /// pedals, encoder counts reconstructed from the per-tick
    /// deltas) plus the installed backend's send (below). The
    /// engine's live metrics are not observable per tick (see the
    /// module docs), so the snapshot's p99s stay unmeasured and
    /// `seqGap` / `txFail` stay 0 — the loop rate is measured from
    /// these ticks and is real.
    /// The engine's metrics as last reported by a per-tick send.
    /// The plain-`send` path (the engine's final neutral) reuses
    /// these instead of resetting them to "unmeasured".
    fn live_metrics(&self) -> EngineMetrics {
        *lock(&self.metrics)
    }

    /// Feed one engine tick into the snapshot + button-edge feed
    /// and return the tick the backend should send: the pipeline
    /// output with the persisted bindings applied
    /// ([`AppStateCore::apply_bindings`]). The caller
    /// ([`EngineSink`]) must send the returned value, not the raw
    /// tick — otherwise a bound button would reach the game
    /// un-remapped.
    fn ingest_engine_output(
        &self,
        output: &PipelineOutputState,
        metrics: &EngineMetrics,
    ) -> PipelineOutputState {
        // Encoder counts come from the latest frame (reported by the
        // feeder via `note_frame_encoders`), not from the output's
        // deltas: the engine repeats the last output on ticks with no
        // new frame, so accumulating deltas here would over-count.
        let (encoder_counts, n_enc) = {
            let accum = lock(&self.engine_encoders);
            (accum.counts, accum.n_enc)
        };
        // L8: clutch presence comes from the active profile's clutch
        // module, not from the reading: a present-but-released clutch
        // reads exactly 0.0 and must still report `Some(0.0)`. With
        // no clutch tuning (the Default profile) the old rule stands
        // — a moved clutch is reported, a still one is `None`.
        // L4: the draft counts — a clutch tuning edited but not yet
        // saved is live, so it reports present too.
        let clutch_enabled = self
            .current_profile()
            .and_then(|profile| {
                profile
                    .module_tunings
                    .get("clutch")
                    .map(|tuning| tuning.enabled)
            })
            .unwrap_or(false);
        let clutch = if clutch_enabled || output.clutch > 0.0 {
            Some(f64::from(output.clutch))
        } else {
            None
        };
        let _ = self.ingest_frame(FrameInput {
            steering_deg: f64::from(output.steering) * STEERING_FULL_LOCK_DEG,
            pedals: PedalSnapshot {
                accel: f64::from(output.accelerator),
                brake: f64::from(output.brake),
                clutch,
            },
            encoder_counts,
            n_enc,
            output: *output,
            metrics: *metrics,
        });
        // `ingest_frame` stored the binding-remapped output as
        // `last_output` — that is the tick the backend sends.
        *lock(&self.last_output)
    }

    /// Sends one engine tick to the installed backend. When the
    /// backend is not connected (output disabled, or no backend
    /// installed) this is a success with nothing to send — output
    /// disabled is a legitimate state, not a sink failure, so the
    /// engine must not count it as an error 200 times a second. When
    /// connected, a refused update is a real sink error and the engine
    /// counts it and falls back to neutral, per its contract.
    ///
    /// # Errors
    ///
    /// Returns [`SinkError`] when the connected backend refuses the
    /// state.
    fn send_engine_output(&self, output: &PipelineOutputState) -> Result<(), SinkError> {
        // R2-09 safety net: output off means nothing is sent, even
        // if the backend is still connected (e.g. output was
        // disabled while a backend switch was in flight and the
        // disconnect was skipped). In the normal flow output-off
        // already implies disconnected, so this changes nothing
        // there — it only closes the race window.
        if !*lock(&self.output_enabled) {
            return Ok(());
        }
        // PERF F-15: clone the backend handle under the slot lock,
        // then run the update on the handle's own lock — never on
        // the slot lock. A switch swaps the slot handle without
        // waiting out this update (up to ~200 ms on HIDMaestro's
        // sidecar round-trip), and this tick is not stalled by a
        // switch either: it keeps sending to the instance it cloned
        // and the next tick picks up the new backend.
        let handle = self.backend_handle();
        let mut backend = lock(&handle);
        if !backend.is_connected() {
            return Ok(());
        }
        if backend.update(output).is_err() {
            drop(backend);
            // C-FAIL-1: the live backend refused a mid-session update
            // — try the fallback once (rate-limited), then retry the
            // update there.
            return self.try_mid_session_fallback(output);
        }
        Ok(())
    }

    /// Mid-session failover (C-FAIL-1): on a refused backend update,
    /// switch the live slot to the fallback backend
    /// (`HIDMaestro` → vJoy) and retry the update once there. Fires
    /// at most once until an explicit user re-select calls
    /// [`reset_fallback`](Self::reset_fallback), and at most once
    /// per 30 s even after a reset. The switch is logged in the
    /// engine detail; the persisted backend choice is untouched (the
    /// next launch retries the selected backend). Returns the retry's
    /// outcome, or the original refusal when no fallback exists, the
    /// switch fails, or the rate limit bites.
    fn try_mid_session_fallback(&self, output: &PipelineOutputState) -> Result<(), SinkError> {
        let vjoy_device_id = lock(&self.store).settings().vjoy_device_id;
        self.try_mid_session_fallback_with_factories(
            output,
            &real_backend_factories(vjoy_device_id),
        )
    }

    /// [`try_mid_session_fallback`](Self::try_mid_session_fallback)
    /// with injectable factories (the test seam: tests drive the
    /// failover with mock backends; production uses the real ones).
    fn try_mid_session_fallback_with_factories(
        &self,
        output: &PipelineOutputState,
        factories: &BackendFactories,
    ) -> Result<(), SinkError> {
        const REFUSED: &str = "output backend refused the state";
        if self.fallback_used.load(Ordering::SeqCst) {
            return Err(SinkError::new(REFUSED));
        }
        let now = Instant::now();
        if let Some(last) = *lock(&self.fallback_last_attempt)
            && now.duration_since(last) < Duration::from_secs(30)
        {
            return Err(SinkError::new(REFUSED));
        }
        let live =
            BackendKind::from_backend_name(&self.backend_name()).unwrap_or(BackendKind::None);
        let Some(fallback) = live.fallback() else {
            return Err(SinkError::new(REFUSED));
        };
        *lock(&self.fallback_last_attempt) = Some(now);
        self.fallback_used.store(true, Ordering::SeqCst);
        // Automatic path (mid-session failover): the fallback stays
        // allowed — only an explicit user switch bans it (Bug 4/8).
        match self.apply_backend_switch(fallback, factories, true) {
            Ok(outcome) => {
                self.note_engine_detail(format!(
                    "mid-session failover: {} refused an update, switched to {} \
                     (C-FAIL-1: once until an explicit re-select)",
                    live.display_name(),
                    outcome.active.display_name()
                ));
                // PERF F-15: same handle-clone as
                // `send_engine_output` — the retry update runs on
                // the fresh backend's own lock, never the slot lock.
                let handle = self.backend_handle();
                let mut backend = lock(&handle);
                if !backend.is_connected() {
                    return Err(SinkError::new(REFUSED));
                }
                backend.update(output).map_err(|_| SinkError::new(REFUSED))
            }
            Err(reason) => {
                self.note_engine_detail(format!(
                    "mid-session failover to {} failed: {reason} — staying on {}",
                    fallback.display_name(),
                    live.display_name()
                ));
                Err(SinkError::new(REFUSED))
            }
        }
    }
}

impl AppStateCore {
    /* ------------------------------------------------------------------ */
    /* Profiles + settings                                                 */
    /* ------------------------------------------------------------------ */

    /// List-row summaries for the Profiles screen, in store order.
    #[must_use]
    pub fn list_profiles(&self) -> Vec<ProfileSummary> {
        lock(&self.store).list_summaries()
    }

    /// The active profile's summary, if one is active.
    #[must_use]
    pub fn active_profile(&self) -> Option<ProfileSummary> {
        self.list_profiles()
            .into_iter()
            .find(|summary| summary.active)
    }

    /// Summary for one profile id from an already-locked store.
    fn summary(store: &ProfileStore, id: &str) -> Result<ProfileSummary, String> {
        store
            .list_summaries()
            .into_iter()
            .find(|summary| summary.id == id)
            .ok_or_else(|| format!("no profile with id '{id}'"))
    }

    /// Activates a profile (a manual choice: auto-switch locks off
    /// until re-enabled) and persists the store.
    ///
    /// # Errors
    ///
    /// Returns a message for an unknown id or a store save failure.
    pub fn activate_profile(&self, id: &str) -> Result<ProfileSummary, String> {
        let summary = {
            let mut store = lock(&self.store);
            store
                .activate(id, true)
                .map_err(|error| error.to_string())?;
            store.save().map_err(|error| error.to_string())?;
            Self::summary(&store, id)?
        };
        // A switch drops the old profile's draft (its edits belong to
        // the old profile) and pushes the new profile's plan live.
        *lock(&self.draft) = None;
        self.push_pipeline_plan();
        self.note_slow_state_changed();
        Ok(summary)
    }

    /* ------------------------------------------------------------------ */
    /* Button→action bindings (v2.0.2, Round 2B)                          */
    /* ------------------------------------------------------------------ */

    /// All persisted button→action bindings, in button order.
    #[must_use]
    pub fn list_bindings(&self) -> Vec<ButtonBinding> {
        lock(&self.bindings)
            .list()
            .iter()
            .map(|(&button, action)| ButtonBinding {
                button,
                action: action.clone(),
            })
            .collect()
    }

    /// Bind `button` to `action` and persist it (v2.0.2). The
    /// binding applies from the next engine tick on.
    ///
    /// # Errors
    ///
    /// Returns [`BindingError`] with code `InvalidButton`,
    /// `UnknownAction`, or `BindingSaveFailed`.
    pub fn set_binding(&self, button: u8, action: &str) -> Result<ButtonBinding, BindingError> {
        lock(&self.bindings).set(button, action)?;
        self.note_slow_state_changed();
        Ok(ButtonBinding {
            button,
            action: action.to_string(),
        })
    }

    /// Remove the binding for `button` and persist the removal.
    /// Returns `true` when a binding was actually removed.
    ///
    /// # Errors
    ///
    /// Returns [`BindingError`] with code `BindingSaveFailed` when
    /// the removal cannot be persisted.
    pub fn clear_binding(&self, button: u8) -> Result<bool, BindingError> {
        let removed = lock(&self.bindings).clear(button)?;
        if removed {
            self.note_slow_state_changed();
        }
        Ok(removed)
    }

    /// Load-time warnings from the bindings store (dropped entries,
    /// corrupt file). Empty on a clean load; the daemon logs them at
    /// startup so a dropped binding is never silent.
    #[must_use]
    pub fn binding_load_warnings(&self) -> Vec<String> {
        self.binding_load_warnings.clone()
    }

    /// Load-time warnings from the manual-games store (dropped
    /// entries, corrupt file). Empty on a clean load; the daemon
    /// logs them at startup so a dropped game is never silent.
    #[must_use]
    pub fn manual_game_load_warnings(&self) -> Vec<String> {
        self.manual_game_load_warnings.clone()
    }

    /// Drain the app actions queued by bound button presses (oldest
    /// first). The daemon manager loop runs them — profile cycles
    /// happen there, not on the 200 Hz engine tick.
    #[must_use]
    pub fn drain_binding_actions(&self) -> Vec<BindingAppAction> {
        lock(&self.binding_actions).drain(..).collect()
    }

    /// Cycle the active profile to the next (`forward: true`) or
    /// previous profile (wrap-around; a single profile stays put).
    /// This is what a button bound to `profile_next` /
    /// `profile_previous` runs. Activation saves + pushes the new
    /// profile's pipeline plan, exactly like a manual switch.
    ///
    /// # Errors
    ///
    /// Returns a message when there is no profile to switch to, or
    /// the activation / save fails.
    pub fn cycle_profile(&self, forward: bool) -> Result<ProfileSummary, String> {
        let profiles = self.list_profiles();
        if profiles.is_empty() {
            return Err("no profiles to cycle through".to_string());
        }
        let current = self.active_profile().map(|summary| summary.id);
        let index = current
            .as_deref()
            .and_then(|id| profiles.iter().position(|summary| summary.id == id));
        let next = match index {
            Some(i) if forward => (i + 1) % profiles.len(),
            Some(i) => (i + profiles.len() - 1) % profiles.len(),
            None => 0,
        };
        self.activate_profile(&profiles[next].id)
    }

    /* ------------------------------------------------------------------ */
    /* v2.0.2 Round 3: Wi-Fi password forget, board address, module toggles, */
    /* auto-pause, tray, backup, profile rename/link, knobs, firmware   */
    /* (B5, H2/H3/H4, H7, M1/M6/M7/M9, M-8) — backing impls for the     */
    /* AppApi trait methods; the daemon maps the String errors to       */
    /* typed UserError.                                                 */
    /* ------------------------------------------------------------------ */

    /// Forget the Wi-Fi password ("Forget password / reset to open
    /// AP", D19): deletes the stored `:wifi` password for the
    /// current board, closes the session gate, and clears the
    /// session records. The daemon also deletes the Windows WLAN
    /// profile in the same `clear_wifi_password` call. The
    /// board-side password is cleared only by holding BOOT ≥ 10 s
    /// (D7) — the caller must tell the user that if the board
    /// still has a password. A missing stored password is
    /// already-forgotten (`NotFound` is fine).
    ///
    /// # Errors
    ///
    /// Returns a message when the stored password cannot be deleted
    /// for a reason other than "not stored".
    pub fn clear_wifi_password(self: &Arc<Self>) -> Result<(), String> {
        if let Some(board_id) = self
            .session_activation()
            .map(|activation| activation.board_id)
            .or_else(|| self.last_observed_board_mac())
        {
            match KeyStore::new().delete_wifi_password(&board_id) {
                Ok(()) | Err(KeystoreError::NotFound) => {}
                Err(error) => {
                    return Err(format!(
                        "could not delete the stored Wi-Fi password for {board_id}: {error}"
                    ));
                }
            }
            self.push_connection_log(
                "INFO",
                &format!("wifi password forgotten for board {board_id}"),
            );
        }
        // R3-D-08: the stored password is gone — the cached
        // `password_set` flag must not keep reporting it as set.
        self.invalidate_wifi_password_cache();
        self.close_session_gate();
        self.clear_session_records();
        Ok(())
    }

    /// The board address the transport uses (H4): the persisted
    /// `esp_ip` (sanitized to a private/loopback IPv4 literal on
    /// write, default `192.168.4.1`). The transport reads this on
    /// every connect, so persisting the address is what makes the
    /// edit take effect.
    #[must_use]
    pub fn board_address(&self) -> String {
        self.settings().esp_ip
    }

    /// Persist a new board address (H4). Only a private/loopback
    /// IPv4 literal is accepted — a hostname such as `rsh.local` or
    /// a public IP is rejected with a plain message instead of being
    /// silently replaced by the default, so the user knows the edit
    /// did not take. Returns the address actually stored.
    ///
    /// # Errors
    ///
    /// Returns a message when the address is not a usable board IP
    /// or the settings save fails.
    pub fn set_board_address(self: &Arc<Self>, address: &str) -> Result<String, String> {
        let address = address.trim();
        let usable = address
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|ip| is_allowed_board_ip(&ip));
        if !usable {
            return Err(format!(
                "\"{address}\" is not a board address — enter the wheel's Wi-Fi IP \
                 (for example 192.168.4.1)"
            ));
        }
        let merged = self.set_settings_patch(AppSettingsPatch {
            esp_ip: Some(address.to_string()),
            ..Default::default()
        })?;
        Ok(merged.esp_ip)
    }

    /// Whether a module is enabled (H7): the draft's value wins (it
    /// holds the latest edit), else the stored profile's flag, else
    /// on — except clutch, whose pipeline default is off (see
    /// [`Self::module_plan_tuning`]).
    #[must_use]
    pub fn module_enabled(&self, module: &str) -> bool {
        let module = canonical_module_id(module);
        if let Some(draft) = lock(&self.draft).as_ref()
            && let Some(entry) = draft.profile.module_tunings.get(&module)
        {
            return entry.enabled;
        }
        lock(&self.store)
            .active()
            .and_then(|active| {
                active
                    .profile
                    .module_tunings
                    .get(&module)
                    .map(|entry| entry.enabled)
            })
            .unwrap_or(module != "clutch")
    }

    /// Enable/disable a module (H7): writes the flag into the draft
    /// (same pattern as [`Self::set_tuning`]) and pushes the rebuilt
    /// pipeline plan live, so the toggle takes effect on the next
    /// tick instead of only after Save. Persists on
    /// [`Self::save_draft`]. LED has no pipeline slot — the flag
    /// still persists and the card greys out; the toggle never fails
    /// for a disconnected board.
    ///
    /// # Errors
    ///
    /// Returns a message when there is no active profile to draft
    /// against.
    pub fn set_module_enabled(
        self: &Arc<Self>,
        module: &str,
        enabled: bool,
    ) -> Result<bool, String> {
        let module = canonical_module_id(module);
        let active = {
            lock(&self.store)
                .active()
                .map(|active| active.profile.clone())
                .ok_or_else(|| "no active profile".to_string())?
        };
        {
            let mut draft_slot = lock(&self.draft);
            let draft = draft_slot.get_or_insert_with(|| ProfileDraft::new(active));
            let entry = draft
                .profile
                .module_tunings
                .entry(module.clone())
                .or_default();
            entry.enabled = enabled;
            draft.mark_dirty(&module);
        }
        self.push_pipeline_plan();
        self.note_slow_state_changed();
        Ok(enabled)
    }

    /// Whether auto-switch is paused (M6): the stored
    /// `auto_switch_paused` flag.
    #[must_use]
    pub fn auto_pause(&self) -> bool {
        self.settings().auto_switch_paused
    }

    /// Pause/resume automatic profile switching (M6): persists the
    /// flag and actuates it — paused stops the auto-switch watcher
    /// (see [`Self::sync_auto_switch_from_settings`]), resumed
    /// restarts it when auto-switch is on. Returns the stored value.
    ///
    /// # Errors
    ///
    /// Returns a message when the settings save fails.
    pub fn set_auto_pause(self: &Arc<Self>, paused: bool) -> Result<bool, String> {
        let merged = self.set_settings_patch(AppSettingsPatch {
            auto_switch_paused: Some(paused),
            ..Default::default()
        })?;
        Ok(merged.auto_switch_paused)
    }

    /// Whether the window hides to the tray on close (M7 / A11):
    /// the stored `close_to_tray` flag (default on).
    #[must_use]
    pub fn close_to_tray(&self) -> bool {
        self.settings().close_to_tray
    }

    /// Persist the "keep running in the tray" choice (M7 / A11): the
    /// UI's close handler reads this on every close, so the switch
    /// takes effect immediately. Returns the stored value.
    ///
    /// # Errors
    ///
    /// Returns a message when the settings save fails.
    pub fn set_close_to_tray(self: &Arc<Self>, enabled: bool) -> Result<bool, String> {
        let merged = self.set_settings_patch(AppSettingsPatch {
            close_to_tray: Some(enabled),
            ..Default::default()
        })?;
        Ok(merged.close_to_tray)
    }

    /// Restore the newest present profiles backup (M7): `bak1` first,
    /// else `bak2`. The restored profiles + active id replace the
    /// live ones; the draft and the session tunings are dropped so no
    /// stale edit can overwrite the restore on the next save, and the
    /// pipeline plan is rebuilt from the restored profile. Returns a
    /// plain summary naming the file and when it was saved.
    ///
    /// # Errors
    ///
    /// Returns a message when no profiles backup exists or the
    /// restore fails.
    pub fn restore_latest_backup(&self) -> Result<String, String> {
        let (generation, file, saved_at) = {
            let store = lock(&self.store);
            let backups = store.list_backups();
            let pick = backups
                .iter()
                .find(|info| {
                    info.file == "profiles.json" && info.generation == "bak1" && info.present
                })
                .or_else(|| {
                    backups.iter().find(|info| {
                        info.file == "profiles.json" && info.generation == "bak2" && info.present
                    })
                });
            match pick {
                Some(info) => (
                    if info.generation == "bak1" {
                        BackupGeneration::Bak1
                    } else {
                        BackupGeneration::Bak2
                    },
                    info.file.clone(),
                    info.saved_at.clone(),
                ),
                None => {
                    return Err(
                        "no profiles backup to restore — backups are made automatically \
                         when profiles change"
                            .to_string(),
                    );
                }
            }
        };
        {
            let mut store = lock(&self.store);
            store
                .restore_backup(generation)
                .map_err(|error| error.to_string())?;
        }
        *lock(&self.draft) = None;
        lock(&self.tunings).clear();
        self.push_pipeline_plan();
        Ok(match saved_at {
            Some(when) => format!("Restored {file} backup from {when}"),
            None => format!("Restored {file} backup"),
        })
    }

    /// Rename a profile (M1): validates the name, writes it into the
    /// stored profile, and persists. When the renamed profile is the
    /// active one, the draft's name is updated too, so the next Save
    /// cannot overwrite the rename with the old name. The id never
    /// changes — a rename is not a new profile.
    ///
    /// # Errors
    ///
    /// Returns a message for an invalid name, an unknown id, or a
    /// store save failure.
    pub fn rename_profile(&self, id: &str, name: &str) -> Result<ProfileSummary, String> {
        let errors = validate_profile_name(name);
        if !errors.is_empty() {
            return Err(format!("invalid profile name: {}", errors.join("; ")));
        }
        let name = name.trim().to_string();
        let summary = {
            let mut store = lock(&self.store);
            let mut profile = store
                .list()
                .iter()
                .find(|stored| stored.id == id)
                .map(|stored| stored.profile.clone())
                .ok_or_else(|| format!("no profile with id '{id}'"))?;
            profile.name.clone_from(&name);
            store
                .update(id, profile)
                .map_err(|error| error.to_string())?;
            store.save().map_err(|error| error.to_string())?;
            Self::summary(&store, id)?
        };
        if lock(&self.store)
            .active()
            .is_some_and(|active| active.id == id)
            && let Some(draft) = lock(&self.draft).as_mut()
        {
            draft.profile.name = name;
        }
        self.note_slow_state_changed();
        Ok(summary)
    }

    /// Link a profile to a game (M1): sets the profile's game from
    /// the slug (`"assetto_corsa"`, `"dirt_rally_2"`, …) and persists.
    /// When the linked profile is the active one, the draft's game is
    /// updated too, so the next Save cannot unlink it again.
    ///
    /// # Errors
    ///
    /// Returns a message for an unknown game slug or id, or a store
    /// save failure.
    pub fn link_profile(&self, id: &str, game_slug: &str) -> Result<ProfileSummary, String> {
        // Manual games (game-scanner removal): a profile links to a
        // manual entry by storing the profile id on the entry itself
        // — the profile store only knows the four catalogue `GameId`s,
        // so a manual slug would map to `Unknown` below. The binding
        // is what `list_games` reports as `bound_profile_id`.
        if lock(&self.manual_games).get(game_slug).is_some() {
            if !self.profile_exists(id) {
                return Err(format!("no profile with id '{id}'"));
            }
            let _ = lock(&self.manual_games).set_profile(game_slug, Some(id.to_string()))?;
            self.note_slow_state_changed();
            return Self::summary(&lock(&self.store), id);
        }
        let game = game_id_from_slug(game_slug);
        if game == GameId::Unknown {
            return Err(format!("unknown game '{game_slug}'"));
        }
        let summary = {
            let mut store = lock(&self.store);
            let mut profile = store
                .list()
                .iter()
                .find(|stored| stored.id == id)
                .map(|stored| stored.profile.clone())
                .ok_or_else(|| format!("no profile with id '{id}'"))?;
            profile.game = game;
            store
                .update(id, profile)
                .map_err(|error| error.to_string())?;
            store.save().map_err(|error| error.to_string())?;
            Self::summary(&store, id)?
        };
        if lock(&self.store)
            .active()
            .is_some_and(|active| active.id == id)
            && let Some(draft) = lock(&self.draft).as_mut()
        {
            draft.profile.game = game;
        }
        self.note_slow_state_changed();
        Ok(summary)
    }

    /// Persisted knob actions (M9): `[[action; 3]; 2]` — knob 0/1,
    /// row 0..=2, action id or `""` for not set.
    #[must_use]
    pub fn knob_actions(&self) -> [[String; 3]; 2] {
        lock(&self.bindings).knobs()
    }

    /// Persist one knob action (M9): knob 0/1, row 0..=2, action id
    /// or `""` to clear. Validates the knob/row and the action id.
    ///
    /// # Errors
    ///
    /// Returns [`BindingError`] for an invalid knob, row, or action,
    /// or a persist failure.
    pub fn set_knob_action(&self, knob: u8, row: u8, action: &str) -> Result<(), String> {
        lock(&self.bindings)
            .set_knob(knob, row, action)
            .map_err(|error| error.to_string())?;
        self.note_slow_state_changed();
        Ok(())
    }

    /// Firmware check (H3/M-8): the board's live version (from the
    /// authenticated session, `"Unknown"` with no session) against
    /// the bundled OTA image's version (parsed from the signed
    /// image's `Version` marker — no signature check here; the flash
    /// flow verifies before uploading). `update_available` is true
    /// only when both versions parse and the bundled one is newer —
    /// an unknown board version never claims an update.
    #[must_use]
    pub fn firmware_check(&self) -> FirmwareCheck {
        let board = self
            .firmware_version()
            .unwrap_or_else(|_| "Unknown".to_string());
        let bundled =
            crate::data_dir().and_then(|dir| firmware_image::bundled_firmware_version(&dir));
        let update_available = match &bundled {
            Some(bundled) => {
                firmware_image::compare_versions(bundled, &board)
                    == Some(std::cmp::Ordering::Greater)
            }
            None => false,
        };
        FirmwareCheck {
            board_version: board,
            bundled_version: bundled,
            update_available,
        }
    }

    /// Current firmware-flash status (H-4): the background thread's
    /// progress / outcome, or the idle default when no flash ever
    /// started.
    #[must_use]
    pub fn firmware_update_status(&self) -> FirmwareUpdateStatus {
        lock(&self.firmware_update).clone()
    }

    /// Start a firmware flash from the bundled OTA image (H-4): the
    /// flash runs on a background thread (upload + board restart +
    /// confirm takes minutes) and this returns the running status
    /// immediately — the UI polls
    /// [`Self::firmware_update_status`] for progress and the
    /// confirmed version. Requires a live session (the board version
    /// for the anti-downgrade check and the board id for the keystore
    /// Wi-Fi password both come from it) and a bundled image. A
    /// second start while one is running is an error, never a second
    /// flash. The thread reports every stage into the status slot
    /// and ends it with success + confirmed version, or failure +
    /// the plain reason.
    ///
    /// # Errors
    ///
    /// Returns a message when a flash is already running, there is
    /// no live session, no bundled image, or no stored Wi-Fi
    /// password for this board.
    pub fn start_firmware_update(self: &Arc<Self>) -> Result<FirmwareUpdateStatus, String> {
        if lock(&self.firmware_update).running {
            return Err("a firmware update is already running — wait for it to finish".to_string());
        }
        let activation = self.session_activation().ok_or_else(|| {
            "no live board session — connect the wheel and let it activate before updating \
             its firmware"
                .to_string()
        })?;
        let installed = activation.firmware;
        let board_id = activation.board_id.clone();
        let data_dir = crate::data_dir()
            .ok_or_else(|| "could not resolve the RSH data directory".to_string())?;
        let image_path = firmware_image::bundled_firmware_path(&data_dir).ok_or_else(|| {
            "no bundled firmware image found — check for updates first".to_string()
        })?;
        // R2-20: a flash starts from a freshly written image — never
        // trust the cached version for the pre-flight check.
        firmware_image::invalidate_bundled_firmware_version_cache();
        let bundled = firmware_image::bundled_firmware_version(&data_dir).ok_or_else(|| {
            "the bundled firmware image has no version — it may be corrupt".to_string()
        })?;
        // B6: validate the bundled image against the installed version
        // before starting the flash — a bad image fails here with the
        // typed wording, not mid-upload (the flash flow re-validates
        // before any upload; this is the user-facing wording).
        let validation = validate_firmware_image_against(&image_path, installed);
        if let Some(err) = firmware_validation_user_error(&validation) {
            return Err(format!("{} — {}", err.title, err.what_to_do));
        }
        let password = KeyStore::new().get_wifi_password(&board_id).map_err(|_| {
            "no Wi-Fi password stored for this board — set a Wi-Fi password in Settings first"
                .to_string()
        })?;
        let host: std::net::IpAddr = self
            .settings()
            .esp_ip
            .parse()
            .map_err(|_| "the stored board address is not an IP address".to_string())?;
        let target = WebOtaTarget::new(host, password);
        *lock(&self.firmware_update) = FirmwareUpdateStatus {
            running: true,
            stage: Some("validating".to_string()),
            percent: None,
            done: false,
            success: false,
            version: None,
            error: None,
        };
        let state = Arc::clone(self);
        std::thread::spawn(move || {
            let mut progress = |report: crate::webota::FlashProgress| {
                let mut slot = lock(&state.firmware_update);
                slot.stage = Some(match report.stage {
                    crate::webota::FlashStage::Validating => "validating".to_string(),
                    crate::webota::FlashStage::Uploading => "uploading".to_string(),
                    crate::webota::FlashStage::WaitingForRestart => "waitingForRestart".to_string(),
                    crate::webota::FlashStage::Confirming => "confirming".to_string(),
                });
                slot.percent = report.percent;
            };
            let expected = bundled.clone();
            let mut confirm =
                || wait_for_board_version(&target, &expected, Duration::from_secs(30));
            let result =
                flash_firmware(&image_path, &target, installed, &mut progress, &mut confirm);
            *lock(&state.firmware_update) = FirmwareUpdateStatus {
                running: false,
                stage: None,
                percent: None,
                done: true,
                success: result.success,
                version: result.version,
                error: result.error,
            };
            // The flash outcome changes what `firmware_check`
            // reports — bump so the next slow poll re-reads it.
            state.note_slow_state_changed();
        });
        Ok(self.firmware_update_status())
    }

    /// Creates a profile with default tuning under `name` and persists.
    ///
    /// # Errors
    ///
    /// Returns a message when the name is invalid or saving fails.
    pub fn create_profile(&self, name: &str) -> Result<ProfileSummary, String> {
        let mut store = lock(&self.store);
        let created = store.create(name).map_err(|error| error.to_string())?;
        store.save().map_err(|error| error.to_string())?;
        let summary = Self::summary(&store, &created.id)?;
        self.note_slow_state_changed();
        Ok(summary)
    }

    /// Duplicates a profile (new id, `"copy"` name) and persists.
    ///
    /// # Errors
    ///
    /// Returns a message for an unknown id or a store save failure.
    pub fn duplicate_profile(&self, id: &str) -> Result<ProfileSummary, String> {
        let mut store = lock(&self.store);
        let created = store.duplicate(id).map_err(|error| error.to_string())?;
        store.save().map_err(|error| error.to_string())?;
        let summary = Self::summary(&store, &created.id)?;
        self.note_slow_state_changed();
        Ok(summary)
    }

    /// Deletes a profile and persists.
    ///
    /// # Errors
    ///
    /// Returns a message for an unknown id or a store save failure.
    pub fn delete_profile(&self, id: &str) -> Result<(), String> {
        let mut store = lock(&self.store);
        store.delete(id).map_err(|error| error.to_string())?;
        store.save().map_err(|error| error.to_string())?;
        self.note_slow_state_changed();
        Ok(())
    }

    /// Imports a profile file (size-capped, schema-checked by the
    /// store) and persists.
    ///
    /// # Errors
    ///
    /// Returns a message when the file cannot be read, is too large,
    /// fails schema checks, or saving fails.
    pub fn import_profile(&self, path: &str) -> Result<ProfileSummary, String> {
        let mut store = lock(&self.store);
        let imported = store
            .import_profile(Path::new(path))
            .map_err(|error| error.to_string())?;
        store.save().map_err(|error| error.to_string())?;
        let summary = Self::summary(&store, &imported.id)?;
        self.note_slow_state_changed();
        Ok(summary)
    }

    /// Exports a profile to a file.
    ///
    /// # Errors
    ///
    /// Returns a message for an unknown id or a write failure.
    pub fn export_profile(&self, id: &str, path: &str) -> Result<(), String> {
        lock(&self.store)
            .export_profile(id, Path::new(path))
            .map_err(|error| error.to_string())
    }

    /// Current app settings.
    #[must_use]
    pub fn settings(&self) -> AppSettings {
        lock(&self.store).settings().clone()
    }

    /// Current app settings as a shared pointer (DS-30): clones the
    /// `Arc`, not the struct — the 20 Hz `poll()` path takes this.
    #[must_use]
    pub fn settings_arc(&self) -> Arc<AppSettings> {
        lock(&self.settings_cache).clone()
    }

    /// Re-reads the store's settings into the `Arc` cache (DS-30):
    /// call at the end of every settings write so readers never see
    /// a stale copy. One struct clone per write, never per read.
    fn refresh_settings_cache(&self) {
        let fresh = lock(&self.store).settings().clone();
        *lock(&self.settings_cache) = Arc::new(fresh);
    }

    /// Replaces the app settings and persists them. When the settings
    /// flip the output flag, the change is actuated through the same
    /// path as the Output toggle (disable → neutral send). The
    /// settings are sanitized on the way in (M10), so a public
    /// `esp_ip` or a non-canonical theme never persists; the returned
    /// settings are the sanitized ones actually stored.
    ///
    /// # Errors
    ///
    /// Returns a message when saving fails.
    pub fn set_settings(self: &Arc<Self>, settings: AppSettings) -> Result<AppSettings, String> {
        let apply_output = settings.output_enabled;
        let sanitized = {
            let mut store = lock(&self.store);
            store.set_settings(settings);
            store.save().map_err(|error| error.to_string())?;
            store.settings().clone()
        };
        let current = *lock(&self.output_enabled);
        if current != apply_output {
            let _ = self.set_output_enabled(apply_output);
        }
        self.sync_auto_switch_from_settings(&sanitized);
        // DS-30: the write above changed the store — refresh the
        // `Arc` cache so `settings_arc()` readers see it at once.
        self.refresh_settings_cache();
        Ok(sanitized)
    }

    /// Applies a partial settings update (C-SET-1): only the fields
    /// present in `patch` overwrite the stored settings; absent
    /// fields keep their values, so a partial save can never wipe a
    /// setting the caller did not send. The result is sanitized
    /// (M10) and persisted, and the output flag / auto-switch flag
    /// are actuated the same way as a full replace. Returns the
    /// merged, sanitized settings actually stored.
    ///
    /// # Errors
    ///
    /// Returns a message when saving fails.
    pub fn set_settings_patch(
        self: &Arc<Self>,
        patch: AppSettingsPatch,
    ) -> Result<AppSettings, String> {
        let merged = {
            let mut store = lock(&self.store);
            let merged = store.set_settings_patch(patch);
            store.save().map_err(|error| error.to_string())?;
            merged
        };
        let current = *lock(&self.output_enabled);
        if current != merged.output_enabled {
            let _ = self.set_output_enabled(merged.output_enabled);
        }
        self.sync_auto_switch_from_settings(&merged);
        // DS-30: refresh the `Arc` cache (see `set_settings`).
        self.refresh_settings_cache();
        Ok(merged)
    }

    /// Syncs the auto-switch watcher with the stored settings
    /// (C-SET-1 / H3): when `auto_switch_profiles` is on AND auto-pause
    /// is off, the manual-override lock is cleared and the watcher
    /// thread starts (when it is not running); when off — or paused
    /// (M6) — the thread stops. Called after every settings write.
    fn sync_auto_switch_from_settings(self: &Arc<Self>, settings: &AppSettings) {
        if settings.auto_switch_profiles && !settings.auto_switch_paused {
            let cleared = {
                let mut store = lock(&self.store);
                if store.auto_switch_locked() {
                    store.clear_auto_switch_lock();
                    true
                } else {
                    false
                }
            };
            if cleared {
                // Persist the cleared lock, or the next launch
                // re-locks auto-switch.
                let _ = lock(&self.store).save();
            }
            self.start_auto_switch();
        } else {
            self.stop_auto_switch();
        }
    }

    /// Imports C# profiles from a file (M6): validates each imported
    /// profile and persists the store. Returns summaries of the
    /// profiles that were actually added (diffed by id, so a
    /// renamed duplicate reports its real stored name). See
    /// [`ProfileStore::import_csharp_profiles`].
    ///
    /// # Errors
    ///
    /// Returns a message when the file cannot be read or no profile
    /// validates.
    pub fn import_csharp_profiles(&self, path: &str) -> Result<Vec<ProfileSummary>, String> {
        let mut store = lock(&self.store);
        let before: std::collections::HashSet<String> = store
            .list()
            .iter()
            .map(|stored| stored.id.clone())
            .collect();
        store
            .import_csharp_profiles(Path::new(path))
            .map_err(|error| error.to_string())?;
        store.save().map_err(|error| error.to_string())?;
        Ok(store
            .list()
            .iter()
            .filter(|stored| !before.contains(&stored.id))
            .filter_map(|stored| Self::summary(&store, &stored.id).ok())
            .collect())
    }

    /// Imports C# settings from a file as a MERGE (M6): only the
    /// fields present in the file overwrite the stored settings.
    /// Returns the merged, sanitized settings.
    ///
    /// # Errors
    ///
    /// Returns a message when the file cannot be read or saving fails.
    pub fn import_csharp_settings(&self, path: &str) -> Result<AppSettings, String> {
        let merged = {
            let mut store = lock(&self.store);
            let merged = store
                .import_csharp_settings(Path::new(path))
                .map_err(|error| error.to_string())?;
            store.save().map_err(|error| error.to_string())?;
            merged
        };
        Ok(merged)
    }

    /// Forces a backup rotation now and returns the backup list.
    ///
    /// # Errors
    ///
    /// Returns a message when a backup file cannot be moved.
    pub fn backup_now(&self) -> Result<Vec<BackupInfo>, String> {
        lock(&self.store)
            .backup_now()
            .map_err(|error| error.to_string())
    }

    /// Restores one backup over the live store file (M9): `path` is
    /// the real path of a backup file — a generation file
    /// (`profiles.json`, `profiles.json.bak1`, `profiles.json.bak2`,
    /// `settings.json`, `settings.json.bak1`, `settings.json.bak2`)
    /// in the store directory, or a bare generation name resolved
    /// against the store directory. Restoring the live file itself is
    /// a no-op. The in-memory state reloads from the restored file.
    ///
    /// # Errors
    ///
    /// Returns a message when the path names no known file, the file
    /// does not exist, or it cannot be read/parsed.
    pub fn restore_backup(&self, path: &str) -> Result<(), String> {
        lock(&self.store)
            .restore_backup_path(Path::new(path))
            .map_err(|error| error.to_string())
    }

    /* ------------------------------------------------------------------ */
    /* Games                                                               */
    /* ------------------------------------------------------------------ */

    /// Every manually added game with its install state, bound
    /// profile, and running state (the tracked game only — refresh
    /// first, so a game that exited on its own stops showing as
    /// running). There is no detection: the list is exactly what the
    /// user added (game-scanner removal, v2.0.3+). `installed` is
    /// true when the stored exe/shortcut path exists on disk.
    #[must_use]
    pub fn list_games(&self) -> Vec<GameInfo> {
        let games = lock(&self.manual_games).list().to_vec();
        let mut launcher = lock(&self.launcher);
        let _ = launcher.refresh();
        let running_id = launcher.running().map(|running| running.game_id.clone());
        games
            .into_iter()
            .map(|game| GameInfo {
                id: game.id.clone(),
                name: game.name.clone(),
                bound_profile_id: game.bound_profile_id.clone(),
                running: running_id.as_deref() == Some(game.id.as_str()),
                installed: Path::new(&game.exe_path).is_file(),
            })
            .collect()
    }

    /// One game's info, or an error for an unknown game id.
    fn game_info(&self, id: &str) -> Result<GameInfo, String> {
        self.list_games()
            .into_iter()
            .find(|game| game.id == id)
            .ok_or_else(|| format!("unknown game '{id}'"))
    }

    /// True when the launcher currently tracks a running game
    /// (R3-D-11: the daemon's background-resilience guard escalates
    /// while a game runs, not only while the wheel is Active).
    /// Cheap lock read only — no `refresh()`, so a game that just
    /// exited on its own still reads as running until the next
    /// slow-poll refresh clears the record (~1 s late at most, and
    /// escalating a little long is harmless; relaxing late is not
    /// a correctness issue for the guard).
    #[must_use]
    pub fn is_game_running(&self) -> bool {
        lock(&self.launcher).running().is_some()
    }

    /// Launches a game (exactly one at a time; the launcher tracks the
    /// pid). The running record is what `stop_game` kills — never any
    /// other same-name process. H4: the active profile before the
    /// launch is captured, so `stop_game` can restore it when the
    /// game closes (the watcher never restores).
    ///
    /// # Errors
    ///
    /// Returns a message when the game is unknown, not installed,
    /// another game is already running, or the OS refuses to start it.
    pub fn launch_game(&self, id: &str) -> Result<GameInfo, String> {
        use crate::launcher::LaunchError;
        let game = lock(&self.manual_games)
            .get(id)
            .cloned()
            .ok_or_else(|| format!("unknown game '{id}'"))?;
        if !Path::new(&game.exe_path).is_file() {
            return Err(format!(
                "game '{}' is not installed (exe not found: {})",
                game.id, game.exe_path
            ));
        }
        {
            let mut launcher = lock(&self.launcher);
            launcher
                .launch(&game.id, &game.exe_path)
                .map_err(|error| match error {
                    LaunchError::NotInstalled(game_id) => {
                        format!("game '{game_id}' is not installed")
                    }
                    LaunchError::AlreadyRunning(game_id) => {
                        format!("game '{game_id}' is already running")
                    }
                    LaunchError::SpawnFailed(message) => {
                        format!("failed to launch game: {message}")
                    }
                    LaunchError::UnsupportedPlatform(detail) => {
                        format!("game launch is not supported on this platform: {detail}")
                    }
                })?;
        }
        // H4: capture the pre-game profile for the stop-time restore.
        let pre_game = lock(&self.store).active().map(|active| active.id.clone());
        *lock(&self.pre_game_profile) = pre_game;
        self.queue_app_event(
            GAME_STATE_EVENT,
            serde_json::json!({ "gameId": id, "state": "launched" }),
        );
        let info = self.game_info(id)?;
        self.note_slow_state_changed();
        Ok(info)
    }

    /// Stops the tracked running game (only the tracked pid). H4:
    /// after a successful stop the pre-game profile captured by
    /// `launch_game` is restored (automatic activation, persisted).
    /// The refresh + stop run under one launcher lock (M12), so the
    /// record cannot change between the check and the kill.
    ///
    /// # Errors
    ///
    /// Returns a message when nothing is running, another game is
    /// running, or the tracked process could not be terminated.
    pub fn stop_game(&self, id: &str) -> Result<GameInfo, String> {
        {
            let mut launcher = lock(&self.launcher);
            let running_id = launcher.refresh().map(|running| running.game_id.clone());
            match running_id.as_deref() {
                None => return Err("no game is running".to_string()),
                Some(running) if running != id => {
                    return Err(format!(
                        "game '{id}' is not running (running game is '{running}')"
                    ));
                }
                Some(_) => {}
            }
            launcher.stop()?;
        }
        // H4: restore the pre-game profile (best effort — a failed
        // restore must not turn a successful stop into an error).
        if let Some(pre_game) = lock(&self.pre_game_profile).take() {
            let restored = {
                let mut store = lock(&self.store);
                store.activate(&pre_game, false).is_ok() && store.save().is_ok()
            };
            if restored {
                self.queue_app_event(
                    GAME_STATE_EVENT,
                    serde_json::json!({
                        "gameId": id,
                        "state": "stopped",
                        "restoredProfileId": pre_game,
                    }),
                );
            } else {
                self.queue_app_event(
                    GAME_STATE_EVENT,
                    serde_json::json!({ "gameId": id, "state": "stopped" }),
                );
            }
        } else {
            self.queue_app_event(
                GAME_STATE_EVENT,
                serde_json::json!({ "gameId": id, "state": "stopped" }),
            );
        }
        let info = self.game_info(id)?;
        self.note_slow_state_changed();
        Ok(info)
    }

    /// One game's current state (installed / running / bound profile).
    ///
    /// # Errors
    ///
    /// Returns a message for an unknown game id.
    pub fn get_game_state(&self, id: &str) -> Result<GameInfo, String> {
        self.game_info(id)
    }

    /// True when a profile with `id` exists in the store.
    fn profile_exists(&self, id: &str) -> bool {
        lock(&self.store)
            .list()
            .iter()
            .any(|stored| stored.id == id)
    }

    /// Validate an optional bound profile id: `None` (or blank)
    /// means "no profile bound"; otherwise the profile must exist.
    fn check_bound_profile(&self, profile_id: Option<String>) -> Result<Option<String>, String> {
        let profile_id = profile_id.filter(|id| !id.trim().is_empty());
        if let Some(id) = &profile_id
            && !self.profile_exists(id)
        {
            return Err(format!("no profile with id '{id}'"));
        }
        Ok(profile_id)
    }

    /// Add a manual game (name + exe/shortcut path, optional bound
    /// profile) and persist it. The id is derived from the name.
    ///
    /// # Errors
    ///
    /// Returns a message for an empty name or exe path, an unknown
    /// bound profile, a full list, or a save failure.
    pub fn add_manual_game(
        &self,
        name: &str,
        exe_path: &str,
        bound_profile_id: Option<String>,
    ) -> Result<GameInfo, String> {
        let bound_profile_id = self.check_bound_profile(bound_profile_id)?;
        let game = lock(&self.manual_games).add(name, exe_path, bound_profile_id)?;
        let info = self.game_info(&game.id)?;
        self.note_slow_state_changed();
        Ok(info)
    }

    /// Update a manual game's name / exe path / bound profile. The
    /// id is stable and never changes. Refuses an exe-path change
    /// while the game is running (the tracked record holds the old
    /// path); name / profile changes are fine mid-run.
    ///
    /// # Errors
    ///
    /// Returns a message for an unknown id, an empty name or exe
    /// path, an unknown bound profile, a mid-run exe change, or a
    /// save failure.
    pub fn update_manual_game(
        &self,
        id: &str,
        name: &str,
        exe_path: &str,
        bound_profile_id: Option<String>,
    ) -> Result<GameInfo, String> {
        let bound_profile_id = self.check_bound_profile(bound_profile_id)?;
        {
            let running_id = lock(&self.launcher)
                .refresh()
                .map(|running| running.game_id.clone());
            if running_id.as_deref() == Some(id) {
                let stored = lock(&self.manual_games)
                    .get(id)
                    .cloned()
                    .ok_or_else(|| format!("unknown game '{id}'"))?;
                if stored.exe_path.trim() != exe_path.trim() {
                    return Err(format!(
                        "game '{id}' is running — stop it before changing its exe path"
                    ));
                }
            }
        }
        let game = lock(&self.manual_games).update(id, name, exe_path, bound_profile_id)?;
        let info = self.game_info(&game.id)?;
        self.note_slow_state_changed();
        Ok(info)
    }

    /// Remove a manual game. Refuses while the game is running
    /// (stop it first — the launcher tracks the running game by id).
    ///
    /// # Errors
    ///
    /// Returns a message for an unknown id, a running game, or a
    /// save failure.
    pub fn remove_manual_game(&self, id: &str) -> Result<bool, String> {
        {
            let running_id = lock(&self.launcher)
                .refresh()
                .map(|running| running.game_id.clone());
            if running_id.as_deref() == Some(id) {
                return Err(format!(
                    "game '{id}' is running — stop it before removing it"
                ));
            }
        }
        lock(&self.manual_games).remove(id)?;
        self.note_slow_state_changed();
        Ok(true)
    }

    /* ------------------------------------------------------------------ */
    /* Tuning + calibration                                                */
    /* ------------------------------------------------------------------ */

    /// The tuning for one module: the session's validated tuning when
    /// one was set, else the active profile's stored tuning converted
    /// to the frontend shape, else the neutral default (deadzone 0,
    /// linear curve, full 0–3300 mV span, no smoothing). Nothing here
    /// is invented data — the default is the documented neutral tuning.
    /// The module id is canonicalized first (C-MOD-1: `"accel"` →
    /// `"accelerator"`), so a caller using either spelling reads the
    /// same tuning. The extended fields (C-TUNE-1: min/max mV, bite
    /// point, horn button, LED brightness/pattern) are read back from
    /// the stored tuning instead of hardcoded `None`/0–3300.
    #[must_use]
    pub fn get_tuning(&self, module: &str) -> TuningSettings {
        let module = canonical_module_id(module);
        if let Some(tuning) = lock(&self.tunings).get(&module) {
            return tuning.clone();
        }
        // Bug 7 (PC-test fix): steering invert + lock live on the
        // profile (`invert_steering` / `steering_scale`), not in the
        // per-module tuning — both branches below report them, so
        // the UI can init the Advanced controls from a stored
        // profile even before any steering edit created a module
        // entry. Other modules never carry them (`None`).
        let steering_opts = |profile: &WheelProfile| -> (Option<bool>, Option<f64>) {
            if module == "steering" {
                (
                    Some(profile.invert_steering),
                    Some(snap_steering_lock_deg(profile.steering_scale * 900.0)),
                )
            } else {
                (None, None)
            }
        };
        if let Some(active) = lock(&self.store).active()
            && let Some(stored) = active.profile.module_tunings.get(&module)
        {
            let (invert, steering_lock_deg) = steering_opts(&active.profile);
            return TuningSettings {
                module: module.clone(),
                deadzone: stored.deadzone * 100.0,
                curve: 1.0 + stored.curve_strength,
                min_mv: stored.min_mv.unwrap_or(0.0),
                max_mv: stored.max_mv.unwrap_or(3300.0),
                smoothing: stored.smoothing * 100.0,
                invert,
                steering_lock_deg,
                bite_point: stored.bite_point,
                shifter_gate: if module == "shifter" {
                    Some(if stored.gate_mode.eq_ignore_ascii_case("h-pattern") {
                        ShifterGate::HPattern
                    } else {
                        ShifterGate::Sequential
                    })
                } else {
                    None
                },
                horn_button: stored.horn_button,
                led_brightness: stored.led_brightness,
                led_pattern: stored
                    .led_pattern
                    .as_deref()
                    .and_then(LedPattern::parse_name),
                // M2/H8: stored colour (hex) + handbrake mode round-trip.
                led_color: stored.led_color.as_deref().and_then(parse_hex_color),
                handbrake_button_mode: stored.handbrake_button_mode,
                // --- Encoder per-module tuning ---
                // Old profiles carry neither field: the defaults
                // (50 ms pulse, 4 counts per click) apply.
                encoder_pulse_ms: Some(stored.encoder_pulse_ms.unwrap_or(50.0)),
                encoder_counts_per_click: Some(stored.encoder_counts_per_click.unwrap_or(4)),
            };
        }
        let (invert, steering_lock_deg) = lock(&self.store)
            .active()
            .map_or((None, None), |active| steering_opts(&active.profile));
        TuningSettings {
            module,
            deadzone: 0.0,
            curve: 1.0,
            min_mv: 0.0,
            max_mv: 3300.0,
            smoothing: 0.0,
            invert,
            steering_lock_deg,
            bite_point: None,
            shifter_gate: None,
            horn_button: None,
            led_brightness: None,
            led_pattern: None,
            led_color: None,
            handbrake_button_mode: None,
            // --- Encoder per-module tuning ---
            encoder_pulse_ms: Some(50.0),
            encoder_counts_per_click: Some(4),
        }
    }

    /// Validates and stores one module's tuning for this session, and
    /// writes it into the in-memory draft (L4, D-1) — NOT to disk:
    /// the tuning survives a restart only after an explicit Save
    /// ([`Self::save_draft`]). The same call pushes the rebuilt
    /// [`PipelinePlan`] live to the running engine, so a slider move
    /// takes effect on the next tick instead of only after Save.
    /// The curve mapping is approximate (exponent 1.0 = linear
    /// strength 0.0). The module id is canonicalized (C-MOD-1). When
    /// the module is `"led"` and a pattern is set, the LED test is
    /// applied best-effort (C-LED-1): a board that is not connected
    /// never fails the tuning save.
    ///
    /// # Errors
    ///
    /// Returns a message when the tuning fails validation or there is
    /// no active profile to draft against.
    pub fn set_tuning(self: &Arc<Self>, tuning: &TuningSettings) -> Result<TuningSettings, String> {
        let errors = validate_tuning(tuning);
        if !errors.is_empty() {
            return Err(format!("invalid tuning: {}", errors.join("; ")));
        }
        let tuning = TuningSettings {
            module: canonical_module_id(&tuning.module),
            ..tuning.clone()
        };
        lock(&self.tunings).insert(tuning.module.clone(), tuning.clone());
        // Draft first (from the stored active profile when this is
        // the first edit since the last save/switch), then mark it
        // dirty. No store write, no disk save here.
        let active = {
            let store = lock(&self.store);
            store
                .active()
                .map(|active| active.profile.clone())
                .ok_or_else(|| "no active profile".to_string())?
        };
        {
            let mut draft_slot = lock(&self.draft);
            let draft = draft_slot.get_or_insert_with(|| ProfileDraft::new(active));
            Self::apply_tuning_to_profile(&mut draft.profile, &tuning);
            draft.mark_dirty(&tuning.module);
        }
        self.push_pipeline_plan();
        // C-LED-1: apply the LED pattern best-effort — a board that
        // is not connected never fails the tuning save.
        if tuning.module == "led"
            && let Some(pattern) = tuning.led_pattern
        {
            let brightness = tuning.led_brightness.unwrap_or(100);
            let _ = self.led_test_with_color(pattern, Some(brightness), None, tuning.led_color);
        }
        self.note_slow_state_changed();
        Ok(tuning)
    }

    /// Applies one validated tuning into a profile's stored
    /// per-module tuning (deadzone, curve, smoothing, gate mode,
    /// min/max mV, bite point, horn button, LED brightness/pattern —
    /// all of them, not just the core three). Shared by the draft
    /// path ([`Self::set_tuning`]); the store itself is never touched
    /// here.
    fn apply_tuning_to_profile(profile: &mut WheelProfile, tuning: &TuningSettings) {
        let entry = profile
            .module_tunings
            .entry(tuning.module.clone())
            .or_insert_with(|| PerModuleTuning {
                deadzone: 0.0,
                curve_strength: 0.0,
                smoothing: 0.0,
                invert: false,
                scale: 1.0,
                enabled: true,
                gate_mode: "Sequential".to_string(),
                min_mv: None,
                max_mv: None,
                bite_point: None,
                horn_button: None,
                led_brightness: None,
                led_pattern: None,
                led_color: None,
                handbrake_button_mode: None,
                // --- Encoder per-module tuning ---
                encoder_pulse_ms: None,
                encoder_counts_per_click: None,
            });
        entry.deadzone = tuning.deadzone / 100.0;
        entry.curve_strength = (tuning.curve - 1.0).clamp(-1.0, 1.0);
        entry.smoothing = tuning.smoothing / 100.0;
        // Bug 7 (PC-test fix): steering invert + lock are profile
        // fields — the pipeline plan reads `invert_steering` /
        // `steering_scale` from the profile, not the module entry —
        // so a steering edit writes both, keeping the entry in sync
        // for the C# export shape. Other modules never carry them.
        if tuning.module == "steering" {
            if let Some(invert) = tuning.invert {
                profile.invert_steering = invert;
                entry.invert = invert;
            }
            if let Some(lock_deg) = tuning.steering_lock_deg {
                let scale = steering_lock_deg_to_scale(lock_deg);
                profile.steering_scale = scale;
                entry.scale = scale;
            }
        }
        entry.min_mv = Some(tuning.min_mv);
        entry.max_mv = Some(tuning.max_mv);
        entry.bite_point = tuning.bite_point;
        entry.horn_button = tuning.horn_button;
        entry.led_brightness = tuning.led_brightness;
        entry.led_pattern = tuning.led_pattern.map(|pattern| pattern.name().to_string());
        // M2/H8: colour persists as "#RRGGBB", handbrake mode as-is.
        entry.led_color = tuning.led_color.map(|color| format!("#{color:06X}"));
        entry.handbrake_button_mode = tuning.handbrake_button_mode;
        // --- Encoder per-module tuning ---
        entry.encoder_pulse_ms = tuning.encoder_pulse_ms;
        entry.encoder_counts_per_click = tuning.encoder_counts_per_click;
        if let Some(gate) = tuning.shifter_gate {
            entry.gate_mode = match gate {
                ShifterGate::Sequential => "Sequential".to_string(),
                ShifterGate::HPattern => "H-pattern".to_string(),
            };
        }
    }

    /// Draft status for the UI's Save / Discard prompt ([`DraftStatus`]):
    /// [`None`] when no draft exists (nothing was edited since the
    /// last save or profile switch — nothing to prompt about).
    #[must_use]
    pub fn draft_status(&self) -> Option<DraftStatus> {
        let profile_id = lock(&self.store).active().map(|active| active.id.clone())?;
        lock(&self.draft)
            .as_ref()
            .map(|draft| draft.status(&profile_id))
    }

    /// Whether the draft has unsaved edits.
    #[must_use]
    pub fn draft_dirty(&self) -> bool {
        lock(&self.draft).as_ref().is_some_and(|draft| draft.dirty)
    }

    /// Persists the draft to disk (L4 Save): writes the draft profile
    /// into the store and saves, then marks the draft clean. The live
    /// plan already matches the draft (every `set_tuning` pushed it),
    /// so no push is needed here. Returns `true` when something was
    /// actually saved, `false` when there was no dirty draft.
    ///
    /// # Errors
    ///
    /// Returns a message when there is no active profile or the store
    /// save fails.
    pub fn save_draft(&self) -> Result<bool, String> {
        let Some(profile) = lock(&self.draft)
            .as_ref()
            .filter(|draft| draft.dirty)
            .map(|draft| draft.profile.clone())
        else {
            return Ok(false);
        };
        let mut store = lock(&self.store);
        let id = store
            .active()
            .map(|active| active.id.clone())
            .ok_or_else(|| "no active profile".to_string())?;
        store
            .update(&id, profile)
            .map_err(|error| error.to_string())?;
        store.save().map_err(|error| error.to_string())?;
        drop(store);
        if let Some(draft) = lock(&self.draft).as_mut() {
            draft.mark_clean();
        }
        self.note_slow_state_changed();
        Ok(true)
    }

    /// Discards the draft (L4 Discard): the session tunings revert to
    /// the stored profile and the live plan is pushed again from
    /// disk, so the wheel behaves as the saved profile says. Returns
    /// `true` when a dirty draft was discarded, `false` when there
    /// was nothing to discard.
    pub fn discard_draft(self: &Arc<Self>) -> bool {
        let had_dirty = lock(&self.draft).take().is_some_and(|draft| draft.dirty);
        if had_dirty {
            lock(&self.tunings).clear();
            self.push_pipeline_plan();
            self.note_slow_state_changed();
        }
        had_dirty
    }

    /// Switches profile with the dirty guard (L4, preview §8.4): when
    /// the draft is dirty and `discard_unsaved` is false, the switch
    /// is refused with a message naming the profile — the UI shows
    /// Save / Discard and retries with `discard_unsaved: true` (or
    /// saves first via [`Self::save_draft`]). A successful switch
    /// drops the old draft and pushes the new profile's plan live.
    ///
    /// # Errors
    ///
    /// Returns a message for a dirty draft (without discard), an
    /// unknown id, or a store save failure.
    pub fn switch_profile(
        self: &Arc<Self>,
        id: &str,
        discard_unsaved: bool,
    ) -> Result<ProfileSummary, String> {
        if self.draft_dirty() && !discard_unsaved {
            let name = self
                .draft_status()
                .map(|status| status.profile_name)
                .unwrap_or_default();
            return Err(format!(
                "unsaved changes in profile '{name}': save or discard them before switching profiles"
            ));
        }
        if discard_unsaved {
            let _ = self.discard_draft();
        }
        self.activate_profile(id)
    }

    /// Installs the live-plan sender (called by the engine supervisor
    /// when the engine starts; the supervisor owns the channel).
    pub fn set_plan_sender(&self, tx: tokio::sync::watch::Sender<PipelinePlan>) {
        *lock(&self.plan_tx) = Some(tx);
    }

    /// Builds the live [`PipelinePlan`] from the current tunings:
    /// the session tunings (draft values when edited, else the stored
    /// profile's) plus the profile's enable flags, invert flags,
    /// steering scale, calibration centre, ramp times, and
    /// combined-pedals switch. The plan is sanitized by the pipeline
    /// on entry, so this never has to re-check ranges.
    #[must_use]
    pub fn build_pipeline_plan(&self) -> PipelinePlan {
        let profile = self.current_profile();
        let mut plan = PipelinePlan::default();
        for (module, slot) in [
            ("steering", &mut plan.steering),
            ("accelerator", &mut plan.accelerator),
            ("brake", &mut plan.brake),
            ("clutch", &mut plan.clutch),
            ("handbrake", &mut plan.handbrake),
            ("shifter", &mut plan.shifter),
            ("horn", &mut plan.horn),
            ("buttons", &mut plan.buttons),
            ("encoders", &mut plan.encoders),
        ] {
            *slot = Self::module_plan_tuning(profile.as_ref(), module, &self.get_tuning(module));
        }
        plan.combine_pedals = profile.as_ref().is_some_and(|p| p.combine_pedals);
        plan
    }

    /// One module's pipeline tuning from its frontend tuning plus the
    /// profile's stored flags for that module.
    ///
    /// The casts below are intentional and safe: every value is
    /// clamped to its documented range just before the cast
    /// (deadzone 0..=0.5, curve -1..=1, mV 0..=3300, bite 0..=1), and
    /// the pipeline itself is `f32`, so the `f64` → `f32` step loses
    /// only precision the pipeline never had.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn module_plan_tuning(
        profile: Option<&WheelProfile>,
        module: &str,
        tuning: &TuningSettings,
    ) -> rsh_engine::pipeline::ModuleTuning {
        use rsh_engine::pipeline::{ModuleTuning, ShifterGateMode};
        let canonical = canonical_module_id(module);
        let stored = profile.and_then(|p| p.module_tunings.get(&canonical));
        let invert = match (profile, module) {
            (Some(p), "steering") => p.invert_steering,
            (Some(p), "accelerator") => p.invert_accel,
            (Some(p), "brake") => p.invert_brake,
            _ => false,
        };
        let (scale, center_mv, press_ms, release_ms) = match profile {
            Some(p) => (
                if module == "steering" {
                    p.steering_scale as f32
                } else {
                    1.0
                },
                if module == "steering" {
                    p.steering_calibration
                        .as_ref()
                        .filter(|c| c.is_set)
                        .map_or(ModuleTuning::default().center_mv, |cal| {
                            cal.raw_center.clamp(0, 3300) as u16
                        })
                } else {
                    ModuleTuning::default().center_mv
                },
                p.pedal_press_ms as f32,
                p.pedal_release_ms as f32,
            ),
            None => (
                1.0,
                ModuleTuning::default().center_mv,
                ModuleTuning::default().press_ms,
                ModuleTuning::default().release_ms,
            ),
        };
        ModuleTuning {
            enabled: stored.map_or(module != "clutch", |s| s.enabled),
            deadzone: (tuning.deadzone / 100.0) as f32,
            curve_strength: (tuning.curve - 1.0).clamp(-1.0, 1.0) as f32,
            smoothing: (tuning.smoothing / 100.0) as f32,
            min_mv: tuning.min_mv.clamp(0.0, 3300.0) as u16,
            max_mv: tuning.max_mv.clamp(0.0, 3300.0) as u16,
            center_mv,
            invert,
            scale,
            press_ms,
            release_ms,
            bite_point: tuning.bite_point.unwrap_or(0.0) as f32,
            gate_mode: match tuning.shifter_gate {
                Some(ShifterGate::HPattern) => ShifterGateMode::HPattern,
                _ => ShifterGateMode::Sequential,
            },
            // --- Encoder per-module tuning ---
            pulse_ms: tuning.encoder_pulse_ms.unwrap_or(50.0) as f32,
            counts_per_click: tuning.encoder_counts_per_click.unwrap_or(4),
        }
    }

    /// The profile the live plan is built from: the draft when one
    /// exists (unsaved edits included), else the stored active
    /// profile. [`None`] only when no profile is active at all.
    fn current_profile(&self) -> Option<WheelProfile> {
        if let Some(draft) = lock(&self.draft).as_ref() {
            return Some(draft.profile.clone());
        }
        lock(&self.store)
            .active()
            .map(|active| active.profile.clone())
    }

    /// Pushes the current plan to the running engine (latest-wins
    /// `watch` send). A no-op while the engine is not running — the
    /// next engine start pushes the stored profile's plan.
    pub fn push_pipeline_plan(&self) {
        let Some(tx) = lock(&self.plan_tx).as_ref().cloned() else {
            return;
        };
        let _ = tx.send(self.build_pipeline_plan());
    }

    /// Arms learn mode (L4): the next physical wheel-button press
    /// (1–32) is captured for the binding editor. Any previous
    /// unconsumed pick is cleared.
    pub fn start_learn(&self) {
        *lock(&self.learn_armed) = true;
        *lock(&self.learned_button) = None;
    }

    /// Cancels learn mode without picking a button.
    pub fn cancel_learn(&self) {
        *lock(&self.learn_armed) = false;
    }

    /// Whether learn mode is armed.
    #[must_use]
    pub fn is_learning(&self) -> bool {
        *lock(&self.learn_armed)
    }

    /// Takes the button learn mode picked ([`None`] when nothing was
    /// picked yet). Consuming: a second read returns [`None`] until
    /// the next pick.
    #[must_use]
    pub fn take_learned_button(&self) -> Option<u8> {
        lock(&self.learned_button).take()
    }

    /// The active profile's captured calibration. Errors honestly when
    /// there is no active profile or no calibration was captured yet —
    /// an unset calibration is never reported as zeros.
    ///
    /// # Errors
    ///
    /// Returns a message when no active profile exists or no steering
    /// calibration was captured.
    pub fn get_calibration(&self) -> Result<Calibration, String> {
        let store = lock(&self.store);
        let active = store
            .active()
            .ok_or_else(|| "no active profile".to_string())?;
        let profile = &active.profile;
        let steering = profile
            .steering_calibration
            .as_ref()
            .filter(|calibration| calibration.is_set)
            .ok_or_else(|| "no steering calibration captured yet".to_string())?;
        let mut pedal_min_mv = BTreeMap::new();
        let mut pedal_max_mv = BTreeMap::new();
        for (key, calibration) in [
            ("accel", &profile.accel_calibration),
            ("brake", &profile.brake_calibration),
            ("clutch", &profile.clutch_calibration),
        ] {
            if let Some(calibration) = calibration.as_ref().filter(|c| c.is_set) {
                pedal_min_mv.insert(key.to_string(), f64::from(calibration.raw_min));
                pedal_max_mv.insert(key.to_string(), f64::from(calibration.raw_max));
            }
        }
        Ok(Calibration {
            steering_center_mv: f64::from(steering.raw_center),
            pedal_min_mv,
            pedal_max_mv,
            calibrated_at: profile.calibrated_at_utc.clone(),
        })
    }

    /// Saves a captured calibration into the active profile. Steering
    /// keeps its existing min/max when present (the frontend shape
    /// carries only the centre); with no steering calibration yet, the
    /// centre is stored with min = max = centre, which the profile's
    /// own sanity rules flag as needing recalibration — a centre-only
    /// capture is incomplete, and the store says so instead of hiding it.
    ///
    /// # Errors
    ///
    /// Returns a message when there is no active profile, a value is
    /// out of range, a pedal key is unknown, or the save fails.
    pub fn save_calibration(&self, calibration: Calibration) -> Result<Calibration, String> {
        if !(0.0..=3300.0).contains(&calibration.steering_center_mv) {
            return Err(format!(
                "steering_center_mv {} is outside 0..=3300 mV",
                calibration.steering_center_mv
            ));
        }
        let mut pedals: Vec<(&str, i32, i32)> = Vec::new();
        for (key, min, max) in Self::pedal_entries(&calibration)? {
            pedals.push((key, min, max));
        }
        let mut store = lock(&self.store);
        let active = store
            .active()
            .ok_or_else(|| "no active profile".to_string())?;
        let mut profile = active.profile.clone();
        let id = active.id.clone();
        let center = mv_to_i32(calibration.steering_center_mv);
        // L4: every saved calibration stamps when it was captured
        // (RFC 3339) — the Overview "Pedals calibrated" line reads
        // this, never a hardcoded date.
        profile.calibrated_at_utc = format_system_time_rfc3339(SystemTime::now());
        profile.steering_calibration = Some(match profile.steering_calibration {
            Some(existing) => SteeringCalibration {
                raw_min: existing.raw_min,
                raw_center: center,
                raw_max: existing.raw_max,
                is_set: true,
                is_millivolts: true,
            },
            None => SteeringCalibration {
                raw_min: center,
                raw_center: center,
                raw_max: center,
                is_set: true,
                is_millivolts: true,
            },
        });
        for (key, min, max) in pedals {
            let slot = match key {
                "accel" => &mut profile.accel_calibration,
                "brake" => &mut profile.brake_calibration,
                _ => &mut profile.clutch_calibration,
            };
            *slot = Some(PedalCalibration {
                raw_min: min,
                raw_max: max,
                is_set: true,
                is_millivolts: true,
            });
        }
        store
            .update(&id, profile)
            .map_err(|error| error.to_string())?;
        store.save().map_err(|error| error.to_string())?;
        Ok(calibration)
    }

    /// Validates + normalizes the pedal entries of a calibration
    /// (`"accelerator"` is accepted as `"accel"`). Returns the
    /// normalized `(key, min_mv, max_mv)` triples.
    fn pedal_entries(calibration: &Calibration) -> Result<Vec<(&'static str, i32, i32)>, String> {
        let mut out: Vec<(&'static str, i32, i32)> = Vec::new();
        for (raw_key, min, max) in calibration
            .pedal_min_mv
            .iter()
            .map(|(key, min)| (key, min, calibration.pedal_max_mv.get(key)))
        {
            let key = match raw_key.to_ascii_lowercase().as_str() {
                "accel" | "accelerator" => "accel",
                "brake" => "brake",
                "clutch" => "clutch",
                other => return Err(format!("unknown pedal '{other}' in calibration")),
            };
            let Some(max) = max else {
                return Err(format!("pedal '{key}' has a minimum but no maximum"));
            };
            if !(0.0..=3300.0).contains(min) || !(0.0..=3300.0).contains(max) {
                return Err(format!(
                    "pedal '{key}' range {min}..={max} mV is outside 0..=3300 mV"
                ));
            }
            if min > max {
                return Err(format!(
                    "pedal '{key}' minimum {min} mV is above its maximum {max} mV"
                ));
            }
            out.push((key, mv_to_i32(*min), mv_to_i32(*max)));
        }
        Ok(out)
    }
}

/// Capacity of the engine supervisor's command channel (connect /
/// disconnect commands are rare; a full channel is an error, never a
/// block — see [`AppStateCore::connect_transport`]).
/// Retained connection-log tail (R2-23/R2-42):
/// the IPC shape carries at most this many recent lines, so a
/// ~1 Hz slow poll never clones + serialises the full ≤200-line
/// buffer.
pub const CONNECTION_LOG_TAIL_LINES: usize = 20;

const ENGINE_COMMAND_CAPACITY: usize = 16;

/// Bounded engine stop: [`AppStateCore::stop_engine`] waits at most
/// this long for the engine's final report (the engine itself returns
/// within one 5 ms tick of the stop signal).
const ENGINE_STOP_TIMEOUT: Duration = Duration::from_secs(5);

/// Steering full lock in degrees: the pipeline's steering is `-1..=1`
/// and the snapshot's steering is `-450..=450` (900° lock-to-lock).
const STEERING_FULL_LOCK_DEG: f64 = 450.0;

/// Capacity of the engine's frame channel (200 Hz; the engine drains
/// the latest frame per tick, so a full channel drops the newest
/// frame instead of blocking the transport).
const ENGINE_FRAME_CHANNEL_CAPACITY: usize = 256;

/// Commands the state sends the engine supervisor thread.
#[derive(Clone, Copy, Debug)]
enum EngineCommand {
    /// Connect a transport and feed the engine from it.
    Connect(TransportKind),
    /// Disconnect the live transport; the engine keeps ticking idle.
    Disconnect,
}

/// Cumulative encoder counts from the latest frame, as reported by
/// the frame feeder (see
/// [`AppStateCore::note_frame_encoders`]). The sink reads these when
/// it feeds [`AppStateCore::ingest_frame`]; the snapshot tracker
/// diffs them into deltas itself.
#[derive(Clone, Copy, Debug, Default)]
struct EngineEncoderAccum {
    /// Cumulative counts from the latest frame, one per encoder.
    counts: [i16; MAX_ENCODERS],
    /// How many encoders the latest frame carries (`0..=4`).
    n_enc: usize,
}

/// Latest raw axis readings in mV, as reported by the frame feeder
/// (see [`AppStateCore::note_raw_axes`]). v2.0.2 capture plumbing:
/// the engine consumes the frame's `axes_mv` inside the pipeline and
/// the state used to only ever see the processed output — so a
/// centre / pedal endpoint could never be captured from live data.
/// The feeder now also reports the raw axes per frame (same pattern
/// as [`EngineEncoderAccum`]), and the capture methods sample this
/// slot. A sample older than [`RAW_MV_FRESH_FOR`] is refused by
/// capture — a stale reading must never be saved as a calibration.
#[derive(Clone, Copy, Debug, Default)]
struct RawAxesSample {
    /// Raw axis values in mV by slot 0–7 (`None` = slot not present).
    axes: [Option<u16>; MAX_AXES],
    /// When the sample arrived (`None` = no frame ever seen).
    at: Option<Instant>,
}

/// A raw-mV sample counts as live for this long after the frame that
/// carried it arrived. Frames flow at 200 Hz while the wheel is
/// connected, so 2 s is generous against jitter and still refuses a
/// reading from a wheel that dropped away.
const RAW_MV_FRESH_FOR: Duration = Duration::from_secs(2);

/// Raw axis slot for the steering centre capture (protocol §1.3).
const SLOT_STEERING: usize = 0;

/// The engine's [`PacketSource`](rsh_engine::engine::PacketSource):
/// a tokio `mpsc` channel the transports feed. Awaiting the channel
/// is cancellation-safe, so the engine's zero-timeout polls never lose
/// a frame; [`None`] (every sender dropped) means the supervisor is
/// gone and the engine parks until stopped.
struct ChannelPacketSource {
    /// Frame channel receiver.
    rx: tokio::sync::mpsc::Receiver<StreamFrame>,
}

impl PacketSource for ChannelPacketSource {
    /// Await the next frame from the channel.
    fn next_frame(&mut self) -> impl std::future::Future<Output = Option<StreamFrame>> + Send {
        self.rx.recv()
    }
}

/// The engine's [`OutputSink`](rsh_engine::engine::OutputSink): the
/// app state itself. Every tick feeds the snapshot (via
/// [`AppStateCore::ingest_frame`]) and the installed backend's
/// `update`. The sink holds the state by [`Arc`] — the engine task
/// owns it until it stops, and the state drops its end of the cycle
/// in [`AppStateCore::stop_engine`].
struct EngineSink {
    /// The state this sink feeds.
    state: Arc<AppStateCore>,
}

impl OutputSink for EngineSink {
    /// Send one processed tick: snapshot feed first (never fails the
    /// send — a snapshot is bookkeeping), then the backend send of
    /// the binding-remapped tick.
    fn send(&mut self, state: &PipelineOutputState) -> Result<(), SinkError> {
        // The plain-`send` path (the engine's final neutral in
        // `finish()`) has no live metrics of its own — it reuses the
        // last measured ones instead of resetting them to "unmeasured".
        let metrics = self.state.live_metrics();
        let remapped = self.state.ingest_engine_output(state, &metrics);
        self.state.send_engine_output(&remapped)
    }

    /// Send one processed tick with the engine's live per-tick
    /// metrics (H1 condition 11): the snapshot carries real jitter /
    /// latency / loss p99s instead of "unmeasured".
    fn send_with_metrics(
        &mut self,
        state: &PipelineOutputState,
        metrics: &EngineMetrics,
    ) -> Result<(), SinkError> {
        let remapped = self.state.ingest_engine_output(state, metrics);
        self.state.send_engine_output(&remapped)
    }
}

/// Pure half of [`migrate_persisted_backend_default`]: migrate only
/// when the persisted choice is the old default (`HIDMaestro`) AND
/// vJoy is actually installed — a persisted `HIDMaestro` with no vJoy
/// driver stays put (the setup wizard offers the vJoy install as the
/// fallback instead), and any other persisted choice is never touched.
/// There is no way to tell a v2.0.0 default apart from an explicit
/// user `HIDMaestro` choice — the store records only the value — so
/// this migration knowingly also moves an explicit `HIDMaestro`
/// choice when vJoy is installed; the user can re-pick `HIDMaestro` on
/// the Output page, and the migration note names the change.
#[must_use]
pub fn should_migrate_persisted_backend(persisted_backend: &str, vjoy_installed: bool) -> bool {
    persisted_backend == "HIDMaestro" && vjoy_installed
}

/// One-time v2.0.0 → v2.0.1 migration (BACKEND-2): v2.0.0 persisted
/// `HIDMaestro` as the default on every install, so every existing
/// store carries it. When vJoy is installed, the persisted choice is
/// switched to `VJoy` and saved, and the human-readable note is
/// returned for the startup log; otherwise nothing changes and
/// [`None`] is returned. A save failure reverts the in-memory
/// settings to `HIDMaestro` (disk and memory must agree) and returns
/// [`None`] — the next launch retries the migration.
pub fn migrate_persisted_backend_default(store: &mut ProfileStore) -> Option<String> {
    if !should_migrate_persisted_backend(
        &store.settings().output_backend,
        resolve_dll_path().is_some(),
    ) {
        return None;
    }
    let mut settings = store.settings().clone();
    settings.output_backend = "VJoy".to_string();
    store.set_settings(settings);
    if let Ok(()) = store.save() {
        Some(
            "migrated persisted HIDMaestro default to VJoy (vJoy driver found; re-pick HIDMaestro on the Output page to go back)"
                .to_string(),
        )
    } else {
        let mut settings = store.settings().clone();
        settings.output_backend = "HIDMaestro".to_string();
        store.set_settings(settings);
        None
    }
}

/// Background vJoy install worker (v2.0.1 hotfix BACKEND-2): download
/// the pinned installer with the inbox `curl.exe` (Windows 10+),
/// verify size + SHA-256 before executing anything, run it silent
/// ([`VJOY_INSTALLER_ARGS`]), then re-probe the DLL. Runs on its own
/// thread; it only ever touches [`AppStateCore::vjoy_install`], and
/// always clears `installing` — success or failure — so a stuck
/// "installing" can never wedge the status. The installer exe's own
/// manifest triggers the one UAC prompt; the download itself runs
/// without a console window (`CREATE_NO_WINDOW`).
fn run_vjoy_install(state: &Arc<AppStateCore>) {
    // DS-40: the `installing` flag must clear even if this thread
    // panics — the old code only cleared it on the normal return
    // path, so a panicked install latched "already in progress"
    // forever. The guard resets on drop (normal return, error
    // return, or unwind); the explicit reset below stays as the
    // documented normal-path write.
    struct ResetInstalling<'a>(&'a Arc<AppStateCore>);
    impl Drop for ResetInstalling<'_> {
        fn drop(&mut self) {
            let mut install = lock(&self.0.vjoy_install);
            install.installing = false;
        }
    }
    let _guard = ResetInstalling(state);
    let result = run_vjoy_install_inner();
    // R2-20: the install may have placed (or failed to place) the
    // DLL — the cached probe must not keep answering the old result.
    state.invalidate_vjoy_dll_cache();
    let mut install = lock(&state.vjoy_install);
    install.installing = false;
    install.error = result.err();
}

/// Wait for a spawned child with a hard timeout (DS-40): poll
/// `try_wait()` every 100 ms; on timeout kill the child and report.
/// The vJoy installer used to be awaited with a blocking `.status()`
/// that waited forever. Platform-free (std only) so the Linux suite
/// can cover the timeout itself with `/bin/sleep`.
fn wait_child_timeout(
    mut child: std::process::Child,
    timeout: Duration,
    what: &'static str,
) -> Result<std::process::ExitStatus, String> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "{what} timed out after {} s — killed",
                        timeout.as_secs()
                    ));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(format!("could not wait for {what}: {error}")),
        }
    }
}

/// The fallible half of [`run_vjoy_install`]: `Ok(())` only when the
/// DLL resolves after the install; every failure path returns the
/// message the status reports.
fn run_vjoy_install_inner() -> Result<(), String> {
    let dir = std::env::temp_dir().join("RSH");
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("cannot create the vJoy download folder: {error}"))?;
    let installer = dir.join(VJOY_INSTALLER_FILE_NAME);
    // Windows 10+ ships curl.exe — no new HTTP dependency. The
    // download runs windowless; `--fail` makes HTTP errors fail the
    // command instead of writing an error page to the file.
    // DS-40: `--max-time 120 --connect-timeout 15` — the download
    // used to have no timeout at all, so a stalled network left
    // the install thread (and the `installing` flag) stuck forever.
    let mut download = Command::new("curl.exe");
    download.args([
        "-L",
        "--fail",
        "--max-time",
        "120",
        "--connect-timeout",
        "15",
        "-o",
        &installer.to_string_lossy(),
        VJOY_INSTALLER_URL,
    ]);
    #[cfg(windows)]
    download.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let status = download
        .status()
        .map_err(|error| format!("could not run curl.exe to download vJoy: {error}"))?;
    if !status.success() {
        return Err(format!(
            "vJoy download failed (curl exit {}); check the connection and retry",
            status.code().unwrap_or(-1)
        ));
    }
    let bytes = std::fs::read(&installer)
        .map_err(|error| format!("downloaded vJoy installer could not be read: {error}"))?;
    let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if size != VJOY_INSTALLER_SIZE_BYTES {
        let _ = std::fs::remove_file(&installer);
        return Err(format!(
            "downloaded vJoy installer has the wrong size ({size} bytes, expected {VJOY_INSTALLER_SIZE_BYTES}) — deleted, not run"
        ));
    }
    let digest = Sha256::digest(&bytes);
    let hex = digest
        .iter()
        .fold(String::with_capacity(64), |mut acc, byte| {
            let _ = write!(acc, "{byte:02x}");
            acc
        });
    if hex != VJOY_INSTALLER_SHA256_HEX {
        let _ = std::fs::remove_file(&installer);
        return Err(
            "downloaded vJoy installer failed SHA-256 verification — deleted, not run (download source unverified)"
                .to_string(),
        );
    }
    // Silent install. The exe's manifest elevates (one UAC prompt —
    // expected); `/NORESTART` never reboots unattended.
    // DS-40: the installer used to be awaited with a blocking
    // `.status()` (forever, with inherited stdio) — now it is
    // spawned detached with nulled stdio (the same bug class as the
    // NSIS `Exec` hang and the daemon/UI spawns) and bounded to
    // 120 s, killed on timeout.
    let mut installer_cmd = Command::new(&installer);
    installer_cmd
        .args(VJOY_INSTALLER_ARGS)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    installer_cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let child = installer_cmd
        .spawn()
        .map_err(|error| format!("could not run the vJoy installer: {error}"))?;
    let status = wait_child_timeout(child, Duration::from_secs(120), "vJoy installer")?;
    if !status.success() {
        return Err(format!(
            "vJoy installer exited with code {}",
            status.code().unwrap_or(-1)
        ));
    }
    if resolve_dll_path().is_some() {
        Ok(())
    } else {
        Err(
            "vJoy installer finished but the driver DLL was not found — a restart may be required"
                .to_string(),
        )
    }
}

/// Builds the real backend factories for the selector: vJoy
/// (default), `HIDMaestro` (explicit choice, this settings' device
/// id), and the no-device sink.
fn real_backend_factories(vjoy_device_id: u32) -> BackendFactories {
    BackendFactories {
        hidmaestro: Some(Box::new(|| {
            Box::new(HidMaestroOutput::new()) as Box<dyn IJoystickOutput>
        })),
        vjoy: Some(Box::new(move || {
            Box::new(VJoyOutput::new(vjoy_device_id)) as Box<dyn IJoystickOutput>
        })),
        none: Box::new(|| Box::new(NullSink) as Box<dyn IJoystickOutput>),
    }
}

/// The supervisor thread's entry point: builds the *network*
/// current-thread tokio runtime (timers + forwarder/activator
/// tasks — the transports use std sockets + std reader threads, no
/// tokio net) and blocks on the supervisor loop.
///
/// A-02 (Phase 2): the 200 Hz engine does NOT run on this runtime.
/// [`engine_supervisor_run`] spawns it on its own std thread
/// (`rsh-engine-200hz`) with its own current-thread runtime, so a
/// slow network wait (HELLO timeout, backoff sleep, control op)
/// can never stall the engine tick, and a busy engine tick can
/// never starve the activator. The two runtimes communicate only
/// through channels (frames in, report out, stop via the watch
/// handle). A runtime build failure is recorded on the state; the
/// app keeps running without an engine instead of crashing at
/// startup.
fn engine_supervisor_main(
    state: Arc<AppStateCore>,
    cmd_rx: tokio::sync::mpsc::Receiver<EngineCommand>,
    report_tx: StdSender<EngineReport>,
    ready_tx: StdSender<()>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            state.note_engine_failure(format!("could not build engine runtime: {error}"));
            return;
        }
    };
    runtime.block_on(engine_supervisor_run(state, cmd_rx, report_tx, ready_tx));
}

/// The supervisor loop: runs the engine task, applies transport
/// connect/disconnect commands to the forwarder, and delivers the
/// engine's final report to the state when the engine stops. The loop
/// exits only when the engine task ends (stop signalled through its
/// [`StopHandle`]); a closed command channel alone never ends it —
/// the engine keeps ticking idle until stopped.
async fn engine_supervisor_run(
    state: Arc<AppStateCore>,
    mut cmd_rx: tokio::sync::mpsc::Receiver<EngineCommand>,
    report_tx: StdSender<EngineReport>,
    ready_tx: StdSender<()>,
) {
    let (frame_tx, frame_rx) = tokio::sync::mpsc::channel(ENGINE_FRAME_CHANNEL_CAPACITY);
    *lock(&state.engine_frame_tx) = Some(frame_tx.clone());
    let sink = EngineSink {
        state: Arc::clone(&state),
    };
    // L4 pipeline push: the watch channel carries the latest plan
    // (initial value = the stored profile's plan, not the default —
    // the engine must not run one tick on defaults when a profile
    // already says otherwise). The engine applies it at the next
    // tick; later `set_tuning` calls send through the installed
    // sender.
    let (plan_tx, plan_rx) = tokio::sync::watch::channel(state.build_pipeline_plan());
    let engine = WheelEngine::new(
        ProcessingPipeline::new(state.build_pipeline_plan()),
        ChannelPacketSource { rx: frame_rx },
        Box::new(sink),
    )
    .with_plan_receiver(plan_rx);
    state.set_plan_sender(plan_tx);
    // Status pump (H1 condition 12): the forwarder drains the
    // transport's status channel into this sender after every stream
    // frame, so board loss counters and `btnsEx` reach the engine.
    let status_tx = engine.status_sender();
    let stop = engine.stop_handle();
    state.set_stop_handle(stop);
    // A-02: the engine runs on its own std thread with its own
    // current-thread runtime — never on this supervisor runtime.
    // The thread reports its final EngineReport on `report_tx`
    // (the std channel `stop_engine` reads) and then signals
    // `engine_done`; the supervisor loop below only watches the
    // signal. The thread is detached by design: it ends exactly
    // when `engine.run()` returns (stop signalled through the watch
    // handle), which always precedes the done signal.
    let (engine_done_tx, mut engine_done_rx) = tokio::sync::oneshot::channel::<()>();
    let report_tx_for_engine = report_tx.clone();
    let engine_thread_result = std::thread::Builder::new()
        .name("rsh-engine-200hz".to_string())
        .spawn(move || {
            let Ok(engine_runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                let _ = report_tx_for_engine.send(zero_engine_report());
                let _ = engine_done_tx.send(());
                return;
            };
            let report = engine_runtime.block_on(engine.run());
            let _ = report_tx_for_engine.send(report);
            let _ = engine_done_tx.send(());
        });
    if engine_thread_result.is_err() {
        state.note_engine_failure("could not spawn engine thread (rsh-engine-200hz)".to_string());
        let _ = report_tx.send(zero_engine_report());
        let _ = ready_tx.send(());
        return;
    }
    // The engine thread is running: tell `start_engine` the
    // supervisor is up (the frame channel is live from here on).
    let _ = ready_tx.send(());
    let mut forwarder: Option<tokio::task::JoinHandle<()>> = None;
    // Assigned on the only path that leaves the loop (the engine
    // task ending), so no initializer is needed or wanted.
    let report: Option<EngineReport>;
    // The command branch is disabled (pending forever) once the
    // sender is gone, so a closed channel cannot spin the select.
    let mut commands_open = true;
    loop {
        tokio::select! {
            cmd = async {
                if commands_open {
                    cmd_rx.recv().await
                } else {
                    std::future::pending().await
                }
            } => {
                match cmd {
                    Some(EngineCommand::Connect(kind)) => {
                        if let Some(running) = forwarder.take() {
                            running.abort();
                        }
                        state.set_connection(ConnectionState::Connecting);
                        state.note_engine_detail(format!(
                            "transport {} connecting — engine running",
                            kind.as_str()
                        ));
                        forwarder = Some(spawn_forwarder(
                            Arc::clone(&state),
                            kind,
                            frame_tx.clone(),
                            status_tx.clone(),
                        ));
                    }
                    Some(EngineCommand::Disconnect) => {
                        if let Some(running) = forwarder.take() {
                            running.abort();
                        }
                        // A disconnect ends the session: the gate
                        // closes, so a later reconnect must
                        // re-authenticate (SEC-E-14).
                        state.close_session_gate();
                        state.set_connection(ConnectionState::Disconnected);
                        state.note_engine_detail(
                            "transport disconnected — engine idle (watchdog neutral)"
                                .to_string(),
                        );
                    }
                    None => {
                        commands_open = false;
                    }
                }
            }
            // A-02: the engine lives on its own thread; the done
            // signal fires after its report was sent on the std
            // channel (read by `stop_engine`). A dropped sender
            // means the engine thread died before reporting — the
            // report channel then yields nothing and the stop path
            // reports the timeout honestly.
            _ = &mut engine_done_rx => {
                report = None;
                break;
            }
        }
    }
    if let Some(running) = forwarder.take() {
        running.abort();
    }
    *lock(&state.engine_frame_tx) = None;
    state.set_connection(ConnectionState::Disconnected);
    // The engine thread already sent its report on `report_tx`
    // before signalling done (or died trying — `report` is `None`
    // then, and `stop_engine`'s 5 s read reports the timeout).
    // `report_tx` was moved into the engine thread; nothing left to
    // send here.
    let _ = report;
}

/// A zeroed [`EngineReport`] for the join-error path: the engine task
/// ended abnormally, so there are no real counters — but the state
/// still gets a report (all zeros) instead of a stop that never
/// returns.
fn zero_engine_report() -> EngineReport {
    EngineReport {
        counters: rsh_engine::engine::EngineCounters::default(),
        metrics: EngineMetrics::new(),
        sequence_accepted: 0,
        sequence_gaps: 0,
        sequence_duplicates: 0,
        sequence_rebaselines: 0,
        ping_timeouts: 0,
        ping_last_delay_us: None,
    }
}

/// How long the activator waits for one `HELLO` reply per attempt
/// (C-03, single source: `rsh_api::HELLO_WAIT`): 750 ms, above the
/// firmware's 500 ms `HELLO`-reply rate limiter (F-04), so a reply
/// is never lost to the limiter. The wait ends as soon as a `HELLO`
/// arrives — the timeout only matters when no board answers at all.
/// (Phase 2: the 200 Hz engine now runs on its own thread/runtime —
/// A-02 — so this wait no longer stalls the frame loop the way it
/// would have on the old shared current-thread runtime.)
const SESSION_HELLO_WAIT: Duration = HELLO_WAIT;

/// `HELLO` wait attempts per activation (C-03, single source:
/// `rsh_api::HELLO_ATTEMPTS`): 3.
const SESSION_HELLO_ATTEMPTS: usize = HELLO_ATTEMPTS as usize;

/// A transport that can both stream frames and run a session (H5).
///
/// Both live transports satisfy this: [`UdpTransport`] and
/// [`SerialTransport`] each implement [`PacketSource`] (stream),
/// [`ControlTransport`] (HMAC control channel), and `next_hello`
/// (the `HELLO` reply channel). The activator needs all three on the
/// *same* transport instance — stream + control share one socket /
/// one serial port, demuxed into channels, and a second socket cannot
/// receive control replies (H1 §2) — so the forwarder hands its own
/// transport to the activator and takes it back afterwards.
trait SessionTransport: PacketSource + ControlTransport {
    /// Await the next `HELLO` reply. Returns [`None`] once the
    /// transport is shut down. Cancellation-safe: dropping the
    /// future must not lose a `HELLO` (both transports back this
    /// with a tokio `mpsc` receive, which is cancellation-safe), so
    /// the activator can time-box each wait and retry.
    fn recv_hello(&mut self) -> impl Future<Output = Option<HelloReply>> + Send;

    /// Send one HELO requesting a session (protocol §2.1).
    ///
    /// `session_flag 1` asks for a **new** session (fresh nonce);
    /// `0` resumes (old nonce back). The activator always asks for a
    /// new session: D30 forbids reusing an old nonce/counter after a
    /// session ends.
    ///
    /// # Errors
    ///
    /// Returns a message when the HELO cannot be sent. Callers treat
    /// this as non-fatal (the transport's own HELO path may still
    /// deliver a `HELLO`; a dead link surfaces as a `HELLO` timeout).
    fn request_hello(&mut self, session_flag: u8) -> Result<(), String>;

    /// Non-blocking drain of one queued `HELLO` reply (R2-07): the
    /// frame loop calls this after every stream frame so HELLOs
    /// arriving after activation still refresh the observed window
    /// state. Default: no HELLO available (transports without a
    /// HELLO channel keep it).
    fn try_next_hello(&mut self) -> Option<HelloReply> {
        None
    }

    /// Short transport label for connection log lines (`"wifi"` /
    /// `"usb"` — Phase 0 gate §6.2 format 1/2).
    fn transport_label(&self) -> &'static str {
        "unknown"
    }
}

impl SessionTransport for UdpTransport {
    fn recv_hello(&mut self) -> impl Future<Output = Option<HelloReply>> + Send {
        self.next_hello()
    }

    fn try_next_hello(&mut self) -> Option<HelloReply> {
        self.try_next_hello()
    }

    fn transport_label(&self) -> &'static str {
        "wifi"
    }

    fn request_hello(&mut self, session_flag: u8) -> Result<(), String> {
        // One-shot HELO: the 1 Hz HELO thread always sends flag 0
        // (resume), so a new session needs this explicit flag-1 HELO.
        self.send_helo(session_flag)
            .map_err(|error| error.to_string())
    }
}

impl SessionTransport for SerialTransport {
    fn recv_hello(&mut self) -> impl Future<Output = Option<HelloReply>> + Send {
        self.next_hello()
    }

    fn try_next_hello(&mut self) -> Option<HelloReply> {
        self.try_next_hello()
    }

    fn transport_label(&self) -> &'static str {
        "usb"
    }

    fn request_hello(&mut self, session_flag: u8) -> Result<(), String> {
        // Serial has no automatic HELO thread: the session layer
        // drives HELO on demand (connect / pair / reconnect).
        self.send_helo(session_flag)
            .map_err(|error| error.to_string())
    }
}

/// Outcome of awaiting a new-session `HELLO` (H5-control).
enum NewSessionHello {
    /// A `HELLO` arrived: an echo-1 reply answering the new-session
    /// HELO when one came, else the most recent echo-0 reply.
    Found(HelloReply),
    /// No `HELLO` arrived within the wait attempts.
    None,
    /// Phase 2 (Wi-Fi gap): no OS route to the board address, so no
    /// HELO was even attempted — the caller reports Unreachable
    /// without the timeout log line (no timeout happened).
    NoRoute,
    /// The transport died while waiting (its `HELLO` channel closed).
    TransportClosed,
}

/// Await a `HELLO` answering a new-session request (H5, shared by the
/// activator and the H5-control helper — the same code path, not a
/// copy).
///
/// Sends a flag-1 HELO first (D30: only a new session gets a fresh
/// nonce; a failed one-shot HELO is not fatal — the transport's own
/// HELO path may still deliver a `HELLO`, and a dead link surfaces as
/// a `HELLO` timeout), then waits up to
/// [`SESSION_HELLO_ATTEMPTS`] × [`SESSION_HELLO_WAIT`], preferring a
/// reply with `session_echo 1` and keeping the most recent echo-0
/// reply as a fallback (a stale echo-0 reply fails loudly instead
/// of silently opening the gate). A stale echo-0 reply triggers one
/// more explicit
/// flag-1 HELO; a timeout re-requests a new session once, then waits
/// again.
/// Whether the OS has a route to the board address (Phase 2,
/// Wi-Fi gap): a UDP `connect()` performs the route lookup without
/// sending a packet. An unparseable address is treated as "no
/// route" (fail-closed — the caller reports Unreachable instead of
/// HELO-ing into the void); a `connect()` that fails for any other
/// reason is also "no route", since no packet could leave the PC
/// either way. Unit-testable without a board: loopback always has
/// a route, an unroutable test-net address does not (on a host
/// without a default route to it — the test pins the unparseable
/// case instead, which is deterministic everywhere).
fn wifi_route_exists(board_ip: &str) -> bool {
    use std::io::ErrorKind;
    use std::net::{IpAddr, SocketAddr, UdpSocket};
    let Ok(ip) = board_ip.parse::<IpAddr>() else {
        return false;
    };
    let Ok(socket) = UdpSocket::bind("0.0.0.0:0") else {
        // Cannot even open a socket: fail open — the HELO path
        // reports the real failure instead of a wrong "no route".
        return true;
    };
    match socket.connect(SocketAddr::new(ip, HELO_PORT)) {
        // No route to the board (Windows: WSAENETUNREACH 10051 —
        // the user's 34× `HELO send failed` case): skip HELO.
        Err(e)
            if matches!(
                e.kind(),
                ErrorKind::NetworkUnreachable | ErrorKind::HostUnreachable
            ) =>
        {
            false
        }
        // Anything else — `Ok(())`, or an error such as EPERM /
        // EACCES from a sandbox or firewall policy, where a route
        // can exist behind the block — fails OPEN: the normal
        // HELO/timeout path runs and reports the real failure
        // ("HELO send failed" / "HELLO timeout") instead of a
        // wrong "no route".
        _ => true,
    }
}

async fn await_new_session_hello<T: SessionTransport>(
    state: &Arc<AppStateCore>,
    transport: &mut T,
) -> NewSessionHello {
    // Phase 2 (Wi-Fi gap): when the transport is Wi-Fi and there is
    // no route to the board address (the PC is not on the RSH Wi-Fi
    // — the log's 34× `HELO send failed … os error 10051` case),
    // skip the HELO attempts entirely instead of sending into the
    // void 3× and waiting out every timeout. A UDP `connect()` does
    // a route lookup without sending a packet, so this is a pure
    // local check; a route that exists (board off, AP up) still
    // takes the normal HELO/timeout path. A policy block (EPERM)
    // is NOT "no route" — `wifi_route_exists` fails open there.
    // Serial is untouched, and the fake test transport (label
    // "unknown") never takes this branch.
    if transport.transport_label() == "wifi" && !wifi_route_exists(&state.settings().esp_ip) {
        state.push_connection_log(
            "WARN",
            &format!(
                "no route to board {} — Wi-Fi not joined to the RSH network (or USB not plugged in); skipping HELO",
                state.settings().esp_ip
            ),
        );
        return NewSessionHello::NoRoute;
    }
    // Phase 0 gate §6.2 format 2 (HELO sent — one line per send).
    let send_helo = |state: &Arc<AppStateCore>, transport: &mut T| {
        match transport.request_hello(1) {
            Ok(()) => {
                state.push_connection_log(
                    "INFO",
                    &format!(
                        "HELO sent: session_flag=1 transport={} (fresh nonce requested)",
                        transport.transport_label()
                    ),
                );
            }
            Err(error) => {
                // The send itself failed (e.g. sandbox blocks UDP
                // `sendto`): log the attempt, not a success — the
                // timeout below will still fire and fail closed.
                state.push_connection_log(
                    "WARN",
                    &format!(
                        "HELO send failed: transport={} error={error}",
                        transport.transport_label()
                    ),
                );
            }
        }
    };
    send_helo(state, transport);
    let mut fallback: Option<HelloReply> = None;
    for _ in 0..SESSION_HELLO_ATTEMPTS {
        match tokio::time::timeout(SESSION_HELLO_WAIT, transport.recv_hello()).await {
            Ok(Some(reply)) => {
                if reply.session_echo == 1 {
                    return NewSessionHello::Found(reply);
                }
                fallback = Some(reply);
                // The board answered a routine flag-0 HELO with the
                // old nonce: ask for a new session explicitly, then
                // keep waiting for its answer.
                send_helo(state, transport);
            }
            Ok(None) => {
                // The transport closed while awaiting HELLO.
                return NewSessionHello::TransportClosed;
            }
            Err(_) => {
                // No HELLO in this window: re-request a new session
                // once, then wait again.
                send_helo(state, transport);
            }
        }
    }
    match fallback {
        Some(hello) => NewSessionHello::Found(hello),
        None => NewSessionHello::None,
    }
}

/// Open a session from a `HELLO` (H5, shared by the activator and the
/// H5-control helper): `BoardSession::pair` for an unpaired board
/// with the window open, `resume` otherwise — either way a signed
/// `GET_CAPS` goes first and only its success activates the session.
/// The device key + Wi-Fi password are stored in the OS credential
/// store by `pair`; the in-memory key is zeroized by
/// [`BoardSession::close`], which the caller runs.
///
/// Returns the session (live or not) plus the `GET_CAPS` outcome, so
/// the caller keeps the session on failure and can still close it
/// cleanly instead of dropping a half-open one.
/// Open a session for one observed `HELLO` (no-pairing
/// redesign, protocol v2.2): a single unsigned `connect(hello)`
/// — an unsigned `GET_CAPS` first, no key, no nonce, no window,
/// no replacement branch. The `BoardSession::connect` shape is
/// WS-2's contract (plan §4: `resume()` → `connect()`).
async fn open_session_for_hello<T, K>(
    transport: T,
    keystore: K,
    hello: &HelloReply,
) -> (BoardSession<K, T>, Result<GetCapsReply, SessionError>)
where
    T: SessionTransport,
    K: KeyStoreLike,
{
    let board_id = hello
        .board_mac
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<String>>()
        .join(":");
    let mut session = BoardSession::new(&board_id, keystore, transport);
    let outcome = session.connect(hello).await;
    (session, outcome)
}

/// One session-activation attempt on `transport` (H5, SEC-E-14).
///
/// The SEC-E-14 flow: transport up → unsigned `GET_CAPS` first →
/// `open_session_gate()` → frames flow to the engine/output.
///
/// 1. Awaits a new-session `HELLO` via [`await_new_session_hello`]
///    (flag-1 HELO — a new session).
/// 2. Runs [`open_session_for_hello`] (single unsigned
///    `connect(hello)` — a `GET_CAPS` goes first and only its
///    success activates the session).
/// 3. On success records the activation and opens the session gate;
///    on any failure records the reason and leaves the gate exactly
///    as it was (closed in production) — fail-closed, never silently
///    opened.
/// 4. Hands the transport back via `BoardSession::take_transport`
///    (the session itself does not survive: a later control
///    operation starts a fresh session), so the forwarder can
///    stream frames on the same transport.
///
/// Returns [`None`] only when the transport died mid-activation (its
/// `HELLO` channel closed): the caller treats that as transport loss
/// and rebuilds. A failed activation (no `HELLO`, unavailable
/// credential store) returns [`Some`] — the transport is alive,
/// the gate stays closed, and the forwarder falls through to the
/// frame loop, where frames are held and counted until a later
/// activation succeeds.
/// Guard that marks a forwarder activation in flight (R2-28) and
/// clears the flag on drop, so every exit path of
/// `activate_session_on` — including the early `None` returns —
/// clears it. While armed, `AppStateCore::connect_session` no-ops
/// instead of aborting the forwarder mid-activation.
struct ActivationInFlightGuard<'a> {
    flag: &'a AtomicBool,
}

impl<'a> ActivationInFlightGuard<'a> {
    fn arm(flag: &'a AtomicBool) -> Self {
        flag.store(true, Ordering::SeqCst);
        Self { flag }
    }
}

impl Drop for ActivationInFlightGuard<'_> {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::SeqCst);
    }
}

/// Guard that marks one forwarder retry wait as armed (R3-D-02) and
/// unmarks it on drop, so every exit path — including a task abort
/// — clears it. The wait is a count, not a bool: the frame loop's
/// retry deadline and `forwarder_retry_after_failure`'s backoff
/// sleep nest, and a bool cleared by the inner guard would lie
/// while the outer wait still holds. The drop saturates at 0, so a
/// `disconnect_transport` reset racing a dropping guard can never
/// underflow the count.
struct ForwarderRetryWaitGuard<'a> {
    count: &'a AtomicU32,
}

impl<'a> ForwarderRetryWaitGuard<'a> {
    fn arm(count: &'a AtomicU32) -> Self {
        count.fetch_add(1, Ordering::SeqCst);
        Self { count }
    }
}

impl Drop for ForwarderRetryWaitGuard<'_> {
    // `fetch_update` is deprecated in favour of `try_update`, but
    // `try_update` is stable only since Rust 1.95 and the MSRV
    // floor is 1.90 (clippy.toml) — the old name is a rename, not
    // a removal, so keep it with the lint allowed until the MSRV
    // moves past 1.95.
    #[allow(deprecated)]
    fn drop(&mut self) {
        let _ = self
            .count
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| v.checked_sub(1));
    }
}

async fn activate_session_on<T, K>(
    state: &Arc<AppStateCore>,
    transport: T,
    keystore: K,
) -> Option<T>
where
    T: SessionTransport,
    K: KeyStoreLike,
{
    let _activation_guard = ActivationInFlightGuard::arm(&state.activation_in_flight);
    let mut transport = transport;
    match await_new_session_hello(state, &mut transport).await {
        NewSessionHello::TransportClosed => {
            state.note_session_error_typed(
                ConnectionError::TransportClosed,
                "transport closed while awaiting the board HELLO",
            );
            None
        }
        NewSessionHello::NoRoute => {
            // Phase 2 (Wi-Fi gap): no HELO was sent and no timeout
            // elapsed — the WARN line in `await_new_session_hello`
            // already names the fix; record Unreachable so the UI
            // shows its proactive Wi-Fi/USB banner + notification.
            // D15: no route means no HELLO could have been seen —
            // the reachable latch clears.
            state.clear_board_reachable();
            state.note_session_error_typed(
                ConnectionError::Unreachable,
                &format!(
                    "no HELLO: no route to the board at {} — Wi-Fi not joined to the RSH network (or USB not plugged in)",
                    state.settings().esp_ip
                ),
            );
            Some(transport)
        }
        NewSessionHello::None => {
            // Phase 0 gate §6.2 format 4 (HELLO timeout).
            let total_ms = SESSION_HELLO_WAIT.as_millis() * SESSION_HELLO_ATTEMPTS as u128;
            let address = if transport.transport_label() == "wifi" {
                state.settings().esp_ip
            } else {
                // R2-29: the address here is log-only — read the
                // cached port, never probe inline on the supervisor
                // runtime (a probe is up to 500 ms per candidate).
                state.cached_board_port().unwrap_or_default()
            };
            state.clear_board_reachable();
            state.push_connection_log(
                "WARN",
                &format!(
                    "HELLO timeout after {total_ms}ms (attempt {SESSION_HELLO_ATTEMPTS}/{SESSION_HELLO_ATTEMPTS}): board unreachable at {address} — check Wi-Fi is joined to the RSH network"
                ),
            );
            state.note_session_error_typed(
                ConnectionError::Unreachable,
                &format!(
                    "no HELLO reply from the board within {total_ms} ms — board not responding \
                     on this transport",
                ),
            );
            Some(transport)
        }
        NewSessionHello::Found(hello) => {
            state.note_observed_hello(&hello);
            // M-12: no `std::sync::Mutex` guard is ever held across
            // an await in this module.
            let (mut session, outcome) = open_session_for_hello(transport, keystore, &hello).await;
            match outcome {
                Ok(caps) => {
                    state.record_session_activation(SessionActivation {
                        board_id: session.board_id().to_string(),
                        firmware: caps.firmware,
                        protocol: caps.protocol,
                        caps,
                    });
                    state.open_session_gate();
                }
                Err(error) => {
                    let typed = AppStateCore::classify_session_error(&error);
                    state.note_session_error_typed(
                        typed,
                        &format!("session activation failed: {error}"),
                    );
                    // Fail-closed: the gate stays exactly as it was
                    // (closed in production — never opened on a
                    // failed GET_CAPS).
                }
            }
            session.close();
            session.take_transport()
        }
    }
}

/// Run one control operation under a fresh session (H5-control).
///
/// The helper the 11 remaining commands are built on: it opens a
/// session exactly the way the activator does — [`await_new_session_hello`]
/// (flag-1 HELO) then [`open_session_for_hello`] (unsigned
/// `GET_CAPS` first) — runs the single operation `op` under that
/// session, then closes the session. The session never survives
/// the call, and the gate is never touched: this is a transient
/// control session, not an activation — a successful op does not
/// open the gate, and a failed op does not close a gate that was
/// already open (the forwarder owns the gate). The transport is
/// dropped afterwards: UDP control uses an ephemeral local port,
/// and a serial control session holds the port only for the
/// duration of the op.
///
/// Fail-closed throughout: no `HELLO`, a transport that died, or
/// any op failure returns an [`Err`] with the reason (also
/// recorded on the separate control channel via
/// [`AppStateCore::note_control_error`] — never as an activation
/// failure, so a failed control op cannot knock a live session
/// into `Error` (R2-05)) — never a partial success. A successful
/// activation whose op then fails still closes the session
/// cleanly before returning.
async fn run_control_session<T, K, F, R>(
    state: &Arc<AppStateCore>,
    transport: T,
    keystore: K,
    op: F,
) -> Result<R, String>
where
    T: SessionTransport,
    K: KeyStoreLike,
    // The op's future borrows the `&mut BoardSession` it is handed,
    // so its type depends on that borrow's lifetime — a plain
    // `FnOnce(&mut BoardSession) -> Fut` bound cannot express it.
    // The boxed future carries the borrow lifetime (`'s`) instead.
    F: for<'s> FnOnce(
        &'s mut BoardSession<K, T>,
    ) -> Pin<Box<dyn Future<Output = Result<R, String>> + 's>>,
{
    let mut transport = transport;
    match await_new_session_hello(state, &mut transport).await {
        NewSessionHello::TransportClosed => {
            let reason = "transport closed while awaiting the board HELLO";
            state.note_control_error(reason);
            Err(reason.to_string())
        }
        NewSessionHello::NoRoute => {
            let reason = format!(
                "no HELLO: no route to the board at {} — Wi-Fi not joined to the RSH network (or USB not plugged in)",
                state.settings().esp_ip
            );
            state.note_control_error(&reason);
            Err(reason)
        }
        NewSessionHello::None => {
            let total_ms = SESSION_HELLO_WAIT.as_millis() * SESSION_HELLO_ATTEMPTS as u128;
            let reason = format!(
                "no HELLO reply from the board within {total_ms} ms — board not responding \
                 on this transport"
            );
            state.note_control_error(&reason);
            Err(reason)
        }
        NewSessionHello::Found(hello) => {
            let (mut session, outcome) = open_session_for_hello(transport, keystore, &hello).await;
            let result = match outcome {
                Ok(_) => op(&mut session).await,
                Err(error) => Err(format!("session activation failed: {error}")),
            };
            if let Err(error) = &result {
                // R2-05: a control-session failure is recorded on the
                // separate control channel, never as an activation
                // failure — it must not knock a live session into
                // Error via the manager loop's ActivationFailed feed.
                state.note_control_error(error);
            }
            session.close();
            let _ = session.take_transport();
            result
        }
    }
}

/// Block the calling thread on a control future (H5-control).
///
/// The control commands are synchronous, so
/// each builds a short-lived current-thread tokio runtime and blocks
/// on [`run_control_session`]. Contract: call only OUTSIDE a tokio
/// runtime — Tauri sync commands satisfy this; a call from inside a
/// runtime panics (a runtime cannot be built inside one), so tests
/// must be plain `#[test]`, never `#[tokio::test]`.
///
/// DS-02 (P0 audit): the only in-runtime caller was the daemon
/// manager loop's tray "Restart wheel" path (now `spawn_blocking`,
/// see `rsh-daemon/src/main.rs`); every other caller reaches here
/// via IPC dispatch on `spawn_blocking` (safe — no runtime context)
/// or from a plain thread / test. As a backstop this function now
/// FAILS with an error instead of panicking when it is called from
/// inside a runtime: a future misuse degrades one command instead
/// of killing its caller task.
fn block_on_control<R>(future: impl Future<Output = Result<R, String>>) -> Result<R, String> {
    if tokio::runtime::Handle::try_current().is_ok() {
        return Err(
            "control command called from inside a tokio runtime — run it on spawn_blocking instead (DS-02)"
                .to_string(),
        );
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("could not start the control runtime: {error}"))?;
    runtime.block_on(future)
}

/// The live board identity + firmware version, read under a fresh
/// session (H5-control, `upload_firmware` step 1).
struct LiveBoardInfo {
    /// Board id (`AA:BB:…` MAC), as the session recorded it.
    board_id: String,
    /// Live firmware version from the session's `GET_CAPS`.
    firmware: (u8, u8, u8),
}

/// Read the live board identity + firmware version under a fresh
/// session (H5-control, `upload_firmware` step 1). Synchronous on
/// purpose: it only reads what the session already authenticated —
/// the caller wraps it in [`std::future::ready`] at the call site.
fn op_live_board_info<T, K>(session: &mut BoardSession<K, T>) -> Result<LiveBoardInfo, String>
where
    T: SessionTransport,
    K: KeyStoreLike,
{
    let Some(caps) = session.caps() else {
        return Err("session has no GET_CAPS reply — the session did not activate".to_string());
    };
    Ok(LiveBoardInfo {
        board_id: session.board_id().to_string(),
        firmware: caps.firmware,
    })
}

/// `SET_PASSWORD` under a fresh session (H5-control): change the
/// board AP password, then store the same password as the Wi-Fi / OTA
/// password (the AP password and the `WebOTA` password are one
/// credential — a stale stored password would 401 later uploads).
/// The stored Wi-Fi password (if any) is sent as the current
/// password (D3) — without it, changing a password that is already
/// set fails with `BadPassword`. If the keystore update fails the
/// error says so plainly: the board password DID change, only the
/// local record failed.
async fn op_set_password<T, K>(
    session: &mut BoardSession<K, T>,
    password: String,
) -> Result<(), String>
where
    T: SessionTransport,
    K: KeyStoreLike,
{
    // D3: the v2.2 payload must carry the current password when one is
    // set — the board rejects the change otherwise (BadPassword). The
    // stored Wi-Fi password IS the current one (one credential); no
    // password stored yet means first setup (current = none).
    //
    // R3-D-10: only `NotFound` means "no password stored yet". Any
    // other keystore error (a dead credential store) must fail HERE,
    // before the board is called — the old `.ok()` turned it into
    // `current = None`, the board answered `BadPassword`, and the
    // user was told to hold BOOT for 10 s for a fault that was in
    // the local store, not on the board.
    let board_id = session.board_id().to_string();
    let current = match session.keystore().get_wifi_password(&board_id) {
        Ok(password) => Some(password),
        Err(KeystoreError::NotFound) => None,
        Err(error) => {
            return Err(format!(
                "could not read the stored Wi-Fi password ({error}) — the board password was not changed; check the credential store and retry"
            ));
        }
    };
    {
        let Some(client) = session.control_client_mut() else {
            return Err("session has no control client — the session did not activate".to_string());
        };
        client
            .set_password(current.as_deref().map(str::as_bytes), password.as_bytes())
            .await
            .map_err(|error| match error {
                rsh_transport::control_client::ControlError::BoardError(
                    rsh_protocol::control::ErrorCode::BadPassword,
                ) => "the board already has a Wi-Fi password and the stored one did not match — if you forgot it, hold BOOT for 10 s to clear it, then set a new password".to_string(),
                other => other.to_string(),
            })?;
    }
    session
        .keystore()
        .store_wifi_password(&board_id, &password)
        .map_err(|error| {
            format!(
                "board password changed, but the new password could not be stored ({error}) — \
                 set a Wi-Fi password in Settings before the next firmware upload"
            )
        })?;
    Ok(())
}

/// Parse a stored LED colour (`"#RRGGBB"` or `"RRGGBB"`) into
/// `0xRRGGBB` (M2, v2.0.2). Anything else (wrong length, non-hex)
/// is `None` — never a guessed colour.
#[must_use]
pub fn parse_hex_color(text: &str) -> Option<u32> {
    let hex = text.strip_prefix('#').unwrap_or(text);
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(hex, 16).ok()
}

/// Builds the `LED_TEST` payload for a pattern + brightness
/// (C-LED-1): the brightness (0–100 %) scales the base RGB before
/// encoding, so a 50 % brightness halves every channel. The base
/// colour is white (255, 255, 255) for SOLID / BLINK / CHASE; the
/// rainbow pattern ignores the colour bytes board-side, and OFF sends
/// pattern id 0. The period is validated to 100–5000 ms (the
/// protocol's range) and defaults to 1000 ms; `repeats` is 0 (run
/// 5 s, then stop).
///
/// # Errors
///
/// Returns a message when the period is outside 100–5000 ms.
///
/// # Panics
///
/// Never in practice: the scaled channel is `(255 * brightness +
/// 50) / 100` with brightness clamped to 0–100, so it is always in
/// 0–255 and the `u8` conversion cannot fail.
pub fn build_led_test_pattern(
    pattern: LedPattern,
    brightness: u8,
    period_ms: Option<u16>,
) -> Result<LedTestPattern, String> {
    build_led_test_pattern_with_color(pattern, brightness, period_ms, None)
}

/// [`build_led_test_pattern`] with an explicit colour (M2, v2.0.2):
/// `Some(0xRRGGBB)` tints the test flash; `None` is the historic
/// white flash. Every channel scales with the brightness, so 0%
/// brightness is dark whatever the colour.
///
/// # Errors
///
/// Returns an error when the period is outside 100..=5000 ms.
///
/// # Panics
///
/// Never panics in practice: both `expect` calls guard arithmetic
/// bounded by construction (`255 * brightness <= 25500`, `/ 100 <=
/// 255`; `base * channel / 255 <= 255`).
pub fn build_led_test_pattern_with_color(
    pattern: LedPattern,
    brightness: u8,
    period_ms: Option<u16>,
    color: Option<u32>,
) -> Result<LedTestPattern, String> {
    let period_ms = period_ms.unwrap_or(1000);
    if !(100..=5000).contains(&period_ms) {
        return Err(format!(
            "LED test period {period_ms} ms is outside 100..=5000 ms"
        ));
    }
    let brightness = brightness.min(100);
    // Integer scaling (rounds to nearest): 255 * brightness / 100,
    // exact — no float cast, so no truncation/sign-loss lint.
    let base = u8::try_from((255u32 * u32::from(brightness) + 50) / 100)
        .expect("255 * brightness <= 25500, / 100 <= 255");
    let (r, g, b) = match color {
        Some(rgb) => {
            let scale = |channel: u32| -> u8 {
                u8::try_from(u32::from(base) * channel / 255).expect("base * channel / 255 <= 255")
            };
            (
                scale((rgb >> 16) & 0xFF),
                scale((rgb >> 8) & 0xFF),
                scale(rgb & 0xFF),
            )
        }
        None => (base, base, base),
    };
    Ok(LedTestPattern {
        pattern_id: pattern.pattern_id(),
        r,
        g,
        b,
        period_ms,
        repeats: 0,
    })
}

/// `LED_TEST` under a fresh session (C-LED-1).
async fn op_led_test<T, K>(
    session: &mut BoardSession<K, T>,
    pattern: LedTestPattern,
) -> Result<(), String>
where
    T: SessionTransport,
    K: KeyStoreLike,
{
    let Some(client) = session.control_client_mut() else {
        return Err("session has no control client — the session did not activate".to_string());
    };
    client
        .led_test(&pattern)
        .await
        .map_err(|error| error.to_string())
}

/// `KEY_RESET` under a fresh session (M-6).
async fn op_factory_reset<T, K>(session: &mut BoardSession<K, T>) -> Result<(), String>
where
    T: SessionTransport,
    K: KeyStoreLike,
{
    let Some(client) = session.control_client_mut() else {
        return Err("session has no control client — the session did not activate".to_string());
    };
    client
        .factory_reset()
        .await
        .map_err(|error| error.to_string())
}

/// `REBOOT` under a fresh session (H5-control). The reply arrives
/// before the reboot.
async fn op_reboot<T, K>(session: &mut BoardSession<K, T>) -> Result<(), String>
where
    T: SessionTransport,
    K: KeyStoreLike,
{
    let Some(client) = session.control_client_mut() else {
        return Err("session has no control client — the session did not activate".to_string());
    };
    client.reboot().await.map_err(|error| error.to_string())
}

/// Read the board's whole live config blob under a fresh session
/// (H5-control, `get_config` / `get_pin_map`): chunked `GET_CFG`
/// via a throwaway transaction client (no state change).
async fn op_read_config_blob<T, K>(session: &mut BoardSession<K, T>) -> Result<Vec<u8>, String>
where
    T: SessionTransport,
    K: KeyStoreLike,
{
    let Some(client) = session.take_control_client() else {
        return Err("session has no control client — the session did not activate".to_string());
    };
    let mut txn = ConfigTransaction::new(client);
    let result = txn
        .read_current_blob()
        .await
        .map_err(|error| error.to_string());
    let client = txn.into_inner();
    session.restore_control_client(client);
    result
}

/// Run one config-transaction step under a fresh session, taking the
/// control client out of the session for the step and handing it back
/// before the session closes (so the key is still zeroized exactly
/// once, by [`BoardSession::close`]).
///
/// Every step returns the new snapshot alongside its outcome — the
/// snapshot is saved even when the step fails, so a failed stage
/// (state back to `Idle`) or a timed-out confirm (state still
/// `Committed`) is recorded too, never lost.
async fn op_stage_config<T, K>(
    session: &mut BoardSession<K, T>,
    snapshot: ConfigTxnSnapshot,
    blob: Vec<u8>,
) -> Result<(Result<(), String>, ConfigTxnSnapshot), String>
where
    T: SessionTransport,
    K: KeyStoreLike,
{
    let Some(client) = session.take_control_client() else {
        return Ok((
            Err("session has no control client — the session did not activate".to_string()),
            snapshot,
        ));
    };
    let mut txn = ConfigTransaction::resume(client, snapshot);
    let result = txn.stage(&blob).await.map_err(|error| error.to_string());
    let snapshot = txn.snapshot();
    let client = txn.into_inner();
    session.restore_control_client(client);
    Ok((result, snapshot))
}

/// `COMMIT` under a fresh session (H5-control, `commit_config`).
async fn op_commit_config<T, K>(
    session: &mut BoardSession<K, T>,
    snapshot: ConfigTxnSnapshot,
) -> Result<(Result<(), String>, ConfigTxnSnapshot), String>
where
    T: SessionTransport,
    K: KeyStoreLike,
{
    let Some(client) = session.take_control_client() else {
        return Ok((
            Err("session has no control client — the session did not activate".to_string()),
            snapshot,
        ));
    };
    let mut txn = ConfigTransaction::resume(client, snapshot);
    let result = txn.commit().await.map_err(|error| error.to_string());
    let snapshot = txn.snapshot();
    let client = txn.into_inner();
    session.restore_control_client(client);
    Ok((result, snapshot))
}

/// `CONFIRM` under a fresh session (H5-control, `confirm_config`).
async fn op_confirm_config<T, K>(
    session: &mut BoardSession<K, T>,
    snapshot: ConfigTxnSnapshot,
) -> Result<(Result<(), String>, ConfigTxnSnapshot), String>
where
    T: SessionTransport,
    K: KeyStoreLike,
{
    let Some(client) = session.take_control_client() else {
        return Ok((
            Err("session has no control client — the session did not activate".to_string()),
            snapshot,
        ));
    };
    let mut txn = ConfigTransaction::resume(client, snapshot);
    let result = txn.confirm().await.map_err(|error| error.to_string());
    let snapshot = txn.snapshot();
    let client = txn.into_inner();
    session.restore_control_client(client);
    Ok((result, snapshot))
}

/// `REVERT` + read-back verify under a fresh session (H5-control,
/// `rollback_config`).
async fn op_rollback_config<T, K>(
    session: &mut BoardSession<K, T>,
    snapshot: ConfigTxnSnapshot,
) -> Result<(Result<(), String>, ConfigTxnSnapshot), String>
where
    T: SessionTransport,
    K: KeyStoreLike,
{
    let Some(client) = session.take_control_client() else {
        return Ok((
            Err("session has no control client — the session did not activate".to_string()),
            snapshot,
        ));
    };
    let mut txn = ConfigTransaction::resume(client, snapshot);
    let result = txn.rollback().await.map_err(|error| error.to_string());
    let snapshot = txn.snapshot();
    let client = txn.into_inner();
    session.restore_control_client(client);
    Ok((result, snapshot))
}

/// Stage + `COMMIT` a pin map under one fresh session (H5-control,
/// `set_pin_map`): read the live blob, merge the submitted map into
/// it (role-based, see [`merge_pin_map_into_blob`]), then stage +
/// commit, so the editor's 30 s confirm bar follows immediately.
///
/// A merge failure is not a control error — it is a validation
/// verdict: the returned [`PinMapValidation`] is `valid: false` with
/// the reason, and nothing is staged. A stage/commit failure is an
/// [`Err`] (the board was touched and the outcome is unknown).
async fn op_stage_and_commit_pin_map<T, K>(
    session: &mut BoardSession<K, T>,
    snapshot: ConfigTxnSnapshot,
    pins: Vec<PinAssignment>,
) -> Result<(Result<PinMapValidation, String>, ConfigTxnSnapshot), String>
where
    T: SessionTransport,
    K: KeyStoreLike,
{
    let Some(client) = session.take_control_client() else {
        return Ok((
            Err("session has no control client — the session did not activate".to_string()),
            snapshot,
        ));
    };
    let mut txn = ConfigTransaction::resume(client, snapshot);
    let result = async {
        let live = txn
            .read_current_blob()
            .await
            .map_err(|error| error.to_string())?;
        let live_blob = ConfigBlob::decode(&live).map_err(|error| {
            format!("board returned a config blob that does not decode: {error}")
        })?;
        let merged = match merge_pin_map_into_blob(&live_blob, &pins) {
            Ok(blob) => blob,
            Err(error) => {
                return Ok(PinMapValidation {
                    valid: false,
                    errors: vec![error],
                    warnings: Vec::new(),
                });
            }
        };
        let merged_bytes = merged
            .encode()
            .map_err(|error| format!("merged pin map could not be encoded: {error}"))?;
        txn.stage(&merged_bytes)
            .await
            .map_err(|error| error.to_string())?;
        txn.commit().await.map_err(|error| error.to_string())?;
        Ok(PinMapValidation {
            valid: true,
            errors: Vec::new(),
            warnings: Vec::new(),
        })
    }
    .await;
    let snapshot = txn.snapshot();
    let client = txn.into_inner();
    session.restore_control_client(client);
    Ok((result, snapshot))
}

/// Spawns the transport forwarder task for `kind`: it builds the
/// transport, runs the session activator on it (H5 — signed
/// `GET_CAPS` first, gate opens only on success), forwards stream
/// frames into the engine's frame channel, pumps status packets into
/// the engine's status channel (H1 condition 12), and rebuilds the
/// transport with exponential backoff (M-11) after a loss until
/// aborted or the circuit breaker trips (user reconnect required).
/// The task never panics on transport errors — every failure is
/// noted on the state and retried.
/// Stream frames are held while the HMAC session gate is closed
/// (SEC-E-14, H1 condition 13) — see [`forward_with_retry`].
fn spawn_forwarder(
    state: Arc<AppStateCore>,
    kind: TransportKind,
    frame_tx: tokio::sync::mpsc::Sender<StreamFrame>,
    status_tx: tokio::sync::mpsc::Sender<StatusPacket>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        match kind {
            TransportKind::Udp => {
                // mDNS resolution blocks (up to the 500 ms bound) —
                // never on the network runtime thread: resolve it on
                // the blocking pool, then forward on the runtime.
                let state_for_config = Arc::clone(&state);
                let config = tokio::task::spawn_blocking(move || udp_config_for(&state_for_config))
                    .await
                    .unwrap_or_default();
                let state_for_bind = Arc::clone(&state);
                forward_with_retry(state, kind, frame_tx, status_tx, move || {
                    // C-12: the bind failure is typed and recorded —
                    // never dropped with `.ok()`.
                    match UdpTransport::bind_typed(config) {
                        Ok(transport) => Some(transport),
                        Err(bind_error) => {
                            let is_addr_in_use = matches!(bind_error, UdpBindError::AddrInUse);
                            let kind = match bind_error {
                                UdpBindError::AddrInUse => rsh_api::BindErrorKind::AddrInUse,
                                UdpBindError::PermissionDenied => {
                                    rsh_api::BindErrorKind::PermissionDenied
                                }
                                UdpBindError::NoInterface => rsh_api::BindErrorKind::NoInterface,
                                UdpBindError::Other(_) => rsh_api::BindErrorKind::Other,
                            };
                            // DS-06: diagnose the holder BEFORE
                            // recording anything. An `AddrInUse`
                            // whose holder is our own process (or
                            // no holder at all — the port reads free)
                            // is our own rebind race, not another
                            // app: the previous transport's reader
                            // thread still holds the socket for
                            // ~100 ms after the drop, so the retry's
                            // `bind(4211)` collides with ourselves
                            // (measured 3/3, no other process
                            // holding the port). It must NOT be
                            // recorded as a foreign
                            // `BindFailed(AddrInUse)` — that latches
                            // the F21 terminal stop with a wrong
                            // "another app" diagnosis ~5 s after
                            // launch on any board-less machine.
                            // It returns `None` with NO typed
                            // `BindFailed` recorded, so the forwarder
                            // retries with the normal M-11 backoff,
                            // by which time the socket is free. Only
                            // a foreign pid takes the F21 path below.
                            let holder = if is_addr_in_use {
                                diagnose_udp_port_holder(config.local_port)
                            } else {
                                None
                            };
                            if is_addr_in_use && !udp_addr_in_use_is_foreign(holder.as_ref()) {
                                state_for_bind.note_engine_transport_error(
                                    TransportKind::Udp,
                                    "UDP bind failed: AddrInUse held by our own just-closed socket (or no holder found) — retrying with backoff",
                                );
                                state_for_bind.note_engine_detail(
                                    "transport udp port conflict (AddrInUse, UDP 4211) was our own rebind race — retrying, not a foreign holder"
                                        .to_string(),
                                );
                                return None;
                            }
                            // F21: name the holder. A port conflict
                            // is a fact about another process, not a
                            // transient error — the diagnosis goes
                            // into the recorded reason so the log and
                            // the engine status say WHO holds UDP
                            // 4211 (usually the still-running legacy
                            // Tauri app, `RSH.exe`).
                            let holder_detail = holder
                                .map(|holder| {
                                    if holder.is_legacy_rsh_app() {
                                        format!(
                                            " — port held by the old RSH app: {}",
                                            holder.describe()
                                        )
                                    } else if holder.is_current_rsh() {
                                        format!(
                                            " — port held by another RSH process: {}",
                                            holder.describe()
                                        )
                                    } else {
                                        format!(" — port held by {}", holder.describe())
                                    }
                                })
                                .unwrap_or_default();
                            state_for_bind.note_engine_transport_error(
                                TransportKind::Udp,
                                &format!(
                                    "UDP bind failed: {} — {}{holder_detail}",
                                    kind.code(),
                                    kind.hint()
                                ),
                            );
                            state_for_bind.note_session_error_typed(
                                ConnectionError::BindFailed(kind),
                                &format!("UDP bind failed: {}{holder_detail}", kind.code()),
                            );
                            None
                        }
                    }
                })
                .await;
            }
            TransportKind::Serial => {
                let state_for_open = Arc::clone(&state);
                forward_with_retry(state, kind, frame_tx, status_tx, move || {
                    let port = detect_board_port()?;
                    match SerialTransport::open(SerialConfig::new(port)) {
                        Ok(transport) => Some(transport),
                        Err(error) => {
                            state_for_open.note_engine_transport_error(
                                TransportKind::Serial,
                                &format!("serial port could not be opened: {error}"),
                            );
                            None
                        }
                    }
                })
                .await;
            }
        }
    })
}

/// DS-06: whether a diagnosed UDP 4211 holder makes `AddrInUse`
/// a foreign conflict (terminal, F21) or our own rebind race
/// (transient, retry with the normal M-11 backoff). A holder that
/// is our own process (`pid == ours`) — or no holder at all (the
/// port reads free, i.e. our just-dropped socket was the holder
/// and its reader thread had not released the fd yet) — is our
/// own race, not another app. Only a foreign pid is terminal.
/// Pure: unit-tested without binding anything.
fn udp_addr_in_use_is_foreign(holder: Option<&PortHolder>) -> bool {
    match holder {
        Some(holder) => holder.pid != std::process::id(),
        None => false,
    }
}

/// Builds the UDP config from settings: the board IP is the
/// configured `esp_ip` when it parses as an IP literal, else the
/// `192.168.4.1` default (settings validation already clamps a
/// non-private `esp_ip` to the default on load, so the fallback here
/// only fires for a hand-edited store).
fn udp_config_for(state: &AppStateCore) -> UdpConfig {
    let esp_ip = state.settings().esp_ip;
    // C-11: `rsh.local` is resolved via mDNS (discovery.rs — it
    // existed but had no caller) with the 192.168.4.1 fallback.
    // The lookup is bounded to 500 ms so a missing board never
    // stalls connect; the caller runs this off the runtime thread.
    if esp_ip.eq_ignore_ascii_case("rsh.local") {
        let resolved = Discovery::new(IpAddr::V4(DEFAULT_BOARD_IP))
            .with_mdns_timeout(Duration::from_millis(500))
            .resolve_board();
        state.note_engine_detail(format!(
            "rsh.local resolved via mDNS to {resolved} (fallback 192.168.4.1)"
        ));
        return UdpConfig::new(resolved);
    }
    // M-1: only a private-range or loopback board IP is accepted; a
    // public IP (or a non-literal) falls back to the board AP default
    // and notes why, so a mistyped settings value can never point
    // the transport at a public host.
    if let Ok(ip) = esp_ip.parse::<std::net::Ipv4Addr>()
        && is_allowed_board_ip(&ip)
    {
        UdpConfig::new(IpAddr::V4(ip))
    } else {
        state.note_engine_detail(format!(
            "configured esp_ip '{esp_ip}' is not a private/loopback IP literal — using default 192.168.4.1"
        ));
        UdpConfig::default()
    }
}

/// Forwards frames from a transport built by `factory` into the
/// engine's frame channel, rebuilding the transport after a build
/// failure or a source loss with exponential backoff (M-11): the
/// first rebuild waits 2 s ([`RECONNECT_INTERVAL`]), each
/// consecutive failure doubles the wait up to a 30 s cap, and a
/// transport that connects and authenticates resets the streak. After
/// 10 consecutive failures the circuit breaker opens: retrying stops,
/// the connection reports `Disconnected`, and the user must reconnect
/// (a new forwarder starts on the next connect — a dead board must
/// not spin the app forever on a fixed 2 s loop). A full frame
/// channel drops the newest frame (the engine keeps only the latest
/// frame per tick anyway) instead of blocking the forwarder. Status
/// packets are pumped into the engine's status channel after every
/// stream frame (H1 condition 12) — but only while the session gate
/// is open: the gate (SEC-E-14, H1 condition 13) holds ALL board
/// data, stream frames and status packets alike, until a
/// `BoardSession` has completed its HMAC-verified `GET_CAPS`. Held
/// frames are counted (never silently dropped) and the gate closes
/// again on transport loss, so a rebuilt transport must
/// re-authenticate.
///
/// Before the frame loop, the session activator (H5) runs on the
/// freshly built transport ([`activate_session_on`]): flag-1 HELO →
/// `HELLO` → `pair`/`resume` with a signed `GET_CAPS` first → gate
/// opens on success. A failed activation does not stop the loop —
/// the transport is alive and frames are held + counted until a
/// later activation (reconnect) succeeds. The loop
/// ends only when the task is aborted or the circuit breaker trips.
async fn forward_with_retry<T, F>(
    state: Arc<AppStateCore>,
    kind: TransportKind,
    frame_tx: tokio::sync::mpsc::Sender<StreamFrame>,
    status_tx: tokio::sync::mpsc::Sender<StatusPacket>,
    factory: F,
) where
    T: SessionTransport + Send + 'static,
    F: FnMut() -> Option<T> + Send + 'static,
{
    forward_with_retry_with_policy(
        state,
        kind,
        frame_tx,
        status_tx,
        factory,
        ForwarderRetryPolicy::default_policy(),
    )
    .await;
}

/// Base retry delay for the transport forwarder (M-11): the first
/// rebuild waits the engine's [`RECONNECT_INTERVAL`] (2 s).
const FORWARDER_BACKOFF_BASE: Duration = rsh_engine::engine::RECONNECT_INTERVAL;

/// Cap on the forwarder retry delay (M-11): the wait never exceeds
/// 30 s however long the failure streak runs.
const FORWARDER_BACKOFF_MAX: Duration = Duration::from_secs(30);

/// Consecutive forwarder failures that open the circuit breaker
/// (M-11): after this many failures in a row retrying stops and the
/// user must reconnect. With the 2 s → 30 s backoff the breaker
/// trips after ~3 min of retries (2 + 4 + 8 + 16 + 5 × 30 s) —
/// long enough to ride out a board reboot and Wi-Fi reassociation,
/// short enough that a dead board does not retry forever.
const FORWARDER_MAX_CONSECUTIVE_FAILURES: u32 = 10;

/// Frame silence that means transport loss (R2-06): once the gate
/// is open, no stream frame for this long — a board that stopped
/// streaming (reboot, Wi-Fi drop) while its socket stays live —
/// ends the frame loop so the transport is rebuilt and the session
/// re-activates. Well above the 250 ms engine watchdog (which
/// already drives output neutral) and the ~5 ms frame spacing, so
/// a healthy stream never trips it.
const FRAME_SILENCE_TIMEOUT: Duration = Duration::from_secs(2);

/// Exponential-backoff + circuit-breaker policy for one
/// [`forward_with_retry`] run (M-11). Pure data: the delay math is
/// unit-tested without sleeping.
#[derive(Clone, Copy, Debug)]
struct ForwarderRetryPolicy {
    /// Delay before the first retry.
    base: Duration,
    /// Delay cap.
    max_delay: Duration,
    /// Consecutive failures that trip the breaker.
    max_consecutive_failures: u32,
}

impl ForwarderRetryPolicy {
    /// The production policy: 2 s base, 30 s cap, breaker at 10.
    const fn default_policy() -> Self {
        Self {
            base: FORWARDER_BACKOFF_BASE,
            max_delay: FORWARDER_BACKOFF_MAX,
            max_consecutive_failures: FORWARDER_MAX_CONSECUTIVE_FAILURES,
        }
    }

    /// Delay before the next attempt after `consecutive_failures`
    /// failures (1-based): the base doubled once per failure after
    /// the first, capped at [`Self::max_delay`].
    fn delay_after(self, consecutive_failures: u32) -> Duration {
        let doublings = consecutive_failures.saturating_sub(1).min(30);
        self.base
            .saturating_mul(1u32 << doublings)
            .min(self.max_delay)
    }
}

/// Backoff state for one [`forward_with_retry`] run (M-11): counts
/// the current consecutive-failure streak against a
/// [`ForwarderRetryPolicy`].
#[derive(Clone, Copy, Debug)]
struct ForwarderBackoff {
    policy: ForwarderRetryPolicy,
    consecutive_failures: u32,
}

impl ForwarderBackoff {
    /// A fresh streak under `policy`.
    const fn new(policy: ForwarderRetryPolicy) -> Self {
        Self {
            policy,
            consecutive_failures: 0,
        }
    }

    /// A transport that connected and authenticated breaks the
    /// streak: the next failure starts again at the base delay.
    fn record_success(&mut self) {
        self.consecutive_failures = 0;
    }

    /// Records one failure: the delay before the next attempt, or
    /// [`None`] when the streak reached
    /// [`FORWARDER_MAX_CONSECUTIVE_FAILURES`] and the circuit
    /// breaker trips.
    fn record_failure(&mut self) -> Option<Duration> {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        if self.consecutive_failures >= self.policy.max_consecutive_failures {
            None
        } else {
            Some(self.policy.delay_after(self.consecutive_failures))
        }
    }

    /// The current consecutive-failure streak (for status text).
    const fn consecutive_failures(self) -> u32 {
        self.consecutive_failures
    }
}

/// Sleeps the M-11 backoff delay after one forwarder failure, or
/// opens the circuit breaker when the streak is spent. Returns
/// `true` when the caller should retry, `false` when retrying stops
/// (breaker open — the caller returns, the connection stays
/// `Disconnected`, and the user must reconnect).
async fn forwarder_retry_after_failure(
    state: &AppStateCore,
    kind: TransportKind,
    backoff: &mut ForwarderBackoff,
    detail: String,
) -> bool {
    if let Some(delay) = backoff.record_failure() {
        state.set_connection(ConnectionState::Disconnected);
        state.set_engine_status(
            EngineTaskStatus::Running { transport: None },
            format!(
                "{detail} — retrying in {delay:?} (consecutive failure {})",
                backoff.consecutive_failures()
            ),
        );
        // R3-D-02: the backoff sleep IS a forwarder retry wait —
        // arm it for the whole sleep, so a manager `RetryTimer`
        // or UI Connect cannot abort this run mid-sleep and reset
        // the streak the breaker counts on.
        let _retry_wait = ForwarderRetryWaitGuard::arm(&state.forwarder_retry_wait);
        tokio::time::sleep(delay).await;
        true
    } else {
        // R3-D-02: the breaker tripped — latch it, so no automatic
        // retry starts a fresh forwarder run (and a fresh streak)
        // behind this one. Only an explicit user Connect clears it.
        state.forwarder_breaker_open.store(true, Ordering::SeqCst);
        state.set_connection(ConnectionState::Disconnected);
        state.set_engine_status(
            EngineTaskStatus::Running { transport: None },
            format!(
                "{detail} — circuit breaker open after {} consecutive failures; reconnect required (connect again from the UI)",
                backoff.consecutive_failures()
            ),
        );
        state.note_engine_detail(format!(
            "transport {} circuit breaker open after {} consecutive failures — forwarder stopped, user reconnect required",
            kind.as_str(),
            backoff.consecutive_failures()
        ));
        false
    }
}

/// [`forward_with_retry`] with an injectable [`ForwarderRetryPolicy`]
/// (the test seam: tests trip the breaker with millisecond delays
/// instead of waiting out the production ~3 min; production uses
/// [`ForwarderRetryPolicy::default_policy`]).
// One function: the activation-gate check (C-01/C-02), the BadTag
// retry (C-13), and the frame loop share the transport + backoff
// state — splitting them would scatter one loop's invariants.
#[allow(clippy::too_many_lines)]
async fn forward_with_retry_with_policy<T, F>(
    state: Arc<AppStateCore>,
    kind: TransportKind,
    frame_tx: tokio::sync::mpsc::Sender<StreamFrame>,
    status_tx: tokio::sync::mpsc::Sender<StatusPacket>,
    factory: F,
    policy: ForwarderRetryPolicy,
) where
    T: SessionTransport + Send + 'static,
    F: FnMut() -> Option<T> + Send + 'static,
{
    let mut backoff = ForwarderBackoff::new(policy);
    // R2-29: the factory runs behind a shared lock so each build
    // can be offloaded to the blocking pool below (the factory is
    // `FnMut`, and `spawn_blocking` takes `FnOnce` — one call per
    // offloaded closure).
    let factory = std::sync::Arc::new(std::sync::Mutex::new(factory));
    loop {
        // R2-29: build the transport on the blocking pool, never on
        // the supervisor's single-threaded runtime. The serial
        // factory enumerates ports + HELO-probes each one (up to
        // 500 ms per candidate) and the UDP factory binds +
        // diagnoses the port holder — either one stalled
        // connect/disconnect handling while it ran inline here. A
        // failed/shut-down blocking pool is a failed build (the
        // backoff/breaker below handles it like any other).
        let transport = {
            let factory = std::sync::Arc::clone(&factory);
            tokio::task::spawn_blocking(move || {
                let mut factory = factory
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                factory()
            })
            .await
            .unwrap_or(None)
        };
        let Some(transport) = transport else {
            // F21: a UDP port conflict never self-heals — the holder
            // must be closed by the user. Do not burn the M-11
            // backoff (10 retries, ~3 min) on it: stop this
            // forwarder run now; the user reconnects after closing
            // the other app. Scoped to UDP: the typed error is set
            // by the UDP factory above on every bind failure, so
            // within a UDP run it is fresh (a serial run never
            // sets it and never takes this branch). Other open
            // failures keep the backoff + breaker — a board reboot
            // or Wi-Fi flap CAN heal, a port holder cannot.
            if kind == TransportKind::Udp
                && matches!(
                    state.session_last_error_typed(),
                    Some(ConnectionError::BindFailed(
                        rsh_api::BindErrorKind::AddrInUse
                    ))
                )
            {
                state.set_connection(ConnectionState::Disconnected);
                // DS-06 (d): the user-facing text carries the
                // diagnosed holder — the recorded reason already
                // names it ("UDP bind failed: AddrInUse — port
                // held by RSH.exe (pid …)") — instead of only the
                // generic "another app" line.
                let recorded = state.session_last_error().unwrap_or_default();
                let status = if recorded.is_empty() {
                    "wheel port (UDP 4211) is held by another app — close the other RSH, then reconnect (no automatic retry)".to_string()
                } else {
                    format!("{recorded} — close the other app, then reconnect (no automatic retry)")
                };
                state.set_engine_status(EngineTaskStatus::Running { transport: None }, status);
                state.note_engine_detail(
                    "transport udp port conflict (AddrInUse, UDP 4211) — forwarder stopped, no bind retry; user reconnect required after closing the holder".to_string(),
                );
                // R3-D-02: the forwarder stopped — latch it like a
                // tripped breaker, so a manager retry timer armed
                // by an earlier failure cannot respawn a fresh
                // forwarder (and a fresh bind attempt) behind it.
                // Only an explicit user Connect clears it.
                state.forwarder_breaker_open.store(true, Ordering::SeqCst);
                return;
            }
            state.note_engine_transport_error(kind, "transport could not be opened");
            if !forwarder_retry_after_failure(
                &state,
                kind,
                &mut backoff,
                format!("transport {} could not be opened", kind.as_str()),
            )
            .await
            {
                return;
            }
            continue;
        };
        // C-02: the state is Connecting — NOT Connected — until the
        // session gate opens (unsigned GET_CAPS passed). The old code
        // set Connected here, before the session, so the UI showed
        // "Connected" while no session existed.
        state.set_connection(ConnectionState::Connecting);
        state.set_engine_status(
            EngineTaskStatus::Running {
                transport: Some(kind),
            },
            format!(
                "transport {} connected — connecting session (SEC-E-14); frames held until the session gate opens",
                kind.as_str()
            ),
        );
        // Session activation (H5): unsigned GET_CAPS first, gate opens
        // only on success, transport handed back for streaming. A
        // failed activation returns the transport (gate stays closed);
        // only a transport that died mid-activation returns None, and
        // that rebuilds like a loss.
        let Some(mut transport) = activate_session_on(&state, transport, KeyStore::new()).await
        else {
            state.close_session_gate();
            state.clear_session_records();
            if !forwarder_retry_after_failure(
                &state,
                kind,
                &mut backoff,
                format!("transport {} lost during session activation", kind.as_str()),
            )
            .await
            {
                return;
            }
            continue;
        };
        // C-01: a failed activation no longer falls through to the
        // frame loop "Connected" with no data (the old behaviour:
        // one attempt, then "Connected" forever). Nothing latches
        // on a pairing reason anymore (pairing does not exist): a
        // failed activation enters the frame loop with the gate
        // closed — the transport stays up, frames are held and
        // counted (SEC-E-14), and re-activation is attempted when
        // the backoff deadline fires. Dropping the transport on a
        // retryable failure would lose the held frames and the
        // "gate closed" proof the rig relies on.
        let mut retry_deadline: Option<tokio::time::Instant> = None;
        // R3-D-02: held while a retry deadline is armed (below) —
        // the forwarder owns the retry in that window, so the
        // manager / UI must not abort this run. Dropped when the
        // gate opens, when the deadline fires, or at the end of
        // this iteration (transport loss falls through to
        // `forwarder_retry_after_failure`, which arms its own
        // wait for the backoff sleep).
        let mut _retry_wait_guard: Option<ForwarderRetryWaitGuard<'_>> = None;
        if state.session_gate_open() {
            // A connected transport breaks the failure streak
            // (M-11): the next loss starts again at the 2 s base.
            // Only now — gate open, GET_CAPS passed — is it
            // Connected.
            backoff.record_success();
            state.set_connection(ConnectionState::Connected);
        } else {
            let detail = format!(
                "transport {} session activation failed ({})",
                kind.as_str(),
                state.session_last_error().unwrap_or_default()
            );
            if let Some(delay) = backoff.record_failure() {
                retry_deadline = Some(tokio::time::Instant::now() + delay);
                // R3-D-02: the deadline wait is a forwarder retry
                // wait — arm it now, before the frame loop starts
                // waiting on it.
                _retry_wait_guard = Some(ForwarderRetryWaitGuard::arm(&state.forwarder_retry_wait));
                state.set_engine_status(
                    EngineTaskStatus::Running {
                        transport: Some(kind),
                    },
                    format!(
                        "{detail} — holding frames, retrying activation in {delay:?} (consecutive failure {})",
                        backoff.consecutive_failures()
                    ),
                );
            } else {
                // R3-D-02: the breaker tripped — latch it, so no
                // automatic retry starts a fresh forwarder run (and
                // a fresh streak) behind this one. Only an explicit
                // user Connect clears it.
                state.forwarder_breaker_open.store(true, Ordering::SeqCst);
                state.set_connection(ConnectionState::Disconnected);
                state.set_engine_status(
                    EngineTaskStatus::Running { transport: None },
                    format!(
                        "{detail} — circuit breaker open after {} consecutive failures; reconnect required (connect again from the UI)",
                        backoff.consecutive_failures()
                    ),
                );
                state.note_engine_detail(format!(
                    "transport {} circuit breaker open after {} consecutive failures — forwarder stopped, user reconnect required",
                    kind.as_str(),
                    backoff.consecutive_failures()
                ));
                return;
            }
        }
        // Event-driven drain: awaiting the source blocks until a
        // frame arrives; `None` means the source dropped. While a
        // retry deadline is armed (activation failed, gate closed)
        // the wait is bounded by it: the deadline fires the outer
        // loop, which rebuilds and re-activates. Once the gate
        // opens the deadline is cleared — an authenticated
        // transport never re-activates mid-stream.
        let mut deadline_fired = false;
        loop {
            let frame = match retry_deadline {
                Some(deadline) if !state.session_gate_open() => {
                    match tokio::time::timeout_at(deadline, transport.next_frame()).await {
                        Ok(Some(frame)) => frame,
                        Ok(None) => break, // transport dropped
                        Err(_) => {
                            deadline_fired = true;
                            break; // retry deadline: re-activate
                        }
                    }
                }
                _ => {
                    // R2-06: bound the authenticated wait — a live
                    // socket with a silent board used to pend here
                    // forever (silence only ended the loop if the
                    // source channel *closed*). 2 s of silence is
                    // transport loss: break out so the loop rebuilds
                    // + re-activates below, like any other loss.
                    match tokio::time::timeout(FRAME_SILENCE_TIMEOUT, transport.next_frame()).await
                    {
                        Ok(Some(frame)) => frame,
                        Ok(None) => break, // transport dropped
                        Err(_) => {
                            state.note_engine_detail(
                                "stream silent for 2 s — treating as transport loss, rebuilding"
                                    .to_string(),
                            );
                            break;
                        }
                    }
                }
            };
            // R2-07: drain HELLO replies that arrived while
            // streaming — the frame loop is the only reader left
            // after activation, so without this the 8-cap channel
            // overflow-drops and the observed HELLO goes stale (a
            // new HELLO after a board reboot was never seen).
            // Every reply refreshes the observed HELLO via
            // `note_observed_hello`.
            while let Some(hello) = transport.try_next_hello() {
                state.note_observed_hello(&hello);
            }
            // C-04 heartbeat: every frame (stream or status, via the
            // status pump below) stamps the watchdog clock.
            state.note_stream_frame();
            // Session gate (SEC-E-14): hold unauthenticated frames —
            // counted, never silently dropped — before they can reach
            // the engine, the snapshots, or the output backend.
            if !state.session_gate_open() {
                state.note_gate_held_frame();
                continue;
            }
            retry_deadline = None;
            // R3-D-02: the gate opened — the wait is over, the
            // forwarder no longer owns a retry.
            _retry_wait_guard = None;
            // Status pump (H1 condition 12): board loss counters and
            // `btnsEx` reach the engine. A full/closed engine status
            // channel drops the packet (the engine drains its own
            // channel every tick; a stale counter is never fatal).
            while let Some(status) = transport.try_next_status() {
                let _ = status_tx.try_send(status);
            }
            // The frame's cumulative encoder counts are the snapshot's
            // truth (see `note_frame_encoders`); report them before
            // handing the frame to the engine.
            state.note_frame_encoders(frame.encoders, frame.n_enc);
            // v2.0.2 capture plumbing: the frame's raw mV is the
            // capture source (see `note_raw_axes`) — reported for
            // authenticated frames only, next to the encoders.
            state.note_raw_axes(frame.axes_mv);
            if frame_tx.try_send(frame).is_err() {
                // Channel full (engine behind) or closed (engine
                // stopping): drop the newest frame, never block.
            }
        }
        // Transport loss ends the session: the gate closes and the
        // session records are cleared, so the rebuilt transport must
        // re-authenticate (SEC-E-14). A fired retry deadline is not
        // a second failure: the backoff delay was already spent
        // holding frames in the loop above, and the failure was
        // already recorded when the deadline was armed — rebuild +
        // re-activate now, no second sleep and no second count.
        state.close_session_gate();
        state.clear_session_records();
        if deadline_fired {
            // R3-D-02: the deadline fired — the wait is over, the
            // rebuild below starts a fresh activation attempt.
            _retry_wait_guard = None;
            // DS-06 (a): teardown-before-rebind — the old
            // transport (and its socket, still held by its reader
            // thread for ~100 ms after the flag is set) is dropped
            // HERE, before the loop-top factory binds again, so
            // the rebind cannot collide with it. The drop was
            // previously implicit at the end of the iteration; it
            // is explicit now so the ordering is visible and cannot
            // regress silently.
            drop(transport);
            continue;
        }
        if !forwarder_retry_after_failure(
            &state,
            kind,
            &mut backoff,
            format!("transport {} lost", kind.as_str()),
        )
        .await
        {
            return;
        }
    }
}

/// Human-readable detail for a live backend kind, as
/// [`AppStateCore::output_state`] reports it. The `HIDMaestro` line
/// always carries the UAC note: the elevated helper runs for the whole
/// session, so Windows asks on every launch while it is active.
#[must_use]
fn live_backend_detail(kind: BackendKind, vjoy_device_id: u32) -> String {
    match kind {
        BackendKind::HidMaestro => format!("HIDMaestro — explicit-choice backend. {UAC_NOTE}"),
        BackendKind::VJoy => {
            format!("vJoy device {vjoy_device_id} — default backend, no UAC prompt needed.")
        }
        BackendKind::None => "No output backend — output is disabled.".to_string(),
    }
}

/// Millivolts (`f64`, already range-checked by the caller) to the
/// profile's `i32` millivolt fields. The value is clamped to the ADC
/// scale first, so the truncating cast below cannot overflow.
#[allow(clippy::cast_possible_truncation)]
fn mv_to_i32(value: f64) -> i32 {
    value.round().clamp(0.0, 3300.0) as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, AtomicUsize};
    use std::time::Duration;

    use rsh_engine::encoder::EncoderButtons;
    use rsh_protocol::PinTableEntry;
    use rsh_security::keystore::KeystoreError;

    /// A test-only recording backend (lives in this test module, never
    /// in `rsh-output` and never in production code): implements the
    /// real [`IJoystickOutput`] trait, records every state it is sent,
    /// and fails every update when `fail` is set.
    #[derive(Clone)]
    struct RecordingBackend {
        sent: Arc<Mutex<Vec<PipelineOutputState>>>,
        connected: Arc<Mutex<bool>>,
        fail: Arc<Mutex<bool>>,
        fail_connect: Arc<Mutex<bool>>,
        /// Message a failing `try_connect` reports (default "mock
        /// connect failed"; tests set it to e.g. an "administrator
        /// rights" wording to model a UAC decline).
        fail_connect_message: Arc<Mutex<String>>,
        /// Reported by `name()`; the failover test needs a backend
        /// that reports `"HIDMaestro"` / `"vJoy"`.
        name: &'static str,
    }

    impl RecordingBackend {
        /// Test-only constructor: the backend plus handles to its
        /// recorded states and its fail flag.
        #[allow(clippy::type_complexity)]
        fn new() -> (Self, Arc<Mutex<Vec<PipelineOutputState>>>, Arc<Mutex<bool>>) {
            Self::new_named("RecordingBackend")
        }

        /// Test-only constructor with a reported backend name
        /// (C-FAIL-1: the failover keys off the live backend's name).
        #[allow(clippy::type_complexity)]
        fn new_named(
            name: &'static str,
        ) -> (Self, Arc<Mutex<Vec<PipelineOutputState>>>, Arc<Mutex<bool>>) {
            let sent = Arc::new(Mutex::new(Vec::new()));
            let connected = Arc::new(Mutex::new(false));
            let fail = Arc::new(Mutex::new(false));
            (
                Self {
                    sent: Arc::clone(&sent),
                    connected: Arc::clone(&connected),
                    fail: Arc::clone(&fail),
                    fail_connect: Arc::new(Mutex::new(false)),
                    fail_connect_message: Arc::new(Mutex::new("mock connect failed".to_string())),
                    name,
                },
                sent,
                fail,
            )
        }

        /// Makes every later `try_connect` fail (mock driver missing).
        fn set_fail_connect(&self, fail: bool) {
            *self
                .fail_connect
                .lock()
                .expect("fail_connect lock poisoned") = fail;
        }

        /// Message a failing `try_connect` reports.
        fn set_fail_connect_message(&self, message: &str) {
            *self
                .fail_connect_message
                .lock()
                .expect("fail_connect_message lock poisoned") = message.to_string();
        }
    }

    impl IJoystickOutput for RecordingBackend {
        fn name(&self) -> &'static str {
            self.name
        }

        fn is_connected(&self) -> bool {
            *self.connected.lock().expect("connected lock poisoned")
        }

        fn try_connect(&mut self) -> Result<(), VJoyError> {
            if *self
                .fail_connect
                .lock()
                .expect("fail_connect lock poisoned")
            {
                let message = self
                    .fail_connect_message
                    .lock()
                    .expect("fail_connect_message lock poisoned")
                    .clone();
                return Err(VJoyError::DriverNotFound(message));
            }
            *self.connected.lock().expect("connected lock poisoned") = true;
            Ok(())
        }

        fn disconnect(&mut self) {
            *self.connected.lock().expect("connected lock poisoned") = false;
        }

        fn update(&mut self, state: &ProcessedState) -> Result<(), VJoyError> {
            if !self.is_connected() {
                return Err(VJoyError::NotConnected);
            }
            if *self.fail.lock().expect("fail lock poisoned") {
                return Err(VJoyError::UpdateFailed(1));
            }
            self.sent.lock().expect("sent lock poisoned").push(*state);
            Ok(())
        }

        fn send_neutral(&mut self) -> Result<(), VJoyError> {
            if !self.is_connected() {
                return Ok(());
            }
            self.sent
                .lock()
                .expect("sent lock poisoned")
                .push(PipelineOutputState::neutral());
            Ok(())
        }
    }

    /// One side of the `BlockingBackend` handshake: a bool flag
    /// plus its condvar (test-only; keeps `new()`'s return type
    /// below clippy's complexity threshold).
    type FlagPair = Arc<(Mutex<bool>, Condvar)>;

    /// Test-only backend whose `update()` blocks until the test
    /// releases it (PERF F-15): models a `HIDMaestro` sidecar
    /// round-trip that takes ~200 ms, so the test can prove a
    /// backend switch does not wait it out. `update()` signals
    /// `entered` on entry and waits on `release` (bounded, so a
    /// regression fails the test instead of hanging the suite).
    /// Connected from construction — the test enables output
    /// directly instead of going through the connect path.
    struct BlockingBackend {
        entered: FlagPair,
        release: FlagPair,
        connected: Arc<Mutex<bool>>,
    }

    impl BlockingBackend {
        fn new() -> (Self, FlagPair, FlagPair) {
            let entered = Arc::new((Mutex::new(false), Condvar::new()));
            let release = Arc::new((Mutex::new(false), Condvar::new()));
            (
                Self {
                    entered: Arc::clone(&entered),
                    release: Arc::clone(&release),
                    connected: Arc::new(Mutex::new(true)),
                },
                entered,
                release,
            )
        }

        fn set_flag(flag: &Arc<(Mutex<bool>, Condvar)>) {
            let (mutex, condvar) = &**flag;
            *mutex.lock().expect("flag lock poisoned") = true;
            condvar.notify_all();
        }

        fn wait_flag(flag: &Arc<(Mutex<bool>, Condvar)>, timeout: Duration) -> bool {
            let (mutex, condvar) = &**flag;
            let guard = mutex.lock().expect("flag lock poisoned");
            if *guard {
                return true;
            }
            let (guard, _timed_out) = condvar
                .wait_timeout(guard, timeout)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard
        }
    }

    impl IJoystickOutput for BlockingBackend {
        fn name(&self) -> &'static str {
            "BlockingBackend"
        }

        fn is_connected(&self) -> bool {
            *self.connected.lock().expect("connected lock poisoned")
        }

        fn try_connect(&mut self) -> Result<(), VJoyError> {
            *self.connected.lock().expect("connected lock poisoned") = true;
            Ok(())
        }

        fn disconnect(&mut self) {
            *self.connected.lock().expect("connected lock poisoned") = false;
        }

        fn update(&mut self, _state: &ProcessedState) -> Result<(), VJoyError> {
            if !self.is_connected() {
                return Err(VJoyError::NotConnected);
            }
            Self::set_flag(&self.entered);
            if !Self::wait_flag(&self.release, Duration::from_secs(10)) {
                return Err(VJoyError::UpdateFailed(1));
            }
            Ok(())
        }

        fn send_neutral(&mut self) -> Result<(), VJoyError> {
            Ok(())
        }
    }

    fn test_dir(name: &str) -> PathBuf {
        // Unique per call: tests run in parallel and must never share a
        // store directory (a shared dir races `remove_dir_all` against
        // another test's live store).
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "rsh-g0-state-test-{}-{name}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create test dir");
        dir
    }

    fn test_state() -> (Arc<AppStateCore>, PathBuf) {
        let dir = test_dir("main");
        let store = ProfileStore::open(&dir).expect("open test store");
        (Arc::new(AppStateCore::with_null_sink(store)), dir)
    }

    /// State whose persisted `output_backend` setting is `backend`
    /// (e.g. `"VJoy"`), for startup-activation tests of a non-default
    /// choice. The store is mutated before the state is built, so the
    /// state reads the choice exactly as a real launch would.
    fn test_state_with_backend_setting(backend: &str) -> (Arc<AppStateCore>, PathBuf) {
        let dir = test_dir("backend-setting");
        let mut store = ProfileStore::open(&dir).expect("open test store");
        let mut settings = store.settings().clone();
        settings.output_backend = backend.to_string();
        store.set_settings(settings);
        (Arc::new(AppStateCore::with_null_sink(store)), dir)
    }

    #[allow(clippy::type_complexity)]
    fn test_state_with_recording() -> (
        Arc<AppStateCore>,
        Arc<Mutex<Vec<PipelineOutputState>>>,
        Arc<Mutex<bool>>,
    ) {
        let dir = test_dir("recording");
        let store = ProfileStore::open(&dir).expect("open test store");
        let (backend, sent, fail) = RecordingBackend::new();
        let state = Arc::new(AppStateCore::new(
            store,
            Box::new(backend),
            "test recording backend",
        ));
        (state, sent, fail)
    }

    fn test_frame(steering_deg: f64) -> FrameInput {
        FrameInput {
            steering_deg,
            pedals: PedalSnapshot {
                accel: 0.5,
                brake: 0.0,
                clutch: None,
            },
            encoder_counts: [0; MAX_ENCODERS],
            n_enc: 0,
            output: PipelineOutputState::neutral(),
            metrics: EngineMetrics::new(),
        }
    }

    fn activate_first_profile(state: &AppStateCore) -> String {
        let first = state
            .list_profiles()
            .into_iter()
            .next()
            .expect("default profile exists");
        let activated = state
            .activate_profile(&first.id)
            .expect("activate default profile");
        assert!(activated.active);
        activated.id
    }

    #[test]
    fn null_sink_state_reports_not_connected_and_not_sent() {
        let (state, _dir) = test_state();
        assert_eq!(state.backend_name(), "NullSink");
        let initial = state.output_state();
        assert_eq!(initial.backend, "NullSink");
        assert!(!initial.connected);
        assert_eq!(initial.enabled, Some(true)); // settings default
        assert!(initial.detail.contains("no output backend installed"));

        // Disabling with no device connected sends nothing, and says
        // so honestly.
        let disabled = state.set_output_enabled(false);
        assert_eq!(disabled.enabled, Some(false));
        assert!(!disabled.connected);
        assert!(
            disabled.detail.contains("not connected"),
            "detail: {}",
            disabled.detail
        );
        // Enabling attempts a connect, which fails honestly — nothing
        // is delivered. (Hotfix v1.0.1: the enable now activates the
        // selected backend through the selector instead of only
        // `try_connect`-ing the NullSink, so the failure names the
        // backend activation, not a bare connect.)
        let enabled = state.set_output_enabled(true);
        assert_eq!(enabled.enabled, Some(true));
        assert!(!enabled.connected);
        assert!(
            enabled.detail.contains("backend activation failed"),
            "detail: {}",
            enabled.detail
        );
    }

    #[test]
    fn recording_backend_actuation_is_real() {
        let (state, sent, fail) = test_state_with_recording();
        // Enable acquires the device and sends the last output (neutral).
        let enabled = state.set_output_enabled(true);
        assert!(enabled.connected);
        assert_eq!(sent.lock().expect("sent lock").len(), 1);
        assert_eq!(
            *sent.lock().expect("sent lock").last().unwrap(),
            PipelineOutputState::neutral()
        );
        // Ingest a deflected output, re-enable: the last output is resent.
        let mut last = PipelineOutputState::neutral();
        last.steering = 0.5;
        let _ = state.ingest_frame(FrameInput {
            output: last,
            ..test_frame(45.0)
        });
        let _ = state.set_output_enabled(true);
        let sent_states = sent.lock().expect("sent lock").clone();
        assert_eq!(sent_states.len(), 2);
        assert_eq!(sent_states[1], last);
        // Disable → neutral reaches the backend, device released.
        let disabled = state.set_output_enabled(false);
        assert!(!disabled.connected);
        assert_eq!(disabled.enabled, Some(false));
        let sent_states = sent.lock().expect("sent lock").clone();
        assert_eq!(sent_states.len(), 3);
        assert_eq!(sent_states[2], PipelineOutputState::neutral());
        // A failing update is reported, not hidden (the connect still
        // succeeds, so the backend stays connected).
        *fail.lock().expect("fail lock") = true;
        let enabled = state.set_output_enabled(true);
        assert!(enabled.connected);
        assert!(
            enabled.detail.contains("update failed"),
            "detail: {}",
            enabled.detail
        );
    }

    #[test]
    fn shutdown_neutral_is_attempted_and_honestly_reported() {
        // Recording backend, connected: neutral IS sent, stop was not
        // signalled (no engine task registered — the None path), and
        // the device is released afterwards.
        let (state, sent, _fail) = test_state_with_recording();
        let _ = state.set_output_enabled(true); // connect
        let outcome = state.shutdown();
        assert!(!outcome.stop_signalled);
        assert!(outcome.neutral_sent);
        assert_eq!(
            *sent.lock().expect("sent lock").last().unwrap(),
            PipelineOutputState::neutral()
        );
        assert!(!state.output_state().connected);
        // Recording backend, never connected: neutral NOT sent — there
        // is no device to send it to, and the trait's disconnected
        // no-op must not be reported as a delivery.
        let (cold_state, _sent, _fail) = test_state_with_recording();
        let outcome = cold_state.shutdown();
        assert!(!outcome.stop_signalled);
        assert!(!outcome.neutral_sent);
        // NullSink: neutral attempted, honestly NOT sent, no panic.
        let (null_state, _dir) = test_state();
        let outcome = null_state.shutdown();
        assert!(!outcome.stop_signalled);
        assert!(!outcome.neutral_sent);
        assert_eq!(null_state.output_state().enabled, Some(false));
    }

    #[test]
    fn set_output_backend_rejects_unknown_names_and_persists_none() {
        let (state, _dir) = test_state();
        let error = state
            .set_output_backend("Sideways")
            .expect_err("unknown backend must error");
        assert!(error.contains("unknown output backend"), "{error}");
        // "none" is a real selection: it succeeds and is persisted.
        let switched = state
            .set_output_backend("none")
            .expect("none is selectable");
        assert_eq!(switched.backend, "NullSink");
        assert_eq!(state.settings().output_backend, "None");
        assert_eq!(state.selected_backend(), BackendKind::None);
    }

    #[test]
    fn set_output_backend_without_drivers_fails_honestly_and_keeps_the_old_backend() {
        let (state, _dir) = test_state();
        // Linux sandbox: no HIDMaestro sidecar, no vJoy DLL. An
        // explicit HIDMaestro switch must ERROR with the real
        // reason — never silently fall back to vJoy (Bug 4/8) — and
        // the persisted choice reverts to the previous backend.
        let error = state
            .set_output_backend("HIDMaestro")
            .expect_err("HIDMaestro must fail without a sidecar");
        assert!(
            error.contains("could not switch output backend to HIDMaestro"),
            "{error}"
        );
        assert!(
            !error.contains("fallback"),
            "an explicit switch never falls back: {error}"
        );
        // The failed switch reverts the persisted choice to the
        // previous backend (vJoy here), instead of leaving
        // HIDMaestro persisted for the next launch to retry.
        assert_eq!(state.settings().output_backend, "VJoy");
        // …and the failed switch never moved the in-memory selection
        // off the previous choice (the vJoy default here).
        assert_eq!(state.selected_backend(), BackendKind::VJoy);
        // …but the live backend is untouched: still the sink, still
        // disconnected.
        assert_eq!(state.backend_name(), "NullSink");
        assert!(!state.output_state().connected);
        let error = state
            .set_output_backend("vJoy")
            .expect_err("vJoy must fail without a driver");
        assert!(
            error.contains("could not switch output backend to vJoy"),
            "{error}"
        );
        assert_eq!(state.backend_name(), "NullSink");
        // A failed vJoy switch reverts the persisted choice too.
        assert_eq!(state.settings().output_backend, "VJoy");
    }

    /// Bug 4/8: an explicit `HIDMaestro` switch whose connect fails
    /// (non-UAC reason) errors with the real reason, leaves vJoy
    /// live and connected, and reverts the persisted choice to vJoy
    /// — no silent fallback, no "now vJoy" success.
    #[test]
    fn explicit_hidmaestro_switch_failure_reverts_choice_and_keeps_vjoy() {
        let factories = named_mock_factories(Some("HIDMaestro sidecar failed to start."), None);
        let (state, _dir) = test_state();
        state
            .apply_backend_switch(BackendKind::VJoy, &factories, true)
            .expect("install vJoy mock");
        assert_eq!(state.backend_name(), "vJoy");
        assert!(state.output_state().connected);

        let error = state
            .set_output_backend_with_factories("HIDMaestro", &factories)
            .expect_err("explicit HIDMaestro failure must error");
        assert!(
            error.contains("sidecar failed to start"),
            "the real reason is reported: {error}"
        );
        assert!(!error.contains("fell back"), "no silent fallback: {error}");
        assert_eq!(state.backend_name(), "vJoy", "vJoy stays live");
        assert!(state.output_state().connected, "vJoy stays connected");
        assert_eq!(state.selected_backend(), BackendKind::VJoy);
        assert_eq!(
            state.settings().output_backend,
            "VJoy",
            "persisted choice reverts to the previous backend"
        );
    }

    /// Bug 4/8 (UAC decline, regression guard): a `HIDMaestro`
    /// connect failure naming "administrator rights" fails with
    /// [`AppStateCore::HID_NOT_ADMIN`], latches the decline, and
    /// reverts the persisted choice — no silent fallback to vJoy.
    #[test]
    fn explicit_hidmaestro_switch_uac_decline_returns_hid_not_admin() {
        let factories = named_mock_factories(
            Some("administrator rights are required to install the driver"),
            None,
        );
        let (state, _dir) = test_state();
        state
            .apply_backend_switch(BackendKind::VJoy, &factories, true)
            .expect("install vJoy mock");

        let error = state
            .set_output_backend_with_factories("HIDMaestro", &factories)
            .expect_err("a UAC decline must error");
        assert!(
            error.contains(AppStateCore::HID_NOT_ADMIN),
            "the decline is typed, not generic: {error}"
        );
        assert!(
            state.elevation_declined.load(Ordering::SeqCst),
            "the decline latch is set"
        );
        assert_eq!(state.backend_name(), "vJoy", "vJoy stays live");
        assert_eq!(state.selected_backend(), BackendKind::VJoy);
        assert_eq!(
            state.settings().output_backend,
            "VJoy",
            "persisted choice reverts to the previous backend"
        );
    }

    /// The fallback survives ONLY on automatic paths: enabling
    /// output with a persisted `HIDMaestro` choice whose sidecar fails
    /// still falls back to vJoy (the deferred startup activation),
    /// and the persisted choice stays `HIDMaestro` for the next launch.
    #[test]
    fn automatic_deferred_enable_still_falls_back_to_vjoy() {
        let factories = named_mock_factories(Some("HIDMaestro sidecar failed to start."), None);
        let (state, _dir) = test_state_with_backend_setting("HIDMaestro");
        assert_eq!(state.selected_backend(), BackendKind::HidMaestro);

        let switched = state.set_output_enabled_with_factories(true, &factories);
        assert_eq!(switched.backend, "vJoy", "automatic path falls back");
        assert!(switched.connected, "fallback vJoy is connected");
        assert_eq!(state.backend_name(), "vJoy");
        assert_eq!(
            state.settings().output_backend,
            "HIDMaestro",
            "the automatic path does not rewrite the persisted choice"
        );
    }

    #[test]
    fn list_output_backends_reports_honest_status_on_linux() {
        let (state, _dir) = test_state();
        let backends = state.list_output_backends();
        assert_eq!(backends.len(), 3);
        let hidmaestro = &backends[0];
        assert_eq!(hidmaestro.id, "hidmaestro");
        assert_eq!(hidmaestro.name, "HIDMaestro");
        assert!(!hidmaestro.is_default, "vJoy is the default since v2.0.1");
        assert!(!hidmaestro.selected, "settings default is vJoy");
        assert!(hidmaestro.elevation_required);
        assert!(
            hidmaestro
                .uac_note
                .as_deref()
                .is_some_and(|note| note.contains("UAC")),
            "UAC note is present for HIDMaestro"
        );
        assert_eq!(
            hidmaestro.installed,
            Some(false),
            "no sidecar in the sandbox"
        );
        assert!(!hidmaestro.active);
        assert!(!hidmaestro.connected);
        assert_eq!(hidmaestro.download_url, None);
        let vjoy = &backends[1];
        assert_eq!(vjoy.id, "vjoy");
        assert!(vjoy.is_default, "vJoy is the default since v2.0.1");
        assert!(vjoy.selected, "settings default is vJoy");
        assert_eq!(vjoy.installed, Some(false), "no vJoy DLL in the sandbox");
        assert_eq!(vjoy.download_url.as_deref(), Some(VJOY_DOWNLOAD_URL));
        assert!(!vjoy.elevation_required);
        assert_eq!(vjoy.uac_note, None);
        let none = &backends[2];
        assert_eq!(none.id, "none");
        assert!(
            none.active,
            "the sink is live until a real backend is selected"
        );
        assert_eq!(none.installed, Some(true));
        assert!(!none.connected);
        assert!(!none.elevation_required);
    }

    #[test]
    fn output_state_reports_the_uac_note_for_hidmaestro() {
        // The detail string is built from the live kind, so even the
        // sink state documents the rule; the live HIDMaestro detail
        // (after a real switch on Windows) carries the same note.
        assert!(live_backend_detail(BackendKind::HidMaestro, 1).contains("UAC"));
        assert!(live_backend_detail(BackendKind::HidMaestro, 1).contains("every launch"));
        // vJoy names the absence of a prompt, not the presence of one.
        assert!(live_backend_detail(BackendKind::VJoy, 1).contains("no UAC prompt"));
        // …and vJoy is the default since v2.0.1, HIDMaestro the
        // explicit choice.
        assert!(live_backend_detail(BackendKind::VJoy, 1).contains("default backend"));
        assert!(!live_backend_detail(BackendKind::HidMaestro, 1).contains("default backend"));
    }

    // -- v2.0.1 hotfix BACKEND-2: default backend vJoy ----------------

    #[test]
    fn fresh_store_defaults_to_vjoy_backend() {
        let (state, _dir) = test_state();
        assert_eq!(state.selected_backend(), BackendKind::VJoy);
        assert_eq!(state.settings().output_backend, "VJoy");
        // Linux sandbox: no vJoy DLL, so no migration ran here.
        assert_eq!(state.backend_migration_note(), None);
    }

    #[test]
    fn persisted_backend_migration_decision() {
        // Only a persisted HIDMaestro WITH vJoy installed migrates.
        assert!(should_migrate_persisted_backend("HIDMaestro", true));
        assert!(!should_migrate_persisted_backend("HIDMaestro", false));
        assert!(!should_migrate_persisted_backend("VJoy", true));
        assert!(!should_migrate_persisted_backend("None", true));
        assert!(!should_migrate_persisted_backend("Sideways", true));
    }

    #[test]
    #[cfg(not(windows))]
    fn vjoy_status_reports_not_installed_on_linux() {
        let (state, _dir) = test_state();
        let status = state.vjoy_status();
        assert!(!status.installed, "no vJoy DLL in the sandbox");
        assert!(!status.installing);
        assert_eq!(status.install_error, None);
        assert_eq!(status.dll_path, None);
        assert_eq!(status.version, None, "version is never invented");
    }

    #[test]
    fn vjoy_dll_probe_is_cached_and_invalidation_reprobes() {
        // R2-20: the first status read probes and populates the
        // cache; later reads reuse it; invalidation clears it so
        // the next read probes again.
        let (state, _dir) = test_state();
        assert!(lock(&state.vjoy_dll_cache).probed_at.is_none());
        let first = state.vjoy_status();
        assert!(lock(&state.vjoy_dll_cache).probed_at.is_some());
        let probed_at = lock(&state.vjoy_dll_cache).probed_at;
        let second = state.vjoy_status();
        assert_eq!(lock(&state.vjoy_dll_cache).probed_at, probed_at);
        assert_eq!(first.installed, second.installed);
        assert_eq!(first.dll_path, second.dll_path);
        state.invalidate_vjoy_dll_cache();
        assert!(lock(&state.vjoy_dll_cache).probed_at.is_none());
        let third = state.vjoy_status();
        assert!(lock(&state.vjoy_dll_cache).probed_at.is_some());
        assert_eq!(first.installed, third.installed);
        assert_eq!(first.dll_path, third.dll_path);
    }

    #[test]
    fn backend_switch_invalidates_the_vjoy_dll_probe() {
        // R2-20: a backend-switch job re-probes on the next status
        // read instead of trusting the cached probe.
        let (state, _dir) = test_state();
        let _ = state.vjoy_status();
        assert!(lock(&state.vjoy_dll_cache).probed_at.is_some());
        state
            .set_output_backend("none")
            .expect("none is selectable");
        assert!(
            lock(&state.vjoy_dll_cache).probed_at.is_none(),
            "backend switch must clear the cached probe"
        );
    }

    #[test]
    #[cfg(not(windows))]
    fn install_vjoy_is_windows_only_on_linux() {
        let (state, _dir) = test_state();
        let error = state
            .install_vjoy()
            .expect_err("vJoy install must refuse on Linux");
        assert!(error.contains("Windows-only"), "{error}");
        // The refusal leaves no install running.
        assert!(!state.vjoy_status().installing);
    }

    #[test]
    #[cfg(unix)]
    fn wait_child_timeout_kills_a_hung_child() {
        // DS-40: the installer wait is platform-free (std only),
        // so it is covered here with /bin/sleep instead of the
        // real installer.
        let child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");
        let start = Instant::now();
        let error = wait_child_timeout(child, Duration::from_millis(200), "vJoy installer")
            .expect_err("a 30 s sleep must time out at 200 ms");
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "timeout must bound the wait: {:?}",
            start.elapsed()
        );
        assert!(error.contains("timed out"), "{error}");
        // And a fast child still succeeds with its status.
        let child = std::process::Command::new("/bin/true")
            .spawn()
            .expect("spawn true");
        let status = wait_child_timeout(child, Duration::from_secs(5), "vJoy installer")
            .expect("true must succeed");
        assert!(status.success());
    }

    #[test]
    fn diag_stats_reports_measured_values_not_constants() {
        let (state, _dir) = test_state();
        // No ticks yet: rate unmeasured, loss a real 0 %, counters 0.
        let diag = state.diag_stats();
        assert_eq!(diag.loop_hz, None);
        assert_eq!(diag.loss_pct, 0.0);
        assert_eq!(diag.jitter_ms_p99, None);
        assert_eq!(diag.seq_gap, 0);
        assert_eq!(diag.tx_fail, 0);
        // One tick: still unmeasured rate, counters still the
        // frame's (zero) values — never a fake 200 Hz.
        let _ = state.ingest_frame(test_frame(0.0));
        let diag = state.diag_stats();
        assert_eq!(diag.loop_hz, None);
        assert_eq!(diag.loss_pct, 0.0);
        assert_eq!(diag.seq_gap, 0);
        assert_eq!(diag.tx_fail, 0);
    }

    #[test]
    fn ingest_builds_snapshots_with_measured_loop_rate_and_encoders() {
        let (state, _dir) = test_state();
        // No ticks yet: stats unmeasured, snapshot is the default.
        let measured = state.engine_stats();
        assert_eq!(measured.loop_hz, None);
        assert_eq!(measured.loss_pct, 0.0);
        assert_eq!(state.snapshot().seq, 0);

        // Two ticks: the loop rate is measured from real tick spacing.
        let first = state.ingest_frame(test_frame(0.0));
        assert_eq!(first.seq, 1);
        assert_eq!(first.stats.loop_hz, None); // one tick cannot define a rate
        let second = state.ingest_frame(FrameInput {
            encoder_counts: [10, 0, 0, 0],
            n_enc: 1,
            ..test_frame(12.5)
        });
        assert_eq!(second.seq, 2);
        assert_eq!(second.stats.loop_hz, Some(2.0));
        assert_eq!(second.steering_deg, 12.5);
        assert_eq!(second.encoders.len(), 1);
        assert_eq!(second.encoders[0].count, 10);
        assert_eq!(second.encoders[0].delta, 0); // first sample: baseline
        // Third tick: encoder +3 → CW press, virtual button 65.
        let third = state.ingest_frame(FrameInput {
            encoder_counts: [13, 0, 0, 0],
            n_enc: 1,
            ..test_frame(15.0)
        });
        assert_eq!(third.seq, 3);
        assert_eq!(third.encoders[0].delta, 3);
        assert!(third.encoders[0].cw_pressed);
        assert_eq!(third.encoder_buttons, vec![65]);
        assert_eq!(state.snapshot(), third);
    }

    #[test]
    fn button_press_edges_come_from_pipeline_diffs() {
        let (state, _dir) = test_state();
        let mut pressed = PipelineOutputState::neutral();
        pressed.buttons = 0b101; // buttons 1 and 3
        let _ = state.ingest_frame(FrameInput {
            output: pressed,
            ..test_frame(0.0)
        });
        assert_eq!(
            state.drain_button_presses(),
            vec![
                ButtonPressEvent {
                    button: 1,
                    pressed: true
                },
                ButtonPressEvent {
                    button: 3,
                    pressed: true
                },
            ]
        );
        // Release button 1 only: exactly one release edge.
        let mut released = PipelineOutputState::neutral();
        released.buttons = 0b100;
        let _ = state.ingest_frame(FrameInput {
            output: released,
            ..test_frame(0.0)
        });
        assert_eq!(
            state.drain_button_presses(),
            vec![ButtonPressEvent {
                button: 1,
                pressed: false
            }]
        );
        // No change: no events.
        let _ = state.ingest_frame(FrameInput {
            output: released,
            ..test_frame(0.0)
        });
        assert_eq!(state.drain_button_presses(), Vec::<ButtonPressEvent>::new());
        // Encoder pulse: momentary press edge only (65, true). The
        // output stays `released` so no physical-button edge sneaks in.
        let _ = state.ingest_frame(FrameInput {
            encoder_counts: [0, 0, 0, 0],
            n_enc: 1,
            output: released,
            ..test_frame(0.0)
        });
        assert_eq!(state.drain_button_presses(), Vec::<ButtonPressEvent>::new());
        let _ = state.ingest_frame(FrameInput {
            encoder_counts: [2, 0, 0, 0],
            n_enc: 1,
            output: released,
            ..test_frame(0.0)
        });
        assert_eq!(
            state.drain_button_presses(),
            vec![ButtonPressEvent {
                button: 65,
                pressed: true
            }]
        );
    }

    #[test]
    fn snapshot_emitter_runs_at_30hz_without_blocking_ingest() {
        let (state, _dir) = test_state();
        let count = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let emitter_state = Arc::clone(&state);
        let emitter_count = Arc::clone(&count);
        let emitter_stop = Arc::clone(&stop);
        let emitter = std::thread::spawn(move || {
            AppStateCore::run_snapshot_emitter(
                &emitter_state,
                |_| {
                    emitter_count.fetch_add(1, Ordering::Relaxed);
                },
                &emitter_stop,
            );
        });
        // 200 engine ticks while the emitter runs: ingest must stay fast.
        let ingest_start = Instant::now();
        for _ in 0..200 {
            let _ = state.ingest_frame(test_frame(0.0));
        }
        let ingest_elapsed = ingest_start.elapsed();
        // Wait for ~24 snapshots instead of a fixed 1 s sleep: on a
        // loaded runner the emitter can fall behind the 24..=38
        // window in a fixed window; reaching 24 proves the ~30 Hz
        // rate, and stopping right after keeps the <= 38 bound
        // meaningful.
        assert!(
            wait_until(Duration::from_secs(5), || count.load(Ordering::Relaxed)
                >= 24),
            "emitter stalled: only {} snapshots in 5 s",
            count.load(Ordering::Relaxed)
        );
        stop.store(true, Ordering::Relaxed);
        emitter.join().expect("emitter thread joins");
        let snapshot_count = count.load(Ordering::Relaxed);
        assert!(
            (24..=38).contains(&snapshot_count),
            "emitted {snapshot_count} snapshots in ~1 s, expected ~30"
        );
        assert!(
            ingest_elapsed < Duration::from_millis(500),
            "200 ingests took {ingest_elapsed:?} while emitting — engine blocked on UI"
        );
    }

    #[test]
    fn snapshot_emitter_forwards_button_press_events() {
        let (state, _dir) = test_state();
        let events = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let emitter_state = Arc::clone(&state);
        let emitter_events = Arc::clone(&events);
        let emitter_stop = Arc::clone(&stop);
        let emitter = std::thread::spawn(move || {
            AppStateCore::run_snapshot_emitter(
                &emitter_state,
                |emit| {
                    if let SnapshotEmit::ButtonPress(event) = emit {
                        emitter_events.lock().expect("events lock").push(event);
                    }
                },
                &emitter_stop,
            );
        });
        let mut output = PipelineOutputState::neutral();
        output.buttons = 0b10; // button 2
        let _ = state.ingest_frame(FrameInput {
            output,
            ..test_frame(0.0)
        });
        std::thread::sleep(Duration::from_millis(200));
        stop.store(true, Ordering::Relaxed);
        emitter.join().expect("emitter thread joins");
        let collected = events.lock().expect("events lock").clone();
        assert_eq!(
            collected,
            vec![ButtonPressEvent {
                button: 2,
                pressed: true
            }]
        );
    }

    #[test]
    fn profiles_crud_round_trip_through_state() {
        let (state, dir) = test_state();
        assert_eq!(state.list_profiles().len(), 1); // the Default profile
        assert_eq!(state.active_profile(), None); // nothing active yet
        let created = state.create_profile("Track Day").expect("create");
        assert_eq!(created.name, "Track Day");
        assert_eq!(state.list_profiles().len(), 2);
        let activated = state
            .activate_profile(&created.id)
            .expect("activate created");
        assert!(activated.active);
        assert_eq!(state.active_profile().expect("active").id, created.id);
        let copy = state
            .duplicate_profile(&created.id)
            .expect("duplicate created");
        assert_ne!(copy.id, created.id);
        assert_eq!(state.list_profiles().len(), 3);
        state.delete_profile(&copy.id).expect("delete copy");
        assert_eq!(state.list_profiles().len(), 2);
        // Unknown ids fail honestly, never silently.
        assert!(state.activate_profile("nope").is_err());
        assert!(state.delete_profile("nope").is_err());
        // Mutations persisted: a fresh store over the same dir sees them.
        let reopened = ProfileStore::open(&dir).expect("reopen store");
        assert_eq!(reopened.list().len(), 2);
        assert_eq!(
            reopened.active().expect("active after reopen").id,
            created.id
        );
        // Import/export round-trip through real files.
        let export_path = dir.join("exported-profile.json");
        state
            .export_profile(&created.id, export_path.to_str().expect("utf8 path"))
            .expect("export");
        assert!(export_path.exists());
        let imported = state
            .import_profile(export_path.to_str().expect("utf8 path"))
            .expect("import");
        // The store dedups names on import: "Track Day" already exists,
        // so the import lands as "Track Day (2)".
        assert_eq!(imported.name, "Track Day (2)");
        assert_eq!(state.list_profiles().len(), 3);
    }

    #[test]
    fn settings_round_trip_and_output_flag_actuates() {
        let (state, _sent, _fail) = test_state_with_recording();
        let mut settings = state.settings();
        assert!(settings.output_enabled);
        settings.theme = "Dark".to_string();
        settings.output_enabled = false;
        let saved = state.set_settings(settings).expect("set settings");
        assert_eq!(saved.theme, "Dark");
        assert_eq!(state.settings().theme, "Dark");
        // Flipping the flag actuated the backend (neutral sent).
        assert_eq!(state.output_state().enabled, Some(false));
    }

    #[test]
    fn backups_and_restore_round_trip() {
        let (state, dir) = test_state();
        let created = state.create_profile("Backup Me").expect("create");
        let backups = state.backup_now().expect("backup now");
        // M5: 3 generations per file — profiles first, then settings.
        assert_eq!(backups.len(), 6);
        // `backup_now` rotates the live file into .bak1 and leaves no
        // live file behind until the next save.
        assert!(!backups[0].present);
        assert!(backups[1].present);
        // Delete the profile (this save rotates .bak1 into .bak2), then
        // restore the generation that still holds it.
        state.delete_profile(&created.id).expect("delete");
        assert_eq!(state.list_profiles().len(), 1);
        let bak2 = dir.join("profiles.json.bak2");
        assert!(bak2.exists());
        state
            .restore_backup(bak2.to_str().expect("utf8 path"))
            .expect("restore bak2");
        assert_eq!(state.list_profiles().len(), 2);
        assert!(state.restore_backup("/tmp/not-a-backup.txt").is_err());
    }

    #[test]
    fn manual_games_list_add_update_remove() {
        let (state, _dir) = test_state();
        // No detection: a fresh store lists nothing.
        assert_eq!(state.list_games().len(), 0);
        assert!(state.get_game_state("no-such-game").is_err());
        // Add two games; ids come from the names.
        let first = state
            .add_manual_game("My Racing Game", r"C:\Games\race.exe", None)
            .expect("add");
        assert_eq!(first.id, "my-racing-game");
        assert!(!first.installed); // Linux sandbox: no such exe
        assert!(!first.running);
        let second = state
            .add_manual_game("My Racing Game", r"C:\Games\race2.exe", None)
            .expect("add duplicate name");
        assert_eq!(second.id, "my-racing-game-2");
        assert_eq!(state.list_games().len(), 2);
        // Unknown ids error honestly, and a not-installed game cannot launch.
        let error = state
            .launch_game("my-racing-game")
            .expect_err("launch of a not-installed game must fail");
        assert!(error.contains("not installed"), "{error}");
        assert!(state.stop_game("my-racing-game").is_err());
        // Update keeps the stable id; remove drops the entry.
        let updated = state
            .update_manual_game(
                "my-racing-game",
                "My Racing Game 2",
                r"C:\Games\race.exe",
                None,
            )
            .expect("update");
        assert_eq!(updated.id, "my-racing-game");
        assert_eq!(updated.name, "My Racing Game 2");
        assert!(
            state
                .remove_manual_game("my-racing-game-2")
                .expect("remove")
        );
        assert_eq!(state.list_games().len(), 1);
        assert!(state.remove_manual_game("no-such-game").is_err());
        // Empty names / paths are rejected, never stored.
        assert!(state.add_manual_game("", r"C:\a.exe", None).is_err());
        assert!(state.add_manual_game("Race", "", None).is_err());
        assert_eq!(state.list_games().len(), 1);
    }

    #[test]
    fn manual_game_profile_binding_validates() {
        let (state, _dir) = test_state();
        let profile = state
            .create_profile("Race Profile")
            .expect("create profile");
        let game = state
            .add_manual_game("Race", r"C:\race.exe", Some(profile.id.clone()))
            .expect("add with profile");
        assert_eq!(game.bound_profile_id.as_deref(), Some(profile.id.as_str()));
        // An unknown profile id is rejected at add time.
        assert!(
            state
                .add_manual_game(
                    "Race 2",
                    r"C:\race2.exe",
                    Some("no-such-profile".to_string())
                )
                .is_err()
        );
        // link_profile also binds a manual entry by id.
        let other = state.create_profile("Other").expect("create other");
        let summary = state
            .link_profile(&other.id, &game.id)
            .expect("link manual game");
        assert_eq!(summary.id, other.id);
        let bound = state.get_game_state(&game.id).expect("game state");
        assert_eq!(bound.bound_profile_id.as_deref(), Some(other.id.as_str()));
        assert!(state.link_profile(&other.id, "no-such-game").is_err());
    }

    #[test]
    fn tuning_validates_stores_and_falls_back_to_neutral() {
        let (state, _dir) = test_state();
        // Neutral default for a module with no tuning anywhere.
        let neutral = state.get_tuning("steering");
        assert_eq!(neutral.deadzone, 0.0);
        assert_eq!(neutral.curve, 1.0);
        assert_eq!(neutral.max_mv, 3300.0);
        // Invalid tuning is rejected, never stored.
        let mut bad = neutral.clone();
        bad.module = "brake".to_string();
        bad.deadzone = 99.0;
        assert!(state.set_tuning(&bad).is_err());
        assert_eq!(state.get_tuning("brake").deadzone, 0.0);
        // Valid tuning stores, survives a get, and lands in the draft
        // (L4) — NOT in the stored profile: a fresh store over the
        // same dir still sees the old tuning until Save.
        let _ = activate_first_profile(&state);
        let mut tuning = neutral.clone();
        tuning.module = "brake".to_string();
        tuning.deadzone = 5.0;
        tuning.curve = 1.2;
        tuning.bite_point = Some(0.35);
        let saved = state.set_tuning(&tuning).expect("valid tuning");
        assert_eq!(saved.bite_point, Some(0.35));
        assert_eq!(state.get_tuning("brake").deadzone, 5.0);
        assert_eq!(state.get_tuning("brake").bite_point, Some(0.35));
        assert!(state.draft_dirty());
        let stored_deadzone = lock(&state.store).active().and_then(|active| {
            active
                .profile
                .module_tunings
                .get("brake")
                .map(|entry| entry.deadzone)
        });
        assert_eq!(
            stored_deadzone, None,
            "draft only: disk is untouched before Save"
        );
    }

    #[test]
    fn encoder_tuning_round_trips_and_old_profiles_get_defaults() {
        let (state, _dir) = test_state();
        let _ = activate_first_profile(&state);
        // No stored encoder entry yet: the defaults apply (50 ms
        // pulse, 4 counts per click) — an old profile without the
        // fields loads the same way.
        let neutral = state.get_tuning("encoders");
        assert_eq!(neutral.encoder_pulse_ms, Some(50.0));
        assert_eq!(neutral.encoder_counts_per_click, Some(4));
        // Invalid values are rejected, never stored.
        let mut bad = neutral.clone();
        bad.encoder_pulse_ms = Some(10.0);
        assert!(state.set_tuning(&bad).is_err());
        let mut bad = neutral.clone();
        bad.encoder_counts_per_click = Some(3);
        assert!(state.set_tuning(&bad).is_err());
        assert_eq!(state.get_tuning("encoders").encoder_pulse_ms, Some(50.0));
        // Valid values store, survive a get, and reach the live
        // pipeline plan.
        let mut tuning = neutral.clone();
        tuning.encoder_pulse_ms = Some(120.0);
        tuning.encoder_counts_per_click = Some(2);
        let saved = state.set_tuning(&tuning).expect("valid encoder tuning");
        assert_eq!(saved.encoder_pulse_ms, Some(120.0));
        assert_eq!(saved.encoder_counts_per_click, Some(2));
        assert_eq!(state.get_tuning("encoders").encoder_pulse_ms, Some(120.0));
        assert_eq!(
            state.get_tuning("encoders").encoder_counts_per_click,
            Some(2)
        );
        let plan = state.build_pipeline_plan();
        assert_eq!(plan.encoders.pulse_ms, 120.0);
        assert_eq!(plan.encoders.counts_per_click, 2);
        // Save persists them into the profile on disk.
        assert!(state.save_draft().expect("save draft"));
        let stored = lock(&state.store)
            .active()
            .and_then(|active| active.profile.module_tunings.get("encoders").cloned());
        let stored = stored.expect("stored encoder entry");
        assert_eq!(stored.encoder_pulse_ms, Some(120.0));
        assert_eq!(stored.encoder_counts_per_click, Some(2));
    }

    /// L4 draft/save: Save persists the draft (and only the draft),
    /// Discard reverts the live tuning to the stored profile, and a
    /// profile switch while dirty is refused until Save/Discard.
    #[test]
    fn draft_save_persists_discard_reverts_switch_guards() {
        let (state, dir) = test_state();
        let first_id = activate_first_profile(&state);
        // No draft yet: save is a no-op, switch is free.
        assert!(!state.draft_dirty());
        assert!(!state.save_draft().expect("save clean"));
        assert!(state.draft_status().is_none());

        let mut tuning = state.get_tuning("brake");
        tuning.deadzone = 5.0;
        tuning.bite_point = Some(0.4);
        state.set_tuning(&tuning).expect("set tuning");
        let status = state.draft_status().expect("draft exists");
        assert!(status.dirty);
        assert_eq!(status.dirty_modules, vec!["brake"]);
        assert_eq!(status.profile_id, first_id);

        // Switch while dirty is refused…
        let second = state.create_profile("Second").expect("create second");
        assert!(
            state
                .switch_profile(&second.id, false)
                .is_err_and(|error| error.contains("unsaved changes")),
            "dirty switch must name the unsaved changes"
        );
        assert_eq!(
            state.active_profile().expect("active").id,
            first_id,
            "refused switch stays on the old profile"
        );

        // …Discard reverts the live tuning and frees the switch.
        assert!(state.discard_draft());
        assert!(!state.draft_dirty());
        assert_eq!(state.get_tuning("brake").deadzone, 0.0);
        assert_eq!(state.get_tuning("brake").bite_point, None);
        state
            .switch_profile(&second.id, false)
            .expect("switch after discard");

        // Edit + Save: a fresh store over the same dir sees it.
        let mut steering = state.get_tuning("steering");
        steering.deadzone = 7.0;
        state.set_tuning(&steering).expect("set steering");
        assert!(state.save_draft().expect("save draft"));
        assert!(!state.draft_dirty());
        let reopened = ProfileStore::open(&dir).expect("reopen store");
        let stored = reopened
            .active()
            .expect("active after reopen")
            .profile
            .module_tunings
            .get("steering")
            .expect("steering tuning persisted");
        assert!((stored.deadzone - 0.07).abs() < f64::EPSILON);
    }

    /// L4 pipeline push: the built plan carries the draft tuning —
    /// deadzone, curve, ranges, and the clutch bite point — and a
    /// profile switch rebuilds it from the new profile.
    #[test]
    fn pipeline_plan_carries_draft_tuning_and_bite_point() {
        let (state, _dir) = test_state();
        let _ = activate_first_profile(&state);
        // Default plan: clutch disabled, neutral steering.
        let plan = state.build_pipeline_plan();
        assert!(!plan.clutch.enabled);
        assert_eq!(plan.steering.deadzone, 0.0);
        // Draft a clutch tuning with a bite point: the plan picks it
        // up (and enables the clutch) without any Save.
        let mut clutch = state.get_tuning("clutch");
        clutch.deadzone = 2.0;
        clutch.bite_point = Some(0.4);
        state.set_tuning(&clutch).expect("set clutch");
        let plan = state.build_pipeline_plan();
        assert!(plan.clutch.enabled);
        assert!((plan.clutch.deadzone - 0.02).abs() < f32::EPSILON);
        assert!((plan.clutch.bite_point - 0.4).abs() < f32::EPSILON);
        // Steering range + scale come from the profile/tuning.
        let mut steering = state.get_tuning("steering");
        steering.min_mv = 300.0;
        steering.max_mv = 3300.0;
        steering.curve = 1.5;
        state.set_tuning(&steering).expect("set steering");
        let plan = state.build_pipeline_plan();
        assert!((plan.steering.curve_strength - 0.5).abs() < f32::EPSILON);
        assert_eq!((plan.steering.min_mv, plan.steering.max_mv), (300, 3300));
    }

    /// Bug 7 (PC-test fix): steering invert + lock round-trip —
    /// `get_tuning` reports the profile defaults (not inverted,
    /// 900°), a `set_tuning` with invert + 540° lock lands in the
    /// live plan (inverted, scale 0.6), and a Save persists both
    /// fields so a reopened store reads them back.
    #[test]
    fn steering_invert_and_lock_round_trip_through_plan_and_store() {
        let (state, dir) = test_state();
        let _ = activate_first_profile(&state);
        let tuning = state.get_tuning("steering");
        assert_eq!(tuning.invert, Some(false), "default is not inverted");
        assert_eq!(
            tuning.steering_lock_deg,
            Some(900.0),
            "default is full lock"
        );
        let mut steering = tuning;
        steering.invert = Some(true);
        steering.steering_lock_deg = Some(540.0);
        state
            .set_tuning(&steering)
            .expect("set steering invert+lock");
        let plan = state.build_pipeline_plan();
        assert!(plan.steering.invert, "plan inverts");
        assert!(
            (plan.steering.scale - 0.6).abs() < f32::EPSILON,
            "540° → scale 0.6"
        );
        let back = state.get_tuning("steering");
        assert_eq!(back.invert, Some(true));
        assert_eq!(back.steering_lock_deg, Some(540.0));
        assert!(state.save_draft().expect("save draft"));
        let reopened = ProfileStore::open(&dir).expect("reopen store");
        let profile = &reopened.active().expect("active after reopen").profile;
        assert!(profile.invert_steering, "invert persisted");
        assert!(
            (profile.steering_scale - 0.6).abs() < f64::EPSILON,
            "scale persisted"
        );
        // Other modules never carry steering options.
        assert_eq!(state.get_tuning("brake").invert, None);
        assert_eq!(state.get_tuning("brake").steering_lock_deg, None);
    }

    /// L4 learn mode: arming captures the next physical button press
    /// (from the edge feed), disarms, and the pick is consumed once.
    #[test]
    fn learn_mode_captures_next_physical_press_once() {
        let (state, _dir) = test_state();
        let _ = activate_first_profile(&state);
        assert!(!state.is_learning());
        assert_eq!(state.take_learned_button(), None);
        state.start_learn();
        assert!(state.is_learning());
        // A press of button 3 (bit 2) through the edge feed.
        let mut output = PipelineOutputState::neutral();
        output.buttons = 1 << 2;
        state.ingest_engine_output(&output, &EngineMetrics::new());
        assert!(!state.is_learning(), "pick disarms learn mode");
        assert_eq!(state.take_learned_button(), Some(3));
        assert_eq!(state.take_learned_button(), None, "consumed once");
        // Cancel arms off without a pick.
        state.start_learn();
        state.cancel_learn();
        assert!(!state.is_learning());
        assert_eq!(state.take_learned_button(), None);
    }

    // -- Button→action bindings (v2.0.2, Round 2B) --------------------

    /// Drive one engine tick through the sink path (ingest +
    /// backend send) the way [`EngineSink`] does, and return the
    /// state the recording backend received.
    fn drive_tick(
        state: &AppStateCore,
        sent: &Arc<Mutex<Vec<PipelineOutputState>>>,
        output: &PipelineOutputState,
    ) -> PipelineOutputState {
        let remapped = state.ingest_engine_output(output, &EngineMetrics::new());
        state
            .send_engine_output(&remapped)
            .expect("backend send succeeds");
        sent.lock()
            .expect("sent lock")
            .last()
            .copied()
            .expect("backend received a tick")
    }

    /// Game-action bindings remap the tick before the backend sees
    /// it: the source bit is cleared, the target is set — and with
    /// no binding the tick passes through unchanged.
    #[test]
    fn bindings_remap_game_actions_before_backend_send() {
        let (state, sent, _fail) = test_state_with_recording();
        assert!(state.set_output_enabled(true).connected);
        // No bindings yet: button 5 (bit 4) passes through.
        let mut output = PipelineOutputState::neutral();
        output.buttons = 1 << 4;
        let received = drive_tick(&state, &sent, &output);
        assert_eq!(received.buttons, 1 << 4);
        assert!(!received.horn);
        // Bind button 5 → horn, 6 → gear_2, 7 → handbrake.
        state.set_binding(5, "horn").expect("bind 5");
        state.set_binding(6, "gear_2").expect("bind 6");
        state.set_binding(7, "handbrake").expect("bind 7");
        let mut output = PipelineOutputState::neutral();
        output.buttons = (1 << 4) | (1 << 5) | (1 << 6);
        let received = drive_tick(&state, &sent, &output);
        assert_eq!(
            received.buttons,
            1 << 2,
            "only the horn bit (button 3) survives — source bits 4/5/6 are cleared"
        );
        assert!(received.horn, "horn flag follows the bound horn bit");
        assert_eq!(received.gear, 2, "gear_2 sets the decoded gear field");
        assert_eq!(received.handbrake, 1.0, "handbrake is full while held");
        // Release: the next tick is clean again (per-tick override).
        let received = drive_tick(&state, &sent, &PipelineOutputState::neutral());
        assert_eq!(received.buttons, 0);
        assert_eq!(received.gear, 0);
        assert_eq!(received.handbrake, 0.0);
    }

    /// An encoder virtual button (65–72) bound to a game action
    /// fires momentarily: the pulse is cleared and the target is
    /// set for exactly that tick.
    #[test]
    fn bindings_remap_encoder_virtual_button() {
        let (state, sent, _fail) = test_state_with_recording();
        assert!(state.set_output_enabled(true).connected);
        state.set_binding(65, "shifter_up").expect("bind 65");
        let mut output = PipelineOutputState::neutral();
        output.encoder_buttons = EncoderButtons(0b01); // button 65
        let received = drive_tick(&state, &sent, &output);
        assert_eq!(received.buttons, 1 << 0, "shifter_up = bit 0");
        assert_eq!(
            received.encoder_buttons.0, 0,
            "the source pulse is consumed, not forwarded"
        );
    }

    /// App-action bindings queue exactly one action per press edge:
    /// holding the button does not repeat, and a fresh press after
    /// release queues again. The raw button edge still reaches the
    /// UI highlight (learn mode / `drain_button_presses` see the
    /// physical button).
    #[test]
    fn bindings_app_action_queues_once_per_press_edge() {
        let (state, _dir) = test_state();
        state.set_binding(8, "profile_next").expect("bind 8");
        state.set_binding(65, "profile_previous").expect("bind 65");
        let mut output = PipelineOutputState::neutral();
        output.buttons = 1 << 7; // button 8 pressed
        output.encoder_buttons = EncoderButtons(0b01); // button 65 pulse
        state.ingest_engine_output(&output, &EngineMetrics::new());
        assert_eq!(
            state.drain_binding_actions(),
            vec![
                BindingAppAction::ProfileNext,
                BindingAppAction::ProfilePrevious
            ]
        );
        // Held physical button: no repeat. The encoder pulse is
        // absent this tick, so it does not fire again either.
        let mut held = PipelineOutputState::neutral();
        held.buttons = 1 << 7; // button 8 still held, no pulse
        state.ingest_engine_output(&held, &EngineMetrics::new());
        assert_eq!(state.drain_binding_actions().len(), 0);
        // Release, then press again: one more action.
        state.ingest_engine_output(&PipelineOutputState::neutral(), &EngineMetrics::new());
        assert_eq!(state.drain_binding_actions().len(), 0);
        state.ingest_engine_output(&output, &EngineMetrics::new());
        assert_eq!(
            state.drain_binding_actions(),
            vec![
                BindingAppAction::ProfileNext,
                BindingAppAction::ProfilePrevious
            ]
        );
        // The raw edge still reached the button-press feed.
        let events = state.drain_button_presses();
        assert!(
            events
                .iter()
                .any(|event| event.button == 8 && event.pressed),
            "raw edge survives for the UI highlight: {events:?}"
        );
    }

    /// R2-08: an encoder pulse held across two ticks fires the bound
    /// app action exactly once — the encoder buttons are now
    /// edge-detected against the previous tick's encoder bits, like
    /// the physical-button path. (Before the fix the pulse fired on
    /// every tick it was present, so one detent could cycle several
    /// profiles.)
    #[test]
    fn bindings_encoder_app_action_fires_once_per_detent() {
        let (state, _dir) = test_state();
        state.set_binding(65, "profile_previous").expect("bind 65");
        let mut output = PipelineOutputState::neutral();
        output.encoder_buttons = EncoderButtons(0b01); // button 65 pulse
        state.ingest_engine_output(&output, &EngineMetrics::new());
        assert_eq!(
            state.drain_binding_actions(),
            vec![BindingAppAction::ProfilePrevious],
            "first tick fires"
        );
        // Same pulse still present on the next tick: no repeat.
        state.ingest_engine_output(&output, &EngineMetrics::new());
        assert_eq!(
            state.drain_binding_actions().len(),
            0,
            "held encoder pulse must not repeat"
        );
        // Release, then a fresh pulse: fires again.
        state.ingest_engine_output(&PipelineOutputState::neutral(), &EngineMetrics::new());
        state.ingest_engine_output(&output, &EngineMetrics::new());
        assert_eq!(
            state.drain_binding_actions(),
            vec![BindingAppAction::ProfilePrevious],
            "fresh detent after release fires again"
        );
    }

    /// R2-05: a failed control op (LED test, config) records on its
    /// own channel and never knocks the live session into Error —
    /// `session_last_error_typed` (what the manager loop turns into
    /// `ActivationFailed`) stays untouched.
    #[test]
    fn control_error_does_not_touch_session_error() {
        let (state, _dir) = test_state();
        state.note_control_error("led test failed");
        assert_eq!(
            state.control_last_error().as_deref(),
            Some("led test failed")
        );
        assert!(state.session_last_error_typed().is_none());
        assert!(state.session_last_error().is_none());
        assert!(state.session_error_typed().is_none());
        // …and an activation failure still records on the session
        // channel, leaving the control channel alone.
        state.note_session_error_typed(ConnectionError::TransportClosed, "activation failed");
        assert!(state.session_last_error_typed().is_some());
        assert_eq!(
            state.control_last_error().as_deref(),
            Some("led test failed")
        );
    }

    /// D15: every observed `HELLO` sets the sticky
    /// `board_reachable` latch and records the board MAC +
    /// firmware; `wifi_status()` derives the `RSH-XXXX` SSID from
    /// it. `clear_session_records` (disconnect) clears the latch.
    #[test]
    fn hello_observation_sets_board_reachable_and_wifi_status() {
        use rsh_protocol::hello::HelloReply;

        let (state, _dir) = test_state();
        assert!(!state.board_reachable());
        assert!(state.last_observed_board_mac().is_none());
        let status = state.wifi_status();
        assert!(status.ssid.is_none());
        assert!(!status.connected);
        assert!(!status.password_set);
        let hello = HelloReply {
            protocol: (2, 2),
            flags: 0,
            session_echo: 0,
            board_mac: [0x20, 0x9B, 0xA9, 0x70, 0x9A, 0x55],
            firmware: (4, 0, 3),
            nonce: [0; 16],
            device_key: [0; 32],
            wifi_password: [0; 24],
        };
        state.note_observed_hello(&hello);
        assert!(state.board_reachable(), "HELLO sets the latch");
        assert_eq!(
            state.last_observed_board_mac().as_deref(),
            Some("20:9B:A9:70:9A:55")
        );
        let status = state.wifi_status();
        assert_eq!(status.ssid.as_deref(), Some("RSH-9A55"));
        assert_eq!(status.board_mac.as_deref(), Some("20:9B:A9:70:9A:55"));
        assert!(!status.connected, "no live session yet");
        // A second HELLO keeps the latch set (sticky, not toggled).
        state.note_observed_hello(&hello);
        assert!(state.board_reachable());
        // Disconnect clears the latch + the last HELLO.
        state.clear_session_records();
        assert!(!state.board_reachable());
        assert!(state.last_observed_board_mac().is_none());
        assert!(state.wifi_status().ssid.is_none());
    }

    /// R2-28: `connect_session` no-ops while an activation is
    /// already in flight — a manager `RetryTimer` can no longer
    /// abort a mid-activation forwarder by starting a second
    /// `connect_transport`.
    #[test]
    fn connect_session_noops_while_activation_in_flight() {
        let (state, _dir) = test_state();
        // No board is present, so a real activation attempt would
        // fail — the in-flight flag must short-circuit before any
        // transport work happens.
        state.activation_in_flight.store(true, Ordering::SeqCst);
        assert!(state.connect_session().is_ok());
        state.activation_in_flight.store(false, Ordering::SeqCst);
        // …and the guard clears the flag on drop, so a later
        // activation is not stuck behind a stale flag.
        {
            let _guard = ActivationInFlightGuard::arm(&state.activation_in_flight);
            assert!(state.activation_in_flight.load(Ordering::SeqCst));
        }
        assert!(!state.activation_in_flight.load(Ordering::SeqCst));
    }

    /// R3-D-02: `connect_session` no-ops while the forwarder is
    /// waiting out its own retry backoff or its circuit breaker is
    /// open — a manager `RetryTimer` firing in those windows must
    /// not abort the forwarder and reset its breaker streak, and a
    /// tripped breaker must stay tripped until the user reconnects.
    #[test]
    fn connect_session_noops_while_forwarder_retry_waiting_or_breaker_open() {
        let (state, _dir) = test_state();
        state.forwarder_retry_wait.store(1, Ordering::SeqCst);
        assert!(state.connect_session().is_ok());
        assert!(
            state.forwarder_retry_waiting(),
            "connect_session must not clear the forwarder's retry wait"
        );
        state.forwarder_retry_wait.store(0, Ordering::SeqCst);
        state.forwarder_breaker_open.store(true, Ordering::SeqCst);
        assert!(state.connect_session().is_ok());
        assert!(
            state.forwarder_breaker_open(),
            "connect_session must not clear a tripped breaker — only an explicit user Connect may"
        );
    }

    /// R3-D-05: `connect_transport` has the same in-flight guards
    /// as `connect_session` — a UI Connect during an activation or
    /// a forwarder retry wait no-ops instead of queueing a second
    /// `EngineCommand::Connect` that would abort the live
    /// forwarder. A no-op leaves the session untouched: the gate
    /// stays open and a latched breaker stays latched.
    #[test]
    fn connect_transport_noops_while_activation_in_flight_or_retry_waiting() {
        let (state, _dir) = test_state();
        state.open_session_gate();
        state.activation_in_flight.store(true, Ordering::SeqCst);
        assert!(state.connect_transport(TransportKind::Udp).is_ok());
        assert!(
            state.session_gate_open(),
            "a no-op connect must not close the gate"
        );
        state.activation_in_flight.store(false, Ordering::SeqCst);
        state.forwarder_retry_wait.store(1, Ordering::SeqCst);
        state.forwarder_breaker_open.store(true, Ordering::SeqCst);
        assert!(state.connect_transport(TransportKind::Udp).is_ok());
        assert!(
            state.session_gate_open(),
            "a no-op connect must not close the gate"
        );
        assert!(
            state.forwarder_breaker_open(),
            "a no-op connect must not clear a latched breaker"
        );
    }

    /// R3-D-02: an explicit (re)connect that proceeds clears a
    /// latched breaker — the latch stops automatic retries, not the
    /// user pressing Connect.
    #[test]
    fn connect_transport_clears_breaker_latch() {
        let (state, _dir) = test_state();
        state.forwarder_breaker_open.store(true, Ordering::SeqCst);
        assert!(state.connect_transport(TransportKind::Udp).is_ok());
        assert!(
            !state.forwarder_breaker_open(),
            "an explicit connect clears the breaker latch"
        );
        let _ = state.disconnect_transport();
        let _ = state.stop_engine();
    }

    /// R3-D-02 (integration): while a forwarder run waits out its
    /// backoff sleep, the retry-wait flag is armed and a manager
    /// `connect_session` no-ops instead of aborting the run — and
    /// when the run's breaker trips, the latch is set and
    /// `connect_session` keeps no-op-ing (no automatic respawn).
    #[tokio::test]
    async fn forwarder_retry_wait_blocks_connect_session_until_breaker_trips() {
        let (state, _dir) = test_state();
        let (frame_tx, _frame_rx) = tokio::sync::mpsc::channel(8);
        let (status_tx, _status_rx) = tokio::sync::mpsc::channel(8);
        let policy = ForwarderRetryPolicy {
            base: Duration::from_millis(200),
            max_delay: Duration::from_millis(500),
            max_consecutive_failures: 3,
        };
        let run = forward_with_retry_with_policy(
            Arc::clone(&state),
            TransportKind::Udp,
            frame_tx,
            status_tx,
            || Option::<FakeSessionTransport>::None,
            policy,
        );
        let run = tokio::spawn(run);
        // The run fails its first open immediately, then sleeps
        // 200 ms in its backoff — the wait must be visible.
        let mut waited = 0;
        while !state.forwarder_retry_waiting() && waited < 100 {
            tokio::time::sleep(Duration::from_millis(10)).await;
            waited += 1;
        }
        assert!(
            state.forwarder_retry_waiting(),
            "forwarder must arm its retry wait during the backoff sleep"
        );
        assert!(state.connect_session().is_ok());
        assert!(
            state.forwarder_retry_waiting(),
            "connect_session must not abort the waiting forwarder run"
        );
        tokio::time::timeout(Duration::from_secs(5), run)
            .await
            .expect("breaker must trip promptly, not retry forever")
            .expect("forwarder task joins");
        assert!(
            state.forwarder_breaker_open(),
            "a tripped breaker must latch forwarder_breaker_open"
        );
        assert!(
            !state.forwarder_retry_waiting(),
            "no wait after the run ends"
        );
        assert!(state.connect_session().is_ok());
        assert!(
            state.forwarder_breaker_open(),
            "connect_session must not clear a tripped breaker"
        );
    }

    /// `cycle_profile` wraps around in both directions and activates
    /// (saves + pushes the plan) exactly like a manual switch.
    #[test]
    fn cycle_profile_wraps_around() {
        let (state, _dir) = test_state();
        let first = activate_first_profile(&state);
        state.create_profile("Second").expect("create second");
        let second = state
            .list_profiles()
            .into_iter()
            .find(|summary| summary.name == "Second")
            .expect("second profile listed")
            .id;
        assert_eq!(state.cycle_profile(true).expect("cycle next").id, second);
        assert_eq!(
            state.cycle_profile(true).expect("cycle next wraps").id,
            first
        );
        assert_eq!(
            state.cycle_profile(false).expect("cycle prev wraps").id,
            second
        );
        assert_eq!(
            state.active_profile().expect("active").id,
            second,
            "the cycled-to profile is the active one"
        );
    }

    /// Bindings persist across a state reopen (same data dir), and
    /// a corrupt bindings file surfaces as a load warning instead
    /// of failing the open.
    #[test]
    fn bindings_persist_across_reopen_and_corrupt_file_warns() {
        let (state, dir) = test_state();
        state.set_binding(5, "horn").expect("bind 5");
        drop(state);
        let reopened = AppStateCore::open_with_null_sink(&dir).expect("reopen state with bindings");
        assert_eq!(reopened.list_bindings().len(), 1);
        assert_eq!(reopened.list_bindings()[0].button, 5);
        assert_eq!(reopened.list_bindings()[0].action, "horn");
        assert_eq!(reopened.binding_load_warnings().len(), 0);
        assert!(reopened.clear_binding(5).expect("clear 5"));
        drop(reopened);
        // Corrupt the file, then reopen: the state still opens, the
        // bindings are empty, and the warning names the problem.
        std::fs::write(dir.join("bindings.json"), "{corrupt").expect("write corrupt file");
        let repaired = AppStateCore::open_with_null_sink(&dir).expect("reopen with corrupt file");
        assert_eq!(repaired.list_bindings().len(), 0);
        assert_eq!(repaired.binding_load_warnings().len(), 1);
        assert!(
            repaired.binding_load_warnings()[0].contains("not valid JSON"),
            "warning: {:?}",
            repaired.binding_load_warnings()
        );
    }

    /// Binding validation errors carry stable codes through the
    /// state methods.
    #[test]
    fn binding_errors_carry_stable_codes() {
        let (state, _dir) = test_state();
        assert_eq!(
            state.set_binding(0, "horn").expect_err("button 0").code,
            "InvalidButton"
        );
        assert_eq!(
            state
                .set_binding(5, "warp_drive")
                .expect_err("unknown action")
                .code,
            "UnknownAction"
        );
        assert!(!state.clear_binding(5).expect("clear absent"));
    }

    // -- Deep-scan Phase 2 regressions -----------------------------------

    /// C2 / C-SET-1: a partial settings patch never wipes the fields
    /// it does not mention.
    #[test]
    fn settings_patch_merges_without_wiping() {
        let (state, _dir) = test_state();
        let mut settings = state.settings();
        settings.esp_ip = "10.0.0.5".to_string();
        settings.theme = "Dark".to_string();
        state.set_settings(settings).expect("set settings");
        let patch = AppSettingsPatch {
            theme: Some("light".to_string()),
            ..AppSettingsPatch::default()
        };
        let merged = state.set_settings_patch(patch).expect("patch applies");
        assert_eq!(merged.theme, "Light", "lowercase theme maps to canonical");
        assert_eq!(merged.esp_ip, "10.0.0.5", "absent esp_ip is kept");
        assert_eq!(merged.output_backend, "VJoy");
    }

    /// M-1: a public `esp_ip` is sanitized to the board AP default on
    /// save, while a loopback address (the tests' UDP forwarder) is kept.
    #[test]
    fn settings_sanitize_rejects_public_esp_ip_keeps_loopback() {
        let (state, _dir) = test_state();
        let mut settings = state.settings();
        settings.esp_ip = "8.8.8.8".to_string();
        let saved = state.set_settings(settings).expect("set settings");
        assert_eq!(saved.esp_ip, "192.168.4.1");
        let mut settings = state.settings();
        settings.esp_ip = "127.0.0.1".to_string();
        let saved = state.set_settings(settings).expect("set settings");
        assert_eq!(saved.esp_ip, "127.0.0.1");
    }

    /// C-LED-1: the LED payload scales RGB by brightness and the
    /// period is validated to 100–5000 ms.
    #[test]
    fn led_test_pattern_scales_brightness_and_validates_period() {
        let full = build_led_test_pattern(LedPattern::Solid, 100, None).expect("100%");
        assert_eq!((full.r, full.g, full.b), (255, 255, 255));
        assert_eq!(full.pattern_id, 1);
        assert_eq!(full.period_ms, 1000);
        assert_eq!(full.repeats, 0);
        let half = build_led_test_pattern(LedPattern::Blink, 50, Some(500)).expect("50%");
        assert_eq!((half.r, half.g, half.b), (128, 128, 128));
        assert_eq!(half.pattern_id, 2);
        let off = build_led_test_pattern(LedPattern::Off, 0, Some(100)).expect("off");
        assert_eq!((off.r, off.g, off.b), (0, 0, 0));
        assert_eq!(off.pattern_id, 0);
        let rainbow = build_led_test_pattern(LedPattern::Rainbow, 80, Some(5000)).expect("max");
        assert_eq!(rainbow.pattern_id, 4);
        assert!(build_led_test_pattern(LedPattern::Solid, 100, Some(99)).is_err());
        assert!(build_led_test_pattern(LedPattern::Solid, 100, Some(5001)).is_err());
        // Brightness above 100 clamps instead of overflowing.
        let clamped = build_led_test_pattern(LedPattern::Solid, 200, None).expect("clamp");
        assert_eq!((clamped.r, clamped.g, clamped.b), (255, 255, 255));
    }

    /// C-TUNE-1 / C-MOD-1: the extended tuning fields persist through
    /// `set_tuning`/`get_tuning`, and the `"accel"` spelling reads the
    /// same `"accelerator"` tuning.
    #[test]
    fn tuning_extended_fields_persist_and_accel_canonicalizes() {
        let (state, _dir) = test_state();
        let _ = activate_first_profile(&state);
        let mut tuning = state.get_tuning("accel");
        assert_eq!(tuning.module, "accelerator", "module id is canonicalized");
        tuning.deadzone = 4.0;
        tuning.min_mv = 120.0;
        tuning.max_mv = 3100.0;
        tuning.horn_button = Some(7);
        tuning.led_brightness = Some(60);
        tuning.led_pattern = Some(LedPattern::Blink);
        state.set_tuning(&tuning).expect("set tuning");
        let read_back = state.get_tuning("accelerator");
        assert_eq!(read_back.min_mv, 120.0);
        assert_eq!(read_back.max_mv, 3100.0);
        assert_eq!(read_back.horn_button, Some(7));
        assert_eq!(read_back.led_brightness, Some(60));
        assert_eq!(read_back.led_pattern, Some(LedPattern::Blink));
        // The legacy spelling reads the same stored tuning.
        assert_eq!(state.get_tuning("accel").min_mv, 120.0);
    }

    /// L8: a present-but-released clutch (0.0) reports `Some(0.0)`
    /// when the clutch module is enabled, and `None` with no clutch
    /// tuning (the Default profile).
    #[test]
    fn clutch_zero_reports_present_when_module_enabled() {
        let (state, _dir) = test_state();
        let _ = activate_first_profile(&state);
        let output = PipelineOutputState::neutral();
        let metrics = EngineMetrics::new();
        state.ingest_engine_output(&output, &metrics);
        assert_eq!(
            state.snapshot().pedals.clutch,
            None,
            "no clutch tuning: 0.0 means 'no clutch module'"
        );
        // Enabling the clutch module (any tuning write does) makes
        // the released clutch report `Some(0.0)`.
        let mut tuning = state.get_tuning("clutch");
        tuning.deadzone = 2.0;
        state.set_tuning(&tuning).expect("set clutch tuning");
        state.ingest_engine_output(&output, &metrics);
        assert_eq!(state.snapshot().pedals.clutch, Some(0.0));
    }

    /// H3: `enable_auto_switch` turns the setting on and starts the
    /// watcher thread.
    #[test]
    fn enable_auto_switch_starts_watcher() {
        let (state, _dir) = test_state();
        let settings = state.enable_auto_switch().expect("enable");
        assert!(settings.auto_switch_profiles);
        assert!(state.auto_switch_running());
        assert!(!lock(&state.store).auto_switch_locked());
        state.stop_auto_switch();
        assert!(!state.auto_switch_running());
    }

    /// C-FAIL-1: a refused mid-session update fails over to vJoy
    /// once, a second failure errors without a second switch, and
    /// `reset_fallback` re-arms the flag (the 30 s rate limit still
    /// holds the next attempt).
    #[test]
    fn mid_session_failover_switches_once_and_rearms() {
        let (hm, _hm_sent, hm_fail) = RecordingBackend::new_named("HIDMaestro");
        let (vj, vj_sent, vj_fail) = RecordingBackend::new_named("vJoy");
        let factories = BackendFactories {
            hidmaestro: Some(Box::new(move || {
                Box::new(hm.clone()) as Box<dyn IJoystickOutput>
            })),
            vjoy: Some(Box::new(move || {
                Box::new(vj.clone()) as Box<dyn IJoystickOutput>
            })),
            none: Box::new(|| Box::new(NullSink) as Box<dyn IJoystickOutput>),
        };
        let (state, _dir) = test_state();
        state
            .apply_backend_switch(BackendKind::HidMaestro, &factories, true)
            .expect("install HIDMaestro mock");
        assert_eq!(state.backend_name(), "HIDMaestro");
        let output = PipelineOutputState::neutral();
        // The live backend refuses the update: fail over to vJoy and
        // retry there — the send succeeds on the fallback.
        *hm_fail.lock().expect("hm fail lock") = true;
        assert!(
            state
                .try_mid_session_fallback_with_factories(&output, &factories)
                .is_ok()
        );
        assert!(state.fallback_used());
        assert_eq!(state.backend_name(), "vJoy");
        assert_eq!(vj_sent.lock().expect("vj sent lock").len(), 1);
        // The fallback refusing too: `send_engine_output` routes the
        // refusal into the failover, which is already used — an
        // honest error, no second switch.
        *vj_fail.lock().expect("vj fail lock") = true;
        assert!(state.send_engine_output(&output).is_err());
        assert_eq!(state.backend_name(), "vJoy", "no second switch happened");
        // An explicit re-select re-arms the flag — but the 30 s rate
        // limit still holds the next attempt.
        state.reset_fallback();
        assert!(!state.fallback_used());
        assert!(state.send_engine_output(&output).is_err());
        assert_eq!(state.backend_name(), "vJoy", "rate limit held the retry");
    }

    /// PERF F-15: a slow `update()` must not hold the backend slot
    /// lock. The old `send_engine_output` held `Mutex<Box<...>>`
    /// across `update()`, so a switch (and every slot read) stalled
    /// for the whole `HIDMaestro` sidecar round-trip (~200 ms). Here
    /// a tick is parked inside `update()` while the main thread
    /// clones the backend handle — the slot lock must be free, so
    /// the clone returns at once instead of waiting out the update.
    /// (The switch itself still needs the old instance's lock to
    /// disconnect it, so it waits for at most one update — it is
    /// proven below to complete promptly once the update is
    /// released, and to install the new backend.)
    #[test]
    fn slow_update_does_not_hold_the_backend_slot_lock() {
        let dir = test_dir("blocking-slot");
        let store = ProfileStore::open(&dir).expect("open test store");
        let (backend, entered, release) = BlockingBackend::new();
        let state = Arc::new(AppStateCore::new(
            store,
            Box::new(backend),
            "test blocking backend",
        ));
        *lock(&state.output_enabled) = true;
        let output = PipelineOutputState::neutral();
        let tick_state = Arc::clone(&state);
        let tick = std::thread::spawn(move || tick_state.send_engine_output(&output));
        assert!(
            BlockingBackend::wait_flag(&entered, Duration::from_secs(5)),
            "the tick must enter update() before the slot lock is probed"
        );
        let start = Instant::now();
        let _handle = state.backend_handle();
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "cloning the backend handle must not wait out the parked update"
        );
        BlockingBackend::set_flag(&release);
        let sent = tick.join().expect("tick thread joins");
        assert!(
            sent.is_ok(),
            "the parked tick still sends on the old instance"
        );
        let factories = mock_factories(false, false);
        state
            .apply_backend_switch(BackendKind::VJoy, &factories, true)
            .expect("switch to vJoy after the update");
        assert_eq!(state.backend_name(), "RecordingBackend");
    }

    /// Deep-scan Round 2 item 5 (H2 latch): re-selecting a
    /// disconnected `HIDMaestro` whose manager latch is set must
    /// succeed — the re-select rebuilds the installed instance with
    /// a fresh factory build (latch clear) instead of reconnecting
    /// the latched instance, which would fail fast with
    /// `FallbackNeeded` and fall back to vJoy. The latched instance
    /// is modelled by a mock whose `try_connect` always fails; the
    /// factory builds a fresh mock per call whose connect succeeds.
    #[test]
    fn reselect_disconnected_hidmaestro_rebuilds_latched_instance() {
        let (latched, _latched_sent, _latched_fail) = RecordingBackend::new_named("HIDMaestro");
        latched.set_fail_connect(true);
        let dir = test_dir("latched-reselect");
        let store = ProfileStore::open(&dir).expect("open test store");
        let state = Arc::new(AppStateCore::new(
            store,
            Box::new(latched),
            "test latched backend",
        ));
        let factories = BackendFactories {
            hidmaestro: Some(Box::new(|| {
                let (backend, _, _) = RecordingBackend::new_named("HIDMaestro");
                Box::new(backend) as Box<dyn IJoystickOutput>
            })),
            vjoy: Some(Box::new(|| {
                let (backend, _, _) = RecordingBackend::new_named("vJoy");
                Box::new(backend) as Box<dyn IJoystickOutput>
            })),
            none: Box::new(|| Box::new(NullSink) as Box<dyn IJoystickOutput>),
        };
        // Without the rebuild this re-select would reconnect the
        // latched instance, fail, and fall back to vJoy.
        state.reset_fallback();
        let outcome = state
            .apply_backend_switch(BackendKind::HidMaestro, &factories, false)
            .expect("re-select of a latched backend must succeed on the fresh instance");
        assert_eq!(outcome.active, BackendKind::HidMaestro);
        assert!(!outcome.fell_back);
        assert_eq!(state.backend_name(), "HIDMaestro");
        assert!(state.output_state().connected, "fresh instance connected");
    }

    /// C1: the real auto-switch wiring — a fake car source + fake
    /// foreground over the store-backed source switches the active
    /// profile and queues the `rsh://profile-switched` event.
    #[test]
    fn auto_switch_fake_source_switches_and_queues_event() {
        use rsh_profiles::resolver::{GAME_ASSETTO_CORSA_COMPETIZIONE, MapProfileSource};
        use rsh_profiles::watcher::{CarReading, CarSource, ForegroundDetector};

        /// Fake car source: always the same on-track car.
        #[derive(Clone)]
        struct StubCarSource {
            reading: Option<CarReading>,
        }

        impl CarSource for StubCarSource {
            fn current_car(&self) -> Option<CarReading> {
                self.reading.clone()
            }
        }

        /// Fake foreground: always the same game.
        #[derive(Clone)]
        struct StubForeground {
            game: Option<String>,
        }

        impl ForegroundDetector for StubForeground {
            fn foreground_game(&self) -> Option<String> {
                self.game.clone()
            }
        }

        let (state, _dir) = test_state();
        let created = state.create_profile("ACC Ferrari").expect("create");
        let source = MapProfileSource::new().with_car_profile(
            GAME_ASSETTO_CORSA_COMPETIZIONE,
            // Same spelling as the reading below: `MapProfileSource`
            // normalises by trim + lowercase only (the production
            // store source uses the canonical separator normalizer
            // on both sides instead).
            "Ferrari 488 GT3",
            ProfileId::new(created.id.clone()),
        );
        let resolver = CarProfileResolver::new_owned(Box::new(source));
        let weak = Arc::downgrade(&state);
        let watcher = TelemetryWatcher::new(
            resolver,
            StubCarSource {
                reading: Some(CarReading::new(
                    GAME_ASSETTO_CORSA_COMPETIZIONE,
                    "Ferrari 488 GT3",
                    true,
                )),
            },
            StubForeground {
                game: Some(GAME_ASSETTO_CORSA_COMPETIZIONE.to_string()),
            },
            move |profile_id: &ProfileId| {
                let Some(state) = weak.upgrade() else {
                    return Err("app state dropped".to_string());
                };
                state.apply_auto_switch(profile_id)
            },
            state
                .active_profile()
                .map(|active| ProfileId::new(active.id)),
        );
        state.install_auto_switch_poller(Box::new(watcher));
        // 2 Hz polls for 5 s: the 400 ms debounce + 4-poll / 2 s car
        // stability both clear well inside this window.
        for _ in 0..10 {
            state.poll_auto_switch_once();
            std::thread::sleep(Duration::from_millis(500));
        }
        assert_eq!(
            state.active_profile().map(|active| active.id),
            Some(created.id.clone()),
            "the watcher must switch to the Ferrari profile"
        );
        let events = state.drain_app_events();
        assert!(
            events
                .iter()
                .any(|(name, payload)| name == PROFILE_SWITCHED_EVENT
                    && payload.get("newProfileId").and_then(|value| value.as_str())
                        == Some(created.id.as_str())),
            "profile-switched event must be queued: {events:?}"
        );
    }

    #[test]
    fn calibration_errors_when_unset_and_round_trips_when_saved() {
        let (state, _dir) = test_state();
        assert!(state.get_calibration().is_err()); // no active profile
        let _ = activate_first_profile(&state);
        assert!(state.get_calibration().is_err()); // nothing captured yet
        let mut pedal_min = BTreeMap::new();
        pedal_min.insert("accel".to_string(), 100.0);
        pedal_min.insert("brake".to_string(), 120.0);
        let mut pedal_max = BTreeMap::new();
        pedal_max.insert("accel".to_string(), 3200.0);
        pedal_max.insert("brake".to_string(), 3100.0);
        let calibration = Calibration {
            steering_center_mv: 1650.0,
            pedal_min_mv: pedal_min,
            pedal_max_mv: pedal_max,
            calibrated_at: None,
        };
        let saved = state
            .save_calibration(calibration)
            .expect("save calibration");
        assert_eq!(saved.steering_center_mv, 1650.0);
        let read_back = state.get_calibration().expect("read calibration");
        assert_eq!(read_back.steering_center_mv, 1650.0);
        assert_eq!(read_back.pedal_min_mv.get("accel"), Some(&100.0));
        assert_eq!(read_back.pedal_max_mv.get("brake"), Some(&3100.0));
        // L4: saving stamps the capture time, and the read-back
        // carries it (RFC 3339, never a fake date).
        let stamped = read_back.calibrated_at.as_deref().expect("timestamp set");
        assert!(stamped.contains('T'), "RFC 3339 timestamp: {stamped}");
        // Bad ranges fail honestly.
        let mut bad = read_back.clone();
        bad.pedal_min_mv.insert("accel".to_string(), 4000.0);
        assert!(state.save_calibration(bad).is_err());
    }

    /// v2.0.2 capture plumbing: the latest frame's raw axes reach
    /// the state through `note_raw_axes`, and `raw_mv` reports them
    /// with an age — never a fake 0 before any frame arrives.
    #[test]
    fn raw_mv_reports_latest_frame_axes_and_age() {
        let (state, _dir) = test_state();
        let empty = state.raw_mv();
        assert_eq!(empty.steering_mv, None);
        assert_eq!(empty.accel_mv, None);
        assert_eq!(empty.age_ms, None);
        state.note_raw_axes([
            Some(1812),
            Some(320),
            Some(300),
            None,
            Some(250),
            None,
            None,
            None,
        ]);
        let reading = state.raw_mv();
        assert_eq!(reading.steering_mv, Some(1812));
        assert_eq!(reading.accel_mv, Some(320));
        assert_eq!(reading.brake_mv, Some(300));
        assert_eq!(reading.clutch_mv, None);
        assert_eq!(reading.handbrake_mv, Some(250));
        assert!(reading.age_ms.is_some(), "a noted sample has an age");
        // A newer frame replaces the sample (capture samples "now").
        state.note_raw_axes([
            Some(1790),
            Some(400),
            Some(310),
            Some(900),
            None,
            None,
            None,
            None,
        ]);
        let newer = state.raw_mv();
        assert_eq!(newer.steering_mv, Some(1790));
        assert_eq!(newer.clutch_mv, Some(900));
        assert_eq!(newer.handbrake_mv, None);
    }

    /// v2.0.2: a sample older than the freshness window is refused
    /// (proven through the clock-taking half, so the test never
    /// sleeps), while a fresh sample is accepted.
    #[test]
    fn fresh_raw_axes_refuses_a_stale_sample() {
        let (state, _dir) = test_state();
        assert!(
            state.fresh_raw_axes_at(Instant::now()).is_err(),
            "no frame ever seen must fail, not invent a reading"
        );
        state.note_raw_axes([Some(1800), None, None, None, None, None, None, None]);
        assert!(state.fresh_raw_axes_at(Instant::now()).is_ok());
        let aged = state.fresh_raw_axes_at(Instant::now() + Duration::from_secs(3));
        assert!(aged.is_err(), "a 3 s old sample must be refused");
    }

    /// v2.0.2: centre capture needs live data, then saves the live
    /// steering reading as the centre — and the live pipeline plan
    /// picks it up.
    #[test]
    fn capture_centre_needs_live_data_and_drives_the_plan() {
        let (state, _dir) = test_state();
        let _ = activate_first_profile(&state);
        assert!(
            state.capture_centre().is_err(),
            "no live frame yet: capture must fail, not save a default"
        );
        state.note_raw_axes([
            Some(1812),
            Some(300),
            Some(300),
            None,
            None,
            None,
            None,
            None,
        ]);
        assert_eq!(state.capture_centre().expect("capture centre"), 1812);
        let read_back = state.get_calibration().expect("read calibration");
        assert_eq!(read_back.steering_center_mv, 1812.0);
        assert!(read_back.calibrated_at.is_some(), "capture stamps the time");
        assert_eq!(state.build_pipeline_plan().steering.center_mv, 1812);
    }

    /// v2.0.2: pedal min/max capture round-trips through the
    /// calibration record AND the module tuning, and the live plan
    /// uses the captured range.
    #[test]
    fn capture_pedal_min_max_round_trips_and_drives_the_plan() {
        let (state, _dir) = test_state();
        let _ = activate_first_profile(&state);
        state.note_raw_axes([
            Some(1800),
            Some(320),
            Some(300),
            None,
            None,
            None,
            None,
            None,
        ]);
        assert_eq!(
            state.capture_pedal_min("accel").expect("capture accel min"),
            320
        );
        state.note_raw_axes([
            Some(1800),
            Some(3200),
            Some(300),
            None,
            None,
            None,
            None,
            None,
        ]);
        assert_eq!(
            state.capture_pedal_max("accel").expect("capture accel max"),
            3200
        );
        let tuning = state.get_tuning("accelerator");
        assert_eq!(tuning.min_mv, 320.0);
        assert_eq!(tuning.max_mv, 3200.0);
        let plan = state.build_pipeline_plan();
        assert_eq!(plan.accelerator.min_mv, 320);
        assert_eq!(plan.accelerator.max_mv, 3200);
        // The centre is untouched by a pedal capture: no steering
        // calibration exists yet, so the calibration read still
        // errors instead of inventing a centre.
        assert!(state.get_calibration().is_err());
    }

    /// v2.0.2: a half-pressed capture is refused (span < 660 mV),
    /// an inverted range is refused, and an unknown pedal name is
    /// refused — never saved as a useless range.
    #[test]
    fn capture_pedal_rejects_narrow_span_inverted_range_and_unknown_pedal() {
        let (state, _dir) = test_state();
        let _ = activate_first_profile(&state);
        assert!(state.capture_pedal_min("gear").is_err());
        // Pedal barely moved: min 3000 against the 3300 default max
        // spans 300 mV < 660 mV.
        state.note_raw_axes([
            Some(1800),
            Some(3000),
            Some(300),
            None,
            None,
            None,
            None,
            None,
        ]);
        assert!(state.capture_pedal_min("accel").is_err());
        // A real min, then a max below it: inverted.
        state.note_raw_axes([
            Some(1800),
            Some(320),
            Some(300),
            None,
            None,
            None,
            None,
            None,
        ]);
        assert_eq!(state.capture_pedal_min("accel").expect("capture min"), 320);
        state.note_raw_axes([
            Some(1800),
            Some(200),
            Some(300),
            None,
            None,
            None,
            None,
            None,
        ]);
        assert!(
            state.capture_pedal_max("accel").is_err(),
            "max below min must fail"
        );
    }

    /// v2.0.2: the handbrake has no calibration record, but its
    /// endpoints still capture into the module tuning + live plan.
    #[test]
    fn capture_handbrake_endpoints_land_in_tuning_and_plan() {
        let (state, _dir) = test_state();
        let _ = activate_first_profile(&state);
        state.note_raw_axes([
            Some(1800),
            Some(300),
            Some(300),
            None,
            Some(250),
            None,
            None,
            None,
        ]);
        assert_eq!(
            state
                .capture_pedal_min("handbrake")
                .expect("capture handbrake min"),
            250
        );
        state.note_raw_axes([
            Some(1800),
            Some(300),
            Some(300),
            None,
            Some(3100),
            None,
            None,
            None,
        ]);
        assert_eq!(
            state
                .capture_pedal_max("handbrake")
                .expect("capture handbrake max"),
            3100
        );
        let tuning = state.get_tuning("handbrake");
        assert_eq!(tuning.min_mv, 250.0);
        assert_eq!(tuning.max_mv, 3100.0);
        let plan = state.build_pipeline_plan();
        assert_eq!(plan.handbrake.min_mv, 250);
        assert_eq!(plan.handbrake.max_mv, 3100);
    }

    /* ------------------------------------------------------------------ */
    /* Phase G gate wiring: engine spawn + startup backend               */
    /* ------------------------------------------------------------------ */

    /// Mock backend factories that report the real backend names
    /// (`"HIDMaestro"` / `"vJoy"`), each failing to connect with the
    /// given message when `Some` (connecting fine when `None`).
    /// Clones share the recording state, so the live backend's name
    /// and connection state stay observable after a switch.
    fn named_mock_factories(
        hm_fail_message: Option<&str>,
        vjoy_fail_message: Option<&str>,
    ) -> BackendFactories {
        let (hm, _hm_sent, _hm_fail) = RecordingBackend::new_named("HIDMaestro");
        if let Some(message) = hm_fail_message {
            hm.set_fail_connect(true);
            hm.set_fail_connect_message(message);
        }
        let (vj, _vj_sent, _vj_fail) = RecordingBackend::new_named("vJoy");
        if let Some(message) = vjoy_fail_message {
            vj.set_fail_connect(true);
            vj.set_fail_connect_message(message);
        }
        BackendFactories {
            hidmaestro: Some(Box::new(move || {
                Box::new(hm.clone()) as Box<dyn IJoystickOutput>
            })),
            vjoy: Some(Box::new(move || {
                Box::new(vj.clone()) as Box<dyn IJoystickOutput>
            })),
            none: Box::new(|| Box::new(NullSink) as Box<dyn IJoystickOutput>),
        }
    }

    /// Mock backend factories for the startup-activation tests: the
    /// `HIDMaestro` and vJoy factories build clones of two recording
    /// backends (shared record), each failing to connect when its flag
    /// is set.
    fn mock_factories(hm_fail_connect: bool, vjoy_fail_connect: bool) -> BackendFactories {
        let (hm, _hm_sent, _hm_fail) = RecordingBackend::new();
        hm.set_fail_connect(hm_fail_connect);
        let (vj, _vj_sent, _vj_fail) = RecordingBackend::new();
        vj.set_fail_connect(vjoy_fail_connect);
        BackendFactories {
            hidmaestro: Some(Box::new(move || {
                Box::new(hm.clone()) as Box<dyn IJoystickOutput>
            })),
            vjoy: Some(Box::new(move || {
                Box::new(vj.clone()) as Box<dyn IJoystickOutput>
            })),
            none: Box::new(|| Box::new(NullSink) as Box<dyn IJoystickOutput>),
        }
    }

    /// Polls `cond` every 20 ms until it holds or `timeout` elapses.
    fn wait_until(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if cond() {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// One stream frame: steering on slot 0, two encoders, the given
    /// sequence and board timestamp (5 ms apart, 200 Hz spacing).
    fn test_stream_frame(seq: u16, steer_mv: u16, encoders: &[i16]) -> StreamFrame {
        StreamFrame::new(
            seq,
            u32::from(seq) * 5_000,
            rsh_protocol::StreamFlags::default(),
            &[(0, steer_mv)],
            encoders,
            0,
        )
    }

    /// States the recording backend was sent, cloned out.
    fn recorded_states(sent: &Arc<Mutex<Vec<PipelineOutputState>>>) -> Vec<PipelineOutputState> {
        sent.lock().expect("sent lock poisoned").clone()
    }

    #[test]
    fn transport_kind_parses_names() {
        assert_eq!(TransportKind::parse("udp"), Some(TransportKind::Udp));
        assert_eq!(TransportKind::parse("WiFi"), Some(TransportKind::Udp));
        assert_eq!(TransportKind::parse("serial"), Some(TransportKind::Serial));
        assert_eq!(TransportKind::parse("wired"), Some(TransportKind::Serial));
        assert_eq!(TransportKind::parse("bluetooth"), None);
        assert_eq!(TransportKind::Udp.as_str(), "udp");
        assert_eq!(TransportKind::Serial.as_str(), "serial");
    }

    #[test]
    fn startup_activation_defers_hidmaestro_at_startup() {
        // Hotfix v1.0.1 (UAC deferral): a persisted HIDMaestro choice
        // is NEVER activated at startup — even when the mock backend
        // would connect fine, no factory is touched, no UAC prompt
        // could appear. The outcome records the deferral, the NullSink
        // stays installed, and the persisted choice is untouched.
        // (The fresh-store default is vJoy since v2.0.1, so the
        // persisted HIDMaestro choice is set explicitly here.)
        let (state, _dir) = test_state_with_backend_setting("HIDMaestro");
        assert_eq!(
            state.selected_backend(),
            BackendKind::HidMaestro,
            "persisted HIDMaestro choice is selected"
        );
        let factories = mock_factories(false, false);
        let outcome = state.activate_startup_backend_with_factories(&factories);
        assert_eq!(outcome.requested, BackendKind::HidMaestro);
        assert_eq!(outcome.active, None);
        assert!(!outcome.fell_back);
        let reason = outcome.reason.clone().expect("deferral reason recorded");
        assert!(reason.contains("deferred"), "{reason}");
        assert!(reason.contains("elevation"), "{reason}");
        assert_eq!(state.startup_backend_outcome(), Some(outcome));
        assert_eq!(state.backend_name(), "NullSink");
        assert!(!state.output_state().connected);
        assert!(
            state.output_state().detail.contains("deferred"),
            "{}",
            state.output_state().detail
        );
        // The persisted choice is still HIDMaestro: the next launch
        // defers it the same way, and enabling output retries it.
        assert_eq!(state.selected_backend(), BackendKind::HidMaestro);
        assert_eq!(state.settings().output_backend, "HIDMaestro");
    }

    #[test]
    fn startup_activation_activates_vjoy_when_selected() {
        // A non-elevated choice IS activated at startup: vJoy needs
        // no UAC prompt, so the persisted vJoy choice goes live.
        let (state, _dir) = test_state_with_backend_setting("VJoy");
        assert_eq!(state.selected_backend(), BackendKind::VJoy);
        let factories = mock_factories(false, false);
        let outcome = state.activate_startup_backend_with_factories(&factories);
        assert_eq!(outcome.requested, BackendKind::VJoy);
        assert_eq!(outcome.active, Some(BackendKind::VJoy));
        assert!(!outcome.fell_back);
        assert_eq!(outcome.reason, None);
        assert_eq!(state.startup_backend_outcome(), Some(outcome));
        assert!(state.output_state().connected, "mock backend connected");
        assert_eq!(state.backend_name(), "RecordingBackend");
    }

    #[test]
    fn startup_activation_vjoy_failing_records_reason_and_stays_on_null_sink() {
        // vJoy has no fallback: when its connect fails, the outcome
        // records `active: None` and the reason, and the NullSink
        // stays installed.
        let (state, _dir) = test_state_with_backend_setting("VJoy");
        let factories = mock_factories(false, true);
        let outcome = state.activate_startup_backend_with_factories(&factories);
        assert_eq!(outcome.requested, BackendKind::VJoy);
        assert_eq!(outcome.active, None);
        let reason = outcome.reason.clone().expect("failure reason recorded");
        assert!(reason.contains("mock connect failed"), "{reason}");
        assert_eq!(state.backend_name(), "NullSink");
        assert!(!state.output_state().connected);
        assert!(
            state
                .output_state()
                .detail
                .contains("startup backend activation failed"),
            "{}",
            state.output_state().detail
        );
        assert!(
            state.output_state().detail.contains("mock connect failed"),
            "{}",
            state.output_state().detail
        );
        // The persisted choice is still vJoy for the next launch.
        assert_eq!(state.selected_backend(), BackendKind::VJoy);
        assert_eq!(state.settings().output_backend, "VJoy");
    }

    #[test]
    fn enabling_output_after_deferred_startup_activates_hidmaestro() {
        // The deferred backend is activated on the first user-initiated
        // enable: startup defers, then `set_output_enabled(true)`
        // switches the selected HIDMaestro backend in and connects it.
        // (Fresh-store default is vJoy since v2.0.1 — the persisted
        // HIDMaestro choice is set explicitly here.)
        let (state, _dir) = test_state_with_backend_setting("HIDMaestro");
        let factories = mock_factories(false, false);
        let deferred = state.activate_startup_backend_with_factories(&factories);
        assert_eq!(deferred.active, None, "startup defers HIDMaestro");
        let enabled = state.set_output_enabled_with_factories(true, &factories);
        assert!(enabled.connected, "backend connected after enable");
        assert_eq!(state.backend_name(), "RecordingBackend");
        assert_eq!(state.selected_backend(), BackendKind::HidMaestro);
        assert_eq!(state.settings().output_backend, "HIDMaestro");
    }

    #[test]
    fn enabling_output_with_failing_backends_reports_failure_honestly() {
        // Enable after a deferred startup, with both mock backends
        // failing to connect: no backend goes live, the NullSink stays
        // installed, and the returned state says why.
        // (Fresh-store default is vJoy since v2.0.1 — the persisted
        // HIDMaestro choice is set explicitly here.)
        let (state, _dir) = test_state_with_backend_setting("HIDMaestro");
        let factories = mock_factories(true, true);
        let deferred = state.activate_startup_backend_with_factories(&factories);
        assert_eq!(deferred.active, None, "startup defers HIDMaestro");
        let enabled = state.set_output_enabled_with_factories(true, &factories);
        assert!(!enabled.connected);
        assert_eq!(state.backend_name(), "NullSink");
        assert!(
            enabled.detail.contains("backend activation failed"),
            "{}",
            enabled.detail
        );
        assert!(
            enabled.detail.contains("mock connect failed"),
            "{}",
            enabled.detail
        );
    }

    #[test]
    #[cfg(not(windows))]
    fn startup_activation_without_drivers_defers_hidmaestro_honestly() {
        // Linux sandbox: no HIDMaestro sidecar, no vJoy DLL — the
        // persisted HIDMaestro choice is deferred at startup (not a
        // connect failure: no factory is touched, nothing is probed),
        // and the outcome must say so instead of silently staying on
        // the NullSink.
        // (Fresh-store default is vJoy since v2.0.1 — the persisted
        // HIDMaestro choice is set explicitly here.)
        let (state, _dir) = test_state_with_backend_setting("HIDMaestro");
        let outcome = state.activate_startup_backend();
        assert_eq!(outcome.requested, BackendKind::HidMaestro);
        assert_eq!(outcome.active, None);
        let reason = outcome
            .reason
            .clone()
            .expect("reason recorded: {outcome:?}");
        assert!(reason.contains("deferred"), "{reason}");
        assert_eq!(state.backend_name(), "NullSink");
        assert!(!state.output_state().connected);
    }

    #[test]
    fn stop_engine_without_start_is_a_noop() {
        let (state, _dir) = test_state();
        assert_eq!(state.engine_status(), EngineTaskStatus::Idle);
        let outcome = state.stop_engine();
        assert!(!outcome.joined);
        assert!(outcome.report.is_none());
    }

    #[test]
    fn start_engine_respawns_after_a_dead_supervisor() {
        // DS-16: a finished supervisor handle used to make every
        // later `start_engine` a silent no-op (the stored handle
        // was never liveness-checked), so a dead engine never
        // recovered without a process restart. Plant a thread that
        // already exited as the supervisor: the next start must
        // reap it and spawn fresh, and the engine must report
        // Running afterwards.
        let (state, _dir) = test_state();
        let dead = std::thread::Builder::new()
            .name("rsh-test-dead-supervisor".to_string())
            .spawn(|| {})
            .expect("spawn dead thread");
        while !dead.is_finished() {
            std::thread::sleep(Duration::from_millis(1));
        }
        *lock(&state.engine_thread) = Some(dead);
        state
            .start_engine()
            .expect("start_engine must respawn a dead supervisor");
        assert_eq!(
            state.engine_status(),
            EngineTaskStatus::Running { transport: None },
            "the respawned engine runs idle with no transport"
        );
        // A second start is still a no-op while the fresh one lives.
        state.start_engine().expect("second start is a no-op");
        let outcome = state.stop_engine();
        assert!(outcome.joined, "engine stops bounded");
    }

    #[test]
    fn engine_spawn_flows_frames_to_backend_watchdog_neutral_and_bounded_stop() {
        let (state, sent, _fail) = test_state_with_recording();
        // Connect the recording backend first: the sink only sends to
        // a connected backend (output disabled is not a sink error).
        let _ = state.set_output_enabled(true);
        state.start_engine().expect("engine starts");
        // Starting twice is a no-op, not a second engine.
        state.start_engine().expect("second start is a no-op");
        assert_eq!(
            state.engine_status(),
            EngineTaskStatus::Running { transport: None },
            "engine runs idle with no transport"
        );
        let frame_tx = state
            .engine_frame_sender()
            .expect("frame sender after start");
        for (seq, encoders) in [(0, [0, 0]), (1, [10, -5]), (2, [10, -5])] {
            let frame = test_stream_frame(seq, 3300, &encoders);
            // The forwarder reports each frame's encoder counts; a
            // direct feeder does the same per frame.
            state.note_frame_encoders(frame.encoders, frame.n_enc);
            frame_tx.try_send(frame).expect("frame channel open");
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            wait_until(Duration::from_secs(5), || recorded_states(&sent)
                .iter()
                .any(|s| s.steering > 0.9)),
            "live frame reached the backend: {:?}",
            recorded_states(&sent)
        );
        assert!(
            state.snapshot().steering_deg > 400.0,
            "snapshot shows full-right steering: {:?}",
            state.snapshot()
        );
        let encoders = state.snapshot().encoders;
        assert_eq!(encoders.len(), 2, "both encoders reported: {encoders:?}");
        assert_eq!(encoders[0].count, 10, "encoder 0 count: {encoders:?}");
        assert_eq!(encoders[1].count, -5, "encoder 1 count: {encoders:?}");
        // Stop feeding: after the 250 ms watchdog the engine sends
        // neutral on every tick.
        assert!(
            wait_until(Duration::from_secs(5), || recorded_states(&sent)
                .iter()
                .any(|s| *s == PipelineOutputState::neutral())),
            "watchdog went NEUTRAL on stale data: {:?}",
            recorded_states(&sent)
        );
        let outcome = state.stop_engine();
        assert!(outcome.joined, "bounded stop joined");
        let report = outcome.report.expect("final report delivered");
        assert!(
            report.counters.frames_processed >= 3,
            "frames processed: {}",
            report.counters.frames_processed
        );
        assert_eq!(state.engine_status(), EngineTaskStatus::Stopped);
        assert_eq!(
            state
                .engine_last_report()
                .expect("last report kept")
                .counters
                .frames_processed,
            report.counters.frames_processed
        );
    }

    #[test]
    fn engine_idle_without_transport_sends_no_fake_frames() {
        let (state, sent, _fail) = test_state_with_recording();
        let _ = state.set_output_enabled(true);
        state.start_engine().expect("engine starts");
        // The loop rate counts ticks inside a 1 s window, so a full
        // second of idle ticks must pass before the rate settles.
        std::thread::sleep(Duration::from_millis(1200));
        let recorded = recorded_states(&sent);
        assert!(
            recorded
                .iter()
                .all(|s| *s == PipelineOutputState::neutral()),
            "idle engine sends only neutral, never fake input: {recorded:?}"
        );
        // The loop rate is measured from the real ticks, even idle.
        // R2-39: with no transport and no frames the engine parks
        // at 10 Hz instead of ticking 200 Hz idle — the parked rate
        // is the expected measurement here. Wait for it to settle
        // instead of asserting right after the fixed sleep: under
        // full-suite load the 1 s window may not have settled after
        // 1200 ms (known flake — the rate reads low or None), so
        // poll until it lands in the parked band.
        let engine_stats = state.engine_stats();
        assert!(
            wait_until(Duration::from_secs(10), || state
                .engine_stats()
                .loop_hz
                .is_some_and(|hz| (5.0..=20.0).contains(&hz))),
            "idle loop rate measured (~10 Hz parked): {engine_stats:?}"
        );
        assert_eq!(
            state.engine_status(),
            EngineTaskStatus::Running { transport: None }
        );
        assert_eq!(
            state.snapshot().connection,
            ConnectionState::Disconnected,
            "no transport, no fake connection"
        );
        let outcome = state.stop_engine();
        assert!(outcome.joined, "idle engine stops bounded");
        assert_eq!(
            outcome.report.expect("report").counters.frames_processed,
            0,
            "no frames were ever processed"
        );
    }

    /// The forwarder binds the fixed stream port 4211, so the two
    /// UDP forwarder tests serialize on this lock — without it they
    /// run in parallel, one fails to bind, and its loopback frames
    /// (UDP, sent while unbound) are lost. Held for the whole test,
    /// including the bounded engine stop.
    static UDP_FORWARDER_PORT: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Lock the shared stream-port guard, recovering from poisoning:
    /// a previous holder may have panicked (that test already
    /// failed); the port itself is freed separately by
    /// `wait_for_stream_port_free`, so reusing the lock cannot
    /// double-bind — but `expect`ing here would cascade one failure
    /// into all four port tests.
    fn lock_forwarder_port() -> std::sync::MutexGuard<'static, ()> {
        UDP_FORWARDER_PORT
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Waits until the fixed UDP stream port (4211) is free: the
    /// previous forwarder test's socket lingers ~100–150 ms after its
    /// engine stops (the reader thread exits on the shutdown flag),
    /// and frames sent while the dead socket still holds the port are
    /// swallowed by it instead of reaching the new forwarder. Probing
    /// with a throwaway bind is the direct check — a successful probe
    /// bind means nobody holds the port. Returns false after ~5 s
    /// (the caller fails loudly instead of testing against a stale
    /// socket).
    fn wait_for_stream_port_free() -> bool {
        wait_until(Duration::from_secs(5), || {
            std::net::UdpSocket::bind("0.0.0.0:4211").is_ok()
        })
    }

    /// Sends one loopback datagram, retrying while the forwarder has
    /// not bound the stream port yet: a *connected* UDP socket
    /// reports `ConnectionRefused` until something listens, and the
    /// supervisor binds asynchronously, so a fixed sleep is a race
    /// under load. Panics after ~5 s or on any other send error.
    fn send_loopback_frame(board: &std::net::UdpSocket, frame: &[u8]) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            match board.send(frame) {
                Ok(_) => return,
                Err(error)
                    if error.kind() == std::io::ErrorKind::ConnectionRefused
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(error) => panic!("fake board sends: {error}"),
            }
        }
    }

    #[test]
    fn udp_forwarder_delivers_loopback_frames_to_backend() {
        let _port = lock_forwarder_port();
        assert!(
            wait_for_stream_port_free(),
            "stream port 4211 still held by a previous test's transport"
        );
        let (state, sent, _fail) = test_state_with_recording();
        // Point the board at loopback so the fake board below is the
        // expected sender (the sender filter compares IP only).
        let mut settings = state.settings();
        settings.esp_ip = "127.0.0.1".to_string();
        state.set_settings(settings).expect("settings save");
        let _ = state.set_output_enabled(true);
        state
            .connect_transport(TransportKind::Udp)
            .expect("UDP connect queues");
        // H1 (SEC-E-14): frames only flow once the session gate is
        // open — the test stands in for the session owner here.
        state.open_session_gate();
        assert!(
            state.transport_state().session_active,
            "gate open: {}",
            state.engine_status_detail()
        );
        // Give the forwarder time to bind the stream port.
        std::thread::sleep(Duration::from_millis(300));
        // Fake board: a loopback socket sending stream frames at the
        // transport's fixed stream port (4211).
        let board = std::net::UdpSocket::bind("127.0.0.1:0").expect("fake board binds");
        board
            .connect("127.0.0.1:4211")
            .expect("fake board connects");
        // Re-send until a frame lands: one burst can be dropped
        // under parallel load (same as session_gate Phase 2,
        // d59c89b), so keep sending while waiting instead of
        // fire-once-then-wait. Same 20 s bound as before.
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let mut seq: u16 = 0;
        while !recorded_states(&sent).iter().any(|s| s.steering > 0.9) {
            if std::time::Instant::now() >= deadline {
                break;
            }
            let frame = test_stream_frame(seq, 3300, &[]);
            send_loopback_frame(&board, &frame.encode());
            seq = seq.wrapping_add(1);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            recorded_states(&sent).iter().any(|s| s.steering > 0.9),
            "loopback UDP frames reached the backend: {:?}",
            recorded_states(&sent)
        );
        assert_eq!(
            state.engine_status(),
            EngineTaskStatus::Running {
                transport: Some(TransportKind::Udp),
            },
            "UDP transport live: {}",
            state.engine_status_detail()
        );
        let outcome = state.stop_engine();
        assert!(outcome.joined, "engine stops bounded after UDP");
        assert!(
            outcome.report.expect("report").counters.frames_processed >= 1,
            "at least one loopback frame processed"
        );
    }

    /// H1 (SEC-E-14 + conditions 11/12): one loopback run proving
    /// the session gate, the status pump, and the gate lifecycle.
    /// Serialized against `udp_forwarder_delivers_loopback_frames_to_backend`
    /// by `UDP_FORWARDER_PORT` — both bind the fixed stream port 4211.
    #[test]
    fn session_gate_holds_frames_status_pump_and_disconnect() {
        use rsh_protocol::stream::StatusPacket;
        let _port = lock_forwarder_port();
        assert!(
            wait_for_stream_port_free(),
            "stream port 4211 still held by a previous test's transport"
        );
        let (state, sent, _fail) = test_state_with_recording();
        let mut settings = state.settings();
        settings.esp_ip = "127.0.0.1".to_string();
        state.set_settings(settings).expect("settings save");
        let _ = state.set_output_enabled(true);
        state
            .connect_transport(TransportKind::Udp)
            .expect("UDP connect queues");
        std::thread::sleep(Duration::from_millis(300));
        let board = std::net::UdpSocket::bind("127.0.0.1:0").expect("fake board binds");
        board
            .connect("127.0.0.1:4211")
            .expect("fake board connects");
        // Phase 1 — gate closed: frames are held, counted, and reach
        // neither the backend nor the engine. Re-send until the held
        // count lands: one burst can be dropped under parallel load,
        // same as the Phase 2 delivery below.
        let mut seq: u16 = 0;
        let phase1_deadline = std::time::Instant::now() + Duration::from_secs(20);
        while state.session_gate_held_frames() < 10 {
            if std::time::Instant::now() >= phase1_deadline {
                break;
            }
            let frame = test_stream_frame(seq, 3300, &[]);
            send_loopback_frame(&board, &frame.encode());
            seq = seq.wrapping_add(1);
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(300));
        // The engine still emits its own neutral idle ticks while the
        // gate is closed — the gate proof is that every recorded
        // state is neutral (no transport frame reached the engine).
        assert!(
            recorded_states(&sent)
                .iter()
                .all(|s| *s == PipelineOutputState::neutral()),
            "closed gate must hold every frame: {:?}",
            recorded_states(&sent)
        );
        // H5: the forwarder runs the session activator first (~1 s
        // HELLO wait with no board answering), so the held count
        // lands once the frame loop starts — the frames sit in the
        // transport channel until then, still held, never leaked.
        // The 20 s bound is sandbox margin (timers run ~2–5× slow
        // under parallel load); the assertion itself is unchanged.
        assert!(
            wait_until(Duration::from_secs(20), || state.session_gate_held_frames()
                >= 10),
            "held frames must be counted: {}",
            state.session_gate_held_frames()
        );
        assert!(
            !state.transport_state().session_active,
            "gate closed until a session authenticates"
        );
        // Phase 2 — gate open: frames flow to the backend, and a
        // status packet pumped after a frame reaches the snapshot.
        state.open_session_gate();
        let status = StatusPacket {
            seq: 0,
            uptime_ms: 0,
            rssi: 0,
            battery_pct: 255,
            supply_mv: 0,
            heap: 0,
            overruns: 0,
            max_cycle_us: 0,
            governor: 0,
            reset_reason: 1,
            cfg_crc: 0,
            cfg_ver: 0,
            seq_gap: 7,
            tx_fail: 3,
            btns_ex: None,
        };
        send_loopback_frame(&board, &status.encode());
        // Re-send until a frame lands: a single burst can be
        // dropped (channel full / bind race under parallel load on
        // Windows CI), so keep sending while waiting instead of
        // fire-once-then-wait. Same 20 s bound as before.
        // `seq` continues from Phase 1 so no sequence number is
        // ever sent twice (the engine drops duplicates).
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while !recorded_states(&sent).iter().any(|s| s.steering > 0.9) {
            if std::time::Instant::now() >= deadline {
                break;
            }
            let frame = test_stream_frame(seq, 3300, &[]);
            send_loopback_frame(&board, &frame.encode());
            seq = seq.wrapping_add(1);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            recorded_states(&sent).iter().any(|s| s.steering > 0.9),
            "open gate must deliver frames: {:?}",
            recorded_states(&sent)
        );
        assert!(
            wait_until(Duration::from_secs(20), || state.snapshot().seq_gap == 7
                && state.snapshot().tx_fail == 3),
            "status pump must forward seq_gap/tx_fail: seq_gap={} tx_fail={}",
            state.snapshot().seq_gap,
            state.snapshot().tx_fail
        );
        // Phase 3 — disconnect closes the gate: a later reconnect is
        // a new session and must reconnect.
        let _ = state.disconnect_transport();
        assert!(
            !state.session_gate_open(),
            "disconnect must close the session gate"
        );
        let outcome = state.stop_engine();
        assert!(outcome.joined, "engine stops bounded after UDP");
    }

    /// H1 condition 11: the sink feeds the engine's live per-tick
    /// metrics into the snapshot (not "unmeasured" forever).
    #[test]
    fn per_tick_metrics_reach_the_snapshot() {
        let (state, _sent, _fail) = test_state_with_recording();
        let mut metrics = EngineMetrics::new();
        metrics.record_inter_arrival_jitter(100);
        metrics.record_output_latency(200);
        let output = PipelineOutputState::neutral();
        state.ingest_engine_output(&output, &metrics);
        let snapshot_stats = state.snapshot().stats;
        assert_eq!(
            snapshot_stats.jitter_ms_p99,
            Some(0.1),
            "jitter p99 100 µs must read 0.1 ms: {snapshot_stats:?}"
        );
        assert_eq!(
            snapshot_stats.output_latency_ms,
            Some(0.2),
            "output latency p99 200 µs must read 0.2 ms: {snapshot_stats:?}"
        );
    }

    #[test]
    fn serial_connect_without_board_fails_honestly_or_runs_idle() {
        // Sandbox/CI machines usually have no serial board: the
        // connect errors honestly and the engine never starts. On a
        // machine with a serial port it starts the engine instead —
        // either way the outcome is honest and the stop is bounded.
        let (state, _sent, _fail) = test_state_with_recording();
        if let Err(error) = state.connect_transport(TransportKind::Serial) {
            assert!(error.contains("no board serial port"), "{error}");
            assert_eq!(state.engine_status(), EngineTaskStatus::Idle);
            let outcome = state.stop_engine();
            assert!(!outcome.joined, "never started, nothing to join");
        } else {
            assert_eq!(
                state.engine_status(),
                EngineTaskStatus::Running { transport: None },
                "serial forwarder retries with no board yet: {}",
                state.engine_status_detail()
            );
            let _ = state.disconnect_transport();
            let outcome = state.stop_engine();
            assert!(outcome.joined, "engine stops bounded");
        }
    }

    // --- H3: transport request record for the Settings / Setup screens. ---

    #[test]
    fn transport_request_defaults_to_udp_with_esp_ip() {
        let (state, _dir) = test_state();
        assert_eq!(
            state.transport_request(),
            (TransportKind::Udp, "192.168.4.1".to_string())
        );
    }

    #[test]
    fn transport_request_records_requested_serial_address() {
        let (state, _dir) = test_state();
        state.set_transport_request(TransportKind::Serial, "COM3".to_string());
        assert_eq!(
            state.transport_request(),
            (TransportKind::Serial, "COM3".to_string())
        );
    }

    // --- H3: get_transport_state shape (merged UI + session gate). ---

    #[test]
    fn transport_state_fresh_is_disconnected_wifi_with_esp_ip() {
        let (state, _dir) = test_state();
        let reported = state.transport_state();
        assert_eq!(reported.transport, "wifi");
        assert_eq!(reported.state, ConnectionState::Disconnected);
        assert_eq!(reported.address, "192.168.4.1");
        assert_eq!(reported.error, None);
        assert!(!reported.session_active);
        assert_eq!(reported.frames_held_by_gate, 0);
    }

    #[test]
    fn transport_state_idle_reports_requested_serial_kind() {
        let (state, _dir) = test_state();
        state.set_transport_request(TransportKind::Serial, "COM3".to_string());
        let reported = state.transport_state();
        assert_eq!(reported.transport, "usb");
        assert_eq!(reported.state, ConnectionState::Disconnected);
    }

    /// DS-27: repeated serial `transport_state()` calls must reuse the
    /// cached board port (no board is attached in the test sandbox, so
    /// the address stays empty) and `rescan_board_port()` must clear
    /// the cache without error. The live cost (port enumeration +
    /// up to 500 ms HELO probe per port) is unmeasurable here — the
    /// proof is that `transport_state` no longer calls
    /// `detect_board_port` directly.
    #[test]
    fn ds27_serial_transport_state_uses_cached_port() {
        let (state, _dir) = test_state();
        state.set_transport_request(TransportKind::Serial, "COM3".to_string());
        assert_eq!(state.cached_board_port(), None);
        assert_eq!(state.cached_board_port(), None);
        let reported = state.transport_state();
        assert_eq!(reported.transport, "usb");
        assert_eq!(reported.address, "");
        state.rescan_board_port();
        assert_eq!(state.cached_board_port(), None);
        assert_eq!(state.transport_state().address, "");
    }

    /// PERF F-5: the board-port refresher's cadence — Wi-Fi never
    /// probes (30 s idle wait), Serial re-probes every 5 s while
    /// the board is found, and backs off 10 s → 20 s → 30 s while
    /// it keeps missing.
    #[test]
    fn board_port_refresh_delay_backs_off_on_serial_misses() {
        assert_eq!(
            AppStateCore::board_port_refresh_delay(false, 0),
            Duration::from_secs(30),
            "Wi-Fi never probes: the wait is the backoff cap"
        );
        assert_eq!(
            AppStateCore::board_port_refresh_delay(false, 5),
            Duration::from_secs(30)
        );
        assert_eq!(
            AppStateCore::board_port_refresh_delay(true, 0),
            Duration::from_secs(5),
            "Serial with a board found: 5 s, as before"
        );
        assert_eq!(
            AppStateCore::board_port_refresh_delay(true, 1),
            Duration::from_secs(10)
        );
        assert_eq!(
            AppStateCore::board_port_refresh_delay(true, 2),
            Duration::from_secs(20)
        );
        assert_eq!(
            AppStateCore::board_port_refresh_delay(true, 3),
            Duration::from_secs(30)
        );
        assert_eq!(
            AppStateCore::board_port_refresh_delay(true, 100),
            Duration::from_secs(30),
            "the backoff caps at 30 s, it never grows past it"
        );
    }

    /// PERF F-5: on the default Wi-Fi transport the refresher
    /// thread never touches the serial stack — the cache stays
    /// untouched (still `None`, not even a probed-`None` entry)
    /// while the thread is alive and waiting on its condvar.
    #[test]
    fn board_port_refresher_never_probes_on_wifi() {
        let (state, _dir) = test_state();
        state.start_board_port_refresher();
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            lock(&state.cached_serial_port).is_none(),
            "Wi-Fi: no probe ran, the cache is still untouched"
        );
        // A rescan request wakes the thread at once — it probes
        // once (no board here, so the cache fills with a
        // probed-`None`) and then goes back to waiting.
        state.rescan_board_port();
        assert!(
            wait_until(Duration::from_secs(2), || lock(&state.cached_serial_port)
                .is_some()),
            "a rescan request must wake the refresher promptly"
        );
    }

    /// PERF F-5: on Serial the refresher probes promptly (the
    /// cache fills within 2 s even with no board attached — the
    /// entry is a probed-`None`, not an untouched cache).
    #[test]
    fn board_port_refresher_probes_promptly_on_serial() {
        let (state, _dir) = test_state();
        state.set_transport_request(TransportKind::Serial, String::new());
        state.start_board_port_refresher();
        assert!(
            wait_until(Duration::from_secs(2), || lock(&state.cached_serial_port)
                .is_some()),
            "Serial: the first probe must run at once, not after a 30 s wait"
        );
    }

    /// PERF F-6: the slow-poll generation moves on every mutation
    /// the slow poll reports, and only on success — a failed
    /// attempt must not invalidate the daemon's cache for nothing.
    #[test]
    fn slow_state_generation_bumps_on_mutation_not_on_failure() {
        let (state, _dir) = test_state();
        let first = state
            .list_profiles()
            .into_iter()
            .next()
            .expect("default profile exists");
        state
            .activate_profile(&first.id)
            .expect("activate default profile");
        let gen0 = state.slow_state_generation();
        state.create_profile("Second").expect("create second");
        assert!(state.slow_state_generation() > gen0);
        let gen1 = state.slow_state_generation();
        assert!(
            state.delete_profile("missing-id").is_err(),
            "deleting an unknown profile must fail"
        );
        assert_eq!(
            state.slow_state_generation(),
            gen1,
            "a failed delete must not bump the generation"
        );
        let tuning = TuningSettings::neutral("steering");
        state.set_tuning(&tuning).expect("set tuning");
        assert!(state.slow_state_generation() > gen1);
        let gen2 = state.slow_state_generation();
        state.set_binding(5, "horn").expect("bind 5");
        assert!(state.slow_state_generation() > gen2);
    }

    /// DS-30: the `Arc` settings cache tracks every writer, and the
    /// `Arc` snapshot agrees with the struct read. The cache starts
    /// from the store; `set_settings`, `set_settings_patch`, and
    /// `set_output_backend` (which persists before switching — the
    /// switch itself fails on Linux without a vJoy driver, but the
    /// persisted choice must still reach the cache) all refresh it.
    #[test]
    fn ds30_settings_arc_tracks_writes_and_snapshot_arc_agrees() {
        let (state, _dir) = test_state();
        assert_eq!(state.settings_arc().esp_ip, state.settings().esp_ip);
        let mut settings = state.settings();
        settings.esp_ip = "10.0.0.5".to_string();
        let saved = state.set_settings(settings).expect("set settings");
        assert_eq!(saved.esp_ip, "10.0.0.5");
        assert_eq!(state.settings_arc().esp_ip, "10.0.0.5");
        let patch = AppSettingsPatch {
            esp_ip: Some("10.0.0.9".to_string()),
            ..Default::default()
        };
        let merged = state.set_settings_patch(patch).expect("patch");
        assert_eq!(merged.esp_ip, "10.0.0.9");
        assert_eq!(state.settings_arc().esp_ip, "10.0.0.9");
        let _ = state.set_output_backend("vJoy");
        assert_eq!(state.settings_arc().output_backend, "VJoy");
        assert_eq!(*state.snapshot_arc(), state.snapshot());
        // The settings-aware transport state reads the same `esp_ip`
        // the `Arc` carries — no second store clone.
        assert_eq!(
            state
                .transport_state_with_settings(&state.settings_arc())
                .address,
            "10.0.0.9"
        );
    }

    /// DS-32: `take_connection_log_lines()` drains once — the first
    /// drain returns the queued lines, the second returns nothing,
    /// and the read-back buffer is empty (the manager loop can no
    /// longer re-log a line or re-clone the buffer per tick).
    #[test]
    fn ds32_take_connection_log_lines_drains_once() {
        let (state, _dir) = test_state();
        assert_eq!(state.take_connection_log_lines(), Vec::<String>::new());
        state.open_session_gate();
        let drained = state.take_connection_log_lines();
        assert_eq!(drained.len(), 1);
        assert!(
            drained[0].contains("session gate: closed -> open"),
            "{}",
            drained[0]
        );
        assert_eq!(state.take_connection_log_lines(), Vec::<String>::new());
        assert_eq!(state.connection_log_lines(), Vec::<String>::new());
    }

    /// R2-23/R2-42: the retained tail survives the manager-loop
    /// drain (a diagnostics read must still see the trail) and is
    /// capped at [`CONNECTION_LOG_TAIL_LINES`] lines.
    #[test]
    fn r2_connection_log_tail_survives_drain_and_caps_at_20() {
        let (state, _dir) = test_state();
        for _ in 0..25 {
            state.open_session_gate();
            state.close_session_gate();
        }
        // 50 lines pushed; the drain takes them all from the main
        // buffer — but the tail keeps the last 20.
        assert_eq!(state.take_connection_log_lines().len(), 50);
        assert_eq!(state.connection_log_lines(), Vec::<String>::new());
        let tail = state.connection_log_tail_lines();
        assert_eq!(tail.len(), CONNECTION_LOG_TAIL_LINES);
        assert!(
            tail.last().is_some_and(|l| l.contains("open -> closed")),
            "tail ends with the newest line: {tail:?}"
        );
        assert!(
            tail.first().is_some_and(|l| l.contains("closed -> open")),
            "tail starts with the oldest retained line: {tail:?}"
        );
    }

    #[test]
    fn transport_state_failed_engine_reports_error_verbatim() {
        let (state, _dir) = test_state();
        state.set_engine_status(
            EngineTaskStatus::Failed {
                reason: "engine supervisor died".to_string(),
            },
            "engine supervisor died".to_string(),
        );
        let reported = state.transport_state();
        assert_eq!(reported.state, ConnectionState::Error);
        assert_eq!(reported.error.as_deref(), Some("engine supervisor died"));
    }

    #[test]
    fn transport_state_serializes_to_frontend_shape() {
        let (state, _dir) = test_state();
        let json = serde_json::to_string(&state.transport_state()).expect("serialize");
        assert!(json.contains("\"transport\":\"wifi\""), "{json}");
        assert!(json.contains("\"state\":\"disconnected\""), "{json}");
        assert!(json.contains("\"address\":\"192.168.4.1\""), "{json}");
        assert!(json.contains("\"error\":null"), "{json}");
    }

    // --- H3: corrupt-store repair at open (Phase G condition 16). ---

    #[test]
    fn open_with_repair_repairs_corrupt_store_from_bak1() {
        let dir = test_dir("repair");
        {
            let mut store = ProfileStore::open(&dir).expect("open test store");
            store.create("GT3 Setup").expect("create profile");
            store.save().expect("save");
        }
        std::fs::copy(dir.join("profiles.json"), dir.join("profiles.json.bak1"))
            .expect("stage bak1");
        std::fs::write(dir.join("profiles.json"), "{not json").expect("corrupt");
        assert!(ProfileStore::open(&dir).is_err());
        let (opened, notice) = AppStateCore::open_with_null_sink_repair(&dir);
        let state = opened.expect("repaired open succeeds");
        let notice = notice.expect("repair notice");
        assert!(notice.contains("bak1"), "{notice}");
        assert!(state.list_profiles().iter().any(|p| p.name == "GT3 Setup"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_with_repair_fails_honestly_without_backups() {
        let dir = test_dir("repair-none");
        std::fs::write(dir.join("profiles.json"), "{not json").expect("corrupt");
        let (opened, notice) = AppStateCore::open_with_null_sink_repair(&dir);
        assert!(opened.is_err());
        assert_eq!(notice, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- H5: session-owner activator (SEC-E-14). ---

    /// Board MAC used by the activator tests (and the matching board id).
    const ACT_BOARD_ID: &str = "AA:BB:CC:DD:EE:FF";
    /// Board MAC bytes.
    const ACT_BOARD_MAC: [u8; 6] = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
    /// No device keys / nonces exist in v2.2: the activator tests
    /// need no key material at all — `connect()` is unsigned.
    /// In-memory [`KeyStoreLike`] for the activator tests (never
    /// touches the OS store). Entries live behind an [`Arc`] so the
    /// test keeps a handle to the store the activator consumed.
    /// One entry per (service, account) pair, keyed exactly like the
    /// real `KeyStore` (`"RSH"`, `"board:<BOARD_ID>"`).
    type ActStoreEntries = HashMap<(String, String), Vec<u8>>;

    #[derive(Clone)]
    struct ActKeyStore {
        entries: Arc<Mutex<ActStoreEntries>>,
        /// R3-D-10: when set, `get_wifi_password` fails with
        /// `PlatformUnavailable` (a dead credential store) instead
        /// of reading — the `NotFound` path stays untouched.
        fail_get: Arc<std::sync::atomic::AtomicBool>,
    }

    impl ActKeyStore {
        fn new() -> Self {
            Self {
                entries: Arc::new(Mutex::new(HashMap::new())),
                fail_get: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            }
        }

        /// Make the next `get_wifi_password` fail as if the platform
        /// credential store were unavailable (R3-D-10).
        fn set_fail_get(&self, fail: bool) {
            self.fail_get
                .store(fail, std::sync::atomic::Ordering::SeqCst);
        }

        /// The stored Wi-Fi password for `board_id`, if one was stored
        /// (test inspection — delegates to the [`KeyStoreLike`] getter).
        fn wifi_password(&self, board_id: &str) -> Option<String> {
            self.get_wifi_password(board_id).ok()
        }
    }

    impl KeyStoreLike for ActKeyStore {
        // No-pairing redesign: the trait is Wi-Fi-password-only
        // (WS-2 contract) — no device-key methods exist anymore.
        fn store_wifi_password(&self, board_id: &str, password: &str) -> Result<(), KeystoreError> {
            let account = format!("board:{}:wifi", board_id.trim().to_ascii_uppercase());
            self.entries
                .lock()
                .expect("test store lock")
                .insert(("RSH".to_owned(), account), password.as_bytes().to_vec());
            Ok(())
        }

        fn get_wifi_password(&self, board_id: &str) -> Result<String, KeystoreError> {
            if self.fail_get.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(KeystoreError::PlatformUnavailable);
            }
            let account = format!("board:{}:wifi", board_id.trim().to_ascii_uppercase());
            self.entries
                .lock()
                .expect("test store lock")
                .get(&("RSH".to_owned(), account))
                .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                .ok_or(KeystoreError::NotFound)
        }

        fn delete_wifi_password(&self, board_id: &str) -> Result<(), KeystoreError> {
            let account = format!("board:{}:wifi", board_id.trim().to_ascii_uppercase());
            self.entries
                .lock()
                .expect("test store lock")
                .remove(&("RSH".to_owned(), account))
                .map_or(Err(KeystoreError::NotFound), |_| Ok(()))
        }
    }

    /// The fake board's config state, shared across the fresh sessions
    /// of one test (one board, many sessions): what `GET_CFG`
    /// returns, the last-good blob `REVERT` restores, and the staged
    /// buffer `SET_CFG` accumulates into. `COMMIT` live-applies the
    /// staged blob (so `GET_CFG` returns it while the confirm window
    /// runs); `CONFIRM` makes it the last-good blob; `REVERT`
    /// restores the last-good blob and discards the staged buffer.
    struct FakeBoardConfig {
        /// What `GET_CFG` returns.
        current: Vec<u8>,
        /// The last-good blob (`CONFIRM` saves it, `REVERT` restores it).
        last_good: Vec<u8>,
        /// Bytes staged by `SET_CFG` so far.
        staged: Vec<u8>,
        /// Total length the staged blob will have (0 = nothing staged).
        staged_total: usize,
    }

    impl FakeBoardConfig {
        /// The factory blob as both the live and the last-good config.
        fn new() -> Self {
            let blob = rsh_protocol::factory_default_blob()
                .encode()
                .expect("factory blob encodes");
            Self {
                current: blob.clone(),
                last_good: blob,
                staged: Vec::new(),
                staged_total: 0,
            }
        }
    }

    /// A fake board as a session transport: answers `HELLO`s from a
    /// configured reply, verifies control frames by CRC (protocol
    /// v2.2 — no key, no tag), and serves stream frames from a
    /// channel. The `HELLO` behaviour is switchable per test:
    /// answer always, never answer (forces the activator's
    /// timeout), or report the transport closed.
    ///
    /// The sent control frames and the board config live behind
    /// [`Arc`]s so a test keeps them after the transport is consumed
    /// by a control session (the helper drops the transport when the
    /// session closes): [`FakeSessionTransport::sent_log`] for the
    /// frames, [`FakeSessionTransport::board`] for the config — the
    /// board handle can also be shared across several fresh sessions
    /// against the "same" board via [`FakeSessionTransport::new_with_board`].
    struct FakeSessionTransport {
        hello: Option<HelloReply>,
        sent: Arc<Mutex<Vec<Vec<u8>>>>,
        board: Arc<Mutex<FakeBoardConfig>>,
        pending: VecDeque<Vec<u8>>,
        frame_rx: tokio::sync::mpsc::Receiver<StreamFrame>,
        status_rx: tokio::sync::mpsc::Receiver<StatusPacket>,
        helo_requests: Vec<u8>,
        never_answer_hello: bool,
        hello_closed: bool,
    }

    impl FakeSessionTransport {
        fn new(hello: Option<HelloReply>) -> Self {
            Self::new_with_board(hello, Arc::new(Mutex::new(FakeBoardConfig::new())))
        }

        /// Build a fake transport against a shared board (several
        /// fresh sessions, one board — the H5-control tests use this
        /// to run stage → commit → confirm across sessions).
        fn new_with_board(hello: Option<HelloReply>, board: Arc<Mutex<FakeBoardConfig>>) -> Self {
            let (_frame_tx, frame_rx) = tokio::sync::mpsc::channel(16);
            let (_status_tx, status_rx) = tokio::sync::mpsc::channel(16);
            Self {
                hello,
                sent: Arc::new(Mutex::new(Vec::new())),
                board,
                pending: VecDeque::new(),
                frame_rx,
                status_rx,
                helo_requests: Vec::new(),
                never_answer_hello: false,
                hello_closed: false,
            }
        }

        /// The sent control frames (shared: survives the transport).
        fn sent_log(&self) -> Arc<Mutex<Vec<Vec<u8>>>> {
            Arc::clone(&self.sent)
        }

        /// The control frames the activator sent, parsed.
        fn sent_frames(&self) -> Vec<rsh_protocol::ControlFrame> {
            let sent = self.sent.lock().expect("sent log lock");
            sent.iter()
                .map(|bytes| {
                    rsh_protocol::ControlFrame::parse_structure(bytes).expect("sent frame parses")
                })
                .collect()
        }

        /// Answer one `GET_CFG` from the board's live blob: the reply
        /// body is `totalLen u16 | offset u16 | data…`, data clamped
        /// to the request and to the board's 1019 B per-reply cap.
        fn board_get_cfg(&self, payload: &[u8]) -> Vec<u8> {
            if payload.len() < 4 {
                return Vec::new();
            }
            let offset = usize::from(u16::from_le_bytes([payload[0], payload[1]]));
            let want = usize::from(u16::from_le_bytes([payload[2], payload[3]]));
            let board = self.board.lock().expect("board lock");
            let total = board.current.len();
            let available = total.saturating_sub(offset);
            let take = want.min(available).min(1019);
            let mut body = Vec::with_capacity(4 + take);
            body.extend_from_slice(&u16::try_from(total).unwrap_or(u16::MAX).to_le_bytes());
            body.extend_from_slice(&u16::try_from(offset).unwrap_or(u16::MAX).to_le_bytes());
            body.extend_from_slice(&board.current[offset..offset + take]);
            body
        }

        /// Stage one `SET_CFG` chunk into the board's staged buffer
        /// (offset 0 opens a fresh staging buffer of `total` bytes).
        fn board_set_cfg(&self, payload: &[u8]) {
            if payload.len() < 8 {
                return;
            }
            let offset = usize::from(u16::from_le_bytes([payload[0], payload[1]]));
            let total = usize::from(u16::from_le_bytes([payload[2], payload[3]]));
            let len = usize::from(u16::from_le_bytes([payload[4], payload[5]]));
            let data = &payload[8..];
            if data.len() < len {
                return;
            }
            let mut board = self.board.lock().expect("board lock");
            if offset == 0 {
                board.staged = vec![0u8; total];
                board.staged_total = total;
            }
            if offset + len <= board.staged.len() {
                board.staged[offset..offset + len].copy_from_slice(&data[..len]);
            }
        }

        /// `COMMIT`: live-apply the staged blob (the board runs it
        /// while the confirm window is open).
        fn board_commit(&self) {
            let mut board = self.board.lock().expect("board lock");
            if board.staged_total > 0 && board.staged.len() >= board.staged_total {
                board.current = board.staged[..board.staged_total].to_vec();
            }
        }

        /// `CONFIRM`: the live blob becomes the last-good blob.
        fn board_confirm(&self) {
            let mut board = self.board.lock().expect("board lock");
            board.last_good = board.current.clone();
        }

        /// `REVERT`: restore the last-good blob, discard the staged buffer.
        fn board_revert(&self) {
            let mut board = self.board.lock().expect("board lock");
            board.current = board.last_good.clone();
            board.staged.clear();
            board.staged_total = 0;
        }
    }

    impl PacketSource for FakeSessionTransport {
        fn next_frame(&mut self) -> impl Future<Output = Option<StreamFrame>> + Send {
            self.frame_rx.recv()
        }

        fn try_next_status(&mut self) -> Option<StatusPacket> {
            self.status_rx.try_recv().ok()
        }
    }

    impl rsh_transport::ControlTransport for FakeSessionTransport {
        fn send_control(
            &mut self,
            bytes: &[u8],
        ) -> impl Future<Output = Result<(), rsh_transport::TransportError>> + Send {
            let owned = bytes.to_vec();
            async move {
                self.sent.lock().expect("sent log lock").push(owned.clone());
                // v2.2: frames are verified by CRC only — a bad CRC
                // is dropped silently (never executed, never
                // answered), like the real board.
                let Ok(frame) = rsh_protocol::ControlFrame::verify(&owned) else {
                    return Ok(());
                };
                let body = match frame.cmd {
                    cmd if cmd == rsh_protocol::control::Opcode::GetCaps as u8 => act_caps_body(),
                    cmd if cmd == rsh_protocol::control::Opcode::GetCfg as u8 => {
                        self.board_get_cfg(&frame.payload)
                    }
                    cmd if cmd == rsh_protocol::control::Opcode::SetCfg as u8 => {
                        self.board_set_cfg(&frame.payload);
                        Vec::new()
                    }
                    cmd if cmd == rsh_protocol::control::Opcode::Commit as u8 => {
                        self.board_commit();
                        vec![30]
                    }
                    cmd if cmd == rsh_protocol::control::Opcode::Confirm as u8 => {
                        self.board_confirm();
                        Vec::new()
                    }
                    cmd if cmd == rsh_protocol::control::Opcode::Revert as u8 => {
                        self.board_revert();
                        Vec::new()
                    }
                    _ => Vec::new(),
                };
                let reply = rsh_protocol::control::build_reply(
                    frame.cmd,
                    frame.req_id,
                    rsh_protocol::control::ErrorCode::Ok,
                    &body,
                )
                .expect("fake-board reply builds");
                self.pending.push_back(reply);
                Ok(())
            }
        }

        fn recv_control(
            &mut self,
            _timeout: Duration,
        ) -> impl Future<Output = Result<Vec<u8>, rsh_transport::TransportError>> + Send {
            std::future::ready(
                self.pending
                    .pop_front()
                    .ok_or(rsh_transport::TransportError::Timeout),
            )
        }
    }

    impl SessionTransport for FakeSessionTransport {
        fn recv_hello(&mut self) -> impl Future<Output = Option<HelloReply>> + Send {
            let hello = self.hello.clone();
            let closed = self.hello_closed;
            let never = self.never_answer_hello;
            async move {
                if closed {
                    None
                } else if never {
                    std::future::pending().await
                } else {
                    hello
                }
            }
        }

        fn request_hello(&mut self, session_flag: u8) -> Result<(), String> {
            self.helo_requests.push(session_flag);
            Ok(())
        }
    }

    /// A `GET_CAPS` body *after* the status byte: fw 4.0.1, proto 2.2,
    /// 8 axes / 64 buttons / 4 encoders, Wi-Fi channel 6, one pin-table
    /// entry (pin 34). Mirrors the helper in
    /// `rsh-transport/src/session.rs` tests.
    fn act_caps_body() -> Vec<u8> {
        let mut body = GetCapsReply {
            status: rsh_protocol::control::ErrorCode::Ok,
            board_mac: ACT_BOARD_MAC,
            firmware: (4, 0, 1),
            protocol: (2, 2),
            n_axes_max: 8,
            n_btn_max: 64,
            n_enc_max: 4,
            wifi_channel: 6,
            caps_flags: 0,
            pin_table: vec![PinTableEntry {
                pin: 34,
                kind: 1,
                module: 1,
                caps: 0x05,
            }],
        }
        .encode_body();
        body.remove(0);
        body
    }

    /// A v2.2 `HELLO` reply: flags 0 (no pairing bits exist
    /// anymore), `session_echo` 1 (answers the activator's
    /// new-session HELO), zeroed reserved fields.
    fn act_hello() -> HelloReply {
        HelloReply {
            protocol: (2, 2),
            flags: 0,
            session_echo: 1,
            board_mac: ACT_BOARD_MAC,
            firmware: (4, 0, 1),
            nonce: [0; 16],
            device_key: [0; 32],
            wifi_password: [0; 24],
        }
    }

    #[tokio::test]
    async fn activator_connect_opens_gate_after_unsigned_get_caps() {
        let (state, _dir) = test_state();
        assert!(!state.session_gate_open(), "gate starts closed");
        let store = ActKeyStore::new();
        let transport = FakeSessionTransport::new(Some(act_hello()));
        let transport = activate_session_on(&state, transport, store)
            .await
            .expect("transport survives successful activation");
        assert!(
            state.session_gate_open(),
            "gate opens after unsigned GET_CAPS: {:?}",
            state.session_last_error()
        );
        let activation = state.session_activation().expect("activation recorded");
        assert_eq!(activation.board_id, ACT_BOARD_ID);
        assert_eq!(activation.firmware, (4, 0, 1));
        assert_eq!(activation.protocol, (2, 2));
        assert_eq!(state.firmware_version().expect("firmware version"), "4.0.1");
        assert_eq!(state.session_last_error(), None);
        // Unsigned GET_CAPS was the first (and only) control command.
        let frames = transport.sent_frames();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].cmd, rsh_protocol::control::Opcode::GetCaps as u8);
        assert_eq!(frames[0].req_id, 0);
        // The activator asked for a new session (flag-1 HELO).
        assert!(
            transport.helo_requests.contains(&1),
            "new-session HELO requested: {:?}",
            transport.helo_requests
        );
        // The HELLO set the reachable latch (D15).
        assert!(state.board_reachable());
    }

    #[tokio::test]
    async fn activator_connect_opens_gate_without_storing_secrets() {
        // No-pairing redesign: a v2.2 HELLO discloses no key and no
        // password, so activation stores nothing in the keystore —
        // the gate opens on the unsigned GET_CAPS alone.
        let (state, _dir) = test_state();
        let store = ActKeyStore::new();
        let transport = FakeSessionTransport::new(Some(act_hello()));
        let _transport = activate_session_on(&state, transport, store.clone())
            .await
            .expect("transport survives activation");
        assert!(
            state.session_gate_open(),
            "gate opens after unsigned GET_CAPS: {:?}",
            state.session_last_error()
        );
        assert_eq!(
            store.wifi_password(ACT_BOARD_ID),
            None,
            "activation stores no Wi-Fi password"
        );
    }

    /// Phase 2 (Wi-Fi gap): the route check is deterministic for an
    /// unparseable address (fail-closed) and for loopback (a route
    /// always exists). A real board address depends on the host's
    /// routing table, so it is not asserted here — but a policy
    /// block (EPERM) must fail OPEN: in this sandbox `connect()` to
    /// 192.168.4.1 is EPERM although a route exists, and the
    /// daemon trail test (`connection_log_carries_the_attempt_trail`)
    /// proves the HELO/timeout path still runs there instead of a
    /// wrong "no route".
    #[test]
    fn phase2_wifi_route_check() {
        assert!(
            !wifi_route_exists("not-an-ip"),
            "an unparseable board address has no route"
        );
        assert!(
            wifi_route_exists("127.0.0.1"),
            "loopback always has a route"
        );
    }

    #[tokio::test]
    async fn activator_stale_echo_zero_hello_is_used_as_fallback() {
        // The board only ever answers routine flag-0 HELOs (echo 0):
        // the activator falls back to that HELLO instead of failing
        // outright, and the connect still succeeds.
        let (state, _dir) = test_state();
        let store = ActKeyStore::new();
        let mut hello = act_hello();
        hello.session_echo = 0;
        let transport = FakeSessionTransport::new(Some(hello));
        let _transport = activate_session_on(&state, transport, store)
            .await
            .expect("transport survives fallback activation");
        assert!(
            state.session_gate_open(),
            "gate opens on fallback HELLO: {:?}",
            state.session_last_error()
        );
    }

    #[tokio::test]
    async fn activator_no_hello_keeps_gate_closed() {
        let (state, _dir) = test_state();
        let mut transport = FakeSessionTransport::new(None);
        transport.never_answer_hello = true;
        let started = Instant::now();
        let _transport = activate_session_on(&state, transport, ActKeyStore::new())
            .await
            .expect("transport survives HELLO timeout");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "activation is bounded: {:?}",
            started.elapsed()
        );
        assert!(
            !state.session_gate_open(),
            "gate stays closed with no HELLO"
        );
        assert!(
            !state.board_reachable(),
            "no HELLO means the reachable latch stays clear (D15)"
        );
        let error = state.session_last_error().expect("failure recorded");
        assert!(
            error.contains("no HELLO"),
            "timeout reason recorded: {error}"
        );
    }

    #[tokio::test]
    async fn activator_transport_closed_returns_none() {
        let (state, _dir) = test_state();
        let mut transport = FakeSessionTransport::new(None);
        transport.hello_closed = true;
        let result = activate_session_on(&state, transport, ActKeyStore::new()).await;
        assert!(result.is_none(), "dead transport returns None");
        assert!(!state.session_gate_open());
        assert!(
            state
                .session_last_error()
                .expect("failure recorded")
                .contains("transport closed"),
            "closed reason recorded"
        );
    }

    #[tokio::test]
    async fn activator_disconnect_closes_gate_and_clears_activation() {
        let (state, _dir) = test_state();
        state.start_engine().expect("engine starts");
        let store = ActKeyStore::new();
        let transport = FakeSessionTransport::new(Some(act_hello()));
        let _transport = activate_session_on(&state, transport, store)
            .await
            .expect("activation succeeds");
        assert!(state.session_gate_open());
        assert!(state.session_activation().is_some());
        assert!(state.board_reachable());
        state.disconnect_transport().expect("disconnect queues");
        assert!(!state.session_gate_open(), "disconnect closes the gate");
        assert!(
            state.session_activation().is_none(),
            "disconnect clears the activation"
        );
        assert!(
            !state.board_reachable(),
            "disconnect clears the reachable latch (D15)"
        );
        assert!(
            state.firmware_version().is_err(),
            "no firmware version after disconnect"
        );
        let outcome = state.stop_engine();
        assert!(outcome.joined, "engine stops bounded");
    }

    /// M-11: the backoff delay doubles per consecutive failure
    /// (2, 4, 8, 16 s) and caps at 30 s.
    #[test]
    fn forwarder_backoff_delay_doubles_to_30s_cap() {
        let policy = ForwarderRetryPolicy::default_policy();
        assert_eq!(policy.delay_after(1), Duration::from_secs(2));
        assert_eq!(policy.delay_after(2), Duration::from_secs(4));
        assert_eq!(policy.delay_after(3), Duration::from_secs(8));
        assert_eq!(policy.delay_after(4), Duration::from_secs(16));
        assert_eq!(policy.delay_after(5), Duration::from_secs(30));
        assert_eq!(policy.delay_after(9), Duration::from_secs(30));
        assert_eq!(policy.delay_after(100), Duration::from_secs(30));
    }

    /// M-11: the breaker trips on the 10th consecutive failure, and
    /// a success resets the streak.
    #[test]
    fn forwarder_backoff_trips_after_max_consecutive_failures() {
        let mut backoff = ForwarderBackoff::new(ForwarderRetryPolicy::default_policy());
        for failure in 1..FORWARDER_MAX_CONSECUTIVE_FAILURES {
            assert!(
                backoff.record_failure().is_some(),
                "failure {failure} still retries"
            );
        }
        assert!(
            backoff.record_failure().is_none(),
            "10th failure trips the breaker"
        );
        assert!(backoff.record_failure().is_none(), "breaker stays open");
        backoff.record_success();
        assert_eq!(
            backoff.record_failure(),
            Some(Duration::from_secs(2)),
            "success resets the streak to the base delay"
        );
    }

    /// M-11: a transport that can never be opened stops retrying
    /// instead of looping forever — the breaker trips, the run
    /// returns, and the connection reports Disconnected.
    #[tokio::test]
    async fn forwarder_with_retry_trips_circuit_breaker_when_transport_never_opens() {
        let (state, _dir) = test_state();
        let (frame_tx, _frame_rx) = tokio::sync::mpsc::channel(8);
        let (status_tx, _status_rx) = tokio::sync::mpsc::channel(8);
        let policy = ForwarderRetryPolicy {
            base: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
            max_consecutive_failures: 3,
        };
        let run = forward_with_retry_with_policy(
            Arc::clone(&state),
            TransportKind::Udp,
            frame_tx,
            status_tx,
            || Option::<FakeSessionTransport>::None,
            policy,
        );
        tokio::time::timeout(Duration::from_secs(5), run)
            .await
            .expect("breaker must trip promptly, not retry forever");
        assert_eq!(state.transport_state().state, ConnectionState::Disconnected);
        assert!(
            state.forwarder_breaker_open(),
            "a tripped breaker must latch forwarder_breaker_open (R3-D-02)"
        );
        assert!(
            state.engine_status_detail().contains("circuit breaker"),
            "breaker named in the status: {}",
            state.engine_status_detail()
        );
    }

    /// DS-06: the classifier behind the self-collision fix — no
    /// holder (port reads free) and our own pid are transient, a
    /// foreign pid is terminal.
    #[test]
    fn udp_addr_in_use_self_or_none_is_transient_foreign_is_terminal() {
        assert!(
            !udp_addr_in_use_is_foreign(None),
            "no holder (port free) is our own rebind race, not a foreign app"
        );
        let own = rsh_transport::port_conflict::PortHolder {
            pid: std::process::id(),
            process_name: "rsh-daemon".to_string(),
            exe_path: None,
        };
        assert!(
            !udp_addr_in_use_is_foreign(Some(&own)),
            "our own pid holding the port is a rebind race, not a conflict"
        );
        let foreign = rsh_transport::port_conflict::PortHolder {
            pid: std::process::id().wrapping_add(1),
            process_name: "RSH.exe".to_string(),
            exe_path: None,
        };
        assert!(
            udp_addr_in_use_is_foreign(Some(&foreign)),
            "a foreign pid is a real port conflict (F21 terminal)"
        );
    }

    /// DS-06: a self-collision `AddrInUse` (the factory's transient
    /// path: `None` with NO typed `BindFailed` recorded, as the
    /// rebind race leaves it) must NOT stop the forwarder at one
    /// attempt — it retries with the M-11 backoff and trips the
    /// breaker after the policy's attempts instead of latching a
    /// false "another app" error.
    #[tokio::test]
    async fn forwarder_retries_self_addr_in_use_instead_of_stopping() {
        let (state, _dir) = test_state();
        let (frame_tx, _frame_rx) = tokio::sync::mpsc::channel(8);
        let (status_tx, _status_rx) = tokio::sync::mpsc::channel(8);
        let policy = ForwarderRetryPolicy {
            base: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
            max_consecutive_failures: 3,
        };
        let attempts = Arc::new(AtomicUsize::new(0));
        let state_for_factory = Arc::clone(&state);
        let attempts_for_factory = Arc::clone(&attempts);
        // The last activation failed Unreachable (HELLO timeout,
        // the no-board case) — the typed error a self-collision
        // leaves behind, since the factory records no BindFailed
        // for its own rebind race.
        state.note_session_error_typed(
            ConnectionError::Unreachable,
            "no HELLO reply from the board",
        );
        let run = forward_with_retry_with_policy(
            Arc::clone(&state),
            TransportKind::Udp,
            frame_tx,
            status_tx,
            move || {
                attempts_for_factory.fetch_add(1, Ordering::SeqCst);
                // DS-06 factory behaviour for a self-held port:
                // no typed BindFailed is recorded.
                let _ = &state_for_factory;
                Option::<FakeSessionTransport>::None
            },
            policy,
        );
        tokio::time::timeout(Duration::from_secs(5), run)
            .await
            .expect("self-collision forwarder must retry, then trip the breaker");
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            3,
            "a self-held port retries to the breaker, it must not stop at one attempt like a foreign conflict"
        );
        assert!(
            state.forwarder_breaker_open(),
            "a tripped breaker must latch forwarder_breaker_open (R3-D-02)"
        );
        assert!(
            state.engine_status_detail().contains("circuit breaker"),
            "breaker named in the status, not a false 'another app': {}",
            state.engine_status_detail()
        );
    }

    /// F21: a UDP port conflict stops the forwarder immediately —
    /// no backoff retries. The holder can only be closed by the
    /// user, so the M-11 retry loop would just re-log the same
    /// failure (user log: 117× `AddrInUse` over ~35 min). The
    /// policy here would allow 3 attempts; the run must make
    /// exactly one and name the port in the status.
    #[tokio::test]
    async fn forwarder_stops_immediately_on_addr_in_use() {
        let (state, _dir) = test_state();
        let (frame_tx, _frame_rx) = tokio::sync::mpsc::channel(8);
        let (status_tx, _status_rx) = tokio::sync::mpsc::channel(8);
        let policy = ForwarderRetryPolicy {
            base: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
            max_consecutive_failures: 3,
        };
        let attempts = Arc::new(AtomicUsize::new(0));
        let state_for_factory = Arc::clone(&state);
        let attempts_for_factory = Arc::clone(&attempts);
        let run = forward_with_retry_with_policy(
            Arc::clone(&state),
            TransportKind::Udp,
            frame_tx,
            status_tx,
            move || {
                attempts_for_factory.fetch_add(1, Ordering::SeqCst);
                state_for_factory.note_session_error_typed(
                    ConnectionError::BindFailed(rsh_api::BindErrorKind::AddrInUse),
                    "UDP bind failed: AddrInUse",
                );
                Option::<FakeSessionTransport>::None
            },
            policy,
        );
        tokio::time::timeout(Duration::from_secs(5), run)
            .await
            .expect("port-conflict forwarder must stop promptly, not retry");
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            1,
            "exactly one bind attempt — no backoff retries on a port conflict"
        );
        assert_eq!(state.transport_state().state, ConnectionState::Disconnected);
        assert!(
            state.forwarder_breaker_open(),
            "a stopped forwarder latches forwarder_breaker_open, so no manager retry respawns it (R3-D-02)"
        );
        assert!(
            state.engine_status_detail().contains("4211"),
            "status names the port: {}",
            state.engine_status_detail()
        );
    }

    #[test]
    fn connect_session_without_board_fails_closed_and_reports() {
        // Full startup path (engine start → UDP connect → activator
        // attempt) with no board answering: the gate stays closed and
        // the failure is recorded instead of silently opening.
        let _port = lock_forwarder_port();
        assert!(
            wait_for_stream_port_free(),
            "stream port 4211 still held by a previous test's transport"
        );
        let (state, _dir) = test_state();
        let mut settings = state.settings();
        settings.esp_ip = "127.0.0.1".to_string();
        state.set_settings(settings).expect("settings save");
        let status = state
            .connect_session()
            .expect("connect_session triggers an attempt");
        assert_ne!(
            status.state,
            ConnectionState::Connected,
            "never Connected while the gate is closed (C-02)"
        );
        assert!(
            wait_until(Duration::from_secs(10), || state
                .session_last_error()
                .is_some()),
            "activation failure must be recorded"
        );
        assert!(
            !state.session_gate_open(),
            "gate stays closed with no board"
        );
        assert!(!state.board_reachable(), "no HELLO, no latch (D15)");
        assert!(state.session_activation().is_none());
        let outcome = state.stop_engine();
        assert!(outcome.joined, "engine stops bounded");
    }

    /// Phase 0 gate §6.3 regression test: a connect attempt with no
    /// board must leave a redacted trail — a `HELO sent` line, the
    /// `HELLO timeout` line, and the typed `activation failed:
    /// reason=Unreachable` line — and the typed error must be
    /// `Unreachable` (not a bare string), so the UI can tell "board
    /// unreachable" from other failures. Also: the transport state
    /// must never report `Connected` while the gate is closed (C-02).
    #[test]
    fn connect_session_without_board_leaves_redacted_log_and_typed_error() {
        let _port = lock_forwarder_port();
        assert!(
            wait_for_stream_port_free(),
            "stream port 4211 still held by a previous test's transport"
        );
        let (state, _dir) = test_state();
        let mut settings = state.settings();
        settings.esp_ip = "127.0.0.1".to_string();
        state.set_settings(settings).expect("settings save");
        let _status = state
            .connect_session()
            .expect("connect_session triggers an attempt");
        assert!(
            wait_until(Duration::from_secs(10), || state
                .session_last_error()
                .is_some()),
            "activation failure must be recorded"
        );
        let lines = state.connection_log_lines();
        let joined = lines.join("\n");
        assert!(
            joined.contains("HELO sent: session_flag=1") || joined.contains("HELO send failed:"),
            "HELO attempt line missing (sent or failed):\n{joined}"
        );
        assert!(
            joined.contains("HELLO timeout after"),
            "HELLO-timeout line missing:\n{joined}"
        );
        assert!(
            joined.contains("activation failed: reason=Unreachable"),
            "typed activation-failed line missing:\n{joined}"
        );
        assert_eq!(
            state.session_last_error_typed(),
            Some(ConnectionError::Unreachable)
        );
        assert_eq!(
            state.session_error_typed(),
            Some(ConnectionError::Unreachable)
        );
        assert_ne!(
            state.transport_state().state,
            ConnectionState::Connected,
            "never Connected while the gate is closed (C-02)"
        );
        assert!(!state.session_gate_open());
        let outcome = state.stop_engine();
        assert!(outcome.joined, "engine stops bounded");
    }

    /// Phase 0 gate §6.3 redaction test: a log line carrying a
    /// planted 24-char password / 32-byte key / HMAC hex must not
    /// leak the secret into the connection log (via `redact_secrets`).
    #[test]
    fn connection_log_redacts_planted_secrets() {
        let (state, _dir) = test_state();
        state.push_connection_log(
            "INFO",
            "probe password=AbCdEfGhIjKlMnOpQrStUvWx key=00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff hmac=deadbeef",
        );
        let joined = state.connection_log_lines().join("\n");
        assert!(
            !joined.contains("AbCdEfGhIjKlMnOpQrStUvWx"),
            "password leaked:\n{joined}"
        );
        assert!(
            !joined.contains("00112233445566778899"),
            "key leaked:\n{joined}"
        );
        assert!(!joined.contains("deadbeef"), "hmac leaked:\n{joined}");
        assert!(
            joined.contains("[REDACTED]"),
            "redaction marker missing:\n{joined}"
        );
    }

    /// DS-02: `block_on_control` from inside a tokio runtime must
    /// fail with an error, never panic ("Cannot start a runtime
    /// from within a runtime" used to kill the daemon's manager
    /// loop when tray "Restart wheel" called it). A current-thread
    /// runtime built here is the exact manager-loop shape.
    #[test]
    fn block_on_control_inside_a_runtime_errors_instead_of_panicking() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        let result = rt.block_on(async { block_on_control(async { Ok::<(), String>(()) }) });
        let error = result.expect_err("in-runtime call must fail, not panic");
        assert!(
            error.contains("spawn_blocking"),
            "the error must point at the fix: {error}"
        );
        // And outside a runtime the same call still works.
        assert!(
            block_on_control(async { Ok::<(), String>(()) }).is_ok(),
            "plain-thread call must still succeed"
        );
    }

    /* H5-control: run-a-control-command-under-a-fresh-session. */

    /// Drive one control op under a fresh fake session — the exact
    /// path the sync commands take (`block_on_control` +
    /// `run_control_session`). Plain `#[test]`, never `#[tokio::test]`
    /// (`block_on_control` builds its own runtime).
    fn drive_control<T, K, F, R>(
        state: &Arc<AppStateCore>,
        transport: T,
        keystore: K,
        op: F,
    ) -> Result<R, String>
    where
        T: SessionTransport,
        K: KeyStoreLike,
        F: for<'s> FnOnce(
            &'s mut BoardSession<K, T>,
        ) -> Pin<Box<dyn Future<Output = Result<R, String>> + 's>>,
    {
        block_on_control(run_control_session(state, transport, keystore, op))
    }

    /// A fake transport against `board`, answering a v2.2
    /// `HELLO`. Each call is a new session.
    fn paired_transport(
        board: &Arc<Mutex<FakeBoardConfig>>,
    ) -> (ActKeyStore, FakeSessionTransport) {
        let store = ActKeyStore::new();
        let transport = FakeSessionTransport::new_with_board(Some(act_hello()), Arc::clone(board));
        (store, transport)
    }

    /// The board's live config blob (what `GET_CFG` returns).
    fn board_current(board: &Arc<Mutex<FakeBoardConfig>>) -> Vec<u8> {
        board.lock().expect("board lock").current.clone()
    }

    /// The factory blob with the horn pin moved 4 → 5 (a visible,
    /// valid change for the transaction tests).
    fn horn_moved_blob() -> Vec<u8> {
        let mut blob = rsh_protocol::factory_default_blob();
        for record in &mut blob.records {
            if record.rec_type == rsh_protocol::config::record_type::BUTTON && record.slot == 2 {
                record.pin_a = 5;
            }
        }
        blob.encode().expect("horn-moved blob encodes")
    }

    /// Stage + commit `blob` across two fresh sessions; returns the
    /// `Committed` snapshot (saved into the state).
    fn stage_and_commit(
        state: &Arc<AppStateCore>,
        board: &Arc<Mutex<FakeBoardConfig>>,
        blob: Vec<u8>,
    ) -> ConfigTxnSnapshot {
        let (store, transport) = paired_transport(board);
        let (result, snapshot) = drive_control(state, transport, store, |session| {
            Box::pin(op_stage_config(session, ConfigTxnSnapshot::default(), blob))
        })
        .expect("stage helper succeeds");
        result.expect("stage succeeds");
        assert_eq!(snapshot.state, ConfigState::Staged);
        let (store, transport) = paired_transport(board);
        let (result, snapshot) = drive_control(state, transport, store, |session| {
            Box::pin(op_commit_config(session, snapshot))
        })
        .expect("commit helper succeeds");
        result.expect("commit succeeds");
        assert_eq!(snapshot.state, ConfigState::Committed);
        *lock(&state.config_txn) = snapshot.clone();
        snapshot
    }

    #[test]
    fn control_session_runs_set_password_under_a_fresh_session() {
        let (state, _dir) = test_state();
        assert!(!state.session_gate_open(), "gate starts closed");
        let board = Arc::new(Mutex::new(FakeBoardConfig::new()));
        let (store, transport) = paired_transport(&board);
        let sent_log = transport.sent_log();
        drive_control(&state, transport, store.clone(), |session| {
            Box::pin(op_set_password(session, "new-password-123".to_string()))
        })
        .expect("set_password succeeds");
        // Unsigned GET_CAPS first (reqId 0), then SET_PASSWORD (reqId 1).
        let sent = sent_log.lock().expect("sent log lock");
        let frames: Vec<rsh_protocol::ControlFrame> = sent
            .iter()
            .map(|bytes| {
                rsh_protocol::ControlFrame::parse_structure(bytes).expect("sent frame parses")
            })
            .collect();
        assert_eq!(frames.len(), 2, "only GET_CAPS + SET_PASSWORD sent");
        assert_eq!(frames[0].cmd, rsh_protocol::control::Opcode::GetCaps as u8);
        assert_eq!(frames[0].req_id, 0);
        assert_eq!(
            frames[1].cmd,
            rsh_protocol::control::Opcode::SetPassword as u8
        );
        assert_eq!(frames[1].req_id, 1);
        // The SET_PASSWORD payload is the v2.2 layout (D3): [curLen 0]
        // (no password stored yet) + [newLen 16] + the new password.
        assert_eq!(frames[1].payload[0], 0);
        assert_eq!(frames[1].payload[1], 16);
        assert_eq!(&frames[1].payload[2..], b"new-password-123");
        // The new password is stored as the Wi-Fi password (one credential).
        assert_eq!(
            store.wifi_password(ACT_BOARD_ID).as_deref(),
            Some("new-password-123")
        );
        // A transient control session never activates or opens the gate.
        assert!(!state.session_gate_open(), "gate stays closed");
        assert!(state.session_activation().is_none());
        assert_eq!(state.session_last_error(), None);
    }

    #[test]
    fn control_session_set_password_carries_the_stored_password_as_current() {
        let (state, _dir) = test_state();
        let board = Arc::new(Mutex::new(FakeBoardConfig::new()));
        let (store, transport) = paired_transport(&board);
        // A password is already set: the stored one must travel as the
        // current password (D3), or the board rejects the change.
        store
            .store_wifi_password(ACT_BOARD_ID, "old-password-1")
            .expect("pre-store the current password");
        let sent_log = transport.sent_log();
        drive_control(&state, transport, store.clone(), |session| {
            Box::pin(op_set_password(session, "new-password-123".to_string()))
        })
        .expect("set_password succeeds");
        let sent = sent_log.lock().expect("sent log lock");
        let frames: Vec<rsh_protocol::ControlFrame> = sent
            .iter()
            .map(|bytes| {
                rsh_protocol::ControlFrame::parse_structure(bytes).expect("sent frame parses")
            })
            .collect();
        assert_eq!(frames.len(), 2, "only GET_CAPS + SET_PASSWORD sent");
        // [curLen 14][current "old-password-1"][newLen 16][new].
        assert_eq!(frames[1].payload[0], 14);
        assert_eq!(&frames[1].payload[1..15], b"old-password-1");
        assert_eq!(frames[1].payload[15], 16);
        assert_eq!(&frames[1].payload[16..], b"new-password-123");
        // The new password replaces the stored one.
        assert_eq!(
            store.wifi_password(ACT_BOARD_ID).as_deref(),
            Some("new-password-123")
        );
    }

    #[test]
    fn control_session_set_password_rejects_a_short_password() {
        let (state, _dir) = test_state();
        let board = Arc::new(Mutex::new(FakeBoardConfig::new()));
        let (store, transport) = paired_transport(&board);
        let sent_log = transport.sent_log();
        let error = drive_control(&state, transport, store, |session| {
            Box::pin(op_set_password(session, "short".to_string()))
        })
        .expect_err("short password must fail");
        assert!(error.contains("8..=63"), "{error}");
        // Only GET_CAPS went out — no SET_PASSWORD reached the board.
        assert_eq!(
            sent_log.lock().expect("sent log lock").len(),
            1,
            "only GET_CAPS sent"
        );
        assert!(!state.session_gate_open(), "gate stays closed");
        // R2-05: the failure is recorded on the separate control
        // channel — the session channel (which the manager loop
        // turns into `ActivationFailed`) stays untouched.
        assert!(
            state.control_last_error().is_some(),
            "failure is recorded on the control channel"
        );
        assert!(
            state.session_last_error().is_none(),
            "control failure must not touch the session error"
        );
    }

    /// R3-D-10: a dead credential store must fail BEFORE the board
    /// is called — never as a misleading `BadPassword` + "hold
    /// BOOT 10 s". `NotFound` (first setup) still proceeds with
    /// `current = None`; only other keystore errors fail here.
    #[test]
    fn control_session_set_password_fails_closed_on_keystore_error() {
        let (state, _dir) = test_state();
        let board = Arc::new(Mutex::new(FakeBoardConfig::new()));
        let (store, transport) = paired_transport(&board);
        store.set_fail_get(true);
        let sent_log = transport.sent_log();
        let error = drive_control(&state, transport, store, |session| {
            Box::pin(op_set_password(session, "new-password-123".to_string()))
        })
        .expect_err("a dead keystore must fail");
        assert!(
            error.contains("could not read the stored Wi-Fi password"),
            "{error}"
        );
        assert!(
            !error.contains("BOOT"),
            "must not blame the board / suggest the BOOT hold: {error}"
        );
        // Only GET_CAPS went out — no SET_PASSWORD reached the board.
        assert_eq!(
            sent_log.lock().expect("sent log lock").len(),
            1,
            "only GET_CAPS sent"
        );
    }

    /// R3-D-08: the password-set cache serves the stored flag
    /// within its TTL, re-probes for another board or after
    /// invalidation, and never answers one board's flag for another.
    #[test]
    fn wifi_password_cache_serves_within_ttl_and_reprobes() {
        let mut cache = WifiPasswordCache::default();
        let now = Instant::now();
        assert_eq!(cache.get("AA:BB", now), None, "never probed");
        cache.store("AA:BB", true, now);
        assert_eq!(cache.get("AA:BB", now), Some(true));
        assert_eq!(cache.get("CC:DD", now), None, "another board must re-probe");
        cache.invalidate();
        assert_eq!(cache.get("AA:BB", now), None, "invalidated");
        cache.store("AA:BB", false, now);
        assert_eq!(cache.get("AA:BB", now), Some(false));
        assert_eq!(
            cache.get("AA:BB", now + WIFI_PASSWORD_CACHE_TTL),
            None,
            "TTL expiry re-probes"
        );
    }

    #[test]
    fn control_session_fails_closed_with_no_hello() {
        let (state, _dir) = test_state();
        let board = Arc::new(Mutex::new(FakeBoardConfig::new()));
        let (store, mut transport) = paired_transport(&board);
        transport.never_answer_hello = true;
        let error = drive_control(&state, transport, store, |session| {
            Box::pin(op_reboot(session))
        })
        .expect_err("no HELLO must fail");
        assert!(error.contains("no HELLO"), "{error}");
        assert!(!state.session_gate_open(), "gate stays closed");
        // R2-05: the failure is recorded on the separate control
        // channel — the session channel (which the manager loop
        // turns into `ActivationFailed`) stays untouched.
        assert!(
            state.control_last_error().is_some(),
            "failure is recorded on the control channel"
        );
        assert!(
            state.session_last_error().is_none(),
            "control failure must not touch the session error"
        );
    }

    #[test]
    fn control_session_fails_closed_when_the_transport_dies() {
        let (state, _dir) = test_state();
        let board = Arc::new(Mutex::new(FakeBoardConfig::new()));
        let (store, mut transport) = paired_transport(&board);
        transport.hello_closed = true;
        let error = drive_control(&state, transport, store, |session| {
            Box::pin(op_reboot(session))
        })
        .expect_err("closed transport must fail");
        assert!(error.contains("transport closed"), "{error}");
        assert!(!state.session_gate_open(), "gate stays closed");
    }

    #[test]
    fn control_session_runs_reboot_under_a_fresh_session() {
        let (state, _dir) = test_state();
        let board = Arc::new(Mutex::new(FakeBoardConfig::new()));
        let (store, transport) = paired_transport(&board);
        let sent_log = transport.sent_log();
        drive_control(&state, transport, store, |session| {
            Box::pin(op_reboot(session))
        })
        .expect("reboot succeeds");
        let sent = sent_log.lock().expect("sent log lock");
        let frames: Vec<rsh_protocol::ControlFrame> = sent
            .iter()
            .map(|bytes| {
                rsh_protocol::ControlFrame::parse_structure(bytes).expect("sent frame parses")
            })
            .collect();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[1].cmd, rsh_protocol::control::Opcode::Reboot as u8);
        assert!(!state.session_gate_open(), "gate stays closed");
    }

    #[test]
    fn config_flow_runs_across_fresh_sessions() {
        let (state, _dir) = test_state();
        let board = Arc::new(Mutex::new(FakeBoardConfig::new()));
        let factory = board_current(&board);
        let staged = horn_moved_blob();
        // Stage: the board still runs the factory blob until commit.
        let (store, transport) = paired_transport(&board);
        let (result, snapshot) = drive_control(&state, transport, store, |session| {
            Box::pin(op_stage_config(
                session,
                ConfigTxnSnapshot::default(),
                staged.clone(),
            ))
        })
        .expect("stage helper succeeds");
        result.expect("stage succeeds");
        assert_eq!(snapshot.state, ConfigState::Staged);
        *lock(&state.config_txn) = snapshot.clone();
        assert_eq!(state.config_state(), "staged");
        assert_eq!(board_current(&board), factory, "stage does not go live");
        // Commit on a fresh session: the board live-applies the blob.
        let (store, transport) = paired_transport(&board);
        let (result, snapshot) = drive_control(&state, transport, store, |session| {
            Box::pin(op_commit_config(session, snapshot))
        })
        .expect("commit helper succeeds");
        result.expect("commit succeeds");
        assert_eq!(snapshot.state, ConfigState::Committed);
        assert!(
            snapshot.committed_at.is_some(),
            "commit records when the window started"
        );
        *lock(&state.config_txn) = snapshot.clone();
        assert_eq!(state.config_state(), "pending_confirm");
        assert_eq!(board_current(&board), staged, "commit goes live");
        // Confirm on a third fresh session: the new config is permanent.
        let (store, transport) = paired_transport(&board);
        let (result, snapshot) = drive_control(&state, transport, store, |session| {
            Box::pin(op_confirm_config(session, snapshot))
        })
        .expect("confirm helper succeeds");
        result.expect("confirm succeeds");
        assert_eq!(snapshot.state, ConfigState::Confirmed);
        *lock(&state.config_txn) = snapshot;
        assert_eq!(state.config_state(), "clean");
        assert_eq!(board_current(&board), staged);
    }

    #[test]
    fn config_rollback_restores_the_last_good_blob() {
        let (state, _dir) = test_state();
        let board = Arc::new(Mutex::new(FakeBoardConfig::new()));
        let factory = board_current(&board);
        let staged = horn_moved_blob();
        let snapshot = stage_and_commit(&state, &board, staged.clone());
        assert_eq!(board_current(&board), staged, "commit went live");
        assert_eq!(state.config_state(), "pending_confirm");
        let (store, transport) = paired_transport(&board);
        let (result, snapshot) = drive_control(&state, transport, store, |session| {
            Box::pin(op_rollback_config(session, snapshot))
        })
        .expect("rollback helper succeeds");
        result.expect("rollback succeeds");
        assert_eq!(snapshot.state, ConfigState::RolledBack);
        *lock(&state.config_txn) = snapshot;
        assert_eq!(state.config_state(), "rolled_back");
        assert_eq!(
            board_current(&board),
            factory,
            "rollback restores the last-good blob"
        );
    }

    #[test]
    fn get_config_and_get_pin_map_read_the_live_blob() {
        let (state, _dir) = test_state();
        let board = Arc::new(Mutex::new(FakeBoardConfig::new()));
        let (store, transport) = paired_transport(&board);
        let blob = drive_control(&state, transport, store, |session| {
            Box::pin(op_read_config_blob(session))
        })
        .expect("read succeeds");
        let decoded = ConfigBlob::decode(&blob).expect("board blob decodes");
        let json = config_blob_to_json(&decoded);
        assert_eq!(json["cfgVersion"], 1);
        assert_eq!(
            json["buttons"],
            serde_json::json!({"1": "shifter_up", "2": "shifter_down", "3": "horn"})
        );
        let pins = records_to_pin_assignments(&decoded.records);
        assert_eq!(
            pins,
            vec![
                PinAssignment {
                    function: "steering".to_string(),
                    pin: 34,
                    pins: Vec::new(),
                },
                PinAssignment {
                    function: "accelerator".to_string(),
                    pin: 35,
                    pins: Vec::new(),
                },
                PinAssignment {
                    function: "brake".to_string(),
                    pin: 36,
                    pins: Vec::new(),
                },
                PinAssignment {
                    function: "handbrake".to_string(),
                    pin: 32,
                    pins: Vec::new(),
                },
                PinAssignment {
                    function: "horn".to_string(),
                    pin: 4,
                    pins: Vec::new(),
                },
                PinAssignment {
                    function: "led".to_string(),
                    pin: 21,
                    pins: Vec::new(),
                },
            ]
        );
    }

    #[test]
    fn set_pin_map_merges_and_commits_then_confirms() {
        let (state, _dir) = test_state();
        let board = Arc::new(Mutex::new(FakeBoardConfig::new()));
        let pins = vec![
            PinAssignment {
                function: "steering".to_string(),
                pin: 34,
                pins: Vec::new(),
            },
            PinAssignment {
                function: "accelerator".to_string(),
                pin: 35,
                pins: Vec::new(),
            },
            PinAssignment {
                function: "brake".to_string(),
                pin: 36,
                pins: Vec::new(),
            },
            PinAssignment {
                function: "handbrake".to_string(),
                pin: 32,
                pins: Vec::new(),
            },
            PinAssignment {
                function: "horn".to_string(),
                pin: 4,
                pins: Vec::new(),
            },
            PinAssignment {
                function: "led".to_string(),
                pin: 21,
                pins: Vec::new(),
            },
        ];
        let (store, transport) = paired_transport(&board);
        let (result, snapshot) = drive_control(&state, transport, store, |session| {
            Box::pin(op_stage_and_commit_pin_map(
                session,
                ConfigTxnSnapshot::default(),
                pins,
            ))
        })
        .expect("helper succeeds");
        let validation = result.expect("stage+commit succeeds");
        assert!(validation.valid, "errors: {:?}", validation.errors);
        assert_eq!(snapshot.state, ConfigState::Committed);
        *lock(&state.config_txn) = snapshot.clone();
        assert_eq!(state.config_state(), "pending_confirm");
        // The merged blob on the board: handbrake in AXIS_ADC slot 4
        // (no clutch slot 3 added), shifter up/down + battery kept.
        let merged = ConfigBlob::decode(&board_current(&board)).expect("merged blob decodes");
        let find = |rec_type: u8, slot: u8| {
            merged
                .records
                .iter()
                .find(|r| r.rec_type == rec_type && r.slot == slot)
                .map(|r| r.pin_a)
        };
        assert_eq!(find(rsh_protocol::config::record_type::AXIS_ADC, 3), None);
        assert_eq!(
            find(rsh_protocol::config::record_type::AXIS_ADC, 4),
            Some(32)
        );
        assert_eq!(
            find(rsh_protocol::config::record_type::BUTTON, 0),
            Some(27),
            "shifter-up kept"
        );
        assert_eq!(
            find(rsh_protocol::config::record_type::BATTERY, 0),
            Some(39),
            "battery kept"
        );
        // Confirm on a fresh session.
        let (store, transport) = paired_transport(&board);
        let (result, snapshot) = drive_control(&state, transport, store, |session| {
            Box::pin(op_confirm_config(session, snapshot))
        })
        .expect("helper succeeds");
        result.expect("confirm succeeds");
        assert_eq!(snapshot.state, ConfigState::Confirmed);
    }

    #[test]
    fn set_pin_map_generic_buttons_row_is_a_validation_verdict() {
        let (state, _dir) = test_state();
        let board = Arc::new(Mutex::new(FakeBoardConfig::new()));
        let pins = vec![
            PinAssignment {
                function: "steering".to_string(),
                pin: 34,
                pins: Vec::new(),
            },
            PinAssignment {
                function: "buttons".to_string(),
                pin: 5,
                pins: Vec::new(),
            },
        ];
        let (store, transport) = paired_transport(&board);
        let (result, snapshot) = drive_control(&state, transport, store, |session| {
            Box::pin(op_stage_and_commit_pin_map(
                session,
                ConfigTxnSnapshot::default(),
                pins,
            ))
        })
        .expect("helper succeeds");
        let validation = result.expect("merge failure is a verdict, not an error");
        assert!(!validation.valid);
        assert!(
            validation.errors.iter().any(|e| e.contains("no board pin")),
            "errors: {:?}",
            validation.errors
        );
        assert_eq!(snapshot.state, ConfigState::Idle, "nothing was staged");
        assert_eq!(
            board_current(&board),
            rsh_protocol::factory_default_blob()
                .encode()
                .expect("factory blob encodes"),
            "board untouched"
        );
    }

    #[test]
    fn confirm_after_expiry_errors_without_a_board_call() {
        let (state, _dir) = test_state();
        let snapshot = ConfigTxnSnapshot {
            state: ConfigState::Committed,
            committed_at: Some(
                Instant::now()
                    .checked_sub(Duration::from_secs(31))
                    .expect("31 s ago is representable"),
            ),
            commit_deadline: Duration::from_secs(30),
            ..ConfigTxnSnapshot::default()
        };
        *lock(&state.config_txn) = snapshot;
        let error = state
            .confirm_config()
            .expect_err("expired confirm must fail");
        assert!(error.contains("expired"), "{error}");
        assert_eq!(
            lock(&state.config_txn).state,
            ConfigState::Committed,
            "record stays Committed until rollback verifies"
        );
    }

    #[test]
    fn serial_control_refuses_while_the_live_transport_holds_the_port() {
        let (state, _dir) = test_state();
        *lock(&state.engine_status) = EngineTaskStatus::Running {
            transport: Some(TransportKind::Serial),
        };
        let error = state
            .build_serial_control_transport()
            .expect_err("serial held must fail");
        assert!(error.contains("disconnect the transport first"), "{error}");
    }

    #[test]
    fn sync_get_config_without_board_fails_honestly() {
        // The sync command path end to end (transport build + fresh
        // session + honest failure) with no board answering: ~1 s of
        // HELLO waits, then a plain error — never a guessed config.
        let (state, _dir) = test_state();
        let error = state.get_config().expect_err("no board must fail");
        assert!(error.contains("no HELLO"), "{error}");
        assert!(!state.session_gate_open(), "gate stays closed");
        assert_eq!(state.config_state(), "clean");
    }

    #[test]
    fn sync_upload_firmware_without_board_fails_before_any_upload() {
        // Missing file first (no board contact at all), then a garbage
        // file with no board: the session fails before verification
        // or any HTTP attempt — the upload leg is never reached.
        let (state, dir) = test_state();
        let missing = dir.join("missing.bin");
        let error = state
            .upload_firmware(&missing)
            .expect_err("missing file must fail");
        assert!(error.contains("could not read"), "{error}");
        let garbage = dir.join("garbage.bin");
        std::fs::write(&garbage, b"not a firmware image").expect("write garbage image");
        let error = state
            .upload_firmware(&garbage)
            .expect_err("no board must fail");
        assert!(error.contains("no HELLO"), "{error}");
    }

    #[test]
    fn start_firmware_update_without_session_fails_closed() {
        // H-4 (FD): the new flash flow refuses to start with no live
        // board session — before any image read, keystore lookup, or
        // network attempt — and the status slot stays idle (no
        // phantom "running" flash for the UI to poll).
        let (state, _dir) = test_state();
        assert!(state.session_activation().is_none());
        let error = state
            .start_firmware_update()
            .expect_err("no session must fail");
        assert!(error.contains("no live board session"), "{error}");
        let status = state.firmware_update_status();
        assert!(!status.running, "no flash may be running");
        assert!(!status.done, "no flash may be finished");
        assert_eq!(status.error, None);
    }
}
