// SPDX-License-Identifier: GPL-3.0-or-later
//! The [`FrameSource`] seam: "render the sequence frame at a time, at a quality, cancellably".
//!
//! The render loop asks a source for one output frame at a time ([`RenderRequest`]: a frame of
//! the sequence grid, a rung of the quality ladder and the output [`FrameLayout`] the sink
//! wants) and hands it a reusable buffer to fill. Until FFmpeg decode lands (R31), tests and
//! demos use [`SyntheticSource`](crate::transport::SyntheticSource).
//!
//! # How the real renderer plugs in (WP-1I.05)
//!
//! The engine implements `FrameSource` over the render core and the decoder pool:
//!
//! 1. `render`: take the document snapshot (`DocHandle::snapshot`, lock-free), run
//!    `cutweave_render::evaluate(snapshot, sequence, request.t)`, apply `request.quality`
//!    (`Quality::apply` marks layers proxy-eligible), build the render graph for the target
//!    size `request.layout.width() x height()`, submit it to the `gpu::Compositor`, poll the
//!    readback (checking `cancel` between steps), convert to the layout's pixel format and copy
//!    into `out`. The decoded source frames come from the media layer through
//!    `cutweave_render::FrameSource` (decoder pool + frame cache, WP-1D.03); a cancelled token
//!    should also abandon pending decodes. Return the document `rev` that was rendered.
//! 2. `cached`: look the frame up in the preview frame cache (`cutweave-cache`, keyed by the
//!    graph's cache key and `t`); never decode or render there.
//! 3. `prefetch`: evaluate the hinted times and queue decodes of the active clips' next frames
//!    in the decoder pool (arch §7), so playback finds them decoded.
//!
//! The render thread must not allocate per frame (arch §10 "Threads"). The engine's source keeps
//! its sequence index, scene, render graph and GPU objects across frames, and copies the mapped
//! readback straight into `out`; only wgpu and the media layer (decoding a picture it does not
//! hold yet) allocate inside a render. Its `preview_alloc` test counts both per scope.
//!
//! # Sources (ADR-0022, WP-1H.04)
//!
//! A request whose [`target`](RenderRequest::target) is [`PlaybackTarget::Media`] renders that
//! asset on its own: the engine indexes a transient one-clip timeline built from the asset's
//! probe (once per target and document revision, like the sequence) and fits the picture into
//! the output with the asset's aspect. Nothing of it is stored in the document.

use cutweave_model::{Duration, FrameRate, Rounding, Time};
use cutweave_render::Quality;
use thiserror::Error;

use crate::shm::FrameLayout;
use crate::transport::cancel::CancelToken;
use crate::transport::target::PlaybackTarget;

/// The frame grid and extent of the sequence being played.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SequenceTiming {
    /// The sequence frame rate: frames are picked on its grid (floor).
    pub rate: FrameRate,
    /// End of the content (exclusive): forward playback stops here. Zero when empty.
    pub end: Time,
}

impl SequenceTiming {
    /// Timing at `rate` with content up to `end` (negative ends count as empty).
    #[must_use]
    pub fn new(rate: FrameRate, end: Time) -> SequenceTiming {
        SequenceTiming {
            rate,
            end: end.max(Time::ZERO),
        }
    }

    /// The frame that contains `t` (floor to the frame grid).
    #[must_use]
    pub fn frame_at(&self, t: Time) -> i64 {
        self.rate.time_to_frame(t, Rounding::Floor)
    }

    /// The start of `frame`.
    #[must_use]
    pub fn frame_start(&self, frame: i64) -> Time {
        self.rate
            .frame_to_time(frame)
            .unwrap_or(if frame < 0 { Time::MIN } else { Time::MAX })
    }

    /// The duration of `frame` (frames of non-integer-flick rates differ by one flick).
    #[must_use]
    pub fn frame_duration(&self, frame: i64) -> Duration {
        let d = self
            .frame_start(frame.saturating_add(1))
            .flicks()
            .saturating_sub(self.frame_start(frame).flicks());
        Duration::from_flicks(d.max(0)).unwrap_or(Duration::ZERO)
    }

    /// The last frame with content (0 for an empty sequence).
    #[must_use]
    pub fn last_frame(&self) -> i64 {
        if self.end <= Time::ZERO {
            return 0;
        }
        let last = Time::from_flicks(self.end.flicks() - 1).unwrap_or(Time::ZERO);
        self.frame_at(last).max(0)
    }

    /// `frame` limited to `0..=last_frame()`.
    #[must_use]
    pub fn clamp_frame(&self, frame: i64) -> i64 {
        frame.clamp(0, self.last_frame())
    }

    /// `t` limited to `0..=end`.
    #[must_use]
    pub fn clamp_time(&self, t: Time) -> Time {
        t.clamp(Time::ZERO, self.end)
    }
}

