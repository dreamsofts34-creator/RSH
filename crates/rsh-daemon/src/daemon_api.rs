//! `AppApi` implementation over the live [`AppStateCore`]
//! (LOGIC-FIX-PLAN §2): the daemon's commands, as the IPC server
//! exposes them and the Slint UI (Phase 3) will call them.
//!
//! The connection state comes from the daemon's
//! [`ConnectionManager`](rsh_api::ConnectionManager) (driven by the
//! manager loop in `main.rs`), merged with the live transport state
//! from [`AppStateCore`]: "Connected" is only ever reported when
//! the session gate is open (C-02).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rsh_api::connection::ConnectionState as MgrState;
use rsh_api::errors::{ConnectionError, UserError};
use rsh_api::{
    ApiTransport, AppApi, AppEvent, ButtonBinding, CalibrationInfo, ConnectionInfo, DiagStats,
    DraftStatus, FirmwareCheck, FirmwareUpdateStatus, GameSummary, PairingInfo, PollState,
    ProfileSummary, RawMv, SlowPollState, SnapshotInfo, TuningSettings, VjoyStatus, app_version,
};
use rsh_api::{ConnEvent, ConnectionManager};
use rsh_app_core::snapshot::EngineSnapshot;
use rsh_app_core::state::{
    AppSettingsPatch, AppStateCore, GameInfo, ProfileSummary as CoreProfileSummary, TransportKind,
    TransportState,
};

/// Feed window for the F20 watchdog frame feed (see
/// [`DaemonApi::watchdog_tick`]): a stream frame counts as
/// "flowing now" when the core saw one within this long — two
/// manager-loop ticks (250 ms each). Frames normally arrive every
/// few ms, so a live board refreshes the manager's clock on every
/// tick; a board streaming slower than ~2 Hz is already effectively
/// stalled and the watchdog should fire. Worst-case detection
/// latency is ~2.5 s instead of the nominal 2 s.
const FRAME_FEED_WINDOW: Duration = Duration::from_millis(500);

/// The daemon's API handle: shared by the IPC server, the tray
/// callbacks, and the manager loop.
pub struct DaemonApi {
    /// The app state (engine, transport, profiles, pairing).
    pub state: Arc<AppStateCore>,
    /// The L1 Connection Manager (driven by the manager loop).
    pub manager: Mutex<ConnectionManager>,
}

impl DaemonApi {
    /// Build over an opened [`AppStateCore`].
    #[must_use]
    pub fn new(state: Arc<AppStateCore>) -> Arc<Self> {
        Arc::new(Self {
            state,
            manager: Mutex::new(ConnectionManager::new()),
        })
    }

    /// Feed one manager event (from the manager loop) and return the
    /// action it produced, plus whether the state changed (the loop
    /// publishes `AppEvent::ConnectionChanged` then).
    pub fn manager_event(&self, event: ConnEvent) -> (rsh_api::ManagerAction, bool) {
        let mut manager = self
            .manager
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let action = manager.on_event(event, std::time::Instant::now());
        let changed = manager.take_changed();
        (action, changed)
    }

    /// Drain the manager's transition log lines (the loop writes
    /// them to the diagnostics log).
    pub fn take_manager_log_lines(&self) -> Vec<String> {
        self.manager
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take_log_lines()
    }

    /// Current manager state (for the loop's watchdog / retry ticks).
    pub fn manager_state(&self) -> MgrState {
        self.manager
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .state()
            .clone()
    }

    /// When the current backoff ends (`None` when not backing off).
    pub fn manager_retry_due_at(&self) -> Option<std::time::Instant> {
        self.manager
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retry_due_at()
    }

    /// Run the heartbeat watchdog tick; returns the action.
    ///
    /// F20: the tick first feeds real frame freshness into the
    /// manager (below) — without the feed the watchdog fired 2 s
    /// after every `CapsOk`, healthy session included, because
    /// `FrameSeen` was never fed by production code.
    pub fn watchdog_tick(&self) -> rsh_api::ManagerAction {
        // F20: feed the core's stream-frame clock into the manager.
        // The forwarder stamps `note_stream_frame()` on every
        // stream frame (`state.rs`) and the core exposes it as
        // `seconds_since_last_stream_frame()` — whose docs always
        // said "the daemon's Connection Manager calls this on its
        // watchdog tick" — but nothing ever called it, so the
        // manager's `last_frame_at` was stamped only by `CapsOk`
        // and the watchdog rebuilt the transport ~4 s after every
        // activation, forever, even on a healthy board. A frame
        // inside the feed window refreshes the manager's clock; an
        // older (or absent) frame leaves the manager to fire on its
        // own clock, and the F20 cap in `check_watchdog` latches
        // after 5 frameless rebuilds instead of looping forever.
        if let Some(since) = self.state.seconds_since_last_stream_frame()
            && since < FRAME_FEED_WINDOW.as_secs_f64()
        {
            let _ = self.manager_event(ConnEvent::FrameSeen);
        }
        self.manager
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .check_watchdog(std::time::Instant::now())
    }

    /// Build the [`ConnectionInfo`] from one manager state + one
    /// transport state (L-4, v2.0.2): [`AppApi::connection`] and
    /// [`AppApi::poll`] share this, so both derive the same
    /// label/state from the same reads — never two different
    /// manager generations for one instant.
    fn connection_from(mgr: &MgrState, transport_state: &TransportState) -> ConnectionInfo {
        let (transport, address) = (
            match transport_state.transport.as_str() {
                "usb" => ApiTransport::Usb,
                _ => ApiTransport::Wifi,
            },
            transport_state.address.clone(),
        );
        // The manager is the state of record; the core transport
        // state fills in the address + session gate. "Connected"
        // only when the gate is open.
        let (state, label, error, hint) = match mgr {
            MgrState::Active => (MgrState::Active, "Connected".to_string(), None, None),
            MgrState::Error { reason, hint, .. } => (
                mgr.clone(),
                "Error".to_string(),
                Some(*reason),
                Some(hint.clone()),
            ),
            other => {
                // DS-18: the label is derived from the manager state
                // ALONE when it is not Active. The old code upgraded
                // the label to "Connected" whenever the core still
                // reported Connected (e.g. the watchdog fired →
                // Backoff before `disconnect_transport()` ran), so
                // the published event carried `state: Backoff,
                // label: "Connected"` — the comment above claimed
                // the opposite. Only Active is "Connected" (see
                // `ConnectionState::label`).
                (other.clone(), other.label().to_string(), None, None)
            }
        };
        ConnectionInfo {
            state,
            label,
            transport,
            address,
            session_active: transport_state.session_active,
            error,
            hint,
        }
    }

