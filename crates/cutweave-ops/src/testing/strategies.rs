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
    EntityId, FrameRate, ItemId, MediaId, MediaKind, ProbeInfo, Project, SampleRate, SeededGen,
    SequenceId, TrackId, TrackKind, Vec2,
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

/// Checkpoint names the `edit.checkpoint` and `edit.restore` strategies share.
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
    kind: TrackKind,
    fpf: i64,
    /// Start and length in frames.
    start: i64,
    len: i64,
    /// End of the sequence, in frames.
    seq_end: i64,
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
    tracks: Vec<Trk>,
    items: Vec<Itm>,
    media: Vec<Med>,
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
                        kind: t.kind,
                        fpf,
                        start: frames(i.start().flicks()),
                        len: frames(i.duration().flicks()).max(1),
                        seq_end,
                    });
                    used.extend(i.media());
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

/// The weight and the param strategy of `name` against `p`, or `None` if this table has no
/// strategy for it.
pub(super) fn args_for(name: &str, p: &Pools) -> Option<(u32, BoxedStrategy<Value>)> {
    let edits = if p.items.is_empty() { 1 } else { 6 };
    let empty = p.seqs.is_empty() && p.media.is_empty();
    let no_media = p.media.is_empty();
    Some(match name {
        "project.new" => (if empty { 12 } else { 1 }, project_new()),
        "project.rename" => (1, project_rename()),
        "project.info" | "edit.locks" | "edit.list_checkpoints" => (1, no_params()),
        "media.register" => (if no_media { 4 } else { 2 }, media_register(p)),
        "media.import" => (if no_media { 4 } else { 2 }, media_import()),
        "media.list" => (1, media_list(p)),
        "media.remove" => (1, media_remove(p)),
        "track.add" => (2, track_add(p)),
        "track.remove" => (1, track_remove(p)),
        "track.set" => (2, track_set(p)),
        "timeline.insert" => (if p.seqs.is_empty() { 1 } else { 8 }, timeline_insert(p)),
        "timeline.get" => (1, timeline_get(p)),
        "clip.split" => (edits, clip_split(p)),
        "clip.trim" => (edits, clip_trim(p)),
        "item.move" => (edits, item_move(p)),
        "item.delete" => (edits.div_ceil(2), item_delete(p)),
        "item.set_props" => (edits.div_ceil(2), item_set_props(p)),
        "item.get" => (1, item_get(p)),
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
        _ => return None,
    })
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
    let valid = (0..40_u32, 0..3_u8, 1..=90_i64, any::<bool>(), media_id)
        .prop_map(|(n, kind, secs, audio, media_id)| {
            let (path, probed) = probed_file(n, kind, secs, audio);
            json!({"media_id": media_id, "path": path, "probe": probed.probe,
                   "content_hash": probed.content_hash})
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

fn media_import() -> BoxedStrategy<Value> {
    // Unreadable files are not errors: the import reports them in `failed[]` and registers
    // the rest (WP-1D.07), so they belong with the valid requests.
    let unreadable = prop_oneof![
        8 => Just(None),
        1 => Just(Some(BROKEN_IMPORT_PATH)),
        1 => Just(Some("/import/missing.mp4")),
    ];
    let valid = (vec(select(IMPORT_PATHS.to_vec()), 1..=3), unreadable)
        .prop_map(|(paths, unreadable)| {
            let mut unique: Vec<&str> = Vec::new();
            for p in paths.into_iter().chain(unreadable) {
                if !unique.contains(&p) {
                    unique.push(p);
                }
            }
            json!({"paths": unique})
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
    let valid = (option::of(1..=8_u32), option::of(one_of(p.media_ids())))
        .prop_map(|(limit, cursor)| json!({"limit": limit, "cursor": cursor}))
        .boxed();
    let invalid = prop_oneof![Just(json!({"limit": 0})), Just(json!({"limit": 501}))].boxed();
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
    let valid = select(targets)
        .prop_flat_map(|t| {
            let max = t.frames.unwrap_or(300);
            (
                Just(t.clone()),
                prop_oneof![3 => t.end..t.end + 30, 1 => 0..t.end + 60],
                option::weighted(0.3, 0..max),
                option::weighted(0.6, 1..=max),
                any::<(u8, u8, u8, u32)>(),
                option::weighted(0.4, prop_oneof![Just("reject"), Just("new_track")]),
                option::weighted(0.2, any::<bool>()),
            )
        })
        .prop_map(|(t, at, src, dur, (fa, fs, fd, jitter), overlap, link)| {
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
                "overlap": overlap, "link": link,
            })
        })
        .boxed();
    mix(valid, invalid)
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
            (Just(it), at, any::<u8>(), any::<u32>())
        })
        .prop_map(|(it, at, form, jitter)| {
            json!({"item": it.id, "at": time_json(it.fpf, at, form, jitter)})
        })
        .boxed();
    mix(valid, unknown_item("at"))
}

fn clip_trim(p: &Pools) -> BoxedStrategy<Value> {
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
    )
        .prop_map(|(it, end, k, mode, (form, jitter), unlink)| {
            let edge = if end { it.start + it.len } else { it.start };
            let mut v = json!({"item": it.id, "edge": if end { "end" } else { "start" },
                               "unlink": unlink});
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
    let valid = select(p.items.clone())
        .prop_flat_map(move |it| {
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
            )
        })
        .prop_map(|(it, start, (form, jitter), to_track, unlink, overlap)| {
            json!({"item": it.id, "start": time_json(it.fpf, start, form, jitter),
                   "to_track": to_track, "unlink": unlink, "overlap": overlap})
        })
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
    let props = (
        option::weighted(0.3, prop_oneof![4 => name(), 1 => Just(String::new())]),
        option::weighted(0.3, any::<bool>()),
        option::weighted(0.4, prop_oneof![Just(0.0), Just(1.0), 0.0..=1.0_f64]),
        option::weighted(0.3, gain()),
        option::weighted(0.4, transform),
    )
        .prop_map(|(name, enabled, opacity, audio_gain, transform)| {
            json!({"name": name, "enabled": enabled, "opacity": opacity,
                   "audio_gain": audio_gain, "transform": transform})
        });
    let valid = (one_of(p.item_ids()), props)
        .prop_map(|(item, props)| json!({"item": item, "props": props}))
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

/// Takes export plans and queues nothing (see [`export_ctx`]).
struct QueueNothing;

impl ExportStarter for QueueNothing {
    fn start(&mut self, _plan: ExportPlan) -> Result<JobId, cutweave_commands::CmdError> {
        Ok(EXPORT_JOB_ID)
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
    }
}
