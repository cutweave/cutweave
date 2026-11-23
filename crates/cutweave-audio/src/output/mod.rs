// SPDX-License-Identifier: GPL-3.0-or-later
//! The audio output (WP-1G.02, arch §9): [`AudioOutput`] plays a [`Mixer`] on a device (cpal)
//! or on the null output, publishes the [`AudioClock`], and survives device changes.
//!
//! # Threads
//!
//! ```text
//!  supervisor thread                       device thread (cpal / null)          readers
//!  ─────────────────                       ───────────────────────────          ───────
//!  open stream (device, rate, format) ──▶  callback: Mixer::render ──▶ device   AudioClock::position
//!  watch: stream errors, default device,     publish clock anchor ───────────▶  (any thread)
//!         callbacks still arriving           peak meter ──(rtrb)──────────────▶ drain_meters
//!  restart: drop stream ◀──(mpsc) the player (Mixer + clock writer) comes home
//!           open again (≤ 2 s)
//! ```
//!
//! * **The player** (the [`Mixer`], the clock's only writer, scratch and meters) lives inside the
//!   stream's callback. When a stream is dropped the callback is dropped with it and sends the
//!   player back to the supervisor, which moves it into the next stream: one writer, no locks.
//! * **The callback** is real-time safe: no allocation, no lock, no syscall (it reads `Instant`,
//!   which is a user-mode counter read on Windows and macOS). `testing` builds count its heap
//!   operations ([`OutputStats::rt_heap_ops`]).
//! * **Device changes.** A stream error (device removed, default device changed: cpal reports
//!   `StreamInvalidated`/`DeviceNotAvailable`), a different default device (polled every
//!   500 ms, for backends that do not report it) or a stream that stopped calling back (1 s
//!   watchdog) restarts the stream on the current device. With no device the supervisor retries
//!   every 250 ms. The clock holds ([`ClockState::Stopped`]) meanwhile and never goes back.
//! * **Pause** keeps the device running and plays silence; the clock holds. Seeks made while
//!   paused are applied at once, so the clock shows the new position.
//! * **Rate.** The stream runs at the mixer's rate. Shared-mode WASAPI converts any rate; on a
//!   device that cannot play it, the supervisor keeps retrying and reports the error (the
//!   engine can rebuild the mixer at [`AudioOutput::preferred_rate`]).
//! * **Clock anchors.** Each playing callback anchors the clock at its first advancing sample,
//!   audible at callback entry + the latency cpal reports (`playback − callback`). The first
//!   callback of a stream primes an empty device buffer before the device clock runs (S2
//!   finding 3), and priming or paused buffers do not advance: those hold the clock.
//!
//! # Manual test: device changes
//!
//! Run `device_change_manual` in `tests/output.rs` (its docs have the command) and, while it
//! plays, switch the system default output device, then unplug the device in use and plug it
//! back. Each switch must move the sound within 2 s; the test prints every state change and
//! fails on a long outage or a clock that went back. Record the result per OS in the PR.

mod callback;
#[cfg(all(feature = "device", any(windows, target_os = "macos")))]
mod cpal_backend;
#[cfg(test)]
mod tests;
mod virtual_device;

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, channel, sync_channel};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::thread::{JoinHandle, Thread};
use std::time::{Duration as StdDuration, Instant};

use cutweave_model::{SampleRate, Time};
use rtrb::{Consumer, RingBuffer};

use crate::clock::{AudioClock, ClockShared, ClockState, ClockWriter};
use crate::error::OutputError;
use crate::rt::{BusPeak, Mixer};
use callback::{Player, StreamCallback};

/// Environment variable that picks the output: `null` (no device: a wall-clock-driven clock for
/// CI and headless use), `default` (or unset: the system default device), or a device name
/// (case-insensitive substring).
pub const AUDIO_ENV: &str = "CUTWEAVE_AUDIO";

