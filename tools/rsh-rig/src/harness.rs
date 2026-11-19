//! Rig harness: drives the real `WheelEngine` at 200 Hz from the
//! virtual wheel, through the real wire parser, into the recording
//! output sink — plus the contract-only self-checks.
//!
//! Topology (all in-process; the sandbox blocks UDP `sendto`, Phase G
//! condition 5, so there is no socket in this loop — the wheel hands
//! wire bytes to the transport task over a channel, and the transport
//! parses them with the real `StreamFrame::parse`):
//!
//! ```text
//! VirtualWheel --wire bytes--> transport task --StreamFrame-->
//!   bounded mpsc queue (256, drop-newest on full — the product
//!   forwarder's shape) --> QueuedSource --> WheelEngine -->
//!   RecordingSink (wheel also answers pings via PingBridge, and
//!   sends 1 Hz status)
//! ```
//!
//! # Multi-frame drain support (H-1)
//!
//! The handoff is a QUEUE, not a latest-wins `watch` channel: every
//! parsed frame is enqueued (up to 256; overflows are counted as
//! `queueDropped`, never silently lost), and the engine's
//! `drain_source` polls it repeatedly with a zero timeout —
//! draining up to `MAX_FRAMES_PER_TICK` (32) frames per tick in
//! arrival order and processing every one of them (deep-scan
//! Round 2, R2-ENGINE). Frames past the cap stay queued for the
//! next tick (deferred, never dropped); the engine emits one output
//! per tick (200 Hz). `coalescedPercent` (≤ 0.5% asserted on soak /
//! long-soak) therefore measures rig-side watch-channel coalescing
//! only, never engine drops, and the `emit-200hz` scenario shows
//! the true 200 Hz rate.

//!
//! Metrics use the C# `metrics.json` key names (`TESTING-INPUT.md`
//! §8.3). Axis metrics are recorded in `vJoy` units (0..32767) so the
//! C# thresholds apply unchanged. `engineTicks` is the sink's
//! successful-send count (the engine sends once per tick, plus one
//! final neutral send in `finish()` — noted, not hidden).
//! `watchdogActivations` is measured, not assumed: a silence episode
//! (≥ 300 ms with no frame arrival at the transport) counts only if
//! a neutral send falls inside it (≥ 250 ms after the last arrival).
//!
//! # What `lossPercent` and `tickJitterP99Us` measure (Phase H fix)
//!
//! The product's forwarder queues frames on a bounded `mpsc` (256,
//! drop-newest on full) and the engine drains up to
//! `MAX_FRAMES_PER_TICK` (32) frames per 5 ms tick in arrival order,
//! processing every one (`state.rs` `forward_with_retry` +
//! `engine.rs` `drain_source`). There is no latest-wins collapse
//! left at the engine boundary: the engine processes every drained
//! frame, so `coalescedPercent` is expected to be ~0% on a healthy
//! run (verified 0% on soak after the Round 2 drain fix); a nonzero
//! value means frames that arrived but were genuinely never
//! processed (queue-overflow drops, counted as `queueDropped`, or
//! restart-rebaseline artefacts), reported separately, never hidden.
//! So the engine's sequence gaps conflate TWO different things, and
//! the SLOs are about only one of them:
//!
//! * `lossPercent` — TRUE TRANSPORT LOSS: sequence gaps counted at
//!   ARRIVAL, in the transport task, on parsed frames, with the
//!   engine's own [`SequenceTracker`] rules (wrap, duplicate,
//!   reboot rebaseline) so the two counters are directly comparable.
//!   A frame that never arrives (net loss, bad CRC, channel drop)
//!   is lost; a frame that arrives but the engine never processes
//!   is NOT transport loss (it is `coalesced`, reported separately).
//!   The SLO (`< 0.5% Wi-Fi`) is about the
//!   transport, so it is measured at the transport.
//! * `coalescedFrames` / `coalescedPercent` — frames that DID arrive
//!   but the engine never processed (engine gaps minus arrival gaps).
//!   Reported separately in every run, never hidden: on a healthy
//!   run this is 0% (the multi-frame drain processes every frame);
//!   a nonzero value is a real signal, not a measurement artefact.
//!   `engineLossPercent` / `engineSequenceGaps` keep the old
//!   engine-side numbers for comparison.
//! * `tickJitterP99Us` — PC-RECEIVE inter-arrival jitter p99: the
//!   wall-clock spacing between consecutive frame ARRIVALS at the
//!   transport, vs the 5000 µs nominal — the basis the plan's SLO is
//!   defined on ("Packet inter-arrival jitter (PC receive) p99 ≤
//!   1 ms", `FEATURE-INVENTORY.md`). The engine's board-timestamp
//!   jitter (`engineTickJitterP99Us`, kept as an informational
//!   metric) is NOT the SLO metric: the board clock drifts 100 ppm,
//!   so every double-step (one coalesced frame) measures exactly
//!   5001 µs (`|10001 − 5000|`), which made the soak fail by 1 µs
//!   deterministically in every run — a measurement artefact, not VM
//!   noise and not a product defect. The board-timestamp value is
//!   still recorded, not hidden.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use rsh_benchmarks::alloc::net_live_bytes;
use rsh_engine::engine::{
    EngineReport, OutputSink, PacketSource, PingSender, SinkError, WheelEngine,
};
use rsh_engine::pipeline::{OutputState, PipelinePlan, ProcessingPipeline, ShifterGateMode};
use rsh_engine::sequence::SequenceTracker;
use rsh_protocol::control::{ControlFrame, DuplicateCache, ReplayTracker, build_frame};
use rsh_protocol::error::RejectCounters;
use rsh_protocol::ping::{PingFrame, measure};
use rsh_protocol::stream::{StatusPacket, StreamFrame};
use tokio::sync::{Mutex as AsyncMutex, mpsc};

use crate::metrics::{JitterHistogram, Metrics, Sampler};
use crate::scenarios::{HarnessConfig, Scenario};
use crate::wheel::{TEST_KEY, TEST_NONCE, VirtualWheel, WheelSummary};

/// Lock a mutex, recovering from poisoning (a rig must not die
/// because a task panicked while holding a metrics lock).
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Wire messages from the wheel task to the transport task.
enum WireMsg {
    /// Encoded stream frame (or fuzz bytes).
    Stream(Vec<u8>),
    /// 1 Hz status packet.
    Status(StatusPacket),
    /// Ping reply (already delayed by the wheel task).
    PingReply(PingFrame),
}

/// Shared recording-sink state.
#[derive(Debug, Default)]
struct SinkState {
    /// Successful sends (== engine ticks, +1 final neutral in finish).
    update_calls: u64,
    /// Sends of the all-neutral state.
    neutral_sends: u64,
    /// Timestamps of neutral sends (capped; watchdog proxy).
    neutral_times: Vec<Instant>,
    /// Axis extremes in `vJoy` units (0..32767), non-neutral sends only.
    steering_min: i64,
    steering_max: i64,
    accel_min: i64,
    accel_max: i64,
    brake_min: i64,
    brake_max: i64,
    clutch_min: i64,
    clutch_max: i64,
    handbrake_min: i64,
    handbrake_max: i64,
    /// OR of all observed button words.
    button_mask_or: u32,
    /// Bit `g` set when gear `g` was observed.
    gear_mask_seen: u32,
    /// Largest |encoder delta| observed in one tick.
    max_abs_encoder_delta: i64,
    /// Distinct button words seen (capped at 256).
    distinct_masks: Vec<u32>,
}

impl SinkState {
    /// Fresh state (extremes start inverted so "no data" is visible).
    /// Buffers are pre-allocated BEFORE the run's `reset_counts()`:
    /// `neutral_times` (8192 Instants ≈ 41 s of continuous neutral
    /// output) and `distinct_masks` (256) must not grow inside the
    /// measured window, or the rig's `allocBytesDelta` measures its
    /// own bookkeeping instead of the product (M-1). A 4-hour soak
    /// with a stuck watchdog would pass 8192 neutral sends — pushes
    /// past the cap are dropped (the count is still exact).
    fn new() -> Self {
        Self {
            steering_min: i64::MAX,
            steering_max: i64::MIN,
            accel_min: i64::MAX,
            accel_max: i64::MIN,
            brake_min: i64::MAX,
            brake_max: i64::MIN,
            clutch_min: i64::MAX,
            clutch_max: i64::MIN,
            handbrake_min: i64::MAX,
            handbrake_max: i64::MIN,
            neutral_times: Vec::with_capacity(8192),
            distinct_masks: Vec::with_capacity(256),
            ..Self::default()
        }
    }
}

/// Recording output sink (the C# `RecordingOutput` equivalent):
/// aggregates only — no per-tick storage, so a 4-hour soak stays
/// memory-bounded (§8.3).
#[derive(Debug)]
struct RecordingSink {
    shared: Arc<Mutex<SinkState>>,
    /// When set, sends fail until this instant (fault-output-drop).
    drop_until: Arc<Mutex<Option<Instant>>>,
}

impl RecordingSink {
    /// Scale a -1..=1 axis to `vJoy` units 0..32767.
    fn scale_bipolar(v: f32) -> i64 {
        (f64::from(v).midpoint(1.0) * 32767.0).round() as i64
    }

    /// Scale a 0..=1 axis to `vJoy` units 0..32767.
    fn scale_unipolar(v: f32) -> i64 {
        (f64::from(v) * 32767.0).round() as i64
    }
}

