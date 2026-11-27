// SPDX-License-Identifier: GPL-3.0-or-later
//! Sequences and tracks (arch §4.2). A track keeps its items in an ID-keyed persistent map
//! (O(log n) lookup, O(1) snapshot clones) plus an index ordered by start time.

use std::{borrow::Cow, collections::BTreeMap, fmt, str::FromStr};

use imbl::{OrdMap, OrdSet, Vector};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::{
    Custom, Duration, Extra, FrameRate, InOut, Item, ItemId, MarkerId, Markers, MediaId, MediaKind,
    SampleRate, SequenceId, Time, TrackId,
};

/// The working color space of a sequence. Schema v1 has only Rec.709 SDR.
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
#[non_exhaustive]
pub enum ColorSpace {
    /// Rec.709 primaries, SDR.
    #[default]
    Rec709,
}

/// An sRGB color with straight alpha, written `#rrggbb` (opaque) or `#rrggbbaa`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, specta::Type)]
#[specta(type = String)]
pub struct Color {
    /// Red, 0–255.
    pub r: u8,
    /// Green, 0–255.
    pub g: u8,
    /// Blue, 0–255.
    pub b: u8,
    /// Alpha, 0 (transparent) – 255 (opaque).
    pub a: u8,
}

impl Color {
    /// Opaque black.
    pub const BLACK: Color = Color::rgb(0, 0, 0);

    /// An opaque color.
    #[must_use]
    pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color { r, g, b, a: 255 }
    }
}

/// A string that is not a `#rrggbb` or `#rrggbbaa` color.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("invalid color `{0}`; expected `#rrggbb` or `#rrggbbaa` (lowercase or uppercase hex)")]
pub struct ColorError(pub String);

impl FromStr for Color {
    type Err = ColorError;
    fn from_str(s: &str) -> Result<Color, ColorError> {
        let err = || ColorError(s.to_owned());
        let hex = s.strip_prefix('#').ok_or_else(err)?;
        if !(hex.len() == 6 || hex.len() == 8) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(err());
        }
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| err());
        Ok(Color {
            r: byte(0)?,
            g: byte(2)?,
            b: byte(4)?,
            a: if hex.len() == 8 { byte(6)? } else { 255 },
        })
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)?;
        if self.a != 255 {
            write!(f, "{:02x}", self.a)?;
        }
        Ok(())
    }
}

impl Serialize for Color {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Color, D::Error> {
        Cow::<'de, str>::deserialize(d)?
            .parse()
            .map_err(de::Error::custom)
    }
}

impl JsonSchema for Color {
    fn schema_name() -> Cow<'static, str> {
        "Color".into()
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "sRGB color with straight alpha: \"#rrggbb\" or \"#rrggbbaa\".",
            "type": "string",
            "pattern": "^#([0-9a-fA-F]{6}|[0-9a-fA-F]{8})$"
        })
    }
}

/// Canvas size, frame rate, audio rate and color of a sequence (arch §4.2 `format`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
pub struct SequenceFormat {
    /// Canvas width in pixels.
    pub width: u32,
    /// Canvas height in pixels.
    pub height: u32,
    /// The edit grid: timecodes and `"f:N"` inputs resolve against it.
    pub frame_rate: FrameRate,
    /// The mix rate.
    pub sample_rate: SampleRate,
    /// Working color space.
    pub color_space: ColorSpace,
    /// Canvas color behind all tracks.
    pub background: Color,
    /// What fills the canvas over that color, behind all tracks (WP-2B.02). Absent: the color
    /// alone. Left out of the file while absent, so documents without one read as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_fill: Option<BackgroundFill>,
    /// Fields unknown to this version.
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: Extra,
}

impl SequenceFormat {
    /// A format with Rec.709 color and a black background.
    #[must_use]
    pub fn new(width: u32, height: u32, frame_rate: FrameRate, sample_rate: SampleRate) -> Self {
        SequenceFormat {
            width,
            height,
            frame_rate,
            sample_rate,
            color_space: ColorSpace::Rec709,
            background: Color::BLACK,
            background_fill: None,
            extra: Extra::new(),
        }
    }
}

/// The largest [`BackgroundFill::Blur`] radius: the whole canvas height.
pub const MAX_BACKGROUND_BLUR_RADIUS: f64 = 1.0;