/// The frames per callback requested by default (WBS 1G.02). Backends may use another size:
/// shared-mode WASAPI always calls back at the device period (480 frames at 48 kHz, S2 finding
/// 1); [`OutputInfo::period_frames`] reports what was granted.
pub const DEFAULT_BUFFER_FRAMES: u32 = 256;

/// Meter frames buffered for [`AudioOutput::drain_meters`] (about 4 s at 30 Hz).
const METER_QUEUE: usize = 128;
/// How often the meters publish.
pub const METER_HZ: u32 = 30;
/// How often the supervisor polls the default device (for backends that do not report changes).
const DEFAULT_DEVICE_POLL: StdDuration = StdDuration::from_millis(500);
/// Supervisor wake-up period while a stream runs.
const SUPERVISOR_POLL: StdDuration = StdDuration::from_millis(50);
/// How long a dropped stream may take to hand the player back.
const RETURN_TIMEOUT: StdDuration = StdDuration::from_secs(3);
/// How long [`AudioOutput::start`] waits for the first stream.
const START_TIMEOUT: StdDuration = StdDuration::from_secs(10);

/// Which output to play on.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum OutputDevice {
    /// The system default output device; follows default-device changes.
    #[default]
    Default,
    /// The first output device whose name contains this text (case-insensitive), or whose id
    /// equals it.
    Named(String),
    /// No device: a thread plays the mix into the void at the wall-clock rate (10 ms periods).
    /// For CI, headless MCP and machines without audio.
    Null,
}

/// How [`AudioOutput`] opens and keeps its stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputConfig {
    /// The device.
    pub device: OutputDevice,
    /// Frames per callback to request (`None`: the backend's default). If the backend refuses
    /// it, the stream is opened with the backend default instead.
    pub buffer_frames: Option<u32>,
    /// Retry period while no device is available.
    pub retry_interval: StdDuration,
    /// Restart a stream that delivered no callback for this long.
    pub watchdog: StdDuration,
}

impl OutputConfig {
    /// `device` with the defaults: 256-frame requests, 250 ms retries, 1 s watchdog.
    #[must_use]
    pub fn new(device: OutputDevice) -> OutputConfig {
        OutputConfig {
            device,
            buffer_frames: Some(DEFAULT_BUFFER_FRAMES),
            retry_interval: StdDuration::from_millis(250),
            watchdog: StdDuration::from_secs(1),
        }
    }

    /// The null output.
    #[must_use]
    pub fn null() -> OutputConfig {
        OutputConfig::new(OutputDevice::Null)
    }

    /// The output [`AUDIO_ENV`] (`CUTWEAVE_AUDIO`) asks for.
    #[must_use]
    pub fn from_env() -> OutputConfig {
        OutputConfig::new(OutputDevice::parse(
            std::env::var(AUDIO_ENV).ok().as_deref().unwrap_or(""),
        ))
    }
}

impl Default for OutputConfig {
    fn default() -> OutputConfig {
        OutputConfig::new(OutputDevice::Default)
    }
}

impl OutputDevice {
    /// Parses a [`AUDIO_ENV`] value: `null`/`none`/`off` → [`Null`](Self::Null), empty or
    /// `default` → [`Default`](Self::Default), anything else names a device.
    #[must_use]
    pub fn parse(value: &str) -> OutputDevice {
        let v = value.trim();
        match v.to_ascii_lowercase().as_str() {
            "null" | "none" | "off" => OutputDevice::Null,
            "" | "default" => OutputDevice::Default,
            _ => OutputDevice::Named(v.to_owned()),
        }
    }
}

/// The stream the output runs now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputInfo {
    /// Backend (`WASAPI`, `CoreAudio`, `null`).
    pub host: String,
    /// Device name.
    pub device: String,
    /// Stream rate (always the mixer's rate).
    pub rate: SampleRate,
    /// Device channels (the stereo mix goes to the first two; mono devices get L+R / 2).
    pub channels: u16,
    /// Device sample format (`F32`, `I16`, ...).
    pub sample_format: String,
    /// Frames per callback requested by the configuration.
    pub buffer_requested: Option<u32>,
    /// Frames per callback asked of the backend (`None`: its default), after clamping to the
    /// device's range and any fallback.
    pub buffer_used: Option<u32>,
    /// Frames per callback the backend reports for the stream.
    pub period_frames: Option<u32>,
}

