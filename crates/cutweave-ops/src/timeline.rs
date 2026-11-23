// SPDX-License-Identifier: GPL-3.0-or-later
//! The `timeline` namespace (WP-1B.05; arch §4.3, §5.1): `timeline.insert`, `timeline.get`.
//!
//! **Insert semantics.** `timeline.insert` places a media clip at a time on a track. Placing it
//! where it would overlap another clip is `Invalid{path:"/at"}` by default; with
//! `overlap: "new_track"` a new track of the same kind is created directly above the target and
//! the clip goes there (arch §4.3). There is no ripple in P1 (the magnetic track is P2).
//!
//! **Linked audio.** Inserting a video file that has an audio stream onto a video track also
//! places its audio on the first unlocked audio track that is free over the range, and gives
//! both clips the same `linked_group`. Without such a track the video clip is inserted alone
//! (with `overlap: "new_track"` and an unlocked audio track that is busy, a new audio track is
//! created above it). `link: false` inserts the picture only; inserting onto an audio track
//! inserts the sound only.

use cutweave_commands::{
    CmdError, Command, Query, QueryCtx, Registry, RegistryError, Tx, command, query, wire,
};
use cutweave_model::{
    Clip, Duration, EntityKind, GroupId, Item, ItemId, MediaAsset, MediaId, MediaKind, Project,
    Sequence, SequenceFormat, SequenceId, Time, TimeInput, Track, TrackId, TrackKind,
};

use crate::{
    media::has_audio_stream,
    track::{check_editable_kind, next_track_name},
    util::{resolve_frame_duration, resolve_frame_time, resolve_time},
};

/// Length of a still image when `duration` is omitted: five seconds.
pub const DEFAULT_STILL_SECS: i64 = 5;

/// Registers the `timeline.*` commands and queries.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    registry
        .register::<TimelineInsert>()?
        .register::<TimelineGet>()?;
    Ok(())
}

// ----- timeline.insert -----------------------------------------------------------------------

/// What to do when the clip would overlap another clip on the target track.
#[wire]
#[derive(Copy, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum OverlapPolicy {
    /// Fail with `Invalid{path:"/at"}` (default).
    #[default]
    Reject,
    /// Create a new track of the same kind directly above the target and insert there.
    NewTrack,
}

/// Result of `timeline.insert`.
#[wire]
pub struct TimelineInsertOutput {
    /// The start that was used, snapped to the frame grid.
    pub at: Time,
    /// The clip length that was used, a whole number of frames.
    pub duration: Duration,
    /// The inserted clips: the one on the target track first, then the linked audio clip.
    pub clips: Vec<ItemId>,
    /// The group shared by the clips when audio was linked.
    pub linked_group: Option<GroupId>,
    /// Tracks created by `overlap: "new_track"`, in creation order.
    pub created_tracks: Vec<TrackId>,
}

/// Inserts a media clip on a track at a time. `source_in` is the position in the media the clip
/// starts from and `duration` its length (defaults: the start of the media and the rest of it;
/// stills default to five seconds). Times accept flicks, `"1.5s"`, `"f:45"` and timecodes,
/// resolved against the sequence's frame rate. `at` and `duration` snap to the frame grid
/// (nearest frame, ties later; at least one frame) and the result states the values used;
/// `source_in` is a position in the media and is not snapped, nor is a default duration
/// (the rest of the media, or five seconds for a still). See the module docs for overlap and linked
/// audio behaviour. Fails with `Locked` if the target track is locked.
#[command(
    name = "timeline.insert",
    summary = "Insert a media clip at a time on a track (adds linked audio for video with sound)",
    undoable,
    mcp = "core",
    annotations(destructive = false, idempotent = false)
)]
pub struct TimelineInsert {
    /// The sequence.
    pub sequence: SequenceId,
    /// The target track (of that sequence): a video track for pictures, an audio track for
    /// sound.
    pub track: TrackId,
    /// The media to place.
    pub media: MediaId,
    /// Timeline position of the clip's first frame, `>= 0` (snapped to the frame grid).
    pub at: TimeInput,
    /// Position in the media to start from (default 0).
    pub source_in: Option<TimeInput>,
    /// Clip length, `> 0` (snapped to whole frames, at least one).
    pub duration: Option<TimeInput>,
    /// Overlap handling (default `reject`).
    pub overlap: Option<OverlapPolicy>,
    /// Also insert the audio of a video file as a linked clip (default `true`).
    pub link: Option<bool>,
}

