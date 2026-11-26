// SPDX-License-Identifier: GPL-3.0-or-later
//! Keyframes and easing: the pure evaluator every animated property uses (WP-2C.02, the normative
//! spec is `docs/spec/keyframes.md`, ADR-0024).
//!
//! - [`Animatable<T>`]: a property that is [`Static`](Animatable::Static) or keyed
//!   ([`Keyframes<T>`], item-relative flick times, §3).
//! - [`Interp`]: how the segment that starts at a key moves to the next key (hold, linear,
//!   CSS-style bezier, physical spring, §4); [`Easing`] names the §4.6 presets.
//! - [`AnimValue`]: how each value type interpolates (§5): `f64` (scalars and accumulating
//!   angles), [`Vec2`], `bool` (holds), [`Color`](crate::Color) (linear light, premultiplied)
//!   and [`ShortestAngle`].
//! - Evaluation (§6): [`Animatable::eval`], the cursor-caching [`Evaluator`] and
//!   [`Animatable::eval_range`], bit-identical to each other; then the property clamp of the
//!   registry ([`PropId`], §7).
//! - Edits (§8, §9): `shift`/`window` and the split and trim rules, and the key-list halves of
//!   `keyframe.set/delete/move/clear`, for WP-2C.03's commands.
//!
//! Everything here is deterministic and wasm32-clean: no I/O, clock or randomness, f64 in IEEE
//! round-to-nearest without fused multiply-add, and `exp`/`sin`/`cos`/`pow` from the pure-Rust
//! `libm` so native and wasm agree bit for bit (§6).
//!
//! **Documents.** Keyed values are legal in documents since WP-2C.03's schema bump (ADR-0024):
//! [`Animatable`]'s `Deserialize` is [`Animatable::from_json`] and its JSON Schema is
//! `oneOf: [T, Keyframes<T>]`. `Project::validate` checks every key against its property's
//! stored range (KF-4).

mod easing;
mod edit;
mod error;
mod eval;
mod interp;
mod keyframes;
mod props;
#[cfg(test)]
mod proptests;
mod value;

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use serde_json::Value;

pub use error::{KeyframeError, KeyframeRule};
pub use eval::{Evaluator, sample_times};
pub use interp::{
    BEZIER_Y_LIMIT, Easing, Interp, PRESET_PATTERN, SPRING_DAMPING, SPRING_MASS, SPRING_STIFFNESS,
    UnknownEasing, is_preset_name,
};
pub use keyframes::{Keyframe, Keyframes, MAX_KEYFRAMES};
pub use props::{PropId, PropSpec, PropValue, UnknownProp, ValueRange, ValueType};
pub use value::{AnimValue, LinearRgba, ShortestAngle};

use crate::Time;

/// A property that may vary over the item's time (arch §4.2, keyframes §3.1).
///
/// On the wire a static value is the bare `T` (`"opacity": 1.0`) and a keyed value is
/// `{"keyframes": [...]}` (§3.1).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Animatable<T> {
    /// The same value at every time.
    Static(T),
    /// Keyed values (item-relative times).
    Keyframes(Keyframes<T>),
}

impl<T> Animatable<T> {
    /// The static value; for a keyed property, its first key's value (what `keyframe.clear`
    /// without a time keeps, §9.7). Use [`eval`](Self::eval) for the value at a time.
    #[must_use]
    pub fn value(&self) -> &T {
        match self {
            Animatable::Static(v) => v,
            Animatable::Keyframes(k) => &k.first().value,
        }
    }

    /// Replaces the property with a static value (dropping any keys).
    pub fn set(&mut self, value: T) {
        *self = Animatable::Static(value);
    }

    /// Whether the value is keyed (shows a keyframe lane, even with a single key).
    #[must_use]
    pub fn is_animated(&self) -> bool {
        matches!(self, Animatable::Keyframes(_))
    }

    /// The keys, if keyed.
    #[must_use]
    pub fn keyframes(&self) -> Option<&Keyframes<T>> {
        match self {
            Animatable::Static(_) => None,
            Animatable::Keyframes(k) => Some(k),
        }
    }

    /// The key times in item time, for drawing lanes (keyframes §11); empty when static. Keys
    /// may lie outside `[0, duration)`.
    pub fn sample_times(&self) -> impl Iterator<Item = Time> + '_ {
        self.keyframes()
            .into_iter()
            .flat_map(|k| k.keys().iter().map(|k| k.t))
    }
}

