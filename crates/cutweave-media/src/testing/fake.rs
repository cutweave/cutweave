// SPDX-License-Identifier: GPL-3.0-or-later
//! [`FakeBackend`]: a [`MediaBackend`] over in-memory synthetic media.
//!
//! Registered paths serve generated pictures and tones with exact timestamps; nothing touches
//! FFmpeg or the disk (except that unregistered paths are checked on disk, so missing, damaged and
//! bad paths fail exactly as with the real backend). Upstream crates (cache, jobs, audio,
//! playback, export) test against it without FFmpeg.
//!
//! **Pictures.** Frame `n` of a [`FakeVideo`] has pts `rate.frame_to_time(n)`. Its pixels are a
//! deterministic pattern of `(x, y, n, plane)`, and the first four samples of the first row of
//! plane 0 carry `n` as a little-endian `u32` ([`fake_frame_number`]), so a test can tell which
//! frame it got from the pixels alone.
//!
//! **Tones.** Channel `c` of a [`FakeAudio`] is a sine at `frequency_hz × (c + 1)` with peak
//! `amplitude`, sample `n` at time `rate.samples_to_time(n)` ([`FakeAudio::sample`]).
//!
//! **Decoders** behave like the real ones (the backend contract holds both to it): they poll
//! their [`CancelToken`](crate::CancelToken), report their [`FrameIndex`], and with
//! [`FakeBackend::with_frame_cost`] take time like a real decoder (a seek decodes forward from its
//! keyframe, every [`FAKE_GOP_FRAMES`] frames unless [`FakeVideo::with_gop`] says otherwise), so
//! timing and cancellation can be tested without FFmpeg.
//!
//! **Encoded files read back.** A finished encode registers its output, so probing or decoding
//! it serves synthetic media of the same size, rate and length (frames decode as `Yuv420p`, like
//! an H.264 file). [`MediaBackend::concat`] joins registered outputs the same way. With
//! [`FakeBackend::with_output_files`] encodes and joins also write a small text file describing
//! the media (and only on success: a cancelled or failed encode leaves nothing), which any
//! `FakeBackend` reads back, so outputs survive a new backend (a restarted engine in a test).
//!
//! **Scaling.** A decoder opened with [`DecodeOpts::target_size`] serves frames of that size
//! (the same pattern and frame number).

use std::collections::BTreeMap;
use std::f64::consts::TAU;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration as Wall, Instant};

use cutweave_model::{
    AudioInfo, ColorTags, Duration, Extra, FrameRate, ProbeInfo, Rounding, SampleRate, StreamInfo,
    StreamKind, Time,
};

use crate::backend::{
    AudioDecoder, ConcatSpec, DecodeOpts, EncodeSpec, EncodeSummary, Encoder, MediaBackend,
    VideoDecoder,
};
use crate::cancel;
use crate::encode::{InputCheck, output_color};
use crate::path;
use crate::{
    AudioChunk, AudioFormat, CancelCheck, CancelToken, Capabilities, CodecSupport, FrameIndex,
    IndexEntry, MediaError, PixelFormat, Plane, ScanInfo, TimeBase, VideoFrame,
};

/// Codec name the fake reports for its pictures.
pub const FAKE_VIDEO_CODEC: &str = "rawvideo";
/// Codec name the fake reports for its tones.
pub const FAKE_AUDIO_CODEC: &str = "pcm_f32le";

/// Frames per group of pictures in the fake's index unless [`FakeVideo::with_gop`] sets another
/// (a keyframe every this many frames).
pub const FAKE_GOP_FRAMES: u64 = 12;
/// Bytes per packet in the fake's index (`pos` of frame `n` is `n × this`).
const FAKE_PACKET_BYTES: u64 = 1000;

/// A synthetic video stream.
#[derive(Clone, Debug, PartialEq)]
pub struct FakeVideo {
    /// Width in pixels (at least 4 for [`fake_frame_number`] to read back).
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Frame rate; frame `n` has pts `rate.frame_to_time(n)`.
    pub rate: FrameRate,
    /// Number of frames.
    pub frames: u64,
    /// Native layout of the decoded frames.
    pub format: PixelFormat,
    /// Color tags reported by the probe and on every frame.
    pub color: ColorTags,
    /// Display rotation reported by the probe (0, 90, 180, 270).
    pub rotation: u16,
    /// Frames per group of pictures: a keyframe every this many frames (at least 1).
    pub gop: u64,
}

impl FakeVideo {
    /// `frames` frames of `width × height` 8-bit 4:2:0 at `rate`.
    #[must_use]
    pub fn new(width: u32, height: u32, rate: FrameRate, frames: u64) -> FakeVideo {
        FakeVideo {
            width,
            height,
            rate,
            frames,
            format: PixelFormat::Yuv420p,
            color: ColorTags::default(),
            rotation: 0,
            gop: FAKE_GOP_FRAMES,
        }
    }

    /// As many frames as start before `duration` at `rate`.
    #[must_use]
    pub fn lasting(width: u32, height: u32, rate: FrameRate, duration: Duration) -> FakeVideo {
        let frames = first_unit_at_or_after(
            Time::from(duration),
            |t| rate.time_to_frame(t, Rounding::Floor),
            |n| rate.frame_to_time(n).ok(),
        );
        FakeVideo::new(width, height, rate, u64::try_from(frames).unwrap_or(0))
    }

    /// The same stream decoding to `format`.
    #[must_use]
    pub fn with_format(mut self, format: PixelFormat) -> FakeVideo {
        self.format = format;
        self
    }

