// SPDX-License-Identifier: GPL-3.0-or-later
//! Hardware decode on the corpus (WP-1D.04 acceptance; ADR-0003, arch §7).
//!
//! [`verify`] runs the frame-exact passes of `frame_exact.rs` (sequential, random seeks through
//! one decoder, keyframe and open-GOP edges both ways, a scrub sweep) with a decoder opened under
//! [`HwMode::Auto`], and checks every frame: pts equal to the index's (which `frame_exact`
//! checks against software and the CLI), Y plane equal to the FFmpeg CLI's software decode
//! (bit-exact; P010 compared as its 10-bit values), else PSNR against the software decode of the
//! same frame above 50 dB. It reports which decoder ran and how many frames were bit-exact.
//!
//! - Every machine: with hardware decode off, and with a device that does not exist, the same
//!   passes run in software (`is_hw() == false`).
//! - `#[ignore]`d, and skipped unless `CUTWEAVE_HW_TESTS=1` (a machine with a hardware decoder):
//!   the corpus H.264 4K30 and HEVC 10-bit files decode in hardware, 100 seeks each, exact; every
//!   hardware decoder of the machine (`Capabilities::hw_decode`, including a second GPU) passes
//!   the same on the 4K, 10-bit and generated open-GOP clips (on devices other than NVIDIA's the
//!   open-GOP H.264 clip goes over to software at its first non-IDR keyframe, S3 H10); pooled
//!   decoders report hardware.
//!
//! `CUTWEAVE_HW_TESTS=1 cargo nextest run -p cutweave-media --features ffmpeg --test hw_decode
//! --run-ignored all --no-capture`. Missing corpus files or CLI tools skip with a message.
#![cfg(feature = "ffmpeg")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::Path;
use std::sync::Arc;

use cutweave_media::{
    CancelToken, DecodeOpts, DecoderPool, FfmpegBackend, FrameIndex, HwMode, MediaBackend,
    PixelFormat, VideoDecoder, VideoFrame,
};
use cutweave_model::{ColorPrimaries, MatrixCoefficients, Time, TransferCharacteristics};
use support::{Crc32, Rng};

/// Seeks per file (the PR tier of `frame_exact`).
const SEEKS: usize = 100;
/// Seeks per file in the software runs every machine does (`frame_exact` covers the rest).
const SOFTWARE_SEEKS: usize = 30;
/// The WBS floor for a hardware frame that is not bit-exact.
const MIN_PSNR_DB: f64 = 50.0;

/// Whether the hardware cases run (`CUTWEAVE_HW_TESTS=1`); prints why not.
fn hw_tests() -> bool {
    let on = std::env::var("CUTWEAVE_HW_TESTS").is_ok_and(|v| v == "1");
    if !on {
        eprintln!("skipping: set CUTWEAVE_HW_TESTS=1 on a machine with a hardware decoder");
    }
    on
}

/// What [`verify`] saw.
#[derive(Debug, Default)]
struct Report {
    /// The hardware decoder that ran, if any.
    decoder: Option<String>,
    frames: usize,
    bit_exact: usize,
    min_psnr: f64,
}

struct Checker<'a> {
    name: &'a str,
    index: &'a FrameIndex,
    truth: &'a [(i64, u32)],
    crc: Crc32,
    /// A software decoder for PSNR when a frame is not bit-exact (opened on first need).
    software: Option<Box<dyn VideoDecoder>>,
    open_software: Box<dyn Fn() -> Box<dyn VideoDecoder> + 'a>,
    report: Report,
    failures: Vec<String>,
}

impl Checker<'_> {
    fn frame(&mut self, pass: &str, n: usize, got: Result<VideoFrame, String>) {
        self.report.frames += 1;
        let want_pts = self.index.time_of(n).expect("frame time");
        let result = got.and_then(|f| {
            if f.pts != want_pts {
                return Err(format!(
                    "pts {:?} (frame {}) instead of {want_pts:?}",
                    f.pts,
                    self.index.frame_at(f.pts)
                ));
            }
            if f.damaged {
                return Err("flagged damaged".to_owned());
            }
            f.validate().map_err(|e| e.to_string())?;
            if self.crc.y_plane(&f) == self.truth[n].1 {
                self.report.bit_exact += 1;
                return Ok(());
            }
            let software = self.software.get_or_insert_with(|| (self.open_software)());
            let reference = software
                .seek_exact(want_pts)
                .map_err(|e| format!("software seek: {e}"))?;
            let psnr = support::y_psnr(&f, &reference);
            self.report.min_psnr = self.report.min_psnr.min(psnr);
            if psnr > MIN_PSNR_DB {
                Ok(())
            } else {
                Err(format!("Y PSNR {psnr:.1} dB against software"))
            }
        });
        if let Err(e) = result
            && self.failures.len() < 20
        {
            self.failures
                .push(format!("{}: {pass}: frame {n}: {e}", self.name));
        }
    }
}

