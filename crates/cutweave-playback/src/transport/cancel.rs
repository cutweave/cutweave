// SPDX-License-Identifier: GPL-3.0-or-later
//! Latest-wins cancellation: the [`EpochGate`] every transport request passes through, the
//! [`CancelToken`] renders poll, and the commit gate that makes "no stale frame after a newer
//! request" exact.
//!
//! Every transport request (seek, play, pause, rate change, step, refresh) starts a new
//! **epoch**. Work started for an older epoch is stale: its [`CancelToken`] reads cancelled, and
//! the render thread can no longer commit its frame.
//!
//! The gate word is `epoch << 1 | committing`. The render thread publishes a frame inside
//! [`EpochGate::try_commit`], which flips the low bit with a compare-exchange that only succeeds
//! while the frame's epoch is still current; [`EpochGate::advance`] (the control path) cannot move
//! to the next epoch while that bit is set. So once `advance` has returned, no frame of an older
//! epoch can be published any more: the request is linearized after every frame already
//! committed and before every frame of the old epoch that was still in flight. The render
//! thread never waits (a failed compare-exchange means "stale, drop it"); the control path waits
//! at most for one frame copy into the ring.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// The epoch counter and commit gate shared by the control path and the render thread.
#[derive(Debug, Default)]
pub struct EpochGate {
    /// `epoch << 1 | committing`.
    word: AtomicU64,
}

impl EpochGate {
    /// A gate at epoch 0.
    #[must_use]
    pub fn new() -> EpochGate {
        EpochGate::default()
    }

    /// The current epoch.
    #[must_use]
    pub fn current(&self) -> u64 {
        self.word.load(Ordering::Acquire) >> 1
    }

    /// Starts the next epoch and returns it. Every token of an older epoch reads cancelled from
    /// now on, and no frame of an older epoch can be committed after this returns. Waits
    /// (spinning, then yielding) while the render thread is committing a frame, which is one
    /// copy into the ring.
    pub fn advance(&self) -> u64 {
        let mut spins = 0u32;
        loop {
            let cur = self.word.load(Ordering::Acquire);
            if cur & 1 == 0 {
                let next = cur.wrapping_add(2);
                if self
                    .word
                    .compare_exchange_weak(cur, next, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    return next >> 1;
                }
                continue;
            }
            spins = spins.wrapping_add(1);
            if spins < 64 {
                std::hint::spin_loop();
            } else {
                std::thread::yield_now();
            }
        }
    }

    /// Begins committing a frame of `epoch`, or `None` if `epoch` is no longer current (the
    /// frame is stale and must be dropped). Never waits. Publish the frame while the guard
    /// lives; dropping it ends the commit.
    #[must_use]
    pub fn try_commit(&self, epoch: u64) -> Option<CommitGuard<'_>> {
        let idle = epoch << 1;
        self.word
            .compare_exchange(idle, idle | 1, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| CommitGuard { gate: self, idle })
    }

    /// A token that reads cancelled once the gate moves past `epoch`.
    #[must_use]
    pub fn token(self: &Arc<Self>, epoch: u64) -> CancelToken {
        CancelToken {
            gate: Arc::clone(self),
            epoch,
        }
    }
}

/// A frame commit in progress (see [`EpochGate::try_commit`]); dropping it ends the commit.
#[derive(Debug)]
pub struct CommitGuard<'a> {
    gate: &'a EpochGate,
    idle: u64,
}

impl CommitGuard<'_> {
    /// The epoch being committed.
    #[must_use]
    pub fn epoch(&self) -> u64 {
        self.idle >> 1
    }
}

impl Drop for CommitGuard<'_> {
    fn drop(&mut self) {
        // Only `try_commit` sets the bit and `advance` waits for it, so the word is still
        // `idle | 1` here.
        self.gate.word.store(self.idle, Ordering::Release);
    }
}

