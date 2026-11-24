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
//! wifi, calibration, profiles, games, bindings, knobs, modules,
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
use std::time::{Duration, Instant};

use rsh_api::connection::ConnectionState;
use rsh_api::{
    app_version, ApiTransport, AppApi, AppEvent, ButtonBinding, CalibrationInfo, ConnectionError,
    ConnectionInfo, DiagStats, DraftStatus, FirmwareCheck, FirmwareUpdateStatus, GameSummary,
    PollState, ProfileSummary, RawMv, SlowPollState, SnapshotInfo, TuningSettings, UserError,
    VjoyStatus, WifiStatus,
};
use rsh_ipc::{IpcClient, IpcError};

/// Background-polled live values (B3): the poller task in
/// [`IpcApi::connect`] refreshes these ~20 Hz, so the UI thread
/// only ever reads the cache. `link_dead` is set by the poller when
/// a round fails and cleared on the next good round; fallible calls
/// fail fast on it instead of waiting out the 500 ms timeout.
///
/// F5 (v2.0.3): the slow slots below hold everything else the UI
/// re-reads — firmware / address / diagnostics / wifi /
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
/// failed call. `generation` bumps at the end of a completed
/// slow cycle whose content actually changed (R2-11); the frame
/// timer re-syncs the pages when it changes.
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
    wifi_status: Mutex<Option<WifiStatus>>,
    calibration: Mutex<Option<CalibrationInfo>>,
    /// Latest raw ADC readings in mV (R2-16): refreshed by the
    /// slow poller ~1 Hz; the Advanced readouts read this slot.
    raw_mv: Mutex<Option<RawMv>>,
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
    /// R2-11: hash of the last slow content the generation was
    /// bumped for (see [`slow_content_hash`]) — the generation
    /// bumps only when the content actually changed, never once
    /// per cycle on identical values.
    last_content_hash: AtomicU64,
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
            wifi_status: Mutex::new(None),
            calibration: Mutex::new(None),
            raw_mv: Mutex::new(None),
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
            last_content_hash: AtomicU64::new(0),
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

/// R2-11: no recorded content hash yet — the next completed
/// cycle always bumps (its hash cannot equal this sentinel in
/// practice). Stored by `note_write_landed` after an in-place
/// cache update and by `clear_cached_state`, so the next cycle
/// bumps even when it reverts the cache to its pre-write
/// content (a discard): without it the revert would hash equal
/// to the recorded hash and the UI would never re-sync.
const CONTENT_HASH_DIRTY: u64 = u64::MAX;

/// R2-21: wall-clock budget for the sequential `slow_poll`
/// fallback pass in [`refresh_slow`]. The combined call is one
/// 500 ms round-trip; the fallback is 30 sequential calls, each
/// up to 500 ms — up to ~15 s of stall before the next cycle.
/// A cycle that cannot finish inside the budget keeps its last
/// good values and ends; the next ~1 s cycle retries.
const SLOW_FALLBACK_BUDGET: Duration = Duration::from_secs(2);

/// R2-21: consecutive failed fetches that abort the fallback
/// pass early — a daemon that answers nothing fails each fetch
/// in 500 ms, so 3 in a row is enough signal without waiting
/// out the whole budget.
const SLOW_FALLBACK_MAX_CONSECUTIVE_FAILURES: u32 = 3;

/// Item 3c (perf-infra, PERF-RESEARCH §6 item 3): consecutive
/// slow cycles whose content hash is unchanged before the poller
/// parks at the 5 s cadence (see [`slow_park_steps`]). 3 matches the
/// "3 strikes" motif elsewhere (`LINK_DEAD_AFTER_FAILURES`,
/// `SLOW_FALLBACK_MAX_CONSECUTIVE_FAILURES`) — ~3 s of stability
/// proves the slow values are idle, and a change (or a write)
/// returns to 1 Hz at once.
const SLOW_PARK_AFTER_UNCHANGED_CYCLES: u32 = 3;

/// Item 3c: wait steps (x 50 ms) between slow cycles — 20 = 1 s
/// while the content changes, 100 = 5 s once parked.
const SLOW_PERIOD_STEPS: u32 = 20;
const SLOW_PARKED_PERIOD_STEPS: u32 = 100;

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

/// Item 3c (perf-infra): how many 50 ms wait steps the slow
/// poller takes before its next cycle — 1 s while the slow
/// content is changing, 5 s once it has been unchanged for
/// [`SLOW_PARK_AFTER_UNCHANGED_CYCLES`] cycles. (Pure, so the
/// park decision is unit-testable without a runtime — same
/// pattern as [`link_dead_after_round`].)
fn slow_park_steps(unchanged_cycles: u32) -> u32 {
    if unchanged_cycles >= SLOW_PARK_AFTER_UNCHANGED_CYCLES {
        SLOW_PARKED_PERIOD_STEPS
    } else {
        SLOW_PERIOD_STEPS
    }
}

