// SPDX-License-Identifier: GPL-3.0-or-later
//! The [`MediaBackend`] trait (ADR-0003) and the decoder/encoder handles it opens.
//!
//! The trait is synchronous: probing and decoding run on worker threads (`cutweave-jobs`, the
//! decoder pool), never on the real-time or render threads. Backends are `Send + Sync` and are
//! shared as `Arc<dyn MediaBackend>`; each decoder or encoder handle is owned by one thread at a
//! time (`Send`).
//!
//! Implementations: [`FakeBackend`](crate::FakeBackend) (in-memory synthetic media, always
//! available) and `FfmpegBackend` (feature `ffmpeg`). Both run the same contract tests
//! ([`backend_contract!`](crate::backend_contract)).

use std::path::{Path, PathBuf};

use cutweave_model::{ColorTags, Duration, FrameRate, ProbeInfo, SampleRate, Time};

use crate::encode::{CancelCheck, ProgressSink, RateControl, SpeedPreset};
use crate::{AudioChunk, AudioFormat, Capabilities, MediaError, PixelFormat, VideoFrame};

/// Whether a decoder may use hardware acceleration.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum HwMode {
    /// Use a hardware decoder when the machine has one for the codec; fall back to software
    /// otherwise, or when hardware init or decode fails (WP-1D.04).
    #[default]
    Auto,
    /// Software only.
    Off,
}

/// How to open a video decoder.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct DecodeOpts {
    /// Hardware acceleration policy.
    pub hw: HwMode,
    /// Convert frames to this layout; `None` keeps the decoder's native layout (the GPU converts,
    /// arch §7). `Some(PixelFormat::Rgba8)` is the software-scaled thumbnail path.
    pub target_pixfmt: Option<PixelFormat>,
}

/// A video stream to encode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoEncodeSpec {
    /// Codec-id name (`h264`); the backend picks an implementation it has.
    pub codec: String,
    /// Frame width.
    pub width: u32,
    /// Frame height.
    pub height: u32,
    /// Frame rate of the output (constant). Frame `n` must have pts `rate.frame_to_time(n)`.
    pub rate: FrameRate,
    /// Layout of the frames that will be pushed. RGB frames are converted with the output's
    /// matrix and range; YUV frames must already be in them.
    pub input_format: PixelFormat,
    /// How the encoder spends bits.
    pub rate_control: RateControl,
    /// Encoder speed against compression.
    pub preset: SpeedPreset,
    /// Color of the output: written to the bitstream and the container, and used to convert RGB
    /// input. Unset fields mean BT.709 limited range ([`bt709_tags`](crate::bt709_tags)).
    pub color: ColorTags,
    /// Encoder threads; 0 lets the encoder choose. x264 output is identical run to run for a
    /// given thread count, so `1` also makes it identical across machines that load the same
    /// FFmpeg bundle (the export determinism test, WP-1E.03).
    pub threads: u16,
}

impl VideoEncodeSpec {
    /// H.264 at `width × height` and `rate` from `input_format` frames: CRF 20, preset
    /// `medium`, BT.709 limited range, automatic threads.
    #[must_use]
    pub fn h264(
        width: u32,
        height: u32,
        rate: FrameRate,
        input_format: PixelFormat,
    ) -> VideoEncodeSpec {
        VideoEncodeSpec {
            codec: "h264".to_owned(),
            width,
            height,
            rate,
            input_format,
            rate_control: RateControl::default(),
            preset: SpeedPreset::default(),
            color: crate::bt709_tags(),
            threads: 0,
        }
    }
}

/// An audio stream to encode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioEncodeSpec {
    /// Codec-id name (`aac`).
    pub codec: String,
    /// Output sample rate (pushed chunks must already be at this rate).
    pub rate: SampleRate,
    /// Output channel count (pushed chunks must have this many planes).
    pub channels: u16,
    /// Target bitrate in bits per second, for all channels together.
    pub bitrate: u32,
}

