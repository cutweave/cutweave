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
    Color, Duration, GroupId, Item, ItemId, MediaId, Sequence, SequenceFormat, SequenceId, Time,
    TimeInput, TrackId, TrackKind,
};

/// Length of a still image when `duration` is omitted and the project stores no still
/// duration: five seconds (ADR-0035).
pub const DEFAULT_STILL_SECS: i64 = cutweave_model::DEFAULT_STILL_DURATION_SECS;

/// The frames a still gets when it is placed without a length: the project's still duration
/// (`settings.still_duration`, ADR-0035) snapped to the nearest frame of `rate`, at least one
/// frame (INV-25).
pub(crate) fn default_still_frames(
    doc: &cutweave_model::Project,
    rate: cutweave_model::FrameRate,
) -> Result<i64, cutweave_commands::CmdError> {
    let length = cutweave_model::Time::from_flicks(doc.settings.still_duration().flicks())
        .map_err(|e| cutweave_commands::CmdError::internal(e.to_string()))?;
    Ok(rate
        .time_to_frame(length, cutweave_model::Rounding::Nearest)
        .max(1))
}

pub mod clipboard;
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
        .register::<clipboard::TimelineCopy>()?
        .register::<clipboard::TimelinePaste>()?
        .register::<TimelineGet>()?;
    markers::register(registry)
}

/// What to do when an item would overlap another one on its target track: the P1 `overlap`
/// param, an alias of the placement `mode` (spec INV-40). `reject` fails with `Invalid`
/// (default on free tracks); `new_track` places on a new track of the same kind directly above
/// the target.
// Variants documented above: a plain string `enum` keeps the MCP core tools small (R33).
#[allow(missing_docs)]
#[wire]
#[derive(Copy, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum OverlapPolicy {
    #[default]
    Reject,
    NewTrack,
}

// ----- timeline.get --------------------------------------------------------------------------

/// Deepest `depth` of `timeline.get`.
pub const MAX_GET_DEPTH: u8 = 2;

/// Largest `limit` of `timeline.get`.
pub const MAX_GET_LIMIT: u32 = 1000;

/// One item of a track (depth 2).
#[wire]
pub struct ItemSummary {
    /// The item.
    pub id: ItemId,
    /// Its own name, when it has one (`item.set_props`); otherwise its media's name is shown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
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
    /// The color label, `#rrggbb`, when it has one (`item.set_label_color`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_label: Option<Color>,
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
    /// More items follow: pass it as `cursor` for the next page (only with `limit`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<ItemId>,
}

/// The part of the timeline whose items `timeline.get` lists: `[from, to)`.
#[wire]
pub struct GetRange {
    /// Start (default 0).
    pub from: Option<TimeInput>,
    /// End, exclusive (default: the sequence end).
    pub to: Option<TimeInput>,
}

/// Sections `timeline.get` can be limited to with `fields`.
pub const GET_FIELDS: [&str; 4] = ["name", "format", "duration", "tracks"];

/// Reads a sequence in layers: `depth` 0 is the sequence summary, 1 adds its tracks (summaries
/// only, small), 2 adds the items. `fields` limits the sections returned (`name`, `format`,
/// `duration`, `tracks`); the id and the counts are always included and always describe the
/// whole sequence. At depth 2, `range` lists only the items that overlap `[from, to)`, and
/// `limit` pages them in track order, then start order: `next_cursor` is the `cursor` of the
/// next page (absent on the last one). A long timeline is best read as the depth 1 overview,
/// then depth 2 pages of the part that matters.
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
    /// Depth 2: list only the items that overlap `{from, to}` (flicks, "2.5s" or "f:60").
    pub range: Option<GetRange>,
    /// Depth 2: most items to list, `1..=1000` (default: all); see `next_cursor`.
    pub limit: Option<u32>,
    /// Depth 2 with `limit`: the previous page's `next_cursor`.
    pub cursor: Option<ItemId>,
}

