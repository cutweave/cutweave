//! Document commands. Each is a params struct + `#[command]` + `impl Command`.

use std::path::Path;

use s5_bus::{
    CmdError, Command, ConflictReason, FrameRate, ItemId, MediaId, SequenceId, Time, TrackId, Tx,
    command,
    doc::{Doc, Item, MediaAsset, MediaKind, ProjectMeta, Sequence, Track, TrackKind},
    wire,
};

// ---------------------------------------------------------------------------------------------
// project.open

/// Open a new, empty in-memory project (spike stub: no file I/O). Replaces the current document
/// and clears undo history, so it is not undoable.
#[command(
    name = "project.open",
    summary = "Open a new empty project, replacing the current one",
    undoable = false,
    mcp = "core",
    annotations(destructive = true)
)]
pub struct OpenProject {
    /// Project name.
    pub name: String,
    /// Sequence frame rate (default 30/1).
    pub frame_rate: Option<FrameRate>,
    /// Frame width in pixels (default 1920).
    pub width: Option<u32>,
    /// Frame height in pixels (default 1080).
    pub height: Option<u32>,
}

/// Result of `project.open`.
#[wire]
pub struct OpenProjectOutput {
    /// The new sequence.
    pub sequence: SequenceId,
    /// Its initial video track.
    pub track: TrackId,
}

fn dimension(v: Option<u32>, default: u32, path: &str) -> Result<u32, CmdError> {
    match v.unwrap_or(default) {
        d @ 1..=16_384 => Ok(d),
        d => Err(CmdError::invalid(path, format!("{d} is outside 1..=16384"))),
    }
}

impl Command for OpenProject {
    type Output = OpenProjectOutput;

    fn apply(&self, tx: &mut Tx<'_>) -> Result<OpenProjectOutput, CmdError> {
        if self.name.trim().is_empty() {
            return Err(CmdError::invalid("/name", "must not be empty"));
        }
        let frame_rate = self.frame_rate.unwrap_or(FrameRate::new(30, 1));
        frame_rate
            .validate()
            .map_err(|m| CmdError::invalid("/frame_rate", m))?;
        let width = dimension(self.width, 1920, "/width")?;
        let height = dimension(self.height, 1080, "/height")?;

        let doc = tx.doc();
        let items: Vec<ItemId> = doc.items.keys().cloned().collect();
        let tracks: Vec<TrackId> = doc.tracks.keys().cloned().collect();
        let media: Vec<MediaId> = doc.media.keys().cloned().collect();
        for id in &items {
            tx.remove::<Item>(id)?;
        }
        for id in &tracks {
            tx.remove::<Track>(id)?;
        }
        for id in &media {
            tx.remove::<MediaAsset>(id)?;
        }
        let track = Track {
            id: tx.new_id(),
            kind: TrackKind::Video,
            name: "V1".into(),
        };
        let sequence = Sequence {
            id: tx.new_id(),
            name: "Main".into(),
            frame_rate,
            width,
            height,
            tracks: vec![track.id.clone()],
        };
        let out = OpenProjectOutput {
            sequence: sequence.id.clone(),
            track: track.id.clone(),
        };
        tx.insert(track);
        tx.update_sequence(|s| *s = sequence);
        tx.set_meta(ProjectMeta {
            name: self.name.trim().into(),
        });
        Ok(out)
    }
}

// ---------------------------------------------------------------------------------------------
// media.import

/// Import a media file by reference. Spike stub: nothing is probed; the kind comes from the file
/// extension and duration/frame rate come from the optional overrides.
#[command(
    name = "media.import",
    summary = "Import a media file into the project",
    undoable,
    mcp = "core",
    annotations(destructive = false)
)]
pub struct ImportMedia {
    /// File path (absolute or project-relative).
    pub path: String,
    /// Stub probe override: duration (default 10 s).
    pub duration: Option<Time>,
    /// Stub probe override: frame rate (default 30/1).
    pub frame_rate: Option<FrameRate>,
}