    /// The same stream with color tags.
    #[must_use]
    pub fn with_color(mut self, color: ColorTags) -> FakeVideo {
        self.color = color;
        self
    }

    /// The same stream with a display rotation.
    #[must_use]
    pub fn with_rotation(mut self, degrees: u16) -> FakeVideo {
        self.rotation = degrees;
        self
    }

    /// The same stream with a keyframe every `frames` frames (at least 1; default
    /// [`FAKE_GOP_FRAMES`]).
    #[must_use]
    pub fn with_gop(mut self, frames: u64) -> FakeVideo {
        self.gop = frames.max(1);
        self
    }

    /// The pts of frame `n`.
    pub fn pts(&self, n: u64) -> Result<Time, MediaError> {
        let n = i64::try_from(n).map_err(|_| out_of_range(Time::MAX))?;
        self.rate
            .frame_to_time(n)
            .map_err(|_| out_of_range(Time::MAX))
    }

    /// The end of the last frame (the pts frame `frames` would have).
    pub fn duration(&self) -> Result<Duration, MediaError> {
        let end = self.pts(self.frames)?;
        Duration::try_from(end).map_err(|_| out_of_range(end))
    }

    /// Frame `n` in `format`.
    pub fn frame(&self, n: u64, format: PixelFormat) -> Result<VideoFrame, MediaError> {
        Ok(VideoFrame {
            pts: self.pts(n)?,
            width: self.width,
            height: self.height,
            format,
            planes: fake_planes(self.width, self.height, format, n),
            color: self.color.clone(),
            damaged: false,
        })
    }
}

/// A synthetic audio stream: one sine per channel.
#[derive(Clone, Debug, PartialEq)]
pub struct FakeAudio {
    /// Sample rate.
    pub rate: SampleRate,
    /// Channel count (≥ 1).
    pub channels: u16,
    /// Samples per channel.
    pub samples: u64,
    /// Frequency of channel 0; channel `c` plays `frequency_hz × (c + 1)`.
    pub frequency_hz: f64,
    /// Peak amplitude (0.5 is −6 dBFS).
    pub amplitude: f32,
}

impl FakeAudio {
    /// A 440 Hz (channel 0) tone at −6 dBFS lasting `duration`.
    #[must_use]
    pub fn tone(rate: SampleRate, channels: u16, duration: Duration) -> FakeAudio {
        let samples = first_unit_at_or_after(
            Time::from(duration),
            |t| rate.time_to_samples(t, Rounding::Floor),
            |n| rate.samples_to_time(n).ok(),
        );
        FakeAudio {
            rate,
            channels,
            samples: u64::try_from(samples).unwrap_or(0),
            frequency_hz: 440.0,
            amplitude: 0.5,
        }
    }

    /// Sample `n` of `channel`: `amplitude × sin(2π × f_c × n / rate)`.
    #[must_use]
    // Sample indices stay far below 2^53, and the output is f32 by design.
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    pub fn sample(&self, channel: u16, n: u64) -> f32 {
        let f = self.frequency_hz * f64::from(channel + 1);
        let phase = (f * n as f64 / f64::from(self.rate.hz())).fract();
        (f64::from(self.amplitude) * (TAU * phase).sin()) as f32
    }

    /// The end of the last sample.
    pub fn duration(&self) -> Result<Duration, MediaError> {
        let n = i64::try_from(self.samples).map_err(|_| out_of_range(Time::MAX))?;
        let end = self
            .rate
            .samples_to_time(n)
            .map_err(|_| out_of_range(Time::MAX))?;
        Duration::try_from(end).map_err(|_| out_of_range(end))
    }

    fn format(&self) -> AudioFormat {
        AudioFormat {
            rate: self.rate,
            channels: self.channels,
        }
    }
}

/// A synthetic media file: a video stream, an audio stream, or both (video first).
#[derive(Clone, Debug, PartialEq)]
pub struct FakeMedia {
    /// The picture stream (container index 0), if any.
    pub video: Option<FakeVideo>,
    /// The audio stream (index 1 after a video stream, else 0), if any.
    pub audio: Option<FakeAudio>,
}

impl FakeMedia {
    /// A typical test clip: `duration` of 64×36 8-bit 4:2:0 pictures at `rate` plus a 48 kHz
    /// stereo tone of the same length.
    #[must_use]
    pub fn clip(duration: Duration, rate: FrameRate) -> FakeMedia {
        FakeMedia {
            video: Some(FakeVideo::lasting(64, 36, rate, duration)),
            audio: Some(FakeAudio::tone(SampleRate::HZ_48_000, 2, duration)),
        }
    }

    /// Pictures only.
    #[must_use]
    pub fn video(video: FakeVideo) -> FakeMedia {
        FakeMedia {
            video: Some(video),
            audio: None,
        }
    }

    /// Sound only.
    #[must_use]
    pub fn audio(audio: FakeAudio) -> FakeMedia {
        FakeMedia {
            video: None,
            audio: Some(audio),
        }
    }

    fn audio_index(&self) -> u32 {
        u32::from(self.video.is_some())
    }