impl OutputSink for RecordingSink {
    fn send(&mut self, state: &OutputState) -> Result<(), SinkError> {
        if let Some(until) = *lock(&self.drop_until)
            && Instant::now() < until
        {
            return Err(SinkError::new("rig output drop"));
        }
        let mut s = lock(&self.shared);
        s.update_calls += 1;
        if *state == OutputState::neutral() {
            s.neutral_sends += 1;
            // Capped at the pre-allocated capacity (see `SinkState::new`):
            // pushing past it would grow the buffer inside the measured
            // window (M-1). Watchdog detection only needs neutral times
            // inside silence episodes, which are seconds long.
            if s.neutral_times.len() < 8192 {
                s.neutral_times.push(Instant::now());
            }
            return Ok(());
        }
        // Axis extremes (non-neutral sends only).
        let steer = Self::scale_bipolar(state.steering);
        s.steering_min = s.steering_min.min(steer);
        s.steering_max = s.steering_max.max(steer);
        let accel = Self::scale_unipolar(state.accelerator);
        s.accel_min = s.accel_min.min(accel);
        s.accel_max = s.accel_max.max(accel);
        let brake = Self::scale_unipolar(state.brake);
        s.brake_min = s.brake_min.min(brake);
        s.brake_max = s.brake_max.max(brake);
        let clutch = Self::scale_unipolar(state.clutch);
        s.clutch_min = s.clutch_min.min(clutch);
        s.clutch_max = s.clutch_max.max(clutch);
        let handbrake = Self::scale_unipolar(state.handbrake);
        s.handbrake_min = s.handbrake_min.min(handbrake);
        s.handbrake_max = s.handbrake_max.max(handbrake);
        s.button_mask_or |= state.buttons;
        if state.gear != 0 {
            s.gear_mask_seen |= 1u32 << state.gear;
        }
        for d in state.encoder_deltas {
            let a = i64::from(d).abs();
            if a > s.max_abs_encoder_delta {
                s.max_abs_encoder_delta = a;
            }
        }
        if !s.distinct_masks.contains(&state.buttons) && s.distinct_masks.len() < 256 {
            s.distinct_masks.push(state.buttons);
        }
        Ok(())
    }
}

/// Depth of the transport→engine frame queue (H-1 drain support):
/// live depth + high-water mark + frames dropped on a full queue.
/// The transport increments on every successful enqueue, the source
/// decrements on every dequeue; a drop is counted, never silent.
#[derive(Debug, Default)]
struct QueueDepth {
    current: AtomicUsize,
    max: AtomicUsize,
    dropped: AtomicU64,
}

impl QueueDepth {
    fn on_enqueue(&self) {
        let cur = self.current.fetch_add(1, Ordering::Relaxed) + 1;
        self.max.fetch_max(cur, Ordering::Relaxed);
    }

    fn on_dequeue(&self) {
        self.current.fetch_sub(1, Ordering::Relaxed);
    }

    fn on_drop(&self) {
        self.dropped.fetch_add(1, Ordering::Relaxed);
    }
}

/// Enqueue one parsed frame for the engine, counting it in the queue
/// depth. The count MUST happen-before the frame becomes visible to
/// the consumer: the old `try_send` + count-after let the engine
/// dequeue the frame in between, wrapping `current` to `usize::MAX`
/// when the queue was empty — the next enqueue's `+ 1` then panics in
/// debug and kills the transport task (CI rig-gate + local repro:
/// 26/27, soak/fault-reboot failing on the dead transport). Reserving
/// the permit first acquires the slot WITHOUT publishing the frame, so
/// the count lands before any dequeue can observe it; `Permit::send`
/// publishes afterwards. A failed reserve (full / closed) is a counted
/// drop, exactly as before.
fn enqueue_frame(tx: &mpsc::Sender<StreamFrame>, depth: &QueueDepth, frame: StreamFrame) {
    match tx.try_reserve() {
        Ok(permit) => {
            depth.on_enqueue();
            permit.send(frame);
        }
        Err(_) => depth.on_drop(),
    }
}

/// Queue capacity: mirrors the product forwarder's bounded `mpsc`
/// (256, drop-newest on full — see the module docs). The engine
/// drains every 5 ms and the wheel emits every 5.1 ms, so a healthy
/// run holds ~0–1 frames; a full queue means the engine is stuck,
/// and the drop count says so.
const FRAME_QUEUE_CAPACITY: usize = 256;

/// `PacketSource` over the bounded mpsc queue: the transport
/// enqueues every parsed frame; the engine drains them in arrival
/// order, up to `MAX_FRAMES_PER_TICK` (32) per tick (H-1 multi-frame
/// drain, deep-scan Round 2). Unlike the old `watch` (latest-wins)
/// handoff, no frame is collapsed anywhere — frames past the cap
/// stay queued for the next tick (deferred, never dropped), and the
/// engine processes every drained frame but emits one output per
/// tick (200 Hz). `coalescedPercent` (engine gaps minus arrival gaps)
/// is therefore expected to be 0% on a healthy run; a nonzero value
/// means frames the engine never processed, never a hidden drop.
/// The receiver lives behind an async `Mutex` shared by both engine
/// legs: on restart the new leg reuses the same queue and resumes
/// with the frames that arrived mid-restart instead of losing them.
struct QueuedSource {
    rx: Arc<AsyncMutex<mpsc::Receiver<StreamFrame>>>,
    depth: Arc<QueueDepth>,
}

impl PacketSource for QueuedSource {
    async fn next_frame(&mut self) -> Option<StreamFrame> {
        // The guard is a tokio `MutexGuard`: it may be held across the
        // `recv().await` (async-aware mutex, never a blocking wait),
        // and the engine's zero-timeout poll drops this future — and
        // the guard — as soon as the tick moves on (M-12: no
        // `std::sync::Mutex` is ever held across an `.await` in this
        // file; the `std` locks below are only ever held across
        // synchronous code).
        let mut rx = self.rx.lock().await;
        let frame = rx.recv().await;
        if frame.is_some() {
            self.depth.on_dequeue();
        }
        frame
    }
}

/// Shared ping state between the engine-side sender half and the
/// reply half (the engine owns the sender half; the transport owns
/// the reply half).
#[derive(Debug)]
struct PingShared {
    /// seq -> (t0 in engine-epoch µs, wall time the request left).
    pending: HashMap<u16, (u64, Instant)>,
    /// Measured E2E delays (µs), one per accepted reply.
    samples: Vec<u64>,
}

impl Default for PingShared {
    /// Pre-allocated BEFORE the run's `reset_counts()`: the 200 s
    /// `ntp-ping` run holds ~201 samples, so 512 never grows inside
    /// the measured window (M-1); `pending` holds ≤ a few entries
    /// (1 Hz ping, 500 ms timeout) and 16 is ample.
    fn default() -> Self {
        Self {
            pending: HashMap::with_capacity(16),
            samples: Vec::with_capacity(512),
        }
    }
}

/// Engine-side half: implements `PingSender`, forwards requests to
/// the wheel and records the send time for the delay math.
#[derive(Debug)]
struct PingSenderHalf {
    shared: Arc<Mutex<PingShared>>,
    to_wheel: mpsc::Sender<PingFrame>,
}

impl PingSender for PingSenderHalf {
    fn send_ping(&mut self, frame: PingFrame) {
        let mut st = lock(&self.shared);
        st.pending
            .retain(|_, (_, sent)| sent.elapsed() < Duration::from_millis(500));
        st.pending.insert(frame.seq, (frame.t0, Instant::now()));
        let _ = self.to_wheel.try_send(frame);
    }
}

/// Transport-side half: matches replies to requests, measures the
/// E2E delay with the real `measure()` formula, and forwards the
/// reply to the engine's ping channel.
#[derive(Debug)]
struct PingReplyHalf {
    shared: Arc<Mutex<PingShared>>,
    /// Current leg's ping-reply sender (swapped on engine restart).
    to_engine: Arc<Mutex<Option<mpsc::Sender<PingFrame>>>>,
}

impl PingReplyHalf {
    /// Handle one wheel reply: measure it, then forward it.
    fn on_reply(&self, reply: PingFrame) {
        {
            let mut st = lock(&self.shared);
            let Some((t0, sent)) = st.pending.remove(&reply.seq) else {
                return;
            };
            let elapsed_us = u64::try_from(sent.elapsed().as_micros()).unwrap_or(u64::MAX);
            let t3 = t0.saturating_add(elapsed_us);
            if let Ok(m) = measure(&PingFrame::request(reply.seq, t0), &reply, t3) {
                st.samples.push(m.delay_us);
            }
        }
        if let Some(tx) = lock(&self.to_engine).clone() {
            let _ = tx.try_send(reply);
        }
    }
}

/// Shared transport counters.
#[derive(Debug)]
struct TransportState {
    /// Datagrams received from the wheel.
    received: u64,
    /// Frames that parsed (valid packets).
    parsed_ok: u64,
    /// Parse rejects, by reason.
    rejects: RejectCounters,
    /// Last frame arrival (silence-episode detection).
    last_arrival: Option<Instant>,
    /// First / last frame arrivals (connectivity at end).
    first_frame: Option<Instant>,
    last_frame: Option<Instant>,
    /// Silence episodes: (last arrival before, first arrival after).
    silence_episodes: Vec<(Instant, Instant)>,
    /// Arrival-side sequence tracking: the engine's own
    /// [`SequenceTracker`] rules, run on PARSED frames at arrival.
    /// Its gaps are TRUE TRANSPORT LOSS (frames that never arrived
    /// in usable form); compare with the engine's gaps to get the
    /// coalesced count. Same rules (wrap / duplicate / reboot
    /// rebaseline) so the two counters are directly comparable.
    arrival_seq: SequenceTracker,
    /// PC-receive inter-arrival jitter histogram (wall-clock arrival
    /// spacing vs the 5000 µs nominal — the SLO's defined basis).
    arrival_jitter: JitterHistogram,
}

