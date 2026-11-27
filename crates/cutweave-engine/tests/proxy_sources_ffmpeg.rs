// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2F.02 acceptance on real media (feature `ffmpeg`; needs the bundle's `ffmpeg` CLI,
//! docs/dev/ffmpeg.md). Three 1280 × 720 long-GOP H.264 clips with sound are generated with the
//! CLI into a temporary folder (never committed) and imported into a project that makes a proxy
//! of every video (960 × 540 ProRes Proxy). The engine's media backend is spied on
//! (`SpyBackend` notes every file it is asked to read).
//!
//! - **An export of the three clips opens no proxy file** (0 reads under the proxy cache),
//!   although the three proxies are ready and the preview was just showing one.
//! - **The exported file of the project with proxies equals the exported file without them**:
//!   the same export after the proxies are deleted and the project makes none; ten sample
//!   frames of both files, decoded back with the CLI, have the same hashes (and the files are
//!   the same bytes, which the test reports but does not require of the encoder).
//!
//! `probe_toggling_proxies_during_playback` (ignored: a measurement on this machine's own GPU
//! and clock, run on request) plays a clip in real time, switches the preview's proxies off and
//! on every half second for 5 s and reports the frames dropped. The acceptance budget is 2.

#![cfg(feature = "ffmpeg")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod engine_support;
mod full_support;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration as StdDuration, Instant};

use cutweave_api::{Credential, Token};
use cutweave_engine::{
    AudioMode, Connection, Engine, EngineMode, FullConfig, FullHandle, FullSubsystems, GpuConfig,
};
use cutweave_media::MediaBackend;
use cutweave_media::testing::{OpenLog, SpyBackend};
use engine_support::TempDir;
use full_support::{Clocks, SEC, call, corpus_file, engine_config, eventually, ffmpeg_tool};
use serde_json::{Value, json};

const W: usize = 320;
const H: usize = 180;
/// Frames of the exported timeline: three clips of 1 s at 30 fps.
const FRAMES: usize = 90;
/// The ten frames compared: spread over the three clips, with both ends.
const SAMPLES: [usize; 10] = [0, 9, 19, 29, 30, 44, 59, 60, 75, 89];

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

/// A 1 s 1280 × 720 30 fps H.264 clip (one GOP, B-frames) of the moving test pattern `source`
/// with a tone of `hz`: fine detail everywhere, so a frame decoded from a 960 × 540 proxy
/// could not pass for the original's.
fn generate(tool: &Path, out: &Path, source: &str, hz: u32) {
    let video = format!("{source}=s=1280x720:r=30");
    let audio = format!("sine=frequency={hz}:sample_rate=48000:duration=1");
    ffmpeg(
        tool,
        &[
            "-f",
            "lavfi",
            "-i",
            &video,
            "-f",
            "lavfi",
            "-i",
            &audio,
            "-c:v",
            "libx264",
            "-g",
            "30",
            "-bf",
            "2",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-t",
            "1",
            &out.to_string_lossy(),
        ],
    );
}

struct Rig {
    dir: TempDir,
    engine: Engine,
    handle: FullHandle,
    ui: Connection,
    agent: Connection,
    log: OpenLog,
}

/// A GUI engine on FFmpeg, spied on. `clocks`: the test's clocks, or the machine's own (real
/// time) with `gpu` as given.
fn rig(tag: &str, clocks: Option<&Clocks>, gpu: GpuConfig) -> Rig {
    let dir = TempDir::new(tag);
    let mut config = FullConfig {
        cache_dir: Some(dir.path().join("cache")),
        audio: clocks.map_or(AudioMode::FromEngine, Clocks::mode),
        ..FullConfig::native()
    };
    let backend = config.media.take();
    assert!(backend.is_some(), "{:?}", config.media_problem);
    let spy = SpyBackend::new(backend.unwrap());
    let log = spy.log();
    let backend: Arc<dyn MediaBackend> = Arc::new(spy);
    config.media = Some(backend);
    let full = FullSubsystems::new(config);
    let handle = full.handle();
    let mut config = engine_config(dir.path(), EngineMode::Gui, Box::new(full));
    config.gpu = gpu;
    config.ui_token = Some(Token::parse(&"u".repeat(48)).unwrap());
    let engine = Engine::start(config).unwrap();
    eventually("the GPU", || handle.gpu_settled().then_some(()));
    let ui = engine.handle().connect(&Credential::Ui).unwrap();
    let agent = engine
        .handle()
        .connect(&Credential::Agent("cli".into()))
        .unwrap();
    Rig {
        dir,
        engine,
        handle,
        ui,
        agent,
        log,
    }
}