/// Why a frame is rendered (decoders and caches may treat them differently).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RenderPurpose {
    /// A frame of continuous playback (the next ones follow; keep the decoder streaming).
    Playback,
    /// The immediate low-resolution answer to a seek (random access; speed over quality).
    Scrub,
    /// A still at full quality: the refine after a scrub, a pause, a step or a refresh.
    Still,
}

/// One output frame to render.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderRequest {
    /// What to render: the sequence, or one media asset on its own (ADR-0022). `frame` and `t`
    /// are on the target's own grid and in its own time.
    pub target: PlaybackTarget,
    /// Frame index on the target's grid.
    pub frame: i64,
    /// Its start time (the frame's presentation timestamp).
    pub t: Time,
    /// The quality rung.
    pub quality: Quality,
    /// Output geometry and pixel format (the preview canvas reduced by `quality.scale`): fill
    /// exactly `layout.len()` bytes, plane `i` at `layout.planes()[i].offset`.
    pub layout: FrameLayout,
    /// Why it is rendered.
    pub purpose: RenderPurpose,
}

/// What a source rendered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rendered {
    /// The document revision the frame shows.
    pub rev: u64,
}

/// A render that produced no frame.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum SourceError {
    /// The [`CancelToken`] was cancelled (a newer request exists); not an error.
    #[error("cancelled by a newer request")]
    Cancelled,
    /// Rendering failed (no secrets in the message).
    #[error("render failed: {0}")]
    Failed(String),
}

/// What playback is likely to need next, so the decoder pool can start early.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrefetchHint {
    /// The next frame expected.
    pub next_frame: i64,
    /// Frames between expected frames (negative in reverse).
    pub step: i64,
    /// How many frames ahead to prepare, starting at `next_frame`.
    pub count: u32,
    /// The quality they will be rendered at.
    pub quality: Quality,
}

impl PrefetchHint {
    /// The hinted frames, in the order they are expected.
    pub fn frames(&self) -> impl Iterator<Item = i64> + '_ {
        (0..i64::from(self.count)).map(|i| self.next_frame.saturating_add(i * self.step))
    }
}

/// Renders sequence frames for the transport (see the module docs for the WP-1I.05 renderer).
///
/// Called only from the render thread. Implementations must be deterministic (the same request
/// on the same document gives the same pixels, arch §8), must not block indefinitely, and
/// should poll the [`CancelToken`] between units of work.
pub trait FrameSource: Send {
    /// The sequence's frame grid and extent when playback attaches (later changes reach the
    /// transport through [`Transport::set_timing`](crate::transport::Transport::set_timing)).
    fn timing(&self) -> SequenceTiming;

    /// Renders `request` into `out` (exactly `request.layout.len()` bytes).
    fn render(
        &mut self,
        request: &RenderRequest,
        cancel: &CancelToken,
        out: &mut [u8],
    ) -> Result<Rendered, SourceError>;

    /// Fills `out` from the frame cache if `request` (or a better quality of it, scaled to the
    /// layout) is cached. Must not decode or render. The scrub scheduler tries this first.
    fn cached(&mut self, request: &RenderRequest, out: &mut [u8]) -> Option<Rendered> {
        let _ = (request, out);
        None
    }

    /// A hint of the frames playback will ask for next (after every playback frame).
    fn prefetch(&mut self, hint: &PrefetchHint) {
        let _ = hint;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn timing_maps_times_to_the_frame_grid() {
        let t = SequenceTiming::new(FrameRate::FPS_30, Time::from_secs(10).unwrap());
        assert_eq!(t.last_frame(), 299);
        assert_eq!(t.frame_at(Time::from_flicks(23_520_000 - 1).unwrap()), 0);
        assert_eq!(t.frame_at(Time::from_flicks(23_520_000).unwrap()), 1);
        assert_eq!(t.frame_start(2).flicks(), 2 * 23_520_000);
        assert_eq!(t.frame_duration(5).flicks(), 23_520_000);
        assert_eq!(t.clamp_frame(400), 299);
        assert_eq!(t.clamp_frame(-3), 0);
        assert_eq!(
            t.clamp_time(Time::from_secs(11).unwrap()),
            Time::from_secs(10).unwrap()
        );
        let empty = SequenceTiming::new(FrameRate::FPS_30, Time::from_secs(-1).unwrap());
        assert_eq!(empty.end, Time::ZERO);
        assert_eq!(empty.last_frame(), 0);
        // 29.97: the last frame of 10 s of content.
        let ntsc = SequenceTiming::new(FrameRate::FPS_29_97, Time::from_secs(10).unwrap());
        assert_eq!(ntsc.last_frame(), 299);
        let d = ntsc.frame_duration(0).flicks();
        assert!((23_543_519..=23_543_520).contains(&d), "{d}");
    }

    #[test]
    fn prefetch_hints_list_their_frames() {
        let h = PrefetchHint {
            next_frame: 10,
            step: -2,
            count: 3,
            quality: Quality::FULL,
        };
        assert_eq!(h.frames().collect::<Vec<_>>(), vec![10, 8, 6]);
    }
}
