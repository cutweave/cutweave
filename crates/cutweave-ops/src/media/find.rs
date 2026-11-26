// SPDX-License-Identifier: GPL-3.0-or-later
//! Finding media (WP-1M.12): the filter and sort of `media.list`. The editor's bin applies the
//! same predicate in TypeScript (`packages/editor/src/media/find.ts`); both are checked against
//! `tests/fixtures/media-find.json`, which `tests/media_find_fixture.rs` generates from this code.
//!
//! Text matching: the name is the asset's display name (ADR-0020), else its file name (relative
//! path, else absolute, else the ID); terms are split on space, tab, CR and LF; every term must
//! occur, ignoring case and accents (NFD, combining marks removed, lower case).
//!
//! Folders (WP-2F.07): `folder` and `recursive` scope the list to one folder, a folder and its
//! subfolders, or the top level ([`folder_scope`]).

use std::{borrow::Cow, cmp::Ordering, collections::HashSet};

use cutweave_commands::{CmdError, wire};
use cutweave_model::{Duration, EntityKind, FolderId, MediaAsset, MediaId, MediaKind, Project};
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

/// Longest accepted `filter.text`, in characters.
pub const MAX_FILTER_TEXT_CHARS: usize = 256;

/// Which media to list; every part that is absent or empty lets everything through.
#[wire]
#[derive(Default)]
pub struct MediaFilter {
    /// Terms that must all occur in the name, ignoring case and accents.
    #[serde(default)]
    pub text: Option<String>,
    /// Only these kinds.
    #[serde(default)]
    pub kinds: Option<Vec<MediaKind>>,
    /// Shortest length (flicks), inclusive; stills never match a bound.
    #[serde(default)]
    pub min_duration: Option<Duration>,
    /// Longest length (flicks), inclusive.
    #[serde(default)]
    pub max_duration: Option<Duration>,
    /// Placed on a timeline of any sequence (`true`) or not (`false`).
    #[serde(default)]
    pub used: Option<bool>,
    /// `true`: only files nothing can be read from (no streams).
    #[serde(default)]
    pub problems: Option<bool>,
    // Without `folder`, `recursive: false` lists the top level of the bin (the default lists
    // the whole project); with one, the default is that folder only (ADR-0020). The doc
    // comments are short: they count against the core MCP tool budget.
    /// Only this bin folder.
    #[serde(default)]
    pub folder: Option<FolderId>,
    /// With subfolders (default: true without folder).
    #[serde(default)]
    pub recursive: Option<bool>,
}

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

/// The text of `s` as matching sees it: accents removed, lower case. Plain ASCII that is
/// already lower case is borrowed, so the common case allocates nothing.
#[must_use]
pub fn fold(s: &str) -> Cow<'_, str> {
    if s.is_ascii() {
        return if s.bytes().any(|b| b.is_ascii_uppercase()) {
            Cow::Owned(s.to_ascii_lowercase())
        } else {
            Cow::Borrowed(s)
        };
    }
    Cow::Owned(
        s.nfd()
            .filter(|c| !is_combining_mark(*c))
            .collect::<String>()
            .to_lowercase(),
    )
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

/// A filter prepared for many assets.
pub struct Matcher<'a> {
    filter: &'a MediaFilter,
    terms: Vec<String>,
    used: Option<&'a HashSet<MediaId>>,
    folders: Option<HashSet<Option<FolderId>>>,
}

impl<'a> Matcher<'a> {
    /// Checks the filter and folds its text once. `used` is read when `filter.used` is set.
    ///
    /// # Errors
    /// `Invalid` when the text is too long or the duration bounds are reversed.
    pub fn new(
        filter: &'a MediaFilter,
        used: Option<&'a HashSet<MediaId>>,
    ) -> Result<Matcher<'a>, CmdError> {
        let text = filter.text.as_deref().unwrap_or("");
        if text.chars().count() > MAX_FILTER_TEXT_CHARS {
            return Err(CmdError::invalid(
                "/filter/text",
                format!("the text is at most {MAX_FILTER_TEXT_CHARS} characters"),
            ));
        }
        if let (Some(min), Some(max)) = (filter.min_duration, filter.max_duration)
            && min > max
        {
            return Err(CmdError::invalid(
                "/filter/min_duration",
                "min_duration is longer than max_duration",
            ));
        }
        let terms = fold(text)
            .split(is_term_break)
            .filter(|t| !t.is_empty())
            .map(str::to_owned)
            .collect();
        Ok(Matcher {
            filter,
            terms,
            used,
            folders: None,
        })
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
        if let Some(scope) = &self.folders
            && !scope.contains(&media.folder)
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
        if let Some(want) = f.used {
            let is_used = self.used.is_some_and(|u| u.contains(&media.id));
            if is_used != want {
                return false;
            }
        }
        if f.problems == Some(true) && !is_problem(media) {
            return false;
        }
        if !self.terms.is_empty() {
            let name = asset_name(media);
            let name = fold(&name);
            if !self.terms.iter().all(|t| name.contains(t.as_str())) {
                return false;
            }
        }
        true
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
