// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1I.03: the `/media` HTTP endpoints on a full engine with `FakeBackend` media (no FFmpeg;
//! the corpus run is `media_http_ffmpeg.rs`), over real HTTP with reqwest:
//!
//! - thumbnails are JPEGs of the requested width; ETags, `Cache-Control` and `If-None-Match`
//!   → 304 with no decode, no cache access and no work started;
//! - auth: no token or a wrong one → 401; signed URLs from `POST /media/sign` work without a
//!   header and only for the exact resource; tampered and expired ones → 401;
//! - validation: `w=100000` → 422 with a JSON error naming `w`; unknown media → 404; a stale
//!   `hash` → 409; an engine without media → 503;
//! - disconnecting mid-request cancels the decode within 100 ms; the semaphore bounds the work
//!   in flight;
//! - filmstrips (sprite + manifest), waveforms (peak data and PNG, `not_ready` before the
//!   analysis) and `/media/frame` = `render.frame`, byte for byte;
//! - media with a display rotation: upright thumbnails and filmstrip tiles (#207).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod engine_support;
mod full_support;
mod media_support;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration as Wall;

use cutweave_engine::{AudioMode, Connection, EngineMode, FullConfig, FullSubsystems};
use cutweave_media::testing::{FakeAudio, FakeBackend, FakeMedia, FakeVideo};
use cutweave_model::{Duration, FrameRate, SampleRate};
use engine_support::TempDir;
use full_support::{call, engine_config, full_config};
use media_support::{Server, base64_decode, is_png, jpeg_size};
use serde_json::{Value, json};

/// A 4 s 30 fps 320×180 clip with a stereo tone, on disk (only hashed) and in `fake`. Its probe
/// reports the display `rotation` (90: a phone clip shot upright).
fn fake_clip(dir: &Path, name: &str, fake: FakeBackend, rotation: u16) -> (FakeBackend, PathBuf) {
    let path = dir.join(name);
    std::fs::write(&path, format!("fake media {name}")).unwrap();
    let d = Duration::from_secs(4).unwrap();
    let media = FakeMedia {
        video: Some(FakeVideo::lasting(320, 180, FrameRate::FPS_30, d).with_rotation(rotation)),
        audio: Some(FakeAudio::tone(SampleRate::HZ_48_000, 2, d)),
    };
    (fake.with_media(&path, media), path)
}

/// An engine with one imported clip on the timeline.
struct Fixture {
    server: Server,
    fake: FakeBackend,
    conn: Connection,
    media: String,
    hash: String,
    _dir: TempDir,
}

/// `analysis`: run the import analysis (and wait for it); `cost`: wall time per decoded frame.
fn fixture(tag: &str, analysis: bool, cost: Wall, concurrency: usize) -> Fixture {
    fixture_turned(tag, analysis, cost, concurrency, 0)
}

/// [`fixture`] with a clip whose probe reports a display rotation.
fn fixture_turned(
    tag: &str,
    analysis: bool,
    cost: Wall,
    concurrency: usize,
    rotation: u16,
) -> Fixture {
    let dir = TempDir::new(tag);
    let (fake, clip) = fake_clip(
        dir.path(),
        "a.mp4",
        FakeBackend::new().with_frame_cost(cost),
        rotation,
    );
    let config = FullConfig {
        analysis,
        ..full_config(dir.path(), AudioMode::FromEngine)
    }
    .with_media(Arc::new(fake.clone()));
    let engine = engine_config(
        dir.path(),
        EngineMode::Headless,
        Box::new(FullSubsystems::new(config)),
    );
    let server = Server::start(engine, concurrency);
    let conn = server.agent();
    let project = call(&conn, "project.new", json!({ "name": "Media" })).unwrap();
    let imported = call(
        &conn,
        "media.import",
        json!({ "paths": [clip.to_string_lossy()] }),
    )
    .unwrap();
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
    let media = imported["media_ids"][0].as_str().unwrap().to_owned();
    call(
        &conn,
        "timeline.insert",
        json!({
            "sequence": project["sequence"], "track": project["video_track"],
            "media": media, "at": 0, "duration": "3s",
        }),
    )
    .unwrap();
    let snapshot = server.engine.handle().snapshot().unwrap();
    let hash = snapshot
        .doc
        .media
        .values()
        .next()
        .unwrap()
        .content_hash
        .clone()
        .unwrap();
    Fixture {
        server,
        fake,
        conn,
        media,
        hash,
        _dir: dir,
    }
}

