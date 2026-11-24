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
use std::sync::Arc;

use cutweave_model::{ColorTags, Duration, FrameRate, ProbeInfo, SampleRate, Time};

use crate::encode::{CancelCheck, ProgressSink, RateControl, SpeedPreset};
use crate::{
    AudioChunk, AudioFormat, CancelToken, Capabilities, FrameIndex, MediaError, PixelFormat,
    VideoFrame,
};

/// Whether a decoder may use hardware acceleration (WP-1D.04).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum HwMode {
    /// Use a hardware decoder when the machine has a verified one for the stream (listed in
    /// [`Capabilities::hw_decode`](crate::Capabilities::hw_decode)); software otherwise. With
    /// `FfmpegBackend`, the eligible streams are long-GOP H.264 (8-bit 4:2:0) and HEVC (8/10-bit
    /// 4:2:0) of at least 144 pixels each way, and `CUTWEAVE_HW=off` turns hardware decode off
    /// for the process. A hardware decoder that fails to initialise or while decoding goes over
    /// to software at once, transparently ([`VideoDecoder::hw_failed`]).
    #[default]
    Auto,
    /// Software only.
    Off,
}

/// Whether an encoder may use a hardware video encoder (WP-1E.02).
///
/// The backend lists the hardware encoders that work on this machine in
/// [`Capabilities::hw_encode`](crate::Capabilities::hw_encode), in the order `Auto` tries them
/// (per OS: docs/reference/export-quality.md). Audio is always encoded in software.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum HwEncode {
    /// The first hardware encoder that opens for the stream, else the software encoder. An encoder
    /// that fails to open is skipped silently (logged); one that fails after frames were pushed
    /// ends the encode with [`MediaError::HwUnavailable`], and the caller re-encodes from the
    /// start with `Off` ([`encode_with_fallback`](crate::encode_with_fallback) does this).
    #[default]
    Auto,
    /// Software only (x264). Its output is identical run to run for a given thread count.
    Off,
    /// This hardware encoder (an [`HwCodec::implementation`](crate::HwCodec::implementation)
    /// name such as `h264_nvenc`) and no other: when it cannot open or fails later, the encode
    /// fails with [`MediaError::HwUnavailable`] and nothing falls back.
    Force(String),
}

/// What a video decoder is for: it picks the threading (ADR-0003 decoder pool, S3 finding F4).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DecodeMode {
    /// Random access and scrubbing ([`VideoDecoder::seek_exact`]): frame threads for long-GOP
    /// streams, slice threads for intra-only ones (frame threads cost intra codecs about 15×
    /// on a seek, S3).
    #[default]
    Seek,
    /// Reading frames in order ([`VideoDecoder::next_frame`]: playback, export, filmstrips):
    /// frame threads, for throughput.
    Sequential,
}

/// How to open a video decoder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DecodeOpts {
    /// Hardware acceleration policy ([`HwMode::Auto`] unless set). Hardware decoders return
    /// `Nv12` / `P010` where software returns `Yuv420p` / `Yuv420p10` for the same stream, so
    /// frames of one file can arrive in either layout.
    pub hw: HwMode,
    /// Convert frames to this layout; `None` keeps the decoder's native layout (the GPU converts,
    /// arch §7; layouts outside [`PixelFormat`] are converted to the nearest one).
    /// `Some(PixelFormat::Rgba8)` is the software-scaled thumbnail path.
    pub target_pixfmt: Option<PixelFormat>,
    /// What the decoder is for (threading).
    pub mode: DecodeMode,
    /// Decoder threads; 0 lets the backend choose (FFmpeg: cores + 1, at most 16).
    pub threads: u16,
    /// The file's frame index ([`MediaBackend::index_media`], or the copy in `cache.sqlite`).
    /// `None` makes the decoder build it on open, which reads the whole file: pass it whenever
    /// it is known ([`DecoderPool`](crate::DecoderPool) does after its first decoder).
    pub index: Option<Arc<FrameIndex>>,
}

