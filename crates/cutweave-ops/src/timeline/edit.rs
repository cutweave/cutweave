// SPDX-License-Identifier: GPL-3.0-or-later
//! Range and gap edits (WP-2A.03; timeline spec §4.4–§4.6; ADR-0023): `timeline.ripple_delete`,
//! `timeline.lift`, `timeline.extract` and `timeline.delete_gap`. All four are built on the
//! placement core ([`super::placement`]) and return an [`EditReport`].
//!
//! **IDs** (INV-23): only `lift` and `extract` create items (the right parts of items spanning
//! a range): target tracks in display order, ranges in time order, spanning items in start
//! order; each draws its right parts' group (when linked), then the right parts.

use std::collections::BTreeSet;

use cutweave_commands::{CmdError, Command, QueryCtx, Tx, command, wire};
use cutweave_model::{
    EntityKind, Item, ItemId, Project, Sequence, SequenceId, Time, TimeInput, TrackId,
};

use super::placement::{DryIds, Edit, Ids, Plan, closing, span, time};
use crate::{
    item::MAX_DELETE_ITEMS, report::EditReport, track::check_editable_kind,
    util::resolve_frame_time,
};

/// A time range of a sequence, as used: `[start, end)` on the frame grid.
#[wire]
#[derive(Copy, Eq)]
pub struct UsedRange {
    /// First frame of the range.
    pub start: Time,
    /// End of the range (exclusive).
    pub end: Time,
}

/// A time range in the params: `[start, end)`, `end > start`.
#[wire]
pub struct TimelineRange {
    /// Start of the range, `>= 0`.
    pub start: TimeInput,
    /// End of the range (exclusive), after `start`.
    pub end: TimeInput,
}

// ----- timeline.ripple_delete --------------------------------------------------------------

/// Result of `timeline.ripple_delete`.
#[wire]
pub struct DeletedOutput {
    /// Every deleted item: the requested ones in request order, then their linked partners.
    pub deleted: Vec<ItemId>,
    /// What changed (timeline spec §3.1).
    pub report: EditReport,
}

/// Deletes items and their linked partners and closes the space each given item leaves on its
/// track: what follows on that track moves left by the length deleted before it, and so do the
/// tracks of linked partners of what moves (or every track with `ripple_all_tracks`). All given
/// items must be on one track; gaps that were already there stay. Fails with `Invalid` if the
/// shift would make items overlap on a rippled track, and with `Locked` if it would change a
/// locked track. On the magnetic main track it is the same as `item.delete`.
#[command(
    name = "timeline.ripple_delete",
    summary = "Delete items on one track and close the space they leave (partners follow)",
    undoable,
    mcp = "catalog",
    annotations(destructive = true, idempotent = false)
)]
pub struct TimelineRippleDelete {
    /// The items to delete, at least one, all on one track.
    pub items: Vec<ItemId>,
    /// Shift every track of the sequence, not only the item's track and its partners' tracks
    /// (default `false`).
    pub ripple_all_tracks: Option<bool>,
}

/// The given items (deduplicated, in order) and the sequence holding them; `NotFound`,
/// `Invalid{path:"/items"}` for an empty or oversized list or items of several sequences.
pub(crate) fn given_items(
    doc: &Project,
    items: &[ItemId],
) -> Result<(SequenceId, Vec<ItemId>), CmdError> {
    if items.is_empty() {
        return Err(CmdError::invalid("/items", "give at least one item"));
    }
    if items.len() > MAX_DELETE_ITEMS {
        return Err(CmdError::invalid(
            "/items",
            format!("at most {MAX_DELETE_ITEMS} items per call"),
        ));
    }
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    let mut sequence = None;
    for id in items {
        let at = doc
            .find_item(id)
            .ok_or_else(|| CmdError::not_found(EntityKind::Item, *id))?;
        match sequence {
            None => sequence = Some(at.sequence.id),
            Some(s) if s != at.sequence.id => {
                return Err(CmdError::invalid(
                    "/items",
                    "all items must be in one sequence",
                ));
            }
            Some(_) => {}
        }
        if seen.insert(*id) {
            out.push(*id);
        }
    }
    let sequence = sequence.ok_or_else(|| CmdError::invalid("/items", "give at least one item"))?;
    Ok((sequence, out))
}

