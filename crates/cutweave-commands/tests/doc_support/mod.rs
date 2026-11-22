// SPDX-License-Identifier: GPL-3.0-or-later
//! Doc-actor fixtures (WP-1B.03): a project with a few video tracks, commands that edit it, and
//! helpers that drive and check a [`DocActor`].

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_panics_doc
)]

use std::{
    sync::{Arc, Condvar, Mutex},
    time::Duration as StdDuration,
};

use cutweave_commands::{
    ActorCtx, ActorOptions, Applied, CmdError, Command, ConnId, DocActor, DocConfig, DocEvent,
    DocHandle, EngineCommand, EventStream, Patch, Query, QueryCtx, Registry, Submit, Tx,
    apply_patch, apply_patch_json, command, json_eq, query, wire,
};
use cutweave_model::{
    Clip, Duration, EntityKind, FrameRate, IdGen, Item, ItemId, MediaAsset, MediaId, MediaKind,
    ProbeInfo, Project, SampleRate, SeededGen, Sequence, SequenceFormat, Time, Track, TrackId,
    TrackKind,
};
use serde_json::{Value, json};

/// One frame at 30 fps, in flicks.
pub const FRAME: i64 = 23_520_000;

/// The seed of every deterministic actor in these tests.
pub const SEED: u64 = 42;

/// A project with one 30 fps sequence of `tracks` empty video tracks and one 1 h video.
pub fn project(tracks: usize) -> (Project, Vec<TrackId>, MediaId) {
    let mut ids = SeededGen::new(7);
    let mut project = Project::new("Doc actor fixture");
    let probe = ProbeInfo {
        duration: Some(Duration::from_secs(3600).unwrap()),
        ..ProbeInfo::default()
    };
    let media = MediaAsset::file(ids.mint(), MediaKind::Video, "/media/a.mp4", probe);
    let format = SequenceFormat::new(1920, 1080, FrameRate::FPS_30, SampleRate::HZ_48_000);
    let mut sequence = Sequence::new(ids.mint(), "Main", format);
    let mut track_ids = Vec::new();
    for i in 0..tracks {
        let track = Track::new(ids.mint(), TrackKind::Video, format!("V{}", i + 1));
        track_ids.push(track.id);
        sequence.insert_track(i, track);
    }
    let media_id = media.id;
    project.media.insert(media.id, media);
    project.sequences.insert(sequence.id, sequence);
    project.validate().unwrap();
    (project, track_ids, media_id)
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

/// Enables or disables an item.
#[command(
    name = "item.set_enabled",
    summary = "Enable or disable an item",
    undoable,
    mcp = "hidden",
    annotations(destructive = false)
)]
pub struct SetEnabled {
    /// The item.
    pub item: ItemId,
    /// Enabled?
    pub enabled: bool,
}

impl Command for SetEnabled {
    type Output = Done;

    fn apply(&self, tx: &mut Tx<'_>) -> Result<Done, CmdError> {
        tx.update_item(self.item, |item| {
            item.set_enabled(self.enabled);
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

/// Renames the project; the name `fail` fails after renaming (rollback check).
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
        if self.name == "fail" {
            return Err(CmdError::invalid("/name", "refused after renaming"));
        }
        Ok(Done {})
    }
}

/// Stamps the project's name, not undoably (a non-undoable change for the history tests).
#[command(
    name = "project.stamp",
    summary = "Stamp the project name (not undoable)",
    undoable = false,
    mcp = "hidden",
    annotations(destructive = false)
)]
pub struct Stamp {
    /// The new name.
    pub name: String,
}

impl Command for Stamp {
    type Output = Done;

    fn apply(&self, tx: &mut Tx<'_>) -> Result<Done, CmdError> {
        tx.update_meta(|meta| {
            meta.name.clone_from(&self.name);
            Ok(())
        })?;
        Ok(Done {})
    }
}

/// Does nothing (round-trip benchmark).
#[command(
    name = "project.noop",
    summary = "Do nothing",
    undoable,
    mcp = "hidden",
    annotations(destructive = false)
)]
pub struct Noop {}

impl Command for Noop {
    type Output = Done;

    fn apply(&self, _: &mut Tx<'_>) -> Result<Done, CmdError> {
        Ok(Done {})
    }
}

