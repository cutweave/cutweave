// SPDX-License-Identifier: GPL-3.0-or-later
//! The magnetic main track's placement rules (WP-2A.03; timeline spec §1.2, INV-7, INV-41,
//! INV-122, INV-123), on top of [`super::placement`]:
//!
//! - a placement time on the magnetic track is clamped into `[0, end]` (INV-41);
//! - the items that land on it form one contiguous block, in their original order, from the
//!   block's start (INV-123); a linked group that has a member in the block moves by that
//!   member's shift, so it stays aligned (INV-35);
//! - closing every gap of a track, left to right, the way `timeline.delete_gap` does (used by
//!   `track.set_magnetic{close_gaps}`).

use std::collections::BTreeMap;

use cutweave_model::{GroupId, ItemId, TrackId};

use super::placement::{Edit, first_gap, span};

/// A placement time on the magnetic track `track`: clamped into `[0, end]` (INV-41).
pub(crate) fn clamp_to_end(edit: &Edit<'_>, track: TrackId, t: i64) -> i64 {
    t.clamp(0, edit.track_end(track))
}

/// What a magnetic block needs to know about an item: its ID, group, start and length.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BlockItem {
    pub(crate) id: ItemId,
    pub(crate) group: Option<GroupId>,
    pub(crate) old: i64,
    pub(crate) len: i64,
}

/// The block of `items` (those landing on the magnetic track) packed from `start` in their
/// original order: each item's new start, and the shift of every linked group with a member in
/// the block (the key is the group, or the item for an unlinked one).
pub(crate) fn pack(
    items: &[BlockItem],
    start: i64,
) -> (BTreeMap<ItemId, i64>, BTreeMap<Unit, i64>) {
    let mut ordered: Vec<&BlockItem> = items.iter().collect();
    ordered.sort_by_key(|b| (b.old, b.id));
    let mut starts = BTreeMap::new();
    let mut shifts = BTreeMap::new();
    let mut cursor = start;
    for b in ordered {
        starts.insert(b.id, cursor);
        shifts
            .entry(Unit::of(b.id, b.group))
            .or_insert(cursor - b.old);
        cursor += b.len;
    }
    (starts, shifts)
}

/// The offset of `id` inside its packed block: the length of the block items before it.
pub(crate) fn offset_in_block(items: &[BlockItem], id: ItemId) -> i64 {
    let Some(me) = items.iter().find(|b| b.id == id) else {
        return 0;
    };
    items
        .iter()
        .filter(|b| (b.old, b.id) < (me.old, me.id))
        .map(|b| b.len)
        .sum()
}

/// What moves as one: a linked group, or an unlinked item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Unit {
    /// A linked group.
    Group(GroupId),
    /// An item without a group.
    Item(ItemId),
}

impl Unit {
    /// The unit of an item.
    pub(crate) fn of(id: ItemId, group: Option<GroupId>) -> Unit {
        group.map_or(Unit::Item(id), Unit::Group)
    }
}

/// Closes every gap of `track` (and the space before its first item), left to right: for each
/// gap, a ripple at its end by minus its width, reaching linked partners (INV-30, INV-31).
/// Collisions are judged when the edit finishes (INV-32).
pub(crate) fn close_gaps(edit: &mut Edit<'_>, track: TrackId) {
    // Each pass closes the first gap; a track of n items has at most n gaps.
    let passes = edit.seq.track(&track).map_or(0, |t| t.items.len());
    for _ in 0..passes {
        let Some((from, to)) = edit.seq.track(&track).and_then(first_gap) else {
            return;
        };
        let width = to - from;
        edit.ripple(&[track], false, to, &move |_| -width);
    }
}

/// The end of the last of `items` (by end) on the magnetic track, before the edit: where
/// copies of main-track items go (`item.duplicate`).
pub(crate) fn block_end(edit: &Edit<'_>, track: TrackId, items: &[ItemId]) -> Option<i64> {
    let t = edit.seq.track(&track)?;
    items
        .iter()
        .filter_map(|id| t.items.get(id))
        .map(|i| span(i).1)
        .max()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(n: u128, old: i64, len: i64, group: Option<u128>) -> BlockItem {
        BlockItem {
            id: ItemId::from_u128(n),
            group: group.map(GroupId::from_u128),
            old,
            len,
        }
    }

    #[test]
    fn a_block_packs_in_original_order_and_shifts_groups_with_their_member() {
        let items = [b(2, 60, 10, Some(7)), b(1, 0, 30, None)];
        let (starts, shifts) = pack(&items, 100);
        assert_eq!(starts[&ItemId::from_u128(1)], 100);
        assert_eq!(starts[&ItemId::from_u128(2)], 130);
        assert_eq!(shifts[&Unit::Group(GroupId::from_u128(7))], 70);
        assert_eq!(shifts[&Unit::Item(ItemId::from_u128(1))], 100);
        assert_eq!(offset_in_block(&items, ItemId::from_u128(2)), 30);
        assert_eq!(offset_in_block(&items, ItemId::from_u128(1)), 0);
    }
}
