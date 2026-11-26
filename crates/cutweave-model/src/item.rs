// SPDX-License-Identifier: GPL-3.0-or-later
//! Timeline items (arch §4.2), restricted to schema v2: media clips with audio gain and pan,
//! transform, opacity, crop, flip and blend mode. Text, captions, compositions, generators,
//! adjustments, effects and masks arrive in later versions. Animatable properties may be keyed
//! (WP-2C.03, [`Animatable`] lives in `animate`).

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};

use crate::{
    Animatable, Custom, Duration, Extra, Freeze, GroupId, ItemId, Markers, MediaId, PropId, Speed,
    Time, TimeError,
};

/// A 2D vector (positions, scales, anchors).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type,
)]
#[serde(deny_unknown_fields)]
pub struct Vec2 {
    /// Horizontal component.
    #[specta(type = specta_typescript::Number)]
    pub x: f64,
    /// Vertical component (down is positive).
    #[specta(type = specta_typescript::Number)]
    pub y: f64,
}

impl Vec2 {
    /// `(0, 0)`.
    pub const ZERO: Vec2 = Vec2 { x: 0.0, y: 0.0 };
    /// `(1, 1)`.
    pub const ONE: Vec2 = Vec2 { x: 1.0, y: 1.0 };

    /// `(x, y)`.
    #[must_use]
    pub const fn new(x: f64, y: f64) -> Vec2 {
        Vec2 { x, y }
    }

    /// Whether both components are finite.
    #[must_use]
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

/// Where and how a visual item is placed on the sequence canvas.
///
/// Coordinates are resolution-independent: `position` is the offset of the item's anchor from
/// the canvas center in canvas units (1.0 = the canvas width horizontally and the canvas height
/// vertically; +y is down); `anchor` is a point of the item in item units ((0, 0) top-left,
/// (0.5, 0.5) center); `scale` multiplies the aspect-fit size; `rotation` is in degrees,
/// clockwise.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
pub struct Transform {
    /// Offset of the anchor from the canvas center, in canvas units.
    pub position: Animatable<Vec2>,
    /// Size multiplier per axis (1.0 = aspect-fit).
    pub scale: Animatable<Vec2>,
    /// Clockwise rotation in degrees.
    #[specta(type = Animatable<specta_typescript::Number>)]
    pub rotation: Animatable<f64>,
    /// The item point that `position` places and rotation turns around, in item units.
    pub anchor: Animatable<Vec2>,
    /// Fields unknown to this version.
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: Extra,
}

/// The identity: centered, unscaled, unrotated.
impl Default for Transform {
    fn default() -> Transform {
        Transform {
            position: Vec2::ZERO.into(),
            scale: Vec2::ONE.into(),
            rotation: 0.0.into(),
            anchor: Vec2::new(0.5, 0.5).into(),
            extra: Extra::new(),
        }
    }
}

/// How much of each edge of the upright source picture is cut away, as fractions of the
/// source frame (`0.0..=1.0` each; keyframes §7). Each edge animates on its own, so each is
/// validated on its own: when `left + right >= 1` or `top + bottom >= 1` nothing is visible.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
pub struct Crop {
    /// Fraction removed from the left edge.
    #[specta(type = Animatable<specta_typescript::Number>)]
    pub left: Animatable<f64>,
    /// Fraction removed from the top edge.
    #[specta(type = Animatable<specta_typescript::Number>)]
    pub top: Animatable<f64>,
    /// Fraction removed from the right edge.
    #[specta(type = Animatable<specta_typescript::Number>)]
    pub right: Animatable<f64>,
    /// Fraction removed from the bottom edge.
    #[specta(type = Animatable<specta_typescript::Number>)]
    pub bottom: Animatable<f64>,
    /// Fields unknown to this version.
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: Extra,
}

/// No edge cropped.
impl Default for Crop {
    fn default() -> Crop {
        Crop {
            left: 0.0.into(),
            top: 0.0.into(),
            right: 0.0.into(),
            bottom: 0.0.into(),
            extra: Extra::new(),
        }
    }
}

/// Mirroring of an item about its centre, after the crop (keyframes §7). Hold-only: a keyed
/// flip switches at its keys.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
pub struct Flip {
    /// Mirror left-right.
    pub horizontal: Animatable<bool>,
    /// Mirror top-bottom.
    pub vertical: Animatable<bool>,
    /// Fields unknown to this version.
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: Extra,
}

/// Not mirrored.
impl Default for Flip {
    fn default() -> Flip {
        Flip {
            horizontal: false.into(),
            vertical: false.into(),
            extra: Extra::new(),
        }
    }
}

/// How an item's picture combines with what is below it (arch §8's core set). Not animatable.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    specta::Type,
)]
#[serde(rename_all = "snake_case")]
pub enum BlendMode {
    /// The picture covers what is below, by its opacity.
    #[default]
    Normal,
    /// Darkens: the product of both.
    Multiply,
    /// Lightens: the inverse of the product of the inverses.
    Screen,
    /// Multiply in the shadows, screen in the highlights, by what is below.
    Overlay,
    /// The darker of both, per channel.
    Darken,
    /// The lighter of both, per channel.
    Lighten,
    /// The sum of both, clipped.
    Add,
    /// A gentle overlay.
    SoftLight,
}

