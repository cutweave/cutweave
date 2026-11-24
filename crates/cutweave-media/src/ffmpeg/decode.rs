// SPDX-License-Identifier: GPL-3.0-or-later
//! The FFmpeg [`VideoDecoder`] (WP-1D.03): frame-exact seek from the packet-scan index, software
//! decode to the decoder's own planes, damage handling and cancellation. The port of the
//! validated Spike S3 source (`spikes/s3-decode/src/source.rs`, findings F3, F4, F5, F7).
//!
//! **Seek to frame T** (start keyframe K from the index, ADR-0003):
//!
//! 1. If the decoder already holds T, return it again. If it is positioned past K and before T
//!    (scrubbing forward), keep decoding.
//! 2. Otherwise seek the demuxer on the video stream, in its time base, to K's timestamp (dts for
//!    MP4/MOV/TS, pts for Matroska), flush the decoder (draining it to EOF first if it saw EOF:
//!    FFmpeg 8.1's HEVC decoder otherwise emits a stale frame, S3 F7) and read packets, locating
//!    each in the index: packets before K are dropped without being decoded; if the demuxer
//!    landed after K, retry from earlier keyframes, then reopen the file.
//! 3. Feed packets from K. Packets shown before T go in with `skip_frame = NONREF` (non-reference
//!    pictures are not even decoded); frames before T are dropped. The frame with T's pts is
//!    the answer.
//!
//! **Damage** (S3 F5): a frame past T instead of T (HEVC drops damaged pictures) is returned
//! flagged `damaged`; so is the last frame before T when the stream ends first (truncation).
//! Frames the decoder concealed errors in are flagged too.
//!
//! **Threads** (S3 F4): frame threads for long-GOP seek decoders and sequential decoders, slice
//! threads for seek decoders on intra-only streams (every frame a keyframe in the index).
//!
//! **Layout**: frames keep the decoder's planes when [`PixelFormat`] has them (no CPU
//! conversion, arch §7); other layouts, and an explicit `target_pixfmt` (`Rgba8` thumbnails),
//! go through swscale with the stream's matrix and range.

use std::ffi::c_int;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cutweave_model::{ColorRange, ColorTags, MatrixCoefficients, Time};
use ffmpeg_next::codec::{self, threading};
use ffmpeg_next::format::Pixel;
use ffmpeg_next::format::context::Input;
use ffmpeg_next::media::Type;
use ffmpeg_next::util::error::EAGAIN;
use ffmpeg_next::{Discard, Error, Packet, decoder, frame};

use super::error::media_error;
use super::sys::{self, Scaler};
use super::{index, input, probe};
use crate::backend::{DecodeMode, DecodeOpts, VideoDecoder};
use crate::cancel;
use crate::path::display as shown;
use crate::{CancelToken, FrameIndex, MediaError, PixelFormat, Plane, VideoFrame};

/// Packets one request may feed before giving up (a damaged file that never yields its frame).
const MAX_PACKETS_PER_REQUEST: u64 = 200_000;
/// Packets the index does not know that a landing may skip (MPEG-TS resync fragments).
const MAX_UNKNOWN_PACKETS: u64 = 10_000;
/// Corrupt packets in a row the demuxer may report before the stream counts as ended.
const MAX_DEMUX_ERRORS: u32 = 10_000;
/// Earlier keyframes a seek retries from when the demuxer lands late.
const LANDING_RETRIES: usize = 3;
/// Decode errors one request tolerates (FFmpeg conceals and goes on).
const MAX_DECODE_ERRORS: u64 = 10_000;

