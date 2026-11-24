// SPDX-License-Identifier: GPL-3.0-or-later
//! Helpers shared by the FFmpeg decode tests (WP-1D.03): the corpus, the bundle's CLI tools,
//! ground truth from them, CRC-32, a seeded RNG and the cancel-latency measurement. The port of
//! Spike S3's `truth.rs` and `util.rs`.
//!
//! Each test binary uses part of this module.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::thread;
use std::time::{Duration as Wall, Instant};

use cutweave_media::{
    CancelToken, DecodeOpts, DecoderPool, FfmpegBackend, FrameIndex, MediaBackend, MediaError,
    VideoDecoder, VideoFrame,
};
use cutweave_model::Time;

pub fn backend() -> FfmpegBackend {
    FfmpegBackend::new().expect("the loaded FFmpeg matches the build")
}

/// Cancels `rounds` pending seeks on `d`, each from frame 0 to the last frame (hundreds of
/// milliseconds of 4K decoding), at staggered moments; the latencies (ms) from `cancel()` to the
/// seek returning [`MediaError::Cancelled`]. `d` comes back, checked still exact.
pub fn cancel_latencies(name: &str, d: &mut Box<dyn VideoDecoder>, rounds: u64) -> Vec<f64> {
    let index = d.index();
    let last = index.time_of(index.frame_count() - 1).unwrap();
    let mut latencies = Vec::new();
    let mut decoder = std::mem::replace(d, Box::new(Parked));
    for round in 0..rounds {
        decoder.set_cancel(None);
        decoder.seek_exact(Time::ZERO).unwrap();
        let token = CancelToken::new();
        decoder.set_cancel(Some(token.clone()));
        let worker = thread::spawn(move || {
            let r = decoder.seek_exact(last);
            (r.map(|_| ()), Instant::now(), decoder)
        });
        thread::sleep(Wall::from_millis(15 + 13 * round));
        let cancelled_at = Instant::now();
        token.cancel();
        let (result, returned_at, back) = worker.join().unwrap();
        decoder = back;
        if result == Err(MediaError::Cancelled) {
            latencies.push(returned_at.duration_since(cancelled_at).as_secs_f64() * 1e3);
        }
    }
    decoder.set_cancel(None);
    let f = decoder.seek_exact(last).unwrap();
    assert_eq!(f.pts, last, "{name}: after cancels");
    *d = decoder;
    latencies
}

/// Cancel latencies of pending seeks on the 4K H.264 and the HEVC 10-bit clips, `rounds` each:
/// `(direct decoders, pooled leases)`. `None` without the corpus.
pub fn pending_4k_cancels(rounds: u64) -> Option<(Vec<f64>, Vec<f64>)> {
    let (mut direct, mut pooled) = (Vec::new(), Vec::new());
    for name in ["h264_4k30_longgop.mp4", "hevc_10bit_sdr.mp4"] {
        let path = corpus_file(name)?;
        let index = Arc::new(backend().index_media(&path).unwrap());
        let opts = DecodeOpts::default().with_index(Arc::clone(&index));
        let mut d = backend().open_decoder(&path, &opts).unwrap();
        direct.extend(cancel_latencies(name, &mut d, rounds));
        let pool = DecoderPool::new(Arc::new(backend()), &path).with_index(index);
        let lease = pool.checkout(&opts, None, &CancelToken::new()).unwrap();
        let mut d: Box<dyn VideoDecoder> = Box::new(lease);
        pooled.extend(cancel_latencies(name, &mut d, rounds));
    }
    Some((direct, pooled))
}

/// Prints and returns `(p50, p95, max)` of latencies in milliseconds.
pub fn show_latencies(label: &str, v: &[f64]) -> (f64, f64, f64) {
    let (p50, p95, max) = (
        percentile(v, 50.0),
        percentile(v, 95.0),
        percentile(v, 100.0),
    );
    eprintln!(
        "{label}: {} cancels, p50 {p50:.2} ms, p95 {p95:.2} ms, max {max:.2} ms",
        v.len()
    );
    (p50, p95, max)
}