/// One flat `enum` of the names (a tool list pays for every description).
impl JsonSchema for BlendMode {
    fn schema_name() -> Cow<'static, str> {
        "BlendMode".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "How an item combines with what is below it (arch §8's core set).",
            "type": "string",
            "enum": ["normal", "multiply", "screen", "overlay", "darken", "lighten", "add", "soft_light"]
        })
    }
}

/// Visual properties of an item (arch §4.2 `VisualProps`; keyframes §7).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
pub struct VisualProps {
    /// Placement on the canvas.
    pub transform: Transform,
    /// Opacity, 0.0 (transparent) to 1.0 (opaque).
    #[specta(type = Animatable<specta_typescript::Number>)]
    pub opacity: Animatable<f64>,
    /// Edges of the source picture cut away.
    pub crop: Crop,
    /// Mirroring.
    pub flip: Flip,
    /// How the item combines with what is below.
    pub blend_mode: BlendMode,
    /// Fields unknown to this version.
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: Extra,
}

/// Identity transform, fully opaque.
impl Default for VisualProps {
    fn default() -> VisualProps {
        VisualProps {
            transform: Transform::default(),
            opacity: 1.0.into(),
            crop: Crop::default(),
            flip: Flip::default(),
            blend_mode: BlendMode::Normal,
            extra: Extra::new(),
        }
    }
}

/// Audio properties of an item (arch §4.2 `audio{…}`, v1 subset).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
pub struct AudioProps {
    /// Gain in decibels (0.0 = unchanged).
    #[specta(type = Animatable<specta_typescript::Number>)]
    pub gain: Animatable<f64>,
    /// Stereo position, -1.0 (left) to 1.0 (right), 0.0 = centre.
    #[specta(type = Animatable<specta_typescript::Number>)]
    pub pan: Animatable<f64>,
    /// Fade from silence at the start of the clip (WP-2E.01). Absent in documents written
    /// before it: no fade.
    #[serde(default)]
    pub fade_in: Fade,
    /// Fade to silence at the end of the clip (WP-2E.01). Absent in older documents: no fade.
    #[serde(default)]
    pub fade_out: Fade,
    /// Fields unknown to this version.
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: Extra,
}

/// The shape of a fade: how the gain goes from 0 to 1 over the fade's duration (a fade out runs
/// it backwards).
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    JsonSchema,
    specta::Type,
)]
#[serde(rename_all = "snake_case")]
pub enum FadeCurve {
    /// Gain rises in a straight line (the default).
    #[default]
    Linear,
    /// Gain rises quickly then flattens (`log10(1 + 9x)`): a fast, natural-sounding fade in.
    Log,
    /// Constant power with a matching fade of the same shape on the other clip: `sin(x*pi/2)`.
    EqualPower,
    /// Gentle at both ends: `(1 - cos(x*pi)) / 2`.
    SCurve,
}

