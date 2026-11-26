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
    EntityId, FolderId, FrameRate, ItemId, MarkerId, MediaId, MediaKind, ProbeInfo, Project,
    SampleRate, SeededGen, SequenceId, TrackId, TrackKind, Vec2,
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

/// The prober a harness gives `media.import`: it knows [`IMPORT_PATHS`] and fails
/// [`BROKEN_IMPORT_PATH`].
#[must_use]
pub fn fake_probe() -> FakeProbe {
    let [interview, broll, music, logo, take, voice] = IMPORT_PATHS;
    FakeProbe::new()
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
pub(super) fn args_for(name: &str, p: &Pools) -> Option<(u32, BoxedStrategy<Value>)> {
    let edits: u32 = if p.items.is_empty() { 1 } else { 6 };
    let empty = p.seqs.is_empty() && p.media.is_empty();
    let no_media = p.media.is_empty();
    Some(match name {
        "project.new" => (if empty { 12 } else { 1 }, project_new()),
        "project.rename" => (1, project_rename()),
        "project.info"
        | "edit.locks"
        | "edit.list_checkpoints"
        | "export.list_presets"
        | "media.list_folders" => (1, no_params()),
        "media.register" => (if no_media { 4 } else { 2 }, media_register(p)),
        "media.register_tree" => (if no_media { 3 } else { 1 }, media_register_tree(p)),
        "media.import" => (if no_media { 4 } else { 2 }, media_import(p)),
        "media.list" => (1, media_list(p)),
        "media.remove" => (1, media_remove(p)),
        "media.generate_proxy" | "media.delete_proxy" => (1, proxy_media(p, false)),
        "media.proxy_status" => (1, proxy_media(p, true)),
        "media.create_folder" => (
            if p.folders.is_empty() { 4 } else { 2 },
            media_create_folder(p),
        ),
        "media.rename_folder" => (1, media_rename_folder(p)),
        "media.rename" => (1, media_rename(p)),
        "media.move" => (if p.folders.is_empty() { 1 } else { 3 }, media_move(p)),
        "media.delete_folder" => (1, media_delete_folder(p)),
        "track.add" => (2, track_add(p)),
        "track.remove" => (1, track_remove(p)),
        "track.set" => (2, track_set(p)),
        "track.reorder" => (1, track_reorder(p)),
        "track.set_height" => (1, track_set_height(p)),
        "track.set_magnetic" => (2, track_set_magnetic(p)),
        "track.list" => (1, track_list(p)),
        "timeline.add_marker" => (2, timeline_add_marker(p)),
        "timeline.move_marker" => (1, timeline_move_marker(p)),
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
        "item.move" => (edits, item_move(p)),
        "item.delete" => (edits.div_ceil(2), item_delete(p)),
        "item.duplicate" => (edits.div_ceil(2), item_duplicate(p)),
        "item.set_props" => (edits.div_ceil(2), item_set_props(p)),
        "item.get" => (1, item_get(p)),
        "item.reset_props" => (1, item_reset_props(p)),
        "audio.set" => (edits.div_ceil(2), audio_set(p)),
        "audio.set_fade" => (edits.div_ceil(2), audio_set_fade(p)),
        "audio.set_volume" => (edits.div_ceil(2), audio_set_volume(p)),
        "audio.set_pan" => (edits.div_ceil(2), audio_set_pan(p)),
        "audio.set_gain" => (edits.div_ceil(3), audio_set_gain(p)),
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
        "edit.checkpoint" => (2, checkpoint()),
        "edit.restore" => (2, restore()),
        "edit.delete_checkpoint" => (1, delete_checkpoint()),
        "job.status" | "job.cancel" => (1, job_ref("id")),
        "job.list" => (1, job_list(p)),
        "job.wait" => (1, job_wait()),
        "export.start" => (1, export_start(p)),
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
    let valid = (name(), option::of(format))
        .prop_map(|(name, format)| json!({"name": name, "format": format}))
        .boxed();
    let invalid = prop_oneof![
        bad_name().prop_map(|name| json!({"name": name})),
        Just(json!({"name": "P", "format": {"width": 0}})),
        Just(json!({"name": "P", "format": {"height": 20_000}})),
        Just(json!({"name": "P", "format": {"frame_rate": {"num": 2000, "den": 1}}})),
        Just(json!({"name": "P", "format": {"frame_rate": {"num": 1001, "den": 7}}})),
        Just(json!({"name": "P", "format": {"sample_rate": 1234}})),
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
    )
        .prop_map(|(paths, unreadable, folder)| {
            let mut unique: Vec<&str> = Vec::new();
            for p in paths.into_iter().chain(unreadable) {
                if !unique.contains(&p) {
                    unique.push(p);
                }
            }
            json!({"paths": unique, "folder": folder})
        })
        .boxed();
    let invalid = prop_oneof![
        Just(json!({"paths": []})),
        Just(json!({"paths": ["import/relative.mp4"]})),
        select(IMPORT_PATHS.to_vec()).prop_map(|p| json!({"paths": [p, p]})),
    ]
    .boxed();
    mix(valid, invalid)
}

fn media_list(p: &Pools) -> BoxedStrategy<Value> {
    let filter = (
        option::weighted(0.5, one_of(p.folders.clone())),
        option::of(any::<bool>()),
        option::weighted(0.2, select(vec!["clip", "Clip 1", "interview"])),
    )
        .prop_map(|(folder, recursive, text)| {
            json!({"folder": folder, "recursive": recursive, "text": text})
        });
    let valid = (
        option::of(1..=8_u32),
        option::of(one_of(p.media_ids())),
        option::weighted(0.4, filter),
    )
        .prop_map(
            |(limit, cursor, filter)| json!({"limit": limit, "cursor": cursor, "filter": filter}),
        )
        .boxed();
    let invalid = prop_oneof![
        Just(json!({"limit": 0})),
        Just(json!({"limit": 501})),
        fresh::<FolderId>().prop_map(|folder| json!({"filter": {"folder": folder}})),
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
    let valid = vec(one_of(p.media_ids()), 1..=3)
        .prop_map(|ids| json!({ "media": ids }))
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
    let range = (0..120_i64, 1..120_i64, any::<u8>(), any::<u8>()).prop_map(|(at, len, fa, fb)| {
        json!({"in": time_json(FPF_30, at, fa, 0), "out": time_json(FPF_30, at + len, fb, 0)})
    });
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
    )
        .prop_map(move |(sequence, range, preset, hw, overwrite)| {
            json!({"sequence": sequence, "path": to, "range": range, "preset": preset,
                   "hw": hw, "overwrite": overwrite})
        })
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
        Just(json!({"path": path, "preset": "h265-mkv"})),
        Just(json!({"path": path, "hw": {"force": "H264 NVENC"}})),
        Just(json!({"path": path, "hw": "nvenc"})),
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
/// path allowlist, and a starter that queues nothing and answers [`EXPORT_JOB_ID`].
#[must_use]
pub fn export_ctx(snapshot: Arc<DocSnapshot>) -> ExportCtx {
    ExportCtx {
        snapshot,
        allowed_roots: None,
        starter: Box::new(QueueNothing),
        presets: Arc::default(),
    }
}
