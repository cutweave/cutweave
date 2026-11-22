// SPDX-License-Identifier: GPL-3.0-or-later
//! Wires sources, workers, mixer, meter, clock and the cpal output stream together.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    BufferSize, ErrorKind, FromSample, OutputStreamTimestamp, SampleFormat, SizedSample,
    StreamConfig, SupportedBufferSize,
};
use rtrb::RingBuffer;

use crate::clock::{Anchor, AudioClock};
use crate::limiter::Limiter;
use crate::meter::{LoudnessStats, spawn_meter};
use crate::mixer::{MAX_BLOCK, Mixer, MixerStats, TrackInput, TrackParams};
use crate::rt_alloc::RtSection;
use crate::track::{
    BLOCK_FRAMES, CHANNELS, TrackLatency, TrackPipeline, TrackRenderer, TrackSpec, WorkerStats,
    spawn_worker,
};
use crate::util::NANOS;

/// Callback timing and health counters. "Interval" fields are reset by the logger each row.
#[derive(Debug)]
pub struct CallbackStats {
    /// Callbacks so far.
    pub callbacks: AtomicU64,
    /// Interval min frames per callback (reset to `u64::MAX`).
    pub frames_min: AtomicU64,
    /// Interval max frames per callback.
    pub frames_max: AtomicU64,
    /// Whole-run min frames per callback.
    pub frames_min_all: AtomicU64,
    /// Whole-run max frames per callback.
    pub frames_max_all: AtomicU64,
    /// Interval max time spent inside the callback (ns).
    pub dur_ns_max: AtomicU64,
    /// Interval sum of callback durations (ns).
    pub dur_ns_sum: AtomicU64,
    /// Interval callback count (for the mean).
    pub dur_count: AtomicU64,
    /// Whole-run max callback duration (ns).
    pub dur_ns_max_all: AtomicU64,
    /// Interval max time between callback entries (ns).
    pub gap_ns_max: AtomicU64,
    /// Interval max load: callback duration / buffer duration (per mille).
    pub load_permille_max: AtomicU64,
    /// Whole-run max load (per mille).
    pub load_permille_max_all: AtomicU64,
    /// Latest reported output latency: `playback - callback` (ns).
    pub latency_ns_last: AtomicU64,
    /// Interval min latency (ns).
    pub latency_ns_min: AtomicU64,
    /// Interval max latency (ns).
    pub latency_ns_max: AtomicU64,
    /// Whole-run min / max latency (ns).
    pub latency_ns_min_all: AtomicU64,
    /// Whole-run max latency (ns).
    pub latency_ns_max_all: AtomicU64,
    /// Interval max |playback timestamp - expected continuation| (ns).
    pub ts_err_ns_max: AtomicU64,
    /// Playback-timestamp discontinuities beyond the glitch threshold (device under-/overruns).
    pub glitches: AtomicU64,
    /// Callbacks that arrived later than 1.5 buffers + 1 ms after the previous one.
    pub late_callbacks: AtomicU64,
    /// Heap operations observed inside the callback (must stay 0).
    pub rt_allocs: AtomicU64,
    /// `ErrorKind::Xrun` reports from cpal.
    pub xruns: AtomicU64,
    /// Other stream errors from cpal.
    pub errors: AtomicU64,
    /// Code of the last non-xrun error (see [`error_code`]).
    pub last_error: AtomicU32,
    /// Max playback-timestamp discontinuity during warm-up (first 0.5 s; not counted as glitch).
    pub warmup_ts_err_ns_max: AtomicU64,
    /// Frames of the first [`STARTUP_LOG`] callbacks.
    pub startup_frames: [AtomicU64; STARTUP_LOG],
    /// Reported latency of the first [`STARTUP_LOG`] callbacks (ns).
    pub startup_latency_ns: [AtomicU64; STARTUP_LOG],
}

/// Callbacks whose size/latency are recorded verbatim for the start-up report.
pub const STARTUP_LOG: usize = 8;

