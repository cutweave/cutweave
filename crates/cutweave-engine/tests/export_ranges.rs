// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2G.03b through the engine, without FFmpeg or a GPU (`FakeBackend`, the probe renderer):
//! a marker set queues one export per marker and each encodes its region's frames; the in/out
//! points and custom ranges export the frames they name; `collision: "rename"` steers clear of
//! files that exist and of files another queued export will write; and the disk check reads
//! the free space through the injected `EngineConfig::free_space` (no disk is filled) and
//! refuses an export that does not fit before anything is queued. Real files (ffprobe
//! durations, replacing a file only on success) are in `export_ranges_ffmpeg.rs`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::many_single_char_names
)]

mod export_support;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration as StdDuration;

use cutweave_commands::CmdError;
use cutweave_engine::export::{ExportServices, FreeSpaceProbe};
use cutweave_engine::{Call, Connection, Engine, EngineConfig, EngineMode, Subsystems};
use cutweave_media::PixelFormat;
use cutweave_media::testing::FakeBackend;
use cutweave_model::{FrameRate, SampleRate};
use export_support::{
    ExportOnly, LateSource, ProbeRenderer, TempDir, agent, assert_succeeded, clip,
    export_capabilities, insert, must, new_project, register, services, wait,
};
use serde_json::{Value, json};

const WAIT: StdDuration = StdDuration::from_secs(60);