impl Rig {
    fn proxies_dir(&self) -> PathBuf {
        self.dir.path().join("cache").join("proxies")
    }

    fn proxy_status(&self, media: &Value) -> Value {
        call(
            &self.agent,
            "media.proxy_status",
            json!({ "media": [media] }),
        )
        .unwrap()["proxies"][0]
            .clone()
    }

    /// Waits (up to `secs`) for the proxy of `media` to be `want`.
    fn wait_proxy(&self, media: &Value, want: &str, secs: u64) -> Value {
        let deadline = Instant::now() + StdDuration::from_secs(secs);
        loop {
            let s = self.proxy_status(media);
            if s["status"] == want {
                return s;
            }
            assert!(
                Instant::now() < deadline && s["status"] != "failed",
                "the proxy is not {want}: {s}"
            );
            std::thread::sleep(StdDuration::from_millis(100));
        }
    }

    /// The playback state (the proxy command without a change answers with it).
    fn state(&self) -> Value {
        call(&self.ui, "view.set_preview_quality", json!({})).unwrap()
    }

    /// Exports the whole timeline to `out` in software, at `W` × `H`.
    fn export(&self, out: &Path) {
        let started = call(
            &self.agent,
            "export.start",
            json!({
                "path": out.to_string_lossy(),
                "preset": { "custom": { "width": W, "height": H } },
                "hw": "off",
            }),
        )
        .unwrap();
        let mut done = Value::Null;
        for _ in 0..5 {
            done = call(
                &self.agent,
                "job.wait",
                json!({ "job_id": started["job_id"], "timeout_ms": 60_000 }),
            )
            .unwrap();
            if done["state"] != "running" && done["state"] != "queued" {
                break;
            }
        }
        assert_eq!(done["state"], "succeeded", "{done}");
    }
}

/// FNV-1a of `bytes`: a frame's hash.
fn hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// The hashes of the [`SAMPLES`] frames of the exported `file`, decoded with the CLI to RGB.
fn sample_hashes(tool: &Path, file: &Path) -> Vec<u64> {
    let rgb = ffmpeg(
        tool,
        &[
            "-i",
            &file.to_string_lossy(),
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-",
        ],
    );
    let frame = W * H * 3;
    assert_eq!(rgb.len(), FRAMES * frame, "{} frames", rgb.len() / frame);
    SAMPLES
        .iter()
        .map(|n| hash(&rgb[n * frame..(n + 1) * frame]))
        .collect()
}

