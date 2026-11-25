// SPDX-License-Identifier: GPL-3.0-or-later
//! The placement core (WP-2A.03; timeline spec §3, §3.4–§3.6; ADR-0023): one place for the
//! ripple, cut, split, placement-mode, magnetic, lock, link and attachment rules that every
//! placement command shares (`timeline.insert`/`overwrite`/`lift`/`extract`/`ripple_delete`/
//! `delete_gap`, `item.move`/`delete`/`duplicate`, `clip.split`, and `track.set_magnetic`'s gap
//! closing).
//!
//! # How a command uses it
//!
//! A command builds an [`Edit`] over a snapshot of one sequence, applies its phases to the
//! working copy (lift, cut, close, open, place: spec §3.4), and calls [`Edit::finish`], which
//! settles attachments and judges the result. The outcome is a [`Plan`]: the sequence after the
//! edit, the command's splits and the tracks it created. `validate` computes the plan with
//! placeholder IDs ([`DryIds`]) and drops it; `apply` computes it again with the transaction's
//! IDs and writes it with [`Plan::commit`] (INV-20). Nothing touches the document before the plan
//! succeeded, so a failing command changes nothing.
//!
//! # Judging a plan (INV-27, §3.8)
//!
//! The phases never stop at a semantic problem (an overlap under `reject`, a ripple collision, a
//! magnetic track a free-track ripple would move): they record the first one and go on, so the
//! plan is the one "computed as if nothing were locked". [`Edit::finish`] then fails with
//! `Locked` naming the first locked track (display order) whose items differ, else with the
//! recorded error, else with `Invalid` if a track holds overlapping items (INV-1, INV-32) or the
//! magnetic track has a gap (INV-7). Parameter errors, `NotFound` and `Unsupported` are returned
//! by the commands before an `Edit` exists.
//!
//! # Attachments (INV-36 to INV-39)
//!
//! Every phase that translates items calls [`Edit::follow`] for it: children of translated
//! items move by the same amount, unless the phase moved them itself (a mover, or an item on a
//! rippled track: "one shift per phase") or their parent landed on their track. `finish` then
//! detaches children whose parent is gone (D16) or on their track, clamps children of a parent
//! whose extent changed into its range, detaches children that ended up outside their parent
//! for any other reason, and sends a child that overlaps an item of its track to a track
//! created directly above it (INV-37).
//!
//! # IDs (INV-23)
//!
//! An `Edit` draws new IDs only through [`Ids`], in the order its phases need them: a split or
//! a spanning cut draws the right halves' group (when linked) and then the right halves in
//! member order; a new track draws its ID when it is created. Each command documents its order.

use std::collections::{BTreeMap, BTreeSet};

use cutweave_commands::{CmdError, Tx, wire};
use cutweave_model::{
    Duration, EntityId, EntityKind, FrameRate, GroupId, Item, ItemId, MediaKind, Project, Sequence,
    SequenceId, Time, Track, TrackId, TrackKind,
};

use crate::{
    report::{EditReport, SplitReport},
    timeline::OverlapPolicy,
    track::next_track_name,
};

// ----- placement modes ---------------------------------------------------------------------

/// How a placement treats what is already on the target range (timeline spec §3.6).
#[wire]
#[derive(Copy, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlacementMode {
    /// Split the item under the start, ripple the rest of the track (and its partners' tracks)
    /// later by the placed length, then place (default on the magnetic main track).
    Insert,
    /// Cut the range out of the track (trimming or splitting what is there), then place. Nothing
    /// ripples.
    Overwrite,
    /// Place on the target track if the range is free, otherwise on a new track of the same
    /// kind directly above it.
    NewTrack,
    /// Place if the range is free, otherwise fail with `Invalid` naming the item in the way
    /// (default on free tracks).
    Reject,
}

impl PlacementMode {
    /// The snake-case name, as in the params.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            PlacementMode::Insert => "insert",
            PlacementMode::Overwrite => "overwrite",
            PlacementMode::NewTrack => "new_track",
            PlacementMode::Reject => "reject",
        }
    }
}

impl From<OverlapPolicy> for PlacementMode {
    fn from(p: OverlapPolicy) -> PlacementMode {
        match p {
            OverlapPolicy::Reject => PlacementMode::Reject,
            OverlapPolicy::NewTrack => PlacementMode::NewTrack,
        }
    }
}

/// The mode of a request: `mode`, or its P1 alias `overlap` (giving both with different values
/// is `Invalid{path:"/mode"}`), or `default` (INV-40).
pub(crate) fn resolve_mode(
    mode: Option<PlacementMode>,
    overlap: Option<OverlapPolicy>,
    default: PlacementMode,
) -> Result<PlacementMode, CmdError> {
    match (mode, overlap.map(PlacementMode::from)) {
        (Some(m), Some(o)) if m != o => Err(CmdError::invalid(
            "/mode",
            format!(
                "`mode` \"{}\" and its alias `overlap` \"{}\" disagree; give one of them",
                m.name(),
                o.name()
            ),
        )),
        (Some(m), _) | (None, Some(m)) => Ok(m),
        (None, None) => Ok(default),
    }
}

