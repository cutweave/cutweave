// SPDX-License-Identifier: GPL-3.0-or-later
//! Per-clip automation (WP-2E.01): keyframed gain and pan, fades in and out, and the pan law,
//! applied sample-accurately on the worker side of a lane.
//!
//! **Smoothing.** The envelope is evaluated at control points every [`GRID`] frames of the clip
//! (clip-relative frame `j · GRID`, never relative to a block) and the gains in between are
//! linear ramps. A hold keyframe or a fade edge therefore never steps: it ramps over at most
//! [`GRID`] frames (below the 64-frame limit of the WBS), and, because the grid is anchored to
//! the clip, the result does not depend on how the lane is cut into blocks. The offline mixdown
//! and the real-time workers run this same code, which is what makes preview and export equal.
//!
//! **Pan law.** A mono source is panned with a constant-power law: centre is −3 dB in each
//! channel, hard left or right is 0 dB in one channel. A stereo (or wider) source keeps its
//! image and is balanced instead: unity at centre, the far side attenuated by the same
//! constant-power curve towards the edge, so a default clip is not changed.
//!
//! Nothing here blocks or locks. [`Automation::new`] allocates nothing either (cloning the
//! shared key list is O(1)), and [`Automation::mix`] only reads the evaluators' cursors.

#![allow(clippy::many_single_char_names)]

use cutweave_model::{Animatable, Evaluator, Fade, FadeCurve, PropId, Rounding, SampleRate, Time};
use cutweave_render::AudioSegment;

use crate::dsp::{CHANNELS, db_to_gain};

/// Frames between control points: the 64-frame limit of WBS 2E.01, which keeps the step of a
/// full-scale gain change (a hold from 0 dB to silence) under 0.02 per frame (1/64).
pub(crate) const GRID: i64 = 64;

/// A fade in samples at the output rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FadeFrames {
    pub frames: i64,
    pub curve: FadeCurve,
}

/// Everything about one clip's gain, pan and fades, on the output sample grid. Cheap to clone
/// (the key lists are shared with the document).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Envelope {
    /// Gain in dB over item time.
    pub gain: Animatable<f64>,
    /// Pan `-1..=1` over item time.
    pub pan: Animatable<f64>,
    /// Output sample of the clip's first frame.
    pub origin: i64,
    /// Length of the whole clip in output samples.
    pub len: i64,
    /// Fade in, clamped to the clip.
    pub fade_in: FadeFrames,
    /// Fade out, clamped to the clip.
    pub fade_out: FadeFrames,
}

impl Envelope {
    /// The envelope of a segment's clip at the output rate `rate`. `Err` only when a time cannot
    /// be represented.
    pub fn of_segment(
        seg: &AudioSegment,
        rate: SampleRate,
    ) -> Result<Envelope, cutweave_model::TimeError> {
        let end = seg.clip_start.checked_add(seg.clip_duration)?;
        let origin = rate.time_to_samples(seg.clip_start, Rounding::Nearest);
        let len = (rate.time_to_samples(end, Rounding::Nearest) - origin).max(0);
        let (fade_in, fade_out) = fit_fades(
            fade_frames(seg.fade_in, rate),
            fade_frames(seg.fade_out, rate),
            len,
        );
        Ok(Envelope {
            gain: seg.gain.clone(),
            pan: seg.pan.clone(),
            origin,
            len,
            fade_in,
            fade_out,
        })
    }

    /// A flat envelope of linear gain `gain` over `[origin, origin + len)` (tests).
    #[cfg(test)]
    pub fn flat(gain: f32, origin: i64, len: i64) -> Envelope {
        let db = if gain > 0.0 {
            20.0 * f64::from(gain).log10()
        } else {
            -200.0
        };
        let none = FadeFrames {
            frames: 0,
            curve: FadeCurve::Linear,
        };
        Envelope {
            gain: Animatable::Static(db),
            pan: Animatable::Static(0.0),
            origin,
            len,
            fade_in: none,
            fade_out: none,
        }
    }
}

