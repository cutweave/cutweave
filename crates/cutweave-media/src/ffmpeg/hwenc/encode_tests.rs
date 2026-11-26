// SPDX-License-Identifier: GPL-3.0-or-later
//! Hardware encoder selection, fallback and the probe, through real FFmpeg encodes (WP-1E.02).
//!
//! The fallback cases run on every machine: a pseudo hardware encoder (libx264 under another
//! name, failing where told) stands in for a GPU. Cases that need a real hardware encoder are
//! `#[ignore]`d and also need `CUTWEAVE_HW_TESTS=1` (they skip with a message without it):
//! `CUTWEAVE_HW_TESTS=1 cargo nextest run -p cutweave-media --features ffmpeg --run-ignored all`.
#![allow(clippy::expect_used, clippy::unwrap_used)] // test helpers

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cutweave_model::FrameRate;

use super::{Choice, Codec, Family, Faults, HwEncoder, candidates, probe, with_encoders};
use crate::backend::{EncodeSpec, Encoder, HwEncode, MediaBackend, VideoEncodeSpec};
use crate::ffmpeg::encode::{open, test_encode};
use crate::testing::FakeVideo;
use crate::testing::contract::TempDir;
use crate::{CancelCheck, FfmpegBackend, MediaError, PixelFormat, encode_with_fallback};

/// Above every hardware encoder's minimum (AMF and D3D12VA refuse 160×96 here).
const SIZE: (u32, u32) = (320, 180);
const FRAMES: u64 = 30;
const RATE: FrameRate = FrameRate::FPS_30;

fn backend() -> FfmpegBackend {
    FfmpegBackend::new().expect("the loaded FFmpeg matches the build")
}

/// Whether the hardware cases run (`CUTWEAVE_HW_TESTS=1`); prints why not.
fn hw_tests() -> bool {
    let on = std::env::var("CUTWEAVE_HW_TESTS").is_ok_and(|v| v == "1");
    if !on {
        eprintln!("skipping: set CUTWEAVE_HW_TESTS=1 on a machine with a hardware encoder");
    }
    on
}

/// libx264 named `test_hw`, failing as `faults` says.
fn pseudo(faults: Faults) -> HwEncoder {
    HwEncoder {
        name: "test_hw",
        device: "test",
        family: Family::Pseudo(faults),
        codec: Codec::H264,
    }
}

fn spec(path: &Path, hw: HwEncode) -> EncodeSpec {
    EncodeSpec::new(path)
        .with_video(VideoEncodeSpec::h264(
            SIZE.0,
            SIZE.1,
            RATE,
            PixelFormat::Rgba8,
        ))
        .with_hw(hw)
}

fn push(enc: &mut dyn Encoder, frames: std::ops::Range<u64>) -> Result<(), MediaError> {
    let video = FakeVideo::new(SIZE.0, SIZE.1, RATE, FRAMES);
    for n in frames {
        enc.push_video(&video.frame(n, PixelFormat::Rgba8).expect("fake frame"))?;
    }
    Ok(())
}

fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("list")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// x264 signs its streams with an SEI message (`x264 - core …`); hardware encoders do not.
fn written_by_x264(path: &Path) -> bool {
    std::fs::read(path)
        .expect("read output")
        .windows(11)
        .any(|w| w == b"x264 - core")
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

/// Video frames in `path` by ffprobe, after checking it decodes without a message.
fn frames_in(path: &Path) -> u64 {
    let out = Command::new(tool("ffmpeg"))
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-f", "null", "-"])
        .output()
        .expect("run ffmpeg (source scripts/ffmpeg/env.sh)");
    assert!(
        out.status.success() && out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    frames_muxed(path)
}

/// Video frames the MP4 index of `path` lists (ffprobe `nb_frames`), without decoding.
fn frames_muxed(path: &Path) -> u64 {
    let out = Command::new(tool("ffprobe"))
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=nb_frames",
            "-of",
            "csv=p=0",
        ])
        .arg(path)
        .output()
        .expect("run ffprobe");
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .expect("nb_frames")
}

#[test]
fn a_hardware_encoder_that_cannot_open_is_skipped_for_x264() {
    backend();
    let dir = TempDir::new("hw-open-fails");
    let out = dir.path().join("out.mp4");
    let broken = pseudo(Faults {
        open: true,
        ..Faults::default()
    });
    with_encoders(vec![broken], || {
        // Auto: logged and skipped; x264 writes the file.
        let mut enc = open(&spec(&out, HwEncode::Auto)).expect("open");
        assert_eq!(enc.video_encoder(), Some("libx264"));
        push(&mut *enc, 0..FRAMES).expect("push");
        assert_eq!(enc.finish().expect("finish").video_frames, FRAMES);
        assert!(written_by_x264(&out));
        assert_eq!(frames_in(&out), FRAMES);
        // Forced: no fallback, no file (the earlier output stays).
        let err = open(&spec(&out, HwEncode::Force("test_hw".into()))).err();
        assert!(
            matches!(&err, Some(MediaError::HwUnavailable { reason }) if reason.contains("test_hw")),
            "{err:?}"
        );
        assert_eq!(entries(dir.path()), ["out.mp4"]);
    });
}

