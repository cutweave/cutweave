// SPDX-License-Identifier: GPL-3.0-or-later
//! Param strategies, one per registered entry, drawn from the document: IDs of what exists
//! (plus some that do not), times on the owning sequence's frame grid in every accepted input
//! form, and a share of semantically invalid and malformed params.
//!
//! [`args_for`] is the table. A command added to the registry without an arm here makes
//! [`arb_command_from`](super::arb_command_from) panic with `missing property strategy for
//! <name>`.

use std::{sync::Arc, time::Duration};

use cutweave_jobs::{JobConfig, JobCtx, JobError, JobId, JobKind, JobManager, JobSpec};
use cutweave_model::{
    CollectionId, EntityId, FolderId, FrameRate, ItemId, MarkerId, MediaId, MediaKind, ProbeInfo,
    Project, SampleRate, SeededGen, SequenceId, TrackId, TrackKind, Vec2,
};
use proptest::{
    collection::vec,
    option,
    prelude::*,
    sample::{Index, select, subsequence},
};
use serde_json::{Value, json};

use cutweave_commands::DocSnapshot;

use crate::{
    export::{ExportCtx, ExportPlan, ExportStarter},
    item::ITEM_GET_FIELDS,
    media::{
        AnalysisInput, AnalysisSink, AnalysisStage, AnalysisStages, Analyzer, FakeProbe,
        MediaEvent, ProbeError, Probed,
    },
    timeline::GET_FIELDS,
};

/// The files [`fake_probe`] serves to `media.import`: a video with sound, one without, audio, a
/// still, a path with a space and a Windows path.
pub const IMPORT_PATHS: [&str; 6] = [
    "/import/interview.mp4",
    "/import/b-roll.mp4",
    "/import/music.wav",
    "/import/logo.png",
    "/import/long take.mov",
    "C:\\import\\voice.wav",
];

/// The pictures [`fake_probe`] serves to `media.import_images_as_slideshow` (their names sort
/// differently by name and by number).
pub const SLIDE_PATHS: [&str; 4] = [
    "/import/slides/img10.png",
    "/import/slides/img2.png",
    "/import/slides/Beach.png",
    "C:\\import\\slides\\img1.png",
];

/// How many frames [`FakeFrames`] can store before it gives the last one again.
const FRAME_SLOTS: u128 = 3;

/// The id and the file of slot `n` of [`FakeFrames`]; [`fake_probe`] knows the files as
/// pictures.
fn frame_slot(n: u128) -> (MediaId, String) {
    let id = MediaId::from_u128(0xF4A3_E000 + n);
    (id, format!("/project/generated/frames/{id}.png"))
}

/// The frame store a harness gives `media.import_frame` (its engine has no renderer and no
/// disk): it resolves the source and the time against `doc` the way the engine does, "writes"
/// the next unused of a few fixed files, and records what was discarded.
#[derive(Debug)]
pub struct FakeFrames {
    doc: Project,
    discarded: std::sync::Mutex<Vec<String>>,
}