/// Items taken out of a sequence, with the tracks they were on.
pub(crate) type Removed = Vec<(TrackId, Item)>;

/// Removes `given` and their partners from the edit: `(deleted in output order, removed items
/// with their tracks)`. `Unsupported` for items on tracks that are not editable.
pub(crate) fn remove_with_partners(
    e: &mut Edit<'_>,
    given: &[ItemId],
) -> Result<(Vec<ItemId>, Removed), CmdError> {
    let mut deleted = Vec::new();
    let mut partners = Vec::new();
    let mut removed = Vec::new();
    let mut seen = BTreeSet::new();
    for id in given {
        for (n, (track, m)) in e.members(*id).into_iter().enumerate() {
            if !seen.insert(m) {
                continue;
            }
            let kind = e.seq.track(&track).map(|t| t.kind);
            if let Some(kind) = kind {
                check_editable_kind("/items", kind)?;
            }
            if n == 0 || given.contains(&m) {
                deleted.push(m);
            } else {
                partners.push(m);
            }
        }
    }
    deleted.sort_by_key(|m| given.iter().position(|g| g == m).unwrap_or(usize::MAX));
    deleted.extend(partners);
    for m in &deleted {
        if let Some(r) = e.remove(*m) {
            removed.push(r);
        }
    }
    Ok((deleted, removed))
}

fn plan_ripple_delete(
    cmd: &TimelineRippleDelete,
    doc: &Project,
    ids: &mut dyn Ids,
) -> Result<(Plan, Vec<ItemId>), CmdError> {
    let (sequence, given) = given_items(doc, &cmd.items)?;
    let mut tracks = given
        .iter()
        .filter_map(|id| doc.find_item(id).map(|at| at.track.id));
    let track = tracks
        .next()
        .ok_or_else(|| CmdError::invalid("/items", "give at least one item"))?;
    if tracks.any(|t| t != track) {
        return Err(CmdError::invalid(
            "/items",
            "all items must be on one track; to close ranges across tracks use timeline.extract",
        ));
    }
    let mut e = Edit::new(doc, sequence, "/items")?;
    let (deleted, removed) = remove_with_partners(&mut e, &given)?;
    let on_track: Vec<(i64, i64)> = removed
        .iter()
        .filter(|(t, _)| *t == track)
        .map(|(_, i)| span(i))
        .collect();
    if let Some(theta) = on_track.iter().map(|(s, _)| *s).min() {
        let f = closing(on_track.iter().map(|&(s, e)| (e, e - s)));
        // A removed partner on the magnetic track closes with it (aligned groups).
        let mut ripple_tracks = vec![track];
        if let Some(mag) = e.magnetic()
            && mag != track
            && removed.iter().any(|(t, _)| *t == mag)
        {
            ripple_tracks.push(mag);
        }
        e.ripple(
            &ripple_tracks,
            cmd.ripple_all_tracks.unwrap_or(false),
            theta,
            &f,
        );
    }
    Ok((e.finish(ids)?, deleted))
}