#[test]
fn a_mid_stream_hardware_failure_leaves_no_file_and_re_encodes_in_software() {
    let b = backend();
    let dir = TempDir::new("hw-mid-stream");
    let out = dir.path().join("out.mp4");
    std::fs::write(&out, b"the previous export").expect("write");
    let lost = pseudo(Faults {
        frame: Some(10),
        ..Faults::default()
    });
    with_encoders(vec![lost], || {
        // The encoder alone: frame 10 fails, the partial file goes, the old file stays, and
        // every later call repeats the error.
        let mut enc = b.open_encoder(&spec(&out, HwEncode::Auto)).expect("open");
        assert_eq!(enc.video_encoder(), Some("test_hw"));
        push(&mut *enc, 0..10).expect("frames before the failure");
        let err = push(&mut *enc, 10..11).expect_err("frame 10 fails");
        assert!(
            matches!(&err, MediaError::HwUnavailable { reason } if reason.contains("frame 10")),
            "{err:?}"
        );
        assert_eq!(push(&mut *enc, 11..12), Err(err.clone()));
        assert_eq!(enc.finish().err(), Some(err));
        assert_eq!(entries(dir.path()), ["out.mp4"]);
        assert_eq!(std::fs::read(&out).expect("read"), b"the previous export");

        // With the fallback helper: everything again from frame 0 with x264.
        let mut attempts = Vec::new();
        let summary = encode_with_fallback(&b, &spec(&out, HwEncode::Auto), |enc| {
            attempts.push(enc.video_encoder().expect("a video encoder").to_owned());
            push(enc, 0..FRAMES)
        })
        .expect("encoded in software");
        assert_eq!(attempts, ["test_hw", "libx264"]);
        assert_eq!(summary.video_frames, FRAMES);
        assert_eq!(entries(dir.path()), ["out.mp4"]);
        assert!(written_by_x264(&out));
        assert_eq!(frames_in(&out), FRAMES);
    });

    // A failure while `finish` flushes is handled the same way.
    let flush = pseudo(Faults {
        finish: true,
        ..Faults::default()
    });
    with_encoders(vec![flush], || {
        let mut attempts = Vec::new();
        encode_with_fallback(&b, &spec(&out, HwEncode::Auto), |enc| {
            attempts.push(enc.video_encoder().expect("a video encoder").to_owned());
            push(enc, 0..FRAMES)
        })
        .expect("encoded in software");
        assert_eq!(attempts, ["test_hw", "libx264"]);
        assert_eq!(frames_in(&out), FRAMES);
    });
}

#[test]
fn a_cancel_during_a_hardware_encode_is_not_retried() {
    let b = backend();
    let dir = TempDir::new("hw-cancel");
    let out = dir.path().join("out.mp4");
    let flag = Arc::new(AtomicBool::new(false));
    with_encoders(vec![pseudo(Faults::default())], || {
        let spec =
            spec(&out, HwEncode::Auto).with_cancel(CancelCheck::from_flag(Arc::clone(&flag)));
        let mut calls = 0;
        let result = encode_with_fallback(&b, &spec, |enc| {
            calls += 1;
            push(enc, 0..5)?;
            flag.store(true, Ordering::Relaxed);
            push(enc, 5..FRAMES)
        });
        assert_eq!(result, Err(MediaError::Cancelled));
        assert_eq!(calls, 1);
        assert_eq!(entries(dir.path()), Vec::<String>::new());
    });
}

#[test]
fn off_and_audio_only_outputs_need_no_hardware() {
    let b = backend();
    let dir = TempDir::new("hw-off");
    let broken = pseudo(Faults {
        open: true,
        ..Faults::default()
    });
    with_encoders(vec![broken], || {
        let out = dir.path().join("off.mp4");
        let mut enc = b.open_encoder(&spec(&out, HwEncode::Off)).expect("open");
        assert_eq!(enc.video_encoder(), Some("libx264"));
        push(&mut *enc, 0..FRAMES).expect("push");
        enc.finish().expect("finish");
        assert!(written_by_x264(&out));
        // Without a video stream the policy is not consulted, even a forced encoder.
        let audio = EncodeSpec::new(dir.path().join("audio.mp4"))
            .with_audio(crate::AudioEncodeSpec::aac(
                cutweave_model::SampleRate::HZ_48_000,
                1,
            ))
            .with_hw(HwEncode::Force("h264_nosuch".into()));
        let enc = b.open_encoder(&audio).expect("audio-only open");
        assert_eq!(enc.video_encoder(), None);
    });
}

