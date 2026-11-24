// SPDX-License-Identifier: GPL-3.0-or-later
//! Media-dependent harness tests. They need `python gen_media.py` (media/) and the FFmpeg
//! tools for ground truth, so they are `#[ignore]`d: run with `cargo test -- --ignored`.

use std::path::{Path, PathBuf};

use s3::hw::{HwSpec, HwStatus};
use s3::manifest;
use s3::robust;
use s3::source::{Options, Threads, VideoSource};
use s3::verify::{self, Config};

/// Hardware decoders to test: `S3_HW=d3d11va:1,cuda` or the platform default.
fn hw_specs() -> Vec<HwSpec> {
    let list = std::env::var("S3_HW").unwrap_or_else(|_| {
        if cfg!(windows) {
            "d3d11va,cuda".into()
        } else if cfg!(target_os = "macos") {
            "videotoolbox".into()
        } else {
            "vaapi".into()
        }
    });
    list.split(',')
        .filter_map(|s| HwSpec::parse(s.trim()).ok().flatten())
        .collect()
}

fn hw_opts(spec: HwSpec) -> Options {
    Options {
        threads: Threads::Single,
        hw: Some(spec),
        ..Options::default()
    }
}

/// WP-0E.02: the hardware path is frame-exact (pts and barcode) and every frame is bit-exact or
/// within 50 dB PSNR of the software decode, on the 4K H.264 (1 s and 8.3 s GOP), 4K HEVC Main10
/// (open GOP, RASL), HDR10 and open-GOP H.264 files. A device that does not open on this machine
/// is reported and skipped (that case is the fallback test's).
#[test]
#[ignore = "needs generated media, ffprobe/ffmpeg and a hardware decoder"]
fn hw_decode_is_frame_exact_and_close_to_software() {
    s3::init(true);
    let mut failed = vec![];
    let mut ran = 0;
    for spec in hw_specs() {
        for name in [
            "h264_2160p30_gop30.mp4",
            "h264_2160p30_gop250.mp4",
            "hevc_main10_2160p30_gop60.mp4",
            "hevc_main10_pq_1080p24.mp4",
            "h264_1080p2997_gop60_opengop.mp4",
        ] {
            let cfg = Config {
                seeks: 60,
                seed: 0x5EED,
                opts: hw_opts(spec),
                barcode: true,
            };
            let r = verify::verify(&out(), &media().join(name), cfg).expect("verify");
            println!(
                "{} {name}: frame-exact {} | {} | bit-exact {} | PSNR min {}",
                spec.label(),
                r.random.show(),
                r.hw,
                r.hw_bit_exact.show(),
                r.hw_psnr_min
            );
            if r.hw.contains("device failed") {
                println!("  skipped: {} does not open here", spec.label());
                continue;
            }
            ran += 1;
            if !r.exact() || r.stats.hw_frames == 0 || r.stats.sw_frames > 0 {
                failed.push(format!(
                    "{} {name}: {} {:?}",
                    spec.label(),
                    r.hw,
                    r.failures
                ));
            }
        }
    }
    assert!(ran > 0, "no hardware decoder opened: {:?}", hw_specs());
    assert!(failed.is_empty(), "{failed:#?}");
}

/// WP-0E.02 fallback: with hardware decode forced off, with a device that does not exist, with a
/// codec that has no hwaccel (ProRes), with a profile the hwaccel is never offered for (H.264 High
/// 10) and with one whose hwaccel initialisation fails (HEVC 4:2:2 10-bit, where offered), the
/// source decodes in software and stays frame-exact.
#[test]
#[ignore = "needs generated media and ffprobe/ffmpeg"]
fn hw_failures_fall_back_to_software_cleanly() {
    s3::init(true);
    let spec = hw_specs()[0];
    let missing = HwSpec {
        device: Some(9),
        ..spec
    };
    let check = |name: &str, opts: Options| {
        let cfg = Config {
            seeks: 40,
            seed: 11,
            opts,
            barcode: true,
        };
        let r = verify::verify(&out(), &media().join(name), cfg).expect("verify");
        println!("{name} [{}]: {} | {}", opts.label(), r.random.show(), r.hw);
        assert!(r.exact(), "{name} {}: {:?}", opts.label(), r.failures);
        assert_eq!(
            r.stats.hw_frames, 0,
            "{name}: expected software frames only"
        );
        r
    };
    // Forced off.
    let r = check("h264_2160p30_gop30.mp4", Options::default());
    assert!(r.hw.is_empty());
    // No such device (adapter 9; VA-API render node 9 on Linux).
    let r = check("hevc_main10_pq_1080p24.mp4", hw_opts(missing));
    assert!(r.hw.contains("device failed"), "{}", r.hw);
    // Decoder without a hwaccel.
    let r = check("prores_hq_1080p25.mov", hw_opts(spec));
    assert!(r.hw.contains("has no"), "{}", r.hw);
    // Profiles: hw format never offered / hwaccel init fails -> get_format picks software.
    for name in ["h264_high10_720p30.mp4", "hevc_main422_10_720p30.mp4"] {
        let r = check(name, hw_opts(spec));
        assert!(
            r.hw.contains("has no") || r.hw.contains("sw 1") || r.hw.contains("sw 2"),
            "{name}: {}",
            r.hw
        );
    }
    // The status of an opened source says the same.
    let src =
        VideoSource::open(&media().join("prores_hq_1080p25.mov"), hw_opts(spec)).expect("open");
    assert!(matches!(src.hw_status(), HwStatus::NoHwConfig { .. }));
}