#[test]
fn an_export_reads_no_proxy_and_equals_the_export_without_proxies() {
    let Some(tool) = ffmpeg_tool("ffmpeg") else {
        eprintln!("skipping: no ffmpeg CLI (source scripts/ffmpeg/env.sh)");
        return;
    };
    let clocks = Clocks::new();
    let r = rig("proxy-sources-ffmpeg", Some(&clocks), GpuConfig::Software);
    let clips: Vec<PathBuf> = [("testsrc2", 440), ("testsrc", 660), ("mandelbrot", 880)]
        .iter()
        .enumerate()
        .map(|(n, (source, hz))| {
            let path = r.dir.path().join(format!("clip-{n}.mp4"));
            generate(&tool, &path, source, *hz);
            path
        })
        .collect();

    let project = call(&r.ui, "project.new", json!({ "name": "Proxy sources" })).unwrap();
    call(&r.ui, "media.set_proxy_policy", json!({ "policy": "all" })).unwrap();
    call(
        &r.ui,
        "view.set_preview_size",
        json!({ "width": 640, "height": 360 }),
    )
    .unwrap();
    let paths: Vec<String> = clips
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let imported = call(&r.ui, "media.import", json!({ "paths": paths })).unwrap();
    assert_eq!(imported["failed"], json!([]), "{imported}");
    let media = imported["media_ids"].as_array().unwrap().clone();
    assert_eq!(media.len(), 3);
    let mut proxy_files = Vec::new();
    for id in &media {
        let ready = r.wait_proxy(id, "ready", 180);
        assert_eq!(ready["size"], json!([960, 540]), "{ready}");
        proxy_files.push(PathBuf::from(ready["path"].as_str().unwrap()));
    }
    for (n, id) in media.iter().enumerate() {
        call(
            &r.ui,
            "timeline.insert",
            json!({
                "sequence": project["sequence"], "track": project["video_track"],
                "media": id, "at": i64::try_from(n).unwrap() * SEC, "duration": "1s",
            }),
        )
        .unwrap();
    }

    // The proxies are in use: the frame on screen (second clip) is decoded from one.
    call(&r.ui, "view.seek", json!({ "t": SEC + SEC / 2 })).unwrap();
    let shown = eventually("the proxy on screen", || {
        clocks.advance(SEC / 5);
        let s = r.state();
        (s["state"] == "stopped" && s["proxy"] == json!(true)).then_some(s)
    });
    assert_eq!(shown["use_proxies"], json!(true), "{shown}");
    assert!(
        !r.log.under(&r.proxies_dir()).is_empty(),
        "the spy sees proxy files being read"
    );

    // With proxies: the export opens none of them.
    r.log.clear();
    let with = r.dir.path().join("with-proxies.mp4");
    r.export(&with);
    let under = r.log.under(&r.proxies_dir());
    assert_eq!(
        under,
        Vec::new(),
        "an export reads no file of the proxy cache"
    );
    let opens = r.log.decoder_opens();
    assert!(
        opens.iter().all(|o| !proxy_files.contains(&o.path)),
        "{opens:?}"
    );
    for clip in &clips {
        assert!(
            opens.iter().any(|o| &o.path == clip),
            "{} was not read: {opens:?}",
            clip.display()
        );
    }

    // Without: the proxies deleted, and the project makes none.
    call(&r.ui, "media.set_proxy_policy", json!({ "policy": "off" })).unwrap();
    let deleted = call(&r.agent, "media.delete_proxy", json!({ "media": media })).unwrap();
    for p in deleted["proxies"].as_array().unwrap() {
        assert_eq!(p["status"], "evicted", "{deleted}");
    }
    for id in &media {
        assert_eq!(r.proxy_status(id)["status"], "evicted");
    }
    let without = r.dir.path().join("without-proxies.mp4");
    r.export(&without);

    let a = sample_hashes(&tool, &with);
    let b = sample_hashes(&tool, &without);
    println!("frame hashes with proxies:    {a:016x?}");
    println!("frame hashes without proxies: {b:016x?}");
    assert_eq!(a, b, "the ten sample frames of the two exports");
    // The samples are pictures, each its own: not ten black frames that would agree anyway.
    let mut distinct = a.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(distinct.len(), SAMPLES.len(), "{a:016x?}");
    let same_bytes = std::fs::read(&with).unwrap() == std::fs::read(&without).unwrap();
    println!("the two exported files are the same bytes: {same_bytes}");
    drop((r.ui, r.agent));
    r.engine.shutdown();
}

