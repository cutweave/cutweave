// SPDX-License-Identifier: GPL-3.0-or-later
//! Meters (WP-2E.02, arch §9): the master loudness meter of playback (LUFS momentary,
//! short-term and integrated, true peak; EBU R 128 via `ebur128`) and the offline level
//! analysis behind `audio.get_levels` ([`analyze_levels`]).
//!
//! **Real time.** [`LoudnessMeter`] allocates its buffers in [`LoudnessMeter::new`] and runs
//! `ebur128` in histogram mode, whose integrated measure lives in a fixed 1000-bin table, so
//! [`add`](LoudnessMeter::add), [`reading`](LoudnessMeter::reading) and
//! [`reset`](LoudnessMeter::reset) never allocate (the output callback's allocation counter
//! checks it). The integrated loudness covers the audio played since the last seek.
//!
//! **Offline.** [`analyze_levels`] mixes a range exactly as [`mixdown_with`](crate::mixdown_with)
//! does (export, with the limiter as configured) and measures the master output and every
//! track bus (post-fader: a muted track, or one a solo silences, reads silence): sample peak,
//! true peak, RMS and integrated loudness, plus the master's loudest momentary and short-term
//! loudness (sampled every 100 ms) and loudness range.

use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use cutweave_model::{SampleRate, Time, TrackId};
use cutweave_render::AudioSegment;
use ebur128::{EbuR128, Mode};

use crate::bus::{BusGraph, LevelAccumulator};
use crate::dsp::CHANNELS;
use crate::error::MixError;
use crate::limiter::LimiterConfig;
use crate::offline::{MIXDOWN_CHUNK_FRAMES, OfflineGraph, sample_range};
use crate::source::SourceFactory;

/// A loudness reading in LUFS; `-inf` while it cannot be measured (not enough audio yet, or
/// everything below the −70 LUFS gate).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Loudness {
    /// Momentary loudness (the last 400 ms).
    pub momentary: f64,
    /// Short-term loudness (the last 3 s).
    pub short_term: f64,
    /// Integrated (gated) loudness since the meter started or was reset.
    pub integrated: f64,
}

impl Loudness {
    /// Nothing measured.
    pub const NONE: Loudness = Loudness {
        momentary: f64::NEG_INFINITY,
        short_term: f64::NEG_INFINITY,
        integrated: f64::NEG_INFINITY,
    };
}

fn meter_error(e: ebur128::Error) -> MixError {
    MixError::Meter(match e {
        ebur128::Error::NoMem => "the loudness meter is out of memory",
        ebur128::Error::InvalidMode => "the loudness meter was asked for a mode it does not run",
        ebur128::Error::InvalidChannelIndex => "the loudness meter has no such channel",
    })
}

/// The EBU R 128 meter of a stereo signal at one rate, real-time safe after construction (see
/// the module docs).
pub struct LoudnessMeter {
    inner: EbuR128,
    /// The largest true peak (linear, both channels) since [`take_true_peak`](Self::take_true_peak).
    true_peak: f32,
}

impl std::fmt::Debug for LoudnessMeter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoudnessMeter")
            .field("rate", &self.inner.rate())
            .finish_non_exhaustive()
    }
}

impl LoudnessMeter {
    /// A meter of momentary, short-term and integrated loudness and true peak at `rate` (the
    /// sample peak stands in for the true peak at rates `ebur128` cannot oversample).
    pub fn new(rate: SampleRate) -> Result<LoudnessMeter, MixError> {
        let channels = u32::try_from(CHANNELS).unwrap_or(2);
        let loudness = Mode::I | Mode::S | Mode::HISTOGRAM;
        let inner = EbuR128::new(channels, rate.hz(), loudness | Mode::TRUE_PEAK)
            .or_else(|_| EbuR128::new(channels, rate.hz(), loudness | Mode::SAMPLE_PEAK))
            .map_err(meter_error)?;
        Ok(LoudnessMeter {
            inner,
            true_peak: 0.0,
        })
    }

    /// Adds interleaved stereo audio. Allocation-free.
    pub fn add(&mut self, audio: &[f32]) {
        let frames = audio.len() / CHANNELS;
        if frames == 0 {
            return;
        }
        // Cannot fail: two channels, the modes above, whole frames.
        if self
            .inner
            .add_frames_f32(&audio[..frames * CHANNELS])
            .is_ok()
        {
            for c in 0..2 {
                if let Ok(tp) = self
                    .inner
                    .prev_true_peak(c)
                    .or_else(|_| self.inner.prev_sample_peak(c))
                {
                    #[allow(clippy::cast_possible_truncation)]
                    let tp = tp as f32;
                    self.true_peak = self.true_peak.max(tp);
                }
            }
        }
    }

