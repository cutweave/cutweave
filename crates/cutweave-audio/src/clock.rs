// SPDX-License-Identifier: GPL-3.0-or-later
//! The master clock (arch §9, §10): [`AudioClock`], the deterministic [`ManualClock`], and the
//! writer the output callback publishes through.
//!
//! # How the position is computed
//!
//! The output callback is the only writer. Each callback publishes an *anchor*: "output sample
//! `n` (the first frame of the buffer just mixed) becomes audible at host time `t`", where
//! `t = callback entry + device latency` (cpal's `playback − callback` timestamps). Readers
//! extrapolate the anchor at the nominal rate:
//!
//! ```text
//! position(now) = min(max(floor, anchor + (now − t)), end)
//! ```
//!
//! * `end` is the end of the audio handed to the device: nothing later can be audible, so a
//!   stalled device freezes the clock instead of letting it run ahead.
//! * `floor` makes the clock **monotonic** for every reader on every thread: when the writer
//!   publishes a new anchor it first evaluates the previous one at that instant and never lets
//!   the new line start below it. Anchors jitter by up to about 0.6 ms (Spike S2 finding 5);
//!   a new anchor that is behind holds the clock for that long instead of stepping it back.
//! * While the clock is not [`ClockState::Running`] it holds at `floor`.
//!
//! This is `timeline start + (samples played − latency) / rate` (WBS 1G.02), refined by
//! extrapolation between callbacks.
//!
//! The anchor is published through a seqlock built from atomics: the writer never blocks or
//! allocates, readers retry on a torn read. A reader takes `now` *before* it reads the anchor and
//! the writer evaluates the previous anchor *after* it marked the write in progress (behind a
//! full fence, plus a 5 µs margin for out-of-order timer reads), so a reader that saw the old
//! anchor always used an earlier `now` than the one the new floor was computed at. That makes
//! the clock monotonic across threads, not only per reader (tested with four reader threads).
//!
//! Discontinuities are explicit: a seek starts a new **generation** (readers compare
//! [`ClockReading::generation`]); within a generation the position never decreases. A stream
//! restart after a device change keeps the generation, and the clock stays monotonic.

use std::hint::spin_loop;
use std::sync::atomic::{AtomicI64, AtomicU8, AtomicU64, Ordering, fence};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use cutweave_model::{Duration, FLICKS_PER_SECOND, SampleRate, Time};

const NANOS_PER_SECOND: i128 = 1_000_000_000;

/// How much later than the writer's own reading of the host time the previous anchor is
/// evaluated when a new one is published (see [`ClockWriter::write`]). Readers that saw the
/// old anchor may have read the timer up to a store-buffer drain or an out-of-order timer read
/// later (sub-microsecond); 5 µs covers that with room. The cost: a published floor can be up
/// to 5 µs ahead of the previous line, which only holds the clock for that long.
const FLOOR_MARGIN_NS: u64 = 5_000;

/// What the clock is doing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ClockState {
    /// No output stream runs (not started yet, restarting after a device change, or stopped).
    /// The position holds.
    #[default]
    Stopped,
    /// The output is paused. The position holds.
    Paused,
    /// Waiting for audio to become audible: right after the stream (re)started, or while the
    /// mixer primes after a seek. The position holds.
    Priming,
    /// The device is playing the mix: the position advances with the device clock.
    Running,
}

impl ClockState {
    const fn to_u8(self) -> u8 {
        match self {
            ClockState::Stopped => 0,
            ClockState::Paused => 1,
            ClockState::Priming => 2,
            ClockState::Running => 3,
        }
    }

    const fn from_u8(v: u8) -> ClockState {
        match v {
            1 => ClockState::Paused,
            2 => ClockState::Priming,
            3 => ClockState::Running,
            _ => ClockState::Stopped,
        }
    }
}

