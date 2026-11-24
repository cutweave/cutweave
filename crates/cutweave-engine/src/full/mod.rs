// SPDX-License-Identifier: GPL-3.0-or-later
//! [`FullSubsystems`]: media, render, audio and playback attached to the core engine
//! (WP-1I.05; arch §7, §8, §9, §10).
//!
//! # What it wires
//!
//! - **Media** ([`media`]): the [`MediaBackend`] (FFmpeg in the real engine:
//!   [`FullConfig::native`]), `media.import`'s prober ([`BackendProbe`]: probe + fast content
//!   hash) and the analysis stages `index` → `thumbnails`, `waveform`, filling the global caches
//!   (`cache.sqlite`, thumbnails and filmstrips, waveform pyramids) in the OS cache directory.
//!   [`MediaState`] is what the `/media` HTTP endpoints (WP-1I.03) serve.
//! - **Frames** ([`frames`]): one `DecoderPool` per media item and the RAM `FrameCache`
//!   (budget: the smaller of 1 GiB and a quarter of RAM), shared by preview and `render.frame`.
//! - **Render** ([`gpu`]): a wgpu device per `EngineConfig::gpu` (`auto`: the best adapter,
//!   honouring `CUTWEAVE_GPU`; `software`: WARP / lavapipe), opened on its own thread while the
//!   rest starts. One compositor serves `render.frame` and export (its lock is the GPU budget), a
//!   second one preview. A GPU that fails within [`GPU_START_WAIT`] exits a `Gui` engine with
//!   code 11; otherwise (headless, or a device still opening) the engine falls back to the
//!   software adapter, then to the CPU reference renderer, and says so with
//!   `engine.degraded{subsystem: "gpu"}`. A device lost for good is reported the same way.
//! - **Audio** ([`audio`]): the mixer over the current document's clips, on the output
//!   `EngineConfig::audio` names (`CUTWEAVE_AUDIO` picks the device); when the device fails the
//!   mixer plays on the null clock and `engine.degraded{subsystem: "audio"}` is sent.
//!   `view.set{master_gain}` sets the master volume.
//! - **Playback**: per open document a `PlaybackEngine` (transport, render and status threads)
//!   rendering through [`preview::PreviewSource`] into the preview ring (`Gui` mode: an RGBA8
//!   ring announced in `rpc.hello`'s `shm` and the run-file, with its clock pump) or a null sink
//!   (headless). The `view.*` transport commands run against it; pauses, seeks and steps keep the
//!   view's playhead in step, and opening a project seeks the transport to its saved playhead.
//!   `playback.state` goes to every client at most 30 times a second.
//! - **Commands**: `render.frame` (with this engine's renderer current on the dispatching
//!   thread) and the `view.*` transport commands; `export.start` gets [`ExportServices`];
//!   `media.capabilities` answers from the backend (probed on a worker thread at start).
//!
//! # Lifecycle
//!
//! [`start`](Subsystems::start) brings up the caches, the GPU, audio and (in `Gui` mode) the
//! ring, then attaches playback to the document the engine opened with. A document swap
//! detaches it ([`on_document_closing`](Subsystems::on_document_closing): the watcher, the clock
//! pump, the playback threads, decoders; the ring and the compositor are kept for the next
//! document) and attaches it to the new one. [`shutdown`](Subsystems::shutdown) stops
//! playback, then audio (output before mixer), closes the ring and the decoders, waits for the
//! GPU and capabilities threads, then drops the device and the caches.

pub mod audio;
pub mod frames;
pub mod gpu;
pub mod media;
pub mod preview;
#[cfg(test)]
mod preview_alloc;
mod watch;

use std::any::TypeId;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;

use cutweave_cache::FrameCache;
use cutweave_commands::{CmdError, DocHandle, Registry, RegistryError};
use cutweave_media::MediaBackend;
use cutweave_ops::media::{AnalysisStages, MediaProbe};
use cutweave_ops::view::UiState;
use cutweave_playback::shm::{
    ClockWriter, FramePublisher, PixelFormat, RingAnnouncement, RingConfig,
};
use cutweave_playback::transport::{
    AudioControl, FrameSink, HostClock, ManualAudio, NullSink, OutputAudio, PLAYBACK_STATE_EVENT,
    PlaybackConfig, PlaybackEngine, PlaybackParts, PlaybackState, PlaybackStats, PublisherSink,
    RenderConfig, SystemClock, Transport,
};
use cutweave_render::gpu::AdapterChoice;
use cutweave_render::query::{RenderFrame, SharedBackend};
use serde_json::Value;

