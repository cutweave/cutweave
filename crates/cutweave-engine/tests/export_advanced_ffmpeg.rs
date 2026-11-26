// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2G.05 acceptance through the engine: `export.start{advanced}` writes files `ffprobe` reads
//! back as the advanced block says (keyframe interval from seconds, color tags, audio codec,
//! channels and rate), a preset's own `advanced` block applies and a request overrides it,
//! impossible combinations are refused with a hint before any job exists, and leaving `advanced`
//! out changes nothing (the decoded pictures are identical).
//!
//! **Software encoders only** (`hw: "off"`, or a setting that needs software): the matrix is
//! deterministic and needs no GPU; the renderer is the GPU-free probe renderer. Needs the FFmpeg
//! bundle (`. scripts/ffmpeg/env.sh`).
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
use serde_json::{Value, json};

const WAIT: StdDuration = StdDuration::from_secs(600);
const SIZE: (u32, u32) = (320, 180);
/// 2 s of the first clip then 1 s of the second: 3 s at 30 fps.
const FRAMES: usize = 90;

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

fn ffprobe(path: &Path, extra: &[&str]) -> BTreeMap<String, String> {
    let out = Command::new(tool("ffprobe"))
        .args(["-v", "error", "-show_format", "-show_streams"])
        .args(extra)
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

/// The frame numbers (presentation order) of the video keyframes.
fn keyframes(path: &Path) -> Vec<usize> {
    let info = ffprobe(
        path,
        &[
            "-select_streams",
            "v:0",
            "-show_packets",
            "-show_entries",
            "packet=pts_time,flags",
        ],
    );
    let mut rows: BTreeMap<usize, (f64, bool)> = BTreeMap::new();
    for (k, v) in &info {
        let Some(rest) = k.strip_prefix("packets_packet_") else {
            continue;
        };
        let (index, field) = rest.split_once('_').unwrap();
        let row = rows.entry(index.parse().unwrap()).or_insert((0.0, false));
        match field {
            "pts_time" => row.0 = v.parse().unwrap(),
            "flags" => row.1 = v.starts_with('K'),
            _ => {}
        }
    }
    let mut all: Vec<(f64, bool)> = rows.into_values().collect();
    all.sort_by(|a, b| a.0.total_cmp(&b.0));
    all.iter()
        .enumerate()
        .filter(|(_, p)| p.1)
        .map(|(i, _)| i)
        .collect()
}

/// The MD5 of the decoded pictures (what a frame-hash regression compares).
fn picture_hash(path: &Path) -> String {
    let out = Command::new(tool("ffmpeg"))
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-an", "-f", "md5", "-"])
        .output()
        .expect("run ffmpeg");
    assert!(out.status.success(), "ffmpeg exit {:?}", out.status);
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
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
        let svc = ExportServices::new(
            Arc::new(FfmpegBackend::new().expect("the loaded FFmpeg matches the build")),
            Arc::new(DecoderSources::new(Arc::new(fake.clone()))),
            ProbeRenderer::new(Arc::new(LateSource::default()) as _).shared(),
        )
        // One encoder thread: x264 output is then identical run to run.
        .with_encoder_threads(1);
        let engine = engine(dir.path(), Some(svc));
        let conn = agent(&engine);
        let (seq, video, _) = new_project(&conn);
        let ids: Vec<_> = paths.iter().map(|p| register(&conn, &fake, p)).collect();
        insert(&conn, &seq, &video, ids[0], "0s", "2s", "0s");
        insert(&conn, &seq, &video, ids[1], "2s", "1s", "1s");
        Rig { dir, engine, conn }
    }

    /// `export.start` with `params` plus the path and size; returns the start answer and the
    /// finished file.
    fn export(&self, file: &str, mut params: Value) -> (Value, PathBuf) {
        let path = self.dir.file(file);
        params["path"] = json!(path);
        let started = must(&self.conn, "export.start", params);
        let info = wait(&self.conn, &started["job_id"], WAIT);
        assert_succeeded(&info);
        (started, PathBuf::from(path))
    }

    /// The error of `export.start` with `params` (the path is filled in).
    fn refused(&self, mut params: Value) -> cutweave_commands::CmdError {
        params["path"] = json!(self.dir.file("refused.mp4"));
        self.conn
            .call(Call::new("export.start", params))
            .expect_err("the export must be refused")
    }

    fn close(self) {
        drop(self.conn);
        self.engine.shutdown();
    }
}

