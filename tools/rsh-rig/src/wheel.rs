//! The virtual wheel: a seeded, deterministic model of the ESP32
//! board's stream output (TESTING-INPUT.md §8.2).
//!
//! The wheel is pure logic — no tokio, no channels — so a scenario's
//! frame stream is a pure function of its config and seed. The
//! harness task owns the timing (200 Hz), the channels, and the ping
//! replies; this module only decides *what* the board would send on
//! each tick.
//!
//! Realism modelled here (all seeded):
//! ADC Gaussian noise + slow random-walk drift + occasional
//! high-variance floating bursts; button bounce on edges; encoder
//! glitches (±1 spurious, rare ±50..200 jumps, logged as
//! `enc-glitch`); Gilbert–Elliott bursty loss, reordering, duplicates,
//! interference drop windows; clock drift (ppm); faults
//! (reboot / wifi-drop / power-cut); seq + timestamp wraps; fuzz
//! (corrupt CRC / truncated / random garbage); and the auth gate —
//! the wheel emits no stream frames until a valid HMAC-verified
//! `GET_CAPS` control frame opens it (SEC-E-14 mirror).

use std::collections::VecDeque;

use rsh_protocol::stream::{StatusPacket, StreamFlags, StreamFrame};

use crate::rng::Rng;

/// What the wheel is driving on this run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WheelKind {
    /// Fixture-like sweeps (soak / dropout / chaos / fuzz / faults).
    Soak,
    /// Full-range triangle sweeps (steering + pedals hit both rails).
    Calibration,
    /// Soak motion with a mid-run silence (watchdog must fire).
    SafeMode,
    /// Soak motion with the accelerator slot omitted.
    Modules,
    /// Soak motion plus a full-range clutch sweep.
    Clutch,
    /// Gear bits 5–10 cycle 1..6 (shifter scenarios).
    Shifter,
    /// Random jerks, pedal slams, button mash, short dropouts.
    UserChaos,
}

/// A board fault, scheduled at `at` seconds into the run.
#[derive(Clone, Copy, Debug)]
pub enum Fault {
    /// 2 s silence, then seq 0, ts reset, encoders reset.
    Reboot(f64),
    /// `dur` seconds of silence; seq keeps counting (gap on resume).
    WifiDrop(f64, f64),
    /// Like [`Fault::Reboot`] (power loss is a reboot).
    /// Only used by tests today; kept for the power-cut scenario.
    #[allow(dead_code)]
    PowerCut(f64),
}

/// Static configuration for one wheel run.
#[derive(Clone, Debug)]
#[allow(clippy::struct_excessive_bools)]
pub struct WheelConfig {
    /// Seed for all randomness in this run.
    pub seed: u64,
    /// Motion profile.
    pub kind: WheelKind,
    /// ADC noise + drift on.
    pub noise: bool,
    /// Independent per-packet drop probability (dropout-storm).
    pub loss_pct: f64,
    /// Per-packet duplicate probability.
    pub duplicate_pct: f64,
    /// Per-packet bad-CRC probability.
    pub bad_crc_pct: f64,
    /// Fuzz mode: a share of ticks emit corrupt/truncated/garbage.
    pub fuzz: bool,
    /// Gilbert–Elliott bursty loss + reorder + interference.
    pub net_chaos: bool,
    /// Board clock drift in ppm (default ~100).
    pub clock_drift_ppm: f64,
    /// Starting stream sequence (wrap tests start near 65500).
    pub seq_start: u16,
    /// Starting board timestamp in µs (wrap tests start near `u32::MAX`).
    pub ts_start: u32,
    /// Scheduled fault, if any.
    pub fault: Option<Fault>,
    /// Silence window `(start_s, dur_s)` (safe-mode scenario).
    pub silence: Option<(f64, f64)>,
    /// Axis slots the board does not sample (omitted from frames).
    pub disable_slots: Vec<usize>,
    /// Inject encoder glitches (spurious ±1, rare big jumps).
    pub encoder_glitch: bool,
    /// Per-tick probability of a big (±50..200) encoder jump when
    /// [`WheelConfig::encoder_glitch`] is on.
    pub glitch_big_rate: f64,
    /// Stream only after a valid `GET_CAPS` (auth gate, default on).
    pub require_auth: bool,
    /// Base simulated network delay for ping replies (µs).
    pub ping_delay_base_us: u64,
    /// Ping delay jitter σ (µs).
    pub ping_delay_jitter_us: u64,
    /// Wheel emit period (µs). Default 5100 = 196 Hz — a DOCUMENTED
    /// HARNESS LIMITATION (H-2), not a product rate: the engine ticks
    /// at 200 Hz and the OLD drain took at most one frame per tick
    /// (latest-wins), so a wheel emitting at exactly 200 Hz coalesced
    /// ~3–14% of frames by phase drift and the soak SLOs (loss < 0.5%,
    /// coalesced ≤ 0.5%) could not be read off it. The Round 2
    /// multi-frame drain processes every frame (deep-scan gate:
    /// emit-200hz coalesced 0% at 200 Hz), so the 196 Hz default is
    /// kept as the documented soak default, not because the drain
    /// needs it. At 196 Hz at most one frame lands per tick window,
    /// so every emit is drained exactly once regardless of phase,
    /// and the 195/s rate SLO still holds. The true 200 Hz
    /// coalescing rate is measured by the `emit-200hz` scenario
    /// (5000 µs), which reports `coalescedPercent` with no ≤ 0.5%
    /// assertion on it.
    pub tick_interval_us: u64,
}

