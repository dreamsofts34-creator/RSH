//! Headless 60 s soak runner (Phase H FIX-A).
//!
//! Runs [`rsh_benchmarks::soak::run_soak`] in a plain process — no
//! libtest harness — and prints the outcome as `KEY=VALUE` lines for
//! `tests/soak.rs` to assert on. Exits 0 only when the soak passes;
//! otherwise prints the failures to stderr and exits 1.
//!
//! Why a separate process: libtest fires a 60 s "has been running"
//! notice on its own thread inside the test process, and that notice
//! allocates (512 + 82 = 594 bytes, transiently) at exactly the
//! instant the old in-process soak read its counters — so the
//! in-process test failed with a byte-identical 594 bytes in loaded
//! full-suite runs while passing standalone. The measured code
//! allocates nothing; the harness does. Running the measurement in
//! this child process keeps the harness out of the measured process
//! by construction.

use std::process::ExitCode;

use rsh_benchmarks::alloc::CountingAllocator;
use rsh_benchmarks::soak::run_soak;

#[global_allocator]
static ALLOC: CountingAllocator = CountingAllocator;

fn main() -> ExitCode {
    let outcome = run_soak();
    println!("TICKS={}", outcome.ticks);
    println!("ACCEPTED={}", outcome.accepted);
    println!("ELAPSED_MS={}", outcome.elapsed.as_millis());
    println!("HEAP_GROWTH={}", outcome.heap_growth);
    match outcome.rss_growth {
        Some(growth) => println!("RSS_GROWTH={growth}"),
        None => println!("RSS_GROWTH=unknown"),
    }
    let failures = outcome.failures();
    if failures.is_empty() {
        ExitCode::SUCCESS
    } else {
        for failure in &failures {
            eprintln!("FAIL: {failure}");
        }
        ExitCode::FAILURE
    }
}
