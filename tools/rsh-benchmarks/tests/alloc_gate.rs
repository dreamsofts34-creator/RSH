//! 0 B/tick allocation gate (Phase B5, SW-R8.1).
//!
//! The hot path must not allocate: parse + sequence check + watchdog +
//! pipeline tick, 0 bytes per tick. This test installs the counting
//! allocator ([`CountingAllocator`]) as the test binary's global
//! allocator, warms the state up, then counts allocations over 10 000
//! steady-state ticks per section.
//!
//! Warm-up matters: [`SequenceTracker`](rsh_engine::sequence::SequenceTracker)
//! keeps a 1000-entry sliding loss window in a `VecDeque`, which grows
//! (allocates) while it fills. That is a one-time warm-up cost, not a
//! per-tick allocation, so the tracker is driven through 2000 checks
//! before any measurement. If a section allocates in steady state,
//! the test fails with the exact byte / allocation counts — that is a
//! BUG in the measured code, to be reported with its site, not a
//! threshold to be adjusted.
//!
//! Single test function, single-threaded by construction: the counters
//! are process-global, so the sections run sequentially inside one
//! test and the counters are reset immediately before each measured
//! section and read immediately after it.

use std::hint::black_box;
use std::time::Instant;

use rsh_benchmarks::alloc::{CountingAllocator, allocated_bytes, allocation_count, reset_counts};
use rsh_benchmarks::fixture::representative_inputs;
use rsh_engine::pipeline::ProcessingPipeline;
use rsh_engine::sequence::SequenceTracker;
use rsh_engine::watchdog::Watchdog;
use rsh_protocol::StreamFrame;

#[global_allocator]
static ALLOC: CountingAllocator = CountingAllocator;

/// Ticks per measured section. 10 000 ticks at 200 Hz is 50 s of
/// stream; any per-tick allocation shows up 10 000 times over.
const MEASURED_TICKS: u64 = 10_000;

/// Warm-up ticks for the sequence tracker: past its 1000-entry loss
/// window, so the `VecDeque` is full and steady-state checks cannot
/// grow it.
const TRACKER_WARMUP_TICKS: u16 = 2000;

/// The hot path allocates 0 bytes per tick, in every section.
#[test]
fn hot_path_allocates_zero_bytes_per_tick() {
    let inputs = representative_inputs();
    let mut checksum: u64 = 0;

    // --- Section 1: pipeline tick alone. ---
    let mut pipeline = ProcessingPipeline::default();
    let mut index = 0usize;
    for _ in 0..64 {
        let out = pipeline.process(black_box(&inputs[index % inputs.len()].frame));
        index += 1;
        checksum = checksum.wrapping_add(u64::from(out.steering.to_bits()));
    }
    reset_counts();
    for _ in 0..MEASURED_TICKS {
        let out = pipeline.process(black_box(&inputs[index % inputs.len()].frame));
        index += 1;
        checksum = checksum.wrapping_add(u64::from(out.steering.to_bits()));
    }
    let bytes = allocated_bytes();
    let count = allocation_count();
    assert_eq!(
        bytes, 0,
        "BUG: pipeline tick allocated {bytes} bytes in {count} allocs"
    );
    assert_eq!(
        count, 0,
        "BUG: pipeline tick made {count} allocations ({bytes} bytes)"
    );

    // --- Section 2: full hot path (parse + sequence + watchdog + pipeline). ---
    let mut tracker = SequenceTracker::new();
    let mut watchdog = Watchdog::new();
    let now = Instant::now();
    let mut seq: u16 = 0;
    for _ in 0..TRACKER_WARMUP_TICKS {
        let _decision = tracker.check(seq);
        seq = seq.wrapping_add(1);
    }
    let _warmup = run_hot_path(
        &mut pipeline,
        &mut tracker,
        &mut watchdog,
        now,
        &inputs,
        &mut seq,
        64,
    );
    reset_counts();
    let measured = run_hot_path(
        &mut pipeline,
        &mut tracker,
        &mut watchdog,
        now,
        &inputs,
        &mut seq,
        MEASURED_TICKS,
    );
    checksum = checksum.wrapping_add(measured);
    let bytes = allocated_bytes();
    let count = allocation_count();
    assert_eq!(
        bytes, 0,
        "BUG: full hot path allocated {bytes} bytes in {count} allocs over {MEASURED_TICKS} ticks"
    );
    assert_eq!(
        count, 0,
        "BUG: full hot path made {count} allocations ({bytes} bytes) over {MEASURED_TICKS} ticks"
    );

    // --- Section 3: sequence + watchdog check alone (per-frame overhead). ---
    // Tracker is already warm from section 2; the window is full.
    reset_counts();
    for _ in 0..MEASURED_TICKS {
        let _decision = tracker.check(black_box(seq));
        seq = seq.wrapping_add(1);
        watchdog.tick(now);
        checksum = checksum.wrapping_add(u64::from(watchdog.is_stale(now)));
    }
    let bytes = allocated_bytes();
    let count = allocation_count();
    assert_eq!(
        bytes, 0,
        "BUG: sequence+watchdog check allocated {bytes} bytes in {count} allocs"
    );
    assert_eq!(
        count, 0,
        "BUG: sequence+watchdog check made {count} allocations ({bytes} bytes)"
    );

    black_box(checksum);
}

/// Run `ticks` full hot-path ticks over the fixture inputs, cycling
/// from index 0, and return an output checksum. `seq` is the
/// tracker's monotonic sequence counter, advanced once per tick: the
/// fixture cycles 1024 frames whose seqs repeat (0..1023), and
/// feeding `frame.seq` to the tracker replays old seqs — which L8
/// correctly classifies as duplicates, so the accept path (and its
/// loss-window push) would never be exercised in steady state. The
/// frame itself still goes through the pipeline unchanged.
fn run_hot_path(
    pipeline: &mut ProcessingPipeline,
    tracker: &mut SequenceTracker,
    watchdog: &mut Watchdog,
    now: Instant,
    inputs: &[rsh_benchmarks::fixture::TickInput],
    seq: &mut u16,
    ticks: u64,
) -> u64 {
    let mut checksum: u64 = 0;
    for i in 0..ticks {
        let idx = usize::try_from(i).expect("tick count fits usize");
        let input = &inputs[idx % inputs.len()];
        let frame = StreamFrame::parse(black_box(&input.bytes)).expect("fixed input parses");
        let _decision = tracker.check(*seq);
        *seq = seq.wrapping_add(1);
        watchdog.tick(now);
        let _stale = watchdog.is_stale(now);
        let out = pipeline.process(black_box(&frame));
        checksum = checksum
            .wrapping_add(u64::from(out.steering.to_bits()))
            .wrapping_add(u64::from(out.buttons));
    }
    checksum
}
