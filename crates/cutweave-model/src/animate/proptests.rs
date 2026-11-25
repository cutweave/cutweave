// SPDX-License-Identifier: GPL-3.0-or-later
//! Property tests of the evaluator (WP-2C.02 acceptance criteria, keyframes §12's self-tests):
//! continuity into every non-hold key, monotone linear segments, holds with no intermediate
//! values, `Evaluator` and `eval_range` equal to `eval` bit for bit, split and head-trim
//! identity, and no NaN or infinity for any valid input, extremes included.

// Bit-exact equality of floats is what these tests assert.
#![allow(clippy::float_cmp)]

use proptest::prelude::*;

use super::{
    AnimValue, Animatable, Easing, Interp, Keyframe, Keyframes, LinearRgba, ShortestAngle,
};
use crate::{Color, Duration, FLICKS_PER_SECOND, MAX_SAFE_FLICKS, Time, Vec2};

const S: i64 = FLICKS_PER_SECOND;

fn time(flicks: i64) -> Time {
    Time::from_flicks(flicks).expect("a safe time")
}

/// Any valid interpolation: every preset, plus random bezier and spring parameters across
/// their whole ranges (KF-5).
fn arb_interp() -> impl Strategy<Value = Interp> {
    prop_oneof![
        Just(Interp::Hold),
        Just(Interp::Linear),
        proptest::sample::select(Easing::PRESETS.to_vec()).prop_map(Easing::interp),
        (0.0..=1.0, -10.0..=10.0, 0.0..=1.0, -10.0..=10.0).prop_map(|(x1, y1, x2, y2)| {
            Interp::Bezier {
                x1,
                y1,
                x2,
                y2,
                preset: None,
            }
        }),
        (1.0..=10_000.0, 0.0..=1_000.0, 0.1..=100.0, any::<bool>()).prop_map(
            |(stiffness, damping, mass, bounce)| Interp::Spring {
                stiffness,
                damping,
                mass,
                bounce,
                preset: None,
            }
        ),
    ]
}

/// Sorted, distinct key times: on a 30 fps grid around the item (as commands write them), or
/// anywhere in the safe range (documents need not be snapped, §3.2).
fn arb_times(max: usize) -> impl Strategy<Value = Vec<i64>> {
    let frame = S / 30;
    prop_oneof![
        proptest::collection::btree_set(-90_i64..180, 1..=max)
            .prop_map(move |s| s.into_iter().map(|f| f * frame).collect()),
        proptest::collection::btree_set(-MAX_SAFE_FLICKS..=MAX_SAFE_FLICKS, 1..=max)
            .prop_map(|s| s.into_iter().collect()),
    ]
}

fn arb_keys<T: AnimValue>(
    values: impl Strategy<Value = T> + Clone,
    max: usize,
) -> impl Strategy<Value = Keyframes<T>> {
    arb_times(max)
        .prop_flat_map(move |times| {
            let n = times.len();
            (
                Just(times),
                proptest::collection::vec(values.clone(), n),
                proptest::collection::vec(arb_interp(), n),
            )
        })
        .prop_map(|(times, values, interps)| {
            let keys = times
                .into_iter()
                .zip(values)
                .zip(interps)
                .map(|((t, v), i)| {
                    let i = if T::DISCRETE { Interp::Hold } else { i };
                    Keyframe::new(time(t), v, i)
                })
                .collect();
            Keyframes::new(keys).expect("generated keys are valid")
        })
}

/// Ordinary values, plus values near the f64 limits (where `v1 − v0` overflows).
fn arb_scalar() -> impl Strategy<Value = f64> + Clone {
    prop_oneof![
        8 => -100.0..100.0,
        1 => -1e308..1e308,
        1 => proptest::sample::select(vec![f64::MAX, -f64::MAX, 0.0, -0.0, f64::MIN_POSITIVE]),
    ]
}

