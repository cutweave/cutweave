// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1I.05: the full engine's subsystems wired into the core, without FFmpeg (media from
//! `FakeBackend`; the corpus run with the real backend is `full_engine_ffmpeg.rs`).
//!
//! - Acceptance 3: a forced audio-device failure leaves a working engine on the null clock and
//!   sends `engine.degraded`.
//! - `render.frame` renders on the engine's renderer; imports probe with the media backend and
//!   queue the analysis stages.
//! - Playback on a manual clock: frames are rendered and delivered as the clock advances; pause,
//!   step and seek keep the view's playhead in step; preview sizes apply.
//! - `Gui` mode announces the preview ring in `rpc.hello` and the run-file and removes it at
//!   shutdown; project swaps move playback to the new document without leaking threads.
//! - Deterministic ids hold end to end (import → insert → export twice, same documents and jobs).
//! - WP-1H.04 (ADR-0022): a media item plays on its own (`view.set_source`) frame by frame on
//!   the manual clock, leaves the project, its history, journal and saved document untouched and
//!   the timeline's playhead where it was; removing the asset or opening another project returns
//!   to the timeline; an audio-only asset plays its sound through the mixer.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod engine_support;
mod full_support;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cutweave_api::{Credential, Token};
use cutweave_engine::{AudioMode, Connection, Engine, EngineMode, FullConfig, FullSubsystems};
use cutweave_media::Capabilities;
use cutweave_media::testing::{FakeAudio, FakeBackend, FakeMedia, FakeVideo};
use cutweave_model::{Duration, FrameRate, SampleRate, StreamKind};
use cutweave_playback::shm::{FramePublisher, PixelFormat, RingConfig, RingName};
use engine_support::{TempDir, settle_threads, thread_count};
use full_support::{
    Clocks, SEC, Spy, call, degraded, delivered, drain, engine_config, eventually, full_config,
    play_frames,
};
use serde_json::{Value, json};

/// A 3 s 30 fps 320×180 clip with a stereo tone, as a file on disk (its bytes are only hashed)
/// and in `FakeBackend`.
fn fake_clip(dir: &Path, name: &str, fake: FakeBackend) -> (FakeBackend, PathBuf) {
    let path = dir.join(name);
    std::fs::write(&path, format!("fake media {name}")).unwrap();
    let d = Duration::from_secs(3).unwrap();
    let media = FakeMedia {
        video: Some(FakeVideo::lasting(320, 180, FrameRate::FPS_30, d)),
        audio: Some(FakeAudio::tone(SampleRate::HZ_48_000, 2, d)),
    };
    (fake.with_media(&path, media), path)
}

/// A `secs` s clip at `fps` with a picture (`video`) and a stereo tone (`audio`), as a file on
/// disk (its bytes are only hashed) and in `FakeBackend`.
fn fake_media(
    dir: &Path,
    name: &str,
    fake: FakeBackend,
    (secs, fps): (i64, FrameRate),
    (video, audio): (bool, bool),
) -> (FakeBackend, PathBuf) {
    let path = dir.join(name);
    std::fs::write(&path, format!("fake media {name}")).unwrap();
    let d = Duration::from_secs(secs).unwrap();
    let media = FakeMedia {
        video: video.then(|| FakeVideo::lasting(320, 180, fps, d)),
        audio: audio.then(|| FakeAudio::tone(SampleRate::HZ_48_000, 2, d)),
    };
    (fake.with_media(&path, media), path)
}

/// Imports `path` and waits for its analysis jobs; returns the media id.
fn import_one(conn: &Connection, path: &Path) -> Value {
    let imported = call(
        conn,
        "media.import",
        json!({ "paths": [path.to_string_lossy()] }),
    )
    .unwrap();
    assert_eq!(imported["failed"], json!([]), "{imported}");
    for job in imported["job_ids"].as_array().unwrap() {
        call(
            conn,
            "job.wait",
            json!({ "job_id": job, "timeout_ms": 30_000 }),
        )
        .unwrap();
    }
    imported["media_ids"][0].clone()
}

/// Every file under `dir` with its bytes, by relative path.
fn tree_bytes(dir: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    let mut out = std::collections::BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let rel = path
                    .strip_prefix(dir)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                out.insert(rel, std::fs::read(&path).unwrap());
            }
        }
    }
    out
}

