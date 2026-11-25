// SPDX-License-Identifier: GPL-3.0-or-later
//! Property (5) of the harness: the model invariants every committed document keeps (arch
//! §4.3), and "linked groups intact" as the WP-1B.06 edit semantics define it (module docs of
//! [`crate::item`]):
//!
//! - a linked group never spans sequences (checked by [`Project::validate`]);
//! - no linked group has a single member (`unlink: true` on an item whose partner is on a locked track fails with `Locked`): edits that take members out of a group (`unlink`,
//!   `track.remove`) unlink the last one left;
//! - linked partners move, trim, split and are deleted together: after a successful
//!   `item.move` every partner moved by the same offset; after `clip.trim` every partner's
//!   trimmed edge is where the item's is; after `clip.split` every partner that contained the
//!   time was split there too and the right halves share the new group; after `item.delete`
//!   no partner of a deleted item is left;
//! - `unlink: true` (`item.move`, `clip.trim`) takes only the item out of its group and leaves
//!   its partners as they were (a partner that would be alone in the group is unlinked too).
//!
//! Groups need not be time-aligned: documents may link items that never were (and an undo of
//! an unlink can re-link an item whose partner changed since), so only the edit rules are
//! checked.

use std::collections::BTreeMap;

use cutweave_model::{GroupId, Item, ItemId, Project, TrackId};
use serde_json::Value;

use crate::report::EditReport;

/// Checks the invariants of `doc`: [`Project::validate`] (references, timing, bounds, formats,
/// linked groups inside one sequence, ...), no overlapping items on any track and no linked
/// group with a single member.
pub fn check_invariants(doc: &Project) -> Result<(), String> {
    doc.validate()
        .map_err(|e| format!("Project::validate fails: {e}"))?;
    for s in doc.sequences.values() {
        for t in s.tracks.values() {
            if let Some((a, b)) = t.items.first_overlap() {
                return Err(format!("items {a} and {b} overlap on track {}", t.id));
            }
        }
    }
    let mut sizes: BTreeMap<GroupId, (usize, ItemId)> = BTreeMap::new();
    for t in doc.sequences.values().flat_map(|s| s.tracks.values()) {
        for i in t.items.iter() {
            if let Some(g) = i.linked_group() {
                sizes.entry(g).or_insert((0, i.id())).0 += 1;
            }
        }
    }
    if let Some((g, (_, only))) = sizes.iter().find(|(_, (n, _))| *n == 1) {
        return Err(format!("linked group {g} has a single member, {only}"));
    }
    Ok(())
}

/// The rules of a successful edit that changed `before` into `after` with `params` and
/// returned `output`: the linked-group rules of the group-aware edits (`clip.split`,
/// `clip.trim`, `item.move`, `item.delete`, `timeline.ripple_delete`), and for every command
/// (WP-2A.03) that the items of a locked track did not change (INV-28) and that a returned
/// `report` is exactly the difference (INV-21, [`check_edit_report`]).
pub fn check_linked_edit(
    name: &str,
    params: &Value,
    output: &Value,
    before: &Project,
    after: &Project,
) -> Result<(), String> {
    if !name.starts_with("edit.") {
        locked_unchanged(name, before, after)?;
    }
    if output.get("report").is_some_and(Value::is_object) {
        check_edit_report(name, output, before, after)?;
    }
    match name {
        "item.delete" | "timeline.ripple_delete" => deleted_with_partners(output, before, after),
        "item.move" => moved_together(params, before, after),
        "clip.trim" => trimmed_together(params, before, after),
        "clip.split" => split_together(params, output, before, after),
        _ => Ok(()),
    }
}

/// Timeline spec INV-28: a command never changes the items of a track that is locked before
/// and after it (undo, redo and restore replay history and are exempt).
fn locked_unchanged(name: &str, before: &Project, after: &Project) -> Result<(), String> {
    for (sid, s) in &before.sequences {
        for t in s.tracks.values().filter(|t| t.locked) {
            let Some(now) = after.sequences.get(sid).and_then(|s| s.track(&t.id)) else {
                continue;
            };
            if now.locked && now.items != t.items {
                return Err(format!("{name} changed the items of locked track {}", t.id));
            }
        }
    }
    Ok(())
}