/// The measurement behind "toggling proxies during playback drops no more than 2 frames within
/// 5 s" (WBS WP-2F.02): real time, this machine's own GPU. Plays the corpus 4K long-GOP clip
/// when the corpus is fetched (the kind of file proxies are for), else a generated 1280 × 720
/// one, at a docked preview size (960 × 540), starting on the original, and switches the proxies
/// on and off every 500 ms for 5 s.
/// Prints what it measured; run it alone on a quiet machine:
/// `cargo nextest run -p cutweave-engine --features ffmpeg --test proxy_sources_ffmpeg
/// --run-ignored ignored-only --no-capture`.
#[test]
#[ignore = "a measurement on this machine's GPU in real time: run it on request"]
fn probe_toggling_proxies_during_playback() {
    let Some(tool) = ffmpeg_tool("ffmpeg") else {
        eprintln!("skipping: no ffmpeg CLI (source scripts/ffmpeg/env.sh)");
        return;
    };
    let r = rig("proxy-sources-probe", None, GpuConfig::Auto);
    let (clip, what) = match corpus_file("h264_4k30_longgop.mp4") {
        Some(clip) => (clip, "corpus h264_4k30_longgop.mp4 (3840x2160, 8 s GOPs)"),
        None => {
            let path = r.dir.path().join("probe.mp4");
            let video = "testsrc2=s=1280x720:r=30:d=12";
            ffmpeg(
                &tool,
                &[
                    "-f",
                    "lavfi",
                    "-i",
                    video,
                    "-c:v",
                    "libx264",
                    "-g",
                    "60",
                    "-bf",
                    "2",
                    "-pix_fmt",
                    "yuv420p",
                    &path.to_string_lossy(),
                ],
            );
            (path, "generated 1280x720 H.264, 2 s GOPs")
        }
    };
    let project = call(&r.ui, "project.new", json!({ "name": "Probe" })).unwrap();
    call(&r.ui, "media.set_proxy_policy", json!({ "policy": "all" })).unwrap();
    call(
        &r.ui,
        "view.set_preview_size",
        json!({ "width": 960, "height": 540 }),
    )
    .unwrap();
    let imported = call(
        &r.ui,
        "media.import",
        json!({ "paths": [clip.to_string_lossy()] }),
    )
    .unwrap();
    assert_eq!(imported["failed"], json!([]), "{imported}");
    let media = imported["media_ids"][0].clone();
    r.wait_proxy(&media, "ready", 600);
    call(
        &r.ui,
        "timeline.insert",
        json!({
            "sequence": project["sequence"], "track": project["video_track"],
            "media": media, "at": 0, "duration": "10s",
        }),
    )
    .unwrap();
    let caps = call(&r.agent, "media.capabilities", json!({})).unwrap();

    // Playback starts on the original (proxies off), so the first toggle is a real change of
    // file in the middle of playback: original to proxy. The toggles after it find the clip
    // on its proxy, which it keeps until the next seek (no seek into the original mid-stream).
    // Play for a second and a half first, so the start of playback is not in the measurement.
    call(
        &r.ui,
        "view.set_preview_quality",
        json!({ "use_proxies": false }),
    )
    .unwrap();
    call(&r.ui, "view.seek", json!({ "t": 0 })).unwrap();
    call(&r.ui, "view.play", json!({})).unwrap();
    std::thread::sleep(StdDuration::from_millis(1500));
    let started_on = if r.state()["proxy"] == json!(true) {
        "a proxy"
    } else {
        "the original"
    };
    let before = r.handle.playback_stats().unwrap();
    let start = Instant::now();
    let mut on = false;
    let mut toggles = 0;
    let mut shown_proxy = 0;
    let mut shown_original = 0;
    while start.elapsed() < StdDuration::from_secs(5) {
        on = !on;
        let s = call(
            &r.ui,
            "view.set_preview_quality",
            json!({ "use_proxies": on }),
        )
        .unwrap();
        assert_eq!(s["playing"], json!(true), "{s}");
        toggles += 1;
        std::thread::sleep(StdDuration::from_millis(500));
        if r.state()["proxy"] == json!(true) {
            shown_proxy += 1;
        } else {
            shown_original += 1;
        }
    }
    let after = r.handle.playback_stats().unwrap();
    let state = call(&r.ui, "view.pause", json!({})).unwrap();
    let dropped = after.dropped - before.dropped;
    let delivered = after.delivered - before.delivered;
    println!("WP-2F.02 probe: toggling proxies during playback");
    println!("  clip: {what}; playback started on {started_on}");
    println!("  gpu: {}", caps["gpu"]);
    println!("  hw decode: {}", caps["hw_decode"]);
    println!(
        "  5 s of playback at 30 fps, {toggles} toggles: {delivered} frames delivered, \
         {dropped} dropped (budget 2)"
    );
    println!(
        "  half seconds that ended on a proxy picture: {shown_proxy}, on the original: \
         {shown_original}"
    );
    println!("  errors: {}", after.errors - before.errors);
    assert!(
        state["t"].as_i64().unwrap() > 5 * SEC,
        "it played on: {state}"
    );
    assert!(dropped <= 2, "{dropped} frames dropped within 5 s");
    drop((r.ui, r.agent));
    r.engine.shutdown();
}