impl Default for WheelConfig {
    fn default() -> Self {
        Self {
            seed: 42,
            kind: WheelKind::Soak,
            noise: true,
            loss_pct: 0.0,
            duplicate_pct: 0.0,
            bad_crc_pct: 0.0,
            fuzz: false,
            net_chaos: false,
            clock_drift_ppm: 100.0,
            seq_start: 0,
            ts_start: 0,
            fault: None,
            silence: None,
            disable_slots: Vec::new(),
            encoder_glitch: false,
            glitch_big_rate: 0.0005,
            require_auth: true,
            ping_delay_base_us: 300,
            ping_delay_jitter_us: 150,
            tick_interval_us: 5100,
        }
    }
}

/// What the wheel produced on one 5 ms tick.
#[derive(Clone, Debug, Default)]
pub struct TickOut {
    /// Wire-encoded stream frames to deliver this tick (usually 0–1;
    /// more when duplicates or held reorder frames release).
    pub frames: Vec<Vec<u8>>,
    /// 1 Hz status packet, when due.
    pub status: Option<StatusPacket>,
}

/// End-of-run wheel summary (shared with the harness).
#[derive(Clone, Debug)]
pub struct WheelSummary {
    /// Accepted control frames (`GET_CAPS` verifies).
    pub ctrl_accepted: u64,
    /// Stream frames generated (before net loss).
    pub frames_generated: u64,
    /// Frames dropped by net-chaos / loss simulation.
    pub frames_dropped_net: u64,
    /// Frames sent with a corrupted CRC.
    pub bad_crc_sent: u64,
    /// Duplicate frames sent.
    pub duplicates_sent: u64,
    /// Fuzz packets sent.
    pub fuzz_sent: u64,
    /// Encoder glitch jumps injected.
    pub glitches: u64,
    /// Frames withheld by the closed auth gate.
    pub gated_dropped: u64,
    /// Wheel-side seq-gap / tx-fail counters (status packet).
    pub seq_gap: u64,
    /// Wheel-side tx-fail counter (status packet).
    pub tx_fail: u64,
    /// Last reset reason (1 power-on, 4 software).
    pub reset_reason: u8,
    /// Timestamped event log, one line per event. A single
    /// pre-reserved `String` (8 KiB, reserved at construction —
    /// BEFORE the run's `reset_counts()`), appended to in place:
    /// logging an event during the run allocates nothing, so the
    /// rig's `allocBytesDelta` stays a product measurement, not a
    /// harness-logging artefact (M-1). Lines past 8 KiB are dropped
    /// (counted in `events_dropped`). Read it back with [`Self::events`].
    pub events_log: String,
    /// Events dropped because the log buffer was full.
    pub events_dropped: u64,
}

impl Default for WheelSummary {
    fn default() -> Self {
        Self {
            ctrl_accepted: 0,
            frames_generated: 0,
            frames_dropped_net: 0,
            bad_crc_sent: 0,
            duplicates_sent: 0,
            fuzz_sent: 0,
            glitches: 0,
            gated_dropped: 0,
            seq_gap: 0,
            tx_fail: 0,
            reset_reason: 0,
            events_log: String::with_capacity(8192),
            events_dropped: 0,
        }
    }
}

impl WheelSummary {
    /// Append one event line to the pre-reserved log (no allocation
    /// while capacity remains — see the `events_log` docs).
    pub fn log_event(&mut self, args: std::fmt::Arguments<'_>) {
        use std::fmt::Write as _;
        if self.events_log.len() > 8000 {
            self.events_dropped += 1;
            return;
        }
        let _ = self.events_log.write_fmt(args);
        self.events_log.push('\n');
    }

    /// Event lines, oldest first. Allocates at call time — the
    /// harness calls it AFTER reading `heap_end`, so the allocation
    /// never lands in the measured `allocBytesDelta` window (M-1).
    #[must_use]
    pub fn events(&self) -> Vec<String> {
        self.events_log.lines().map(str::to_string).collect()
    }
}

