//! Transactions and invertible patch ops (arch §5.2).
//!
//! Every mutation goes through [`Tx`], which records an entity-level [`Patch`]
//! `{ before, after }` per change. The inverse of a patch is the same patch with `before` and
//! `after` swapped, so undo/redo are generic and need no per-command code. Patches are coalesced
//! per entity when the transaction finishes (first `before`, last `after`, no-ops dropped).

use std::{collections::BTreeMap, collections::HashMap, fmt};

use serde::Serialize;

use crate::{
    doc::{Doc, Item, MediaAsset, ProjectMeta, Sequence, Track},
    error::{CmdError, EntityKind},
    ids::{EntityId, IdGen, ItemId, MediaId, TrackId},
};

/// Document revision: +1 per applied transaction (including undo/redo).
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
    specta::Type,
)]
#[serde(transparent)]
pub struct Rev(#[specta(type = specta_typescript::Number)] pub u64);

impl fmt::Display for Rev {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// One recorded change. `None` means "absent".
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "entity", rename_all = "snake_case")]
pub enum Patch {
    /// Project metadata changed.
    Meta {
        /// Before.
        before: ProjectMeta,
        /// After.
        after: ProjectMeta,
    },
    /// Sequence changed.
    Sequence {
        /// Before.
        before: Sequence,
        /// After.
        after: Sequence,
    },
    /// Media inserted/updated/removed.
    Media {
        /// Id.
        id: MediaId,
        /// Before.
        before: Option<MediaAsset>,
        /// After.
        after: Option<MediaAsset>,
    },
    /// Track inserted/updated/removed.
    Track {
        /// Id.
        id: TrackId,
        /// Before.
        before: Option<Track>,
        /// After.
        after: Option<Track>,
    },
    /// Item inserted/updated/removed.
    Item {
        /// Id.
        id: ItemId,
        /// Before.
        before: Option<Item>,
        /// After.
        after: Option<Item>,
    },
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum PatchKey {
    Meta,
    Sequence,
    Media(MediaId),
    Track(TrackId),
    Item(ItemId),
}

/// Which side of a patch to compare against the document.
#[derive(Clone, Copy)]
pub(crate) enum Side {
    Before,
    After,
}

impl Patch {
    /// The inverse op.
    #[must_use]
    pub fn inverse(&self) -> Patch {
        match self.clone() {
            Patch::Meta { before, after } => Patch::Meta {
                before: after,
                after: before,
            },
            Patch::Sequence { before, after } => Patch::Sequence {
                before: after,
                after: before,
            },
            Patch::Media { id, before, after } => Patch::Media {
                id,
                before: after,
                after: before,
            },
            Patch::Track { id, before, after } => Patch::Track {
                id,
                before: after,
                after: before,
            },
            Patch::Item { id, before, after } => Patch::Item {
                id,
                before: after,
                after: before,
            },
        }
    }

    /// Human label of the touched entity, e.g. `item itm_…`.
    #[must_use]
    pub fn target(&self) -> String {
        match self {
            Patch::Meta { .. } => "project meta".into(),
            Patch::Sequence { after, .. } => format!("sequence {}", after.id),
            Patch::Media { id, .. } => format!("media {id}"),
            Patch::Track { id, .. } => format!("track {id}"),
            Patch::Item { id, .. } => format!("item {id}"),
        }
    }

    fn key(&self) -> PatchKey {
        match self {
            Patch::Meta { .. } => PatchKey::Meta,
            Patch::Sequence { .. } => PatchKey::Sequence,
            Patch::Media { id, .. } => PatchKey::Media(id.clone()),
            Patch::Track { id, .. } => PatchKey::Track(id.clone()),
            Patch::Item { id, .. } => PatchKey::Item(id.clone()),
        }
    }

    fn is_noop(&self) -> bool {
        match self {
            Patch::Meta { before, after } => before == after,
            Patch::Sequence { before, after } => before == after,
            Patch::Media { before, after, .. } => before == after,
            Patch::Track { before, after, .. } => before == after,
            Patch::Item { before, after, .. } => before == after,
        }
    }

