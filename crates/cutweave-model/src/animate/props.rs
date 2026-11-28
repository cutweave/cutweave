// SPDX-License-Identifier: GPL-3.0-or-later
//! The animatable property registry of P2 (keyframes §7): every property's ID, value type,
//! document path, stored range, evaluation clamp and default.

use std::{fmt, str::FromStr};

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};

use super::interp::Interp;
use crate::Vec2;

/// A closed interval of numbers, per component; `±∞` ends are unbounded.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ValueRange {
    /// The smallest allowed value (`-∞` when unbounded below).
    pub min: f64,
    /// The largest allowed value (`+∞` when unbounded above).
    pub max: f64,
}

impl ValueRange {
    /// Any finite number.
    pub const ANY: ValueRange = ValueRange {
        min: f64::NEG_INFINITY,
        max: f64::INFINITY,
    };

    /// `[min, max]`.
    #[must_use]
    pub const fn new(min: f64, max: f64) -> ValueRange {
        ValueRange { min, max }
    }

    /// `[min, +∞)`.
    #[must_use]
    pub const fn at_least(min: f64) -> ValueRange {
        ValueRange {
            min,
            max: f64::INFINITY,
        }
    }

    /// Whether `x` is finite and within the range (a valid stored value, KF-4).
    #[must_use]
    pub fn contains(self, x: f64) -> bool {
        x.is_finite() && x >= self.min && x <= self.max
    }

    /// `x` moved into the range (the evaluation clamp, §7). NaN passes through; evaluation never
    /// produces one.
    #[must_use]
    pub fn clamp(self, x: f64) -> f64 {
        if x < self.min {
            self.min
        } else if x > self.max {
            self.max
        } else {
            x
        }
    }
}

/// How a property's values interpolate (keyframes §5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ValueType {
    /// A number (§5.1).
    Scalar,
    /// Two numbers sharing one easing (§5.2).
    Vec2,
    /// Degrees that accumulate: 0 → 720 is two turns (§5.3). Interpolates as a scalar.
    Angle,
    /// Degrees that take the shortest way round (§5.3, [`ShortestAngle`](super::ShortestAngle)).
    ShortestAngle,
    /// An sRGB colour, interpolated in linear light, premultiplied (§5.4).
    Color,
    /// A boolean: holds only (§5.5).
    Bool,
}

impl ValueType {
    /// The type's name in the test vectors (`"scalar"`, `"vec2"`, `"angle"`,
    /// `"angle_shortest"`, `"color"`, `"bool"`).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            ValueType::Scalar => "scalar",
            ValueType::Vec2 => "vec2",
            ValueType::Angle => "angle",
            ValueType::ShortestAngle => "angle_shortest",
            ValueType::Color => "color",
            ValueType::Bool => "bool",
        }
    }

    /// Whether values only hold (KF-6).
    #[must_use]
    pub const fn is_discrete(self) -> bool {
        matches!(self, ValueType::Bool)
    }

    /// The interpolation of a new key with nothing to inherit (§9.2): hold for discrete types,
    /// linear otherwise.
    #[must_use]
    pub const fn default_interp(self) -> Interp {
        Interp::default_for(self.is_discrete())
    }
}

/// A property's default value (§7), used when a property is reset (WP-2C.03).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PropValue {
    /// A number.
    Scalar(f64),
    /// A 2D vector.
    Vec2(Vec2),
    /// A boolean.
    Bool(bool),
}

/// One row of the property registry (keyframes §7).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PropSpec {
    /// The property's ID in commands and outputs.
    pub id: PropId,
    /// Where the value lives, relative to the item (`"visual.transform.position"`).
    pub path: &'static str,
    /// How it interpolates.
    pub value_type: ValueType,
    /// What keys and static values must satisfy (KF-4) and commands validate.
    pub stored: ValueRange,
    /// Applied after interpolation, because easings can overshoot.
    pub clamp: ValueRange,
    /// The value of a reset property.
    pub default: PropValue,
}

