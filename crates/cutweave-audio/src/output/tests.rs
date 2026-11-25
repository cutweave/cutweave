// SPDX-License-Identifier: GPL-3.0-or-later
//! Supervisor tests on a fake host: restart on device removal, on a default-device change and on
//! a stalled stream (each within 2 s, clock monotonic throughout), waiting for a device, start
//! failures, pause and seek-while-paused.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration as StdDuration, Instant};

use cutweave_model::{Duration, ItemId, MediaId, SampleRate, Time, TrackId};
use cutweave_render::AudioSegment;

use super::virtual_device::{Faults, VirtualStream};
use super::*;
use crate::{ClockReading, MixerConfig, MixerHandle, PcmFormat, Tone, ToneFactory, ToneSpec};

const RATE: SampleRate = SampleRate::HZ_48_000;

/// Devices a test plugs, unplugs and makes default.
#[derive(Debug, Default)]
struct FakeHost {
    devices: Mutex<BTreeMap<String, Arc<Faults>>>,
    default: Mutex<Option<String>>,
}

impl FakeHost {
    fn with(devices: &[&str], default: Option<&str>) -> Arc<FakeHost> {
        let host = Arc::new(FakeHost::default());
        for d in devices {
            host.plug(d);
        }
        host.set_default(default);
        host
    }

    fn plug(&self, id: &str) {
        self.devices
            .lock()
            .unwrap()
            .insert(id.to_owned(), Arc::new(Faults::default()));
    }

    /// Removes the device: its stream reports a fatal error and stops.
    fn unplug(&self, id: &str) {
        if let Some(f) = self.devices.lock().unwrap().remove(id) {
            f.unplugged.store(true, Ordering::Release);
        }
    }

    fn set_default(&self, id: Option<&str>) {
        *self.default.lock().unwrap() = id.map(str::to_owned);
    }

    fn faults(&self, id: &str) -> Arc<Faults> {
        self.devices.lock().unwrap()[id].clone()
    }
}

struct FakeBackend(Arc<FakeHost>);

impl Backend for FakeBackend {
    fn open(
        &mut self,
        request: &StreamRequest,
        callback: StreamCallback,
        errors: ErrorSink,
    ) -> Result<Box<dyn LiveStream>, OutputError> {
        let id = match &request.device {
            OutputDevice::Default | OutputDevice::Null => self
                .0
                .default
                .lock()
                .unwrap()
                .clone()
                .ok_or(OutputError::NoDevice)?,
            OutputDevice::Named(name) => name.clone(),
        };
        let faults = self
            .0
            .devices
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(|| OutputError::DeviceNotFound(id.clone()))?;
        let info = OutputInfo {
            host: "fake".into(),
            device: id.clone(),
            rate: request.rate,
            channels: 2,
            sample_format: "F32".into(),
            buffer_requested: request.buffer_frames,
            buffer_used: request.buffer_frames,
            period_frames: Some(request.rate.hz() / 200), // 5 ms
        };
        Ok(Box::new(VirtualStream::spawn(
            callback,
            info,
            Some(id),
            Some(faults),
            errors,
        )?))
    }

    fn default_device_id(&mut self) -> Option<String> {
        self.0.default.lock().unwrap().clone()
    }
}

fn mixer() -> (Mixer, MixerHandle) {
    Mixer::new(MixerConfig::new(RATE), Arc::new(ToneFactory::new())).unwrap()
}

fn config(device: OutputDevice) -> OutputConfig {
    let mut c = OutputConfig::new(device);
    c.retry_interval = StdDuration::from_millis(50);
    c.watchdog = StdDuration::from_millis(300);
    c
}

fn start(
    host: &Arc<FakeHost>,
    device: OutputDevice,
) -> Result<(AudioOutput, MixerHandle), StartError> {
    let (m, h) = mixer();
    Ok((start_mixer(host, device, m)?, h))
}

fn start_mixer(
    host: &Arc<FakeHost>,
    device: OutputDevice,
    mixer: Mixer,
) -> Result<AudioOutput, StartError> {
    let host = host.clone();
    AudioOutput::start_with(
        mixer,
        config(device),
        Box::new(move || Box::new(FakeBackend(host)) as Box<dyn Backend>),
    )
}

