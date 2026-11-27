//! [`IJoystickOutput`] implementation over the `HIDMaestro` sidecar.
//!
//! Connect flow (mirrors the C# `HidMaestroOutput.TryConnect`):
//! sidecar spawn + `hello` → `install-driver` (fast path) → `create`
//! (`rsh-wheel-custom`) → capabilities → optional plan validation
//! (D11) → neutral → connected. Any step failing leaves the backend
//! disconnected with [`HidMaestroStatus`] explaining why — the app
//! keeps running with output disabled, and the selector (G4) falls back
//! to vJoy.
//!
//! The 200 Hz path is [`IJoystickOutput::update`]: heartbeat if due,
//! map the frame, submit. A submit error marks the backend
//! disconnected (the manager already counted the failure); the engine
//! treats it as "send neutral and keep ticking", never as fatal — the
//! next `try_connect` is the one retry.

use rsh_engine::pipeline::{OutputState, PipelinePlan};
use rsh_output::vjoy::VJoyError;
use rsh_output::{IJoystickOutput, ProcessedState};

use crate::error::HidMaestroError;
use crate::manager::{CreateInfo, HelloInfo, SidecarConfig, SidecarManager};
use crate::mapping::{
    CUSTOM_RAW_REPORT_SIZE, G29_HANDBRAKE_BUTTON, G29_RAW_REPORT_SIZE, G29PedalMap,
    HidMaestroCapabilities, build_g29_raw_report, build_gamepad_frame, build_raw_report,
    validate_capabilities_for_profile,
};
use crate::{G29_PROFILE_ID, IDENTITY_KEY, PINNED_VERSION, PROFILE_ID};

/// Copy shown on the Output page while this backend is active.
///
/// The UAC prompt is the headline UX cost of this backend (R-1): the
/// elevated helper runs for the **whole session**, so Windows asks on
/// every RSH launch while `HIDMaestro` is the active backend. vJoy needs
/// no prompt at runtime.
pub const UAC_NOTE: &str = "HIDMaestro runs its helper elevated for the whole session — Windows will ask (UAC) on every launch while this backend is active. vJoy needs no prompt.";

/// Configuration for [`HidMaestroOutput`].
#[derive(Clone, Debug)]
pub struct HidMaestroConfig {
    /// Sidecar process config (path, elevation, timeouts).
    pub sidecar: SidecarConfig,
    /// Submit the 15-byte raw report instead of the `SubmitState` frame.
    /// Default `true` (M3 — the raw path carries all 32 buttons plus
    /// encoder pulses' wire slot; the `SubmitState` path drops buttons
    /// 26–32). The Output page shows which path is active.
    pub use_raw_report: bool,
    /// `G29` spoof mode (v2.0.16): when true the backend creates the
    /// `HIDMaestro` built-in `logitech-g29` catalog profile (VID
    /// `0x046D` / PID `0xC24F`) instead of `rsh-wheel-custom`, and the
    /// raw-report path is FORCED (the `SubmitState` frame layout does
    /// not match the G29 profile). Default `false`. Set from the
    /// persisted `g29_spoof_mode` setting by the app-core factory;
    /// toggled live via [`IJoystickOutput::set_g29_spoof_mode`].
    pub g29_spoof: bool,
    /// G29 pedal layout (v2.0.17): which descriptor axes carry the
    /// pedals, from the persisted `g29_pedal_layout` setting
    /// (`"hidmaestro"` default | `"pcgamingwiki"` — see
    /// [`G29PedalMap::from_layout`]). Set from settings by the
    /// app-core factory; changed live via
    /// [`IJoystickOutput::set_g29_pedal_layout`].
    pub g29_pedal_map: G29PedalMap,
    /// G29 handbrake button (v2.0.17): which G29 button (1–25) the
    /// handbrake drives, from the persisted `g29_handbrake_button`
    /// setting (default 8 — [`G29_HANDBRAKE_BUTTON`]). Set from
    /// settings by the app-core factory; changed live via
    /// [`IJoystickOutput::set_g29_handbrake_button`].
    pub g29_handbrake_button: u8,
}

