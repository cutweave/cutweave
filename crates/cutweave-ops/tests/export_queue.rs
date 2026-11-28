// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2G.03a: the export queue's commands over a real queue and job manager (the export itself
//! replaced by a job that waits for the test): `export.enqueue` waits while paused,
//! `export.start` resumes, `export.queue` lists in order, `export.reorder`, `export.cancel`,
//! `export.retry` and `export.remove` with their errors, and how a job is stored
//! ([`PlanCodec`]: the plan and its document survive a round trip).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

use common::{H, assert_code, assert_invalid, insert_av};
use cutweave_commands::{Access, CmdError, DocSnapshot, Rev};
use cutweave_jobs::queue::{QueueItem, TaskCodec};
use cutweave_jobs::{ExportQueue, JobConfig, JobCtx, JobError, JobId, JobManager, JobState};
use cutweave_ops::export::queue::PlanCodec;
use cutweave_ops::export::{ExportCtx, ExportPlan, ExportStarter};
use serde_json::{Value, json};

const LONG: StdDuration = StdDuration::from_secs(20);

/// A temp folder removed on drop.
struct Dir(PathBuf);

impl Dir {
    fn new() -> Dir {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "cutweave-ops-queue-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Dir(dir)
    }

    fn file(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Which exports may finish: the file names the test released, and the ones that fail.
#[derive(Clone, Default)]
struct Ends(Arc<Mutex<(Vec<String>, Vec<String>)>>);

impl Ends {
    fn finish(&self, name: &str) {
        self.0.lock().unwrap().0.push(name.to_owned());
    }
    fn fail(&self, name: &str) {
        self.0.lock().unwrap().1.push(name.to_owned());
    }
}

/// The engine's starter, in short: the queue, with an "export" that waits for the test.
struct QueueStarter(ExportQueue<ExportPlan>);

impl QueueStarter {
    fn add(&self, plan: ExportPlan, resume: bool) -> Result<JobId, CmdError> {
        let output = Some(plan.path.clone());
        self.0
            .enqueue(
                QueueItem {
                    label: "export".into(),
                    output,
                    task: plan,
                },
                resume,
            )
            .map_err(|e| cutweave_ops::export::queue::queue_error(e, "/path"))
    }
}

impl ExportStarter for QueueStarter {
    fn start(&mut self, plan: ExportPlan) -> Result<JobId, CmdError> {
        self.add(plan, true)
    }
    fn enqueue(&mut self, plan: ExportPlan) -> Result<JobId, CmdError> {
        self.add(plan, false)
    }
    fn queue(&mut self) -> Result<ExportQueue<ExportPlan>, CmdError> {
        Ok(self.0.clone())
    }
}

struct Rig {
    h: H,
    jobs: JobManager,
    queue: ExportQueue<ExportPlan>,
    ends: Ends,
    dir: Dir,
}

impl Rig {
    fn new() -> Rig {
        let (mut h, p) = H::project();
        let a = h.add_video("/media/a.mp4", 3, true);
        insert_av(&mut h, p, a, "0");
        let jobs = JobManager::new(JobConfig {
            export_slots: 1,
            ..JobConfig::default()
        })
        .unwrap();
        let ends = Ends::default();
        let watch = ends.clone();
        let runner = Arc::new(move |plan: &ExportPlan, ctx: &JobCtx| {
            let name = plan
                .path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            loop {
                {
                    let ends = watch.0.lock().unwrap();
                    if ends.1.contains(&name) {
                        return Err(JobError::coded(
                            cutweave_jobs::error_code::DISK_FULL,
                            "the disk is full",
                        ));
                    }
                    if ends.0.contains(&name) {
                        return Ok(());
                    }
                }
                ctx.check()?;
                std::thread::sleep(StdDuration::from_millis(2));
            }
        });
        let queue = ExportQueue::new(jobs.clone(), runner, Arc::new(PlanCodec), 1);
        queue.start();
        Rig {
            h,
            jobs,
            queue,
            ends,
            dir: Dir::new(),
        }
    }

    fn call(&self, name: &str, params: Value) -> Result<Value, CmdError> {
        let mut ctx = ExportCtx {
            snapshot: Arc::new(DocSnapshot {
                rev: Rev(3),
                committed_rev: Rev(3),
                doc: self.h.doc(),
            }),
            allowed_roots: None,
            presets: Arc::default(),
            starter: Box::new(QueueStarter(self.queue.clone())),
        };
        self.h
            .registry
            .invoke_engine_json(name, params, &mut ctx, Access::STANDARD)
    }

    fn must(&self, name: &str, params: Value) -> Value {
        self.call(name, params)
            .unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    /// Queues an export to `name` (small, custom size).
    fn add(&self, command: &str, name: &str) -> Value {
        let out = self.must(
            command,
            json!({ "path": self.dir.file(name), "preset": {"custom": {"width": 64, "height": 36}} }),
        );
        out["job_id"].clone()
    }

    fn wait(&self, id: &Value) -> JobState {
        let id: JobId = serde_json::from_value(id.clone()).unwrap();
        self.jobs.wait(id, LONG).unwrap().state
    }

    fn queue(&self) -> Value {
        self.must("export.queue", json!({}))
    }

    fn states(&self) -> Vec<(String, String)> {
        self.queue()["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|j| {
                let path = j["path"].as_str().unwrap();
                let name = path.rsplit(['/', '\\']).next().unwrap().to_owned();
                (name, j["state"].as_str().unwrap().to_owned())
            })
            .collect()
    }
}

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
        .collect()
}

#[test]
fn enqueue_waits_while_paused_and_start_resumes_the_queue() {
    let r = Rig::new();
    let paused = r.must("export.pause_queue", json!({ "paused": true }));
    assert_eq!(paused["paused"], json!(true));
    assert_eq!(paused["parallel"], json!(1));
    let a = r.add("export.enqueue", "a.mp4");
    let out = r.queue();
    assert_eq!(out["jobs"][0]["id"], a);
    assert_eq!(out["jobs"][0]["state"], json!("queued"));
    assert_eq!(out["jobs"][0]["width"], json!(64));
    assert_eq!(out["jobs"][0]["frames"], json!(90));
    assert_eq!(out["jobs"][0]["rev"], json!(3));
    std::thread::sleep(StdDuration::from_millis(50));
    assert_eq!(r.states(), pairs(&[("a.mp4", "queued")]), "paused");
    // export.start adds and resumes.
    let b = r.add("export.start", "b.mp4");
    assert_eq!(r.queue()["paused"], json!(false));
    r.ends.finish("a.mp4");
    r.ends.finish("b.mp4");
    assert_eq!(r.wait(&a), JobState::Succeeded);
    assert_eq!(r.wait(&b), JobState::Succeeded);
    assert_eq!(
        r.states(),
        pairs(&[("a.mp4", "succeeded"), ("b.mp4", "succeeded")])
    );
}

#[test]
fn reorder_cancel_retry_and_remove_work_by_id_and_say_why_not() {
    let r = Rig::new();
    let a = r.add("export.start", "a.mp4");
    let b = r.add("export.start", "b.mp4");
    let c = r.add("export.start", "c.mp4");
    // c before b; the running one cannot move.
    let out = r.must("export.reorder", json!({ "id": c, "before": b }));
    let order: Vec<&Value> = out["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|j| &j["id"])
        .collect();
    assert_eq!(order, [&a, &c, &b]);
    assert_invalid(r.call("export.reorder", json!({ "id": a })), "/id");
    assert_invalid(
        r.call("export.reorder", json!({ "id": b, "before": b })),
        "/before",
    );
    assert_code(
        r.call(
            "export.reorder",
            json!({ "id": b, "before": JobId::from_u128(99) }),
        ),
        "not_found",
    );
    // A running export cannot be removed; cancelling it lets c start.
    assert_invalid(r.call("export.remove", json!({ "id": a })), "/id");
    let cancelled = r.must("export.cancel", json!({ "id": a }));
    assert_eq!(cancelled["id"], a);
    assert_eq!(r.wait(&a), JobState::Cancelled);
    // c fails: it can be retried, keeping its id.
    r.ends.fail("c.mp4");
    assert_eq!(r.wait(&c), JobState::Failed);
    let job = r.queue()["jobs"][1].clone();
    assert_eq!(job["error"], json!("the disk is full"));
    assert_eq!(job["error_code"], json!("disk_full"));
    r.ends.finish("b.mp4");
    assert_eq!(r.wait(&b), JobState::Succeeded);
    r.ends.0.lock().unwrap().1.clear();
    r.ends.finish("c.mp4");
    let retried = r.must("export.retry", json!({ "id": c }));
    assert_eq!(retried["id"], c);
    assert_eq!(r.wait(&c), JobState::Succeeded);
    assert_invalid(r.call("export.retry", json!({ "id": b })), "/id");
    // Finished ones come off the queue; the files stay where they are.
    let out = r.must("export.remove", json!({ "id": a }));
    assert_eq!(out["jobs"].as_array().unwrap().len(), 2);
    assert_code(r.call("export.cancel", json!({ "id": a })), "not_found");
    assert_eq!(
        r.states(),
        pairs(&[("c.mp4", "succeeded"), ("b.mp4", "succeeded")])
    );
}

#[test]
fn one_file_is_written_by_one_unfinished_export() {
    let r = Rig::new();
    r.must("export.pause_queue", json!({ "paused": true }));
    let a = r.add("export.enqueue", "a.mp4");
    let again = r.call(
        "export.enqueue",
        json!({ "path": r.dir.file("a.mp4"), "preset": {"custom": {"width": 64, "height": 36}} }),
    );
    assert_invalid(again, "/path");
    r.must("export.cancel", json!({ "id": a }));
    assert_eq!(r.wait(&a), JobState::Cancelled);
    r.add("export.enqueue", "a.mp4");
}

#[test]
fn without_a_queue_the_queue_commands_are_unsupported() {
    let (h, _) = H::project();
    let mut ctx = cutweave_ops::testing::export_ctx(Arc::new(DocSnapshot {
        rev: Rev(1),
        committed_rev: Rev(1),
        doc: h.doc(),
    }));
    for (name, params) in [
        ("export.queue", json!({})),
        ("export.pause_queue", json!({ "paused": true })),
        ("export.cancel", json!({ "id": JobId::from_u128(1) })),
    ] {
        let r = h
            .registry
            .invoke_engine_json(name, params, &mut ctx, Access::STANDARD);
        assert_code(r, "unsupported");
    }
}

#[test]
fn a_stored_job_keeps_its_plan_and_document() {
    let r = Rig::new();
    r.must("export.pause_queue", json!({ "paused": true }));
    r.add("export.enqueue", "a.mp4");
    let view = r.queue.view();
    let plan = &view.entries[0].task;
    let bytes = PlanCodec.encode(plan).unwrap();
    let back = PlanCodec.decode(&bytes).unwrap();
    assert_eq!(back.snapshot.doc, plan.snapshot.doc);
    assert_eq!(back.snapshot.rev, plan.snapshot.rev);
    assert_eq!(
        (
            back.sequence,
            back.start,
            back.end,
            back.frames,
            back.duration
        ),
        (
            plan.sequence,
            plan.start,
            plan.end,
            plan.frames,
            plan.duration
        )
    );
    assert_eq!(
        (back.width, back.height, back.frame_rate, back.quality),
        (plan.width, plan.height, plan.frame_rate, plan.quality)
    );
    assert_eq!(
        (back.path.clone(), back.overwrite),
        (plan.path.clone(), plan.overwrite)
    );
    assert_eq!((back.hw.clone(), back.rate), (plan.hw.clone(), plan.rate));
    assert!(PlanCodec.decode(b"{}").is_err());
    let mut v: Value = serde_json::from_slice(&bytes).unwrap();
    v["version"] = json!(99);
    let e = PlanCodec
        .decode(&serde_json::to_vec(&v).unwrap())
        .unwrap_err();
    assert!(e.contains("version 99"), "{e}");
}

#[test]
fn the_property_harness_queue_starts_with_exports_its_strategies_name() {
    let (h, _) = H::project();
    let jobs = cutweave_ops::testing::job_manager();
    let queue = cutweave_ops::testing::export_queue(&jobs);
    let snapshot = Arc::new(DocSnapshot {
        rev: Rev(1),
        committed_rev: Rev(1),
        doc: h.doc(),
    });
    let mut ctx = cutweave_ops::testing::queue_ctx(snapshot, &queue);
    let out = h
        .registry
        .invoke_engine_json("export.queue", json!({}), &mut ctx, Access::STANDARD)
        .unwrap();
    let states: Vec<&str> = out["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|j| j["state"].as_str().unwrap())
        .collect();
    assert_eq!(states, ["queued", "cancelled", "queued"]);
    let mut seeded = cutweave_model::SeededGen::new(cutweave_ops::testing::JOB_ID_SEED);
    let ids: Vec<JobId> = (0..6).map(|_| JobId::mint(&mut seeded)).collect();
    let listed: Vec<Value> = out["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|j| j["id"].clone())
        .collect();
    assert_eq!(listed, [json!(ids[3]), json!(ids[4]), json!(ids[5])]);
    let moved = h
        .registry
        .invoke_engine_json(
            "export.reorder",
            json!({"id": ids[3], "before": null}),
            &mut ctx,
            Access::STANDARD,
        )
        .unwrap();
    assert_eq!(moved["jobs"][2]["id"], json!(ids[3]));
}
