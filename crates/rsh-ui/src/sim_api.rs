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
    ConnectionInfo, DraftStatus, PairingInfo, SnapshotInfo, TuningSettings, UserError,
};

/// Simulated live state: just a clock and the output flag.
pub struct SimApi {
    t: Mutex<f64>,
    send_to_game: Mutex<bool>,
    tunings: Mutex<BTreeMap<String, TuningSettings>>,
    dirty_modules: Mutex<BTreeSet<String>>,
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
        }
    }

    /// Advance the simulation clock by `dt` seconds.
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
}
