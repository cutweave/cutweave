// SPDX-License-Identifier: GPL-3.0-or-later
//! Document invariants (arch §4.3) and [`Project::validate`].
//!
//! Commands keep these invariants; property tests assert them after every command
//! (WP-1B.07). Paths in reports are JSON Pointers into the saved document.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::{
    CURRENT_SCHEMA_VERSION, Clip, Custom, Duration, EntityKind, FolderId, GroupId, Item, ItemId,
    MAX_FOLDER_DEPTH, MAX_SAFE_FLICKS, MediaId, MediaKind, Project, PropId, PropRef, Sequence,
    SequenceFormat, SequenceId, Time, TrackId, TrackKind, folder_name_key, folder_name_problem,
    media_name_problem,
};

/// Largest canvas side accepted by validation.
pub const MAX_CANVAS_SIDE: u32 = 16_384;
/// Highest sequence frame rate accepted by validation (fps).
pub const MAX_SEQUENCE_FPS: u32 = 1000;
/// Sequence sample rates accepted by validation (Hz).
pub const SEQUENCE_SAMPLE_RATES: std::ops::RangeInclusive<u32> = 8_000..=384_000;
/// Accepted timeline row heights in pixels (`Track::height`).
pub const TRACK_HEIGHTS: std::ops::RangeInclusive<u32> = 8..=512;

