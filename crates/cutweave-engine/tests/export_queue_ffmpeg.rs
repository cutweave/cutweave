// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2G.03a acceptance with real files: the export queue with the FFmpeg encoder (`hw:
//! "off"`), `FakeBackend` clips and the GPU-free probe renderer (its pictures carry the source
//! frame number, so different timelines encode to different frames):
//!
//! - **Kill harness.** A child process (this test binary re-run as `queue_child_engine`) opens
//!   the project, queues three exports and is killed (`TerminateProcess` / `SIGKILL`) while the
//!   second one runs. An engine restarted on the project reports job 1 `succeeded`, runs job 2
//!   again from the beginning and then job 3; all three files are clean for `ffprobe` and hold
//!   every frame, and no partial file is left.
//! - **Snapshots.** An export queued before an edit encodes the frames of the document it was
//!   queued with: its `framemd5` equals that of an export queued after the edit was undone,
//!   and differs from one queued with the edit.
//! - **Cancel.** Cancelling a running export leaves no file (and no partial file), and the next
//!   export in line runs.
//!
//! Needs the FFmpeg bundle (`. scripts/ffmpeg/env.sh`; `ffmpeg`/`ffprobe` from
//! `$FFMPEG_DIR/bin`); no GPU.
#![cfg(feature = "ffmpeg")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod export_support;

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration as StdDuration;

use cutweave_engine::export::ExportServices;
use cutweave_engine::project_io::ProjectStore;
use cutweave_engine::{Connection, Engine, EngineConfig, EngineMode, Subsystems};
use cutweave_jobs::JobState;
use cutweave_media::testing::FakeBackend;
use cutweave_media::{FfmpegBackend, MediaBackend, PixelFormat};
use cutweave_model::{FrameRate, Project, SampleRate};
use export_support::{
    ExportOnly, LateSource, ProbeRenderer, TempDir, agent, assert_succeeded, clip, insert, job,
    must, new_project, register, wait, wait_until,
};
use serde_json::{Value, json};

const WAIT: StdDuration = StdDuration::from_secs(300);
const CHILD_ENV: &str = "CUTWEAVE_EXPORT_QUEUE_CHILD";
/// Frames of each export: the 4 s timeline at 30 fps.
const FRAMES: u32 = 120;

// ----- the rig ---------------------------------------------------------------------------------

