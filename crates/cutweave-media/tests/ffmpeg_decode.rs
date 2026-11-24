// SPDX-License-Identifier: GPL-3.0-or-later
//! FFmpeg decoding on the corpus beyond frame-exactness (WP-1D.03): native layouts and color
//! tags per file, coded orientation of rotated clips, the RGBA thumbnail path, odd sizes,
//! stills, files without audio, audio sample counts against `ffprobe`, truncated files, the
//! decoder pool over FFmpeg, and cancelling pending 4K long-GOP seeks (the latency budget is
//! measured alone, in `seek_bench`).
//!
//! Missing corpus files or CLI tools skip with a message (`cargo xtask fetch-corpus --tier
//! small`; docs/dev/ffmpeg.md). No network.
#![cfg(feature = "ffmpeg")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use cutweave_media::{
    CancelToken, DecodeOpts, DecoderPool, HwMode, MediaBackend, MediaError, PixelFormat,
    VideoDecoder, VideoFrame,
};
use cutweave_model::{
    ColorPrimaries, ColorRange, Duration, MatrixCoefficients, SampleRate, StreamKind, Time,
    TransferCharacteristics,
};

fn open(path: &Path, opts: &DecodeOpts) -> Box<dyn VideoDecoder> {
    support::backend()
        .open_decoder(path, opts)
        .unwrap_or_else(|e| panic!("open_decoder({}): {e}", path.display()))
}

fn secs(s: f64) -> Time {
    #[allow(clippy::cast_possible_truncation)]
    Time::from_flicks((s * 705_600_000.0) as i64).unwrap()
}

fn middle(d: &mut dyn VideoDecoder) -> VideoFrame {
    let index = d.index();
    let t = index.time_of(index.frame_count() / 2).unwrap();
    d.seek_exact(t).unwrap()
}

/// The software decoders' native layouts (hardware decoders return NV12 / P010 instead:
/// `hw_decode.rs`).
#[test]
fn native_layouts_and_color_tags_per_file() {
    let bt709 = (
        Some(ColorPrimaries::Bt709),
        Some(TransferCharacteristics::Bt709),
        Some(MatrixCoefficients::Bt709),
        Some(ColorRange::Limited),
    );
    let hdr10 = (
        Some(ColorPrimaries::Bt2020),
        Some(TransferCharacteristics::Pq),
        Some(MatrixCoefficients::Bt2020Ncl),
        Some(ColorRange::Limited),
    );
    let cases = [
        (
            "h264_4k30_longgop.mp4",
            PixelFormat::Yuv420p,
            (3840, 2160),
            None,
        ),
        ("h264_1080p60.mp4", PixelFormat::Yuv420p, (1920, 1080), None),
        (
            "hevc_10bit_sdr.mp4",
            PixelFormat::Yuv420p10,
            (1920, 1080),
            Some(bt709),
        ),
        (
            "hevc_10bit_hdr10_pq.mp4",
            PixelFormat::Yuv420p10,
            (1920, 1080),
            Some(hdr10),
        ),
        (
            "prores_422hq_1080p.mov",
            PixelFormat::Yuv422p10,
            (1920, 1080),
            None,
        ),
        // Coded orientation: the portrait clip is stored 1080 wide; the probe says rotate 90.
        (
            "vfr_phone_rotated90.mp4",
            PixelFormat::Yuv420p,
            (1080, 1920),
            None,
        ),
    ];
    for (name, format, size, tags) in cases {
        let Some(path) = support::corpus_file(name) else {
            continue;
        };
        let mut d = open(&path, &DecodeOpts::default().with_hw(HwMode::Off));
        let f = middle(&mut *d);
        assert_eq!(f.format, format, "{name}: layout");
        assert_eq!((f.width, f.height), size, "{name}: size");
        f.validate().unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(!f.damaged, "{name}: damaged");
        if let Some((p, t, m, r)) = tags {
            let c = &f.color;
            assert_eq!(
                (
                    c.primaries.clone(),
                    c.transfer.clone(),
                    c.matrix.clone(),
                    c.range
                ),
                (p, t, m, r),
                "{name}: color tags"
            );
        }
        if format.bit_depth() == 10 {
            // Low-aligned 10-bit samples: nothing above 1023.
            let max = f.planes[0]
                .data
                .as_chunks::<2>()
                .0
                .iter()
                .map(|s| u16::from_le_bytes(*s))
                .max()
                .unwrap();
            assert!(
                max <= 1023 && max > 255,
                "{name}: 10-bit samples up to {max}"
            );
        }
    }
    if let Some(path) = support::corpus_file("vfr_phone_rotated90.mp4") {
        // A quarter turn (the probe's own convention, WP-1D.02); frames stay as coded.
        let probe = support::backend().probe(&path).unwrap();
        assert_eq!(probe.rotation % 180, 90, "rotation {}", probe.rotation);
    }
}

/// BT.709 limited-range luma of an RGBA pixel, for comparing the thumbnail path with the Y
/// plane.
fn luma_of_rgba(px: &[u8]) -> f64 {
    let (r, g, b) = (f64::from(px[0]), f64::from(px[1]), f64::from(px[2]));
    16.0 + (0.2126 * r + 0.7152 * g + 0.0722 * b) * 219.0 / 255.0
}

