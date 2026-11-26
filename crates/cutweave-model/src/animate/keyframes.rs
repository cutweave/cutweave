// SPDX-License-Identifier: GPL-3.0-or-later
//! The keyed form of an animatable value (keyframes §3): a sorted, validated, cheaply cloned
//! list of keys, and the `shift` and `window` operations every edit rule is made of (§8).

use std::{borrow::Cow, sync::Arc};

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer, de,
    ser::{SerializeMap, SerializeStruct},
};
use serde_json::{Map, Value};

use super::{
    error::{KeyframeError, KeyframeRule},
    interp::{self, Interp},
    props::ValueRange,
    value::AnimValue,
};
use crate::{Extra, MAX_SAFE_FLICKS, Time};

/// The most keys one property may have (KF-1).
pub const MAX_KEYFRAMES: usize = 100_000;

/// One key: a value at an item-relative time, and how the segment that starts here moves to
/// the next key (keyframes §3.1).
#[derive(Clone, Debug, PartialEq)]
pub struct Keyframe<T> {
    /// Item-relative flicks: `t = 0` is the item's first frame (§3.2). May lie outside the
    /// item (after a head trim, for example) and may be negative.
    pub t: Time,
    /// The value at `t`.
    pub value: T,
    /// Shapes the segment from this key to the next; kept but unused on the last key.
    pub interp: Interp,
    /// Unknown fields of the key, so newer files round-trip. Boxed: an empty one costs one
    /// pointer per key.
    extra: Option<Box<Extra>>,
}

impl<T> Keyframe<T> {
    /// A key with no unknown fields.
    #[must_use]
    pub fn new(t: Time, value: T, interp: Interp) -> Keyframe<T> {
        Keyframe {
            t,
            value,
            interp,
            extra: None,
        }
    }

    /// Unknown fields of the key, if any.
    #[must_use]
    pub fn extra(&self) -> Option<&Extra> {
        self.extra.as_deref()
    }

    /// The same key at time `t` (fields unknown to this version are kept).
    #[must_use]
    pub fn at(mut self, t: Time) -> Keyframe<T> {
        self.t = t;
        self
    }
}

/// The keyed form of an [`Animatable`](super::Animatable): 1 to 100,000 keys with strictly
/// increasing times (keyframes §3.1, §3.3).
///
/// Always valid: [`Keyframes::new`] and deserialization check KF-1 to KF-7 (KF-4 only for
/// finiteness and type; the property's stored range is [`check_range`](Self::check_range)).
/// The keys are shared, so cloning (as every document snapshot does) is O(1).
#[derive(Clone, Debug, PartialEq)]
pub struct Keyframes<T> {
    keys: Arc<[Keyframe<T>]>,
    extra: Extra,
}

fn at_key(i: usize) -> String {
    format!("/keyframes/{i}")
}

impl<T: AnimValue> Keyframes<T> {
    /// Validated keys: KF-1 (count), KF-2 (strictly increasing times), KF-4 (finite values),
    /// KF-5 and KF-7 (interp parameters and hint), KF-6 (discrete values only hold). KF-3 holds
    /// by construction of [`Time`].
    pub fn new(keys: Vec<Keyframe<T>>) -> Result<Keyframes<T>, KeyframeError> {
        check_count(keys.len())?;
        let mut prev: Option<Time> = None;
        for (i, key) in keys.iter().enumerate() {
            check_order(prev, key.t, i)?;
            prev = Some(key.t);
            check_value(&key.value, ValueRange::ANY, i)?;
            check_discrete::<T>(&key.interp, i)?;
            key.interp
                .validate()
                .map_err(|e| e.under(&format!("{}/interp", at_key(i))))?;
        }
        Ok(Keyframes {
            keys: keys.into(),
            extra: Extra::new(),
        })
    }

