// SPDX-License-Identifier: GPL-3.0-or-later
//! [`DocPath`]: a JSON Pointer (RFC 6901) into the ID-addressed project document, and
//! [`EntityRef`], the entity a path lands in.
//!
//! The document's JSON form is keyed by ID at every level, so paths name entities by ID, never
//! by index (arch §4.3):
//!
//! ```text
//! /meta/name
//! /media/{med_…}
//! /sequences/{seq_…}/track_order
//! /sequences/{seq_…}/tracks/{trk_…}/locked
//! /sequences/{seq_…}/tracks/{trk_…}/items/{itm_…}/visual/opacity
//! ```

use std::{borrow::Cow, fmt, str::FromStr};

use cutweave_model::{ItemId, MediaId, SequenceId, TrackId};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

/// JSON Schema pattern of a [`DocPath`]: empty, or `/`-separated segments where `~` only
/// appears as `~0` or `~1`.
pub const DOC_PATH_PATTERN: &str = "^(/([^~]|~[01])*)*$";

/// Escapes one segment (`~` → `~0`, `/` → `~1`).
#[must_use]
pub fn escape(segment: &str) -> Cow<'_, str> {
    if segment.contains(['~', '/']) {
        Cow::Owned(segment.replace('~', "~0").replace('/', "~1"))
    } else {
        Cow::Borrowed(segment)
    }
}

fn unescape(segment: &str) -> Cow<'_, str> {
    if segment.contains('~') {
        Cow::Owned(segment.replace("~1", "/").replace("~0", "~"))
    } else {
        Cow::Borrowed(segment)
    }
}

/// A string that is not a valid JSON Pointer.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error(
    "`{0}` is not a JSON Pointer (expected \"\" or \"/segment/…\", with `~` written as `~0` and `/` inside a segment as `~1`)"
)]
pub struct DocPathError(pub String);

/// A JSON Pointer (RFC 6901) into the project document. `""` is the document root.
///
/// Built with [`DocPath::root`] and [`DocPath::join`] (which escape segments), or the
/// entity helpers ([`DocPath::item`], …). Serialized as its string.
#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DocPath(String);

impl DocPath {
    /// The document root (`""`).
    #[must_use]
    pub const fn root() -> DocPath {
        DocPath(String::new())
    }

    /// Parses and validates a pointer.
    pub fn parse(s: &str) -> Result<DocPath, DocPathError> {
        let valid = s.is_empty()
            || (s.starts_with('/') && {
                let mut chars = s.chars();
                let mut ok = true;
                while let Some(c) = chars.next() {
                    if c == '~' && !matches!(chars.next(), Some('0' | '1')) {
                        ok = false;
                        break;
                    }
                }
                ok
            });
        if valid {
            Ok(DocPath(s.to_owned()))
        } else {
            Err(DocPathError(s.to_owned()))
        }
    }

    /// The pointer text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this is the document root.
    #[must_use]
    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    /// The unescaped segments (none for the root).
    pub fn segments(&self) -> impl Iterator<Item = Cow<'_, str>> + '_ {
        self.0.split('/').skip(1).map(unescape)
    }

    /// The unescaped segments, collected.
    #[must_use]
    pub fn to_segments(&self) -> Vec<String> {
        self.segments().map(Cow::into_owned).collect()
    }

    /// Number of segments.
    #[must_use]
    pub fn depth(&self) -> usize {
        self.0.matches('/').count()
    }

    /// This path with one more segment (escaped as needed).
    #[must_use]
    pub fn join(&self, segment: impl fmt::Display) -> DocPath {
        let segment = segment.to_string();
        let mut out = String::with_capacity(self.0.len() + segment.len() + 1);
        out.push_str(&self.0);
        out.push('/');
        out.push_str(&escape(&segment));
        DocPath(out)
    }

    /// The parent path and the last (unescaped) segment; `None` for the root.
    #[must_use]
    pub fn split_last(&self) -> Option<(DocPath, String)> {
        let i = self.0.rfind('/')?;
        Some((
            DocPath(self.0[..i].to_owned()),
            unescape(&self.0[i + 1..]).into_owned(),
        ))
    }

