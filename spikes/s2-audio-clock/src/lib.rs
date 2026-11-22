// SPDX-License-Identifier: GPL-3.0-or-later
//! Spike S2 (WP-0D.01): an 8-track cpal mixer whose device clock is the master clock, with
//! rubato resampling, signalsmith-stretch time-stretch, an ebur128 meter + limiter on the master,
//! and a video presenter that follows the audio clock. Throwaway code: only the README findings
//! carry forward (to `cutweave-audio` / `cutweave-playback`).
//!
//! Thread layout:
//! - **cpal callback** (real-time): drains 8 SPSC rings, gain/pan, limiter, publishes the clock.
//! - **workers** (`--workers`): synth source → rubato → signalsmith → ring, running ahead.
//! - **meter**: ebur128 on a tap of the master.
//! - **logger**: CSV + console rows; **main**: video presenter (window or terminal).

pub mod clock;
pub mod engine;
pub mod font;
pub mod limiter;
pub mod meter;
pub mod mixer;
pub mod rt_alloc;
pub mod scene;
pub mod soak;
pub mod synth;
pub mod track;
pub mod util;
pub mod video;

/// Process-wide allocator; counts heap traffic inside real-time sections (see [`rt_alloc`]).
#[global_allocator]
static GLOBAL: rt_alloc::CountingAlloc = rt_alloc::CountingAlloc;