/// An animatable property (keyframes §7). The IDs match `item.set_props`'s patch paths, so
/// `keyframe.*`, `item.set_props` and its `changed` output use the same names.
///
/// Later properties (P3 text, P4 effect and composition parameters) register here through
/// their work packages.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, specta::Type,
)]
#[non_exhaustive]
pub enum PropId {
    /// `visual.transform.position`: offset of the anchor from the canvas centre, canvas units.
    #[serde(rename = "transform.position")]
    TransformPosition,
    /// `visual.transform.scale`: multiplier of the aspect-fit size, per axis.
    #[serde(rename = "transform.scale")]
    TransformScale,
    /// `visual.transform.rotation`: degrees, clockwise, accumulating.
    #[serde(rename = "transform.rotation")]
    TransformRotation,
    /// `visual.transform.anchor`: a point of the item in item units.
    #[serde(rename = "transform.anchor")]
    TransformAnchor,
    /// `visual.opacity`: 0 transparent to 1 opaque.
    #[serde(rename = "opacity")]
    Opacity,
    /// `visual.crop.left`: fraction of the upright source removed from the left edge.
    #[serde(rename = "crop.left")]
    CropLeft,
    /// `visual.crop.top`: fraction removed from the top edge.
    #[serde(rename = "crop.top")]
    CropTop,
    /// `visual.crop.right`: fraction removed from the right edge.
    #[serde(rename = "crop.right")]
    CropRight,
    /// `visual.crop.bottom`: fraction removed from the bottom edge.
    #[serde(rename = "crop.bottom")]
    CropBottom,
    /// `visual.flip.horizontal`: mirror left-right (hold only).
    #[serde(rename = "flip.horizontal")]
    FlipHorizontal,
    /// `visual.flip.vertical`: mirror top-bottom (hold only).
    #[serde(rename = "flip.vertical")]
    FlipVertical,
    /// `audio.gain`: clip level in dB, interpolated in dB (UI label "Volume").
    #[serde(rename = "audio_gain")]
    AudioGain,
    /// `audio.pan`: −1 left to 1 right, 0 centre.
    #[serde(rename = "audio_pan")]
    AudioPan,
    /// `style.size` of a text item: font size in reference pixels (WP-3A.01).
    #[serde(rename = "text.size")]
    TextSize,
    /// `style.letter_spacing` of a text item: extra space between letters, em.
    #[serde(rename = "text.letter_spacing")]
    TextLetterSpacing,
    /// `style.line_height` of a text item: a multiple of the font size.
    #[serde(rename = "text.line_height")]
    TextLineHeight,
}

const UNIT: ValueRange = ValueRange::new(0.0, 1.0);

const fn spec(
    id: PropId,
    path: &'static str,
    value_type: ValueType,
    range: ValueRange,
    default: PropValue,
) -> PropSpec {
    // In P2 every property's evaluation clamp equals its stored range (§7).
    PropSpec {
        id,
        path,
        value_type,
        stored: range,
        clamp: range,
        default,
    }
}

/// The registry, in §7's order.
static REGISTRY: [PropSpec; 16] = [
    spec(
        PropId::TransformPosition,
        "visual.transform.position",
        ValueType::Vec2,
        ValueRange::ANY,
        PropValue::Vec2(Vec2::ZERO),
    ),
    spec(
        PropId::TransformScale,
        "visual.transform.scale",
        ValueType::Vec2,
        ValueRange::at_least(0.0),
        PropValue::Vec2(Vec2::ONE),
    ),
    spec(
        PropId::TransformRotation,
        "visual.transform.rotation",
        ValueType::Angle,
        ValueRange::ANY,
        PropValue::Scalar(0.0),
    ),
    spec(
        PropId::TransformAnchor,
        "visual.transform.anchor",
        ValueType::Vec2,
        ValueRange::ANY,
        PropValue::Vec2(Vec2::new(0.5, 0.5)),
    ),
    spec(
        PropId::Opacity,
        "visual.opacity",
        ValueType::Scalar,
        UNIT,
        PropValue::Scalar(1.0),
    ),
    spec(
        PropId::CropLeft,
        "visual.crop.left",
        ValueType::Scalar,
        UNIT,
        PropValue::Scalar(0.0),
    ),
    spec(
        PropId::CropTop,
        "visual.crop.top",
        ValueType::Scalar,
        UNIT,
        PropValue::Scalar(0.0),
    ),
    spec(
        PropId::CropRight,
        "visual.crop.right",
        ValueType::Scalar,
        UNIT,
        PropValue::Scalar(0.0),
    ),
    spec(
        PropId::CropBottom,
        "visual.crop.bottom",
        ValueType::Scalar,
        UNIT,
        PropValue::Scalar(0.0),
    ),
    spec(
        PropId::FlipHorizontal,
        "visual.flip.horizontal",
        ValueType::Bool,
        ValueRange::ANY,
        PropValue::Bool(false),
    ),
    spec(
        PropId::FlipVertical,
        "visual.flip.vertical",
        ValueType::Bool,
        ValueRange::ANY,
        PropValue::Bool(false),
    ),
    spec(
        PropId::AudioGain,
        "audio.gain",
        ValueType::Scalar,
        ValueRange::new(-144.0, 48.0),
        PropValue::Scalar(0.0),
    ),
    spec(
        PropId::AudioPan,
        "audio.pan",
        ValueType::Scalar,
        ValueRange::new(-1.0, 1.0),
        PropValue::Scalar(0.0),
    ),
    spec(
        PropId::TextSize,
        "style.size",
        ValueType::Scalar,
        ValueRange::new(1.0, 4000.0),
        PropValue::Scalar(72.0),
    ),
    spec(
        PropId::TextLetterSpacing,
        "style.letter_spacing",
        ValueType::Scalar,
        ValueRange::new(-1.0, 10.0),
        PropValue::Scalar(0.0),
    ),
    spec(
        PropId::TextLineHeight,
        "style.line_height",
        ValueType::Scalar,
        ValueRange::new(0.1, 10.0),
        PropValue::Scalar(1.2),
    ),
];