/// Where the output stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum OutputState {
    /// A stream is open and calling back.
    Running,
    /// The stream is being rebuilt after a device change or error.
    Restarting,
    /// No usable device; retrying.
    WaitingForDevice,
    /// Stopped (not started, shut down, or failed for good).
    #[default]
    Stopped,
}

impl OutputState {
    const fn to_u8(self) -> u8 {
        match self {
            OutputState::Running => 0,
            OutputState::Restarting => 1,
            OutputState::WaitingForDevice => 2,
            OutputState::Stopped => 3,
        }
    }

    const fn from_u8(v: u8) -> OutputState {
        match v {
            0 => OutputState::Running,
            1 => OutputState::Restarting,
            2 => OutputState::WaitingForDevice,
            _ => OutputState::Stopped,
        }
    }
}

/// Health counters of the output (whole run, across restarts).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OutputStats {
    /// Where the output stands.
    pub state: OutputState,
    /// Callbacks so far.
    pub callbacks: u64,
    /// Frames handed to devices (including silence while paused or priming).
    pub frames: u64,
    /// Smallest and largest callback, in frames.
    pub callback_frames: Option<(u32, u32)>,
    /// Output latency the device last reported.
    pub latency: StdDuration,
    /// Longest time spent inside a callback.
    pub max_callback_time: StdDuration,
    /// Buffer under/overruns the backend reported.
    pub xruns: u64,
    /// Other stream errors the backend reported.
    pub stream_errors: u64,
    /// Streams rebuilt after a device change, an error or a stall.
    pub restarts: u64,
    /// Heap operations inside the callback; counted only in `testing` builds with the counting
    /// allocator installed (must stay 0).
    pub rt_heap_ops: u64,
    /// Master peak (linear, L and R) of the latest meter window.
    pub master_peak: [f32; 2],
    /// The last error, if any.
    pub last_error: Option<String>,
}

/// Track buses a [`MeterFrame`] carries (further buses are not metered).
pub const MAX_METER_BUSES: usize = 32;

/// A peak meter reading of the master and the track buses, published at [`METER_HZ`] while
/// playing. Fixed size, so the real-time thread publishes it without allocating.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeterFrame {
    /// Timeline position of the end of the window.
    pub at: Time,
    /// Master peak (linear, L and R) over the window, after the limiter.
    pub peak: [f32; 2],
    buses: [BusPeak; MAX_METER_BUSES],
    bus_count: usize,
}

impl MeterFrame {
    /// Peak of each track bus over the window (after the track gain), in mix order.
    #[must_use]
    pub fn buses(&self) -> &[BusPeak] {
        &self.buses[..self.bus_count.min(MAX_METER_BUSES)]
    }
}

/// An output device, as [`output_devices`] lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Backend-specific id ([`OutputDevice::Named`] accepts it).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Whether it is the system default output.
    pub is_default: bool,
    /// Its default rate, if it reports one.
    pub default_rate: Option<u32>,
    /// Its default channel count, if it reports one.
    pub channels: Option<u16>,
    /// Its default sample format, if it reports one.
    pub sample_format: Option<String>,
    /// The frames-per-callback range it reports (`min`, `max`), if any.
    pub buffer_range: Option<(u32, u32)>,
}

/// Lists the output devices of the default host. Empty when the build has no device backend.
pub fn output_devices() -> Result<Vec<DeviceInfo>, OutputError> {
    #[cfg(all(feature = "device", any(windows, target_os = "macos")))]
    {
        cpal_backend::output_devices()
    }
    #[cfg(not(all(feature = "device", any(windows, target_os = "macos"))))]
    {
        Ok(Vec::new())
    }
}

