// SPDX-License-Identifier: GPL-3.0-or-later
//! Offline mixdown (export, tests): the same graph as the real-time mixer, driven synchronously.
//!
//! Lanes come from the same [`LaneRenderer`] the workers run, and buses and the master go
//! through the same [`Ramp`] / [`master`] code as [`Mixer::render`](crate::Mixer::render), so a
//! real-time render with no underruns equals the mixdown of the same range.

use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use cutweave_model::{Rounding, SampleRate, Time, TimeError};
use cutweave_render::AudioSegment;

use crate::dsp::{CHANNELS, Ramp, master};
use crate::error::MixError;
use crate::lane::LaneRenderer;
use crate::plan::plan_buses;
use crate::source::SourceFactory;

/// Frames per [`AudioChunk`] of a [`Mixdown`].
pub const MIXDOWN_CHUNK_FRAMES: usize = 4096;

/// A run of mixed output: interleaved stereo `f32` at `rate`.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioChunk {
    /// Output sample (timeline position at `rate`) of the first frame.
    pub start: i64,
    /// The sample rate.
    pub rate: SampleRate,
    /// Interleaved L, R samples.
    pub samples: Vec<f32>,
}

impl AudioChunk {
    /// Frames in the chunk.
    #[must_use]
    pub fn frames(&self) -> usize {
        self.samples.len() / CHANNELS
    }

    /// Timeline time of the first frame.
    pub fn time(&self) -> Result<Time, TimeError> {
        self.rate.samples_to_time(self.start)
    }
}

struct OfflineBus {
    gain: Ramp,
    lane: LaneRenderer,
}

/// The iterator returned by [`mixdown`]: [`MIXDOWN_CHUNK_FRAMES`]-frame chunks (the last one
/// shorter) covering the range.
pub struct Mixdown {
    rate: SampleRate,
    buses: Vec<OfflineBus>,
    master: Ramp,
    pos: i64,
    end: i64,
    scratch: Vec<f32>,
    errors: Arc<AtomicU64>,
}

impl std::fmt::Debug for Mixdown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mixdown")
            .field("rate", &self.rate)
            .field("buses", &self.buses.len())
            .field("pos", &self.pos)
            .field("end", &self.end)
            .finish_non_exhaustive()
    }
}

impl Mixdown {
    /// Output samples `[start, end)` the mixdown covers.
    #[must_use]
    pub fn samples(&self) -> Range<i64> {
        self.pos..self.end
    }

    /// Clip sources that failed so far (those clips are silent in the mix).
    #[must_use]
    pub fn source_errors(&self) -> u64 {
        self.errors.load(Ordering::Relaxed)
    }
}

/// Mixes `segments` (the output of `evaluate_audio`) over the timeline `range` at `rate`,
/// lazily, chunk by chunk. Master gain is unity (the master volume is a monitoring setting);
/// the soft limiter applies. `range` maps to output samples
/// `[time_to_samples(start), time_to_samples(end))`, nearest sample.
pub fn mixdown(
    sources: &Arc<dyn SourceFactory>,
    segments: &[AudioSegment],
    range: Range<Time>,
    rate: SampleRate,
) -> Result<Mixdown, MixError> {
    let start = rate.time_to_samples(range.start, Rounding::Nearest);
    let end = rate
        .time_to_samples(range.end, Rounding::Nearest)
        .max(start);
    let errors = Arc::new(AtomicU64::new(0));
    let buses = plan_buses(segments, rate)?
        .into_iter()
        .map(|bus| OfflineBus {
            gain: Ramp::at(bus.gain),
            lane: LaneRenderer::new(bus.clips, sources.clone(), rate, start, errors.clone()),
        })
        .collect();
    Ok(Mixdown {
        rate,
        buses,
        master: Ramp::at(1.0),
        pos: start,
        end,
        scratch: vec![0.0; MIXDOWN_CHUNK_FRAMES * CHANNELS],
        errors,
    })
}

impl Iterator for Mixdown {
    type Item = AudioChunk;

    fn next(&mut self) -> Option<AudioChunk> {
        let left = usize::try_from(self.end - self.pos).unwrap_or(0);
        let frames = left.min(MIXDOWN_CHUNK_FRAMES);
        if frames == 0 {
            return None;
        }
        let mut samples = vec![0.0; frames * CHANNELS];
        let lane = &mut self.scratch[..frames * CHANNELS];
        for bus in &mut self.buses {
            bus.lane.render(lane);
            bus.gain.add_scaled(&mut samples, lane);
        }
        master(&mut samples, &mut self.master);
        let chunk = AudioChunk {
            start: self.pos,
            rate: self.rate,
            samples,
        };
        self.pos += i64::try_from(frames).unwrap_or(0);
        Some(chunk)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let left = usize::try_from(self.end - self.pos).unwrap_or(0);
        let n = left.div_ceil(MIXDOWN_CHUNK_FRAMES);
        (n, Some(n))
    }
}

impl ExactSizeIterator for Mixdown {}
