// SPDX-License-Identifier: GPL-3.0-or-later
//! Collections of media (WP-2F.06a, ADR-0036): `media.create_collection`,
//! `media.rename_collection`, `media.delete_collection`, `media.add_to_collection`,
//! `media.remove_from_collection`, `media.set_collection_filter` and the
//! `media.list_collections` query.
//!
//! A collection is a named list an asset can be on without leaving its folder. A manual one
//! stores its members in the order they were added; a smart one stores a [`MediaFilter`] and
//! its members are computed when asked (`media.list` with `filter.collection`). Names are unique
//! ignoring case. No command here changes an asset, a timeline or a file on disk, and each is
//! one undo step.
//!
//! **Members that no longer exist.** The stored form may name an asset that is gone (a build
//! from before collections removed it, or a collected copy left it out). Queries skip such a
//! member and every command that edits the collection drops it. This build never leaves one
//! behind: `media.remove` and `media.delete_folder` take the removed assets out of every manual
//! collection in the same transaction ([`drop_members`]), so undo puts them back.

use std::collections::{BTreeMap, HashSet};

use cutweave_commands::{
    CmdError, Command, DocPath, Query, QueryCtx, Registry, RegistryError, Tx, command, query, wire,
};
use cutweave_model::{
    Collection, CollectionId, CollectionKind, EntityKind, MAX_COLLECTION_MEMBERS, MAX_COLLECTIONS,
    MediaFilter, MediaId, Project, collection_name_key, collection_name_problem,
};

use super::find::{Matcher, collection_members, find_collection, used_media};

/// Registers the collection commands and the `media.list_collections` query.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    registry
        .register::<MediaCreateCollection>()?
        .register::<MediaRenameCollection>()?
        .register::<MediaDeleteCollection>()?
        .register::<MediaAddToCollection>()?
        .register::<MediaRemoveFromCollection>()?
        .register::<MediaSetCollectionFilter>()?
        .register::<MediaListCollections>()?;
    Ok(())
}

fn collections_path() -> DocPath {
    DocPath::root().join("collections")
}

fn collection_path(id: CollectionId) -> DocPath {
    collections_path().join(id)
}

/// Adds a collection. `collections` is left out of the document while empty, so the first one
/// adds the map.
fn insert_collection(tx: &mut Tx<'_>, collection: &Collection) -> Result<(), CmdError> {
    if tx.doc().collections.is_empty() {
        let only: BTreeMap<CollectionId, &Collection> =
            std::iter::once((collection.id, collection)).collect();
        tx.set(&collections_path(), &only)
    } else {
        tx.set(&collection_path(collection.id), collection)
    }
}

/// Removes a collection; the last one takes the map with it.
fn delete_collection(tx: &mut Tx<'_>, id: CollectionId) -> Result<(), CmdError> {
    if tx.doc().collections.len() == 1 {
        tx.remove_path(&collections_path()).map(drop)
    } else {
        tx.remove_path(&collection_path(id)).map(drop)
    }
}

fn write_members(tx: &mut Tx<'_>, id: CollectionId, members: &[MediaId]) -> Result<(), CmdError> {
    tx.set(&collection_path(id).join("kind").join("media"), members)
}

/// Takes `removed` out of every manual collection (with any other member that names no
/// asset). For the commands that remove assets: call it in their transaction, before or after
/// the removal.
pub(super) fn drop_members(tx: &mut Tx<'_>, removed: &[MediaId]) -> Result<(), CmdError> {
    if removed.is_empty() || tx.doc().collections.is_empty() {
        return Ok(());
    }
    let gone: HashSet<MediaId> = removed.iter().copied().collect();
    let doc = tx.doc();
    let edits: Vec<(CollectionId, Vec<MediaId>)> = doc
        .collections
        .values()
        .filter_map(|c| match &c.kind {
            CollectionKind::Manual { media } if media.iter().any(|m| gone.contains(m)) => Some((
                c.id,
                media
                    .iter()
                    .copied()
                    .filter(|m| !gone.contains(m) && doc.media.contains_key(m))
                    .collect(),
            )),
            _ => None,
        })
        .collect();
    for (id, members) in edits {
        write_members(tx, id, &members)?;
    }
    Ok(())
}

