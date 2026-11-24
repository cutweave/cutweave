// SPDX-License-Identifier: GPL-3.0-or-later
//! The output callback: [`Mixer::render`] into the device buffer, clock anchors, the master
//! meter and health counters. Everything here runs on the device's real-time thread: no
//! allocation, no lock, no syscall.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Sender;

use cutweave_model::{SampleRate, Time};
use rtrb::Producer;

use super::{MAX_METER_BUSES, METER_HZ, MeterFrame, OutputShared};
use crate::clock::{ClockShared, ClockState, ClockWriter};
use crate::dsp::{CHANNELS, peak};
use crate::rt::{BusPeak, Mixer};

/// Frames mixed per inner step of a callback (the scratch size). Larger device buffers are
/// mixed in several steps; device periods are far smaller (480 frames on WASAPI at 48 kHz).
pub(crate) const SCRATCH_FRAMES: usize = 4096;

/// A device sample type the stereo `f32` mix converts to.
pub(crate) trait OutputSample: Copy + Send + 'static {
    /// The value of silence.
    const SILENCE: Self;
    /// Converts a sample in `[-1, 1]`.
    fn from_f32(v: f32) -> Self;
}

impl OutputSample for f32 {
    const SILENCE: f32 = 0.0;
    #[inline]
    fn from_f32(v: f32) -> f32 {
        v
    }
}

#[cfg(all(feature = "device", any(windows, target_os = "macos")))]
macro_rules! cpal_samples {
    ($($t:ty),* $(,)?) => {$(
        impl OutputSample for $t {
            const SILENCE: $t = <$t as cpal::Sample>::EQUILIBRIUM;
            #[inline]
            fn from_f32(v: f32) -> $t {
                <$t as cpal::FromSample<f32>>::from_sample_(v)
            }
        }
    )*};
}

#[cfg(all(feature = "device", any(windows, target_os = "macos")))]
cpal_samples!(
    f64,
    i8,
    i16,
    cpal::I24,
    i32,
    i64,
    u8,
    u16,
    cpal::U24,
    u32,
    u64
);

/// The peak meter of the master output.
struct MasterMeter {
    peak: [f32; 2],
    frames: u32,
    window: u32,
    rate: SampleRate,
    frames_out: Producer<MeterFrame>,
}

impl MasterMeter {
    fn new(rate: SampleRate, frames_out: Producer<MeterFrame>) -> MasterMeter {
        MasterMeter {
            peak: [0.0; 2],
            frames: 0,
            window: (rate.hz() / METER_HZ).max(1),
            rate,
            frames_out,
        }
    }

    /// Adds mixed stereo audio that ends at output sample `end`; at the end of each window
    /// publishes a frame with the master peak and the bus peaks the mixer collected.
    fn add(&mut self, audio: &[f32], end: i64, mixer: &mut Mixer, shared: &OutputShared) {
        let p = peak(audio);
        self.peak = [self.peak[0].max(p[0]), self.peak[1].max(p[1])];
        let frames = audio.len() / CHANNELS;
        self.frames = self
            .frames
            .saturating_add(u32::try_from(frames).unwrap_or(u32::MAX));
        if self.frames < self.window {
            return;
        }
        self.frames %= self.window;
        let master = std::mem::take(&mut self.peak);
        shared.peak[0].store(master[0].to_bits(), Ordering::Relaxed);
        shared.peak[1].store(master[1].to_bits(), Ordering::Relaxed);
        let mut frame = MeterFrame {
            at: self.rate.samples_to_time(end).unwrap_or(Time::MAX),
            peak: master,
            buses: [BusPeak::SILENT; MAX_METER_BUSES],
            bus_count: 0,
        };
        frame.bus_count = mixer.take_bus_peaks(&mut frame.buses);
        // A full queue (nobody drains it) drops the frame.
        let _ = self.frames_out.push(frame);
    }
}

/// Callback timing.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Timing {
    /// Host time (clock ns) at callback entry.
    pub entry_ns: u64,
    /// Device latency: the first frame of this buffer is audible this long after `entry_ns`.
    pub latency_ns: u64,
}