/// Result of `media.import`.
#[wire]
pub struct ImportMediaOutput {
    /// The new media id.
    pub media: MediaId,
    /// Detected kind.
    pub kind: MediaKind,
    /// Duration.
    pub duration: Time,
}

fn kind_from_path(path: &str) -> Option<MediaKind> {
    let ext = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "mp4" | "mov" | "mkv" | "webm" => Some(MediaKind::Video),
        "wav" | "mp3" | "aac" | "flac" => Some(MediaKind::Audio),
        "png" | "jpg" | "jpeg" => Some(MediaKind::Image),
        _ => None,
    }
}

impl Command for ImportMedia {
    type Output = ImportMediaOutput;

    fn apply(&self, tx: &mut Tx<'_>) -> Result<ImportMediaOutput, CmdError> {
        let path = self.path.trim();
        if path.is_empty() {
            return Err(CmdError::invalid("/path", "must not be empty"));
        }
        let kind = kind_from_path(path).ok_or_else(|| {
            CmdError::invalid(
                "/path",
                "unsupported file type (spike accepts mp4/mov/mkv/webm, wav/mp3/aac/flac, png/jpg)",
            )
        })?;
        let duration = self.duration.unwrap_or(Time::from_secs(10));
        if duration <= Time::ZERO {
            return Err(CmdError::invalid("/duration", "must be positive"));
        }
        let frame_rate = self.frame_rate.unwrap_or(FrameRate::new(30, 1));
        frame_rate
            .validate()
            .map_err(|m| CmdError::invalid("/frame_rate", m))?;
        let media = MediaAsset {
            id: tx.new_id(),
            path: path.into(),
            kind,
            duration,
            frame_rate,
        };
        let out = ImportMediaOutput {
            media: media.id.clone(),
            kind,
            duration,
        };
        tx.insert(media);
        Ok(out)
    }
}

// ---------------------------------------------------------------------------------------------
// timeline.insert

/// Insert media as a clip. If the target range is occupied, a new track is created next to the
/// target (above for video, below for audio) instead of overlapping (arch §4.3).
#[command(
    name = "timeline.insert",
    summary = "Insert media onto the timeline at a time",
    undoable,
    mcp = "core",
    annotations(destructive = false)
)]
pub struct InsertClip {
    /// Media to insert.
    pub media: MediaId,
    /// Timeline start.
    pub at: Time,
    /// Target track (default: the main track of the matching kind).
    pub track: Option<TrackId>,
    /// Source in-point (default 0).
    pub source_in: Option<Time>,
    /// Clip duration (default: rest of the media).
    pub duration: Option<Time>,
}

/// Result of `timeline.insert`.
#[wire]
pub struct InsertClipOutput {
    /// The new item.
    pub item: ItemId,
    /// Track it landed on.
    pub track: TrackId,
    /// Whether a track was created to avoid an overlap.
    pub created_track: bool,
}

fn track_kind_for(media: MediaKind) -> TrackKind {
    match media {
        MediaKind::Audio => TrackKind::Audio,
        MediaKind::Video | MediaKind::Image => TrackKind::Video,
    }
}

/// Main track of a kind: the bottom-most video track, or the top-most audio track.
fn main_track(doc: &Doc, kind: TrackKind) -> Option<TrackId> {
    let mut of_kind = doc
        .sequence
        .tracks
        .iter()
        .filter(|t| doc.tracks.get(*t).is_some_and(|tr| tr.kind == kind));
    match kind {
        TrackKind::Video => of_kind.next_back().cloned(),
        TrackKind::Audio => of_kind.next().cloned(),
    }
}

