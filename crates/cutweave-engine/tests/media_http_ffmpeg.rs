// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1I.03 acceptance criteria on the FFmpeg backend and the corpus (feature `ffmpeg`; needs
//! the bundle, docs/dev/ffmpeg.md, and `just corpus`), over real HTTP with reqwest:
//!
//! - `/media/{id}/thumb` returns a decodable JPEG of the requested width; repeating it with
//!   `If-None-Match` gives 304 with no work started (so no decode);
//! - `/media/frame` at t = 2 s returns the same bytes as the `render.frame` query;
//! - disconnecting mid-request cancels a real decode within 100 ms;
//! - no token → 401, `w=100000` → 422;
//! - the filmstrip and waveform the import analysis cached are served.

#![cfg(feature = "ffmpeg")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod engine_support;
mod full_support;
mod media_support;

use std::path::Path;
use std::time::Duration as Wall;

use cutweave_engine::{Connection, EngineMode, FullConfig, FullSubsystems};
use engine_support::TempDir;
use full_support::{Clocks, call, corpus_file, engine_config};
use media_support::{Server, base64_decode, is_png, jpeg_size};
use serde_json::{Value, json};

/// A headless FFmpeg engine (software GPU, manual clocks) serving `/media`, with `clips`
/// imported (analysis waited for); the first one is on the timeline from 0 for 4 s.
fn server(dir: &Path, clips: &[&Path]) -> (Server, Connection, Vec<String>) {
    let clocks = Clocks::new();
    let config = FullConfig {
        cache_dir: Some(dir.join("cache")),
        audio: clocks.mode(),
        headless_preview: (320, 180),
        ..FullConfig::native()
    };
    assert!(config.media.is_some(), "{:?}", config.media_problem);
    let mut config = engine_config(
        dir,
        EngineMode::Headless,
        Box::new(FullSubsystems::new(config)),
    );
    config.allowed_roots.extend(full_support::corpus_dir());
    let s = Server::start(config, 4);
    let conn = s.agent();
    let project = call(&conn, "project.new", json!({ "name": "Corpus" })).unwrap();
    let paths: Vec<String> = clips
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let imported = call(&conn, "media.import", json!({ "paths": paths })).unwrap();
    assert_eq!(imported["failed"], json!([]), "{imported}");
    for job in imported["job_ids"].as_array().unwrap() {
        let info = call(
            &conn,
            "job.wait",
            json!({ "job_id": job, "timeout_ms": 60_000 }),
        )
        .unwrap();
        assert_eq!(info["state"], "succeeded", "{info}");
    }
    let media: Vec<String> = imported["media_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m.as_str().unwrap().to_owned())
        .collect();
    call(
        &conn,
        "timeline.insert",
        json!({
            "sequence": project["sequence"], "track": project["video_track"],
            "media": media[0], "at": 0, "duration": "4s",
        }),
    )
    .unwrap();
    (s, conn, media)
}

#[test]
fn thumbnails_etags_frames_and_auth_on_the_corpus() {
    let Some(clip) = corpus_file("sintel_trailer_480p.mp4") else {
        return;
    };
    let t = TempDir::new("media-corpus");
    let (s, conn, media) = server(t.path(), &[&clip]);
    let id = &media[0];

    // A decodable JPEG of the requested width (854×480 → 320×180).
    let path = format!("/media/{id}/thumb?t=1s&w=320");
    let thumb = s.get(&path, &[]);
    assert_eq!(
        thumb.status,
        200,
        "{}",
        String::from_utf8_lossy(&thumb.body)
    );
    assert_eq!(thumb.header("content-type").as_deref(), Some("image/jpeg"));
    assert_eq!(jpeg_size(&thumb.body), (320, 180));
    let etag = thumb.header("etag").unwrap();

    // The repeat with `If-None-Match`: 304, and no work started (the only path that decodes).
    let before = s.stats();
    let again = s.get(&path, &[("if-none-match", etag.as_str())]);
    assert_eq!(again.status, 304);
    let after = s.stats();
    assert_eq!(after.produced, before.produced, "a 304 started work");
    assert_eq!(after.not_modified, before.not_modified + 1);

    // `/media/frame` at 2 s is `render.frame` at 2 s, byte for byte.
    for (fmt, params) in [
        (
            "jpeg",
            json!({ "t": 1_411_200_000_i64, "max_size": 640, "format": "jpeg" }),
        ),
        (
            "png",
            json!({ "t": 1_411_200_000_i64, "max_size": 640, "format": "png" }),
        ),
    ] {
        let reply = s.get(&format!("/media/frame?t=1411200000&w=640&fmt={fmt}"), &[]);
        assert_eq!(
            reply.status,
            200,
            "{}",
            String::from_utf8_lossy(&reply.body)
        );
        let query = call(&conn, "render.frame", params).unwrap();
        assert_eq!(query["notes"], json!([]), "{query}");
        let expected = base64_decode(query["data"].as_str().unwrap());
        assert_eq!(reply.body.len(), expected.len(), "{fmt}");
        assert!(reply.body == expected, "{fmt}: the bytes differ");
    }

    // No token → 401; `w=100000` → 422.
    assert_eq!(s.get_anonymous(&path).status, 401);
    let wide = s.get(&format!("/media/{id}/thumb?w=100000"), &[]);
    assert_eq!(wide.status, 422);
    assert_eq!(wide.json()["error"]["param"], "w");

    // The cached filmstrip and waveform of the import analysis.
    let strip = s.get(&format!("/media/{id}/filmstrip"), &[]);
    assert_eq!(strip.status, 200);
    let manifest: Value =
        serde_json::from_str(&strip.header("x-cutweave-filmstrip").unwrap()).unwrap();
    let (w, h) = jpeg_size(&strip.body);
    assert_eq!(
        (json!(w), json!(h)),
        (manifest["sprite_w"].clone(), manifest["sprite_h"].clone())
    );
    let peaks = s.get(&format!("/media/{id}/waveform?lod=3"), &[]);
    assert_eq!(peaks.status, 200);
    assert_eq!(&peaks.body[..4], b"CWPD");
    let png = s.get(&format!("/media/{id}/waveform?fmt=png"), &[]);
    assert!(is_png(&png.body));

    // A signed URL serves the thumbnail without a header.
    let signed = s.sign(std::slice::from_ref(&path), None).remove(0);
    assert_eq!(s.get_anonymous(&signed).body, thumb.body);
    drop(conn);
}

#[test]
fn disconnecting_cancels_a_real_decode_within_100_ms() {
    let Some(clip) = corpus_file("h264_4k30_longgop.mp4") else {
        return;
    };
    let t = TempDir::new("media-cancel");
    let (s, conn, media) = server(t.path(), &[&clip]);
    // A filmstrip nobody cached: 200 large tiles of a 4K long-GOP clip, seconds of decoding.
    let path = format!("/media/{}/filmstrip?w=1024&count=200", media[0]);
    let budget = Wall::from_millis(100);
    let (stopped, tries) = s.disconnect_best_of(&path, Wall::from_millis(300), budget, 5);
    println!(
        "the FFmpeg decode stopped {:.1} ms after the disconnect ({tries} attempt(s))",
        stopped.as_secs_f64() * 1000.0
    );
    assert!(stopped < budget, "{stopped:?} after {tries} attempt(s)");
    assert_eq!(s.stats().cancelled, u64::from(tries));
    drop(conn);
}
