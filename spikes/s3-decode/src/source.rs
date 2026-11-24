// SPDX-License-Identifier: GPL-3.0-or-later
//! A seekable video source: frame-exact seek = keyframe seek + decode-forward, driven by the
//! packet-scan index (arch §7).
//!
//! Seek algorithm for presentation frame T (start keyframe K from the index):
//! 1. If the decoder is already positioned between K and T (scrubbing forward), keep decoding.
//! 2. Otherwise `avformat_seek_file(stream, ts <= ts(K))`, flush the decoder, and read packets,
//!    locating each in the index. Packets before K are dropped without decoding; if the demuxer
//!    landed after K, retry from the previous keyframe (then from the file start).
//! 3. Feed packets from K on. Frames shown before T are discarded; optionally their packets are
//!    sent with `skip_frame = AVDISCARD_NONREF`, so non-reference B-frames are not even decoded.
//!    The first frame with pts == T is the answer; a frame past T is an `Overshoot` error.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use ffmpeg::codec::threading;
use ffmpeg::util::error::EAGAIN;
use ffmpeg_next as ffmpeg;

use crate::error::{Error, Result, ff};
use crate::ffi;
use crate::hw::{self, HwSpec, HwStatus};
use crate::index::FrameIndex;

/// Decoder threading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Threads {
    /// Frame threading, FFmpeg picks the count (cores + 1, capped at 16).
    Frame(usize),
    /// Slice threading only (lower latency, less parallelism for most H.264/HEVC streams).
    Slice(usize),
    /// One thread.
    Single,
}

impl Threads {
    fn config(self) -> threading::Config {
        match self {
            Threads::Frame(n) => threading::Config {
                kind: threading::Type::Frame,
                count: n,
            },
            Threads::Slice(n) => threading::Config {
                kind: threading::Type::Slice,
                count: n,
            },
            Threads::Single => threading::Config {
                kind: threading::Type::None,
                count: 1,
            },
        }
    }

    /// Parses `frame`, `frame:N`, `slice`, `slice:N` or `single`.
    pub fn parse(s: &str) -> Option<Self> {
        let (kind, n) = s
            .split_once(':')
            .map_or((s, 0), |(k, n)| (k, n.parse().unwrap_or(0)));
        match kind {
            "frame" => Some(Threads::Frame(n)),
            "slice" => Some(Threads::Slice(n)),
            "single" => Some(Threads::Single),
            _ => None,
        }
    }

    /// Short label for tables.
    pub fn label(self) -> String {
        match self {
            Threads::Frame(0) => "frame".into(),
            Threads::Frame(n) => format!("frame:{n}"),
            Threads::Slice(0) => "slice".into(),
            Threads::Slice(n) => format!("slice:{n}"),
            Threads::Single => "single".into(),
        }
    }
}

/// Decode options.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub threads: Threads,
    /// Send packets shown before the target with `AVDISCARD_NONREF` (skip non-reference frames).
    pub skip_nonref: bool,
    /// Keep decoding forward instead of seeking when the decoder is already inside the target's
    /// reach (scrubbing). Off for pure seek measurements.
    pub forward_continue: bool,
    /// Hardware decode (WP-0E.02): `None` = software. Falls back to software when the device or
    /// the hwaccel for this stream is unavailable (see `hw.rs`).
    pub hw: Option<HwSpec>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            threads: Threads::Frame(0),
            skip_nonref: true,
            forward_continue: false,
            hw: None,
        }
    }
}

impl Options {
    /// `threads[+skip]`, prefixed with `hw/` for hardware decode (`d3d11va:1/single+skip`).
    pub fn label(&self) -> String {
        let t = format!(
            "{}{}",
            self.threads.label(),
            if self.skip_nonref { "+skip" } else { "" }
        );
        match self.hw {
            Some(h) => format!("{}/{t}", h.label()),
            None => t,
        }
    }

