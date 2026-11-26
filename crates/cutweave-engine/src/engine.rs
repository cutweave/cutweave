// SPDX-License-Identifier: GPL-3.0-or-later
//! [`Engine`]: the composition root (WP-1I.04; arch §2, §3, §5, §6, §14, §17).
//!
//! [`Engine::start`] wires the core: the registry (`cutweave-ops` + the `edit.*` commands +
//! the session commands + whatever the [`Subsystems`] add), the session (doc actor, project
//! store, journal, autosave, view state), the [`JobManager`] on the engine runtime, the event
//! fan-out, and the local API server (WP-1I.01) with the routes WP-1I.02/1J.01/1I.03 mount.
//! Clients talk to it through an [`EngineHandle`]: [`EngineHandle::connect`] gives a
//! [`Connection`](crate::Connection) per client connection (the in-process dispatch API the
//! RPC layer mounts).
//!
//! # Threads
//!
//! The engine runtime (`cutweave-rt-*`, [`EngineConfig::worker_threads`] async workers plus
//! tokio's blocking pool) runs the API server, job coordination and blocking dispatch from
//! async callers. The doc actor (`cutweave-doc`), its event forwarder, the autosave thread, the
//! project event forwarder and the journal flusher are plain threads of the session. Everything
//! is joined by [`Engine::shutdown`].
//!
//! # Shutdown
//!
//! [`Engine::shutdown`] (also on drop): refuses new calls, stops the API server (removing the
//! run-file), shuts the subsystems down, cancels jobs, then closes the session **without
//! saving**: the doc actor drains its queue, the journal is synced and closed, the view state
//! is flushed and the project lock released; finally the runtime stops.

use std::any::TypeId;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, Instant};

use cutweave_api::{
    ApiConfig, ApiError, ApiServer, Credential, Credentials, RunFile, RunFileContents,
    RunFileProject, RunningServer,
};
use cutweave_commands::{
    Access, Actor, ActorCtx, Clock, CmdError, ConnId, DocHandle, DocSnapshot, ManualClock,
    Registry, RegistryError, Rev, SystemClock, WallTime,
};
use cutweave_jobs::queue::Attach;
use cutweave_jobs::{JobKind, JobManager};
use cutweave_model::{SeededGen, Time};
use cutweave_ops::audio::{LevelsAnalyzer, LevelsCtx};
use cutweave_ops::export::ExportCtx;
use cutweave_ops::export::presets::{PresetCtx, PresetStore};
use cutweave_ops::media::proxy::ProxyCall;
use cutweave_ops::media::{
    AllowedRoots, AnalysisStages, Analyzer, DiskFiles, ImportCtx, ImportPolicy, OUTSIDE_ROOTS_HINT,
    RelinkCtx,
};
use cutweave_ops::project::collect::CollectCtx;
use cutweave_ops::view::{UiState, ViewCtx};
use serde_json::Value;
use tokio::runtime::Runtime;
use tokio::sync::broadcast::error::RecvError as BroadcastRecvError;

use crate::agents::Connections;
use crate::autosave::AutosavePreference;
use crate::collect::EngineCollectHost;
use crate::config::{EngineConfig, EngineMode};
use crate::dispatch::{Connection, Reply};
use crate::events::{EngineEvent, EventBus, EventReceiver};
use crate::export::{ExportJobs, Starter, export_queue, queue_store};
use crate::full::MediaState;
use crate::import::{BusAnalysisSink, ImportSink, NoMediaBackend, ctx_for};
use crate::journal::{IdState, StopReason};
use crate::levels::EngineLevels;
use crate::logging::{self, LogGuard};
use crate::media_cache::CacheCall;
use crate::media_caps::CapabilitiesCall;
use crate::project_io::{LockInfo, ProjectIoError, ProjectStore};
use crate::relink::RelinkTx;
use crate::session::{
    self, Deps, ProjectCloseOutput, ProjectOpenOutput, ProjectRecoverOutput, ProjectSaveOutput,
    ProjectStatus, Session, SessionCall, SessionState,
};
use crate::subsystems::{
    CoreOnly, EngineLink, SubsystemCall, SubsystemError, SubsystemEvents, SubsystemHost,
};

/// Mixed into the `--deterministic-ids` seed for job ids: a stream separate from the
/// document's, so job timing never shifts document ids (WP-1D.07).
pub const JOB_ID_STREAM: u64 = 0x4A4F_4253_5354_524D; // "JOBSSTRM"

/// Engine connections are numbered from here, apart from the per-request numbers the API
/// server assigns for its own routes (connection ids must never be reused, WP-1B.09).
const FIRST_CONN: u64 = 1 << 32;

/// How long the runtime waits for blocking tasks when the engine stops.
const RUNTIME_SHUTDOWN: Duration = Duration::from_secs(5);

/// What a refused `project.open` can pass instead (ADR-0029).
const OPEN_ANYWAY: &str = "pass discard_unsaved: true to drop them";

/// What a refused `project.close` can pass instead (ADR-0029).
const CLOSE_ANYWAY: &str = "pass save: false to drop them";