fn fade_frames(fade: Fade, rate: SampleRate) -> FadeFrames {
    let frames = Time::from_flicks(fade.duration.flicks())
        .map_or(0, |t| rate.time_to_samples(t, Rounding::Nearest));
    FadeFrames {
        frames: frames.max(0),
        curve: fade.curve,
    }
}

/// Fits both fades into `len` frames: each at most `len`, and when together they are longer,
/// both are scaled down in proportion (rounding the fade in down, so the sum is exactly `len`).
fn fit_fades(mut a: FadeFrames, mut b: FadeFrames, len: i64) -> (FadeFrames, FadeFrames) {
    a.frames = a.frames.min(len);
    b.frames = b.frames.min(len);
    let total = a.frames + b.frames;
    if total > len {
        let scaled = i128::from(a.frames) * i128::from(len) / i128::from(total);
        a.frames = i64::try_from(scaled).unwrap_or(0);
        b.frames = len - a.frames;
    }
    (a, b)
}

/// Linear gains `[left, right]` of a clip at one control point.
type Gains = [f32; 2];

/// The state of one playing clip's automation: its evaluators and the control points around
/// the position.
pub(crate) struct Automation {
    env: Envelope,
    rate: SampleRate,
    mono: bool,
    gain: Evaluator<f64>,
    pan: Evaluator<f64>,
    /// `Some` when the envelope is flat: the gains of every frame.
    constant: Option<Gains>,
    /// Index of the control point `p0` (the point at clip frame `min(cell · GRID, len)`).
    cell: i64,
    p0: Gains,
    p1: Gains,
}

impl Automation {
    /// The automation of a clip with `env`; `mono` is whether its source has one channel (the
    /// pan law differs).
    pub fn new(env: Envelope, rate: SampleRate, mono: bool) -> Automation {
        let flat = env.gain.keyframes().is_none()
            && env.pan.keyframes().is_none()
            && env.fade_in.frames == 0
            && env.fade_out.frames == 0;
        let mut a = Automation {
            gain: env.gain.evaluator(),
            pan: env.pan.evaluator(),
            env,
            rate,
            mono,
            constant: None,
            cell: i64::MIN,
            p0: [1.0; 2],
            p1: [1.0; 2],
        };
        if flat {
            a.constant = Some(a.point(0));
        }
        a
    }

    /// Clip frame of control point `j`.
    fn position(&self, j: i64) -> i64 {
        (j * GRID).min(self.env.len)
    }

    /// The gains at control point `j`.
    #[allow(clippy::cast_precision_loss)]
    fn point(&mut self, j: i64) -> Gains {
        let k = self.position(j);
        let t = self.rate.samples_to_time(k).unwrap_or(Time::ZERO);
        let db = PropId::AudioGain.spec().clamp.clamp(self.gain.eval(t));
        let pan = PropId::AudioPan.spec().clamp.clamp(self.pan.eval(t));
        let mut g = f64::from(db_to_gain(db));
        let (fi, fo) = (self.env.fade_in, self.env.fade_out);
        if fi.frames > 0 && k < fi.frames {
            g *= fi.curve.gain(k as f64 / fi.frames as f64);
        }
        if fo.frames > 0 && k > self.env.len - fo.frames {
            g *= fo.curve.gain((self.env.len - k) as f64 / fo.frames as f64);
        }
        let [l, r] = pan_gains(pan, self.mono);
        #[allow(clippy::cast_possible_truncation)]
        [(g * l) as f32, (g * r) as f32]
    }

    /// Makes `p0` and `p1` the control points around cell `j`.
    fn seek_cell(&mut self, j: i64) {
        if self.cell != i64::MIN && j == self.cell + 1 {
            self.p0 = self.p1;
        } else {
            self.p0 = self.point(j);
        }
        self.p1 = self.point(j + 1);
        self.cell = j;
    }

