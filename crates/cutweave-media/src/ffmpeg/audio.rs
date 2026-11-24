// SPDX-License-Identifier: GPL-3.0-or-later
//! The FFmpeg [`AudioDecoder`] (WP-1D.03): sample ranges at the stream's native rate and channel
//! count, as planar `f32` (resampling is the mixer's job, WP-1G).
//!
//! - **Placement.** After a seek, the first decoded frame's pts places the samples (sample `n`
//!   at `rate.samples_to_time(n)`); later frames follow it contiguously. Priming samples the
//!   container marks are removed by FFmpeg (`skip_samples`); encoder end padding is not (S3).
//! - **Seek.** A range that the buffer or a short decode-forward cannot reach seeks the demuxer
//!   on the audio stream to a pre-roll before it (codecs with overlapping transforms need the
//!   previous frame to produce exact samples: at least the codec's `seek_preroll`, else 100 ms;
//!   none for PCM), so a range decodes to the samples a sequential read gives (AAC's noise
//!   substitution draws from decoder state FFmpeg does not reset on a flush: those bands differ
//!   at about −100 dBFS). A demuxer that lands late is retried from 10 s earlier.
//! - **Sequential reads** (`[a, b)` then `[b, c)`) never seek: the decoder keeps what it decoded
//!   past the previous range.
//! - **Sample formats.** Integer and float, packed and planar, are converted to `f32` in Rust
//!   (no swresample: the format conversion is exact and cheap).

use std::path::{Path, PathBuf};

use cutweave_model::{Duration, Rounding, SampleRate, StreamKind, Time};
use ffmpeg_next::codec;
use ffmpeg_next::format::Sample;
use ffmpeg_next::format::context::Input;
use ffmpeg_next::format::sample::Type as SampleType;
use ffmpeg_next::media::Type;
use ffmpeg_next::util::error::EAGAIN;
use ffmpeg_next::{Error, Packet, decoder, frame};

use super::error::media_error;
use super::{input, sys};
use crate::backend::AudioDecoder;
use crate::cancel;
use crate::path::display;
use crate::{AudioChunk, AudioFormat, CancelToken, MediaError, TimeBase};

/// A range starting at most this far (in seconds) past what is decoded decodes forward instead
/// of seeking.
const FORWARD_REACH_SECS: i64 = 1;
/// How much earlier (in seconds) a seek that landed late retries.
const RETRY_BACK_SECS: i64 = 10;
/// Decode errors one request tolerates.
const MAX_DECODE_ERRORS: u32 = 10_000;
/// Corrupt packets in a row before the stream counts as ended.
const MAX_DEMUX_ERRORS: u32 = 10_000;

/// Opens a decoder for audio stream `stream` (container index) of `path`, or the first one.
pub(super) fn open(path: &Path, stream: Option<u32>) -> Result<Box<dyn AudioDecoder>, MediaError> {
    let mut input = input::open(path)?;
    let no_stream = || MediaError::NoStream {
        path: display(path),
        kind: StreamKind::Audio,
        index: stream,
    };
    let index = match stream {
        Some(i) => {
            let i = usize::try_from(i).map_err(|_| no_stream())?;
            let st = input.stream(i).ok_or_else(no_stream)?;
            if st.parameters().medium() != Type::Audio {
                return Err(no_stream());
            }
            i
        }
        None => input
            .streams()
            .best(Type::Audio)
            .ok_or_else(no_stream)?
            .index(),
    };
    let (tb, start_ticks, preroll, pcm) = {
        let st = input.stream(index).ok_or_else(no_stream)?;
        let par = st.parameters();
        let p = sys::stream_params(&par);
        let pcm = p.codec.as_deref().is_some_and(|c| c.starts_with("pcm_"));
        // AV_NOPTS_VALUE (i64::MIN) when the container does not say.
        let start = Some(st.start_time()).filter(|&s| s != i64::MIN);
        (st.time_base(), start, sys::seek_preroll(&par), pcm)
    };
    let corrupt = |message: String| MediaError::Corrupt {
        path: display(path),
        message,
    };
    let time_base = u32::try_from(tb.numerator())
        .ok()
        .zip(u32::try_from(tb.denominator()).ok())
        .and_then(|(n, d)| TimeBase::new(n, d))
        .ok_or_else(|| corrupt(format!("invalid audio time base {tb}")))?;
    sys::discard_other_streams(&mut input, index);
    let decoder = open_codec(path, &input, index)?;
    let rate = SampleRate::new(decoder.rate())
        .map_err(|_| corrupt(format!("invalid sample rate {}", decoder.rate())))?;
    let channels = decoder.channels();
    if channels == 0 {
        return Err(corrupt("the audio stream has no channels".to_owned()));
    }
    let preroll = if pcm {
        0
    } else {
        i64::from(preroll).max(i64::from(rate.hz()) / 10)
    };
    Ok(Box::new(FfmpegAudioDecoder {
        path: path.to_path_buf(),
        input,
        decoder,
        stream: index,
        time_base,
        start_ticks,
        format: AudioFormat { rate, channels },
        preroll,
        frame: frame::Audio::empty(),
        buf: vec![Vec::new(); usize::from(channels)],
        buf_start: 0,
        end: None,
        fresh: true,
        eof_sent: false,
        eof: false,
        cancel: None,
    }))
}