/// Opens a decoder for the main video stream of `path`.
pub(super) fn open(path: &Path, opts: &DecodeOpts) -> Result<Box<dyn VideoDecoder>, MediaError> {
    let index = match &opts.index {
        Some(index) => Arc::clone(index),
        None => Arc::new(index::index_media(path)?),
    };
    let threads = threading_for(opts, &index);
    let opened = open_stream(path, &index, threads)?;
    let end = index.end_time();
    Ok(Box::new(FfmpegVideoDecoder {
        path: path.to_path_buf(),
        input: opened.input,
        decoder: opened.decoder,
        stream: opened.stream,
        threads,
        end,
        index,
        frame: frame::Video::empty(),
        scratch: frame::Video::empty(),
        out: Output::new(opts.target_pixfmt),
        stream_color: opened.color,
        pending: None,
        next_idx: Some(0),
        positioned: true,
        eof_sent: false,
        last_pts: None,
        cancel: None,
        stats: Stats::default(),
    }))
}

/// Frame threads for throughput and long-GOP seeks; slice threads for seeks in intra-only
/// streams, where frame threads only add pipeline delay (S3 F4: ProRes seeks 15× slower).
fn threading_for(opts: &DecodeOpts, index: &FrameIndex) -> threading::Config {
    let intra = index.max_decode_distance() <= 1;
    let kind = match opts.mode {
        DecodeMode::Seek if intra => threading::Type::Slice,
        _ => threading::Type::Frame,
    };
    threading::Config {
        kind,
        count: usize::from(opts.threads),
    }
}

struct Opened {
    input: Input,
    decoder: decoder::Video,
    stream: usize,
    color: ColorTags,
}

/// Opens the container and a decoder for the index's stream.
fn open_stream(
    path: &Path,
    index: &FrameIndex,
    threads: threading::Config,
) -> Result<Opened, MediaError> {
    let mut input = input::open(path)?;
    let stream = usize::try_from(index.stream()).unwrap_or(usize::MAX);
    let not_this_file = || MediaError::Invalid {
        message: format!(
            "the frame index passed for `{}` is another file's (stream {} in time base {}/{})",
            shown(path),
            index.stream(),
            index.time_base().num,
            index.time_base().den
        ),
    };
    let (ctx, time_base, color) = {
        let st = input.stream(stream).ok_or_else(not_this_file)?;
        let par = st.parameters();
        let tb = st.time_base();
        let matches = par.medium() == Type::Video
            && u32::try_from(tb.numerator()).ok() == Some(index.time_base().num)
            && u32::try_from(tb.denominator()).ok() == Some(index.time_base().den);
        if !matches {
            return Err(not_this_file());
        }
        let p = sys::stream_params(&par);
        let color = color_tags(
            p.primaries.as_deref(),
            p.transfer.as_deref(),
            p.matrix.as_deref(),
            p.range,
        );
        let ctx = codec::Context::from_parameters(par).map_err(|e| media_error(path, e))?;
        (ctx, tb, color)
    };
    sys::discard_other_streams(&mut input, stream);
    let mut ctx = ctx;
    ctx.set_threading(threads);
    let mut dec = ctx.decoder();
    dec.set_packet_time_base(time_base);
    let decoder = dec.video().map_err(|e| match e {
        Error::DecoderNotFound => MediaError::Unsupported {
            message: format!("`{}`: no decoder for its video codec", shown(path)),
        },
        e => media_error(path, e),
    })?;
    Ok(Opened {
        input,
        decoder,
        stream,
        color,
    })
}

/// Color tags from FFmpeg names (unknown names are kept, never an error).
fn color_tags(
    primaries: Option<&str>,
    transfer: Option<&str>,
    matrix: Option<&str>,
    range: Option<ColorRange>,
) -> ColorTags {
    ColorTags {
        primaries: primaries.map(probe::parse_name),
        transfer: transfer.map(probe::parse_name),
        matrix: matrix.map(probe::parse_name),
        range,
    }
}

/// What a decode step is looking for.
#[derive(Clone, Copy, Debug)]
enum Want {
    Next,
    Pts(i64),
}

/// Counters for the debug log.
#[derive(Clone, Copy, Debug, Default)]
struct Stats {
    seeks: u64,
    continued: u64,
    landing_retries: u64,
    reopens: u64,
    index_mismatches: u64,
    decode_errors: u64,
    damaged: u64,
}