fn custom() -> Value {
    json!({"custom": {"width": SIZE.0, "height": SIZE.1}})
}

#[test]
fn an_advanced_block_sets_keyframes_color_and_audio() {
    let rig = Rig::new("adv-all");
    let (started, path) = rig.export(
        "all.mp4",
        json!({
            "preset": custom(),
            "hw": "off",
            "advanced": {
                "video": {
                    "rate_control": {"mode": "vbr", "bitrate_kbps": 2000, "max_bitrate_kbps": 3000},
                    "gop": {"keyint_seconds": 1, "scenecut": false, "closed_gop": true},
                    "preset": "veryfast",
                    "profile": "main"
                },
                "color": {"primaries": "smpte170m", "transfer": "smpte170m",
                          "matrix": "smpte170m", "range": "limited"},
                "audio": {"codec": "opus", "bitrate_kbps": 96, "channels": "mono"}
            }
        }),
    );
    // 2 Mb/s video + 96 kb/s audio over 3 s is about 770 KiB.
    let kib = started["estimated_kib"].as_u64().unwrap();
    assert!((700..=800).contains(&kib), "estimated {kib} KiB");
    let info = ffprobe(&path, &[]);
    let v = |k: &str| get(&info, &format!("streams_stream_0_{k}")).to_owned();
    let a = |k: &str| get(&info, &format!("streams_stream_1_{k}")).to_owned();
    assert_eq!(
        (v("codec_name"), v("profile")),
        ("h264".into(), "Main".into())
    );
    // keyint_seconds 1 at 30 fps: a keyframe every 30 frames.
    assert_eq!(keyframes(&path), [0, 30, 60]);
    assert_eq!(
        (
            v("color_primaries"),
            v("color_transfer"),
            v("color_space"),
            v("color_range")
        ),
        (
            "smpte170m".into(),
            "smpte170m".into(),
            "smpte170m".into(),
            "tv".into()
        )
    );
    assert_eq!(a("codec_name"), "opus");
    assert_eq!(
        (a("channels"), a("sample_rate")),
        ("1".into(), "48000".into())
    );
    assert_eq!(v("nb_frames"), FRAMES.to_string());
    rig.close();
}

#[test]
fn audio_sample_rate_and_channels_are_applied_to_the_mix() {
    let rig = Rig::new("adv-audio");
    for (codec, rate, channels, name) in [
        ("aac", 44_100, "mono", "aac"),
        ("flac", 96_000, "stereo", "flac"),
    ] {
        let (_, path) = rig.export(
            &format!("{codec}.mp4"),
            json!({
                "preset": custom(),
                "hw": "off",
                "advanced": {"audio": {"codec": codec, "sample_rate": rate, "channels": channels}}
            }),
        );
        let info = ffprobe(&path, &[]);
        let a = |k: &str| get(&info, &format!("streams_stream_1_{k}")).to_owned();
        assert_eq!(a("codec_name"), name);
        assert_eq!(a("sample_rate"), rate.to_string());
        assert_eq!(a("channels"), if channels == "mono" { "1" } else { "2" });
        // The mix is as long as the picture: 3 s within a frame.
        let duration: f64 = a("duration").parse().unwrap();
        assert!(
            (duration - 3.0).abs() <= 0.05,
            "{codec}: audio is {duration} s"
        );
    }
    rig.close();
}

