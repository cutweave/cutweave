// SPDX-License-Identifier: GPL-3.0-or-later
//! Interpolation kinds and easing presets (keyframes §3.1, §4, §4.6).

use std::{fmt, str::FromStr};

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::{Map, Value};

use super::{
    easing::SegmentEasing,
    error::{KeyframeError, KeyframeRule},
};

/// JSON Schema pattern of a `preset` hint (KF-7).
pub const PRESET_PATTERN: &str = "^[a-z][a-z0-9_]{0,63}$";

/// Bezier `y` handles are limited to `[−10, 10]` (KF-5).
pub const BEZIER_Y_LIMIT: f64 = 10.0;

/// Spring stiffness range (KF-5), N/m.
pub const SPRING_STIFFNESS: (f64, f64) = (1.0, 10_000.0);
/// Spring damping range (KF-5), N·s/m.
pub const SPRING_DAMPING: (f64, f64) = (0.0, 1_000.0);
/// Spring mass range (KF-5), kg.
pub const SPRING_MASS: (f64, f64) = (0.1, 100.0);

/// Whether `s` matches [`PRESET_PATTERN`].
#[must_use]
pub fn is_preset_name(s: &str) -> bool {
    let bytes = s.as_bytes();
    matches!(bytes.first(), Some(b'a'..=b'z'))
        && bytes.len() <= 64
        && bytes
            .iter()
            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_'))
}

/// How the segment that starts at a key moves to the next key (keyframes §4). It is a closed
/// set: a new kind is a schema change.
///
/// Every kind except `hold` is an easing `p(u)` with `p(0) = 0` and `p(u) → 1` as `u → 1`,
/// fed into one lerp per value type (§5). On the wire it is an object tagged by `kind`
/// (`{"kind": "bezier", "x1": 0.42, "y1": 0, "x2": 0.58, "y2": 1, "preset": "ease_in_out"}`)
/// with exactly its fields; construction checks their ranges (KF-5) and the hint (KF-7).
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Interp {
    /// The key's value until the next key, then a jump (§4.2).
    Hold,
    /// `p = u` (§4.3).
    Linear,
    /// A CSS-style cubic Bézier from (0, 0) to (1, 1), normalized to the segment (§4.4).
    /// (`x1`, `y1`) shapes the speed leaving the key, (`x2`, `y2`) the speed arriving at the
    /// next one.
    Bezier {
        /// First control point, time axis, in `[0, 1]`.
        #[schemars(range(min = 0.0, max = 1.0))]
        #[specta(type = specta_typescript::Number)]
        x1: f64,
        /// First control point, progress axis, in `[−10, 10]` (beyond `[0, 1]` overshoots).
        #[schemars(range(min = -10.0, max = 10.0))]
        #[specta(type = specta_typescript::Number)]
        y1: f64,
        /// Second control point, time axis, in `[0, 1]`.
        #[schemars(range(min = 0.0, max = 1.0))]
        #[specta(type = specta_typescript::Number)]
        x2: f64,
        /// Second control point, progress axis, in `[−10, 10]`.
        #[schemars(range(min = -10.0, max = 10.0))]
        #[specta(type = specta_typescript::Number)]
        y2: f64,
        /// The preset these numbers came from, a hint for the UI and agents (§4.6). Never read
        /// by evaluation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(pattern(PRESET_PATTERN))]
        preset: Option<String>,
    },
    /// A damped spring in real seconds, released from rest, plus a residual that lands it
    /// exactly on the next key (§4.5).
    Spring {
        /// Stiffness, N/m, in `[1, 10000]`.
        #[schemars(range(min = 1.0, max = 10_000.0))]
        #[specta(type = specta_typescript::Number)]
        stiffness: f64,
        /// Damping, N·s/m, in `[0, 1000]` (0 never settles).
        #[schemars(range(min = 0.0, max = 1_000.0))]
        #[specta(type = specta_typescript::Number)]
        damping: f64,
        /// Mass, kg, in `[0.1, 100]`.
        #[schemars(range(min = 0.1, max = 100.0))]
        #[specta(type = specta_typescript::Number)]
        mass: f64,
        /// A hard stop at the target: the motion reflects instead of passing it.
        bounce: bool,
        /// The preset these numbers came from (§4.6). Never read by evaluation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(pattern(PRESET_PATTERN))]
        preset: Option<String>,
    },
}