/// The virtual wheel: deterministic board model.
pub struct VirtualWheel {
    cfg: WheelConfig,
    rng: Rng,
    /// Current stream sequence.
    seq: u16,
    /// Board timestamp accumulator (µs, f64 for drift precision).
    ts_us: f64,
    /// Tick index (5 ms each).
    tick: u64,
    /// Cumulative encoder counts.
    encoders: [i16; 4],
    /// Random-walk drift per axis slot (mV, capped ±15).
    drift: [f64; 8],
    /// Previous button word (edge detection for bounce).
    prev_buttons: u32,
    /// Active bounce: (bit, ticks left).
    bounce: Option<(u32, u32)>,
    /// Gilbert–Elliott state: true = bad (lossy) state.
    ge_bad: bool,
    /// Held frames for reorder: (release tick, bytes).
    reorder: VecDeque<(u64, Vec<u8>)>,
    /// Next interference drop window (start tick, end tick).
    interference: Option<(u64, u64)>,
    /// Auth gate closed until a valid `GET_CAPS` arrives.
    gated: bool,
    /// Fault already fired (one-shot faults).
    fault_done: bool,
    /// Board reboot in progress until this tick (silence).
    reboot_until_tick: u64,
    /// Wi-Fi drop in progress until this tick (silence).
    drop_until_tick: u64,
    /// Floating-burst noise until this tick (high-variance ADC).
    float_until_tick: u64,
    /// Status packet sequence.
    status_seq: u16,
    /// Summary counters.
    pub summary: WheelSummary,
}

impl VirtualWheel {
    /// Build a wheel from its config.
    #[must_use]
    pub fn new(cfg: WheelConfig) -> Self {
        let seed = cfg.seed;
        Self {
            gated: cfg.require_auth,
            seq: cfg.seq_start,
            ts_us: f64::from(cfg.ts_start),
            summary: WheelSummary {
                reset_reason: 1,
                ..WheelSummary::default()
            },
            cfg,
            rng: Rng::new(seed),
            tick: 0,
            encoders: [0; 4],
            drift: [0.0; 8],
            prev_buttons: 0,
            bounce: None,
            ge_bad: false,
            reorder: VecDeque::new(),
            interference: None,
            fault_done: false,
            reboot_until_tick: 0,
            drop_until_tick: 0,
            float_until_tick: 0,
            status_seq: 0,
        }
    }

    /// Whether the auth gate is currently open (rig diagnostics and
    /// tests; the harness itself reads `ctrl_accepted` instead).
    #[allow(dead_code)]
    #[must_use]
    pub fn gate_open(&self) -> bool {
        !self.gated
    }

    /// Wheel emit period (µs) — [`WheelConfig::tick_interval_us`]:
    /// 5100 (196 Hz) by default, 5000 for the `emit-200hz` probe.
    #[must_use]
    pub fn tick_interval_us(&self) -> u64 {
        self.cfg.tick_interval_us
    }

    /// Elapsed board seconds (tick * 5 ms) — public for the harness
    /// task's drain-cap diagnostics.
    #[must_use]
    pub fn now_s_pub(&self) -> f64 {
        self.now_s()
    }

    /// Current board time in µs (the clock ping replies stamp `t1`/`t2`
    /// with). Only differences are ever used, so the epoch is the
    /// wheel's own.
    #[must_use]
    pub fn board_now_us(&self) -> u64 {
        self.ts_us as u64
    }

    /// Simulated network delay for one ping reply: base + Gaussian
    /// jitter (clamped ≥ 50 µs). No injected spikes: the ping SLO
    /// measures the engine's ping path, not an invented network.
    #[must_use]
    pub fn ping_delay(&mut self) -> std::time::Duration {
        let jitter = self
            .rng
            .gaussian_ms(0.0, self.cfg.ping_delay_jitter_us as f64);
        let delay_us = self.cfg.ping_delay_base_us as f64 + jitter;
        std::time::Duration::from_micros(delay_us.clamp(50.0, 50_000.0) as u64)
    }

    /// Handle one inbound control frame (bytes). A valid CRC-verified
    /// `GET_CAPS` opens the gate (v2.2: plain, no HMAC); anything else
    /// is ignored.
    pub fn handle_control(&mut self, bytes: &[u8]) {
        use rsh_protocol::control::ControlFrame;
        let Ok(frame) = ControlFrame::verify(bytes) else {
            return;
        };
        if frame.cmd == 0x01 {
            // GET_CAPS.
            self.summary.ctrl_accepted += 1;
            if self.gated {
                self.gated = false;
                let now_s = self.now_s();
                self.summary
                    .log_event(format_args!("t={now_s:.1}s auth gate opened (GET_CAPS)"));
            }
        }
    }

    /// Elapsed board seconds (tick * 5 ms).
    fn now_s(&self) -> f64 {
        self.tick as f64 * 0.005
    }

