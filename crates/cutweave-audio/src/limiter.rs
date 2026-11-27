// SPDX-License-Identifier: GPL-3.0-or-later
//! The master limiter (WP-2E.02, arch §9): a true-peak look-ahead limiter that keeps the mix
//! under its ceiling (default −1 dBTP), run by the real-time mixer and the offline mixdown alike.
//!
//! # How it works
//!
//! Per stereo frame, both channels sharing one gain:
//!
//! 1. **Detector.** A polyphase interpolator (4× below 96 kHz, 2× below 192 kHz, none above: a
//!    48-tap Hann-windowed sinc, the oversampling ITU-R BS.1770-4 Annex 2 describes and `ebur128`
//!    measures true peak with) estimates the peak of the signal between two samples, the sample
//!    itself included: `d[n]` covers `[n − G, n − G + 1)`, where `G` is the filter's delay
//!    ([`Limiter::detector_delay`]).
//! 2. **Required gain** `r[n] = min(1, ceiling / d[n])`.
//! 3. **Hold.** The minimum of `r` over the last `W + 1` frames, `W` the look-ahead in frames.
//! 4. **Release.** The held gain drops at once and recovers exponentially with the release time
//!    constant.
//! 5. **Attack.** The moving average of the last `W` released gains: the gain glides down over
//!    the look-ahead instead of stepping.
//! 6. **Delay.** The audio leaves `D = W + G − 1` frames late ([`Limiter::latency`]), multiplied
//!    by that gain.
//!
//! **Guarantee.** Every value the average of step 5 takes for output sample `m` is held down by
//! the detector outputs that cover `[m − 1, m + 1)`, so the gain applied around sample `m` is at
//! most `ceiling / (true peak around m)`: the output's true peak stays at the ceiling (the gain
//! moves slowly, so the product's inter-sample peaks follow it to well under 0.1 dB).
//!
//! **Transparent below the ceiling.** When no detector output in the window exceeds the ceiling
//! the gain is exactly 1.0, so the output is the input delayed, bit for bit.
//!
//! **Fixed latency.** [`Limiter::latency`] depends only on the configuration and the rate, also
//! when the limiter is disabled (a bypass is a plain delay), so the mixer's latency compensation
//! (the lanes run `D` frames ahead) never changes while it plays.
//!
//! Real-time safe: [`Limiter::new`] allocates everything; [`process`](Limiter::process) and
//! [`reset`](Limiter::reset) never allocate, lock or block. The arithmetic is per frame, in a
//! fixed order, so the output does not depend on how the stream is cut into blocks.

use cutweave_model::SampleRate;

use crate::control::ms_to_frames;
use crate::dsp::CHANNELS;
use crate::error::MixError;

/// The default ceiling: −1 dBTP (EBU R 128 delivery, most streaming platforms).
pub const DEFAULT_CEILING_DBTP: f64 = -1.0;
/// The default look-ahead.
pub const DEFAULT_LOOKAHEAD_MS: u32 = 5;
/// The default release time constant.
pub const DEFAULT_RELEASE_MS: u32 = 100;
/// The highest accepted ceiling (0 dBTP: full scale).
pub const MAX_CEILING_DBTP: f64 = 0.0;
/// The lowest accepted ceiling.
pub const MIN_CEILING_DBTP: f64 = -24.0;
/// The longest accepted look-ahead.
pub const MAX_LOOKAHEAD_MS: u32 = 50;
/// The longest accepted release.
pub const MAX_RELEASE_MS: u32 = 5_000;

/// Taps of the interpolation filter (all phases together).
const TAPS: usize = 48;
/// A held gain this close below its target snaps to it (−0.000_000_9 dB), so the release ends
/// on exactly the target instead of creeping towards it forever.
const SNAP: f64 = 1e-7;