impl<T: Clone> Animatable<T> {
    /// The property with `f` applied to its static value, or to every key's value (times and
    /// curves are kept). Used to rescale a property when the canvas changes (WP-2B.01).
    #[must_use]
    pub fn map_values(&self, f: impl Fn(&T) -> T) -> Animatable<T> {
        match self {
            Animatable::Static(v) => Animatable::Static(f(v)),
            Animatable::Keyframes(k) => Animatable::Keyframes(k.map_values(f)),
        }
    }
}

impl<T: AnimValue> Animatable<T> {
    /// `eval(animatable, t)` (keyframes §6) at item time `t`, in the evaluated form, without the
    /// property clamp. Pure and deterministic: exact at keys, before the first key and after the
    /// last; never NaN or infinite for valid keys.
    #[must_use]
    pub fn eval(&self, t: Time) -> T::Eval {
        match self {
            Animatable::Static(v) => v.to_eval(),
            Animatable::Keyframes(k) => eval::eval_keys(k, t),
        }
    }

    /// [`eval`](Self::eval) followed by a property's evaluation clamp (§6, step 7; §7).
    #[must_use]
    pub fn eval_clamped(&self, t: Time, clamp: ValueRange) -> T::Eval {
        T::clamp(self.eval(t), clamp)
    }

    /// The value at `t` in the stored form (interpolated; colours re-encoded per §5.4).
    /// Borrowed when it is a stored value (static, at a key, outside the keys, or holding).
    #[must_use]
    pub fn value_at(&self, t: Time) -> Cow<'_, T> {
        let Animatable::Keyframes(keys) = self else {
            return Cow::Borrowed(self.value());
        };
        let list = keys.keys();
        let k = keys.key_at_or_before(t).unwrap_or(0);
        let at_key = list[k].t >= t || k + 1 == list.len();
        if at_key || T::DISCRETE || list[k].interp.is_hold() {
            Cow::Borrowed(&list[k].value)
        } else {
            Cow::Owned(T::from_eval(&eval::eval_keys(keys, t)))
        }
    }

    /// A cursor-caching evaluator of this animatable (for playback and audio blocks).
    #[must_use]
    pub fn evaluator(&self) -> Evaluator<T> {
        Evaluator::new(self.clone())
    }

    /// `n` samples from `t0` to `t1` at the times of [`sample_times`] (§6), each exactly
    /// [`eval`](Self::eval)'s value. For curve drawing, `keyframe.eval` and the wasm
    /// `anim.evalRange`.
    #[must_use]
    pub fn eval_range(&self, t0: Time, t1: Time, n: usize) -> Vec<(Time, T::Eval)> {
        let mut evaluator = self.evaluator();
        sample_times(t0, t1, n)
            .map(|t| (t, evaluator.eval(t)))
            .collect()
    }

    /// Parses the wire form (§3.1): a keyed object (anything with a `keyframes` member) through
    /// [`Keyframes::from_json`], otherwise the bare value. Checks KF-1 to KF-7, with KF-4 for
    /// type and finiteness only (see [`check_range`](Self::check_range)).
    pub fn from_json(value: &Value) -> Result<Animatable<T>, KeyframeError> {
        if value.get("keyframes").is_some() {
            return Keyframes::from_json(value).map(Animatable::Keyframes);
        }
        let v = T::deserialize(value).map_err(|e| {
            KeyframeError::rule(KeyframeRule::Kf4, "", format!("{value} is not valid: {e}"))
        })?;
        Animatable::Static(v).checked(ValueRange::ANY)
    }

    /// Checks the static value or every key against a property's stored range (KF-4, §7).
    pub fn check_range(&self, range: ValueRange) -> Result<(), KeyframeError> {
        match self {
            Animatable::Static(v) if v.fits(range) => Ok(()),
            Animatable::Static(v) => Err(KeyframeError::rule(
                KeyframeRule::Kf4,
                "",
                format!(
                    "value {v:?} must be finite and within [{}, {}]",
                    range.min, range.max
                ),
            )),
            Animatable::Keyframes(k) => k.check_range(range),
        }
    }

    fn checked(self, range: ValueRange) -> Result<Animatable<T>, KeyframeError> {
        self.check_range(range).map(|()| self)
    }
}