impl Default for TransportState {
    /// `silence_episodes` pre-allocated (16) BEFORE the run's
    /// `reset_counts()`: episodes are rare (a fault run has 1–5), so
    /// the buffer never grows inside the measured window (M-1).
    fn default() -> Self {
        Self {
            received: 0,
            parsed_ok: 0,
            rejects: RejectCounters::default(),
            last_arrival: None,
            first_frame: None,
            last_frame: None,
            silence_episodes: Vec::with_capacity(16),
            arrival_seq: SequenceTracker::new(),
            arrival_jitter: JitterHistogram::default(),
        }
    }
}

/// Current leg's channel senders (swapped on engine restart).
#[derive(Debug, Default)]
struct LegSenders {
    status_tx: Arc<Mutex<Option<mpsc::Sender<StatusPacket>>>>,
    ping_tx: Arc<Mutex<Option<mpsc::Sender<PingFrame>>>>,
}

/// Build the pipeline plan for a scenario's harness config.
fn plan_for(h: &HarnessConfig) -> PipelinePlan {
    let mut plan = PipelinePlan::default();
    if h.clutch_enabled {
        plan.clutch.enabled = true;
    }
    if h.accel_disabled {
        plan.accelerator.enabled = false;
    }
    if h.hpattern {
        plan.shifter.gate_mode = ShifterGateMode::HPattern;
    }
    plan
}

/// One engine leg: build the engine, wire its senders, spawn `run`.
/// Returns the join handle, stop handle, and the ping sender half.
fn spawn_leg(
    plan: &PipelinePlan,
    frame_rx: &Arc<AsyncMutex<mpsc::Receiver<StreamFrame>>>,
    queue_depth: &Arc<QueueDepth>,
    sink: RecordingSink,
    senders: &LegSenders,
    ping: Option<(Arc<Mutex<PingShared>>, mpsc::Sender<PingFrame>)>,
) -> (
    tokio::task::JoinHandle<EngineReport>,
    rsh_engine::engine::StopHandle,
) {
    let source = QueuedSource {
        rx: Arc::clone(frame_rx),
        depth: Arc::clone(queue_depth),
    };
    let mut engine = WheelEngine::new(ProcessingPipeline::new(*plan), source, Box::new(sink));
    if let Some((shared, to_wheel)) = ping {
        engine = engine.with_ping_sender(Box::new(PingSenderHalf { shared, to_wheel }));
    }
    let stop = engine.stop_handle();
    *lock(&senders.status_tx) = Some(engine.status_sender());
    *lock(&senders.ping_tx) = Some(engine.ping_reply_sender());
    let handle = tokio::spawn(engine.run());
    (handle, stop)
}

/// Wheel task: ticks at the wheel's emit period, control + ping
/// handling, wire output. Per-tick channel drains are capped at 32
/// (L-1: 2× the 16-slot channel capacity) — a stuck producer cannot
/// make one tick drain unboundedly; the cap is far above anything a
/// healthy run produces (≤ 1 control frame + ≤ 1 ping per tick) and
/// a hit is logged in the wheel summary.
async fn wheel_task(
    mut wheel: VirtualWheel,
    mut ctrl_rx: mpsc::Receiver<Vec<u8>>,
    mut ping_req_rx: mpsc::Receiver<PingFrame>,
    wire_tx: mpsc::Sender<WireMsg>,
    summary: Arc<Mutex<WheelSummary>>,
) {
    // H2 experiment (2026-11-14): wheel tick 5.1 ms (196 Hz) instead of
    // 5.0 ms. The engine ticks at 200 Hz; the OLD drain took at most
    // one frame per tick (latest-wins), so when the wheel emitted at
    // exactly the tick rate, the drain/send phase cycled and a bad phase
    // coalesced ~10–15% of frames (10-min soak loss 14.19% at 200 Hz).
    // The Round 2 multi-frame drain (up to 32 frames/tick) processes
    // every frame, so that failure mode is gone (deep-scan gate:
    // emit-200hz coalesced 0% at 200 Hz) — the 196 Hz emit is kept as
    // the documented soak default, not because the drain needs it.
    // A 196 Hz emit is strictly slower than the 200 Hz tick, so at
    // most one frame lands per tick window and every emit is drained
    // exactly once regardless of phase. Rate SLO is >= 195/s, so
    // 196 Hz still satisfies it. This is a DOCUMENTED HARNESS
    // LIMITATION (H-2): the true 200 Hz coalescing rate is measured
    // by the `emit-200hz` scenario (WheelConfig tick_interval_us
    // 5000), not hidden by the 196 Hz default.
    //
    // Missed-tick behavior is SKIP (same as the engine's own loop):
    // the default BURST catch-up fires all missed ticks at once after
    // a stall, which (a) emits a burst of frames that queue up for the
    // multi-frame drain (up to 32/tick; the excess is deferred to the
    // next tick, never dropped), and (b) compresses a
    // 2 s reboot blackout into < 250 ms of wall time, so the
    // engine's reboot evidence (stale watchdog) never fires and the
    // post-reboot stream counts ~1000 duplicates (baseline quick run:
    // fault-reboot duplicatesOrLate 944). A real board clock does not
    // burst-fire ticks — Skip is the faithful model, and it keeps
    // time-based faults (reboot/wifi-drop/silence) at their real
    // wall-clock lengths.
    let emit_us = wheel.tick_interval_us();
    let mut interval = tokio::time::interval(Duration::from_micros(emit_us));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        let mut drained = 0usize;
        while drained < 32 {
            match ctrl_rx.try_recv() {
                Ok(bytes) => {
                    drained += 1;
                    wheel.handle_control(&bytes);
                }
                Err(_) => break,
            }
        }
        if drained >= 32 {
            wheel.summary.log_event(format_args!(
                "t={:.1}s control drain hit the 32/tick cap (L-1 backstop)",
                wheel.now_s_pub()
            ));
        }
        let mut drained = 0usize;
        while drained < 32 {
            match ping_req_rx.try_recv() {
                Ok(req) => {
                    drained += 1;
                    let t1 = wheel.board_now_us();
                    let reply = PingFrame::reply(req.seq, req.t0, t1, t1 + 50);
                    let delay = wheel.ping_delay();
                    let tx = wire_tx.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(delay).await;
                        let _ = tx.send(WireMsg::PingReply(reply)).await;
                    });
                }
                Err(_) => break,
            }
        }
        if drained >= 32 {
            wheel.summary.log_event(format_args!(
                "t={:.1}s ping drain hit the 32/tick cap (L-1 backstop)",
                wheel.now_s_pub()
            ));
        }
        let out = wheel.next_tick();
        for bytes in out.frames {
            if wire_tx.try_send(WireMsg::Stream(bytes)).is_err() {
                break;
            }
        }
        if let Some(status) = out.status {
            let _ = wire_tx.try_send(WireMsg::Status(status));
        }
        *lock(&summary) = wheel.summary.clone();
    }
}

/// Transport task: parse wire bytes with the real parser, forward
/// frames (bounded queue, drop-newest on full), status, and ping replies.
async fn transport_task(
    mut wire_rx: mpsc::Receiver<WireMsg>,
    frame_tx: mpsc::Sender<StreamFrame>,
    queue_depth: Arc<QueueDepth>,
    senders: Arc<LegSenders>,
    ping_reply: PingReplyHalf,
    state: Arc<Mutex<TransportState>>,
) {
    while let Some(msg) = wire_rx.recv().await {
        match msg {
            WireMsg::Stream(bytes) => {
                let mut st = lock(&state);
                st.received += 1;
                if let Ok(frame) = StreamFrame::parse_counted(&bytes, &mut st.rejects) {
                    st.parsed_ok += 1;
                    let now = Instant::now();
                    // True-loss accounting: sequence gaps AT ARRIVAL.
                    // A bad-CRC frame never reaches this branch, so
                    // the next good frame's gap counts it as lost —
                    // honest, the frame was unusable. Reorder and
                    // reboot are handled by the tracker's own rules
                    // (duplicate / rebaseline), exactly as the engine
                    // counts them, so engine gaps − arrival gaps is
                    // the coalesced count by construction.
                    // The `SeqDecision` return is intentionally
                    // ignored: the tracker already counted the outcome
                    // (gap / duplicate / rebaseline) and the transport
                    // needs no per-frame branch.
                    let _ = st.arrival_seq.check(frame.seq);
                    // PC-receive jitter: wall-clock arrival spacing
                    // vs the 5000 µs nominal (the SLO's basis — see
                    // the module docs). The first arrival has no
                    // predecessor and records no sample.
                    if let Some(prev) = st.last_arrival {
                        let spacing_us =
                            u64::try_from(now.duration_since(prev).as_micros()).unwrap_or(u64::MAX);
                        let jitter = spacing_us.abs_diff(5000);
                        st.arrival_jitter.record(jitter);
                    }
                    if let Some(prev) = st.last_arrival
                        && now.duration_since(prev) > Duration::from_millis(300)
                    {
                        st.silence_episodes.push((prev, now));
                    }
                    st.last_arrival = Some(now);
                    if st.first_frame.is_none() {
                        st.first_frame = Some(now);
                    }
                    st.last_frame = Some(now);
                    // Enqueue for the engine (bounded queue, H-1):
                    // drop-newest on full, counted — never silent.
                    // `try_send` is synchronous, so no `.await` runs
                    // while the transport lock is held (M-12).
                    // `enqueue_frame` reserves the permit BEFORE
                    // counting: count-after-send wraps `current` when
                    // the engine dequeues in between (see its docs).
                    enqueue_frame(&frame_tx, &queue_depth, frame);
                }
            }
            WireMsg::Status(pkt) => {
                if let Some(tx) = lock(&senders.status_tx).clone() {
                    let _ = tx.try_send(pkt);
                }
            }
            WireMsg::PingReply(frame) => {
                ping_reply.on_reply(frame);
            }
        }
    }
}