/// The items `timeline.get` lists at depth 2: which of them, and how many.
struct ItemPage {
    /// `[from, to)` in flicks.
    range: Option<(i64, i64)>,
    limit: Option<usize>,
    cursor: Option<ItemId>,
}

/// The listed items of each track, in track order, and the next page's cursor.
type Picked = (Vec<Vec<ItemSummary>>, Option<ItemId>);

impl ItemPage {
    fn of(get: &TimelineGet, sequence: &Sequence, depth: u8) -> Result<ItemPage, CmdError> {
        if depth < 2 {
            for (given, pointer) in [
                (get.range.is_some(), "/range"),
                (get.limit.is_some(), "/limit"),
                (get.cursor.is_some(), "/cursor"),
            ] {
                if given {
                    return Err(CmdError::invalid(
                        pointer,
                        "it selects items, which only depth 2 lists",
                    ));
                }
            }
        }
        let rate = sequence.format.frame_rate;
        let range = match &get.range {
            None => None,
            Some(range) => {
                let from = match range.from {
                    Some(t) => crate::util::resolve_time("/range/from", t, rate)?.flicks(),
                    None => 0,
                };
                let to = match range.to {
                    Some(t) => crate::util::resolve_time("/range/to", t, rate)?.flicks(),
                    None => i64::MAX,
                };
                if to <= from {
                    return Err(CmdError::invalid(
                        "/range/to",
                        "the range must end after it starts",
                    ));
                }
                Some((from, to))
            }
        };
        let limit = match get.limit {
            None => None,
            Some(n) if (1..=MAX_GET_LIMIT).contains(&n) => usize::try_from(n).ok(),
            Some(_) => {
                return Err(CmdError::invalid(
                    "/limit",
                    format!("the limit must be 1..={MAX_GET_LIMIT}"),
                ));
            }
        };
        if get.cursor.is_some() && limit.is_none() {
            return Err(CmdError::invalid(
                "/cursor",
                "a cursor continues a paged read: pass the same `limit` again",
            ));
        }
        Ok(ItemPage {
            range,
            limit,
            cursor: get.cursor,
        })
    }

    fn overlaps(&self, item: &Item) -> bool {
        self.range.is_none_or(|(from, to)| {
            let start = item.start().flicks();
            let end = start.saturating_add(item.duration().flicks());
            start < to && end > from
        })
    }

    fn pick(&self, sequence: &Sequence) -> Result<Picked, CmdError> {
        // Nothing is listed up to the cursor's item: it is the last one of the previous page.
        let mut waiting = self.cursor;
        let mut left = self.limit.unwrap_or(usize::MAX);
        let mut last = None;
        let mut next_cursor = None;
        let mut tracks = Vec::new();
        for track in sequence.tracks_in_order() {
            let mut items = Vec::new();
            for item in track.items.iter().filter(|i| self.overlaps(i)) {
                if let Some(cursor) = waiting {
                    if item.id() == cursor {
                        waiting = None;
                    }
                    continue;
                }
                if left == 0 {
                    next_cursor = next_cursor.or(last);
                    break;
                }
                left -= 1;
                last = Some(item.id());
                items.push(item_summary(item));
            }
            tracks.push(items);
        }
        if waiting.is_some() {
            return Err(CmdError::invalid(
                "/cursor",
                "no listed item has this id any more (the timeline changed): read again \
                 without a cursor",
            ));
        }
        Ok((tracks, next_cursor))
    }
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

pub(crate) fn item_summary(item: &Item) -> ItemSummary {
    ItemSummary {
        id: item.id(),
        name: match item {
            Item::Clip(c) => c.name.clone(),
            _ => None,
        },
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
        color_label: item.color_label(),
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
        let page = ItemPage::of(self, sequence, depth)?;
        let (listed, next_cursor) = if depth >= 2 && wants("tracks") {
            page.pick(sequence)?
        } else {
            (Vec::new(), None)
        };
        let mut listed = listed.into_iter();
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
                    items: (depth >= 2).then(|| listed.next().unwrap_or_default()),
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
            next_cursor,
        })
    }
}
