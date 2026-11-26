// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2F.05d acceptance on the full engine with the FFmpeg backend (feature `ffmpeg`; needs the
//! bundle, docs/dev/ffmpeg.md): `media.import_images_as_slideshow` and `media.import_frame`
//! (ADR-0035 items 3 and 4).
//!
//! - Three pictures become three stills, one after the other, in one undo step.
//! - The frame the Preview shows is saved as `generated/frames/<media id>.png` with exactly the
//!   pixels `render.frame` gives at that time, at the full sequence size, and registered as a
//!   picture; one undo step removes the asset and leaves the file.
//! - The frame of a video asset is saved at the asset's own size.
//! - An untitled document is refused with a message that says to save first.
//!
//! The pictures are written at test time (`cutweave_media::testing::stills`); the video is made
//! with the bundle's `ffmpeg` (the test skips that part without it). No corpus.

#![cfg(feature = "ffmpeg")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_truncation
)]

mod engine_support;
mod full_support;

use std::path::{Path, PathBuf};
use std::process::Command;

use cutweave_api::Credential;
use cutweave_commands::CmdError;
use cutweave_engine::{Connection, Engine, EngineMode, FullConfig, FullSubsystems};
use cutweave_media::testing::stills::{self, png_color};
use engine_support::TempDir;
use full_support::{Clocks, SEC, call, engine_config, ffmpeg_tool};
use serde_json::{Value, json};

fn engine(dir: &Path, clocks: &Clocks) -> Engine {
    let config = FullConfig {
        cache_dir: Some(dir.join("cache")),
        audio: clocks.mode(),
        headless_preview: (320, 180),
        ..FullConfig::native()
    };
    assert!(config.media.is_some(), "{:?}", config.media_problem);
    let full = FullSubsystems::new(config);
    Engine::start(engine_config(dir, EngineMode::Headless, Box::new(full))).unwrap()
}

fn agent(engine: &Engine) -> Connection {
    engine
        .handle()
        .connect(&Credential::Agent("cli".into()))
        .unwrap()
}

/// A 64 × 36 PNG: the left half `left`, the right half `right`.
fn picture(dir: &Path, name: &str, left: [u8; 3], right: [u8; 3]) -> String {
    let (w, h) = (64_u32, 36_u32);
    let mut samples = Vec::with_capacity((w * h * 3) as usize);
    for _ in 0..h {
        for x in 0..w {
            samples.extend_from_slice(if x < w / 2 { &left } else { &right });
        }
    }
    let path = dir.join(name);
    std::fs::write(&path, stills::png(w, h, 8, png_color::RGB, &samples, &[])).unwrap();
    path.to_string_lossy().into_owned()
}

