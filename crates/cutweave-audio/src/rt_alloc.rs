// SPDX-License-Identifier: GPL-3.0-or-later
//! A counting global allocator that proves real-time code never touches the heap.
//!
//! Install [`CountingAlloc`] as the `#[global_allocator]` of a test binary (or a debug build of
//! the engine) and wrap real-time work in an [`RtSection`]: every `alloc`, `dealloc` and
//! `realloc` the current thread makes while a section is open is counted. Other threads (the
//! workers) are not counted. Without the allocator installed, sections count nothing.
//!
//! Operations are counted per [`Scope`] (`cutweave_render::alloc_scope`): code that calls into a
//! library it does not control (wgpu, the media layer) marks that call, so a section can require
//! zero heap operations from its own code ([`RtSection::exit`]) and still measure the library's
//! ([`RtSection::exit_by_scope`]). Unmarked code counts as [`Scope::Own`].
//!
//! This stands in for `assert_no_alloc`, which the WBS names: that crate is BSD-1-Clause, which
//! the root `deny.toml` does not allow (M0 finding S2-11). Same idea, no dependency; the S2 spike
//! proved it on the real cpal callback.
//!
//! Test instrumentation only: compiled with the `testing` feature, so a normal build of the crate
//! contains no `unsafe` code.

// Justification: a `GlobalAlloc` implementation is necessarily `unsafe` (docs/01-architecture.md
// §3 allows it in this crate); it only forwards to `System`. Every use carries a `// SAFETY:`
// comment.
#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

pub use cutweave_render::alloc_scope::Scope;

/// Global allocator that forwards to [`System`] and counts heap operations inside
/// [`RtSection`]s.
#[derive(Debug)]
pub struct CountingAlloc;

static RT_HEAP_OPS: AtomicU64 = AtomicU64::new(0);

/// Heap operations per scope, as counted by one thread.
type ScopeOps = [Cell<u64>; Scope::COUNT];

thread_local! {
    // `const` initialisers without `Drop`: touching these never allocates or registers a
    // destructor, so the allocator itself may use them.
    static DEPTH: Cell<u32> = const { Cell::new(0) };
    static THREAD_OPS: ScopeOps = const { [const { Cell::new(0) }; Scope::COUNT] };
}

#[inline]
fn note_heap_op() {
    if DEPTH.try_with(|d| d.get() > 0).unwrap_or(false) {
        let scope = cutweave_render::alloc_scope::current().min(Scope::COUNT - 1);
        if scope == Scope::Own as usize {
            RT_HEAP_OPS.fetch_add(1, Ordering::Relaxed);
        }
        // A thread being torn down has no counter left to update; the global count has it.
        let _ = THREAD_OPS.try_with(|c| c[scope].set(c[scope].get() + 1));
    }
}

/// This thread's counters so far.
#[inline]
fn thread_ops() -> [u64; Scope::COUNT] {
    THREAD_OPS.with(|c| std::array::from_fn(|i| c[i].get()))
}

// SAFETY: every method forwards to `System` with the caller's arguments unchanged, so `System`'s
// guarantees hold; the counting side effect touches only atomics and const thread-locals and
// never allocates.
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note_heap_op();
        // SAFETY: the caller upholds `GlobalAlloc::alloc`'s contract, which `System` shares.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        note_heap_op();
        // SAFETY: `ptr` was allocated by this allocator, i.e. by `System`, with `layout`.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note_heap_op();
        // SAFETY: the caller upholds `GlobalAlloc::alloc_zeroed`'s contract, which `System` shares.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note_heap_op();
        // SAFETY: `ptr` was allocated by `System` with `layout`; the caller upholds the rest.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

/// Heap operations made inside real-time sections by their own code ([`Scope::Own`]), all
/// threads, since the process started.
#[must_use]
pub fn total_rt_heap_ops() -> u64 {
    RT_HEAP_OPS.load(Ordering::Relaxed)
}

/// Marks the current thread as real-time until [`RtSection::exit`] (or drop). Sections nest.
#[derive(Debug)]
pub struct RtSection {
    start: [u64; Scope::COUNT],
    open: bool,
}

impl RtSection {
    /// Opens a section on the current thread.
    #[inline]
    #[must_use]
    pub fn enter() -> RtSection {
        DEPTH.with(|d| d.set(d.get() + 1));
        RtSection {
            start: thread_ops(),
            open: true,
        }
    }

    /// Closes the section and returns the heap operations its own code made inside it
    /// ([`Scope::Own`]: everything not marked as a library call).
    #[inline]
    #[must_use]
    pub fn exit(self) -> u64 {
        self.exit_by_scope()[Scope::Own as usize]
    }

    /// Closes the section and returns the heap operations made inside it per [`Scope`]
    /// (indexed by `Scope as usize`).
    #[inline]
    #[must_use]
    pub fn exit_by_scope(mut self) -> [u64; Scope::COUNT] {
        self.close();
        let now = thread_ops();
        std::array::from_fn(|i| now[i] - self.start[i])
    }

    #[inline]
    fn close(&mut self) {
        if self.open {
            self.open = false;
            DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
        }
    }
}

impl Drop for RtSection {
    fn drop(&mut self) {
        self.close();
    }
}