    /// Takes `later`'s `after` (both patches touch the same entity).
    fn absorb(&mut self, later: Patch) {
        match (self, later) {
            (Patch::Meta { after, .. }, Patch::Meta { after: a, .. }) => *after = a,
            (Patch::Sequence { after, .. }, Patch::Sequence { after: a, .. }) => *after = a,
            (Patch::Media { after, .. }, Patch::Media { after: a, .. }) => *after = a,
            (Patch::Track { after, .. }, Patch::Track { after: a, .. }) => *after = a,
            (Patch::Item { after, .. }, Patch::Item { after: a, .. }) => *after = a,
            _ => unreachable!("absorb is only called for equal keys"),
        }
    }

    /// Writes `after` into the document.
    pub(crate) fn apply(&self, doc: &mut Doc) {
        fn set<K: Ord + Clone, V: Clone>(map: &mut BTreeMap<K, V>, id: &K, v: Option<&V>) {
            match v {
                Some(v) => {
                    map.insert(id.clone(), v.clone());
                }
                None => {
                    map.remove(id);
                }
            }
        }
        match self {
            Patch::Meta { after, .. } => doc.meta = after.clone(),
            Patch::Sequence { after, .. } => doc.sequence = after.clone(),
            Patch::Media { id, after, .. } => set(&mut doc.media, id, after.as_ref()),
            Patch::Track { id, after, .. } => set(&mut doc.tracks, id, after.as_ref()),
            Patch::Item { id, after, .. } => set(&mut doc.items, id, after.as_ref()),
        }
    }

    /// Whether the document currently holds this patch's `side` value for the entity.
    pub(crate) fn doc_matches(&self, doc: &Doc, side: Side) -> bool {
        fn pick<T>(side: Side, before: T, after: T) -> T {
            match side {
                Side::Before => before,
                Side::After => after,
            }
        }
        match self {
            Patch::Meta { before, after } => &doc.meta == pick(side, before, after),
            Patch::Sequence { before, after } => &doc.sequence == pick(side, before, after),
            Patch::Media { id, before, after } => {
                doc.media.get(id) == pick(side, before, after).as_ref()
            }
            Patch::Track { id, before, after } => {
                doc.tracks.get(id) == pick(side, before, after).as_ref()
            }
            Patch::Item { id, before, after } => {
                doc.items.get(id) == pick(side, before, after).as_ref()
            }
        }
    }
}

/// A map-stored entity the generic [`Tx`] helpers can insert, update and remove.
pub trait Entity: Clone + PartialEq + Sized {
    /// Id type.
    type Id: EntityId;
    /// Kind, for `NotFound` errors.
    const KIND: EntityKind;
    /// The entity's id.
    fn id(&self) -> &Self::Id;
    /// Its map in the document.
    fn map(doc: &Doc) -> &BTreeMap<Self::Id, Self>;
    /// Builds the patch for a change.
    fn patch(id: Self::Id, before: Option<Self>, after: Option<Self>) -> Patch;
}

macro_rules! entity {
    ($ty:ty, $id:ty, $kind:expr, $field:ident, $variant:ident) => {
        impl Entity for $ty {
            type Id = $id;
            const KIND: EntityKind = $kind;
            fn id(&self) -> &$id {
                &self.id
            }
            fn map(doc: &Doc) -> &BTreeMap<$id, Self> {
                &doc.$field
            }
            fn patch(id: $id, before: Option<Self>, after: Option<Self>) -> Patch {
                Patch::$variant { id, before, after }
            }
        }
    };
}
entity!(MediaAsset, MediaId, EntityKind::Media, media, Media);
entity!(Track, TrackId, EntityKind::Track, tracks, Track);
entity!(Item, ItemId, EntityKind::Item, items, Item);

/// A transaction over the document. Commands receive `&mut Tx` and can only mutate through it.
///
/// On error the engine calls [`Tx::rollback`], so a failing command leaves the doc untouched.
pub struct Tx<'a> {
    doc: &'a mut Doc,
    ids: &'a mut IdGen,
    patches: Vec<Patch>,
}

