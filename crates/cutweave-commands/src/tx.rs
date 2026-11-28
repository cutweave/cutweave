// SPDX-License-Identifier: GPL-3.0-or-later
//! Transactions (arch §5.2, ADR-0005): every document mutation goes through a [`Tx`], which
//! records an invertible [`PatchOp`] per change.
//!
//! - [`Transaction`] is the owned state of an open transaction: the base document, the working
//!   document and the ops recorded so far. The document actor (WP-1B.03) keeps one per open
//!   transaction (an implicit single-command one, or an explicit batch, gesture or proposal,
//!   WP-1B.04/09) and [`commit`](Transaction::commit)s or [`cancel`](Transaction::cancel)s it.
//! - [`Tx`] is the handle a command receives: the transaction plus the engine's [`IdGen`] and
//!   the caller's [`Access`]. Commands read through it and mutate only through it.
//!
//! The working document is changed **only** by applying the op being recorded, so the recorded
//! ops replay the transaction exactly, and undo needs no per-command code: it applies the
//! inverse patch. Each op is applied to a copy (O(1) with the persistent collections) that
//! replaces the working document only on success, so a failed or panicking command never
//! leaves a half-applied op behind.

use cutweave_model::{
    EntityId, EntityKind, IdGen, Item, ItemId, ItemRef, MediaAsset, MediaId, Project, ProjectMeta,
    ProjectSettings, Sequence, SequenceId, Track, TrackId,
};
use serde::Serialize;
use serde_json::Value;

use crate::{
    apply,
    ctx::{
        Access, QueryCtx, find_item, find_media, find_sequence, find_track, item_exists, item_ids,
        unlocked,
    },
    diff::{self, diff},
    error::CmdError,
    json::json_eq,
    patch::{Patch, PatchError, PatchOp},
    path::DocPath,
};

/// An open transaction: base document, working document and recorded ops.
#[derive(Clone, Debug)]
pub struct Transaction {
    base: Project,
    doc: Project,
    ops: Vec<PatchOp>,
    /// `ops` is known to be exactly `diff(base, doc)`: [`commit`](Transaction::commit) then
    /// takes it as the net patch without computing and verifying that diff again (which costs
    /// as much as the edit itself: issues #230, #231). See [`Transaction::record_diff`].
    exact: bool,
}

/// A savepoint inside a [`Transaction`] (see [`Transaction::rollback_to`]).
#[derive(Clone, Debug)]
pub struct Mark {
    len: usize,
    doc: Project,
    exact: bool,
}

/// A committed transaction.
#[derive(Clone, Debug)]
pub struct Committed {
    /// The document before the transaction.
    pub base: Project,
    /// The document after it.
    pub doc: Project,
    /// The net change, `base` → `doc`: field-level and coalesced (200 updates of one field in a
    /// gesture become one op). What the journal records and redo applies.
    pub patch: Patch,
    /// `invert(patch)`: what undo applies.
    pub inverse: Patch,
}

impl Committed {
    /// Whether the transaction changed nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.patch.is_empty()
    }
}

impl Transaction {
    /// Opens a transaction on `base` (O(1): the working copy shares its structure).
    #[must_use]
    pub fn begin(base: Project) -> Transaction {
        Transaction {
            doc: base.clone(),
            base,
            ops: Vec::new(),
            exact: false,
        }
    }

    /// The document the transaction started from.
    #[must_use]
    pub fn base(&self) -> &Project {
        &self.base
    }

    /// The working document (with every recorded op applied).
    #[must_use]
    pub fn doc(&self) -> &Project {
        &self.doc
    }

    /// The ops recorded so far, in order (what live `doc.patched` events carry).
    #[must_use]
    pub fn ops(&self) -> &[PatchOp] {
        &self.ops
    }

    /// Whether nothing was recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// A handle for running commands in this transaction.
    pub fn tx<'a>(&'a mut self, ids: &'a mut dyn IdGen, access: Access) -> Tx<'a> {
        Tx {
            txn: self,
            ids,
            access,
        }
    }