/// MB per 10 min for a slope given in bytes/second (600 s window).
const MB_PER_10MIN: f64 = 600.0 / 1_000_000.0;

/// Count watchdog activations from silence episodes + neutral sends:
/// an episode counts only if a neutral send falls ≥ 250 ms after the
/// last arrival and before the next arrival.
fn watchdog_activations(episodes: &[(Instant, Instant)], neutral_times: &[Instant]) -> u64 {
    episodes
        .iter()
        .filter(|(start, end)| {
            neutral_times
                .iter()
                .any(|t| *t >= *start + Duration::from_millis(250) && *t <= *end)
        })
        .count() as u64
}

/// Loss percentage over a sequence span: skipped / (accepted +
/// skipped) × 100. 0.0 when nothing was seen (an empty run reports
/// 0.0 loss with 0 accepted — the rate assertions catch that).
fn loss_percent(gaps: u64, accepted: u64) -> f64 {
    let denom = accepted + gaps;
    if denom == 0 {
        0.0
    } else {
        gaps as f64 * 100.0 / denom as f64
    }
}

/// Frames that arrived but the engine never processed: engine gaps
/// minus arrival gaps. Saturates at 0 across engine restarts (a fresh
/// leg rebaselines, so its gaps do not span the restart the way the
/// run-scoped arrival counter does — the identity is exact on runs
/// without a restart, i.e. soak / long-soak).
fn coalesced_frames(engine_gaps: u64, arrival_gaps: u64) -> u64 {
    engine_gaps.saturating_sub(arrival_gaps)
}

