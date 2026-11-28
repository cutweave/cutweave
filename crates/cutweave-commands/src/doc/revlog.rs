// SPDX-License-Identifier: GPL-3.0-or-later
//! What each recent revision touched (WP-2I.04), so a `Conflict{reason: stale_rev}` can tell a
//! client what changed between the revision it expected and the current one
//! ([`ChangedSince`]).
//!
//! The log holds one entry per change of the live document (one `doc.patched` or `doc.reset`
//! event), the newest [`CHANGED_SINCE_REVS`] of them: the revision the change led to and the
//! items its patch touched, in the sense of the soft locks (module `locks`: added, removed,
//! moved or edited, and the items inside a whole track or sequence that came or went). It is
//! not journaled: a document the engine has just opened or recovered starts with an empty log,
//! and a revision from before that is answered with [`ChangedSince::Unknown`].

use std::collections::{BTreeSet, VecDeque};

use cutweave_model::ItemId;

use super::locks::ItemLocks;
use crate::{
    error::{ChangedSince, Rev},
    patch::Patch,
};

/// How many changes of the live document the engine remembers the touched items of. A stale
/// `expected_rev` older than that gets `changed_since: "unknown"`.
pub const CHANGED_SINCE_REVS: usize = 200;

/// The most items a `changed_since` list names. More than that (in one change, or in all the
/// changes since `expected_rev` together) is reported as `"unknown"`: the caller reads the
/// document again either way, and the error stays small.
pub const CHANGED_SINCE_ITEMS: usize = 100;

/// One change of the live document.
#[derive(Clone, Debug)]
struct Change {
    /// The revision it led to.
    rev: Rev,
    /// The items it touched; `None` when that is not known (a re-composed live document) or
    /// more than [`CHANGED_SINCE_ITEMS`].
    items: Option<Vec<ItemId>>,
}

/// The touched items of the newest [`CHANGED_SINCE_REVS`] changes.
#[derive(Clone, Debug)]
pub(crate) struct RevLog {
    /// Every change after this revision is in `changes`.
    floor: Rev,
    changes: VecDeque<Change>,
}

impl RevLog {
    /// An empty log of a document at `rev`.
    pub(crate) fn new(rev: Rev) -> RevLog {
        RevLog {
            floor: rev,
            changes: VecDeque::new(),
        }
    }

    /// Records the change that led to `rev`: `patch` is what it did to the live document,
    /// `None` when the live document was re-composed (`doc.reset`).
    pub(crate) fn record(&mut self, rev: Rev, patch: Option<&Patch>) {
        let items = patch
            .map(|patch| ItemLocks::of(patch).items())
            .filter(|items| items.len() <= CHANGED_SINCE_ITEMS);
        self.changes.push_back(Change { rev, items });
        while self.changes.len() > CHANGED_SINCE_REVS {
            if let Some(dropped) = self.changes.pop_front() {
                self.floor = dropped.rev;
            }
        }
    }

    /// What changed after `expected` in a document now at `current`.
    pub(crate) fn since(&self, expected: Rev, current: Rev) -> ChangedSince {
        if expected < self.floor || expected >= current {
            return ChangedSince::Unknown;
        }
        let mut touched = BTreeSet::new();
        for change in self
            .changes
            .iter()
            .filter(|c| c.rev > expected && c.rev <= current)
        {
            let Some(items) = &change.items else {
                return ChangedSince::Unknown;
            };
            touched.extend(items.iter().copied());
            if touched.len() > CHANGED_SINCE_ITEMS {
                return ChangedSince::Unknown;
            }
        }
        ChangedSince::Items(touched.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use cutweave_model::{IdGen, SeededGen, SequenceId, TrackId};
    use serde_json::json;

    use super::*;
    use crate::{patch::PatchOp, path::DocPath};

    /// `n` item ids, in id order.
    fn items(n: usize) -> Vec<ItemId> {
        let mut g = SeededGen::new(7);
        let mut ids: Vec<ItemId> = (0..n).map(|_| g.mint()).collect();
        ids.sort();
        ids
    }

    fn touch(items: &[ItemId]) -> Patch {
        let mut g = SeededGen::new(9);
        let (sequence, track): (SequenceId, TrackId) = (g.mint(), g.mint());
        Patch(
            items
                .iter()
                .map(|item| PatchOp::Replace {
                    path: DocPath::item(sequence, track, *item).join("name"),
                    old: json!("a"),
                    value: json!("b"),
                })
                .collect(),
        )
    }

    #[test]
    fn it_names_the_items_touched_after_the_expected_revision() {
        let id = items(3);
        let mut log = RevLog::new(Rev(4));
        log.record(Rev(5), Some(&touch(&[id[2]])));
        log.record(Rev(6), Some(&Patch::new()));
        log.record(Rev(7), Some(&touch(&[id[1], id[0]])));
        assert_eq!(log.since(Rev(4), Rev(7)), ChangedSince::Items(id.clone()));
        assert_eq!(
            log.since(Rev(5), Rev(7)),
            ChangedSince::Items(vec![id[0], id[1]])
        );
        assert_eq!(
            log.since(Rev(6), Rev(7)),
            ChangedSince::Items(vec![id[0], id[1]])
        );
        // A change that touched no item.
        assert_eq!(log.since(Rev(5), Rev(6)), ChangedSince::Items(vec![]));
    }

    #[test]
    fn a_revision_it_does_not_cover_is_unknown() {
        let id = items(3);
        let mut log = RevLog::new(Rev(4));
        log.record(Rev(5), Some(&touch(&[id[1]])));
        // Before the document was opened, the current revision, a future one.
        assert_eq!(log.since(Rev(3), Rev(5)), ChangedSince::Unknown);
        assert_eq!(log.since(Rev(5), Rev(5)), ChangedSince::Unknown);
        assert_eq!(log.since(Rev(9), Rev(5)), ChangedSince::Unknown);
        // A re-composed live document in between.
        log.record(Rev(6), None);
        log.record(Rev(7), Some(&touch(&[id[2]])));
        assert_eq!(log.since(Rev(4), Rev(7)), ChangedSince::Unknown);
        assert_eq!(log.since(Rev(6), Rev(7)), ChangedSince::Items(vec![id[2]]));
    }

    #[test]
    fn it_keeps_the_last_200_changes() {
        let id = items(3);
        let mut log = RevLog::new(Rev(0));
        for n in 1..=250_usize {
            log.record(Rev(n as u64), Some(&touch(&[id[n % 3]])));
        }
        assert_eq!(log.changes.len(), CHANGED_SINCE_REVS);
        assert_eq!(log.since(Rev(49), Rev(250)), ChangedSince::Unknown);
        assert_eq!(log.since(Rev(50), Rev(250)), ChangedSince::Items(id));
    }

    #[test]
    fn more_items_than_the_list_holds_is_unknown() {
        let many = items(CHANGED_SINCE_ITEMS + 1);
        let mut log = RevLog::new(Rev(0));
        log.record(Rev(1), Some(&touch(&many)));
        assert_eq!(log.since(Rev(0), Rev(1)), ChangedSince::Unknown);
        // Spread over several changes too.
        let mut log = RevLog::new(Rev(0));
        log.record(Rev(1), Some(&touch(&many[..60])));
        log.record(Rev(2), Some(&touch(&many[40..])));
        assert_eq!(
            log.since(Rev(1), Rev(2)),
            ChangedSince::Items(many[40..].to_vec())
        );
        assert_eq!(log.since(Rev(0), Rev(2)), ChangedSince::Unknown);
    }
}