/// The engine version (the readiness line, the run-file).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Why the engine could not start; [`exit_code`](Self::exit_code) is the launch-protocol code.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StartError {
    /// Another process holds the project (exit code 10).
    #[error("{message}")]
    ProjectLocked {
        /// The project path as given.
        path: PathBuf,
        /// Who holds it, when known.
        holder: Option<LockInfo>,
        /// What to tell the user.
        message: String,
    },
    /// The project could not be opened (not found, unreadable, a newer lock from another
    /// host…).
    #[error("cannot open the project {}: {source}", path.display())]
    Project {
        /// The project path as given.
        path: PathBuf,
        /// Why.
        #[source]
        source: ProjectIoError,
    },
    /// A subsystem failed (a GPU failure is exit code 11).
    #[error(transparent)]
    Subsystem(#[from] SubsystemError),
    /// The API server could not bind its port (exit code 12) or start.
    #[error(transparent)]
    Api(#[from] ApiError),
    /// The UI credential is missing (`Gui`) or not allowed (headless) (exit code 13).
    #[error("bad handshake: {0}")]
    Handshake(String),
    /// The configuration contradicts itself (exit code 2).
    #[error("invalid engine configuration: {0}")]
    Config(String),
    /// A registration broke the command contract (a bug).
    #[error("command registry: {0}")]
    Registry(#[from] RegistryError),
    /// The session could not be built.
    #[error("cannot start the session: {0}")]
    Session(CmdError),
    /// The runtime, a thread or the log file could not be created.
    #[error("cannot start the engine: {0}")]
    Io(#[from] std::io::Error),
}

impl StartError {
    /// The process exit code (launch protocol): 10 project locked, 11 GPU init failed, 12 port
    /// bind error, 13 bad handshake, 2 bad configuration, 1 anything else.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        match self {
            StartError::ProjectLocked { .. } => crate::launch::EXIT_PROJECT_LOCKED,
            StartError::Subsystem(e) if e.kind == crate::subsystems::SubsystemKind::Gpu => {
                crate::launch::EXIT_GPU_INIT_FAILED
            }
            StartError::Api(e) => e.exit_code(),
            StartError::Handshake(_) => crate::launch::EXIT_HANDSHAKE,
            StartError::Config(_) => crate::launch::EXIT_USAGE,
            _ => crate::launch::EXIT_FAILURE,
        }
    }

    /// The machine code of the launch protocol's error line.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            StartError::ProjectLocked { .. } => "project_locked",
            StartError::Subsystem(e) if e.kind == crate::subsystems::SubsystemKind::Gpu => {
                "gpu_init_failed"
            }
            StartError::Subsystem(_) => "subsystem_failed",
            StartError::Api(ApiError::Bind { .. }) => "port_in_use",
            StartError::Api(_) => "server_failed",
            StartError::Handshake(_) => "bad_handshake",
            StartError::Config(_) => "usage",
            StartError::Project { .. } => "project_open_failed",
            StartError::Registry(_) | StartError::Session(_) | StartError::Io(_) => "start_failed",
        }
    }

    fn from_project(path: &Path, e: ProjectIoError) -> StartError {
        match e {
            ProjectIoError::Locked { holder, reason } => StartError::ProjectLocked {
                path: path.to_path_buf(),
                message: ProjectIoError::Locked {
                    holder: holder.clone(),
                    reason,
                }
                .to_string(),
                holder,
            },
            source => StartError::Project {
                path: path.to_path_buf(),
                source,
            },
        }
    }
}

/// The shared engine state behind every [`EngineHandle`] and [`Connection`].
pub(crate) struct Core {
    pub(crate) mode: EngineMode,
    pub(crate) deps: Deps,
    /// The folders agents and other non-UI callers may use besides the project folders: the
    /// `--allow-root` ones until `project.set_allowed_roots` replaces them (WP-1J.05). Read on
    /// every call that takes a path, so a change applies at once, also to open connections.
    approved_roots: RwLock<Vec<PathBuf>>,
    /// The folder of the project the engine was started with: usable for the engine's lifetime
    /// (the launcher chose it), also after it is closed.
    start_root: Option<PathBuf>,
    /// The agent connections open now (`project.connections`).
    pub(crate) connections: Connections,
    pub(crate) runtime: tokio::runtime::Handle,
    session: RwLock<Option<Session>>,
    state: AtomicU8,
    next_conn: AtomicU64,
    stopping: AtomicBool,
    pub(crate) has_ui: bool,
    /// The export queue (WP-2G.03a): its store follows the session (the project's
    /// `.cutweave/exports.sqlite`, or memory for an untitled one).
    pub(crate) exports: ExportJobs,
    /// The export presets: built-in and the user's (WP-2G.01).
    presets: Arc<PresetStore>,
    /// Free disk space of a folder, for the exports' disk check (WP-2G.03b).
    free_space: crate::export::FreeSpaceProbe,
    /// The run-file, once the server runs (its `shm` follows the preview ring).
    run_file: Mutex<Option<PathBuf>>,
}

fn state_code(s: SessionState) -> u8 {
    match s {
        SessionState::NoProject => 0,
        SessionState::Open => 1,
        SessionState::Closing => 2,
    }
}

fn state_of(code: u8) -> SessionState {
    match code {
        1 => SessionState::Open,
        2 => SessionState::Closing,
        _ => SessionState::NoProject,
    }
}

fn stopped() -> CmdError {
    CmdError::unsupported("the engine is shutting down")
}

impl Core {
    /// The export queue on `deps`' job manager, `slots` exports at a time (WP-2G.03a).
    fn new_exports(deps: &Deps, slots: usize) -> ExportJobs {
        export_queue(deps.jobs.clone(), Arc::clone(&deps.subsystems), slots)
    }

    /// Points the export queue at `session`'s store (see [`crate::export`]).
    fn attach_exports(&self, session: &Session, mode: Attach) {
        self.exports
            .attach(queue_store(session.export_meta_dir().as_deref()), mode);
    }

    /// Installs the engine's first session, with its export queue.
    fn install_first(&self, first: Session) {
        self.settle_state(&first);
        self.attach_exports(&first, Attach::Suspend);
        *self.session.write().unwrap_or_else(PoisonError::into_inner) = Some(first);
    }

    fn set_state(&self, s: SessionState) {
        self.state.store(state_code(s), Ordering::Release);
    }

    fn settle_state(&self, session: &Session) {
        self.set_state(if session.project_root().is_some() {
            SessionState::Open
        } else {
            SessionState::NoProject
        });
    }

