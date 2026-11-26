// SPDX-License-Identifier: GPL-3.0-or-later
//! Audio output and master clock (WP-1G.02).
//!
//! * The null output (`CUTWEAVE_AUDIO=null`, CI): the 8-source mix plays in real time with an
//!   allocation-free callback, 0 underruns, a monotonic clock that keeps wall-clock time, and
//!   meter frames for the master and every bus.
//! * A real device, when the machine has one (skipped cleanly otherwise): a short smoke test, a
//!   stream restart on the real backend, the 10-minute soak (`#[ignore]`; run with
//!   `cargo nextest run -p cutweave-audio --run-ignored only device_soak`,
//!   `CUTWEAVE_AUDIO_SOAK_SECS` overrides the length) that measures underruns and the drift of
//!   the audio clock against `Instant`, and the manual device-change check
//!   (`device_change_manual`, see its docs).
//!
//! Device runs are silent (master gain −∞) unless `CUTWEAVE_AUDIO_AUDIBLE=1`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

mod common;

use std::time::{Duration as StdDuration, Instant};

use common::{eight_sources, segments};
use cutweave_audio::rt_alloc::CountingAlloc;
use cutweave_audio::{
    AUDIO_ENV, AudioClock, AudioOutput, ClockReading, ClockState, Mixer, MixerConfig, MixerHandle,
    OutputConfig, OutputDevice, OutputError, OutputInfo, OutputState, OutputStats,
};
use cutweave_model::Time;

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

/// The 8-source tone mix playing on an output, clock running.
struct Playing {
    output: AudioOutput,
    handle: MixerHandle,
    clock: AudioClock,
}

impl Playing {
    /// Starts `config`'s output with `seconds` (plus margin) of the 8-source mix and plays it
    /// from 0. `Err` if the output could not start.
    fn start(config: OutputConfig, seconds: f64, audible: bool) -> Result<Playing, OutputError> {
        let rate = AudioOutput::preferred_rate(&config)?;
        let (sources, segs) = eight_sources(seconds + 5.0);
        let (mixer, handle) = Mixer::new(MixerConfig::new(rate), sources).unwrap();
        if !audible {
            handle.set_master_gain(f64::NEG_INFINITY);
        }
        let output = AudioOutput::start(mixer, config).map_err(|e| e.error)?;
        let clock = output.clock();
        handle.set_segments(&segments(&segs)).unwrap();
        handle.seek(Time::ZERO);
        let t0 = Instant::now();
        // The seek applies while paused (the callback keeps taking plans).
        while !handle.stats().settled() || clock.generation() == 0 {
            assert!(
                t0.elapsed() < StdDuration::from_secs(5),
                "the seek never applied"
            );
            std::thread::sleep(StdDuration::from_millis(1));
        }
        assert_eq!(clock.position(), Time::ZERO);
        output.play();
        while !clock.is_running() {
            assert!(
                t0.elapsed() < StdDuration::from_secs(10),
                "the clock never ran"
            );
            std::thread::sleep(StdDuration::from_millis(1));
        }
        Ok(Playing {
            output,
            handle,
            clock,
        })
    }

    /// Stops the output (mixer first, then its workers) and checks nothing leaked.
    fn finish(self) {
        drop(self.output.stop().expect("the mixer comes back"));
        let stats = self.handle.stats();
        assert_eq!(stats.leaked_plans, 0);
        assert_eq!(stats.source_errors, 0);
    }
}

fn audible() -> bool {
    std::env::var("CUTWEAVE_AUDIO_AUDIBLE").is_ok_and(|v| v == "1")
}

/// What a playback run measured.
#[derive(Debug)]
struct Report {
    info: Option<OutputInfo>,
    stats: OutputStats,
    seconds: f64,
    /// Lane frames the mixer played as silence (late rings).
    underrun_frames: u64,
    /// Clock readings that went back within a generation (must be 0).
    backward_reads: u64,
    clock_reads: u64,
    /// Audio clock advance minus `Instant` advance over the measured span (ms).
    drift_ms: f64,
    /// Least-squares slope of (audio − wall) over the run (parts per million).
    drift_ppm: f64,
    /// Largest deviation of a reading from that fit (ms): jitter and steps.
    max_residual_ms: f64,
    /// Smallest look-ahead a lane ring had (ms).
    min_lookahead_ms: f64,
}