/// A stand-in while the decoder under test is on another thread.
struct Parked;

impl VideoDecoder for Parked {
    fn next_frame(&mut self) -> Result<Option<VideoFrame>, MediaError> {
        Err(MediaError::Cancelled)
    }
    fn seek_exact(&mut self, _: Time) -> Result<VideoFrame, MediaError> {
        Err(MediaError::Cancelled)
    }
    fn index(&self) -> Arc<FrameIndex> {
        unreachable!("parked")
    }
    fn set_cancel(&mut self, _: Option<CancelToken>) {}
}

/// The corpus directory: `CUTWEAVE_CORPUS_DIR`, else `testdata/corpus` of this checkout, else
/// that of the main checkout when this is a git worktree (corpora are fetched once per clone).
pub fn corpus_dir() -> Option<PathBuf> {
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
    let main = gitdir.parent()?.parent()?.parent()?;
    let dir = main.join("testdata/corpus");
    dir.is_dir().then_some(dir)
}

/// A corpus file by name, or a skip message.
pub fn corpus_file(name: &str) -> Option<PathBuf> {
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

/// The FFmpeg tool `name` (`ffprobe`, `ffmpeg`): the bundle's, else the one on `PATH`; `None`
/// (with a skip message) when neither runs.
pub fn tool(name: &str) -> Option<PathBuf> {
    let exe = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    let in_bundle = std::env::var_os("FFMPEG_DIR")
        .map(|d| PathBuf::from(d).join("bin").join(&exe))
        .filter(|p| p.is_file());
    let candidate = in_bundle.unwrap_or_else(|| PathBuf::from(&exe));
    let ok = Command::new(&candidate)
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success());
    if ok {
        Some(candidate)
    } else {
        eprintln!("skipping: no {name} (the bundle's bin/ or PATH)");
        None
    }
}

/// Runs `tool` with `args`, returning stdout; panics with stderr on failure.
pub fn run(tool: &Path, args: &[&str]) -> String {
    let out = Command::new(tool)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("cannot run {}: {e}", tool.display()));
    assert!(
        out.status.success(),
        "{} {args:?} failed: {}",
        tool.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf-8 output")
}

/// A scratch directory for generated media and cached truth (in cargo's per-crate test tmpdir,
/// git-ignored with `target/`).
pub fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

/// Generates `name` into the scratch media directory with the `ffmpeg` CLI (lavfi sources, so
/// nothing is downloaded), unless it is already there. `None` without an `ffmpeg`.
pub fn generated(name: &str, args: &[&str]) -> Option<PathBuf> {
    let ffmpeg = tool("ffmpeg")?;
    let path = scratch("generated-media").join(name);
    if !path.is_file() {
        let tmp = path.with_extension(format!(
            "partial-{}.{}",
            std::process::id(),
            path.extension().and_then(|e| e.to_str()).unwrap_or("bin")
        ));
        let mut all: Vec<&str> = vec!["-v", "error", "-nostdin", "-y"];
        all.extend_from_slice(args);
        let tmp_s = tmp.to_str().expect("utf-8 path").to_owned();
        all.push(&tmp_s);
        run(&ffmpeg, &all);
        // Another test process may have won the race; either file is the same.
        let _ = std::fs::rename(&tmp, &path);
        let _ = std::fs::remove_file(&tmp);
    }
    Some(path)
}

/// Small clips generated for what the corpus lacks (never committed): odd sizes (VP9 long GOP
/// with hidden alt-ref frames, FFV1 intra; `testsrc2` itself rounds to even sizes, so the scale
/// filter makes them odd), open GOP (H.264 at 29.97 fps, HEVC 10-bit CRA/RASL in Matroska with
/// its pts cues and 1 ms time base) and MPEG-TS (no index: seeks bisect on dts).
pub const GENERATED_CLIPS: [&str; 5] = [
    "vp9_99x57.webm",
    "ffv1_101x55.mkv",
    "h264_opengop_2997.mp4",
    "hevc_opengop_10bit.mkv",
    "h264_gop25.ts",
];

