// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2D.01b: [`ReadBack`] on real long-GOP streams (feature `ffmpeg`; clips generated with the
//! bundle's `ffmpeg` CLI into the test scratch directory, never committed).
//!
//! - **Frame-exact:** every frame read back while playing a clip backwards, through a pooled
//!   playback decoder (frame threads) and windows smaller than its GOPs, is bit-identical to the
//!   frame an uncached seek returns. The clips: a pixel-coded counter (frame `N` carries `N` as
//!   eight black or white bars, so the right frame is checked from the pixels too) in H.264 with
//!   B-frames and 4-second GOPs, open-GOP H.264 at 29.97 fps and open-GOP 10-bit HEVC.
//! - **Measured** (ignored benchmark, run alone): the per-frame time of reverse playback of the
//!   1080p60 corpus clip (2-second GOPs) through a read-back, against a seek per frame.

#![cfg(feature = "ffmpeg")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use cutweave_media::{
    CancelToken, DecodeMode, DecodeOpts, DecoderPool, HwMode, MediaBackend, MediaError, ReadBack,
    VideoDecoder, VideoFrame,
};

/// Width of one bar of the counter.
const BAR: u32 = 32;
const COUNTER_W: u32 = 8 * BAR;
const COUNTER_H: u32 = 144;

/// A counter clip: 256×144, 30 fps, 8 s (240 frames), H.264 with 3 B-frames and a keyframe every
/// 120 frames; frame `N` has bar `b` white when bit `b` of `N` is set.
fn counter_clip() -> Option<std::path::PathBuf> {
    let source = format!("color=c=black:s={COUNTER_W}x{COUNTER_H}:r=30:d=8");
    let bars =
        format!("geq=lum='if(mod(floor(N/pow(2\\,floor(X/{BAR})))\\,2)\\,235\\,16)':cb=128:cr=128");
    support::generated(
        "counter_gop120.mp4",
        &[
            "-f",
            "lavfi",
            "-i",
            &source,
            "-vf",
            &bars,
            "-c:v",
            "libx264",
            "-g",
            "120",
            "-bf",
            "3",
            "-x264-params",
            "scenecut=0",
            "-pix_fmt",
            "yuv420p",
        ],
    )
}

/// The number the bars of a counter frame encode.
fn counter(frame: &VideoFrame) -> u32 {
    let y = &frame.planes[0];
    let row = (COUNTER_H / 2) as usize * y.stride as usize;
    (0..8)
        .filter(|b| y.data[row + (BAR / 2 + BAR * b) as usize] > 128)
        .map(|b| 1 << b)
        .sum()
}

/// Plays `path` backwards through a read-back of `capacity` frames on a pooled playback decoder
/// and checks every frame against an uncached seek; returns the windows decoded.
fn reverse_equals_seeks(path: &Path, capacity: usize, check: impl Fn(usize, &VideoFrame)) -> u64 {
    let backend: Arc<dyn MediaBackend> = Arc::new(support::backend());
    let index = Arc::new(backend.index_media(path).unwrap());
    let pool = DecoderPool::new(Arc::clone(&backend), path).with_index(Arc::clone(&index));
    let playback = DecodeOpts::default()
        .with_hw(HwMode::Off)
        .with_mode(DecodeMode::Sequential);
    let mut lease = pool
        .checkout(&playback, None, &CancelToken::new())
        .unwrap();
    let mut seeker = backend
        .open_decoder(
            path,
            &DecodeOpts::default()
                .with_hw(HwMode::Off)
                .with_index(Arc::clone(&index)),
        )
        .unwrap();
    let mut rb: ReadBack<Arc<VideoFrame>> = ReadBack::new(capacity);
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    for n in (0..index.frame_count()).rev() {
        let frame = match rb.get(n) {
            Some(f) => Arc::clone(f),
            None => rb
                .fill(&mut lease, &index, n, |f| Ok::<_, MediaError>(Arc::new(f)))
                .unwrap()
                .unwrap_or_else(|| panic!("{name}: frame {n} was not read back")),
        };
        let sought = seeker.seek_exact(index.time_of(n).unwrap()).unwrap();
        assert_eq!(
            frame.pts, sought.pts,
            "{name}: frame {n} read back at another time"
        );
        assert!(
            *frame == sought,
            "{name}: frame {n} read back differs from the uncached seek"
        );
        check(n, &frame);
    }
    rb.fills()
}

