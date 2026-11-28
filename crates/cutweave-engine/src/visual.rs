// SPDX-License-Identifier: GPL-3.0-or-later
//! The engine behind `media.filmstrip` and `audio.waveform` (WP-2I.02; arch §14):
//! [`EngineVisual`] implements `cutweave-ops`' [`VisualService`] over the media caches the
//! `/media` HTTP routes use ([`MediaState`]) and the export's audio sources.
//!
//! - **Filmstrips** come from the `ThumbCache`: the whole clip's strip is the entry the
//!   timeline and `GET /media/{id}/filmstrip` share (so the result can name that route as its
//!   `link`); a part of a clip is `ThumbCache::filmstrip_range`. Either is decoded on a miss.
//! - **Waveforms of a media file** are read from its peak pyramid (`WaveformCache`, computed
//!   here if the analysis job has not written it yet): the level whose bins are no wider than
//!   a pixel column for the picture, level 0 (256 samples a bin) for the silences.
//! - **Waveforms of a clip or a mix** are mixed like an export (`cutweave_audio::mixdown_with`
//!   over the export's sources; the master limiter on for a mix, off for one clip) while the
//!   call waits, and measured as they pass: columns, 256-sample blocks for the silences, and
//!   on request an EBU R 128 meter read at even steps.
//!
//! The pictures are drawn by `cutweave_render::wave` and encoded as PNG.

use std::sync::Arc;

use cutweave_audio::{BusGraph, CHANNELS, LimiterConfig, LoudnessMeter, mixdown_with};
use cutweave_cache::{
    BASE_SAMPLES_PER_BIN, CacheError, FilmstripSpec, PeakData, Track, Waveform, WaveformRequest,
};
use cutweave_commands::CmdError;
use cutweave_media::CancelToken;
use cutweave_model::{
    ItemId, MediaId, Project, Rounding, SampleRate, SequenceId, StreamKind, Time,
};
use cutweave_ops::visual::{
    LoudnessMade, StripMade, StripRequest, VisualService, WaveLane, WaveMade, WaveRequest,
    WaveSource,
};
use cutweave_render::encode::{base64, encode_png};
use cutweave_render::wave::{self, Column, WaveError};

use crate::export::ExportSources;
use crate::full::{MediaFile, MediaState};

/// Most lanes of a split waveform (channels past this are left out).
const MAX_LANES: u16 = 8;
/// Most blocks the silences of a media file are found on; a longer file uses a coarser level.
const MAX_ENVELOPE_BLOCKS: usize = 4_000_000;
/// Most short-term loudness readings of one waveform.
const MAX_LOUDNESS_POINTS: i64 = 240;

/// `media.filmstrip` and `audio.waveform` over the engine's caches and decoders.
pub struct EngineVisual {
    state: MediaState,
    sources: Option<Arc<dyn ExportSources>>,
}

impl std::fmt::Debug for EngineVisual {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineVisual")
            .field("state", &self.state)
            .field("sources", &self.sources.is_some())
            .finish()
    }
}

impl EngineVisual {
    /// A maker over `state`'s caches, mixing clips with `sources` (the export's).
    #[must_use]
    pub fn new(state: MediaState, sources: Option<Arc<dyn ExportSources>>) -> EngineVisual {
        EngineVisual { state, sources }
    }
}

fn cache_failure(path: &str, e: CacheError) -> CmdError {
    use cutweave_media::MediaError as M;
    match e {
        CacheError::InvalidParams(message) => CmdError::invalid(path, message),
        CacheError::Media(M::NotFound { .. }) => CmdError::invalid(
            path,
            format!("the media file is offline: {e}; media.relink finds it again"),
        ),
        CacheError::Media(M::NoStream { .. }) => CmdError::invalid(path, e.to_string()),
        other => CmdError::internal(format!("cannot read the media: {other}")),
    }
}

/// The file of asset `id` in `doc`.
fn media_file(doc: &Project, id: MediaId, path: &str) -> Result<MediaFile, CmdError> {
    let asset = doc
        .media
        .get(&id)
        .ok_or_else(|| CmdError::not_found(cutweave_model::EntityKind::Media, id))?;
    let file = crate::export::sources::export_path(asset)
        .ok_or_else(|| CmdError::invalid(path, "the asset has no file"))?;
    Ok(MediaFile {
        id,
        path: file.into(),
        hash: asset.content_hash.as_deref().and_then(|h| h.parse().ok()),
        probe: asset.probe.clone(),
    })
}

