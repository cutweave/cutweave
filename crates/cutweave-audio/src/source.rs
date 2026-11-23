// SPDX-License-Identifier: GPL-3.0-or-later
//! The source seam of the mixer (R29): clips pull interleaved `f32` PCM through [`PcmSource`]s
//! that a [`SourceFactory`] opens per clip.
//!
//! The mixer never decodes anything itself. Until the FFmpeg backend lands, sources are
//! generated ([`ToneFactory`](crate::ToneFactory)); decoded clip audio plugs in through an
//! adapter over the media backend's `AudioDecoder` (WP-1D.01 / WP-1D.03).
//!
//! # Contract
//!
//! * A source has a fixed native [`PcmFormat`]: any sample rate the model accepts and any
//!   channel count ≥ 1. The mixer down-mixes to stereo (mono is copied to both sides, 5.1 uses
//!   the ITU-R BS.775 coefficients, other layouts keep their first two channels) and resamples
//!   to the device rate with `rubato` on the worker thread, so sources never convert rates.
//! * Frame `n` of a source is media time `format().rate.samples_to_time(n)`: frame 0 is media
//!   time 0, not the start of the requested range.
//! * Sources run on the mixer's worker threads, never on the real-time thread, so they may
//!   allocate and do I/O. They must be deterministic: the same frames for the same position.
//!
//! # Adapter plan for the real decoder (WP-1D.01 / WP-1D.03)
//!
//! `cutweave-audio` will depend on `cutweave-media` for its `AudioDecoder` trait only (WBS 1G.01)
//! and implement `SourceFactory` over it, so `FakeBackend` and the FFmpeg backend both work:
//!
//! * `open(request)`: resolve `request.media` to the asset's path through the media index and open
//!   an `AudioDecoder` on `request.stream` (first audio stream when `None`). The decoder converts
//!   the sample *format* to interleaved `f32` (swresample format conversion only); it keeps the
//!   stream's native rate and channel layout, which `format()` reports. `request.start..end` is a
//!   read-ahead hint (the clip's source range): the decoder may stop decoding past it.
//! * `seek(frame)`: convert the frame to the stream timebase exactly through flicks
//!   (`rate.samples_to_time`, arch §4.1), seek to the packet at or before it, then decode and
//!   discard up to the exact frame (sample-accurate seek; the media index maps VFR/priming).
//! * `read(out)`: hand out decoded frames, carrying a partial decoded frame over to the next call;
//!   return `Ok(0)` at end of stream and `SourceError::Decode` on a hard decode error.
//!
//! Nothing else changes: the lane renderer, resampler and real-time graph are source-agnostic.

use cutweave_model::{MediaId, SampleRate, Time};

use crate::error::SourceError;

/// The native PCM layout of a source: interleaved `f32`, `channels` samples per frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PcmFormat {
    /// Frames per second.
    pub rate: SampleRate,
    /// Samples per frame (≥ 1).
    pub channels: u16,
}

impl PcmFormat {
    /// `channels` at `rate`.
    #[must_use]
    pub const fn new(rate: SampleRate, channels: u16) -> PcmFormat {
        PcmFormat { rate, channels }
    }
}

/// What a clip asks a [`SourceFactory`] to open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceRequest {
    /// The media to read.
    pub media: MediaId,
    /// Index of the container audio stream (`None`: the first audio stream).
    pub stream: Option<u32>,
    /// Start of the source range the clip plays (media time).
    pub start: Time,
    /// End of that range (exclusive). A hint: reads may continue past it.
    pub end: Time,
}

/// A pull source of interleaved `f32` PCM in its native format.
///
/// Called only from the mixer's worker threads (or the offline mixdown), never from the
/// real-time thread.
pub trait PcmSource: Send {
    /// The native format. Constant for the life of the source.
    fn format(&self) -> PcmFormat;

    /// Positions the source so the next [`read`](Self::read) starts at frame `frame` (frame 0 is
    /// media time 0). Seeking past the end is allowed; reads then return `Ok(0)`.
    fn seek(&mut self, frame: u64) -> Result<(), SourceError>;

    /// Fills `out` (a whole number of frames) with the next frames and returns how many frames it
    /// wrote. Short reads are allowed; `Ok(0)` means end of stream.
    fn read(&mut self, out: &mut [f32]) -> Result<usize, SourceError>;
}

/// Opens a [`PcmSource`] per clip. Shared by all worker threads.
pub trait SourceFactory: Send + Sync {
    /// Opens `request.media`. The caller seeks before the first read.
    fn open(&self, request: &SourceRequest) -> Result<Box<dyn PcmSource>, SourceError>;
}
