// SPDX-License-Identifier: GPL-3.0-or-later
//! [`ProxyGenerator`]: makes the proxy of one original (WP-2F.01), resumably.
//!
//! 1. **Index.** The original's frame index from `cache.sqlite` (the `index` analysis stage
//!    stored it; else a packet scan). Proxy frame `n` is original frame `n`: the proxy has the
//!    original's frame count, at a constant rate (the probed one), and the index is stored with
//!    it as its `pts_map`. Variable-frame-rate originals are conformed that way.
//! 2. **Parts.** Frames are cut into parts of [`GenerateOptions::part_frames`] frames, each
//!    encoded into its own MOV file ([`ProxyStore::part`]): decoded sequentially (hardware when
//!    the machine has a verified decoder), scaled and converted by the decoder in one swscale
//!    pass ([`DecodeOpts::target_size`]) and encoded intra-frame ([`ProxyCodec`]), with the
//!    original's sound as 16-bit PCM. A part appears under its name only when complete (the
//!    encoder's partial file), so after a cancel, a failure or a killed process the finished
//!    parts are kept and the next generation continues after them.
//! 3. **Join.** The parts are joined by copying packets ([`MediaBackend::concat`]) into the
//!    proxy file, which is indexed; the row becomes ready ([`ProxyStore::finish`]) and the size
//!    cap is enforced.
//!
//! **Frames that are missing** in the decode (a damaged stream drops a picture) are filled with
//! the previous frame, so the counts never drift. **Sound** of part `k` covers the original's
//! samples from the start of its first frame (sample 0 for the first part) to the start of the
//! next part's (the end of the stream for the last), so the joined proxy has the original's
//! sound from sample 0, gap-free; a decode error in the sound is filled with silence.
//!
//! **Niceness.** Decoders use at most half the cores and encoders a quarter, so a running
//! proxy leaves room for the preview; the job budget runs two at a time (one while exporting).

use std::path::Path;
use std::sync::Arc;

use cutweave_media::{
    AudioChunk, AudioDecoder, AudioEncodeSpec, CancelCheck, CancelToken, ConcatSpec, DecodeMode,
    DecodeOpts, EncodeSpec, Encoder, FrameIndex, HwMode, MediaBackend, MediaError, RateControl,
    SpeedPreset, VideoDecoder, VideoEncodeSpec, VideoFrame,
};
use cutweave_model::{
    ColorTags, Duration, FrameRate, ProbeInfo, Rounding, SampleRate, StreamKind, Time,
};

use super::store::{ProxyRecord, ProxyStore};
use super::{ProxyCodec, ProxyProfile};
use crate::{CacheDb, CacheError, ContentHash};

/// Default frames per part: 20 s at 30 fps. A killed generation loses at most one part.
pub const DEFAULT_PART_FRAMES: usize = 600;
/// Sound pushed ahead of the pictures in a part, at most: half a second, in flicks.
const AUDIO_LEAD_FLICKS: i64 = cutweave_model::FLICKS_PER_SECOND / 2;
/// Sound decoded per call.
const AUDIO_CHUNK_SAMPLES: i64 = 24_000;
/// Sound channels a proxy keeps (PCM in MOV); originals with more get a silent proxy.
const MAX_AUDIO_CHANNELS: u16 = 8;

/// The original a proxy is made of.
#[derive(Clone, Debug, PartialEq)]
pub struct ProxySource {
    /// Its file.
    pub path: std::path::PathBuf,
    /// Its content hash (the cache key).
    pub hash: ContentHash,
    /// What the import probe found (picture size, frame rate, color, sound).
    pub probe: ProbeInfo,
}

/// How proxies are generated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenerateOptions {
    /// Frames per part (see the module docs).
    pub part_frames: usize,
    /// Hardware decoding of the originals.
    pub decode_hw: HwMode,
    /// Decoder threads (0: half the cores).
    pub decode_threads: u16,
    /// Encoder threads (0: a quarter of the cores).
    pub encode_threads: u16,
}

impl Default for GenerateOptions {
    fn default() -> GenerateOptions {
        GenerateOptions {
            part_frames: DEFAULT_PART_FRAMES,
            decode_hw: HwMode::Auto,
            decode_threads: 0,
            encode_threads: 0,
        }
    }
}

/// What a generation did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenerateReport {
    /// The proxy's row now (`ready`, or `evicted` when it alone is over the cap).
    pub proxy: ProxyRecord,
    /// Parts encoded by this run.
    pub encoded_parts: usize,
    /// Parts found finished from an earlier run and reused.
    pub reused_parts: usize,
    /// Other proxies the cap evicted.
    pub evicted: Vec<ProxyRecord>,
}

