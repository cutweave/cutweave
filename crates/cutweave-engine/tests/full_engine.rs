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
    let folder = engine_support::empty_project(t.path(), "Cold");
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
    assert!(
        elapsed < std::time::Duration::from_millis(1500),
        "cold start took {elapsed:?}"
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
