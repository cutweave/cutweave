// SPDX-License-Identifier: GPL-3.0-or-later
//! Journal test fixtures (WP-1C.02): a project with a few tracks, document commands that edit
//! it (including a `#[secret]` one), a journaled session over a project folder, and a
//! deterministic pipelined workload shared by the crash-recovery child and its reference run.

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::must_use_candidate
)]

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use cutweave_commands::{
    ActorCtx, Applied, Clock, CmdError, Command, ConnId, DocActor, DocConfig, DocHandle,
    ManualClock, Pending, Registry, RequestId, Submit, Tx, TxId, WallTime, apply_patch, command,
    wire,
};
use cutweave_engine::journal::{
    IdTracker, Journal, JournalConfig, RecoverOptions, Recovered, SyncPolicy,
};
use cutweave_engine::project_io::{OpenReport, ProjectStore};
use cutweave_model::{
    Clip, Duration, FrameRate, IdGen, Item, ItemId, MediaAsset, MediaKind, ProbeInfo, Project,
    SampleRate, SeededGen, Sequence, SequenceFormat, Track, TrackId, TrackKind,
};
use serde_json::{Value, json};

/// One frame at 30 fps, in flicks.
pub const FRAME: i64 = 23_520_000;

/// The `--deterministic-ids` seed of every session in these tests.
pub const SEED: u64 = 42;

/// The secret the `ai.set_key` tests plant; it must never appear in a journal file.
pub const SENTINEL: &str = "sk-SENTINEL-0123456789abcdef";

/// A temp directory removed on drop.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> TempDir {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "cutweave-jnl-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A project with one 30 fps sequence of `tracks` empty video tracks and one media asset.
/// Saved documents depend on the folder only through the asset's `path_rel`, which is the same
/// for project folders at the same depth (siblings).
pub fn fixture(tracks: usize) -> Project {
    let mut ids = SeededGen::new(7);
    let mut project = Project::new("Journal fixture");
    let probe = ProbeInfo {
        duration: Some(Duration::from_secs(3600).unwrap()),
        ..ProbeInfo::default()
    };
    let media = MediaAsset::file(ids.mint(), MediaKind::Video, "/media/a.mp4", probe);
    let format = SequenceFormat::new(1920, 1080, FrameRate::FPS_30, SampleRate::HZ_48_000);
    let mut sequence = Sequence::new(ids.mint(), "Main", format);
    for i in 0..tracks {
        let track = Track::new(ids.mint(), TrackKind::Video, format!("V{}", i + 1));
        sequence.insert_track(i, track);
    }
    project.media.insert(media.id, media);
    project.sequences.insert(sequence.id, sequence);
    project.validate().unwrap();
    project
}

/// The track ids of `doc`, in id order.
pub fn tracks(doc: &Project) -> Vec<TrackId> {
    let mut out: Vec<TrackId> = doc
        .sequences
        .values()
        .flat_map(|s| s.tracks.values().map(|t| t.id))
        .collect();
    out.sort();
    out
}

/// Every item id of `doc`, in id order.
pub fn items(doc: &Project) -> Vec<ItemId> {
    let mut out: Vec<ItemId> = doc
        .sequences
        .values()
        .flat_map(|s| s.tracks.values())
        .flat_map(|t| t.items.iter_by_id().map(|(id, _)| *id))
        .collect();
    out.sort();
    out
}

/// The new item.
#[wire]
pub struct ItemOut {
    /// Its id.
    pub item: ItemId,
}

/// Nothing.
#[wire]
#[derive(Default)]
pub struct Done {}

/// Appends a clip of `frames` frames at the end of `track`.
#[command(
    name = "timeline.append_clip",
    summary = "Append a clip at the end of a track",
    undoable,
    mcp = "hidden",
    annotations(destructive = false)
)]
pub struct AppendClip {
    /// The track.
    pub track: TrackId,
    /// Length in frames (at least 1).
    pub frames: u32,
}

impl Command for AppendClip {
    type Output = ItemOut;