/// Timeline spec INV-21: a returned `report` is exactly the difference between the documents
/// (given the command's own splits and created tracks); its split right halves are new, its
/// left halves existed, and its created tracks are new.
///
/// # Errors
///
/// A description of the first mismatch.
pub fn check_edit_report(
    name: &str,
    output: &Value,
    before: &Project,
    after: &Project,
) -> Result<(), String> {
    let reported: EditReport = serde_json::from_value(output["report"].clone())
        .map_err(|e| format!("{name}: the report does not parse: {e}"))?;
    let mut derived = EditReport::default();
    for (sid, s) in &after.sequences {
        let Some(b) = before.sequences.get(sid) else {
            continue;
        };
        let d = EditReport::diff(b, s, &reported.split, &[]);
        derived.created.extend(d.created);
        derived.removed.extend(d.removed);
        derived.moved.extend(d.moved);
        derived.trimmed.extend(d.trimmed);
        derived.relinked.extend(d.relinked);
        derived.replaced.extend(d.replaced);
        derived.toggled.extend(d.toggled);
    }
    derived.split.clone_from(&reported.split);
    derived.created_tracks.clone_from(&reported.created_tracks);
    derived.created.sort();
    derived.removed.sort();
    derived.moved.sort_by_key(|m| m.item);
    derived.trimmed.sort_by_key(|m| m.item);
    derived.relinked.sort_by_key(|m| m.item);
    derived.replaced.sort_by_key(|m| m.item);
    derived.toggled.sort_by_key(|m| m.item);
    if derived != reported {
        return Err(format!(
            "{name}: the report is not the difference of the documents:\n reported {}\n derived  {}",
            serde_json::to_string(&reported).unwrap_or_default(),
            serde_json::to_string(&derived).unwrap_or_default()
        ));
    }
    for s in &reported.split {
        if item(before, s.right).is_some() || item(after, s.right).is_none() {
            return Err(format!("{name}: split right half {} is not new", s.right));
        }
        if item(before, s.left).is_none() {
            return Err(format!("{name}: split left half {} did not exist", s.left));
        }
    }
    for t in &reported.created_tracks {
        let exists = |doc: &Project| doc.sequences.values().any(|s| s.track(t).is_some());
        if exists(before) || !exists(after) {
            return Err(format!("{name}: created track {t} is not new"));
        }
    }
    Ok(())
}

fn id_at(v: &Value) -> Result<ItemId, String> {
    v.as_str()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("expected an item id, got {v}"))
}

fn item(doc: &Project, id: ItemId) -> Option<(TrackId, Item)> {
    doc.find_item(&id).map(|at| (at.track.id, at.item.clone()))
}

fn found(doc: &Project, id: ItemId, when: &str) -> Result<(TrackId, Item), String> {
    item(doc, id).ok_or_else(|| format!("{id} is missing {when}"))
}

/// The members of `group` in `doc`, by id.
fn members(doc: &Project, group: GroupId) -> Vec<ItemId> {
    let mut out: Vec<ItemId> = doc
        .sequences
        .values()
        .flat_map(|s| s.tracks.values())
        .flat_map(|t| t.items.iter())
        .filter(|i| i.linked_group() == Some(group))
        .map(Item::id)
        .collect();
    out.sort();
    out
}

/// The item's partners in `doc` (its group's other members), by id.
fn partners(doc: &Project, it: &Item) -> Vec<ItemId> {
    it.linked_group().map_or_else(Vec::new, |g| {
        members(doc, g)
            .into_iter()
            .filter(|id| *id != it.id())
            .collect()
    })
}

fn end(it: &Item) -> i64 {
    it.start().flicks() + it.duration().flicks()
}