impl FakeFrames {
    /// A store for the document `doc`.
    #[must_use]
    pub fn new(doc: Project) -> FakeFrames {
        FakeFrames {
            doc,
            discarded: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// The files that were discarded so far.
    #[must_use]
    pub fn discarded(&self) -> Vec<String> {
        self.discarded
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl crate::media::FrameGrabber for FakeFrames {
    fn grab(
        &self,
        source: crate::media::FrameOf,
        at: cutweave_model::TimeInput,
    ) -> Result<crate::media::GrabbedFrame, cutweave_commands::CmdError> {
        use cutweave_commands::CmdError;
        let (name, rate, size, length) = match source {
            crate::media::FrameOf::Sequence => {
                let sequence = self.doc.sequences.values().next().ok_or_else(|| {
                    CmdError::invalid("/source", "the project has no sequence to take a frame of")
                })?;
                (
                    sequence.name.clone(),
                    sequence.format.frame_rate,
                    (sequence.format.width, sequence.format.height),
                    None,
                )
            }
            crate::media::FrameOf::Media(id) => {
                let asset =
                    self.doc.media.get(&id).ok_or_else(|| {
                        CmdError::not_found(cutweave_model::EntityKind::Media, id)
                    })?;
                if !crate::media::has_frames(asset.kind) {
                    return Err(CmdError::invalid(
                        "/source/media",
                        format!("media {id} is not a video: it has no frame to take"),
                    ));
                }
                (
                    crate::media::asset_name(asset).into_owned(),
                    asset.probe.fps.unwrap_or(FrameRate::FPS_30),
                    (1920, 1080),
                    asset.probe.duration,
                )
            }
        };
        let time = at
            .resolve(Some(rate))
            .map_err(|e| CmdError::invalid("/at", e.to_string()))?;
        if time < cutweave_model::Time::ZERO
            || length.is_some_and(|length| time.flicks() >= length.flicks())
        {
            return Err(CmdError::invalid("/at", "the time is outside the source"));
        }
        let slot = (0..FRAME_SLOTS)
            .map(frame_slot)
            .find(|(id, _)| !self.doc.media.contains_key(id))
            .unwrap_or_else(|| frame_slot(FRAME_SLOTS - 1));
        Ok(crate::media::GrabbedFrame {
            media_id: slot.0,
            path: slot.1,
            width: size.0,
            height: size.1,
            time,
            label: crate::media::frame_label(&name, time, rate),
        })
    }

    fn discard(&self, frame: &crate::media::GrabbedFrame) {
        self.discarded
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(frame.path.clone());
    }
}
/// The pictures of the numbered run [`fake_probe`] serves to `media.import_sequence`
/// (`shot_0004.png` is missing: a gap).
pub const SEQUENCE_PATHS: [&str; 5] = [
    "/import/render/shot_0001.png",
    "/import/render/shot_0002.png",
    "/import/render/shot_0003.png",
    "/import/render/shot_0005.png",
    "/import/render/shot_0006.png",
];

/// The folder of [`SEQUENCE_PATHS`].
pub const SEQUENCE_FOLDER: &str = "/import/render";

/// A file [`fake_probe`] cannot read: `media.import` reports it in `failed[]` (code
/// `unsupported`) and imports the other files.
pub const BROKEN_IMPORT_PATH: &str = "/import/notes.txt";

/// Seed of the job IDs a harness's job manager mints ([`job_manager`]), so the `job.*`
/// strategies can name jobs that exist.
pub const JOB_ID_SEED: u64 = 7;

/// How many of the first [`JOB_ID_SEED`] job IDs the `job.*` strategies draw from: the three
/// jobs [`job_manager`] starts with, then analysis jobs of the first imports.
const KNOWN_JOBS: usize = 8;

/// Checkpoint names the `edit.checkpoint`, `edit.restore` and `edit.delete_checkpoint`
/// strategies share.
pub const CHECKPOINT_NAMES: [&str; 3] = ["alpha", "beta", "gamma"];

/// The label of the second actor (`agent:prop`) the strategies name in history scopes.
pub const AGENT_LABEL: &str = "prop";

/// The prober a harness gives `media.import`: it knows [`IMPORT_PATHS`], the pictures of
/// [`SLIDE_PATHS`] and the files of [`FakeFrames`], and fails [`BROKEN_IMPORT_PATH`].
#[must_use]
pub fn fake_probe() -> FakeProbe {
    let [interview, broll, music, logo, take, voice] = IMPORT_PATHS;
    let frames: Vec<String> = (0..FRAME_SLOTS).map(|n| frame_slot(n).1).collect();
    SEQUENCE_PATHS
        .iter()
        .fold(FakeProbe::new(), |probe, picture| {
            probe.with(picture, FakeProbe::image(picture))
        })
        .with(interview, FakeProbe::video(interview, 20, true))
        .with(broll, FakeProbe::video(broll, 8, false))
        .with(music, FakeProbe::audio(music, 30))
        .with(logo, FakeProbe::image(logo))
        .with(take, FakeProbe::video(take, 60, true))
        .with(voice, FakeProbe::audio(voice, 12))
        .with_error(
            BROKEN_IMPORT_PATH,
            ProbeError::Unsupported {
                path: BROKEN_IMPORT_PATH.to_owned(),
                reason: "no audio or video streams".to_owned(),
            },
        )
        .with(SLIDE_PATHS[0], FakeProbe::image(SLIDE_PATHS[0]))
        .with(SLIDE_PATHS[1], FakeProbe::image(SLIDE_PATHS[1]))
        .with(SLIDE_PATHS[2], FakeProbe::image(SLIDE_PATHS[2]))
        .with(SLIDE_PATHS[3], FakeProbe::image(SLIDE_PATHS[3]))
        .with(&frames[0], FakeProbe::image(&frames[0]))
        .with(&frames[1], FakeProbe::image(&frames[1]))
        .with(&frames[2], FakeProbe::image(&frames[2]))
}

/// A job manager for a harness (the `job.*` commands' engine context): IDs from
/// [`JOB_ID_SEED`], two decode slots, and three jobs already submitted, so `job.*` requests
/// find jobs in several states: one that succeeds at once, one that fails, and one that runs
/// for up to 200 ms unless cancelled. Job bodies never touch the document.
///
/// # Panics
///
/// If the manager's runtime cannot start.
#[must_use]
pub fn job_manager() -> JobManager {
    let jobs = JobManager::builder()
        .config(JobConfig {
            decode_slots: 2,
            ..JobConfig::default()
        })
        .ids(Box::new(SeededGen::new(JOB_ID_SEED)))
        .build()
        .unwrap_or_else(|e| panic!("the job manager does not start: {e}"));
    jobs.submit(JobSpec::new(JobKind::Analysis, "index"), |_| Ok(()));
    jobs.submit(JobSpec::new(JobKind::Analysis, "thumbnails"), |_| {
        Err(JobError::failed("fake decoder error"))
    });
    jobs.submit(JobSpec::new(JobKind::Export, "export"), |ctx| {
        ctx.sleep(Duration::from_millis(200))
    });
    jobs
}

/// One fake analysis stage (`index`, which succeeds at once) for `media.import` in a harness.
struct FakeIndex;

impl AnalysisStage for FakeIndex {
    fn name(&self) -> &'static str {
        "index"
    }

    fn builds_index(&self) -> bool {
        true
    }

    fn run(&self, _: &AnalysisInput, ctx: &JobCtx) -> Result<(), JobError> {
        ctx.check()
    }
}

/// Drops the events of analysis jobs.
struct DiscardEvents;

impl AnalysisSink for DiscardEvents {
    fn publish(&self, _: MediaEvent) {}
}

/// An analyzer for `media.import` in a harness: the fake `index` stage on `jobs`, events
/// discarded.
///
/// # Panics
///
/// Never (the single stage always registers).
#[must_use]
pub fn analyzer(jobs: &JobManager) -> Analyzer {
    let mut stages = AnalysisStages::new();
    if let Err(e) = stages.add(Arc::new(FakeIndex)) {
        panic!("the fake stage does not register: {e}");
    }
    Analyzer::new(jobs.clone(), Arc::new(stages), Arc::new(DiscardEvents))
}

/// The job IDs the `job.*` strategies name (see [`KNOWN_JOBS`]).
fn known_job_ids() -> Vec<JobId> {
    let mut ids = SeededGen::new(JOB_ID_SEED);
    (0..KNOWN_JOBS).map(|_| JobId::mint(&mut ids)).collect()
}

// ----- the document's vocabulary -------------------------------------------------------------

/// Flicks per frame at 30 fps, the grid of params that name no sequence.
const FPF_30: i64 = 23_520_000;
/// Flicks per millisecond (seconds text is written to the millisecond).
const FLICKS_PER_MS: i64 = 705_600;

#[derive(Clone, Debug)]
struct Trk {
    id: TrackId,
    seq: SequenceId,
    kind: TrackKind,
    fpf: i64,
    /// End of its last item, in frames.
    end: i64,
}

#[derive(Clone, Debug)]
struct Itm {
    id: ItemId,
    seq: SequenceId,
    track: TrackId,
    kind: TrackKind,
    fpf: i64,
    /// Start and length in frames.
    start: i64,
    len: i64,
    /// End of the sequence, in frames.
    seq_end: i64,
    /// The keys of its animated properties: property ID and item time in flicks.
    keys: Vec<(&'static str, i64)>,
    /// The media it plays, if it is a clip.
    media: Option<MediaId>,
}

#[derive(Clone, Debug)]
struct Med {
    id: MediaId,
    kind: MediaKind,
    /// Playable length, for media whose clips are bounded by it.
    flicks: Option<i64>,
    used: bool,
}

/// What the strategies can refer to in a document.
#[derive(Clone, Debug, Default)]
pub(super) struct Pools {
    seqs: Vec<SequenceId>,
    /// Each sequence with its flicks per frame.
    seq_fpf: Vec<(SequenceId, i64)>,
    markers: Vec<MarkerId>,
    tracks: Vec<Trk>,
    items: Vec<Itm>,
    media: Vec<Med>,
    /// Media folders (ADR-0020).
    folders: Vec<FolderId>,
    /// Collections of media (ADR-0036), and the manual ones among them.
    collections: Vec<CollectionId>,
    manual_collections: Vec<CollectionId>,
}

impl Pools {
    pub(super) fn of(doc: &Project) -> Pools {
        let mut pools = Pools::default();
        let mut used = std::collections::BTreeSet::new();
        for s in doc.sequences.values() {
            let fpf = s
                .format
                .frame_rate
                .flicks_per_frame()
                .unwrap_or(FPF_30)
                .max(1);
            let frames = |flicks: i64| flicks.div_euclid(fpf);
            let seq_end = frames(s.duration().flicks());
            pools.seqs.push(s.id);
            pools.seq_fpf.push((s.id, fpf));
            pools.markers.extend(s.markers.keys().copied());
            for t in s.tracks_in_order() {
                pools.tracks.push(Trk {
                    id: t.id,
                    seq: s.id,
                    kind: t.kind,
                    fpf,
                    end: frames(t.items.end().flicks()),
                });
                for i in t.items.iter() {
                    pools.items.push(Itm {
                        id: i.id(),
                        seq: s.id,
                        track: t.id,
                        kind: t.kind,
                        fpf,
                        start: frames(i.start().flicks()),
                        len: frames(i.duration().flicks()).max(1),
                        seq_end,
                        keys: item_keys(i),
                        media: i.media(),
                    });
                    used.extend(i.media());
                    pools.markers.extend(i.markers().keys().copied());
                }
            }
        }
        pools.media = doc
            .media
            .values()
            .map(|m| Med {
                id: m.id,
                kind: m.kind,
                flicks: m
                    .probe
                    .duration
                    .filter(|_| m.kind != MediaKind::Image)
                    .map(cutweave_model::Duration::flicks),
                used: used.contains(&m.id),
            })
            .collect();
        pools.folders = doc.folders.keys().copied().collect();
        pools.collections = doc.collections.keys().copied().collect();
        pools.manual_collections = doc
            .collections
            .values()
            .filter(|c| matches!(c.kind, cutweave_model::CollectionKind::Manual { .. }))
            .map(|c| c.id)
            .collect();
        pools
    }

    fn item_ids(&self) -> Vec<ItemId> {
        self.items.iter().map(|i| i.id).collect()
    }

    fn track_ids(&self) -> Vec<TrackId> {
        self.tracks.iter().map(|t| t.id).collect()
    }

    fn media_ids(&self) -> Vec<MediaId> {
        self.media.iter().map(|m| m.id).collect()
    }

    /// The media that can have proxies (video and image sequences), else every asset: the
    /// proxy commands mostly name these, and sometimes one they refuse.
    fn proxy_ids(&self) -> Vec<MediaId> {
        let video: Vec<MediaId> = self
            .media
            .iter()
            .filter(|m| matches!(m.kind, MediaKind::Video | MediaKind::ImageSequence))
            .map(|m| m.id)
            .collect();
        if video.is_empty() {
            self.media_ids()
        } else {
            video
        }
    }
}

/// The keys of a clip's animated properties.
fn item_keys(item: &cutweave_model::Item) -> Vec<(&'static str, i64)> {
    use cutweave_model::{PropId, PropRef};
    let Some(clip) = item.as_clip() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for id in PropId::ALL {
        let times: Vec<i64> = match clip.prop(id) {
            PropRef::Scalar(a) => a.sample_times().map(cutweave_model::Time::flicks).collect(),
            PropRef::Vec2(a) => a.sample_times().map(cutweave_model::Time::flicks).collect(),
            PropRef::Bool(a) => a.sample_times().map(cutweave_model::Time::flicks).collect(),
        };
        out.extend(times.into_iter().map(|t| (id.as_str(), t)));
    }
    out
}

/// The weight and the param strategy of `name` against `p`, or `None` if this table has no
/// strategy for it.
#[allow(clippy::too_many_lines)] // one arm per command; a lookup table reads best as one match
pub(super) fn args_for(name: &str, p: &Pools) -> Option<(u32, BoxedStrategy<Value>)> {
    let edits: u32 = if p.items.is_empty() { 1 } else { 6 };
    let empty = p.seqs.is_empty() && p.media.is_empty();
    let no_media = p.media.is_empty();
    Some(match name {
        "project.new" => (if empty { 12 } else { 1 }, project_new()),
        "project.rename" => (1, project_rename()),
        "project.set_settings" => (1, project_set_settings()),
        "project.info"
        | "edit.locks"
        | "edit.list_checkpoints"
        | "export.list_presets"
        | "export.list_encoders"
        | "export.queue"
        | "media.list_folders"
        | "media.list_tags"
        | "media.list_collections" => (1, no_params()),
        "media.add_tags" => (if no_media { 1 } else { 3 }, media_tags(p)),
        "media.remove_tags" => (1, media_tags(p)),
        "media.rename_tag" => (1, media_rename_tag()),
        "media.set_rating" => (if no_media { 1 } else { 2 }, media_set_rating(p)),
        "media.set_note" => (if no_media { 1 } else { 2 }, media_set_note(p)),
        "media.create_collection" => (
            if p.collections.is_empty() { 4 } else { 2 },
            media_create_collection(p),
        ),
        "media.rename_collection" => (1, media_rename_collection(p)),
        "media.delete_collection" => (1, media_delete_collection(p)),
        "media.add_to_collection" => (
            if p.manual_collections.is_empty() {
                1
            } else {
                3
            },
            media_collection_members(p),
        ),
        "media.remove_from_collection" => (1, media_collection_members(p)),
        "media.set_collection_filter" => (1, media_set_collection_filter(p)),
        "media.register" => (if no_media { 4 } else { 2 }, media_register(p)),
        "media.register_tree" => (if no_media { 3 } else { 1 }, media_register_tree(p)),
        "media.import" => (if no_media { 4 } else { 2 }, media_import(p)),
        "media.import_images_as_slideshow" => (2, media_slideshow(p)),
        "media.import_frame" => (1, media_import_frame(p)),
        "media.import_sequence" => (1, media_import_sequence(p)),
        "media.list" => (1, media_list(p)),
        "media.remove" => (1, media_remove(p)),
        "media.generate_proxy" | "media.delete_proxy" => (1, proxy_media(p, false)),
        "media.proxy_status" => (1, proxy_media(p, true)),
        "media.set_proxy_policy" => (1, proxy_policy()),
        "media.set_proxy_mode" => (1, proxy_mode(p)),
        "project.collect" | "project.collect_estimate" => (1, project_collect()),
        "project.archive" => (1, project_archive()),
        "media.set_location" => (1, media_set_location(p)),
        "media.relink" => (1, media_relink(p)),
        "media.relink_folder" => (1, media_relink_folder(p)),
        "media.find_missing" => (
            1,
            prop_oneof![Just(json!({})), Just(json!({"verify": true}))].boxed(),
        ),
        "media.create_folder" => (
            if p.folders.is_empty() { 4 } else { 2 },
            media_create_folder(p),
        ),
        "media.rename_folder" => (1, media_rename_folder(p)),
        "media.rename" => (1, media_rename(p)),
        "media.move" => (if p.folders.is_empty() { 1 } else { 3 }, media_move(p)),
        "media.delete_folder" => (1, media_delete_folder(p)),
        "sequence.create" => (if p.seqs.is_empty() { 1 } else { 2 }, sequence_create()),
        "sequence.create_from_media" => (1, sequence_create_from_media(p)),
        "sequence.duplicate" => (1, sequence_duplicate(p)),
        "sequence.rename" => (1, sequence_rename(p)),
        "sequence.delete" => (1, sequence_delete(p)),
        "sequence.set_format" => (2, sequence_set_format(p)),
        "sequence.list" => (1, sequence_list()),
        "track.add" => (2, track_add(p)),
        "track.remove" => (1, track_remove(p)),
        "track.set" => (2, track_set(p)),
        "track.reorder" => (1, track_reorder(p)),
        "track.set_height" => (1, track_set_height(p)),
        "track.set_magnetic" => (2, track_set_magnetic(p)),
        "track.list" => (1, track_list(p)),
        "timeline.add_marker" => (2, timeline_add_marker(p)),
        "timeline.move_marker" => (1, timeline_move_marker(p)),
        "timeline.update_marker" => (1, timeline_update_marker(p)),
        "timeline.remove_marker" => (1, timeline_remove_marker(p)),
        "timeline.set_in_out" => (1, timeline_set_in_out(p)),
        "timeline.clear_in_out" => (1, timeline_clear_in_out(p)),
        "timeline.list_markers" => (1, timeline_list_markers(p)),
        "timeline.insert" => (if p.seqs.is_empty() { 1 } else { 8 }, timeline_insert(p)),
        "timeline.overwrite" => (if p.seqs.is_empty() { 1 } else { 3 }, timeline_overwrite(p)),
        "timeline.lift" | "timeline.extract" => (edits.div_ceil(3), timeline_ranges(p)),
        "timeline.ripple_delete" => (edits.div_ceil(2), ripple_delete(p)),
        "timeline.delete_gap" => (edits.div_ceil(3), delete_gap(p)),
        "timeline.get" => (1, timeline_get(p)),
        "clip.split" => (edits, clip_split(p)),
        "clip.trim" => (edits, edge_trim(p, false)),
        "clip.ripple_trim" => (edits.div_ceil(2), edge_trim(p, true)),
        "clip.roll" => (edits.div_ceil(2), clip_roll(p)),
        "clip.slip" => (edits.div_ceil(2), clip_shift(p, false)),
        "clip.slide" => (edits.div_ceil(2), clip_shift(p, true)),
        "clip.set_speed" => (edits.div_ceil(2), clip_set_speed(p)),
        "clip.reverse" => (edits.div_ceil(3), clip_reverse(p)),
        "clip.reset_speed" => (edits.div_ceil(3), clip_reset_speed(p)),
        "clip.freeze_frame" => (edits.div_ceil(3), clip_freeze_frame(p)),
        "item.move" => (edits, item_move(p)),
        "item.delete" => (edits.div_ceil(2), item_delete(p)),
        "item.link" => (3, item_link(p)),
        "item.unlink" => (3, item_unlink(p)),
        "item.attach" => (3, item_attach(p)),
        "item.detach" => (3, item_detach(p)),
        "item.group_select" => (1, item_group_select(p)),
        "timeline.copy" => (1, item_delete(p)),
        "timeline.paste" => (edits.div_ceil(2), timeline_paste(p)),
        "clip.paste_attributes" => (edits.div_ceil(3), clip_paste_attributes(p)),
        "clip.replace" => (edits.div_ceil(2), clip_replace(p)),
        "item.duplicate" => (edits.div_ceil(2), item_duplicate(p)),
        "item.set_props" => (edits.div_ceil(2), item_set_props(p)),
        "item.get" => (1, item_get(p)),
        "item.reset_props" => (1, item_reset_props(p)),
        "item.set_label_color" => (1, item_set_label_color(p)),
        "item.apply_layout" => (edits.div_ceil(3), item_apply_layout(p)),
        "item.layouts" => (1, item_layouts()),
        "audio.set" => (edits.div_ceil(2), audio_set(p)),
        "audio.set_fade" => (edits.div_ceil(2), audio_set_fade(p)),
        "audio.set_volume" => (edits.div_ceil(2), audio_set_volume(p)),
        "audio.set_pan" => (edits.div_ceil(2), audio_set_pan(p)),
        "audio.set_gain" => (edits.div_ceil(3), audio_set_gain(p)),
        "audio.get_levels" => (1, audio_get_levels(p)),
        "keyframe.set" => (edits, keyframe_set(p)),
        "keyframe.delete" => (edits.div_ceil(3), keyframe_delete(p)),
        "keyframe.move" => (edits.div_ceil(3), keyframe_move(p)),
        "keyframe.set_interp" => (edits.div_ceil(3), keyframe_set_interp(p)),
        "keyframe.clear" => (edits.div_ceil(3), keyframe_clear(p)),
        "keyframe.paste" => (edits.div_ceil(3), keyframe_paste(p)),
        "keyframe.copy" => (1, keyframe_copy(p)),
        "keyframe.eval" => (1, keyframe_eval(p)),
        "edit.begin_tx" => (2, begin_tx()),
        "edit.commit_tx" => (2, tx_ref()),
        "edit.cancel_tx" => (1, tx_ref()),
        "edit.undo" => (3, history_scope()),
        "edit.redo" => (2, history_scope()),
        "edit.history" => (1, history_page()),
        "edit.set_history_limit" => (1, history_limit()),
        "edit.checkpoint" => (2, checkpoint()),
        "edit.restore" => (2, restore()),
        "edit.delete_checkpoint" => (1, delete_checkpoint()),
        "job.status" | "job.cancel" => (1, job_ref("id")),
        "job.list" => (1, job_list(p)),
        "job.wait" => (1, job_wait()),
        "export.start" | "export.enqueue" => (1, export_start(p)),
        "export.describe_options" => (1, describe_options()),
        "export.cancel" | "export.retry" | "export.remove" => (2, export_job_ref()),
        "export.pause_queue" => (1, pause_queue()),
        "export.reorder" => (1, export_reorder()),
        "export.resolve_preset" => (1, preset_ref("preset")),
        "export.save_preset" => (1, preset_save()),
        "export.delete_preset" => (1, preset_ref("id")),
        _ => return None,
    })
}

/// `{<key>: preset id}`: built-in ids, ids a save may have made, and unknown ones.
fn preset_ref(key: &'static str) -> BoxedStrategy<Value> {
    select(vec![
        "youtube-1080p",
        "tiktok",
        "h264-draft",
        "prores-422-hq",
        "mine-a",
        "mine-b",
        "missing",
        "Bad Id",
    ])
    .prop_map(move |id| json!({ key: id }))
    .boxed()
}

/// `export.save_preset` of a small H.264 preset under one of a few ids, sometimes invalid.
fn preset_save() -> BoxedStrategy<Value> {
    (
        select(vec!["mine-a", "mine-b", "youtube-1080p", "Bad Id"]),
        select(vec![20_u32, 28, 99]),
    )
        .prop_map(|(id, crf)| {
            json!({"preset": {
                "schema_version": 1, "id": id, "name": "Mine", "container": "mp4",
                "video": {"codec": "h264", "rate_control": {"mode": "crf", "crf": crf},
                          "hw_preferred": true},
                "audio": {"codec": "aac", "bitrate_kbps": 192, "sample_rate": 48000, "channels": 2},
                "size": {"kind": "fit", "width": 1280, "height": 720},
                "fps": {"kind": "follow_sequence"}
            }})
        })
        .boxed()
}

// ----- building blocks -----------------------------------------------------------------------

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// `ms` milliseconds as seconds text (`"-1.500s"`).
fn secs_text(ms: i64) -> String {
    let sign = if ms < 0 { "-" } else { "" };
    let ms = ms.unsigned_abs();
    format!("{sign}{}.{:03}s", ms / 1000, ms % 1000)
}

/// Frame `frame` at `fpf` flicks per frame, in an input form picked by `form`: `"f:N"`,
/// integer flicks, seconds text (when exact to the millisecond) or flicks `jitter` off the
/// frame grid (edit commands snap those to the nearest frame).
fn time_json(fpf: i64, frame: i64, form: u8, jitter: u32) -> Value {
    let flicks = frame.saturating_mul(fpf);
    match form % 8 {
        0..=2 if frame >= 0 => Value::from(format!("f:{frame}")),
        3 if flicks % FLICKS_PER_MS == 0 => Value::from(secs_text(flicks / FLICKS_PER_MS)),
        4 => Value::from(flicks.saturating_add(i64::from(jitter) % fpf.max(1))),
        _ => Value::from(flicks),
    }
}

/// An id that is not in any document.
fn fresh<I: EntityId>() -> BoxedStrategy<Value> {
    any::<u128>()
        .prop_map(|raw| Value::String(I::from_u128(raw).to_string()))
        .boxed()
}

/// One of `ids` (usually), or an id that does not exist.
fn one_of<I: EntityId>(ids: Vec<I>) -> BoxedStrategy<Value> {
    if ids.is_empty() {
        return fresh::<I>();
    }
    let known = select(ids).prop_map(|id| Value::String(id.to_string()));
    prop_oneof![9 => known, 1 => fresh::<I>()].boxed()
}

const WORDS: [&str; 6] = [
    "Main",
    "Interview",
    "B-roll",
    "Résumé",
    "Ünïcode ✂",
    "  Padded  ",
];

fn name() -> BoxedStrategy<String> {
    (select(WORDS.to_vec()), 0..100_u32)
        .prop_map(|(w, n)| format!("{w} {n}"))
        .boxed()
}

fn bad_name() -> BoxedStrategy<String> {
    prop_oneof![
        Just(String::new()),
        Just("   ".to_owned()),
        Just("x".repeat(201)),
        Just("tab\tname".to_owned()),
    ]
    .boxed()
}

fn gain() -> BoxedStrategy<f64> {
    prop_oneof![Just(0.0), Just(-6.0), -144.0..=48.0_f64].boxed()
}

/// A malformed variant of a `valid` params object: an unknown field, a missing field, or a
/// field of the wrong type (decoding fails with `Invalid` at that field, unless the dropped
/// field was optional).
fn malformed(valid: BoxedStrategy<Value>) -> BoxedStrategy<Value> {
    (valid, any::<Index>(), 0..3_u8)
        .prop_map(|(mut v, at, how)| {
            if let Value::Object(map) = &mut v {
                let key = map.keys().nth(at.index(map.len().max(1))).cloned();
                match (how, key) {
                    (1, Some(k)) => {
                        map.remove(&k);
                    }
                    (2, Some(k)) => {
                        map.insert(k, json!([[]]));
                    }
                    _ => {
                        map.insert("unexpected".to_owned(), Value::Bool(true));
                    }
                }
            }
            v
        })
        .boxed()
}

/// Mostly `valid` params, some semantically `invalid` ones and some malformed ones.
fn mix(valid: BoxedStrategy<Value>, invalid: BoxedStrategy<Value>) -> BoxedStrategy<Value> {
    prop_oneof![14 => valid.clone(), 4 => invalid, 2 => malformed(valid)].boxed()
}

fn no_params() -> BoxedStrategy<Value> {
    prop_oneof![9 => Just(json!({})), 1 => Just(json!({"unexpected": 1}))].boxed()
}

// ----- project, media ------------------------------------------------------------------------

fn project_new() -> BoxedStrategy<Value> {
    let format = (
        option::of(16..=3840_u32),
        option::of(16..=2160_u32),
        option::of(select(FrameRate::COMMON.to_vec())),
        option::of(select(SampleRate::COMMON.to_vec())),
    )
        .prop_map(|(width, height, frame_rate, sample_rate)| {
            json!({"width": width, "height": height, "frame_rate": frame_rate,
                   "sample_rate": sample_rate})
        });
    let still = option::of(prop_oneof![
        Just(json!("3s")),
        Just(json!("f:1")),
        Just(json!("3600s")),
    ]);
    let policy = option::of(select(vec!["off", "auto", "all"]));
    let valid = (name(), option::of(format), still, policy)
        .prop_map(|(name, format, still, policy)| {
            json!({"name": name, "format": format, "still_duration": still,
                   "proxy_policy": policy})
        })
        .boxed();
    let invalid = prop_oneof![
        bad_name().prop_map(|name| json!({"name": name})),
        Just(json!({"name": "P", "format": {"width": 0}})),
        Just(json!({"name": "P", "format": {"height": 20_000}})),
        Just(json!({"name": "P", "format": {"frame_rate": {"num": 2000, "den": 1}}})),
        Just(json!({"name": "P", "format": {"frame_rate": {"num": 1001, "den": 7}}})),
        Just(json!({"name": "P", "format": {"sample_rate": 1234}})),
        Just(json!({"name": "P", "still_duration": "0s"})),
        Just(json!({"name": "P", "still_duration": "3601s"})),
        Just(json!({"name": "P", "proxy_policy": "some"})),
        Just(json!({"name": "P", "settings": {"nope": 1}})),
        Just(json!({"name": "P", "settings": {"still_duration": 0}})),
        Just(json!({"name": "P", "proxy_policy": "off", "settings": {"proxy_policy": "all"}})),
    ]
    .boxed();
    let with_settings = (name(), settings_patch())
        .prop_map(|(name, settings)| json!({"name": name, "settings": settings}))
        .boxed();
    mix(prop_oneof![3 => valid, 1 => with_settings].boxed(), invalid)
}

/// A valid patch of the project's settings (WP-2H.08): any subset of the keys, `null` for the
/// optional ones.
fn settings_patch() -> BoxedStrategy<Value> {
    let still = option::of(prop_oneof![
        Just(json!(705_600_000_i64)),
        Just(json!(2_116_800_000_i64)),
        Just(Value::Null),
    ]);
    let policy = option::of(prop_oneof![
        select(vec!["off", "auto", "all"]).prop_map(|p| json!(p)),
        Just(Value::Null),
    ]);
    let format = option::of(
        (
            option::of(16..=3840_u32),
            option::of(select(FrameRate::COMMON.to_vec())),
            option::of(select(vec!["#000000", "#10203040"])),
        )
            .prop_map(|(width, frame_rate, background)| {
                let mut format = serde_json::Map::new();
                if let Some(width) = width {
                    format.insert("width".into(), json!(width));
                }
                if let Some(rate) = frame_rate {
                    format.insert("frame_rate".into(), json!(rate));
                }
                if let Some(background) = background {
                    format.insert("background".into(), json!(background));
                }
                Value::Object(format)
            }),
    );
    (still, policy, format)
        .prop_map(|(still, policy, format)| {
            let mut patch = serde_json::Map::new();
            if let Some(still) = still {
                patch.insert("still_duration".into(), still);
            }
            if let Some(policy) = policy {
                patch.insert("proxy_policy".into(), policy);
            }
            if let Some(format) = format {
                patch.insert("default_format".into(), format);
            }
            Value::Object(patch)
        })
        .boxed()
}

/// `project.set_settings` (WP-2H.08): a patch of any keys; an unknown key, a value of the
/// wrong type or out of range, no patch.
fn project_set_settings() -> BoxedStrategy<Value> {
    let valid = settings_patch()
        .prop_map(|patch| json!({ "patch": patch }))
        .boxed();
    let invalid = prop_oneof![
        Just(json!({"patch": {"nope": true}})),
        Just(json!({"patch": {"default_format": {"widht": 4}}})),
        Just(json!({"patch": {"proxy_policy": "some"}})),
        Just(json!({"patch": {"still_duration": 0}})),
        Just(json!({"patch": {"still_duration": 2_540_866_560_000_i64}})),
        Just(json!({"patch": {"default_format": {"width": 0}}})),
        Just(json!({"patch": {"default_format": null}})),
        Just(json!({"patch": 3})),
        Just(json!({})),
    ]
    .boxed();
    mix(valid, invalid)
}

fn project_rename() -> BoxedStrategy<Value> {
    mix(
        name().prop_map(|name| json!({"name": name})).boxed(),
        bad_name().prop_map(|name| json!({"name": name})).boxed(),
    )
}

/// A probed file `n` of kind `kind` (video, audio, still).
fn probed_file(n: u32, kind: u8, secs: i64, audio: bool) -> (String, Probed) {
    let ext = ["mp4", "wav", "png"][usize::from(kind % 3)];
    let path = format!("/media/reg/{n}.{ext}");
    let probed = match kind % 3 {
        0 => FakeProbe::video(&path, secs, audio),
        1 => FakeProbe::audio(&path, secs),
        _ => FakeProbe::image(&path),
    };
    (path, probed)
}

fn media_register(p: &Pools) -> BoxedStrategy<Value> {
    let media_id = prop_oneof![
        8 => Just(Value::Null),
        1 => fresh::<MediaId>(),
        1 => one_of(p.media_ids()),
    ];
    let valid = (
        0..40_u32,
        0..3_u8,
        1..=90_i64,
        any::<bool>(),
        media_id,
        option::weighted(0.3, one_of(p.folders.clone())),
    )
        .prop_map(|(n, kind, secs, audio, media_id, folder)| {
            let (path, probed) = probed_file(n, kind, secs, audio);
            json!({"media_id": media_id, "path": path, "probe": probed.probe,
                   "content_hash": probed.content_hash, "folder": folder})
        })
        .boxed();
    let invalid = (0..40_u32, 0..6_u8)
        .prop_map(|(n, how)| {
            let (path, probed) = probed_file(n, 0, 10, true);
            let mut v = json!({"path": path, "probe": probed.probe,
                               "content_hash": probed.content_hash});
            match how {
                0 => v["path"] = json!("relative/clip.mp4"),
                1 => v["content_hash"] = json!(""),
                2 => v["content_hash"] = json!("has space"),
                3 => v["probe"]["rotation"] = json!(45),
                4 => v["probe"]["duration"] = json!(0),
                _ => v["probe"] = json!(ProbeInfo::default()),
            }
            v
        })
        .boxed();
    mix(valid, invalid)
}

/// Files with the bin folders above them (`media.import` of a folder, WP-2F.09).
fn media_register_tree(p: &Pools) -> BoxedStrategy<Value> {
    let entry = (
        vec(
            select(vec!["Day 1", "day 1", "B-roll", "Audio", "Day 1 2"]),
            0..=3,
        ),
        0..40_u32,
        0..3_u8,
        1..=90_i64,
        any::<bool>(),
    )
        .prop_map(|(dirs, n, kind, secs, audio)| {
            let (path, probed) = probed_file(n, kind, secs, audio);
            json!({"dirs": dirs, "register": {"path": path, "probe": probed.probe,
                   "content_hash": probed.content_hash}})
        });
    let valid = (
        option::weighted(0.3, one_of(p.folders.clone())),
        vec(entry, 1..=4),
    )
        .prop_map(|(folder, entries)| json!({"folder": folder, "entries": entries}))
        .boxed();
    let invalid = prop_oneof![
        Just(json!({"entries": []})),
        Just(json!({"entries": [{"dirs": ["a/b"], "register": {
            "path": "/m/a.mp4", "probe": ProbeInfo::default(), "content_hash": "h"}}]})),
        Just(json!({"entries": [{"dirs": [], "register": {
            "path": "relative.mp4", "probe": ProbeInfo::default(), "content_hash": "h"}}]})),
    ]
    .boxed();
    mix(valid, invalid)
}

fn media_import(p: &Pools) -> BoxedStrategy<Value> {
    // Unreadable files are not errors: the import reports them in `failed[]` and registers
    // the rest (WP-1D.07), so they belong with the valid requests.
    let unreadable = prop_oneof![
        8 => Just(None),
        1 => Just(Some(BROKEN_IMPORT_PATH)),
        1 => Just(Some("/import/missing.mp4")),
    ];
    let valid = (
        vec(select(IMPORT_PATHS.to_vec()), 1..=3),
        unreadable,
        option::weighted(0.3, one_of(p.folders.clone())),
        // Absent and `false` both leave the files where they are.
        option::weighted(0.2, Just(false)),
    )
        .prop_map(|(paths, unreadable, folder, copy)| {
            let mut unique: Vec<&str> = Vec::new();
            for p in paths.into_iter().chain(unreadable) {
                if !unique.contains(&p) {
                    unique.push(p);
                }
            }
            json!({"paths": unique, "folder": folder, "copy_to_project": copy})
        })
        .boxed();
    let invalid = prop_oneof![
        Just(json!({"paths": []})),
        // The harness document is untitled: it has no folder to copy into (WP-2F.04c).
        Just(json!({"paths": [IMPORT_PATHS[0]], "copy_to_project": true})),
        Just(json!({"paths": ["import/relative.mp4"]})),
        select(IMPORT_PATHS.to_vec()).prop_map(|p| json!({"paths": [p, p]})),
    ]
    .boxed();
    mix(valid, invalid)
}

fn media_slideshow(p: &Pools) -> BoxedStrategy<Value> {
    let mut files = SLIDE_PATHS.to_vec();
    // A still that `media.import` may have brought in already, a video (left out, in
    // `failed`) and a file that cannot be read.
    files.extend([IMPORT_PATHS[3], IMPORT_PATHS[0], BROKEN_IMPORT_PATH]);
    let video: Vec<Trk> = p
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Video)
        .cloned()
        .collect();
    let place = if video.is_empty() {
        Just((Value::Null, Value::Null, Value::Null)).boxed()
    } else {
        (
            select(video),
            any::<bool>(),
            option::weighted(0.5, 0..90_i64),
            any::<(u8, u32)>(),
        )
            .prop_map(|(t, named, past_end, (form, jitter))| {
                let at =
                    past_end.map_or(Value::Null, |n| time_json(t.fpf, t.end + n, form, jitter));
                if named {
                    (json!(t.seq), json!(t.id), at)
                } else {
                    (Value::Null, Value::Null, Value::Null)
                }
            })
            .boxed()
    };
    let valid = (
        subsequence(files, 1..=4),
        place,
        option::weighted(0.5, prop_oneof![Just("f:1"), Just("2s"), Just("0.5s")]),
        option::weighted(0.6, select(vec!["given", "name", "name_numeric"])),
    )
        .prop_map(|(files, (sequence, track, at), per_image, order)| {
            json!({"files": files, "sequence": sequence, "track": track, "at": at,
                   "per_image": per_image, "order": order})
        })
        .boxed();
    let audio = p
        .tracks
        .iter()
        .find(|t| t.kind == TrackKind::Audio)
        .map_or(Value::Null, |t| json!(t.id));
    let invalid = prop_oneof![
        Just(json!({"files": []})),
        Just(json!({"files": ["import/relative.png"]})),
        Just(json!({"files": [SLIDE_PATHS[0], SLIDE_PATHS[0]]})),
        Just(json!({"files": [BROKEN_IMPORT_PATH, IMPORT_PATHS[2]]})),
        Just(json!({"files": [SLIDE_PATHS[0]], "per_image": "0s"})),
        Just(json!({"files": [SLIDE_PATHS[1]], "order": "random"})),
        Just(json!({"files": [SLIDE_PATHS[1]], "track": audio})),
        fresh::<SequenceId>().prop_map(|s| json!({"files": [SLIDE_PATHS[2]], "sequence": s})),
        fresh::<TrackId>().prop_map(|t| json!({"files": [SLIDE_PATHS[2]], "track": t})),
    ]
    .boxed();
    mix(valid, invalid)
}

fn media_import_frame(p: &Pools) -> BoxedStrategy<Value> {
    let videos: Vec<MediaId> = p
        .media
        .iter()
        .filter(|m| crate::media::has_frames(m.kind))
        .map(|m| m.id)
        .collect();
    let source = if videos.is_empty() {
        Just(json!("sequence")).boxed()
    } else {
        prop_oneof![
            2 => Just(json!("sequence")),
            1 => select(videos).prop_map(|id| json!({"media": id})),
        ]
        .boxed()
    };
    let valid = (
        source,
        prop_oneof![Just(json!(0)), Just(json!("f:12")), Just(json!("1.5s"))],
        option::weighted(0.4, name()),
        option::weighted(0.3, one_of(p.folders.clone())),
    )
        .prop_map(|(source, at, name, folder)| {
            json!({"source": source, "at": at, "name": name, "folder": folder})
        })
        .boxed();
    let stills = p
        .media
        .iter()
        .find(|m| !crate::media::has_frames(m.kind))
        .map_or(json!("clip"), |m| json!({"media": m.id}));
    let invalid = prop_oneof![
        Just(json!({"source": "sequence", "at": "-1s"})),
        Just(json!({"source": "preview", "at": 0})),
        Just(json!({"source": stills, "at": 0})),
        bad_name().prop_map(|name| json!({"source": "sequence", "at": 0, "name": name})),
        fresh::<MediaId>().prop_map(|id| json!({"source": {"media": id}, "at": 0})),
    ]
    .boxed();
    mix(valid, invalid)
}

fn media_import_sequence(p: &Pools) -> BoxedStrategy<Value> {
    let mut places = SEQUENCE_PATHS.to_vec();
    places.push(SEQUENCE_FOLDER);
    let fps = prop_oneof![
        3 => Just(Value::Null),
        1 => Just(json!({"num": 24, "den": 1})),
        1 => Just(json!({"num": 30000, "den": 1001})),
    ];
    let valid = (
        select(places),
        fps,
        option::weighted(0.3, one_of(p.folders.clone())),
        // Absent and `false` both leave the pictures where they are.
        option::weighted(0.2, Just(false)),
    )
        .prop_map(|(path, fps, folder, copy)| {
            json!({"path": path, "fps": fps, "folder": folder, "copy_to_project": copy})
        })
        .boxed();
    let invalid = prop_oneof![
        Just(json!({"path": "import/render/shot_0001.png"})),
        // A still is not a run, and a folder without one is not either.
        Just(json!({"path": IMPORT_PATHS[3]})),
        Just(json!({"path": "/import"})),
        Just(json!({"path": "/import/render/shot_0004.png"})),
        Just(json!({"path": SEQUENCE_PATHS[0], "fps": {"num": 0, "den": 1}})),
        // The harness document is untitled: it has no folder to copy into (#215).
        Just(json!({"path": SEQUENCE_PATHS[0], "copy_to_project": true})),
    ]
    .boxed();
    mix(valid, invalid)
}

fn media_list(p: &Pools) -> BoxedStrategy<Value> {
    let filter = (
        option::weighted(0.5, one_of(p.folders.clone())),
        option::of(any::<bool>()),
        option::weighted(0.2, select(vec!["clip", "Clip 1", "interview"])),
        option::weighted(0.2, vec(tag(), 1..=2)),
        option::weighted(0.2, 1..=5_u8),
        option::weighted(0.3, one_of(p.collections.clone())),
    )
        .prop_map(|(folder, recursive, text, tags, min_rating, collection)| {
            json!({"folder": folder, "recursive": recursive, "text": text, "tags": tags,
                   "min_rating": min_rating, "collection": collection})
        });
    let valid = (
        option::of(1..=8_u32),
        option::of(one_of(p.media_ids())),
        option::weighted(0.4, filter),
        any::<bool>(),
    )
        .prop_map(|(limit, cursor, filter, note)| {
            let include = if note { json!(["note"]) } else { Value::Null };
            json!({"limit": limit, "cursor": cursor, "filter": filter, "include": include})
        })
        .boxed();
    let invalid = prop_oneof![
        Just(json!({"limit": 0})),
        Just(json!({"limit": 501})),
        fresh::<FolderId>().prop_map(|folder| json!({"filter": {"folder": folder}})),
    ]
    .boxed();
    mix(valid, invalid)
}

// ----- tags, ratings, notes and collections (WP-2F.06a, ADR-0036) -----------------------------

/// Tags that clash often (ignoring case and accents) and cover Unicode and padding.
const TAGS: [&str; 8] = [
    "interview",
    "Interview",
    "b-roll",
    "Café",
    "cafe",
    "  padded  ",
    "Day 1",
    "日本語",
];

fn tag() -> BoxedStrategy<String> {
    select(TAGS.to_vec()).prop_map(str::to_owned).boxed()
}

fn bad_tag() -> BoxedStrategy<String> {
    prop_oneof![
        Just(String::new()),
        Just("   ".to_owned()),
        Just("a,b".to_owned()),
        Just("tab\there".to_owned()),
        Just("x".repeat(65)),
    ]
    .boxed()
}

/// A few distinct assets (sometimes one that does not exist).
fn some_media(p: &Pools) -> BoxedStrategy<Vec<Value>> {
    vec(one_of(p.media_ids()), 1..=3)
        .prop_map(|media| {
            let mut unique: Vec<Value> = Vec::new();
            for m in media {
                if !unique.contains(&m) {
                    unique.push(m);
                }
            }
            unique
        })
        .boxed()
}

/// `media.add_tags` and `media.remove_tags`.
fn media_tags(p: &Pools) -> BoxedStrategy<Value> {
    let valid = (some_media(p), vec(tag(), 1..=3))
        .prop_map(|(media, tags)| json!({"media": media, "tags": tags}))
        .boxed();
    let invalid = prop_oneof![
        (some_media(p), bad_tag()).prop_map(|(media, tag)| json!({"media": media, "tags": [tag]})),
        some_media(p).prop_map(|media| json!({"media": media, "tags": []})),
        Just(json!({"media": [], "tags": ["x"]})),
        one_of(p.media_ids()).prop_map(|m| json!({"media": [m.clone(), m], "tags": ["x"]})),
    ]
    .boxed();
    mix(valid, invalid)
}

fn media_rename_tag() -> BoxedStrategy<Value> {
    let valid = (tag(), tag())
        .prop_map(|(from, to)| json!({"from": from, "to": to}))
        .boxed();
    let invalid = prop_oneof![
        (tag(), bad_tag()).prop_map(|(from, to)| json!({"from": from, "to": to})),
        (bad_tag(), tag()).prop_map(|(from, to)| json!({"from": from, "to": to})),
    ]
    .boxed();
    mix(valid, invalid)
}

fn media_set_rating(p: &Pools) -> BoxedStrategy<Value> {
    let valid = (some_media(p), option::weighted(0.8, 1..=5_u8))
        .prop_map(|(media, rating)| json!({"media": media, "rating": rating}))
        .boxed();
    let invalid = prop_oneof![
        (some_media(p), select(vec![0_u8, 6, 200]))
            .prop_map(|(media, rating)| json!({"media": media, "rating": rating})),
        Just(json!({"media": [], "rating": 3})),
    ]
    .boxed();
    mix(valid, invalid)
}

fn media_set_note(p: &Pools) -> BoxedStrategy<Value> {
    let note = prop_oneof![
        Just("Good take.\nUse the second half.".to_owned()),
        Just("  naïve mix  ".to_owned()),
        Just("   ".to_owned()),
    ];
    let valid = (one_of(p.media_ids()), option::weighted(0.8, note))
        .prop_map(|(media, note)| json!({"media": media, "note": note}))
        .boxed();
    let invalid = (
        one_of(p.media_ids()),
        prop_oneof![Just("bell\u{7}".to_owned()), Just("x".repeat(2001))],
    )
        .prop_map(|(media, note)| json!({"media": media, "note": note}))
        .boxed();
    mix(valid, invalid)
}

/// Collection names that clash often (ignoring case).
fn collection_name() -> BoxedStrategy<String> {
    select(vec![
        "Selects",
        "selects",
        "Best takes",
        "  Padded  ",
        "Unused interviews",
        "Música",
    ])
    .prop_map(str::to_owned)
    .boxed()
}

fn bad_collection_name() -> BoxedStrategy<String> {
    prop_oneof![
        Just(String::new()),
        Just("line\nbreak".to_owned()),
        Just("x".repeat(256)),
    ]
    .boxed()
}

/// A filter a smart collection can store (it names no collection).
fn smart_filter(p: &Pools) -> BoxedStrategy<Value> {
    (
        option::weighted(0.4, vec(tag(), 1..=2)),
        option::weighted(0.3, 1..=5_u8),
        option::weighted(0.3, any::<bool>()),
        option::weighted(0.3, select(vec!["video", "audio", "image"])),
        option::weighted(0.2, one_of(p.folders.clone())),
        option::weighted(0.2, select(vec!["clip", "interview", "naive"])),
    )
        .prop_map(|(tags, min_rating, used, kind, folder, text)| {
            let mut filter = serde_json::Map::new();
            let mut put = |key: &str, value: Value| {
                if !value.is_null() {
                    filter.insert(key.to_owned(), value);
                }
            };
            put("tags", json!(tags));
            put("min_rating", json!(min_rating));
            put("used", json!(used));
            put("kinds", kind.map_or(Value::Null, |k| json!([k])));
            put("folder", json!(folder));
            put("text", json!(text));
            Value::Object(filter)
        })
        .boxed()
}

fn media_create_collection(p: &Pools) -> BoxedStrategy<Value> {
    let kind = prop_oneof![
        Just(json!({"type": "manual"})),
        some_media(p).prop_map(|media| json!({"type": "manual", "media": media})),
        smart_filter(p).prop_map(|filter| json!({"type": "smart", "filter": filter})),
    ];
    let valid = (collection_name(), kind)
        .prop_map(|(name, kind)| json!({"name": name, "kind": kind}))
        .boxed();
    let invalid = prop_oneof![
        bad_collection_name().prop_map(|name| json!({"name": name, "kind": {"type": "manual"}})),
        one_of(p.collections.clone()).prop_map(|c| {
            json!({"name": "Nested", "kind": {"type": "smart", "filter": {"collection": c}}})
        }),
        Just(json!({"name": "Bad filter", "kind": {"type": "smart", "filter": {"min_rating": 9}}})),
        Just(json!({"name": "No kind"})),
    ]
    .boxed();
    mix(valid, invalid)
}

fn media_rename_collection(p: &Pools) -> BoxedStrategy<Value> {
    let valid = (one_of(p.collections.clone()), collection_name())
        .prop_map(|(collection, name)| json!({"collection": collection, "name": name}))
        .boxed();
    let invalid = (one_of(p.collections.clone()), bad_collection_name())
        .prop_map(|(collection, name)| json!({"collection": collection, "name": name}))
        .boxed();
    mix(valid, invalid)
}

fn media_delete_collection(p: &Pools) -> BoxedStrategy<Value> {
    let valid = one_of(p.collections.clone())
        .prop_map(|collection| json!({"collection": collection}))
        .boxed();
    let invalid = fresh::<CollectionId>()
        .prop_map(|collection| json!({"collection": collection}))
        .boxed();
    mix(valid, invalid)
}

/// `media.add_to_collection` and `media.remove_from_collection`: mostly a manual collection.
fn media_collection_members(p: &Pools) -> BoxedStrategy<Value> {
    let manual = if p.manual_collections.is_empty() {
        p.collections.clone()
    } else {
        p.manual_collections.clone()
    };
    let valid = (one_of(manual), some_media(p))
        .prop_map(|(collection, media)| json!({"collection": collection, "media": media}))
        .boxed();
    let invalid = prop_oneof![
        (one_of(p.collections.clone()), some_media(p))
            .prop_map(|(collection, media)| json!({"collection": collection, "media": media})),
        one_of(p.collections.clone()).prop_map(|c| json!({"collection": c, "media": []})),
        (one_of(p.collections.clone()), one_of(p.media_ids()))
            .prop_map(|(c, m)| json!({"collection": c, "media": [m.clone(), m]})),
    ]
    .boxed();
    mix(valid, invalid)
}

fn media_set_collection_filter(p: &Pools) -> BoxedStrategy<Value> {
    let smart: Vec<CollectionId> = p
        .collections
        .iter()
        .filter(|c| !p.manual_collections.contains(c))
        .copied()
        .collect();
    let target = if smart.is_empty() {
        p.collections.clone()
    } else {
        smart
    };
    let valid = (one_of(target), smart_filter(p))
        .prop_map(|(collection, filter)| json!({"collection": collection, "filter": filter}))
        .boxed();
    let invalid = prop_oneof![
        (one_of(p.collections.clone()), one_of(p.collections.clone()))
            .prop_map(|(c, other)| json!({"collection": c, "filter": {"collection": other}})),
        one_of(p.collections.clone())
            .prop_map(|c| json!({"collection": c, "filter": {"tags": ["a,b"]}})),
    ]
    .boxed();
    mix(valid, invalid)
}

// ----- media folders (WP-2F.07) --------------------------------------------------------------

/// Folder names that clash often (case-insensitively) and cover Unicode and padding.
const FOLDER_NAMES: [&str; 7] = [
    "B-roll",
    "b-ROLL",
    "Interviews",
    "Day 1",
    "Día 1 ✂",
    "  Padded  ",
    "New folder",
];

fn folder_name() -> BoxedStrategy<String> {
    prop_oneof![
        4 => select(FOLDER_NAMES.to_vec()).prop_map(str::to_owned),
        1 => (select(FOLDER_NAMES.to_vec()), 2..5_u32).prop_map(|(w, n)| format!("{w} {n}")),
    ]
    .boxed()
}

fn bad_folder_name() -> BoxedStrategy<String> {
    prop_oneof![
        Just(String::new()),
        Just("   ".to_owned()),
        Just("B-roll/Drone".to_owned()),
        Just("tab\tname".to_owned()),
        Just("x".repeat(256)),
    ]
    .boxed()
}

fn on_conflict() -> BoxedStrategy<Option<&'static str>> {
    option::of(select(vec!["fail", "rename"])).boxed()
}

fn media_create_folder(p: &Pools) -> BoxedStrategy<Value> {
    let valid = (
        option::weighted(0.6, one_of(p.folders.clone())),
        option::weighted(0.8, folder_name()),
        on_conflict(),
    )
        .prop_map(|(parent, name, on_conflict)| {
            json!({"parent": parent, "name": name, "on_conflict": on_conflict})
        })
        .boxed();
    let invalid = prop_oneof![
        bad_folder_name().prop_map(|name| json!({"name": name})),
        fresh::<FolderId>().prop_map(|parent| json!({"parent": parent})),
        Just(json!({"on_conflict": "merge"})),
    ]
    .boxed();
    mix(valid, invalid)
}

fn media_rename_folder(p: &Pools) -> BoxedStrategy<Value> {
    let valid = (one_of(p.folders.clone()), folder_name())
        .prop_map(|(folder, name)| json!({"folder": folder, "name": name}))
        .boxed();
    let invalid = (one_of(p.folders.clone()), bad_folder_name())
        .prop_map(|(folder, name)| json!({"folder": folder, "name": name}))
        .boxed();
    mix(valid, invalid)
}

fn media_rename(p: &Pools) -> BoxedStrategy<Value> {
    let display = prop_oneof![
        Just("Interview A".to_owned()),
        Just("Take 1/2 ✂".to_owned()),
        Just("  Padded  ".to_owned()),
    ];
    let valid = (one_of(p.media_ids()), option::weighted(0.7, display))
        .prop_map(|(media, name)| json!({"media": media, "name": name}))
        .boxed();
    let invalid = (
        one_of(p.media_ids()),
        prop_oneof![
            Just(String::new()),
            Just("line\nbreak".to_owned()),
            Just("x".repeat(256)),
        ],
    )
        .prop_map(|(media, name)| json!({"media": media, "name": name}))
        .boxed();
    mix(valid, invalid)
}

fn media_move(p: &Pools) -> BoxedStrategy<Value> {
    let valid = (
        vec(one_of(p.media_ids()), 0..=3),
        vec(one_of(p.folders.clone()), 0..=2),
        option::weighted(0.6, one_of(p.folders.clone())),
        on_conflict(),
    )
        .prop_map(|(media, folders, to, on_conflict)| {
            let unique = |ids: Vec<Value>| {
                let mut out: Vec<Value> = Vec::new();
                for id in ids {
                    if !out.contains(&id) {
                        out.push(id);
                    }
                }
                out
            };
            let (media, folders) = (unique(media), unique(folders));
            json!({"media": media, "folders": folders, "to": to, "on_conflict": on_conflict})
        })
        .boxed();
    let invalid = prop_oneof![
        Just(json!({})),
        Just(json!({"media": [], "folders": []})),
        one_of(p.media_ids()).prop_map(|m| json!({"media": [m.clone(), m]})),
        one_of(p.folders.clone()).prop_map(|f| json!({"folders": [f.clone()], "to": f})),
        fresh::<FolderId>().prop_map(|to| json!({"media": [], "folders": [], "to": to})),
    ]
    .boxed();
    mix(valid, invalid)
}

fn media_delete_folder(p: &Pools) -> BoxedStrategy<Value> {
    let valid = (
        one_of(p.folders.clone()),
        prop_oneof![Just("move_up"), Just("remove")],
    )
        .prop_map(|(folder, contents)| json!({"folder": folder, "contents": contents}))
        .boxed();
    let invalid = prop_oneof![
        fresh::<FolderId>().prop_map(|folder| json!({"folder": folder, "contents": "remove"})),
        one_of(p.folders.clone()).prop_map(|folder| json!({"folder": folder, "contents": "keep"})),
    ]
    .boxed();
    mix(valid, invalid)
}

fn media_remove(p: &Pools) -> BoxedStrategy<Value> {
    let (used, unused): (Vec<&Med>, Vec<&Med>) = p.media.iter().partition(|m| m.used);
    let ids = |ms: &[&Med]| ms.iter().map(|m| m.id).collect::<Vec<_>>();
    let valid = one_of(if unused.is_empty() {
        p.media_ids()
    } else {
        ids(&unused)
    });
    mix(
        valid.prop_map(|id| json!({"id": id})).boxed(),
        one_of(ids(&used)).prop_map(|id| json!({"id": id})).boxed(),
    )
}

/// `{media: [ids]}` for the proxy commands (WP-2F.01): known or unknown assets, an empty or
/// repeated list; `media.proxy_status` may leave the list out.
fn proxy_media(p: &Pools, optional: bool) -> BoxedStrategy<Value> {
    let valid = prop_oneof![
        4 => one_of(p.proxy_ids()).prop_map(|id| json!({ "media": [id] })),
        1 => vec(one_of(p.media_ids()), 1..=3).prop_map(|ids| json!({ "media": ids })),
    ]
    .boxed();
    let invalid = prop_oneof![
        Just(json!({ "media": [] })),
        one_of(p.media_ids()).prop_map(|id| json!({ "media": [id.clone(), id] })),
    ]
    .boxed();
    if optional {
        prop_oneof![8 => mix(valid, invalid), 2 => Just(json!({}))].boxed()
    } else {
        mix(valid, invalid)
    }
}

/// `media.set_proxy_policy` (ADR-0027): each policy, or a word that is none.
fn proxy_policy() -> BoxedStrategy<Value> {
    let valid = select(vec!["off", "auto", "all"])
        .prop_map(|policy| json!({ "policy": policy }))
        .boxed();
    let invalid = prop_oneof![Just(json!({ "policy": "some" })), Just(json!({}))].boxed();
    mix(valid, invalid)
}

/// `media.set_proxy_mode` (ADR-0027): a mode for known or unknown assets, an empty or repeated
/// list.
fn proxy_mode(p: &Pools) -> BoxedStrategy<Value> {
    let mode = || select(vec!["auto", "force", "off"]);
    let valid = prop_oneof![
        4 => (one_of(p.proxy_ids()), mode())
            .prop_map(|(id, mode)| json!({ "media": [id], "mode": mode })),
        1 => (vec(one_of(p.media_ids()), 1..=3), mode())
            .prop_map(|(ids, mode)| json!({ "media": ids, "mode": mode })),
    ]
    .boxed();
    let invalid = prop_oneof![
        mode().prop_map(|mode| json!({ "media": [], "mode": mode })),
        one_of(p.media_ids()).prop_map(|id| json!({ "media": [id.clone(), id], "mode": "off" })),
        one_of(p.media_ids()).prop_map(|id| json!({ "media": [id], "mode": "never" })),
    ]
    .boxed();
    mix(valid, invalid)
}

/// `project.collect` and its estimate: a destination no test writes media into (the
/// harness's documents name files that do not exist, so a collect with media is refused).
fn project_collect() -> BoxedStrategy<Value> {
    let valid = (
        option::of(select(vec!["copy_all", "copy_external_only"])),
        option::of(any::<bool>()),
        option::of(any::<bool>()),
    )
        .prop_map(|(mode, trim, generated)| {
            json!({"dest": "/collect/out", "mode": mode, "trim_unused": trim,
                   "include_generated": generated})
        })
        .boxed();
    let invalid = prop_oneof![
        Just(json!({"dest": "collect/relative"})),
        Just(json!({"dest": "/collect/out", "mode": "everything"})),
    ]
    .boxed();
    mix(valid, invalid)
}

/// `project.archive`: like a collect, nothing is written (the harness's project write writes
/// nothing, so the job ends before it makes a file).
fn project_archive() -> BoxedStrategy<Value> {
    let valid = (
        select(vec![
            ("folder", "/collect/archive"),
            ("zip", "/collect/archive.zip"),
        ]),
        option::of(any::<bool>()),
        option::of(any::<bool>()),
    )
        .prop_map(|((format, dest), trim, generated)| {
            json!({"dest": dest, "format": format, "trim_unused": trim,
                   "include_generated": generated})
        })
        .boxed();
    let invalid = prop_oneof![
        Just(json!({"dest": "collect/relative", "format": "folder"})),
        Just(json!({"dest": "/collect/archive", "format": "zip"})),
        Just(json!({"dest": "/collect/archive.zip", "format": "tar"})),
        Just(json!({"dest": "/collect/archive"})),
    ]
    .boxed();
    mix(valid, invalid)
}

/// The engine's part of a collect in a harness: an unknown amount of free space, no project
/// anywhere, and a project write that writes nothing.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoDisk;

impl crate::project::collect::CollectHost for NoDisk {
    fn free_space(&self, _folder: &std::path::Path) -> Option<u64> {
        None
    }

    fn holds_project(&self, _folder: &std::path::Path) -> bool {
        false
    }

    fn write_project(
        &self,
        folder: &std::path::Path,
        _doc: &Project,
    ) -> Result<std::path::PathBuf, String> {
        Ok(folder.join("copy.cutweave"))
    }

    fn hasher(&self) -> Box<dyn crate::project::collect::ContentHasher> {
        Box::new(NoHash)
    }
}

/// The hasher of [`NoDisk`]: every file hashes to nothing.
struct NoHash;

impl crate::project::collect::ContentHasher for NoHash {
    fn update(&mut self, _bytes: &[u8]) {}

    fn finish(self: Box<Self>) -> String {
        String::new()
    }
}

/// `media.set_location`: an asset at a new path, sometimes with the content of another file.
fn media_set_location(p: &Pools) -> BoxedStrategy<Value> {
    let valid = (
        one_of(p.media_ids()),
        0..40_u32,
        option::weighted(0.3, 0..3_u8),
    )
        .prop_map(|(media, n, other)| match other {
            None => json!({"media": media, "path": format!("/media/moved/{n}.mp4")}),
            Some(kind) => {
                let (path, probed) = probed_file(n, kind, 90, true);
                json!({"media": media, "path": path, "probe": probed.probe,
                       "content_hash": probed.content_hash})
            }
        })
        .boxed();
    let invalid = one_of(p.media_ids())
        .prop_flat_map(|media| {
            prop_oneof![
                Just(json!({"media": media.clone(), "path": "moved/relative.mp4"})),
                Just(json!({"media": media, "path": "/media/moved/a.mp4",
                            "content_hash": "has space"})),
            ]
        })
        .boxed();
    mix(valid, invalid)
}

/// `media.relink`: an asset and one of the files the harness's disk holds, or one it does not.
fn media_relink(p: &Pools) -> BoxedStrategy<Value> {
    let path = prop_oneof![
        8 => select(IMPORT_PATHS.to_vec()),
        1 => Just(BROKEN_IMPORT_PATH),
        1 => Just("/import/missing.mp4"),
    ];
    let valid = (one_of(p.media_ids()), path)
        .prop_map(|(media, path)| json!({"media": media, "path": path}))
        .boxed();
    let invalid = one_of(p.media_ids())
        .prop_map(|media| json!({"media": media, "path": "import/relative.mp4"}))
        .boxed();
    mix(valid, invalid)
}

/// `media.relink_folder`: the folder of [`IMPORT_PATHS`], for every missing asset or a few.
fn media_relink_folder(p: &Pools) -> BoxedStrategy<Value> {
    let media = option::weighted(0.3, vec(one_of(p.media_ids()), 1..=3));
    let valid = (media, option::of(any::<bool>()))
        .prop_map(|(media, recursive)| match media {
            None => json!({"folder": "/import", "recursive": recursive}),
            Some(media) => json!({"folder": "/import", "recursive": recursive, "media": media}),
        })
        .boxed();
    let invalid = prop_oneof![
        Just(json!({"folder": "import"})),
        Just(json!({"folder": "/not-a-folder"})),
        Just(json!({"folder": "/import", "media": []})),
    ]
    .boxed();
    mix(valid, invalid)
}

/// The disk a harness gives the relink commands: the files [`fake_probe`] knows.
#[must_use]
pub fn fake_files() -> crate::media::MemoryFiles {
    IMPORT_PATHS
        .iter()
        .filter(|p| p.starts_with('/'))
        .fold(crate::media::MemoryFiles::new(), |files, path| {
            files.with_file(path)
        })
}

// ----- sequences -----------------------------------------------------------------------------

const PRESET_IDS: [&str; 6] = ["16:9", "16:9-4k", "9:16", "1:1", "4:5", "21:9"];

fn sequence_create() -> BoxedStrategy<Value> {
    let format = (
        option::of(16..=2000_u32),
        option::of(16..=2000_u32),
        option::of(select(FrameRate::COMMON.to_vec())),
        option::of(select(vec!["#000000", "#ff8800", "#1a2b3c80"])),
    )
        .prop_map(|(width, height, frame_rate, background)| {
            json!({"width": width, "height": height, "frame_rate": frame_rate,
                   "background": background})
        });
    let valid = (
        name(),
        option::weighted(0.6, select(PRESET_IDS.to_vec())),
        option::weighted(0.3, format),
    )
        .prop_map(
            |(name, preset, format)| json!({"name": name, "preset": preset, "format": format}),
        )
        .boxed();
    let invalid = prop_oneof![
        bad_name().prop_map(|name| json!({"name": name})),
        Just(json!({"name": "S", "preset": "2:3"})),
        Just(json!({"name": "S", "format": {"width": 0}})),
        Just(json!({"name": "S", "format": {"background": "red"}})),
        Just(json!({"name": "S", "format": {"frame_rate": {"num": 1001, "den": 7}}})),
    ]
    .boxed();
    mix(valid, invalid)
}

fn sequence_create_from_media(p: &Pools) -> BoxedStrategy<Value> {
    let valid = (
        proptest::collection::vec(one_of(p.media_ids()), 1..4),
        option::weighted(0.3, name()),
    )
        .prop_map(|(media, name)| json!({"media": media, "name": name}))
        .boxed();
    let invalid = prop_oneof![
        Just(json!({"media": []})),
        bad_name().prop_map(|name| json!({"media": [], "name": name})),
    ]
    .boxed();
    mix(valid, invalid)
}

fn sequence_duplicate(p: &Pools) -> BoxedStrategy<Value> {
    let valid = (one_of(p.seqs.clone()), option::weighted(0.3, name()))
        .prop_map(|(sequence, name)| json!({"sequence": sequence, "name": name}))
        .boxed();
    let invalid = (one_of(p.seqs.clone()), bad_name())
        .prop_map(|(sequence, name)| json!({"sequence": sequence, "name": name}))
        .boxed();
    mix(valid, invalid)
}

fn sequence_rename(p: &Pools) -> BoxedStrategy<Value> {
    let valid = (one_of(p.seqs.clone()), name())
        .prop_map(|(sequence, name)| json!({"sequence": sequence, "name": name}))
        .boxed();
    let invalid = (one_of(p.seqs.clone()), bad_name())
        .prop_map(|(sequence, name)| json!({"sequence": sequence, "name": name}))
        .boxed();
    mix(valid, invalid)
}

fn sequence_delete(p: &Pools) -> BoxedStrategy<Value> {
    let valid = one_of(p.seqs.clone())
        .prop_map(|sequence| json!({"sequence": sequence}))
        .boxed();
    mix(valid.clone(), valid)
}

fn sequence_set_format(p: &Pools) -> BoxedStrategy<Value> {
    let valid = (
        one_of(p.seqs.clone()),
        option::weighted(0.6, 16..=2000_u32),
        option::weighted(0.6, 16..=2000_u32),
        option::weighted(0.5, select(FrameRate::COMMON.to_vec())),
        option::weighted(0.2, select(SampleRate::COMMON.to_vec())),
        option::weighted(0.3, select(vec!["#000000", "#336699"])),
        option::weighted(0.7, select(vec!["fit", "fill", "keep_scale", "none"])),
    )
        .prop_map(
            |(sequence, width, height, frame_rate, sample_rate, background, reframe)| {
                json!({"sequence": sequence, "width": width, "height": height,
                       "frame_rate": frame_rate, "sample_rate": sample_rate,
                       "background": background, "reframe": reframe})
            },
        )
        .boxed();
    let invalid = (
        one_of(p.seqs.clone()),
        prop_oneof![
            Just(json!({"width": 0})),
            Just(json!({"height": 20_000})),
            Just(json!({"frame_rate": {"num": 1001, "den": 7}})),
            Just(json!({"sample_rate": 1234})),
            Just(json!({"reframe": "stretch"})),
        ],
    )
        .prop_map(|(sequence, mut params)| {
            params["sequence"] = json!(sequence);
            params
        })
        .boxed();
    mix(valid, invalid)
}

fn sequence_list() -> BoxedStrategy<Value> {
    prop_oneof![
        3 => Just(json!({})),
        1 => Just(json!({"presets": true})),
        1 => Just(json!({"unexpected": 1})),
    ]
    .boxed()
}

// ----- tracks, timeline ----------------------------------------------------------------------

fn track_add(p: &Pools) -> BoxedStrategy<Value> {
    let kind = prop_oneof![5 => Just("video"), 4 => Just("audio")];
    let valid = (
        one_of(p.seqs.clone()),
        kind,
        option::weighted(0.3, name()),
        option::weighted(0.3, 0..=count(p.tracks.len()) + 1),
    )
        .prop_map(|(sequence, kind, name, index)| {
            json!({"sequence": sequence, "kind": kind, "name": name, "index": index})
        })
        .boxed();
    let invalid = (
        one_of(p.seqs.clone()),
        select(vec!["text", "caption", "adjustment", "video"]),
        bad_name(),
    )
        .prop_map(
            |(sequence, kind, name)| json!({"sequence": sequence, "kind": kind, "name": name}),
        )
        .boxed();
    mix(valid, invalid)
}

fn track_remove(p: &Pools) -> BoxedStrategy<Value> {
    mix(
        one_of(p.track_ids())
            .prop_map(|track| json!({"track": track}))
            .boxed(),
        prop_oneof![fresh::<TrackId>(), one_of(p.item_ids())]
            .prop_map(|track| json!({"track": track}))
            .boxed(),
    )
}

fn track_set(p: &Pools) -> BoxedStrategy<Value> {
    let locked = prop_oneof![2 => Just(false), 1 => Just(true)];
    let valid = (
        one_of(p.track_ids()),
        option::weighted(0.2, name()),
        option::weighted(0.3, any::<bool>()),
        option::weighted(0.3, any::<bool>()),
        option::weighted(0.4, locked),
        option::weighted(0.2, any::<bool>()),
        option::weighted(0.3, gain()),
    )
        .prop_map(|(track, name, muted, hidden, locked, solo, gain)| {
            json!({"track": track, "name": name, "muted": muted, "hidden": hidden,
                   "locked": locked, "solo": solo, "gain": gain})
        })
        .boxed();
    let invalid = (
        one_of(p.track_ids()),
        prop_oneof![
            Just(json!({})),
            Just(json!({"gain": 100.0})),
            Just(json!({"gain": -200.0})),
            bad_name().prop_map(|name| json!({"name": name})),
        ],
    )
        .prop_map(|(track, mut v)| {
            v["track"] = track;
            v
        })
        .boxed();
    mix(valid, invalid)
}

fn track_reorder(p: &Pools) -> BoxedStrategy<Value> {
    let valid = (one_of(p.track_ids()), 0..=count(p.tracks.len()) + 1)
        .prop_map(|(track, index)| json!({"track": track, "index": index}))
        .boxed();
    let invalid = (fresh::<TrackId>(), 0..3_u32)
        .prop_map(|(track, index)| json!({"track": track, "index": index}))
        .boxed();
    mix(valid, invalid)
}

fn track_set_height(p: &Pools) -> BoxedStrategy<Value> {
    let valid = (
        one_of(p.track_ids()),
        option::weighted(0.8, prop_oneof![Just(8_u32), Just(512), 8..=512_u32]),
    )
        .prop_map(|(track, height)| json!({"track": track, "height": height}))
        .boxed();
    let invalid = (
        one_of(p.track_ids()),
        prop_oneof![Just(0_u32), Just(7), Just(513), Just(100_000)],
    )
        .prop_map(|(track, height)| json!({"track": track, "height": height}))
        .boxed();
    mix(valid, invalid)
}

fn track_set_magnetic(p: &Pools) -> BoxedStrategy<Value> {
    let valid = (
        one_of(p.track_ids()),
        prop_oneof![3 => Just(true), 1 => Just(false)],
        option::weighted(0.6, any::<bool>()),
    )
        .prop_map(|(track, magnetic, close_gaps)| {
            json!({"track": track, "magnetic": magnetic, "close_gaps": close_gaps})
        })
        .boxed();
    let invalid = (fresh::<TrackId>(), any::<bool>())
        .prop_map(|(track, magnetic)| json!({"track": track, "magnetic": magnetic}))
        .boxed();
    mix(valid, invalid)
}

fn track_list(p: &Pools) -> BoxedStrategy<Value> {
    mix(
        one_of(p.seqs.clone())
            .prop_map(|sequence| json!({"sequence": sequence}))
            .boxed(),
        fresh::<SequenceId>()
            .prop_map(|sequence| json!({"sequence": sequence}))
            .boxed(),
    )
}

const LABELS: [&str; 4] = ["", "Intro", "Beat \u{266a}", "  Chapter 1  "];

fn timeline_add_marker(p: &Pools) -> BoxedStrategy<Value> {
    let invalid_seq = fresh::<SequenceId>()
        .prop_map(|sequence| json!({"sequence": sequence, "t": 0}))
        .boxed();
    if p.seq_fpf.is_empty() {
        return mix(invalid_seq.clone(), invalid_seq);
    }
    let items = p.items.clone();
    let valid = select(p.seq_fpf.clone())
        .prop_flat_map(move |(seq, fpf)| {
            let mine: Vec<ItemId> = items
                .iter()
                .filter(|i| i.seq == seq)
                .map(|i| i.id)
                .collect();
            let item = if mine.is_empty() {
                Just(None).boxed()
            } else {
                option::weighted(0.4, select(mine)).boxed()
            };
            (
                Just((seq, fpf)),
                item,
                0..200_i64,
                any::<(u8, u32)>(),
                option::weighted(0.3, 1..60_i64),
                option::weighted(0.6, select(LABELS.to_vec())),
                option::weighted(0.3, Just("#336699")),
                option::weighted(0.4, select(vec!["user", "beat", "chapter"])),
            )
        })
        .prop_map(
            |((seq, fpf), item, frame, (form, jitter), dur, label, color, kind)| {
                json!({
                    "sequence": seq, "item": item, "t": time_json(fpf, frame, form, jitter),
                    "duration": dur.map(|d| time_json(fpf, d, form, 0)),
                    "label": label, "color": color, "kind": kind,
                })
            },
        )
        .boxed();
    let seq = select(p.seq_fpf.clone()).prop_map(|(s, _)| s);
    let invalid = (
        seq,
        prop_oneof![
            Just(json!({"t": "-1.0s"})),
            Just(json!({"t": 0, "duration": 0})),
            Just(json!({"t": 0, "label": "x".repeat(201)})),
            Just(json!({"t": 0, "label": "tab\there"})),
            Just(json!({"t": 0, "color": "red"})),
            Just(json!({"t": 0, "kind": "bar"})),
        ],
    )
        .prop_map(|(sequence, mut v)| {
            v["sequence"] = json!(sequence);
            v
        })
        .boxed();
    mix(valid, prop_oneof![3 => invalid, 1 => invalid_seq].boxed())
}

fn timeline_move_marker(p: &Pools) -> BoxedStrategy<Value> {
    let valid = (one_of(p.markers.clone()), 0..200_i64, any::<(u8, u32)>())
        .prop_map(|(marker, frame, (form, jitter))| {
            json!({"marker": marker, "t": time_json(FPF_30, frame, form, jitter)})
        })
        .boxed();
    let invalid = (one_of(p.markers.clone()), Just("-2.0s"))
        .prop_map(|(marker, t)| json!({"marker": marker, "t": t}))
        .boxed();
    mix(valid, invalid)
}

/// `timeline.update_marker` (issue #222): some of label, color, length and kind; sometimes
/// none of them, a label that is too long or a negative length.
fn timeline_update_marker(p: &Pools) -> BoxedStrategy<Value> {
    let valid = (
        one_of(p.markers.clone()),
        option::weighted(0.6, select(LABELS.to_vec())),
        option::weighted(0.4, select(vec!["#336699", "#f2b134"])),
        option::weighted(0.4, 0..60_i64),
        option::weighted(0.4, select(vec!["user", "beat", "chapter"])),
    )
        .prop_map(|(id, label, color, dur, kind)| {
            json!({
                "id": id, "label": label, "color": color,
                "duration": dur.map(|d| time_json(FPF_30, d, 0, 0)),
                "kind": kind.unwrap_or(if label.is_none() && color.is_none() && dur.is_none() {
                    "chapter"
                } else {
                    "user"
                }),
            })
        })
        .boxed();
    let invalid = (
        one_of(p.markers.clone()),
        prop_oneof![
            Just(json!({})),
            Just(json!({"label": "x".repeat(201)})),
            Just(json!({"duration": "-1.0s"})),
            Just(json!({"color": "red"})),
        ],
    )
        .prop_map(|(id, mut v)| {
            v["id"] = id;
            v
        })
        .boxed();
    mix(valid, invalid)
}

fn timeline_remove_marker(p: &Pools) -> BoxedStrategy<Value> {
    mix(
        one_of(p.markers.clone())
            .prop_map(|marker| json!({"marker": marker}))
            .boxed(),
        fresh::<MarkerId>()
            .prop_map(|marker| json!({"marker": marker}))
            .boxed(),
    )
}

fn timeline_set_in_out(p: &Pools) -> BoxedStrategy<Value> {
    if p.seq_fpf.is_empty() {
        let bad = fresh::<SequenceId>()
            .prop_map(|sequence| json!({"sequence": sequence, "start": 0, "end": 10}))
            .boxed();
        return mix(bad.clone(), bad);
    }
    let valid = (
        select(p.seq_fpf.clone()),
        0..150_i64,
        1..100_i64,
        any::<(u8, u32)>(),
    )
        .prop_map(|((seq, fpf), start, len, (form, jitter))| {
            json!({"sequence": seq,
                   "start": time_json(fpf, start, form, jitter),
                   "end": time_json(fpf, start + len, form, 0)})
        })
        .boxed();
    let invalid = (
        select(p.seq_fpf.clone()),
        prop_oneof![
            Just(json!({"start": 30, "end": 30})),
            Just(json!({"start": 30, "end": 10})),
            Just(json!({"start": "-1.0s", "end": 10})),
        ],
    )
        .prop_map(|((sequence, _), mut v)| {
            v["sequence"] = json!(sequence);
            v
        })
        .boxed();
    mix(valid, invalid)
}

fn timeline_clear_in_out(p: &Pools) -> BoxedStrategy<Value> {
    mix(
        one_of(p.seqs.clone())
            .prop_map(|sequence| json!({"sequence": sequence}))
            .boxed(),
        fresh::<SequenceId>()
            .prop_map(|sequence| json!({"sequence": sequence}))
            .boxed(),
    )
}

fn timeline_list_markers(p: &Pools) -> BoxedStrategy<Value> {
    let valid = (
        one_of(p.seqs.clone()),
        option::weighted(0.3, one_of(p.item_ids())),
    )
        .prop_map(|(sequence, item)| json!({"sequence": sequence, "item": item}))
        .boxed();
    let invalid = fresh::<SequenceId>()
        .prop_map(|sequence| json!({"sequence": sequence}))
        .boxed();
    mix(valid, invalid)
}

/// `audio.get_levels`: a sequence, sometimes a range and the limiter switch; sometimes a range
/// that is empty or past the end, or an unknown sequence.
fn audio_get_levels(p: &Pools) -> BoxedStrategy<Value> {
    let valid = (
        one_of(p.seqs.clone()),
        option::of(select(vec![("0s", "1s"), ("0.5s", "f:30"), ("f:0", "2s")])),
        option::of(any::<bool>()),
    )
        .prop_map(|(sequence, range, limiter)| {
            let mut v = json!({"sequence": sequence, "limiter": limiter});
            if let Some((start, end)) = range {
                v["in"] = json!(start);
                v["out"] = json!(end);
            }
            v
        })
        .boxed();
    let invalid = prop_oneof![
        fresh::<SequenceId>().prop_map(|sequence| json!({"sequence": sequence})),
        one_of(p.seqs.clone())
            .prop_map(|sequence| json!({"sequence": sequence, "in": "2s", "out": "1s"})),
        one_of(p.seqs.clone()).prop_map(|sequence| json!({"sequence": sequence, "out": "99999s"})),
    ]
    .boxed();
    mix(valid, invalid)
}

/// A track and a media file it can play.
#[derive(Clone, Debug)]
struct InsertTarget {
    seq: SequenceId,
    track: TrackId,
    fpf: i64,
    end: i64,
    media: MediaId,
    /// The media length in frames, when clips are bounded by it.
    frames: Option<i64>,
}

fn timeline_insert(p: &Pools) -> BoxedStrategy<Value> {
    let mut targets = Vec::new();
    for t in &p.tracks {
        for m in p.media.iter().filter(|m| t.kind.accepts(m.kind)) {
            targets.push(InsertTarget {
                seq: t.seq,
                track: t.id,
                fpf: t.fpf,
                end: t.end,
                media: m.id,
                frames: m.flicks.map(|f| (f / t.fpf).max(1)),
            });
        }
    }
    let invalid = any_insert(p);
    if targets.is_empty() {
        return mix(invalid.clone(), invalid);
    }
    let audio: Vec<TrackId> = p
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Audio)
        .map(|t| t.id)
        .collect();
    let audio_track = if audio.is_empty() {
        Just(None).boxed()
    } else {
        option::weighted(0.15, select(audio)).boxed()
    };
    let valid = (select(targets), audio_track)
        .prop_flat_map(|(t, audio_track)| {
            let max = t.frames.unwrap_or(300);
            (
                Just((t.clone(), audio_track)),
                prop_oneof![3 => t.end..t.end + 30, 1 => 0..t.end + 60],
                option::weighted(0.3, 0..max),
                option::weighted(0.6, 1..=max),
                any::<(u8, u8, u8, u32)>(),
                option::weighted(0.2, prop_oneof![Just("reject"), Just("new_track")]),
                option::weighted(0.2, any::<bool>()),
                option::weighted(0.4, select(MODES.to_vec())),
                option::weighted(0.15, any::<bool>()),
            )
        })
        .prop_map(
            |((t, audio_track), at, src, dur, (fa, fs, fd, jitter), overlap, link, mode, all)| {
                // Keep both given ends inside the media most of the time.
                let dur = match (src, dur, t.frames) {
                    (Some(s), Some(d), Some(m)) if s + d > m => Some((m - s).max(1)),
                    (_, d, _) => d,
                };
                json!({
                    "sequence": t.seq, "track": t.track, "media": t.media,
                    "at": time_json(t.fpf, at, fa, jitter),
                    "source_in": src.map(|s| time_json(t.fpf, s, fs, 0)),
                    "duration": dur.map(|d| time_json(t.fpf, d, fd, jitter)),
                    "overlap": overlap, "link": link, "mode": mode,
                    "audio_track": audio_track, "ripple_all_tracks": all,
                })
            },
        )
        .boxed();
    mix(valid, invalid)
}

/// The placement modes (timeline spec §3.6).
const MODES: [&str; 4] = ["insert", "overwrite", "new_track", "reject"];

/// `timeline.overwrite`: `timeline.insert`'s params without the mode and its alias.
fn timeline_overwrite(p: &Pools) -> BoxedStrategy<Value> {
    timeline_insert(p)
        .prop_map(|mut v| {
            if let Value::Object(map) = &mut v {
                for key in ["overlap", "mode", "ripple_all_tracks"] {
                    map.remove(key);
                }
            }
            v
        })
        .boxed()
}

/// `timeline.lift` and `timeline.extract`: ranges around the items, some tracks, or the in/out.
fn timeline_ranges(p: &Pools) -> BoxedStrategy<Value> {
    let invalid_seq = fresh::<SequenceId>()
        .prop_map(|sequence| json!({"sequence": sequence, "ranges": [{"start": 0, "end": 10}]}))
        .boxed();
    if p.seq_fpf.is_empty() {
        return mix(invalid_seq.clone(), invalid_seq);
    }
    let tracks: Arc<[Trk]> = p.tracks.clone().into();
    let valid = select(p.seq_fpf.clone())
        .prop_flat_map(move |(seq, fpf)| {
            let mine: Vec<TrackId> = tracks
                .iter()
                .filter(|t| t.seq == seq)
                .map(|t| t.id)
                .collect();
            let end = tracks
                .iter()
                .filter(|t| t.seq == seq)
                .map(|t| t.end)
                .max()
                .unwrap_or(0);
            let pick = if mine.is_empty() {
                Just(None).boxed()
            } else {
                option::weighted(0.4, subsequence(mine.clone(), 1..=mine.len())).boxed()
            };
            (
                Just((seq, fpf)),
                option::weighted(0.85, vec((0..end + 20, 1..40_i64), 1..=3)),
                pick,
                any::<(u8, u32)>(),
            )
        })
        .prop_map(|((seq, fpf), ranges, tracks, (form, jitter))| {
            let ranges = ranges.map(|rs| {
                rs.into_iter()
                    .map(|(s, len)| {
                        json!({"start": time_json(fpf, s, form, jitter),
                               "end": time_json(fpf, s + len, form, 0)})
                    })
                    .collect::<Vec<_>>()
            });
            json!({"sequence": seq, "ranges": ranges, "tracks": tracks})
        })
        .boxed();
    let invalid = (
        select(p.seq_fpf.clone()),
        prop_oneof![
            Just(json!({"ranges": []})),
            Just(json!({"ranges": [{"start": 30, "end": 30}]})),
            Just(json!({"ranges": [{"start": "-1.0s", "end": 10}]})),
            Just(json!({"tracks": []})),
            fresh::<TrackId>().prop_map(|t| json!({"tracks": [t]})),
        ],
    )
        .prop_map(|((sequence, _), mut v)| {
            v["sequence"] = json!(sequence);
            v
        })
        .boxed();
    mix(valid, prop_oneof![3 => invalid, 1 => invalid_seq].boxed())
}

fn ripple_delete(p: &Pools) -> BoxedStrategy<Value> {
    let invalid = prop_oneof![
        Just(json!({"items": []})),
        fresh::<ItemId>().prop_map(|item| json!({"items": [item]})),
        vec(one_of(p.item_ids()), 2..=3).prop_map(|items| json!({"items": items})),
    ]
    .boxed();
    if p.items.is_empty() {
        return mix(invalid.clone(), invalid);
    }
    // Items of one track: one item, or a few of the same track.
    let by_track: Vec<Vec<ItemId>> = p
        .tracks
        .iter()
        .map(|t| {
            p.items
                .iter()
                .filter(|i| i.track == t.id)
                .map(|i| i.id)
                .collect::<Vec<_>>()
        })
        .filter(|v| !v.is_empty())
        .collect();
    let valid = (select(by_track), option::weighted(0.2, any::<bool>()))
        .prop_flat_map(|(items, all)| {
            let n = items.len();
            (subsequence(items, 1..=n.min(3)), Just(all))
        })
        .prop_map(|(items, all)| json!({"items": items, "ripple_all_tracks": all}))
        .boxed();
    mix(valid, invalid)
}

fn delete_gap(p: &Pools) -> BoxedStrategy<Value> {
    let invalid = (fresh::<TrackId>(), 0..50_i64)
        .prop_map(|(track, at)| json!({"track": track, "at": time_json(FPF_30, at, 0, 0)}))
        .boxed();
    if p.tracks.is_empty() {
        return mix(invalid.clone(), invalid);
    }
    let valid = (
        select(p.tracks.clone()),
        any::<(u8, u32)>(),
        option::weighted(0.2, any::<bool>()),
    )
        .prop_flat_map(|(t, form, all)| (Just((t.clone(), form, all)), 0..t.end + 10))
        .prop_map(|((t, (form, jitter), all), at)| {
            json!({"track": t.id, "at": time_json(t.fpf, at, form, jitter),
                   "ripple_all_tracks": all})
        })
        .boxed();
    mix(valid, invalid)
}

fn item_duplicate(p: &Pools) -> BoxedStrategy<Value> {
    let invalid = prop_oneof![
        Just(json!({"items": []})),
        fresh::<ItemId>().prop_map(|item| json!({"items": [item]})),
    ]
    .boxed();
    if p.items.is_empty() {
        return mix(invalid.clone(), invalid);
    }
    let valid = (
        vec(one_of(p.item_ids()), 1..=2),
        option::weighted(0.3, (-60..120_i64, any::<u8>())),
        option::weighted(0.4, select(MODES.to_vec())),
    )
        .prop_map(|(items, offset, mode)| {
            json!({"items": items,
                   "offset": offset.map(|(o, form)| offset_json(o, form)),
                   "mode": mode})
        })
        .boxed();
    mix(valid, invalid)
}

/// A relative time of `frames` 30 fps frames: flicks, or `"f:N"` when not negative.
fn offset_json(frames: i64, form: u8) -> Value {
    if frames >= 0 && form.is_multiple_of(2) {
        Value::from(format!("f:{frames}"))
    } else {
        Value::from(frames * FPF_30)
    }
}

/// `timeline.insert` with unrelated sequence, track and media (mostly invalid).
fn any_insert(p: &Pools) -> BoxedStrategy<Value> {
    (
        one_of(p.seqs.clone()),
        one_of(p.track_ids()),
        one_of(p.media_ids()),
        -30..300_i64,
        option::of(-10..100_i64),
        option::of(-5..400_i64),
        any::<(u8, u8, u8)>(),
    )
        .prop_map(|(sequence, track, media, at, src, dur, (fa, fs, fd))| {
            json!({
                "sequence": sequence, "track": track, "media": media,
                "at": time_json(FPF_30, at, fa, 0),
                "source_in": src.map(|s| time_json(FPF_30, s, fs, 0)),
                "duration": dur.map(|d| time_json(FPF_30, d, fd, 0)),
            })
        })
        .boxed()
}

fn timeline_get(p: &Pools) -> BoxedStrategy<Value> {
    let valid = (
        one_of(p.seqs.clone()),
        option::of(0..=2_u8),
        option::weighted(0.3, subsequence(GET_FIELDS.to_vec(), 0..=GET_FIELDS.len())),
    )
        .prop_map(|(sequence, depth, fields)| {
            json!({"sequence": sequence, "depth": depth, "fields": fields})
        })
        .boxed();
    let invalid = (
        one_of(p.seqs.clone()),
        prop_oneof![
            Just(json!({"depth": 3})),
            Just(json!({"fields": ["name", "bogus"]}))
        ],
    )
        .prop_map(|(sequence, mut v)| {
            v["sequence"] = sequence;
            v
        })
        .boxed();
    mix(valid, invalid)
}

// ----- items ---------------------------------------------------------------------------------

fn item_get(p: &Pools) -> BoxedStrategy<Value> {
    let valid = (
        one_of(p.item_ids()),
        option::weighted(
            0.3,
            subsequence(ITEM_GET_FIELDS.to_vec(), 0..=ITEM_GET_FIELDS.len()),
        ),
    )
        .prop_map(|(item, fields)| json!({"item": item, "fields": fields}))
        .boxed();
    let invalid = one_of(p.item_ids())
        .prop_map(|item| json!({"item": item, "fields": ["visual", "bogus"]}))
        .boxed();
    mix(valid, invalid)
}

/// `name` params naming an item that does not exist, with a time.
fn unknown_item(time_key: &'static str) -> BoxedStrategy<Value> {
    (fresh::<ItemId>(), -30..300_i64, any::<u8>())
        .prop_map(
            move |(item, f, form)| json!({"item": item, time_key: time_json(FPF_30, f, form, 0)}),
        )
        .boxed()
}

fn clip_split(p: &Pools) -> BoxedStrategy<Value> {
    if p.items.is_empty() {
        return mix(unknown_item("at"), unknown_item("at"));
    }
    let valid = select(p.items.clone())
        .prop_flat_map(|it| {
            let inside = if it.len > 1 {
                (it.start + 1..it.start + it.len).boxed()
            } else {
                Just(it.start).boxed()
            };
            let at = prop_oneof![
                8 => inside,
                1 => Just(it.start),
                1 => Just(it.start + it.len),
                1 => it.start - 5..it.start + it.len + 5,
            ];
            (
                Just(it),
                at,
                any::<u8>(),
                any::<u32>(),
                0..10_u8,
                option::weighted(0.1, any::<bool>()),
            )
        })
        .prop_map(|(it, at, form, jitter, shape, fine)| {
            let at = time_json(it.fpf, at, form, jitter);
            // Mostly `item`; sometimes `items` (the WP-2A.03 form), or both (`Invalid`).
            match shape {
                0 => json!({"items": [it.id], "at": at, "fine": fine}),
                1 if form % 4 == 0 => json!({"item": it.id, "items": [it.id], "at": at}),
                _ => json!({"item": it.id, "at": at, "fine": fine}),
            }
        })
        .boxed();
    mix(valid, unknown_item("at"))
}

/// One clip (`item`) or two (`items`) of the pools, or an unknown one.
fn speed_targets(p: &Pools) -> BoxedStrategy<Value> {
    let unknown = fresh::<ItemId>().prop_map(|id| json!({"item": id})).boxed();
    if p.items.is_empty() {
        return unknown;
    }
    let known = (
        select(p.items.clone()),
        option::weighted(0.2, select(p.items.clone())),
    )
        .prop_map(|(it, other)| match other {
            Some(o) => json!({"items": [it.id, o.id]}),
            None => json!({"item": it.id}),
        })
        .boxed();
    prop_oneof![8 => known, 1 => unknown].boxed()
}

/// `clip.set_speed` (WP-2D.01): one or two clips at a speed in any input form (sometimes out
/// of range or not a speed), sometimes keeping the duration, rippling or scaling keys.
fn clip_set_speed(p: &Pools) -> BoxedStrategy<Value> {
    let speeds = select(vec![
        json!(2),
        json!(0.5),
        json!("1/3"),
        json!("3/2"),
        json!({"num": 5, "den": 4}),
        json!(1),
        json!("2x"),
        json!(4),
        json!(0.25),
        json!("7/13"),
        json!(101),
        json!("fast"),
    ]);
    let valid = (
        speed_targets(p),
        speeds,
        option::weighted(0.3, select(vec!["in_point", "duration"])),
        option::weighted(0.3, any::<bool>()),
        option::weighted(0.3, select(vec!["keep", "scale"])),
    )
        .prop_map(|(mut v, speed, keep, ripple, keys)| {
            v["speed"] = speed;
            v["keep"] = json!(keep);
            v["ripple"] = json!(ripple);
            v["keyframes"] = json!(keys);
            v
        })
        .boxed();
    let invalid = fresh::<ItemId>()
        .prop_map(|id| json!({"item": id, "speed": 2}))
        .boxed();
    mix(valid, invalid)
}

/// `clip.reverse` (WP-2D.01): one or two clips, backwards, forwards or toggled.
fn clip_reverse(p: &Pools) -> BoxedStrategy<Value> {
    let valid = (speed_targets(p), option::weighted(0.5, any::<bool>()))
        .prop_map(|(mut v, reverse)| {
            v["reverse"] = json!(reverse);
            v
        })
        .boxed();
    mix(valid.clone(), speed_targets(p))
}

/// `clip.reset_speed` (WP-2D.01): one or two clips, sometimes rippling or scaling keys.
fn clip_reset_speed(p: &Pools) -> BoxedStrategy<Value> {
    let valid = (
        speed_targets(p),
        option::weighted(0.3, any::<bool>()),
        option::weighted(0.3, select(vec!["keep", "scale"])),
    )
        .prop_map(|(mut v, ripple, keys)| {
            v["ripple"] = json!(ripple);
            v["keyframes"] = json!(keys);
            v
        })
        .boxed();
    mix(valid.clone(), speed_targets(p))
}

/// `clip.freeze_frame` (WP-2D.01): a clip frozen mostly inside it (sometimes at its first frame,
/// its end or outside), for the default 3 s or a few frames.
fn clip_freeze_frame(p: &Pools) -> BoxedStrategy<Value> {
    if p.items.is_empty() {
        return mix(unknown_item("at"), unknown_item("at"));
    }
    let valid = select(p.items.clone())
        .prop_flat_map(|it| {
            let inside = if it.len > 1 {
                (it.start + 1..it.start + it.len).boxed()
            } else {
                Just(it.start).boxed()
            };
            let at = prop_oneof![
                8 => inside,
                2 => Just(it.start),
                1 => Just(it.start + it.len),
                1 => it.start - 5..it.start + it.len + 5,
            ];
            (
                Just(it),
                at,
                any::<(u8, u32)>(),
                option::weighted(0.4, 0..120_i64),
            )
        })
        .prop_map(|(it, at, (form, jitter), frames)| {
            json!({"item": it.id, "at": time_json(it.fpf, at, form, jitter),
                   "duration": frames.map(|f| time_json(it.fpf, f, form, 0))})
        })
        .boxed();
    mix(valid, unknown_item("at"))
}

/// `clip.trim` (`ripple: false`) or `clip.ripple_trim` (WP-2A.04): an edge of a clip to a time
/// or by a delta of up to 45 frames either way, sometimes clamped, sometimes on the sample grid
/// (`fine`, valid for audio clips only), the ripple sometimes on every track.
fn edge_trim(p: &Pools, ripple: bool) -> BoxedStrategy<Value> {
    let invalid = unknown_item("to")
        .prop_map(|mut v| {
            v["edge"] = json!("end");
            v
        })
        .boxed();
    if p.items.is_empty() {
        return mix(invalid.clone(), invalid);
    }
    let valid = (
        select(p.items.clone()),
        any::<bool>(),
        -45..=45_i64,
        0..10_u8,
        any::<(u8, u32)>(),
        option::weighted(0.3, any::<bool>()),
        (
            option::weighted(0.4, any::<bool>()),
            option::weighted(0.1, any::<bool>()),
            option::weighted(0.15, any::<bool>()),
        ),
    )
        .prop_map(
            move |(it, end, k, mode, (form, jitter), unlink, (clamp, fine, all))| {
                let edge = if end { it.start + it.len } else { it.start };
                let mut v = json!({"item": it.id, "edge": if end { "end" } else { "start" },
                                   "unlink": unlink, "clamp": clamp, "fine": fine});
                if ripple {
                    v["ripple_all_tracks"] = json!(all);
                }
                let to = time_json(it.fpf, edge + k, form, jitter);
                let delta = time_json(it.fpf, k, form, jitter);
                match mode {
                    0..=5 => v["to"] = to,
                    6..=8 => v["delta"] = delta,
                    // Both, or neither below: `Invalid`.
                    9 if k % 2 == 0 => {
                        v["to"] = to;
                        v["delta"] = delta;
                    }
                    _ => {}
                }
                v
            },
        )
        .boxed();
    mix(valid, invalid)
}

/// `clip.roll` (WP-2A.04): mostly two clips that touch on one track (the cut moved to a time or
/// by a delta), sometimes two that do not (`Invalid` at `/right`).
fn clip_roll(p: &Pools) -> BoxedStrategy<Value> {
    let invalid = (fresh::<ItemId>(), one_of(p.item_ids()), -30..30_i64)
        .prop_map(|(left, right, d)| {
            json!({"left": left, "right": right, "delta": time_json(FPF_30, d, 5, 0)})
        })
        .boxed();
    let pairs: Vec<(Itm, Itm)> = p
        .items
        .iter()
        .flat_map(|a| {
            p.items
                .iter()
                .filter(move |b| b.track == a.track && b.start == a.start + a.len)
                .map(move |b| (a.clone(), b.clone()))
        })
        .collect();
    if pairs.is_empty() {
        return mix(invalid.clone(), invalid);
    }
    let valid = (
        select(pairs),
        -45..=45_i64,
        any::<bool>(),
        any::<(u8, u32)>(),
        option::weighted(0.4, any::<bool>()),
    )
        .prop_map(|((l, r), k, to, (form, jitter), clamp)| {
            let mut v = json!({"left": l.id, "right": r.id, "clamp": clamp});
            if to {
                v["to"] = time_json(l.fpf, r.start + k, form, jitter);
            } else {
                v["delta"] = time_json(l.fpf, k, form, jitter);
            }
            v
        })
        .boxed();
    mix(valid, invalid)
}

/// `clip.slip` (WP-2A.04) and `clip.slide` (`slide`): a clip and a delta of up to 60 frames
/// either way, often clamped (a clip that plays all of its media has nothing to slip, and one
/// between two others little room to slide); a slip sometimes on the sample grid (`fine`).
fn clip_shift(p: &Pools, slide: bool) -> BoxedStrategy<Value> {
    let invalid = unknown_item("delta");
    if p.items.is_empty() {
        return mix(invalid.clone(), invalid);
    }
    let clamp = prop_oneof![3 => Just(Some(true)), 1 => Just(Some(false)), 2 => Just(None)];
    let valid = (
        select(p.items.clone()),
        -60..=60_i64,
        any::<(u8, u32)>(),
        clamp,
        option::weighted(0.1, any::<bool>()),
    )
        .prop_map(move |(it, k, (form, jitter), clamp, fine)| {
            let mut v = json!({"item": it.id, "delta": time_json(it.fpf, k, form, jitter),
                               "clamp": clamp});
            if !slide {
                v["fine"] = json!(fine);
            }
            v
        })
        .boxed();
    mix(valid, invalid)
}

fn item_move(p: &Pools) -> BoxedStrategy<Value> {
    let invalid = unknown_item("start");
    if p.items.is_empty() {
        return mix(invalid.clone(), invalid);
    }
    let tracks: Arc<[Trk]> = p.tracks.clone().into();
    let items: Arc<[Itm]> = p.items.clone().into();
    let valid = select(p.items.clone())
        .prop_flat_map(move |it| {
            let others: Vec<ItemId> = items
                .iter()
                .filter(|o| o.seq == it.seq)
                .map(|o| o.id)
                .collect();
            let lanes: Vec<Value> = tracks
                .iter()
                .filter(|t| t.seq == it.seq && t.kind == it.kind)
                .map(|t| json!(t.id))
                .collect();
            let all: Vec<Value> = tracks.iter().map(|t| json!(t.id)).collect();
            let start = prop_oneof![
                5 => it.start - 60..it.start + 61,
                3 => 0..it.seq_end + 60,
                1 => -30..0_i64,
            ];
            let to_track = option::weighted(
                0.4,
                prop_oneof![8 => select(lanes), 1 => select(all), 1 => fresh::<TrackId>()],
            );
            let overlap = option::weighted(0.4, prop_oneof![Just("reject"), Just("new_track")]);
            (
                Just(it),
                start,
                any::<(u8, u32)>(),
                to_track,
                option::weighted(0.3, any::<bool>()),
                overlap,
                option::weighted(0.3, select(MODES.to_vec())),
                0..12_u8,
                option::weighted(0.1, any::<bool>()),
                option::weighted(0.1, any::<bool>()),
                select(others.clone()),
            )
        })
        .prop_map(
            |(
                it,
                start,
                (form, jitter),
                to_track,
                unlink,
                overlap,
                mode,
                shape,
                all,
                fine,
                other,
            )| {
                let mut v = json!({"to_track": to_track, "unlink": unlink, "overlap": overlap,
                                   "mode": mode, "ripple_all_tracks": all, "fine": fine});
                // The WP-2A.03 forms: `items` (with another item) and `delta`.
                match shape {
                    0 | 1 => v["items"] = json!([it.id, other]),
                    2 => v["items"] = json!([it.id]),
                    _ => v["item"] = json!(it.id),
                }
                match shape {
                    1 | 3..=5 => {
                        let frames = start - it.start;
                        v["delta"] = if frames >= 0 && form.is_multiple_of(2) {
                            json!(format!("f:{frames}"))
                        } else {
                            json!(frames * it.fpf)
                        };
                    }
                    _ => v["start"] = time_json(it.fpf, start, form, jitter),
                }
                v
            },
        )
        .boxed();
    mix(valid, invalid)
}

/// Pairs of items of one sequence that start and end together (what `item.link` accepts), when
/// `aligned`; else pairs where the second starts inside the first (what `item.attach` accepts).
fn item_pairs(p: &Pools, aligned: bool) -> Vec<(ItemId, ItemId)> {
    let mut out = Vec::new();
    for a in &p.items {
        for b in &p.items {
            let ok = a.seq == b.seq
                && a.id != b.id
                && a.track != b.track
                && if aligned {
                    a.id < b.id && a.start == b.start && a.len == b.len
                } else {
                    a.start <= b.start && b.start < a.start + a.len
                };
            if ok {
                out.push((a.id, b.id));
            }
        }
    }
    out
}

fn item_link(p: &Pools) -> BoxedStrategy<Value> {
    let invalid = prop_oneof![
        Just(json!({"items": []})),
        one_of(p.item_ids()).prop_map(|item| json!({"items": [item]})),
        (one_of(p.item_ids()), one_of(p.item_ids())).prop_map(|(a, b)| json!({"items": [a, b]})),
    ]
    .boxed();
    let pairs = item_pairs(p, true);
    if pairs.is_empty() {
        return mix(invalid.clone(), invalid);
    }
    let valid = select(pairs)
        .prop_map(|(a, b)| json!({"items": [a, b]}))
        .boxed();
    mix(valid, invalid)
}

fn item_unlink(p: &Pools) -> BoxedStrategy<Value> {
    let valid = vec(one_of(p.item_ids()), 1..=2)
        .prop_map(|items| json!({"items": items}))
        .boxed();
    let invalid = prop_oneof![
        Just(json!({"items": []})),
        fresh::<ItemId>().prop_map(|item| json!({"items": [item]})),
    ]
    .boxed();
    mix(valid, invalid)
}

fn item_attach(p: &Pools) -> BoxedStrategy<Value> {
    let invalid = prop_oneof![
        Just(json!({"items": [], "parent": null})),
        (one_of(p.item_ids()), one_of(p.item_ids()))
            .prop_map(|(a, b)| json!({"items": [a], "parent": b})),
    ]
    .boxed();
    let pairs = item_pairs(p, false);
    if pairs.is_empty() {
        return mix(invalid.clone(), invalid);
    }
    let valid = select(pairs)
        .prop_map(|(parent, child)| json!({"items": [child], "parent": parent}))
        .boxed();
    mix(valid, invalid)
}

fn item_detach(p: &Pools) -> BoxedStrategy<Value> {
    item_unlink(p)
}

/// `timeline.paste`: a one-clip payload of an existing media onto a track of the right kind,
/// or a payload that is not one (another version, no items, a media that does not exist).
fn timeline_paste(p: &Pools) -> BoxedStrategy<Value> {
    let clip = |media: MediaId, id: u128, len: i64| {
        let item = cutweave_model::Clip::new(
            ItemId::from_u128(id),
            media,
            cutweave_model::Time::ZERO,
            cutweave_model::Duration::from_flicks(len * FPF_30)
                .unwrap_or(cutweave_model::Duration::ZERO),
        );
        serde_json::to_value(cutweave_model::Item::Clip(item)).unwrap_or(Value::Null)
    };
    let payload = move |t: &Trk, media: MediaId, id: u128, len: i64| {
        json!({"version": 1, "sequence": t.seq, "frame_rate": {"num": 30, "den": 1},
               "span": len * FPF_30,
               "tracks": [{"kind": t.kind, "row": 0, "source": t.id}],
               "items": [{"track": 0, "item": clip(media, id, len)}]})
    };
    let invalid = prop_oneof![
        any::<u32>().prop_map(|n| json!({"payload": {"version": n.saturating_add(2), "sequence": SequenceId::from_u128(1),
            "frame_rate": {"num": 30, "den": 1}, "span": 0, "tracks": [], "items": []}, "at": 0})),
        Just(json!({"payload": {}, "at": 0})),
    ]
    .boxed();
    let tracks: Vec<Trk> = p
        .tracks
        .iter()
        .filter(|t| matches!(t.kind, TrackKind::Video | TrackKind::Audio))
        .cloned()
        .collect();
    if tracks.is_empty() || p.media.is_empty() {
        return mix(invalid.clone(), invalid);
    }
    let valid = (
        select(tracks),
        select(p.media.clone()),
        any::<u128>(),
        1..40_i64,
        0..60_i64,
        any::<bool>(),
        option::weighted(0.4, select(MODES.to_vec())),
    )
        .prop_map(move |(t, m, id, len, at, back, mode)| {
            json!({"payload": payload(&t, m.id, id, len), "at": format!("f:{at}"),
                   "track": back.then_some(t.id), "mode": mode})
        })
        .boxed();
    mix(valid, invalid)
}

/// `clip.paste_attributes`: a source, some targets and some groups.
fn clip_paste_attributes(p: &Pools) -> BoxedStrategy<Value> {
    let groups = ["transform", "crop", "flip", "compositing", "audio"];
    let invalid = prop_oneof![
        Just(json!({"from": ItemId::from_u128(1), "to": [], "which": []})),
        (fresh::<ItemId>(), fresh::<ItemId>())
            .prop_map(|(a, b)| json!({"from": a, "to": [b], "which": ["audio"]})),
    ]
    .boxed();
    if p.items.is_empty() {
        return mix(invalid.clone(), invalid);
    }
    let valid = (
        one_of(p.item_ids()),
        vec(one_of(p.item_ids()), 1..=2),
        subsequence(groups.to_vec(), 0..=5),
    )
        .prop_map(|(from, to, which)| json!({"from": from, "to": to, "which": which}))
        .boxed();
    mix(valid, invalid)
}

/// `clip.replace` (WP-2A.11): a clip and a media of the pools in any mode, sometimes with a
/// source position (also negative or past the media), sometimes an unknown clip or media.
fn clip_replace(p: &Pools) -> BoxedStrategy<Value> {
    let invalid = prop_oneof![
        (fresh::<ItemId>(), fresh::<MediaId>()).prop_map(|(i, m)| json!({"item": i, "media": m})),
        Just(
            json!({"item": ItemId::from_u128(1), "media": MediaId::from_u128(1),
                    "mode": "fit_to_source", "source_in": "-1s"})
        ),
    ]
    .boxed();
    if p.items.is_empty() || p.media.is_empty() {
        return mix(invalid.clone(), invalid);
    }
    // Mostly a media of the clip's own family (pictures for picture tracks, sound for audio).
    let (items, media) = (p.items.clone(), p.media.clone());
    let valid = select(items)
        .prop_flat_map(move |it| {
            let fits: Vec<MediaId> = media
                .iter()
                .filter(|m| Some(m.id) != it.media)
                .filter(|m| match it.kind {
                    TrackKind::Audio => matches!(m.kind, MediaKind::Audio | MediaKind::Video),
                    _ => matches!(m.kind, MediaKind::Video | MediaKind::Image),
                })
                .map(|m| m.id)
                .collect();
            let all: Vec<MediaId> = media.iter().map(|m| m.id).collect();
            let pick = if fits.is_empty() {
                one_of(all)
            } else {
                prop_oneof![9 => one_of(fits), 1 => one_of(all)].boxed()
            };
            (Just(it.id), pick)
        })
        .prop_flat_map(|(item, media)| {
            (
                Just(item),
                Just(media),
                option::weighted(
                    0.6,
                    select(vec!["keep_duration", "keep_source_in", "fit_to_source"]),
                ),
                option::weighted(0.3, select(vec!["0s", "1s", "3s", "10s", "-1s", "500s"])),
            )
        })
        .prop_map(|(item, media, mode, source_in)| {
            let mut v = json!({"item": item, "media": media});
            if let Some(m) = mode {
                v["mode"] = json!(m);
            }
            if let Some(t) = source_in {
                v["source_in"] = json!(t);
            }
            v
        })
        .boxed();
    mix(valid, invalid)
}

fn item_group_select(p: &Pools) -> BoxedStrategy<Value> {
    item_unlink(p)
}

fn item_delete(p: &Pools) -> BoxedStrategy<Value> {
    let valid = vec(one_of(p.item_ids()), 1..=3)
        .prop_map(|items| json!({"items": items}))
        .boxed();
    let invalid = prop_oneof![
        Just(json!({"items": []})),
        fresh::<ItemId>().prop_map(|item| json!({"items": [item]})),
    ]
    .boxed();
    mix(valid, invalid)
}

fn vec2(lo: f64, hi: f64) -> BoxedStrategy<Vec2> {
    (lo..=hi, lo..=hi)
        .prop_map(|(x, y)| Vec2::new(x, y))
        .boxed()
}

fn item_set_props(p: &Pools) -> BoxedStrategy<Value> {
    let transform = (
        option::of(vec2(-2.0, 2.0)),
        option::of(prop_oneof![Just(Vec2::ONE), vec2(0.0, 4.0)]),
        option::of(-720.0..=720.0_f64),
        option::of(vec2(-1.0, 1.0)),
    )
        .prop_map(|(position, scale, rotation, anchor)| {
            json!({"position": position, "scale": scale, "rotation": rotation, "anchor": anchor})
        });
    let crop = (
        option::of(0.0..=1.0_f64),
        option::of(0.0..=1.0_f64),
        option::of(0.0..=1.0_f64),
        option::of(0.0..=1.0_f64),
    )
        .prop_map(|(left, top, right, bottom)| {
            json!({"left": left, "top": top, "right": right, "bottom": bottom})
        });
    let flip = (option::of(any::<bool>()), option::of(any::<bool>()))
        .prop_map(|(horizontal, vertical)| json!({"horizontal": horizontal, "vertical": vertical}));
    let props = (
        (
            option::weighted(0.3, prop_oneof![4 => name(), 1 => Just(String::new())]),
            option::weighted(0.3, any::<bool>()),
            option::weighted(0.4, prop_oneof![Just(0.0), Just(1.0), 0.0..=1.0_f64]),
            option::weighted(0.3, gain()),
            option::weighted(0.4, transform),
        ),
        (
            option::weighted(0.2, -1.0..=1.0_f64),
            option::weighted(0.2, crop),
            option::weighted(0.2, flip),
            option::weighted(
                0.2,
                select(vec![
                    "normal",
                    "multiply",
                    "screen",
                    "overlay",
                    "soft_light",
                    "add",
                ]),
            ),
        ),
    )
        .prop_map(
            |((name, enabled, opacity, audio_gain, transform), (pan, crop, flip, blend))| {
                json!({"name": name, "enabled": enabled, "opacity": opacity,
                       "audio_gain": audio_gain, "transform": transform, "audio_pan": pan,
                       "crop": crop, "flip": flip, "blend_mode": blend})
            },
        );
    // `at` lets an animated property take a key; it is ignored for static ones.
    let valid = (
        one_of(p.item_ids()),
        props,
        option::weighted(0.6, near_any(p)),
    )
        .prop_map(|(item, props, at)| json!({"item": item, "props": props, "at": at}))
        .boxed();
    let bad = prop_oneof![
        Just(json!({})),
        Just(json!({"opacity": 1.5})),
        Just(json!({"audio_gain": 60.0})),
        Just(json!({"transform": {"scale": {"x": -1.0, "y": 1.0}}})),
        bad_name().prop_map(|name| json!({"name": name})),
    ];
    let invalid = (one_of(p.item_ids()), bad)
        .prop_map(|(item, props)| json!({"item": item, "props": props}))
        .boxed();
    mix(valid, invalid)
}

/// A sequence time near some item (or just a small time when there are none).
fn near_any(p: &Pools) -> BoxedStrategy<Value> {
    match p.items.first() {
        None => (0..60_i64).prop_map(|f| time_json(FPF_30, f, 0, 0)).boxed(),
        Some(_) => select(p.items.clone())
            .prop_flat_map(|it| near_item(&it))
            .boxed(),
    }
}

// ----- keyframe.*, item.reset_props (WP-2C.03) ----------------------------------------------------

const PROP_IDS: [&str; 13] = [
    "transform.position",
    "transform.scale",
    "transform.rotation",
    "transform.anchor",
    "opacity",
    "crop.left",
    "crop.top",
    "crop.right",
    "crop.bottom",
    "flip.horizontal",
    "flip.vertical",
    "audio_gain",
    "audio_pan",
];

const EASINGS: [&str; 8] = [
    "linear",
    "hold",
    "ease_in",
    "ease_out",
    "ease_in_out",
    "back_out",
    "spring",
    "bounce",
];

/// A value of the property `prop`, mostly inside its range.
fn prop_value(prop: &'static str) -> BoxedStrategy<Value> {
    match prop {
        "transform.position" | "transform.anchor" => (-2.0..=2.0_f64, -2.0..=2.0_f64)
            .prop_map(|(x, y)| json!({"x": x, "y": y}))
            .boxed(),
        "transform.scale" => prop_oneof![
            9 => (0.0..=4.0_f64, 0.0..=4.0_f64).prop_map(|(x, y)| json!({"x": x, "y": y})),
            1 => Just(json!({"x": -1.0, "y": 1.0})),
        ]
        .boxed(),
        "transform.rotation" => (-720.0..=720.0_f64).prop_map(|v| json!(v)).boxed(),
        "opacity" | "crop.left" | "crop.top" | "crop.right" | "crop.bottom" => prop_oneof![
            9 => (0.0..=1.0_f64).prop_map(|v| json!(v)),
            1 => Just(json!(1.5)),
        ]
        .boxed(),
        "flip.horizontal" | "flip.vertical" => any::<bool>().prop_map(|b| json!(b)).boxed(),
        "audio_gain" => {
            prop_oneof![9 => gain().prop_map(|v| json!(v)), 1 => Just(json!(99.0))].boxed()
        }
        _ => prop_oneof![
            9 => (-1.0..=1.0_f64).prop_map(|v| json!(v)),
            1 => Just(json!(-2.0)),
        ]
        .boxed(),
    }
}

/// A sequence time near the item: a frame from a little before its start to a little past its
/// end, in a random input form.
fn near_item(it: &Itm) -> BoxedStrategy<Value> {
    let fpf = it.fpf;
    (
        it.start - 3..it.start + it.len + 3,
        any::<u8>(),
        any::<u32>(),
    )
        .prop_map(move |(f, form, jitter)| time_json(fpf, f, form, jitter))
        .boxed()
}

/// `keyframe.set`: one to three keys on one property of an item, mostly valid.
fn keyframe_set(p: &Pools) -> BoxedStrategy<Value> {
    if p.items.is_empty() {
        return mix(unknown_item("at"), unknown_item("at"));
    }
    let valid = (select(p.items.clone()), select(PROP_IDS.to_vec()))
        .prop_flat_map(|(it, prop)| {
            let key = (
                near_item(&it),
                option::weighted(0.8, prop_value(prop)),
                option::weighted(0.5, select(EASINGS.to_vec())),
                any::<bool>(),
            )
                .prop_map(|(at, value, easing, use_t)| {
                    let mut k = serde_json::Map::new();
                    k.insert(if use_t { "t" } else { "at" }.into(), at);
                    if let Some(v) = value {
                        k.insert("value".into(), v);
                    }
                    if let Some(e) = easing {
                        k.insert("easing".into(), json!(e));
                    }
                    Value::Object(k)
                });
            (
                Just(it),
                Just(prop),
                vec(key, 1..4),
                option::weighted(0.2, any::<bool>()),
            )
        })
        .prop_map(|(it, prop, keys, replace)| {
            json!({"item": it.id, "prop": prop, "keys": keys, "replace": replace})
        })
        .boxed();
    let invalid = prop_oneof![
        Just(json!({"prop": "opacity", "keys": []})),
        Just(json!({"item": ItemId::from_u128(99), "prop": "opacity", "keys": [{"at": "1s"}]})),
    ]
    .boxed();
    mix(valid, invalid)
}

/// A key of `it` (the key list is empty for an item without keys: then a random time).
fn key_refs(it: &Itm) -> BoxedStrategy<Value> {
    let fresh_key = (select(PROP_IDS.to_vec()), -5..60_i64)
        .prop_map(|(prop, f)| json!({"prop": prop, "t": f * 1_000_000}))
        .boxed();
    if it.keys.is_empty() {
        return vec(fresh_key, 1..3).prop_map(Value::Array).boxed();
    }
    let known: Vec<Value> = it
        .keys
        .iter()
        .map(|(prop, t)| json!({"prop": prop, "t": t}))
        .collect();
    prop_oneof![
        9 => subsequence(known, 1..=it.keys.len().min(4)).prop_map(Value::Array),
        1 => vec(fresh_key, 1..3).prop_map(Value::Array),
    ]
    .boxed()
}

/// An item with the strategy of a command that addresses its keys.
fn with_keys(
    p: &Pools,
    f: impl Fn(Itm, Value) -> BoxedStrategy<Value> + Send + Sync + 'static,
) -> BoxedStrategy<Value> {
    if p.items.is_empty() {
        return mix(unknown_item("at"), unknown_item("at"));
    }
    let f = Arc::new(f);
    let valid = select(p.items.clone())
        .prop_flat_map(move |it| {
            let keys = key_refs(&it);
            let f = Arc::clone(&f);
            (Just(it), keys).prop_flat_map(move |(it, keys)| f(it, keys))
        })
        .boxed();
    mix(valid, unknown_item("at"))
}

fn keyframe_delete(p: &Pools) -> BoxedStrategy<Value> {
    with_keys(p, |it, keys| {
        Just(json!({"item": it.id, "keys": keys})).boxed()
    })
}

fn keyframe_copy(p: &Pools) -> BoxedStrategy<Value> {
    keyframe_delete(p)
}

fn keyframe_move(p: &Pools) -> BoxedStrategy<Value> {
    with_keys(p, |it, keys| {
        let fpf = it.fpf;
        (
            -20..20_i64,
            any::<u8>(),
            any::<u32>(),
            any::<bool>(),
            near_item(&it),
        )
            .prop_map(move |(f, form, jitter, by, to)| {
                if by {
                    json!({"item": it.id, "keys": keys, "delta": time_json(fpf, f, form, jitter)})
                } else {
                    json!({"item": it.id, "keys": keys, "to_at": to})
                }
            })
            .boxed()
    })
}

fn keyframe_set_interp(p: &Pools) -> BoxedStrategy<Value> {
    with_keys(p, |it, keys| {
        select(EASINGS.to_vec())
            .prop_map(move |e| json!({"item": it.id, "keys": keys, "easing": e}))
            .boxed()
    })
}

fn keyframe_clear(p: &Pools) -> BoxedStrategy<Value> {
    if p.items.is_empty() {
        return mix(unknown_item("at"), unknown_item("at"));
    }
    let valid = (select(p.items.clone()), select(PROP_IDS.to_vec()))
        .prop_flat_map(|(it, prop)| {
            (
                Just(it.clone()),
                Just(prop),
                option::weighted(0.5, near_item(&it)),
            )
        })
        .prop_map(|(it, prop, at)| json!({"item": it.id, "prop": prop, "at": at}))
        .boxed();
    mix(valid, unknown_item("at"))
}

fn keyframe_paste(p: &Pools) -> BoxedStrategy<Value> {
    if p.items.is_empty() {
        return mix(unknown_item("at"), unknown_item("at"));
    }
    let valid = (select(p.items.clone()), select(PROP_IDS.to_vec()))
        .prop_flat_map(|(it, prop)| {
            (
                Just(it.clone()),
                Just(prop),
                vec((0..30_i64, prop_value(prop)), 1..4),
                near_item(&it),
            )
        })
        .prop_map(|(it, prop, keys, at)| {
            let keys: Vec<Value> = keys
                .into_iter()
                .map(|(f, v)| {
                    json!({"prop": prop, "dt": f * it.fpf, "value": v,
                           "interp": {"kind": "linear"}})
                })
                .collect();
            json!({"item": it.id, "payload": {"version": 1, "keys": keys}, "at": at})
        })
        .boxed();
    mix(valid, unknown_item("at"))
}

fn keyframe_eval(p: &Pools) -> BoxedStrategy<Value> {
    if p.items.is_empty() {
        return mix(unknown_item("at"), unknown_item("at"));
    }
    let valid = (select(p.items.clone()), select(PROP_IDS.to_vec()))
        .prop_flat_map(|(it, prop)| {
            (
                Just(it.clone()),
                Just(prop),
                near_item(&it),
                near_item(&it),
                2..40_u32,
                any::<bool>(),
            )
        })
        .prop_map(|(it, prop, a, b, n, range)| {
            if range {
                json!({"item": it.id, "prop": prop, "from": a, "to": b, "samples": n})
            } else {
                json!({"item": it.id, "prop": prop, "at": a})
            }
        })
        .boxed();
    mix(valid, unknown_item("at"))
}

fn item_apply_layout(p: &Pools) -> BoxedStrategy<Value> {
    // Static video clips of one sequence (an animated property needs `at`): a layout takes two
    // to four of them.
    let by_seq: Vec<Vec<ItemId>> = p
        .seqs
        .iter()
        .map(|s| {
            p.items
                .iter()
                .filter(|i| i.seq == *s && i.kind == TrackKind::Video && i.keys.is_empty())
                .map(|i| i.id)
                .collect::<Vec<_>>()
        })
        .filter(|v| v.len() >= 2)
        .collect();
    if by_seq.is_empty() {
        return mix(unknown_item("at"), unknown_item("at"));
    }
    let valid = select(by_seq)
        .prop_flat_map(|items| {
            let n = items.len().min(4);
            (2..=n).prop_flat_map(move |count| {
                let layouts: Vec<&'static str> = match count {
                    2 => vec!["side_by_side", "top_bottom", "pip_top_right"],
                    3 => vec!["columns_3", "one_large_two_small"],
                    _ => vec!["grid_2x2"],
                };
                (
                    subsequence(items.clone(), count),
                    select(layouts),
                    select(vec!["crop", "fit"]),
                    option::of(0.0..=40.0_f64),
                )
            })
        })
        .prop_map(|(items, layout, fill, gap)| {
            json!({"items": items, "layout": layout, "fill": fill, "gap": gap})
        })
        .boxed();
    mix(valid, unknown_item("at"))
}

