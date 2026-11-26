// SPDX-License-Identifier: GPL-3.0-or-later
//! `timeline.copy` and `timeline.paste` (WP-2A.06; timeline spec §4.8, §3.6; ADR-0023): the
//! clipboard of the timeline. `timeline.copy` is a pure query that serializes items into a
//! [`ClipboardPayload`] (relative layout: starts relative to the earliest item, relative track
//! rows, links). The editor keeps the payload as `application/x-cutweave-items+json` on the
//! system clipboard; an agent passes it back to `timeline.paste`.
//!
//! **Copy** takes the given items and their linked partners, like `item.duplicate`. Items keep
//! their ID, group and attachment inside the payload; `paste` remaps them.
//!
//! **Paste** gives every item and group a new ID, remaps links (a group with fewer than two
//! members in the payload is dropped), and places the items like a duplicate does: each on the
//! track its payload row maps to, at `at` plus its relative start, with the placement `mode`
//! (default `insert` when something lands on the magnetic main track, else `reject`). Items
//! that land on the magnetic track form one block from `at` (clamped into the track, INV-41).
//! A copied child stays attached to the copy of its parent; with its parent not in the payload
//! it keeps its original parent only if that is in the target sequence and the copy starts
//! inside it, and is detached otherwise (INV-9). Media references are kept; a media that is not
//! in the project is `NotFound`. Pasting into a sequence of another frame rate snaps starts and
//! durations to the target's frame grid.
//!
//! **Tracks.** The payload lists the source tracks in display order with their kind and row (the
//! distance in display order from the top-most one). `track` is where the top-most payload track
//! goes; the others keep their rows relative to it and must be tracks of the same kind
//! (`Invalid{path:"/track"}` otherwise). Without `track` the items go back to their source
//! tracks, which must still exist.
//!
//! **IDs** (INV-23): per payload item in payload order, the new group of its source's group
//! (first time only), the item, then its markers (by marker ID); then the placement's splits,
//! cuts and tracks.

use std::collections::{BTreeMap, BTreeSet};

use cutweave_commands::{CmdError, Command, Query, QueryCtx, Tx, command, query, wire};
use cutweave_model::{
    Duration, EntityKind, FrameRate, GroupId, Item, ItemId, Project, Rounding, SequenceId, Time,
    TimeInput, TrackId, TrackKind,
};

use super::{
    edit::given_items,
    magnetic::{BlockItem, Unit, clamp_to_end, pack},
    placement::{DryIds, Edit, Ids, Landing, Plan, dur, resolve_mode, time},
};
use crate::{
    item::{DuplicatedItem, remint},
    report::EditReport,
    timeline::PlacementMode,
    track::check_editable_kind,
    util::resolve_frame_time,
};

/// The payload version this build writes and reads.
pub const CLIPBOARD_VERSION: u32 = 1;

/// One track of a payload: the kind it had and where it was.
#[wire]
pub struct ClipboardTrack {
    /// What the track holds; pasted items go only to tracks of this kind.
    pub kind: TrackKind,
    /// Display-order distance from the top-most track of the payload (0 for the first).
    pub row: u32,
    /// The track the items were copied from (where `timeline.paste` goes without `track`).
    pub source: TrackId,
}

/// One copied item.
#[wire]
pub struct ClipboardItem {
    /// Index into the payload's `tracks`.
    pub track: u32,
    /// The item as copied, with its `start` relative to the earliest copied item. Its `id`,
    /// `linked_group` and `attached_to` only relate the items of the payload to each other.
    pub item: Item,
}

/// Copied items, as `application/x-cutweave-items+json` on the system clipboard (arch §5;
/// WP-2A.06). Version 1.
#[wire]
pub struct ClipboardPayload {
    /// Always 1.
    pub version: u32,
    /// The sequence the items came from.
    pub sequence: SequenceId,
    /// Its frame rate: pasting into a sequence with another one snaps to the frame grid.
    pub frame_rate: FrameRate,
    /// Length of the copied range: the latest end minus the earliest start.
    pub span: Duration,
    /// The source tracks in display order, top first.
    pub tracks: Vec<ClipboardTrack>,
    /// The items, by source track (display order), then start, then ID.
    pub items: Vec<ClipboardItem>,
}

// ----- timeline.copy -----------------------------------------------------------------------

/// Result of `timeline.copy`.
#[wire]
pub struct TimelineCopyOutput {
    /// The payload to paste.
    pub clipboard: ClipboardPayload,
}

/// Reads items and their linked partners into a clipboard payload (nothing changes).
/// `NotFound` for an unknown item; `Invalid{path:"/items"}` for none or items of several
/// sequences.
#[query(
    name = "timeline.copy",
    summary = "Copy items and their linked partners into a clipboard payload",
    mcp = "catalog"
)]
pub struct TimelineCopy {
    /// The items to copy, at least one, of one sequence.
    pub items: Vec<ItemId>,
}