    /// Advance one 5 ms tick; returns the frames/status for this tick.
    /// The board clock advances even during silence (the board keeps
    /// sampling; only the air is quiet) except across a reboot, which
    /// resets seq/ts/encoders.
    #[must_use]
    pub fn next_tick(&mut self) -> TickOut {
        self.tick += 1;
        let now_s = self.now_s();
        let mut out = TickOut::default();

        // Fault schedule (one-shot).
        if !self.fault_done
            && let Some(fault) = self.cfg.fault
        {
            let (at, kind) = match fault {
                Fault::Reboot(at) => (at, "reboot"),
                Fault::WifiDrop(at, _) => (at, "wifi-drop"),
                Fault::PowerCut(at) => (at, "power-cut"),
            };
            if now_s >= at {
                self.fault_done = true;
                match fault {
                    Fault::Reboot(_) | Fault::PowerCut(_) => {
                        self.reboot_until_tick = self.tick + 400; // 2 s.
                        self.summary.log_event(format_args!(
                            "t={now_s:.1}s fault {kind}: 2 s silence, seq/ts reset after"
                        ));
                    }
                    Fault::WifiDrop(_, dur) => {
                        let ticks = (dur * 200.0) as u64;
                        self.drop_until_tick = self.tick + ticks;
                        self.summary.log_event(format_args!(
                            "t={now_s:.1}s fault wifi-drop: {dur} s silence, seq continues"
                        ));
                    }
                }
            }
        }

        // Reboot completes: reset board state.
        if self.reboot_until_tick != 0 && self.tick >= self.reboot_until_tick {
            self.reboot_until_tick = 0;
            self.seq = 0;
            self.ts_us = 0.0;
            self.encoders = [0; 4];
            self.summary.reset_reason = 4;
            self.summary.log_event(format_args!(
                "t={now_s:.1}s board back after reboot (seq 0, ts 0)"
            ));
        }

        // Advance the board clock (drift applied) every tick.
        self.ts_us += 5000.0 * (1.0 + self.cfg.clock_drift_ppm / 1_000_000.0);

        // Silence windows: reboot, wifi-drop, safe-mode, user-chaos
        // dropouts. No frame is emitted, but the clock keeps running
        // and (except across a reboot) the sequence keeps counting.
        let in_silence = self.tick < self.reboot_until_tick
            || self.tick < self.drop_until_tick
            || self
                .cfg
                .silence
                .is_some_and(|(start, dur)| now_s >= start && now_s < start + dur)
            || (self.cfg.kind == WheelKind::UserChaos && self.user_chaos_dropout());
        if in_silence {
            // The board still sampled: sequence advances while quiet.
            if self.tick >= self.reboot_until_tick {
                self.seq = self.seq.wrapping_add(1);
            }
            return self.maybe_status(out);
        }

        // Motion + noise + glitches → one stream frame.
        let frame = self.build_frame();
        self.summary.frames_generated += 1;
        let mut bytes = frame.encode();

        // Fuzz replaces the valid frame for this tick.
        if self.cfg.fuzz {
            let r = self.rng.next_f64();
            if r < 0.15 {
                // Corrupt-CRC frame (structurally valid).
                let n = bytes.len();
                bytes[n - 1] ^= 0xFF;
                self.summary.bad_crc_sent += 1;
                self.summary.fuzz_sent += 1;
                out.frames.push(bytes);
                self.seq = self.seq.wrapping_add(1);
                return self.maybe_status(out);
            }
            if r < 0.25 {
                // Truncated frame.
                let cut = 5 + (self.rng.next_u64() as usize % bytes.len().saturating_sub(6));
                bytes.truncate(cut);
                self.summary.fuzz_sent += 1;
                out.frames.push(bytes);
                self.seq = self.seq.wrapping_add(1);
                return self.maybe_status(out);
            }
            if r < 0.30 {
                // Random garbage.
                let len = 10 + (self.rng.next_u64() as usize % 30);
                let garbage: Vec<u8> = (0..len).map(|_| self.rng.next_u64() as u8).collect();
                self.summary.fuzz_sent += 1;
                out.frames.push(garbage);
                self.seq = self.seq.wrapping_add(1);
                return self.maybe_status(out);
            }
        }

        // Auth gate: nothing leaves the board until GET_CAPS.
        if self.gated {
            self.summary.gated_dropped += 1;
            self.seq = self.seq.wrapping_add(1);
            return self.maybe_status(out);
        }

        // Net chaos / loss: the frame never leaves the board.
        if self.drop_this_tick() {
            self.summary.frames_dropped_net += 1;
            self.summary.seq_gap += 1;
            self.summary.tx_fail += 1;
            self.seq = self.seq.wrapping_add(1);
            // Still release any held reorder frames.
            out.frames.append(&mut self.release_due());
            return self.maybe_status(out);
        }

        // Bad CRC (dropout-storm): sent, but the PC must reject it.
        if self.rng.next_f64() < self.cfg.bad_crc_pct {
            let n = bytes.len();
            bytes[n - 1] ^= 0xFF;
            self.summary.bad_crc_sent += 1;
        }

        // Reorder: hold this frame 1–3 ticks (net chaos only).
        if self.cfg.net_chaos && self.rng.chance(0.05) {
            let hold = 1 + (self.rng.next_u64() % 3);
            self.reorder.push_back((self.tick + hold, bytes));
        } else {
            out.frames.push(bytes);
        }
        out.frames.append(&mut self.release_due());

        // Duplicates: send the frame twice.
        if self.rng.next_f64() < self.cfg.duplicate_pct {
            let dup = out.frames.last().cloned();
            if let Some(d) = dup {
                out.frames.push(d);
                self.summary.duplicates_sent += 1;
            }
        }

        self.seq = self.seq.wrapping_add(1);
        self.maybe_status(out)
    }