/// A mixer playing a 60 s tone on one track (so seeks really prime), and its segments.
fn tone_mixer() -> (Mixer, MixerHandle, Vec<AudioSegment>) {
    let mut sources = ToneFactory::new();
    let media = MediaId::from_u128(1);
    sources.insert(
        media,
        ToneSpec::new(
            Tone::Sine {
                freq_hz: 440.0,
                amplitude: 0.1,
                phase: 0.0,
            },
            PcmFormat::new(RATE, 2),
        ),
    );
    let (m, h) = Mixer::new(MixerConfig::new(RATE), Arc::new(sources)).unwrap();
    let segment = AudioSegment {
        item: ItemId::from_u128(1),
        track: TrackId::from_u128(1),
        media,
        stream: Some(0),
        dst_start: Time::ZERO,
        dst_duration: Duration::from_secs(60).unwrap(),
        src_start: Time::ZERO,
        clip_gain_db: 0.0,
        clip_start: Time::ZERO,
        clip_duration: Duration::from_secs(60).unwrap(),
        gain: cutweave_model::Animatable::Static(0.0),
        pan: cutweave_model::Animatable::Static(0.0),
        fade_in: cutweave_model::Fade::default(),
        fade_out: cutweave_model::Fade::default(),
        track_gain_db: 0.0,
        reverse: false,
        speed: (1, 1),
        sample_rate: RATE,
    };
    (m, h, vec![segment])
}

fn wait_until(what: &str, timeout: StdDuration, mut cond: impl FnMut() -> bool) -> StdDuration {
    let t0 = Instant::now();
    while !cond() {
        assert!(
            t0.elapsed() < timeout,
            "timed out after {timeout:?} waiting for {what}"
        );
        std::thread::sleep(StdDuration::from_millis(2));
    }
    t0.elapsed()
}

/// What a [`Watcher`] saw.
#[derive(Debug, Default)]
struct Watched {
    readings: usize,
    /// (time, output state, clock reading) at every change of either state.
    transitions: Vec<(StdDuration, OutputState, ClockReading)>,
    /// Longest time the output was running and playing while the clock was not `Running`.
    longest_not_running: StdDuration,
    /// Readings where the output reported `Running` while the clock was `Stopped`.
    running_while_stopped: usize,
}

impl Watched {
    fn print(&self, label: &str) {
        eprintln!("{label}: {} readings", self.readings);
        for (t, output, clock) in &self.transitions {
            eprintln!(
                "  {:>8.3} s  output {output:?}, clock {:?} at {} (generation {})",
                t.as_secs_f64(),
                clock.state,
                clock.position,
                clock.generation
            );
        }
        eprintln!(
            "  longest playing-but-not-running stretch {:?}; output Running with clock Stopped in {} readings",
            self.longest_not_running, self.running_while_stopped
        );
    }
}

/// Reads the clock and the output state every millisecond on its own thread: checks the clock
/// never goes back within a generation, and records how long the clock stayed off `Running`
/// while the output ran and played.
struct Watcher {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<Watched>>,
}

impl Watcher {
    fn new(out: &AudioOutput) -> Watcher {
        let (clock, shared) = (out.clock(), out.shared.clone());
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        let thread = std::thread::spawn(move || {
            let start = Instant::now();
            let mut w = Watched::default();
            let mut prev: Option<(OutputState, ClockReading)> = None;
            let mut off_since: Option<Instant> = None;
            while !s.load(Ordering::Relaxed) {
                // The output state first: once it reads `Running`, the clock already left
                // `Stopped` (read in the other order, a transition in between looks like one).
                let state = OutputState::from_u8(shared.state.load(Ordering::Acquire));
                let playing = shared.playing.load(Ordering::Acquire);
                let r = clock.read();
                if let Some((p_state, p)) = prev {
                    assert!(
                        r.generation != p.generation || r.position >= p.position,
                        "clock went back: {p:?} -> {r:?}"
                    );
                    if p_state != state || p.state != r.state {
                        w.transitions.push((start.elapsed(), state, r));
                    }
                } else {
                    w.transitions.push((start.elapsed(), state, r));
                }
                if state == OutputState::Running && r.state == ClockState::Stopped {
                    w.running_while_stopped += 1;
                }
                if state == OutputState::Running && playing && !r.is_running() {
                    let since = *off_since.get_or_insert_with(Instant::now);
                    w.longest_not_running = w.longest_not_running.max(since.elapsed());
                } else {
                    off_since = None;
                }
                prev = Some((state, r));
                w.readings += 1;
                std::thread::sleep(StdDuration::from_millis(1));
            }
            w
        });
        Watcher {
            stop,
            thread: Some(thread),
        }
    }