/// What fills a sequence's canvas behind all tracks, over its `background` color (WP-2B.02).
/// It is static: a background has no keyframes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BackgroundFill {
    /// A blurred copy of a picture of the sequence, scaled to cover the canvas. Where no such
    /// picture shows, the color does.
    Blur {
        /// Which picture is blurred.
        #[serde(default)]
        source: BlurSource,
        /// The blur's standard deviation as a fraction of the canvas height, `0..=1`, so
        /// preview and export blur alike at any size.
        radius: f64,
        /// How much of the blurred picture's light is taken away, `0..=1` (0: none, 1: black).
        #[serde(default)]
        dim: f64,
    },
    /// A still image of the project. An image that no longer exists draws nothing.
    Image {
        /// The image asset.
        media: MediaId,
        /// How the image is sized to the canvas.
        #[serde(default)]
        fit: BackgroundFit,
    },
    /// Nothing: the canvas starts fully transparent and the color is not drawn. Only exports
    /// that keep alpha show it; the others see black.
    Transparent,
}

impl BackgroundFill {
    /// The image asset the fill shows, if it is an image.
    #[must_use]
    pub fn media(&self) -> Option<MediaId> {
        match self {
            BackgroundFill::Image { media, .. } => Some(*media),
            BackgroundFill::Blur { .. } | BackgroundFill::Transparent => None,
        }
    }

    /// What is wrong with the fill's numbers, if anything.
    #[must_use]
    pub fn problem(&self) -> Option<&'static str> {
        let BackgroundFill::Blur { radius, dim, .. } = self else {
            return None;
        };
        if !(0.0..=MAX_BACKGROUND_BLUR_RADIUS).contains(radius) {
            return Some("the blur radius must be between 0 and 1 (a fraction of the height)");
        }
        if !(0.0..=1.0).contains(dim) {
            return Some("the dim must be between 0 and 1");
        }
        None
    }
}

/// The picture a [`BackgroundFill::Blur`] blurs.
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
pub enum BlurSource {
    /// The lowest picture showing at that time.
    #[default]
    Below,
    /// The picture on the main track (the magnetic track; without one, the lowest video
    /// track).
    MainTrack,
}

/// How a [`BackgroundFill::Image`] is sized to the canvas.
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
pub enum BackgroundFit {
    /// Fills the canvas, cropping what does not fit.
    #[default]
    Cover,
    /// The whole image inside the canvas; the color shows around it.
    Contain,
    /// Stretched to the canvas, changing its shape.
    Stretch,
}

/// 1920×1080 at 30 fps, 48 kHz (the `project.new` default, WP-1B.05).
impl Default for SequenceFormat {
    fn default() -> SequenceFormat {
        SequenceFormat::new(1920, 1080, FrameRate::FPS_30, SampleRate::HZ_48_000)
    }
}

/// What a track holds. Schema v1 edits only video and audio tracks; the other kinds exist so
/// documents and commands can name them (ops answer `Unsupported`).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, specta::Type,
)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    /// Picture layers.
    Video,
    /// Sound.
    Audio,
    /// Titles (later versions).
    Text,
    /// Captions (later versions).
    Caption,
    /// Adjustment layers (later versions).
    Adjustment,
}

impl TrackKind {
    /// Whether a clip of `media` can sit on a track of this kind: video tracks take pictures
    /// (video, image, sequence), audio tracks take sound (audio, video, sequence).
    #[must_use]
    pub const fn accepts(self, media: MediaKind) -> bool {
        match self {
            TrackKind::Video => matches!(
                media,
                MediaKind::Video
                    | MediaKind::Image
                    | MediaKind::ImageSequence
                    | MediaKind::SequenceRef
            ),
            // An image sequence has no sound (ADR-0035).
            TrackKind::Audio => matches!(
                media,
                MediaKind::Audio | MediaKind::Video | MediaKind::SequenceRef
            ),
            TrackKind::Text | TrackKind::Caption | TrackKind::Adjustment => false,
        }
    }

    /// Lowercase name for messages.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            TrackKind::Video => "video",
            TrackKind::Audio => "audio",
            TrackKind::Text => "text",
            TrackKind::Caption => "caption",
            TrackKind::Adjustment => "adjustment",
        }
    }
}

impl fmt::Display for TrackKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The items of one track: an ID-keyed persistent map plus an index ordered by start.
///
/// Mutate only through [`insert`](Self::insert), [`remove`](Self::remove) and
/// [`update`](Self::update), which keep the index in step. Cloning is O(1) and shares
/// structure. Serialized as an object keyed by item ID (in ID order).
#[derive(Clone, Default)]
pub struct TrackItems {
    by_id: OrdMap<ItemId, Item>,
    by_start: OrdSet<(Time, ItemId)>,
}

