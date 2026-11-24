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

use cutweave_model::{Duration, FrameRate, ProbeInfo, SampleRate, Time};

use crate::{
    AudioChunk, AudioFormat, Capabilities, FrameIndex, MediaError, PixelFormat, VideoFrame,
};

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
    /// Frame rate of the output.
    pub rate: FrameRate,
    /// Layout of the frames that will be pushed.
    pub input_format: PixelFormat,
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
}

/// What to encode and where. Codec settings (CRF, preset, bitrate) arrive with WP-1E.01.
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
}

impl EncodeSpec {
    /// Checks what every backend requires before opening an output: at least one stream,
    /// non-empty codec names, a non-empty video size, at least one audio channel and a syntactically
    /// valid path. Backends call it first, so they reject bad specs the same way.
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
        }
        if let Some(a) = &self.audio {
            if a.codec.is_empty() {
                return invalid("the audio codec name is empty");
            }
            if a.channels == 0 {
                return invalid("the audio stream has no channels");
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

    /// The frame index of the main video stream of `path`: a packet scan, no decoding (arch §7).
    /// Time-consuming for long files (it reads the whole container), so run it on a worker
    /// thread.
    ///
    /// Errors as for [`probe`](Self::probe), plus [`MediaError::NoStream`] for a file without
    /// pictures and [`MediaError::Corrupt`] for a stream whose packets carry no usable
    /// timestamps or no keyframe.
    fn index_media(&self, path: &Path) -> Result<FrameIndex, MediaError>;

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
pub trait Encoder: Send {
    /// Appends a video frame. Frames must match the spec and have increasing pts.
    fn push_video(&mut self, frame: &VideoFrame) -> Result<(), MediaError>;

    /// Appends audio. Chunks must match the spec and follow each other without gaps.
    fn push_audio(&mut self, chunk: &AudioChunk) -> Result<(), MediaError>;

    /// Flushes and closes the output.
    fn finish(self: Box<Self>) -> Result<EncodeSummary, MediaError>;
}
