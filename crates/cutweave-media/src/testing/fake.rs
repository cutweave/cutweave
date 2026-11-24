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

use std::collections::BTreeMap;
use std::f64::consts::TAU;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use cutweave_model::{
    AudioInfo, ColorTags, Duration, Extra, FrameRate, ProbeInfo, Rounding, SampleRate, StreamInfo,
    StreamKind, Time,
};

use crate::backend::{
    AudioDecoder, DecodeOpts, EncodeSpec, EncodeSummary, Encoder, MediaBackend, VideoDecoder,
};
use crate::encode::InputCheck;
use crate::path;
use crate::{
    AudioChunk, AudioFormat, CancelCheck, Capabilities, CodecSupport, FrameIndex, IndexEntry,
    MediaError, PixelFormat, Plane, ScanInfo, TimeBase, VideoFrame,
};

/// Codec name the fake reports for its pictures.
pub const FAKE_VIDEO_CODEC: &str = "rawvideo";
/// Codec name the fake reports for its tones.
pub const FAKE_AUDIO_CODEC: &str = "pcm_f32le";

/// Frames per group of pictures in the fake's index (a keyframe every this many frames).
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
    /// [`TimeBase::FLICKS`]), a keyframe every [`FAKE_GOP_FRAMES`] frames.
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
                    is_key: n % FAKE_GOP_FRAMES == 0,
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
    encodes: Mutex<Vec<(PathBuf, EncodeSummary)>>,
}

/// A [`MediaBackend`] serving registered in-memory media (see the module docs).
///
/// Clones share their counters and encode log.
#[derive(Clone, Debug)]
pub struct FakeBackend {
    media: BTreeMap<PathBuf, Entry>,
    caps: Capabilities,
    shared: Arc<Shared>,
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
            shared: Arc::default(),
        }
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

    fn lookup(&self, p: &Path) -> Result<&FakeMedia, MediaError> {
        path::check_syntax(p)?;
        match self.media.get(p) {
            Some(Entry::Media(m)) => Ok(m),
            Some(Entry::Error(e)) => Err(e.clone()),
            None => {
                path::check_file(p)?;
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
        self.lookup(path)?.index(path)
    }

    fn open_decoder(
        &self,
        p: &Path,
        opts: &DecodeOpts,
    ) -> Result<Box<dyn VideoDecoder>, MediaError> {
        let media = self.lookup(p)?;
        let video = media.video.clone().ok_or_else(|| MediaError::NoStream {
            path: path::display(p),
            kind: StreamKind::Video,
            index: None,
        })?;
        Ok(Box::new(FakeVideoDecoder {
            format: opts.target_pixfmt.unwrap_or(video.format),
            video,
            next: 0,
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
        Ok(Box::new(FakeAudioDecoder { audio }))
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
            shared: Arc::clone(&self.shared),
        }))
    }

    fn capabilities(&self) -> Capabilities {
        self.caps.clone()
    }
}

struct FakeVideoDecoder {
    video: FakeVideo,
    format: PixelFormat,
    /// Index of the frame `next_frame` returns.
    next: u64,
    shared: Arc<Shared>,
}

impl FakeVideoDecoder {
    fn emit(&mut self, n: u64) -> Result<VideoFrame, MediaError> {
        let frame = self.video.frame(n, self.format)?;
        self.next = n + 1;
        self.shared.decoded_frames.fetch_add(1, Ordering::Relaxed);
        Ok(frame)
    }
}

impl VideoDecoder for FakeVideoDecoder {
    fn next_frame(&mut self) -> Result<Option<VideoFrame>, MediaError> {
        if self.next >= self.video.frames {
            return Ok(None);
        }
        self.emit(self.next).map(Some)
    }

    fn seek_exact(&mut self, t: Time) -> Result<VideoFrame, MediaError> {
        if self.video.frames == 0 || t >= Time::from(self.video.duration()?) {
            return Err(out_of_range(t));
        }
        let n = u64::try_from(self.video.rate.time_to_frame(t, Rounding::Floor)).unwrap_or(0);
        self.emit(n.min(self.video.frames - 1))
    }
}

struct FakeAudioDecoder {
    audio: FakeAudio,
}

impl AudioDecoder for FakeAudioDecoder {
    fn format(&self) -> AudioFormat {
        self.audio.format()
    }

    fn decode_range(&mut self, start: Time, dur: Duration) -> Result<AudioChunk, MediaError> {
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
    shared: Arc<Shared>,
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
        self.shared
            .encodes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((self.spec.path.clone(), summary));
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
        PixelFormat::Nv12 | PixelFormat::Yuv420p => j,
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
        for format in [
            PixelFormat::Nv12,
            PixelFormat::P010,
            PixelFormat::Yuv420p,
            PixelFormat::Rgba8,
        ] {
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