/// State shared by the API, the supervisor, the callback and the error callback.
#[derive(Debug, Default)]
pub(crate) struct OutputShared {
    pub playing: AtomicBool,
    pub stop: AtomicBool,
    pub restart: AtomicBool,
    pub state: AtomicU8,
    pub callbacks: AtomicU64,
    pub frames: AtomicU64,
    pub min_frames: AtomicU32,
    pub max_frames: AtomicU32,
    pub latency_ns: AtomicU64,
    pub max_callback_ns: AtomicU64,
    pub xruns: AtomicU64,
    pub stream_errors: AtomicU64,
    pub restarts: AtomicU64,
    pub rt_heap_ops: AtomicU64,
    pub peak: [AtomicU32; 2],
    /// Non-real-time fields (the callback never touches them).
    pub info: Mutex<Option<OutputInfo>>,
    pub last_error: Mutex<Option<String>>,
    pub supervisor: OnceLock<Thread>,
}

impl OutputShared {
    fn new() -> OutputShared {
        let s = OutputShared::default();
        s.min_frames.store(u32::MAX, Ordering::Relaxed);
        s.state
            .store(OutputState::Stopped.to_u8(), Ordering::Relaxed);
        s
    }

    fn set_state(&self, state: OutputState) {
        self.state.store(state.to_u8(), Ordering::Release);
    }

    fn set_error(&self, msg: impl Into<String>) {
        *self
            .last_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(msg.into());
    }

    fn wake_supervisor(&self) {
        if let Some(t) = self.supervisor.get() {
            t.unpark();
        }
    }
}

/// What a stream's error callback can report.
#[derive(Clone, Debug)]
// Only the device backend reports xruns and warnings.
#[cfg_attr(
    not(all(feature = "device", any(windows, target_os = "macos"))),
    allow(dead_code)
)]
pub(crate) enum StreamFault {
    /// A buffer under/overrun: counted only.
    Xrun,
    /// Informational (e.g. real-time priority refused): logged only.
    Warning(String),
    /// The stream is broken or on the wrong device: restart it.
    Fatal(String),
}

/// The error callback's handle: counters and a restart request to the supervisor. Never blocks
/// on the real-time thread (an xrun is one atomic increment).
#[derive(Clone, Debug)]
pub(crate) struct ErrorSink {
    shared: Arc<OutputShared>,
}

impl ErrorSink {
    pub fn report(&self, fault: StreamFault) {
        match fault {
            StreamFault::Xrun => {
                self.shared.xruns.fetch_add(1, Ordering::Relaxed);
            }
            StreamFault::Warning(msg) => tracing::warn!("audio output: {msg}"),
            StreamFault::Fatal(msg) => {
                self.shared.stream_errors.fetch_add(1, Ordering::Relaxed);
                tracing::warn!("audio stream error, restarting: {msg}");
                self.shared.set_error(msg);
                self.shared.restart.store(true, Ordering::Release);
                self.shared.wake_supervisor();
            }
        }
    }
}

/// What the supervisor asks a backend for.
#[derive(Clone, Debug)]
// The null output ignores `device`.
#[cfg_attr(
    not(all(feature = "device", any(windows, target_os = "macos"))),
    allow(dead_code)
)]
pub(crate) struct StreamRequest {
    pub device: OutputDevice,
    pub rate: SampleRate,
    pub buffer_frames: Option<u32>,
}

/// A running stream; dropping it stops it and drops its callback (which sends the player home).
pub(crate) trait LiveStream {
    fn info(&self) -> OutputInfo;
    /// The device's id, for default-device change detection.
    fn device_id(&self) -> Option<String>;
}

/// An audio backend (cpal, null, or a test fake). Lives on the supervisor thread.
pub(crate) trait Backend {
    /// Opens and starts a stream for `request` that calls `callback`. On failure the callback
    /// has been dropped (so the player is on its way home).
    fn open(
        &mut self,
        request: &StreamRequest,
        callback: StreamCallback,
        errors: ErrorSink,
    ) -> Result<Box<dyn LiveStream>, OutputError>;