    /// The session, for a request (shared).
    pub(crate) fn session(&self) -> Result<RwLockReadGuard<'_, Option<Session>>, CmdError> {
        if self.stopping.load(Ordering::Acquire) {
            return Err(stopped());
        }
        let guard = self.session.read().unwrap_or_else(PoisonError::into_inner);
        if guard.is_none() {
            return Err(stopped());
        }
        Ok(guard)
    }

    fn session_mut(&self) -> Result<RwLockWriteGuard<'_, Option<Session>>, CmdError> {
        if self.stopping.load(Ordering::Acquire) {
            return Err(stopped());
        }
        let guard = self.session.write().unwrap_or_else(PoisonError::into_inner);
        if guard.is_none() {
            return Err(stopped());
        }
        Ok(guard)
    }

    /// A fresh connection id (never reused).
    pub(crate) fn next_conn(&self) -> ConnId {
        ConnId::new(self.next_conn.fetch_add(1, Ordering::Relaxed))
    }

    /// The current document handle.
    pub(crate) fn doc(&self) -> Result<DocHandle, CmdError> {
        let guard = self.session()?;
        guard
            .as_ref()
            .map(|s| s.handle().clone())
            .ok_or_else(stopped)
    }

    /// A reply carrying the current revisions.
    pub(crate) fn reply(&self, output: Value) -> Reply {
        let snap = self.doc().map(|d| d.snapshot()).ok();
        Reply {
            output,
            rev: snap.as_ref().map_or(Rev::ZERO, |s| s.rev),
            committed_rev: snap.as_ref().map_or(Rev::ZERO, |s| s.committed_rev),
            applied: None,
        }
    }

    // ----- engine commands ---------------------------------------------------------------------

    /// Runs an engine command whose context is not the doc actor's.
    pub(crate) fn run_engine(
        self: &Arc<Self>,
        ctx: &ActorCtx,
        name: &str,
        params: Value,
        context: TypeId,
    ) -> Result<Reply, CmdError> {
        let registry = &self.deps.registry;
        if context == TypeId::of::<JobManager>() {
            // `job.wait` blocks this (non-async) thread for up to 60 s: fine here, callers on
            // async workers use `Connection::call_async`.
            let mut jobs = self.deps.jobs.clone();
            let output = registry.invoke_engine_json(name, params, &mut jobs, ctx.access())?;
            return Ok(self.reply(output));
        }
        if context == TypeId::of::<ImportCtx>() {
            return self.import(ctx, name, params);
        }
        if context == TypeId::of::<RelinkCtx>() {
            return self.relink(ctx, name, params);
        }
        if context == TypeId::of::<CollectCtx>() {
            return self.collect(ctx, name, params);
        }
        if context == TypeId::of::<ExportCtx>() {
            return self.export(ctx, name, params);
        }
        if context == TypeId::of::<LevelsCtx>() {
            // `audio.get_levels` decodes and mixes the range on this (non-async) thread, like
            // `job.wait` blocks it; the document of the moment is measured.
            let mut call = LevelsCtx {
                snapshot: self.doc()?.snapshot(),
                analyzer: self.deps.subsystems.export_services().map(|services| {
                    Arc::new(EngineLevels::new(services.sources)) as Arc<dyn LevelsAnalyzer>
                }),
            };
            let output = registry.invoke_engine_json(name, params, &mut call, ctx.access())?;
            return Ok(self.reply(output));
        }
        if context == TypeId::of::<PresetCtx>() {
            let mut call = PresetCtx {
                store: Arc::clone(&self.presets),
                snapshot: self.session().ok().and_then(|guard| {
                    guard
                        .as_ref()
                        .map(|session| session.handle().committed_snapshot())
                }),
            };
            let output = registry.invoke_engine_json(name, params, &mut call, ctx.access())?;
            return Ok(self.reply(output));
        }
        if context == TypeId::of::<ViewCtx>() {
            return self.view(ctx, name, params);
        }
        if context == TypeId::of::<CapabilitiesCall>() {
            let mut call = CapabilitiesCall {
                caps: self.deps.subsystems.media_capabilities(),
                gpu: self.deps.subsystems.gpu_adapter(),
            };
            let output = registry.invoke_engine_json(name, params, &mut call, ctx.access())?;
            return Ok(self.reply(output));
        }
        if context == TypeId::of::<CacheCall>() {
            let mut call = CacheCall {
                control: self.deps.subsystems.cache_control(),
            };
            let output = registry.invoke_engine_json(name, params, &mut call, ctx.access())?;
            return Ok(self.reply(output));
        }
        if context == TypeId::of::<ProxyCall>() {
            let mut call = ProxyCall {
                service: self.deps.subsystems.proxy_service(),
                doc: self.doc()?.snapshot().doc.clone(),
            };
            let output = registry.invoke_engine_json(name, params, &mut call, ctx.access())?;
            return Ok(self.reply(output));
        }
        if context == TypeId::of::<SessionCall>() {
            let mut call = SessionCall {
                core: Arc::clone(self),
                caller: ctx.clone(),
            };
            let output = registry.invoke_engine_json(name, params, &mut call, ctx.access())?;
            return Ok(self.reply(output));
        }
        let doc = self.doc()?;
        let call = SubsystemCall {
            name,
            params,
            context,
            caller: ctx,
            registry,
            doc: &doc,
        };
        match self.deps.subsystems.invoke(call) {
            Some(result) => result.map(|output| self.reply(output)),
            None => Err(CmdError::unsupported(format!(
                "`{name}` needs a subsystem this engine does not run"
            ))),
        }
    }

    /// The folders `ctx` may name in a path (arch §14, §18): `None` (anything) for the UI
    /// credential, which only passes what the user picked; for agent, CLI, plugin and internal
    /// callers [`agent_roots`](Self::agent_roots).
    fn path_roots(&self, ctx: &ActorCtx, session: &Session) -> Option<AllowedRoots> {
        if ctx.access() == Access::UI {
            return None;
        }
        Some(AllowedRoots::new(self.agent_roots(session)))
    }

    /// The folders every caller but the UI may name in a path, as they are right now: the
    /// approved roots (`--allow-root`, or what `project.set_allowed_roots` set last), the
    /// folder of the project the engine was started with, and the open project's folder.
    fn agent_roots(&self, session: &Session) -> Vec<PathBuf> {
        let mut roots = self
            .approved_roots
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        roots.extend(self.start_root.iter().cloned());
        roots.extend(session.project_root().map(Path::to_path_buf));
        roots
    }

    /// `project.info`'s `allowed_roots`: [`agent_roots`](Self::agent_roots), without
    /// duplicates.
    pub(crate) fn allowed_roots_now(&self) -> Result<Vec<String>, CmdError> {
        let guard = self.session()?;
        let Some(session) = guard.as_ref() else {
            return Err(stopped());
        };
        Ok(crate::agents::distinct(self.agent_roots(session)))
    }

    /// `project.set_allowed_roots`: replaces the approved roots (already canonical).
    pub(crate) fn set_approved_roots(&self, roots: Vec<PathBuf>) {
        tracing::info!(count = roots.len(), "the folders agents may use changed");
        *self
            .approved_roots
            .write()
            .unwrap_or_else(PoisonError::into_inner) = roots;
    }

    /// `Invalid` at `pointer` unless `ctx` may use `path` ([`path_roots`](Self::path_roots);
    /// the same rule as `media.import`).
    pub(crate) fn check_path(
        &self,
        ctx: &ActorCtx,
        pointer: &str,
        path: &str,
    ) -> Result<(), CmdError> {
        let guard = self.session()?;
        let Some(session) = guard.as_ref() else {
            return Err(stopped());
        };
        match self.path_roots(ctx, session) {
            Some(roots) if !roots.allows(path) => Err(CmdError::invalid(
                pointer,
                format!(
                    "`{path}` is outside the folders you may use (the open project's folder and \
                     the folders the user allowed); {OUTSIDE_ROOTS_HINT}"
                ),
            )),
            _ => Ok(()),
        }
    }

    /// `media.import` with a per-call [`ImportCtx`] (WP-1D.07 handoff): the UI imports whatever
    /// the user picked, everyone else only from [`path_roots`](Self::path_roots).
    fn import(&self, ctx: &ActorCtx, name: &str, params: Value) -> Result<Reply, CmdError> {
        let guard = self.session()?;
        let Some(session) = guard.as_ref() else {
            return Err(stopped());
        };
        let handle = session.handle().clone();
        let policy = match self.path_roots(ctx, session) {
            None => ImportPolicy::default(),
            Some(roots) => ImportPolicy::restricted(roots),
        };
        let sink = ImportSink::new(handle, ctx_for(ctx.actor(), self.next_conn()));
        let prober = self
            .deps
            .subsystems
            .media_probe()
            .unwrap_or_else(|| Arc::new(NoMediaBackend));
        let stages = self
            .deps
            .subsystems
            .analysis_stages()
            .unwrap_or_else(|| Arc::new(AnalysisStages::new()));
        let bus = Arc::new(BusAnalysisSink(self.deps.events.clone()));
        let analyzer = Analyzer::new(self.deps.jobs.clone(), stages, bus.clone());
        let mut import = ImportCtx::new(prober, Box::new(sink))
            .with_policy(policy)
            .with_analysis(analyzer)
            .with_events(bus);
        if let Some(proxies) = self.deps.subsystems.proxy_service() {
            import = import.with_proxies(proxies);
        }
        let result = self
            .deps
            .registry
            .invoke_engine_json(name, params, &mut import, ctx.access());
        // Closes the sink's connection before the session can change.
        drop(import);
        drop(guard);
        result.map(|output| self.reply(output))
    }

    /// The relink commands (WP-2F.03) with a per-call [`RelinkCtx`]: the real disk, the media
    /// prober and the caller's read allowlist ([`path_roots`](Self::path_roots)), as for
    /// `media.import`.
    fn relink(&self, ctx: &ActorCtx, name: &str, params: Value) -> Result<Reply, CmdError> {
        let guard = self.session()?;
        let Some(session) = guard.as_ref() else {
            return Err(stopped());
        };
        let handle = session.handle().clone();
        let policy = match self.path_roots(ctx, session) {
            None => ImportPolicy::default(),
            Some(roots) => ImportPolicy::restricted(roots),
        };
        let prober = self
            .deps
            .subsystems
            .media_probe()
            .unwrap_or_else(|| Arc::new(NoMediaBackend));
        let sink = RelinkTx::new(handle.clone(), ctx_for(ctx.actor(), self.next_conn()));
        let mut relink = RelinkCtx {
            doc: handle.snapshot().doc.clone(),
            prober,
            files: Arc::new(DiskFiles),
            sink: Some(Box::new(sink)),
            policy,
        };
        let result = self
            .deps
            .registry
            .invoke_engine_json(name, params, &mut relink, ctx.access());
        // Closes the sink's connection before the session can change.
        drop(relink);
        drop(guard);
        result.map(|output| self.reply(output))
    }

    /// The collect commands (WP-2F.04) against the committed document of the moment, with the
    /// caller's write allowlist ([`path_roots`](Self::path_roots)), as for `export.start`.
    fn collect(&self, ctx: &ActorCtx, name: &str, params: Value) -> Result<Reply, CmdError> {
        let mut call = {
            let guard = self.session()?;
            let Some(session) = guard.as_ref() else {
                return Err(stopped());
            };
            CollectCtx {
                doc: session.handle().committed_snapshot().doc.clone(),
                project_root: session
                    .project_root()
                    .map(|root| root.to_string_lossy().into_owned()),
                allowed_roots: self.path_roots(ctx, session),
                jobs: self.deps.jobs.clone(),
                host: Arc::new(EngineCollectHost {
                    free_space: Arc::clone(&self.free_space),
                }),
            }
        };
        let output =
            self.deps
                .registry
                .invoke_engine_json(name, params, &mut call, ctx.access())?;
        Ok(self.reply(output))
    }

    /// `export.start` against the committed document of the moment (later edits never change
    /// an export), with the caller's write allowlist ([`path_roots`](Self::path_roots)) and
    /// the subsystems' export services.
    fn export(&self, ctx: &ActorCtx, name: &str, params: Value) -> Result<Reply, CmdError> {
        let (snapshot, allowed_roots) = {
            let guard = self.session()?;
            let Some(session) = guard.as_ref() else {
                return Err(stopped());
            };
            (
                session.handle().committed_snapshot(),
                self.path_roots(ctx, session),
            )
        };
        let starter = Starter::new(self.exports.clone(), Arc::clone(&self.deps.subsystems))
            .with_free_space(Arc::clone(&self.free_space));
        let mut export = ExportCtx {
            snapshot,
            allowed_roots,
            starter: Box::new(starter),
            presets: Arc::clone(&self.presets),
        };
        let output =
            self.deps
                .registry
                .invoke_engine_json(name, params, &mut export, ctx.access())?;
        Ok(self.reply(output))
    }

    /// A `view.*` command against the session's view state; `view.changed` goes to the other
    /// clients.
    fn view(&self, ctx: &ActorCtx, name: &str, params: Value) -> Result<Reply, CmdError> {
        let guard = self.session()?;
        let Some(session) = guard.as_ref() else {
            return Err(stopped());
        };
        let (result, change) = {
            let mut view = session.view.lock().unwrap_or_else(PoisonError::into_inner);
            let result = view.with_ctx(|vc| {
                self.deps
                    .registry
                    .invoke_engine_json(name, params, vc, ctx.access())
            });
            let change =
                view.with_ctx(|vc| vc.take_event().map(|change| (change, vc.state().clone())));
            (result, change)
        };
        drop(guard);
        if let Some((change, state)) = change {
            self.deps.events.publish(EngineEvent::View {
                change,
                origin: Some(ctx.conn()),
            });
            self.deps.subsystems.on_view(&state, false);
        }
        result.map(|output| self.reply(output))
    }

    /// Starts the subsystems' devices (after the core is up, before the server binds) and gives
    /// them the first document's view state.
    fn start_subsystems(self: &Arc<Self>, config: &EngineConfig) -> Result<(), StartError> {
        let host = SubsystemHost {
            mode: config.mode,
            audio: config.audio,
            gpu: config.gpu,
            deterministic_ids: self.deps.seed,
            events: SubsystemEvents(self.deps.events.clone()),
            runtime: self.runtime.clone(),
            link: EngineLink {
                core: Arc::downgrade(self),
            },
        };
        self.deps.subsystems.start(&host)?;
        self.announce_first_view();
        Ok(())
    }

    /// Gives the subsystems the view state of the document the engine started with (master
    /// volume, the saved playhead).
    fn announce_first_view(&self) {
        let guard = self.session.read().unwrap_or_else(PoisonError::into_inner);
        if let Some(session) = guard.as_ref() {
            self.deps
                .subsystems
                .on_view(&Core::view_state(session), true);
        }
    }

    /// The view state of `session` (for [`Subsystems::on_view`](crate::Subsystems::on_view)).
    fn view_state(session: &Session) -> UiState {
        session
            .view
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .with_ctx(|vc| vc.state().clone())
    }

    /// Moves the view playhead with the registered `view.set_playhead` command (the transport
    /// keeps it in step on pause, seek and stop; WP-1I.05).
    pub(crate) fn set_view_playhead(
        &self,
        t: Time,
        origin: Option<ConnId>,
    ) -> Result<(), CmdError> {
        let guard = self.session()?;
        let Some(session) = guard.as_ref() else {
            return Err(stopped());
        };
        let change = {
            let mut view = session.view.lock().unwrap_or_else(PoisonError::into_inner);
            view.with_ctx(|vc| {
                self.deps
                    .registry
                    .invoke_engine_json(
                        "view.set_playhead",
                        serde_json::json!({ "at": t.flicks() }),
                        vc,
                        Access::STANDARD,
                    )
                    .map(|_| vc.take_event())
            })
        }?;
        drop(guard);
        if let Some(change) = change {
            self.deps
                .events
                .publish(EngineEvent::View { change, origin });
        }
        Ok(())
    }

    /// Rewrites the run-file (atomically) with `change`, which says whether it changed
    /// anything. The lock is held throughout, so rewrites do not lose each other's fields and
    /// none recreates the file after shutdown removed it.
    fn update_run_file(&self, what: &str, change: impl FnOnce(&mut RunFileContents) -> bool) {
        let guard = self.run_file.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(path) = guard.as_ref() else { return };
        if let Err(e) = RunFile::update(path, change) {
            tracing::warn!(error = %e, "could not update the run-file's {what}");
        }
    }

    /// Rewrites the run-file's `shm` with the current preview ring (on the blocking pool).
    pub(crate) fn preview_ring_changed(self: &Arc<Self>) {
        let core = Arc::clone(self);
        self.runtime.spawn_blocking(move || {
            let shm = core.deps.subsystems.preview_ring_name();
            core.update_run_file("preview ring", |c| {
                let changed = c.shm != shm;
                c.shm = shm;
                changed
            });
        });
    }

    /// The open project as the run-file records it: none for an untitled document.
    fn run_file_project(&self) -> Option<RunFileProject> {
        let guard = self.session().ok()?;
        let session = guard.as_ref()?;
        let root = session.project_root()?;
        Some(RunFileProject {
            name: session.status().name,
            path: root.to_string_lossy().into_owned(),
        })
    }

    /// Rewrites the run-file's `project` to what is open now (on the blocking pool). Called
    /// after every open, close, save and recovery, so `cutweave engines` and the choice of an
    /// engine by project stay right. A rename shows after the next save.
    pub(crate) fn project_changed(self: &Arc<Self>) {
        let core = Arc::clone(self);
        self.runtime.spawn_blocking(move || {
            let project = core.run_file_project();
            core.update_run_file("project", |c| {
                let changed = c.project != project;
                c.project = project;
                changed
            });
        });
    }

    /// Records that the window gained the focus (`project.report_focus`); returns the time,
    /// milliseconds since the Unix epoch. The file is written on the blocking pool.
    pub(crate) fn report_focus(self: &Arc<Self>) -> u64 {
        let at = self.deps.clock.now().as_millis();
        let core = Arc::clone(self);
        self.runtime.spawn_blocking(move || {
            core.update_run_file("focus time", |c| {
                // Reports can overtake each other on the pool; the latest one stands.
                let changed = c.focused_at.is_none_or(|old| old < at);
                if changed {
                    c.focused_at = Some(at);
                }
                changed
            });
        });
        at
    }

    // ----- the session commands ----------------------------------------------------------------

    /// Swaps `slot`'s session for `next`, closing the old one (no save), and announces it.
    fn swap_session(
        self: &Arc<Self>,
        slot: &mut Option<Session>,
        build: impl FnOnce(&Deps, Option<IdState>) -> Result<Session, CmdError>,
    ) -> Result<(Option<PathBuf>, ProjectStatus), CmdError> {
        let Some(old) = slot.take() else {
            return Err(stopped());
        };
        self.set_state(SessionState::Closing);
        let prev_rev = old.handle().snapshot().rev;
        let ids = old.ids();
        // Exports are the queue's: an untitled document's move into the next project, a
        // project's stay behind in it (suspended) and run again when it is opened again.
        let exports = if old.project_root().is_none() {
            Attach::Carry
        } else {
            Attach::Suspend
        };
        self.deps
            .jobs
            .cancel_matching(|j| j.kind != JobKind::Export);
        let closed = old.close(&self.deps);
        let (next, error) = match build(&self.deps, ids) {
            Ok(next) => (next, None),
            // Keep a document either way.
            Err(e) => (Session::untitled(&self.deps, ids)?, Some(e)),
        };
        self.attach_exports(&next, exports);
        self.settle_state(&next);
        if let Some(path) = &closed {
            session::announce_closed(&self.deps, path);
        }
        session::announce_reset(&self.deps, prev_rev, &next);
        let status = next.status();
        self.deps.subsystems.on_view(&Core::view_state(&next), true);
        *slot = Some(next);
        self.project_changed();
        if let Some(e) = error {
            return Err(e);
        }
        Ok((closed, status))
    }

    /// `project.open` (ADR-0029): refuses to drop unsaved changes unless `discard_unsaved`, and
    /// announces the open (`project.opening`, then `project.opened` or `project.open_failed`).
    pub(crate) fn open_project(
        self: &Arc<Self>,
        path: &Path,
        discard_unsaved: bool,
        actor: &Actor,
    ) -> Result<ProjectOpenOutput, CmdError> {
        // Discarding closes as `project.close` with `save: false` does: no save, not even
        // autosave's, so what the person chose not to keep is not written.
        let save = discard_unsaved.then_some(false);
        {
            let guard = self.session()?;
            if let Some(current) = guard.as_ref() {
                // Opening the project that is already open changes nothing.
                if current.is_project(path) {
                    return Ok(ProjectOpenOutput {
                        project: current.status(),
                        missing_media: Vec::new(),
                        migrated_from: None,
                    });
                }
                // Refused before anything is read or locked; the app tells the person.
                if let Err(e) = current.guard_unsaved(save, OPEN_ANYWAY) {
                    session::announce_open_failed(&self.deps, path, actor, &e);
                    return Err(e);
                }
            }
        }
        session::announce_opening(&self.deps, path, actor);
        let result = self.open_announced(path, save);
        if let Err(e) = &result {
            session::announce_open_failed(&self.deps, path, actor, e);
        }
        result
    }

    /// The open itself, after `project.opening`.
    fn open_announced(
        self: &Arc<Self>,
        path: &Path,
        save: Option<bool>,
    ) -> Result<ProjectOpenOutput, CmdError> {
        let opened = session::open_store(path).map_err(|e| session::io_error(&e, "/path"))?;
        let missing_media = opened.report.missing_media.iter().map(|m| m.id).collect();
        let migrated_from = opened.report.migration.as_ref().map(|m| m.from_version);
        let mut slot = self.session_mut()?;
        if let Some(current) = slot.as_ref() {
            // Checked again under the write lock: an edit may have landed since.
            current.guard_unsaved(save, OPEN_ANYWAY)?;
            current.save_before_close(save)?;
        }
        let (_, status) =
            self.swap_session(&mut slot, |deps, ids| Session::open(deps, opened, ids))?;
        session::announce_opened(&self.deps, status.clone());
        tracing::info!(path = %path.display(), rev = %status.rev, "project opened");
        Ok(ProjectOpenOutput {
            project: status,
            missing_media,
            migrated_from,
        })
    }

    pub(crate) fn close_project(
        self: &Arc<Self>,
        save: Option<bool>,
    ) -> Result<ProjectCloseOutput, CmdError> {
        let mut slot = self.session_mut()?;
        if let Some(current) = slot.as_ref() {
            current.guard_unsaved(save, CLOSE_ANYWAY)?;
            current.save_before_close(save)?;
        }
        let (closed, status) = self.swap_session(&mut slot, Session::untitled)?;
        if let Some(path) = &closed {
            tracing::info!(path = %path.display(), "project closed");
        }
        Ok(ProjectCloseOutput {
            closed: closed.map(|p| p.to_string_lossy().into_owned()),
            project: status,
        })
    }

    pub(crate) fn set_autosave(&self, pref: AutosavePreference) -> Result<(), CmdError> {
        *self
            .deps
            .autosave_pref
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(pref);
        let guard = self.session()?;
        if let Some(session) = guard.as_ref() {
            session.apply_autosave(pref);
        }
        Ok(())
    }

    pub(crate) fn save_project(self: &Arc<Self>) -> Result<ProjectSaveOutput, CmdError> {
        let guard = self.session()?;
        let Some(session) = guard.as_ref() else {
            return Err(stopped());
        };
        let (rev, path) = session.save()?;
        self.project_changed();
        Ok(ProjectSaveOutput { path, rev })
    }

    pub(crate) fn save_project_as(
        self: &Arc<Self>,
        folder: &Path,
    ) -> Result<ProjectSaveOutput, CmdError> {
        let mut slot = self.session_mut()?;
        let Some(current) = slot.as_mut() else {
            return Err(stopped());
        };
        if current.can_save_as_in_place()? {
            let rev = current.save_as_in_place(&self.deps, folder)?;
            self.attach_exports(current, Attach::Carry); // the queue moves with the project
            let status = current.status();
            let path = status.path.clone().unwrap_or_default();
            session::announce_opened(&self.deps, status);
            self.project_changed();
            return Ok(ProjectSaveOutput { path, rev });
        }
        // Untitled or read-only: write a new project from the committed document and reopen
        // it journaled.
        let snap = current.handle().committed_snapshot();
        // A document still called "Untitled" takes the name chosen for the folder; one the
        // person named keeps its name (#165).
        let mut doc = snap.doc.clone();
        if doc.meta.name == session::UNTITLED
            && let Some(name) = folder.file_name().and_then(|n| n.to_str())
        {
            name.clone_into(&mut doc.meta.name);
        }
        let store =
            ProjectStore::create(folder, &doc).map_err(|e| session::io_error(&e, "/path"))?;
        let document = store.document_path().to_path_buf();
        store.close().map_err(|e| session::io_error(&e, "/path"))?;
        let opened = session::open_store(&document).map_err(|e| session::io_error(&e, "/path"))?;
        let (_, status) =
            self.swap_session(&mut slot, |deps, ids| Session::open(deps, opened, ids))?;
        session::announce_opened(&self.deps, status.clone());
        Ok(ProjectSaveOutput {
            path: document.to_string_lossy().into_owned(),
            rev: status.saved_rev.unwrap_or(status.rev),
        })
    }

    pub(crate) fn decide_recovery(
        self: &Arc<Self>,
        discard: bool,
    ) -> Result<ProjectRecoverOutput, CmdError> {
        let mut slot = self.session_mut()?;
        let Some(current) = slot.as_mut() else {
            return Err(stopped());
        };
        let recovered = current.decide_recovery(&self.deps, discard)?;
        self.project_changed();
        let status = current.status();
        session::announce_opened(&self.deps, status.clone());
        let damaged = recovered
            .report
            .stopped
            .as_ref()
            .is_some_and(|s| s.reason != StopReason::Truncated);
        Ok(ProjectRecoverOutput {
            project: status,
            applied: u32::try_from(recovered.report.applied).unwrap_or(u32::MAX),
            damaged,
        })
    }
}