/// A broken document invariant: what [`Project::validate`] reports.
#[derive(
    Clone, Debug, PartialEq, Eq, thiserror::Error, Serialize, schemars::JsonSchema, specta::Type,
)]
#[serde(tag = "invariant", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Invariant {
    /// The document is not at the schema version this build edits (open it through
    /// `open`/`migrate_to_current`, which migrate older documents).
    #[error("schema_version is {found}, but this version of Cutweave edits {expected}")]
    SchemaVersion {
        /// The document's `meta.schema_version`.
        found: u32,
        /// `CURRENT_SCHEMA_VERSION`.
        expected: u32,
    },
    /// A map key differs from the ID inside the entity.
    #[error("`{path}/{key}` holds an entity whose id is `{id}`")]
    KeyMismatch {
        /// The map (`/media`, `/sequences/<id>/tracks`, …).
        path: String,
        /// The key.
        key: String,
        /// The entity's own ID.
        id: String,
    },
    /// A reference to an entity that does not exist.
    #[error("`{path}` refers to {kind} `{id}`, which does not exist")]
    DanglingReference {
        /// Where the reference is.
        path: String,
        /// The kind of entity referred to.
        kind: EntityKind,
        /// The missing ID.
        id: String,
    },
    /// A media asset whose fields contradict its kind.
    #[error("media `{media}` is invalid: {reason}")]
    InvalidMedia {
        /// The media.
        media: MediaId,
        /// What is wrong.
        reason: String,
    },
    /// A sequence format (or the default format) outside the supported range.
    #[error("format at `{path}` is invalid: {reason}")]
    InvalidFormat {
        /// The format.
        path: String,
        /// What is wrong.
        reason: String,
    },
    /// `track_order` is not exactly the keys of `tracks`.
    #[error("track order of sequence `{sequence}` is invalid: {reason}")]
    TrackOrder {
        /// The sequence.
        sequence: SequenceId,
        /// What is wrong.
        reason: String,
    },
    /// A clip whose media cannot play on its track (e.g. audio on a video track).
    #[error(
        "item `{item}` on {track_kind} track `{track}` cannot play {media_kind} media `{media}`"
    )]
    IncompatibleMedia {
        /// The item.
        item: ItemId,
        /// Its track.
        track: TrackId,
        /// The track kind.
        track_kind: TrackKind,
        /// The media.
        media: MediaId,
        /// The media kind.
        media_kind: MediaKind,
    },
    /// An item with a non-positive duration, a negative start or source position, or an end
    /// past the largest representable time.
    #[error("item `{item}` has invalid timing: {reason}")]
    InvalidTiming {
        /// The item.
        item: ItemId,
        /// What is wrong.
        reason: String,
    },
    /// A clip that reads past the end of its media (`source_in + duration > media duration`).
    #[error(
        "item `{item}` reads media `{media}` up to {source_end} but the media lasts {media_duration}"
    )]
    SourceOutOfBounds {
        /// The item.
        item: ItemId,
        /// The media.
        media: MediaId,
        /// `source_in + duration`.
        source_end: Time,
        /// The media duration.
        media_duration: Duration,
    },
    /// Two items of one track overlap (arch §4.3: never).
    #[error("items `{first}` and `{second}` overlap on track `{track}` of sequence `{sequence}`")]
    Overlap {
        /// The sequence.
        sequence: SequenceId,
        /// The track.
        track: TrackId,
        /// The earlier item.
        first: ItemId,
        /// The item starting inside it.
        second: ItemId,
    },
    /// Sequences that contain each other through compound clips (arch §4.3).
    #[error("compound clips form a cycle: {}", display_cycle(cycle))]
    CompoundCycle {
        /// The sequences on the cycle; the first is repeated at the end.
        cycle: Vec<SequenceId>,
    },
    /// A linked group with items in two sequences.
    #[error("linked group `{group}` has items in sequences `{first}` and `{second}`")]
    LinkedGroupSpansSequences {
        /// The group.
        group: GroupId,
        /// One sequence.
        first: SequenceId,
        /// Another sequence.
        second: SequenceId,
    },
    /// A number that is not finite or outside its range (e.g. opacity above 1).
    #[error("`{path}` is invalid: {reason}")]
    InvalidValue {
        /// The value.
        path: String,
        /// What is wrong.
        reason: String,
    },
    /// The magnetic main track rules (spec INV-6, INV-7): at most one per sequence, a video
    /// track, gap-free from 0.
    #[error("magnetic track `{track}` of sequence `{sequence}` is invalid: {reason}")]
    MagneticTrack {
        /// The sequence.
        sequence: SequenceId,
        /// The offending track.
        track: TrackId,
        /// What is wrong.
        reason: String,
    },
    /// A linked group with fewer than two members (spec INV-8).
    #[error("linked group `{group}` has {members} member(s); a group needs at least two")]
    LinkedGroupTooSmall {
        /// The group.
        group: GroupId,
        /// How many items carry it.
        members: u32,
    },
    /// Linked items that are not aligned: members differ in `start` or `duration` (spec INV-8).
    #[error(
        "linked group `{group}` is not aligned: `{first}` and `{second}` differ in start or duration"
    )]
    LinkedGroupMisaligned {
        /// The group.
        group: GroupId,
        /// The first member (lowest ID).
        first: ItemId,
        /// A member that differs from it.
        second: ItemId,
    },
    /// An attachment that breaks spec INV-9.
    #[error("item `{item}` is attached to `{parent}`, which is not allowed: {reason}")]
    Attachment {
        /// The child.
        item: ItemId,
        /// The item it is attached to.
        parent: ItemId,
        /// What is wrong.
        reason: String,
    },
    /// Media folders whose parents form a cycle (ADR-0020).
    #[error("media folders form a cycle: {}", display_cycle(cycle))]
    FolderCycle {
        /// The folders on the cycle, each followed by its parent; the first is repeated at the
        /// end.
        cycle: Vec<FolderId>,
    },
    /// A media folder nested deeper than `MAX_FOLDER_DEPTH` levels (ADR-0020).
    #[error(
        "media folder `{folder}` is nested {depth} levels deep; at most {MAX_FOLDER_DEPTH} are allowed"
    )]
    FolderTooDeep {
        /// The folder.
        folder: FolderId,
        /// Its depth (1 at the top level).
        depth: u32,
    },
    /// A media folder name that is empty, too long, or holds `/` or control characters
    /// (ADR-0020).
    #[error("media folder `{folder}` has an invalid name: {reason}")]
    InvalidFolderName {
        /// The folder.
        folder: FolderId,
        /// What is wrong.
        reason: String,
    },
    /// Two folders of one parent whose names are equal ignoring case (ADR-0020).
    #[error(
        "media folders `{first}` and `{second}` have the same parent and the name `{name}` (ignoring case)"
    )]
    DuplicateFolderName {
        /// The parent; absent at the top level.
        parent: Option<FolderId>,
        /// The folder with the lower ID.
        first: FolderId,
        /// Another folder with the same name.
        second: FolderId,
        /// The name of `second`.
        name: String,
    },
    /// Plugin data under a key that is not a namespace (arch §4.3).
    #[error(
        "custom data key `{key}` at `{path}` is not a plugin namespace such as `com.example.plugin`"
    )]
    CustomNamespace {
        /// The custom map.
        path: String,
        /// The key.
        key: String,
    },
}

