// SPDX-License-Identifier: GPL-3.0-or-later
//! Per-track worker pipeline (non-real-time): synthetic source → `rubato` resample to the device
//! rate → `signalsmith-stretch` (speed 0.5×–2×, pitch preserved) → lock-free SPSC ring (`rtrb`)
//! that the audio callback drains. This is the "decode/stretch ahead into ring buffers" half of §9.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rtrb::Producer;
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};
use signalsmith_stretch::Stretch;

use crate::synth::{Source, SourceKind};
use crate::util::AtomicF64;

/// Channels carried through the pipeline and the rings (interleaved stereo).
pub const CHANNELS: usize = 2;
/// Frames per block pushed into a track ring.
pub const BLOCK_FRAMES: usize = 256;
/// Input chunk of the FFT resampler (frames at the source rate).
pub const RESAMPLER_CHUNK: usize = 1024;

/// Playback speed of a track (time-stretch ratio; pitch is preserved).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Speed {
    /// Constant speed.
    Fixed(f64),
    /// Log-triangle sweep between `min` and `max` every `period_s` seconds of output.
    Sweep {
        /// Slowest speed.
        min: f64,
        /// Fastest speed.
        max: f64,
        /// Sweep period in seconds.
        period_s: f64,
    },
}

impl Speed {
    /// Speed at output time `t` seconds.
    pub fn at(self, t: f64) -> f64 {
        match self {
            Speed::Fixed(s) => s,
            Speed::Sweep { min, max, period_s } => {
                let phase = (t / period_s).fract();
                let tri = 1.0 - (2.0 * phase - 1.0).abs();
                min * (max / min).powf(tri)
            }
        }
    }
}

/// Static description of one mixer track.
#[derive(Clone, Debug)]
pub struct TrackSpec {
    /// Short label for logs.
    pub name: &'static str,
    /// What the synthetic source plays.
    pub kind: SourceKind,
    /// Source sample rate (resampled to the device rate when different).
    pub src_rate: u32,
    /// Time-stretch speed.
    pub speed: Speed,
    /// Route through signalsmith-stretch (the A/V click track bypasses it).
    pub stretch: bool,
    /// Initial gain (dB).
    pub gain_db: f32,
    /// Initial pan, -1 (left) .. +1 (right).
    pub pan: f32,
}

/// Growable interleaved-stereo FIFO used between the resampler and the stretcher (worker only).
#[derive(Debug, Default)]
pub struct Fifo {
    buf: Vec<f32>,
    start: usize,
}

impl Fifo {
    /// Frames available.
    pub fn frames(&self) -> usize {
        (self.buf.len() - self.start) / CHANNELS
    }
    /// Appends interleaved samples.
    pub fn push(&mut self, s: &[f32]) {
        self.buf.extend_from_slice(s);
    }
    /// The next `frames` frames (caller ensures availability).
    pub fn peek(&self, frames: usize) -> &[f32] {
        &self.buf[self.start..self.start + frames * CHANNELS]
    }
    /// Drops `frames` frames from the front.
    pub fn consume(&mut self, frames: usize) {
        self.start += frames * CHANNELS;
        if self.start >= 1 << 15 && self.start * 2 >= self.buf.len() {
            self.buf.drain(..self.start);
            self.start = 0;
        }
    }
}

/// Streaming wrapper over `signalsmith_stretch::Stretch` with fractional input accounting.
pub struct StreamStretch {
    st: Stretch,
    frac: f64,
    /// Reported input latency (frames).
    pub input_latency: usize,
    /// Reported output latency (frames).
    pub output_latency: usize,
}

impl std::fmt::Debug for StreamStretch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamStretch")
            .field("input_latency", &self.input_latency)
            .field("output_latency", &self.output_latency)
            .finish_non_exhaustive()
    }
}

impl StreamStretch {
    /// Stereo stretcher at `rate` (`cheaper` selects signalsmith's cheaper preset).
    pub fn new(rate: u32, cheaper: bool) -> Self {
        let st = if cheaper {
            Stretch::preset_cheaper(CHANNELS as u32, rate)
        } else {
            Stretch::preset_default(CHANNELS as u32, rate)
        };
        let (input_latency, output_latency) = (st.input_latency(), st.output_latency());
        Self {
            st,
            frac: 0.0,
            input_latency,
            output_latency,
        }
    }

    /// Input frames to feed for `out_frames` of output at `speed` (remainder carried forward, so
    /// consumed/produced converges to `speed` exactly).
    pub fn input_needed(&mut self, out_frames: usize, speed: f64) -> usize {
        let exact = out_frames as f64 * speed + self.frac;
        let n = exact.floor();
        self.frac = exact - n;
        n as usize
    }

