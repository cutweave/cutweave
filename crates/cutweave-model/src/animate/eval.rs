// SPDX-License-Identifier: GPL-3.0-or-later
//! Evaluation (keyframes §6): one-off [`Animatable::eval`](super::Animatable::eval), the
//! cursor-caching [`Evaluator`] for playback, and bulk sampling.
//!
//! Both paths locate the key the same way and interpolate through the same [`Segment`], so the
//! evaluator returns exactly the bits of `eval` (WP-2C.02's property test checks it).

use super::{
    Animatable,
    easing::SegmentEasing,
    keyframes::{Keyframe, Keyframes},
    value::AnimValue,
};
use crate::Time;

/// Where `t` falls in a key list.
enum Place {
    /// At a key, before the first key or after the last: the key's value, exactly (§6, step 3).
    Key(usize),
    /// Strictly inside segment `k`: `t_k < t < t_{k+1}`.
    Inside(usize),
}

/// Classifies `t` given `k`, the last key with `t_k ≤ t` (`None` before the first key).
fn place<T>(keys: &[Keyframe<T>], k: Option<usize>, t: Time) -> Place {
    match k {
        None => Place::Key(0),
        Some(k) if keys[k].t == t || k + 1 == keys.len() => Place::Key(k),
        Some(k) => Place::Inside(k),
    }
}

/// Everything about segment `k` that does not depend on `t`: its bounds, its easing and its
/// end values in the evaluated form.
#[derive(Clone, Debug)]
struct Segment<T: AnimValue> {
    index: usize,
    t0: Time,
    t1: Time,
    easing: SegmentEasing,
    v0: T::Eval,
    v1: T::Eval,
}

impl<T: AnimValue> Segment<T> {
    fn new(keys: &[Keyframe<T>], index: usize) -> Segment<T> {
        let (k0, k1) = (&keys[index], &keys[index + 1]);
        // Exact in i64: |t| < 2^53, so the span is below 2^54 (§4.1, step 1).
        let span = k1.t.flicks() - k0.t.flicks();
        let easing = if T::DISCRETE {
            SegmentEasing::new(&super::Interp::Hold, span)
        } else {
            SegmentEasing::new(&k0.interp, span)
        };
        Segment {
            index,
            t0: k0.t,
            t1: k1.t,
            easing,
            v0: k0.value.to_eval(),
            v1: k1.value.to_eval(),
        }
    }

    fn contains(&self, t: Time) -> bool {
        self.t0 < t && t < self.t1
    }

    /// The value at `t`, strictly inside the segment (§6, steps 4 to 6).
    fn value(&self, t: Time) -> T::Eval {
        // Discrete values and hold segments return the start value with no arithmetic, so a
        // hold is exact even where `v1 − v0` would overflow (§4.2).
        if self.easing.is_hold() {
            return self.v0.clone();
        }
        let p = self.easing.progress(t.flicks() - self.t0.flicks());
        T::sanitize(T::combine(&self.v0, &self.v1, p), &self.v0)
    }
}

/// `eval(K, t)` (§6) without a cache.
pub(super) fn eval_keys<T: AnimValue>(keys: &Keyframes<T>, t: Time) -> T::Eval {
    let list = keys.keys();
    match place(list, keys.key_at_or_before(t), t) {
        Place::Key(k) => list[k].value.to_eval(),
        Place::Inside(k) => Segment::new(list, k).value(t),
    }
}

/// Evaluates one animatable at many times, fast when the times mostly increase (playback,
/// audio blocks, curve drawing): it keeps the segment of the last query and its precomputed
/// easing, and moves to the next segment without a search (keyframes §6, WP-2C.02).
///
/// Any order of queries is correct, and every result has exactly the bits of
/// [`Animatable::eval`]. It owns a clone of the keys (O(1): they are shared).
#[derive(Clone, Debug)]
pub struct Evaluator<T: AnimValue> {
    inner: Inner<T>,
}

#[derive(Clone, Debug)]
enum Inner<T: AnimValue> {
    /// A static value, already in the evaluated form.
    Static(T::Eval),
    /// Keys, and the segment of the last query that fell strictly inside one.
    Keyed {
        keys: Keyframes<T>,
        segment: Option<Segment<T>>,
    },
}

impl<T: AnimValue> Evaluator<T> {
    /// An evaluator of `anim`.
    #[must_use]
    pub fn new(anim: Animatable<T>) -> Evaluator<T> {
        let inner = match anim {
            Animatable::Static(v) => Inner::Static(v.to_eval()),
            Animatable::Keyframes(keys) => Inner::Keyed {
                keys,
                segment: None,
            },
        };
        Evaluator { inner }
    }

    /// The value at item time `t`: exactly [`Animatable::eval`]'s.
    pub fn eval(&mut self, t: Time) -> T::Eval {
        let (keys, segment) = match &mut self.inner {
            Inner::Static(v) => return v.clone(),
            Inner::Keyed { keys, segment } => (keys, segment),
        };
        let hint = match segment {
            Some(seg) if seg.contains(t) => return seg.value(t),
            Some(seg) => Some(seg.index),
            None => None,
        };
        let list = keys.keys();
        match place(list, locate(keys, t, hint), t) {
            Place::Key(k) => list[k].value.to_eval(),
            Place::Inside(k) => segment.insert(Segment::new(list, k)).value(t),
        }
    }
}

/// The last key with `t_k ≤ t`, trying the segments at and after `hint` before a binary search.
fn locate<T>(keys: &Keyframes<T>, t: Time, hint: Option<usize>) -> Option<usize> {
    let list = keys.keys();
    if let Some(h) = hint {
        // Playback moves forward: the cached segment or the next one or two.
        for k in h..(h + 3).min(list.len()) {
            let after = list.get(k + 1).is_none_or(|next| t < next.t);
            if list[k].t <= t && after {
                return Some(k);
            }
        }
    }
    keys.key_at_or_before(t)
}

/// `t_i = t0 + ⌊(t1 − t0)·i / (n − 1)⌋` for `i = 0..n` (§6), in `i128` with floor division;
/// `n = 1` samples `t0`, `n = 0` nothing.
pub fn sample_times(t0: Time, t1: Time, n: usize) -> impl Iterator<Item = Time> {
    let start = i128::from(t0.flicks());
    let length = i128::from(t1.flicks()) - start;
    let steps = i128::try_from(n.saturating_sub(1))
        .unwrap_or(i128::MAX)
        .max(1);
    (0..n).map(move |index| {
        let index = i128::try_from(index).unwrap_or(i128::MAX);
        let flicks = start + (length * index).div_euclid(steps);
        // Between t0 and t1, so always a valid time.
        i64::try_from(flicks)
            .ok()
            .and_then(|flicks| Time::from_flicks(flicks).ok())
            .unwrap_or(t0)
    })
}
