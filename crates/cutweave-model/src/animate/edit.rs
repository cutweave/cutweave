// SPDX-License-Identifier: GPL-3.0-or-later
//! Pure edits of an animatable (keyframes §8, §9): what splits and trims do to keys, and the
//! key-list halves of `keyframe.set/delete/move/clear`. WP-2C.03's commands add the rest
//! (resolving items and times, snapping, locks, patches and undo). Static values are never
//! touched by retiming.

use super::{
    Animatable, Interp,
    error::{KeyframeError, KeyframeRule},
    keyframes::{Keyframe, offset},
    props::ValueRange,
    value::AnimValue,
};
use crate::{Duration, Time};

impl<T: AnimValue> Animatable<T> {
    /// `shift(K, δ)` (§8): every key time plus `delta`. Static values are unchanged.
    pub fn shifted(&self, delta: Time) -> Result<Animatable<T>, KeyframeError> {
        match self {
            Animatable::Keyframes(k) => k.shifted(delta).map(Animatable::Keyframes),
            Animatable::Static(_) => Ok(self.clone()),
        }
    }

    /// `window(K, a, b)` (§8): the keys needed to evaluate `[a, b)` exactly.
    #[must_use]
    pub fn window(&self, a: Time, b: Time) -> Animatable<T> {
        match self {
            Animatable::Keyframes(k) => Animatable::Keyframes(k.window(a, b)),
            Animatable::Static(_) => self.clone(),
        }
    }

    /// A split at item time `at` (`0 < at < duration`, §8.1): the left half keeps
    /// `window(K, 0, at)`, the right half `shift(window(K, at, D), −at)`. Both halves evaluate
    /// bit-identically to the original at every timeline position they cover, and nothing is
    /// inserted (§8.3).
    pub fn split(
        &self,
        at: Time,
        duration: Duration,
    ) -> Result<(Animatable<T>, Animatable<T>), KeyframeError> {
        let left = self.window(Time::ZERO, at);
        let right = self.window(at, duration.into()).shifted(negate(at)?)?;
        Ok((left, right))
    }

    /// A head trim by `delta` (`> 0` trims in, `< 0` extends) of an item `duration` long
    /// (§8.1): keys shift by `−delta`, so they stay on the same timeline position. With
    /// `prune`, then `window(…, 0, duration − delta)`.
    pub fn trim_start(
        &self,
        delta: Time,
        duration: Duration,
        prune: bool,
    ) -> Result<Animatable<T>, KeyframeError> {
        let back = negate(delta)?;
        let out = self.shifted(back)?;
        if !prune {
            return Ok(out);
        }
        let end = offset(duration.into(), back).ok_or_else(|| {
            KeyframeError::Invalid("the trimmed item's end leaves the time range".into())
        })?;
        Ok(out.window(Time::ZERO, end))
    }

    /// A tail trim to `new_duration` (§8.1): keys are unchanged; with `prune`,
    /// `window(K, 0, new_duration)`.
    #[must_use]
    pub fn trim_end(&self, new_duration: Duration, prune: bool) -> Animatable<T> {
        if prune {
            self.window(Time::ZERO, new_duration.into())
        } else {
            self.clone()
        }
    }

    /// `keyframe.set` for one key at item time `t`, already snapped (§9.2):
    ///
    /// - `value` omitted keys the current value at `t` after the property clamp (as the
    ///   Inspector's ◇); a static property gives its static value.
    /// - A static property becomes animated with exactly this key.
    /// - A key at an occupied time keeps its interp unless one is given; a new key inherits the
    ///   interp of the key before it, else the default (linear, or hold for discrete values).
    ///
    /// Errors: KF-4 (value outside `stored`), KF-5/KF-6/KF-7 (the interp), KF-1 (more than
    /// 100,000 keys).
    pub fn set_key(
        &self,
        t: Time,
        value: Option<T>,
        interp: Option<Interp>,
        stored: ValueRange,
        clamp: ValueRange,
    ) -> Result<Animatable<T>, KeyframeError> {
        let value = match value {
            Some(v) => v,
            None => T::from_eval(&self.eval_clamped(t, clamp)),
        };
        if !value.fits(stored) {
            return Err(KeyframeError::rule(
                KeyframeRule::Kf4,
                "/value",
                format!(
                    "value {value:?} must be finite and within [{}, {}]",
                    stored.min, stored.max
                ),
            ));
        }
        if let Some(interp) = &interp {
            if T::DISCRETE && !interp.is_hold() {
                return Err(KeyframeError::rule(
                    KeyframeRule::Kf6,
                    "/interp/kind",
                    format!("discrete values only hold, got {}", interp.kind()),
                ));
            }
            interp.validate().map_err(|e| e.under("/interp"))?;
        }
        let default = || Interp::default_for(T::DISCRETE);
        let Animatable::Keyframes(keys) = self else {
            let key = Keyframe::new(t, value, interp.unwrap_or_else(default));
            return super::Keyframes::new(vec![key]).map(Animatable::Keyframes);
        };
        let mut list = keys.keys().to_vec();
        let i = list.partition_point(|k| k.t < t);
        match list.get_mut(i) {
            Some(key) if key.t == t => {
                key.value = value;
                if let Some(interp) = interp {
                    key.interp = interp;
                }
            }
            _ => {
                let inherited = match i.checked_sub(1) {
                    Some(prev) => list[prev].interp.clone(),
                    None => default(),
                };
                list.insert(i, Keyframe::new(t, value, interp.unwrap_or(inherited)));
            }
        }
        keys.with_keys(list).map(Animatable::Keyframes)
    }

