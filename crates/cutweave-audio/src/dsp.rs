// SPDX-License-Identifier: GPL-3.0-or-later
//! Allocation-free sample math shared by the real-time graph and the offline mixdown: gains,
//! per-frame gain ramps, the master soft limiter and the stereo down-mix.
//!
//! All buffers are interleaved stereo ([`CHANNELS`] samples per frame).

/// Channels of every buffer inside the graph and of the mixer output (interleaved L, R).
pub const CHANNELS: usize = 2;

/// Gains at or below this level are treated as silence (−144 dB, below 24-bit resolution).
const SILENCE_DB: f64 = -144.0;
/// Largest accepted gain (+24 dB); larger requests are clamped.
const MAX_DB: f64 = 24.0;

/// Linear gain of `db` decibels. NaN means unity; −∞ and anything ≤ −144 dB are silence;
/// values above +24 dB are clamped.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub fn db_to_gain(db: f64) -> f32 {
    if db.is_nan() {
        1.0
    } else if db <= SILENCE_DB {
        0.0
    } else {
        10f64.powf(db.min(MAX_DB) / 20.0) as f32
    }
}

/// Start of the soft limiter's knee: −1 dBFS. Below it the limiter is exactly transparent.
pub const LIMITER_KNEE: f32 = 0.891_250_96;

/// The master limiter placeholder (WBS 1G.01): a memoryless soft clipper at 0 dBFS.
///
/// Identity up to [`LIMITER_KNEE`]; above it the excess is compressed with `u / (1 + u)`, which
/// meets the identity with slope 1 at the knee and approaches ±1.0 without reaching it, so the
/// output never exceeds 0 dBFS. No `tanh`, no state (so it is independent of buffer sizes), and
/// non-finite input becomes silence. The P2 limiter (look-ahead, true peak) replaces it.
#[must_use]
pub fn soft_limit(x: f32) -> f32 {
    if !x.is_finite() {
        return 0.0;
    }
    let a = x.abs();
    if a <= LIMITER_KNEE {
        return x;
    }
    let range = 1.0 - LIMITER_KNEE;
    let u = (a - LIMITER_KNEE) / range;
    let y = LIMITER_KNEE + range * (u / (1.0 + u));
    y.copysign(x)
}

/// A gain that moves linearly to a new target over a fixed number of frames (no zipper noise),
/// and is an exact constant otherwise.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ramp {
    current: f32,
    target: f32,
    step: f32,
    left: u32,
}

impl Ramp {
    /// A steady gain of `gain`.
    #[must_use]
    pub const fn at(gain: f32) -> Ramp {
        Ramp {
            current: gain,
            target: gain,
            step: 0.0,
            left: 0,
        }
    }

    /// Heads to `target` over `frames` frames from the current value (at once when `frames` is
    /// 0). A target equal to the current one leaves the ramp untouched.
    #[allow(clippy::cast_precision_loss)]
    pub fn set_target(&mut self, target: f32, frames: u32) {
        if target.to_bits() == self.target.to_bits() {
            return;
        }
        self.target = target;
        if frames == 0 {
            self.current = target;
            self.left = 0;
        } else {
            self.step = (target - self.current) / frames as f32;
            self.left = frames;
        }
    }

    /// The largest gain magnitude the ramp applies from now to its target (meters scale a
    /// pre-gain peak by it).
    #[must_use]
    pub fn max_gain(&self) -> f32 {
        self.current.abs().max(self.target.abs())
    }

    /// Gain of the next frame.
    #[inline]
    fn next(&mut self) -> f32 {
        if self.left == 0 {
            return self.current;
        }
        self.left -= 1;
        if self.left == 0 {
            self.current = self.target;
            self.step = 0.0;
        } else {
            self.current += self.step;
        }
        self.current
    }

    /// `out += src · gain`, frame by frame (stereo).
    pub fn add_scaled(&mut self, out: &mut [f32], src: &[f32]) {
        let (out, _) = out.as_chunks_mut::<CHANNELS>();
        let (src, _) = src.as_chunks::<CHANNELS>();
        if self.left == 0 {
            let g = self.current;
            for (o, s) in out.iter_mut().zip(src) {
                o[0] += s[0] * g;
                o[1] += s[1] * g;
            }
            return;
        }
        for (o, s) in out.iter_mut().zip(src) {
            let g = self.next();
            o[0] += s[0] * g;
            o[1] += s[1] * g;
        }
    }

    /// `buf *= gain`, frame by frame (stereo).
    pub fn scale(&mut self, buf: &mut [f32]) {
        let (buf, _) = buf.as_chunks_mut::<CHANNELS>();
        if self.left == 0 {
            if self.current.to_bits() != 1.0f32.to_bits() {
                let g = self.current;
                for f in buf {
                    f[0] *= g;
                    f[1] *= g;
                }
            }
            return;
        }
        for f in buf {
            let g = self.next();
            f[0] *= g;
            f[1] *= g;
        }
    }
}

/// Peak absolute value of the left and right channels of `buf` (interleaved stereo).
#[must_use]
pub fn peak(buf: &[f32]) -> [f32; 2] {
    let (frames, _) = buf.as_chunks::<CHANNELS>();
    frames.iter().fold([0.0f32; 2], |p, f| {
        [p[0].max(f[0].abs()), p[1].max(f[1].abs())]
    })
}