fn png(image: &cutweave_render::Image) -> Result<(String, u32), CmdError> {
    let mut bytes = Vec::new();
    encode_png(image, &mut bytes)
        .map_err(|e| CmdError::internal(format!("image encoding failed: {e}")))?;
    Ok((
        base64(&bytes),
        u32::try_from(bytes.len()).unwrap_or(u32::MAX),
    ))
}

/// Draws `lanes` (named by `names`) for `request` and encodes the picture.
fn picture(
    lanes: &[Vec<Column>],
    names: &[String],
    request: &WaveRequest,
) -> Result<(String, u32, Vec<WaveLane>), CmdError> {
    let (image, layout) = wave::draw(
        lanes,
        (request.from, request.to),
        (request.width, request.height),
        request.scale_db,
    )
    .map_err(|e| match e {
        WaveError::TooLow { .. } => CmdError::invalid("/height", e.to_string()),
        WaveError::Lanes => CmdError::internal(e.to_string()),
    })?;
    let (data, bytes) = png(&image)?;
    let lanes = names
        .iter()
        .zip(&layout.lanes)
        .map(|(name, rect)| WaveLane {
            channel: name.clone(),
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
        })
        .collect();
    Ok((data, bytes, lanes))
}

/// The sample range `[first, end)` of `from..to` at `rate`, at least one sample long.
fn sample_range(from: Time, to: Time, rate: SampleRate) -> (i64, i64) {
    let first = rate.time_to_samples(from, Rounding::Nearest).max(0);
    let end = rate.time_to_samples(to, Rounding::Nearest).max(first + 1);
    (first, end)
}

/// The columns of `peaks` for the samples `[first, end)` spread over `width` pixels: each
/// column the extremes of the bins that overlap its samples; silence past the last bin.
fn columns_of(peaks: &PeakData, (first, end): (i64, i64), width: u32) -> Vec<Column> {
    let bin = i128::from(peaks.samples_per_bin.max(1));
    let total = i128::from(end - first);
    let origin = i128::from(peaks.first_bin);
    let at = |x: u32| i128::from(first) + total * i128::from(x) / i128::from(width.max(1));
    (0..width)
        .map(|x| {
            let (a, b) = (at(x), at(x + 1).max(at(x) + 1));
            let lo = usize::try_from(a / bin - origin).unwrap_or(0);
            let hi = usize::try_from((b + bin - 1) / bin - origin)
                .unwrap_or(0)
                .min(peaks.peaks.len());
            let mut column = Column::EMPTY;
            for peak in peaks.peaks.get(lo..hi).unwrap_or(&[]) {
                column.add(f32::from(peak.min) / 32767.0);
                column.add(f32::from(peak.max) / 32767.0);
            }
            column.or_silent()
        })
        .collect()
}

/// The finest level of `waveform` with at most [`MAX_ENVELOPE_BLOCKS`] bins in `from..to`, as
/// the largest magnitude of each bin.
fn envelope_of(waveform: &Waveform, from: Time, to: Time) -> Option<(Vec<f32>, Time, u32)> {
    for lod in 0..waveform.levels() {
        let peaks = waveform.peaks(lod, from..to, Track::Mix)?;
        if peaks.peaks.len() > MAX_ENVELOPE_BLOCKS && lod + 1 < waveform.levels() {
            continue;
        }
        let envelope = peaks
            .peaks
            .iter()
            .map(|p| f32::from(p.min.unsigned_abs().max(p.max.unsigned_abs())) / 32767.0)
            .collect();
        let block = u32::try_from(peaks.samples_per_bin).unwrap_or(BASE_SAMPLES_PER_BIN);
        return Some((envelope, peaks.start_time().unwrap_or(from), block));
    }
    None
}