/// Makes proxies (see the module docs).
pub struct ProxyGenerator {
    backend: Arc<dyn MediaBackend>,
    db: Arc<CacheDb>,
    store: Arc<ProxyStore>,
    options: GenerateOptions,
}

impl std::fmt::Debug for ProxyGenerator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyGenerator")
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

fn cores() -> u16 {
    std::thread::available_parallelism()
        .map_or(2, std::num::NonZero::get)
        .try_into()
        .unwrap_or(u16::MAX)
}

/// `done / total` as a fraction (frame counts stay far below 2^52).
#[allow(clippy::cast_precision_loss)]
fn fraction(done: usize, total: usize) -> f64 {
    done as f64 / total.max(1) as f64
}

fn invalid(message: String) -> CacheError {
    CacheError::InvalidParams(message)
}

fn media(e: MediaError) -> CacheError {
    CacheError::Media(e)
}

/// The picture size of the main video stream.
fn coded_size(probe: &ProbeInfo) -> Option<(u32, u32)> {
    let v = probe.streams.iter().find(|s| s.kind == StreamKind::Video)?;
    Some((v.width?, v.height?)).filter(|(w, h)| *w > 0 && *h > 0)
}

/// The proxy's constant frame rate: the probed one, else the index's average, else 30 fps.
fn proxy_rate(probe: &ProbeInfo, index: &FrameIndex) -> FrameRate {
    if let Some(rate) = probe.fps {
        return rate;
    }
    // The average in thousandths of a frame per second: frames × 1000 / seconds, in flicks.
    let frames = i128::try_from(index.frame_count()).unwrap_or(0);
    index
        .duration()
        .filter(|d| *d > Duration::ZERO && frames > 0)
        .and_then(|d| {
            let per_second = i128::from(cutweave_model::FLICKS_PER_SECOND);
            let thousandths =
                (frames * 1000 * per_second + i128::from(d.flicks()) / 2) / i128::from(d.flicks());
            u32::try_from(thousandths)
                .ok()
                .filter(|t| (1..=1_000_000).contains(t))
        })
        .and_then(|t| FrameRate::new(t, 1000).ok())
        .unwrap_or(FrameRate::FPS_30)
}

impl ProxyGenerator {
    /// Generates with `backend` into `store`, reading original indexes from `db`.
    #[must_use]
    pub fn new(
        backend: Arc<dyn MediaBackend>,
        db: Arc<CacheDb>,
        store: Arc<ProxyStore>,
        options: GenerateOptions,
    ) -> ProxyGenerator {
        ProxyGenerator {
            backend,
            db,
            store,
            options,
        }
    }

    /// The store.
    #[must_use]
    pub fn store(&self) -> &Arc<ProxyStore> {
        &self.store
    }

    /// The profile this backend makes: its best proxy codec at the default size; `None` when
    /// it encodes no proxy codec.
    #[must_use]
    pub fn profile(&self) -> Option<ProxyProfile> {
        ProxyProfile::for_backend(&self.backend.capabilities())
    }