    /// Whether `self` is `other` or one of its ancestors (segment-wise).
    #[must_use]
    pub fn is_prefix_of(&self, other: &DocPath) -> bool {
        other.0.starts_with(&self.0)
            && (other.0.len() == self.0.len() || other.0.as_bytes()[self.0.len()] == b'/')
    }

    /// `/meta`.
    #[must_use]
    pub fn meta() -> DocPath {
        DocPath::root().join("meta")
    }

    /// `/settings`.
    #[must_use]
    pub fn settings() -> DocPath {
        DocPath::root().join("settings")
    }

    /// `/custom` (project-level plugin data).
    #[must_use]
    pub fn custom() -> DocPath {
        DocPath::root().join("custom")
    }

    /// `/media/{id}`.
    #[must_use]
    pub fn media(id: MediaId) -> DocPath {
        DocPath::root().join("media").join(id)
    }

    /// `/sequences/{id}`.
    #[must_use]
    pub fn sequence(id: SequenceId) -> DocPath {
        DocPath::root().join("sequences").join(id)
    }

    /// `/sequences/{sequence}/tracks/{track}`.
    #[must_use]
    pub fn track(sequence: SequenceId, track: TrackId) -> DocPath {
        DocPath::sequence(sequence).join("tracks").join(track)
    }

    /// `/sequences/{sequence}/tracks/{track}/items/{item}`.
    #[must_use]
    pub fn item(sequence: SequenceId, track: TrackId, item: ItemId) -> DocPath {
        DocPath::track(sequence, track).join("items").join(item)
    }

    /// The innermost entity this path lands in (see [`EntityRef`]). Paths whose ID segments do
    /// not parse land in the nearest enclosing entity.
    #[must_use]
    pub fn entity(&self) -> EntityRef {
        let segments: Vec<Cow<'_, str>> = self.segments().collect();
        let seg = |i: usize| segments.get(i).map(AsRef::as_ref);
        match (seg(0), seg(1)) {
            (Some("media"), Some(id)) => id.parse().map_or(EntityRef::Project, EntityRef::Media),
            (Some("sequences"), Some(id)) => {
                let Ok(sequence) = id.parse::<SequenceId>() else {
                    return EntityRef::Project;
                };
                let Some(Ok(track)) = (seg(2) == Some("tracks"))
                    .then(|| seg(3).map(str::parse::<TrackId>))
                    .flatten()
                else {
                    return EntityRef::Sequence(sequence);
                };
                match (seg(4), seg(5).map(str::parse::<ItemId>)) {
                    (Some("items"), Some(Ok(item))) => EntityRef::Item {
                        sequence,
                        track,
                        item,
                    },
                    _ => EntityRef::Track { sequence, track },
                }
            }
            _ => EntityRef::Project,
        }
    }
}

/// The entity a [`DocPath`] lands in: what an op touches, for soft locks (WP-1B.09) and
/// scoped checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EntityRef {
    /// Project-level data: `meta`, `settings`, `custom`, unknown fields, or a whole collection.
    Project,
    /// A media asset.
    Media(MediaId),
    /// A sequence's own fields (name, format, track order, …).
    Sequence(SequenceId),
    /// A track's own fields (flags, gain, …), or the track as a whole.
    Track {
        /// The sequence holding the track.
        sequence: SequenceId,
        /// The track.
        track: TrackId,
    },
    /// An item, or one of its fields.
    Item {
        /// The sequence holding the item.
        sequence: SequenceId,
        /// The track holding the item.
        track: TrackId,
        /// The item.
        item: ItemId,
    },
}

impl FromStr for DocPath {
    type Err = DocPathError;
    fn from_str(s: &str) -> Result<DocPath, DocPathError> {
        DocPath::parse(s)
    }
}

impl fmt::Display for DocPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for DocPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DocPath({:?})", self.0)
    }
}