/// A new decoder for stream `index` of `input` (fresh state: see [`FfmpegAudioDecoder::seek`]).
fn open_codec(path: &Path, input: &Input, index: usize) -> Result<decoder::Audio, MediaError> {
    let st = input.stream(index).ok_or_else(|| MediaError::NoStream {
        path: display(path),
        kind: StreamKind::Audio,
        index: u32::try_from(index).ok(),
    })?;
    let ctx = codec::Context::from_parameters(st.parameters()).map_err(|e| media_error(path, e))?;
    let mut dec = ctx.decoder();
    dec.set_packet_time_base(st.time_base());
    dec.audio().map_err(|e| match e {
        Error::DecoderNotFound => MediaError::Unsupported {
            message: format!("`{}`: no decoder for its audio codec", display(path)),
        },
        e => media_error(path, e),
    })
}

struct FfmpegAudioDecoder {
    path: PathBuf,
    input: Input,
    decoder: decoder::Audio,
    stream: usize,
    time_base: TimeBase,
    /// The stream's first timestamp, when the container reports it: a seek before it that
    /// fails is retried there (demuxers without an index fail a seek before their first entry).
    start_ticks: Option<i64>,
    format: AudioFormat,
    /// Samples decoded before a range after a seek.
    preroll: i64,
    frame: frame::Audio,
    /// Decoded samples per channel, sample `buf_start` first.
    buf: Vec<Vec<f32>>,
    buf_start: i64,
    /// Index of the next sample the decoder produces (`None` right after a seek, until a frame
    /// places it).
    end: Option<i64>,
    /// Nothing was read since the file was opened: the next read starts at the stream's start
    /// without a seek.
    fresh: bool,
    eof_sent: bool,
    /// The decoder is drained: nothing follows `end`.
    eof: bool,
    cancel: Option<CancelToken>,
}

impl FfmpegAudioDecoder {
    fn cancelled(&self) -> Result<(), MediaError> {
        cancel::check(self.cancel.as_ref())
    }

    fn corrupt(&self, message: String) -> MediaError {
        MediaError::Corrupt {
            path: display(&self.path),
            message,
        }
    }

    fn rate(&self) -> i64 {
        i64::from(self.format.rate.hz())
    }

    fn buf_len(&self) -> i64 {
        i64::try_from(self.buf.first().map_or(0, Vec::len)).unwrap_or(i64::MAX)
    }

    /// The first sample whose start time is at or after `t` (0 for `t <= 0`).
    fn sample_at_or_after(&self, t: Time) -> i64 {
        if t <= Time::ZERO {
            return 0;
        }
        let rate = self.format.rate;
        let n = rate.time_to_samples(t, Rounding::Floor);
        if rate.samples_to_time(n).is_ok_and(|s| s >= t) {
            n
        } else {
            n + 1
        }
    }

    /// Stream ticks at or before sample `n`.
    fn ticks_of(&self, n: i64) -> i64 {
        let tb = self.time_base;
        let num = i128::from(n) * i128::from(tb.den);
        let den = i128::from(self.rate()) * i128::from(tb.num);
        i64::try_from(num.div_euclid(den)).unwrap_or(if num < 0 { i64::MIN } else { i64::MAX })
    }

    /// The sample index of stream tick `ticks` (nearest).
    fn sample_of(&self, ticks: i64) -> i64 {
        let tb = self.time_base;
        let num = i128::from(ticks) * i128::from(tb.num) * i128::from(self.rate());
        let den = i128::from(tb.den);
        let half = den / 2;
        let n = if num >= 0 {
            (num + half) / den
        } else {
            -((-num + half) / den)
        };
        i64::try_from(n).unwrap_or(if n < 0 { i64::MIN } else { i64::MAX })
    }

