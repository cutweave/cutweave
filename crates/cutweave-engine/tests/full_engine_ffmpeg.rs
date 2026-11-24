// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1I.05 acceptance 1, on the FFmpeg backend and the corpus (feature `ffmpeg`; needs the
//! bundle, docs/dev/ffmpeg.md, and `cargo xtask fetch-corpus --tier small`):
//!
//! a full headless engine (manual clock, software GPU) imports corpus clips, places one,
//! renders `render.frame`, plays 2 s on the manual clock, exports 1 s (checked with `ffprobe`)
//! and shuts down with no leaked threads or shared-memory objects. Also: real probe data for
//! H.264, HEVC 10-bit and the rotated VFR phone clip, and `media.capabilities` from FFmpeg.
//!
//! WP-1H.04 (ADR-0022): corpus clips play as sources (`view.set_source`), frame by frame on the
//! manual clock without touching the document, and in real time on the null output (a
//! measurement for an idle machine: `--run-ignored only -E 'test(real_time)'`).

#![cfg(feature = "ffmpeg")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod engine_support;
mod full_support;

use std::process::Command;
use std::time::Instant;

use cutweave_api::Credential;
use cutweave_engine::{Connection, Engine, EngineMode, FullConfig, FullSubsystems};
use engine_support::{TempDir, settle_threads, thread_count};
use full_support::{
    Clocks, SEC, call, corpus_file, delivered, engine_config, ffmpeg_tool, play_frames,
};
use serde_json::{Value, json};

/// The engine the app runs, with its caches in `dir` and the manual clocks.
fn engine(dir: &std::path::Path, clocks: &Clocks) -> (Engine, cutweave_engine::FullHandle) {
    let config = FullConfig {
        cache_dir: Some(dir.join("cache")),
        audio: clocks.mode(),
        headless_preview: (320, 180),
        ..FullConfig::native()
    };
    assert!(config.media.is_some(), "{:?}", config.media_problem);
    let full = FullSubsystems::new(config);
    let handle = full.handle();
    let mut config = engine_config(dir, EngineMode::Headless, Box::new(full));
    // Agents import the corpus in place (never writing to it).
    config.allowed_roots.extend(full_support::corpus_dir());
    let engine = Engine::start(config).unwrap();
    (engine, handle)
}

fn agent(engine: &Engine) -> Connection {
    engine
        .handle()
        .connect(&Credential::Agent("cli".into()))
        .unwrap()
}

fn wait_jobs(conn: &Connection, jobs: &Value) {
    for job in jobs.as_array().unwrap() {
        let info = call(
            conn,
            "job.wait",
            json!({ "job_id": job, "timeout_ms": 60_000 }),
        )
        .unwrap();
        assert_eq!(info["state"], "succeeded", "{info}");
    }
}

/// `ffprobe -show_streams -show_format` of `path` as JSON.
fn ffprobe(tool: &std::path::Path, path: &std::path::Path) -> Value {
    let out = Command::new(tool)
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_streams",
            "-show_format",
        ])
        .arg(path)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    serde_json::from_slice(&out.stdout).unwrap()
}

/// Acceptance 1. The scenario runs twice: process-wide pools that outlive an engine by design
/// (the software rasterizer's worker threads, the runtime's idle blocking threads) are warm
/// after the first run, so the second must end where the first did.
#[test]
fn import_render_play_export_and_shut_down_cleanly_on_the_corpus() {
    let (Some(clip), Some(probe_tool)) = (
        corpus_file("sintel_trailer_480p.mp4"),
        ffmpeg_tool("ffprobe"),
    ) else {
        return;
    };
    let t = TempDir::new("corpus");
    let before = thread_count();
    scenario(&t, &clip, &probe_tool, 1);
    let warm = settle_threads(before, 0);
    scenario(&t, &clip, &probe_tool, 2);
    // The software rasterizer tears its device down asynchronously: allow a few stragglers
    // (three runs measured 31, 31, 34 threads; an engine-sized leak is 25+).
    let after = settle_threads(warm, 3);
    println!("threads: {before} at start, {warm} after the first run, {after} after the second");
    assert!(
        after <= warm + 3,
        "threads leaked: {warm} after the first run, {after} after the second"
    );
}