/// RFC 6901 escaping of one JSON pointer token.
fn pointer_token(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

fn in_range(x: f64, (lo, hi): (f64, f64)) -> bool {
    x.is_finite() && x >= lo && x <= hi
}

impl Interp {
    /// Parses and validates the wire form, naming the broken invariant (KF-5, KF-7).
    pub fn from_json(value: &Value) -> Result<Interp, KeyframeError> {
        parse(value, false)
    }

    /// Checks the parameter ranges (KF-5) and the hint (KF-7) of a constructed value.
    pub fn validate(&self) -> Result<(), KeyframeError> {
        self.check_params()?;
        match self.preset() {
            Some(name) if !is_preset_name(name) => Err(KeyframeError::rule(
                KeyframeRule::Kf7,
                "/preset",
                format!("preset hint `{name}` must match {PRESET_PATTERN}"),
            )),
            _ => Ok(()),
        }
    }

    fn check_params(&self) -> Result<(), KeyframeError> {
        let check = |name: &str, x: f64, range: (f64, f64)| {
            if in_range(x, range) {
                Ok(())
            } else {
                Err(KeyframeError::rule(
                    KeyframeRule::Kf5,
                    format!("/{name}"),
                    format!(
                        "{} {name} must be in [{}, {}], got {x}",
                        self.kind(),
                        range.0,
                        range.1
                    ),
                ))
            }
        };
        let y = (-BEZIER_Y_LIMIT, BEZIER_Y_LIMIT);
        match *self {
            Interp::Hold | Interp::Linear => Ok(()),
            Interp::Bezier { x1, y1, x2, y2, .. } => {
                check("x1", x1, (0.0, 1.0))?;
                check("x2", x2, (0.0, 1.0))?;
                check("y1", y1, y)?;
                check("y2", y2, y)
            }
            Interp::Spring {
                stiffness,
                damping,
                mass,
                ..
            } => {
                check("stiffness", stiffness, SPRING_STIFFNESS)?;
                check("damping", damping, SPRING_DAMPING)?;
                check("mass", mass, SPRING_MASS)
            }
        }
    }

    /// The `kind` tag: `"hold"`, `"linear"`, `"bezier"` or `"spring"`.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Interp::Hold => "hold",
            Interp::Linear => "linear",
            Interp::Bezier { .. } => "bezier",
            Interp::Spring { .. } => "spring",
        }
    }

    /// Whether the segment holds its start value.
    #[must_use]
    pub const fn is_hold(&self) -> bool {
        matches!(self, Interp::Hold)
    }

    /// The preset hint, if any (§4.6).
    #[must_use]
    pub fn preset(&self) -> Option<&str> {
        match self {
            Interp::Bezier { preset, .. } | Interp::Spring { preset, .. } => preset.as_deref(),
            Interp::Hold | Interp::Linear => None,
        }
    }

    /// The same parameters with no hint, as any parameter edit leaves them (§9.6).
    #[must_use]
    pub fn without_preset(mut self) -> Interp {
        if let Interp::Bezier { preset, .. } | Interp::Spring { preset, .. } = &mut self {
            *preset = None;
        }
        self
    }

    /// The preset to show in the UI: the hint's preset while the parameters equal its row of
    /// the §4.6 table exactly, else `None` ("Custom").
    #[must_use]
    pub fn matching_preset(&self) -> Option<Easing> {
        let easing: Easing = self.preset()?.parse().ok()?;
        (easing.is_preset() && easing.interp() == *self).then_some(easing)
    }

    /// The equivalent CSS `cubic-bezier(x1, y1, x2, y2)` handles: `linear` is
    /// `(0, 0, 1, 1)`; `hold` and `spring` have none.
    #[must_use]
    pub fn to_bezier(&self) -> Option<[f64; 4]> {
        match *self {
            Interp::Linear => Some([0.0, 0.0, 1.0, 1.0]),
            Interp::Bezier { x1, y1, x2, y2, .. } => Some([x1, y1, x2, y2]),
            Interp::Hold | Interp::Spring { .. } => None,
        }
    }

    /// The easing progress `p` of a segment of `span` flicks, `dt` flicks after its start
    /// (§4.1): `u = dt / span` as f64, then the kind's `E(u)`. `hold` gives 0.
    ///
    /// The times are raw flicks because a span between two keys can reach 2⁵⁴ − 2, beyond the
    /// range of a `Duration`. Exact at `dt = 0` for every kind; meant for `0 < dt < span`
    /// (outside, the formulas extrapolate). A `span` of zero or less gives 0.
    #[must_use]
    pub fn progress(&self, dt: i64, span: i64) -> f64 {
        SegmentEasing::new(self, span).progress(dt)
    }

    /// The interpolation of a new key with nothing to inherit (§9.2): hold for discrete values,
    /// linear otherwise.
    #[must_use]
    pub const fn default_for(discrete: bool) -> Interp {
        if discrete {
            Interp::Hold
        } else {
            Interp::Linear
        }
    }
}