/// One of [`GENERATED_CLIPS`], generated on first use.
pub fn generated_clip(name: &str) -> Option<PathBuf> {
    let args: &[&str] = match name {
        "vp9_99x57.webm" => &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=198x114:rate=25:duration=6",
            "-vf",
            "scale=99:57",
            "-c:v",
            "libvpx-vp9",
            "-g",
            "50",
            "-b:v",
            "200k",
            "-pix_fmt",
            "yuv420p",
        ],
        "ffv1_101x55.mkv" => &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=202x110:rate=24:duration=3",
            "-vf",
            "scale=101:55",
            "-c:v",
            "ffv1",
            "-pix_fmt",
            "yuv420p",
        ],
        "h264_opengop_2997.mp4" => &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x240:rate=30000/1001:duration=8",
            "-c:v",
            "libx264",
            "-g",
            "60",
            "-bf",
            "3",
            "-x264-params",
            "open-gop=1:scenecut=0",
            "-pix_fmt",
            "yuv420p",
        ],
        "hevc_opengop_10bit.mkv" => &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x240:rate=25:duration=8",
            "-c:v",
            "libx265",
            "-x265-params",
            "keyint=50:min-keyint=50:open-gop=1:scenecut=0:bframes=4:log-level=error",
            "-pix_fmt",
            "yuv420p10le",
        ],
        "h264_gop25.ts" => &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x240:rate=25:duration=8",
            "-c:v",
            "libx264",
            "-g",
            "25",
            "-bf",
            "2",
            "-pix_fmt",
            "yuv420p",
        ],
        _ => panic!("unknown generated clip {name}"),
    };
    generated(name, args)
}

/// `(pts, Y-plane CRC-32)` of every frame of video stream `stream` of `media`, in output order,
/// from `ffmpeg -vf extractplanes=y -f framehash -hash crc32` (frames in coded orientation, the
/// stream's own timestamps), cached in the scratch directory by name, size and modification
/// time.
pub fn y_crcs(media: &Path, stream: u32) -> Option<Vec<(i64, u32)>> {
    let ffmpeg = tool("ffmpeg")?;
    let meta = std::fs::metadata(media).expect("media metadata");
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    let name = media.file_name().unwrap().to_string_lossy().into_owned();
    let cache = scratch("truth").join(format!("{name}.{}.{mtime}.{stream}.ycrc", meta.len()));
    let text = if let Ok(s) = std::fs::read_to_string(&cache) {
        s
    } else {
        let map = format!("0:{stream}");
        let s = run(
            &ffmpeg,
            &[
                "-v",
                "error",
                "-nostdin",
                "-noautorotate",
                "-copyts",
                "-i",
                media.to_str().expect("utf-8 path"),
                "-map",
                &map,
                "-vf",
                "extractplanes=y",
                "-fps_mode",
                "passthrough",
                "-enc_time_base",
                "demux",
                "-c:v",
                "rawvideo",
                "-f",
                "framehash",
                "-hash",
                "crc32",
                "-",
            ],
        );
        let tmp = cache.with_extension(format!("tmp{}", std::process::id()));
        std::fs::write(&tmp, &s).expect("write truth cache");
        let _ = std::fs::rename(&tmp, &cache);
        s
    };
    Some(parse_framehash(&text))
}

/// framehash rows: `stream, dts, pts, duration, size, hash`.
fn parse_framehash(s: &str) -> Vec<(i64, u32)> {
    s.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| {
            let cols: Vec<&str> = l.split(',').map(str::trim).collect();
            assert!(cols.len() >= 6, "bad framehash line: {l}");
            let pts: i64 = cols[2].parse().expect("framehash pts");
            let crc = u32::from_str_radix(cols[5], 16).expect("framehash crc");
            (pts, crc)
        })
        .collect()
}

