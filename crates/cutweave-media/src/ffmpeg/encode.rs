// SPDX-License-Identifier: GPL-3.0-or-later
//! The FFmpeg [`Encoder`]: H.264 (libx264) and AAC (FFmpeg's native encoder) in MP4 (WP-1E.01).
//!
//! - **Video.** libx264 in `yuv420p` with CRF or an average bitrate and an x264 speed preset.
//!   BT.709 primaries, transfer and matrix in limited range go into the SPS (VUI) and the MP4
//!   `colr` box. RGBA input is converted by swscale with the BT.709 matrix into limited range;
//!   NV12 and YUV 4:2:0 input are only re-laid out. Constant frame rate: the codec time base
//!   is one frame, so frame `n` (pts `rate.frame_to_time(n)` in flicks, checked by
//!   `InputCheck`) gets pts `n`. This is the only place flicks become a codec time base (arch
//!   §4.1).
//! - **Audio.** FFmpeg's native `aac` takes planar f32, the layout of [`AudioChunk`], so pushed
//!   samples are only cut into its 1024-sample frames (the last may be short). Pts count
//!   samples.
//! - **Container.** MP4 with `movflags=+faststart` (the index ahead of the media data);
//!   `av_interleaved_write_frame` interleaves the streams.
//! - **Output file.** Written to a sibling `.<name>.<pid>-<n>.partial` through a Rust writer
//!   (custom AVIO) and renamed over the target when `finish` succeeds; removed on cancel, failure
//!   or drop, so an existing target survives until then. The writer keeps the OS error that
//!   FFmpeg only sees as `EIO` (a full disk becomes [`MediaError::Io`] with the OS message) and
//!   polls the cancel check, so a cancel also stops a `finish` that is flushing or moving the
//!   index.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use cutweave_model::{
    ColorPrimaries, ColorRange, Duration, FrameRate, MatrixCoefficients, SampleRate,
    TransferCharacteristics,
};
use ffmpeg_next::codec::{self, encoder};
use ffmpeg_next::format::context::{Output, StreamIo};
use ffmpeg_next::format::{self, Pixel, Sample, sample};
use ffmpeg_next::util::color;
use ffmpeg_next::{ChannelLayout, Dictionary, Packet, Rational, frame};

use super::sys::Scaler;
use crate::backend::{AudioEncodeSpec, EncodeSpec, EncodeSummary, Encoder, VideoEncodeSpec};
use crate::encode::{InputCheck, output_color};
use crate::path::display;
use crate::{
    AudioChunk, CancelCheck, MediaError, PixelFormat, ProgressSink, RateControl, VideoFrame,
};

/// The container this encoder writes.
const MP4: &str = "mp4";
/// The H.264 encoder.
const X264: &str = "libx264";
/// The AAC encoder (FFmpeg's own; no libfdk).
const AAC: &str = "aac";
/// swscale's BT.709 matrix.
const SWS_CS_ITU709: std::ffi::c_int = ffmpeg_next::ffi::SWS_CS_ITU709;

/// What the encoder's file writer needs: the partial file, or a test's wrapper around it.
pub(super) trait WriteSeek: Write + Seek + Send {}
impl<T: Write + Seek + Send> WriteSeek for T {}

/// Opens an encoder for `spec` (already validated) writing through the partial file itself.
pub(super) fn open(spec: &EncodeSpec) -> Result<Box<dyn Encoder>, MediaError> {
    open_with(spec, |file| Box::new(file))
}

/// [`open`], with the partial file wrapped by `wrap` (tests simulate a full disk with it).
pub(super) fn open_with(
    spec: &EncodeSpec,
    wrap: impl FnOnce(File) -> Box<dyn WriteSeek>,
) -> Result<Box<dyn Encoder>, MediaError> {
    check_container(spec)?;
    let video = spec.video.as_ref().map(VideoPlan::new).transpose()?;
    let audio = spec.audio.as_ref().map(AudioPlan::new).transpose()?;
    check_target(&spec.path)?;
    let (partial, file) = PartialFile::create(&spec.path)?;
    let state = Arc::new(SinkState::default());
    let sink = Sink {
        inner: wrap(file),
        pos: 0,
        state: Arc::clone(&state),
        cancel: spec.cancel.clone(),
    };
    let mut enc = FfmpegEncoder {
        out: None,
        video: None,
        audio: None,
        partial: Some(partial),
        path: spec.path.clone(),
        input: InputCheck::new(spec),
        cancel: spec.cancel.clone(),
        progress: spec.progress.clone(),
        io: state,
        encoded: Duration::ZERO,
        failed: None,
    };
    // From here on, an error drops `enc`, which closes and removes the partial file.
    match enc.start(sink, video.as_ref(), audio.as_ref()) {
        Ok(()) => Ok(Box::new(enc)),
        Err(fault) => Err(enc.fail(fault)),
    }
}

/// Only MP4 (named, or from a `.mp4` / `.m4v` extension).
fn check_container(spec: &EncodeSpec) -> Result<(), MediaError> {
    let name = match &spec.container {
        Some(name) => name.to_ascii_lowercase(),
        None => match spec
            .path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("mp4" | "m4v") => MP4.to_owned(),
            Some(ext) => ext.to_owned(),
            None => String::new(),
        },
    };
    if name == MP4 {
        return Ok(());
    }
    Err(MediaError::Unsupported {
        message: format!(
            "`{}`: the {} container; this backend writes MP4 (other containers arrive with the \
             export presets, P2)",
            display(&spec.path),
            if name.is_empty() { "unnamed" } else { &name }
        ),
    })
}