fn arb_vec2() -> impl Strategy<Value = Vec2> + Clone {
    (arb_scalar(), arb_scalar()).prop_map(|(x, y)| Vec2::new(x, y))
}

fn arb_color() -> impl Strategy<Value = Color> + Clone {
    any::<[u8; 4]>().prop_map(|[r, g, b, a]| Color { r, g, b, a })
}

/// A probe time: anywhere, or near a key (one flick either side).
fn probes<T>(keys: &Keyframes<T>, extra: &[i64]) -> Vec<Time> {
    let mut out: Vec<Time> = extra.iter().map(|&t| time(t)).collect();
    for k in keys.keys() {
        let t = k.t.flicks();
        for d in [-1, 0, 1] {
            if let Ok(t) = Time::from_flicks(t.saturating_add(d)) {
                out.push(t);
            }
        }
    }
    out
}

/// Components of an evaluated value, for finiteness and closeness checks.
trait Components {
    /// Whether linear segments are monotone per component (scalars and vectors; colours clamp
    /// each channel to the interpolated alpha, §5.4).
    const MONOTONE: bool = true;
    fn components(&self) -> Vec<f64>;
}

impl Components for f64 {
    fn components(&self) -> Vec<f64> {
        vec![*self]
    }
}

impl Components for Vec2 {
    fn components(&self) -> Vec<f64> {
        vec![self.x, self.y]
    }
}

impl Components for LinearRgba {
    const MONOTONE: bool = false;
    fn components(&self) -> Vec<f64> {
        vec![self.r, self.g, self.b, self.a]
    }
}

impl Components for bool {
    fn components(&self) -> Vec<f64> {
        vec![]
    }
}

/// Whether continuity one flick before the next key is promised within 1e-6 of the segment's
/// spread (§4.1): for segments from a frame to 1,000 s long, and for every kind but hint-less
/// random beziers. A bezier whose end is (nearly) vertical (`x2` → 1, `y2` ≠ 1) reaches the
/// next key only in the limit: its last flick can still cover 1e-4 of the spread.
fn continuity_promised(interp: &Interp, span: i64) -> bool {
    let sane_span = (S / 30..=1_000 * S).contains(&span);
    let shape = !matches!(interp, Interp::Bezier { preset: None, .. });
    sane_span && shape
}