    /// Build the [`SnapshotInfo`] from one core snapshot + the live
    /// button mask (L-4, v2.0.2): [`AppApi::snapshot`] and
    /// [`AppApi::poll`] share this, so both report the same `seq`
    /// for the same engine tick.
    fn snapshot_from(snap: &EngineSnapshot, button_mask: u64) -> SnapshotInfo {
        SnapshotInfo {
            steering_deg: snap.steering_deg,
            throttle: snap.pedals.accel,
            brake: snap.pedals.brake,
            clutch: snap.pedals.clutch,
            // H10: live button bitmask from the last edge frame.
            button_mask,
            // M-3: live handbrake + gear from the pipeline output.
            handbrake: snap.handbrake,
            gear: snap.gear,
            // L-4: the engine tick this snapshot was built from.
            seq: snap.seq,
        }
    }
}

impl AppApi for DaemonApi {
    fn version(&self) -> String {
        app_version().to_string()
    }

    fn connection(&self) -> ConnectionInfo {
        Self::connection_from(&self.manager_state(), &self.state.transport_state())
    }

    fn connect(&self, transport: ApiTransport) -> Result<ConnectionInfo, UserError> {
        let kind = match transport {
            ApiTransport::Wifi => TransportKind::Udp,
            ApiTransport::Usb => TransportKind::Serial,
        };
        self.state
            .connect_transport(kind)
            .map_err(|detail| UserError::from_connection(ConnectionError::Unreachable, detail))?;
        self.manager_event(ConnEvent::Connect);
        Ok(self.connection())
    }

    fn disconnect(&self) -> Result<ConnectionInfo, UserError> {
        self.state
            .disconnect_transport()
            .map_err(|detail| UserError::from_connection(ConnectionError::Unknown, detail))?;
        self.manager_event(ConnEvent::Disconnect);
        Ok(self.connection())
    }

    fn pairing_status(&self) -> PairingInfo {
        let status = self.state.pairing_status();
        PairingInfo {
            seconds_left: status.seconds_left,
            password: status.password,
            error: self.state.pairing_error_typed(),
            error_detail: status.error,
            board_mac: status.board_mac,
            replacement_required: status.replacement_required,
            key_reset_armed: status.key_reset_armed,
            // DS-25: the pairing attempt trail (HELO sends, HELLO
            // replies, timeouts — see
            // `AppStateCore::pairing_log_tail_lines`) rides on the
            // status the wizard already polls, so the UI can show
            // e.g. "Sent HELO to 192.168.4.1 — no reply (3 tries)"
            // instead of only "No pairing window". The log lines
            // are redacted at the source (never a password).
            // R2-42: the retained tail (≤20 lines), never the full
            // ≤200-line buffer — the slow poller calls this ~1 Hz.
            // R2-23: the tail is never drained, so the manager
            // loop's 250 ms file drain cannot starve this reader.
            attempt_log: self.state.pairing_log_tail_lines(),
        }
    }

    /// Phase 2 (RC-6): the ~1 Hz slow poll reads pairing status via
    /// `pairing_status_peek()` — never the consuming
    /// `pairing_status()`. The old path consumed the one-time
    /// password on every poll cycle and logged "one-time password
    /// disclosed to UI" 20× in the user's log, although no UI
    /// element ever renders the password. The peek's password is
    /// stripped here too (the UI never displays it; the post-pairing
    /// Wi-Fi rejoin uses the daemon's own stash, R2-27), so the
    /// password is never shipped over IPC once a second. Direct
    /// `pairing_status()` calls (begin / reset / confirm replies)
    /// keep the consuming behaviour — an explicit read is a real
    /// disclosure. This override otherwise mirrors the trait
    /// default field-for-field; if the default gains a field,
    /// update this too (the `SlowPollState` struct literal fails
    /// to compile on a missing field, so drift is caught).
    fn slow_poll(&self) -> SlowPollState {
        let mut tunings = std::collections::BTreeMap::new();
        for module in [
            "steering",
            "throttle",
            "brake",
            "clutch",
            "handbrake",
            "led",
            "shifter",
            "horn",
        ] {
            tunings.insert(module.to_string(), self.get_tuning(module));
        }
        let mut modules = std::collections::BTreeMap::new();
        for module in ["handbrake", "shifter", "horn", "led"] {
            modules.insert(module.to_string(), self.module_enabled(module));
        }
        let peeked = self.state.pairing_status_peek();
        let pairing_status = PairingInfo {
            seconds_left: peeked.seconds_left,
            password: None,
            error: self.state.pairing_error_typed(),
            error_detail: peeked.error,
            board_mac: peeked.board_mac,
            replacement_required: peeked.replacement_required,
            key_reset_armed: peeked.key_reset_armed,
            attempt_log: self.state.pairing_log_tail_lines(),
        };
        SlowPollState {
            version: self.version(),
            firmware_version: self.firmware_version(),
            backend: self.output_backend(),
            autostart: self.autostart(),
            board_address: self.board_address(),
            diag_stats: self.diag_stats().ok(),
            pairing_status,
            calibration: self.calibration().ok(),
            raw_mv: self.raw_mv().ok(),
            profiles: self.list_profiles(),
            games: self.list_games(),
            firmware_check: self.firmware_check().ok(),
            firmware_update_status: self.firmware_update_status(),
            vjoy_status: self.vjoy_status().ok(),
            auto_pause: self.auto_pause(),
            close_to_tray: self.close_to_tray(),
            bindings: self.list_bindings(),
            knobs: self.knob_actions(),
            draft: self.draft_status(),
            tunings,
            modules,
        }
    }