fn display_cycle<T: std::fmt::Display>(cycle: &[T]) -> String {
    cycle
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" -> ")
}

impl Project {
    /// Checks every document invariant and returns the first violation, in a deterministic
    /// order (media, folders, then sequences, by ID).
    pub fn validate(&self) -> Result<(), Invariant> {
        match self.violations().into_iter().next() {
            Some(violation) => Err(violation),
            None => Ok(()),
        }
    }

    /// Every violated invariant, in a deterministic order (empty for a valid document).
    #[must_use]
    pub fn violations(&self) -> Vec<Invariant> {
        let mut out = Vec::new();
        if self.meta.schema_version != CURRENT_SCHEMA_VERSION {
            out.push(Invariant::SchemaVersion {
                found: self.meta.schema_version,
                expected: CURRENT_SCHEMA_VERSION,
            });
        }
        check_custom("", &self.custom, &mut out);
        check_format(
            "/settings/default_format",
            &self.settings.default_format,
            &mut out,
        );
        self.check_media(&mut out);
        self.check_folders(&mut out);
        let mut groups = BTreeMap::new();
        for (key, sequence) in &self.sequences {
            let path = format!("/sequences/{key}");
            if *key != sequence.id {
                out.push(Invariant::KeyMismatch {
                    path: "/sequences".into(),
                    key: key.to_string(),
                    id: sequence.id.to_string(),
                });
            }
            check_format(&format!("{path}/format"), &sequence.format, &mut out);
            check_track_order(*key, sequence, &mut out);
            check_markers(&path, &sequence.markers, &mut out);
            if let Some(range) = &sequence.in_out
                && (range.start < Time::ZERO || range.end <= range.start)
            {
                out.push(Invariant::InvalidValue {
                    path: format!("{path}/in_out"),
                    reason: "the in point must not be negative and the out point must follow it"
                        .into(),
                });
            }
            check_magnetic(*key, sequence, &mut out);
            check_attachments(sequence, &mut out);
            for (track_key, track) in &sequence.tracks {
                let track_path = format!("{path}/tracks/{track_key}");
                if *track_key != track.id {
                    out.push(Invariant::KeyMismatch {
                        path: format!("{path}/tracks"),
                        key: track_key.to_string(),
                        id: track.id.to_string(),
                    });
                }
                check_finite(&format!("{track_path}/gain"), track.gain, &mut out);
                if let Some(height) = track.height
                    && !TRACK_HEIGHTS.contains(&height)
                {
                    out.push(Invariant::InvalidValue {
                        path: format!("{track_path}/height"),
                        reason: format!(
                            "the height must be between {} and {} pixels, got {height}",
                            TRACK_HEIGHTS.start(),
                            TRACK_HEIGHTS.end()
                        ),
                    });
                }
                check_custom(&track_path, &track.custom, &mut out);
                for (item_key, item) in track.items.iter_by_id() {
                    let item_path = format!("{track_path}/items/{item_key}");
                    if *item_key != item.id() {
                        out.push(Invariant::KeyMismatch {
                            path: format!("{track_path}/items"),
                            key: item_key.to_string(),
                            id: item.id().to_string(),
                        });
                    }
                    self.check_item(&item_path, *track_key, track.kind, item, &mut out);
                    if let Some(group) = item.linked_group() {
                        let first = *groups.entry(group).or_insert(*key);
                        if first != *key {
                            out.push(Invariant::LinkedGroupSpansSequences {
                                group,
                                first,
                                second: *key,
                            });
                        }
                    }
                }
                if let Some((first, second)) = track.items.first_overlap() {
                    out.push(Invariant::Overlap {
                        sequence: *key,
                        track: *track_key,
                        first,
                        second,
                    });
                }
            }
            check_custom(&path, &sequence.custom, &mut out);
        }
        self.check_compound_cycles(&mut out);
        self.check_linked_groups(&mut out);
        dedup_linked_groups(&mut out);
        out
    }