    /// What [`MediaBackend::index_media`] reports: one packet per frame in presentation order
    /// (no B-frames), pts and dts equal to the frame's time in flicks (time base
    /// [`TimeBase::FLICKS`]), a keyframe every [`FakeVideo::gop`] frames.
    pub fn index(&self, p: &Path) -> Result<FrameIndex, MediaError> {
        let v = self.video.as_ref().ok_or_else(|| MediaError::NoStream {
            path: path::display(p),
            kind: StreamKind::Video,
            index: None,
        })?;
        let entries = (0..v.frames)
            .map(|n| {
                let flicks = v.pts(n)?.flicks();
                Ok(IndexEntry {
                    pts: Some(flicks),
                    dts: Some(flicks),
                    is_key: n % v.gop.max(1) == 0,
                    discard: false,
                    pos: i64::try_from(n * FAKE_PACKET_BYTES).unwrap_or(-1),
                })
            })
            .collect::<Result<Vec<_>, MediaError>>()?;
        FrameIndex::from_entries(0, TimeBase::FLICKS, entries, ScanInfo::default())
            .map_err(|e| e.into_media(p))
    }

    /// What [`MediaBackend::probe`] reports for this media.
    pub fn probe(&self) -> Result<ProbeInfo, MediaError> {
        let mut info = ProbeInfo::default();
        let mut duration = Duration::ZERO;
        if let Some(v) = &self.video {
            info.streams.push(StreamInfo {
                index: 0,
                kind: StreamKind::Video,
                codec: Some(FAKE_VIDEO_CODEC.to_owned()),
                profile: None,
                width: Some(v.width),
                height: Some(v.height),
                pixel_format: Some(v.format.ffmpeg_name().to_owned()),
                sample_rate: None,
                channels: None,
                language: None,
                extra: Extra::new(),
            });
            info.fps = Some(v.rate);
            info.rotation = v.rotation;
            info.color = v.color.clone();
            duration = duration.max(v.duration()?);
        }
        if let Some(a) = &self.audio {
            info.streams.push(StreamInfo {
                index: self.audio_index(),
                kind: StreamKind::Audio,
                codec: Some(FAKE_AUDIO_CODEC.to_owned()),
                profile: None,
                width: None,
                height: None,
                pixel_format: None,
                sample_rate: Some(a.rate),
                channels: Some(a.channels),
                language: None,
                extra: Extra::new(),
            });
            info.audio = Some(AudioInfo {
                rate: a.rate,
                channels: a.channels,
            });
            duration = duration.max(a.duration()?);
        }
        info.duration = Some(duration);
        Ok(info)
    }
}

#[derive(Clone, Debug)]
enum Entry {
    Media(FakeMedia),
    Error(MediaError),
}

#[derive(Debug, Default)]
struct Shared {
    decoded_frames: AtomicU64,
    index_scans: AtomicU64,
    decoders_opened: AtomicU64,
    /// Decoders opened on the pretend hardware path ([`FakeBackend::with_pseudo_hw_decode`]).
    hw_decoders_opened: AtomicU64,
    encodes: Mutex<Vec<(PathBuf, EncodeSummary)>>,
    /// Finished encodes, readable back.
    outputs: Mutex<BTreeMap<PathBuf, FakeMedia>>,
}

/// A [`MediaBackend`] serving registered in-memory media (see the module docs).
///
/// Clones share their counters, encode log and encoded outputs.
#[derive(Clone, Debug)]
pub struct FakeBackend {
    media: BTreeMap<PathBuf, Entry>,
    caps: Capabilities,
    frame_cost: Wall,
    /// Decoders opened with [`HwMode::Auto`](crate::HwMode::Auto) pretend to be hardware ones.
    pseudo_hw: bool,
    /// Encodes and joins write a description file ([`FakeBackend::with_output_files`]).
    write_files: bool,
    shared: Arc<Shared>,
}

/// The first line of a file [`FakeBackend::with_output_files`] writes.
const FAKE_FILE_MAGIC: &str = "cutweave-fake-media 1";

impl FakeMedia {
    /// The text a [`FakeBackend::with_output_files`] output holds.
    fn describe(&self) -> String {
        use std::fmt::Write as _;
        let mut text = format!("{FAKE_FILE_MAGIC}\n");
        if let Some(v) = &self.video {
            let _ = writeln!(
                text,
                "video {} {} {}/{} {} {}",
                v.width,
                v.height,
                v.rate.num(),
                v.rate.den(),
                v.frames,
                v.format.ffmpeg_name()
            );
        }
        if let Some(a) = &self.audio {
            let _ = writeln!(text, "audio {} {} {}", a.rate.hz(), a.channels, a.samples);
        }
        text
    }

    /// Reads back [`describe`](Self::describe); `None` for any other content.
    fn parse(text: &str) -> Option<FakeMedia> {
        let mut lines = text.lines();
        if lines.next()? != FAKE_FILE_MAGIC {
            return None;
        }
        let mut media = FakeMedia {
            video: None,
            audio: None,
        };
        for line in lines {
            let f: Vec<&str> = line.split_whitespace().collect();
            match f.as_slice() {
                ["video", w, h, rate, frames, format] => {
                    let (num, den) = rate.split_once('/')?;
                    let rate = FrameRate::new(num.parse().ok()?, den.parse().ok()?).ok()?;
                    let format = PixelFormat::ALL
                        .into_iter()
                        .find(|p| p.ffmpeg_name() == *format)?;
                    media.video = Some(
                        FakeVideo::new(
                            w.parse().ok()?,
                            h.parse().ok()?,
                            rate,
                            frames.parse().ok()?,
                        )
                        .with_format(format),
                    );
                }
                ["audio", rate, channels, samples] => {
                    let mut audio = FakeAudio::tone(
                        SampleRate::new(rate.parse().ok()?).ok()?,
                        channels.parse().ok()?,
                        Duration::ZERO,
                    );
                    audio.samples = samples.parse().ok()?;
                    media.audio = Some(audio);
                }
                _ => return None,
            }
        }
        Some(media)
    }
}

