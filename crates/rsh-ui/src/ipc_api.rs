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
//! F5 (v2.0.3): the slow values — firmware, address, diagnostics,
//! pairing, calibration, profiles, games, bindings, knobs, modules,
//! tunings, draft — are refreshed ~1 Hz by a second runtime task
//! ([`refresh_slow`]) into the same cache, so the UI's whole sync
//! suite reads cache only and `reconnect_to_live` fetches its seed
//! on a background job ([`LiveSeed`]). A successful write updates
//! its slot at once (or pulls the next slow cycle forward), so the
//! UI converges within ~100 ms instead of at the next 1 s tick.
//!
//! The connection subscribes to [`AppEvent`] pushes (`subscribe`)
//! at connect time; [`IpcApi::poll_events`] drains them for the
//! UI's frame loop.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

use rsh_api::connection::ConnectionState;
use rsh_api::{
    app_version, ApiTransport, AppApi, AppEvent, ButtonBinding, CalibrationInfo, ConnectionError,
    ConnectionInfo, DiagStats, DraftStatus, FirmwareCheck, FirmwareUpdateStatus, GameSummary,
    PairingInfo, PollState, ProfileSummary, SlowPollState, SnapshotInfo, TuningSettings, UserError,
    VjoyStatus,
};
use rsh_ipc::{IpcClient, IpcError};

/// Background-polled live values (B3): the poller task in
/// [`IpcApi::connect`] refreshes these ~20 Hz, so the UI thread
/// only ever reads the cache. `link_dead` is set by the poller when
/// a round fails and cleared on the next good round; fallible calls
/// fail fast on it instead of waiting out the 500 ms timeout.
///
/// F5 (v2.0.3): the slow slots below hold everything else the UI
/// re-reads — firmware / address / diagnostics / pairing /
/// calibration / profiles / games / bindings / knobs / modules /
/// tunings / draft — refreshed ~1 Hz by a second poller task on
/// the runtime (never the UI thread). The UI's sync suite
/// (`sync_pedals`, `sync_extras`, `sync_overview`,
/// `refresh_profiles`, `refresh_games`, `sync_diag`,
/// `reconnect_to_live`) therefore reads cache only: on a slow or
/// dying daemon the UI thread never waits out a 500 ms round-trip.
/// A slot is `None` until its first fetch — reads fall back exactly
/// as they did on an IPC error before (empty lists, `"Unknown"`,
/// neutral tuning), so a cold cache is indistinguishable from a
/// failed call. `generation` bumps at the end of every completed
/// slow cycle; the frame timer re-syncs the pages when it changes.
struct LiveCache {
    connection: Mutex<Option<ConnectionInfo>>,
    snapshot: Mutex<Option<SnapshotInfo>>,
    output: Mutex<Option<bool>>,
    link_dead: AtomicBool,
    /// DS-19: consecutive failed fast-poll rounds. The link trips
    /// only at [`LINK_DEAD_AFTER_FAILURES`] — one transient failure
    /// used to wedge every UI call (`call_async` fails fast on
    /// `link_dead`) until a poll round succeeded.
    consecutive_poll_failures: AtomicU32,
    /// DS-19: true while a long IPC call (e.g. `set_output_backend`,
    /// 75 s) is in flight. The fast poller freezes its failure
    /// counter while this is set: the long call holds the
    /// single-flight IPC lock, so polls behind it time out and
    /// would otherwise trip the link spuriously.
    long_call_in_flight: AtomicBool,
    version: Mutex<Option<String>>,
    firmware_version: Mutex<Option<String>>,
    board_address: Mutex<Option<String>>,
    diag_stats: Mutex<Option<DiagStats>>,
    pairing_status: Mutex<Option<PairingInfo>>,
    calibration: Mutex<Option<CalibrationInfo>>,
    profiles: Mutex<Option<Vec<ProfileSummary>>>,
    games: Mutex<Option<Vec<GameSummary>>>,
    bindings: Mutex<Option<Vec<ButtonBinding>>>,
    knobs: Mutex<Option<[[String; 3]; 2]>>,
    firmware_check: Mutex<Option<FirmwareCheck>>,
    firmware_update_status: Mutex<Option<FirmwareUpdateStatus>>,
    vjoy_status: Mutex<Option<VjoyStatus>>,
    auto_pause: Mutex<Option<bool>>,
    close_to_tray: Mutex<Option<bool>>,
    /// Last autostart flag (also the `autostart()` read slot).
    autostart: Mutex<Option<bool>>,
    /// Last output backend name (also the `output_backend()` slot).
    backend: Mutex<Option<String>>,
    /// Module on/off by index: 0 handbrake, 1 shifter, 2 horn, 3 led
    /// (the same order [`module_index`] and the UI's `module_states`
    /// use).
    modules: Mutex<[Option<bool>; 4]>,
    tunings: Mutex<HashMap<String, TuningSettings>>,
    draft: Mutex<Option<DraftStatus>>,
    /// True once a slow cycle has completed (the slots hold daemon
    /// values, not just their fallbacks).
    slow_filled: AtomicBool,
    /// Set by a successful write (or the UI) to pull the next slow
    /// cycle forward at once instead of waiting out the 1 s period.
    refresh_requested: AtomicBool,
    /// Write epoch (DS-29): bumped by every `request_slow_refresh`
    /// — a slow cycle that started before the epoch moved can carry
    /// pre-write values, so it must not bump `generation` (see
    /// `refresh_slow`). The poller re-runs at once while the flag
    /// above is set, so the next bump carries post-write values.
    refresh_epoch: AtomicU64,
    generation: AtomicU64,
}

impl Default for LiveCache {
    fn default() -> Self {
        Self {
            connection: Mutex::new(None),
            snapshot: Mutex::new(None),
            output: Mutex::new(None),
            link_dead: AtomicBool::new(false),
            consecutive_poll_failures: AtomicU32::new(0),
            long_call_in_flight: AtomicBool::new(false),
            version: Mutex::new(None),
            firmware_version: Mutex::new(None),
            board_address: Mutex::new(None),
            diag_stats: Mutex::new(None),
            pairing_status: Mutex::new(None),
            calibration: Mutex::new(None),
            profiles: Mutex::new(None),
            games: Mutex::new(None),
            bindings: Mutex::new(None),
            knobs: Mutex::new(None),
            firmware_check: Mutex::new(None),
            firmware_update_status: Mutex::new(None),
            vjoy_status: Mutex::new(None),
            auto_pause: Mutex::new(None),
            close_to_tray: Mutex::new(None),
            autostart: Mutex::new(None),
            backend: Mutex::new(None),
            modules: Mutex::new([None; 4]),
            tunings: Mutex::new(HashMap::new()),
            draft: Mutex::new(None),
            slow_filled: AtomicBool::new(false),
            refresh_requested: AtomicBool::new(false),
            refresh_epoch: AtomicU64::new(0),
            generation: AtomicU64::new(0),
        }
    }
}

/// Module on/off slot index for [`LiveCache::modules`]:
/// handbrake 0, shifter 1, horn 2, led 3. `None` for a tuning-only
/// module (steering / throttle / brake / clutch have no toggle).
fn module_index(module: &str) -> Option<usize> {
    match module {
        "handbrake" => Some(0),
        "shifter" => Some(1),
        "horn" => Some(2),
        "led" => Some(3),
        _ => None,
    }
}

/// DS-19: consecutive failed fast-poll rounds after which the link
/// counts as dead. One transient failure (a slow daemon, a hiccup)
/// used to trip `link_dead` at once and wedge every UI call until
/// a poll round succeeded.
const LINK_DEAD_AFTER_FAILURES: u32 = 3;

