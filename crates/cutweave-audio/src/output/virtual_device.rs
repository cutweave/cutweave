// SPDX-License-Identifier: GPL-3.0-or-later
//! Streams without a device: the null output (`CUTWEAVE_AUDIO=null`), and the fake devices of the
//! supervisor tests. A thread calls the callback every period on a wall-clock schedule and
//! discards the audio, so the mixer and the clock advance at the real-time rate.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration as StdDuration, Instant};

use super::callback::StreamCallback;
use super::{Backend, ErrorSink, LiveStream, OutputError, OutputInfo, StreamFault, StreamRequest};

/// The null output's period: 10 ms, like a typical device (and cheap for headless use).
const NULL_PERIOD_MS: u32 = 10;
/// Channels of a virtual device.
const CHANNELS: u16 = 2;

/// Faults a test can inject into a virtual device.
#[derive(Debug, Default)]
pub(crate) struct Faults {
    /// The device disappeared: the stream reports a fatal error once and stops calling back.
    pub unplugged: AtomicBool,
    /// The stream stops calling back without reporting anything.
    pub stalled: AtomicBool,
    /// Delay before a new stream's first callback (ms), like a device that takes a while to
    /// start (WASAPI: tens of milliseconds after `Start`).
    pub start_delay_ms: AtomicU64,
}

/// One virtual stream: a thread calling the callback every `period` frames.
pub(crate) struct VirtualStream {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    info: OutputInfo,
    device_id: Option<String>,
}

impl VirtualStream {
    pub fn spawn(
        mut callback: StreamCallback,
        info: OutputInfo,
        device_id: Option<String>,
        faults: Option<Arc<Faults>>,
        errors: ErrorSink,
    ) -> Result<VirtualStream, OutputError> {
        let rate = info.rate.hz();
        let period_frames = info.period_frames.unwrap_or(rate / 100).max(1);
        let channels = usize::from(info.channels);
        // The first frame of a buffer is audible one period after it is mixed (a double
        // buffer): the clock then has a period of slack against scheduling jitter.
        let latency_ns = u64::from(period_frames) * 1_000_000_000 / u64::from(rate.max(1));
        let period = StdDuration::from_nanos(latency_ns);
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let thread = std::thread::Builder::new()
            .name("cutweave-audio-null".into())
            .spawn(move || {
                let mut buf = vec![0.0f32; period_frames as usize * channels];
                let mut reported = false;
                let delay = faults
                    .as_ref()
                    .map_or(0, |f| f.start_delay_ms.load(Ordering::Acquire));
                let started = Instant::now();
                while started.elapsed() < StdDuration::from_millis(delay)
                    && !thread_stop.load(Ordering::Acquire)
                {
                    std::thread::sleep(StdDuration::from_millis(1));
                }
                let mut deadline = Instant::now();
                while !thread_stop.load(Ordering::Acquire) {
                    let unplugged = faults
                        .as_ref()
                        .is_some_and(|f| f.unplugged.load(Ordering::Acquire));
                    let stalled = faults
                        .as_ref()
                        .is_some_and(|f| f.stalled.load(Ordering::Acquire));
                    if unplugged && !reported {
                        reported = true;
                        errors.report(StreamFault::Fatal("the device was removed".into()));
                    }
                    if !unplugged && !stalled {
                        callback.process(&mut buf, channels, latency_ns);
                    }
                    deadline += period;
                    let now = Instant::now();
                    if let Some(wait) = deadline.checked_duration_since(now) {
                        std::thread::sleep(wait);
                    } else if now.duration_since(deadline) > StdDuration::from_millis(200) {
                        // Suspended or starved: restart the schedule instead of bursting.
                        deadline = now;
                    }
                }
                drop(callback); // sends the player home
            })
            .map_err(|e| OutputError::Thread(e.to_string()))?;
        Ok(VirtualStream {
            stop,
            thread: Some(thread),
            info,
            device_id,
        })
    }
}

impl LiveStream for VirtualStream {
    fn info(&self) -> OutputInfo {
        self.info.clone()
    }

    fn device_id(&self) -> Option<String> {
        self.device_id.clone()
    }
}

impl Drop for VirtualStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take()
            && t.join().is_err()
        {
            tracing::error!("the null audio thread panicked");
        }
    }
}

/// The null output: never fails, follows nothing.
pub(crate) struct NullBackend;

impl Backend for NullBackend {
    fn open(
        &mut self,
        request: &StreamRequest,
        callback: StreamCallback,
        errors: ErrorSink,
    ) -> Result<Box<dyn LiveStream>, OutputError> {
        let period = request.rate.hz() * NULL_PERIOD_MS / 1000;
        let info = OutputInfo {
            host: "null".into(),
            device: "null".into(),
            rate: request.rate,
            channels: CHANNELS,
            sample_format: "F32".into(),
            buffer_requested: request.buffer_frames,
            buffer_used: request.buffer_frames,
            period_frames: Some(period),
        };
        Ok(Box::new(VirtualStream::spawn(
            callback, info, None, None, errors,
        )?))
    }

    fn default_device_id(&mut self) -> Option<String> {
        None
    }
}
