// SPDX-License-Identifier: GPL-3.0-or-later
//! The project document root (arch §4.2), schema v2: meta, settings, media, the media folders
//! (ADR-0020) and sequences, plus plugin data.

use crate::Duration;
use std::collections::BTreeMap;

use imbl::OrdMap;
use serde::{Deserialize, Serialize};

use crate::{
    CURRENT_SCHEMA_VERSION, Collection, CollectionId, Custom, Extra, FolderId, Item, ItemId,
    MediaAsset, MediaFolder, MediaId, Sequence, SequenceFormat, SequenceId, Track,
};

/// Document metadata.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, specta::Type)]
pub struct ProjectMeta {
    /// Version of the document format (`CURRENT_SCHEMA_VERSION` when saved by this build).
    /// Older documents are migrated on open; newer ones open read-only (arch §4.3).
    pub schema_version: u32,
    /// Project name.
    pub name: String,
    /// When the project was created (RFC 3339 UTC, e.g. `2026-11-22T10:00:00Z`), set by the
    /// engine (frozen under `--deterministic-ids`).
    pub created: Option<String>,
    /// Version of the Cutweave build that last saved the document.
    pub app_version: Option<String>,
    /// Fields unknown to this version.
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: Extra,
}

/// The still duration of a project that stores none: five seconds (ADR-0035).
pub const DEFAULT_STILL_DURATION_SECS: i64 = 5;
/// The longest still duration a project may store: one hour.
pub const MAX_STILL_DURATION_SECS: i64 = 3600;

/// When a project's video gets proxies automatically (ADR-0027). Proxies themselves are cache
/// state of each computer; the policy is the editor's intent and travels with the project.
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
    schemars::JsonSchema,
    specta::Type,
)]
#[serde(rename_all = "snake_case")]
pub enum ProxyPolicy {
    /// Never automatically (`media.generate_proxy` still makes one).
    Off,
    /// For video that is slow to scrub: long-GOP H.264 or HEVC larger than 1920 pixels, faster
    /// than 60 fps, or 10-bit HEVC. The default.
    #[default]
    Auto,
    /// For every video file.
    All,
}

impl ProxyPolicy {
    /// Whether it is the default, which is left out of the file.
    #[must_use]
    pub fn is_auto(&self) -> bool {
        *self == ProxyPolicy::Auto
    }

    /// Lowercase name for messages.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            ProxyPolicy::Off => "off",
            ProxyPolicy::Auto => "auto",
            ProxyPolicy::All => "all",
        }
    }
}

impl ProjectSettings {
    /// How long a still image is when it is placed without a length: the project's value, else
    /// five seconds.
    #[must_use]
    pub fn still_duration(&self) -> Duration {
        self.still_duration.unwrap_or_else(|| {
            Duration::from_secs(DEFAULT_STILL_DURATION_SECS).unwrap_or(Duration::ZERO)
        })
    }
}

/// Project-wide settings (arch §4.2 `settings`, v1 subset).
#[derive(
    Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema, specta::Type,
)]
pub struct ProjectSettings {
    /// Format of new sequences.
    pub default_format: SequenceFormat,
    /// How long a still image is when it is placed without a length (ADR-0035). Absent: five
    /// seconds ([`DEFAULT_STILL_DURATION_SECS`]); left out of the file while absent, so
    /// documents written before the field read the same. Changing it never alters clips that
    /// already exist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub still_duration: Option<Duration>,
    /// Which video gets proxies automatically (ADR-0027). `auto` is left out of the file, so
    /// documents written before the field read the same.
    #[serde(default, skip_serializing_if = "ProxyPolicy::is_auto")]
    pub proxy_policy: ProxyPolicy,
    /// Fields unknown to this version.
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: Extra,
}