    fn begin_pairing(&self) -> Result<PairingInfo, UserError> {
        self.state.begin_pairing().map_err(|detail| {
            let typed = self
                .state
                .pairing_error_typed()
                .unwrap_or(ConnectionError::Unknown);
            UserError::from_connection(typed, detail)
        })?;
        Ok(self.pairing_status())
    }

    fn reset_pairing(&self) -> Result<PairingInfo, UserError> {
        self.state.reset_pairing().map_err(|detail| {
            let typed = self
                .state
                .pairing_error_typed()
                .unwrap_or(ConnectionError::Unknown);
            UserError::from_connection(typed, detail)
        })?;
        Ok(self.pairing_status())
    }

    fn confirm_pairing_replacement(&self) -> Result<PairingInfo, UserError> {
        // R2-51: `state.confirm_pairing_replacement` arms the
        // one-time flag and calls `activate_session()`, which only
        // queues the transport connect and returns — the
        // activation itself runs async in the forwarder, so this
        // never blocks the IPC dispatch path. The UI polls
        // `pairing_status()` for the outcome.
        self.state.confirm_pairing_replacement().map_err(|detail| {
            let typed = self
                .state
                .pairing_error_typed()
                .unwrap_or(ConnectionError::Unknown);
            UserError::from_connection(typed, detail)
        })?;
        Ok(self.pairing_status())
    }

    fn snapshot(&self) -> SnapshotInfo {
        Self::snapshot_from(&self.state.snapshot(), self.state.pressed_button_mask())
    }

    /// L-4 (v2.0.2): one combined live-state read. Each source is
    /// read exactly once, back-to-back in this one synchronous
    /// dispatch (no IPC / await / sleep between the reads): one
    /// core snapshot (carries the engine `seq`), one transport
    /// state, one manager state, one settings copy — then the
    /// connection is derived from the same manager + transport
    /// reads as the snapshot's tick, so the three fields cannot
    /// tear the way three separate IPC calls could. No lock is
    /// held across the reads (each accessor clones under its own
    /// short lock), so there is no new lock-ordering vs the
    /// manager loop or the engine tick.
    fn poll(&self) -> PollState {
        // DS-30: one `Arc` clone per source, not one struct clone
        // per read — the settings are read once and shared between
        // the transport state and the output flag.
        let settings = self.state.settings_arc();
        let core_snap = self.state.snapshot_arc();
        let transport = self.state.transport_state_with_settings(&settings);
        let mgr = self.manager_state();
        let connection = Self::connection_from(&mgr, &transport);
        let snapshot = Self::snapshot_from(&core_snap, self.state.pressed_button_mask());
        PollState {
            snapshot,
            connection,
            output_enabled: settings.output_enabled,
        }
    }

    fn output_enabled(&self) -> bool {
        self.state.settings().output_enabled
    }

    fn set_output_enabled(&self, enabled: bool) {
        // DS-41: the old code discarded the returned state AND never
        // persisted the flag — a backend activation/connect failure
        // was invisible, and the toggle did not survive a restart
        // (`settings().output_enabled` is the persisted source the
        // UI polls on the next `poll`). Now: a failure in the
        // applied state is noted in the engine detail (which the
        // diagnostics surface), the flag is persisted, and a persist
        // failure is logged + noted and reverts the in-memory toggle
        // so the engine and the polled flag agree.
        let applied = self.state.set_output_enabled(enabled);
        if applied.detail.contains("failed") {
            self.state
                .note_engine_detail(format!("output enable failed: {}", applied.detail));
        }
        let patch = AppSettingsPatch {
            output_enabled: Some(enabled),
            ..Default::default()
        };
        if let Err(error) = self.state.set_settings_patch(patch) {
            eprintln!("[rsh-daemon] set_output_enabled persist failed: {error}");
            self.state.note_engine_detail(format!(
                "output setting could not be saved ({error}) — the toggle was reverted"
            ));
            let _ = self.state.set_output_enabled(!enabled);
        }
    }

    fn get_tuning(&self, module: &str) -> TuningSettings {
        self.state.get_tuning(module)
    }

    fn set_tuning(&self, tuning: &TuningSettings) -> Result<TuningSettings, UserError> {
        self.state
            .set_tuning(tuning)
            .map_err(|detail| UserError::from_connection(ConnectionError::Unknown, detail))
    }

    fn draft_status(&self) -> Option<DraftStatus> {
        self.state.draft_status()
    }

    fn save_draft(&self) -> Result<bool, UserError> {
        self.state
            .save_draft()
            .map_err(|detail| UserError::from_connection(ConnectionError::Unknown, detail))
    }

    fn discard_draft(&self) -> bool {
        self.state.discard_draft()
    }

    fn autostart(&self) -> bool {
        self.state.settings().autostart
    }

    fn set_autostart(&self, enabled: bool) -> Result<bool, UserError> {
        let mut settings = self.state.settings();
        settings.autostart = enabled;
        let saved = self
            .state
            .set_settings(settings)
            .map_err(|detail| UserError::from_connection(ConnectionError::Unknown, detail))?;
        // Sync the Windows Run key / Linux .desktop entry right away
        // so the toggle takes effect without a daemon restart.
        let exe = std::env::current_exe().map_err(|detail| {
            UserError::from_connection(
                ConnectionError::Unknown,
                format!("cannot resolve the daemon exe for autostart sync: {detail}"),
            )
        })?;
        crate::tray_autostart::sync_autostart(enabled, &exe)
            .map_err(|detail| UserError::from_connection(ConnectionError::Unknown, detail))?;
        Ok(saved.autostart)
    }

    fn firmware_version(&self) -> String {
        self.state
            .firmware_version()
            .unwrap_or_else(|_| "Unknown".to_string())
    }

    fn firmware_check(&self) -> Result<FirmwareCheck, UserError> {
        Ok(self.state.firmware_check())
    }

    fn update_firmware(&self) -> Result<FirmwareUpdateStatus, UserError> {
        self.state
            .start_firmware_update()
            .map_err(|detail| UserError::from_connection(ConnectionError::Unknown, detail))
    }

    fn firmware_update_status(&self) -> FirmwareUpdateStatus {
        self.state.firmware_update_status()
    }

    fn board_address(&self) -> String {
        self.state.board_address()
    }