fn unlinked(params: &Value) -> bool {
    params["unlink"].as_bool().unwrap_or(false)
}

/// `unlink: true`: the item left its group, its partners did not change.
fn only_item_unlinked(
    target: ItemId,
    before_item: &Item,
    before: &Project,
    after: &Project,
) -> Result<(), String> {
    let (_, now) = found(after, target, "after the edit")?;
    if now.linked_group().is_some() {
        return Err(format!("{target} is still linked after `unlink: true`"));
    }
    // A partner left alone in the group is unlinked too (no single-member groups); apart from
    // that nothing about it may change.
    let others = partners(before, before_item);
    for p in &others {
        let (t0, mut p0) = found(before, *p, "before the edit")?;
        let (t1, mut p1) = found(after, *p, "after the edit")?;
        let alone = others.len() == 1;
        let group_ok =
            p1.linked_group() == p0.linked_group() || (alone && p1.linked_group().is_none());
        p0.set_linked_group(None);
        p1.set_linked_group(None);
        if t0 != t1 || p0 != p1 || !group_ok {
            return Err(format!(
                "partner {p} of {target} changed although only {target} was unlinked"
            ));
        }
    }
    Ok(())
}

fn deleted_with_partners(output: &Value, before: &Project, after: &Project) -> Result<(), String> {
    let deleted = output["deleted"]
        .as_array()
        .ok_or("item.delete returned no `deleted` list")?;
    for d in deleted {
        let id = id_at(d)?;
        if item(after, id).is_some() {
            return Err(format!(
                "item.delete reported {id} deleted, but it is still there"
            ));
        }
        let (_, was) = found(before, id, "before the delete")?;
        for p in partners(before, &was) {
            if item(after, p).is_some() {
                return Err(format!(
                    "item.delete deleted {id} but left its linked partner {p}"
                ));
            }
        }
    }
    Ok(())
}

/// The given items of an `item`/`items` request.
fn given(params: &Value) -> Result<Vec<ItemId>, String> {
    let mut out: Vec<ItemId> = match params.get("items").and_then(Value::as_array) {
        Some(list) => list.iter().map(id_at).collect::<Result<_, _>>()?,
        None => vec![id_at(&params["item"])?],
    };
    let mut seen = std::collections::BTreeSet::new();
    out.retain(|id| seen.insert(*id));
    Ok(out)
}

/// Whether `doc` has a magnetic track (ripples may move partners of unlinked items there).
fn has_magnetic(doc: &Project) -> bool {
    doc.sequences
        .values()
        .any(|s| s.tracks.values().any(|t| t.magnetic))
}

fn moved_together(params: &Value, before: &Project, after: &Project) -> Result<(), String> {
    let targets = given(params)?;
    // P1: a single unlinked move on free tracks (`reject` or `new_track`) leaves the partners
    // as they were; otherwise a ripple or a cut may change them, so only the group rule holds.
    let p1 = targets.len() == 1
        && params.get("items").is_none_or(Value::is_null)
        && !matches!(params["mode"].as_str(), Some("insert" | "overwrite"))
        && !has_magnetic(before);
    for target in targets {
        let (_, was) = found(before, target, "before the move")?;
        if unlinked(params) {
            if p1 {
                return only_item_unlinked(target, &was, before, after);
            }
            let (_, now) = found(after, target, "after the move")?;
            if now.linked_group().is_some() {
                return Err(format!("{target} is still linked after `unlink: true`"));
            }
            continue;
        }
        let (_, now) = found(after, target, "after the move")?;
        let delta = now.start().flicks() - was.start().flicks();
        if now.linked_group() != was.linked_group() {
            return Err(format!("item.move changed the group of {target}"));
        }
        for p in partners(before, &was) {
            let (_, p0) = found(before, p, "before the move")?;
            let (_, p1) = found(after, p, "after the move")?;
            if p1.start().flicks() - p0.start().flicks() != delta
                || p1.linked_group() != was.linked_group()
            {
                return Err(format!(
                    "linked partner {p} of {target} did not move with it (offset {} instead of {delta})",
                    p1.start().flicks() - p0.start().flicks()
                ));
            }
        }
    }
    Ok(())
}