/// What the callback owns that survives a stream restart: the mixer, the clock's writer, the
/// scratch buffer and the meter.
pub(crate) struct Player {
    pub mixer: Mixer,
    pub clock: ClockWriter,
    scratch: Box<[f32]>,
    seeks: u64,
    meter: MasterMeter,
    shared: Arc<OutputShared>,
}

impl Player {
    pub fn new(
        mixer: Mixer,
        clock: ClockWriter,
        meter: Producer<MeterFrame>,
        shared: Arc<OutputShared>,
    ) -> Player {
        let rate = mixer.rate();
        Player {
            seeks: mixer.seeks(),
            mixer,
            clock,
            scratch: vec![0.0; SCRATCH_FRAMES * CHANNELS].into_boxed_slice(),
            meter: MasterMeter::new(rate, meter),
            shared,
        }
    }

    /// Fills `out` (`channels` interleaved) and publishes the clock. `first`: the first callback
    /// of a stream, which primes an empty device buffer before the device clock runs (S2
    /// finding 3), so it is not used as a clock anchor.
    fn process<T: OutputSample>(&mut self, out: &mut [T], channels: usize, t: Timing, first: bool) {
        let channels = channels.max(1);
        let frames = out.len() / channels;
        let (out, tail) = out.split_at_mut(frames * channels);
        tail.fill(T::SILENCE);
        let rate = u64::from(self.mixer.rate().hz());

        if !self.shared.playing.load(Ordering::Acquire) {
            // Paused: apply queued plans (a seek moves the held clock), play silence.
            let _ = self.mixer.ready(0);
            out.fill(T::SILENCE);
            let reset = self.take_seek().then(|| self.mixer.position());
            self.clock.hold(
                ClockState::Paused,
                reset,
                Some(t.latency_ns),
                ClockShared::host_ns,
            );
            return;
        }

        // The last step that advanced anchors the clock: (its first sample, frames before it).
        let mut anchor: Option<(i64, usize)> = None;
        let mut end = 0;
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(SCRATCH_FRAMES);
            let mix = &mut self.scratch[..n * CHANNELS];
            let seeks = self.mixer.seeks();
            let info = self.mixer.render(mix);
            if let Some(p) = info.position {
                let p_end = p.saturating_add(i64::try_from(n).unwrap_or(0));
                anchor = Some((p, done));
                end = p_end;
                self.meter.add(mix, p_end, &mut self.mixer, &self.shared);
            } else if self.mixer.seeks() != seeks {
                // A seek in a later step: what earlier steps played is no longer the timeline.
                anchor = None;
            }
            write_device(
                &mut out[done * channels..(done + n) * channels],
                mix,
                channels,
            );
            done += n;
        }

        let seeked = self.take_seek();
        match anchor {
            Some((p, offset)) if !first => {
                let offset_ns = (offset as u64).saturating_mul(1_000_000_000) / rate.max(1);
                let audible = t
                    .entry_ns
                    .saturating_add(t.latency_ns)
                    .saturating_add(offset_ns);
                self.clock
                    .run(p, audible, end, t.latency_ns, seeked, ClockShared::host_ns);
            }
            Some((p, _)) => self.clock.hold(
                ClockState::Priming,
                seeked.then_some(p),
                Some(t.latency_ns),
                ClockShared::host_ns,
            ),
            None => {
                let target = self.mixer.position();
                self.clock.hold(
                    ClockState::Priming,
                    seeked.then_some(target),
                    Some(t.latency_ns),
                    ClockShared::host_ns,
                );
            }
        }
    }

    /// Whether the mixer applied a seek since the last call.
    fn take_seek(&mut self) -> bool {
        let seeks = self.mixer.seeks();
        let seeked = seeks != self.seeks;
        self.seeks = seeks;
        seeked
    }

    /// Records the callback in the health counters.
    fn account(&self, frames: usize, t: Timing, exit_ns: u64) {
        let s = &*self.shared;
        let frames32 = u32::try_from(frames).unwrap_or(u32::MAX);
        // Release: the supervisor reports `Running` once it sees this, and whoever sees that
        // must see the clock state this callback published.
        s.callbacks.fetch_add(1, Ordering::Release);
        s.frames.fetch_add(frames as u64, Ordering::Relaxed);
        s.min_frames.fetch_min(frames32, Ordering::Relaxed);
        s.max_frames.fetch_max(frames32, Ordering::Relaxed);
        s.latency_ns.store(t.latency_ns, Ordering::Relaxed);
        s.max_callback_ns
            .fetch_max(exit_ns.saturating_sub(t.entry_ns), Ordering::Relaxed);
    }
}

