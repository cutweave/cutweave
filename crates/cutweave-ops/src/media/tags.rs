// SPDX-License-Identifier: GPL-3.0-or-later
//! Tags, ratings and notes on media (WP-2F.06a, ADR-0036): `media.add_tags`,
//! `media.remove_tags`, `media.rename_tag`, `media.set_rating`, `media.set_note` and the
//! `media.list_tags` query.
//!
//! A tag is a word people type: 1 to 64 characters, no comma, no control characters. Two tags
//! are the same when they are equal ignoring case and accents ([`tag_key`]); an asset keeps the
//! spelling it got first, and its tags are stored sorted by that key. There is no tag registry:
//! a tag exists while an asset carries it. Adding and removing are idempotent, and every
//! command is one undo step that never touches a timeline or a file on disk.

use std::collections::{BTreeMap, HashSet};

use cutweave_commands::{
    CmdError, Command, DocPath, Query, QueryCtx, Registry, RegistryError, Tx, command, query, wire,
};
use cutweave_model::{
    EntityKind, MAX_NOTE_CHARS, MAX_TAGS_PER_ASSET, MediaId, Project, RATINGS, note_problem,
    tag_key, tag_problem,
};

/// Most assets one tag or rating command accepts.
pub const MAX_TAG_MEDIA: usize = 256;
/// Most tags one `media.add_tags` or `media.remove_tags` accepts.
pub const MAX_TAGS_PER_CALL: usize = 16;

/// Registers the tag, rating and note commands and the `media.list_tags` query.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    registry
        .register::<MediaAddTags>()?
        .register::<MediaRemoveTags>()?
        .register::<MediaRenameTag>()?
        .register::<MediaSetRating>()?
        .register::<MediaSetNote>()?
        .register::<MediaListTags>()?;
    Ok(())
}

/// Checks the `media` of a tag or rating command: 1 to [`MAX_TAG_MEDIA`] assets that exist,
/// none listed twice.
fn check_media(doc: &Project, media: &[MediaId]) -> Result<(), CmdError> {
    if media.is_empty() || media.len() > MAX_TAG_MEDIA {
        return Err(CmdError::invalid(
            "/media",
            format!("give 1..={MAX_TAG_MEDIA} assets"),
        ));
    }
    let mut seen = HashSet::with_capacity(media.len());
    for (i, id) in media.iter().enumerate() {
        if !seen.insert(*id) {
            return Err(CmdError::invalid(
                format!("/media/{i}"),
                format!("`{id}` is listed twice"),
            ));
        }
        if !doc.media.contains_key(id) {
            return Err(CmdError::not_found(EntityKind::Media, *id));
        }
    }
    Ok(())
}

/// One tag as it is stored: trimmed, and passing [`tag_problem`].
fn check_tag(pointer: &str, tag: &str) -> Result<String, CmdError> {
    let tag = tag.trim();
    match tag_problem(tag) {
        Some(problem) => Err(CmdError::invalid(pointer, problem)),
        None => Ok(tag.to_owned()),
    }
}

/// The `tags` of a call: 1 to [`MAX_TAGS_PER_CALL`], each checked and trimmed; a tag given
/// twice (ignoring case and accents) counts once, in its first spelling.
fn check_tags(tags: &[String]) -> Result<Vec<String>, CmdError> {
    if tags.is_empty() || tags.len() > MAX_TAGS_PER_CALL {
        return Err(CmdError::invalid(
            "/tags",
            format!("give 1..={MAX_TAGS_PER_CALL} tags"),
        ));
    }
    let mut out: Vec<String> = Vec::with_capacity(tags.len());
    for (i, tag) in tags.iter().enumerate() {
        let tag = check_tag(&format!("/tags/{i}"), tag)?;
        if !out.iter().any(|t| tag_key(t) == tag_key(&tag)) {
            out.push(tag);
        }
    }
    Ok(out)
}

/// Sorts tags into their stored order (by [`tag_key`]).
fn sort_tags(tags: &mut [String]) {
    tags.sort_by(|a, b| tag_key(a).cmp(&tag_key(b)));
}

/// Writes an asset's tags; an empty list leaves the field out of the document.
fn write_tags(tx: &mut Tx<'_>, media: MediaId, tags: &[String]) -> Result<(), CmdError> {
    let path = DocPath::media(media).join("tags");
    if tags.is_empty() {
        if tx.get(&path).is_some() {
            tx.remove_path(&path)?;
        }
        Ok(())
    } else {
        tx.set(&path, tags)
    }
}

/// Writes an optional field of an asset that is left out of the document while absent.
fn write_optional<T: serde::Serialize>(
    tx: &mut Tx<'_>,
    media: MediaId,
    field: &str,
    value: Option<&T>,
) -> Result<(), CmdError> {
    let path = DocPath::media(media).join(field);
    if let Some(value) = value {
        return tx.set(&path, value);
    }
    if tx.get(&path).is_some() {
        tx.remove_path(&path)?;
    }
    Ok(())
}

/// Result of the tag and rating commands.
#[wire]
pub struct MediaTagged {
    /// The assets that changed (those that already were as asked are left out).
    pub changed: Vec<MediaId>,
}