/// Two 3 s clips (64x36, 30 fps) at absolute paths inside `dir` (the files need not exist).
fn media(dir: &Path) -> (FakeBackend, [String; 2]) {
    let file = |n: &str| dir.join(n).to_string_lossy().into_owned();
    let paths = [file("a.mp4"), file("b.mp4")];
    let mut fake = FakeBackend::new();
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

/// `FakeBackend` decoding, FFmpeg encoding (one x264 thread), the probe renderer taking at
/// least `slow` per frame.
fn services(fake: &FakeBackend, slow: StdDuration) -> ExportServices {
    let late = Arc::new(LateSource::default());
    let encoder: Arc<dyn MediaBackend> =
        Arc::new(FfmpegBackend::new().expect("the loaded FFmpeg matches the build"));
    ExportServices::new(
        encoder,
        Arc::new(cutweave_engine::export::DecoderSources::new(Arc::new(
            fake.clone(),
        ))),
        ProbeRenderer::new(late as _).slow(slow).shared(),
    )
    .with_encoder_threads(1)
}

/// A oneshot engine on `project` (or untitled) whose agents may write in `dir`.
fn engine_on(dir: &Path, project: Option<&Path>, services: ExportServices) -> Engine {
    let mut config = EngineConfig::new(EngineMode::Oneshot);
    config.allowed_roots = vec![dir.to_path_buf()];
    config.project = project.map(Path::to_path_buf);
    config.subsystems = Some(Box::new(ExportOnly(services)) as Box<dyn Subsystems>);
    Engine::start(config).unwrap()
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

/// A small software export to `path`.
fn small(path: &Path) -> Value {
    json!({
        "path": path.to_string_lossy(),
        "hw": "off",
        "preset": {"custom": {"width": 64, "height": 36}},
    })
}

// ----- tools -----------------------------------------------------------------------------------

/// `ffprobe` or `ffmpeg` from the bundle the tests link against.
fn tool(name: &str) -> PathBuf {
    if let Ok(dir) = std::env::var("FFMPEG_DIR") {
        let exe = Path::new(&dir)
            .join("bin")
            .join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        if exe.is_file() {
            return exe;
        }
    }
    PathBuf::from(name)
}

/// `ffprobe`'s view of the file (counting its video frames); panics on any complaint.
fn probe(path: &Path) -> BTreeMap<String, String> {
    let out = Command::new(tool("ffprobe"))
        .args([
            "-v",
            "error",
            "-count_frames",
            "-show_format",
            "-show_streams",
            "-of",
            "flat=s=_",
        ])
        .arg(path)
        .output()
        .expect("run ffprobe (source scripts/ffmpeg/env.sh)");
    assert!(
        out.status.success() && out.stderr.is_empty(),
        "ffprobe {}: {}",
        path.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.to_owned(), v.trim_matches('"').to_owned()))
        .collect()
}

/// Asserts `path` is a clean MP4 with all [`FRAMES`] frames of H.264 and an AAC track.
fn assert_valid(path: &Path) {
    let p = probe(path);
    assert_eq!(p["streams_stream_0_codec_name"], "h264", "{p:?}");
    assert_eq!(
        p["streams_stream_0_nb_read_frames"],
        FRAMES.to_string(),
        "{}",
        path.display()
    );
    assert_eq!(p["streams_stream_1_codec_name"], "aac", "{p:?}");
}

/// The `framemd5` lines of the file's video (hashes of the decoded frames).
fn framemd5(path: &Path) -> Vec<String> {
    let out = Command::new(tool("ffmpeg"))
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(path)
        .args(["-map", "0:v", "-f", "framemd5", "-"])
        .output()
        .expect("run ffmpeg");
    assert!(
        out.status.success(),
        "ffmpeg: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

/// Files in `dir` whose name ends with `.partial`.
fn partials(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".partial"))
        .collect()
}

// ----- the kill harness ------------------------------------------------------------------------

/// Child half: opens the project in `CUTWEAVE_EXPORT_QUEUE_CHILD`, queues three exports into
/// its `out` folder, prints their ids, and prints `RUNNING` once the second one is a fifth
/// done; then waits to be killed.
#[test]
fn queue_child_engine() {
    let Ok(folder) = std::env::var(CHILD_ENV) else {
        return; // a normal test run: nothing to do
    };
    let folder = PathBuf::from(folder);
    let dir = folder.parent().unwrap().to_path_buf();
    let (fake, _) = media(&dir);
    let e = engine_on(
        &dir,
        Some(&folder),
        services(&fake, StdDuration::from_millis(15)),
    );
    let conn = agent(&e);
    let out = dir.join("out");
    let ids: Vec<Value> = ["one.mp4", "two.mp4", "three.mp4"]
        .iter()
        .map(|n| must(&conn, "export.start", small(&out.join(n)))["job_id"].clone())
        .collect();
    println!("IDS {}", serde_json::to_string(&ids).unwrap());
    assert_succeeded(&wait(&conn, &ids[0], WAIT));
    wait_until(&conn, &ids[1], WAIT, |j| {
        j.state == JobState::Running && j.progress.fraction >= 0.2
    });
    println!("RUNNING");
    // The parent kills us; never shut down cleanly.
    std::thread::sleep(StdDuration::from_secs(300));
    drop(conn);
    drop(e);
}

#[test]
fn three_queued_exports_survive_an_engine_killed_during_the_second() {
    let dir = TempDir::new("queue-kill");
    let folder = dir.path().join("Kill");
    ProjectStore::create(&folder, &Project::new("Kill"))
        .unwrap()
        .close()
        .unwrap();
    std::fs::create_dir_all(dir.path().join("out")).unwrap();
    let (fake, paths) = media(dir.path());
    {
        let e = engine_on(
            dir.path(),
            Some(&folder),
            services(&fake, StdDuration::ZERO),
        );
        let conn = agent(&e);
        timeline(&conn, &fake, &paths);
        must(&conn, "project.save", json!({}));
        drop(conn);
        e.shutdown();
    }

    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "queue_child_engine",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_ENV, &folder)
        .stdout(Stdio::piped())
        // Shown when this test fails.
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut ids: Vec<Value> = Vec::new();
    let mut running = false;
    let mut said = Vec::new();
    for line in BufReader::new(child.stdout.take().unwrap()).lines() {
        let line = line.unwrap();
        said.push(line.clone());
        // libtest may print the test's name on the same line first.
        if let Some(at) = line.find("IDS ") {
            ids = serde_json::from_str(&line[at + 4..]).unwrap();
        }
        if line.trim() == "RUNNING" {
            running = true;
            break;
        }
    }
    child.kill().unwrap();
    let _ = child.wait();
    assert!(
        running && ids.len() == 3,
        "the child never got to job 2: {said:?}"
    );
    let out = dir.path().join("out");
    assert!(out.join("one.mp4").is_file());
    assert!(!out.join("two.mp4").exists(), "job 2 was cut short");

    // A new engine on the project picks the queue up where the killed one left it.
    let e = engine_on(
        dir.path(),
        Some(&folder),
        services(&fake, StdDuration::ZERO),
    );
    let conn = agent(&e);
    let first = job(&conn, "job.status", json!({ "id": ids[0] }));
    assert_succeeded(&first);
    assert_succeeded(&wait(&conn, &ids[1], WAIT));
    assert_succeeded(&wait(&conn, &ids[2], WAIT));
    let states: Vec<String> = must(&conn, "export.queue", json!({}))["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|j| j["state"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(states, ["succeeded", "succeeded", "succeeded"]);
    for name in ["one.mp4", "two.mp4", "three.mp4"] {
        assert_valid(&out.join(name));
    }
    assert_eq!(partials(&out), Vec::<String>::new(), "no partial file left");
    drop(conn);
    e.shutdown();
}

// ----- snapshots and cancel --------------------------------------------------------------------

#[test]
fn an_export_encodes_the_document_it_was_queued_with() {
    let dir = TempDir::new("queue-snapshot");
    let (fake, paths) = media(dir.path());
    let e = engine_on(dir.path(), None, services(&fake, StdDuration::ZERO));
    let conn = agent(&e);
    let clips = timeline(&conn, &fake, &paths);
    must(&conn, "export.pause_queue", json!({ "paused": true }));
    let before = dir.path().join("before.mp4");
    let edited = dir.path().join("edited.mp4");
    let again = dir.path().join("again.mp4");
    let x = must(&conn, "export.enqueue", small(&before))["job_id"].clone();
    // Clip A goes away: the first two seconds show other frames (B, or nothing).
    let (a_video, a_audio) = clips[0].clone();
    must(&conn, "item.delete", json!({ "items": [a_video, a_audio] }));
    let y = must(&conn, "export.enqueue", small(&edited))["job_id"].clone();
    must(&conn, "edit.undo", json!({}));
    let z = must(&conn, "export.enqueue", small(&again))["job_id"].clone();
    must(&conn, "export.pause_queue", json!({ "paused": false }));
    for id in [&x, &y, &z] {
        assert_succeeded(&wait(&conn, id, WAIT));
    }
    assert_valid(&before);
    assert_valid(&again);
    let (hx, hz, hy) = (framemd5(&before), framemd5(&again), framemd5(&edited));
    assert_eq!(hx.len(), FRAMES as usize);
    assert_eq!(
        hx, hz,
        "the queued export kept the document it was queued with"
    );
    assert_ne!(hy[0], hx[0], "the edit changed the first frames");
    drop(conn);
    e.shutdown();
}

#[test]
fn a_cancelled_export_leaves_no_file_and_the_queue_goes_on() {
    let dir = TempDir::new("queue-cancel");
    let (fake, paths) = media(dir.path());
    let e = engine_on(
        dir.path(),
        None,
        services(&fake, StdDuration::from_millis(10)),
    );
    let conn = agent(&e);
    timeline(&conn, &fake, &paths);
    let out = dir.path().join("out");
    std::fs::create_dir_all(&out).unwrap();
    let a = must(&conn, "export.start", small(&out.join("a.mp4")))["job_id"].clone();
    let b = must(&conn, "export.start", small(&out.join("b.mp4")))["job_id"].clone();
    wait_until(&conn, &a, WAIT, |j| j.progress.fraction >= 0.5);
    must(&conn, "export.cancel", json!({ "id": a }));
    assert_eq!(wait(&conn, &a, WAIT).state, JobState::Cancelled);
    assert_succeeded(&wait(&conn, &b, WAIT));
    let mut names: Vec<String> = std::fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["b.mp4"],
        "no file and no partial file of the cancelled one"
    );
    assert_valid(&out.join("b.mp4"));
    let done = job(&conn, "job.status", json!({ "id": b }));
    assert_eq!(done.progress.frame, Some(u64::from(FRAMES)));
    assert!(done.progress.bytes.is_some_and(|n| n > 0), "{done:?}");
    drop(conn);
    e.shutdown();
}
