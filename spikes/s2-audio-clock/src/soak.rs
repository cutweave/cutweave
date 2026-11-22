// SPDX-License-Identifier: GPL-3.0-or-later
//! Periodic health logger (CSV + console) for normal and `--soak` runs.
//!
//! One row per interval: audio clock vs wall clock drift, video frame vs audio clock and vs wall
//! clock, callback sizes/timing/load, reported latency, timestamp discontinuities, track underruns,
//! cpal xruns, RT allocations, ring headroom, worker load and loudness. Rows are flushed as they are
//! written so an interrupted soak still leaves a usable file.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::clock::AudioClock;
use crate::engine::CallbackStats;
use crate::meter::LoudnessStats;
use crate::mixer::MixerStats;
use crate::track::WorkerStats;
use crate::util::{NANOS, lin_to_db};
use crate::video::VideoStats;

/// CSV header (one row per log interval).
pub const CSV_HEADER: &str = "wall_s,audio_clock_s,frames_submitted,drift_audio_wall_ms,drift_jitter_ms,\
drift_audio_wall_frames,video_frame,video_expected_frame,video_minus_audio_frames,\
video_minus_wall_frames,video_presented,video_skipped,callbacks,cb_frames_min,cb_frames_max,\
cb_dur_us_mean,cb_dur_us_max,cb_gap_us_max,cb_load_max_pct,latency_ms,latency_ms_min,\
latency_ms_max,ts_err_us_max,device_glitches,late_callbacks,track_underrun_events,\
track_underrun_frames,cpal_xruns,cpal_errors,rt_allocs,ring_min_ms,worker_load_pct,\
worker_load_max_pct,lufs_m,lufs_s,lufs_i,true_peak_dbtp,limiter_gr_db_max,meter_dropped,\
present_us_max";

/// Everything the logger reads.
#[derive(Debug)]
pub struct LoggerCtx {
    /// Master clock.
    pub clock: Arc<AudioClock>,
    /// Callback stats.
    pub cb: Arc<CallbackStats>,
    /// Mixer stats.
    pub mix: Arc<MixerStats>,
    /// Loudness.
    pub loud: Arc<LoudnessStats>,
    /// Presenter stats.
    pub video: Arc<VideoStats>,
    /// Worker stats.
    pub workers: Vec<Arc<WorkerStats>>,
    /// Video frame rate.
    pub fps: u32,
    /// Deliberate video shift (`--av-offset-ms`), so video-vs-audio compares like with like.
    pub av_offset_ms: f64,
    /// Row interval.
    pub interval: Duration,
    /// CSV output path.
    pub csv: PathBuf,
}

/// Aggregates over the whole run, returned when the logger stops.
#[derive(Clone, Debug, Default)]
pub struct RunTotals {
    /// Rows written.
    pub rows: u64,
    /// Seconds since frame 0 was audible, at the last row.
    pub wall_s: f64,
    /// Drift at the last row (ms).
    pub drift_ms_end: f64,
    /// Largest |drift| seen (ms).
    pub drift_ms_max_abs: f64,
    /// Largest |video frame − audio-clock frame| seen.
    pub video_minus_audio_max_abs: i64,
    /// Worker load: mean over rows (%).
    pub worker_load_mean_pct: f64,
    /// Worker load: max single-worker row (%).
    pub worker_load_max_pct: f64,
    /// Smallest ring headroom seen (ms).
    pub ring_min_ms: f64,
    /// Largest limiter gain reduction (dB, positive).
    pub limiter_gr_db_max: f64,
    /// Mean reported latency across rows (ms).
    pub latency_ms_mean: f64,
    /// Largest within-row drift spread (anchor jitter, ms).
    pub drift_jitter_ms_max: f64,
    /// Least-squares drift slope over all rows with clock steps removed (ppm; + = audio clock faster
    /// than wall clock).
    pub drift_ppm_fit: f64,
    /// Fitted drift at the end of the run (ms, steps removed).
    pub drift_ms_fit_end: f64,
    /// Clock steps (row-to-row drift change > 1 ms).
    pub drift_steps: u64,
    /// Sum of all clock steps (ms).
    pub drift_step_ms_total: f64,
}

