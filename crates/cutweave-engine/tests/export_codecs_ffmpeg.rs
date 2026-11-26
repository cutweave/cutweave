// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2G.02 acceptance through the engine: `export.start` with the built-in HEVC, AV1 and ProRes
//! presets (and a user preset for ProRes 4444 with alpha) writes files `ffprobe` reads back as
//! the preset says: codec, profile, pixel format, BT.709 tags, `hvc1` for HEVC, faststart,
//! duration within one frame, and the audio track (AAC, or 24-bit PCM in MOV). `export.list_encoders`
//! lists the codecs, and a forced hardware encoder of the wrong codec is refused.
//!
//! **Software encoders only** (`hw: "off"`): the matrix is deterministic and needs no GPU; the
//! renderer is the GPU-free probe renderer (the pictures are not the point here, the file is).
//! Needs the FFmpeg bundle (`. scripts/ffmpeg/env.sh`, `ffprobe` from `$FFMPEG_DIR/bin`).
#![cfg(feature = "ffmpeg")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod export_support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration as StdDuration;

use cutweave_engine::Call;
use cutweave_engine::export::{DecoderSources, ExportServices};
use cutweave_media::testing::FakeBackend;
use cutweave_media::{FfmpegBackend, PixelFormat};
use cutweave_model::{FrameRate, SampleRate};
use export_support::{
    LateSource, ProbeRenderer, TempDir, agent, assert_succeeded, clip, engine, insert, must,
    new_project, register, wait,
};
use serde_json::json;

const WAIT: StdDuration = StdDuration::from_secs(600);
const SIZE: (u32, u32) = (320, 180);
/// 2 s of the first clip then 1 s of the second: 3 s at 30 fps.
const FRAMES: u32 = 90;

fn tool(name: &str) -> PathBuf {
    if let Ok(dir) = std::env::var("FFMPEG_DIR") {
        let exe = Path::new(&dir)
            .join("bin")
            .join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        if exe.is_file() {
            return exe;
        }
    }
    PathBuf::from(name)
}

fn probe(path: &Path) -> BTreeMap<String, String> {
    let out = Command::new(tool("ffprobe"))
        .args([
            "-v",
            "error",
            "-show_format",
            "-show_streams",
            "-count_frames",
        ])
        .args(["-of", "flat=s=_"])
        .arg(path)
        .output()
        .expect("run ffprobe (source scripts/ffmpeg/env.sh)");
    assert!(
        out.status.success() && out.stderr.is_empty(),
        "ffprobe: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.to_owned(), v.trim_matches('"').to_owned()))
        .collect()
}

fn get<'a>(info: &'a BTreeMap<String, String>, key: &str) -> &'a str {
    info.get(key).map_or_else(
        || panic!("ffprobe has no `{key}`: {info:#?}"),
        String::as_str,
    )
}

/// An engine over the real FFmpeg encoder with a two-clip, 3 s project at 320x180.
struct Rig {
    dir: TempDir,
    engine: cutweave_engine::Engine,
    conn: cutweave_engine::Connection,
}

impl Rig {
    fn new(tag: &str) -> Rig {
        let dir = TempDir::new(tag);
        let paths = [dir.file("a.mp4"), dir.file("b.mp4")];
        let fake = FakeBackend::new()
            .with_media(
                &paths[0],
                clip(
                    3,
                    SIZE,
                    FrameRate::FPS_30,
                    PixelFormat::Yuv420p,
                    440.0,
                    SampleRate::HZ_48_000,
                ),
            )
            .with_media(
                &paths[1],
                clip(
                    3,
                    SIZE,
                    FrameRate::FPS_30,
                    PixelFormat::Nv12,
                    660.0,
                    SampleRate::HZ_48_000,
                ),
            );
        // Decode the fake clips (the export's sources), encode with the real FFmpeg encoder.
        let svc = ExportServices::new(
            Arc::new(FfmpegBackend::new().expect("the loaded FFmpeg matches the build")),
            Arc::new(DecoderSources::new(Arc::new(fake.clone()))),
            ProbeRenderer::new(Arc::new(LateSource::default()) as _).shared(),
        );
        let engine = engine(dir.path(), Some(svc));
        let conn = agent(&engine);
        let (seq, video, _) = new_project(&conn);
        let ids: Vec<_> = paths.iter().map(|p| register(&conn, &fake, p)).collect();
        insert(&conn, &seq, &video, ids[0], "0s", "2s", "0s");
        insert(&conn, &seq, &video, ids[1], "2s", "1s", "1s");
        Rig { dir, engine, conn }
    }

