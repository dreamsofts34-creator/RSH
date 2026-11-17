//! Simulated `AppApi` implementation (Phase 3 fallback).
//!
//! The UI is built against the real [`rsh_api::AppApi`] trait — it calls
//! `connection()`, `snapshot()`, `output_enabled()` / `set_output_enabled()`
//! on a `dyn AppApi`, never on ad-hoc data. When the Phase 2 daemon is
//! reachable on its IPC socket the UI uses [`crate::ipc_api::IpcApi`]
//! instead; this in-process simulation (sine sweeps, like the Phase 1
//! spike) is only the fallback for when no daemon is running. The swap
//! is a constructor change only: no UI code and no trait change.
//!
//! The window always shows a "Simulated data" banner while this
//! implementation is in use, so simulated values are never presented
//! as live board data. Tuning edits go through the same `set_tuning`
//! / `save_draft` surface as the live path (kept in memory, validated
//! the same way) so the UI wiring is identical in both modes.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use rsh_api::{
    app_version, connection::ConnectionState, validate_tuning, ApiTransport, AppApi,
    CalibrationInfo, ConnectionError, ConnectionInfo, DiagStats, DraftStatus, GameSummary,
    PairingInfo, ProfileSummary, SnapshotInfo, TuningSettings, UserError, VjoyStatus,
};

/// Simulated live state: just a clock and the output flag.
pub struct SimApi {
    t: Mutex<f64>,
    send_to_game: Mutex<bool>,
    tunings: Mutex<BTreeMap<String, TuningSettings>>,
    dirty_modules: Mutex<BTreeSet<String>>,
    autostart: Mutex<bool>,
    backend: Mutex<String>,
}

impl SimApi {
    /// Build with output on and the clock at zero.
    #[must_use]
    pub fn new() -> Self {
        Self {
            t: Mutex::new(0.0),
            send_to_game: Mutex::new(true),
            tunings: Mutex::new(BTreeMap::new()),
            dirty_modules: Mutex::new(BTreeSet::new()),
            // Fresh-store default (v2.0.1): the always-on daemon
            // autostarts, and vJoy is the default backend.
            autostart: Mutex::new(true),
            backend: Mutex::new("vJoy".to_string()),
        }
    }

    /// Advance the simulation clock by `dt` seconds.
    ///
    /// Currently unused (B1, v2.0.2): the sim's sine snapshot is
    /// never displayed, so the frame timer no longer advances it.
    /// Kept for dev/test use — a frozen sim reads its t=0 values.
    #[allow(dead_code)]
    pub fn advance(&self, dt: f64) {
        if let Ok(mut t) = self.t.lock() {
            *t += dt;
        }
    }

    /// Current simulation time (seconds).
    #[must_use]
    pub fn time(&self) -> f64 {
        self.t.lock().map_or(0.0, |t| *t)
    }
}

impl Default for SimApi {
    fn default() -> Self {
        Self::new()
    }
}

impl SimApi {
    fn clear_tunings(&self) {
        if let Ok(mut tunings) = self.tunings.lock() {
            tunings.clear();
        }
    }

    /// Honest error for calls that only the daemon can serve — the
    /// sim never fakes a success or a value for these.
    fn needs_daemon(method: &str) -> UserError {
        UserError::from_connection(
            ConnectionError::Unknown,
            format!("{method} needs the daemon"),
        )
    }
}

impl AppApi for SimApi {
    fn version(&self) -> String {
        app_version().to_string()
    }

    fn connection(&self) -> ConnectionInfo {
        ConnectionInfo {
            state: ConnectionState::Active,
            label: "Connected".to_string(),
            transport: ApiTransport::Wifi,
            address: "rsh.local".to_string(),
            session_active: true,
            error: None,
            hint: None,
        }
    }

    fn connect(&self, _transport: ApiTransport) -> Result<ConnectionInfo, UserError> {
        Ok(self.connection())
    }

    fn disconnect(&self) -> Result<ConnectionInfo, UserError> {
        Ok(ConnectionInfo {
            state: ConnectionState::Idle,
            label: "Disconnected".to_string(),
            transport: ApiTransport::Wifi,
            address: String::new(),
            session_active: false,
            error: None,
            hint: None,
        })
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
        let t = self.time();
        SnapshotInfo {
            steering_deg: 120.0 * (t * 0.4).sin(),
            throttle: 0.5 + 0.5 * (t * 0.7).sin(),
            brake: (0.5 + 0.5 * (t * 0.9 + 1.3).sin()).clamp(0.0, 1.0),
            clutch: Some(0.15 + 0.1 * (t * 0.5).sin()),
            // H10: the sim has no wheel — no button is ever pressed.
            button_mask: 0,
            // M-3: the sim has no handbrake or shifter either.
            handbrake: 0.0,
            gear: 0,
        }
    }