struct FfmpegVideoDecoder {
    path: PathBuf,
    input: Input,
    decoder: decoder::Video,
    stream: usize,
    threads: threading::Config,
    index: Arc<FrameIndex>,
    /// Seeks at or past this are out of range (`None`: a single picture, never).
    end: Option<Time>,
    /// The latest frame the decoder produced; valid while `last_pts` is `Some`.
    frame: frame::Video,
    /// Receive target, swapped with `frame` on success (a failed receive unrefs its target).
    scratch: frame::Video,
    out: Output,
    /// The stream's tags, for fields a frame leaves unspecified.
    stream_color: ColorTags,
    /// A packet read but not yet sent (the landing keyframe, or one the decoder refused).
    pending: Option<Packet>,
    /// Decode index of the next packet to send, when known.
    next_idx: Option<usize>,
    /// The decoder was fed contiguously from a keyframe since the last flush.
    positioned: bool,
    eof_sent: bool,
    /// pts (ticks) of `frame`.
    last_pts: Option<i64>,
    cancel: Option<CancelToken>,
    stats: Stats,
}

impl FfmpegVideoDecoder {
    fn cancelled(&self) -> Result<(), MediaError> {
        cancel::check(self.cancel.as_ref())
    }

    fn corrupt(&self, message: String) -> MediaError {
        MediaError::Corrupt {
            path: shown(&self.path),
            message,
        }
    }

    /// Frame-exact seek to presentation index `n`.
    fn seek_frame(&mut self, n: usize) -> Result<VideoFrame, MediaError> {
        let Some(&target) = self.index.frames().get(n) else {
            return Err(self.corrupt(format!("frame {n} is not in the index")));
        };
        if self.last_pts == Some(target.pts) {
            return self.output(false);
        }
        let reachable = self.positioned
            && !self.eof_sent
            && self.last_pts.is_none_or(|p| p < target.pts)
            && self.next_idx.is_some_and(|i| target.start_idx <= i);
        if reachable {
            self.stats.continued += 1;
        } else {
            self.reposition(target.start_idx)?;
        }
        match self.decode(Want::Pts(target.pts))? {
            Some(frame) => Ok(frame),
            None => Err(self.corrupt(format!(
                "frame {n} (pts {}) could not be decoded and no frame precedes it",
                target.pts
            ))),
        }
    }

    /// Positions the demuxer so that the next packet fed is keyframe packet `k`, the decoder
    /// flushed.
    fn reposition(&mut self, k: usize) -> Result<(), MediaError> {
        self.stats.seeks += 1;
        let earlier: Vec<usize> = self.index.keys_before(k).take(LANDING_RETRIES).collect();
        for key in std::iter::once(k).chain(earlier) {
            self.cancelled()?;
            let ts = self.index.seek_ts(key);
            if sys::seek_stream(&mut self.input, self.stream, ts).is_ok() && self.land_at(k)? {
                return Ok(());
            }
            self.stats.landing_retries += 1;
        }
        // Last resort: reopen the file and read from its start.
        self.stats.reopens += 1;
        tracing::debug!(
            target: "cutweave_media",
            "{}: demuxer seeks never landed at or before keyframe packet {k}; reading from the start",
            shown(&self.path)
        );
        let opened = open_stream(&self.path, &self.index, self.threads)?;
        self.input = opened.input;
        self.decoder = opened.decoder;
        self.eof_sent = false;
        if self.land_at(k)? {
            return Ok(());
        }
        Err(self.corrupt(format!(
            "keyframe packet {k} of the index is not in the file (the file changed?)"
        )))
    }