    /// The loudness now. Allocation-free.
    #[must_use]
    pub fn reading(&self) -> Loudness {
        Loudness {
            momentary: self.inner.loudness_momentary().unwrap_or(f64::NEG_INFINITY),
            short_term: self.inner.loudness_shortterm().unwrap_or(f64::NEG_INFINITY),
            integrated: self.inner.loudness_global().unwrap_or(f64::NEG_INFINITY),
        }
    }

    /// The largest true peak (linear, both channels) since the last call, and starts again.
    pub fn take_true_peak(&mut self) -> f32 {
        std::mem::take(&mut self.true_peak)
    }

    /// Forgets everything (after a seek). Allocation-free.
    pub fn reset(&mut self) {
        self.inner.reset();
        self.true_peak = 0.0;
    }
}

/// The levels of one signal over an analysed range.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Levels {
    /// Sample peak, left and right (linear).
    pub peak: [f32; CHANNELS],
    /// True peak, left and right (linear; 4× oversampled below 96 kHz).
    pub true_peak: [f32; CHANNELS],
    /// RMS, left and right (linear: a full-scale sine reads 0.707).
    pub rms: [f32; CHANNELS],
    /// Integrated (gated) loudness in LUFS; `-inf` when everything is below the −70 LUFS gate.
    pub integrated: f64,
    /// The loudest momentary loudness, sampled every 100 ms (master only).
    pub max_momentary: Option<f64>,
    /// The loudest short-term loudness, sampled every 100 ms (master only).
    pub max_short_term: Option<f64>,
    /// Loudness range in LU (master only).
    pub range: Option<f64>,
}

/// The levels of one track bus (post-fader).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackLevels {
    /// The track.
    pub track: TrackId,
    /// Its levels.
    pub levels: Levels,
}

/// What [`analyze_levels`] measured.
#[derive(Clone, Debug, PartialEq)]
pub struct LevelReport {
    /// The analysed output samples `[start, end)`.
    pub samples: Range<i64>,
    /// The rate of the analysis.
    pub rate: SampleRate,
    /// The master output (after the limiter when it is on).
    pub master: Levels,
    /// Every track bus with audio in the range, in mix order.
    pub tracks: Vec<TrackLevels>,
    /// The smallest gain the limiter applied (1.0: it never limited).
    pub limiter_gain: f32,
    /// Clip sources that failed (those clips were silent).
    pub source_errors: u64,
}

/// An offline meter: peak and RMS, `ebur128` loudness and true peak, and the 100 ms maxima.
struct Analyzer {
    acc: LevelAccumulator,
    ebu: EbuR128,
    full: bool,
    /// Frames fed since the last 100 ms boundary, and the frames per 100 ms.
    since_block: usize,
    block: usize,
    max_m: f64,
    max_s: f64,
}

impl Analyzer {
    fn new(rate: SampleRate, full: bool) -> Result<Analyzer, MixError> {
        let mode = if full {
            Mode::I | Mode::S | Mode::LRA | Mode::TRUE_PEAK | Mode::HISTOGRAM
        } else {
            Mode::I | Mode::TRUE_PEAK | Mode::HISTOGRAM
        };
        let channels = u32::try_from(CHANNELS).unwrap_or(2);
        Ok(Analyzer {
            acc: LevelAccumulator::default(),
            ebu: EbuR128::new(channels, rate.hz(), mode).map_err(meter_error)?,
            full,
            since_block: 0,
            block: usize::try_from(rate.hz() / 10).unwrap_or(4800).max(1),
            max_m: f64::NEG_INFINITY,
            max_s: f64::NEG_INFINITY,
        })
    }

    fn add(&mut self, mut audio: &[f32]) -> Result<(), MixError> {
        self.acc.add(audio);
        while !audio.is_empty() {
            let frames = (audio.len() / CHANNELS).min(self.block - self.since_block);
            let (now, rest) = audio.split_at(frames * CHANNELS);
            self.ebu.add_frames_f32(now).map_err(meter_error)?;
            self.since_block += frames;
            if self.since_block == self.block {
                self.since_block = 0;
                if self.full {
                    let m = self.ebu.loudness_momentary().map_err(meter_error)?;
                    let s = self.ebu.loudness_shortterm().map_err(meter_error)?;
                    self.max_m = self.max_m.max(m);
                    self.max_s = self.max_s.max(s);
                }
            }
            audio = rest;
        }
        Ok(())
    }

    fn finish(self) -> Result<Levels, MixError> {
        let tp = |c| self.ebu.true_peak(c).map_err(meter_error);
        #[allow(clippy::cast_possible_truncation)]
        let true_peak = [tp(0)? as f32, tp(1)? as f32];
        Ok(Levels {
            peak: self.acc.peak,
            true_peak,
            rms: self.acc.rms(),
            integrated: self.ebu.loudness_global().map_err(meter_error)?,
            max_momentary: self.full.then_some(self.max_m),
            max_short_term: self.full.then_some(self.max_s),
            range: if self.full {
                Some(self.ebu.loudness_range().map_err(meter_error)?)
            } else {
                None
            },
        })
    }
}

