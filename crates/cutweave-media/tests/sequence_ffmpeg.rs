// SPDX-License-Identifier: GPL-3.0-or-later
//! Image sequences on the FFmpeg backend (WP-2F.05c, ADR-0035): a run of numbered PNGs probes
//! as one video from its first file, its frame index is computed, and every frame decodes to
//! exactly its own picture (a frame-number burn-in), in order and by random access. A missing
//! number shows the picture before it, flagged damaged.
//!
//! The pictures are written at test time by `cutweave_media::testing::stills::numbered_png`:
//! no corpus, no network. Needs the FFmpeg bundle (`just test-ffmpeg`, docs/dev/ffmpeg.md).
#![cfg(feature = "ffmpeg")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use cutweave_media::testing::contract::TempDir;
use cutweave_media::testing::stills::{burned_number, numbered_png};
use cutweave_media::{
    DecodeMode, DecodeOpts, DecoderPool, FfmpegBackend, HashMode, MediaBackend, MediaError,
    PixelFormat, SequenceSource, VideoFrame, content_hash,
};
use cutweave_model::{Duration, Extra, FrameRate, ImageSequence, StreamKind, Time};

const W: u32 = 32;
const H: u32 = 18;

fn backend() -> FfmpegBackend {
    FfmpegBackend::new().expect("the loaded FFmpeg matches the build")
}

/// Writes `shot 50%_NNNN.png` for `first..first + count` except `missing` and returns the
/// sequence. The name has a space and a percent sign on purpose.
fn write_run(
    dir: &TempDir,
    first: u32,
    count: u32,
    missing: &[u32],
    fps: FrameRate,
) -> SequenceSource {
    for number in (first..first + count).filter(|n| !missing.contains(n)) {
        let path = dir.path().join(format!("shot 50%_{number:04}.png"));
        std::fs::write(path, numbered_png(W, H, number)).unwrap();
    }
    SequenceSource {
        dir: dir.path().to_path_buf(),
        run: ImageSequence {
            pattern: "shot 50%%_%04d.png".into(),
            first,
            count,
            fps,
            missing: missing.to_vec(),
            extra: Extra::new(),
        },
    }
}

fn rgba_opts() -> DecodeOpts {
    DecodeOpts::default().with_target(PixelFormat::Rgba8)
}

/// The number burnt into `frame`, checked at its four corners and its middle.
fn number_of(frame: &VideoFrame) -> u32 {
    frame.validate().unwrap();
    assert_eq!(
        (frame.width, frame.height, frame.format),
        (W, H, PixelFormat::Rgba8)
    );
    let plane = &frame.planes[0];
    let at = |x: u32, y: u32| {
        let i = y as usize * plane.stride as usize + x as usize * 4;
        burned_number([plane.data[i], plane.data[i + 1], plane.data[i + 2]])
    };
    let number = at(0, 0);
    for (x, y) in [(W - 1, 0), (0, H - 1), (W - 1, H - 1), (W / 2, H / 2)] {
        assert_eq!(at(x, y), number, "pixel ({x}, {y})");
    }
    number
}

#[test]
fn a_run_probes_from_its_first_file_as_one_video() {
    let dir = TempDir::new("seq-probe");
    let run = write_run(&dir, 1, 300, &[], FrameRate::FPS_30);
    let locator = run.locator();
    let started = Instant::now();
    let probe = backend().probe(&locator).unwrap();
    let index = backend().index_media(&locator).unwrap();
    let hash = content_hash(&locator, HashMode::Fast).unwrap();
    let took = started.elapsed();
    assert_eq!(probe.duration, Some(Duration::from_secs(10).unwrap()));
    assert_eq!(probe.fps, Some(FrameRate::FPS_30));
    assert!(!probe.vfr);
    assert_eq!(probe.audio, None);
    assert_eq!(probe.streams.len(), 1);
    assert_eq!(probe.streams[0].kind, StreamKind::Video);
    assert_eq!(
        (probe.streams[0].width, probe.streams[0].height),
        (Some(W), Some(H))
    );
    assert_eq!(index.frame_count(), 300);
    assert_eq!(index.duration(), probe.duration);
    // The sequence and a still of its first picture have different cache keys; the same run
    // hashes the same again, another rate or length does not.
    let first = content_hash(&run.first_file(), HashMode::Fast).unwrap();
    assert_ne!(hash, first);
    assert_eq!(content_hash(&locator, HashMode::Fast).unwrap(), hash);
    let mut shorter = run.clone();
    shorter.run.count = 299;
    assert_ne!(
        content_hash(&shorter.locator(), HashMode::Fast).unwrap(),
        hash
    );
    // Probe only: far below the 3 s the import has (ADR-0035).
    assert!(took.as_secs_f64() < 3.0, "probing took {took:?}");
    // No sound, and a clear error when the first file is gone.
    assert!(matches!(
        backend().open_audio_decoder(&locator, None),
        Err(MediaError::NoStream { .. })
    ));
    let nowhere = SequenceSource {
        dir: dir.path().join("nowhere"),
        run: run.run.clone(),
    };
    assert!(matches!(
        backend().probe(&nowhere.locator()),
        Err(MediaError::NotFound { .. })
    ));
    assert!(matches!(
        backend().open_decoder(&nowhere.locator(), &rgba_opts()),
        Err(MediaError::NotFound { .. })
    ));
}

