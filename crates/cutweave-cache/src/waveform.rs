// SPDX-License-Identifier: GPL-3.0-or-later
//! Waveform pyramids (WP-1D.06, arch §7): min/max peaks of an audio stream at several
//! resolutions, and the file format they are stored in (`docs/reference/waveform-format.md`).
//!
//! - [`WaveformBuilder`] streams decoded audio in and builds a [`Waveform`]: level 0 holds one
//!   [`Peak`] per [`BASE_SAMPLES_PER_BIN`] (256) samples, every further level merges
//!   [`LEVEL_FACTOR`] (4) bins of the one before (256, 1024, 4096, … samples per bin), so a bin of
//!   level N+1 is *exactly* the min/max of its children.
//! - Tracks: the mono [`Track::Mix`] (the mean of the channels, sample by sample) and, when
//!   requested, one [`Track::Channel`] per channel.
//! - [`Waveform::peaks`] answers `(lod, time range, track)` with a [`PeakData`] (what the
//!   `/media/{id}/waveform` endpoint serves and the timeline draws); [`render_png`] draws one.
//!
//! Peaks are `i16` (`round(sample × 32767)`, clamped), so a −6 dBFS sine reads ±16 384.

// Sample counts and bin indices stay far below 2^52 and every cast to `usize` is of a value bounded
// by an in-memory vector's length, so the casts the pedantic lints worry about cannot lose data.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::chunks_exact_to_as_chunks
)]

use std::ops::Range;

use cutweave_media::{AudioChunk, MediaError};
use cutweave_model::{Rounding, SampleRate, Time};

use crate::CacheError;

/// Samples per bin of level 0.
pub const BASE_SAMPLES_PER_BIN: u32 = 256;
/// How many bins of level N one bin of level N+1 covers.
pub const LEVEL_FACTOR: u32 = 4;
/// The most levels a pyramid has (256 × 4¹¹ = 1 073 741 824 samples per bin at the top).
pub const MAX_LEVELS: u8 = 12;

const MAGIC: &[u8; 4] = b"CWWF";
const FORMAT_VERSION: u8 = 1;
const HEADER_BYTES: usize = 32;
const CHECKSUM_BYTES: usize = 8;
const FLAG_PER_CHANNEL: u8 = 1;

/// The extreme sample values of one bin, as `i16` (`±32767` is full scale). An empty bin never
/// occurs: a bin covers at least one sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Peak {
    /// The lowest sample.
    pub min: i16,
    /// The highest sample.
    pub max: i16,
}

impl Peak {
    fn merge(self, other: Peak) -> Peak {
        Peak {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        }
    }
}

/// Which signal a [`PeakData`] describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Track {
    /// The mono mix: the mean of all channels.
    Mix,
    /// One source channel (0-based). Present only in a per-channel waveform.
    Channel(u16),
}

/// Options for building a pyramid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WaveformOptions {
    /// Also keep a pyramid per source channel (about `channels` times the file size). Mono
    /// sources never need it: their mix is the channel.
    pub per_channel: bool,
}

/// A finished peak pyramid of one audio stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Waveform {
    rate: SampleRate,
    channels: u16,
    samples: u64,
    per_channel: bool,
    /// `[track][level][bin]`; track 0 is the mix, tracks `1..=channels` the channels.
    tracks: Vec<Vec<Vec<Peak>>>,
}

fn quantize(x: f32) -> i16 {
    // NaN saturates to 0; the clamp keeps overs at full scale.
    #[allow(clippy::cast_possible_truncation)]
    {
        (x.clamp(-1.0, 1.0) * 32767.0).round() as i16
    }
}

fn bins_of(samples: u64, per_bin: u64) -> u64 {
    samples.div_ceil(per_bin)
}

/// The number of levels for `samples` samples: until one bin covers everything, at most
/// [`MAX_LEVELS`].
fn level_count(samples: u64) -> u8 {
    let mut levels = 1_u8;
    let mut bins = bins_of(samples, u64::from(BASE_SAMPLES_PER_BIN));
    while bins > 1 && levels < MAX_LEVELS {
        bins = bins.div_ceil(u64::from(LEVEL_FACTOR));
        levels += 1;
    }
    levels
}

fn build_levels(first: Vec<Peak>, count: u8) -> Vec<Vec<Peak>> {
    let mut out = Vec::with_capacity(usize::from(count));
    out.push(first);
    for _ in 1..count {
        let next: Vec<Peak> = out[out.len() - 1]
            .chunks(LEVEL_FACTOR as usize)
            .map(|c| c.iter().copied().reduce(Peak::merge).unwrap_or(c[0]))
            .collect();
        out.push(next);
    }
    out
}

