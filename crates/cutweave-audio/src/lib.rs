// SPDX-License-Identifier: GPL-3.0-or-later
//! `cutweave-audio`: mixer graph, effects, resampling, time-stretch, loudness and output clock
//! (arch §9).
//!
//! WP-1G.01 (per R29) builds the mixer graph v0:
//!
//! ```text
//! per-clip sources ─▶ per-track lane ─▶ per-track bus ─▶ master ─▶ 2-ch f32 at the output rate
//! (PcmSource: decode  (Σ clips × clip   (× track gain,   (× master gain,
//!  → stereo → rubato)  gain, worker)     ramped; RT)      soft limiter at 0 dBFS; RT)
//! ```
//!
//! * [`PcmSource`] / [`SourceFactory`]: the source seam. [`ToneFactory`] generates sines,
//!   impulses and silence for tests and demos; decoded clip audio plugs in through an adapter
//!   over the media backend's `AudioDecoder` when WP-1D.01/1D.03 land (plan in [`source`]).
//! * [`Mixer`] (real-time half, owned by the audio callback) and [`MixerHandle`] (control half):
//!   lock-free SPSC rings fed by worker threads ≥ 500 ms ahead, plans delivered through a
//!   lock-free queue, seeks that flush and refill. Threading model: [`control`](crate::control)
//!   module docs.
//! * [`mixdown`]: the offline renderer (export) over the same lane and bus code.
//! * `rt_alloc` (feature `testing`): the allocation counter that proves `Mixer::render` never
//!   allocates.
//!
//! The input is the segment list of `cutweave_render::evaluate_audio` (WP-1F.01). Times are
//! flicks; output positions are sample indices at the output rate, converted only through
//! `SampleRate::time_to_samples` / `samples_to_time` (arch §4.1).
//!
//! WP-1G.02 adds the output and the master clock:
//!
//! * [`AudioOutput`] plays a [`Mixer`] on a device through cpal (feature `device`, on by
//!   default: WASAPI, CoreAudio) or on the null output ([`OutputDevice::Null`],
//!   `CUTWEAVE_AUDIO=null`), restarts the stream when the device changes, pauses, and meters the
//!   master. Threading and restart rules: [`output`](crate::output) module docs.
//! * [`AudioClock`]: the timeline position of the audio audible now, lock-free from any thread,
//!   monotonic within a generation (a seek starts a new one). Video follows it (arch §10).
//!   [`ManualClock`] is its deterministic stand-in for tests.
//! * Master volume: [`MixerHandle::set_master_gain`] (dB) and
//!   [`MixerHandle::set_master_gain_linear`] (the `view` state's linear `master_gain`).
//!
//! WP-2E.02 adds the bus graph, the master limiter and the meters:
//!
//! ```text
//! lane ─▶ track bus (gain, pan, mute, exclusive solo; ramped) ─▶ Σ ─▶ master gain ─▶ true-peak limiter ─▶ out
//!          └ peak, RMS (post-fader)                                                   └ peak, RMS, LUFS M/S/I, true peak, gain reduction
//! ```
//!
//! * [`BusGraph`] / [`BusParams`]: per-track gain, pan, mute and solo (exclusive
//!   solo-in-place), fed with [`MixerHandle::set_mix`] or [`mixdown_with`]. Bus changes ramp in
//!   at the next callback without re-rendering lanes.
//! * [`Limiter`] / [`LimiterConfig`]: the true-peak look-ahead limiter on the master (default
//!   −1 dBTP, 5 ms look-ahead, 100 ms release). Its latency is compensated: the lanes run that
//!   far ahead, so output sample `n` is still timeline sample `n`, in playback and offline.
//! * Meters: [`MeterFrame`]s at [`METER_HZ`] carry the bus levels ([`BusLevel`]) and the master
//!   peak, RMS, true peak, loudness ([`Loudness`], via [`LoudnessMeter`]) and limiter gain.
//!   [`analyze_levels`] measures a range offline (`audio.get_levels`).

mod automation;
mod bus;
mod clock;
mod control;
mod dsp;
mod error;
mod lane;
mod limiter;
mod meter;
mod offline;
pub mod output;
mod plan;
mod priority;
mod reverse;
mod rt;
#[cfg(feature = "testing")]
pub mod rt_alloc;
pub mod source;
mod tone;
mod voice;
mod worker;

pub use bus::{BusGraph, BusLevel, BusParams};
pub use clock::{AudioClock, ClockReading, ClockState, ManualClock};
pub use control::{MIN_PREFETCH_MS, MixerConfig, MixerHandle, MixerStats};
pub use dsp::{CHANNELS, db_to_gain};
pub use error::{MixError, OutputError, SourceError};
pub use limiter::{
    DEFAULT_CEILING_DBTP, DEFAULT_LOOKAHEAD_MS, DEFAULT_RELEASE_MS, Limiter, LimiterConfig,
    MAX_CEILING_DBTP, MAX_LOOKAHEAD_MS, MAX_RELEASE_MS, MIN_CEILING_DBTP,
};
pub use meter::{LevelReport, Levels, Loudness, LoudnessMeter, TrackLevels, analyze_levels, to_db};
pub use offline::{AudioChunk, MIXDOWN_CHUNK_FRAMES, Mixdown, mixdown, mixdown_with};
pub use output::{
    AUDIO_ENV, AudioOutput, DEFAULT_BUFFER_FRAMES, DeviceInfo, MAX_METER_BUSES, METER_HZ,
    MeterFrame, OutputConfig, OutputDevice, OutputInfo, OutputState, OutputStats, StartError,
    output_devices,
};
pub use rt::{Mixer, RenderInfo};
pub use source::{PcmFormat, PcmSource, SourceFactory, SourceRequest};
pub use tone::{Tone, ToneFactory, ToneSource, ToneSpec};
