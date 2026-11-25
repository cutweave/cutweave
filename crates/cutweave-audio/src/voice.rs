// SPDX-License-Identifier: GPL-3.0-or-later
//! One clip's source chain (worker side): [`PcmSource`] → stereo down-mix → `rubato` resample to
//! the output rate → output-rate stereo frames on demand.

use cutweave_model::{Rounding, SampleRate};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};

use crate::automation::Automation;
use crate::dsp::{CHANNELS, downmix_into};
use crate::error::SourceError;
use crate::plan::{ClipPlan, source_time_at};
use crate::source::{PcmSource, SourceFactory, SourceRequest};

/// Source frames read (and resampled) per step.
const SOURCE_CHUNK: usize = 1024;

/// A synchronous FFT resampler (fixed ratio source rate → output rate) with its start-up delay
/// trimmed, so output frame 0 is source frame 0.
struct Resample {
    fft: Fft<f32>,
    out: Vec<f32>,
    trim: usize,
}

impl Resample {
    fn new(from: SampleRate, to: SampleRate) -> Result<Resample, SourceError> {
        let fft = Fft::<f32>::new(
            from.hz() as usize,
            to.hz() as usize,
            SOURCE_CHUNK,
            CHANNELS,
            FixedSync::Input,
        )
        .map_err(|e| SourceError::Format(format!("no resampler {from} → {to}: {e}")))?;
        let out = vec![0.0; fft.output_frames_max() * CHANNELS];
        let trim = fft.output_delay();
        Ok(Resample { fft, out, trim })
    }

    /// Resamples one `SOURCE_CHUNK` of stereo input and appends the (trimmed) output to `fifo`.
    fn process(&mut self, input: &[f32], fifo: &mut Vec<f32>) -> Result<(), SourceError> {
        let err = |e: &dyn std::fmt::Display| SourceError::Decode(format!("resampler: {e}"));
        let frames_in = self.fft.input_frames_next();
        let input = InterleavedSlice::new(input, CHANNELS, frames_in).map_err(|e| err(&e))?;
        let frames_out = self.out.len() / CHANNELS;
        let mut output =
            InterleavedSlice::new_mut(&mut self.out, CHANNELS, frames_out).map_err(|e| err(&e))?;
        let (_, produced) = self
            .fft
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
        let mut source = sources.open(&SourceRequest {
            media: clip.media,
            stream: clip.stream,
            start: clip.src_start,
            end: clip.src_end,
        })?;
        let format = source.format();
        let channels = usize::from(format.channels);
        if channels == 0 {
            return Err(SourceError::Format("a source without channels".into()));
        }
        let src_time = source_time_at(clip, rate, at)
            .map_err(|e| SourceError::Format(format!("source position: {e}")))?;
        let frame = format.rate.time_to_samples(src_time, Rounding::Nearest);
        source.seek(u64::try_from(frame).unwrap_or(0))?;
        let resample = if format.rate == rate {
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