impl Report {
    fn print(&self, label: &str) {
        let s = &self.stats;
        eprintln!(
            "{label}: {:.1} s on {:?}\n  callbacks {} (frames per callback {:?}), latency {:?}, \
             max callback {:?}\n  underrun frames {}, xruns {}, stream errors {}, restarts {}, \
             RT heap ops {}\n  clock: {} reads, {} backward, drift {:+.3} ms ({:+.2} ppm), \
             max residual {:.3} ms; min lane look-ahead {:.1} ms",
            self.seconds,
            self.info,
            s.callbacks,
            s.callback_frames,
            s.latency,
            s.max_callback_time,
            self.underrun_frames,
            s.xruns,
            s.stream_errors,
            s.restarts,
            s.rt_heap_ops,
            self.clock_reads,
            self.backward_reads,
            self.drift_ms,
            self.drift_ppm,
            self.max_residual_ms,
            self.min_lookahead_ms,
        );
    }
}

/// Least-squares line through `(x, y)`: (slope, intercept).
fn fit(points: &[(f64, f64)]) -> (f64, f64) {
    let n = points.len() as f64;
    let (sx, sy) = points
        .iter()
        .fold((0.0, 0.0), |(a, b), (x, y)| (a + x, b + y));
    let (mx, my) = (sx / n, sy / n);
    let (mut sxy, mut sxx) = (0.0, 0.0);
    for (x, y) in points {
        sxy += (x - mx) * (y - my);
        sxx += (x - mx) * (x - mx);
    }
    let slope = if sxx > 0.0 { sxy / sxx } else { 0.0 };
    (slope, my - slope * mx)
}

/// Plays the 8-source tone mix for `seconds` on `config`'s output and measures underruns, the
/// clock's monotonicity and its drift against `Instant`.
fn play_mix(config: OutputConfig, seconds: f64) -> Result<Report, OutputError> {
    let run = Playing::start(config, seconds, audible())?;
    let clock = &run.clock;
    // Skip the start-up transient (S2 finding 3) before measuring.
    std::thread::sleep(StdDuration::from_millis(500));
    run.handle.reset_lookahead();

    let start = Instant::now();
    let first = clock.read();
    let mut prev = first;
    let (mut reads, mut backward) = (0u64, 0u64);
    let mut points = Vec::new();
    let mut next_point = start;
    let mut last = (start, first.position);
    while start.elapsed().as_secs_f64() < seconds {
        let now = Instant::now();
        let r = clock.read();
        reads += 1;
        if r.generation == prev.generation && r.position < prev.position {
            backward += 1;
        }
        prev = r;
        if now >= next_point && r.state == ClockState::Running {
            let wall = now.duration_since(start).as_secs_f64();
            let audio = r.position.as_secs_f64() - first.position.as_secs_f64();
            points.push((wall, audio - wall));
            last = (now, r.position);
            next_point += StdDuration::from_millis(100);
        }
        std::thread::sleep(StdDuration::from_millis(1));
    }
    let wall = last.0.duration_since(start).as_secs_f64();
    let audio = last.1.as_secs_f64() - first.position.as_secs_f64();
    let (slope, intercept) = fit(&points);
    let max_residual = points
        .iter()
        .map(|(x, y)| (y - (slope * x + intercept)).abs())
        .fold(0.0, f64::max);
    let mix = run.handle.stats();
    let rate = run.output.rate();
    let report = Report {
        info: run.output.info(),
        stats: run.output.stats(),
        seconds: wall,
        underrun_frames: mix.underrun_frames,
        backward_reads: backward,
        clock_reads: reads,
        drift_ms: (audio - wall) * 1e3,
        drift_ppm: slope * 1e6,
        max_residual_ms: max_residual * 1e3,
        min_lookahead_ms: mix
            .min_lookahead_frames
            .map_or(f64::NAN, |f| f as f64 * 1e3 / f64::from(rate.hz())),
    };
    run.finish();
    Ok(report)
}