/// A trimmed collection name that passes [`collection_name_problem`] and that no collection
/// other than `except` has, ignoring case.
fn check_name(doc: &Project, name: &str, except: Option<CollectionId>) -> Result<String, CmdError> {
    let name = name.trim();
    if let Some(problem) = collection_name_problem(name) {
        return Err(CmdError::invalid("/name", problem));
    }
    let key = collection_name_key(name);
    if doc
        .collections
        .values()
        .any(|c| Some(c.id) != except && collection_name_key(&c.name) == key)
    {
        return Err(CmdError::invalid(
            "/name",
            format!("there is already a collection named `{name}` (names ignore case)"),
        ));
    }
    Ok(name.to_owned())
}

/// The filter a smart collection stores: checked against `doc` (its folder exists), naming no
/// collection, its tags trimmed.
fn check_filter(
    doc: &Project,
    filter: &MediaFilter,
    pointer: &str,
) -> Result<MediaFilter, CmdError> {
    let at = |e: CmdError| match e {
        CmdError::Invalid { path, msg } => CmdError::invalid(
            format!("{pointer}{}", path.strip_prefix("/filter").unwrap_or(&path)),
            msg,
        ),
        other => other,
    };
    if filter.collection.is_some() {
        return Err(CmdError::invalid(
            format!("{pointer}/collection"),
            "the filter of a smart collection cannot name a collection",
        ));
    }
    Matcher::for_doc(doc, filter, None).map_err(at)?;
    let mut stored = filter.clone();
    if let Some(tags) = &mut stored.tags {
        for tag in tags.iter_mut() {
            *tag = tag.trim().to_owned();
        }
    }
    Ok(stored)
}

/// The members `media` of a call: at most [`MAX_COLLECTION_MEMBERS`], none listed twice.
fn check_listed(pointer: &str, media: &[MediaId]) -> Result<(), CmdError> {
    if media.len() > MAX_COLLECTION_MEMBERS {
        return Err(CmdError::invalid(
            pointer,
            format!("a collection has at most {MAX_COLLECTION_MEMBERS} members"),
        ));
    }
    let mut seen = HashSet::with_capacity(media.len());
    for (i, id) in media.iter().enumerate() {
        if !seen.insert(*id) {
            return Err(CmdError::invalid(
                format!("{pointer}/{i}"),
                format!("`{id}` is listed twice"),
            ));
        }
    }
    Ok(())
}

fn all_exist(doc: &Project, media: &[MediaId]) -> Result<(), CmdError> {
    match media.iter().find(|m| !doc.media.contains_key(*m)) {
        Some(missing) => Err(CmdError::not_found(EntityKind::Media, *missing)),
        None => Ok(()),
    }
}

/// The stored members of a manual collection, or `Invalid` for a smart one.
fn manual_members<'a>(collection: &'a Collection, verb: &str) -> Result<&'a [MediaId], CmdError> {
    match &collection.kind {
        CollectionKind::Manual { media } => Ok(media),
        CollectionKind::Smart { .. } => Err(CmdError::invalid(
            "/collection",
            format!(
                "`{}` is a smart collection: its members come from its filter, so you cannot \
                 {verb}; change the filter with `media.set_collection_filter`",
                collection.name
            ),
        )),
    }
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

// ----- media.create_collection ---------------------------------------------------------------

/// Result of `media.create_collection` and `media.rename_collection`.
#[wire]
pub struct CollectionNamed {
    /// The collection.
    pub collection_id: CollectionId,
    /// Its name (trimmed).
    pub name: String,
}

/// Creates a collection of media: a named list an asset can be on without leaving its folder.
/// `kind` is `{"type": "manual", "media": [...]}` (the members, in order; may be empty or left
/// out) or `{"type": "smart", "filter": {...}}` (a saved `media.list` filter whose matches are
/// the members, always current; it cannot name a collection). The name must be new, ignoring
/// case. A project holds at most 1,000 collections and a manual one at most 10,000 members.
#[command(
    name = "media.create_collection",
    summary = "Create a manual or smart (saved filter) collection of media",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = false)
)]
pub struct MediaCreateCollection {
    /// Its name: 1 to 255 characters without control characters (trimmed), unique ignoring
    /// case.
    pub name: String,
    /// Manual (members) or smart (a filter).
    pub kind: CollectionKind,
}

