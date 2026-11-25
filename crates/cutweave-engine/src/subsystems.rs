// SPDX-License-Identifier: GPL-3.0-or-later
//! The [`Subsystems`] extension point (WP-1I.04): how WP-1I.05 attaches media, render, audio
//! and playback to the core engine.
//!
//! The core engine serves every document command, the `job.*`, `view.*` and `project.*`
//! commands and `media.import` (whose probe has no media backend without a subsystem: files
//! end up in `failed[]`). A [`Subsystems`] implementation adds:
//!
//! - commands ([`register`](Subsystems::register)) whose engine-command context it owns and
//!   runs ([`invoke`](Subsystems::invoke)): `render.frame`, transport;
//! - what `export.start` renders, decodes and encodes with
//!   ([`export_services`](Subsystems::export_services); the core runs the command itself);
//! - the media prober and the analysis stages `media.import` uses;
//! - devices started after the core is up ([`start`](Subsystems::start); a GPU failure in `Gui`
//!   mode exits with code 11) and stopped in order before it goes down
//!   ([`shutdown`](Subsystems::shutdown));
//! - per-project hooks: the new document handle after every document swap, teardown before a
//!   project closes, and the view state ([`on_view`](Subsystems::on_view): master volume, the
//!   saved playhead);
//! - the renderer the `render.*` queries run on ([`frame_backend`](Subsystems::frame_backend)),
//!   the preview ring to announce ([`preview_ring`](Subsystems::preview_ring)) and the state
//!   the media HTTP endpoints serve ([`media_state`](Subsystems::media_state), WP-1I.03).
//!
//! The full engine's implementation is [`FullSubsystems`](crate::full::FullSubsystems)
//! (WP-1I.05).
//!
//! Methods take `&self` and are called from any thread (implementations use interior
//! mutability); no engine lock is held while they run, except the session lock during the
//! per-project hooks (they must not call back into the engine's dispatch, nor use the
//! [`EngineLink`]).

use std::any::TypeId;
use std::sync::{Arc, Weak};

use cutweave_commands::{ActorCtx, CmdError, ConnId, DocHandle, Registry, RegistryError};
use cutweave_model::Time;
use cutweave_ops::media::{AnalysisStages, MediaProbe, ProxyService};
use cutweave_ops::view::UiState;
use cutweave_render::query::SharedBackend;
use serde_json::Value;

use crate::config::{AudioConfig, EngineMode, GpuConfig};
use crate::engine::Core;
use crate::events::{EngineEvent, EventBus, EventReceiver};
use crate::export::ExportServices;
use crate::full::MediaState;
use crate::project_io::ProjectLayout;

/// Which subsystem failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubsystemKind {
    /// GPU initialization (exit code 11 in `Gui` mode).
    Gpu,
    /// Audio output.
    Audio,
    /// Anything else.
    Other,
}

/// A subsystem could not start.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{kind:?} subsystem failed: {message}")]
pub struct SubsystemError {
    /// Which one.
    pub kind: SubsystemKind,
    /// Why (no secrets).
    pub message: String,
}

/// What subsystems get when they start.
#[derive(Clone, Debug)]
pub struct SubsystemHost {
    /// How the engine runs.
    pub mode: EngineMode,
    /// `EngineConfig::audio`.
    pub audio: AudioConfig,
    /// `EngineConfig::gpu`.
    pub gpu: GpuConfig,
    /// `EngineConfig::deterministic_ids` (WP-1B.08: seed render/playback ids from it too).
    pub deterministic_ids: Option<u64>,
    /// Publishes subsystem events (`playback.state`, `engine.degraded`).
    pub events: SubsystemEvents,
    /// The engine runtime (for async work; never block its workers).
    pub runtime: tokio::runtime::Handle,
    /// What a subsystem may ask of the engine later (never from the per-project hooks).
    pub link: EngineLink,
}

