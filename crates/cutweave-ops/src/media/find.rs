// SPDX-License-Identifier: GPL-3.0-or-later
//! Finding media (WP-1M.12): the filter and sort of `media.list`. The editor's bin applies the
//! same predicate in TypeScript (`packages/editor/src/media/find.ts`); both are checked against
//! `tests/fixtures/media-find.json`, which `tests/media_find_fixture.rs` generates from this code.
//!
//! Text matching: a term is looked for in the name, the tags and the note. The name is the
//! asset's display name (ADR-0020), else its file name (relative path, else absolute, else the
//! ID); terms are split on space, tab, CR and LF; every term must occur, ignoring case and
//! accents (NFD, combining marks removed, lower case).
//!
//! Folders (WP-2F.07): `folder` and `recursive` scope the list to one folder, a folder and its
//! subfolders, or the top level ([`folder_scope`]).
//!
//! Tags, ratings and collections (WP-2F.06a, ADR-0036): `tags` keeps the assets that have all
//! of them (compared by `tag_key`), `min_rating` those rated at least that, `collection` the
//! members of a manual or smart collection; a term of `text` may also occur in a tag or in the
//! note.

use std::{borrow::Cow, cmp::Ordering, collections::HashSet};

use cutweave_commands::{CmdError, wire};
use cutweave_model::{
    Collection, CollectionId, CollectionKind, EntityKind, FolderId, MediaAsset, MediaId, MediaKind,
    Project, tag_key,
};
// The filter is part of the document since ADR-0036 (a smart collection stores one), so the
// type and the fold it matches with live in `cutweave-model`.
pub use cutweave_model::{MAX_FILTER_TEXT_CHARS, MediaFilter, fold};

/// What to order by: `imported` (by ID), `name`, `duration` (stills first when ascending) or
/// `kind` (video, audio, image, sequence; by name within a kind).
#[wire]
#[derive(Copy, Eq, Hash)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum MediaSortBy {
    Imported,
    Name,
    Duration,
    Kind,
}

/// `asc` (default), or `desc`: exactly the reverse.
#[wire]
#[derive(Copy, Default, Eq, Hash)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum MediaSortOrder {
    #[default]
    Asc,
    Desc,
}

/// How to order the list; without one, by ID ascending.
#[wire]
#[derive(Copy, Eq)]
pub struct MediaSort {
    /// What to order by.
    pub by: MediaSortBy,
    /// Direction.
    #[serde(default)]
    pub order: MediaSortOrder,
}

impl Default for MediaSort {
    fn default() -> MediaSort {
        MediaSort {
            by: MediaSortBy::Imported,
            order: MediaSortOrder::Asc,
        }
    }
}

/// The name the bin shows for an asset: its display name (ADR-0020), else the last component of
/// its relative or absolute path, else its ID.
#[must_use]
pub fn asset_name(media: &MediaAsset) -> Cow<'_, str> {
    if let Some(name) = &media.name {
        return Cow::Borrowed(name.as_str());
    }
    match media.path_rel.as_ref().or(media.path_abs.as_ref()) {
        Some(path) => Cow::Borrowed(path.rsplit(['/', '\\']).next().unwrap_or(path.as_str())),
        None => Cow::Owned(media.id.to_string()),
    }
}

/// The IDs of every asset placed on a timeline of any sequence.
#[must_use]
pub fn used_media(doc: &Project) -> HashSet<MediaId> {
    doc.sequences
        .values()
        .flat_map(|s| s.tracks.values())
        .flat_map(|t| t.items.iter())
        .filter_map(cutweave_model::Item::media)
        .collect()
}

fn is_term_break(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r' | '\n')
}

/// A file kind whose probe found no streams: nothing can be read from it.
#[must_use]
pub fn is_problem(media: &MediaAsset) -> bool {
    media.kind != MediaKind::SequenceRef && media.probe.streams.is_empty()
}

/// The folders whose assets `filter` keeps (`None` inside: the top level), or `None` when it
/// keeps every folder.
///
/// # Errors
/// `NotFound` when `filter.folder` does not exist.
pub fn folder_scope(
    doc: &Project,
    filter: &MediaFilter,
) -> Result<Option<HashSet<Option<FolderId>>>, CmdError> {
    if let Some(folder) = filter.folder
        && !doc.folders.contains_key(&folder)
    {
        return Err(CmdError::not_found(EntityKind::Folder, folder));
    }
    let recursive = filter.recursive.unwrap_or(filter.folder.is_none());
    Ok(match (filter.folder, recursive) {
        (None, true) => None,
        (folder, false) => Some(std::iter::once(folder).collect()),
        (Some(folder), true) => Some(doc.folder_subtree(folder).into_iter().map(Some).collect()),
    })
}