impl<T> From<T> for Animatable<T> {
    fn from(value: T) -> Animatable<T> {
        Animatable::Static(value)
    }
}

impl<T: Default> Default for Animatable<T> {
    fn default() -> Animatable<T> {
        Animatable::Static(T::default())
    }
}

/// The bare value, or `{"keyframes": [...]}` (§3.1).
impl<T: Serialize> Serialize for Animatable<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Animatable::Static(v) => v.serialize(s),
            Animatable::Keyframes(k) => k.serialize(s),
        }
    }
}

/// The wire form (§3.1): a bare value, or an object with a `keyframes` member read through
/// [`Animatable::from_json`], so a keyed value is validated (KF-1 to KF-7) as it is parsed. A
/// value's stored range is checked by `Project::validate` (KF-4).
impl<'de, T: AnimValue> Deserialize<'de> for Animatable<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Animatable<T>, D::Error> {
        d.deserialize_any(AnimatableVisitor(std::marker::PhantomData))
    }
}

/// Reads a bare number without building a `Value` (most animatables are static scalars, and
/// documents hold thousands of them); everything else goes through [`Animatable::from_json`].
struct AnimatableVisitor<T>(std::marker::PhantomData<T>);

impl<'de, T: AnimValue> de::Visitor<'de> for AnimatableVisitor<T> {
    type Value = Animatable<T>;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a value or an object with `keyframes`")
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Animatable<T>, E> {
        Animatable::from_json(&Value::Bool(v)).map_err(E::custom)
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Animatable<T>, E> {
        let value = T::deserialize(de::value::F64Deserializer::<E>::new(v))
            .map_err(|e| E::custom(KeyframeError::rule(KeyframeRule::Kf4, "", e.to_string())))?;
        Animatable::Static(value)
            .checked(ValueRange::ANY)
            .map_err(E::custom)
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Animatable<T>, E> {
        Animatable::from_json(&Value::from(v)).map_err(E::custom)
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Animatable<T>, E> {
        Animatable::from_json(&Value::from(v)).map_err(E::custom)
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Animatable<T>, E> {
        Animatable::from_json(&Value::from(v)).map_err(E::custom)
    }

    fn visit_map<A: de::MapAccess<'de>>(self, map: A) -> Result<Animatable<T>, A::Error> {
        let value = Value::deserialize(de::value::MapAccessDeserializer::new(map))?;
        Animatable::from_json(&value).map_err(de::Error::custom)
    }
}

/// `oneOf: [T, Keyframes<T>]` (§3.1): the two never overlap, since a keyed value is an object
/// with a `keyframes` member and no value type has such a field.
impl<T: JsonSchema> JsonSchema for Animatable<T> {
    fn schema_name() -> Cow<'static, str> {
        format!("Animatable_{}", T::schema_name()).into()
    }

    fn schema_id() -> Cow<'static, str> {
        format!("cutweave_model::Animatable<{}>", T::schema_id()).into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let value = generator.subschema_for::<T>();
        let keyed = generator.subschema_for::<Keyframes<T>>();
        json_schema!({
            "description": "A static value, or keyed values over item time (keyframes §3.1).",
            "oneOf": [value, keyed]
        })
    }
}

/// A static value or keyed values (keyframes §3.1); the wire form for TypeScript.
#[derive(Serialize, specta::Type)]
#[serde(rename = "Animatable", untagged)]
#[allow(dead_code)] // describes the wire form to specta; never constructed
enum AnimatableWire<T> {
    Static(T),
    Keyed(KeyedWire<T>),
}

#[derive(Serialize, specta::Type)]
#[serde(rename = "Keyframes")]
#[allow(dead_code)]
struct KeyedWire<T> {
    keyframes: Vec<KeyframeWire<T>>,
}

#[derive(Serialize, specta::Type)]
#[serde(rename = "Keyframe")]
#[allow(dead_code)]
struct KeyframeWire<T> {
    /// Item-relative flicks.
    t: Time,
    value: T,
    interp: Interp,
}

impl<T: specta::Type> specta::Type for Animatable<T> {
    fn definition(types: &mut specta::Types) -> specta::datatype::DataType {
        AnimatableWire::<T>::definition(types)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{Color, FLICKS_PER_SECOND, Vec2};

    const S: i64 = FLICKS_PER_SECOND;

    fn t(flicks: i64) -> Time {
        Time::from_flicks(flicks).unwrap()
    }

    fn anim(keys: &Value) -> Animatable<f64> {
        Animatable::from_json(&json!({ "keyframes": keys })).unwrap()
    }

    #[test]
    fn static_and_keyed_basics() {
        let s = Animatable::from(0.25);
        assert_eq!(s.eval(t(5)), 0.25);
        assert_eq!(*s.value_at(t(5)), 0.25);
        assert_eq!(s.sample_times().count(), 0);
        let a = anim(&json!([
            {"t": 0, "value": 0.0, "interp": {"kind": "linear"}},
            {"t": S, "value": 1.0, "interp": {"kind": "hold"}}
        ]));
        assert!(a.is_animated());
        assert_eq!(*a.value(), 0.0, "the first key's value");
        assert_eq!(a.eval(t(S / 4)), 0.25);
        assert_eq!(*a.value_at(t(S / 4)), 0.25);
        assert!(matches!(a.value_at(t(S)), Cow::Borrowed(_)));
        assert_eq!(a.eval(t(-1)), 0.0);
        assert_eq!(a.eval(t(2 * S)), 1.0);
        assert_eq!(a.sample_times().collect::<Vec<_>>(), vec![t(0), t(S)]);
        let range = a.eval_range(t(0), t(S), 5);
        let values: Vec<f64> = range.iter().map(|(_, v)| *v).collect();
        assert_eq!(values, vec![0.0, 0.25, 0.5, 0.75, 1.0]);
        assert_eq!(a.eval_range(t(0), t(S), 0), vec![]);
    }

    #[test]
    fn sample_times_floor_divide() {
        let ts: Vec<i64> = sample_times(t(0), t(10), 4).map(Time::flicks).collect();
        assert_eq!(ts, vec![0, 3, 6, 10]);
        let ts: Vec<i64> = sample_times(t(5), t(-5), 3).map(Time::flicks).collect();
        assert_eq!(ts, vec![5, 0, -5]);
        let ts: Vec<i64> = sample_times(t(7), t(9), 1).map(Time::flicks).collect();
        assert_eq!(ts, vec![7]);
        let ts: Vec<i64> = sample_times(Time::MIN, Time::MAX, 3)
            .map(Time::flicks)
            .collect();
        assert_eq!(ts, vec![Time::MIN.flicks(), 0, Time::MAX.flicks()]);
    }

    #[test]
    fn wire_forms() {
        let a = anim(&json!([{"t": 7, "value": 0.5, "interp": {"kind": "linear"}}]));
        assert_eq!(
            serde_json::to_value(&a).unwrap(),
            json!({"keyframes": [{"t": 7, "value": 0.5, "interp": {"kind": "linear"}}]})
        );
        assert_eq!(
            serde_json::to_value(Animatable::from(0.5)).unwrap(),
            json!(0.5)
        );
        // Documents read the keyed form, validated.
        let err = serde_json::from_value::<Animatable<f64>>(json!({"keyframes": []}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("KF-1"), "{err}");
        let back: Animatable<f64> =
            serde_json::from_value(serde_json::to_value(&a).unwrap()).unwrap();
        assert_eq!(back, a);
        assert_eq!(
            Animatable::<f64>::from_json(&json!("x"))
                .unwrap_err()
                .code(),
            "KF-4"
        );
        assert_eq!(
            Animatable::<Vec2>::from_json(&json!({"x": 1.0}))
                .unwrap_err()
                .code(),
            "KF-4"
        );
        let c = Animatable::<Color>::from_json(&json!("#ff000080")).unwrap();
        assert_eq!(c.eval(Time::ZERO).a, 128.0 / 255.0);
    }

    #[test]
    fn value_at_reencodes_colours() {
        let a = Animatable::<Color>::from_json(&json!({"keyframes": [
            {"t": 0, "value": "#000000", "interp": {"kind": "linear"}},
            {"t": S, "value": "#ffffff", "interp": {"kind": "linear"}}
        ]}))
        .unwrap();
        // Half way in linear light is 0.5, which sRGB encodes as 188 (0xbc).
        assert_eq!(*a.value_at(t(S / 2)), Color::rgb(0xbc, 0xbc, 0xbc));
    }
}
