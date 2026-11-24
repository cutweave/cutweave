// SPDX-License-Identifier: GPL-3.0-or-later
//! Hardware decode policy, stream eligibility and the software fallback (WP-1D.04).
//!
//! The fallback cases run on every machine: a pseudo hardware decoder (software decoding that
//! claims to be hardware, failing where told) stands in for a GPU, and every frame is compared
//! with a plain software decoder of the same file. Cases that need a real hardware decoder are
//! `#[ignore]`d and also need `CUTWEAVE_HW_TESTS=1` (they skip with a message without it):
//! `CUTWEAVE_HW_TESTS=1 cargo nextest run -p cutweave-media --features ffmpeg --run-ignored all`.
#![allow(clippy::expect_used, clippy::unwrap_used)] // test helpers

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use cutweave_model::FrameRate;
use tracing::Level;

use super::{Api, Device, Faults, Policy, StreamClass, candidates, eligible, h264_idr};
use crate::backend::{EncodeSpec, HwEncode, MediaBackend, VideoDecoder, VideoEncodeSpec};
use crate::ffmpeg::tests::recorded;
use crate::testing::FakeVideo;
use crate::testing::contract::TempDir;
use crate::{CancelToken, DecodeOpts, DecoderPool, FfmpegBackend, HwMode, PixelFormat, VideoFrame};

// ---------------------------------------------------------------------------------------------
// Policy and eligibility
// ---------------------------------------------------------------------------------------------

fn devices(p: &Policy) -> Vec<String> {
    match p {
        Policy::Devices(d) => d.iter().map(Device::label).collect(),
        other => panic!("not a device list: {other:?}"),
    }
}

#[test]
fn policies_parse_from_the_cutweave_hw_syntax() {
    for off in ["off", "OFF", " none ", "sw", "0", "false"] {
        assert_eq!(Policy::parse(off), Ok(Policy::Off), "{off}");
    }
    for auto in ["", "auto", "Auto", "on", "1"] {
        assert_eq!(Policy::parse(auto), Ok(Policy::auto()), "{auto}");
    }
    let p = Policy::parse("D3D11VA:AMD, cuda").unwrap();
    assert_eq!(devices(&p), ["d3d11va:amd", "cuda"]);
    let p = Policy::parse("nvdec,d3d11va,cuda").unwrap();
    assert_eq!(
        devices(&p),
        ["cuda", "d3d11va:nvidia", "d3d11va:amd", "d3d11va:intel"],
        "every D3D11VA vendor, duplicates dropped"
    );
    let p = Policy::parse("d3d11va:vendor=0x1af4,vaapi:/dev/dri/renderD129,vt").unwrap();
    assert_eq!(
        devices(&p),
        [
            "d3d11va:vendor=0x1af4",
            "vaapi:/dev/dri/renderD129",
            "videotoolbox"
        ]
    );
    for bad in [
        "d3d11va:1",
        "cuda:0",
        "vulkan",
        "vaapi:card0",
        ",",
        "d3d11va:vendor=xyz",
    ] {
        assert!(Policy::parse(bad).is_err(), "{bad} parsed");
    }
}

#[test]
fn the_os_devices_are_in_auto_order_and_never_by_index() {
    let labels: Vec<String> = candidates().iter().map(Device::label).collect();
    let want: &[&str] = if cfg!(windows) {
        &["cuda", "d3d11va:nvidia", "d3d11va:amd", "d3d11va:intel"]
    } else if cfg!(target_os = "macos") {
        &["videotoolbox"]
    } else {
        &["cuda", "vaapi"]
    };
    assert_eq!(labels, want);
    for d in candidates() {
        assert!(d.vendor.is_some() || d.api != Api::D3d11va, "{d:?}");
    }
    // Only NVIDIA's decoders start H.264 at non-IDR keyframes (S3 H10).
    assert!(Device::of(Api::Cuda).exact_from_non_idr_keyframes());
    assert!(Device::d3d11va(super::NVIDIA).exact_from_non_idr_keyframes());
    assert!(!Device::d3d11va(super::AMD).exact_from_non_idr_keyframes());
    assert!(!Device::of(Api::Vaapi).exact_from_non_idr_keyframes());
    assert_eq!(
        Device::d3d11va(super::AMD).implementation("hevc"),
        "hevc_d3d11va"
    );
    assert_eq!(Device::of(Api::Cuda).implementation("h264"), "h264_nvdec");
}