fn trimmed_together(params: &Value, before: &Project, after: &Project) -> Result<(), String> {
    let target = id_at(&params["item"])?;
    let (_, was) = found(before, target, "before the trim")?;
    if unlinked(params) {
        return only_item_unlinked(target, &was, before, after);
    }
    let (_, now) = found(after, target, "after the trim")?;
    let start_edge = params["edge"].as_str() == Some("start");
    let edge = |it: &Item| {
        if start_edge {
            it.start().flicks()
        } else {
            end(it)
        }
    };
    for p in partners(before, &was) {
        let (_, p1) = found(after, p, "after the trim")?;
        if edge(&p1) != edge(&now) || p1.linked_group() != was.linked_group() {
            return Err(format!(
                "linked partner {p} of {target} was not trimmed with it ({} edge at {} instead of {})",
                if start_edge { "start" } else { "end" },
                edge(&p1),
                edge(&now)
            ));
        }
    }
    Ok(())
}

fn split_together(
    params: &Value,
    output: &Value,
    before: &Project,
    after: &Project,
) -> Result<(), String> {
    let targets = given(params)?;
    let first = *targets.first().ok_or("clip.split named no clip")?;
    let at = output["at"].as_i64().ok_or("clip.split returned no `at`")?;
    let right_group: Option<GroupId> = output["right_group"].as_str().and_then(|s| s.parse().ok());
    let mut pairs = vec![(id_at(&output["left"])?, id_at(&output["right"])?)];
    for pair in output["linked"]
        .as_array()
        .ok_or("clip.split returned no `linked`")?
    {
        pairs.push((id_at(&pair["left"])?, id_at(&pair["right"])?));
    }
    // Every given clip and every partner that contained `at` is a left half now; the first
    // given clip comes first.
    let mut contained: Vec<ItemId> = Vec::new();
    for target in &targets {
        let (_, was) = found(before, *target, "before the split")?;
        for id in std::iter::once(*target).chain(partners(before, &was)) {
            let inside = item(before, id).is_some_and(|(_, i)| {
                i.as_clip().is_some() && i.start().flicks() < at && at < end(&i)
            });
            if inside && !contained.contains(&id) {
                contained.push(id);
            }
        }
    }
    let mut lefts: Vec<ItemId> = pairs.iter().map(|(l, _)| *l).collect();
    for list in [&mut contained, &mut lefts] {
        if let Some(rest) = list.get_mut(1..) {
            rest.sort();
        }
    }
    if contained != lefts {
        return Err(format!(
            "clip.split at {at} split {lefts:?}, but {contained:?} contained it"
        ));
    }
    let (_, first_was) = found(before, first, "before the split")?;
    if first_was.linked_group().is_some() != right_group.is_some() {
        return Err(format!(
            "clip.split: right group {right_group:?} for a clip in group {:?}",
            first_was.linked_group()
        ));
    }
    for (left, right) in pairs {
        let (_, was) = found(before, left, "before the split")?;
        let (lt, l) = found(after, left, "after the split")?;
        let (rt, r) = found(after, right, "after the split")?;
        if end(&l) != at || r.start().flicks() != at || lt != rt {
            return Err(format!(
                "{left} | {right} is not a split at {at} on one track"
            ));
        }
        let expected = if partners(before, &first_was).contains(&left) || left == first {
            right_group
        } else {
            r.linked_group()
        };
        if l.linked_group() != was.linked_group()
            || r.linked_group() != expected
            || r.linked_group().is_some() != was.linked_group().is_some()
        {
            return Err(format!(
                "after the split, {left} is in {:?} (expected {:?}) and {right} in {:?} (expected {expected:?})",
                l.linked_group(),
                was.linked_group(),
                r.linked_group()
            ));
        }
    }
    Ok(())
}