impl Default for FakeBackend {
    fn default() -> Self {
        FakeBackend::new()
    }
}

impl FakeBackend {
    /// No media; decodes and encodes `rawvideo` and `pcm_f32le`.
    #[must_use]
    pub fn new() -> FakeBackend {
        let codec = |name: &str, kind| CodecSupport {
            name: name.to_owned(),
            kind,
            decoders: vec![name.to_owned()],
            encoders: vec![name.to_owned()],
        };
        FakeBackend {
            media: BTreeMap::new(),
            caps: Capabilities::with_codecs([
                codec(FAKE_VIDEO_CODEC, StreamKind::Video),
                codec(FAKE_AUDIO_CODEC, StreamKind::Audio),
            ]),
            frame_cost: Wall::ZERO,
            pseudo_hw: false,
            write_files: false,
            shared: Arc::default(),
        }
    }

    /// Makes encodes and joins write a small file describing their output at the output path
    /// (when they succeed), which every `FakeBackend` reads back as that media: outputs then
    /// exist on disk, survive a new backend, and their files can be listed, sized and deleted
    /// (proxy tests, WP-2F.01).
    #[must_use]
    pub fn with_output_files(mut self) -> FakeBackend {
        self.write_files = true;
        self
    }

    /// Registers `media` as an encoded output at `path` (and writes its file when outputs are
    /// files).
    fn store_output(&self, path: &Path, media: FakeMedia) -> Result<(), MediaError> {
        if self.write_files {
            let tmp = path.with_extension("fake-partial");
            let io = |e: std::io::Error| MediaError::Io {
                path: path::display(path),
                message: e.to_string(),
            };
            std::fs::write(&tmp, media.describe()).map_err(io)?;
            std::fs::rename(&tmp, path).map_err(io)?;
        }
        self.shared
            .outputs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(path.to_owned(), media);
        Ok(())
    }

    /// Makes every video frame a decoder decodes take `cost` of wall time (sleeping, polling its
    /// cancel token at least every millisecond): `next_frame` decodes one frame, and a seek
    /// decodes forward from the frame's keyframe (or from where the decoder is, when that is on
    /// the way), like a long-GOP decoder.
    #[must_use]
    pub fn with_frame_cost(mut self, cost: Wall) -> FakeBackend {
        self.frame_cost = cost;
        self
    }

    /// Makes every decoder opened with [`HwMode::Auto`](crate::HwMode::Auto) report itself as a
    /// hardware decoder ([`VideoDecoder::is_hw`]) and count in
    /// [`hw_decoders_opened`](Self::hw_decoders_opened): the decoder-kind probe of tests of
    /// the hardware decoding setting. The pixels are the same.
    #[must_use]
    pub fn with_pseudo_hw_decode(mut self) -> FakeBackend {
        self.pseudo_hw = true;
        self
    }

    /// Video decoders opened on the pretend hardware path.
    #[must_use]
    pub fn hw_decoders_opened(&self) -> u64 {
        self.shared.hw_decoders_opened.load(Ordering::Relaxed)
    }

    /// Frame indexes built so far (`index_media` calls and decoders opened without an index).
    #[must_use]
    pub fn index_scans(&self) -> u64 {
        self.shared.index_scans.load(Ordering::Relaxed)
    }

    /// Video decoders opened so far.
    #[must_use]
    pub fn decoders_opened(&self) -> u64 {
        self.shared.decoders_opened.load(Ordering::Relaxed)
    }

    /// Serves `media` at `path` (the path need not exist on disk).
    #[must_use]
    pub fn with_media(mut self, path: impl Into<PathBuf>, media: FakeMedia) -> FakeBackend {
        self.media.insert(path.into(), Entry::Media(media));
        self
    }

    /// Fails every probe and open of `path` with `error`.
    #[must_use]
    pub fn with_error(mut self, path: impl Into<PathBuf>, error: MediaError) -> FakeBackend {
        self.media.insert(path.into(), Entry::Error(error));
        self
    }

    /// Reports `caps` instead of the default codecs (encoders check it).
    #[must_use]
    pub fn with_capabilities(mut self, caps: Capabilities) -> FakeBackend {
        self.caps = caps;
        self
    }

    /// Video frames handed out so far by every decoder of this backend and its clones.
    #[must_use]
    pub fn decoded_frames(&self) -> u64 {
        self.shared.decoded_frames.load(Ordering::Relaxed)
    }

    /// Every finished encode so far: output path and what was written.
    #[must_use]
    pub fn encodes(&self) -> Vec<(PathBuf, EncodeSummary)> {
        self.shared
            .encodes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The media at `p`: registered, or written by an encode of this backend (or a clone).
    fn lookup(&self, p: &Path) -> Result<FakeMedia, MediaError> {
        path::check_syntax(p)?;
        match self.media.get(p) {
            Some(Entry::Media(m)) => Ok(m.clone()),
            Some(Entry::Error(e)) => Err(e.clone()),
            None => {
                let encoded = self
                    .shared
                    .outputs
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get(p)
                    .cloned();
                if let Some(m) = encoded {
                    return Ok(m);
                }
                path::check_file(p)?;
                // A file another fake backend wrote ([`FakeBackend::with_output_files`]).
                if let Some(m) = std::fs::read(p)
                    .ok()
                    .filter(|b| b.starts_with(FAKE_FILE_MAGIC.as_bytes()))
                    .and_then(|b| FakeMedia::parse(&String::from_utf8_lossy(&b)))
                {
                    return Ok(m);
                }
                Err(MediaError::Corrupt {
                    path: path::display(p),
                    message: "not a media file this backend can read \
                              (FakeBackend serves registered paths only)"
                        .to_owned(),
                })
            }
        }
    }
}

impl MediaBackend for FakeBackend {
    fn probe(&self, path: &Path) -> Result<ProbeInfo, MediaError> {
        self.lookup(path)?.probe()
    }

