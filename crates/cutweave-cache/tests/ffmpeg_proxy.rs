// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2F.01 acceptance with the FFmpeg backend and real media:
//!
//! - the corpus 4K long-GOP clip becomes a 960×540 ProRes Proxy with PCM sound and the same
//!   frame count, at a recorded speed (`RECORD proxy …`: seconds of media per second);
//! - frame mapping: on a variable-frame-rate clip whose pictures carry their frame number as a
//!   barcode (made here with the bundle's `ffmpeg`), 500 random frames of the proxy show the
//!   original's frame number: 100%;
//! - kill and resume: a generation in a child process is killed (`Child::kill`, a hard kill)
//!   after a few parts; a new generation reuses them and completes;
//! - scrubbing: random seeks on the proxy against the 4K long-GOP original (hardware decode,
//!   then software), `RECORD scrub …` p50 / p95; the proxy's p95 is gated at 80 ms (arch §19,
//!   best of three attempts on a loaded machine), the original's is recorded.
//!
//! `. scripts/ffmpeg/env.sh && cargo nextest run -p cutweave-cache --features ffmpeg --test
//! ffmpeg_proxy --no-capture`. The corpus is `testdata/corpus` (`CUTWEAVE_CORPUS_DIR`
//! overrides; a worktree falls back to the main checkout's); missing files or tools skip with a
//! message. The 4K HEVC throughput run is `#[ignore]`d (it makes a 30 s 4K HEVC 10-bit clip
//! first): `--run-ignored only`.
#![cfg(feature = "ffmpeg")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)] // test helpers and measurements

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration as Wall, Instant};

use cutweave_cache::proxy::{
    GenerateOptions, ProxyGenerator, ProxyProfile, ProxySource, ProxyState, ProxyStore,
};
use cutweave_cache::{CacheDb, HashMode};
use cutweave_media::{
    CancelToken, DecodeMode, DecodeOpts, DecoderPool, FfmpegBackend, HwMode, MediaBackend,
    PixelFormat, VideoDecoder, VideoFrame,
};
use cutweave_model::StreamKind;

/// The environment variable that makes `child_generation` generate (the kill-and-resume test
/// runs this test binary as its child): the cache directory, the original and the part size,
/// separated by `|`.
const CHILD_ENV: &str = "CUTWEAVE_PROXY_CHILD";

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

/// The bundle's `ffmpeg`, else the one on `PATH`.
fn ffmpeg() -> Option<PathBuf> {
    let exe = format!("ffmpeg{}", std::env::consts::EXE_SUFFIX);
    let candidate = std::env::var_os("FFMPEG_DIR")
        .map(|d| PathBuf::from(d).join("bin").join(&exe))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from(&exe));
    let ok = Command::new(&candidate)
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success());
    if ok {
        Some(candidate)
    } else {
        eprintln!("skipping: no ffmpeg (the bundle's bin/ or PATH)");
        None
    }
}

/// A scratch directory in cargo's per-crate test tmpdir (git-ignored), emptied first.
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Generates `name` with `ffmpeg` (lavfi sources) into the scratch media directory unless it
/// is there.
fn generated(name: &str, args: &[&str]) -> Option<PathBuf> {
    let ffmpeg = ffmpeg()?;
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("proxy-media");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    if !path.is_file() {
        let tmp = dir.join(format!("partial-{}-{name}", std::process::id()));
        let out = Command::new(&ffmpeg)
            .args(["-v", "error", "-nostdin", "-y"])
            .args(args)
            .arg(&tmp)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "ffmpeg failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let _ = std::fs::rename(&tmp, &path);
        let _ = std::fs::remove_file(&tmp);
    }
    Some(path)
}

fn backend() -> Arc<FfmpegBackend> {
    Arc::new(FfmpegBackend::new().expect("FFmpeg matches the build"))
}

struct Setup {
    backend: Arc<FfmpegBackend>,
    db: Arc<CacheDb>,
    store: Arc<ProxyStore>,
    source: ProxySource,
}

fn setup(cache: &Path, original: &Path) -> Setup {
    let backend = backend();
    let db = Arc::new(CacheDb::open(&cache.join("cache.sqlite")).unwrap());
    let store = Arc::new(ProxyStore::new(
        Arc::clone(&db),
        cache.join("proxies"),
        ProxyStore::DEFAULT_MAX_BYTES,
    ));
    let source = ProxySource {
        path: original.to_owned(),
        hash: cutweave_media::content_hash(original, HashMode::Fast).unwrap(),
        probe: backend.probe(original).unwrap(),
    };
    Setup {
        backend,
        db,
        store,
        source,
    }
}