/// Where a clip goes: an existing track, or a new one created directly above `above`.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Slot {
    Existing(TrackId),
    New { above: TrackId },
}

struct Plan {
    at: Time,
    source_in: Time,
    duration: Duration,
    main_kind: TrackKind,
    main: Slot,
    audio: Option<Slot>,
}

pub(crate) fn overlapping(track: &Track, start: Time, end: Time) -> Option<ItemId> {
    track.items.overlapping(start, end).next().map(Item::id)
}

impl TimelineInsert {
    /// The resolved `(at, source_in, duration)`, checked against the sequence rate and the
    /// media bounds.
    fn range(
        &self,
        sequence: &Sequence,
        media: &MediaAsset,
    ) -> Result<(Time, Time, Duration), CmdError> {
        let rate = sequence.format.frame_rate;
        let at = resolve_frame_time("/at", self.at, rate)?;
        if at < Time::ZERO {
            return Err(CmdError::invalid("/at", "the time must not be negative"));
        }
        let source_in = match self.source_in {
            Some(t) => resolve_time("/source_in", t, rate)?,
            None => Time::ZERO,
        };
        if source_in < Time::ZERO {
            return Err(CmdError::invalid(
                "/source_in",
                "the source position must not be negative",
            ));
        }
        let bounded = media
            .probe
            .duration
            .filter(|_| media.kind != MediaKind::Image);
        let duration = match (self.duration, bounded) {
            (Some(d), _) => resolve_frame_duration("/duration", d, rate)?,
            // Defaults derived from the media are not snapped: a clip that plays the rest of
            // its media ends exactly where the media does.
            (None, Some(total)) => total
                .flicks()
                .checked_sub(source_in.flicks())
                .and_then(|f| Duration::from_flicks(f).ok())
                .filter(|d| !d.is_zero())
                .ok_or_else(|| {
                    CmdError::invalid(
                        "/source_in",
                        "the source position is at or past the media end",
                    )
                })?,
            (None, None) if media.kind == MediaKind::Image => {
                Duration::from_secs(DEFAULT_STILL_SECS)
                    .map_err(|e| CmdError::internal(e.to_string()))?
            }
            (None, None) => {
                return Err(CmdError::invalid(
                    "/duration",
                    "the media length is unknown, so a duration is required",
                ));
            }
        };
        if let Some(total) = bounded
            && i128::from(source_in.flicks()) + i128::from(duration.flicks())
                > i128::from(total.flicks())
        {
            return Err(CmdError::invalid(
                "/duration",
                "the clip would read past the end of the media",
            ));
        }
        Ok((at, source_in, duration))
    }

    fn plan(&self, doc: &Project) -> Result<Plan, CmdError> {
        let sequence = doc
            .sequences
            .get(&self.sequence)
            .ok_or_else(|| CmdError::not_found(EntityKind::Sequence, self.sequence))?;
        let track = match sequence.track(&self.track) {
            Some(t) => t,
            None if doc
                .sequences
                .values()
                .any(|s| s.track(&self.track).is_some()) =>
            {
                return Err(CmdError::invalid(
                    "/track",
                    format!(
                        "track `{}` is not in sequence `{}`",
                        self.track, self.sequence
                    ),
                ));
            }
            None => return Err(CmdError::not_found(EntityKind::Track, self.track)),
        };
        let media = doc
            .media
            .get(&self.media)
            .ok_or_else(|| CmdError::not_found(EntityKind::Media, self.media))?;
        check_editable_kind("/track", track.kind)?;
        if media.kind == MediaKind::SequenceRef {
            return Err(CmdError::unsupported(
                "compound clips (sequence media) cannot be inserted yet",
            ));
        }
        if !track.kind.accepts(media.kind) {
            return Err(CmdError::invalid(
                "/track",
                format!("a {} track cannot play {} media", track.kind, media.kind),
            ));
        }
        if track.locked {
            return Err(CmdError::locked(EntityKind::Track, track.id));
        }
        let (at, source_in, duration) = self.range(sequence, media)?;
        let end = at
            .checked_add(duration)
            .map_err(|e| CmdError::invalid("/at", e.to_string()))?;
        let policy = self.overlap.unwrap_or_default();

        let main = match overlapping(track, at, end) {
            None => Slot::Existing(track.id),
            Some(other) => match policy {
                OverlapPolicy::Reject => {
                    return Err(CmdError::invalid(
                        "/at",
                        format!(
                            "the clip would overlap item `{other}` on track `{}`; choose a free \
                             time or use overlap \"new_track\"",
                            track.id
                        ),
                    ));
                }
                OverlapPolicy::NewTrack => Slot::New { above: track.id },
            },
        };
        let audio = if track.kind == TrackKind::Video
            && self.link != Some(false)
            && has_audio_stream(media)
        {
            audio_partner(sequence, at, end, policy)
        } else {
            None
        };
        Ok(Plan {
            at,
            source_in,
            duration,
            main_kind: track.kind,
            main,
            audio,
        })
    }
}

