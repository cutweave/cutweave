// SPDX-License-Identifier: GPL-3.0-or-later
//! Media folders (WP-2F.07, ADR-0020): `media.create_folder`, `media.rename_folder`,
//! `media.rename`, `media.move`, `media.delete_folder` and the `media.list_folders` query.
//!
//! Every asset is in one folder or at the top level, and folders nest up to
//! [`MAX_FOLDER_DEPTH`] levels. Sibling folder names are unique ignoring case, so a create or
//! a move into a folder that already has the name either fails (`on_conflict: fail`, the
//! default) or picks the next free name (`rename`: "Day 1" becomes "Day 1 2").
//! `media.delete_folder` never changes a timeline and never touches a file on disk: `move_up`
//! lifts the folder's contents one level, `remove` removes the folder, its subfolders and every
//! asset no timeline uses, and moves the used ones to the deleted folder's parent. Each
//! command is one undo step.

use std::{
    collections::{BTreeMap, HashSet},
    fmt::Display,
    hash::{BuildHasher, Hash},
};

use cutweave_commands::{
    CmdError, Command, DocPath, Query, QueryCtx, Registry, RegistryError, Tx, command, query, wire,
};
use cutweave_model::{
    EntityKind, FolderId, MAX_FOLDER_DEPTH, MAX_FOLDER_NAME_CHARS, MediaFolder, MediaId, Project,
    folder_name_key, folder_name_problem, media_name_problem,
};

use super::{asset_name, used_media};

/// The name of a folder created without one ("New folder", then "New folder 2", …).
pub const DEFAULT_FOLDER_NAME: &str = "New folder";

/// Most assets plus folders one `media.move` accepts.
pub const MAX_MOVE_ENTRIES: usize = 10_000;

/// Registers the folder commands and the `media.list_folders` query.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    registry
        .register::<MediaCreateFolder>()?
        .register::<MediaRenameFolder>()?
        .register::<MediaRename>()?
        .register::<MediaMove>()?
        .register::<MediaDeleteFolder>()?
        .register::<MediaListFolders>()?;
    Ok(())
}

/// What to do when a folder's name is already taken where it goes.
#[wire]
#[derive(Copy, Default, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum OnConflict {
    /// Fail with `Invalid` (the default).
    #[default]
    Fail,
    /// Use the next free name: "Day 1 2", "Day 1 3", …
    Rename,
}

/// A folder that got a new name because its name was taken where it went.
#[wire]
pub struct FolderRenamed {
    /// The folder.
    pub folder: FolderId,
    /// Its name before.
    pub from: String,
    /// Its name now.
    pub to: String,
}

/// The folder, or `NotFound`.
pub fn find_folder(doc: &Project, id: FolderId) -> Result<&MediaFolder, CmdError> {
    doc.folders
        .get(&id)
        .ok_or_else(|| CmdError::not_found(EntityKind::Folder, id))
}

fn folder_doc_path(id: FolderId) -> DocPath {
    DocPath::root().join("folders").join(id)
}

/// A trimmed folder name that passes [`folder_name_problem`].
fn check_folder_name(pointer: &str, name: &str) -> Result<String, CmdError> {
    let name = name.trim();
    match folder_name_problem(name) {
        Some(problem) => Err(CmdError::invalid(pointer, problem)),
        None => Ok(name.to_owned()),
    }
}

/// A trimmed asset display name that passes [`media_name_problem`].
fn check_media_name(pointer: &str, name: &str) -> Result<String, CmdError> {
    let name = name.trim();
    match media_name_problem(name) {
        Some(problem) => Err(CmdError::invalid(pointer, problem)),
        None => Ok(name.to_owned()),
    }
}