    /// After a demuxer seek: flushes the decoder and reads up to packet `k`. `false` when the
    /// demuxer landed past `k`.
    fn land_at(&mut self, k: usize) -> Result<bool, MediaError> {
        if self.eof_sent {
            // FFmpeg 8.1's HEVC decoder (slice/no threads) keeps a frame queued after EOF across
            // `avcodec_flush_buffers` and emits it after the next seek (S3 F7): drain first.
            for _ in 0..1024 {
                self.cancelled()?;
                if self.decoder.receive_frame(&mut self.scratch).is_err() {
                    break;
                }
            }
        }
        self.decoder.flush();
        self.pending = None;
        self.positioned = false;
        self.eof_sent = false;
        self.last_pts = None;
        self.next_idx = None;
        let mut unknown = 0u64;
        loop {
            self.cancelled()?;
            let Some(pkt) = self.read_packet() else {
                return Ok(false);
            };
            match self.index.locate(pkt.dts(), pkt.pts(), position(&pkt)) {
                Some(i) if i < k => {}
                Some(i) if i == k => {
                    self.pending = Some(pkt);
                    self.next_idx = Some(k);
                    self.positioned = true;
                    return Ok(true);
                }
                Some(_) => return Ok(false),
                None => {
                    unknown += 1;
                    if unknown > MAX_UNKNOWN_PACKETS {
                        return Ok(false);
                    }
                }
            }
        }
    }

    /// The next packet of the stream; `None` at the end (or a terminal demux error: truncated
    /// files end in I/O errors, and the frames past it surface as damage).
    fn read_packet(&mut self) -> Option<Packet> {
        let mut corrupt = 0u32;
        loop {
            let mut pkt = Packet::empty();
            match pkt.read(&mut self.input) {
                Ok(()) if pkt.stream() == self.stream => return Some(pkt),
                Ok(()) => {}
                Err(Error::Eof) => return None,
                Err(Error::InvalidData) => {
                    corrupt += 1;
                    if corrupt > MAX_DEMUX_ERRORS {
                        return None;
                    }
                }
                Err(e) => {
                    tracing::debug!(
                        target: "cutweave_media",
                        "{}: reading stopped: {e}",
                        shown(&self.path)
                    );
                    return None;
                }
            }
        }
    }

    /// The decode loop of sequential decoding and seeking. `Ok(None)`: the stream ended (for a
    /// seek: with no frame before the target to stand in for it).
    fn decode(&mut self, want: Want) -> Result<Option<VideoFrame>, MediaError> {
        let mut packets = 0u64;
        let mut errors = 0u64;
        loop {
            // Everything the decoder has ready.
            loop {
                match self.decoder.receive_frame(&mut self.scratch) {
                    Ok(()) => {
                        let Some(pts) = self.scratch.pts().or_else(|| self.scratch.timestamp())
                        else {
                            continue; // a frame that cannot be placed is no answer
                        };
                        std::mem::swap(&mut self.frame, &mut self.scratch);
                        self.last_pts = Some(pts);
                        match want {
                            Want::Next => return self.output(false).map(Some),
                            Want::Pts(t) if pts == t => return self.output(false).map(Some),
                            Want::Pts(t) if pts > t => {
                                tracing::warn!(
                                    target: "cutweave_media",
                                    "{}: frame at pts {t} is damaged; showing pts {pts}",
                                    shown(&self.path)
                                );
                                return self.output(true).map(Some);
                            }
                            Want::Pts(_) => {}
                        }
                    }
                    Err(Error::Other { errno }) if errno == EAGAIN => break,
                    Err(Error::Eof) => return self.end_of_stream(want),
                    Err(Error::InvalidData) => {
                        self.stats.decode_errors += 1;
                        errors += 1;
                        if errors > MAX_DECODE_ERRORS {
                            return Err(self.corrupt("too many decode errors".to_owned()));
                        }
                    }
                    Err(e) => return Err(media_error(&self.path, e)),
                }
            }
            if self.eof_sent {
                // Draining after EOF never answers EAGAIN; guard anyway.
                return self.end_of_stream(want);
            }
            self.cancelled()?;
            packets += 1;
            if packets > MAX_PACKETS_PER_REQUEST {
                return Err(self.corrupt(format!("no frame after {packets} packets")));
            }
            let pkt = match self.pending.take() {
                Some(p) => Some(p),
                None => self.read_packet(),
            };
            let Some(pkt) = pkt else {
                self.eof_sent = true;
                self.next_idx = None;
                match self.decoder.send_eof() {
                    Ok(()) | Err(Error::Eof) => {}
                    Err(e) => return Err(media_error(&self.path, e)),
                }
                continue;
            };
            if let Some(n) = self.next_idx {
                let at = self.index.locate(pkt.dts(), pkt.pts(), position(&pkt));
                if at != Some(n) {
                    self.stats.index_mismatches += 1;
                }
                self.next_idx = Some(at.unwrap_or(n) + 1);
            }
            let skip = match want {
                Want::Pts(t) => pkt.pts().or(pkt.dts()).is_some_and(|p| p < t),
                Want::Next => false,
            };
            self.decoder.skip_frame(if skip {
                Discard::NonReference
            } else {
                Discard::Default
            });
            match self.decoder.send_packet(&pkt) {
                Ok(()) => {}
                Err(Error::Other { errno }) if errno == EAGAIN => self.pending = Some(pkt),
                // Damaged data: FFmpeg conceals and goes on.
                Err(_) => {
                    self.stats.decode_errors += 1;
                    errors += 1;
                    if errors > MAX_DECODE_ERRORS {
                        return Err(self.corrupt("too many decode errors".to_owned()));
                    }
                }
            }
        }
    }

