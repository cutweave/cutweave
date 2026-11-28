// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2I.02: `media.filmstrip`, `audio.waveform` and `render.contact_sheet` on a full engine
//! with `FakeBackend` media (software GPU, null audio; no FFmpeg), through the engine's
//! dispatch, the path RPC, MCP and the CLI share:
//!
//! - a filmstrip is one JPEG within `max_size`, its tiles at even times; the whole clip's strip
//!   names the `/media/{id}/filmstrip` route, which serves the same bytes under the ETag the
//!   result gives; a part of a clip is a strip of its own;
//! - the waveform of a 440 Hz sine at -6 dBFS peaks at the expected pixel rows (within 1 px),
//!   reports -6 dBFS and no silence, and names the `/media/{id}/waveform` route;
//! - the mix of a sequence with a 2 s gap reports that silence within 50 ms, and the loudness
//!   on request; one clip is drawn over its own extent;
//! - a contact sheet of the sequence is a JPEG with the items of each cell, by name.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod engine_support;
mod full_support;
mod media_support;

use std::sync::Arc;

use cutweave_engine::{AudioMode, Connection, EngineMode, FullConfig, FullSubsystems};
use cutweave_media::testing::{FakeAudio, FakeBackend, FakeMedia, FakeVideo};
use cutweave_model::{Duration, FLICKS_PER_SECOND, FrameRate, SampleRate};
use cutweave_render::wave::{WAVE, row_of};
use engine_support::TempDir;
use full_support::{call, engine_config, full_config};
use media_support::{Server, base64_decode, is_png, jpeg_size};
use serde_json::{Value, json};

/// A full engine with a 4 s clip (320x180 pictures, a mono 440 Hz sine at -6 dBFS) placed
/// twice on the timeline, 0 s..3 s and 5 s..8 s: the mix is silent from 3 s to 5 s.
struct Fixture {
    server: Server,
    conn: Connection,
    media: String,
    sequence: Value,
    /// The audio clip of the first placement.
    sound: Value,
    _dir: TempDir,
}

fn fixture(tag: &str) -> Fixture {
    let dir = TempDir::new(tag);
    let clip = dir.path().join("tone.mp4");
    std::fs::write(&clip, format!("fake media {tag}")).unwrap();
    let four = Duration::from_secs(4).unwrap();
    let fake = FakeBackend::new().with_media(
        &clip,
        FakeMedia {
            video: Some(FakeVideo::lasting(320, 180, FrameRate::FPS_30, four)),
            audio: Some(FakeAudio::tone(SampleRate::HZ_48_000, 1, four)),
        },
    );
    let config = FullConfig {
        analysis: true,
        ..full_config(dir.path(), AudioMode::FromEngine)
    }
    .with_media(Arc::new(fake));
    let engine = engine_config(
        dir.path(),
        EngineMode::Headless,
        Box::new(FullSubsystems::new(config)),
    );
    let server = Server::start(engine, 4);
    let conn = server.agent();
    let project = call(&conn, "project.new", json!({ "name": "Visual" })).unwrap();
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
    let mut sound = Value::Null;
    for at in ["0s", "5s"] {
        let placed = call(
            &conn,
            "timeline.insert",
            json!({
                "sequence": project["sequence"], "track": project["video_track"],
                "media": media, "at": at, "duration": "3s",
            }),
        )
        .unwrap();
        if sound.is_null() {
            sound = placed["clips"][1].clone();
        }
    }
    assert!(sound.is_string(), "a linked audio clip");
    Fixture {
        server,
        conn,
        media,
        sequence: project["sequence"].clone(),
        sound,
        _dir: dir,
    }
}

fn secs(s: f64) -> i64 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    let flicks = (s * FLICKS_PER_SECOND as f64).round() as i64;
    flicks
}

fn picture_bytes(out: &Value) -> Vec<u8> {
    let bytes = base64_decode(out["data"].as_str().unwrap());
    assert_eq!(out["bytes"], json!(bytes.len()));
    bytes
}

/// RGBA pixels of a PNG.
fn decode_png(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
        .read_info()
        .unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    let data = match info.color_type {
        png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
        png::ColorType::Rgb => buf[..info.buffer_size()]
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        other => panic!("unexpected PNG colour type {other:?}"),
    };
    (info.width, info.height, data)
}

