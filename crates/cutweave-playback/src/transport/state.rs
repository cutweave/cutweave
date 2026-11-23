// SPDX-License-Identifier: GPL-3.0-or-later
//! Transport state: JKL rates ([`Rate`], [`shuttle`]), the `playback.state` payload
//! ([`PlaybackState`]) and the lock-free snapshot the render thread, the clock pump and the
//! status pump read.

use std::sync::atomic::{AtomicI32, AtomicI64, AtomicU32, AtomicU64, Ordering, fence};

use cutweave_audio::ClockReading;
use cutweave_commands::{Rev, wire};
use cutweave_model::{FLICKS_PER_SECOND, FrameRate, Time};
use cutweave_render::Scale;

use crate::transport::source::SequenceTiming;

/// One of the JKL playback rates: ±1×, ±2×, ±4× (in thousandths, the ring's `rate_milli`).
///
/// Only 1× forward is audible and follows the audio clock. The mixer has no varispeed yet
/// (time-stretch is future work), so 2×/4× and every reverse rate are video-only in P1: the
/// audio is paused (muted) and video steps through the frame grid on the host clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rate(i32);

impl Rate {
    /// Normal speed, forward (L).
    pub const FORWARD: Rate = Rate(1000);
    /// Normal speed, reverse (J).
    pub const REVERSE: Rate = Rate(-1000);
    /// Every level, slowest reverse to fastest forward magnitude order: −4×, −2×, −1×, 1×, 2×, 4×.
    pub const LEVELS: [Rate; 6] = [
        Rate(-4000),
        Rate(-2000),
        Rate(-1000),
        Rate(1000),
        Rate(2000),
        Rate(4000),
    ];

    /// The level with `milli` thousandths, if it is one.
    #[must_use]
    pub fn from_milli(milli: i32) -> Option<Rate> {
        Rate::LEVELS.into_iter().find(|r| r.0 == milli)
    }

    /// The level equal to `rate` (1.0 = 1×), if it is one.
    #[must_use]
    pub fn from_f64(rate: f64) -> Option<Rate> {
        Rate::LEVELS
            .into_iter()
            .find(|r| (r.as_f64() - rate).abs() < 1e-9)
    }

    /// Thousandths of normal speed (negative in reverse).
    #[must_use]
    pub const fn milli(self) -> i32 {
        self.0
    }

    /// As a factor (1.0 = 1×, negative in reverse).
    #[must_use]
    pub fn as_f64(self) -> f64 {
        f64::from(self.0) / 1000.0
    }

    /// Whether it plays backwards.
    #[must_use]
    pub const fn is_reverse(self) -> bool {
        self.0 < 0
    }

    /// The speed magnitude: 1, 2 or 4.
    #[must_use]
    pub const fn speed(self) -> u32 {
        self.0.unsigned_abs() / 1000
    }

    /// Whether this rate plays audio and follows the audio clock (1× forward only).
    #[must_use]
    pub const fn follows_audio(self) -> bool {
        self.0 == Rate::FORWARD.0
    }

    /// The next faster level in the same direction (4× stays 4×).
    #[must_use]
    pub const fn faster(self) -> Rate {
        let m = self.0.unsigned_abs();
        let next = if m >= 2000 { 4000 } else { 2000 };
        Rate(if self.0 < 0 { -next } else { next })
    }
}

/// A JKL key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Shuttle {
    /// Reverse: −1×, then −2×, −4×.
    J,
    /// Pause.
    K,
    /// Forward: 1×, then 2×, 4×.
    L,
}

/// The rate after pressing `key` while playing at `current` (`None` = stopped); `None` means
/// pause. Pressing a direction key while stopped or playing the other way starts at 1× in that
/// direction; pressing it again speeds up to 2× and 4×.
#[must_use]
pub fn shuttle(current: Option<Rate>, key: Shuttle) -> Option<Rate> {
    match key {
        Shuttle::K => None,
        Shuttle::L => Some(match current {
            Some(r) if !r.is_reverse() => r.faster(),
            _ => Rate::FORWARD,
        }),
        Shuttle::J => Some(match current {
            Some(r) if r.is_reverse() => r.faster(),
            _ => Rate::REVERSE,
        }),
    }
}

