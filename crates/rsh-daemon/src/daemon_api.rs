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

use rsh_api::connection::ConnectionState as MgrState;
use rsh_api::errors::{ConnectionError, UserError};
use rsh_api::{
    ApiTransport, AppApi, AppEvent, CalibrationInfo, ConnectionInfo, DiagStats, DraftStatus,
    GameSummary, PairingInfo, ProfileSummary, SnapshotInfo, TuningSettings, VjoyStatus,
    app_version,
};
use rsh_api::{ConnEvent, ConnectionManager};
use rsh_app_core::snapshot::ConnectionState as CoreState;
use rsh_app_core::state::{
    AppStateCore, GameInfo, ProfileSummary as CoreProfileSummary, TransportKind,
};

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
    pub fn watchdog_tick(&self) -> rsh_api::ManagerAction {
        self.manager
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .check_watchdog(std::time::Instant::now())
    }
}

impl AppApi for DaemonApi {
    fn version(&self) -> String {
        app_version().to_string()
    }

    fn connection(&self) -> ConnectionInfo {
        let transport_state = self.state.transport_state();
        let mgr = self.manager_state();
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
        let (state, label, error, hint) = match &mgr {
            MgrState::Active => (MgrState::Active, "Connected".to_string(), None, None),
            MgrState::Error { reason, hint, .. } => (
                mgr.clone(),
                "Error".to_string(),
                Some(*reason),
                Some(hint.clone()),
            ),
            other => {
                // Not Active: report the manager's label, but never
                // "Connected" — and if the core says Connected while
                // the gate is closed, downgrade (C-02 defence).
                let label = if transport_state.session_active
                    && matches!(transport_state.state, CoreState::Connected)
                {
                    "Connected".to_string()
                } else {
                    other.label().to_string()
                };
                (other.clone(), label, None, None)
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

    fn snapshot(&self) -> SnapshotInfo {
        let snap = self.state.snapshot();
        SnapshotInfo {
            steering_deg: snap.steering_deg,
            throttle: snap.pedals.accel,
            brake: snap.pedals.brake,
            clutch: snap.pedals.clutch,
        }
    }

    fn output_enabled(&self) -> bool {
        self.state.settings().output_enabled
    }

    fn set_output_enabled(&self, enabled: bool) {
        let _ = self.state.set_output_enabled(enabled);
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

    fn output_backend(&self) -> String {
        self.state.backend_name()
    }

    fn set_output_backend(&self, name: &str) -> Result<String, UserError> {
        self.state
            .set_output_backend(name)
            .map_err(|detail| UserError::from_connection(ConnectionError::Unknown, detail))?;
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
    // diagnostics. (No `set_centre` / `calibrate_pedal` /
    // `set_button_action` — no backend: raw mV never reaches the
    // state, and button→action rebinding is not a board feature. See
    // `AppApi::calibration`.)
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

    fn vjoy_status(&self) -> Result<VjoyStatus, UserError> {
        Ok(self.state.vjoy_status())
    }

    fn install_vjoy(&self) -> Result<String, UserError> {
        self.state
            .install_vjoy()
            .map_err(|detail| UserError::from_connection(ConnectionError::Unknown, detail))
    }

    fn diag_stats(&self) -> Result<DiagStats, UserError> {
        Ok(self.state.diag_stats())
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