/// Renames the project, then panics.
#[command(
    name = "project.explode",
    summary = "Rename the project, then panic",
    undoable,
    mcp = "hidden",
    annotations(destructive = false)
)]
pub struct Explode {}

impl Command for Explode {
    type Output = Done;

    fn apply(&self, tx: &mut Tx<'_>) -> Result<Done, CmdError> {
        tx.update_meta(|meta| {
            meta.name = "half-applied".into();
            Ok(())
        })?;
        panic!("boom");
    }
}

/// Stores a provider key; panics with the key in the message when the provider is `panic`.
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

    fn apply(&self, _: &mut Tx<'_>) -> Result<Done, CmdError> {
        assert!(self.provider != "panic", "leaking {}", self.key);
        Ok(Done {})
    }
}

/// A gate a command can block on, so a test controls exactly when it finishes.
pub struct Gate {
    state: Mutex<(bool, bool)>,
    cv: Condvar,
}

impl Gate {
    const fn new() -> Gate {
        Gate {
            state: Mutex::new((false, false)),
            cv: Condvar::new(),
        }
    }

    fn enter_and_wait(&self) {
        let mut state = self.state.lock().unwrap();
        state.0 = true;
        self.cv.notify_all();
        while !state.1 {
            state = self.cv.wait(state).unwrap();
        }
    }

    /// Blocks until a command entered the gate.
    pub fn wait_entered(&self) {
        let mut state = self.state.lock().unwrap();
        while !state.0 {
            state = self.cv.wait(state).unwrap();
        }
    }

    /// Lets the command finish.
    pub fn release(&self) {
        self.state.lock().unwrap().1 = true;
        self.cv.notify_all();
    }
}

/// The gate of [`Block`] (one blocking test per process: nextest runs each test alone).
pub static GATE: Gate = Gate::new();

/// Renames the project, then blocks on [`GATE`].
#[command(
    name = "project.block",
    summary = "Rename the project, then wait for the test",
    undoable,
    mcp = "hidden",
    annotations(destructive = false)
)]
pub struct Block {}

impl Command for Block {
    type Output = Done;

    fn apply(&self, tx: &mut Tx<'_>) -> Result<Done, CmdError> {
        tx.update_meta(|meta| {
            meta.name = "blocked".into();
            Ok(())
        })?;
        GATE.enter_and_wait();
        Ok(Done {})
    }
}

/// UI view state stand-in: an engine command whose context is not the document actor's.
#[derive(Debug, Default)]
pub struct ViewState;

/// Moves the playhead (an engine command the doc actor does not run).
#[command(
    name = "view.set_playhead",
    summary = "Move the playhead",
    view,
    mcp = "hidden"
)]
pub struct SetPlayhead {
    /// Where.
    pub at: Time,
}

impl EngineCommand for SetPlayhead {
    type Context = ViewState;
    type Output = Done;

    fn run(&self, _: &mut ViewState) -> Result<Done, CmdError> {
        Ok(Done {})
    }
}

/// The count.
#[wire]
pub struct Count {
    /// How many items.
    pub count: u64,
}

/// Counts the items of the document.
#[query(name = "timeline.count_items", summary = "Count items", mcp = "hidden")]
pub struct CountItems {}

impl Query for CountItems {
    type Output = Count;

    fn run(&self, ctx: &QueryCtx<'_>) -> Result<Count, CmdError> {
        Ok(Count {
            count: ctx.doc().item_count() as u64,
        })
    }
}

/// Every test entry.
pub fn registry() -> Arc<Registry> {
    let mut r = Registry::new();
    r.register::<AppendClip>()
        .unwrap()
        .register::<RenameItem>()
        .unwrap()
        .register::<SetEnabled>()
        .unwrap()
        .register::<DeleteItem>()
        .unwrap()
        .register::<RenameProject>()
        .unwrap()
        .register::<Stamp>()
        .unwrap()
        .register::<Noop>()
        .unwrap()
        .register::<Explode>()
        .unwrap()
        .register::<SetKey>()
        .unwrap()
        .register::<Block>()
        .unwrap()
        .register::<SetPlayhead>()
        .unwrap()
        .register::<CountItems>()
        .unwrap();
    // The production `edit.*` commands (transactions, undo/redo, history, checkpoints).
    cutweave_commands::edit::register(&mut r).unwrap();
    Arc::new(r)
}