fn class(codec: &str, layout: &str, (width, height): (u32, u32)) -> StreamClass {
    StreamClass {
        codec: Some(codec.to_owned()),
        pixel_format: Some(layout.to_owned()),
        width,
        height,
        intra_only: false,
    }
}

#[test]
fn only_long_gop_420_h264_and_hevc_decode_in_hardware() {
    let uhd = (3840, 2160);
    assert_eq!(eligible(&class("h264", "yuv420p", uhd)), Ok("h264"));
    assert_eq!(eligible(&class("h264", "yuvj420p", uhd)), Ok("h264"));
    assert_eq!(eligible(&class("hevc", "yuv420p10le", uhd)), Ok("hevc"));
    assert_eq!(eligible(&class("hevc", "yuv420p", (144, 144))), Ok("hevc"));
    for (c, why) in [
        (class("h264", "yuv420p10le", uhd), "High 10"),
        (class("h264", "yuv422p", uhd), "4:2:2"),
        (class("hevc", "yuv422p10le", uhd), "HEVC 4:2:2"),
        (class("prores", "yuv422p10le", uhd), "intra codec"),
        (class("mjpeg", "yuvj420p", uhd), "proxy codec (S3 H8)"),
        (class("vp9", "yuv420p", uhd), "not corpus-verified"),
        (class("h264", "yuv420p", (64, 36)), "tiny"),
        (class("hevc", "yuv420p", (1920, 143)), "below 144"),
        (
            StreamClass {
                intra_only: true,
                ..class("h264", "yuv420p", uhd)
            },
            "all-intra H.264",
        ),
    ] {
        assert!(eligible(&c).is_err(), "{why}: {c:?}");
    }
}

#[test]
fn idr_pictures_are_told_from_other_h264_keyframes() {
    // NAL headers: 0x67 SPS, 0x68 PPS, 0x06 SEI, 0x65 IDR slice, 0x41 non-IDR slice, 0x09 AUD.
    let annex_b_idr = [
        0, 0, 0, 1, 0x09, 0xf0, 0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x65, 0x88,
    ];
    let annex_b_i = [0, 0, 1, 0x06, 5, 0, 0, 0, 1, 0x41, 0x9a];
    assert_eq!(h264_idr(&annex_b_idr, None), Some(true));
    assert_eq!(h264_idr(&annex_b_i, None), Some(false));
    assert_eq!(h264_idr(&[0, 0, 1, 0x67, 1], None), None, "no slice");
    // avcC: 4-byte big-endian lengths. The second NAL is 0x101 bytes long, so its length
    // starts 00 00 01 like an Annex B start code: framing must come from the extradata.
    let mut avcc = vec![0, 0, 0, 2, 0x06, 0x05];
    avcc.extend([0, 0, 1, 1, 0x65]);
    avcc.extend(std::iter::repeat_n(0x11, 0x100));
    assert_eq!(h264_idr(&avcc, Some(4)), Some(true));
    let short = [0, 2, 0x06, 0x05, 0, 2, 0x41, 0x9a];
    assert_eq!(h264_idr(&short, Some(2)), Some(false));
    assert_eq!(h264_idr(&[0, 0, 0, 9, 0x65], Some(4)), None, "truncated");
    assert_eq!(
        h264_idr(&avcc, Some(3)),
        None,
        "no 3-byte NAL lengths in avcC"
    );
}

// ---------------------------------------------------------------------------------------------
// Fallback with a pseudo hardware decoder (every machine)
// ---------------------------------------------------------------------------------------------

const SIZE: (u32, u32) = (320, 240);
const FRAMES: u64 = 90;

fn backend() -> FfmpegBackend {
    FfmpegBackend::new().expect("the loaded FFmpeg matches the build")
}