#[test]
fn null_output_plays_the_mix_in_real_time() {
    let report = play_mix(OutputConfig::null(), 2.0).unwrap();
    report.print("null output");
    assert_eq!(report.stats.rt_heap_ops, 0, "the callback touched the heap");
    assert_eq!(report.underrun_frames, 0);
    assert_eq!(report.backward_reads, 0);
    assert_eq!(report.stats.restarts, 0);
    assert!(report.stats.callbacks > 100);
    // A wall-clock-driven clock: within a few periods of `Instant` over the run.
    assert!(report.drift_ms.abs() < 30.0, "{report:?}");
}

#[test]
fn meters_report_the_master_and_every_bus() {
    let run = Playing::start(OutputConfig::null(), 1.0, true).unwrap();
    std::thread::sleep(StdDuration::from_millis(500));
    let mut frames = Vec::new();
    run.output.drain_meters(&mut frames);
    // ~30 Hz.
    assert!((10..=20).contains(&frames.len()), "{} frames", frames.len());
    assert!(frames.windows(2).all(|w| w[0].at < w[1].at));
    // Every track plays a tone of amplitude 0.05 (0.1 for the impulse track, one impulse per
    // 100 ms) at −1 dB clip and −2 dB track gain: each bus peaks above 0 and below 0.1 (a
    // 5.1 source down-mixes to about 2.4× its amplitude); the master sums them.
    let mut tracks = std::collections::BTreeMap::new();
    for f in &frames {
        assert_eq!(f.buses().len(), 8);
        assert!(f.peak[0] > 0.0 && f.peak[0] < 1.0, "{f:?}");
        for bus in f.buses() {
            let p: &mut f32 = tracks.entry(bus.track).or_default();
            *p = p.max(bus.peak[0]);
        }
    }
    assert_eq!(tracks.len(), 8);
    for (track, peak) in tracks {
        assert!(peak > 0.0 && peak < 0.1, "{track}: {peak}");
    }
    assert!(run.output.stats().master_peak[0] > 0.0);
    // WP-2E.02: RMS under the peak, a quiet mix the limiter leaves alone, and loudness once
    // 400 ms have played (integrated since the start; the mix is about −20 LUFS).
    let last = frames.last().unwrap();
    assert!(last.rms[0] > 0.0 && last.rms[0] <= last.peak[0], "{last:?}");
    assert!(last.true_peak >= last.peak[0].max(last.peak[1]) * 0.99);
    assert!(frames.iter().all(|f| f.limiter_gain >= 1.0), "no limiting");
    assert!(
        frames
            .iter()
            .all(|f| f.buses().iter().all(|b| b.rms[0] <= b.peak[0]))
    );
    let l = last.loudness;
    assert!(l.momentary > -40.0 && l.momentary < -5.0, "{l:?}");
    assert!(l.integrated > -40.0 && l.integrated < -5.0, "{l:?}");
    run.finish();
}

/// The device output, unless the machine has none (or `CUTWEAVE_AUDIO=null` is set).
fn device_config() -> Option<OutputConfig> {
    let config = OutputConfig::from_env();
    if config.device == OutputDevice::Null {
        eprintln!("skipped: {AUDIO_ENV}=null");
        return None;
    }
    match AudioOutput::preferred_rate(&config) {
        Ok(_) => Some(config),
        Err(e) => {
            eprintln!("skipped: no audio device ({e})");
            None
        }
    }
}

fn skip_without_device<T>(result: Result<T, OutputError>) -> Option<T> {
    match result {
        Ok(r) => Some(r),
        Err(e @ (OutputError::NoDevice | OutputError::NoBackend)) => {
            eprintln!("skipped: {e}");
            None
        }
        Err(e) => panic!("the device output failed: {e}"),
    }
}

/// A short real-device run (skips cleanly without a device).
#[test]
fn device_smoke() {
    let Some(config) = device_config() else {
        return;
    };
    let Some(report) = skip_without_device(play_mix(config, 3.0)) else {
        return;
    };
    report.print("device smoke");
    let devices = cutweave_audio::output_devices().unwrap();
    eprintln!("output devices: {devices:#?}");
    assert!(
        devices.iter().any(|d| d.is_default),
        "no default device listed"
    );
    assert_eq!(report.stats.rt_heap_ops, 0, "the callback touched the heap");
    assert_eq!(report.backward_reads, 0);
    assert!(report.stats.callbacks > 0);
    // The clock follows the device: within 1 % of `Instant` over 3 s (devices drift by ppm).
    assert!(report.drift_ms.abs() < 30.0, "{report:?}");
}