/// A random time inside frame `n`'s display interval (every other one exactly on it).
fn time_inside(index: &FrameIndex, n: usize, rng: &mut Rng) -> Time {
    let start = index.time_of(n).expect("frame time");
    let next = index
        .time_of(n + 1)
        .or_else(|| index.end_time())
        .unwrap_or(start);
    let span = u64::try_from((next.flicks() - start.flicks()).max(1)).unwrap_or(1);
    let offset = if rng.next_u64().is_multiple_of(2) {
        0
    } else {
        i64::try_from(rng.next_u64() % span).unwrap_or(0)
    };
    Time::from_flicks(start.flicks() + offset).expect("time")
}

fn seek(d: &mut dyn VideoDecoder, t: Time) -> Result<VideoFrame, String> {
    d.seek_exact(t)
        .map_err(|e| format!("seek_exact({t:?}): {e}"))
}

/// What [`verify`] expects of the decoders it opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expect {
    /// Software from open.
    Software,
    /// Hardware from open to the end.
    Hardware,
    /// Hardware from open; seeks that land on non-IDR H.264 keyframes go over to software
    /// (S3 H10 on devices other than NVIDIA's), and at least one decoder does.
    HardwareThenSoftware,
}

/// The frame-exact passes over `path` on `backend` with `seeks` random seeks, the decoders as
/// `expect` says. `None` without the CLI.
#[allow(clippy::too_many_lines)]
fn verify(backend: &FfmpegBackend, path: &Path, seeks: usize, expect: Expect) -> Option<Report> {
    let expect_hw = expect != Expect::Software;
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    let index = Arc::new(
        backend
            .index_media(path)
            .unwrap_or_else(|e| panic!("{name}: index: {e}")),
    );
    let truth = support::y_crcs(path, index.stream())?;
    assert_eq!(truth.len(), index.frame_count(), "{name}: CLI frames");
    let opts = DecodeOpts::default().with_index(Arc::clone(&index));
    let open = || {
        let d = backend
            .open_decoder(path, &opts)
            .unwrap_or_else(|e| panic!("{name}: open_decoder: {e}"));
        assert_eq!(
            d.is_hw(),
            expect_hw,
            "{name}: decoder is_hw ({:?})",
            d.hw_decoder()
        );
        d
    };
    let software_opts = opts.clone().with_hw(HwMode::Off);
    let mut check = Checker {
        name: &name,
        index: &index,
        truth: &truth,
        crc: Crc32::default(),
        software: None,
        open_software: Box::new(move || {
            support::backend()
                .open_decoder(path, &software_opts)
                .expect("software decoder")
        }),
        report: Report {
            min_psnr: f64::INFINITY,
            ..Report::default()
        },
        failures: Vec::new(),
    };
    let frames = index.frame_count();
    let mut decoders: Vec<Box<dyn VideoDecoder>> = Vec::new();

    // 1. Sequential.
    let mut d = open();
    check.report.decoder = d.hw_decoder().map(str::to_owned);
    for n in 0..frames {
        let got = d
            .next_frame()
            .map_err(|e| e.to_string())
            .and_then(|f| f.ok_or_else(|| "the stream ended early".to_owned()));
        check.frame("sequential", n, got);
    }
    assert!(
        matches!(d.next_frame(), Ok(None)),
        "{name}: frames after the last"
    );
    decoders.push(d);

    // 2. Random seeks through one decoder.
    let mut d = open();
    let mut rng = Rng::new(support::seed(&name));
    for _ in 0..seeks {
        let n = rng.below(frames);
        let t = time_inside(&index, n, &mut rng);
        check.frame("random", n, seek(&mut *d, t));
    }
    decoders.push(d);

    // 3. Edges: the ends, every keyframe ±1 and every open-GOP leading picture, both ways.
    let mut edges: Vec<usize> = (0..4.min(frames))
        .chain(frames.saturating_sub(4)..frames)
        .collect();
    for k in index.key_frames() {
        edges.extend([k.saturating_sub(1), k, (k + 1).min(frames - 1)]);
    }
    edges.extend((0..frames).filter(|&n| {
        let f = index.frames()[n];
        index.keys_before(f.decode_idx + 1).next() != Some(f.start_idx)
    }));
    edges.sort_unstable();
    edges.dedup();
    for order in [edges.clone(), edges.iter().rev().copied().collect()] {
        let mut d = open();
        for n in order {
            check.frame("edges", n, seek(&mut *d, index.time_of(n).expect("time")));
        }
        decoders.push(d);
    }

    // 4. Scrub: a forward sweep, then single-frame steps.
    let mut d = open();
    for n in (0..frames).step_by((frames / 60).max(1)) {
        let t = time_inside(&index, n, &mut rng);
        check.frame("scrub", n, seek(&mut *d, t));
    }
    let start = rng.below(frames.saturating_sub(20).max(1));
    for n in start..(start + 20).min(frames) {
        check.frame("scrub", n, seek(&mut *d, index.time_of(n).expect("time")));
    }
    decoders.push(d);

    let fell_back = decoders.iter().filter(|d| d.hw_failed()).count();
    for d in &decoders {
        if expect == Expect::HardwareThenSoftware && d.hw_failed() {
            assert!(!d.is_hw(), "{name}: hardware after the fallback");
        } else {
            assert_eq!(d.is_hw(), expect_hw, "{name}: is_hw after the passes");
            assert!(!d.hw_failed(), "{name}: a hardware decoder failed");
        }
    }
    if expect == Expect::HardwareThenSoftware {
        assert!(fell_back > 0, "{name}: no decoder left the hardware");
    }
    let r = &check.report;
    eprintln!(
        "{name}: {} ({} frames, {} seeks): {}/{} frames bit-exact against software{}{}",
        r.decoder.as_deref().unwrap_or("software"),
        frames,
        seeks,
        r.bit_exact,
        r.frames,
        if r.min_psnr.is_finite() {
            format!(", min Y PSNR {:.1} dB", r.min_psnr)
        } else {
            String::new()
        },
        if fell_back > 0 {
            format!(
                "; {fell_back}/{} decoders went over to software",
                decoders.len()
            )
        } else {
            String::new()
        }
    );
    assert!(
        check.failures.is_empty(),
        "{name}: {} checks failed:\n{}",
        check.failures.len(),
        check.failures.join("\n")
    );
    Some(check.report)
}