/// A cheap, cloneable handle to a running engine: what the RPC, MCP and media layers mount
/// (via [`ApiOptions::mounts`](crate::ApiOptions)) and what in-process clients (CLI oneshot,
/// tests) use.
#[derive(Clone)]
pub struct EngineHandle {
    pub(crate) core: Arc<Core>,
}

impl std::fmt::Debug for EngineHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineHandle")
            .field("mode", &self.core.mode)
            .field("state", &self.state())
            .finish_non_exhaustive()
    }
}

impl EngineHandle {
    /// Opens a client connection with `credential`'s actor and access (ADR-0016): one per
    /// WebSocket connection, MCP session or CLI invocation. Each gets a new connection id that
    /// is never reused; dropping the last clone of the [`Connection`] closes it, rolling back
    /// its open transaction (WP-1B.09). The UI credential exists only in `Gui` mode.
    pub fn connect(&self, credential: &Credential) -> Result<Connection, CmdError> {
        if self.core.stopping.load(Ordering::Acquire) {
            return Err(stopped());
        }
        if *credential == Credential::Ui && !self.core.has_ui {
            return Err(CmdError::unsupported(
                "this engine runs without a UI, so the UI credential does not exist",
            ));
        }
        let conn = match credential {
            Credential::Internal => ConnId::INTERNAL,
            _ => self.core.next_conn(),
        };
        let ctx = credential
            .actor_ctx(conn)
            .map_err(|e| CmdError::invalid("", e.to_string()))?;
        Ok(Connection::new(Arc::clone(&self.core), ctx))
    }