fn item_layouts() -> BoxedStrategy<Value> {
    prop_oneof![
        Just(json!({})),
        (0..6_u32).prop_map(|count| json!({"count": count}))
    ]
    .boxed()
}

fn item_reset_props(p: &Pools) -> BoxedStrategy<Value> {
    let shape = prop_oneof![
        3 => Just(json!({})),
        3 => select(vec!["transform", "crop", "flip", "compositing", "audio"])
            .prop_map(|g| json!({"group": g})),
        2 => subsequence(PROP_IDS.to_vec(), 1..4).prop_map(|props| json!({"props": props})),
        1 => Just(json!({"group": "crop", "props": ["opacity"]})),
    ];
    let valid = (one_of(p.item_ids()), shape)
        .prop_map(|(item, mut shape)| {
            shape["item"] = item;
            shape
        })
        .boxed();
    mix(valid, unknown_item("at"))
}

/// `item.set_label_color` (issue #222): one to three items get a colour or lose it;
/// sometimes no item, an unknown one or a colour that is not one.
fn item_set_label_color(p: &Pools) -> BoxedStrategy<Value> {
    let ids = p.item_ids();
    let bad = prop_oneof![
        Just(json!({"items": []})),
        fresh::<ItemId>().prop_map(|i| json!({"items": [i], "color": "#336699"})),
    ]
    .boxed();
    if ids.is_empty() {
        return mix(bad.clone(), bad);
    }
    let most = ids.len().min(3);
    let valid = (
        subsequence(ids.clone(), 1..=most),
        option::weighted(0.7, select(vec!["#336699", "#d9534f", "#5cb85c"])),
    )
        .prop_map(|(items, color)| json!({"items": items, "color": color}))
        .boxed();
    let wrong = select(ids)
        .prop_map(|i| json!({"items": [i], "color": "red"}))
        .boxed();
    mix(valid, prop_oneof![bad, wrong].boxed())
}

