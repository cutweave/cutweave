// SPDX-License-Identifier: GPL-3.0-or-later
//! What splits, cuts and trims do to the keys of an item (keyframes §8.1): the rules, applied
//! to every animated property of a clip. Static properties are never touched.

use cutweave_model::{AnimValue, Animatable, Clip, Duration, KeyframeError, PropId, PropMut, Time};

/// One retiming rule of keyframes §8.1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Retime {
    /// The left half of a split: `window(K, 0, end)` (`end` is the item time of the cut).
    Left { end: Time },
    /// The right half of a split (or the part after a cut): `shift(window(K, from, to), −from)`.
    Right { from: Time, to: Time },
    /// A head trim by `delta` of an item `duration` long (`> 0` trims in, `< 0` extends): keys
    /// shift by `−delta`; with `prune`, then `window(…, 0, duration − delta)`.
    Head {
        delta: Time,
        duration: Duration,
        prune: bool,
    },
    /// A tail trim to `duration`: keys stay; with `prune`, `window(K, 0, duration)`.
    Tail { duration: Duration, prune: bool },
}

impl Retime {
    fn apply<T: AnimValue>(self, a: &Animatable<T>) -> Result<Animatable<T>, KeyframeError> {
        match self {
            Retime::Left { end } => Ok(a.window(Time::ZERO, end)),
            Retime::Right { from, to } => {
                let negated = Time::from_flicks(-from.flicks())
                    .map_err(|e| KeyframeError::Invalid(e.to_string()))?;
                a.window(from, to).shifted(negated)
            }
            Retime::Head {
                delta,
                duration,
                prune,
            } => a.trim_start(delta, duration, prune),
            Retime::Tail { duration, prune } => Ok(a.trim_end(duration, prune)),
        }
    }
}

/// Applies `rule` to every animated property of `clip`. On error the clip may be half changed:
/// callers discard it.
pub(crate) fn retime_clip(clip: &mut Clip, rule: Retime) -> Result<(), KeyframeError> {
    fn one<T: AnimValue>(a: &mut Animatable<T>, rule: Retime) -> Result<(), KeyframeError> {
        if a.is_animated() {
            *a = rule.apply(a)?;
        }
        Ok(())
    }
    for id in PropId::ALL {
        match clip.prop_mut(id) {
            PropMut::Scalar(a) => one(a, rule)?,
            PropMut::Vec2(a) => one(a, rule)?,
            PropMut::Bool(a) => one(a, rule)?,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use cutweave_model::{Interp, Keyframe, Keyframes, MediaId};

    use super::*;

    fn t(f: i64) -> Time {
        Time::from_flicks(f).unwrap()
    }

    fn keyed(times: &[i64]) -> Animatable<f64> {
        #[allow(clippy::cast_precision_loss)]
        let keys = times
            .iter()
            .map(|&f| Keyframe::new(t(f), f as f64 / 100.0, Interp::Linear))
            .collect();
        Animatable::Keyframes(Keyframes::new(keys).unwrap())
    }

    fn times(a: &Animatable<f64>) -> Vec<i64> {
        a.sample_times().map(Time::flicks).collect()
    }

    fn clip() -> Clip {
        let mut c = Clip::new(
            cutweave_model::ItemId::from_u128(1),
            MediaId::from_u128(2),
            Time::ZERO,
            Duration::from_flicks(100).unwrap(),
        );
        c.visual.opacity = keyed(&[0, 40, 80]);
        c
    }

    #[test]
    fn rules_apply_to_animated_properties_only() {
        let mut left = clip();
        retime_clip(&mut left, Retime::Left { end: t(50) }).unwrap();
        assert_eq!(times(&left.visual.opacity), vec![0, 40, 80]);
        let mut right = clip();
        retime_clip(
            &mut right,
            Retime::Right {
                from: t(50),
                to: t(100),
            },
        )
        .unwrap();
        // The key before the cut and the first key after are kept; all shift by -50.
        assert_eq!(times(&right.visual.opacity), vec![-10, 30]);
        assert_eq!(right.visual.crop.left, 0.0.into());
        let mut head = clip();
        retime_clip(
            &mut head,
            Retime::Head {
                delta: t(30),
                duration: Duration::from_flicks(100).unwrap(),
                prune: false,
            },
        )
        .unwrap();
        assert_eq!(times(&head.visual.opacity), vec![-30, 10, 50]);
        retime_clip(
            &mut head,
            Retime::Tail {
                duration: Duration::from_flicks(20).unwrap(),
                prune: true,
            },
        )
        .unwrap();
        assert_eq!(times(&head.visual.opacity), vec![-30, 10, 50]);
    }
}