    /// Attach the 1 Hz status packet when due.
    fn maybe_status(&mut self, mut out: TickOut) -> TickOut {
        if self.tick.is_multiple_of(200) {
            let pkt = StatusPacket {
                seq: self.status_seq,
                uptime_ms: u32::try_from(self.tick * 5).unwrap_or(u32::MAX),
                rssi: -52,
                battery_pct: 255,
                supply_mv: 5000,
                heap: 200_000,
                overruns: 0,
                max_cycle_us: 3500,
                governor: 0,
                reset_reason: self.summary.reset_reason,
                cfg_crc: 0,
                cfg_ver: 1,
                seq_gap: u16::try_from(self.summary.seq_gap).unwrap_or(u16::MAX),
                tx_fail: u16::try_from(self.summary.tx_fail).unwrap_or(u16::MAX),
                btns_ex: None,
            };
            self.status_seq = self.status_seq.wrapping_add(1);
            out.status = Some(pkt);
        }
        out
    }

    /// Frames held for reorder whose release tick has arrived.
    /// Capped at 64 per tick (L-1): the reorder queue is bounded by
    /// construction (5% hold chance, 1–3 tick hold ≈ ≤ 1 frame/tick
    /// in practice), so the cap never binds on a healthy run — it is
    /// a backstop so a pathological queue cannot stall the 5 ms tick.
    /// Unreleased frames stay queued for the next tick.
    fn release_due(&mut self) -> Vec<Vec<u8>> {
        let mut released = Vec::new();
        while released.len() < 64
            && self
                .reorder
                .front()
                .is_some_and(|(tick, _)| *tick <= self.tick)
        {
            if let Some((_, bytes)) = self.reorder.pop_front() {
                released.push(bytes);
            }
        }
        released
    }

    /// Gilbert–Elliott + interference + iid loss decision for this tick.
    fn drop_this_tick(&mut self) -> bool {
        // IID loss (dropout-storm).
        if self.rng.next_f64() < self.cfg.loss_pct {
            return true;
        }
        if !self.cfg.net_chaos {
            return false;
        }
        // Interference: every ~7 s a 200 ms drop window.
        if self.interference.is_none() && self.rng.chance(0.0007) {
            self.interference = Some((self.tick, self.tick + 40));
            let now_s = self.now_s();
            self.summary.log_event(format_args!(
                "t={now_s:.1}s interference: 200 ms drop window"
            ));
        }
        if let Some((start, end)) = self.interference {
            if self.tick >= start && self.tick < end {
                if self.tick + 1 >= end {
                    self.interference = None;
                }
                return true;
            }
            if self.tick >= end {
                self.interference = None;
            }
        }
        // Gilbert–Elliott: good 0.2% loss, bad 80% loss, mean burst 4.
        if self.ge_bad {
            if self.rng.chance(0.25) {
                self.ge_bad = false;
            }
            self.rng.chance(0.80)
        } else {
            if self.rng.chance(0.005) {
                self.ge_bad = true;
            }
            self.rng.chance(0.002)
        }
    }

    /// User-chaos short dropouts: ~0.5 s of silence every ~5 s.
    fn user_chaos_dropout(&mut self) -> bool {
        // Deterministic-ish pattern from the tick index: a 100-tick
        // (0.5 s) dropout starting every 1000 ticks, offset by the
        // seed so runs differ.
        let offset = self.cfg.seed % 1000;
        let phase = (self.tick + offset) % 1000;
        (500..600).contains(&phase)
    }

