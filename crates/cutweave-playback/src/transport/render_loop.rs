// SPDX-License-Identifier: GPL-3.0-or-later
//! [`RenderLoop`]: the body of the render thread. Each [`tick`](RenderLoop::tick) reads the
//! transport snapshot and the audio clock (lock-free), renders at most one frame through the
//! [`FrameSource`] and delivers it to the [`FrameSink`], or says how long it may sleep.
//!
//! * **Playing.** The target is the frame (floor to the sequence grid) at the playhead
//!   predicted for when the render finishes: the audio clock at 1×, the host clock at the
//!   video-only JKL rates, plus the recent render time (capped by
//!   [`RenderConfig::max_lead`]). Late frames are skipped, never waited for, and audio is
//!   never touched: a slow render drops frames, it does not delay the clock. Frames come at
//!   [`AutoQuality`]'s rung; after each one the source gets a [`PrefetchHint`].
//! * **Stopped after a seek (scrubbing).** Latest wins: the frame cache first (a hit is final),
//!   else a quarter-resolution frame at once (`LOWRES`), then the full-quality frame once no
//!   newer seek arrived for [`RenderConfig::refine_after`] (100 ms, arch §10 step 5).
//! * **Stopped otherwise** (pause, step, refresh): the full-quality frame, unless that exact
//!   frame is already on screen.
//!
//! A new request (any transport change) starts a new epoch: renders of older epochs are told
//! through their [`CancelToken`](crate::transport::CancelToken) and can never be delivered
//! ([`EpochGate::try_commit`](crate::transport::EpochGate::try_commit)). A new target
//! (`view.set_source`, ADR-0022) is such a request, on the target's own frame grid; it also
//! changes the snapshot's content counter, so a frame of the old target on screen is never
//! taken for one of the new target.
//!
//! No locks, no blocking and no allocation per frame: the pixel buffer is reused (it grows only
//! when the preview size grows), state is read through seqlocks and counters are atomics.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration as StdDuration;

use cutweave_model::{FrameRate, Time};
use cutweave_render::{AutoQuality, AutoQualityConfig, PlaybackMode, Quality, Scale};

use crate::shm::PreviewConfig;
use crate::transport::control::Shared;
use crate::transport::sink::{FrameInfo, FrameSink, SinkError};
use crate::transport::source::{
    FrameSource, PrefetchHint, RenderPurpose, RenderRequest, SourceError,
};
use crate::transport::state::{FrameQuality, Rate, Snapshot, flicks_to_ns, ns_to_flicks};

/// The shortest sleep the loop asks for (avoids spinning on sub-microsecond deadlines).
const MIN_WAIT_NS: u64 = 100_000;

/// Tuning of the [`RenderLoop`].
#[derive(Clone, Debug, PartialEq)]
pub struct RenderConfig {
    /// Time without a newer seek after which a scrub frame refines to full quality (arch §10).
    pub refine_after: StdDuration,
    /// Frames hinted to the source after each playback frame.
    pub prefetch_frames: u32,
    /// Upper bound of the render-time prediction: playback renders the frame due when the
    /// render is expected to finish.
    pub max_lead: StdDuration,
    /// Heartbeat period while no frames are delivered (the ring needs ≥ 10 Hz).
    pub heartbeat: StdDuration,
    /// Longest sleep without a wake-up.
    pub max_idle: StdDuration,
    /// Poll period while 1× playback waits for the audio to become audible after a seek.
    pub prime_poll: StdDuration,
    /// The quality ladder and its playback policy (WP-1F.03).
    pub quality: AutoQualityConfig,
}

impl Default for RenderConfig {
    fn default() -> RenderConfig {
        RenderConfig {
            refine_after: StdDuration::from_millis(100),
            prefetch_frames: 8,
            max_lead: StdDuration::from_millis(100),
            heartbeat: StdDuration::from_millis(50),
            max_idle: StdDuration::from_millis(100),
            prime_poll: StdDuration::from_millis(2),
            quality: AutoQualityConfig::default(),
        }
    }
}