    fn clear(&mut self) {
        for p in &mut self.buf {
            p.clear();
        }
        self.buf_start = 0;
        self.end = None;
        self.eof_sent = false;
        self.eof = false;
    }

    /// Positions the decoder a pre-roll before sample `first` and decodes until the first frame
    /// places the samples.
    ///
    /// The codec context is recreated rather than flushed: `avcodec_flush_buffers` keeps some
    /// state (AAC's noise-substitution generator), so after a flush a range would depend on what
    /// was decoded before. With a fresh decoder it depends only on where it starts.
    fn seek(&mut self, first: i64) -> Result<(), MediaError> {
        let mut back = 0;
        for attempt in 0..2 {
            self.cancelled()?;
            let target = first.saturating_sub(self.preroll).saturating_sub(back);
            let ts = self.ticks_of(target);
            // Demuxers without an index (raw AC3) fail a seek before their first entry; retry
            // at the stream's start. (Not first: MP4's priming packet sits before `start_time`.)
            let landed = sys::seek_stream(&mut self.input, self.stream, ts).is_ok()
                || self.start_ticks.is_some_and(|s| {
                    ts < s && sys::seek_stream(&mut self.input, self.stream, s).is_ok()
                });
            if !landed {
                // The demuxer cannot seek there: read from the start instead (correct, slow).
                self.input = input::open(&self.path)?;
                sys::discard_other_streams(&mut self.input, self.stream);
            }
            self.decoder = open_codec(&self.path, &self.input, self.stream)?;
            self.clear();
            if !self.decode_frame()? {
                return Ok(());
            }
            if self.buf_start <= first || attempt == 1 {
                return Ok(());
            }
            back = RETRY_BACK_SECS.saturating_mul(self.rate());
        }
        Ok(())
    }

    /// The next packet of the stream; `None` at its end.
    fn read_packet(&mut self) -> Option<Packet> {
        let mut corrupt = 0u32;
        loop {
            let mut pkt = Packet::empty();
            match pkt.read(&mut self.input) {
                Ok(()) if pkt.stream() == self.stream => return Some(pkt),
                Ok(()) => {}
                Err(Error::InvalidData) => {
                    corrupt += 1;
                    if corrupt > MAX_DEMUX_ERRORS {
                        return None;
                    }
                }
                Err(_) => return None,
            }
        }
    }

    /// Decodes the next frame into the buffer; `false` at the end of the stream.
    fn decode_frame(&mut self) -> Result<bool, MediaError> {
        let mut errors = 0u32;
        loop {
            match self.decoder.receive_frame(&mut self.frame) {
                Ok(()) => {
                    self.append()?;
                    return Ok(true);
                }
                Err(Error::Other { errno }) if errno == EAGAIN => {}
                Err(Error::Eof) => {
                    self.eof = true;
                    return Ok(false);
                }
                Err(_) => {
                    errors += 1;
                    if errors > MAX_DECODE_ERRORS {
                        return Err(self.corrupt("too many audio decode errors".to_owned()));
                    }
                    continue;
                }
            }
            if self.eof_sent {
                self.eof = true;
                return Ok(false);
            }
            self.cancelled()?;
            if let Some(pkt) = self.read_packet() {
                if self.decoder.send_packet(&pkt).is_err() {
                    errors += 1;
                    if errors > MAX_DECODE_ERRORS {
                        return Err(self.corrupt("too many audio decode errors".to_owned()));
                    }
                }
            } else {
                self.eof_sent = true;
                let _ = self.decoder.send_eof();
            }
        }
    }