// ----- audio.* (WP-2E.01) -------------------------------------------------------------------

fn fade_secs() -> BoxedStrategy<Value> {
    prop_oneof![
        Just(json!(0)),
        (1..=4_000_i32).prop_map(|ms| json!(format!("{}s", f64::from(ms) / 1000.0))),
    ]
    .boxed()
}

fn fade_change() -> BoxedStrategy<Value> {
    (
        option::weighted(0.7, fade_secs()),
        option::weighted(0.7, fade_secs()),
        option::weighted(0.5, select(vec!["linear", "log", "equal_power", "s_curve"])),
    )
        .prop_map(|(i, o, curve)| {
            let mut v = json!({"curve": curve});
            if let Some(i) = i {
                v["in"] = i;
            }
            if let Some(o) = o {
                v["out"] = o;
            }
            v
        })
        .boxed()
}

fn audio_set_fade(p: &Pools) -> BoxedStrategy<Value> {
    let valid = (one_of(p.item_ids()), fade_change())
        .prop_map(|(item, mut change)| {
            change["item"] = item;
            change
        })
        .boxed();
    let invalid = (one_of(p.item_ids()), Just(json!({})))
        .prop_map(|(item, mut change)| {
            change["item"] = item;
            change
        })
        .boxed();
    mix(valid, invalid)
}