impl Waveform {
    /// Samples per second of the source.
    #[must_use]
    pub fn rate(&self) -> SampleRate {
        self.rate
    }

    /// Source channel count.
    #[must_use]
    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// Samples per channel in the source.
    #[must_use]
    pub fn samples(&self) -> u64 {
        self.samples
    }

    /// Whether [`Track::Channel`] is available.
    #[must_use]
    pub fn has_per_channel(&self) -> bool {
        self.per_channel
    }

    /// Number of levels (LODs `0..levels`).
    #[must_use]
    pub fn levels(&self) -> u8 {
        u8::try_from(self.tracks[0].len()).unwrap_or(u8::MAX)
    }

    /// Samples per bin at `lod`: 256, 1024, 4096, … (`None` past the last level).
    #[must_use]
    pub fn samples_per_bin(&self, lod: u8) -> Option<u64> {
        (lod < self.levels())
            .then(|| u64::from(BASE_SAMPLES_PER_BIN) * u64::from(LEVEL_FACTOR).pow(u32::from(lod)))
    }

    /// The coarsest level whose bins are no wider than `samples_per_pixel` (level 0 when even
    /// that is too coarse): the level to draw at that zoom without losing a peak.
    #[must_use]
    pub fn lod_for(&self, samples_per_pixel: f64) -> u8 {
        let mut best = 0;
        for lod in 0..self.levels() {
            let spb = self.samples_per_bin(lod).unwrap_or(u64::MAX);
            // Bin widths stay far below 2^53.
            #[allow(clippy::cast_precision_loss)]
            if (spb as f64) <= samples_per_pixel {
                best = lod;
            }
        }
        best
    }

    /// The bin count at `lod`.
    #[must_use]
    pub fn bin_count(&self, lod: u8) -> Option<usize> {
        self.tracks[0].get(usize::from(lod)).map(Vec::len)
    }

    fn track_index(&self, track: Track) -> Option<usize> {
        match track {
            Track::Mix => Some(0),
            Track::Channel(_) if self.channels == 1 => Some(0),
            Track::Channel(c) => {
                (self.per_channel && c < self.channels).then(|| usize::from(c) + 1)
            }
        }
    }

    /// The bins of `track` at `lod` that overlap the media-time `range` (clamped to the audio),
    /// or `None` for an unknown level or an unavailable track. A reversed or empty range gives
    /// no bins.
    #[must_use]
    pub fn peaks(&self, lod: u8, range: Range<Time>, track: Track) -> Option<PeakData> {
        let spb = self.samples_per_bin(lod)?;
        let bins = self.tracks[self.track_index(track)?].get(usize::from(lod))?;
        let to_sample =
            |t: Time| u64::try_from(self.rate.time_to_samples(t, Rounding::Floor)).unwrap_or(0);
        let first = (to_sample(range.start) / spb).min(bins.len() as u64);
        let end_sample = to_sample(range.end);
        let last = end_sample.div_ceil(spb).min(bins.len() as u64).max(first);
        let (first_u, last_u) = (first as usize, last as usize);
        Some(PeakData {
            lod,
            samples_per_bin: spb,
            rate: self.rate,
            track,
            first_bin: first,
            peaks: bins[first_u..last_u].to_vec(),
        })
    }

    /// The whole of `track` at `lod`.
    #[must_use]
    pub fn all_peaks(&self, lod: u8, track: Track) -> Option<PeakData> {
        self.peaks(lod, Time::ZERO..Time::MAX, track)
    }