    fn set_board_address(&self, address: &str) -> Result<String, UserError> {
        self.state
            .set_board_address(address)
            .map_err(|detail| UserError::from_connection(ConnectionError::Unknown, detail))
    }

    fn module_enabled(&self, module: &str) -> bool {
        self.state.module_enabled(module)
    }

    fn set_module_enabled(&self, module: &str, enabled: bool) -> Result<bool, UserError> {
        self.state
            .set_module_enabled(module, enabled)
            .map_err(|detail| UserError::from_connection(ConnectionError::Unknown, detail))
    }

    fn auto_pause(&self) -> bool {
        self.state.auto_pause()
    }

    fn set_auto_pause(&self, paused: bool) -> Result<bool, UserError> {
        self.state
            .set_auto_pause(paused)
            .map_err(|detail| UserError::from_connection(ConnectionError::Unknown, detail))
    }

    fn close_to_tray(&self) -> bool {
        self.state.close_to_tray()
    }

    fn set_close_to_tray(&self, enabled: bool) -> Result<bool, UserError> {
        self.state
            .set_close_to_tray(enabled)
            .map_err(|detail| UserError::from_connection(ConnectionError::Unknown, detail))
    }

    fn output_backend(&self) -> String {
        self.state.backend_name()
    }

    fn set_output_backend(&self, name: &str) -> Result<String, UserError> {
        self.state.set_output_backend(name).map_err(|detail| {
            // B6/A10: the admin-decline case gets a typed error with
            // plain words — not `Unknown` / "Something went wrong."
            // (the UI shows the title + what happened + what to do).
            if detail == AppStateCore::HID_NOT_ADMIN {
                UserError {
                    code: "not_admin".to_string(),
                    title: "Admin permission needed".to_string(),
                    what_happened: detail.clone(),
                    what_to_do:
                        "Approve the Windows prompt when switching to HIDMaestro, or keep using vJoy (no permission needed)."
                            .to_string(),
                    detail: None,
                }
            } else {
                UserError::from_connection(ConnectionError::Unknown, detail)
            }
        })?;
        Ok(self.state.backend_name())
    }

    fn backup_now(&self) -> Result<String, UserError> {
        let backups = self
            .state
            .backup_now()
            .map_err(|detail| UserError::from_connection(ConnectionError::Unknown, detail))?;
        // `BackupInfo` has no name/path/id — a row is (file,
        // generation); report the first generation actually on disk.
        match backups.iter().find(|backup| backup.present) {
            Some(backup) => Ok(format!(
                "Backup saved: {} ({})",
                backup.file, backup.generation
            )),
            None => Ok("Backup saved".to_string()),
        }
    }

    fn list_profiles(&self) -> Vec<ProfileSummary> {
        self.state
            .list_profiles()
            .iter()
            .map(to_api_profile)
            .collect()
    }

    fn activate_profile(&self, id: &str) -> Result<String, UserError> {
        self.state
            .activate_profile(id)
            .map(|summary| summary.name)
            .map_err(|detail| UserError::from_connection(ConnectionError::Unknown, detail))
    }

    fn create_profile(&self, name: &str) -> Result<String, UserError> {
        self.state
            .create_profile(name)
            .map(|summary| summary.name)
            .map_err(|detail| UserError::from_connection(ConnectionError::Unknown, detail))
    }

    fn duplicate_profile(&self, id: &str) -> Result<String, UserError> {
        self.state
            .duplicate_profile(id)
            .map(|summary| summary.name)
            .map_err(|detail| UserError::from_connection(ConnectionError::Unknown, detail))
    }

    fn delete_profile(&self, id: &str) -> Result<bool, UserError> {
        self.state
            .delete_profile(id)
            .map(|()| true)
            .map_err(|detail| UserError::from_connection(ConnectionError::Unknown, detail))
    }

    fn list_games(&self) -> Vec<GameSummary> {
        self.state.list_games().iter().map(to_api_game).collect()
    }

    fn launch_game(&self, id: &str) -> Result<GameSummary, UserError> {
        self.state
            .launch_game(id)
            .map(|game| to_api_game(&game))
            .map_err(|detail| UserError::from_connection(ConnectionError::Unknown, detail))
    }

    fn stop_game(&self, id: &str) -> Result<GameSummary, UserError> {
        self.state
            .stop_game(id)
            .map(|game| to_api_game(&game))
            .map_err(|detail| UserError::from_connection(ConnectionError::Unknown, detail))
    }

    fn game_state(&self, id: &str) -> Result<GameSummary, UserError> {
        self.state
            .get_game_state(id)
            .map(|game| to_api_game(&game))
            .map_err(|detail| UserError::from_connection(ConnectionError::Unknown, detail))
    }

    fn add_game(
        &self,
        name: &str,
        exe_path: &str,
        profile_id: Option<String>,
    ) -> Result<GameSummary, UserError> {
        self.state
            .add_manual_game(name, exe_path, profile_id)
            .map(|game| to_api_game(&game))
            .map_err(|detail| UserError::from_connection(ConnectionError::Unknown, detail))
    }

    fn update_game(
        &self,
        id: &str,
        name: &str,
        exe_path: &str,
        profile_id: Option<String>,
    ) -> Result<GameSummary, UserError> {
        self.state
            .update_manual_game(id, name, exe_path, profile_id)
            .map(|game| to_api_game(&game))
            .map_err(|detail| UserError::from_connection(ConnectionError::Unknown, detail))
    }

    fn remove_game(&self, id: &str) -> Result<bool, UserError> {
        self.state
            .remove_manual_game(id)
            .map_err(|detail| UserError::from_connection(ConnectionError::Unknown, detail))
    }

    fn start_learn(&self) -> Result<bool, UserError> {
        self.state.start_learn();
        Ok(self.state.is_learning())
    }

    fn cancel_learn(&self) -> Result<bool, UserError> {
        self.state.cancel_learn();
        Ok(self.state.is_learning())
    }

    fn is_learning(&self) -> bool {
        self.state.is_learning()
    }

    fn take_learned_button(&self) -> Option<u8> {
        self.state.take_learned_button()
    }

