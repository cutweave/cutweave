// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1D.07: `media.import` with timed parallel probing, the path allowlist and background
//! analysis jobs; `job.status`, `job.list`, `job.wait`, `job.cancel`; the `media.indexed` and
//! `media.analysis_failed` events (analysis never mutates the document). FFmpeg-free (R27):
//! fake prober and fake analysis stages.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use common::{H, TestAnalysisSink, assert_invalid, id};
use cutweave_commands::{Access, CmdError};
use cutweave_jobs::{JobConfig, JobCtx, JobError, JobId, JobManager};
use cutweave_model::MediaId;
use cutweave_ops::{
    job::{JobWait, MAX_WAIT_MS},
    media::{
        AllowedRoots, AnalysisInput, AnalysisStage, AnalysisStages, Analyzer, FakeProbe,
        ImportPolicy, MediaAnalysisFailed, MediaEvent, MediaIndexed, ProbeError,
    },
};
use serde_json::{Value, json};

const SETTLE: Duration = Duration::from_secs(10);

// ----- fake stages ---------------------------------------------------------------------------

/// Succeeds after reporting half-way progress.
struct Quick {
    name: &'static str,
    requires: &'static [&'static str],
    index: bool,
    ran: Arc<Mutex<Vec<(&'static str, MediaId)>>>,
}

impl AnalysisStage for Quick {
    fn name(&self) -> &'static str {
        self.name
    }
    fn requires(&self) -> &'static [&'static str] {
        self.requires
    }
    fn builds_index(&self) -> bool {
        self.index
    }
    fn run(&self, media: &AnalysisInput, ctx: &JobCtx) -> Result<(), JobError> {
        ctx.progress(0.5, Some("half"));
        ctx.check()?;
        self.ran.lock().unwrap().push((self.name, media.media_id));
        Ok(())
    }
}

/// Fails every asset.
struct Failing;

impl AnalysisStage for Failing {
    fn name(&self) -> &'static str {
        "index"
    }
    fn builds_index(&self) -> bool {
        true
    }
    fn run(&self, _: &AnalysisInput, _: &JobCtx) -> Result<(), JobError> {
        Err(JobError::failed("corrupt GOP at 00:00:03"))
    }
}

/// Runs until cancelled (polling every few milliseconds), signalling when it starts.
struct Long(Mutex<mpsc::Sender<MediaId>>);

impl AnalysisStage for Long {
    fn name(&self) -> &'static str {
        "index"
    }
    fn run(&self, media: &AnalysisInput, ctx: &JobCtx) -> Result<(), JobError> {
        let _ = self.0.lock().unwrap().send(media.media_id);
        ctx.sleep(Duration::from_secs(30))?;
        Err(JobError::failed("never cancelled"))
    }
}

/// Counts how many run at once.
struct Counting {
    now: AtomicUsize,
    max: AtomicUsize,
}

impl AnalysisStage for Counting {
    fn name(&self) -> &'static str {
        "count"
    }
    fn run(&self, _: &AnalysisInput, ctx: &JobCtx) -> Result<(), JobError> {
        let running = self.now.fetch_add(1, Ordering::SeqCst) + 1;
        self.max.fetch_max(running, Ordering::SeqCst);
        let r = ctx.sleep(Duration::from_millis(40));
        self.now.fetch_sub(1, Ordering::SeqCst);
        r
    }
}

// ----- harness -------------------------------------------------------------------------------

struct J {
    h: H,
    jobs: JobManager,
    sink: Arc<TestAnalysisSink>,
    stages: Arc<AnalysisStages>,
}

impl J {
    fn new(stages: Vec<Arc<dyn AnalysisStage>>) -> J {
        J::with_slots(stages, 2)
    }

    fn with_slots(stages: Vec<Arc<dyn AnalysisStage>>, decode_slots: usize) -> J {
        let h = H::empty();
        let sink = Arc::new(TestAnalysisSink::default());
        let mut registered = AnalysisStages::new();
        for stage in stages {
            registered.add(stage).unwrap();
        }
        J {
            h,
            jobs: JobManager::new(JobConfig {
                decode_slots,
                ..JobConfig::default()
            })
            .unwrap(),
            sink,
            stages: Arc::new(registered),
        }
    }

