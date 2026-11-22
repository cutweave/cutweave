// SPDX-License-Identifier: GPL-3.0-or-later
//! The master clock: audio device position, latency-compensated, readable lock-free from any thread.
//!
//! The audio callback is the only writer. Each callback publishes an [`Anchor`]: "timeline frame
//! `frame` (the first frame of this buffer) becomes audible at wall time `wall_ns`", where
//! `wall_ns = Instant::now() at callback entry + (playback - callback)` from cpal's
//! `OutputStreamTimestamp` (WASAPI: device position + buffered + stream latency). Readers
//! extrapolate from the newest anchor at the nominal rate, clamped so a stalled stream cannot run
//! away. Anchors are published through a seqlock built from atomics, so the writer never blocks and
//! readers never see a torn anchor. This is the value §10 puts in the preview SHM header.

use std::hint::spin_loop;
use std::sync::atomic::{AtomicU64, Ordering, fence};
use std::time::Instant;

use crate::util::NANOS;

/// One clock observation published by the audio callback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Anchor {
    /// Timeline frame (device rate) of the first frame of the buffer being submitted.
    pub frame: u64,
    /// Wall time (ns since [`AudioClock::epoch`]) at which `frame` is audible.
    pub wall_ns: u64,
    /// Frames in that buffer (bounds extrapolation).
    pub span_frames: u64,
}

const UNSET: u64 = u64::MAX;

/// Lock-free audio master clock (single writer: the audio callback).
#[derive(Debug)]
pub struct AudioClock {
    epoch: Instant,
    sample_rate: u32,
    seq: AtomicU64,
    anchor_frame: AtomicU64,
    anchor_wall_ns: AtomicU64,
    span_frames: AtomicU64,
    /// Wall time at which frame 0 was audible, fixed from the first anchor after warm-up.
    base_wall_ns: AtomicU64,
    frames_submitted: AtomicU64,
}

impl AudioClock {
    /// Creates a clock for a stream at `sample_rate`; `epoch` is the wall-clock origin.
    pub fn new(sample_rate: u32, epoch: Instant) -> Self {
        Self {
            epoch,
            sample_rate,
            seq: AtomicU64::new(0),
            anchor_frame: AtomicU64::new(0),
            anchor_wall_ns: AtomicU64::new(0),
            span_frames: AtomicU64::new(0),
            base_wall_ns: AtomicU64::new(UNSET),
            frames_submitted: AtomicU64::new(0),
        }
    }

    /// Wall-clock origin shared by every timestamp in the spike.
    pub fn epoch(&self) -> Instant {
        self.epoch
    }

    /// Device sample rate.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Nanoseconds since the epoch for `t`.
    #[inline]
    pub fn wall_ns(&self, t: Instant) -> u64 {
        u64::try_from(t.saturating_duration_since(self.epoch).as_nanos()).unwrap_or(u64::MAX)
    }

    /// Nanoseconds since the epoch, now.
    #[inline]
    pub fn now_ns(&self) -> u64 {
        self.wall_ns(Instant::now())
    }

    // ---- writer side (audio callback only) ----