/// `Interp`'s wire form, strictly (deserializing through [`Interp::from_json`]).
impl<'de> Deserialize<'de> for Interp {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Interp, D::Error> {
        let value = Value::deserialize(d)?;
        Interp::from_json(&value).map_err(de::Error::custom)
    }
}

/// Parses an interp, checking in the reference evaluator's order: the kind and its exact
/// fields (KF-5), discreteness (KF-6), the parameters (KF-5), the hint (KF-7).
pub(crate) fn parse(value: &Value, discrete: bool) -> Result<Interp, KeyframeError> {
    let kf5 = |path: String, msg: String| KeyframeError::rule(KeyframeRule::Kf5, path, msg);
    let Some(obj) = value.as_object() else {
        return Err(kf5(
            String::new(),
            format!("an interpolation is an object with a `kind`, got {value}"),
        ));
    };
    let kind = obj.get("kind").and_then(Value::as_str).unwrap_or_default();
    let fields: &[&str] = match kind {
        "hold" | "linear" => &["kind"],
        "bezier" => &["kind", "x1", "y1", "x2", "y2", "preset"],
        "spring" => &["kind", "stiffness", "damping", "mass", "bounce", "preset"],
        _ => {
            return Err(kf5(
                "/kind".into(),
                format!(
                    "unknown interpolation kind {}; expected hold, linear, bezier or spring",
                    obj.get("kind").unwrap_or(&Value::Null)
                ),
            ));
        }
    };
    if let Some(field) = obj.keys().find(|k| !fields.contains(&k.as_str())) {
        return Err(kf5(
            format!("/{}", pointer_token(field)),
            format!("a {kind} interpolation has no field `{field}`"),
        ));
    }
    if discrete && kind != "hold" {
        return Err(KeyframeError::rule(
            KeyframeRule::Kf6,
            "/kind",
            format!("discrete values only hold, got {kind}"),
        ));
    }
    let interp = match kind {
        "hold" => Interp::Hold,
        "linear" => Interp::Linear,
        "bezier" => Interp::Bezier {
            x1: number(obj, kind, "x1")?,
            y1: number(obj, kind, "y1")?,
            x2: number(obj, kind, "x2")?,
            y2: number(obj, kind, "y2")?,
            preset: None,
        },
        _ => Interp::Spring {
            stiffness: number(obj, kind, "stiffness")?,
            damping: number(obj, kind, "damping")?,
            mass: number(obj, kind, "mass")?,
            bounce: obj
                .get("bounce")
                .and_then(Value::as_bool)
                .ok_or_else(|| kf5("/bounce".into(), "spring bounce must be a boolean".into()))?,
            preset: None,
        },
    };
    interp.check_params()?;
    let preset = match obj.get("preset") {
        None => None,
        Some(Value::String(name)) if is_preset_name(name) => Some(name.clone()),
        Some(other) => {
            return Err(KeyframeError::rule(
                KeyframeRule::Kf7,
                "/preset",
                format!("preset hint {other} must match {PRESET_PATTERN}"),
            ));
        }
    };
    Ok(match interp {
        Interp::Bezier { x1, y1, x2, y2, .. } => Interp::Bezier {
            x1,
            y1,
            x2,
            y2,
            preset,
        },
        Interp::Spring {
            stiffness,
            damping,
            mass,
            bounce,
            ..
        } => Interp::Spring {
            stiffness,
            damping,
            mass,
            bounce,
            preset,
        },
        other => other,
    })
}

/// A required finite number field.
fn number(obj: &Map<String, Value>, kind: &str, field: &str) -> Result<f64, KeyframeError> {
    obj.get(field)
        .and_then(Value::as_f64)
        .filter(|x| x.is_finite())
        .ok_or_else(|| {
            KeyframeError::rule(
                KeyframeRule::Kf5,
                format!("/{field}"),
                format!("{kind} {field} must be a finite number"),
            )
        })
}