fn create_track(tx: &mut Tx<'_>, kind: TrackKind, near: Option<&TrackId>) -> TrackId {
    let doc = tx.doc();
    let n = doc.tracks.values().filter(|t| t.kind == kind).count() + 1;
    let order = &doc.sequence.tracks;
    let first_audio = order
        .iter()
        .position(|t| {
            doc.tracks
                .get(t)
                .is_some_and(|tr| tr.kind == TrackKind::Audio)
        })
        .unwrap_or(order.len());
    let pos = match (kind, near.and_then(|t| order.iter().position(|x| x == t))) {
        (TrackKind::Video, Some(p)) => p,
        (TrackKind::Audio, Some(p)) => p + 1,
        (TrackKind::Video, None) => first_audio,
        (TrackKind::Audio, None) => order.len(),
    };
    let prefix = match kind {
        TrackKind::Video => "V",
        TrackKind::Audio => "A",
    };
    let id: TrackId = tx.new_id();
    tx.insert(Track {
        id: id.clone(),
        kind,
        name: format!("{prefix}{n}"),
    });
    tx.update_sequence(|s| s.tracks.insert(pos, id.clone()));
    id
}

impl Command for InsertClip {
    type Output = InsertClipOutput;

    fn apply(&self, tx: &mut Tx<'_>) -> Result<InsertClipOutput, CmdError> {
        let media = tx.get::<MediaAsset>(&self.media)?.clone();
        if self.at < Time::ZERO {
            return Err(CmdError::invalid("/at", "must be >= 0"));
        }
        let source_in = self.source_in.unwrap_or(Time::ZERO);
        if source_in < Time::ZERO || source_in >= media.duration {
            return Err(CmdError::invalid("/source_in", "must be within the media"));
        }
        let duration = self.duration.unwrap_or(media.duration - source_in);
        if duration <= Time::ZERO {
            return Err(CmdError::invalid("/duration", "must be positive"));
        }
        if source_in + duration > media.duration {
            return Err(CmdError::invalid(
                "/duration",
                "runs past the end of the media",
            ));
        }
        let kind = track_kind_for(media.kind);
        let target = match &self.track {
            Some(t) => {
                if tx.get::<Track>(t)?.kind != kind {
                    return Err(CmdError::invalid(
                        "/track",
                        format!("a {:?} clip cannot go on that track", media.kind),
                    ));
                }
                Some(t.clone())
            }
            None => main_track(tx.doc(), kind),
        };
        let end = self.at + duration;
        let (track, created_track) = match target {
            Some(t) if tx.doc().range_is_free(&t, self.at, end, &[]) => (t, false),
            near => (create_track(tx, kind, near.as_ref()), true),
        };
        let id: ItemId = tx.new_id();
        tx.insert(Item {
            id: id.clone(),
            track: track.clone(),
            media: media.id,
            start: self.at,
            duration,
            source_in,
        });
        Ok(InsertClipOutput {
            item: id,
            track,
            created_track,
        })
    }
}

// ---------------------------------------------------------------------------------------------
// clip.split

/// Split a clip in two at a timeline time. The left part keeps the id.
#[command(
    name = "clip.split",
    summary = "Split a clip at a timeline time",
    undoable,
    mcp = "core",
    annotations(destructive = false, idempotent = false)
)]
pub struct SplitClip {
    /// Clip to split.
    pub item: ItemId,
    /// Split point; must be strictly inside the clip.
    pub at: Time,
}

/// Result of `clip.split`.
#[wire]
pub struct SplitClipOutput {
    /// Left part (same id as the input).
    pub left: ItemId,
    /// New right part.
    pub right: ItemId,
}

impl Command for SplitClip {
    type Output = SplitClipOutput;

    fn apply(&self, tx: &mut Tx<'_>) -> Result<SplitClipOutput, CmdError> {
        let item = tx.get::<Item>(&self.item)?.clone();
        if !(item.start < self.at && self.at < item.end()) {
            return Err(CmdError::invalid(
                "/at",
                format!(
                    "must be strictly inside the clip ({}..{} flicks)",
                    item.start.flicks(),
                    item.end().flicks()
                ),
            ));
        }
        let left_duration = self.at - item.start;
        tx.update::<Item>(&item.id, |i| {
            i.duration = left_duration;
            Ok(())
        })?;
        let right: ItemId = tx.new_id();
        tx.insert(Item {
            id: right.clone(),
            track: item.track.clone(),
            media: item.media.clone(),
            start: self.at,
            duration: item.duration - left_duration,
            source_in: item.source_in + left_duration,
        });
        Ok(SplitClipOutput {
            left: item.id,
            right,
        })
    }
}

