// SPDX-License-Identifier: GPL-3.0-or-later
//! One clip's source chain (worker side): [`PcmSource`] (backwards for a reversed clip) → stereo
//! down-mix → `rubato` resample to the output rate → output-rate stereo frames on demand.
//!
//! **Speed** (WP-2D.01): a clip at speed `s` reads its source `s` times as fast, so the
//! resampler maps the source rate × `s` to the output rate (a sinc resampler at that exact
//! ratio; the pitch follows the speed until WP-2D.02 adds pitch preservation). At normal speed
//! the chain is unchanged: no resampler at the output rate, the fixed FFT one otherwise.

use cutweave_model::{Rounding, SampleRate, Speed};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{
    Async, Fft, FixedAsync, FixedSync, Resampler, SincInterpolationParameters, WindowFunction,
};

use crate::automation::Automation;
use crate::dsp::{CHANNELS, downmix_into};
use crate::error::SourceError;
use crate::plan::{ClipPlan, source_time_at};
use crate::reverse::Reversed;
use crate::source::{PcmSource, SourceFactory, SourceRequest};

/// Source frames read (and resampled) per step.
const SOURCE_CHUNK: usize = 1024;

/// A resampler (source rate → output rate, times the clip's speed) with its start-up delay
/// trimmed, so output frame 0 is source frame 0.
struct Resample {
    inner: Box<dyn Resampler<f32>>,
    out: Vec<f32>,
    trim: usize,
}

impl Resample {
    /// The fixed-ratio FFT resampler of normal speed.
    fn new(from: SampleRate, to: SampleRate) -> Result<Resample, SourceError> {
        let fft = Fft::<f32>::new(
            from.hz() as usize,
            to.hz() as usize,
            SOURCE_CHUNK,
            CHANNELS,
            FixedSync::Input,
        )
        .map_err(|e| SourceError::Format(format!("no resampler {from} → {to}: {e}")))?;
        Ok(Resample::of(Box::new(fft)))
    }

    /// A sinc resampler at the exact ratio `to / (from × speed)` (WP-2D.01): the source is read
    /// `speed` times as fast as it plays.
    fn varispeed(from: SampleRate, to: SampleRate, speed: Speed) -> Result<Resample, SourceError> {
        let ratio = (f64::from(to.hz()) * f64::from(speed.den()))
            / (f64::from(from.hz()) * f64::from(speed.num()));
        let params = SincInterpolationParameters::new(128, WindowFunction::BlackmanHarris2);
        let sinc = Async::<f32>::new_sinc(
            ratio,
            1.0,
            &params,
            SOURCE_CHUNK,
            CHANNELS,
            FixedAsync::Input,
        )
        .map_err(|e| SourceError::Format(format!("no resampler {from} → {to} at {speed}x: {e}")))?;
        Ok(Resample::of(Box::new(sinc)))
    }

    fn of(inner: Box<dyn Resampler<f32>>) -> Resample {
        let out = vec![0.0; inner.output_frames_max() * CHANNELS];
        let trim = inner.output_delay();
        Resample { inner, out, trim }
    }

    /// Resamples one `SOURCE_CHUNK` of stereo input and appends the (trimmed) output to `fifo`.
    fn process(&mut self, input: &[f32], fifo: &mut Vec<f32>) -> Result<(), SourceError> {
        let err = |e: &dyn std::fmt::Display| SourceError::Decode(format!("resampler: {e}"));
        let frames_in = self.inner.input_frames_next();
        let input = InterleavedSlice::new(input, CHANNELS, frames_in).map_err(|e| err(&e))?;
        let frames_out = self.out.len() / CHANNELS;
        let mut output =
            InterleavedSlice::new_mut(&mut self.out, CHANNELS, frames_out).map_err(|e| err(&e))?;
        let (_, produced) = self
            .inner
            .process_into_buffer(&input, &mut output, None)
            .map_err(|e| err(&e))?;
        let skip = self.trim.min(produced);
        self.trim -= skip;
        fifo.extend_from_slice(&self.out[skip * CHANNELS..produced * CHANNELS]);
        Ok(())
    }
}

/// One clip's source chain, positioned on the output grid. Renders output-rate stereo frames in
/// order, independent of how the requests are sized.
pub(crate) struct ClipVoice {
    source: Box<dyn PcmSource>,
    channels: usize,
    raw: Vec<f32>,
    stereo: Vec<f32>,
    resample: Option<Resample>,
    /// Output-rate stereo frames ready to hand out, from `fifo_at`.
    fifo: Vec<f32>,
    fifo_at: usize,
    ended: bool,
    /// Gain, pan and fades, and the output sample of the next frame.
    auto: Automation,
    pos: i64,
}