/// A named easing (keyframes §4.6): one of the 21 presets, or `linear` or `hold`. Commands
/// accept it instead of an [`Interp`] (`easing: "ease_out"`) and expand it on write
/// ([`Easing::interp`]); documents never store the name alone.
///
/// The table is part of the spec: changing a preset's numbers means a new name, because
/// documents store the expanded numbers and old projects must never change appearance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum Easing {
    /// The key's value until the next key.
    Hold,
    /// Constant speed.
    Linear,
    /// CSS `ease-in`: bezier (0.42, 0, 1, 1).
    EaseIn,
    /// CSS `ease-out`: bezier (0, 0, 0.58, 1).
    EaseOut,
    /// CSS `ease-in-out`: bezier (0.42, 0, 0.58, 1).
    EaseInOut,
    /// Bezier (0.12, 0, 0.39, 0).
    SineIn,
    /// Bezier (0.61, 1, 0.88, 1).
    SineOut,
    /// Bezier (0.37, 0, 0.63, 1).
    SineInOut,
    /// Bezier (0.32, 0, 0.67, 0).
    CubicIn,
    /// Bezier (0.33, 1, 0.68, 1).
    CubicOut,
    /// Bezier (0.65, 0, 0.35, 1).
    CubicInOut,
    /// Bezier (0.7, 0, 0.84, 0).
    ExpoIn,
    /// Bezier (0.16, 1, 0.3, 1).
    ExpoOut,
    /// Bezier (0.87, 0, 0.13, 1).
    ExpoInOut,
    /// Bezier (0.36, 0, 0.66, −0.56): pulls back before leaving.
    BackIn,
    /// Bezier (0.34, 1.56, 0.64, 1): overshoots, then settles.
    BackOut,
    /// Bezier (0.68, −0.6, 0.32, 1.6).
    BackInOut,
    /// Spring 170 / 26 / 1 (ζ ≈ 1.00).
    Spring,
    /// Spring 120 / 14 / 1 (ζ ≈ 0.64).
    SpringGentle,
    /// Spring 180 / 12 / 1 (ζ ≈ 0.45).
    SpringWobbly,
    /// Spring 210 / 20 / 1 (ζ ≈ 0.69).
    SpringStiff,
    /// Spring 200 / 6 / 1 (ζ ≈ 0.21): several decaying oscillations.
    Elastic,
    /// Spring 160 / 10 / 1 with `bounce` (ζ ≈ 0.40): each bounce about a quarter as high.
    Bounce,
}

/// A preset's stored parameters.
enum Params {
    Bezier(f64, f64, f64, f64),
    Spring(f64, f64, f64, bool),
}

impl Easing {
    /// The 21 presets in §4.6's order (the preset picker's order).
    pub const PRESETS: [Easing; 21] = [
        Easing::EaseIn,
        Easing::EaseOut,
        Easing::EaseInOut,
        Easing::SineIn,
        Easing::SineOut,
        Easing::SineInOut,
        Easing::CubicIn,
        Easing::CubicOut,
        Easing::CubicInOut,
        Easing::ExpoIn,
        Easing::ExpoOut,
        Easing::ExpoInOut,
        Easing::BackIn,
        Easing::BackOut,
        Easing::BackInOut,
        Easing::Spring,
        Easing::SpringGentle,
        Easing::SpringWobbly,
        Easing::SpringStiff,
        Easing::Elastic,
        Easing::Bounce,
    ];

    /// Every name a command accepts: `hold`, `linear`, then the presets.
    pub const ALL: [Easing; 23] = {
        let mut all = [Easing::Hold; 23];
        all[1] = Easing::Linear;
        let mut i = 0;
        while i < Easing::PRESETS.len() {
            all[i + 2] = Easing::PRESETS[i];
            i += 1;
        }
        all
    };