/// The corpus file, or a skip message.
fn corpus(name: &str) -> Option<std::path::PathBuf> {
    support::corpus_file(name)
}

// ---------------------------------------------------------------------------------------------
// Every machine: software when hardware decode is off or unavailable
// ---------------------------------------------------------------------------------------------

#[test]
fn with_hardware_decode_off_the_corpus_decodes_exactly_in_software() {
    let off = support::backend().with_hw_decoders("off").unwrap();
    assert_eq!(off.capabilities().hw_decode, Vec::new());
    for name in ["h264_4k30_longgop.mp4", "hevc_10bit_sdr.mp4"] {
        let Some(path) = corpus(name) else { continue };
        verify(&off, &path, SOFTWARE_SEEKS, Expect::Software);
    }
}

#[test]
fn a_hardware_device_that_does_not_exist_leaves_software_decode() {
    // A GPU vendor no machine has (Windows), a DRM node that does not exist (Linux).
    let spec = if cfg!(windows) {
        "d3d11va:vendor=0xdead"
    } else if cfg!(target_os = "macos") {
        eprintln!("skipping: VideoToolbox always opens on macOS");
        return;
    } else {
        "vaapi:/dev/dri/renderD250"
    };
    let missing = support::backend().with_hw_decoders(spec).unwrap();
    assert_eq!(missing.capabilities().hw_decode, Vec::new());
    let Some(path) = corpus("h264_1080p60.mp4") else {
        return;
    };
    verify(&missing, &path, SOFTWARE_SEEKS, Expect::Software);
    // Not a failure: no hardware decoder was ever tried for the stream.
    let d = missing.open_decoder(&path, &DecodeOpts::default()).unwrap();
    assert!(!d.is_hw() && !d.hw_failed());
}

#[test]
fn intra_codecs_stay_in_software_under_auto() {
    let Some(path) = corpus("prores_422hq_1080p.mov") else {
        return;
    };
    let d = support::backend()
        .open_decoder(&path, &DecodeOpts::default())
        .unwrap();
    assert!(!d.is_hw() && !d.hw_failed() && d.hw_decoder().is_none());
}

