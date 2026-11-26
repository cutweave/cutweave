// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2E.02 through the full engine (media from `FakeBackend`, the null audio output):
//!
//! - `audio.get_levels` measures a sequence's mix with the engine's decoders: a −20 dBFS tone
//!   reads −20 dB peak and RMS; muting the track silences it; the limiter switch is echoed.
//! - `playback.meters` arrives while playing, with the master and the track bus, and follows
//!   a mute within the ramp.
//! - A core engine without media decoding answers `audio.get_levels` with `Unsupported` (and
//!   an empty sequence with `Invalid`).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod engine_support;
mod full_support;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cutweave_api::Credential;
use cutweave_engine::{AudioMode, Connection, Engine, EngineMode, METERS_EVENT};
use cutweave_media::testing::{FakeAudio, FakeBackend, FakeMedia, FakeVideo};
use cutweave_model::{Duration, FrameRate, SampleRate};
use engine_support::TempDir;
use full_support::{Spy, call, drain, engine_config, eventually, full_config};
use serde_json::{Value, json};

/// A 3 s clip (picture and a stereo tone at `amplitude`) as a file on disk and in `FakeBackend`.
fn tone_clip(dir: &Path, amplitude: f32) -> (FakeBackend, PathBuf) {
    let path = dir.join("tone.mp4");
    std::fs::write(&path, "fake media tone").unwrap();
    let d = Duration::from_secs(3).unwrap();
    let mut audio = FakeAudio::tone(SampleRate::HZ_48_000, 2, d);
    audio.amplitude = amplitude;
    let media = FakeMedia {
        video: Some(FakeVideo::lasting(320, 180, FrameRate::FPS_30, d)),
        audio: Some(audio),
    };
    (FakeBackend::new().with_media(&path, media), path)
}

fn agent(engine: &Engine) -> Connection {
    engine
        .handle()
        .connect(&Credential::Agent("cli".into()))
        .unwrap()
}

/// `project.new`, the clip imported and 2 s of it inserted at 0 (its sound linked on the audio
/// track): the project output.
fn timeline(conn: &Connection, clip: &Path) -> Value {
    let project = call(conn, "project.new", json!({ "name": "Levels" })).unwrap();
    let imported = call(
        conn,
        "media.import",
        json!({ "paths": [clip.to_string_lossy()] }),
    )
    .unwrap();
    assert_eq!(imported["failed"], json!([]), "{imported}");
    call(
        conn,
        "timeline.insert",
        json!({
            "sequence": project["sequence"], "track": project["video_track"],
            "media": imported["media_ids"][0], "at": 0, "duration": "2s",
        }),
    )
    .unwrap();
    project
}

fn db(v: &Value) -> f64 {
    v.as_f64().unwrap_or_else(|| panic!("not a level: {v}"))
}

#[test]
fn get_levels_measures_the_mix_and_follows_mute() {
    let t = TempDir::new("levels");
    let (fake, clip) = tone_clip(t.path(), 0.1);
    let config = full_config(t.path(), AudioMode::FromEngine).with_media(Arc::new(fake));
    let (spy, _handle, _early) = Spy::new(config);
    let engine =
        Engine::start(engine_config(t.path(), EngineMode::Headless, Box::new(spy))).unwrap();
    let conn = agent(&engine);
    let project = timeline(&conn, &clip);

    let out = call(&conn, "audio.get_levels", json!({})).unwrap();
    assert_eq!(out["in"], json!(0));
    assert_eq!(out["out"], json!(2 * full_support::SEC));
    assert_eq!(out["limiter"], json!(true));
    let master = &out["master"];
    assert!((db(&master["peak_db"]) + 20.0).abs() < 0.05, "{out}");
    assert!((db(&master["rms_db"]) + 20.0).abs() <= 0.2, "{out}");
    assert!((db(&master["true_peak_db"]) + 20.0).abs() < 0.1, "{out}");
    assert!(db(&master["lufs_integrated"]) < -15.0, "{out}");
    assert!(master["lufs_momentary_max"].is_number(), "{out}");
    assert_eq!(out["limiter_reduction_db"], json!(0.0));
    assert_eq!(out["source_errors"], json!(0));
    let tracks = out["tracks"].as_array().unwrap();
    assert_eq!(tracks.len(), 1, "{out}");
    assert_eq!(tracks[0]["track"], project["audio_track"]);
    assert!((db(&tracks[0]["levels"]["peak_db"]) + 20.0).abs() < 0.05);
    assert!(tracks[0]["levels"].get("lufs_momentary_max").is_none());

    // A range and the limiter switch.
    let part = call(
        &conn,
        "audio.get_levels",
        json!({ "in": "0.5s", "out": "1.5s", "limiter": false }),
    )
    .unwrap();
    assert_eq!(part["in"], json!(full_support::SEC / 2));
    assert_eq!(part["limiter"], json!(false));
    assert!((db(&part["master"]["peak_db"]) + 20.0).abs() < 0.05);

    // Muted: the track reads silence, and so does the master.
    call(
        &conn,
        "track.set",
        json!({ "track": project["audio_track"], "muted": true }),
    )
    .unwrap();
    let muted = call(&conn, "audio.get_levels", json!({})).unwrap();
    assert_eq!(muted["master"]["peak_db"], Value::Null, "{muted}");
    assert_eq!(muted["master"]["lufs_integrated"], Value::Null, "{muted}");
    assert_eq!(
        muted["tracks"][0]["levels"]["rms_db"],
        Value::Null,
        "{muted}"
    );

    // Out of range.
    let e = call(&conn, "audio.get_levels", json!({ "out": "5s" })).unwrap_err();
    assert_eq!(e.code(), "invalid");
    drop(conn);
    engine.shutdown();
}