    /// Parses a label written by [`Options::label`] (`forward_continue` off).
    pub fn parse(s: &str) -> std::result::Result<Self, String> {
        let (hw, rest) = match s.split_once('/') {
            Some((h, r)) => (HwSpec::parse(h)?, r),
            None => (None, s),
        };
        let (t, skip) = rest
            .split_once('+')
            .map_or((rest, false), |(t, s)| (t, s == "skip"));
        let threads = Threads::parse(t).ok_or(format!("bad threads in {s}"))?;
        Ok(Options {
            threads,
            skip_nonref: skip,
            forward_continue: false,
            hw,
        })
    }
}

/// Counters, reset by the caller between measurements.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub seeks: u64,
    pub continued: u64,
    pub landing_retries: u64,
    pub reopens: u64,
    pub packets_dropped: u64,
    pub packets_sent: u64,
    pub packets_skipped_nonref: u64,
    pub frames_out: u64,
    /// Frames returned with `AV_FRAME_FLAG_CORRUPT` or `decode_error_flags` (concealed errors).
    pub corrupt_frames: u64,
    pub decode_errors: u64,
    pub demux_errors: u64,
    pub index_mismatches: u64,
    /// Returned frames that were hardware surfaces (copied to system memory).
    pub hw_frames: u64,
    /// Returned frames that came out of the decoder in a software format.
    pub sw_frames: u64,
    /// Time spent in `av_hwframe_transfer_data` for returned frames.
    pub transfer_ns: u64,
}

/// What a decode step is looking for.
#[derive(Clone, Copy)]
enum Want {
    Next,
    Pts(i64),
}

/// Upper bound on packets read for one request; protects against corrupt files that never end.
const MAX_PACKETS_PER_REQUEST: u64 = 200_000;

/// An open, indexed, seekable video stream.
pub struct VideoSource {
    path: PathBuf,
    input: ffmpeg::format::context::Input,
    // Field order matters: the decoder points at `negotiation` (`AVCodecContext.opaque`) and
    // holds a reference to `hw_device`, so it is declared (and dropped) first.
    decoder: ffmpeg::decoder::Video,
    negotiation: Option<Box<hw::Negotiation>>,
    hw_device: Option<hw::Device>,
    hw_status: HwStatus,
    pub index: FrameIndex,
    /// The frame as decoded (a hardware surface on the hw path).
    frame: ffmpeg::frame::Video,
    /// The returned frame copied to system memory, when `frame` is a hardware surface.
    sysmem: ffmpeg::frame::Video,
    /// `frame()` returns `sysmem` (the last returned frame was a hardware surface).
    returned_hw: bool,
    /// Copy returned hardware frames to system memory (off for the zero-copy experiment, which
    /// reads `decoded_frame()` on the GPU).
    transfer: bool,
    opts: Options,
    /// A packet already read from the demuxer but not yet sent (the landing keyframe).
    pending: Option<ffmpeg::Packet>,
    /// Decode index of the next packet the demuxer returns, when known.
    next_idx: Option<usize>,
    /// The decoder was fed contiguously from a keyframe since the last flush.
    positioned: bool,
    eof_sent: bool,
    /// pts of the frame currently held in `frame`.
    last_pts: Option<i64>,
    pub stats: Stats,
    /// `S3_TRACE=1`: log every packet sent and frame received to stderr.
    trace: bool,
}

impl VideoSource {
    /// Opens `path`, scans the index and opens the decoder.
    pub fn open(path: &Path, opts: Options) -> Result<Self> {
        let index = FrameIndex::scan(path)?;
        Self::open_with_index(path, index, opts)
    }

