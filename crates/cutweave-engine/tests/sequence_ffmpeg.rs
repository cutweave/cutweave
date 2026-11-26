// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2F.05c acceptance on the full engine with the FFmpeg backend (ADR-0035; feature `ffmpeg`,
//! needs the bundle, docs/dev/ffmpeg.md): a 300-frame PNG sequence
//!
//! - imports with `media.import_sequence` in under 3 s (one picture is probed, the frame index
//!   is computed), as one asset of 10 s at the sequence's 30 fps;
//! - shows exactly its own picture at every frame of the timeline (`render.frame` on the
//!   30 fps grid reads the frame number burnt into each picture);
//! - plays at 30 fps on the manual clock without a dropped frame;
//! - is one undo step, and comes back with redo;
//! - exports like video: the file has 300 frames, each still its own picture.
//!
//! The pictures are written at test time: no corpus, no network.

#![cfg(feature = "ffmpeg")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_truncation
)]

mod engine_support;
mod full_support;

use std::path::{Path, PathBuf};
use std::time::Instant;

use cutweave_api::Credential;
use cutweave_engine::{Connection, Engine, EngineMode, FullConfig, FullHandle, FullSubsystems};
use cutweave_media::testing::stills::{self, png_color};
use cutweave_media::{DecodeMode, DecodeOpts, FfmpegBackend, MediaBackend, PixelFormat};
use engine_support::TempDir;
use full_support::{Clocks, FRAME_30, SEC, call, delivered, engine_config, play_frames};
use serde_json::{Value, json};

const FRAMES: u32 = 300;
/// The pictures: 64 × 36, the frame number as two colours (left half, right half).
const W: u32 = 64;
const H: u32 = 36;
/// Six levels per channel, 44 apart: a colour survives the compositor and H.264.
const LEVELS: [u8; 6] = [20, 64, 108, 152, 196, 240];

fn colour_of(value: u32) -> [u8; 3] {
    [value % 6, value / 6 % 6, value / 36 % 6].map(|d| LEVELS[d as usize])
}

fn value_of(rgb: [u8; 3]) -> u32 {
    let digit = |v: u8| {
        (0..6_u32)
            .min_by_key(|&i| LEVELS[i as usize].abs_diff(v))
            .unwrap()
    };
    digit(rgb[0]) + 6 * digit(rgb[1]) + 36 * digit(rgb[2])
}

/// The picture of `number`: its low part on the left half, its high part on the right.
fn burnt(number: u32) -> Vec<u8> {
    let (left, right) = (colour_of(number % 216), colour_of(number / 216));
    let mut samples = Vec::with_capacity((W * H * 3) as usize);
    for _ in 0..H {
        for x in 0..W {
            samples.extend_from_slice(if x < W / 2 { &left } else { &right });
        }
    }
    stills::png(W, H, 8, png_color::RGB, &samples, &[])
}

/// The number read from the middles of the two halves of a picture.
fn number_in(left: [u8; 3], right: [u8; 3]) -> u32 {
    value_of(left) + 216 * value_of(right)
}

fn engine(dir: &Path, clocks: &Clocks) -> (Engine, FullHandle) {
    let config = FullConfig {
        cache_dir: Some(dir.join("cache")),
        audio: clocks.mode(),
        headless_preview: (320, 180),
        ..FullConfig::native()
    };
    assert!(config.media.is_some(), "{:?}", config.media_problem);
    let full = FullSubsystems::new(config);
    let handle = full.handle();
    let config = engine_config(dir, EngineMode::Headless, Box::new(full));
    (Engine::start(config).unwrap(), handle)
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
            json!({ "job_id": job, "timeout_ms": 120_000 }),
        )
        .unwrap();
        assert_eq!(info["state"], "succeeded", "{info}");
    }
}

fn unbase64(s: &str) -> Vec<u8> {
    let val = |c: u8| -> u32 {
        match c {
            b'A'..=b'Z' => u32::from(c - b'A'),
            b'a'..=b'z' => u32::from(c - b'a') + 26,
            b'0'..=b'9' => u32::from(c - b'0') + 52,
            b'+' => 62,
            b'/' => 63,
            _ => panic!("not base64: {c}"),
        }
    };
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    for chunk in s.as_bytes().chunks(4) {
        let pad = chunk.iter().rev().take_while(|&&c| c == b'=').count();
        let mut n = 0u32;
        for &c in &chunk[..4 - pad] {
            n = (n << 6) | val(c);
        }
        n <<= 6 * u32::try_from(pad).unwrap();
        out.extend_from_slice(&n.to_be_bytes()[1..4 - pad]);
    }
    out
}