    /// Stretches `input` (interleaved) into exactly `output.len() / 2` frames.
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) {
        self.st.process(input, output);
    }
}

/// Source + resampler + stretcher for one track; renders device-rate stereo blocks on demand.
pub struct TrackRenderer {
    source: Source,
    src_chunk: Vec<f32>,
    resampler: Option<Fft<f32>>,
    rs_out: Vec<f32>,
    rs_trim: usize,
    fifo: Fifo,
    stretch: Option<StreamStretch>,
    speed: Speed,
    out_rate: u32,
    frames_out: u64,
    discard: usize,
    /// Speed used for the most recent block.
    pub current_speed: Arc<AtomicF64>,
}

impl std::fmt::Debug for TrackRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrackRenderer")
            .field("src_rate", &self.source.rate())
            .field("out_rate", &self.out_rate)
            .field("resampler", &self.resampler.is_some())
            .field("stretch", &self.stretch)
            .finish_non_exhaustive()
    }
}

/// Latencies a track adds (reported at start-up).
#[derive(Clone, Copy, Debug, Default)]
pub struct TrackLatency {
    /// Resampler delay trimmed at start (device frames).
    pub resampler_delay: usize,
    /// Stretch input latency (frames), if stretched.
    pub stretch_in: usize,
    /// Stretch output latency (frames), if stretched.
    pub stretch_out: usize,
}

impl TrackRenderer {
    /// Builds the renderer for `spec` at the device rate `out_rate`.
    pub fn new(spec: &TrackSpec, out_rate: u32, cheaper_stretch: bool, seed: u64) -> Self {
        let source = Source::new(spec.kind.clone(), spec.src_rate, seed);
        let resampler = (spec.src_rate != out_rate).then(|| {
            Fft::<f32>::new(
                spec.src_rate as usize,
                out_rate as usize,
                RESAMPLER_CHUNK,
                CHANNELS,
                FixedSync::Input,
            )
            .unwrap_or_else(|e| panic!("rubato Fft {}→{out_rate}: {e}", spec.src_rate))
        });
        let (rs_out_len, rs_trim) = resampler.as_ref().map_or((0, 0), |r| {
            (r.output_frames_max() * CHANNELS, r.output_delay())
        });
        let stretch = spec
            .stretch
            .then(|| StreamStretch::new(out_rate, cheaper_stretch));
        let speed0 = spec.speed.at(0.0);
        // Align the stretched output with the timeline: drop the stretcher's start-up latency.
        let discard = stretch.as_ref().map_or(0, |s| {
            s.output_latency + (s.input_latency as f64 / speed0).round() as usize
        });
        Self {
            source,
            src_chunk: vec![0.0; RESAMPLER_CHUNK * CHANNELS],
            resampler,
            rs_out: vec![0.0; rs_out_len],
            rs_trim,
            fifo: Fifo::default(),
            stretch,
            speed: spec.speed,
            out_rate,
            frames_out: 0,
            discard,
            current_speed: Arc::new(AtomicF64::new(speed0)),
        }
    }

    /// Start-up latencies of this track's chain.
    pub fn latency(&self) -> TrackLatency {
        TrackLatency {
            resampler_delay: self.resampler.as_ref().map_or(0, Resampler::output_delay),
            stretch_in: self.stretch.as_ref().map_or(0, |s| s.input_latency),
            stretch_out: self.stretch.as_ref().map_or(0, |s| s.output_latency),
        }
    }

    /// Ensures the FIFO holds at least `frames` device-rate frames.
    fn feed(&mut self, frames: usize) {
        while self.fifo.frames() < frames {
            match self.resampler.as_mut() {
                Some(rs) => {
                    let n_in = rs.input_frames_next();
                    let chunk = &mut self.src_chunk[..n_in * CHANNELS];
                    self.source.fill(chunk);
                    let input = InterleavedSlice::new(&*chunk, CHANNELS, n_in)
                        .expect("input adapter sized from input_frames_next");
                    let out_frames = self.rs_out.len() / CHANNELS;
                    let mut output =
                        InterleavedSlice::new_mut(&mut self.rs_out, CHANNELS, out_frames)
                            .expect("output adapter sized from output_frames_max");
                    let (_, n_out) = rs
                        .process_into_buffer(&input, &mut output, None)
                        .expect("rubato process_into_buffer");
                    let skip = self.rs_trim.min(n_out);
                    self.rs_trim -= skip;
                    self.fifo
                        .push(&self.rs_out[skip * CHANNELS..n_out * CHANNELS]);
                }
                None => {
                    self.source.fill(&mut self.src_chunk);
                    self.fifo.push(&self.src_chunk);
                }
            }
        }
    }

