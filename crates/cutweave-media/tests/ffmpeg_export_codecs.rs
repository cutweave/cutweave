// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2G.02 acceptance, encoder side: HEVC, AV1 and ProRes exports through the FFmpeg backend,
//! checked with `ffprobe` (codec, profile, pixel format, color tags, duration, audio) and by
//! decoding them back (PSNR against the input). **Software encoders only** (`hw: Off`): the
//! output is deterministic and no GPU is needed. Hardware encoders are probed and listed
//! (`hardware_encoders_are_listed_per_codec`) and used by `auto` when present, but no test here
//! requires one. Inputs are generated (no media files, no network).
//!
//! `ffprobe` / `ffmpeg` come from the bundle (`$FFMPEG_DIR/bin`, else `PATH`), as set up by
//! `scripts/ffmpeg/env.sh` (docs/dev/ffmpeg.md).
#![cfg(feature = "ffmpeg")]
#![allow(clippy::expect_used, clippy::unwrap_used)] // test helpers

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use cutweave_media::testing::contract::TempDir;
use cutweave_media::testing::{FakeAudio, FakeVideo};
use cutweave_media::{
    AudioChunk, AudioEncodeSpec, DecodeOpts, EncodeSpec, FfmpegBackend, HwEncode, HwMode,
    MediaBackend, MediaError, PixelFormat, RateControl, SpeedPreset, VideoEncodeSpec, VideoFrame,
};
use cutweave_model::{Duration, FrameRate, SampleRate};

const SIZE: (u32, u32) = (160, 96);
const RATE: FrameRate = FrameRate::FPS_25;
const FRAMES: u64 = 25;

fn backend() -> FfmpegBackend {
    FfmpegBackend::new().expect("the loaded FFmpeg matches the build")
}

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

/// `ffprobe -show_format -show_streams` as flat `key -> value` pairs.
fn probe(path: &Path) -> BTreeMap<String, String> {
    let out = Command::new(tool("ffprobe"))
        .args([
            "-v",
            "error",
            "-show_format",
            "-show_streams",
            "-of",
            "flat=s=_",
        ])
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
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.to_owned(), v.trim_matches('"').to_owned()))
        .collect()
}

fn get<'a>(info: &'a BTreeMap<String, String>, key: &str) -> &'a str {
    info.get(key).map_or_else(
        || panic!("ffprobe has no `{key}`: {info:#?}"),
        String::as_str,
    )
}

