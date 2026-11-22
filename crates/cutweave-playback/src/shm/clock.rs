// SPDX-License-Identifier: GPL-3.0-or-later
//! The audio clock words of the ring: [`ClockSample`], [`ClockWriter`] and [`ClockPump`].
//!
//! The preview present loop picks "the newest frame with `pts <= clock - latency`" (arch §10
//! step 3; S1 measured 0 skipped frames with a 20 ms latency). It reads the clock from the
//! ring header and extrapolates between samples with [`ClockSample::extrapolate`].

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;

use cutweave_model::Time;

use crate::shm::layout::{ClockState, FLICKS_PER_SECOND, RATE_MILLI_1X};
use crate::shm::os::host_clock_ns;
use crate::shm::publisher::Shared;

/// Readers stop extrapolating the clock this long after its last sample: a stalled clock
/// writer then freezes the preview instead of letting it run ahead.
pub const MAX_CLOCK_EXTRAPOLATION_NS: u64 = 100_000_000;
/// Default [`ClockPump`] period: 1 ms (1 kHz nominal, as in Spike S1), which keeps the rate
/// above the required 200 Hz even when sleeps overshoot on a busy machine. Run the pump while
/// playing or scrubbing; a stopped clock needs one sample, since readers do not extrapolate it.
pub const DEFAULT_CLOCK_PERIOD: std::time::Duration = std::time::Duration::from_millis(1);

/// One audio clock sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockSample {
    /// Timeline position of the audio clock (`AudioClock::position`, WP-1G.02).
    pub position: Time,
    /// Host time ([`crate::shm::host_clock_ns`]) at which `position` was sampled.
    pub host_ns: u64,
    /// Playback rate in thousandths (1000 = 1x, -2000 = 2x reverse; JKL levels).
    pub rate_milli: i32,
    /// Transport state.
    pub state: ClockState,
}

impl ClockSample {
    /// A sample taken now.
    #[must_use]
    pub fn now(position: Time, state: ClockState, rate_milli: i32) -> Self {
        Self {
            position,
            host_ns: host_clock_ns(),
            rate_milli,
            state,
        }
    }

    /// A stopped clock at `position`, sampled now.
    #[must_use]
    pub fn stopped(position: Time) -> Self {
        Self::now(position, ClockState::Stopped, RATE_MILLI_1X)
    }

    /// Decode raw header words (unknown states read as stopped, out-of-range positions as 0).
    pub(crate) fn from_raw(flicks: i64, host_ns: u64, rate_milli: i32, state: u32) -> Self {
        Self {
            position: Time::from_flicks(flicks).unwrap_or(Time::ZERO),
            host_ns,
            rate_milli,
            state: ClockState::from_u32(state).unwrap_or_default(),
        }
    }

    /// The clock position at host time `now_ns`, in flicks: the sample itself unless
    /// [`ClockState::Playing`], else advanced by `(now_ns - host_ns) * rate`, with the
    /// elapsed time capped at [`MAX_CLOCK_EXTRAPOLATION_NS`]. Readers mirror this formula.
    #[must_use]
    pub fn extrapolate(&self, now_ns: u64) -> i64 {
        let base = self.position.flicks();
        if self.state != ClockState::Playing {
            return base;
        }
        let elapsed = now_ns
            .saturating_sub(self.host_ns)
            .min(MAX_CLOCK_EXTRAPOLATION_NS);
        let delta =
            i128::from(elapsed) * i128::from(FLICKS_PER_SECOND) * i128::from(self.rate_milli)
                / (1_000_000_000 * i128::from(RATE_MILLI_1X));
        i64::try_from(i128::from(base) + delta).unwrap_or(base)
    }
}

/// Publishes the audio clock into the publisher's current ring (it follows ring moves).
/// Cheap to clone; safe to use from any thread; never blocks on readers.
#[derive(Clone)]
pub struct ClockWriter {
    shared: Arc<Shared>,
}

impl ClockWriter {
    pub(crate) fn new(shared: Arc<Shared>) -> Self {
        Self { shared }
    }

    /// Store `sample`. Returns false once the publisher has been dropped.
    #[must_use = "false means the publisher is gone and the writer can stop"]
    pub fn publish(&self, sample: &ClockSample) -> bool {
        self.shared
            .with_current(|ring| ring.write_clock(sample))
            .is_some()
    }
}