#[test]
fn a_preset_carries_an_advanced_block_and_a_request_overrides_it() {
    let rig = Rig::new("adv-preset");
    must(
        &rig.conn,
        "export.save_preset",
        json!({"preset": {
            "schema_version": 1, "id": "tight-gop", "name": "Tight GOP",
            "container": "mp4",
            "video": {"codec": "h264", "rate_control": {"mode": "crf", "crf": 28},
                      "hw_preferred": false},
            "audio": {"codec": "aac", "bitrate_kbps": 128, "sample_rate": 48000, "channels": 2},
            "size": {"kind": "follow_sequence"}, "fps": {"kind": "follow_sequence"},
            "advanced": {"video": {"gop": {"keyint": 15, "scenecut": false},
                                   "tune": "film"}}
        }}),
    );
    let (_, path) = rig.export(
        "preset.mp4",
        json!({"preset": {"custom": {"preset": "tight-gop", "width": SIZE.0, "height": SIZE.1}}}),
    );
    assert_eq!(keyframes(&path), [0, 15, 30, 45, 60, 75]);
    // The request's GOP replaces the preset's.
    let (_, path) = rig.export(
        "override.mp4",
        json!({
            "preset": {"custom": {"preset": "tight-gop", "width": SIZE.0, "height": SIZE.1}},
            "advanced": {"video": {"gop": {"keyint": 45, "scenecut": false}}}
        }),
    );
    assert_eq!(keyframes(&path), [0, 45]);
    // A preset whose advanced block cannot work is refused when it is saved.
    let err = rig
        .conn
        .call(Call::new(
            "export.save_preset",
            json!({"preset": {
                "schema_version": 1, "id": "bad-prores", "name": "Bad",
                "container": "mov",
                "video": {"codec": "prores", "rate_control": {"mode": "profile"},
                          "profile": "422hq", "hw_preferred": false},
                "audio": {"codec": "pcm", "sample_rate": 48000, "channels": 2},
                "size": {"kind": "follow_sequence"}, "fps": {"kind": "follow_sequence"},
                "advanced": {"video": {"rate_control": {"mode": "crf", "crf": 18}}}
            }}),
        ))
        .unwrap_err();
    assert_eq!(err.code(), "invalid", "{err}");
    assert!(
        err.to_string().contains("/advanced/video/rate_control"),
        "{err}"
    );
    rig.close();
}

#[test]
fn impossible_combinations_are_refused_with_a_hint_and_start_nothing() {
    let rig = Rig::new("adv-refused");
    let cases: [(Value, &str, &str, &str); 5] = [
        (
            json!({"preset": "prores-422-hq", "advanced": {"video": {"rate_control": {"mode": "crf", "crf": 18}}}}),
            "invalid",
            "/advanced/video/rate_control",
            "profile",
        ),
        (
            json!({"preset": "av1-small", "advanced": {"video": {"rate_control": {"mode": "cbr", "bitrate_kbps": 2000}}}}),
            "invalid",
            "/advanced/video/rate_control/mode",
            "vbr",
        ),
        (
            json!({"preset": custom(), "advanced": {"audio": {"codec": "opus", "sample_rate": 44100}}}),
            "invalid",
            "/advanced/audio/sample_rate",
            "48000",
        ),
        (
            json!({"preset": custom(), "hw": {"force": "h264_nvenc"}, "advanced": {"video": {"gop": {"closed_gop": true}}}}),
            "invalid",
            "/advanced/video/gop/closed_gop",
            "hw: \"off\"",
        ),
        (
            json!({"preset": custom(), "advanced": {"color": {"transfer": "smpte2084"}}}),
            "unsupported",
            "HDR",
            "SDR",
        ),
    ];
    for (params, code, pointer, hint) in cases {
        let err = rig.refused(params.clone());
        assert_eq!(err.code(), code, "{params}: {err}");
        let text = err.to_string();
        assert!(
            text.contains(pointer) && text.contains(hint),
            "{params}: {text}"
        );
    }
    // Two-pass is not available yet.
    let err = rig.refused(json!({
        "preset": custom(),
        "advanced": {"video": {"rate_control": {"mode": "vbr", "bitrate_kbps": 2000, "two_pass": true}}}
    }));
    assert_eq!(err.code(), "unsupported", "{err}");
    // No job was queued for any of them.
    let queue = must(&rig.conn, "export.queue", json!({}));
    assert_eq!(queue["jobs"].as_array().map_or(0, Vec::len), 0, "{queue}");
    rig.close();
}

