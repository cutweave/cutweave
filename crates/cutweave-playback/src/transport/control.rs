// SPDX-License-Identifier: GPL-3.0-or-later
//! [`Transport`]: the control handle behind the `view.*` transport commands.
//!
//! Control calls (play, pause, seek, rate, step) are serialised by one lock that only the
//! control path takes: the calling thread updates the state machine, drives the audio
//! ([`AudioControl`]), starts a new epoch ([`EpochGate::advance`], which cancels in-flight
//! renders) and publishes the new state through a seqlock. The render thread, the ring clock
//! pump and the status pump only read that snapshot and the audio clock, lock-free.

use std::sync::atomic::{AtomicI64, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread::Thread;

use cutweave_audio::{AudioClock, ClockReading};
use cutweave_commands::Rev;
use cutweave_model::Time;

use crate::shm::{ClockSample, ClockState, PixelFormat, PreviewConfig};
use crate::transport::audio::AudioControl;
use crate::transport::cancel::EpochGate;
use crate::transport::host::HostClock;
use crate::transport::source::SequenceTiming;
use crate::transport::state::{
    FrameQuality, PlaybackState, Rate, Shuttle, Snapshot, SnapshotCell, TransportMode, shuttle,
};

/// Counters of the render loop since playback attached.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlaybackStats {
    /// Frames delivered to the sink.
    pub delivered: u64,
    /// Frames skipped during playback because rendering fell behind.
    pub dropped: u64,
    /// Renders abandoned because a newer request superseded them (never delivered).
    pub cancelled: u64,
    /// Renders that failed.
    pub errors: u64,
    /// Sequence number the sink gave the newest frame.
    pub last_seq: u64,
}

/// Render-loop counters, written by the render thread only (atomics, no locks).
#[derive(Debug)]
pub(crate) struct Stats {
    pub delivered: AtomicU64,
    pub dropped: AtomicU64,
    pub cancelled: AtomicU64,
    pub errors: AtomicU64,
    pub last_seq: AtomicU64,
    /// The newest delivered frame: index, document revision and quality.
    pub last_frame: AtomicI64,
    pub last_rev: AtomicU64,
    pub last_quality: AtomicU8,
    /// The epoch whose full-quality still is delivered (ends `Scrubbing`).
    pub refined_epoch: AtomicU64,
}

impl Stats {
    fn new() -> Stats {
        Stats {
            delivered: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            cancelled: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            last_seq: AtomicU64::new(0),
            last_frame: AtomicI64::new(-1),
            last_rev: AtomicU64::new(0),
            last_quality: AtomicU8::new(FrameQuality::Full.to_u8()),
            refined_epoch: AtomicU64::new(u64::MAX),
        }
    }
}

/// The transport's control state (behind the control lock).
pub(crate) struct Control {
    audio: Box<dyn AudioControl>,
    /// The published state (the writer's copy of the snapshot).
    snap: Snapshot,
}

/// Everything the transport shares with its threads.
pub(crate) struct Shared {
    control: Mutex<Control>,
    pub snapshot: SnapshotCell,
    pub gate: Arc<EpochGate>,
    pub host: Arc<dyn HostClock>,
    pub clock: AudioClock,
    pub stats: Stats,
    /// A pending `set_preview_size` (packed by [`pack_preview`]; 0 = none).
    pub preview_request: AtomicU64,
    pub render_thread: OnceLock<Thread>,
    pub status_thread: OnceLock<Thread>,
}

const PREVIEW_VALID: u64 = 1 << 63;
const PREVIEW_MASK: u64 = 0x00FF_FFFF;

/// A preview size request in one word: valid bit, width (24 bits), height (24 bits).
fn pack_preview(width: u32, height: u32) -> u64 {
    PREVIEW_VALID | ((u64::from(width) & PREVIEW_MASK) << 32) | (u64::from(height) & PREVIEW_MASK)
}

#[allow(clippy::cast_possible_truncation)] // fields were packed from 24-bit values
fn unpack_preview(v: u64) -> Option<(u32, u32)> {
    (v & PREVIEW_VALID != 0)
        .then_some((((v >> 32) & PREVIEW_MASK) as u32, (v & PREVIEW_MASK) as u32))
}

impl Shared {
    /// The newest unapplied preview size request (render thread).
    pub(crate) fn take_preview_request(&self) -> Option<(u32, u32)> {
        unpack_preview(self.preview_request.swap(0, Ordering::AcqRel))
    }

    fn wake(&self) {
        if let Some(t) = self.render_thread.get() {
            t.unpark();
        }
        if let Some(t) = self.status_thread.get() {
            t.unpark();
        }
    }

