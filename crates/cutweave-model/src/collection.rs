// SPDX-License-Identifier: GPL-3.0-or-later
//! Tags, ratings and notes on media, and collections (ADR-0036).
//!
//! - **Tags** ([`MediaAsset::tags`](crate::MediaAsset)): words people type. Two tags are the
//!   same when their [`tag_key`]s are equal (case and accents ignored, the fold of media
//!   search: [`fold`]); an asset stores them as first typed, sorted by key, without repeats.
//!   There is no tag registry: a project's tags are whatever its assets carry.
//! - **Rating** (1 to 5, absent: unrated) and **note** (plain text, [`note_problem`]).
//! - **Collections** ([`Collection`], `Project::collections`): a list an asset can be on
//!   without leaving its folder. A manual one stores its members in the order they were added;
//!   a smart one stores a [`MediaFilter`] and its members are computed when asked. A stored
//!   member that names no asset is allowed (an older build, or a trimmed copy, may have removed
//!   the asset): queries skip it and the next edit of the collection drops it.
//!
//! Every field is optional and left out of the file while empty, so documents written before
//! them read the same and the schema version did not change.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

use crate::{CollectionId, Custom, Duration, Extra, FolderId, MediaId, MediaKind};

/// Longest tag, in characters.
pub const MAX_TAG_CHARS: usize = 64;
/// Most tags on one asset.
pub const MAX_TAGS_PER_ASSET: usize = 64;
/// Longest note, in characters.
pub const MAX_NOTE_CHARS: usize = 2000;
/// The ratings an asset can have (whole stars).
pub const RATINGS: std::ops::RangeInclusive<u8> = 1..=5;
/// Longest collection name, in characters.
pub const MAX_COLLECTION_NAME_CHARS: usize = 255;
/// Most collections in a project.
pub const MAX_COLLECTIONS: usize = 1000;
/// Most members of a manual collection.
pub const MAX_COLLECTION_MEMBERS: usize = 10_000;
/// Longest `MediaFilter::text`, in characters.
pub const MAX_FILTER_TEXT_CHARS: usize = 256;
/// Most tags one `MediaFilter` names.
pub const MAX_FILTER_TAGS: usize = 16;

/// The text of `s` as media search and tag comparison see it: accents removed, lower case
/// (NFD, combining marks dropped, Unicode lower case). Plain ASCII that is already lower case
/// is borrowed, so the common case allocates nothing.
#[must_use]
pub fn fold(s: &str) -> Cow<'_, str> {
    if s.is_ascii() {
        return if s.bytes().any(|b| b.is_ascii_uppercase()) {
            Cow::Owned(s.to_ascii_lowercase())
        } else {
            Cow::Borrowed(s)
        };
    }
    Cow::Owned(
        s.nfd()
            .filter(|c| !is_combining_mark(*c))
            .collect::<String>()
            .to_lowercase(),
    )
}

/// The form tags are compared and sorted in: two tags are the same when their keys are equal
/// (`Café` and `cafe` are).
#[must_use]
pub fn tag_key(tag: &str) -> Cow<'_, str> {
    fold(tag)
}

/// Why `tag` cannot be a stored tag, or `None` if it can: 1 to [`MAX_TAG_CHARS`] characters
/// with no space around it, no comma and no control characters.
#[must_use]
pub fn tag_problem(tag: &str) -> Option<&'static str> {
    if tag.trim().is_empty() {
        Some("a tag must not be empty")
    } else if tag.trim() != tag {
        Some("a tag must not start or end with a space")
    } else if tag.chars().count() > MAX_TAG_CHARS {
        Some("a tag must be at most 64 characters")
    } else if tag.contains(',') {
        Some("a tag must not contain a comma")
    } else if tag.chars().any(char::is_control) {
        Some("a tag must not contain control characters")
    } else {
        None
    }
}

/// Why `tags` cannot be an asset's stored tags, or `None` if they can: each passes
/// [`tag_problem`], there are at most [`MAX_TAGS_PER_ASSET`], and they are sorted by
/// [`tag_key`] without repeats.
#[must_use]
pub fn tags_problem(tags: &[String]) -> Option<&'static str> {
    if tags.len() > MAX_TAGS_PER_ASSET {
        return Some("an asset has at most 64 tags");
    }
    if let Some(problem) = tags.iter().find_map(|t| tag_problem(t)) {
        return Some(problem);
    }
    let sorted = tags
        .windows(2)
        .all(|pair| tag_key(&pair[0]) < tag_key(&pair[1]));
    if sorted {
        None
    } else {
        Some("tags must be sorted ignoring case and accents, without repeats")
    }
}

/// Why `note` cannot be a stored note, or `None` if it can: it holds a visible character, is at
/// most [`MAX_NOTE_CHARS`] characters and has no control characters other than line breaks and
/// tabs.
#[must_use]
pub fn note_problem(note: &str) -> Option<&'static str> {
    if note.trim().is_empty() {
        Some("a note must not be empty (leave it out instead)")
    } else if note.chars().count() > MAX_NOTE_CHARS {
        Some("a note must be at most 2000 characters")
    } else if note
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        Some("a note must not contain control characters other than line breaks and tabs")
    } else {
        None
    }
}