/// One consistent reading of the clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockReading {
    /// Timeline position of the audio that is audible now (latency-compensated).
    pub position: Time,
    /// What the clock is doing.
    pub state: ClockState,
    /// Increments at every discontinuity (a seek). Within one generation `position` never
    /// decreases, on any thread.
    pub generation: u64,
}

impl ClockReading {
    /// Whether the position is advancing.
    #[must_use]
    pub const fn is_running(&self) -> bool {
        matches!(self.state, ClockState::Running)
    }
}

/// `ns` nanoseconds in flicks, rounded towards −∞ (saturating).
pub(crate) fn nanos_to_flicks(ns: i128) -> i64 {
    let f = (ns * i128::from(FLICKS_PER_SECOND)).div_euclid(NANOS_PER_SECOND);
    i64::try_from(f).unwrap_or(if f < 0 { i64::MIN } else { i64::MAX })
}

/// The published state (also the writer's mirror of it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Published {
    pub state: ClockState,
    pub generation: u64,
    /// Timeline position (flicks) audible at host time `anchor_ns`.
    pub anchor: i64,
    pub anchor_ns: u64,
    /// The clock never reads below this (flicks); while not running it reads exactly this.
    pub floor: i64,
    /// End of the audio handed to the device (flicks): the clock never reads above it.
    pub end: i64,
    pub latency_ns: u64,
}

impl Published {
    const fn holding(state: ClockState, at: i64) -> Published {
        Published {
            state,
            generation: 0,
            anchor: at,
            anchor_ns: 0,
            floor: at,
            end: at,
            latency_ns: 0,
        }
    }

    /// The position (flicks) at host time `now_ns`. Non-decreasing in `now_ns`.
    pub fn value_at(&self, now_ns: u64) -> i64 {
        if self.state != ClockState::Running {
            return self.floor;
        }
        let elapsed = i128::from(now_ns) - i128::from(self.anchor_ns);
        let line = self.anchor.saturating_add(nanos_to_flicks(elapsed));
        line.max(self.floor).min(self.end.max(self.floor))
    }
}

/// The atomics behind an [`AudioClock`].
#[derive(Debug)]
pub(crate) struct ClockShared {
    epoch: Instant,
    rate: SampleRate,
    seq: AtomicU64,
    state: AtomicU8,
    generation: AtomicU64,
    anchor: AtomicI64,
    anchor_ns: AtomicU64,
    floor: AtomicI64,
    end: AtomicI64,
    latency_ns: AtomicU64,
}

impl ClockShared {
    fn new(rate: SampleRate, initial: Published) -> ClockShared {
        ClockShared {
            epoch: Instant::now(),
            rate,
            seq: AtomicU64::new(0),
            state: AtomicU8::new(initial.state.to_u8()),
            generation: AtomicU64::new(initial.generation),
            anchor: AtomicI64::new(initial.anchor),
            anchor_ns: AtomicU64::new(initial.anchor_ns),
            floor: AtomicI64::new(initial.floor),
            end: AtomicI64::new(initial.end),
            latency_ns: AtomicU64::new(initial.latency_ns),
        }
    }

    /// Host time: nanoseconds since this clock was created (`Instant`, i.e. QPC on Windows,
    /// `mach_absolute_time` on macOS; read without a syscall).
    #[inline]
    pub fn host_ns(&self) -> u64 {
        self.ns_of(Instant::now())
    }

    #[inline]
    fn ns_of(&self, at: Instant) -> u64 {
        u64::try_from(at.saturating_duration_since(self.epoch).as_nanos()).unwrap_or(u64::MAX)
    }