/// The path must not be a folder, and its folder must exist.
fn check_target(path: &Path) -> Result<(), MediaError> {
    let invalid = |reason: String| MediaError::InvalidPath {
        path: display(path),
        reason,
    };
    if path.is_dir() {
        return Err(invalid("is a folder".to_owned()));
    }
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    if !parent.is_dir() {
        return Err(invalid(format!(
            "the folder `{}` does not exist",
            display(parent)
        )));
    }
    Ok(())
}

fn unsupported(message: String) -> MediaError {
    MediaError::Unsupported { message }
}

/// A [`FrameRate`] or sample rate as an FFmpeg rational.
fn rational(num: u32, den: u32) -> Result<Rational, MediaError> {
    match (i32::try_from(num), i32::try_from(den)) {
        (Ok(n), Ok(d)) => Ok(Rational::new(n, d)),
        _ => Err(unsupported(format!("the rate {num}/{den} is out of range"))),
    }
}

/// The checked video settings, before anything is opened.
struct VideoPlan {
    spec: VideoEncodeSpec,
    codec: ffmpeg_next::Codec,
    /// One frame.
    time_base: Rational,
    fps: Rational,
}

impl VideoPlan {
    fn new(spec: &VideoEncodeSpec) -> Result<VideoPlan, MediaError> {
        if spec.codec != "h264" {
            return Err(unsupported(format!(
                "no `{}` encoder for MP4: this backend writes H.264 (HEVC and AV1 arrive in P2)",
                spec.codec
            )));
        }
        let codec = encoder::find_by_name(X264)
            .ok_or_else(|| unsupported(format!("this FFmpeg build has no {X264}")))?;
        if !matches!(
            spec.input_format,
            PixelFormat::Rgba8 | PixelFormat::Nv12 | PixelFormat::Yuv420p
        ) {
            return Err(unsupported(format!(
                "{:?} input: 8-bit H.264 takes Rgba8, Nv12 or Yuv420p frames",
                spec.input_format
            )));
        }
        if !spec.width.is_multiple_of(2) || !spec.height.is_multiple_of(2) {
            return Err(MediaError::Invalid {
                message: format!(
                    "H.264 4:2:0 needs an even width and height, not {}x{}",
                    spec.width, spec.height
                ),
            });
        }
        let color = output_color(&spec.color);
        if color.primaries != Some(ColorPrimaries::Bt709)
            || color.transfer != Some(TransferCharacteristics::Bt709)
            || color.matrix != Some(MatrixCoefficients::Bt709)
            || color.range != Some(ColorRange::Limited)
        {
            return Err(unsupported(format!(
                "output color {color:?}: exports are BT.709 limited range (other tags arrive with \
                 the advanced export settings, P2)"
            )));
        }
        let (num, den) = (spec.rate.num(), spec.rate.den());
        Ok(VideoPlan {
            spec: spec.clone(),
            codec,
            time_base: rational(den, num)?,
            fps: rational(num, den)?,
        })
    }
}

/// The checked audio settings, before anything is opened.
struct AudioPlan {
    spec: AudioEncodeSpec,
    codec: ffmpeg_next::Codec,
    rate: i32,
}

/// The layout of a mono or stereo stream.
fn layout(channels: u16) -> ChannelLayout {
    if channels == 1 {
        ChannelLayout::MONO
    } else {
        ChannelLayout::STEREO
    }
}

impl AudioPlan {
    fn new(spec: &AudioEncodeSpec) -> Result<AudioPlan, MediaError> {
        if spec.codec != AAC {
            return Err(unsupported(format!(
                "no `{}` encoder for MP4: this backend writes AAC",
                spec.codec
            )));
        }
        let codec = encoder::find_by_name(AAC)
            .ok_or_else(|| unsupported(format!("this FFmpeg build has no {AAC} encoder")))?;
        if !matches!(spec.channels, 1 | 2) {
            return Err(unsupported(format!(
                "{} audio channels: exports are mono or stereo",
                spec.channels
            )));
        }
        let hz = spec.rate.hz();
        let rate = i32::try_from(hz).map_err(|_| unsupported(format!("{hz} Hz")))?;
        let supported = codec
            .audio()
            .ok()
            .and_then(|a| a.rates())
            .is_none_or(|mut rates| rates.any(|r| r == rate));
        if !supported {
            return Err(unsupported(format!("AAC at {}", spec.rate)));
        }
        Ok(AudioPlan {
            spec: spec.clone(),
            codec,
            rate,
        })
    }
}

/// The partial output file: removed on drop unless [`commit`](Self::commit)ted.
struct PartialFile {
    path: PathBuf,
    /// Set once the file became the output.
    committed: bool,
}