/// `base` if no key in `taken` matches it ignoring case, else `"base 2"`, `"base 3"`, … (the
/// base shortened so the name stays within [`MAX_FOLDER_NAME_CHARS`] characters).
#[must_use]
pub fn unique_folder_name<S: BuildHasher>(base: &str, taken: &HashSet<String, S>) -> String {
    if !taken.contains(&folder_name_key(base)) {
        return base.to_owned();
    }
    let mut n = 2_u64;
    loop {
        let suffix = format!(" {n}");
        let room = MAX_FOLDER_NAME_CHARS.saturating_sub(suffix.chars().count());
        let stem: String = base.chars().take(room).collect();
        let candidate = format!("{}{suffix}", stem.trim_end());
        if !taken.contains(&folder_name_key(&candidate)) {
            return candidate;
        }
        n += 1;
    }
}

/// The name keys of the folders in `parent` (`None`: the top level), leaving out `except`.
fn sibling_keys(doc: &Project, parent: Option<FolderId>, except: &[FolderId]) -> HashSet<String> {
    doc.folders
        .values()
        .filter(|f| f.parent == parent && !except.contains(&f.id))
        .map(|f| folder_name_key(&f.name))
        .collect()
}

/// "folder `B-roll/Drone`" or "the top level", for messages.
fn place(doc: &Project, folder: Option<FolderId>) -> String {
    match folder {
        None => "the top level".to_owned(),
        Some(f) => format!(
            "folder `{}`",
            doc.folder_path(f).unwrap_or_else(|| f.to_string())
        ),
    }
}

/// How deep `folder` is (0 for the top level).
pub(super) fn depth_of(doc: &Project, folder: Option<FolderId>) -> usize {
    folder.map_or(0, |f| doc.folder_depth(f).unwrap_or(MAX_FOLDER_DEPTH))
}

/// Picks the name of a folder arriving in a place whose names are `taken`.
fn arriving_name(
    pointer: &str,
    doc: &Project,
    name: &str,
    to: Option<FolderId>,
    taken: &HashSet<String>,
    on_conflict: OnConflict,
) -> Result<String, CmdError> {
    if !taken.contains(&folder_name_key(name)) {
        return Ok(name.to_owned());
    }
    match on_conflict {
        OnConflict::Rename => Ok(unique_folder_name(name, taken)),
        OnConflict::Fail => Err(CmdError::invalid(
            pointer,
            format!(
                "{} already has a folder named `{name}` (names ignore case); pick another name or \
                 pass `on_conflict: \"rename\"`",
                place(doc, to)
            ),
        )),
    }
}

// ----- media.create_folder -------------------------------------------------------------------

/// Result of `media.create_folder`.
#[wire]
pub struct MediaFolderCreated {
    /// The new folder.
    pub folder_id: FolderId,
    /// Its name (the next free one with `on_conflict: rename` or without a name).
    pub name: String,
    /// The folder it is in; absent at the top level.
    pub parent: Option<FolderId>,
}

/// Creates a folder in the media bin, at the top level or inside `parent` (at most 32 levels
/// deep). Without a name it is "New folder" (or "New folder 2", … when taken). A name that a
/// sibling already has, ignoring case, fails with `Invalid` unless `on_conflict` is `rename`.
#[command(
    name = "media.create_folder",
    summary = "Create a folder in the media bin (top level or inside another folder)",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = false)
)]
pub struct MediaCreateFolder {
    /// The folder to create it in; absent: the top level.
    pub parent: Option<FolderId>,
    /// Its name: 1 to 255 characters, no `/` or control characters (trimmed). Default "New
    /// folder".
    pub name: Option<String>,
    /// When a sibling already has the name: `fail` (default) or `rename` ("Day 1 2").
    pub on_conflict: Option<OnConflict>,
}

