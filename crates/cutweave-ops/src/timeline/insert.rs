// SPDX-License-Identifier: GPL-3.0-or-later
//! `timeline.insert` and `timeline.overwrite` (WP-1B.05, WP-2A.03; timeline spec §4.1, §3.6,
//! INV-40, INV-41, INV-45, INV-101 to INV-104; ADR-0023).
//!
//! The clip is placed with a **mode** (`mode`, or its P1 alias `overlap`): `insert` splits the
//! item under `at` and ripples the target track (and the tracks of linked partners of what
//! shifts) by the clip's length; `overwrite` cuts the range out of the target track; `new_track`
//! goes to a new track directly above when the range is busy; `reject` fails then. The default
//! is `insert` on the magnetic main track (where `at` is clamped into `[0, end]`) and `reject`
//! elsewhere (P1). `timeline.overwrite` is `timeline.insert` with mode `overwrite` (D13).
//!
//! **Linked audio** (INV-45): a video file with sound placed on a video track (unless
//! `link: false`) also places its sound, linked, on `audio_track` (which must then be free), or
//! on the first unlocked audio track free over the range after the command's own ripple and
//! cuts, or (`insert`, `overwrite`, `new_track`) on a new audio track above the first unlocked
//! one; otherwise the picture goes alone.
//!
//! **IDs** (INV-23): the splits of the insert or the cuts of the overwrite first (see
//! [`super::placement`]), then the P1 order: the clips' group (with audio), the new track of the
//! picture (if any), the picture, the new audio track (if any), the sound.

use cutweave_commands::{CmdError, Command, QueryCtx, Tx, command, wire};
use cutweave_model::{
    Clip, Duration, EntityKind, GroupId, Item, ItemId, MediaAsset, MediaId, MediaKind, Project,
    Rounding, Sequence, SequenceId, Time, TimeInput, TrackId, TrackKind,
};

use super::{
    DEFAULT_STILL_SECS, OverlapPolicy,
    magnetic::clamp_to_end,
    placement::{DryIds, Edit, Ids, PlacementMode, Plan, resolve_mode, time},
};
use crate::{
    media::has_audio_stream,
    report::EditReport,
    track::check_editable_kind,
    util::{resolve_frame_duration, resolve_frame_time, resolve_time},
};

/// Result of `timeline.insert` and `timeline.overwrite`.
#[wire]
pub struct TimelineInsertOutput {
    /// The start that was used: snapped to the frame grid, clamped on the magnetic track.
    pub at: Time,
    /// The clip length that was used, a whole number of frames.
    pub duration: Duration,
    /// The inserted clips: the one on the target track first, then the linked audio clip.
    pub clips: Vec<ItemId>,
    /// The group shared by the clips when audio was linked.
    pub linked_group: Option<GroupId>,
    /// Tracks created by the placement, in creation order.
    pub created_tracks: Vec<TrackId>,
    /// What changed (timeline spec §3.1).
    pub report: EditReport,
}

/// Inserts a media clip on a track at a time. `source_in` is the position in the media the clip
/// starts from and `duration` its length (defaults: the start of the media and the rest of it,
/// floored to whole frames; stills last five seconds). Times accept flicks, `"1.5s"`, `"f:45"`
/// and timecodes, snapped to the sequence's frame grid (the result states the values used;
/// `source_in` is a media position and is not snapped). `mode` decides what happens to what is
/// already there: `insert` (default on the magnetic main track) splits and ripples, `overwrite`
/// cuts the range out, `new_track` goes to a new track above, `reject` (default elsewhere)
/// fails; `overlap` is the P1 alias of `mode`. A video with sound also places its audio,
/// linked. Fails with `Locked` if the placement would change a locked track.
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
    /// The target track: video for pictures, audio for sound.
    pub track: TrackId,
    /// The media to place.
    pub media: MediaId,
    /// Timeline position of the clip's first frame, `>= 0`.
    pub at: TimeInput,
    /// Position in the media to start from (default 0).
    pub source_in: Option<TimeInput>,
    /// Clip length, `> 0` (default: the rest of the media).
    pub duration: Option<TimeInput>,
    /// Alias of `mode`.
    pub overlap: Option<OverlapPolicy>,
    /// Also place a video's sound, linked (default `true`).
    pub link: Option<bool>,
    /// Default: `insert` on the main track, else `reject`.
    pub mode: Option<PlacementMode>,
    /// Track for the linked sound.
    pub audio_track: Option<TrackId>,
    /// `insert` ripples every track.
    pub ripple_all_tracks: Option<bool>,
}

