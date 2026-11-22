// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared fixtures: a small project and hand-written (macro-free) registry entries, which show
//! that the traits work without `#[command]` (e.g. for plugin-defined commands later).

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use cutweave_commands::{
    Annotations, CmdError, Command, CommandSpec, Described, EngineCommand, EntryKind, McpExposure,
    Query, QueryCtx, Scope, Tx,
};
use cutweave_model::{
    Clip, Duration, EntityKind, FrameRate, IdGen, Item, ItemId, MediaAsset, MediaId, MediaKind,
    ProbeInfo, Project, SampleRate, SeededGen, Sequence, SequenceFormat, SequenceId, Time,
    TimeInput, Track, TrackId, TrackKind,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const FRAME: i64 = 23_520_000; // one frame at 30 fps, in flicks

pub fn secs(s: i64) -> Time {
    Time::from_secs(s).unwrap()
}

pub fn dur(s: i64) -> Duration {
    Duration::from_secs(s).unwrap()
}

/// IDs of the [`fixture`] project.
#[derive(Clone, Copy, Debug)]
pub struct Ids {
    pub media: MediaId,
    pub sequence: SequenceId,
    pub video: TrackId,
    pub audio: TrackId,
    /// Clip on V1 at 0..4 s.
    pub a: ItemId,
    /// Clip on V1 at 4..10 s.
    pub b: ItemId,
}

/// One 30 fps sequence with a video track (two clips of a 60 s video) and an empty audio
/// track, built with deterministic IDs.
pub fn fixture() -> (Project, Ids, SeededGen) {
    let mut ids = SeededGen::new(7);
    let mut project = Project::new("Fixture");
    let probe = ProbeInfo {
        duration: Some(dur(60)),
        ..ProbeInfo::default()
    };
    let media = MediaAsset::file(ids.mint(), MediaKind::Video, "/media/a.mp4", probe);
    let format = SequenceFormat::new(1920, 1080, FrameRate::FPS_30, SampleRate::HZ_48_000);
    let mut sequence = Sequence::new(ids.mint(), "Main", format);
    let mut video = Track::new(ids.mint(), TrackKind::Video, "V1");
    let audio = Track::new(ids.mint(), TrackKind::Audio, "A1");
    let a = Clip::new(ids.mint(), media.id, secs(0), dur(4));
    let mut b = Clip::new(ids.mint(), media.id, secs(4), dur(6));
    b.source_in = secs(10);
    let out = Ids {
        media: media.id,
        sequence: sequence.id,
        video: video.id,
        audio: audio.id,
        a: a.id,
        b: b.id,
    };
    video.items.insert(a.into());
    video.items.insert(b.into());
    sequence.insert_track(0, video);
    sequence.insert_track(1, audio);
    project.media.insert(media.id, media);
    project.sequences.insert(sequence.id, sequence);
    project.validate().unwrap();
    (project, out, ids)
}

const fn spec(name: &'static str, kind: EntryKind) -> CommandSpec {
    CommandSpec {
        name,
        summary: "A test entry",
        kind,
        undoable: matches!(kind, EntryKind::Command),
        view: false,
        scope: Scope::Any,
        mcp: McpExposure::Catalog,
        annotations: Annotations {
            read_only: matches!(kind, EntryKind::Query),
            destructive: false,
            idempotent: matches!(kind, EntryKind::Query),
            open_world: false,
        },
        secret_fields: &[],
        rust_type: "test",
    }
}

/// Splits a clip at a timeline time (the hand-written counterpart of arch §5.1's example).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct SplitClip {
    /// The clip to split.
    pub item: ItemId,
    /// Where: flicks, seconds, a frame or a timecode.
    pub at: TimeInput,
}

/// The two halves.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
pub struct SplitOutput {
    pub left: ItemId,
    pub right: ItemId,
}

impl Described for SplitClip {
    const SPEC: CommandSpec = CommandSpec {
        name: "clip.split",
        summary: "Split a clip at a timeline time",
        mcp: McpExposure::Core,
        ..spec("clip.split", EntryKind::Command)
    };
}