impl MediaCreateFolder {
    fn plan(&self, doc: &Project) -> Result<String, CmdError> {
        if let Some(parent) = self.parent {
            find_folder(doc, parent)?;
            if depth_of(doc, Some(parent)) >= MAX_FOLDER_DEPTH {
                return Err(CmdError::invalid(
                    "/parent",
                    format!(
                        "{} is {MAX_FOLDER_DEPTH} levels deep; folders nest at most \
                         {MAX_FOLDER_DEPTH} levels",
                        place(doc, Some(parent))
                    ),
                ));
            }
        }
        let taken = sibling_keys(doc, self.parent, &[]);
        match &self.name {
            None => Ok(unique_folder_name(DEFAULT_FOLDER_NAME, &taken)),
            Some(name) => {
                let name = check_folder_name("/name", name)?;
                arriving_name(
                    "/name",
                    doc,
                    &name,
                    self.parent,
                    &taken,
                    self.on_conflict.unwrap_or_default(),
                )
            }
        }
    }
}

impl Command for MediaCreateFolder {
    type Output = MediaFolderCreated;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        self.plan(ctx.doc()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<MediaFolderCreated, CmdError> {
        let name = self.plan(tx.doc())?;
        let folder = MediaFolder::new(tx.new_id(), name, self.parent);
        tx.set(&folder_doc_path(folder.id), &folder)?;
        Ok(MediaFolderCreated {
            folder_id: folder.id,
            name: folder.name,
            parent: folder.parent,
        })
    }
}

// ----- media.rename_folder -------------------------------------------------------------------

/// Result of `media.rename_folder`.
#[wire]
pub struct MediaFolderRenamed {
    /// The folder.
    pub folder_id: FolderId,
    /// Its name now.
    pub name: String,
}

/// Renames a folder of the media bin. Fails with `Invalid` when a sibling already has the
/// name, ignoring case (changing only the case of the folder's own name is fine).
#[command(
    name = "media.rename_folder",
    summary = "Rename a folder of the media bin",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct MediaRenameFolder {
    /// The folder.
    pub folder: FolderId,
    /// The new name: 1 to 255 characters, no `/` or control characters (trimmed).
    pub name: String,
}

impl MediaRenameFolder {
    fn plan(&self, doc: &Project) -> Result<String, CmdError> {
        let folder = find_folder(doc, self.folder)?;
        let name = check_folder_name("/name", &self.name)?;
        let taken = sibling_keys(doc, folder.parent, &[folder.id]);
        arriving_name("/name", doc, &name, folder.parent, &taken, OnConflict::Fail)
    }
}

impl Command for MediaRenameFolder {
    type Output = MediaFolderRenamed;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        self.plan(ctx.doc()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<MediaFolderRenamed, CmdError> {
        let name = self.plan(tx.doc())?;
        tx.set(&folder_doc_path(self.folder).join("name"), &name)?;
        Ok(MediaFolderRenamed {
            folder_id: self.folder,
            name,
        })
    }
}

// ----- media.rename --------------------------------------------------------------------------

/// Result of `media.rename`.
#[wire]
pub struct MediaRenamed {
    /// The asset.
    pub media_id: MediaId,
    /// Its display name; absent: the bin shows the file name.
    pub name: Option<String>,
    /// The name the bin shows now.
    pub display_name: String,
}

/// Sets the name the media bin shows for an asset (the file on disk keeps its name). Without
/// a name the bin shows the file name again. Display names need not be unique.
#[command(
    name = "media.rename",
    summary = "Set the name the media bin shows for an asset, or go back to the file name",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct MediaRename {
    /// The asset.
    pub media: MediaId,
    /// The display name: 1 to 255 characters without control characters (trimmed); absent:
    /// the file name.
    pub name: Option<String>,
}

impl MediaRename {
    fn plan(&self, ctx: &QueryCtx<'_>) -> Result<Option<String>, CmdError> {
        ctx.media(self.media)?;
        self.name
            .as_deref()
            .map(|n| check_media_name("/name", n))
            .transpose()
    }
}

impl Command for MediaRename {
    type Output = MediaRenamed;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        self.plan(ctx).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<MediaRenamed, CmdError> {
        let name = self.plan(&tx.query_ctx())?;
        tx.set(&DocPath::media(self.media).join("name"), &name)?;
        let media = tx.media(self.media)?;
        Ok(MediaRenamed {
            media_id: self.media,
            name,
            display_name: asset_name(media).into_owned(),
        })
    }
}

// ----- media.move ----------------------------------------------------------------------------

/// Result of `media.move`.
#[wire]
pub struct MediaMoved {
    /// Where everything went; absent: the top level.
    pub to: Option<FolderId>,
    /// The assets that changed folder (those already there are left out).
    pub media: Vec<MediaId>,
    /// The folders that changed parent (those already there are left out).
    pub folders: Vec<FolderId>,
    /// Folders renamed because their name was taken (`on_conflict: rename`).
    pub renamed: Vec<FolderRenamed>,
}

/// Moves assets and folders (with everything inside them) into a folder, or to the top level
/// without `to`. Moving a folder into itself or one of its subfolders, or so deep that
/// something would be nested more than 32 levels, fails with `Invalid`. A moved folder whose
/// name the destination already has, ignoring case, fails with `Invalid` unless `on_conflict`
/// is `rename`. Timelines and files on disk never change.
#[command(
    name = "media.move",
    summary = "Move media assets and folders into a folder of the bin, or to its top level",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct MediaMove {
    /// Assets to move.
    pub media: Option<Vec<MediaId>>,
    /// Folders to move, with their contents.
    pub folders: Option<Vec<FolderId>>,
    /// The destination folder; absent: the top level.
    pub to: Option<FolderId>,
    /// When a moved folder's name is taken in `to`: `fail` (default) or `rename`.
    pub on_conflict: Option<OnConflict>,
}

/// What a valid `media.move` changes.
struct MovePlan {
    media: Vec<MediaId>,
    /// `(folder, new name if renamed)`.
    folders: Vec<(FolderId, Option<String>)>,
    renamed: Vec<FolderRenamed>,
}

fn no_repeats<T: Eq + Hash + Copy + Display>(pointer: &str, ids: &[T]) -> Result<(), CmdError> {
    let mut seen = HashSet::with_capacity(ids.len());
    for (i, id) in ids.iter().enumerate() {
        if !seen.insert(*id) {
            return Err(CmdError::invalid(
                format!("{pointer}/{i}"),
                format!("`{id}` is listed twice"),
            ));
        }
    }
    Ok(())
}

impl MediaMove {
    fn plan(&self, doc: &Project) -> Result<MovePlan, CmdError> {
        let media = self.media.as_deref().unwrap_or_default();
        let folders = self.folders.as_deref().unwrap_or_default();
        if media.is_empty() && folders.is_empty() {
            return Err(CmdError::invalid(
                "/media",
                "give at least one asset in `media` or one folder in `folders`",
            ));
        }
        if media.len() + folders.len() > MAX_MOVE_ENTRIES {
            return Err(CmdError::invalid(
                "/media",
                format!("move at most {MAX_MOVE_ENTRIES} assets and folders at once"),
            ));
        }
        no_repeats("/media", media)?;
        no_repeats("/folders", folders)?;
        if let Some(to) = self.to {
            find_folder(doc, to)?;
        }
        for id in media {
            doc.media
                .get(id)
                .ok_or_else(|| CmdError::not_found(EntityKind::Media, *id))?;
        }
        for (i, id) in folders.iter().enumerate() {
            let folder = find_folder(doc, *id)?;
            if let Some(to) = self.to
                && doc.folder_is_within(to, *id)
            {
                return Err(CmdError::invalid(
                    format!("/folders/{i}"),
                    format!(
                        "folder `{}` cannot move into itself or one of its subfolders",
                        folder.name
                    ),
                ));
            }
        }

        // Names: the moved folders join the folders already in `to`.
        let on_conflict = self.on_conflict.unwrap_or_default();
        let mut taken = sibling_keys(doc, self.to, &[]);
        let mut moved = Vec::new();
        let mut renamed = Vec::new();
        let mut after = doc.clone();
        for (i, id) in folders.iter().enumerate() {
            let folder = find_folder(doc, *id)?;
            if folder.parent == self.to {
                continue;
            }
            let pointer = format!("/folders/{i}");
            let name = arriving_name(&pointer, doc, &folder.name, self.to, &taken, on_conflict)?;
            taken.insert(folder_name_key(&name));
            let new_name = (name != folder.name).then(|| name.clone());
            if let Some(to) = &new_name {
                renamed.push(FolderRenamed {
                    folder: *id,
                    from: folder.name.clone(),
                    to: to.clone(),
                });
            }
            if let Some(f) = after.folders.get_mut(id) {
                f.parent = self.to;
            }
            moved.push((*id, new_name));
        }

        // Depth: nothing may end up deeper than the limit. Nothing was before, so a folder that
        // is too deep now is inside a moved one; the error points at that one.
        if !moved.is_empty()
            && let Some((deep, depth)) = after
                .folders
                .keys()
                .map(|f| (*f, depth_of(&after, Some(*f))))
                .find(|(_, d)| *d > MAX_FOLDER_DEPTH)
        {
            let chain = after.folder_ancestry(deep);
            let i = folders
                .iter()
                .position(|f| chain.contains(f))
                .unwrap_or_default();
            return Err(CmdError::invalid(
                format!("/folders/{i}"),
                format!(
                    "moving folder `{}` to {} would nest folders {depth} levels deep; at most \
                     {MAX_FOLDER_DEPTH} are allowed",
                    doc.folders.get(&folders[i]).map_or("", |f| f.name.as_str()),
                    place(doc, self.to)
                ),
            ));
        }

        let media = media
            .iter()
            .copied()
            .filter(|m| doc.media.get(m).is_some_and(|a| a.folder != self.to))
            .collect();
        Ok(MovePlan {
            media,
            folders: moved,
            renamed,
        })
    }
}

impl Command for MediaMove {
    type Output = MediaMoved;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        self.plan(ctx.doc()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<MediaMoved, CmdError> {
        let plan = self.plan(tx.doc())?;
        for (id, name) in &plan.folders {
            let path = folder_doc_path(*id);
            if let Some(name) = name {
                tx.set(&path.join("name"), name)?;
            }
            tx.set(&path.join("parent"), self.to)?;
        }
        for id in &plan.media {
            tx.set(&DocPath::media(*id).join("folder"), self.to)?;
        }
        Ok(MediaMoved {
            to: self.to,
            media: plan.media,
            folders: plan.folders.into_iter().map(|(id, _)| id).collect(),
            renamed: plan.renamed,
        })
    }
}

// ----- media.delete_folder -------------------------------------------------------------------

/// What `media.delete_folder` does with a folder's contents.
#[wire]
#[derive(Copy, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum FolderContents {
    /// Lift the folder's assets and subfolders one level, into its parent.
    MoveUp,
    /// Remove its subfolders and every asset inside that no timeline uses; the used ones move
    /// to the deleted folder's parent.
    Remove,
}

/// Result of `media.delete_folder`.
#[wire]
pub struct MediaFolderDeleted {
    /// The deleted folder.
    pub folder_id: FolderId,
    /// Where its kept contents went; absent: the top level.
    pub parent: Option<FolderId>,
    /// Every folder deleted: the folder, and with `remove` its subfolders.
    pub removed_folders: Vec<FolderId>,
    /// Assets removed from the project (`remove` only; the files stay on disk).
    pub removed: Vec<MediaId>,
    /// Assets that were inside and are now in `parent`: with `remove` the ones a timeline uses,
    /// with `move_up` every asset the folder held.
    pub kept: Vec<MediaId>,
    /// Subfolders now in `parent` (`move_up` only).
    pub moved_folders: Vec<FolderId>,
    /// Subfolders renamed because `parent` already had their name.
    pub renamed: Vec<FolderRenamed>,
}

/// Deletes a folder of the media bin. `contents: move_up` lifts its assets and subfolders one
/// level (a subfolder whose name the parent already has gets the next free name);
/// `contents: remove` also removes its subfolders and every asset inside that no timeline
/// uses (taking them off every manual collection), and moves the used assets to the deleted
/// folder's parent. Timelines and files on disk never change; one undo restores everything.
#[command(
    name = "media.delete_folder",
    summary = "Delete a media bin folder: lift its contents one level, or remove unused media in it",
    undoable,
    mcp = "catalog",
    annotations(destructive = true, idempotent = false)
)]
pub struct MediaDeleteFolder {
    /// The folder.
    pub folder: FolderId,
    /// `move_up` (lift the contents one level) or `remove` (remove the unused media in it).
    pub contents: FolderContents,
}