/// Imports `clip`: real probe data, a fast content hash, the analysis jobs (waited for), the
/// poster thumbnail in the cache. Returns the media id.
fn import(engine: &Engine, conn: &Connection, clip: &std::path::Path) -> Value {
    let imported = call(
        conn,
        "media.import",
        json!({ "paths": [clip.to_string_lossy()] }),
    )
    .unwrap();
    assert_eq!(imported["failed"], json!([]), "{imported}");
    let media = imported["media_ids"][0].clone();
    assert_eq!(
        imported["job_ids"].as_array().unwrap().len(),
        3,
        "{imported}"
    );
    wait_jobs(conn, &imported["job_ids"]);
    let snapshot = engine.handle().snapshot().unwrap();
    let asset = snapshot.doc.media.values().next().unwrap();
    assert_eq!(asset.probe.streams[0].codec.as_deref(), Some("h264"));
    assert_eq!(asset.probe.streams[0].width, Some(854));
    assert!(
        asset
            .content_hash
            .as_deref()
            .unwrap()
            .starts_with("blake3-fast:")
    );
    let state = engine.handle().media_state().unwrap();
    let file = state
        .media(serde_json::from_value(media.clone()).unwrap())
        .unwrap();
    let reference = cutweave_engine::MediaState::media_ref(&file).unwrap();
    let thumbs = state.thumbs.as_ref().unwrap();
    assert!(
        thumbs
            .cached_thumb(&reference, cutweave_model::Time::ZERO, 320)
            .unwrap()
            .is_some(),
        "the poster thumbnail is not cached"
    );
    media
}