fn error_of(reply: &media_support::Reply) -> Value {
    reply.json()["error"].clone()
}

#[test]
fn thumbnails_are_jpegs_with_content_etags_and_a_repeat_is_304_without_work() {
    let f = fixture("thumb", false, Wall::ZERO, 4);
    let s = &f.server;
    let path = format!("/media/{}/thumb?t=1000&w=160", f.media);
    let first = s.get(&path, &[]);
    assert_eq!(
        first.status,
        200,
        "{}",
        String::from_utf8_lossy(&first.body)
    );
    assert_eq!(first.header("content-type").as_deref(), Some("image/jpeg"));
    // The 320×180 clip at 160 px wide.
    assert_eq!(jpeg_size(&first.body), (160, 90));
    let etag = first.header("etag").unwrap();
    assert!(
        etag.starts_with(&format!("\"{}-", f.hash)) && etag.ends_with('"'),
        "{etag}"
    );
    // The URL names no content: revalidate.
    assert_eq!(
        first.header("cache-control").as_deref(),
        Some("private, no-cache")
    );

    // `t` is floored to the frame grid: a time inside the same frame is the same resource.
    let same_frame = s.get(&format!("/media/{}/thumb?t=2000&w=160", f.media), &[]);
    assert_eq!(same_frame.header("etag"), Some(etag.clone()));

    // With `hash`, the URL names the content: immutable.
    let named = s.get(
        &format!("/media/{}/thumb?t=0&w=160&hash={}", f.media, f.hash),
        &[],
    );
    assert_eq!(named.status, 200);
    assert_eq!(
        named.header("cache-control").as_deref(),
        Some("private, max-age=31536000, immutable")
    );

    // A repeat with the ETag: 304 from the request alone (no decode, no index, no produce).
    let before = (s.stats(), f.fake.decoded_frames(), f.fake.index_scans());
    let again = s.get(&path, &[("if-none-match", etag.as_str())]);
    assert_eq!(again.status, 304);
    assert_eq!(again.body.len(), 0);
    assert_eq!(again.header("etag"), Some(etag.clone()));
    let after = (s.stats(), f.fake.decoded_frames(), f.fake.index_scans());
    assert_eq!(after.0.produced, before.0.produced, "work was started");
    assert_eq!(after.0.not_modified, before.0.not_modified + 1);
    assert_eq!((after.1, after.2), (before.1, before.2), "decoded on a 304");
    // A list with a weak tag matches too; another tag does not.
    let listed = s.get(&path, &[("if-none-match", &format!("\"x\", W/{etag}"))]);
    assert_eq!(listed.status, 304);
    assert_eq!(s.get(&path, &[("if-none-match", "\"other\"")]).status, 200);
    // HEAD answers the headers.
    let head = s.request(
        "HEAD",
        &path,
        &[("authorization", format!("Bearer {}", s.token))],
        None,
    );
    assert_eq!(head.status, 200);
    assert_eq!(head.header("etag"), Some(etag));
}

