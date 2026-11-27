// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2D.01b on real H.264 through the preview's frame source (feature `ffmpeg`; needs the
//! bundle's `ffmpeg` CLI and, for the timing, the corpus: `cargo xtask fetch-corpus`).
//!
//! - **Frame-exact:** a clip whose every frame carries its own number as eight black or white
//!   bars (an OCR-free counter that survives H.264), 640×360, 30 fps, 240 frames, keyframes every
//!   120 frames with B-frames, is played backwards through the preview (playback frames, so
//!   read back in windows: of 40 frames, then of whole GOPs). Every frame shows the right
//!   number and is bit-identical to the uncached, software-decoded frame `render.frame` uses
//!   (the export's decode).
//! - **Timing** (arch §19, the WP-2D.01 criterion): reverse playback of the 1080p60 corpus clip
//!   (H.264, keyframes every 120 frames, 480 frames) costs the frame source at most 150 ms per
//!   frame at p95 once its proxy is ready (asserted; every frame read backwards comes from the
//!   proxy). Without a proxy the read-back serves it; the per-frame numbers, with the cost of
//!   a seek per frame for comparison, are printed (recorded in docs/01-architecture.md §7).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Instant;

use cutweave_cache::{CacheDb, FrameCache, FrameIndex};
use cutweave_commands::{DocActor, DocConfig, Registry};
use cutweave_media::{DecodeMode, FfmpegBackend, HashMode, HwMode, MediaBackend};
use cutweave_model::{MediaAsset, MediaId, MediaKind, Project};
use cutweave_render::{FrameRequest, FrameSource, VideoFrame};

use super::frames::{DecodePolicy, MediaFrames, PreviewFrames};
use super::media::CurrentDoc;
use super::proxy::FullProxies;
use crate::subsystems::EngineLink;

const W: u32 = 640;
const H: u32 = 360;
/// Width of one bar of the counter.
const BAR: u32 = 80;
const FRAMES: usize = 240;

/// A scratch folder for this test process.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cutweave-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The bundle's `ffmpeg` (else the one on `PATH`), if it runs.
fn ffmpeg() -> Option<PathBuf> {
    let exe = format!("ffmpeg{}", std::env::consts::EXE_SUFFIX);
    let tool = std::env::var_os("FFMPEG_DIR")
        .map(|d| PathBuf::from(d).join("bin").join(&exe))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from(&exe));
    let runs = Command::new(&tool)
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success());
    if runs {
        Some(tool)
    } else {
        eprintln!("skipping: no ffmpeg CLI (source scripts/ffmpeg/env.sh)");
        None
    }
}

/// The test-media corpus file `name`: `CUTWEAVE_CORPUS_DIR`, else `testdata/corpus` of this
/// checkout, else of the main checkout when this is a git worktree.
fn corpus_file(name: &str) -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = std::env::var_os("CUTWEAVE_CORPUS_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            let local = root.join("testdata/corpus");
            if local.is_dir() {
                return Some(local);
            }
            let gitdir = std::fs::read_to_string(root.join(".git")).ok()?;
            let gitdir = PathBuf::from(gitdir.strip_prefix("gitdir:")?.trim());
            Some(gitdir.parent()?.parent()?.parent()?.join("testdata/corpus"))
        });
    let path = dir.map(|d| d.join(name)).filter(|p| p.is_file());
    if path.is_none() {
        eprintln!("skipping: {name} is not in the corpus (cargo xtask fetch-corpus)");
    }
    path
}

/// The counter clip: frame `N` has bar `b` white when bit `b` of `N` is set.
fn counter_clip(tool: &Path, out: &Path) {
    let source = format!("color=c=black:s={W}x{H}:r=30:d=8");
    let bars =
        format!("geq=lum='if(mod(floor(N/pow(2\\,floor(X/{BAR})))\\,2)\\,235\\,16)':cb=128:cr=128");
    let status = Command::new(tool)
        .args([
            "-v", "error", "-nostdin", "-y", "-f", "lavfi", "-i", &source,
        ])
        .args(["-vf", &bars, "-c:v", "libx264", "-g", "120", "-bf", "2"])
        .args(["-x264-params", "scenecut=0", "-pix_fmt", "yuv420p"])
        .arg(out)
        .status()
        .unwrap();
    assert!(status.success(), "ffmpeg made the counter clip");
}