    pub(crate) fn state_of(
        &self,
        snap: &Snapshot,
        audio: &ClockReading,
        now_ns: u64,
    ) -> PlaybackState {
        let t = snap.position(audio, now_ns);
        let state = if snap.rate_milli != 0 {
            TransportMode::Playing
        } else if snap.scrub && self.stats.refined_epoch.load(Ordering::Acquire) != snap.epoch {
            TransportMode::Scrubbing
        } else {
            TransportMode::Stopped
        };
        PlaybackState {
            state,
            playing: snap.rate_milli != 0,
            rate: f64::from(snap.rate_milli) / 1000.0,
            t: Time::from_flicks(t).unwrap_or(Time::ZERO),
            rev: Rev(self.stats.last_rev.load(Ordering::Relaxed)),
            quality: FrameQuality::from_u8(self.stats.last_quality.load(Ordering::Relaxed)),
            dropped: u32::try_from(self.stats.dropped.load(Ordering::Relaxed)).unwrap_or(u32::MAX),
        }
    }
}

/// The transport: play, pause, seek, JKL rates and frame steps, plus lock-free reads of the
/// live state. Cheap to clone (all clones control the same playback); every method may be
/// called from any thread. See the [module docs](crate::transport) for the threading model.
#[derive(Clone)]
pub struct Transport {
    pub(crate) shared: Arc<Shared>,
}

impl std::fmt::Debug for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Transport")
            .field("state", &self.state())
            .field("epoch", &self.epoch())
            .finish_non_exhaustive()
    }
}