/// Why `name` cannot name a collection, or `None` if it can: it holds a visible character, is
/// at most [`MAX_COLLECTION_NAME_CHARS`] characters and has no control characters.
#[must_use]
pub fn collection_name_problem(name: &str) -> Option<&'static str> {
    if name.trim().is_empty() {
        Some("a collection name must not be empty")
    } else if name.chars().count() > MAX_COLLECTION_NAME_CHARS {
        Some("a collection name must be at most 255 characters")
    } else if name.chars().any(char::is_control) {
        Some("a collection name must not contain control characters")
    } else {
        None
    }
}

/// The form collection names are compared in: two names clash when their keys are equal
/// (Unicode lower case, as folder names).
#[must_use]
pub fn collection_name_key(name: &str) -> String {
    name.to_lowercase()
}

/// Which media to list; every part that is absent or empty lets everything through. The filter
/// of `media.list`, and what a smart collection stores (ADR-0036).
#[derive(
    Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema, specta::Type,
)]
#[serde(deny_unknown_fields)]
pub struct MediaFilter {
    /// Terms that must all occur in the name, tags or note, ignoring case and accents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Only these kinds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<MediaKind>>,
    /// Shortest length (flicks), inclusive; stills never match a bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_duration: Option<Duration>,
    /// Longest length (flicks), inclusive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_duration: Option<Duration>,
    /// Placed on a timeline of any sequence (`true`) or not (`false`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used: Option<bool>,
    /// `true`: only files nothing can be read from (no streams).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problems: Option<bool>,
    // Without `folder`, `recursive: false` lists the top level of the bin (the default lists
    // the whole project); with one, the default is that folder only (ADR-0020). The doc
    // comments are short: they count against the core MCP tool budget.
    /// Only this bin folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<FolderId>,
    /// With subfolders (default: true without folder).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recursive: Option<bool>,
    // Tags compare by `tag_key`; at most `MAX_FILTER_TAGS`. A smart collection's own filter
    // may not name a collection (ADR-0036).
    /// Has all these tags.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    /// Rated at least this (1-5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1, max = 5))]
    pub min_rating: Option<u8>,
    /// On this collection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection: Option<CollectionId>,
}

impl MediaFilter {
    /// Why the filter cannot be used, as the JSON Pointer of the offending part (inside the
    /// filter) and the reason, or `None` if it can: the text is at most
    /// [`MAX_FILTER_TEXT_CHARS`] characters, the duration bounds are in order, there are at
    /// most [`MAX_FILTER_TAGS`] tags and each passes [`tag_problem`] after trimming, and the
    /// rating is one of [`RATINGS`]. Whether its folder and collection exist is the caller's
    /// to decide.
    #[must_use]
    pub fn problem(&self) -> Option<(String, &'static str)> {
        if self
            .text
            .as_deref()
            .is_some_and(|t| t.chars().count() > MAX_FILTER_TEXT_CHARS)
        {
            return Some(("/text".into(), "the text is at most 256 characters"));
        }
        if let (Some(min), Some(max)) = (self.min_duration, self.max_duration)
            && min > max
        {
            return Some((
                "/min_duration".into(),
                "min_duration is longer than max_duration",
            ));
        }
        if let Some(tags) = &self.tags {
            if tags.len() > MAX_FILTER_TAGS {
                return Some(("/tags".into(), "a filter names at most 16 tags"));
            }
            for (i, tag) in tags.iter().enumerate() {
                if let Some(problem) = tag_problem(tag.trim()) {
                    return Some((format!("/tags/{i}"), problem));
                }
            }
        }
        if self.min_rating.is_some_and(|r| !RATINGS.contains(&r)) {
            return Some(("/min_rating".into(), "the rating must be 1 to 5"));
        }
        None
    }
}

/// What a collection holds (ADR-0036).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, specta::Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CollectionKind {
    /// A list of assets, kept in the order they were added.
    Manual {
        /// The members, without repeats. One that names no asset is skipped by queries and
        /// dropped by the next edit of the collection.
        #[serde(default)]
        media: Vec<MediaId>,
    },
    /// A saved search: the assets that pass the filter, computed when asked.
    Smart {
        /// The filter. It may not name a collection.
        filter: MediaFilter,
    },
}

impl CollectionKind {
    /// An empty manual collection.
    #[must_use]
    pub fn manual() -> CollectionKind {
        CollectionKind::Manual { media: Vec::new() }
    }

    /// `"manual"` or `"smart"`.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            CollectionKind::Manual { .. } => "manual",
            CollectionKind::Smart { .. } => "smart",
        }
    }
}