impl Command for TimelineRippleDelete {
    type Output = DeletedOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        plan_ripple_delete(self, ctx.doc(), &mut DryIds::default()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<DeletedOutput, CmdError> {
        let doc = tx.doc().clone();
        let (plan, deleted) = plan_ripple_delete(self, &doc, tx)?;
        let report = plan.commit(tx)?;
        Ok(DeletedOutput { deleted, report })
    }
}

// ----- timeline.lift and timeline.extract --------------------------------------------------

/// Result of `timeline.lift` and `timeline.extract`.
#[wire]
pub struct TimelineRangesOutput {
    /// The ranges used: snapped to the frame grid, sorted, overlapping and touching ones
    /// merged.
    pub ranges: Vec<UsedRange>,
    /// What changed (timeline spec §3.1).
    pub report: EditReport,
}

/// Lifts time ranges out of tracks and leaves the space empty: items inside a range are
/// removed, items across an edge are trimmed to it, an item across a whole range is split
/// around it, and linked partners get the same cut (also on tracks not listed). On the magnetic
/// main track the space closes (its partners' tracks follow). `ranges` default to the sequence
/// in/out range, `tracks` to every track. Fails with `Locked` if it would change a locked
/// track (pass `tracks` to leave one out).
#[command(
    name = "timeline.lift",
    summary = "Cut time ranges out of tracks, leaving gaps (the magnetic main track closes)",
    undoable,
    mcp = "catalog",
    annotations(destructive = true, idempotent = false)
)]
pub struct TimelineLift {
    /// The sequence.
    pub sequence: SequenceId,
    /// The ranges to cut (default: the sequence in/out range).
    pub ranges: Option<Vec<TimelineRange>>,
    /// The tracks to cut on (default: every track of the sequence).
    pub tracks: Option<Vec<TrackId>>,
}

/// Extracts time ranges: cuts them out of the tracks like `timeline.lift`, then closes them on
/// every target track: whatever starts at or after a range end moves left by the length of the
/// ranges before it, and so do the tracks of linked partners of what moves. `ranges` default to
/// the sequence in/out range, `tracks` to every track. Fails with `Locked` if it would change
/// a locked track, and with `Invalid` if the shift would make items overlap or move the
/// magnetic main track when it is not a target.
#[command(
    name = "timeline.extract",
    summary = "Cut time ranges out of tracks and close them (ripple)",
    undoable,
    mcp = "catalog",
    annotations(destructive = true, idempotent = false)
)]
pub struct TimelineExtract {
    /// The sequence.
    pub sequence: SequenceId,
    /// The ranges to cut (default: the sequence in/out range).
    pub ranges: Option<Vec<TimelineRange>>,
    /// The tracks to cut on (default: every track of the sequence).
    pub tracks: Option<Vec<TrackId>>,
}

/// The longest `ranges` and `tracks` lists.
const MAX_RANGES: usize = 1_000;

/// The ranges of a request (or the in/out range), snapped, sorted and merged where they
/// overlap or touch.
fn ranges_of(
    seq: &Sequence,
    ranges: Option<&[TimelineRange]>,
) -> Result<Vec<(i64, i64)>, CmdError> {
    let rate = seq.format.frame_rate;
    let mut spans: Vec<(i64, i64)> = match ranges {
        Some([]) => return Err(CmdError::invalid("/ranges", "give at least one range")),
        Some(list) if list.len() > MAX_RANGES => {
            return Err(CmdError::invalid(
                "/ranges",
                format!("at most {MAX_RANGES} ranges per call"),
            ));
        }
        Some(list) => {
            let mut out = Vec::with_capacity(list.len());
            for (n, r) in list.iter().enumerate() {
                let s = resolve_frame_time(&format!("/ranges/{n}/start"), r.start, rate)?;
                let e = resolve_frame_time(&format!("/ranges/{n}/end"), r.end, rate)?;
                if s < Time::ZERO {
                    return Err(CmdError::invalid(
                        format!("/ranges/{n}/start"),
                        "the time must not be negative",
                    ));
                }
                if e <= s {
                    return Err(CmdError::invalid(
                        format!("/ranges/{n}"),
                        "the range must end after it starts",
                    ));
                }
                out.push((s.flicks(), e.flicks()));
            }
            out
        }
        None => match &seq.in_out {
            Some(io) => vec![(io.start.flicks(), io.end.flicks())],
            None => {
                return Err(CmdError::invalid(
                    "/ranges",
                    "give ranges, or set the sequence in and out points first",
                ));
            }
        },
    };
    spans.sort_unstable();
    let mut merged: Vec<(i64, i64)> = Vec::with_capacity(spans.len());
    for (s, e) in spans {
        match merged.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => merged.push((s, e)),
        }
    }
    Ok(merged)
}