    /// `keyframe.delete` (§9.4): removes the keys at `ts` (exact item times). Deleting every key
    /// makes the property static at its first key's value. A missing key, or a static property,
    /// is [`KeyframeError::NotFound`].
    pub fn delete_keys(&self, ts: &[Time]) -> Result<Animatable<T>, KeyframeError> {
        let keys = self.keyed()?;
        let ts = addressed(keys, ts)?;
        let left: Vec<Keyframe<T>> = keys
            .keys()
            .iter()
            .filter(|k| ts.binary_search(&k.t).is_err())
            .cloned()
            .collect();
        if left.is_empty() {
            return Ok(Animatable::Static(keys.first().value.clone()));
        }
        keys.with_keys(left).map(Animatable::Keyframes)
    }

    /// `keyframe.move` by `delta` (§9.5): the keys at `ts` move together, keeping their value
    /// and interp, and may pass their neighbours (the list re-sorts). Landing on a key that does
    /// not move, or leaving ±(2⁵³ − 1), is [`KeyframeError::Invalid`]; a missing key or a static
    /// property is [`KeyframeError::NotFound`].
    pub fn move_keys(&self, ts: &[Time], delta: Time) -> Result<Animatable<T>, KeyframeError> {
        let keys = self.keyed()?;
        let ts = addressed(keys, ts)?;
        // Both halves stay sorted; the moved keys keep distinct times (one shared delta), so a
        // collision can only be with a key that stays.
        let (moving, mut out): (Vec<_>, Vec<_>) = keys
            .keys()
            .iter()
            .cloned()
            .partition(|k| ts.binary_search(&k.t).is_ok());
        let mut moved = Vec::with_capacity(moving.len());
        for key in moving {
            let to = offset(key.t, delta).ok_or_else(|| {
                KeyframeError::Invalid(format!(
                    "moving the keyframe at t = {} by {} flicks leaves the time range",
                    key.t.flicks(),
                    delta.flicks()
                ))
            })?;
            if out.binary_search_by(|k| k.t.cmp(&to)).is_ok() {
                return Err(KeyframeError::Invalid(format!(
                    "a keyframe already exists at t = {}",
                    to.flicks()
                )));
            }
            moved.push(key.at(to));
        }
        out.extend(moved);
        out.sort_by_key(|k| k.t);
        keys.with_keys(out).map(Animatable::Keyframes)
    }

    /// `keyframe.clear` (§9.7): static at the value at `t` (property-clamped), or at the first
    /// key's value without a time. A static property is unchanged.
    #[must_use]
    pub fn cleared(&self, t: Option<Time>, clamp: ValueRange) -> Animatable<T> {
        match (self, t) {
            (Animatable::Static(_), _) => self.clone(),
            (Animatable::Keyframes(k), None) => Animatable::Static(k.first().value.clone()),
            (Animatable::Keyframes(_), Some(t)) => {
                Animatable::Static(T::from_eval(&self.eval_clamped(t, clamp)))
            }
        }
    }

    fn keyed(&self) -> Result<&super::Keyframes<T>, KeyframeError> {
        self.keyframes()
            .ok_or_else(|| KeyframeError::NotFound("the property is not animated".into()))
    }
}

/// The addressed key times, sorted and deduplicated; `NotFound` for the first one with no key.
fn addressed<T>(keys: &super::Keyframes<T>, ts: &[Time]) -> Result<Vec<Time>, KeyframeError> {
    if let Some(t) = ts.iter().find(|t| keys.index_of(**t).is_none()) {
        return Err(KeyframeError::NotFound(format!(
            "no keyframe at t = {}",
            t.flicks()
        )));
    }
    let mut sorted = ts.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    Ok(sorted)
}

