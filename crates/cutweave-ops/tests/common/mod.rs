// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared harness of the per-command tests (WP-1B.05/06): runs registered commands in a
//! transaction on a shared document and, for every success, asserts the undo/redo identity and
//! document validity; for every failure, that nothing changed. The registry-driven property
//! harness (WP-1B.07) is `registry_props.rs`, built on `cutweave_ops::testing`.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};

use cutweave_commands::{
    Access, CmdError, Committed, QueryCtx, Registry, Transaction, apply_patch,
};
use cutweave_model::{GroupId, ItemId, MediaId, Project, SeededGen, SequenceId, Time, TrackId};
use cutweave_ops::media::{
    AnalysisSink, FakeProbe, ImportCtx, MediaEvent, MediaRegister, MediaRegistered, MediaSink,
    register_batch,
};
use serde_json::{Value, json};

struct Shared {
    doc: Project,
    ids: SeededGen,
    /// Number of commits, for "nothing changed" checks.
    commits: usize,
}

/// A registry with every op, and a document it edits.
pub struct H {
    pub registry: Arc<Registry>,
    shared: Arc<Mutex<Shared>>,
    pub last: Option<Committed>,
}

pub fn secs(s: i64) -> Time {
    Time::from_secs(s).unwrap()
}

impl H {
    /// An empty project.
    pub fn empty() -> H {
        let mut registry = Registry::new();
        cutweave_ops::register(&mut registry).unwrap();
        H {
            registry: Arc::new(registry),
            shared: Arc::new(Mutex::new(Shared {
                doc: Project::new("Empty"),
                ids: SeededGen::new(42),
                commits: 0,
            })),
            last: None,
        }
    }

    /// A harness over an existing document.
    pub fn with_doc(doc: Project) -> H {
        let h = H::empty();
        h.shared.lock().unwrap().doc = doc;
        h
    }

    /// Replaces the document (built by hand, e.g. with attachments no command makes yet),
    /// keeping the ID generator, so new IDs never collide with the ones already issued.
    pub fn replace_doc(&mut self, doc: Project) {
        self.shared.lock().unwrap().doc = doc;
    }

    /// `project.new` applied: one sequence with V1 and A1.
    pub fn project() -> (H, P) {
        let mut h = H::empty();
        let out = h.run("project.new", json!({"name": "Test"})).unwrap();
        let p = P {
            sequence: id(&out["sequence"]),
            video: id(&out["video_track"]),
            audio: id(&out["audio_track"]),
        };
        (h, p)
    }

    pub fn doc(&self) -> Project {
        self.shared.lock().unwrap().doc.clone()
    }

    pub fn commits(&self) -> usize {
        self.shared.lock().unwrap().commits
    }

    /// Runs `name` in its own transaction. On success commits and asserts the undo/redo
    /// identity and `Project::validate`; on failure asserts that nothing changed.
    pub fn run(&mut self, name: &str, params: Value) -> Result<Value, CmdError> {
        let mut s = self.shared.lock().unwrap();
        let base = s.doc.clone();
        let Shared { doc, ids, commits } = &mut *s;
        let mut txn = Transaction::begin(base.clone());
        let result = {
            let mut tx = txn.tx(ids, Access::STANDARD);
            self.registry.invoke_json(name, params, &mut tx)
        };
        match result {
            Ok(out) => {
                let committed = txn.commit();
                assert_eq!(
                    apply_patch(&committed.doc, &committed.inverse).unwrap(),
                    base,
                    "{name}: undo must restore the document"
                );
                assert_eq!(
                    apply_patch(&base, &committed.patch).unwrap(),
                    committed.doc,
                    "{name}: redo must reproduce the document"
                );
                assert_eq!(
                    apply_patch(
                        &apply_patch(&committed.doc, &committed.inverse).unwrap(),
                        &committed.patch
                    )
                    .unwrap(),
                    committed.doc,
                    "{name}: redo after undo"
                );
                if let Err(e) = committed.doc.validate() {
                    panic!("{name}: the document is invalid after the command: {e}");
                }
                *doc = committed.doc.clone();
                *commits += 1;
                self.last = Some(committed);
                Ok(out)
            }
            Err(e) => {
                assert_eq!(
                    txn.doc(),
                    &base,
                    "{name}: a failed command must change nothing"
                );
                assert!(
                    txn.is_empty(),
                    "{name}: a failed command must record nothing"
                );
                Err(e)
            }
        }
    }

    /// Like `run`, but the call must succeed.
    pub fn ok(&mut self, name: &str, params: Value) -> Value {
        self.run(name, params)
            .unwrap_or_else(|e| panic!("{name} failed: {e}"))
    }