#[test]
fn every_route_needs_a_token_or_a_signature_for_that_resource() {
    let f = fixture("auth", false, Wall::ZERO, 4);
    let s = &f.server;
    let path = format!("/media/{}/thumb?t=0&w=64", f.media);
    // No credential, a wrong token, a token in the query: 401.
    let none = s.get_anonymous(&path);
    assert_eq!(none.status, 401);
    assert_eq!(error_of(&none)["code"], "unauthorized");
    assert_eq!(none.header("www-authenticate").as_deref(), Some("Bearer"));
    let wrong = s.request(
        "GET",
        &path,
        &[("authorization", format!("Bearer {}", "0".repeat(64)))],
        None,
    );
    assert_eq!(wrong.status, 401);
    assert_eq!(
        s.get_anonymous(&format!("{path}&token={}", s.token)).status,
        401
    );
    for route in ["/media/frame?t=0", "/media/sign"] {
        assert_eq!(s.get_anonymous(route).status, 401, "{route}");
    }

    // A signed URL works without a header, for exactly that resource.
    let signed = s.sign(std::slice::from_ref(&path), None).remove(0);
    assert!(
        signed.contains("&exp=") && signed.contains("&sig="),
        "{signed}"
    );
    assert!(!signed.contains(&s.token));
    let ok = s.get_anonymous(&signed);
    assert_eq!(ok.status, 200, "{}", String::from_utf8_lossy(&ok.body));
    assert_eq!(jpeg_size(&ok.body).0, 64);
    for tampered in [
        signed.replace("w=64", "w=65"),
        signed.replace("/thumb", "/filmstrip"),
        signed.replace("&exp=", "&exp=9"),
        format!("{signed}0"),
    ] {
        let reply = s.get_anonymous(&tampered);
        assert_eq!(reply.status, 401, "{tampered}");
        assert_eq!(error_of(&reply)["code"], "bad_signature", "{tampered}");
    }
    // A wrong token is refused even on a signed URL.
    let both = s.request(
        "GET",
        &signed,
        &[("authorization", format!("Bearer {}", "0".repeat(64)))],
        None,
    );
    assert_eq!(both.status, 401);
    // Signed URLs are GET/HEAD only, and never sign.
    let post = s.request("POST", &signed, &[], None);
    assert_eq!(post.status, 401);
    let bad = s.request(
        "POST",
        "/media/sign",
        &[
            ("authorization", format!("Bearer {}", s.token)),
            ("content-type", "application/json".into()),
        ],
        Some(json!({ "paths": ["/rpc"] }).to_string()),
    );
    assert_eq!(bad.status, 422);
    assert_eq!(error_of(&bad)["param"], "paths/0");

    // An expired signature: 401 `expired_signature`.
    let short = s.sign(std::slice::from_ref(&path), Some(1)).remove(0);
    assert_eq!(s.get_anonymous(&short).status, 200);
    std::thread::sleep(Wall::from_millis(2100));
    let expired = s.get_anonymous(&short);
    assert_eq!(expired.status, 401);
    assert_eq!(error_of(&expired)["code"], "expired_signature");
}

#[test]
fn bad_parameters_are_422_unknown_media_404_and_stale_hashes_409() {
    let f = fixture("params", false, Wall::ZERO, 4);
    let s = &f.server;
    let wide = s.get(&format!("/media/{}/thumb?w=100000", f.media), &[]);
    assert_eq!(wide.status, 422);
    assert_eq!(
        wide.header("content-type").as_deref(),
        Some("application/json")
    );
    assert_eq!(error_of(&wide)["code"], "invalid_params");
    assert_eq!(error_of(&wide)["param"], "w");
    for (path, param) in [
        (format!("/media/{}/thumb?width=10", f.media), "width"),
        (format!("/media/{}/filmstrip?count=0", f.media), "count"),
        (format!("/media/{}/waveform?lod=99", f.media), "lod"),
        (format!("/media/{}/thumb?t=f:x", f.media), "t"),
        ("/media/frame?w=10".to_owned(), "t"),
        ("/media/frame?t=0&w=5000".to_owned(), "w"),
    ] {
        let reply = s.get(&path, &[]);
        assert_eq!(reply.status, 422, "{path}");
        assert_eq!(error_of(&reply)["param"], param, "{path}");
    }
    let unknown = s.get("/media/med_0190f5c2-0000-7000-8000-000000000000/thumb", &[]);
    assert_eq!(unknown.status, 404);
    assert_eq!(error_of(&unknown)["code"], "not_found");
    assert_eq!(s.get("/media/nonsense/thumb", &[]).status, 404);
    assert_eq!(
        s.get(
            "/media/frame?t=0&seq=seq_0190f5c2-0000-7000-8000-000000000000",
            &[]
        )
        .status,
        404
    );
    let stale = s.get(
        &format!(
            "/media/{}/thumb?hash=blake3-fast:{}",
            f.media,
            "0".repeat(64)
        ),
        &[],
    );
    assert_eq!(stale.status, 409);
    assert_eq!(error_of(&stale)["code"], "content_changed");
    // No work for any of them.
    assert_eq!(s.stats().produced, 0);
}