/// The `engine.degraded` event: a subsystem runs in a reduced mode (arch §8, §9; WP-1I.05).
pub const DEGRADED_EVENT: &str = "engine.degraded";

/// The events subsystems emit, as `(name, summary)` (for the generated events catalog).
pub const SUBSYSTEM_EVENTS: &[(&str, &str)] = &[(
    DEGRADED_EVENT,
    "A subsystem runs in a reduced mode (audio on the null clock, software rendering); payload: {subsystem, reason}",
)];

/// Publishes subsystem events into the engine fan-out.
#[derive(Clone, Debug)]
pub struct SubsystemEvents(pub(crate) EventBus);

impl SubsystemEvents {
    /// Sends `name` with `payload` (a JSON object) to every subscriber.
    pub fn publish(&self, name: &'static str, payload: Value) {
        self.0.publish(EngineEvent::Subsystem { name, payload });
    }

    /// Sends `engine.degraded{subsystem, reason}` (and logs it as a warning).
    pub fn degraded(&self, subsystem: &str, reason: &str) {
        tracing::warn!(subsystem, reason, "engine degraded");
        self.publish(
            DEGRADED_EVENT,
            serde_json::json!({ "subsystem": subsystem, "reason": reason }),
        );
    }

    /// A subscription to the whole engine fan-out (document, job, view and subsystem events),
    /// starting with the next event.
    #[must_use]
    pub fn subscribe(&self) -> EventReceiver {
        self.0.subscribe()
    }
}

/// What a running subsystem may ask of the engine: held weakly, so calls after the engine
/// stopped do nothing. Never call it from the per-project hooks (they run under the session
/// lock).
#[derive(Clone)]
pub struct EngineLink {
    pub(crate) core: Weak<Core>,
}

impl std::fmt::Debug for EngineLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineLink")
            .field("alive", &(self.core.strong_count() > 0))
            .finish()
    }
}

impl EngineLink {
    /// A link to nothing (tests of a subsystem outside an engine).
    #[must_use]
    pub fn detached() -> EngineLink {
        EngineLink { core: Weak::new() }
    }

    /// Moves the view state's playhead to `t` with the registered `view.set_playhead`
    /// command, announcing `view.changed` to every client but `origin`.
    pub fn set_playhead(&self, t: Time, origin: Option<ConnId>) -> Result<(), CmdError> {
        match self.core.upgrade() {
            Some(core) => core.set_view_playhead(t, origin),
            None => Ok(()),
        }
    }

    /// The engine's job manager (proxy jobs, WP-2F.01); `None` once the engine stopped.
    #[must_use]
    pub fn jobs(&self) -> Option<cutweave_jobs::JobManager> {
        self.core.upgrade().map(|core| core.deps.jobs.clone())
    }

    /// The preview ring moved: the run-file's `shm` is rewritten (off the calling thread).
    pub fn preview_ring_changed(&self) {
        if let Some(core) = self.core.upgrade() {
            core.preview_ring_changed();
        }
    }
}

/// An engine command routed to a subsystem.
#[derive(Debug)]
pub struct SubsystemCall<'a> {
    /// The command's name.
    pub name: &'a str,
    /// Its params (an object).
    pub params: Value,
    /// The `TypeId` of its engine-command context.
    pub context: TypeId,
    /// The caller (actor, connection, access).
    pub caller: &'a ActorCtx,
    /// The registry (to run the command against the subsystem's context with
    /// [`Registry::invoke_engine_json`]).
    pub registry: &'a Registry,
    /// The current document.
    pub doc: &'a DocHandle,
}

/// The extension point WP-1I.05 implements (see the module docs). Every method has a default,
/// so the core engine runs with [`CoreOnly`].
pub trait Subsystems: Send + Sync {
    /// Registers the subsystems' commands (called once, before the registry is frozen).
    fn register(&self, registry: &mut Registry) -> Result<(), RegistryError> {
        let _ = registry;
        Ok(())
    }

