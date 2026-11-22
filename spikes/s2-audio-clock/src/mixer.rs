// SPDX-License-Identifier: GPL-3.0-or-later
//! Real-time mixer graph: N track rings → per-track gain/pan → master bus → limiter → device.
//!
//! Runs inside the cpal callback. No allocation, no locks, no syscalls: rings are `rtrb` SPSC
//! consumers, parameters are atomics, every buffer is allocated at construction. A track whose ring
//! runs dry plays silence for the missing frames and later drops the same number of frames, so it
//! stays aligned with the timeline (the clock never stretches to wait for a slow worker).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use rtrb::{Consumer, Producer};

use crate::limiter::Limiter;
use crate::util::{AtomicF32, db_to_lin};

/// Largest block processed in one pass (bigger device buffers are split).
pub const MAX_BLOCK: usize = 1024;

/// Per-track parameters, written by any control thread and read by the callback.
#[derive(Debug)]
pub struct TrackParams {
    /// Gain in dB.
    pub gain_db: AtomicF32,
    /// Pan, -1 (left) .. +1 (right).
    pub pan: AtomicF32,
    /// Mute flag.
    pub mute: AtomicBool,
}

impl TrackParams {
    /// Parameters with the given initial gain and pan.
    pub fn new(gain_db: f32, pan: f32) -> Self {
        Self {
            gain_db: AtomicF32::new(gain_db),
            pan: AtomicF32::new(pan),
            mute: AtomicBool::new(false),
        }
    }
}

/// Stereo balance pan law for stereo sources: unity on both sides at centre, the far side
/// attenuated with a quarter-cosine (constant-power curve) towards the extremes.
#[inline]
pub fn pan_gains(pan: f32) -> (f32, f32) {
    let p = pan.clamp(-1.0, 1.0);
    let far = (p.abs() * std::f32::consts::FRAC_PI_2).cos();
    if p <= 0.0 { (1.0, far) } else { (far, 1.0) }
}

/// Consumer end of a track ring plus its smoothing state.
#[derive(Debug)]
pub struct TrackInput {
    consumer: Consumer<f32>,
    params: Arc<TrackParams>,
    /// Samples owed to the timeline after an underrun (dropped as soon as they arrive).
    deficit: usize,
    gl: f32,
    gr: f32,
}

impl TrackInput {
    /// Wraps a ring consumer and its parameters.
    pub fn new(consumer: Consumer<f32>, params: Arc<TrackParams>) -> Self {
        let (l, r) = pan_gains(params.pan.load());
        let g = db_to_lin(params.gain_db.load());
        Self {
            consumer,
            params,
            deficit: 0,
            gl: g * l,
            gr: g * r,
        }
    }

    /// Samples currently buffered in the ring.
    pub fn buffered_samples(&self) -> usize {
        self.consumer.slots()
    }
}

/// Mixer counters shared with the logger (all lock-free).
#[derive(Debug)]
pub struct MixerStats {
    /// Track blocks that hit an empty ring (cumulative).
    pub underrun_events: AtomicU64,
    /// Frames of silence inserted because a ring was empty (cumulative).
    pub underrun_frames: AtomicU64,
    /// Smallest ring fill (frames, any track) since the logger last reset it.
    pub ring_min_frames: AtomicU64,
    /// Smallest limiter gain since the last reset (f32 bits; positive floats order like u32).
    pub limiter_min_gain: AtomicU32,
    /// Master frames the loudness tap could not accept (meter thread too slow).
    pub meter_dropped: AtomicU64,
}

impl Default for MixerStats {
    fn default() -> Self {
        Self {
            underrun_events: AtomicU64::new(0),
            underrun_frames: AtomicU64::new(0),
            ring_min_frames: AtomicU64::new(u64::MAX),
            limiter_min_gain: AtomicU32::new(1.0f32.to_bits()),
            meter_dropped: AtomicU64::new(0),
        }
    }
}

/// The real-time mixer.
#[derive(Debug)]
pub struct Mixer {
    tracks: Vec<TrackInput>,
    scratch: Vec<f32>,
    mix: Vec<f32>,
    limiter: Limiter,
    tap: Option<Producer<f32>>,
    stats: Arc<MixerStats>,
}