impl TrackItems {
    /// An empty track.
    #[must_use]
    pub fn new() -> TrackItems {
        TrackItems::default()
    }

    /// Number of items.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    /// Whether the track is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// The item with this ID.
    #[must_use]
    pub fn get(&self, id: &ItemId) -> Option<&Item> {
        self.by_id.get(id)
    }

    /// Whether the track holds this item.
    #[must_use]
    pub fn contains(&self, id: &ItemId) -> bool {
        self.by_id.contains_key(id)
    }

    /// Adds `item` under its ID, replacing (and returning) an item with the same ID. Does not
    /// check overlaps; commands do, and `Project::validate` reports
    /// them.
    pub fn insert(&mut self, item: Item) -> Option<Item> {
        let id = item.id();
        let start = item.start();
        let old = self.by_id.insert(id, item);
        if let Some(old) = &old {
            self.by_start.remove(&(old.start(), id));
        }
        self.by_start.insert((start, id));
        old
    }

    /// Removes and returns the item with this ID.
    pub fn remove(&mut self, id: &ItemId) -> Option<Item> {
        let old = self.by_id.remove(id)?;
        self.by_start.remove(&(old.start(), *id));
        Some(old)
    }

    /// Changes the item with this ID in place (re-indexing if its start changes) and returns
    /// `f`'s result, or `None` if there is no such item. `f` must not change the item's ID.
    pub fn update<R>(&mut self, id: &ItemId, f: impl FnOnce(&mut Item) -> R) -> Option<R> {
        let item = self.by_id.get_mut(id)?;
        let before = item.start();
        let result = f(item);
        let after = item.start();
        if before != after {
            self.by_start.remove(&(before, *id));
            self.by_start.insert((after, *id));
        }
        Some(result)
    }

    /// Items ordered by start time (ties by ID).
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &Item> + '_ {
        self.by_start
            .iter()
            .filter_map(|(_, id)| self.by_id.get(id))
    }

    /// `(key, item)` pairs in ID order. The key equals `item.id()` in a valid document.
    pub fn iter_by_id(&self) -> impl Iterator<Item = (&ItemId, &Item)> + '_ {
        self.by_id.iter()
    }

    /// The item playing at `t` (`start <= t < end`), if any.
    #[must_use]
    pub fn at(&self, t: Time) -> Option<&Item> {
        let (_, id) = self
            .by_start
            .range(..=(t, ItemId::from_u128(u128::MAX)))
            .next_back()?;
        let item = self.by_id.get(id)?;
        (i128::from(t.flicks()) < item.end_wide()).then_some(item)
    }

    /// Items intersecting the half-open range `[start, end)`, in start order. Assumes the track
    /// has no overlaps (a valid document).
    pub fn overlapping(&self, start: Time, end: Time) -> impl Iterator<Item = &Item> + '_ {
        let low = (start, ItemId::from_u128(0));
        let before = self
            .by_start
            .range(..low)
            .next_back()
            .and_then(|(_, id)| self.by_id.get(id))
            .filter(|item| item.end_wide() > i128::from(start.flicks()));
        let inside = self
            .by_start
            .range(low..(end, ItemId::from_u128(0)))
            .filter_map(|(_, id)| self.by_id.get(id));
        before.into_iter().chain(inside)
    }

    /// The first pair of overlapping items in start order, if any.
    #[must_use]
    pub fn first_overlap(&self) -> Option<(ItemId, ItemId)> {
        let mut furthest: Option<(i128, ItemId)> = None;
        for item in self.iter() {
            if let Some((end, id)) = furthest
                && i128::from(item.start().flicks()) < end
            {
                return Some((id, item.id()));
            }
            let end = item.end_wide();
            if furthest.is_none_or(|(e, _)| end > e) {
                furthest = Some((end, item.id()));
            }
        }
        None
    }

    /// The end of the last item (zero for an empty track), saturating at [`Time::MAX`].
    #[must_use]
    pub fn end(&self) -> Time {
        let end = self.by_id.values().map(Item::end_wide).max().unwrap_or(0);
        let clamped = end.clamp(0, i128::from(Time::MAX.flicks()));
        i64::try_from(clamped)
            .ok()
            .and_then(|f| Time::from_flicks(f).ok())
            .unwrap_or(Time::MAX)
    }

    /// Whether `self` and `other` share the same storage (snapshot check).
    #[must_use]
    pub fn ptr_eq(&self, other: &TrackItems) -> bool {
        self.by_id.ptr_eq(&other.by_id) && self.by_start.ptr_eq(&other.by_start)
    }
}