    fn render_raw(&mut self, out: &mut [f32]) {
        let n = out.len() / CHANNELS;
        let t = self.frames_out as f64 / f64::from(self.out_rate);
        let speed = self.speed.at(t);
        match self.stretch.as_mut() {
            Some(st) => {
                let n_in = st.input_needed(n, speed);
                self.feed(n_in);
                let st = self.stretch.as_mut().expect("checked above");
                st.process(self.fifo.peek(n_in), out);
                self.fifo.consume(n_in);
            }
            None => {
                self.feed(n);
                out.copy_from_slice(self.fifo.peek(n));
                self.fifo.consume(n);
            }
        }
        self.current_speed.store(speed);
        self.frames_out += n as u64;
    }

    /// Renders exactly `out.len() / 2` device-rate frames aligned to the track timeline.
    pub fn render(&mut self, out: &mut [f32]) {
        while self.discard > 0 {
            let n = self.discard.min(out.len() / CHANNELS);
            self.render_raw(&mut out[..n * CHANNELS]);
            self.discard -= n;
            self.frames_out -= n as u64;
        }
        self.render_raw(out);
    }
}

/// A renderer bound to the producer end of its track ring.
#[derive(Debug)]
pub struct TrackPipeline {
    renderer: TrackRenderer,
    block: Vec<f32>,
    producer: Producer<f32>,
}

impl TrackPipeline {
    /// Binds `renderer` to `producer`.
    pub fn new(renderer: TrackRenderer, producer: Producer<f32>) -> Self {
        Self {
            renderer,
            block: vec![0.0; BLOCK_FRAMES * CHANNELS],
            producer,
        }
    }

    /// Fills the ring with whole blocks until it is full; returns blocks pushed.
    pub fn pump(&mut self) -> usize {
        let mut n = 0;
        while self.producer.slots() >= self.block.len() {
            self.renderer.render(&mut self.block);
            if self.producer.push_entire_slice(&self.block).is_err() {
                break;
            }
            n += 1;
        }
        n
    }

    /// True when the ring has no room for another block.
    pub fn is_full(&self) -> bool {
        self.producer.slots() < self.block.len()
    }
}

/// Worker thread accounting (CPU load of decode/stretch-ahead).
#[derive(Debug, Default)]
pub struct WorkerStats {
    /// Nanoseconds spent rendering.
    pub busy_ns: AtomicU64,
    /// Blocks pushed.
    pub blocks: AtomicU64,
}

