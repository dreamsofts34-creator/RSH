//! `WheelEngine` — the 200 Hz engine task (B1 / SW-R3.1).
//!
//! One `tokio` task owns the [`ProcessingPipeline`], the
//! [`SequenceTracker`], the [`Watchdog`], the [`PingTracker`] and the
//! [`EngineMetrics`], and ticks every 5 ms on a
//! [`tokio::time::interval`] (missed ticks are skipped, never caught
//! up in a burst). Each tick it:
//!
//! 1. drains status packets (see below) and ping replies;
//! 2. sends a ping request every second, if a [`PingSender`] is
//!    configured, and drops ping rounds older than 500 ms;
//! 3. reconnects the source every 2 s after a drop, if a
//!    [`SourceFactory`] is configured;
//! 4. drains the source, processing **every** queued frame in
//!    arrival order (H-1 multi-frame drain, bounded to
//!    `MAX_FRAMES_PER_TICK` frames per tick — frames past the cap
//!    stay queued for the next tick, never dropped);
//! 5. runs the sequence check (duplicates dropped silently, gaps
//!    counted), ticks the watchdog, runs the pipeline, and sends the
//!    output — or [`OutputState::neutral()`] when the watchdog
//!    reports stale data.
//!
//! The loop never dies: a failing sink, a dropped source, a duplicate
//! or out-of-order frame, or a stale watchdog all degrade to counted
//! events plus neutral output while the loop keeps ticking.
//!
//! # Status packets and ping replies — channels, not methods
//!
//! [`WheelEngine::run`] takes ownership of the engine, so a caller
//! cannot drive a `&mut self` method while the loop runs. Status
//! packets and ping replies therefore arrive on bounded `mpsc`
//! channels handed out before `run`: [`WheelEngine::status_sender`]
//! and [`WheelEngine::ping_reply_sender`]. The engine drains both
//! channels once per tick. A status packet updates the sequence
//! tracker's board counters (`seqGap`/`txFail`), the metrics, and the
//! latest `btnsEx` value (buttons 32–63), which the engine merges
//! into the [`OutputState::buttons_ex`] field — the pipeline itself
//! never sees status packets. A ping reply is fed to the
//! [`PingTracker`] and its measurement recorded in the metrics.
//!
//! # Stopping
//!
//! [`WheelEngine::stop`] (or [`StopHandle::stop`], obtained from
//! [`WheelEngine::stop_handle`] before `run` consumes the engine)
//! sets a `watch` flag. `run` notices it on the next tick at the
//! latest — even when the source is stuck forever, because the source
//! is only ever polled with a zero timeout and never awaited — and
//! returns an [`EngineReport`] with the final counters and metrics.
//! As a last act it makes a best-effort neutral send, so the output
//! is left parked, not frozen on the last live state.

use std::error::Error;
use std::fmt::{Display, Formatter};
use std::time::{Duration, Instant};

use rsh_protocol::ping::PingFrame;
use rsh_protocol::stream::{StatusPacket, StreamFrame};
use tokio::sync::{mpsc, watch};
use tokio::time::{MissedTickBehavior, interval, timeout};

use crate::metrics::EngineMetrics;
use crate::ping_client::PingTracker;
use crate::pipeline::{OutputState, PipelinePlan, ProcessingPipeline};
use crate::sequence::{SeqDecision, SequenceTracker};
use crate::watchdog::Watchdog;

/// Tick period: 5 ms = 200 Hz (SW-R3.1).
pub const TICK_INTERVAL: Duration = Duration::from_millis(5);
/// Reconnect cadence after the source drops: try again every 2 s.
pub const RECONNECT_INTERVAL: Duration = Duration::from_secs(2);
/// Ping cadence: one NTP-style ping request per second.
pub const PING_INTERVAL: Duration = Duration::from_secs(1);
/// Expected stream-frame spacing in µs (200 Hz), the baseline for
/// the inter-arrival jitter metric.
pub const EXPECTED_FRAME_INTERVAL_US: u32 = 5_000;

/// Windows timer-resolution guard: the 200 Hz loop ticks on a
/// `tokio::time::interval(5 ms)`, but Windows timers (and
/// `std::thread::sleep`) only resolve to the OS timer period —
/// ~15.6 ms by default — so the loop would tick at ~64 Hz instead
/// of 200 Hz (release dry run #6: idle rate measured 73 Hz on the
/// Windows runner and the `loop_hz > 150` assertion failed, while
/// the same test passes on Linux). While the engine runs, request
/// 1 ms resolution with `timeBeginPeriod(1)` (winmm — the standard
/// mechanism games and media apps use) and release it on drop with
/// the matching `timeEndPeriod(1)`. No-op off Windows.
#[cfg(windows)]
struct TimerResolutionGuard;
#[cfg(windows)]
impl TimerResolutionGuard {
    fn acquire() -> Self {
        // SAFETY: `timeBeginPeriod(1)` is a plain winmm call with a
        // valid period (1 ms is the minimum); the result only reports
        // failure, the call itself cannot fault. The matching
        // `timeEndPeriod(1)` runs in `Drop` below.
        let _ = unsafe { windows::Win32::Media::timeBeginPeriod(1) };
        Self
    }
}
#[cfg(windows)]
impl Drop for TimerResolutionGuard {
    fn drop(&mut self) {
        // SAFETY: paired with the `timeBeginPeriod(1)` in `acquire`.
        let _ = unsafe { windows::Win32::Media::timeEndPeriod(1) };
    }
}
#[cfg(not(windows))]
struct TimerResolutionGuard;
#[cfg(not(windows))]
impl TimerResolutionGuard {
    fn acquire() -> Self {
        Self
    }
}

/// Signed inter-arrival delta (µs) between two board timestamps
/// (`u32` µs, wraps every ~71.6 min).
///
/// A reordered arrival lands *behind* the previous timestamp, so
/// the wrapping `u32` delta is huge (near 2³²) although the real
/// jump is small and negative. Splitting at half the `u32` range
/// tells the two apart: a legitimate forward jump can never reach
/// it (the watchdog fires after 250 ms of silence), while a
/// reordered jump sits just below 2³². Reading the raw wrapping
/// delta as an unsigned gap recorded a near-2³² µs jitter
/// sample in `chaos-net` (PERF-H2.md §4.1); the signed reading
/// keeps it at the true ~15 000 µs.
#[must_use]
pub fn signed_timestamp_delta_us(new: u32, old: u32) -> i64 {
    let delta = new.wrapping_sub(old);
    if delta > u32::MAX / 2 {
        // Backward jump of (2³² − delta) µs.
        -i64::from(u32::MAX - delta + 1)
    } else {
        i64::from(delta)
    }
}

/// Inter-arrival jitter (µs) for one timestamp step: the distance
/// of the signed delta from the expected 5 ms spacing. Never
/// overflows: the signed delta keeps reordered arrivals small.
#[must_use]
pub fn inter_arrival_jitter_us(new: u32, old: u32) -> u64 {
    let delta = signed_timestamp_delta_us(new, old);
    delta.abs_diff(i64::from(EXPECTED_FRAME_INTERVAL_US))
}
/// Capacity of the status-packet channel (bounded; unbounded never).
const STATUS_CHANNEL_CAPACITY: usize = 16;
/// Capacity of the ping-reply channel (bounded; unbounded never).
const PING_REPLY_CHANNEL_CAPACITY: usize = 8;
/// Max stream frames drained (and processed) in one 5 ms tick (H-1:
/// multi-frame drain).
///
/// The nominal load is 1 frame/tick. A delayed tick can find 2+
/// frames queued; draining them all instead of keeping only the
/// latest is what removes the ~3.5% coalesced loss. The cap bounds
/// the worst-case tick work: 32 frames clear 160 ms of backlog per
/// tick, and frames past the cap stay queued for the next tick —
/// deferred, never dropped — so the cap cannot reintroduce
/// coalescing, it only bounds drain latency.
const MAX_FRAMES_PER_TICK: usize = 32;
/// Max status packets drained per tick (L-1: 2× the 16-slot channel
/// capacity — a racing producer cannot make one tick drain
/// unboundedly).
const MAX_STATUS_PER_TICK: usize = 2 * STATUS_CHANNEL_CAPACITY;
/// Max ping replies drained per tick (L-1: 2× the 8-slot channel
/// capacity).
const MAX_PING_REPLIES_PER_TICK: usize = 2 * PING_REPLY_CHANNEL_CAPACITY;

