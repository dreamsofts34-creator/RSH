//! Phase B5 engine hot-path benches (SW-R8.1) — `rsh-engine`.
//!
//! Every bench measures one tick of the 200 Hz hot path on the fixed
//! representative input from [`rsh_benchmarks::fixture`]
//! (`representative_inputs()`: 1024 frames, steering + accelerator +
//! brake + handbrake in mV, 2 encoders stepping `+2` / `-1`, horn bit
//! on every 7th frame, `seq` incrementing from 0, 5000 µs per frame).
//! No randomness, no wall clock: the same index always yields the same
//! frame, so numbers are comparable run to run. Benches cycle through
//! the 1024 frames, so pipeline / tracker state evolves exactly as it
//! would on a live stream instead of re-processing one frozen frame.
//!
//! Benches:
//!
//! * `pipeline_tick` — [`ProcessingPipeline::process`] alone.
//! * `hot_path_parse_seq_pipeline` — parse (`StreamFrame::parse` from
//!   wire bytes) + [`SequenceTracker::check`] + [`Watchdog`] tick /
//!   staleness poll + `process`: the whole per-frame engine hot path.
//! * `sequence_watchdog_check` — `SequenceTracker::check` +
//!   `Watchdog::tick` + `Watchdog::is_stale` alone: the per-frame
//!   bookkeeping overhead. The watchdog is ticked/polled with one
//!   fixed `Instant` captured outside the loop, so the bench measures
//!   the check cost, not a `clock_gettime` syscall per tick (a real
//!   `Instant::now()` per tick would dominate the number and is not
//!   part of the bookkeeping under test).
//! * `alloc_probe_pipeline` / `alloc_probe_hot_path` — the same work as
//!   the first two benches, run through `iter_custom` under the
//!   counting allocator ([`CountingAllocator`]): they **panic the bench
//!   binary** if a single byte is allocated in the measured section, so
//!   `cargo bench` itself fails on a hot-path allocation. The
//!   `SequenceTracker` is warmed up with 2000 checks before the probe
//!   so its 1000-entry loss window is full and its `VecDeque` no
//!   longer grows — window growth is a one-time warm-up cost, not a
//!   per-tick allocation, and the probe measures the steady state.
//!
//! Thresholds (`thresholds.json`, enforced by `check-thresholds.py`):
//! `pipeline_tick` ≤ 1100 ns (C# baseline, plan §Phase B), hot path
//! ≤ measured mean + 20% headroom, 0 B/tick everywhere.

use std::hint::black_box;
use std::time::Instant;

use criterion::{Criterion, criterion_group, criterion_main};
use rsh_benchmarks::alloc::{CountingAllocator, allocated_bytes, allocation_count, reset_counts};
use rsh_benchmarks::fixture::{INPUT_COUNT, representative_inputs};
use rsh_engine::pipeline::ProcessingPipeline;
use rsh_engine::sequence::SequenceTracker;
use rsh_engine::watchdog::Watchdog;
use rsh_protocol::StreamFrame;

#[global_allocator]
static ALLOC: CountingAllocator = CountingAllocator;

/// Run `iters` pipeline ticks over the cycling inputs, returning a
/// checksum of the outputs (black-boxed by the caller so the work
/// cannot be optimised away).
fn run_pipeline_ticks(
    pipeline: &mut ProcessingPipeline,
    inputs: &[rsh_benchmarks::fixture::TickInput],
    index: &mut usize,
    iters: u64,
) -> u64 {
    let mut checksum: u64 = 0;
    for _ in 0..iters {
        let input = &inputs[*index % INPUT_COUNT];
        *index += 1;
        let out = pipeline.process(black_box(&input.frame));
        checksum = checksum
            .wrapping_add(u64::from(out.steering.to_bits()))
            .wrapping_add(u64::from(out.accelerator.to_bits()))
            .wrapping_add(u64::from(out.buttons))
            .wrapping_add(u64::from(out.encoder_deltas[0].cast_unsigned()));
    }
    checksum
}

/// Run `iters` full hot-path ticks (parse + sequence check + watchdog
/// + pipeline) over the cycling inputs, returning an output checksum.
fn run_hot_path_ticks(
    pipeline: &mut ProcessingPipeline,
    tracker: &mut SequenceTracker,
    watchdog: &mut Watchdog,
    now: Instant,
    inputs: &[rsh_benchmarks::fixture::TickInput],
    index: &mut usize,
    iters: u64,
) -> u64 {
    let mut checksum: u64 = 0;
    for _ in 0..iters {
        let input = &inputs[*index % INPUT_COUNT];
        *index += 1;
        let frame = StreamFrame::parse(black_box(&input.bytes)).expect("fixed input parses");
        let _decision = tracker.check(frame.seq);
        watchdog.tick(now);
        let _stale = watchdog.is_stale(now);
        let out = pipeline.process(black_box(&frame));
        checksum = checksum
            .wrapping_add(u64::from(out.steering.to_bits()))
            .wrapping_add(u64::from(out.buttons));
    }
    checksum
}