/// Overwrites a range of a track with a media clip: `timeline.insert` with mode `overwrite`.
/// Whatever is under `[at, at + duration)` on the track is cut out first (items inside are
/// removed, items across an edge are trimmed, an item across the whole range is split around
/// it), and their linked partners get the same cut. Nothing ripples. On the magnetic main track
/// `at` is clamped to its end.
#[command(
    name = "timeline.overwrite",
    summary = "Place a media clip over a range of a track, cutting out what is there",
    undoable,
    mcp = "catalog",
    annotations(destructive = true, idempotent = false)
)]
pub struct TimelineOverwrite {
    /// The sequence.
    pub sequence: SequenceId,
    /// The target track.
    pub track: TrackId,
    /// The media to place.
    pub media: MediaId,
    /// Timeline position of the clip's first frame, `>= 0`.
    pub at: TimeInput,
    /// Position in the media to start from (default 0).
    pub source_in: Option<TimeInput>,
    /// Clip length, `> 0` (default: the rest of the media).
    pub duration: Option<TimeInput>,
    /// Also place the audio of a video file as a linked clip (default `true`).
    pub link: Option<bool>,
    /// The audio track for the linked sound (default: the first unlocked free one).
    pub audio_track: Option<TrackId>,
}

impl TimelineOverwrite {
    fn as_insert(&self) -> TimelineInsert {
        TimelineInsert {
            sequence: self.sequence,
            track: self.track,
            media: self.media,
            at: self.at,
            source_in: self.source_in,
            duration: self.duration,
            overlap: None,
            link: self.link,
            mode: Some(PlacementMode::Overwrite),
            audio_track: self.audio_track,
            ripple_all_tracks: None,
        }
    }
}

/// Where a clip goes: an existing track, or a new one created directly above `above`.
#[derive(Clone, Copy, Debug)]
enum Slot {
    Existing(TrackId),
    New { above: TrackId },
}

