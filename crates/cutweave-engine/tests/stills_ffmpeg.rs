// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2F.05a acceptance on the full engine with the FFmpeg backend (feature `ffmpeg`; needs the
//! bundle, docs/dev/ffmpeg.md): still images imported with `media.import` show correctly in
//! `render.frame`.
//!
//! - A JPEG taken with the camera turned (EXIF orientation 6) shows upright.
//! - A 16-bit PNG with alpha composites over the colour below it: straight alpha in the file,
//!   blended in linear light.
//! - PNG, JPEG, WebP, TIFF and BMP files all import as images, without a duration.
//!
//! The pictures are written at test time (`cutweave_media::testing::stills`): no corpus.

#![cfg(feature = "ffmpeg")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod engine_support;
mod full_support;

use std::path::{Path, PathBuf};

use cutweave_api::Credential;
use cutweave_engine::{Connection, Engine, EngineMode, FullConfig, FullSubsystems};
use cutweave_media::testing::stills::{self, exif_orientation, png_color};
use engine_support::TempDir;
use full_support::{Clocks, SEC, call, engine_config};
use serde_json::{Value, json};

fn engine(dir: &Path, clocks: &Clocks) -> Engine {
    let config = FullConfig {
        cache_dir: Some(dir.join("cache")),
        audio: clocks.mode(),
        headless_preview: (320, 180),
        ..FullConfig::native()
    };
    assert!(config.media.is_some(), "{:?}", config.media_problem);
    let config = engine_config(
        dir,
        EngineMode::Headless,
        Box::new(FullSubsystems::new(config)),
    );
    Engine::start(config).unwrap()
}

fn agent(engine: &Engine) -> Connection {
    engine
        .handle()
        .connect(&Credential::Agent("cli".into()))
        .unwrap()
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

/// Imports `paths` and waits for their jobs; the media ids, in order.
fn import(conn: &Connection, paths: &[PathBuf]) -> Vec<Value> {
    let paths: Vec<String> = paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let imported = call(conn, "media.import", json!({ "paths": paths })).unwrap();
    assert_eq!(imported["failed"], json!([]), "{imported}");
    for job in imported["job_ids"].as_array().unwrap() {
        let info = call(
            conn,
            "job.wait",
            json!({ "job_id": job, "timeout_ms": 60_000 }),
        )
        .unwrap();
        assert_eq!(info["state"], "succeeded", "{info}");
    }
    imported["media_ids"].as_array().unwrap().clone()
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

/// A rendered frame: its size and R'G'B' pixels.
struct Picture {
    width: u32,
    data: Vec<[u8; 3]>,
}

impl Picture {
    fn at(&self, x: u32, y: u32) -> [u8; 3] {
        self.data[(y * self.width + x) as usize]
    }
}

/// `render.frame` at `t` as a 320 × 180 picture; nothing may be missing.
fn frame(conn: &Connection, t: i64) -> Picture {
    let out = call(
        conn,
        "render.frame",
        json!({ "t": t, "max_size": 320, "format": "png" }),
    )
    .unwrap();
    assert_eq!(out["notes"], json!([]), "{}", out["notes"]);
    assert_eq!(
        (out["width"].clone(), out["height"].clone()),
        (json!(320), json!(180))
    );
    let bytes = unbase64(out["data"].as_str().unwrap());
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
        .read_info()
        .unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    let channels = match info.color_type {
        png::ColorType::Rgba => 4,
        png::ColorType::Rgb => 3,
        other => panic!("unexpected PNG colour type {other:?}"),
    };
    Picture {
        width: info.width,
        data: buf[..info.buffer_size()]
            .chunks(channels)
            .map(|c| [c[0], c[1], c[2]])
            .collect(),
    }
}

fn close(a: [u8; 3], b: [u8; 3], tolerance: u8) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= tolerance)
}

/// JFIF's full-range BT.601 Y'CbCr of an R'G'B' colour.
fn ycbcr([r, g, b]: [u8; 3]) -> [u8; 3] {
    let (r, g, b) = (f64::from(r), f64::from(g), f64::from(b));
    let y = 0.299 * r + 0.587 * g + 0.114 * b;
    let cb = 128.0 - 0.168_736 * r - 0.331_264 * g + 0.5 * b;
    let cr = 128.0 + 0.5 * r - 0.418_688 * g - 0.081_312 * b;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    [y, cb, cr].map(|v| v.round().clamp(0.0, 255.0) as u8)
}

