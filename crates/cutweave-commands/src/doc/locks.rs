// SPDX-License-Identifier: GPL-3.0-or-later
//! The minimal soft lock (WP-1B.09; arch §5.2, ADR-0017): while a `gesture` or `proposal`
//! transaction is open, the items its commands touched are locked for **other actors**. Their
//! requests that would change one of those items fail fast, before anything is applied, with
//! `Conflict{reason: item_in_gesture, retry_after_ms}`; requests on other items proceed. Locks
//! live inside their open transaction, so commit, cancel, connection drop and idle expiry
//! release them by construction.
//!
//! - [`ItemLocks`]: one transaction's locked items, with every item path its ops used, so a
//!   coarser change above a locked item (removing its track, replacing its sequence) counts as
//!   touching it.
//! - [`TxLocks`] / [`LockList`]: what `edit.locks` reports (UI "in use by" indicators, agents
//!   deciding whether to wait).
//!
//! What "touch" means (WP-2A.12, spec INV-304): every item an op adds, removes, moves or edits,
//! including the items inside a whole track or sequence an op adds, removes or replaces, plus
//! every item below a path an op writes. A command's patch is exactly the difference its
//! `EditReport` names (spec INV-21), so this is the union of the items the reports of a
//! gesture's updates name: ripple-shifted items, linked group members and following attached
//! children included, because the commands that move them (WP-2A.03/04/05) write them. A
//! member a partial (`unlink: true`) edit leaves alone is not written, so it is not locked.
//! The set only grows until the transaction closes, also when `_meta.replace` rolls earlier
//! updates back.

use std::collections::{BTreeMap, BTreeSet};

use cutweave_model::{ItemId, TrackId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    clock::WallTime,
    identity::Actor,
    request::{TxId, TxKind},
};
use crate::{
    error::Rev,
    patch::{Patch, PatchOp},
    path::{DocPath, EntityRef},
};

/// The items one open transaction soft-locks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ItemLocks {
    /// Each locked item, with the item paths (`/sequences/S/tracks/T/items/I`) its ops used
    /// (two after a move to another track).
    items: BTreeMap<ItemId, BTreeSet<DocPath>>,
    /// Every path of `items`, for the "above a locked item" check.
    places: BTreeSet<DocPath>,
}

impl ItemLocks {
    /// Everything `patch` touches.
    pub(crate) fn of(patch: &Patch) -> ItemLocks {
        let mut locks = ItemLocks::default();
        locks.extend(patch);
        locks
    }

    /// Whether nothing is locked.
    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Whether `item` is locked.
    pub(crate) fn contains(&self, item: ItemId) -> bool {
        self.items.contains_key(&item)
    }

    /// The locked items, in id order.
    pub(crate) fn items(&self) -> Vec<ItemId> {
        self.items.keys().copied().collect()
    }

    /// Whether every item and place of `other` is locked here too.
    pub(crate) fn covers(&self, other: &ItemLocks) -> bool {
        other.places.is_subset(&self.places)
            && other.items.keys().all(|item| self.items.contains_key(item))
    }

    /// Locks the items `patch` touches; returns how many were not locked yet.
    pub(crate) fn extend(&mut self, patch: &Patch) -> usize {
        let before = self.items.len();
        for op in patch {
            for path in op.paths() {
                if let EntityRef::Item {
                    sequence,
                    track,
                    item,
                } = path.entity()
                {
                    self.lock(DocPath::item(sequence, track, item), item);
                }
            }
            // A whole track or sequence added, removed or replaced: the items inside it.
            for (path, value) in whole_values(op) {
                self.lock_nested(path, value);
            }
        }
        self.items.len() - before
    }

    fn lock(&mut self, place: DocPath, item: ItemId) {
        self.places.insert(place.clone());
        self.items.entry(item).or_default().insert(place);
    }

