//! A toy project document (a small subset of arch §4.2): one sequence, tracks, media, items.
//!
//! Normalized and ID-addressed. `Track.items` from the architecture is *derived* here
//! ([`Doc::track_items`]) instead of stored, so a split is 2 patches instead of 3 and there is
//! no denormalized list to keep consistent.
//!
//! Only the engine owns a `Doc` mutably; commands see it through [`crate::Tx`].

use std::collections::BTreeMap;

use serde::Serialize;

use crate::{
    ids::{IdGen, ItemId, MediaId, SequenceId, TrackId},
    time::{FrameRate, Time},
    wire,
};

/// Project metadata.
#[wire]
pub struct ProjectMeta {
    /// Display name.
    pub name: String,
}

/// The (single, in the spike) sequence.
#[wire]
pub struct Sequence {
    /// Id.
    pub id: SequenceId,
    /// Display name.
    pub name: String,
    /// Timeline frame rate; also the context for frame-based timecode inputs.
    pub frame_rate: FrameRate,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Track order, top to bottom.
    pub tracks: Vec<TrackId>,
}

/// What a media asset contains.
#[wire]
#[derive(Copy, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    /// Video (possibly with audio).
    Video,
    /// Audio only.
    Audio,
    /// Still image.
    Image,
}

/// An imported file (by reference).
#[wire]
pub struct MediaAsset {
    /// Id.
    pub id: MediaId,
    /// Path as given at import.
    pub path: String,
    /// Content kind.
    pub kind: MediaKind,
    /// Duration (stub probe).
    pub duration: Time,
    /// Native frame rate (stub probe).
    pub frame_rate: FrameRate,
}

/// Track kind.
#[wire]
#[derive(Copy, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    /// Video track.
    Video,
    /// Audio track.
    Audio,
}

/// A track.
#[wire]
pub struct Track {
    /// Id.
    pub id: TrackId,
    /// Kind.
    pub kind: TrackKind,
    /// Display name.
    pub name: String,
}

/// A media clip on a track.
#[wire]
pub struct Item {
    /// Id.
    pub id: ItemId,
    /// Owning track.
    pub track: TrackId,
    /// Source media.
    pub media: MediaId,
    /// Timeline start.
    pub start: Time,
    /// Timeline duration (> 0).
    pub duration: Time,
    /// Source time shown at `start`.
    pub source_in: Time,
}

impl Item {
    /// Timeline end (exclusive).
    #[must_use]
    pub fn end(&self) -> Time {
        self.start + self.duration
    }
}

/// The whole document. Equality is structural (used by the undo property tests).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Doc {
    /// Metadata.
    pub meta: ProjectMeta,
    /// The sequence.
    pub sequence: Sequence,
    /// Media by id.
    pub media: BTreeMap<MediaId, MediaAsset>,
    /// Tracks by id (order lives in `sequence.tracks`).
    pub tracks: BTreeMap<TrackId, Track>,
    /// Items by id.
    pub items: BTreeMap<ItemId, Item>,
}

impl Doc {
    /// A fresh project with one video track `V1`.
    pub fn new(name: &str, frame_rate: FrameRate, ids: &mut IdGen) -> Doc {
        let track = Track {
            id: ids.mint(),
            kind: TrackKind::Video,
            name: "V1".into(),
        };
        let sequence = Sequence {
            id: ids.mint(),
            name: "Main".into(),
            frame_rate,
            width: 1920,
            height: 1080,
            tracks: vec![track.id.clone()],
        };
        Doc {
            meta: ProjectMeta { name: name.into() },
            sequence,
            media: BTreeMap::new(),
            tracks: BTreeMap::from([(track.id.clone(), track)]),
            items: BTreeMap::new(),
        }
    }

    /// Items on `track`, sorted by start.
    #[must_use]
    pub fn track_items(&self, track: &TrackId) -> Vec<&Item> {
        let mut v: Vec<&Item> = self.items.values().filter(|i| &i.track == track).collect();
        v.sort_by_key(|i| (i.start, i.id.clone()));
        v
    }

    /// Whether `[start, end)` on `track` is free, ignoring the items in `except`.
    #[must_use]
    pub fn range_is_free(
        &self,
        track: &TrackId,
        start: Time,
        end: Time,
        except: &[&ItemId],
    ) -> bool {
        self.items.values().all(|i| {
            &i.track != track || except.contains(&&i.id) || i.end() <= start || i.start >= end
        })
    }

    /// Sequence duration (end of the last item).
    #[must_use]
    pub fn duration(&self) -> Time {
        self.items
            .values()
            .map(Item::end)
            .max()
            .unwrap_or(Time::ZERO)
    }

    /// Checks the model rules (arch §4.3) the toy doc can express.
    pub fn check_invariants(&self) -> Result<(), String> {
        let mut seen = std::collections::BTreeSet::new();
        for t in &self.sequence.tracks {
            if !self.tracks.contains_key(t) {
                return Err(format!("sequence lists unknown track {t}"));
            }
            if !seen.insert(t) {
                return Err(format!("track {t} listed twice"));
            }
        }
        if seen.len() != self.tracks.len() {
            return Err("a track is missing from sequence.tracks".into());
        }
        for (id, item) in &self.items {
            if id != &item.id {
                return Err(format!("item key {id} != id {}", item.id));
            }
            let media = self
                .media
                .get(&item.media)
                .ok_or_else(|| format!("item {id} references missing media"))?;
            if !self.tracks.contains_key(&item.track) {
                return Err(format!("item {id} on missing track"));
            }
            if item.duration <= Time::ZERO || item.start < Time::ZERO || item.source_in < Time::ZERO
            {
                return Err(format!("item {id} has a negative or empty range"));
            }
            if item.source_in + item.duration > media.duration {
                return Err(format!("item {id} runs past the end of its media"));
            }
        }
        for track in self.tracks.keys() {
            for pair in self.track_items(track).windows(2) {
                if pair[0].end() > pair[1].start {
                    return Err(format!(
                        "overlap on {track}: {} / {}",
                        pair[0].id, pair[1].id
                    ));
                }
            }
        }
        Ok(())
    }
}
