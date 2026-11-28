// SPDX-License-Identifier: GPL-3.0-or-later
//! `item.move`, `item.delete` and `item.duplicate` (WP-1B.06, WP-2A.03; timeline spec §4.2,
//! §4.3, §4.8; ADR-0023), on the placement core ([`crate::timeline::placement`]).
//!
//! **Move** = lift, then place (spec §4.2). The movers (the given items and, unless `unlink`,
//! their linked partners) leave their tracks; on the magnetic main track the space they leave
//! closes. The primary (the first given item) goes to `to_track` at `start` (or its old start
//! plus `delta`); given items on its track keep their offsets to it, partners stay on their own
//! tracks, aligned. Movers that land on the magnetic track form one block in their original
//! order; its start is read in the lifted layout and clamped into `[0, end]`, and a group with
//! a member there moves with that member. Then the movers are placed with the mode (default
//! `insert` on the magnetic track, `reject` elsewhere). Children of the movers follow them.
//!
//! **Delete** removes the items and their partners; the magnetic track closes the space, free
//! tracks keep it; children of removed items are detached and stay (D16).
//!
//! **Duplicate** copies the items and their partners (new groups, fresh marker IDs). Copies go
//! to their original tracks at their start plus `offset` (default: the selection's span); copies
//! of magnetic-track items are inserted as one block right after the last copied one.
//!
//! **IDs** (INV-23). Move: the splits of an insert or the cuts of an overwrite, then new tracks
//! as placements need them. Delete: none. Duplicate: per copied item in output order, the new
//! group of its source's group (first time only), the copy, then its markers (by marker ID);
//! then the placement's splits, cuts and tracks.

use std::collections::{BTreeMap, BTreeSet};

use cutweave_commands::{CmdError, Command, QueryCtx, Tx, command, wire};
use cutweave_model::{
    EntityKind, GroupId, Item, ItemId, MarkerId, Markers, Project, Time, TimeInput, TrackId,
    TrackKind,
};

use crate::{
    report::EditReport,
    timeline::{
        OverlapPolicy, PlacementMode,
        edit::{given_items, remove_with_partners},
        magnetic::{BlockItem, Unit, block_end, clamp_to_end, offset_in_block, pack},
        placement::{DryIds, Edit, Ids, Landing, Plan, resolve_mode, span},
    },
    track::check_editable_kind,
    util::{resolve_frame_time, resolve_sample_time},
};

// ----- item.move ---------------------------------------------------------------------------

/// Where one item ended up.
#[wire]
pub struct MovedItem {
    /// The item.
    pub item: ItemId,
    /// The track it is on now.
    pub track: TrackId,
    /// Its new start.
    pub start: Time,
}

/// Result of `item.move`.
#[wire]
pub struct ItemMoveOutput {
    /// The moved items: the given ones in order, each followed by its linked partners.
    pub items: Vec<MovedItem>,
    /// Tracks created by the placement, in creation order.
    pub created_tracks: Vec<TrackId>,
    /// What changed (timeline spec §3.1).
    pub report: EditReport,
}

/// Moves items to a new start (`start`, or `delta` from where the first one is), optionally
/// onto another track of the same kind and sequence. Give `item`, or `items` (the first is the
/// primary; the others keep their offsets to it). Linked partners move by the same amount on
/// their own tracks (`unlink: true` moves only the given items and takes them out of their
/// groups). On the magnetic main track the space an item leaves closes, and an item moved onto
/// it is inserted (default; the start is read after the lift and clamped to the track's end).
/// Elsewhere an overlap is `Invalid` by default; `mode` (or its alias `overlap`) picks
/// `new_track`, `overwrite` or `insert`. Children attached to a moved item follow it. Times
/// snap to the frame grid (`fine: true`: the audio sample grid, audio-only moves).
#[command(
    name = "item.move",
    summary = "Move items in time and/or to another track (linked partners follow)",
    undoable,
    mcp = "core",
    annotations(destructive = false, idempotent = false)
)]
pub struct ItemMove {
    /// The item (or `items`).
    pub item: Option<ItemId>,
    /// Items; the first leads.
    pub items: Option<Vec<ItemId>>,
    /// New start, `>= 0` (or `delta`).
    pub start: Option<TimeInput>,
    /// Offset of the start.
    pub delta: Option<TimeInput>,
    /// Target track (default: stay).
    pub to_track: Option<TrackId>,
    /// Move only the given items, unlinked.
    pub unlink: Option<bool>,
    /// Alias of `mode`.
    pub overlap: Option<OverlapPolicy>,
    /// Default: `insert` on the main track, else `reject`.
    pub mode: Option<PlacementMode>,
    /// Ripples reach every track.
    pub ripple_all_tracks: Option<bool>,
    /// Audio sample grid.
    pub fine: Option<bool>,
}

