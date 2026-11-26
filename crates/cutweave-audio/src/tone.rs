// SPDX-License-Identifier: GPL-3.0-or-later
//! Generated, deterministic sources for tests and demos: sines, impulse trains, seeded pink
//! noise and silence (R29: the mixer is built and verified against these until decoded audio
//! lands).
//!
//! Every sample is a pure function of its frame index (no accumulated phase), so a source
//! returns the same frames whatever the read sizes and seeks, and tests can compute the exact
//! expected value of any frame with [`ToneSpec::sample`].

use std::collections::BTreeMap;
use std::f64::consts::TAU;

use cutweave_model::MediaId;

use crate::error::SourceError;
use crate::source::{PcmFormat, PcmSource, SourceFactory, SourceRequest};

/// What a [`ToneSource`] plays. Every channel carries the same signal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tone {
    /// `amplitude · sin(2π · freq_hz · n / rate + phase)`.
    Sine {
        /// Frequency in Hz.
        freq_hz: f64,
        /// Peak amplitude.
        amplitude: f32,
        /// Phase at frame 0, in radians.
        phase: f64,
    },
    /// `amplitude` at frames `0, period, 2·period, …` (only frame 0 when `period` is 0), 0
    /// elsewhere.
    Impulse {
        /// Height of each impulse.
        amplitude: f32,
        /// Frames between impulses (0: a single impulse at frame 0).
        period: u64,
    },
    /// A constant `level` (a DC offset): what the envelope tests multiply their gains by.
    Dc {
        /// The value of every frame.
        level: f32,
    },
    /// Pink noise (−3 dB per octave from about 1.5 Hz up): the Voss–McCartney sum of 16 white
    /// noise rows, row `k` holding a new value every `2^k` frames, each value a hash of `seed`,
    /// the row and the frame. Peaks stay within `±amplitude`. Every frame is a pure function of
    /// its index, like the other tones.
    PinkNoise {
        /// The largest possible magnitude.
        amplitude: f32,
        /// The noise seed (equal seeds play equal noise).
        seed: u64,
    },
    /// Digital silence.
    Silence,
}

/// Rows of the pink noise generator.
const PINK_ROWS: u32 = 16;

/// A value in `[-1, 1)` from `seed`, `row` and `index` (SplitMix64 finalizer).
#[allow(clippy::cast_precision_loss)]
fn white(seed: u64, row: u32, index: u64) -> f64 {
    let mut z = seed
        ^ u64::from(row).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ index.wrapping_mul(0xD1B5_4A32_D192_ED03);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 52) as f64 - 1.0
}

/// A generated source: what it plays, its native format and its length.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToneSpec {
    /// The signal.
    pub tone: Tone,
    /// Native rate and channel count.
    pub format: PcmFormat,
    /// Length in frames (`None`: endless).
    pub frames: Option<u64>,
}

impl ToneSpec {
    /// An endless `tone` in `format`.
    #[must_use]
    pub const fn new(tone: Tone, format: PcmFormat) -> ToneSpec {
        ToneSpec {
            tone,
            format,
            frames: None,
        }
    }

    /// The same source, `frames` long.
    #[must_use]
    pub const fn with_frames(mut self, frames: u64) -> ToneSpec {
        self.frames = Some(frames);
        self
    }

    /// The exact value of frame `frame` (every channel), 0 past the end.
    #[must_use]
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    pub fn sample(&self, frame: u64) -> f32 {
        if self.frames.is_some_and(|len| frame >= len) {
            return 0.0;
        }
        match self.tone {
            Tone::Sine {
                freq_hz,
                amplitude,
                phase,
            } => {
                // Reduce the cycle count to [0, 1) before scaling by 2π: exact for any frame index
                // a timeline can reach (|frame| < 2^53).
                let cycles = (frame as f64 * freq_hz / f64::from(self.format.rate.hz())).fract();
                (f64::from(amplitude) * (TAU * cycles + phase).sin()) as f32
            }
            Tone::Impulse { amplitude, period } => {
                let hit = if period == 0 {
                    frame == 0
                } else {
                    frame.is_multiple_of(period)
                };
                if hit { amplitude } else { 0.0 }
            }
            Tone::Dc { level } => level,
            Tone::PinkNoise { amplitude, seed } => {
                let sum: f64 = (0..PINK_ROWS)
                    .map(|row| white(seed, row, frame >> row))
                    .sum();
                (f64::from(amplitude) * sum / f64::from(PINK_ROWS)) as f32
            }
            Tone::Silence => 0.0,
        }
    }
}

/// A [`PcmSource`] over a [`ToneSpec`].
#[derive(Clone, Debug)]
pub struct ToneSource {
    spec: ToneSpec,
    pos: u64,
}

impl ToneSource {
    /// A source at frame 0.
    #[must_use]
    pub const fn new(spec: ToneSpec) -> ToneSource {
        ToneSource { spec, pos: 0 }
    }
}

impl PcmSource for ToneSource {
    fn format(&self) -> PcmFormat {
        self.spec.format
    }