    /// The stream ended before `want` was found.
    fn end_of_stream(&mut self, want: Want) -> Result<Option<VideoFrame>, MediaError> {
        self.positioned = false;
        match (want, self.last_pts) {
            (Want::Pts(t), Some(p)) if p < t => {
                tracing::warn!(
                    target: "cutweave_media",
                    "{}: the stream ends before pts {t}; showing pts {p}",
                    shown(&self.path)
                );
                self.output(true).map(Some)
            }
            _ => Ok(None),
        }
    }

    /// `frame` as a [`VideoFrame`]; `substituted` when it stands in for another frame.
    fn output(&mut self, substituted: bool) -> Result<VideoFrame, MediaError> {
        let ticks = self
            .last_pts
            .ok_or_else(|| self.corrupt("no decoded frame".to_owned()))?;
        let pts = self
            .index
            .time_base()
            .to_time(ticks)
            .ok_or_else(|| self.corrupt(format!("pts {ticks} is out of range")))?;
        let damaged = substituted || self.frame.is_corrupt() || self.frame.has_decode_errors();
        if damaged {
            self.stats.damaged += 1;
        }
        let fc = sys::frame_color(&self.frame);
        let own = color_tags(
            fc.primaries.as_deref(),
            fc.transfer.as_deref(),
            fc.matrix.as_deref(),
            fc.range,
        );
        let color = ColorTags {
            primaries: own
                .primaries
                .or_else(|| self.stream_color.primaries.clone()),
            transfer: own.transfer.or_else(|| self.stream_color.transfer.clone()),
            matrix: own.matrix.or_else(|| self.stream_color.matrix.clone()),
            range: own.range.or(self.stream_color.range),
        };
        self.out
            .convert(&self.frame, pts, color, damaged)
            .map_err(|e| match e {
                Fault::Ffmpeg(e) => media_error(&self.path, e),
                Fault::Media(e) => e,
            })
    }
}

impl VideoDecoder for FfmpegVideoDecoder {
    fn next_frame(&mut self) -> Result<Option<VideoFrame>, MediaError> {
        self.cancelled()?;
        self.decode(Want::Next)
    }

    fn seek_exact(&mut self, t: Time) -> Result<VideoFrame, MediaError> {
        self.cancelled()?;
        if self.end.is_some_and(|end| t >= end) {
            return Err(MediaError::OutOfRange { t });
        }
        let n = self.index.frame_at(t);
        self.seek_frame(n)
    }

    fn index(&self) -> Arc<FrameIndex> {
        Arc::clone(&self.index)
    }

    fn set_cancel(&mut self, cancel: Option<CancelToken>) {
        self.cancel = cancel;
    }
}