/// The master stage: master gain, then the soft limiter. Shared by the real-time graph and the
/// offline mixdown.
pub fn master(buf: &mut [f32], gain: &mut Ramp) {
    gain.scale(buf);
    for s in buf {
        *s = soft_limit(*s);
    }
}

/// ITU-R BS.775 down-mix coefficient of the centre and surround channels (−3 dB).
const DOWNMIX_SIDE: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// Down-mixes `src` (interleaved, `channels` per frame) into stereo `dst` (whole frames of
/// both). Mono is copied to both sides; 5.1 (L R C LFE Ls Rs) uses BS.775 (LFE dropped); any
/// other layout keeps its first two channels.
pub fn downmix_into(src: &[f32], channels: usize, dst: &mut [f32]) {
    let (dst, _) = dst.as_chunks_mut::<CHANNELS>();
    match channels {
        0 => {}
        1 => {
            for (d, &s) in dst.iter_mut().zip(src) {
                *d = [s, s];
            }
        }
        2 => {
            for (d, s) in dst.iter_mut().zip(src.as_chunks::<2>().0) {
                *d = *s;
            }
        }
        6 => {
            for (d, s) in dst.iter_mut().zip(src.as_chunks::<6>().0) {
                let centre = s[2] * DOWNMIX_SIDE;
                *d = [
                    s[0] + centre + s[4] * DOWNMIX_SIDE,
                    s[1] + centre + s[5] * DOWNMIX_SIDE,
                ];
            }
        }
        n => {
            for (d, s) in dst.iter_mut().zip(src.chunks_exact(n)) {
                *d = [s[0], s[1]];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)]

    use super::*;

    #[test]
    fn gains() {
        assert_eq!(db_to_gain(0.0), 1.0);
        assert!((db_to_gain(-6.020_6) - 0.5).abs() < 1e-5);
        assert_eq!(db_to_gain(f64::NEG_INFINITY), 0.0);
        assert_eq!(db_to_gain(-200.0), 0.0);
        assert_eq!(db_to_gain(f64::NAN), 1.0);
        assert!((db_to_gain(100.0) - db_to_gain(24.0)).abs() < f32::EPSILON);
    }

    #[test]
    fn limiter_is_transparent_below_the_knee_and_never_reaches_full_scale() {
        for x in [0.0, 0.1, -0.5, LIMITER_KNEE, -LIMITER_KNEE] {
            assert_eq!(soft_limit(x), x);
        }
        let mut last = LIMITER_KNEE;
        for i in 1..=10_000 {
            #[allow(clippy::cast_precision_loss)]
            let x = LIMITER_KNEE + i as f32 * 0.01;
            let y = soft_limit(x);
            assert!(y >= last && y < 1.0, "{x} → {y}");
            assert_eq!(soft_limit(-x), -y);
            last = y;
        }
        assert_eq!(soft_limit(f32::NAN), 0.0);
        assert_eq!(soft_limit(f32::INFINITY), 0.0);
        // Slope 1 at the knee: no corner.
        let d = 1e-4;
        let slope = (soft_limit(LIMITER_KNEE + d) - LIMITER_KNEE) / d;
        assert!((slope - 1.0).abs() < 1e-2, "{slope}");
    }

    #[test]
    fn ramps_reach_their_target_exactly_and_are_split_invariant() {
        let src = vec![1.0f32; 2 * 100];
        let mut whole = Ramp::at(1.0);
        whole.set_target(0.0, 64);
        let mut a = vec![0.0f32; 200];
        whole.add_scaled(&mut a, &src);

        let mut split = Ramp::at(1.0);
        split.set_target(0.0, 64);
        let mut b = vec![0.0f32; 200];
        split.add_scaled(&mut b[..34], &src[..34]);
        split.add_scaled(&mut b[34..], &src[34..]);
        assert_eq!(a, b);
        assert_eq!(a[2 * 63], 0.0);
        assert_eq!(a[2 * 99], 0.0);
        assert!(a[0] < 1.0 && a[0] > 0.98);
        assert_eq!(whole, Ramp::at(0.0));
    }

    #[test]
    fn downmix_layouts() {
        let mut out = [0.0; 4];
        downmix_into(&[0.5, -0.25], 1, &mut out);
        assert_eq!(out, [0.5, 0.5, -0.25, -0.25]);
        downmix_into(&[0.1, 0.2, 0.3, 0.4], 2, &mut out);
        assert_eq!(out, [0.1, 0.2, 0.3, 0.4]);
        let mut one = [0.0; 2];
        downmix_into(&[0.1, 0.2, 0.5, 9.0, 0.3, 0.4], 6, &mut one);
        let k = DOWNMIX_SIDE;
        assert_eq!(one, [0.1 + 0.5 * k + 0.3 * k, 0.2 + 0.5 * k + 0.4 * k]);
        downmix_into(&[0.1, 0.2, 0.3], 3, &mut one);
        assert_eq!(one, [0.1, 0.2]);
    }
}
