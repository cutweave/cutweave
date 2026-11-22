//! Headless export: demo timeline → PNG frame sequence.
//!
//! H.264/MP4 via FFmpeg is **pending**: FFmpeg is not available on the build machine (the pinned
//! bundle arrives with WP-0A.06). The intended path pipes the same frames, converted to BT.709
//! limited-range Y'CbCr, into the encoder; see `README.md`.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::Result;
use crate::gpu::Renderer;
use crate::image::{cicp_for, write_png};
use crate::source::SourceProvider;
use crate::timeline::demo_scene;

/// Export statistics.
#[derive(Clone, Debug, Default)]
pub struct ExportStats {
    pub frames: usize,
    pub seconds: f64,
    /// Mean `render` wall time (encode, uploads, submit, wait, readback) per frame.
    pub render_ms_mean: f64,
    /// GPU time per frame (timestamp queries), when available.
    pub gpu_ms: Vec<f64>,
}

/// Path of frame `f` inside `dir`.
#[must_use]
pub fn frame_path(dir: &Path, f: i64) -> PathBuf {
    dir.join(format!("frame_{f:05}.png"))
}

/// Renders `frames` of the demo timeline and writes them as PNGs (with `cICP` tags).
pub fn export_png_sequence(
    renderer: &mut Renderer,
    provider: &dyn SourceProvider,
    size: (u32, u32),
    frames: Range<i64>,
    dir: &Path,
) -> Result<ExportStats> {
    std::fs::create_dir_all(dir)?;
    let t0 = Instant::now();
    let mut stats = ExportStats::default();
    let mut render_ms = 0.0;
    for f in frames {
        let scene = demo_scene(size.0, size.1, f);
        let frame = renderer.render_scene(&scene, provider)?;
        render_ms += frame.stats.cpu_ms;
        if let Some(ns) = frame.stats.gpu_ns {
            stats.gpu_ms.push(ns as f64 / 1e6);
        }
        write_png(
            &frame_path(dir, f),
            &frame.image,
            Some(cicp_for(&scene.output)),
        )?;
        stats.frames += 1;
    }
    stats.seconds = t0.elapsed().as_secs_f64();
    stats.render_ms_mean = render_ms / stats.frames.max(1) as f64;
    Ok(stats)
}