impl Drop for FfmpegVideoDecoder {
    fn drop(&mut self) {
        let s = self.stats;
        if s.seeks + s.continued > 0 {
            tracing::debug!(
                target: "cutweave_media",
                "{}: {} seeks ({} continued), {} landing retries, {} reopens, {} index \
                 mismatches, {} decode errors, {} damaged frames",
                shown(&self.path),
                s.seeks,
                s.continued,
                s.landing_retries,
                s.reopens,
                s.index_mismatches,
                s.decode_errors,
                s.damaged
            );
        }
    }
}

/// The byte position of a packet, -1 when unknown.
fn position(pkt: &Packet) -> i64 {
    i64::try_from(pkt.position()).unwrap_or(-1)
}

// ---------------------------------------------------------------------------------------------
// Output layouts
// ---------------------------------------------------------------------------------------------

enum Fault {
    Ffmpeg(Error),
    Media(MediaError),
}

/// The [`PixelFormat`] a decoder layout is returned in as is, if any.
fn native_layout(pixel: Pixel) -> Option<PixelFormat> {
    Some(match pixel {
        Pixel::YUV420P | Pixel::YUVJ420P => PixelFormat::Yuv420p,
        Pixel::NV12 => PixelFormat::Nv12,
        Pixel::P010LE => PixelFormat::P010,
        Pixel::YUV420P10LE => PixelFormat::Yuv420p10,
        Pixel::YUV422P | Pixel::YUVJ422P => PixelFormat::Yuv422p,
        Pixel::YUV422P10LE => PixelFormat::Yuv422p10,
        Pixel::RGBA => PixelFormat::Rgba8,
        _ => return None,
    })
}

/// FFmpeg's layout for a [`PixelFormat`].
fn pixel_of(format: PixelFormat) -> Pixel {
    match format {
        PixelFormat::Nv12 => Pixel::NV12,
        PixelFormat::P010 => Pixel::P010LE,
        PixelFormat::Yuv420p10 => Pixel::YUV420P10LE,
        PixelFormat::Yuv422p => Pixel::YUV422P,
        PixelFormat::Yuv422p10 => Pixel::YUV422P10LE,
        PixelFormat::Rgba8 => Pixel::RGBA,
        PixelFormat::Yuv420p => Pixel::YUV420P,
    }
}

/// The nearest [`PixelFormat`] for a layout without one: RGB, gray + alpha and anything with
/// alpha become `Rgba8`; Y'CbCr and gray keep their bit depth class (8 or 10 bits) and their
/// vertical chroma resolution (4:4:4 and 4:1:1 become 4:2:2, 4:1:0 becomes 4:2:0).
fn fallback_layout(pixel: Pixel) -> Option<PixelFormat> {
    let d = sys::pix_desc(pixel)?;
    if d.opaque {
        return None;
    }
    if d.rgb || d.alpha {
        return Some(PixelFormat::Rgba8);
    }
    let full_height_chroma = d.components >= 3 && d.log2_chroma_h == 0;
    Some(match (full_height_chroma, d.depth > 8) {
        (true, false) => PixelFormat::Yuv422p,
        (true, true) => PixelFormat::Yuv422p10,
        (false, false) => PixelFormat::Yuv420p,
        (false, true) => PixelFormat::Yuv420p10,
    })
}

/// swscale's matrix (`SWS_CS_*`) for Y'CbCr tagged `matrix`; untagged pictures use BT.709 from
/// 720 lines up and BT.601 below (what players assume).
fn sws_matrix(matrix: Option<&MatrixCoefficients>, height: u32) -> c_int {
    use ffmpeg_next::ffi::{
        SWS_CS_BT2020, SWS_CS_FCC, SWS_CS_ITU601, SWS_CS_ITU709, SWS_CS_SMPTE240M,
    };
    let cs = match matrix {
        Some(MatrixCoefficients::Bt709) => SWS_CS_ITU709,
        Some(MatrixCoefficients::Fcc) => SWS_CS_FCC,
        Some(MatrixCoefficients::Bt470Bg | MatrixCoefficients::Smpte170M) => SWS_CS_ITU601,
        Some(MatrixCoefficients::Smpte240M) => SWS_CS_SMPTE240M,
        Some(MatrixCoefficients::Bt2020Ncl | MatrixCoefficients::Bt2020Cl) => SWS_CS_BT2020,
        _ if height >= 720 => SWS_CS_ITU709,
        _ => SWS_CS_ITU601,
    };
    c_int::try_from(cs).unwrap_or(5)
}