fn audio_set_volume(p: &Pools) -> BoxedStrategy<Value> {
    let valid = (
        one_of(p.item_ids()),
        prop_oneof![Just(-144.0), Just(12.0), -60.0..=12.0_f64],
        option::weighted(0.6, near_any(p)),
    )
        .prop_map(|(item, db, at)| json!({"item": item, "db": db, "at": at}))
        .boxed();
    let invalid = (one_of(p.item_ids()), Just(40.0))
        .prop_map(|(item, db)| json!({"item": item, "db": db}))
        .boxed();
    mix(valid, invalid)
}

fn audio_set_pan(p: &Pools) -> BoxedStrategy<Value> {
    let valid = (
        one_of(p.item_ids()),
        -1.0..=1.0_f64,
        option::weighted(0.6, near_any(p)),
    )
        .prop_map(|(item, pan, at)| json!({"item": item, "pan": pan, "at": at}))
        .boxed();
    let invalid = (one_of(p.item_ids()), Just(1.5))
        .prop_map(|(item, pan)| json!({"item": item, "pan": pan}))
        .boxed();
    mix(valid, invalid)
}

fn audio_set_gain(p: &Pools) -> BoxedStrategy<Value> {
    let valid = (one_of(p.item_ids()), -144.0..=12.0_f64)
        .prop_map(|(item, db)| json!({"item": item, "db": db}))
        .boxed();
    mix(valid, unknown_item("db"))
}