impl PartialFile {
    /// Creates `.<name>.<pid>-<n>.partial` next to `target`.
    fn create(target: &Path) -> Result<(PartialFile, File), MediaError> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let name = target
            .file_name()
            .map_or_else(|| "output".into(), |n| n.to_string_lossy());
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = target.with_file_name(format!(".{name}.{}-{n}.partial", std::process::id()));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| MediaError::Io {
                path: display(target),
                message: format!("cannot create `{}`: {e}", display(&path)),
            })?;
        Ok((
            PartialFile {
                path,
                committed: false,
            },
            file,
        ))
    }

    /// Renames the file over `target` (replacing a file there). On failure the partial file is
    /// still removed on drop.
    fn commit(mut self, target: &Path) -> io::Result<()> {
        fs::rename(&self.path, target)?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for PartialFile {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        if let Err(e) = fs::remove_file(&self.path)
            && e.kind() != io::ErrorKind::NotFound
        {
            let path = display(&self.path);
            tracing::warn!(
                target: "cutweave_media",
                "could not remove the partial output `{path}`: {e}"
            );
        }
    }
}

/// What the writer shares with the encoder.
#[derive(Debug, Default)]
struct SinkState {
    /// The first I/O error, as the OS reported it.
    error: Mutex<Option<String>>,
    /// Bytes in the file so far (the furthest position written).
    size: AtomicU64,
}

impl SinkState {
    fn record(&self, e: &io::Error) {
        let mut slot = self.error.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.is_none() {
            *slot = Some(e.to_string());
        }
    }

    fn error(&self) -> Option<String> {
        self.error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// The custom AVIO target: the partial file, counting bytes, keeping errors and failing every
/// write once cancelled.
struct Sink {
    inner: Box<dyn WriteSeek>,
    pos: u64,
    state: Arc<SinkState>,
    cancel: Option<CancelCheck>,
}

impl Sink {
    fn kept<T>(&self, r: io::Result<T>) -> io::Result<T> {
        if let Err(e) = &r
            && e.kind() != io::ErrorKind::Interrupted
        {
            self.state.record(e);
        }
        r
    }
}

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.cancel.as_ref().is_some_and(CancelCheck::is_cancelled) {
            // Not recorded: the encoder reports the cancel itself.
            return Err(io::Error::other("cancelled"));
        }
        let r = self.inner.write(buf);
        let n = self.kept(r)?;
        self.pos += n as u64;
        self.state.size.fetch_max(self.pos, Ordering::Relaxed);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        let r = self.inner.flush();
        self.kept(r)
    }
}

impl Seek for Sink {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let r = self.inner.seek(to);
        self.pos = self.kept(r)?;
        Ok(self.pos)
    }
}

/// Why an FFmpeg step failed, before it becomes a [`MediaError`].
#[derive(Debug)]
enum Fault {
    /// Opening or running an encoder.
    Encode(&'static str, ffmpeg_next::Error),
    /// The muxer or the writer.
    Write(ffmpeg_next::Error),
    /// The cancel check answered `true`.
    Cancelled,
    /// Anything already typed.
    Media(MediaError),
}

/// How the input reaches `yuv420p`.
enum Convert {
    /// Already YUV 4:2:0 planar: copy the planes.
    Copy,
    /// swscale from the staging frame's layout.
    Scale {
        scaler: Scaler,
        staging: frame::Video,
    },
}

impl Convert {
    fn new(input: PixelFormat, size: (u32, u32)) -> Result<Convert, ffmpeg_next::Error> {
        let src = match input {
            PixelFormat::Rgba8 => Pixel::RGBA,
            PixelFormat::Nv12 => Pixel::NV12,
            _ => return Ok(Convert::Copy),
        };
        // RGB input is full range; NV12 input is already limited range like the output.
        let src_full = input == PixelFormat::Rgba8;
        Ok(Convert::Scale {
            scaler: Scaler::new(src, Pixel::YUV420P, size, SWS_CS_ITU709, (src_full, false))?,
            staging: frame::Video::new(src, size.0, size.1),
        })
    }