    fn finish(mut self) -> Watched {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap()
    }
}
/// After every (re)start while playing, the clock is `Running` again within 1 s of the output
/// running, and the output never reports `Running` while the clock is `Stopped`.
fn assert_recovered(w: &Watched) {
    assert!(
        w.longest_not_running < StdDuration::from_secs(1),
        "the clock stayed off Running for {:?} while the output ran and played",
        w.longest_not_running
    );
    assert_eq!(
        w.running_while_stopped, 0,
        "the output reported Running while the clock was Stopped"
    );
}

fn running(out: &AudioOutput) -> bool {
    out.stats().state == OutputState::Running && out.clock().is_running()
}

fn device(out: &AudioOutput) -> Option<String> {
    out.info().map(|i| i.device)
}

#[test]
fn restarts_on_device_removal_within_two_seconds() {
    let host = FakeHost::with(&["A", "B"], Some("A"));
    let (out, _h) = start(&host, OutputDevice::Default).unwrap();
    out.play();
    wait_until("running", StdDuration::from_secs(2), || running(&out));
    assert_eq!(device(&out).as_deref(), Some("A"));
    let watcher = Watcher::new(&out);
    std::thread::sleep(StdDuration::from_millis(100));
    let before = out.clock().position();

    host.unplug("A");
    host.set_default(Some("B"));
    let took = wait_until("restart on B", StdDuration::from_secs(2), || {
        running(&out) && device(&out).as_deref() == Some("B")
    });
    std::thread::sleep(StdDuration::from_millis(100));
    let seen = watcher.finish();
    seen.print("device removal");
    assert_recovered(&seen);
    let stats = out.stats();
    eprintln!("device removal: restarted on B after {took:?}; {stats:?}");
    assert_eq!(stats.restarts, 1);
    // The removal is reported as a stream error (unless the default-device poll got there first).
    assert!(stats.stream_errors <= 1);
    assert!(out.clock().position() > before);
    assert!(
        seen.transitions.iter().all(|(_, _, r)| r.generation == 0),
        "a restart is not a seek"
    );
    assert!(
        seen.transitions
            .iter()
            .any(|(_, _, r)| r.state != ClockState::Running)
    );
}

#[test]
fn restarts_when_the_default_device_changes() {
    let host = FakeHost::with(&["A", "B"], Some("A"));
    let (out, _h) = start(&host, OutputDevice::Default).unwrap();
    out.play();
    wait_until("running", StdDuration::from_secs(2), || running(&out));
    let watcher = Watcher::new(&out);
    // No stream error: only the default changes (the supervisor polls it).
    host.set_default(Some("B"));
    let took = wait_until("restart on B", StdDuration::from_secs(2), || {
        running(&out) && device(&out).as_deref() == Some("B")
    });
    let seen = watcher.finish();
    seen.print("default change");
    assert_recovered(&seen);
    eprintln!("default change: restarted on B after {took:?}");
    assert_eq!(out.stats().restarts, 1);
    assert_eq!(out.stats().stream_errors, 0);
}

#[test]
fn a_named_device_is_not_moved_to_the_new_default() {
    let host = FakeHost::with(&["A", "B"], Some("A"));
    let (out, _h) = start(&host, OutputDevice::Named("A".into())).unwrap();
    out.play();
    wait_until("running", StdDuration::from_secs(2), || running(&out));
    host.set_default(Some("B"));
    std::thread::sleep(StdDuration::from_millis(700));
    assert_eq!(out.stats().restarts, 0);
    assert_eq!(device(&out).as_deref(), Some("A"));
}