impl Query for TimelineCopy {
    type Output = TimelineCopyOutput;

    fn run(&self, ctx: &QueryCtx<'_>) -> Result<TimelineCopyOutput, CmdError> {
        let doc = ctx.doc();
        let (sequence, given) = given_items(doc, &self.items)?;
        let seq = doc
            .sequences
            .get(&sequence)
            .ok_or_else(|| CmdError::not_found(EntityKind::Sequence, sequence))?;
        let wanted: BTreeSet<ItemId> = given.iter().copied().collect();
        let groups: BTreeSet<GroupId> = given
            .iter()
            .filter_map(|id| seq.find_item(id).and_then(|(_, i)| i.linked_group()))
            .collect();
        // (display index, item) of everything copied.
        let mut found: Vec<(usize, &Item)> = Vec::new();
        for (index, track) in seq.tracks_in_order().enumerate() {
            for (_, item) in track.items.iter_by_id() {
                let by_group = item.linked_group().is_some_and(|g| groups.contains(&g));
                if wanted.contains(&item.id()) || by_group {
                    check_editable_kind("/items", track.kind)?;
                    found.push((index, item));
                }
            }
        }
        found.sort_by_key(|(index, item)| (*index, item.start(), item.id()));
        let origin = found
            .iter()
            .map(|(_, i)| i.start().flicks())
            .min()
            .unwrap_or(0);
        let end = found
            .iter()
            .map(|(_, i)| i.start().flicks() + i.duration().flicks())
            .max()
            .unwrap_or(origin);
        let order: Vec<&cutweave_model::Track> = seq.tracks_in_order().collect();
        let mut tracks: Vec<ClipboardTrack> = Vec::new();
        let mut slot: BTreeMap<usize, u32> = BTreeMap::new();
        let top = found.first().map_or(0, |(index, _)| *index);
        let mut items = Vec::with_capacity(found.len());
        for (index, item) in found {
            let slot = *slot.entry(index).or_insert_with(|| {
                let track = order[index];
                tracks.push(ClipboardTrack {
                    kind: track.kind,
                    row: u32::try_from(index - top).unwrap_or(u32::MAX),
                    source: track.id,
                });
                u32::try_from(tracks.len() - 1).unwrap_or(u32::MAX)
            });
            let mut copy = item.clone();
            copy.set_start(time(item.start().flicks() - origin));
            items.push(ClipboardItem {
                track: slot,
                item: copy,
            });
        }
        Ok(TimelineCopyOutput {
            clipboard: ClipboardPayload {
                version: CLIPBOARD_VERSION,
                sequence,
                frame_rate: seq.format.frame_rate,
                span: dur(end - origin),
                tracks,
                items,
            },
        })
    }
}

// ----- timeline.paste ----------------------------------------------------------------------

/// Result of `timeline.paste`.
#[wire]
pub struct TimelinePasteOutput {
    /// The new items: each payload item's ID (`source`) and the ID of its copy.
    pub copies: Vec<DuplicatedItem>,
    /// Tracks created by the placement, in creation order.
    pub created_tracks: Vec<TrackId>,
    /// What changed (timeline spec §3.1).
    pub report: EditReport,
}

/// Pastes a clipboard payload at `at`. Every item and group gets a new ID and links are
/// remapped; media references are kept, so it works across sequences. `track` is where the
/// top-most payload track goes (the others keep their rows and need the same kind); without it
/// the items return to their source tracks. `mode` picks the placement (default `insert` with
/// items on the magnetic main track, else `reject`). `Locked` if a track it would change is
/// locked; nothing changes then.
#[command(
    name = "timeline.paste",
    summary = "Paste a clipboard payload from timeline.copy at a time and track",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = false)
)]
pub struct TimelinePaste {
    /// The payload `timeline.copy` returned.
    pub payload: ClipboardPayload,
    /// Where the earliest copied item goes.
    pub at: TimeInput,
    /// The track for the top-most payload track (default: the source tracks).
    pub track: Option<TrackId>,
    /// Placement mode.
    pub mode: Option<PlacementMode>,
}

fn bad(path: &str, msg: impl Into<String>) -> CmdError {
    CmdError::invalid(format!("/payload{path}"), msg)
}