/// How the master limiter behaves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LimiterConfig {
    /// Off: the audio passes unchanged (still delayed by [`Limiter::latency`]).
    pub enabled: bool,
    /// The true-peak ceiling in dBTP (`-24..=0`).
    pub ceiling_dbtp: f64,
    /// How far ahead the limiter looks, in milliseconds (`1..=50`): also its attack time.
    pub lookahead_ms: u32,
    /// The release time constant in milliseconds (`1..=5000`).
    pub release_ms: u32,
}

impl Default for LimiterConfig {
    /// On, −1 dBTP, 5 ms look-ahead, 100 ms release.
    fn default() -> LimiterConfig {
        LimiterConfig::DEFAULT
    }
}

impl LimiterConfig {
    /// On, −1 dBTP, 5 ms look-ahead, 100 ms release.
    pub const DEFAULT: LimiterConfig = LimiterConfig {
        enabled: true,
        ceiling_dbtp: DEFAULT_CEILING_DBTP,
        lookahead_ms: DEFAULT_LOOKAHEAD_MS,
        release_ms: DEFAULT_RELEASE_MS,
    };

    /// The defaults, switched off (a delay only).
    #[must_use]
    pub fn disabled() -> LimiterConfig {
        LimiterConfig {
            enabled: false,
            ..LimiterConfig::default()
        }
    }

    /// Checks the ranges.
    pub fn validate(&self) -> Result<(), MixError> {
        if !(MIN_CEILING_DBTP..=MAX_CEILING_DBTP).contains(&self.ceiling_dbtp) {
            return Err(MixError::Config(
                "the limiter ceiling must be between -24 and 0 dBTP",
            ));
        }
        if !(1..=MAX_LOOKAHEAD_MS).contains(&self.lookahead_ms) {
            return Err(MixError::Config(
                "the limiter look-ahead must be between 1 and 50 ms",
            ));
        }
        if !(1..=MAX_RELEASE_MS).contains(&self.release_ms) {
            return Err(MixError::Config(
                "the limiter release must be between 1 and 5000 ms",
            ));
        }
        Ok(())
    }
}

/// The true-peak detector: the polyphase interpolator and its input history.
struct Detector {
    /// Oversampling factor (1: sample peak only).
    factor: usize,
    /// Taps per phase.
    phase_taps: usize,
    /// Coefficients, `[tap][phase]` flattened (`tap · factor + phase`).
    coeffs: Box<[f32]>,
    /// Per channel: the last `phase_taps` inputs, newest first from `head`, written twice
    /// (`head` and `head + phase_taps`) so the window is one contiguous slice.
    history: [Box<[f32]>; CHANNELS],
    head: usize,
}

impl Detector {
    fn new(rate: SampleRate) -> Detector {
        let factor = if rate.hz() < 96_000 {
            4
        } else if rate.hz() < 192_000 {
            2
        } else {
            1
        };
        let phase_taps = if factor == 1 { 1 } else { TAPS / factor };
        let coeffs = if factor == 1 {
            vec![1.0f32].into_boxed_slice()
        } else {
            (0..TAPS)
                .map(|tap| {
                    #[allow(clippy::cast_precision_loss)]
                    let (tap, taps, factor) = (tap as f64, TAPS as f64, factor as f64);
                    let window = 0.5 * (1.0 - (2.0 * std::f64::consts::PI * tap / taps).cos());
                    let offset = tap - taps / 2.0;
                    let arg = offset * std::f64::consts::PI / factor;
                    let sinc = if offset.abs() < 1e-9 {
                        1.0
                    } else {
                        arg.sin() / arg
                    };
                    #[allow(clippy::cast_possible_truncation)]
                    let coeff = (window * sinc) as f32;
                    coeff
                })
                .collect()
        };
        Detector {
            factor,
            phase_taps,
            coeffs,
            history: std::array::from_fn(|_| vec![0.0; 2 * phase_taps].into_boxed_slice()),
            head: 0,
        }
    }

