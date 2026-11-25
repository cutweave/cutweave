// SPDX-License-Identifier: GPL-3.0-or-later
//! Timeline items (arch §4.2), restricted to schema v1: media clips with audio gain, transform
//! and opacity. Text, captions, compositions, generators, adjustments, effects, masks and
//! keyframes arrive in later versions.

use std::{borrow::Cow, convert::Infallible, fmt, marker::PhantomData};

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::{Custom, Duration, Extra, GroupId, ItemId, Markers, MediaId, Time, TimeError};

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

/// The keyframed form of an [`Animatable`]. Keyframes are not part of schema v1, so this type
/// has no values: code that matches on [`Animatable::Keyframes`] compiles today and is flagged
/// by the compiler once keyframes land.
pub struct Keyframes<T> {
    never: Infallible,
    _marker: PhantomData<fn() -> T>,
}

impl<T> Keyframes<T> {
    /// Unreachable in schema v1 (the type has no values).
    #[must_use]
    pub fn unreachable<R>(&self) -> R {
        match self.never {}
    }
}

impl<T> Clone for Keyframes<T> {
    fn clone(&self) -> Self {
        self.unreachable()
    }
}

impl<T> fmt::Debug for Keyframes<T> {
    fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.unreachable()
    }
}

impl<T> PartialEq for Keyframes<T> {
    fn eq(&self, _: &Self) -> bool {
        self.unreachable()
    }
}

/// A property that may vary over the item's time (arch §4.2: `Static(T) | Keyframes(…)`).
///
/// Schema v1 only has static values: on the wire a static value is the bare `T`
/// (`"opacity": 1.0`), and a keyframed object (`{"keyframes": [...]}`) is rejected with a clear
/// error until keyframes are introduced.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Animatable<T> {
    /// The same value at every time.
    Static(T),
    /// Keyframed values (not constructible in schema v1).
    Keyframes(Keyframes<T>),
}

impl<T> Animatable<T> {
    /// The value at item-relative time `t`. In schema v1 every value is static.
    #[must_use]
    pub fn value_at(&self, _t: Time) -> &T {
        self.value()
    }

    /// The static value (in schema v1, every value).
    #[must_use]
    pub fn value(&self) -> &T {
        match self {
            Animatable::Static(v) => v,
            Animatable::Keyframes(k) => k.unreachable(),
        }
    }

    /// Replaces the property with a static value.
    pub fn set(&mut self, value: T) {
        *self = Animatable::Static(value);
    }

    /// Whether the value changes over time.
    #[must_use]
    pub fn is_animated(&self) -> bool {
        matches!(self, Animatable::Keyframes(_))
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

impl<T: Serialize> Serialize for Animatable<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.value().serialize(s)
    }
}

impl<'de, T: de::DeserializeOwned> Deserialize<'de> for Animatable<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Animatable<T>, D::Error> {
        let value = serde_json::Value::deserialize(d)?;
        if value.get("keyframes").is_some() {
            return Err(de::Error::custom(
                "keyframed values are not supported by this version of Cutweave (schema v1)",
            ));
        }
        T::deserialize(value)
            .map(Animatable::Static)
            .map_err(de::Error::custom)
    }
}

impl<T: JsonSchema> JsonSchema for Animatable<T> {
    fn schema_name() -> Cow<'static, str> {
        T::schema_name()
    }

    fn schema_id() -> Cow<'static, str> {
        T::schema_id()
    }

    fn inline_schema() -> bool {
        T::inline_schema()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        T::json_schema(generator)
    }
}

impl<T: specta::Type> specta::Type for Animatable<T> {
    fn definition(types: &mut specta::Types) -> specta::datatype::DataType {
        T::definition(types)
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
    #[specta(type = specta_typescript::Number)]
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

/// Visual properties of an item (arch §4.2 `VisualProps`, v1 subset).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
pub struct VisualProps {
    /// Placement on the canvas.
    pub transform: Transform,
    /// Opacity, 0.0 (transparent) to 1.0 (opaque).
    #[specta(type = specta_typescript::Number)]
    pub opacity: Animatable<f64>,
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
            extra: Extra::new(),
        }
    }
}

/// Audio properties of an item (arch §4.2 `audio{…}`, v1 subset).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
pub struct AudioProps {
    /// Gain in decibels (0.0 = unchanged).
    #[specta(type = specta_typescript::Number)]
    pub gain: Animatable<f64>,
    /// Fields unknown to this version.
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: Extra,
}

/// A media clip: a range of a `MediaAsset` placed on a track.
///
/// The clip occupies `[start, start + duration)` on the timeline and plays the source range
/// `[source_in, source_in + duration)` (speed 1 in schema v1).
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
    /// Source position (from the start of the media) of the first frame.
    pub source_in: Time,
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
            audio: AudioProps::default(),
            visual: VisualProps::default(),
            custom: Custom::new(),
            extra: Extra::new(),
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
        value["visual"]["blend_mode"] = json!("screen");
        let item: Item = serde_json::from_value(value.clone()).unwrap();
        let clip = item.as_clip().unwrap();
        assert_eq!(
            clip.extra.get("future_field"),
            Some(&json!({"a": [1, 2], "b": 1}))
        );
        assert_eq!(clip.visual.extra.get("blend_mode"), Some(&json!("screen")));
        assert_eq!(serde_json::to_value(&item).unwrap(), value);
    }

    #[test]
    fn keyframes_are_rejected_in_v1() {
        let mut value = serde_json::to_value(Item::from(clip())).unwrap();
        value["visual"]["opacity"] = json!({"keyframes": [{"t": 0, "value": 1.0}]});
        let err = serde_json::from_value::<Item>(value)
            .unwrap_err()
            .to_string();
        assert!(err.contains("keyframed values are not supported"), "{err}");
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