#[test]
fn an_engine_without_media_answers_503() {
    let dir = TempDir::new("nomedia");
    let s = Server::start(engine_support::headless(dir.path()), 4);
    let reply = s.get("/media/frame?t=0", &[]);
    assert_eq!(reply.status, 503);
    assert_eq!(error_of(&reply)["code"], "unavailable");
    // Signing works anyway (it needs no media).
    assert_eq!(s.sign(&["/media/frame?t=0".to_owned()], None).len(), 1);
}

#[test]
fn disconnecting_mid_request_cancels_the_decode_within_100_ms() {
    // 40 ms per decoded frame: a 30-tile filmstrip takes over a second.
    let f = fixture("cancel", false, Wall::from_millis(40), 4);
    let s = &f.server;
    let path = format!("/media/{}/filmstrip?w=64&count=30", f.media);
    // Best of a few (see `disconnect_best_of`): the budget is 100 ms, and a loaded machine can
    // deschedule the server thread for longer than that once.
    let budget = Wall::from_millis(100);
    let (stopped, tries) = s.disconnect_best_of(&path, Wall::from_millis(150), budget, 5);
    println!(
        "the decode stopped {:.1} ms after the disconnect ({tries} attempt(s))",
        stopped.as_secs_f64() * 1000.0
    );
    assert!(stopped < budget, "{stopped:?} after {tries} attempt(s)");
    assert_eq!(s.stats().cancelled, u64::from(tries));
    // The cancelled strip was not stored: asking again produces it in full.
    let decoded = f.fake.decoded_frames();
    let full = s.get(&path, &[]);
    assert_eq!(full.status, 200);
    assert!(f.fake.decoded_frames() > decoded + 20);

    // A thumbnail seek (decode forward through a GOP) is cancelled too.
    let thumb = format!("/media/{}/thumb?t=3s&w=64", f.media);
    let (stopped, tries) = s.disconnect_best_of(&thumb, Wall::from_millis(100), budget, 5);
    assert!(stopped < budget, "{stopped:?} after {tries} attempt(s)");
}

#[test]
fn the_semaphore_bounds_the_work_in_flight() {
    let f = fixture("slots", false, Wall::from_millis(10), 1);
    let s = &f.server;
    let max = Arc::new(AtomicU64::new(0));
    let done = Arc::new(AtomicBool::new(false));
    let watcher = {
        let (stats, max, done) = (Arc::clone(&s.stats), Arc::clone(&max), Arc::clone(&done));
        std::thread::spawn(move || {
            while !done.load(Ordering::SeqCst) {
                max.fetch_max(stats.snapshot().in_flight, Ordering::SeqCst);
                std::thread::yield_now();
            }
        })
    };
    let replies = s.rt.block_on(async {
        let mut tasks = Vec::new();
        for t in [0, 1, 2, 3] {
            let url = s.url(&format!("/media/{}/thumb?t={t}s&w=32", f.media));
            let req = s.http.get(url).bearer_auth(&s.token);
            tasks.push(tokio::spawn(
                async move { req.send().await.unwrap().status() },
            ));
        }
        let mut out = Vec::new();
        for task in tasks {
            out.push(task.await.unwrap().as_u16());
        }
        out
    });
    done.store(true, Ordering::SeqCst);
    let _ = watcher.join();
    assert_eq!(replies, vec![200; 4]);
    assert_eq!(max.load(Ordering::SeqCst), 1);
}