impl MediaDeleteFolder {
    fn plan(&self, doc: &Project) -> Result<MediaFolderDeleted, CmdError> {
        let folder = find_folder(doc, self.folder)?;
        let parent = folder.parent;
        let mut out = MediaFolderDeleted {
            folder_id: folder.id,
            parent,
            removed_folders: Vec::new(),
            removed: Vec::new(),
            kept: Vec::new(),
            moved_folders: Vec::new(),
            renamed: Vec::new(),
        };
        match self.contents {
            FolderContents::MoveUp => {
                out.removed_folders.push(folder.id);
                let mut taken = sibling_keys(doc, parent, &[folder.id]);
                for sub in doc.folders.values().filter(|f| f.parent == Some(folder.id)) {
                    let name = unique_folder_name(&sub.name, &taken);
                    taken.insert(folder_name_key(&name));
                    if name != sub.name {
                        out.renamed.push(FolderRenamed {
                            folder: sub.id,
                            from: sub.name.clone(),
                            to: name,
                        });
                    }
                    out.moved_folders.push(sub.id);
                }
                out.kept = doc
                    .media
                    .values()
                    .filter(|m| m.folder == Some(folder.id))
                    .map(|m| m.id)
                    .collect();
            }
            FolderContents::Remove => {
                out.removed_folders = doc.folder_subtree(folder.id);
                let inside: HashSet<FolderId> = out.removed_folders.iter().copied().collect();
                let used = used_media(doc);
                for media in doc.media.values() {
                    if media.folder.is_some_and(|f| inside.contains(&f)) {
                        if used.contains(&media.id) {
                            out.kept.push(media.id);
                        } else {
                            out.removed.push(media.id);
                        }
                    }
                }
            }
        }
        Ok(out)
    }
}