impl<'a> Tx<'a> {
    pub(crate) fn new(doc: &'a mut Doc, ids: &'a mut IdGen) -> Tx<'a> {
        Tx {
            doc,
            ids,
            patches: Vec::new(),
        }
    }

    /// Read access to the in-transaction document state.
    #[must_use]
    pub fn doc(&self) -> &Doc {
        self.doc
    }

    /// Mints a new id (deterministic in test mode).
    pub fn new_id<I: EntityId>(&mut self) -> I {
        self.ids.mint()
    }

    /// Looks an entity up or fails with `NotFound`.
    pub fn get<E: Entity>(&self, id: &E::Id) -> Result<&E, CmdError> {
        E::map(self.doc).get(id).ok_or_else(|| CmdError::NotFound {
            entity: E::KIND,
            id: id.to_string(),
        })
    }

    /// Records and applies a raw patch.
    pub(crate) fn record(&mut self, patch: Patch) {
        patch.apply(self.doc);
        self.patches.push(patch);
    }

    /// Inserts a new entity (its id must be fresh).
    pub fn insert<E: Entity>(&mut self, entity: E) {
        let id = entity.id().clone();
        let before = E::map(self.doc).get(&id).cloned();
        debug_assert!(before.is_none(), "insert of an existing id {id}");
        self.record(E::patch(id, before, Some(entity)));
    }

    /// Updates an entity in place; `f` may reject the change (nothing is written then).
    pub fn update<E: Entity>(
        &mut self,
        id: &E::Id,
        f: impl FnOnce(&mut E) -> Result<(), CmdError>,
    ) -> Result<(), CmdError> {
        let before = self.get::<E>(id)?.clone();
        let mut after = before.clone();
        f(&mut after)?;
        if after != before {
            self.record(E::patch(id.clone(), Some(before), Some(after)));
        }
        Ok(())
    }

    /// Removes an entity, returning it.
    pub fn remove<E: Entity>(&mut self, id: &E::Id) -> Result<E, CmdError> {
        let before = self.get::<E>(id)?.clone();
        self.record(E::patch(id.clone(), Some(before.clone()), None));
        Ok(before)
    }

    /// Updates the sequence.
    pub fn update_sequence(&mut self, f: impl FnOnce(&mut Sequence)) {
        let before = self.doc.sequence.clone();
        let mut after = before.clone();
        f(&mut after);
        if after != before {
            self.record(Patch::Sequence { before, after });
        }
    }

    /// Replaces project metadata.
    pub fn set_meta(&mut self, meta: ProjectMeta) {
        let before = self.doc.meta.clone();
        if before != meta {
            self.record(Patch::Meta {
                before,
                after: meta,
            });
        }
    }

    /// Reverts everything recorded so far (used on error and for dry runs).
    pub(crate) fn rollback(self) {
        for p in self.patches.iter().rev() {
            p.inverse().apply(self.doc);
        }
    }

    /// Ends the transaction, returning coalesced forward patches (first `before`, last `after`
    /// per entity, ordered by first touch, no-ops dropped).
    pub(crate) fn finish(self) -> Vec<Patch> {
        coalesce(self.patches)
    }
}

pub(crate) fn coalesce(patches: Vec<Patch>) -> Vec<Patch> {
    let mut out: Vec<Patch> = Vec::with_capacity(patches.len());
    let mut index: HashMap<PatchKey, usize> = HashMap::new();
    for p in patches {
        if let Some(&i) = index.get(&p.key()) {
            out[i].absorb(p);
        } else {
            index.insert(p.key(), out.len());
            out.push(p);
        }
    }
    out.retain(|p| !p.is_noop());
    out
}