impl Mixer {
    /// Builds the mixer; all memory is allocated here.
    pub fn new(
        tracks: Vec<TrackInput>,
        limiter: Limiter,
        tap: Option<Producer<f32>>,
        stats: Arc<MixerStats>,
    ) -> Self {
        Self {
            tracks,
            scratch: vec![0.0; MAX_BLOCK * 2],
            mix: vec![0.0; MAX_BLOCK * 2],
            limiter,
            tap,
            stats,
        }
    }

    /// Number of tracks.
    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }

    /// Fills `out` (interleaved, `channels` per frame) with the next block of the mix.
    pub fn process(&mut self, out: &mut [f32], channels: usize) {
        let ch = channels.max(1);
        for chunk in out.chunks_mut(MAX_BLOCK * ch) {
            self.process_block(chunk, ch);
        }
    }

    fn process_block(&mut self, out: &mut [f32], ch: usize) {
        let n = out.len() / ch;
        let mix = &mut self.mix[..n * 2];
        mix.fill(0.0);
        let mut min_fill = u64::MAX;
        for t in &mut self.tracks {
            // Catch up after an earlier underrun so the track stays on the timeline.
            if t.deficit > 0 {
                let drop = t.consumer.slots().min(t.deficit);
                if drop > 0 {
                    if let Ok(chunk) = t.consumer.read_chunk(drop) {
                        chunk.commit_all();
                    }
                    t.deficit -= drop;
                }
            }
            let buf = &mut self.scratch[..n * 2];
            let (_, missing) = t.consumer.pop_partial_slice(buf);
            let missing_len = missing.len();
            if missing_len > 0 {
                missing.fill(0.0);
                t.deficit += missing_len;
                self.stats.underrun_events.fetch_add(1, Ordering::Relaxed);
                self.stats
                    .underrun_frames
                    .fetch_add((missing_len / 2) as u64, Ordering::Relaxed);
            }
            min_fill = min_fill.min((t.consumer.slots() / 2) as u64);

            let (tl, tr) = if t.params.mute.load(Ordering::Relaxed) {
                (0.0, 0.0)
            } else {
                let g = db_to_lin(t.params.gain_db.load());
                let (pl, pr) = pan_gains(t.params.pan.load());
                (g * pl, g * pr)
            };
            // Linear ramp to the new gains across the block (no zipper noise).
            let inv = 1.0 / n.max(1) as f32;
            let (dl, dr) = ((tl - t.gl) * inv, (tr - t.gr) * inv);
            let (mut gl, mut gr) = (t.gl, t.gr);
            for (m, s) in mix
                .as_chunks_mut::<2>()
                .0
                .iter_mut()
                .zip(buf.as_chunks::<2>().0)
            {
                gl += dl;
                gr += dr;
                m[0] += s[0] * gl;
                m[1] += s[1] * gr;
            }
            t.gl = tl;
            t.gr = tr;
        }

        let g = self.limiter.process(mix);
        self.stats
            .limiter_min_gain
            .fetch_min(g.max(0.0).to_bits(), Ordering::Relaxed);
        if min_fill != u64::MAX {
            self.stats
                .ring_min_frames
                .fetch_min(min_fill, Ordering::Relaxed);
        }

        match ch {
            1 => {
                for (o, m) in out.iter_mut().zip(mix.as_chunks::<2>().0) {
                    *o = 0.5 * (m[0] + m[1]);
                }
            }
            _ => {
                for (o, m) in out.chunks_exact_mut(ch).zip(mix.as_chunks::<2>().0) {
                    o[0] = m[0];
                    o[1] = m[1];
                    o[2..].fill(0.0);
                }
            }
        }

        if let Some(tap) = self.tap.as_mut() {
            let (_, rest) = tap.push_partial_slice(mix);
            if !rest.is_empty() {
                self.stats
                    .meter_dropped
                    .fetch_add((rest.len() / 2) as u64, Ordering::Relaxed);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rt_alloc::RtSection;
    use rtrb::RingBuffer;

    fn limiter() -> Limiter {
        // Effectively transparent for these tests: 0 dBFS ceiling, levels well below it.
        Limiter::new(48_000, 0.0, 0.02, 100.0)
    }

    #[test]
    fn pan_law_centre_is_unity_for_stereo_sources() {
        assert_eq!(pan_gains(0.0), (1.0, 1.0));
        let (l, r) = pan_gains(-1.0);
        assert_eq!(l, 1.0);
        assert!(r.abs() < 1e-6);
        let (l, r) = pan_gains(0.5);
        assert!((l - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6 && r == 1.0);
    }

    #[test]
    fn ring_buffer_wraps_and_preserves_order_across_threads() {
        // WBS test: ring buffer. A producer thread pushes a counter in uneven chunk sizes; the
        // consumer pops uneven chunk sizes and checks the sequence is gap-free and ordered.
        let (mut p, mut c) = RingBuffer::<f32>::new(1_000);
        const N: u32 = 2_000_000;
        let producer = std::thread::spawn(move || {
            let mut next = 0u32;
            let mut sizes = [7usize, 64, 333, 1, 999].iter().cycle();
            let mut chunk = Vec::with_capacity(1_000);
            while next < N {
                let want = (*sizes.next().unwrap()).min((N - next) as usize);
                chunk.clear();
                chunk.extend((next..next + want as u32).map(|v| v as f32));
                let mut rest: &[f32] = &chunk;
                while !rest.is_empty() {
                    let (_, r) = p.push_partial_slice(rest);
                    rest = r;
                    std::hint::spin_loop();
                }
                next += want as u32;
            }
        });
        let mut expect = 0u32;
        let mut buf = vec![0.0f32; 777];
        let mut sizes = [5usize, 777, 128, 3].iter().cycle();
        while expect < N {
            let want = *sizes.next().unwrap();
            let (got, _) = c.pop_partial_slice(&mut buf[..want]);
            for v in got.iter() {
                // f32 represents these integers exactly (N < 2^24).
                assert_eq!(*v as u32, expect);
                expect += 1;
            }
        }
        producer.join().unwrap();
    }

    #[test]
    fn underrun_inserts_silence_then_realigns() {
        let (mut p, c) = RingBuffer::<f32>::new(4_096);
        let params = Arc::new(TrackParams::new(0.0, 0.0));
        let stats = Arc::new(MixerStats::default());
        let mut mixer = Mixer::new(
            vec![TrackInput::new(c, params)],
            limiter(),
            None,
            stats.clone(),
        );
        let lat = Limiter::new(48_000, 0.0, 0.02, 100.0).latency();
        // Timeline value v at frame f is f / 10000 (both channels).
        let frame_val = |f: usize| f as f32 / 10_000.0;
        let push = |p: &mut rtrb::Producer<f32>, from: usize, n: usize| {
            let v: Vec<f32> = (from..from + n).flat_map(|f| [frame_val(f); 2]).collect();
            p.push_entire_slice(&v).unwrap();
        };
        let mut out = vec![0.0f32; 2 * 64];
        let mut all = Vec::new();
        push(&mut p, 0, 64);
        mixer.process(&mut out, 2); // frames 0..64
        all.extend_from_slice(&out);
        mixer.process(&mut out, 2); // underrun: frames 64..128 are silence
        all.extend_from_slice(&out);
        assert_eq!(stats.underrun_frames.load(Ordering::Relaxed), 64);
        push(&mut p, 64, 128); // the late worker delivers frames 64..192
        mixer.process(&mut out, 2); // must play 128..192, dropping the stale 64..128
        all.extend_from_slice(&out);
        // Account for the limiter's look-ahead delay.
        let f = 128 + 32;
        let got = all[2 * (f + lat)];
        assert!(
            (got - frame_val(f)).abs() < 1e-6,
            "got {got}, want {}",
            frame_val(f)
        );
    }

    #[test]
    fn process_does_not_allocate() {
        let mut inputs = Vec::new();
        let mut producers = Vec::new();
        for i in 0..8 {
            let (p, c) = RingBuffer::<f32>::new(48_000);
            producers.push(p);
            inputs.push(TrackInput::new(
                c,
                Arc::new(TrackParams::new(-12.0, i as f32 / 8.0)),
            ));
        }
        let (tap, _tap_c) = RingBuffer::<f32>::new(8_192);
        let stats = Arc::new(MixerStats::default());
        let mut mixer = Mixer::new(inputs, limiter(), Some(tap), stats);
        let block = vec![0.25f32; 2 * 256];
        let mut out = vec![0.0f32; 4 * 256]; // 4-channel device: exercises channel mapping
        let mut allocs = 0;
        for _ in 0..500 {
            for p in &mut producers {
                let _ = p.push_entire_slice(&block);
            }
            let section = RtSection::enter();
            mixer.process(&mut out, 4);
            allocs += section.exit();
        }
        assert_eq!(allocs, 0);
        assert!(
            out.as_chunks::<4>()
                .0
                .iter()
                .all(|f| f[2] == 0.0 && f[3] == 0.0)
        );
    }
}
