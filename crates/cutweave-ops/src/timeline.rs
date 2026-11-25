// SPDX-License-Identifier: GPL-3.0-or-later
//! The `timeline` namespace (WP-1B.05, WP-2A.02, WP-2A.03; arch §4.3, §5.1; timeline spec §4):
//! `timeline.insert` and `timeline.overwrite` ([`insert`]), `timeline.lift`, `timeline.extract`,
//! `timeline.ripple_delete` and `timeline.delete_gap` ([`edit`]), the marker and in/out commands
//! ([`markers`]) and the `timeline.get` query.
//!
//! Every placement command computes its result with the shared placement core
//! ([`placement`]: ripple, cut, split, placement modes, locks, links and attachments in one
//! place) and the magnetic main track's rules ([`magnetic`]), and returns an
//! [`EditReport`](crate::report::EditReport) of exactly what changed.

use cutweave_commands::{CmdError, Query, QueryCtx, Registry, RegistryError, query, wire};
use cutweave_model::{
    Duration, GroupId, Item, ItemId, MediaId, SequenceFormat, SequenceId, Time, TrackId, TrackKind,
};

/// Length of a still image when `duration` is omitted: five seconds (snapped to the nearest
/// frame, INV-25).
pub const DEFAULT_STILL_SECS: i64 = 5;

pub mod edit;
mod insert;
pub(crate) mod magnetic;
pub mod markers;
pub mod placement;

pub use edit::{
    DeletedOutput, TimelineDeleteGap, TimelineDeleteGapOutput, TimelineExtract, TimelineLift,
    TimelineRange, TimelineRangesOutput, TimelineRippleDelete, UsedRange,
};
pub use insert::{TimelineInsert, TimelineInsertOutput, TimelineOverwrite};
pub use placement::PlacementMode;

/// Registers the `timeline.*` commands and queries.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    registry
        .register::<TimelineInsert>()?
        .register::<TimelineOverwrite>()?
        .register::<TimelineLift>()?
        .register::<TimelineExtract>()?
        .register::<TimelineRippleDelete>()?
        .register::<TimelineDeleteGap>()?
        .register::<TimelineGet>()?;
    markers::register(registry)
}

/// What to do when an item would overlap another one on its target track: the P1 `overlap`
/// param, an alias of the placement `mode` (spec INV-40).
#[wire]
#[derive(Copy, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum OverlapPolicy {
    /// Fail with `Invalid` (default on free tracks).
    #[default]
    Reject,
    /// Create a new track of the same kind directly above the target and place there.
    NewTrack,
}

// ----- timeline.get --------------------------------------------------------------------------

/// Deepest `depth` of `timeline.get`.
pub const MAX_GET_DEPTH: u8 = 2;

/// One item of a track (depth 2).
#[wire]
pub struct ItemSummary {
    /// The item.
    pub id: ItemId,
    /// Item kind (`clip`).
    pub kind: String,
    /// Timeline position of the first frame.
    pub start: Time,
    /// Length on the timeline.
    pub duration: Duration,
    /// The media a clip plays.
    pub media: Option<MediaId>,
    /// The linked group, if any.
    pub linked_group: Option<GroupId>,
    /// Whether the item is rendered and heard.
    pub enabled: bool,
}

/// One track (depth 1 and 2).
#[wire]
#[allow(clippy::struct_excessive_bools)] // mirrors the track's independent toggles
pub struct TrackSummary {
    /// The track.
    pub id: TrackId,
    /// What it holds.
    pub kind: TrackKind,
    /// Display name.
    pub name: String,
    /// Silenced.
    pub muted: bool,
    /// Not rendered.
    pub hidden: bool,
    /// Item edits fail with `Locked`.
    pub locked: bool,
    /// Solo.
    pub solo: bool,
    /// The magnetic main track.
    pub magnetic: bool,
    /// Row height in pixels; `None` is the default.
    pub height: Option<u32>,
    /// Gain in dB.
    pub gain: f64,
    /// Number of items.
    pub item_count: u32,
    /// End of its last item.
    pub end: Time,
    /// The items in start order (depth 2 only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<Vec<ItemSummary>>,
}