    /// Locks the items of the track (`/sequences/S/tracks/T`), its item map (`…/items`) or the
    /// sequence (`/sequences/S`) that `value` is at `path`.
    fn lock_nested(&mut self, path: &DocPath, value: &Value) {
        match path.entity() {
            EntityRef::Track { sequence, track } => {
                let whole = DocPath::track(sequence, track);
                let items = if *path == whole {
                    value.get("items")
                } else if *path == whole.join("items") {
                    Some(value)
                } else {
                    None
                };
                for item in items
                    .and_then(Value::as_object)
                    .into_iter()
                    .flat_map(|m| m.keys())
                {
                    if let Ok(item) = item.parse::<ItemId>() {
                        self.lock(DocPath::item(sequence, track, item), item);
                    }
                }
            }
            EntityRef::Sequence(sequence) if *path == DocPath::sequence(sequence) => {
                let tracks = value.get("tracks").and_then(Value::as_object);
                for (track, value) in tracks.into_iter().flatten() {
                    if let Ok(track) = track.parse::<TrackId>() {
                        self.lock_nested(&DocPath::track(sequence, track), value);
                    }
                }
            }
            _ => {}
        }
    }

    /// The first locked item `patch` touches: an op on the item or inside it, or an op on a
    /// path above a place the item was at (its track, its sequence, the document).
    pub(crate) fn touched_by(&self, patch: &Patch) -> Option<ItemId> {
        if self.items.is_empty() {
            return None;
        }
        patch
            .iter()
            .flat_map(PatchOp::paths)
            .find_map(|path| self.touches(path))
    }

    fn touches(&self, path: &DocPath) -> Option<ItemId> {
        if let EntityRef::Item { item, .. } = path.entity() {
            return self.contains(item).then_some(item);
        }
        // The places below `path` sort right after it (a string prefix sorts first).
        self.places
            .range(path.clone()..)
            .take_while(|place| place.as_str().starts_with(path.as_str()))
            .find(|place| path.is_prefix_of(place))
            .and_then(|place| match place.entity() {
                EntityRef::Item { item, .. } => Some(item),
                _ => None,
            })
    }
}

/// The values an op puts at or takes from its path (an add's value, a removal's preimage, both
/// sides of a replacement), for the items inside a whole track or sequence.
fn whole_values(op: &PatchOp) -> impl Iterator<Item = (&DocPath, &Value)> {
    let (path, first, second) = match op {
        PatchOp::Add { path, value } => (path, Some(value), None),
        PatchOp::Remove { path, old } => (path, Some(old), None),
        PatchOp::Replace { path, old, value } => (path, Some(old), Some(value)),
        PatchOp::Move { path, .. } => (path, None, None),
    };
    first.into_iter().chain(second).map(move |v| (path, v))
}

/// An open transaction and the items it soft-locks, as `edit.locks` reports it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, specta::Type)]
pub struct TxLocks {
    /// The transaction.
    pub tx: TxId,
    /// `batch`, `gesture` or `proposal`.
    pub kind: TxKind,
    /// Its label (the undo entry's name).
    pub label: String,
    /// Who opened it.
    pub actor: Actor,
    /// Whether the caller's connection owns it.
    pub mine: bool,
    /// The items it soft-locks, in id order. Only gestures and proposals lock; a batch's
    /// changes are invisible until it commits, so it locks nothing.
    pub items: Vec<ItemId>,
    /// When it was opened.
    pub opened_at: WallTime,
    /// When a request last named it.
    pub last_activity: WallTime,
    /// A gesture's idle deadline: the engine cancels it then unless a request names it first.
    /// `null` for proposals and batches (no idle timeout).
    pub expires_at: Option<WallTime>,
    /// A commit by another transaction overlapped its changes: it can only be cancelled.
    pub conflicted: bool,
}

/// What `edit.locks` returns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, specta::Type)]
pub struct LockList {
    /// Every open transaction, oldest first.
    pub transactions: Vec<TxLocks>,
    /// The live revision.
    pub rev: Rev,
    /// The committed revision.
    pub committed_rev: Rev,
}