/// The number burnt into the frame `render.frame` shows at `t` (320 × 180, nothing missing).
fn rendered_number(conn: &Connection, t: i64) -> u32 {
    let out = call(
        conn,
        "render.frame",
        json!({ "t": t, "max_size": 320, "format": "png" }),
    )
    .unwrap();
    assert_eq!(out["notes"], json!([]), "t {t}: {}", out["notes"]);
    let bytes = unbase64(out["data"].as_str().unwrap());
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
        .read_info()
        .unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    assert_eq!((info.width, info.height), (320, 180));
    let channels = match info.color_type {
        png::ColorType::Rgba => 4,
        png::ColorType::Rgb => 3,
        other => panic!("unexpected PNG colour type {other:?}"),
    };
    let at = |x: usize, y: usize| {
        let i = (y * 320 + x) * channels;
        [buf[i], buf[i + 1], buf[i + 2]]
    };
    number_in(at(80, 90), at(240, 90))
}

/// The numbers burnt into every frame of the video file at `path`.
fn numbers_in_file(path: &Path) -> Vec<u32> {
    let backend = FfmpegBackend::new().unwrap();
    let opts = DecodeOpts::default()
        .with_target(PixelFormat::Rgba8)
        .with_mode(DecodeMode::Sequential);
    let mut decoder = backend.open_decoder(path, &opts).unwrap();
    let mut numbers = Vec::new();
    while let Some(frame) = decoder.next_frame().unwrap() {
        let plane = &frame.planes[0];
        let at = |x: u32, y: u32| {
            let i = y as usize * plane.stride as usize + x as usize * 4;
            [plane.data[i], plane.data[i + 1], plane.data[i + 2]]
        };
        let (w, h) = (frame.width, frame.height);
        numbers.push(number_in(at(w / 4, h / 2), at(3 * w / 4, h / 2)));
    }
    numbers
}

fn media_total(conn: &Connection) -> Value {
    call(conn, "media.list", json!({})).unwrap()["total"].clone()
}

#[test]
fn a_300_frame_png_sequence_imports_fast_plays_exact_and_exports() {
    let t = TempDir::new("image-sequence");
    let clocks = Clocks::new();
    let (engine, handle) = engine(t.path(), &clocks);
    let conn = agent(&engine);
    let project = call(&conn, "project.new", json!({ "name": "Sequence" })).unwrap();

    let render = t.path().join("render");
    std::fs::create_dir_all(&render).unwrap();
    for number in 1..=FRAMES {
        std::fs::write(render.join(format!("shot_{number:04}.png")), burnt(number)).unwrap();
    }
    let one_of_them: PathBuf = render.join("shot_0150.png");

    // Import: probe only. One picture is read, whatever the length of the run.
    let started = Instant::now();
    let imported = call(
        &conn,
        "media.import_sequence",
        json!({ "path": one_of_them.to_string_lossy() }),
    )
    .unwrap();
    let took = started.elapsed();
    println!("media.import_sequence of {FRAMES} pictures: {took:?}");
    assert!(took.as_secs_f64() < 3.0, "the import took {took:?}");
    assert_eq!(imported["kind"], "image_sequence", "{imported}");
    assert_eq!(imported["duplicate"], false);
    assert_eq!(imported["warnings"], json!([]));
    assert_eq!(
        imported["image_sequence"],
        json!({"pattern": "shot_%04d.png", "first": 1, "count": FRAMES,
               "fps": {"num": 30, "den": 1}, "missing": []}),
        "the run gets the frame rate of the sequence the editor shows"
    );
    wait_jobs(&conn, &imported["job_ids"]);
    let media = imported["media_id"].clone();
    assert_eq!(media_total(&conn), json!(1), "one asset, not {FRAMES}");
    {
        let snapshot = engine.handle().snapshot().unwrap();
        let asset = snapshot.doc.media.values().next().unwrap();
        assert_eq!(asset.probe.duration.map(|d| d.flicks()), Some(10 * SEC));
        assert_eq!(
            asset.path_abs.as_deref(),
            Some(render.join("shot_0001.png").to_string_lossy().as_ref())
        );
        let picture = &asset.probe.streams[0];
        assert_eq!((picture.width, picture.height), (Some(W), Some(H)));
        assert!(asset.probe.audio.is_none());
    }

    // On the timeline it is video: its whole length, no linked sound.
    let inserted = call(
        &conn,
        "timeline.insert",
        json!({
            "sequence": project["sequence"], "track": project["video_track"],
            "media": media, "at": 0,
        }),
    )
    .unwrap();
    assert_eq!(inserted["duration"], json!(10 * SEC), "{inserted}");
    assert_eq!(inserted["clips"].as_array().unwrap().len(), 1, "{inserted}");

    // Every frame is exact: frame n of the timeline shows picture 1 + n.
    let started = Instant::now();
    for n in 0..FRAMES {
        let t = i64::from(n) * FRAME_30;
        assert_eq!(rendered_number(&conn, t), 1 + n, "frame {n}");
    }
    // Out of order too, and a time inside a frame shows that frame.
    for n in [299_u32, 0, 150, 149, 151, 7] {
        let t = i64::from(n) * FRAME_30 + FRAME_30 / 2;
        assert_eq!(rendered_number(&conn, t), 1 + n, "frame {n}");
    }
    println!("render.frame of every frame: {:?}", started.elapsed());

    // It plays at 30 fps: every frame delivered on the manual clock, none dropped.
    call(&conn, "view.seek", json!({ "t": 0 })).unwrap();
    let before = delivered(&handle);
    let started = Instant::now();
    play_frames(&conn, &handle, &clocks, FRAMES - 1);
    println!("playback of {} frames: {:?}", FRAMES - 1, started.elapsed());
    let counters = handle.playback_stats().unwrap();
    assert!(
        counters.delivered >= before + u64::from(FRAMES - 1),
        "{counters:?}"
    );
    assert_eq!((counters.dropped, counters.errors), (0, 0), "{counters:?}");
    let paused = call(&conn, "view.pause", json!({})).unwrap();
    assert_eq!(paused["t"], json!(i64::from(FRAMES - 1) * FRAME_30));

    // One undo step: undoing the insert and then the import leaves no media; redo restores.
    call(&conn, "edit.undo", json!({})).unwrap();
    assert_eq!(media_total(&conn), json!(1));
    call(&conn, "edit.undo", json!({})).unwrap();
    assert_eq!(
        media_total(&conn),
        json!(0),
        "one undo removes the sequence"
    );
    call(&conn, "edit.redo", json!({})).unwrap();
    call(&conn, "edit.redo", json!({})).unwrap();
    assert_eq!(media_total(&conn), json!(1));

    // Export: the sequence is video. 300 frames, each still its own picture.
    let out = t.path().join("out.mp4");
    let export = call(
        &conn,
        "export.start",
        json!({
            "path": out.to_string_lossy(),
            "preset": { "custom": { "width": 320, "height": 180 } },
            "hw": "off",
        }),
    )
    .unwrap();
    wait_jobs(&conn, &json!([export["job_id"]]));
    let numbers = numbers_in_file(&out);
    let expected: Vec<u32> = (1..=FRAMES).collect();
    assert_eq!(numbers, expected, "the exported frames");

    drop(conn);
    engine.shutdown();
}