/// What the transport is doing (`playback.state` `state`).
#[wire]
#[derive(Copy, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TransportMode {
    /// Stopped or paused on a frame.
    #[default]
    Stopped,
    /// Playing at `rate`.
    Playing,
    /// Stopped after a seek whose full-quality frame is not delivered yet.
    Scrubbing,
}

/// Preview resolution of a delivered frame (arch §8 quality ladder).
#[wire]
#[derive(Copy, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum FrameQuality {
    /// Full resolution.
    #[default]
    Full,
    /// Half resolution.
    Half,
    /// Quarter resolution.
    Quarter,
}

impl FrameQuality {
    /// The quality of frames rendered at `scale`.
    #[must_use]
    pub const fn of(scale: Scale) -> FrameQuality {
        match scale {
            Scale::Full => FrameQuality::Full,
            Scale::Half => FrameQuality::Half,
            Scale::Quarter => FrameQuality::Quarter,
        }
    }

    pub(crate) const fn to_u8(self) -> u8 {
        match self {
            FrameQuality::Full => 0,
            FrameQuality::Half => 1,
            FrameQuality::Quarter => 2,
        }
    }

    pub(crate) const fn from_u8(v: u8) -> FrameQuality {
        match v {
            1 => FrameQuality::Half,
            2 => FrameQuality::Quarter,
            _ => FrameQuality::Full,
        }
    }
}

/// The `playback.state` event payload and the result of the `view.*` transport commands.
#[wire]
pub struct PlaybackState {
    /// `stopped`, `playing` or `scrubbing`.
    pub state: TransportMode,
    /// Whether the playhead advances.
    pub playing: bool,
    /// Playback rate: 1 normal, 2 and 4 fast forward, −1, −2 and −4 reverse; 0 when stopped.
    pub rate: f64,
    /// The live playhead in flicks (at 1× the audio clock: the audio audible now).
    pub t: Time,
    /// Document revision of the newest delivered frame.
    pub rev: Rev,
    /// Resolution of the newest delivered frame.
    pub quality: FrameQuality,
    /// Frames skipped since playback attached because rendering fell behind (saturates).
    pub dropped: u32,
}

/// Flicks per nanosecond as a ratio: `ns * FLICKS_PER_SECOND / 1e9`.
pub(crate) fn ns_to_flicks(ns: u64) -> i64 {
    let f = i128::from(ns) * i128::from(FLICKS_PER_SECOND) / 1_000_000_000;
    i64::try_from(f).unwrap_or(i64::MAX)
}

/// Nanoseconds of `flicks` (rounded up), for wake-up deadlines.
pub(crate) fn flicks_to_ns(flicks: i64) -> u64 {
    let f = i128::from(flicks.max(0));
    let ns =
        (f * 1_000_000_000 + i128::from(FLICKS_PER_SECOND) - 1) / i128::from(FLICKS_PER_SECOND);
    u64::try_from(ns).unwrap_or(u64::MAX)
}

/// The transport state as the render thread, the clock pump and the status pump see it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Snapshot {
    /// The request this state answers ([`EpochGate`](crate::transport::EpochGate)).
    pub epoch: u64,
    /// Incremented by refreshes (document or timing changes): frames rendered before it are
    /// out of date even at the same frame index.
    pub content: u64,
    /// 0 when stopped, else the [`Rate`] in thousandths.
    pub rate_milli: i32,
    /// Stopped after a seek: quarter frame first, full refine after the idle delay.
    pub scrub: bool,
    /// Stopped: the position. 1×: the position audio was seeked to. Other rates: the position
    /// at host time `anchor_ns`.
    pub anchor: i64,
    /// Other rates: host time of `anchor`. Stopped: host time of the last seek.
    pub anchor_ns: u64,
    /// 1×: the audio clock generation before the seek; the position holds at `anchor` until
    /// the clock moves past it.
    pub audio_gen: u64,
    /// Sequence end (flicks, exclusive).
    pub end: i64,
    /// Sequence frame rate.
    pub fps_num: u32,
    /// Sequence frame rate.
    pub fps_den: u32,
}