    /// A savepoint at the current state (O(1)).
    #[must_use]
    pub fn mark(&self) -> Mark {
        Mark {
            len: self.ops.len(),
            doc: self.doc.clone(),
            exact: self.exact,
        }
    }

    /// The ops recorded since `mark` (e.g. by one command of a gesture, for its live event).
    #[must_use]
    pub fn ops_since(&self, mark: &Mark) -> &[PatchOp] {
        self.ops.get(mark.len..).unwrap_or(&[])
    }

    /// Reverts everything recorded since `mark` (O(1)). The registry does this for each failed
    /// command, so a failing command inside a batch or gesture leaves the earlier ones intact.
    pub fn rollback_to(&mut self, mark: Mark) {
        debug_assert!(mark.len <= self.ops.len(), "mark from a later state");
        self.ops.truncate(mark.len);
        self.doc = mark.doc;
        self.exact = mark.exact;
    }

    /// Ends the transaction. The net patch is the verified [`diff`] of base and result when it
    /// can be expressed field by field, otherwise the recorded ops. When the recorded ops are
    /// known to be that diff already (one diff-based edit, the usual command), they are taken
    /// as they are.
    #[must_use]
    pub fn commit(self) -> Committed {
        let patch = if self.ops.is_empty() {
            Patch::new()
        } else if self.exact {
            // Debug builds (every test run) compute the diff as well and compare.
            #[cfg(debug_assertions)]
            crate::effort::uncounted(|| {
                debug_assert_eq!(
                    diff(&self.base, &self.doc).ok().as_ref().map(|p| &p.0),
                    Some(&self.ops),
                    "the recorded ops are not the diff of the transaction"
                );
            });
            Patch(self.ops)
        } else {
            diff(&self.base, &self.doc).unwrap_or(Patch(self.ops))
        };
        Committed {
            inverse: patch.invert(),
            patch,
            base: self.base,
            doc: self.doc,
        }
    }

    /// Abandons the transaction and returns the base document.
    #[must_use]
    pub fn cancel(self) -> Project {
        self.base
    }

    /// Applies `ops` to a copy of the working document, then records them. All or nothing.
    fn record(&mut self, ops: Vec<PatchOp>) -> Result<(), CmdError> {
        if ops.is_empty() {
            return Ok(());
        }
        let mut next = self.doc.clone();
        for op in &ops {
            check_locks(&next, op)?;
            apply::apply_op(&mut next, op).map_err(patch_failed)?;
        }
        self.doc = next;
        self.ops.extend(ops);
        self.exact = false;
        Ok(())
    }

    /// [`record`](Self::record) for ops that are the field-level diff of one entity of the
    /// working document and its edited copy; `reached` says whether the document they led to
    /// holds exactly that copy.
    ///
    /// When they are the transaction's first ops and `reached` holds, they are the transaction's
    /// net patch as [`diff`] would compute and verify it: the working document was the base, so
    /// `diff(base, doc)` visits this one entity with the same two values and yields the same
    /// ops, and applying them to the base is how the working document was made. `commit` then
    /// skips that second diff and its verification.
    fn record_diff(
        &mut self,
        ops: Vec<PatchOp>,
        reached: impl FnOnce(&Project) -> bool,
    ) -> Result<(), CmdError> {
        let first = self.ops.is_empty();
        let any = !ops.is_empty();
        self.record(ops)?;
        if first && any {
            self.exact = reached(&self.doc);
        }
        Ok(())
    }
}

#[allow(clippy::needless_pass_by_value)] // used as `map_err(patch_failed)`
fn patch_failed(e: PatchError) -> CmdError {
    CmdError::internal(format!("a document edit failed: {e}"))
}

