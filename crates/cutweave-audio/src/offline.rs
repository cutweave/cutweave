// SPDX-License-Identifier: GPL-3.0-or-later
//! Offline mixdown (export, analysis, tests): the same graph as the real-time mixer, driven
//! synchronously.
//!
//! Lanes come from the same [`LaneRenderer`] the workers run, buses from the same
//! [`mix_bus`](crate::bus) stage and the master from the same gain and [`Limiter`], so a
//! real-time render with no underruns equals the mixdown of the same range.
//!
//! **Latency compensation.** The limiter delays the master by [`Limiter::latency`] frames, so
//! the lanes run that far ahead of the output: before its first chunk a mixdown feeds the
//! limiter the first `latency` frames of the range (and drops what comes out, the silence the
//! limiter starts with), exactly as the real-time mixer does after a seek. Output sample `n`
//! is therefore timeline sample `n`.

use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use cutweave_model::{Rounding, SampleRate, Time, TimeError, TrackId};
use cutweave_render::AudioSegment;

use crate::bus::{BusGraph, MasterBus, StereoRamp, mix_bus};
use crate::dsp::{CHANNELS, Ramp};
use crate::error::MixError;
use crate::lane::LaneRenderer;
use crate::limiter::{Limiter, LimiterConfig};
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
    track: TrackId,
    gains: StereoRamp,
    lane: LaneRenderer,
}

/// The lanes, buses and master of an offline mix, and where the lanes are.
pub(crate) struct OfflineGraph {
    buses: Vec<OfflineBus>,
    master: MasterBus,
    /// Output sample of the lanes' next frame (`latency` ahead of the output once primed).
    lane_pos: i64,
    primed: bool,
    scratch: Vec<f32>,
}

impl OfflineGraph {
    /// The graph of `segments` with the bus settings of `buses`, its lanes starting at output
    /// sample `start`.
    pub fn new(
        sources: &Arc<dyn SourceFactory>,
        segments: &[AudioSegment],
        buses: &BusGraph,
        start: i64,
        rate: SampleRate,
        limiter: LimiterConfig,
        errors: &Arc<AtomicU64>,
    ) -> Result<OfflineGraph, MixError> {
        let buses = plan_buses(segments, rate, buses)?
            .into_iter()
            .map(|bus| OfflineBus {
                track: bus.track,
                gains: StereoRamp::at(bus.gains),
                lane: LaneRenderer::new(bus.clips, sources.clone(), rate, start, errors.clone()),
            })
            .collect();
        Ok(OfflineGraph {
            buses,
            master: MasterBus {
                gain: Ramp::at(1.0),
                limiter: Limiter::new(limiter, rate)?,
            },
            lane_pos: start,
            primed: false,
            scratch: vec![0.0; MIXDOWN_CHUNK_FRAMES * CHANNELS],
        })
    }