#[test]
fn filmstrips_waveforms_and_frames() {
    let f = fixture("all", true, Wall::ZERO, 4);
    let s = &f.server;

    // The filmstrip the import analysis cached (100 tiles of 160 px), with its manifest.
    let strip = s.get(&format!("/media/{}/filmstrip", f.media), &[]);
    assert_eq!(strip.status, 200);
    let manifest: Value =
        serde_json::from_str(&strip.header("x-cutweave-filmstrip").unwrap()).unwrap();
    assert_eq!(
        (manifest["tile_w"].clone(), manifest["tile_h"].clone()),
        (json!(160), json!(90))
    );
    assert_eq!(manifest["count"], 100);
    let sprite = jpeg_size(&strip.body);
    assert_eq!(
        (json!(sprite.0), json!(sprite.1)),
        (manifest["sprite_w"].clone(), manifest["sprite_h"].clone())
    );
    let json_form = s.get(&format!("/media/{}/filmstrip?fmt=json", f.media), &[]);
    assert_eq!(
        json_form.header("content-type").as_deref(),
        Some("application/json")
    );
    assert_eq!(json_form.json(), manifest);
    assert_ne!(json_form.header("etag"), strip.header("etag"));
    // `h` narrows the tiles to keep the aspect: 45 px high → 80 px wide.
    let short = s.get(
        &format!("/media/{}/filmstrip?w=160&h=45&count=4", f.media),
        &[],
    );
    let short: Value =
        serde_json::from_str(&short.header("x-cutweave-filmstrip").unwrap()).unwrap();
    assert_eq!(
        (short["tile_w"].clone(), short["tile_h"].clone()),
        (json!(80), json!(45))
    );

    // Waveforms: the peak data of docs/reference/waveform-format.md, or a PNG.
    let peaks = s.get(&format!("/media/{}/waveform?lod=2", f.media), &[]);
    assert_eq!(
        peaks.status,
        200,
        "{}",
        String::from_utf8_lossy(&peaks.body)
    );
    assert_eq!(&peaks.body[..4], b"CWPD");
    assert_eq!(peaks.body[5], 2, "the level");
    let rate = u32::from_le_bytes(peaks.body[12..16].try_into().unwrap());
    let per_bin = u32::from_le_bytes(peaks.body[16..20].try_into().unwrap());
    let bins = u32::from_le_bytes(peaks.body[28..32].try_into().unwrap());
    assert_eq!((rate, per_bin), (48_000, 256 * 16));
    // 4 s at 48 kHz in bins of 4096 samples.
    assert_eq!(bins, (4 * 48_000_u32).div_ceil(4096));
    assert_eq!(peaks.body.len(), 32 + 4 * bins as usize);
    let tag = peaks.header("etag").unwrap();
    assert!(tag.starts_with(&format!("\"{}-", f.hash)), "{tag}");
    let again = s.get(
        &format!("/media/{}/waveform?lod=2", f.media),
        &[("if-none-match", tag.as_str())],
    );
    assert_eq!(again.status, 304);
    let png = s.get(
        &format!("/media/{}/waveform?fmt=png&w=200&h=40", f.media),
        &[],
    );
    assert_eq!(png.header("content-type").as_deref(), Some("image/png"));
    assert!(is_png(&png.body));
    let deep = s.get(&format!("/media/{}/waveform?lod=11", f.media), &[]);
    assert_eq!(deep.status, 422);
    assert_eq!(error_of(&deep)["param"], "lod");

    // `/media/frame` is `render.frame`, byte for byte (PNG and JPEG).
    for (fmt, params) in [
        (
            "png",
            json!({ "t": "1s", "max_size": 160, "format": "png" }),
        ),
        ("jpeg", json!({ "t": "1s", "max_size": 160 })),
    ] {
        let reply = s.get(&format!("/media/frame?t=1s&w=160&fmt={fmt}"), &[]);
        assert_eq!(
            reply.status,
            200,
            "{}",
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(reply.header("cache-control").as_deref(), Some("no-store"));
        assert!(reply.header("x-cutweave-rev").is_some());
        let query = call(&f.conn, "render.frame", params).unwrap();
        assert_eq!(
            reply.body,
            base64_decode(query["data"].as_str().unwrap()),
            "{fmt}"
        );
    }
    let boxed = s.get("/media/frame?t=0&w=320&h=90", &[]);
    assert_eq!(jpeg_size(&boxed.body), (160, 90));
}

#[test]
fn rotated_media_give_upright_thumbnails_and_filmstrips() {
    // A 320×180 clip the probe says to show turned a quarter: a 180×320 picture.
    let f = fixture_turned("turned", true, Wall::ZERO, 4, 90);
    let s = &f.server;
    // Entries in the thumbnail cache (the preview decodes too, so decodes are not counted).
    let entries = || {
        let status = call(&f.conn, "media.cache_status", json!({})).unwrap();
        let kinds = status["kinds"].as_array().unwrap();
        let thumbs = kinds.iter().find(|k| k["kind"] == "thumbnails").unwrap();
        thumbs["entries"].as_u64().unwrap()
    };
    assert_eq!(entries(), 2, "the poster and the filmstrip of the import");

    // The poster and the filmstrip of the import analysis are the upright ones.
    let poster = s.get(&format!("/media/{}/thumb", f.media), &[]);
    assert_eq!(
        poster.status,
        200,
        "{}",
        String::from_utf8_lossy(&poster.body)
    );
    // Never upscaled: 320 px asked for, the upright picture is 180 wide.
    assert_eq!(jpeg_size(&poster.body), (180, 320));
    let strip = s.get(&format!("/media/{}/filmstrip", f.media), &[]);
    assert_eq!(strip.status, 200);
    let manifest: Value =
        serde_json::from_str(&strip.header("x-cutweave-filmstrip").unwrap()).unwrap();
    assert_eq!(
        (manifest["tile_w"].clone(), manifest["tile_h"].clone()),
        (json!(160), json!(284))
    );
    let sprite = jpeg_size(&strip.body);
    assert_eq!(
        (json!(sprite.0), json!(sprite.1)),
        (manifest["sprite_w"].clone(), manifest["sprite_h"].clone())
    );
    assert_eq!(entries(), 2, "both were made by the import analysis");

    // A width is the upright picture's, and `h` narrows the tiles by its aspect.
    let thumb = s.get(&format!("/media/{}/thumb?t=1000&w=90", f.media), &[]);
    assert_eq!(jpeg_size(&thumb.body), (90, 160));
    let short = s.get(
        &format!("/media/{}/filmstrip?w=160&h=160&count=4", f.media),
        &[],
    );
    let short: Value =
        serde_json::from_str(&short.header("x-cutweave-filmstrip").unwrap()).unwrap();
    assert_eq!(
        (short["tile_w"].clone(), short["tile_h"].clone()),
        (json!(90), json!(160))
    );
}

#[test]
fn a_waveform_before_its_analysis_is_not_ready() {
    let f = fixture("notready", false, Wall::ZERO, 4);
    let reply = f.server.get(&format!("/media/{}/waveform", f.media), &[]);
    assert_eq!(reply.status, 404);
    assert_eq!(error_of(&reply)["code"], "not_ready");
    // Errors are never cached.
    assert_eq!(reply.header("cache-control").as_deref(), Some("no-store"));
}