/// WP-0E.02 path (a): H.264 4K and HEVC 10-bit frames decoded (hardware where it opens), copied to
/// system memory, uploaded to wgpu textures and converted to RGB; the read-back RGB equals the
/// CPU conversion (within 2/255) and shows the right frame barcode.
#[cfg(feature = "gpu")]
#[test]
#[ignore = "needs generated media and a GPU"]
fn gpu_upload_renders_4k_h264_and_hevc_10_bit() {
    use s3::gpu::{self, Gpu};
    s3::init(true);
    let mut g = Gpu::new("").expect("wgpu device");
    for name in ["h264_2160p30_gop30.mp4", "hevc_main10_2160p30_gop60.mp4"] {
        for opts in [Options::default(), hw_opts(hw_specs()[0])] {
            let mut src = VideoSource::open(&media().join(name), opts).expect("open");
            for pi in [0, 77, 150] {
                src.seek_frame(pi).expect("seek");
                g.upload(src.frame()).expect("upload");
                let rgba = g.read_rgba().expect("readback");
                let f = src.frame();
                let reference = gpu::cpu_reference(f).expect("4:2:0 frame");
                let (max, _) = gpu::compare_rgba(&rgba, &reference);
                let code = gpu::barcode_from_rgba(&rgba, f.width() as usize, f.height() as usize);
                println!(
                    "{name} [{}] f{pi} {:?}: max diff {max}, barcode {code:?} ({})",
                    opts.label(),
                    f.format(),
                    src.hw_summary()
                );
                assert!(max <= 2, "{name} f{pi}: max diff {max}");
                assert_eq!(code, Some(pi as u32), "{name} f{pi}");
            }
        }
    }
}

fn media() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("media")
}

fn out() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("s3-out")
}

fn entries(kinds: &[&str]) -> Vec<manifest::Entry> {
    let m =
        manifest::load(&media()).expect("media/manifest.tsv missing: run `python gen_media.py`");
    m.into_iter()
        .filter(|e| kinds.contains(&e.kind.as_str()))
        .collect()
}

/// WP-0E.01 acceptance: `frame-exact: 500/500` for every generated video and proxy file, plus
/// exact sequential, edge and scrub passes and an index identical to ffprobe's.
#[test]
#[ignore = "needs generated media and ffprobe/ffmpeg"]
fn every_file_is_frame_exact_on_500_random_seeks() {
    s3::init(true);
    let mut failed = vec![];
    for e in entries(&["video", "proxy"]) {
        let cfg = Config {
            seeks: 500,
            seed: 0x5EED,
            opts: Options::default(),
            barcode: e.flag("barcode"),
        };
        let r = verify::verify(&out(), &e.path, cfg).expect("verify");
        println!("{:<36} frame-exact: {}", r.file, r.random.show());
        if !r.exact() || r.random.total != 500 {
            failed.push(format!(
                "{}: random {} seq {} edges {} scrub {} {:?}",
                r.file,
                r.random.show(),
                r.sequential.show(),
                r.edges.show(),
                r.scrub.show(),
                r.failures
            ));
        }
    }
    assert!(failed.is_empty(), "{failed:#?}");
}

/// Frame-exactness does not depend on skipping non-reference frames or on the thread mode.
#[test]
#[ignore = "needs generated media and ffprobe/ffmpeg"]
fn exact_without_skip_and_with_slice_threads() {
    s3::init(true);
    for name in [
        "h264_1080p2997_gop60_opengop.mp4",
        "hevc_main10_pq_1080p24.mp4",
        "h264_vfr_phone_1080p_rot90.mp4",
    ] {
        let path = media().join(name);
        for opts in [
            Options {
                threads: Threads::Frame(0),
                skip_nonref: false,
                ..Options::default()
            },
            Options {
                threads: Threads::Slice(0),
                skip_nonref: true,
                ..Options::default()
            },
        ] {
            let r = verify::verify(
                &out(),
                &path,
                Config {
                    seeks: 100,
                    seed: 7,
                    opts,
                    barcode: true,
                },
            )
            .expect("verify");
            assert!(r.exact(), "{name} {opts:?}: {:?}", r.failures);
        }
    }
}

/// Broken files never panic; corrupt ones end in a typed error or partial decode.
#[test]
#[ignore = "needs generated media"]
fn broken_files_fail_cleanly() {
    s3::init(true);
    for e in entries(&["broken"]) {
        let o = robust::check(&e.path);
        assert!(o.panicked.is_none(), "{}: {:?}", e.name, o.panicked);
        if e.get("open") == Some("fail") {
            assert_eq!(o.stage, "open", "{} should fail to open: {o:?}", e.name);
            assert!(o.error.is_some());
        }
    }
}

/// The scrub path (forward-continue) and a repeated request return the same frame as a cold seek.
#[test]
#[ignore = "needs generated media"]
fn repeated_and_continued_requests_return_the_target() {
    s3::init(true);
    let mut src = VideoSource::open(
        &media().join("h264_1080p30_gop30.mov"),
        Options {
            forward_continue: true,
            ..Options::default()
        },
    )
    .expect("open");
    for pi in [10, 10, 11, 25, 31, 31, 200, 5] {
        src.seek_frame(pi).expect("seek");
        assert_eq!(src.frame().pts(), Some(src.index.frames[pi].pts));
    }
    assert!(src.stats.continued >= 3, "{:?}", src.stats);
}