struct MovePlan {
    plan: Plan,
    items: Vec<MovedItem>,
}

/// `item` or `items`, exactly one.
fn one_or_many(item: Option<ItemId>, items: Option<&[ItemId]>) -> Result<Vec<ItemId>, CmdError> {
    match (item, items) {
        (Some(i), None) => Ok(vec![i]),
        (None, Some([])) => Err(CmdError::invalid("/items", "give at least one item")),
        (None, Some(list)) => Ok(list.to_vec()),
        _ => Err(CmdError::invalid(
            "",
            "give exactly one of `item` or `items`",
        )),
    }
}

/// `Invalid{path:"/fine"}` unless every item is on an audio track (INV-26).
fn check_fine(e: &Edit<'_>, items: &[ItemId], tracks: &[TrackId]) -> Result<(), CmdError> {
    let audio = |t: &TrackId| e.seq.track(t).is_some_and(|t| t.kind == TrackKind::Audio);
    let linked_to_video = items
        .iter()
        .flat_map(|id| e.members(*id))
        .any(|(t, _)| !audio(&t));
    if linked_to_video || !tracks.iter().all(audio) {
        return Err(CmdError::invalid(
            "/fine",
            "a fine edit needs audio items that are not linked to video",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_lines)] // one plan, phase by phase (spec §4.2)
fn plan_move(cmd: &ItemMove, doc: &Project, ids: &mut dyn Ids) -> Result<MovePlan, CmdError> {
    let given = one_or_many(cmd.item, cmd.items.as_deref())?;
    let (pointer, input, is_delta) = match (cmd.start, cmd.delta) {
        (Some(s), None) => ("/start", s, false),
        (None, Some(d)) => ("/delta", d, true),
        _ => {
            return Err(CmdError::invalid(
                "",
                "give exactly one of `start` (new start) or `delta` (offset)",
            ));
        }
    };
    let (sequence, given) = given_items(doc, &given)?;
    let seq = &doc.sequences[&sequence];
    let unlink = cmd.unlink.unwrap_or(false);
    let fine = cmd.fine.unwrap_or(false);
    let all = cmd.ripple_all_tracks.unwrap_or(false);
    let primary = given[0];
    let (primary_track, primary_old) = seq
        .find_item(&primary)
        .map(|(t, i)| (t.id, i.start().flicks()))
        .ok_or_else(|| CmdError::not_found(EntityKind::Item, primary))?;
    let target = match cmd.to_track {
        None => primary_track,
        Some(t) => {
            let Some(track) = seq.track(&t) else {
                if doc.sequences.values().any(|s| s.track(&t).is_some()) {
                    return Err(CmdError::invalid(
                        "/to_track",
                        format!("track `{t}` is not in the item's sequence"),
                    ));
                }
                return Err(CmdError::not_found(EntityKind::Track, t));
            };
            let from = &seq.tracks[&primary_track];
            if track.kind != from.kind {
                return Err(CmdError::invalid(
                    "/to_track",
                    format!("a {} item cannot move to a {} track", from.kind, track.kind),
                ));
            }
            check_editable_kind("/to_track", track.kind)?;
            t
        }
    };
    let mut e = Edit::new(doc, sequence, pointer)?;
    let track_of = |id: &ItemId| e.locate(*id).map(|(t, _)| t);
    if cmd.to_track.is_some() && given.iter().any(|id| track_of(id) != Some(primary_track)) {
        return Err(CmdError::unsupported(
            "moving items of several tracks to another track is not supported; move them \
             without `to_track`",
        ));
    }
    // The movers, in output order: each given item, then its partners.
    let mut movers: Vec<ItemId> = Vec::new();
    for id in &given {
        let group = if unlink {
            vec![(primary_track, *id)]
        } else {
            e.members(*id)
        };
        for (_, m) in group {
            if !movers.contains(&m) {
                movers.push(m);
            }
        }
    }
    for m in &movers {
        if let Some(t) = track_of(m).and_then(|t| e.seq.track(&t)) {
            check_editable_kind("/items", t.kind)?;
        }
    }
    let dest_of = |e: &Edit<'_>, id: &ItemId| -> Option<TrackId> {
        let t = e.locate(*id).map(|(t, _)| t)?;
        Some(if given.contains(id) && t == primary_track {
            target
        } else {
            t
        })
    };
    let dests: BTreeMap<ItemId, TrackId> = movers
        .iter()
        .filter_map(|m| dest_of(&e, m).map(|d| (*m, d)))
        .collect();
    let rate = seq.format.frame_rate;
    let resolved = if fine {
        let mut tracks: Vec<TrackId> = dests.values().copied().collect();
        tracks.extend(movers.iter().filter_map(track_of));
        check_fine(&e, &given, &tracks)?;
        resolve_sample_time(pointer, input, rate, seq.format.sample_rate)?
    } else {
        resolve_frame_time(pointer, input, rate)?
    }
    .flicks();
    if !is_delta && resolved < 0 {
        return Err(CmdError::invalid(pointer, "the time must not be negative"));
    }
    let requested = if is_delta {
        primary_old + resolved
    } else {
        resolved
    };
    let target_magnetic = e.seq.track(&target).is_some_and(|t| t.magnetic);
    let default = if target_magnetic {
        PlacementMode::Insert
    } else {
        PlacementMode::Reject
    };
    let mode = resolve_mode(cmd.mode, cmd.overlap, default)?;
    if requested < 0 && !target_magnetic {
        return Err(CmdError::invalid(
            pointer,
            format!("item `{primary}` would start before zero"),
        ));
    }

    // 1. Lift: the movers leave their tracks; the magnetic track closes.
    let mut lifted: Vec<(TrackId, Item)> = Vec::new();
    for m in &movers {
        if let Some((t, mut item)) = e.remove(*m) {
            if unlink && let Some(g) = item.linked_group() {
                e.mark_left(g);
                item.set_linked_group(None);
            }
            lifted.push((t, item));
        }
    }
    e.close_magnetic(&lifted, all);

    // 2. Target: the magnetic block, then everything else by its unit's shift.
    let mag = e.magnetic();
    let block: Vec<BlockItem> = lifted
        .iter()
        .filter(|(_, i)| mag.is_some() && dests.get(&i.id()).copied() == mag)
        .map(|(_, i)| BlockItem {
            id: i.id(),
            group: i.linked_group(),
            old: i.start().flicks(),
            len: i.duration().flicks(),
        })
        .collect();
    let (block_starts, shifts) = match (mag, block.first()) {
        (Some(m), Some(first)) => {
            let anchor = if block.iter().any(|b| b.id == primary) {
                requested - offset_in_block(&block, primary)
            } else {
                let earliest = block.iter().map(|b| b.old).min().unwrap_or(first.old);
                earliest + (requested - primary_old)
            };
            pack(&block, clamp_to_end(&e, m, anchor))
        }
        _ => (BTreeMap::new(), BTreeMap::new()),
    };
    let primary_group = lifted
        .iter()
        .find(|(_, i)| i.id() == primary)
        .and_then(|(_, i)| i.linked_group());
    let delta = shifts
        .get(&Unit::of(primary, primary_group))
        .copied()
        .unwrap_or(requested - primary_old);
    let landings: Vec<Landing> = lifted
        .into_iter()
        .map(|(from, item)| {
            let id = item.id();
            let old = item.start().flicks();
            let start = block_starts.get(&id).copied().unwrap_or_else(|| {
                old + shifts
                    .get(&Unit::of(id, item.linked_group()))
                    .copied()
                    .unwrap_or(delta)
            });
            Landing {
                dest: dests.get(&id).copied().unwrap_or(from),
                item,
                start,
                old,
            }
        })
        .collect();

    // 3. Place.
    let placed = e.place_movers(ids, landings, mode, target, all);
    let plan = e.finish(ids)?;
    let items = placed
        .into_iter()
        .map(|(id, _, _)| {
            let (track, start) = plan
                .after
                .find_item(&id)
                .map_or((target, Time::ZERO), |(t, i)| (t.id, i.start()));
            MovedItem {
                item: id,
                track,
                start,
            }
        })
        .collect();
    Ok(MovePlan { plan, items })
}

impl Command for ItemMove {
    type Output = ItemMoveOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        plan_move(self, ctx.doc(), &mut DryIds::default()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<ItemMoveOutput, CmdError> {
        let doc = tx.doc().clone();
        let MovePlan { plan, items } = plan_move(self, &doc, tx)?;
        let report = plan.commit(tx)?;
        Ok(ItemMoveOutput {
            items,
            created_tracks: plan.created_tracks,
            report,
        })
    }
}

// ----- item.delete -------------------------------------------------------------------------

/// Result of `item.delete`.
#[wire]
pub struct ItemDeleteOutput {
    /// Every deleted item: the requested ones in request order, then their linked partners.
    pub deleted: Vec<ItemId>,
    /// What changed (timeline spec §3.1).
    pub report: EditReport,
}

/// Deletes items and their linked partners. On free tracks the space stays; on the magnetic
/// main track it closes (what follows moves left, and the tracks of its linked partners
/// follow). Children attached to a deleted item are detached and keep their place. All or
/// nothing: if the edit would change a locked track it fails with `Locked`. Unknown items are
/// `NotFound`; repeated IDs are deleted once; all items must be in one sequence.
#[command(
    name = "item.delete",
    summary = "Delete items and their linked partners (the main track closes up)",
    undoable,
    mcp = "core",
    annotations(destructive = true, idempotent = false)
)]
pub struct ItemDelete {
    /// The items to delete, at least one.
    pub items: Vec<ItemId>,
}