/// Mixes `segments` over `range` at `rate` with the bus settings `buses` and the limiter
/// `limiter` (as an export would) and measures the master and every track bus (see the module
/// docs). Allocates per call; runs on the calling thread.
pub fn analyze_levels(
    sources: &Arc<dyn SourceFactory>,
    segments: &[AudioSegment],
    buses: &BusGraph,
    range: Range<Time>,
    rate: SampleRate,
    limiter: LimiterConfig,
) -> Result<LevelReport, MixError> {
    let (start, end) = sample_range(&range, rate);
    let errors = Arc::new(AtomicU64::new(0));
    let mut graph = OfflineGraph::new(sources, segments, buses, start, rate, limiter, &errors)?;
    let tracks: Vec<TrackId> = graph.tracks().collect();
    let mut master = Analyzer::new(rate, true)?;
    let mut per_bus = tracks
        .iter()
        .map(|_| Analyzer::new(rate, false))
        .collect::<Result<Vec<_>, _>>()?;
    let mut failure: Option<MixError> = None;
    let mut out = vec![0.0f32; MIXDOWN_CHUNK_FRAMES * CHANNELS];
    let mut pos = start;
    while pos < end {
        let frames = usize::try_from(end - pos)
            .unwrap_or(usize::MAX)
            .min(MIXDOWN_CHUNK_FRAMES);
        let block = &mut out[..frames * CHANNELS];
        graph.render(block, &mut |bus, lane_pos, post_fader| {
            // Only the lane samples inside the range (the lanes run ahead of the output).
            let a = usize::try_from(start.saturating_sub(lane_pos)).unwrap_or(0);
            let b = usize::try_from(end.saturating_sub(lane_pos)).unwrap_or(0);
            let n = post_fader.len() / CHANNELS;
            let (a, b) = (a.min(n), b.min(n));
            if a < b
                && let Some(analyzer) = per_bus.get_mut(bus)
                && let Err(e) = analyzer.add(&post_fader[a * CHANNELS..b * CHANNELS])
            {
                failure.get_or_insert(e);
            }
        });
        if let Some(e) = failure.take() {
            return Err(e);
        }
        master.add(block)?;
        pos += i64::try_from(frames).unwrap_or(0);
    }
    let limiter_gain = graph.take_min_gain();
    Ok(LevelReport {
        samples: start..end,
        rate,
        master: master.finish()?,
        tracks: tracks
            .into_iter()
            .zip(per_bus)
            .map(|(track, a)| a.finish().map(|levels| TrackLevels { track, levels }))
            .collect::<Result<_, _>>()?,
        limiter_gain,
        source_errors: errors.load(Ordering::Relaxed),
    })
}

/// `20 log10(x)` in dB, `-inf` for silence.
#[must_use]
pub fn to_db(linear: f64) -> f64 {
    if linear > 0.0 {
        20.0 * linear.log10()
    } else {
        f64::NEG_INFINITY
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation
    )]

    use super::*;

    fn stereo_sine(freq: f64, amplitude: f64, frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|n| {
                let v = (amplitude
                    * (2.0 * std::f64::consts::PI * freq * n as f64 / 48_000.0).sin())
                    as f32;
                [v, v]
            })
            .collect()
    }

    /// EBU Tech 3341 test 1: a stereo 1 kHz sine at −23 dBFS reads −23.0 LUFS (M, S, I).
    #[test]
    fn tech_3341_case_1_reads_minus_23_lufs() {
        let amplitude = 10f64.powf(-23.0 / 20.0);
        let mut m = LoudnessMeter::new(SampleRate::HZ_48_000).unwrap();
        let audio = stereo_sine(1000.0, amplitude, 48_000 * 20);
        for chunk in audio.chunks(2 * 480) {
            m.add(chunk);
        }
        let r = m.reading();
        for (what, v) in [("M", r.momentary), ("S", r.short_term), ("I", r.integrated)] {
            assert!((v + 23.0).abs() <= 0.1, "{what}: {v}");
        }
        let tp = m.take_true_peak();
        assert!((to_db(f64::from(tp)) + 23.0).abs() < 0.1);
        assert_eq!(m.take_true_peak(), 0.0);
        m.reset();
        assert_eq!(m.reading().integrated, f64::NEG_INFINITY);
    }

    #[test]
    fn to_db_of_silence_is_minus_infinity() {
        assert_eq!(to_db(0.0), f64::NEG_INFINITY);
        assert!((to_db(0.1) + 20.0).abs() < 1e-12);
    }
}