/// Row-to-row drift change treated as a clock step rather than drift (ms).
pub const STEP_MS: f64 = 1.0;

/// Running least-squares line fit `y = a + b x`.
#[derive(Debug, Default)]
pub struct LinFit {
    n: f64,
    sx: f64,
    sy: f64,
    sxx: f64,
    sxy: f64,
}

impl LinFit {
    /// Adds a point.
    pub fn add(&mut self, x: f64, y: f64) {
        self.n += 1.0;
        self.sx += x;
        self.sy += y;
        self.sxx += x * x;
        self.sxy += x * y;
    }

    /// `(slope, intercept)` once two distinct x values were added.
    pub fn line(&self) -> Option<(f64, f64)> {
        let den = self.n * self.sxx - self.sx * self.sx;
        if self.n < 2.0 || den.abs() < 1e-12 {
            return None;
        }
        let b = (self.n * self.sxy - self.sx * self.sy) / den;
        Some((b, (self.sy - b * self.sx) / self.n))
    }
}

fn swap_u64(a: &AtomicU64, v: u64) -> u64 {
    a.swap(v, Ordering::Relaxed)
}

/// Spawns the logger thread.
pub fn spawn_logger(
    ctx: LoggerCtx,
    stop: Arc<AtomicBool>,
) -> JoinHandle<Result<RunTotals, String>> {
    std::thread::Builder::new()
        .name("s2-logger".into())
        .spawn(move || run(&ctx, &stop))
        .expect("spawn logger thread")
}