fn plan_delete(
    cmd: &ItemDelete,
    doc: &Project,
    ids: &mut dyn Ids,
) -> Result<(Plan, Vec<ItemId>), CmdError> {
    let (sequence, given) = given_items(doc, &cmd.items)?;
    let mut e = Edit::new(doc, sequence, "/items")?;
    let (deleted, removed) = remove_with_partners(&mut e, &given)?;
    e.close_magnetic(&removed, false);
    Ok((e.finish(ids)?, deleted))
}

impl Command for ItemDelete {
    type Output = ItemDeleteOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        plan_delete(self, ctx.doc(), &mut DryIds::default()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<ItemDeleteOutput, CmdError> {
        let doc = tx.doc().clone();
        let (plan, deleted) = plan_delete(self, &doc, tx)?;
        let report = plan.commit(tx)?;
        Ok(ItemDeleteOutput { deleted, report })
    }
}

// ----- item.duplicate ----------------------------------------------------------------------

/// One copy.
#[wire]
pub struct DuplicatedItem {
    /// The copied item.
    pub source: ItemId,
    /// Its copy.
    pub copy: ItemId,
}

/// Result of `item.duplicate`.
#[wire]
pub struct ItemDuplicateOutput {
    /// The copies: the given items in order, each followed by its linked partners.
    pub copies: Vec<DuplicatedItem>,
    /// Tracks created by the placement, in creation order.
    pub created_tracks: Vec<TrackId>,
    /// What changed (timeline spec §3.1).
    pub report: EditReport,
}

/// Duplicates items and their linked partners. Each copy goes to its original's track at its
/// start plus `offset` (default: the selection's span, so the copies follow the originals);
/// each copied group becomes a new group, markers get new IDs. Copies of magnetic main-track
/// items are inserted as one block right after the last copied one (`offset` is not allowed
/// then). A copied child stays attached to the copy of its parent when that was copied too,
/// to its original parent when the copy still starts inside it, and is detached otherwise.
/// Overlaps on free tracks are `Invalid` by default; `mode` picks `new_track`, `overwrite` or
/// `insert`.
#[command(
    name = "item.duplicate",
    summary = "Duplicate items and their linked partners after the selection (inserted on the main track)",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = false)
)]
pub struct ItemDuplicate {
    /// The items to copy, at least one, of one sequence.
    pub items: Vec<ItemId>,
    /// Where the copies go relative to the originals (default: the selection's span; not
    /// allowed for magnetic main-track items).
    pub offset: Option<TimeInput>,
    /// Placement mode for the copies (default `insert` with main-track items, else `reject`).
    pub mode: Option<PlacementMode>,
}