#[test]
fn bad_hardware_specs_are_invalid() {
    for bad in ["d3d11va:0", "vulkan", "cuda:1"] {
        let err = support::backend().with_hw_decoders(bad).err();
        assert!(
            matches!(err, Some(cutweave_media::MediaError::Invalid { .. })),
            "{bad}: {err:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Machines with a hardware decoder (`CUTWEAVE_HW_TESTS=1`)
// ---------------------------------------------------------------------------------------------

#[test]
#[ignore = "hardware: CUTWEAVE_HW_TESTS=1, --run-ignored all"]
fn the_corpus_decodes_in_hardware_frame_exactly() {
    if !hw_tests() {
        return;
    }
    let backend = support::backend();
    assert!(
        !backend.capabilities().hw_decode.is_empty(),
        "CUTWEAVE_HW_TESTS=1 but no hardware decoder passed the probe"
    );
    // The WBS files first, then the rest of the small tier's H.264/HEVC.
    for name in [
        "h264_4k30_longgop.mp4",
        "hevc_10bit_sdr.mp4",
        "hevc_10bit_hdr10_pq.mp4",
        "h264_1080p60.mp4",
        "vfr_phone_rotated90.mp4",
        "no_audio_stream.mp4",
        "sintel_trailer_480p.mp4",
    ] {
        let Some(path) = corpus(name) else { continue };
        verify(&backend, &path, SEEKS, Expect::Hardware);
    }
}

#[test]
#[ignore = "hardware: CUTWEAVE_HW_TESTS=1, --run-ignored all"]
fn hardware_frames_are_nv12_or_p010_with_the_streams_colour() {
    if !hw_tests() {
        return;
    }
    let cases = [
        ("h264_4k30_longgop.mp4", PixelFormat::Nv12, None),
        (
            "hevc_10bit_sdr.mp4",
            PixelFormat::P010,
            Some((ColorPrimaries::Bt709, TransferCharacteristics::Bt709)),
        ),
        (
            "hevc_10bit_hdr10_pq.mp4",
            PixelFormat::P010,
            Some((ColorPrimaries::Bt2020, TransferCharacteristics::Pq)),
        ),
    ];
    for (name, format, color) in cases {
        let Some(path) = corpus(name) else { continue };
        let mut d = support::backend()
            .open_decoder(&path, &DecodeOpts::default())
            .unwrap();
        assert!(d.is_hw(), "{name}");
        let index = d.index();
        let f = d
            .seek_exact(index.time_of(index.frame_count() / 2).unwrap())
            .unwrap();
        assert_eq!(f.format, format, "{name}");
        f.validate().unwrap();
        if let Some((primaries, transfer)) = color {
            let wide = primaries == ColorPrimaries::Bt2020;
            assert_eq!(f.color.primaries, Some(primaries), "{name}");
            assert_eq!(f.color.transfer, Some(transfer), "{name}");
            if wide {
                assert_eq!(f.color.matrix, Some(MatrixCoefficients::Bt2020Ncl));
            }
        }
        if format == PixelFormat::P010 {
            // High-aligned 10-bit samples: the low 6 bits are zero.
            assert!(
                f.planes[0]
                    .data
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .all(|s| u16::from_le_bytes(*s).trailing_zeros() >= 6),
                "{name}: P010 samples"
            );
        }
    }
}

#[test]
#[ignore = "hardware: CUTWEAVE_HW_TESTS=1, --run-ignored all"]
fn every_hardware_decoder_of_the_machine_is_frame_exact() {
    if !hw_tests() {
        return;
    }
    let caps = support::backend().capabilities();
    let mut devices: Vec<String> = caps.hw_decode.iter().map(|h| h.device.clone()).collect();
    devices.dedup();
    for device in devices {
        eprintln!("--- {device}");
        let backend = support::backend().with_hw_decoders(&device).unwrap();
        // Only NVIDIA's decoders are exact from H.264 keyframes that are not IDR pictures (S3
        // H10); on the others such a stream goes over to software at the first seek to one.
        let nvidia = device == "cuda" || device == "d3d11va:nvidia";
        let files = [
            (corpus("h264_4k30_longgop.mp4"), Expect::Hardware),
            (corpus("hevc_10bit_sdr.mp4"), Expect::Hardware),
            // Open GOP: H.264 with non-IDR I-frames (S3 H10) and HEVC CRA/RASL.
            (
                support::generated_clip("h264_opengop_2997.mp4"),
                if nvidia {
                    Expect::Hardware
                } else {
                    Expect::HardwareThenSoftware
                },
            ),
            (
                support::generated_clip("hevc_opengop_10bit.mkv"),
                Expect::Hardware,
            ),
        ];
        for (path, expect) in files {
            if let Some(path) = path {
                verify(&backend, &path, SEEKS, expect);
            }
        }
    }
}

#[test]
#[ignore = "hardware: CUTWEAVE_HW_TESTS=1, --run-ignored all"]
fn pooled_decoders_run_in_hardware() {
    if !hw_tests() {
        return;
    }
    let Some(path) = corpus("h264_4k30_longgop.mp4") else {
        return;
    };
    let pool = DecoderPool::new(Arc::new(support::backend()), &path);
    let mut lease = pool
        .checkout(&DecodeOpts::default(), None, &CancelToken::new())
        .unwrap();
    assert!(lease.is_hw(), "{:?}", lease.hw_decoder());
    let index = lease.index();
    let f = lease.seek_exact(index.time_of(100).unwrap()).unwrap();
    assert_eq!(f.pts, index.time_of(100).unwrap());
    assert!(lease.is_hw() && !lease.hw_failed() && !pool.hw_failed());
    eprintln!("pooled: {}", lease.hw_decoder().unwrap_or("?"));
}