    /// The tracks of the buses, in mix order.
    pub fn tracks(&self) -> impl Iterator<Item = TrackId> + '_ {
        self.buses.iter().map(|b| b.track)
    }

    /// Frames the master delays (the lanes run this far ahead).
    pub fn latency(&self) -> usize {
        self.master.latency()
    }

    /// The limiter's smallest gain since the last call.
    pub fn take_min_gain(&mut self) -> f32 {
        self.master.limiter.take_min_gain()
    }

    /// Mixes the lanes' next `out.len() / 2` frames into `out` (overwritten, before the master)
    /// and advances them. `tap(bus, lane_pos, post_fader)` sees every bus's post-fader signal
    /// of the block, which starts at lane sample `lane_pos`.
    fn mix_lanes(&mut self, out: &mut [f32], tap: &mut dyn FnMut(usize, i64, &[f32])) {
        out.fill(0.0);
        for chunk in out.chunks_mut(MIXDOWN_CHUNK_FRAMES * CHANNELS) {
            let lane = &mut self.scratch[..chunk.len()];
            for (i, bus) in self.buses.iter_mut().enumerate() {
                bus.lane.render(lane);
                mix_bus(&mut bus.gains, lane, chunk);
                tap(i, self.lane_pos, lane);
            }
            self.lane_pos += i64::try_from(chunk.len() / CHANNELS).unwrap_or(0);
        }
    }

    /// Writes the next `out.len() / 2` output frames into `out` (after the master): the first
    /// call first primes the limiter with `latency` frames of the lanes. `tap` as in
    /// [`mix_lanes`](Self::mix_lanes), for every lane block (the priming one included).
    pub fn render(&mut self, out: &mut [f32], tap: &mut dyn FnMut(usize, i64, &[f32])) {
        if !self.primed {
            self.primed = true;
            let mut left = self.latency();
            let mut pre = vec![0.0; MIXDOWN_CHUNK_FRAMES * CHANNELS];
            while left > 0 {
                let n = left.min(MIXDOWN_CHUNK_FRAMES);
                let block = &mut pre[..n * CHANNELS];
                self.mix_lanes(block, tap);
                self.master.process(block);
                left -= n;
            }
        }
        self.mix_lanes(out, tap);
        self.master.process(out);
    }
}

/// The iterator returned by [`mixdown`]: [`MIXDOWN_CHUNK_FRAMES`]-frame chunks (the last one
/// shorter) covering the range.
pub struct Mixdown {
    rate: SampleRate,
    graph: OfflineGraph,
    pos: i64,
    end: i64,
    errors: Arc<AtomicU64>,
}

impl std::fmt::Debug for Mixdown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mixdown")
            .field("rate", &self.rate)
            .field("buses", &self.graph.buses.len())
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

    /// Frames the master limiter delays (compensated: output sample `n` is timeline sample `n`).
    #[must_use]
    pub fn latency(&self) -> usize {
        self.graph.latency()
    }
}

/// `range` as output samples `[time_to_samples(start), time_to_samples(end))`, nearest sample.
pub(crate) fn sample_range(range: &Range<Time>, rate: SampleRate) -> (i64, i64) {
    let start = rate.time_to_samples(range.start, Rounding::Nearest);
    let end = rate
        .time_to_samples(range.end, Rounding::Nearest)
        .max(start);
    (start, end)
}

/// Mixes `segments` (the output of `evaluate_audio`) over the timeline `range` at `rate`,
/// lazily, chunk by chunk, with every track at its segments' track gain and the default master
/// limiter (on, −1 dBTP). Master gain is unity (the master volume is a monitoring setting).
/// `range` maps to output samples `[time_to_samples(start), time_to_samples(end))`, nearest
/// sample. [`mixdown_with`] takes the bus settings and the limiter.
pub fn mixdown(
    sources: &Arc<dyn SourceFactory>,
    segments: &[AudioSegment],
    range: Range<Time>,
    rate: SampleRate,
) -> Result<Mixdown, MixError> {
    mixdown_with(
        sources,
        segments,
        &BusGraph::from_segments(segments),
        range,
        rate,
        LimiterConfig::default(),
    )
}

/// [`mixdown`] with the track buses of `buses` (gain, pan, mute and solo; tracks it does not
/// know play at their segments' track gain) and the master limiter `limiter`.
pub fn mixdown_with(
    sources: &Arc<dyn SourceFactory>,
    segments: &[AudioSegment],
    buses: &BusGraph,
    range: Range<Time>,
    rate: SampleRate,
    limiter: LimiterConfig,
) -> Result<Mixdown, MixError> {
    let (start, end) = sample_range(&range, rate);
    let errors = Arc::new(AtomicU64::new(0));
    let graph = OfflineGraph::new(sources, segments, buses, start, rate, limiter, &errors)?;
    Ok(Mixdown {
        rate,
        graph,
        pos: start,
        end,
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
        self.graph.render(&mut samples, &mut |_, _, _| {});
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