    /// `frame` as a new `yuv420p` frame (new, because the encoder may keep a reference).
    fn run(&mut self, frame: &VideoFrame) -> Result<frame::Video, Fault> {
        let mut yuv = frame::Video::new(Pixel::YUV420P, frame.width, frame.height);
        match self {
            Convert::Copy => copy_planes(frame, &mut yuv)?,
            Convert::Scale { scaler, staging } => {
                copy_planes(frame, staging)?;
                scaler
                    .run(staging, &mut yuv)
                    .map_err(|e| Fault::Encode("swscale", e))?;
            }
        }
        Ok(yuv)
    }
}

/// Copies the planes of `frame` (validated, same layout) into `dst`, row by row.
fn copy_planes(frame: &VideoFrame, dst: &mut frame::Video) -> Result<(), Fault> {
    let short = || {
        Fault::Media(MediaError::Backend {
            message: "an FFmpeg frame is smaller than its layout".to_owned(),
        })
    };
    for i in 0..frame.format.plane_count() {
        let (_, height, _) = frame.format.plane_layout(i, frame.width, frame.height);
        let stride = dst.stride(i);
        let data = dst.data_mut(i);
        for y in 0..height {
            let row = frame.row(i, y).ok_or_else(short)?;
            let at = y as usize * stride;
            data.get_mut(at..at + row.len())
                .ok_or_else(short)?
                .copy_from_slice(row);
        }
    }
    Ok(())
}

/// The open video stream.
struct VideoOut {
    encoder: encoder::video::Encoder,
    stream: usize,
    time_base: Rational,
    stream_time_base: Rational,
    rate: FrameRate,
    convert: Convert,
}

/// The open audio stream.
struct AudioOut {
    encoder: encoder::audio::Encoder,
    stream: usize,
    time_base: Rational,
    stream_time_base: Rational,
    rate: SampleRate,
    /// Channel count (1 or 2; `ChannelLayout` itself is not `Send`).
    channels: u16,
    /// Samples per encoder frame.
    frame_size: usize,
    /// Whether the last frame may be shorter (else it is padded with silence).
    small_last_frame: bool,
    /// Samples pushed but not yet sent, per channel.
    pending: Vec<Vec<f32>>,
    /// Samples sent to the encoder (the pts of the next frame).
    sent: i64,
}

impl AudioOut {
    /// Sends the first `n` pending samples as one frame of `len ≥ n` samples (zero-padded).
    fn send(&mut self, n: usize, len: usize) -> Result<(), Fault> {
        let mut f = frame::Audio::new(
            Sample::F32(sample::Type::Planar),
            len,
            layout(self.channels),
        );
        f.set_rate(self.rate.hz());
        for (c, pending) in self.pending.iter_mut().enumerate() {
            let plane = f.plane_mut::<f32>(c);
            plane[..n].copy_from_slice(&pending[..n]);
            plane[n..].fill(0.0);
            pending.drain(..n);
        }
        f.set_pts(Some(self.sent));
        self.sent += i64::try_from(n).unwrap_or(i64::MAX);
        self.encoder
            .send_frame(&f)
            .map_err(|e| Fault::Encode(AAC, e))
    }
}

/// The FFmpeg H.264 + AAC MP4 encoder (see the module docs).
struct FfmpegEncoder {
    // Field order matters: `out` (which owns the writer and so the open file) must close before
    // `partial` removes the file.
    out: Option<Output>,
    video: Option<VideoOut>,
    audio: Option<AudioOut>,
    partial: Option<PartialFile>,
    /// The target.
    path: PathBuf,
    input: InputCheck,
    cancel: Option<CancelCheck>,
    progress: Option<ProgressSink>,
    io: Arc<SinkState>,
    /// Output media time the encoders have produced.
    encoded: Duration,
    /// The error that ended this encode; every later call returns it.
    failed: Option<MediaError>,
}

impl FfmpegEncoder {
    /// Creates the muxer on `sink`, opens the encoders and writes the header.
    fn start(
        &mut self,
        sink: Sink,
        video: Option<&VideoPlan>,
        audio: Option<&AudioPlan>,
    ) -> Result<(), Fault> {
        let io = StreamIo::from_write_seek(sink).map_err(Fault::Write)?;
        let url = self
            .partial
            .as_ref()
            .map_or_else(String::new, |p| display(&p.path));
        // The URL is the partial file's own path: `+faststart` re-opens it to move the index.
        let mut out = format::output_to_stream(io, Some(&url), Some(MP4)).map_err(Fault::Write)?;
        let global_header = out.format().flags().contains(format::Flags::GLOBAL_HEADER);
        let mut video = video
            .map(|plan| open_video(&mut out, plan, global_header))
            .transpose()?;
        let mut audio = audio
            .map(|plan| open_audio(&mut out, plan, global_header))
            .transpose()?;
        let mut options = Dictionary::new();
        options.set("movflags", "+faststart");
        let rest = out.write_header_with(options).map_err(Fault::Write)?;
        if rest.get("movflags").is_some() {
            return Err(Fault::Media(MediaError::Backend {
                message: "the MP4 muxer ignored `movflags`".to_owned(),
            }));
        }
        drop(rest);
        // The muxer picks the stream time bases while writing the header.
        if let Some(v) = &mut video {
            v.stream_time_base = stream_time_base(&out, v.stream)?;
        }
        if let Some(a) = &mut audio {
            a.stream_time_base = stream_time_base(&out, a.stream)?;
        }
        self.out = Some(out);
        self.video = video;
        self.audio = audio;
        Ok(())
    }

    fn cancelled(&self) -> bool {
        self.cancel.as_ref().is_some_and(CancelCheck::is_cancelled)
    }

    /// The error a fault means: a cancel wins; then an I/O error the writer saw.
    fn error(&self, fault: Fault) -> MediaError {
        if self.cancelled() || matches!(fault, Fault::Cancelled) {
            return MediaError::Cancelled;
        }
        if let Some(message) = self.io.error() {
            return MediaError::Io {
                path: display(&self.path),
                message: format!("writing the output failed: {message}"),
            };
        }
        match fault {
            Fault::Encode(which, e) => MediaError::Backend {
                message: format!("{which}: {e}"),
            },
            Fault::Write(e) => MediaError::Backend {
                message: format!("writing `{}`: {e}", display(&self.path)),
            },
            Fault::Media(e) => e,
            Fault::Cancelled => MediaError::Cancelled,
        }
    }

    /// Ends the encode: closes and removes the partial output, and remembers the error.
    fn fail(&mut self, fault: Fault) -> MediaError {
        let err = self.error(fault);
        self.out = None;
        self.video = None;
        self.audio = None;
        self.partial = None;
        self.failed = Some(err.clone());
        err
    }

    /// `Err` once the encode failed or was cancelled.
    fn ready(&mut self) -> Result<(), MediaError> {
        if let Some(e) = &self.failed {
            return Err(e.clone());
        }
        if self.cancelled() {
            return Err(self.fail(Fault::Cancelled));
        }
        Ok(())
    }