/// A background thread that samples a clock source every `period` and publishes it (WBS:
/// "audio clock word updated at ≥ 200 Hz from `AudioClock`"). Stops when dropped or when
/// the publisher is gone.
pub struct ClockPump {
    stop: Arc<AtomicBool>,
    updates: Arc<AtomicU64>,
    thread: Option<JoinHandle<()>>,
}

impl ClockPump {
    /// Start the pump. `source` is called on the pump thread each period (read the lock-free
    /// `AudioClock` there, never block).
    pub fn spawn<F>(
        writer: ClockWriter,
        period: std::time::Duration,
        mut source: F,
    ) -> std::io::Result<Self>
    where
        F: FnMut() -> ClockSample + Send + 'static,
    {
        let stop = Arc::new(AtomicBool::new(false));
        let updates = Arc::new(AtomicU64::new(0));
        let (stop2, updates2) = (Arc::clone(&stop), Arc::clone(&updates));
        let thread = std::thread::Builder::new()
            .name("cw-preview-clock".into())
            .spawn(move || {
                // Deadline-paced: an oversleep shortens the next sleep instead of lowering the
                // rate; after a long stall the schedule restarts rather than bursting.
                let mut next = std::time::Instant::now();
                while !stop2.load(Ordering::Relaxed) {
                    if !writer.publish(&source()) {
                        break;
                    }
                    updates2.fetch_add(1, Ordering::Relaxed);
                    next += period;
                    let now = std::time::Instant::now();
                    match next.checked_duration_since(now) {
                        Some(wait) => std::thread::sleep(wait),
                        None if now - next > period => next = now,
                        None => {}
                    }
                }
            })?;
        Ok(Self {
            stop,
            updates,
            thread: Some(thread),
        })
    }

    /// Samples published so far.
    #[must_use]
    pub fn updates(&self) -> u64 {
        self.updates.load(Ordering::Relaxed)
    }

    /// False once the pump thread has exited (stopped, or the publisher is gone).
    #[must_use]
    pub fn thread_running(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }

    /// Stop the pump and wait for its thread. Returns the number of samples published.
    #[allow(clippy::must_use_candidate)] // Stopping is the point; the count is optional.
    pub fn stop(mut self) -> u64 {
        self.shutdown();
        self.updates()
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            // A panicking clock source already reported itself; nothing to add here.
            let _ = t.join();
        }
    }
}

impl Drop for ClockPump {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(state: ClockState, rate_milli: i32) -> ClockSample {
        ClockSample {
            position: Time::from_secs(10).unwrap(),
            host_ns: 1_000_000_000,
            rate_milli,
            state,
        }
    }

    #[test]
    fn extrapolation_follows_rate_and_state() {
        let ten_s = 10 * FLICKS_PER_SECOND;
        let ms20 = 1_000_000_000 + 20_000_000;
        let per_20ms = FLICKS_PER_SECOND / 50;
        assert_eq!(
            sample(ClockState::Playing, 1000).extrapolate(ms20),
            ten_s + per_20ms
        );
        assert_eq!(
            sample(ClockState::Playing, 2000).extrapolate(ms20),
            ten_s + 2 * per_20ms
        );
        assert_eq!(
            sample(ClockState::Playing, -1000).extrapolate(ms20),
            ten_s - per_20ms
        );
        assert_eq!(
            sample(ClockState::Playing, 500).extrapolate(ms20),
            ten_s + per_20ms / 2
        );
        assert_eq!(sample(ClockState::Stopped, 1000).extrapolate(ms20), ten_s);
        assert_eq!(sample(ClockState::Scrubbing, 1000).extrapolate(ms20), ten_s);
        // A reader clock behind the sample does not move it backwards.
        assert_eq!(sample(ClockState::Playing, 1000).extrapolate(0), ten_s);
        // Capped at 100 ms after the last sample.
        let late = 1_000_000_000 + 5_000_000_000;
        assert_eq!(
            sample(ClockState::Playing, 1000).extrapolate(late),
            ten_s + FLICKS_PER_SECOND / 10
        );
    }
}