impl Serialize for DocPath {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for DocPath {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<DocPath, D::Error> {
        let s = Cow::<'de, str>::deserialize(d)?;
        DocPath::parse(&s).map_err(de::Error::custom)
    }
}

impl JsonSchema for DocPath {
    fn schema_name() -> Cow<'static, str> {
        "DocPath".into()
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "JSON Pointer (RFC 6901) into the project document, addressed by ID (e.g. \"/sequences/seq_…/tracks/trk_…/items/itm_…/start\").",
            "type": "string",
            "pattern": DOC_PATH_PATTERN
        })
    }
}

impl specta::Type for DocPath {
    fn definition(types: &mut specta::Types) -> specta::datatype::DataType {
        <String as specta::Type>::definition(types)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_validates_escapes() {
        for ok in ["", "/", "/a", "/a/b", "/a~0b/c~1d", "//", "/a/"] {
            assert_eq!(DocPath::parse(ok).unwrap().as_str(), ok);
        }
        for bad in ["a", "a/b", "/a~", "/a~2", "/~x"] {
            assert!(DocPath::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn segments_unescape_and_join_escapes() {
        let p = DocPath::root().join("a/b").join("c~d").join("");
        assert_eq!(p.as_str(), "/a~1b/c~0d/");
        assert_eq!(p.to_segments(), ["a/b", "c~d", ""]);
        assert_eq!(p.depth(), 3);
        assert_eq!(DocPath::root().depth(), 0);
        assert_eq!(DocPath::root().to_segments().len(), 0);
        let (parent, last) = p.split_last().unwrap();
        assert_eq!((parent.as_str(), last.as_str()), ("/a~1b/c~0d", ""));
        assert!(DocPath::root().split_last().is_none());
        // `~01` decodes to `~1`, not `/` (decode `~1` first, then `~0`).
        assert_eq!(DocPath::parse("/~01").unwrap().to_segments(), ["~1"]);
    }

    #[test]
    fn prefixes_are_segment_wise() {
        let a = DocPath::parse("/media/x").unwrap();
        assert!(a.is_prefix_of(&DocPath::parse("/media/x/probe").unwrap()));
        assert!(a.is_prefix_of(&a));
        assert!(!a.is_prefix_of(&DocPath::parse("/media/xy").unwrap()));
        assert!(DocPath::root().is_prefix_of(&a));
    }

    #[test]
    fn entity_of_a_path() {
        let (s, t, i, m) = (
            SequenceId::from_u128(1),
            TrackId::from_u128(2),
            ItemId::from_u128(3),
            MediaId::from_u128(4),
        );
        assert_eq!(DocPath::meta().join("name").entity(), EntityRef::Project);
        assert_eq!(
            DocPath::media(m).join("probe").entity(),
            EntityRef::Media(m)
        );
        assert_eq!(
            DocPath::sequence(s).join("track_order").join(0).entity(),
            EntityRef::Sequence(s)
        );
        assert_eq!(
            DocPath::track(s, t).join("locked").entity(),
            EntityRef::Track {
                sequence: s,
                track: t
            }
        );
        assert_eq!(
            DocPath::track(s, t).join("items").entity(),
            EntityRef::Track {
                sequence: s,
                track: t
            }
        );
        assert_eq!(
            DocPath::item(s, t, i).join("start").entity(),
            EntityRef::Item {
                sequence: s,
                track: t,
                item: i
            }
        );
        assert_eq!(
            DocPath::parse("/sequences/nope/name").unwrap().entity(),
            EntityRef::Project
        );
        assert_eq!(
            DocPath::item(s, t, i).as_str(),
            format!("/sequences/{s}/tracks/{t}/items/{i}")
        );
    }

    #[test]
    fn serde_uses_the_string() {
        let p = DocPath::media(MediaId::from_u128(1));
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v, serde_json::json!(p.as_str()));
        assert_eq!(serde_json::from_value::<DocPath>(v).unwrap(), p);
        assert!(serde_json::from_value::<DocPath>(serde_json::json!("x")).is_err());
    }
}