/// A deterministic config over `doc`.
pub fn config(doc: Project) -> DocConfig {
    DocConfig::deterministic(registry(), doc, SEED)
}

/// A deterministic actor over `doc`.
pub fn spawn(doc: Project) -> DocActor {
    DocActor::spawn(config(doc)).unwrap()
}

/// A deterministic actor with a custom event capacity.
pub fn spawn_with_capacity(doc: Project, capacity: usize) -> DocActor {
    DocActor::spawn_with(
        config(doc),
        ActorOptions {
            event_capacity: capacity,
            ..ActorOptions::default()
        },
    )
    .unwrap()
}

/// The UI on connection 1.
pub fn ui() -> ActorCtx {
    ActorCtx::ui(ConnId::new(1))
}

/// An agent on connection 2.
pub fn agent() -> ActorCtx {
    ActorCtx::agent(ConnId::new(2), "claude").unwrap()
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
pub fn append(h: &DocHandle, ctx: &ActorCtx, track: TrackId, frames: u32) -> ItemId {
    let out = submit(
        h,
        ctx,
        "timeline.append_clip",
        json!({"track": track, "frames": frames}),
    )
    .unwrap()
    .output;
    serde_json::from_value(out["item"].clone()).unwrap()
}

/// The next `doc.patched` event already queued, skipping other events.
pub fn next_patched(stream: &mut EventStream) -> cutweave_commands::DocPatched {
    loop {
        let event = stream.try_recv().unwrap();
        if let Some(p) = event.as_patched() {
            return p.clone();
        }
    }
}

/// Drains every queued event.
pub fn drain(stream: &mut EventStream) -> Vec<Arc<DocEvent>> {
    let mut out = Vec::new();
    while let Some(event) = stream.recv_timeout(StdDuration::ZERO).unwrap() {
        out.push(event);
    }
    out
}

/// A typed and a JSON mirror of the live document, kept up to date from events only (the UI
/// docStore contract).
pub struct Mirror {
    /// The typed mirror.
    pub doc: Project,
    /// The JSON mirror.
    pub json: Value,
    /// Its revision.
    pub rev: cutweave_commands::Rev,
}

impl Mirror {
    /// A mirror of `doc` at `rev`.
    pub fn new(doc: &Project, rev: cutweave_commands::Rev) -> Mirror {
        Mirror {
            doc: doc.clone(),
            json: serde_json::to_value(doc).unwrap(),
            rev,
        }
    }

    /// Applies one event (panics on a revision gap or a reset).
    pub fn apply(&mut self, event: &DocEvent) {
        match event {
            DocEvent::Patched(p) => {
                assert_eq!(p.prev_rev, self.rev, "revision gap before {}", p.rev);
                assert!(p.rev > p.prev_rev, "revisions must increase");
                self.doc = apply_patch(&self.doc, &p.ops).unwrap();
                apply_patch_json(&mut self.json, &p.ops).unwrap();
                self.rev = p.rev;
            }
            DocEvent::Reset(r) => panic!("unexpected reset at {}", r.rev),
            _ => {}
        }
    }

    /// Asserts the mirror equals `doc`, typed and as JSON.
    pub fn assert_matches(&self, doc: &Project) {
        assert_eq!(&self.doc, doc, "typed mirror");
        assert!(
            json_eq(&self.json, &serde_json::to_value(doc).unwrap()),
            "JSON mirror"
        );
    }
}

/// Every item id of the document, in id order.
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

/// The name of an item (clips only).
pub fn item_name(doc: &Project, item: ItemId) -> Option<String> {
    doc.find_item(&item)?.item.as_clip()?.name.clone()
}

/// Applies `patches` in order to `doc`.
pub fn replay(doc: &Project, patches: &[Patch]) -> Project {
    patches
        .iter()
        .fold(doc.clone(), |d, p| apply_patch(&d, p).unwrap())
}

/// `NotFound` for an item.
pub fn item_not_found(id: ItemId) -> CmdError {
    CmdError::not_found(EntityKind::Item, id)
}

/// Some random-but-fixed id source for tests that need extra ids.
pub fn ids() -> SeededGen {
    SeededGen::new(1234)
}

/// Mints an item id that is not in any fixture.
pub fn stray_item() -> ItemId {
    ids().mint()
}
