// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2G.03b acceptance with real files: the FFmpeg encoder (`hw: "off"`), `FakeBackend` clips
//! and the GPU-free probe renderer.
//!
//! - **Marker set.** Four markers export to four files whose frame counts and durations match
//!   the markers' (within one frame), each a clean H.264 + AAC MP4 for `ffprobe`.
//! - **Collisions.** `fail` refuses an existing file and leaves it; `rename` writes a free name
//!   and leaves the old file; `overwrite` replaces the file only when the export succeeds (a
//!   cancelled one leaves the old file and no partial file).
//!
//! Needs the FFmpeg bundle (`. scripts/ffmpeg/env.sh`; `ffprobe` from `$FFMPEG_DIR/bin`); no GPU.
#![cfg(feature = "ffmpeg")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod export_support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration as StdDuration;

use cutweave_commands::CmdError;
use cutweave_engine::export::ExportServices;
use cutweave_engine::{Call, Connection, Engine, EngineConfig, EngineMode, Subsystems};
use cutweave_jobs::JobState;
use cutweave_media::testing::FakeBackend;
use cutweave_media::{FfmpegBackend, MediaBackend, PixelFormat};
use cutweave_model::{FrameRate, SampleRate};
use export_support::{
    ExportOnly, LateSource, ProbeRenderer, TempDir, agent, assert_succeeded, clip, insert, must,
    new_project, register, wait, wait_until,
};
use serde_json::{Value, json};

const WAIT: StdDuration = StdDuration::from_secs(300);

/// A 4 s clip (64x36, 30 fps) at an absolute path inside `dir` (the file need not exist).
fn media(dir: &Path) -> (FakeBackend, String) {
    let path = dir.join("a.mp4").to_string_lossy().into_owned();
    let fake = FakeBackend::new().with_media(
        &path,
        clip(
            4,
            (64, 36),
            FrameRate::FPS_30,
            PixelFormat::Yuv420p,
            440.0,
            SampleRate::HZ_48_000,
        ),
    );
    (fake, path)
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

fn engine_on(dir: &Path, services: ExportServices) -> Engine {
    let mut config = EngineConfig::new(EngineMode::Oneshot);
    config.allowed_roots = vec![dir.to_path_buf()];
    config.subsystems = Some(Box::new(ExportOnly(services)) as Box<dyn Subsystems>);
    Engine::start(config).unwrap()
}

/// A 4 s timeline with markers at 0 s, 1 s, 2.5 s and 3 s (1 s, 1 s, 0.5 s and 1 s long).
fn project(conn: &Connection, fake: &FakeBackend, path: &str) {
    let (seq, video, _) = new_project(conn);
    let id = register(conn, fake, path);
    insert(conn, &seq, &video, id, "0s", "4s", "0s");
    for (t, d, label) in [
        ("0s", "1s", "Intro"),
        ("1s", "1s", "Main: one"),
        ("2.5s", "0.5s", "Main two"),
        ("3s", "1s", ""),
    ] {
        must(
            conn,
            "timeline.add_marker",
            json!({"sequence": seq, "t": t, "duration": d, "label": label}),
        );
    }
}

fn small(path: &Path) -> Value {
    json!({
        "path": path.to_string_lossy(),
        "hw": "off",
        "preset": {"custom": {"width": 64, "height": 36}},
    })
}

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

fn frames_of(path: &Path) -> i64 {
    let p = probe(path);
    assert_eq!(p["streams_stream_0_codec_name"], "h264", "{p:?}");
    assert_eq!(p["streams_stream_1_codec_name"], "aac", "{p:?}");
    p["streams_stream_0_nb_read_frames"].parse().unwrap()
}

fn partials(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".partial"))
        .collect()
}