fn generator(s: &Setup, part_frames: usize) -> ProxyGenerator {
    ProxyGenerator::new(
        s.backend.clone(),
        Arc::clone(&s.db),
        Arc::clone(&s.store),
        GenerateOptions {
            part_frames,
            ..GenerateOptions::default()
        },
    )
}

fn profile(s: &Setup) -> ProxyProfile {
    ProxyProfile::for_backend(&s.backend.capabilities()).expect("a proxy codec")
}

fn seconds(s: &Setup) -> f64 {
    s.source
        .probe
        .duration
        .map_or(0.0, cutweave_model::Duration::as_secs_f64)
}

/// Generates and records the speed; returns the ready proxy's frame count.
fn generate_and_record(label: &str, original: &Path) -> u64 {
    let cache = scratch(&format!("proxy-speed-{label}"));
    let s = setup(&cache, original);
    // Index first, as the `index` stage does before the proxy job runs.
    s.db.frame_index_for(&*s.backend, &s.source.path, &s.source.hash)
        .unwrap();
    let began = Instant::now();
    let report = generator(&s, cutweave_cache::proxy::DEFAULT_PART_FRAMES)
        .generate(&s.source, &profile(&s), &CancelToken::new(), &mut |_| {})
        .unwrap();
    let took = began.elapsed().as_secs_f64();
    let media = seconds(&s);
    eprintln!(
        "RECORD proxy {label}: {media:.1} s of media in {took:.2} s = {:.2}x realtime, {} frames, \
         {:.1} MB ({})",
        media / took,
        report.proxy.frames,
        report.proxy.bytes as f64 / 1e6,
        report.proxy.profile,
    );
    assert_eq!(report.proxy.state, ProxyState::Ready);
    let probe = s.backend.probe(&report.proxy.path).unwrap();
    let video = probe
        .streams
        .iter()
        .find(|st| st.kind == StreamKind::Video)
        .unwrap();
    assert_eq!(video.codec.as_deref(), Some("prores"));
    let original = s
        .source
        .probe
        .streams
        .iter()
        .find(|st| st.kind == StreamKind::Video)
        .unwrap();
    let expected = profile(&s).size_for((original.width.unwrap(), original.height.unwrap()));
    assert_eq!(
        (video.width, video.height),
        (Some(expected.0), Some(expected.1))
    );
    if s.source.probe.audio.is_some() {
        let audio = probe.audio.unwrap();
        assert_eq!(Some(audio), s.source.probe.audio);
    }
    let _ = std::fs::remove_dir_all(&cache);
    report.proxy.frames
}

#[test]
fn the_4k_long_gop_corpus_clip_becomes_a_prores_proxy() {
    let Some(original) = corpus_file("h264_4k30_longgop.mp4") else {
        return;
    };
    assert_eq!(generate_and_record("4k-h264-8s", &original), 240);
}

#[test]
#[ignore = "makes a 30 s 4K HEVC 10-bit clip first (about a minute); run with --run-ignored only"]
fn proxy_generation_speed_on_4k_hevc() {
    let Some(original) = generated(
        "hevc_4k_10bit_30s.mp4",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=3840x2160:rate=30:duration=30",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=30",
            "-c:v",
            "libx265",
            "-preset",
            "ultrafast",
            "-x265-params",
            "keyint=60:log-level=error",
            "-pix_fmt",
            "yuv420p10le",
            "-tag:v",
            "hvc1",
            "-c:a",
            "aac",
            "-shortest",
        ],
    ) else {
        return;
    };
    assert_eq!(generate_and_record("4k-hevc10-30s", &original), 900);
    if let Some(long) = corpus_file("h264_4k30_longgop_60s.mp4") {
        generate_and_record("4k-h264-60s", &long);
    }
}

/// The frame number a barcode frame carries: 16 columns, bit `i` (least significant first) set
/// when column `i` is bright (luma, coded orientation; 8- or 10-bit, any size).
fn barcode(frame: &VideoFrame) -> u32 {
    let wide = matches!(
        frame.format,
        PixelFormat::Yuv420p10 | PixelFormat::Yuv422p10 | PixelFormat::P010
    );
    let row = frame.row(0, frame.height / 2).unwrap();
    let luma = |x: u32| -> u32 {
        if wide {
            let i = (x * 2) as usize;
            let v = u32::from(u16::from_le_bytes([row[i], row[i + 1]]));
            if frame.format == PixelFormat::P010 {
                v >> 8
            } else {
                v >> 2
            }
        } else {
            u32::from(row[x as usize])
        }
    };
    (0..16).fold(0, |acc, bit| {
        let x = (2 * bit + 1) * frame.width / 32;
        acc | (u32::from(luma(x) > 128) << bit)
    })
}

