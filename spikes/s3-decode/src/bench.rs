// SPDX-License-Identifier: GPL-3.0-or-later
//! Timing: random-seek latency, scrub latency and sequential decode throughput.
//!
//! Latency is request → decoded frame in CPU memory (native pixel format, no conversion, no
//! upload): demux + decode only, the part S3 owns. On the hardware path (WP-0E.02) that includes
//! `av_hwframe_transfer_data` of the returned frame, so CPU and GPU decode compare like for like;
//! `s3 gpu` adds the upload to a wgpu texture. The OS file cache is warm (the index scan just read
//! the file); cold-disk seeks are not measured.

use std::path::Path;
use std::time::Instant;

use crate::error::Result;
use crate::index::FrameIndex;
use crate::monitor::{Phase, PhaseClock};
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
    pub fn of(ms: &[f64]) -> Self {
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
    /// Packets sent per seek beyond the target's own decode position (`decode_idx - start_idx +
    /// 1`): the decoder's output delay (B-frame reordering, frame threads, hw surfaces in flight).
    pub delay_packets: f64,
    /// Frames the decoder returned per random seek (fewer with `skip_frame = NONREF` honoured).
    pub frames_out_per_seek: f64,
    /// `av_hwframe_transfer_data` per seek (hw path; 0 for software).
    pub transfer_ms: f64,
    /// Scrub: a forward then backward sweep over ~120 positions with forward-continue.
    pub scrub: Dist,
    /// Single-frame stepping forward (arrow key), 60 steps.
    pub step: Dist,
    /// Sequential decode throughput, frames per second (every frame transferred on the hw path).
    pub decode_fps: f64,
    /// Hardware setup and fallback summary (empty for software configs).
    pub hw: String,
    /// Wall/CPU windows: `seek`, `scrub`, `step`, `sequential`.
    pub phases: Vec<Phase>,
}

impl Row {
    /// The phase called `name`.
    pub fn phase(&self, name: &str) -> Option<&Phase> {
        self.phases.iter().find(|p| p.name == name)
    }
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
    let mut phases = vec![];

    // Warm-up: decoder threads, first allocations, hw surface pool.
    let mut rng = Rng::new(seed ^ 0xA5A5);
    for _ in 0..3 {
        src.seek_frame(rng.below(frames))?;
    }

    // Random cold seeks.
    let mut rng = Rng::new(seed);
    let mut ms = Vec::with_capacity(seeks);
    src.stats = Default::default();
    let mut needed = 0usize;
    let clock = PhaseClock::start("seek");
    for _ in 0..seeks {
        let pi = rng.below(frames);
        let f = src.index.frames[pi];
        needed += f.decode_idx.saturating_sub(f.start_idx) + 1;
        let t = Instant::now();
        src.seek_frame(pi)?;
        ms.push(t.elapsed().as_secs_f64() * 1e3);
    }
    phases.push(clock.stop());
    let seek = Dist::of(&ms);
    let n = seeks.max(1) as f64;
    let packets_per_seek = src.stats.packets_sent as f64 / n;
    let delay_packets = (src.stats.packets_sent as f64 - needed as f64) / n;
    let frames_out_per_seek = src.stats.frames_out as f64 / n;
    let transfer_ms = src.stats.transfer_ns as f64 / 1e6 / n;

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
    let clock = PhaseClock::start("scrub");
    for pi in targets {
        let t = Instant::now();
        src.seek_frame(pi)?;
        ms.push(t.elapsed().as_secs_f64() * 1e3);
    }
    phases.push(clock.stop());
    let scrub = Dist::of(&ms);

    // Frame stepping from a mid-GOP position.
    let start = frames / 3;
    src.seek_frame(start)?;
    let mut ms = Vec::new();
    let clock = PhaseClock::start("step");
    for pi in start + 1..(start + 61).min(frames) {
        let t = Instant::now();
        src.seek_frame(pi)?;
        ms.push(t.elapsed().as_secs_f64() * 1e3);
    }
    phases.push(clock.stop());
    let stepd = Dist::of(&ms);

    // Sequential decode throughput.
    src.rewind()?;
    let clock = PhaseClock::start("sequential");
    let t = Instant::now();
    let mut n = 0usize;
    while src.next_frame()? {
        n += 1;
    }
    let decode_fps = n as f64 / t.elapsed().as_secs_f64();
    phases.push(clock.stop());

    Ok(Row {
        file: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        config: if label.is_empty() {
            opts.label()
        } else {
            label.to_string()
        },
        width,
        height,
        frames,
        gop_max,
        open_ms,
        index_ms,
        seek,
        packets_per_seek,
        delay_packets,
        frames_out_per_seek,
        transfer_ms,
        scrub,
        step: stepd,
        decode_fps,
        hw: if opts.hw.is_some() {
            src.hw_summary()
        } else {
            String::new()
        },
        phases,
    })
}