#[allow(clippy::too_many_lines)]
fn run(ctx: &LoggerCtx, stop: &AtomicBool) -> Result<RunTotals, String> {
    let file = File::create(&ctx.csv).map_err(|e| format!("create {}: {e}", ctx.csv.display()))?;
    let mut w = BufWriter::new(file);
    writeln!(w, "{CSV_HEADER}").map_err(|e| e.to_string())?;
    let rate = f64::from(ctx.clock.sample_rate());
    let fps = f64::from(ctx.fps);
    let mut totals = RunTotals {
        ring_min_ms: f64::INFINITY,
        ..RunTotals::default()
    };
    let mut latency_sum = 0.0;
    let mut load_sum = 0.0;
    let mut prev_busy: Vec<u64> = ctx.workers.iter().map(|_| 0).collect();
    let mut prev_t = Instant::now();
    let mut next = Instant::now() + ctx.interval;
    let mut fit = LinFit::default();
    let mut prev_drift: Option<f64> = None;
    let mut step_sum = 0.0;
    loop {
        // Sleep in short steps (stop requests are honoured quickly) and sample the drift at each
        // step: a row reports the mean of ~50 anchors, which removes the per-callback jitter.
        let (mut d_sum, mut d_n, mut d_min, mut d_max) =
            (0.0, 0u32, f64::INFINITY, f64::NEG_INFINITY);
        while Instant::now() < next {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20).min(next - Instant::now()));
            if let Some(d) = ctx.clock.drift_ns() {
                let d = d / 1e6;
                d_sum += d;
                d_n += 1;
                d_min = d_min.min(d);
                d_max = d_max.max(d);
            }
        }
        let stopping = stop.load(Ordering::Relaxed);
        next += ctx.interval;
        let now_t = Instant::now();
        let dt_ns = now_t.duration_since(prev_t).as_nanos() as f64;
        prev_t = now_t;

        let clock = &ctx.clock;
        let now = clock.now_ns();
        let Some(base) = clock.base_wall_ns() else {
            if stopping {
                break;
            }
            continue; // not playing for 0.5 s yet
        };
        let wall_s = (now as f64 - base as f64) / NANOS;
        let audio_s = clock.position_frames_at(now).unwrap_or(0.0) / rate;
        let drift_ms = if d_n > 0 {
            d_sum / f64::from(d_n)
        } else {
            clock.drift_ns().unwrap_or(0.0) / 1e6
        };
        let drift_jitter_ms = if d_n > 1 { d_max - d_min } else { 0.0 };
        // A row-to-row change > 1 ms is a clock step (device/engine discontinuity), not drift:
        // normal drift moves ~0.01 ms/s. Steps are counted and removed before the slope fit.
        if let Some(prev) = prev_drift
            && (drift_ms - prev).abs() > STEP_MS
        {
            totals.drift_steps += 1;
            step_sum += drift_ms - prev;
            totals.drift_step_ms_total = step_sum;
        }
        prev_drift = Some(drift_ms);
        fit.add(wall_s, drift_ms - step_sum);
        let vstats = &ctx.video;
        let vframe = vstats.last_index.load(Ordering::Relaxed);
        let expected = ((audio_s + ctx.av_offset_ms / 1e3) * fps + 1e-6).floor() as i64;
        let wall_frame = (wall_s * fps).floor() as i64;

        let cb = &ctx.cb;
        let frames_min = swap_u64(&cb.frames_min, u64::MAX);
        let frames_max = swap_u64(&cb.frames_max, 0);
        let dur_sum = swap_u64(&cb.dur_ns_sum, 0);
        let dur_n = swap_u64(&cb.dur_count, 0).max(1);
        let dur_max = swap_u64(&cb.dur_ns_max, 0);
        let gap_max = swap_u64(&cb.gap_ns_max, 0);
        let load_max = swap_u64(&cb.load_permille_max, 0);
        let lat_last = cb.latency_ns_last.load(Ordering::Relaxed) as f64 / 1e6;
        let lat_min = swap_u64(&cb.latency_ns_min, u64::MAX);
        let lat_max = swap_u64(&cb.latency_ns_max, 0);
        let ts_err = swap_u64(&cb.ts_err_ns_max, 0);
        let ring_min = swap_u64(&ctx.mix.ring_min_frames, u64::MAX);
        let ring_min_ms = if ring_min == u64::MAX {
            f64::NAN
        } else {
            ring_min as f64 / rate * 1000.0
        };
        let min_gain = f32::from_bits(
            ctx.mix
                .limiter_min_gain
                .swap(1.0f32.to_bits(), Ordering::Relaxed),
        );
        let gr_db = -f64::from(lin_to_db(min_gain.max(1e-9)));
        let present_max = swap_u64(&vstats.present_ns_max, 0);

        let mut load_total = 0.0;
        let mut load_peak = 0.0f64;
        for (ws, prev) in ctx.workers.iter().zip(prev_busy.iter_mut()) {
            let busy = ws.busy_ns.load(Ordering::Relaxed);
            let l = (busy - *prev) as f64 / dt_ns.max(1.0) * 100.0;
            *prev = busy;
            load_total += l;
            load_peak = load_peak.max(l);
        }
        let load_mean = load_total / ctx.workers.len().max(1) as f64;

        let u = |v: u64| if v == u64::MAX { f64::NAN } else { v as f64 };
        let row = format!(
            "{wall_s:.3},{audio_s:.6},{},{drift_ms:.4},{drift_jitter_ms:.4},{:.5},{vframe},{expected},{},{},{},{},{},{},{},{:.1},{:.1},{:.1},{:.1},{lat_last:.3},{:.3},{:.3},{:.1},{},{},{},{},{},{},{},{ring_min_ms:.1},{load_mean:.2},{load_peak:.2},{:.2},{:.2},{:.2},{:.2},{gr_db:.2},{},{:.1}",
            clock.frames_submitted(),
            drift_ms * fps / 1000.0,
            vframe - expected,
            vframe - wall_frame,
            vstats.presented.load(Ordering::Relaxed),
            vstats.skipped.load(Ordering::Relaxed),
            cb.callbacks.load(Ordering::Relaxed),
            u(frames_min),
            frames_max,
            dur_sum as f64 / dur_n as f64 / 1e3,
            dur_max as f64 / 1e3,
            gap_max as f64 / 1e3,
            load_max as f64 / 10.0,
            u(lat_min) / 1e6,
            lat_max as f64 / 1e6,
            ts_err as f64 / 1e3,
            cb.glitches.load(Ordering::Relaxed),
            cb.late_callbacks.load(Ordering::Relaxed),
            ctx.mix.underrun_events.load(Ordering::Relaxed),
            ctx.mix.underrun_frames.load(Ordering::Relaxed),
            cb.xruns.load(Ordering::Relaxed),
            cb.errors.load(Ordering::Relaxed),
            cb.rt_allocs.load(Ordering::Relaxed),
            ctx.loud.momentary.load(),
            ctx.loud.shortterm.load(),
            ctx.loud.integrated.load(),
            ctx.loud.true_peak_dbtp.load(),
            ctx.mix.meter_dropped.load(Ordering::Relaxed),
            present_max as f64 / 1e3,
        );
        writeln!(w, "{row}").map_err(|e| e.to_string())?;
        w.flush().map_err(|e| e.to_string())?;

        println!(
            "[{wall_s:8.1}s] clk {audio_s:9.3}s drift {drift_ms:+8.3}ms | cb {}f lat {lat_last:5.1}ms load {:4.1}% | underruns {} glitches {} xruns {} | LUFS M {:6.1} I {:6.1} | video f{vframe} ({:+}) skipped {}",
            frames_max,
            load_max as f64 / 10.0,
            ctx.mix.underrun_events.load(Ordering::Relaxed),
            cb.glitches.load(Ordering::Relaxed),
            cb.xruns.load(Ordering::Relaxed),
            ctx.loud.momentary.load(),
            ctx.loud.integrated.load(),
            vframe - expected,
            vstats.skipped.load(Ordering::Relaxed),
        );

        totals.rows += 1;
        totals.wall_s = wall_s;
        totals.drift_ms_end = drift_ms;
        totals.drift_ms_max_abs = totals.drift_ms_max_abs.max(drift_ms.abs());
        totals.drift_jitter_ms_max = totals.drift_jitter_ms_max.max(drift_jitter_ms);
        if let Some((slope, intercept)) = fit.line() {
            totals.drift_ppm_fit = slope * 1000.0;
            totals.drift_ms_fit_end = intercept + slope * wall_s;
        }
        // The final row is sampled after the presenter has stopped, so it is not compared.
        if vframe >= 0 && !stopping {
            totals.video_minus_audio_max_abs = totals
                .video_minus_audio_max_abs
                .max((vframe - expected).abs());
        }
        load_sum += load_mean;
        totals.worker_load_mean_pct = load_sum / totals.rows as f64;
        totals.worker_load_max_pct = totals.worker_load_max_pct.max(load_peak);
        if ring_min_ms.is_finite() {
            totals.ring_min_ms = totals.ring_min_ms.min(ring_min_ms);
        }
        totals.limiter_gr_db_max = totals.limiter_gr_db_max.max(gr_db);
        latency_sum += lat_last;
        totals.latency_ms_mean = latency_sum / totals.rows as f64;
        if stopping {
            break;
        }
    }
    Ok(totals)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_fit_recovers_slope() {
        let mut f = LinFit::default();
        assert!(f.line().is_none());
        for i in 0..100 {
            let x = f64::from(i);
            let noise = if i % 2 == 0 { 0.3 } else { -0.3 };
            f.add(x, 2.0 + 0.01 * x + noise);
        }
        let (b, a) = f.line().unwrap();
        assert!((b - 0.01).abs() < 1e-3 && (a - 2.0).abs() < 0.1, "{b} {a}");
    }
}
