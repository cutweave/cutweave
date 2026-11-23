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
//! * [`rt_alloc`]: the allocation counter that proves `Mixer::render` never allocates.
//!
//! The input is the segment list of `cutweave_render::evaluate_audio` (WP-1F.01). Times are
//! flicks; output positions are sample indices at the output rate, converted only through
//! `SampleRate::time_to_samples` / `samples_to_time` (arch §4.1). cpal output and the master
//! clock are WP-1G.02.

// Justification: this crate talks to OS, FFI or real-time interfaces (docs/01-architecture.md §3)
// and is allowed to use `unsafe`; every use must carry a `// SAFETY:` comment. Today the only
// unsafe code is the counting allocator in `rt_alloc`.
#![allow(unsafe_code)]

mod control;
mod dsp;
mod error;
mod lane;
mod offline;
mod plan;
mod rt;
pub mod rt_alloc;
pub mod source;
mod tone;
mod voice;
mod worker;

pub use control::{MIN_PREFETCH_MS, MixerConfig, MixerHandle, MixerStats};
pub use dsp::{CHANNELS, LIMITER_KNEE, db_to_gain, soft_limit};
pub use error::{MixError, SourceError};
pub use offline::{AudioChunk, MIXDOWN_CHUNK_FRAMES, Mixdown, mixdown};
pub use rt::{Mixer, RenderInfo};
pub use source::{PcmFormat, PcmSource, SourceFactory, SourceRequest};
pub use tone::{Tone, ToneFactory, ToneSource, ToneSpec};