/// The number the bars of a counter picture show (luma of the first plane).
fn counter(frame: &VideoFrame) -> u32 {
    let y = &frame.planes[0];
    let row = (H / 2) as usize * y.stride as usize;
    (0..8)
        .filter(|b| y.data[row + (BAR / 2 + BAR * b) as usize] > 128)
        .map(|b| 1 << b)
        .sum()
}

/// One media item, imported: the document, its actor, and what the frame sources need.
struct Fixture {
    actor: DocActor,
    current: CurrentDoc,
    id: MediaId,
    asset: MediaAsset,
    index: Arc<FrameIndex>,
    backend: Arc<dyn MediaBackend>,
}

impl Fixture {
    fn new(path: &Path) -> Fixture {
        let backend: Arc<dyn MediaBackend> = Arc::new(FfmpegBackend::new().unwrap());
        let probe = backend.probe(path).unwrap();
        let index = Arc::new(backend.index_media(path).unwrap());
        let id = MediaId::from_u128(0x2d01b);
        let mut asset = MediaAsset::file(id, MediaKind::Video, path.to_string_lossy(), probe);
        let hash = cutweave_media::content_hash(path, HashMode::Fast).unwrap();
        asset.content_hash = Some(hash.to_string());
        let mut project = Project::new("reverse");
        project.media.insert(id, asset.clone());
        let actor = DocActor::spawn(DocConfig::new(Arc::new(Registry::new()), project)).unwrap();
        let current = CurrentDoc::default();
        current.set(Some(actor.handle().clone()));
        Fixture {
            actor,
            current,
            id,
            asset,
            index,
            backend,
        }
    }

    /// Frame sources over a fresh frame cache, decoding `hw` in the preview.
    fn frames(&self, hw: HwMode, proxies: Option<Arc<FullProxies>>) -> Arc<MediaFrames> {
        let policy = Arc::new(DecodePolicy::default());
        policy.set(hw);
        Arc::new(
            MediaFrames::new(
                Some(Arc::clone(&self.backend)),
                self.current.clone(),
                Arc::new(FrameCache::new(512 << 20)),
                None,
                policy,
            )
            .with_proxies(proxies),
        )
    }

    /// The request for frame `n` of the media as the preview asks for it while it uses proxies
    /// (the default), drawn at the original's full size: a proxy is too small for it, so only
    /// playback read backwards takes one (WP-2D.01b, WP-2F.02).
    fn request(&self, n: usize) -> FrameRequest {
        let video = &self.asset.probe.streams[0];
        let (width, height) = (video.width.unwrap(), video.height.unwrap());
        FrameRequest {
            media: self.id,
            stream: Some(0),
            source_time: self.index.time_of(n).unwrap(),
            source_size: Some((width, height)),
            rotation: 0,
            color: self.asset.probe.color.clone(),
            still: false,
            proxy: true,
            drawn_long_edge: Some(width.max(height)),
        }
    }

    /// Plays the media backwards from its last frame through `preview` as playback frames;
    /// calls `each` with every frame and the time the frame source took for it (ms).
    fn play_backwards(
        &self,
        preview: &PreviewFrames,
        frames: impl Iterator<Item = usize>,
        mut each: impl FnMut(usize, &Arc<VideoFrame>, f64),
    ) {
        let snapshot = self.actor.handle().snapshot();
        let cancel = cutweave_playback::transport::CancelToken::never();
        for n in frames {
            let request = self.request(n);
            let started = Instant::now();
            let frame = preview
                .request(&snapshot, &cancel, DecodeMode::Sequential)
                .frame(&request)
                .unwrap();
            each(n, &frame, started.elapsed().as_secs_f64() * 1e3);
        }
    }
}