    /// Exports with `preset` (at 320x180) in software into `file`; returns the job's message.
    fn export(&self, preset: &str, file: &str) -> (PathBuf, String) {
        let path = self.dir.file(file);
        let started = must(
            &self.conn,
            "export.start",
            json!({
                "path": path,
                "preset": {"custom": {"preset": preset, "width": SIZE.0, "height": SIZE.1}},
                "hw": "off",
            }),
        );
        let info = wait(&self.conn, &started["job_id"], WAIT);
        assert_succeeded(&info);
        (
            PathBuf::from(path),
            info.progress.message.unwrap_or_default(),
        )
    }

    fn close(self) {
        drop(self.conn);
        self.engine.shutdown();
    }
}

/// What `ffprobe` must read: the video codec, profile, pixel format and sample entry, and the
/// audio codec.
struct Expect {
    codec: &'static str,
    profile: &'static str,
    pix_fmt: &'static str,
    tag: &'static str,
    audio: &'static str,
    encoder: &'static str,
}

fn conform(path: &Path, message: &str, want: &Expect) {
    let info = probe(path);
    let v = |k: &str| get(&info, &format!("streams_stream_0_{k}")).to_owned();
    let a = |k: &str| get(&info, &format!("streams_stream_1_{k}")).to_owned();
    assert!(message.contains(want.encoder), "{message}");
    assert_eq!(v("codec_name"), want.codec);
    assert_eq!(v("profile"), want.profile);
    assert_eq!(v("pix_fmt"), want.pix_fmt);
    assert_eq!(v("codec_tag_string"), want.tag);
    assert_eq!(
        (v("width"), v("height")),
        (SIZE.0.to_string(), SIZE.1.to_string())
    );
    assert_eq!(v("r_frame_rate"), "30/1");
    assert_eq!(v("nb_read_frames"), FRAMES.to_string());
    // Rec.709 by default: primaries, transfer, matrix, limited range.
    for (key, value) in [
        ("color_primaries", "bt709"),
        ("color_transfer", "bt709"),
        ("color_space", "bt709"),
        ("color_range", "tv"),
    ] {
        assert_eq!(v(key), value, "{key}");
    }
    assert_eq!(a("codec_name"), want.audio);
    assert_eq!(
        (a("channels"), a("sample_rate")),
        ("2".into(), "48000".into())
    );
    let duration: f64 = get(&info, "format_duration").parse().unwrap();
    assert!(
        (duration - 3.0).abs() <= 1.0 / 30.0,
        "duration {duration} s is not 3 s ±1 frame"
    );
    // Faststart: the index comes before the media data.
    let bytes = std::fs::read(path).unwrap();
    let at = |kind: &[u8; 4]| bytes.windows(4).position(|w| w == kind).unwrap();
    assert!(at(b"moov") < at(b"mdat"), "moov before mdat");
    let out = Command::new(tool("ffmpeg"))
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-f", "null", "-"])
        .output()
        .expect("run ffmpeg");
    assert!(
        out.status.success() && out.stderr.is_empty(),
        "ffmpeg: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn the_builtin_hevc_archive_preset_exports_10_bit_main10_tagged_hvc1() {
    let rig = Rig::new("hevc");
    let (path, message) = rig.export("archive-hevc", "archive.mp4");
    conform(
        &path,
        &message,
        &Expect {
            codec: "hevc",
            profile: "Main 10",
            pix_fmt: "yuv420p10le",
            tag: "hvc1",
            audio: "aac",
            encoder: "libx265",
        },
    );
    rig.close();
}

#[test]
fn the_builtin_av1_preset_exports_av1_in_mp4() {
    let rig = Rig::new("av1");
    let (path, message) = rig.export("av1-small", "small.mp4");
    conform(
        &path,
        &message,
        &Expect {
            codec: "av1",
            profile: "Main",
            pix_fmt: "yuv420p",
            tag: "av01",
            audio: "aac",
            encoder: "libsvtav1",
        },
    );
    rig.close();
}

#[test]
fn the_builtin_prores_preset_exports_422_hq_in_mov_with_24_bit_pcm() {
    let rig = Rig::new("prores");
    let (path, message) = rig.export("prores-422-hq", "master.mov");
    conform(
        &path,
        &message,
        &Expect {
            codec: "prores",
            profile: "HQ",
            pix_fmt: "yuv422p10le",
            tag: "apch",
            audio: "pcm_s24le",
            encoder: "prores_ks",
        },
    );
    rig.close();
}

#[test]
fn the_default_export_is_still_h264_aac_mp4_and_a_user_preset_can_ask_for_prores_4444() {
    let rig = Rig::new("h264-4444");
    let path = rig.dir.file("default.mp4");
    let started = must(
        &rig.conn,
        "export.start",
        json!({"path": path, "preset": {"custom": {"width": SIZE.0, "height": SIZE.1}}, "hw": "off"}),
    );
    let info = wait(&rig.conn, &started["job_id"], WAIT);
    assert_succeeded(&info);
    conform(
        Path::new(&path),
        &info.progress.message.unwrap_or_default(),
        &Expect {
            codec: "h264",
            profile: "High",
            pix_fmt: "yuv420p",
            tag: "avc1",
            audio: "aac",
            encoder: "libx264",
        },
    );
    // ProRes 4444 with alpha, saved as the user's own preset.
    must(
        &rig.conn,
        "export.save_preset",
        json!({"preset": {
            "schema_version": 1, "id": "prores-4444-alpha", "name": "ProRes 4444 alpha",
            "container": "mov",
            "video": {"codec": "prores", "rate_control": {"mode": "profile"},
                      "profile": "4444", "pix_fmt": "yuva444p10le", "hw_preferred": false},
            "audio": {"codec": "pcm", "sample_rate": 48000, "channels": 2},
            "size": {"kind": "follow_sequence"}, "fps": {"kind": "follow_sequence"}
        }}),
    );
    let (path, message) = rig.export("prores-4444-alpha", "alpha.mov");
    let info = probe(&path);
    assert!(message.contains("prores_ks"), "{message}");
    assert_eq!(get(&info, "streams_stream_0_profile"), "4444");
    // The 4444 decoder reports 12-bit planes with alpha.
    assert_eq!(get(&info, "streams_stream_0_pix_fmt"), "yuva444p12le");
    assert_eq!(get(&info, "streams_stream_0_codec_tag_string"), "ap4h");
    rig.close();
}

#[test]
fn export_list_encoders_lists_the_codecs_and_a_forced_encoder_must_match_the_codec() {
    let rig = Rig::new("encoders");
    let out = must(&rig.conn, "export.list_encoders", json!({}));
    let codecs = out["codecs"].as_array().unwrap();
    let names: Vec<&str> = codecs
        .iter()
        .map(|c| c["codec"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["h264", "hevc", "av1", "prores"]);
    let software: Vec<&str> = codecs
        .iter()
        .map(|c| c["software"].as_str().unwrap_or("none"))
        .collect();
    assert_eq!(software, ["libx264", "libx265", "libsvtav1", "prores_ks"]);
    for c in codecs {
        assert_eq!(c["available"], json!(true), "{c}");
        // Every hardware encoder listed passed the trial encode and is named after its codec.
        for hw in c["hardware"].as_array().unwrap() {
            let name = hw["implementation"].as_str().unwrap();
            assert!(name.starts_with(c["codec"].as_str().unwrap()), "{name}");
        }
    }
    // An H.264 encoder cannot be forced onto an HEVC export; a made-up one is refused too.
    for name in ["h264_nvenc", "hevc_nosuch"] {
        let path = rig.dir.file("forced.mp4");
        let err = rig
            .conn
            .call(Call::new(
                "export.start",
                json!({"path": path, "preset": "archive-hevc", "hw": {"force": name}}),
            ))
            .unwrap_err();
        assert_eq!(err.code(), "invalid", "{name}: {err}");
    }
    rig.close();
}