impl EngineVisual {
    fn media_wave(
        &self,
        doc: &Project,
        request: &WaveRequest,
        media: MediaId,
        stream: Option<u32>,
    ) -> Result<WaveMade, CmdError> {
        let path = "/target/media";
        let waveforms = self
            .state
            .waveforms
            .clone()
            .ok_or_else(|| CmdError::unsupported("the waveform cache is unavailable"))?;
        let file = media_file(doc, media, path)?;
        let hash = MediaState::media_ref(&file)
            .map_err(|e| cache_failure(path, e))?
            .hash;
        let channels = file
            .probe
            .streams
            .iter()
            .find(|s| s.kind == StreamKind::Audio && stream.is_none_or(|index| s.index == index))
            .and_then(|s| s.channels)
            .unwrap_or(1);
        let split = request.split && channels > 1;
        let ask = WaveformRequest {
            stream,
            per_channel: split,
            duration_hint: file.probe.duration,
        };
        let waveform = waveforms
            .ensure(
                &*self.state.backend,
                &file.path,
                &hash,
                &ask,
                &CancelToken::new(),
                &mut |_| {},
            )
            .map_err(|e| cache_failure(path, e))?;
        let rate = waveform.rate();
        let samples = sample_range(request.from, request.to, rate);
        // Sample counts stay far below 2^53.
        #[allow(clippy::cast_precision_loss)]
        let per_pixel = (samples.1 - samples.0) as f64 / f64::from(request.width);
        let lod = waveform.lod_for(per_pixel);
        let tracks: Vec<(Track, String)> = if split {
            (0..waveform.channels().min(MAX_LANES))
                .map(|c| (Track::Channel(c), format!("ch{}", c + 1)))
                .collect()
        } else {
            vec![(Track::Mix, "mix".to_owned())]
        };
        let mut lanes = Vec::with_capacity(tracks.len());
        for (track, _) in &tracks {
            let peaks = waveform
                .peaks(lod, request.from..request.to, *track)
                .ok_or_else(|| CmdError::internal("the waveform has no such track"))?;
            lanes.push(columns_of(&peaks, samples, request.width));
        }
        let names: Vec<String> = tracks.into_iter().map(|(_, name)| name).collect();
        let (data, bytes, lanes) = picture(&lanes, &names, request)?;
        let (envelope, envelope_start, block_samples) =
            envelope_of(&waveform, request.from, request.to)
                .ok_or_else(|| CmdError::internal("the waveform has no levels"))?;
        // The route draws the same peaks (mix only) as a plain PNG; its ETag is the content
        // hash and a hash of these parameters.
        let link = (file.hash.is_some() && !split).then(|| {
            let stream = stream.map_or_else(String::new, |s| format!("&stream={s}"));
            format!(
                "/media/{media}/waveform?fmt=png&w={}&h={}&lod={lod}&start={}&end={}{stream}&hash={hash}",
                request.width,
                request.height,
                request.from.flicks(),
                request.to.flicks(),
            )
        });
        Ok(WaveMade {
            data,
            bytes,
            lanes,
            sample_rate: rate.hz(),
            envelope,
            envelope_start,
            block_samples,
            loudness: None,
            source_errors: 0,
            link,
        })
    }

    fn mix_wave(
        &self,
        doc: &Project,
        request: &WaveRequest,
        sequence: SequenceId,
        item: Option<ItemId>,
    ) -> Result<WaveMade, CmdError> {
        let sources = self.sources.as_ref().ok_or_else(|| {
            CmdError::unsupported("this engine has no decoders to mix a clip or a sequence with")
        })?;
        let seq = doc
            .sequences
            .get(&sequence)
            .ok_or_else(|| CmdError::invalid("/target", "the sequence is gone"))?;
        let rate = seq.format.sample_rate;
        let (mut segments, mut buses) =
            crate::levels::mix_of(doc, sequence, request.from, request.to)
                .map_err(|e| CmdError::internal(format!("cannot evaluate the audio: {e}")))?;
        let mut limiter = LimiterConfig::default();
        if let Some(item) = item {
            // One clip as it plays: no other clip, no mute or solo, no master limiter.
            segments.retain(|s| s.item == item);
            buses = BusGraph::from_segments(&segments);
            limiter.enabled = false;
        }
        let mut mix = mixdown_with(
            &sources.audio(doc),
            &segments,
            &buses,
            request.from..request.to,
            rate,
            limiter,
        )
        .map_err(|e| CmdError::internal(format!("cannot mix the audio: {e}")))?;
        let mut meter = Measure::new(request, rate, mix.samples())?;
        for chunk in mix.by_ref() {
            meter.add(&chunk.samples);
        }
        let source_errors = u32::try_from(mix.source_errors()).unwrap_or(u32::MAX);
        let (lanes, names) = meter.lanes();
        let (data, bytes, lanes) = picture(&lanes, &names, request)?;
        Ok(WaveMade {
            data,
            bytes,
            lanes,
            sample_rate: rate.hz(),
            envelope: meter.envelope,
            envelope_start: rate.samples_to_time(meter.first).unwrap_or(request.from),
            block_samples: BASE_SAMPLES_PER_BIN,
            loudness: meter.loudness.map(|l| LoudnessMade {
                integrated: l.meter.reading().integrated,
                short_term: l.points,
            }),
            source_errors,
            link: None,
        })
    }
}

