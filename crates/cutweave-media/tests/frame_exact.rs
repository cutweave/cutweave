// SPDX-License-Identifier: GPL-3.0-or-later
//! Frame-exact seek on the corpus (WP-1D.03 acceptance, ADR-0003): every frame the decoder
//! returns is checked against the FFmpeg CLI of the same bundle by pts (the index's, which
//! WP-1D.02 checks against `ffprobe`) and by the CRC-32 of its Y plane
//! (`ffmpeg -vf extractplanes=y -f framehash`).
//!
//! Per file, four passes (the S3 harness's):
//! 1. **sequential**: `next_frame` over the whole stream;
//! 2. **random**: N seeded random seeks through one decoder (production behaviour: a seek that
//!    can decode forward does), each to a random time inside the target frame's display interval
//!    (`frame-exact: N/N`);
//! 3. **edges**: the first and last four frames, every keyframe ±1 and every open-GOP leading
//!    frame, forward and then in reverse;
//! 4. **scrub**: a forward sweep of about 60 positions plus single-frame steps.
//!
//! `cargo nextest run -p cutweave-media --features ffmpeg --test frame_exact` runs the 100-seek
//! tier (PR CI, `--tier small`); `-- --ignored` runs the 500-seek tier (nightly) and the 60 s 4K
//! clip. Files: the small-tier corpus (`cargo xtask fetch-corpus --tier small`; missing files
//! skip with a message), plus small clips generated with the `ffmpeg` CLI into cargo's test
//! tmpdir (never committed) for what the corpus lacks ([`support::GENERATED_CLIPS`]): odd sizes
//! (VP9, FFV1), open GOP (H.264 at 29.97 fps, HEVC 10-bit CRA/RASL in Matroska) and MPEG-TS.
#![cfg(feature = "ffmpeg")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::Path;

use cutweave_media::{DecodeOpts, FrameIndex, MediaBackend, VideoDecoder, VideoFrame};
use cutweave_model::Time;
use support::{Crc32, Rng};

/// Seeks per file in PR CI.
const SMALL_TIER_SEEKS: usize = 100;
/// Seeks per file nightly (`--ignored`).
const NIGHTLY_SEEKS: usize = 500;

struct Check<'a> {
    name: &'a str,
    index: &'a FrameIndex,
    truth: &'a [(i64, u32)],
    crc: Crc32,
    failures: Vec<String>,
    ok: usize,
    total: usize,
}

impl Check<'_> {
    /// Checks that `got` is frame `n`: its pts and Y-plane CRC, and that it is not flagged.
    fn frame(&mut self, pass: &str, n: usize, got: Result<VideoFrame, String>) {
        self.total += 1;
        let want_pts = self.index.time_of(n).expect("frame time");
        let result = got.and_then(|f| {
            if f.pts != want_pts {
                let got_n = self.index.frame_at(f.pts);
                return Err(format!(
                    "pts {:?} (frame {got_n}) instead of {want_pts:?}",
                    f.pts
                ));
            }
            let crc = self.crc.y_plane(&f);
            if crc != self.truth[n].1 {
                return Err(format!("Y crc {crc:08x} != ffmpeg {:08x}", self.truth[n].1));
            }
            if f.damaged {
                return Err("flagged damaged".to_owned());
            }
            f.validate().map_err(|e| e.to_string())
        });
        match result {
            Ok(()) => self.ok += 1,
            Err(e) => {
                if self.failures.len() < 20 {
                    self.failures
                        .push(format!("{}: {pass}: frame {n}: {e}", self.name));
                }
            }
        }
    }
}

/// A random time inside frame `n`'s display interval (until the next frame or the end).
fn time_inside(index: &FrameIndex, n: usize, rng: &mut Rng) -> Time {
    let start = index.time_of(n).expect("frame time");
    let next = index
        .time_of(n + 1)
        .or_else(|| index.end_time())
        .unwrap_or(start);
    let span = u64::try_from((next.flicks() - start.flicks()).max(1)).unwrap_or(1);
    let offset = i64::try_from(rng.next_u64() % span).unwrap_or(0);
    // Every other seek lands exactly on the frame's own time.
    let offset = if rng.next_u64().is_multiple_of(2) {
        0
    } else {
        offset
    };
    Time::from_flicks(start.flicks() + offset).expect("time")
}

fn seek(d: &mut dyn VideoDecoder, t: Time) -> Result<VideoFrame, String> {
    d.seek_exact(t)
        .map_err(|e| format!("seek_exact({t:?}): {e}"))
}