    /// A subscription to the event fan-out (see [`crate::events`]).
    #[must_use]
    pub fn subscribe(&self) -> EventReceiver {
        self.core.deps.events.subscribe()
    }

    /// The live document (lock-free once the session is read; `project.snapshot`).
    pub fn snapshot(&self) -> Result<Arc<DocSnapshot>, CmdError> {
        Ok(self.core.doc()?.snapshot())
    }

    /// The current document handle (it changes when the document is swapped; prefer
    /// [`connect`](Self::connect) for requests).
    pub fn doc(&self) -> Result<DocHandle, CmdError> {
        self.core.doc()
    }

    /// What the engine holds (project path, revision, dirty, read-only, recovery).
    pub fn status(&self) -> Result<ProjectStatus, CmdError> {
        let guard = self.core.session()?;
        guard.as_ref().map(Session::status).ok_or_else(stopped)
    }

    /// The session's lifecycle state (`Closing` while a swap or shutdown runs).
    #[must_use]
    pub fn state(&self) -> SessionState {
        state_of(self.core.state.load(Ordering::Acquire))
    }

    /// The registry (names, schemas, MCP annotations).
    #[must_use]
    pub fn registry(&self) -> Arc<Registry> {
        Arc::clone(&self.core.deps.registry)
    }