/// What [`watch`] saw.
#[derive(Debug, Default)]
struct Watched {
    /// Every stretch in which the output was not running or the clock was not `Running`.
    outages: Vec<StdDuration>,
    /// Longest stretch in which the output reported `Running` (the mix plays) but the clock was
    /// not `Running`.
    longest_not_running: StdDuration,
    /// Readings where the output reported `Running` while the clock was `Stopped`.
    running_while_stopped: usize,
}

impl Watched {
    /// After every (re)start the clock runs again within 1 s of the output running, and the
    /// output never reports `Running` while the clock is `Stopped` (WP-1G.02 follow-up).
    fn assert_clock_recovers(&self) {
        assert!(
            self.longest_not_running < StdDuration::from_secs(1),
            "the clock stayed off Running for {:?} while the output ran",
            self.longest_not_running
        );
        assert_eq!(
            self.running_while_stopped, 0,
            "the output reported Running while the clock was Stopped"
        );
    }
}

/// Watches the output state and the clock until `until` returns true or `limit` passes (the
/// mix plays throughout): prints every change of either state, checks the clock never goes back
/// within a generation, and measures outages and how long the clock stayed off `Running` while
/// the output ran.
fn watch(run: &Playing, limit: StdDuration, mut until: impl FnMut(&Playing) -> bool) -> Watched {
    let start = Instant::now();
    let mut w = Watched::default();
    let mut prev: Option<(OutputState, ClockReading)> = None;
    let mut down_since: Option<Instant> = None;
    let mut off_since: Option<Instant> = None;
    while start.elapsed() < limit && !until(run) {
        // The output state first: once it reads `Running`, the clock already left `Stopped`.
        let state = run.output.stats().state;
        let r = run.clock.read();
        if let Some((_, p)) = prev {
            assert!(
                r.generation != p.generation || r.position >= p.position,
                "the clock went back: {p:?} -> {r:?}"
            );
        }
        if prev.is_none_or(|(s, p)| s != state || p.state != r.state) {
            eprintln!(
                "{:>8.3} s  output {state:?}, clock {:?} at {} (generation {}), device {:?}",
                start.elapsed().as_secs_f64(),
                r.state,
                r.position,
                r.generation,
                run.output.info().map(|i| i.device)
            );
        }
        prev = Some((state, r));
        let output_up = state == OutputState::Running;
        if output_up && r.state == ClockState::Stopped {
            w.running_while_stopped += 1;
        }
        if output_up && !r.is_running() {
            let since = *off_since.get_or_insert_with(Instant::now);
            w.longest_not_running = w.longest_not_running.max(since.elapsed());
        } else {
            off_since = None;
        }
        match (output_up && r.is_running(), down_since) {
            (false, None) => down_since = Some(Instant::now()),
            (true, Some(t)) => {
                w.outages.push(t.elapsed());
                down_since = None;
            }
            _ => {}
        }
        std::thread::sleep(StdDuration::from_millis(1));
    }
    if let Some(t) = down_since {
        w.outages.push(t.elapsed());
    }
    w
}
/// A stream restart on the real backend (the device-change path without unplugging anything):
/// the mixer comes back from the cpal thread, a new stream opens, the clock resumes without
/// going back, well within 2 s.
#[test]
fn device_restart_recovers() {
    let Some(config) = device_config() else {
        return;
    };
    let Some(run) = skip_without_device(Playing::start(config, 4.0, audible())) else {
        return;
    };
    std::thread::sleep(StdDuration::from_millis(300));
    let before = run.clock.position();
    run.output.restart();
    let mut restarted = false;
    let seen = watch(&run, StdDuration::from_secs(3), |r| {
        restarted |= r.output.stats().restarts == 1;
        restarted && r.clock.is_running() && r.clock.position() > before
    });
    let stats = run.output.stats();
    eprintln!("restart on {:?}: {seen:?}; {stats:?}", run.output.info());
    assert_eq!(stats.restarts, 1);
    assert_eq!(stats.rt_heap_ops, 0);
    assert!(
        seen.outages.iter().all(|d| *d < StdDuration::from_secs(2)),
        "{seen:?}"
    );
    seen.assert_clock_recovers();
    assert!(run.clock.position() > before);
    run.finish();
}