    fn index_media(&self, path: &Path) -> Result<FrameIndex, MediaError> {
        let index = self.lookup(path)?.index(path)?;
        self.shared.index_scans.fetch_add(1, Ordering::Relaxed);
        Ok(index)
    }

    fn open_decoder(
        &self,
        p: &Path,
        opts: &DecodeOpts,
    ) -> Result<Box<dyn VideoDecoder>, MediaError> {
        let media = self.lookup(p)?;
        let mut video = media.video.clone().ok_or_else(|| MediaError::NoStream {
            path: path::display(p),
            kind: StreamKind::Video,
            index: None,
        })?;
        let index = match &opts.index {
            Some(index) => Arc::clone(index),
            None => Arc::new(self.index_media(p)?),
        };
        if index.frame_count() as u64 != video.frames || index.time_base() != TimeBase::FLICKS {
            return Err(MediaError::Invalid {
                message: format!(
                    "the frame index passed for `{}` is another file's",
                    p.display()
                ),
            });
        }
        self.shared.decoders_opened.fetch_add(1, Ordering::Relaxed);
        if let Some((width, height)) = opts.target_size.filter(|(w, h)| *w > 0 && *h > 0) {
            video.width = width;
            video.height = height;
        }
        let hw = self.pseudo_hw && opts.hw == crate::HwMode::Auto;
        if hw {
            self.shared
                .hw_decoders_opened
                .fetch_add(1, Ordering::Relaxed);
        }
        Ok(Box::new(FakeVideoDecoder {
            hw,
            format: opts.target_pixfmt.unwrap_or(video.format),
            video,
            index,
            next: 0,
            cost: self.frame_cost,
            cancel: None,
            shared: Arc::clone(&self.shared),
        }))
    }

    fn open_audio_decoder(
        &self,
        p: &Path,
        stream: Option<u32>,
    ) -> Result<Box<dyn AudioDecoder>, MediaError> {
        let media = self.lookup(p)?;
        let no_stream = || MediaError::NoStream {
            path: path::display(p),
            kind: StreamKind::Audio,
            index: stream,
        };
        let audio = media.audio.clone().ok_or_else(no_stream)?;
        if stream.is_some_and(|i| i != media.audio_index()) {
            return Err(no_stream());
        }
        Ok(Box::new(FakeAudioDecoder {
            audio,
            cancel: None,
        }))
    }

    fn open_encoder(&self, spec: &EncodeSpec) -> Result<Box<dyn Encoder>, MediaError> {
        spec.validate()?;
        let codecs = spec
            .video
            .iter()
            .map(|v| &v.codec)
            .chain(spec.audio.iter().map(|a| &a.codec));
        for codec in codecs {
            if !self.caps.can_encode(codec) {
                return Err(MediaError::Unsupported {
                    message: format!("no encoder for `{codec}`"),
                });
            }
        }
        // No hardware here: `Auto` encodes in "software", a forced hardware encoder is missing.
        if let (Some(_), crate::HwEncode::Force(name)) = (&spec.video, &spec.hw) {
            return Err(MediaError::HwUnavailable {
                reason: format!("the fake backend has no hardware encoder `{name}`"),
            });
        }
        Ok(Box::new(FakeEncoder {
            input: InputCheck::new(spec),
            spec: spec.clone(),
            bytes: 0,
            cancelled: false,
            backend: self.clone(),
        }))
    }

    fn concat(&self, spec: &ConcatSpec) -> Result<EncodeSummary, MediaError> {
        spec.validate()?;
        let mut joined: Option<FakeMedia> = None;
        for part in &spec.parts {
            if spec.cancel.as_ref().is_some_and(CancelCheck::is_cancelled) {
                return Err(MediaError::Cancelled);
            }
            let media = self.lookup(part)?;
            joined = Some(match joined {
                None => media,
                Some(mut j) => {
                    let same_video = match (&j.video, &media.video) {
                        (Some(a), Some(b)) => {
                            (a.width, a.height, a.rate, a.format)
                                == (b.width, b.height, b.rate, b.format)
                        }
                        (None, None) => true,
                        _ => false,
                    };
                    let same_audio = match (&j.audio, &media.audio) {
                        (Some(a), Some(b)) => (a.rate, a.channels) == (b.rate, b.channels),
                        (None, None) => true,
                        _ => false,
                    };
                    if !same_video || !same_audio {
                        return Err(MediaError::Invalid {
                            message: format!(
                                "`{}` does not have the streams of the first part",
                                path::display(part)
                            ),
                        });
                    }
                    if let (Some(a), Some(b)) = (&mut j.video, &media.video) {
                        a.frames += b.frames;
                    }
                    if let (Some(a), Some(b)) = (&mut j.audio, &media.audio) {
                        a.samples += b.samples;
                    }
                    j
                }
            });
        }
        let joined = joined.ok_or_else(|| MediaError::Invalid {
            message: "nothing to join".to_owned(),
        })?;
        let video_frames = joined.video.as_ref().map_or(0, |v| v.frames);
        let audio_samples = joined.audio.as_ref().map_or(0, |a| a.samples);
        let duration = joined.probe()?.duration.unwrap_or(Duration::ZERO);
        self.store_output(&spec.path, joined)?;
        Ok(EncodeSummary {
            video_frames,
            audio_samples,
            duration,
        })
    }