impl MediaCreateCollection {
    fn plan(&self, doc: &Project) -> Result<(String, CollectionKind), CmdError> {
        let name = check_name(doc, &self.name, None)?;
        if doc.collections.len() >= MAX_COLLECTIONS {
            return Err(CmdError::invalid(
                "/name",
                format!("a project holds at most {MAX_COLLECTIONS} collections"),
            ));
        }
        let kind = match &self.kind {
            CollectionKind::Manual { media } => {
                check_listed("/kind/media", media)?;
                all_exist(doc, media)?;
                CollectionKind::Manual {
                    media: media.clone(),
                }
            }
            CollectionKind::Smart { filter } => CollectionKind::Smart {
                filter: check_filter(doc, filter, "/kind/filter")?,
            },
        };
        Ok((name, kind))
    }
}

impl Command for MediaCreateCollection {
    type Output = CollectionNamed;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        self.plan(ctx.doc()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<CollectionNamed, CmdError> {
        let (name, kind) = self.plan(tx.doc())?;
        let collection = Collection::new(tx.new_id(), name, kind);
        insert_collection(tx, &collection)?;
        Ok(CollectionNamed {
            collection_id: collection.id,
            name: collection.name,
        })
    }
}

// ----- media.rename_collection ---------------------------------------------------------------

/// Renames a collection. Fails with `Invalid` when another collection has the name, ignoring
/// case (changing only the case of its own name is fine).
#[command(
    name = "media.rename_collection",
    summary = "Rename a collection of media",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct MediaRenameCollection {
    /// The collection.
    pub collection: CollectionId,
    /// The new name: 1 to 255 characters without control characters (trimmed).
    pub name: String,
}

impl Command for MediaRenameCollection {
    type Output = CollectionNamed;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        find_collection(ctx.doc(), self.collection)?;
        check_name(ctx.doc(), &self.name, Some(self.collection)).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<CollectionNamed, CmdError> {
        let collection = find_collection(tx.doc(), self.collection)?;
        let name = check_name(tx.doc(), &self.name, Some(self.collection))?;
        // An edit of the collection: members that name no asset go.
        let stale = match &collection.kind {
            CollectionKind::Manual { media } => {
                let kept = collection_members(tx.doc(), collection, None);
                (kept.len() != media.len()).then_some(kept)
            }
            CollectionKind::Smart { .. } => None,
        };
        tx.set(&collection_path(self.collection).join("name"), &name)?;
        if let Some(kept) = stale {
            write_members(tx, self.collection, &kept)?;
        }
        Ok(CollectionNamed {
            collection_id: self.collection,
            name,
        })
    }
}

// ----- media.delete_collection ---------------------------------------------------------------

/// Result of `media.delete_collection`.
#[wire]
#[derive(Copy)]
pub struct CollectionDeleted {
    /// The deleted collection.
    pub collection_id: CollectionId,
}

/// Deletes a collection. Its assets stay in the project, in their folders and on their
/// timelines; only the list goes.
#[command(
    name = "media.delete_collection",
    summary = "Delete a collection of media (the assets stay)",
    undoable,
    mcp = "catalog",
    annotations(destructive = true, idempotent = false)
)]
pub struct MediaDeleteCollection {
    /// The collection.
    pub collection: CollectionId,
}

impl Command for MediaDeleteCollection {
    type Output = CollectionDeleted;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        find_collection(ctx.doc(), self.collection).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<CollectionDeleted, CmdError> {
        find_collection(tx.doc(), self.collection)?;
        delete_collection(tx, self.collection)?;
        Ok(CollectionDeleted {
            collection_id: self.collection,
        })
    }
}

// ----- media.add_to_collection ---------------------------------------------------------------

/// Result of `media.add_to_collection`.
#[wire]
pub struct MediaAddedToCollection {
    /// The collection.
    pub collection_id: CollectionId,
    /// The assets that joined it, in order (those already on it are left out).
    pub added: Vec<MediaId>,
    /// How many members it has now.
    pub members: u32,
}

/// Adds assets to the end of a manual collection, in the order given. An asset already on it
/// stays where it is, so the call can be repeated. A collection holds at most 10,000 members.
/// Fails with `Invalid` for a smart collection, whose members come from its filter.
#[command(
    name = "media.add_to_collection",
    summary = "Add media assets to a manual collection (idempotent)",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct MediaAddToCollection {
    /// The collection (manual).
    pub collection: CollectionId,
    /// The assets to add (at least one, no repeats).
    pub media: Vec<MediaId>,
}