/// A 3 s H.264 clip at 320×240 with B-frames (x264, the software encoder), in `dir`.
fn clip(dir: &Path) -> PathBuf {
    let path = dir.join("clip.mp4");
    let spec = EncodeSpec::new(&path)
        .with_video(VideoEncodeSpec::h264(
            SIZE.0,
            SIZE.1,
            FrameRate::FPS_30,
            PixelFormat::Rgba8,
        ))
        .with_hw(HwEncode::Off);
    let mut enc = backend().open_encoder(&spec).expect("encoder");
    let video = FakeVideo::new(SIZE.0, SIZE.1, FrameRate::FPS_30, FRAMES);
    for n in 0..FRAMES {
        enc.push_video(&video.frame(n, PixelFormat::Rgba8).expect("frame"))
            .expect("push");
    }
    enc.finish().expect("finish");
    path
}

/// Every frame of `path` decoded in software, in order.
fn software_frames(path: &Path) -> Vec<VideoFrame> {
    let opts = DecodeOpts::default().with_hw(HwMode::Off);
    let mut d = backend()
        .open_decoder(path, &opts)
        .expect("software decoder");
    let mut frames = Vec::new();
    while let Some(f) = d.next_frame().expect("next_frame") {
        frames.push(f);
    }
    assert_eq!(frames.len() as u64, FRAMES);
    frames
}

fn pseudo(faults: Faults) -> FfmpegBackend {
    backend().with_pseudo_hw(faults)
}

fn open(b: &FfmpegBackend, path: &Path) -> Box<dyn VideoDecoder> {
    b.open_decoder(path, &DecodeOpts::default())
        .expect("open_decoder")
}

fn warnings(events: &[(String, Level, String)]) -> Vec<&str> {
    events
        .iter()
        .filter(|(target, level, m)| {
            target == "cutweave_media" && *level == Level::WARN && m.contains("hardware decode")
        })
        .map(|(_, _, m)| m.as_str())
        .collect()
}

#[test]
fn a_pseudo_hardware_decoder_that_never_fails_stays_hardware() {
    let dir = TempDir::new("hwdec-ok");
    let path = clip(dir.path());
    let want = software_frames(&path);
    let mut d = open(&pseudo(Faults::default()), &path);
    assert!(d.is_hw() && !d.hw_failed());
    assert_eq!(d.hw_decoder(), Some("h264_pseudo"));
    for n in [45, 3, 89, 0, 46] {
        let f = d.seek_exact(want[n].pts).unwrap();
        assert_eq!(f, want[n], "frame {n}");
    }
    assert!(d.is_hw() && !d.hw_failed());
}

#[test]
fn a_device_that_does_not_open_falls_back_while_opening() {
    let dir = TempDir::new("hwdec-open");
    let path = clip(dir.path());
    let want = software_frames(&path);
    let mut d = None;
    let events = recorded(|| {
        d = Some(open(
            &pseudo(Faults {
                open: true,
                ..Faults::default()
            }),
            &path,
        ));
    });
    let mut d = d.unwrap();
    assert!(!d.is_hw() && d.hw_failed() && d.hw_decoder().is_none());
    let w = warnings(&events);
    assert_eq!(w.len(), 1, "{events:#?}");
    assert!(
        w[0].contains("h264_pseudo") && w[0].contains("injected"),
        "{w:?}"
    );
    for (n, want) in want.iter().enumerate() {
        assert_eq!(d.next_frame().unwrap().as_ref(), Some(want), "frame {n}");
    }
    assert_eq!(d.next_frame().unwrap(), None);
}

