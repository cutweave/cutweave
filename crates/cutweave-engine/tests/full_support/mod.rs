// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared fixtures of the WP-1I.05 full-engine tests: engines with [`FullSubsystems`] (software
//! GPU, null or manual audio, caches in the test folder), a subsystems wrapper that records
//! the events the subsystems send while starting, and playback on a manual clock.

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::must_use_candidate
)]

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration as StdDuration, Instant};

use cutweave_audio::ManualClock;
use cutweave_commands::{CmdError, DocHandle, Registry, RegistryError};
use cutweave_engine::project_io::ProjectLayout;
use cutweave_engine::{
    AudioMode, Call, Connection, EngineConfig, EngineEvent, EngineMode, EventReceiver,
    ExportServices, FullConfig, FullHandle, FullSubsystems, GpuConfig, LogConfig, MediaState,
    SubsystemCall, SubsystemError, SubsystemHost, Subsystems,
};
use cutweave_model::{Duration, SampleRate};
use cutweave_ops::media::{AnalysisStages, MediaProbe};
use cutweave_ops::view::UiState;
use cutweave_playback::transport::{PlaybackStats, VirtualClock};
use cutweave_render::query::SharedBackend;
use serde_json::{Value, json};

/// Flicks per second.
pub const SEC: i64 = 705_600_000;
/// One frame at 30 fps, in flicks.
pub const FRAME_30: i64 = SEC / 30;
/// How long a test waits for anything.
pub const WAIT: StdDuration = StdDuration::from_secs(60);

/// Full subsystems that also record the events sent while they start (before any client can
/// subscribe): the engine's fan-out is subscribed in `start`, ahead of the real start.
pub struct Spy {
    pub inner: FullSubsystems,
    pub early: Arc<Mutex<Option<EventReceiver>>>,
}

impl Spy {
    pub fn new(config: FullConfig) -> (Spy, FullHandle, Arc<Mutex<Option<EventReceiver>>>) {
        let inner = FullSubsystems::new(config);
        let handle = inner.handle();
        let early = Arc::new(Mutex::new(None));
        (
            Spy {
                inner,
                early: Arc::clone(&early),
            },
            handle,
            early,
        )
    }
}

impl Subsystems for Spy {
    fn register(&self, registry: &mut Registry) -> Result<(), RegistryError> {
        self.inner.register(registry)
    }
    fn start(&self, host: &SubsystemHost) -> Result<(), SubsystemError> {
        *self.early.lock().unwrap() = Some(host.events.subscribe());
        self.inner.start(host)
    }
    fn media_probe(&self) -> Option<Arc<dyn MediaProbe>> {
        self.inner.media_probe()
    }
    fn analysis_stages(&self) -> Option<Arc<AnalysisStages>> {
        self.inner.analysis_stages()
    }
    fn export_services(&self) -> Option<ExportServices> {
        self.inner.export_services()
    }
    fn on_document(&self, doc: &DocHandle, project: Option<&ProjectLayout>) {
        self.inner.on_document(doc, project);
    }
    fn on_document_closing(&self) {
        self.inner.on_document_closing();
    }
    fn on_view(&self, view: &UiState, opened: bool) {
        self.inner.on_view(view, opened);
    }
    fn invoke(&self, call: SubsystemCall<'_>) -> Option<Result<Value, CmdError>> {
        self.inner.invoke(call)
    }
    fn frame_backend(&self) -> Option<SharedBackend> {
        self.inner.frame_backend()
    }
    fn preview_ring(&self) -> Option<String> {
        self.inner.preview_ring()
    }
    fn preview_ring_name(&self) -> Option<String> {
        self.inner.preview_ring_name()
    }
    fn media_state(&self) -> Option<MediaState> {
        self.inner.media_state()
    }
    fn media_capabilities(&self) -> Option<cutweave_media::Capabilities> {
        self.inner.media_capabilities()
    }
    fn gpu_adapter(&self) -> Option<cutweave_engine::media_caps::GpuAdapterInfo> {
        self.inner.gpu_adapter()
    }
    fn cache_control(&self) -> Option<Arc<dyn cutweave_engine::CacheControl>> {
        self.inner.cache_control()
    }
    fn shutdown(&self) {
        self.inner.shutdown();
    }
}

/// A manual master clock (48 kHz) and host clock for deterministic playback.
#[derive(Clone)]
pub struct Clocks {
    pub audio: Arc<ManualClock>,
    pub host: VirtualClock,
}

impl Clocks {
    pub fn new() -> Clocks {
        Clocks {
            audio: Arc::new(ManualClock::new(SampleRate::HZ_48_000)),
            host: VirtualClock::new(1_000_000_000),
        }
    }

    pub fn mode(&self) -> AudioMode {
        AudioMode::Manual {
            clock: Arc::clone(&self.audio),
            host: self.host.clone(),
        }
    }

    /// Moves both clocks forward by `flicks`.
    pub fn advance(&self, flicks: i64) {
        self.audio.advance(Duration::from_flicks(flicks).unwrap());
        self.host
            .advance(u64::try_from(i128::from(flicks) * 1_000_000_000 / i128::from(SEC)).unwrap());
    }
}