/// The loudness of a mix as it passes: an EBU R 128 meter read every `step` frames.
struct Loudness {
    meter: LoudnessMeter,
    step: i64,
    points: Vec<(Time, f64)>,
}

/// Measures a stereo mix as its chunks pass: the picture's columns, the 256-sample envelope
/// and, on request, the loudness.
struct Measure {
    rate: SampleRate,
    /// Output sample of the first frame.
    first: i64,
    /// Frames in the range.
    total: i64,
    /// Frames seen so far.
    seen: i64,
    width: u32,
    split: bool,
    /// The mix lane, or left and right.
    columns: [Vec<Column>; 2],
    envelope: Vec<f32>,
    loudness: Option<Loudness>,
}

impl Measure {
    fn new(
        request: &WaveRequest,
        rate: SampleRate,
        samples: std::ops::Range<i64>,
    ) -> Result<Measure, CmdError> {
        let total = (samples.end - samples.start).max(1);
        let width = request.width as usize;
        let loudness = if request.loudness {
            // A reading every 100 ms, or less often so a long range stays a short list.
            let tenth = i64::from(rate.hz() / 10).max(1);
            let steps = (total / tenth / MAX_LOUDNESS_POINTS).max(1);
            Some(Loudness {
                meter: LoudnessMeter::new(rate)
                    .map_err(|e| CmdError::internal(format!("cannot measure loudness: {e}")))?,
                step: tenth * steps,
                points: Vec::new(),
            })
        } else {
            None
        };
        Ok(Measure {
            rate,
            first: samples.start,
            total,
            seen: 0,
            width: request.width,
            split: request.split,
            columns: [vec![Column::EMPTY; width], vec![Column::EMPTY; width]],
            envelope: Vec::new(),
            loudness,
        })
    }

    /// Adds interleaved stereo frames.
    fn add(&mut self, samples: &[f32]) {
        let block = i64::from(BASE_SAMPLES_PER_BIN);
        for (i, frame) in (0i64..).zip(samples.as_chunks::<CHANNELS>().0) {
            let at = self.seen + i;
            let (left, right) = (frame[0], frame[1]);
            let x = usize::try_from(at * i64::from(self.width) / self.total)
                .unwrap_or(0)
                .min(self.width as usize - 1);
            if self.split {
                self.columns[0][x].add(left);
                self.columns[1][x].add(right);
            } else {
                self.columns[0][x].add(f32::midpoint(left, right));
            }
            let slot = usize::try_from(at / block).unwrap_or(0);
            if self.envelope.len() <= slot {
                self.envelope.resize(slot + 1, 0.0);
            }
            self.envelope[slot] = self.envelope[slot].max(left.abs()).max(right.abs());
        }
        let frames = i64::try_from(samples.len() / CHANNELS).unwrap_or(0);
        if let Some(loudness) = &mut self.loudness {
            // Feed the meter up to each reading's frame, so readings do not depend on how the
            // mix is cut into chunks.
            let mut done = 0;
            while done < frames {
                let at = self.seen + done;
                let next = (at / loudness.step + 1) * loudness.step;
                let take = (next - at).min(frames - done);
                let (a, b) = (
                    usize::try_from(done).unwrap_or(0) * CHANNELS,
                    usize::try_from(done + take).unwrap_or(0) * CHANNELS,
                );
                loudness.meter.add(&samples[a..b]);
                done += take;
                if at + take == next
                    && let Ok(t) = self.rate.samples_to_time(self.first + next)
                {
                    loudness
                        .points
                        .push((t, loudness.meter.reading().short_term));
                }
            }
        }
        self.seen += frames;
    }