    fn report(&self) {
        if let Some(sink) = &self.progress {
            sink.report(
                &self
                    .input
                    .progress(self.encoded, self.io.size.load(Ordering::Relaxed)),
            );
        }
    }

    fn encode_video(&mut self, frame: &VideoFrame, index: i64) -> Result<(), Fault> {
        let v = self.video.as_mut().ok_or(Fault::Cancelled)?;
        let mut yuv = v.convert.run(frame)?;
        yuv.set_pts(Some(index));
        v.encoder
            .send_frame(&yuv)
            .map_err(|e| Fault::Encode(X264, e))?;
        self.drain_video()
    }

    fn encode_audio(&mut self, chunk: &AudioChunk) -> Result<(), Fault> {
        let a = self.audio.as_mut().ok_or(Fault::Cancelled)?;
        for (pending, plane) in a.pending.iter_mut().zip(&chunk.planes) {
            pending.extend_from_slice(plane);
        }
        loop {
            let a = self.audio.as_mut().ok_or(Fault::Cancelled)?;
            if a.pending.first().map_or(0, Vec::len) < a.frame_size {
                return Ok(());
            }
            a.send(a.frame_size, a.frame_size)?;
            self.drain_audio()?;
        }
    }

    fn drain_video(&mut self) -> Result<(), Fault> {
        let (Some(v), Some(out)) = (self.video.as_mut(), self.out.as_mut()) else {
            return Err(Fault::Cancelled);
        };
        let end = drain(
            &mut v.encoder,
            out,
            (v.stream, v.time_base, v.stream_time_base),
            1,
            self.cancel.as_ref(),
        )?;
        if let Some(t) = end.and_then(|n| v.rate.frame_to_time(n).ok()) {
            self.encoded = self
                .encoded
                .max(Duration::try_from(t).unwrap_or(Duration::ZERO));
        }
        Ok(())
    }

    fn drain_audio(&mut self) -> Result<(), Fault> {
        let (Some(a), Some(out)) = (self.audio.as_mut(), self.out.as_mut()) else {
            return Err(Fault::Cancelled);
        };
        let end = drain(
            &mut a.encoder,
            out,
            (a.stream, a.time_base, a.stream_time_base),
            0,
            self.cancel.as_ref(),
        )?;
        if let Some(t) = end.and_then(|n| a.rate.samples_to_time(n.max(0)).ok()) {
            self.encoded = self
                .encoded
                .max(Duration::try_from(t).unwrap_or(Duration::ZERO));
        }
        Ok(())
    }