#[test]
#[allow(clippy::cast_precision_loss)]
fn rgba_thumbnails_keep_the_picture() {
    for name in [
        "h264_1080p60.mp4",
        "hevc_10bit_sdr.mp4",
        "prores_422hq_1080p.mov",
    ] {
        let Some(path) = support::corpus_file(name) else {
            continue;
        };
        let native = middle(&mut *open(&path, &DecodeOpts::default()));
        let rgba = middle(&mut *open(
            &path,
            &DecodeOpts::default().with_target(PixelFormat::Rgba8),
        ));
        assert_eq!(rgba.format, PixelFormat::Rgba8);
        assert_eq!(
            (rgba.width, rgba.height, rgba.pts),
            (native.width, native.height, native.pts)
        );
        assert_eq!(rgba.color.matrix, Some(MatrixCoefficients::Rgb));
        assert_eq!(rgba.color.range, Some(ColorRange::Full));
        rgba.validate().unwrap();
        // Mean luma through the RGB matrix equals the mean of the Y plane (8-bit scale). The
        // native frame is P010 (high-aligned) from a hardware decoder, else low-aligned.
        let shift = u32::from(native.format.bit_depth() - 8);
        let samples = support::y_samples(&native);
        let y_sum: f64 = samples.iter().map(|&v| f64::from(v >> shift)).sum();
        let y_n = samples.len() as f64;
        let (mut c_sum, mut c_n) = (0.0, 0.0);
        for y in 0..rgba.height {
            for px in rgba.row(0, y).unwrap().as_chunks::<4>().0 {
                c_sum += luma_of_rgba(px);
                c_n += 1.0;
                assert_eq!(px[3], 255, "{name}: opaque");
            }
        }
        let (y_mean, c_mean) = (y_sum / y_n, c_sum / c_n);
        assert!(
            (y_mean - c_mean).abs() < 2.0,
            "{name}: mean luma {y_mean:.2} (Y plane) vs {c_mean:.2} (RGBA)"
        );
    }
}

#[test]
fn odd_sizes_keep_their_last_row_and_column() {
    let Some(path) = support::generated_clip("vp9_99x57.webm") else {
        return;
    };
    let mut d = open(&path, &DecodeOpts::default());
    let f = middle(&mut *d);
    assert_eq!(
        (f.width, f.height, f.format),
        (99, 57, PixelFormat::Yuv420p)
    );
    assert_eq!(f.planes[1].stride, 50);
    assert_eq!(f.planes[1].data.len(), 50 * 29);
    let rgba = middle(&mut *open(
        &path,
        &DecodeOpts::default().with_target(PixelFormat::Rgba8),
    ));
    assert_eq!((rgba.width, rgba.height), (99, 57));
    rgba.validate().unwrap();
}

#[test]
fn stills_decode_to_rgba_and_never_end() {
    let Some(path) = support::corpus_file("image_png.png") else {
        return;
    };
    let mut d = open(&path, &DecodeOpts::default());
    assert_eq!(d.index().frame_count(), 1);
    for t in [Time::ZERO, secs(10.0), secs(3600.0)] {
        let f = d.seek_exact(t).unwrap();
        assert_eq!(
            (f.format, f.width, f.height),
            (PixelFormat::Rgba8, 1920, 1080)
        );
        f.validate().unwrap();
    }
}

#[test]
fn files_without_audio_still_decode_video() {
    let Some(path) = support::corpus_file("no_audio_stream.mp4") else {
        return;
    };
    let err = support::backend().open_audio_decoder(&path, None).err();
    assert!(
        matches!(
            err,
            Some(MediaError::NoStream {
                kind: StreamKind::Audio,
                ..
            })
        ),
        "{err:?}"
    );
    let f = middle(&mut *open(&path, &DecodeOpts::default()));
    assert_eq!((f.width, f.height), (1280, 720));
}