#[test]
fn a_hardware_failure_during_a_seek_reruns_it_in_software() {
    let dir = TempDir::new("hwdec-seek");
    let path = clip(dir.path());
    let want = software_frames(&path);
    // The first seek feeds packets 0..; packet 7 fails.
    let mut d = open(
        &pseudo(Faults {
            packet: Some(7),
            ..Faults::default()
        }),
        &path,
    );
    assert!(d.is_hw());
    let mut got = None;
    let events = recorded(|| got = Some(d.seek_exact(want[60].pts)));
    assert_eq!(got.unwrap().unwrap(), want[60]);
    assert!(!d.is_hw() && d.hw_failed());
    assert_eq!(warnings(&events).len(), 1, "{events:#?}");
    // The decoder stays exact in software, backwards and forwards.
    for n in [10, 59, 61, 89, 0] {
        assert_eq!(d.seek_exact(want[n].pts).unwrap(), want[n], "frame {n}");
    }
    assert_eq!(d.next_frame().unwrap().as_ref(), Some(&want[1]));
}

#[test]
fn a_failure_in_sequential_decoding_resumes_after_the_last_frame() {
    let dir = TempDir::new("hwdec-seq");
    let path = clip(dir.path());
    let want = software_frames(&path);
    for packet in [0, 1, 30, 89] {
        let mut d = open(
            &pseudo(Faults {
                packet: Some(packet),
                ..Faults::default()
            }),
            &path,
        );
        let mut got = Vec::new();
        while let Some(f) = d.next_frame().unwrap() {
            got.push(f);
        }
        assert!(d.hw_failed(), "packet {packet}");
        assert_eq!(
            got.len(),
            want.len(),
            "packet {packet}: no frame lost or repeated"
        );
        for (n, (g, w)) in got.iter().zip(&want).enumerate() {
            assert_eq!(g, w, "packet {packet}: frame {n}");
        }
    }
}

#[test]
fn a_failed_copy_to_memory_falls_back() {
    let dir = TempDir::new("hwdec-copy");
    let path = clip(dir.path());
    let want = software_frames(&path);
    let mut d = open(
        &pseudo(Faults {
            transfer: true,
            ..Faults::default()
        }),
        &path,
    );
    assert_eq!(d.seek_exact(want[30].pts).unwrap(), want[30]);
    assert!(!d.is_hw() && d.hw_failed());
    let mut d = open(
        &pseudo(Faults {
            transfer: true,
            ..Faults::default()
        }),
        &path,
    );
    assert_eq!(d.next_frame().unwrap().as_ref(), Some(&want[0]));
    assert_eq!(d.next_frame().unwrap().as_ref(), Some(&want[1]));
    assert!(d.hw_failed());
}

#[test]
fn hw_off_and_ineligible_streams_never_try_the_hardware() {
    let dir = TempDir::new("hwdec-off");
    let path = clip(dir.path());
    let failing = pseudo(Faults {
        open: true,
        ..Faults::default()
    });
    let d = failing
        .open_decoder(&path, &DecodeOpts::default().with_hw(HwMode::Off))
        .unwrap();
    assert!(!d.is_hw() && !d.hw_failed());
    let d = backend()
        .with_hw_decoders("off")
        .unwrap()
        .open_decoder(&path, &DecodeOpts::default())
        .unwrap();
    assert!(!d.is_hw() && !d.hw_failed());
    // Below the hardware minimum size.
    let small = dir.path().join("small.mp4");
    let spec = EncodeSpec::new(&small)
        .with_video(VideoEncodeSpec::h264(
            128,
            72,
            FrameRate::FPS_30,
            PixelFormat::Rgba8,
        ))
        .with_hw(HwEncode::Off);
    let mut enc = backend().open_encoder(&spec).unwrap();
    let video = FakeVideo::new(128, 72, FrameRate::FPS_30, 10);
    for n in 0..10 {
        enc.push_video(&video.frame(n, PixelFormat::Rgba8).unwrap())
            .unwrap();
    }
    enc.finish().unwrap();
    let d = failing
        .open_decoder(&small, &DecodeOpts::default())
        .unwrap();
    assert!(!d.is_hw() && !d.hw_failed());
}