    pub fn query(&self, name: &str, params: Value) -> Result<Value, CmdError> {
        let doc = self.doc();
        self.registry
            .query_json(name, params, &QueryCtx::new(&doc, Access::STANDARD))
    }

    /// Whether a dry run (`validate_json`) accepts the call.
    pub fn validate(&self, name: &str, params: Value) -> Result<(), CmdError> {
        let doc = self.doc();
        self.registry
            .validate_json(name, params, &QueryCtx::new(&doc, Access::STANDARD))
    }

    /// A sink that registers imports into this harness's document in one transaction.
    pub fn import_ctx(&self, prober: FakeProbe) -> ImportCtx {
        ImportCtx::new(Arc::new(prober), Box::new(HSink(self.shared.clone())))
    }

    /// Imports a fake video (`secs` seconds, optional audio) and returns its media ID.
    pub fn add_video(&mut self, path: &str, secs: i64, audio: bool) -> MediaId {
        let p = FakeProbe::video(path, secs, audio);
        self.register(path, &p)
    }

    pub fn add_audio(&mut self, path: &str, secs: i64) -> MediaId {
        self.register(path, &FakeProbe::audio(path, secs))
    }

    pub fn add_image(&mut self, path: &str) -> MediaId {
        self.register(path, &FakeProbe::image(path))
    }

    fn register(&mut self, path: &str, p: &cutweave_ops::media::Probed) -> MediaId {
        let out = self.ok(
            "media.register",
            json!({"path": path, "probe": p.probe, "content_hash": p.content_hash}),
        );
        id(&out["media_id"])
    }
}

struct HSink(Arc<Mutex<Shared>>);

impl MediaSink for HSink {
    fn register(&mut self, batch: Vec<MediaRegister>) -> Result<Vec<MediaRegistered>, CmdError> {
        let mut s = self.0.lock().unwrap();
        let Shared { doc, ids, commits } = &mut *s;
        let mut txn = Transaction::begin(doc.clone());
        let out = register_batch(&mut txn.tx(ids, Access::STANDARD), &batch)?;
        let committed = txn.commit();
        *doc = committed.doc;
        *commits += 1;
        Ok(out)
    }
}

/// The engine side of analysis jobs: their event fan-out (WP-1D.07). Records the events.
#[derive(Default)]
pub struct TestAnalysisSink {
    /// Published events, in order.
    pub events: Mutex<Vec<MediaEvent>>,
}

impl AnalysisSink for TestAnalysisSink {
    fn publish(&self, event: MediaEvent) {
        self.events.lock().unwrap().push(event);
    }
}

/// IDs of the project `H::project` creates.
#[derive(Clone, Copy, Debug)]
pub struct P {
    pub sequence: SequenceId,
    pub video: TrackId,
    pub audio: TrackId,
}

pub fn id<T: serde::de::DeserializeOwned>(v: &Value) -> T {
    serde_json::from_value(v.clone()).unwrap()
}

pub fn item_id(v: &Value) -> ItemId {
    id(v)
}

pub fn group_id(v: &Value) -> Option<GroupId> {
    id(v)
}

/// Asserts the error is `Invalid` at exactly `path`.
#[track_caller]
pub fn assert_invalid(r: Result<Value, CmdError>, path: &str) {
    match r {
        Err(CmdError::Invalid { path: p, msg }) => {
            assert_eq!(p, path, "wrong path (msg: {msg})");
            assert_ne!(msg, "");
        }
        other => panic!("expected Invalid at `{path}`, got {other:?}"),
    }
}

#[track_caller]
pub fn assert_code(r: Result<Value, CmdError>, code: &str) {
    match r {
        Err(e) => assert_eq!(e.code(), code, "{e}"),
        Ok(v) => panic!("expected `{code}`, got Ok({v})"),
    }
}

// ----- helpers of the WP-1B.06 edit tests ----------------------------------------------------

/// The clip `item` as stored in the document.
pub fn clip_of(h: &H, item: ItemId) -> cutweave_model::Clip {
    h.doc()
        .find_item(&item)
        .unwrap_or_else(|| panic!("{item} is not in the document"))
        .item
        .as_clip()
        .unwrap()
        .clone()
}

/// The track an item is on.
pub fn track_of(h: &H, item: ItemId) -> TrackId {
    h.doc().find_item(&item).unwrap().track.id
}

/// A video with sound inserted at `at` of the video track: `(video clip, audio clip)`.
pub fn insert_av(h: &mut H, p: P, media: MediaId, at: &str) -> (ItemId, ItemId) {
    let out = h.ok(
        "timeline.insert",
        json!({"sequence": p.sequence, "track": p.video, "media": media, "at": at}),
    );
    (item_id(&out["clips"][0]), item_id(&out["clips"][1]))
}

/// One frame at 30 fps in flicks.
pub const FRAME30: i64 = 705_600_000 / 30;