/// The resolved `(at, source_in, duration)` in flicks, checked against the sequence rate and
/// the media bounds (INV-24, INV-25).
fn range(
    cmd: &TimelineInsert,
    sequence: &Sequence,
    media: &MediaAsset,
) -> Result<(i64, Time, Duration), CmdError> {
    let rate = sequence.format.frame_rate;
    let at = resolve_frame_time("/at", cmd.at, rate)?;
    if at < Time::ZERO {
        return Err(CmdError::invalid("/at", "the time must not be negative"));
    }
    let source_in = match cmd.source_in {
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
    let whole_frames = |flicks: i64| -> Result<Duration, CmdError> {
        let frames = rate.time_to_frame(time(flicks), Rounding::Floor);
        if frames < 1 {
            return Err(CmdError::invalid(
                "/source_in",
                "less than one frame of media is left after the source position",
            ));
        }
        rate.frame_to_time(frames)
            .ok()
            .and_then(|t| Duration::try_from(t).ok())
            .ok_or_else(|| CmdError::invalid("/duration", "the clip would be too long"))
    };
    let duration = match (cmd.duration, bounded) {
        (Some(d), _) => resolve_frame_duration("/duration", d, rate)?,
        // INV-25: the rest of the media, floored to whole frames (never past its end).
        (None, Some(total)) => whole_frames(total.flicks() - source_in.flicks())?,
        (None, None) if media.kind == MediaKind::Image => {
            let five = Time::from_secs(DEFAULT_STILL_SECS)
                .map_err(|e| CmdError::internal(e.to_string()))?;
            let frames = rate.time_to_frame(five, Rounding::Nearest).max(1);
            rate.frame_to_time(frames)
                .ok()
                .and_then(|t| Duration::try_from(t).ok())
                .ok_or_else(|| CmdError::internal("a still's default length overflows"))?
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
    Ok((at.flicks(), source_in, duration))
}

struct Planned {
    plan: Plan,
    at: i64,
    duration: Duration,
    clips: Vec<ItemId>,
    group: Option<GroupId>,
}

#[allow(clippy::too_many_lines)] // resolve, then the phases of spec §4.1 in order
fn plan(cmd: &TimelineInsert, doc: &Project, ids: &mut dyn Ids) -> Result<Planned, CmdError> {
    let sequence = doc
        .sequences
        .get(&cmd.sequence)
        .ok_or_else(|| CmdError::not_found(EntityKind::Sequence, cmd.sequence))?;
    let in_sequence = |id: TrackId, pointer: &str| -> Result<(), CmdError> {
        if sequence.track(&id).is_some() {
            Ok(())
        } else if doc.sequences.values().any(|s| s.track(&id).is_some()) {
            Err(CmdError::invalid(
                pointer,
                format!("track `{id}` is not in sequence `{}`", cmd.sequence),
            ))
        } else {
            Err(CmdError::not_found(EntityKind::Track, id))
        }
    };
    in_sequence(cmd.track, "/track")?;
    let track = &sequence.tracks[&cmd.track];
    let media = doc
        .media
        .get(&cmd.media)
        .ok_or_else(|| CmdError::not_found(EntityKind::Media, cmd.media))?;
    if let Some(a) = cmd.audio_track {
        in_sequence(a, "/audio_track")?;
    }
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
    if let Some(a) = cmd.audio_track
        && sequence.tracks[&a].kind != TrackKind::Audio
    {
        return Err(CmdError::invalid(
            "/audio_track",
            format!("track `{a}` is not an audio track"),
        ));
    }
    let default = if track.magnetic {
        PlacementMode::Insert
    } else {
        PlacementMode::Reject
    };
    let mode = resolve_mode(cmd.mode, cmd.overlap, default)?;
    let (at, source_in, duration) = range(cmd, sequence, media)?;

    let mut e = Edit::new(doc, cmd.sequence, "/at")?;
    let at = if track.magnetic {
        clamp_to_end(&e, track.id, at)
    } else {
        at
    };
    let end = at + duration.flicks();
    match mode {
        PlacementMode::Insert => {
            e.split_at(ids, track.id, at);
            let d = duration.flicks();
            e.ripple(
                &[track.id],
                cmd.ripple_all_tracks.unwrap_or(false),
                at,
                &move |_| d,
            );
        }
        PlacementMode::Overwrite => e.cut(ids, track.id, at, end),
        PlacementMode::NewTrack | PlacementMode::Reject => {}
    }
    let main = match e.blocker(track.id, at, end, None) {
        None => Slot::Existing(track.id),
        Some(_) if mode == PlacementMode::NewTrack => Slot::New { above: track.id },
        Some(other) => {
            let name = e.track_name(track.id);
            e.defer(CmdError::invalid(
                "/at",
                format!(
                    "the clip would overlap item `{other}` on track `{name}`; choose a free \
                     time or use mode \"new_track\""
                ),
            ));
            Slot::Existing(track.id)
        }
    };
    let audio =
        if track.kind == TrackKind::Video && cmd.link != Some(false) && has_audio_stream(media) {
            audio_slot(&mut e, cmd.audio_track, at, end, mode)
        } else {
            None
        };

    let group = audio.map(|_| e.mint::<GroupId>(ids));
    let mut clips = Vec::new();
    for slot in std::iter::once(Some(main))
        .chain(std::iter::once(audio))
        .flatten()
    {
        let track = match slot {
            Slot::Existing(t) => t,
            Slot::New { above } => e.new_track_above(ids, above),
        };
        let id: ItemId = e.mint(ids);
        let mut clip = Clip::new(id, media.id, time(at), duration);
        clip.source_in = source_in;
        clip.linked_group = group;
        e.insert(track, Item::Clip(clip));
        clips.push(id);
    }
    Ok(Planned {
        plan: e.finish(ids)?,
        at,
        duration,
        clips,
        group,
    })
}

/// INV-45: where the linked sound goes.
fn audio_slot(
    e: &mut Edit<'_>,
    audio_track: Option<TrackId>,
    at: i64,
    end: i64,
    mode: PlacementMode,
) -> Option<Slot> {
    if let Some(a) = audio_track {
        if let Some(other) = e.blocker(a, at, end, None) {
            let name = e.track_name(a);
            e.defer(CmdError::invalid(
                "/audio_track",
                format!("the sound would overlap item `{other}` on track `{name}`"),
            ));
        }
        return Some(Slot::Existing(a));
    }
    let unlocked: Vec<TrackId> = e
        .seq
        .tracks_in_order()
        .filter(|t| t.kind == TrackKind::Audio && !t.locked)
        .map(|t| t.id)
        .collect();
    if let Some(free) = unlocked
        .iter()
        .find(|t| e.blocker(**t, at, end, None).is_none())
    {
        return Some(Slot::Existing(*free));
    }
    match (mode, unlocked.first()) {
        (PlacementMode::Reject, _) | (_, None) => None,
        (_, Some(first)) => Some(Slot::New { above: *first }),
    }
}

fn run(cmd: &TimelineInsert, tx: &mut Tx<'_>) -> Result<TimelineInsertOutput, CmdError> {
    let doc = tx.doc().clone();
    let planned = plan(cmd, &doc, tx)?;
    let report = planned.plan.commit(tx)?;
    Ok(TimelineInsertOutput {
        at: time(planned.at),
        duration: planned.duration,
        clips: planned.clips,
        linked_group: planned.group,
        created_tracks: planned.plan.created_tracks,
        report,
    })
}

impl Command for TimelineInsert {
    type Output = TimelineInsertOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        plan(self, ctx.doc(), &mut DryIds::default()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<TimelineInsertOutput, CmdError> {
        run(self, tx)
    }
}

impl Command for TimelineOverwrite {
    type Output = TimelineInsertOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        plan(&self.as_insert(), ctx.doc(), &mut DryIds::default()).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<TimelineInsertOutput, CmdError> {
        run(&self.as_insert(), tx)
    }
}