    fn import_with(
        &self,
        prober: FakeProbe,
        policy: ImportPolicy,
        params: Value,
    ) -> Result<Value, CmdError> {
        let analyzer = Analyzer::new(
            self.jobs.clone(),
            Arc::clone(&self.stages),
            Arc::clone(&self.sink) as Arc<dyn cutweave_ops::media::AnalysisSink>,
        );
        let mut ctx = self
            .h
            .import_ctx(prober)
            .with_policy(policy)
            .with_analysis(analyzer);
        self.h
            .registry
            .invoke_engine_json("media.import", params, &mut ctx, Access::STANDARD)
    }

    fn import(&self, prober: FakeProbe, paths: &[&str]) -> Value {
        self.import_with(prober, ImportPolicy::default(), json!({"paths": paths}))
            .unwrap()
    }

    /// Runs a `job.*` command through the registry against the job manager.
    fn job(&self, name: &str, params: Value) -> Result<Value, CmdError> {
        let mut jobs = self.jobs.clone();
        self.h
            .registry
            .invoke_engine_json(name, params, &mut jobs, Access::STANDARD)
    }

    fn wait(&self, job: &Value, timeout_ms: u32) -> Value {
        self.job("job.wait", json!({"job_id": job, "timeout_ms": timeout_ms}))
            .unwrap()
    }
}

fn three() -> FakeProbe {
    FakeProbe::new()
        .with("/m/a.mp4", FakeProbe::video("/m/a.mp4", 10, true))
        .with("/m/b.wav", FakeProbe::audio("/m/b.wav", 4))
        .with("/m/c.png", FakeProbe::image("/m/c.png"))
}

fn quick(
    name: &'static str,
    requires: &'static [&'static str],
    index: bool,
    ran: &Arc<Mutex<Vec<(&'static str, MediaId)>>>,
) -> Arc<dyn AnalysisStage> {
    Arc::new(Quick {
        name,
        requires,
        index,
        ran: Arc::clone(ran),
    })
}

fn job_ids(out: &Value) -> Vec<Value> {
    out["job_ids"].as_array().unwrap().clone()
}

// ----- media.import --------------------------------------------------------------------------