    /// The latest consistent published state.
    pub fn load(&self) -> Published {
        let mut spins = 0u32;
        loop {
            let s1 = self.seq.load(Ordering::Acquire);
            if s1 & 1 == 0 {
                let p = Published {
                    state: ClockState::from_u8(self.state.load(Ordering::Relaxed)),
                    generation: self.generation.load(Ordering::Relaxed),
                    anchor: self.anchor.load(Ordering::Relaxed),
                    anchor_ns: self.anchor_ns.load(Ordering::Relaxed),
                    floor: self.floor.load(Ordering::Relaxed),
                    end: self.end.load(Ordering::Relaxed),
                    latency_ns: self.latency_ns.load(Ordering::Relaxed),
                };
                fence(Ordering::Acquire);
                if self.seq.load(Ordering::Relaxed) == s1 {
                    return p;
                }
            }
            // The writer is mid-update (a few nanoseconds of straight-line code).
            spins = spins.wrapping_add(1);
            if spins.is_multiple_of(64) {
                std::thread::yield_now();
            } else {
                spin_loop();
            }
        }
    }

    fn read_at(&self, now_ns: u64) -> ClockReading {
        let p = self.load();
        ClockReading {
            position: clamp_time(p.value_at(now_ns)),
            state: p.state,
            generation: p.generation,
        }
    }
}

fn clamp_time(flicks: i64) -> Time {
    let f = flicks.clamp(Time::MIN.flicks(), Time::MAX.flicks());
    Time::from_flicks(f).unwrap_or(Time::ZERO)
}

/// The master clock: the timeline position of the audio that is audible now.
///
/// Cheap to clone; every method is lock-free and safe to call from any thread (it never blocks
/// the audio callback, which is its only writer). Get one from
/// [`AudioOutput::clock`](crate::AudioOutput::clock) or [`ManualClock::clock`].
#[derive(Clone, Debug)]
pub struct AudioClock {
    shared: Arc<ClockShared>,
}

impl AudioClock {
    /// The timeline position of the audio audible now (latency-compensated). Never decreases
    /// within a [`generation`](ClockReading::generation), on any thread.
    #[must_use]
    pub fn position(&self) -> Time {
        self.read().position
    }

    /// Position, state and generation, read consistently.
    #[must_use]
    pub fn read(&self) -> ClockReading {
        // `now` before the anchor: see the module docs (cross-thread monotonicity).
        let now = self.shared.host_ns();
        self.shared.read_at(now)
    }

    /// The position the clock will have at `at` if the device keeps playing (for example the
    /// next vsync). Not clamped to earlier readings, so not necessarily monotonic.
    #[must_use]
    pub fn position_at(&self, at: Instant) -> Time {
        let now = self.shared.ns_of(at);
        self.shared.read_at(now).position
    }

    /// Whether the position is advancing.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.state() == ClockState::Running
    }

    /// What the clock is doing.
    #[must_use]
    pub fn state(&self) -> ClockState {
        self.shared.load().state
    }

    /// The current generation (incremented by every seek).
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.shared.load().generation
    }

    /// The output (device) sample rate.
    #[must_use]
    pub fn sample_rate(&self) -> SampleRate {
        self.shared.rate
    }

    /// The output latency the device last reported (cpal `playback − callback`): the time from
    /// mixing a sample to hearing it, as far as software can see. Excludes what lies past the
    /// endpoint (wireless links, DAC), which needs a measured per-device offset (S2 finding 2).
    #[must_use]
    pub fn latency(&self) -> Duration {
        let ns = self.shared.load().latency_ns;
        Duration::from_flicks(nanos_to_flicks(i128::from(ns))).unwrap_or(Duration::ZERO)
    }
}

/// The single writer of a clock. Owned by the output callback (moved between streams on a
/// restart, so there is never more than one writer).
#[derive(Debug)]
pub(crate) struct ClockWriter {
    shared: Arc<ClockShared>,
    cur: Published,
    seq: u64,
}

impl ClockWriter {
    /// A stopped clock at output sample `start`, and its reader.
    pub fn new(rate: SampleRate, start: i64) -> (ClockWriter, AudioClock) {
        let at = rate.samples_to_time(start).map_or(0, Time::flicks);
        let cur = Published::holding(ClockState::Stopped, at);
        let shared = Arc::new(ClockShared::new(rate, cur));
        let clock = AudioClock {
            shared: shared.clone(),
        };
        (
            ClockWriter {
                shared,
                cur,
                seq: 0,
            },
            clock,
        )
    }