/// Result of `timeline.get`.
#[wire]
pub struct TimelineGetOutput {
    /// The sequence.
    pub id: SequenceId,
    /// Its name (unless `fields` leaves it out).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Its format (unless `fields` leaves it out).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<SequenceFormat>,
    /// End of the last item on any track (unless `fields` leaves it out).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<Duration>,
    /// Number of tracks.
    pub track_count: u32,
    /// Number of items on all tracks.
    pub item_count: u32,
    /// The tracks in display order, top to bottom (depth >= 1, unless `fields` leaves them
    /// out).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tracks: Option<Vec<TrackSummary>>,
}

/// Sections `timeline.get` can be limited to with `fields`.
pub const GET_FIELDS: [&str; 4] = ["name", "format", "duration", "tracks"];

/// Reads a sequence in layers: `depth` 0 is the sequence summary, 1 adds its tracks (summaries
/// only, small), 2 adds every item. `fields` limits the sections returned (`name`, `format`,
/// `duration`, `tracks`); the id and the counts are always included.
#[query(
    name = "timeline.get",
    summary = "Get a sequence: summary (depth 0), tracks (1) or tracks with items (2)",
    mcp = "core"
)]
pub struct TimelineGet {
    /// The sequence.
    pub sequence: SequenceId,
    /// Detail level `0..=2` (default 1).
    pub depth: Option<u8>,
    /// Sections to include: any of `name`, `format`, `duration`, `tracks` (default all).
    pub fields: Option<Vec<String>>,
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

pub(crate) fn item_summary(item: &Item) -> ItemSummary {
    ItemSummary {
        id: item.id(),
        kind: match item {
            Item::Clip(_) => "clip",
            _ => "other",
        }
        .to_owned(),
        start: item.start(),
        duration: item.duration(),
        media: item.media(),
        linked_group: item.linked_group(),
        enabled: item.enabled(),
    }
}

impl Query for TimelineGet {
    type Output = TimelineGetOutput;

    fn run(&self, ctx: &QueryCtx<'_>) -> Result<TimelineGetOutput, CmdError> {
        let sequence = ctx.sequence(self.sequence)?;
        let depth = self.depth.unwrap_or(1);
        if depth > MAX_GET_DEPTH {
            return Err(CmdError::invalid(
                "/depth",
                format!("the depth must be 0..={MAX_GET_DEPTH}"),
            ));
        }
        let wants = |name: &str| {
            self.fields
                .as_ref()
                .is_none_or(|f| f.iter().any(|x| x == name))
        };
        if let Some(fields) = &self.fields {
            for (i, f) in fields.iter().enumerate() {
                if !GET_FIELDS.contains(&f.as_str()) {
                    return Err(CmdError::invalid(
                        format!("/fields/{i}"),
                        format!("unknown field `{f}`; use {}", GET_FIELDS.join(", ")),
                    ));
                }
            }
        }
        let tracks = (depth >= 1 && wants("tracks")).then(|| {
            sequence
                .tracks_in_order()
                .map(|t| TrackSummary {
                    id: t.id,
                    kind: t.kind,
                    name: t.name.clone(),
                    muted: t.muted,
                    hidden: t.hidden,
                    locked: t.locked,
                    solo: t.solo,
                    magnetic: t.magnetic,
                    height: t.height,
                    gain: t.gain,
                    item_count: count(t.items.len()),
                    end: t.items.end(),
                    items: (depth >= 2).then(|| t.items.iter().map(item_summary).collect()),
                })
                .collect()
        });
        Ok(TimelineGetOutput {
            id: sequence.id,
            name: wants("name").then(|| sequence.name.clone()),
            format: wants("format").then(|| sequence.format.clone()),
            duration: wants("duration").then(|| sequence.duration()),
            track_count: count(sequence.tracks.len()),
            item_count: count(sequence.tracks.values().map(|t| t.items.len()).sum()),
            tracks,
        })
    }
}