pub use audio::AudioMode;
pub use frames::MediaFrames;
pub use gpu::{GPU_START_WAIT, RENDER_WAIT};
pub use media::{BackendProbe, CurrentDoc, MediaFile, MediaState, total_ram};

use self::audio::{Audio, LivePcm};
use self::gpu::{LateGpu, LateRenderer};
use self::media::{Caches, IndexStage, ThumbStage, WaveformStage};
use self::preview::{CompositorSlot, PreviewSource};
use self::watch::{DocWatcher, Targets};
use crate::config::{EngineMode, GpuConfig};
use crate::export::{DecoderSources, ExportServices};
use crate::project_io::ProjectLayout;
use crate::subsystems::{
    EngineLink, SubsystemCall, SubsystemError, SubsystemEvents, SubsystemHost, SubsystemKind,
    Subsystems,
};

/// Default size of a headless engine's preview frames (they are rendered and dropped).
pub const HEADLESS_PREVIEW: (u32, u32) = (640, 360);
/// Default largest preview the ring's slots hold before it moves to a bigger ring.
pub const RING_SIZE: (u32, u32) = (1920, 1080);

/// How [`FullSubsystems`] runs.
pub struct FullConfig {
    /// Decodes, probes and encodes; `None`: no media backend (imports report every file as
    /// unreadable, clips render as missing, exports are unsupported).
    pub media: Option<Arc<dyn MediaBackend>>,
    /// `media.import`'s prober; `None`: [`BackendProbe`] over [`media`](Self::media).
    pub probe: Option<Arc<dyn MediaProbe>>,
    /// Root of the caches; `None`: `<OS cache dir>/cutweave`.
    pub cache_dir: Option<PathBuf>,
    /// Playback audio.
    pub audio: AudioMode,
    /// Preview frame size of a headless engine.
    pub headless_preview: (u32, u32),
    /// Largest preview the ring holds before moving (`Gui` mode).
    pub ring_size: (u32, u32),
    /// Run the analysis stages after import (index, thumbnails, waveform).
    pub analysis: bool,
    /// The frame cache budget in bytes; `None`: `FrameCache::default_budget(total RAM)`.
    pub frame_cache_bytes: Option<usize>,
    /// Why the media backend could not be created (reported as `engine.degraded` at start).
    pub media_problem: Option<String>,
}

impl Default for FullConfig {
    fn default() -> FullConfig {
        FullConfig {
            media: None,
            probe: None,
            cache_dir: None,
            audio: AudioMode::FromEngine,
            headless_preview: HEADLESS_PREVIEW,
            ring_size: RING_SIZE,
            analysis: true,
            frame_cache_bytes: None,
            media_problem: None,
        }
    }
}

impl std::fmt::Debug for FullConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FullConfig")
            .field("media", &self.media.is_some())
            .field("cache_dir", &self.cache_dir)
            .field("audio", &self.audio)
            .field("analysis", &self.analysis)
            .finish_non_exhaustive()
    }
}

impl FullConfig {
    /// The real engine's configuration: the FFmpeg backend when this build has it (feature
    /// `ffmpeg`), otherwise no media backend.
    #[must_use]
    pub fn native() -> FullConfig {
        let mut config = FullConfig::default();
        #[cfg(feature = "ffmpeg")]
        match cutweave_media::FfmpegBackend::new() {
            Ok(backend) => config.media = Some(Arc::new(backend)),
            Err(e) => config.media_problem = Some(format!("FFmpeg is unusable: {e}")),
        }
        #[cfg(not(feature = "ffmpeg"))]
        {
            config.media_problem =
                Some("this engine was built without FFmpeg (feature `ffmpeg`)".into());
        }
        config
    }

    /// With `backend` decoding, probing and encoding.
    #[must_use]
    pub fn with_media(mut self, backend: Arc<dyn MediaBackend>) -> FullConfig {
        self.media = Some(backend);
        self.media_problem = None;
        self
    }
}