fn split_time(ctx: &QueryCtx<'_>, command: &SplitClip) -> Result<(Time, Clip, TrackId), CmdError> {
    let at = ctx.item(command.item)?;
    let rate = at.sequence.format.frame_rate;
    let t = command
        .at
        .resolve(Some(rate))
        .map_err(|e| CmdError::invalid("/at", e.to_string()))?;
    let clip = at
        .item
        .as_clip()
        .ok_or_else(|| CmdError::unsupported("only clips can be split"))?
        .clone();
    let end = (clip.start + clip.duration).map_err(|e| CmdError::internal(e.to_string()))?;
    if t <= clip.start || t >= end {
        return Err(CmdError::invalid("/at", "must be strictly inside the item"));
    }
    if at.track.locked {
        return Err(CmdError::locked(EntityKind::Track, at.track.id));
    }
    Ok((t, clip, at.track.id))
}

impl Command for SplitClip {
    type Output = SplitOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        split_time(ctx, self).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<SplitOutput, CmdError> {
        let (t, left, track) = split_time(&tx.query_ctx(), self)?;
        let offset = (t - left.start).map_err(|e| CmdError::internal(e.to_string()))?;
        let mut right = left.clone();
        right.id = tx.new_id();
        right.start = t;
        right.duration = (left.duration - offset).map_err(|e| CmdError::internal(e.to_string()))?;
        right.source_in =
            (left.source_in + offset).map_err(|e| CmdError::internal(e.to_string()))?;
        tx.update_item(self.item, |item| {
            item.set_duration(offset);
            Ok(())
        })?;
        tx.insert_item(track, &Item::from(right.clone()))?;
        Ok(SplitOutput {
            left: left.id,
            right: right.id,
        })
    }
}

/// Fails after a partial change (tests rollback).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct RenameThenFail {
    pub name: String,
}

/// Empty output.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
pub struct Done {}

impl Described for RenameThenFail {
    const SPEC: CommandSpec = spec("project.rename", EntryKind::Command);
}

impl Command for RenameThenFail {
    type Output = Done;
    fn apply(&self, tx: &mut Tx<'_>) -> Result<Done, CmdError> {
        tx.update_meta(|meta| {
            meta.name.clone_from(&self.name);
            Ok(())
        })?;
        if self.name == "fail" {
            return Err(CmdError::invalid("/name", "refused after renaming"));
        }
        Ok(Done {})
    }
}

/// Counts items.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct CountItems {
    /// Only this sequence.
    #[serde(default)]
    pub sequence: Option<SequenceId>,
}

/// The count.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
pub struct Count {
    pub count: u32,
}

impl Described for CountItems {
    const SPEC: CommandSpec = spec("timeline.count", EntryKind::Query);
}

impl Query for CountItems {
    type Output = Count;
    fn run(&self, ctx: &QueryCtx<'_>) -> Result<Count, CmdError> {
        let n = match self.sequence {
            Some(id) => ctx
                .sequence(id)?
                .tracks
                .values()
                .map(|t| t.items.len())
                .sum(),
            None => ctx.doc().item_count(),
        };
        Ok(Count {
            count: u32::try_from(n).unwrap_or(u32::MAX),
        })
    }
}

/// UI view state, owned by the engine (stand-in for WP-1C.04).
#[derive(Debug, Default)]
pub struct ViewState {
    pub playhead: Time,
}

/// Moves the playhead.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct SetPlayhead {
    pub at: Time,
}

impl Described for SetPlayhead {
    const SPEC: CommandSpec = CommandSpec {
        undoable: false,
        view: true,
        ..spec("view.set_playhead", EntryKind::Engine)
    };
}

impl EngineCommand for SetPlayhead {
    type Context = ViewState;
    type Output = Done;
    fn run(&self, ctx: &mut ViewState) -> Result<Done, CmdError> {
        ctx.playhead = self.at;
        Ok(Done {})
    }
}

/// A UI-only command with a secret param.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct SetKey {
    pub provider: String,
    pub key: String,
}

impl Described for SetKey {
    const SPEC: CommandSpec = CommandSpec {
        scope: Scope::UiOnly,
        secret_fields: &["key"],
        undoable: false,
        ..spec("ai.set_key", EntryKind::Command)
    };
}

impl Command for SetKey {
    type Output = Done;
    fn apply(&self, _: &mut Tx<'_>) -> Result<Done, CmdError> {
        Ok(Done {})
    }
}