    /// The current default output device's id, if there is one.
    fn default_device_id(&mut self) -> Option<String>;
}

/// Makes the backend on the supervisor thread (cpal objects need not be `Send`).
pub(crate) type BackendFactory = Box<dyn FnOnce() -> Box<dyn Backend> + Send>;

// The `Err` arm exists in builds without a device backend.
#[allow(clippy::unnecessary_wraps)]
fn backend_factory(device: &OutputDevice) -> Result<BackendFactory, OutputError> {
    match device {
        OutputDevice::Null => Ok(Box::new(|| {
            Box::new(virtual_device::NullBackend) as Box<dyn Backend>
        })),
        #[cfg(all(feature = "device", any(windows, target_os = "macos")))]
        OutputDevice::Default | OutputDevice::Named(_) => Ok(Box::new(|| {
            Box::new(cpal_backend::CpalBackend::new()) as Box<dyn Backend>
        })),
        #[cfg(not(all(feature = "device", any(windows, target_os = "macos"))))]
        OutputDevice::Default | OutputDevice::Named(_) => Err(OutputError::NoBackend),
    }
}

/// [`AudioOutput::start`] failed; the mixer is handed back (for example to retry on the null
/// output).
#[derive(Debug, thiserror::Error)]
#[error("could not start the audio output: {error}")]
pub struct StartError {
    /// Why.
    #[source]
    pub error: OutputError,
    /// The mixer that was passed in (`None` only if the output thread could not be started or
    /// panicked).
    pub mixer: Option<Box<Mixer>>,
}

/// Plays a [`Mixer`] on an output device and publishes the master [`AudioClock`].
///
/// Starts paused (silence, clock held): call [`play`](Self::play). Drop it (or call
/// [`stop`](Self::stop)) **before** the [`MixerHandle`](crate::MixerHandle): the mixer must stop
/// before its workers, or plans it still holds are leaked.
pub struct AudioOutput {
    shared: Arc<OutputShared>,
    clock: AudioClock,
    rate: SampleRate,
    meters: Mutex<Consumer<MeterFrame>>,
    supervisor: Option<JoinHandle<Option<Player>>>,
}

impl std::fmt::Debug for AudioOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioOutput")
            .field("rate", &self.rate)
            .field("info", &self.info())
            .field("stats", &self.stats())
            .finish_non_exhaustive()
    }
}

impl AudioOutput {
    /// The rate to build the mixer at for `config`: the device's own rate (so the OS does not
    /// resample), or 48 kHz for the null output.
    pub fn preferred_rate(config: &OutputConfig) -> Result<SampleRate, OutputError> {
        match &config.device {
            OutputDevice::Null => Ok(SampleRate::HZ_48_000),
            #[cfg(all(feature = "device", any(windows, target_os = "macos")))]
            device => cpal_backend::preferred_rate(device),
            #[cfg(not(all(feature = "device", any(windows, target_os = "macos"))))]
            _ => Err(OutputError::NoBackend),
        }
    }

    /// Opens the output for `config` and plays `mixer` on it (paused). Fails, handing the mixer
    /// back, if no stream could be opened.
    pub fn start(mixer: Mixer, config: OutputConfig) -> Result<AudioOutput, StartError> {
        match backend_factory(&config.device) {
            Ok(factory) => AudioOutput::start_with(mixer, config, factory),
            Err(error) => Err(StartError {
                error,
                mixer: Some(Box::new(mixer)),
            }),
        }
    }

