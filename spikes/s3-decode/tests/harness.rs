// SPDX-License-Identifier: GPL-3.0-or-later
//! Media-dependent harness tests. They need `python gen_media.py` (media/) and the FFmpeg
//! tools for ground truth, so they are `#[ignore]`d: run with `cargo test -- --ignored`.

use std::path::{Path, PathBuf};

use s3::manifest;
use s3::robust;
use s3::source::{Options, Threads, VideoSource};
use s3::verify::{self, Config};

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
                forward_continue: false,
            },
            Options {
                threads: Threads::Slice(0),
                skip_nonref: true,
                forward_continue: false,
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