#[test]
fn reverse_read_back_equals_uncached_seeks_on_long_gop_streams() {
    let Some(clip) = counter_clip() else { return };
    // Windows of 16 inside 120-frame GOPs: eight windows per GOP, two GOPs.
    let fills = reverse_equals_seeks(&clip, 16, |n, frame| {
        assert_eq!(counter(frame), u32::try_from(n).unwrap(), "counter at {n}");
    });
    assert_eq!(fills, 16);
    // Whole GOPs: one window each.
    assert_eq!(reverse_equals_seeks(&clip, 256, |_, _| {}), 2);
    for name in ["h264_opengop_2997.mp4", "hevc_opengop_10bit.mkv"] {
        let Some(clip) = support::generated_clip(name) else {
            return;
        };
        reverse_equals_seeks(&clip, 24, |_, _| {});
    }
}

fn percentiles(name: &str, mut ms: Vec<f64>) -> (f64, f64, f64) {
    ms.sort_by(f64::total_cmp);
    let at = |q: f64| {
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let i = ((ms.len() - 1) as f64 * q).round() as usize;
        ms[i]
    };
    let (p50, p95, max) = (at(0.5), at(0.95), at(1.0));
    eprintln!(
        "{name}: {} frames, p50 {p50:.1} ms, p95 {p95:.1} ms, max {max:.1} ms",
        ms.len()
    );
    (p50, p95, max)
}

/// Reverse playback of the 1080p60 corpus clip (H.264, keyframes every 120 frames): per-frame
/// time through a read-back of 192 MiB, against a seek per frame (on a stretch of 120 frames).
#[test]
#[ignore = "benchmark: run alone with --run-ignored ignored-only --no-capture"]
fn reverse_read_back_of_the_1080p_corpus_clip() {
    let Some(path) = support::corpus_file("h264_1080p60.mp4") else {
        return;
    };
    let backend: Arc<dyn MediaBackend> = Arc::new(support::backend());
    let index = Arc::new(backend.index_media(&path).unwrap());
    let pool = DecoderPool::new(Arc::clone(&backend), &path).with_index(Arc::clone(&index));
    for hw in [HwMode::Off, HwMode::Auto] {
        let opts = DecodeOpts::default()
            .with_hw(hw)
            .with_mode(DecodeMode::Sequential);
        let mut lease = pool.checkout(&opts, None, &CancelToken::new()).unwrap();
        let label = format!("{hw:?} ({})", lease.hw_decoder().unwrap_or("software"));
        let frames = index.frame_count();
        let mut rb: ReadBack<Arc<VideoFrame>> =
            ReadBack::new(ReadBack::<()>::capacity_for(192 << 20, 1920 * 1080 * 3 / 2));
        let mut times = Vec::new();
        for n in (0..frames).rev() {
            let t = Instant::now();
            if rb.get(n).is_none() {
                rb.fill(&mut lease, &index, n, |f| Ok::<_, MediaError>(Arc::new(f)))
                    .unwrap();
            }
            times.push(t.elapsed().as_secs_f64() * 1e3);
        }
        percentiles(
            &format!(
                "{label} read-back, capacity {}, {} windows",
                rb.capacity(),
                rb.fills()
            ),
            times,
        );
        let mut times = Vec::new();
        for n in (frames - 120..frames).rev() {
            let t = Instant::now();
            lease.seek_exact(index.time_of(n).unwrap()).unwrap();
            times.push(t.elapsed().as_secs_f64() * 1e3);
        }
        percentiles(&format!("{label} seek per frame"), times);
    }
}