/// Writes the stereo mix `src` to the device buffer `out` (`channels` interleaved): the first
/// two channels get L and R, the others silence; a mono device gets (L + R) / 2.
fn write_device<T: OutputSample>(out: &mut [T], src: &[f32], channels: usize) {
    let (src, _) = src.as_chunks::<CHANNELS>();
    match channels {
        1 => {
            for (o, s) in out.iter_mut().zip(src) {
                *o = T::from_f32(f32::midpoint(s[0], s[1]));
            }
        }
        2 => {
            for (o, s) in out.as_chunks_mut::<2>().0.iter_mut().zip(src) {
                *o = [T::from_f32(s[0]), T::from_f32(s[1])];
            }
        }
        n => {
            for (o, s) in out.chunks_exact_mut(n).zip(src) {
                o[0] = T::from_f32(s[0]);
                o[1] = T::from_f32(s[1]);
                o[2..].fill(T::SILENCE);
            }
        }
    }
}

/// The data callback a backend calls. Owns the [`Player`] while the stream lives; when the
/// stream drops it, the player is sent back to the supervisor.
pub(crate) struct StreamCallback {
    player: Option<Player>,
    home: Sender<Player>,
    first: bool,
}

impl StreamCallback {
    pub fn new(player: Player, home: Sender<Player>) -> StreamCallback {
        StreamCallback {
            player: Some(player),
            home,
            first: true,
        }
    }

    /// The real-time entry point: fill `out` (`channels` interleaved samples per frame); the
    /// first frame becomes audible `latency_ns` from now.
    #[inline]
    pub fn process<T: OutputSample>(&mut self, out: &mut [T], channels: usize, latency_ns: u64) {
        #[cfg(feature = "testing")]
        let section = crate::rt_alloc::RtSection::enter();
        let first = std::mem::replace(&mut self.first, false);
        if let Some(player) = self.player.as_mut() {
            let entry_ns = player.clock.host_ns();
            let t = Timing {
                entry_ns,
                latency_ns,
            };
            player.process(out, channels, t, first);
            player.account(out.len() / channels.max(1), t, player.clock.host_ns());
        } else {
            out.fill(T::SILENCE);
        }
        #[cfg(feature = "testing")]
        {
            let ops = section.exit();
            if ops > 0
                && let Some(player) = &self.player
            {
                player.shared.rt_heap_ops.fetch_add(ops, Ordering::Relaxed);
            }
        }
    }
}

impl Drop for StreamCallback {
    fn drop(&mut self) {
        if let Some(player) = self.player.take() {
            // The supervisor is gone only when the whole output is being torn down.
            let _ = self.home.send(player);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::float_cmp)]

    use super::*;

    #[test]
    fn stereo_maps_to_any_channel_count() {
        let src = [0.5f32, -0.25, 1.0, 0.0];
        let mut mono = [0.0f32; 2];
        write_device(&mut mono, &src, 1);
        assert_eq!(mono, [0.125, 0.5]);
        let mut stereo = [0.0f32; 4];
        write_device(&mut stereo, &src, 2);
        assert_eq!(stereo, src);
        let mut six = [9.0f32; 12];
        write_device(&mut six, &src, 6);
        assert_eq!(
            six,
            [0.5, -0.25, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0]
        );
    }
}
