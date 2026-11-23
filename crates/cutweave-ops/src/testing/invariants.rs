// SPDX-License-Identifier: GPL-3.0-or-later
//! Property (5) of the harness: the model invariants every committed document keeps (arch
//! §4.3), and "linked groups intact" as the WP-1B.06 edit semantics define it (module docs of
//! [`crate::item`]):
//!
//! - a linked group never spans sequences (checked by [`Project::validate`]);
//! - linked partners move, trim, split and are deleted together: after a successful
//!   `item.move` every partner moved by the same offset; after `clip.trim` every partner's
//!   trimmed edge is where the item's is; after `clip.split` every partner that contained the
//!   time was split there too and the right halves share the new group; after `item.delete`
//!   no partner of a deleted item is left;
//! - `unlink: true` (`item.move`, `clip.trim`) takes only the item out of its group and leaves
//!   its partners as they were.
//!
//! Groups need not be time-aligned: documents may link items that never were (and an undo of
//! an unlink can re-link an item whose partner changed since), so only the edit rules are
//! checked.

use cutweave_model::{GroupId, Item, ItemId, Project, TrackId};
use serde_json::Value;

/// Checks the invariants of `doc`: [`Project::validate`] (references, timing, bounds, formats,
/// linked groups inside one sequence, ...) and no overlapping items on any track.
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
    Ok(())
}

/// The linked-group rules of a successful group-aware edit (`clip.split`, `clip.trim`,
/// `item.move`, `item.delete`) that changed `before` into `after` with `params` and returned
/// `output`. Other commands pass.
pub fn check_linked_edit(
    name: &str,
    params: &Value,
    output: &Value,
    before: &Project,
    after: &Project,
) -> Result<(), String> {
    match name {
        "item.delete" => deleted_with_partners(output, before, after),
        "item.move" => moved_together(params, before, after),
        "clip.trim" => trimmed_together(params, before, after),
        "clip.split" => split_together(params, output, before, after),
        _ => Ok(()),
    }
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
    for p in partners(before, before_item) {
        if item(before, p) != item(after, p) {
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

fn moved_together(params: &Value, before: &Project, after: &Project) -> Result<(), String> {
    let target = id_at(&params["item"])?;
    let (_, was) = found(before, target, "before the move")?;
    if unlinked(params) {
        return only_item_unlinked(target, &was, before, after);
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
    let target = id_at(&params["item"])?;
    let at = output["at"].as_i64().ok_or("clip.split returned no `at`")?;
    let right_group: Option<GroupId> = output["right_group"].as_str().and_then(|s| s.parse().ok());
    let (_, was) = found(before, target, "before the split")?;
    let mut pairs = vec![(id_at(&output["left"])?, id_at(&output["right"])?)];
    for pair in output["linked"]
        .as_array()
        .ok_or("clip.split returned no `linked`")?
    {
        pairs.push((id_at(&pair["left"])?, id_at(&pair["right"])?));
    }
    // Every partner that contained `at` (and the item itself) is a left half now. (The item
    // comes first in both lists; the partners' order is the command's.)
    let mut contained: Vec<ItemId> = std::iter::once(target)
        .chain(partners(before, &was))
        .filter(|id| {
            item(before, *id).is_some_and(|(_, i)| {
                i.as_clip().is_some() && i.start().flicks() < at && at < end(&i)
            })
        })
        .collect();
    let mut lefts: Vec<ItemId> = pairs.iter().map(|(l, _)| *l).collect();
    for list in [&mut contained, &mut lefts] {
        if let Some(partners) = list.get_mut(1..) {
            partners.sort();
        }
    }
    if contained != lefts {
        return Err(format!(
            "clip.split at {at} split {lefts:?}, but {contained:?} contained it"
        ));
    }
    if (pairs.len() > 1) != right_group.is_some() {
        return Err(format!(
            "clip.split: right group {right_group:?} for {} halves",
            pairs.len()
        ));
    }
    for (left, right) in pairs {
        let (lt, l) = found(after, left, "after the split")?;
        let (rt, r) = found(after, right, "after the split")?;
        if end(&l) != at || r.start().flicks() != at || lt != rt {
            return Err(format!(
                "{left} | {right} is not a split at {at} on one track"
            ));
        }
        if l.linked_group() != was.linked_group() || r.linked_group() != right_group {
            return Err(format!(
                "after the split, {left} is in {:?} (expected {:?}) and {right} in {:?} (expected {right_group:?})",
                l.linked_group(),
                was.linked_group(),
                r.linked_group()
            ));
        }
    }
    Ok(())
}