impl AudioEncodeSpec {
    /// The AAC bitrate exports use unless told otherwise.
    pub const DEFAULT_BITRATE: u32 = 192_000;

    /// AAC at `rate` with `channels` channels and 192 kb/s.
    #[must_use]
    pub fn aac(rate: SampleRate, channels: u16) -> AudioEncodeSpec {
        AudioEncodeSpec {
            codec: "aac".to_owned(),
            rate,
            channels,
            bitrate: AudioEncodeSpec::DEFAULT_BITRATE,
        }
    }
}

/// What to encode and where, and the hooks the encoder calls while it works.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodeSpec {
    /// Output file. The container follows from the extension unless `container` is set.
    pub path: PathBuf,
    /// Container (muxer) name (`mp4`), or `None` to infer it from `path`.
    pub container: Option<String>,
    /// The video stream, if any.
    pub video: Option<VideoEncodeSpec>,
    /// The audio stream, if any.
    pub audio: Option<AudioEncodeSpec>,
    /// Polled while encoding; once it answers `true` the encoder stops with
    /// [`MediaError::Cancelled`] and removes its partial output.
    pub cancel: Option<CancelCheck>,
    /// Receives progress while encoding.
    pub progress: Option<ProgressSink>,
}

impl EncodeSpec {
    /// An output at `path` with no streams and no hooks yet.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> EncodeSpec {
        EncodeSpec {
            path: path.into(),
            container: None,
            video: None,
            audio: None,
            cancel: None,
            progress: None,
        }
    }

    /// The same output with a video stream.
    #[must_use]
    pub fn with_video(mut self, video: VideoEncodeSpec) -> EncodeSpec {
        self.video = Some(video);
        self
    }

    /// The same output with an audio stream.
    #[must_use]
    pub fn with_audio(mut self, audio: AudioEncodeSpec) -> EncodeSpec {
        self.audio = Some(audio);
        self
    }

    /// The same output, cancelled by `cancel`.
    #[must_use]
    pub fn with_cancel(mut self, cancel: CancelCheck) -> EncodeSpec {
        self.cancel = Some(cancel);
        self
    }

    /// The same output, reporting to `progress`.
    #[must_use]
    pub fn with_progress(mut self, progress: ProgressSink) -> EncodeSpec {
        self.progress = Some(progress);
        self
    }

    /// Checks what every backend requires before opening an output: at least one stream,
    /// non-empty codec names, a non-empty video size, a CRF on the 0–51 scale, non-zero
    /// bitrates, at least one audio channel and a syntactically valid path. Backends call it
    /// first, so they reject bad specs the same way.
    pub fn validate(&self) -> Result<(), MediaError> {
        let invalid = |message: &str| {
            Err(MediaError::Invalid {
                message: message.to_owned(),
            })
        };
        crate::path::check_syntax(&self.path)?;
        if self.video.is_none() && self.audio.is_none() {
            return invalid("an encode spec needs a video or an audio stream");
        }
        if let Some(v) = &self.video {
            if v.codec.is_empty() {
                return invalid("the video codec name is empty");
            }
            if v.width == 0 || v.height == 0 {
                return invalid("the video size is empty");
            }
            match v.rate_control {
                RateControl::Crf(q) if q > RateControl::MAX_CRF => {
                    return invalid("the CRF is above 51");
                }
                RateControl::Bitrate(0) => return invalid("the video bitrate is zero"),
                _ => {}
            }
        }
        if let Some(a) = &self.audio {
            if a.codec.is_empty() {
                return invalid("the audio codec name is empty");
            }
            if a.channels == 0 {
                return invalid("the audio stream has no channels");
            }
            if a.bitrate == 0 {
                return invalid("the audio bitrate is zero");
            }
        }
        Ok(())
    }
}

/// What an encoder wrote.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EncodeSummary {
    /// Video frames written.
    pub video_frames: u64,
    /// Audio samples written per channel.
    pub audio_samples: u64,
    /// Duration of the output (end of the last frame or sample).
    pub duration: Duration,
}