    pub(crate) fn start_with(
        mixer: Mixer,
        config: OutputConfig,
        factory: BackendFactory,
    ) -> Result<AudioOutput, StartError> {
        let rate = mixer.rate();
        let (writer, clock) = ClockWriter::new(rate, mixer.position());
        let shared = Arc::new(OutputShared::new());
        let (meter_tx, meter_rx) = RingBuffer::new(METER_QUEUE);
        let player = Player::new(mixer, writer, meter_tx, shared.clone());
        let (ready_tx, ready_rx) = sync_channel(1);
        let thread_shared = shared.clone();
        let spawned = std::thread::Builder::new()
            .name("cutweave-audio-output".into())
            .spawn(move || {
                let backend = factory();
                Supervisor::new(backend, config, thread_shared, rate).run(player, &ready_tx)
            });
        let handle = match spawned {
            Ok(h) => h,
            Err(e) => {
                // The closure (and the player in it) was dropped: the mixer is gone.
                return Err(StartError {
                    error: OutputError::Thread(e.to_string()),
                    mixer: None,
                });
            }
        };
        let _ = shared.supervisor.set(handle.thread().clone());
        let outcome = ready_rx.recv_timeout(START_TIMEOUT);
        let output = AudioOutput {
            shared,
            clock,
            rate,
            meters: Mutex::new(meter_rx),
            supervisor: Some(handle),
        };
        match outcome {
            Ok(Ok(())) => Ok(output),
            Ok(Err(error)) => Err(output.fail(error)),
            Err(_) => Err(output.fail(OutputError::Thread(
                "the first stream did not open in time".into(),
            ))),
        }
    }

    /// Shuts down after a failed start and recovers the mixer.
    fn fail(mut self, error: OutputError) -> StartError {
        let mixer = self.shutdown().map(|p| Box::new(p.mixer));
        StartError { error, mixer }
    }

    /// The master clock.
    #[must_use]
    pub fn clock(&self) -> AudioClock {
        self.clock.clone()
    }

    /// The mixer's (and stream's) rate.
    #[must_use]
    pub fn rate(&self) -> SampleRate {
        self.rate
    }

    /// Plays: the mixer advances and the clock runs (after priming).
    pub fn play(&self) {
        self.shared.playing.store(true, Ordering::Release);
    }

    /// Pauses: the device plays silence, the mixer and the clock hold.
    pub fn pause(&self) {
        self.shared.playing.store(false, Ordering::Release);
    }

    /// Whether [`play`](Self::play) is in effect.
    #[must_use]
    pub fn is_playing(&self) -> bool {
        self.shared.playing.load(Ordering::Acquire)
    }

    /// Rebuilds the stream on the configured device (for example after the user changed the
    /// device's format), exactly as after a device change: the clock holds, then resumes
    /// without going back.
    pub fn restart(&self) {
        self.shared.restart.store(true, Ordering::Release);
        self.shared.wake_supervisor();
    }