/// The acceptance criterion: 300 frames at 30 fps, every frame exact.
#[test]
fn every_frame_of_a_300_frame_run_is_exact_in_order_and_by_seek() {
    let dir = TempDir::new("seq-exact");
    let fps = FrameRate::FPS_30;
    let run = write_run(&dir, 1, 300, &[], fps);
    let locator = run.locator();
    let opts = rgba_opts().with_mode(DecodeMode::Sequential);
    let mut decoder = backend().open_decoder(&locator, &opts).unwrap();
    let mut n = 0_u32;
    while let Some(frame) = decoder.next_frame().unwrap() {
        assert_eq!(
            frame.pts,
            fps.frame_to_time(i64::from(n)).unwrap(),
            "frame {n}"
        );
        assert_eq!(number_of(&frame), 1 + n, "frame {n}");
        assert!(!frame.damaged, "frame {n}");
        n += 1;
    }
    assert_eq!(n, 300);

    // Random access, backwards and forwards, lands on the same pictures.
    let mut seeker = backend().open_decoder(&locator, &rgba_opts()).unwrap();
    for n in [299_u32, 0, 150, 151, 149, 7, 298, 1, 0] {
        let t = fps.frame_to_time(i64::from(n)).unwrap();
        let frame = seeker.seek_exact(t).unwrap();
        assert_eq!(
            (number_of(&frame), frame.pts),
            (1 + n, t),
            "seek to frame {n}"
        );
        // A time inside the frame shows the same frame.
        let inside = Time::from_flicks(t.flicks() + 1000).unwrap();
        assert_eq!(number_of(&seeker.seek_exact(inside).unwrap()), 1 + n);
    }
    let next = seeker.next_frame().unwrap().unwrap();
    assert_eq!(
        number_of(&next),
        2,
        "next_frame continues after the last seek"
    );
    let end = fps.frame_to_time(300).unwrap();
    assert!(matches!(
        seeker.seek_exact(end),
        Err(MediaError::OutOfRange { .. })
    ));
}

#[test]
fn a_missing_number_shows_the_picture_before_it_flagged() {
    let dir = TempDir::new("seq-gaps");
    let fps = FrameRate::FPS_23_976;
    let run = write_run(&dir, 98, 40, &[103, 104, 120], fps);
    let locator = run.locator();
    let mut decoder = backend().open_decoder(&locator, &rgba_opts()).unwrap();
    assert_eq!(decoder.index().frame_count(), 40);
    let mut shown = Vec::new();
    while let Some(frame) = decoder.next_frame().unwrap() {
        let n = i64::try_from(shown.len()).unwrap();
        assert_eq!(frame.pts, fps.frame_to_time(n).unwrap(), "frame {n}");
        shown.push((number_of(&frame), frame.damaged));
    }
    let expected: Vec<(u32, bool)> = (98..138)
        .map(|n| match n {
            103 | 104 => (102, true),
            120 => (119, true),
            n => (n, false),
        })
        .collect();
    assert_eq!(shown, expected);
    // Seeks across the gaps.
    for (frame, number) in [
        (39_i64, 137_u32),
        (6, 102),
        (5, 102),
        (4, 102),
        (7, 105),
        (22, 119),
    ] {
        let got = decoder
            .seek_exact(fps.frame_to_time(frame).unwrap())
            .unwrap();
        assert_eq!(number_of(&got), number, "frame {frame}");
    }
}

/// The decoder pool, the cache and export take a sequence by its locator like any file.
#[test]
fn the_decoder_pool_serves_a_sequence_by_its_locator() {
    let dir = TempDir::new("seq-pool");
    let fps = FrameRate::FPS_30;
    let run = write_run(&dir, 0, 12, &[], fps);
    let locator: PathBuf = run.locator();
    let pool = DecoderPool::new(Arc::new(backend()), locator);
    let token = cutweave_media::CancelToken::new();
    for n in [3_i64, 4, 11, 0] {
        let t = fps.frame_to_time(n).unwrap();
        let mut lease = pool.checkout(&rgba_opts(), Some(t), &token).unwrap();
        let frame = cutweave_media::VideoDecoder::seek_exact(&mut lease, t).unwrap();
        assert_eq!(i64::from(number_of(&frame)), n);
    }
    assert_eq!(pool.index().map(|i| i.frame_count()), Some(12));
}