    /// Serializes to the file format (docs/reference/waveform-format.md).
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let levels = self.levels();
        let payload: usize = self
            .tracks
            .iter()
            .map(|t| t.iter().map(|l| l.len() * 4).sum::<usize>())
            .sum();
        let mut out = Vec::with_capacity(HEADER_BYTES + payload + CHECKSUM_BYTES);
        out.extend_from_slice(MAGIC);
        out.push(FORMAT_VERSION);
        out.push(if self.per_channel {
            FLAG_PER_CHANNEL
        } else {
            0
        });
        out.extend_from_slice(&self.channels.to_le_bytes());
        out.extend_from_slice(&self.rate.hz().to_le_bytes());
        out.extend_from_slice(&BASE_SAMPLES_PER_BIN.to_le_bytes());
        out.extend_from_slice(&LEVEL_FACTOR.to_le_bytes());
        out.push(levels);
        out.push(u8::try_from(self.tracks.len()).unwrap_or(u8::MAX));
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&self.samples.to_le_bytes());
        for track in &self.tracks {
            for level in track {
                for p in level {
                    out.extend_from_slice(&p.min.to_le_bytes());
                    out.extend_from_slice(&p.max.to_le_bytes());
                }
            }
        }
        let sum = checksum(&out);
        out.extend_from_slice(&sum.to_le_bytes());
        out
    }

    /// Parses the file format. `None` for anything else: a wrong magic or version, a size that
    /// disagrees with the header, a bad checksum.
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Option<Waveform> {
        if bytes.len() < HEADER_BYTES + CHECKSUM_BYTES || &bytes[..4] != MAGIC {
            return None;
        }
        let (body, tail) = bytes.split_at(bytes.len() - CHECKSUM_BYTES);
        if u64::from_le_bytes(tail.try_into().ok()?) != checksum(body) {
            return None;
        }
        if body[4] != FORMAT_VERSION {
            return None;
        }
        let per_channel = body[5] & FLAG_PER_CHANNEL != 0;
        let channels = u16::from_le_bytes(body[6..8].try_into().ok()?);
        let rate = SampleRate::new(u32::from_le_bytes(body[8..12].try_into().ok()?)).ok()?;
        let base = u32::from_le_bytes(body[12..16].try_into().ok()?);
        let factor = u32::from_le_bytes(body[16..20].try_into().ok()?);
        let levels = body[20];
        let tracks = usize::from(body[21]);
        let samples = u64::from_le_bytes(body[24..32].try_into().ok()?);
        let expected_tracks = if per_channel {
            usize::from(channels) + 1
        } else {
            1
        };
        if base != BASE_SAMPLES_PER_BIN
            || factor != LEVEL_FACTOR
            || channels == 0
            || (per_channel && channels < 2)
            || tracks != expected_tracks
            || levels != level_count(samples)
        {
            return None;
        }
        // Bin counts per level follow from the sample count.
        let mut counts = Vec::with_capacity(usize::from(levels));
        let mut bins = bins_of(samples, u64::from(base));
        for _ in 0..levels {
            counts.push(usize::try_from(bins).ok()?);
            bins = bins.div_ceil(u64::from(factor));
        }
        let per_track: usize = counts.iter().sum();
        if body.len() != HEADER_BYTES + per_track.checked_mul(4)?.checked_mul(tracks)? {
            return None;
        }
        let mut data = body[HEADER_BYTES..].chunks_exact(4);
        let mut out = Vec::with_capacity(tracks);
        for _ in 0..tracks {
            let mut track = Vec::with_capacity(counts.len());
            for &n in &counts {
                let level: Vec<Peak> = data
                    .by_ref()
                    .take(n)
                    .map(|c| Peak {
                        min: i16::from_le_bytes([c[0], c[1]]),
                        max: i16::from_le_bytes([c[2], c[3]]),
                    })
                    .collect();
                track.push(level);
            }
            out.push(track);
        }
        Some(Waveform {
            rate,
            channels,
            samples,
            per_channel,
            tracks: out,
        })
    }
}

/// FNV-1a, 64 bit: detects damage to a cache file (not a security measure).
fn checksum(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// A slice of one track at one level: what a query returns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeakData {
    /// The level (0 is the finest).
    pub lod: u8,
    /// Samples per bin at this level.
    pub samples_per_bin: u64,
    /// The source sample rate.
    pub rate: SampleRate,
    /// The signal.
    pub track: Track,
    /// Index of `peaks[0]` among the bins of the level; its first sample is
    /// `first_bin × samples_per_bin`.
    pub first_bin: u64,
    /// The bins, in time order.
    pub peaks: Vec<Peak>,
}

const PEAKDATA_MAGIC: &[u8; 4] = b"CWPD";
const PEAKDATA_HEADER: usize = 32;

impl PeakData {
    /// Index of the first sample covered (`first_bin × samples_per_bin`).
    #[must_use]
    pub fn first_sample(&self) -> u64 {
        self.first_bin * self.samples_per_bin
    }

    /// Media time of the first sample covered.
    #[must_use]
    pub fn start_time(&self) -> Option<Time> {
        self.rate
            .samples_to_time(i64::try_from(self.first_sample()).ok()?)
            .ok()
    }

