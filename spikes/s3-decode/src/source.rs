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

use ffmpeg::codec::threading;
use ffmpeg::util::error::EAGAIN;
use ffmpeg_next as ffmpeg;

use crate::error::{Error, Result, ff};
use crate::ffi;
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
}

impl Default for Options {
    fn default() -> Self {
        Options {
            threads: Threads::Frame(0),
            skip_nonref: true,
            forward_continue: false,
        }
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
    decoder: ffmpeg::decoder::Video,
    pub index: FrameIndex,
    frame: ffmpeg::frame::Video,
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
        let (input, decoder) = open_decoder(path, index.stream, opts.threads)?;
        Ok(VideoSource {
            path: path.to_path_buf(),
            input,
            decoder,
            index,
            frame: ffmpeg::frame::Video::empty(),
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

    /// The most recently decoded frame.
    pub fn frame(&self) -> &ffmpeg::frame::Video {
        &self.frame
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
        let (input, decoder) = open_decoder(&self.path, self.index.stream, self.opts.threads)?;
        self.input = input;
        self.decoder = decoder;
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
                            Want::Next => return Ok(()),
                            Want::Pts(t) if pts == t => return Ok(()),
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

/// Opens the container and a decoder for stream `stream`.
fn open_decoder(
    path: &Path,
    stream: usize,
    threads: Threads,
) -> Result<(ffmpeg::format::context::Input, ffmpeg::decoder::Video)> {
    let p = path.to_str().ok_or(Error::NonUtf8Path)?;
    let input = ffmpeg::format::input(p).map_err(ff("avformat_open_input"))?;
    let st = input.stream(stream).ok_or(Error::NoVideoStream)?;
    let tb = st.time_base();
    let mut ctx = ffmpeg::codec::Context::from_parameters(st.parameters())
        .map_err(ff("avcodec_parameters_to_context"))?;
    ctx.set_threading(threads.config());
    let mut dec = ctx.decoder();
    dec.set_packet_time_base(tb);
    let video = dec.video().map_err(|_| Error::NoVideoStream)?;
    Ok((input, video))
}