/// Playback within this many seconds of the start is warm-up: the device clock is still starting,
/// so timestamp jumps are reported separately and not counted as glitches.
pub const WARMUP_S: f64 = 0.5;

impl Default for CallbackStats {
    fn default() -> Self {
        let z = || AtomicU64::new(0);
        let m = || AtomicU64::new(u64::MAX);
        Self {
            warmup_ts_err_ns_max: z(),
            startup_frames: std::array::from_fn(|_| z()),
            startup_latency_ns: std::array::from_fn(|_| z()),
            callbacks: z(),
            frames_min: m(),
            frames_max: z(),
            frames_min_all: m(),
            frames_max_all: z(),
            dur_ns_max: z(),
            dur_ns_sum: z(),
            dur_count: z(),
            dur_ns_max_all: z(),
            gap_ns_max: z(),
            load_permille_max: z(),
            load_permille_max_all: z(),
            latency_ns_last: z(),
            latency_ns_min: m(),
            latency_ns_max: z(),
            latency_ns_min_all: m(),
            latency_ns_max_all: z(),
            ts_err_ns_max: z(),
            glitches: z(),
            late_callbacks: z(),
            rt_allocs: z(),
            xruns: z(),
            errors: z(),
            last_error: AtomicU32::new(0),
        }
    }
}

/// Stable numeric code for a cpal error kind (CSV/summary friendly).
pub fn error_code(kind: ErrorKind) -> u32 {
    match kind {
        ErrorKind::DeviceBusy => 1,
        ErrorKind::DeviceChanged => 2,
        ErrorKind::DeviceNotAvailable => 3,
        ErrorKind::HostUnavailable => 4,
        ErrorKind::InvalidInput => 5,
        ErrorKind::PermissionDenied => 6,
        ErrorKind::RealtimeDenied => 7,
        ErrorKind::ResourceExhausted => 8,
        ErrorKind::StreamInvalidated => 9,
        ErrorKind::UnsupportedConfig => 10,
        ErrorKind::UnsupportedOperation => 11,
        ErrorKind::Xrun => 12,
        ErrorKind::BackendError => 13,
        _ => 99,
    }
}

/// The state owned by the cpal data callback.
#[derive(Debug)]
pub struct Callback {
    mixer: Mixer,
    clock: Arc<AudioClock>,
    stats: Arc<CallbackStats>,
    channels: usize,
    latency_comp: bool,
    scratch: Vec<f32>,
    frames_written: u64,
    prev_enter_ns: u64,
    prev_playback_ns: u64,
    prev_frames: u64,
    glitch_threshold_ns: u64,
}

#[inline]
fn fetch_max(a: &AtomicU64, v: u64) {
    a.fetch_max(v, Ordering::Relaxed);
}
#[inline]
fn fetch_min(a: &AtomicU64, v: u64) {
    a.fetch_min(v, Ordering::Relaxed);
}

impl Callback {
    /// Callback state for a stream with `channels` output channels.
    pub fn new(
        mixer: Mixer,
        clock: Arc<AudioClock>,
        stats: Arc<CallbackStats>,
        channels: usize,
        latency_comp: bool,
    ) -> Self {
        Self {
            mixer,
            clock,
            stats,
            channels: channels.max(1),
            latency_comp,
            scratch: vec![0.0; MAX_BLOCK * channels.max(1)],
            frames_written: 0,
            prev_enter_ns: 0,
            prev_playback_ns: 0,
            prev_frames: 0,
            glitch_threshold_ns: 1_500_000,
        }
    }