/// The spec's claims for one animatable, probed at `extra` times and around every key.
fn check_all<T: AnimValue>(keys: &Keyframes<T>, extra: &[i64]) -> Result<(), TestCaseError>
where
    T::Eval: Components,
{
    let anim = Animatable::Keyframes(keys.clone());
    let list = keys.keys();
    let ts = probes(keys, extra);

    // No NaN or infinity, anywhere; and the cursor evaluator returns the same bits in any
    // order (forwards, then backwards).
    let mut evaluator = anim.evaluator();
    let mut sorted = ts.clone();
    sorted.sort();
    for &t in sorted.iter().chain(sorted.iter().rev()).chain(ts.iter()) {
        let v = anim.eval(t);
        prop_assert!(
            v.components().iter().all(|c| c.is_finite()),
            "{v:?} at {t:?}"
        );
        prop_assert_eq!(evaluator.eval(t), v, "Evaluator at {:?}", t);
    }

    // Exact at keys and clamped outside them.
    for k in list {
        prop_assert_eq!(anim.eval(k.t), k.value.to_eval());
    }
    if let Ok(before) = Time::from_flicks(list[0].t.flicks() - 1) {
        prop_assert_eq!(anim.eval(before), list[0].value.to_eval());
    }

    for pair in list.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        let span = b.t.flicks() - a.t.flicks();
        let v0 = a.value.to_eval();
        if T::DISCRETE || a.interp.is_hold() {
            // Hold: no intermediate values.
            let mid = time(a.t.flicks() + span / 2);
            prop_assert_eq!(anim.eval(mid), v0.clone());
            prop_assert_eq!(anim.eval(time(b.t.flicks() - 1)), v0);
            continue;
        }
        let v1 = b.value.to_eval();
        let (c0, c1) = (v0.components(), v1.components());
        if c0.iter().chain(&c1).any(|c| c.abs() > 1e300) {
            continue; // overflow territory: only finiteness is promised
        }
        // Continuity one flick before the next key, within 1e-6 of the segment's spread (the
        // reference's self-test bound). Springs move at most ω₀ ≤ 316 distance units per
        // second, so their last flick stays well inside it.
        if continuity_promised(&a.interp, span) {
            let near = anim.eval(time(b.t.flicks() - 1)).components();
            let spread = c0
                .iter()
                .zip(&c1)
                .map(|(x, y)| (y - x).abs())
                .fold(0.0_f64, f64::max);
            let slack = 1e-6 * spread.max(1.0);
            for (n, w) in near.iter().zip(&c1) {
                prop_assert!(
                    (n - w).abs() <= slack,
                    "{n} vs {w} before t = {:?} ({:?})",
                    b.t,
                    a.interp
                );
            }
        }
        // Linear segments are monotone.
        if T::Eval::MONOTONE && matches!(a.interp, Interp::Linear) {
            let samples: Vec<Vec<f64>> = (0..=64)
                .map(|j| {
                    let dt = i64::try_from(i128::from(span) * j / 64).expect("within span");
                    anim.eval(time(a.t.flicks() + dt)).components()
                })
                .collect();
            for c in 0..c0.len() {
                let up = c1[c] >= c0[c];
                for w in samples.windows(2) {
                    let ok = if up {
                        w[0][c] <= w[1][c]
                    } else {
                        w[0][c] >= w[1][c]
                    };
                    prop_assert!(ok, "linear segment not monotone: {:?}", w);
                }
            }
        }
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(384))]

    #[test]
    fn scalar_claims(keys in arb_keys(arb_scalar(), 8), extra in proptest::collection::vec(-3 * S..6 * S, 0..16)) {
        check_all(&keys, &extra)?;
    }

    #[test]
    fn vec2_claims(keys in arb_keys(arb_vec2(), 6), extra in proptest::collection::vec(-3 * S..6 * S, 0..8)) {
        check_all(&keys, &extra)?;
    }

    #[test]
    fn color_claims(keys in arb_keys(arb_color(), 6), extra in proptest::collection::vec(-3 * S..6 * S, 0..8)) {
        check_all(&keys, &extra)?;
    }

    #[test]
    fn bool_claims(keys in arb_keys(any::<bool>(), 6), extra in proptest::collection::vec(-3 * S..6 * S, 0..8)) {
        check_all(&keys, &extra)?;
    }

    #[test]
    fn shortest_angle_is_finite(keys in arb_keys((-720.0..720.0).prop_map(ShortestAngle), 6), t in -3 * S..6 * S) {
        let anim = Animatable::Keyframes(keys);
        prop_assert!(anim.eval(time(t)).is_finite());
    }

    /// `eval_range` and a forward `Evaluator` sweep equal `eval` bit for bit.
    #[test]
    fn bulk_and_cursor_equal_eval(keys in arb_keys(arb_scalar(), 8), t0 in -4 * S..2 * S, len in 0..8 * S, n in 0_usize..200) {
        let anim = Animatable::Keyframes(keys);
        let (t0, t1) = (time(t0), time(t0 + len));
        let samples = anim.eval_range(t0, t1, n);
        prop_assert_eq!(samples.len(), n);
        let times: Vec<Time> = crate::sample_times(t0, t1, n).collect();
        for ((t, v), want_t) in samples.iter().zip(&times) {
            prop_assert_eq!(t, want_t);
            prop_assert!(*t >= t0 && *t <= t1);
            prop_assert_eq!(*v, anim.eval(*t));
        }
        if n >= 2 {
            prop_assert_eq!(samples.first().map(|s| s.0), Some(t0));
            prop_assert_eq!(samples.last().map(|s| s.0), Some(t1));
        }
    }

    /// Splits and head trims keep every rendered value, bit for bit (§8: `window` is exact and
    /// `shift` keeps every `dt` and span).
    #[test]
    fn split_and_trim_identity(
        keys in arb_keys(arb_scalar(), 8),
        dur_frames in 1_i64..240,
        cut in 0.0_f64..1.0,
        delta_frames in -240_i64..240,
        probes in proptest::collection::vec(0.0_f64..1.0, 24),
    ) {
        let frame = S / 30;
        let anim = Animatable::Keyframes(keys);
        let d = dur_frames * frame;
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        let at = ((cut * (d - 1) as f64) as i64).clamp(1, d - 1).max(1);
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        let ts: Vec<i64> = probes.iter().map(|p| (p * d as f64) as i64).chain([0, at - 1, at, d - 1]).collect();
        let duration = Duration::from_flicks(d).unwrap();
        // A shift that would push a key past ±(2^53 - 1) fails (`Invalid`, §8); otherwise the
        // halves agree with the original everywhere.
        if let Ok((left, right)) = anim.split(time(at), duration) {
            for &t in &ts {
                let got = if t < at { left.eval(time(t)) } else { right.eval(time(t - at)) };
                prop_assert_eq!(got.to_bits(), anim.eval(time(t)).to_bits(), "split at {} probe {}", at, t);
            }
        }
        let delta = delta_frames * frame;
        if delta < d {
            for prune in [false, true] {
                let Ok(trimmed) = anim.trim_start(time(delta), duration, prune) else { continue };
                for &t in ts.iter().filter(|&&t| t >= delta) {
                    prop_assert_eq!(trimmed.eval(time(t - delta)).to_bits(), anim.eval(time(t)).to_bits());
                }
            }
        }
        let tail = anim.trim_end(Duration::from_flicks(d / 2 + 1).unwrap(), true);
        for &t in ts.iter().filter(|&&t| t <= d / 2) {
            prop_assert_eq!(tail.eval(time(t)).to_bits(), anim.eval(time(t)).to_bits());
        }
    }

    /// Construction rejects coincident keys (zero-length segments, KF-2).
    #[test]
    fn coincident_keys_are_rejected(t in -S..S, a in arb_scalar(), b in arb_scalar()) {
        let keys = vec![Keyframe::new(time(t), a, Interp::Linear), Keyframe::new(time(t), b, Interp::Linear)];
        prop_assert_eq!(Keyframes::new(keys).unwrap_err().code(), "KF-2");
    }

    /// Easing progress is finite for any valid interp, span and position, including spans up
    /// to 2⁵⁴ − 2 flicks and positions outside the segment.
    #[test]
    fn progress_is_finite(interp in arb_interp(), span in 1_i64..=(2 * MAX_SAFE_FLICKS), u in 0.0_f64..=1.0) {
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        let dt = ((span as f64) * u) as i64;
        let p = interp.progress(dt.clamp(0, span), span);
        prop_assert!(p.is_finite(), "{interp:?} {dt}/{span}: {p}");
        prop_assert_eq!(interp.progress(0, span), 0.0);
    }
}

#[test]
fn bezier_presets_are_monotone_in_time() {
    // Bezier presets with y in [0, 1] never leave [0, 1] (no overshoot).
    for easing in [
        Easing::EaseIn,
        Easing::EaseOut,
        Easing::EaseInOut,
        Easing::SineInOut,
    ] {
        let interp = easing.interp();
        let mut prev = 0.0;
        for j in 0..=1000 {
            let p = interp.progress(j * S / 1000, S);
            assert!((0.0..=1.0 + 1e-12).contains(&p), "{easing}: {p}");
            assert!(p >= prev - 1e-12, "{easing} decreases at {j}");
            prev = p;
        }
    }
}