impl FadeCurve {
    /// The gain `0..=1` of a fade in at progress `x` (`0` start, `1` end); a fade out uses
    /// `gain(1 - x)`. Pure, so preview and export agree.
    #[must_use]
    pub fn gain(self, x: f64) -> f64 {
        let x = x.clamp(0.0, 1.0);
        match self {
            FadeCurve::Linear => x,
            FadeCurve::Log => (1.0 + 9.0 * x).log10(),
            FadeCurve::EqualPower => (x * std::f64::consts::FRAC_PI_2).sin(),
            FadeCurve::SCurve => (1.0 - (x * std::f64::consts::PI).cos()) / 2.0,
        }
    }
}

/// A fade at one end of a clip. A zero duration is no fade. Fades are measured on the clip as
/// placed (after trimming), so trimming keeps their length unless the clip becomes shorter than
/// both fades together, when they are scaled down at playback (the stored values stay).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, specta::Type,
)]
pub struct Fade {
    /// Length of the fade.
    pub duration: Duration,
    /// Its shape.
    pub curve: FadeCurve,
}

/// A media clip: a range of a `MediaAsset` placed on a track.
///
/// The clip occupies `[start, start + duration)` on the timeline and plays the source range
/// `[source_in, source_in + duration × speed)`, backwards when `reverse`, or one frame of it for
/// its whole length when it is a `freeze` frame (WP-2D.01; the mapping is `SourceMap`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
pub struct Clip {
    /// The item ID (also its key in the track's `items`).
    pub id: ItemId,
    /// Display name; `None` shows the media name.
    pub name: Option<String>,
    /// Timeline position of the first frame.
    pub start: Time,
    /// Length on the timeline (positive).
    pub duration: Duration,
    /// Disabled items are kept but neither rendered nor heard.
    pub enabled: bool,
    /// Items with the same group move, trim and delete together (e.g. linked video and audio).
    pub linked_group: Option<GroupId>,
    /// The item this one follows when it moves (schema v2, timeline spec §1.4): an overlay
    /// attached to a main-track clip. `None` when free.
    pub attached_to: Option<ItemId>,
    /// Markers in item-relative time; they move with the item.
    #[schemars(with = "std::collections::BTreeMap<crate::MarkerId, crate::Marker>")]
    #[specta(type = std::collections::BTreeMap<crate::MarkerId, crate::Marker>)]
    pub markers: Markers,
    /// The media this clip plays.
    pub media: MediaId,
    /// Source position (from the start of the media) of the first frame (of the last frame
    /// shown, when reversed).
    pub source_in: Time,
    /// Playback speed (WP-2D.01): a timeline span `d` plays `d × speed` of media (INV-47).
    /// Absent in documents written before it (the v2 fill-in adds it): normal speed.
    #[serde(default)]
    pub speed: Speed,
    /// Plays its source range backwards (WP-2D.01). Absent in older documents: forwards.
    #[serde(default)]
    pub reverse: bool,
    /// A freeze frame (WP-2D.01): shows the frame at `at_source` for its whole length, silent.
    /// Absent in older documents: not a freeze frame.
    #[serde(default)]
    pub freeze: Option<Freeze>,
    /// Audio properties.
    pub audio: AudioProps,
    /// Visual properties.
    pub visual: VisualProps,
    /// Plugin data (arch §4.3).
    pub custom: Custom,
    /// Fields unknown to this version.
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: Extra,
}

impl Clip {
    /// An enabled clip of `media` at `start` for `duration`, from the start of the media, with
    /// default audio and visual properties.
    #[must_use]
    pub fn new(id: ItemId, media: MediaId, start: Time, duration: Duration) -> Clip {
        Clip {
            id,
            name: None,
            start,
            duration,
            enabled: true,
            linked_group: None,
            attached_to: None,
            markers: Markers::new(),
            media,
            source_in: Time::ZERO,
            speed: Speed::ONE,
            reverse: false,
            freeze: None,
            audio: AudioProps::default(),
            visual: VisualProps::default(),
            custom: Custom::new(),
            extra: Extra::new(),
        }
    }
}

