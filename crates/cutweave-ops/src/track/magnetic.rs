// SPDX-License-Identifier: GPL-3.0-or-later
//! `track.set_magnetic` (timeline spec §4.17, INV-6, INV-7, INV-281; ADR-0023): make a video
//! track the sequence's main (magnetic) track, or turn the magnet off.
//!
//! Making a track magnetic needs it to be gap-free from 0. With `close_gaps: true` the command
//! closes every gap itself, left to right, the way `timeline.delete_gap` does: a ripple with
//! the gap's end as threshold and the gap's width as shift, on the track and on every track
//! that holds a linked partner of an item that shifts (INV-30, INV-31). The plan is computed
//! purely on a snapshot ([`plan`]) and `validate` and `apply` share it (INV-20).
//!
//! The ripple here is deliberately small. WP-2A.03's placement core owns the general ripple;
//! when it lands it can replace [`close_gaps`], which only has to keep this module's tests
//! green.

use std::collections::{BTreeMap, BTreeSet};

use cutweave_commands::{CmdError, Command, QueryCtx, Tx, command, wire};
use cutweave_model::{
    EntityKind, GroupId, ItemId, Project, Sequence, Time, Track, TrackId, TrackKind,
};

use crate::report::EditReport;

/// Result of `track.set_magnetic`.
#[wire]
pub struct TrackSetMagneticOutput {
    /// The track.
    pub track: TrackId,
    /// Whether it is the main track now.
    pub magnetic: bool,
    /// The track that was the main track and no longer is, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous: Option<TrackId>,
    /// What changed (items moved by closing gaps).
    pub report: EditReport,
}

/// Makes a video track the main (magnetic) track, or turns the magnet off. Making a track the
/// main track clears the flag of the previous one in the same step. A track with gaps (or that
/// does not start at 0) cannot become the main track unless `close_gaps` is set, which closes
/// every gap from left to right; linked partners ripple with their items. Turning the magnet
/// off only clears the flag and also works on a locked track. Closing gaps on a locked track,
/// or on a track the ripple reaches, fails with `Locked`.
#[command(
    name = "track.set_magnetic",
    summary = "Make a video track the magnetic main track (gap-free from 0), or turn the magnet off",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct TrackSetMagnetic {
    /// The track.
    pub track: TrackId,
    /// `true` makes it the main track, `false` clears its flag.
    pub magnetic: bool,
    /// Close gaps (and the space before the first item) when making the track magnetic,
    /// instead of failing (default `false`).
    pub close_gaps: Option<bool>,
}

/// What `track.set_magnetic` will do.
struct Plan {
    /// The sequence of the track.
    sequence: cutweave_model::SequenceId,
    /// The magnetic flag changes (otherwise the command is a no-op).
    flag_changes: bool,
    /// The track that loses its flag.
    previous: Option<TrackId>,
    /// `(item, new start)` for every item the gap closing moves.
    shifts: Vec<(ItemId, Time)>,
}

fn find_track(doc: &Project, id: TrackId) -> Result<(&Sequence, &Track), CmdError> {
    doc.sequences
        .values()
        .find_map(|s| s.tracks.get(&id).map(|t| (s, t)))
        .ok_or_else(|| CmdError::not_found(EntityKind::Track, id))
}

/// An item of the working copy the gap closing shifts around.
#[derive(Clone, Copy)]
struct Work {
    id: ItemId,
    start: i64,
    duration: i64,
    group: Option<GroupId>,
}

impl Work {
    fn end(&self) -> i64 {
        self.start + self.duration
    }
}

/// Two items of one track that the ripple would make overlap: `(track, first, second)`.
type Collision = (TrackId, ItemId, ItemId);

/// The first gap of a track's items (sorted by start), as `[from, to)`.
fn first_gap(items: &[Work]) -> Option<(i64, i64)> {
    let mut expected = 0;
    for item in items {
        if item.start > expected {
            return Some((expected, item.start));
        }
        expected = expected.max(item.end());
    }
    None
}