/// The audio track for the linked clip: the first unlocked one that is free over the range, or
/// (under `new_track`) a new one above the first unlocked one.
fn audio_partner(sequence: &Sequence, at: Time, end: Time, policy: OverlapPolicy) -> Option<Slot> {
    let mut unlocked = sequence
        .tracks_in_order()
        .filter(|t| t.kind == TrackKind::Audio && !t.locked)
        .peekable();
    let first = unlocked.peek().map(|t| t.id)?;
    if let Some(free) = unlocked.find(|t| overlapping(t, at, end).is_none()) {
        return Some(Slot::Existing(free.id));
    }
    (policy == OverlapPolicy::NewTrack).then_some(Slot::New { above: first })
}

fn clip(id: ItemId, media: &MediaAsset, plan: &Plan, group: Option<GroupId>) -> Item {
    let mut clip = Clip::new(id, media.id, plan.at, plan.duration);
    clip.source_in = plan.source_in;
    clip.linked_group = group;
    clip.into()
}

pub(crate) fn materialize(
    tx: &mut Tx<'_>,
    sequence: SequenceId,
    slot: Slot,
    kind: TrackKind,
    created: &mut Vec<TrackId>,
) -> Result<TrackId, CmdError> {
    match slot {
        Slot::Existing(id) => Ok(id),
        Slot::New { above } => {
            let seq = tx.sequence(sequence)?;
            let index = seq.track_order.index_of(&above).unwrap_or(0);
            let name = next_track_name(seq, kind);
            let track = Track::new(tx.new_id(), kind, name);
            tx.insert_track(sequence, index, &track)?;
            created.push(track.id);
            Ok(track.id)
        }
    }
}

