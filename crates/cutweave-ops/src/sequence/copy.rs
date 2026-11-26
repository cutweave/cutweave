// SPDX-License-Identifier: GPL-3.0-or-later
//! Deep copy of a sequence for `sequence.duplicate`: every entity of the copy (the sequence,
//! its tracks, items, markers and linked groups) gets a new ID, and the references between them
//! (attachments, linked groups) follow. Media is shared: the copy's clips name the same assets.

use std::collections::BTreeMap;

use cutweave_commands::{CmdError, Tx};
use cutweave_model::{GroupId, Item, ItemId, MarkerId, Markers, Sequence, SequenceId, Track};

/// A copy of `source` named `name` with fresh IDs, in the same track order.
pub(super) fn deep_copy(
    tx: &mut Tx<'_>,
    source: &Sequence,
    name: String,
) -> Result<Sequence, CmdError> {
    let mut copy = Sequence::new(tx.new_id::<SequenceId>(), name, source.format.clone());
    copy.in_out.clone_from(&source.in_out);
    copy.custom.clone_from(&source.custom);
    copy.extra.clone_from(&source.extra);
    copy.markers = remark(tx, &source.markers);

    // New IDs first, in a fixed order, so references can follow.
    let mut items: BTreeMap<ItemId, ItemId> = BTreeMap::new();
    let mut groups: BTreeMap<GroupId, GroupId> = BTreeMap::new();
    for track in source.tracks_in_order() {
        for item in track.items.iter() {
            items.insert(item.id(), tx.new_id::<ItemId>());
            if let Some(g) = item.linked_group() {
                groups.entry(g).or_insert_with(|| tx.new_id::<GroupId>());
            }
        }
    }
    for (index, track) in source.tracks_in_order().enumerate() {
        let mut t = Track::new(tx.new_id(), track.kind, track.name.clone());
        t.muted = track.muted;
        t.hidden = track.hidden;
        t.locked = track.locked;
        t.solo = track.solo;
        t.magnetic = track.magnetic;
        t.height = track.height;
        t.gain = track.gain;
        t.custom.clone_from(&track.custom);
        t.extra.clone_from(&track.extra);
        for item in track.items.iter() {
            let Some(clip) = item.as_clip() else {
                return Err(CmdError::unsupported(
                    "this item kind cannot be duplicated yet",
                ));
            };
            let mut c = clip.clone();
            c.id = items[&clip.id];
            c.linked_group = clip.linked_group.map(|g| groups[&g]);
            c.attached_to = clip.attached_to.and_then(|p| items.get(&p).copied());
            c.markers = remark(tx, &clip.markers);
            t.items.insert(Item::Clip(c));
        }
        copy.insert_track(index, t);
    }
    Ok(copy)
}

/// The same markers under new IDs.
fn remark(tx: &mut Tx<'_>, markers: &Markers) -> Markers {
    markers
        .values()
        .map(|m| {
            let mut m = m.clone();
            m.id = tx.new_id::<MarkerId>();
            (m.id, m)
        })
        .collect()
}