    /// The lanes drawn and their names.
    fn lanes(&self) -> (Vec<Vec<Column>>, Vec<String>) {
        if self.split {
            (
                self.columns.to_vec(),
                vec!["left".to_owned(), "right".to_owned()],
            )
        } else {
            (vec![self.columns[0].clone()], vec!["mix".to_owned()])
        }
    }
}

impl VisualService for EngineVisual {
    fn filmstrip(&self, doc: &Project, request: &StripRequest) -> Result<StripMade, CmdError> {
        let path = "/media";
        let thumbs = self.state.thumbs.clone().ok_or_else(|| {
            CmdError::unsupported("the thumbnail cache is unavailable (no usable cache directory)")
        })?;
        let file = media_file(doc, request.media, path)?;
        let reference = MediaState::media_ref(&file).map_err(|e| cache_failure(path, e))?;
        let spec = FilmstripSpec {
            count: request.count,
            tile_w: request.tile_width,
        };
        let token = CancelToken::new();
        let strip = match request.range {
            None => thumbs.filmstrip(&reference, spec, &token),
            Some((from, to)) => thumbs.filmstrip_range(&reference, spec, from..to, &token),
        }
        .map_err(|e| cache_failure(path, e))?;
        let manifest = strip.manifest;
        if manifest.columns != manifest.count {
            return Err(CmdError::internal(
                "the filmstrip does not fit one row of the sprite",
            ));
        }
        // The whole clip's strip is the entry `GET /media/{id}/filmstrip` serves.
        let link = (request.range.is_none() && file.hash.is_some()).then(|| {
            format!(
                "/media/{}/filmstrip?w={}&count={}&hash={}",
                request.media, spec.tile_w, spec.count, reference.hash
            )
        });
        Ok(StripMade {
            bytes: u32::try_from(strip.jpeg.len()).unwrap_or(u32::MAX),
            data: base64(&strip.jpeg),
            tile_width: manifest.tile_w,
            tile_height: manifest.tile_h,
            start: manifest.start,
            interval: manifest.interval,
            etag: strip.etag,
            link,
        })
    }