/// `−t` (always in range: times are symmetric around zero).
fn negate(t: Time) -> Result<Time, KeyframeError> {
    Time::from_flicks(-t.flicks()).map_err(|e| KeyframeError::Invalid(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MAX_SAFE_FLICKS, animate::Keyframes};

    fn t(flicks: i64) -> Time {
        Time::from_flicks(flicks).unwrap()
    }

    fn anim(times: &[i64]) -> Animatable<f64> {
        #[allow(clippy::cast_precision_loss)]
        let keys = times
            .iter()
            .map(|&f| Keyframe::new(t(f), f as f64, Interp::Linear))
            .collect();
        Animatable::Keyframes(Keyframes::new(keys).unwrap())
    }

    fn times(a: &Animatable<f64>) -> Vec<i64> {
        a.sample_times().map(Time::flicks).collect()
    }

    const ANY: ValueRange = ValueRange::ANY;

    #[test]
    fn moved_keys_may_land_where_another_moved_key_was() {
        let a = anim(&[0, 10, 30]);
        let moved = a.move_keys(&[t(0), t(10), t(0)], t(10)).unwrap();
        assert_eq!(times(&moved), vec![10, 20, 30]);
        assert_eq!(
            moved.eval(t(10)).to_bits(),
            0.0_f64.to_bits(),
            "the key from 0 keeps its value"
        );
        let onto = a.move_keys(&[t(10)], t(20)).unwrap_err();
        assert_eq!(onto.code(), "Invalid", "landing on a key that stays");
        let edge = anim(&[MAX_SAFE_FLICKS]);
        let out = edge.move_keys(&[t(MAX_SAFE_FLICKS)], t(1)).unwrap_err();
        assert_eq!(out.code(), "Invalid");
        assert_eq!(a.move_keys(&[t(5)], t(1)).unwrap_err().code(), "NotFound");
        let stat = Animatable::from(1.0);
        assert_eq!(
            stat.move_keys(&[t(0)], t(1)).unwrap_err().code(),
            "NotFound"
        );
    }

    #[test]
    fn delete_and_clear() {
        let a = anim(&[0, 10, 20]);
        assert_eq!(times(&a.delete_keys(&[t(10), t(10)]).unwrap()), vec![0, 20]);
        let all = a.delete_keys(&[t(0), t(10), t(20)]).unwrap();
        assert_eq!(all, Animatable::from(0.0));
        assert_eq!(a.cleared(Some(t(5)), ANY), Animatable::from(5.0));
        let unit = ValueRange::new(0.0, 1.0);
        assert_eq!(a.cleared(Some(t(5)), unit), Animatable::from(1.0));
        assert_eq!(a.cleared(None, ANY), Animatable::from(0.0));
        let stat = Animatable::from(3.0);
        assert_eq!(stat.cleared(Some(t(1)), ANY), stat);
    }

    #[test]
    fn set_key_rules() {
        let stat = Animatable::from(0.5);
        let one = stat.set_key(t(7), None, None, ANY, ANY).unwrap();
        let first = one.keyframes().map(|k| k.first().clone());
        assert_eq!(
            first,
            Some(Keyframe::new(t(7), 0.5, Interp::Linear)),
            "the static value"
        );
        let flip = Animatable::from(false);
        let held = flip.set_key(t(0), Some(true), None, ANY, ANY).unwrap();
        let interp = held.keyframes().map(|k| k.first().interp.clone());
        assert_eq!(interp, Some(Interp::Hold));
        let bad = Interp::Bezier {
            x1: 0.0,
            y1: 0.0,
            x2: 1.0,
            y2: 1.0,
            preset: Some("No".into()),
        };
        let err = stat.set_key(t(0), Some(0.1), Some(bad), ANY, ANY);
        assert_eq!(err.unwrap_err().code(), "KF-7");
        let err = stat.set_key(t(0), Some(f64::NAN), None, ANY, ANY);
        assert_eq!(err.unwrap_err().code(), "KF-4");
    }

    #[test]
    fn retime_keeps_static_values() {
        let s = Animatable::from(0.25);
        let d = Duration::from_flicks(100).unwrap();
        assert_eq!(s.split(t(50), d).unwrap(), (s.clone(), s.clone()));
        assert_eq!(s.trim_start(t(10), d, true).unwrap(), s);
        assert_eq!(s.trim_end(d, true), s);
        let a = anim(&[-50, 0, 50, 150]);
        let extended = a.trim_start(t(-20), d, true).unwrap();
        assert_eq!(times(&extended), vec![-30, 20, 70, 170]);
        let trimmed = a.trim_start(t(60), d, true).unwrap();
        assert_eq!(times(&trimmed), vec![-10, 90]);
    }
}