// ----- IDs ---------------------------------------------------------------------------------

/// Where an edit draws its new IDs: the transaction's generator when the command applies,
/// placeholders when it only validates. A plan draws them in the same order either way.
pub(crate) trait Ids {
    /// The next raw value.
    fn next_raw(&mut self) -> u128;
}

impl Ids for Tx<'_> {
    fn next_raw(&mut self) -> u128 {
        self.new_id::<ItemId>().as_u128()
    }
}

/// Placeholder IDs for a dry run (`validate`): increasing values far from real ones.
pub(crate) struct DryIds(u128);

impl Default for DryIds {
    fn default() -> DryIds {
        DryIds(u128::MAX / 3 * 2)
    }
}

impl Ids for DryIds {
    fn next_raw(&mut self) -> u128 {
        self.0 += 1;
        self.0
    }
}

/// A fresh typed ID from `ids`.
pub(crate) fn mint<I: EntityId>(ids: &mut dyn Ids) -> I {
    I::from_u128(ids.next_raw())
}

// ----- small conversions -------------------------------------------------------------------

/// A time from flicks, saturating at the representable range.
pub(crate) fn time(flicks: i64) -> Time {
    Time::from_flicks(flicks).unwrap_or(if flicks < 0 { Time::MIN } else { Time::MAX })
}

/// A duration from flicks (zero or more), saturating.
pub(crate) fn dur(flicks: i64) -> Duration {
    Duration::from_flicks(flicks.max(0)).unwrap_or(Duration::ZERO)
}

/// `(start, end)` of an item in flicks.
pub(crate) fn span(item: &Item) -> (i64, i64) {
    let s = item.start().flicks();
    (s, s.saturating_add(item.duration().flicks()))
}

/// Flicks per frame of `rate` (the length of frame 0 when the rate is not exact in flicks).
pub(crate) fn frame_flicks(rate: FrameRate) -> i64 {
    rate.flicks_per_frame()
        .or_else(|| rate.frame_to_time(1).ok().map(Time::flicks))
        .unwrap_or(1)
        .max(1)
}

// ----- the plan ----------------------------------------------------------------------------

/// The outcome of an edit: the sequence after it, and what the report cannot tell from the
/// difference alone (splits, created tracks).
#[derive(Clone, Debug)]
pub(crate) struct Plan {
    /// The sequence.
    pub(crate) sequence: SequenceId,
    /// The sequence before the edit.
    pub(crate) before: Sequence,
    /// The sequence after the edit.
    pub(crate) after: Sequence,
    /// The edit's splits (right halves that still exist).
    pub(crate) splits: Vec<SplitReport>,
    /// The tracks the edit created, in creation order.
    pub(crate) created_tracks: Vec<TrackId>,
}

impl Plan {
    /// The exact difference the edit makes (INV-21).
    pub(crate) fn report(&self) -> EditReport {
        EditReport::diff(
            &self.before,
            &self.after,
            &self.splits,
            &self.created_tracks,
        )
    }

    /// Writes the sequence after the edit through the transaction (one diff, all or nothing)
    /// and returns the report.
    pub(crate) fn commit(&self, tx: &mut Tx<'_>) -> Result<EditReport, CmdError> {
        let after = self.after.clone();
        tx.update_sequence(self.sequence, move |s| {
            *s = after;
            Ok(())
        })?;
        Ok(self.report())
    }
}

// ----- the working edit --------------------------------------------------------------------

/// A placement edit in progress over one sequence (see the module docs).
pub(crate) struct Edit<'d> {
    doc: &'d Project,
    before: Sequence,
    /// The working copy.
    pub(crate) seq: Sequence,
    /// Flicks per frame of the sequence.
    pub(crate) frame: i64,
    pointer: String,
    splits: Vec<SplitReport>,
    created_tracks: Vec<TrackId>,
    /// Tracks this edit created directly above a track, by that track.
    above: BTreeMap<TrackId, Vec<TrackId>>,
    deferred: Option<CmdError>,
    /// Items the edit moved itself: movers, copies and ripple shifts.
    moved: BTreeSet<ItemId>,
    /// Items whose extent changed (trimmed by a cut, left halves of splits).
    retimed: BTreeSet<ItemId>,
    /// Children that followed their parent or were clamped into it.
    followers: BTreeSet<ItemId>,
    /// Groups that lost members by `unlink`.
    left_groups: BTreeSet<GroupId>,
}

