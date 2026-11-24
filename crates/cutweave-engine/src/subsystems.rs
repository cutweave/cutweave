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
//!   project closes.
//!
//! Methods take `&self` and are called from any thread (implementations use interior
//! mutability); no engine lock is held while they run, except the session lock during the
//! per-project hooks (they must not call back into the engine's dispatch).

use std::any::TypeId;
use std::sync::Arc;

use cutweave_commands::{ActorCtx, CmdError, DocHandle, Registry, RegistryError};
use cutweave_ops::media::{AnalysisStages, MediaProbe};
use serde_json::Value;

use crate::config::{AudioConfig, EngineMode, GpuConfig};
use crate::events::{EngineEvent, EventBus};
use crate::export::ExportServices;
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
}

/// Publishes subsystem events into the engine fan-out.
#[derive(Clone, Debug)]
pub struct SubsystemEvents(pub(crate) EventBus);

impl SubsystemEvents {
    /// Sends `name` with `payload` (a JSON object) to every subscriber.
    pub fn publish(&self, name: &'static str, payload: Value) {
        self.0.publish(EngineEvent::Subsystem { name, payload });
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