/// The payload is well formed: version, tracks, items, unique IDs.
fn check_payload(p: &ClipboardPayload) -> Result<(), CmdError> {
    if p.version != CLIPBOARD_VERSION {
        return Err(bad(
            "/version",
            format!(
                "version {} is not supported; this build reads version {CLIPBOARD_VERSION}",
                p.version
            ),
        ));
    }
    if p.items.is_empty() || p.items.len() > crate::item::MAX_DELETE_ITEMS {
        return Err(bad(
            "/items",
            format!("give between 1 and {} items", crate::item::MAX_DELETE_ITEMS),
        ));
    }
    let mut last_row = None;
    for t in &p.tracks {
        check_editable_kind("/payload/tracks", t.kind)?;
        let ordered = last_row.is_none_or(|r| t.row > r);
        if !ordered || (last_row.is_none() && t.row != 0) {
            return Err(bad(
                "/tracks",
                "rows start at 0 and increase in display order",
            ));
        }
        last_row = Some(t.row);
    }
    let mut seen = BTreeSet::new();
    for c in &p.items {
        let item = &c.item;
        if c.track as usize >= p.tracks.len() {
            return Err(bad("/items", "an item names a track the payload lacks"));
        }
        if !seen.insert(item.id()) {
            return Err(bad(
                "/items",
                format!("item `{}` is in the payload twice", item.id()),
            ));
        }
        if item.start().flicks() < 0 || item.duration().flicks() <= 0 {
            return Err(bad(
                "/items",
                format!("item `{}` has a negative start or no length", item.id()),
            ));
        }
    }
    Ok(())
}

/// The tracks each payload track goes to, in payload order.
fn destinations(
    doc: &Project,
    p: &ClipboardPayload,
    track: Option<TrackId>,
) -> Result<(SequenceId, Vec<TrackId>), CmdError> {
    let anchor = match track {
        Some(t) => t,
        None => p
            .tracks
            .first()
            .map(|t| t.source)
            .ok_or_else(|| bad("/tracks", "the payload has no tracks"))?,
    };
    let seq = doc
        .sequences
        .values()
        .find(|s| s.track(&anchor).is_some())
        .ok_or_else(|| {
            if track.is_some() {
                CmdError::not_found(EntityKind::Track, anchor)
            } else {
                CmdError::invalid(
                    "/track",
                    "the source tracks are gone; give `track` to say where to paste",
                )
            }
        })?;
    let order: Vec<&cutweave_model::Track> = seq.tracks_in_order().collect();
    let mut out = Vec::with_capacity(p.tracks.len());
    if track.is_none() {
        for t in &p.tracks {
            let found = seq.track(&t.source).ok_or_else(|| {
                CmdError::invalid(
                    "/track",
                    "a source track is gone or in another sequence; give `track`",
                )
            })?;
            out.push(found.id);
        }
        return Ok((seq.id, out));
    }
    let base = order.iter().position(|t| t.id == anchor).unwrap_or(0);
    for t in &p.tracks {
        let found = order.get(base + t.row as usize).ok_or_else(|| {
            CmdError::invalid(
                "/track",
                "the sequence has too few tracks below this one for the pasted rows",
            )
        })?;
        if found.kind != t.kind {
            return Err(CmdError::invalid(
                "/track",
                format!(
                    "track `{}` is not a {:?} track; the pasted items need one at that row",
                    found.name, t.kind
                ),
            ));
        }
        out.push(found.id);
    }
    Ok((seq.id, out))
}

fn snap(rate: FrameRate, flicks: i64) -> Result<i64, CmdError> {
    rate.snap_to_frame(time(flicks), Rounding::Nearest)
        .map(Time::flicks)
        .map_err(|e| CmdError::invalid("/payload", e.to_string()))
}