    // v2.0.1 hotfix BACKEND-2: calibration read, vJoy status/install,
    // diagnostics. v2.0.2 adds the capture backend on top
    // (`raw_mv`, `capture_centre`, `capture_pedal_min/max`): raw mV
    // now reaches the state (see `AppStateCore::note_raw_axes`), so
    // capture samples the live reading instead of inverting the
    // processed values. Button→action rebinding is still not a board
    // feature (see `AppApi::calibration`).
    fn calibration(&self) -> Result<CalibrationInfo, UserError> {
        self.state
            .get_calibration()
            .map(|calibration| CalibrationInfo {
                steering_center_mv: calibration.steering_center_mv,
                pedal_min_mv: calibration.pedal_min_mv,
                pedal_max_mv: calibration.pedal_max_mv,
                calibrated_at: calibration.calibrated_at,
            })
            .map_err(|detail| UserError::from_connection(ConnectionError::Unknown, detail))
    }

    /// Latest raw ADC readings in mV (v2.0.2): the live sample the
    /// capture methods use. Never an error while the daemon runs —
    /// an empty reading (no frame yet) is data, not a failure.
    fn raw_mv(&self) -> Result<RawMv, UserError> {
        Ok(self.state.raw_mv())
    }

    /// Capture the steering centre from the live raw reading
    /// (v2.0.2). The core's message is classified into a stable code
    /// so the UI can explain the failure instead of showing a bare
    /// "Unknown" (see [`capture_user_error`]).
    fn capture_centre(&self) -> Result<u16, UserError> {
        self.state.capture_centre().map_err(capture_user_error)
    }

    /// Capture one pedal's released endpoint from the live raw
    /// reading (v2.0.2).
    fn capture_pedal_min(&self, pedal: &str) -> Result<u16, UserError> {
        self.state
            .capture_pedal_min(pedal)
            .map_err(capture_user_error)
    }

    /// Capture one pedal's fully-pressed endpoint from the live raw
    /// reading (v2.0.2).
    fn capture_pedal_max(&self, pedal: &str) -> Result<u16, UserError> {
        self.state
            .capture_pedal_max(pedal)
            .map_err(capture_user_error)
    }

    /// All persisted button→action bindings (v2.0.2), in button
    /// order.
    fn list_bindings(&self) -> Vec<ButtonBinding> {
        self.state.list_bindings()
    }

    /// Bind a wheel button to an action and persist it (v2.0.2).
    /// The error code is the store's stable code
    /// (`InvalidButton` / `UnknownAction` / `BindingSaveFailed`),
    /// so the UI can explain the failure instead of showing a bare
    /// "Unknown" (see [`binding_user_error`]).
    fn set_binding(&self, button: u8, action: &str) -> Result<ButtonBinding, UserError> {
        self.state
            .set_binding(button, action)
            .map_err(|error| binding_user_error(error.code, error.message))
    }

    /// Remove a button's binding (v2.0.2). Returns `true` when a
    /// binding was actually removed.
    fn clear_binding(&self, button: u8) -> Result<bool, UserError> {
        self.state
            .clear_binding(button)
            .map_err(|error| binding_user_error(error.code, error.message))
    }

    fn rename_profile(&self, id: &str, name: &str) -> Result<String, UserError> {
        self.state
            .rename_profile(id, name)
            .map(|summary| summary.name)
            .map_err(|detail| UserError::from_connection(ConnectionError::Unknown, detail))
    }

    fn link_profile(&self, id: &str, game_slug: &str) -> Result<String, UserError> {
        self.state
            .link_profile(id, game_slug)
            .map(|summary| summary.name)
            .map_err(|detail| UserError::from_connection(ConnectionError::Unknown, detail))
    }

    fn restore_backup(&self) -> Result<String, UserError> {
        self.state
            .restore_latest_backup()
            .map_err(|detail| UserError::from_connection(ConnectionError::Unknown, detail))
    }

    fn knob_actions(&self) -> [[String; 3]; 2] {
        self.state.knob_actions()
    }

    fn set_knob_action(&self, knob: u8, row: u8, action: &str) -> Result<(), UserError> {
        self.state
            .set_knob_action(knob, row, action)
            .map_err(|detail| UserError::from_connection(ConnectionError::Unknown, detail))
    }

    fn vjoy_status(&self) -> Result<VjoyStatus, UserError> {
        Ok(self.state.vjoy_status())
    }

    fn install_vjoy(&self) -> Result<String, UserError> {
        self.state
            .install_vjoy()
            .map_err(|detail| UserError::from_connection(ConnectionError::Unknown, detail))
    }

    fn diag_stats(&self) -> Result<DiagStats, UserError> {
        let mut stats = self.state.diag_stats();
        // DS-82: the IPC dispatch latency aggregate, measured by
        // the server on every dispatch.
        if let Some((p50, p99)) = rsh_ipc::ipc_latency_global_p50_p99_ms() {
            stats.ipc_p50_ms = Some(p50);
            stats.ipc_p99_ms = Some(p99);
        }
        // R2-46: the per-method detail — which method is slow.
        // `ipc_latency_snapshot` is `(method, calls, p50 µs, p99 µs)`,
        // sorted by method name; µs → ms here. The µs values are
        // dispatch durations — far below 2^53 µs — so the `u64 as
        // f64` cast cannot lose precision.
        #[allow(clippy::cast_precision_loss)]
        let per_method: Vec<rsh_api::IpcMethodLatency> = rsh_ipc::ipc_latency_snapshot()
            .into_iter()
            .map(
                |(method, calls, p50_us, p99_us)| rsh_api::IpcMethodLatency {
                    method,
                    calls,
                    p50_ms: p50_us as f64 / 1000.0,
                    p99_ms: p99_us as f64 / 1000.0,
                },
            )
            .collect();
        stats.ipc_per_method_ms = per_method;
        Ok(stats)
    }
}

