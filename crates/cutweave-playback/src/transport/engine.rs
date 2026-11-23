// SPDX-License-Identifier: GPL-3.0-or-later
//! [`PlaybackEngine`]: the transport with its render and status threads, and the ring clock
//! pump.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration as StdDuration;

use thiserror::Error;

use crate::shm::{ClockPump, ClockWriter, DEFAULT_CLOCK_PERIOD};
use crate::transport::audio::AudioControl;
use crate::transport::control::Transport;
use crate::transport::host::{HostClock, SystemClock};
use crate::transport::render_loop::{RenderConfig, RenderLoop, Tick};
use crate::transport::sink::{FrameSink, SinkError};
use crate::transport::source::FrameSource;
use crate::transport::status::{StateSink, StatusPump};

/// Playback could not start.
#[derive(Debug, Error)]
pub enum PlaybackError {
    /// The sink has no valid frame layout.
    #[error(transparent)]
    Sink(#[from] SinkError),
    /// A thread could not be started.
    #[error("could not start the {name} thread: {source}")]
    Thread {
        /// Which thread.
        name: &'static str,
        /// Why.
        source: std::io::Error,
    },
}

/// How playback runs.
#[derive(Clone)]
pub struct PlaybackConfig {
    /// The render loop's tuning.
    pub render: RenderConfig,
    /// The host clock ([`SystemClock`] unless simulating).
    pub host: Arc<dyn HostClock>,
}

impl Default for PlaybackConfig {
    fn default() -> PlaybackConfig {
        PlaybackConfig {
            render: RenderConfig::default(),
            host: Arc::new(SystemClock),
        }
    }
}

impl std::fmt::Debug for PlaybackConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlaybackConfig")
            .field("render", &self.render)
            .finish_non_exhaustive()
    }
}

/// The transport, render loop and status pump before they run: drive them by hand
/// ([`RenderLoop::tick`], [`StatusPump::poll`]) for deterministic tests, or hand them to
/// [`PlaybackEngine::spawn`].
#[derive(Debug)]
pub struct PlaybackParts {
    /// The control handle.
    pub transport: Transport,
    /// The render thread's body.
    pub render: RenderLoop,
    /// The status thread's body.
    pub status: StatusPump,
}

impl PlaybackParts {
    /// Playback of `source` (stopped at 0) following `audio`'s clock, delivering to `sink`.
    pub fn new(
        source: Box<dyn FrameSource>,
        audio: Box<dyn AudioControl>,
        sink: Box<dyn FrameSink>,
        config: PlaybackConfig,
    ) -> Result<PlaybackParts, PlaybackError> {
        let transport = Transport::new(audio, source.timing(), Arc::clone(&config.host));
        let render = RenderLoop::new(Arc::clone(&transport.shared), source, sink, config.render)?;
        let status = StatusPump::new(transport.clone());
        Ok(PlaybackParts {
            transport,
            render,
            status,
        })
    }
}

/// Playback attached to a frame source, an audio output and a frame sink (WBS
/// `PlaybackEngine::attach`), running on two threads:
///
/// * `cw-playback-render` runs the [`RenderLoop`], sleeping until its next deadline or a
///   transport change;
/// * `cw-playback-status` runs the [`StatusPump`] (`playback.state` at ≤ 30 Hz, the stop at
///   the end of the content).
///
/// Control it through [`transport`](Self::transport) (the `view.*` commands' context). For the
/// preview ring, also run [`spawn_clock_pump`](Self::spawn_clock_pump). Dropping it stops the
/// threads; [`shutdown`](Self::shutdown) also returns the source and sink.
pub struct PlaybackEngine {
    transport: Transport,
    stop: Arc<AtomicBool>,
    render: Option<JoinHandle<RenderLoop>>,
    status: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for PlaybackEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlaybackEngine")
            .field("transport", &self.transport)
            .finish_non_exhaustive()
    }
}

fn sleep_until(host: &dyn HostClock, wake_at_ns: u64) {
    let now = host.now_ns();
    if wake_at_ns > now {
        std::thread::park_timeout(StdDuration::from_nanos(wake_at_ns - now));
    }
}