/// What a [`RenderLoop::tick`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tick {
    /// It rendered (or tried to): tick again now.
    Busy,
    /// Nothing to do before host time `wake_at_ns`, unless the transport changes.
    Idle {
        /// When to tick next.
        wake_at_ns: u64,
    },
}

/// Progress on the current request.
#[derive(Clone, Copy, Debug)]
struct Progress {
    epoch: u64,
    /// Playing: the newest delivered frame of this request.
    last_frame: Option<i64>,
    /// Scrubbing: the low-resolution frame is out (or failed).
    lowres_done: bool,
    /// Stopped: the full-quality frame is out (or failed); nothing more to do.
    still_done: bool,
    /// A failure was logged for this request.
    error_logged: bool,
}

impl Progress {
    const fn new(epoch: u64) -> Progress {
        Progress {
            epoch,
            last_frame: None,
            lowres_done: false,
            still_done: false,
            error_logged: false,
        }
    }
}

/// The frame on screen (the newest delivered).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Shown {
    frame: i64,
    content: u64,
    full: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Produce {
    CacheOnly,
    Render,
    CacheThenRender,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Delivered { elapsed_ns: u64 },
    Miss,
    Cancelled,
    Failed,
}

/// The render thread's body (see the module docs).
/// [`PlaybackEngine`](crate::transport::PlaybackEngine) runs it on its own thread; tests and
/// deterministic tools drive it with [`tick`](Self::tick) under a
/// [`VirtualClock`](crate::transport::VirtualClock) and a `ManualClock`.
pub struct RenderLoop {
    shared: Arc<Shared>,
    source: Box<dyn FrameSource>,
    sink: Box<dyn FrameSink>,
    config: RenderConfig,
    auto: AutoQuality,
    /// The frame rate `auto` measures against: the sequence rate times the speed.
    auto_rate: (FrameRate, u32),
    buf: Vec<u8>,
    progress: Progress,
    shown: Option<Shown>,
    /// Smoothed render time (ns) of playback frames: the prediction lead.
    render_ema_ns: u64,
    last_heartbeat_ns: u64,
}

impl std::fmt::Debug for RenderLoop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderLoop")
            .field("config", &self.config)
            .field("quality", &self.auto.quality())
            .field("epoch", &self.progress.epoch)
            .finish_non_exhaustive()
    }
}

fn std_ns(d: StdDuration) -> u64 {
    u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)
}

impl RenderLoop {
    pub(crate) fn new(
        shared: Arc<Shared>,
        source: Box<dyn FrameSource>,
        sink: Box<dyn FrameSink>,
        config: RenderConfig,
    ) -> Result<RenderLoop, SinkError> {
        let full = sink.layout(Scale::Full)?;
        let rate = shared.snapshot.load().timing().rate;
        let auto = AutoQuality::with_config(rate, config.quality.clone());
        let now = shared.host.now_ns();
        Ok(RenderLoop {
            shared,
            source,
            sink,
            config,
            auto,
            auto_rate: (rate, 1),
            buf: vec![0; full.len()],
            progress: Progress::new(u64::MAX),
            shown: None,
            render_ema_ns: 0,
            last_heartbeat_ns: now,
        })
    }

    /// The source.
    #[must_use]
    pub fn source(&self) -> &dyn FrameSource {
        self.source.as_ref()
    }

    /// The sink.
    #[must_use]
    pub fn sink(&self) -> &dyn FrameSink {
        self.sink.as_ref()
    }

    /// The source and the sink back.
    #[must_use]
    pub fn into_parts(self) -> (Box<dyn FrameSource>, Box<dyn FrameSink>) {
        (self.source, self.sink)
    }

    /// The quality the next playback frame renders at.
    #[must_use]
    pub fn playback_quality(&self) -> Quality {
        self.auto.quality()
    }

    fn now(&self) -> u64 {
        self.shared.host.now_ns()
    }

    /// Does the next piece of work: renders and delivers at most one frame. Never blocks.
    pub fn tick(&mut self) -> Tick {
        if let Some((width, height)) = self.shared.take_preview_request() {
            let format = self.sink.preview().format;
            self.reconfigure(PreviewConfig {
                width,
                height,
                format,
            });
        }
        let now = self.now();
        let snap = self.shared.snapshot.load();
        if snap.epoch != self.progress.epoch {
            self.progress = Progress::new(snap.epoch);
        }
        match snap.rate() {
            None => self.tick_stopped(&snap, now),
            Some(rate) => self.tick_playing(&snap, rate, now),
        }
    }