    /// INV-8: every group has two or more members with identical `start` and `duration`.
    fn check_linked_groups(&self, out: &mut Vec<Invariant>) {
        let mut groups: BTreeMap<GroupId, Vec<&Item>> = BTreeMap::new();
        for item in self
            .sequences
            .values()
            .flat_map(|s| s.tracks.values())
            .flat_map(|t| t.items.iter_by_id().map(|(_, i)| i))
        {
            if let Some(group) = item.linked_group() {
                groups.entry(group).or_default().push(item);
            }
        }
        for (group, mut members) in groups {
            if members.len() < 2 {
                out.push(Invariant::LinkedGroupTooSmall {
                    group,
                    members: u32::try_from(members.len()).unwrap_or(u32::MAX),
                });
                continue;
            }
            members.sort_by_key(|i| i.id());
            let first = members[0];
            if let Some(other) = members
                .iter()
                .find(|i| i.start() != first.start() || i.duration() != first.duration())
            {
                out.push(Invariant::LinkedGroupMisaligned {
                    group,
                    first: first.id(),
                    second: other.id(),
                });
            }
        }
    }

    fn check_media(&self, out: &mut Vec<Invariant>) {
        for (key, media) in &self.media {
            let path = format!("/media/{key}");
            if *key != media.id {
                out.push(Invariant::KeyMismatch {
                    path: "/media".into(),
                    key: key.to_string(),
                    id: media.id.to_string(),
                });
            }
            let invalid = |reason: &str| Invariant::InvalidMedia {
                media: *key,
                reason: reason.to_owned(),
            };
            if media.kind.is_file() {
                if media.path_abs.as_deref().is_none_or(str::is_empty) {
                    out.push(invalid("a file asset needs a non-empty path_abs"));
                }
                if media.sequence.is_some() {
                    out.push(invalid("only sequence_ref media may reference a sequence"));
                }
            } else {
                match media.sequence {
                    None => out.push(invalid("sequence_ref media needs a sequence")),
                    Some(sequence) if !self.sequences.contains_key(&sequence) => {
                        out.push(Invariant::DanglingReference {
                            path: format!("{path}/sequence"),
                            kind: EntityKind::Sequence,
                            id: sequence.to_string(),
                        });
                    }
                    Some(_) => {}
                }
            }
            if !matches!(media.probe.rotation, 0 | 90 | 180 | 270) {
                out.push(invalid("probe rotation must be 0, 90, 180 or 270"));
            }
            if let Some(reason) = media.name.as_deref().and_then(media_name_problem) {
                out.push(invalid(reason));
            }
            if let Some(folder) = media.folder
                && !self.folders.contains_key(&folder)
            {
                out.push(Invariant::DanglingReference {
                    path: format!("{path}/folder"),
                    kind: EntityKind::Folder,
                    id: folder.to_string(),
                });
            }
            check_custom(&path, &media.custom, out);
        }
    }

    /// ADR-0020: keys match IDs, parents exist, names are valid and unique among siblings
    /// ignoring case, there is no cycle and nothing is nested deeper than `MAX_FOLDER_DEPTH`.
    fn check_folders(&self, out: &mut Vec<Invariant>) {
        use std::collections::btree_map::Entry;

        let mut names: BTreeMap<(Option<FolderId>, String), FolderId> = BTreeMap::new();
        for (key, folder) in &self.folders {
            let path = format!("/folders/{key}");
            if *key != folder.id {
                out.push(Invariant::KeyMismatch {
                    path: "/folders".into(),
                    key: key.to_string(),
                    id: folder.id.to_string(),
                });
            }
            if let Some(reason) = folder_name_problem(&folder.name) {
                out.push(Invariant::InvalidFolderName {
                    folder: *key,
                    reason: reason.to_owned(),
                });
            }
            if let Some(parent) = folder.parent
                && !self.folders.contains_key(&parent)
            {
                out.push(Invariant::DanglingReference {
                    path: format!("{path}/parent"),
                    kind: EntityKind::Folder,
                    id: parent.to_string(),
                });
            }
            match names.entry((folder.parent, folder_name_key(&folder.name))) {
                Entry::Vacant(slot) => {
                    slot.insert(*key);
                }
                Entry::Occupied(first) => out.push(Invariant::DuplicateFolderName {
                    parent: folder.parent,
                    first: *first.get(),
                    second: *key,
                    name: folder.name.clone(),
                }),
            }
            check_custom(&path, &folder.custom, out);
        }
        self.check_folder_tree(out);
    }