impl Default for HidMaestroConfig {
    fn default() -> Self {
        Self {
            sidecar: SidecarConfig::default(),
            use_raw_report: true,
            g29_spoof: false,
            g29_pedal_map: G29PedalMap::PROFILE,
            g29_handbrake_button: G29_HANDBRAKE_BUTTON,
        }
    }
}

impl HidMaestroConfig {
    /// Configuration pointing at an explicit sidecar path (tests, dev).
    #[must_use]
    pub fn with_sidecar_path(path: std::path::PathBuf) -> Self {
        Self {
            sidecar: SidecarConfig::with_path(path),
            use_raw_report: true,
            g29_spoof: false,
            g29_pedal_map: G29PedalMap::PROFILE,
            g29_handbrake_button: G29_HANDBRAKE_BUTTON,
        }
    }

    /// Configuration with G29 spoof mode on/off (tests, factories).
    #[must_use]
    pub fn with_g29_spoof(mut self, enabled: bool) -> Self {
        self.g29_spoof = enabled;
        self
    }

    /// Configuration with a G29 pedal layout (tests, factories):
    /// the map itself, parsed from the persisted setting by the
    /// caller (see [`G29PedalMap::from_layout`]).
    #[must_use]
    pub fn with_g29_pedal_map(mut self, map: G29PedalMap) -> Self {
        self.g29_pedal_map = map;
        self
    }

    /// Configuration with a G29 handbrake button (tests,
    /// factories): clamped to 1–25, the G29 profile's button range.
    #[must_use]
    pub fn with_g29_handbrake_button(mut self, button: u8) -> Self {
        self.g29_handbrake_button = button.clamp(1, 25);
        self
    }
}

/// The backend's state as the Output page shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HidMaestroStateKind {
    /// Never attempted (or cleanly disconnected).
    Idle,
    /// Connected and sending.
    Ready,
    /// Sidecar executable not found (reinstall RSH).
    SidecarMissing,
    /// Not elevated (UAC declined / not admin).
    NotAdmin,
    /// Driver not installed / install failed.
    DriverMissing,
    /// Profile refused or device create failed.
    CreateFailed,
    /// Sidecar died or stopped answering.
    SidecarFailed,
    /// Sidecar is alive but its `HIDMaestro` context could not be
    /// created (P1, v2.0.11 — `context-unavailable`): the driver
    /// install / device create was never attempted.
    ContextUnavailable,
    /// One retry used up — vJoy fallback is in effect until the user
    /// re-selects this backend. Retained for the status vocabulary
    /// (the Output page renders it with a "switched to vJoy"
    /// banner); `fail()` no longer produces it (P2-5, v2.0.12 scan
    /// — the retry-budget error maps to `Error`, because on the
    /// explicit path no fallback happens and the banner lied).
    FallbackToVJoy,
    /// Any other failure (message carries the detail).
    Error,
}

/// Snapshot for the Output page (G4): state + human-readable message +
/// diagnostics. The page renders `NotAdmin` with the UAC note and
/// `FallbackToVJoy` with a "switched to vJoy" banner.
#[derive(Clone, Debug, PartialEq)]
pub struct HidMaestroStatus {
    /// Machine-readable state.
    pub kind: HidMaestroStateKind,
    /// Human-readable message (shown verbatim).
    pub message: String,
    /// SDK version the sidecar reports (`"2.1.0"`).
    pub sdk_version: Option<String>,
    /// Driver version (`"2.1.0"` form), once created.
    pub driver_version: Option<String>,
    /// Transport index of the created controller, once created.
    pub transport_index: Option<i32>,
    /// Last submit latency in microseconds, once submitted.
    pub last_submit_latency_us: Option<f64>,
    /// Submits sent this session.
    pub submit_count: u64,
    /// True: this backend needs the elevated helper (always true —
    /// surfaced so the page can show the UAC copy).
    pub elevation_required: bool,
    /// UAC copy ([`UAC_NOTE`]).
    pub uac_note: &'static str,
}