// ---------------------------------------------------------------------------------------------
// clip.trim

/// Move a clip's in-point and/or out-point on the timeline. The source in-point follows the new
/// start; trimming past the media bounds or into a neighbour fails.
#[command(
    name = "clip.trim",
    summary = "Trim a clip's start and/or end on the timeline",
    undoable,
    mcp = "core",
    annotations(destructive = false, idempotent = true)
)]
pub struct TrimClip {
    /// Clip to trim.
    pub item: ItemId,
    /// New timeline start.
    pub start: Option<Time>,
    /// New timeline end (exclusive).
    pub end: Option<Time>,
}

/// Result of `clip.trim`.
#[wire]
pub struct TrimClipOutput {
    /// The clip.
    pub item: ItemId,
    /// Its new start.
    pub start: Time,
    /// Its new end.
    pub end: Time,
}

impl Command for TrimClip {
    type Output = TrimClipOutput;

    fn apply(&self, tx: &mut Tx<'_>) -> Result<TrimClipOutput, CmdError> {
        if self.start.is_none() && self.end.is_none() {
            return Err(CmdError::invalid("", "provide `start` and/or `end`"));
        }
        let item = tx.get::<Item>(&self.item)?.clone();
        let media_duration = tx.get::<MediaAsset>(&item.media)?.duration;
        let start = self.start.unwrap_or(item.start);
        let end = self.end.unwrap_or(item.end());
        if start < Time::ZERO {
            return Err(CmdError::invalid("/start", "must be >= 0"));
        }
        if end <= start {
            let path = if self.end.is_some() { "/end" } else { "/start" };
            return Err(CmdError::invalid(path, "end must be after start"));
        }
        let source_in = item.source_in + (start - item.start);
        if source_in < Time::ZERO {
            return Err(CmdError::invalid(
                "/start",
                "trims before the start of the media",
            ));
        }
        if source_in + (end - start) > media_duration {
            return Err(CmdError::invalid("/end", "trims past the end of the media"));
        }
        if !tx.doc().range_is_free(&item.track, start, end, &[&item.id]) {
            return Err(CmdError::conflict(
                ConflictReason::Overlap,
                format!(
                    "trimmed range would overlap another item on track {}",
                    item.track
                ),
            ));
        }
        tx.update::<Item>(&item.id, |i| {
            i.start = start;
            i.duration = end - start;
            i.source_in = source_in;
            Ok(())
        })?;
        Ok(TrimClipOutput {
            item: item.id,
            start,
            end,
        })
    }
}

// ---------------------------------------------------------------------------------------------
// item.delete

/// Delete timeline items (all or nothing).
#[command(
    name = "item.delete",
    summary = "Delete timeline items",
    undoable,
    mcp = "core",
    annotations(destructive = true, idempotent = false)
)]
pub struct DeleteItems {
    /// Items to delete (at least one, no duplicates).
    pub items: Vec<ItemId>,
}

/// Result of `item.delete`.
#[wire]
pub struct DeleteItemsOutput {
    /// Deleted ids.
    pub deleted: Vec<ItemId>,
}

impl Command for DeleteItems {
    type Output = DeleteItemsOutput;

    fn apply(&self, tx: &mut Tx<'_>) -> Result<DeleteItemsOutput, CmdError> {
        if self.items.is_empty() {
            return Err(CmdError::invalid("/items", "provide at least one item"));
        }
        for (i, id) in self.items.iter().enumerate() {
            if self.items[..i].contains(id) {
                return Err(CmdError::invalid(format!("/items/{i}"), "duplicate id"));
            }
            tx.remove::<Item>(id)?;
        }
        Ok(DeleteItemsOutput {
            deleted: self.items.clone(),
        })
    }
}