impl Snapshot {
    pub(crate) fn new(timing: SequenceTiming) -> Snapshot {
        Snapshot {
            epoch: 0,
            content: 0,
            rate_milli: 0,
            scrub: false,
            anchor: 0,
            anchor_ns: 0,
            audio_gen: 0,
            end: timing.end.flicks(),
            fps_num: timing.rate.num(),
            fps_den: timing.rate.den(),
        }
    }

    pub(crate) fn timing(&self) -> SequenceTiming {
        SequenceTiming::new(
            FrameRate::new(self.fps_num, self.fps_den).unwrap_or(FrameRate::FPS_30),
            Time::from_flicks(self.end).unwrap_or(Time::ZERO),
        )
    }

    pub(crate) fn set_timing(&mut self, timing: SequenceTiming) {
        self.end = timing.end.flicks();
        self.fps_num = timing.rate.num();
        self.fps_den = timing.rate.den();
    }

    pub(crate) fn rate(&self) -> Option<Rate> {
        Rate::from_milli(self.rate_milli)
    }

    /// 1× playback whose audio seek has not reached the clock yet.
    pub(crate) fn audio_pending(&self, audio: &ClockReading) -> bool {
        self.rate_milli == Rate::FORWARD.0 && audio.generation <= self.audio_gen
    }

    /// The playhead (flicks) at host time `now_ns`, within `0..=end`.
    pub(crate) fn position(&self, audio: &ClockReading, now_ns: u64) -> i64 {
        let p = match self.rate_milli {
            0 => self.anchor,
            m if m == Rate::FORWARD.0 => {
                if self.audio_pending(audio) {
                    self.anchor
                } else {
                    // The clock starts the generation at the nearest output sample to the
                    // seek target, which can be a fraction of a sample before it.
                    audio.position.flicks().max(self.anchor)
                }
            }
            m => {
                let elapsed = ns_to_flicks(now_ns.saturating_sub(self.anchor_ns));
                let moved = i128::from(elapsed) * i128::from(m) / 1000;
                i64::try_from(i128::from(self.anchor) + moved).unwrap_or(self.anchor)
            }
        };
        p.clamp(0, self.end.max(0))
    }

    /// Whether playback reached the end (forward) or the start (reverse) at `position`.
    pub(crate) fn at_boundary(&self, position: i64) -> bool {
        match self.rate_milli {
            0 => false,
            m if m > 0 => position >= self.end,
            _ => position <= 0,
        }
    }
}

/// A [`Snapshot`] behind a seqlock: one writer (the transport's control lock serialises
/// writers), any number of wait-free readers that retry on a torn read. No allocation.
#[derive(Debug)]
pub(crate) struct SnapshotCell {
    seq: AtomicU64,
    epoch: AtomicU64,
    content: AtomicU64,
    rate_milli: AtomicI32,
    flags: AtomicU32,
    anchor: AtomicI64,
    anchor_ns: AtomicU64,
    audio_gen: AtomicU64,
    end: AtomicI64,
    fps_num: AtomicU32,
    fps_den: AtomicU32,
}

const FLAG_SCRUB: u32 = 1;

impl SnapshotCell {
    pub(crate) fn new(s: &Snapshot) -> SnapshotCell {
        let cell = SnapshotCell {
            seq: AtomicU64::new(0),
            epoch: AtomicU64::new(0),
            content: AtomicU64::new(0),
            rate_milli: AtomicI32::new(0),
            flags: AtomicU32::new(0),
            anchor: AtomicI64::new(0),
            anchor_ns: AtomicU64::new(0),
            audio_gen: AtomicU64::new(0),
            end: AtomicI64::new(0),
            fps_num: AtomicU32::new(0),
            fps_den: AtomicU32::new(0),
        };
        cell.store(s);
        cell
    }