/// A collection of media (ADR-0036): a named list an asset can be on without leaving its
/// folder.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, specta::Type)]
pub struct Collection {
    /// The collection ID (also its key in `Project::collections`).
    pub id: CollectionId,
    /// The name shown in the bin: 1 to 255 characters without control characters, unique
    /// among collections ignoring case.
    pub name: String,
    /// Manual (stored members) or smart (a stored filter).
    pub kind: CollectionKind,
    /// Plugin data (arch §4.3).
    pub custom: Custom,
    /// Fields unknown to this version.
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: Extra,
}

impl Collection {
    /// A collection named `name`.
    #[must_use]
    pub fn new(id: CollectionId, name: impl Into<String>, kind: CollectionKind) -> Collection {
        Collection {
            id,
            name: name.into(),
            kind,
            custom: Custom::new(),
            extra: Extra::new(),
        }
    }

    /// The stored members of a manual collection (empty for a smart one).
    #[must_use]
    pub fn manual_members(&self) -> &[MediaId] {
        match &self.kind {
            CollectionKind::Manual { media } => media,
            CollectionKind::Smart { .. } => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folding_ignores_case_and_accents() {
        assert_eq!(fold("ÉCLAIR Ñandú"), "eclair nandu");
        assert!(matches!(fold("plain"), Cow::Borrowed(_)));
        assert_eq!(tag_key("Café"), tag_key("cafe"));
        assert_ne!(tag_key("b-roll"), tag_key("broll"));
    }

    #[test]
    fn tags() {
        assert_eq!(tag_problem("interview"), None);
        assert_eq!(tag_problem("Day 1"), None);
        assert_eq!(tag_problem(&"é".repeat(64)), None);
        assert!(tag_problem("").is_some());
        assert!(tag_problem(" x").is_some());
        assert!(tag_problem("a,b").is_some());
        assert!(tag_problem("a\tb").is_some());
        assert!(tag_problem(&"x".repeat(65)).is_some());
        let list = |tags: &[&str]| tags.iter().map(|t| (*t).to_owned()).collect::<Vec<_>>();
        assert_eq!(tags_problem(&list(&[])), None);
        assert_eq!(tags_problem(&list(&["b-roll", "Café", "interview"])), None);
        assert!(tags_problem(&list(&["interview", "b-roll"])).is_some());
        assert!(tags_problem(&list(&["cafe", "Café"])).is_some());
        let many: Vec<String> = (0..65).map(|n| format!("t{n:03}")).collect();
        assert!(tags_problem(&many).is_some());
        assert_eq!(tags_problem(&many[..64]), None);
    }

    #[test]
    fn notes_and_collection_names() {
        assert_eq!(note_problem("Good take.\nUse the second half."), None);
        assert!(note_problem("  ").is_some());
        assert!(note_problem("a\u{7}b").is_some());
        assert!(note_problem(&"x".repeat(2001)).is_some());
        assert_eq!(note_problem(&"é".repeat(2000)), None);
        assert_eq!(collection_name_problem("Best takes / day 1"), None);
        assert!(collection_name_problem("").is_some());
        assert!(collection_name_problem("a\nb").is_some());
        assert!(collection_name_problem(&"x".repeat(256)).is_some());
        assert_eq!(
            collection_name_key("Selects"),
            collection_name_key("SELECTS")
        );
    }

    #[test]
    fn filter_problems_point_at_the_part() {
        let ok = MediaFilter {
            tags: Some(vec![" interview ".into()]),
            min_rating: Some(5),
            ..MediaFilter::default()
        };
        assert_eq!(ok.problem(), None);
        let at = |f: MediaFilter| f.problem().map(|(p, _)| p);
        assert_eq!(
            at(MediaFilter {
                text: Some("x".repeat(257)),
                ..MediaFilter::default()
            })
            .as_deref(),
            Some("/text")
        );
        assert_eq!(
            at(MediaFilter {
                tags: Some(vec!["a".into(), "b,c".into()]),
                ..MediaFilter::default()
            })
            .as_deref(),
            Some("/tags/1")
        );
        assert_eq!(
            at(MediaFilter {
                tags: Some(vec!["t".into(); 17]),
                ..MediaFilter::default()
            })
            .as_deref(),
            Some("/tags")
        );
        assert_eq!(
            at(MediaFilter {
                min_rating: Some(0),
                ..MediaFilter::default()
            })
            .as_deref(),
            Some("/min_rating")
        );
    }

    #[test]
    fn a_filter_is_stored_without_its_absent_parts() {
        let filter = MediaFilter {
            kinds: Some(vec![MediaKind::Video]),
            used: Some(false),
            ..MediaFilter::default()
        };
        let json = serde_json::to_value(&filter).unwrap();
        assert_eq!(json, serde_json::json!({"kinds": ["video"], "used": false}));
        assert_eq!(serde_json::from_value::<MediaFilter>(json).unwrap(), filter);
        assert!(serde_json::from_str::<MediaFilter>(r#"{"imported_within_days": 3}"#).is_err());
    }
}