/// Register the timing benches (pipeline tick, full hot path,
/// sequence + watchdog overhead) on `c`.
fn bench_timing(c: &mut Criterion, inputs: &[rsh_benchmarks::fixture::TickInput]) {
    c.bench_function("pipeline_tick", |b| {
        let mut pipeline = ProcessingPipeline::default();
        let mut index = 0usize;
        // Warm up filters/ramps/encoder baseline so the timed loop
        // measures steady-state ticks, not first-sample transients.
        let _warmup = run_pipeline_ticks(&mut pipeline, inputs, &mut index, 64);
        b.iter(|| {
            let checksum = run_pipeline_ticks(&mut pipeline, inputs, &mut index, 1);
            black_box(checksum);
        });
    });

    c.bench_function("hot_path_parse_seq_pipeline", |b| {
        let mut pipeline = ProcessingPipeline::default();
        let mut tracker = SequenceTracker::new();
        let mut watchdog = Watchdog::new();
        let now = Instant::now();
        let mut index = 0usize;
        let _warmup = run_hot_path_ticks(
            &mut pipeline,
            &mut tracker,
            &mut watchdog,
            now,
            inputs,
            &mut index,
            64,
        );
        b.iter(|| {
            let checksum = run_hot_path_ticks(
                &mut pipeline,
                &mut tracker,
                &mut watchdog,
                now,
                inputs,
                &mut index,
                1,
            );
            black_box(checksum);
        });
    });

    c.bench_function("sequence_watchdog_check", |b| {
        let mut tracker = SequenceTracker::new();
        let mut watchdog = Watchdog::new();
        let now = Instant::now();
        // Warm the tracker past its 1000-entry loss window so the
        // timed loop measures steady-state checks (ring-buffer
        // push/pop, no growth) rather than window fill.
        let mut seq: u16 = 0;
        for _ in 0..2000 {
            let _decision = tracker.check(seq);
            seq = seq.wrapping_add(1);
        }
        b.iter(|| {
            let _decision = tracker.check(black_box(seq));
            seq = seq.wrapping_add(1);
            watchdog.tick(now);
            black_box(watchdog.is_stale(now));
        });
    });
}

/// Register the allocation probes: the same work as the timing
/// benches, run under the counting allocator. A probe panics the
/// bench binary on any allocation, so `cargo bench` fails on a
/// hot-path allocation.
fn bench_alloc_probes(c: &mut Criterion, inputs: &[rsh_benchmarks::fixture::TickInput]) {
    c.bench_function("alloc_probe_pipeline", |b| {
        let mut pipeline = ProcessingPipeline::default();
        let mut index = 0usize;
        let _warmup = run_pipeline_ticks(&mut pipeline, inputs, &mut index, 64);
        b.iter_custom(|iters| {
            reset_counts();
            let start = Instant::now();
            let checksum = run_pipeline_ticks(&mut pipeline, inputs, &mut index, iters);
            let elapsed = start.elapsed();
            black_box(checksum);
            assert_eq!(
                allocated_bytes(),
                0,
                "BUG: pipeline tick allocated {} bytes in {} allocs (0 B/tick required)",
                allocated_bytes(),
                allocation_count()
            );
            elapsed
        });
    });

    c.bench_function("alloc_probe_hot_path", |b| {
        let mut pipeline = ProcessingPipeline::default();
        let mut tracker = SequenceTracker::new();
        let mut watchdog = Watchdog::new();
        let now = Instant::now();
        let mut index = 0usize;
        // Warm up pipeline state AND the tracker's loss window, so the
        // probe measures steady-state ticks (see module docs).
        let _warmup = run_hot_path_ticks(
            &mut pipeline,
            &mut tracker,
            &mut watchdog,
            now,
            inputs,
            &mut index,
            2000,
        );
        b.iter_custom(|iters| {
            reset_counts();
            let start = Instant::now();
            let checksum = run_hot_path_ticks(
                &mut pipeline,
                &mut tracker,
                &mut watchdog,
                now,
                inputs,
                &mut index,
                iters,
            );
            let elapsed = start.elapsed();
            black_box(checksum);
            assert_eq!(
                allocated_bytes(),
                0,
                "BUG: hot path allocated {} bytes in {} allocs (0 B/tick required)",
                allocated_bytes(),
                allocation_count()
            );
            elapsed
        });
    });
}

fn benches(c: &mut Criterion) {
    let inputs = representative_inputs();
    assert_eq!(inputs.len(), INPUT_COUNT);
    bench_timing(c, &inputs);
    bench_alloc_probes(c, &inputs);
}

criterion_group!(benches_group, benches);
criterion_main!(benches_group);