/// The collection, or `NotFound`.
pub fn find_collection(doc: &Project, id: CollectionId) -> Result<&Collection, CmdError> {
    doc.collections
        .get(&id)
        .ok_or_else(|| CmdError::not_found(EntityKind::Collection, id))
}

/// Whether matching `filter` reads which assets are on a timeline ([`used_media`]): it asks
/// for `used`, or names a smart collection whose filter does.
#[must_use]
pub fn needs_used(doc: &Project, filter: &MediaFilter) -> bool {
    filter.used.is_some()
        || filter
            .collection
            .and_then(|c| doc.collections.get(&c))
            .is_some_and(|c| match &c.kind {
                CollectionKind::Smart { filter } => filter.used.is_some(),
                CollectionKind::Manual { .. } => false,
            })
}

/// A filter prepared for many assets.
pub struct Matcher<'a> {
    filter: &'a MediaFilter,
    terms: Vec<String>,
    /// The keys ([`tag_key`]) of the tags an asset must have.
    tags: Vec<String>,
    used: Option<&'a HashSet<MediaId>>,
    folders: Option<HashSet<Option<FolderId>>>,
    /// The members of the manual collection the filter names.
    members: Option<HashSet<MediaId>>,
    /// The filter of the smart collection the filter names.
    smart: Option<Box<Matcher<'a>>>,
    /// Nothing matches: a smart collection whose filter can no longer be used.
    nothing: bool,
}

impl<'a> Matcher<'a> {
    /// Checks the filter and folds its text and tags once. `used` is read when `filter.used`
    /// is set. The folder and the collection of the filter are not applied: see
    /// [`Matcher::for_doc`].
    ///
    /// # Errors
    /// `Invalid` when the filter cannot be used ([`MediaFilter::problem`]).
    pub fn new(
        filter: &'a MediaFilter,
        used: Option<&'a HashSet<MediaId>>,
    ) -> Result<Matcher<'a>, CmdError> {
        if let Some((pointer, reason)) = filter.problem() {
            return Err(CmdError::invalid(format!("/filter{pointer}"), reason));
        }
        let terms = fold(filter.text.as_deref().unwrap_or(""))
            .split(is_term_break)
            .filter(|t| !t.is_empty())
            .map(str::to_owned)
            .collect();
        let tags = filter
            .tags
            .iter()
            .flatten()
            .map(|t| tag_key(t.trim()).into_owned())
            .collect();
        Ok(Matcher {
            filter,
            terms,
            tags,
            used,
            folders: None,
            members: None,
            smart: None,
            nothing: false,
        })
    }

    /// The whole filter against `doc`: [`Matcher::new`] plus its folder scope and its
    /// collection. `used` is [`used_media`] of `doc`, needed when [`needs_used`] says so.
    ///
    /// A smart collection whose own filter can no longer be used (its folder is gone) has no
    /// members.
    ///
    /// # Errors
    /// `Invalid` as [`Matcher::new`]; `NotFound` when the folder or the collection of `filter`
    /// does not exist.
    pub fn for_doc(
        doc: &'a Project,
        filter: &'a MediaFilter,
        used: Option<&'a HashSet<MediaId>>,
    ) -> Result<Matcher<'a>, CmdError> {
        let mut matcher = Matcher::new(filter, used)?.in_folders(folder_scope(doc, filter)?);
        if let Some(id) = filter.collection {
            match &find_collection(doc, id)?.kind {
                CollectionKind::Manual { media } => {
                    matcher.members = Some(media.iter().copied().collect());
                }
                CollectionKind::Smart { filter: inner } => {
                    // A stored filter names no collection (a document invariant), so this
                    // goes one level deep; one that does is treated as matching nothing.
                    match Matcher::for_doc(doc, inner, used) {
                        Ok(inner_matcher) if inner.collection.is_none() => {
                            matcher.smart = Some(Box::new(inner_matcher));
                        }
                        _ => matcher.nothing = true,
                    }
                }
            }
        }
        Ok(matcher)
    }

    /// The same matcher, keeping only assets in `scope` (see [`folder_scope`]).
    #[must_use]
    pub fn in_folders(mut self, scope: Option<HashSet<Option<FolderId>>>) -> Matcher<'a> {
        self.folders = scope;
        self
    }

    /// Whether the asset passes every part of the filter.
    #[must_use]
    pub fn matches(&self, media: &MediaAsset) -> bool {
        let f = self.filter;
        if self.nothing {
            return false;
        }
        if let Some(scope) = &self.folders
            && !scope.contains(&media.folder)
        {
            return false;
        }
        if let Some(members) = &self.members
            && !members.contains(&media.id)
        {
            return false;
        }
        if let Some(kinds) = &f.kinds
            && !kinds.is_empty()
            && !kinds.contains(&media.kind)
        {
            return false;
        }
        if f.min_duration.is_some() || f.max_duration.is_some() {
            let Some(d) = media.probe.duration else {
                return false;
            };
            if f.min_duration.is_some_and(|m| d < m) || f.max_duration.is_some_and(|m| d > m) {
                return false;
            }
        }
        if let Some(min) = f.min_rating
            && media.rating.is_none_or(|r| r < min)
        {
            return false;
        }
        if let Some(want) = f.used {
            let is_used = self.used.is_some_and(|u| u.contains(&media.id));
            if is_used != want {
                return false;
            }
        }
        if f.problems == Some(true) && !is_problem(media) {
            return false;
        }
        if !self
            .tags
            .iter()
            .all(|want| media.tags.iter().any(|t| tag_key(t) == want.as_str()))
        {
            return false;
        }
        if !self.terms.is_empty() && !self.text_matches(media) {
            return false;
        }
        self.smart.as_ref().is_none_or(|inner| inner.matches(media))
    }

    /// Every term occurs in the name, a tag or the note. Tags and the note are folded only
    /// when the name alone does not hold every term.
    fn text_matches(&self, media: &MediaAsset) -> bool {
        let name = asset_name(media);
        let name = fold(&name);
        if self.terms.iter().all(|t| name.contains(t.as_str())) {
            return true;
        }
        if media.tags.is_empty() && media.note.is_none() {
            return false;
        }
        let tags: Vec<Cow<'_, str>> = media.tags.iter().map(|t| fold(t)).collect();
        let note = media.note.as_deref().map(fold);
        self.terms.iter().all(|t| {
            let t = t.as_str();
            name.contains(t)
                || tags.iter().any(|tag| tag.contains(t))
                || note.as_ref().is_some_and(|n| n.contains(t))
        })
    }
}