impl Command for MediaDeleteFolder {
    type Output = MediaFolderDeleted;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        self.plan(ctx.doc()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<MediaFolderDeleted, CmdError> {
        let plan = self.plan(tx.doc())?;
        let renames: BTreeMap<FolderId, &str> = plan
            .renamed
            .iter()
            .map(|r| (r.folder, r.to.as_str()))
            .collect();
        for sub in &plan.moved_folders {
            let path = folder_doc_path(*sub);
            if let Some(name) = renames.get(sub) {
                tx.set(&path.join("name"), name)?;
            }
            tx.set(&path.join("parent"), plan.parent)?;
        }
        for media in &plan.kept {
            tx.set(&DocPath::media(*media).join("folder"), plan.parent)?;
        }
        // ADR-0036: a removed asset leaves every manual collection in the same step.
        super::collections::drop_members(tx, &plan.removed)?;
        for media in &plan.removed {
            tx.remove_media(*media)?;
        }
        // Children first, so the folder tree stays a tree after every step.
        for folder in plan.removed_folders.iter().rev() {
            tx.remove_path(&folder_doc_path(*folder))?;
        }
        Ok(plan)
    }
}

// ----- media.list_folders --------------------------------------------------------------------

/// One folder of the media bin.
#[wire]
pub struct FolderSummary {
    /// The folder.
    pub id: FolderId,
    /// Its name.
    pub name: String,
    /// The folder it is in; absent at the top level.
    pub parent: Option<FolderId>,
    /// Names from the top level down, joined with `/` (`B-roll/Drone`).
    pub path: String,
    /// Nesting depth: 1 at the top level.
    pub depth: u32,
    /// Assets directly inside.
    pub media_count: u32,
    /// Folders directly inside.
    pub folder_count: u32,
}

/// Result of `media.list_folders`.
#[wire]
pub struct MediaListFoldersOutput {
    /// Every folder, depth first: parents before their subfolders, siblings by name ignoring
    /// case.
    pub folders: Vec<FolderSummary>,
    /// Assets at the top level of the bin (in no folder).
    pub top_level_media: u32,
    /// Assets in the project.
    pub total_media: u32,
}

/// Lists the media bin's folder tree: each folder's ID, name, parent, path and how many
/// assets and subfolders it holds directly.
#[query(
    name = "media.list_folders",
    summary = "List the media bin's folder tree with paths and counts",
    mcp = "catalog"
)]
pub struct MediaListFolders {}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