fn audio_set(p: &Pools) -> BoxedStrategy<Value> {
    let change = prop_oneof![
        fade_change().prop_map(|mut f| {
            f["kind"] = json!("fade");
            f
        }),
        (-60.0..=12.0_f64).prop_map(|db| json!({"kind": "volume", "db": db})),
        (-1.0..=1.0_f64).prop_map(|pan| json!({"kind": "pan", "pan": pan})),
    ];
    let valid = (
        one_of(p.item_ids()),
        change,
        option::weighted(0.6, near_any(p)),
    )
        .prop_map(|(item, set, at)| json!({"item": item, "set": set, "at": at}))
        .boxed();
    mix(valid, unknown_item("at"))
}

// ----- edit.* --------------------------------------------------------------------------------

fn begin_tx() -> BoxedStrategy<Value> {
    let valid = (
        option::of(select(vec!["batch", "gesture", "proposal"])),
        name(),
    )
        .prop_map(|(kind, label)| json!({"kind": kind, "label": label}))
        .boxed();
    let invalid = prop_oneof![
        Just(json!({"label": ""})),
        Just(json!({"label": "x".repeat(201)})),
        Just(json!({"label": "a\u{1}b"})),
        Just(json!({"kind": "merge", "label": "x"})),
    ]
    .boxed();
    mix(valid, invalid)
}