    /// Frames between an input and the detector output that describes it.
    fn delay(&self) -> usize {
        if self.factor == 1 {
            0
        } else {
            self.phase_taps / 2
        }
    }

    fn reset(&mut self) {
        for h in &mut self.history {
            h.fill(0.0);
        }
        self.head = 0;
    }

    /// Takes the next input frame and returns the peak magnitude (both channels) of the signal
    /// in `[n − delay, n − delay + 1)`, the sample at `n − delay` included.
    #[inline]
    fn push(&mut self, frame: [f32; CHANNELS]) -> f32 {
        let taps = self.phase_taps;
        self.head = if self.head == 0 {
            taps - 1
        } else {
            self.head - 1
        };
        let (head, delay, factor) = (self.head, self.delay(), self.factor);
        let mut peak = 0.0f32;
        for (h, &x) in self.history.iter_mut().zip(&frame) {
            h[head] = x;
            h[head + taps] = x;
            let window = &h[head..head + taps];
            // The delayed sample itself (the phase-0 output equals it up to rounding).
            peak = peak.max(window[delay].abs());
            if factor > 1 {
                for p in 0..factor {
                    let mut acc = 0.0f32;
                    for (t, &s) in window.iter().enumerate() {
                        acc += self.coeffs[t * factor + p] * s;
                    }
                    peak = peak.max(acc.abs());
                }
            }
        }
        peak
    }
}

/// The sliding minimum of the last `len` values: a monotonic queue in a fixed ring.
struct MinHold {
    len: u64,
    /// `(index, value)`, values increasing from front to back.
    ring: Box<[(u64, f64)]>,
    front: usize,
    count: usize,
    next: u64,
}

impl MinHold {
    fn new(len: usize) -> MinHold {
        MinHold {
            len: len as u64,
            ring: vec![(0, 1.0); len.max(1)].into_boxed_slice(),
            front: 0,
            count: 0,
            next: 0,
        }
    }

    fn reset(&mut self) {
        self.front = 0;
        self.count = 0;
        self.next = 0;
    }

    /// Adds `v` and returns the minimum of the last `len` values.
    #[inline]
    fn push(&mut self, v: f64) -> f64 {
        let cap = self.ring.len();
        while self.count > 0 {
            let back = (self.front + self.count - 1) % cap;
            if self.ring[back].1 >= v {
                self.count -= 1;
            } else {
                break;
            }
        }
        let idx = self.next;
        self.next += 1;
        while self.count > 0 && self.ring[self.front].0 + self.len <= idx {
            self.front = (self.front + 1) % cap;
            self.count -= 1;
        }
        let slot = (self.front + self.count) % cap;
        self.ring[slot] = (idx, v);
        self.count += 1;
        self.ring[self.front].1
    }
}

/// The moving average of the last `len` values (all 1.0 at start), exactly 1.0 when they all are.
struct MovingMean {
    values: Box<[f64]>,
    pos: usize,
    sum: f64,
    /// Values in the window below 1.0.
    below: usize,
}

impl MovingMean {
    fn new(len: usize) -> MovingMean {
        let len = len.max(1);
        MovingMean {
            values: vec![1.0; len].into_boxed_slice(),
            pos: 0,
            #[allow(clippy::cast_precision_loss)]
            sum: len as f64,
            below: 0,
        }
    }

    fn reset(&mut self) {
        self.values.fill(1.0);
        self.pos = 0;
        #[allow(clippy::cast_precision_loss)]
        let n = self.values.len() as f64;
        self.sum = n;
        self.below = 0;
    }

    #[inline]
    fn push(&mut self, v: f64) -> f64 {
        let old = std::mem::replace(&mut self.values[self.pos], v);
        self.pos = (self.pos + 1) % self.values.len();
        if old < 1.0 {
            self.below -= 1;
        }
        if v < 1.0 {
            self.below += 1;
        }
        #[allow(clippy::cast_precision_loss)]
        let n = self.values.len() as f64;
        if self.below == 0 {
            // Exact again: no drift survives a stretch of unity gain.
            self.sum = n;
            return 1.0;
        }
        self.sum += v - old;
        (self.sum / n).min(1.0)
    }
}