    /// How the engine runs.
    #[must_use]
    pub fn mode(&self) -> EngineMode {
        self.core.mode
    }

    /// The job manager.
    #[must_use]
    pub fn jobs(&self) -> &JobManager {
        &self.core.deps.jobs
    }

    /// The engine runtime (spawn async work; never block its workers).
    #[must_use]
    pub fn runtime(&self) -> &tokio::runtime::Handle {
        &self.core.runtime
    }

    /// The preview ring announcement (`rpc.hello`'s `shm`), when the engine publishes one.
    #[must_use]
    pub fn preview_ring(&self) -> Option<String> {
        self.core.deps.subsystems.preview_ring()
    }

    /// What the `/media` HTTP endpoints serve (WP-1I.03), when media subsystems run.
    #[must_use]
    pub fn media_state(&self) -> Option<MediaState> {
        self.core.deps.subsystems.media_state()
    }
}

/// Every command the core engine serves: `cutweave-ops`, the `edit.*` commands and the
/// session commands (the generated catalog should be built from this, see the integrator
/// notes of WP-1I.04).
pub fn build_registry() -> Result<Registry, RegistryError> {
    let mut registry = Registry::new();
    cutweave_ops::register(&mut registry)?;
    cutweave_commands::edit::register(&mut registry)?;
    session::register(&mut registry)?;
    crate::agents::register(&mut registry)?;
    crate::media_caps::register(&mut registry)?;
    crate::media_cache::register(&mut registry)?;
    Ok(registry)
}