    /// The stream that runs now (`None` while restarting or waiting for a device).
    #[must_use]
    pub fn info(&self) -> Option<OutputInfo> {
        self.shared
            .info
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The health counters.
    #[must_use]
    pub fn stats(&self) -> OutputStats {
        let s = &*self.shared;
        let min = s.min_frames.load(Ordering::Relaxed);
        let max = s.max_frames.load(Ordering::Relaxed);
        OutputStats {
            state: OutputState::from_u8(s.state.load(Ordering::Acquire)),
            callbacks: s.callbacks.load(Ordering::Relaxed),
            frames: s.frames.load(Ordering::Relaxed),
            callback_frames: (min != u32::MAX).then_some((min, max)),
            latency: StdDuration::from_nanos(s.latency_ns.load(Ordering::Relaxed)),
            max_callback_time: StdDuration::from_nanos(s.max_callback_ns.load(Ordering::Relaxed)),
            xruns: s.xruns.load(Ordering::Relaxed),
            stream_errors: s.stream_errors.load(Ordering::Relaxed),
            restarts: s.restarts.load(Ordering::Relaxed),
            rt_heap_ops: s.rt_heap_ops.load(Ordering::Relaxed),
            master_peak: [
                f32::from_bits(s.peak[0].load(Ordering::Relaxed)),
                f32::from_bits(s.peak[1].load(Ordering::Relaxed)),
            ],
            last_error: s
                .last_error
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
        }
    }

    /// Moves the meter frames published since the last call into `out` (oldest first). The
    /// queue holds about 4 s; older frames are dropped when nobody drains it.
    pub fn drain_meters(&self, out: &mut Vec<MeterFrame>) {
        let mut rx = self.meters.lock().unwrap_or_else(PoisonError::into_inner);
        while let Ok(frame) = rx.pop() {
            out.push(frame);
        }
    }

    /// Stops the stream and returns the mixer (`None` if a backend never returned it).
    pub fn stop(mut self) -> Option<Mixer> {
        self.shutdown().map(|p| p.mixer)
    }

    fn shutdown(&mut self) -> Option<Player> {
        let handle = self.supervisor.take()?;
        self.shared.stop.store(true, Ordering::Release);
        handle.thread().unpark();
        handle.join().unwrap_or_else(|_| {
            tracing::error!("the audio output supervisor panicked");
            None
        })
    }
}

impl Drop for AudioOutput {
    fn drop(&mut self) {
        drop(self.shutdown());
    }
}

/// What the supervisor watches on the running stream.
struct Watch {
    /// The callback count when it last changed, and when that was.
    callbacks: u64,
    progress: Instant,
    next_default_poll: Instant,
}

impl Watch {
    fn new(shared: &OutputShared) -> Watch {
        let now = Instant::now();
        Watch {
            callbacks: shared.callbacks.load(Ordering::Relaxed),
            progress: now,
            next_default_poll: now + DEFAULT_DEVICE_POLL,
        }
    }
}

/// The supervisor: opens streams and restarts them.
struct Supervisor {
    backend: Box<dyn Backend>,
    config: OutputConfig,
    shared: Arc<OutputShared>,
    rate: SampleRate,
    home_tx: Sender<Player>,
    home_rx: Receiver<Player>,
}

impl Supervisor {
    fn new(
        backend: Box<dyn Backend>,
        config: OutputConfig,
        shared: Arc<OutputShared>,
        rate: SampleRate,
    ) -> Supervisor {
        let (home_tx, home_rx) = channel();
        Supervisor {
            backend,
            config,
            shared,
            rate,
            home_tx,
            home_rx,
        }
    }

    fn set_info(&self, info: Option<OutputInfo>) {
        *self
            .shared
            .info
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = info;
    }

    /// Why the running stream must be rebuilt, if it must: a stream error or restart request,
    /// no callback for the watchdog period, or (following the default device) a new default.
    fn restart_reason(&mut self, live: &dyn LiveStream, watch: &mut Watch) -> Option<String> {
        let now = Instant::now();
        let mut reason = None;
        if self.shared.restart.swap(false, Ordering::AcqRel) {
            reason = Some("stream error or restart request".to_owned());
        }
        let callbacks = self.shared.callbacks.load(Ordering::Relaxed);
        if callbacks != watch.callbacks {
            watch.callbacks = callbacks;
            watch.progress = now;
        } else if now.duration_since(watch.progress) > self.config.watchdog {
            reason = Some(format!("no callback for {:?}", self.config.watchdog));
        }
        if self.config.device == OutputDevice::Default && now >= watch.next_default_poll {
            watch.next_default_poll = now + DEFAULT_DEVICE_POLL;
            let current = self.backend.default_device_id();
            if current != live.device_id() {
                reason = Some(format!("default device changed to {current:?}"));
            }
        }
        reason
    }