    /// The name (`"ease_out"`).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Easing::Hold => "hold",
            Easing::Linear => "linear",
            Easing::EaseIn => "ease_in",
            Easing::EaseOut => "ease_out",
            Easing::EaseInOut => "ease_in_out",
            Easing::SineIn => "sine_in",
            Easing::SineOut => "sine_out",
            Easing::SineInOut => "sine_in_out",
            Easing::CubicIn => "cubic_in",
            Easing::CubicOut => "cubic_out",
            Easing::CubicInOut => "cubic_in_out",
            Easing::ExpoIn => "expo_in",
            Easing::ExpoOut => "expo_out",
            Easing::ExpoInOut => "expo_in_out",
            Easing::BackIn => "back_in",
            Easing::BackOut => "back_out",
            Easing::BackInOut => "back_in_out",
            Easing::Spring => "spring",
            Easing::SpringGentle => "spring_gentle",
            Easing::SpringWobbly => "spring_wobbly",
            Easing::SpringStiff => "spring_stiff",
            Easing::Elastic => "elastic",
            Easing::Bounce => "bounce",
        }
    }

    /// Whether this is one of the 21 presets (not `hold` or `linear`).
    #[must_use]
    pub const fn is_preset(self) -> bool {
        !matches!(self, Easing::Hold | Easing::Linear)
    }

    /// §4.6's numbers.
    const fn params(self) -> Option<Params> {
        use Params::{Bezier, Spring};
        Some(match self {
            Easing::Hold | Easing::Linear => return None,
            Easing::EaseIn => Bezier(0.42, 0.0, 1.0, 1.0),
            Easing::EaseOut => Bezier(0.0, 0.0, 0.58, 1.0),
            Easing::EaseInOut => Bezier(0.42, 0.0, 0.58, 1.0),
            Easing::SineIn => Bezier(0.12, 0.0, 0.39, 0.0),
            Easing::SineOut => Bezier(0.61, 1.0, 0.88, 1.0),
            Easing::SineInOut => Bezier(0.37, 0.0, 0.63, 1.0),
            Easing::CubicIn => Bezier(0.32, 0.0, 0.67, 0.0),
            Easing::CubicOut => Bezier(0.33, 1.0, 0.68, 1.0),
            Easing::CubicInOut => Bezier(0.65, 0.0, 0.35, 1.0),
            Easing::ExpoIn => Bezier(0.7, 0.0, 0.84, 0.0),
            Easing::ExpoOut => Bezier(0.16, 1.0, 0.3, 1.0),
            Easing::ExpoInOut => Bezier(0.87, 0.0, 0.13, 1.0),
            Easing::BackIn => Bezier(0.36, 0.0, 0.66, -0.56),
            Easing::BackOut => Bezier(0.34, 1.56, 0.64, 1.0),
            Easing::BackInOut => Bezier(0.68, -0.6, 0.32, 1.6),
            Easing::Spring => Spring(170.0, 26.0, 1.0, false),
            Easing::SpringGentle => Spring(120.0, 14.0, 1.0, false),
            Easing::SpringWobbly => Spring(180.0, 12.0, 1.0, false),
            Easing::SpringStiff => Spring(210.0, 20.0, 1.0, false),
            Easing::Elastic => Spring(200.0, 6.0, 1.0, false),
            Easing::Bounce => Spring(160.0, 10.0, 1.0, true),
        })
    }

    /// The stored interpolation, expanded on write (§4.6): a preset's parameters with its name
    /// as the hint; `hold` and `linear` without one.
    #[must_use]
    pub fn interp(self) -> Interp {
        let preset = || Some(self.name().to_owned());
        match self.params() {
            None if self == Easing::Hold => Interp::Hold,
            None => Interp::Linear,
            Some(Params::Bezier(x1, y1, x2, y2)) => Interp::Bezier {
                x1,
                y1,
                x2,
                y2,
                preset: preset(),
            },
            Some(Params::Spring(stiffness, damping, mass, bounce)) => Interp::Spring {
                stiffness,
                damping,
                mass,
                bounce,
                preset: preset(),
            },
        }
    }

    /// The stored interpolation of the preset `name` (with its hint), or `None` when `name` is
    /// not one of the 21 presets (`linear` and `hold` are names but not presets).
    #[must_use]
    pub fn preset(name: &str) -> Option<Interp> {
        let easing: Easing = name.parse().ok()?;
        easing.is_preset().then(|| easing.interp())
    }

    /// The CSS `cubic-bezier` handles of this easing, when it has them (`linear` and the
    /// bezier presets).
    #[must_use]
    pub fn to_bezier(self) -> Option<[f64; 4]> {
        self.interp().to_bezier()
    }
}

/// One flat `enum` of the names: a `oneOf` with a description per name would cost an agent's
/// tool list hundreds of tokens (keyframes §10).
impl JsonSchema for Easing {
    fn schema_name() -> Cow<'static, str> {
        "Easing".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        let names: Vec<&str> = Easing::ALL.iter().map(|e| e.name()).collect();
        json_schema!({
            "description": "A preset name (ease_in, ease_out, back_out, spring, bounce, ...), `linear` or `hold`.",
            "type": "string",
            "enum": names
        })
    }
}