/// Source of decoded stream frames for the engine.
///
/// Implementations are polled once per tick with a zero timeout and
/// the poll future may be dropped while pending, so `next_frame`
/// must be cancellation-safe (a channel `recv` is; a future that
/// loses data when dropped is not).
pub trait PacketSource: Send {
    /// Await the next frame. Returning [`None`] means the source
    /// dropped or closed; the engine then retries a reconnect (when a
    /// [`SourceFactory`] is configured) or parks until stopped.
    ///
    /// Written as an `impl Future` return rather than `async fn`:
    /// the `Send` bound on the future is what keeps the engine
    /// `Send` (and therefore spawnable), and `async fn` in a trait
    /// cannot carry that bound.
    fn next_frame(&mut self) -> impl std::future::Future<Output = Option<StreamFrame>> + Send;

    /// Non-blocking drain of one queued status packet, if the source
    /// carries one (H1 condition 12: the status pump). The default
    /// returns [`None`] — sources without a status channel (the
    /// channel-backed test source) simply have no status to pump. UDP
    /// and serial transports override this with their status channel;
    /// the forwarder drains it after every stream frame so board loss
    /// counters and `btnsEx` reach the engine.
    fn try_next_status(&mut self) -> Option<StatusPacket> {
        None
    }
}

/// Builds a fresh [`PacketSource`] after the previous one dropped.
///
/// The engine calls this at most once per [`RECONNECT_INTERVAL`]
/// while it has no source. The call runs on a blocking thread
/// (L7: [`tokio::task::spawn_blocking`]), so it may open a serial
/// port or do discovery without blocking the 200 Hz tick — but it
/// must still be cancellation-safe in the sense that a panic in the
/// factory loses the factory and parks the engine.
pub trait SourceFactory: Send {
    /// The source type this factory builds. It must be the same type
    /// the engine was constructed with: [`PacketSource`] uses an
    /// `async fn`, which is not `dyn`-compatible on stable Rust, so
    /// the engine is generic over the source type and the factory
    /// names it here instead of boxing it.
    type Source: PacketSource;

    /// Build a fresh source. Returns [`None`] when no source is
    /// available yet; the engine retries after
    /// [`RECONNECT_INTERVAL`].
    fn create_source(&mut self) -> Option<Self::Source>;
}

/// Sink for processed output states (an output backend in Phases G,
/// a recording mock in tests).
///
/// `send` is synchronous: output calls have a sub-100 µs budget
/// (SW-R3.1) and must never block the 200 Hz loop.
pub trait OutputSink: Send {
    /// Send one output state to the backend.
    ///
    /// # Errors
    ///
    /// Returns [`SinkError`] when the backend refuses the state. The
    /// engine counts the error, falls back to neutral, and keeps
    /// ticking — a sink error never kills the loop.
    fn send(&mut self, state: &OutputState) -> Result<(), SinkError>;

    /// Send one output state with the engine's live per-tick metrics
    /// (H1 condition 11: the per-tick metrics outlet). The default
    /// implementation ignores the metrics and delegates to
    /// [`send`](Self::send), so existing sinks keep working; a sink
    /// that feeds snapshots overrides this to report real jitter /
    /// latency / loss p99s instead of "unmeasured".
    ///
    /// # Errors
    ///
    /// Same as [`send`](Self::send).
    fn send_with_metrics(
        &mut self,
        state: &OutputState,
        _metrics: &EngineMetrics,
    ) -> Result<(), SinkError> {
        self.send(state)
    }
}

/// Sends ping request frames toward the board.
///
/// Best-effort and non-blocking by contract: implementations must
/// never block the 200 Hz loop (a `try_send` on a bounded channel is
/// the intended shape). A ping that never leaves is indistinguishable
/// from a lost reply — the 500 ms timeout covers both, and there is
/// no retry.
pub trait PingSender: Send {
    /// Send one ping request frame toward the board.
    fn send_ping(&mut self, frame: PingFrame);
}

/// Failure from an [`OutputSink`]: the backend refused a state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SinkError {
    /// Short static description of the failure.
    pub message: &'static str,
}

impl SinkError {
    /// Build an error with a static message.
    #[must_use]
    pub const fn new(message: &'static str) -> Self {
        Self { message }
    }
}

impl Display for SinkError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "output sink error: {}", self.message)
    }
}

impl Error for SinkError {}

/// Loop-survival counters, exposed in the [`EngineReport`] that
/// [`WheelEngine::run`] returns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EngineCounters {
    /// Frames accepted by the sequence tracker and run through the
    /// pipeline.
    pub frames_processed: u64,
    /// Frames dropped as duplicates by the sequence tracker
    /// (retransmissions — dropped silently, never processed).
    pub frames_dropped_duplicate: u64,
    /// Failed [`OutputSink::send`] calls. Each failed send counts,
    /// including a failed neutral fallback send.
    pub sink_errors: u64,
    /// Reconnect attempts: one per [`SourceFactory::create_source`]
    /// call after the source dropped.
    pub source_reconnects: u64,
    /// Encoder glitch jumps dropped by the filter so far (M4).
    pub encoder_glitch_dropped: u64,
}

/// What [`WheelEngine::run`] returns when the loop stops: the final
/// counters, metrics, and sequence/ping tallies.
#[derive(Clone, Copy, Debug)]
pub struct EngineReport {
    /// Loop-survival counters.
    pub counters: EngineCounters,
    /// Final metrics snapshot (jitter / output-latency / E2E p99s,
    /// loss window, `seqGap`/`txFail` pass-through).
    pub metrics: EngineMetrics,
    /// Packets accepted by the sequence tracker.
    pub sequence_accepted: u64,
    /// Sequences skipped (sum of all gap counts).
    pub sequence_gaps: u64,
    /// Duplicate packets seen by the sequence tracker.
    pub sequence_duplicates: u64,
    /// Times the tracker rebaselined after a board reboot signature.
    pub sequence_rebaselines: u64,
    /// Ping rounds dropped by the 500 ms timeout.
    pub ping_timeouts: u64,
    /// E2E delay (µs) of the most recent accepted ping reply,
    /// [`None`] while unmeasured.
    pub ping_last_delay_us: Option<u64>,
}

/// Cloneable handle for stopping a running engine.
///
/// Obtained from [`WheelEngine::stop_handle`] before `run` consumes
/// the engine; [`WheelEngine::stop`] signals the same flag.
#[derive(Clone, Debug)]
pub struct StopHandle {
    /// Sender side of the stop `watch` channel.
    tx: watch::Sender<bool>,
}

impl StopHandle {
    /// Signal the engine to stop. `run` returns within one tick
    /// (well inside the ~5 s bound), even with a stuck source.
    pub fn stop(&self) {
        let _ = self.tx.send(true);
    }

    /// Whether a stop has been requested.
    #[must_use]
    pub fn is_stopping(&self) -> bool {
        *self.tx.borrow()
    }
}