    fn capabilities(&self) -> Capabilities {
        self.caps.clone()
    }
}

struct FakeVideoDecoder {
    /// A pretend hardware decoder.
    hw: bool,
    video: FakeVideo,
    format: PixelFormat,
    index: Arc<FrameIndex>,
    /// Index of the frame `next_frame` returns.
    next: u64,
    /// Wall time per decoded frame ([`FakeBackend::with_frame_cost`]).
    cost: Wall,
    cancel: Option<CancelToken>,
    shared: Arc<Shared>,
}

impl FakeVideoDecoder {
    /// Spends `frames × cost` like a decoder would, polling the cancel token at least every
    /// millisecond.
    fn work(&self, frames: u64) -> Result<(), MediaError> {
        cancel::check(self.cancel.as_ref())?;
        if self.cost.is_zero() {
            return Ok(());
        }
        let total = self
            .cost
            .saturating_mul(u32::try_from(frames).unwrap_or(u32::MAX));
        let start = Instant::now();
        loop {
            let left = total.saturating_sub(start.elapsed());
            if left.is_zero() {
                return Ok(());
            }
            std::thread::sleep(left.min(Wall::from_millis(1)));
            cancel::check(self.cancel.as_ref())?;
        }
    }

    fn emit(&mut self, n: u64) -> Result<VideoFrame, MediaError> {
        let frame = self.video.frame(n, self.format)?;
        self.next = n + 1;
        self.shared.decoded_frames.fetch_add(1, Ordering::Relaxed);
        Ok(frame)
    }
}

impl VideoDecoder for FakeVideoDecoder {
    fn next_frame(&mut self) -> Result<Option<VideoFrame>, MediaError> {
        cancel::check(self.cancel.as_ref())?;
        if self.next >= self.video.frames {
            return Ok(None);
        }
        self.work(1)?;
        self.emit(self.next).map(Some)
    }

    fn seek_exact(&mut self, t: Time) -> Result<VideoFrame, MediaError> {
        cancel::check(self.cancel.as_ref())?;
        if self.video.frames == 0 || t >= Time::from(self.video.duration()?) {
            return Err(out_of_range(t));
        }
        let n = u64::try_from(self.video.rate.time_to_frame(t, Rounding::Floor))
            .unwrap_or(0)
            .min(self.video.frames - 1);
        // Decode forward from the keyframe, or from where the decoder is when that is on the
        // way (the same frame again costs nothing).
        let key = n - n % self.video.gop.max(1);
        let from = if (key..=n + 1).contains(&self.next) {
            self.next
        } else {
            key
        };
        self.work((n + 1).saturating_sub(from))?;
        self.emit(n)
    }

    fn index(&self) -> Arc<FrameIndex> {
        Arc::clone(&self.index)
    }

    fn set_cancel(&mut self, cancel: Option<CancelToken>) {
        self.cancel = cancel;
    }

    fn is_hw(&self) -> bool {
        self.hw
    }
}

struct FakeAudioDecoder {
    audio: FakeAudio,
    cancel: Option<CancelToken>,
}

impl AudioDecoder for FakeAudioDecoder {
    fn format(&self) -> AudioFormat {
        self.audio.format()
    }

    fn set_cancel(&mut self, cancel: Option<CancelToken>) {
        self.cancel = cancel;
    }

    fn decode_range(&mut self, start: Time, dur: Duration) -> Result<AudioChunk, MediaError> {
        cancel::check(self.cancel.as_ref())?;
        let rate = self.audio.rate;
        let index = |t: Time| {
            let n = first_unit_at_or_after(
                t,
                |t| rate.time_to_samples(t, Rounding::Floor),
                |n| rate.samples_to_time(n).ok(),
            );
            u64::try_from(n).unwrap_or(0).min(self.audio.samples)
        };
        let first = index(start);
        let last = index(start.saturating_add(dur));
        let pts = rate
            .samples_to_time(i64::try_from(first).unwrap_or(i64::MAX))
            .map_err(|_| out_of_range(start))?;
        let planes = (0..self.audio.channels)
            .map(|c| (first..last).map(|n| self.audio.sample(c, n)).collect())
            .collect();
        Ok(AudioChunk {
            pts,
            rate,
            channels: self.audio.channels,
            planes,
        })
    }
}

/// Checks its input like a real encoder (the shared `InputCheck`), honours the spec's cancel
/// check and progress sink, and writes nothing: its "bytes" are the raw sizes pushed.
struct FakeEncoder {
    spec: EncodeSpec,
    input: InputCheck,
    bytes: u64,
    cancelled: bool,
    backend: FakeBackend,
}

impl FakeEncoder {
    fn check_cancel(&mut self) -> Result<(), MediaError> {
        if self.cancelled
            || self
                .spec
                .cancel
                .as_ref()
                .is_some_and(CancelCheck::is_cancelled)
        {
            self.cancelled = true;
            return Err(MediaError::Cancelled);
        }
        Ok(())
    }