    /// Host time of this clock (see [`ClockShared::host_ns`]).
    #[inline]
    pub fn host_ns(&self) -> u64 {
        self.shared.host_ns()
    }

    /// The published state.
    pub const fn current(&self) -> &Published {
        &self.cur
    }

    #[inline]
    fn flicks(&self, sample: i64) -> Option<i64> {
        self.shared
            .rate
            .samples_to_time(sample)
            .ok()
            .map(Time::flicks)
    }

    /// Publishes the state `build` makes from the current one and the host time taken after
    /// the write was marked in progress. Wait-free, allocation-free.
    #[inline]
    fn write(
        &mut self,
        now: impl FnOnce(&ClockShared) -> u64,
        build: impl FnOnce(&Published, u64) -> Published,
    ) {
        let s = &*self.shared;
        s.seq.store(self.seq.wrapping_add(1), Ordering::Relaxed);
        // Full fence: the "in progress" mark must be visible to readers before the host time
        // is read (a release fence lets it wait in the store buffer while time passes).
        fence(Ordering::SeqCst);
        // Timer reads are not ordered with memory accesses (TSC / counter reads execute out of
        // order), so a reader may pair the old anchor with a `now` slightly later than ours:
        // evaluate the old anchor a little later to cover that.
        let t = now(s).saturating_add(FLOOR_MARGIN_NS);
        let next = build(&self.cur, t);
        s.state.store(next.state.to_u8(), Ordering::Relaxed);
        s.generation.store(next.generation, Ordering::Relaxed);
        s.anchor.store(next.anchor, Ordering::Relaxed);
        s.anchor_ns.store(next.anchor_ns, Ordering::Relaxed);
        s.floor.store(next.floor, Ordering::Relaxed);
        s.end.store(next.end, Ordering::Relaxed);
        s.latency_ns.store(next.latency_ns, Ordering::Relaxed);
        self.seq = self.seq.wrapping_add(2);
        s.seq.store(self.seq, Ordering::Release);
        self.cur = next;
    }

    /// The device is playing: output sample `anchor` is audible at host time `audible_ns`, and
    /// everything before output sample `end` has been handed to the device. With `reset` (a
    /// seek) a new generation starts at `anchor`, held until it is audible.
    pub fn run(
        &mut self,
        anchor: i64,
        audible_ns: u64,
        end: i64,
        latency_ns: u64,
        reset: bool,
        now: impl FnOnce(&ClockShared) -> u64,
    ) {
        let (Some(anchor), Some(end)) = (self.flicks(anchor), self.flicks(end)) else {
            return; // beyond ±2⁵³ flicks (months of timeline): keep the last state
        };
        self.write(now, |cur, t| {
            let (generation, floor) = if reset {
                (cur.generation.wrapping_add(1), anchor)
            } else {
                (cur.generation, cur.value_at(t))
            };
            Published {
                state: ClockState::Running,
                generation,
                anchor,
                anchor_ns: audible_ns,
                floor,
                end: end.max(floor),
                latency_ns,
            }
        });
    }

    /// The clock holds in `state`. With `reset_to` (a seek) a new generation starts, held at
    /// that output sample; otherwise it holds where it is now.
    pub fn hold(
        &mut self,
        state: ClockState,
        reset_to: Option<i64>,
        latency_ns: Option<u64>,
        now: impl FnOnce(&ClockShared) -> u64,
    ) {
        let target = match reset_to {
            Some(sample) => match self.flicks(sample) {
                Some(f) => Some(f),
                None => return,
            },
            None => None,
        };
        if target.is_none()
            && self.cur.state == state
            && state != ClockState::Running
            && latency_ns.is_none_or(|l| l == self.cur.latency_ns)
        {
            return; // already holding there: nothing to publish
        }
        self.write(now, |cur, t| {
            let (generation, at) = match target {
                Some(f) => (cur.generation.wrapping_add(1), f),
                None => (cur.generation, cur.value_at(t)),
            };
            Published {
                state,
                generation,
                anchor: at,
                anchor_ns: t,
                floor: at,
                end: at,
                latency_ns: latency_ns.unwrap_or(cur.latency_ns),
            }
        });
    }