/// What a membership edit does: the new stored members, the ones that came or went, and
/// whether anything is written.
struct MemberPlan {
    members: Vec<MediaId>,
    delta: Vec<MediaId>,
    write: bool,
}

impl MediaAddToCollection {
    fn plan(&self, doc: &Project) -> Result<MemberPlan, CmdError> {
        let collection = find_collection(doc, self.collection)?;
        let stored = manual_members(collection, "add assets to it")?;
        if self.media.is_empty() {
            return Err(CmdError::invalid("/media", "give at least one asset"));
        }
        check_listed("/media", &self.media)?;
        all_exist(doc, &self.media)?;
        let mut members = collection_members(doc, collection, None);
        let on: HashSet<MediaId> = members.iter().copied().collect();
        let added: Vec<MediaId> = self
            .media
            .iter()
            .copied()
            .filter(|m| !on.contains(m))
            .collect();
        members.extend(&added);
        if members.len() > MAX_COLLECTION_MEMBERS {
            return Err(CmdError::invalid(
                "/media",
                format!(
                    "`{}` would have {} members; a collection holds at most \
                     {MAX_COLLECTION_MEMBERS}",
                    collection.name,
                    members.len()
                ),
            ));
        }
        Ok(MemberPlan {
            write: members.as_slice() != stored,
            members,
            delta: added,
        })
    }
}