impl<'d> Edit<'d> {
    /// An edit of `sequence`. `pointer` is the param that `Invalid` errors of the phases point
    /// at (the edit point, spec §3.8).
    pub(crate) fn new(
        doc: &'d Project,
        sequence: SequenceId,
        pointer: &str,
    ) -> Result<Edit<'d>, CmdError> {
        let seq = doc
            .sequences
            .get(&sequence)
            .ok_or_else(|| CmdError::not_found(EntityKind::Sequence, sequence))?
            .clone();
        Ok(Edit {
            doc,
            frame: frame_flicks(seq.format.frame_rate),
            before: seq.clone(),
            seq,
            pointer: pointer.to_owned(),
            splits: Vec::new(),
            created_tracks: Vec::new(),
            above: BTreeMap::new(),
            deferred: None,
            moved: BTreeSet::new(),
            retimed: BTreeSet::new(),
            followers: BTreeSet::new(),
            left_groups: BTreeSet::new(),
        })
    }

    /// `Invalid` at the edit's pointer.
    pub(crate) fn invalid(&self, msg: impl Into<String>) -> CmdError {
        CmdError::invalid(self.pointer.clone(), msg)
    }

    /// Records a semantic error; the first one wins and `finish` reports it after the locks.
    pub(crate) fn defer(&mut self, error: CmdError) {
        if self.deferred.is_none() {
            self.deferred = Some(error);
        }
    }

    /// The sequence's magnetic main track in the working copy.
    pub(crate) fn magnetic(&self) -> Option<TrackId> {
        self.seq.tracks.values().find(|t| t.magnetic).map(|t| t.id)
    }

    /// A track's display name (or its ID if it is gone).
    pub(crate) fn track_name(&self, track: TrackId) -> String {
        self.seq
            .track(&track)
            .map_or_else(|| track.to_string(), |t| t.name.clone())
    }

    /// The end of a track's last item.
    pub(crate) fn track_end(&self, track: TrackId) -> i64 {
        self.seq.track(&track).map_or(0, |t| t.items.end().flicks())
    }

    /// Whether `media` is a still (its clips keep their source position when cut or split).
    fn is_still(&self, item: &Item) -> bool {
        item.media()
            .and_then(|m| self.doc.media.get(&m))
            .is_some_and(|m| m.kind == MediaKind::Image)
    }

    /// The item with its track.
    pub(crate) fn locate(&self, id: ItemId) -> Option<(TrackId, &Item)> {
        self.seq.find_item(&id).map(|(t, i)| (t.id, i))
    }

    /// The item followed by its linked partners (track-ID, then item-ID order).
    pub(crate) fn members(&self, id: ItemId) -> Vec<(TrackId, ItemId)> {
        let Some((track, item)) = self.locate(id) else {
            return Vec::new();
        };
        let mut out = vec![(track, id)];
        if let Some(group) = item.linked_group() {
            for t in self.seq.tracks.values() {
                for (_, other) in t.items.iter_by_id() {
                    if other.id() != id && other.linked_group() == Some(group) {
                        out.push((t.id, other.id()));
                    }
                }
            }
        }
        out
    }

    /// Changes an item in place.
    pub(crate) fn update(&mut self, id: ItemId, f: impl FnOnce(&mut Item)) {
        if let Some((track, _)) = self.locate(id)
            && let Some(t) = self.seq.track_mut(&track)
        {
            t.items.update(&id, f);
        }
    }

    /// Takes an item out of the working copy.
    pub(crate) fn remove(&mut self, id: ItemId) -> Option<(TrackId, Item)> {
        let (track, _) = self.locate(id)?;
        let item = self.seq.track_mut(&track)?.items.remove(&id)?;
        Some((track, item))
    }

    /// Puts an item on a track of the working copy.
    pub(crate) fn insert(&mut self, track: TrackId, item: Item) {
        if let Some(t) = self.seq.track_mut(&track) {
            t.items.insert(item);
        }
    }

    /// Notes that the command itself moved `id` (its parent does not carry it).
    pub(crate) fn mark_moved(&mut self, id: ItemId) {
        self.moved.insert(id);
    }

    /// Notes that `group` lost members by `unlink` (a single member left is unlinked).
    pub(crate) fn mark_left(&mut self, group: GroupId) {
        self.left_groups.insert(group);
    }

    /// The first item of `track` that intersects `[s, e)` (other than `except`).
    pub(crate) fn blocker(
        &self,
        track: TrackId,
        s: i64,
        e: i64,
        except: Option<ItemId>,
    ) -> Option<ItemId> {
        let t = self.seq.track(&track)?;
        t.items
            .overlapping(time(s), time(e))
            .map(Item::id)
            .find(|id| Some(*id) != except)
    }

    /// Items of `track` that intersect `[s, e)`, in start order.
    fn overlapping(&self, track: TrackId, s: i64, e: i64) -> Vec<ItemId> {
        self.seq.track(&track).map_or_else(Vec::new, |t| {
            t.items
                .overlapping(time(s), time(e))
                .map(Item::id)
                .collect()
        })
    }

    /// The item of `track` that strictly contains `t` (`start < t < end`).
    pub(crate) fn spanning(&self, track: TrackId, t: i64) -> Option<ItemId> {
        let tr = self.seq.track(&track)?;
        let item = tr.items.at(time(t))?;
        (item.start().flicks() < t).then(|| item.id())
    }

    // ----- tracks ----------------------------------------------------------------------------

    /// A new track of `above`'s kind directly above it (INV-42), named like P1, never magnetic.
    pub(crate) fn new_track_above(&mut self, ids: &mut dyn Ids, above: TrackId) -> TrackId {
        let kind = self.seq.track(&above).map_or(TrackKind::Video, |t| t.kind);
        let id: TrackId = mint(ids);
        let name = next_track_name(&self.seq, kind);
        let index = self.seq.track_order.index_of(&above).unwrap_or(0);
        self.seq.insert_track(index, Track::new(id, kind, name));
        self.created_tracks.push(id);
        self.above.entry(above).or_default().push(id);
        id
    }

    /// A track directly above `track` that is free over `[s, e)`: one this edit created above
    /// it, or a new one.
    pub(crate) fn free_track_above(
        &mut self,
        ids: &mut dyn Ids,
        track: TrackId,
        s: i64,
        e: i64,
    ) -> TrackId {
        let reusable = self.above.get(&track).cloned().unwrap_or_default();
        for t in reusable {
            if self.blocker(t, s, e, None).is_none() {
                return t;
            }
        }
        self.new_track_above(ids, track)
    }

    // ----- splits and cuts (INV-43, INV-44) ------------------------------------------------------

    /// Splits `item` and its linked partners at `at`, which must lie strictly inside them
    /// (members that do not contain it are left alone). Left halves keep their IDs; right
    /// halves get new IDs and, when there are several, a new group. Children at or after `at`
    /// move to the right halves (INV-39), item markers too (re-based). IDs: the right group,
    /// then the right halves in member order. Returns the `(left, right)` pairs, `item` first.
    pub(crate) fn split(
        &mut self,
        ids: &mut dyn Ids,
        item: ItemId,
        at: i64,
    ) -> Vec<(ItemId, ItemId)> {
        let members: Vec<(TrackId, ItemId)> = self
            .members(item)
            .into_iter()
            .filter(|(_, id)| {
                self.locate(*id).is_some_and(|(_, i)| {
                    let (s, e) = span(i);
                    s < at && at < e
                })
            })
            .collect();
        self.carve_group(ids, &members, at, at)
    }

    /// Splits the item of `track` that strictly contains `t`, if any (with its partners).
    pub(crate) fn split_at(&mut self, ids: &mut dyn Ids, track: TrackId, t: i64) {
        if let Some(id) = self.spanning(track, t) {
            self.split(ids, id, t);
        }
    }

    /// Splits each of `members` into `[start, t)` (keeps the ID) and `[u, end)` (new ID).
    fn carve_group(
        &mut self,
        ids: &mut dyn Ids,
        members: &[(TrackId, ItemId)],
        t: i64,
        u: i64,
    ) -> Vec<(ItemId, ItemId)> {
        if members.is_empty() {
            return Vec::new();
        }
        let group = (members.len() > 1).then(|| mint::<GroupId>(ids));
        let mut pairs = Vec::with_capacity(members.len());
        for &(track, id) in members {
            let right: ItemId = mint(ids);
            self.carve(track, id, t, u, right, group);
            pairs.push((id, right));
        }
        pairs
    }

    /// One item cut into `[start, t)` and `[u, end)`; the right part is `right` in `group`.
    fn carve(
        &mut self,
        track: TrackId,
        id: ItemId,
        t: i64,
        u: i64,
        right: ItemId,
        group: Option<GroupId>,
    ) {
        let Some((_, item)) = self.locate(id) else {
            return;
        };
        let item = item.clone();
        let still = self.is_still(&item);
        let (s, e) = span(&item);
        let (left_markers, right_markers) = split_markers(item.markers(), t - s, u - s);
        let mut r = item.clone();
        if let Item::Clip(c) = &mut r {
            c.id = right;
            c.start = time(u);
            c.duration = dur(e - u);
            c.linked_group = group;
            c.markers = right_markers;
            if !still {
                c.source_in = time(c.source_in.flicks() + (u - s));
            }
        }
        self.update(id, |i| {
            i.set_duration(dur(t - s));
            *i.markers_mut() = left_markers;
        });
        self.insert(track, r);
        self.retimed.insert(id);
        self.splits.push(SplitReport {
            left: id,
            right,
            at: time(u),
        });
        // INV-39: children at or after the right part's start follow it.
        let children: Vec<ItemId> = self
            .seq
            .tracks
            .values()
            .flat_map(|tr| tr.items.iter())
            .filter(|c| c.attached_to() == Some(id) && c.start().flicks() >= u)
            .map(Item::id)
            .collect();
        for c in children {
            self.update(c, |i| i.set_attached_to(Some(right)));
        }
    }

    /// The end of `id` moves to `t` (keeps its start).
    fn trim_end(&mut self, id: ItemId, t: i64) {
        self.update(id, |i| {
            let s = i.start().flicks();
            i.set_duration(dur(t - s));
        });
        self.retimed.insert(id);
    }

    /// The start of `id` moves to `u` (keeps its end; the source advances, markers re-base).
    fn trim_start(&mut self, id: ItemId, u: i64) {
        let still = self.locate(id).is_some_and(|(_, i)| self.is_still(i));
        self.update(id, |i| {
            let (s, e) = span(i);
            let cut = u - s;
            let (_, kept) = split_markers(i.markers(), cut, cut);
            *i.markers_mut() = kept;
            i.set_start(time(u));
            i.set_duration(dur(e - u));
            if !still && let Some(c) = i.as_clip_mut() {
                c.source_in = time(c.source_in.flicks() + cut);
            }
        });
        self.retimed.insert(id);
    }

    /// INV-44: cuts `[t, u)` out of `track`. Items inside are removed, items straddling an end
    /// are trimmed to it, an item spanning the range is split around it (the left part keeps
    /// its ID). Every member of an affected item's group gets the same cut on its own track.
    /// IDs: per spanning item, in start order, its right parts' group and then the right parts.
    pub(crate) fn cut(&mut self, ids: &mut dyn Ids, track: TrackId, t: i64, u: i64) {
        if u <= t {
            return;
        }
        let mut done: BTreeSet<ItemId> = BTreeSet::new();
        for id in self.overlapping(track, t, u) {
            if done.contains(&id) {
                continue;
            }
            let members = self.members(id);
            done.extend(members.iter().map(|(_, m)| *m));
            let mut spanning = Vec::new();
            for &(tr, m) in &members {
                let Some((_, item)) = self.locate(m) else {
                    continue;
                };
                let (s, e) = span(item);
                if e <= t || s >= u {
                    continue;
                }
                match (s < t, e > u) {
                    (false, false) => {
                        self.remove(m);
                    }
                    (true, true) => spanning.push((tr, m)),
                    (true, false) => self.trim_end(m, t),
                    (false, true) => self.trim_start(m, u),
                }
            }
            self.carve_group(ids, &spanning, t, u);
        }
    }

    // ----- ripple (INV-30 to INV-33) -------------------------------------------------------------

    /// One ripple: every item at or after `theta` on the rippled tracks moves by `f(start)`.
    /// The rippled tracks are `ripple_tracks` plus every track holding a linked partner of an
    /// item that shifts (INV-31), or every track with `all`. A shift of the magnetic track by a
    /// ripple that does not start on it is recorded as `Invalid` (INV-33); collisions surface
    /// in `finish` (INV-32). Children follow (INV-36). Returns the rippled tracks.
    pub(crate) fn ripple(
        &mut self,
        ripple_tracks: &[TrackId],
        all: bool,
        theta: i64,
        f: &dyn Fn(i64) -> i64,
    ) -> BTreeSet<TrackId> {
        let mut rippled: BTreeSet<TrackId> = if all {
            self.seq.tracks.keys().copied().collect()
        } else {
            ripple_tracks.iter().copied().collect()
        };
        let shifts = |item: &Item| f(item.start().flicks()) != 0;
        if !all {
            let mut group_tracks: BTreeMap<GroupId, BTreeSet<TrackId>> = BTreeMap::new();
            for t in self.seq.tracks.values() {
                for item in t.items.iter() {
                    if let Some(g) = item.linked_group() {
                        group_tracks.entry(g).or_default().insert(t.id);
                    }
                }
            }
            let mut queue: Vec<TrackId> = rippled.iter().copied().collect();
            while let Some(tid) = queue.pop() {
                let Some(track) = self.seq.track(&tid) else {
                    continue;
                };
                let reached: Vec<TrackId> = track
                    .items
                    .iter()
                    .rev()
                    .take_while(|i| i.start().flicks() >= theta)
                    .filter(|i| shifts(i))
                    .filter_map(Item::linked_group)
                    .flat_map(|g| group_tracks.get(&g).into_iter().flatten().copied())
                    .collect();
                for t in reached {
                    if rippled.insert(t) {
                        queue.push(t);
                    }
                }
            }
        }
        if let Some(mag) = self.magnetic()
            && rippled.contains(&mag)
            && !ripple_tracks.contains(&mag)
            && self.seq.track(&mag).is_some_and(|t| {
                t.items
                    .iter()
                    .rev()
                    .take_while(|i| i.start().flicks() >= theta)
                    .any(|i| shifts(i))
            })
        {
            let name = self.track_name(mag);
            self.defer(self.invalid(format!(
                "the main track `{name}` cannot open or close a gap here; edit on the main \
                 track instead"
            )));
        }
        let mut deltas: BTreeMap<ItemId, i64> = BTreeMap::new();
        for tid in &rippled {
            let Some(track) = self.seq.track(tid) else {
                continue;
            };
            let shifted: Vec<(ItemId, i64)> = track
                .items
                .iter()
                .rev()
                .take_while(|i| i.start().flicks() >= theta)
                .filter_map(|i| {
                    let d = f(i.start().flicks());
                    (d != 0).then(|| (i.id(), d))
                })
                .collect();
            if shifted.is_empty() {
                continue;
            }
            if let Some(t) = self.seq.track_mut(tid) {
                for (id, d) in &shifted {
                    t.items.update(id, |i| {
                        let s = i.start().flicks();
                        i.set_start(time(s + d));
                    });
                }
            }
            for (id, d) in shifted {
                deltas.insert(id, d);
                self.moved.insert(id);
            }
        }
        self.follow(&deltas, &rippled, &BTreeSet::new());
        rippled
    }

    /// INV-36: children of items translated in one phase by `deltas` translate with them,
    /// unless they are on a track the phase decided (`decided_tracks`), the phase moved them
    /// (`decided_items` or `deltas`), or their parent is now on their track.
    pub(crate) fn follow(
        &mut self,
        deltas: &BTreeMap<ItemId, i64>,
        decided_tracks: &BTreeSet<TrackId>,
        decided_items: &BTreeSet<ItemId>,
    ) {
        if deltas.is_empty() {
            return;
        }
        let mut moves: Vec<(ItemId, i64)> = Vec::new();
        for t in self.seq.tracks.values() {
            if decided_tracks.contains(&t.id) {
                continue;
            }
            for item in t.items.iter() {
                let Some(parent) = item.attached_to() else {
                    continue;
                };
                let Some(&d) = deltas.get(&parent) else {
                    continue;
                };
                if d == 0 || decided_items.contains(&item.id()) || deltas.contains_key(&item.id()) {
                    continue;
                }
                if self.locate(parent).is_some_and(|(pt, _)| pt == t.id) {
                    continue;
                }
                moves.push((item.id(), d));
            }
        }
        for (id, d) in moves {
            self.update(id, |i| {
                let s = i.start().flicks();
                i.set_start(time(s + d));
            });
            self.followers.insert(id);
        }
    }

    /// Closes the space that `removed` items of the magnetic track left (a lift or a delete):
    /// one ripple with the magnetic track as ripple track and `f(t) = −(total length of removed
    /// items that ended at or before t)`.
    pub(crate) fn close_magnetic(&mut self, removed: &[(TrackId, Item)], all: bool) {
        let Some(mag) = self.magnetic() else {
            return;
        };
        let mut gone: Vec<(i64, i64)> = removed
            .iter()
            .filter(|(t, _)| *t == mag)
            .map(|(_, i)| span(i))
            .collect();
        if gone.is_empty() {
            return;
        }
        gone.sort_unstable();
        let theta = gone[0].0;
        let f = closing(gone.iter().map(|&(s, e)| (e, e - s)));
        self.ripple(&[mag], all, theta, &f);
    }

    // ----- placing movers (moves, duplicates) ----------------------------------------------------

    /// Places items that are not in the working copy (lifted movers, new copies) at their new
    /// starts with `mode` (spec §3.6, §4.2 step 3). Items landing on the magnetic track form one
    /// contiguous block; `insert` splits and ripples there (or, with no block, on `target` over
    /// the movers landing on it); linked partners on other tracks rely on that ripple and fail
    /// like `reject` when they still overlap. Children of the movers follow them.
    pub(crate) fn place_movers(
        &mut self,
        ids: &mut dyn Ids,
        movers: Vec<Landing>,
        mode: PlacementMode,
        target: TrackId,
        all: bool,
    ) -> Vec<(ItemId, TrackId, i64)> {
        let mag = self.magnetic();
        let range = |ls: &[&Landing]| -> Option<(i64, i64)> {
            let s = ls.iter().map(|l| l.start).min()?;
            let e = ls.iter().map(|l| l.end()).max()?;
            Some((s, e))
        };
        let block: Vec<&Landing> = movers.iter().filter(|l| Some(l.dest) == mag).collect();
        let block_range = range(&block);
        let mut decided: BTreeSet<TrackId> = BTreeSet::new();
        let mut dest: BTreeMap<ItemId, TrackId> =
            movers.iter().map(|l| (l.item.id(), l.dest)).collect();
        match mode {
            PlacementMode::Insert => {
                let (track, opening) = match (mag, block_range) {
                    (Some(m), Some(r)) => (m, Some(r)),
                    _ => {
                        let on_target: Vec<&Landing> =
                            movers.iter().filter(|l| l.dest == target).collect();
                        (target, range(&on_target))
                    }
                };
                if let Some((s, e)) = opening {
                    self.split_at(ids, track, s);
                    let d = e - s;
                    decided = self.ripple(&[track], all, s, &move |_| d);
                }
            }
            PlacementMode::Overwrite => {
                if let (Some(m), Some((s, e))) = (mag, block_range) {
                    self.cut(ids, m, s, e);
                }
                for l in movers.iter().filter(|l| Some(l.dest) != mag) {
                    self.cut(ids, l.dest, l.start, l.end());
                }
            }
            PlacementMode::NewTrack => {
                if let (Some(m), Some((s, e))) = (mag, block_range)
                    && self.blocker(m, s, e, None).is_some()
                {
                    let up = self.free_track_above(ids, m, s, e);
                    for l in &block {
                        dest.insert(l.item.id(), up);
                    }
                }
                for l in movers.iter().filter(|l| Some(l.dest) != mag) {
                    if self.blocker(l.dest, l.start, l.end(), None).is_some() {
                        let up = self.free_track_above(ids, l.dest, l.start, l.end());
                        dest.insert(l.item.id(), up);
                    }
                }
            }
            PlacementMode::Reject => {}
        }
        let mut placed = Vec::with_capacity(movers.len());
        let mut deltas = BTreeMap::new();
        let mover_ids: BTreeSet<ItemId> = movers.iter().map(|l| l.item.id()).collect();
        for l in movers {
            let id = l.item.id();
            let track = dest.get(&id).copied().unwrap_or(l.dest);
            if let Some(other) = self.blocker(track, l.start, l.end(), None) {
                let name = self.track_name(track);
                let hint = if mode == PlacementMode::Reject {
                    "; choose a free time or use mode \"new_track\""
                } else {
                    ""
                };
                self.defer(self.invalid(format!(
                    "item `{id}` would overlap item `{other}` on track `{name}`{hint}"
                )));
            }
            if l.start < 0 {
                self.defer(self.invalid(format!("item `{id}` would start before zero")));
            }
            let mut item = l.item;
            item.set_start(time(l.start));
            self.insert(track, item);
            self.moved.insert(id);
            deltas.insert(id, l.start - l.old);
            placed.push((id, track, l.start));
        }
        self.follow(&deltas, &decided, &mover_ids);
        placed
    }

    // ----- finishing ---------------------------------------------------------------------------

    /// Settles attachments, then judges the plan: `Locked`, then the recorded error, then
    /// overlaps and the magnetic track's gaps (module docs).
    pub(crate) fn finish(mut self, ids: &mut dyn Ids) -> Result<Plan, CmdError> {
        self.settle_children(ids);
        self.dissolve_orphans();
        self.check_locks()?;
        if let Some(e) = self.deferred.take() {
            return Err(e);
        }
        self.check_layout()?;
        let after = self.seq;
        let splits = self
            .splits
            .into_iter()
            .filter(|s| after.find_item(&s.right).is_some())
            .collect();
        Ok(Plan {
            sequence: after.id,
            before: self.before,
            after,
            splits,
            created_tracks: self.created_tracks,
        })
    }

    /// INV-36 to INV-38 after the phases (module docs).
    fn settle_children(&mut self, ids: &mut dyn Ids) {
        let mut detach = Vec::new();
        let mut clamp = Vec::new();
        for t in self.seq.tracks.values() {
            for c in t.items.iter() {
                let Some(p) = c.attached_to() else {
                    continue;
                };
                let Some((pt, parent)) = self.locate(p) else {
                    detach.push(c.id());
                    continue;
                };
                if pt == t.id {
                    detach.push(c.id());
                    continue;
                }
                let (ps, pe) = span(parent);
                let s = c.start().flicks();
                if ps <= s && s < pe {
                    continue;
                }
                if self.retimed.contains(&p) && !self.moved.contains(&c.id()) {
                    clamp.push((c.id(), s.clamp(ps, (pe - self.frame).max(ps))));
                } else {
                    detach.push(c.id());
                }
            }
        }
        for id in detach {
            self.update(id, |i| i.set_attached_to(None));
        }
        for (id, s) in clamp {
            self.update(id, |i| i.set_start(time(s)));
            self.followers.insert(id);
        }
        // INV-37: a child that now overlaps an item of its track goes up.
        let order: BTreeMap<TrackId, usize> = self
            .seq
            .track_order
            .iter()
            .enumerate()
            .map(|(n, t)| (*t, n))
            .collect();
        let mut followers: Vec<(usize, i64, ItemId)> = self
            .followers
            .iter()
            .filter_map(|id| {
                let (t, i) = self.locate(*id)?;
                Some((order.get(&t).copied().unwrap_or(0), i.start().flicks(), *id))
            })
            .collect();
        followers.sort_unstable();
        for (_, _, id) in followers {
            let Some((track, item)) = self.locate(id) else {
                continue;
            };
            let (s, e) = span(item);
            let crowded = self.seq.track(&track).is_some_and(|t| {
                t.items.iter().any(|o| {
                    let (os, oe) = span(o);
                    o.id() != id && os < e && s < oe
                })
            });
            if crowded {
                let up = self.free_track_above(ids, track, s, e);
                if let Some((_, item)) = self.remove(id) {
                    self.insert(up, item);
                }
            }
        }
    }

    /// A group that `unlink` left with one member is dissolved (P1 orphan rule).
    fn dissolve_orphans(&mut self) {
        let groups = std::mem::take(&mut self.left_groups);
        for g in groups {
            let members: Vec<ItemId> = self
                .seq
                .tracks
                .values()
                .flat_map(|t| t.items.iter())
                .filter(|i| i.linked_group() == Some(g))
                .map(Item::id)
                .collect();
            if let [only] = members.as_slice() {
                self.update(*only, |i| i.set_linked_group(None));
            }
        }
    }

    /// INV-27: `Locked` for the first locked track (display order) whose items changed.
    fn check_locks(&self) -> Result<(), CmdError> {
        for t in self.before.tracks_in_order().filter(|t| t.locked) {
            let changed = self
                .seq
                .track(&t.id)
                .is_none_or(|now| !now.items.ptr_eq(&t.items) && now.items != t.items);
            if changed {
                return Err(CmdError::locked(EntityKind::Track, t.id));
            }
        }
        Ok(())
    }

    /// INV-1/INV-2/INV-32 (no overlaps, no item before 0) and INV-7 (the magnetic track is
    /// gap-free) on the tracks the edit changed.
    fn check_layout(&self) -> Result<(), CmdError> {
        for t in self.seq.tracks_in_order() {
            if self
                .before
                .track(&t.id)
                .is_some_and(|b| b.items.ptr_eq(&t.items))
            {
                continue;
            }
            if let Some((a, b)) = t.items.first_overlap() {
                return Err(self.invalid(format!(
                    "the edit would make item `{a}` overlap item `{b}` on track `{}`",
                    t.name
                )));
            }
            if let Some(i) = t.items.iter().next()
                && i.start() < Time::ZERO
            {
                return Err(self.invalid(format!(
                    "the edit would make item `{}` start before zero on track `{}`",
                    i.id(),
                    t.name
                )));
            }
            if t.magnetic
                && let Some((from, to)) = first_gap(t)
            {
                return Err(self.invalid(format!(
                    "the main track `{}` cannot open or close a gap here (from {from} to {to} \
                     flicks); edit on the main track instead",
                    t.name
                )));
            }
        }
        Ok(())
    }
}