    fn apply(&self, tx: &mut Tx<'_>) -> Result<ItemOut, CmdError> {
        if self.frames == 0 {
            return Err(CmdError::invalid("/frames", "must be at least 1"));
        }
        let start = tx.track(self.track)?.1.items.end();
        let media = *tx
            .doc()
            .media
            .keys()
            .next()
            .ok_or_else(|| CmdError::invalid("", "no media"))?;
        let duration = Duration::from_flicks(i64::from(self.frames) * FRAME)
            .map_err(|e| CmdError::internal(e.to_string()))?;
        let clip = Clip::new(tx.new_id(), media, start, duration);
        let item = clip.id;
        tx.insert_item(self.track, &Item::from(clip))?;
        Ok(ItemOut { item })
    }
}

/// Renames an item.
#[command(
    name = "item.rename",
    summary = "Rename an item",
    undoable,
    mcp = "hidden",
    annotations(destructive = false)
)]
pub struct RenameItem {
    /// The item.
    pub item: ItemId,
    /// Its new name.
    pub name: String,
}

impl Command for RenameItem {
    type Output = Done;

    fn apply(&self, tx: &mut Tx<'_>) -> Result<Done, CmdError> {
        tx.update_item(self.item, |item| {
            let clip = item
                .as_clip_mut()
                .ok_or_else(|| CmdError::unsupported("only clips have names here"))?;
            clip.name = Some(self.name.clone());
            Ok(Done {})
        })
    }
}

/// Deletes an item.
#[command(
    name = "item.delete",
    summary = "Delete an item",
    undoable,
    mcp = "hidden",
    annotations(destructive = false)
)]
pub struct DeleteItem {
    /// The item.
    pub item: ItemId,
}

impl Command for DeleteItem {
    type Output = Done;

    fn apply(&self, tx: &mut Tx<'_>) -> Result<Done, CmdError> {
        tx.remove_item(self.item)?;
        Ok(Done {})
    }
}

/// Renames the project.
#[command(
    name = "project.rename",
    summary = "Rename the project",
    undoable,
    mcp = "hidden",
    annotations(destructive = false)
)]
pub struct RenameProject {
    /// The new name.
    pub name: String,
}

impl Command for RenameProject {
    type Output = Done;

    fn apply(&self, tx: &mut Tx<'_>) -> Result<Done, CmdError> {
        tx.update_meta(|meta| {
            meta.name.clone_from(&self.name);
            Ok(())
        })?;
        Ok(Done {})
    }
}

/// Stores a provider key (stand-in for `ai.set_key`): stamps the provider into the project
/// name so the transaction changes the document and is journaled. The key is `#[secret]`.
#[command(
    name = "ai.set_key",
    summary = "Store an API key",
    scope = "ui",
    undoable,
    mcp = "hidden",
    annotations(destructive = false)
)]
pub struct SetKey {
    /// Provider.
    pub provider: String,
    /// The key.
    #[secret]
    pub key: String,
}

impl Command for SetKey {
    type Output = Done;

    fn apply(&self, tx: &mut Tx<'_>) -> Result<Done, CmdError> {
        tx.update_meta(|meta| {
            meta.name = format!("key for {}", self.provider);
            Ok(())
        })?;
        Ok(Done {})
    }
}

/// Every test command plus the production `edit.*` commands.
pub fn registry() -> Arc<Registry> {
    let mut r = Registry::new();
    r.register::<AppendClip>()
        .unwrap()
        .register::<RenameItem>()
        .unwrap()
        .register::<DeleteItem>()
        .unwrap()
        .register::<RenameProject>()
        .unwrap()
        .register::<SetKey>()
        .unwrap();
    cutweave_commands::edit::register(&mut r).unwrap();
    Arc::new(r)
}

/// The UI on connection 1.
pub fn ui() -> ActorCtx {
    ActorCtx::ui(ConnId::new(1))
}

/// An agent on connection 2.
pub fn agent() -> ActorCtx {
    ActorCtx::agent(ConnId::new(2), "claude").unwrap()
}

/// Creates the project folder `<dir>/<name>` holding `doc` (and releases it).
pub fn create_project(dir: &Path, name: &str, doc: &Project) -> PathBuf {
    let folder = dir.join(name);
    ProjectStore::create(&folder, doc).unwrap().close().unwrap();
    folder
}