    fn output_enabled(&self) -> bool {
        self.send_to_game.lock().map_or(true, |f| *f)
    }

    fn set_output_enabled(&self, enabled: bool) {
        if let Ok(mut f) = self.send_to_game.lock() {
            *f = enabled;
        }
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
        if let Ok(mut dirty) = self.dirty_modules.lock() {
            dirty.insert(tuning.module.clone());
        }
        Ok(tuning.clone())
    }

    fn draft_status(&self) -> Option<DraftStatus> {
        let dirty_modules: Vec<String> = self
            .dirty_modules
            .lock()
            .ok()
            .map(|dirty| dirty.iter().cloned().collect())
            .unwrap_or_default();
        if dirty_modules.is_empty() {
            return None;
        }
        Some(DraftStatus {
            profile_id: "sim".to_string(),
            profile_name: "Simulated".to_string(),
            dirty: true,
            dirty_modules,
        })
    }

    fn save_draft(&self) -> Result<bool, UserError> {
        let had_dirty = self.dirty_modules.lock().is_ok_and(|mut dirty| {
            let had = !dirty.is_empty();
            dirty.clear();
            had
        });
        Ok(had_dirty)
    }

    fn discard_draft(&self) -> bool {
        let had_dirty = self.dirty_modules.lock().is_ok_and(|mut dirty| {
            let had = !dirty.is_empty();
            dirty.clear();
            had
        });
        if had_dirty {
            self.clear_tunings();
        }
        had_dirty
    }

    fn autostart(&self) -> bool {
        self.autostart.lock().map_or(true, |flag| *flag)
    }

    fn set_autostart(&self, enabled: bool) -> Result<bool, UserError> {
        if let Ok(mut flag) = self.autostart.lock() {
            *flag = enabled;
        }
        Ok(enabled)
    }

    /// H5 (v2.0.2): the sim has no wheel behind it, so it reports
    /// no firmware version — never the hardcoded "4.0.2". The UI
    /// shows "—" for "Unknown" everywhere (same as the live path
    /// when the daemon cannot be reached).
    fn firmware_version(&self) -> String {
        "Unknown".to_string()
    }

    fn output_backend(&self) -> String {
        self.backend
            .lock()
            .map_or_else(|_| "vJoy".to_string(), |name| name.clone())
    }

    fn set_output_backend(&self, name: &str) -> Result<String, UserError> {
        if !matches!(name, "HIDMaestro" | "vJoy") {
            return Err(UserError::from_connection(
                rsh_api::ConnectionError::Unknown,
                format!("unknown output backend '{name}' (expected HIDMaestro or vJoy)"),
            ));
        }
        if let Ok(mut backend) = self.backend.lock() {
            *backend = name.to_string();
        }
        Ok(name.to_string())
    }

    fn backup_now(&self) -> Result<String, UserError> {
        // The sim must not pretend a backup happened: there is no
        // daemon store behind it.
        Err(UserError::from_connection(
            rsh_api::ConnectionError::Unknown,
            "backup needs the daemon",
        ))
    }

    // v2.0.1 hotfix: the sim never invents profiles, games, or
    // learn picks — lists are empty, fallible calls error honestly
    // (the UI must show "needs the daemon", never fake data).
    fn list_profiles(&self) -> Vec<ProfileSummary> {
        Vec::new()
    }

    fn activate_profile(&self, _id: &str) -> Result<String, UserError> {
        Err(Self::needs_daemon("activate_profile"))
    }

    fn create_profile(&self, _name: &str) -> Result<String, UserError> {
        Err(Self::needs_daemon("create_profile"))
    }

    fn duplicate_profile(&self, _id: &str) -> Result<String, UserError> {
        Err(Self::needs_daemon("duplicate_profile"))
    }

    fn delete_profile(&self, _id: &str) -> Result<bool, UserError> {
        Err(Self::needs_daemon("delete_profile"))
    }

    fn list_games(&self) -> Vec<GameSummary> {
        Vec::new()
    }

    fn launch_game(&self, _id: &str) -> Result<GameSummary, UserError> {
        Err(Self::needs_daemon("launch_game"))
    }