#[test]
fn import_registers_three_files_synchronously_and_analysis_completes_through_job_wait() {
    let ran = Arc::new(Mutex::new(Vec::new()));
    let j = J::new(vec![
        quick("index", &[], true, &ran),
        quick("thumbnails", &["index"], false, &ran),
    ]);
    let t = Instant::now();
    let out = j.import(three(), &["/m/a.mp4", "/m/b.wav", "/m/c.png"]);
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());

    // Registration is synchronous: the ids are in the document right away, with their probe.
    let media_ids: Vec<MediaId> = id(&out["media_ids"]);
    assert_eq!(media_ids.len(), 3);
    assert_eq!(out["failed"], json!([]));
    let doc = j.h.doc();
    let probes = three();
    for (mid, path) in media_ids.iter().zip(["/m/a.mp4", "/m/b.wav", "/m/c.png"]) {
        let asset = &doc.media[mid];
        assert_eq!(asset.path_abs.as_deref(), Some(path));
        let expected = cutweave_ops::media::MediaProbe::probe(&probes, path).unwrap();
        assert_eq!(asset.probe, expected.probe, "{path}: probe stored");
    }
    let list = j.h.query("media.list", json!({})).unwrap();
    assert_eq!(list["total"], 3);
    for m in list["media"].as_array().unwrap() {
        assert!(media_ids.contains(&id(&m["id"])));
    }
    let listed_video = list["media"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["kind"] == "video")
        .unwrap();
    assert_eq!(listed_video["duration"], 10 * 705_600_000_i64);
    assert_eq!(listed_video["has_audio"], true);

    // Two stages per asset; every job finishes and is observable through job.wait.
    let jobs = job_ids(&out);
    assert_eq!(jobs.len(), 6);
    for job in &jobs {
        let done = j.wait(job, MAX_WAIT_MS);
        assert_eq!(done["state"], "succeeded", "{done}");
        assert_eq!(done["kind"], "analysis");
        assert_eq!(done["progress"]["fraction"], 1.0);
        assert!(media_ids.contains(&id(&done["media_id"])));
    }
    // Thumbnails ran after the index of the same asset.
    let ran = ran.lock().unwrap().clone();
    assert_eq!(ran.len(), 6);
    for mid in &media_ids {
        let order: Vec<&str> = ran
            .iter()
            .filter(|(_, m)| m == mid)
            .map(|(n, _)| *n)
            .collect();
        assert_eq!(order, ["index", "thumbnails"]);
    }
    // One `media.indexed` per asset.
    let mut indexed: Vec<MediaId> = j
        .sink
        .events
        .lock()
        .unwrap()
        .iter()
        .map(|e| match e {
            MediaEvent::Indexed(MediaIndexed { media_id }) => *media_id,
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    indexed.sort();
    let mut expected = media_ids.clone();
    expected.sort();
    assert_eq!(indexed, expected);
    assert_eq!(
        j.h.commits(),
        1,
        "one registration transaction; analysis never writes the document"
    );
}

#[test]
fn corrupt_files_are_reported_in_failed_without_an_asset() {
    let j = J::new(Vec::new());
    let prober = three()
        .with_error(
            "/m/broken.mp4",
            ProbeError::Unsupported {
                path: "/m/broken.mp4".into(),
                reason: "moov atom not found".into(),
            },
        )
        .with_error(
            "/m/locked.mov",
            ProbeError::Io {
                path: "/m/locked.mov".into(),
                reason: "permission denied".into(),
            },
        );
    let out = j.import(
        prober,
        &[
            "/m/a.mp4",
            "/m/broken.mp4",
            "/m/missing.mp4",
            "/m/locked.mov",
        ],
    );
    assert_eq!(out["media_ids"].as_array().unwrap().len(), 1);
    assert_eq!(out["media"][0]["path"], "/m/a.mp4");
    let failed = out["failed"].as_array().unwrap();
    let summary: Vec<(&str, &str)> = failed
        .iter()
        .map(|f| (f["path"].as_str().unwrap(), f["code"].as_str().unwrap()))
        .collect();
    assert_eq!(
        summary,
        [
            ("/m/broken.mp4", "unsupported"),
            ("/m/missing.mp4", "not_found"),
            ("/m/locked.mov", "io"),
        ]
    );
    assert!(
        failed[0]["error"]
            .as_str()
            .unwrap()
            .contains("moov atom not found")
    );
    assert_eq!(j.h.doc().media.len(), 1);
    assert_eq!(out["job_ids"], json!([]), "no stages installed");
}

#[test]
fn unregistrable_probe_results_are_reported_as_invalid() {
    let j = J::new(Vec::new());
    let mut silent = FakeProbe::video("/m/silent.bin", 3, false);
    silent.probe.streams.clear();
    let out = j.import(
        three().with("/m/silent.bin", silent),
        &["/m/silent.bin", "/m/b.wav"],
    );
    assert_eq!(out["failed"][0]["code"], "invalid");
    assert_eq!(out["failed"][0]["path"], "/m/silent.bin");
    assert_eq!(out["media_ids"].as_array().unwrap().len(), 1);
}

#[test]
fn a_probe_over_the_timeout_is_reported_within_timeout_plus_100_ms() {
    let j = J::new(Vec::new());
    let timeout = Duration::from_millis(300);
    let prober = three().with_delay("/m/a.mp4", Duration::from_secs(5));
    let policy = ImportPolicy {
        probe_timeout: timeout,
        ..ImportPolicy::default()
    };
    let t = Instant::now();
    let out = j
        .import_with(
            prober,
            policy,
            json!({"paths": ["/m/a.mp4", "/m/b.wav", "/m/c.png"]}),
        )
        .unwrap();
    let took = t.elapsed();
    assert!(took < timeout + Duration::from_millis(100), "{took:?}");
    assert_eq!(out["failed"][0]["path"], "/m/a.mp4");
    assert_eq!(out["failed"][0]["code"], "timed_out");
    assert_eq!(
        out["media_ids"].as_array().unwrap().len(),
        2,
        "the others imported"
    );
    assert_eq!(j.h.doc().media.len(), 2);
}

#[test]
fn probes_run_at_most_four_at_a_time() {
    let j = J::new(Vec::new());
    let mut prober = FakeProbe::new();
    let paths: Vec<String> = (0..8).map(|i| format!("/m/{i}.wav")).collect();
    for p in &paths {
        prober = prober
            .with(p, FakeProbe::audio(p, 2))
            .with_delay(p, Duration::from_millis(150));
    }
    let t = Instant::now();
    let out = j
        .import_with(prober, ImportPolicy::default(), json!({"paths": paths}))
        .unwrap();
    let took = t.elapsed();
    assert_eq!(out["media_ids"].as_array().unwrap().len(), 8);
    // 8 files of 150 ms, 4 at a time: two rounds (not one, not eight).
    assert!(took >= Duration::from_millis(300), "{took:?}");
    assert!(took < Duration::from_millis(1200), "{took:?}");
}

#[test]
fn a_path_outside_the_allowed_roots_is_invalid_in_restricted_mode() {
    let j = J::new(Vec::new());
    let policy = ImportPolicy::restricted(AllowedRoots::new(["/m"]));
    let prober = three().with("/etc/passwd.wav", FakeProbe::audio("/etc/passwd.wav", 1));
    assert_invalid(
        j.import_with(
            prober.clone(),
            policy.clone(),
            json!({"paths": ["/m/a.mp4", "/etc/passwd.wav"]}),
        ),
        "/paths/1",
    );
    assert_invalid(
        j.import_with(
            prober.clone(),
            policy.clone(),
            json!({"paths": ["/m/../etc/passwd.wav"]}),
        ),
        "/paths/0",
    );
    assert_eq!(j.h.doc().media.len(), 0, "nothing was probed or imported");
    // Inside the roots it imports; the editor UI (no allowlist) may import anything.
    let ok = j
        .import_with(prober.clone(), policy, json!({"paths": ["/m/a.mp4"]}))
        .unwrap();
    assert_eq!(ok["media_ids"].as_array().unwrap().len(), 1);
    let ui = j
        .import_with(
            prober,
            ImportPolicy::default(),
            json!({"paths": ["/etc/passwd.wav"]}),
        )
        .unwrap();
    assert_eq!(ui["media_ids"].as_array().unwrap().len(), 1);
}

#[test]
fn a_sink_error_points_at_the_requested_path_not_the_batch_entry() {
    /// Refuses the batch, naming its first entry (as `register_batch` would).
    struct Refuses;
    impl cutweave_ops::media::MediaSink for Refuses {
        fn register(
            &mut self,
            _: Vec<cutweave_ops::media::MediaRegister>,
        ) -> Result<Vec<cutweave_ops::media::MediaRegistered>, CmdError> {
            Err(CmdError::invalid("/paths/0/content_hash", "refused"))
        }
    }
    let h = H::empty();
    let mut ctx = cutweave_ops::media::ImportCtx::new(Arc::new(three()), Box::new(Refuses));
    // `/m/missing.mp4` fails to probe, so the batch's first entry is request path 1.
    let result = h.registry.invoke_engine_json(
        "media.import",
        json!({"paths": ["/m/missing.mp4", "/m/a.mp4"]}),
        &mut ctx,
        Access::STANDARD,
    );
    assert_invalid(result, "/paths/1/content_hash");
}

#[test]
fn duplicates_queue_no_new_analysis() {
    let ran = Arc::new(Mutex::new(Vec::new()));
    let j = J::new(vec![quick("index", &[], true, &ran)]);
    let first = j.import(three(), &["/m/a.mp4"]);
    assert_eq!(job_ids(&first).len(), 1);
    let again = j.import(
        three().with("/m/copy.mp4", FakeProbe::video("/m/a.mp4", 10, true)),
        &["/m/copy.mp4"],
    );
    assert_eq!(again["media"][0]["duplicate"], true);
    assert_eq!(again["media_ids"], first["media_ids"]);
    assert_eq!(again["job_ids"], json!([]));
}

#[test]
fn stages_that_do_not_apply_are_skipped() {
    struct AudioOnly;
    impl AnalysisStage for AudioOnly {
        fn name(&self) -> &'static str {
            "waveform"
        }
        fn applies_to(&self, media: &AnalysisInput) -> bool {
            media.probe.audio.is_some()
        }
        fn run(&self, _: &AnalysisInput, _: &JobCtx) -> Result<(), JobError> {
            Ok(())
        }
    }
    let j = J::new(vec![Arc::new(AudioOnly)]);
    let out = j.import(three(), &["/m/a.mp4", "/m/b.wav", "/m/c.png"]);
    assert_eq!(job_ids(&out).len(), 2, "no waveform for the still image");
}