/// The preview ring's announcement: JSON for `rpc.hello`, the name for the run-file.
#[derive(Clone, Debug)]
struct Announced {
    json: String,
    name: String,
}

impl Announced {
    fn of(ring: &RingAnnouncement) -> Announced {
        let json = serde_json::json!({
            "name": ring.name.as_str(),
            // A hex string: a u64 does not survive JSON numbers in JavaScript.
            "epoch": format!("0x{:016x}", ring.epoch),
            "layout_version": ring.layout_version,
            "mapping_size": ring.mapping_size,
        });
        Announced {
            json: json.to_string(),
            name: ring.name.as_str().to_owned(),
        }
    }
}

/// What runs between `start` and `shutdown`.
struct Started {
    mode: EngineMode,
    events: SubsystemEvents,
    link: EngineLink,
    caches: Caches,
    frames: Arc<MediaFrames>,
    renderer: SharedBackend,
    gpu: Arc<LateGpu>,
    /// Opens the GPU (joined at shutdown).
    gpu_thread: Option<JoinHandle<()>>,
    audio: Audio,
    /// The frame sink between documents (the ring publisher in `Gui` mode).
    sink_home: Option<Box<dyn FrameSink>>,
    clock_writer: Option<ClockWriter>,
    stages: Arc<AnalysisStages>,
    probe: Option<Arc<dyn MediaProbe>>,
    lost_reported: Arc<AtomicBool>,
    /// Warms the backend's capabilities (joined at shutdown).
    caps_thread: Option<JoinHandle<()>>,
}

/// Playback of the open document.
struct PerDoc {
    transport: Transport,
    playback: PlaybackEngine,
    pump: Option<cutweave_playback::shm::ClockPump>,
    watcher: DocWatcher,
}

struct Inner {
    config: FullConfig,
    doc: CurrentDoc,
    pcm: Arc<LivePcm>,
    frame_cache: Arc<FrameCache>,
    started: Mutex<Option<Started>>,
    per_doc: Mutex<Option<PerDoc>>,
    ring: Arc<Mutex<Option<Announced>>>,
    degraded: Mutex<Vec<(String, String)>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Records and sends `engine.degraded{subsystem, reason}`.
fn degrade(inner: &Inner, events: &SubsystemEvents, subsystem: &str, reason: &str) {
    lock(&inner.degraded).push((subsystem.to_owned(), reason.to_owned()));
    events.degraded(subsystem, reason);
}

/// What starts next to the GPU: the caches, audio and the preview ring.
type Devices = (Caches, Audio, Option<(Box<dyn FrameSink>, ClockWriter)>);

/// Media, render, audio and playback for the core engine (see the module docs).
pub struct FullSubsystems {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for FullSubsystems {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FullSubsystems")
            .field("config", &self.inner.config)
            .finish_non_exhaustive()
    }
}

/// A view into running [`FullSubsystems`] for tests and diagnostics (cheap to clone).
#[derive(Clone)]
pub struct FullHandle {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for FullHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FullHandle").finish_non_exhaustive()
    }
}

impl FullHandle {
    /// The render loop's counters of the open document's playback.
    #[must_use]
    pub fn playback_stats(&self) -> Option<PlaybackStats> {
        lock(&self.inner.per_doc)
            .as_ref()
            .map(|p| p.transport.stats())
    }

    /// The transport state of the open document's playback.
    #[must_use]
    pub fn playback_state(&self) -> Option<PlaybackState> {
        lock(&self.inner.per_doc)
            .as_ref()
            .map(|p| p.transport.state())
    }

    /// The `engine.degraded` reports so far, as `(subsystem, reason)`.
    #[must_use]
    pub fn degraded(&self) -> Vec<(String, String)> {
        lock(&self.inner.degraded).clone()
    }

    /// The preview ring's name (`Gui` mode).
    #[must_use]
    pub fn ring_name(&self) -> Option<String> {
        lock(&self.inner.ring).as_ref().map(|a| a.name.clone())
    }

