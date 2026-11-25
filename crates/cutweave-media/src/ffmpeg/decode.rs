// SPDX-License-Identifier: GPL-3.0-or-later
//! The FFmpeg [`VideoDecoder`] (WP-1D.03): frame-exact seek from the packet-scan index, software
//! decode to the decoder's own planes, damage handling and cancellation. The port of the
//! validated Spike S3 source (`spikes/s3-decode/src/source.rs`, findings F3, F4, F5, F7).
//! Hardware decode with software fallback is WP-1D.04 (see **Hardware** below and `hwdec`).
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
//! conversion, arch §7); other layouts, an explicit `target_pixfmt` (`Rgba8` thumbnails) and an
//! explicit `target_size` (proxies, WP-2F.01) go through swscale with the stream's matrix and
//! range.
//!
//! **Hardware** (WP-1D.04, S3 H1-H10): under [`HwMode::Auto`] an eligible stream decodes with a
//! device `hwdec` verified for its codec, on one thread (S3 H3: frame threads only add delay with
//! a hwaccel). The hwaccel only replaces slice decoding inside libavcodec, so the seek rule,
//! landing checks, NONREF skipping and drain-before-flush above apply unchanged. Frames dropped
//! on the way to T never leave the GPU; the one returned is copied to system memory as NV12 or
//! P010. Any hardware failure (the device or hwaccel does not initialise, a decode call fails,
//! frames come out in software, the copy fails) makes the decoder log a warning, set
//! `hw_failed`, reopen its codec in software with the software threading, and run the request
//! again from its keyframe; sequential decoding resumes after the last frame it returned. So
//! does a seek that lands on an H.264 keyframe that is not an IDR picture on a device not known
//! to be exact from one (S3 H10, `hwdec`).

use std::ffi::c_int;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cutweave_model::{ColorRange, ColorTags, MatrixCoefficients, Time};
use ffmpeg_next::codec::{self, threading};
use ffmpeg_next::format::Pixel;
use ffmpeg_next::format::context::Input;
use ffmpeg_next::media::Type;
use ffmpeg_next::util::error::EAGAIN;
use ffmpeg_next::{Discard, Error, Packet, Rational, decoder, frame};

use super::error::media_error;
use super::hwdec::{self, Backing, Choice, StreamClass};
use super::sys::{self, HwDecode, Scaler};
use super::{index, input, probe};
use crate::backend::{DecodeMode, DecodeOpts, HwMode, VideoDecoder};
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
/// Hardware decoders use one thread (S3 H3).
const HW_THREADS: threading::Config = threading::Config {
    kind: threading::Type::Frame,
    count: 1,
};

/// Opens a decoder for the main video stream of `path`, in hardware when `opts.hw` and `policy`
/// allow it and the stream is eligible (`hwdec`).
pub(super) fn open(
    path: &Path,
    opts: &DecodeOpts,
    policy: &hwdec::Policy,
) -> Result<Box<dyn VideoDecoder>, MediaError> {
    let index = match &opts.index {
        Some(index) => Arc::clone(index),
        None => Arc::new(index::index_media(path)?),
    };
    let (input, stream, info) = open_input(path, &index)?;
    let choice = match opts.hw {
        HwMode::Off => None,
        HwMode::Auto => {
            let mut class = info.class.clone();
            class.intra_only = index.max_decode_distance() <= 1;
            match hwdec::choose(policy, &class) {
                Ok(choice) => Some(choice),
                Err(why) => {
                    tracing::debug!(
                        target: "cutweave_media",
                        "{}: software decode: {why}",
                        shown(path)
                    );
                    None
                }
            }
        }
    };
    open_opened(path, opts, index, (input, stream, info), choice)
}

/// Tests: a decoder on `choice` whatever the stream's eligibility (real init failures).
#[cfg(test)]
pub(super) fn open_with_choice(
    path: &Path,
    opts: &DecodeOpts,
    choice: Choice,
) -> Result<Box<dyn VideoDecoder>, MediaError> {
    let index = match &opts.index {
        Some(index) => Arc::clone(index),
        None => Arc::new(index::index_media(path)?),
    };
    let opened = open_input(path, &index)?;
    open_opened(path, opts, index, opened, Some(choice))
}