    /// Sets an exact position (flicks) in `state` ([`ManualClock`]); `reset` starts a new
    /// generation. A running exact clock does not extrapolate.
    fn set_exact(&mut self, state: ClockState, at: i64, reset: bool) {
        self.write(
            |_| 0,
            |cur, _| Published {
                state,
                generation: if reset {
                    cur.generation.wrapping_add(1)
                } else {
                    cur.generation
                },
                anchor: at,
                anchor_ns: 0,
                floor: at,
                end: at,
                latency_ns: cur.latency_ns,
            },
        );
    }
}

/// A deterministic clock for tests: it moves only when told to (no wall clock, no device).
///
/// Its [`AudioClock`] behaves like a device clock to readers (position, state, generation), so
/// code that follows the audio clock (WP-1H.01 transport, A/V sync) can be driven frame by
/// frame.
#[derive(Debug)]
pub struct ManualClock {
    writer: Mutex<ClockWriter>,
    clock: AudioClock,
}

impl ManualClock {
    /// A paused clock at [`Time::ZERO`] for output rate `rate`.
    #[must_use]
    pub fn new(rate: SampleRate) -> ManualClock {
        let (mut writer, clock) = ClockWriter::new(rate, 0);
        writer.set_exact(ClockState::Paused, 0, false);
        ManualClock {
            writer: Mutex::new(writer),
            clock,
        }
    }