#[test]
fn playback_meters_follow_the_mix_while_playing() {
    let t = TempDir::new("meters");
    let (fake, clip) = tone_clip(t.path(), 0.5);
    let config = full_config(t.path(), AudioMode::FromEngine).with_media(Arc::new(fake));
    let (spy, _handle, early) = Spy::new(config);
    let engine =
        Engine::start(engine_config(t.path(), EngineMode::Headless, Box::new(spy))).unwrap();
    let mut rx = early.lock().unwrap().take().unwrap();
    let conn = agent(&engine);
    let project = timeline(&conn, &clip);
    let track = project["audio_track"].clone();
    call(&conn, "view.play", json!({})).unwrap();

    let mut meters = |want: &dyn Fn(&Value) -> bool| {
        let mut rx_events = Vec::new();
        eventually("a matching playback.meters event", || {
            rx_events.extend(drain(&mut rx));
            rx_events
                .iter()
                .filter(|e| e.name() == METERS_EVENT)
                .map(cutweave_engine::EngineEvent::payload)
                .find(|p| want(p))
        })
    };
    // The tone (0.5, −6 dBFS) on the master and on the track bus.
    let loud = meters(&|p| {
        p["master"]["peak_db"][0]
            .as_f64()
            .is_some_and(|v| (v + 6.02).abs() < 0.1)
    });
    assert!(loud["t"].as_i64().unwrap() > 0, "{loud}");
    let bus = &loud["buses"][0];
    assert_eq!(bus["track"], track, "{loud}");
    assert!((db(&bus["peak_db"][1]) + 6.02).abs() < 0.1, "{loud}");
    // AES17 RMS reads a sine at its peak level (within the partial cycle of a 33 ms window).
    assert!(
        (db(&bus["rms_db"][0]) - db(&bus["peak_db"][0])).abs() < 0.1,
        "{loud}"
    );
    assert_eq!(loud["master"]["gr_db"], json!(0.0), "{loud}");
    assert!(loud["master"]["true_peak_db"].is_number(), "{loud}");

    // Muted while playing: the bus and the master go silent.
    call(&conn, "track.set", json!({ "track": track, "muted": true })).unwrap();
    let quiet =
        meters(&|p| p["buses"][0]["peak_db"][0].is_null() && p["master"]["peak_db"][0].is_null());
    assert_eq!(quiet["buses"][0]["track"], track, "{quiet}");
    call(&conn, "view.pause", json!({})).unwrap();
    drop(conn);
    engine.shutdown();
}

#[test]
fn a_core_engine_cannot_measure_levels() {
    use cutweave_media::MediaBackend;
    let t = TempDir::new("levels-core");
    let (fake, clip) = tone_clip(t.path(), 0.1);
    let mut config = cutweave_engine::EngineConfig::new(EngineMode::Oneshot);
    config.allowed_roots = vec![t.path().to_path_buf()];
    let engine = Engine::start(config).unwrap();
    let conn = agent(&engine);
    let project = call(&conn, "project.new", json!({ "name": "Core" })).unwrap();
    // An empty sequence has nothing to measure.
    let e = call(&conn, "audio.get_levels", json!({})).unwrap_err();
    assert_eq!(e.code(), "invalid", "{e}");
    // With a clip, the core engine has no decoders to measure it with.
    let path = clip.to_string_lossy().into_owned();
    let probe = fake.probe(&clip).unwrap();
    let media = call(
        &conn,
        "media.register",
        json!({ "path": path, "probe": probe, "content_hash": "fake:tone" }),
    )
    .unwrap();
    call(
        &conn,
        "timeline.insert",
        json!({
            "sequence": project["sequence"], "track": project["video_track"],
            "media": media["media_id"], "at": 0, "duration": "1s",
        }),
    )
    .unwrap();
    let e = call(&conn, "audio.get_levels", json!({})).unwrap_err();
    assert_eq!(e.code(), "unsupported", "{e}");
    drop(conn);
    engine.shutdown();
}