#[allow(clippy::too_many_lines)] // one plan, phase by phase (spec §4.8)
fn plan_duplicate(
    cmd: &ItemDuplicate,
    doc: &Project,
    ids: &mut dyn Ids,
) -> Result<(Plan, Vec<DuplicatedItem>), CmdError> {
    let (sequence, given) = given_items(doc, &cmd.items)?;
    let seq = &doc.sequences[&sequence];
    let pointer = if cmd.offset.is_some() {
        "/offset"
    } else {
        "/items"
    };
    let mut e = Edit::new(doc, sequence, pointer)?;
    let mut sources: Vec<(TrackId, Item)> = Vec::new();
    let mut seen = BTreeSet::new();
    for id in &given {
        for (t, m) in e.members(*id) {
            if seen.insert(m)
                && let Some((_, item)) = e.locate(m)
            {
                sources.push((t, item.clone()));
            }
        }
    }
    for (t, _) in &sources {
        if let Some(track) = e.seq.track(t) {
            check_editable_kind("/items", track.kind)?;
        }
    }
    let mag = e.magnetic();
    let on_mag: Vec<ItemId> = sources
        .iter()
        .filter(|(t, _)| Some(*t) == mag)
        .map(|(_, i)| i.id())
        .collect();
    let offset = match cmd.offset {
        Some(_) if !on_mag.is_empty() => {
            return Err(CmdError::invalid(
                "/offset",
                "copies of main-track items go right after them; `offset` is not allowed",
            ));
        }
        Some(input) => resolve_frame_time("/offset", input, seq.format.frame_rate)?.flicks(),
        None => {
            let lo = sources.iter().map(|(_, i)| span(i).0).min().unwrap_or(0);
            let hi = sources.iter().map(|(_, i)| span(i).1).max().unwrap_or(0);
            hi - lo
        }
    };
    let mode = resolve_mode(
        cmd.mode,
        None,
        if on_mag.is_empty() {
            PlacementMode::Reject
        } else {
            PlacementMode::Insert
        },
    )?;

    // The copies (IDs in output order).
    let mut groups: BTreeMap<GroupId, GroupId> = BTreeMap::new();
    let mut copy_of: BTreeMap<ItemId, ItemId> = BTreeMap::new();
    let mut copies: Vec<(TrackId, Item, Item)> = Vec::new();
    for (t, src) in &sources {
        let group = src
            .linked_group()
            .map(|g| *groups.entry(g).or_insert_with(|| e.mint(ids)));
        let id: ItemId = e.mint(ids);
        let mut copy = src.clone();
        copy.set_id(id);
        copy.set_linked_group(group);
        let markers = remint(copy.markers(), &mut e, ids);
        *copy.markers_mut() = markers;
        copy_of.insert(src.id(), id);
        copies.push((*t, src.clone(), copy));
    }
    for (_, _, copy) in &mut copies {
        if let Some(p) = copy.attached_to() {
            let parent = copy_of.get(&p).copied().unwrap_or(p);
            copy.set_attached_to(Some(parent));
        }
    }
    let block: Vec<BlockItem> = copies
        .iter()
        .filter(|(t, _, _)| Some(*t) == mag)
        .map(|(_, src, copy)| BlockItem {
            id: copy.id(),
            group: copy.linked_group(),
            old: src.start().flicks(),
            len: src.duration().flicks(),
        })
        .collect();
    let (block_starts, shifts) = match mag.and_then(|m| block_end(&e, m, &on_mag)) {
        Some(end) if !block.is_empty() => pack(&block, end),
        _ => (BTreeMap::new(), BTreeMap::new()),
    };
    let primary_track = sources
        .first()
        .map(|(t, _)| *t)
        .ok_or_else(|| CmdError::invalid("/items", "give at least one item"))?;
    let mut out = Vec::with_capacity(copies.len());
    let landings: Vec<Landing> = copies
        .into_iter()
        .map(|(t, src, copy)| {
            out.push(DuplicatedItem {
                source: src.id(),
                copy: copy.id(),
            });
            let start = block_starts.get(&copy.id()).copied().unwrap_or_else(|| {
                src.start().flicks()
                    + shifts
                        .get(&Unit::of(copy.id(), copy.linked_group()))
                        .copied()
                        .unwrap_or(offset)
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
    e.place_movers(ids, landings, mode, primary_track, false);
    Ok((e.finish(ids)?, out))
}

/// Copies of item markers with fresh IDs (in marker-ID order).
pub(crate) fn remint(markers: &Markers, e: &mut Edit<'_>, ids: &mut dyn Ids) -> Markers {
    let mut out = Markers::new();
    for m in markers.values() {
        let mut copy = m.clone();
        copy.id = e.mint::<MarkerId>(ids);
        out.insert(copy.id, copy);
    }
    out
}

impl Command for ItemDuplicate {
    type Output = ItemDuplicateOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        plan_duplicate(self, ctx.doc(), &mut DryIds::default()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<ItemDuplicateOutput, CmdError> {
        let doc = tx.doc().clone();
        let (plan, copies) = plan_duplicate(self, &doc, tx)?;
        let report = plan.commit(tx)?;
        Ok(ItemDuplicateOutput {
            copies,
            created_tracks: plan.created_tracks,
            report,
        })
    }
}