/// CRC-32 (IEEE, reflected, as zlib and FFmpeg's `framehash -hash crc32`), slicing by 8.
pub struct Crc32 {
    tables: Box<[[u32; 256]; 8]>,
}

impl Default for Crc32 {
    fn default() -> Self {
        let mut tables = Box::new([[0u32; 256]; 8]);
        for i in 0..256u32 {
            let mut c = i;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            tables[0][i as usize] = c;
        }
        for i in 0..256 {
            for t in 1..8 {
                let prev = tables[t - 1][i];
                tables[t][i] = (prev >> 8) ^ tables[0][(prev & 0xFF) as usize];
            }
        }
        Crc32 { tables }
    }
}

impl Crc32 {
    /// Continues a running CRC (`state` starts at `0xFFFF_FFFF`).
    pub fn update(&self, mut state: u32, data: &[u8]) -> u32 {
        let t = &self.tables;
        let (chunks, rest) = data.as_chunks::<8>();
        for c in chunks {
            let lo = state ^ u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
            let hi = u32::from_le_bytes([c[4], c[5], c[6], c[7]]);
            state = t[7][(lo & 0xFF) as usize]
                ^ t[6][((lo >> 8) & 0xFF) as usize]
                ^ t[5][((lo >> 16) & 0xFF) as usize]
                ^ t[4][(lo >> 24) as usize]
                ^ t[3][(hi & 0xFF) as usize]
                ^ t[2][((hi >> 8) & 0xFF) as usize]
                ^ t[1][((hi >> 16) & 0xFF) as usize]
                ^ t[0][(hi >> 24) as usize];
        }
        for &b in rest {
            state = t[0][((state ^ u32::from(b)) & 0xFF) as usize] ^ (state >> 8);
        }
        state
    }

    /// One-shot CRC of `data`.
    pub fn checksum(&self, data: &[u8]) -> u32 {
        self.update(0xFFFF_FFFF, data) ^ 0xFFFF_FFFF
    }

    /// CRC of plane 0's rows without stride padding (what `extractplanes=y` + rawvideo hash).
    pub fn y_plane(&self, frame: &VideoFrame) -> u32 {
        let (_, h, _) = frame.format.plane_layout(0, frame.width, frame.height);
        let mut s = 0xFFFF_FFFF;
        for y in 0..h {
            s = self.update(s, frame.row(0, y).expect("row"));
        }
        s ^ 0xFFFF_FFFF
    }
}

/// SplitMix64: seeded and reproducible across platforms (failing seeks can be replayed).
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` (`n > 0`).
    #[allow(clippy::cast_possible_truncation)]
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n.max(1) as u64) as usize
    }
}

/// A seed from a name (FNV-1a), so each file gets its own reproducible seek list.
pub fn seed(name: &str) -> u64 {
    name.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Nearest-rank percentile (`p` in 0..=100) of milliseconds; `NaN` when empty.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
pub fn percentile(samples: &[f64], p: f64) -> f64 {
    if samples.is_empty() {
        return f64::NAN;
    }
    let mut v = samples.to_vec();
    v.sort_by(f64::total_cmp);
    let rank = ((p / 100.0) * v.len() as f64).ceil() as usize;
    v[rank.clamp(1, v.len()) - 1]
}

#[test]
fn crc32_matches_the_standard_check_value() {
    let c = Crc32::default();
    assert_eq!(c.checksum(b"123456789"), 0xCBF4_3926);
    assert_eq!(c.checksum(b""), 0);
    let s = c.update(0xFFFF_FFFF, b"12345");
    assert_eq!(c.update(s, b"6789") ^ 0xFFFF_FFFF, 0xCBF4_3926);
    let long: Vec<u8> = (0..1000u32).map(|i| (i * 7 % 251) as u8).collect();
    let mut bytewise = 0xFFFF_FFFFu32;
    for &b in &long {
        bytewise = c.tables[0][((bytewise ^ u32::from(b)) & 0xFF) as usize] ^ (bytewise >> 8);
    }
    assert_eq!(c.checksum(&long), bytewise ^ 0xFFFF_FFFF);
}
