// SPDX-License-Identifier: GPL-3.0-or-later
//! The audio side of the transport: [`AudioControl`] is what the transport drives (play, pause,
//! seek) and whose [`AudioClock`] 1× playback follows (arch §9 "Output and master clock").
//!
//! * [`OutputAudio`]: the real output ([`AudioOutput`] + [`MixerHandle`], WP-1G.02). With
//!   [`OutputConfig::null`](cutweave_audio::OutputConfig::null) it is the headless clock (MCP,
//!   CLI, CI): a wall-clock-driven output that plays nothing.
//! * [`ManualAudio`]: a [`ManualClock`] for deterministic tests; it counts the calls the
//!   transport makes, so tests can prove audio is never paused or re-seeked to wait for video.
//!
//! Only the control path calls these (under the transport's control lock, never the render
//! thread); the render thread, the clock pump and the status pump only read the lock-free
//! [`AudioClock`].

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use cutweave_audio::{AudioClock, AudioOutput, ManualClock, MixerHandle};
use cutweave_model::Time;

use crate::transport::target::PlaybackTarget;

/// The audio output as the transport drives it.
pub trait AudioControl: Send {
    /// The master clock (lock-free; read from any thread).
    fn clock(&self) -> AudioClock;
    /// Starts (or resumes) audible playback at 1×.
    fn play(&mut self);
    /// Pauses: silence, the clock holds.
    fn pause(&mut self);
    /// Moves the audio playhead to `t`: the clock starts a new generation at `t` and holds there
    /// until the audio at `t` is audible.
    fn seek(&mut self, t: Time);
    /// What plays changed (`view.set_source`, ADR-0022): from now on the audio is `target`'s,
    /// in its own time. Called paused, before the next `seek`, which re-primes from it. The
    /// transport's audio alone cannot know a target's sound: the engine feeds the mixer (the
    /// default does nothing).
    fn set_target(&mut self, target: PlaybackTarget) {
        let _ = target;
    }
}

/// The device (or null) output playing the mixer.
///
/// Both halves are shared: the engine keeps its own `Arc`s to feed the mixer
/// (`MixerHandle::set_segments`, master gain) and read the output (meters, stats) while the
/// transport drives play, pause and seek. WP-1G.02 requires the output to stop before the mixer
/// handle is dropped: this type drops its output reference first, and the engine must not drop
/// the last handle while it still holds the output.
pub struct OutputAudio {
    // Field order is drop order: the output reference goes before the mixer's.
    output: Arc<AudioOutput>,
    mixer: Arc<MixerHandle>,
}

impl std::fmt::Debug for OutputAudio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutputAudio")
            .field("output", &self.output)
            .field("mixer", &self.mixer)
            .finish()
    }
}

impl OutputAudio {
    /// The transport's audio over a started `output` and the handle of the mixer it plays.
    #[must_use]
    pub fn new(output: Arc<AudioOutput>, mixer: Arc<MixerHandle>) -> OutputAudio {
        OutputAudio { output, mixer }
    }

    /// The output.
    #[must_use]
    pub fn output(&self) -> &Arc<AudioOutput> {
        &self.output
    }

    /// The mixer handle (set segments, master gain).
    #[must_use]
    pub fn mixer(&self) -> &Arc<MixerHandle> {
        &self.mixer
    }
}

impl AudioControl for OutputAudio {
    fn clock(&self) -> AudioClock {
        self.output.clock()
    }

    fn play(&mut self) {
        self.output.play();
    }

    fn pause(&mut self) {
        self.output.pause();
    }

    fn seek(&mut self, t: Time) {
        self.mixer.seek(t);
    }
}

/// How often the transport called each [`AudioControl`] method.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AudioCalls {
    /// `play` calls.
    pub play: u64,
    /// `pause` calls.
    pub pause: u64,
    /// `seek` calls.
    pub seek: u64,
    /// `set_target` calls.
    pub set_target: u64,
}

#[derive(Debug, Default)]
struct CallCounters {
    play: AtomicU64,
    pause: AtomicU64,
    seek: AtomicU64,
    set_target: AtomicU64,
}

/// A [`ManualClock`] as the transport's audio (tests, deterministic demos): play/pause set its
/// state, seek jumps it (new generation). The test advances it.
#[derive(Clone, Debug)]
pub struct ManualAudio {
    clock: Arc<ManualClock>,
    calls: Arc<CallCounters>,
}

impl ManualAudio {
    /// Audio over `clock`.
    #[must_use]
    pub fn new(clock: Arc<ManualClock>) -> ManualAudio {
        ManualAudio {
            clock,
            calls: Arc::default(),
        }
    }

    /// The manual clock (advance it to move time).
    #[must_use]
    pub fn manual(&self) -> &Arc<ManualClock> {
        &self.clock
    }

    /// The calls made so far (clones share the counters).
    #[must_use]
    pub fn calls(&self) -> AudioCalls {
        AudioCalls {
            play: self.calls.play.load(Ordering::SeqCst),
            pause: self.calls.pause.load(Ordering::SeqCst),
            seek: self.calls.seek.load(Ordering::SeqCst),
            set_target: self.calls.set_target.load(Ordering::SeqCst),
        }
    }
}

impl AudioControl for ManualAudio {
    fn clock(&self) -> AudioClock {
        self.clock.clock()
    }

    fn play(&mut self) {
        self.calls.play.fetch_add(1, Ordering::SeqCst);
        self.clock.play();
    }

    fn pause(&mut self) {
        self.calls.pause.fetch_add(1, Ordering::SeqCst);
        self.clock.pause();
    }

    fn seek(&mut self, t: Time) {
        self.calls.seek.fetch_add(1, Ordering::SeqCst);
        self.clock.seek(t);
    }

    fn set_target(&mut self, _target: PlaybackTarget) {
        self.calls.set_target.fetch_add(1, Ordering::SeqCst);
    }
}