    /// Opens a stream for `player` (falling back to the backend's default buffer size if the
    /// requested one is refused). On failure, returns the player if it came back.
    // The player moves through here once per open attempt (not a hot path).
    #[allow(clippy::result_large_err)]
    fn open(
        &mut self,
        player: Player,
    ) -> Result<Box<dyn LiveStream>, (OutputError, Option<Player>)> {
        let mut request = StreamRequest {
            device: self.config.device.clone(),
            rate: self.rate,
            buffer_frames: self.config.buffer_frames,
        };
        let mut player = player;
        loop {
            self.shared.restart.store(false, Ordering::Release);
            let callback = StreamCallback::new(player, self.home_tx.clone());
            let errors = ErrorSink {
                shared: self.shared.clone(),
            };
            match self.backend.open(&request, callback, errors) {
                Ok(stream) => {
                    let mut info = stream.info();
                    info.buffer_requested = self.config.buffer_frames;
                    tracing::info!(
                        host = %info.host,
                        device = %info.device,
                        rate = info.rate.hz(),
                        channels = info.channels,
                        format = %info.sample_format,
                        requested = ?info.buffer_requested,
                        used = ?info.buffer_used,
                        period = ?info.period_frames,
                        "audio output stream started"
                    );
                    self.set_info(Some(info));
                    return Ok(stream);
                }
                Err(error) => {
                    let back = self.home_rx.recv_timeout(RETURN_TIMEOUT).ok();
                    match back {
                        Some(p) if request.buffer_frames.is_some() => {
                            tracing::info!(
                                "audio stream with {:?}-frame buffers failed ({error}); \
                                 retrying with the backend default",
                                request.buffer_frames
                            );
                            request.buffer_frames = None;
                            player = p;
                        }
                        back => return Err((error, back)),
                    }
                }
            }
        }
    }

    fn run(
        mut self,
        player: Player,
        ready: &SyncSender<Result<(), OutputError>>,
    ) -> Option<Player> {
        let mut stream = match self.open(player) {
            Ok(s) => {
                self.shared.set_state(OutputState::Running);
                let _ = ready.send(Ok(()));
                Some(s)
            }
            Err((error, player)) => {
                let _ = ready.send(Err(error));
                return player;
            }
        };
        let mut player: Option<Player> = None;
        let mut retry_at: Option<Instant> = None;
        let mut watch = Watch::new(&self.shared);

        while !self.shared.stop.load(Ordering::Acquire) {
            if let Some(live) = &stream
                && let Some(reason) = self.restart_reason(live.as_ref(), &mut watch)
            {
                tracing::info!("audio output restarting: {reason}");
                self.shared.set_state(OutputState::Restarting);
                self.set_info(None);
                drop(stream.take());
                player = self.home_rx.recv_timeout(RETURN_TIMEOUT).ok();
                let Some(p) = player.as_mut() else {
                    self.shared
                        .set_error("the audio stream never released the mixer");
                    break;
                };
                p.clock
                    .hold(ClockState::Stopped, None, None, ClockShared::host_ns);
                self.shared.restarts.fetch_add(1, Ordering::Relaxed);
                retry_at = Some(Instant::now());
            }
            if stream.is_none() && retry_at.is_some_and(|t| Instant::now() >= t) {
                let Some(p) = player.take() else { break };
                match self.open(p) {
                    Ok(s) => {
                        stream = Some(s);
                        retry_at = None;
                        watch = Watch::new(&self.shared);
                        self.shared.set_state(OutputState::Running);
                    }
                    Err((error, back)) => {
                        self.shared.set_error(error.to_string());
                        let Some(back) = back else {
                            tracing::error!("the audio stream never released the mixer");
                            break;
                        };
                        player = Some(back);
                        self.shared.set_state(OutputState::WaitingForDevice);
                        retry_at = Some(Instant::now() + self.config.retry_interval);
                    }
                }
            }
            let wait = match (stream.is_some(), retry_at) {
                (false, Some(t)) => t
                    .saturating_duration_since(Instant::now())
                    .min(SUPERVISOR_POLL),
                _ => SUPERVISOR_POLL,
            };
            std::thread::park_timeout(wait);
        }

        // Shutdown: stop the stream, take the player back, leave the clock stopped.
        drop(stream.take());
        let mut player = player.or_else(|| self.home_rx.recv_timeout(RETURN_TIMEOUT).ok());
        if let Some(p) = player.as_mut() {
            p.clock
                .hold(ClockState::Stopped, None, None, ClockShared::host_ns);
        }
        self.set_info(None);
        self.shared.set_state(OutputState::Stopped);
        player
    }
}
