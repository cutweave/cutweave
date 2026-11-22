// SPDX-License-Identifier: GPL-3.0-or-later
//! EBU R128 loudness meter on the master bus (`ebur128`), off the real-time thread.
//!
//! The callback copies the post-limiter master into an SPSC tap ring; this thread drains it into
//! `EbuR128` (momentary, short-term, integrated, true peak) and publishes the readings as atomics.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ebur128::{EbuR128, Mode};
use rtrb::Consumer;

use crate::util::AtomicF32;

/// Latest loudness readings (LUFS / dBTP; `-inf` while undefined).
#[derive(Debug)]
pub struct LoudnessStats {
    /// Momentary loudness (400 ms).
    pub momentary: AtomicF32,
    /// Short-term loudness (3 s).
    pub shortterm: AtomicF32,
    /// Integrated (gated) loudness since start.
    pub integrated: AtomicF32,
    /// Max true peak over both channels since start (dBTP).
    pub true_peak_dbtp: AtomicF32,
    /// Frames measured.
    pub frames: AtomicU64,
}

impl Default for LoudnessStats {
    fn default() -> Self {
        Self {
            momentary: AtomicF32::new(f32::NEG_INFINITY),
            shortterm: AtomicF32::new(f32::NEG_INFINITY),
            integrated: AtomicF32::new(f32::NEG_INFINITY),
            true_peak_dbtp: AtomicF32::new(f32::NEG_INFINITY),
            frames: AtomicU64::new(0),
        }
    }
}

fn new_meter(rate: u32) -> EbuR128 {
    EbuR128::new(2, rate, Mode::M | Mode::S | Mode::I | Mode::TRUE_PEAK)
        .expect("ebur128 supports stereo at any device rate")
}

fn publish(m: &EbuR128, stats: &LoudnessStats) {
    let lufs = |r: Result<f64, ebur128::Error>| r.map_or(f32::NEG_INFINITY, |v| v as f32);
    stats.momentary.store(lufs(m.loudness_momentary()));
    stats.shortterm.store(lufs(m.loudness_shortterm()));
    stats.integrated.store(lufs(m.loudness_global()));
    let tp = (0..2)
        .filter_map(|c| m.true_peak(c).ok())
        .fold(0.0f64, f64::max);
    stats.true_peak_dbtp.store(20.0 * (tp as f32).log10());
}

/// Spawns the meter thread draining `tap` (interleaved stereo at `rate`).
pub fn spawn_meter(
    mut tap: Consumer<f32>,
    rate: u32,
    stats: Arc<LoudnessStats>,
    stop: Arc<AtomicBool>,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("s2-meter".into())
        .spawn(move || {
            let mut m = new_meter(rate);
            let mut last_publish = Instant::now();
            while !stop.load(Ordering::Relaxed) {
                let n = tap.slots() & !1;
                if n > 0 {
                    if let Ok(chunk) = tap.read_chunk(n) {
                        let (a, b) = chunk.as_slices();
                        // The ring capacity is even, so both halves hold whole frames.
                        let _ = m.add_frames_f32(a);
                        let _ = m.add_frames_f32(b);
                        chunk.commit_all();
                    }
                    stats.frames.fetch_add((n / 2) as u64, Ordering::Relaxed);
                }
                if last_publish.elapsed() >= Duration::from_millis(100) {
                    publish(&m, &stats);
                    last_publish = Instant::now();
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            publish(&m, &stats);
        })
        .expect("spawn meter thread")
}

/// Integrated loudness (LUFS) of an interleaved stereo buffer (offline helper for tests).
pub fn integrated_lufs(frames: &[f32], rate: u32) -> f64 {
    let mut m = new_meter(rate);
    m.add_frames_f32(frames).expect("stereo frames");
    m.loudness_global().expect("integrated loudness")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_tone_reads_minus_23_lufs() {
        // EBU Tech 3341 case 1: stereo 1 kHz sine at -23 dBFS for 20 s → -23.0 ±0.1 LUFS.
        let rate = 48_000;
        let amp = 10f32.powf(-23.0 / 20.0);
        let buf: Vec<f32> = (0..rate * 20)
            .flat_map(|i| {
                let v = amp
                    * (std::f64::consts::TAU * 1000.0 * f64::from(i) / f64::from(rate)).sin()
                        as f32;
                [v, v]
            })
            .collect();
        let lufs = integrated_lufs(&buf, rate);
        assert!((lufs + 23.0).abs() <= 0.1, "integrated {lufs} LUFS");
    }
}