    /// Publishes a new anchor. Wait-free; called once per callback.
    #[inline]
    pub fn publish(&self, a: Anchor) {
        let s = self.seq.load(Ordering::Relaxed);
        self.seq.store(s.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        self.anchor_frame.store(a.frame, Ordering::Relaxed);
        self.anchor_wall_ns.store(a.wall_ns, Ordering::Relaxed);
        self.span_frames.store(a.span_frames, Ordering::Relaxed);
        self.seq.store(s.wrapping_add(2), Ordering::Release);
        // Drift baseline: the first anchor after 0.5 s of playback (start-up padding settles).
        if self.base_wall_ns.load(Ordering::Relaxed) == UNSET
            && a.frame >= u64::from(self.sample_rate / 2)
        {
            let base = a.wall_ns as f64 - a.frame as f64 * NANOS / f64::from(self.sample_rate);
            self.base_wall_ns
                .store(base.max(0.0) as u64, Ordering::Relaxed);
        }
    }

    /// Adds frames handed to the device (sample-accurate submitted count).
    #[inline]
    pub fn add_submitted(&self, frames: u64) {
        self.frames_submitted.fetch_add(frames, Ordering::Relaxed);
    }

    // ---- reader side (any thread) ----

    /// Frames handed to the device so far (exact integer).
    pub fn frames_submitted(&self) -> u64 {
        self.frames_submitted.load(Ordering::Relaxed)
    }

    /// Latest consistent anchor, or `None` before the first callback.
    pub fn anchor(&self) -> Option<Anchor> {
        loop {
            let s1 = self.seq.load(Ordering::Acquire);
            if s1 & 1 == 1 {
                spin_loop();
                continue;
            }
            let a = Anchor {
                frame: self.anchor_frame.load(Ordering::Relaxed),
                wall_ns: self.anchor_wall_ns.load(Ordering::Relaxed),
                span_frames: self.span_frames.load(Ordering::Relaxed),
            };
            fence(Ordering::Acquire);
            let s2 = self.seq.load(Ordering::Relaxed);
            if s1 == s2 {
                return (s1 != 0).then_some(a);
            }
            spin_loop();
        }
    }

    /// Audible timeline position (frames) at wall time `wall_ns`, extrapolated from the latest
    /// anchor. Clamped to `[0, anchor + 2 buffers]` so a stalled device cannot run the clock away.
    pub fn position_frames_at(&self, wall_ns: u64) -> Option<f64> {
        let a = self.anchor()?;
        Some(extrapolate(a, wall_ns, self.sample_rate))
    }

    /// Wall time (ns since epoch) at which timeline frame `frame` is audible, per the latest anchor.
    pub fn wall_ns_of_frame(&self, frame: f64) -> Option<f64> {
        let a = self.anchor()?;
        Some(a.wall_ns as f64 + (frame - a.frame as f64) * NANOS / f64::from(self.sample_rate))
    }

    /// Audio clock minus wall clock (ns) at the latest anchor, relative to the warm-up baseline.
    /// Positive: the device consumed samples faster than `Instant` advanced.
    pub fn drift_ns(&self) -> Option<f64> {
        let a = self.anchor()?;
        let base = self.base_wall_ns.load(Ordering::Relaxed);
        if base == UNSET {
            return None;
        }
        let audio = a.frame as f64 * NANOS / f64::from(self.sample_rate);
        let wall = a.wall_ns as f64 - base as f64;
        Some(audio - wall)
    }

    /// Wall time (ns since epoch) at which frame 0 was audible (drift baseline), once known.
    pub fn base_wall_ns(&self) -> Option<u64> {
        let b = self.base_wall_ns.load(Ordering::Relaxed);
        (b != UNSET).then_some(b)
    }
}

/// Extrapolates an anchor to `wall_ns` at the nominal rate, with the stall clamp.
#[inline]
pub fn extrapolate(a: Anchor, wall_ns: u64, sample_rate: u32) -> f64 {
    let dt_ns = wall_ns as f64 - a.wall_ns as f64;
    let pos = a.frame as f64 + dt_ns * f64::from(sample_rate) / NANOS;
    let max = (a.frame + 2 * a.span_frames.max(1)) as f64;
    pos.clamp(0.0, max)
}

/// Per-consumer monotonic view of the clock (each reader thread owns one).
#[derive(Debug, Default)]
pub struct MonotonicReader {
    last_frames: f64,
}

impl MonotonicReader {
    /// Position in seconds at `wall_ns`, never smaller than any value this reader returned before.
    pub fn seconds_at(&mut self, clock: &AudioClock, wall_ns: u64) -> Option<f64> {
        let p = clock.position_frames_at(wall_ns)?;
        if p > self.last_frames {
            self.last_frames = p;
        }
        Some(self.last_frames / f64::from(clock.sample_rate()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn none_before_first_anchor() {
        let c = AudioClock::new(48_000, Instant::now());
        assert!(c.anchor().is_none());
        assert!(c.position_frames_at(1_000).is_none());
        assert!(c.drift_ns().is_none());
    }

    #[test]
    fn extrapolates_and_clamps() {
        let a = Anchor {
            frame: 48_000,
            wall_ns: 2_000_000_000,
            span_frames: 480,
        };
        // 5 ms after the anchor: +240 frames.
        assert!((extrapolate(a, 2_005_000_000, 48_000) - 48_240.0).abs() < 1e-6);
        // Before the anchor (previous buffer still playing): allowed, but never below zero.
        assert!((extrapolate(a, 1_990_000_000, 48_000) - 47_520.0).abs() < 1e-6);
        assert_eq!(extrapolate(a, 0, 48_000), 0.0);
        // Stalled device: clamped to two buffers past the anchor.
        assert_eq!(extrapolate(a, 9_000_000_000, 48_000), 48_960.0);
    }

    #[test]
    fn latency_compensation_and_drift() {
        let c = AudioClock::new(48_000, Instant::now());
        // Frame 0 audible at 1.0 s (e.g. 1.0 s of start-up + latency), then a device clock that runs
        // 100 ppm fast: frame 48000 * k audible at 1.0 + k * (1 - 1e-4) s.
        for k in 0..=10u64 {
            let wall = 1_000_000_000.0 + k as f64 * 1e9 * (1.0 - 1e-4);
            c.publish(Anchor {
                frame: k * 48_000,
                wall_ns: wall as u64,
                span_frames: 480,
            });
        }
        let drift_ms = c.drift_ns().unwrap() / 1e6;
        // Baseline taken at k = 1, so 9 s of 100 ppm = 0.9 ms.
        assert!((drift_ms - 0.9).abs() < 0.01, "drift {drift_ms} ms");
        let w = c.wall_ns_of_frame(480_000.0).unwrap();
        assert!((w - (1e9 + 10.0 * 1e9 * (1.0 - 1e-4))).abs() < 1.0);
    }

    #[test]
    fn monotonic_reader_never_goes_back() {
        let c = AudioClock::new(48_000, Instant::now());
        c.publish(Anchor {
            frame: 4800,
            wall_ns: 100_000_000,
            span_frames: 480,
        });
        let mut r = MonotonicReader::default();
        let t1 = r.seconds_at(&c, 104_000_000).unwrap();
        // A new anchor that says we were slightly ahead: raw position would step back.
        c.publish(Anchor {
            frame: 5280,
            wall_ns: 111_000_000,
            span_frames: 480,
        });
        let t2 = r.seconds_at(&c, 104_500_000).unwrap();
        assert!(t2 >= t1);
    }

    #[test]
    fn seqlock_never_tears() {
        let c = Arc::new(AudioClock::new(48_000, Instant::now()));
        let stop = Arc::new(AtomicBool::new(false));
        let writer = {
            let (c, stop) = (c.clone(), stop.clone());
            std::thread::spawn(move || {
                let mut f = 1u64;
                while !stop.load(Ordering::Relaxed) {
                    c.publish(Anchor {
                        frame: f,
                        wall_ns: f * 2,
                        span_frames: f * 3,
                    });
                    f += 1;
                }
            })
        };
        let mut reads = 0;
        while reads < 200_000 {
            if let Some(a) = c.anchor() {
                assert_eq!(a.wall_ns, a.frame * 2);
                assert_eq!(a.span_frames, a.frame * 3);
                reads += 1;
            }
        }
        stop.store(true, Ordering::Relaxed);
        writer.join().unwrap();
    }
}