/// A journaled session over a project folder: store, journal and doc actor.
pub struct Session {
    pub store: ProjectStore,
    pub journal: Journal,
    pub actor: DocActor,
    pub handle: DocHandle,
    pub report: OpenReport,
    pub recovered: Recovered,
    pub clock: Arc<ManualClock>,
}

/// How to open a session.
#[derive(Clone)]
pub struct SessionOptions {
    pub sync: SyncPolicy,
    pub clock: Arc<ManualClock>,
    pub discard: bool,
}

impl Default for SessionOptions {
    fn default() -> SessionOptions {
        SessionOptions {
            sync: SyncPolicy::Manual,
            clock: Arc::new(ManualClock::new(WallTime::UNIX_EPOCH)),
            discard: false,
        }
    }
}

/// Opens `folder`, recovers (or discards) its journal, starts journaling and spawns a
/// deterministic doc actor over the result (the engine's open sequence).
pub fn open_session(folder: &Path, options: &SessionOptions) -> Session {
    let opened = ProjectStore::open(folder).unwrap();
    let mut store = opened.store;
    let recover = RecoverOptions::at(options.clock.now());
    let recovered = if options.discard {
        store.discard_recovery(&opened.project, &recover).unwrap()
    } else {
        store.recover(&opened.project, &recover).unwrap()
    };
    let tracker = recovered.id_tracker(SEED);
    let registry = registry();
    let journal = store
        .start_journal(
            JournalConfig::new(Arc::clone(&options.clock) as Arc<dyn Clock>)
                .with_sync(options.sync)
                .with_registry(Arc::clone(&registry))
                .with_ids(tracker.clone()),
        )
        .unwrap();
    let config = recovered
        .configure(config_for(
            &tracker,
            registry,
            recovered.project.clone(),
            &options.clock,
        ))
        .with_journal(journal.sink());
    let actor = DocActor::spawn(config).unwrap();
    let handle = actor.handle();
    Session {
        store,
        journal,
        actor,
        handle,
        report: opened.report,
        recovered,
        clock: Arc::clone(&options.clock),
    }
}

/// The deterministic config of `tracker` over `doc`, on `clock`.
pub fn config_for(
    tracker: &IdTracker,
    registry: Arc<Registry>,
    doc: Project,
    clock: &Arc<ManualClock>,
) -> DocConfig {
    tracker
        .config(registry, doc)
        .with_clock(Arc::clone(clock) as Arc<dyn Clock>)
}

/// Submits and waits.
pub fn submit(
    h: &DocHandle,
    ctx: &ActorCtx,
    name: &str,
    params: Value,
) -> Result<Applied, CmdError> {
    h.submit(Submit::new(ctx.clone(), name, params)).wait()
}

/// Appends a clip and returns its id.
pub fn append(h: &DocHandle, track: TrackId, frames: u32) -> ItemId {
    let out = submit(
        h,
        &ui(),
        "timeline.append_clip",
        json!({"track": track, "frames": frames}),
    )
    .unwrap()
    .output;
    serde_json::from_value(out["item"].clone()).unwrap()
}

/// Every `.jnl` file under `dir` (recursively), concatenated.
pub fn all_journal_bytes(dir: &Path) -> Vec<u8> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            out.extend(all_journal_bytes(&path));
        } else {
            // The lock file is OS-locked while a session is open.
            if let Ok(bytes) = fs::read(&path) {
                out.extend(bytes);
            }
        }
    }
    out
}

/// Whether `needle` occurs in `haystack`.
pub fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

// ----- the pipelined workload (crash test) ----------------------------------------------------

/// Requests kept in flight by the workload driver (arch exit 3: resend after restart).
pub const DEPTH: usize = 5;

/// SplitMix64: a tiny deterministic generator (no `rand` dependency in tests).
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % n as u64).unwrap()
    }
}

/// One request of a workload op.
#[derive(Clone, Debug)]
pub struct Req {
    pub name: String,
    pub params: Value,
    pub id: RequestId,
}

/// One workload op: a single request, or a batch transaction run synchronously.
#[derive(Clone, Debug)]
pub enum Op {
    Single(Req),
    Batch {
        begin: RequestId,
        steps: Vec<Req>,
        commit: RequestId,
    },
}