/// An item placed by [`Edit::place_movers`]: its new start and destination track; `old` is
/// its start before the edit (what its children follow).
#[derive(Clone, Debug)]
pub(crate) struct Landing {
    pub(crate) item: Item,
    pub(crate) dest: TrackId,
    pub(crate) start: i64,
    pub(crate) old: i64,
}

impl Landing {
    fn end(&self) -> i64 {
        self.start + self.item.duration().flicks()
    }
}

/// The first gap of a track (the space before its first item counts), as `(from, to)` flicks.
pub(crate) fn first_gap(track: &Track) -> Option<(i64, i64)> {
    let mut expected = 0;
    for item in track.items.iter() {
        let (s, e) = span(item);
        if s > expected {
            return Some((expected, s));
        }
        expected = expected.max(e);
    }
    None
}

/// The closing shift `f(t) = −(total length of the spans that ended at or before t)` for spans
/// given as `(end, length)`.
pub(crate) fn closing(spans: impl Iterator<Item = (i64, i64)>) -> impl Fn(i64) -> i64 {
    let mut ends: Vec<(i64, i64)> = spans.collect();
    ends.sort_unstable();
    let mut acc = 0;
    let prefix: Vec<(i64, i64)> = ends
        .into_iter()
        .map(|(e, len)| {
            acc += len;
            (e, acc)
        })
        .collect();
    move |t: i64| {
        let n = prefix.partition_point(|(e, _)| *e <= t);
        if n == 0 { 0 } else { -prefix[n - 1].1 }
    }
}

