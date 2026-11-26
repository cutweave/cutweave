// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2G.03a through the engine, without FFmpeg or a GPU (`FakeBackend` decoding and encoding,
//! the probe renderer): exports queue and run one at a time in order while editing goes on
//! (command round trips stay under 16 ms), `export.enqueue` waits while the queue is paused and
//! `export.start` resumes it, a cancel lets the next export start, progress carries frames,
//! speed, time left and bytes, the export pacer is asked before every frame, and the queue
//! lives in the project: it survives an engine restart (a running export starts again), stays
//! behind when another project is opened and moves along when an untitled document is saved.
//! Real files (ffprobe, frame hashes, a killed engine) are in `export_queue_ffmpeg.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod export_support;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration as StdDuration, Instant};

use cutweave_engine::export::{ExportPacer, ExportServices};
use cutweave_engine::project_io::ProjectStore;
use cutweave_engine::{Connection, Engine, EngineConfig, EngineMode, Subsystems};
use cutweave_jobs::{CancelToken, JobState};
use cutweave_media::PixelFormat;
use cutweave_media::testing::FakeBackend;
use cutweave_model::{FrameRate, Project, SampleRate};
use export_support::{
    ExportOnly, Gate, LateSource, ProbeRenderer, TempDir, agent, assert_succeeded, clip,
    export_capabilities, insert, job, must, new_project, register, services, wait, wait_until,
};
use serde_json::{Value, json};

const WAIT: StdDuration = StdDuration::from_secs(60);

/// Two 3 s clips (64x36, 30 fps) at absolute paths inside `dir`.
fn media(dir: &Path) -> (FakeBackend, [String; 2]) {
    let file = |n: &str| dir.join(n).to_string_lossy().into_owned();
    let paths = [file("a.mp4"), file("b.mp4")];
    let mut fake = FakeBackend::new().with_capabilities(export_capabilities());
    for (path, hz) in paths.iter().zip([440.0, 660.0]) {
        fake = fake.with_media(
            path,
            clip(
                3,
                (64, 36),
                FrameRate::FPS_30,
                PixelFormat::Yuv420p,
                hz,
                SampleRate::HZ_48_000,
            ),
        );
    }
    (fake, paths)
}

/// A 4 s timeline: A [0, 2 s), B [2 s, 4 s). Returns the clips `(video, audio)`.
fn timeline(conn: &Connection, fake: &FakeBackend, paths: &[String; 2]) -> Vec<(Value, Value)> {
    let (seq, video, _) = new_project(conn);
    let ids: Vec<_> = paths.iter().map(|p| register(conn, fake, p)).collect();
    [
        insert(conn, &seq, &video, ids[0], "0s", "2s", "0s"),
        insert(conn, &seq, &video, ids[1], "2s", "2s", "0s"),
    ]
    .iter()
    .map(|o| (o["clips"][0].clone(), o["clips"][1].clone()))
    .collect()
}

/// The small export the tests queue.
fn small(path: &str) -> Value {
    json!({ "path": path, "preset": {"custom": {"width": 64, "height": 36}} })
}

/// Export services over `fake` with a probe renderer.
fn svc(fake: &FakeBackend, gate: Option<&Gate>, slow: StdDuration) -> ExportServices {
    let late = Arc::new(LateSource::default());
    let mut renderer = ProbeRenderer::new(late as _).slow(slow);
    if let Some(gate) = gate {
        renderer = renderer.gated(gate.clone());
    }
    services(Arc::new(fake.clone()), renderer.shared())
}

/// A oneshot engine on `project` (a folder) or untitled, whose agents may write in `dir`.
fn engine_on(dir: &Path, project: Option<&Path>, services: ExportServices) -> Engine {
    let mut config = EngineConfig::new(EngineMode::Oneshot);
    config.allowed_roots = vec![dir.to_path_buf()];
    config.project = project.map(Path::to_path_buf);
    config.subsystems = Some(Box::new(ExportOnly(services)) as Box<dyn Subsystems>);
    Engine::start(config).unwrap()
}

/// An empty project folder `name` in `dir`.
fn project_folder(dir: &Path, name: &str) -> PathBuf {
    let folder = dir.join(name);
    ProjectStore::create(&folder, &Project::new(name))
        .unwrap()
        .close()
        .unwrap();
    folder
}