    /// Whether the engine renders on a GPU device (else the CPU reference renderer, or the
    /// device is still opening).
    #[must_use]
    pub fn has_gpu(&self) -> bool {
        lock(&self.inner.started)
            .as_ref()
            .is_some_and(|s| s.gpu.has_gpu())
    }

    /// Whether the GPU finished opening (or is known to be absent).
    #[must_use]
    pub fn gpu_settled(&self) -> bool {
        lock(&self.inner.started)
            .as_ref()
            .is_some_and(|s| s.gpu.is_settled())
    }

    /// Media items with live decoder pools.
    #[must_use]
    pub fn decoder_pools(&self) -> usize {
        lock(&self.inner.started)
            .as_ref()
            .map_or(0, |s| s.frames.pool_count())
    }

    /// Whether playback is attached to a document.
    #[must_use]
    pub fn playing_a_document(&self) -> bool {
        lock(&self.inner.per_doc).is_some()
    }
}

impl FullSubsystems {
    /// Subsystems as `config` says; nothing starts before the engine calls `start`.
    #[must_use]
    pub fn new(config: FullConfig) -> FullSubsystems {
        let budget = config
            .frame_cache_bytes
            .unwrap_or_else(|| FrameCache::default_budget(total_ram()));
        FullSubsystems {
            inner: Arc::new(Inner {
                pcm: Arc::new(LivePcm::new(config.media.clone())),
                frame_cache: Arc::new(FrameCache::new(budget)),
                config,
                doc: CurrentDoc::default(),
                started: Mutex::new(None),
                per_doc: Mutex::new(None),
                ring: Arc::new(Mutex::new(None)),
                degraded: Mutex::new(Vec::new()),
            }),
        }
    }

    /// The real engine's subsystems ([`FullConfig::native`]).
    #[must_use]
    pub fn native() -> FullSubsystems {
        FullSubsystems::new(FullConfig::native())
    }

    /// A handle for tests and diagnostics.
    #[must_use]
    pub fn handle(&self) -> FullHandle {
        FullHandle {
            inner: Arc::clone(&self.inner),
        }
    }

    fn degrade(&self, events: &SubsystemEvents, subsystem: &str, reason: &str) {
        degrade(&self.inner, events, subsystem, reason);
    }

