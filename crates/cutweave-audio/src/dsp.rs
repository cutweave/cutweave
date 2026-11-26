// SPDX-License-Identifier: GPL-3.0-or-later
//! Allocation-free sample math shared by the real-time graph and the offline mixdown: gains,
//! per-frame gain ramps and the stereo down-mix. The master limiter is [`crate::Limiter`].
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
    fn ramps_reach_their_target_exactly_and_are_split_invariant() {
        let mut whole = Ramp::at(1.0);
        whole.set_target(0.0, 64);
        let mut a = vec![1.0f32; 200];
        whole.scale(&mut a);

        let mut split = Ramp::at(1.0);
        split.set_target(0.0, 64);
        let mut b = vec![1.0f32; 200];
        split.scale(&mut b[..34]);
        split.scale(&mut b[34..]);
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