impl Command for MediaAddToCollection {
    type Output = MediaAddedToCollection;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        self.plan(ctx.doc()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<MediaAddedToCollection, CmdError> {
        let plan = self.plan(tx.doc())?;
        if plan.write {
            write_members(tx, self.collection, &plan.members)?;
        }
        Ok(MediaAddedToCollection {
            collection_id: self.collection,
            added: plan.delta,
            members: count(plan.members.len()),
        })
    }
}

// ----- media.remove_from_collection ----------------------------------------------------------

/// Result of `media.remove_from_collection`.
#[wire]
pub struct MediaRemovedFromCollection {
    /// The collection.
    pub collection_id: CollectionId,
    /// The assets that left it (those that were not on it are left out).
    pub removed: Vec<MediaId>,
    /// How many members it has now.
    pub members: u32,
}

/// Takes assets off a manual collection; the assets themselves stay in the project. An asset
/// that is not on it is ignored, so the call can be repeated. Fails with `Invalid` for a smart
/// collection, whose members come from its filter.
#[command(
    name = "media.remove_from_collection",
    summary = "Take media assets off a manual collection (the assets stay; idempotent)",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct MediaRemoveFromCollection {
    /// The collection (manual).
    pub collection: CollectionId,
    /// The assets to take off (at least one, no repeats).
    pub media: Vec<MediaId>,
}

impl MediaRemoveFromCollection {
    fn plan(&self, doc: &Project) -> Result<MemberPlan, CmdError> {
        let collection = find_collection(doc, self.collection)?;
        let stored = manual_members(collection, "take assets off it")?;
        if self.media.is_empty() {
            return Err(CmdError::invalid("/media", "give at least one asset"));
        }
        check_listed("/media", &self.media)?;
        let gone: HashSet<MediaId> = self.media.iter().copied().collect();
        let before = collection_members(doc, collection, None);
        let removed: Vec<MediaId> = before
            .iter()
            .copied()
            .filter(|m| gone.contains(m))
            .collect();
        let members: Vec<MediaId> = before.into_iter().filter(|m| !gone.contains(m)).collect();
        Ok(MemberPlan {
            write: members.as_slice() != stored,
            members,
            delta: removed,
        })
    }
}

impl Command for MediaRemoveFromCollection {
    type Output = MediaRemovedFromCollection;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        self.plan(ctx.doc()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<MediaRemovedFromCollection, CmdError> {
        let plan = self.plan(tx.doc())?;
        if plan.write {
            write_members(tx, self.collection, &plan.members)?;
        }
        Ok(MediaRemovedFromCollection {
            collection_id: self.collection,
            removed: plan.delta,
            members: count(plan.members.len()),
        })
    }
}

// ----- media.set_collection_filter -----------------------------------------------------------

/// Result of `media.set_collection_filter`.
#[wire]
#[derive(Copy)]
pub struct CollectionFilterSet {
    /// The collection.
    pub collection_id: CollectionId,
    /// How many assets pass the new filter now.
    pub members: u32,
}

/// Replaces the filter of a smart collection (the same filter `media.list` takes; it cannot
/// name a collection). Fails with `Invalid` for a manual collection.
#[command(
    name = "media.set_collection_filter",
    summary = "Replace the filter of a smart collection of media",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct MediaSetCollectionFilter {
    /// The collection (smart).
    pub collection: CollectionId,
    /// The new filter.
    pub filter: MediaFilter,
}

impl MediaSetCollectionFilter {
    fn plan(&self, doc: &Project) -> Result<MediaFilter, CmdError> {
        let collection = find_collection(doc, self.collection)?;
        if matches!(collection.kind, CollectionKind::Manual { .. }) {
            return Err(CmdError::invalid(
                "/collection",
                format!(
                    "`{}` is a manual collection and has no filter; change its members with \
                     `media.add_to_collection` and `media.remove_from_collection`",
                    collection.name
                ),
            ));
        }
        check_filter(doc, &self.filter, "/filter")
    }
}

impl Command for MediaSetCollectionFilter {
    type Output = CollectionFilterSet;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        self.plan(ctx.doc()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<CollectionFilterSet, CmdError> {
        let filter = self.plan(tx.doc())?;
        tx.set(
            &collection_path(self.collection).join("kind").join("filter"),
            &filter,
        )?;
        let doc = tx.doc();
        let used = filter.used.is_some().then(|| used_media(doc));
        let members = find_collection(doc, self.collection)
            .map(|c| collection_members(doc, c, used.as_ref()).len())?;
        Ok(CollectionFilterSet {
            collection_id: self.collection,
            members: count(members),
        })
    }
}

// ----- media.list_collections ----------------------------------------------------------------

/// Whether a collection stores its members or a filter.
#[wire]
#[derive(Copy, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum CollectionType {
    /// Stored members.
    Manual,
    /// A stored filter; the members are computed.
    Smart,
}

/// One collection of media.
#[wire]
pub struct CollectionSummary {
    /// The collection.
    pub id: CollectionId,
    /// Its name.
    pub name: String,
    /// Manual or smart.
    pub kind: CollectionType,
    /// The filter of a smart collection; absent for a manual one.
    pub filter: Option<MediaFilter>,
    /// How many assets are on it now (a smart collection's filter is run; members that no
    /// longer exist are not counted).
    pub media_count: u32,
}

/// Result of `media.list_collections`.
#[wire]
pub struct MediaListCollectionsOutput {
    /// Every collection, by name ignoring case.
    pub collections: Vec<CollectionSummary>,
}

/// Lists the project's collections of media: each one's ID, name, kind (manual or smart), the
/// filter of a smart one and how many assets are on it. List the members with `media.list` and
/// `filter.collection`.
#[query(
    name = "media.list_collections",
    summary = "List the project's media collections with kind and member count",
    mcp = "catalog"
)]
pub struct MediaListCollections {}

impl Query for MediaListCollections {
    type Output = MediaListCollectionsOutput;

    fn run(&self, ctx: &QueryCtx<'_>) -> Result<MediaListCollectionsOutput, CmdError> {
        let doc = ctx.doc();
        let needs_used = doc
            .collections
            .values()
            .any(|c| matches!(&c.kind, CollectionKind::Smart { filter } if filter.used.is_some()));
        let used = needs_used.then(|| used_media(doc));
        let mut collections: Vec<CollectionSummary> = doc
            .collections
            .values()
            .map(|c| CollectionSummary {
                id: c.id,
                name: c.name.clone(),
                kind: match c.kind {
                    CollectionKind::Manual { .. } => CollectionType::Manual,
                    CollectionKind::Smart { .. } => CollectionType::Smart,
                },
                filter: match &c.kind {
                    CollectionKind::Manual { .. } => None,
                    CollectionKind::Smart { filter } => Some(filter.clone()),
                },
                media_count: count(collection_members(doc, c, used.as_ref()).len()),
            })
            .collect();
        collections.sort_by_cached_key(|c| (collection_name_key(&c.name), c.name.clone(), c.id));
        Ok(MediaListCollectionsOutput { collections })
    }
}