    fn seek(&mut self, frame: u64) -> Result<(), SourceError> {
        self.pos = frame;
        Ok(())
    }

    fn read(&mut self, out: &mut [f32]) -> Result<usize, SourceError> {
        let channels = usize::from(self.spec.format.channels.max(1));
        let mut frames = out.len() / channels;
        if let Some(len) = self.spec.frames {
            let left = len.saturating_sub(self.pos);
            frames = frames.min(usize::try_from(left).unwrap_or(usize::MAX));
        }
        for (i, frame) in out.chunks_exact_mut(channels).take(frames).enumerate() {
            frame.fill(self.spec.sample(self.pos + i as u64));
        }
        self.pos += frames as u64;
        Ok(frames)
    }
}

/// A [`SourceFactory`] that maps media IDs to generated tones.
#[derive(Clone, Debug, Default)]
pub struct ToneFactory {
    tones: BTreeMap<MediaId, ToneSpec>,
}

impl ToneFactory {
    /// An empty factory (every `open` fails with [`SourceError::UnknownMedia`]).
    #[must_use]
    pub fn new() -> ToneFactory {
        ToneFactory::default()
    }

    /// Registers (or replaces) the tone of `media`.
    pub fn insert(&mut self, media: MediaId, spec: ToneSpec) -> &mut ToneFactory {
        self.tones.insert(media, spec);
        self
    }

    /// Builder form of [`insert`](Self::insert).
    #[must_use]
    pub fn with(mut self, media: MediaId, spec: ToneSpec) -> ToneFactory {
        self.tones.insert(media, spec);
        self
    }

    /// The tone registered for `media`.
    #[must_use]
    pub fn spec(&self, media: MediaId) -> Option<&ToneSpec> {
        self.tones.get(&media)
    }
}

impl SourceFactory for ToneFactory {
    fn open(&self, request: &SourceRequest) -> Result<Box<dyn PcmSource>, SourceError> {
        let spec = self
            .tones
            .get(&request.media)
            .ok_or(SourceError::UnknownMedia(request.media))?;
        Ok(Box::new(ToneSource::new(*spec)))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp, clippy::unwrap_used)]

    use cutweave_model::SampleRate;

    use super::*;

    fn sine(channels: u16) -> ToneSpec {
        ToneSpec::new(
            Tone::Sine {
                freq_hz: 997.0,
                amplitude: 0.5,
                phase: 0.25,
            },
            PcmFormat::new(SampleRate::HZ_44_100, channels),
        )
    }

    #[test]
    fn reads_are_independent_of_chunking_and_seeks() {
        let spec = sine(2);
        let mut whole = ToneSource::new(spec);
        let mut a = vec![0.0; 2 * 1000];
        assert_eq!(whole.read(&mut a).unwrap(), 1000);

        let mut pieces = ToneSource::new(spec);
        let mut b = vec![0.0; 2 * 1000];
        let mut at = 0;
        for n in [1, 7, 100, 333, 559] {
            assert_eq!(pieces.read(&mut b[at * 2..(at + n) * 2]).unwrap(), n);
            at += n;
        }
        assert_eq!(a, b);

        let mut seeked = ToneSource::new(spec);
        seeked.seek(400).unwrap();
        let mut c = vec![0.0; 2 * 600];
        seeked.read(&mut c).unwrap();
        assert_eq!(&a[800..], &c[..]);
    }

    #[test]
    fn finite_sources_end() {
        let spec = sine(1).with_frames(10);
        let mut s = ToneSource::new(spec);
        s.seek(8).unwrap();
        let mut buf = [0.0; 5];
        assert_eq!(s.read(&mut buf).unwrap(), 2);
        assert_eq!(buf[..2], [spec.sample(8), spec.sample(9)]);
        assert_eq!(s.read(&mut buf).unwrap(), 0);
        assert_eq!(spec.sample(10), 0.0);
    }

    #[test]
    fn impulses_land_on_their_period() {
        let format = PcmFormat::new(SampleRate::HZ_48_000, 1);
        let train = ToneSpec::new(
            Tone::Impulse {
                amplitude: 1.0,
                period: 100,
            },
            format,
        );
        let single = ToneSpec::new(
            Tone::Impulse {
                amplitude: 0.5,
                period: 0,
            },
            format,
        );
        assert_eq!(train.sample(0), 1.0);
        assert_eq!(train.sample(300), 1.0);
        assert_eq!(train.sample(301), 0.0);
        assert_eq!(single.sample(0), 0.5);
        assert_eq!(single.sample(100), 0.0);
    }

    #[test]
    fn factory_opens_registered_media_only() {
        let media = MediaId::from_u128(7);
        let factory = ToneFactory::new().with(media, sine(1));
        let request = |media| SourceRequest {
            media,
            stream: None,
            start: cutweave_model::Time::ZERO,
            end: cutweave_model::Time::ZERO,
        };
        assert!(factory.open(&request(media)).is_ok());
        assert_eq!(
            factory.open(&request(MediaId::from_u128(8))).err(),
            Some(SourceError::UnknownMedia(MediaId::from_u128(8)))
        );
    }
}