/// WBS 1G.02 acceptance on a machine with an audio device: 10 minutes of the 8-source mix at
/// 256-frame requests, 0 underruns, and the drift of the audio clock against `Instant`.
///
/// The clock follows the device's sample clock (arch §9: the device is the master), so its
/// drift against `Instant` is the device crystal's offset from the system timer: +11 ppm on the
/// S2 machine, i.e. about 6.6 ms per 10 minutes, over the WBS's 5 ms (S2 finding 4: devices are
/// typically ±20–100 ppm). The test therefore asserts what the clock itself controls — it never
/// goes back and tracks the device's linear progress within 2 ms (residual around the fit) —
/// bounds the device offset at 100 ppm, and prints the drift against the 5 ms budget.
#[test]
#[ignore = "10-minute real-device soak; run by hand or nightly on a machine with audio"]
fn device_soak() {
    let Some(config) = device_config() else {
        return;
    };
    let seconds = std::env::var("CUTWEAVE_AUDIO_SOAK_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(600.0);
    let report = play_mix(config, seconds).expect("the device output failed");
    report.print("device soak");
    assert_eq!(report.stats.rt_heap_ops, 0, "the callback touched the heap");
    assert_eq!(report.underrun_frames, 0, "underruns");
    assert_eq!(report.backward_reads, 0);
    assert_eq!(report.stats.restarts, 0);
    let budget_ms = 5.0 * seconds / 600.0;
    eprintln!(
        "drift against Instant: {:+.3} ms over {seconds} s ({:+.2} ppm); WBS budget {budget_ms:.3} ms: {}",
        report.drift_ms,
        report.drift_ppm,
        if report.drift_ms.abs() < budget_ms {
            "within"
        } else {
            "EXCEEDED by the device crystal (not by the clock: see the residual)"
        }
    );
    assert!(
        report.max_residual_ms < 2.0,
        "the clock strays {:.3} ms from the device's progress",
        report.max_residual_ms
    );
    assert!(
        report.drift_ppm.abs() < 100.0,
        "device clock {:+.2} ppm against Instant",
        report.drift_ppm
    );
}

/// Manual hardware check (WBS 1G.02): device changes during playback.
///
/// ```text
/// CUTWEAVE_AUDIO_AUDIBLE=1 CUTWEAVE_AUDIO_MANUAL_SECS=60 \
///   cargo nextest run -p cutweave-audio --run-ignored only device_change_manual --no-capture
/// ```
///
/// While the tone mix plays (60 s by default), change the output at least twice:
/// 1. switch the system default output device (Windows: Settings > System > Sound > Output, or
///    the taskbar volume flyout; macOS: System Settings > Sound > Output);
/// 2. unplug the device in use (USB/Bluetooth headset, or disable it in the Sound control
///    panel), wait a few seconds, plug it back in.
///
/// Expected: the sound moves to the new default within 2 s each time and plays on; while no
/// device exists the output waits and resumes when one appears. The test prints every change of
/// the output or clock state and fails if an outage with a device present lasted 2 s or more,
/// if the clock ever went back, if it stayed off `Running` for 1 s or more while the output ran
/// (or was `Stopped` while the output reported `Running`), or if nothing restarted.
#[test]
#[ignore = "manual: change or unplug the output device while it plays"]
fn device_change_manual() {
    let Some(config) = device_config() else {
        return;
    };
    let seconds = std::env::var("CUTWEAVE_AUDIO_MANUAL_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(60.0);
    let run = Playing::start(config, seconds, audible()).expect("the device output failed");
    eprintln!("playing for {seconds} s: change the default output device and unplug it now");
    let seen = watch(&run, StdDuration::from_secs_f64(seconds), |_| false);
    let stats = run.output.stats();
    eprintln!("{seen:?}\n{stats:?}");
    assert!(stats.restarts >= 1, "no device change happened");
    assert_eq!(run.output.stats().state, OutputState::Running);
    assert!(
        run.clock.is_running(),
        "the clock is not running at the end"
    );
    assert!(
        seen.outages.iter().all(|d| *d < StdDuration::from_secs(2)),
        "an outage lasted 2 s or more: {:?} (expected only while no device existed)",
        seen.outages
    );
    seen.assert_clock_recovers();
    run.finish();
}