impl HidMaestroStatus {
    /// Idle: never attempted or cleanly disconnected.
    #[must_use]
    pub fn idle() -> Self {
        Self {
            kind: HidMaestroStateKind::Idle,
            message: "HIDMaestro backend idle — not connected yet.".to_string(),
            sdk_version: None,
            driver_version: None,
            transport_index: None,
            last_submit_latency_us: None,
            submit_count: 0,
            elevation_required: true,
            uac_note: UAC_NOTE,
        }
    }
}

/// `HIDMaestro` output backend (G2): the default backend.
///
/// Created with [`HidMaestroOutput::new`] (default config, sidecar
/// resolved beside the app) or [`HidMaestroOutput::with_config`]
/// (explicit sidecar path — tests, dev). Connect flow mirrors the C#
/// `HidMaestroOutput.TryConnect`; see the module docs.
pub struct HidMaestroOutput {
    config: HidMaestroConfig,
    manager: SidecarManager,
    connected: bool,
    caps_known: bool,
    capabilities: HidMaestroCapabilities,
    status: HidMaestroStatus,
    last_error: Option<HidMaestroError>,
    raw_report: [u8; CUSTOM_RAW_REPORT_SIZE],
    /// G29 spoof mode (mirrors `config.g29_spoof`; toggled live via
    /// `set_g29_spoof_mode`). When true the active profile is
    /// `logitech-g29` and every submit is a 12-byte raw report.
    g29_spoof: bool,
    /// G29 pedal map in effect (mirrors `config.g29_pedal_map`;
    /// changed live via `set_g29_pedal_layout`). Consulted on every
    /// G29 tick — a layout change needs no restart.
    g29_pedal_map: G29PedalMap,
    /// G29 handbrake button in effect (mirrors
    /// `config.g29_handbrake_button`; changed live via
    /// `set_g29_handbrake_button`). Consulted on every G29 tick.
    g29_handbrake_button: u8,
    /// 12-byte G29 raw report buffer (separate from the 15-byte
    /// custom buffer — the sizes must never be confused).
    raw_report_g29: [u8; G29_RAW_REPORT_SIZE],
}

impl HidMaestroOutput {
    /// Create the backend with the default config. Nothing is spawned
    /// yet — the sidecar starts on the first
    /// [`try_connect`](IJoystickOutput::try_connect).
    #[must_use]
    pub fn new() -> Self {
        Self::with_config(HidMaestroConfig::default())
    }

    /// Create the backend with an explicit config (sidecar path,
    /// raw-report toggle).
    #[must_use]
    pub fn with_config(config: HidMaestroConfig) -> Self {
        let manager = SidecarManager::new(config.sidecar.clone());
        let g29_spoof = config.g29_spoof;
        let g29_pedal_map = config.g29_pedal_map;
        let g29_handbrake_button = config.g29_handbrake_button;
        Self {
            config,
            manager,
            connected: false,
            caps_known: false,
            capabilities: HidMaestroCapabilities::unknown(),
            status: HidMaestroStatus::idle(),
            last_error: None,
            raw_report: [0u8; CUSTOM_RAW_REPORT_SIZE],
            g29_spoof,
            g29_pedal_map,
            g29_handbrake_button,
            raw_report_g29: [0u8; G29_RAW_REPORT_SIZE],
        }
    }

    /// True when G29 spoof mode is on (active profile `logitech-g29`).
    #[must_use]
    pub const fn g29_spoof(&self) -> bool {
        self.g29_spoof
    }

    /// G29 pedal map in effect (which descriptor axes carry the
    /// pedals — see [`G29PedalMap`]).
    #[must_use]
    pub const fn g29_pedal_map(&self) -> G29PedalMap {
        self.g29_pedal_map
    }

    /// G29 handbrake button in effect (1–25).
    #[must_use]
    pub const fn g29_handbrake_button(&self) -> u8 {
        self.g29_handbrake_button
    }

