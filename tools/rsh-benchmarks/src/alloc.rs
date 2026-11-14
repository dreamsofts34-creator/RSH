//! Counting allocator for the RSH benchmark / test harness (Phase B5,
//! SW-R8.1).
//!
//! The hot path (parse + sequence check + pipeline tick) must allocate
//! **0 bytes per tick**. This module wraps [`std::alloc::System`] in a
//! [`CountingAllocator`] that counts every allocation and deallocation
//! while delegating the real work to `System`, so the counts measure the
//! program under test — not a different allocator.
//!
//! Install it as the binary's global allocator, then scope a
//! measurement with [`reset_counts`] / [`allocated_bytes`]:
//!
//! ```rust
//! use rsh_benchmarks::alloc::{CountingAllocator, allocated_bytes, allocation_count, reset_counts};
//!
//! #[global_allocator]
//! static ALLOC: CountingAllocator = CountingAllocator;
//!
//! # fn main() {
//! reset_counts();
//! // ... hot-path work ...
//! assert_eq!(allocated_bytes(), 0);
//! assert_eq!(allocation_count(), 0);
//! # }
//! ```
//!
//! # What the counters mean
//!
//! * [`allocated_bytes`] — total bytes requested from the allocator
//!   since the last [`reset_counts`] (sum of `layout.size()` per
//!   `alloc` / `realloc` growth).
//! * [`allocation_count`] — number of `alloc` calls since the reset.
//! * [`deallocated_bytes`] — total bytes returned since the reset.
//! * [`net_live_bytes`] — `allocated - deallocated` (saturating):
//!   heap growth attributable to the measured section. A hot path that
//!   allocates and frees equally still fails the 0 B/tick gate — the
//!   gate counts *allocations*, not just net growth — but
//!   [`net_live_bytes`] is the right metric for the soak test, where a
//!   leak is what matters.
//!
//! # Caveats
//!
//! * The counters are process-global. Warm up first (e.g. drive a
//!   [`rsh_engine::sequence::SequenceTracker`] past its 1000-entry loss
//!   window so its `VecDeque` stops growing), then [`reset_counts`]
//!   immediately before the measured section and read the counters
//!   immediately after — anything else running in the process (test
//!   harness, criterion) allocates too, and must stay outside the
//!   measured window.
//! * `realloc` is counted as freeing the old block and allocating the
//!   new one; the underlying allocator may in reality resize in place,
//!   so the counts are a conservative upper bound for `realloc` users.
//!   The hot path does not call `realloc`.
//! * Counters use [`std::sync::atomic::Ordering::Relaxed`]: they are
//!   exact per call, but concurrent readers may observe a slightly
//!   stale total. Measurements are single-threaded by construction.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};

/// Total bytes allocated since the last [`reset_counts`].
pub static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);
/// Number of `alloc` calls since the last [`reset_counts`].
pub static ALLOCATION_COUNT: AtomicU64 = AtomicU64::new(0);
/// Total bytes deallocated since the last [`reset_counts`].
pub static DEALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);
/// Number of `dealloc` calls since the last [`reset_counts`].
pub static DEALLOCATION_COUNT: AtomicU64 = AtomicU64::new(0);

/// Reset all four counters to zero. Call immediately before the
/// measured section; see the module docs for why the window matters.
pub fn reset_counts() {
    ALLOCATED_BYTES.store(0, Ordering::Relaxed);
    ALLOCATION_COUNT.store(0, Ordering::Relaxed);
    DEALLOCATED_BYTES.store(0, Ordering::Relaxed);
    DEALLOCATION_COUNT.store(0, Ordering::Relaxed);
}

/// Total bytes allocated since the last [`reset_counts`].
#[must_use]
pub fn allocated_bytes() -> u64 {
    ALLOCATED_BYTES.load(Ordering::Relaxed)
}

/// Number of `alloc` calls since the last [`reset_counts`].
#[must_use]
pub fn allocation_count() -> u64 {
    ALLOCATION_COUNT.load(Ordering::Relaxed)
}

/// Total bytes deallocated since the last [`reset_counts`].
#[must_use]
pub fn deallocated_bytes() -> u64 {
    DEALLOCATED_BYTES.load(Ordering::Relaxed)
}

/// Number of `dealloc` calls since the last [`reset_counts`].
#[must_use]
pub fn deallocation_count() -> u64 {
    DEALLOCATION_COUNT.load(Ordering::Relaxed)
}

/// Heap growth since the last [`reset_counts`]: allocated bytes minus
/// deallocated bytes, saturating at zero. `0` means every allocation
/// in the measured section was freed again inside it.
#[must_use]
pub fn net_live_bytes() -> u64 {
    allocated_bytes().saturating_sub(deallocated_bytes())
}

/// A [`GlobalAlloc`] that counts allocations and forwards them to
/// [`System`].
///
/// # Safety
///
/// The implementation only forwards to `System` and bumps atomic
/// counters; it upholds every [`GlobalAlloc`] contract (`alloc` returns
/// a suitably aligned block or null, `dealloc` is only called with a
/// block from this allocator, and so on) because `System` does.
#[derive(Debug, Default, Clone, Copy)]
pub struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: `layout` comes from the caller, exactly as `System`
        // would receive it; the pointer is only dereferenced by the
        // caller after a non-null return, which `System` guarantees.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            ALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
            ALLOCATION_COUNT.fetch_add(1, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr`/`layout` are a block previously returned by
        // this allocator's `alloc`/`realloc`, per the `GlobalAlloc`
        // contract; forwarding to `System` is therefore valid.
        unsafe { System.dealloc(ptr, layout) };
        DEALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        DEALLOCATION_COUNT.fetch_add(1, Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: same contract as `alloc`/`dealloc` above.
        let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() {
            DEALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
            DEALLOCATION_COUNT.fetch_add(1, Ordering::Relaxed);
            ALLOCATED_BYTES.fetch_add(new_size as u64, Ordering::Relaxed);
            ALLOCATION_COUNT.fetch_add(1, Ordering::Relaxed);
        }
        new_ptr
    }
}