    fn with<R>(&self, f: impl FnOnce(&mut ClockWriter) -> R) -> R {
        f(&mut self.writer.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// The reader half.
    #[must_use]
    pub fn clock(&self) -> AudioClock {
        self.clock.clone()
    }

    /// Sets the state (the position is kept).
    pub fn set_state(&self, state: ClockState) {
        self.with(|w| {
            let at = w.current().floor;
            w.set_exact(state, at, false);
        });
    }

    /// Marks the clock running.
    pub fn play(&self) {
        self.set_state(ClockState::Running);
    }

    /// Marks the clock paused.
    pub fn pause(&self) {
        self.set_state(ClockState::Paused);
    }

    /// Moves the position forward by `d` (whatever the state) and returns it.
    pub fn advance(&self, d: Duration) -> Time {
        self.with(|w| {
            let cur = *w.current();
            let at = cur.floor.saturating_add(d.flicks().max(0));
            w.set_exact(cur.state, at, false);
            clamp_time(at)
        })
    }

    /// Moves the position forward by `samples` output samples (whatever the state) and returns
    /// it.
    pub fn advance_samples(&self, samples: i64) -> Time {
        let d = self
            .clock
            .sample_rate()
            .samples_to_time(samples.max(0))
            .map_or(Duration::ZERO, |t| {
                Duration::try_from(t).unwrap_or(Duration::ZERO)
            });
        self.advance(d)
    }

    /// Jumps to `t`, starting a new generation (a seek).
    pub fn seek(&self, t: Time) {
        self.with(|w| {
            let state = w.current().state;
            w.set_exact(state, t.flicks(), true);
        });
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss
    )]

    use std::sync::atomic::AtomicBool;

    use proptest::prelude::*;

    use super::*;

    const RATE: SampleRate = SampleRate::HZ_48_000;
    /// Flicks per sample at 48 kHz.
    const FPS: i64 = 14_700;
    const MS: u64 = 1_000_000;

    fn writer() -> (ClockWriter, AudioClock) {
        ClockWriter::new(RATE, 0)
    }

    #[test]
    fn nanos_convert_to_flicks() {
        assert_eq!(nanos_to_flicks(1_000_000_000), FLICKS_PER_SECOND);
        assert_eq!(nanos_to_flicks(0), 0);
        // One 48 kHz sample is 20 833.3 ns = 14 700 flicks.
        assert_eq!(nanos_to_flicks(20_833), 14_699);
        assert_eq!(nanos_to_flicks(-1), -1);
    }

    #[test]
    fn starts_stopped_at_the_start_sample() {
        let (_, clock) = ClockWriter::new(RATE, 48_000);
        let r = clock.read();
        assert_eq!(r.state, ClockState::Stopped);
        assert_eq!(r.position, Time::from_secs(1).unwrap());
        assert!(!clock.is_running());
    }

    #[test]
    fn extrapolates_between_anchors_and_freezes_at_the_end_of_the_audio() {
        let (mut w, clock) = writer();
        // Sample 4800 (100 ms) audible at t = 1 s; 480 frames handed over (up to 5280).
        w.run(4800, 1_000 * MS, 5280, 30 * MS, false, |_| 970 * MS);
        let p = w.current();
        // 5 ms after the anchor: +240 samples.
        assert_eq!(p.value_at(1_005 * MS), (4800 + 240) * FPS);
        // Before the anchor (the previous buffer still plays) the line is below, but the floor
        // (where the stopped clock was, 0) is lower still.
        assert_eq!(p.value_at(990 * MS), (4800 - 480) * FPS);
        // A stalled device: never past the audio handed over.
        assert_eq!(p.value_at(9_000 * MS), 5280 * FPS);
        assert!(clock.is_running());
        assert_eq!(clock.latency().flicks(), 30 * FLICKS_PER_SECOND / 1000);
    }

    #[test]
    fn an_anchor_behind_the_previous_line_holds_instead_of_stepping_back() {
        let (mut w, _) = writer();
        w.run(4800, 1_000 * MS, 5280, 0, false, |_| 1_000 * MS);
        // Next buffer: the device says sample 5280 is audible at 1.0105 s, 0.5 ms later than the
        // previous anchor predicted (1.010 s). Published at 1.004 s.
        // The old line, evaluated with the writer's margin.
        let before = w.current().value_at(1_004 * MS + FLOOR_MARGIN_NS);
        w.run(5280, 1_010_500_000, 5760, 0, false, |_| 1_004 * MS);
        let p = *w.current();
        assert_eq!(p.floor, before);
        assert!(p.value_at(1_004 * MS) >= before);
        assert!(p.value_at(1_004_100_000) >= p.value_at(1_004 * MS));
    }

    #[test]
    fn holds_keep_the_position_and_resets_start_a_generation() {
        let (mut w, clock) = writer();
        w.run(4800, 1_000 * MS, 5280, 0, false, |_| 1_000 * MS);
        w.hold(ClockState::Paused, None, None, |_| 1_002 * MS);
        let held = w.current().floor;
        // 2 ms after the anchor (+96 samples), plus the writer's 5 µs margin.
        assert_eq!(
            held,
            (4800 + 96) * FPS + nanos_to_flicks(i128::from(FLOOR_MARGIN_NS))
        );
        assert_eq!(clock.state(), ClockState::Paused);
        assert_eq!(clock.generation(), 0);
        // A seek while paused: new generation at the target.
        w.hold(ClockState::Priming, Some(96_000), None, |_| 1_003 * MS);
        assert_eq!(clock.generation(), 1);
        assert_eq!(clock.position(), Time::from_secs(2).unwrap());
        // Playing again from the target: held there until it is audible.
        w.run(96_000, 1_050 * MS, 96_480, 30 * MS, false, |_| 1_020 * MS);
        assert_eq!(w.current().value_at(1_020 * MS), 96_000 * FPS);
        assert_eq!(w.current().value_at(1_060 * MS), (96_000 + 480) * FPS);
    }

    #[test]
    fn a_running_reset_holds_at_the_target_until_audible() {
        let (mut w, _) = writer();
        w.run(480_000, 100 * MS, 480_480, 0, false, |_| 100 * MS);
        w.run(4800, 200 * MS, 5280, 30 * MS, true, |_| 170 * MS);
        let p = *w.current();
        assert_eq!(p.generation, 1);
        assert_eq!(p.value_at(170 * MS), 4800 * FPS);
        assert_eq!(p.value_at(205 * MS), (4800 + 240) * FPS);
    }

    #[test]
    fn manual_clock_is_exact() {
        let m = ManualClock::new(RATE);
        let c = m.clock();
        assert_eq!(c.state(), ClockState::Paused);
        m.play();
        assert!(c.is_running());
        let one_frame = Duration::from_flicks(FLICKS_PER_SECOND / 30).unwrap();
        for k in 1..=300 {
            assert_eq!(m.advance(one_frame).flicks(), k * one_frame.flicks());
            assert_eq!(c.position().flicks(), k * one_frame.flicks());
        }
        assert_eq!(m.advance_samples(48_000), Time::from_secs(11).unwrap());
        m.seek(Time::from_secs(3).unwrap());
        assert_eq!(c.read().generation, 1);
        assert_eq!(c.position(), Time::from_secs(3).unwrap());
        m.pause();
        assert_eq!(c.state(), ClockState::Paused);
        assert_eq!(c.position(), Time::from_secs(3).unwrap());
    }

    /// One step of a simulated device: a callback (with jittered timing and latency, some of
    /// them priming or paused, some seeking) or a reader somewhere between callbacks.
    #[derive(Clone, Debug)]
    enum Step {
        Callback {
            frames: u16,
            /// Lateness of the callback after its nominal time (µs).
            late_us: u16,
            /// Latency jitter around 30 ms (µs, ±).
            jitter_us: i16,
            kind: u8,
        },
        Read {
            after_us: u16,
        },
    }

    fn steps() -> impl Strategy<Value = Vec<Step>> {
        let step = prop_oneof![
            (1u16..2048, 0u16..3000, -900i16..900, 0u8..20).prop_map(
                |(frames, late_us, jitter_us, kind)| Step::Callback {
                    frames,
                    late_us,
                    jitter_us,
                    kind
                }
            ),
            (0u16..12_000).prop_map(|after_us| Step::Read { after_us }),
        ];
        prop::collection::vec(step, 1..400)
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

        /// WBS 1G.02: `AudioClock::position()` is monotonic non-decreasing (within a generation),
        /// whatever the callback sizes, scheduling jitter, latency jitter, priming, pauses and
        /// stream restarts; seeks start a new generation.
        #[test]
        fn position_is_monotonic_within_a_generation(steps in steps()) {
            let (mut w, _) = writer();
            let rate = f64::from(RATE.hz());
            let mut now: u64 = 1_000 * MS;
            let mut nominal: u64 = now;
            let mut pos: i64 = 0; // mixer position (output samples)
            let mut last: Option<(u64, i64)> = None; // (generation, value)
            let mut check = |p: &Published, t: u64| -> Result<(), TestCaseError> {
                let v = p.value_at(t);
                if let Some((g, prev)) = last
                    && g == p.generation
                {
                    prop_assert!(v >= prev, "clock went back: {} -> {} at {}", prev, v, t);
                }
                last = Some((p.generation, v));
                Ok(())
            };
            for step in steps {
                match step {
                    Step::Callback { frames, late_us, jitter_us, kind } => {
                        let frames = i64::from(frames);
                        now = now.max(nominal + u64::from(late_us) * 1000);
                        let latency = (30_000 + i64::from(jitter_us)) as u64 * 1000;
                        match kind {
                            0 => {
                                // A seek: the mixer jumps and primes.
                                pos = (pos + 7_919) % 500_000;
                                w.hold(ClockState::Priming, Some(pos), Some(latency), |_| now);
                            }
                            1 => w.hold(ClockState::Paused, None, None, |_| now),
                            2 => w.hold(ClockState::Stopped, None, None, |_| now), // restart
                            3 => w.hold(ClockState::Priming, None, Some(latency), |_| now),
                            _ => {
                                w.run(pos, now + latency, pos + frames, latency, false, |_| now);
                                pos += frames;
                            }
                        }
                        check(w.current(), now)?;
                        nominal += (frames as f64 / rate * 1e9) as u64;
                    }
                    Step::Read { after_us } => {
                        now += u64::from(after_us) * 1000;
                        check(w.current(), now)?;
                    }
                }
            }
        }
    }

    /// The same across threads: a writer publishing jittered anchors at full speed, readers on
    /// four threads. A read that starts after another read finished never returns less.
    #[test]
    fn position_is_monotonic_across_threads() {
        let (mut w, clock) = writer();
        let stop = Arc::new(AtomicBool::new(false));
        // Highest value returned by any finished read (flicks).
        let seen = Arc::new(AtomicI64::new(i64::MIN));
        let readers: Vec<_> = (0..4)
            .map(|_| {
                let (clock, stop, seen) = (clock.clone(), stop.clone(), seen.clone());
                std::thread::spawn(move || {
                    let mut reads = 0u64;
                    while !stop.load(Ordering::Relaxed) {
                        let before = seen.load(Ordering::Acquire);
                        let r = clock.read();
                        assert_eq!(r.generation, 0);
                        let v = r.position.flicks();
                        assert!(v >= before, "read {v} after a finished read of {before}");
                        seen.fetch_max(v, Ordering::AcqRel);
                        reads += 1;
                    }
                    reads
                })
            })
            .collect();
        let start = Instant::now();
        let mut pos = 0i64;
        let mut k = 0u64;
        while start.elapsed() < std::time::Duration::from_millis(300) {
            // 256-frame buffers, latency jittering by ±1 ms around 20 ms.
            let jitter = (k * 7_919 % 2_000_000) as i64 - 1_000_000;
            let latency = (20_000_000 + jitter) as u64;
            let now = w.host_ns();
            w.run(
                pos,
                now + latency,
                pos + 256,
                latency,
                false,
                ClockShared::host_ns,
            );
            pos += 256;
            k += 1;
            if k.is_multiple_of(97) {
                w.hold(ClockState::Priming, None, None, ClockShared::host_ns);
            }
        }
        stop.store(true, Ordering::Relaxed);
        let reads: u64 = readers.into_iter().map(|r| r.join().unwrap()).sum();
        assert!(reads > 1000, "only {reads} reads");
    }

    #[test]
    fn seqlock_never_tears() {
        let (mut w, clock) = writer();
        let stop = Arc::new(AtomicBool::new(false));
        let reader = {
            let (clock, stop) = (clock.clone(), stop.clone());
            std::thread::spawn(move || {
                let mut n = 0;
                while !stop.load(Ordering::Relaxed) || n < 10_000 {
                    let p = clock.shared.load();
                    if p.state == ClockState::Running {
                        // Every published anchor has end = anchor + 480 samples and the matching ns.
                        assert_eq!(p.end - p.anchor, 480 * FPS);
                        assert_eq!(p.anchor_ns, (p.anchor / FPS) as u64 * 1000);
                    }
                    n += 1;
                }
            })
        };
        for k in 1..200_000i64 {
            let anchor = k * 480;
            w.run(anchor, anchor as u64 * 1000, anchor + 480, 0, true, |_| 0);
        }
        stop.store(true, Ordering::Relaxed);
        reader.join().unwrap();
    }
}