/// The master limiter (see the module docs). One per mixer or mixdown.
pub struct Limiter {
    config: LimiterConfig,
    /// The ceiling as a linear amplitude.
    ceiling: f64,
    detector: Detector,
    hold: MinHold,
    /// Release coefficient per frame.
    release: f64,
    /// The held gain after the release.
    env: f64,
    attack: MovingMean,
    /// `latency` frames of input, oldest at `delay_pos`.
    delay: Box<[[f32; CHANNELS]]>,
    delay_pos: usize,
    /// The smallest gain applied since [`take_min_gain`](Self::take_min_gain).
    min_gain: f32,
}

impl std::fmt::Debug for Limiter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Limiter")
            .field("config", &self.config)
            .field("latency", &self.latency())
            .finish_non_exhaustive()
    }
}

impl Limiter {
    /// A limiter for audio at `rate` (allocates its buffers).
    pub fn new(config: LimiterConfig, rate: SampleRate) -> Result<Limiter, MixError> {
        config.validate()?;
        let window = usize::try_from(ms_to_frames(rate, config.lookahead_ms)?)
            .map_err(|_| MixError::Config("limiter look-ahead out of range"))?
            .max(1);
        let detector = Detector::new(rate);
        let latency = window + detector.delay() - 1;
        let release_frames = f64::from(config.release_ms) * f64::from(rate.hz()) / 1000.0;
        Ok(Limiter {
            ceiling: 10f64.powf(config.ceiling_dbtp / 20.0),
            config,
            detector,
            hold: MinHold::new(window + 1),
            release: 1.0 - (-1.0 / release_frames.max(1.0)).exp(),
            env: 1.0,
            attack: MovingMean::new(window),
            delay: vec![[0.0; CHANNELS]; latency].into_boxed_slice(),
            delay_pos: 0,
            min_gain: 1.0,
        })
    }

    /// Its configuration.
    #[must_use]
    pub fn config(&self) -> LimiterConfig {
        self.config
    }

    /// Frames between an input frame and the output frame that carries it.
    #[must_use]
    pub fn latency(&self) -> usize {
        self.delay.len()
    }

    /// Frames between an input and the true-peak detector output that describes it.
    #[must_use]
    pub fn detector_delay(&self) -> usize {
        self.detector.delay()
    }

    /// Forgets everything (silence before the next frame), as after a seek. Allocation-free.
    pub fn reset(&mut self) {
        self.detector.reset();
        self.hold.reset();
        self.attack.reset();
        self.env = 1.0;
        self.delay.fill([0.0; CHANNELS]);
        self.delay_pos = 0;
    }

    /// The smallest gain applied since the last call (1.0: no limiting), and restarts the
    /// measurement.
    pub fn take_min_gain(&mut self) -> f32 {
        std::mem::replace(&mut self.min_gain, 1.0)
    }

    /// Limits `buf` (interleaved stereo) in place: each frame is replaced by the frame
    /// [`latency`](Self::latency) frames earlier, scaled by the limiter gain. Non-finite input
    /// counts as silence.
    pub fn process(&mut self, buf: &mut [f32]) {
        let (frames, _) = buf.as_chunks_mut::<CHANNELS>();
        for f in frames {
            let x = [finite(f[0]), finite(f[1])];
            let gain = if self.config.enabled {
                self.gain_for(x)
            } else {
                1.0
            };
            let y = self.delayed(x);
            if gain < 1.0 {
                #[allow(clippy::cast_possible_truncation)]
                let g = gain as f32;
                self.min_gain = self.min_gain.min(g);
                *f = [y[0] * g, y[1] * g];
            } else {
                *f = y;
            }
        }
    }