/// A media library behind one interface (arch §7, ADR-0003).
pub trait MediaBackend: Send + Sync {
    /// Streams, duration, frame rate, rotation and color tags of the file at `path`.
    ///
    /// Errors: [`MediaError::NotFound`], [`MediaError::InvalidPath`], [`MediaError::Corrupt`],
    /// [`MediaError::Unsupported`], [`MediaError::Io`]. Never panics on bad input.
    fn probe(&self, path: &Path) -> Result<ProbeInfo, MediaError>;

    /// Opens a decoder for the main video stream of `path`.
    fn open_decoder(
        &self,
        path: &Path,
        opts: &DecodeOpts,
    ) -> Result<Box<dyn VideoDecoder>, MediaError>;

    /// Opens a decoder for audio stream `stream` (container index) of `path`, or for the first
    /// audio stream when `None`.
    fn open_audio_decoder(
        &self,
        path: &Path,
        stream: Option<u32>,
    ) -> Result<Box<dyn AudioDecoder>, MediaError>;

    /// Creates the output file described by `spec`.
    fn open_encoder(&self, spec: &EncodeSpec) -> Result<Box<dyn Encoder>, MediaError>;

    /// What this backend can decode and encode on this machine. Cheap: backends compute it once.
    fn capabilities(&self) -> Capabilities;
}

/// A video decoder on one stream. Frames come back in presentation order with their exact
/// source pts.
pub trait VideoDecoder: Send {
    /// The next frame in presentation order, or `None` at the end of the stream.
    fn next_frame(&mut self) -> Result<Option<VideoFrame>, MediaError>;

    /// The frame on screen at media time `t`: the last frame whose pts is at or before `t` (the
    /// first frame for a `t` before it). Subsequent [`next_frame`](Self::next_frame) calls
    /// continue after it. [`MediaError::OutOfRange`] past the end of the stream.
    fn seek_exact(&mut self, t: Time) -> Result<VideoFrame, MediaError>;

    /// Whether frames come from a hardware decoder (WP-1D.04).
    fn is_hw(&self) -> bool {
        false
    }
}

/// An audio decoder on one stream, at the stream's native rate and channel count.
pub trait AudioDecoder: Send {
    /// The decoded format.
    fn format(&self) -> AudioFormat;

    /// The samples whose start times fall in `[start, start + dur)`, as one chunk whose `pts` is
    /// the first such sample's time. Ranges past the end are cut short (possibly to zero
    /// samples); consecutive ranges join without gaps or overlaps.
    fn decode_range(&mut self, start: Time, dur: Duration) -> Result<AudioChunk, MediaError>;
}

/// An encoder writing one output file.
///
/// The file appears at the spec's path only when [`finish`](Self::finish) succeeds (an existing
/// file there is replaced then). Cancelling ([`EncodeSpec::cancel`]), a failure and dropping the
/// encoder before `finish` remove the partial output and leave an existing file untouched.
///
/// **Input.** Video frames match the stream's size and layout, and frame `n` has pts
/// `rate.frame_to_time(n)`: frames start at zero and follow each other on the output's frame
/// grid (constant frame rate; flicks become the codec's time base only inside the encoder, arch
/// §4.1). YUV frames carry no matrix or range tag that contradicts the output's. Audio chunks
/// match the stream's rate and channel count and continue it without gaps from sample zero.
/// Push audio and video in step, a second or so apart at most: the muxer interleaves them, and
/// buffers whatever runs ahead. Every backend rejects other input with
/// [`MediaError::Invalid`] and then accepts the next valid frame or chunk.
pub trait Encoder: Send {
    /// Appends video frame `n`: it matches the spec and has pts `rate.frame_to_time(n)`.
    fn push_video(&mut self, frame: &VideoFrame) -> Result<(), MediaError>;

    /// Appends audio. Chunks must match the spec and follow each other without gaps from 0.
    fn push_audio(&mut self, chunk: &AudioChunk) -> Result<(), MediaError>;

    /// Flushes and closes the output.
    fn finish(self: Box<Self>) -> Result<EncodeSummary, MediaError>;
}