    fn stop_game(&self, _id: &str) -> Result<GameSummary, UserError> {
        Err(Self::needs_daemon("stop_game"))
    }

    fn game_state(&self, _id: &str) -> Result<GameSummary, UserError> {
        Err(Self::needs_daemon("game_state"))
    }

    fn start_learn(&self) -> Result<bool, UserError> {
        Err(Self::needs_daemon("start_learn"))
    }

    fn cancel_learn(&self) -> Result<bool, UserError> {
        Err(Self::needs_daemon("cancel_learn"))
    }

    // v2.0.1 hotfix BACKEND-2: the sim never fakes a calibration, a
    // vJoy install, or diagnostics — all four need the daemon.
    fn calibration(&self) -> Result<CalibrationInfo, UserError> {
        Err(Self::needs_daemon("calibration"))
    }

    fn vjoy_status(&self) -> Result<VjoyStatus, UserError> {
        Err(Self::needs_daemon("vjoy_status"))
    }

    fn install_vjoy(&self) -> Result<String, UserError> {
        Err(Self::needs_daemon("install_vjoy"))
    }

    fn diag_stats(&self) -> Result<DiagStats, UserError> {
        Err(Self::needs_daemon("diag_stats"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sim_tuning_round_trip_and_draft_lifecycle() {
        let api = SimApi::new();
        assert!(api.draft_status().is_none());
        assert_eq!(api.get_tuning("steering").curve, 1.0);
        let mut upd = TuningSettings::neutral("steering");
        upd.deadzone = 5.0;
        upd.curve = 1.6;
        let stored = api.set_tuning(&upd).expect("valid tuning");
        assert_eq!(stored.deadzone, 5.0);
        assert_eq!(api.get_tuning("steering").deadzone, 5.0);
        let status = api.draft_status().expect("dirty after edit");
        assert!(status.dirty);
        assert_eq!(status.dirty_modules, vec!["steering"]);
        let mut bad = TuningSettings::neutral("steering");
        bad.curve = 9.0;
        assert!(api.set_tuning(&bad).is_err());
        assert!(api.save_draft().expect("save"));
        assert!(api.draft_status().is_none());
        assert!(!api.discard_draft());
        api.set_tuning(&upd).expect("edit again");
        assert!(api.discard_draft());
        assert_eq!(api.get_tuning("steering").deadzone, 0.0);
    }

    #[test]
    fn sim_settings_defaults_and_validation() {
        let api = SimApi::new();
        // v2.0.1 defaults: autostart on, vJoy backend.
        assert!(api.autostart());
        assert!(!api.set_autostart(false).expect("disable autostart"));
        assert!(!api.autostart());
        assert!(api.set_autostart(true).expect("enable autostart"));
        assert!(api.autostart());
        // H5: the sim reports no firmware version (never "4.0.2").
        assert_eq!(api.firmware_version(), "Unknown");
        assert_eq!(api.output_backend(), "vJoy");
        assert_eq!(api.set_output_backend("vJoy").expect("vJoy"), "vJoy");
        assert_eq!(api.output_backend(), "vJoy");
        assert!(api.set_output_backend("none").is_err());
        assert!(api.set_output_backend("bogus").is_err());
        assert_eq!(api.output_backend(), "vJoy");
        // The sim never pretends a backup happened.
        assert!(api.backup_now().is_err());
    }

    #[test]
    fn sim_profiles_games_learn_never_fake() {
        let api = SimApi::new();
        // Empty lists, never invented rows.
        assert_eq!(api.list_profiles().len(), 0);
        assert_eq!(api.list_games().len(), 0);
        // Every fallible call errors honestly.
        assert!(api.activate_profile("x").is_err());
        assert!(api.create_profile("New").is_err());
        assert!(api.duplicate_profile("x").is_err());
        assert!(api.delete_profile("x").is_err());
        assert!(api.launch_game("assetto_corsa").is_err());
        assert!(api.stop_game("assetto_corsa").is_err());
        assert!(api.game_state("assetto_corsa").is_err());
        assert!(api.start_learn().is_err());
        assert!(api.cancel_learn().is_err());
        // Calibration / vJoy / diagnostics never fake in the sim.
        assert!(api.calibration().is_err());
        assert!(api.vjoy_status().is_err());
        assert!(api.install_vjoy().is_err());
        assert!(api.diag_stats().is_err());
        // Learn is never armed in the sim, and no pick ever appears.
        assert!(!api.is_learning());
        assert_eq!(api.take_learned_button(), None);
    }
}