impl Query for MediaListFolders {
    type Output = MediaListFoldersOutput;

    fn run(&self, ctx: &QueryCtx<'_>) -> Result<MediaListFoldersOutput, CmdError> {
        let doc = ctx.doc();
        let mut media_in: BTreeMap<Option<FolderId>, usize> = BTreeMap::new();
        for media in doc.media.values() {
            *media_in.entry(media.folder).or_default() += 1;
        }
        let mut children = doc.folder_children();
        for kids in children.values_mut() {
            kids.sort_by_cached_key(|id| {
                doc.folders
                    .get(id)
                    .map(|f| (folder_name_key(&f.name), f.name.clone(), *id))
            });
        }
        let mut folders = Vec::with_capacity(doc.folders.len());
        let mut stack: Vec<(FolderId, String, u32)> =
            children.get(&None).map_or_else(Vec::new, |top| {
                top.iter().rev().map(|id| (*id, String::new(), 1)).collect()
            });
        while let Some((id, prefix, depth)) = stack.pop() {
            let Some(folder) = doc.folders.get(&id) else {
                continue;
            };
            let path = if prefix.is_empty() {
                folder.name.clone()
            } else {
                format!("{prefix}/{}", folder.name)
            };
            let kids = children.get(&Some(id)).map_or(&[][..], Vec::as_slice);
            stack.extend(kids.iter().rev().map(|k| (*k, path.clone(), depth + 1)));
            folders.push(FolderSummary {
                id,
                name: folder.name.clone(),
                parent: folder.parent,
                path,
                depth,
                media_count: count(media_in.get(&Some(id)).copied().unwrap_or(0)),
                folder_count: count(kids.len()),
            });
        }
        Ok(MediaListFoldersOutput {
            folders,
            top_level_media: count(media_in.get(&None).copied().unwrap_or(0)),
            total_media: count(doc.media.len()),
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn keys(names: &[&str]) -> HashSet<String> {
        names.iter().map(|n| folder_name_key(n)).collect()
    }

    #[test]
    fn unique_names_count_up_and_stay_within_the_limit() {
        assert_eq!(unique_folder_name("Day 1", &keys(&[])), "Day 1");
        assert_eq!(unique_folder_name("Day 1", &keys(&["day 1"])), "Day 1 2");
        assert_eq!(
            unique_folder_name("Day 1", &keys(&["DAY 1", "Day 1 2", "day 1 3"])),
            "Day 1 4"
        );
        let long = "x".repeat(MAX_FOLDER_NAME_CHARS);
        let next = unique_folder_name(&long, &keys(&[&long]));
        assert_eq!(next.chars().count(), MAX_FOLDER_NAME_CHARS);
        assert!(next.ends_with("x 2"));
        assert_eq!(folder_name_problem(&next), None);
    }
}