impl PlaybackEngine {
    /// Starts playback of `source` following `audio`, delivering to `sink` and publishing
    /// `playback.state` to `events`.
    pub fn attach(
        source: Box<dyn FrameSource>,
        audio: Box<dyn AudioControl>,
        sink: Box<dyn FrameSink>,
        events: Box<dyn StateSink>,
        config: PlaybackConfig,
    ) -> Result<PlaybackEngine, PlaybackError> {
        PlaybackEngine::spawn(PlaybackParts::new(source, audio, sink, config)?, events)
    }

    /// Runs `parts` on the render and status threads.
    pub fn spawn(
        parts: PlaybackParts,
        mut events: Box<dyn StateSink>,
    ) -> Result<PlaybackEngine, PlaybackError> {
        let PlaybackParts {
            transport,
            mut render,
            mut status,
        } = parts;
        let stop = Arc::new(AtomicBool::new(false));
        let mut engine = PlaybackEngine {
            transport: transport.clone(),
            stop: Arc::clone(&stop),
            render: None,
            status: None,
        };
        let host = Arc::clone(&transport.shared.host);
        let flag = Arc::clone(&stop);
        let handle = std::thread::Builder::new()
            .name("cw-playback-render".into())
            .spawn(move || {
                while !flag.load(Ordering::Acquire) {
                    if let Tick::Idle { wake_at_ns } = render.tick() {
                        sleep_until(host.as_ref(), wake_at_ns);
                    }
                }
                render
            })
            .map_err(|source| PlaybackError::Thread {
                name: "render",
                source,
            })?;
        let _ = transport.shared.render_thread.set(handle.thread().clone());
        engine.render = Some(handle);

        let host = Arc::clone(&transport.shared.host);
        let flag = Arc::clone(&stop);
        let handle = std::thread::Builder::new()
            .name("cw-playback-status".into())
            .spawn(move || {
                while !flag.load(Ordering::Acquire) {
                    let poll = status.poll();
                    if let Some(state) = &poll.event {
                        events.publish(state);
                    }
                    sleep_until(host.as_ref(), poll.wake_at_ns);
                }
            })
            .map_err(|source| PlaybackError::Thread {
                name: "status",
                source,
            })?;
        let _ = transport.shared.status_thread.set(handle.thread().clone());
        engine.status = Some(handle);
        Ok(engine)
    }

    /// The transport (clone it for the `view.*` commands).
    #[must_use]
    pub fn transport(&self) -> &Transport {
        &self.transport
    }

    /// Publishes the transport's clock into the preview ring at 1 kHz
    /// ([`Transport::clock_sample`]; WBS 1H.02: ≥ 200 Hz). Take `writer` from
    /// [`FramePublisher::clock_writer`](crate::shm::FramePublisher::clock_writer) before
    /// moving the publisher into a [`PublisherSink`](crate::transport::PublisherSink). The
    /// transport's host clock must be [`SystemClock`].
    pub fn spawn_clock_pump(&self, writer: ClockWriter) -> std::io::Result<ClockPump> {
        let transport = self.transport.clone();
        ClockPump::spawn(writer, DEFAULT_CLOCK_PERIOD, move || {
            transport.clock_sample()
        })
    }

    /// Stops the threads and returns the render loop (with the source and sink).
    #[must_use]
    pub fn shutdown(mut self) -> Option<RenderLoop> {
        self.stop_threads()
    }

    fn stop_threads(&mut self) -> Option<RenderLoop> {
        self.stop.store(true, Ordering::Release);
        let mut render = None;
        if let Some(h) = self.render.take() {
            h.thread().unpark();
            render = h.join().ok();
        }
        if let Some(h) = self.status.take() {
            h.thread().unpark();
            // A panicking event sink already reported itself.
            let _ = h.join();
        }
        render
    }
}

impl Drop for PlaybackEngine {
    fn drop(&mut self) {
        let _ = self.stop_threads();
    }
}