    fn reconfigure(&mut self, preview: PreviewConfig) {
        match self.sink.configure(preview) {
            Ok(()) => {
                if let Ok(full) = self.sink.layout(Scale::Full)
                    && self.buf.len() < full.len()
                {
                    // Allocation only when the preview grows.
                    self.buf.resize(full.len(), 0);
                }
                // Show the current frame at the new size.
                self.shown = None;
                self.progress = Progress::new(self.progress.epoch);
            }
            Err(e) => tracing::warn!(error = %e, "the preview could not be reconfigured"),
        }
    }

    fn idle(&mut self, now: u64, due: u64) -> Tick {
        let beat = std_ns(self.config.heartbeat);
        if now.saturating_sub(self.last_heartbeat_ns) >= beat {
            self.sink.heartbeat();
            self.last_heartbeat_ns = now;
        }
        let wake = due
            .min(now.saturating_add(std_ns(self.config.max_idle)))
            .min(self.last_heartbeat_ns.saturating_add(beat))
            .max(now.saturating_add(MIN_WAIT_NS));
        Tick::Idle { wake_at_ns: wake }
    }

    fn finish_still(&mut self, epoch: u64) {
        self.progress.still_done = true;
        self.shared
            .stats
            .refined_epoch
            .store(epoch, Ordering::Release);
    }

    // ----- stopped and scrubbing ------------------------------------------------------------

    fn tick_stopped(&mut self, snap: &Snapshot, now: u64) -> Tick {
        if self.progress.still_done {
            return self.idle(now, u64::MAX);
        }
        let timing = snap.timing();
        let at = Time::from_flicks(snap.anchor).unwrap_or(Time::ZERO);
        let frame = timing.clamp_frame(timing.frame_at(at));
        let on_screen = Shown {
            frame,
            content: snap.content,
            full: true,
        };
        if self.shown == Some(on_screen) {
            // Paused on a frame playback already showed at full quality, or a seek back to it.
            self.finish_still(snap.epoch);
            return self.idle(now, u64::MAX);
        }
        if !snap.scrub {
            self.auto.set_mode(PlaybackMode::Paused);
            let outcome = self.produce(
                snap,
                frame,
                Quality::FULL,
                RenderPurpose::Still,
                false,
                Produce::CacheThenRender,
            );
            if outcome != Outcome::Cancelled {
                self.finish_still(snap.epoch);
            }
            return Tick::Busy;
        }
        if !self.progress.lowres_done {
            // Latest wins: the frame cache first, a hit is final.
            match self.produce(
                snap,
                frame,
                Quality::FULL,
                RenderPurpose::Still,
                false,
                Produce::CacheOnly,
            ) {
                Outcome::Delivered { .. } => {
                    self.finish_still(snap.epoch);
                    return Tick::Busy;
                }
                Outcome::Cancelled => return Tick::Busy,
                Outcome::Miss | Outcome::Failed => {}
            }
            self.auto.set_mode(PlaybackMode::Scrubbing);
            let quick = self.auto.quality();
            if quick.is_full() {
                let outcome = self.produce(
                    snap,
                    frame,
                    quick,
                    RenderPurpose::Still,
                    false,
                    Produce::Render,
                );
                if outcome != Outcome::Cancelled {
                    self.finish_still(snap.epoch);
                }
                return Tick::Busy;
            }
            if self.produce(
                snap,
                frame,
                quick,
                RenderPurpose::Scrub,
                true,
                Produce::Render,
            ) != Outcome::Cancelled
            {
                self.progress.lowres_done = true;
            }
            return Tick::Busy;
        }
        let due = snap
            .anchor_ns
            .saturating_add(std_ns(self.config.refine_after));
        if now < due {
            return self.idle(now, due);
        }
        self.auto.set_mode(PlaybackMode::Paused);
        if self.produce(
            snap,
            frame,
            Quality::FULL,
            RenderPurpose::Still,
            false,
            Produce::Render,
        ) != Outcome::Cancelled
        {
            self.finish_still(snap.epoch);
        }
        Tick::Busy
    }