/// Run one live scenario against the real engine. Returns the metrics
/// and the periodic sampler (RSS/heap trace for the soak report).
/// `log` receives progress lines (the 4-hour soak logs hourly).
pub async fn run_live(
    scenario: &Scenario,
    duration_s: u64,
    seed: u64,
    mut log: impl FnMut(&str),
) -> (Metrics, Sampler) {
    let mut metrics = Metrics::new(duration_s as f64, seed);
    let mut sampler = Sampler::new(Duration::from_secs(10));

    // Shared state.
    let sink_state = Arc::new(Mutex::new(SinkState::new()));
    let drop_until = Arc::new(Mutex::new(None));
    let transport_state = Arc::new(Mutex::new(TransportState::default()));
    let wheel_summary = Arc::new(Mutex::new(WheelSummary::default()));
    let ping_shared = Arc::new(Mutex::new(PingShared::default()));
    let senders = Arc::new(LegSenders::default());
    let (frame_tx, frame_rx) = mpsc::channel::<StreamFrame>(FRAME_QUEUE_CAPACITY);
    let frame_rx = Arc::new(AsyncMutex::new(frame_rx));
    let queue_depth = Arc::new(QueueDepth::default());
    let (wire_tx, wire_rx) = mpsc::channel::<WireMsg>(1024);
    let (ctrl_tx, ctrl_rx) = mpsc::channel::<Vec<u8>>(16);
    let (ping_req_tx, ping_req_rx) = mpsc::channel::<PingFrame>(16);

    let mut wheel_cfg = scenario.wheel.clone();
    wheel_cfg.seed = seed;
    let plan = plan_for(&scenario.harness);

    // Auth gate: send GET_CAPS before the wheel task starts, so the
    // gate opens on the first tick (the channel buffers it).
    let mut auth_get_caps_ok = 0.0;
    if !scenario.harness.no_auto_get_caps
        && let Ok(bytes) = build_frame(&TEST_KEY, &TEST_NONCE, 0x01, 1, 0, b"")
        && ctrl_tx.send(bytes).await.is_ok()
    {
        auth_get_caps_ok = 1.0;
    }

    // Spawn the engine leg first (its senders feed the transport).
    let sink = RecordingSink {
        shared: Arc::clone(&sink_state),
        drop_until: Arc::clone(&drop_until),
    };
    let ping_arg = if scenario.harness.ping {
        Some((Arc::clone(&ping_shared), ping_req_tx.clone()))
    } else {
        None
    };
    let (mut engine_handle, mut stop_handle) =
        spawn_leg(&plan, &frame_rx, &queue_depth, sink, &senders, ping_arg);

    // Transport + wheel tasks.
    let ping_reply = PingReplyHalf {
        shared: Arc::clone(&ping_shared),
        to_engine: Arc::clone(&senders.ping_tx),
    };
    let transport_handle = tokio::spawn(transport_task(
        wire_rx,
        frame_tx,
        Arc::clone(&queue_depth),
        Arc::clone(&senders),
        ping_reply,
        Arc::clone(&transport_state),
    ));
    let wheel = VirtualWheel::new(wheel_cfg);
    let wheel_handle = tokio::spawn(wheel_task(
        wheel,
        ctrl_rx,
        ping_req_rx,
        wire_tx,
        Arc::clone(&wheel_summary),
    ));

    // Settle, then baseline the heap counters for the run.
    tokio::time::sleep(Duration::from_millis(200)).await;
    rsh_benchmarks::alloc::reset_counts();
    let heap_start = net_live_bytes();
    let run_start = Instant::now();

    // Timed loop in 100 ms chunks: sampler, output-drop window,
    // engine restart, progress logging.
    let mut restarts = 0u64;
    let mut drop_started = false;
    let mut restarted = false;
    // M-6: a leg that does not stop in 5 s fails the scenario (see
    // `engineStopFailed`), it is not just an event line.
    let mut stop_failed = false;
    // H-5: wall-clock blackout of the engine restart (stop → new leg
    // spawned). Arrival `lossPercent` is 0.0% across a restart by
    // construction (arrivals never stop), so this is the number that
    // says the restart actually happened and how long output was dark.
    let mut engine_restart_ms: u64 = 0;
    let mut next_log = Duration::from_secs(60);
    let mut reports: Vec<EngineReport> = Vec::new();
    let duration = Duration::from_secs(duration_s);
    loop {
        let elapsed = run_start.elapsed();
        if elapsed >= duration {
            break;
        }
        sampler.maybe_sample(net_live_bytes());
        if elapsed >= next_log {
            let st = lock(&transport_state);
            let t_s = elapsed.as_secs_f64();
            log(&format!(
                "[{id}] t={t_s:.0}s valid={valid} rate={rate:.1}/s",
                id = scenario.id,
                valid = st.parsed_ok,
                rate = st.parsed_ok as f64 / t_s.max(0.001),
            ));
            next_log += Duration::from_secs(60);
        }
        if let Some((at, dur)) = scenario.harness.output_drop
            && !drop_started
            && elapsed.as_secs_f64() >= at
        {
            drop_started = true;
            *lock(&drop_until) = Some(Instant::now() + Duration::from_secs_f64(dur));
            log(&format!(
                "[{id}] output drop window: {dur}s",
                id = scenario.id
            ));
        }
        if let Some(at) = scenario.harness.restart_at
            && !restarted
            && elapsed.as_secs_f64() >= at
        {
            restarted = true;
            restarts += 1;
            log(&format!(
                "[{id}] restarting engine (leg 2)",
                id = scenario.id
            ));
            let restart_t0 = Instant::now();
            stop_handle.stop();
            match tokio::time::timeout(Duration::from_secs(5), &mut engine_handle).await {
                Ok(Ok(report)) => reports.push(report),
                Ok(Err(e)) => {
                    metrics.events.push(format!("engine leg 1 panicked: {e}"));
                }
                Err(_) => {
                    stop_failed = true;
                    metrics
                        .events
                        .push("engine leg 1 did not stop in 5 s".to_string());
                    engine_handle.abort();
                }
            }
            let sink2 = RecordingSink {
                shared: Arc::clone(&sink_state),
                drop_until: Arc::clone(&drop_until),
            };
            let ping_arg2 = if scenario.harness.ping {
                Some((Arc::clone(&ping_shared), ping_req_tx.clone()))
            } else {
                None
            };
            let (handle2, stop2) =
                spawn_leg(&plan, &frame_rx, &queue_depth, sink2, &senders, ping_arg2);
            engine_restart_ms = u64::try_from(restart_t0.elapsed().as_millis()).unwrap_or(u64::MAX);
            engine_handle = handle2;
            stop_handle = stop2;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Teardown: stop the engine, then abort the wheel + transport.
    let uptime = run_start.elapsed();
    stop_handle.stop();
    match tokio::time::timeout(Duration::from_secs(5), engine_handle).await {
        Ok(Ok(report)) => reports.push(report),
        Ok(Err(e)) => metrics.events.push(format!("engine panicked: {e}")),
        Err(_) => {
            stop_failed = true;
            metrics
                .events
                .push("engine did not stop in 5 s".to_string());
        }
    }
    wheel_handle.abort();
    transport_handle.abort();
    sampler.maybe_sample(net_live_bytes());
    let heap_end = net_live_bytes();
    metrics.uptime_sec = uptime.as_secs_f64();
    metrics.peak_working_set_mb = sampler.peak_rss_bytes() as f64 / 1_000_000.0;

    // Combine per-leg reports (counters sum; p99s take the max leg).
    let mut gaps = 0u64;
    let mut accepted = 0u64;
    let mut duplicates = 0u64;
    let mut timeouts = 0u64;
    let mut jitter_p99: Option<u64> = None;
    let mut output_p99: Option<u64> = None;
    for r in &reports {
        gaps += r.sequence_gaps;
        accepted += r.sequence_accepted;
        duplicates += r.sequence_duplicates;
        timeouts += r.ping_timeouts;
        jitter_p99 = match (jitter_p99, r.metrics.jitter_p99()) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, None) => a,
            (None, b) => b,
        };
        output_p99 = match (output_p99, r.metrics.output_latency_p99()) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, None) => a,
            (None, b) => b,
        };
    }

    // Assemble metrics.
    let sink = lock(&sink_state);
    let transport = lock(&transport_state);
    let wheel_sum = lock(&wheel_summary);
    let ping = lock(&ping_shared);
    let secs = uptime.as_secs_f64().max(0.001);

    metrics.set("engineTicks", sink.update_calls as f64);
    metrics.set("updateCalls", sink.update_calls as f64);
    metrics.set("neutralSends", sink.neutral_sends as f64);
    metrics.set("datagrams", transport.received as f64);
    metrics.set("validPackets", transport.parsed_ok as f64);
    metrics.set("badCrc", transport.rejects.bad_crc as f64);
    metrics.set("badMagic", transport.rejects.bad_magic as f64);
    metrics.set("tooShort", transport.rejects.too_short as f64);
    metrics.set("badVersion", transport.rejects.bad_version as f64);
    metrics.set("badFrame", transport.rejects.bad_frame as f64);
    // TRUE TRANSPORT LOSS (see the module docs): sequence gaps counted
    // at ARRIVAL, not at the engine. The engine's gaps conflate
    // transport loss with coalescing (frames that arrived but were
    // never processed); the SLO is about
    // the transport, so `lossPercent` is the arrival number and the
    // engine number is kept separately, never hidden.
    let arrival_gaps = transport.arrival_seq.gaps();
    let arrival_accepted = transport.arrival_seq.accepted();
    let coalesced = coalesced_frames(gaps, arrival_gaps);
    metrics.set("lostPackets", arrival_gaps as f64);
    metrics.set("duplicatesOrLate", duplicates as f64);
    metrics.set("lossPercent", loss_percent(arrival_gaps, arrival_accepted));
    metrics.set(
        "transportLossPercent",
        loss_percent(arrival_gaps, arrival_accepted),
    );
    metrics.set("transportArrivalGaps", arrival_gaps as f64);
    metrics.set(
        "transportDuplicates",
        transport.arrival_seq.duplicates() as f64,
    );
    metrics.set(
        "transportRebaselines",
        transport.arrival_seq.rebaselines() as f64,
    );
    metrics.set("engineLossPercent", loss_percent(gaps, accepted));
    metrics.set("engineSequenceGaps", gaps as f64);
    metrics.set("coalescedFrames", coalesced as f64);
    // Share of the arrived stream's sequence span that arrived but
    // was never processed by the engine (denominator = arrival span,
    // not accepted + coalesced — coalesced frames are a subset of
    // the arrivals, not additional to them).
    let arrival_span = arrival_accepted + arrival_gaps;
    metrics.set(
        "coalescedPercent",
        if arrival_span == 0 {
            0.0
        } else {
            coalesced as f64 * 100.0 / arrival_span as f64
        },
    );
    metrics.set("currentRatePerSecond", transport.parsed_ok as f64 / secs);
    // PC-RECEIVE jitter p99 (the SLO's basis — see the module docs).
    // The engine's board-timestamp jitter is kept separately as
    // `engineTickJitterP99Us` (informational, never hidden).
    if let Some(v) = transport.arrival_jitter.p99() {
        metrics.set("tickJitterP99Us", v as f64);
    }
    metrics.set(
        "pcReceiveJitterOver5000",
        transport.arrival_jitter.over_5000() as f64,
    );
    metrics.arrival_jitter_hist = transport.arrival_jitter.nonzero_bins();
    if let Some(v) = jitter_p99 {
        metrics.set("engineTickJitterP99Us", v as f64);
    }
    if let Some(v) = output_p99 {
        metrics.set("outputLatencyP99Us", v as f64);
    }
    metrics.set(
        "e2eMeasured",
        if ping.samples.is_empty() { 0.0 } else { 1.0 },
    );
    metrics.set("e2eSamples", ping.samples.len() as f64);
    metrics.set("e2eTimeouts", timeouts as f64);
    // Raw ping samples (reply order) + max / second-max / over-SLO
    // count, so a failing p99 can be diagnosed from the run itself
    // instead of guessed at: with 31 samples the nearest-rank p99 IS
    // the single slowest sample.
    metrics.e2e_samples_us = ping.samples.clone();
    if !ping.samples.is_empty() {
        let mut sorted = ping.samples.clone();
        sorted.sort_unstable();
        let max = *sorted.last().unwrap_or(&0);
        let second_max = sorted.iter().rev().nth(1).copied().unwrap_or(max);
        metrics.set("e2eDelayMaxUs", max as f64);
        metrics.set("e2eDelaySecondMaxUs", second_max as f64);
        metrics.set(
            "e2eSamplesOverSlo",
            ping.samples.iter().filter(|s| **s > 10_000).count() as f64,
        );
        if let Some(p50) = crate::metrics::percentile_us(&ping.samples, 50) {
            metrics.set("e2eDelayP50Us", p50 as f64);
        }
        if let Some(p99) = crate::metrics::percentile_us(&ping.samples, 99) {
            metrics.set("e2eDelayP99Us", p99 as f64);
        }
        // H-8: bootstrap 95% CI for the p99 (200 resamples, seeded
        // by the run seed — deterministic). The SLO still asserts on
        // the p99 itself; the interval says how wide the tail
        // estimate is. This is a LOOPBACK p99 (in-process channel,
        // no real Wi-Fi): a real-Wi-Fi re-measure is a Windows/human
        // gate, recorded in the report — the number here must never
        // be quoted as a Wi-Fi measurement.
        if let Some((lo, hi)) = crate::metrics::bootstrap_p99_ci(&ping.samples, 200, seed) {
            metrics.set("e2eDelayP99CiLowUs", lo as f64);
            metrics.set("e2eDelayP99CiHighUs", hi as f64);
        }
    }
    metrics.set(
        "watchdogActivations",
        watchdog_activations(&transport.silence_episodes, &sink.neutral_times) as f64,
    );
    let last_arrival = transport.last_frame;
    metrics.set(
        "watchdogActiveAtEnd",
        match last_arrival {
            Some(t) if t.elapsed() > Duration::from_millis(250) => 1.0,
            _ => 0.0,
        },
    );
    metrics.set(
        "wheelConnectedAtEnd",
        match last_arrival {
            Some(t) if t.elapsed() <= Duration::from_millis(250) => 1.0,
            _ => 0.0,
        },
    );
    metrics.set("restarts", restarts as f64);
    // M-6: 1.0 when a leg (or the final engine) did not stop in 5 s —
    // every live scenario asserts this is 0, so a stuck stop FAILS
    // the scenario instead of leaving an event line behind.
    metrics.set("engineStopFailed", if stop_failed { 1.0 } else { 0.0 });
    // H-5: engine-restart blackout (ms, stop → new leg spawned;
    // 0 when the scenario restarts nothing). Stands next to the
    // restart scenarios' `lossPercent` (0.0% by construction), so the
    // restart is visible as a blackout, not as a perfect loss number.
    metrics.set("engineRestartMs", engine_restart_ms as f64);
    // H-5: reboot/silence blackout, reported separately from
    // `lossPercent`: every silence episode's wall-clock length and
    // the frames it cost at the nominal 200 Hz emit rate. A reboot
    // rebaselines the sequence, so its 2 s of silence costs ~400
    // frames WITHOUT counting as transport loss — the blackout
    // number is where that loss is visible, with its own bound.
    let mut silence_total_ms: u64 = 0;
    let mut silence_max_ms: u64 = 0;
    for (start, end) in &transport.silence_episodes {
        let ms = u64::try_from(end.duration_since(*start).as_millis()).unwrap_or(u64::MAX);
        silence_total_ms = silence_total_ms.saturating_add(ms);
        silence_max_ms = silence_max_ms.max(ms);
    }
    metrics.set("silenceEpisodes", transport.silence_episodes.len() as f64);
    metrics.set("silenceTotalMs", silence_total_ms as f64);
    metrics.set("silenceMaxMs", silence_max_ms as f64);
    metrics.set(
        "blackoutExpectedFrames",
        silence_total_ms as f64 * 200.0 / 1000.0,
    );
    // H-1 drain support, observed: frames the queue dropped (full)
    // and its high-water mark. Healthy runs show 0 dropped and a
    // depth of ~0–2.
    metrics.set(
        "queueDropped",
        queue_depth.dropped.load(Ordering::Relaxed) as f64,
    );
    metrics.set(
        "queueDepthMax",
        queue_depth.max.load(Ordering::Relaxed) as f64,
    );
    metrics.set("wheelCtrlAccepted", wheel_sum.ctrl_accepted as f64);
    metrics.set("authGetCapsOk", auth_get_caps_ok);
    // Axis extremes (only when non-neutral output was observed).
    let axis = |min: i64, max: i64| (min != i64::MAX).then_some((min, max));
    if let Some((lo, hi)) = axis(sink.steering_min, sink.steering_max) {
        metrics.set("steeringAxisMin", lo as f64);
        metrics.set("steeringAxisMax", hi as f64);
    }
    if let Some((lo, hi)) = axis(sink.accel_min, sink.accel_max) {
        metrics.set("accelAxisMin", lo as f64);
        metrics.set("accelAxisMax", hi as f64);
    }
    if let Some((lo, hi)) = axis(sink.brake_min, sink.brake_max) {
        metrics.set("brakeAxisMin", lo as f64);
        metrics.set("brakeAxisMax", hi as f64);
    }
    if let Some((lo, hi)) = axis(sink.clutch_min, sink.clutch_max) {
        metrics.set("clutchAxisMin", lo as f64);
        metrics.set("clutchAxisMax", hi as f64);
    }
    if let Some((lo, hi)) = axis(sink.handbrake_min, sink.handbrake_max) {
        metrics.set("handbrakeAxisMin", lo as f64);
        metrics.set("handbrakeAxisMax", hi as f64);
    }
    metrics.set("buttonMaskOr", sink.button_mask_or as f64);
    metrics.set("gearMaskSeen", sink.gear_mask_seen as f64);
    metrics.set("maxAbsEncoderDelta", sink.max_abs_encoder_delta as f64);
    metrics.set("distinctButtonMasks", sink.distinct_masks.len() as f64);
    metrics.set(
        "allocBytesDelta",
        heap_end.saturating_sub(heap_start) as f64,
    );
    metrics.set("perSecondSamples", sampler.samples.len() as f64);
    // Memory slopes (MB per 10 min) from 60 s onward (skip warm-up).
    metrics.set(
        "heapSlopeMBPer10Min",
        sampler.heap_slope_from(60.0) * MB_PER_10MIN,
    );
    metrics.set(
        "rssSlopeMBPer10Min",
        sampler.rss_slope_from(60.0) * MB_PER_10MIN,
    );
    // M-8: the same slopes over the 60 s window (samples from 10 s
    // onward — the first sample is warm-up), pro-rated to MB/10 min
    // against the same 1 MB/10 min bound, PLUS the slope standard
    // error: a 60 s slope over ~6 samples is noisy, and the SE says
    // whether it is distinguishable from flat.
    let (heap_slope_60, heap_se_60) = sampler.heap_slope_se_from(10.0);
    let (rss_slope_60, rss_se_60) = sampler.rss_slope_se_from(10.0);
    metrics.set("heapSlope60sMBPer10Min", heap_slope_60 * MB_PER_10MIN);
    metrics.set("heapSlope60sStdErrMBPer10Min", heap_se_60 * MB_PER_10MIN);
    metrics.set("rssSlope60sMBPer10Min", rss_slope_60 * MB_PER_10MIN);
    metrics.set("rssSlope60sStdErrMBPer10Min", rss_se_60 * MB_PER_10MIN);
    let (heap_se_600, rss_se_600) = (
        sampler.heap_slope_se_from(60.0).1,
        sampler.rss_slope_se_from(60.0).1,
    );
    metrics.set("heapSlopeStdErrMBPer10Min", heap_se_600 * MB_PER_10MIN);
    metrics.set("rssSlopeStdErrMBPer10Min", rss_se_600 * MB_PER_10MIN);
    // Wheel events are read AFTER `heap_end`, so the `events()`
    // allocation never lands in the measured window (M-1).
    metrics.events.extend(wheel_sum.events());
    if wheel_sum.events_dropped > 0 {
        metrics.events.push(format!(
            "wheel event log dropped {} events (8 KiB buffer full)",
            wheel_sum.events_dropped
        ));
    }
    (metrics, sampler)
}