#[test]
#[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
fn audio_sample_counts_match_ffprobe_and_seeks_are_placed_exactly() {
    // (file, rate, channels, samples per ffprobe's frame nb_samples)
    let cases = [
        ("audio_22050.wav", 22_050, 1, 110_250),
        ("audio_44100.wav", 44_100, 1, 220_500),
        ("audio_48000.wav", 48_000, 1, 240_000),
        ("audio_96000.wav", 96_000, 1, 480_000),
        ("audio_5_1.ac3", 48_000, 6, 241_152),
        ("h264_1080p60.mp4", 48_000, 1, 384_000),
        ("vfr_phone_rotated90.mp4", 48_000, 1, 480_256),
        ("prores_422hq_1080p.mov", 48_000, 1, 144_000),
    ];
    let backend = support::backend();
    for (name, hz, channels, samples) in cases {
        let Some(path) = support::corpus_file(name) else {
            continue;
        };
        let rate = SampleRate::new(hz).unwrap();
        let mut a = backend.open_audio_decoder(&path, None).unwrap();
        let format = a.format();
        assert_eq!(
            (format.rate, format.channels),
            (rate, channels),
            "{name}: format"
        );
        let whole = a
            .decode_range(Time::ZERO, Duration::from_secs(3600).unwrap())
            .unwrap();
        assert_eq!(whole.frames(), samples, "{name}: samples vs ffprobe");
        whole.validate().unwrap();
        let peak = whole.planes[0].iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        assert!((0.05..=1.0).contains(&peak), "{name}: peak {peak}");
        // Ranges after seeks, forward and back, sit exactly where the whole read has them.
        let mut a = backend.open_audio_decoder(&path, None).unwrap();
        for first in [samples * 3 / 4, samples / 5, samples / 2] {
            let len = hz as usize / 10;
            let t = rate.samples_to_time(first as i64).unwrap();
            let chunk = a
                .decode_range(
                    t,
                    Duration::try_from(rate.samples_to_time(len as i64).unwrap()).unwrap(),
                )
                .unwrap();
            assert_eq!(chunk.pts, t, "{name}: pts at {first}");
            assert_eq!(chunk.frames(), len, "{name}: length at {first}");
            let worst = chunk
                .planes
                .iter()
                .zip(&whole.planes)
                .flat_map(|(got, all)| {
                    got.iter()
                        .zip(&all[first..first + len])
                        .map(|(g, w)| (g - w).abs())
                })
                .fold(0.0_f32, f32::max);
            assert!(
                worst <= cutweave_media::testing::contract::AUDIO_SEEK_TOLERANCE,
                "{name}: samples {first}.. after a seek differ by {worst}"
            );
        }
    }
}

#[test]
fn truncated_files_never_block() {
    let Some(source) = support::corpus_file("h264_4k30_longgop.mp4") else {
        return;
    };
    // The 4K clip is fast-start (index first), so a cut keeps the index and loses media data.
    let bytes = std::fs::read(&source).unwrap();
    let path = support::scratch("truncated").join("h264_4k30_longgop_cut.mp4");
    std::fs::write(&path, &bytes[..bytes.len() * 3 / 5]).unwrap();
    let backend = support::backend();
    let index = Arc::new(backend.index_media(&path).unwrap());
    let full = backend.index_media(&source).unwrap();
    assert!(
        index.frame_count() < full.frame_count(),
        "the cut lost frames"
    );
    let mut d = open(&path, &DecodeOpts::default().with_index(Arc::clone(&index)));
    let t0 = Instant::now();
    let (mut ok, mut damaged) = (0, 0);
    for n in (0..index.frame_count())
        .step_by(7)
        .chain([index.frame_count() - 1])
    {
        let f = d
            .seek_exact(index.time_of(n).unwrap())
            .unwrap_or_else(|e| panic!("frame {n} of the cut file: {e}"));
        if f.damaged {
            damaged += 1;
        } else {
            ok += 1;
        }
    }
    eprintln!(
        "truncated 4K clip: {} of {} frames indexed, {ok} seeks exact, {damaged} damaged, {:?}",
        index.frame_count(),
        full.frame_count(),
        t0.elapsed()
    );
    assert!(ok > damaged, "{ok} exact vs {damaged} damaged");
}

#[test]
fn the_pool_shares_one_index_over_ffmpeg() {
    let Some(path) = support::corpus_file("h264_1080p60.mp4") else {
        return;
    };
    let pool = DecoderPool::new(Arc::new(support::backend()), &path);
    let never = CancelToken::new();
    let opts = DecodeOpts::default();
    let mut a = pool.checkout(&opts, None, &never).unwrap();
    let mut b = pool.checkout(&opts, None, &never).unwrap();
    assert!(
        Arc::ptr_eq(&a.index(), &b.index()),
        "one index for both decoders"
    );
    let fa = a.seek_exact(secs(2.0)).unwrap();
    let fb = b.seek_exact(secs(2.0)).unwrap();
    assert!(fa == fb, "two decoders agree");
    drop(a);
    drop(b);
    assert_eq!((pool.live(), pool.idle(), pool.opened()), (2, 2, 2));
}

/// Pending 4K long-GOP seeks, direct and through a pool lease, return
/// [`MediaError::Cancelled`] when cancelled, and the decoders stay exact. The latencies are
/// printed; the 10 ms budget is asserted by the `seek_bench` measurement, which runs alone (in
/// this parallel suite the machine is saturated and a wait measures the scheduler).
#[test]
fn pending_4k_seeks_cancel_and_decoders_stay_exact() {
    let Some((direct, pooled)) = support::pending_4k_cancels(4) else {
        return;
    };
    support::show_latencies("direct decoder, pending 4K seeks (loaded)", &direct);
    support::show_latencies("pooled lease, pending 4K seeks (loaded)", &pooled);
    // Every cancelled seek was still decoding when cancelled (hundreds of milliseconds of
    // work), so each one returned `Cancelled`.
    assert_eq!(direct.len(), 8, "direct seeks that returned Cancelled");
    assert_eq!(pooled.len(), 8, "pooled seeks that returned Cancelled");
}