/// Runs `fut` to completion on `rt`, also when called from inside an async context (then on a
/// helper thread, since a runtime cannot block inside another).
fn block_on<F>(rt: &tokio::runtime::Handle, fut: F) -> F::Output
where
    F: std::future::Future + Send,
    F::Output: Send,
{
    if tokio::runtime::Handle::try_current().is_ok() {
        std::thread::scope(|s| match s.spawn(|| rt.block_on(fut)).join() {
            Ok(out) => out,
            Err(panic) => std::panic::resume_unwind(panic),
        })
    } else {
        rt.block_on(fut)
    }
}

/// The engine runtime; dropping it waits for its threads (also from async code, where a
/// runtime may not block: then on a helper thread).
struct OwnedRuntime(Option<Runtime>);

impl OwnedRuntime {
    fn handle(&self) -> Option<&tokio::runtime::Handle> {
        self.0.as_ref().map(Runtime::handle)
    }

    fn stop(&mut self) {
        let Some(rt) = self.0.take() else {
            return;
        };
        if tokio::runtime::Handle::try_current().is_ok() {
            let joined = std::thread::spawn(move || rt.shutdown_timeout(RUNTIME_SHUTDOWN)).join();
            if joined.is_err() {
                tracing::error!("the engine runtime did not stop cleanly");
            }
        } else {
            rt.shutdown_timeout(RUNTIME_SHUTDOWN);
        }
    }
}

impl Drop for OwnedRuntime {
    fn drop(&mut self) {
        self.stop();
    }
}

/// A running engine. Dropping it shuts it down (see [`Engine::shutdown`]).
pub struct Engine {
    core: Arc<Core>,
    runtime: OwnedRuntime,
    server: Option<RunningServer>,
    /// The run-file folder whose run-file is being prepared ahead of the server's start.
    prepared_run_dir: Option<PathBuf>,
    log: Option<LogGuard>,
    stopped: bool,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("mode", &self.core.mode)
            .field("port", &self.port())
            .finish_non_exhaustive()
    }
}

impl Engine {
    /// Starts the core (see the module docs). Errors carry their launch-protocol exit code
    /// ([`StartError::exit_code`]); anything started before the failure is stopped again.
    #[allow(clippy::too_many_lines)]
    pub fn start(mut config: EngineConfig) -> Result<Engine, StartError> {
        let mut phases = crate::startup::Phases::new();
        Engine::check(&config)?;
        let log = if config.log.enabled() {
            Some(logging::init(&config.log)?)
        } else {
            None
        };
        let runtime = OwnedRuntime(Some(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(config.worker_threads.max(1))
                .thread_name("cutweave-rt")
                .enable_all()
                .build()?,
        ));
        let rt = runtime
            .handle()
            .cloned()
            .ok_or_else(|| StartError::Config("no runtime".into()))?;
        phases.mark("runtime");
        let seed = config.deterministic_ids;
        let clock: Arc<dyn Clock> = match (&config.clock, seed) {
            (Some(clock), _) => Arc::clone(clock),
            (None, Some(_)) => Arc::new(ManualClock::new(WallTime::UNIX_EPOCH)),
            (None, None) => Arc::new(SystemClock),
        };
        let mut jobs = JobManager::builder()
            .config(config.jobs.clone())
            .runtime(rt.clone());
        if let Some(seed) = seed {
            jobs = jobs.ids(Box::new(SeededGen::new(seed ^ JOB_ID_STREAM)));
        }
        let jobs = jobs.build()?;
        let subsystems: Arc<dyn crate::Subsystems> = match config.subsystems.take() {
            Some(s) => Arc::from(s),
            None => Arc::new(CoreOnly),
        };
        let mut registry = build_registry()?;
        subsystems.register(&mut registry)?;
        phases.mark("registry");
        let deps = Deps {
            registry: Arc::new(registry),
            clock,
            seed,
            journal_sync: config.journal_sync,
            autosave: config.autosave,
            autosave_pref: Mutex::new(None),
            events: EventBus::new(config.event_capacity),
            subsystems,
            jobs,
        };
        let first = match &config.project {
            Some(path) => {
                let opened =
                    session::open_store(path).map_err(|e| StartError::from_project(path, e))?;
                Session::open(&deps, opened, None).map_err(StartError::Session)?
            }
            None => Session::untitled(&deps, None).map_err(StartError::Session)?,
        };
        phases.mark("project");
        // The project the engine was started with stays usable for agents after it is closed
        // (the launcher chose it); nothing else is without `--allow-root`.
        let core = Arc::new(Core {
            mode: config.mode,
            approved_roots: RwLock::new(config.allowed_roots.clone()),
            start_root: first.project_root().map(Path::to_path_buf),
            connections: Connections::default(),
            runtime: rt,
            state: AtomicU8::new(0),
            next_conn: AtomicU64::new(FIRST_CONN),
            stopping: AtomicBool::new(false),
            has_ui: config.mode == EngineMode::Gui,
            session: RwLock::new(None),
            exports: Core::new_exports(&deps, config.jobs.export_slots),
            presets: Arc::new(PresetStore::open(config.presets_dir.clone())),
            free_space: config
                .free_space
                .clone()
                .unwrap_or_else(|| Arc::new(crate::export::disk::free_space)),
            run_file: Mutex::new(None),
            deps,
        });
        core.install_first(first);
        let mut engine = Engine {
            core: Arc::clone(&core),
            runtime,
            server: None,
            prepared_run_dir: None,
            log,
            stopped: false,
        };
        // From here on, dropping `engine` on an error stops what already runs.
        engine.forward_job_events();
        engine.prepare_run_file(&config);
        core.start_subsystems(&config)?;
        core.exports.start(); // queued exports (restored ones too) need the subsystems
        phases.mark("subsystems");
        engine.serve(&mut config, &mut phases)?;
        phases.mark("server");
        tracing::info!(phases = %phases, "engine start phases");
        tracing::info!(
            mode = config.mode.name(),
            port = engine.port(),
            deterministic = seed.is_some(),
            elapsed_ms = crate::startup::ms(phases.total()),
            "engine ready"
        );
        Ok(engine)
    }

