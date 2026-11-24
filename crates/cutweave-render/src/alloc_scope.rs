// SPDX-License-Identifier: GPL-3.0-or-later
//! Allocation scopes: which side of a library boundary the current thread is on, so a counting
//! allocator can tell the render thread's own heap use from what a dependency does inside a call
//! (arch §10 "Threads", AGENTS.md: no per-frame allocation on render threads).
//!
//! The preview render path runs entirely in [`Scope::Own`] except where it calls into code it
//! does not control: wgpu (and the driver below it) in [`Scope::Wgpu`], and the media layer
//! (decoders, the shared RAM frame cache) in [`Scope::Media`]. The counting allocator of
//! `cutweave-audio` (feature `testing`) reads [`current`] on every heap operation inside a
//! real-time section and counts each scope separately, so a test can require zero allocations
//! from our own code and a documented bound from the rest.
//!
//! Marking costs one thread-local store on entry and one on exit (no allocation, no lock);
//! nothing reads the scope outside such tests.

use std::cell::Cell;

/// Who is running on this thread, as far as heap use is concerned.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Scope {
    /// Our own code (the default).
    #[default]
    Own = 0,
    /// Inside a wgpu call (wgpu-core, wgpu-hal and the driver), including dropping wgpu objects.
    Wgpu = 1,
    /// Inside the media layer: decoding a source frame or the shared RAM frame cache.
    Media = 2,
}

impl Scope {
    /// Number of scopes (the counting allocator keeps one counter per scope).
    pub const COUNT: usize = 3;

    /// Every scope, in index order.
    pub const ALL: [Scope; Scope::COUNT] = [Scope::Own, Scope::Wgpu, Scope::Media];

    /// Short name (diagnostics).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Scope::Own => "own",
            Scope::Wgpu => "wgpu",
            Scope::Media => "media",
        }
    }
}

thread_local! {
    // `const` initialiser without `Drop`: reading it never allocates, so a global allocator may
    // call `current` from inside `alloc`.
    static CURRENT: Cell<u8> = const { Cell::new(0) };
}

/// The current thread's scope, as an index below [`Scope::COUNT`] (0 while the thread is being
/// torn down).
#[inline]
#[must_use]
pub fn current() -> usize {
    usize::from(CURRENT.try_with(Cell::get).unwrap_or(0))
}

/// Restores the previous scope when dropped (also on unwinding).
#[derive(Debug)]
#[must_use = "the scope ends when the guard is dropped"]
pub struct ScopeGuard {
    previous: u8,
}

impl Drop for ScopeGuard {
    #[inline]
    fn drop(&mut self) {
        let _ = CURRENT.try_with(|c| c.set(self.previous));
    }
}

/// Enters `scope` on this thread until the guard is dropped.
#[inline]
pub fn enter(scope: Scope) -> ScopeGuard {
    ScopeGuard {
        previous: CURRENT.try_with(|c| c.replace(scope as u8)).unwrap_or(0),
    }
}

/// Runs `f` in `scope`. What it returns belongs to the caller again: dropping that later counts
/// in the caller's scope.
#[inline]
pub fn within<R>(scope: Scope, f: impl FnOnce() -> R) -> R {
    let _guard = enter(scope);
    f()
}

/// Runs `f` in [`Scope::Wgpu`].
#[inline]
pub fn wgpu<R>(f: impl FnOnce() -> R) -> R {
    within(Scope::Wgpu, f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_nest_and_restore() {
        assert_eq!(current(), 0);
        within(Scope::Wgpu, || {
            assert_eq!(current(), Scope::Wgpu as usize);
            within(Scope::Media, || {
                assert_eq!(current(), Scope::Media as usize);
            });
            assert_eq!(current(), Scope::Wgpu as usize);
        });
        assert_eq!(current(), 0);
        let caught = std::panic::catch_unwind(|| within(Scope::Media, || panic!("unwinds")));
        assert!(caught.is_err());
        assert_eq!(current(), 0, "a panic restores the scope");
        assert_eq!(Scope::ALL.map(Scope::name), ["own", "wgpu", "media"]);
    }
}
