// SPDX-License-Identifier: GPL-3.0-or-later
//! Simple look-ahead brick-wall peak limiter for the master bus (real-time safe: no allocation
//! after construction).
//!
//! Gain computer: per-frame target `min(1, ceiling / peak)`; a sliding minimum over the look-ahead
//! window, instant attack / exponential release, then a box filter of the window length. Because
//! every value averaged by the box filter is at most the target of the frame leaving the delay line,
//! the output sample peak never exceeds the ceiling (true peak can overshoot slightly; the product
//! limiter in P2 adds oversampling).

/// Stereo look-ahead limiter.
#[derive(Debug)]
pub struct Limiter {
    ceiling: f32,
    len: usize,
    delay: Vec<f32>,
    targets: Vec<f32>,
    smooth: Vec<f32>,
    sum: f64,
    pos_delay: usize,
    pos_target: usize,
    pos_smooth: usize,
    r_prev: f32,
    release_coef: f32,
    since_resum: u32,
}

impl Limiter {
    /// Limiter at `rate` with the given ceiling (dBFS), look-ahead (ms) and release (ms).
    pub fn new(rate: u32, ceiling_db: f32, lookahead_ms: f32, release_ms: f32) -> Self {
        let len = ((lookahead_ms / 1000.0 * rate as f32).round() as usize).max(1);
        let release_coef = 1.0 - (-1.0 / (release_ms / 1000.0 * rate as f32)).exp();
        Self {
            ceiling: crate::util::db_to_lin(ceiling_db),
            len,
            delay: vec![0.0; len * 2],
            targets: vec![1.0; len + 1],
            smooth: vec![1.0; len],
            sum: len as f64,
            pos_delay: 0,
            pos_target: 0,
            pos_smooth: 0,
            r_prev: 1.0,
            release_coef,
            since_resum: 0,
        }
    }

    /// Added latency in frames.
    pub fn latency(&self) -> usize {
        self.len
    }

    /// Ceiling as linear amplitude.
    pub fn ceiling(&self) -> f32 {
        self.ceiling
    }

    /// Limits interleaved stereo `buf` in place; returns the smallest gain applied (1.0 = none).
    pub fn process(&mut self, buf: &mut [f32]) -> f32 {
        let mut min_gain = 1.0f32;
        for frame in buf.as_chunks_mut::<2>().0 {
            let (l, r) = (frame[0], frame[1]);
            let peak = l.abs().max(r.abs());
            let t = if peak > self.ceiling {
                self.ceiling / peak
            } else {
                1.0
            };
            self.targets[self.pos_target] = t;
            self.pos_target += 1;
            if self.pos_target == self.targets.len() {
                self.pos_target = 0;
            }
            let window_min = self.targets.iter().copied().fold(1.0f32, f32::min);
            let released = self.r_prev + (1.0 - self.r_prev) * self.release_coef;
            let rr = window_min.min(released);
            self.r_prev = rr;
            self.sum += f64::from(rr) - f64::from(self.smooth[self.pos_smooth]);
            self.smooth[self.pos_smooth] = rr;
            self.pos_smooth += 1;
            if self.pos_smooth == self.len {
                self.pos_smooth = 0;
            }
            self.since_resum += 1;
            if self.since_resum >= 1 << 16 {
                self.since_resum = 0;
                self.sum = self.smooth.iter().map(|v| f64::from(*v)).sum();
            }
            let g = (self.sum / self.len as f64) as f32;
            min_gain = min_gain.min(g);
            let i = self.pos_delay * 2;
            let (dl, dr) = (self.delay[i], self.delay[i + 1]);
            self.delay[i] = l;
            self.delay[i + 1] = r;
            self.pos_delay += 1;
            if self.pos_delay == self.len {
                self.pos_delay = 0;
            }
            frame[0] = dl * g;
            frame[1] = dr * g;
        }
        min_gain
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::{db_to_lin, lin_to_db};

    fn sine(amp: f32, freq: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let v = amp * (std::f32::consts::TAU * freq * i as f32 / 48_000.0).sin();
                [v, v * 0.7]
            })
            .collect()
    }

    #[test]
    fn hot_signal_never_exceeds_ceiling() {
        // +6 dBFS sine, 10 s, ceiling -1 dBFS.
        let mut lim = Limiter::new(48_000, -1.0, 1.5, 100.0);
        let mut buf = sine(db_to_lin(6.0), 997.0, 480_000);
        for block in buf.chunks_mut(512) {
            lim.process(block);
        }
        let peak = buf.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(
            lin_to_db(peak) <= -1.0 + 1e-4,
            "peak {} dBFS",
            lin_to_db(peak)
        );
        // And it is actually limiting to (close to) the ceiling, not just muting.
        assert!(lin_to_db(peak) > -1.5);
    }

    #[test]
    fn quiet_signal_passes_unchanged_after_latency() {
        let mut lim = Limiter::new(48_000, -1.0, 1.5, 100.0);
        let input = sine(0.25, 440.0, 4_800);
        let mut buf = input.clone();
        let g = lim.process(&mut buf);
        assert_eq!(g, 1.0);
        let lat = lim.latency();
        for i in 0..(4_800 - lat) {
            assert!((buf[(i + lat) * 2] - input[i * 2]).abs() < 1e-6);
        }
    }

    #[test]
    fn transient_is_caught_by_lookahead() {
        let mut lim = Limiter::new(48_000, -1.0, 1.5, 100.0);
        let mut buf = vec![0.0f32; 2 * 4_800];
        buf[2 * 1_000] = 4.0; // single +12 dBFS spike
        buf[2 * 1_000 + 1] = -4.0;
        lim.process(&mut buf);
        let peak = buf.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak <= db_to_lin(-1.0) * (1.0 + 1e-5), "peak {peak}");
    }
}