    /// Starts devices once the core is up (before the API server binds).
    fn start(&self, host: &SubsystemHost) -> Result<(), SubsystemError> {
        let _ = host;
        Ok(())
    }

    /// The prober `media.import` uses; `None`: no media backend (imports report every file as
    /// unreadable).
    fn media_probe(&self) -> Option<Arc<dyn MediaProbe>> {
        None
    }

    /// The analysis stages run on newly imported media (none by default).
    fn analysis_stages(&self) -> Option<Arc<AnalysisStages>> {
        None
    }

    /// What `export.start` renders, decodes and encodes with: the media backend's encoder, the
    /// clip sources and the renderer shared with `render.frame` (its lock is the GPU budget).
    /// Asked on every `export.start`; `None` (the default): exports fail with `Unsupported`.
    fn export_services(&self) -> Option<ExportServices> {
        None
    }

    /// A document was installed: at start, after `project.open` / `project.close` /
    /// `project.recover` / `project.discard_recovery` and after saving an untitled document.
    /// `project` is the project folder, `None` for an untitled document.
    fn on_document(&self, doc: &DocHandle, project: Option<&ProjectLayout>) {
        let _ = (doc, project);
    }

    /// The current document is about to be replaced or closed (stop playback, drop
    /// per-project caches).
    fn on_document_closing(&self) {}

    /// The view state: `opened` right after a document was installed (and once after
    /// [`start`](Self::start)), else after a `view.*` command changed it. Called under the
    /// session lock when `opened`.
    fn on_view(&self, view: &UiState, opened: bool) {
        let _ = (view, opened);
    }

    /// The renderer the `render.*` queries use on the dispatching thread
    /// (`cutweave_render::query::enter`); `None`: they fail with `Unsupported`.
    fn frame_backend(&self) -> Option<SharedBackend> {
        None
    }

    /// The preview ring to announce in `rpc.hello` (`shm`): its announcement as JSON text
    /// `{name, epoch, layout_version, mapping_size}`; `None` without a ring.
    fn preview_ring(&self) -> Option<String> {
        None
    }

    /// The name of the preview ring, for the run-file's `shm`.
    fn preview_ring_name(&self) -> Option<String> {
        None
    }

    /// The media services the `/media` HTTP endpoints serve (WP-1I.03); `None` without them.
    fn media_state(&self) -> Option<MediaState> {
        None
    }

    /// What the media backend can decode and encode (`media.capabilities`); `None`: no
    /// backend.
    fn media_capabilities(&self) -> Option<cutweave_media::Capabilities> {
        None
    }

    /// The graphics adapter the renderer runs on, once it is open (`media.capabilities`);
    /// `None`: not open (yet), or the CPU renderer.
    fn gpu_adapter(&self) -> Option<crate::media_caps::GpuAdapterInfo> {
        None
    }

    /// The media caches and decoding policy the cache commands act on (`media.cache_status`
    /// and the UI-only `media.set_cache_config`, `media.clear_cache`,
    /// `media.set_decode_policy`; WP-1D.08); `None`: this engine has none.
    fn cache_control(&self) -> Option<Arc<dyn crate::media_cache::CacheControl>> {
        None
    }

    /// The proxies `media.import` queues and the proxy commands act on (WP-2F.01); `None`:
    /// this engine makes none (`media.proxy_status` reports none, `media.generate_proxy` is
    /// unsupported).
    fn proxy_service(&self) -> Option<Arc<dyn ProxyService>> {
        None
    }

    /// Runs an engine command whose context this implementation owns; `None` if it does not.
    fn invoke(&self, call: SubsystemCall<'_>) -> Option<Result<Value, CmdError>> {
        let _ = call;
        None
    }

    /// Stops everything in order (playback, render and audio threads, caches). Called once,
    /// after the API server stopped and before the document closes.
    fn shutdown(&self) {}
}

/// The core engine: no subsystem attached.
#[derive(Clone, Copy, Debug, Default)]
pub struct CoreOnly;

impl Subsystems for CoreOnly {}