impl ClipVoice {
    /// Opens `clip`'s source so the first rendered frame is output sample `at`
    /// (`clip.start ≤ at < clip.end`) at the output rate `rate`.
    pub fn open(
        sources: &dyn SourceFactory,
        clip: &ClipPlan,
        rate: SampleRate,
        at: i64,
    ) -> Result<ClipVoice, SourceError> {
        // The source range as a read-ahead hint: a reversed clip reads down to `src_end`.
        let (start, end) = if clip.reverse {
            (clip.src_end, clip.src_start)
        } else {
            (clip.src_start, clip.src_end)
        };
        let mut source = sources.open(&SourceRequest {
            media: clip.media,
            stream: clip.stream,
            start,
            end,
        })?;
        if clip.reverse {
            source = Box::new(Reversed::new(source));
        }
        let format = source.format();
        let channels = usize::from(format.channels);
        if channels == 0 {
            return Err(SourceError::Format("a source without channels".into()));
        }
        let src_time = source_time_at(clip, rate, at)
            .map_err(|e| SourceError::Format(format!("source position: {e}")))?;
        // Reversed, the first position is just inside the end of the range: the sample holding it.
        let rounding = if clip.reverse {
            Rounding::Floor
        } else {
            Rounding::Nearest
        };
        let frame = format.rate.time_to_samples(src_time, rounding);
        source.seek(u64::try_from(frame).unwrap_or(0))?;
        let resample = if !clip.speed.is_one() {
            Some(Resample::varispeed(format.rate, rate, clip.speed)?)
        } else if format.rate == rate {
            None
        } else {
            Some(Resample::new(format.rate, rate)?)
        };
        Ok(ClipVoice {
            source,
            channels,
            raw: vec![0.0; SOURCE_CHUNK * channels],
            stereo: vec![0.0; SOURCE_CHUNK * CHANNELS],
            resample,
            fifo: Vec::with_capacity(SOURCE_CHUNK * 4 * CHANNELS),
            fifo_at: 0,
            ended: false,
            auto: Automation::new(clip.env.clone(), rate, channels == 1),
            pos: at,
        })
    }

    fn buffered(&self) -> usize {
        (self.fifo.len() - self.fifo_at) / CHANNELS
    }

    /// Reads one chunk of source frames (zeros past the end of the stream) into `stereo`.
    fn read_chunk(&mut self) -> Result<(), SourceError> {
        let mut got = 0;
        while got < SOURCE_CHUNK && !self.ended {
            let n = self.source.read(&mut self.raw[got * self.channels..])?;
            if n == 0 {
                self.ended = true;
            }
            got += n.min(SOURCE_CHUNK - got);
        }
        self.raw[got * self.channels..].fill(0.0);
        downmix_into(&self.raw, self.channels, &mut self.stereo);
        Ok(())
    }

    /// Makes at least `frames` output frames available.
    fn fill(&mut self, frames: usize) -> Result<(), SourceError> {
        if self.fifo_at > 0 && self.fifo_at * 2 >= self.fifo.len() {
            self.fifo.drain(..self.fifo_at);
            self.fifo_at = 0;
        }
        while self.buffered() < frames {
            self.read_chunk()?;
            match self.resample.as_mut() {
                Some(r) => r.process(&self.stereo, &mut self.fifo)?,
                None => self.fifo.extend_from_slice(&self.stereo),
            }
        }
        Ok(())
    }

    /// Renders the next `out.len() / 2` frames and adds them, through the clip's gain, pan and
    /// fades, to `out`.
    pub fn render_add(&mut self, out: &mut [f32]) -> Result<(), SourceError> {
        let frames = out.len() / CHANNELS;
        self.fill(frames)?;
        let src = &self.fifo[self.fifo_at..self.fifo_at + frames * CHANNELS];
        self.auto.mix(self.pos, src, out);
        self.fifo_at += frames * CHANNELS;
        self.pos += i64::try_from(frames).unwrap_or(0);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::float_cmp,
        clippy::unwrap_used,
        clippy::cast_possible_wrap,
        clippy::cast_sign_loss
    )]

    use cutweave_model::{Duration, ItemId, MediaId, Time};

    use super::*;
    use crate::automation::Envelope;
    use crate::source::PcmFormat;
    use crate::tone::{Tone, ToneFactory, ToneSpec};

    fn clip(start: i64, end: i64, src_start: Time) -> ClipPlan {
        ClipPlan {
            item: ItemId::from_u128(1),
            media: MediaId::from_u128(1),
            stream: None,
            start,
            end,
            src_start,
            src_end: src_start
                .checked_add(Duration::from_secs(100).unwrap())
                .unwrap(),
            speed: cutweave_model::Speed::ONE,
            reverse: false,
            env: Envelope::flat(1.0, start, end - start),
        }
    }

    #[test]
    fn same_rate_voices_play_the_source_from_the_exact_frame() {
        let spec = ToneSpec::new(
            Tone::Sine {
                freq_hz: 440.0,
                amplitude: 0.5,
                phase: 0.0,
            },
            PcmFormat::new(SampleRate::HZ_48_000, 2),
        );
        let factory = ToneFactory::new().with(MediaId::from_u128(1), spec);
        let src_start = SampleRate::HZ_48_000.samples_to_time(1000).unwrap();
        let mut c = clip(500, 100_000, src_start);
        c.env = Envelope::flat(2.0, 500, 99_500);
        // Opened 37 samples into the clip: source frame 1037.
        let mut v = ClipVoice::open(&factory, &c, SampleRate::HZ_48_000, 537).unwrap();
        let mut out = vec![0.0; 2 * 3000];
        for piece in out.chunks_mut(2 * 300) {
            v.render_add(piece).unwrap();
        }
        for (i, f) in out.as_chunks::<2>().0.iter().enumerate() {
            let want = 2.0 * spec.sample(1037 + i as u64);
            assert_eq!(f, &[want, want], "frame {i}");
        }
    }

    #[test]
    fn short_sources_end_in_silence() {
        let spec = ToneSpec::new(
            Tone::Impulse {
                amplitude: 1.0,
                period: 1,
            },
            PcmFormat::new(SampleRate::HZ_48_000, 2),
        )
        .with_frames(10);
        let factory = ToneFactory::new().with(MediaId::from_u128(1), spec);
        let mut v = ClipVoice::open(
            &factory,
            &clip(0, 100, Time::ZERO),
            SampleRate::HZ_48_000,
            0,
        )
        .unwrap();
        let mut out = vec![0.0; 2 * 2000];
        v.render_add(&mut out).unwrap();
        assert!(out[..20].iter().all(|&s| s == 1.0));
        assert!(out[20..].iter().all(|&s| s == 0.0));
    }
}