    /// Refuses configurations that contradict the mode.
    fn check(config: &EngineConfig) -> Result<(), StartError> {
        match (config.mode, config.ui_token.is_some()) {
            (EngineMode::Gui, false) => {
                return Err(StartError::Handshake(
                    "a GUI engine needs the UI credential from the spawn handshake".into(),
                ));
            }
            (EngineMode::Headless | EngineMode::Oneshot, true) => {
                return Err(StartError::Handshake(
                    "a headless engine has no UI credential; refusing the UI token".into(),
                ));
            }
            _ => {}
        }
        if config.mode == EngineMode::Oneshot && config.port.is_some() {
            return Err(StartError::Config(
                "a oneshot engine runs no server; do not give it a port".into(),
            ));
        }
        Ok(())
    }

    /// Starts the API server (WP-1I.01) with the mounted routes, when the mode serves.
    fn serve(
        &mut self,
        config: &mut EngineConfig,
        phases: &mut crate::startup::Phases,
    ) -> Result<(), StartError> {
        let Some(port) = config.server_port() else {
            return Ok(());
        };
        let mounts = match config.api.mounts.take() {
            Some(factory) => factory(self.handle()),
            None => cutweave_api::Mounts::default(),
        };
        phases.mark("mounts");
        let project = self.core.run_file_project();
        let api = ApiConfig {
            port,
            run_dir: config.api.run_dir.clone(),
            ui_token: config.ui_token.take(),
            agent_labels: config.api.agent_labels.clone(),
            allowed_origins: config.api.allowed_origins.clone(),
            project,
            version: VERSION.to_owned(),
            enable_test_routes: config.api.enable_test_routes,
            mounts,
            shm: self.core.deps.subsystems.preview_ring_name(),
        };
        let server = block_on(&self.core.runtime, ApiServer::start(api))?;
        *self
            .core
            .run_file
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(server.run_file_path().to_path_buf());
        self.server = Some(server);
        Ok(())
    }

    /// Makes the run-file's private file while the subsystems start (on Windows that runs
    /// `whoami` and `icacls`, seconds on a busy machine), so the server's start before the ready
    /// line only fills it (issue #143).
    fn prepare_run_file(&mut self, config: &EngineConfig) {
        if config.server_port().is_none() {
            return;
        }
        let Some(dir) = config.api.run_dir.clone().or_else(RunFile::default_dir) else {
            return;
        };
        let spawned = std::thread::Builder::new()
            .name("cw-run-file".into())
            .spawn({
                let dir = dir.clone();
                move || RunFile::prepare(&dir)
            });
        match spawned {
            Ok(_) => self.prepared_run_dir = Some(dir),
            Err(e) => tracing::debug!(error = %e, "cannot prepare the run-file early"),
        }
    }

    fn forward_job_events(&self) {
        let mut rx = self.core.deps.jobs.subscribe();
        let bus = self.core.deps.events.clone();
        self.core.runtime.spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(event) => bus.publish(EngineEvent::Job(event)),
                    // Progress is a stream of whole states: a lagging forwarder just goes on.
                    Err(BroadcastRecvError::Lagged(_)) => {}
                    Err(BroadcastRecvError::Closed) => break,
                }
            }
        });
    }

    /// A handle for clients.
    #[must_use]
    pub fn handle(&self) -> EngineHandle {
        EngineHandle {
            core: Arc::clone(&self.core),
        }
    }

    /// The API server's port, if it serves.
    #[must_use]
    pub fn port(&self) -> Option<u16> {
        self.server.as_ref().map(RunningServer::port)
    }

    /// The run-file, if the engine serves.
    #[must_use]
    pub fn run_file_path(&self) -> Option<&Path> {
        self.server.as_ref().map(RunningServer::run_file_path)
    }

    /// The server's credentials (agent tokens for in-process clients and tests).
    #[must_use]
    pub fn credentials(&self) -> Option<&Credentials> {
        self.server.as_ref().map(RunningServer::credentials)
    }

    /// The log file, if logging is on.
    #[must_use]
    pub fn log_file(&self) -> Option<&Path> {
        self.log.as_ref().map(LogGuard::path)
    }

    /// Stops the engine (see the module docs). Saves nothing.
    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        if std::mem::replace(&mut self.stopped, true) {
            return;
        }
        let started = Instant::now();
        let core = &self.core;
        core.stopping.store(true, Ordering::Release);
        core.set_state(SessionState::Closing);
        // No run-file rewrite may race the server removing it.
        core.run_file
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let (Some(server), Some(rt)) = (self.server.take(), self.runtime.handle()) {
            block_on(rt, server.shutdown());
        } else if let Some(dir) = self.prepared_run_dir.take() {
            // The start failed before the server wrote the run-file.
            RunFile::discard_prepared(&dir);
        }
        core.deps.subsystems.shutdown();
        // The queue stops writing first, so its exports keep their stored state and run again
        // next time.
        core.exports.shutdown();
        core.deps.jobs.cancel_all();
        let session = core
            .session
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(session) = session {
            session.close(&core.deps);
        }
        self.runtime.stop();
        tracing::info!(
            elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            "engine stopped"
        );
        if let Some(log) = self.log.take() {
            log.finish();
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Handles and connections cross threads (RPC tasks, CLI, tests).
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<EngineHandle>();
    assert_send_sync::<Connection>();
};