// ----- analysis failures ---------------------------------------------------------------------

#[test]
fn a_failing_stage_is_a_failed_job_and_an_event_never_a_document_change() {
    let ran = Arc::new(Mutex::new(Vec::new()));
    let j = J::new(vec![
        Arc::new(Failing),
        quick("thumbnails", &["index"], false, &ran),
    ]);
    let out = j.import(three(), &["/m/a.mp4"]);
    let mid: MediaId = id(&out["media_ids"][0]);
    let doc_after_import = j.h.doc();
    let jobs = job_ids(&out);
    let index = j.wait(&jobs[0], MAX_WAIT_MS);
    assert_eq!(index["state"], "failed");
    assert_eq!(index["label"], "index");
    assert_eq!(index["error"], "corrupt GOP at 00:00:03");
    let thumbs = j.wait(&jobs[1], MAX_WAIT_MS);
    assert_eq!(thumbs["state"], "failed");
    assert!(thumbs["error"].as_str().unwrap().starts_with("skipped"));
    assert!(ran.lock().unwrap().is_empty());

    // One `media.analysis_failed`, for the stage that failed (not for the skipped one), and
    // no `media.indexed`.
    assert_eq!(
        *j.sink.events.lock().unwrap(),
        [MediaEvent::AnalysisFailed(MediaAnalysisFailed {
            media_id: mid,
            stage: "index".into(),
            job_id: id(&jobs[0]),
            error: "corrupt GOP at 00:00:03".into(),
        })]
    );
    // The failure is visible as job state for the asset...
    let on_asset = j
        .job("job.list", json!({"media_id": mid, "state": "failed"}))
        .unwrap();
    let labels: Vec<&str> = on_asset["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|job| job["label"].as_str().unwrap())
        .collect();
    assert_eq!(labels, ["thumbnails", "index"]);
    // ...and the document is untouched: the import stays the only change (undo stays clean).
    assert_eq!(j.h.doc(), doc_after_import);
    assert_eq!(j.h.commits(), 1);
    let asset = serde_json::to_value(&j.h.doc().media[&mid]).unwrap();
    assert!(asset.get("analysis_error").is_none());
}

#[test]
fn a_panicking_stage_fails_its_job_with_an_event() {
    struct Panics;
    impl AnalysisStage for Panics {
        fn name(&self) -> &'static str {
            "waveform"
        }
        fn run(&self, _: &AnalysisInput, _: &JobCtx) -> Result<(), JobError> {
            panic!("decoder exploded")
        }
    }
    let j = J::new(vec![Arc::new(Panics)]);
    let out = j.import(three(), &["/m/b.wav"]);
    let job = job_ids(&out)[0].clone();
    let done = j.wait(&job, MAX_WAIT_MS);
    assert_eq!(done["state"], "failed");
    assert_eq!(done["error"], "the stage crashed");
    match j.sink.events.lock().unwrap().as_slice() {
        [MediaEvent::AnalysisFailed(e)] => {
            assert_eq!(e.stage, "waveform");
            assert_eq!(json!(e.job_id), job);
        }
        other => panic!("expected one media.analysis_failed, got {other:?}"),
    }
}