    /// Profile id this backend will create on connect
    /// (`rsh-wheel-custom`, or `logitech-g29` when spoofing).
    #[must_use]
    pub const fn active_profile_id(&self) -> &'static str {
        if self.g29_spoof {
            G29_PROFILE_ID
        } else {
            PROFILE_ID
        }
    }

    /// The Output-page snapshot: state + message + diagnostics.
    /// Updated by every connect attempt, submit, and disconnect.
    #[must_use]
    pub fn status(&self) -> HidMaestroStatus {
        self.status.clone()
    }

    /// The last backend error (rich form — the trait error is a lossy
    /// vJoy-flavoured mapping; see `error.rs`).
    #[must_use]
    pub fn last_error(&self) -> Option<HidMaestroError> {
        self.last_error.clone()
    }

    /// Capabilities read at connect (unknown before the first
    /// successful connect).
    #[must_use]
    pub const fn capabilities(&self) -> HidMaestroCapabilities {
        self.capabilities
    }

    /// True once the one retry was used up — the selector (G4) must
    /// fall back to vJoy until the user re-selects this backend.
    #[must_use]
    pub fn fallback_needed(&self) -> bool {
        self.manager.fallback_needed()
    }

    /// Clear the fallback latch (user explicitly re-selected the
    /// backend in Output settings — G4).
    pub fn reset_fallback(&mut self) {
        self.manager.reset_fallback();
        self.status = HidMaestroStatus::idle();
        self.last_error = None;
    }

    /// Whether this backend submits raw reports (`true`, default) or
    /// `SubmitState` frames (`false`).
    #[must_use]
    pub const fn use_raw_report(&self) -> bool {
        self.config.use_raw_report
    }

    /// Selector hook (G4): static backend info — name, default-ness,
    /// elevation requirement, fallback target. No sidecar is touched.
    #[must_use]
    pub const fn backend_info() -> BackendInfo {
        BackendInfo {
            name: "HIDMaestro",
            is_default: true,
            requires_elevation: true,
            fallback_backend: "vJoy",
            sdk_version: PINNED_VERSION,
            profile_id: PROFILE_ID,
            identity_key: IDENTITY_KEY,
            uac_note: UAC_NOTE,
        }
    }

    /// Connect, validating the device against the enabled modules of
    /// `plan` when one is given (D11). `None` skips validation.
    ///
    /// # Errors
    ///
    /// See [`HidMaestroError`]: sidecar missing / not elevated, driver
    /// missing or install failed, profile refused, device create
    /// failed, or a capabilities error (device relinquished in that
    /// case). The error is also recorded in
    /// [`status`](Self::status) / [`last_error`](Self::last_error).
    pub fn connect_with_plan(
        &mut self,
        plan: Option<&PipelinePlan>,
    ) -> Result<(), HidMaestroError> {
        if self.connected {
            return Ok(());
        }
        if let Err(err) = self.connect_inner(plan) {
            self.fail(&err);
            return Err(err);
        }
        Ok(())
    }

    fn connect_inner(&mut self, plan: Option<&PipelinePlan>) -> Result<(), HidMaestroError> {
        let _hello: HelloInfo = self.manager.connect()?;
        // HID P2-1 (polish round): production `connect` never called
        // `describe-profile` — the only callers were the stub sidecar
        // and tests. Verify the descriptor/size before the first
        // submit so a packing mismatch fails here, not in-game. The
        // profile described is the one this backend will create
        // (custom, or G29 when spoofing — the G29 check also pins
        // the descriptor hex to the catalog profile's 123 bytes).
        let profile_id = self.active_profile_id();
        self.manager.describe_profile(profile_id)?;
        self.manager.install_driver()?;
        let created: CreateInfo = self.manager.create(profile_id)?;
        self.capabilities = created.capabilities;
        self.caps_known = true;

        if let Some(plan) = plan {
            let errors =
                validate_capabilities_for_profile(plan, &self.capabilities, self.g29_spoof);
            if !errors.is_empty() {
                // Relinquish the device: a plan the device cannot serve
                // must not leave a live controller behind.
                self.manager.shutdown();
                return Err(HidMaestroError::CreateFailed(format!(
                    "HIDMaestro device cannot serve this plan: {}",
                    errors.join(" ")
                )));
            }
        }

        // Neutral first, so a game never sees a stale deflection.
        // P2-2: the FIRST neutral after create gets the longer
        // first-neutral bound (slow first driver I/O), not the
        // 200 ms submit bound — a slow first neutral used to fail
        // the whole switch after the device was created.
        self.send_first_neutral_inner()?;
        self.connected = true;
        self.last_error = None;
        self.status = HidMaestroStatus {
            kind: HidMaestroStateKind::Ready,
            message: format!(
                "HIDMaestro ready — {} (driver {}, transport {}, {}).",
                profile_id,
                created.driver_version.as_deref().unwrap_or("?"),
                created
                    .transport_index
                    .map_or("?".to_string(), |i| i.to_string()),
                // M3: the Output page shows which submit path is live.
                // G29 forces the raw path (the SubmitState layout
                // does not match the G29 profile) regardless of the
                // `use_raw_report` setting.
                if self.g29_spoof {
                    "raw-report submit (G29, buttons 1–25, handbrake→L2)"
                } else if self.config.use_raw_report {
                    "raw-report submit (buttons 1–32)"
                } else {
                    "SubmitState submit (buttons 1–25)"
                },
            ),
            sdk_version: self.manager.sdk_version(),
            driver_version: created.driver_version,
            transport_index: created.transport_index,
            last_submit_latency_us: None,
            submit_count: 0,
            elevation_required: true,
            uac_note: UAC_NOTE,
        };
        Ok(())
    }

    fn fail(&mut self, err: &HidMaestroError) {
        self.connected = false;
        // A failed backend must not leave a live sidecar holding a
        // created device: shut it down (best-effort, no-op when the
        // transport is already gone).
        self.manager.shutdown();
        self.caps_known = false;
        self.capabilities = HidMaestroCapabilities::unknown();
        self.last_error = Some(err.clone());
        let kind = match err {
            HidMaestroError::SidecarNotFound(_) => HidMaestroStateKind::SidecarMissing,
            HidMaestroError::NotAdmin => HidMaestroStateKind::NotAdmin,
            HidMaestroError::DriverNotInstalled(_) | HidMaestroError::DriverInstallFailed(_) => {
                HidMaestroStateKind::DriverMissing
            }
            HidMaestroError::ProfileRefused(_) | HidMaestroError::CreateFailed(_) => {
                HidMaestroStateKind::CreateFailed
            }
            HidMaestroError::SidecarDied(_)
            | HidMaestroError::ReplyTimeout(_)
            | HidMaestroError::HelloTimeout(_)
            | HidMaestroError::BadHello(_)
            | HidMaestroError::BadReply(_) => HidMaestroStateKind::SidecarFailed,
            HidMaestroError::ContextUnavailable(_) => HidMaestroStateKind::ContextUnavailable,
            // P2-5 (v2.0.12 scan): the retry-budget error is an
            // `Error`, NOT `FallbackToVJoy` — on the explicit path
            // no fallback happens (the selector errors and the slot
            // stays untouched), so the old mapping told the Output
            // page the backend had "switched to vJoy" when vJoy was
            // not active at all. The automatic path never shows
            // this status: its fallback installs vJoy and reports
            // vJoy `Ready`. The arm stays explicit even though its
            // body matches the wildcard — it pins this decision, so
            // a later wildcard change cannot silently re-label the
            // retry-budget error as a fallback again.
            #[allow(clippy::match_same_arms)]
            HidMaestroError::FallbackNeeded => HidMaestroStateKind::Error,
            _ => HidMaestroStateKind::Error,
        };
        self.status = HidMaestroStatus {
            kind,
            message: err.to_string(),
            sdk_version: self.manager.sdk_version(),
            driver_version: self.manager.driver_version(),
            transport_index: self.manager.transport_index(),
            last_submit_latency_us: self.manager.last_submit_latency_us(),
            submit_count: self.manager.submit_count(),
            elevation_required: true,
            uac_note: UAC_NOTE,
        };
    }

    fn send_neutral_inner(&mut self) -> Result<(), HidMaestroError> {
        // L2: Rust USES the sidecar's `neutral` command (with the raw
        // flag matching the active submit path) instead of
        // re-submitting a neutral frame — the sidecar owns the neutral
        // frame for its own submit path. G29 always uses the raw
        // neutral (12-byte G29 report), whatever `use_raw_report` is.
        self.manager
            .neutral(self.config.use_raw_report || self.g29_spoof)
    }

    /// First neutral after `create` (P2-2): the longer
    /// first-neutral bound, used only by the connect flow.
    fn send_first_neutral_inner(&mut self) -> Result<(), HidMaestroError> {
        self.manager.neutral_first(self.config.use_raw_report)
    }

    fn current_caps(&self) -> HidMaestroCapabilities {
        if self.caps_known {
            self.capabilities
        } else {
            HidMaestroCapabilities::unknown()
        }
    }

    fn refresh_status_after_submit(&mut self) {
        self.status.last_submit_latency_us = self.manager.last_submit_latency_us();
        self.status.submit_count = self.manager.submit_count();
        self.status.driver_version = self.manager.driver_version();
        self.status.transport_index = self.manager.transport_index();
    }
}