    /// The wire form (docs/reference/waveform-format.md, "Peak data"): a 32-byte header then
    /// `min, max` as little-endian `i16` per bin.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(PEAKDATA_HEADER + self.peaks.len() * 4);
        out.extend_from_slice(PEAKDATA_MAGIC);
        out.push(1);
        out.push(self.lod);
        let (kind, channel) = match self.track {
            Track::Mix => (0_u8, 0_u16),
            Track::Channel(c) => (1, c),
        };
        out.push(kind);
        out.push(0);
        out.extend_from_slice(&channel.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&self.rate.hz().to_le_bytes());
        out.extend_from_slice(
            &u32::try_from(self.samples_per_bin)
                .unwrap_or(u32::MAX)
                .to_le_bytes(),
        );
        out.extend_from_slice(&self.first_bin.to_le_bytes());
        out.extend_from_slice(
            &u32::try_from(self.peaks.len())
                .unwrap_or(u32::MAX)
                .to_le_bytes(),
        );
        for p in &self.peaks {
            out.extend_from_slice(&p.min.to_le_bytes());
            out.extend_from_slice(&p.max.to_le_bytes());
        }
        out
    }

    /// Parses [`to_bytes`](Self::to_bytes); `None` for anything else.
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Option<PeakData> {
        if bytes.len() < PEAKDATA_HEADER || &bytes[..4] != PEAKDATA_MAGIC || bytes[4] != 1 {
            return None;
        }
        let channel = u16::from_le_bytes(bytes[8..10].try_into().ok()?);
        let track = match bytes[6] {
            0 => Track::Mix,
            1 => Track::Channel(channel),
            _ => return None,
        };
        let rate = SampleRate::new(u32::from_le_bytes(bytes[12..16].try_into().ok()?)).ok()?;
        let samples_per_bin = u64::from(u32::from_le_bytes(bytes[16..20].try_into().ok()?));
        let first_bin = u64::from_le_bytes(bytes[20..28].try_into().ok()?);
        let count = usize::try_from(u32::from_le_bytes(bytes[28..32].try_into().ok()?)).ok()?;
        let data = &bytes[PEAKDATA_HEADER..];
        if data.len() != count.checked_mul(4)? {
            return None;
        }
        let peaks = data
            .chunks_exact(4)
            .map(|c| Peak {
                min: i16::from_le_bytes([c[0], c[1]]),
                max: i16::from_le_bytes([c[2], c[3]]),
            })
            .collect();
        Some(PeakData {
            lod: bytes[5],
            samples_per_bin,
            rate,
            track,
            first_bin,
            peaks,
        })
    }
}

struct TrackBuf {
    bins: Vec<Peak>,
    min: i16,
    max: i16,
}

impl TrackBuf {
    fn new() -> TrackBuf {
        TrackBuf {
            bins: Vec::new(),
            min: i16::MAX,
            max: i16::MIN,
        }
    }

    fn take(&mut self, min: i16, max: i16) {
        self.min = self.min.min(min);
        self.max = self.max.max(max);
    }

    fn close_bin(&mut self) {
        self.bins.push(Peak {
            min: self.min,
            max: self.max,
        });
        self.min = i16::MAX;
        self.max = i16::MIN;
    }
}

/// Builds a [`Waveform`] from audio fed in time order, in any chunking (the result does not
/// depend on it).
pub struct WaveformBuilder {
    rate: SampleRate,
    channels: u16,
    per_channel: bool,
    tracks: Vec<TrackBuf>,
    /// Samples consumed so far (per channel).
    cursor: u64,
    /// Samples in the open bin.
    fill: u32,
}

impl WaveformBuilder {
    /// A builder for audio of `channels` channels at `rate`.
    #[must_use]
    pub fn new(rate: SampleRate, channels: u16, options: WaveformOptions) -> WaveformBuilder {
        let channels = channels.max(1);
        let per_channel = options.per_channel && channels > 1;
        let tracks = if per_channel {
            usize::from(channels) + 1
        } else {
            1
        };
        WaveformBuilder {
            rate,
            channels,
            per_channel,
            tracks: (0..tracks).map(|_| TrackBuf::new()).collect(),
            cursor: 0,
            fill: 0,
        }
    }

    /// Samples per channel consumed so far.
    #[must_use]
    pub fn samples(&self) -> u64 {
        self.cursor
    }

    /// Feeds `chunk`, placed by its `pts`: samples before the current position are skipped (an
    /// overlap), a gap is filled with silence.
    ///
    /// Errors: [`MediaError::Invalid`] if the chunk's format differs from the builder's or its
    /// planes are inconsistent.
    pub fn push_chunk(&mut self, chunk: &AudioChunk) -> Result<(), CacheError> {
        chunk.validate()?;
        if chunk.rate != self.rate || chunk.channels != self.channels {
            return Err(MediaError::Invalid {
                message: format!(
                    "audio chunk is {} Hz × {}, the waveform is {} Hz × {}",
                    chunk.rate.hz(),
                    chunk.channels,
                    self.rate.hz(),
                    self.channels
                ),
            }
            .into());
        }
        let at =
            u64::try_from(self.rate.time_to_samples(chunk.pts, Rounding::Nearest)).unwrap_or(0);
        if at > self.cursor {
            self.push_silence(at - self.cursor);
        }
        let skip = usize::try_from(self.cursor.saturating_sub(at)).unwrap_or(usize::MAX);
        let planes: Vec<&[f32]> = chunk
            .planes
            .iter()
            .map(|p| p.get(skip..).unwrap_or(&[]))
            .collect();
        self.push_planar(&planes)
    }