impl Op {
    pub fn is_batch(&self) -> bool {
        matches!(self, Op::Batch { .. })
    }
}

/// What an op returned.
#[derive(Clone, Debug)]
pub enum OpResult {
    Single(Result<Applied, CmdError>),
    Batch(Result<Applied, CmdError>),
}

impl OpResult {
    /// The document patch it committed, if any.
    fn committed_ops(&self) -> Option<&cutweave_commands::Patch> {
        match self {
            OpResult::Single(Ok(a)) | OpResult::Batch(Ok(a)) => Some(&a.ops),
            _ => None,
        }
    }
}

/// Generates ops deterministically: op `i` depends only on the seed and the document after op
/// `i - DEPTH` (which is deterministic whatever the timing), so the pipelined child and the
/// in-process reference generate the same ops.
pub struct Workload {
    rng: Rng,
    ids: SeededGen,
    tracks: Vec<TrackId>,
}

impl Workload {
    pub fn new(seed: u64, tracks: Vec<TrackId>) -> Workload {
        Workload {
            rng: Rng::new(seed),
            ids: SeededGen::new(seed ^ 0x00C0_FFEE),
            tracks,
        }
    }

    fn rid(&mut self) -> RequestId {
        RequestId::from_u128(self.ids.next_u128())
    }

    fn req(&mut self, name: &str, params: Value) -> Req {
        Req {
            name: name.to_owned(),
            params,
            id: self.rid(),
        }
    }

    /// Op `i`, generated against `doc` (the document after op `i - DEPTH`).
    pub fn op(&mut self, i: usize, doc: &Project) -> Op {
        let existing = items(doc);
        let track = self.tracks[self.rng.below(self.tracks.len())];
        let frames = 1 + self.rng.below(60);
        let roll = self.rng.below(100);
        let pick =
            |rng: &mut Rng| (!existing.is_empty()).then(|| existing[rng.below(existing.len())]);
        match roll {
            0..=34 => Op::Single(self.req(
                "timeline.append_clip",
                json!({"track": track, "frames": frames}),
            )),
            35..=54 => match pick(&mut self.rng) {
                Some(item) => Op::Single(self.req(
                    "item.rename",
                    json!({"item": item, "name": format!("c{i}")}),
                )),
                None => Op::Single(self.req(
                    "timeline.append_clip",
                    json!({"track": track, "frames": frames}),
                )),
            },
            55..=64 => match pick(&mut self.rng) {
                Some(item) => Op::Single(self.req("item.delete", json!({"item": item}))),
                None => Op::Single(self.req("project.rename", json!({"name": format!("p{i}")}))),
            },
            65..=74 => Op::Single(self.req("project.rename", json!({"name": format!("p{i}")}))),
            75..=84 => {
                let begin = self.rid();
                let mut steps = vec![self.req(
                    "timeline.append_clip",
                    json!({"track": track, "frames": frames}),
                )];
                if let Some(item) = pick(&mut self.rng) {
                    steps.push(self.req(
                        "item.rename",
                        json!({"item": item, "name": format!("b{i}")}),
                    ));
                }
                steps.push(self.req("project.rename", json!({"name": format!("bp{i}")})));
                let commit = self.rid();
                Op::Batch {
                    begin,
                    steps,
                    commit,
                }
            }
            85..=94 => Op::Single(self.req("edit.undo", json!({}))),
            _ => Op::Single(self.req(
                "ai.set_key",
                json!({"provider": format!("prov{i}"), "key": SENTINEL}),
            )),
        }
    }
}

/// Runs a batch op synchronously; the result is the commit's.
pub fn run_batch(
    h: &DocHandle,
    begin: RequestId,
    steps: &[Req],
    commit: RequestId,
) -> Result<Applied, CmdError> {
    let ctx = ui();
    let opened = h
        .submit(
            Submit::new(ctx.clone(), "edit.begin_tx", json!({"label": "batch"}))
                .with_request_id(begin),
        )
        .wait()?;
    let tx: TxId = serde_json::from_value(opened.output["tx"].clone()).unwrap();
    for step in steps {
        // A failing step leaves the batch open; the others still run.
        let _ = h
            .submit(
                Submit::new(ctx.clone(), step.name.clone(), step.params.clone())
                    .with_request_id(step.id)
                    .in_tx(tx),
            )
            .wait();
    }
    h.submit(Submit::new(ctx, "edit.commit_tx", json!({"tx": tx})).with_request_id(commit))
        .wait()
}