fn tx_ref() -> BoxedStrategy<Value> {
    prop_oneof![
        8 => Just(json!({})),
        1 => (1..80_u64).prop_map(|n| json!({"tx": format!("tx_{n}")})),
        1 => Just(json!({"tx": "tx_x"})),
    ]
    .boxed()
}

fn history_scope() -> BoxedStrategy<Value> {
    prop_oneof![
        6 => Just(json!({})),
        2 => Just(json!({"actor": "*"})),
        1 => Just(json!({"actor": "user"})),
        1 => Just(json!({"actor": format!("agent:{AGENT_LABEL}")})),
        1 => Just(json!({"actor": "robot"})),
    ]
    .boxed()
}

fn history_page() -> BoxedStrategy<Value> {
    let valid = (
        option::of(0..=20_u32),
        option::weighted(
            0.2,
            prop_oneof![
                (0..30_u64).prop_map(|n| format!("h_{n}")),
                Just("zzz".to_owned())
            ],
        ),
        option::weighted(
            0.2,
            select(vec!["user".to_owned(), format!("agent:{AGENT_LABEL}")]),
        ),
    )
        .prop_map(
            |(limit, cursor, actor)| json!({"limit": limit, "cursor": cursor, "actor": actor}),
        )
        .boxed();
    mix(valid, Just(json!({"limit": 600})).boxed())
}