    /// Opens `path` with an index built earlier.
    pub fn open_with_index(path: &Path, index: FrameIndex, opts: Options) -> Result<Self> {
        // Fallback 1: no device -> software decode, recorded in `hw_status`.
        let (hw_device, mut hw_status) = match opts.hw {
            None => (None, HwStatus::Off),
            Some(spec) => match hw::Device::create(spec) {
                Ok(d) => (Some(d), HwStatus::Off),
                Err(reason) => (None, HwStatus::NoDevice { spec, reason }),
            },
        };
        let (input, decoder, negotiation, status) =
            open_decoder(path, index.stream, opts.threads, hw_device.as_ref())?;
        if let Some(s) = status {
            hw_status = s;
        }
        Ok(VideoSource {
            path: path.to_path_buf(),
            input,
            decoder,
            negotiation,
            hw_device,
            hw_status,
            index,
            frame: ffmpeg::frame::Video::empty(),
            sysmem: ffmpeg::frame::Video::empty(),
            returned_hw: false,
            transfer: true,
            opts,
            pending: None,
            next_idx: Some(0),
            positioned: true,
            eof_sent: false,
            last_pts: None,
            stats: Stats::default(),
            trace: std::env::var_os("S3_TRACE").is_some(),
        })
    }

    /// Current options.
    pub fn options(&self) -> Options {
        self.opts
    }

    /// Changes `skip_nonref` / `forward_continue` (threading needs a reopen).
    pub fn set_options(&mut self, skip_nonref: bool, forward_continue: bool) {
        self.opts.skip_nonref = skip_nonref;
        self.opts.forward_continue = forward_continue;
    }

    /// The decoder (for color tags, pixel format, ...).
    pub fn decoder(&self) -> &ffmpeg::decoder::Video {
        &self.decoder
    }

    /// Rewinds to the first packet (sequential decoding from the start).
    pub fn rewind(&mut self) -> Result<()> {
        let first_key = self
            .index
            .frames
            .first()
            .map(|f| f.start_idx)
            .ok_or(Error::EmptyIndex)?;
        self.reposition(first_key)
    }

    /// The most recently returned frame, in system memory (copied from the GPU on the hw path).
    pub fn frame(&self) -> &ffmpeg::frame::Video {
        if self.returned_hw {
            &self.sysmem
        } else {
            &self.frame
        }
    }

    /// The most recently returned frame as the decoder produced it: a hardware surface on the hw
    /// path (what a zero-copy importer consumes), else the same as [`VideoSource::frame`].
    pub fn decoded_frame(&self) -> &ffmpeg::frame::Video {
        &self.frame
    }

    /// How the hwaccel setup went (device, decoder config).
    pub fn hw_status(&self) -> &HwStatus {
        &self.hw_status
    }

    /// The hardware device context, when one is open.
    pub fn hw_device(&self) -> Option<&hw::Device> {
        self.hw_device.as_ref()
    }

    /// `get_format` outcomes so far: (hw format picked, software format picked). A software pick
    /// with a device attached means the stream fell back (profile/bit depth not offered, or
    /// hwaccel initialisation failed and libavcodec asked again).
    pub fn negotiation(&self) -> (u32, u32) {
        self.negotiation.as_ref().map_or((0, 0), |n| {
            (
                n.hw_picks.load(Ordering::Relaxed),
                n.sw_picks.load(Ordering::Relaxed),
            )
        })
    }

    /// One-line hw summary: status, `get_format` picks and returned frame counts.
    pub fn hw_summary(&self) -> String {
        let (h, s) = self.negotiation();
        match &self.hw_status {
            HwStatus::Attached { .. } => format!(
                "{}; get_format hw {h} sw {s}; frames hw {} sw {}",
                self.hw_status.show(),
                self.stats.hw_frames,
                self.stats.sw_frames
            ),
            other => other.show(),
        }
    }

    /// Turns the copy of returned hardware frames to system memory on or off. Off, `frame()` is
    /// the hardware surface itself (zero-copy experiment).
    pub fn set_transfer(&mut self, transfer: bool) {
        self.transfer = transfer;
    }