/// Item 3c: the unchanged-cycle count after one slow cycle.
/// Returns `(unchanged_after)`: a generation bump (content
/// actually changed — R2-11) resets to 0, so the next wait is
/// 1 Hz again at once; a cycle that ran while the link was dead
/// also resets to 0 — `refresh_slow` bailed early, so the
/// unchanged generation is no evidence of stable content, and
/// parking would delay the post-recovery refill by up to 5 s;
/// anything else counts one more stable cycle. (Pure — see
/// [`slow_park_steps`].)
fn slow_unchanged_after_cycle(
    generation_changed: bool,
    link_dead: bool,
    unchanged_cycles: u32,
) -> u32 {
    if generation_changed || link_dead {
        0
    } else {
        unchanged_cycles.saturating_add(1)
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

/// F-11 (perf-infra, PERF-RESEARCH §6 item 7): job-lane A
/// methods — the tuning/draft write class. `set_tuning` (slider
/// pushes; the daemon persists the record), `save_draft`,
/// `discard_draft` and `set_module_enabled` can each hold a
/// connection's single-flight lock up to the 500 ms call bound,
/// and they are order-sensitive with each other (a save must not
/// overtake an in-flight tuning push, or the saved draft misses
/// it). They share lane A (`client`); every other job call runs
/// on lane B (`job2_client`), so a slow save can no longer stall
/// an unrelated job behind it. The poller lanes (fast/slow) and
/// the long-call lane are untouched.
/// Bound for add / update game: an atomic file write plus a process
/// refresh on the daemon, which can exceed the default 500 ms.
const GAME_WRITE_TIMEOUT: Duration = Duration::from_secs(10);

const JOB_LANE_A_METHODS: &[&str] = &[
    "set_tuning",
    "save_draft",
    "discard_draft",
    "set_module_enabled",
];

/// Open a second IPC connection for one background lane (DS-80,
/// R2-22): the fast poller, the slow poller and the long-call
/// lane each get their own connection, so a slow call on one lane
/// never delays another lane behind the single-flight lock.
/// Verified with a real `ping` before use: a server that accepts
/// only one connection (the `ipc_api` test server) leaves the
/// second connect unanswered, so the lane falls back to the
/// shared client instead of stalling. The probe is bounded to
/// 200 ms — a healthy daemon answers in under a millisecond.
fn probe_lane_client(
    rt: &tokio::runtime::Runtime,
    path: &Path,
    main: &Arc<IpcClient>,
) -> Arc<IpcClient> {
    rt.block_on(async {
        let candidate = tokio::time::timeout(Duration::from_secs(2), IpcClient::connect(path))
            .await
            .ok()
            .and_then(Result::ok);
        match candidate {
            Some(candidate) => {
                let pinged =
                    tokio::time::timeout(Duration::from_millis(200), candidate.ping()).await;
                match pinged {
                    Ok(Ok(true)) => candidate,
                    _ => main.clone(),
                }
            }
            None => main.clone(),
        }
    })
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
    /// Wi-Fi status (wizard Connect step: SSID / password set).
    pub wifi: WifiStatus,
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
        Self {
            backend: api.output_backend(),
            autostart: api.autostart(),
            connect,
            wifi: api.wifi_status(),
        }
    }
}

/// Live daemon API over IPC.
pub struct IpcApi {
    client: Arc<IpcClient>,
    /// R2-22: the long-call lane — `call_async_long` (the 75 s
    /// backend switch) runs here, so a UAC dwell can no longer
    /// stall the slow poller or a `set_tuning` job behind the
    /// main client's single-flight lock. Falls back to `client`
    /// when the daemon accepts only one connection (see
    /// [`probe_lane_client`]).
    long_client: Arc<IpcClient>,
    /// F-11 (perf-infra): the second job lane — every
    /// [`call_async`](Self::call_async) call whose method is NOT
    /// in [`JOB_LANE_A_METHODS`] runs here, so a slow
    /// `set_tuning`/save on the main job client can no longer
    /// stall an unrelated job behind the single-flight lock.
    /// Falls back to `client` when the daemon accepts only one
    /// connection (see [`probe_lane_client`]).
    job2_client: Arc<IpcClient>,
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
        // DS-80 / R2-22: one connection per lane — fast poller,
        // slow poller, long calls — so no lane can stall another
        // behind the single-flight lock. Each probe falls back to
        // the shared client when the daemon accepts only one
        // connection (see `probe_lane_client`).
        let poll_client = probe_lane_client(&rt, path, &client);
        let slow_client = probe_lane_client(&rt, path, &client);
        let long_client = probe_lane_client(&rt, path, &client);
        // F-11 (perf-infra): the second job lane — same probe +
        // fallback as the other lanes.
        let job2_client = probe_lane_client(&rt, path, &client);
        let cache = Arc::new(LiveCache::default());
        let api = Arc::new(Self {
            client: client.clone(),
            long_client,
            job2_client,
            rt,
            cache: cache.clone(),
        });
        {
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
            // F5 (v2.0.3) / R2-22: the slow poller refreshes every
            // other read-only value the UI shows (~1 Hz, one
            // sequential pass over the read methods — each bounded
            // to 500 ms and fail-fast once the fast poller proves
            // the link dead, so a dying daemon costs one cheap
            // skipped cycle instead of a ~15 s stall). A first
            // pass runs at once, so the cache is warm ~50 ms after
            // connect on a healthy daemon; later passes wait out
            // the period (1 s, or 5 s once parked — item 3c) in
            // 50 ms steps and run early when a write asked for a
            // refresh. The poller runs on its OWN connection
            // (R2-22): a 75 s backend switch on the job lane can
            // no longer stall it, and a stalled slow cycle can no
            // longer delay a `set_tuning` job.
            let slow_cache = cache;
            api.rt.spawn(async move {
                refresh_slow(&slow_client, &slow_cache).await;
                // Item 3c (perf-infra): park the cadence when
                // the slow content is stable — 3 unchanged
                // cycles (~3 s) move the wait from 1 s to 5 s,
                // and any generation bump returns to 1 Hz at
                // once. The wait still steps in 50 ms slices and
                // checks `refresh_requested` every step, so a
                // write pulling the next cycle forward lands just
                // as fast while parked as at 1 Hz — parking only
                // skips idle daemon round-trips, never a
                // write-triggered refresh. The link-dead case
                // never parks (see `slow_unchanged_after_cycle`).
                let mut unchanged_cycles: u32 = 0;
                loop {
                    for _ in 0..slow_park_steps(unchanged_cycles) {
                        if slow_cache.refresh_requested.swap(false, Ordering::Relaxed) {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                    let generation_before = slow_cache.generation.load(Ordering::Relaxed);
                    refresh_slow(&slow_client, &slow_cache).await;
                    let generation_changed =
                        slow_cache.generation.load(Ordering::Relaxed) != generation_before;
                    unchanged_cycles = slow_unchanged_after_cycle(
                        generation_changed,
                        slow_cache.link_dead.load(Ordering::Relaxed),
                        unchanged_cycles,
                    );
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
    ///
    /// F-11 (perf-infra): two job lanes — methods in
    /// [`JOB_LANE_A_METHODS`] (the tuning/draft writes) run on the
    /// main job client, everything else on `job2_client`, so a
    /// slow save no longer stalls an unrelated job behind the
    /// single-flight lock. On a single-connection daemon both
    /// lanes are the same client (probe fallback), so behaviour
    /// is unchanged there.
    async fn call_async(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, IpcError> {
        if self.cache.link_dead.load(Ordering::Relaxed) {
            return Err(IpcError::Closed);
        }
        let lane = if JOB_LANE_A_METHODS.contains(&method) {
            &self.client
        } else {
            &self.job2_client
        };
        lane.call(method, params).await
    }

    /// [`call_async`] with a longer bound for calls that touch the
    /// disk and the process list on the daemon side (add / update /
    /// remove game: an atomic file write + a process refresh, which
    /// can pass the default 500 ms on Windows with an AV scan — and
    /// then surfaced as a bogus "Connection dropped"). Same
    /// fail-fast on a proven-dead link, same shared job client.
    async fn call_async_slow(
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

    /// [`call_async`] with an explicit bound (B-2, v2.0.2): the
    /// backend switch waits out a UAC prompt + a 60 s sidecar spawn
    /// (75 s), so the UI does not time out while the user is still
    /// answering the prompt. Same fail-fast on a proven-dead link.
    /// Runs on the long-call lane (R2-22), not the shared job
    /// client — the switch must not stall the slow poller or a
    /// `set_tuning` job behind the single-flight lock.
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
        // call is in flight. On a single-connection daemon the
        // lane falls back to the shared client, so the polls
        // behind it time out on the single-flight lock — which is
        // not evidence of a dead link. The guard clears the flag
        // on every exit path, including cancellation/panic.
        self.cache
            .long_call_in_flight
            .store(true, Ordering::Relaxed);
        let _guard = LongCallGuard { cache: &self.cache };
        self.long_client
            .call_with_timeout(method, params, timeout)
            .await
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
        if let Ok(mut slot) = self.cache.wifi_status.lock() {
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
        self.cache
            .last_content_hash
            .store(CONTENT_HASH_DIRTY, Ordering::Relaxed);
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

    /// Whether the fast poller has proven the link dead (DS-19):
    /// true after [`LINK_DEAD_AFTER_FAILURES`] consecutive failed
    /// `poll` rounds, cleared by the next good round. This is the
    /// fast poller's failure signal, exposed for the UI's health
    /// monitor (WS-UI merge) — `call_async` already fails fast on
    /// it, but no `AppApi` method exposed it, so the UI could not
    /// read the signal itself. Read-only: no behaviour change,
    /// no poller code touched. Called by `spawn_health_monitor`
    /// (perf F-1 merge) instead of a separate 1 Hz `ping()`.
    #[must_use]
    pub fn link_dead(&self) -> bool {
        self.cache.link_dead.load(Ordering::Relaxed)
    }

    /// Current slow-cache generation (F5): bumps at the end of a
    /// completed slow cycle whose content actually changed (R2-11 —
    /// a cycle that reads back identical values does not bump), or
    /// of a cycle a write pulled forward (a discard can hash back
    /// to a previously seen value and still must bump). The frame
    /// timer re-syncs the pages when it changes — that is how a
    /// daemon-side change reaches the UI without any UI-thread IPC.
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
        self.note_write_landed();
    }

    /// Note that a write landed (R2-13, DS-29): bump the write
    /// epoch so an in-flight slow cycle cannot bump the generation
    /// with pre-write values — WITHOUT pulling the next cycle
    /// forward. `set_tuning` updates its slot in place, so no
    /// forced re-fetch is needed; the next natural ~1 Hz cycle
    /// re-reads the rest. R2-11: the write also invalidates the
    /// recorded content hash (`CONTENT_HASH_DIRTY`) — the in-place
    /// update never passed through a cycle, so without this a
    /// later cycle that reverts the cache to its pre-write
    /// content (a discard) would hash equal to the recorded hash
    /// and never bump, leaving the UI waiting for a refresh that
    /// already happened.
    fn note_write_landed(&self) {
        self.cache.refresh_epoch.fetch_add(1, Ordering::Relaxed);
        self.cache
            .last_content_hash
            .store(CONTENT_HASH_DIRTY, Ordering::Relaxed);
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

    /// Build the [`UserError`] for a failed `set_output_backend`
    /// (Bug 4/8): the daemon's `IpcError::Api` payload carries its
    /// code first and the failure's real reason after it
    /// (`"{code}: {reason}"` — see `dispatch_settings` in
    /// `rsh-ipc`), wrapped by the client as
    /// `"daemon API error: {code}: daemon API error: {code}:
    /// {reason}"`. The reason is the text after the LAST marker,
    /// split from its code at the first `": "` (the reason itself
    /// may contain `": "`). A `not_admin` code rebuilds the typed
    /// UAC-decline error (same words as the daemon's — see
    /// `daemon_api.rs` / `AppStateCore::HID_NOT_ADMIN`, the source
    /// of truth); any other code keeps the daemon's reason as the
    /// detail, so the toast names the sidecar/driver failure instead
    /// of a generic link failure. A payload without a reason (fake
    /// daemon, empty message) or a non-`Api` error (dead link,
    /// timeout) is still the generic link failure — only the
    /// daemon's own typed reason is preserved, nothing is invented.
    fn daemon_backend_error(error: &IpcError) -> UserError {
        let payload = error.to_string();
        let Some(tail) = payload
            .rsplit("daemon API error: ")
            .next()
            .filter(|tail| *tail != payload.as_str())
        else {
            return Self::ipc_failed();
        };
        let (code, reason) = match tail.split_once(": ") {
            Some((code, reason)) => (code.trim(), reason.trim()),
            None => (tail.trim(), ""),
        };
        if code == "not_admin" {
            // The daemon's typed decline: `HIDMaestro needs
            // administrator rights — approve the UAC prompt to
            // switch to it, or run RSH as administrator. vJoy needs
            // no prompt.` (kept in sync with `HID_NOT_ADMIN`).
            return UserError {
                code: "not_admin".to_string(),
                title: "Admin permission needed".to_string(),
                what_happened: if reason.is_empty() {
                    "HIDMaestro needs administrator rights to switch to it."
                        .to_string()
                } else {
                    reason.to_string()
                },
                what_to_do: "Approve the Windows prompt when switching to HIDMaestro, or keep using vJoy (no permission needed)."
                    .to_string(),
                detail: None,
            };
        }
        if reason.is_empty() {
            return Self::ipc_failed();
        }
        UserError::from_connection(ConnectionError::Unknown, reason.to_string())
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
            board_reachable: false,
            error: Some(ConnectionError::TransportClosed),
            hint: Some(ConnectionError::TransportClosed.hint().to_string()),
        }
    }
}

/// One slow refresh pass (F5, v2.0.3): every read-only value the
/// UI shows that the fast poller does not cover, fetched
/// sequentially into the cache. Each step checks `link_dead` first
/// and bails, and the sequential fallback pass is budgeted
/// (R2-21: 2 s wall clock / 3 consecutive failures) — a daemon
/// that died mid-pass costs at most ~2.5 s, never the whole
/// ~15 s pass. A failed step keeps its last good value (or the
/// cold-cache fallback): a slow value going briefly stale beats
/// a frozen frame. A completed pass marks the cache filled and
/// bumps the generation when the content changed, so the frame
/// timer re-syncs the pages from the fresh values.
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
        set_slot!(wifi_status, state.wifi_status);
        if let Some(calibration) = state.calibration {
            set_slot!(calibration, calibration);
        }
        if let Some(raw_mv) = state.raw_mv {
            set_slot!(raw_mv, raw_mv);
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
        bump_generation_if_changed(cache, epoch);
        return;
    }
    // R2-21: the fallback pass is budgeted (see
    // `SLOW_FALLBACK_BUDGET` / `SLOW_FALLBACK_MAX_CONSECUTIVE_FAILURES`):
    // a cycle that runs long keeps its last good values and ends
    // instead of stalling the poller for ~15 s; the next ~1 s
    // cycle retries.
    let fallback_started = Instant::now();
    let mut consecutive_failures = 0u32;
    // Finish the pass the same way the normal tail does, then
    // leave: the next ~1 s cycle retries with fresh values.
    let finish_fallback = || {
        cache.slow_filled.store(true, Ordering::Relaxed);
        bump_generation_if_changed(cache, epoch);
    };
    // One call, parsed or ignored. `parse` turns the reply into
    // the slot's value (`None` = unparseable, keep the old value).
    macro_rules! fetch {
        ($method:literal, $slot:ident, $parse:expr) => {{
            if cache.link_dead.load(Ordering::Relaxed)
                || fallback_started.elapsed() >= SLOW_FALLBACK_BUDGET
                || consecutive_failures >= SLOW_FALLBACK_MAX_CONSECUTIVE_FAILURES
            {
                finish_fallback();
                return;
            }
            match client.call($method, serde_json::json!({})).await {
                Ok(value) => {
                    consecutive_failures = 0;
                    let parse: fn(&serde_json::Value) -> Option<_> = $parse;
                    if let Some(parsed) = parse(&value) {
                        if let Ok(mut slot) = cache.$slot.lock() {
                            *slot = Some(parsed);
                        }
                    }
                }
                Err(_) => {
                    consecutive_failures += 1;
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
    fetch!("wifi_status", wifi_status, |v| {
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
    // R2-21: the budget / failure cap applies here too.
    if !cache.link_dead.load(Ordering::Relaxed)
        && fallback_started.elapsed() < SLOW_FALLBACK_BUDGET
        && consecutive_failures < SLOW_FALLBACK_MAX_CONSECUTIVE_FAILURES
    {
        match client.call("draft_status", serde_json::json!({})).await {
            Ok(value) => {
                consecutive_failures = 0;
                if let Ok(draft) = serde_json::from_value::<Option<DraftStatus>>(value) {
                    if let Ok(mut slot) = cache.draft.lock() {
                        *slot = draft;
                    }
                }
            }
            Err(_) => {
                consecutive_failures += 1;
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
        if cache.link_dead.load(Ordering::Relaxed)
            || fallback_started.elapsed() >= SLOW_FALLBACK_BUDGET
            || consecutive_failures >= SLOW_FALLBACK_MAX_CONSECUTIVE_FAILURES
        {
            break;
        }
        match client
            .call("get_tuning", serde_json::json!({"module": module}))
            .await
        {
            Ok(value) => {
                consecutive_failures = 0;
                if let Ok(tuning) = serde_json::from_value::<TuningSettings>(value) {
                    if let Ok(mut tunings) = cache.tunings.lock() {
                        tunings.insert(module.to_string(), tuning);
                    }
                }
            }
            Err(_) => {
                consecutive_failures += 1;
            }
        }
    }
    for (module, index) in [("handbrake", 0), ("shifter", 1), ("horn", 2), ("led", 3)] {
        if cache.link_dead.load(Ordering::Relaxed)
            || fallback_started.elapsed() >= SLOW_FALLBACK_BUDGET
            || consecutive_failures >= SLOW_FALLBACK_MAX_CONSECUTIVE_FAILURES
        {
            break;
        }
        match client
            .call("module_enabled", serde_json::json!({"module": module}))
            .await
        {
            Ok(value) => {
                consecutive_failures = 0;
                if let Some(enabled) = value.get("enabled").and_then(serde_json::Value::as_bool) {
                    if let Ok(mut modules) = cache.modules.lock() {
                        modules[index] = Some(enabled);
                    }
                }
            }
            Err(_) => {
                consecutive_failures += 1;
            }
        }
    }
    finish_fallback();
}

/// Hash one slow slot's JSON into the content hash (R2-11):
/// JSON field order is fixed per struct, so equal values hash
/// equal; an unserialisable value (never expected — every slot
/// type is `Serialize`) contributes nothing instead of failing
/// the cycle.
fn hash_slot<T: serde::Serialize>(
    hasher: &mut std::collections::hash_map::DefaultHasher,
    value: &T,
) {
    use std::hash::Hash;
    if let Ok(text) = serde_json::to_string(value) {
        text.hash(hasher);
    }
}

/// Content hash of the slow cache (R2-11): every slow slot that
/// feeds `resync_from_cache`, EXCEPT `diag_stats` — its p99s and
/// counters move every cycle while live, so including it would
/// bump the generation ~1 Hz forever and defeat the point. Diag
/// text is re-synced on the 1 Hz tick itself (guarded writes),
/// not through the generation. Tunings hash with sorted keys: a
/// `HashMap` iterates in arbitrary order, and the hash must be
/// stable for equal contents.
fn slow_content_hash(cache: &LiveCache) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::Hasher;
    let mut hasher = DefaultHasher::new();
    macro_rules! slot {
        ($field:ident) => {
            if let Ok(guard) = cache.$field.lock() {
                hash_slot(&mut hasher, &*guard);
            }
        };
    }
    slot!(version);
    slot!(firmware_version);
    slot!(backend);
    slot!(autostart);
    slot!(board_address);
    slot!(wifi_status);
    slot!(calibration);
    slot!(profiles);
    slot!(games);
    slot!(firmware_check);
    slot!(firmware_update_status);
    slot!(vjoy_status);
    slot!(auto_pause);
    slot!(close_to_tray);
    slot!(bindings);
    slot!(knobs);
    slot!(modules);
    slot!(draft);
    if let Ok(tunings) = cache.tunings.lock() {
        let mut keys: Vec<&String> = tunings.keys().collect();
        keys.sort();
        for key in keys {
            if let Some(tuning) = tunings.get(key) {
                hash_slot(&mut hasher, &key);
                hash_slot(&mut hasher, &tuning);
            }
        }
    }
    hasher.finish()
}

/// Bump the slow generation when the cycle's data is fresh
/// (DS-29) AND its content actually changed (R2-11): skip the
/// bump when a write asked for a refresh while this cycle ran —
/// the cycle may carry pre-write values, and its bump would tell
/// the UI (and the `discard_draft` test) the cache is current
/// when it is not — and skip it when the cycle changed nothing,
/// so the frame timer stops re-syncing every page ~1 Hz with
/// zero changes. `slow_filled` is NOT gated: the slots still hold
/// genuine daemon values, only possibly a cycle old.
fn bump_generation_if_changed(cache: &LiveCache, epoch: u64) {
    if cache.refresh_epoch.load(Ordering::Relaxed) != epoch {
        return;
    }
    let hash = slow_content_hash(cache);
    if cache.last_content_hash.swap(hash, Ordering::Relaxed) != hash {
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
            // next slow cycle forward so wifi / firmware follow.
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

    /// Cached Wi-Fi status (F5): the slow poller refreshes it
    /// ~1 Hz, so this never touches the socket. No cache yet means
    /// no password set and not connected (the pre-poller fallback).
    fn wifi_status(&self) -> WifiStatus {
        self.cache
            .wifi_status
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
            .unwrap_or(WifiStatus {
                ssid: None,
                connected: false,
                password_set: false,
                board_mac: None,
            })
    }

    fn set_wifi_password(&self, password: &str, confirm: &str) -> Result<WifiStatus, UserError> {
        let got: Result<WifiStatus, IpcError> = self.rt.block_on(async {
            let value = self
                .call_async(
                    "set_wifi_password",
                    serde_json::json!({"password": password, "confirm": confirm}),
                )
                .await?;
            serde_json::from_value(value).map_err(|e| IpcError::Protocol(e.to_string()))
        });
        match got {
            Ok(status) => {
                // F5: the password changes the Wi-Fi state — pull
                // the next slow cycle forward so the Settings
                // indicator follows the real state.
                self.request_slow_refresh();
                if let Ok(mut slot) = self.cache.wifi_status.lock() {
                    *slot = Some(status.clone());
                }
                Ok(status)
            }
            Err(e) => Err(Self::api_error(&e)),
        }
    }

    fn clear_wifi_password(&self) -> Result<WifiStatus, UserError> {
        let got: Result<WifiStatus, IpcError> = self.rt.block_on(async {
            let value = self
                .call_async("clear_wifi_password", serde_json::json!({}))
                .await?;
            serde_json::from_value(value).map_err(|e| IpcError::Protocol(e.to_string()))
        });
        match got {
            Ok(status) => {
                self.request_slow_refresh();
                if let Ok(mut slot) = self.cache.wifi_status.lock() {
                    *slot = Some(status.clone());
                }
                Ok(status)
            }
            Err(e) => Err(Self::api_error(&e)),
        }
    }

    fn scan_rsh_networks(&self) -> Vec<String> {
        self.rt
            .block_on(self.call_async("scan_rsh_networks", serde_json::json!({})))
            .ok()
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default()
    }

    fn rejoin_wifi_open(&self, ssid: &str) -> Result<WifiStatus, UserError> {
        let got: Result<WifiStatus, IpcError> = self.rt.block_on(async {
            let value = self
                .call_async("rejoin_wifi_open", serde_json::json!({"ssid": ssid}))
                .await?;
            serde_json::from_value(value).map_err(|e| IpcError::Protocol(e.to_string()))
        });
        match got {
            Ok(status) => {
                self.request_slow_refresh();
                Ok(status)
            }
            Err(e) => Err(Self::api_error(&e)),
        }
    }

    fn rejoin_wifi(&self, ssid: &str, password: &str) -> Result<WifiStatus, UserError> {
        let got: Result<WifiStatus, IpcError> = self.rt.block_on(async {
            let value = self
                .call_async(
                    "rejoin_wifi",
                    serde_json::json!({"ssid": ssid, "password": password}),
                )
                .await?;
            serde_json::from_value(value).map_err(|e| IpcError::Protocol(e.to_string()))
        });
        match got {
            Ok(status) => {
                self.request_slow_refresh();
                Ok(status)
            }
            Err(e) => Err(Self::api_error(&e)),
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
                // F5/F6: the stored record is the cache now, and
                // the draft is dirty at once (the daemon marks
                // it) — the next sync reads the new values, not
                // the pre-push ones. R2-13: NO forced slow
                // refresh here — the cache is already updated in
                // place, and a forced `slow_poll` after every
                // drag end fired the full ~1 Hz re-sync (list
                // rebuilds, `reg` spawn) ~150 ms after every
                // slider move. The epoch bump stays (DS-29): an
                // in-flight cycle must not bump the generation
                // with pre-push values. The normal ~1 Hz poll
                // picks up anything else.
                if let Ok(mut tunings) = self.cache.tunings.lock() {
                    tunings.insert(stored.module.clone(), stored.clone());
                }
                self.mark_draft_dirty(&stored.module);
                self.note_write_landed();
                Ok(stored)
            }
            Err(_) => Err(Self::ipc_failed()),
        }
    }

    fn shifter_sensitivity(&self) -> Result<Option<u8>, UserError> {
        let got: Result<Option<u8>, IpcError> = self.rt.block_on(async {
            let value = self
                .call_async("shifter_sensitivity", serde_json::json!({}))
                .await?;
            value
                .get("sensitivity")
                .and_then(|sensitivity| {
                    if sensitivity.is_null() {
                        Some(None)
                    } else {
                        sensitivity
                            .as_u64()
                            .and_then(|n| u8::try_from(n).ok())
                            .map(Some)
                    }
                })
                .ok_or_else(|| {
                    IpcError::Protocol(
                        "shifter_sensitivity must return {\"sensitivity\": number|null}"
                            .to_string(),
                    )
                })
        });
        // B6: the daemon's reason survives (`api_error` splits the
        // `code|detail` payload) — a "no shifter is configured"
        // must not collapse into a generic link failure.
        got.map_err(|error| Self::api_error(&error))
    }

    fn set_shifter_sensitivity(&self, sensitivity: u8) -> Result<u8, UserError> {
        let got: Result<u8, IpcError> = self.rt.block_on(async {
            let value = self
                .call_async(
                    "set_shifter_sensitivity",
                    serde_json::json!({"sensitivity": sensitivity}),
                )
                .await?;
            value
                .get("sensitivity")
                .and_then(serde_json::Value::as_u64)
                .and_then(|n| u8::try_from(n).ok())
                .ok_or_else(|| {
                    IpcError::Protocol(
                        "set_shifter_sensitivity must return {\"sensitivity\": number}".to_string(),
                    )
                })
        });
        got.map_err(|error| Self::api_error(&error))
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
            // Bug 4/8: a daemon-side switch failure must reach the
            // UI with its real reason — the old blanket
            // `ipc_failed()` turned every failed switch into
            // "daemon IPC link failed", hiding whether the sidecar,
            // the driver, or the UAC prompt was the problem.
            Err(error) => Err(Self::daemon_backend_error(&error)),
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

    /// Cached raw ADC readings (R2-16): the slow poller refreshes
    /// them ~1 Hz, so this never touches the socket. No cache yet
    /// is an error — the Advanced readouts show "—", never a fake 0.
    fn raw_mv(&self) -> Result<RawMv, UserError> {
        self.cache
            .raw_mv
            .lock()
            .ok()
            .and_then(|slot| *slot)
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

    fn capture_steering_left(&self) -> Result<u16, UserError> {
        self.capture_mv("capture_steering_left", serde_json::json!({}))
    }

    fn capture_steering_right(&self) -> Result<u16, UserError> {
        self.capture_mv("capture_steering_right", serde_json::json!({}))
    }

    // v2.0.2 Round 3: firmware check/update, board
    // address, module toggles, auto-pause, tray, backup, profile
    // rename/link, knob actions, bindings. Failures surface the
    // daemon's typed reason ([`Self::api_error`]), never a bare
    // "Unknown" — the UI shows the reason + the next step.
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

    /// Long-press bindings (BU2): a direct call, not the slow-poll
    /// cache — the UI reads them on (re)connect and after a save,
    /// like the knob actions. A failed read means an empty list
    /// (the display defaults stand), matching `list_bindings` with
    /// no cache yet.
    fn list_long_press_bindings(&self) -> Vec<ButtonBinding> {
        let got: Result<Vec<ButtonBinding>, IpcError> = self.rt.block_on(async {
            let value = self
                .call_async("list_long_press_bindings", serde_json::json!({}))
                .await?;
            value
                .get("bindings")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(|e| IpcError::Protocol(e.to_string()))?
                .ok_or_else(|| {
                    IpcError::Protocol("list_long_press_bindings has no bindings".to_string())
                })
        });
        got.unwrap_or_default()
    }

    fn set_long_press_binding(&self, button: u8, action: &str) -> Result<ButtonBinding, UserError> {
        let got: Result<ButtonBinding, IpcError> = self.rt.block_on(async {
            let value = self
                .call_async(
                    "set_long_press_binding",
                    serde_json::json!({"button": button, "action": action}),
                )
                .await?;
            value
                .get("binding")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(|e| IpcError::Protocol(e.to_string()))?
                .ok_or_else(|| {
                    IpcError::Protocol("set_long_press_binding has no binding".to_string())
                })
        });
        match got {
            Ok(binding) => {
                self.request_slow_refresh();
                Ok(binding)
            }
            Err(e) => Err(Self::api_error(&e)),
        }
    }

    fn clear_long_press_binding(&self, button: u8) -> Result<bool, UserError> {
        let got: Result<bool, IpcError> = self.rt.block_on(async {
            let value = self
                .call_async(
                    "clear_long_press_binding",
                    serde_json::json!({"button": button}),
                )
                .await?;
            value
                .get("cleared")
                .and_then(serde_json::Value::as_bool)
                .ok_or_else(|| {
                    IpcError::Protocol("clear_long_press_binding has no cleared".to_string())
                })
        });
        match got {
            Ok(cleared) => {
                if cleared {
                    self.request_slow_refresh();
                }
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
                .call_async_slow(
                    method,
                    serde_json::json!({
                        "id": id,
                        "name": name,
                        "exe_path": exe_path,
                        "profile_id": profile_id,
                    }),
                    GAME_WRITE_TIMEOUT,
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
            Err(e) => Err(Self::game_write_error(&e)),
        }
    }

    /// The [`UserError`] for a failed add / update game call: the
    /// daemon's `code|reason` payload (see `detailed_api_error`)
    /// keeps its real reason (empty name, unknown profile, "cannot
    /// save manual games: …") instead of collapsing into the
    /// generic "daemon IPC link failed". A dropped link / timeout
    /// stays the generic link failure.
    fn game_write_error(error: &IpcError) -> UserError {
        if let IpcError::Api(payload) = error {
            let head = payload
                .split(": daemon API error: ")
                .next()
                .unwrap_or(payload);
            if let Some((code, reason)) = head.split_once('|') {
                if !reason.is_empty() {
                    return UserError {
                        code: code.to_string(),
                        title: "Couldn't save the game".to_string(),
                        what_happened: format!("{reason}."),
                        what_to_do: String::new(),
                        detail: Some(reason.to_string()),
                    };
                }
            }
        }
        // Timeout / closed link / protocol: say what actually
        // happened instead of the misleading "Connection dropped".
        UserError {
            code: "Unknown".to_string(),
            title: "Couldn't save the game".to_string(),
            what_happened: format!("RSH didn't complete the request ({error})."),
            what_to_do: "Try again.".to_string(),
            detail: Some(error.to_string()),
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

    /// Item 3c (perf-infra): the park decision — 1 s steps while
    /// the slow content changes, 5 s steps after 3 unchanged
    /// cycles; a change or a dead link resets the count at once.
    #[test]
    fn slow_park_decision() {
        // Changing content never parks, however long it runs.
        assert_eq!(slow_park_steps(0), SLOW_PERIOD_STEPS);
        assert_eq!(slow_unchanged_after_cycle(true, false, 2), 0);
        assert_eq!(slow_unchanged_after_cycle(true, false, 10), 0);
        // One or two stable cycles: still 1 s, counting up.
        assert_eq!(slow_park_steps(1), SLOW_PERIOD_STEPS);
        assert_eq!(slow_unchanged_after_cycle(false, false, 0), 1);
        assert_eq!(slow_unchanged_after_cycle(false, false, 1), 2);
        assert_eq!(slow_park_steps(2), SLOW_PERIOD_STEPS);
        // The third stable cycle parks the next wait at 5 s.
        assert_eq!(slow_unchanged_after_cycle(false, false, 2), 3);
        assert_eq!(slow_park_steps(3), SLOW_PARKED_PERIOD_STEPS);
        assert_eq!(slow_park_steps(4), SLOW_PARKED_PERIOD_STEPS);
        // A change after parking returns to 1 Hz at once.
        assert_eq!(slow_unchanged_after_cycle(true, false, 3), 0);
        assert_eq!(slow_park_steps(0), SLOW_PERIOD_STEPS);
        // A cycle that ran while the link was dead never counts
        // toward parking — and never parks on a dead link.
        assert_eq!(slow_unchanged_after_cycle(false, true, 2), 0);
        assert_eq!(slow_unchanged_after_cycle(false, true, 10), 0);
        // Saturating: the count cannot wrap.
        assert_eq!(slow_unchanged_after_cycle(false, false, u32::MAX), u32::MAX);
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
        assert!(
            !client.wifi_status().password_set,
            "sim has no Wi-Fi password set"
        );
        assert!(!client.firmware_update_status().running);
        drop(client);
        handle.join().expect("server thread");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// R2-11: the slow generation bumps only when the slow values
    /// actually change — identical cycles stay quiet, a real write
    /// bumps again.
    #[test]
    #[cfg(unix)]
    fn slow_generation_bumps_only_on_content_change() {
        let (client, dir, handle, _guard) = live_client();
        assert!(
            wait_for(&client, IpcApi::slow_filled),
            "slow cache should fill"
        );
        let generation = client.slow_generation();
        assert!(generation > 0, "first cycle bumps");
        // Two more ~1 s cycles over an unchanged sim: no bump.
        std::thread::sleep(Duration::from_millis(2500));
        assert_eq!(
            client.slow_generation(),
            generation,
            "unchanged values must not bump"
        );
        // A real change bumps again.
        let mut upd = TuningSettings::neutral("steering");
        upd.deadzone = 5.0;
        client.set_tuning(&upd).expect("set_tuning");
        let mut waited = 0;
        while client.slow_generation() == generation && waited < 100 {
            std::thread::sleep(Duration::from_millis(50));
            waited += 1;
        }
        assert_ne!(
            client.slow_generation(),
            generation,
            "changed values must bump"
        );
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

    /// SH1: the shifter sensitivity rides the IPC methods, not
    /// `set_tuning`. The sim has no board, so the read is `None`
    /// (no shifter configured) and the write is refused — and the
    /// refusal's reason survives the IPC hop instead of collapsing
    /// into a generic link failure.
    #[test]
    #[cfg(unix)]
    fn shifter_sensitivity_over_live_client() {
        let (client, dir, handle, _guard) = live_client();
        assert_eq!(
            client.shifter_sensitivity().expect("read"),
            None,
            "sim has no board, so no shifter is configured"
        );
        let err = client
            .set_shifter_sensitivity(70)
            .expect_err("sim cannot write board config");
        assert_eq!(err.code, "Unknown");
        assert!(
            err.detail
                .as_deref()
                .is_some_and(|detail| detail.contains("not supported")),
            "reason must survive, got detail: {:?}",
            err.detail
        );
        drop(client);
        handle.join().expect("server thread");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// F-11 (perf-infra): a slow lane-A job (`set_tuning`, 400 ms
    /// on the daemon) must not stall a job on lane B. The slow
    /// `set_tuning` runs on a background thread; once the stub
    /// signals it entered the call, a lane-B job (`backup_now`)
    /// must return in well under the tuning's 400 ms — on one
    /// shared client it would queue behind the tuning and take
    /// ~400 ms. The server is multi-accept (`serve_listener`) on
    /// a multi-thread runtime, so the stub's blocking sleep stalls
    /// only its own worker, and the lane probes in
    /// [`IpcApi::connect`] all succeed (no fallback to the shared
    /// client — on a single-accept server the lanes would collapse
    /// and this test would fail, by design).
    #[cfg(unix)]
    struct SlowTuningApi {
        entered: Arc<std::sync::atomic::AtomicBool>,
    }

    #[cfg(unix)]
    impl AppApi for SlowTuningApi {
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
                board_reachable: false,
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

        fn wifi_status(&self) -> rsh_api::WifiStatus {
            rsh_api::WifiStatus {
                ssid: None,
                connected: false,
                password_set: false,
                board_mac: None,
            }
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
            // Signal entry first, then stall: the test's lane-B
            // call must land while this call is still in flight.
            // 400 ms stays under the 500 ms IPC call bound, so
            // the tuning itself still succeeds.
            self.entered
                .store(true, std::sync::atomic::Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(400));
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
    }

    #[test]
    #[cfg(unix)]
    fn slow_tuning_job_does_not_stall_other_job_lane() {
        let _guard = crate::live_probe::TEST_IPC_PIPE_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = std::env::temp_dir().join(format!("rsh-job-lane-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let socket = rsh_ipc::socket_path(&dir);
        let entered = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let api: Arc<dyn AppApi> = Arc::new(SlowTuningApi {
            entered: entered.clone(),
        });
        let (tx, _rx) = broadcast::channel(64);
        let server = Arc::new(IpcServer::new(api, tx));
        let socket_for_thread = socket.clone();
        // Detached server thread (capture-test precedent): it
        // serves until the test process exits. Multi-thread
        // runtime, so the stub's blocking 400 ms sleep stalls
        // only its own worker, not the other lanes.
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(4)
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
        // Lane A and lane B must be distinct connections here —
        // if every probe had fallen back to the shared client,
        // the timing assertion below could not pass.
        let mut upd = TuningSettings::neutral("brake");
        upd.deadzone = 7.0;
        let tuning_client = client.clone();
        let started = std::time::Instant::now();
        let tuning_handle = std::thread::spawn(move || tuning_client.set_tuning(&upd));
        // Wait until the slow tuning is actually in flight on the
        // daemon before issuing the lane-B call.
        for _ in 0..100 {
            if entered.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            entered.load(std::sync::atomic::Ordering::SeqCst),
            "slow set_tuning never started on the daemon"
        );
        // Lane-B job: `backup_now` is a default-Err on the stub,
        // which is fine — the test measures that the call
        // RETURNS promptly, not that it succeeds.
        let lane_b_start = std::time::Instant::now();
        let _ = client.backup_now();
        let lane_b_elapsed = lane_b_start.elapsed();
        assert!(
            lane_b_elapsed < Duration::from_millis(250),
            "lane-B job took {lane_b_elapsed:?} while a slow set_tuning was in flight — the lanes are not separate"
        );
        let stored = tuning_handle
            .join()
            .expect("tuning thread")
            .expect("slow set_tuning still succeeds");
        assert_eq!(stored.deadzone, 7.0);
        assert!(
            started.elapsed() >= Duration::from_millis(300),
            "the tuning was not actually slow — the test proved nothing"
        );
        drop(client);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Item 3c (perf-infra): once the slow content has been
    /// stable long enough to park (~3 s), a write that pulls the
    /// next cycle forward still lands at once — the parked 5 s
    /// wait must not delay a write-triggered refresh. Waits for
    /// the cache to fill, sleeps past the 3 unchanged cycles
    /// (`SimApi`'s slow values never change on their own), then
    /// dirties a tuning and discards the draft (which requests
    /// a refresh) and requires the generation bump within 2 s —
    /// without the early break it would take up to 5 s.
    #[test]
    #[cfg(unix)]
    fn parked_slow_poller_still_refreshes_early_on_write() {
        let (client, dir, handle, _guard) = live_client();
        assert!(
            wait_for(&client, IpcApi::slow_filled),
            "slow cache should fill"
        );
        // Settle: the first bump can land just after `slow_filled`
        // is observed (fill is stored before the bump), so record
        // the generation only once it is quiet.
        std::thread::sleep(Duration::from_millis(500));
        let generation = client.slow_generation();
        // 3 unchanged cycles (~3 s) park the poller; sleep past
        // them with margin. The SimApi slow values are static, so
        // no cycle bumps in the meantime.
        std::thread::sleep(Duration::from_millis(4000));
        assert_eq!(
            client.slow_generation(),
            generation,
            "stable slow content must not bump while parked"
        );
        let mut upd = TuningSettings::neutral("steering");
        upd.deadzone = 9.0;
        client.set_tuning(&upd).expect("set_tuning");
        assert!(client.discard_draft(), "discard must report a draft");
        let started = std::time::Instant::now();
        let mut waited = 0;
        while client.slow_generation() == generation && waited < 40 {
            std::thread::sleep(Duration::from_millis(50));
            waited += 1;
        }
        assert_ne!(
            client.slow_generation(),
            generation,
            "a write must pull the parked slow cycle forward (took {:?})",
            started.elapsed()
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "parked refresh took {:?} — the early break did not fire",
            started.elapsed()
        );
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
        assert!(
            seed.connect
                .as_ref()
                .is_ok_and(|info| matches!(info.state, ConnectionState::Active)),
            "sim session is Active"
        );
        assert_eq!(seed.backend, "vJoy");
        assert!(seed.autostart);
        assert!(
            !seed.wifi.password_set,
            "sim seed has no Wi-Fi password set"
        );
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
        // R3-D-15 (WS-4): the title is a human-readable headline,
        // never the raw code — the code lives only in `code`.
        assert_eq!(error.title, "Wheel not found");
        // The fake daemon sends an empty message (`"Unreachable: "`)
        // where the real server sends `"daemon API error:
        // Unreachable"` — both must parse to the same kind.
        let error = IpcApi::daemon_connect_error("Unreachable: ");
        assert_eq!(error.code, "Unreachable");
        // No-pairing redesign (protocol v2.2): `BadTag` died with
        // pairing — there is no device-key/tag step any more, so the
        // code is unrecognised and stays the generic link failure,
        // like any other unknown code.
        let error = IpcApi::daemon_connect_error("BadTag: daemon API error: BadTag");
        assert_eq!(error.code, "TransportClosed");
        let error = IpcApi::daemon_connect_error("AddrInUse: daemon API error: AddrInUse");
        assert_eq!(error.code, "AddrInUse");
        let error = IpcApi::daemon_connect_error("NoSuchCode: daemon API error: NoSuchCode");
        assert_eq!(
            error.code, "TransportClosed",
            "an unrecognised daemon code stays the generic link failure"
        );
    }

    #[test]
    fn daemon_backend_error_keeps_the_daemons_reason() {
        // Bug 4/8: a failed switch arrives as
        // `IpcError::Api("Unknown: daemon API error: Unknown:
        // <reason>")` — the daemon's reason must survive as the
        // detail (the toast shows it), not collapse into the
        // generic "daemon IPC link failed". The reason itself may
        // contain ": " — only the first split is the code.
        let error = IpcApi::daemon_backend_error(&IpcError::Api(
            "Unknown: daemon API error: Unknown: could not switch output backend to HIDMaestro: HIDMaestro could not connect: HIDMaestro sidecar failed to start.".to_string(),
        ));
        assert_eq!(error.code, "Unknown");
        assert_eq!(
            error.detail.as_deref(),
            Some("could not switch output backend to HIDMaestro: HIDMaestro could not connect: HIDMaestro sidecar failed to start."),
            "the full reason survives, colons included"
        );
        // A `not_admin` code rebuilds the typed UAC-decline error.
        let error = IpcApi::daemon_backend_error(&IpcError::Api(
            "not_admin: daemon API error: not_admin: HIDMaestro needs administrator rights — approve the UAC prompt to switch to it, or run RSH as administrator. vJoy needs no prompt.".to_string(),
        ));
        assert_eq!(error.code, "not_admin");
        assert_eq!(error.title, "Admin permission needed");
        assert!(
            error.what_happened.contains("administrator rights"),
            "{}",
            error.what_happened
        );
        assert!(
            error.what_to_do.contains("Approve the Windows prompt"),
            "{}",
            error.what_to_do
        );
        // A payload without a reason (fake daemon, empty message)
        // and a non-`Api` error (dead link) stay the generic link
        // failure — nothing is invented.
        let error = IpcApi::daemon_backend_error(&IpcError::Api(
            "Unknown: daemon API error: Unknown".to_string(),
        ));
        assert_eq!(error.code, "TransportClosed");
        let error = IpcApi::daemon_backend_error(&IpcError::Io("connection refused".to_string()));
        assert_eq!(error.code, "TransportClosed");
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
                board_reachable: false,
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

        fn wifi_status(&self) -> rsh_api::WifiStatus {
            rsh_api::WifiStatus {
                ssid: None,
                connected: false,
                password_set: false,
                board_mac: None,
            }
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
        /// R2-22 test hook: each `list_profiles` dispatch returns a
        /// profile whose name carries this counter, so every
        /// `slow_poll` cycle sees different content (and the
        /// generation bumps once per completed cycle).
        profiles_epoch: AtomicUsize,
        /// R2-22 test hook: `set_output_backend` sleeps this long
        /// before delegating — a stand-in for the 75 s backend
        /// switch (UAC prompt + sidecar spawn).
        backend_switch_delay: std::time::Duration,
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
                profiles_epoch: AtomicUsize::new(0),
                backend_switch_delay: std::time::Duration::ZERO,
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

        fn wifi_status(&self) -> WifiStatus {
            self.inner.wifi_status()
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

        // DS-80 test hook: a controllable blocking handler. R2-22:
        // every dispatch also returns a fresh profile name, so
        // consecutive `slow_poll` cycles see different content.
        fn list_profiles(&self) -> Vec<ProfileSummary> {
            if !self.list_profiles_delay.is_zero() {
                std::thread::sleep(self.list_profiles_delay);
            }
            let epoch = self.profiles_epoch.fetch_add(1, Ordering::Relaxed);
            vec![ProfileSummary {
                id: "counting".to_string(),
                name: format!("counting-{epoch}"),
                game: None,
                active: true,
            }]
        }

        // R2-22 test hook: a controllable long call — the backend
        // switch the slow poller must not stall behind.
        fn set_output_backend(&self, name: &str) -> Result<String, UserError> {
            if !self.backend_switch_delay.is_zero() {
                std::thread::sleep(self.backend_switch_delay);
            }
            self.inner.set_output_backend(name)
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

    /// R2-11: the slow generation bumps only when the slow content
    /// actually changed. Against the static `SimApi` the first
    /// cycle bumps once, then two more identical cycles bump
    /// nothing — before the fix the generation moved every cycle
    /// and the frame timer re-synced every page ~1 Hz.
    #[test]
    #[cfg(unix)]
    fn slow_generation_only_bumps_when_content_changes() {
        let (client, dir, handle, _guard) = live_client();
        assert!(
            wait_for(&client, IpcApi::slow_filled),
            "slow cache should fill"
        );
        let gen_first = client.slow_generation();
        assert!(gen_first > 0, "a completed cycle bumps the generation");
        // Two more full cycles on identical content.
        std::thread::sleep(Duration::from_millis(2200));
        assert_eq!(
            client.slow_generation(),
            gen_first,
            "identical slow cycles must not bump the generation"
        );
        drop(client);
        handle.join().expect("server thread");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// R2-21: the sequential `slow_poll` fallback is budgeted. A
    /// daemon that accepts the socket but never answers used to
    /// cost 30 sequential × 500 ms calls (~15 s) per cycle; now
    /// the pass aborts after 3 consecutive failures / 2 s and
    /// keeps its last good values.
    #[test]
    #[cfg(unix)]
    fn slow_fallback_aborts_within_budget_when_daemon_silent() {
        let dir = std::env::temp_dir().join(format!("rsh-fallback-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let sock = dir.join("rsh-daemon.sock");
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        rt.block_on(async {
            let listener = tokio::net::UnixListener::bind(&sock).expect("bind");
            // Accept and hold the stream without ever answering.
            tokio::spawn(async move {
                if let Ok((stream, _)) = listener.accept().await {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    drop(stream);
                }
            });
            let client = IpcClient::connect(&sock).await.expect("connect");
            let cache = Arc::new(LiveCache::default());
            let start = Instant::now();
            refresh_slow(&client, &cache).await;
            let elapsed = start.elapsed();
            // 500 ms (failed combined call) + 3 × 500 ms (failed
            // fetches up to the consecutive-failure cap) ≈ 2 s.
            assert!(
                elapsed >= Duration::from_millis(1500),
                "fallback ended suspiciously fast: {elapsed:?}"
            );
            assert!(
                elapsed < Duration::from_secs(5),
                "budgeted fallback must end in ~2.5 s, took {elapsed:?}"
            );
            assert!(
                cache.slow_filled.load(Ordering::Relaxed),
                "the aborted pass still marks the cache filled (last-good values)"
            );
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// R2-22: the slow poller runs on its own connection — while
    /// a 3 s backend switch (75 s bound, long-call lane) is in
    /// flight, the slow poller still completes cycles on its own
    /// lane and the generation advances. Without the dedicated
    /// lanes the switch would hold the shared client's
    /// single-flight lock for 3 s, the poller's `slow_poll`
    /// calls would time out behind it, and the generation would
    /// not move. (The counting server returns a fresh profile
    /// name per `slow_poll`, so every completed cycle changes
    /// the content and bumps. The 200 ms `list_profiles` delay
    /// keeps each `slow_poll` inside the 500 ms call timeout —
    /// a longer sleep would time the poller's own cycles out
    /// and prove nothing.)
    #[test]
    #[cfg(unix)]
    fn r2_22_slow_poller_keeps_refreshing_during_backend_switch() {
        let _guard = crate::live_probe::TEST_IPC_PIPE_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = std::env::temp_dir().join(format!("rsh-r2-22-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let sock = dir.join("rsh-daemon.sock");
        let mut counting = CountingApi::with_list_profiles_delay(Duration::from_millis(200));
        counting.backend_switch_delay = Duration::from_secs(3);
        let counting = Arc::new(counting);
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
                // Four connections: main + fast poller + slow
                // poller + long-call lane, served concurrently.
                let mut tasks = tokio::task::JoinSet::new();
                for _ in 0..4 {
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
        assert!(
            wait_for(&client, IpcApi::slow_filled),
            "slow cache should fill"
        );
        let gen_before = client.slow_generation();
        assert!(gen_before > 0, "a completed cycle bumps the generation");
        // Block the long-call lane for ~3 s from another thread:
        // a backend switch whose server-side handler sleeps,
        // holding the long lane's single-flight lock.
        let blocked = client.clone();
        let join = std::thread::spawn(move || {
            blocked
                .set_output_backend("vJoy")
                .expect("backend switch completes");
        });
        // The slow poller's ~1.2 s period (200 ms work + 1 s
        // wait) fits twice inside the 3 s switch — its
        // generation must move.
        let mut gen_after = gen_before;
        for _ in 0..120 {
            gen_after = client.slow_generation();
            if gen_after > gen_before {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            gen_after > gen_before,
            "slow poller stalled behind the backend switch: generation stayed {gen_before}"
        );
        join.join().expect("backend switch finishes");
        drop(client);
        handle.join().expect("server thread");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