/// `FakeBackend` codecs plus the export's `h264` and `aac` encoders.
fn with_encoders(fake: FakeBackend) -> FakeBackend {
    let codec = |name: &str, kind| cutweave_media::CodecSupport {
        name: name.to_owned(),
        kind,
        decoders: vec![name.to_owned()],
        encoders: vec![name.to_owned()],
    };
    fake.with_capabilities(Capabilities::with_codecs([
        codec("rawvideo", StreamKind::Video),
        codec("pcm_f32le", StreamKind::Audio),
        codec("h264", StreamKind::Video),
        codec("aac", StreamKind::Audio),
    ]))
}

fn agent(engine: &Engine) -> Connection {
    engine
        .handle()
        .connect(&Credential::Agent("cli".into()))
        .unwrap()
}

/// `project.new`, then `media.import` of `clip` and `timeline.insert` of 2 s of it at 0:
/// `(media id, item id, import output)`.
fn build_timeline(conn: &Connection, clip: &Path) -> (Value, Value, Value) {
    let project = call(conn, "project.new", json!({ "name": "Full" })).unwrap();
    let imported = call(
        conn,
        "media.import",
        json!({ "paths": [clip.to_string_lossy()] }),
    )
    .unwrap();
    assert_eq!(imported["failed"], json!([]), "{imported}");
    let media = imported["media_ids"][0].clone();
    let inserted = call(
        conn,
        "timeline.insert",
        json!({
            "sequence": project["sequence"], "track": project["video_track"],
            "media": media, "at": 0, "duration": "2s",
        }),
    )
    .unwrap();
    let item = inserted["clips"][0].clone();
    (media, item, imported)
}

/// Acceptance 3: the audio device fails to start; the engine works on the null clock and
/// sends `engine.degraded{subsystem: "audio"}`.
#[test]
fn a_failing_audio_device_degrades_to_the_null_clock() {
    let t = TempDir::new("degraded");
    let (fake, clip) = fake_clip(t.path(), "a.mp4", FakeBackend::new());
    let config = full_config(
        t.path(),
        AudioMode::FailDevice("simulated: the device is gone".into()),
    )
    .with_media(Arc::new(fake));
    let (spy, handle, early) = Spy::new(config);
    let mut engine_config = engine_config(t.path(), EngineMode::Headless, Box::new(spy));
    engine_config.audio = cutweave_engine::AudioConfig::Default;
    let engine = Engine::start(engine_config).unwrap();

    let mut rx = early.lock().unwrap().take().unwrap();
    let reports = degraded(&drain(&mut rx));
    assert_eq!(reports.len(), 1, "{reports:?}");
    assert_eq!(reports[0]["subsystem"], "audio");
    assert!(
        reports[0]["reason"]
            .as_str()
            .unwrap()
            .contains("null clock"),
        "{reports:?}"
    );
    assert_eq!(handle.degraded()[0].0, "audio");

    // The engine works: edits, a frame, and playback that advances on the null clock.
    let conn = agent(&engine);
    build_timeline(&conn, &clip);
    let frame = call(&conn, "render.frame", json!({ "t": "1s", "format": "png" })).unwrap();
    assert_eq!(frame["mime"], "image/png");
    let state = call(&conn, "view.play", json!({})).unwrap();
    assert_eq!(state["playing"], json!(true));
    let moved = eventually("the null clock to advance", || {
        let t = handle.playback_state()?.t.flicks();
        (t > SEC / 10).then_some(t)
    });
    assert!(moved > 0);
    let paused = call(&conn, "view.pause", json!({})).unwrap();
    assert_eq!(paused["playing"], json!(false));
    drop(conn);
    engine.shutdown();
}