/// Item markers cut at the item-relative times `t <= u`: markers before `t` stay left; markers
/// at or after `u` go right, re-based to start there; markers in between are cut out with the
/// content they mark.
fn split_markers(
    markers: &cutweave_model::Markers,
    t: i64,
    u: i64,
) -> (cutweave_model::Markers, cutweave_model::Markers) {
    let mut left = cutweave_model::Markers::new();
    let mut right = cutweave_model::Markers::new();
    for m in markers.values() {
        let at = m.t.flicks();
        if at < t {
            left.insert(m.id, m.clone());
        } else if at >= u {
            let mut moved = m.clone();
            moved.t = time(at - u);
            right.insert(moved.id, moved);
        }
    }
    (left, right)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_sums_the_spans_ended_by_then() {
        let f = closing([(30, 10), (60, 5)].into_iter());
        assert_eq!(f(0), 0);
        assert_eq!(f(29), 0);
        assert_eq!(f(30), -10);
        assert_eq!(f(59), -10);
        assert_eq!(f(60), -15);
        assert_eq!(f(1000), -15);
    }

    #[test]
    fn modes_resolve_with_the_overlap_alias() {
        let d = PlacementMode::Reject;
        assert_eq!(resolve_mode(None, None, d).ok(), Some(d));
        assert_eq!(
            resolve_mode(None, Some(OverlapPolicy::NewTrack), d).ok(),
            Some(PlacementMode::NewTrack)
        );
        assert_eq!(
            resolve_mode(Some(PlacementMode::Insert), None, d).ok(),
            Some(PlacementMode::Insert)
        );
        assert_eq!(
            resolve_mode(
                Some(PlacementMode::NewTrack),
                Some(OverlapPolicy::NewTrack),
                d
            )
            .ok(),
            Some(PlacementMode::NewTrack)
        );
        assert!(resolve_mode(Some(PlacementMode::Insert), Some(OverlapPolicy::Reject), d).is_err());
    }
}