#[test]
fn four_markers_make_four_files_of_the_markers_lengths() {
    let dir = TempDir::new("ranges-ffmpeg-markers");
    let (fake, path) = media(dir.path());
    let out = dir.path().join("out");
    std::fs::create_dir_all(&out).unwrap();
    let e = engine_on(dir.path(), services(&fake, StdDuration::ZERO));
    let conn = agent(&e);
    project(&conn, &fake, &path);
    let mut p = small(&out.join("{seq}_{marker_label}_{n}.mp4"));
    p["range"] = json!({"kind": "markers"});
    let started = must(&conn, "export.start", p);
    let jobs = started["jobs"].as_array().unwrap().clone();
    assert_eq!(jobs.len(), 4, "{started}");
    for j in &jobs {
        assert_succeeded(&wait(&conn, &j["job_id"], WAIT));
    }
    // 1 s, 1 s, 0.5 s and 1 s at 30 fps, within a frame.
    for (j, want) in jobs.iter().zip([30i64, 30, 15, 30]) {
        let file = PathBuf::from(j["path"].as_str().unwrap());
        let frames = frames_of(&file);
        assert!(
            (frames - want).abs() <= 1,
            "{}: {frames} frames, wanted {want}",
            file.display()
        );
        let seconds: f64 = probe(&file)["format_duration"].parse().unwrap();
        let wanted = want as f64 / 30.0;
        assert!(
            (seconds - wanted).abs() <= 2.0 / 30.0,
            "{}: {seconds} s, wanted {wanted} s",
            file.display()
        );
    }
    let mut names: Vec<String> = std::fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names.len(), 4, "{names:?}");
    assert!(
        names.iter().any(|n| n.ends_with("_Intro_1.mp4")),
        "{names:?}"
    );
    assert!(
        names.iter().any(|n| n.ends_with("_marker_4.mp4")),
        "{names:?}"
    );
    assert_eq!(partials(&out), Vec::<String>::new());
    drop(conn);
    e.shutdown();
}

#[test]
fn each_collision_policy_treats_the_existing_file_as_promised() {
    let dir = TempDir::new("ranges-ffmpeg-collision");
    let (fake, path) = media(dir.path());
    let e = engine_on(dir.path(), services(&fake, StdDuration::from_millis(10)));
    let conn = agent(&e);
    project(&conn, &fake, &path);
    let out = dir.path().join("out.mp4");
    std::fs::write(&out, b"old").unwrap();

    // fail: Invalid at /path, the file stays.
    let mut p = small(&out);
    p["collision"] = json!("fail");
    match conn.call(Call::new("export.start", p)) {
        Err(CmdError::Invalid { path, .. }) => assert_eq!(path, "/path"),
        other => panic!("expected Invalid at /path, got {other:?}"),
    }
    assert_eq!(std::fs::read(&out).unwrap(), b"old");

    // rename: a free name, a clean file, the old one untouched.
    let mut p = small(&out);
    p["collision"] = json!("rename");
    let started = must(&conn, "export.start", p);
    let renamed = dir.path().join("out (1).mp4");
    assert_eq!(started["path"], json!(renamed.to_string_lossy()));
    assert_succeeded(&wait(&conn, &started["job_id"], WAIT));
    assert_eq!(frames_of(&renamed), 120);
    assert_eq!(std::fs::read(&out).unwrap(), b"old");

    // overwrite, cancelled half way: the old file stays and nothing is left behind.
    let mut p = small(&out);
    p["collision"] = json!("overwrite");
    let started = must(&conn, "export.start", p.clone());
    wait_until(&conn, &started["job_id"], WAIT, |j| {
        j.state == JobState::Running && j.progress.fraction >= 0.3
    });
    must(&conn, "export.cancel", json!({ "id": started["job_id"] }));
    assert_eq!(
        wait(&conn, &started["job_id"], WAIT).state,
        JobState::Cancelled
    );
    assert_eq!(
        std::fs::read(&out).unwrap(),
        b"old",
        "a cancelled export replaces nothing"
    );
    assert_eq!(partials(dir.path()), Vec::<String>::new());

    // overwrite that succeeds: the file is replaced by the new one.
    let started = must(&conn, "export.start", p);
    assert_succeeded(&wait(&conn, &started["job_id"], WAIT));
    assert_eq!(frames_of(&out), 120);
    assert_eq!(partials(dir.path()), Vec::<String>::new());
    drop(conn);
    e.shutdown();
}