    /// Opens the wgpu device per `gpu` and `mode` on its own thread ([`gpu`] module docs).
    fn spawn_gpu(
        &self,
        host: &SubsystemHost,
    ) -> Result<(Arc<LateGpu>, JoinHandle<()>), SubsystemError> {
        let choice = match host.gpu {
            GpuConfig::Software => AdapterChoice::Software,
            GpuConfig::Auto => AdapterChoice::from_env_or(AdapterChoice::Auto),
        };
        let late = Arc::new(LateGpu::new());
        let gui = host.mode == EngineMode::Gui;
        let (inner, events, gpu) = (
            Arc::clone(&self.inner),
            host.events.clone(),
            Arc::clone(&late),
        );
        let thread = std::thread::Builder::new()
            .name("cw-gpu-init".into())
            .spawn(move || {
                let began = std::time::Instant::now();
                let opened = match gpu::open(&choice) {
                    Ok(opened) => Some(opened),
                    Err(reason) if gui && gpu.fail_start(&reason) => return,
                    Err(reason) => {
                        let software = (choice != AdapterChoice::Software)
                            .then(|| gpu::open(&AdapterChoice::Software).ok())
                            .flatten();
                        let fallback = if software.is_some() {
                            "rendering on the software adapter"
                        } else {
                            "rendering on the CPU (slow)"
                        };
                        degrade(&inner, &events, "gpu", &format!("{reason}; {fallback}"));
                        software
                    }
                };
                tracing::info!(
                    adapter = opened.as_ref().map(|(ctx, _, _)| ctx.info().key.clone()),
                    elapsed_ms = u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX),
                    "GPU ready"
                );
                gpu.settle(opened);
                // A frame shown while the GPU was opening was black: render it again.
                if let Some(per_doc) = lock(&inner.per_doc).as_ref() {
                    per_doc.transport.refresh();
                }
            })
            .map_err(|e| SubsystemError {
                kind: SubsystemKind::Gpu,
                message: format!("cannot start the GPU thread: {e}"),
            })?;
        Ok((late, thread))
    }

    /// The preview ring of a `Gui` engine, its clock writer and sink.
    fn start_ring(&self, host: &SubsystemHost) -> Option<(Box<dyn FrameSink>, ClockWriter)> {
        if host.mode != EngineMode::Gui {
            return None;
        }
        // Rings of engines that were killed (Linux; a no-op elsewhere).
        if let Err(e) = cutweave_playback::shm::reap_stale_rings() {
            tracing::debug!(error = %e, "cannot reap stale preview rings");
        }
        let (w, h) = self.inner.config.ring_size;
        let publisher = match FramePublisher::create(&RingConfig::new(w, h, PixelFormat::Rgba8)) {
            Ok(p) => p,
            Err(e) => {
                self.degrade(
                    &host.events,
                    "preview",
                    &format!("no preview ring ({e}); the preview falls back to JPEG frames"),
                );
                return None;
            }
        };
        *lock(&self.inner.ring) = Some(Announced::of(&publisher.announcement()));
        let writer = publisher.clock_writer();
        let ring = Arc::clone(&self.inner.ring);
        let link = host.link.clone();
        let sink = PublisherSink::new(publisher).on_ring_moved(Box::new(move |moved| {
            *lock(&ring) = Some(Announced::of(moved));
            link.preview_ring_changed();
        }));
        Some((Box::new(sink), writer))
    }

    /// Attaches playback to `doc` (with `started` locked).
    fn attach(&self, started: &mut Started, doc: &DocHandle) {
        let source = PreviewSource::new(
            doc.clone(),
            CompositorSlot::new(Arc::clone(&started.gpu)),
            Arc::clone(&started.frames),
            started.events.clone(),
            Arc::clone(&started.lost_reported),
        );
        let sink = started.sink_home.take().unwrap_or_else(|| {
            let (w, h) = self.inner.config.headless_preview;
            Box::new(NullSink::new(w, h))
        });
        let (audio, host): (Box<dyn AudioControl>, Arc<dyn HostClock>) = match &started.audio {
            Audio::Output { output, mixer } => (
                Box::new(OutputAudio::new(Arc::clone(output), Arc::clone(mixer))),
                Arc::new(SystemClock),
            ),
            Audio::Manual { clock, host } => (
                Box::new(ManualAudio::new(Arc::clone(clock))),
                Arc::new(host.clone()),
            ),
        };
        let config = PlaybackConfig {
            render: RenderConfig::default(),
            host,
        };
        let parts = match PlaybackParts::new(Box::new(source), audio, sink, config) {
            Ok(parts) => parts,
            Err(e) => {
                self.degrade(&started.events, "playback", &format!("no playback: {e}"));
                return;
            }
        };
        let events = started.events.clone();
        let publish = move |state: &PlaybackState| {
            events.publish(
                PLAYBACK_STATE_EVENT,
                serde_json::to_value(state).unwrap_or(Value::Null),
            );
        };
        let playback = match PlaybackEngine::spawn(parts, Box::new(publish)) {
            Ok(p) => p,
            Err(e) => {
                self.degrade(&started.events, "playback", &format!("no playback: {e}"));
                return;
            }
        };
        let pump = started.clock_writer.as_ref().and_then(|w| {
            playback
                .spawn_clock_pump(w.clone())
                .map_err(|e| tracing::warn!(error = %e, "no preview clock pump"))
                .ok()
        });
        let transport = playback.transport().clone();
        let targets = Targets {
            transport: transport.clone(),
            mixer: started.audio.mixer().cloned(),
            pcm: Arc::clone(&self.inner.pcm),
            frames: Arc::clone(&started.frames),
            thumbs: started.caches.thumbs.clone(),
        };
        let watcher = match DocWatcher::spawn(doc.clone(), targets) {
            Ok(w) => w,
            Err(e) => {
                tracing::error!(error = %e, "cannot watch the document; playback stops");
                Self::recover_sink(started, playback);
                return;
            }
        };
        *lock(&self.inner.per_doc) = Some(PerDoc {
            transport,
            playback,
            pump,
            watcher,
        });
    }

    /// Stops `playback` and keeps its sink for the next document.
    fn recover_sink(started: &mut Started, playback: PlaybackEngine) {
        if let Some(render) = playback.shutdown() {
            let (source, sink) = render.into_parts();
            // The source gives the compositor back.
            drop(source);
            started.sink_home = Some(sink);
        }
    }

    /// Detaches playback from the open document (with `started` locked, if it runs).
    fn detach(&self, started: Option<&mut Started>) {
        let per_doc = lock(&self.inner.per_doc).take();
        let Some(PerDoc {
            playback,
            pump,
            watcher,
            ..
        }) = per_doc
        else {
            return;
        };
        watcher.stop();
        if let Some(pump) = pump {
            pump.stop();
        }
        match started {
            Some(started) => {
                Self::recover_sink(started, playback);
                started.audio.idle();
                started.frames.clear();
            }
            None => drop(playback.shutdown()),
        }
    }

    /// Asks the backend for its capabilities on a worker thread, so the per-process hardware
    /// probes (encoders, WP-1E.02; decoders, WP-1D.04: seconds) run before the first import,
    /// scrub or export dialog needs them (`media.capabilities` then answers from the cache).
    fn warm_capabilities(&self) -> Option<JoinHandle<()>> {
        let backend = Arc::clone(self.inner.config.media.as_ref()?);
        std::thread::Builder::new()
            .name("cw-media-caps".into())
            .spawn(move || {
                let began = std::time::Instant::now();
                let caps = backend.capabilities();
                tracing::info!(
                    hw_encode = caps.hw_encode.len(),
                    hw_decode = caps.hw_decode.len(),
                    elapsed_ms = u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX),
                    "media capabilities probed"
                );
            })
            .map_err(|e| tracing::warn!(error = %e, "cannot probe media capabilities early"))
            .ok()
    }

    /// The analysis stages over `caches`: `index` → `thumbnails`, and `waveform`.
    fn analysis(&self, caches: &Caches) -> Arc<AnalysisStages> {
        let mut stages = AnalysisStages::new();
        let config = &self.inner.config;
        if !config.analysis {
            return Arc::new(stages);
        }
        let Some(backend) = &config.media else {
            return Arc::new(stages);
        };
        let mut add = |stage: Arc<dyn cutweave_ops::media::AnalysisStage>| {
            if let Err(e) = stages.add(stage) {
                tracing::error!(error = %e, "an analysis stage could not be registered");
            }
        };
        if let Some(db) = &caches.db {
            add(Arc::new(IndexStage {
                backend: Arc::clone(backend),
                db: Arc::clone(db),
            }));
            if let Some(thumbs) = &caches.thumbs {
                add(Arc::new(ThumbStage {
                    thumbs: Arc::clone(thumbs),
                }));
            }
        }
        if let Some(waveforms) = &caches.waveforms {
            add(Arc::new(WaveformStage {
                backend: Arc::clone(backend),
                waveforms: Arc::clone(waveforms),
            }));
        }
        Arc::new(stages)
    }

    /// The caches, audio and (in `Gui` mode) the preview ring.
    fn start_devices(&self, host: &SubsystemHost) -> Result<Devices, SubsystemError> {
        let config = &self.inner.config;
        if let Some(problem) = &config.media_problem {
            self.degrade(&host.events, "media", problem);
        }
        // Caches.
        let caches = match &config.media {
            Some(backend) => {
                let (caches, problem) = Caches::open(backend, config.cache_dir.as_deref());
                if let Some(problem) = problem {
                    self.degrade(&host.events, "cache", &problem);
                }
                caches
            }
            None => Caches::default(),
        };
        // Audio.
        let (audio, degraded) = audio::start(&config.audio, host.audio, &self.inner.pcm)?;
        if let Some(reason) = degraded {
            self.degrade(&host.events, "audio", &reason);
        }
        // Preview ring (Gui).
        let ring = self.start_ring(host);
        Ok((caches, audio, ring))
    }

    fn transport(&self) -> Option<Transport> {
        lock(&self.inner.per_doc)
            .as_ref()
            .map(|p| p.transport.clone())
    }
}