/// Contract-only self-checks (no engine run): the three `duration 0`
/// scenarios from the registry.
#[must_use]
pub fn run_contract(scenario: &Scenario) -> Metrics {
    let mut metrics = Metrics::new(0.0, 42);
    let (pass, detail) = match scenario.id {
        "config-transaction" => self_check_config_transaction(),
        "control-channel" => self_check_control_channel(),
        "pin-map-push" => self_check_pin_map_push(),
        "pairing-window-open" => self_check_pairing_window_open(),
        "pairing-window-expired" => self_check_pairing_window_expired(),
        "pairing-unreachable" => self_check_pairing_unreachable(),
        "ota-e2e-success" => crate::ota_checks::self_check_ota_e2e_success(),
        "ota-e2e-same-version" => crate::ota_checks::self_check_ota_e2e_same_version_reinstall(),
        "ota-e2e-bad-signature" => crate::ota_checks::self_check_ota_e2e_bad_signature(),
        "ota-e2e-disconnect-rollback" => {
            crate::ota_checks::self_check_ota_e2e_disconnect_rollback()
        }
        "upgrade-preserve" => crate::ota_checks::self_check_upgrade_preserve(),
        "installer-preserves-data" => crate::ota_checks::self_check_installer_preserves_data(),
        // v2.0.2 gate checks (fix plan §C — the plan was removed from
        // docs/ in the 2026-11-19 repo cleanup, see git history).
        "gate-backend-switch" => crate::gate_checks::gate_backend_switch(),
        "gate-user-errors" => crate::gate_checks::gate_user_errors(),
        "gate-ipc-timeout" => crate::gate_checks::gate_ipc_timeout(),
        "gate-perf" => crate::gate_checks::gate_perf(),
        "gate-ota-e2e" => crate::gate_checks::gate_ota_e2e(),
        "gate-installer-audit" => crate::gate_checks::gate_installer_audit(),
        "gate-slider-source" => crate::gate_checks::gate_slider_source(),
        "gate-tray-persistence-source" => crate::gate_checks::gate_tray_persistence_source(),
        "gate-tray-data-source" => crate::gate_checks::gate_tray_data_source(),
        "gate-x-close-source" => crate::gate_checks::gate_x_close_source(),
        "gate-wizard-reopen-source" => crate::gate_checks::gate_wizard_reopen_source(),
        "gate-wizard-fake-data-source" => crate::gate_checks::gate_wizard_fake_data_source(),
        "gate-loading-source" => crate::gate_checks::gate_loading_source(),
        "gate-hover-source" => crate::gate_checks::gate_hover_source(),
        "gate-theme-source" => crate::gate_checks::gate_theme_source(),
        "gate-overflow-source" => crate::gate_checks::gate_overflow_source(),
        "gate-notifications-source" => crate::gate_checks::gate_notifications_source(),
        // Cards layout fix (2026-11-19): Extras ON/OFF size + responsive gaps.
        "gate-extras-card-size-source" => crate::gate_checks::gate_extras_card_size_source(),
        "gate-card-gap-source" => crate::gate_checks::gate_card_gap_source(),
        // v2.0.2 gate checks, re-scan findings (B-1, B-2, H-3, H-4, M-9).
        "gate-pairing-long-call" => crate::gate_checks::gate_pairing_long_call(),
        "gate-hid-uac-dwell" => crate::gate_checks::gate_hid_uac_dwell(),
        "gate-bindings-ui" => crate::gate_checks::gate_bindings_ui(),
        "gate-ota-flash-caller" => crate::gate_checks::gate_ota_flash_caller(),
        "gate-version-bump" => crate::gate_checks::gate_version_bump(),
        // v2.0.3 lifecycle checks (F17): real daemon/UI processes
        // under Xvfb — the checks the v2.0.2 gate was missing.
        "gate-respawn-bound" => crate::lifecycle_checks::gate_respawn_bound(),
        "gate-ipc-bind-failure" => crate::lifecycle_checks::gate_ipc_bind_failure(),
        "gate-hung-daemon" => crate::lifecycle_checks::gate_hung_daemon(),
        "gate-already-running-race" => crate::lifecycle_checks::gate_already_running_race(),
        "gate-xvfb-hidden-shown" => crate::lifecycle_checks::gate_xvfb_hidden_shown(),
        "gate-rapid-reconnect" => crate::lifecycle_checks::gate_rapid_reconnect(),
        "gate-frame-budget" => crate::lifecycle_checks::gate_frame_budget(),
        "gate-toast-dedupe-integration" => crate::lifecycle_checks::gate_toast_dedupe_integration(),
        "gate-lifecycle-soak" => crate::lifecycle_checks::gate_lifecycle_soak(),
        "gate-tray-popup-timeline" => crate::lifecycle_checks::gate_tray_popup_timeline(),
        _ => (
            false,
            format!("no self-check defined for {id}", id = scenario.id),
        ),
    };
    metrics.self_check = if pass { "pass".into() } else { "fail".into() };
    metrics.events.push(detail);
    metrics
}