/// A swscale conversion for one source layout, size and color.
struct Converter {
    key: (Pixel, u32, u32, c_int, bool, PixelFormat),
    scaler: Scaler,
    out: frame::Video,
}

/// How decoded frames become the returned layout.
struct Output {
    target: Option<PixelFormat>,
    converter: Option<Converter>,
}

impl Output {
    fn new(target: Option<PixelFormat>) -> Output {
        Output {
            target,
            converter: None,
        }
    }

    fn convert(
        &mut self,
        src: &frame::Video,
        pts: Time,
        mut color: ColorTags,
        damaged: bool,
    ) -> Result<VideoFrame, Fault> {
        let pixel = src.format();
        let (width, height) = (src.width(), src.height());
        if width == 0 || height == 0 {
            return Err(Fault::Media(MediaError::Backend {
                message: format!("the decoder returned an empty {width}x{height} frame"),
            }));
        }
        if matches!(pixel, Pixel::YUVJ420P | Pixel::YUVJ422P) && color.range.is_none() {
            color.range = Some(ColorRange::Full);
        }
        let native = native_layout(pixel);
        let want = match (self.target, native) {
            (Some(t), _) => t,
            (None, Some(n)) => n,
            (None, None) => fallback_layout(pixel).ok_or_else(|| {
                Fault::Media(MediaError::Unsupported {
                    message: format!(
                        "decoded frames in `{}` cannot be read",
                        pixel.descriptor().map_or("?", |d| d.name())
                    ),
                })
            })?,
        };
        let planes = if native == Some(want) {
            copy_planes(src, want)?
        } else {
            let src_full = color.range == Some(ColorRange::Full);
            let dst_full = if want.is_yuv() { src_full } else { true };
            let matrix = sws_matrix(color.matrix.as_ref(), height);
            let key = (pixel, width, height, matrix, src_full, want);
            if self.converter.as_ref().is_none_or(|c| c.key != key) {
                self.converter = Some(Converter {
                    key,
                    scaler: Scaler::new(
                        pixel,
                        pixel_of(want),
                        (width, height),
                        matrix,
                        (src_full, dst_full),
                    )
                    .map_err(Fault::Ffmpeg)?,
                    out: frame::Video::empty(),
                });
            }
            let Some(c) = self.converter.as_mut() else {
                unreachable!("set above")
            };
            c.scaler.run(src, &mut c.out).map_err(Fault::Ffmpeg)?;
            if !want.is_yuv() {
                color.matrix = Some(MatrixCoefficients::Rgb);
                color.range = Some(ColorRange::Full);
            }
            copy_planes(&c.out, want)?
        };
        Ok(VideoFrame {
            pts,
            width,
            height,
            format: want,
            planes,
            color,
            damaged,
        })
    }
}

/// The planes of `src` (in `layout`) as tight rows.
fn copy_planes(src: &frame::Video, layout: PixelFormat) -> Result<Vec<Plane>, Fault> {
    let (width, height) = (src.width(), src.height());
    (0..layout.plane_count())
        .map(|i| {
            let (w, h, bpt) = layout.plane_layout(i, width, height);
            let row = w as usize * bpt as usize;
            let data = sys::copy_plane(src, i, row, h as usize).ok_or_else(|| {
                Fault::Media(MediaError::Backend {
                    message: format!("plane {i} of a decoded {layout:?} frame is too small"),
                })
            })?;
            Ok(Plane {
                data,
                stride: u32::try_from(row).unwrap_or(u32::MAX),
            })
        })
        .collect()
}