    /// Sends what is pending, drains both encoders and writes the trailer (which moves the index
    /// to the front).
    fn flush(&mut self) -> Result<(), Fault> {
        if let Some(a) = self.audio.as_mut() {
            let n = a.pending.first().map_or(0, Vec::len);
            if n > 0 {
                let len = if a.small_last_frame { n } else { a.frame_size };
                a.send(n, len)?;
            }
            a.encoder.send_eof().map_err(|e| Fault::Encode(AAC, e))?;
            self.drain_audio()?;
            self.report();
        }
        if let Some(v) = self.video.as_mut() {
            v.encoder.send_eof().map_err(|e| Fault::Encode(X264, e))?;
            self.drain_video()?;
            self.report();
        }
        if self.cancelled() {
            return Err(Fault::Cancelled);
        }
        let out = self.out.as_mut().ok_or(Fault::Cancelled)?;
        out.write_trailer().map_err(Fault::Write)
    }
}

/// Moves every packet `enc` has ready into the muxer, giving packets without a duration
/// `default_duration` (one frame for video: CFR). Returns the end of the latest packet in the
/// codec time base.
fn drain(
    enc: &mut encoder::Encoder,
    out: &mut Output,
    (stream, time_base, stream_time_base): (usize, Rational, Rational),
    default_duration: i64,
    cancel: Option<&CancelCheck>,
) -> Result<Option<i64>, Fault> {
    let mut packet = Packet::empty();
    let mut end = None;
    loop {
        match enc.receive_packet(&mut packet) {
            Ok(()) => {}
            Err(ffmpeg_next::Error::Other { errno }) if errno == ffmpeg_next::error::EAGAIN => {
                return Ok(end);
            }
            Err(ffmpeg_next::Error::Eof) => return Ok(end),
            Err(e) => return Err(Fault::Encode("encoder", e)),
        }
        if packet.duration() == 0 {
            packet.set_duration(default_duration);
        }
        if let Some(pts) = packet.pts() {
            let e = pts.saturating_add(packet.duration());
            end = Some(end.map_or(e, |m: i64| m.max(e)));
        }
        packet.set_stream(stream);
        packet.rescale_ts(time_base, stream_time_base);
        packet.write_interleaved(out).map_err(Fault::Write)?;
        if cancel.is_some_and(CancelCheck::is_cancelled) {
            return Err(Fault::Cancelled);
        }
    }
}

fn stream_time_base(out: &Output, index: usize) -> Result<Rational, Fault> {
    out.stream(index).map(|s| s.time_base()).ok_or_else(|| {
        Fault::Media(MediaError::Backend {
            message: format!("the muxer lost stream {index}"),
        })
    })
}

fn open_video(out: &mut Output, plan: &VideoPlan, global_header: bool) -> Result<VideoOut, Fault> {
    let spec = &plan.spec;
    let mut ctx = codec::context::Context::new_with_codec(plan.codec)
        .encoder()
        .video()
        .map_err(|e| Fault::Encode(X264, e))?;
    ctx.set_width(spec.width);
    ctx.set_height(spec.height);
    ctx.set_format(Pixel::YUV420P);
    ctx.set_time_base(plan.time_base);
    ctx.set_frame_rate(Some(plan.fps));
    ctx.set_aspect_ratio(Rational::new(1, 1));
    ctx.set_colorspace(color::Space::BT709);
    ctx.set_color_range(color::Range::MPEG);
    ctx.set_color_primaries(color::Primaries::BT709);
    ctx.set_color_transfer_characteristic(color::TransferCharacteristic::BT709);
    if global_header {
        ctx.set_flags(codec::Flags::GLOBAL_HEADER);
    }
    let mut options = Dictionary::new();
    options.set("preset", spec.preset.name());
    match spec.rate_control {
        RateControl::Crf(q) => options.set("crf", &q.to_string()),
        RateControl::Bitrate(bps) => ctx.set_bit_rate(bps as usize),
    }
    let threads = if spec.threads == 0 {
        "auto".to_owned()
    } else {
        spec.threads.to_string()
    };
    options.set("threads", &threads);
    let encoder = ctx.open_with(options).map_err(|e| {
        Fault::Media(unsupported(format!(
            "{X264} refused {}x{} at {} fps: {e}",
            spec.width, spec.height, spec.rate
        )))
    })?;
    let mut stream = out.add_stream(plan.codec).map_err(Fault::Write)?;
    stream.set_parameters(&encoder);
    stream.set_time_base(plan.time_base);
    stream.set_rate(plan.fps);
    stream.set_avg_frame_rate(plan.fps);
    let index = stream.index();
    Ok(VideoOut {
        encoder,
        stream: index,
        time_base: plan.time_base,
        stream_time_base: plan.time_base,
        rate: spec.rate,
        convert: Convert::new(spec.input_format, (spec.width, spec.height))
            .map_err(|e| Fault::Encode("swscale", e))?,
    })
}

fn open_audio(out: &mut Output, plan: &AudioPlan, global_header: bool) -> Result<AudioOut, Fault> {
    let spec = &plan.spec;
    let mut ctx = codec::context::Context::new_with_codec(plan.codec)
        .encoder()
        .audio()
        .map_err(|e| Fault::Encode(AAC, e))?;
    let time_base = Rational::new(1, plan.rate);
    ctx.set_rate(plan.rate);
    ctx.set_channel_layout(layout(spec.channels));
    ctx.set_format(Sample::F32(sample::Type::Planar));
    ctx.set_bit_rate(spec.bitrate as usize);
    ctx.set_time_base(time_base);
    if global_header {
        ctx.set_flags(codec::Flags::GLOBAL_HEADER);
    }
    let encoder = ctx.open_with(Dictionary::new()).map_err(|e| {
        Fault::Media(unsupported(format!(
            "{AAC} refused {} x{} at {} b/s: {e}",
            spec.rate, spec.channels, spec.bitrate
        )))
    })?;
    let mut stream = out.add_stream(plan.codec).map_err(Fault::Write)?;
    stream.set_parameters(&encoder);
    stream.set_time_base(time_base);
    let index = stream.index();
    let frame_size = match encoder.frame_size() {
        0 => 1024,
        n => n as usize,
    };
    let caps = plan.codec.capabilities();
    Ok(AudioOut {
        stream: index,
        time_base,
        stream_time_base: time_base,
        rate: spec.rate,
        channels: spec.channels,
        frame_size,
        small_last_frame: caps.intersects(
            codec::capabilities::Capabilities::SMALL_LAST_FRAME
                | codec::capabilities::Capabilities::VARIABLE_FRAME_SIZE,
        ),
        pending: vec![Vec::with_capacity(2 * frame_size); usize::from(spec.channels)],
        sent: 0,
        encoder,
    })
}

impl Encoder for FfmpegEncoder {
    fn push_video(&mut self, frame: &VideoFrame) -> Result<(), MediaError> {
        self.ready()?;
        let index = self.input.video(frame)?;
        if let Err(fault) = self.encode_video(frame, index) {
            return Err(self.fail(fault));
        }
        self.input.add_video();
        self.report();
        Ok(())
    }

    fn push_audio(&mut self, chunk: &AudioChunk) -> Result<(), MediaError> {
        self.ready()?;
        self.input.audio(chunk)?;
        if let Err(fault) = self.encode_audio(chunk) {
            return Err(self.fail(fault));
        }
        self.input.add_audio(chunk.frames());
        self.report();
        Ok(())
    }

    fn finish(mut self: Box<Self>) -> Result<EncodeSummary, MediaError> {
        self.ready()?;
        if let Err(fault) = self.flush() {
            return Err(self.fail(fault));
        }
        // Close the file (the writer goes with the muxer) before renaming it.
        self.out = None;
        self.video = None;
        self.audio = None;
        if let Some(message) = self.io.error() {
            return Err(self.fail(Fault::Media(MediaError::Io {
                path: display(&self.path),
                message: format!("writing the output failed: {message}"),
            })));
        }
        if self.cancelled() {
            return Err(self.fail(Fault::Cancelled));
        }
        let partial = self.partial.take().ok_or(MediaError::Cancelled)?;
        if let Err(e) = partial.commit(&self.path) {
            return Err(self.fail(Fault::Media(MediaError::Io {
                path: display(&self.path),
                message: format!("cannot move the finished output into place: {e}"),
            })));
        }
        let summary = self.input.summary();
        self.encoded = summary.duration;
        self.report();
        Ok(summary)
    }
}

#[cfg(test)]
mod tests {
    use cutweave_model::{ColorTags, Time};