/// Config transaction: factory blob → `SET_CFG` chunks → staging →
/// finish → identical blob; tampered chunk rejected; incomplete
/// staging cannot finish.
fn self_check_config_transaction() -> (bool, String) {
    use rsh_protocol::{ConfigStaging, SetCfgChunk, factory_default_blob};
    let mut notes: Vec<String> = Vec::new();
    let blob = factory_default_blob();
    let Ok(bytes) = blob.encode() else {
        return (false, "factory blob encode failed".into());
    };
    // Chunk the blob at 64 B per SET_CFG (NOT the 1024 B wire max):
    // the factory blob is 188 B, so 1024 B chunks would make it a
    // single chunk and the "incomplete staging" case below could not
    // be incomplete — start(chunks[0]) would already hold the whole
    // blob and finish() would (correctly) succeed. At 64 B the blob
    // spans 3 chunks, so staging only the first is genuinely
    // incomplete and finish() must reject it (H2 rig bug, quick
    // run 2026-11-14: "incomplete staging finished successfully").
    let mut chunks: Vec<SetCfgChunk> = Vec::new();
    let mut off = 0usize;
    while off < bytes.len() {
        let end = (off + 64).min(bytes.len());
        chunks.push(SetCfgChunk::new(
            u16::try_from(off).unwrap_or(u16::MAX),
            u16::try_from(bytes.len()).unwrap_or(u16::MAX),
            bytes[off..end].to_vec(),
        ));
        off = end;
    }
    let Ok(mut staging) = ConfigStaging::start(&chunks[0]) else {
        return (false, "staging start failed".into());
    };
    for chunk in &chunks[1..] {
        if let Err(e) = staging.add(chunk) {
            return (false, format!("staging add failed: {e:?}"));
        }
    }
    if !staging.is_complete() {
        return (false, "staging incomplete after all chunks".into());
    }
    let Ok(done) = staging.finish() else {
        return (false, "staging finish failed on a valid blob".into());
    };
    let Ok(roundtrip) = done.encode() else {
        return (false, "finished blob encode failed".into());
    };
    if roundtrip != bytes {
        return (false, "blob round-trip mismatch".into());
    }
    notes.push(format!(
        "round-trip OK ({n} B, {c} chunks)",
        n = bytes.len(),
        c = chunks.len()
    ));
    // Tampered chunk: flip one data byte — finish must reject it.
    let mut bad_chunks = chunks.clone();
    if let Some(first) = bad_chunks.get_mut(0) {
        let mut data = first.data.clone();
        if let Some(b) = data.first_mut() {
            *b ^= 0xFF;
        }
        *first = SetCfgChunk::new(first.offset, first.total, data);
    }
    let mut bad = match ConfigStaging::start(&bad_chunks[0]) {
        Ok(s) => s,
        Err(e) => return (false, format!("tampered staging start failed: {e:?}")),
    };
    let mut tamper_rejected = false;
    for chunk in &bad_chunks[1..] {
        if bad.add(chunk).is_err() {
            tamper_rejected = true;
            break;
        }
    }
    if !tamper_rejected {
        match bad.finish() {
            Ok(_) => {
                return (false, "tampered blob was ACCEPTED by finish()".into());
            }
            Err(_) => tamper_rejected = true,
        }
    }
    if !tamper_rejected {
        return (false, "tampered blob not rejected".into());
    }
    notes.push("tampered chunk rejected".into());
    // Incomplete staging cannot finish.
    let incomplete = ConfigStaging::start(&chunks[0]).expect("start works");
    if incomplete.finish().is_ok() {
        return (false, "incomplete staging finished successfully".into());
    }
    notes.push("incomplete staging rejected".into());
    (true, notes.join("; "))
}

/// Control channel: `GET_CAPS` verifies; tampered tag rejected; replay
/// rejected; duplicate reqId rejected; reply round-trips.
fn self_check_control_channel() -> (bool, String) {
    use rsh_protocol::control::{Opcode, build_reply};
    let mut notes: Vec<String> = Vec::new();
    let Ok(frame_bytes) = build_frame(&TEST_KEY, &TEST_NONCE, Opcode::GetCaps as u8, 1, 0, b"")
    else {
        return (false, "GET_CAPS build failed".into());
    };
    let Ok(frame) = ControlFrame::verify(&frame_bytes, &TEST_KEY, &TEST_NONCE) else {
        return (false, "valid GET_CAPS did not verify".into());
    };
    if frame.cmd != Opcode::GetCaps as u8 {
        return (false, "verified frame has the wrong opcode".into());
    }
    notes.push("GET_CAPS verifies".into());
    // Tampered tag.
    let mut bad = frame_bytes.clone();
    if let Some(b) = bad.last_mut() {
        *b ^= 0xFF;
    }
    if ControlFrame::verify(&bad, &TEST_KEY, &TEST_NONCE).is_ok() {
        return (false, "tampered tag was ACCEPTED".into());
    }
    notes.push("tampered tag rejected".into());
    // Replay: same counter twice.
    let mut replay = ReplayTracker::new();
    if !replay.observe(5) || replay.observe(5) {
        return (
            false,
            "replay tracker did not reject the second observe(5)".into(),
        );
    }
    if !replay.is_replay(5) || replay.is_replay(6) {
        return (false, "replay tracker is_replay wrong".into());
    }
    notes.push("replay rejected".into());
    // Duplicate reqId.
    let mut dup = DuplicateCache::new();
    if !dup.insert(1, 0) || dup.insert(1, 0) {
        return (
            false,
            "duplicate cache did not reject the second insert(1, 0)".into(),
        );
    }
    if !dup.is_duplicate(1, 0) {
        return (false, "duplicate cache is_duplicate wrong".into());
    }
    notes.push("duplicate reqId rejected".into());
    // Reply round-trip.
    let Ok(reply_bytes) = build_reply(
        &TEST_KEY,
        &TEST_NONCE,
        0x01,
        1,
        0,
        rsh_protocol::ErrorCode::Ok,
        b"",
    ) else {
        return (false, "reply build failed".into());
    };
    if ControlFrame::verify(&reply_bytes, &TEST_KEY, &TEST_NONCE).is_err() {
        return (false, "reply did not verify".into());
    }
    notes.push("reply round-trips".into());
    (true, notes.join("; "))
}

/// Pin-map push: factory blob validates + round-trips through `SET_CFG`
/// chunks; a conflicting pin map is rejected by PIN-RULES.
fn self_check_pin_map_push() -> (bool, String) {
    use rsh_protocol::{
        ConfigBlob, ConfigStaging, PinModule, SetCfgChunk, factory_default_blob, validate,
        validate_blob,
    };
    let mut notes: Vec<String> = Vec::new();
    let blob = factory_default_blob();
    let errors = validate_blob(&blob);
    if !errors.is_empty() {
        return (false, format!("factory blob failed PIN-RULES: {errors:?}"));
    }
    notes.push("factory blob passes PIN-RULES".into());
    let Ok(bytes) = blob.encode() else {
        return (false, "factory blob encode failed".into());
    };
    let mut chunks: Vec<SetCfgChunk> = Vec::new();
    let mut off = 0usize;
    while off < bytes.len() {
        let end = (off + 1024).min(bytes.len());
        chunks.push(SetCfgChunk::new(
            u16::try_from(off).unwrap_or(u16::MAX),
            u16::try_from(bytes.len()).unwrap_or(u16::MAX),
            bytes[off..end].to_vec(),
        ));
        off = end;
    }
    let Ok(mut staging) = ConfigStaging::start(&chunks[0]) else {
        return (false, "staging start failed".into());
    };
    for chunk in &chunks[1..] {
        if let Err(e) = staging.add(chunk) {
            return (false, format!("staging add failed: {e:?}"));
        }
    }
    match staging.finish() {
        Ok(done) => match done.encode() {
            Ok(rt) => match ConfigBlob::decode(&rt) {
                Ok(_) => notes.push("blob round-trips through SET_CFG chunks".into()),
                Err(e) => return (false, format!("finished blob decode failed: {e:?}")),
            },
            Err(e) => return (false, format!("finished blob encode failed: {e:?}")),
        },
        Err(e) => return (false, format!("staging finish failed: {e:?}")),
    }
    // Conflicting map: two AXIS modules on the same pin.
    let conflict = vec![PinModule::axis(25), PinModule::axis(25)];
    let errors = validate(&conflict, false);
    if errors.is_empty() {
        return (
            false,
            "conflicting pin map was ACCEPTED by PIN-RULES".into(),
        );
    }
    notes.push(format!(
        "conflicting map rejected ({n} errors)",
        n = errors.len()
    ));
    (true, notes.join("; "))
}

/// Pairing self-checks (Phase 2, Phase 0 §4.4 gap): the L1
/// Connection Manager driven through the three pairing outcomes.
/// Pure logic — no wheel, no UDP — so these run as contract
/// scenarios (duration 0) in every rig mode.
fn self_check_pairing_window_open() -> (bool, String) {
    use rsh_api::{ConnEvent, ConnectionManager, ManagerAction};
    let mut manager = ConnectionManager::new();
    let now = Instant::now();
    manager.on_event(ConnEvent::Connect, now);
    let action = manager.on_event(
        ConnEvent::HelloReceived {
            unpaired: true,
            window_open: true,
        },
        now,
    );
    if action != ManagerAction::StartAuthentication {
        return (
            false,
            format!("window-open HELLO must start auth, got {action:?}"),
        );
    }
    manager.on_event(ConnEvent::Authenticating, now);
    manager.on_event(ConnEvent::CapsOk, now);
    if !manager.state().is_active() {
        return (
            false,
            format!(
                "must be Active after GET_CAPS ok, got {:?}",
                manager.state()
            ),
        );
    }
    if manager.state().label() != "Connected" {
        return (false, "Active must label Connected".into());
    }
    (
        true,
        "window-open pairing: Idle -> LinkUp -> Hello -> Authenticating -> Active (Connected)"
            .into(),
    )
}

fn self_check_pairing_window_expired() -> (bool, String) {
    use rsh_api::{ConnEvent, ConnectionError, ConnectionManager, ManagerAction};
    let mut manager = ConnectionManager::new();
    let now = Instant::now();
    manager.on_event(ConnEvent::Connect, now);
    let action = manager.on_event(
        ConnEvent::HelloReceived {
            unpaired: true,
            window_open: false,
        },
        now,
    );
    if action != ManagerAction::NeedsPairing(ConnectionError::WindowClosed) {
        return (
            false,
            format!("expired window must fail closed NeedsPairing(WindowClosed), got {action:?}"),
        );
    }
    if manager.state().is_active() {
        return (false, "expired window must never reach Active".into());
    }
    if !matches!(
        manager.state(),
        rsh_api::ConnectionState::Error {
            retry_in_ms: None,
            ..
        }
    ) {
        return (
            false,
            format!(
                "expired window must stop auto-retry (retry None), got {:?}",
                manager.state()
            ),
        );
    }
    (
        true,
        "window-expired pairing: fails closed with typed reason WindowClosed, auto-retry stopped"
            .into(),
    )
}