/// Refuses ops on the items of a locked track, and removing or replacing a locked track.
/// Edits of a locked track's own fields (e.g. unlocking it) are allowed.
fn check_locks(doc: &Project, op: &PatchOp) -> Result<(), CmdError> {
    let whole_track_change = !matches!(op, PatchOp::Add { .. });
    for (i, path) in op.paths().enumerate() {
        // For a move, `from` (i == 0) is removed: like a removal.
        let removes = whole_track_change || (i == 0 && matches!(op, PatchOp::Move { .. }));
        let segments = path.to_segments();
        let [s0, sequence, s2, track, rest @ ..] = segments.as_slice() else {
            continue;
        };
        if s0 != "sequences" || s2 != "tracks" {
            continue;
        }
        let touches_items = match rest.first() {
            None => removes,
            Some(head) => head == "items",
        };
        if !touches_items {
            continue;
        }
        let (Ok(sequence), Ok(track)) = (sequence.parse::<SequenceId>(), track.parse::<TrackId>())
        else {
            continue;
        };
        if let Some(track) = doc.sequences.get(&sequence).and_then(|s| s.track(&track)) {
            unlocked(track)?;
        }
    }
    Ok(())
}

fn to_value<T: Serialize>(value: &T) -> Result<Value, CmdError> {
    serde_json::to_value(value)
        .map_err(|e| CmdError::internal(format!("value serialization failed: {e}")))
}

fn already_exists(kind: EntityKind, id: impl std::fmt::Display) -> CmdError {
    CmdError::invalid("", format!("{kind} `{id}` already exists"))
}

/// The handle a [`Command`](crate::Command) mutates the document through.
///
/// Reads see the transaction's working document. Item edits are refused with `Locked` on a
/// locked track. Every mutator is all-or-nothing.
pub struct Tx<'a> {
    txn: &'a mut Transaction,
    ids: &'a mut dyn IdGen,
    access: Access,
}