    /// Feeds planar samples that follow the previous ones.
    ///
    /// Errors: [`MediaError::Invalid`] for a plane count other than the channel count or planes
    /// of different lengths.
    pub fn push_planar(&mut self, planes: &[&[f32]]) -> Result<(), CacheError> {
        let n = planes.first().map_or(0, |p| p.len());
        if planes.len() != usize::from(self.channels) || planes.iter().any(|p| p.len() != n) {
            return Err(MediaError::Invalid {
                message: "planar audio does not match the waveform's channel count".to_owned(),
            }
            .into());
        }
        let bin = BASE_SAMPLES_PER_BIN as usize;
        let inv = 1.0 / f32::from(self.channels);
        let mut pos = 0;
        while pos < n {
            let take = (bin - self.fill as usize).min(n - pos);
            // The mix: mean of the channels per sample, then quantized.
            let (mut mn, mut mx) = (i16::MAX, i16::MIN);
            for i in pos..pos + take {
                let sum: f32 = planes.iter().map(|p| p[i]).sum();
                let q = quantize(sum * inv);
                mn = mn.min(q);
                mx = mx.max(q);
            }
            self.tracks[0].take(mn, mx);
            if self.per_channel {
                for (c, plane) in planes.iter().enumerate() {
                    let (mut mn, mut mx) = (i16::MAX, i16::MIN);
                    for &x in &plane[pos..pos + take] {
                        let q = quantize(x);
                        mn = mn.min(q);
                        mx = mx.max(q);
                    }
                    self.tracks[c + 1].take(mn, mx);
                }
            }
            pos += take;
            self.fill += u32::try_from(take).unwrap_or(0);
            self.cursor += take as u64;
            if self.fill == BASE_SAMPLES_PER_BIN {
                self.close_bin();
            }
        }
        Ok(())
    }

    fn push_silence(&mut self, mut n: u64) {
        let zeros = vec![0.0_f32; BASE_SAMPLES_PER_BIN as usize];
        let planes: Vec<&[f32]> = (0..self.channels).map(|_| zeros.as_slice()).collect();
        while n > 0 {
            let take = n.min(u64::from(BASE_SAMPLES_PER_BIN)) as usize;
            let cut: Vec<&[f32]> = planes.iter().map(|p| &p[..take]).collect();
            // Cannot fail: the planes match by construction.
            let _ = self.push_planar(&cut);
            n -= take as u64;
        }
    }

    fn close_bin(&mut self) {
        for t in &mut self.tracks {
            t.close_bin();
        }
        self.fill = 0;
    }

    /// Closes the last, partial bin and builds the pyramid.
    #[must_use]
    pub fn finish(mut self) -> Waveform {
        if self.fill > 0 {
            self.close_bin();
        }
        let levels = level_count(self.cursor);
        Waveform {
            rate: self.rate,
            channels: self.channels,
            samples: self.cursor,
            per_channel: self.per_channel,
            tracks: self
                .tracks
                .into_iter()
                .map(|t| build_levels(t.bins, levels))
                .collect(),
        }
    }
}

/// Colours and the zero line of [`render_png`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WaveformStyle {
    /// RGBA of the background (fully transparent by default).
    pub background: [u8; 4],
    /// RGBA of the waveform.
    pub foreground: [u8; 4],
}

impl Default for WaveformStyle {
    fn default() -> Self {
        WaveformStyle {
            background: [0, 0, 0, 0],
            foreground: [255, 255, 255, 255],
        }
    }
}