/// An animatable property of a clip by registry ID, typed by its value type (see
/// [`Clip::prop`]).
#[derive(Clone, Copy, Debug)]
pub enum PropRef<'a> {
    /// A number (opacity, crop edges, rotation, gain, pan).
    Scalar(&'a Animatable<f64>),
    /// A 2D vector (position, scale, anchor).
    Vec2(&'a Animatable<Vec2>),
    /// A boolean (flip).
    Bool(&'a Animatable<bool>),
}

/// The mutable twin of [`PropRef`] (see [`Clip::prop_mut`]).
#[derive(Debug)]
pub enum PropMut<'a> {
    /// A number.
    Scalar(&'a mut Animatable<f64>),
    /// A 2D vector.
    Vec2(&'a mut Animatable<Vec2>),
    /// A boolean.
    Bool(&'a mut Animatable<bool>),
}

impl Clip {
    /// The animatable property with registry ID `id` (keyframes §7).
    #[must_use]
    pub fn prop(&self, id: PropId) -> PropRef<'_> {
        let v = &self.visual;
        match id {
            PropId::TransformPosition => PropRef::Vec2(&v.transform.position),
            PropId::TransformScale => PropRef::Vec2(&v.transform.scale),
            PropId::TransformRotation => PropRef::Scalar(&v.transform.rotation),
            PropId::TransformAnchor => PropRef::Vec2(&v.transform.anchor),
            PropId::Opacity => PropRef::Scalar(&v.opacity),
            PropId::CropLeft => PropRef::Scalar(&v.crop.left),
            PropId::CropTop => PropRef::Scalar(&v.crop.top),
            PropId::CropRight => PropRef::Scalar(&v.crop.right),
            PropId::CropBottom => PropRef::Scalar(&v.crop.bottom),
            PropId::FlipHorizontal => PropRef::Bool(&v.flip.horizontal),
            PropId::FlipVertical => PropRef::Bool(&v.flip.vertical),
            PropId::AudioGain => PropRef::Scalar(&self.audio.gain),
            PropId::AudioPan => PropRef::Scalar(&self.audio.pan),
        }
    }

    /// The mutable animatable property with registry ID `id`.
    pub fn prop_mut(&mut self, id: PropId) -> PropMut<'_> {
        let v = &mut self.visual;
        match id {
            PropId::TransformPosition => PropMut::Vec2(&mut v.transform.position),
            PropId::TransformScale => PropMut::Vec2(&mut v.transform.scale),
            PropId::TransformRotation => PropMut::Scalar(&mut v.transform.rotation),
            PropId::TransformAnchor => PropMut::Vec2(&mut v.transform.anchor),
            PropId::Opacity => PropMut::Scalar(&mut v.opacity),
            PropId::CropLeft => PropMut::Scalar(&mut v.crop.left),
            PropId::CropTop => PropMut::Scalar(&mut v.crop.top),
            PropId::CropRight => PropMut::Scalar(&mut v.crop.right),
            PropId::CropBottom => PropMut::Scalar(&mut v.crop.bottom),
            PropId::FlipHorizontal => PropMut::Bool(&mut v.flip.horizontal),
            PropId::FlipVertical => PropMut::Bool(&mut v.flip.vertical),
            PropId::AudioGain => PropMut::Scalar(&mut self.audio.gain),
            PropId::AudioPan => PropMut::Scalar(&mut self.audio.pan),
        }
    }
}

/// Anything placed on a track (arch §4.2). Schema v1 has only [`Clip`]; matching code outside
/// this crate needs a wildcard arm for the kinds still to come.
///
/// Serialized as the variant's object plus a `"kind"` tag (`{"kind": "clip", …}`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Item {
    /// A media clip.
    Clip(Clip),
}

impl Item {
    /// The item ID.
    #[must_use]
    pub fn id(&self) -> ItemId {
        match self {
            Item::Clip(c) => c.id,
        }
    }

    /// Timeline position of the first frame.
    #[must_use]
    pub fn start(&self) -> Time {
        match self {
            Item::Clip(c) => c.start,
        }
    }

    /// Length on the timeline.
    #[must_use]
    pub fn duration(&self) -> Duration {
        match self {
            Item::Clip(c) => c.duration,
        }
    }

    /// The first time after the item, `start + duration`; `Err` if that leaves the safe range
    /// (only possible in an invalid document).
    pub fn end(&self) -> Result<Time, TimeError> {
        self.start() + self.duration()
    }

    /// `start + duration` in wide integers (never fails; used by validation).
    pub(crate) fn end_wide(&self) -> i128 {
        i128::from(self.start().flicks()) + i128::from(self.duration().flicks())
    }

    /// Whether the item is rendered and heard.
    #[must_use]
    pub fn enabled(&self) -> bool {
        match self {
            Item::Clip(c) => c.enabled,
        }
    }

    /// The linked group, if any.
    #[must_use]
    pub fn linked_group(&self) -> Option<GroupId> {
        match self {
            Item::Clip(c) => c.linked_group,
        }
    }

    /// The item this one is attached to (follows), if any.
    #[must_use]
    pub fn attached_to(&self) -> Option<ItemId> {
        match self {
            Item::Clip(c) => c.attached_to,
        }
    }

    /// Markers in item-relative time.
    #[must_use]
    pub fn markers(&self) -> &Markers {
        match self {
            Item::Clip(c) => &c.markers,
        }
    }

    /// The media the item plays, if it plays one.
    #[must_use]
    pub fn media(&self) -> Option<MediaId> {
        match self {
            Item::Clip(c) => Some(c.media),
        }
    }

    /// Plugin data.
    #[must_use]
    pub fn custom(&self) -> &Custom {
        match self {
            Item::Clip(c) => &c.custom,
        }
    }

    /// Moves the item. On a track, change it through `TrackItems::update`
    /// so the start index stays correct.
    pub fn set_start(&mut self, start: Time) {
        match self {
            Item::Clip(c) => c.start = start,
        }
    }

    /// Changes the length.
    pub fn set_duration(&mut self, duration: Duration) {
        match self {
            Item::Clip(c) => c.duration = duration,
        }
    }

    /// Enables or disables the item.
    pub fn set_enabled(&mut self, enabled: bool) {
        match self {
            Item::Clip(c) => c.enabled = enabled,
        }
    }

    /// Sets or clears the linked group.
    pub fn set_linked_group(&mut self, group: Option<GroupId>) {
        match self {
            Item::Clip(c) => c.linked_group = group,
        }
    }

    /// Sets or clears the item this one is attached to.
    pub fn set_attached_to(&mut self, parent: Option<ItemId>) {
        match self {
            Item::Clip(c) => c.attached_to = parent,
        }
    }

    /// The markers, mutably.
    pub fn markers_mut(&mut self) -> &mut Markers {
        match self {
            Item::Clip(c) => &mut c.markers,
        }
    }

    /// The clip, if this is one.
    #[must_use]
    pub fn as_clip(&self) -> Option<&Clip> {
        match self {
            Item::Clip(c) => Some(c),
        }
    }

    /// The clip, mutably, if this is one.
    pub fn as_clip_mut(&mut self) -> Option<&mut Clip> {
        match self {
            Item::Clip(c) => Some(c),
        }
    }
}

impl From<Clip> for Item {
    fn from(clip: Clip) -> Item {
        Item::Clip(clip)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn clip() -> Clip {
        Clip::new(
            ItemId::from_u128(1),
            MediaId::from_u128(2),
            Time::from_secs(1).unwrap(),
            Duration::from_secs(2).unwrap(),
        )
    }

    #[test]
    fn item_wire_form_is_tagged_and_complete() {
        let item = Item::from(clip());
        let value = serde_json::to_value(&item).unwrap();
        assert_eq!(value["kind"], "clip");
        assert_eq!(value["start"], 705_600_000);
        assert_eq!(value["visual"]["opacity"], 1.0);
        assert_eq!(
            value["visual"]["transform"]["anchor"],
            json!({"x": 0.5, "y": 0.5})
        );
        assert_eq!(value["audio"]["gain"], 0.0);
        assert_eq!(value["linked_group"], json!(null));
        assert_eq!(value["attached_to"], json!(null));
        assert_eq!(value["markers"], json!({}));
        let back: Item = serde_json::from_value(value).unwrap();
        assert_eq!(back, item);
    }

    #[test]
    fn every_field_is_required_except_options() {
        let mut value = serde_json::to_value(Item::from(clip())).unwrap();
        value.as_object_mut().unwrap().remove("name");
        assert!(
            serde_json::from_value::<Item>(value.clone()).is_ok(),
            "options may be omitted"
        );
        value.as_object_mut().unwrap().remove("enabled");
        let err = serde_json::from_value::<Item>(value)
            .unwrap_err()
            .to_string();
        assert!(err.contains("missing field `enabled`"), "{err}");
    }

    #[test]
    fn unknown_fields_are_kept() {
        let mut value = serde_json::to_value(Item::from(clip())).unwrap();
        value["future_field"] = json!({"b": 1, "a": [1, 2]});
        value["visual"]["look"] = json!("screen");
        let item: Item = serde_json::from_value(value.clone()).unwrap();
        let clip = item.as_clip().unwrap();
        assert_eq!(
            clip.extra.get("future_field"),
            Some(&json!({"a": [1, 2], "b": 1}))
        );
        assert_eq!(clip.visual.extra.get("look"), Some(&json!("screen")));
        assert_eq!(serde_json::to_value(&item).unwrap(), value);
    }

    #[test]
    fn keyed_values_round_trip_and_are_validated() {
        let mut value = serde_json::to_value(Item::from(clip())).unwrap();
        value["visual"]["opacity"] = json!({"keyframes": [
            {"t": 0, "value": 0.0, "interp": {"kind": "linear"}},
            {"t": 7, "value": 1.0, "interp": {"kind": "hold"}}
        ]});
        value["visual"]["flip"]["horizontal"] = json!({"keyframes": [
            {"t": 0, "value": false, "interp": {"kind": "hold"}}
        ]});
        let item: Item = serde_json::from_value(value.clone()).unwrap();
        assert!(item.as_clip().unwrap().visual.opacity.is_animated());
        assert_eq!(serde_json::to_value(&item).unwrap(), value);
        value["visual"]["opacity"] = json!({"keyframes": [{"t": 0, "value": 1.0}]});
        let err = serde_json::from_value::<Item>(value)
            .unwrap_err()
            .to_string();
        assert!(err.contains("KF-5"), "{err}");
    }

    #[test]
    fn vec2_rejects_unknown_fields() {
        assert!(serde_json::from_value::<Vec2>(json!({"x": 1, "y": 2, "z": 3})).is_err());
    }

    #[test]
    fn accessors() {
        let mut item = Item::from(clip());
        assert_eq!(item.end(), Time::from_secs(3));
        item.set_start(Time::MAX);
        assert!(item.end().is_err());
        item.set_duration(Duration::ZERO);
        item.set_enabled(false);
        item.set_linked_group(Some(GroupId::from_u128(9)));
        assert!(!item.enabled());
        assert_eq!(item.linked_group(), Some(GroupId::from_u128(9)));
        assert_eq!(item.media(), Some(MediaId::from_u128(2)));
        assert!(item.custom().is_empty());
        let mut opacity = Animatable::from(0.5);
        assert!(!opacity.is_animated());
        opacity.set(0.25);
        assert_eq!(*opacity.value_at(Time::ZERO), 0.25);
    }
}