/// Cooperative cancellation of one render: cancelled once a newer transport request exists.
///
/// [`FrameSource::render`](crate::transport::FrameSource::render) implementations poll
/// [`is_cancelled`](Self::is_cancelled) between units of work (before a decode, between the
/// composite and the readback) and return
/// [`SourceError::Cancelled`](crate::transport::SourceError::Cancelled). Polling is one atomic
/// load. Cloning shares the gate (no allocation).
#[derive(Clone, Debug)]
pub struct CancelToken {
    gate: Arc<EpochGate>,
    epoch: u64,
}

impl CancelToken {
    /// A token that is never cancelled (one-off renders outside the transport).
    #[must_use]
    pub fn never() -> CancelToken {
        CancelToken {
            gate: Arc::new(EpochGate::new()),
            epoch: 0,
        }
    }

    /// Whether a newer request superseded the work this token belongs to.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.gate.current() != self.epoch
    }

    /// The epoch the work belongs to.
    #[must_use]
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::atomic::{AtomicBool, AtomicU64};
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn advancing_cancels_older_tokens_and_commits() {
        let gate = Arc::new(EpochGate::new());
        assert_eq!(gate.current(), 0);
        let t0 = gate.token(0);
        assert!(!t0.is_cancelled());
        assert_eq!(gate.advance(), 1);
        assert!(t0.is_cancelled());
        assert!(gate.try_commit(0).is_none(), "a stale epoch cannot commit");
        let t1 = gate.token(1);
        assert!(!t1.is_cancelled());
        {
            let guard = gate.try_commit(1).unwrap();
            assert_eq!(guard.epoch(), 1);
            assert!(gate.try_commit(1).is_none(), "one commit at a time");
            assert_eq!(gate.current(), 1);
        }
        assert!(gate.try_commit(1).is_some());
        assert!(!CancelToken::never().is_cancelled());
    }

    #[test]
    fn advance_waits_for_a_commit_in_progress() {
        let gate = Arc::new(EpochGate::new());
        let guard = gate.try_commit(0).unwrap();
        let advanced = Arc::new(AtomicBool::new(false));
        let thread = {
            let (gate, advanced) = (Arc::clone(&gate), Arc::clone(&advanced));
            std::thread::spawn(move || {
                let e = gate.advance();
                advanced.store(true, Ordering::SeqCst);
                e
            })
        };
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_millis(30) {
            assert!(!advanced.load(Ordering::SeqCst), "advanced during a commit");
            std::thread::yield_now();
        }
        drop(guard);
        assert_eq!(thread.join().unwrap(), 1);
        assert!(advanced.load(Ordering::SeqCst));
        assert_eq!(gate.current(), 1);
    }

    #[test]
    fn concurrent_commits_never_see_a_newer_epoch() {
        // A committer that checks the gate's epoch inside its commit always finds its own.
        let gate = Arc::new(EpochGate::new());
        let stop = Arc::new(AtomicBool::new(false));
        let commits = Arc::new(AtomicU64::new(0));
        let committer = {
            let (gate, stop, commits) =
                (Arc::clone(&gate), Arc::clone(&stop), Arc::clone(&commits));
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let e = gate.current();
                    std::hint::spin_loop();
                    if let Some(g) = gate.try_commit(e) {
                        assert_eq!(gate.current(), g.epoch());
                        commits.fetch_add(1, Ordering::Relaxed);
                    }
                }
            })
        };
        let t0 = Instant::now();
        let mut advances = 0u64;
        while advances < 20_000 || commits.load(Ordering::Relaxed) < 1_000 {
            assert!(t0.elapsed() < Duration::from_secs(20), "no progress");
            gate.advance();
            advances += 1;
            if advances >= 20_000 {
                // Enough advances: keep racing the committer, but slowly enough that it gets
                // its 1 000 commits even on a loaded machine (it can starve while every core
                // is busy).
                std::thread::sleep(Duration::from_micros(50));
            } else if advances.is_multiple_of(16) {
                std::thread::yield_now();
            }
        }
        stop.store(true, Ordering::Relaxed);
        committer.join().unwrap();
        assert_eq!(gate.current(), advances);
    }
}