#[test]
fn reversed_preview_frames_show_the_exact_frames_an_uncached_decode_shows() {
    let Some(tool) = ffmpeg() else { return };
    let dir = scratch("reverse-counter");
    let clip = dir.join("counter.mp4");
    counter_clip(&tool, &clip);
    let f = Fixture::new(&clip);
    assert_eq!(f.index.frame_count(), FRAMES);
    // Software in the preview too, so its pixels can equal the software agent frames.
    let frames = f.frames(HwMode::Off, None);
    let frame_bytes = (W * H * 3 / 2) as usize;
    for (budget, windows) in [(40 * frame_bytes, 6), (super::frames::READBACK_BYTES, 2)] {
        let preview = PreviewFrames::new(Arc::clone(&frames)).with_readback_budget(budget);
        f.play_backwards(&preview, (0..FRAMES).rev(), |n, frame, _| {
            assert_eq!(counter(frame), u32::try_from(n).unwrap(), "counter at {n}");
            let uncached = frames.frame(&f.request(n)).unwrap();
            assert_eq!(
                (frame.width, frame.height, frame.format),
                (uncached.width, uncached.height, uncached.format)
            );
            assert!(
                frame.planes == uncached.planes,
                "frame {n} read back differs from the uncached decode"
            );
        });
        // The first frame is decoded the ordinary way (nothing was read before it); each
        // window after is one read-back: 40 frames inside 120-frame GOPs (119 frames left
        // in the second GOP: 40 + 40 + 39, then 40 + 40 + 40), or whole GOPs.
        assert_eq!(preview.readback_fills(f.id), windows, "budget {budget}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// p50, p95 and max of `ms`, printed with `what`.
fn percentiles(what: &str, mut ms: Vec<f64>) -> (f64, f64, f64) {
    ms.sort_by(f64::total_cmp);
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let at = |q: f64| ms[((ms.len() - 1) as f64 * q).round() as usize];
    let (p50, p95, max) = (at(0.5), at(0.95), at(1.0));
    eprintln!(
        "{what}: {} frames, p50 {p50:.1} ms, p95 {p95:.1} ms, max {max:.1} ms",
        ms.len()
    );
    (p50, p95, max)
}

#[test]
fn reverse_p95_of_a_1080p_long_gop_clip_is_under_150_ms_with_its_proxy() {
    let Some(clip) = corpus_file("h264_1080p60.mp4") else {
        return;
    };
    let f = Fixture::new(&clip);
    let frames_total = f.index.frame_count();
    let dir = scratch("reverse-timing");
    let db = Arc::new(CacheDb::open_in_memory().unwrap());
    let proxies = Arc::new(FullProxies::new(
        &f.backend,
        Arc::clone(&db),
        dir.join("proxies"),
        f.current.clone(),
        EngineLink::detached(),
    ));
    let made = Instant::now();
    let record = proxies.generate_now(&f.asset);
    eprintln!(
        "proxy {}x{} made in {:.1} s",
        record.size.0,
        record.size.1,
        made.elapsed().as_secs_f64()
    );

    // With the proxy: every frame read backwards comes from it.
    let preview = PreviewFrames::new(f.frames(HwMode::Auto, Some(Arc::clone(&proxies))));
    let mut times = Vec::new();
    f.play_backwards(&preview, (0..frames_total).rev(), |n, frame, ms| {
        if n + 1 < frames_total {
            assert_eq!(
                (frame.width, frame.height),
                record.size,
                "frame {n} from the proxy"
            );
        }
        times.push(ms);
    });
    let (_, p95, _) = percentiles("1080p60 reverse playback with its proxy", times);
    assert!(p95 < 150.0, "reverse playback p95 {p95:.1} ms with a proxy");

    // Without: the read-back of the original, and a seek per frame for comparison.
    let preview = PreviewFrames::new(f.frames(HwMode::Auto, None));
    let mut times = Vec::new();
    f.play_backwards(&preview, (0..frames_total).rev(), |_, frame, ms| {
        assert_eq!((frame.width, frame.height), (1920, 1080));
        times.push(ms);
    });
    percentiles(
        &format!(
            "1080p60 reverse playback without a proxy, read back in {} windows",
            preview.readback_fills(f.id)
        ),
        times,
    );
    let one_frame = PreviewFrames::new(f.frames(HwMode::Auto, None)).with_readback_budget(1);
    let mut times = Vec::new();
    f.play_backwards(
        &one_frame,
        (frames_total - 121..frames_total).rev(),
        |_, _, ms| times.push(ms),
    );
    times.remove(0);
    percentiles(
        "1080p60 reverse playback without a proxy, a seek per frame",
        times,
    );
    drop(proxies);
    let _ = std::fs::remove_dir_all(&dir);
}