impl PartialEq for TrackItems {
    fn eq(&self, other: &TrackItems) -> bool {
        self.by_id == other.by_id
    }
}

impl fmt::Debug for TrackItems {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.by_id.iter()).finish()
    }
}

impl FromIterator<Item> for TrackItems {
    fn from_iter<I: IntoIterator<Item = Item>>(iter: I) -> TrackItems {
        let mut items = TrackItems::new();
        for item in iter {
            items.insert(item);
        }
        items
    }
}

impl Serialize for TrackItems {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.by_id.serialize(s)
    }
}

impl<'de> Deserialize<'de> for TrackItems {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<TrackItems, D::Error> {
        let by_id = OrdMap::<ItemId, Item>::deserialize(d)?;
        // Index under the stored key, so a key/ID mismatch stays visible to validation.
        let by_start = by_id
            .iter()
            .map(|(key, item)| (item.start(), *key))
            .collect();
        Ok(TrackItems { by_id, by_start })
    }
}

impl JsonSchema for TrackItems {
    fn schema_name() -> Cow<'static, str> {
        "TrackItems".into()
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        generator.subschema_for::<BTreeMap<ItemId, Item>>()
    }
}

impl specta::Type for TrackItems {
    fn definition(types: &mut specta::Types) -> specta::datatype::DataType {
        <BTreeMap<ItemId, Item> as specta::Type>::definition(types)
    }
}

/// A track of a sequence (arch §4.2): items that never overlap (arch §4.3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
#[allow(clippy::struct_excessive_bools)] // independent user-facing toggles
pub struct Track {
    /// The track ID (also its key in `Sequence::tracks`).
    pub id: TrackId,
    /// What the track holds.
    pub kind: TrackKind,
    /// Display name.
    pub name: String,
    /// The items, keyed by ID.
    pub items: TrackItems,
    /// Silenced (audio).
    pub muted: bool,
    /// Not rendered (video).
    pub hidden: bool,
    /// Commands that would change the track's items fail with `Locked`.
    pub locked: bool,
    /// Solo (audio): when any track is solo, only solo tracks are heard.
    pub solo: bool,
    /// The magnetic main track: ripple-by-default editing, gap-free from 0 (timeline spec
    /// §1.2). At most one video track per sequence has it (`Project::validate`).
    pub magnetic: bool,
    /// Row height in the timeline, in pixels: a persisted UI hint (`None` = the default).
    /// Changing it never touches items.
    pub height: Option<u32>,
    /// Track gain in decibels (0.0 = unchanged).
    #[specta(type = specta_typescript::Number)]
    pub gain: f64,
    /// Plugin data (arch §4.3).
    pub custom: Custom,
    /// Fields unknown to this version.
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: Extra,
}

impl Track {
    /// An empty, unlocked, audible and visible track.
    #[must_use]
    pub fn new(id: TrackId, kind: TrackKind, name: impl Into<String>) -> Track {
        Track {
            id,
            kind,
            name: name.into(),
            items: TrackItems::new(),
            muted: false,
            hidden: false,
            locked: false,
            solo: false,
            magnetic: false,
            height: None,
            gain: 0.0,
            custom: Custom::new(),
            extra: Extra::new(),
        }
    }
}

/// A timeline (arch §4.2). A project may hold several (e.g. a long cut and its shorts); the v1
/// UI shows one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
pub struct Sequence {
    /// The sequence ID (also its key in `Project::sequences`).
    pub id: SequenceId,
    /// Display name.
    pub name: String,
    /// Canvas, rates and color.
    pub format: SequenceFormat,
    /// The tracks, keyed by ID.
    #[schemars(with = "BTreeMap<TrackId, Track>")]
    #[specta(type = BTreeMap<TrackId, Track>)]
    pub tracks: OrdMap<TrackId, Track>,
    /// Display order, top to bottom: every key of `tracks` exactly once. Upper video tracks
    /// are composited over lower ones.
    #[schemars(with = "Vec<TrackId>")]
    #[specta(type = Vec<TrackId>)]
    pub track_order: Vector<TrackId>,
    /// Markers in sequence time, keyed by ID. They never move with a ripple (spec INV-34).
    #[schemars(with = "BTreeMap<MarkerId, crate::Marker>")]
    #[specta(type = BTreeMap<MarkerId, crate::Marker>)]
    pub markers: Markers,
    /// The in and out points; both ends or neither.
    pub in_out: Option<InOut>,
    /// Plugin data (arch §4.3).
    pub custom: Custom,
    /// Fields unknown to this version.
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: Extra,
}