    /// Parses and validates the wire form `{"keyframes": [{"t", "value", "interp"}, …]}`,
    /// naming the first broken invariant in the reference evaluator's order (per key: KF-3,
    /// KF-2, KF-4, then the interp's KF-5, KF-6, KF-7). Unknown members of the object and of
    /// each key are kept.
    pub fn from_json(value: &Value) -> Result<Keyframes<T>, KeyframeError> {
        let Some(obj) = value.as_object() else {
            return Err(KeyframeError::rule(
                KeyframeRule::Kf1,
                "",
                "a keyed value is an object with a `keyframes` array",
            ));
        };
        let list = obj
            .get("keyframes")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                KeyframeError::rule(
                    KeyframeRule::Kf1,
                    "/keyframes",
                    "`keyframes` must be an array",
                )
            })?;
        check_count(list.len())?;
        let mut keys = Vec::with_capacity(list.len());
        let mut prev: Option<Time> = None;
        for (i, raw) in list.iter().enumerate() {
            let key = parse_key::<T>(raw, i, prev)?;
            prev = Some(key.t);
            keys.push(key);
        }
        Ok(Keyframes {
            keys: keys.into(),
            extra: extra_of(obj, &["keyframes"]),
        })
    }

    /// Checks every value against a property's stored range (KF-4, §7).
    pub fn check_range(&self, range: ValueRange) -> Result<(), KeyframeError> {
        self.keys
            .iter()
            .enumerate()
            .try_for_each(|(i, key)| check_value(&key.value, range, i))
    }

    /// The same keys with new times or values; keeps the unknown members of the keyed object.
    pub(crate) fn with_keys(&self, keys: Vec<Keyframe<T>>) -> Result<Keyframes<T>, KeyframeError> {
        let mut out = Keyframes::new(keys)?;
        out.extra = self.extra.clone();
        Ok(out)
    }
}

impl<T> Keyframes<T> {
    /// The keys, sorted by strictly increasing `t`; never empty.
    #[must_use]
    pub fn keys(&self) -> &[Keyframe<T>] {
        &self.keys
    }

    /// The number of keys (at least 1).
    #[must_use]
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Always `false`: a keyed value has at least one key (KF-1).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// The first key.
    #[must_use]
    pub fn first(&self) -> &Keyframe<T> {
        // Never empty (KF-1, checked at construction).
        &self.keys[0]
    }

    /// Unknown members of the keyed object.
    #[must_use]
    pub fn extra(&self) -> &Extra {
        &self.extra
    }

    /// The index of the last key with `t_k ≤ t`, or `None` before the first key (§6, step 2:
    /// a binary search).
    #[must_use]
    pub fn key_at_or_before(&self, t: Time) -> Option<usize> {
        self.keys.partition_point(|k| k.t <= t).checked_sub(1)
    }

    /// The index of the key at exactly `t`.
    #[must_use]
    pub fn index_of(&self, t: Time) -> Option<usize> {
        self.keys.binary_search_by(|k| k.t.cmp(&t)).ok()
    }
}

impl<T: Clone> Keyframes<T> {
    /// The same keys (times, curves and unknown fields) with `f` applied to every value. For
    /// maps that keep values valid, such as multiplying by a positive factor; the caller
    /// checks the result against the property's range.
    #[must_use]
    pub fn map_values(&self, f: impl Fn(&T) -> T) -> Keyframes<T> {
        let keys: Vec<Keyframe<T>> = self
            .keys
            .iter()
            .map(|k| {
                let mut key = k.clone();
                key.value = f(&k.value);
                key
            })
            .collect();
        Keyframes {
            keys: keys.into(),
            extra: self.extra.clone(),
        }
    }

    /// `shift(K, δ)` (§8): every `t` becomes `t + δ`, exactly. Fails with
    /// [`KeyframeError::Invalid`] if a result leaves ±(2⁵³ − 1); keys are never dropped.
    pub fn shifted(&self, delta: Time) -> Result<Keyframes<T>, KeyframeError> {
        if delta == Time::ZERO {
            return Ok(self.clone());
        }
        let keys = self
            .keys
            .iter()
            .map(|k| {
                let t = offset(k.t, delta).ok_or_else(|| {
                    KeyframeError::Invalid(format!(
                        "shifting the keyframe at t = {} by {} flicks leaves the time range",
                        k.t.flicks(),
                        delta.flicks()
                    ))
                })?;
                Ok(k.clone().at(t))
            })
            .collect::<Result<Vec<_>, KeyframeError>>()?;
        Ok(Keyframes {
            keys: keys.into(),
            extra: self.extra.clone(),
        })
    }

    /// Every key moved to `f(t)` (WP-2D.01's `scale_times`, keyframes §8.1). `f` must not
    /// decrease; where two keys land on one time, the later one wins.
    #[must_use]
    pub fn retimed(&self, f: impl Fn(Time) -> Time) -> Keyframes<T> {
        let mut keys: Vec<Keyframe<T>> = Vec::with_capacity(self.keys.len());
        for k in self.keys.iter() {
            let t = f(k.t);
            if let Some(last) = keys.last_mut()
                && last.t >= t
            {
                *last = k.clone().at(last.t);
                continue;
            }
            keys.push(k.clone().at(t));
        }
        Keyframes {
            keys: keys.into(),
            extra: self.extra.clone(),
        }
    }

