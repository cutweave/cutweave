// SPDX-License-Identifier: GPL-3.0-or-later
//! Timing: random-seek latency, scrub latency and sequential decode throughput.
//!
//! Latency is request → decoded frame in CPU memory (native pixel format, no conversion, no
//! upload): demux + decode only, the part S3 owns. The OS file cache is warm (the index scan
//! just read the file); cold-disk seeks are not measured.

use std::path::Path;
use std::time::Instant;

use crate::error::Result;
use crate::index::FrameIndex;
use crate::source::{Options, VideoSource};
use crate::util::{Rng, mean, percentile};

/// Latency distribution in milliseconds.
#[derive(Clone, Copy, Debug, Default)]
pub struct Dist {
    pub n: usize,
    pub p50: f64,
    pub p95: f64,
    pub max: f64,
    pub mean: f64,
}

impl Dist {
    fn of(ms: &[f64]) -> Self {
        Dist {
            n: ms.len(),
            p50: percentile(ms, 50.0).unwrap_or(0.0),
            p95: percentile(ms, 95.0).unwrap_or(0.0),
            max: ms.iter().copied().fold(0.0, f64::max),
            mean: mean(ms),
        }
    }
}

/// One benchmark row (file × configuration).
#[derive(Clone, Debug, Default)]
pub struct Row {
    pub file: String,
    pub config: String,
    pub width: u32,
    pub height: u32,
    pub frames: usize,
    pub gop_max: usize,
    pub open_ms: f64,
    pub index_ms: f64,
    /// Cold random seeks (keyframe seek + decode-forward every time).
    pub seek: Dist,
    /// Packets sent to the decoder per random seek (decode-forward distance).
    pub packets_per_seek: f64,
    /// Scrub: a forward then backward sweep over ~120 positions with forward-continue.
    pub scrub: Dist,
    /// Single-frame stepping forward (arrow key), 60 steps.
    pub step: Dist,
    /// Sequential decode throughput, frames per second.
    pub decode_fps: f64,
}

/// Runs the benchmark for one file and configuration.
pub fn run(path: &Path, opts: Options, seeks: usize, seed: u64, label: &str) -> Result<Row> {
    let t0 = Instant::now();
    let index = FrameIndex::scan(path)?;
    let index_ms = index.scan_time.as_secs_f64() * 1e3;
    let frames = index.frames.len();
    let gop_max = index.gop_stats().2;
    let mut src = VideoSource::open_with_index(
        path,
        index,
        Options {
            forward_continue: false,
            ..opts
        },
    )?;
    let open_ms = t0.elapsed().as_secs_f64() * 1e3;
    let (width, height) = (src.decoder().width(), src.decoder().height());

    // Warm-up: decoder threads, first allocations.
    let mut rng = Rng::new(seed ^ 0xA5A5);
    for _ in 0..3 {
        src.seek_frame(rng.below(frames))?;
    }

    // Random cold seeks.
    let mut rng = Rng::new(seed);
    let mut ms = Vec::with_capacity(seeks);
    src.stats = Default::default();
    for _ in 0..seeks {
        let pi = rng.below(frames);
        let t = Instant::now();
        src.seek_frame(pi)?;
        ms.push(t.elapsed().as_secs_f64() * 1e3);
    }
    let seek = Dist::of(&ms);
    let packets_per_seek = src.stats.packets_sent as f64 / seeks.max(1) as f64;

    // Scrub sweep, forward then backward, with forward-continue (what a playhead drag does).
    src.set_options(opts.skip_nonref, true);
    let step = (frames / 120).max(1);
    let fwd: Vec<usize> = (0..frames).step_by(step).collect();
    let targets: Vec<usize> = fwd
        .iter()
        .copied()
        .chain(fwd.iter().rev().copied())
        .collect();
    let mut ms = Vec::with_capacity(targets.len());
    for pi in targets {
        let t = Instant::now();
        src.seek_frame(pi)?;
        ms.push(t.elapsed().as_secs_f64() * 1e3);
    }
    let scrub = Dist::of(&ms);

    // Frame stepping from a mid-GOP position.
    let start = frames / 3;
    src.seek_frame(start)?;
    let mut ms = Vec::new();
    for pi in start + 1..(start + 61).min(frames) {
        let t = Instant::now();
        src.seek_frame(pi)?;
        ms.push(t.elapsed().as_secs_f64() * 1e3);
    }
    let stepd = Dist::of(&ms);

    // Sequential decode throughput.
    src.rewind()?;
    let t = Instant::now();
    let mut n = 0usize;
    while src.next_frame()? {
        n += 1;
    }
    let decode_fps = n as f64 / t.elapsed().as_secs_f64();

    Ok(Row {
        file: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        config: label.to_string(),
        width,
        height,
        frames,
        gop_max,
        open_ms,
        index_ms,
        seek,
        packets_per_seek,
        scrub,
        step: stepd,
        decode_fps,
    })
}