#[allow(clippy::too_many_lines)] // one plan, phase by phase (spec §4.8)
fn plan_paste(
    cmd: &TimelinePaste,
    doc: &Project,
    ids: &mut dyn Ids,
) -> Result<(Plan, Vec<DuplicatedItem>), CmdError> {
    let p = &cmd.payload;
    check_payload(p)?;
    let (sequence, dest) = destinations(doc, p, cmd.track)?;
    let mut e = Edit::new(doc, sequence, "/at")?;
    let rate = e.seq.format.frame_rate;
    let regrid = rate != p.frame_rate;
    let at = resolve_frame_time("/at", cmd.at, rate)?.flicks();
    for t in &dest {
        if let Some(track) = e.seq.track(t) {
            check_editable_kind("/track", track.kind)?;
        }
    }
    for c in &p.items {
        let Some(id) = c.item.media() else { continue };
        let asset = doc
            .media
            .get(&id)
            .ok_or_else(|| CmdError::not_found(EntityKind::Media, id))?;
        // What `Project::validate` holds every clip to: the media plays on the track, and the
        // source range lies inside it (a payload an agent wrote by hand may break either).
        if let Some(track) = e.seq.track(&dest[c.track as usize])
            && !track.kind.accepts(asset.kind)
        {
            return Err(bad(
                "/items",
                format!("media `{id}` cannot play on a {:?} track", track.kind),
            ));
        }
        if let (Some(clip), Some(len)) = (c.item.as_clip(), asset.probe.duration)
            && asset.kind != cutweave_model::MediaKind::Image
            && i128::from(clip.source_in.flicks()) + i128::from(clip.duration.flicks())
                > i128::from(len.flicks())
        {
            return Err(bad(
                "/items",
                format!("item `{}` reads past the end of media `{id}`", clip.id),
            ));
        }
    }
    let mag = e.magnetic();
    let on_mag = |c: &ClipboardItem| Some(dest[c.track as usize]) == mag;
    let mode = resolve_mode(
        cmd.mode,
        None,
        if p.items.iter().any(on_mag) {
            PlacementMode::Insert
        } else {
            PlacementMode::Reject
        },
    )?;

    // Starts and lengths on the target's grid.
    let mut spans: BTreeMap<ItemId, (i64, i64)> = BTreeMap::new();
    for c in &p.items {
        let (s, l) = (c.item.start().flicks(), c.item.duration().flicks());
        let span = if regrid {
            let frame = super::placement::frame_flicks(rate);
            (snap(rate, at + s)? - at, snap(rate, l)?.max(frame))
        } else {
            (s, l)
        };
        spans.insert(c.item.id(), span);
    }

    // Groups with two or more members in the payload stay groups.
    let mut members: BTreeMap<cutweave_model::GroupId, usize> = BTreeMap::new();
    for c in &p.items {
        if let Some(g) = c.item.linked_group() {
            *members.entry(g).or_default() += 1;
        }
    }

    // The new items (IDs in payload order).
    let mut groups: BTreeMap<cutweave_model::GroupId, cutweave_model::GroupId> = BTreeMap::new();
    let mut copy_of: BTreeMap<ItemId, ItemId> = BTreeMap::new();
    let mut copies: Vec<(TrackId, ItemId, Item, i64)> = Vec::new();
    for c in &p.items {
        let src = &c.item;
        let group = src
            .linked_group()
            .filter(|g| members.get(g).copied().unwrap_or(0) > 1)
            .map(|g| *groups.entry(g).or_insert_with(|| e.mint(ids)));
        let id: ItemId = e.mint(ids);
        let mut copy = src.clone();
        let (rel, len) = spans[&src.id()];
        copy.set_start(time(rel));
        copy.set_duration(dur(len));
        if let Item::Clip(clip) = &mut copy {
            clip.id = id;
            clip.linked_group = group;
            clip.markers = remint(&clip.markers, &mut e, ids);
        }
        copy_of.insert(src.id(), id);
        copies.push((dest[c.track as usize], src.id(), copy, rel));
    }
    for (_, _, copy, _) in &mut copies {
        if let Some(parent) = copy.attached_to() {
            let kept = if let Some(new) = copy_of.get(&parent) {
                Some(*new)
            } else if e.locate(parent).is_some() {
                Some(parent)
            } else {
                None
            };
            copy.set_attached_to(kept);
        }
    }

    // Items on the magnetic track form one block from `at` (INV-123, INV-41).
    let block: Vec<BlockItem> = copies
        .iter()
        .filter(|(t, ..)| Some(*t) == mag)
        .map(|(_, _, copy, rel)| BlockItem {
            id: copy.id(),
            group: copy.linked_group(),
            old: *rel,
            len: copy.duration().flicks(),
        })
        .collect();
    let (block_starts, shifts) = match (mag, block.iter().map(|b| b.old).min()) {
        (Some(m), Some(first)) => pack(&block, clamp_to_end(&e, m, at + first)),
        _ => (BTreeMap::new(), BTreeMap::new()),
    };
    let primary = copies
        .first()
        .map(|(t, ..)| *t)
        .ok_or_else(|| bad("/items", "give at least one item"))?;
    let mut out = Vec::with_capacity(copies.len());
    let landings: Vec<Landing> = copies
        .into_iter()
        .map(|(t, source, copy, rel)| {
            out.push(DuplicatedItem {
                source,
                copy: copy.id(),
            });
            let start = block_starts.get(&copy.id()).copied().unwrap_or_else(|| {
                rel + shifts
                    .get(&Unit::of(copy.id(), copy.linked_group()))
                    .copied()
                    .unwrap_or(at)
            });
            e.mark_moved(copy.id());
            Landing {
                dest: t,
                item: copy,
                start,
                old: start,
            }
        })
        .collect();
    e.place_movers(ids, landings, mode, primary, false);
    let r = e.finish(ids)?;
    Ok((r, out))
}

impl Command for TimelinePaste {
    type Output = TimelinePasteOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        plan_paste(self, ctx.doc(), &mut DryIds::default()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<TimelinePasteOutput, CmdError> {
        let doc = tx.doc().clone();
        let (plan, copies) = plan_paste(self, &doc, tx)?;
        let report = plan.commit(tx)?;
        Ok(TimelinePasteOutput {
            copies,
            created_tracks: plan.created_tracks,
            report,
        })
    }
}
