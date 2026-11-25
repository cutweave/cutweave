// SPDX-License-Identifier: GPL-3.0-or-later
//! `item.link`, `item.unlink`, `item.attach`, `item.detach` and the query `item.group_select`
//! (WP-2A.05; timeline spec §1.3, §1.4, §4.15, §4.16; ADR-0023).
//!
//! **Link** puts the given items and every member of their current groups into one new group;
//! the members must be aligned (same start and duration, INV-8) and attached to the same parent
//! or to none (INV-9). **Unlink** takes the given items (only them) out of their groups; a group
//! left with one member dissolves (INV-262). **Attach** sets `attached_to` on the given items
//! and their linked partners, **detach** clears it (INV-271). None of them moves anything, and
//! each is one command, hence one history entry (INV-48).
//!
//! The placement core judges these edits like any other: a plan that changes an item of a locked
//! track is `Locked` (INV-27) before any semantic error, so the checks here are recorded with
//! [`Edit::defer`] after the change is made, not returned early.
//!
//! **IDs** (INV-23). Link mints the new group first; the others mint nothing.

use std::collections::BTreeSet;

use cutweave_commands::{CmdError, Command, Query, QueryCtx, Tx, command, query, wire};
use cutweave_model::{EntityKind, GroupId, Item, ItemId, Project, SequenceId, TrackId};

use crate::{
    report::EditReport,
    timeline::{
        edit::given_items,
        placement::{DryIds, Edit, Ids, Plan, span},
    },
    track::check_editable_kind,
};

/// The distinct members of `given` and their linked partners, in output order (each given item
/// followed by its partners), with the track of each. `Unsupported` for non-editable tracks.
fn with_partners(e: &Edit<'_>, given: &[ItemId]) -> Result<Vec<(TrackId, ItemId)>, CmdError> {
    let mut out: Vec<(TrackId, ItemId)> = Vec::new();
    for id in given {
        for (track, m) in e.members(*id) {
            if out.iter().any(|(_, o)| *o == m) {
                continue;
            }
            if let Some(t) = e.seq.track(&track) {
                check_editable_kind("/items", t.kind)?;
            }
            out.push((track, m));
        }
    }
    Ok(out)
}

fn item_in<'a>(e: &'a Edit<'_>, id: ItemId) -> Option<&'a Item> {
    e.locate(id).map(|(_, i)| i)
}

// ----- item.link ---------------------------------------------------------------------------

/// Result of `item.link`.
#[wire]
pub struct ItemLinkOutput {
    /// The new group.
    pub group: GroupId,
    /// Its members: the given items in order, then the partners they brought along.
    pub members: Vec<ItemId>,
    /// What changed (timeline spec §3.1).
    pub report: EditReport,
}

/// Links items into one group, so they move, trim, split, are deleted and duplicated together.
/// The items must be in one sequence, on other tracks than each other, and aligned (identical
/// start and duration; trim first, or use `item.attach`). Every member of the items' current
/// groups joins the new group, and the old groups no longer exist. All members must be attached
/// to the same parent, or none to any. Nothing moves.
#[command(
    name = "item.link",
    summary = "Link aligned items into one group (they edit together)",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = false)
)]
pub struct ItemLink {
    /// The items to link, at least two distinct ones.
    pub items: Vec<ItemId>,
}

fn plan_link(
    cmd: &ItemLink,
    doc: &Project,
    ids: &mut dyn Ids,
) -> Result<(Plan, GroupId, Vec<ItemId>), CmdError> {
    let (sequence, given) = given_items(doc, &cmd.items)?;
    if given.len() < 2 {
        return Err(CmdError::invalid(
            "/items",
            "give at least two distinct items to link",
        ));
    }
    let mut e = Edit::new(doc, sequence, "/items")?;
    let members = with_partners(&e, &given)?;
    let group = e.mint::<GroupId>(ids);
    let first = members
        .first()
        .and_then(|(_, id)| item_in(&e, *id))
        .map(|i| (span(i), i.attached_to()));
    if let Some((first_span, first_parent)) = first {
        for (_, id) in &members {
            let Some(item) = item_in(&e, *id) else {
                continue;
            };
            if span(item) != first_span {
                let msg = format!(
                    "item `{id}` does not start and end with the other items; trim them to the \
                     same range first, or use `item.attach`"
                );
                e.defer(e.invalid(msg));
            } else if item.attached_to() != first_parent {
                let msg = format!(
                    "item `{id}` is not attached to the same parent as the other items; attach \
                     or detach them first"
                );
                e.defer(e.invalid(msg));
            }
        }
        if let Some(parent) = first_parent
            && members.iter().any(|(_, m)| *m == parent)
        {
            e.defer(e.invalid("an item cannot be linked to its own parent"));
        }
    }
    for (_, id) in &members {
        e.update(*id, |i| i.set_linked_group(Some(group)));
    }
    let ordered = members.iter().map(|(_, id)| *id).collect();
    Ok((e.finish(ids)?, group, ordered))
}

