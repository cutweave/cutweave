// SPDX-License-Identifier: GPL-3.0-or-later
//! Synthetic stereo sources standing in for decoded clips (media decode is out of scope).
//!
//! Every source renders interleaved stereo `f32` at its own sample rate. Event times (clicks,
//! bursts) are derived from the integer sample index, so they are exact on the source timeline.

use std::f64::consts::TAU;

/// What a source plays.
#[derive(Clone, Debug)]
pub enum SourceKind {
    /// Sum of partials `(frequency Hz, amplitude)` with an optional tremolo `(rate Hz, depth 0..1)`.
    /// `detune` offsets the right channel's frequencies (Hz) for a wider image.
    Tone {
        /// Partials.
        partials: Vec<(f64, f32)>,
        /// Optional amplitude modulation.
        tremolo: Option<(f64, f32)>,
        /// Right-channel detune in Hz.
        detune: f64,
    },
    /// A/V reference click: a decaying sine burst starting exactly at `k * interval` (k >= 1).
    Clicks {
        /// Seconds between clicks (first click at `interval`).
        interval: f64,
        /// Peak amplitude.
        amp: f32,
        /// Burst carrier frequency (Hz).
        freq: f64,
    },
    /// Decorrelated white-noise bursts of `burst` seconds every `period` seconds.
    NoiseBursts {
        /// Seconds between burst starts.
        period: f64,
        /// Burst length in seconds.
        burst: f64,
        /// Amplitude.
        amp: f32,
    },
}

/// Click shape: decay time constant and total length (seconds).
pub const CLICK_TAU_S: f64 = 0.003;
/// Total click length (seconds); after this the click track is silent until the next one.
pub const CLICK_LEN_S: f64 = 0.020;

/// A running synthetic source.
#[derive(Debug)]
pub struct Source {
    kind: SourceKind,
    rate: u32,
    /// Next frame index to render.
    pos: u64,
    phases: Vec<[f64; 2]>,
    trem_phase: f64,
    rng: [u64; 2],
}

impl Source {
    /// New source at `rate` Hz.
    pub fn new(kind: SourceKind, rate: u32, seed: u64) -> Self {
        let n = match &kind {
            SourceKind::Tone { partials, .. } => partials.len(),
            _ => 0,
        };
        Self {
            kind,
            rate,
            pos: 0,
            phases: vec![[0.0; 2]; n],
            trem_phase: 0.0,
            rng: [seed | 1, (seed ^ 0x9E37_79B9_7F4A_7C15) | 1],
        }
    }

    /// Source sample rate.
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Renders `out.len() / 2` stereo frames.
    pub fn fill(&mut self, out: &mut [f32]) {
        let rate = f64::from(self.rate);
        match &self.kind {
            SourceKind::Tone {
                partials,
                tremolo,
                detune,
            } => {
                for frame in out.as_chunks_mut::<2>().0 {
                    let trem = match tremolo {
                        Some((r, depth)) => {
                            self.trem_phase = (self.trem_phase + r / rate).fract();
                            1.0 - depth * 0.5 * (1.0 - (TAU * self.trem_phase).cos() as f32)
                        }
                        None => 1.0,
                    };
                    let (mut l, mut r) = (0.0f32, 0.0f32);
                    for ((f, a), ph) in partials.iter().zip(self.phases.iter_mut()) {
                        l += a * (TAU * ph[0]).sin() as f32;
                        r += a * (TAU * ph[1]).sin() as f32;
                        ph[0] = (ph[0] + f / rate).fract();
                        ph[1] = (ph[1] + (f + detune) / rate).fract();
                    }
                    frame[0] = l * trem;
                    frame[1] = r * trem;
                }
            }
            SourceKind::Clicks {
                interval,
                amp,
                freq,
            } => {
                let period = interval * rate;
                let len = CLICK_LEN_S * rate;
                for (i, frame) in out.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                    let n = self.pos + i as u64;
                    let v = click_value(n, period, len, rate, *freq, *amp);
                    frame[0] = v;
                    frame[1] = v;
                }
            }
            SourceKind::NoiseBursts { period, burst, amp } => {
                let period_f = period * rate;
                let burst_f = burst * rate;
                let ramp = 0.005 * rate;
                for (i, frame) in out.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                    let n = (self.pos + i as u64) as f64;
                    let t = n - (n / period_f).floor() * period_f;
                    let env = if t < burst_f {
                        (t / ramp).min(1.0).min((burst_f - t) / ramp) as f32
                    } else {
                        0.0
                    };
                    let a = next_noise(&mut self.rng[0]);
                    let b = next_noise(&mut self.rng[1]);
                    frame[0] = amp * env * a;
                    frame[1] = amp * env * b;
                }
            }
        }
        self.pos += (out.len() / 2) as u64;
    }
}

/// Exact start frame of click `k` (k >= 1) at `rate` for a click every `interval` seconds.
pub fn click_start_frame(k: u64, interval: f64, rate: f64) -> f64 {
    k as f64 * interval * rate
}

fn click_value(n: u64, period: f64, len: f64, rate: f64, freq: f64, amp: f32) -> f32 {
    // Latest click start (rounded to the sample grid) at or before n; clicks are k >= 1.
    let nf = n as f64;
    let mut k = ((nf + 0.5) / period).floor();
    let mut start = (k * period).round();
    if start > nf {
        k -= 1.0;
        start = (k * period).round();
    }
    if k < 1.0 {
        return 0.0;
    }
    let d = nf - start;
    if !(0.0..len).contains(&d) {
        return 0.0;
    }
    let t = d / rate;
    amp * ((TAU * freq * t).sin() * (-t / CLICK_TAU_S).exp()) as f32
}

/// xorshift64 white noise in [-1, 1).
#[inline]
fn next_noise(state: &mut u64) -> f32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    ((x >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clicks_start_exactly_on_the_timeline() {
        let mut s = Source::new(
            SourceKind::Clicks {
                interval: 0.5,
                amp: 0.8,
                freq: 3000.0,
            },
            44_100,
            1,
        );
        let mut buf = vec![0.0f32; 2 * 44_100];
        s.fill(&mut buf);
        let first_nonzero = buf
            .as_chunks::<2>()
            .0
            .iter()
            .position(|f| f[0] != 0.0)
            .unwrap();
        // sin(0) = 0 at the exact start, so the first non-zero sample is start + 1.
        assert_eq!(first_nonzero, 22_050 + 1);
        assert!(buf[..2 * 22_050].iter().all(|v| *v == 0.0));
    }

    #[test]
    fn tone_level_matches_amplitude() {
        let amp = 10f32.powf(-23.0 / 20.0);
        let mut s = Source::new(
            SourceKind::Tone {
                partials: vec![(1000.0, amp)],
                tremolo: None,
                detune: 0.0,
            },
            48_000,
            1,
        );
        let mut buf = vec![0.0f32; 2 * 48_000];
        s.fill(&mut buf);
        let peak = buf.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((peak - amp).abs() < 1e-4);
    }
}