    /// Publishes `s`. Callers serialise (the control lock).
    pub(crate) fn store(&self, s: &Snapshot) {
        let r = Ordering::Relaxed;
        let seq = self.seq.load(r);
        self.seq.store(seq.wrapping_add(1), r);
        // Orders the odd sequence number before the field stores (seqlock writer).
        fence(Ordering::Release);
        self.epoch.store(s.epoch, r);
        self.content.store(s.content, r);
        self.rate_milli.store(s.rate_milli, r);
        self.flags.store(if s.scrub { FLAG_SCRUB } else { 0 }, r);
        self.anchor.store(s.anchor, r);
        self.anchor_ns.store(s.anchor_ns, r);
        self.audio_gen.store(s.audio_gen, r);
        self.end.store(s.end, r);
        self.fps_num.store(s.fps_num, r);
        self.fps_den.store(s.fps_den, r);
        self.seq.store(seq.wrapping_add(2), Ordering::Release);
    }

    /// The latest consistent snapshot. Wait-free unless a write is in progress (a few
    /// nanoseconds of stores), then it retries.
    pub(crate) fn load(&self) -> Snapshot {
        let r = Ordering::Relaxed;
        let mut spins = 0u32;
        loop {
            let s1 = self.seq.load(Ordering::Acquire);
            if s1 & 1 == 0 {
                let s = Snapshot {
                    epoch: self.epoch.load(r),
                    content: self.content.load(r),
                    rate_milli: self.rate_milli.load(r),
                    scrub: self.flags.load(r) & FLAG_SCRUB != 0,
                    anchor: self.anchor.load(r),
                    anchor_ns: self.anchor_ns.load(r),
                    audio_gen: self.audio_gen.load(r),
                    end: self.end.load(r),
                    fps_num: self.fps_num.load(r),
                    fps_den: self.fps_den.load(r),
                };
                fence(Ordering::Acquire);
                if self.seq.load(r) == s1 {
                    return s;
                }
            }
            spins = spins.wrapping_add(1);
            if spins.is_multiple_of(64) {
                std::thread::yield_now();
            } else {
                std::hint::spin_loop();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use cutweave_audio::ClockState;

    use super::*;

    #[test]
    fn jkl_levels() {
        use Shuttle::{J, K, L};
        let r = |m| Rate::from_milli(m).unwrap();
        assert_eq!(shuttle(None, L), Some(Rate::FORWARD));
        assert_eq!(shuttle(Some(r(1000)), L), Some(r(2000)));
        assert_eq!(shuttle(Some(r(2000)), L), Some(r(4000)));
        assert_eq!(shuttle(Some(r(4000)), L), Some(r(4000)));
        assert_eq!(shuttle(Some(r(-4000)), L), Some(Rate::FORWARD));
        assert_eq!(shuttle(None, J), Some(Rate::REVERSE));
        assert_eq!(shuttle(Some(r(-1000)), J), Some(r(-2000)));
        assert_eq!(shuttle(Some(r(-2000)), J), Some(r(-4000)));
        assert_eq!(shuttle(Some(r(-4000)), J), Some(r(-4000)));
        assert_eq!(shuttle(Some(r(2000)), J), Some(Rate::REVERSE));
        for level in Rate::LEVELS {
            assert_eq!(shuttle(Some(level), K), None);
            assert_eq!(Rate::from_f64(level.as_f64()), Some(level));
        }
        assert_eq!(shuttle(None, K), None);
        assert_eq!(Rate::from_f64(0.5), None);
        assert_eq!(Rate::from_f64(3.0), None);
        assert_eq!(Rate::from_milli(0), None);
        assert!(Rate::FORWARD.follows_audio());
        assert!(!r(2000).follows_audio() && !Rate::REVERSE.follows_audio());
        assert_eq!((r(-4000).speed(), r(2000).speed()), (4, 2));
    }

    fn reading(position: i64, generation: u64) -> ClockReading {
        ClockReading {
            position: Time::from_flicks(position).unwrap(),
            state: ClockState::Running,
            generation,
        }
    }

    #[test]
    fn positions_follow_the_clock_or_the_host_time() {
        let sec = FLICKS_PER_SECOND;
        let timing = SequenceTiming::new(FrameRate::FPS_30, Time::from_secs(10).unwrap());
        let mut s = Snapshot::new(timing);
        s.anchor = 2 * sec;
        assert_eq!(s.position(&reading(0, 0), 5), 2 * sec, "stopped holds");
        // 1×: hold until the seek reaches the clock, then follow it (never below the target).
        s.rate_milli = 1000;
        s.audio_gen = 3;
        assert_eq!(s.position(&reading(7 * sec, 3), 0), 2 * sec);
        assert_eq!(s.position(&reading(2 * sec - 5, 4), 0), 2 * sec);
        assert_eq!(s.position(&reading(3 * sec, 4), 0), 3 * sec);
        assert_eq!(
            s.position(&reading(30 * sec, 4), 0),
            10 * sec,
            "clamped to the end"
        );
        // Host-driven: −2× from 2 s anchored at host 1 s.
        s.rate_milli = -2000;
        s.anchor_ns = 1_000_000_000;
        assert_eq!(s.position(&reading(0, 0), 1_500_000_000), sec);
        assert_eq!(s.position(&reading(0, 0), 9_000_000_000), 0, "clamped at 0");
        assert!(s.at_boundary(0));
        s.rate_milli = 4000;
        assert_eq!(s.position(&reading(0, 0), 1_250_000_000), 3 * sec);
        assert!(s.at_boundary(10 * sec) && !s.at_boundary(9 * sec));
        assert_eq!(ns_to_flicks(1_000_000_000), sec);
        assert_eq!(flicks_to_ns(sec), 1_000_000_000);
        assert_eq!(flicks_to_ns(1), 2);
    }

    #[test]
    fn readers_never_see_a_torn_snapshot() {
        let timing = SequenceTiming::new(FrameRate::FPS_30, Time::from_secs(10).unwrap());
        let make = |e: u64| {
            let mut s = Snapshot::new(timing);
            s.epoch = e;
            s.content = e * 3;
            s.anchor = i64::try_from(e).unwrap() * 7;
            s.anchor_ns = e + 1;
            s.audio_gen = e ^ 0x55;
            s.scrub = e % 2 == 1;
            s
        };
        let cell = Arc::new(SnapshotCell::new(&make(0)));
        let stop = Arc::new(AtomicBool::new(false));
        let reads = Arc::new(AtomicU64::new(0));
        let readers: Vec<_> = (0..3)
            .map(|_| {
                let (cell, stop, reads) =
                    (Arc::clone(&cell), Arc::clone(&stop), Arc::clone(&reads));
                std::thread::spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        let s = cell.load();
                        // Every field of a write derives from one counter.
                        let e = s.epoch;
                        assert_eq!(s.content, e * 3);
                        assert_eq!(s.anchor, i64::try_from(e).unwrap() * 7);
                        assert_eq!(s.anchor_ns, e + 1);
                        assert_eq!(s.audio_gen, e ^ 0x55);
                        assert_eq!(s.scrub, e % 2 == 1);
                        reads.fetch_add(1, Ordering::Relaxed);
                    }
                })
            })
            .collect();
        let t0 = std::time::Instant::now();
        let mut e = 0u64;
        while e < 200_000 || reads.load(Ordering::Relaxed) < 100_000 {
            assert!(
                t0.elapsed() < std::time::Duration::from_secs(20),
                "no progress"
            );
            e += 1;
            cell.store(&make(e));
        }
        stop.store(true, Ordering::Relaxed);
        for r in readers {
            r.join().unwrap();
        }
    }
}