    /// `window(K, a, b)` (§8): the keys with `a ≤ t < b`, plus the last key before `a` and the
    /// first key at or after `b`. Exact: for every `τ` in `[a, b)` it evaluates to the same bits
    /// as `self`. Never empty, since every key is before, inside or after the window.
    #[must_use]
    pub fn window(&self, a: Time, b: Time) -> Keyframes<T> {
        let lo = self.keys.partition_point(|k| k.t < a).saturating_sub(1);
        let inside_end = self.keys.partition_point(|k| k.t < b).max(lo);
        let hi = (inside_end + 1).min(self.keys.len());
        if lo == 0 && hi == self.keys.len() {
            return self.clone();
        }
        Keyframes {
            keys: self.keys[lo..hi].into(),
            extra: self.extra.clone(),
        }
    }
}

/// `t + delta` within ±(2⁵³ − 1).
pub(crate) fn offset(t: Time, delta: Time) -> Option<Time> {
    let sum = i128::from(t.flicks()) + i128::from(delta.flicks());
    i64::try_from(sum)
        .ok()
        .and_then(|t| Time::from_flicks(t).ok())
}

fn check_count(n: usize) -> Result<(), KeyframeError> {
    if (1..=MAX_KEYFRAMES).contains(&n) {
        Ok(())
    } else {
        Err(KeyframeError::rule(
            KeyframeRule::Kf1,
            "/keyframes",
            format!("a keyed value has 1 to {MAX_KEYFRAMES} keyframes, got {n}"),
        ))
    }
}

fn check_order(prev: Option<Time>, t: Time, i: usize) -> Result<(), KeyframeError> {
    match prev {
        Some(prev) if t <= prev => Err(KeyframeError::rule(
            KeyframeRule::Kf2,
            format!("{}/t", at_key(i)),
            format!(
                "keyframe times must strictly increase ({} then {})",
                prev.flicks(),
                t.flicks()
            ),
        )),
        _ => Ok(()),
    }
}

fn check_value<T: AnimValue>(value: &T, range: ValueRange, i: usize) -> Result<(), KeyframeError> {
    if value.fits(range) {
        Ok(())
    } else {
        Err(KeyframeError::rule(
            KeyframeRule::Kf4,
            format!("{}/value", at_key(i)),
            format!(
                "value {value:?} must be finite and within [{}, {}]",
                range.min, range.max
            ),
        ))
    }
}

fn check_discrete<T: AnimValue>(interp: &Interp, i: usize) -> Result<(), KeyframeError> {
    if T::DISCRETE && !interp.is_hold() {
        Err(KeyframeError::rule(
            KeyframeRule::Kf6,
            format!("{}/interp/kind", at_key(i)),
            format!("discrete values only hold, got {}", interp.kind()),
        ))
    } else {
        Ok(())
    }
}

fn parse_key<T: AnimValue>(
    raw: &Value,
    i: usize,
    prev: Option<Time>,
) -> Result<Keyframe<T>, KeyframeError> {
    let path = at_key(i);
    let obj = raw.as_object();
    let field = |name: &str| obj.and_then(|o| o.get(name));
    let t = field("t")
        .and_then(Value::as_i64)
        .filter(|t| t.unsigned_abs() <= MAX_SAFE_FLICKS.unsigned_abs())
        .and_then(|t| Time::from_flicks(t).ok())
        .ok_or_else(|| {
            KeyframeError::rule(
                KeyframeRule::Kf3,
                format!("{path}/t"),
                format!(
                    "t {} is not an integer flick time within ±(2^53 - 1)",
                    field("t").unwrap_or(&Value::Null)
                ),
            )
        })?;
    check_order(prev, t, i)?;
    let value = field("value")
        .and_then(|v| T::deserialize(v).ok())
        .ok_or_else(|| {
            KeyframeError::rule(
                KeyframeRule::Kf4,
                format!("{path}/value"),
                format!(
                    "{} is not a valid value of this property",
                    field("value").unwrap_or(&Value::Null)
                ),
            )
        })?;
    check_value(&value, ValueRange::ANY, i)?;
    let interp = interp::parse(field("interp").unwrap_or(&Value::Null), T::DISCRETE)
        .map_err(|e| e.under(&format!("{path}/interp")))?;
    let extra = obj.map(|o| extra_of(o, &["t", "value", "interp"]));
    Ok(Keyframe {
        t,
        value,
        interp,
        extra: extra.filter(|e| !e.is_empty()).map(Box::new),
    })
}

/// The members of `obj` other than `known`.
fn extra_of(obj: &Map<String, Value>, known: &[&str]) -> Extra {
    let mut extra = Extra::new();
    for (k, v) in obj {
        if !known.contains(&k.as_str()) {
            extra.insert(k.clone(), v.clone());
        }
    }
    extra
}