#[test]
fn every_proxy_frame_of_a_vfr_clip_is_the_original_frame() {
    // 60 fps pictures whose 16 columns spell the frame number, thinned to a variable frame
    // rate (every second frame plus every seventh: gaps of 16.7, 33.3 and 50 ms).
    let Some(original) = generated(
        "vfr_barcode.mp4",
        &[
            "-f",
            "lavfi",
            "-i",
            "nullsrc=size=640x360:rate=60:duration=12",
            "-vf",
            "geq=lum='if(mod(floor(N/pow(2,floor(X*16/W))),2),235,16)':cb=128:cr=128,\
             select='not(mod(n,2))+not(mod(n,7))'",
            "-fps_mode",
            "vfr",
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-crf",
            "12",
            "-g",
            "90",
            "-pix_fmt",
            "yuv420p",
        ],
    ) else {
        return;
    };
    let cache = scratch("proxy-vfr");
    let s = setup(&cache, &original);
    let report = generator(&s, 200)
        .generate(&s.source, &profile(&s), &CancelToken::new(), &mut |_| {})
        .unwrap();
    let ready = s
        .store
        .ready(&s.source.hash, &report.proxy.profile)
        .unwrap()
        .unwrap();
    let index = &ready.source_index;
    assert!(index.is_vfr(), "the clip has a variable frame rate");
    let frames = index.frame_count();
    assert_eq!(ready.index.frame_count(), frames);
    let mut src = s
        .backend
        .open_decoder(
            &original,
            &DecodeOpts::default()
                .with_hw(HwMode::Off)
                .with_index(Arc::clone(index)),
        )
        .unwrap();
    let mut proxy = s
        .backend
        .open_decoder(
            &ready.record.path,
            &DecodeOpts::default()
                .with_hw(HwMode::Off)
                .with_index(Arc::clone(&ready.index)),
        )
        .unwrap();
    // 500 random frames (seeded), plus both ends.
    let mut state = 0x2f01_u64;
    let mut picks: Vec<usize> = (0..500)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            usize::try_from((state >> 33) % frames as u64).unwrap()
        })
        .collect();
    picks.extend([0, frames - 1]);
    let mut matched = 0;
    for n in &picks {
        let t = index.time_of(*n).unwrap();
        let (k, _) = ready.frame_at(t).unwrap();
        assert_eq!(k, *n);
        let want = barcode(&src.seek_exact(t).unwrap());
        let got = barcode(&proxy.seek_exact(ready.index.time_of(k).unwrap()).unwrap());
        if want == got {
            matched += 1;
        } else {
            eprintln!("frame {n}: original {want}, proxy {got}");
        }
    }
    eprintln!(
        "RECORD proxy frame mapping: {matched}/{} frames of a VFR clip ({frames} frames)",
        picks.len()
    );
    assert_eq!(matched, picks.len(), "100% of the frames map");
    let _ = std::fs::remove_dir_all(&cache);
}

/// The child half of `a_killed_generation_resumes_from_its_parts` (a no-op otherwise).
#[test]
fn child_generation() {
    let Some(spec) = std::env::var_os(CHILD_ENV) else {
        return;
    };
    let spec = spec.to_string_lossy().into_owned();
    let mut parts = spec.split('|');
    let (cache, original, part_frames) = (
        PathBuf::from(parts.next().unwrap()),
        PathBuf::from(parts.next().unwrap()),
        parts.next().unwrap().parse::<usize>().unwrap(),
    );
    let s = setup(&cache, &original);
    // Runs until killed (or finishes, which the parent then reports).
    generator(&s, part_frames)
        .generate(&s.source, &profile(&s), &CancelToken::new(), &mut |_| {})
        .unwrap();
}