impl Subsystems for FullSubsystems {
    fn register(&self, registry: &mut Registry) -> Result<(), RegistryError> {
        cutweave_playback::transport::register(registry)?;
        // `render.contact_sheet` stays out of the MCP surface until P2 (WP-1J.02).
        registry.register::<RenderFrame>()?;
        Ok(())
    }

    fn start(&self, host: &SubsystemHost) -> Result<(), SubsystemError> {
        // The GPU device takes most of the start (about a second for the driver, far more on a
        // fresh user profile): it opens on its own thread while the caches, audio and the ring
        // start here, and the start waits for it only so long (`gpu` module docs).
        let (gpu, gpu_thread) = self.spawn_gpu(host)?;
        let rest = self.start_devices(host);
        let waited = gpu.wait_start(if host.mode == EngineMode::Gui {
            GPU_START_WAIT
        } else {
            std::time::Duration::ZERO
        });
        let failed = match (rest, waited) {
            (Ok(devices), Ok(())) => Ok(devices),
            (Err(e), _) => Err(e),
            (Ok(devices), Err(reason)) => {
                drop(devices);
                Err(SubsystemError {
                    kind: SubsystemKind::Gpu,
                    message: reason,
                })
            }
        };
        let (caches, audio, ring) = match failed {
            Ok(devices) => devices,
            Err(e) => {
                *lock(&self.inner.ring) = None;
                let _ = gpu_thread.join();
                gpu.close();
                return Err(e);
            }
        };
        let frames = Arc::new(MediaFrames::new(
            self.inner.config.media.clone(),
            self.inner.doc.clone(),
            Arc::clone(&self.inner.frame_cache),
            caches.db.clone(),
        ));
        let renderer: SharedBackend = Arc::new(Mutex::new(LateRenderer::new(
            Arc::clone(&gpu),
            Arc::clone(&frames),
        )));
        let (sink_home, clock_writer) = match ring {
            Some((sink, writer)) => (Some(sink), Some(writer)),
            None => (None, None),
        };
        let config = &self.inner.config;
        // Import.
        let probe = config.probe.clone().or_else(|| {
            config
                .media
                .as_ref()
                .map(|b| Arc::new(BackendProbe::new(Arc::clone(b))) as Arc<dyn MediaProbe>)
        });
        let stages = self.analysis(&caches);
        let mut started = Started {
            mode: host.mode,
            events: host.events.clone(),
            link: host.link.clone(),
            caches,
            frames,
            renderer,
            gpu,
            gpu_thread: Some(gpu_thread),
            audio,
            sink_home,
            clock_writer,
            stages,
            probe,
            lost_reported: Arc::new(AtomicBool::new(false)),
            caps_thread: self.warm_capabilities(),
        };
        if let Some(doc) = self.inner.doc.handle() {
            self.attach(&mut started, &doc);
        }
        tracing::info!(
            mode = started.mode.name(),
            gpu = started.gpu.adapter(),
            gpu_settled = started.gpu.is_settled(),
            audio = ?started.audio,
            ring = self.handle().ring_name(),
            "media, render, audio and playback started"
        );
        *lock(&self.inner.started) = Some(started);
        Ok(())
    }