    fn waveform(&self, doc: &Project, request: &WaveRequest) -> Result<WaveMade, CmdError> {
        match request.source {
            WaveSource::Media { media, stream } => self.media_wave(doc, request, media, stream),
            WaveSource::Item { sequence, item } => {
                self.mix_wave(doc, request, sequence, Some(item))
            }
            WaveSource::Mix { sequence } => self.mix_wave(doc, request, sequence, None),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use cutweave_cache::{WaveformBuilder, WaveformOptions};
    use cutweave_model::FLICKS_PER_SECOND;

    use super::*;

    fn secs(s: f64) -> Time {
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        Time::from_flicks((s * FLICKS_PER_SECOND as f64).round() as i64).unwrap()
    }

    /// `seconds` of a 440 Hz sine at -6 dBFS, silent where `silent` says so.
    fn pyramid(seconds: usize, silent: impl Fn(f64) -> bool) -> Waveform {
        let rate = SampleRate::HZ_48_000;
        let mut builder = WaveformBuilder::new(rate, 1, WaveformOptions::default());
        #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
        let samples: Vec<f32> = (0..seconds * 48_000)
            .map(|n| {
                let t = n as f64 / 48_000.0;
                if silent(t) {
                    0.0
                } else {
                    (0.5 * (std::f64::consts::TAU * 440.0 * t).sin()) as f32
                }
            })
            .collect();
        for chunk in samples.chunks(4800) {
            builder.push_planar(&[chunk]).unwrap();
        }
        builder.finish()
    }

    #[test]
    fn columns_follow_the_pyramid_and_end_in_silence() {
        let waveform = pyramid(4, |_| false);
        let rate = waveform.rate();
        let range = sample_range(secs(0.0), secs(4.0), rate);
        assert_eq!(range, (0, 192_000));
        let lod = waveform.lod_for(192_000.0 / 400.0);
        assert_eq!(lod, 0, "480 samples a pixel: 256-sample bins");
        let peaks = waveform
            .peaks(lod, secs(0.0)..secs(4.0), Track::Mix)
            .unwrap();
        let columns = columns_of(&peaks, range, 400);
        assert_eq!(columns.len(), 400);
        for c in &columns {
            assert!((c.max - 0.5).abs() < 0.002 && (c.min + 0.5).abs() < 0.002);
        }
        // A range that starts inside the file and runs past its end: sound, then silence.
        let range = sample_range(secs(3.0), secs(5.0), rate);
        let peaks = waveform
            .peaks(lod, secs(3.0)..secs(5.0), Track::Mix)
            .unwrap();
        let columns = columns_of(&peaks, range, 100);
        assert!((columns[10].max - 0.5).abs() < 0.002);
        assert_eq!(columns[80], Column::default());
    }

    /// WP-2I.02 acceptance, on the peak pyramid: a 2 s silence in a file is found within 50 ms.
    #[test]
    fn the_envelope_of_a_pyramid_finds_a_two_second_silence() {
        let waveform = pyramid(8, |t| (3.0..5.0).contains(&t));
        let (envelope, start, block) = envelope_of(&waveform, secs(0.0), secs(8.0)).unwrap();
        assert_eq!((start, block), (Time::ZERO, 256));
        assert_eq!(envelope.len(), 1500);
        let found = cutweave_ops::visual::silences(
            &envelope,
            start,
            block,
            waveform.rate().hz(),
            (secs(0.0), secs(8.0)),
        );
        assert_eq!(found.len(), 1);
        let fifty_ms = FLICKS_PER_SECOND / 20;
        assert!((found[0].from.flicks() - secs(3.0).flicks()).abs() <= fifty_ms);
        assert!((found[0].to.flicks() - secs(5.0).flicks()).abs() <= fifty_ms);
        // Asked for a part of the file, the envelope starts at the bin that holds its start.
        let (part, start, _) = envelope_of(&waveform, secs(4.0), secs(6.0)).unwrap();
        assert_eq!(start, secs(4.0));
        assert_eq!(part.len(), 375);
    }

    #[test]
    fn a_mix_is_measured_whatever_its_chunks() {
        let request = WaveRequest {
            source: WaveSource::Mix {
                sequence: SequenceId::from_u128(1),
            },
            from: secs(1.0),
            to: secs(5.0),
            width: 200,
            height: 100,
            split: true,
            scale_db: None,
            loudness: true,
        };
        let rate = SampleRate::HZ_48_000;
        // Left: a -6 dBFS sine. Right: silence.
        #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
        let stereo: Vec<f32> = (0..4 * 48_000)
            .flat_map(|n| {
                let t = f64::from(n) / 48_000.0;
                [
                    (0.5 * (std::f64::consts::TAU * 440.0 * t).sin()) as f32,
                    0.0,
                ]
            })
            .collect();
        let measure = |chunk: usize| {
            let mut m = Measure::new(&request, rate, 48_000..5 * 48_000).unwrap();
            for part in stereo.chunks(chunk * CHANNELS) {
                m.add(part);
            }
            m
        };
        let (a, b) = (measure(4096), measure(1000));
        assert_eq!(a.columns, b.columns);
        assert_eq!(a.envelope, b.envelope);
        let points = |m: &Measure| m.loudness.as_ref().unwrap().points.clone();
        assert_eq!(points(&a).len(), 40, "every 100 ms of 4 s");
        assert_eq!(points(&a), points(&b));
        assert_eq!(points(&a)[0].0, secs(1.1));
        assert_eq!(points(&a)[39].0, secs(5.0));
        // Short-term loudness needs 3 s of sound; then a steady tone reads a steady level.
        let last = points(&a)[39].1;
        assert!(last.is_finite() && (-20.0..-5.0).contains(&last), "{last}");
        assert!((points(&a)[35].1 - last).abs() < 0.2);
        let (lanes, names) = a.lanes();
        assert_eq!(names, ["left", "right"]);
        assert!((lanes[0][100].max - 0.5).abs() < 0.001);
        assert_eq!(lanes[1][100].or_silent(), Column::default());
        assert_eq!(a.envelope.len(), 750);
        assert!((a.envelope[3] - 0.5).abs() < 0.001);
    }
}