    /// Walks every folder up to the top level once: reports each cycle once and every folder
    /// deeper than `MAX_FOLDER_DEPTH`. A missing parent counts as the top level (it is reported
    /// as a dangling reference).
    fn check_folder_tree(&self, out: &mut Vec<Invariant>) {
        #[derive(Clone, Copy)]
        enum Depth {
            At(usize),
            OnCycle,
        }
        let mut known: BTreeMap<FolderId, Depth> = BTreeMap::new();
        for start in self.folders.keys() {
            let mut chain: Vec<FolderId> = Vec::new();
            let mut cur = *start;
            let base = loop {
                if let Some(depth) = known.get(&cur) {
                    break match depth {
                        Depth::At(d) => *d,
                        Depth::OnCycle => 0,
                    };
                }
                if let Some(at) = chain.iter().position(|f| *f == cur) {
                    let mut cycle = chain[at..].to_vec();
                    cycle.push(cur);
                    out.push(Invariant::FolderCycle { cycle });
                    for f in chain.drain(at..) {
                        known.insert(f, Depth::OnCycle);
                    }
                    break 0;
                }
                chain.push(cur);
                match self.folders.get(&cur).and_then(|f| f.parent) {
                    Some(parent) if self.folders.contains_key(&parent) => cur = parent,
                    _ => break 0,
                }
            };
            for (above, folder) in chain.iter().rev().enumerate() {
                let depth = base + above + 1;
                known.insert(*folder, Depth::At(depth));
                if depth > MAX_FOLDER_DEPTH {
                    out.push(Invariant::FolderTooDeep {
                        folder: *folder,
                        depth: u32::try_from(depth).unwrap_or(u32::MAX),
                    });
                }
            }
        }
    }

    fn check_item(
        &self,
        path: &str,
        track: TrackId,
        track_kind: TrackKind,
        item: &Item,
        out: &mut Vec<Invariant>,
    ) {
        let id = item.id();
        let timing = |reason: &str| Invariant::InvalidTiming {
            item: id,
            reason: reason.to_owned(),
        };
        if item.duration().is_zero() {
            out.push(timing("duration must be positive"));
        }
        if item.start() < Time::ZERO {
            out.push(timing("start must not be negative"));
        }
        if item.end_wide() > i128::from(MAX_SAFE_FLICKS) {
            out.push(timing("the item ends past the largest representable time"));
        }
        check_custom(path, item.custom(), out);
        check_markers(path, item.markers(), out);
        match item {
            Item::Clip(clip) => self.check_clip(path, track, track_kind, clip, out),
        }
    }

    fn check_clip(
        &self,
        path: &str,
        track: TrackId,
        track_kind: TrackKind,
        clip: &Clip,
        out: &mut Vec<Invariant>,
    ) {
        if clip.source_in < Time::ZERO {
            out.push(Invariant::InvalidTiming {
                item: clip.id,
                reason: "source_in must not be negative".into(),
            });
        }
        match self.media.get(&clip.media) {
            None => out.push(Invariant::DanglingReference {
                path: format!("{path}/media"),
                kind: EntityKind::Media,
                id: clip.media.to_string(),
            }),
            Some(media) => {
                if !track_kind.accepts(media.kind) {
                    out.push(Invariant::IncompatibleMedia {
                        item: clip.id,
                        track,
                        track_kind,
                        media: clip.media,
                        media_kind: media.kind,
                    });
                }
                if let Some(media_duration) = media.probe.duration
                    && media.kind != MediaKind::Image
                {
                    let end =
                        i128::from(clip.source_in.flicks()) + i128::from(clip.duration.flicks());
                    if end > i128::from(media_duration.flicks()) {
                        out.push(Invariant::SourceOutOfBounds {
                            item: clip.id,
                            media: clip.media,
                            source_end: clip.source_in.saturating_add(clip.duration),
                            media_duration,
                        });
                    }
                }
            }
        }
        check_props(path, clip, out);
    }

