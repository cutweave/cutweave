// SPDX-License-Identifier: GPL-3.0-or-later
//! Transport, A/V sync, scrub scheduler and JKL (WP-1H.01, arch §10; feature `transport`).
//!
//! Per R31 this is built against a frame-source seam with synthetic frames: [`FrameSource`]
//! renders "the sequence frame at a time, at a quality, cancellably" and [`SyntheticSource`]
//! paints a test pattern whose pixels encode the frame number. The render core and decoder
//! pool plug in behind the same trait at WP-1I.05 ([`source`] module docs).
//!
//! # Pieces
//!
//! * [`Transport`]: the control handle (`view.play`, `view.pause`, `view.seek`,
//!   `view.set_rate`, `view.step`, `view.set_source`, [`commands`]) and lock-free reads of the
//!   live state. States: `Stopped | Playing{rate} | Scrubbing` ([`TransportMode`]); JKL rates
//!   ±1×/±2×/±4× ([`Rate`], [`shuttle`]). 1× forward plays audio and follows the audio clock
//!   ([`cutweave_audio::AudioClock`]); 2×/4× and every reverse rate are video-only in P1
//!   (audio muted) and step through the frame grid on the host clock.
//! * [`PlaybackTarget`]: what plays (ADR-0022), the sequence or one media asset on its own (a
//!   source, [`SourceInfo`], found by a [`SourceResolver`]). Each target has its own playhead;
//!   every transport command acts on the current one, in its time.
//! * [`RenderLoop`]: the render thread's body: A/V sync, skip-frames, prefetch hints, the
//!   latest-wins scrub scheduler (cache → quarter → full after 100 ms idle) and the quality
//!   policy ([`cutweave_render::AutoQuality`]).
//! * [`StatusPump`]: `playback.state` ([`PlaybackState`]) at ≤ 30 Hz and the stop at the end of
//!   the content.
//! * [`FrameSink`]: [`PublisherSink`] publishes into the preview ring (WP-1H.02), [`NullSink`]
//!   discards frames (headless).
//! * [`AudioControl`]: [`OutputAudio`] (the WP-1G.02 output; with the null output it is the
//!   headless clock for MCP, the CLI and CI) or [`ManualAudio`] (tests).
//! * [`PlaybackEngine`]: all of the above on their threads; [`PlaybackParts`] to drive them by
//!   hand under a [`VirtualClock`].
//!
//! # Threads
//!
//! ```text
//!  callers (RPC / MCP / CLI)        cw-playback-render          cw-playback-status   cw-preview-clock
//!  ─────────────────────────        ──────────────────          ──────────────────   ────────────────
//!  Transport::seek/play/…           RenderLoop::tick            StatusPump::poll     ClockPump (1 kHz)
//!   ├ control lock (control path)    ├ snapshot (seqlock) ◀──┐   ├ snapshot, clock    └ clock_sample()
//!   ├ AudioControl: seek/play/pause  ├ AudioClock (seqlock)  │   ├ stop at the end       ▶ ring clock
//!   ├ EpochGate::advance ────────────┼▶ CancelToken polls    │   └ playback.state ≤ 30 Hz
//!   ├ snapshot store ────────────────┴───────────────────────┘
//!   └ unpark render + status          FrameSource::render → EpochGate::try_commit → FrameSink
//! ```
//!
//! The control path is the only one that locks (one mutex, held for the state change and the
//! audio calls). The render thread never locks, blocks or allocates per frame: it reads the
//! transport snapshot and the audio clock through seqlocks, reuses its pixel buffer and counts
//! into atomics; it sleeps with `park_timeout` until its next deadline and is unparked by every
//! control change.
//!
//! # Latest wins, exactly
//!
//! Every control change starts a new epoch ([`EpochGate`]). Renders of older epochs see their
//! [`CancelToken`] cancelled, and a frame is delivered only inside
//! [`EpochGate::try_commit`], which fails for an older epoch and which `advance` waits for. So
//! once a request returned, no frame rendered for an older request is ever delivered.
//!
//! # A/V sync
//!
//! At 1× the playhead is the audio clock: after a seek the position holds at the target until
//! the clock's generation changes (the mixer re-primed), then follows it. The render loop
//! renders the frame (floor to the grid) the playhead will be at when the render finishes, so a
//! slow render skips frames (counted as dropped) instead of falling behind, and never pauses or
//! re-seeks the audio. Pausing keeps the audible position; playing seeks the audio there first
//! (sample-exact resume, WP-1G.02).

mod audio;
mod cancel;
pub mod commands;
mod control;
mod engine;
mod host;
mod render_loop;
mod sink;
pub mod source;
mod state;
mod status;
mod synthetic;
mod target;

#[cfg(test)]
mod tests;

pub use audio::{AudioCalls, AudioControl, ManualAudio, OutputAudio};
pub use cancel::{CancelToken, CommitGuard, EpochGate};
pub use commands::register;
pub use control::{PlaybackStats, Transport};
pub use engine::{PlaybackConfig, PlaybackEngine, PlaybackError, PlaybackParts};
pub use host::{HostClock, SystemClock, VirtualClock};
pub use render_loop::{RenderConfig, RenderLoop, Tick};
pub use sink::{
    FrameInfo, FrameSink, NullSink, PublisherSink, RingMoved, SinkError, scaled_preview,
};
pub use source::{
    FrameSource, PrefetchHint, RenderPurpose, RenderRequest, Rendered, SequenceTiming, SourceError,
};
pub use state::{FrameQuality, PlaybackState, Rate, Shuttle, TransportMode, shuttle};
pub use status::{MAX_STATE_HZ, PLAYBACK_STATE_EVENT, StateSink, StatusPoll, StatusPump};
pub use synthetic::{MARKER_LEN, SyntheticSource, decode_frame_number};
pub use target::{PlaybackTarget, SourceInfo, SourceResolver};

/// The events this module emits, as `(name, summary)` (for the generated events catalog).
pub const EVENTS: &[(&str, &str)] = &[(
    PLAYBACK_STATE_EVENT,
    "The transport changed or the playhead moved (at most 30 per second while playing)",
)];