/// The new tags of each asset a tag command changes.
type TagPlan = Vec<(MediaId, Vec<String>)>;

fn apply_plan(tx: &mut Tx<'_>, plan: TagPlan) -> Result<MediaTagged, CmdError> {
    let mut changed = Vec::with_capacity(plan.len());
    for (media, tags) in plan {
        write_tags(tx, media, &tags)?;
        changed.push(media);
    }
    Ok(MediaTagged { changed })
}

// ----- media.add_tags ------------------------------------------------------------------------

/// Adds tags to media assets. A tag is 1 to 64 characters without commas or control characters
/// (trimmed); two tags are the same when they are equal ignoring case and accents, and an
/// asset that already has a tag keeps its spelling. An asset holds at most 64 tags. Adding a
/// tag an asset already has changes nothing, so the call can be repeated.
#[command(
    name = "media.add_tags",
    summary = "Add tags to media assets (idempotent)",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct MediaAddTags {
    /// The assets (1..=256, no repeats).
    pub media: Vec<MediaId>,
    /// The tags to add (1..=16).
    pub tags: Vec<String>,
}

impl MediaAddTags {
    fn plan(&self, doc: &Project) -> Result<TagPlan, CmdError> {
        check_media(doc, &self.media)?;
        let tags = check_tags(&self.tags)?;
        let mut plan = Vec::new();
        for (i, id) in self.media.iter().enumerate() {
            let Some(asset) = doc.media.get(id) else {
                continue;
            };
            let mut next = asset.tags.clone();
            for tag in &tags {
                if !next.iter().any(|t| tag_key(t) == tag_key(tag)) {
                    next.push(tag.clone());
                }
            }
            if next.len() == asset.tags.len() {
                continue;
            }
            if next.len() > MAX_TAGS_PER_ASSET {
                return Err(CmdError::invalid(
                    format!("/media/{i}"),
                    format!(
                        "media `{id}` would have {} tags; an asset holds at most \
                         {MAX_TAGS_PER_ASSET}",
                        next.len()
                    ),
                ));
            }
            sort_tags(&mut next);
            plan.push((*id, next));
        }
        Ok(plan)
    }
}