/// The assets on `collection`, as `media.list` would return them: a manual collection's
/// members in the order they were added, skipping those that name no asset; a smart
/// collection's in ID order. `used` is [`used_media`] of `doc`, needed when the collection is
/// smart and its filter asks for `used`.
#[must_use]
// `used` is the set `used_media` returns, which `Matcher` borrows as it is.
#[allow(clippy::implicit_hasher)]
pub fn collection_members(
    doc: &Project,
    collection: &Collection,
    used: Option<&HashSet<MediaId>>,
) -> Vec<MediaId> {
    match &collection.kind {
        CollectionKind::Manual { media } => media
            .iter()
            .copied()
            .filter(|m| doc.media.contains_key(m))
            .collect(),
        CollectionKind::Smart { filter } => {
            if filter.collection.is_some() {
                return Vec::new();
            }
            let Ok(matcher) = Matcher::for_doc(doc, filter, used) else {
                return Vec::new();
            };
            doc.media
                .values()
                .filter(|m| matcher.matches(m))
                .map(|m| m.id)
                .collect()
        }
    }
}

fn kind_rank(kind: MediaKind) -> u8 {
    match kind {
        MediaKind::Video => 0,
        MediaKind::Audio => 1,
        MediaKind::Image => 2,
        MediaKind::ImageSequence => 3,
        MediaKind::SequenceRef => 4,
    }
}

/// An asset with its sort keys computed once.
pub struct Keyed<'a> {
    /// The asset.
    pub media: &'a MediaAsset,
    name: Cow<'a, str>,
}

impl<'a> Keyed<'a> {
    /// Computes the keys `by` needs for `media`.
    #[must_use]
    pub fn new(by: MediaSortBy, media: &'a MediaAsset) -> Keyed<'a> {
        let name = match by {
            MediaSortBy::Name | MediaSortBy::Kind => match asset_name(media) {
                Cow::Borrowed(name) => fold(name),
                Cow::Owned(name) => Cow::Owned(fold(&name).into_owned()),
            },
            MediaSortBy::Imported | MediaSortBy::Duration => Cow::Borrowed(""),
        };
        Keyed { media, name }
    }
}

/// The ascending order of two assets under `by`; ties fall back to the ID, so it is total.
/// Names compare by code point.
#[must_use]
pub fn compare(by: MediaSortBy, a: &Keyed<'_>, b: &Keyed<'_>) -> Ordering {
    let primary = match by {
        MediaSortBy::Imported => Ordering::Equal,
        MediaSortBy::Name => a.name.cmp(&b.name),
        MediaSortBy::Duration => a.media.probe.duration.cmp(&b.media.probe.duration),
        MediaSortBy::Kind => kind_rank(a.media.kind)
            .cmp(&kind_rank(b.media.kind))
            .then_with(|| a.name.cmp(&b.name)),
    };
    primary.then_with(|| a.media.id.cmp(&b.media.id))
}