#[test]
fn a_killed_generation_resumes_from_its_parts() {
    let Some(original) = corpus_file("h264_4k30_longgop.mp4") else {
        return;
    };
    let cache = scratch("proxy-kill");
    let part_frames = 16;
    let s = setup(&cache, &original);
    let key = profile(&s).key();
    let parts_dir = s.store.dir(&s.source.hash, &key).join("parts");
    let finished = |dir: &Path| -> usize {
        std::fs::read_dir(dir).map_or(0, |entries| {
            entries
                .flatten()
                .filter(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    Path::new(&name)
                        .extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("mov"))
                        && !name.starts_with('.')
                })
                .count()
        })
    };
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "child_generation", "--nocapture"])
        .env(
            CHILD_ENV,
            format!("{}|{}|{part_frames}", cache.display(), original.display()),
        )
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Wall::from_secs(120);
    while finished(&parts_dir) < 3 {
        assert!(Instant::now() < deadline, "the child made no parts");
        assert!(
            child.try_wait().unwrap().is_none(),
            "the child finished before it was killed"
        );
        std::thread::sleep(Wall::from_millis(10));
    }
    child.kill().unwrap();
    child.wait().unwrap();
    let kept = finished(&parts_dir);
    assert_eq!(
        s.store.get(&s.source.hash, &key).unwrap().unwrap().state,
        ProxyState::Building
    );
    // A new engine (backend, database connection, generator) finishes the job.
    drop(s);
    let s = setup(&cache, &original);
    let report = generator(&s, part_frames)
        .generate(&s.source, &profile(&s), &CancelToken::new(), &mut |_| {})
        .unwrap();
    let total = report.proxy.frames.div_ceil(part_frames as u64) as usize;
    eprintln!(
        "RECORD proxy resume: killed after {kept} of {total} parts; resumed with {} reused and {} \
         encoded",
        report.reused_parts, report.encoded_parts
    );
    assert!(report.reused_parts >= kept.min(total), "{report:?}");
    assert_eq!(report.reused_parts + report.encoded_parts, total);
    let index = s.db.get_main_frame_index(&s.source.hash).unwrap().unwrap();
    assert_eq!(report.proxy.frames, index.frame_count() as u64);
    assert_eq!(
        s.backend
            .index_media(&report.proxy.path)
            .unwrap()
            .frame_count(),
        index.frame_count()
    );
    let _ = std::fs::remove_dir_all(&cache);
}

/// `(p50, p95)` in milliseconds.
fn percentiles(mut v: Vec<f64>) -> (f64, f64) {
    v.sort_by(f64::total_cmp);
    let at = |p: f64| v[((v.len() - 1) as f64 * p).round() as usize];
    (at(0.5), at(0.95))
}

/// Random seeks through a pooled seek decoder, as the preview's scrub decodes: the latency of
/// each (open included for the first), converted for the compositor like the preview does.
fn scrub(backend: &Arc<FfmpegBackend>, path: &Path, hw: HwMode, seeks: usize) -> (f64, f64) {
    let index = Arc::new(backend.index_media(path).unwrap());
    let pool = DecoderPool::new(backend.clone(), path).with_index(Arc::clone(&index));
    let frames = index.frame_count();
    let mut state = 0x5c2b_u64;
    let mut times = Vec::with_capacity(seeks);
    for _ in 0..seeks {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        let n = usize::try_from((state >> 33) % frames as u64).unwrap();
        let t = index.time_of(n).unwrap();
        let opts = DecodeOpts::default()
            .with_mode(DecodeMode::Seek)
            .with_hw(hw)
            .with_target(PixelFormat::P010);
        let began = Instant::now();
        let mut lease = pool.checkout(&opts, Some(t), &CancelToken::new()).unwrap();
        let f = lease.seek_exact(t).unwrap();
        times.push(began.elapsed().as_secs_f64() * 1e3);
        assert_eq!(f.pts, t);
    }
    percentiles(times)
}

#[test]
fn scrubbing_the_proxy_is_within_budget() {
    let Some(original) = corpus_file("h264_4k30_longgop.mp4") else {
        return;
    };
    let cache = scratch("proxy-scrub");
    let s = setup(&cache, &original);
    let report = generator(&s, cutweave_cache::proxy::DEFAULT_PART_FRAMES)
        .generate(&s.source, &profile(&s), &CancelToken::new(), &mut |_| {})
        .unwrap();
    let proxy = report.proxy.path;
    // Best of three: the machine runs other work.
    let mut best = f64::INFINITY;
    for attempt in 0..3 {
        let (p50, p95) = scrub(&s.backend, &proxy, HwMode::Off, 100);
        eprintln!("RECORD scrub proxy (attempt {attempt}): p50 {p50:.1} ms, p95 {p95:.1} ms");
        best = best.min(p95);
        if best < 80.0 {
            break;
        }
    }
    let (hw50, hw95) = scrub(&s.backend, &original, HwMode::Auto, 30);
    let (sw50, sw95) = scrub(&s.backend, &original, HwMode::Off, 30);
    eprintln!(
        "RECORD scrub 4K long-GOP original: hardware/auto p50 {hw50:.0} ms, p95 {hw95:.0} ms; \
         software p50 {sw50:.0} ms, p95 {sw95:.0} ms (8.3 s GOP: recorded, not gated)"
    );
    assert!(best < 80.0, "proxy scrub p95 {best:.1} ms (budget 80 ms)");
    let _ = std::fs::remove_dir_all(&cache);
}