/// The decoder on an opened input, in hardware on `choice` if given.
fn open_opened(
    path: &Path,
    opts: &DecodeOpts,
    index: Arc<FrameIndex>,
    (input, stream, info): (Input, usize, StreamInfo),
    choice: Option<Choice>,
) -> Result<Box<dyn VideoDecoder>, MediaError> {
    let sw_threads = threading_for(opts, &index);
    let end = index.end_time();
    let mut d = FfmpegVideoDecoder {
        path: path.to_path_buf(),
        input,
        codec: None,
        stream,
        time_base: info.time_base,
        nal_length: info.nal_length,
        sw_threads,
        hw: choice,
        hw_failed: false,
        end,
        index,
        frame: frame::Video::empty(),
        scratch: frame::Video::empty(),
        sw: frame::Video::empty(),
        out: Output::new(opts.target_pixfmt, opts.target_size),
        stream_color: info.color,
        pending: None,
        next_idx: Some(0),
        positioned: true,
        eof_sent: false,
        last_pts: None,
        returned: None,
        sent: 0,
        cancel: None,
        stats: Stats::default(),
    };
    d.codec = Some(d.open_codec()?);
    if let Some(c) = &d.hw {
        tracing::debug!(
            target: "cutweave_media",
            "{}: hardware decode with {} on {}{}",
            shown(path),
            c.implementation,
            c.device,
            if c.idr_starts_only {
                " (seeks must land on IDR pictures)"
            } else {
                ""
            }
        );
    }
    Ok(Box::new(d))
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

/// What opening a decoder needs to know about its stream.
struct StreamInfo {
    time_base: Rational,
    color: ColorTags,
    class: StreamClass,
    /// H.264 `avcC` NAL length size (`None`: Annex B, or not H.264).
    nal_length: Option<usize>,
}

/// Opens the container, checks the index's stream against it and makes the demuxer skip every
/// other stream.
fn open_input(path: &Path, index: &FrameIndex) -> Result<(Input, usize, StreamInfo), MediaError> {
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
    let info = {
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
        let nal_length = if p.codec.as_deref() == Some("h264") {
            sys::avc_nal_length_size(&par)
        } else {
            None
        };
        StreamInfo {
            time_base: tb,
            nal_length,
            color: color_tags(
                p.primaries.as_deref(),
                p.transfer.as_deref(),
                p.matrix.as_deref(),
                p.range,
            ),
            class: StreamClass {
                codec: p.codec,
                pixel_format: p.pixel_format,
                width: p.width,
                height: p.height,
                intra_only: false,
            },
        }
    };
    sys::discard_other_streams(&mut input, stream);
    Ok((input, stream, info))
}

/// An open libavcodec decoder and, in hardware, the state its `get_format` callback reads. The
/// decoder is declared first so it is dropped before that state.
struct Codec {
    decoder: decoder::Video,
    hw: Option<HwDecode>,
}

/// Opens a decoder for `stream` of `input`, with `hw`'s device attached when it has one.
fn open_codec(
    path: &Path,
    input: &Input,
    stream: usize,
    time_base: Rational,
    threads: threading::Config,
    hw: Option<&Choice>,
) -> Result<Codec, MediaError> {
    let st = input.stream(stream).ok_or_else(|| MediaError::Backend {
        message: format!("`{}`: stream {stream} went away", shown(path)),
    })?;
    let mut ctx =
        codec::Context::from_parameters(st.parameters()).map_err(|e| media_error(path, e))?;
    ctx.set_threading(threads);
    let hw = match hw.map(|c| &c.backing) {
        Some(Backing::Device {
            device,
            surface_format,
        }) => Some(
            HwDecode::attach(&mut ctx, device, *surface_format).map_err(|e| {
                MediaError::Backend {
                    message: format!("attaching the hardware device failed: {e}"),
                }
            })?,
        ),
        #[cfg(test)]
        Some(Backing::Pseudo(faults)) if faults.open => {
            return Err(MediaError::Backend {
                message: "the hardware device did not open (injected)".to_owned(),
            });
        }
        _ => None,
    };
    let mut dec = ctx.decoder();
    dec.set_packet_time_base(time_base);
    let decoder = dec.video().map_err(|e| match e {
        Error::DecoderNotFound => MediaError::Unsupported {
            message: format!("`{}`: no decoder for its video codec", shown(path)),
        },
        e => media_error(path, e),
    })?;
    Ok(Codec { decoder, hw })
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

/// Why a decode step failed: an error for the caller, or the hardware decoder, which the
/// caller handles by going over to software and trying again.
enum Fail {
    Media(MediaError),
    Hardware(String),
}

impl From<MediaError> for Fail {
    fn from(e: MediaError) -> Fail {
        Fail::Media(e)
    }
}

impl Fail {
    /// The error for the caller (a hardware failure after the fallback is a backend fault).
    fn into_media(self) -> MediaError {
        match self {
            Fail::Media(e) => e,
            Fail::Hardware(why) => MediaError::Backend {
                message: format!("hardware decode failed after the software fallback: {why}"),
            },
        }
    }
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
    /// Always `Some` once opened (an `Option` only so `open` can build it with `self`).
    codec: Option<Codec>,
    stream: usize,
    time_base: Rational,
    /// H.264 `avcC` NAL length size (`None`: Annex B, or not H.264).
    nal_length: Option<usize>,
    /// Threading of a software decoder for this stream and mode.
    sw_threads: threading::Config,
    /// The hardware decoder in use; `None` in software.
    hw: Option<Choice>,
    /// The decoder went over from hardware to software after a failure.
    hw_failed: bool,
    index: Arc<FrameIndex>,
    /// Seeks at or past this are out of range (`None`: a single picture, never).
    end: Option<Time>,
    /// The latest frame the decoder produced; valid while `last_pts` is `Some`.
    frame: frame::Video,
    /// Receive target, swapped with `frame` on success (a failed receive unrefs its target).
    scratch: frame::Video,
    /// The system-memory copy of a hardware `frame` (buffers reused from copy to copy).
    sw: frame::Video,
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
    /// pts (ticks) of the last frame returned.
    returned: Option<i64>,
    /// Packets sent to a decoder since open.
    sent: u64,
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

    fn codec(&mut self) -> Result<&mut Codec, MediaError> {
        self.codec.as_mut().ok_or_else(not_open)
    }

    /// A codec for the decoder's stream: in hardware while `hw` is set, else in software. A
    /// hardware open that fails goes over to software (see [`fall_back`](Self::fall_back)).
    fn open_codec(&mut self) -> Result<Codec, MediaError> {
        if let Some(choice) = &self.hw {
            match open_codec(
                &self.path,
                &self.input,
                self.stream,
                self.time_base,
                HW_THREADS,
                Some(choice),
            ) {
                Ok(codec) => return Ok(codec),
                Err(e) => self.mark_failed(&e.to_string()),
            }
        }
        open_codec(
            &self.path,
            &self.input,
            self.stream,
            self.time_base,
            self.sw_threads,
            None,
        )
    }

    /// Leaves hardware decoding for good: logs why and sets `hw_failed`.
    fn mark_failed(&mut self, why: &str) {
        let Some(choice) = self.hw.take() else {
            return;
        };
        self.hw_failed = true;
        tracing::warn!(
            target: "cutweave_media",
            "{}: hardware decode ({} on {}) failed: {why}; decoding in software from here",
            shown(&self.path),
            choice.implementation,
            choice.device
        );
    }

    /// After a hardware failure: a software codec in place of the hardware one, the decoder
    /// unpositioned (the next request seeks).
    fn fall_back(&mut self, why: &str) -> Result<(), MediaError> {
        if self.hw.is_none() {
            return Err(Fail::Hardware(why.to_owned()).into_media());
        }
        self.mark_failed(why);
        // The hardware decoder goes first, then its frames.
        self.codec = None;
        self.frame = frame::Video::empty();
        self.scratch = frame::Video::empty();
        self.codec = Some(self.open_codec()?);
        self.pending = None;
        self.positioned = false;
        self.eof_sent = false;
        self.last_pts = None;
        self.next_idx = None;
        Ok(())
    }

    /// On a device that must start H.264 decoding at IDR pictures (S3 H10): the keyframe a
    /// reposition just landed on (`pending`) must be one.
    fn check_idr_landing(&self) -> Result<(), Fail> {
        if !self.hw.as_ref().is_some_and(|c| c.idr_starts_only) {
            return Ok(());
        }
        let idr = self
            .pending
            .as_ref()
            .and_then(|p| p.data())
            .and_then(|data| hwdec::h264_idr(data, self.nal_length));
        if idr == Some(false) {
            return Err(Fail::Hardware(
                "the stream has H.264 keyframes that are not IDR pictures, which this GPU \
                 decoder does not decode exactly after a seek (S3 H10)"
                    .to_owned(),
            ));
        }
        Ok(())
    }

    /// Frame-exact seek to presentation index `n`.
    fn seek_frame(&mut self, n: usize) -> Result<VideoFrame, Fail> {
        let Some(&target) = self.index.frames().get(n) else {
            return Err(self
                .corrupt(format!("frame {n} is not in the index"))
                .into());
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
            self.check_idr_landing()?;
        }
        match self.decode(Want::Pts(target.pts))? {
            Some(frame) => Ok(frame),
            None => Err(self
                .corrupt(format!(
                    "frame {n} (pts {}) could not be decoded and no frame precedes it",
                    target.pts
                ))
                .into()),
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
        let (input, _, _) = open_input(&self.path, &self.index)?;
        self.codec = None;
        self.input = input;
        self.codec = Some(self.open_codec()?);
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
                let codec = self.codec.as_mut().ok_or_else(not_open)?;
                if codec.decoder.receive_frame(&mut self.scratch).is_err() {
                    break;
                }
            }
        }
        self.codec()?.decoder.flush();
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

    /// A decode call failing with `e`: in hardware a hardware failure (damaged data, which both
    /// paths report as `InvalidData`, is handled by the caller), else an error.
    fn decode_failure(&self, e: Error) -> Fail {
        if self.hw.is_some() {
            Fail::Hardware(format!("decoding failed: {e}"))
        } else {
            Fail::Media(media_error(&self.path, e))
        }
    }

    /// In hardware: whether libavcodec went over to software for this stream on its own (the
    /// hwaccel did not initialise, or the stream's format has none).
    fn check_negotiation(&self) -> Result<(), Fail> {
        match self.codec.as_ref().and_then(|c| c.hw.as_ref()) {
            Some(hw) if hw.fell_back() => Err(Fail::Hardware(
                if hw.was_offered() {
                    "the hardware decoder did not initialise for this stream"
                } else {
                    "the hardware has no decoder for this stream's format"
                }
                .to_owned(),
            )),
            _ => Ok(()),
        }
    }

    /// In hardware: `scratch` (just received) must be a hardware surface.
    fn check_surface(&self) -> Result<(), Fail> {
        match self.codec.as_ref().and_then(|c| c.hw.as_ref()) {
            Some(hw) if !hw.is_surface(&self.scratch) => Err(Fail::Hardware(
                "frames came out of the hardware decoder in software".to_owned(),
            )),
            _ => Ok(()),
        }
    }

    /// Tests: a pseudo hardware decoder failing at this packet.
    #[cfg(test)]
    fn injected_packet_fault(&self) -> Result<(), Fail> {
        match &self.hw {
            Some(Choice {
                backing: Backing::Pseudo(f),
                ..
            }) if f.packet == Some(self.sent) => Err(Fail::Hardware(format!(
                "device lost at packet {} (injected)",
                self.sent
            ))),
            _ => Ok(()),
        }
    }

    /// The decode loop of sequential decoding and seeking. `Ok(None)`: the stream ended (for a
    /// seek: with no frame before the target to stand in for it).
    fn decode(&mut self, want: Want) -> Result<Option<VideoFrame>, Fail> {
        let mut packets = 0u64;
        let mut errors = 0u64;
        loop {
            // Everything the decoder has ready.
            loop {
                let codec = self.codec.as_mut().ok_or_else(not_open)?;
                let received = codec.decoder.receive_frame(&mut self.scratch);
                match received {
                    Ok(()) => {
                        self.check_surface()?;
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
                    Err(Error::InvalidData) => self.damaged_data(&mut errors)?,
                    Err(e) => return Err(self.decode_failure(e)),
                }
            }
            if self.eof_sent {
                // Draining after EOF never answers EAGAIN; guard anyway.
                return self.end_of_stream(want);
            }
            self.cancelled()?;
            packets += 1;
            if packets > MAX_PACKETS_PER_REQUEST {
                return Err(self
                    .corrupt(format!("no frame after {packets} packets"))
                    .into());
            }
            let pkt = match self.pending.take() {
                Some(p) => Some(p),
                None => self.read_packet(),
            };
            let Some(pkt) = pkt else {
                self.eof_sent = true;
                self.next_idx = None;
                match self.codec()?.decoder.send_eof() {
                    Ok(()) | Err(Error::Eof) => {}
                    Err(e) => return Err(self.decode_failure(e)),
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
            self.send(pkt, skip, &mut errors)?;
        }
    }

    /// Sends `pkt` to the decoder, skipping non-reference pictures when `skip`; a packet the
    /// decoder refuses for now is kept for the next call.
    fn send(&mut self, pkt: Packet, skip: bool, errors: &mut u64) -> Result<(), Fail> {
        #[cfg(test)]
        self.injected_packet_fault()?;
        self.sent += 1;
        let decoder = &mut self.codec()?.decoder;
        decoder.skip_frame(if skip {
            Discard::NonReference
        } else {
            Discard::Default
        });
        match decoder.send_packet(&pkt) {
            Ok(()) => {}
            Err(Error::Other { errno }) if errno == EAGAIN => self.pending = Some(pkt),
            // Damaged data: FFmpeg conceals and goes on (in both paths).
            Err(Error::InvalidData) => self.damaged_data(errors)?,
            Err(e) if self.hw.is_some() => return Err(self.decode_failure(e)),
            Err(_) => self.damaged_data(errors)?,
        }
        self.check_negotiation()
    }

    /// Counts a decode error on damaged data (FFmpeg conceals it); too many end the request.
    fn damaged_data(&mut self, errors: &mut u64) -> Result<(), Fail> {
        self.stats.decode_errors += 1;
        *errors += 1;
        if *errors > MAX_DECODE_ERRORS {
            return Err(self.corrupt("too many decode errors".to_owned()).into());
        }
        Ok(())
    }

    /// The stream ended before `want` was found.
    fn end_of_stream(&mut self, want: Want) -> Result<Option<VideoFrame>, Fail> {
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

    /// `frame` as a [`VideoFrame`]; `substituted` when it stands in for another frame. A
    /// hardware surface is copied to system memory first.
    fn output(&mut self, substituted: bool) -> Result<VideoFrame, Fail> {
        let ticks = self
            .last_pts
            .ok_or_else(|| self.corrupt("no decoded frame".to_owned()))?;
        let pts = self
            .index
            .time_base()
            .to_time(ticks)
            .ok_or_else(|| self.corrupt(format!("pts {ticks} is out of range")))?;
        let damaged = substituted || self.frame.is_corrupt() || self.frame.has_decode_errors();
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
        #[cfg(test)]
        if let Some(Choice {
            backing: Backing::Pseudo(f),
            ..
        }) = &self.hw
            && f.transfer
        {
            return Err(Fail::Hardware(
                "copying the frame to memory failed (injected)".to_owned(),
            ));
        }
        let surface = self
            .codec
            .as_ref()
            .and_then(|c| c.hw.as_ref())
            .is_some_and(|hw| hw.is_surface(&self.frame));
        if surface {
            sys::transfer_surface(&self.frame, &mut self.sw)
                .map_err(|e| Fail::Hardware(format!("copying the frame to memory failed: {e}")))?;
        }
        let src = if surface { &self.sw } else { &self.frame };
        let frame = self
            .out
            .convert(src, pts, color, damaged)
            .map_err(|e| match e {
                Fault::Ffmpeg(e) => media_error(&self.path, e),
                Fault::Media(e) => e,
            })?;
        if damaged {
            self.stats.damaged += 1;
        }
        self.returned = Some(ticks);
        Ok(frame)
    }

    /// After a hardware failure in sequential decoding: the frame after the last one returned,
    /// decoded in software (`None` past the end).
    fn resume(&mut self) -> Result<Option<VideoFrame>, MediaError> {
        let n = match self.returned {
            None => 0,
            Some(p) => match self.index.frame_by_pts(p) {
                Some(i) => i + 1,
                None => self
                    .index
                    .time_base()
                    .to_time(p)
                    .map_or(0, |t| self.index.frame_at(t) + 1),
            },
        };
        if n >= self.index.frame_count() {
            return Ok(None);
        }
        self.seek_frame(n).map(Some).map_err(Fail::into_media)
    }
}

impl VideoDecoder for FfmpegVideoDecoder {
    fn next_frame(&mut self) -> Result<Option<VideoFrame>, MediaError> {
        self.cancelled()?;
        match self.decode(Want::Next) {
            Err(Fail::Hardware(why)) => {
                self.fall_back(&why)?;
                self.resume()
            }
            other => other.map_err(Fail::into_media),
        }
    }

    fn seek_exact(&mut self, t: Time) -> Result<VideoFrame, MediaError> {
        self.cancelled()?;
        if self.end.is_some_and(|end| t >= end) {
            return Err(MediaError::OutOfRange { t });
        }
        let n = self.index.frame_at(t);
        match self.seek_frame(n) {
            Err(Fail::Hardware(why)) => {
                self.fall_back(&why)?;
                self.seek_frame(n).map_err(Fail::into_media)
            }
            other => other.map_err(Fail::into_media),
        }
    }

    fn index(&self) -> Arc<FrameIndex> {
        Arc::clone(&self.index)
    }

    fn set_cancel(&mut self, cancel: Option<CancelToken>) {
        self.cancel = cancel;
    }

    fn is_hw(&self) -> bool {
        self.hw.is_some()
    }

    fn hw_failed(&self) -> bool {
        self.hw_failed
    }

    fn hw_decoder(&self) -> Option<&str> {
        self.hw.as_ref().map(|c| c.implementation.as_str())
    }
}

impl Drop for FfmpegVideoDecoder {
    fn drop(&mut self) {
        let s = self.stats;
        if s.seeks + s.continued > 0 {
            tracing::debug!(
                target: "cutweave_media",
                "{}: {} seeks ({} continued), {} landing retries, {} reopens, {} index \
                 mismatches, {} decode errors, {} damaged frames, {}",
                shown(&self.path),
                s.seeks,
                s.continued,
                s.landing_retries,
                s.reopens,
                s.index_mismatches,
                s.decode_errors,
                s.damaged,
                match (&self.hw, self.hw_failed) {
                    (Some(c), _) => format!("hardware ({})", c.implementation),
                    (None, true) => "software after a hardware failure".to_owned(),
                    (None, false) => "software".to_owned(),
                }
            );
        }
        // Close the decoder before the frames it handed out.
        self.codec = None;
    }
}

/// The error for a decoder whose codec is missing (only between a close and a reopen).
fn not_open() -> MediaError {
    MediaError::Backend {
        message: "the decoder is not open".to_owned(),
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

/// The key of a [`Converter`]: source layout and size, matrix, source range, target layout
/// and size.
type ConverterKey = (Pixel, (u32, u32), c_int, bool, PixelFormat, (u32, u32));

/// A swscale conversion for one source layout, size and color.
struct Converter {
    key: ConverterKey,
    scaler: Scaler,
    out: frame::Video,
}

/// How decoded frames become the returned layout and size.
struct Output {
    target: Option<PixelFormat>,
    size: Option<(u32, u32)>,
    converter: Option<Converter>,
}

impl Output {
    fn new(target: Option<PixelFormat>, size: Option<(u32, u32)>) -> Output {
        Output {
            target,
            size: size.filter(|(w, h)| *w > 0 && *h > 0),
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
        let out_size = self.size.unwrap_or((width, height));
        let planes = if native == Some(want) && out_size == (width, height) {
            copy_planes(src, want)?
        } else {
            let src_full = color.range == Some(ColorRange::Full);
            let dst_full = if want.is_yuv() { src_full } else { true };
            let matrix = sws_matrix(color.matrix.as_ref(), height);
            let key = (pixel, (width, height), matrix, src_full, want, out_size);
            if self.converter.as_ref().is_none_or(|c| c.key != key) {
                self.converter = Some(Converter {
                    key,
                    scaler: Scaler::scaling(
                        pixel,
                        pixel_of(want),
                        ((width, height), out_size),
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
            width: out_size.0,
            height: out_size.1,
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