/// `edit.set_history_limit`: limits deeper than any sequence the harness runs, so the undo
/// properties see every step (the trimming itself is unit-tested in `cutweave-commands`).
fn history_limit() -> BoxedStrategy<Value> {
    let valid = (500..=10_000_u32)
        .prop_map(|steps| json!({"steps": steps}))
        .boxed();
    let invalid = prop_oneof![
        Just(json!({"steps": 0})),
        Just(json!({"steps": 10_001})),
        Just(json!({})),
    ]
    .boxed();
    mix(valid, invalid)
}

fn checkpoint() -> BoxedStrategy<Value> {
    let valid = (
        option::of(select(CHECKPOINT_NAMES.to_vec())),
        option::weighted(0.2, Just("a note")),
    )
        .prop_map(|(name, note)| json!({"name": name, "note": note}))
        .boxed();
    let invalid = select(vec![
        "cp_2".to_owned(),
        " alpha".to_owned(),
        String::new(),
        "x".repeat(65),
    ])
    .prop_map(|name| json!({"name": name}))
    .boxed();
    mix(valid, invalid)
}

fn restore() -> BoxedStrategy<Value> {
    // Names, or the ids of the first checkpoints (`cp_<n>`, counted per document actor).
    let checkpoint = prop_oneof![
        select(CHECKPOINT_NAMES.to_vec()).prop_map(str::to_owned),
        (1..=3_u32).prop_map(|n| format!("cp_{n}")),
    ];
    let valid = (checkpoint, prop::bool::weighted(0.3))
        .prop_map(|(checkpoint, force)| json!({"checkpoint": checkpoint, "force": force}))
        .boxed();
    mix(valid, Just(json!({"checkpoint": "nope"})).boxed())
}

fn delete_checkpoint() -> BoxedStrategy<Value> {
    let checkpoint = prop_oneof![
        select(CHECKPOINT_NAMES.to_vec()).prop_map(str::to_owned),
        (1..=3_u32).prop_map(|n| format!("cp_{n}")),
    ];
    let valid = checkpoint
        .prop_map(|checkpoint| json!({"checkpoint": checkpoint}))
        .boxed();
    mix(valid, Just(json!({"checkpoint": 7})).boxed())
}

// ----- job.* ---------------------------------------------------------------------------------

/// One of the jobs [`job_manager`] and the harness imports create (usually), or an unknown one.
fn job_id() -> BoxedStrategy<Value> {
    let known = select(known_job_ids()).prop_map(|id| Value::String(id.to_string()));
    let unknown = any::<u128>().prop_map(|raw| Value::String(JobId::from_u128(raw).to_string()));
    prop_oneof![9 => known, 1 => unknown].boxed()
}

/// `{<key>: job}` (`job.status`, `job.cancel`).
fn job_ref(key: &'static str) -> BoxedStrategy<Value> {
    let object = move |id: Value| {
        let mut params = serde_json::Map::new();
        params.insert(key.to_owned(), id);
        Value::Object(params)
    };
    let valid = job_id().prop_map(object).boxed();
    let invalid = prop_oneof![
        Just(object(json!("job_1"))),
        fresh::<MediaId>().prop_map(object),
    ]
    .boxed();
    mix(valid, invalid)
}

fn job_list(p: &Pools) -> BoxedStrategy<Value> {
    let state = select(vec![
        "queued",
        "running",
        "succeeded",
        "failed",
        "cancelled",
    ]);
    let kind = select(vec!["analysis", "export"]);
    let valid = (
        option::of(one_of(p.media_ids())),
        option::of(state),
        option::of(kind),
        option::of(1..=20_u32),
    )
        .prop_map(|(media_id, state, kind, limit)| {
            json!({"media_id": media_id, "state": state, "kind": kind, "limit": limit})
        })
        .boxed();
    let invalid = prop_oneof![
        Just(json!({"limit": 0})),
        Just(json!({"limit": 501})),
        Just(json!({"state": "done"})),
    ]
    .boxed();
    mix(valid, invalid)
}

/// `job.wait` with a timeout of at most 5 ms. Not `mix`ed: a malformed variant could drop
/// `timeout_ms` and wait the 30 s default on a running job.
fn job_wait() -> BoxedStrategy<Value> {
    let valid = (job_id(), 0..=5_u32)
        .prop_map(|(id, ms)| json!({"job_id": id, "timeout_ms": ms}))
        .boxed();
    let invalid = prop_oneof![
        Just(json!({"job_id": "job_1", "timeout_ms": 0})),
        job_id().prop_map(|id| json!({"job_id": id, "timeout_ms": -1})),
        job_id().prop_map(|id| json!({"job_id": id, "timeout_ms": 1, "unexpected": true})),
        Just(json!({"id": "job_1", "timeout_ms": 0})),
    ]
    .boxed();
    prop_oneof![16 => valid, 4 => invalid].boxed()
}

// ----- export --------------------------------------------------------------------------------

/// File name of the `export.start` output in the OS temp folder. Harnesses give the command a
/// starter that queues nothing, so no file is written there (only the writability check's
/// probe file, created and removed at once).
pub const EXPORT_FILE_NAME: &str = "cutweave-props-export.mp4";

/// `export.start` to [`EXPORT_FILE_NAME`] in the OS temp folder: the whole sequence or a range
/// on the 30 fps grid (sometimes past the end), the preset or custom settings.
fn export_start(p: &Pools) -> BoxedStrategy<Value> {
    let path = std::env::temp_dir()
        .join(EXPORT_FILE_NAME)
        .to_string_lossy()
        .into_owned();
    let custom_range =
        (0..120_i64, 1..120_i64, any::<u8>(), any::<u8>()).prop_map(|(at, len, fa, fb)| {
            json!({"in": time_json(FPF_30, at, fa, 0), "out": time_json(FPF_30, at + len, fb, 0)})
        });
    // The range kinds of WP-2G.03b: the sequence's in/out points and marker sets are often
    // refused (no points set, a plain file name for several files), which is the point.
    let range = prop_oneof![
        4 => custom_range,
        1 => Just(json!({"kind": "sequence"})),
        1 => Just(json!({"kind": "in_out"})),
        1 => Just(json!({"kind": "markers"})),
        1 => Just(json!({"kind": "markers", "pair_mode": "pairs"})),
    ];
    let custom = (
        option::of(select(vec![320_u32, 640, 1280, 1920])),
        option::of(select(vec![180_u32, 360, 720, 1080])),
        option::of(select(FrameRate::COMMON.to_vec())),
        option::of(select(vec!["low", "medium", "high"])),
    )
        .prop_map(|(width, height, fps, quality)| {
            json!({"custom": {"width": width, "height": height, "fps": fps, "quality": quality}})
        });
    let preset = prop_oneof![
        2 => Just(Value::Null),
        1 => Just(json!("h264-mp4-1080p")),
        2 => custom,
    ];
    let to = path.clone();
    let valid = (
        one_of(p.seqs.clone()),
        option::weighted(0.4, range),
        preset,
        option::of(select(vec![
            json!("auto"),
            json!("off"),
            json!({"force": "h264_nvenc"}),
        ])),
        option::of(any::<bool>()),
        option::of(select(vec!["rename", "overwrite", "fail"])),
        option::of(any::<bool>()),
        option::weighted(0.4, advanced_block()),
    )
        .prop_map(
            move |(sequence, range, preset, hw, overwrite, collision, allow_offline, advanced)| {
                json!({"sequence": sequence, "path": to, "range": range, "preset": preset,
                       "hw": hw, "overwrite": overwrite, "collision": collision,
                       "allow_offline": allow_offline, "advanced": advanced})
            },
        )
        .boxed();
    let in_temp = move |name: &str| {
        std::env::temp_dir()
            .join(name)
            .to_string_lossy()
            .into_owned()
    };
    let invalid = prop_oneof![
        Just(json!({"path": "relative/out.mp4"})),
        Just(json!({"path": in_temp("cutweave-props-export.mov")})),
        Just(json!({"path": in_temp("cutweave-missing-folder/out.mp4")})),
        Just(json!({"path": path, "preset": {"custom": {"width": 641}}})),
        Just(json!({"path": path, "preset": {"custom": {"fps": {"num": 1000, "den": 1}}}})),
        Just(json!({"path": path, "range": {"in": "2s", "out": "1s"}})),
        Just(json!({"path": path, "range": {"kind": "wholesale"}})),
        Just(json!({"path": path, "overwrite": true, "collision": "rename"})),
        Just(json!({"path": path, "preset": "h265-mkv"})),
        Just(json!({"path": path, "hw": {"force": "H264 NVENC"}})),
        Just(json!({"path": path, "hw": "nvenc"})),
        Just(json!({"path": path, "advanced": {"video": {"nonsense": 1}}})),
        Just(json!({"path": path, "advanced": {"video": {"gop": {"keyint": 0}}}})),
        Just(json!({"path": path, "advanced": {"audio": {"sample_rate": 22050}}})),
    ]
    .boxed();
    mix(valid, invalid)
}

/// An `advanced` block of `export.start`: mostly workable ones, some the checks refuse (ProRes
/// with a CRF, constant bitrate on AV1, HDR, two-pass).
fn advanced_block() -> BoxedStrategy<Value> {
    select(vec![
        json!({}),
        json!({"video": {"rate_control": {"mode": "crf", "crf": 18}}}),
        json!({"video": {"rate_control": {"mode": "cbr", "bitrate_kbps": 8000}}}),
        json!({"video": {"rate_control": {"mode": "vbr", "bitrate_kbps": 6000, "max_bitrate_kbps": 9000, "buffer_kbit": 6000}}}),
        json!({"video": {"rate_control": {"mode": "cq", "crf": 22, "max_bitrate_kbps": 5000}}}),
        json!({"video": {"rate_control": {"mode": "vbr", "bitrate_kbps": 4000, "two_pass": true}}}),
        json!({"video": {"gop": {"keyint_seconds": 2, "scenecut": false, "closed_gop": true, "bframes": 2}}}),
        json!({"video": {"gop": {"keyint": 60, "keyint_min": 30}, "preset": "slow", "tune": "film", "level": "4.1"}}),
        json!({"video": {"profile": "main10", "pix_fmt": "yuv420p10le", "threads": 4}}),
        json!({"color": {"primaries": "smpte170m", "transfer": "smpte170m", "matrix": "smpte170m", "range": "full", "tag_only": true}}),
        json!({"color": {"transfer": "smpte2084"}}),
        json!({"audio": {"codec": "opus", "bitrate_kbps": 128, "channels": "mono"}}),
        json!({"audio": {"codec": "pcm_s16", "sample_rate": 96000}}),
    ])
    .boxed()
}

/// `export.describe_options`: a codec, sometimes with an encoder or container, sometimes wrong.
fn describe_options() -> BoxedStrategy<Value> {
    let valid = (
        select(vec!["h264", "hevc", "av1", "prores"]),
        option::of(select(vec![
            "libx264",
            "libx265",
            "hevc_nvenc",
            "h264_amf",
            "av1_qsv",
        ])),
        option::of(select(vec!["mp4", "mov"])),
    )
        .prop_map(|(codec, encoder, container)| {
            json!({"codec": codec, "encoder": encoder, "container": container})
        })
        .boxed();
    let invalid = prop_oneof![
        Just(json!({})),
        Just(json!({"codec": "vp9"})),
        Just(json!({"codec": "hevc", "encoder": "h264_nvenc"})),
    ]
    .boxed();
    mix(valid, invalid)
}

/// `export.pause_queue`: `{paused}`, sometimes malformed.
fn pause_queue() -> BoxedStrategy<Value> {
    let valid = any::<bool>()
        .prop_map(|paused| json!({"paused": paused}))
        .boxed();
    let invalid = prop_oneof![
        Just(json!({})),
        Just(json!({"paused": "yes"})),
        Just(json!({"paused": true, "parallel": 2})),
    ]
    .boxed();
    mix(valid, invalid)
}

/// One of the exports [`export_queue`] starts with (the 4th to 6th job IDs), usually, or any
/// job the `job.*` strategies name.
fn export_job_id() -> BoxedStrategy<Value> {
    let queued = select(known_job_ids()[3..6].to_vec())
        .prop_map(|id| Value::String(id.to_string()))
        .boxed();
    prop_oneof![4 => queued, 1 => job_id()].boxed()
}

/// `{id: export}` (`export.cancel`, `export.retry`, `export.remove`).
fn export_job_ref() -> BoxedStrategy<Value> {
    let valid = export_job_id().prop_map(|id| json!({"id": id})).boxed();
    let invalid = prop_oneof![
        Just(json!({"id": "job_1"})),
        fresh::<MediaId>().prop_map(|id| json!({"id": id})),
    ]
    .boxed();
    mix(valid, invalid)
}

/// `export.reorder`: a job before another one, or to the end.
fn export_reorder() -> BoxedStrategy<Value> {
    let valid = (export_job_id(), option::weighted(0.3, export_job_id()))
        .prop_map(|(id, before)| json!({"id": id, "before": before}))
        .boxed();
    let invalid = prop_oneof![
        Just(json!({"before": "job_1"})),
        export_job_id().prop_map(|id| json!({"id": id, "before": 3})),
    ]
    .boxed();
    mix(valid, invalid)
}

/// The job id [`export_ctx`]'s starter returns for every export it is given.
pub const EXPORT_JOB_ID: JobId = JobId::from_u128(0xE0);

/// A proxy service that makes none (the engine context of the proxy commands in a harness,
/// WP-2F.01): every asset reports `none`, and generating or deleting changes nothing.
#[derive(Debug, Default)]
pub struct NoProxies;

impl crate::media::ProxyService for NoProxies {
    fn status(
        &self,
        _doc: &cutweave_model::Project,
        media: &[cutweave_model::MediaId],
    ) -> Vec<crate::media::proxy::ProxyInfo> {
        media
            .iter()
            .map(|id| crate::media::proxy::ProxyInfo::none(*id))
            .collect()
    }

    fn generate(
        &self,
        doc: &cutweave_model::Project,
        media: &[cutweave_model::MediaId],
    ) -> Result<Vec<crate::media::proxy::ProxyInfo>, cutweave_commands::CmdError> {
        Ok(self.status(doc, media))
    }

    fn delete(
        &self,
        doc: &cutweave_model::Project,
        media: &[cutweave_model::MediaId],
    ) -> Result<Vec<crate::media::proxy::ProxyInfo>, cutweave_commands::CmdError> {
        Ok(self.status(doc, media))
    }

    fn imported(
        &self,
        _media: cutweave_model::MediaId,
        _after: &[JobId],
    ) -> crate::media::ImportProxy {
        crate::media::ImportProxy::None
    }
}

/// Takes export plans and queues nothing (see [`export_ctx`]).
struct QueueNothing;

impl ExportStarter for QueueNothing {
    fn start(&mut self, _plan: ExportPlan) -> Result<JobId, cutweave_commands::CmdError> {
        Ok(EXPORT_JOB_ID)
    }
}

/// The engine context of the preset commands in a harness: `store` and `snapshot`.
#[must_use]
pub fn preset_ctx(
    store: Arc<crate::export::presets::PresetStore>,
    snapshot: Arc<DocSnapshot>,
) -> crate::export::presets::PresetCtx {
    crate::export::presets::PresetCtx {
        store,
        snapshot: Some(snapshot),
    }
}

/// The engine context of `export.start` in a harness: `snapshot` (the committed document), no
/// path allowlist, and a starter that queues nothing and answers [`EXPORT_JOB_ID`] (the queue
/// commands answer `Unsupported`).
#[must_use]
pub fn export_ctx(snapshot: Arc<DocSnapshot>) -> ExportCtx {
    ExportCtx {
        snapshot,
        allowed_roots: None,
        starter: Box::new(QueueNothing),
        presets: Arc::default(),
    }
}

/// An export queue for a harness (WP-2G.03a) on `jobs`: never started, so its exports wait in
/// line (deterministically) until cancelled or removed; nothing is rendered or written. It
/// starts with three exports of an empty document, so the `export.*` strategies (which name
/// the first job IDs of [`JOB_ID_SEED`]) find some: the 4th ID waits, the 5th was cancelled
/// (it can be retried), the 6th waits.
#[must_use]
pub fn export_queue(jobs: &JobManager) -> cutweave_jobs::ExportQueue<ExportPlan> {
    let queue = cutweave_jobs::ExportQueue::new(
        jobs.clone(),
        Arc::new(|_: &ExportPlan, ctx: &JobCtx| ctx.check()),
        Arc::new(crate::export::queue::PlanCodec),
        1,
    );
    let plan = || ExportPlan {
        snapshot: Arc::new(DocSnapshot {
            rev: cutweave_commands::Rev::ZERO,
            committed_rev: cutweave_commands::Rev::ZERO,
            doc: Project::new("Queued"),
        }),
        sequence: SequenceId::from_u128(1),
        path: std::env::temp_dir().join(EXPORT_FILE_NAME),
        overwrite: true,
        allow_offline: false,
        start: cutweave_model::Time::ZERO,
        end: cutweave_model::Time::from_secs(1).unwrap_or(cutweave_model::Time::ZERO),
        width: 64,
        height: 36,
        frame_rate: FrameRate::FPS_30,
        frames: 30,
        duration: cutweave_model::Duration::from_secs(1).unwrap_or_default(),
        quality: crate::export::ExportQuality::Low,
        hw: crate::export::ExportHw::Off,
        rate: None,
        warnings: Vec::new(),
        output: crate::export::PlanOutput::default(),
    };
    let mut ids = Vec::new();
    for _ in 0..3 {
        let item = cutweave_jobs::QueueItem {
            label: "export".to_owned(),
            output: None,
            task: plan(),
        };
        if let Ok(id) = queue.enqueue(item, false) {
            ids.push(id);
        }
    }
    if let Some(id) = ids.get(1) {
        let _ = queue.cancel(*id);
    }
    queue
}

/// Adds exports to a harness's queue without claiming their output (the strategies name one
/// file).
struct QueueInLine(cutweave_jobs::ExportQueue<ExportPlan>);

impl QueueInLine {
    fn add(&self, plan: ExportPlan, resume: bool) -> Result<JobId, cutweave_commands::CmdError> {
        let item = cutweave_jobs::QueueItem {
            label: "export".to_owned(),
            output: None,
            task: plan,
        };
        self.0
            .enqueue(item, resume)
            .map_err(|e| crate::export::queue::queue_error(e, "/path"))
    }
}

impl ExportStarter for QueueInLine {
    fn start(&mut self, plan: ExportPlan) -> Result<JobId, cutweave_commands::CmdError> {
        self.add(plan, true)
    }

    fn enqueue(&mut self, plan: ExportPlan) -> Result<JobId, cutweave_commands::CmdError> {
        self.add(plan, false)
    }

    fn queue(
        &mut self,
    ) -> Result<cutweave_jobs::ExportQueue<ExportPlan>, cutweave_commands::CmdError> {
        Ok(self.0.clone())
    }
}

/// The engine context of the `export.*` commands in a harness with an [`export_queue`].
#[must_use]
pub fn queue_ctx(
    snapshot: Arc<DocSnapshot>,
    queue: &cutweave_jobs::ExportQueue<ExportPlan>,
) -> ExportCtx {
    ExportCtx {
        snapshot,
        allowed_roots: None,
        starter: Box::new(QueueInLine(queue.clone())),
        presets: Arc::default(),
    }
}