impl<T: Serialize> Serialize for Keyframe<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let extra = self.extra.as_deref();
        let mut map = s.serialize_map(Some(3 + extra.map_or(0, Extra::len)))?;
        map.serialize_entry("t", &self.t)?;
        map.serialize_entry("value", &self.value)?;
        map.serialize_entry("interp", &self.interp)?;
        for (k, v) in extra.into_iter().flat_map(Extra::iter) {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}

impl<T: Serialize> Serialize for Keyframes<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if self.extra.is_empty() {
            let mut st = s.serialize_struct("Keyframes", 1)?;
            st.serialize_field("keyframes", &*self.keys)?;
            return st.end();
        }
        let mut map = s.serialize_map(Some(1 + self.extra.len()))?;
        map.serialize_entry("keyframes", &*self.keys)?;
        for (k, v) in self.extra.iter() {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}

/// The keyed wire form, strictly (through [`Keyframes::from_json`]): errors name the broken
/// invariant (`"KF-2 at /keyframes/1/t: …"`).
impl<'de, T: AnimValue> Deserialize<'de> for Keyframes<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Keyframes<T>, D::Error> {
        let value = Value::deserialize(d)?;
        Keyframes::from_json(&value).map_err(de::Error::custom)
    }
}

impl<T: JsonSchema> JsonSchema for Keyframe<T> {
    fn schema_name() -> Cow<'static, str> {
        format!("Keyframe_{}", T::schema_name()).into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let value = generator.subschema_for::<T>();
        let interp = generator.subschema_for::<Interp>();
        json_schema!({
            "description": "A key: a value at an item-relative time (flicks), and the easing of the segment to the next key (keyframes §3.1).",
            "type": "object",
            "properties": {
                "t": {
                    "description": "Item-relative time in flicks (1/705600000 s); 0 is the item's first frame.",
                    "type": "integer",
                    "minimum": -MAX_SAFE_FLICKS,
                    "maximum": MAX_SAFE_FLICKS
                },
                "value": value,
                "interp": interp
            },
            "required": ["t", "value", "interp"]
        })
    }
}