    /// The data callback body: publish the clock anchor, mix, convert, account. Real-time safe.
    pub fn process<T>(&mut self, out: &mut [T], ts: OutputStreamTimestamp)
    where
        T: SizedSample + FromSample<f32>,
    {
        let enter = Instant::now();
        let section = RtSection::enter();
        let enter_ns = self.clock.wall_ns(enter);
        let frames = (out.len() / self.channels) as u64;
        let rate = f64::from(self.clock.sample_rate());

        // 1. Clock: the first frame of this buffer is audible `playback - callback` from now.
        let latency_ns = u64::try_from(
            ts.playback
                .saturating_duration_since(ts.callback)
                .as_nanos(),
        )
        .unwrap_or(u64::MAX);
        let anchor_wall = if self.latency_comp {
            enter_ns.saturating_add(latency_ns)
        } else {
            enter_ns
        };
        let st = &*self.stats;
        let n = st.callbacks.fetch_add(1, Ordering::Relaxed);
        // The first callback primes an empty device buffer before the device clock runs (WASAPI
        // reports `playback == callback` there), so its prediction is not used as an anchor.
        if n > 0 {
            self.clock.publish(Anchor {
                frame: self.frames_written,
                wall_ns: anchor_wall,
                span_frames: frames,
            });
        }
        let warmup = (self.frames_written as f64) < WARMUP_S * rate;
        if let Some(i) = usize::try_from(n).ok().filter(|i| *i < STARTUP_LOG) {
            st.startup_frames[i].store(frames, Ordering::Relaxed);
            st.startup_latency_ns[i].store(latency_ns, Ordering::Relaxed);
        }

        // 2. Mix (into f32 scratch, then convert to the device format).
        let ch = self.channels;
        for chunk in out.chunks_mut(MAX_BLOCK * ch) {
            let s = &mut self.scratch[..chunk.len()];
            self.mixer.process(s, ch);
            for (d, v) in chunk.iter_mut().zip(s.iter()) {
                *d = T::from_sample(*v);
            }
        }
        self.frames_written += frames;
        self.clock.add_submitted(frames);

        // 3. Health accounting (atomics only). Warm-up callbacks are kept out of the steady-state
        // latency and glitch statistics (the start-up arrays above record them instead).
        fetch_min(&st.frames_min, frames);
        fetch_max(&st.frames_max, frames);
        fetch_min(&st.frames_min_all, frames);
        fetch_max(&st.frames_max_all, frames);
        st.latency_ns_last.store(latency_ns, Ordering::Relaxed);
        if !warmup {
            fetch_min(&st.latency_ns_min, latency_ns);
            fetch_max(&st.latency_ns_max, latency_ns);
            fetch_min(&st.latency_ns_min_all, latency_ns);
            fetch_max(&st.latency_ns_max_all, latency_ns);
        }
        let playback_ns = u64::try_from(ts.playback.as_nanos()).unwrap_or(u64::MAX);
        if n > 0 {
            let prev_dur_ns = (self.prev_frames as f64 * NANOS / rate) as u64;
            let gap = enter_ns.saturating_sub(self.prev_enter_ns);
            fetch_max(&st.gap_ns_max, gap);
            let expected = self.prev_playback_ns.saturating_add(prev_dur_ns);
            let err = playback_ns.abs_diff(expected);
            if warmup {
                fetch_max(&st.warmup_ts_err_ns_max, err);
            } else {
                if gap > prev_dur_ns + prev_dur_ns / 2 + 1_000_000 {
                    st.late_callbacks.fetch_add(1, Ordering::Relaxed);
                }
                fetch_max(&st.ts_err_ns_max, err);
                if err > self.glitch_threshold_ns {
                    st.glitches.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        self.prev_enter_ns = enter_ns;
        self.prev_playback_ns = playback_ns;
        self.prev_frames = frames;

        let dur_ns = u64::try_from(enter.elapsed().as_nanos()).unwrap_or(u64::MAX);
        fetch_max(&st.dur_ns_max, dur_ns);
        fetch_max(&st.dur_ns_max_all, dur_ns);
        st.dur_ns_sum.fetch_add(dur_ns, Ordering::Relaxed);
        st.dur_count.fetch_add(1, Ordering::Relaxed);
        let buf_ns = (frames as f64 * NANOS / rate).max(1.0);
        let load = (dur_ns as f64 / buf_ns * 1000.0) as u64;
        fetch_max(&st.load_permille_max, load);
        fetch_max(&st.load_permille_max_all, load);

        // 4. Real-time discipline: no heap traffic inside the callback.
        let allocs = section.exit();
        if allocs > 0 {
            st.rt_allocs.fetch_add(allocs, Ordering::Relaxed);
        }
        debug_assert!(
            allocs == 0,
            "audio callback performed {allocs} heap operation(s)"
        );
    }
}

/// Built tracks: worker-side pipelines and callback-side inputs.
#[derive(Debug)]
pub struct Tracks {
    /// Worker side.
    pub pipelines: Vec<TrackPipeline>,
    /// Callback side.
    pub inputs: Vec<TrackInput>,
    /// Shared parameters (index = track).
    pub params: Vec<Arc<TrackParams>>,
    /// Start-up latencies per track.
    pub latencies: Vec<TrackLatency>,
}

/// Builds renderers and SPSC rings for `specs` at the device `rate`.
pub fn build_tracks(specs: &[TrackSpec], rate: u32, ring_ms: u32, cheaper: bool) -> Tracks {
    let ring_frames =
        (rate as usize * ring_ms as usize / 1000).div_ceil(BLOCK_FRAMES) * BLOCK_FRAMES;
    let mut t = Tracks {
        pipelines: Vec::new(),
        inputs: Vec::new(),
        params: Vec::new(),
        latencies: Vec::new(),
    };
    for (i, spec) in specs.iter().enumerate() {
        let renderer = TrackRenderer::new(spec, rate, cheaper, 0x5EED + i as u64);
        t.latencies.push(renderer.latency());
        let (p, c) = RingBuffer::<f32>::new(ring_frames.max(2 * BLOCK_FRAMES) * CHANNELS);
        let params = Arc::new(TrackParams::new(spec.gain_db, spec.pan));
        t.pipelines.push(TrackPipeline::new(renderer, p));
        t.inputs.push(TrackInput::new(c, params.clone()));
        t.params.push(params);
    }
    t
}

/// Engine options (from the CLI).
#[derive(Clone, Debug)]
pub struct EngineConfig {
    /// Tracks to play.
    pub specs: Vec<TrackSpec>,
    /// Requested frames per callback (`None` = backend default).
    pub buffer: Option<u32>,
    /// Ring capacity per track (ms).
    pub ring_ms: u32,
    /// Worker threads for source/resample/stretch.
    pub workers: usize,
    /// Apply cpal's reported output latency to the clock.
    pub latency_comp: bool,
    /// Master limiter ceiling (dBFS).
    pub limiter_ceiling_db: f32,
    /// Output device name substring (default device if `None`).
    pub device: Option<String>,
    /// Use signalsmith's cheaper preset.
    pub cheaper_stretch: bool,
    /// Raise worker threads to high priority.
    pub high_priority_workers: bool,
}

/// What the stream actually ended up with.
#[derive(Clone, Debug)]
pub struct StreamInfo {
    /// cpal host name.
    pub host: String,
    /// Output device name.
    pub device: String,
    /// Device sample rate.
    pub sample_rate: u32,
    /// Device channels.
    pub channels: u16,
    /// Device sample format.
    pub format: String,
    /// Requested buffer ("256" / "default").
    pub buffer_requested: String,
    /// Buffer mode used to build the stream.
    pub buffer_used: String,
    /// Buffer-size range the device reports.
    pub supported_buffer: String,
    /// `Stream::buffer_size()` after build (frames per callback per cpal).
    pub cpal_buffer_size: Option<u32>,
    /// Per-track (name, latency).
    pub track_latency: Vec<(String, TrackLatency)>,
    /// Limiter look-ahead (frames).
    pub limiter_latency: usize,
}

/// Running engine handle.
pub struct Engine {
    stream: cpal::Stream,
    /// Master clock.
    pub clock: Arc<AudioClock>,
    /// Callback health.
    pub cb_stats: Arc<CallbackStats>,
    /// Mixer health.
    pub mix_stats: Arc<MixerStats>,
    /// Loudness readings.
    pub loudness: Arc<LoudnessStats>,
    /// One per worker thread.
    pub worker_stats: Vec<Arc<WorkerStats>>,
    /// Track parameters.
    pub params: Vec<Arc<TrackParams>>,
    /// Resolved stream configuration.
    pub info: StreamInfo,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("info", &self.info)
            .finish_non_exhaustive()
    }
}

fn pick_device(host: &cpal::Host, name: Option<&str>) -> Result<cpal::Device, String> {
    match name {
        None => host
            .default_output_device()
            .ok_or_else(|| "no default output device".to_string()),
        Some(needle) => {
            let needle = needle.to_lowercase();
            host.output_devices()
                .map_err(|e| format!("enumerate devices: {e}"))?
                .find(|d| d.to_string().to_lowercase().contains(&needle))
                .ok_or_else(|| format!("no output device matching `{needle}`"))
        }
    }
}

/// Prints the output devices of the default host.
pub fn list_devices() -> Result<(), String> {
    let host = cpal::default_host();
    println!("host: {}", host.id().name());
    let default = host.default_output_device().map(|d| d.to_string());
    for d in host
        .output_devices()
        .map_err(|e| format!("enumerate devices: {e}"))?
    {
        let name = d.to_string();
        let mark = if Some(&name) == default.as_ref() {
            "*"
        } else {
            " "
        };
        match d.default_output_config() {
            Ok(c) => println!(
                "{mark} {name}  [{} Hz, {} ch, {:?}, buffer {:?}]",
                c.sample_rate(),
                c.channels(),
                c.sample_format(),
                c.buffer_size()
            ),
            Err(e) => println!("{mark} {name}  [{e}]"),
        }
    }
    Ok(())
}

impl Engine {
    /// Starts the engine; falls back to the backend default buffer if the requested size fails.
    pub fn start(cfg: &EngineConfig, epoch: Instant) -> Result<Self, String> {
        match Self::try_start(cfg, cfg.buffer, epoch) {
            Ok(e) => Ok(e),
            Err(e) if cfg.buffer.is_some() => {
                eprintln!(
                    "stream with fixed buffer failed ({e}); retrying with the default buffer"
                );
                Self::try_start(cfg, None, epoch)
            }
            Err(e) => Err(e),
        }
    }

    fn try_start(cfg: &EngineConfig, buffer: Option<u32>, epoch: Instant) -> Result<Self, String> {
        let host = cpal::default_host();
        let device = pick_device(&host, cfg.device.as_deref())?;
        let supported = device
            .default_output_config()
            .map_err(|e| format!("default output config: {e}"))?;
        let rate = supported.sample_rate();
        let channels = supported.channels();
        let format = supported.sample_format();
        let supported_buffer = match supported.buffer_size() {
            SupportedBufferSize::Range { min, max } => format!("{min}..={max}"),
            SupportedBufferSize::Unknown => "unknown".to_string(),
        };

        let clock = Arc::new(AudioClock::new(rate, epoch));
        let cb_stats = Arc::new(CallbackStats::default());
        let mix_stats = Arc::new(MixerStats::default());
        let loudness = Arc::new(LoudnessStats::default());
        let stop = Arc::new(AtomicBool::new(false));
        let mut threads = Vec::new();

        let tracks = build_tracks(&cfg.specs, rate, cfg.ring_ms, cfg.cheaper_stretch);
        let track_latency = cfg
            .specs
            .iter()
            .zip(&tracks.latencies)
            .map(|(s, l)| (s.name.to_string(), *l))
            .collect();
        let params = tracks.params.clone();

        // Workers (round-robin track assignment).
        let n_workers = cfg.workers.clamp(1, cfg.specs.len().max(1));
        let mut buckets: Vec<Vec<TrackPipeline>> = (0..n_workers).map(|_| Vec::new()).collect();
        for (i, p) in tracks.pipelines.into_iter().enumerate() {
            buckets[i % n_workers].push(p);
        }
        let mut worker_stats = Vec::new();
        for (i, b) in buckets.into_iter().enumerate() {
            let ws = Arc::new(WorkerStats::default());
            threads.push(spawn_worker(
                i,
                b,
                stop.clone(),
                ws.clone(),
                cfg.high_priority_workers,
            ));
            worker_stats.push(ws);
        }

        // Prefill: wait until every ring is (nearly) full before starting the device.
        let inputs = tracks.inputs;
        let deadline = Instant::now() + Duration::from_secs(20);
        let cap = (rate as usize * cfg.ring_ms as usize / 1000) * CHANNELS;
        while inputs
            .iter()
            .any(|t| t.buffered_samples() + 2 * BLOCK_FRAMES * CHANNELS < cap)
        {
            if Instant::now() > deadline {
                stop.store(true, Ordering::Relaxed);
                return Err("workers could not prefill the track rings in 20 s".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }

        // Meter tap (1 s of stereo) and meter thread.
        let (tap_p, tap_c) = RingBuffer::<f32>::new(rate as usize * 2);
        threads.push(spawn_meter(tap_c, rate, loudness.clone(), stop.clone()));

        let limiter = Limiter::new(rate, cfg.limiter_ceiling_db, 1.5, 100.0);
        let limiter_latency = limiter.latency();
        let mixer = Mixer::new(inputs, limiter, Some(tap_p), mix_stats.clone());
        let callback = Callback::new(
            mixer,
            clock.clone(),
            cb_stats.clone(),
            usize::from(channels),
            cfg.latency_comp,
        );

        let config = StreamConfig {
            channels,
            sample_rate: rate,
            buffer_size: buffer.map_or(BufferSize::Default, BufferSize::Fixed),
        };
        let stream = match format {
            SampleFormat::F32 => build::<f32>(&device, config, callback, cb_stats.clone()),
            SampleFormat::F64 => build::<f64>(&device, config, callback, cb_stats.clone()),
            SampleFormat::I16 => build::<i16>(&device, config, callback, cb_stats.clone()),
            SampleFormat::I32 => build::<i32>(&device, config, callback, cb_stats.clone()),
            SampleFormat::U16 => build::<u16>(&device, config, callback, cb_stats.clone()),
            other => Err(format!("unsupported device sample format {other:?}")),
        };
        let stream = stream.and_then(|s| s.play().map(|()| s).map_err(|e| format!("play: {e}")));
        let stream = match stream {
            Ok(s) => s,
            Err(e) => {
                stop.store(true, Ordering::Relaxed);
                for t in threads {
                    let _ = t.join();
                }
                return Err(e);
            }
        };
        let cpal_buffer_size = stream.buffer_size().ok();

        Ok(Self {
            stream,
            clock,
            cb_stats,
            mix_stats,
            loudness,
            worker_stats,
            params,
            info: StreamInfo {
                host: host.id().name().to_string(),
                device: device.to_string(),
                sample_rate: rate,
                channels,
                format: format!("{format:?}"),
                buffer_requested: cfg
                    .buffer
                    .map_or_else(|| "default".into(), |b| b.to_string()),
                buffer_used: buffer.map_or_else(|| "default".into(), |b| format!("fixed {b}")),
                supported_buffer,
                cpal_buffer_size,
                track_latency,
                limiter_latency,
            },
            stop,
            threads,
        })
    }

    /// Stops the stream and joins all engine threads.
    pub fn shutdown(self) {
        let _ = self.stream.pause();
        drop(self.stream);
        self.stop.store(true, Ordering::Relaxed);
        for t in self.threads {
            let _ = t.join();
        }
    }
}

fn build<T>(
    device: &cpal::Device,
    config: StreamConfig,
    mut cb: Callback,
    stats: Arc<CallbackStats>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    device
        .build_output_stream::<T, _, _>(
            config,
            move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
                cb.process(data, info.timestamp())
            },
            move |err: cpal::Error| {
                if err.kind() == ErrorKind::Xrun {
                    stats.xruns.fetch_add(1, Ordering::Relaxed);
                } else {
                    stats.errors.fetch_add(1, Ordering::Relaxed);
                    stats
                        .last_error
                        .store(error_code(err.kind()), Ordering::Relaxed);
                    eprintln!("cpal stream error: {err}");
                }
            },
            None,
        )
        .map_err(|e| format!("build output stream ({config:?}): {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meter::integrated_lufs;
    use crate::scene::{Scene, tracks};
    use cpal::StreamInstant;

    /// Mock sink: drives the real callback with synthetic timestamps; workers pumped inline.
    fn run_mock(
        scene: Scene,
        n_tracks: usize,
        secs: f64,
        buf: usize,
    ) -> (
        Vec<f32>,
        Arc<CallbackStats>,
        Arc<MixerStats>,
        Arc<AudioClock>,
    ) {
        let rate = 48_000;
        let specs = tracks(scene, 1.0, n_tracks);
        let mut t = build_tracks(&specs, rate, 250, false);
        let clock = Arc::new(AudioClock::new(rate, Instant::now()));
        let cb_stats = Arc::new(CallbackStats::default());
        let mix_stats = Arc::new(MixerStats::default());
        let mixer = Mixer::new(
            std::mem::take(&mut t.inputs),
            Limiter::new(rate, -1.0, 1.5, 100.0),
            None,
            mix_stats.clone(),
        );
        let mut cb = Callback::new(mixer, clock.clone(), cb_stats.clone(), 2, true);
        let total = (secs * f64::from(rate)) as usize;
        let mut out = vec![0.0f32; buf * 2];
        let mut all = Vec::with_capacity(total * 2);
        let latency = Duration::from_millis(20);
        let mut done = 0usize;
        while done < total {
            for p in &mut t.pipelines {
                p.pump();
            }
            let callback = StreamInstant::from_secs_f64(1.0 + done as f64 / f64::from(rate));
            let ts = OutputStreamTimestamp {
                callback,
                playback: callback + latency,
            };
            cb.process(&mut out, ts);
            all.extend_from_slice(&out);
            done += buf;
        }
        (all, cb_stats, mix_stats, clock)
    }

    #[test]
    fn mock_sink_8_tracks_no_underruns_no_allocations() {
        // 10 s of the full 8-track scene at 256-frame callbacks (stretch 0.5×–2× active).
        let (_, cb, mix, clock) = run_mock(Scene::Mix, 8, 10.0, 256);
        assert_eq!(cb.rt_allocs.load(Ordering::Relaxed), 0);
        assert_eq!(mix.underrun_events.load(Ordering::Relaxed), 0);
        assert_eq!(cb.glitches.load(Ordering::Relaxed), 0);
        assert_eq!(clock.frames_submitted(), 480_000);
        assert_eq!(cb.latency_ns_last.load(Ordering::Relaxed), 20_000_000);
    }

    #[test]
    fn reference_tone_through_full_chain_reads_minus_23_lufs() {
        // Acceptance: -23 LUFS reference (44.1 kHz source → rubato → mixer → limiter) ±0.1 LU.
        let (all, ..) = run_mock(Scene::RefTone, 1, 20.0, 480);
        let lufs = integrated_lufs(&all, 48_000);
        assert!((lufs + 23.0).abs() <= 0.1, "integrated {lufs} LUFS");
    }

    #[test]
    fn clicks_land_on_the_timeline_through_the_mixer() {
        // Click k must start at device frame k * 48000 (+ limiter look-ahead), ±2 frames.
        let (all, ..) = run_mock(Scene::Sync, 1, 3.2, 256);
        let lat = Limiter::new(48_000, -1.0, 1.5, 100.0).latency();
        for k in 1..=3usize {
            let expected = k * 48_000 + lat;
            let lo = expected - 40;
            let first = (lo..lo + 80)
                .find(|&f| all[f * 2].abs() > 0.05)
                .expect("click present");
            // 2 kHz burst: |x| > 0.05 is reached within ~1 frame of the onset.
            assert!(
                first.abs_diff(expected) <= 2,
                "click {k}: {first} vs {expected}"
            );
        }
    }
}