/// DS-19: how the fast poller updates `link_dead` after one round.
/// `failures` is the consecutive-failure count BEFORE this round.
/// Returns `(link_dead, failures_after)`: a success clears the
/// streak and the link; a failure counts up and trips the link at
/// [`LINK_DEAD_AFTER_FAILURES`]; while a long call is in flight the
/// counter freezes — the polls behind it time out on the
/// single-flight IPC lock, which is not evidence of a dead link.
/// (Pure, so the thresholds are unit-testable without a runtime.)
fn link_dead_after_round(ok: bool, failures: u32, long_call_in_flight: bool) -> (bool, u32) {
    if ok {
        (false, 0)
    } else if long_call_in_flight {
        (false, failures)
    } else {
        let failures = failures.saturating_add(1);
        (failures >= LINK_DEAD_AFTER_FAILURES, failures)
    }
}

/// DS-19: clears `LiveCache::long_call_in_flight` on drop, so a
/// cancelled or panicking long call cannot freeze the poller's
/// failure counter forever.
struct LongCallGuard<'a> {
    cache: &'a LiveCache,
}

impl Drop for LongCallGuard<'_> {
    fn drop(&mut self) {
        self.cache
            .long_call_in_flight
            .store(false, Ordering::Relaxed);
    }
}

/// Everything `reconnect_to_live` needs (F5): the whole
/// reconnect-time fetch, done on a background job thread and handed
/// to the UI thread as plain owned data — `Send`, no `Rc`, no
/// window handle. [`LiveSeed::fetch`] connects first, then runs one
/// synchronous slow refresh (which fills the [`LiveCache`] as a
/// side effect), then reads the seed out of the cache — so the
/// UI-thread apply step is cache reads only.
///
/// The seed carries only the verdicts the apply step cannot read
/// from the cache itself: the session `connect` result (a live
/// call, not a cached value) and the switches the wizard seeds
/// from. Everything else (tunings, bindings, profiles, …) the
/// apply reads straight from the filled cache.
pub struct LiveSeed {
    /// Active output backend name.
    pub backend: String,
    /// Start-with-Windows flag.
    pub autostart: bool,
    /// The session `connect` verdict (the wizard's Connect step
    /// follows it; a failure notifies instead of blocking).
    pub connect: Result<ConnectionInfo, UserError>,
    /// Pairing window state (wizard Pair step).
    pub pairing: PairingInfo,
    /// Whether the `connect` verdict is an Active session.
    pub connection_active: bool,
}

impl LiveSeed {
    /// Fetch the whole seed on the calling (job) thread: connect
    /// on the transport the last session used, run one synchronous
    /// slow refresh (filling the live cache), then read the seed
    /// out of the cache. Every step is bounded by the 500 ms IPC
    /// bound / the poller's fail-fast, so a dying daemon finishes
    /// fast with defaults instead of hanging the fetch.
    pub fn fetch(api: &IpcApi, transport: ApiTransport) -> Self {
        let connect = api.connect(transport);
        api.refresh_slow_blocking();
        let connection_active = connect
            .as_ref()
            .is_ok_and(|info| matches!(info.state, ConnectionState::Active));
        Self {
            backend: api.output_backend(),
            autostart: api.autostart(),
            connect,
            pairing: api.pairing_status(),
            connection_active,
        }
    }
}