impl<T: JsonSchema> JsonSchema for Keyframes<T> {
    fn schema_name() -> Cow<'static, str> {
        format!("Keyframes_{}", T::schema_name()).into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let key = generator.subschema_for::<Keyframe<T>>();
        json_schema!({
            "description": "A keyed (animated) value: keys sorted by strictly increasing t (keyframes §3).",
            "type": "object",
            "properties": {
                "keyframes": {
                    "type": "array",
                    "items": key,
                    "minItems": 1,
                    "maxItems": MAX_KEYFRAMES
                }
            },
            "required": ["keyframes"]
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn kf(t: i64, v: f64) -> Keyframe<f64> {
        Keyframe::new(Time::from_flicks(t).unwrap(), v, Interp::Linear)
    }

    fn times(k: &Keyframes<f64>) -> Vec<i64> {
        k.keys().iter().map(|k| k.t.flicks()).collect()
    }

    #[test]
    fn construction_checks_the_invariants() {
        let code = |keys: Vec<Keyframe<f64>>| Keyframes::new(keys).unwrap_err().code();
        assert_eq!(code(vec![]), "KF-1");
        assert_eq!(code(vec![kf(0, 0.0), kf(0, 1.0)]), "KF-2");
        assert_eq!(code(vec![kf(5, 0.0), kf(1, 1.0)]), "KF-2");
        assert_eq!(code(vec![kf(0, f64::NAN)]), "KF-4");
        let bad = Keyframe::new(
            Time::ZERO,
            0.0,
            Interp::Bezier {
                x1: 2.0,
                y1: 0.0,
                x2: 0.5,
                y2: 1.0,
                preset: None,
            },
        );
        assert_eq!(code(vec![bad]), "KF-5");
        let flip = Keyframes::new(vec![Keyframe::new(Time::ZERO, true, Interp::Linear)]);
        assert_eq!(flip.unwrap_err().code(), "KF-6");
        let k = Keyframes::new(vec![kf(0, 0.0), kf(10, 1.0)]).unwrap();
        assert_eq!(
            k.check_range(ValueRange::new(0.0, 0.5)).unwrap_err().code(),
            "KF-4"
        );
    }

    #[test]
    fn unknown_members_round_trip() {
        let wire = json!({"keyframes": [
            {"t": 0, "value": 0.5, "interp": {"kind": "linear"}, "future": [1]},
            {"t": 10, "value": 1.0, "interp": {"kind": "hold"}}
        ], "lane": "x"});
        let k = Keyframes::<f64>::from_json(&wire).unwrap();
        assert_eq!(k.extra().get("lane"), Some(&json!("x")));
        assert_eq!(
            k.first().extra().and_then(|e| e.get("future")),
            Some(&json!([1]))
        );
        assert!(k.keys()[1].extra().is_none());
        assert_eq!(serde_json::to_value(&k).unwrap(), wire);
        let back: Keyframes<f64> = serde_json::from_value(wire).unwrap();
        assert_eq!(back, k);
    }

    #[test]
    fn deserialize_errors_name_the_rule() {
        let err = serde_json::from_value::<Keyframes<f64>>(json!({"keyframes": [
            {"t": 0, "value": 0.0, "interp": {"kind": "linear"}},
            {"t": 0, "value": 1.0, "interp": {"kind": "linear"}}
        ]}))
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("KF-2") && err.contains("/keyframes/1/t"),
            "{err}"
        );
        let err = Keyframes::<f64>::from_json(&json!({"keyframes": [
            {"t": 0, "value": 0.0, "interp": {"kind": "bezier", "x1": 0, "y1": 0, "x2": 1, "y2": 99}}
        ]}))
        .unwrap_err();
        assert_eq!(
            err,
            KeyframeError::rule(
                KeyframeRule::Kf5,
                "/keyframes/0/interp/y2",
                "bezier y2 must be in [-10, 10], got 99"
            )
        );
        for bad in [
            json!(1.0),
            json!({"keyframes": 3}),
            json!({"keyframes": [7]}),
        ] {
            assert!(Keyframes::<f64>::from_json(&bad).is_err(), "{bad}");
        }
        // `t` must be an integer: an integral float or a string is not one (KF-3).
        for t in [json!(1.0), json!("1s"), json!(null)] {
            let wire = json!({"keyframes": [{"t": t, "value": 0.0, "interp": {"kind": "linear"}}]});
            assert_eq!(
                Keyframes::<f64>::from_json(&wire).unwrap_err().code(),
                "KF-3"
            );
        }
    }

    #[test]
    fn shift_and_window() {
        let k = Keyframes::new(vec![kf(-10, 0.0), kf(0, 1.0), kf(10, 2.0), kf(20, 3.0)]).unwrap();
        assert_eq!(times(&k.window_flicks(0, 10)), vec![-10, 0, 10]);
        assert_eq!(times(&k.window_flicks(1, 10)), vec![0, 10]);
        assert_eq!(times(&k.window_flicks(1, 11)), vec![0, 10, 20]);
        assert_eq!(times(&k.window_flicks(30, 40)), vec![20]);
        assert_eq!(times(&k.window_flicks(-30, -20)), vec![-10]);
        assert_eq!(
            times(&k.window_flicks(5, 5)),
            vec![0, 10],
            "an empty window keeps its bracket"
        );
        assert_eq!(times(&k.window_flicks(-10, 21)), vec![-10, 0, 10, 20]);
        assert_eq!(
            times(&k.shifted(Time::from_flicks(-5).unwrap()).unwrap()),
            vec![-15, -5, 5, 15]
        );
        let edge = Keyframes::new(vec![kf(MAX_SAFE_FLICKS, 0.0)]).unwrap();
        assert_eq!(
            edge.shifted(Time::from_flicks(1).unwrap())
                .unwrap_err()
                .code(),
            "Invalid"
        );
        assert_eq!(k.key_at_or_before(Time::from_flicks(-11).unwrap()), None);
        assert_eq!(k.key_at_or_before(Time::from_flicks(10).unwrap()), Some(2));
        assert_eq!(k.index_of(Time::from_flicks(20).unwrap()), Some(3));
        assert_eq!(k.index_of(Time::from_flicks(21).unwrap()), None);
    }

    impl Keyframes<f64> {
        fn window_flicks(&self, a: i64, b: i64) -> Keyframes<f64> {
            self.window(Time::from_flicks(a).unwrap(), Time::from_flicks(b).unwrap())
        }
    }

    #[test]
    fn schema_names_the_shape() {
        let schema = schemars::schema_for!(Keyframes<f64>);
        let value = serde_json::to_value(&schema).unwrap();
        assert_eq!(value["required"], json!(["keyframes"]));
        assert_eq!(value["properties"]["keyframes"]["minItems"], json!(1));
        let interp = &value["$defs"]["Interp"];
        assert_eq!(
            interp["oneOf"].as_array().map(Vec::len),
            Some(4),
            "{interp}"
        );
    }
}