    use super::*;
    use crate::testing::contract::TempDir;
    use crate::{FfmpegBackend, MediaBackend, Plane};

    fn solid(format: PixelFormat, (w, h): (u32, u32), rgb: [u8; 3]) -> VideoFrame {
        let planes = match format {
            PixelFormat::Rgba8 => vec![Plane {
                data: [rgb[0], rgb[1], rgb[2], 255].repeat((w * h) as usize),
                stride: 4 * w,
            }],
            _ => unreachable!("test frames are RGBA"),
        };
        VideoFrame {
            pts: Time::ZERO,
            width: w,
            height: h,
            format,
            planes,
            color: ColorTags::default(),
            damaged: false,
        }
    }

    #[allow(clippy::cast_precision_loss)] // small test planes
    fn mean(plane: &[u8], stride: usize, (w, h): (usize, usize)) -> f64 {
        let sum: u64 = (0..h)
            .flat_map(|y| {
                plane[y * stride..y * stride + w]
                    .iter()
                    .map(|&v| u64::from(v))
            })
            .sum();
        sum as f64 / (w * h) as f64
    }

    #[test]
    fn rgba_becomes_bt709_limited_range_yuv() {
        FfmpegBackend::new().expect("backend");
        let size = (16, 8);
        let mut convert = Convert::new(PixelFormat::Rgba8, size).expect("swscale");
        // BT.709 limited range: Y = 16 + 219·Y', Cb/Cr = 128 + 224·(B'−Y')/1.8556, (R'−Y')/1.5748.
        // BT.601 would give red Y 81, green Y 145: the matrix is checked, not just the range.
        for (rgb, want) in [
            ([0, 0, 0], [16.0, 128.0, 128.0]),
            ([255, 255, 255], [235.0, 128.0, 128.0]),
            ([255, 0, 0], [62.6, 102.3, 240.0]),
            ([0, 255, 0], [172.6, 41.7, 26.3]),
            ([0, 0, 255], [31.8, 240.0, 117.7]),
        ] {
            let yuv = convert
                .run(&solid(PixelFormat::Rgba8, size, rgb))
                .expect("convert");
            assert_eq!(yuv.format(), Pixel::YUV420P);
            let got = [
                mean(yuv.data(0), yuv.stride(0), (16, 8)),
                mean(yuv.data(1), yuv.stride(1), (8, 4)),
                mean(yuv.data(2), yuv.stride(2), (8, 4)),
            ];
            for (g, w) in got.iter().zip(want) {
                assert!((g - w).abs() <= 1.0, "{rgb:?}: got {got:?}, want {want:?}");
            }
        }
    }

    #[test]
    #[allow(clippy::cast_possible_truncation)] // sample values stay below 256
    fn nv12_and_yuv420p_are_only_relaid_out() {
        FfmpegBackend::new().expect("backend");
        let (w, h) = (8_u32, 4_u32);
        let y: Vec<u8> = (0..w * h).map(|i| (16 + i * 5 % 200) as u8).collect();
        let cb: Vec<u8> = (0..(w / 2) * (h / 2)).map(|i| (60 + i * 7) as u8).collect();
        let cr: Vec<u8> = (0..(w / 2) * (h / 2))
            .map(|i| (200 - i * 3) as u8)
            .collect();
        let uv: Vec<u8> = cb.iter().zip(&cr).flat_map(|(&u, &v)| [u, v]).collect();
        let nv12 = VideoFrame {
            pts: Time::ZERO,
            width: w,
            height: h,
            format: PixelFormat::Nv12,
            planes: vec![
                Plane {
                    data: y.clone(),
                    stride: w,
                },
                Plane {
                    data: uv,
                    stride: w,
                },
            ],
            color: ColorTags::default(),
            damaged: false,
        };
        let planar = VideoFrame {
            format: PixelFormat::Yuv420p,
            planes: vec![
                Plane {
                    data: y.clone(),
                    stride: w,
                },
                Plane {
                    data: cb.clone(),
                    stride: w / 2,
                },
                Plane {
                    data: cr.clone(),
                    stride: w / 2,
                },
            ],
            ..nv12.clone()
        };
        for frame in [nv12, planar] {
            let mut convert = Convert::new(frame.format, (w, h)).expect("swscale");
            let out = convert.run(&frame).expect("convert");
            let plane = |i: usize, (pw, ph): (u32, u32)| -> Vec<u8> {
                (0..ph as usize)
                    .flat_map(|r| out.data(i)[r * out.stride(i)..][..pw as usize].to_vec())
                    .collect()
            };
            assert_eq!(plane(0, (w, h)), y, "{:?} luma", frame.format);
            assert_eq!(plane(1, (w / 2, h / 2)), cb, "{:?} Cb", frame.format);
            assert_eq!(plane(2, (w / 2, h / 2)), cr, "{:?} Cr", frame.format);
        }
    }