impl Tx<'_> {
    /// The working document.
    #[must_use]
    pub fn doc(&self) -> &Project {
        &self.txn.doc
    }

    /// The caller's access.
    #[must_use]
    pub fn access(&self) -> Access {
        self.access
    }

    /// A read context over the working document (for queries run inside a transaction).
    #[must_use]
    pub fn query_ctx(&self) -> QueryCtx<'_> {
        QueryCtx::new(&self.txn.doc, self.access)
    }

    /// A fresh ID from the engine's generator (seeded under `--deterministic-ids`, WP-1B.08).
    pub fn new_id<I: EntityId>(&mut self) -> I {
        I::new_with(&mut *self.ids)
    }

    /// A savepoint for trying something and backing out (see [`Transaction::mark`]).
    #[must_use]
    pub fn savepoint(&self) -> Mark {
        self.txn.mark()
    }

    /// Reverts the changes made since `mark`.
    pub fn rollback_to(&mut self, mark: Mark) {
        self.txn.rollback_to(mark);
    }

    /// The ops recorded since `mark`.
    #[must_use]
    pub fn ops_since(&self, mark: &Mark) -> &[PatchOp] {
        self.txn.ops_since(mark)
    }

    // ----- lookups ---------------------------------------------------------------------------

    /// The media asset, or `NotFound`.
    pub fn media(&self, id: MediaId) -> Result<&MediaAsset, CmdError> {
        find_media(&self.txn.doc, id)
    }

    /// The sequence, or `NotFound`.
    pub fn sequence(&self, id: SequenceId) -> Result<&Sequence, CmdError> {
        find_sequence(&self.txn.doc, id)
    }

    /// The track and its sequence, or `NotFound`.
    pub fn track(&self, id: TrackId) -> Result<(&Sequence, &Track), CmdError> {
        find_track(&self.txn.doc, id)
    }

    /// The item with its sequence and track, or `NotFound`.
    pub fn item(&self, id: ItemId) -> Result<ItemRef<'_>, CmdError> {
        find_item(&self.txn.doc, id)
    }

    /// The value at `path` in the working document's JSON form, if any.
    #[must_use]
    pub fn get(&self, path: &DocPath) -> Option<Value> {
        apply::get(&self.txn.doc, path).ok()
    }

    // ----- generic mutation ------------------------------------------------------------------

    /// Applies and records one op (its preimage must match). Prefer the typed helpers, which
    /// build the ops for you.
    pub fn apply(&mut self, op: PatchOp) -> Result<(), CmdError> {
        self.txn.record(vec![op])
    }

    /// Applies and records `ops` in order, all or nothing.
    pub fn apply_all(&mut self, ops: impl IntoIterator<Item = PatchOp>) -> Result<(), CmdError> {
        self.txn.record(ops.into_iter().collect())
    }

    /// Sets the value at `path`: `replace` if present (nothing if equal), `add` if absent.
    pub fn set(&mut self, path: &DocPath, value: impl Serialize) -> Result<(), CmdError> {
        let value = to_value(&value)?;
        match apply::get(&self.txn.doc, path) {
            Ok(old) if json_eq(&old, &value) => Ok(()),
            Ok(old) => self.apply(PatchOp::Replace {
                path: path.clone(),
                old,
                value,
            }),
            Err(PatchError::NotFound { .. }) => self.apply(PatchOp::Add {
                path: path.clone(),
                value,
            }),
            Err(e) => Err(patch_failed(e)),
        }
    }

    /// Removes the value at `path` and returns it.
    pub fn remove_path(&mut self, path: &DocPath) -> Result<Value, CmdError> {
        let old = apply::get(&self.txn.doc, path).map_err(patch_failed)?;
        self.apply(PatchOp::Remove {
            path: path.clone(),
            old: old.clone(),
        })?;
        Ok(old)
    }

    /// Edits the whole document with `f` and records the difference. The most general
    /// mutator; the targeted ones below are cheaper and say what they touch.
    pub fn update<R>(
        &mut self,
        f: impl FnOnce(&mut Project) -> Result<R, CmdError>,
    ) -> Result<R, CmdError> {
        let mut after = self.txn.doc.clone();
        let out = f(&mut after)?;
        let patch = diff(&self.txn.doc, &after).map_err(patch_failed)?;
        // `diff` verified that these ops lead from the working document to `after`.
        self.txn.record_diff(patch.0, |_| true)?;
        Ok(out)
    }

    /// Edits the project metadata.
    pub fn update_meta<R>(
        &mut self,
        f: impl FnOnce(&mut ProjectMeta) -> Result<R, CmdError>,
    ) -> Result<R, CmdError> {
        self.update(|doc| f(&mut doc.meta))
    }

    /// Edits the project settings.
    pub fn update_settings<R>(
        &mut self,
        f: impl FnOnce(&mut ProjectSettings) -> Result<R, CmdError>,
    ) -> Result<R, CmdError> {
        self.update(|doc| f(&mut doc.settings))
    }

    // ----- media -----------------------------------------------------------------------------

    /// Adds a media asset (its ID must be new).
    pub fn insert_media(&mut self, media: &MediaAsset) -> Result<(), CmdError> {
        if self.txn.doc.media.contains_key(&media.id) {
            return Err(already_exists(EntityKind::Media, media.id));
        }
        let value = to_value(media)?;
        self.apply(PatchOp::Add {
            path: DocPath::media(media.id),
            value,
        })
    }

    /// Removes a media asset and returns it. Does not check references (commands do).
    pub fn remove_media(&mut self, id: MediaId) -> Result<MediaAsset, CmdError> {
        let media = self.media(id)?.clone();
        let old = to_value(&media)?;
        self.apply(PatchOp::Remove {
            path: DocPath::media(id),
            old,
        })?;
        Ok(media)
    }

    /// Edits a media asset (its ID cannot change).
    pub fn update_media<R>(
        &mut self,
        id: MediaId,
        f: impl FnOnce(&mut MediaAsset) -> Result<R, CmdError>,
    ) -> Result<R, CmdError> {
        let before = self.media(id)?.clone();
        let mut after = before.clone();
        let out = f(&mut after)?;
        if after.id != id {
            return Err(CmdError::internal(
                "update_media must not change the media id",
            ));
        }
        let mut ops = Vec::new();
        diff::diff_value(&DocPath::media(id), &before, &after, &mut ops).map_err(patch_failed)?;
        self.txn
            .record_diff(ops, |doc| doc.media.get(&id) == Some(&after))?;
        Ok(out)
    }

    // ----- sequences -------------------------------------------------------------------------

    /// Adds a sequence (its ID, and the IDs of any tracks and items in it, must be new).
    pub fn insert_sequence(&mut self, sequence: &Sequence) -> Result<(), CmdError> {
        if self.txn.doc.sequences.contains_key(&sequence.id) {
            return Err(already_exists(EntityKind::Sequence, sequence.id));
        }
        for track in sequence.tracks.values() {
            self.check_new_track(track)?;
        }
        let value = to_value(sequence)?;
        self.apply(PatchOp::Add {
            path: DocPath::sequence(sequence.id),
            value,
        })
    }

    /// Removes a sequence and returns it. Does not check compound-clip references.
    pub fn remove_sequence(&mut self, id: SequenceId) -> Result<Sequence, CmdError> {
        let sequence = self.sequence(id)?.clone();
        let old = to_value(&sequence)?;
        self.apply(PatchOp::Remove {
            path: DocPath::sequence(id),
            old,
        })?;
        Ok(sequence)
    }

    /// Edits a sequence, including its tracks and items (IDs cannot change).
    pub fn update_sequence<R>(
        &mut self,
        id: SequenceId,
        f: impl FnOnce(&mut Sequence) -> Result<R, CmdError>,
    ) -> Result<R, CmdError> {
        let before = self.sequence(id)?.clone();
        let mut after = before.clone();
        let out = f(&mut after)?;
        if after.id != id {
            return Err(CmdError::internal(
                "update_sequence must not change the sequence id",
            ));
        }
        let mut ops = Vec::new();
        diff::diff_sequence(&DocPath::sequence(id), &before, &after, &mut ops)
            .map_err(patch_failed)?;
        self.txn
            .record_diff(ops, |doc| doc.sequences.get(&id) == Some(&after))?;
        Ok(out)
    }

    // ----- tracks ----------------------------------------------------------------------------

    fn check_new_track(&self, track: &Track) -> Result<(), CmdError> {
        if find_track(&self.txn.doc, track.id).is_ok() {
            return Err(already_exists(EntityKind::Track, track.id));
        }
        if let Some(id) = item_ids(track).find(|id| item_exists(&self.txn.doc, *id)) {
            return Err(already_exists(EntityKind::Item, id));
        }
        Ok(())
    }

    /// Adds `track` to `sequence` at display position `index` (clamped to the end; 0 = top).
    pub fn insert_track(
        &mut self,
        sequence: SequenceId,
        index: usize,
        track: &Track,
    ) -> Result<(), CmdError> {
        let order_len = self.sequence(sequence)?.track_order.len();
        self.check_new_track(track)?;
        let id = track.id;
        let value = to_value(track)?;
        self.apply_all([
            PatchOp::Add {
                path: DocPath::track(sequence, id),
                value,
            },
            PatchOp::Add {
                path: DocPath::sequence(sequence)
                    .join("track_order")
                    .join(index.min(order_len)),
                value: to_value(&id)?,
            },
        ])
    }

    /// Removes a track (with its items) and returns it. `Locked` if the track is locked.
    pub fn remove_track(&mut self, id: TrackId) -> Result<Track, CmdError> {
        let (sequence, track) = self.track(id)?;
        let (sequence, track) = (sequence.id, track.clone());
        let position = self.sequence(sequence)?.track_order.index_of(&id);
        let mut ops = Vec::new();
        if let Some(position) = position {
            ops.push(PatchOp::Remove {
                path: DocPath::sequence(sequence)
                    .join("track_order")
                    .join(position),
                old: to_value(&id)?,
            });
        }
        ops.push(PatchOp::Remove {
            path: DocPath::track(sequence, id),
            old: to_value(&track)?,
        });
        self.txn.record(ops)?;
        Ok(track)
    }

    /// Edits a track. Its own fields (name, flags, gain) can always change, also while it is
    /// locked; changing its items while it is locked is `Locked`.
    pub fn update_track<R>(
        &mut self,
        id: TrackId,
        f: impl FnOnce(&mut Track) -> Result<R, CmdError>,
    ) -> Result<R, CmdError> {
        let (sequence, before) = self.track(id)?;
        let (sequence, before) = (sequence.id, before.clone());
        let mut after = before.clone();
        let out = f(&mut after)?;
        if after.id != id {
            return Err(CmdError::internal(
                "update_track must not change the track id",
            ));
        }
        let mut ops = Vec::new();
        diff::diff_track(&DocPath::track(sequence, id), &before, &after, &mut ops)
            .map_err(patch_failed)?;
        self.txn.record_diff(ops, |doc| {
            find_track(doc, id).is_ok_and(|(_, track)| *track == after)
        })?;
        Ok(out)
    }

    // ----- items -----------------------------------------------------------------------------

    /// Adds `item` to `track`. The item ID must be new; `Locked` on a locked track. Does not
    /// check overlaps (commands do, arch §4.3).
    pub fn insert_item(&mut self, track: TrackId, item: &Item) -> Result<(), CmdError> {
        let (sequence, _) = self.track(track)?;
        let sequence = sequence.id;
        if item_exists(&self.txn.doc, item.id()) {
            return Err(already_exists(EntityKind::Item, item.id()));
        }
        let value = to_value(item)?;
        self.apply(PatchOp::Add {
            path: DocPath::item(sequence, track, item.id()),
            value,
        })
    }

    /// Removes an item and returns it; `Locked` on a locked track.
    pub fn remove_item(&mut self, id: ItemId) -> Result<Item, CmdError> {
        let at = self.item(id)?;
        let (path, item) = (
            DocPath::item(at.sequence.id, at.track.id, id),
            at.item.clone(),
        );
        self.apply(PatchOp::Remove {
            path,
            old: to_value(&item)?,
        })?;
        Ok(item)
    }

    /// Edits an item (its ID cannot change) and records the changed fields; `Locked` on a
    /// locked track. Moving it in time is a change of `start`; to another track, use
    /// [`move_item`](Self::move_item).
    pub fn update_item<R>(
        &mut self,
        id: ItemId,
        f: impl FnOnce(&mut Item) -> Result<R, CmdError>,
    ) -> Result<R, CmdError> {
        let at = self.item(id)?;
        let (path, before) = (
            DocPath::item(at.sequence.id, at.track.id, id),
            at.item.clone(),
        );
        let mut after = before.clone();
        let out = f(&mut after)?;
        if after.id() != id {
            return Err(CmdError::internal(
                "update_item must not change the item id",
            ));
        }
        let mut ops = Vec::new();
        diff::diff_value(&path, &before, &after, &mut ops).map_err(patch_failed)?;
        self.txn.record_diff(ops, |doc| {
            find_item(doc, id).is_ok_and(|at| *at.item == after)
        })?;
        Ok(out)
    }

    /// Moves an item to another track (of any sequence), keeping its fields. `Locked` if
    /// either track is locked. Does not check track kinds or overlaps (commands do).
    pub fn move_item(&mut self, id: ItemId, to_track: TrackId) -> Result<(), CmdError> {
        let at = self.item(id)?;
        let (from_sequence, from_track) = (at.sequence.id, at.track.id);
        let (to_sequence, _) = self.track(to_track)?;
        let to_sequence = to_sequence.id;
        if from_track == to_track {
            return Ok(());
        }
        self.apply(PatchOp::Move {
            from: DocPath::item(from_sequence, from_track, id),
            path: DocPath::item(to_sequence, to_track, id),
        })
    }
}