impl Command for ItemLink {
    type Output = ItemLinkOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        plan_link(self, ctx.doc(), &mut DryIds::default()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<ItemLinkOutput, CmdError> {
        let doc = tx.doc().clone();
        let (plan, group, members) = plan_link(self, &doc, tx)?;
        let report = plan.commit(tx)?;
        Ok(ItemLinkOutput {
            group,
            members,
            report,
        })
    }
}

// ----- item.unlink -------------------------------------------------------------------------

/// Result of `item.unlink`.
#[wire]
pub struct ItemUnlinkOutput {
    /// What changed (timeline spec §3.1).
    pub report: EditReport,
}

/// Takes the given items (only them) out of their linked groups. A group left with one member
/// is dissolved. Items that are not linked are left as they are. Nothing moves.
#[command(
    name = "item.unlink",
    summary = "Take items out of their linked groups",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct ItemUnlink {
    /// The items to unlink.
    pub items: Vec<ItemId>,
}

fn plan_unlink(cmd: &ItemUnlink, doc: &Project, ids: &mut dyn Ids) -> Result<Plan, CmdError> {
    let (sequence, given) = given_items(doc, &cmd.items)?;
    let mut e = Edit::new(doc, sequence, "/items")?;
    for id in &given {
        let Some(item) = item_in(&e, *id) else {
            continue;
        };
        if let Some(g) = item.linked_group() {
            e.mark_left(g);
            e.update(*id, |i| i.set_linked_group(None));
        }
    }
    e.finish(ids)
}

impl Command for ItemUnlink {
    type Output = ItemUnlinkOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        plan_unlink(self, ctx.doc(), &mut DryIds::default()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<ItemUnlinkOutput, CmdError> {
        let doc = tx.doc().clone();
        let plan = plan_unlink(self, &doc, tx)?;
        Ok(ItemUnlinkOutput {
            report: plan.commit(tx)?,
        })
    }
}

// ----- item.attach and item.detach ---------------------------------------------------------

/// Result of `item.attach` and `item.detach`.
#[wire]
pub struct ItemAttachOutput {
    /// The items whose attachment was set or cleared: the given ones in order, each followed by
    /// its linked partners.
    pub items: Vec<ItemId>,
    /// What changed (timeline spec §3.1).
    pub report: EditReport,
}

/// Attaches items (and their linked partners) to a parent item, so they follow it when it moves
/// (timeline spec §1.4). Every child must start inside the parent, be on another track than it
/// and not in its linked group; the parent must not be attached itself, and a child must not be
/// a parent. A child is never moved to make it fit. Attaching again replaces the old parent.
#[command(
    name = "item.attach",
    summary = "Attach items to a parent item so they follow it",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct ItemAttach {
    /// The children, at least one.
    pub items: Vec<ItemId>,
    /// The parent item, in the same sequence.
    pub parent: ItemId,
}

fn plan_attach(
    cmd: &ItemAttach,
    doc: &Project,
    ids: &mut dyn Ids,
) -> Result<(Plan, Vec<ItemId>), CmdError> {
    let (sequence, given) = given_items(doc, &cmd.items)?;
    let parent_at = doc
        .find_item(&cmd.parent)
        .ok_or_else(|| CmdError::not_found(EntityKind::Item, cmd.parent))?;
    if parent_at.sequence.id != sequence {
        return Err(CmdError::invalid(
            "/parent",
            "the parent must be in the same sequence as the items",
        ));
    }
    let (parent_track, parent) = (parent_at.track.id, parent_at.item.clone());
    let mut e = Edit::new(doc, sequence, "/items")?;
    let members = with_partners(&e, &given)?;
    if parent.attached_to().is_some() {
        e.defer(CmdError::invalid(
            "/parent",
            "the parent is attached to another item itself; attachments are one level deep",
        ));
    }
    let (ps, pe) = span(&parent);
    let all: BTreeSet<ItemId> = members.iter().map(|(_, m)| *m).collect();
    for (track, id) in &members {
        let Some(item) = item_in(&e, *id) else {
            continue;
        };
        let s = item.start().flicks();
        let has_children = e.seq.tracks.values().any(|t| {
            t.items
                .iter()
                .any(|o| o.attached_to() == Some(*id) && !all.contains(&o.id()))
        });
        let problem = if all.contains(&cmd.parent) {
            Some("an item cannot be attached to itself or to a member of its own group")
        } else if has_children {
            Some("an item that has children of its own cannot become a child")
        } else if *track == parent_track {
            Some("an item cannot be attached to an item of its own track")
        } else if s < ps || s >= pe {
            Some("an item must start inside its parent to be attached to it")
        } else {
            None
        };
        if let Some(msg) = problem {
            e.defer(e.invalid(format!("item `{id}`: {msg}")));
        }
    }
    for (_, id) in &members {
        e.update(*id, |i| i.set_attached_to(Some(cmd.parent)));
    }
    let ordered = members.iter().map(|(_, id)| *id).collect();
    Ok((e.finish(ids)?, ordered))
}

impl Command for ItemAttach {
    type Output = ItemAttachOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        plan_attach(self, ctx.doc(), &mut DryIds::default()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<ItemAttachOutput, CmdError> {
        let doc = tx.doc().clone();
        let (plan, items) = plan_attach(self, &doc, tx)?;
        Ok(ItemAttachOutput {
            items,
            report: plan.commit(tx)?,
        })
    }
}

/// Clears the attachment of items and their linked partners; they keep their place. Items that
/// are not attached are left as they are.
#[command(
    name = "item.detach",
    summary = "Detach items from their parent item",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct ItemDetach {
    /// The children to detach.
    pub items: Vec<ItemId>,
}

fn plan_detach(
    cmd: &ItemDetach,
    doc: &Project,
    ids: &mut dyn Ids,
) -> Result<(Plan, Vec<ItemId>), CmdError> {
    let (sequence, given) = given_items(doc, &cmd.items)?;
    let mut e = Edit::new(doc, sequence, "/items")?;
    let members = with_partners(&e, &given)?;
    for (_, id) in &members {
        e.update(*id, |i| i.set_attached_to(None));
    }
    let ordered = members.iter().map(|(_, id)| *id).collect();
    Ok((e.finish(ids)?, ordered))
}

impl Command for ItemDetach {
    type Output = ItemAttachOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        plan_detach(self, ctx.doc(), &mut DryIds::default()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<ItemAttachOutput, CmdError> {
        let doc = tx.doc().clone();
        let (plan, items) = plan_detach(self, &doc, tx)?;
        Ok(ItemAttachOutput {
            items,
            report: plan.commit(tx)?,
        })
    }
}

// ----- item.group_select -------------------------------------------------------------------

/// One selected item.
#[wire]
pub struct GroupMember {
    /// The item.
    pub item: ItemId,
    /// Its track.
    pub track: TrackId,
    /// Its linked group, if any.
    pub linked_group: Option<GroupId>,
}

/// Result of `item.group_select`.
#[wire]
pub struct ItemGroupSelectOutput {
    /// The sequence of the items.
    pub sequence: SequenceId,
    /// The given items in order, each followed by the other members of its group (no repeats).
    pub items: Vec<GroupMember>,
}

/// The selection a click on these items makes: the items and every member of their linked
/// groups (timeline spec INV-341). Attached children are not included. A read-only query.
#[query(
    name = "item.group_select",
    summary = "The items and every member of their linked groups (what a click selects)",
    mcp = "catalog"
)]
pub struct ItemGroupSelect {
    /// The items, in one sequence.
    pub items: Vec<ItemId>,
}

impl Query for ItemGroupSelect {
    type Output = ItemGroupSelectOutput;

    fn run(&self, ctx: &QueryCtx<'_>) -> Result<ItemGroupSelectOutput, CmdError> {
        let doc = ctx.doc();
        let (sequence, given) = given_items(doc, &self.items)?;
        let e = Edit::new(doc, sequence, "/items")?;
        let mut items: Vec<GroupMember> = Vec::new();
        for id in &given {
            for (track, m) in e.members(*id) {
                if items.iter().all(|o| o.item != m) {
                    items.push(GroupMember {
                        item: m,
                        track,
                        linked_group: item_in(&e, m).and_then(Item::linked_group),
                    });
                }
            }
        }
        Ok(ItemGroupSelectOutput { sequence, items })
    }
}