#[test]
fn the_probe_is_empty_without_usable_hardware() {
    backend();
    // What a runner without a GPU sees: encoders missing from the build or failing to open
    // or to encode. Nothing is listed and nothing panics.
    let absent = HwEncoder {
        name: "h264_nosuch",
        device: "none",
        family: Family::Nvenc,
        codec: Codec::H264,
    };
    let refuses = pseudo(Faults {
        open: true,
        ..Faults::default()
    });
    let breaks = pseudo(Faults {
        frame: Some(0),
        ..Faults::default()
    });
    let flush = pseudo(Faults {
        finish: true,
        ..Faults::default()
    });
    assert_eq!(probe(Codec::H264, &[absent, refuses, breaks, flush]), []);
    // A working encoder is found.
    assert_eq!(
        probe(Codec::H264, &[pseudo(Faults::default())]),
        [pseudo(Faults::default())]
    );
    assert_eq!(
        test_encode(
            &VideoEncodeSpec::h264(256, 256, RATE, PixelFormat::Nv12),
            Choice::Software,
            3
        ),
        Ok(())
    );
}

#[test]
fn the_backend_lists_probed_encoders_in_auto_order() {
    // Whatever this machine has: a subsequence of the OS table, named and typed like it.
    let caps = backend().capabilities();
    let table = candidates(Codec::H264);
    let mut rest = table.iter();
    for hw in caps.hw_encode.iter().filter(|hw| hw.codec == "h264") {
        let entry = rest
            .find(|c| c.name == hw.implementation)
            .unwrap_or_else(|| panic!("{hw:?} out of order or not in {table:?}"));
        assert_eq!(hw.device, entry.device);
    }
    eprintln!("hardware encoders here: {:?}", caps.hw_encode);
    // Cached: the same list again.
    assert_eq!(backend().capabilities().hw_encode, caps.hw_encode);
}

#[test]
#[ignore = "hardware: CUTWEAVE_HW_TESTS=1, --run-ignored all"]
fn hardware_encoders_are_found_and_each_writes_a_file() {
    if !hw_tests() {
        return;
    }
    let b = backend();
    let caps = b.capabilities();
    assert!(
        !caps.hw_encode.is_empty(),
        "CUTWEAVE_HW_TESTS=1 but no hardware H.264 encoder was found"
    );
    let dir = TempDir::new("hw-each");
    for hw in &caps.hw_encode {
        let out = dir.path().join(format!("{}.mp4", hw.implementation));
        let mut enc = b
            .open_encoder(&spec(&out, HwEncode::Force(hw.implementation.clone())))
            .unwrap_or_else(|e| panic!("{}: {e}", hw.implementation));
        assert_eq!(enc.video_encoder(), Some(hw.implementation.as_str()));
        push(&mut *enc, 0..FRAMES).expect("push");
        enc.finish().expect("finish");
        assert!(!written_by_x264(&out), "{}", hw.implementation);
        assert_eq!(frames_in(&out), FRAMES, "{}", hw.implementation);
    }
}

/// The VA-API input path (NV12 uploaded into GPU surfaces, `hw_frames_ctx`, CQP), run through
/// D3D12VA, which shares VA-API's FFmpeg encode framework, on Windows. Checks that every frame
/// reaches the encoder and the file, not the picture: on the reference machine D3D12VA's own
/// H.264 is damaged even from the `ffmpeg` CLI (Radeon iGPU), and the RTX refuses to encode.
#[cfg(windows)]
#[test]
#[ignore = "hardware: CUTWEAVE_HW_TESTS=1, --run-ignored all"]
fn gpu_surface_encoders_take_uploaded_frames() {
    if !hw_tests() {
        return;
    }
    let b = backend();
    let surfaces = super::D3D12VA;
    assert_eq!(
        test_encode(
            &VideoEncodeSpec::h264(256, 256, RATE, PixelFormat::Nv12),
            Choice::Hardware(surfaces),
            3
        ),
        Ok(())
    );
    let dir = TempDir::new("hw-surfaces");
    let out = dir.path().join("out.mp4");
    with_encoders(vec![surfaces], || {
        let mut enc = b
            .open_encoder(&spec(&out, HwEncode::Force(surfaces.name.into())))
            .expect("open");
        push(&mut *enc, 0..FRAMES).expect("push");
        enc.finish().expect("finish");
    });
    assert!(!written_by_x264(&out));
    assert_eq!(frames_muxed(&out), FRAMES);
}
