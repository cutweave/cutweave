// SPDX-License-Identifier: GPL-3.0-or-later
//! [`Access`] (what the caller's credential may do) and [`QueryCtx`] (what a query reads), plus
//! the ID lookups shared by queries and transactions.

use cutweave_model::{
    EntityKind, Item, ItemId, ItemRef, MediaAsset, MediaId, Project, Sequence, SequenceId, Track,
    TrackId,
};

use crate::{
    error::{CmdError, Rev},
    meta::{CommandMeta, Scope},
};

/// What the caller's credential may invoke (arch §6, ADR-0016).
///
/// The engine derives it from the authenticated credential, never from the request: the UI
/// credential gets [`Access::UI`], agent, plugin and CLI tokens and internal jobs get
/// [`Access::STANDARD`]. The registry checks it against each entry's [`Scope`] on every call
/// (also for every step of a batch), so UI-only commands cannot be reached any other way.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Access {
    ui: bool,
}

impl Access {
    /// The UI credential: every scope.
    pub const UI: Access = Access { ui: true };
    /// Any other credential: everything except UI-only scopes.
    pub const STANDARD: Access = Access { ui: false };

    /// Whether this access may invoke an entry of `scope`.
    #[must_use]
    pub const fn allows(self, scope: Scope) -> bool {
        match scope {
            Scope::Any => true,
            Scope::UiOnly => self.ui,
        }
    }

    /// `Forbidden` unless this access may invoke `meta`.
    pub fn check(self, meta: &CommandMeta) -> Result<(), CmdError> {
        if self.allows(meta.scope) {
            Ok(())
        } else {
            Err(CmdError::Forbidden {
                scope: meta.scope,
                msg: format!(
                    "`{}` can only be used from the Cutweave editor UI; ask the user to do it there",
                    meta.name
                ),
            })
        }
    }
}

/// What a [`Query`](crate::Query) (and a command's [`validate`](crate::Command::validate)
/// hook) reads: an immutable document snapshot and the caller's access.
#[derive(Clone, Copy, Debug)]
pub struct QueryCtx<'a> {
    doc: &'a Project,
    access: Access,
    rev: Option<Rev>,
}

impl<'a> QueryCtx<'a> {
    /// A context over `doc` for a caller with `access`.
    #[must_use]
    pub const fn new(doc: &'a Project, access: Access) -> QueryCtx<'a> {
        QueryCtx {
            doc,
            access,
            rev: None,
        }
    }

    /// The same context, stating the snapshot's revision.
    #[must_use]
    pub const fn with_rev(mut self, rev: Rev) -> QueryCtx<'a> {
        self.rev = Some(rev);
        self
    }

    /// The document snapshot.
    #[must_use]
    pub const fn doc(&self) -> &'a Project {
        self.doc
    }

    /// The caller's access.
    #[must_use]
    pub const fn access(&self) -> Access {
        self.access
    }

    /// The snapshot's revision, when the engine states it.
    #[must_use]
    pub const fn rev(&self) -> Option<Rev> {
        self.rev
    }

    /// The media asset, or `NotFound`.
    pub fn media(&self, id: MediaId) -> Result<&'a MediaAsset, CmdError> {
        find_media(self.doc, id)
    }

    /// The sequence, or `NotFound`.
    pub fn sequence(&self, id: SequenceId) -> Result<&'a Sequence, CmdError> {
        find_sequence(self.doc, id)
    }

    /// The track and its sequence, or `NotFound`.
    pub fn track(&self, id: TrackId) -> Result<(&'a Sequence, &'a Track), CmdError> {
        find_track(self.doc, id)
    }

    /// The item with its sequence and track, or `NotFound`.
    pub fn item(&self, id: ItemId) -> Result<ItemRef<'a>, CmdError> {
        find_item(self.doc, id)
    }
}

pub(crate) fn find_media(doc: &Project, id: MediaId) -> Result<&MediaAsset, CmdError> {
    doc.media
        .get(&id)
        .ok_or_else(|| CmdError::not_found(EntityKind::Media, id))
}

pub(crate) fn find_sequence(doc: &Project, id: SequenceId) -> Result<&Sequence, CmdError> {
    doc.sequences
        .get(&id)
        .ok_or_else(|| CmdError::not_found(EntityKind::Sequence, id))
}

pub(crate) fn find_track(doc: &Project, id: TrackId) -> Result<(&Sequence, &Track), CmdError> {
    doc.sequences
        .values()
        .find_map(|s| s.track(&id).map(|t| (s, t)))
        .ok_or_else(|| CmdError::not_found(EntityKind::Track, id))
}

pub(crate) fn find_item(doc: &Project, id: ItemId) -> Result<ItemRef<'_>, CmdError> {
    doc.find_item(&id)
        .ok_or_else(|| CmdError::not_found(EntityKind::Item, id))
}

/// `Locked` if `track` is locked.
pub(crate) fn unlocked(track: &Track) -> Result<(), CmdError> {
    if track.locked {
        Err(CmdError::locked(EntityKind::Track, track.id))
    } else {
        Ok(())
    }
}

/// Whether any item of `doc` has this ID (IDs are unique across the whole document).
pub(crate) fn item_exists(doc: &Project, id: ItemId) -> bool {
    doc.find_item(&id).is_some()
}

/// IDs of the items of `track`.
pub(crate) fn item_ids(track: &Track) -> impl Iterator<Item = ItemId> + '_ {
    track
        .items
        .iter_by_id()
        .map(|(_, item): (_, &Item)| item.id())
}