#[test]
fn restarts_a_stalled_stream() {
    let host = FakeHost::with(&["A"], Some("A"));
    let (out, _h) = start(&host, OutputDevice::Default).unwrap();
    out.play();
    wait_until("running", StdDuration::from_secs(2), || running(&out));
    let watcher = Watcher::new(&out);
    let faults = host.faults("A");
    faults.stalled.store(true, Ordering::Release);
    wait_until("watchdog restart", StdDuration::from_secs(2), || {
        out.stats().restarts >= 1
    });
    faults.stalled.store(false, Ordering::Release);
    wait_until("running again", StdDuration::from_secs(2), || running(&out));
    let seen = watcher.finish();
    seen.print("stalled stream");
    assert_eq!(seen.running_while_stopped, 0);
    assert_eq!(out.stats().stream_errors, 0);
}

#[test]
fn waits_for_a_device_then_resumes() {
    let host = FakeHost::with(&["A"], Some("A"));
    let (out, _h) = start(&host, OutputDevice::Default).unwrap();
    out.play();
    wait_until("running", StdDuration::from_secs(2), || running(&out));
    let watcher = Watcher::new(&out);
    host.unplug("A");
    host.set_default(None);
    wait_until("waiting", StdDuration::from_secs(2), || {
        out.stats().state == OutputState::WaitingForDevice
    });
    let held = out.clock().read();
    assert_eq!(held.state, ClockState::Stopped);
    std::thread::sleep(StdDuration::from_millis(150));
    assert_eq!(
        out.clock().position(),
        held.position,
        "a stopped clock holds"
    );
    assert!(out.info().is_none());
    assert!(out.stats().last_error.is_some());

    host.plug("C");
    host.set_default(Some("C"));
    let took = wait_until("running on C", StdDuration::from_secs(2), || {
        running(&out) && device(&out).as_deref() == Some("C")
    });
    let seen = watcher.finish();
    seen.print("device came back");
    assert_recovered(&seen);
    eprintln!("device came back after {took:?}");
    assert!(out.clock().position() > held.position);
}

/// The WP-1G.02 follow-up: after any restart while playing, however restarts land, the clock
/// returns to `Running`. Seven restarts in a few seconds: a plain one, one before the reopened
/// stream's first callback (devices that take 40 ms to start), one while the mixer primes after
/// a seek, a default-device change, a device removal, and one while paused.
#[test]
fn back_to_back_restarts_always_return_the_clock_to_running() {
    let host = FakeHost::with(&["A", "B", "C"], Some("A"));
    host.faults("A").start_delay_ms.store(40, Ordering::Release);
    host.faults("B").start_delay_ms.store(20, Ordering::Release);
    let (mixer, handle, segments) = tone_mixer();
    let out = start_mixer(&host, OutputDevice::Default, mixer).unwrap();
    handle.set_segments(&segments).unwrap();
    handle.seek(Time::ZERO);
    out.play();
    wait_until("running", StdDuration::from_secs(2), || running(&out));
    let watcher = Watcher::new(&out);
    let t0 = Instant::now();
    let restarted = |n: u64| {
        wait_until(&format!("restart {n}"), StdDuration::from_secs(2), || {
            out.stats().restarts >= n && running(&out)
        })
    };

    out.restart();
    restarted(1);
    // A restart before the reopened stream's first callback.
    out.restart();
    wait_until("reopened", StdDuration::from_secs(2), || {
        out.stats().restarts >= 2 && out.info().is_some()
    });
    out.restart();
    restarted(3);
    // A restart while the mixer primes after a seek.
    handle.seek(Time::from_secs(5).unwrap());
    out.restart();
    restarted(4);
    // The default device changes (found by the poll), then the device in use disappears.
    host.set_default(Some("B"));
    restarted(5);
    host.unplug("B");
    host.set_default(Some("C"));
    restarted(6);
    assert_eq!(device(&out).as_deref(), Some("C"));
    // A restart while paused: the clock holds Paused, then runs on play.
    out.pause();
    out.restart();
    wait_until("restart 7, paused", StdDuration::from_secs(2), || {
        out.stats().restarts >= 7
            && out.stats().state == OutputState::Running
            && out.clock().state() == ClockState::Paused
    });
    out.play();
    wait_until("running after play", StdDuration::from_secs(1), || {
        out.clock().is_running()
    });
    let elapsed = t0.elapsed();
    std::thread::sleep(StdDuration::from_millis(50));
    let seen = watcher.finish();
    seen.print("back-to-back restarts");
    assert_recovered(&seen);
    assert_eq!(out.stats().restarts, 7);
    assert!(elapsed < StdDuration::from_secs(6), "{elapsed:?}");
    assert!(out.clock().position() > Time::from_secs(5).unwrap());
}