#[test]
fn a_pool_opens_later_decoders_in_software_after_a_failure() {
    let dir = TempDir::new("hwdec-pool");
    let path = clip(dir.path());
    let want = software_frames(&path);
    let b = pseudo(Faults {
        packet: Some(3),
        ..Faults::default()
    });
    let pool = DecoderPool::with_max_live(Arc::new(b), &path, 3);
    let never = CancelToken::new();
    let opts = DecodeOpts::default();
    let mut first = pool.checkout(&opts, None, &never).unwrap();
    let second = pool.checkout(&opts, None, &never).unwrap();
    assert!(first.is_hw() && second.is_hw() && !pool.hw_failed());
    assert_eq!(first.hw_decoder(), Some("h264_pseudo"));
    assert_eq!(first.seek_exact(want[50].pts).unwrap(), want[50]);
    assert!(!first.is_hw() && first.hw_failed() && first.hw_decoder().is_none());
    assert!(pool.hw_failed());
    // A decoder opened from now on is software; the one already open keeps its hardware.
    let third = pool.checkout(&opts, None, &never).unwrap();
    assert!(!third.is_hw() && !third.hw_failed());
    assert!(second.is_hw());
    drop((first, second, third));
    assert_eq!(pool.opened(), 3);
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

/// `dir/name` made by the `ffmpeg` CLI from a lavfi test source with `args`; `None` (with a skip
/// message) without a CLI.
fn generate(dir: &Path, name: &str, source: &str, args: &[&str]) -> Option<PathBuf> {
    let path = dir.join(name);
    let out = Command::new(tool("ffmpeg"))
        .args(["-v", "error", "-nostdin", "-y", "-f", "lavfi", "-i", source])
        .args(args)
        .arg(&path)
        .output();
    match out {
        Ok(out) if out.status.success() => Some(path),
        Ok(out) => panic!("ffmpeg failed: {}", String::from_utf8_lossy(&out.stderr)),
        Err(e) => {
            eprintln!("skipping: no ffmpeg CLI ({e}; source scripts/ffmpeg/env.sh)");
            None
        }
    }
}

/// S3 H10: on a device that must start H.264 at IDR pictures, a seek landing on a keyframe that
/// is not one goes over to software (exactly), in MP4 (`avcC` framing) and MPEG-TS (Annex B);
/// a stream whose keyframes are all IDR stays in hardware.
#[test]
fn a_seek_landing_on_a_non_idr_keyframe_leaves_a_decoder_that_needs_idr() {
    let dir = TempDir::new("hwdec-idr");
    let source = "testsrc2=size=320x240:rate=30:duration=6";
    let open_gop = ["-c:v", "libx264", "-g", "60", "-bf", "3"];
    let open_gop_params = ["-x264-params", "open-gop=1:scenecut=0"];
    let clips = [
        (
            "open.mp4",
            true,
            [&open_gop[..], &open_gop_params[..]].concat(),
        ),
        (
            "open.ts",
            true,
            [&open_gop[..], &open_gop_params[..]].concat(),
        ),
        (
            "closed.mp4",
            false,
            [
                "-c:v",
                "libx264",
                "-g",
                "60",
                "-bf",
                "3",
                "-x264-params",
                "scenecut=0",
            ]
            .to_vec(),
        ),
    ];
    let needs_idr = pseudo(Faults {
        idr_starts_only: true,
        ..Faults::default()
    });
    for (name, non_idr_keys, args) in clips {
        let Some(path) = generate(dir.path(), name, source, &args) else {
            return;
        };
        let index = Arc::new(backend().index_media(&path).unwrap());
        let opts = DecodeOpts::default().with_index(Arc::clone(&index));
        let mut sw = backend()
            .open_decoder(&path, &opts.clone().with_hw(HwMode::Off))
            .unwrap();
        let mut d = needs_idr.open_decoder(&path, &opts).unwrap();
        // Frame 30 starts at the first keyframe, an IDR picture.
        let t = index.time_of(30).unwrap();
        assert_eq!(
            d.seek_exact(t).unwrap(),
            sw.seek_exact(t).unwrap(),
            "{name}"
        );
        assert!(d.is_hw(), "{name}: still hardware after an IDR start");
        // Frame 150 starts at the third keyframe.
        let t = index.time_of(150).unwrap();
        assert_eq!(
            d.seek_exact(t).unwrap(),
            sw.seek_exact(t).unwrap(),
            "{name}"
        );
        assert_eq!(d.hw_failed(), non_idr_keys, "{name}");
        assert_eq!(d.is_hw(), !non_idr_keys, "{name}");
    }
}

// ---------------------------------------------------------------------------------------------
// Real hardware (`CUTWEAVE_HW_TESTS=1`)
// ---------------------------------------------------------------------------------------------

/// Whether the hardware cases run (`CUTWEAVE_HW_TESTS=1`); prints why not.
fn hw_tests() -> bool {
    let on = std::env::var("CUTWEAVE_HW_TESTS").is_ok_and(|v| v == "1");
    if !on {
        eprintln!("skipping: set CUTWEAVE_HW_TESTS=1 on a machine with a hardware decoder");
    }
    on
}

#[test]
#[ignore = "hardware: CUTWEAVE_HW_TESTS=1, --run-ignored all"]
fn the_probe_verifies_the_machines_hardware_decoders() {
    if !hw_tests() {
        return;
    }
    let caps = backend().capabilities();
    for hw in &caps.hw_decode {
        eprintln!("hardware decoder: {} on {}", hw.implementation, hw.device);
    }
    assert!(
        caps.hw_decode.iter().any(|hw| hw.codec == "h264"),
        "CUTWEAVE_HW_TESTS=1 but no hardware H.264 decoder passed the probe: {:?}",
        caps.hw_decode
    );
    // The list follows the policy: one device named, only its entries.
    let first = caps.hw_decode[0].device.clone();
    let only = backend().with_hw_decoders(&first).unwrap().capabilities();
    assert!(
        only.hw_decode.iter().all(|hw| hw.device == first),
        "{only:?}"
    );
}

/// The real init failure (S3 H6 case 4, the WBS's "forced init error"): H.264 High 10 is offered
/// to NVDEC, whose hwaccel then fails to initialise on GPUs without it (every GeForce up to the
/// RTX 40 series). The eligibility filter normally keeps such streams in software; this test
/// opens the hardware path for one directly.
#[test]
#[ignore = "hardware: CUTWEAVE_HW_TESTS=1, --run-ignored all"]
fn a_hwaccel_that_does_not_initialise_falls_back_to_software() {
    if !hw_tests() {
        return;
    }
    let cuda = Device::of(Api::Cuda);
    if super::verified(&cuda, "h264").is_err() {
        eprintln!("skipping: no NVDEC here");
        return;
    }
    let dir = TempDir::new("hwdec-high10");
    let Some(path) = generate(
        dir.path(),
        "high10.mp4",
        "testsrc2=size=640x360:rate=30:duration=2",
        &["-c:v", "libx264", "-pix_fmt", "yuv420p10le", "-g", "30"],
    ) else {
        return;
    };
    let index = Arc::new(backend().index_media(&path).unwrap());
    let opts = DecodeOpts::default().with_index(Arc::clone(&index));
    let mut sw = backend()
        .open_decoder(&path, &opts.clone().with_hw(HwMode::Off))
        .unwrap();
    // A choice for the stream as if it were eligible.
    let found = super::verified(&cuda, "h264").unwrap();
    let choice = super::Choice {
        device: cuda.label(),
        implementation: cuda.implementation("h264"),
        idr_starts_only: false,
        backing: super::Backing::Device {
            device: found.device,
            surface_format: found.surface_format,
        },
    };
    let mut d = crate::ffmpeg::decode::open_with_choice(&path, &opts, choice).unwrap();
    assert!(d.is_hw(), "opened on the hardware path");
    let mut got = None;
    let events = recorded(|| got = Some(d.seek_exact(index.time_of(40).unwrap())));
    let got = got.unwrap().unwrap();
    assert_eq!(got, sw.seek_exact(index.time_of(40).unwrap()).unwrap());
    assert!(!d.is_hw() && d.hw_failed());
    let w = warnings(&events);
    assert_eq!(w.len(), 1, "{events:#?}");
    eprintln!("{}", w[0]);
}