    /// `out += src · gains`, for the frames starting at output sample `pos` (`src` and `out`
    /// are interleaved stereo of the same length). Frames outside the clip are silent.
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_wrap)]
    pub fn mix(&mut self, pos: i64, src: &[f32], out: &mut [f32]) {
        let k0 = pos - self.env.origin;
        let (out, _) = out.as_chunks_mut::<CHANNELS>();
        let (src, _) = src.as_chunks::<CHANNELS>();
        if let Some([l, r]) = self.constant {
            for (o, s) in out.iter_mut().zip(src) {
                o[0] += s[0] * l;
                o[1] += s[1] * r;
            }
            return;
        }
        for (i, (o, s)) in out.iter_mut().zip(src).enumerate() {
            let k = k0 + i as i64;
            if k < 0 || k >= self.env.len {
                continue;
            }
            let j = k / GRID;
            if j != self.cell {
                self.seek_cell(j);
            }
            let (a, b) = (self.position(j), self.position(j + 1));
            let (g0, g1) = (self.p0, self.p1);
            if b <= a || k == a {
                o[0] += s[0] * g0[0];
                o[1] += s[1] * g0[1];
            } else {
                let f = (k - a) as f32 / (b - a) as f32;
                o[0] += s[0] * (g0[0] + (g1[0] - g0[0]) * f);
                o[1] += s[1] * (g0[1] + (g1[1] - g0[1]) * f);
            }
        }
    }
}