#[test]
fn start_without_a_device_fails_and_returns_the_mixer() {
    let host = FakeHost::with(&[], None);
    let err = start(&host, OutputDevice::Default).unwrap_err();
    assert_eq!(err.error, OutputError::NoDevice);
    let mixer = err.mixer.expect("the mixer comes back");
    assert_eq!(mixer.position(), 0);
    let err = start(&host, OutputDevice::Named("X".into())).unwrap_err();
    assert_eq!(err.error, OutputError::DeviceNotFound("X".into()));
    assert!(err.mixer.is_some());
}

#[test]
fn pause_holds_and_a_seek_while_paused_moves_the_clock() {
    let host = FakeHost::with(&["A"], Some("A"));
    let (out, handle) = start(&host, OutputDevice::Default).unwrap();
    let clock = out.clock();
    // Starts paused at 0.
    wait_until("paused", StdDuration::from_secs(2), || {
        clock.state() == ClockState::Paused
    });
    assert_eq!(clock.position(), Time::ZERO);
    out.play();
    wait_until("running", StdDuration::from_secs(2), || running(&out));
    std::thread::sleep(StdDuration::from_millis(100));
    out.pause();
    wait_until("paused", StdDuration::from_secs(2), || {
        clock.state() == ClockState::Paused
    });
    let held = clock.read();
    assert!(held.position > Time::ZERO);
    std::thread::sleep(StdDuration::from_millis(50));
    assert_eq!(clock.read(), held);

    let two = Time::from_secs(2).unwrap();
    handle.seek(two);
    wait_until("seek applied", StdDuration::from_secs(2), || {
        clock.generation() == held.generation + 1
    });
    assert_eq!(clock.position(), two);
    assert_eq!(clock.state(), ClockState::Paused);
    out.play();
    wait_until("running", StdDuration::from_secs(2), || clock.is_running());
    std::thread::sleep(StdDuration::from_millis(100));
    let p = clock.position();
    assert!(p > two && p < Time::from_secs(3).unwrap(), "{p}");
    // Stop hands the mixer back where it was.
    let mixer = out.stop().unwrap();
    assert!(mixer.position() >= RATE.time_to_samples(p, cutweave_model::Rounding::Floor));
    assert_eq!(clock.state(), ClockState::Stopped);
}

#[test]
fn null_output_runs_at_the_wall_clock_rate() {
    let (m, _h) = mixer();
    let out = AudioOutput::start(m, OutputConfig::null()).unwrap();
    let info = out.info().unwrap();
    assert_eq!(info.host, "null");
    assert_eq!(info.period_frames, Some(480));
    out.play();
    let clock = out.clock();
    wait_until("running", StdDuration::from_secs(2), || clock.is_running());
    let (t0, p0) = (Instant::now(), clock.position());
    std::thread::sleep(StdDuration::from_millis(300));
    let (t1, p1) = (Instant::now(), clock.position());
    let audio = p1.as_secs_f64() - p0.as_secs_f64();
    let wall = (t1 - t0).as_secs_f64();
    assert!(
        (audio - wall).abs() < 0.03,
        "audio {audio} s vs wall {wall} s"
    );
    assert_eq!(out.stats().restarts, 0);
}

#[test]
fn env_values_pick_the_output() {
    assert_eq!(OutputDevice::parse("null"), OutputDevice::Null);
    assert_eq!(OutputDevice::parse(" NULL "), OutputDevice::Null);
    assert_eq!(OutputDevice::parse(""), OutputDevice::Default);
    assert_eq!(OutputDevice::parse("default"), OutputDevice::Default);
    assert_eq!(
        OutputDevice::parse("Speakers (Realtek)"),
        OutputDevice::Named("Speakers (Realtek)".into())
    );
}