/// A full configuration for tests: caches under `dir`, small headless previews.
pub fn full_config(dir: &Path, audio: AudioMode) -> FullConfig {
    FullConfig {
        cache_dir: Some(dir.join("cache")),
        audio,
        headless_preview: (160, 90),
        ..FullConfig::default()
    }
}

/// An engine configuration for `subsystems` in `mode` (software GPU, null audio, agents may
/// use `dir`).
pub fn engine_config(
    dir: &Path,
    mode: EngineMode,
    subsystems: Box<dyn Subsystems>,
) -> EngineConfig {
    let mut config = EngineConfig::new(mode);
    config.log = LogConfig::disabled();
    config.allowed_roots = vec![dir.to_path_buf()];
    config.gpu = GpuConfig::Software;
    config.audio = cutweave_engine::AudioConfig::Null;
    config.subsystems = Some(subsystems);
    if mode != EngineMode::Oneshot {
        config.port = Some(0);
        config.api.run_dir = Some(dir.join("run"));
    }
    config
}

/// Calls `name`, returning the output or the error.
pub fn call(conn: &Connection, name: &str, params: Value) -> Result<Value, CmdError> {
    conn.call(Call::new(name, params)).map(|r| r.output)
}

/// Polls `f` until it returns `Some` (or panics after [`WAIT`]).
pub fn eventually<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(StdDuration::from_millis(5));
    }
}

/// Delivered frames so far.
pub fn delivered(handle: &FullHandle) -> u64 {
    handle
        .playback_stats()
        .map_or(0, |s: PlaybackStats| s.delivered)
}

/// Plays `frames` frames at 30 fps on the manual clocks, one frame at a time, waiting for each
/// to be delivered (so none is dropped).
pub fn play_frames(conn: &Connection, handle: &FullHandle, clocks: &Clocks, frames: u32) -> Value {
    let state = call(conn, "view.play", json!({})).unwrap();
    assert_eq!(state["playing"], json!(true), "{state}");
    for _ in 0..frames {
        let before = delivered(handle);
        clocks.advance(FRAME_30);
        eventually("the next playback frame", || {
            (delivered(handle) > before).then_some(())
        });
    }
    state
}

/// The test-media corpus: `CUTWEAVE_CORPUS_DIR`, else `testdata/corpus` of this checkout, else
/// that of the main checkout when this is a git worktree.
pub fn corpus_dir() -> Option<std::path::PathBuf> {
    if let Some(dir) = std::env::var_os("CUTWEAVE_CORPUS_DIR") {
        let dir = std::path::PathBuf::from(dir);
        return dir.is_dir().then_some(dir);
    }
    // The workspace root without `..`: allowed roots never match a path with `..` in it
    // (`AllowedRoots::allows`), so the corpus folder must be spelled out.
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2)?;
    let local = root.join("testdata/corpus");
    if local.is_dir() {
        return Some(local);
    }
    // A worktree's `.git` is a file: `gitdir: <main>/.git/worktrees/<name>`.
    let gitdir = std::fs::read_to_string(root.join(".git")).ok()?;
    let gitdir = std::path::PathBuf::from(gitdir.strip_prefix("gitdir:")?.trim());
    let main = gitdir.parent()?.parent()?.parent()?;
    let dir = main.join("testdata/corpus");
    dir.is_dir().then_some(dir)
}

/// A corpus file by name, or `None` with a skip message.
pub fn corpus_file(name: &str) -> Option<std::path::PathBuf> {
    let Some(dir) = corpus_dir() else {
        eprintln!("skipping: no corpus (cargo xtask fetch-corpus --tier small)");
        return None;
    };
    let path = dir.join(name);
    if path.is_file() {
        Some(path)
    } else {
        eprintln!("skipping: {name} is not in {}", dir.display());
        None
    }
}

/// The bundle's (or `PATH`'s) FFmpeg tool `name` (`ffprobe`), if it runs.
pub fn ffmpeg_tool(name: &str) -> Option<std::path::PathBuf> {
    let exe = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    let in_bundle = std::env::var_os("FFMPEG_DIR")
        .map(|d| std::path::PathBuf::from(d).join("bin").join(&exe))
        .filter(|p| p.is_file());
    let tool = in_bundle.unwrap_or_else(|| std::path::PathBuf::from(&exe));
    std::process::Command::new(&tool)
        .arg("-version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| tool)
}

/// Every event queued in `rx` now.
pub fn drain(rx: &mut EventReceiver) -> Vec<EngineEvent> {
    let mut out = Vec::new();
    while let Ok(e) = rx.try_recv() {
        out.push(e);
    }
    out
}

/// The `engine.degraded` payloads among `events`.
pub fn degraded(events: &[EngineEvent]) -> Vec<Value> {
    events
        .iter()
        .filter(|e| e.name() == "engine.degraded")
        .map(EngineEvent::payload)
        .collect()
}
