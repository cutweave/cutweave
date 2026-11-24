// SPDX-License-Identifier: GPL-3.0-or-later
//! Thumbnails and filmstrips of real corpus clips through the FFmpeg backend (WP-1D.05).
//!
//! `. scripts/ffmpeg/env.sh && cargo nextest run -p cutweave-cache --features ffmpeg --test
//! ffmpeg_thumbs --no-capture` (add `--release` for the recorded timing). The corpus is the
//! git-ignored `testdata/corpus` (`CUTWEAVE_CORPUS_DIR` overrides; a worktree falls back to the
//! main checkout's). Missing files or tools skip with a message.
//!
//! The 60 s 1080p clip of the filmstrip criterion is made on first use from the 8 s
//! `h264_1080p60.mp4` (stream-copied in a loop with the bundle's `ffmpeg`) and removed afterwards. Timing is recorded, not gated (`RECORD ...` lines).
#![cfg(feature = "ffmpeg")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Instant;

use cutweave_cache::{CacheDb, FilmstripSpec, MediaRef, ThumbCache, ThumbConfig};
use cutweave_media::{CancelToken, FfmpegBackend};
use cutweave_model::Time;

fn corpus_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CUTWEAVE_CORPUS_DIR") {
        let dir = PathBuf::from(dir);
        return dir.is_dir().then_some(dir);
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let local = root.join("testdata/corpus");
    if local.is_dir() {
        return Some(local);
    }
    // A worktree's `.git` is a file: `gitdir: <main>/.git/worktrees/<name>`.
    let gitdir = std::fs::read_to_string(root.join(".git")).ok()?;
    let gitdir = PathBuf::from(gitdir.strip_prefix("gitdir:")?.trim());
    let dir = gitdir.parent()?.parent()?.parent()?.join("testdata/corpus");
    dir.is_dir().then_some(dir)
}

fn corpus_file(name: &str) -> Option<PathBuf> {
    let Some(dir) = corpus_dir() else {
        eprintln!("skipping: no corpus (cargo xtask fetch-corpus --tier small)");
        return None;
    };
    let path = dir.join(name);
    if path.is_file() {
        Some(path)
    } else {
        eprintln!("skipping: {name} is not in {}", dir.display());
        None
    }
}

fn cache(dir: &Path) -> ThumbCache {
    let backend = FfmpegBackend::new().expect("FFmpeg matches the build");
    ThumbCache::new(
        Arc::new(backend),
        Arc::new(CacheDb::open_in_memory().expect("db")),
        ThumbConfig::new(dir),
    )
}

fn temp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cutweave-cache-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn decode_jpeg(bytes: &[u8]) -> (u32, u32) {
    use zune_jpeg::JpegDecoder;
    use zune_jpeg::zune_core::bytestream::ZCursor;
    let mut d = JpegDecoder::new(ZCursor::new(bytes));
    d.decode().expect("valid JPEG");
    let info = d.info().expect("info");
    (u32::from(info.width), u32::from(info.height))
}

#[test]
fn a_corpus_thumb_is_a_jpeg_of_the_requested_width_and_the_second_request_is_a_hit() {
    let Some(path) = corpus_file("h264_1080p60.mp4") else {
        return;
    };
    let dir = temp("thumb");
    let cache = cache(&dir);
    let media = MediaRef::from_path(&path).expect("media");
    let cancel = CancelToken::new();
    let t = Time::from_secs(2).unwrap();

    let started = Instant::now();
    let first = cache.thumb(&media, t, 320, &cancel).expect("thumb");
    let cold = started.elapsed();
    assert_eq!((first.width, first.height), (320, 180));
    assert_eq!(decode_jpeg(&first.jpeg), (320, 180));

    let started = Instant::now();
    let cached = cache.cached_thumb(&media, t, 320).expect("lookup");
    let warm = started.elapsed();
    assert_eq!(cached.as_ref(), Some(&first), "served from the disk cache");
    assert_eq!(first.etag, cache.thumb_etag(&media, t, 320).unwrap());
    eprintln!("RECORD thumb 1080p60 -> 320 px: cold {cold:?}, cached {warm:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The 60 s 1080p clip, generated into `dir` (a per-test temp dir, never the shared corpus: the
/// looped copy has timestamp jumps that other tests' corpus-wide checks reject).
fn sixty_second_1080p(dir: &Path) -> Option<PathBuf> {
    let source = corpus_file("h264_1080p60.mp4")?;
    let out = dir.join("h264_1080p60_60s.mp4");
    let exe = std::env::var_os("FFMPEG_DIR")
        .map(|d| {
            PathBuf::from(d)
                .join("bin")
                .join(format!("ffmpeg{}", std::env::consts::EXE_SUFFIX))
        })
        .filter(|p| p.is_file());
    let Some(exe) = exe else {
        eprintln!("skipping: no 60 s 1080p clip and no ffmpeg CLI to generate it");
        return None;
    };
    eprintln!("generating {} (60 s of 1080p60)", out.display());
    let partial = out.with_extension("partial.mp4");
    let ok = Command::new(exe)
        .args(["-v", "error", "-y", "-stream_loop", "8", "-i"])
        .arg(&source)
        .args(["-t", "60", "-an", "-c:v", "copy"])
        .arg(&partial)
        .status()
        .is_ok_and(|s| s.success());
    if !ok || std::fs::rename(&partial, &out).is_err() {
        eprintln!("skipping: generating the 60 s clip failed");
        let _ = std::fs::remove_file(&partial);
        return None;
    }
    Some(out)
}

#[test]
fn a_filmstrip_of_a_60_second_1080p_clip_with_100_tiles() {
    let dir = temp("strip");
    let Some(path) = sixty_second_1080p(&dir) else {
        let _ = std::fs::remove_dir_all(&dir);
        return;
    };
    let cache = cache(&dir.join("cache"));
    let media = MediaRef::from_path(&path).expect("media");
    let cancel = CancelToken::new();
    let spec = FilmstripSpec {
        count: 100,
        tile_w: 160,
    };

    // The frame index scan is its own, one-off cost (stored in cache.sqlite at import).
    let started = Instant::now();
    cache
        .thumb(&media, Time::ZERO, 32, &cancel)
        .expect("warm the index");
    let index = started.elapsed();

    let started = Instant::now();
    let strip = cache.filmstrip(&media, spec, &cancel).expect("filmstrip");
    let took = started.elapsed();
    let m = strip.manifest;
    assert_eq!((m.tile_w, m.tile_h, m.count), (160, 90, 100));
    assert_eq!(decode_jpeg(&strip.jpeg), m.sprite_size());
    let seconds = m.interval.as_secs_f64() * f64::from(m.count);
    assert!((seconds - 60.0).abs() < 1.0, "interval {:?}", m.interval);

    let started = Instant::now();
    let again = cache.cached_filmstrip(&media, spec).expect("lookup");
    let cached = started.elapsed();
    assert_eq!(again.as_ref(), Some(&strip));
    eprintln!(
        "RECORD filmstrip 60 s 1080p, 100 tiles of 160 px ({} KB sprite): {took:?} \
         (index scan + first thumb {index:?}; cached lookup {cached:?}; criterion < 5 s, recorded)",
        strip.jpeg.len() / 1024
    );
    let _ = std::fs::remove_dir_all(&dir);
}
