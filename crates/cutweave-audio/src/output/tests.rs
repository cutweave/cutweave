// SPDX-License-Identifier: GPL-3.0-or-later
//! Supervisor tests on a fake host: restart on device removal, on a default-device change and on
//! a stalled stream (each within 2 s, clock monotonic throughout), waiting for a device, start
//! failures, pause and seek-while-paused.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration as StdDuration, Instant};

use cutweave_model::{SampleRate, Time};

use super::virtual_device::{Faults, VirtualStream};
use super::*;
use crate::{ClockReading, MixerConfig, MixerHandle, ToneFactory};

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
    let host = host.clone();
    let out = AudioOutput::start_with(
        m,
        config(device),
        Box::new(move || Box::new(FakeBackend(host)) as Box<dyn Backend>),
    )?;
    Ok((out, h))
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

/// Reads the clock every millisecond on its own thread and checks it never goes back within a
/// generation.
struct Watcher {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<Vec<ClockReading>>>,
}

impl Watcher {
    fn new(clock: AudioClock) -> Watcher {
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        let thread = std::thread::spawn(move || {
            let mut seen: Vec<ClockReading> = Vec::new();
            while !s.load(Ordering::Relaxed) {
                let r = clock.read();
                if let Some(prev) = seen.last() {
                    assert!(
                        r.generation != prev.generation || r.position >= prev.position,
                        "clock went back: {prev:?} -> {r:?}"
                    );
                }
                seen.push(r);
                std::thread::sleep(StdDuration::from_millis(1));
            }
            seen
        });
        Watcher {
            stop,
            thread: Some(thread),
        }
    }

    fn finish(mut self) -> Vec<ClockReading> {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap()
    }
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
    let watcher = Watcher::new(out.clock());
    std::thread::sleep(StdDuration::from_millis(100));
    let before = out.clock().position();

    host.unplug("A");
    host.set_default(Some("B"));
    let took = wait_until("restart on B", StdDuration::from_secs(2), || {
        running(&out) && device(&out).as_deref() == Some("B")
    });
    std::thread::sleep(StdDuration::from_millis(100));
    let seen = watcher.finish();
    let stats = out.stats();
    eprintln!("device removal: restarted on B after {took:?}; {stats:?}");
    assert_eq!(stats.restarts, 1);
    // The removal is reported as a stream error (unless the default-device poll got there first).
    assert!(stats.stream_errors <= 1);
    assert!(out.clock().position() > before);
    assert!(
        seen.iter().all(|r| r.generation == 0),
        "a restart is not a seek"
    );
    assert!(
        seen.iter()
            .any(|r| r.state == ClockState::Stopped || r.state == ClockState::Priming)
    );
}

#[test]
fn restarts_when_the_default_device_changes() {
    let host = FakeHost::with(&["A", "B"], Some("A"));
    let (out, _h) = start(&host, OutputDevice::Default).unwrap();
    out.play();
    wait_until("running", StdDuration::from_secs(2), || running(&out));
    let watcher = Watcher::new(out.clock());
    // No stream error: only the default changes (the supervisor polls it).
    host.set_default(Some("B"));
    let took = wait_until("restart on B", StdDuration::from_secs(2), || {
        running(&out) && device(&out).as_deref() == Some("B")
    });
    watcher.finish();
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
    let watcher = Watcher::new(out.clock());
    let faults = host.faults("A");
    faults.stalled.store(true, Ordering::Release);
    wait_until("watchdog restart", StdDuration::from_secs(2), || {
        out.stats().restarts >= 1
    });
    faults.stalled.store(false, Ordering::Release);
    wait_until("running again", StdDuration::from_secs(2), || running(&out));
    watcher.finish();
    assert_eq!(out.stats().stream_errors, 0);
}

#[test]
fn waits_for_a_device_then_resumes() {
    let host = FakeHost::with(&["A"], Some("A"));
    let (out, _h) = start(&host, OutputDevice::Default).unwrap();
    out.play();
    wait_until("running", StdDuration::from_secs(2), || running(&out));
    let watcher = Watcher::new(out.clock());
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
    watcher.finish();
    eprintln!("device came back after {took:?}");
    assert!(out.clock().position() > held.position);
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