/// `ffmpeg -v error -i <path> -f null -` exits 0 with nothing on stderr.
fn assert_decodes_cleanly(path: &Path) {
    let out = Command::new(tool("ffmpeg"))
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-f", "null", "-"])
        .output()
        .expect("run ffmpeg");
    assert!(out.status.success(), "ffmpeg exit {:?}", out.status);
    assert!(
        out.stderr.is_empty(),
        "ffmpeg stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// One row of the conformance matrix: what to encode and what `ffprobe` must read back.
struct Case {
    name: &'static str,
    codec: &'static str,
    file: &'static str,
    bit_depth: u8,
    profile: Option<&'static str>,
    alpha: bool,
    /// Audio codec the file carries.
    audio: &'static str,
    // Expected:
    codec_name: &'static str,
    ff_profile: &'static str,
    pix_fmt: &'static str,
    /// The sample entry's FourCC (`codec_tag_string`).
    tag: &'static str,
}

const AAC: &str = "aac";
const PCM24: &str = "pcm_s24le";

impl Case {
    const fn new(name: &'static str, codec: &'static str, file: &'static str) -> Case {
        Case {
            name,
            codec,
            file,
            bit_depth: 8,
            profile: None,
            alpha: false,
            audio: AAC,
            codec_name: "",
            ff_profile: "",
            pix_fmt: "yuv420p",
            tag: "",
        }
    }
}

/// Encodes `FRAMES` generated RGBA frames and a stereo 48 kHz tone as `case` says (software
/// encoders) and returns the file.
fn encode(dir: &TempDir, case: &Case) -> PathBuf {
    let path = dir.path().join(case.file);
    let mut video =
        VideoEncodeSpec::h264(SIZE.0, SIZE.1, RATE, PixelFormat::Rgba8).with_codec(case.codec);
    video.bit_depth = case.bit_depth;
    video.profile = case.profile.map(str::to_owned);
    video.alpha = case.alpha;
    video.rate_control = RateControl::Crf(24);
    video.preset = SpeedPreset::Veryfast;
    video.threads = 2;
    let audio = AudioEncodeSpec {
        codec: case.audio.to_owned(),
        ..AudioEncodeSpec::aac(SampleRate::HZ_48_000, 2)
    };
    let spec = EncodeSpec::new(&path)
        .with_video(video)
        .with_audio(audio)
        .with_hw(HwEncode::Off);
    let mut enc = backend()
        .open_encoder(&spec)
        .unwrap_or_else(|e| panic!("{}: open: {e}", case.name));
    let source = FakeVideo::new(SIZE.0, SIZE.1, RATE, FRAMES).with_format(PixelFormat::Rgba8);
    let tone = FakeAudio::tone(SampleRate::HZ_48_000, 2, Duration::from_secs(1).unwrap());
    let per_frame = 48_000 / 25;
    for n in 0..FRAMES {
        enc.push_video(&source.frame(n, PixelFormat::Rgba8).unwrap())
            .unwrap_or_else(|e| panic!("{}: push_video {n}: {e}", case.name));
        let first = n * per_frame;
        enc.push_audio(&AudioChunk {
            pts: tone
                .rate
                .samples_to_time(i64::try_from(first).unwrap())
                .unwrap(),
            rate: tone.rate,
            channels: 2,
            planes: (0..2)
                .map(|c| {
                    (first..first + per_frame)
                        .map(|i| tone.sample(c, i))
                        .collect()
                })
                .collect(),
        })
        .unwrap_or_else(|e| panic!("{}: push_audio {n}: {e}", case.name));
    }
    let summary = enc
        .finish()
        .unwrap_or_else(|e| panic!("{}: finish: {e}", case.name));
    assert_eq!(summary.video_frames, FRAMES, "{}", case.name);
    path
}

/// The top-level boxes of an MP4/MOV file, in order.
fn boxes(bytes: &[u8]) -> Vec<String> {
    let mut kinds = Vec::new();
    let mut at = 0;
    while at + 8 <= bytes.len() {
        let size = match u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) {
            0 => bytes.len() - at,
            1 => usize::try_from(u64::from_be_bytes(
                bytes[at + 8..at + 16].try_into().unwrap(),
            ))
            .unwrap(),
            n => n as usize,
        };
        kinds.push(String::from_utf8_lossy(&bytes[at + 4..at + 8]).into_owned());
        assert!(size >= 8, "bad box at {at}");
        at += size;
    }
    kinds
}

/// Runs the matrix row: the file is valid, and `ffprobe` reads the codec, profile, pixel
/// format, BT.709 color tags, duration and audio the row names.
fn conform(case: &Case) {
    let dir = TempDir::new(case.name);
    let path = encode(&dir, case);
    let info = probe(&path);
    let v = |key: &str| get(&info, &format!("streams_stream_0_{key}")).to_owned();
    let a = |key: &str| get(&info, &format!("streams_stream_1_{key}")).to_owned();
    assert_eq!(v("codec_name"), case.codec_name, "{}: codec", case.name);
    assert_eq!(v("profile"), case.ff_profile, "{}: profile", case.name);
    assert_eq!(v("pix_fmt"), case.pix_fmt, "{}: pix_fmt", case.name);
    assert_eq!(
        v("codec_tag_string"),
        case.tag,
        "{}: sample entry",
        case.name
    );
    assert_eq!(
        (v("width"), v("height")),
        (SIZE.0.to_string(), SIZE.1.to_string())
    );
    // Rec.709 written correctly by default: primaries, transfer, matrix and limited range.
    assert_eq!(v("color_primaries"), "bt709", "{}", case.name);
    assert_eq!(v("color_transfer"), "bt709", "{}", case.name);
    assert_eq!(v("color_space"), "bt709", "{}", case.name);
    assert_eq!(v("color_range"), "tv", "{}", case.name);
    // Audio.
    assert_eq!(a("codec_name"), case.audio, "{}: audio codec", case.name);
    assert_eq!(a("channels"), "2", "{}", case.name);
    assert_eq!(a("sample_rate"), "48000", "{}", case.name);
    // Duration within one frame of the input's one second.
    let duration: f64 = get(&info, "format_duration").parse().unwrap();
    assert!(
        (duration - 1.0).abs() <= 0.04 + 1e-9,
        "{}: duration {duration}",
        case.name
    );
    // The index is in front of the media data (faststart).
    let kinds = boxes(&std::fs::read(&path).unwrap());
    let at = |kind: &str| kinds.iter().position(|k| k == kind);
    assert!(
        at("moov").unwrap() < at("mdat").unwrap(),
        "{}: moov must come before mdat: {kinds:?}",
        case.name
    );
    assert_decodes_cleanly(&path);
}

#[test]
fn h264_mp4() {
    conform(&Case {
        codec_name: "h264",
        ff_profile: "High",
        tag: "avc1",
        ..Case::new("h264-mp4", "h264", "out.mp4")
    });
}

#[test]
fn hevc_mp4_is_tagged_hvc1() {
    conform(&Case {
        codec_name: "hevc",
        ff_profile: "Main",
        tag: "hvc1",
        ..Case::new("hevc-mp4", "hevc", "out.mp4")
    });
}

#[test]
fn hevc_mp4_10_bit() {
    conform(&Case {
        bit_depth: 10,
        codec_name: "hevc",
        ff_profile: "Main 10",
        pix_fmt: "yuv420p10le",
        tag: "hvc1",
        ..Case::new("hevc10-mp4", "hevc", "out.mp4")
    });
}

#[test]
fn hevc_in_mov_is_tagged_hvc1_too() {
    conform(&Case {
        codec_name: "hevc",
        ff_profile: "Main",
        tag: "hvc1",
        ..Case::new("hevc-mov", "hevc", "out.mov")
    });
}

#[test]
fn av1_mp4() {
    conform(&Case {
        codec_name: "av1",
        ff_profile: "Main",
        tag: "av01",
        ..Case::new("av1-mp4", "av1", "out.mp4")
    });
}

#[test]
fn av1_mp4_10_bit() {
    conform(&Case {
        bit_depth: 10,
        codec_name: "av1",
        ff_profile: "Main",
        pix_fmt: "yuv420p10le",
        tag: "av01",
        ..Case::new("av1-10-mp4", "av1", "out.mp4")
    });
}

/// A ProRes row: 10-bit 4:2:2 (4:4:4 for the 4444 profiles), 24-bit PCM in MOV.
fn prores(name: &'static str, profile: &'static str, ff: &'static str, tag: &'static str) -> Case {
    Case {
        profile: Some(profile),
        audio: PCM24,
        codec_name: "prores",
        ff_profile: ff,
        pix_fmt: "yuv422p10le",
        tag,
        ..Case::new(name, "prores", "out.mov")
    }
}

#[test]
fn prores_422_profiles() {
    for case in [
        prores("prores-proxy", "422proxy", "Proxy", "apco"),
        prores("prores-lt", "422lt", "LT", "apcs"),
        prores("prores-422", "422", "Standard", "apcn"),
        prores("prores-hq", "422hq", "HQ", "apch"),
    ] {
        conform(&case);
    }
}

#[test]
fn prores_4444_and_4444_xq_with_and_without_alpha() {
    conform(&Case {
        pix_fmt: "yuv444p12le",
        ..prores("prores-4444", "4444", "4444", "ap4h")
    });
    conform(&Case {
        pix_fmt: "yuva444p12le",
        alpha: true,
        ..prores("prores-4444-alpha", "4444", "4444", "ap4h")
    });
    conform(&Case {
        pix_fmt: "yuv444p12le",
        ..prores("prores-4444xq", "4444xq", "XQ", "ap4x")
    });
}

#[test]
fn hevc_and_av1_reject_what_they_cannot_hold() {
    let dir = TempDir::new("reject");
    let b = backend();
    let open = |name: &str, codec: &str, tweak: &dyn Fn(&mut VideoEncodeSpec)| {
        let mut video =
            VideoEncodeSpec::h264(SIZE.0, SIZE.1, RATE, PixelFormat::Rgba8).with_codec(codec);
        tweak(&mut video);
        b.open_encoder(
            &EncodeSpec::new(dir.path().join(name))
                .with_video(video)
                .with_hw(HwEncode::Off),
        )
        .err()
    };
    // H.264 is 8-bit; AV1 goes into MP4 only; ProRes into MOV only; alpha needs 4444.
    assert!(matches!(
        open("a.mp4", "h264", &|v| v.bit_depth = 10),
        Some(MediaError::Unsupported { .. })
    ));
    assert!(matches!(
        open("a.mov", "av1", &|_| {}),
        Some(MediaError::Unsupported { .. })
    ));
    assert!(matches!(
        open("a.mp4", "prores", &|_| {}),
        Some(MediaError::Unsupported { .. })
    ));
    assert!(matches!(
        open("b.mov", "prores", &|v| v.alpha = true),
        Some(MediaError::Unsupported { .. })
    ));
    assert!(matches!(
        open("c.mov", "prores", &|v| v.profile = Some("nope".into())),
        Some(MediaError::Unsupported { .. })
    ));
    assert!(matches!(
        open("b.mp4", "hevc", &|v| v.profile = Some("main10".into())),
        Some(MediaError::Unsupported { .. })
    ));
    assert!(matches!(
        open("c.mp4", "hevc", &|v| v.bit_depth = 12),
        Some(MediaError::Invalid { .. })
    ));
    // Odd sizes for 4:2:0 are `Invalid`, like H.264.
    let mut odd = VideoEncodeSpec::h264(161, 96, RATE, PixelFormat::Rgba8).with_codec("hevc");
    odd.threads = 1;
    assert!(matches!(
        b.open_encoder(&EncodeSpec::new(dir.path().join("d.mp4")).with_video(odd))
            .err(),
        Some(MediaError::Invalid { .. })
    ));
    // Nothing was left behind.
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

/// Peak-signal-to-noise ratio of the luma plane of `got` against `want` in dB; 10-bit samples
/// are 16-bit little-endian words.
fn luma_psnr(want: &VideoFrame, got: &VideoFrame) -> f64 {
    assert_eq!(want.format, got.format, "same layout");
    let (w, h) = (want.width as usize, want.height as usize);
    let (bytes, peak) = if want.format == PixelFormat::Yuv420p10 {
        (2, 1023.0_f64)
    } else {
        (1, 255.0_f64)
    };
    let mut sum = 0.0_f64;
    for y in 0..h {
        let a = want.row(0, y as u32).unwrap();
        let b = got.row(0, y as u32).unwrap();
        for x in 0..w {
            let s = |row: &[u8]| {
                if bytes == 2 {
                    f64::from(u16::from_le_bytes([row[2 * x], row[2 * x + 1]]))
                } else {
                    f64::from(row[x])
                }
            };
            let d = s(a) - s(b);
            sum += d * d;
        }
    }
    let mse = sum / (w * h) as f64;
    if mse == 0.0 {
        return 99.0;
    }
    10.0 * (peak * peak / mse).log10()
}

/// The CRF-preset acceptance: HEVC decodes against its input at PSNR >= 40 dB, 8 and 10 bit
/// (the archive preset is 10-bit CRF 16), frame by frame on the frame grid.
#[test]
fn hevc_at_the_archive_crf_decodes_at_40_db_or_better() {
    for (format, bit_depth, crf) in [
        (PixelFormat::Yuv420p, 8, 16),
        (PixelFormat::Yuv420p10, 10, 16),
    ] {
        let dir = TempDir::new("psnr");
        let path = dir.path().join("out.mp4");
        let source = FakeVideo::new(SIZE.0, SIZE.1, RATE, FRAMES).with_format(format);
        let mut video = VideoEncodeSpec::h264(SIZE.0, SIZE.1, RATE, format).with_codec("hevc");
        video.bit_depth = bit_depth;
        video.rate_control = RateControl::Crf(crf);
        video.preset = SpeedPreset::Medium;
        video.threads = 1;
        let mut enc = backend()
            .open_encoder(
                &EncodeSpec::new(&path)
                    .with_video(video)
                    .with_hw(HwEncode::Off),
            )
            .expect("open");
        assert_eq!(enc.video_encoder(), Some("libx265"));
        for n in 0..FRAMES {
            enc.push_video(&source.frame(n, format).unwrap()).unwrap();
        }
        enc.finish().unwrap();
        let mut dec = backend()
            .open_decoder(&path, &DecodeOpts::default().with_hw(HwMode::Off))
            .unwrap();
        let mut worst = f64::MAX;
        for n in 0..FRAMES {
            let got = dec.next_frame().unwrap().expect("a frame");
            assert_eq!(
                got.pts,
                RATE.frame_to_time(i64::try_from(n).unwrap()).unwrap(),
                "frame {n} sits on the grid"
            );
            worst = worst.min(luma_psnr(&source.frame(n, format).unwrap(), &got));
        }
        assert!(
            worst >= 40.0,
            "{bit_depth}-bit HEVC: worst PSNR {worst:.1} dB"
        );
    }
}

#[test]
fn a_forced_hardware_encoder_that_does_not_exist_fails_and_auto_still_encodes() {
    let dir = TempDir::new("force");
    let b = backend();
    let video =
        || VideoEncodeSpec::h264(SIZE.0, SIZE.1, RATE, PixelFormat::Rgba8).with_codec("hevc");
    let forced = EncodeSpec::new(dir.path().join("a.mp4"))
        .with_video(video())
        .with_hw(HwEncode::Force("hevc_nosuch".into()));
    assert!(matches!(
        b.open_encoder(&forced).err(),
        Some(MediaError::HwUnavailable { .. })
    ));
    // An H.264 encoder cannot be forced onto an HEVC stream.
    let wrong = EncodeSpec::new(dir.path().join("b.mp4"))
        .with_video(video())
        .with_hw(HwEncode::Force("h264_nvenc".into()));
    assert!(matches!(
        b.open_encoder(&wrong).err(),
        Some(MediaError::HwUnavailable { .. })
    ));
    // Auto uses a hardware encoder when this machine has a working one, else x265.
    let auto = EncodeSpec::new(dir.path().join("c.mp4")).with_video(video());
    let enc = b.open_encoder(&auto).expect("auto opens");
    let name = enc.video_encoder().unwrap().to_owned();
    let hw: Vec<String> = b
        .capabilities()
        .hw_encode
        .iter()
        .filter(|c| c.codec == "hevc")
        .map(|c| c.implementation.clone())
        .collect();
    assert!(
        name == "libx265" && hw.is_empty() || hw.contains(&name) || name == "libx265",
        "{name} vs {hw:?}"
    );
}

#[test]
fn hardware_encoders_are_listed_per_codec() {
    let caps = backend().capabilities();
    for codec in ["h264", "hevc", "av1", "prores"] {
        assert!(caps.can_encode(codec), "a software {codec} encoder");
    }
    // Whatever this machine has passed an 8-frame trial encode: named after its codec.
    for hw in &caps.hw_encode {
        assert!(
            ["h264", "hevc", "av1"].contains(&hw.codec.as_str())
                && hw.implementation.starts_with(&hw.codec),
            "{hw:?}"
        );
    }
    eprintln!("hardware encoders here: {:?}", caps.hw_encode);
}

/// Frame count check on the grid for a non-integer rate (HEVC at 29.97).
#[test]
fn ntsc_rates_keep_their_frame_grid() {
    let dir = TempDir::new("ntsc");
    let path = dir.path().join("out.mp4");
    let rate = FrameRate::FPS_29_97;
    let mut video =
        VideoEncodeSpec::h264(SIZE.0, SIZE.1, rate, PixelFormat::Rgba8).with_codec("hevc");
    video.preset = SpeedPreset::Ultrafast;
    let source = FakeVideo::new(SIZE.0, SIZE.1, rate, 30).with_format(PixelFormat::Rgba8);
    let mut enc = backend()
        .open_encoder(
            &EncodeSpec::new(&path)
                .with_video(video)
                .with_hw(HwEncode::Off),
        )
        .unwrap();
    for n in 0..30 {
        enc.push_video(&source.frame(n, PixelFormat::Rgba8).unwrap())
            .unwrap();
    }
    let summary = enc.finish().unwrap();
    assert_eq!(summary.video_frames, 30);
    let want = rate.frame_to_time(30).unwrap();
    assert_eq!(summary.duration.flicks(), want.flicks());
    let info = probe(&path);
    assert_eq!(get(&info, "streams_stream_0_nb_frames"), "30");
    assert_eq!(get(&info, "streams_stream_0_avg_frame_rate"), "30000/1001");
}

/// Hardware: every HEVC and AV1 encoder this machine passed the trial encode with writes a
/// valid file at 8 bit (`ffprobe` codec and `hvc1`, a clean decode); 10 bit is tried too and
/// reported (a GPU may not have it). Never part of the default run: no test requires a GPU.
#[test]
#[ignore = "hardware: CUTWEAVE_HW_TESTS=1, --run-ignored all"]
fn every_hardware_hevc_and_av1_encoder_writes_a_valid_file() {
    if std::env::var("CUTWEAVE_HW_TESTS").as_deref() != Ok("1") {
        eprintln!("skipping: set CUTWEAVE_HW_TESTS=1 on a machine with a hardware encoder");
        return;
    }
    let b = backend();
    let found: Vec<_> = b
        .capabilities()
        .hw_encode
        .into_iter()
        .filter(|c| c.codec == "hevc" || c.codec == "av1")
        .collect();
    eprintln!("hardware HEVC/AV1 encoders here: {found:?}");
    // Above every hardware encoder's minimum size.
    let (w, h) = (320, 256);
    for hw in found {
        for bit_depth in [8_u8, 10] {
            let dir = TempDir::new("hw-codec");
            let path = dir.path().join("out.mp4");
            let mut video =
                VideoEncodeSpec::h264(w, h, RATE, PixelFormat::Rgba8).with_codec(&hw.codec);
            video.bit_depth = bit_depth;
            let spec = EncodeSpec::new(&path)
                .with_video(video)
                .with_hw(HwEncode::Force(hw.implementation.clone()));
            let mut enc = match b.open_encoder(&spec) {
                Ok(enc) => enc,
                Err(e) if bit_depth == 10 => {
                    eprintln!("{} at 10 bit: not available ({e})", hw.implementation);
                    continue;
                }
                Err(e) => panic!("{} at 8 bit: {e}", hw.implementation),
            };
            let source = FakeVideo::new(w, h, RATE, FRAMES).with_format(PixelFormat::Rgba8);
            for n in 0..FRAMES {
                enc.push_video(&source.frame(n, PixelFormat::Rgba8).unwrap())
                    .unwrap_or_else(|e| panic!("{} push {n}: {e}", hw.implementation));
            }
            let summary = enc
                .finish()
                .unwrap_or_else(|e| panic!("{} finish: {e}", hw.implementation));
            assert_eq!(summary.video_frames, FRAMES);
            let info = probe(&path);
            assert_eq!(get(&info, "streams_stream_0_codec_name"), hw.codec);
            assert_eq!(get(&info, "streams_stream_0_color_space"), "bt709");
            if hw.codec == "hevc" {
                assert_eq!(get(&info, "streams_stream_0_codec_tag_string"), "hvc1");
            }
            assert_decodes_cleanly(&path);
            eprintln!("{} at {bit_depth} bit: ok", hw.implementation);
        }
    }
}