    // ----- playing ----------------------------------------------------------------------------

    fn tick_playing(&mut self, snap: &Snapshot, rate: Rate, now: u64) -> Tick {
        let timing = snap.timing();
        self.auto.set_mode(PlaybackMode::Playing);
        let speed = rate.speed();
        if self.auto_rate != (timing.rate, speed) {
            let delivered =
                FrameRate::new(timing.rate.num().saturating_mul(speed), timing.rate.den())
                    .unwrap_or(timing.rate);
            self.auto.set_frame_rate(delivered);
            self.auto_rate = (timing.rate, speed);
        }
        let audio = self.shared.clock.read();
        let pos = snap.position(&audio, now);
        // Predict where the playhead will be when this render is done.
        let lead_ns = self.render_ema_ns.min(std_ns(self.config.max_lead));
        let lead = ns_to_flicks(lead_ns).saturating_mul(i64::from(speed));
        let predicted = if rate.is_reverse() {
            pos.saturating_sub(lead)
        } else {
            pos.saturating_add(lead)
        }
        .clamp(0, snap.end.max(0));
        let predicted_t = Time::from_flicks(predicted).unwrap_or(Time::ZERO);
        // At 2× and 4× every `speed`-th frame from where playback started (one frame per frame
        // period, like the frame rate the preview presents at).
        let stride = i64::from(speed);
        let anchor = timing.frame_at(Time::from_flicks(snap.anchor).unwrap_or(Time::ZERO));
        let frame = timing.frame_at(predicted_t);
        let snapped = if rate.is_reverse() {
            anchor - (anchor - frame).max(0) / stride * stride
        } else {
            anchor + (frame - anchor).max(0) / stride * stride
        };
        let target = timing.clamp_frame(snapped);

        if self.progress.last_frame.is_none()
            && self
                .shown
                .is_some_and(|s| s.frame == target && s.content == snap.content)
        {
            // Playback starts on the frame already on screen (play after pause or a seek).
            self.progress.last_frame = Some(target);
        }
        if self.progress.last_frame == Some(target) {
            let due = self.next_frame_due(snap, rate, &audio, target, predicted, now);
            return self.idle(now, due);
        }
        let quality = self.auto.quality();
        let outcome = self.produce(
            snap,
            target,
            quality,
            RenderPurpose::Playback,
            false,
            Produce::CacheThenRender,
        );
        match outcome {
            Outcome::Delivered { elapsed_ns } => {
                if let Some(last) = self.progress.last_frame {
                    let gap = target.abs_diff(last);
                    if gap > u64::from(speed) {
                        // Grid frames between the two that were never shown.
                        let dropped = gap / u64::from(speed) - 1;
                        self.shared
                            .stats
                            .dropped
                            .fetch_add(dropped, Ordering::Relaxed);
                        self.sink.add_dropped(dropped);
                    }
                }
                self.progress.last_frame = Some(target);
                self.auto.record_frame(StdDuration::from_nanos(elapsed_ns));
                let ema = i128::from(self.render_ema_ns);
                let next = ema + (i128::from(elapsed_ns) - ema) / 4;
                self.render_ema_ns = u64::try_from(next.max(0)).unwrap_or(0);
                let step = i64::from(speed) * if rate.is_reverse() { -1 } else { 1 };
                self.source.prefetch(&PrefetchHint {
                    next_frame: target.saturating_add(step),
                    step,
                    count: self.config.prefetch_frames,
                    quality: self.auto.quality(),
                });
            }
            // Skip a frame that cannot be rendered instead of retrying it in a hot loop.
            Outcome::Failed => self.progress.last_frame = Some(target),
            Outcome::Cancelled | Outcome::Miss => {}
        }
        Tick::Busy
    }