/// Exports the first second to `out` and checks it with `ffprobe`: H.264 + AAC, 1 s long.
fn export_one_second(conn: &Connection, out: &std::path::Path, probe_tool: &std::path::Path) {
    let export = call(
        conn,
        "export.start",
        json!({
            "path": out.to_string_lossy(),
            "range": { "in": 0, "out": "1s" },
            "preset": { "custom": { "width": 640, "height": 360 } },
            "hw": "off",
        }),
    )
    .unwrap();
    wait_jobs(conn, &json!([export["job_id"]]));
    let probed = ffprobe(probe_tool, out);
    let streams = probed["streams"].as_array().unwrap();
    assert!(
        streams.iter().any(|s| s["codec_name"] == "h264"),
        "{probed}"
    );
    assert!(streams.iter().any(|s| s["codec_name"] == "aac"), "{probed}");
    let duration: f64 = probed["format"]["duration"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((duration - 1.0).abs() < 0.1, "{duration}");
}

/// Import → insert → `render.frame` → play 2 s on the manual clock → export 1 s → shutdown.
fn scenario(t: &TempDir, clip: &std::path::Path, probe_tool: &std::path::Path, round: u32) {
    let started = Instant::now();
    let clocks = Clocks::new();
    let (engine, handle) = engine(t.path(), &clocks);
    let conn = agent(&engine);
    let project = call(&conn, "project.new", json!({ "name": "Corpus" })).unwrap();
    let media = import(&engine, &conn, clip);
    println!("import + analysis: {:?}", started.elapsed());

    // Insert 3 s.
    let inserted = call(
        &conn,
        "timeline.insert",
        json!({
            "sequence": project["sequence"], "track": project["video_track"],
            "media": media, "at": 0, "duration": "3s",
        }),
    )
    .unwrap();
    let item = inserted["clips"][0].clone();

    // render.frame: the clip is on screen, nothing missing; different times differ.
    let at = |t: &str| {
        call(
            &conn,
            "render.frame",
            json!({ "t": t, "max_size": 320, "format": "png" }),
        )
        .unwrap()
    };
    let one = at("1s");
    assert_eq!(one["items_visible"][0]["item"], item, "{one}");
    assert_eq!(one["notes"], json!([]), "{one}");
    assert_eq!(
        (one["width"].clone(), one["height"].clone()),
        (json!(320), json!(180))
    );
    let two = at("2s");
    assert_ne!(
        one["data"], two["data"],
        "the frames at 1 s and 2 s are identical"
    );
    println!("render.frame: {:?}", started.elapsed());

    // Play 2 s on the manual clock: every frame rendered and delivered, none dropped.
    let before = delivered(&handle);
    play_frames(&conn, &handle, &clocks, 60);
    println!("playback: {:?}", started.elapsed());
    let playing = handle.playback_state().unwrap();
    assert_eq!(playing.t.flicks(), 2 * SEC, "{playing:?}");
    let counters = handle.playback_stats().unwrap();
    assert!(counters.delivered >= before + 60, "{counters:?}");
    assert_eq!((counters.dropped, counters.errors), (0, 0), "{counters:?}");
    let paused = call(&conn, "view.pause", json!({})).unwrap();
    assert_eq!(paused["t"], json!(2 * SEC));

    export_one_second(&conn, &t.path().join(format!("out{round}.mp4")), probe_tool);
    println!("export: {:?}", started.elapsed());

    // Shutdown: playback, audio, decoders, GPU and caches stop.
    drop(conn);
    engine.shutdown();
    println!(
        "run {round}: import, analysis, render.frame, 2 s playback, 1 s export, shutdown: {:.0} ms",
        started.elapsed().as_secs_f64() * 1000.0
    );
}

/// Real probe data from FFmpeg for the clips F17 imports (H.264, HEVC 10-bit, the rotated
/// VFR phone clip), and the backend's capabilities.
#[test]
fn real_probe_data_and_capabilities() {
    let names = [
        "h264_1080p60.mp4",
        "hevc_10bit_sdr.mp4",
        "vfr_phone_rotated90.mp4",
    ];
    let files: Option<Vec<_>> = names.iter().map(|n| corpus_file(n)).collect();
    let Some(files) = files else { return };
    let t = TempDir::new("probe");
    let clocks = Clocks::new();
    let (engine, _) = engine(t.path(), &clocks);
    let conn = agent(&engine);
    let project = call(&conn, "project.new", json!({ "name": "Probe" })).unwrap();
    let paths: Vec<String> = files
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let imported = call(&conn, "media.import", json!({ "paths": paths })).unwrap();
    assert_eq!(imported["failed"], json!([]), "{imported}");
    let snapshot = engine.handle().snapshot().unwrap();
    let by_name = |name: &str| {
        snapshot
            .doc
            .media
            .values()
            .find(|m| m.path_abs.as_deref().is_some_and(|p| p.ends_with(name)))
            .unwrap()
            .clone()
    };
    let hevc = by_name("hevc_10bit_sdr.mp4");
    assert_eq!(hevc.probe.streams[0].codec.as_deref(), Some("hevc"));
    let phone = by_name("vfr_phone_rotated90.mp4");
    // A quarter turn (the display matrix says which way).
    assert_eq!(phone.probe.rotation % 180, 90, "{:?}", phone.probe);
    assert!(phone.probe.vfr, "{:?}", phone.probe);

    // Every clip renders: the HEVC 10-bit one decodes in a layout the compositor does not read
    // (`yuv420p10le`) and is converted; the phone clip turns upright (its size swaps).
    let (seq, track) = (project["sequence"].clone(), project["video_track"].clone());
    let mut at = 0;
    for media in [&hevc, &phone] {
        let inserted = call(
            &conn,
            "timeline.insert",
            json!({ "sequence": seq, "track": track, "media": media.id, "at": at, "duration": "2s" }),
        )
        .unwrap();
        let item = inserted["clips"][0].clone();
        let frame = call(
            &conn,
            "render.frame",
            json!({ "t": at + SEC, "max_size": 320, "format": "png" }),
        )
        .unwrap();
        assert_eq!(frame["items_visible"][0]["item"], item, "{frame}");
        assert_eq!(frame["notes"], json!([]), "{frame}");
        at += 2 * SEC;
    }
    let caps = call(&conn, "media.capabilities", json!({})).unwrap();
    assert_eq!(caps["available"], json!(true));
    let h264 = caps["codecs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "h264")
        .unwrap()
        .clone();
    assert!(h264["encoders"].to_string().contains("libx264"), "{h264}");
    println!("hardware encoders: {}", caps["hw_encode"]);
    drop(conn);
    engine.shutdown();
}

// ----- WP-1H.04: media items on their own (ADR-0022) -----------------------------------------

/// The start of frame `n` at `fps` (`{num, den}` as `playback.state.source` gives it).
fn frame_start(fps: &Value, n: i64) -> i64 {
    let rate = cutweave_model::FrameRate::new(
        u32::try_from(fps["num"].as_u64().unwrap()).unwrap(),
        u32::try_from(fps["den"].as_u64().unwrap()).unwrap(),
    )
    .unwrap();
    rate.frame_to_time(n).unwrap().flicks()
}

/// Imports `paths` and waits for their analysis; returns the media ids in order.
fn import_all(conn: &Connection, paths: &[std::path::PathBuf]) -> Vec<Value> {
    let paths: Vec<String> = paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let imported = call(conn, "media.import", json!({ "paths": paths })).unwrap();
    assert_eq!(imported["failed"], json!([]), "{imported}");
    wait_jobs(conn, &imported["job_ids"]);
    imported["media_ids"].as_array().unwrap().clone()
}

/// The H.264 1080p clip (60 fps, with AAC) and the HEVC 10-bit 1080p30 clip play as sources on
/// the manual clock: every frame of their first 2 s in order while the master clock advances,
/// none dropped, the document untouched, the timeline's playhead where it was.
#[test]
fn corpus_clips_play_as_sources_frame_by_frame() {
    let names = ["h264_1080p60.mp4", "hevc_10bit_sdr.mp4"];
    let files: Option<Vec<_>> = names.iter().map(|n| corpus_file(n)).collect();
    let Some(files) = files else { return };
    let t = TempDir::new("corpus-sources");
    let clocks = Clocks::new();
    let (engine, handle) = engine(t.path(), &clocks);
    let conn = agent(&engine);
    call(&conn, "project.new", json!({ "name": "Sources" })).unwrap();
    let media = import_all(&conn, &files);
    let rev = engine.handle().snapshot().unwrap().rev;
    for (name, id) in names.iter().zip(&media) {
        let started = Instant::now();
        let s = call(&conn, "view.set_source", json!({ "media": id })).unwrap();
        let fps = s["source"]["fps"].clone();
        assert_eq!(s["source"]["media"], *id, "{s}");
        full_support::eventually("the source's first frame", || {
            (handle.playback_stats()?.last_frame == 0).then_some(())
        });
        let state = call(&conn, "view.play", json!({})).unwrap();
        assert_eq!(state["playing"], json!(true), "{state}");
        // Playing seeked the master clock to the source's 0: it runs in the source's time.
        assert_eq!(clocks.audio.clock().position().flicks(), 0);
        let frames = (0..)
            .take_while(|n| frame_start(&fps, *n) < 2 * SEC)
            .count();
        let frames = i64::try_from(frames).unwrap();
        for n in 1..frames {
            let before = delivered(&handle);
            clocks.advance(frame_start(&fps, n) - clocks.audio.clock().position().flicks());
            full_support::eventually("the next source frame", || {
                (delivered(&handle) > before).then_some(())
            });
            assert_eq!(
                handle.playback_stats().unwrap().last_frame,
                n,
                "{name}: in order, none skipped"
            );
        }
        let state = handle.playback_state().unwrap();
        assert!(state.playing, "{name}: {state:?}");
        assert_eq!(state.t.flicks(), frame_start(&fps, frames - 1));
        let counters = handle.playback_stats().unwrap();
        assert_eq!(
            (counters.dropped, counters.errors),
            (0, 0),
            "{name}: {counters:?}"
        );
        call(&conn, "view.pause", json!({})).unwrap();
        println!(
            "{name} as a source: {frames} frames at {}/{} fps in order, 0 dropped, {:.0} ms",
            fps["num"],
            fps["den"],
            started.elapsed().as_secs_f64() * 1000.0
        );
    }
    let back = call(&conn, "view.set_source", json!({})).unwrap();
    assert_eq!(
        (back["source"].clone(), back["t"].clone()),
        (json!(null), json!(0))
    );
    assert_eq!(
        engine.handle().snapshot().unwrap().rev,
        rev,
        "the document did not change"
    );
    drop(conn);
    engine.shutdown();
}

/// The dev-machine measurement (WP-1H.04 acceptance): the H.264 1080p clip and the HEVC 10-bit
/// clip each play as a source for 10 s in real time (on the null output's wall clock; replayed
/// from the start when they end) with 0 dropped frames, and the WAV's sound reaches the master
/// meter. Real time on an idle machine, so not a gate: run it alone with
/// `cargo nextest run -p cutweave-engine --features ffmpeg --run-ignored only -E 'test(real_time)'`.
#[test]
#[ignore = "real-time measurement on an idle machine; run alone (see the doc comment)"]
fn corpus_sources_play_in_real_time_without_drops() {
    let names = ["h264_1080p60.mp4", "hevc_10bit_sdr.mp4", "audio_48000.wav"];
    let files: Option<Vec<_>> = names.iter().map(|n| corpus_file(n)).collect();
    let Some(files) = files else { return };
    let t = TempDir::new("corpus-real-time");
    let config = FullConfig {
        cache_dir: Some(t.path().join("cache")),
        audio: cutweave_engine::AudioMode::FromEngine,
        ..FullConfig::native()
    };
    let full = FullSubsystems::new(config);
    let handle = full.handle();
    let mut config = engine_config(t.path(), EngineMode::Headless, Box::new(full));
    config.allowed_roots.extend(full_support::corpus_dir());
    config.gpu = cutweave_engine::GpuConfig::Auto;
    let engine = Engine::start(config).unwrap();
    let conn = agent(&engine);
    call(&conn, "project.new", json!({ "name": "Real time" })).unwrap();
    let media = import_all(&conn, &files);
    // Let the GPU open before measuring.
    full_support::eventually("the GPU", || handle.gpu_settled().then_some(()));
    let mut report = Vec::new();
    for (name, id) in names.iter().zip(&media).take(2) {
        call(&conn, "view.set_source", json!({ "media": id })).unwrap();
        let before = handle.playback_stats().unwrap();
        let started = Instant::now();
        call(&conn, "view.play", json!({})).unwrap();
        while started.elapsed() < std::time::Duration::from_secs(10) {
            std::thread::sleep(std::time::Duration::from_millis(20));
            if !handle.playback_state().unwrap().playing {
                // The clip ended: play it again from its start.
                call(&conn, "view.play", json!({})).unwrap();
            }
        }
        call(&conn, "view.pause", json!({})).unwrap();
        let after = handle.playback_stats().unwrap();
        let (frames, dropped) = (
            after.delivered - before.delivered,
            after.dropped - before.dropped,
        );
        println!(
            "{name}: 10 s in real time as a source: {frames} frames delivered, {dropped} dropped \
             (gpu: {}, degraded: {:?})",
            handle.has_gpu(),
            handle.degraded()
        );
        report.push((*name, frames, dropped));
    }
    // The WAV: its sound reaches the master meter while the master clock runs.
    call(&conn, "view.set_source", json!({ "media": media[2] })).unwrap();
    call(&conn, "view.play", json!({})).unwrap();
    let peak = full_support::eventually("the WAV in the master meter", || {
        let out = handle.audio_output_stats()?;
        (out.master_peak[0] > 0.01).then_some(out.master_peak)
    });
    let moved = full_support::eventually("the master clock to move", || {
        let s = handle.playback_state()?;
        (s.t.flicks() > SEC / 2).then_some(s.t)
    });
    let mixer = handle.mixer_stats().unwrap();
    println!("audio_48000.wav as a source: master peak {peak:?} at {moved:?}, {mixer:?}");
    assert_eq!(mixer.source_errors, 0, "{mixer:?}");
    call(&conn, "view.pause", json!({})).unwrap();
    drop(conn);
    engine.shutdown();
    for (name, frames, dropped) in report {
        assert!(frames > 0, "{name}: nothing played");
        assert_eq!(dropped, 0, "{name}: {dropped} frames dropped in 10 s");
    }
}
