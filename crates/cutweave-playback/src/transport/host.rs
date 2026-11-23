// SPDX-License-Identifier: GPL-3.0-or-later
//! The injected host clock (determinism, arch §8): everything the transport times (the scrub
//! refine delay, render durations for the quality policy, rates other than 1×, the
//! `playback.state` throttle) reads a [`HostClock`], never the wall clock directly.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// A monotonic host clock in nanoseconds.
pub trait HostClock: Send + Sync {
    /// Nanoseconds since an arbitrary origin; never decreases.
    fn now_ns(&self) -> u64;
}

/// The host clock of the preview ring ([`crate::shm::host_clock_ns`]: QPC on Windows,
/// `CLOCK_MONOTONIC` on Linux, `CLOCK_UPTIME_RAW` on macOS), so clock samples the transport
/// writes into the ring are on the clock readers extrapolate with.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl HostClock for SystemClock {
    fn now_ns(&self) -> u64 {
        crate::shm::host_clock_ns()
    }
}

/// A clock that moves only when told to (tests and deterministic simulations). Clones share it.
#[derive(Clone, Debug, Default)]
pub struct VirtualClock(Arc<AtomicU64>);

impl VirtualClock {
    /// A clock at `start_ns`.
    #[must_use]
    pub fn new(start_ns: u64) -> VirtualClock {
        VirtualClock(Arc::new(AtomicU64::new(start_ns)))
    }

    /// Moves forward by `ns`.
    pub fn advance(&self, ns: u64) {
        self.0.fetch_add(ns, Ordering::SeqCst);
    }

    /// Moves to `ns` if that is later (the clock never goes back).
    pub fn advance_to(&self, ns: u64) {
        self.0.fetch_max(ns, Ordering::SeqCst);
    }
}

impl HostClock for VirtualClock {
    fn now_ns(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_clock_only_moves_forward() {
        let c = VirtualClock::new(10);
        assert_eq!(c.now_ns(), 10);
        c.advance(5);
        assert_eq!(c.now_ns(), 15);
        c.advance_to(12);
        assert_eq!(c.now_ns(), 15);
        c.advance_to(20);
        assert_eq!(c.now_ns(), 20);
        let shared = c.clone();
        shared.advance(1);
        assert_eq!(c.now_ns(), 21);
        let a = SystemClock.now_ns();
        assert!(SystemClock.now_ns() >= a);
    }
}
