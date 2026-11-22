// SPDX-License-Identifier: GPL-3.0-or-later
//! Counting global allocator: proves the real-time audio callback never touches the heap.
//!
//! Code inside an [`RtSection`] is "real-time": every `alloc`/`dealloc`/`realloc` made by that
//! thread while the section is open is counted (globally and per thread). The cpal callback opens a
//! section around all of its work; in debug builds it then asserts the per-callback count is zero,
//! in release builds the count is only reported (CSV column `rt_allocs`).
//!
//! This is the spike's stand-in for `assert_no_alloc` (BSD-1-Clause, not in the root `deny.toml`
//! allow-list): same idea, ~60 lines, no extra dependency.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

/// Global allocator that forwards to [`System`] and counts heap traffic inside [`RtSection`]s.
#[derive(Debug)]
pub struct CountingAlloc;

static RT_ALLOCS_TOTAL: AtomicU64 = AtomicU64::new(0);

thread_local! {
    // `const` initialisers without `Drop`: accessing these never allocates or registers destructors,
    // so they are safe to touch from inside the allocator itself.
    static RT_DEPTH: Cell<u32> = const { Cell::new(0) };
    static THREAD_RT_ALLOCS: Cell<u64> = const { Cell::new(0) };
}

#[inline]
fn note_heap_op() {
    let in_rt = RT_DEPTH.try_with(|d| d.get() > 0).unwrap_or(false);
    if in_rt {
        RT_ALLOCS_TOTAL.fetch_add(1, Ordering::Relaxed);
        let _ = THREAD_RT_ALLOCS.try_with(|c| c.set(c.get() + 1));
    }
}

// SAFETY: every method forwards to `System` with the caller's arguments unchanged; the counting
// side effect does not allocate.
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note_heap_op();
        // SAFETY: same contract as the caller's.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        note_heap_op();
        // SAFETY: same contract as the caller's.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note_heap_op();
        // SAFETY: same contract as the caller's.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note_heap_op();
        // SAFETY: same contract as the caller's.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

/// Total heap operations observed inside real-time sections, across all threads, since start.
pub fn total_rt_allocs() -> u64 {
    RT_ALLOCS_TOTAL.load(Ordering::Relaxed)
}

/// Marks the current thread as real-time until [`RtSection::exit`] (or drop).
#[derive(Debug)]
pub struct RtSection {
    start: u64,
    open: bool,
}

impl RtSection {
    /// Opens a real-time section on the current thread (sections nest).
    #[inline]
    pub fn enter() -> Self {
        RT_DEPTH.with(|d| d.set(d.get() + 1));
        Self {
            start: THREAD_RT_ALLOCS.with(Cell::get),
            open: true,
        }
    }

    /// Closes the section and returns the number of heap operations made inside it.
    #[inline]
    pub fn exit(mut self) -> u64 {
        self.close();
        THREAD_RT_ALLOCS.with(Cell::get) - self.start
    }

    #[inline]
    fn close(&mut self) {
        if self.open {
            self.open = false;
            RT_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
        }
    }
}

impl Drop for RtSection {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_allocations_inside_section_only() {
        let outside = vec![1u8; 64];
        let section = RtSection::enter();
        let inside = vec![2u8; 64]; // alloc
        drop(inside); // dealloc
        let n = section.exit();
        assert_eq!(n, 2, "alloc + dealloc inside the section");
        drop(outside); // not counted
        let section = RtSection::enter();
        let x = std::hint::black_box(3u64) * 7;
        assert_eq!(section.exit(), 0, "pure arithmetic must not allocate ({x})");
    }
}