/// The target tracks of a request in display order (default: every editable track).
fn targets_of(
    doc: &Project,
    seq: &Sequence,
    tracks: Option<&[TrackId]>,
) -> Result<Vec<TrackId>, CmdError> {
    match tracks {
        Some([]) => Err(CmdError::invalid("/tracks", "give at least one track")),
        Some(list) => {
            let mut out = Vec::new();
            for (n, t) in list.iter().enumerate() {
                let Some(track) = seq.track(t) else {
                    if doc.sequences.values().any(|s| s.track(t).is_some()) {
                        return Err(CmdError::invalid(
                            format!("/tracks/{n}"),
                            format!("track `{t}` is not in sequence `{}`", seq.id),
                        ));
                    }
                    return Err(CmdError::not_found(EntityKind::Track, *t));
                };
                check_editable_kind(&format!("/tracks/{n}"), track.kind)?;
                if !out.contains(t) {
                    out.push(*t);
                }
            }
            // Display order, so new IDs do not depend on the order of the list.
            Ok(seq
                .track_order
                .iter()
                .filter(|t| out.contains(t))
                .copied()
                .collect())
        }
        None => Ok(seq
            .tracks_in_order()
            .filter(|t| check_editable_kind("/tracks", t.kind).is_ok())
            .map(|t| t.id)
            .collect()),
    }
}

fn plan_ranges(
    doc: &Project,
    sequence: SequenceId,
    ranges: Option<&[TimelineRange]>,
    tracks: Option<&[TrackId]>,
    extract: bool,
    ids: &mut dyn Ids,
) -> Result<(Plan, Vec<UsedRange>), CmdError> {
    let seq = doc
        .sequences
        .get(&sequence)
        .ok_or_else(|| CmdError::not_found(EntityKind::Sequence, sequence))?;
    let merged = ranges_of(seq, ranges)?;
    let targets = targets_of(doc, seq, tracks)?;
    let mut e = Edit::new(doc, sequence, "/ranges")?;
    for track in &targets {
        for &(s, u) in &merged {
            e.cut(ids, *track, s, u);
        }
    }
    if extract {
        if let Some(&(theta, _)) = merged.first() {
            let f = closing(merged.iter().map(|&(s, u)| (u, u - s)));
            e.ripple(&targets, false, theta, &f);
        }
    } else if let Some(mag) = e.magnetic() {
        // The magnetic track closes what was cut out of it: its gaps now.
        let gaps = gaps_of(&e, mag);
        if let Some(&(theta, _)) = gaps.first() {
            let f = closing(gaps.iter().map(|&(s, u)| (u, u - s)));
            e.ripple(&[mag], false, theta, &f);
        }
    }
    let used = merged
        .iter()
        .map(|&(s, u)| UsedRange {
            start: time(s),
            end: time(u),
        })
        .collect();
    Ok((e.finish(ids)?, used))
}

/// The gaps of a track (the space before its first item counts), as `(from, to)` flicks.
fn gaps_of(e: &Edit<'_>, track: TrackId) -> Vec<(i64, i64)> {
    let mut out = Vec::new();
    let mut expected = 0;
    if let Some(t) = e.seq.track(&track) {
        for item in t.items.iter() {
            let (s, end) = span(item);
            if s > expected {
                out.push((expected, s));
            }
            expected = expected.max(end);
        }
    }
    out
}

