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
        missing(keys, ts)?;
        let left: Vec<Keyframe<T>> = keys
            .keys()
            .iter()
            .filter(|k| !ts.contains(&k.t))
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
        missing(keys, ts)?;
        let (moving, staying): (Vec<_>, Vec<_>) =
            keys.keys().iter().cloned().partition(|k| ts.contains(&k.t));
        let mut out = staying;
        for key in moving {
            let to = offset(key.t, delta).ok_or_else(|| {
                KeyframeError::Invalid(format!(
                    "moving the keyframe at t = {} by {} flicks leaves the time range",
                    key.t.flicks(),
                    delta.flicks()
                ))
            })?;
            if out.iter().any(|k| k.t == to) {
                return Err(KeyframeError::Invalid(format!(
                    "a keyframe already exists at t = {}",
                    to.flicks()
                )));
            }
            out.push(key.at(to));
        }
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

/// `NotFound` for the first time in `ts` with no key.
fn missing<T>(keys: &super::Keyframes<T>, ts: &[Time]) -> Result<(), KeyframeError> {
    match ts.iter().find(|t| keys.index_of(**t).is_none()) {
        Some(t) => Err(KeyframeError::NotFound(format!(
            "no keyframe at t = {}",
            t.flicks()
        ))),
        None => Ok(()),
    }
}

/// `−t` (always in range: times are symmetric around zero).
fn negate(t: Time) -> Result<Time, KeyframeError> {
    Time::from_flicks(-t.flicks()).map_err(|e| KeyframeError::Invalid(e.to_string()))
}