impl Command for MediaAddTags {
    type Output = MediaTagged;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        self.plan(ctx.doc()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<MediaTagged, CmdError> {
        let plan = self.plan(tx.doc())?;
        apply_plan(tx, plan)
    }
}

// ----- media.remove_tags ---------------------------------------------------------------------

/// Removes tags from media assets, matching them ignoring case and accents. Removing a tag an
/// asset does not have changes nothing, so the call can be repeated.
#[command(
    name = "media.remove_tags",
    summary = "Remove tags from media assets (idempotent)",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct MediaRemoveTags {
    /// The assets (1..=256, no repeats).
    pub media: Vec<MediaId>,
    /// The tags to remove (1..=16).
    pub tags: Vec<String>,
}

impl MediaRemoveTags {
    fn plan(&self, doc: &Project) -> Result<TagPlan, CmdError> {
        check_media(doc, &self.media)?;
        let tags = check_tags(&self.tags)?;
        let mut plan = Vec::new();
        for id in &self.media {
            let Some(asset) = doc.media.get(id) else {
                continue;
            };
            let next: Vec<String> = asset
                .tags
                .iter()
                .filter(|t| !tags.iter().any(|gone| tag_key(gone) == tag_key(t)))
                .cloned()
                .collect();
            if next.len() != asset.tags.len() {
                plan.push((*id, next));
            }
        }
        Ok(plan)
    }
}

impl Command for MediaRemoveTags {
    type Output = MediaTagged;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        self.plan(ctx.doc()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<MediaTagged, CmdError> {
        let plan = self.plan(tx.doc())?;
        apply_plan(tx, plan)
    }
}

// ----- media.rename_tag ----------------------------------------------------------------------

/// Renames a tag on every asset that has it, as one undo step. `from` is matched ignoring case
/// and accents; afterwards each of those assets carries `to` exactly as given (an asset that
/// had both keeps one). Renaming to another spelling of the same tag (`interview` to
/// `Interview`) respells it. When no asset has `from`, nothing changes.
#[command(
    name = "media.rename_tag",
    summary = "Rename a tag on every media asset that has it",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct MediaRenameTag {
    /// The tag to rename.
    pub from: String,
    /// Its new name: 1 to 64 characters without commas or control characters (trimmed).
    pub to: String,
}

impl MediaRenameTag {
    fn plan(&self, doc: &Project) -> Result<TagPlan, CmdError> {
        let from = check_tag("/from", &self.from)?;
        let to = check_tag("/to", &self.to)?;
        let (from_key, to_key) = (tag_key(&from), tag_key(&to));
        let mut plan = Vec::new();
        for asset in doc.media.values() {
            if !asset.tags.iter().any(|t| tag_key(t) == from_key) {
                continue;
            }
            let mut next: Vec<String> = asset
                .tags
                .iter()
                .filter(|t| {
                    let key = tag_key(t);
                    key != from_key && key != to_key
                })
                .cloned()
                .collect();
            next.push(to.clone());
            sort_tags(&mut next);
            if next != asset.tags {
                plan.push((asset.id, next));
            }
        }
        Ok(plan)
    }
}

impl Command for MediaRenameTag {
    type Output = MediaTagged;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        self.plan(ctx.doc()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<MediaTagged, CmdError> {
        let plan = self.plan(tx.doc())?;
        apply_plan(tx, plan)
    }
}

// ----- media.set_rating ----------------------------------------------------------------------

/// Rates media assets from 1 to 5 stars, or clears their rating without `rating`.
#[command(
    name = "media.set_rating",
    summary = "Rate media assets 1 to 5 stars, or clear the rating",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct MediaSetRating {
    /// The assets (1..=256, no repeats).
    pub media: Vec<MediaId>,
    /// Whole stars, 1 to 5; absent or null: unrated.
    #[schemars(range(min = 1, max = 5))]
    pub rating: Option<u8>,
}

impl MediaSetRating {
    fn plan(&self, doc: &Project) -> Result<Vec<MediaId>, CmdError> {
        check_media(doc, &self.media)?;
        if self.rating.is_some_and(|r| !RATINGS.contains(&r)) {
            return Err(CmdError::invalid(
                "/rating",
                "the rating must be 1 to 5, or absent to clear it",
            ));
        }
        Ok(self
            .media
            .iter()
            .copied()
            .filter(|id| doc.media.get(id).is_some_and(|m| m.rating != self.rating))
            .collect())
    }
}

impl Command for MediaSetRating {
    type Output = MediaTagged;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        self.plan(ctx.doc()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<MediaTagged, CmdError> {
        let changed = self.plan(tx.doc())?;
        for id in &changed {
            write_optional(tx, *id, "rating", self.rating.as_ref())?;
        }
        Ok(MediaTagged { changed })
    }
}

// ----- media.set_note ------------------------------------------------------------------------

/// Result of `media.set_note`.
#[wire]
#[derive(Copy)]
pub struct MediaNoteSet {
    /// The asset.
    pub media_id: MediaId,
    /// Whether it has a note now.
    pub has_note: bool,
}

/// Sets the note of a media asset: plain text of at most 2,000 characters (line breaks and
/// tabs allowed, other control characters not; space around it is trimmed). Without `note`, or
/// with a blank one, the note is removed.
#[command(
    name = "media.set_note",
    summary = "Set or clear the note of a media asset",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct MediaSetNote {
    /// The asset.
    pub media: MediaId,
    /// The note; absent, null or blank: no note.
    pub note: Option<String>,
}

impl MediaSetNote {
    fn plan(&self, ctx: &QueryCtx<'_>) -> Result<Option<String>, CmdError> {
        ctx.media(self.media)?;
        let Some(note) = self
            .note
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
        else {
            return Ok(None);
        };
        if note.chars().count() > MAX_NOTE_CHARS {
            return Err(CmdError::invalid(
                "/note",
                format!("a note is at most {MAX_NOTE_CHARS} characters"),
            ));
        }
        match note_problem(note) {
            Some(problem) => Err(CmdError::invalid("/note", problem)),
            None => Ok(Some(note.to_owned())),
        }
    }
}

impl Command for MediaSetNote {
    type Output = MediaNoteSet;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        self.plan(ctx).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<MediaNoteSet, CmdError> {
        let note = self.plan(&tx.query_ctx())?;
        write_optional(tx, self.media, "note", note.as_ref())?;
        Ok(MediaNoteSet {
            media_id: self.media,
            has_note: note.is_some(),
        })
    }
}

// ----- media.list_tags -----------------------------------------------------------------------

/// One tag of the project.
#[wire]
pub struct TagCount {
    /// The tag, in the spelling of the first asset (by ID) that has it.
    pub tag: String,
    /// How many assets have it.
    pub count: u32,
}

/// Result of `media.list_tags`.
#[wire]
pub struct MediaListTagsOutput {
    /// Every tag some asset has, sorted ignoring case and accents.
    pub tags: Vec<TagCount>,
}

/// Lists every tag in the project with the number of assets that carry it. Tags that differ
/// only in case or accents count as one.
#[query(
    name = "media.list_tags",
    summary = "List the project's media tags with how many assets have each",
    mcp = "catalog"
)]
pub struct MediaListTags {}

impl Query for MediaListTags {
    type Output = MediaListTagsOutput;

    fn run(&self, ctx: &QueryCtx<'_>) -> Result<MediaListTagsOutput, CmdError> {
        let mut counts: BTreeMap<String, TagCount> = BTreeMap::new();
        for asset in ctx.doc().media.values() {
            for tag in &asset.tags {
                counts
                    .entry(tag_key(tag).into_owned())
                    .or_insert_with(|| TagCount {
                        tag: tag.clone(),
                        count: 0,
                    })
                    .count += 1;
            }
        }
        Ok(MediaListTagsOutput {
            tags: counts.into_values().collect(),
        })
    }
}