#[test]
fn a_filmstrip_is_one_jpeg_within_max_size_and_links_the_media_route() {
    let f = fixture("strip");
    let out = call(&f.conn, "media.filmstrip", json!({ "media": f.media })).unwrap();
    assert_eq!(out["mime"], "image/jpeg");
    // 8 tiles of a 16:9 picture in 768 px: 96 x 54 each.
    assert_eq!(
        (out["width"].clone(), out["height"].clone()),
        (json!(768), json!(54))
    );
    assert_eq!(out["tile_width"], 96);
    let jpeg = picture_bytes(&out);
    assert_eq!(jpeg_size(&jpeg), (768, 54));
    let tiles = out["tiles"].as_array().unwrap();
    assert_eq!(tiles.len(), 8);
    for (i, tile) in (0i64..).zip(tiles) {
        assert_eq!(tile["t"], json!(i * secs(0.5)), "tile {i}");
        assert_eq!(tile["x"], json!(i * 96));
    }
    assert_eq!(
        (out["from"].clone(), out["to"].clone()),
        (json!(0), json!(secs(4.0)))
    );

    // The whole clip's strip is what `/media/{id}/filmstrip` serves, under the same ETag.
    let link = out["link"].as_str().unwrap();
    assert!(
        link.starts_with(&format!("/media/{}/filmstrip?", f.media)),
        "{link}"
    );
    let served = f.server.get(link, &[]);
    assert_eq!(served.status, 200, "{link}");
    assert_eq!(served.body, jpeg);
    let etag = format!("\"{}\"", out["etag"].as_str().unwrap());
    assert_eq!(served.header("etag").as_deref(), Some(etag.as_str()));
    assert!(
        served
            .header("cache-control")
            .unwrap()
            .contains("immutable")
    );
    let unchanged = f.server.get(link, &[("if-none-match", etag.as_str())]);
    assert_eq!(unchanged.status, 304);

    // The same request again: the same answer, from the cache.
    let again = call(&f.conn, "media.filmstrip", json!({ "media": f.media })).unwrap();
    assert_eq!(again, out);

    // A part of the clip: its own strip, with no route of its own.
    let part = call(
        &f.conn,
        "media.filmstrip",
        json!({ "media": f.media, "from": "1s", "to": "3s", "count": 4, "max_size": 128 }),
    )
    .unwrap();
    assert_eq!(
        (part["width"].clone(), part["height"].clone()),
        (json!(128), json!(18))
    );
    assert_eq!(jpeg_size(&picture_bytes(&part)), (128, 18));
    let shown: Vec<i64> = part["tiles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["t"].as_i64().unwrap())
        .collect();
    assert_eq!(shown, [secs(1.0), secs(1.5), secs(2.0), secs(2.5)]);
    assert!(part.get("link").is_none());
    assert_ne!(part["etag"], out["etag"]);

    // `max_size` is honoured across its range, whatever the tile count; tiles never upscale.
    for (max_size, count) in [
        (64u32, 1u32),
        (64, 8),
        (100, 3),
        (333, 5),
        (1024, 2),
        (2048, 64),
    ] {
        let strip = call(
            &f.conn,
            "media.filmstrip",
            json!({ "media": f.media, "max_size": max_size, "count": count }),
        )
        .unwrap();
        let (w, h) = jpeg_size(&picture_bytes(&strip));
        assert!(
            w <= max_size && h <= max_size,
            "{max_size}/{count}: {w}x{h}"
        );
        assert_eq!(
            (json!(w), json!(h)),
            (strip["width"].clone(), strip["height"].clone())
        );
        assert!(strip["tile_width"].as_u64().unwrap() <= 320);
        assert_eq!(
            strip["tiles"].as_array().unwrap().len(),
            count as usize,
            "{count}"
        );
    }

    let e = call(
        &f.conn,
        "media.filmstrip",
        json!({ "media": "med_00000000000000000000000000000001" }),
    )
    .unwrap_err();
    assert_eq!(e.code(), "not_found");
    let e = call(
        &f.conn,
        "media.filmstrip",
        json!({ "media": f.media, "count": 64, "max_size": 64 }),
    )
    .unwrap_err();
    assert_eq!(e.code(), "invalid");
}

/// The first and last row of column `x` of `lane` drawn in the waveform colour.
fn lit_rows(pixels: &(u32, u32, Vec<u8>), lane: &Value, x: u32) -> Option<(u32, u32)> {
    let (width, _, data) = pixels;
    let (top, height) = (
        u32::try_from(lane["y"].as_u64().unwrap()).unwrap(),
        u32::try_from(lane["height"].as_u64().unwrap()).unwrap(),
    );
    let lit: Vec<u32> = (top..top + height)
        .filter(|y| {
            let at = ((y * width + x) * 4) as usize;
            data[at..at + 4] == WAVE
        })
        .collect();
    Some((*lit.first()? - top, *lit.last()? - top))
}

/// WP-2I.02 acceptance: the waveform of a 440 Hz sine at -6 dBFS peaks at the expected pixel
/// rows, within 1 px, on a linear and on a dB scale.
#[test]
fn the_waveform_of_a_sine_peaks_at_the_expected_rows() {
    let f = fixture("wave-media");
    let target = json!({ "kind": "media", "media": f.media });
    for scale_db in [None, Some(60.0)] {
        let mut params = json!({ "target": target, "width": 600, "height": 214 });
        if let Some(db) = scale_db {
            params["scale_db"] = json!(db);
        }
        let out = call(&f.conn, "audio.waveform", params).unwrap();
        assert_eq!(out["mime"], "image/png");
        assert_eq!(
            (out["width"].clone(), out["height"].clone()),
            (json!(600), json!(214))
        );
        assert_eq!(out["sample_rate"], 48_000);
        assert_eq!(
            (out["from"].clone(), out["to"].clone()),
            (json!(0), json!(secs(4.0)))
        );
        let png = picture_bytes(&out);
        assert!(is_png(&png));
        let pixels = decode_png(&png);
        assert_eq!((pixels.0, pixels.1), (600, 214));
        let lanes = out["lanes"].as_array().unwrap();
        assert_eq!(lanes.len(), 1);
        let lane = &lanes[0];
        assert_eq!(lane["channel"], "mix");
        assert_eq!(
            (lane["x"].clone(), lane["width"].clone()),
            (json!(0), json!(600))
        );
        let height = u32::try_from(lane["height"].as_u64().unwrap()).unwrap();
        assert_eq!(height, 200, "214 px less the ruler");
        let (top, bottom) = (
            row_of(0.5, height, scale_db),
            row_of(-0.5, height, scale_db),
        );
        if scale_db.is_none() {
            assert_eq!(
                (top, bottom),
                (50, 149),
                "a quarter and three quarters of the lane"
            );
        }
        for x in 0..600 {
            let (first, last) = lit_rows(&pixels, lane, x).expect("every column has sound");
            assert!(
                first.abs_diff(top) <= 1,
                "column {x}: top row {first}, expected {top}"
            );
            assert!(
                last.abs_diff(bottom) <= 1,
                "column {x}: bottom row {last}, expected {bottom}"
            );
        }

        let levels = &out["audio_levels"];
        assert!(
            (levels["peak_db"].as_f64().unwrap() + 6.02).abs() < 0.05,
            "{levels}"
        );
        assert_eq!(levels["peaks_db"].as_array().unwrap().len(), 64);
        assert_eq!(levels["slice"], json!(secs(4.0) / 64));
        assert_eq!(levels["silences"], json!([]));
        assert_eq!(levels["silence_below_db"], json!(-60.0));
        assert!(levels.get("lufs_integrated").is_none());
        assert_eq!(out.get("scale_db").and_then(Value::as_f64), scale_db);

        // The same peaks over HTTP: a PNG of the same size, cacheable by content.
        let link = out["link"].as_str().unwrap();
        assert!(
            link.starts_with(&format!("/media/{}/waveform?fmt=png", f.media)),
            "{link}"
        );
        let served = f.server.get(link, &[]);
        assert_eq!(served.status, 200, "{link}");
        assert!(is_png(&served.body));
        assert!(served.header("etag").is_some());
        assert!(
            served
                .header("cache-control")
                .unwrap()
                .contains("immutable")
        );
    }

    // A part of the file, and the errors of a picture that cannot hold its lanes.
    let part = call(
        &f.conn,
        "audio.waveform",
        json!({ "target": target, "from": "1s", "to": "1.5s", "width": 64, "height": 32 }),
    )
    .unwrap();
    assert_eq!(
        (part["from"].clone(), part["to"].clone()),
        (json!(secs(1.0)), json!(secs(1.5)))
    );
    let e = call(
        &f.conn,
        "audio.waveform",
        json!({ "target": target, "loudness": true }),
    )
    .unwrap_err();
    assert_eq!(e.code(), "invalid");
}

/// WP-2I.02 acceptance: a known 2 s silence is annotated within 50 ms.
#[test]
fn the_mix_of_a_sequence_reports_its_two_second_gap_and_the_loudness() {
    let f = fixture("wave-mix");
    let out = call(
        &f.conn,
        "audio.waveform",
        json!({
            "target": { "kind": "range", "sequence": f.sequence }, "loudness": true,
            "channels": "split", "width": 800
        }),
    )
    .unwrap();
    assert_eq!(
        (out["from"].clone(), out["to"].clone()),
        (json!(0), json!(secs(8.0)))
    );
    assert_eq!(out["sequence"], f.sequence);
    assert!(out.get("link").is_none());
    assert_eq!(out["source_errors"], 0);
    let lanes = out["lanes"].as_array().unwrap();
    let names: Vec<&str> = lanes
        .iter()
        .map(|l| l["channel"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["left", "right"]);
    let pixels = decode_png(&picture_bytes(&out));
    // Sound at 1 s and at 6 s, nothing at 4 s (800 px for 8 s).
    assert!(lit_rows(&pixels, &lanes[0], 100).is_some());
    assert!(lit_rows(&pixels, &lanes[1], 600).is_some());
    assert!(lit_rows(&pixels, &lanes[0], 400).is_none());

    let levels = &out["audio_levels"];
    let silences = levels["silences"].as_array().unwrap();
    assert_eq!(silences.len(), 1, "{levels}");
    let fifty_ms = FLICKS_PER_SECOND / 20;
    let (from, to) = (
        silences[0]["from"].as_i64().unwrap(),
        silences[0]["to"].as_i64().unwrap(),
    );
    assert!((from - secs(3.0)).abs() <= fifty_ms, "silence from {from}");
    assert!((to - secs(5.0)).abs() <= fifty_ms, "silence to {to}");
    let peak = levels["peak_db"].as_f64().unwrap();
    assert!((-12.0..-1.0).contains(&peak), "peak {peak} dBFS");
    let slices = levels["peaks_db"].as_array().unwrap();
    assert!(slices[4].is_number() && slices[30].is_null() && slices[60].is_number());
    // Loudness: a reading every 100 ms; the tone is heard, the gap is quieter.
    let integrated = levels["lufs_integrated"].as_f64().unwrap();
    assert!((-30.0..-3.0).contains(&integrated), "{integrated} LUFS");
    let course = levels["lufs_short_term"].as_array().unwrap();
    assert_eq!(course.len(), 80);
    assert_eq!(course[79]["t"], json!(secs(8.0)));
    assert!(course[79]["lufs"].is_number(), "{}", course[79]);

    // One clip, over its own extent by default: sound all the way, no silence.
    let clip = call(
        &f.conn,
        "audio.waveform",
        json!({ "target": { "kind": "item", "item": f.sound } }),
    )
    .unwrap();
    assert_eq!(
        (clip["from"].clone(), clip["to"].clone()),
        (json!(0), json!(secs(3.0)))
    );
    assert_eq!(clip["audio_levels"]["silences"], json!([]));
    assert_eq!(clip["lanes"].as_array().unwrap().len(), 1);
    let pixels = decode_png(&picture_bytes(&clip));
    assert!(lit_rows(&pixels, &clip["lanes"][0], 400).is_some());
    let e = call(
        &f.conn,
        "audio.waveform",
        json!({ "target": { "kind": "item", "item": f.sound }, "to": "4s" }),
    )
    .unwrap_err();
    assert_eq!(e.code(), "invalid");
}

#[test]
fn a_contact_sheet_of_the_sequence_names_the_items_of_each_cell() {
    let f = fixture("sheet");
    call(
        &f.conn,
        "timeline.add_marker",
        json!({ "sequence": f.sequence, "t": "6s", "label": "second take" }),
    )
    .unwrap();
    let out = call(&f.conn, "render.contact_sheet", json!({ "count": 4 })).unwrap();
    assert_eq!(out["mime"], "image/jpeg");
    let jpeg = picture_bytes(&out);
    let (w, h) = jpeg_size(&jpeg);
    assert_eq!(
        (json!(w), json!(h)),
        (out["width"].clone(), out["height"].clone())
    );
    assert!(w <= 768 && h <= 768);
    assert!(jpeg.len() <= 400_000);
    let frames = out["frames"].as_array().unwrap();
    let times: Vec<i64> = frames.iter().map(|c| c["t"].as_i64().unwrap()).collect();
    assert_eq!(times, [0, secs(2.0), secs(4.0), secs(6.0)]);
    // The clip is on screen at 0 s, 2 s and 6 s; 4 s is in the gap.
    let shown = &frames[0]["items_visible"][0];
    assert!(shown["item"].as_str().unwrap().starts_with("itm_"));
    assert!(shown["track"].as_str().unwrap().starts_with("trk_"));
    assert_eq!(shown["name"], "tone.mp4");
    assert_eq!(frames[2]["items_visible"], json!([]));

    // At the cuts and at the markers.
    let cuts = call(&f.conn, "render.contact_sheet", json!({ "at": "cuts" })).unwrap();
    let times: Vec<i64> = cuts["frames"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["t"].as_i64().unwrap())
        .collect();
    assert_eq!(times, [0, secs(5.0)]);
    let marked = call(&f.conn, "render.contact_sheet", json!({ "at": "markers" })).unwrap();
    assert_eq!(marked["frames"][0]["t"], json!(secs(6.0)));
    assert_eq!(marked["frames"].as_array().unwrap().len(), 1);
}