    /// The partial file behind a disk that is full after `budget` bytes; counts what it took.
    struct FullDisk {
        file: File,
        budget: u64,
        written: Arc<AtomicU64>,
    }

    impl Write for FullDisk {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let len = buf.len() as u64;
            if len > self.budget {
                return Err(io::Error::new(
                    io::ErrorKind::StorageFull,
                    "no space left on device (simulated)",
                ));
            }
            let n = self.file.write(buf)?;
            self.budget -= n as u64;
            self.written.fetch_add(n as u64, Ordering::Relaxed);
            Ok(n)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.file.flush()
        }
    }

    impl Seek for FullDisk {
        fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
            self.file.seek(to)
        }
    }

    fn frame(n: u64, (w, h): (u32, u32)) -> VideoFrame {
        crate::testing::FakeVideo::new(w, h, FrameRate::FPS_30, n + 1)
            .frame(n, PixelFormat::Rgba8)
            .expect("fake frame")
    }

    const SIZE: (u32, u32) = (320, 180);
    const FRAMES: u64 = 30;

    /// A one-thread (so byte-identical every run) lossless 320×180 encode of 30 frames on a
    /// disk with `budget` bytes free: its result, the error of a failed push if any, and the
    /// bytes the disk took.
    fn encode_on_disk(path: &Path, budget: u64) -> (Result<EncodeSummary, MediaError>, u64) {
        let spec = EncodeSpec::new(path).with_video(VideoEncodeSpec {
            preset: crate::SpeedPreset::Ultrafast,
            rate_control: RateControl::Crf(0),
            threads: 1,
            ..VideoEncodeSpec::h264(SIZE.0, SIZE.1, FrameRate::FPS_30, PixelFormat::Rgba8)
        });
        let written = Arc::new(AtomicU64::new(0));
        let disk = {
            let written = Arc::clone(&written);
            move |file| -> Box<dyn WriteSeek> {
                Box::new(FullDisk {
                    file,
                    budget,
                    written,
                })
            }
        };
        let mut enc = match open_with(&spec, disk) {
            Ok(enc) => enc,
            Err(e) => return (Err(e), written.load(Ordering::Relaxed)),
        };
        for n in 0..FRAMES {
            if let Err(e) = enc.push_video(&frame(n, SIZE)) {
                // Later calls repeat the error.
                assert_eq!(enc.push_video(&frame(n + 1, SIZE)), Err(e.clone()));
                assert_eq!(enc.finish().err(), Some(e.clone()));
                return (Err(e), written.load(Ordering::Relaxed));
            }
        }
        (enc.finish(), written.load(Ordering::Relaxed))
    }

    fn assert_disk_full(result: Result<EncodeSummary, MediaError>, path: &Path, when: &str) {
        match result {
            Err(MediaError::Io { path: p, message }) => {
                assert_eq!(p, display(path), "{when}");
                assert!(message.contains("simulated"), "{when}: {message}");
            }
            other => panic!("{when}: want Io, got {other:?}"),
        }
    }

    #[test]
    fn a_full_disk_is_an_io_error_and_leaves_no_file() {
        FfmpegBackend::new().expect("backend");
        let dir = TempDir::new("disk-full");
        let path = dir.path().join("out.mp4");
        let listing = || {
            std::fs::read_dir(dir.path())
                .expect("list")
                .filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };

        // Enough room: the size of the whole encode, faststart rewrite included.
        let (ok, total) = encode_on_disk(&path, u64::MAX);
        assert_eq!(ok.map(|s| s.video_frames), Ok(FRAMES));
        assert_eq!(listing(), ["out.mp4"]);
        std::fs::remove_file(&path).expect("remove");

        // Full while frames are pushed: lossless frames overflow 64 KiB long before the end.
        let (result, _) = encode_on_disk(&path, 64 * 1024);
        assert_disk_full(result, &path, "while pushing");
        assert!(listing().is_empty(), "{:?}", listing());

        // Full on the very last write, inside `finish` (trailer and index move).
        let (result, took) = encode_on_disk(&path, total - 1);
        assert_disk_full(result, &path, "while finishing");
        assert!(took < total);
        assert!(listing().is_empty(), "{:?}", listing());

        // No room at all.
        let (result, _) = encode_on_disk(&path, 0);
        assert_disk_full(result, &path, "with no room at all");
        assert!(listing().is_empty(), "{:?}", listing());
    }

    #[test]
    fn the_backend_opens_this_encoder() {
        let backend = FfmpegBackend::new().expect("backend");
        let dir = TempDir::new("open");
        let path = dir.path().join("a.mp4");
        let spec =
            EncodeSpec::new(&path).with_audio(AudioEncodeSpec::aac(SampleRate::HZ_48_000, 1));
        let mut enc = backend.open_encoder(&spec).expect("open");
        let tone = crate::testing::FakeAudio::tone(
            SampleRate::HZ_48_000,
            1,
            Duration::from_secs(1).expect("1 s"),
        );
        enc.push_audio(&AudioChunk {
            pts: cutweave_model::Time::ZERO,
            rate: SampleRate::HZ_48_000,
            channels: 1,
            planes: vec![(0..48_000).map(|n| tone.sample(0, n)).collect()],
        })
        .expect("push");
        let summary = enc.finish().expect("finish");
        assert_eq!(summary.audio_samples, 48_000);
        assert!(path.exists());
    }
}