/// The shifts that close every gap of `track`, and the first collision (INV-32) if the
/// ripple cannot be applied.
fn close_gaps(sequence: &Sequence, track: &Track) -> (Vec<(ItemId, Time)>, Option<Collision>) {
    let mut work: BTreeMap<TrackId, Vec<Work>> = sequence
        .tracks
        .values()
        .map(|t| {
            let items = t
                .items
                .iter()
                .map(|i| Work {
                    id: i.id(),
                    start: i.start().flicks(),
                    duration: i.duration().flicks(),
                    group: i.linked_group(),
                })
                .collect();
            (t.id, items)
        })
        .collect();
    let mut group_tracks: BTreeMap<GroupId, BTreeSet<TrackId>> = BTreeMap::new();
    for t in sequence.tracks.values() {
        for item in t.items.iter() {
            if let Some(g) = item.linked_group() {
                group_tracks.entry(g).or_default().insert(t.id);
            }
        }
    }
    let original: BTreeMap<ItemId, i64> =
        work.values().flatten().map(|w| (w.id, w.start)).collect();
    let mut collision = None;
    // Each pass closes the first gap; a track of n items has at most n gaps.
    while let Some((from, to)) = work.get(&track.id).and_then(|items| first_gap(items)) {
        let (threshold, width) = (to, to - from);
        let mut rippled: BTreeSet<TrackId> = BTreeSet::from([track.id]);
        loop {
            let reached: BTreeSet<TrackId> = rippled
                .iter()
                .filter_map(|t| work.get(t))
                .flatten()
                .filter(|w| w.start >= threshold)
                .filter_map(|w| w.group)
                .flat_map(|g| group_tracks.get(&g).into_iter().flatten().copied())
                .collect();
            let before = rippled.len();
            rippled.extend(reached);
            if rippled.len() == before {
                break;
            }
        }
        for t in &rippled {
            for w in work.get_mut(t).into_iter().flatten() {
                if w.start >= threshold {
                    w.start -= width;
                }
            }
            if let Some(items) = work.get_mut(t) {
                items.sort_by_key(|w| (w.start, w.id));
                if let Some(pair) = items
                    .windows(2)
                    .find(|p| p[0].end() > p[1].start)
                    .map(|p| (*t, p[0].id, p[1].id))
                {
                    collision = Some(pair);
                }
            }
        }
        if collision.is_some() {
            break;
        }
    }
    let shifts = work
        .values()
        .flatten()
        .filter(|w| original.get(&w.id) != Some(&w.start))
        .filter_map(|w| Time::from_flicks(w.start).ok().map(|t| (w.id, t)))
        .collect();
    (shifts, collision)
}

fn plan(cmd: &TrackSetMagnetic, doc: &Project) -> Result<Plan, CmdError> {
    let (sequence, track) = find_track(doc, cmd.track)?;
    let previous = sequence
        .tracks
        .values()
        .find(|t| t.magnetic && t.id != track.id)
        .map(|t| t.id);
    let mut plan = Plan {
        sequence: sequence.id,
        flag_changes: track.magnetic != cmd.magnetic,
        previous: None,
        shifts: Vec::new(),
    };
    if !cmd.magnetic {
        return Ok(plan);
    }
    if track.kind != TrackKind::Video {
        return Err(CmdError::invalid(
            "/track",
            format!(
                "only a video track can be the main track; `{}` is a {} track",
                track.name, track.kind
            ),
        ));
    }
    plan.previous = previous;
    let items: Vec<Work> = track
        .items
        .iter()
        .map(|i| Work {
            id: i.id(),
            start: i.start().flicks(),
            duration: i.duration().flicks(),
            group: None,
        })
        .collect();
    let Some((from, to)) = first_gap(&items) else {
        return Ok(plan);
    };
    if !cmd.close_gaps.unwrap_or(false) {
        let rate = sequence.format.frame_rate;
        let describe = |t: i64| {
            Time::from_flicks(t).ok().map_or_else(
                || t.to_string(),
                |t| {
                    format!(
                        "frame {}",
                        rate.time_to_frame(t, cutweave_model::Rounding::Nearest)
                    )
                },
            )
        };
        return Err(CmdError::invalid(
            "/track",
            format!(
                "track `{}` has a gap from {} to {}; a main track is gap-free from 0. \
                 Pass close_gaps to close it",
                track.name,
                describe(from),
                describe(to)
            ),
        ));
    }
    let (shifts, collision) = close_gaps(sequence, track);
    // The locks come before the collision (INV-27): the plan is judged as if nothing were locked.
    for t in sequence.tracks_in_order().filter(|t| t.locked) {
        if shifts.iter().any(|(id, _)| t.items.contains(id)) {
            return Err(CmdError::locked(EntityKind::Track, t.id));
        }
    }
    if let Some((on, a, b)) = collision {
        let name = sequence
            .track(&on)
            .map_or_else(String::new, |t| t.name.clone());
        return Err(CmdError::invalid(
            "/track",
            format!(
                "closing the gaps would make `{a}` overlap `{b}` on track `{name}`; \
                 the ripple cannot be applied"
            ),
        ));
    }
    plan.shifts = shifts;
    Ok(plan)
}

impl Command for TrackSetMagnetic {
    type Output = TrackSetMagneticOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        plan(self, ctx.doc()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<TrackSetMagneticOutput, CmdError> {
        let plan = plan(self, tx.doc())?;
        let before = tx.sequence(plan.sequence)?.clone();
        for (item, start) in &plan.shifts {
            tx.update_item(*item, |i| {
                i.set_start(*start);
                Ok(())
            })?;
        }
        if let Some(previous) = plan.previous {
            tx.update_track(previous, |t| {
                t.magnetic = false;
                Ok(())
            })?;
        }
        if plan.flag_changes {
            tx.update_track(self.track, |t| {
                t.magnetic = self.magnetic;
                Ok(())
            })?;
        }
        let report = EditReport::diff(&before, tx.sequence(plan.sequence)?, &[], &[]);
        Ok(TrackSetMagneticOutput {
            track: self.track,
            magnetic: self.magnetic,
            previous: plan.previous,
            report,
        })
    }
}