/// The per-channel gains of `pan` (`-1` left … `1` right), see the module docs.
#[must_use]
pub(crate) fn pan_gains(pan: f64, mono: bool) -> [f64; 2] {
    let theta = (pan.clamp(-1.0, 1.0) + 1.0) * std::f64::consts::FRAC_PI_4;
    let (l, r) = (theta.cos(), theta.sin());
    if mono {
        [l, r]
    } else {
        // Balance: unity on the near side, constant-power attenuation on the far side.
        [
            (l * std::f64::consts::SQRT_2).min(1.0),
            (r * std::f64::consts::SQRT_2).min(1.0),
        ]
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::float_cmp,
        clippy::unwrap_used,
        clippy::cast_precision_loss,
        clippy::cast_possible_wrap
    )]

    use cutweave_model::{Interp, Keyframe, Keyframes};

    use super::*;

    const RATE: SampleRate = SampleRate::HZ_48_000;

    fn ones(frames: usize) -> Vec<f32> {
        vec![1.0; frames * CHANNELS]
    }

    fn left(a: &mut Automation, k0: i64, frames: usize) -> Vec<f32> {
        let src = ones(frames);
        let mut out = vec![0.0; frames * CHANNELS];
        a.mix(k0, &src, &mut out);
        out.iter().step_by(CHANNELS).copied().collect()
    }

    fn env(len: i64, fade_in: i64, fade_out: i64, curve: FadeCurve) -> Envelope {
        let mut e = Envelope::flat(1.0, 0, len);
        e.fade_in = FadeFrames {
            frames: fade_in,
            curve,
        };
        e.fade_out = FadeFrames {
            frames: fade_out,
            curve,
        };
        e
    }

    #[test]
    fn a_flat_envelope_is_a_constant() {
        let mut a = Automation::new(Envelope::flat(0.5, 0, 1000), RATE, false);
        assert!(left(&mut a, 0, 1000).iter().all(|&g| g == 0.5));
    }

    #[test]
    fn linear_fade_in_is_half_way_at_half_the_fade() {
        let mut a = Automation::new(env(96_000, 48_000, 0, FadeCurve::Linear), RATE, false);
        let g = left(&mut a, 0, 48_001);
        assert_eq!(g[0], 0.0);
        assert!((g[24_000] - 0.5).abs() < 1e-6, "{}", g[24_000]);
        assert!((g[48_000] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn the_result_does_not_depend_on_the_block_size() {
        let e = env(5000, 1500, 2000, FadeCurve::SCurve);
        let whole = left(&mut Automation::new(e.clone(), RATE, false), 0, 5000);
        for block in [1, 7, 33, 64, 1000] {
            let mut a = Automation::new(e.clone(), RATE, false);
            let mut got = Vec::new();
            let mut k = 0;
            while k < 5000 {
                let n = block.min(5000 - k);
                got.extend(left(&mut a, k as i64, n));
                k += n;
            }
            assert_eq!(got, whole, "block {block}");
        }
    }

    #[test]
    fn fades_end_in_silence_and_the_last_frame_is_nearly_so() {
        let mut a = Automation::new(env(1000, 0, 400, FadeCurve::Linear), RATE, false);
        let g = left(&mut a, 0, 1000);
        assert_eq!(g[500], 1.0);
        assert!(g[999] < 0.01, "{}", g[999]);
        assert!(g.windows(2).all(|w| w[1] <= w[0] + 1e-6), "monotonic");
    }

    #[test]
    fn fade_curves_cover_zero_to_one() {
        for curve in [
            FadeCurve::Linear,
            FadeCurve::Log,
            FadeCurve::EqualPower,
            FadeCurve::SCurve,
        ] {
            assert!(curve.gain(0.0).abs() < 1e-12, "{curve:?}");
            assert!((curve.gain(1.0) - 1.0).abs() < 1e-12, "{curve:?}");
            let mut last = 0.0;
            for i in 1..=100 {
                let g = curve.gain(f64::from(i) / 100.0);
                assert!(g >= last, "{curve:?} monotonic");
                last = g;
            }
        }
        assert!((FadeCurve::EqualPower.gain(0.5) - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-12);
    }

    #[test]
    fn overlong_fades_scale_down_to_fit() {
        let f = |frames| FadeFrames {
            frames,
            curve: FadeCurve::Linear,
        };
        let (a, b) = fit_fades(f(600), f(600), 1000);
        assert_eq!((a.frames, b.frames), (500, 500));
        let (a, b) = fit_fades(f(5000), f(0), 1000);
        assert_eq!((a.frames, b.frames), (1000, 0));
        let (a, b) = fit_fades(f(300), f(400), 1000);
        assert_eq!((a.frames, b.frames), (300, 400));
    }

    #[test]
    fn pan_law_is_constant_power_for_mono_and_balance_for_stereo() {
        let [l, r] = pan_gains(0.0, true);
        assert!((20.0 * l.log10() + 3.0103).abs() < 0.01);
        assert!((l - r).abs() < 1e-12);
        let [l, r] = pan_gains(-1.0, true);
        assert!((l - 1.0).abs() < 1e-12 && r.abs() < 1e-12);
        assert_eq!(pan_gains(0.0, false), [1.0, 1.0]);
        let [l, r] = pan_gains(1.0, false);
        assert!(l.abs() < 1e-12 && (r - 1.0).abs() < 1e-12);
        // Constant power across the sweep for mono.
        for i in -10..=10 {
            let [l, r] = pan_gains(f64::from(i) / 10.0, true);
            assert!((l * l + r * r - 1.0).abs() < 1e-12);
        }
    }

    #[test]
    fn a_hold_jump_between_keys_ramps_instead_of_stepping() {
        let t = |s: i64| Time::from_flicks(RATE.samples_to_time(s).unwrap().flicks()).unwrap();
        let keys = Keyframes::new(vec![
            Keyframe::new(t(1000), 0.0, Interp::Hold),
            Keyframe::new(t(1001), -20.0, Interp::Hold),
        ])
        .unwrap();
        let mut e = Envelope::flat(1.0, 0, 3000);
        e.gain = Animatable::Keyframes(keys);
        let mut a = Automation::new(e, RATE, false);
        let g = left(&mut a, 0, 3000);
        assert_eq!(g[500], 1.0);
        assert!((g[2500] - 0.1).abs() < 1e-6);
        let worst = g
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max);
        assert!(worst < 0.02, "largest step {worst}");
    }
}