macro_rules! ranges_command {
    ($cmd:ty, $extract:expr) => {
        impl Command for $cmd {
            type Output = TimelineRangesOutput;

            fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
                plan_ranges(
                    ctx.doc(),
                    self.sequence,
                    self.ranges.as_deref(),
                    self.tracks.as_deref(),
                    $extract,
                    &mut DryIds::default(),
                )
                .map(drop)
            }

            fn apply(&self, tx: &mut Tx<'_>) -> Result<TimelineRangesOutput, CmdError> {
                let doc = tx.doc().clone();
                let (plan, ranges) = plan_ranges(
                    &doc,
                    self.sequence,
                    self.ranges.as_deref(),
                    self.tracks.as_deref(),
                    $extract,
                    tx,
                )?;
                let report = plan.commit(tx)?;
                Ok(TimelineRangesOutput { ranges, report })
            }
        }
    };
}

ranges_command!(TimelineLift, false);
ranges_command!(TimelineExtract, true);

// ----- timeline.delete_gap -----------------------------------------------------------------

/// Result of `timeline.delete_gap`.
#[wire]
pub struct TimelineDeleteGapOutput {
    /// The gap that was closed.
    pub gap: UsedRange,
    /// What changed (timeline spec §3.1).
    pub report: EditReport,
}

/// Closes the gap of a track that contains `at`: the items from the gap's end on move left by
/// its length, and so do the tracks of linked partners of what moves (or every track with
/// `ripple_all_tracks`). A gap is the empty space between two items, or before the first one;
/// `at` inside an item or after the last one is `Invalid` (the magnetic main track has no
/// gaps). Fails with `Invalid` if the shift would make items overlap, and with `Locked` if it
/// would change a locked track.
#[command(
    name = "timeline.delete_gap",
    summary = "Close the gap of a track at a time (what follows moves left; partners follow)",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = false)
)]
pub struct TimelineDeleteGap {
    /// The track.
    pub track: TrackId,
    /// A time inside the gap.
    pub at: TimeInput,
    /// Shift every track of the sequence, not only this one and its partners' tracks (default
    /// `false`).
    pub ripple_all_tracks: Option<bool>,
}

fn plan_delete_gap(
    cmd: &TimelineDeleteGap,
    doc: &Project,
    ids: &mut dyn Ids,
) -> Result<(Plan, UsedRange), CmdError> {
    let (sequence, track) = doc
        .sequences
        .values()
        .find_map(|s| s.track(&cmd.track).map(|t| (s, t)))
        .ok_or_else(|| CmdError::not_found(EntityKind::Track, cmd.track))?;
    check_editable_kind("/track", track.kind)?;
    let at = resolve_frame_time("/at", cmd.at, sequence.format.frame_rate)?.flicks();
    if at < 0 {
        return Err(CmdError::invalid("/at", "the time must not be negative"));
    }
    let mut expected = 0;
    let mut gap = None;
    for item in track.items.iter() {
        let (s, e) = span(item);
        if at < s {
            if at >= expected {
                gap = Some((expected, s));
            }
            break;
        }
        if at < e {
            return Err(CmdError::invalid(
                "/at",
                format!("{at} flicks is inside item `{}`, not in a gap", item.id()),
            ));
        }
        expected = expected.max(e);
    }
    let Some((from, to)) = gap else {
        let why = if track.magnetic {
            format!(
                "`{}` is the magnetic main track: it has no gaps",
                track.name
            )
        } else {
            format!("there is no gap at that time on track `{}`", track.name)
        };
        return Err(CmdError::invalid("/at", why));
    };
    let mut e = Edit::new(doc, sequence.id, "/at")?;
    let width = to - from;
    e.ripple(
        &[track.id],
        cmd.ripple_all_tracks.unwrap_or(false),
        to,
        &move |_| -width,
    );
    Ok((
        e.finish(ids)?,
        UsedRange {
            start: time(from),
            end: time(to),
        },
    ))
}

impl Command for TimelineDeleteGap {
    type Output = TimelineDeleteGapOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        plan_delete_gap(self, ctx.doc(), &mut DryIds::default()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<TimelineDeleteGapOutput, CmdError> {
        let doc = tx.doc().clone();
        let (plan, gap) = plan_delete_gap(self, &doc, tx)?;
        let report = plan.commit(tx)?;
        Ok(TimelineDeleteGapOutput { gap, report })
    }
}