    /// Makes (or finishes) the proxy of `source` in `profile`. `cancel` stops it between
    /// frames, keeping the finished parts; `progress` gets the done fraction. A proxy that is
    /// already ready is returned as it is.
    pub fn generate(
        &self,
        source: &ProxySource,
        profile: &ProxyProfile,
        cancel: &CancelToken,
        progress: &mut dyn FnMut(f64),
    ) -> Result<GenerateReport, CacheError> {
        let key = profile.key();
        if let Some(ready) = self.store.ready(&source.hash, &key)? {
            progress(1.0);
            return Ok(GenerateReport {
                proxy: ready.record.clone(),
                encoded_parts: 0,
                reused_parts: 0,
                evicted: Vec::new(),
            });
        }
        let index = Arc::new(self.db.frame_index_for(
            &*self.backend,
            &source.path,
            &source.hash,
        )?);
        let frames = index.frame_count();
        if frames == 0 {
            return Err(invalid(format!(
                "`{}` has no frames to make a proxy of",
                source.path.display()
            )));
        }
        let original = coded_size(&source.probe).ok_or_else(|| {
            invalid(format!(
                "`{}` has no picture size in its probe",
                source.path.display()
            ))
        })?;
        let plan = Plan {
            source,
            index: Arc::clone(&index),
            codec: profile.codec,
            size: profile.size_for(original),
            rate: proxy_rate(&source.probe, &index),
            audio: source
                .probe
                .audio
                .filter(|a| (1..=MAX_AUDIO_CHANNELS).contains(&a.channels))
                .map(|a| (a.rate, a.channels)),
        };
        self.store
            .begin(&source.hash, profile, frames as u64, plan.size)?;
        let part_frames = self.options.part_frames.max(1);
        let parts = frames.div_ceil(part_frames);
        let mut run = Run::new(self, &plan, cancel);
        let mut encoded = 0;
        let mut reused = 0;
        let mut paths = Vec::with_capacity(parts);
        for k in 0..parts {
            let path = self.store.part(&source.hash, &key, k);
            let range = k * part_frames..((k + 1) * part_frames).min(frames);
            if path.is_file() {
                reused += 1;
            } else {
                run.part(&path, range.clone(), k + 1 == parts, &mut |done| {
                    progress(0.97 * fraction(range.start + done, frames));
                })?;
                encoded += 1;
            }
            progress(0.97 * fraction(range.end, frames));
            paths.push(path);
        }
        drop(run);
        cancel.check()?;
        let file = self.store.file(&source.hash, &key);
        let token = cancel.clone();
        let joined = self.backend.concat(
            &ConcatSpec::new(paths, &file)
                .with_cancel(CancelCheck::new(move || token.is_cancelled())),
        );
        let joined = match joined {
            Ok(summary) => summary,
            Err(MediaError::Cancelled) => return Err(media(MediaError::Cancelled)),
            Err(e) => return Err(self.failed(source, &key, media(e))),
        };
        progress(0.99);
        let proxy_index = match self.backend.index_media(&file) {
            Ok(i) => i,
            Err(e) => return Err(self.failed(source, &key, media(e))),
        };
        if proxy_index.frame_count() != frames || joined.video_frames != frames as u64 {
            let e = invalid(format!(
                "the proxy of `{}` has {} frames, the original {frames}",
                source.path.display(),
                proxy_index.frame_count()
            ));
            return Err(self.failed(source, &key, e));
        }
        let (proxy, evicted) = self
            .store
            .finish(&source.hash, &key, &proxy_index, &index)?;
        progress(1.0);
        Ok(GenerateReport {
            proxy,
            encoded_parts: encoded,
            reused_parts: reused,
            evicted,
        })
    }

    /// Records `error` as the proxy's failure and returns it.
    fn failed(&self, source: &ProxySource, key: &str, error: CacheError) -> CacheError {
        if let Err(e) = self.store.fail(&source.hash, key, &error.to_string()) {
            tracing::warn!(error = %e, "cannot record a failed proxy");
        }
        error
    }
}

/// What every part of one generation shares.
struct Plan<'a> {
    source: &'a ProxySource,
    index: Arc<FrameIndex>,
    codec: ProxyCodec,
    size: (u32, u32),
    rate: FrameRate,
    /// Sample rate and channels of the sound, if the proxy has some.
    audio: Option<(SampleRate, u16)>,
}

/// One generation's decoders, kept across parts so consecutive parts decode on without a seek.
struct Run<'a> {
    generator: &'a ProxyGenerator,
    plan: &'a Plan<'a>,
    cancel: &'a CancelToken,
    video: Option<Box<dyn VideoDecoder>>,
    /// The original frame `video` returns next.
    next: usize,
    /// A decoded frame not used yet (it belongs to a later frame number).
    pending: Option<VideoFrame>,
    audio: Option<Box<dyn AudioDecoder>>,
    /// The original sample `audio` returns next.
    audio_next: i64,
    /// The sound failed to decode: the rest of the proxy is silent.
    audio_failed: bool,
}