fn queue_states(conn: &Connection) -> Vec<(String, String)> {
    must(conn, "export.queue", json!({}))["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|j| {
            let path = j["path"].as_str().unwrap();
            let name = Path::new(path).file_name().unwrap().to_string_lossy();
            (name.into_owned(), j["state"].as_str().unwrap().to_owned())
        })
        .collect()
}

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
        .collect()
}

fn encoded_names(fake: &FakeBackend) -> Vec<String> {
    fake.encodes()
        .iter()
        .map(|(p, _)| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

/// p95 of `samples`.
fn p95(mut samples: Vec<StdDuration>) -> StdDuration {
    samples.sort();
    samples[(samples.len() * 95 / 100).min(samples.len() - 1)]
}

#[test]
fn exports_run_one_at_a_time_in_order_while_editing_goes_on() {
    let dir = TempDir::new("queue-order");
    let (fake, paths) = media(dir.path());
    let gate = Gate::closed();
    let e = engine_on(
        dir.path(),
        None,
        svc(&fake, Some(&gate), StdDuration::from_millis(2)),
    );
    let conn = agent(&e);
    let clips = timeline(&conn, &fake, &paths);
    let ids: Vec<Value> = ["one.mp4", "two.mp4", "three.mp4"]
        .iter()
        .map(|n| must(&conn, "export.start", small(&dir.file(n)))["job_id"].clone())
        .collect();
    wait_until(&conn, &ids[0], WAIT, |j| j.state == JobState::Running);
    assert_eq!(
        queue_states(&conn),
        pairs(&[
            ("one.mp4", "running"),
            ("two.mp4", "queued"),
            ("three.mp4", "queued")
        ])
    );
    // Editing goes on while the first export waits on the GPU: round trips stay short, and
    // the exports keep the document they were queued with.
    let mut rtt = Vec::new();
    for i in 0..200 {
        let asked = Instant::now();
        must(
            &conn,
            "project.rename",
            json!({ "name": format!("Edit {i}") }),
        );
        rtt.push(asked.elapsed());
    }
    let (video, audio) = clips[1].clone();
    must(&conn, "item.delete", json!({ "items": [video, audio] }));
    let p95 = p95(rtt);
    println!("editing round trip p95 during an export: {p95:?}");
    assert!(
        p95 < StdDuration::from_millis(16),
        "editing round trip p95 {p95:?} during an export"
    );
    gate.open();
    for id in &ids {
        assert_succeeded(&wait(&conn, id, WAIT));
    }
    assert_eq!(encoded_names(&fake), ["one.mp4", "two.mp4", "three.mp4"]);
    for (_, summary) in fake.encodes() {
        assert_eq!(summary.video_frames, 120, "the 4 s it was queued with");
    }
    drop(conn);
    e.shutdown();
}

#[test]
fn editing_round_trips_stay_under_16_ms_while_an_export_renders() {
    let dir = TempDir::new("queue-rtt");
    let (fake, paths) = media(dir.path());
    // Frames keep coming (5 ms each), so the export is busy for the whole measurement.
    let e = engine_on(
        dir.path(),
        None,
        svc(&fake, None, StdDuration::from_millis(5)),
    );
    let conn = agent(&e);
    let clips = timeline(&conn, &fake, &paths);
    let id = must(&conn, "export.start", small(&dir.file("busy.mp4")))["job_id"].clone();
    wait_until(&conn, &id, WAIT, |j| j.progress.fraction > 0.02);
    let (video, _) = clips[0].clone();
    let mut rtt = Vec::new();
    for i in 0..300 {
        let asked = Instant::now();
        must(
            &conn,
            "item.set_props",
            json!({ "item": video, "props": { "opacity": 0.5 + f64::from(i % 50) / 100.0 } }),
        );
        rtt.push(asked.elapsed());
    }
    let running = job(&conn, "job.status", json!({ "id": id }));
    assert_eq!(
        running.state,
        JobState::Running,
        "measured during the export"
    );
    let p95 = p95(rtt);
    println!("item.set_props round trip p95 during an export: {p95:?}");
    assert!(p95 < StdDuration::from_millis(16), "p95 {p95:?}");
    assert_succeeded(&wait(&conn, &id, WAIT));
    drop(conn);
    e.shutdown();
}

#[test]
fn enqueue_waits_while_paused_start_resumes_and_a_cancel_lets_the_next_one_run() {
    let dir = TempDir::new("queue-pause");
    let (fake, paths) = media(dir.path());
    let e = engine_on(
        dir.path(),
        None,
        svc(&fake, None, StdDuration::from_millis(5)),
    );
    let conn = agent(&e);
    timeline(&conn, &fake, &paths);
    let paused = must(&conn, "export.pause_queue", json!({ "paused": true }));
    assert_eq!(paused["paused"], json!(true));
    let a = must(&conn, "export.enqueue", small(&dir.file("a-out.mp4")))["job_id"].clone();
    std::thread::sleep(StdDuration::from_millis(100));
    assert_eq!(
        job(&conn, "job.status", json!({ "id": a })).state,
        JobState::Queued
    );
    assert_eq!(fake.encodes().len(), 0, "nothing runs while paused");
    // export.start adds and resumes; the first one goes first.
    let b = must(&conn, "export.start", small(&dir.file("b-out.mp4")))["job_id"].clone();
    let half = wait_until(&conn, &a, WAIT, |j| j.progress.fraction >= 0.3);
    // Progress carries the counters the queue panel shows.
    let p = &half.progress;
    assert_eq!(p.frames, Some(120));
    assert!(p.frame.is_some_and(|f| f > 0 && f < 120), "{p:?}");
    assert!(p.fps.is_some_and(|f| f > 0.0), "{p:?}");
    assert!(p.eta_ms.is_some() && p.bytes.is_some(), "{p:?}");
    let cancelled = must(&conn, "export.cancel", json!({ "id": a }));
    assert_eq!(cancelled["id"], a);
    assert_eq!(wait(&conn, &a, WAIT).state, JobState::Cancelled);
    let done = wait(&conn, &b, WAIT);
    assert_succeeded(&done);
    assert_eq!(done.progress.frame, Some(120));
    assert_eq!(encoded_names(&fake), ["b-out.mp4"]);
    // The cancelled one can be retried with its id; finished ones come off the queue.
    let retried = must(&conn, "export.retry", json!({ "id": a }));
    assert_eq!(retried["id"], a);
    assert_succeeded(&wait(&conn, &a, WAIT));
    must(&conn, "export.remove", json!({ "id": b }));
    assert_eq!(queue_states(&conn), pairs(&[("a-out.mp4", "succeeded")]));
    drop(conn);
    e.shutdown();
}

/// Counts the frames it paced.
#[derive(Default)]
struct CountingPacer(AtomicU32);

impl ExportPacer for CountingPacer {
    fn before_frame(&self, _cancel: &CancelToken) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn the_pacer_is_asked_before_every_frame() {
    let dir = TempDir::new("queue-pacer");
    let (fake, paths) = media(dir.path());
    let pacer = Arc::new(CountingPacer::default());
    let services = svc(&fake, None, StdDuration::ZERO).with_pacer(Arc::clone(&pacer) as _);
    let e = engine_on(dir.path(), None, services);
    let conn = agent(&e);
    timeline(&conn, &fake, &paths);
    let id = must(&conn, "export.start", small(&dir.file("paced.mp4")))["job_id"].clone();
    assert_succeeded(&wait(&conn, &id, WAIT));
    assert_eq!(pacer.0.load(Ordering::Relaxed), 120);
    drop(conn);
    e.shutdown();
}

#[test]
fn the_queue_survives_a_restart_and_a_running_export_starts_again() {
    let dir = TempDir::new("queue-restart");
    let folder = project_folder(dir.path(), "Restart");
    let (fake, paths) = media(dir.path());
    let gate = Gate::closed();
    let (first, second);
    {
        let e = engine_on(
            dir.path(),
            Some(&folder),
            svc(&fake, Some(&gate), StdDuration::ZERO),
        );
        let conn = agent(&e);
        timeline(&conn, &fake, &paths);
        must(&conn, "project.save", json!({}));
        first = must(&conn, "export.start", small(&dir.file("first.mp4")))["job_id"].clone();
        second = must(&conn, "export.start", small(&dir.file("second.mp4")))["job_id"].clone();
        wait_until(&conn, &first, WAIT, |j| j.state == JobState::Running);
        // The engine stops while the first export waits on the renderer.
        drop(conn);
        e.shutdown();
    }
    assert!(folder.join(".cutweave").join("exports.sqlite").is_file());
    assert_eq!(fake.encodes().len(), 0);
    gate.open();
    let e = engine_on(
        dir.path(),
        Some(&folder),
        svc(&fake, None, StdDuration::ZERO),
    );
    let conn = agent(&e);
    let listed = must(&conn, "export.queue", json!({}));
    assert_eq!(listed["jobs"][0]["id"], first);
    assert_eq!(listed["jobs"][1]["id"], second);
    assert_succeeded(&wait(&conn, &first, WAIT));
    assert_succeeded(&wait(&conn, &second, WAIT));
    assert_eq!(encoded_names(&fake), ["first.mp4", "second.mp4"]);
    assert_eq!(
        queue_states(&conn),
        pairs(&[("first.mp4", "succeeded"), ("second.mp4", "succeeded")])
    );
    drop(conn);
    e.shutdown();
    // And once more: finished exports are remembered, nothing runs again.
    let e = engine_on(
        dir.path(),
        Some(&folder),
        svc(&fake, None, StdDuration::ZERO),
    );
    let conn = agent(&e);
    assert_succeeded(&job(&conn, "job.status", json!({ "id": first })));
    std::thread::sleep(StdDuration::from_millis(100));
    assert_eq!(fake.encodes().len(), 2);
    drop(conn);
    e.shutdown();
}

#[test]
fn another_project_suspends_the_queue_and_reopening_resumes_it() {
    let dir = TempDir::new("queue-switch");
    let one = project_folder(dir.path(), "One");
    let two = project_folder(dir.path(), "Two");
    let (fake, paths) = media(dir.path());
    let gate = Gate::closed();
    let e = engine_on(
        dir.path(),
        Some(&one),
        svc(&fake, Some(&gate), StdDuration::ZERO),
    );
    let conn = agent(&e);
    timeline(&conn, &fake, &paths);
    must(&conn, "project.save", json!({}));
    let id = must(&conn, "export.start", small(&dir.file("one.mp4")))["job_id"].clone();
    wait_until(&conn, &id, WAIT, |j| j.state == JobState::Running);
    must(
        &conn,
        "project.open",
        json!({ "path": two.to_string_lossy() }),
    );
    assert_eq!(queue_states(&conn), Vec::<(String, String)>::new());
    // It was asked to stop; it does once the renderer lets go of its frame.
    assert!(job(&conn, "job.status", json!({ "id": id })).cancel_requested);
    gate.open();
    assert_eq!(
        wait(&conn, &id, WAIT).state,
        JobState::Cancelled,
        "it stopped"
    );
    must(
        &conn,
        "project.open",
        json!({ "path": one.to_string_lossy() }),
    );
    assert_eq!(queue_states(&conn).len(), 1);
    assert_succeeded(&wait(&conn, &id, WAIT));
    assert_eq!(encoded_names(&fake), ["one.mp4"]);
    drop(conn);
    e.shutdown();
}

#[test]
fn an_untitled_documents_queue_moves_into_the_project_it_is_saved_as() {
    let dir = TempDir::new("queue-save-as");
    let (fake, paths) = media(dir.path());
    let e = engine_on(dir.path(), None, svc(&fake, None, StdDuration::ZERO));
    let conn = agent(&e);
    timeline(&conn, &fake, &paths);
    must(&conn, "export.pause_queue", json!({ "paused": true }));
    let id = must(&conn, "export.enqueue", small(&dir.file("later.mp4")))["job_id"].clone();
    let folder = dir.path().join("Saved");
    must(
        &conn,
        "project.save_as",
        json!({ "path": folder.to_string_lossy() }),
    );
    let listed = must(&conn, "export.queue", json!({}));
    assert_eq!(listed["paused"], json!(true));
    assert_eq!(listed["jobs"][0]["id"], id);
    assert!(folder.join(".cutweave").join("exports.sqlite").is_file());
    must(&conn, "export.pause_queue", json!({ "paused": false }));
    assert_succeeded(&wait(&conn, &id, WAIT));
    drop(conn);
    e.shutdown();
}

#[test]
fn a_shared_output_path_is_refused_while_an_export_writes_it() {
    let dir = TempDir::new("queue-paths");
    let (fake, paths) = media(dir.path());
    let e = engine_on(dir.path(), None, svc(&fake, None, StdDuration::ZERO));
    let conn = agent(&e);
    timeline(&conn, &fake, &paths);
    must(&conn, "export.pause_queue", json!({ "paused": true }));
    let out = dir.file("same.mp4");
    must(&conn, "export.enqueue", small(&out));
    match conn.call(cutweave_engine::Call::new("export.enqueue", small(&out))) {
        Err(cutweave_commands::CmdError::Invalid { path, msg }) => {
            assert_eq!(path, "/path");
            assert!(msg.contains("already writing"), "{msg}");
        }
        other => panic!("expected Invalid at /path, got {other:?}"),
    }
    drop(conn);
    e.shutdown();
}