    /// Build the stream frame for this tick: motion + noise + drift +
    /// bounce + encoder steps/glitches.
    fn build_frame(&mut self) -> StreamFrame {
        let (axes, buttons, enc_step) = self.motion();
        let mut axes_mv = axes;

        // ADC noise + drift + floating bursts — EXCEPT on calibration
        // sweeps (Calibration / Clutch kinds): a rail-measurement
        // stimulus must present exact rails. The drift alone (±15 mV
        // random walk) exceeds the rail tolerance (100/32767 of the
        // 3000 mV span ≈ 9 mV), and a random 2 s floating burst over
        // the single bottom dwell in a 20 s window held the filter
        // ~2% off the rail (H2: steering min 2619 / 824 / 417 across
        // identical-seed runs — the threshold sits inside the noise
        // spread, so the scenario flaked). Thresholds are unchanged;
        // the stimulus is exact instead. Noise/fault behaviour is
        // still covered by soak, dropout-storm, chaos-net and fuzz.
        if self.cfg.noise && !matches!(self.cfg.kind, WheelKind::Calibration | WheelKind::Clutch) {
            let floating = self.tick < self.float_until_tick;
            if !floating && self.rng.chance(0.00025) {
                self.float_until_tick = self.tick + 400; // ~2 s burst.
                let now_s = self.now_s();
                self.summary
                    .log_event(format_args!("t={now_s:.1}s floating sensor burst (2 s)"));
            }
            let sigma_steer = if floating { 80.0 } else { 3.0 };
            let sigma_pedal = if floating { 80.0 } else { 5.0 };
            for (slot, v) in axes_mv.iter_mut().enumerate() {
                let Some(mv) = v else { continue };
                let sigma = if slot == 0 { sigma_steer } else { sigma_pedal };
                // Slow random-walk drift, capped at ±15 mV.
                self.drift[slot] =
                    (self.drift[slot] + self.rng.gaussian() * 0.5).clamp(-15.0, 15.0);
                let noisy = f64::from(*mv) + self.rng.gaussian() * sigma + self.drift[slot];
                *mv = noisy.clamp(0.0, 3300.0) as u16;
            }
        }

        // Button bounce: on an edge, sometimes chatter for 2–4 ticks.
        let mut buttons = buttons;
        let edges = buttons ^ self.prev_buttons;
        if edges != 0 && self.bounce.is_none() && self.rng.chance(0.5) {
            let bit = edges.trailing_zeros();
            let dur = 2 + (self.rng.next_u64() % 3) as u32;
            self.bounce = Some((bit, dur));
        }
        if let Some((bit, left)) = self.bounce {
            buttons ^= 1u32 << bit;
            if left <= 1 {
                self.bounce = None;
            } else {
                self.bounce = Some((bit, left - 1));
            }
        }
        self.prev_buttons = buttons;

        // Encoders: normal steps + glitches.
        for (i, step) in enc_step.iter().enumerate() {
            self.encoders[i] = self.encoders[i].wrapping_add(*step);
        }
        if self.cfg.encoder_glitch {
            // Spurious ±1.
            if self.rng.chance(0.005) {
                let idx = (self.rng.next_u64() % 4) as usize;
                let d: i16 = if self.rng.chance(0.5) { 1 } else { -1 };
                self.encoders[idx] = self.encoders[idx].wrapping_add(d);
            }
            // Rare big jump (±50..200) — logged; the engine is
            // expected to filter |delta| > 32/tick (TESTING-INPUT §8.4).
            if self.rng.chance(self.cfg.glitch_big_rate) {
                let idx = (self.rng.next_u64() % 4) as usize;
                let mag = self.rng.range_i64(50, 200);
                let d = if self.rng.chance(0.5) { mag } else { -mag } as i16;
                self.encoders[idx] = self.encoders[idx].wrapping_add(d);
                self.summary.glitches += 1;
                let now_s = self.now_s();
                self.summary.log_event(format_args!(
                    "t={now_s:.1}s enc-glitch: encoder {idx} jump {d:+}"
                ));
            }
        }

        let axes_pairs: Vec<(usize, u16)> = axes_mv
            .iter()
            .enumerate()
            .filter_map(|(i, v)| v.map(|mv| (i, mv)))
            .collect();
        // The board timestamp is a u32 µs clock (~71.6 min) and MUST
        // wrap. `ts_us as u32` does NOT wrap — Rust float→int casts
        // SATURATE — so a wheel started near u32::MAX (long-soak
        // `ts_start`) froze every timestamp at u32::MAX after ~2
        // ticks: deltas of 0, engine jitter exactly 5000, and the
        // wrap the scenario exists to test never happened. Wrap
        // explicitly through u64 (the accumulator itself stays
        // monotonic: `board_now_us` feeds the ping t1/t2 stamps,
        // which are u64 and must not wrap).
        let timestamp_us = (self.ts_us as u64 % (1u64 << 32)) as u32;
        StreamFrame::new(
            self.seq,
            timestamp_us,
            StreamFlags::default(),
            &axes_pairs,
            &self.encoders,
            buttons,
        )
    }