/// Classify a core capture failure into a typed [`UserError`]
/// (v2.0.2). The core reports plain strings; the IPC layer only
/// carries the code + detail (see `rsh_ipc::dispatch`), so the code
/// is the stable handle the UI matches on and the detail is the
/// human reason. Prefixes are the core's own message starts — the
/// match is on the failure class, never on a substring of a value.
#[must_use]
pub fn capture_user_error(detail: String) -> UserError {
    let (code, title, what_to_do) = if detail.starts_with("no live wheel data")
        || detail.starts_with("live wheel data is stale")
        || detail.starts_with("no steering reading")
        || detail.starts_with("no live reading")
    {
        (
            "NoLiveData",
            "No live wheel data",
            "Connect the wheel and wait for live data, then try again.",
        )
    } else if detail.starts_with("unknown pedal") {
        (
            "UnknownPedal",
            "Unknown pedal",
            "Pick the accelerator, brake, clutch, or handbrake.",
        )
    } else if detail.contains("is not below its maximum") || detail.contains("spans only") {
        (
            "BadRange",
            "Pedal range too small",
            "Move the pedal through its full travel — release it fully, then press it fully — and capture each end.",
        )
    } else if detail.starts_with("no active profile") {
        (
            "NoActiveProfile",
            "No active profile",
            "Activate a profile first, then capture again.",
        )
    } else if detail.contains("is outside 0..=3300") {
        (
            "ReadingOutOfRange",
            "Reading out of range",
            "Check the wheel wiring, then try again.",
        )
    } else {
        (
            "Unknown",
            "Something went wrong",
            "Retry, then check the diagnostics log.",
        )
    };
    UserError {
        code: code.to_string(),
        title: title.to_string(),
        what_happened: detail.clone(),
        what_to_do: what_to_do.to_string(),
        detail: Some(detail),
    }
}

/// Map a binding failure to a typed [`UserError`] (v2.0.2). The
/// store's code is already stable (`InvalidButton` /
/// `UnknownAction` / `BindingSaveFailed`) — it is carried verbatim,
/// with a title and a "what to do" per code, so the UI matches on
/// the code and the IPC layer only has to carry `code|detail` (see
/// `rsh_ipc::dispatch`). Takes the code + message instead of the
/// store error type so this crate needs no `rsh-profiles`
/// dependency.
#[must_use]
pub fn binding_user_error(code: &str, message: String) -> UserError {
    let (title, what_to_do) = match code {
        "InvalidButton" => (
            "Button cannot be bound",
            "Bind a physical button (1-32) or an encoder button (65-72).",
        ),
        "UnknownAction" => (
            "Unknown action",
            "Pick a game input (horn, shifter_up, shifter_down, gear_1-6, handbrake) or an app action (profile_next, profile_previous).",
        ),
        "BindingSaveFailed" => (
            "Could not save the binding",
            "Check the RSH data folder is writable, then try again.",
        ),
        _ => (
            "Something went wrong",
            "Retry, then check the diagnostics log.",
        ),
    };
    UserError {
        code: code.to_string(),
        title: title.to_string(),
        what_happened: message.clone(),
        what_to_do: what_to_do.to_string(),
        detail: Some(message),
    }
}

/// Map the store's `ProfileSummary` to the API wire shape (same
/// fields — the copy exists only because `rsh-api` cannot depend on
/// `rsh-profiles`).
fn to_api_profile(summary: &CoreProfileSummary) -> ProfileSummary {
    ProfileSummary {
        id: summary.id.clone(),
        name: summary.name.clone(),
        game: summary.game.clone(),
        active: summary.active,
    }
}

/// Map the core's `GameInfo` to the API wire shape (same fields).
fn to_api_game(game: &GameInfo) -> GameSummary {
    GameSummary {
        id: game.id.clone(),
        name: game.name.clone(),
        running: game.running,
        installed: game.installed,
        bound_profile_id: game.bound_profile_id.clone(),
    }
}