fn wait(conn: &Connection, out: &Value) {
    for job in out["job_ids"].as_array().unwrap() {
        let info = call(
            conn,
            "job.wait",
            json!({ "job_id": job, "timeout_ms": 60_000 }),
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

/// A decoded PNG: its size, colour type and samples.
#[derive(Debug, PartialEq, Eq)]
struct Decoded {
    width: u32,
    height: u32,
    channels: usize,
    samples: Vec<u8>,
}

impl Decoded {
    fn at(&self, x: u32, y: u32) -> [u8; 3] {
        let i = (y * self.width + x) as usize * self.channels;
        [self.samples[i], self.samples[i + 1], self.samples[i + 2]]
    }
}

fn decode(bytes: &[u8]) -> Decoded {
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
        .read_info()
        .unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    assert_eq!(info.bit_depth, png::BitDepth::Eight);
    let channels = match info.color_type {
        png::ColorType::Rgba => 4,
        png::ColorType::Rgb => 3,
        other => panic!("unexpected PNG colour type {other:?}"),
    };
    buf.truncate(info.buffer_size());
    Decoded {
        width: info.width,
        height: info.height,
        channels,
        samples: buf,
    }
}

/// `render.frame` at `t`, as large as the sequence (up to 4096 px).
fn render_frame(conn: &Connection, t: i64) -> Decoded {
    let out = call(
        conn,
        "render.frame",
        json!({ "t": t, "max_size": 4096, "format": "png" }),
    )
    .unwrap();
    assert_eq!(out["notes"], json!([]), "{}", out["notes"]);
    decode(&unbase64(out["data"].as_str().unwrap()))
}

fn frame_files(project: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(project.join("generated").join("frames")) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries.map(|e| e.unwrap().path()).collect();
    files.sort();
    files
}

/// Whether two colours are the same within the rounding of the renderer.
fn close(a: [u8; 3], b: [u8; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 3)
}

const RED: [u8; 3] = [200, 30, 30];
const GREEN: [u8; 3] = [30, 200, 30];
const BLUE: [u8; 3] = [30, 30, 200];
const WHITE: [u8; 3] = [240, 240, 240];

#[test]
#[allow(clippy::too_many_lines)] // one story: slideshow, frame, undo, redo, the same frame again
fn a_slideshow_and_a_frame_of_the_preview_are_one_undo_step_each() {
    let t = TempDir::new("frame-seq");
    let clocks = Clocks::new();
    let engine = engine(t.path(), &clocks);
    let conn = agent(&engine);
    let project = call(&conn, "project.new", json!({ "name": "Frames" })).unwrap();

    // An untitled document has no folder for the picture.
    let refused = call(
        &conn,
        "media.import_frame",
        json!({ "source": "sequence", "at": 0 }),
    )
    .unwrap_err();
    let CmdError::Invalid { msg, .. } = &refused else {
        panic!("expected Invalid, got {refused:?}");
    };
    assert!(msg.contains("save the project first"), "{msg}");

    let folder = t.path().join("Frames");
    call(
        &conn,
        "project.save_as",
        json!({ "path": folder.to_string_lossy() }),
    )
    .unwrap();

    // Three pictures, one second each, by name: a, b, c.
    let files = [
        picture(t.path(), "c.png", BLUE, WHITE),
        picture(t.path(), "a.png", RED, GREEN),
        picture(t.path(), "b.png", GREEN, BLUE),
    ];
    let show = call(
        &conn,
        "media.import_images_as_slideshow",
        json!({ "files": files, "per_image": "1s", "order": "name" }),
    )
    .unwrap();
    wait(&conn, &show);
    assert_eq!(show["failed"], json!([]), "{show}");
    assert_eq!(show["clips"].as_array().unwrap().len(), 3, "{show}");
    assert_eq!(show["track"], project["video_track"]);
    assert_eq!(show["at"], json!(0));
    assert_eq!(show["per_image"], json!(SEC));
    assert_eq!(show["end"], json!(3 * SEC));
    let snapshot = engine.handle().snapshot().unwrap();
    assert_eq!(snapshot.doc.media.len(), 3);
    let sequence = snapshot.doc.sequences.values().next().unwrap();
    let (width, height) = (sequence.format.width, sequence.format.height);
    drop(snapshot);

    // The frame at 1.5 s shows b.png (green | blue), fitted to the sequence.
    let at = SEC + SEC / 2;
    let out = call(
        &conn,
        "media.import_frame",
        json!({ "source": "sequence", "at": at }),
    )
    .unwrap();
    wait(&conn, &out);
    assert_eq!(out["duplicate"], false, "{out}");
    assert_eq!(out["time"], json!(at));
    assert_eq!(
        (out["width"].clone(), out["height"].clone()),
        (json!(width), json!(height)),
        "the full sequence size"
    );
    let saved = PathBuf::from(out["path"].as_str().unwrap());
    let media_id = out["media_id"].as_str().unwrap().to_owned();
    assert_eq!(
        saved,
        folder
            .join("generated")
            .join("frames")
            .join(format!("{media_id}.png")),
        "the file is named after the new asset"
    );
    // Exactly the pixels of `render.frame` at that time.
    let file = decode(&std::fs::read(&saved).unwrap());
    let rendered = render_frame(&conn, at);
    assert_eq!((file.width, file.height), (width, height));
    assert!(
        file == rendered,
        "the saved frame differs from render.frame"
    );
    assert!(close(file.at(width / 4, height / 2), GREEN));
    assert!(close(file.at(width * 3 / 4, height / 2), BLUE));

    // It is a picture in the bin, named after the sequence and the timecode.
    let listed = call(&conn, "media.list", json!({})).unwrap();
    assert_eq!(listed["total"], 4, "{listed}");
    let card = listed["media"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == out["media_id"])
        .unwrap();
    assert_eq!(card["kind"], "image", "{card}");
    assert_eq!(card["name"], out["name"]);
    assert_eq!(out["name"], "Sequence 1 00:00:01:15");

    // One undo removes the asset; the file stays.
    call(&conn, "edit.undo", json!({})).unwrap();
    assert_eq!(call(&conn, "media.list", json!({})).unwrap()["total"], 3);
    assert!(saved.is_file(), "undo keeps the file");
    // One more undo removes the whole slideshow: the clips and the three pictures.
    call(&conn, "edit.undo", json!({})).unwrap();
    let snapshot = engine.handle().snapshot().unwrap();
    assert!(snapshot.doc.media.is_empty());
    assert!(
        snapshot
            .doc
            .sequences
            .values()
            .all(|s| s.tracks.values().all(|t| t.items.is_empty())),
        "the slideshow is gone in one step"
    );
    drop(snapshot);

    // Redo both: the same asset comes back, pointing at the same file; nothing is written.
    call(&conn, "edit.redo", json!({})).unwrap();
    call(&conn, "edit.redo", json!({})).unwrap();
    let snapshot = engine.handle().snapshot().unwrap();
    let asset = snapshot
        .doc
        .media
        .values()
        .find(|m| m.id.to_string() == media_id)
        .expect("the frame is back");
    assert_eq!(asset.path_abs.as_deref(), saved.to_str());
    drop(snapshot);
    assert_eq!(frame_files(&folder), std::slice::from_ref(&saved));

    // The same frame again is the same picture: no second asset, and no second file is left.
    let again = call(
        &conn,
        "media.import_frame",
        json!({ "source": "sequence", "at": at, "name": "Again" }),
    )
    .unwrap();
    assert_eq!(again["duplicate"], true, "{again}");
    assert_eq!(again["media_id"], out["media_id"]);
    assert_eq!(again["path"], out["path"]);
    assert_eq!(frame_files(&folder), [saved]);

    // Another time is another picture, with the given name, in the given bin folder.
    let bin = call(&conn, "media.create_folder", json!({ "name": "Stills" })).unwrap();
    let other = call(
        &conn,
        "media.import_frame",
        json!({ "source": "sequence", "at": "f:6", "name": "Poster", "folder": bin["folder_id"] }),
    )
    .unwrap();
    wait(&conn, &other);
    assert_eq!(other["name"], "Poster");
    assert_eq!(frame_files(&folder).len(), 2);
    let file = decode(&std::fs::read(other["path"].as_str().unwrap()).unwrap());
    assert!(
        file == render_frame(&conn, SEC / 5),
        "the second saved frame differs from render.frame"
    );
    assert!(close(file.at(width / 4, height / 2), RED));

    // A time before zero is refused, and nothing is written for it.
    let refused = call(
        &conn,
        "media.import_frame",
        json!({ "source": "sequence", "at": "-1s" }),
    );
    assert!(
        matches!(refused, Err(CmdError::Invalid { .. })),
        "{refused:?}"
    );
    assert_eq!(frame_files(&folder).len(), 2);
    drop(conn);
    engine.shutdown();
}

/// Runs the bundle's `ffmpeg`.
fn ffmpeg(tool: &Path, args: &[&str]) {
    let out = Command::new(tool)
        .args(["-v", "error", "-nostdin", "-y"])
        .args(args)
        .output()
        .expect("run ffmpeg");
    assert!(
        out.status.success(),
        "ffmpeg {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_frame_of_a_video_asset_has_the_assets_own_size() {
    let Some(tool) = ffmpeg_tool("ffmpeg") else {
        eprintln!("skipping: no ffmpeg CLI (source scripts/ffmpeg/env.sh)");
        return;
    };
    let t = TempDir::new("frame-src");
    let clip = t.path().join("bars.mp4");
    // One second of 192 × 108 at 30 fps: the left half white, the right half black.
    ffmpeg(
        &tool,
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=192x108:r=30:d=1",
            "-vf",
            "geq=lum='if(lt(X\\,96)\\,235\\,16)':cb=128:cr=128",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            &clip.to_string_lossy(),
        ],
    );
    let clocks = Clocks::new();
    let engine = engine(t.path(), &clocks);
    let conn = agent(&engine);
    call(&conn, "project.new", json!({ "name": "Source" })).unwrap();
    let folder = t.path().join("Source");
    call(
        &conn,
        "project.save_as",
        json!({ "path": folder.to_string_lossy() }),
    )
    .unwrap();
    let imported = call(
        &conn,
        "media.import",
        json!({ "paths": [clip.to_string_lossy()] }),
    )
    .unwrap();
    assert_eq!(imported["failed"], json!([]), "{imported}");
    wait(&conn, &imported);
    let video = imported["media_ids"][0].clone();

    let out = call(
        &conn,
        "media.import_frame",
        json!({ "source": { "media": video }, "at": "0.5s" }),
    )
    .unwrap();
    wait(&conn, &out);
    assert_eq!(
        (out["width"].clone(), out["height"].clone()),
        (json!(192), json!(108)),
        "the asset's own size, not the sequence's"
    );
    assert_eq!(out["name"], "bars.mp4 00:00:00:15");
    let file = decode(&std::fs::read(out["path"].as_str().unwrap()).unwrap());
    assert_eq!((file.width, file.height), (192, 108));
    let (left, right) = (file.at(40, 54), file.at(150, 54));
    assert!(left.iter().all(|c| *c > 220), "{left:?}");
    assert!(right.iter().all(|c| *c < 30), "{right:?}");
    // The asset is not on the timeline: the document's sequence is untouched.
    let snapshot = engine.handle().snapshot().unwrap();
    assert_eq!(snapshot.doc.media.len(), 2);
    assert!(
        snapshot
            .doc
            .sequences
            .values()
            .all(|s| s.tracks.values().all(|t| t.items.is_empty()))
    );
    drop(snapshot);

    // Past the end of the media, a still and an unknown asset are refused.
    for (source, at) in [
        (json!({ "media": video }), json!("1s")),
        (json!({ "media": out["media_id"] }), json!(0)),
    ] {
        let refused = call(
            &conn,
            "media.import_frame",
            json!({ "source": source, "at": at }),
        );
        assert!(
            matches!(refused, Err(CmdError::Invalid { .. })),
            "{refused:?}"
        );
    }
    assert_eq!(frame_files(&folder).len(), 1);
    drop(conn);
    engine.shutdown();
}