// ----- job.* ---------------------------------------------------------------------------------

#[test]
fn job_wait_times_out_on_a_running_job_and_returns_at_once_when_finished() {
    let (tx, started) = mpsc::channel();
    let j = J::new(vec![Arc::new(Long(Mutex::new(tx)))]);
    let out = j.import(three(), &["/m/a.mp4"]);
    let job = job_ids(&out)[0].clone();
    started.recv_timeout(SETTLE).unwrap();

    let t = Instant::now();
    let info = j.wait(&job, 100);
    let waited = t.elapsed();
    assert_eq!(info["state"], "running");
    assert_eq!(info["progress"]["fraction"], 0.0);
    assert!(waited >= Duration::from_millis(100), "{waited:?}");
    assert!(waited < Duration::from_secs(1), "{waited:?}");

    j.job("job.cancel", json!({"id": job})).unwrap();
    assert_eq!(j.wait(&job, MAX_WAIT_MS)["state"], "cancelled");
    let t = Instant::now();
    assert_eq!(j.wait(&job, MAX_WAIT_MS)["state"], "cancelled");
    assert!(
        t.elapsed() < Duration::from_millis(100),
        "{:?}",
        t.elapsed()
    );
}

#[test]
fn job_cancel_stops_a_running_analysis_within_500_ms() {
    let (tx, started) = mpsc::channel();
    let j = J::new(vec![Arc::new(Long(Mutex::new(tx)))]);
    let out = j.import(three(), &["/m/a.mp4"]);
    let job = job_ids(&out)[0].clone();
    started.recv_timeout(SETTLE).unwrap();
    let t = Instant::now();
    let info = j.job("job.cancel", json!({"id": job})).unwrap();
    assert_eq!(info["state"], "running");
    assert_eq!(info["cancel_requested"], true);
    let done = j.wait(&job, 2_000);
    let took = t.elapsed();
    assert_eq!(done["state"], "cancelled");
    assert!(took < Duration::from_millis(500), "{took:?}");
    // A cancelled stage is not reported as a failure.
    assert!(j.sink.events.lock().unwrap().is_empty());
    // Cancelling a finished job changes nothing.
    assert_eq!(j.job("job.cancel", json!({"id": job})).unwrap(), done);
}