impl DecodeOpts {
    /// The same options with a known frame index.
    #[must_use]
    pub fn with_index(mut self, index: Arc<FrameIndex>) -> DecodeOpts {
        self.index = Some(index);
        self
    }

    /// The same options converting every frame to `format`.
    #[must_use]
    pub fn with_target(mut self, format: PixelFormat) -> DecodeOpts {
        self.target_pixfmt = Some(format);
        self
    }

    /// The same options for `mode`.
    #[must_use]
    pub fn with_mode(mut self, mode: DecodeMode) -> DecodeOpts {
        self.mode = mode;
        self
    }

    /// The same options with hardware policy `hw`.
    #[must_use]
    pub fn with_hw(mut self, hw: HwMode) -> DecodeOpts {
        self.hw = hw;
        self
    }

    /// Whether decoders opened with `self` and with `other` behave the same (everything but the
    /// index, which only saves the scan).
    #[must_use]
    pub fn same_decoder(&self, other: &DecodeOpts) -> bool {
        (self.hw, self.target_pixfmt, self.mode, self.threads)
            == (other.hw, other.target_pixfmt, other.mode, other.threads)
    }
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
    /// FFmpeg bundle (the export determinism test, WP-1E.03, which also sets
    /// [`HwEncode::Off`]: hardware encoders ignore this field).
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
    /// Hardware video encoding policy ([`HwEncode::Auto`] unless set).
    pub hw: HwEncode,
}

impl EncodeSpec {
    /// An output at `path` with no streams and no hooks yet, hardware encoding on
    /// [`HwEncode::Auto`].
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> EncodeSpec {
        EncodeSpec {
            path: path.into(),
            container: None,
            video: None,
            audio: None,
            cancel: None,
            progress: None,
            hw: HwEncode::Auto,
        }
    }