#[test]
fn a_software_only_setting_moves_an_auto_export_to_the_software_encoder_with_a_warning() {
    let rig = Rig::new("adv-auto");
    let (started, path) = rig.export(
        "auto.mp4",
        json!({
            "preset": custom(),
            "hw": "auto",
            "advanced": {"video": {"gop": {"closed_gop": true, "keyint": 30, "scenecut": false}}}
        }),
    );
    let warnings = started["warnings"].as_array().unwrap();
    assert!(
        warnings.iter().any(|w| w["code"] == "software_encoder"),
        "{warnings:?}"
    );
    assert_eq!(keyframes(&path), [0, 30, 60]);
    rig.close();
}

#[test]
fn leaving_advanced_out_changes_nothing() {
    let rig = Rig::new("adv-identical");
    let hash = |file: &str, extra: Value| {
        let mut params = json!({"preset": custom(), "hw": "off"});
        if !extra.is_null() {
            params["advanced"] = extra;
        }
        let (_, path) = rig.export(file, params);
        picture_hash(&path)
    };
    let plain = hash("plain.mp4", Value::Null);
    assert_eq!(plain, hash("empty.mp4", json!({})), "an empty block");
    assert_eq!(
        plain,
        hash(
            "empty-video.mp4",
            json!({"video": {}, "color": {}, "audio": {}})
        ),
        "empty sections"
    );
    // Naming the defaults is not a change either: BT.709 limited and 48 kHz stereo AAC.
    assert_eq!(
        plain,
        hash(
            "defaults.mp4",
            json!({"color": {"primaries": "bt709", "transfer": "bt709", "matrix": "bt709", "range": "limited"},
                   "audio": {"codec": "aac", "sample_rate": 48000, "channels": "stereo"}})
        ),
        "the defaults named"
    );
    rig.close();
}

#[test]
fn describe_options_answers_per_codec_and_encoder() {
    let rig = Rig::new("adv-describe");
    let h264 = must(
        &rig.conn,
        "export.describe_options",
        json!({"codec": "h264"}),
    );
    assert_eq!(h264["encoder"], "libx264");
    assert_eq!(h264["hardware"], json!(false));
    let modes = &h264["schema"]["properties"]["video"]["properties"]["rate_control"]["properties"]
        ["mode"]["enum"];
    assert_eq!(modes, &json!(["crf", "cbr", "vbr", "cq"]));
    // AV1 has no constant bitrate; ProRes has no rate control at all.
    let av1 = must(
        &rig.conn,
        "export.describe_options",
        json!({"codec": "av1"}),
    );
    assert_eq!(
        av1["schema"]["properties"]["video"]["properties"]["rate_control"]["properties"]["mode"]["enum"],
        json!(["crf", "vbr", "cq"])
    );
    let prores = must(
        &rig.conn,
        "export.describe_options",
        json!({"codec": "prores"}),
    );
    assert!(
        prores["schema"]["properties"]["video"]["properties"]
            .get("rate_control")
            .is_none()
    );
    // A hardware encoder: the basics only.
    let nvenc = must(
        &rig.conn,
        "export.describe_options",
        json!({"codec": "hevc", "encoder": "hevc_nvenc"}),
    );
    assert_eq!(nvenc["hardware"], json!(true));
    assert!(
        nvenc["schema"]["properties"]["video"]["properties"]["gop"]["properties"]
            .get("closed_gop")
            .is_none()
    );
    // An encoder of another codec is refused.
    let err = rig
        .conn
        .call(Call::new(
            "export.describe_options",
            json!({"codec": "hevc", "encoder": "h264_nvenc"}),
        ))
        .unwrap_err();
    assert_eq!(err.code(), "invalid", "{err}");
    rig.close();
}
