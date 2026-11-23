// SPDX-License-Identifier: GPL-3.0-or-later
//! The project document root (arch §4.2), restricted to schema v1: meta, settings, media and
//! sequences, plus plugin data.

use std::collections::BTreeMap;

use imbl::OrdMap;
use serde::{Deserialize, Serialize};

use crate::{
    CURRENT_SCHEMA_VERSION, Custom, Extra, Item, ItemId, MediaAsset, MediaId, Sequence,
    SequenceFormat, SequenceId, Track,
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

/// Project-wide settings (arch §4.2 `settings`, v1 subset).
#[derive(
    Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema, specta::Type,
)]
pub struct ProjectSettings {
    /// Format of new sequences.
    pub default_format: SequenceFormat,
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