fn self_check_pairing_unreachable() -> (bool, String) {
    use rsh_api::{ConnEvent, ConnectionError, ConnectionManager};
    let mut manager = ConnectionManager::new();
    let now = Instant::now();
    manager.on_event(ConnEvent::Connect, now);
    for _ in 0..3 {
        manager.on_event(ConnEvent::HelloTimeout, now);
    }
    match manager.state() {
        rsh_api::ConnectionState::Error { reason, .. }
            if *reason == ConnectionError::Unreachable => {}
        other => {
            return (
                false,
                format!("3 HELLO timeouts must end in Error(Unreachable), got {other:?}"),
            );
        }
    }
    if manager.state().is_active() {
        return (false, "unreachable board must never reach Active".into());
    }
    (true, "unreachable board: fails closed with typed reason Unreachable (distinct from WindowClosed)".into())
}

#[cfg(test)]
mod tests {
    use super::{
        FRAME_QUEUE_CAPACITY, PacketSource, QueueDepth, QueuedSource, coalesced_frames,
        enqueue_frame, loss_percent,
    };
    use rsh_protocol::StreamFrame;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::{Mutex as AsyncMutex, mpsc};
    use tokio::time::timeout;

    /// True-loss vs coalesced arithmetic (Phase H fix): the gate
    /// soak shape is 117,695 arrivals, 0 arrival gaps, ~4,060
    /// engine gaps — true loss 0%, everything coalesced.
    #[test]
    fn true_loss_and_coalesced_math() {
        assert_eq!(loss_percent(0, 117_695), 0.0);
        assert_eq!(coalesced_frames(4060, 0), 4060);
        // A restart rebaselines the fresh engine leg, so its gaps do
        // not span the restart: the saturating form holds at 0.
        assert_eq!(coalesced_frames(10, 50), 0);
        // Loss formula: 5 skipped of a 1005 span.
        assert!((loss_percent(5, 1000) - 5.0 * 100.0 / 1005.0).abs() < 1e-12);
        assert_eq!(loss_percent(0, 0), 0.0);
    }

    /// H-1 drain support: the queue handoff preserves EVERY frame,
    /// in order — unlike the old latest-wins `watch` channel, which
    /// collapsed frames in the channel (the H2 repro received 0/400
    /// with the old `WatchSource` shape). A burst of 200 frames
    /// (under the 256-frame capacity) enqueued at once must all come
    /// back out, in sequence order; any coalescing is then frames
    /// the engine never processed, measured as `coalescedPercent`
    /// — never a channel loss.
    /// Also proves the depth accounting: max depth hits 200, drops 0.
    #[tokio::test]
    async fn queued_source_delivers_every_frame_in_order() {
        let (tx, rx) = mpsc::channel::<StreamFrame>(FRAME_QUEUE_CAPACITY);
        let depth = Arc::new(QueueDepth::default());
        let mut src = QueuedSource {
            rx: Arc::new(AsyncMutex::new(rx)),
            depth: Arc::clone(&depth),
        };
        for n in 1..=200u64 {
            let frame = StreamFrame::new(
                n as u16,
                (n * 5000) as u32,
                rsh_protocol::StreamFlags::default(),
                &[],
                &[0; 4],
                0,
            );
            tx.try_send(frame).expect("queue holds 200");
            depth.on_enqueue();
        }
        let mut received: Vec<u16> = Vec::new();
        while received.len() < 200 {
            let got = timeout(Duration::from_millis(100), src.next_frame()).await;
            match got {
                Ok(Some(frame)) => received.push(frame.seq),
                other => panic!("queue stalled after {} frames: {other:?}", received.len()),
            }
        }
        let expected: Vec<u16> = (1..=200).collect();
        assert_eq!(received, expected, "frames must arrive in order, none lost");
        assert_eq!(depth.max.load(std::sync::atomic::Ordering::Relaxed), 200);
        assert_eq!(depth.dropped.load(std::sync::atomic::Ordering::Relaxed), 0);
    }

    /// The engine's drain shape (zero-timeout poll, keep the latest)
    /// over the queue still takes the newest when several frames are
    /// queued — the coalescing is the engine's per-tick choice, and
    /// the frames it skipped are still IN the queue for a future
    /// multi-frame drain (they were not collapsed by the channel).
    #[tokio::test]
    async fn queued_source_keeps_backlog_for_multi_frame_drain() {
        let (tx, rx) = mpsc::channel::<StreamFrame>(FRAME_QUEUE_CAPACITY);
        let depth = Arc::new(QueueDepth::default());
        let mut src = QueuedSource {
            rx: Arc::new(AsyncMutex::new(rx)),
            depth: Arc::clone(&depth),
        };
        for n in 1..=10u64 {
            let frame = StreamFrame::new(
                n as u16,
                (n * 5000) as u32,
                rsh_protocol::StreamFlags::default(),
                &[],
                &[0; 4],
                0,
            );
            tx.try_send(frame).expect("enqueue");
            depth.on_enqueue();
        }
        // Queue drain shape: zero-timeout polls until empty; the last
        // frame polled is the newest (the engine's multi-frame drain
        // collects ALL of them — see `drain_source`).
        let mut latest: Option<u16> = None;
        let mut polled = 0u64;
        while let Ok(Some(frame)) = timeout(Duration::ZERO, src.next_frame()).await {
            latest = Some(frame.seq);
            polled += 1;
        }
        assert_eq!(polled, 10, "drain saw all 10 queued frames");
        assert_eq!(latest, Some(10), "last polled frame is the newest");
        // And a second consumer over the same queue (leg 2 after a
        // restart) still sees frames enqueued after the swap.
        let frame = StreamFrame::new(
            11,
            11 * 5000,
            rsh_protocol::StreamFlags::default(),
            &[],
            &[0; 4],
            0,
        );
        tx.try_send(frame).expect("enqueue after drain");
        depth.on_enqueue();
        let got = timeout(Duration::from_millis(100), src.next_frame()).await;
        assert!(matches!(got, Ok(Some(f)) if f.seq == 11));
    }

    /// H-1 regression: the queue-depth count must happen-before the
    /// frame becomes visible (see `enqueue_frame`). The old `try_send` +
    /// count-after shape wrapped `current` to `usize::MAX` whenever the
    /// engine dequeued the frame in the gap while the queue was empty —
    /// the next enqueue's `+ 1` then panicked in debug and killed the
    /// transport task (CI rig-gate + local repro: 26/27). This test runs
    /// the exact shape that fired it: the queue is EMPTY at every send
    /// (the producer waits for `current == 0` before sending the next
    /// frame) while the consumer spins on the engine's zero-timeout
    /// polls, so a dequeue landing in the send/count gap is likely —
    /// on the old code it panics with "attempt to add with overflow";
    /// on the fixed code the books balance exactly.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn queue_depth_count_before_publish_regression() {
        use std::sync::atomic::Ordering as O;
        const FRAMES: u64 = 30_000;
        let (tx, rx) = mpsc::channel::<StreamFrame>(FRAME_QUEUE_CAPACITY);
        let depth = Arc::new(QueueDepth::default());
        let rx = Arc::new(AsyncMutex::new(rx));

        // Producer: one frame at a time through the production helper,
        // waiting for the queue to drain to empty before the next send
        // (the racy shape — a tight loop keeps the queue full, which
        // hides the wrap).
        let prod_depth = Arc::clone(&depth);
        let producer = tokio::spawn(async move {
            let mut n: u16 = 0;
            for _ in 0..FRAMES {
                n = n.wrapping_add(1);
                let frame = StreamFrame::new(
                    n,
                    (n as u32).wrapping_mul(5000),
                    rsh_protocol::StreamFlags::default(),
                    &[],
                    &[0; 4],
                    0,
                );
                enqueue_frame(&tx, &prod_depth, frame);
                // Wait for the consumer to drain it (bounded: a wrapped
                // counter never returns to 0, so bail out loudly instead
                // of hanging — the panic in the producer already fails
                // the test in that case).
                let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
                while prod_depth.current.load(O::Relaxed) != 0 {
                    assert!(
                        tokio::time::Instant::now() <= deadline,
                        "queue did not drain to empty within 5 s"
                    );
                    tokio::task::yield_now().await;
                }
            }
        });

        // Consumer: the engine's drain shape — zero-timeout polls,
        // spinning (no sleep) so it is polling when the next frame
        // lands, exactly like the 200 Hz tick polling an empty queue.
        let mut src = QueuedSource {
            rx: Arc::clone(&rx),
            depth: Arc::clone(&depth),
        };
        let mut drained = 0u64;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        while drained < FRAMES && tokio::time::Instant::now() < deadline {
            let mut got_this_tick = 0;
            while got_this_tick < 32 {
                match timeout(Duration::ZERO, src.next_frame()).await {
                    Ok(Some(_)) => {
                        drained += 1;
                        got_this_tick += 1;
                    }
                    _ => break,
                }
            }
            tokio::task::yield_now().await;
        }
        producer.await.expect("producer alive");
        // Drain whatever is left, then the books must balance exactly.
        while let Ok(Some(_)) = timeout(Duration::from_millis(100), src.next_frame()).await {
            drained += 1;
        }
        let dropped = depth.dropped.load(O::Relaxed);
        let cur = depth.current.load(O::Relaxed);
        assert_eq!(
            drained + dropped,
            FRAMES,
            "every frame is either drained or counted dropped (drained {drained}, dropped {dropped})"
        );
        assert_eq!(cur, 0, "queue must be empty after a full drain");
        assert_eq!(dropped, 0, "a draining consumer must not drop frames");
    }
}