/// Build the `AppEvent::ConnectionChanged` for the current manager state.
pub fn connection_changed_event(api: &DaemonApi) -> AppEvent {
    let info = api.connection();
    let reason = match &info.state {
        MgrState::Error { reason, .. } => Some(*reason),
        _ => None,
    };
    AppEvent::ConnectionChanged {
        state: info.state,
        label: info.label,
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("rsh-daemon-capture-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn live_api() -> (Arc<DaemonApi>, std::path::PathBuf) {
        live_api_named("api")
    }

    fn live_api_named(name: &str) -> (Arc<DaemonApi>, std::path::PathBuf) {
        let dir = temp_dir(name);
        let state =
            Arc::new(AppStateCore::open_with_null_sink(&dir).expect("open state with null sink"));
        let first = state
            .list_profiles()
            .into_iter()
            .next()
            .expect("default profile exists");
        state
            .activate_profile(&first.id)
            .expect("activate default profile");
        (DaemonApi::new(state), dir)
    }

    /// R2-46: `diag_stats` exports the per-method IPC latency —
    /// the method that is slow must be visible, not just the global
    /// aggregate. (The latency rings are process-global, so the
    /// assertion is "the recorded method is present", not exact
    /// counts — other tests in this process may record too.)
    #[test]
    fn diag_stats_exports_per_method_ipc_latency() {
        use std::time::Duration;

        let (api, _dir) = live_api_named("diag-stats");
        rsh_ipc::record_ipc_latency("poll", Duration::from_micros(2_000));
        let stats = api.diag_stats().expect("diag_stats");
        let entry = stats
            .ipc_per_method_ms
            .iter()
            .find(|entry| entry.method == "poll")
            .expect("poll entry present");
        assert!(entry.calls >= 1);
        assert!(entry.p50_ms >= 0.0);
        assert!(entry.p99_ms >= entry.p50_ms);
        assert!(
            stats.ipc_p50_ms.is_some() && stats.ipc_p99_ms.is_some(),
            "global aggregate still filled: {stats:?}"
        );
    }

    #[test]
    fn capture_user_error_classifies_core_failures() {
        let no_data = capture_user_error("no live wheel data yet — connect the wheel".into());
        assert_eq!(no_data.code, "NoLiveData");
        assert_eq!(no_data.title, "No live wheel data");
        assert!(no_data.detail.is_some(), "the reason is kept in detail");
        let stale = capture_user_error(
            "live wheel data is stale (3000 ms since the last frame) — reconnect the wheel".into(),
        );
        assert_eq!(stale.code, "NoLiveData");
        let bad_range = capture_user_error(
            "pedal 'accel' range 3000..=3300 mV spans only 300 mV (need at least 660 mV) — move the pedal through its full travel and capture again".into(),
        );
        assert_eq!(bad_range.code, "BadRange");
        let unknown = capture_user_error(
            "unknown pedal 'gear' (want accel, brake, clutch, or handbrake)".into(),
        );
        assert_eq!(unknown.code, "UnknownPedal");
        let other = capture_user_error("no active profile".into());
        assert_eq!(other.code, "NoActiveProfile");
    }

    #[test]
    fn daemon_api_capture_round_trips_the_live_reading() {
        let (api, _dir) = live_api();
        // No frame yet: an honest typed failure, not a fake capture.
        let error = api
            .capture_centre()
            .expect_err("capture without data must fail");
        assert_eq!(error.code, "NoLiveData");
        assert!(
            error
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("no live wheel data")),
            "detail keeps the reason: {error:?}"
        );
        api.state.note_raw_axes([
            Some(1812),
            Some(320),
            Some(300),
            None,
            None,
            None,
            None,
            None,
        ]);
        let reading = api.raw_mv().expect("raw reading");
        assert_eq!(reading.steering_mv, Some(1812));
        assert_eq!(reading.accel_mv, Some(320));
        assert_eq!(api.capture_centre().expect("capture centre"), 1812);
        let calibration = api.calibration().expect("calibration after capture");
        assert_eq!(calibration.steering_center_mv, 1812.0);
        assert_eq!(
            api.capture_pedal_min("accel").expect("capture accel min"),
            320
        );
        api.state.note_raw_axes([
            Some(1812),
            Some(3200),
            Some(300),
            None,
            None,
            None,
            None,
            None,
        ]);
        assert_eq!(
            api.capture_pedal_max("accel").expect("capture accel max"),
            3200
        );
        let calibration = api.calibration().expect("calibration after pedal capture");
        assert_eq!(
            calibration.pedal_min_mv.get("accel"),
            Some(&320.0),
            "pedal min round-trips through the API"
        );
        assert_eq!(calibration.pedal_max_mv.get("accel"), Some(&3200.0));
    }

    #[test]
    fn daemon_api_bindings_round_trip_and_error_codes() {
        let (api, _dir) = live_api_named("bindings");
        assert_eq!(api.list_bindings().len(), 0);
        let bound = api.set_binding(5, "horn").expect("bind button 5");
        assert_eq!(bound.button, 5);
        assert_eq!(bound.action, "horn");
        let bound = api
            .set_binding(65, "profile_next")
            .expect("bind encoder button");
        assert_eq!(bound.button, 65);
        let listed = api.list_bindings();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].button, 5, "button order, not insertion order");
        assert_eq!(listed[1].button, 65);
        // Rebind the same button: the new action wins.
        assert_eq!(
            api.set_binding(5, "shifter_up").expect("rebind").action,
            "shifter_up"
        );
        assert_eq!(api.list_bindings().len(), 2);
        // Clear: true when bound, false when absent.
        assert!(api.clear_binding(5).expect("clear bound"));
        assert!(!api.clear_binding(5).expect("clear absent"));
        assert_eq!(api.list_bindings().len(), 1);
        // Error codes are stable, not a bare "Unknown".
        assert_eq!(
            api.set_binding(0, "horn").expect_err("button 0").code,
            "InvalidButton"
        );
        assert_eq!(
            api.set_binding(33, "horn").expect_err("button 33").code,
            "InvalidButton"
        );
        let error = api
            .set_binding(5, "warp_drive")
            .expect_err("unknown action");
        assert_eq!(error.code, "UnknownAction");
        assert_eq!(error.title, "Unknown action");
        assert!(
            error
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("warp_drive")),
            "detail keeps the reason: {error:?}"
        );
    }

    #[test]
    fn binding_user_error_maps_each_stable_code() {
        let invalid = binding_user_error("InvalidButton", "button 0 cannot be bound".to_string());
        assert_eq!(invalid.code, "InvalidButton");
        assert_eq!(invalid.title, "Button cannot be bound");
        let unknown = binding_user_error("UnknownAction", "unknown action 'nope'".to_string());
        assert_eq!(unknown.code, "UnknownAction");
        assert!(
            unknown
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("nope"))
        );
        let saved = binding_user_error("BindingSaveFailed", "disk full".to_string());
        assert_eq!(saved.code, "BindingSaveFailed");
        assert_eq!(saved.detail.as_deref(), Some("disk full"));
    }

    #[test]
    fn poll_agrees_with_the_separate_getters_at_the_same_instant() {
        // L-4 (v2.0.2): one `poll` carries snapshot + connection +
        // output, and — with no engine tick or manager transition
        // in between — it agrees exactly with the three separate
        // getters: same snapshot `seq`, same connection, same
        // output flag. (The tearing mechanism itself — state
        // changing between separate reads — is proven by the
        // flipping-stub test in `rsh-api`.)
        let (api, _dir) = live_api_named("poll");
        let polled = api.poll();
        assert_eq!(
            polled.snapshot,
            api.snapshot(),
            "poll's snapshot is the live snapshot"
        );
        assert_eq!(
            polled.connection,
            api.connection(),
            "poll's connection is the live connection"
        );
        assert_eq!(polled.output_enabled, api.output_enabled());
        // No engine tick yet: seq 0, zeroed snapshot — never a
        // fake reading.
        assert_eq!(polled.snapshot.seq, 0);
        assert_eq!(polled.snapshot.steering_deg, 0.0);
        // A second poll is identical while nothing moves.
        assert_eq!(api.poll(), polled);
    }

    #[test]
    fn connection_label_is_never_connected_outside_active() {
        // DS-18: for every non-Active manager state, the label must
        // not be "Connected" even when the core transport still
        // reports Connected with an open session gate (the watchdog
        // window: manager in Backoff, core not yet disconnected).
        // The old code upgraded the label from the transport state
        // and published `state: Backoff, label: "Connected"`.
        let transport = TransportState {
            transport: "wifi".to_string(),
            state: rsh_app_core::snapshot::ConnectionState::Connected,
            address: "192.168.4.1".to_string(),
            error: None,
            session_active: true,
            frames_held_by_gate: 0,
        };
        for mgr in [
            MgrState::Idle,
            MgrState::LinkUp,
            MgrState::Hello,
            MgrState::Authenticating,
            MgrState::Backoff {
                wait_ms: 2000,
                attempt: 1,
            },
            MgrState::Error {
                reason: ConnectionError::Unreachable,
                hint: "board not responding".to_string(),
                retry_in_ms: None,
            },
        ] {
            let info = DaemonApi::connection_from(&mgr, &transport);
            assert_ne!(
                info.label, "Connected",
                "manager state {mgr:?} must not label Connected"
            );
            assert_eq!(info.state, mgr);
        }
        // And Active still reports Connected.
        let info = DaemonApi::connection_from(&MgrState::Active, &transport);
        assert_eq!(info.label, "Connected");
        assert_eq!(info.state, MgrState::Active);
    }

    #[test]
    fn set_output_enabled_persists_the_flag() {
        // DS-41: the toggle used to live only in memory — the UI
        // polls `settings().output_enabled`, so a restart reverted
        // it and a persist failure was invisible. Now the flag is
        // persisted, and the polled value agrees with the toggle.
        let (api, _dir) = live_api_named("output-persist");
        api.set_output_enabled(true);
        assert!(
            api.state.settings().output_enabled,
            "the toggle must persist"
        );
        assert!(api.output_enabled());
        api.set_output_enabled(false);
        assert!(
            !api.state.settings().output_enabled,
            "toggling off must persist too"
        );
    }

    /// Phase 2 (RC-6): the ~1 Hz `slow_poll` never ships the
    /// one-time pairing password over IPC and never consumes it —
    /// the password rides only on a direct `pairing_status()` read.
    /// With no board present there is no password at all, so the
    /// test asserts the structural contract that matters to the UI:
    /// the attempt trail still rides on `slow_poll` (the UI can
    /// show what happened) while `password` is always `None`.
    /// Non-consumption itself is proven in rsh-app-core by
    /// `pairing_password_is_single_use` (peek never consumes).
    #[test]
    fn phase2_slow_poll_never_ships_the_pairing_password() {
        let (api, _dir) = live_api_named("slow-poll-no-password");
        api.begin_pairing()
            .expect("begin_pairing triggers an attempt");
        let polled = api.slow_poll();
        assert_eq!(
            polled.pairing_status.password, None,
            "RC-6: the 1 Hz poll must never ship the one-time password"
        );
        assert!(
            polled
                .pairing_status
                .attempt_log
                .iter()
                .any(|line| line.contains("begin_pairing")),
            "RC-6: the attempt trail must still ride on slow_poll: {:?}",
            polled.pairing_status.attempt_log
        );
        // A second poll is identical in this respect — nothing was
        // consumed by the first one.
        assert_eq!(api.slow_poll().pairing_status.password, None);
        assert_eq!(api.pairing_status().password, None);
    }

    #[test]
    fn pairing_status_carries_the_attempt_trail() {
        // DS-25: the wizard polls `pairing_status`; the attempt
        // trail (attempt line, HELO sends, HELLO timeout) must ride
        // on it so the UI can show what actually happened instead
        // of only "No pairing window".
        let (api, _dir) = live_api_named("pairing-trail");
        assert!(
            api.pairing_status().attempt_log.is_empty(),
            "no attempt yet: the trail is empty, never fake"
        );
        // The forwarder needs UDP 4211 free; another test process
        // may hold it briefly, so probe first and retry the
        // attempt (up to 3 rounds) instead of failing on a
        // transient foreign bind.
        let udp_4211_free = || std::net::UdpSocket::bind("127.0.0.1:4211").is_ok();
        let mut trail: Vec<String> = Vec::new();
        for _attempt in 0..3 {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while !udp_4211_free() && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(100));
            }
            api.begin_pairing()
                .expect("begin_pairing triggers an attempt");
            trail = api.pairing_status().attempt_log;
            assert!(
                trail.iter().any(|line| line.contains("begin_pairing")),
                "trail must start with the attempt line: {trail:?}"
            );
            let wait_deadline = std::time::Instant::now() + Duration::from_secs(10);
            while !trail.iter().any(|line| line.contains("HELLO timeout"))
                && std::time::Instant::now() < wait_deadline
            {
                std::thread::sleep(Duration::from_millis(100));
                trail = api.pairing_status().attempt_log;
            }
            if trail.iter().any(|line| line.contains("HELLO timeout")) {
                break;
            }
            let _ = api.state.stop_engine();
        }
        assert!(
            trail.iter().any(|line| line.contains("HELLO timeout")),
            "trail must record the timeout: {trail:?}"
        );
        assert!(
            trail
                .iter()
                .any(|line| line.contains("HELO sent") || line.contains("HELO send failed")),
            "trail must record the HELO attempts: {trail:?}"
        );
        // (In a sandbox with no route to 192.168.4.1 the send fails
        // with EPERM and the failure is logged instead — either way
        // the attempt is on the trail.)
        let _ = api.state.stop_engine();
    }

    #[test]
    fn watchdog_tick_feeds_live_frames_and_fires_when_stale() {
        // F20: the manager reaches Active, a frame is stamped just
        // now — the tick must NOT rebuild (pre-fix it rebuilt 2 s
        // after every CapsOk, frames or not, because nothing ever
        // fed FrameSeen). Then the frame goes stale (>2 s, no new
        // frame) — the same tick must rebuild, proving the feed
        // expires instead of masking a dead board forever.
        let (api, _dir) = live_api_named("watchdog-feed");
        api.manager_event(ConnEvent::Connect);
        api.manager_event(ConnEvent::HelloReceived {
            unpaired: false,
            window_open: false,
        });
        api.manager_event(ConnEvent::Authenticating);
        api.manager_event(ConnEvent::CapsOk);
        assert!(api.manager_state().is_active());
        api.state.note_stream_frame();
        assert_eq!(
            api.watchdog_tick(),
            rsh_api::ManagerAction::None,
            "a live frame must suppress the watchdog"
        );
        assert!(api.manager_state().is_active());
        std::thread::sleep(Duration::from_millis(2100));
        assert_eq!(
            api.watchdog_tick(),
            rsh_api::ManagerAction::RebuildTransport,
            "a frame 2.1 s old must not feed the watchdog"
        );
    }
}