    fn check_compound_cycles(&self, out: &mut Vec<Invariant>) {
        let mut edges: BTreeMap<SequenceId, BTreeSet<SequenceId>> = BTreeMap::new();
        for (key, sequence) in &self.sequences {
            let targets = edges.entry(*key).or_default();
            for track in sequence.tracks.values() {
                for item in track.items.iter() {
                    let target = item
                        .media()
                        .and_then(|m| self.media.get(&m))
                        .and_then(|m| m.sequence)
                        .filter(|s| self.sequences.contains_key(s));
                    targets.extend(target);
                }
            }
        }
        let mut state: BTreeMap<SequenceId, Visit> = BTreeMap::new();
        let mut stack = Vec::new();
        for start in edges.keys() {
            if !state.contains_key(start) {
                visit(*start, &edges, &mut state, &mut stack, out);
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Visit {
    OnStack,
    Done,
}

fn visit(
    node: SequenceId,
    edges: &BTreeMap<SequenceId, BTreeSet<SequenceId>>,
    state: &mut BTreeMap<SequenceId, Visit>,
    stack: &mut Vec<SequenceId>,
    out: &mut Vec<Invariant>,
) {
    state.insert(node, Visit::OnStack);
    stack.push(node);
    for next in edges.get(&node).into_iter().flatten() {
        match state.get(next) {
            Some(Visit::OnStack) => {
                let from = stack.iter().position(|s| s == next).unwrap_or(0);
                let mut cycle = stack[from..].to_vec();
                cycle.push(*next);
                out.push(Invariant::CompoundCycle { cycle });
            }
            Some(Visit::Done) => {}
            None => visit(*next, edges, state, stack, out),
        }
    }
    stack.pop();
    state.insert(node, Visit::Done);
}

/// Keeps one report per linked group.
fn dedup_linked_groups(out: &mut Vec<Invariant>) {
    let mut seen = BTreeSet::new();
    out.retain(|v| match v {
        Invariant::LinkedGroupSpansSequences { group, .. } => seen.insert(*group),
        _ => true,
    });
}

fn check_markers(path: &str, markers: &crate::Markers, out: &mut Vec<Invariant>) {
    for (key, marker) in markers {
        let at = format!("{path}/markers/{key}");
        if *key != marker.id {
            out.push(Invariant::KeyMismatch {
                path: format!("{path}/markers"),
                key: key.to_string(),
                id: marker.id.to_string(),
            });
        }
        let mut invalid = |reason: &str| {
            out.push(Invariant::InvalidValue {
                path: at.clone(),
                reason: reason.to_owned(),
            });
        };
        if marker.t < Time::ZERO {
            invalid("a marker must not be before time 0");
        }
        if let Some(d) = marker.duration {
            if d.is_zero() {
                invalid("a range marker needs a positive duration");
            } else if i128::from(marker.t.flicks()) + i128::from(d.flicks())
                > i128::from(MAX_SAFE_FLICKS)
            {
                invalid("the marker ends past the largest representable time");
            }
        }
    }
}

/// INV-6 and INV-7: at most one magnetic track, a video track, gap-free from 0.
fn check_magnetic(key: SequenceId, sequence: &Sequence, out: &mut Vec<Invariant>) {
    let mut first: Option<TrackId> = None;
    for track in sequence.tracks_in_order().filter(|t| t.magnetic) {
        let mut bad = |reason: String| {
            out.push(Invariant::MagneticTrack {
                sequence: key,
                track: track.id,
                reason,
            });
        };
        if let Some(main) = first {
            bad(format!("track `{main}` is already the magnetic track"));
        } else {
            first = Some(track.id);
        }
        if track.kind != TrackKind::Video {
            bad(format!("a {} track cannot be magnetic", track.kind));
        }
        let mut expected = 0_i128;
        for item in track.items.iter() {
            if i128::from(item.start().flicks()) != expected {
                bad(format!(
                    "item `{}` starts at {} but the track is gap-free from 0 and expects {expected}",
                    item.id(),
                    item.start().flicks()
                ));
                break;
            }
            expected = item.end_wide();
        }
    }
}

/// INV-9: attachments are one level deep, to an item of the same sequence on another track
/// whose range holds the child's start, and every member of a child's group follows.
fn check_attachments(sequence: &Sequence, out: &mut Vec<Invariant>) {
    let mut items: BTreeMap<ItemId, (TrackId, &Item)> = BTreeMap::new();
    let mut parents: BTreeSet<ItemId> = BTreeSet::new();
    for track in sequence.tracks.values() {
        for (_, item) in track.items.iter_by_id() {
            items.insert(item.id(), (track.id, item));
            parents.extend(item.attached_to());
        }
    }
    for (track_id, child) in items.values() {
        let Some(parent_id) = child.attached_to() else {
            continue;
        };
        let mut bad = |reason: String| {
            out.push(Invariant::Attachment {
                item: child.id(),
                parent: parent_id,
                reason,
            });
        };
        let Some((parent_track, parent)) = items.get(&parent_id) else {
            bad("the parent is not an item of the same sequence".into());
            continue;
        };
        if parent.attached_to().is_some() {
            bad("the parent is itself attached (attachments are one level deep)".into());
        }
        if parents.contains(&child.id()) {
            bad("the child is itself a parent (attachments are one level deep)".into());
        }
        if parent_track == track_id {
            bad("the child is on its parent's track".into());
        }
        if child.linked_group().is_some() && child.linked_group() == parent.linked_group() {
            bad("the child is linked to its own parent".into());
        }
        if child.start() < parent.start() || i128::from(child.start().flicks()) >= parent.end_wide()
        {
            bad("the child starts outside its parent's range".into());
        }
        if let Some(group) = child.linked_group()
            && let Some((_, other)) = items.values().find(|(_, i)| {
                i.linked_group() == Some(group) && i.attached_to() != Some(parent_id)
            })
        {
            bad(format!(
                "linked partner `{}` is not attached to the same parent",
                other.id()
            ));
        }
    }
}

fn check_custom(path: &str, custom: &Custom, out: &mut Vec<Invariant>) {
    for key in custom.invalid_keys() {
        out.push(Invariant::CustomNamespace {
            path: format!("{path}/custom"),
            key: key.clone(),
        });
    }
}

fn check_finite(path: &str, value: f64, out: &mut Vec<Invariant>) {
    if !value.is_finite() {
        out.push(Invariant::InvalidValue {
            path: path.to_owned(),
            reason: format!("must be a finite number, got {value}"),
        });
    }
}

/// Every animatable property against its registry row (keyframes §7, KF-4): a static value or
/// every key must be finite and within the stored range.
fn check_props(path: &str, clip: &crate::Clip, out: &mut Vec<Invariant>) {
    for id in PropId::ALL {
        let spec = id.spec();
        let checked = match clip.prop(id) {
            PropRef::Scalar(a) => a.check_range(spec.stored),
            PropRef::Vec2(a) => a.check_range(spec.stored),
            PropRef::Bool(a) => a.check_range(spec.stored),
        };
        if let Err(e) = checked {
            out.push(Invariant::InvalidValue {
                path: format!("{path}/{}", spec.path.replace('.', "/")),
                reason: e.to_string(),
            });
        }
    }
}

fn check_track_order(key: SequenceId, sequence: &crate::Sequence, out: &mut Vec<Invariant>) {
    let reason = |text: String| Invariant::TrackOrder {
        sequence: key,
        reason: text,
    };
    let mut seen = BTreeSet::new();
    for id in &sequence.track_order {
        if !seen.insert(*id) {
            out.push(reason(format!("track `{id}` is listed twice")));
        } else if !sequence.tracks.contains_key(id) {
            out.push(reason(format!("track `{id}` is listed but does not exist")));
        }
    }
    for id in sequence.tracks.keys() {
        if !seen.contains(id) {
            out.push(reason(format!("track `{id}` is missing from track_order")));
        }
    }
}

fn check_format(path: &str, format: &SequenceFormat, out: &mut Vec<Invariant>) {
    let mut invalid = |reason: String| {
        out.push(Invariant::InvalidFormat {
            path: path.to_owned(),
            reason,
        });
    };
    for (side, value) in [("width", format.width), ("height", format.height)] {
        if value == 0 || value > MAX_CANVAS_SIDE {
            invalid(format!(
                "{side} must be between 1 and {MAX_CANVAS_SIDE}, got {value}"
            ));
        }
    }
    let rate = format.frame_rate;
    let (num, den) = (u64::from(rate.num()), u64::from(rate.den()));
    if num < den || num > u64::from(MAX_SEQUENCE_FPS) * den {
        invalid(format!(
            "frame rate {rate} must be between 1 and {MAX_SEQUENCE_FPS} fps"
        ));
    } else if !rate.is_exact() {
        invalid(format!(
            "frame rate {rate} is not a whole number of flicks per frame (ADR-0004)"
        ));
    }
    let hz = format.sample_rate.hz();
    if !SEQUENCE_SAMPLE_RATES.contains(&hz) {
        invalid(format!(
            "sample rate {hz} Hz must be between {} and {} Hz",
            SEQUENCE_SAMPLE_RATES.start(),
            SEQUENCE_SAMPLE_RATES.end()
        ));
    } else if !format.sample_rate.is_exact() {
        invalid(format!(
            "sample rate {hz} Hz is not a whole number of flicks per sample (ADR-0004)"
        ));
    }
}