/// The 200 Hz engine task.
///
/// Owns the pipeline, the sequence tracker, the watchdog, the ping
/// tracker and the metrics. Headless: no transport or output crate
/// is involved — the source, sink, factory and ping sender are all
/// injected traits, so the engine is testable with mocks.
///
/// Generic over the source type `S` because [`PacketSource`] has an
/// `async fn` and is therefore not `dyn`-compatible on stable Rust;
/// the sink, factory and ping sender are boxed traits (their methods
/// are synchronous and `dyn`-compatible).
pub struct WheelEngine<S: PacketSource> {
    /// Per-module processing pipeline.
    pipeline: ProcessingPipeline,
    /// Current frame source, [`None`] while parked after a drop.
    source: Option<S>,
    /// Factory for reconnects after a drop, [`None`] when the engine
    /// parks instead of reconnecting.
    factory: Option<Box<dyn SourceFactory<Source = S> + Send>>,
    /// Output backend.
    sink: Box<dyn OutputSink + Send>,
    /// Ping request sender, [`None`] when ping is disabled.
    ping_sender: Option<Box<dyn PingSender + Send>>,
    /// Reboot-safe sequence tracking.
    sequence: SequenceTracker,
    /// 250 ms stale watchdog.
    watchdog: Watchdog,
    /// NTP-style ping tracker.
    ping: PingTracker,
    /// Metrics (jitter / latency p99s, loss, status counters).
    metrics: EngineMetrics,
    /// Loop-survival counters.
    counters: EngineCounters,
    /// Last output sent (repeated on ticks with no new frame while
    /// the watchdog still reports fresh data).
    last_output: OutputState,
    /// Latest `btnsEx` (buttons 32–63) from a status packet, merged
    /// into every processed output.
    last_btns_ex: u32,
    /// `timestamp_us` of the last processed frame, for the
    /// inter-arrival jitter metric.
    last_frame_ts: Option<u32>,
    /// `uptime_ms` of the last status packet seen, for detecting a
    /// board uptime reset (L8 reboot evidence).
    last_status_uptime_ms: Option<u32>,
    /// A status packet since the last frame showed the board uptime
    /// jumping backwards (reboot evidence, L8); consumed by the next
    /// [`Self::handle_frame`].
    status_uptime_reset: bool,
    /// Monotonic clock origin for ping `t0` timestamps.
    started_at: Instant,
    /// Next instant a ping request is due (first ping immediately).
    next_ping_at: Instant,
    /// Next instant a reconnect may be attempted, [`None`] while a
    /// source is live or no retry is scheduled.
    next_reconnect_at: Option<Instant>,
    /// Receiver for status packets (see [`Self::status_sender`]).
    status_rx: mpsc::Receiver<StatusPacket>,
    /// Sender half of the status channel, cloned out to callers.
    status_tx: mpsc::Sender<StatusPacket>,
    /// Receiver for ping replies (see [`Self::ping_reply_sender`]).
    ping_rx: mpsc::Receiver<PingFrame>,
    /// Sender half of the ping-reply channel, cloned out to callers.
    ping_tx: mpsc::Sender<PingFrame>,
    /// Sender side of the stop `watch` channel.
    stop_tx: watch::Sender<bool>,
    /// Receiver side of the stop `watch` channel.
    stop_rx: watch::Receiver<bool>,
    /// Receiver for live pipeline plans (L4 pipeline push), [`None`]
    /// when no plan sender is attached. A `watch` channel carries
    /// only the latest plan, so a slider drag that sends ten plans
    /// between ticks applies exactly the newest one — no queue, no
    /// backlog, no stale plan.
    plan_rx: Option<watch::Receiver<PipelinePlan>>,
}