/// Spawns a worker that keeps `pipes` topped up until `stop`.
pub fn spawn_worker(
    id: usize,
    mut pipes: Vec<TrackPipeline>,
    stop: Arc<AtomicBool>,
    stats: Arc<WorkerStats>,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name(format!("s2-worker-{id}"))
        .spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let t0 = Instant::now();
                let pushed: usize = pipes.iter_mut().map(TrackPipeline::pump).sum();
                if pushed > 0 {
                    let busy = u64::try_from(t0.elapsed().as_nanos()).unwrap_or(u64::MAX);
                    stats.busy_ns.fetch_add(busy, Ordering::Relaxed);
                    stats.blocks.fetch_add(pushed as u64, Ordering::Relaxed);
                } else {
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        })
        .expect("spawn worker thread")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::click_start_frame;

    fn tone(freq: f64) -> SourceKind {
        SourceKind::Tone {
            partials: vec![(freq, 0.5)],
            tremolo: None,
            detune: 0.0,
        }
    }

    fn spec(kind: SourceKind, src_rate: u32, speed: f64, stretch: bool) -> TrackSpec {
        TrackSpec {
            name: "t",
            kind,
            src_rate,
            speed: Speed::Fixed(speed),
            stretch,
            gain_db: 0.0,
            pan: 0.0,
        }
    }

    fn render_secs(r: &mut TrackRenderer, rate: u32, secs: f64) -> Vec<f32> {
        let frames = (secs * f64::from(rate)) as usize / BLOCK_FRAMES * BLOCK_FRAMES;
        let mut out = vec![0.0; frames * CHANNELS];
        for block in out.as_chunks_mut::<{ BLOCK_FRAMES * CHANNELS }>().0 {
            r.render(block);
        }
        out
    }

    /// Frequency estimate from positive-going zero crossings (left channel), skipping `skip` frames.
    fn zero_cross_freq(buf: &[f32], rate: u32, skip: usize) -> f64 {
        let left: Vec<f32> = buf
            .as_chunks::<2>()
            .0
            .iter()
            .skip(skip)
            .map(|f| f[0])
            .collect();
        let xs: Vec<usize> = (1..left.len())
            .filter(|&i| left[i - 1] < 0.0 && left[i] >= 0.0)
            .collect();
        let (first, last) = (xs[0], xs[xs.len() - 1]);
        (xs.len() - 1) as f64 * f64::from(rate) / (last - first) as f64
    }

    #[test]
    fn stretch_output_length_within_half_percent() {
        // WBS test: stretch ratio sanity, output length within 0.5 % of expected.
        for speed in [0.5, 1.0, 2.0] {
            let mut st = StreamStretch::new(48_000, false);
            let input = vec![0.1f32; 48_000 * 4 * CHANNELS];
            let mut consumed = 0usize;
            let mut produced = 0usize;
            let mut out = vec![0.0f32; BLOCK_FRAMES * CHANNELS];
            while consumed + (BLOCK_FRAMES as f64 * speed) as usize + 1 < 48_000 * 4 {
                let n_in = st.input_needed(BLOCK_FRAMES, speed);
                st.process(&input[consumed * 2..(consumed + n_in) * 2], &mut out);
                consumed += n_in;
                produced += BLOCK_FRAMES;
            }
            let expected = consumed as f64 / speed;
            let err = (produced as f64 - expected).abs() / expected;
            assert!(
                err < 0.005,
                "speed {speed}: produced {produced}, expected {expected}"
            );
        }
    }

    #[test]
    fn stretch_preserves_pitch() {
        for speed in [0.5, 2.0] {
            let mut r =
                TrackRenderer::new(&spec(tone(440.0), 48_000, speed, true), 48_000, false, 1);
            let out = render_secs(&mut r, 48_000, 2.0);
            let f = zero_cross_freq(&out, 48_000, 9_600);
            assert!(
                (f - 440.0).abs() / 440.0 < 0.01,
                "speed {speed}: measured {f} Hz"
            );
        }
    }

    #[test]
    fn stretch_scales_event_spacing() {
        // Clicks every 0.5 s of source time come out every 0.5 / speed s of output time.
        for speed in [0.5, 2.0] {
            let kind = SourceKind::Clicks {
                interval: 0.5,
                amp: 0.8,
                freq: 2000.0,
            };
            let mut r = TrackRenderer::new(&spec(kind, 48_000, speed, true), 48_000, false, 1);
            let out = render_secs(&mut r, 48_000, 8.0);
            // Envelope onsets: first frame above threshold after >= 50 ms of quiet.
            let env: Vec<f32> = out.as_chunks::<2>().0.iter().map(|f| f[0].abs()).collect();
            let mut onsets = Vec::new();
            let mut quiet = usize::MAX / 2;
            for (i, v) in env.iter().enumerate() {
                if *v > 0.2 {
                    if quiet > 2_400 {
                        onsets.push(i);
                    }
                    quiet = 0;
                } else {
                    quiet += 1;
                }
            }
            assert!(onsets.len() >= 6, "speed {speed}: onsets {onsets:?}");
            let spacing = (onsets[onsets.len() - 1] - onsets[1]) as f64 / (onsets.len() - 2) as f64;
            let expected = 0.5 * 48_000.0 / speed;
            let err = (spacing - expected).abs() / expected;
            assert!(
                err < 0.005,
                "speed {speed}: spacing {spacing}, expected {expected}"
            );
        }
    }

    #[test]
    fn resampler_keeps_click_on_the_timeline() {
        // 44.1 kHz click track → 48 kHz with the rubato delay trimmed: onset within ±2 frames.
        for src_rate in [44_100u32, 96_000] {
            let kind = SourceKind::Clicks {
                interval: 0.5,
                amp: 0.8,
                freq: 3000.0,
            };
            let mut r = TrackRenderer::new(&spec(kind, src_rate, 1.0, false), 48_000, false, 1);
            let out = render_secs(&mut r, 48_000, 1.2);
            for k in 1..=2u64 {
                let expected = click_start_frame(k, 0.5, 48_000.0);
                // Peak of the first lobe (quarter period of 3 kHz at 48 kHz = 4 frames after onset).
                let lo = expected as usize - 50;
                let peak = (lo..lo + 100)
                    .max_by(|&a, &b| out[a * 2].abs().total_cmp(&out[b * 2].abs()))
                    .unwrap();
                let onset_est = peak as f64 - 4.0;
                assert!(
                    (onset_est - expected).abs() <= 2.0,
                    "{src_rate} Hz click {k}: onset {onset_est}, expected {expected}"
                );
            }
        }
    }

    #[test]
    fn speed_sweep_spans_range() {
        let s = Speed::Sweep {
            min: 0.5,
            max: 2.0,
            period_s: 20.0,
        };
        assert!((s.at(0.0) - 0.5).abs() < 1e-9);
        assert!((s.at(10.0) - 2.0).abs() < 1e-9);
        assert!((s.at(5.0) - 1.0).abs() < 1e-9);
    }
}