    /// Runs the gain computer on the input frame `x` and returns the gain for the frame that
    /// leaves the delay line now.
    #[inline]
    fn gain_for(&mut self, x: [f32; CHANNELS]) -> f64 {
        let peak = f64::from(self.detector.push(x));
        let required = if peak > self.ceiling {
            self.ceiling / peak
        } else {
            1.0
        };
        let held = self.hold.push(required);
        if held < self.env {
            self.env = held;
        } else if held > self.env {
            self.env += (held - self.env) * self.release;
            if held - self.env < SNAP {
                self.env = held;
            }
        }
        self.attack.push(self.env)
    }

    /// Pushes `x` into the delay line and returns the frame that leaves it.
    #[inline]
    fn delayed(&mut self, x: [f32; CHANNELS]) -> [f32; CHANNELS] {
        if self.delay.is_empty() {
            return x;
        }
        let out = std::mem::replace(&mut self.delay[self.delay_pos], x);
        self.delay_pos += 1;
        if self.delay_pos == self.delay.len() {
            self.delay_pos = 0;
        }
        out
    }
}

/// `x`, or silence when it is not finite.
#[inline]
fn finite(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::float_cmp,
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation
    )]

    use super::*;

    const RATE: SampleRate = SampleRate::HZ_48_000;

    fn sine(freq: f64, amplitude: f64, frames: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(frames * 2);
        for n in 0..frames {
            let v = (amplitude * (2.0 * std::f64::consts::PI * freq * n as f64 / 48_000.0).sin())
                as f32;
            out.extend_from_slice(&[v, v]);
        }
        out
    }

    /// True peak of `buf` with ebur128 (the measurement of the WBS).
    fn true_peak_db(buf: &[f32]) -> f64 {
        let mut m = ebur128::EbuR128::new(2, 48_000, ebur128::Mode::TRUE_PEAK).unwrap();
        m.add_frames_f32(buf).unwrap();
        let tp = m.true_peak(0).unwrap().max(m.true_peak(1).unwrap());
        20.0 * tp.log10()
    }

    #[test]
    fn defaults_and_latency() {
        let l = Limiter::new(LimiterConfig::default(), RATE).unwrap();
        // 5 ms = 240 frames, plus the 4× detector's 6 frames, minus one.
        assert_eq!(l.detector_delay(), 6);
        assert_eq!(l.latency(), 245);
        let off = Limiter::new(LimiterConfig::disabled(), RATE).unwrap();
        assert_eq!(off.latency(), l.latency(), "a bypass keeps the latency");
        let hi = Limiter::new(LimiterConfig::default(), SampleRate::HZ_96_000).unwrap();
        assert_eq!(hi.detector_delay(), 12);
        assert_eq!(hi.latency(), 480 + 12 - 1);
        let bad = LimiterConfig {
            ceiling_dbtp: 1.0,
            ..LimiterConfig::default()
        };
        assert!(Limiter::new(bad, RATE).is_err());
        let bad = LimiterConfig {
            lookahead_ms: 0,
            ..LimiterConfig::default()
        };
        assert!(Limiter::new(bad, RATE).is_err());
    }

    #[test]
    fn below_the_ceiling_it_is_a_pure_delay() {
        let mut l = Limiter::new(LimiterConfig::default(), RATE).unwrap();
        let input = sine(997.0, 0.85, 4800);
        let mut buf = input.clone();
        l.process(&mut buf);
        let d = l.latency() * 2;
        assert!(buf[..d].iter().all(|&s| s == 0.0));
        assert_eq!(&buf[d..], &input[..input.len() - d]);
        assert_eq!(l.take_min_gain(), 1.0);
    }

    #[test]
    fn block_size_does_not_matter() {
        let input = sine(1234.0, 1.8, 9600);
        let mut whole = input.clone();
        Limiter::new(LimiterConfig::default(), RATE)
            .unwrap()
            .process(&mut whole);
        for block in [1usize, 7, 256, 4096] {
            let mut l = Limiter::new(LimiterConfig::default(), RATE).unwrap();
            let mut pieces = input.clone();
            for chunk in pieces.chunks_mut(block * 2) {
                l.process(chunk);
            }
            assert_eq!(pieces, whole, "block {block}");
        }
    }

    /// WBS 2E.02: a +6 dBFS sine comes out at most 0.1 dB over the ceiling (true peak measured
    /// with ebur128), for low, mid and high frequencies, at the default and another ceiling.
    #[test]
    fn hot_sines_stay_under_the_true_peak_ceiling() {
        for (freq, ceiling) in [
            (100.0, -1.0),
            (997.0, -1.0),
            (5_000.0, -1.0),
            (11_025.0, -1.0),
            (17_000.0, -1.0),
            (997.0, -6.0),
        ] {
            let config = LimiterConfig {
                ceiling_dbtp: ceiling,
                ..LimiterConfig::default()
            };
            let mut l = Limiter::new(config, RATE).unwrap();
            let mut buf = sine(freq, 2.0, 48_000);
            l.process(&mut buf);
            let tp = true_peak_db(&buf);
            assert!(
                tp <= ceiling + 0.1,
                "{freq} Hz: {tp:.3} dBTP over {ceiling}"
            );
            // And it does not crush the signal: within 1 dB of the ceiling once settled.
            let tail = &buf[24_000..];
            let peak = tail.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            assert!(
                20.0 * f64::from(peak).log10() > ceiling - 1.5,
                "{freq} Hz: peak {peak}"
            );
            assert!(l.take_min_gain() < 0.5);
        }
    }

    #[test]
    fn a_sudden_peak_is_caught_before_it_plays() {
        let mut l = Limiter::new(LimiterConfig::default(), RATE).unwrap();
        // Silence, then one full-scale burst of 2.0.
        let mut buf = vec![0.0f32; 2 * 2000];
        for s in &mut buf[2 * 1000..2 * 1010] {
            *s = 2.0;
        }
        l.process(&mut buf);
        let ceiling = 10f32.powf(-1.0 / 20.0);
        let peak = buf.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak <= ceiling * 1.0001, "{peak}");
        assert!(true_peak_db(&buf) <= -1.0 + 0.1);
    }

    #[test]
    fn the_gain_recovers_after_the_release() {
        let mut l = Limiter::new(LimiterConfig::default(), RATE).unwrap();
        let mut loud = sine(500.0, 2.0, 4800);
        l.process(&mut loud);
        // Two seconds of a quiet tone later (20 release time constants) the gain is back to
        // exactly 1.
        let quiet = sine(500.0, 0.25, 48_000 * 3);
        let mut buf = quiet.clone();
        l.process(&mut buf);
        let d = l.latency() * 2;
        let late = 48_000 * 2 * 2;
        assert_eq!(&buf[late..], &quiet[late - d..quiet.len() - d]);
    }

    #[test]
    fn non_finite_input_is_silence_and_reset_forgets() {
        let mut l = Limiter::new(LimiterConfig::default(), RATE).unwrap();
        let mut buf = vec![f32::NAN, f32::INFINITY];
        buf.extend(sine(440.0, 0.5, 1000));
        l.process(&mut buf);
        assert!(buf.iter().all(|s| s.is_finite()));
        let mut loud = sine(440.0, 3.0, 1000);
        l.process(&mut loud);
        l.reset();
        let mut zero = vec![0.0f32; 2 * 1000];
        l.process(&mut zero);
        assert!(zero.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn disabled_is_a_plain_delay() {
        let mut l = Limiter::new(LimiterConfig::disabled(), RATE).unwrap();
        let input = sine(997.0, 2.0, 2000);
        let mut buf = input.clone();
        l.process(&mut buf);
        let d = l.latency() * 2;
        assert_eq!(&buf[d..], &input[..input.len() - d]);
        assert_eq!(l.take_min_gain(), 1.0);
    }
}