/// Draws `data` as a `width × height` RGBA PNG: one column per pixel, each a vertical line from
/// the column's highest to its lowest peak (at least one pixel tall), zero at mid height. The
/// bins are spread over the width; with more pixels than bins, bins repeat.
///
/// Errors: [`CacheError::Io`] if `width` or `height` is zero or the PNG cannot be encoded.
pub fn render_png(
    data: &PeakData,
    width: u32,
    height: u32,
    style: &WaveformStyle,
) -> Result<Vec<u8>, CacheError> {
    let fail = |message: String| CacheError::Io {
        path: "<waveform png>".to_owned(),
        message,
    };
    if width == 0 || height == 0 {
        return Err(fail("width and height must be at least 1".to_owned()));
    }
    let (w, h) = (width as usize, height as usize);
    let mut pixels = Vec::with_capacity(w * h * 4);
    for _ in 0..w * h {
        pixels.extend_from_slice(&style.background);
    }
    let n = data.peaks.len();
    let row_of = |v: i16| -> usize {
        // +32767 maps to row 0, -32767 to row h-1.
        let f = (32767.0 - f64::from(v)) / 65534.0;
        // `f` is in [0, 1], so the product is in [0, h-1].
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let row = (f * (h - 1) as f64).round() as usize;
        row.min(h - 1)
    };
    if n > 0 {
        for x in 0..w {
            let from = x * n / w;
            let to = ((x + 1) * n / w).clamp(from + 1, n);
            let peak = data.peaks[from..to]
                .iter()
                .copied()
                .reduce(Peak::merge)
                .unwrap_or(data.peaks[from]);
            let (top, bottom) = (row_of(peak.max), row_of(peak.min));
            for y in top.min(bottom)..=top.max(bottom) {
                let at = (y * w + x) * 4;
                pixels[at..at + 4].copy_from_slice(&style.foreground);
            }
        }
    }
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| fail(e.to_string()))?;
    writer
        .write_image_data(&pixels)
        .map_err(|e| fail(e.to_string()))?;
    writer.finish().map_err(|e| fail(e.to_string()))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const RATE: SampleRate = SampleRate::HZ_48_000;

    fn sine(hz: f64, amp: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let phase = (hz * i as f64 / 48_000.0).fract();
                (f64::from(amp) * (std::f64::consts::TAU * phase).sin()) as f32
            })
            .collect()
    }

    fn build(planes: &[Vec<f32>], per_channel: bool, chunk: usize) -> Waveform {
        let mut b =
            WaveformBuilder::new(RATE, planes.len() as u16, WaveformOptions { per_channel });
        let n = planes[0].len();
        let mut at = 0;
        while at < n {
            let end = (at + chunk).min(n);
            let cut: Vec<&[f32]> = planes.iter().map(|p| &p[at..end]).collect();
            b.push_planar(&cut).expect("push");
            at = end;
        }
        b.finish()
    }

    #[test]
    fn a_minus_6_dbfs_sine_reads_16384_at_every_level() {
        // 1 kHz at 48 kHz: 48 samples per period, so every bin of 256+ samples sees both peaks.
        let wf = build(&[sine(1000.0, 0.5, 48_000 * 20)], false, 4800);
        assert!(wf.levels() >= 6, "levels {}", wf.levels());
        for lod in 0..wf.levels() {
            let all = wf.all_peaks(lod, Track::Mix).expect("level");
            for (i, p) in all.peaks.iter().enumerate() {
                assert!(
                    (i32::from(p.max) - 16384).abs() <= 1,
                    "lod {lod} bin {i}: {p:?}"
                );
                assert!(
                    (i32::from(p.min) + 16384).abs() <= 1,
                    "lod {lod} bin {i}: {p:?}"
                );
            }
        }
    }

    #[test]
    fn levels_are_256_1024_4096_and_end_at_one_bin() {
        let wf = build(&[vec![0.25; 256 * 4 * 4 * 3 + 10]], false, 1000);
        assert_eq!(wf.samples_per_bin(0), Some(256));
        assert_eq!(wf.samples_per_bin(1), Some(1024));
        assert_eq!(wf.samples_per_bin(2), Some(4096));
        assert_eq!(wf.samples_per_bin(wf.levels()), None);
        assert_eq!(wf.bin_count(wf.levels() - 1), Some(1));
        assert_eq!(wf.bin_count(0), Some(49));
        let empty = build(&[vec![]], false, 10);
        assert_eq!((empty.levels(), empty.bin_count(0)), (1, Some(0)));
    }

    #[test]
    fn quantization_clamps_and_rounds() {
        let wf = build(&[vec![2.0, -3.0, 0.0, f32::NAN]], false, 4);
        let p = wf.all_peaks(0, Track::Mix).expect("lod 0").peaks[0];
        assert_eq!((p.min, p.max), (-32767, 32767));
    }

    #[test]
    fn mix_is_the_mean_and_channels_are_kept_when_asked() {
        let l = vec![1.0_f32; 512];
        let r = vec![-0.5_f32; 512];
        let wf = build(&[l.clone(), r.clone()], true, 100);
        assert!(wf.has_per_channel());
        let mix = wf.all_peaks(0, Track::Mix).expect("mix").peaks;
        assert_eq!(
            mix,
            vec![
                Peak {
                    min: 8192,
                    max: 8192
                };
                2
            ]
        );
        let left = wf.all_peaks(0, Track::Channel(0)).expect("left").peaks;
        let right = wf.all_peaks(0, Track::Channel(1)).expect("right").peaks;
        assert_eq!(
            left[0],
            Peak {
                min: 32767,
                max: 32767
            }
        );
        assert_eq!(
            right[0],
            Peak {
                min: -16384,
                max: -16384
            }
        );
        assert!(wf.all_peaks(0, Track::Channel(2)).is_none());
        let mono_only = build(&[l, r], false, 100);
        assert!(!mono_only.has_per_channel());
        assert!(mono_only.all_peaks(0, Track::Channel(0)).is_none());
        // A mono source's only channel is its mix.
        let mono = build(&[vec![0.5; 300]], true, 100);
        assert_eq!(mono.all_peaks(0, Track::Channel(0)), {
            let mut m = mono.all_peaks(0, Track::Mix).expect("mix");
            m.track = Track::Channel(0);
            Some(m)
        });
    }

    #[test]
    fn range_queries_return_the_overlapping_bins() {
        // 10 bins of 256 samples, each with a distinct value.
        let samples: Vec<f32> = (0..10).flat_map(|b| vec![b as f32 / 10.0; 256]).collect();
        let wf = build(&[samples], false, 777);
        let at = |n: i64| RATE.samples_to_time(n).expect("time");
        // Samples 300..700 touch bins 1 and 2.
        let d = wf.peaks(0, at(300)..at(700), Track::Mix).expect("peaks");
        assert_eq!((d.first_bin, d.peaks.len()), (1, 2));
        assert_eq!(d.first_sample(), 256);
        assert_eq!(d.start_time(), Some(at(256)));
        // Past the end clamps, a reversed range is empty.
        let d = wf
            .peaks(0, at(2000)..at(1_000_000), Track::Mix)
            .expect("peaks");
        assert_eq!((d.first_bin, d.peaks.len()), (7, 3));
        let d = wf.peaks(0, at(700)..at(300), Track::Mix).expect("peaks");
        assert_eq!(d.peaks, []);
        let d = wf
            .peaks(0, at(5_000_000)..at(6_000_000), Track::Mix)
            .expect("peaks");
        assert_eq!(d.peaks, []);
        assert!(wf.peaks(40, Time::ZERO..Time::MAX, Track::Mix).is_none());
        // Negative times clamp to the start.
        let d = wf
            .peaks(0, Time::from_secs(-5).expect("t")..at(256), Track::Mix)
            .expect("peaks");
        assert_eq!((d.first_bin, d.peaks.len()), (0, 1));
    }

    #[test]
    fn lod_for_picks_the_coarsest_level_that_loses_nothing() {
        let wf = build(&[vec![0.0; 256 * 64]], false, 4096);
        assert_eq!(wf.lod_for(10.0), 0);
        assert_eq!(wf.lod_for(256.0), 0);
        assert_eq!(wf.lod_for(1100.0), 1);
        assert_eq!(wf.lod_for(1e12), wf.levels() - 1);
    }

    #[test]
    fn chunks_are_placed_by_pts_with_gaps_as_silence_and_overlaps_skipped() {
        let mut b = WaveformBuilder::new(RATE, 1, WaveformOptions::default());
        let chunk = |first: i64, v: f32, n: usize| AudioChunk {
            pts: RATE.samples_to_time(first).expect("t"),
            rate: RATE,
            channels: 1,
            planes: vec![vec![v; n]],
        };
        b.push_chunk(&chunk(256, 0.5, 256)).expect("late start"); // 256 silent samples first
        b.push_chunk(&chunk(400, 1.0, 400)).expect("overlap"); // 112 samples overlap, 288 new
        let wf = b.finish();
        assert_eq!(wf.samples(), 800);
        let p = wf.all_peaks(0, Track::Mix).expect("lod 0").peaks;
        assert_eq!(p[0], Peak { min: 0, max: 0 });
        assert_eq!(
            p[1],
            Peak {
                min: 16384,
                max: 16384
            }
        );
        assert_eq!(
            p[2],
            Peak {
                min: 32767,
                max: 32767
            }
        );
        assert_eq!(p[3].max, 32767);
        let mut b = WaveformBuilder::new(RATE, 2, WaveformOptions::default());
        assert!(
            b.push_chunk(&chunk(0, 0.0, 10)).is_err(),
            "wrong channel count"
        );
    }

    #[test]
    fn the_file_round_trips_and_damage_is_rejected() {
        let wf = build(
            &[sine(440.0, 0.8, 30_000), sine(660.0, 0.3, 30_000)],
            true,
            999,
        );
        let bytes = wf.to_bytes();
        assert_eq!(Waveform::from_bytes(&bytes), Some(wf.clone()));
        assert_eq!(&bytes[..4], b"CWWF");
        for cut in [0, 10, bytes.len() - 1] {
            assert_eq!(
                Waveform::from_bytes(&bytes[..cut]),
                None,
                "truncated at {cut}"
            );
        }
        let mut bad = bytes.clone();
        bad[HEADER_BYTES + 3] ^= 0x40;
        assert_eq!(Waveform::from_bytes(&bad), None, "payload flip");
        let mut bad = bytes;
        bad[4] = 9;
        assert_eq!(Waveform::from_bytes(&bad), None, "unknown version");
        assert_eq!(Waveform::from_bytes(b""), None);
    }

    #[test]
    fn peak_data_round_trips() {
        let wf = build(&[sine(440.0, 0.8, 5000), sine(660.0, 0.3, 5000)], true, 999);
        for track in [Track::Mix, Track::Channel(1)] {
            let d = wf.all_peaks(1, track).expect("peaks");
            assert_eq!(PeakData::from_bytes(&d.to_bytes()), Some(d));
        }
        assert_eq!(PeakData::from_bytes(b"CWPD"), None);
    }

    #[test]
    fn png_has_the_requested_size_and_draws_the_peaks() {
        let wf = build(&[sine(1000.0, 0.5, 48_000)], false, 4800);
        let data = wf.all_peaks(0, Track::Mix).expect("peaks");
        let bytes = render_png(&data, 100, 40, &WaveformStyle::default()).expect("png");
        let mut reader = png::Decoder::new(std::io::Cursor::new(bytes.as_slice()))
            .read_info()
            .expect("png header");
        let mut buf = vec![0; reader.output_buffer_size().expect("size")];
        let info = reader.next_frame(&mut buf).expect("frame");
        assert_eq!((info.width, info.height), (100, 40));
        let alpha = |x: usize, y: usize| buf[(y * 100 + x) * 4 + 3];
        // Half amplitude: lit between about rows 10 and 30, dark at the top and bottom edge.
        assert_eq!(alpha(50, 20), 255);
        assert_eq!(alpha(50, 0), 0);
        assert_eq!(alpha(50, 39), 0);
        assert!(render_png(&data, 0, 10, &WaveformStyle::default()).is_err());
        // No data: an empty picture, not an error.
        let empty = PeakData {
            peaks: vec![],
            ..data
        };
        assert!(render_png(&empty, 8, 8, &WaveformStyle::default()).is_ok());
    }

    proptest! {
        #[test]
        fn each_level_is_the_exact_min_max_of_the_one_below(
            samples in proptest::collection::vec(-1.2_f32..1.2, 0..20_000),
            chunk in 1_usize..5000,
            second in any::<bool>(),
        ) {
            let planes = if second {
                vec![samples.clone(), samples.iter().rev().copied().collect()]
            } else {
                vec![samples.clone()]
            };
            let wf = build(&planes, true, chunk);
            // Chunking never changes the result.
            prop_assert_eq!(&wf, &build(&planes, true, planes[0].len().max(1)));
            // Level 0 against a brute-force pass.
            let channels = planes.len();
            let mix: Vec<i16> = (0..samples.len())
                .map(|i| quantize(planes.iter().map(|p| p[i]).sum::<f32>() / channels as f32))
                .collect();
            let l0 = wf.all_peaks(0, Track::Mix).expect("lod 0").peaks;
            let brute: Vec<Peak> = mix
                .chunks(256)
                .map(|c| Peak { min: *c.iter().min().expect("bin"), max: *c.iter().max().expect("bin") })
                .collect();
            prop_assert_eq!(&l0, &brute);
            for track in 0..=channels {
                let t = if track == 0 { Track::Mix } else if channels == 1 { break } else { Track::Channel(track as u16 - 1) };
                for lod in 1..wf.levels() {
                    let below = wf.all_peaks(lod - 1, t).expect("below").peaks;
                    let this = wf.all_peaks(lod, t).expect("this").peaks;
                    let expect: Vec<Peak> = below
                        .chunks(4)
                        .map(|c| Peak {
                            min: c.iter().map(|p| p.min).min().expect("child"),
                            max: c.iter().map(|p| p.max).max().expect("child"),
                        })
                        .collect();
                    prop_assert_eq!(this, expect);
                }
            }
            prop_assert_eq!(Waveform::from_bytes(&wf.to_bytes()), Some(wf));
        }
    }
}