impl LockList {
    /// The open transaction locking `item`, if any (the first one: only transactions of the
    /// same actor can share an item).
    #[must_use]
    pub fn holder(&self, item: ItemId) -> Option<&TxLocks> {
        self.transactions.iter().find(|t| t.items.contains(&item))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use cutweave_model::{IdGen, SeededGen, SequenceId, TrackId};
    use serde_json::json;

    use super::*;

    fn ids() -> (SequenceId, TrackId, TrackId, ItemId, ItemId) {
        let mut g = SeededGen::new(9);
        (g.mint(), g.mint(), g.mint(), g.mint(), g.mint())
    }

    fn replace(path: DocPath) -> PatchOp {
        PatchOp::Replace {
            path,
            old: json!(1),
            value: json!(2),
        }
    }

    #[test]
    fn locks_follow_items_through_moves_and_catch_coarser_changes() {
        let (s, t1, t2, x, y) = ids();
        let at_t1 = DocPath::item(s, t1, x);
        let locks = ItemLocks::of(&Patch(vec![
            replace(at_t1.join("start")),
            PatchOp::Move {
                from: at_t1.clone(),
                path: DocPath::item(s, t2, x),
            },
        ]));
        assert_eq!(locks.items(), [x]);
        assert!(locks.contains(x) && !locks.contains(y));

        // Inside the item, wherever it is addressed.
        let touched = |op: PatchOp| locks.touched_by(&Patch(vec![op]));
        assert_eq!(
            touched(replace(DocPath::item(s, t2, x).join("name"))),
            Some(x)
        );
        assert_eq!(touched(replace(DocPath::item(s, t1, y).join("name"))), None);
        // Above a place it was at: its old or new track, its sequence, the document.
        for above in [
            DocPath::track(s, t1),
            DocPath::track(s, t2).join("items"),
            DocPath::sequence(s),
            DocPath::root().join("sequences"),
            DocPath::root(),
        ] {
            assert_eq!(touched(replace(above.clone())), Some(x), "{above}");
        }
        // Beside it: other fields of the track or sequence, project metadata.
        for beside in [
            DocPath::track(s, t1).join("locked"),
            DocPath::sequence(s).join("track_order"),
            DocPath::meta().join("name"),
        ] {
            assert_eq!(touched(replace(beside.clone())), None, "{beside}");
        }
        assert_eq!(
            ItemLocks::default().touched_by(&Patch(vec![replace(DocPath::root())])),
            None
        );
    }

    #[test]
    fn items_inside_a_whole_track_or_sequence_are_locked() {
        let (seq, t1, t2, removed, mapped) = ids();
        let mut gen_ids = SeededGen::new(10);
        let (added, seq2): (ItemId, SequenceId) = (gen_ids.mint(), gen_ids.mint());
        let t3: TrackId = gen_ids.mint();
        let items = |id: ItemId| json!({ id.to_string(): {"start": 0} });
        let locks = ItemLocks::of(&Patch(vec![
            // A track removed with its items, one created with an item, one's map replaced.
            PatchOp::Remove {
                path: DocPath::track(seq, t1),
                old: json!({"name": "V1", "items": items(removed)}),
            },
            PatchOp::Replace {
                path: DocPath::track(seq, t2).join("items"),
                old: json!({}),
                value: items(mapped),
            },
            PatchOp::Add {
                path: DocPath::sequence(seq2),
                value: json!({"tracks": { t3.to_string(): {"items": items(added)} }}),
            },
            // Track fields beside the items lock nothing.
            replace(DocPath::track(seq, t2).join("name")),
        ]));
        let mut all = vec![removed, mapped, added];
        all.sort();
        assert_eq!(locks.items(), all);
        // Edits of those items by another actor are caught, wherever they are addressed.
        let edit = DocPath::item(seq2, t3, added).join("name");
        assert_eq!(locks.touched_by(&Patch(vec![replace(edit)])), Some(added));
        assert_eq!(
            locks.touched_by(&Patch(vec![replace(DocPath::track(seq, t1))])),
            Some(removed)
        );
    }

    #[test]
    fn extending_reports_only_new_items() {
        let (s, t1, _, x, y) = ids();
        let mut locks = ItemLocks::default();
        assert!(locks.is_empty());
        let on = |item| Patch(vec![replace(DocPath::item(s, t1, item).join("name"))]);
        assert_eq!(locks.extend(&on(x)), 1);
        assert_eq!(locks.extend(&on(x)), 0);
        assert_eq!(locks.extend(&on(y)), 1);
        assert_eq!(locks.extend(&Patch(vec![replace(DocPath::meta())])), 0);
        let mut both = vec![x, y];
        both.sort();
        assert_eq!(locks.items(), both);
    }
}