/// The project document: everything saved in `<Name>.cutweave` (arch §17).
///
/// Entities are addressed by ID, never by index (arch §4.3). All collections are persistent
/// (`imbl`), so `clone()` is O(1) and snapshots share structure (ADR-0005). Mutate only through
/// commands (`cutweave-ops`); readers get immutable snapshots.
///
/// The JSON form has a stable layout (fields in declaration order, maps sorted by key), so
/// saved files diff cleanly.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, specta::Type)]
pub struct Project {
    /// Document metadata.
    pub meta: ProjectMeta,
    /// Project-wide settings.
    pub settings: ProjectSettings,
    /// Media, keyed by ID.
    #[schemars(with = "BTreeMap<MediaId, MediaAsset>")]
    #[specta(type = BTreeMap<MediaId, MediaAsset>)]
    pub media: OrdMap<MediaId, MediaAsset>,
    /// The folder tree of the media bin, keyed by ID (ADR-0020).
    #[schemars(with = "BTreeMap<FolderId, MediaFolder>")]
    #[specta(type = BTreeMap<FolderId, MediaFolder>)]
    pub folders: OrdMap<FolderId, MediaFolder>,
    /// Collections of media, keyed by ID (ADR-0036). Left out of the file while empty, so
    /// documents written before the field read the same.
    #[serde(default, skip_serializing_if = "OrdMap::is_empty")]
    #[schemars(with = "BTreeMap<CollectionId, Collection>")]
    #[specta(type = BTreeMap<CollectionId, Collection>)]
    pub collections: OrdMap<CollectionId, Collection>,
    /// Sequences, keyed by ID.
    #[schemars(with = "BTreeMap<SequenceId, Sequence>")]
    #[specta(type = BTreeMap<SequenceId, Sequence>)]
    pub sequences: OrdMap<SequenceId, Sequence>,
    /// Plugin data (arch §4.3).
    pub custom: Custom,
    /// Fields unknown to this version.
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: Extra,
}

/// Where an item lives.
#[derive(Clone, Copy, Debug)]
pub struct ItemRef<'a> {
    /// The sequence holding the item.
    pub sequence: &'a Sequence,
    /// The track holding the item.
    pub track: &'a Track,
    /// The item.
    pub item: &'a Item,
}

impl Project {
    /// An empty project named `name` with default settings.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Project {
        Project {
            meta: ProjectMeta {
                schema_version: CURRENT_SCHEMA_VERSION,
                name: name.into(),
                created: None,
                app_version: None,
                extra: Extra::new(),
            },
            settings: ProjectSettings::default(),
            media: OrdMap::new(),
            folders: OrdMap::new(),
            collections: OrdMap::new(),
            sequences: OrdMap::new(),
            custom: Custom::new(),
            extra: Extra::new(),
        }
    }

    /// The highest position of the [`SeededGen`](crate::SeededGen) stream for `seed` among the
    /// entity IDs in this document (0 when there is none): a generator resumed past it never
    /// mints an ID the document holds (arch §4.4, WP-1B.08). IDs from other seeds or real
    /// UUIDv7s are ignored.
    #[must_use]
    pub fn seeded_id_high_water(&self, seed: u64) -> u64 {
        fn visit(v: &serde_json::Value, seed: u64, best: &mut u64) {
            match v {
                serde_json::Value::String(s) => note(s, seed, best),
                serde_json::Value::Array(a) => a.iter().for_each(|v| visit(v, seed, best)),
                serde_json::Value::Object(o) => {
                    for (k, v) in o {
                        note(k, seed, best);
                        visit(v, seed, best);
                    }
                }
                _ => {}
            }
        }
        fn note(s: &str, seed: u64, best: &mut u64) {
            let hex = match s.as_bytes() {
                [_, _, _, b'_', ..] if s.len() == 36 => &s[4..],
                _ => return,
            };
            if let Ok(raw) = u128::from_str_radix(hex, 16)
                && let Some(c) = crate::SeededGen::counter_of(seed, raw)
            {
                *best = (*best).max(c);
            }
        }
        let mut best = 0;
        if let Ok(v) = serde_json::to_value(self) {
            visit(&v, seed, &mut best);
        }
        best
    }

    /// Finds an item in any sequence.
    #[must_use]
    pub fn find_item(&self, id: &ItemId) -> Option<ItemRef<'_>> {
        self.sequences.values().find_map(|sequence| {
            sequence.find_item(id).map(|(track, item)| ItemRef {
                sequence,
                track,
                item,
            })
        })
    }

    /// Total number of items on all tracks of all sequences.
    #[must_use]
    pub fn item_count(&self) -> usize {
        self.sequences
            .values()
            .flat_map(|s| s.tracks.values())
            .map(|t| t.items.len())
            .sum()
    }
}

/// An empty project named "Untitled".
impl Default for Project {
    fn default() -> Project {
        Project::new("Untitled")
    }
}