    /// The same output with hardware encoding policy `hw`.
    #[must_use]
    pub fn with_hw(mut self, hw: HwEncode) -> EncodeSpec {
        self.hw = hw;
        self
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
    /// bitrates, at least one audio channel, a non-empty name in [`HwEncode::Force`] and a
    /// syntactically valid path. Backends call it first, so they reject bad specs the same way.
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
            if matches!(&self.hw, HwEncode::Force(name) if name.trim().is_empty()) {
                return invalid("the forced hardware encoder name is empty");
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

    /// The frame index of the main video stream of `path`: a packet scan, no decoding (arch §7).
    /// Time-consuming for long files (it reads the whole container), so run it on a worker
    /// thread.
    ///
    /// Errors as for [`probe`](Self::probe), plus [`MediaError::NoStream`] for a file without
    /// pictures and [`MediaError::Corrupt`] for a stream whose packets carry no usable
    /// timestamps or no keyframe.
    fn index_media(&self, path: &Path) -> Result<FrameIndex, MediaError>;

    /// Opens a decoder for the main video stream of `path` (the stream
    /// [`index_media`](Self::index_media) indexes). Builds the frame index first unless
    /// `opts.index` has it. Callers that open several decoders on one file share them through a
    /// [`DecoderPool`](crate::DecoderPool).
    ///
    /// Errors as for [`probe`](Self::probe), plus [`MediaError::NoStream`] for a file without
    /// pictures, [`MediaError::Unsupported`] for a codec the backend cannot decode and
    /// [`MediaError::Invalid`] for an `opts.index` that belongs to another file.
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

/// A video decoder on the main video stream of a file. Frames come back in presentation order
/// with their exact source pts (the index's, mapped to flicks), in coded orientation (the probe's
/// `rotation` says how to display them) and at their coded size (odd sizes included).
///
/// **Frame-exact seek** (ADR-0003): [`seek_exact`](Self::seek_exact) finds the frame in the
/// [`FrameIndex`], starts decoding at the keyframe the index names for it (open-GOP leading
/// pictures start one GOP earlier), and decodes forward; it continues from where the decoder is
/// instead when that is on the way (scrubbing forward). The result is bit-identical to decoding
/// the file from the start.
///
/// **Damage** never blocks: a frame the decoder cannot produce resolves to the nearest following
/// frame (else the nearest preceding one), flagged [`VideoFrame::damaged`].
///
/// **Cancellation**: with a token set ([`set_cancel`](Self::set_cancel)) every call polls it
/// between packets and returns [`MediaError::Cancelled`] once it is cancelled. The decoder stays
/// usable; where `next_frame` continues after a cancelled call is unspecified until the next
/// seek.
pub trait VideoDecoder: Send {
    /// The next frame in presentation order, or `None` at the end of the stream.
    fn next_frame(&mut self) -> Result<Option<VideoFrame>, MediaError>;

    /// The frame on screen at media time `t`: the last frame whose pts is at or before `t` (the
    /// first frame for a `t` before it). Subsequent [`next_frame`](Self::next_frame) calls
    /// continue after it. [`MediaError::OutOfRange`] at or past the end of the last frame (a
    /// stream of one picture never ends).
    fn seek_exact(&mut self, t: Time) -> Result<VideoFrame, MediaError>;

    /// The frame index the decoder seeks with (the one passed in [`DecodeOpts::index`], or the
    /// one it built on open).
    fn index(&self) -> Arc<FrameIndex>;

    /// Makes every following call poll `cancel` (`None` stops polling).
    fn set_cancel(&mut self, cancel: Option<CancelToken>);

    /// Whether frames come from a hardware decoder (WP-1D.04): `true` from open while a
    /// hardware decoder is in use, `false` in software and after a fallback.
    fn is_hw(&self) -> bool {
        false
    }

    /// Whether this decoder started on a hardware decoder that then failed (to initialise for
    /// the stream, while decoding, or copying a frame to memory), so it went over to software
    /// (WP-1D.04). The request in progress was run again in software, so no caller saw the
    /// failure; the backend logged it as a warning. Stays `true` for the decoder's life.
    /// [`DecoderPool::hw_failed`](crate::DecoderPool::hw_failed) keeps it for the media.
    fn hw_failed(&self) -> bool {
        false
    }

    /// The hardware decoder in use (`h264_nvdec`, `hevc_d3d11va`), for logs and diagnostics;
    /// `None` in software.
    fn hw_decoder(&self) -> Option<&str> {
        None
    }
}

/// An audio decoder on one stream, at the stream's native rate and channel count.
///
/// Sample `n` of the stream starts at media time `rate.samples_to_time(n)`; the decoder places
/// samples by the stream's own timestamps (priming samples the container marks are already
/// removed), so a stream that starts late starts at a later sample. A range read after a seek is
/// placed sample-exactly for containers whose audio time base is the sample rate (MP4, MOV,
/// WAV), within half a time-base tick elsewhere (Matroska: 1 ms), and holds the samples a
/// sequential read gives, except for noise that lossy codecs synthesize from decoder state (AAC
/// noise substitution: around −100 dBFS).
pub trait AudioDecoder: Send {
    /// The decoded format.
    fn format(&self) -> AudioFormat;

    /// The samples whose start times fall in `[start, start + dur)`, as one chunk whose `pts` is
    /// the first such sample's time. Ranges past the end are cut short (possibly to zero
    /// samples); consecutive ranges join without gaps or overlaps.
    fn decode_range(&mut self, start: Time, dur: Duration) -> Result<AudioChunk, MediaError>;

    /// Makes every following call poll `cancel` (`None` stops polling).
    fn set_cancel(&mut self, cancel: Option<CancelToken>);
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

    /// The implementation encoding the video stream (`libx264`, `h264_nvenc`), for logs and job
    /// reports; `None` without a video stream or when the backend does not name it. Fixed for
    /// the encoder's life: hardware fallback happens while opening, or by re-encoding
    /// ([`HwEncode::Auto`]).
    fn video_encoder(&self) -> Option<&str> {
        None
    }
}