/// A 4 s clip (64x36, 30 fps) inside `dir`.
fn media(dir: &Path) -> (FakeBackend, String) {
    let path = dir.join("a.mp4").to_string_lossy().into_owned();
    let fake = FakeBackend::new()
        .with_capabilities(export_capabilities())
        .with_media(
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

fn svc(fake: &FakeBackend) -> ExportServices {
    let late = Arc::new(LateSource::default());
    services(
        Arc::new(fake.clone()),
        ProbeRenderer::new(late as _).shared(),
    )
}

fn engine_with(dir: &Path, services: ExportServices, free: Option<FreeSpaceProbe>) -> Engine {
    let mut config = EngineConfig::new(EngineMode::Oneshot);
    config.allowed_roots = vec![dir.to_path_buf()];
    config.subsystems = Some(Box::new(ExportOnly(services)) as Box<dyn Subsystems>);
    config.free_space = free;
    Engine::start(config).unwrap()
}

/// A 4 s timeline with markers at 0 s, 1 s, 2.5 s and 3 s of 1 s, 1 s, 0.5 s and 1 s. Returns
/// the sequence id.
fn project(conn: &Connection, fake: &FakeBackend, path: &str) -> Value {
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
    seq
}

fn small(path: &str) -> Value {
    json!({ "path": path, "preset": {"custom": {"width": 64, "height": 36}} })
}

fn invalid_at(conn: &Connection, name: &str, params: Value) -> String {
    match conn.call(Call::new(name, params)) {
        Err(CmdError::Invalid { path, .. }) => path,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

fn encoded(fake: &FakeBackend) -> Vec<(String, u64)> {
    let mut v: Vec<(String, u64)> = fake
        .encodes()
        .iter()
        .map(|(p, s)| {
            (
                p.file_name().unwrap().to_string_lossy().into_owned(),
                s.video_frames,
            )
        })
        .collect();
    v.sort();
    v
}

#[test]
fn a_marker_set_queues_one_export_per_marker_and_each_encodes_its_region() {
    let dir = TempDir::new("ranges-markers");
    let (fake, path) = media(dir.path());
    let e = engine_with(dir.path(), svc(&fake), None);
    let conn = agent(&e);
    project(&conn, &fake, &path);
    must(&conn, "export.pause_queue", json!({ "paused": true }));
    let out = must(
        &conn,
        "export.enqueue",
        json!({
            "path": dir.file("{n}_{marker_label}.mp4"),
            "range": {"kind": "markers"},
            "preset": {"custom": {"width": 64, "height": 36}},
        }),
    );
    let jobs = out["jobs"].as_array().unwrap().clone();
    assert_eq!(jobs.len(), 4, "{out}");
    let queue = must(&conn, "export.queue", json!({}));
    assert_eq!(queue["jobs"].as_array().unwrap().len(), 4);
    must(&conn, "export.pause_queue", json!({ "paused": false }));
    for j in &jobs {
        assert_succeeded(&wait(&conn, &j["job_id"], WAIT));
    }
    // 30 fps: 1 s, 1 s, 0.5 s, 1 s, each within a frame.
    let got = encoded(&fake);
    let names: Vec<&str> = got.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        [
            "1_Intro.mp4",
            "2_Main_ one.mp4",
            "3_Main two.mp4",
            "4_marker.mp4"
        ]
    );
    for ((_, frames), want) in got.iter().zip([30u64, 30, 15, 30]) {
        assert!(frames.abs_diff(want) <= 1, "{frames} frames, wanted {want}");
    }
    drop(conn);
    e.shutdown();
}

#[test]
fn in_out_and_custom_ranges_export_the_frames_they_name() {
    let dir = TempDir::new("ranges-inout");
    let (fake, path) = media(dir.path());
    let e = engine_with(dir.path(), svc(&fake), None);
    let conn = agent(&e);
    let seq = project(&conn, &fake, &path);
    // No in/out points yet.
    let mut p = small(&dir.file("in.mp4"));
    p["range"] = json!({"kind": "in_out"});
    assert_eq!(invalid_at(&conn, "export.start", p.clone()), "/range/kind");
    must(
        &conn,
        "timeline.set_in_out",
        json!({"sequence": seq, "start": "1s", "end": "2s"}),
    );
    let a = must(&conn, "export.start", p)["job_id"].clone();
    let mut q = small(&dir.file("custom.mp4"));
    q["range"] = json!({"kind": "custom", "in": "2s", "out": "3s"});
    let b = must(&conn, "export.start", q)["job_id"].clone();
    for id in [&a, &b] {
        assert_succeeded(&wait(&conn, id, WAIT));
    }
    assert_eq!(
        encoded(&fake),
        [("custom.mp4".to_owned(), 30), ("in.mp4".to_owned(), 30)]
    );
    drop(conn);
    e.shutdown();
}

#[test]
fn rename_avoids_existing_files_and_files_other_exports_will_write() {
    let dir = TempDir::new("ranges-rename");
    let (fake, path) = media(dir.path());
    let e = engine_with(dir.path(), svc(&fake), None);
    let conn = agent(&e);
    project(&conn, &fake, &path);
    must(&conn, "export.pause_queue", json!({ "paused": true }));
    std::fs::write(dir.file("out.mp4"), b"old").unwrap();
    let path_of = |collision: Option<&str>| -> Value {
        let mut p = small(&dir.file("out.mp4"));
        if let Some(c) = collision {
            p["collision"] = json!(c);
        }
        let r = conn.call(Call::new("export.enqueue", p));
        match r {
            Ok(reply) => reply.output["path"].clone(),
            Err(CmdError::Invalid { path, .. }) => json!(format!("invalid {path}")),
            Err(e) => panic!("{e}"),
        }
    };
    // `out.mp4` exists: the default says so, `rename` goes round it.
    assert_eq!(path_of(None), json!("invalid /overwrite"));
    assert_eq!(path_of(Some("fail")), json!("invalid /path"));
    assert_eq!(path_of(Some("rename")), json!(dir.file("out (1).mp4")));
    // `out (1).mp4` does not exist yet but is queued: the next one takes `(2)`.
    assert_eq!(path_of(Some("rename")), json!(dir.file("out (2).mp4")));
    // Overwriting a file another queued export writes is refused, not queued twice.
    assert_eq!(path_of(Some("overwrite")), json!(dir.file("out.mp4")));
    assert_eq!(path_of(Some("overwrite")), json!("invalid /path"));
    assert_eq!(
        std::fs::read(dir.file("out.mp4")).unwrap(),
        b"old",
        "nothing has run"
    );
    drop(conn);
    e.shutdown();
}

#[test]
fn the_disk_check_uses_the_free_space_it_is_given() {
    let dir = TempDir::new("ranges-disk");
    let (fake, path) = media(dir.path());
    let free = Arc::new(AtomicU64::new(1_000));
    let probe: FreeSpaceProbe = {
        let free = Arc::clone(&free);
        Arc::new(move |_| Some(free.load(Ordering::SeqCst)))
    };
    let e = engine_with(dir.path(), svc(&fake), Some(probe));
    let conn = agent(&e);
    project(&conn, &fake, &path);
    must(&conn, "export.pause_queue", json!({ "paused": true }));
    // 1 KB free: refused, and nothing is queued.
    let mut p = json!({"path": dir.file("big.mp4")});
    assert_eq!(invalid_at(&conn, "export.start", p.clone()), "/path");
    assert_eq!(
        must(&conn, "export.queue", json!({}))["jobs"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    // Room again: it goes in.
    free.store(100_000_000_000, Ordering::SeqCst);
    p["preset"] = json!({"custom": {"width": 64, "height": 36}});
    must(&conn, "export.enqueue", p);
    // A marker set needs the room of all of its files together.
    free.store(5_000, Ordering::SeqCst);
    let set = json!({
        "path": dir.file("{n}.mp4"),
        "range": {"kind": "markers"},
    });
    assert_eq!(invalid_at(&conn, "export.enqueue", set), "/path");
    assert_eq!(
        must(&conn, "export.queue", json!({}))["jobs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    drop(conn);
    e.shutdown();
}
