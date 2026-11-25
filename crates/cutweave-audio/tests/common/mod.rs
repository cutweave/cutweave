// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared fixtures of the mixer integration tests: segment builders, tone media, the offline
//! reference and a real-time driver.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

use std::sync::Arc;
use std::time::Duration as StdDuration;

use cutweave_audio::{Mixer, PcmFormat, SourceFactory, Tone, ToneFactory, ToneSpec, mixdown};
use cutweave_model::{
    Animatable, Duration, FLICKS_PER_SECOND, Fade, ItemId, MediaId, Rounding, SampleRate, Time,
    TrackId,
};
use cutweave_render::AudioSegment;

/// The output rate of every test.
pub const RATE: SampleRate = SampleRate::HZ_48_000;

/// `s` seconds, rounded to the nearest flick.
pub fn secs(s: f64) -> Time {
    Time::from_flicks((s * FLICKS_PER_SECOND as f64).round() as i64).unwrap()
}

/// Output sample of `t` (the mixer's grid).
pub fn sample_of(t: Time) -> i64 {
    RATE.time_to_samples(t, Rounding::Nearest)
}

/// A sine tone media.
pub fn sine(freq_hz: f64, amplitude: f32, phase: f64, rate: SampleRate, channels: u16) -> ToneSpec {
    ToneSpec::new(
        Tone::Sine {
            freq_hz,
            amplitude,
            phase,
        },
        PcmFormat::new(rate, channels),
    )
}

/// One clip as a mixer segment.
#[derive(Clone, Debug)]
pub struct Seg {
    pub track: u128,
    pub item: u128,
    pub media: u128,
    pub start: Time,
    pub duration: Duration,
    pub src_start: Time,
    pub clip_db: f64,
    pub track_db: f64,
    pub gain: Option<Animatable<f64>>,
    pub pan: Animatable<f64>,
    pub fade_in: Fade,
    pub fade_out: Fade,
}

impl Seg {
    pub fn new(track: u128, item: u128, media: u128, start: Time, end: Time) -> Seg {
        Seg {
            track,
            item,
            media,
            start,
            duration: end.duration_since(start).unwrap(),
            src_start: Time::ZERO,
            clip_db: 0.0,
            track_db: 0.0,
            gain: None,
            pan: Animatable::Static(0.0),
            fade_in: Fade::default(),
            fade_out: Fade::default(),
        }
    }

    pub fn src(mut self, t: Time) -> Seg {
        self.src_start = t;
        self
    }

    pub fn gains(mut self, clip_db: f64, track_db: f64) -> Seg {
        self.clip_db = clip_db;
        self.track_db = track_db;
        self
    }

    pub fn end(&self) -> Time {
        self.start.checked_add(self.duration).unwrap()
    }

    pub fn segment(&self) -> AudioSegment {
        AudioSegment {
            item: ItemId::from_u128(self.item),
            track: TrackId::from_u128(self.track),
            media: MediaId::from_u128(self.media),
            stream: Some(0),
            dst_start: self.start,
            dst_duration: self.duration,
            src_start: self.src_start,
            clip_gain_db: self.clip_db,
            clip_start: self.start,
            clip_duration: self.duration,
            gain: self
                .gain
                .clone()
                .unwrap_or(Animatable::Static(self.clip_db)),
            pan: self.pan.clone(),
            fade_in: self.fade_in,
            fade_out: self.fade_out,
            track_gain_db: self.track_db,
            reverse: false,
            speed: (1, 1),
            sample_rate: RATE,
        }
    }
}

pub fn segments(segs: &[Seg]) -> Vec<AudioSegment> {
    segs.iter().map(Seg::segment).collect()
}

/// Tone media `1..` from `specs`.
pub fn factory(specs: &[ToneSpec]) -> Arc<dyn SourceFactory> {
    let mut f = ToneFactory::new();
    for (i, spec) in specs.iter().enumerate() {
        f.insert(MediaId::from_u128(i as u128 + 1), *spec);
    }
    Arc::new(f)
}

/// The offline mix of `[start, end)` as one interleaved buffer.
pub fn offline(
    sources: &Arc<dyn SourceFactory>,
    segs: &[AudioSegment],
    start: Time,
    end: Time,
) -> Vec<f32> {
    let mut out = Vec::new();
    for chunk in mixdown(sources, segs, start..end, RATE).unwrap() {
        out.extend_from_slice(&chunk.samples);
    }
    out
}

/// Renders the real-time mixer in `buffer`-frame callbacks until it has produced at least
/// `frames` frames from its current position, waiting before each callback until the lanes hold
/// the data (never underruns, never paced). Returns everything produced (whole buffers).
pub fn render_ready(mixer: &mut Mixer, frames: usize, buffer: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(frames * 2);
    let mut buf = vec![0.0f32; buffer * 2];
    // Enough look-ahead for the prime check too (100 ms by default).
    let need = buffer.max(sample_of(secs(0.1)) as usize);
    while out.len() < frames * 2 {
        let mut spins = 0;
        while !mixer.ready(need) {
            spins += 1;
            assert!(spins < 20_000, "lanes never filled");
            std::thread::sleep(StdDuration::from_micros(200));
        }
        let info = mixer.render(&mut buf);
        if info.position.is_some() {
            out.extend_from_slice(&buf);
        }
    }
    out
}

/// Largest absolute difference of two equally long buffers, and where.
pub fn max_diff(a: &[f32], b: &[f32]) -> (f32, usize) {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b)
        .enumerate()
        .map(|(i, (x, y))| ((x - y).abs(), i))
        .fold((0.0, 0), |m, d| if d.0 > m.0 { d } else { m })
}

/// 8 sources on 8 tracks: native, resampled (44.1 and 96 kHz) and down-mixed (mono, stereo,
/// 5.1), with gaps and clip changes so lanes open new voices while playing.
pub fn eight_sources(seconds: f64) -> (Arc<dyn SourceFactory>, Vec<Seg>) {
    let specs = [
        sine(110.0, 0.05, 0.0, RATE, 1),
        sine(220.0, 0.05, 0.1, SampleRate::HZ_44_100, 2),
        sine(330.0, 0.05, 0.2, SampleRate::HZ_96_000, 6),
        sine(440.0, 0.05, 0.3, SampleRate::HZ_44_100, 1),
        sine(550.0, 0.05, 0.4, RATE, 2),
        sine(660.0, 0.05, 0.5, SampleRate::HZ_44_100, 2),
        sine(770.0, 0.05, 0.6, RATE, 1),
        ToneSpec::new(
            Tone::Impulse {
                amplitude: 0.1,
                period: 4800,
            },
            PcmFormat::new(RATE, 1),
        ),
    ];
    let mut segs = Vec::new();
    for track in 1..=8u128 {
        // Each track: 2 s clips with a 0.25 s gap, alternating source offsets.
        let mut t = 0.0;
        let mut item = track * 1000;
        while t < seconds {
            let end = (t + 2.0).min(seconds + 1.0);
            segs.push(
                Seg::new(track, item, track, secs(t), secs(end))
                    .src(secs((item % 3) as f64 * 0.37))
                    .gains(-1.0, -2.0),
            );
            item += 1;
            t = end + 0.25;
        }
    }
    (factory(&specs), segs)
}