impl<S: PacketSource + 'static> WheelEngine<S> {
    /// Build an engine from a pipeline, a frame source and an output
    /// sink. Ping is disabled and there is no reconnect factory until
    /// [`Self::with_ping_sender`] / [`Self::with_source_factory`]
    /// add them.
    #[must_use]
    pub fn new(pipeline: ProcessingPipeline, source: S, sink: Box<dyn OutputSink + Send>) -> Self {
        let (status_tx, status_rx) = mpsc::channel(STATUS_CHANNEL_CAPACITY);
        let (ping_tx, ping_rx) = mpsc::channel(PING_REPLY_CHANNEL_CAPACITY);
        let (stop_tx, stop_rx) = watch::channel(false);
        let now = Instant::now();
        Self {
            pipeline,
            source: Some(source),
            factory: None,
            sink,
            ping_sender: None,
            sequence: SequenceTracker::new(),
            watchdog: Watchdog::new(),
            ping: PingTracker::new(),
            metrics: EngineMetrics::new(),
            counters: EngineCounters::default(),
            last_output: OutputState::neutral(),
            last_btns_ex: 0,
            last_frame_ts: None,
            last_status_uptime_ms: None,
            status_uptime_reset: false,
            started_at: now,
            next_ping_at: now,
            next_reconnect_at: None,
            status_rx,
            status_tx,
            ping_rx,
            ping_tx,
            stop_tx,
            stop_rx,
            plan_rx: None,
        }
    }

    /// Attach a live-plan receiver (L4 pipeline push): the newest
    /// plan on the channel is applied at the start of the next tick
    /// via `set_plan_live` (no filter/ramp reset), so tuning sliders
    /// take effect live without stepping the output.
    #[must_use]
    pub fn with_plan_receiver(mut self, rx: watch::Receiver<PipelinePlan>) -> Self {
        self.plan_rx = Some(rx);
        self
    }

    /// Add a factory for reconnects after the source drops (at most
    /// one attempt per [`RECONNECT_INTERVAL`]). Without a factory the
    /// engine parks after a drop: it keeps ticking and sending
    /// neutral until stopped.
    #[must_use]
    pub fn with_source_factory(
        mut self,
        factory: Box<dyn SourceFactory<Source = S> + Send>,
    ) -> Self {
        self.factory = Some(factory);
        self
    }

    /// Add a sender for the 1 Hz ping requests. Without one, no ping
    /// rounds are started and E2E latency stays unmeasured.
    #[must_use]
    pub fn with_ping_sender(mut self, sender: Box<dyn PingSender + Send>) -> Self {
        self.ping_sender = Some(sender);
        self
    }

    /// Sender for board status packets. The caller (transport, Phase
    /// C) sends decoded [`StatusPacket`]s here; the engine drains
    /// them once per tick and updates the board loss counters, the
    /// metrics, and the `btnsEx` merge value.
    #[must_use]
    pub fn status_sender(&self) -> mpsc::Sender<StatusPacket> {
        self.status_tx.clone()
    }

    /// Sender for ping reply frames. The caller (transport, Phase C)
    /// sends decoded [`PingFrame`] replies here; the engine drains
    /// them once per tick and feeds them to the [`PingTracker`].
    #[must_use]
    pub fn ping_reply_sender(&self) -> mpsc::Sender<PingFrame> {
        self.ping_tx.clone()
    }

    /// Cloneable stop handle for use after `run` consumes the engine
    /// (e.g. from the task that spawned it).
    #[must_use]
    pub fn stop_handle(&self) -> StopHandle {
        StopHandle {
            tx: self.stop_tx.clone(),
        }
    }

    /// Signal the engine to stop. `run` returns within one tick —
    /// well inside the ~5 s bound — even when the source is stuck.
    /// Only usable before `run` consumes the engine; after that, use
    /// the [`StopHandle`] from [`Self::stop_handle`].
    pub fn stop(&self) {
        let _ = self.stop_tx.send(true);
    }

    /// Run the 200 Hz loop until stopped, then return the final
    /// [`EngineReport`]. Never panics on bad input and never dies on
    /// a source/sink error: errors are counted, output goes neutral,
    /// and the loop keeps ticking.
    pub async fn run(mut self) -> EngineReport {
        // Windows timer resolution (see `TimerResolutionGuard`):
        // without it the 5 ms interval ticks at ~64 Hz there.
        let _timer_resolution = TimerResolutionGuard::acquire();
        let mut ticker = interval(TICK_INTERVAL);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        if !*self.stop_rx.borrow() {
            loop {
                tokio::select! {
                    _ = ticker.tick() => self.on_tick().await,
                    result = self.stop_rx.changed() => {
                        match result {
                            Ok(()) => {
                                if *self.stop_rx.borrow() {
                                    break;
                                }
                            }
                            // Stop sender dropped: treat as a stop.
                            Err(_) => break,
                        }
                    }
                }
            }
        }
        self.finish()
    }

    /// One 5 ms tick: status/ping drains, ping send, reconnect, drain
    /// every queued frame, process them in arrival order, and send
    /// the output (neutral when the watchdog reports stale data).
    /// Only one output is sent per tick — the state of the last
    /// processed frame — so the output rate stays 200 Hz even when
    /// a delayed tick drains several frames.
    async fn on_tick(&mut self) {
        let now = Instant::now();
        self.drain_plan();
        self.drain_status();
        self.drain_ping_replies(now);
        self.maybe_send_ping(now);
        self.maybe_reconnect(now).await;
        let frames = self.drain_source().await;
        let had_frame = !frames.is_empty();
        let started = Instant::now();
        let mut out = if self.watchdog.is_stale(now) {
            OutputState::neutral()
        } else {
            self.last_output
        };
        for frame in &frames {
            out = self.handle_frame(frame, now);
        }
        let out = if self.watchdog.is_stale(now) {
            OutputState::neutral()
        } else {
            out
        };
        if self.send_output(&out) && had_frame {
            self.metrics
                .record_output_latency(duration_us(started.elapsed()));
        }
    }

    /// Drain a pending live plan, if any (L4 pipeline push): the
    /// `watch` channel holds only the latest plan, so this applies
    /// exactly one swap per tick, before any frame is processed —
    /// never mid-frame. A dropped sender is ignored (the last plan
    /// stays active); a closed channel never stops the tick.
    fn drain_plan(&mut self) {
        let Some(rx) = self.plan_rx.as_mut() else {
            return;
        };
        if rx.has_changed().unwrap_or(false) {
            let plan = *rx.borrow_and_update();
            self.pipeline.set_plan_live(plan);
        }
    }

    /// Drain all queued status packets: board loss counters, metrics
    /// pass-through, and the `btnsEx` merge value. Also watches the
    /// board uptime: a backward jump of more than 1 s is a reboot
    /// (uptime only ever increases, apart from the ~49-day `u32`
    /// wrap, which is a jump of ~2³² ms — far past the threshold too),
    /// and arms the rebaseline evidence for the next frame (L8).
    fn drain_status(&mut self) {
        let mut drained = 0;
        while drained < MAX_STATUS_PER_TICK {
            let Ok(status) = self.status_rx.try_recv() else {
                break;
            };
            drained += 1;
            if let Some(last) = self.last_status_uptime_ms
                && last.wrapping_sub(status.uptime_ms) > 1_000
            {
                self.status_uptime_reset = true;
            }
            self.last_status_uptime_ms = Some(status.uptime_ms);
            self.sequence.ingest_status(&status);
            self.metrics.update_status_counters(
                self.sequence.board_seq_gap(),
                self.sequence.board_tx_fail(),
            );
            if let Some(btns_ex) = status.btns_ex {
                self.last_btns_ex = btns_ex;
            }
        }
    }

    /// Drain all queued ping replies into the tracker; accepted
    /// measurements feed the E2E metric. Late or stale replies are
    /// ignored by the tracker and never measured.
    fn drain_ping_replies(&mut self, now: Instant) {
        let now_us = monotonic_us(self.started_at, now);
        let mut drained = 0;
        while drained < MAX_PING_REPLIES_PER_TICK {
            let Ok(frame) = self.ping_rx.try_recv() else {
                break;
            };
            drained += 1;
            if let Some(measurement) = self.ping.on_reply(frame, now_us) {
                self.metrics.record_ping_measurement(&measurement);
            }
        }
    }

    /// Drop a ping round older than 500 ms (no retry) and, on the 1 s
    /// cadence, start a new round through the [`PingSender`].
    fn maybe_send_ping(&mut self, now: Instant) {
        let now_us = monotonic_us(self.started_at, now);
        let _ = self.ping.check_timeout(now_us);
        let Some(sender) = self.ping_sender.as_mut() else {
            return;
        };
        if now < self.next_ping_at {
            return;
        }
        self.next_ping_at = now + PING_INTERVAL;
        let request = self.ping.next_request(now_us);
        sender.send_ping(request);
    }

    /// If the source is gone and a reconnect is due, ask the factory
    /// for a fresh source. With no factory the engine stays parked:
    /// it keeps ticking and sending neutral until stopped.
    ///
    /// L7: the factory call runs on a blocking thread
    /// ([`tokio::task::spawn_blocking`]) — `create_source` may open a
    /// serial port or do discovery, which must never block the 200 Hz
    /// tick. The factory is taken out of the engine for the call and
    /// restored afterwards; if the blocking task panics the factory is
    /// lost and the engine parks (a retry is still scheduled, but with
    /// no factory it stays parked — a panicking factory must not wedge
    /// the loop into a spawn storm).
    async fn maybe_reconnect(&mut self, now: Instant) {
        if self.source.is_some() {
            return;
        }
        let Some(due) = self.next_reconnect_at else {
            return;
        };
        if now < due {
            return;
        }
        let Some(mut factory) = self.factory.take() else {
            return;
        };
        self.counters.source_reconnects += 1;
        let spawned = tokio::task::spawn_blocking(move || {
            let source = factory.create_source();
            (factory, source)
        })
        .await;
        match spawned {
            Ok((factory, Some(source))) => {
                self.factory = Some(factory);
                self.source = Some(source);
                self.next_reconnect_at = None;
            }
            Ok((factory, None)) => {
                self.factory = Some(factory);
                self.next_reconnect_at = Some(now + RECONNECT_INTERVAL);
            }
            Err(_) => {
                self.next_reconnect_at = Some(now + RECONNECT_INTERVAL);
            }
        }
    }

    /// Drain the source, returning every frame queued right now, in
    /// arrival order, up to `MAX_FRAMES_PER_TICK` (H-1 multi-frame
    /// drain). Polls are zero-timeout, so a stuck source can never
    /// block the tick; a `None` marks the source dropped and schedules
    /// a reconnect. Frames already collected are still returned when
    /// the source drops mid-drain, and frames past the cap stay
    /// queued for the next tick — deferred, never dropped.
    async fn drain_source(&mut self) -> Vec<StreamFrame> {
        let mut frames = Vec::new();
        let mut dropped = false;
        while frames.len() < MAX_FRAMES_PER_TICK {
            let polled = {
                let Some(source) = self.source.as_mut() else {
                    break;
                };
                timeout(Duration::ZERO, source.next_frame()).await
            };
            match polled {
                Ok(Some(frame)) => frames.push(frame),
                Ok(None) => {
                    dropped = true;
                    break;
                }
                // Nothing ready right now: the tick moves on.
                Err(_) => break,
            }
        }
        if dropped {
            self.source = None;
            self.next_reconnect_at = Some(Instant::now() + RECONNECT_INTERVAL);
        }
        frames
    }

    /// Handle one drained frame: watchdog tick, jitter sample, the
    /// sequence decision, and the pipeline. Duplicates are dropped
    /// silently (the current output is repeated); gaps are counted
    /// in the metrics loss window and processing continues.
    ///
    /// L1: jitter is sampled only for accepted frames — a duplicate
    /// retransmission's timestamp is stale by definition and must not
    /// pollute the jitter p99. A gap is recorded as one batch
    /// ([`EngineMetrics::record_gap`]), not a per-packet loop.
    /// L8: a large backward sequence jump rebaselines only with
    /// reboot evidence — a stale watchdog (no frame for ≥ 250 ms) or
    /// a status-packet uptime reset since the last frame.
    fn handle_frame(&mut self, frame: &StreamFrame, now: Instant) -> OutputState {
        // Reboot evidence is read before the watchdog tick: a frame
        // arriving after ≥ 250 ms of silence is evidence the board
        // may have restarted.
        let reboot_evidence = self.watchdog.is_stale(now) || self.status_uptime_reset;
        self.status_uptime_reset = false;
        // Any frame arrival proves the link is alive, even a
        // duplicate retransmission.
        self.watchdog.tick(now);
        let decision = self
            .sequence
            .check_with_evidence(frame.seq, reboot_evidence);
        if decision == SeqDecision::Duplicate {
            self.counters.frames_dropped_duplicate += 1;
            return self.last_output;
        }
        if let Some(prev_ts) = self.last_frame_ts {
            self.metrics
                .record_inter_arrival_jitter(inter_arrival_jitter_us(frame.timestamp_us, prev_ts));
        }
        self.last_frame_ts = Some(frame.timestamp_us);
        let skipped = match decision {
            SeqDecision::Gap(skipped) => {
                self.metrics.record_gap(skipped);
                skipped
            }
            SeqDecision::Accept => {
                self.metrics.record_packet(true);
                0
            }
            SeqDecision::Duplicate => unreachable!("duplicates returned above"),
        };
        let mut out = self.pipeline.process_with_gap(frame, skipped);
        out.buttons_ex = self.last_btns_ex;
        self.counters.frames_processed += 1;
        self.counters.encoder_glitch_dropped = self.pipeline.encoder_glitch_dropped();
        self.last_output = out;
        out
    }

    /// Send one output state. On failure the error is counted and a
    /// neutral state is attempted instead (unless the state already
    /// is neutral). Returns `true` when at least one send succeeded.
    fn send_output(&mut self, state: &OutputState) -> bool {
        if self.sink.send_with_metrics(state, &self.metrics).is_ok() {
            return true;
        }
        self.counters.sink_errors += 1;
        if *state == OutputState::neutral() {
            return false;
        }
        if self
            .sink
            .send_with_metrics(&OutputState::neutral(), &self.metrics)
            .is_ok()
        {
            true
        } else {
            self.counters.sink_errors += 1;
            false
        }
    }

    /// Best-effort final neutral send, then the report. The final
    /// send is not counted in `sink_errors`: the loop is over, and a
    /// failing backend at shutdown must not rewrite the run's
    /// counters.
    fn finish(&mut self) -> EngineReport {
        let _ = self.sink.send(&OutputState::neutral());
        self.report()
    }

    /// Snapshot the final counters, metrics, and tallies.
    fn report(&self) -> EngineReport {
        EngineReport {
            counters: self.counters,
            metrics: self.metrics,
            sequence_accepted: self.sequence.accepted(),
            sequence_gaps: self.sequence.gaps(),
            sequence_duplicates: self.sequence.duplicates(),
            sequence_rebaselines: self.sequence.rebaselines(),
            ping_timeouts: self.ping.timeouts(),
            ping_last_delay_us: self.ping.last_delay_us(),
        }
    }
}