    /// Appends the decoded `frame` to the buffer, converted to planar `f32`.
    fn append(&mut self) -> Result<(), MediaError> {
        let f = &self.frame;
        let n = f.samples();
        if f.rate() != self.format.rate.hz() || f.channels() != self.format.channels {
            return Err(MediaError::Unsupported {
                message: format!(
                    "`{}`: the audio format changes mid-stream ({} Hz × {} to {} Hz × {})",
                    display(&self.path),
                    self.format.rate.hz(),
                    self.format.channels,
                    f.rate(),
                    f.channels()
                ),
            });
        }
        let start = match self.end {
            Some(e) => e,
            None => f
                .pts()
                .or_else(|| f.timestamp())
                .map_or(0, |p| self.sample_of(p)),
        };
        if self.buf_len() == 0 {
            self.buf_start = start;
        }
        let channels = usize::from(self.format.channels);
        let format = f.format();
        let (bytes, planar) = match format {
            Sample::U8(t) => (1, t == SampleType::Planar),
            Sample::I16(t) => (2, t == SampleType::Planar),
            Sample::I32(t) | Sample::F32(t) => (4, t == SampleType::Planar),
            Sample::I64(t) | Sample::F64(t) => (8, t == SampleType::Planar),
            Sample::None => {
                return Err(MediaError::Unsupported {
                    message: format!("`{}`: audio sample format {format:?}", display(&self.path)),
                });
            }
        };
        let short = || MediaError::Backend {
            message: "a decoded audio frame is smaller than its sample count".to_owned(),
        };
        let convert = |b: &[u8]| -> f32 { sample_to_f32(format, b) };
        if planar {
            for (c, plane) in self.buf.iter_mut().enumerate().take(channels) {
                let data = f.data(c).get(..n * bytes).ok_or_else(short)?;
                plane.extend(data.chunks_exact(bytes).map(convert));
            }
        } else {
            let data = f.data(0).get(..n * bytes * channels).ok_or_else(short)?;
            for (c, plane) in self.buf.iter_mut().enumerate().take(channels) {
                plane.extend(
                    data.chunks_exact(bytes)
                        .skip(c)
                        .step_by(channels)
                        .map(convert),
                );
            }
        }
        self.end = Some(start + i64::try_from(n).unwrap_or(i64::MAX));
        Ok(())
    }

    /// Drops buffered samples before `n`.
    fn drop_before(&mut self, n: i64) {
        let k = usize::try_from(n - self.buf_start)
            .unwrap_or(0)
            .min(usize::try_from(self.buf_len()).unwrap_or(0));
        if k > 0 {
            for p in &mut self.buf {
                p.drain(..k);
            }
            self.buf_start += i64::try_from(k).unwrap_or(0);
        }
    }
}

/// One sample in `format` (native endianness) as `f32` in `[-1, 1]`.
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)] // audio samples
fn sample_to_f32(format: Sample, b: &[u8]) -> f32 {
    match format {
        Sample::U8(_) => (f32::from(b[0]) - 128.0) / 128.0,
        Sample::I16(_) => f32::from(i16::from_ne_bytes([b[0], b[1]])) / 32_768.0,
        Sample::I32(_) => {
            (f64::from(i32::from_ne_bytes([b[0], b[1], b[2], b[3]])) / 2_147_483_648.0) as f32
        }
        Sample::F32(_) => f32::from_ne_bytes([b[0], b[1], b[2], b[3]]),
        Sample::I64(_) => {
            let v = i64::from_ne_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
            (v as f64 / 9_223_372_036_854_775_808.0) as f32
        }
        Sample::F64(_) => {
            f64::from_ne_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as f32
        }
        Sample::None => 0.0,
    }
}

impl AudioDecoder for FfmpegAudioDecoder {
    fn format(&self) -> AudioFormat {
        self.format
    }

    fn set_cancel(&mut self, cancel: Option<CancelToken>) {
        self.cancel = cancel;
    }

    fn decode_range(&mut self, start: Time, dur: Duration) -> Result<AudioChunk, MediaError> {
        self.cancelled()?;
        let first = self.sample_at_or_after(start);
        let last = self
            .sample_at_or_after(start.saturating_add(dur))
            .max(first);
        if self.fresh {
            // The file is at its start: the first frame places the stream without a seek.
            self.fresh = false;
            self.decode_frame()?;
        }
        // Reachable without a seek: the buffer still starts at or before `first`, and the
        // decoder is at most a second short of it (or drained, when `first` is past the end).
        let reachable = self.end.is_some_and(|end| {
            self.buf_start <= first
                && (first <= end.saturating_add(FORWARD_REACH_SECS * self.rate()) || self.eof)
        });
        if !reachable {
            self.seek(first)?;
        }
        while self.end.is_some_and(|end| end < last) && !self.eof {
            if !self.decode_frame()? {
                break;
            }
            // Keep the buffer to what this and the next ranges need.
            if self.buf_start < first {
                self.drop_before(first);
            }
        }
        let end = self.end.unwrap_or(first);
        let from = first.max(self.buf_start).min(end);
        let to = last.min(end).max(from);
        let at = |n: i64| usize::try_from(n - self.buf_start).unwrap_or(0);
        let planes: Vec<Vec<f32>> = self
            .buf
            .iter()
            .map(|p| p.get(at(from)..at(to)).unwrap_or_default().to_vec())
            .collect();
        let pts = self
            .format
            .rate
            .samples_to_time(from)
            .map_err(|_| MediaError::OutOfRange { t: start })?;
        self.drop_before(to);
        Ok(AudioChunk {
            pts,
            rate: self.format.rate,
            channels: self.format.channels,
            planes,
        })
    }
}