impl fmt::Display for Easing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A string that is not an easing name.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("unknown easing `{0}`")]
pub struct UnknownEasing(pub String);

impl FromStr for Easing {
    type Err = UnknownEasing;
    fn from_str(s: &str) -> Result<Easing, UnknownEasing> {
        Easing::ALL
            .into_iter()
            .find(|e| e.name() == s)
            .ok_or_else(|| UnknownEasing(s.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn wire_form_round_trips() {
        for easing in Easing::ALL {
            let interp = easing.interp();
            interp.validate().unwrap();
            let value = serde_json::to_value(&interp).unwrap();
            assert_eq!(value["kind"], interp.kind());
            assert_eq!(Interp::from_json(&value), Ok(interp.clone()));
            let back: Interp = serde_json::from_value(value).unwrap();
            assert_eq!(back, interp);
            assert_eq!(easing.name().parse::<Easing>(), Ok(easing));
            assert_eq!(serde_json::to_value(easing).unwrap(), json!(easing.name()));
        }
        assert_eq!(
            serde_json::to_value(Interp::Linear).unwrap(),
            json!({"kind": "linear"})
        );
    }

    #[test]
    fn strict_fields_and_ranges() {
        let code = |v: Value| Interp::from_json(&v).unwrap_err().code();
        assert_eq!(code(json!({"kind": "hold", "x1": 0})), "KF-5");
        assert_eq!(code(json!({"kind": "linear", "preset": "x"})), "KF-5");
        assert_eq!(
            code(json!({"kind": "bezier", "x1": 0, "y1": 0, "x2": 1})),
            "KF-5"
        );
        assert_eq!(
            code(json!({"kind": "bezier", "x1": true, "y1": 0, "x2": 1, "y2": 1})),
            "KF-5"
        );
        assert_eq!(
            code(
                json!({"kind": "spring", "stiffness": 0.5, "damping": 1, "mass": 1, "bounce": false})
            ),
            "KF-5"
        );
        assert_eq!(
            code(json!({"kind": "spring", "stiffness": 100, "damping": 1, "mass": 1, "bounce": 0})),
            "KF-5"
        );
        assert_eq!(
            code(json!({"kind": "bezier", "x1": 0, "y1": 0, "x2": 1, "y2": 1, "preset": null})),
            "KF-7"
        );
        assert_eq!(
            code(json!({"kind": "bezier", "x1": 0, "y1": 0, "x2": 1, "y2": 1, "preset": "9x"})),
            "KF-7"
        );
        assert_eq!(code(json!("linear")), "KF-5");
        assert_eq!(code(json!({})), "KF-5");
        // Integers are numbers; unknown preset names are allowed (a newer version may add them).
        let interp = Interp::from_json(
            &json!({"kind": "bezier", "x1": 0, "y1": 0, "x2": 1, "y2": 1, "preset": "ease_out_2"}),
        )
        .unwrap();
        assert_eq!(interp.preset(), Some("ease_out_2"));
        assert_eq!(interp.matching_preset(), None);
        let err = serde_json::from_value::<Interp>(json!({"kind": "cubic"})).unwrap_err();
        assert!(err.to_string().contains("KF-5"), "{err}");
    }

    #[test]
    fn preset_names() {
        assert!(is_preset_name("a"));
        assert!(is_preset_name("ease_out_2"));
        assert!(is_preset_name(&"a".repeat(64)));
        assert!(!is_preset_name(&"a".repeat(65)));
        assert!(!is_preset_name(""));
        assert!(!is_preset_name("Ease"));
        assert!(!is_preset_name("_a"));
        assert!(!is_preset_name("a-b"));
    }

    #[test]
    fn matching_preset_needs_the_table_numbers() {
        let mut interp = Easing::EaseOut.interp();
        assert_eq!(interp.matching_preset(), Some(Easing::EaseOut));
        if let Interp::Bezier { x2, .. } = &mut interp {
            *x2 = 0.6;
        }
        assert_eq!(interp.matching_preset(), None, "edited numbers show Custom");
        assert_eq!(
            Easing::EaseOut.interp().without_preset().matching_preset(),
            None
        );
        assert_eq!(Easing::Linear.to_bezier(), Some([0.0, 0.0, 1.0, 1.0]));
        assert_eq!(Easing::Spring.to_bezier(), None);
        assert_eq!(
            Easing::preset("ease_in").unwrap().to_bezier(),
            Some([0.42, 0.0, 1.0, 1.0])
        );
    }
}