/// Microseconds from `start` to `now`, saturating at `u64::MAX` on
/// overflow (an `Instant` pair can never realistically overflow, but
/// the metric must never panic).
fn monotonic_us(start: Instant, now: Instant) -> u64 {
    duration_us(now.saturating_duration_since(start))
}

/// Microseconds in a [`Duration`], saturating at `u64::MAX`.
fn duration_us(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    };

    use rsh_protocol::ping::PingFrame;
    use rsh_protocol::stream::StatusPacket;
    use rsh_protocol::{StreamFlags, StreamFrame};
    use tokio::sync::mpsc;
    use tokio::task::JoinHandle;
    use tokio::time::{Duration, sleep, timeout};

    use super::*;
    use crate::pipeline::PipelinePlan;

    /// Channel-backed source: `next_frame` awaits the channel and
    /// returns [`None`] once every sender is dropped.
    struct ChannelSource {
        /// Frame channel.
        rx: mpsc::Receiver<StreamFrame>,
    }

    impl ChannelSource {
        /// Wrap a receiver.
        fn new(rx: mpsc::Receiver<StreamFrame>) -> Self {
            Self { rx }
        }
    }

    impl PacketSource for ChannelSource {
        fn next_frame(&mut self) -> impl std::future::Future<Output = Option<StreamFrame>> + Send {
            self.rx.recv()
        }
    }

    /// A source that never yields a frame (a stuck transport).
    struct StuckSource;

    impl PacketSource for StuckSource {
        fn next_frame(&mut self) -> impl std::future::Future<Output = Option<StreamFrame>> + Send {
            std::future::pending()
        }
    }

    /// Sink recording every state it is offered — including attempts
    /// that fail — so tests can observe neutral fallbacks.
    struct RecordingSink {
        /// Recorded states, in send order.
        states: Arc<Mutex<Vec<OutputState>>>,
        /// When true, every `send` fails.
        fail: bool,
    }

    impl RecordingSink {
        /// Build a failing or succeeding sink plus its shared record.
        fn new(fail: bool) -> (Self, Arc<Mutex<Vec<OutputState>>>) {
            let states = Arc::new(Mutex::new(Vec::new()));
            (
                Self {
                    states: Arc::clone(&states),
                    fail,
                },
                states,
            )
        }
    }

    impl OutputSink for RecordingSink {
        fn send(&mut self, state: &OutputState) -> Result<(), SinkError> {
            {
                let mut guard = self.states.lock().expect("sink states lock");
                guard.push(*state);
            }
            if self.fail {
                Err(SinkError::new("mock sink failure"))
            } else {
                Ok(())
            }
        }
    }

    /// Ping sender recording every request it is offered.
    struct RecordingPingSender {
        /// Recorded requests, in send order.
        sent: Arc<Mutex<Vec<PingFrame>>>,
    }

    impl RecordingPingSender {
        /// Build the sender plus its shared record.
        fn new() -> (Self, Arc<Mutex<Vec<PingFrame>>>) {
            let sent = Arc::new(Mutex::new(Vec::new()));
            (
                Self {
                    sent: Arc::clone(&sent),
                },
                sent,
            )
        }
    }

    impl PingSender for RecordingPingSender {
        fn send_ping(&mut self, frame: PingFrame) {
            let mut guard = self.sent.lock().expect("ping sent lock");
            guard.push(frame);
        }
    }

    /// Factory handing out a fresh channel source per call and keeping
    /// the senders, so the test can feed frames to the reconnected
    /// source.
    struct ChannelFactory {
        /// Reconnect attempts so far.
        attempts: Arc<AtomicU64>,
        /// Senders for the sources handed out, in creation order.
        senders: Arc<Mutex<Vec<mpsc::Sender<StreamFrame>>>>,
    }

    impl ChannelFactory {
        /// Build the factory.
        fn new() -> Self {
            Self {
                attempts: Arc::new(AtomicU64::new(0)),
                senders: Arc::new(Mutex::new(Vec::new())),
            }
        }

        /// Shared reconnect-attempt counter.
        fn attempts(&self) -> Arc<AtomicU64> {
            Arc::clone(&self.attempts)
        }

        /// Shared record of the senders handed out, in creation order.
        fn senders(&self) -> Arc<Mutex<Vec<mpsc::Sender<StreamFrame>>>> {
            Arc::clone(&self.senders)
        }
    }

    impl SourceFactory for ChannelFactory {
        type Source = ChannelSource;

        fn create_source(&mut self) -> Option<Self::Source> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            let (tx, rx) = mpsc::channel(32);
            self.senders.lock().expect("factory senders lock").push(tx);
            Some(ChannelSource::new(rx))
        }
    }

    /// A frame with steering on slot 0 (3300 mV = full right) and the
    /// given sequence and board timestamp.
    fn frame(seq: u16, timestamp_us: u32, steer_mv: u16) -> StreamFrame {
        StreamFrame::new(
            seq,
            timestamp_us,
            StreamFlags::default(),
            &[(0, steer_mv)],
            &[],
            0,
        )
    }

    /// A status packet carrying only the loss counters (and optional
    /// `btnsEx`) the engine consumes.
    fn status(seq_gap: u16, tx_fail: u16, btns_ex: Option<u32>) -> StatusPacket {
        StatusPacket {
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
            seq_gap,
            tx_fail,
            btns_ex,
        }
    }

    /// Snapshot of the states a [`RecordingSink`] has been offered.
    fn recorded(states: &Mutex<Vec<OutputState>>) -> Vec<OutputState> {
        states.lock().expect("read recorded states").clone()
    }

    /// Running engine plus the handles the test drives it with.
    struct Harness {
        /// The spawned `run` task.
        join: JoinHandle<EngineReport>,
        /// Stop handle for the running engine.
        stop: StopHandle,
        /// States the sink was offered, in send order.
        states: Arc<Mutex<Vec<OutputState>>>,
        /// Feed frames to the engine's source through this.
        tx: mpsc::Sender<StreamFrame>,
        /// Feed status packets to the engine through this.
        status_tx: mpsc::Sender<StatusPacket>,
    }

    /// Build an engine on a channel source and spawn it.
    fn start(fail_sink: bool) -> Harness {
        let (tx, rx) = mpsc::channel(32);
        let (sink, states) = RecordingSink::new(fail_sink);
        let engine = WheelEngine::new(
            ProcessingPipeline::new(PipelinePlan::default()),
            ChannelSource::new(rx),
            Box::new(sink),
        );
        let status_tx = engine.status_sender();
        let stop = engine.stop_handle();
        let join = tokio::spawn(engine.run());
        Harness {
            join,
            stop,
            states,
            tx,
            status_tx,
        }
    }

    /// Stop the engine and await its report (fails the test if `run`
    /// does not return within 5 s).
    async fn stop_and_report(harness: &mut Harness) -> EngineReport {
        harness.stop.stop();
        timeout(Duration::from_secs(5), &mut harness.join)
            .await
            .expect("run() returns within 5 s of stop()")
            .expect("engine task joins cleanly")
    }

    #[test]
    fn engine_is_send() {
        /// Compile-time assertion: the engine must be spawnable.
        fn assert_send<T: Send>() {}
        assert_send::<WheelEngine<ChannelSource>>();
    }

    #[tokio::test]
    async fn happy_path_processes_frames_and_sends_output() {
        let mut harness = start(false);
        for (seq, ts) in [(0, 0), (1, 5_000), (2, 10_000)] {
            harness
                .tx
                .send(frame(seq, ts, 3300))
                .await
                .expect("source channel open");
            sleep(Duration::from_millis(50)).await;
        }
        let report = stop_and_report(&mut harness).await;
        assert_eq!(report.counters.frames_processed, 3);
        assert_eq!(report.counters.frames_dropped_duplicate, 0);
        assert_eq!(report.counters.sink_errors, 0);
        // Board timestamps 5000 µs apart: jitter samples are exactly 0.
        assert_eq!(report.metrics.jitter_p99(), Some(0));
        assert!(report.metrics.output_latency_p99().is_some());
        let states = recorded(&harness.states);
        assert!(states.iter().any(|s| s.steering > 0.9), "{states:?}");
    }

    #[tokio::test]
    async fn plan_receiver_applies_latest_plan_before_next_tick() {
        // L4 pipeline push: a plan sent on the watch channel takes
        // effect on the next tick, before the frame is processed —
        // and only the latest plan applies when several are sent
        // between ticks.
        use tokio::sync::watch;
        let (tx, rx) = mpsc::channel(32);
        let (sink, states) = RecordingSink::new(false);
        let (plan_tx, plan_rx) = watch::channel(PipelinePlan::default());
        let mut engine = WheelEngine::new(
            ProcessingPipeline::new(PipelinePlan::default()),
            ChannelSource::new(rx),
            Box::new(sink),
        )
        .with_plan_receiver(plan_rx);
        // Steering at 2550 mV maps to +0.5; the default deadzone
        // (0.02) leaves ~0.49.
        tx.send(frame(0, 0, 2550)).await.expect("source open");
        engine.on_tick().await;
        let first = recorded(&states).into_iter().last().expect("one output");
        assert!(first.steering > 0.4, "default plan passes: {first:?}");
        // Two rapid pushes: only the latest (deadzone 0.5) may apply.
        let mut mid = PipelinePlan::default();
        mid.steering.deadzone = 0.1;
        plan_tx.send(mid).expect("plan channel open");
        let mut latest = PipelinePlan::default();
        latest.steering.deadzone = 0.5;
        plan_tx.send(latest).expect("plan channel open");
        tx.send(frame(1, 5_000, 2550)).await.expect("source open");
        engine.on_tick().await;
        // The new plan zeroes the raw value, but the smoothing filter
        // decays from its previous state instead of jumping — drive
        // a dozen more ticks at the same input so it settles near 0.
        for (seq, ts) in (2..=13_u16).map(|s| (s, u32::from(s) * 5_000)) {
            tx.send(frame(seq, ts, 2550)).await.expect("source open");
            engine.on_tick().await;
        }
        let second = recorded(&states).into_iter().last().expect("outputs");
        assert!(
            second.steering.abs() < 0.05,
            "latest plan (deadzone 0.5) swallows +0.5: {second:?}"
        );
    }

    #[tokio::test]
    async fn multi_frame_drain_processes_every_queued_frame() {
        // H-1 regression: three frames queued before a single tick
        // must ALL be processed — the old latest-wins drain kept only
        // the last one and the other two counted as coalesced gaps.
        // Driving `on_tick` directly (no ticker) makes the queueing
        // deterministic: all three frames are waiting when the tick runs.
        let (tx, rx) = mpsc::channel(32);
        let (sink, states) = RecordingSink::new(false);
        let mut engine = WheelEngine::new(
            ProcessingPipeline::new(PipelinePlan::default()),
            ChannelSource::new(rx),
            Box::new(sink),
        );
        for (seq, ts) in [(0, 0), (1, 5_000), (2, 10_000)] {
            tx.send(frame(seq, ts, 3300))
                .await
                .expect("source channel open");
        }
        engine.on_tick().await;
        let report = engine.report();
        assert_eq!(report.counters.frames_processed, 3);
        assert_eq!(report.sequence_gaps, 0);
        assert_eq!(report.counters.frames_dropped_duplicate, 0);
        // One output per tick, from the LAST frame (steering full right).
        let states = recorded(&states);
        assert_eq!(states.len(), 1, "one send per tick: {states:?}");
        assert!(states[0].steering > 0.9, "{states:?}");
    }

    #[tokio::test]
    async fn drain_source_is_bounded_and_defers_the_rest() {
        // H-1 bound: one tick drains at most MAX_FRAMES_PER_TICK
        // frames; the rest stay queued for the next tick — deferred,
        // never dropped.
        let (tx, rx) = mpsc::channel(64);
        let (sink, _states) = RecordingSink::new(false);
        let mut engine = WheelEngine::new(
            ProcessingPipeline::new(PipelinePlan::default()),
            ChannelSource::new(rx),
            Box::new(sink),
        );
        let cap_u16 = u16::try_from(MAX_FRAMES_PER_TICK).expect("cap fits u16");
        for seq in 0..(cap_u16 + 8) {
            tx.send(frame(seq, u32::from(seq) * 5_000, 3300))
                .await
                .expect("source channel open");
        }
        let first = engine.drain_source().await;
        assert_eq!(first.len(), MAX_FRAMES_PER_TICK);
        assert_eq!(first.first().map(|f| f.seq), Some(0));
        assert_eq!(first.last().map(|f| f.seq), Some(cap_u16 - 1));
        let rest = engine.drain_source().await;
        assert_eq!(rest.len(), 8);
        assert_eq!(rest.first().map(|f| f.seq), Some(cap_u16));
    }

    #[test]
    fn signed_timestamp_delta_handles_reorder_and_wrap() {
        assert_eq!(signed_timestamp_delta_us(5_000, 0), 5_000);
        assert_eq!(inter_arrival_jitter_us(5_000, 0), 0);
        // Reordered arrival: 5000 µs behind the previous timestamp.
        assert_eq!(signed_timestamp_delta_us(5_000, 10_000), -5_000);
        assert_eq!(inter_arrival_jitter_us(5_000, 10_000), 10_000);
        // One missed tick: 10 000 µs spacing is 5 000 µs of jitter.
        assert_eq!(inter_arrival_jitter_us(15_000, 5_000), 5_000);
        // u32 timestamp wrap: 4_294_967_290 -> 10 is +16, not ~2³².
        assert_eq!(signed_timestamp_delta_us(10, 4_294_967_290), 16);
        assert_eq!(inter_arrival_jitter_us(10, 4_294_967_290), 4_984);
    }

    #[tokio::test]
    async fn reordered_frame_does_not_overflow_jitter_p99() {
        // Regression (chaos-net, PERF-H2.md §4.1): a reordered
        // arrival lands behind the previous board timestamp, and
        // the old unsigned reading of the wrapping delta recorded
        // a ~4_294_957_295 µs jitter sample. The signed reading
        // must keep it at the true value.
        let mut harness = start(false);
        for (seq, ts) in [(0, 0), (1, 5_000), (2, 10_000)] {
            harness
                .tx
                .send(frame(seq, ts, 3300))
                .await
                .expect("source channel open");
            sleep(Duration::from_millis(50)).await;
        }
        // Retransmission of seq 1, timestamp 5000 — 5000 µs behind
        // the previous frame. L1: jitter is recorded only for
        // accepted frames, so this duplicate yields NO sample.
        harness
            .tx
            .send(frame(1, 5_000, 3300))
            .await
            .expect("source channel open");
        sleep(Duration::from_millis(50)).await;
        let report = stop_and_report(&mut harness).await;
        assert_eq!(report.counters.frames_dropped_duplicate, 1);
        // Samples: 0, 0 — the duplicate contributed nothing.
        // Nearest-rank p99 is 0, never ~2³² and never 10_000.
        assert_eq!(report.metrics.jitter_p99(), Some(0));
    }

    #[tokio::test]
    async fn gap_records_loss_batch_and_duplicate_skips_jitter() {
        // L1: seq 0 then seq 5 (4 skipped) records 4 lost + 1
        // received in one batch (loss 4/6); the duplicate of seq 5
        // with a reordered timestamp adds no jitter sample.
        let mut harness = start(false);
        harness
            .tx
            .send(frame(0, 0, 3300))
            .await
            .expect("send seq 0");
        sleep(Duration::from_millis(50)).await;
        harness
            .tx
            .send(frame(5, 25_000, 3300))
            .await
            .expect("send seq 5");
        sleep(Duration::from_millis(50)).await;
        harness
            .tx
            .send(frame(5, 20_000, 3300))
            .await
            .expect("send duplicate seq 5");
        sleep(Duration::from_millis(50)).await;
        let report = stop_and_report(&mut harness).await;
        assert_eq!(report.counters.frames_processed, 2);
        assert_eq!(report.counters.frames_dropped_duplicate, 1);
        assert_eq!(report.sequence_gaps, 4);
        assert!((report.metrics.loss_fraction() - 4.0 / 6.0).abs() < f64::EPSILON);
        // Jitter samples: only the accepted seq-5 frame, |25000-0-5000|.
        assert_eq!(report.metrics.jitter_p99(), Some(20_000));
    }

    #[tokio::test]
    async fn large_backward_jump_after_silence_rebaselines() {
        // L8: seq 1000, 1001, then silence > 250 ms (stale watchdog),
        // then seq 0 — the board rebooted, so seq 0 accepts and
        // rebaselines instead of counting as a duplicate.
        let mut harness = start(false);
        harness
            .tx
            .send(frame(1000, 0, 3300))
            .await
            .expect("send seq 1000");
        sleep(Duration::from_millis(50)).await;
        harness
            .tx
            .send(frame(1001, 5_000, 3300))
            .await
            .expect("send seq 1001");
        sleep(Duration::from_millis(50)).await;
        sleep(Duration::from_millis(300)).await; // watchdog goes stale
        harness
            .tx
            .send(frame(0, 10_000, 3300))
            .await
            .expect("send seq 0 after reboot");
        sleep(Duration::from_millis(50)).await;
        let report = stop_and_report(&mut harness).await;
        assert_eq!(report.counters.frames_processed, 3);
        assert_eq!(report.counters.frames_dropped_duplicate, 0);
        assert_eq!(report.sequence_rebaselines, 1);
    }

    #[tokio::test]
    async fn large_backward_jump_without_silence_is_duplicate() {
        // L8 without evidence: seq 0 right after seq 1001 (no
        // silence, no uptime reset) is a duplicate, not a reboot.
        let mut harness = start(false);
        harness
            .tx
            .send(frame(1000, 0, 3300))
            .await
            .expect("send seq 1000");
        sleep(Duration::from_millis(50)).await;
        harness
            .tx
            .send(frame(1001, 5_000, 3300))
            .await
            .expect("send seq 1001");
        sleep(Duration::from_millis(50)).await;
        harness
            .tx
            .send(frame(0, 10_000, 3300))
            .await
            .expect("send seq 0");
        sleep(Duration::from_millis(50)).await;
        let report = stop_and_report(&mut harness).await;
        assert_eq!(report.counters.frames_processed, 2);
        assert_eq!(report.counters.frames_dropped_duplicate, 1);
        assert_eq!(report.sequence_rebaselines, 0);
    }

    /// Sink recording the per-tick metrics the engine offers (H1
    /// condition 11: the per-tick metrics outlet). Records the
    /// jitter p99 seen on each tick.
    struct MetricsRecordingSink {
        /// Jitter p99 observed per tick (`None` = unmeasured yet).
        seen_jitter_p99: Arc<Mutex<Vec<Option<u64>>>>,
    }

    impl OutputSink for MetricsRecordingSink {
        fn send(&mut self, _state: &OutputState) -> Result<(), SinkError> {
            Ok(())
        }

        fn send_with_metrics(
            &mut self,
            _state: &OutputState,
            metrics: &EngineMetrics,
        ) -> Result<(), SinkError> {
            self.seen_jitter_p99
                .lock()
                .expect("metrics record lock")
                .push(metrics.jitter_p99());
            Ok(())
        }
    }

    #[tokio::test]
    async fn per_tick_metrics_outlet_reaches_the_sink() {
        let (tx, rx) = mpsc::channel(32);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = MetricsRecordingSink {
            seen_jitter_p99: Arc::clone(&seen),
        };
        let engine = WheelEngine::new(
            ProcessingPipeline::new(PipelinePlan::default()),
            ChannelSource::new(rx),
            Box::new(sink),
        );
        let stop = engine.stop_handle();
        let join = tokio::spawn(engine.run());
        for (seq, ts) in [(0, 0), (1, 5_000), (2, 10_000)] {
            tx.send(frame(seq, ts, 3300))
                .await
                .expect("source channel open");
            sleep(Duration::from_millis(50)).await;
        }
        stop.stop();
        timeout(Duration::from_secs(5), join)
            .await
            .expect("run() returns within 5 s of stop()")
            .expect("engine task joins cleanly");
        // Board timestamps 5000 µs apart: once two frames have
        // arrived the jitter samples are exactly 0, and the sink must
        // have observed that live — not "unmeasured" forever.
        let seen = seen.lock().expect("read recorded metrics");
        assert!(
            seen.contains(&Some(0)),
            "sink never observed a measured jitter p99: {seen:?}"
        );
    }

    #[tokio::test]
    async fn duplicate_frames_dropped_silently() {
        let mut harness = start(false);
        harness
            .tx
            .send(frame(5, 0, 3300))
            .await
            .expect("source channel open");
        sleep(Duration::from_millis(60)).await;
        // Same sequence again: a retransmission, dropped silently.
        harness
            .tx
            .send(frame(5, 5_000, 3300))
            .await
            .expect("source channel open");
        sleep(Duration::from_millis(60)).await;
        harness
            .tx
            .send(frame(6, 10_000, 3300))
            .await
            .expect("source channel open");
        sleep(Duration::from_millis(60)).await;
        let report = stop_and_report(&mut harness).await;
        assert_eq!(report.counters.frames_processed, 2);
        assert_eq!(report.counters.frames_dropped_duplicate, 1);
        assert_eq!(report.sequence_duplicates, 1);
    }

    #[tokio::test]
    async fn gap_counted_and_loop_continues() {
        let mut harness = start(false);
        harness
            .tx
            .send(frame(10, 0, 3300))
            .await
            .expect("source channel open");
        sleep(Duration::from_millis(60)).await;
        // Four sequences skipped before seq 15.
        harness
            .tx
            .send(frame(15, 5_000, 3300))
            .await
            .expect("source channel open");
        sleep(Duration::from_millis(60)).await;
        let report = stop_and_report(&mut harness).await;
        assert_eq!(report.counters.frames_processed, 2);
        assert_eq!(report.sequence_gaps, 4);
        assert!(report.metrics.loss_fraction() > 0.0);
    }

    #[tokio::test]
    async fn malformed_and_duplicate_frames_do_not_kill_loop() {
        // "Malformed" at engine level is what survives parsing: a
        // duplicate, an empty frame (no axes at all), and a wild
        // sequence jump. None of them may kill the loop.
        let mut harness = start(false);
        harness
            .tx
            .send(frame(1, 0, 3300))
            .await
            .expect("source channel open");
        sleep(Duration::from_millis(60)).await;
        harness
            .tx
            .send(frame(1, 5_000, 3300))
            .await
            .expect("duplicate accepted by channel");
        sleep(Duration::from_millis(60)).await;
        let empty = StreamFrame::new(2, 10_000, StreamFlags::default(), &[], &[], 0);
        harness.tx.send(empty).await.expect("channel open");
        sleep(Duration::from_millis(60)).await;
        harness
            .tx
            .send(frame(900, 15_000, 3300))
            .await
            .expect("channel open");
        sleep(Duration::from_millis(60)).await;
        assert!(!harness.join.is_finished(), "loop still alive");
        let report = stop_and_report(&mut harness).await;
        assert_eq!(report.counters.frames_processed, 3);
        assert_eq!(report.counters.frames_dropped_duplicate, 1);
        assert_eq!(report.sequence_gaps, 897);
    }

    #[tokio::test]
    async fn transport_drop_reconnects_and_loop_stays_alive() {
        let (tx, rx) = mpsc::channel(32);
        let (sink, states) = RecordingSink::new(false);
        let factory = ChannelFactory::new();
        let attempts = factory.attempts();
        let senders = factory.senders();
        let engine = WheelEngine::new(
            ProcessingPipeline::new(PipelinePlan::default()),
            ChannelSource::new(rx),
            Box::new(sink),
        )
        .with_source_factory(Box::new(factory));
        let stop = engine.stop_handle();
        let join = tokio::spawn(engine.run());
        tx.send(frame(0, 0, 3300)).await.expect("channel open");
        sleep(Duration::from_millis(60)).await;
        // Drop the transport: the only sender goes away.
        drop(tx);
        assert!(!join.is_finished(), "loop alive right after drop");
        // Reconnect fires 2 s after the drop; give it time, then feed
        // a frame through the reconnected source.
        sleep(Duration::from_millis(2300)).await;
        assert!(attempts.load(Ordering::SeqCst) >= 1, "reconnect attempted");
        let fresh_tx = {
            let guard = senders.lock().expect("factory senders lock");
            guard.last().expect("factory handed out a sender").clone()
        };
        fresh_tx
            .send(frame(1, 5_000, 3300))
            .await
            .expect("reconnected source accepts frames");
        sleep(Duration::from_millis(100)).await;
        assert!(!join.is_finished(), "loop alive after reconnect");
        stop.stop();
        let report = timeout(Duration::from_secs(5), join)
            .await
            .expect("run() returns after stop()")
            .expect("engine task joins cleanly");
        assert!(report.counters.source_reconnects >= 1);
        assert_eq!(report.counters.frames_processed, 2);
        assert!(
            recorded(&states).iter().any(|s| s.steering > 0.9),
            "post-reconnect frame reached the sink"
        );
    }

    #[tokio::test]
    async fn no_factory_parks_but_keeps_ticking_neutral() {
        let (tx, rx) = mpsc::channel(32);
        let (sink, states) = RecordingSink::new(false);
        let engine = WheelEngine::new(
            ProcessingPipeline::new(PipelinePlan::default()),
            ChannelSource::new(rx),
            Box::new(sink),
        );
        let stop = engine.stop_handle();
        let join = tokio::spawn(engine.run());
        tx.send(frame(0, 0, 3300)).await.expect("channel open");
        sleep(Duration::from_millis(60)).await;
        drop(tx);
        // No factory: no reconnect, but the loop keeps ticking and,
        // once the watchdog goes stale, keeps sending neutral.
        sleep(Duration::from_millis(600)).await;
        assert!(!join.is_finished(), "loop alive while parked");
        let recorded_states = recorded(&states);
        assert!(
            recorded_states.iter().any(|s| *s == OutputState::neutral()),
            "parked engine sends neutral"
        );
        stop.stop();
        let report = timeout(Duration::from_secs(5), join)
            .await
            .expect("run() returns after stop()")
            .expect("engine task joins cleanly");
        assert_eq!(report.counters.source_reconnects, 0);
    }

    #[tokio::test]
    async fn failing_sink_loop_survives_and_counts_errors() {
        let mut harness = start(true);
        harness
            .tx
            .send(frame(0, 0, 3300))
            .await
            .expect("source channel open");
        sleep(Duration::from_millis(300)).await;
        assert!(!harness.join.is_finished(), "loop alive with failing sink");
        let report = stop_and_report(&mut harness).await;
        assert!(report.counters.frames_processed >= 1);
        assert!(report.counters.sink_errors > 0, "sink errors counted");
        // The engine fell back to neutral on every failed send, and
        // the recording sink observed those attempts.
        let states = recorded(&harness.states);
        assert!(
            states.iter().any(|s| *s == OutputState::neutral()),
            "neutral fallback attempted on sink failure"
        );
    }

    #[tokio::test]
    async fn stop_while_source_stuck_returns_promptly() {
        let (sink, _states) = RecordingSink::new(false);
        let engine = WheelEngine::new(
            ProcessingPipeline::new(PipelinePlan::default()),
            StuckSource,
            Box::new(sink),
        );
        let stop = engine.stop_handle();
        let join = tokio::spawn(engine.run());
        sleep(Duration::from_millis(100)).await;
        let started = Instant::now();
        stop.stop();
        let report = timeout(Duration::from_secs(5), join)
            .await
            .expect("run() returns within 5 s of stop() with a stuck source")
            .expect("engine task joins cleanly");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "stop bounded: {:?}",
            started.elapsed()
        );
        assert_eq!(report.counters.frames_processed, 0);
    }

    #[tokio::test]
    async fn stop_before_run_returns_immediately() {
        let (tx, rx) = mpsc::channel(32);
        drop(tx);
        let (sink, _states) = RecordingSink::new(false);
        let engine = WheelEngine::new(
            ProcessingPipeline::new(PipelinePlan::default()),
            ChannelSource::new(rx),
            Box::new(sink),
        );
        engine.stop();
        let report = timeout(Duration::from_secs(5), Box::pin(engine.run()))
            .await
            .expect("run() returns promptly when already stopped");
        assert_eq!(report.counters.frames_processed, 0);
    }

    #[tokio::test]
    async fn watchdog_stale_sends_neutral() {
        let mut harness = start(false);
        harness
            .tx
            .send(frame(0, 0, 3300))
            .await
            .expect("source channel open");
        sleep(Duration::from_millis(100)).await;
        // One live frame, then silence: after 250 ms the watchdog is
        // stale and every tick sends neutral.
        sleep(Duration::from_millis(500)).await;
        let states = recorded(&harness.states);
        assert!(
            states.iter().any(|s| *s == OutputState::neutral()),
            "neutral observed while stale, before stop: {states:?}"
        );
        assert!(
            states.iter().any(|s| s.steering > 0.9),
            "live frame was sent before going stale"
        );
        let report = stop_and_report(&mut harness).await;
        assert_eq!(report.counters.frames_processed, 1);
    }

    #[tokio::test]
    async fn ping_request_sent_each_second() {
        let (tx, rx) = mpsc::channel(32);
        let (sink, _states) = RecordingSink::new(false);
        let (ping_sender, sent) = RecordingPingSender::new();
        let engine = WheelEngine::new(
            ProcessingPipeline::new(PipelinePlan::default()),
            ChannelSource::new(rx),
            Box::new(sink),
        )
        .with_ping_sender(Box::new(ping_sender));
        let stop = engine.stop_handle();
        let join = tokio::spawn(engine.run());
        drop(tx);
        sleep(Duration::from_millis(2200)).await;
        stop.stop();
        let report = timeout(Duration::from_secs(5), join)
            .await
            .expect("run() returns after stop()")
            .expect("engine task joins cleanly");
        let sent = sent.lock().expect("ping sent lock");
        assert!(sent.len() >= 2, "pings at 0 s, 1 s, 2 s: {}", sent.len());
        assert_eq!(sent[0].seq, 0);
        assert_eq!(sent[1].seq, 1);
        // No replies: every round times out, E2E stays unmeasured.
        assert!(report.ping_timeouts >= 1);
        assert_eq!(report.ping_last_delay_us, None);
        assert_eq!(report.metrics.e2e_latency_p99(), None);
    }

    #[tokio::test]
    async fn ping_reply_feeds_e2e_metric() {
        let (tx, rx) = mpsc::channel(32);
        let (sink, _states) = RecordingSink::new(false);
        let (ping_sender, sent) = RecordingPingSender::new();
        let engine = WheelEngine::new(
            ProcessingPipeline::new(PipelinePlan::default()),
            ChannelSource::new(rx),
            Box::new(sink),
        )
        .with_ping_sender(Box::new(ping_sender));
        let reply_tx = engine.ping_reply_sender();
        let stop = engine.stop_handle();
        let join = tokio::spawn(engine.run());
        drop(tx);
        sleep(Duration::from_millis(150)).await;
        let request = {
            let guard = sent.lock().expect("ping sent lock");
            *guard.first().expect("a ping request was sent")
        };
        // Board answers 100 µs after receiving, 50 µs processing.
        let reply = PingFrame::reply(request.seq, request.t0, request.t0 + 100, request.t0 + 150);
        reply_tx.send(reply).await.expect("reply channel open");
        sleep(Duration::from_millis(150)).await;
        stop.stop();
        let report = timeout(Duration::from_secs(5), join)
            .await
            .expect("run() returns after stop()")
            .expect("engine task joins cleanly");
        let delay = report
            .ping_last_delay_us
            .expect("reply measured an E2E delay");
        assert!(
            delay > 50_000,
            "delay reflects the ~150 ms round trip: {delay}"
        );
        assert_eq!(report.metrics.e2e_latency_p99(), Some(delay));
    }

    #[tokio::test]
    async fn status_packet_updates_metrics_and_btns_ex() {
        let mut harness = start(false);
        // Status first: seq_gap 7, tx_fail 3, buttons 32–63 word.
        harness
            .status_tx
            .send(status(7, 3, Some(0x0000_0005)))
            .await
            .expect("status channel open");
        sleep(Duration::from_millis(60)).await;
        harness
            .tx
            .send(frame(0, 0, 3300))
            .await
            .expect("source channel open");
        sleep(Duration::from_millis(60)).await;
        let report = stop_and_report(&mut harness).await;
        assert_eq!(report.metrics.seq_gap, 7);
        assert_eq!(report.metrics.tx_fail, 3);
        let states = recorded(&harness.states);
        assert!(
            states.iter().any(|s| s.buttons_ex == 0x0000_0005),
            "btnsEx merged into the output: {states:?}"
        );
    }
}