const RED: [u8; 3] = [200, 30, 30];
const GREEN: [u8; 3] = [30, 200, 30];
const BLUE: [u8; 3] = [30, 30, 200];
const WHITE: [u8; 3] = [240, 240, 240];
const DARK: [u8; 3] = [20, 20, 20];
const YELLOW: [u8; 3] = [220, 220, 40];

/// A rotated JPEG (EXIF 6) displays upright (WBS acceptance 1 of WP-2F.05).
#[test]
fn a_jpeg_with_exif_orientation_6_shows_upright() {
    let t = TempDir::new("still-exif");
    let clocks = Clocks::new();
    let engine = engine(t.path(), &clocks);
    let conn = agent(&engine);
    let project = call(&conn, "project.new", json!({ "name": "Stills" })).unwrap();

    // Coded 24 × 16 (landscape), three cells by two:
    //   red   green blue
    //   white dark  yellow
    // The camera was turned: shown turned 90° clockwise, the picture is 16 × 24 (portrait):
    //   white  red
    //   dark   green
    //   yellow blue
    let cells = [RED, GREEN, BLUE, WHITE, DARK, YELLOW].map(ycbcr);
    let jpeg = stills::jpeg(3, 2, &cells, Some(&exif_orientation(6, false)));
    let media = import(&conn, &[write(t.path(), "turned.jpg", &jpeg)]);

    let listed = call(&conn, "media.list", json!({})).unwrap();
    assert_eq!(listed["media"][0]["kind"], "image", "{listed}");
    let snapshot = engine.handle().snapshot().unwrap();
    let asset = snapshot.doc.media.values().next().unwrap();
    assert_eq!(asset.probe.rotation, 90, "{:?}", asset.probe);
    assert_eq!(asset.probe.duration, None);

    // No duration given: a still gets the default length (5 s today).
    let inserted = call(
        &conn,
        "timeline.insert",
        json!({
            "sequence": project["sequence"], "track": project["video_track"],
            "media": media[0], "at": 0,
        }),
    )
    .unwrap();
    assert_eq!(inserted["duration"], json!(5 * SEC), "{inserted}");

    // The portrait picture is fitted to the 320 × 180 frame's height: 120 × 180 in the middle
    // (x 100 to 220), each cell 60 × 60. The same picture at any time of the clip.
    for at in [0, 2 * SEC, 5 * SEC - 1] {
        let picture = frame(&conn, at);
        let expected = [[WHITE, RED], [DARK, GREEN], [YELLOW, BLUE]];
        for (row, colours) in expected.iter().enumerate() {
            for (col, colour) in colours.iter().enumerate() {
                let (x, y) = (130 + 60 * col as u32, 30 + 60 * row as u32);
                let got = picture.at(x, y);
                assert!(
                    close(got, *colour, 6),
                    "t {at}: ({x}, {y}) is {got:?}, expected {colour:?}"
                );
            }
        }
        // Beside the picture: the sequence's black background.
        assert_eq!(picture.at(40, 90), [0, 0, 0]);
        assert_eq!(picture.at(280, 90), [0, 0, 0]);
    }
    drop(conn);
    engine.shutdown();
}

fn srgb_to_linear(v: u8) -> f64 {
    let c = f64::from(v) / 255.0;
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(l: f64) -> u8 {
    let c = if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    };
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        (c * 255.0).round().clamp(0.0, 255.0) as u8
    }
}