impl Default for HidMaestroOutput {
    // `Default` must equal `new()` (status idle, caps unknown); a
    // derived impl cannot express that, so the manual impl stays.
    #[allow(clippy::derivable_impls)]
    fn default() -> Self {
        Self::new()
    }
}

impl IJoystickOutput for HidMaestroOutput {
    fn name(&self) -> &'static str {
        "HIDMaestro"
    }

    /// P2 (v2.0.11): re-arm the manager IN PLACE — the explicit
    /// re-select path calls this on the installed backend instead of
    /// replacing it with a fresh factory build, so the manager (its
    /// retry budget, its decline latch) survives across attempts and
    /// only an explicit re-select clears it.
    fn reset_fallback(&mut self) {
        HidMaestroOutput::reset_fallback(self);
    }

    /// G29 spoof mode (v2.0.16): enabling / disabling disposes and
    /// recreates the controller with the right profile — the backend
    /// restart semantics, reusing the existing connect path
    /// (`disconnect` + `connect_with_plan(None)`), NOT a second
    /// restart mechanism. A disconnected backend just records the
    /// flag (the next connect creates the right profile); a
    /// connected backend is neutralled with the OLD profile first,
    /// then recreated with the new one. A failed recreate leaves the
    /// backend disconnected with the failure in `status` /
    /// `last_error` (the daemon rolls the persisted flag back on
    /// that outcome) — the flag itself still records the requested
    /// mode, so a later re-enable retries with the right profile.
    /// NOTE: the recreate is synchronous and can take as long as a
    /// backend switch (sidecar spawn + UAC dwell) — callers must
    /// run it outside the backend slot lock (see
    /// `AppStateCore::set_g29_spoof_mode`), never on the 200 Hz tick.
    fn set_g29_spoof_mode(&mut self, enabled: bool) {
        if enabled == self.g29_spoof {
            return;
        }
        let was_connected = self.connected;
        if was_connected {
            // Neutral + dispose under the OLD profile first.
            self.disconnect();
        }
        self.g29_spoof = enabled;
        self.config.g29_spoof = enabled;
        if was_connected && let Err(err) = self.connect_with_plan(None) {
            // `connect_with_plan` already recorded the failure in
            // `status` / `last_error` via `fail()`; nothing more to
            // do here — the daemon sees a disconnected backend and
            // rolls the persisted flag back.
            let _ = err;
        }
    }

    /// G29 pedal layout, live (v2.0.17): the map is consulted on
    /// the next tick — no restart. An unknown layout string keeps
    /// the current map (the settings layer canonicalizes unknowns
    /// back to the default before they get here).
    fn set_g29_pedal_layout(&mut self, layout: &str) {
        if let Some(map) = G29PedalMap::from_layout(layout) {
            self.g29_pedal_map = map;
            self.config.g29_pedal_map = map;
        }
    }

    /// G29 handbrake button, live (v2.0.17): consulted on the next
    /// tick — no restart. Out-of-range buttons are clamped to
    /// 1–25, the G29 profile's button range.
    fn set_g29_handbrake_button(&mut self, button: u8) {
        let button = button.clamp(1, 25);
        self.g29_handbrake_button = button;
        self.config.g29_handbrake_button = button;
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    fn try_connect(&mut self) -> Result<(), VJoyError> {
        self.connect_with_plan(None).map_err(VJoyError::from)
    }

    fn try_connect_with_plan(&mut self, plan: &PipelinePlan) -> Result<(), VJoyError> {
        self.connect_with_plan(Some(plan)).map_err(VJoyError::from)
    }

    fn disconnect(&mut self) {
        let was_connected = self.connected;
        self.connected = false;
        // Neutral only when the device was connected — sending neutral
        // to a dead device is pointless (and would resurrect the
        // sidecar conversation for nothing).
        if was_connected {
            let _ = self.send_neutral_inner();
        }
        self.manager.shutdown();
        self.caps_known = false;
        self.capabilities = HidMaestroCapabilities::unknown();
        self.status = HidMaestroStatus::idle();
    }

    fn update(&mut self, state: &ProcessedState) -> Result<(), VJoyError> {
        if !self.connected {
            return Err(VJoyError::NotConnected);
        }
        // Heartbeat first: a dead sidecar is noticed here, within ~1 s
        // of its death, instead of surfacing as a hung submit.
        if let Err(err) = self.manager.ensure_alive() {
            self.fail(&err);
            return Err(VJoyError::from(err));
        }
        // PERF P1-1 (polish round): the 200 Hz tick submits
        // async — enqueue only, no round-trip wait. A refused
        // submit surfaces on the next tick via the drained reply;
        // a dead sidecar is caught by `ensure_alive` above.
        // G29 FORCES the raw-report path: the 12-byte G29 report is
        // the only submit shape for that profile — the SubmitState
        // frame layout does not match it (and the sidecar refuses a
        // `submit` while a G29 controller is active, defence in
        // depth). `use_raw_report: false` is ignored under G29.
        let result = if self.g29_spoof {
            build_g29_raw_report(
                state,
                &self.current_caps(),
                self.g29_pedal_map,
                self.g29_handbrake_button,
                &mut self.raw_report_g29,
            );
            let report = self.raw_report_g29;
            self.manager.submit_raw_async(&report)
        } else if self.config.use_raw_report {
            build_raw_report(state, &self.current_caps(), &mut self.raw_report);
            let report = self.raw_report;
            self.manager.submit_raw_async(&report)
        } else {
            let frame = build_gamepad_frame(state, &self.current_caps());
            self.manager.submit_async(frame)
        };
        if let Err(err) = result {
            // The device is gone (or refused the frame): mark
            // disconnected so the next try_connect is the retry.
            // The engine sends neutral and keeps ticking.
            self.fail(&err);
            return Err(VJoyError::from(err));
        }
        self.refresh_status_after_submit();
        Ok(())
    }

    fn send_neutral(&mut self) -> Result<(), VJoyError> {
        if !self.connected {
            return Ok(());
        }
        // Best-effort by design (watchdog / shutdown path): a failed
        // neutral send is swallowed, never fatal.
        if self.send_neutral_inner().is_err() {
            self.connected = false;
            self.manager.shutdown();
        }
        Ok(())
    }
}

/// Static backend info for the selector (G4) — no sidecar touched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BackendInfo {
    /// Backend name for logs and the Output page.
    pub name: &'static str,
    /// True: the default backend (`HIDMaestro` primary, vJoy fallback).
    pub is_default: bool,
    /// True: the backend needs an elevated helper for the session.
    pub requires_elevation: bool,
    /// Fallback backend name.
    pub fallback_backend: &'static str,
    /// Pinned SDK version.
    pub sdk_version: &'static str,
    /// Custom profile id.
    pub profile_id: &'static str,
    /// Controller identity key.
    pub identity_key: &'static str,
    /// UAC copy for the Output page.
    pub uac_note: &'static str,
}

/// Re-export for callers that build frames directly.
pub type HidMaestroFrame = crate::GamepadFrame;

/// The processed state the engine hands to this backend (alias, same
/// type as [`OutputState`]).
pub type HidMaestroProcessedState = OutputState;