/// Live daemon API over IPC.
pub struct IpcApi {
    client: Arc<IpcClient>,
    rt: tokio::runtime::Runtime,
    cache: Arc<LiveCache>,
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
        });
        {
            // DS-80: the fast poller gets its OWN connection, so a
            // slow call on the main client (a `slow_poll`, a job
            // call) never delays the 20 Hz `poll` behind the
            // single-flight lock. Verified with a real `ping`
            // before use: a server that accepts only one
            // connection (the ipc_api test server) leaves the
            // second connect unanswered, so the poller falls back
            // to the shared client instead of stalling. The probe
            // is bounded to 200 ms — a healthy daemon answers in
            // under a millisecond.
            let poll_client = api.rt.block_on(async {
                let candidate =
                    tokio::time::timeout(Duration::from_secs(2), IpcClient::connect(path))
                        .await
                        .ok()
                        .and_then(Result::ok);
                match candidate {
                    Some(candidate) => {
                        let pinged =
                            tokio::time::timeout(Duration::from_millis(200), candidate.ping())
                                .await;
                        match pinged {
                            Ok(Ok(true)) => candidate,
                            _ => client.clone(),
                        }
                    }
                    None => client.clone(),
                }
            });
            let poll_cache = cache.clone();
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
                    // DS-19: the link trips only after N
                    // consecutive failed rounds (and never while a
                    // long call is in flight — see
                    // `link_dead_after_round`).
                    let (dead, failures) = link_dead_after_round(
                        ok,
                        poll_cache.consecutive_poll_failures.load(Ordering::Relaxed),
                        poll_cache.long_call_in_flight.load(Ordering::Relaxed),
                    );
                    poll_cache.link_dead.store(dead, Ordering::Relaxed);
                    poll_cache
                        .consecutive_poll_failures
                        .store(failures, Ordering::Relaxed);
                    // A9/B7: a failed round backs off to 500 ms — a
                    // wedged daemon must not let the poller hog the
                    // single-flight IPC lock (and the CPU) at 20 Hz,
                    // starving the UI thread's own calls.
                    let delay_ms = if ok { 50 } else { 500 };
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                }
            });
        }
        {
            // F5 (v2.0.3): the slow poller refreshes every other
            // read-only value the UI shows (~1 Hz, one sequential
            // pass over the read methods — each bounded to 500 ms
            // and fail-fast once the fast poller proves the link
            // dead, so a dying daemon costs one cheap skipped cycle
            // instead of a ~15 s stall). A first pass runs at once,
            // so the cache is warm ~50 ms after connect on a healthy
            // daemon; later passes wait out the 1 s period in 50 ms
            // steps and run early when a write asked for a refresh.
            let slow_client = client;
            let slow_cache = cache;
            api.rt.spawn(async move {
                refresh_slow(&slow_client, &slow_cache).await;
                loop {
                    for _ in 0..20 {
                        if slow_cache.refresh_requested.swap(false, Ordering::Relaxed) {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                    refresh_slow(&slow_client, &slow_cache).await;
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
        // DS-19: exempt the poller's failure counter while the long
        // call holds the single-flight IPC lock (its polls behind it
        // time out, which is not a dead link). The guard clears the
        // flag on every exit path, including cancellation/panic.
        self.cache
            .long_call_in_flight
            .store(true, Ordering::Relaxed);
        let _guard = LongCallGuard { cache: &self.cache };
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
    /// until its `IpcApi` is dropped with the runtime. F5: the slow
    /// slots go too — a dead daemon's firmware / profiles / tunings
    /// must not survive as if they were live.
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
        if let Ok(mut slot) = self.cache.version.lock() {
            *slot = None;
        }
        if let Ok(mut slot) = self.cache.firmware_version.lock() {
            *slot = None;
        }
        if let Ok(mut slot) = self.cache.board_address.lock() {
            *slot = None;
        }
        if let Ok(mut slot) = self.cache.diag_stats.lock() {
            *slot = None;
        }
        if let Ok(mut slot) = self.cache.pairing_status.lock() {
            *slot = None;
        }
        if let Ok(mut slot) = self.cache.calibration.lock() {
            *slot = None;
        }
        if let Ok(mut slot) = self.cache.profiles.lock() {
            *slot = None;
        }
        if let Ok(mut slot) = self.cache.games.lock() {
            *slot = None;
        }
        if let Ok(mut slot) = self.cache.bindings.lock() {
            *slot = None;
        }
        if let Ok(mut slot) = self.cache.knobs.lock() {
            *slot = None;
        }
        if let Ok(mut slot) = self.cache.firmware_check.lock() {
            *slot = None;
        }
        if let Ok(mut slot) = self.cache.firmware_update_status.lock() {
            *slot = None;
        }
        if let Ok(mut slot) = self.cache.vjoy_status.lock() {
            *slot = None;
        }
        if let Ok(mut slot) = self.cache.auto_pause.lock() {
            *slot = None;
        }
        if let Ok(mut slot) = self.cache.close_to_tray.lock() {
            *slot = None;
        }
        if let Ok(mut slot) = self.cache.autostart.lock() {
            *slot = None;
        }
        if let Ok(mut slot) = self.cache.backend.lock() {
            *slot = None;
        }
        if let Ok(mut slot) = self.cache.modules.lock() {
            *slot = [None; 4];
        }
        if let Ok(mut slot) = self.cache.tunings.lock() {
            slot.clear();
        }
        if let Ok(mut slot) = self.cache.draft.lock() {
            *slot = None;
        }
        self.cache.slow_filled.store(false, Ordering::Relaxed);
        self.cache.link_dead.store(true, Ordering::Relaxed);
    }

    /// True once a slow cycle has completed (F5): the slow slots
    /// hold daemon values, not just their cold-cache fallbacks.
    /// Tests wait on this instead of sleeping a fixed delay.
    /// (Diagnostic accessor — production code keys off
    /// `slow_generation` instead, so this reads as dead outside
    /// tests.)
    #[allow(dead_code)]
    #[must_use]
    pub fn slow_filled(&self) -> bool {
        self.cache.slow_filled.load(Ordering::Relaxed)
    }

    /// Current slow-cache generation (F5): bumps at the end of
    /// every completed slow cycle. The frame timer re-syncs the
    /// pages when it changes — that is how a daemon-side change
    /// reaches the UI without any UI-thread IPC.
    #[must_use]
    pub fn slow_generation(&self) -> u64 {
        self.cache.generation.load(Ordering::Relaxed)
    }

    /// Pull the next slow cycle forward at once (F5/F6): a
    /// successful write calls this so the cache — and the UI reading
    /// it — converges within ~100 ms instead of at the next 1 s
    /// tick. Cheap (one atomic store); the poller clears the flag
    /// when it starts the early cycle.
    pub fn request_slow_refresh(&self) {
        self.cache.refresh_requested.store(true, Ordering::Relaxed);
        // DS-29: a write is in flight — a slow cycle that started
        // before this must not bump the generation with pre-write
        // values (see `refresh_slow`).
        self.cache.refresh_epoch.fetch_add(1, Ordering::Relaxed);
    }

    /// One slow refresh, run synchronously on the calling (job)
    /// thread (F5): the reconnect seed job calls this after
    /// `connect`, so the seed is read out of a just-filled cache.
    /// Blocking here is fine — the caller is already a background
    /// thread, never the UI thread.
    pub fn refresh_slow_blocking(&self) {
        self.rt.block_on(refresh_slow(&self.client, &self.cache));
    }

    /// Mark the cached draft dirty for `module` (F6): the daemon
    /// marks the draft dirty on `set_tuning` /
    /// `set_module_enabled`, so a successful write dirties the
    /// cached draft at once instead of waiting for the slow
    /// poller's next cycle — the UI's unsaved dot (read from
    /// `draft_status()`) never lags a slider push. A binding /
    /// knob write does NOT mark it (the daemon doesn't either).
    fn mark_draft_dirty(&self, module: &str) {
        if let Ok(mut slot) = self.cache.draft.lock() {
            match slot.as_mut() {
                Some(status) => {
                    status.dirty = true;
                    if !status.dirty_modules.contains(&module.to_string()) {
                        status.dirty_modules.push(module.to_string());
                        status.dirty_modules.sort();
                    }
                }
                None => {
                    *slot = Some(DraftStatus {
                        profile_id: String::new(),
                        profile_name: String::new(),
                        dirty: true,
                        dirty_modules: vec![module.to_string()],
                    });
                }
            }
        }
    }

    /// Drain all pushed [`AppEvent`]s waiting on this connection.
    #[must_use]
    pub fn poll_events(&self) -> Vec<AppEvent> {
        // DS-78: a plain sync `mpsc` drain — no `block_on` per event
        // (and none per empty frame either).
        let mut out = Vec::new();
        while let Some(ev) = self.client.try_recv_event() {
            out.push(ev);
        }
        out
    }

    fn ipc_failed() -> UserError {
        UserError::from_connection(ConnectionError::TransportClosed, "daemon IPC link failed")
    }

    /// Build the [`UserError`] for a failed session `connect`
    /// (F26): the daemon's `IpcError::Api` payload carries its
    /// `UserError` code first (`"{code}: {message}"` — see
    /// `IpcClient::call_once`), so a no-board `Unreachable` comes
    /// back typed instead of collapsing into `TransportClosed`
    /// (which used to notify "Reconnect failed" on every no-board
    /// (re)connect). The code is the text before the first `:` —
    /// the real server sends `"{code}: daemon API error: {code}"`
    /// and the fake daemon an empty message (`"{code}: "`), and
    /// both must parse. An unrecognised payload is still the
    /// generic link failure — only the daemon's own typed reason
    /// is preserved, nothing is invented.
    fn daemon_connect_error(payload: &str) -> UserError {
        let code = payload.split(':').next().unwrap_or(payload).trim();
        match ConnectionError::from_code(code) {
            Some(error) => UserError::from_connection(error, payload.to_string()),
            None => Self::ipc_failed(),
        }
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

/// One slow refresh pass (F5, v2.0.3): every read-only value the
/// UI shows that the fast poller does not cover, fetched
/// sequentially into the cache. Each step checks `link_dead` first
/// and bails — a daemon that died mid-pass costs at most one more
/// 500 ms call, never the whole pass. A failed step keeps its last
/// good value (or the cold-cache fallback): a slow value going
/// briefly stale beats a frozen frame. A completed pass marks the
/// cache filled and bumps the generation, so the frame timer
/// re-syncs the pages from the fresh values.
#[allow(clippy::too_many_lines)]
async fn refresh_slow(client: &IpcClient, cache: &LiveCache) {
    if cache.link_dead.load(Ordering::Relaxed) {
        return;
    }
    // DS-29: the epoch this cycle started at — a write that lands
    // while the cycle runs makes its data possibly stale (the cycle
    // reads everything at once, up front), so the generation bump
    // at the end is skipped when the epoch moved (see below). The
    // poller re-runs at once while `refresh_requested` is set, so
    // the next bump carries post-write values.
    let epoch = cache.refresh_epoch.load(Ordering::Relaxed);
    // DS-29: one combined call first (30 -> 1 round-trips). On any
    // failure — old daemon without the method, an unparseable reply
    // — fall through to the sequential per-slot pass below, which
    // keeps exactly its old behaviour (including `link_dead` checks).
    let slow_state = match client.call("slow_poll", serde_json::json!({})).await {
        Ok(value) => serde_json::from_value::<SlowPollState>(value).ok(),
        Err(_) => None,
    };
    if let Some(state) = slow_state {
        macro_rules! set_slot {
            ($slot:ident, $value:expr) => {
                if let Ok(mut slot) = cache.$slot.lock() {
                    *slot = Some($value);
                }
            };
        }
        set_slot!(version, state.version);
        set_slot!(firmware_version, state.firmware_version);
        set_slot!(backend, state.backend);
        set_slot!(autostart, state.autostart);
        set_slot!(board_address, state.board_address);
        if let Some(diag) = state.diag_stats {
            set_slot!(diag_stats, diag);
        }
        set_slot!(pairing_status, state.pairing_status);
        if let Some(calibration) = state.calibration {
            set_slot!(calibration, calibration);
        }
        set_slot!(profiles, state.profiles);
        set_slot!(games, state.games);
        if let Some(check) = state.firmware_check {
            set_slot!(firmware_check, check);
        }
        set_slot!(firmware_update_status, state.firmware_update_status);
        if let Some(vjoy) = state.vjoy_status {
            set_slot!(vjoy_status, vjoy);
        }
        set_slot!(auto_pause, state.auto_pause);
        set_slot!(close_to_tray, state.close_to_tray);
        set_slot!(bindings, state.bindings);
        set_slot!(knobs, state.knobs);
        // Draft is `None`-or-object: store whichever parsed, so a
        // saved draft clears the cached one (as the old path did).
        if let Ok(mut slot) = cache.draft.lock() {
            *slot = state.draft;
        }
        if let Ok(mut tunings) = cache.tunings.lock() {
            for (module, tuning) in state.tunings {
                tunings.insert(module, tuning);
            }
        }
        if let Ok(mut modules) = cache.modules.lock() {
            for (module, index) in [("handbrake", 0), ("shifter", 1), ("horn", 2), ("led", 3)] {
                if let Some(enabled) = state.modules.get(module) {
                    modules[index] = Some(*enabled);
                }
            }
        }
        cache.slow_filled.store(true, Ordering::Relaxed);
        bump_generation_if_fresh(cache, epoch);
        return;
    }
    // One call, parsed or ignored. `parse` turns the reply into
    // the slot's value (`None` = unparseable, keep the old value).
    macro_rules! fetch {
        ($method:literal, $slot:ident, $parse:expr) => {{
            if cache.link_dead.load(Ordering::Relaxed) {
                return;
            }
            if let Ok(value) = client.call($method, serde_json::json!({})).await {
                let parse: fn(&serde_json::Value) -> Option<_> = $parse;
                if let Some(parsed) = parse(&value) {
                    if let Ok(mut slot) = cache.$slot.lock() {
                        *slot = Some(parsed);
                    }
                }
            }
        }};
    }
    fetch!("version", version, |v: &serde_json::Value| {
        v.get("version")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    });
    fetch!("firmware_version", firmware_version, |v| {
        v.get("firmware_version")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    });
    fetch!("output_backend", backend, |v| {
        v.get("output_backend")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    });
    fetch!("autostart", autostart, |v| {
        v.get("autostart").and_then(serde_json::Value::as_bool)
    });
    fetch!("board_address", board_address, |v| {
        v.get("address")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    });
    fetch!("diag_stats", diag_stats, |v| {
        serde_json::from_value(v.clone()).ok()
    });
    fetch!("pairing_status", pairing_status, |v| {
        serde_json::from_value(v.clone()).ok()
    });
    fetch!("calibration", calibration, |v| {
        serde_json::from_value(v.clone()).ok()
    });
    fetch!("list_profiles", profiles, |v| {
        serde_json::from_value(v.clone()).ok()
    });
    fetch!("list_games", games, |v| {
        serde_json::from_value(v.clone()).ok()
    });
    fetch!("firmware_check", firmware_check, |v| {
        serde_json::from_value(v.clone()).ok()
    });
    fetch!("firmware_update_status", firmware_update_status, |v| {
        serde_json::from_value(v.clone()).ok()
    });
    fetch!("vjoy_status", vjoy_status, |v| {
        serde_json::from_value(v.clone()).ok()
    });
    fetch!("auto_pause", auto_pause, |v| {
        v.get("paused").and_then(serde_json::Value::as_bool)
    });
    fetch!("close_to_tray", close_to_tray, |v| {
        v.get("close_to_tray").and_then(serde_json::Value::as_bool)
    });
    // `list_bindings` / `knob_actions` nest their payloads under a
    // key, like their trait methods already parse them.
    fetch!("list_bindings", bindings, |v| {
        v.get("bindings")
            .cloned()
            .and_then(|b| serde_json::from_value(b).ok())
    });
    fetch!("knob_actions", knobs, |v| {
        v.get("knobs")
            .cloned()
            .and_then(|k| serde_json::from_value(k).ok())
    });
    // The draft reply is `null` (no draft) or an object: store
    // whichever parsed, so a saved draft clears the cached one.
    if !cache.link_dead.load(Ordering::Relaxed) {
        if let Ok(value) = client.call("draft_status", serde_json::json!({})).await {
            if let Ok(draft) = serde_json::from_value::<Option<DraftStatus>>(value) {
                if let Ok(mut slot) = cache.draft.lock() {
                    *slot = draft;
                }
            }
        }
    }
    // Per-module reads: tunings for every module, on/off for the
    // four toggleable ones.
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
        if cache.link_dead.load(Ordering::Relaxed) {
            return;
        }
        if let Ok(value) = client
            .call("get_tuning", serde_json::json!({"module": module}))
            .await
        {
            if let Ok(tuning) = serde_json::from_value::<TuningSettings>(value) {
                if let Ok(mut tunings) = cache.tunings.lock() {
                    tunings.insert(module.to_string(), tuning);
                }
            }
        }
    }
    for (module, index) in [("handbrake", 0), ("shifter", 1), ("horn", 2), ("led", 3)] {
        if cache.link_dead.load(Ordering::Relaxed) {
            return;
        }
        if let Ok(value) = client
            .call("module_enabled", serde_json::json!({"module": module}))
            .await
        {
            if let Some(enabled) = value.get("enabled").and_then(serde_json::Value::as_bool) {
                if let Ok(mut modules) = cache.modules.lock() {
                    modules[index] = Some(enabled);
                }
            }
        }
    }
    cache.slow_filled.store(true, Ordering::Relaxed);
    bump_generation_if_fresh(cache, epoch);
}

/// Bump the slow generation when the cycle's data is fresh
/// (DS-29): skip the bump when a write asked for a refresh while
/// this cycle ran — the cycle may carry pre-write values, and its
/// bump would tell the UI (and the `discard_draft` test) the cache
/// is current when it is not. `slow_filled` is NOT gated: the slots
/// still hold genuine daemon values, only possibly a cycle old.
fn bump_generation_if_fresh(cache: &LiveCache, epoch: u64) {
    if cache.refresh_epoch.load(Ordering::Relaxed) == epoch {
        cache.generation.fetch_add(1, Ordering::Relaxed);
    }
}

impl AppApi for IpcApi {
    /// Cached version (F5): the slow poller refreshes it ~1 Hz, so
    /// this never touches the socket. No cache yet means the local
    /// app version (the pre-poller fallback).
    fn version(&self) -> String {
        self.cache
            .version
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
            .unwrap_or_else(|| app_version().to_string())
    }

    /// Slow-cache generation (F5): the frame timer re-syncs the
    /// pages when this changes. `0` for implementations without a
    /// slow cache (the trait default) — they never trigger a resync.
    fn cache_generation(&self) -> u64 {
        self.slow_generation()
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
        match got {
            // F5: a (dis)connect changes the session — pull the
            // next slow cycle forward so pairing / firmware follow.
            Ok(info) => {
                self.request_slow_refresh();
                Ok(info)
            }
            // F26: keep the daemon's typed reason (a no-board
            // `Unreachable` must not become `TransportClosed`).
            Err(IpcError::Api(payload)) => Err(Self::daemon_connect_error(&payload)),
            Err(_) => Err(Self::ipc_failed()),
        }
    }

    fn disconnect(&self) -> Result<ConnectionInfo, UserError> {
        let got: Result<ConnectionInfo, IpcError> = self.rt.block_on(async {
            let value = self.call_async("disconnect", serde_json::json!({})).await?;
            serde_json::from_value(value).map_err(|e| IpcError::Protocol(e.to_string()))
        });
        match got {
            Ok(info) => {
                self.request_slow_refresh();
                Ok(info)
            }
            Err(_) => Err(Self::ipc_failed()),
        }
    }

    /// Cached pairing status (F5): the slow poller refreshes it
    /// ~1 Hz, so this never touches the socket. No cache yet means
    /// no window (the pre-poller fallback).
    fn pairing_status(&self) -> PairingInfo {
        self.cache
            .pairing_status
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
            .unwrap_or(PairingInfo {
                seconds_left: 0,
                password: None,
                error: None,
                error_detail: None,
                board_mac: None,
                replacement_required: false,
                key_reset_armed: false,
                attempt_log: Vec::new(),
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
                attempt_log: Vec::new(),
            }))
        });
        match got {
            // F5: pairing started — pull the next slow cycle forward
            // so the wizard's countdown follows the real window.
            Ok(info) => {
                self.request_slow_refresh();
                Ok(info)
            }
            Err(_) => Err(Self::ipc_failed()),
        }
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

    /// Cached tuning record (F5): the slow poller refreshes every
    /// module ~1 Hz, so this never touches the socket. No cache yet
    /// means the neutral record (the pre-poller fallback).
    fn get_tuning(&self, module: &str) -> TuningSettings {
        self.cache
            .tunings
            .lock()
            .ok()
            .and_then(|tunings| tunings.get(module).cloned())
            .unwrap_or_else(|| TuningSettings::neutral(module))
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
        match got {
            Ok(stored) => {
                // F5/F6: the stored record is the cache now, the
                // draft is dirty at once (the daemon marks it), and
                // the poller re-fetches the rest promptly — the next
                // sync reads the new values, not the pre-push ones.
                if let Ok(mut tunings) = self.cache.tunings.lock() {
                    tunings.insert(stored.module.clone(), stored.clone());
                }
                self.mark_draft_dirty(&stored.module);
                self.request_slow_refresh();
                Ok(stored)
            }
            Err(_) => Err(Self::ipc_failed()),
        }
    }

    /// Cached draft status (F5): the slow poller refreshes it ~1 Hz
    /// (and a successful `set_tuning` / `set_module_enabled` marks
    /// it dirty at once), so this never touches the socket. No
    /// cache yet means no draft (the pre-poller fallback).
    fn draft_status(&self) -> Option<DraftStatus> {
        self.cache.draft.lock().ok().and_then(|slot| slot.clone())
    }

    fn save_draft(&self) -> Result<bool, UserError> {
        let got: Result<bool, IpcError> = self.rt.block_on(async {
            let value = self.call_async("save_draft", serde_json::json!({})).await?;
            value
                .get("saved")
                .and_then(serde_json::Value::as_bool)
                .ok_or_else(|| IpcError::Protocol("save_draft has no saved".to_string()))
        });
        match got {
            Ok(saved) => {
                // F5: a saved draft is gone — clear the cached one
                // at once instead of waiting for the poller.
                if let Ok(mut slot) = self.cache.draft.lock() {
                    *slot = None;
                }
                Ok(saved)
            }
            Err(_) => Err(Self::ipc_failed()),
        }
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
        let discarded = got.unwrap_or(false);
        if discarded {
            // F5: a discarded draft is gone — clear the cached one.
            if let Ok(mut slot) = self.cache.draft.lock() {
                *slot = None;
            }
            // A discard also reverts the stored tunings — pull the
            // next slow cycle forward so the cache follows.
            self.request_slow_refresh();
        }
        discarded
    }

    /// Cached autostart flag (F5): the slow poller refreshes it
    /// ~1 Hz, so this never touches the socket. No cache yet means
    /// off (the pre-poller fallback).
    fn autostart(&self) -> bool {
        self.cache
            .autostart
            .lock()
            .ok()
            .and_then(|slot| *slot)
            .unwrap_or(false)
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
                if let Ok(mut slot) = self.cache.autostart.lock() {
                    *slot = Some(new_state);
                }
                Ok(new_state)
            }
            Err(_) => Err(Self::ipc_failed()),
        }
    }

    /// Cached firmware version (F5): the slow poller refreshes it
    /// ~1 Hz, so this never touches the socket. No cache yet means
    /// `"Unknown"` (the pre-poller fallback).
    fn firmware_version(&self) -> String {
        self.cache
            .firmware_version
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
            .unwrap_or_else(|| "Unknown".to_string())
    }

    /// Cached output backend (F5): the slow poller refreshes it
    /// ~1 Hz (and a successful switch stores it at once), so this
    /// never touches the socket. No cache yet means `"Unknown"` (the
    /// pre-poller fallback).
    fn output_backend(&self) -> String {
        self.cache
            .backend
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
            .unwrap_or_else(|| "Unknown".to_string())
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
                if let Ok(mut slot) = self.cache.backend.lock() {
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
    /// Cached profiles list (F5): the slow poller refreshes it
    /// ~1 Hz, so this never touches the socket. No cache yet means
    /// an empty list — never invented profiles.
    fn list_profiles(&self) -> Vec<ProfileSummary> {
        self.cache
            .profiles
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
            .unwrap_or_default()
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
        match got {
            Ok(deleted) => {
                if deleted {
                    if let Ok(mut slot) = self.cache.profiles.lock() {
                        if let Some(profiles) = slot.as_mut() {
                            profiles.retain(|p| p.id != id);
                        }
                    }
                }
                self.request_slow_refresh();
                Ok(deleted)
            }
            Err(_) => Err(Self::ipc_failed()),
        }
    }

    /// Cached games list (F5): the slow poller refreshes it ~1 Hz,
    /// so this never touches the socket. No cache yet means an empty
    /// list — never invented games.
    fn list_games(&self) -> Vec<GameSummary> {
        self.cache
            .games
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
            .unwrap_or_default()
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

    /// Add a manually added game (name + exe/shortcut path,
    /// optional bound profile id); returns the new entry. A
    /// `None` / empty profile id means no profile bound.
    fn add_game(
        &self,
        name: &str,
        exe_path: &str,
        profile_id: Option<String>,
    ) -> Result<GameSummary, UserError> {
        self.game_write_call("add_game", None, name, exe_path, profile_id)
    }

    /// Update a manually added game's name / exe path / bound
    /// profile; returns the updated entry.
    fn update_game(
        &self,
        id: &str,
        name: &str,
        exe_path: &str,
        profile_id: Option<String>,
    ) -> Result<GameSummary, UserError> {
        self.game_write_call("update_game", Some(id), name, exe_path, profile_id)
    }

    /// Remove a manually added game; returns `true` when removed.
    fn remove_game(&self, id: &str) -> Result<bool, UserError> {
        let got: Result<bool, IpcError> = self.rt.block_on(async {
            let value = self
                .call_async("remove_game", serde_json::json!({"id": id}))
                .await?;
            value
                .get("removed")
                .and_then(serde_json::Value::as_bool)
                .ok_or_else(|| IpcError::Protocol("remove_game has no removed".to_string()))
        });
        match got {
            Ok(removed) => {
                if removed {
                    if let Ok(mut slot) = self.cache.games.lock() {
                        if let Some(games) = slot.as_mut() {
                            games.retain(|g| g.id != id);
                        }
                    }
                }
                self.request_slow_refresh();
                Ok(removed)
            }
            Err(_) => Err(Self::ipc_failed()),
        }
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
    /// Cached calibration read (F5): the slow poller refreshes it
    /// ~1 Hz, so this never touches the socket. No cache yet (or
    /// never captured) is an error — the sim / no-daemon behaviour.
    fn calibration(&self) -> Result<CalibrationInfo, UserError> {
        self.cache
            .calibration
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
            .ok_or_else(Self::ipc_failed)
    }

    /// Cached vJoy status (F5): the slow poller refreshes it ~1 Hz,
    /// so this never touches the socket. No cache yet is an error —
    /// the status is honestly unknown, never a fake "installed".
    fn vjoy_status(&self) -> Result<VjoyStatus, UserError> {
        self.cache
            .vjoy_status
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
            .ok_or_else(Self::ipc_failed)
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
        match got {
            Ok(message) => {
                // F5: the install changes the driver state — pull
                // the next slow cycle forward so the Drivers step
                // converges promptly.
                self.request_slow_refresh();
                Ok(message)
            }
            Err(_) => Err(Self::ipc_failed()),
        }
    }

    /// Cached diagnostics (F5): the slow poller refreshes them ~1 Hz,
    /// so this never touches the socket. No cache yet is an error —
    /// the drawer shows "—" / "No telemetry yet".
    fn diag_stats(&self) -> Result<DiagStats, UserError> {
        self.cache
            .diag_stats
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
            .ok_or_else(Self::ipc_failed)
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
        match got {
            Ok(info) => {
                // F5: the reset changes the pairing state — pull
                // the next slow cycle forward.
                self.request_slow_refresh();
                Ok(info)
            }
            Err(e) => Err(Self::api_error(&e)),
        }
    }

    // R2-51: confirm a key replacement (WS-A server side) — NOT
    // fire-and-forget like `begin_pairing`: the daemon returns the
    // `PairingInfo` promptly, and the outcome arrives via
    // `pairing_status()` polling + `PairingFinished`. Same call
    // pattern as `reset_pairing` above.
    fn confirm_pairing_replacement(&self) -> Result<PairingInfo, UserError> {
        let got: Result<PairingInfo, IpcError> = self.rt.block_on(async {
            let value = self
                .call_async("confirm_pairing_replacement", serde_json::json!({}))
                .await?;
            serde_json::from_value(value).map_err(|e| IpcError::Protocol(e.to_string()))
        });
        match got {
            Ok(info) => {
                // F5: the confirm changes the pairing state — pull
                // the next slow cycle forward.
                self.request_slow_refresh();
                Ok(info)
            }
            Err(e) => Err(Self::api_error(&e)),
        }
    }

    /// Cached bindings list (F5): the slow poller refreshes it ~1 Hz,
    /// so this never touches the socket. No cache yet means an empty
    /// list (the display defaults stand).
    fn list_bindings(&self) -> Vec<ButtonBinding> {
        self.cache
            .bindings
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
            .unwrap_or_default()
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
        match got {
            Ok(binding) => {
                // F5: the stored binding is the cache now.
                if let Ok(mut slot) = self.cache.bindings.lock() {
                    if let Some(bindings) = slot.as_mut() {
                        if let Some(old) = bindings.iter_mut().find(|b| b.button == button) {
                            *old = binding.clone();
                        } else {
                            bindings.push(binding.clone());
                        }
                    }
                }
                self.request_slow_refresh();
                Ok(binding)
            }
            Err(e) => Err(Self::api_error(&e)),
        }
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
        match got {
            Ok(cleared) => {
                if cleared {
                    if let Ok(mut slot) = self.cache.bindings.lock() {
                        if let Some(bindings) = slot.as_mut() {
                            bindings.retain(|b| b.button != button);
                        }
                    }
                }
                self.request_slow_refresh();
                Ok(cleared)
            }
            Err(e) => Err(Self::api_error(&e)),
        }
    }

    /// Cached firmware check (F5): the slow poller refreshes it
    /// ~1 Hz, so this never touches the socket. No cache yet is an
    /// error — the Settings firmware line falls back to the live
    /// version alone.
    fn firmware_check(&self) -> Result<FirmwareCheck, UserError> {
        self.cache
            .firmware_check
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
            .ok_or_else(Self::ipc_failed)
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
        match got {
            Ok(status) => {
                // F5: the flash changes the update status — pull
                // the next slow cycle forward.
                self.request_slow_refresh();
                Ok(status)
            }
            Err(e) => Err(Self::api_error(&e)),
        }
    }

    /// Cached firmware-update status (F5): the slow poller
    /// refreshes it ~1 Hz, so the 1 Hz flash poll never touches
    /// the socket. No cache yet means the idle default.
    fn firmware_update_status(&self) -> FirmwareUpdateStatus {
        self.cache
            .firmware_update_status
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
            .unwrap_or_default()
    }

    /// Cached board address (F5): the slow poller refreshes it ~1 Hz,
    /// so this never touches the socket. No cache yet means the
    /// default board address (the pre-poller fallback).
    fn board_address(&self) -> String {
        self.cache
            .board_address
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
            .unwrap_or_else(|| "192.168.4.1".to_string())
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
        match got {
            Ok(stored) => {
                if let Ok(mut slot) = self.cache.board_address.lock() {
                    *slot = Some(stored.clone());
                }
                Ok(stored)
            }
            Err(e) => Err(Self::api_error(&e)),
        }
    }

    /// Cached module flag (F5): the slow poller refreshes it ~1 Hz,
    /// so this never touches the socket. No cache yet means enabled
    /// (the pre-poller fallback); unknown modules are enabled too.
    fn module_enabled(&self, module: &str) -> bool {
        module_index(module)
            .and_then(|index| {
                self.cache
                    .modules
                    .lock()
                    .ok()
                    .and_then(|modules| modules[index])
            })
            .unwrap_or(true)
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
                .ok_or_else(|| IpcError::Protocol("module_enabled has no enabled".to_string()))
        });
        match got {
            Ok(stored) => {
                if let Some(index) = module_index(module) {
                    if let Ok(mut modules) = self.cache.modules.lock() {
                        modules[index] = Some(stored);
                    }
                }
                // F5: the daemon marks the draft dirty on a module
                // toggle — the cached draft follows at once.
                self.mark_draft_dirty(module);
                self.request_slow_refresh();
                Ok(stored)
            }
            Err(e) => Err(Self::api_error(&e)),
        }
    }

    /// Cached auto-pause flag (F5): the slow poller refreshes it
    /// ~1 Hz, so this never touches the socket. No cache yet means
    /// not paused (the pre-poller fallback).
    fn auto_pause(&self) -> bool {
        self.cache
            .auto_pause
            .lock()
            .ok()
            .and_then(|slot| *slot)
            .unwrap_or(false)
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
        match got {
            Ok(stored) => {
                if let Ok(mut slot) = self.cache.auto_pause.lock() {
                    *slot = Some(stored);
                }
                Ok(stored)
            }
            Err(e) => Err(Self::api_error(&e)),
        }
    }

    /// Cached keep-in-tray flag (F5): the slow poller refreshes it
    /// ~1 Hz, so the close handler never touches the socket. No
    /// cache yet means keep in tray (the pre-poller fallback).
    fn close_to_tray(&self) -> bool {
        self.cache
            .close_to_tray
            .lock()
            .ok()
            .and_then(|slot| *slot)
            .unwrap_or(true)
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
        match got {
            Ok(stored) => {
                if let Ok(mut slot) = self.cache.close_to_tray.lock() {
                    *slot = Some(stored);
                }
                Ok(stored)
            }
            Err(e) => Err(Self::api_error(&e)),
        }
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

    /// Cached knob actions (F5): the slow poller refreshes them
    /// ~1 Hz, so this never touches the socket. No cache yet means
    /// the all-empty default (the pre-poller fallback).
    fn knob_actions(&self) -> [[String; 3]; 2] {
        self.cache
            .knobs
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
            .unwrap_or_default()
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
        match got {
            Ok(()) => {
                // F5: the stored action is the cache now.
                if let Ok(mut slot) = self.cache.knobs.lock() {
                    if let Some(knobs) = slot.as_mut() {
                        let k = usize::from(knob);
                        let r = usize::from(row);
                        if k < 2 && r < 3 {
                            knobs[k][r] = action.to_string();
                        }
                    }
                }
                self.request_slow_refresh();
                Ok(())
            }
            Err(e) => Err(Self::api_error(&e)),
        }
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
        match got {
            Ok(mv) => {
                // F5: a capture changes the calibration — pull the
                // next slow cycle forward so the mV labels converge.
                self.request_slow_refresh();
                Ok(mv)
            }
            Err(e) => Err(Self::capture_error(&e)),
        }
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
        match got {
            // F5: a rename / link changes the profiles list — pull
            // the next slow cycle forward.
            Ok(name) => {
                self.request_slow_refresh();
                Ok(name)
            }
            Err(e) => Err(Self::api_error(&e)),
        }
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
        match got {
            // F5: an activate / create / duplicate changes the
            // profiles list — pull the next slow cycle forward.
            Ok(name) => {
                self.request_slow_refresh();
                Ok(name)
            }
            Err(_) => Err(Self::ipc_failed()),
        }
    }

    /// Shared manual-game write call (`add_game` / `update_game`):
    /// name + exe path in, the stored [`GameSummary`] out. An
    /// empty profile id is sent as `null` (no profile bound), the
    /// same as an absent one. Success upserts the games cache at
    /// once (add appends, update replaces by id) and pulls the
    /// next slow cycle forward.
    fn game_write_call(
        &self,
        method: &str,
        id: Option<&str>,
        name: &str,
        exe_path: &str,
        profile_id: Option<String>,
    ) -> Result<GameSummary, UserError> {
        let profile_id = profile_id.filter(|s| !s.is_empty());
        let got: Result<GameSummary, IpcError> = self.rt.block_on(async {
            let value = self
                .call_async(
                    method,
                    serde_json::json!({
                        "id": id,
                        "name": name,
                        "exe_path": exe_path,
                        "profile_id": profile_id,
                    }),
                )
                .await?;
            serde_json::from_value(value).map_err(|e| IpcError::Protocol(e.to_string()))
        });
        match got {
            Ok(summary) => {
                if let Ok(mut slot) = self.cache.games.lock() {
                    if let Some(games) = slot.as_mut() {
                        if let Some(old) = games.iter_mut().find(|g| g.id == summary.id) {
                            *old = summary.clone();
                        } else {
                            games.push(summary.clone());
                        }
                    }
                }
                self.request_slow_refresh();
                Ok(summary)
            }
            Err(_) => Err(Self::ipc_failed()),
        }
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
        match got {
            Ok(summary) => {
                // F5: a launch / stop changes the games list — the
                // returned summary is the cache now.
                if let Ok(mut slot) = self.cache.games.lock() {
                    if let Some(games) = slot.as_mut() {
                        if let Some(old) = games.iter_mut().find(|g| g.id == summary.id) {
                            *old = summary.clone();
                        }
                    }
                }
                self.request_slow_refresh();
                Ok(summary)
            }
            Err(_) => Err(Self::ipc_failed()),
        }
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

    /// DS-19: the link trips only after N=3 consecutive failed poll
    /// rounds — a single transient failure must not wedge the UI —
    /// and the counter freezes while a long call is in flight.
    #[test]
    fn link_dead_needs_three_consecutive_failures() {
        // Success clears the streak and the link.
        assert_eq!(link_dead_after_round(true, 0, false), (false, 0));
        assert_eq!(link_dead_after_round(true, 2, false), (false, 0));
        // One or two failures: counted, but the link stays up.
        assert_eq!(link_dead_after_round(false, 0, false), (false, 1));
        assert_eq!(link_dead_after_round(false, 1, false), (false, 2));
        // The third consecutive failure trips it, and it stays tripped.
        assert_eq!(link_dead_after_round(false, 2, false), (true, 3));
        assert_eq!(link_dead_after_round(false, 3, false), (true, 4));
        // A later success clears it again.
        assert_eq!(link_dead_after_round(true, 4, false), (false, 0));
        // While a long call is in flight the counter freezes:
        // failures are not counted and the link never trips.
        assert_eq!(link_dead_after_round(false, 0, true), (false, 0));
        assert_eq!(link_dead_after_round(false, 2, true), (false, 2));
        // ... and a success during a long call still clears.
        assert_eq!(link_dead_after_round(true, 2, true), (false, 0));
    }
    // DS-07 (deep-scan FC-06/CI-01): the only use of PathBuf in this
    // test module is inside the #[cfg(unix)] fn live_client below, so
    // the import is unused on Windows and fails clippy-windows with
    // -D warnings. Gate it to unix.
    #[cfg(unix)]
    use std::path::PathBuf;
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
    #[allow(clippy::too_many_lines)]
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
        // F5: the slow values (autostart, backend, …) arrive on
        // the slow poller's first cycle — wait for it instead of
        // asserting cold-cache fallbacks.
        let mut waited = 0;
        while !client.slow_filled() && waited < 100 {
            std::thread::sleep(Duration::from_millis(50));
            waited += 1;
        }
        assert!(client.slow_filled(), "slow cache should fill");
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
        // Discard reverts to neutral — the daemon clears its
        // tunings, and the discard pulls a slow refresh forward,
        // so wait for the cache to follow instead of asserting a
        // stale pre-discard record.
        client.set_tuning(&upd).expect("edit again");
        let generation = client.slow_generation();
        assert!(client.discard_draft());
        let mut waited = 0;
        while client.slow_generation() == generation && waited < 100 {
            std::thread::sleep(Duration::from_millis(50));
            waited += 1;
        }
        assert_ne!(
            client.slow_generation(),
            generation,
            "slow cache should refresh after discard"
        );
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
        // Manual-game writes over the same socket: the sim has no
        // manual store, so every write honestly errors (no fake
        // entry) and the list stays empty. This still proves the
        // method names dispatch end to end — an unknown method
        // would fail the same way, so the success path is covered
        // only against the real daemon store, not here (open).
        assert!(client
            .add_game("Test Game", "C:\\Games\\test.exe", None)
            .is_err());
        assert!(client
            .update_game("x", "Test Game", "C:\\Games\\test.exe", None)
            .is_err());
        assert!(client.remove_game("x").is_err());
        assert_eq!(client.list_games().len(), 0);
        assert!(client.start_learn().is_err());
        assert!(client.cancel_learn().is_err());
        assert!(!client.is_learning());
        assert_eq!(client.take_learned_button(), None);
        drop(client);
        handle.join().expect("server thread");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Spin up a real Unix-socket `IpcServer` over `SimApi` (the
    /// daemon stand-in) and connect a real `IpcApi` to it (F5
    /// tests). Returns the client, the temp dir, the server thread,
    /// and the held pipe guard — drop the client, join the thread,
    /// then remove the dir. Plain `#[test]`: same runtime rules as
    /// `tuning_round_trip_over_real_socket`.
    #[cfg(unix)]
    fn live_client() -> (
        Arc<IpcApi>,
        PathBuf,
        std::thread::JoinHandle<()>,
        std::sync::MutexGuard<'static, ()>,
    ) {
        let guard = crate::live_probe::TEST_IPC_PIPE_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = std::env::temp_dir().join(format!("rsh-f5-test-{}", std::process::id()));
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
        (client, dir, handle, guard)
    }

    /// Wait up to ~5 s for a condition on the live client (F5
    /// tests): the pollers fill asynchronously, so tests wait
    /// instead of sleeping a fixed delay.
    #[cfg(unix)]
    fn wait_for(client: &IpcApi, mut done: impl FnMut(&IpcApi) -> bool) -> bool {
        for _ in 0..100 {
            if done(client) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        done(client)
    }

    /// F5: the slow poller fills every slow slot with the daemon's
    /// values (~1 Hz) — the same values the old per-call reads
    /// returned, now without any UI-thread round-trip.
    #[test]
    #[cfg(unix)]
    fn slow_cache_fills_with_daemon_values() {
        let (client, dir, handle, _guard) = live_client();
        assert!(
            wait_for(&client, IpcApi::slow_filled),
            "slow cache should fill"
        );
        assert!(
            client.slow_generation() > 0,
            "a completed cycle bumps the generation"
        );
        assert_eq!(client.version(), app_version());
        assert_eq!(client.firmware_version(), "Unknown");
        assert_eq!(client.output_backend(), "vJoy");
        assert!(client.autostart());
        assert_eq!(client.board_address(), "192.168.4.1");
        assert!(client.close_to_tray());
        assert!(!client.auto_pause());
        for module in ["handbrake", "shifter", "horn", "led", "steering"] {
            assert!(client.module_enabled(module), "{module} enabled");
        }
        let check = client.firmware_check().expect("sim firmware check");
        assert_eq!(check.board_version, "Unknown");
        assert!(!check.update_available);
        assert!(client.calibration().is_err());
        assert!(client.vjoy_status().is_err());
        assert!(client.diag_stats().is_err());
        assert_eq!(client.list_profiles().len(), 0);
        assert_eq!(client.list_games().len(), 0);
        assert_eq!(client.list_bindings().len(), 0);
        assert_eq!(client.get_tuning("brake").module, "brake");
        assert_eq!(client.pairing_status().seconds_left, 0);
        assert!(!client.firmware_update_status().running);
        drop(client);
        handle.join().expect("server thread");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// F5/F6: a successful write updates its cache slot at once —
    /// no waiting for the next slow cycle. Proves the write-through
    /// half of the cache (the read half is the test above).
    #[test]
    #[cfg(unix)]
    fn set_tuning_updates_tuning_cache_at_once() {
        let (client, dir, handle, _guard) = live_client();
        // Deliberately no fill wait: the write must update the
        // cache by itself.
        let mut upd = TuningSettings::neutral("brake");
        upd.deadzone = 7.0;
        let stored = client.set_tuning(&upd).expect("set_tuning");
        assert_eq!(stored.deadzone, 7.0);
        assert_eq!(client.get_tuning("brake").deadzone, 7.0);
        assert!(client.draft_status().is_some_and(|s| s.dirty));
        assert!(client.save_draft().expect("save"));
        assert!(client.draft_status().is_none());
        drop(client);
        handle.join().expect("server thread");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// F5: `clear_cached_state` empties the slow slots too (no
    /// stale firmware / profiles / tunings survive a death), and
    /// the pollers refill them — the fast one clears `link_dead`
    /// on its next good round, then the slow one refills.
    #[test]
    #[cfg(unix)]
    fn clear_cached_state_clears_slow_cache() {
        let (client, dir, handle, _guard) = live_client();
        assert!(
            wait_for(&client, IpcApi::slow_filled),
            "slow cache should fill"
        );
        assert_eq!(client.output_backend(), "vJoy");
        client.clear_cached_state();
        assert!(!client.slow_filled(), "clear must reset the filled flag");
        assert_eq!(client.output_backend(), "Unknown");
        assert_eq!(client.firmware_version(), "Unknown");
        assert!(!client.autostart());
        assert_eq!(client.get_tuning("steering").curve, 1.0);
        assert_eq!(client.list_profiles().len(), 0);
        assert!(
            wait_for(&client, IpcApi::slow_filled),
            "slow cache should refill"
        );
        assert_eq!(client.output_backend(), "vJoy");
        drop(client);
        handle.join().expect("server thread");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// F5: the reconnect seed fetch runs its own synchronous slow
    /// refresh (no fill wait needed) and reports the session
    /// `connect` verdict the wizard follows; the rest of the seed
    /// lands in the cache (read back through the client here).
    #[test]
    #[cfg(unix)]
    fn live_seed_fetch_fills_cache_and_reports_connect() {
        let (client, dir, handle, _guard) = live_client();
        let seed = LiveSeed::fetch(&client, ApiTransport::Wifi);
        assert!(seed.connect.is_ok(), "sim connect succeeds");
        assert!(seed.connection_active, "sim session is Active");
        assert_eq!(seed.backend, "vJoy");
        assert!(seed.autostart);
        assert_eq!(seed.pairing.seconds_left, 0);
        assert!(client.slow_filled(), "seed fetch fills the slow cache");
        assert_eq!(client.version(), app_version());
        assert_eq!(client.firmware_version(), "Unknown");
        assert_eq!(client.get_tuning("steering").module, "steering");
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

    #[test]
    fn daemon_connect_error_keeps_unreachable_instead_of_transport_closed() {
        // F26: the daemon's no-board verdict arrives as
        // `IpcError::Api("Unreachable: daemon API error:
        // Unreachable")` — it must stay `Unreachable`, not
        // collapse into `TransportClosed` (which is what every
        // no-board (re)connect used to notify as "Reconnect
        // failed"). A real typed reason survives too; an
        // unrecognised daemon code stays the generic link failure.
        let error = IpcApi::daemon_connect_error("Unreachable: daemon API error: Unreachable");
        assert_eq!(error.code, "Unreachable");
        assert_eq!(error.title, "Unreachable");
        // The fake daemon sends an empty message (`"Unreachable: "`)
        // where the real server sends `"daemon API error:
        // Unreachable"` — both must parse to the same kind.
        let error = IpcApi::daemon_connect_error("Unreachable: ");
        assert_eq!(error.code, "Unreachable");
        let error = IpcApi::daemon_connect_error("BadTag: daemon API error: BadTag");
        assert_eq!(error.code, "BadTag");
        let error = IpcApi::daemon_connect_error("AddrInUse: daemon API error: AddrInUse");
        assert_eq!(error.code, "AddrInUse");
        let error = IpcApi::daemon_connect_error("NoSuchCode: daemon API error: NoSuchCode");
        assert_eq!(
            error.code, "TransportClosed",
            "an unrecognised daemon code stays the generic link failure"
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
                attempt_log: Vec::new(),
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
        /// Test-only: `list_profiles` sleeps this long (DS-80 — a
        /// controllable blocking handler). Zero in normal tests.
        list_profiles_delay: std::time::Duration,
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
                list_profiles_delay: std::time::Duration::ZERO,
            }
        }

        fn with_list_profiles_delay(delay: std::time::Duration) -> Self {
            let mut api = Self::new();
            api.list_profiles_delay = delay;
            api
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

        // DS-80 test hook: a controllable blocking handler.
        fn list_profiles(&self) -> Vec<ProfileSummary> {
            if !self.list_profiles_delay.is_zero() {
                std::thread::sleep(self.list_profiles_delay);
            }
            self.inner.list_profiles()
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

    /// DS-80: the fast poller runs on its own connection — while a
    /// 2 s `slow_poll` occupies the main client (its single-flight
    /// lock held by the server-side `list_profiles` sleep), the
    /// poller keeps running `poll` cycles on the second connection.
    /// Without the dedicated connection the poller would stall
    /// behind the blocked main client and run 0 cycles here.
    #[test]
    #[cfg(unix)]
    fn ds80_fast_poller_keeps_polling_while_main_client_blocked() {
        let _guard = crate::live_probe::TEST_IPC_PIPE_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = std::env::temp_dir().join(format!("rsh-ds80-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let sock = dir.join("rsh-daemon.sock");
        let counting = Arc::new(CountingApi::with_list_profiles_delay(
            std::time::Duration::from_secs(2),
        ));
        let api: Arc<dyn AppApi> = counting.clone();
        let (tx, _rx) = broadcast::channel(64);
        let server = Arc::new(IpcServer::new(api, tx));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
        let sock_clone = sock.clone();
        let handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("server runtime");
            rt.block_on(async move {
                let listener = rsh_ipc::bind_unix_socket(&sock_clone).expect("bind");
                ready_tx.send(()).expect("ready signal");
                // Two connections: the main client and the fast
                // poller's own connection, served concurrently —
                // each serve task starts on accept (the main
                // client's `subscribe` must be answered before the
                // poller connects, so serving cannot wait for the
                // second accept).
                let mut tasks = tokio::task::JoinSet::new();
                for _ in 0..2 {
                    if let Ok((stream, _)) = listener.accept().await {
                        let server = Arc::clone(&server);
                        tasks.spawn(async move {
                            server.serve_stream(stream).await;
                        });
                    }
                }
                while tasks.join_next().await.is_some() {}
            });
        });
        ready_rx.recv().expect("server bound");
        let client = IpcApi::connect(&sock).expect("connect");
        // Wait for the poller to run at least once.
        let mut polls = 0;
        for _ in 0..100 {
            polls = counting.poll_calls.load(Ordering::Relaxed);
            if polls >= 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(polls >= 1, "poller never ran a poll cycle");
        // Block the main client for ~2 s from another thread: a
        // slow refresh whose server-side `slow_poll` sleeps in
        // `list_profiles`, holding the main client's
        // single-flight lock the whole time.
        let blocked = client.clone();
        let join = std::thread::spawn(move || {
            blocked.refresh_slow_blocking();
        });
        // While the main client is blocked, the poller (own
        // connection) must keep running: ~20 cycles/s.
        std::thread::sleep(Duration::from_millis(1000));
        let polls_during = counting.poll_calls.load(Ordering::Relaxed) - polls;
        assert!(
            polls_during >= 5,
            "poller stalled behind the blocked main client: only {polls_during} polls in 1 s"
        );
        join.join().expect("blocked refresh finishes");
        drop(client);
        handle.join().expect("server thread");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