impl Command for TimelineInsert {
    type Output = TimelineInsertOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        self.plan(ctx.doc()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<TimelineInsertOutput, CmdError> {
        let plan = self.plan(tx.doc())?;
        let media = tx.media(self.media)?.clone();
        let group = plan.audio.map(|_| tx.new_id::<GroupId>());
        let mut created = Vec::new();
        let main = materialize(tx, self.sequence, plan.main, plan.main_kind, &mut created)?;
        let main_id: ItemId = tx.new_id();
        tx.insert_item(main, &clip(main_id, &media, &plan, group))?;
        let mut clips = vec![main_id];
        if let Some(slot) = plan.audio {
            let audio = materialize(tx, self.sequence, slot, TrackKind::Audio, &mut created)?;
            let id: ItemId = tx.new_id();
            tx.insert_item(audio, &clip(id, &media, &plan, group))?;
            clips.push(id);
        }
        Ok(TimelineInsertOutput {
            at: plan.at,
            duration: plan.duration,
            clips,
            linked_group: group,
            created_tracks: created,
        })
    }
}

// ----- timeline.get --------------------------------------------------------------------------

/// Deepest `depth` of `timeline.get`.
pub const MAX_GET_DEPTH: u8 = 2;

/// One item of a track (depth 2).
#[wire]
pub struct ItemSummary {
    /// The item.
    pub id: ItemId,
    /// Item kind (`clip`).
    pub kind: String,
    /// Timeline position of the first frame.
    pub start: Time,
    /// Length on the timeline.
    pub duration: Duration,
    /// The media a clip plays.
    pub media: Option<MediaId>,
    /// The linked group, if any.
    pub linked_group: Option<GroupId>,
    /// Whether the item is rendered and heard.
    pub enabled: bool,
}

/// One track (depth 1 and 2).
#[wire]
#[allow(clippy::struct_excessive_bools)] // mirrors the track's independent toggles
pub struct TrackSummary {
    /// The track.
    pub id: TrackId,
    /// What it holds.
    pub kind: TrackKind,
    /// Display name.
    pub name: String,
    /// Silenced.
    pub muted: bool,
    /// Not rendered.
    pub hidden: bool,
    /// Item edits fail with `Locked`.
    pub locked: bool,
    /// Solo.
    pub solo: bool,
    /// Gain in dB.
    pub gain: f64,
    /// Number of items.
    pub item_count: u32,
    /// End of its last item.
    pub end: Time,
    /// The items in start order (depth 2 only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<Vec<ItemSummary>>,
}

/// Result of `timeline.get`.
#[wire]
pub struct TimelineGetOutput {
    /// The sequence.
    pub id: SequenceId,
    /// Its name (unless `fields` leaves it out).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Its format (unless `fields` leaves it out).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<SequenceFormat>,
    /// End of the last item on any track (unless `fields` leaves it out).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<Duration>,
    /// Number of tracks.
    pub track_count: u32,
    /// Number of items on all tracks.
    pub item_count: u32,
    /// The tracks in display order, top to bottom (depth >= 1, unless `fields` leaves them
    /// out).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tracks: Option<Vec<TrackSummary>>,
}

/// Sections `timeline.get` can be limited to with `fields`.
pub const GET_FIELDS: [&str; 4] = ["name", "format", "duration", "tracks"];

/// Reads a sequence in layers: `depth` 0 is the sequence summary, 1 adds its tracks (summaries
/// only, small), 2 adds every item. `fields` limits the sections returned (`name`, `format`,
/// `duration`, `tracks`); the id and the counts are always included.
#[query(
    name = "timeline.get",
    summary = "Get a sequence: summary (depth 0), tracks (1) or tracks with items (2)",
    mcp = "core"
)]
pub struct TimelineGet {
    /// The sequence.
    pub sequence: SequenceId,
    /// Detail level `0..=2` (default 1).
    pub depth: Option<u8>,
    /// Sections to include: any of `name`, `format`, `duration`, `tracks` (default all).
    pub fields: Option<Vec<String>>,
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

pub(crate) fn item_summary(item: &Item) -> ItemSummary {
    ItemSummary {
        id: item.id(),
        kind: match item {
            Item::Clip(_) => "clip",
            _ => "other",
        }
        .to_owned(),
        start: item.start(),
        duration: item.duration(),
        media: item.media(),
        linked_group: item.linked_group(),
        enabled: item.enabled(),
    }
}

impl Query for TimelineGet {
    type Output = TimelineGetOutput;

    fn run(&self, ctx: &QueryCtx<'_>) -> Result<TimelineGetOutput, CmdError> {
        let sequence = ctx.sequence(self.sequence)?;
        let depth = self.depth.unwrap_or(1);
        if depth > MAX_GET_DEPTH {
            return Err(CmdError::invalid(
                "/depth",
                format!("the depth must be 0..={MAX_GET_DEPTH}"),
            ));
        }
        let wants = |name: &str| {
            self.fields
                .as_ref()
                .is_none_or(|f| f.iter().any(|x| x == name))
        };
        if let Some(fields) = &self.fields {
            for (i, f) in fields.iter().enumerate() {
                if !GET_FIELDS.contains(&f.as_str()) {
                    return Err(CmdError::invalid(
                        format!("/fields/{i}"),
                        format!("unknown field `{f}`; use {}", GET_FIELDS.join(", ")),
                    ));
                }
            }
        }
        let tracks = (depth >= 1 && wants("tracks")).then(|| {
            sequence
                .tracks_in_order()
                .map(|t| TrackSummary {
                    id: t.id,
                    kind: t.kind,
                    name: t.name.clone(),
                    muted: t.muted,
                    hidden: t.hidden,
                    locked: t.locked,
                    solo: t.solo,
                    gain: t.gain,
                    item_count: count(t.items.len()),
                    end: t.items.end(),
                    items: (depth >= 2).then(|| t.items.iter().map(item_summary).collect()),
                })
                .collect()
        });
        Ok(TimelineGetOutput {
            id: sequence.id,
            name: wants("name").then(|| sequence.name.clone()),
            format: wants("format").then(|| sequence.format.clone()),
            duration: wants("duration").then(|| sequence.duration()),
            track_count: count(sequence.tracks.len()),
            item_count: count(sequence.tracks.values().map(|t| t.items.len()).sum()),
            tracks,
        })
    }
}
