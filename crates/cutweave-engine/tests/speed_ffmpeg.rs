// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2D.01 acceptance, frame-exact speed on real H.264 (feature `ffmpeg`; needs the bundle's
//! `ffmpeg` CLI, docs/dev/ffmpeg.md): a clip whose every frame carries its own number as eight
//! black or white bars (bit `b` of the frame number is bar `b`; an OCR-free counter that survives
//! colour conversion, scaling and H.264) is generated with the `ffmpeg` CLI into a temporary
//! folder (never committed): 640×360, 30 fps, 60 frames, long GOP with B-frames. A full headless
//! engine (software GPU) imports it, places 2 s and sets the speed to 0.5 with `clip.set_speed`:
//!
//! - **preview** (`render.frame`) and **export** (an MP4 decoded back with the CLI) both show
//!   source frame `floor(k × 0.5)` at timeline frame `k`, for every one of the 120 frames;
//! - **reversed** (`clip.reverse`), both show source frame `59 − floor(k × 0.5)`: the first frame
//!   of the timeline is the last frame of the range.
//!
//! Reverse playback scrubs backwards through the decoder's ordinary seek (keyframe, then decode
//! forward), so without a proxy every frame of a long GOP costs a GOP decode; with a proxy
//! (intra-frame) it costs one frame. That cost is measured and bounded in WP-2D.01b.

#![cfg(feature = "ffmpeg")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod engine_support;
mod full_support;

use std::path::Path;
use std::process::Command;

use cutweave_api::Credential;
use cutweave_engine::{Connection, Engine, EngineMode, FullConfig, FullSubsystems};
use engine_support::TempDir;
use full_support::{Clocks, SEC, call, engine_config, ffmpeg_tool};
use serde_json::{Value, json};

const W: usize = 640;
const H: usize = 360;
/// Frames of the generated clip (2 s at 30 fps).
const FRAMES: u32 = 60;
/// Width of one bar of the counter.
const BAR: usize = 80;

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

/// Runs the bundle's `ffmpeg` and returns its stdout.
fn ffmpeg(tool: &Path, args: &[&str]) -> Vec<u8> {
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
    out.stdout
}

/// The counter clip: frame `N` has bar `b` white when bit `b` of `N` is set.
fn generate(tool: &Path, out: &Path) {
    let source = format!("color=c=black:s={W}x{H}:r=30:d=2");
    let bars =
        format!("geq=lum='if(mod(floor(N/pow(2\\,floor(X/{BAR})))\\,2)\\,235\\,16)':cb=128:cr=128");
    ffmpeg(
        tool,
        &[
            "-f",
            "lavfi",
            "-i",
            &source,
            "-vf",
            &bars,
            "-c:v",
            "libx264",
            "-g",
            "30",
            "-bf",
            "2",
            "-pix_fmt",
            "yuv420p",
            &out.to_string_lossy(),
        ],
    );
}

/// The number the bars of a `W`×`H` picture encode; `value(x, y)` is the brightness there.
fn counter(value: impl Fn(usize, usize) -> u8) -> u32 {
    (0..8)
        .filter(|b| value(BAR / 2 + BAR * b, H / 2) > 128)
        .map(|b| 1 << b)
        .sum()
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

/// The counter `render.frame` shows at timeline frame `k` (30 fps).
fn preview_counter(conn: &Connection, k: u32) -> u32 {
    let t = i64::from(k) * SEC / 30;
    let out = call(
        conn,
        "render.frame",
        json!({ "t": t, "max_size": W, "format": "png" }),
    )
    .unwrap();
    assert_eq!(
        (out["width"].clone(), out["height"].clone()),
        (json!(W), json!(H))
    );
    let png = unbase64(out["data"].as_str().unwrap());
    let mut reader = png::Decoder::new(std::io::Cursor::new(png))
        .read_info()
        .unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    let channels = info.color_type.samples();
    counter(|x, y| buf[(y * W + x) * channels])
}

/// The counters of every frame of the exported MP4, decoded with the CLI to grey.
fn export_counters(conn: &Connection, tool: &Path, out: &Path) -> Vec<u32> {
    let started = call(
        conn,
        "export.start",
        json!({
            "path": out.to_string_lossy(),
            "range": { "in": 0, "out": "4s" },
            "preset": { "custom": { "width": W, "height": H } },
            "hw": "off",
        }),
    )
    .unwrap();
    let job = call(
        conn,
        "job.wait",
        json!({ "id": started["job_id"], "timeout_ms": 60_000 }),
    )
    .unwrap();
    assert_eq!(job["state"], "succeeded", "{job}");
    let grey = ffmpeg(
        tool,
        &[
            "-i",
            &out.to_string_lossy(),
            "-f",
            "rawvideo",
            "-pix_fmt",
            "gray",
            "-",
        ],
    );
    grey.chunks(W * H)
        .map(|frame| counter(|x, y| frame[y * W + x]))
        .collect()
}

#[test]
fn half_speed_and_reverse_show_the_exact_source_frame_in_preview_and_export() {
    let Some(tool) = ffmpeg_tool("ffmpeg") else {
        eprintln!("skipping: no ffmpeg CLI (source scripts/ffmpeg/env.sh)");
        return;
    };
    let t = TempDir::new("speed");
    let clip = t.path().join("counter.mp4");
    generate(&tool, &clip);
    let clocks = Clocks::new();
    let engine = engine(t.path(), &clocks);
    let conn = agent(&engine);
    let project = call(&conn, "project.new", json!({ "name": "Speed" })).unwrap();
    let imported = call(
        &conn,
        "media.import",
        json!({ "paths": [clip.to_string_lossy()] }),
    )
    .unwrap();
    assert_eq!(imported["failed"], json!([]), "{imported}");
    for job in imported["job_ids"].as_array().unwrap() {
        call(
            &conn,
            "job.wait",
            json!({ "id": job, "timeout_ms": 60_000 }),
        )
        .unwrap();
    }
    let inserted = call(
        &conn,
        "timeline.insert",
        json!({
            "sequence": project["sequence"], "track": project["video_track"],
            "media": imported["media_ids"][0], "at": 0, "duration": "2s",
        }),
    )
    .unwrap();
    let item: Value = inserted["clips"][0].clone();
    // The counter is right at normal speed before anything else is checked.
    assert_eq!(preview_counter(&conn, 37), 37);

    let sped = call(
        &conn,
        "clip.set_speed",
        json!({ "item": item, "speed": 0.5 }),
    )
    .unwrap();
    assert_eq!(sped["items"][0]["duration"], json!(4 * SEC), "{sped}");
    for k in 0..2 * FRAMES {
        assert_eq!(
            preview_counter(&conn, k),
            k / 2,
            "preview, timeline frame {k}"
        );
    }
    let exported = export_counters(&conn, &tool, &t.path().join("half.mp4"));
    let want: Vec<u32> = (0..2 * FRAMES).map(|k| k / 2).collect();
    assert_eq!(exported, want, "export at half speed");

    call(&conn, "clip.reverse", json!({ "item": item })).unwrap();
    for k in (0..2 * FRAMES).step_by(7).chain([2 * FRAMES - 1]) {
        assert_eq!(
            preview_counter(&conn, k),
            FRAMES - 1 - k / 2,
            "reversed preview, timeline frame {k}"
        );
    }
    let exported = export_counters(&conn, &tool, &t.path().join("reversed.mp4"));
    let want: Vec<u32> = (0..2 * FRAMES).map(|k| FRAMES - 1 - k / 2).collect();
    assert_eq!(exported, want, "export reversed at half speed");

    drop(conn);
    engine.shutdown();
}