    /// Host time at which the playhead reaches the next frame boundary.
    fn next_frame_due(
        &self,
        snap: &Snapshot,
        rate: Rate,
        audio: &cutweave_audio::ClockReading,
        target: i64,
        predicted: i64,
        now: u64,
    ) -> u64 {
        if rate.follows_audio() && (!audio.is_running() || snap.audio_pending(audio)) {
            return now.saturating_add(std_ns(self.config.prime_poll));
        }
        let timing = snap.timing();
        let stride = i64::from(rate.speed());
        let remaining = if rate.is_reverse() {
            if predicted <= 0 {
                return u64::MAX; // at the start: the status pump stops playback
            }
            predicted - timing.frame_start(target - stride + 1).flicks() + 1
        } else {
            if predicted >= snap.end {
                return u64::MAX; // at the end: the status pump stops playback
            }
            timing.frame_start(target + stride).flicks() - predicted
        };
        now.saturating_add(flicks_to_ns(remaining) / u64::from(rate.speed()))
    }

    // ----- rendering and delivery ---------------------------------------------------------

    fn produce(
        &mut self,
        snap: &Snapshot,
        frame: i64,
        quality: Quality,
        purpose: RenderPurpose,
        lowres: bool,
        how: Produce,
    ) -> Outcome {
        let start = self.now();
        let layout = match self.sink.layout(quality.scale) {
            Ok(l) => l,
            Err(e) => {
                self.failed(frame, &e);
                return Outcome::Failed;
            }
        };
        let len = layout.len();
        if self.buf.len() < len {
            self.buf.resize(len, 0);
        }
        let timing = snap.timing();
        let request = RenderRequest {
            target: snap.target,
            frame,
            t: timing.frame_start(frame),
            quality,
            layout,
            purpose,
        };
        let out = &mut self.buf[..len];
        let cached = match how {
            Produce::CacheOnly | Produce::CacheThenRender => self.source.cached(&request, out),
            Produce::Render => None,
        };
        let rendered = match (cached, how) {
            (Some(r), _) => Ok(r),
            (None, Produce::CacheOnly) => return Outcome::Miss,
            (None, _) => {
                let token = self.shared.gate.token(snap.epoch);
                self.source.render(&request, &token, out)
            }
        };
        let rendered = match rendered {
            Ok(r) => r,
            Err(SourceError::Cancelled) => {
                self.shared.stats.cancelled.fetch_add(1, Ordering::Relaxed);
                return Outcome::Cancelled;
            }
            Err(e) => {
                self.failed(frame, &e);
                return Outcome::Failed;
            }
        };
        // Deliver only if no newer request exists, atomically with respect to new requests.
        let Some(commit) = self.shared.gate.try_commit(snap.epoch) else {
            self.shared.stats.cancelled.fetch_add(1, Ordering::Relaxed);
            return Outcome::Cancelled;
        };
        let info = FrameInfo {
            frame,
            pts: request.t,
            duration: timing.frame_duration(frame),
            rev: rendered.rev,
            quality,
            lowres,
            epoch: snap.epoch,
            render_ns: start,
        };
        let delivered = self.sink.deliver(&info, &layout, &self.buf[..len]);
        drop(commit);
        match delivered {
            Ok(seq) => {
                let stats = &self.shared.stats;
                stats.last_frame.store(frame, Ordering::Relaxed);
                stats.last_rev.store(rendered.rev, Ordering::Relaxed);
                stats
                    .last_quality
                    .store(FrameQuality::of(quality.scale).to_u8(), Ordering::Relaxed);
                stats.last_seq.store(seq, Ordering::Relaxed);
                stats.delivered.fetch_add(1, Ordering::Release);
                self.shown = Some(Shown {
                    frame,
                    content: snap.content,
                    full: quality.is_full(),
                });
                let end = self.now();
                self.last_heartbeat_ns = end;
                Outcome::Delivered {
                    elapsed_ns: end.saturating_sub(start),
                }
            }
            Err(e) => {
                self.failed(frame, &e);
                Outcome::Failed
            }
        }
    }

    fn failed(&mut self, frame: i64, error: &dyn std::fmt::Display) {
        self.shared.stats.errors.fetch_add(1, Ordering::Relaxed);
        if !self.progress.error_logged {
            self.progress.error_logged = true;
            tracing::warn!(%error, frame, "playback could not render a frame");
        }
    }
}