#[test]
fn the_decode_budget_limits_concurrent_analysis() {
    let counting = Arc::new(Counting {
        now: AtomicUsize::new(0),
        max: AtomicUsize::new(0),
    });
    let j = J::with_slots(vec![Arc::clone(&counting) as Arc<dyn AnalysisStage>], 2);
    let mut prober = FakeProbe::new();
    let paths: Vec<String> = (0..6).map(|i| format!("/m/{i}.wav")).collect();
    for p in &paths {
        prober = prober.with(p, FakeProbe::audio(p, 2));
    }
    let out = j
        .import_with(prober, ImportPolicy::default(), json!({"paths": paths}))
        .unwrap();
    let jobs = job_ids(&out);
    assert_eq!(jobs.len(), 6);
    for job in &jobs {
        assert_eq!(j.wait(job, MAX_WAIT_MS)["state"], "succeeded");
    }
    assert_eq!(counting.max.load(Ordering::SeqCst), 2);
}

#[test]
fn job_status_and_list_report_jobs_newest_first() {
    let ran = Arc::new(Mutex::new(Vec::new()));
    let j = J::new(vec![quick("index", &[], true, &ran)]);
    let out = j.import(three(), &["/m/a.mp4", "/m/b.wav"]);
    let jobs = job_ids(&out);
    for job in &jobs {
        j.wait(job, MAX_WAIT_MS);
    }
    let status = j.job("job.status", json!({"id": jobs[0]})).unwrap();
    assert_eq!(status["id"], jobs[0]);
    assert_eq!(status["label"], "index");
    assert_eq!(status["state"], "succeeded");
    assert_eq!(status["media_id"], out["media_ids"][0]);

    let list = j.job("job.list", json!({})).unwrap();
    assert_eq!(list["active"], 0);
    let listed: Vec<Value> = list["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|job| job["id"].clone())
        .collect();
    assert_eq!(listed, [jobs[1].clone(), jobs[0].clone()]);
    let one = j
        .job(
            "job.list",
            json!({"media_id": out["media_ids"][1], "state": "succeeded", "kind": "analysis"}),
        )
        .unwrap();
    assert_eq!(one["jobs"].as_array().unwrap().len(), 1);
    assert_eq!(one["jobs"][0]["id"], jobs[1]);
    let none = j.job("job.list", json!({"state": "running"})).unwrap();
    assert_eq!(none["jobs"], json!([]));
    assert_invalid(j.job("job.list", json!({"limit": 0})), "/limit");
    assert_invalid(j.job("job.list", json!({"limit": 501})), "/limit");
    assert_invalid(j.job("job.list", json!({"state": "done"})), "/state");
}

#[test]
fn unknown_jobs_are_not_found_and_bad_ids_invalid() {
    let j = J::new(Vec::new());
    let unknown = JobId::from_u128(42);
    for (name, params) in [
        ("job.status", json!({"id": unknown})),
        ("job.cancel", json!({"id": unknown})),
        ("job.wait", json!({"job_id": unknown, "timeout_ms": 0})),
    ] {
        match j.job(name, params) {
            Err(CmdError::NotFound { kind, id }) => {
                assert_eq!(kind, cutweave_commands::ResourceKind::Job, "{name}");
                assert_eq!(id, unknown.to_string());
            }
            other => panic!("{name}: expected NotFound, got {other:?}"),
        }
    }
    assert_invalid(j.job("job.status", json!({"id": "med_1"})), "/id");
    assert_invalid(j.job("job.wait", json!({"id": unknown})), "/id");
}

#[test]
fn job_wait_timeouts_default_and_cap() {
    let wait = |ms: Option<u32>| JobWait {
        job_id: JobId::from_u128(1),
        timeout_ms: ms,
    };
    assert_eq!(wait(None).timeout(), Duration::from_secs(30));
    assert_eq!(wait(Some(100)).timeout(), Duration::from_millis(100));
    assert_eq!(wait(Some(600_000)).timeout(), Duration::from_secs(60));
    assert_eq!(wait(Some(0)).timeout(), Duration::ZERO);
}