impl Transport {
    /// A stopped transport at 0 over `audio` for a sequence with `timing`; `host` times
    /// everything that is not the audio clock.
    pub(crate) fn new(
        audio: Box<dyn AudioControl>,
        timing: SequenceTiming,
        host: Arc<dyn HostClock>,
    ) -> Transport {
        let snap = Snapshot::new(timing);
        let clock = audio.clock();
        Transport {
            shared: Arc::new(Shared {
                control: Mutex::new(Control { audio, snap }),
                snapshot: SnapshotCell::new(&snap),
                gate: Arc::new(EpochGate::new()),
                host,
                clock,
                stats: Stats::new(),
                preview_request: AtomicU64::new(0),
                render_thread: OnceLock::new(),
                status_thread: OnceLock::new(),
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Control> {
        self.shared
            .control
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn now(&self) -> u64 {
        self.shared.host.now_ns()
    }

    /// Publishes `next` as a new request: cancels in-flight work first (so nothing of the old
    /// epoch can be committed once the new state is visible), then wakes the threads.
    fn publish(&self, c: &mut Control, mut next: Snapshot) -> PlaybackState {
        next.epoch = self.shared.gate.advance();
        c.snap = next;
        self.shared.snapshot.store(&next);
        self.shared.wake();
        self.shared
            .state_of(&next, &self.shared.clock.read(), self.now())
    }

    /// The state without a change.
    fn unchanged(&self, c: &Control) -> PlaybackState {
        self.shared
            .state_of(&c.snap, &self.shared.clock.read(), self.now())
    }
}

// Control. Each method returns the state after the change (the `view.*` command result);
// callers that only want the change may ignore it.
#[allow(clippy::must_use_candidate)]
impl Transport {
    /// Plays forward at normal speed with audio (`view.play`; L from stopped). At the end of
    /// the content, starts again from 0. Idempotent.
    pub fn play(&self) -> PlaybackState {
        self.set_rate(Rate::FORWARD)
    }

    /// Plays at `rate` (`view.set_rate`): 1× forward follows the audio clock; the other levels
    /// are video-only (audio muted) and step through the frame grid on the host clock.
    /// Idempotent.
    pub fn set_rate(&self, rate: Rate) -> PlaybackState {
        let mut c = self.lock();
        self.set_rate_in(&mut c, rate)
    }

    fn set_rate_in(&self, c: &mut Control, rate: Rate) -> PlaybackState {
        let cur = c.snap;
        if cur.rate_milli == rate.milli() {
            return self.unchanged(c);
        }
        let now = self.now();
        let mut pos = cur.position(&self.shared.clock.read(), now);
        let end = cur.end.max(0);
        if rate.is_reverse() {
            if pos <= 0 {
                // Nothing before the start: stop there.
                return self.pause_in(c);
            }
        } else if pos >= end {
            if end == 0 {
                return self.pause_in(c);
            }
            pos = 0;
        }
        let mut next = cur;
        next.rate_milli = rate.milli();
        next.scrub = false;
        next.anchor = pos;
        next.anchor_ns = now;
        if rate.follows_audio() {
            next.audio_gen = self.shared.clock.generation();
            c.audio.seek(Time::from_flicks(pos).unwrap_or(Time::ZERO));
            c.audio.play();
        } else if cur.rate().is_some_and(Rate::follows_audio) {
            c.audio.pause();
        }
        self.publish(c, next)
    }

    /// Pauses on the current position (`view.pause`; K); the frame refines to full quality.
    /// Idempotent.
    pub fn pause(&self) -> PlaybackState {
        let mut c = self.lock();
        self.pause_in(&mut c)
    }

    fn pause_in(&self, c: &mut Control) -> PlaybackState {
        let cur = c.snap;
        if cur.rate_milli == 0 {
            return self.unchanged(c);
        }
        let now = self.now();
        let pos = cur.position(&self.shared.clock.read(), now);
        if cur.rate().is_some_and(Rate::follows_audio) {
            c.audio.pause();
        }
        let mut next = cur;
        next.rate_milli = 0;
        next.scrub = false;
        next.anchor = pos;
        next.anchor_ns = now;
        self.publish(c, next)
    }

    /// Applies a JKL key: J/L start or speed up reverse/forward playback, K pauses.
    pub fn shuttle(&self, key: Shuttle) -> PlaybackState {
        let mut c = self.lock();
        match shuttle(c.snap.rate(), key) {
            Some(rate) => self.set_rate_in(&mut c, rate),
            None => self.pause_in(&mut c),
        }
    }

    /// Moves the playhead to `t` (`view.seek`; clamped to the content). While stopped this is
    /// a scrub: a quarter-resolution frame first, the full-quality frame after 100 ms without
    /// a newer seek. While playing, playback continues from `t` (at 1× after the audio
    /// re-primes). Every seek cancels in-flight renders (latest wins), except a seek to the
    /// frame already being shown or rendered, which only moves the playhead.
    pub fn seek(&self, t: Time) -> PlaybackState {
        let mut c = self.lock();
        let cur = c.snap;
        let now = self.now();
        let timing = cur.timing();
        let t = timing.clamp_time(t);
        let mut next = cur;
        next.anchor = t.flicks();
        next.anchor_ns = now;
        match cur.rate() {
            None => {
                let at = |f: i64| timing.frame_at(Time::from_flicks(f).unwrap_or(Time::ZERO));
                if timing.clamp_frame(at(t.flicks())) == timing.clamp_frame(at(cur.anchor)) {
                    // Same frame: keep the render in flight, restart the refine delay.
                    c.snap = next;
                    self.shared.snapshot.store(&next);
                    self.shared.wake();
                    return self.unchanged(&c);
                }
                next.scrub = true;
            }
            Some(rate) if rate.follows_audio() => {
                next.audio_gen = self.shared.clock.generation();
                c.audio.seek(t);
            }
            Some(_) => {}
        }
        self.publish(&mut c, next)
    }

    /// Pauses and moves `frames` frames along the sequence grid (`view.step`; negative steps
    /// back), clamped to the content. The frame renders at full quality.
    pub fn step(&self, frames: i64) -> PlaybackState {
        let mut c = self.lock();
        let cur = c.snap;
        let now = self.now();
        let pos = cur.position(&self.shared.clock.read(), now);
        let timing = cur.timing();
        let at = |f: i64| timing.frame_at(Time::from_flicks(f).unwrap_or(Time::ZERO));
        let frame = timing.clamp_frame(at(pos).saturating_add(frames));
        let mut next = cur;
        next.rate_milli = 0;
        next.scrub = false;
        next.anchor = timing.frame_start(frame).flicks();
        next.anchor_ns = now;
        if next.anchor == cur.anchor && cur.rate_milli == 0 && !cur.scrub {
            return self.unchanged(&c);
        }
        if cur.rate().is_some_and(Rate::follows_audio) {
            c.audio.pause();
        }
        self.publish(&mut c, next)
    }

    /// Re-renders the current frame and everything after it (the document changed). Playback
    /// continues; a stopped frame refines straight to full quality.
    pub fn refresh(&self) -> PlaybackState {
        let mut c = self.lock();
        let mut next = c.snap;
        next.content = next.content.wrapping_add(1);
        next.scrub = false;
        self.publish(&mut c, next)
    }

    /// Changes the sequence's frame grid or extent (the document changed) and refreshes. The
    /// playhead is clamped to the new content.
    pub fn set_timing(&self, timing: SequenceTiming) -> PlaybackState {
        let mut c = self.lock();
        let cur = c.snap;
        let now = self.now();
        let pos = cur.position(&self.shared.clock.read(), now);
        let mut next = cur;
        next.set_timing(timing);
        next.content = next.content.wrapping_add(1);
        next.scrub = false;
        let clamped = pos.clamp(0, next.end.max(0));
        match cur.rate() {
            None => next.anchor = clamped,
            Some(r) if r.follows_audio() => {}
            Some(_) => {
                next.anchor = clamped;
                next.anchor_ns = now;
            }
        }
        self.publish(&mut c, next)
    }

    /// Requests a new preview size in pixels (`view.set_preview_size`; the sink keeps its
    /// pixel format and fits the size to it). The render thread applies it before its next
    /// frame and re-renders the current one.
    pub fn set_preview_size(&self, width: u32, height: u32) {
        let fitted = PreviewConfig {
            width,
            height,
            format: PixelFormat::Rgba8,
        }
        .fitted();
        self.shared
            .preview_request
            .store(pack_preview(fitted.width, fitted.height), Ordering::Release);
        self.shared.wake();
    }

    /// Stops a playback that reached the end (forward) or the start (reverse), unless a newer
    /// request than `epoch` exists. Returns whether it stopped. Called by the status pump.
    pub(crate) fn stop_at_boundary(&self, epoch: u64) -> bool {
        let mut c = self.lock();
        let cur = c.snap;
        if cur.epoch != epoch || cur.rate_milli == 0 {
            return false;
        }
        let now = self.now();
        let pos = cur.position(&self.shared.clock.read(), now);
        if !cur.at_boundary(pos) {
            return false;
        }
        self.pause_in(&mut c);
        true
    }
}

// Lock-free reads.
impl Transport {
    /// The live state (`playback.state`).
    #[must_use]
    pub fn state(&self) -> PlaybackState {
        let now = self.now();
        let snap = self.shared.snapshot.load();
        self.shared.state_of(&snap, &self.shared.clock.read(), now)
    }

    /// The live playhead.
    #[must_use]
    pub fn position(&self) -> Time {
        let snap = self.shared.snapshot.load();
        let p = snap.position(&self.shared.clock.read(), self.now());
        Time::from_flicks(p).unwrap_or(Time::ZERO)
    }

    /// The rate, or `None` when stopped.
    #[must_use]
    pub fn rate(&self) -> Option<Rate> {
        self.shared.snapshot.load().rate()
    }

    /// The sequence's frame grid and extent.
    #[must_use]
    pub fn timing(&self) -> SequenceTiming {
        self.shared.snapshot.load().timing()
    }

    /// The current request epoch (every control change starts a new one).
    #[must_use]
    pub fn epoch(&self) -> u64 {
        self.shared.gate.current()
    }

    /// The epoch gate (frames of older epochs are never delivered).
    #[must_use]
    pub fn gate(&self) -> &Arc<EpochGate> {
        &self.shared.gate
    }

    /// The master clock 1× playback follows.
    #[must_use]
    pub fn clock(&self) -> &AudioClock {
        &self.shared.clock
    }

    /// The render loop's counters.
    #[must_use]
    pub fn stats(&self) -> PlaybackStats {
        let s = &self.shared.stats;
        PlaybackStats {
            delivered: s.delivered.load(Ordering::Relaxed),
            dropped: s.dropped.load(Ordering::Relaxed),
            cancelled: s.cancelled.load(Ordering::Relaxed),
            errors: s.errors.load(Ordering::Relaxed),
            last_seq: s.last_seq.load(Ordering::Relaxed),
        }
    }

    /// The ring's clock words now (the [`ClockPump`](crate::shm::ClockPump) source): the
    /// position on the host clock, `Playing` only while the position advances (not while the
    /// audio primes after a seek, nor at the end), `Scrubbing` while a seek is refined.
    /// `host_ns` is on this transport's host clock, which must be
    /// [`SystemClock`](crate::transport::SystemClock) for ring readers.
    #[must_use]
    pub fn clock_sample(&self) -> ClockSample {
        let now = self.now();
        let snap = self.shared.snapshot.load();
        let audio = self.shared.clock.read();
        let pos = snap.position(&audio, now);
        let mode = self.shared.state_of(&snap, &audio, now).state;
        let advancing = match snap.rate() {
            None => false,
            Some(r) if r.follows_audio() => audio.is_running() && !snap.audio_pending(&audio),
            Some(_) => true,
        } && !snap.at_boundary(pos);
        let state = match mode {
            TransportMode::Playing if advancing => ClockState::Playing,
            TransportMode::Scrubbing => ClockState::Scrubbing,
            _ => ClockState::Stopped,
        };
        ClockSample {
            position: Time::from_flicks(pos).unwrap_or(Time::ZERO),
            host_ns: now,
            rate_milli: if state == ClockState::Playing {
                snap.rate_milli
            } else {
                Rate::FORWARD.milli()
            },
            state,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_requests_round_trip() {
        for (w, h) in [(1920, 1080), (8192, 1), (1, 8192)] {
            assert_eq!(unpack_preview(pack_preview(w, h)), Some((w, h)));
        }
        assert_eq!(unpack_preview(0), None);
    }
}