impl<'a> Run<'a> {
    fn new(generator: &'a ProxyGenerator, plan: &'a Plan<'a>, cancel: &'a CancelToken) -> Run<'a> {
        Run {
            generator,
            plan,
            cancel,
            video: None,
            next: usize::MAX,
            pending: None,
            audio: None,
            audio_next: -1,
            audio_failed: false,
        }
    }

    fn decode_opts(&self) -> DecodeOpts {
        let options = &self.generator.options;
        let threads = if options.decode_threads == 0 {
            (cores() / 2).max(2)
        } else {
            options.decode_threads
        };
        DecodeOpts {
            threads,
            ..DecodeOpts::default()
                .with_index(Arc::clone(&self.plan.index))
                .with_mode(DecodeMode::Sequential)
                .with_hw(options.decode_hw)
                .with_target(self.plan.codec.input_format())
                .with_size(self.plan.size.0, self.plan.size.1)
        }
    }

    /// The decoded original frame `n` (frames come in order; a missing one is `None`).
    fn frame(&mut self, n: usize) -> Result<Option<VideoFrame>, MediaError> {
        let index = Arc::clone(&self.plan.index);
        let expected = index
            .time_of(n)
            .ok_or(MediaError::OutOfRange { t: Time::MAX })?;
        if self.next != n || self.video.is_none() {
            // Start, or resume after finished parts: seek to the frame.
            self.pending = None;
            if self.video.is_none() {
                let mut d = self
                    .generator
                    .backend
                    .open_decoder(&self.plan.source.path, &self.decode_opts())?;
                d.set_cancel(Some(self.cancel.clone()));
                self.video = Some(d);
            }
            let d = self.video.as_mut().ok_or(MediaError::Cancelled)?;
            let f = d.seek_exact(expected)?;
            self.next = n + 1;
            if f.pts == expected {
                return Ok(Some(f));
            }
            self.pending = Some(f);
            return Ok(None);
        }
        self.next = n + 1;
        loop {
            let f = match self.pending.take() {
                Some(f) => f,
                None => match self
                    .video
                    .as_mut()
                    .ok_or(MediaError::Cancelled)?
                    .next_frame()?
                {
                    Some(f) => f,
                    None => return Ok(None),
                },
            };
            if f.pts == expected {
                return Ok(Some(f));
            }
            if f.pts > expected {
                // Frame `n` is missing: keep this one for its own number.
                self.pending = Some(f);
                return Ok(None);
            }
            // An earlier frame (a duplicate timestamp): skip it.
        }
    }

    /// The original sample at time `t` (floor, at least 0).
    fn sample_at(rate: SampleRate, t: Time) -> i64 {
        rate.time_to_samples(t.max(Time::ZERO), Rounding::Floor)
            .max(0)
    }

    /// Up to `max` samples of the original's sound from sample `from`, silence after an error.
    fn sound(&mut self, from: i64, max: i64) -> Result<Vec<Vec<f32>>, MediaError> {
        let Some((rate, channels)) = self.plan.audio else {
            return Ok(Vec::new());
        };
        let silence = |n: usize| vec![vec![0.0_f32; n]; usize::from(channels)];
        let count = usize::try_from(max.max(0)).unwrap_or(0);
        if self.audio_failed {
            return Ok(silence(count));
        }
        if self.audio.is_none() {
            match self
                .generator
                .backend
                .open_audio_decoder(&self.plan.source.path, None)
            {
                Ok(mut d) => {
                    d.set_cancel(Some(self.cancel.clone()));
                    self.audio = Some(d);
                }
                Err(e) => {
                    tracing::warn!(error = %e, path = %self.plan.source.path.display(), "the proxy's sound cannot be decoded; it is silent");
                    self.audio_failed = true;
                    return Ok(silence(count));
                }
            }
        }
        let start = rate
            .samples_to_time(from)
            .map_err(|_| MediaError::OutOfRange { t: Time::MAX })?;
        let dur = rate
            .samples_to_time(max)
            .ok()
            .and_then(|t| Duration::try_from(t).ok())
            .unwrap_or(Duration::ZERO);
        let d = self.audio.as_mut().ok_or(MediaError::Cancelled)?;
        match d.decode_range(start, dur) {
            Ok(chunk) if chunk.channels == channels => {
                let mut planes = chunk.planes;
                for p in &mut planes {
                    p.truncate(count);
                }
                let got = planes.first().map_or(0, Vec::len);
                self.audio_next = from.saturating_add(i64::try_from(got).unwrap_or(i64::MAX));
                Ok(planes)
            }
            Ok(_) | Err(MediaError::OutOfRange { .. }) => Ok(Vec::new()),
            Err(MediaError::Cancelled) => Err(MediaError::Cancelled),
            Err(e) => {
                tracing::warn!(error = %e, path = %self.plan.source.path.display(), "the proxy's sound failed to decode; the rest is silent");
                self.audio_failed = true;
                Ok(silence(count))
            }
        }
    }

    /// Encodes original frames `range` (and their sound) into the part at `path`.
    fn part(
        &mut self,
        path: &Path,
        range: std::ops::Range<usize>,
        last: bool,
        progress: &mut dyn FnMut(usize),
    ) -> Result<(), CacheError> {
        let plan = self.plan;
        let options = &self.generator.options;
        let color: ColorTags = plan.source.probe.color.clone();
        let mut spec = EncodeSpec::new(path)
            .with_hw(cutweave_media::HwEncode::Off)
            .with_video(VideoEncodeSpec {
                codec: plan.codec.codec().to_owned(),
                width: plan.size.0,
                height: plan.size.1,
                rate: plan.rate,
                input_format: plan.codec.input_format(),
                rate_control: RateControl::default(),
                preset: SpeedPreset::default(),
                color: color.clone(),
                threads: if options.encode_threads == 0 {
                    (cores() / 4).max(1)
                } else {
                    options.encode_threads
                },
            });
        if let Some((rate, channels)) = plan.audio {
            spec = spec.with_audio(AudioEncodeSpec {
                codec: "pcm_s16le".to_owned(),
                rate,
                channels,
                bitrate: rate.hz() * u32::from(channels) * 16,
            });
        }
        let token = self.cancel.clone();
        spec = spec.with_cancel(CancelCheck::new(move || token.is_cancelled()));
        let mut enc = self.generator.backend.open_encoder(&spec).map_err(media)?;
        // The part's sound: from its first frame's start (0 for the first part) to the next
        // part's first frame (the end of the stream for the last part).
        let sound = plan.audio.map(|(rate, _)| {
            let from = if range.start == 0 {
                0
            } else {
                plan.index
                    .time_of(range.start)
                    .map_or(0, |t| Self::sample_at(rate, t))
            };
            let to = if last {
                i64::MAX
            } else {
                plan.index
                    .time_of(range.end)
                    .map_or(i64::MAX, |t| Self::sample_at(rate, t))
            };
            (rate, from, to)
        });
        let mut written: i64 = 0;
        let mut previous: Option<VideoFrame> = None;
        for (i, n) in range.clone().enumerate() {
            let mut frame = match self.frame(n).map_err(media)? {
                Some(f) => f,
                None => match &previous {
                    Some(p) => p.clone(),
                    // The first frames are missing: the next decoded one stands in.
                    None => self.next_available().map_err(media)?,
                },
            };
            let local = i64::try_from(i).unwrap_or(i64::MAX);
            frame.pts = plan
                .rate
                .frame_to_time(local)
                .map_err(|e| invalid(e.to_string()))?;
            frame.color = color.clone();
            frame.damaged = false;
            enc.push_video(&frame).map_err(media)?;
            // Sound up to half a second past the pictures.
            if let Some((rate, from, to)) = sound {
                let video_end = plan
                    .rate
                    .frame_to_time(local + 1)
                    .map_err(|e| invalid(e.to_string()))?;
                let lead = Duration::from_flicks(AUDIO_LEAD_FLICKS).unwrap_or(Duration::ZERO);
                let until = Self::sample_at(rate, video_end.saturating_add(lead));
                self.push_sound(&mut *enc, rate, from, to, &mut written, until)?;
            }
            progress(i + 1);
            previous = Some(frame);
        }
        if let Some((rate, from, to)) = sound {
            self.push_sound(&mut *enc, rate, from, to, &mut written, i64::MAX)?;
        }
        enc.finish().map_err(media)?;
        Ok(())
    }

    /// A frame for a part whose first frames are missing: the next one the decoder has.
    fn next_available(&mut self) -> Result<VideoFrame, MediaError> {
        if let Some(f) = self.pending.clone() {
            return Ok(f);
        }
        let d = self.video.as_mut().ok_or(MediaError::Cancelled)?;
        let f = d
            .next_frame()?
            .ok_or(MediaError::OutOfRange { t: Time::MAX })?;
        self.pending = Some(f.clone());
        Ok(f)
    }

    /// Pushes the part's sound (original samples `from..to`) up to local sample `until`.
    fn push_sound(
        &mut self,
        enc: &mut dyn Encoder,
        rate: SampleRate,
        from: i64,
        to: i64,
        written: &mut i64,
        until: i64,
    ) -> Result<(), CacheError> {
        while *written < until {
            let at = from.saturating_add(*written);
            if at >= to {
                return Ok(());
            }
            let want = AUDIO_CHUNK_SAMPLES
                .min(to.saturating_sub(at))
                .min(until.saturating_sub(*written));
            if self.audio_next >= 0 && self.audio_next != at && self.audio.is_some() {
                // Not where the decoder is (a resumed part): `decode_range` seeks.
                self.audio_next = at;
            }
            let planes = self.sound(at, want).map_err(media)?;
            let got = planes.first().map_or(0, Vec::len);
            if got == 0 {
                // The end of the stream.
                return Ok(());
            }
            let channels = u16::try_from(planes.len()).unwrap_or(0);
            enc.push_audio(&AudioChunk {
                pts: rate
                    .samples_to_time(*written)
                    .map_err(|e| invalid(e.to_string()))?,
                rate,
                channels,
                planes,
            })
            .map_err(media)?;
            let got = i64::try_from(got).unwrap_or(i64::MAX);
            *written = written.saturating_add(got);
            if got < want {
                return Ok(());
            }
        }
        Ok(())
    }
}