    fn media_probe(&self) -> Option<Arc<dyn MediaProbe>> {
        lock(&self.inner.started)
            .as_ref()
            .and_then(|s| s.probe.clone())
    }

    fn analysis_stages(&self) -> Option<Arc<AnalysisStages>> {
        lock(&self.inner.started)
            .as_ref()
            .map(|s| Arc::clone(&s.stages))
    }

    fn export_services(&self) -> Option<ExportServices> {
        let started = lock(&self.inner.started);
        let started = started.as_ref()?;
        let backend = self.inner.config.media.as_ref()?;
        Some(ExportServices::new(
            Arc::clone(backend),
            Arc::new(DecoderSources::new(Arc::clone(backend))),
            Arc::clone(&started.renderer),
        ))
    }

    fn on_document(&self, doc: &DocHandle, project: Option<&ProjectLayout>) {
        let _ = project;
        self.inner.doc.set(Some(doc.clone()));
        let mut started = lock(&self.inner.started);
        if let Some(started) = started.as_mut() {
            self.attach(started, doc);
        }
    }

    fn on_document_closing(&self) {
        let mut started = lock(&self.inner.started);
        self.detach(started.as_mut());
        drop(started);
        self.inner.doc.set(None);
        self.inner.pcm.set_doc(None);
    }