/// A run with gaps: the warnings name them and a missing frame shows the picture before it.
#[test]
fn a_missing_picture_shows_the_one_before_it() {
    let t = TempDir::new("image-sequence-gaps");
    let clocks = Clocks::new();
    let (engine, _handle) = engine(t.path(), &clocks);
    let conn = agent(&engine);
    let project = call(&conn, "project.new", json!({ "name": "Gaps" })).unwrap();
    let render = t.path().join("render");
    std::fs::create_dir_all(&render).unwrap();
    for number in (98..128_u32).filter(|n| ![103, 104, 120].contains(n)) {
        std::fs::write(render.join(format!("f {number}.png")), burnt(number)).unwrap();
    }
    // The folder stands for its one run; 24 fps is asked for.
    let imported = call(
        &conn,
        "media.import_sequence",
        json!({ "path": render.to_string_lossy(), "fps": {"num": 24, "den": 1} }),
    )
    .unwrap();
    assert_eq!(imported["image_sequence"]["pattern"], "f %d.png");
    assert_eq!(
        imported["image_sequence"]["missing"],
        json!([103, 104, 120])
    );
    assert_eq!(
        imported["image_sequence"]["fps"],
        json!({"num": 24, "den": 1})
    );
    assert_eq!(
        imported["warnings"],
        json!([
            "Pictures 103 to 104 are missing: the picture before shows instead.",
            "Picture 120 is missing: the picture before shows instead."
        ])
    );
    wait_jobs(&conn, &imported["job_ids"]);
    call(
        &conn,
        "timeline.insert",
        json!({
            "sequence": project["sequence"], "track": project["video_track"],
            "media": imported["media_id"], "at": 0, "duration": "1s",
        }),
    )
    .unwrap();
    // 24 fps media on the 30 fps timeline: sample in the middle of each source frame.
    let frame_24 = SEC / 24;
    for (source_frame, number) in [
        (0_i64, 98_u32),
        (4, 102),
        (5, 102),
        (6, 102),
        (7, 105),
        (22, 119),
        (23, 121),
    ] {
        let t = source_frame * frame_24 + frame_24 / 2;
        assert_eq!(
            rendered_number(&conn, t),
            number,
            "source frame {source_frame}"
        );
    }
    // The same run again is the same asset.
    let again = call(
        &conn,
        "media.import_sequence",
        json!({ "path": render.join("f 110.png").to_string_lossy(), "fps": {"num": 24, "den": 1} }),
    )
    .unwrap();
    assert_eq!(again["duplicate"], true);
    assert_eq!(again["media_id"], imported["media_id"]);
    drop(conn);
    engine.shutdown();
}