    /// Copies the frame about to be returned to system memory when it is a hardware surface.
    fn finish_frame(&mut self) -> Result<()> {
        let hw_fmt = self.negotiation.as_ref().map(|n| n.hw_format());
        let is_hw = hw_fmt.is_some_and(|f| hw::is_hw_frame(&self.frame, f));
        self.returned_hw = is_hw && self.transfer;
        if is_hw && !self.transfer {
            self.stats.hw_frames += 1;
        } else if self.returned_hw {
            let t = std::time::Instant::now();
            hw::transfer(&self.frame, &mut self.sysmem).map_err(|err| Error::Ffmpeg {
                op: "av_hwframe_transfer_data",
                err,
            })?;
            self.stats.transfer_ns += t.elapsed().as_nanos() as u64;
            self.stats.hw_frames += 1;
        } else {
            self.stats.sw_frames += 1;
        }
        Ok(())
    }

    /// Decodes the next frame in presentation order into `frame()`; false at the end of the stream.
    pub fn next_frame(&mut self) -> Result<bool> {
        match self.decode(Want::Next) {
            Ok(()) => Ok(true),
            Err(Error::EndOfStream { wanted: None }) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Frame-exact seek to presentation index `pi`; the frame is then in `frame()`.
    pub fn seek_frame(&mut self, pi: usize) -> Result<()> {
        let target = *self.index.frames.get(pi).ok_or(Error::NoSuchFrame(pi))?;
        if self.last_pts == Some(target.pts) && self.positioned {
            return Ok(());
        }
        let reachable = self.positioned
            && !self.eof_sent
            && self.last_pts.is_none_or(|p| p < target.pts)
            && self.next_idx.is_some_and(|n| target.start_idx <= n);
        if self.opts.forward_continue && reachable {
            self.stats.continued += 1;
        } else {
            self.reposition(target.start_idx)?;
        }
        self.decode(Want::Pts(target.pts))
    }

    /// Positions the demuxer so the next packet fed is keyframe packet `k`, decoder flushed.
    fn reposition(&mut self, k: usize) -> Result<()> {
        self.stats.seeks += 1;
        let earlier: Vec<usize> = self.index.keys_before(k).take(3).collect();
        let mut attempts = 0u32;
        for key in std::iter::once(k).chain(earlier) {
            attempts += 1;
            let ts = self.index.seek_ts(key);
            if ffi::seek_stream(&mut self.input, self.index.stream, ts).is_err() {
                self.stats.landing_retries += 1;
                continue;
            }
            match self.land_at(k)? {
                true => return Ok(()),
                false => self.stats.landing_retries += 1,
            }
        }
        // Last resort: reopen the file and read from the start.
        self.stats.reopens += 1;
        let (input, decoder, negotiation, _) = open_decoder(
            &self.path,
            self.index.stream,
            self.opts.threads,
            self.hw_device.as_ref(),
        )?;
        self.input = input;
        // The old decoder (pointing at the old negotiation box) goes first, then the old box.
        self.decoder = decoder;
        self.negotiation = negotiation;
        self.returned_hw = false;
        if self.land_at(k)? {
            return Ok(());
        }
        Err(Error::SeekLanding {
            key_decode_idx: k,
            attempts,
        })
    }

    /// After a demuxer seek: flushes the decoder and reads up to packet `k`. Returns false if
    /// the demuxer landed past `k`.
    fn land_at(&mut self, k: usize) -> Result<bool> {
        if self.eof_sent {
            // FFmpeg 8.1's HEVC decoder without frame threading keeps a frame that is still
            // waiting to be output after EOF across `avcodec_flush_buffers`, and emits it after
            // the next seek (found by the verifier: seek to the last frame, then back). Drain to
            // AVERROR_EOF before flushing.
            let mut sink = ffmpeg::frame::Video::empty();
            for _ in 0..1024 {
                if self.decoder.receive_frame(&mut sink).is_err() {
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
            let Some(pkt) = self.read_video_packet()? else {
                return Ok(false);
            };
            let pos = pkt.position() as i64;
            match self.index.locate(pkt.dts(), pkt.pts(), pos) {
                Some(i) if i < k => self.stats.packets_dropped += 1,
                Some(i) if i == k => {
                    self.pending = Some(pkt);
                    self.next_idx = Some(k);
                    self.positioned = true;
                    return Ok(true);
                }
                Some(_) => return Ok(false),
                None => {
                    // A packet the scan never saw (e.g. a resync fragment after an MPEG-TS seek).
                    unknown += 1;
                    self.stats.packets_dropped += 1;
                    if unknown > 10_000 {
                        return Ok(false);
                    }
                }
            }
        }
    }

    /// Reads the next packet of our stream; `None` at EOF or on a terminal demux error.
    fn read_video_packet(&mut self) -> Result<Option<ffmpeg::Packet>> {
        let mut corrupt = 0u32;
        loop {
            let mut pkt = ffmpeg::Packet::empty();
            match pkt.read(&mut self.input) {
                Ok(()) if pkt.stream() == self.index.stream => return Ok(Some(pkt)),
                Ok(()) => {}
                Err(ffmpeg::Error::Eof) => return Ok(None),
                Err(ffmpeg::Error::InvalidData) => {
                    self.stats.demux_errors += 1;
                    corrupt += 1;
                    if corrupt > 10_000 {
                        return Ok(None);
                    }
                }
                // Truncated files end in an I/O error; treat as end of stream (the frames we
                // could not reach surface as `EndOfStream`).
                Err(_) => {
                    self.stats.demux_errors += 1;
                    return Ok(None);
                }
            }
        }
    }

    /// The decode loop shared by sequential decoding and seeking.
    fn decode(&mut self, want: Want) -> Result<()> {
        let mut packets = 0u64;
        self.returned_hw = false;
        loop {
            // Drain everything the decoder has ready.
            loop {
                match self.decoder.receive_frame(&mut self.frame) {
                    Ok(()) => {
                        self.stats.frames_out += 1;
                        if self.frame.is_corrupt() || self.frame.has_decode_errors() {
                            // Concealed damage: FFmpeg still returns a frame (H.264/HEVC conceal).
                            self.stats.corrupt_frames += 1;
                        }
                        let pts = self
                            .frame
                            .pts()
                            .or_else(|| self.frame.timestamp())
                            .ok_or(Error::FrameWithoutPts)?;
                        self.last_pts = Some(pts);
                        if self.trace {
                            eprintln!("  out  pts {pts}");
                        }
                        match want {
                            // Only the returned frame is copied off the GPU (hw path).
                            Want::Next => return self.finish_frame(),
                            Want::Pts(t) if pts == t => return self.finish_frame(),
                            Want::Pts(t) if pts > t => {
                                return Err(Error::Overshoot {
                                    wanted: t,
                                    got: pts,
                                });
                            }
                            Want::Pts(_) => {}
                        }
                    }
                    Err(ffmpeg::Error::Other { errno }) if errno == EAGAIN => break,
                    Err(ffmpeg::Error::Eof) => {
                        self.positioned = false;
                        return Err(Error::EndOfStream {
                            wanted: want_pts(want),
                        });
                    }
                    Err(ffmpeg::Error::InvalidData) => self.stats.decode_errors += 1,
                    Err(err) => {
                        return Err(Error::Ffmpeg {
                            op: "avcodec_receive_frame",
                            err,
                        });
                    }
                }
            }
            if self.eof_sent {
                // Draining after EOF never returns EAGAIN; guard anyway.
                self.positioned = false;
                return Err(Error::EndOfStream {
                    wanted: want_pts(want),
                });
            }
            packets += 1;
            if packets > MAX_PACKETS_PER_REQUEST {
                return Err(Error::Runaway { packets });
            }
            let pkt = match self.pending.take() {
                Some(p) => Some(p),
                None => self.read_video_packet()?,
            };
            let Some(pkt) = pkt else {
                self.eof_sent = true;
                self.next_idx = None;
                if let Err(err) = self.decoder.send_eof()
                    && err != ffmpeg::Error::Eof
                {
                    return Err(Error::Ffmpeg {
                        op: "avcodec_send_packet(NULL)",
                        err,
                    });
                }
                continue;
            };
            if let Some(n) = self.next_idx {
                if self
                    .index
                    .locate(pkt.dts(), pkt.pts(), pkt.position() as i64)
                    != Some(n)
                {
                    self.stats.index_mismatches += 1;
                }
                self.next_idx = Some(n + 1);
            }
            let skip = match want {
                Want::Pts(t) => {
                    self.opts.skip_nonref && pkt.pts().or(pkt.dts()).is_some_and(|p| p < t)
                }
                Want::Next => false,
            };
            self.decoder.skip_frame(if skip {
                ffmpeg::Discard::NonReference
            } else {
                ffmpeg::Discard::Default
            });
            if skip {
                self.stats.packets_skipped_nonref += 1;
            }
            if self.trace {
                eprintln!(
                    "  send pts {:?} dts {:?} key {} skip {skip}",
                    pkt.pts(),
                    pkt.dts(),
                    pkt.is_key()
                );
            }
            self.stats.packets_sent += 1;
            match self.decoder.send_packet(&pkt) {
                Ok(()) => {}
                // Corrupt data: FFmpeg conceals and continues; count it.
                Err(ffmpeg::Error::InvalidData) => self.stats.decode_errors += 1,
                Err(ffmpeg::Error::Other { errno }) if errno == EAGAIN => {
                    // Cannot happen after a full drain; keep the packet for the next round.
                    self.pending = Some(pkt);
                }
                Err(err) => {
                    self.stats.decode_errors += 1;
                    if self.stats.decode_errors > 10_000 {
                        return Err(Error::Ffmpeg {
                            op: "avcodec_send_packet",
                            err,
                        });
                    }
                }
            }
        }
    }
}

fn want_pts(w: Want) -> Option<i64> {
    match w {
        Want::Next => None,
        Want::Pts(t) => Some(t),
    }
}

/// What `open_decoder` returns: the container, the decoder, the `get_format` state (hw path
/// only; must outlive the decoder) and the hw status when a device was given.
type Opened = (
    ffmpeg::format::context::Input,
    ffmpeg::decoder::Video,
    Option<Box<hw::Negotiation>>,
    Option<HwStatus>,
);

/// Opens the container and a decoder for stream `stream`, attaching `hw` when the decoder has a
/// hwaccel for that device type (else fallback 2: software, reported in the status).
fn open_decoder(
    path: &Path,
    stream: usize,
    threads: Threads,
    hw: Option<&hw::Device>,
) -> Result<Opened> {
    let p = path.to_str().ok_or(Error::NonUtf8Path)?;
    let input = ffmpeg::format::input(p).map_err(ff("avformat_open_input"))?;
    let st = input.stream(stream).ok_or(Error::NoVideoStream)?;
    let tb = st.time_base();
    let mut ctx = ffmpeg::codec::Context::from_parameters(st.parameters())
        .map_err(ff("avcodec_parameters_to_context"))?;
    ctx.set_threading(threads.config());
    let mut negotiation = None;
    let mut status = None;
    if let Some(dev) = hw {
        let codec = ffmpeg::decoder::find(ctx.id()).ok_or(Error::NoVideoStream)?;
        match dev.pix_fmt_for(&codec) {
            Some(fmt) => {
                let neg = Box::new(hw::Negotiation::new(fmt));
                hw::attach(&mut ctx, dev, &neg);
                negotiation = Some(neg);
                status = Some(HwStatus::Attached {
                    spec: dev.spec,
                    pix_fmt: hw::pix_fmt_name(fmt),
                });
            }
            None => {
                status = Some(HwStatus::NoHwConfig {
                    spec: dev.spec,
                    codec: codec.name().to_string(),
                });
            }
        }
    }
    let mut dec = ctx.decoder();
    dec.set_packet_time_base(tb);
    let video = dec.video().map_err(|_| Error::NoVideoStream)?;
    Ok((input, video, negotiation, status))
}