/// Submits a single op (not waiting).
pub fn submit_single(h: &DocHandle, req: &Req) -> Pending<Applied> {
    h.submit(Submit::new(ui(), req.name.clone(), req.params.clone()).with_request_id(req.id))
}

/// The reference (uninterrupted) run of a workload.
pub struct Reference {
    pub ops: Vec<Op>,
    pub results: Vec<OpResult>,
    /// The committed document after each op.
    pub docs: Vec<Project>,
    /// The committed revision after each op.
    pub revs: Vec<cutweave_commands::Rev>,
    pub initial: Project,
}

/// Drives `n` ops of the workload over `h` with up to [`DEPTH`] requests in flight, calling
/// `on_done(i, &result)` as each op completes (in order) and `between(i)` after it.
pub fn drive(
    h: &DocHandle,
    seed: u64,
    n: usize,
    mut on_done: impl FnMut(usize, &OpResult),
    mut between: impl FnMut(usize),
) -> Reference {
    let initial = h.committed_snapshot().doc.clone();
    let mut workload = Workload::new(seed, tracks(&initial));
    let mut ops = Vec::with_capacity(n);
    let mut results = Vec::with_capacity(n);
    let mut docs: Vec<Project> = Vec::with_capacity(n);
    let mut revs = Vec::with_capacity(n);
    let mut in_flight: VecDeque<(usize, Pending<Applied>)> = VecDeque::new();
    let mut complete = |i: usize,
                        result: OpResult,
                        results: &mut Vec<OpResult>,
                        docs: &mut Vec<Project>,
                        revs: &mut Vec<cutweave_commands::Rev>| {
        let prev = docs.last().cloned().unwrap_or_else(|| initial.clone());
        let prev_rev = revs.last().copied().unwrap_or(cutweave_commands::Rev::ZERO);
        let (doc, rev) = match result.committed_ops() {
            Some(ops) if !ops.is_empty() => {
                let rev = match &result {
                    OpResult::Single(Ok(a)) | OpResult::Batch(Ok(a)) => a.committed_rev,
                    _ => prev_rev,
                };
                (apply_patch(&prev, ops).unwrap(), rev)
            }
            _ => (prev, prev_rev),
        };
        on_done(i, &result);
        results.push(result);
        docs.push(doc);
        revs.push(rev);
        between(i);
    };
    for i in 0..n {
        // Op i is generated against the document after op i - DEPTH: wait for it.
        while in_flight.len() >= DEPTH || (i >= DEPTH && docs.len() < i + 1 - DEPTH) {
            let (j, pending) = in_flight.pop_front().unwrap();
            complete(
                j,
                OpResult::Single(pending.wait()),
                &mut results,
                &mut docs,
                &mut revs,
            );
        }
        let basis = if i >= DEPTH {
            docs[i - DEPTH].clone()
        } else {
            initial.clone()
        };
        let op = workload.op(i, &basis);
        ops.push(op.clone());
        match &op {
            Op::Single(req) => in_flight.push_back((i, submit_single(h, req))),
            Op::Batch {
                begin,
                steps,
                commit,
            } => {
                while let Some((j, pending)) = in_flight.pop_front() {
                    complete(
                        j,
                        OpResult::Single(pending.wait()),
                        &mut results,
                        &mut docs,
                        &mut revs,
                    );
                }
                let result = run_batch(h, *begin, steps, *commit);
                complete(
                    i,
                    OpResult::Batch(result),
                    &mut results,
                    &mut docs,
                    &mut revs,
                );
            }
        }
    }
    while let Some((j, pending)) = in_flight.pop_front() {
        complete(
            j,
            OpResult::Single(pending.wait()),
            &mut results,
            &mut docs,
            &mut revs,
        );
    }
    Reference {
        ops,
        results,
        docs,
        revs,
        initial,
    }
}