/// `render.frame` runs on the engine's renderer (GPU, or the CPU reference without one) with
/// pictures decoded through the decoder pools; imports probe with the backend and hash the
/// content, and queue the analysis stages.
#[test]
fn render_frame_and_imports_use_the_full_engine() {
    let t = TempDir::new("render");
    let (fake, clip) = fake_clip(t.path(), "a.mp4", FakeBackend::new());
    let config = full_config(t.path(), AudioMode::FromEngine).with_media(Arc::new(fake));
    let full = FullSubsystems::new(config);
    let handle = full.handle();
    let engine = Engine::start(engine_config(
        t.path(),
        EngineMode::Headless,
        Box::new(full),
    ))
    .unwrap();
    let conn = agent(&engine);
    let (_, item, imported) = build_timeline(&conn, &clip);
    let snapshot = engine.handle().snapshot().unwrap();
    let asset = snapshot.doc.media.values().next().unwrap();
    assert!(
        asset
            .content_hash
            .as_deref()
            .is_some_and(|h| h.starts_with("blake3-fast:")),
        "{asset:?}"
    );
    // index → thumbnails, and the waveform: one job each.
    let jobs = imported["job_ids"].as_array().unwrap().clone();
    assert_eq!(jobs.len(), 3, "{imported}");
    for job in &jobs {
        let info = call(
            &conn,
            "job.wait",
            json!({ "job_id": job, "timeout_ms": 30_000 }),
        )
        .unwrap();
        assert_eq!(info["state"], "succeeded", "{info}");
    }

    let frame = call(
        &conn,
        "render.frame",
        json!({ "t": "1s", "max_size": 256, "format": "png" }),
    )
    .unwrap();
    assert_eq!(frame["mime"], "image/png");
    assert_eq!(
        (frame["width"].clone(), frame["height"].clone()),
        (json!(256), json!(144))
    );
    assert_eq!(frame["items_visible"][0]["item"], item, "{frame}");
    assert!(
        frame["notes"].as_array().unwrap().is_empty(),
        "{}",
        frame["notes"]
    );
    assert!(handle.decoder_pools() >= 1);
    // `media.capabilities` answers from the backend (the export dialog's encoder list).
    let caps = call(&conn, "media.capabilities", json!({})).unwrap();
    assert_eq!(caps["available"], json!(true), "{caps}");
    assert!(
        caps["codecs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["name"] == "rawvideo"),
        "{caps}"
    );
    assert_eq!(caps["hw_encode"], json!([]));
    // The media HTTP state (WP-1I.03) resolves the asset.
    let state = engine.handle().media_state().unwrap();
    let media_id = serde_json::from_value(imported["media_ids"][0].clone()).unwrap();
    let file = state.media(media_id).unwrap();
    assert_eq!(file.path, clip);
    assert!(file.hash.is_some());
    drop(conn);
    engine.shutdown();
}

/// Acceptance 2: cold start to ready (subsystems up, server bound, run-file written) with an
/// empty project, as the app runs it: `Gui` mode, the best GPU, the default audio output, the
/// preview ring, FFmpeg when built with it. Budget 1.5 s (arch §19: 2.5 s for the whole app).
///
/// A measurement for an idle machine (the GPU driver dominates it: about a second here), not a
/// gate: next to the whole workspace's tests the driver alone takes longer. Run it alone:
/// `cargo nextest run -p cutweave-engine --features ffmpeg --run-ignored only -E 'test(cold_start)'`.
#[test]
#[ignore = "timing on an idle machine; run alone (see the doc comment)"]
fn cold_start_of_the_full_gui_engine_is_under_a_second_and_a_half() {
    cold_start(EngineMode::Gui);
}

/// The same for a headless server (null audio, no ring).
#[test]
fn cold_start_of_the_full_headless_engine_is_under_a_second_and_a_half() {
    cold_start(EngineMode::Headless);
}

fn cold_start(mode: EngineMode) {
    let t = TempDir::new("cold");
    // Best of up to three starts (`best_within`): contention from the machine's other work only
    // adds to the time, a real regression adds to every start. Each start checks its own
    // correctness.
    let budget = std::time::Duration::from_millis(1500);
    let mut n = 0;
    let (elapsed, tries) = engine_support::best_within(budget, 3, || {
        n += 1;
        let folder = engine_support::empty_project(t.path(), &format!("Cold{n}"));
        let started = std::time::Instant::now();
        let full = FullSubsystems::new(FullConfig {
            cache_dir: Some(t.path().join("cache")),
            ..FullConfig::native()
        });
        let handle = full.handle();
        let mut config = engine_config(t.path(), mode, Box::new(full));
        config.project = Some(folder);
        config.gpu = cutweave_engine::GpuConfig::Auto;
        if mode == EngineMode::Gui {
            config.audio = cutweave_engine::AudioConfig::Default;
            config.ui_token = Some(Token::parse(&"u".repeat(48)).unwrap());
        }
        let engine = Engine::start(config).unwrap();
        let elapsed = started.elapsed();
        assert!(engine.port().is_some());
        assert!(handle.playing_a_document());
        println!(
            "cold start to ready ({} full engine, empty project, gpu open: {}, degraded: {:?}): {:.1} ms",
            mode.name(),
            handle.has_gpu(),
            handle.degraded(),
            elapsed.as_secs_f64() * 1000.0
        );
        engine.shutdown();
        elapsed
    });
    assert!(
        elapsed < budget,
        "cold start took {elapsed:?} (best of {tries})"
    );
}

/// Without a media backend `media.capabilities` says so instead of failing.
#[test]
fn media_capabilities_without_a_backend_report_unavailable() {
    let t = TempDir::new("nocaps");
    let config = engine_config(
        t.path(),
        EngineMode::Oneshot,
        Box::new(cutweave_engine::DevStubMedia),
    );
    let engine = Engine::start(config).unwrap();
    let conn = agent(&engine);
    let caps = call(&conn, "media.capabilities", json!({})).unwrap();
    assert_eq!(
        caps,
        json!({ "available": false, "codecs": [], "hw_encode": [], "hw_decode": [] })
    );
    drop(conn);
    engine.shutdown();
}

/// Playback on a manual clock: one frame per frame of clock, pause and step keep the view's
/// playhead in step, `view.set_preview_size` applies, `playback.state` reaches clients.
#[test]
fn playback_follows_the_manual_clock_and_keeps_the_view_playhead() {
    let t = TempDir::new("manual");
    let (fake, clip) = fake_clip(t.path(), "a.mp4", FakeBackend::new());
    let clocks = Clocks::new();
    let config = full_config(t.path(), clocks.mode()).with_media(Arc::new(fake));
    let full = FullSubsystems::new(config);
    let handle = full.handle();
    let engine = Engine::start(engine_config(
        t.path(),
        EngineMode::Headless,
        Box::new(full),
    ))
    .unwrap();
    let mut rx = engine.handle().subscribe();
    let conn = agent(&engine);
    build_timeline(&conn, &clip);

    // The timeline is 2 s: play 1 s.
    let before = delivered(&handle);
    play_frames(&conn, &handle, &clocks, 30);
    let state = handle.playback_state().unwrap();
    assert!(state.playing);
    assert_eq!(state.t.flicks(), SEC, "{state:?}");
    assert!(delivered(&handle) >= before + 30);
    let counters = handle.playback_stats().unwrap();
    assert_eq!((counters.dropped, counters.errors), (0, 0), "{counters:?}");

    let paused = call(&conn, "view.pause", json!({})).unwrap();
    assert_eq!(paused["t"], json!(SEC));
    let view = call(&conn, "view.get", json!({})).unwrap();
    assert_eq!(view["playhead"], json!(SEC), "{view}");
    let stepped = call(&conn, "view.step", json!({ "frames": -3 })).unwrap();
    assert_eq!(stepped["t"], json!(SEC - 3 * full_support::FRAME_30));
    let view = call(&conn, "view.get", json!({})).unwrap();
    assert_eq!(view["playhead"], stepped["t"]);
    let size = call(
        &conn,
        "view.set_preview_size",
        json!({ "width": 321, "height": 181 }),
    )
    .unwrap();
    assert_eq!(size, json!({ "width": 321, "height": 181 }));
    let states: Vec<Value> = drain(&mut rx)
        .iter()
        .filter(|e| e.name() == "playback.state")
        .map(cutweave_engine::EngineEvent::payload)
        .collect();
    assert!(states.len() > 1, "{states:?}");
    assert!(states.iter().any(|s| s["playing"] == json!(true)));
    drop(conn);
    engine.shutdown();
}

/// Opening and closing projects moves playback to each new document; nothing accumulates
/// (threads, decoder pools).
#[test]
fn project_swaps_move_playback_without_leaking_threads() {
    let t = TempDir::new("swaps");
    let (fake, clip) = fake_clip(t.path(), "a.mp4", FakeBackend::new());
    let config = full_config(t.path(), AudioMode::FromEngine).with_media(Arc::new(fake));
    let full = FullSubsystems::new(config);
    let handle = full.handle();
    let engine = Engine::start(engine_config(
        t.path(),
        EngineMode::Headless,
        Box::new(full),
    ))
    .unwrap();
    let conn = agent(&engine);
    assert!(handle.playing_a_document());
    let mut counts = Vec::new();
    for round in 0..5 {
        let (_, _, imported) = build_timeline(&conn, &clip);
        // Analysis jobs run on the runtime's blocking pool, whose idle threads linger: let
        // them finish so rounds do not overlap.
        for job in imported["job_ids"].as_array().unwrap() {
            call(
                &conn,
                "job.wait",
                json!({ "job_id": job, "timeout_ms": 30_000 }),
            )
            .unwrap();
        }
        call(
            &conn,
            "render.frame",
            json!({ "t": "0.5s", "max_size": 64 }),
        )
        .unwrap();
        call(&conn, "view.play", json!({})).unwrap();
        let folder = t.path().join(format!("p{round}"));
        call(
            &conn,
            "project.save_as",
            json!({ "path": folder.to_string_lossy() }),
        )
        .unwrap();
        assert!(handle.playing_a_document());
        let closed = call(&conn, "project.close", json!({ "save": false })).unwrap();
        assert!(closed["closed"].is_string(), "{closed}");
        assert!(handle.playing_a_document());
        // The closed project's decoders are gone.
        assert_eq!(handle.decoder_pools(), 0);
        counts.push(thread_count());
    }
    // The first rounds warm up thread pools (GPU driver, runtime); later swaps add nothing.
    // Idle pool threads (the runtime's blocking pool keeps them 10 s) may take a while to go
    // on a busy machine: wait up to 20 s.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let mut settled = settle_threads(counts[1], 3);
    while settled > counts[1] + 3 && std::time::Instant::now() < deadline {
        settled = settle_threads(counts[1], 3);
    }
    assert!(
        settled <= counts[1] + 3,
        "threads grew across swaps: {counts:?} then {settled}"
    );
    drop(conn);
    engine.shutdown();
}

/// `Gui` mode: the preview ring is announced in `rpc.hello` (`shm`, JSON with name and epoch)
/// and its name in the run-file; shutdown removes both, leaving no shared-memory object.
#[test]
fn gui_mode_announces_the_ring_and_leaves_nothing_behind() {
    let t = TempDir::new("gui");
    let clocks = Clocks::new();
    let config = full_config(t.path(), clocks.mode());
    let full = FullSubsystems::new(config);
    let handle = full.handle();
    let mut config = engine_config(t.path(), EngineMode::Gui, Box::new(full));
    config.ui_token = Some(Token::parse(&"u".repeat(48)).unwrap());
    let engine = Engine::start(config).unwrap();
    let announced: Value = serde_json::from_str(&engine.handle().preview_ring().unwrap()).unwrap();
    let name = announced["name"].as_str().unwrap().to_owned();
    assert!(name.starts_with("cwp-"), "{announced}");
    assert!(announced["epoch"].as_str().unwrap().starts_with("0x"));
    assert_eq!(handle.ring_name().as_deref(), Some(name.as_str()));
    let run_file = engine.run_file_path().unwrap().to_path_buf();
    let contents: Value =
        serde_json::from_str(&std::fs::read_to_string(&run_file).unwrap()).unwrap();
    assert_eq!(contents["shm"], json!(name));
    // The UI connection drives the transport like the editor does.
    let ui = engine.handle().connect(&Credential::Ui).unwrap();
    let before = delivered(&handle);
    call(
        &ui,
        "view.set_preview_size",
        json!({ "width": 640, "height": 360 }),
    )
    .unwrap();
    eventually("a frame in the ring", || {
        (delivered(&handle) > before).then_some(())
    });
    drop(ui);
    engine.shutdown();
    assert!(!run_file.exists());
    // The object is gone: the same name can be created again.
    let again = FramePublisher::create_named(
        &RingName::parse(&name).unwrap(),
        &RingConfig::new(64, 64, PixelFormat::Rgba8),
    );
    assert!(
        again.is_ok(),
        "the ring {name} outlived the engine: {:?}",
        again.err()
    );
}

/// `--deterministic-ids`: the same import → insert → export gives the same documents and job
/// ids twice (WP-1B.08 end to end, with the subsystems' analysis and export jobs).
#[test]
fn deterministic_ids_hold_end_to_end() {
    let t = TempDir::new("determinism");
    let (fake, clip) = fake_clip(t.path(), "a.mp4", with_encoders(FakeBackend::new()));
    let run = |n: u32| {
        let config = FullConfig {
            analysis: true,
            ..full_config(&t.path().join(format!("caches{n}")), AudioMode::FromEngine)
        }
        .with_media(Arc::new(fake.clone()));
        let mut config = engine_config(
            t.path(),
            EngineMode::Oneshot,
            Box::new(FullSubsystems::new(config)),
        );
        config.deterministic_ids = Some(42);
        let engine = Engine::start(config).unwrap();
        let conn = agent(&engine);
        let (_, _, imported) = build_timeline(&conn, &clip);
        let out = t.path().join(format!("out{n}.mp4"));
        let export = call(
            &conn,
            "export.start",
            json!({
                "path": out.to_string_lossy(),
                "range": { "in": 0, "out": "1s" },
                "preset": { "custom": { "width": 64, "height": 36 } },
            }),
        )
        .unwrap();
        let done = call(
            &conn,
            "job.wait",
            json!({ "job_id": export["job_id"], "timeout_ms": 60_000 }),
        )
        .unwrap();
        assert_eq!(done["state"], "succeeded", "{done}");
        let doc = serde_json::to_value(&engine.handle().snapshot().unwrap().doc).unwrap();
        drop(conn);
        engine.shutdown();
        (doc, imported["job_ids"].clone(), export["job_id"].clone())
    };
    let first = run(1);
    let second = run(2);
    assert_eq!(first, second);
}

// ----- WP-1H.04: a media item on its own (ADR-0022) ------------------------------------------

/// One frame at 25 fps, in flicks.
const FRAME_25: i64 = SEC / 25;

/// A headless engine on the manual clocks with `fake`'s media, its GPU open (a headless engine
/// opens it in the background and re-renders the current frame when it is ready, which would
/// count as a frame of its own).
fn manual_engine(
    dir: &Path,
    fake: FakeBackend,
    clocks: &Clocks,
) -> (Engine, cutweave_engine::FullHandle) {
    let config = full_config(dir, clocks.mode()).with_media(Arc::new(fake));
    let full = FullSubsystems::new(config);
    let handle = full.handle();
    let engine = Engine::start(engine_config(dir, EngineMode::Headless, Box::new(full))).unwrap();
    eventually("the GPU", || handle.gpu_settled().then_some(()));
    (engine, handle)
}

/// The newest delivered frame's index.
fn last_frame(handle: &cutweave_engine::FullHandle) -> i64 {
    handle.playback_stats().map_or(-1, |s| s.last_frame)
}

/// What a source session must leave as it was: the revision, the dirty flag, the undo history,
/// the saved document and the journal of the project in `folder`.
#[derive(Debug, PartialEq)]
struct Untouched {
    rev: u64,
    dirty: bool,
    history: Value,
    document: Vec<u8>,
    journal: std::collections::BTreeMap<String, Vec<u8>>,
}

impl Untouched {
    fn now(engine: &Engine, conn: &Connection, folder: &Path) -> Untouched {
        let status = engine.handle().status().unwrap();
        Untouched {
            rev: engine.handle().snapshot().unwrap().rev.0,
            dirty: status.dirty,
            history: call(conn, "edit.history", json!({})).unwrap(),
            document: std::fs::read(status.path.unwrap()).unwrap(),
            journal: tree_bytes(&folder.join(".cutweave").join("journal")),
        }
    }
}

/// Plays the target from frame 0 (on screen) to its end, `frames` frames of `frame` flicks, one
/// manual-clock frame at a time: each frame is delivered in order and in step with the master
/// clock, and the end stops playback. Returns the worst A/V offset.
fn play_to_the_end(
    conn: &Connection,
    handle: &cutweave_engine::FullHandle,
    clocks: &Clocks,
    frames: i64,
    frame: i64,
) -> i64 {
    let state = call(conn, "view.play", json!({})).unwrap();
    assert_eq!(state["playing"], json!(true), "{state}");
    let mut worst = 0;
    for i in 1..frames {
        clocks.advance(frame);
        // A re-render of the frame before (a refresh) is not the next one: wait for this one.
        let shown = eventually("the next frame", || {
            let last = last_frame(handle);
            (last >= i).then_some(last)
        });
        assert_eq!(shown, i, "in order, none skipped");
        let offset = (clocks.audio.clock().position().flicks() - i * frame).abs();
        worst = worst.max(offset);
        assert!(offset <= frame, "frame {i}: A/V offset {offset} flicks");
    }
    let playing = handle.playback_state().unwrap();
    assert_eq!(playing.t.flicks(), (frames - 1) * frame, "{playing:?}");
    clocks.advance(frame);
    let end = eventually("the end", || {
        let s = handle.playback_state()?;
        (!s.playing).then_some(s)
    });
    assert_eq!(end.t.flicks(), frames * frame);
    let counters = handle.playback_stats().unwrap();
    assert_eq!((counters.dropped, counters.errors), (0, 0), "{counters:?}");
    worst
}

/// WP-1H.04 acceptance: a 10 s 25 fps media item plays on its own, all 250 frames in order on
/// the manual clock, in step with the master clock; seeks and steps act on it in its own time;
/// back on the timeline its frame shows at its unchanged playhead. Meanwhile the project stays
/// clean and its revision, undo history, journal and saved document stay byte-identical.
#[test]
fn a_media_item_plays_on_its_own_and_leaves_the_project_untouched() {
    let t = TempDir::new("source");
    let (fake, clip) = fake_clip(t.path(), "a.mp4", FakeBackend::new());
    let (fake, bin) = fake_media(
        t.path(),
        "b.mp4",
        fake,
        (10, FrameRate::FPS_25),
        (true, true),
    );
    let clocks = Clocks::new();
    let (engine, handle) = manual_engine(t.path(), fake, &clocks);
    let conn = agent(&engine);
    build_timeline(&conn, &clip);
    let media = import_one(&conn, &bin);
    let folder = t.path().join("Source");
    call(
        &conn,
        "project.save_as",
        json!({ "path": folder.to_string_lossy() }),
    )
    .unwrap();
    let seq = call(&conn, "view.seek", json!({ "t": "1s" })).unwrap();
    assert_eq!(seq["t"], json!(SEC));
    eventually("the timeline's frame at 1 s", || {
        (last_frame(&handle) == 30).then_some(())
    });

    let before = Untouched::now(&engine, &conn, &folder);
    assert!(!before.dirty, "{before:?}");

    let s = call(&conn, "view.set_source", json!({ "media": media })).unwrap();
    assert_eq!(
        s["source"],
        json!({ "media": media, "duration": 10 * SEC, "fps": { "num": 25, "den": 1 } })
    );
    assert_eq!(
        (s["t"].clone(), s["playing"].clone()),
        (json!(0), json!(false))
    );
    eventually("the source's first frame", || {
        (last_frame(&handle) == 0).then_some(())
    });
    let worst = play_to_the_end(&conn, &handle, &clocks, 250, FRAME_25);
    println!("source: 250 frames at 25 fps in order, worst A/V offset {worst} flicks");
    assert_eq!(
        call(&conn, "view.get", json!({})).unwrap()["playhead"],
        json!(SEC),
        "the timeline's playhead does not move while a source plays"
    );

    // Seeks and steps act on the source, in its time.
    let s = call(&conn, "view.seek", json!({ "t": "f:100" })).unwrap();
    assert_eq!(s["t"], json!(100 * FRAME_25));
    let s = call(&conn, "view.step", json!({ "frames": -1 })).unwrap();
    assert_eq!(s["t"], json!(99 * FRAME_25));
    eventually("the stepped source frame", || {
        (last_frame(&handle) == 99).then_some(())
    });
    assert_eq!(
        call(&conn, "view.get", json!({})).unwrap()["playhead"],
        json!(SEC)
    );

    // Back to the timeline: its frame at its unchanged playhead; the source keeps its own.
    let s = call(&conn, "view.set_source", json!({})).unwrap();
    assert_eq!(
        (s["source"].clone(), s["t"].clone()),
        (json!(null), json!(SEC))
    );
    eventually("the timeline's frame again", || {
        (last_frame(&handle) == 30).then_some(())
    });
    let s = call(&conn, "view.set_source", json!({ "media": media })).unwrap();
    assert_eq!(s["t"], json!(99 * FRAME_25));
    call(&conn, "view.set_source", json!({})).unwrap();

    // Nothing reached the project.
    assert_eq!(Untouched::now(&engine, &conn, &folder), before);
    // Unknown or unplayable media are refused.
    let err = call(
        &conn,
        "view.set_source",
        json!({ "media": "med_0190f5c200007000800000000000000a" }),
    )
    .unwrap_err();
    assert_eq!(err.code(), "not_found", "{err}");
    drop(conn);
    engine.shutdown();
}

/// WP-1H.04 acceptance: removing the asset while it plays returns to the timeline (stopped, on
/// its playhead); undoing the removal does not bring the source back; opening another project
/// (`doc.reset`) ends a source too.
#[test]
fn removing_the_media_or_opening_another_project_ends_the_source() {
    let t = TempDir::new("source-ends");
    let (fake, clip) = fake_clip(t.path(), "a.mp4", FakeBackend::new());
    let (fake, bin) = fake_media(
        t.path(),
        "b.mp4",
        fake,
        (4, FrameRate::FPS_30),
        (true, true),
    );
    let clocks = Clocks::new();
    let (engine, handle) = manual_engine(t.path(), fake, &clocks);
    let mut rx = engine.handle().subscribe();
    let conn = agent(&engine);
    build_timeline(&conn, &clip);
    let media = import_one(&conn, &bin);
    call(&conn, "view.seek", json!({ "t": SEC / 2 })).unwrap();

    let s = call(&conn, "view.set_source", json!({ "media": media })).unwrap();
    assert_eq!(s["source"]["media"], media);
    play_frames(&conn, &handle, &clocks, 10);
    assert!(handle.playback_state().unwrap().playing);
    call(&conn, "media.remove", json!({ "id": media })).unwrap();
    let back = eventually("the source to end", || {
        let s = handle.playback_state()?;
        s.source.is_none().then_some(s)
    });
    assert!(!back.playing, "{back:?}");
    assert_eq!(back.t.flicks(), SEC / 2, "the timeline's own playhead");
    // Clients hear it: a `playback.state` with `source: null` after ones with the source (the
    // status pump's 30 Hz throttle runs on the manual host clock: move it on).
    let mut states: Vec<Value> = Vec::new();
    eventually("playback.state without the source", || {
        clocks.advance(full_support::FRAME_30);
        states.extend(
            drain(&mut rx)
                .iter()
                .filter(|e| e.name() == "playback.state")
                .map(cutweave_engine::EngineEvent::payload),
        );
        let with_source = states.iter().rposition(|s| s["source"]["media"] == media)?;
        let without = states.iter().rposition(|s| s["source"].is_null())?;
        (without > with_source).then_some(())
    });
    // Undo brings the asset back, not the source.
    call(&conn, "edit.undo", json!({})).unwrap();
    assert!(
        engine.handle().snapshot().unwrap().doc.media.contains_key(
            &serde_json::from_value::<cutweave_model::MediaId>(media.clone()).unwrap()
        )
    );
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(handle.playback_state().unwrap().source.is_none());

    // Another document (`doc.reset`): it plays its own timeline.
    let folder = t.path().join("Ends");
    call(
        &conn,
        "project.save_as",
        json!({ "path": folder.to_string_lossy() }),
    )
    .unwrap();
    let s = call(&conn, "view.set_source", json!({ "media": media })).unwrap();
    assert_eq!(s["source"]["media"], media);
    call(&conn, "project.close", json!({ "save": false })).unwrap();
    let state = handle.playback_state().unwrap();
    assert!(state.source.is_none(), "{state:?}");
    let err = call(&conn, "view.set_source", json!({ "media": media })).unwrap_err();
    assert_eq!(err.code(), "not_found", "{err}");
    drop(conn);
    engine.shutdown();
}

/// WP-1H.04 acceptance: an audio-only asset plays as a source (on a 30 fps grid over a blank
/// picture) and its sound goes through the mixer to the (null) output.
#[test]
fn an_audio_only_media_item_plays_its_sound() {
    let t = TempDir::new("source-audio");
    let (fake, tone) = fake_media(
        t.path(),
        "tone.wav",
        FakeBackend::new(),
        (5, FrameRate::FPS_30),
        (false, true),
    );
    let config = full_config(t.path(), AudioMode::FromEngine).with_media(Arc::new(fake));
    let full = FullSubsystems::new(config);
    let handle = full.handle();
    let engine = Engine::start(engine_config(
        t.path(),
        EngineMode::Headless,
        Box::new(full),
    ))
    .unwrap();
    let conn = agent(&engine);
    call(&conn, "project.new", json!({ "name": "Tone" })).unwrap();
    let media = import_one(&conn, &tone);
    let s = call(&conn, "view.set_source", json!({ "media": media })).unwrap();
    assert_eq!(
        s["source"],
        json!({ "media": media, "duration": 5 * SEC, "fps": { "num": 30, "den": 1 } })
    );
    let silent = handle.audio_output_stats().unwrap().master_peak;
    assert!(silent[0] < 1e-6, "silence before playing: {silent:?}");
    call(&conn, "view.play", json!({})).unwrap();
    let peak = eventually("the tone in the master meter", || {
        let out = handle.audio_output_stats()?;
        (out.master_peak[0] > 0.01).then_some(out.master_peak)
    });
    let state = eventually("the master clock to move", || {
        let s = handle.playback_state()?;
        (s.t.flicks() > SEC / 10).then_some(s)
    });
    assert!(state.playing && state.source.is_some(), "{state:?}");
    let mixer = handle.mixer_stats().unwrap();
    assert_eq!(mixer.source_errors, 0, "{mixer:?}");
    assert!(delivered(&handle) > 0, "the blank picture is delivered too");
    println!("audio-only source: master peak {peak:?}, {mixer:?}");
    call(&conn, "view.pause", json!({})).unwrap();
    drop(conn);
    engine.shutdown();
}