/// A 16-bit PNG with alpha composites correctly over colour (WBS acceptance 1 of WP-2F.05).
#[test]
fn a_16_bit_png_with_alpha_composites_over_the_colour_below() {
    let t = TempDir::new("still-alpha");
    let clocks = Clocks::new();
    let engine = engine(t.path(), &clocks);
    let conn = agent(&engine);
    let project = call(&conn, "project.new", json!({ "name": "Alpha" })).unwrap();

    // Below: a 32 × 18 BMP of one colour (it fills the 16:9 frame).
    let below = [30_u8, 60, 200];
    let bmp = stills::bmp(32, 18, &below.repeat(32 * 18));
    // Above: a 32 × 18 PNG, 16 bits per sample. Left half opaque, right half about half
    // transparent with the same straight (not premultiplied) colour; the bottom rows are fully
    // transparent with a colour that must not show.
    let above = [0xC0_u8, 0x40, 0x20];
    let alpha = 0x80_u8;
    let mut samples = Vec::new();
    for y in 0..18 {
        for x in 0..32 {
            let (rgb, a) = if y >= 12 {
                ([0xFF, 0xFF, 0xFF], 0)
            } else if x < 16 {
                (above, 0xFF)
            } else {
                (above, alpha)
            };
            for v in [rgb[0], rgb[1], rgb[2], a] {
                // The 8-bit value in both bytes: 0x80 becomes 0x8080.
                samples.extend_from_slice(&[v, v]);
            }
        }
    }
    let png = stills::png(32, 18, 16, png_color::RGBA, &samples, &[]);
    let media = import(
        &conn,
        &[
            write(t.path(), "below.bmp", &bmp),
            write(t.path(), "above.png", &png),
        ],
    );
    let top = call(
        &conn,
        "track.add",
        json!({ "sequence": project["sequence"], "kind": "video" }),
    )
    .unwrap();
    for (track, media) in [
        (&project["video_track"], &media[0]),
        (&top["track"], &media[1]),
    ] {
        call(
            &conn,
            "timeline.insert",
            json!({
                "sequence": project["sequence"], "track": track, "media": media,
                "at": 0, "duration": "2s",
            }),
        )
        .unwrap();
    }

    let picture = frame(&conn, SEC);
    // Opaque: the PNG's colour.
    let got = picture.at(80, 60);
    assert!(close(got, above, 2), "opaque: {got:?}");
    // Half transparent: `a·above + (1 − a)·below` in linear light. (Blending the encoded values
    // would give about [111, 62, 116]; treating the file as premultiplied, a brighter picture.)
    let a = f64::from(alpha) / 255.0;
    let mut expected = [0_u8; 3];
    for c in 0..3 {
        expected[c] =
            linear_to_srgb(a * srgb_to_linear(above[c]) + (1.0 - a) * srgb_to_linear(below[c]));
    }
    let got = picture.at(240, 60);
    assert!(
        close(got, expected, 3),
        "half: {got:?}, expected {expected:?}"
    );
    assert!(
        !close(got, [111, 62, 116], 8),
        "blended in encoded values: {got:?}"
    );
    // Fully transparent: only the colour below, whatever colour the PNG stores there.
    let got = picture.at(160, 165);
    assert!(close(got, below, 2), "transparent: {got:?}");
    drop(conn);
    engine.shutdown();
}

/// PNG, JPEG, WebP, TIFF and BMP all import as images through `media.import`.
#[test]
fn png_jpeg_webp_tiff_and_bmp_import_as_images() {
    let t = TempDir::new("still-kinds");
    let clocks = Clocks::new();
    let engine = engine(t.path(), &clocks);
    let conn = agent(&engine);
    let project = call(&conn, "project.new", json!({ "name": "Kinds" })).unwrap();
    let colour = [200_u8, 120, 40];
    let rgb = colour.repeat(32 * 16);
    let rgba: Vec<u8> = rgb
        .chunks(3)
        .flat_map(|c| [c[0], c[1], c[2], 255])
        .collect();
    // WebP: red 200, green 120, blue 40 everywhere (the second green is unused).
    let webp = stills::webp(
        32,
        16,
        &vec![false; 32 * 16],
        [200, 40, 255],
        [120, 121],
        None,
    );
    let files = [
        ("a.png", stills::png(32, 16, 8, png_color::RGBA, &rgba, &[])),
        ("b.jpg", stills::jpeg(4, 2, &[ycbcr(colour); 8], None)),
        ("c.webp", webp),
        ("d.tiff", stills::tiff(32, 16, 3, 8, &rgb, None)),
        ("e.bmp", stills::bmp(32, 16, &rgb)),
    ];
    let paths: Vec<PathBuf> = files
        .iter()
        .map(|(name, bytes)| write(t.path(), name, bytes))
        .collect();
    let media = import(&conn, &paths);
    assert_eq!(media.len(), files.len());
    let snapshot = engine.handle().snapshot().unwrap();
    for asset in snapshot.doc.media.values() {
        assert_eq!(
            asset.kind,
            cutweave_model::MediaKind::Image,
            "{:?}",
            asset.path_abs
        );
        assert_eq!(asset.probe.duration, None, "{:?}", asset.path_abs);
        assert_eq!(asset.probe.rotation, 0, "{:?}", asset.path_abs);
    }
    // One after the other on the timeline: each shows its colour (2:1 pictures in a 16:9
    // frame: full width, 160 high, in the middle).
    for (i, id) in media.iter().enumerate() {
        let at = i64::try_from(i).unwrap() * SEC;
        call(
            &conn,
            "timeline.insert",
            json!({
                "sequence": project["sequence"], "track": project["video_track"],
                "media": id, "at": at, "duration": "1s",
            }),
        )
        .unwrap();
        let got = frame(&conn, at + SEC / 2).at(160, 90);
        assert!(close(got, colour, 5), "{}: {got:?}", files[i].0);
    }
    drop(conn);
    engine.shutdown();
}