impl Sequence {
    /// An empty sequence.
    #[must_use]
    pub fn new(id: SequenceId, name: impl Into<String>, format: SequenceFormat) -> Sequence {
        Sequence {
            id,
            name: name.into(),
            format,
            tracks: OrdMap::new(),
            track_order: Vector::new(),
            markers: Markers::new(),
            in_out: None,
            custom: Custom::new(),
            extra: Extra::new(),
        }
    }

    /// The track with this ID.
    #[must_use]
    pub fn track(&self, id: &TrackId) -> Option<&Track> {
        self.tracks.get(id)
    }

    /// The track with this ID, mutably (copy-on-write).
    pub fn track_mut(&mut self, id: &TrackId) -> Option<&mut Track> {
        self.tracks.get_mut(id)
    }

    /// Tracks in display order (top to bottom).
    pub fn tracks_in_order(&self) -> impl Iterator<Item = &Track> + '_ {
        self.track_order.iter().filter_map(|id| self.tracks.get(id))
    }

    /// Adds `track` at display position `index` (clamped to the end), replacing a track with
    /// the same ID.
    pub fn insert_track(&mut self, index: usize, track: Track) {
        let id = track.id;
        self.remove_track(&id);
        self.tracks.insert(id, track);
        self.track_order
            .insert(index.min(self.track_order.len()), id);
    }

    /// Removes and returns the track with this ID.
    pub fn remove_track(&mut self, id: &TrackId) -> Option<Track> {
        self.track_order.retain(|t| t != id);
        self.tracks.remove(id)
    }

    /// The track holding `item`, and the item.
    #[must_use]
    pub fn find_item(&self, item: &ItemId) -> Option<(&Track, &Item)> {
        self.tracks
            .values()
            .find_map(|track| track.items.get(item).map(|i| (track, i)))
    }

    /// The end of the last item on any track.
    #[must_use]
    pub fn duration(&self) -> Duration {
        let end = self
            .tracks
            .values()
            .map(|t| t.items.end())
            .max()
            .unwrap_or(Time::ZERO);
        Duration::try_from(end).unwrap_or(Duration::ZERO)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Clip, MediaId};

    fn clip(n: u128, start: i64, duration: i64) -> Item {
        Clip::new(
            ItemId::from_u128(n),
            MediaId::from_u128(1),
            Time::from_flicks(start).unwrap(),
            Duration::from_flicks(duration).unwrap(),
        )
        .into()
    }

    fn ids<'a>(items: impl Iterator<Item = &'a Item>) -> Vec<u128> {
        items.map(|i| i.id().as_u128()).collect()
    }

    fn at(f: i64) -> Time {
        Time::from_flicks(f).unwrap()
    }

    #[test]
    fn index_follows_mutations() {
        let mut items: TrackItems = [clip(3, 200, 50), clip(1, 0, 100), clip(2, 100, 100)]
            .into_iter()
            .collect();
        assert_eq!(ids(items.iter()), [1, 2, 3]);
        items.update(&ItemId::from_u128(1), |i| i.set_start(at(300)));
        assert_eq!(ids(items.iter()), [2, 3, 1]);
        let replaced = items.insert(clip(2, 500, 10)).unwrap();
        assert_eq!(replaced.start(), at(100));
        assert_eq!(ids(items.iter()), [3, 1, 2]);
        assert_eq!(
            items
                .remove(&ItemId::from_u128(3))
                .map(|i| i.id().as_u128()),
            Some(3)
        );
        assert_eq!(ids(items.iter()), [1, 2]);
        assert_eq!(items.len(), 2);
        assert!(items.update(&ItemId::from_u128(9), |_| ()).is_none());
        assert!(items.remove(&ItemId::from_u128(9)).is_none());
        assert_eq!(items.end(), at(510));
    }

    #[test]
    fn lookups_by_time() {
        let items: TrackItems = [clip(1, 0, 100), clip(2, 100, 100), clip(3, 300, 50)]
            .into_iter()
            .collect();
        assert_eq!(items.at(at(0)).map(Item::id), Some(ItemId::from_u128(1)));
        assert_eq!(items.at(at(99)).map(Item::id), Some(ItemId::from_u128(1)));
        assert_eq!(items.at(at(100)).map(Item::id), Some(ItemId::from_u128(2)));
        assert!(items.at(at(250)).is_none());
        assert!(items.at(at(-1)).is_none());
        assert_eq!(ids(items.overlapping(at(50), at(150))), [1, 2]);
        assert_eq!(ids(items.overlapping(at(100), at(300))), [2]);
        assert_eq!(ids(items.overlapping(at(200), at(300))), Vec::<u128>::new());
        assert_eq!(ids(items.overlapping(at(0), at(1000))), [1, 2, 3]);
        assert_eq!(items.first_overlap(), None);
    }

    #[test]
    fn overlaps_are_found() {
        let items: TrackItems = [clip(1, 0, 100), clip(2, 99, 10)].into_iter().collect();
        assert_eq!(
            items.first_overlap(),
            Some((ItemId::from_u128(1), ItemId::from_u128(2)))
        );
        // An item nested inside a long one is found even after a short one in between.
        let items: TrackItems = [clip(1, 0, 1000), clip(2, 10, 5), clip(3, 500, 5)]
            .into_iter()
            .collect();
        assert_eq!(
            items.first_overlap(),
            Some((ItemId::from_u128(1), ItemId::from_u128(2)))
        );
    }

    #[test]
    fn serializes_as_id_keyed_object_and_reindexes_on_load() {
        let items: TrackItems = [clip(2, 0, 10), clip(1, 10, 10)].into_iter().collect();
        let value = serde_json::to_value(&items).unwrap();
        let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
        assert_eq!(keys.len(), 2);
        let back: TrackItems = serde_json::from_value(value).unwrap();
        assert_eq!(back, items);
        assert_eq!(ids(back.iter()), [2, 1]);
        let copy = back.clone();
        assert!(copy.ptr_eq(&back));
    }

    #[test]
    fn track_order_helpers() {
        let mut seq = Sequence::new(SequenceId::from_u128(1), "Main", SequenceFormat::default());
        let v1 = Track::new(TrackId::from_u128(1), TrackKind::Video, "V1");
        let v2 = Track::new(TrackId::from_u128(2), TrackKind::Video, "V2");
        let a1 = Track::new(TrackId::from_u128(3), TrackKind::Audio, "A1");
        seq.insert_track(0, v1);
        seq.insert_track(99, a1);
        seq.insert_track(0, v2);
        let names: Vec<&str> = seq.tracks_in_order().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["V2", "V1", "A1"]);
        seq.track_mut(&TrackId::from_u128(1))
            .unwrap()
            .items
            .insert(clip(7, 5, 10));
        let (track, item) = seq.find_item(&ItemId::from_u128(7)).unwrap();
        assert_eq!((track.name.as_str(), item.start()), ("V1", at(5)));
        assert_eq!(seq.duration(), Duration::from_flicks(15).unwrap());
        assert!(seq.remove_track(&TrackId::from_u128(2)).is_some());
        assert_eq!(seq.track_order.len(), 2);
        assert!(seq.track(&TrackId::from_u128(2)).is_none());
    }

    #[test]
    fn colors() {
        assert_eq!("#ff8000".parse::<Color>(), Ok(Color::rgb(255, 128, 0)));
        assert_eq!("#FF800080".parse::<Color>().unwrap().a, 128);
        assert_eq!(Color::rgb(1, 2, 3).to_string(), "#010203");
        assert_eq!(
            Color {
                a: 0,
                ..Color::BLACK
            }
            .to_string(),
            "#00000000"
        );
        for bad in ["ff8000", "#ff80", "#ff80000", "#gg8000", "#ff800080ff"] {
            assert!(bad.parse::<Color>().is_err(), "{bad}");
        }
        assert_eq!(
            serde_json::to_string(&Color::BLACK).unwrap(),
            r##""#000000""##
        );
    }

    #[test]
    fn track_kinds_accept_matching_media() {
        assert!(TrackKind::Video.accepts(MediaKind::Image));
        assert!(!TrackKind::Video.accepts(MediaKind::Audio));
        assert!(TrackKind::Audio.accepts(MediaKind::Video));
        assert!(!TrackKind::Audio.accepts(MediaKind::Image));
        assert!(TrackKind::Video.accepts(MediaKind::ImageSequence));
        assert!(!TrackKind::Audio.accepts(MediaKind::ImageSequence));
        assert!(!TrackKind::Text.accepts(MediaKind::Video));
    }
}