    /// Motion profile for this tick: axis slots, buttons, encoder steps.
    /// Returns `(axes_mv[8], buttons, enc_step[4])`.
    fn motion(&mut self) -> ([Option<u16>; 8], u32, [i16; 4]) {
        let i = self.tick;
        let mut axes: [Option<u16>; 8] = [None; 8];
        let disabled = |slot: usize| self.cfg.disable_slots.contains(&slot);
        // Fixture-like closed-form sweeps (deterministic).
        let steering_soak = 1100 + ((i.wrapping_mul(73)) % 1401) as u16;
        let accel_soak = 300 + ((i.wrapping_mul(131)) % 3001) as u16;
        let brake_soak = 300 + ((i.wrapping_mul(97)) % 3001) as u16;
        let handbrake_soak = 300 + ((i.wrapping_mul(57)) % 3001) as u16;
        let buttons_soak: u32 = if i.is_multiple_of(7) { 0b101 } else { 0b001 };
        let enc_step: [i16; 4] = [2, -1, 0, 0];

        // Trapezoid wave, 12 s period: 5 s ramp, 1 s dwell at each
        // rail. A sharp triangle reversal never lets the pipeline's
        // smoothing/ramp settle at the rail, so a calibration sweep
        // dwells there instead — like a user holding full lock while
        // calibrating. Shared by Calibration (all axes) and Modules
        // (steering only — the "other modules still work" check needs
        // a full-range steering sweep, not the 1100..2500 mV soak
        // sawtooth, which can never reach axis 30000).
        let trap = |phase: u64| -> u16 {
            let p = (i + phase) % 2400;
            let v = if p < 1000 {
                p
            } else if p < 1200 {
                1000
            } else {
                2200_u64.saturating_sub(p)
            };
            (300 + v * 3) as u16
        };

        match self.cfg.kind {
            WheelKind::Soak | WheelKind::SafeMode => {
                if !disabled(0) {
                    axes[0] = Some(steering_soak);
                }
                if !disabled(1) {
                    axes[1] = Some(accel_soak);
                }
                if !disabled(2) {
                    axes[2] = Some(brake_soak);
                }
                if !disabled(4) {
                    axes[4] = Some(handbrake_soak);
                }
                (axes, buttons_soak, enc_step)
            }
            WheelKind::Modules => {
                if !disabled(0) {
                    axes[0] = Some(trap(0));
                }
                if !disabled(1) {
                    axes[1] = Some(accel_soak);
                }
                if !disabled(2) {
                    axes[2] = Some(brake_soak);
                }
                if !disabled(4) {
                    axes[4] = Some(handbrake_soak);
                }
                (axes, buttons_soak, enc_step)
            }
            WheelKind::Calibration => {
                if !disabled(0) {
                    axes[0] = Some(trap(0));
                }
                if !disabled(1) {
                    axes[1] = Some(trap(500));
                }
                if !disabled(2) {
                    axes[2] = Some(trap(1000));
                }
                if !disabled(4) {
                    // Handbrake trapezoid too (M-10): the old
                    // sawtooth touched each rail for ONE tick, so
                    // with coalescing + pipeline smoothing the
                    // output never reached the rails (baseline:
                    // handbrake min 602 / max 30987 in
                    // calibration-span — both outside the rail
                    // assertions). The trapezoid dwells 1 s at each
                    // rail like every other calibration axis, so the
                    // filter settles and the rails are observable.
                    axes[4] = Some(trap(1500));
                }
                (axes, buttons_soak, enc_step)
            }
            WheelKind::Clutch => {
                if !disabled(0) {
                    axes[0] = Some(steering_soak);
                }
                if !disabled(1) {
                    axes[1] = Some(accel_soak);
                }
                if !disabled(2) {
                    axes[2] = Some(brake_soak);
                }
                if !disabled(3) {
                    // Clutch trapezoid, 12 s period, full range with
                    // 1 s rail dwells (same reason as calibration).
                    let p = i % 2400;
                    let v = if p < 1000 {
                        p
                    } else if p < 1200 {
                        1000
                    } else {
                        2200_u64.saturating_sub(p)
                    };
                    axes[3] = Some((300 + v * 3) as u16);
                }
                if !disabled(4) {
                    axes[4] = Some(handbrake_soak);
                }
                (axes, buttons_soak, enc_step)
            }
            WheelKind::Shifter => {
                if !disabled(0) {
                    axes[0] = Some(steering_soak);
                }
                if !disabled(1) {
                    axes[1] = Some(accel_soak);
                }
                if !disabled(2) {
                    axes[2] = Some(brake_soak);
                }
                if !disabled(4) {
                    axes[4] = Some(handbrake_soak);
                }
                // One gear bit at a time, 1 s per gear, gears 1..6.
                let gear = (i / 200) % 6 + 1;
                let buttons: u32 = 1u32 << (4 + gear);
                (axes, buttons, enc_step)
            }
            WheelKind::UserChaos => {
                // Random jerks / pedal slams / button mash.
                if !disabled(0) {
                    axes[0] = Some(self.rng.range_i64(300, 3300) as u16);
                }
                if !disabled(1) {
                    axes[1] = Some(self.rng.range_i64(300, 3300) as u16);
                }
                if !disabled(2) {
                    axes[2] = Some(self.rng.range_i64(300, 3300) as u16);
                }
                if !disabled(4) {
                    axes[4] = Some(self.rng.range_i64(300, 3300) as u16);
                }
                let buttons: u32 = (self.rng.next_u64() & 0x7FF) as u32;
                let enc_step: [i16; 4] = [
                    self.rng.range_i64(-3, 3) as i16,
                    self.rng.range_i64(-3, 3) as i16,
                    0,
                    0,
                ];
                (axes, buttons, enc_step)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{VirtualWheel, WheelConfig, WheelKind};

    #[test]
    fn gated_wheel_emits_nothing_until_get_caps() {
        let mut wheel = VirtualWheel::new(WheelConfig::default());
        for _ in 0..10 {
            let out = wheel.next_tick();
            assert_eq!(out.frames.len(), 0);
        }
        // Open the gate with a real GET_CAPS frame (v2.2: plain, CRC).
        let frame = rsh_protocol::control::build_frame(0x01, 1, b"").expect("builds");
        wheel.handle_control(&frame);
        assert!(wheel.gate_open());
        let out = wheel.next_tick();
        assert_eq!(out.frames.len(), 1);
    }

    #[test]
    fn seq_wraps_cleanly() {
        let mut wheel = VirtualWheel::new(WheelConfig {
            seq_start: 65534,
            require_auth: false,
            ..WheelConfig::default()
        });
        let mut seqs = Vec::new();
        for _ in 0..5 {
            let out = wheel.next_tick();
            for bytes in &out.frames {
                let f = rsh_protocol::StreamFrame::parse(bytes).expect("parses");
                seqs.push(f.seq);
            }
        }
        assert_eq!(seqs, vec![65534, 65535, 0, 1, 2]);
    }

    #[test]
    fn board_timestamp_wraps_instead_of_saturating() {
        // Regression: `ts_us as u32` saturates at u32::MAX, so a wheel
        // started near the wrap froze every timestamp at u32::MAX
        // (long-soak engine jitter exactly 5000, and the u32 wrap the
        // scenario exists to test never happened). The timestamp must
        // wrap: from ts_start 4_294_960_000 the frames step 5000.5 µs
        // and cross u32::MAX within ~2 ticks.
        let mut wheel = VirtualWheel::new(WheelConfig {
            ts_start: 4_294_960_000,
            require_auth: false,
            ..WheelConfig::default()
        });
        let mut stamps = Vec::new();
        for _ in 0..5 {
            let out = wheel.next_tick();
            for bytes in &out.frames {
                let f = rsh_protocol::StreamFrame::parse(bytes).expect("parses");
                stamps.push(f.timestamp_us);
            }
        }
        assert_eq!(stamps.len(), 5);
        // The wrap happened: a later timestamp is smaller than an
        // earlier one, and no timestamp froze at u32::MAX.
        assert!(
            stamps.windows(2).any(|w| w[1] < w[0]),
            "no wrap in {stamps:?}"
        );
        assert!(
            !stamps.contains(&u32::MAX),
            "timestamp saturated: {stamps:?}"
        );
        // Signed deltas across the wrap stay ~one tick (5000.5 µs).
        for w in stamps.windows(2) {
            let d = rsh_engine::engine::signed_timestamp_delta_us(w[1], w[0]);
            assert!(
                (4999..=5002).contains(&d),
                "delta {d} across wrap: {stamps:?}"
            );
        }
    }

    #[test]
    fn reboot_resets_seq_and_ts() {
        let mut wheel = VirtualWheel::new(WheelConfig {
            kind: WheelKind::Soak,
            fault: Some(super::Fault::Reboot(0.01)),
            require_auth: false,
            ..WheelConfig::default()
        });
        let mut saw_zero_seq = false;
        for _ in 0..600 {
            let out = wheel.next_tick();
            for bytes in &out.frames {
                let f = rsh_protocol::StreamFrame::parse(bytes).expect("parses");
                if f.seq == 0 {
                    saw_zero_seq = true;
                }
            }
        }
        assert!(saw_zero_seq, "seq restarts at 0 after reboot");
    }

    #[test]
    fn power_cut_behaves_like_reboot() {
        let mut wheel = VirtualWheel::new(WheelConfig {
            kind: WheelKind::Soak,
            fault: Some(super::Fault::PowerCut(0.01)),
            require_auth: false,
            ..WheelConfig::default()
        });
        let mut saw_zero_seq = false;
        for _ in 0..600 {
            let out = wheel.next_tick();
            for bytes in &out.frames {
                let f = rsh_protocol::StreamFrame::parse(bytes).expect("parses");
                if f.seq == 0 {
                    saw_zero_seq = true;
                }
            }
        }
        assert!(saw_zero_seq, "seq restarts at 0 after power-cut");
    }

    #[test]
    fn reorder_release_is_capped_per_tick() {
        // L-1 backstop: even with 100 frames held for reorder, one
        // `release_due` returns at most 64 and keeps the rest queued
        // for the next tick — a pathological queue cannot stall the
        // 5 ms tick, and nothing is dropped by the cap.
        let mut wheel = VirtualWheel::new(WheelConfig {
            require_auth: false,
            ..WheelConfig::default()
        });
        for n in 0..100u64 {
            wheel.reorder.push_back((0, vec![n as u8]));
        }
        let first = wheel.release_due();
        assert_eq!(first.len(), 64, "cap is 64 per tick");
        assert_eq!(wheel.reorder.len(), 36, "remainder stays queued");
        let second = wheel.release_due();
        assert_eq!(second.len(), 36, "remainder releases next tick");
        assert!(wheel.reorder.is_empty());
    }

    #[test]
    fn event_log_is_allocation_free_after_construction() {
        // M-1: the wheel summary's event buffer is pre-reserved, so
        // logging events during a run grows nothing — the rig's
        // `allocBytesDelta` measures the product, not its own log.
        let mut summary = super::WheelSummary::default();
        assert!(summary.events_log.capacity() >= 8192);
        summary.log_event(format_args!("t=1.0s test event"));
        assert_eq!(summary.events(), vec!["t=1.0s test event".to_string()]);
        assert_eq!(summary.events_dropped, 0);
    }
}