/// Runs the four passes over `path` with `seeks` random seeks.
#[allow(clippy::too_many_lines)]
fn verify(path: &Path, seeks: usize) {
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    let backend = support::backend();
    let index = backend
        .index_media(path)
        .unwrap_or_else(|e| panic!("{name}: index: {e}"));
    let Some(truth) = support::y_crcs(path, index.stream()) else {
        return;
    };
    let frames = index.frame_count();
    assert_eq!(truth.len(), frames, "{name}: ffmpeg frames vs index frames");
    for (n, (pts, _)) in truth.iter().enumerate() {
        assert_eq!(
            *pts,
            index.frames()[n].pts,
            "{name}: frame {n} pts vs ffmpeg"
        );
    }
    let index = std::sync::Arc::new(index);
    let opts = DecodeOpts::default().with_index(std::sync::Arc::clone(&index));
    let open = || {
        backend
            .open_decoder(path, &opts)
            .unwrap_or_else(|e| panic!("{name}: open_decoder: {e}"))
    };
    let mut check = Check {
        name: &name,
        index: &index,
        truth: &truth,
        crc: Crc32::default(),
        failures: Vec::new(),
        ok: 0,
        total: 0,
    };

    // 1. Sequential.
    let mut d = open();
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
    let sequential = (check.ok, check.total);

    // 2. Random seeks through one decoder.
    let mut d = open();
    let mut rng = Rng::new(support::seed(&name));
    let before = check.ok;
    for _ in 0..seeks {
        let n = rng.below(frames);
        let t = time_inside(&index, n, &mut rng);
        check.frame("random", n, seek(&mut *d, t));
    }
    let random = check.ok - before;

    // 3. Edges, forward then reversed, on a fresh decoder each way.
    let mut edges: Vec<usize> = (0..4.min(frames))
        .chain(frames.saturating_sub(4)..frames)
        .collect();
    for k in index.key_frames() {
        edges.extend([k.saturating_sub(1), k, (k + 1).min(frames - 1)]);
    }
    // Open-GOP leading pictures: decoded after a keyframe but shown before it, so they start
    // from an earlier keyframe than the latest one in decode order.
    let leading: Vec<usize> = (0..frames)
        .filter(|&n| {
            let f = index.frames()[n];
            index.keys_before(f.decode_idx + 1).next() != Some(f.start_idx)
        })
        .collect();
    edges.extend(&leading);
    edges.sort_unstable();
    edges.dedup();
    let before = (check.ok, check.total);
    for order in [edges.clone(), edges.iter().rev().copied().collect()] {
        let mut d = open();
        for n in order {
            let t = index.time_of(n).expect("time");
            check.frame("edges", n, seek(&mut *d, t));
        }
    }
    let edge = (check.ok - before.0, check.total - before.1);

    // 4. Scrub: a forward sweep, then single-frame steps, on one decoder.
    let mut d = open();
    let before = (check.ok, check.total);
    let step = (frames / 60).max(1);
    for n in (0..frames).step_by(step) {
        let t = time_inside(&index, n, &mut rng);
        check.frame("scrub", n, seek(&mut *d, t));
    }
    let start = rng.below(frames.saturating_sub(20).max(1));
    for n in start..(start + 20).min(frames) {
        let t = index.time_of(n).expect("time");
        check.frame("scrub", n, seek(&mut *d, t));
    }
    let scrub = (check.ok - before.0, check.total - before.1);

    eprintln!(
        "{name}: {frames} frames, {} keyframes, {} open-GOP leading, max decode distance {}; \
         sequential {}/{}, frame-exact: {random}/{seeks}, edges {}/{}, scrub {}/{}",
        index.key_frames().count(),
        leading.len(),
        index.max_decode_distance(),
        sequential.0,
        sequential.1,
        edge.0,
        edge.1,
        scrub.0,
        scrub.1,
    );
    assert!(
        check.failures.is_empty(),
        "{name}: {} of {} checks failed:\n{}",
        check.total - check.ok,
        check.total,
        check.failures.join("\n")
    );
}

fn corpus(name: &str, seeks: usize) {
    if let Some(path) = support::corpus_file(name) {
        verify(&path, seeks);
    }
}

/// A clip generated for what the corpus lacks ([`support::GENERATED_CLIPS`]).
fn generated(name: &str, seeks: usize) {
    if let Some(path) = support::generated_clip(name) {
        verify(&path, seeks);
    }
}

macro_rules! frame_exact {
    ($module:ident, $kind:ident, $file:literal) => {
        mod $module {
            #[test]
            fn small_tier_100_seeks() {
                super::$kind($file, super::SMALL_TIER_SEEKS);
            }

            #[test]
            #[ignore = "nightly tier: 500 seeks (`-- --ignored`)"]
            fn nightly_500_seeks() {
                super::$kind($file, super::NIGHTLY_SEEKS);
            }
        }
    };
}

// The four WBS files: H.264 4K30 long GOP, HEVC 10-bit, ProRes, the VFR phone clip.
frame_exact!(h264_4k30_longgop, corpus, "h264_4k30_longgop.mp4");
frame_exact!(hevc_10bit_sdr, corpus, "hevc_10bit_sdr.mp4");
frame_exact!(hevc_10bit_hdr10_pq, corpus, "hevc_10bit_hdr10_pq.mp4");
frame_exact!(prores_422hq_1080p, corpus, "prores_422hq_1080p.mov");
frame_exact!(vfr_phone_rotated90, corpus, "vfr_phone_rotated90.mp4");
// The rest of the small tier's video.
frame_exact!(h264_1080p60, corpus, "h264_1080p60.mp4");
frame_exact!(no_audio_stream, corpus, "no_audio_stream.mp4");
frame_exact!(sintel_trailer_480p, corpus, "sintel_trailer_480p.mp4");
// Odd sizes, open GOP and containers the corpus lacks.
frame_exact!(vp9_odd_size, generated, "vp9_99x57.webm");
frame_exact!(ffv1_odd_size, generated, "ffv1_101x55.mkv");
frame_exact!(h264_open_gop, generated, "h264_opengop_2997.mp4");
frame_exact!(hevc_open_gop_matroska, generated, "hevc_opengop_10bit.mkv");
frame_exact!(h264_mpeg_ts, generated, "h264_gop25.ts");

/// The 60 s 4K long-GOP clip (generated by the probe/index tests; 1 800 frames): nightly only.
#[test]
#[ignore = "nightly tier: 60 s of 4K, 500 seeks (`-- --ignored`)"]
fn h264_4k30_longgop_60s_nightly_500_seeks() {
    corpus("h264_4k30_longgop_60s.mp4", NIGHTLY_SEEKS);
}