    fn report(&self) {
        if let Some(sink) = &self.spec.progress {
            sink.report(
                &self
                    .input
                    .progress(self.input.summary().duration, self.bytes),
            );
        }
    }
}

impl Encoder for FakeEncoder {
    fn push_video(&mut self, frame: &VideoFrame) -> Result<(), MediaError> {
        self.check_cancel()?;
        self.input.video(frame)?;
        self.input.add_video();
        self.bytes += frame
            .planes
            .iter()
            .map(|p| p.data.len() as u64)
            .sum::<u64>();
        self.report();
        Ok(())
    }

    fn push_audio(&mut self, chunk: &AudioChunk) -> Result<(), MediaError> {
        self.check_cancel()?;
        self.input.audio(chunk)?;
        self.input.add_audio(chunk.frames());
        self.bytes += 4 * chunk.frames() as u64 * u64::from(chunk.channels);
        self.report();
        Ok(())
    }

    fn finish(mut self: Box<Self>) -> Result<EncodeSummary, MediaError> {
        self.check_cancel()?;
        let summary = self.input.summary();
        self.report();
        self.backend
            .shared
            .encodes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((self.spec.path.clone(), summary));
        // The output reads back as synthetic media of the same shape (an H.264 file decodes to
        // 4:2:0 in the output's color).
        let written = FakeMedia {
            video: self.spec.video.as_ref().map(|v| FakeVideo {
                width: v.width,
                height: v.height,
                rate: v.rate,
                frames: summary.video_frames,
                format: PixelFormat::Yuv420p,
                color: output_color(&v.color),
                rotation: 0,
                gop: FAKE_GOP_FRAMES,
            }),
            audio: self.spec.audio.as_ref().map(|a| FakeAudio {
                rate: a.rate,
                channels: a.channels,
                samples: summary.audio_samples,
                frequency_hz: 440.0,
                amplitude: 0.5,
            }),
        };
        self.backend.store_output(&self.spec.path, written)?;
        Ok(summary)
    }
}

/// The smallest unit index whose start time is at or after `t` (0 for `t ≤ 0`).
fn first_unit_at_or_after(
    t: Time,
    floor: impl Fn(Time) -> i64,
    start_of: impl Fn(i64) -> Option<Time>,
) -> i64 {
    if t <= Time::ZERO {
        return 0;
    }
    let n = floor(t);
    if start_of(n).is_some_and(|s| s >= t) {
        n
    } else {
        n + 1
    }
}

fn out_of_range(t: Time) -> MediaError {
    MediaError::OutOfRange { t }
}

/// The planes of fake frame `n` (see the module docs).
// The pattern and the label are intentionally modulo 256 / 2^32.
#[allow(clippy::cast_possible_truncation)]
fn fake_planes(width: u32, height: u32, format: PixelFormat, n: u64) -> Vec<Plane> {
    let mut planes: Vec<Plane> = (0..format.plane_count())
        .map(|i| {
            let (w, h, bpt) = format.plane_layout(i, width, height);
            let stride = w * bpt;
            let mut data = Vec::with_capacity((stride * h) as usize);
            for y in 0..h {
                for x in 0..w {
                    for k in 0..bpt {
                        let v = (u64::from(x)
                            + 2 * u64::from(y)
                            + 3 * n
                            + 17 * i as u64
                            + 5 * u64::from(k)) as u8;
                        data.push(match format {
                            PixelFormat::P010 if k % 2 == 0 => 0, // low 6 bits of the 16-bit word
                            // High byte of a low-aligned 10-bit sample: values stay below 1024.
                            PixelFormat::Yuv420p10 | PixelFormat::Yuv422p10 if k % 2 == 1 => {
                                v & 0x03
                            }
                            PixelFormat::Rgba8 if k == 3 => 0xff, // opaque
                            _ => v,
                        });
                    }
                }
            }
            Plane { data, stride }
        })
        .collect();
    // The frame number in the first four samples of row 0 of plane 0.
    let label = (n as u32).to_le_bytes();
    let row_bytes = (width * format.plane_layout(0, width, height).2) as usize;
    if let Some(p0) = planes.first_mut() {
        for (j, byte) in label.iter().enumerate() {
            let at = sample_byte(format, j);
            if at < row_bytes
                && let Some(slot) = p0.data.get_mut(at)
            {
                *slot = *byte;
            }
        }
    }
    planes
}

/// Offset in row 0 of plane 0 of the byte that carries label byte `j`.
fn sample_byte(format: PixelFormat, j: usize) -> usize {
    match format {
        PixelFormat::Nv12 | PixelFormat::Yuv420p | PixelFormat::Yuv422p => j,
        // The low byte of a low-aligned 10-bit sample (the label stays a valid sample).
        PixelFormat::Yuv420p10 | PixelFormat::Yuv422p10 => 2 * j,
        PixelFormat::P010 => 2 * j + 1,
        PixelFormat::Rgba8 => 4 * j,
    }
}

/// The frame number a [`FakeBackend`] frame carries in its pixels, or `None` if the frame is
/// narrower than 4 pixels.
#[must_use]
pub fn fake_frame_number(frame: &VideoFrame) -> Option<u32> {
    let row = frame.row(0, 0)?;
    let mut bytes = [0u8; 4];
    for (j, b) in bytes.iter_mut().enumerate() {
        *b = *row.get(sample_byte(frame.format, j))?;
    }
    Some(u32::from_le_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_format_carries_its_frame_number() {
        for format in PixelFormat::ALL {
            let v = FakeVideo::new(7, 5, FrameRate::FPS_25, 300_000).with_format(format);
            for n in [0, 1, 255, 256, 299_999] {
                let f = v.frame(n, format).unwrap();
                assert_eq!(f.validate(), Ok(()), "{format:?}");
                assert_eq!(
                    fake_frame_number(&f),
                    u32::try_from(n).ok(),
                    "{format:?} {n}"
                );
            }
        }
        let narrow = FakeVideo::new(2, 2, FrameRate::FPS_25, 1).frame(0, PixelFormat::Nv12);
        assert_eq!(fake_frame_number(&narrow.unwrap()), None);
    }

    #[test]
    fn frames_differ_and_repeat_deterministically() {
        let v = FakeVideo::new(16, 8, FrameRate::FPS_30, 3);
        let a = v.frame(1, PixelFormat::Yuv420p).unwrap();
        assert_eq!(a, v.frame(1, PixelFormat::Yuv420p).unwrap());
        assert_ne!(a.planes, v.frame(2, PixelFormat::Yuv420p).unwrap().planes);
    }

    #[test]
    fn lengths_cover_the_duration() {
        let ten = Duration::from_secs(10).unwrap();
        assert_eq!(FakeVideo::lasting(8, 8, FrameRate::FPS_30, ten).frames, 300);
        assert_eq!(
            FakeVideo::lasting(8, 8, FrameRate::FPS_29_97, ten).frames,
            300
        );
        assert_eq!(
            FakeAudio::tone(SampleRate::HZ_44_100, 1, ten).samples,
            441_000
        );
        let short = Duration::from_flicks(1).unwrap();
        assert_eq!(FakeVideo::lasting(8, 8, FrameRate::FPS_30, short).frames, 1);
        assert_eq!(
            FakeVideo::lasting(8, 8, FrameRate::FPS_30, Duration::ZERO).frames,
            0
        );
    }

    #[test]
    fn decoders_scale_and_joins_add_up_and_files_survive_a_new_backend() {
        use crate::testing::contract::TempDir;
        let dir = TempDir::new("fake-proxy-io");
        let src = dir.path().join("src.mov");
        let backend = FakeBackend::new()
            .with_media(
                &src,
                FakeMedia::video(FakeVideo::new(64, 36, FrameRate::FPS_25, 30)),
            )
            .with_output_files();
        // Scaled decode keeps the frame numbers.
        let opts = DecodeOpts::default().with_size(16, 8);
        let mut d = backend.open_decoder(&src, &opts).expect("open");
        let f = d
            .seek_exact(FrameRate::FPS_25.frame_to_time(7).expect("t"))
            .expect("seek");
        assert_eq!((f.width, f.height), (16, 8));
        assert_eq!(fake_frame_number(&f), Some(7));
        // Two encodes, joined: the files describe their media and read back elsewhere.
        let mut parts = Vec::new();
        for (i, frames) in [(0, 4_i64), (1, 6)] {
            let path = dir.path().join(format!("part{i}.mov"));
            let spec = EncodeSpec::new(&path).with_video(crate::VideoEncodeSpec::h264(
                16,
                8,
                FrameRate::FPS_25,
                PixelFormat::Yuv420p,
            ));
            let video = FakeVideo::new(16, 8, FrameRate::FPS_25, 10);
            let mut enc = FakeBackend::new()
                .with_capabilities(Capabilities::with_codecs([CodecSupport {
                    name: "h264".into(),
                    kind: StreamKind::Video,
                    decoders: vec![],
                    encoders: vec!["h264".into()],
                }]))
                .with_output_files()
                .open_encoder(&spec)
                .expect("encoder");
            for n in 0..frames {
                let mut frame = video
                    .frame(u64::try_from(n).expect("n"), PixelFormat::Yuv420p)
                    .expect("frame");
                frame.pts = FrameRate::FPS_25.frame_to_time(n).expect("pts");
                enc.push_video(&frame).expect("push");
            }
            enc.finish().expect("finish");
            assert!(path.is_file());
            parts.push(path);
        }
        let out = dir.path().join("joined.mov");
        let summary = backend.concat(&ConcatSpec::new(parts, &out)).expect("join");
        assert_eq!(summary.video_frames, 10);
        let fresh = FakeBackend::new();
        assert_eq!(fresh.index_media(&out).expect("index").frame_count(), 10);
        let cancelled = backend.concat(
            &ConcatSpec::new(vec![out.clone()], dir.path().join("c.mov"))
                .with_cancel(CancelCheck::new(|| true)),
        );
        assert_eq!(cancelled, Err(MediaError::Cancelled));
        assert!(!dir.path().join("c.mov").exists());
    }

    #[test]
    fn unit_rounding_is_ceil_to_the_grid() {
        let rate = SampleRate::HZ_48_000;
        let floor = |t| rate.time_to_samples(t, Rounding::Floor);
        let start = |n| rate.samples_to_time(n).ok();
        let one = rate.samples_to_time(1).unwrap();
        assert_eq!(first_unit_at_or_after(Time::ZERO, floor, start), 0);
        assert_eq!(first_unit_at_or_after(one, floor, start), 1);
        let just_after = Time::from_flicks(one.flicks() + 1).unwrap();
        assert_eq!(first_unit_at_or_after(just_after, floor, start), 2);
        assert_eq!(
            first_unit_at_or_after(Time::from_flicks(-5).unwrap(), floor, start),
            0
        );
    }
}