    fn on_view(&self, view: &UiState, opened: bool) {
        if let Some(mixer) = lock(&self.inner.started)
            .as_ref()
            .and_then(|s| s.audio.mixer().cloned())
        {
            mixer.set_master_gain_linear(view.master_gain);
        }
        if opened && let Some(transport) = self.transport() {
            // Opening a project shows its saved playhead.
            transport.seek(view.playhead);
        }
    }

    fn invoke(&self, call: SubsystemCall<'_>) -> Option<Result<Value, CmdError>> {
        if call.context != TypeId::of::<Transport>() {
            return None;
        }
        let Some(mut transport) = self.transport() else {
            return Some(Err(CmdError::unsupported(
                "playback is not running (no document is open)",
            )));
        };
        // The document watcher follows edits asynchronously; a command right after an edit
        // (insert, then play) must already see the new extent.
        let timing = preview::timing_of(&call.doc.snapshot().doc);
        if timing != transport.timing() {
            transport.set_timing(timing);
        }
        let seek = call.name == "view.seek";
        let result = call.registry.invoke_engine_json(
            call.name,
            call.params,
            &mut transport,
            call.caller.access(),
        );
        // Keep the view's playhead where playback stopped (pause, step, seek, end).
        if let Ok(output) = &result
            && let Ok(state) = serde_json::from_value::<PlaybackState>(output.clone())
            && (seek || !state.playing)
            && let Some(link) = lock(&self.inner.started).as_ref().map(|s| s.link.clone())
            && let Err(e) = link.set_playhead(state.t, Some(call.caller.conn()))
        {
            tracing::debug!(error = %e, "cannot move the view playhead with the transport");
        }
        Some(result)
    }

    fn frame_backend(&self) -> Option<SharedBackend> {
        lock(&self.inner.started)
            .as_ref()
            .map(|s| Arc::clone(&s.renderer))
    }

    fn preview_ring(&self) -> Option<String> {
        lock(&self.inner.ring).as_ref().map(|a| a.json.clone())
    }

    fn preview_ring_name(&self) -> Option<String> {
        lock(&self.inner.ring).as_ref().map(|a| a.name.clone())
    }

    fn media_state(&self) -> Option<MediaState> {
        let started = lock(&self.inner.started);
        let started = started.as_ref()?;
        Some(MediaState {
            backend: Arc::clone(self.inner.config.media.as_ref()?),
            thumbs: started.caches.thumbs.clone(),
            waveforms: started.caches.waveforms.clone(),
            db: started.caches.db.clone(),
            frames: Arc::clone(&self.inner.frame_cache),
            renderer: Arc::clone(&started.renderer),
            doc: self.inner.doc.clone(),
        })
    }

    fn media_capabilities(&self) -> Option<cutweave_media::Capabilities> {
        self.inner
            .config
            .media
            .as_ref()
            .map(|backend| backend.capabilities())
    }

    fn shutdown(&self) {
        // 1. Playback (watcher, clock pump, render and status threads).
        let started = {
            let mut guard = lock(&self.inner.started);
            self.detach(guard.as_mut());
            guard.take()
        };
        self.inner.doc.set(None);
        self.inner.pcm.set_doc(None);
        let Some(started) = started else { return };
        let Started {
            frames,
            renderer,
            gpu,
            gpu_thread,
            caps_thread,
            audio,
            sink_home,
            clock_writer,
            caches,
            ..
        } = started;
        // 2. Audio: the output (device thread) before the mixer workers.
        audio.stop();
        // 3. The preview ring (marked closed for readers, then unmapped) and its clock.
        drop(clock_writer);
        drop(sink_home);
        *lock(&self.inner.ring) = None;
        // 4. Decoders, GPU (a device still opening is waited for), caches.
        frames.clear();
        for (name, thread) in [("GPU", gpu_thread), ("capabilities", caps_thread)] {
            if let Some(thread) = thread
                && thread.join().is_err()
            {
                tracing::error!("the {name} thread panicked");
            }
        }
        drop(renderer);
        gpu.close();
        self.inner.frame_cache.clear();
        drop(caches);
        tracing::info!("media, render, audio and playback stopped");
    }
}
