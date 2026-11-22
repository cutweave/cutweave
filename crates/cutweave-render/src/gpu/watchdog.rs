// SPDX-License-Identifier: GPL-3.0-or-later
//! The render-thread watchdog (arch §8 "GPU robustness"): logs frames that take longer than a
//! threshold, including frames that never finish (a hung driver).
//!
//! The render path only bumps two atomics per frame ([`Watchdog::begin`]); all timing happens on
//! the watchdog's own thread, so rendering stays free of clocks (determinism, arch §8). A frame
//! is reported once, at most `threshold / 10` (capped at 50 ms) after it crossed the threshold.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Debug, Default)]
struct Shared {
    frame: AtomicU64,
    busy: AtomicBool,
    stalls: AtomicU64,
}

/// A thread watching render frames.
#[derive(Debug)]
pub struct Watchdog {
    shared: Arc<Shared>,
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

/// Marks a frame in flight; the frame ends when this is dropped.
#[derive(Debug)]
#[must_use = "the frame ends when the guard is dropped"]
pub struct FrameGuard(Arc<Shared>);

impl Drop for FrameGuard {
    fn drop(&mut self) {
        self.0.busy.store(false, Ordering::Release);
    }
}

impl Watchdog {
    /// Starts a watchdog reporting frames slower than `threshold` for `label` (the adapter).
    #[must_use]
    pub fn spawn(threshold: Duration, label: String) -> Watchdog {
        let shared = Arc::new(Shared::default());
        let (stop, rx) = mpsc::channel::<()>();
        let watched = Arc::clone(&shared);
        let tick = (threshold / 10).clamp(Duration::from_millis(1), Duration::from_millis(50));
        let thread = std::thread::Builder::new()
            .name("cutweave-render-watchdog".into())
            .spawn(move || {
                // (frame number, first seen busy, reported)
                let mut current: Option<(u64, Instant, bool)> = None;
                loop {
                    match rx.recv_timeout(tick) {
                        Err(RecvTimeoutError::Timeout) => {}
                        Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
                    }
                    if !watched.busy.load(Ordering::Acquire) {
                        current = None;
                        continue;
                    }
                    let frame = watched.frame.load(Ordering::Acquire);
                    match &mut current {
                        Some((f, since, reported)) if *f == frame => {
                            let elapsed = since.elapsed();
                            if !*reported && elapsed >= threshold {
                                *reported = true;
                                watched.stalls.fetch_add(1, Ordering::AcqRel);
                                tracing::warn!(
                                    adapter = %label,
                                    frame,
                                    elapsed_ms = elapsed.as_millis(),
                                    threshold_ms = threshold.as_millis(),
                                    "render frame exceeded the watchdog threshold"
                                );
                            }
                        }
                        _ => current = Some((frame, Instant::now(), false)),
                    }
                }
            })
            .ok();
        Watchdog {
            shared,
            stop: Some(stop),
            thread,
        }
    }

    /// Marks the start of a frame; drop the guard when it ends.
    pub fn begin(&self) -> FrameGuard {
        self.shared.frame.fetch_add(1, Ordering::AcqRel);
        self.shared.busy.store(true, Ordering::Release);
        FrameGuard(Arc::clone(&self.shared))
    }

    /// Frames reported as too slow so far.
    #[must_use]
    pub fn stalls(&self) -> u64 {
        self.shared.stalls.load(Ordering::Acquire)
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slow_frames_are_reported_once_and_fast_ones_never() {
        let dog = Watchdog::spawn(Duration::from_millis(30), "test".into());
        for _ in 0..5 {
            let _frame = dog.begin();
        }
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(dog.stalls(), 0, "idle and fast frames are not stalls");
        {
            let _slow = dog.begin();
            std::thread::sleep(Duration::from_millis(150));
        }
        assert_eq!(dog.stalls(), 1, "a slow frame is reported exactly once");
        {
            let _another = dog.begin();
            std::thread::sleep(Duration::from_millis(150));
        }
        assert_eq!(dog.stalls(), 2);
    }
}