impl PropId {
    /// Every property, in §7's order (P2's, then the text properties of WP-3A.01).
    pub const ALL: [PropId; 16] = [
        PropId::TransformPosition,
        PropId::TransformScale,
        PropId::TransformRotation,
        PropId::TransformAnchor,
        PropId::Opacity,
        PropId::CropLeft,
        PropId::CropTop,
        PropId::CropRight,
        PropId::CropBottom,
        PropId::FlipHorizontal,
        PropId::FlipVertical,
        PropId::AudioGain,
        PropId::AudioPan,
        PropId::TextSize,
        PropId::TextLetterSpacing,
        PropId::TextLineHeight,
    ];

    /// Whether the property belongs to a text item's style (not to its visual properties).
    #[must_use]
    pub const fn is_text_style(self) -> bool {
        matches!(
            self,
            PropId::TextSize | PropId::TextLetterSpacing | PropId::TextLineHeight
        )
    }

    /// The property's registry row.
    #[must_use]
    pub fn spec(self) -> &'static PropSpec {
        // `REGISTRY` lists every variant in declaration order (checked by a test).
        &REGISTRY[self as usize]
    }

    /// The property's ID (`"transform.position"`, `"opacity"`, `"audio_gain"`, …).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            PropId::TransformPosition => "transform.position",
            PropId::TransformScale => "transform.scale",
            PropId::TransformRotation => "transform.rotation",
            PropId::TransformAnchor => "transform.anchor",
            PropId::Opacity => "opacity",
            PropId::CropLeft => "crop.left",
            PropId::CropTop => "crop.top",
            PropId::CropRight => "crop.right",
            PropId::CropBottom => "crop.bottom",
            PropId::FlipHorizontal => "flip.horizontal",
            PropId::FlipVertical => "flip.vertical",
            PropId::AudioGain => "audio_gain",
            PropId::AudioPan => "audio_pan",
            PropId::TextSize => "text.size",
            PropId::TextLetterSpacing => "text.letter_spacing",
            PropId::TextLineHeight => "text.line_height",
        }
    }
}

/// One flat `enum` of the IDs (see [`Easing`](super::Easing)'s schema).
impl JsonSchema for PropId {
    fn schema_name() -> Cow<'static, str> {
        "PropId".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        let ids: Vec<&str> = PropId::ALL.iter().map(|p| p.as_str()).collect();
        json_schema!({
            "description": "An animatable property of an item.",
            "type": "string",
            "enum": ids
        })
    }
}

impl fmt::Display for PropId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A string that is not a P2 property ID.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("unknown property `{0}`")]
pub struct UnknownProp(pub String);

impl FromStr for PropId {
    type Err = UnknownProp;
    fn from_str(s: &str) -> Result<PropId, UnknownProp> {
        PropId::ALL
            .into_iter()
            .find(|p| p.as_str() == s)
            .ok_or_else(|| UnknownProp(s.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_rows_match_their_ids() {
        for (i, id) in PropId::ALL.into_iter().enumerate() {
            assert_eq!(id as usize, i);
            assert_eq!(id.spec().id, id);
            assert_eq!(id.as_str().parse::<PropId>(), Ok(id));
            assert_eq!(
                serde_json::to_value(id).unwrap(),
                serde_json::json!(id.as_str())
            );
            let spec = id.spec();
            let default_fits = match spec.default {
                PropValue::Scalar(v) => spec.stored.contains(v),
                PropValue::Vec2(v) => spec.stored.contains(v.x) && spec.stored.contains(v.y),
                PropValue::Bool(_) => spec.value_type == ValueType::Bool,
            };
            assert!(default_fits, "{id}'s default is valid");
        }
        assert_eq!(
            "blend_mode".parse::<PropId>(),
            Err(UnknownProp("blend_mode".into()))
        );
    }

    #[test]
    fn ranges() {
        assert!(UNIT.contains(0.0) && UNIT.contains(1.0) && !UNIT.contains(1.5));
        assert!(!ValueRange::ANY.contains(f64::NAN));
        assert_eq!(UNIT.clamp(-0.25), 0.0);
        assert_eq!(UNIT.clamp(1.25), 1.0);
        assert_eq!(ValueRange::at_least(0.0).clamp(1e300), 1e300);
        assert_eq!(ValueType::Bool.default_interp(), Interp::Hold);
        assert_eq!(ValueType::Vec2.default_interp(), Interp::Linear);
    }
}
