// SPDX-License-Identifier: GPL-3.0-or-later
//! One engine = one open project (arch §14): the session holding the document actor, the
//! project store with its journal and autosave, and the view state (WP-1I.04).
//!
//! # Lifecycle
//!
//! `NoProject` (an untitled in-memory document: every document command works, nothing is
//! journaled) → `Open` (a project folder: store, lock, journal, autosave) → `Closing` (during
//! `project.close`, a swap and engine shutdown). `project.open`, `project.close`,
//! `project.save_as` of an untitled document and the recovery decision **swap the document**:
//! a new doc actor replaces the old one, open jobs are cancelled, a new journal starts, and the
//! fan-out sends `doc.reset` (clients resync) plus `project.opened` / `project.closed`.
//!
//! # Open, step by step (the WP-1C.02/1C.03 handoffs)
//!
//! 1. [`ProjectStore::open`] (the lock; exit code 10 at start when another process holds it).
//! 2. Read-only store (newer schema): a doc actor whose commits are refused; no journal, no
//!    autosave.
//! 3. Unsaved work in the journal: the saved document is shown, commits are refused and
//!    `project.recovery_available` is sent until `project.recover` or
//!    `project.discard_recovery`.
//! 4. Otherwise (and after that decision): recover/discard, [`ProjectStore::start_journal`],
//!    the doc actor over the result with the journal as its commit sink, then the
//!    [`AutosaveService`] with `saved_rev` = the revision of the document on disk (the
//!    journal's base revision after a restore), its thread, and its events forwarded.
//!
//! # Closing
//!
//! `project.close` saves first when asked (`save: true`) or, by default, when autosave is on
//! and the document is dirty (WP-1C.03: `save_now()` on close). Engine shutdown never saves:
//! unsaved work stays in the journal and is offered on the next open. Then: subsystems are told,
//! the autosave thread stops, the doc actor drains and stops, the view state is flushed, and
//! the store closes (journal synced, lock released).

mod commands;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cutweave_commands::{
    Actor, ActorOptions, Clock, CmdError, CommitRecord, ConflictReason, DocActor, DocConfig,
    DocEvent, DocHandle, DocReset, HistoryLimits, RecvError, Registry, Rev, WallTime, wire,
};
use cutweave_jobs::JobManager;
use cutweave_model::Project;
use cutweave_ops::view::ViewCtx;

use crate::autosave::{
    AutosaveConfig, AutosavePreference, AutosaveService, AutosaveSettings, AutosaveTask,
    ProjectEvent, ProjectSaved, SharedStore,
};
use crate::events::{
    EngineEvent, EventBus, ProjectClosed, ProjectOpenFailed, ProjectOpening, SessionEvent,
};
use crate::journal::{
    IdState, IdTracker, JournalConfig, JournalError, RecoverOptions, Recovered, RecoveryAvailable,
    SyncPolicy,
};
use crate::project_io::{Opened, ProjectIoError, ProjectLayout, ProjectStore, ReadOnlyReason};
use crate::subsystems::Subsystems;
use crate::ui_state::UiStateStore;

pub use commands::{
    ProjectClose, ProjectCloseOutput, ProjectDiscardRecovery, ProjectOpen, ProjectOpenOutput,
    ProjectRecover, ProjectRecoverOutput, ProjectSave, ProjectSaveAs, ProjectSaveOutput,
    SessionCall, register,
};

/// Name of the untitled document the engine holds when no project is open.
pub const UNTITLED: &str = "Untitled";

/// Where the session is in its lifecycle.
#[wire]
#[derive(Copy, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// An untitled document, not backed by a project folder.
    #[default]
    NoProject,
    /// A project folder is open.
    Open,
    /// The project is being closed (a swap or engine shutdown is under way).
    Closing,
}

/// Unsaved work from a previous session (what a restore would bring back).
#[wire]
pub struct RecoveryInfo {
    /// Transactions a restore would replay.
    pub records: u32,
    /// The committed revision of the saved document.
    pub base_rev: Rev,
    /// The revision a restore reaches.
    pub last_rev: Rev,
    /// When the last of them was committed.
    pub last_ts: WallTime,
    /// The journal ends with a damaged record (replay stops there).
    pub damaged: bool,
}

impl From<&RecoveryAvailable> for RecoveryInfo {
    fn from(r: &RecoveryAvailable) -> RecoveryInfo {
        RecoveryInfo {
            records: u32::try_from(r.records).unwrap_or(u32::MAX),
            base_rev: r.base_rev,
            last_rev: r.last_rev,
            last_ts: r.last_ts,
            damaged: r.damaged,
        }
    }
}

/// What the engine holds: the `project.*` commands return it and `project.opened` carries it.
#[wire]
pub struct ProjectStatus {
    /// `no_project` (an untitled document) or `open`.
    pub state: SessionState,
    /// The project document (`.cutweave`) path; absent for an untitled document.
    pub path: Option<String>,
    /// The project name (from the document).
    pub name: String,
    /// The live document revision.
    pub rev: Rev,
    /// The committed revision last written to the project file; absent while untitled,
    /// read-only or awaiting a recovery decision.
    pub saved_rev: Option<Rev>,
    /// Whether committed changes are not in the project file yet.
    pub dirty: bool,
    /// Why the project is read-only (edits are refused), if it is.
    pub read_only: Option<String>,
    /// Unsaved work from a previous session awaiting `project.recover` or
    /// `project.discard_recovery` (edits are refused until then).
    pub recovery: Option<RecoveryInfo>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What building and swapping sessions needs from the engine (shared, immutable).
pub(crate) struct Deps {
    pub(crate) registry: Arc<Registry>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) seed: Option<u64>,
    pub(crate) journal_sync: SyncPolicy,
    pub(crate) autosave: Option<AutosaveConfig>,
    /// The editor's autosave preference (`project.set_autosave`); wins over the project's
    /// `settings.autosave` for every session this engine builds (arch §17).
    pub(crate) autosave_pref: Mutex<Option<AutosavePreference>>,
    /// The undo steps kept per actor the editor asked for (`edit.set_history_limit`,
    /// WP-2A.13): every document this engine opens starts with it.
    pub(crate) history_limit: Mutex<Option<u32>>,
    pub(crate) events: EventBus,
    pub(crate) subsystems: Arc<dyn Subsystems>,
    pub(crate) jobs: JobManager,
}

// ----- view state ----------------------------------------------------------------------------

/// The `view.*` context of the session: persisted for a project, in memory when untitled.
#[derive(Debug)]
pub(crate) enum ViewHost {
    Memory(ViewCtx),
    Stored(UiStateStore),
}

impl ViewHost {
    pub(crate) fn with_ctx<R>(&mut self, f: impl FnOnce(&mut ViewCtx) -> R) -> R {
        match self {
            ViewHost::Memory(ctx) => f(ctx),
            ViewHost::Stored(store) => store.with_ctx(Instant::now(), f),
        }
    }

    fn set_doc(&mut self, doc: &Project) -> bool {
        self.with_ctx(|ctx| ctx.set_doc(doc))
    }

    fn poll(&mut self) {
        if let ViewHost::Stored(store) = self
            && let Err(e) = store.poll(Instant::now())
        {
            tracing::warn!(error = %e, "could not write ui-state.json");
        }
    }

    fn flush(&mut self) {
        if let ViewHost::Stored(store) = self
            && let Err(e) = store.flush()
        {
            tracing::warn!(error = %e, "could not write ui-state.json");
        }
    }

    fn take_warnings(&mut self) -> Vec<String> {
        match self {
            ViewHost::Memory(_) => Vec::new(),
            ViewHost::Stored(store) => store.take_warnings(),
        }
    }
}

// ----- the session -----------------------------------------------------------------------------

/// Why commits are refused in a phase without a journal.
const RECOVERY_PENDING: &str = "unsaved work from a previous session awaits a decision: call \
     project.recover or project.discard_recovery before editing";

/// The phase of an open project.
pub(crate) enum Phase {
    /// Never written (newer schema or opened read-only): commits are refused.
    ReadOnly(ReadOnlyReason),
    /// Unsaved work awaits `project.recover` / `project.discard_recovery`; commits are refused.
    AwaitingRecovery {
        base: Box<Project>,
        available: RecoveryAvailable,
    },
    /// Journaled and autosaved.
    Editing(Editing),
}

/// The autosave of an editable project.
pub(crate) struct Editing {
    autosave: AutosaveService,
    task: Option<AutosaveTask>,
    forwarder: Option<JoinHandle<()>>,
}

impl Editing {
    /// Stops the autosave thread, drops the service and joins its event forwarder.
    fn stop(mut self) {
        drop(self.task.take());
        let forwarder = self.forwarder.take();
        drop(self);
        if let Some(f) = forwarder
            && f.join().is_err()
        {
            tracing::error!("the project event forwarder panicked");
        }
    }
}

/// An open project folder.
pub(crate) struct OpenProject {
    layout: ProjectLayout,
    store: SharedStore,
    phase: Phase,
}

/// The running document and what belongs to it.
pub(crate) struct Session {
    actor: Option<DocActor>,
    handle: DocHandle,
    forwarder: Option<JoinHandle<()>>,
    tracker: Option<IdTracker>,
    pub(crate) view: Arc<Mutex<ViewHost>>,
    project: Option<OpenProject>,
}

/// A running doc actor with its event forwarder.
struct Doc {
    actor: DocActor,
    handle: DocHandle,
    forwarder: JoinHandle<()>,
}

impl Session {
    /// The current document.
    pub(crate) fn handle(&self) -> &DocHandle {
        &self.handle
    }

    /// The open project's folder, if any.
    pub(crate) fn project_root(&self) -> Option<&Path> {
        self.project.as_ref().map(|p| p.layout.root.as_path())
    }

    /// Where the export queue of this session is kept (WP-2G.03a): the project's `.cutweave`
    /// folder, unless the project is read-only (or there is none: an untitled document's queue
    /// lives in memory).
    pub(crate) fn export_meta_dir(&self) -> Option<PathBuf> {
        let project = self.project.as_ref()?;
        if matches!(project.phase, Phase::ReadOnly(_)) {
            return None;
        }
        Some(project.layout.meta_dir())
    }

    /// Whether `path` (a folder or `.cutweave` file) is the open project.
    pub(crate) fn is_project(&self, path: &Path) -> bool {
        let Some(p) = &self.project else {
            return false;
        };
        let Ok(wanted) = std::fs::canonicalize(path) else {
            return false;
        };
        [&p.layout.root, &p.layout.document]
            .iter()
            .any(|known| std::fs::canonicalize(known).is_ok_and(|k| k == wanted))
    }

    /// The deterministic id streams' position (to continue them in the next session).
    pub(crate) fn ids(&self) -> Option<IdState> {
        self.tracker.as_ref().map(IdTracker::state)
    }

    /// What the engine holds.
    pub(crate) fn status(&self) -> ProjectStatus {
        let snap = self.handle.snapshot();
        let mut status = ProjectStatus {
            state: SessionState::NoProject,
            path: None,
            name: snap.doc.meta.name.clone(),
            rev: snap.rev,
            saved_rev: None,
            dirty: true,
            read_only: None,
            recovery: None,
        };
        if let Some(p) = &self.project {
            status.state = SessionState::Open;
            status.path = Some(p.layout.document.to_string_lossy().into_owned());
            match &p.phase {
                Phase::ReadOnly(reason) => {
                    status.read_only = Some(reason.to_string());
                    status.dirty = false;
                }
                Phase::AwaitingRecovery { available, .. } => {
                    status.recovery = Some(RecoveryInfo::from(available));
                    status.dirty = false;
                }
                Phase::Editing(e) => {
                    status.saved_rev = Some(e.autosave.saved_rev());
                    status.dirty = e.autosave.is_dirty();
                }
            }
        } else {
            status.dirty = snap.committed_rev != Rev::ZERO;
        }
        status
    }

    // ----- building ----------------------------------------------------------------------------

    /// An untitled document (`NoProject`).
    pub(crate) fn untitled(deps: &Deps, ids: Option<IdState>) -> Result<Session, CmdError> {
        let mut doc = Project::new(UNTITLED);
        doc.meta.created = Some(created_stamp(&*deps.clock));
        let tracker = deps
            .seed
            .map(|s| IdTracker::resume(s, ids.unwrap_or_default()));
        let config = doc_config(deps, tracker.as_ref(), doc.clone());
        let view = Arc::new(Mutex::new(ViewHost::Memory(ViewCtx::new(doc))));
        let running = spawn_doc(deps, config, &view)?;
        deps.subsystems.on_document(&running.handle, None);
        Ok(Session::assemble(running, tracker, view, None))
    }

    /// A session over a freshly opened store (see the module docs, steps 2–4). Sends
    /// `project.recovery_available` when unsaved work awaits a decision.
    pub(crate) fn open(
        deps: &Deps,
        opened: Opened,
        ids: Option<IdState>,
    ) -> Result<Session, CmdError> {
        let Opened {
            store,
            project: base,
            report,
        } = opened;
        let layout = store.layout().clone();
        let view = Arc::new(Mutex::new(ViewHost::Stored(UiStateStore::open_for(
            &store, &base,
        ))));
        for warning in lock(&view).take_warnings() {
            tracing::warn!(project = %layout.root.display(), "{warning}");
        }
        let read_only = store.read_only().cloned();
        let store: SharedStore = Arc::new(Mutex::new(store));
        let fresh = || {
            deps.seed
                .map(|s| IdTracker::resume(s, ids.unwrap_or_default()).past(&base))
        };
        let (running, tracker, phase) = if let Some(reason) = read_only {
            let tracker = fresh();
            let message = format!(
                "the project is open read-only ({reason}); save a copy with project.save_as"
            );
            let config = doc_config(deps, tracker.as_ref(), base).with_journal(refusing(message));
            (
                spawn_doc(deps, config, &view)?,
                tracker,
                Phase::ReadOnly(reason),
            )
        } else if let Some(available) = report.recovery.clone() {
            let tracker = fresh();
            let config = doc_config(deps, tracker.as_ref(), base.clone())
                .with_journal(refusing(RECOVERY_PENDING.to_owned()));
            let running = spawn_doc(deps, config, &view)?;
            deps.events
                .publish(EngineEvent::Project(ProjectEvent::recovery_available(
                    &available,
                )));
            (
                running,
                tracker,
                Phase::AwaitingRecovery {
                    base: Box::new(base),
                    available,
                },
            )
        } else {
            let recovered = lock(&store)
                .recover(&base, &RecoverOptions::from_clock(&*deps.clock))
                .map_err(|e| io_error(&e, "/path"))?;
            let saved_rev = recovered.rev;
            let (running, tracker, editing) =
                start_editing(deps, &store, &recovered, saved_rev, &view, ids)?;
            (running, tracker, Phase::Editing(editing))
        };
        deps.subsystems.on_document(&running.handle, Some(&layout));
        Ok(Session::assemble(
            running,
            tracker,
            view,
            Some(OpenProject {
                layout,
                store,
                phase,
            }),
        ))
    }

    fn assemble(
        doc: Doc,
        tracker: Option<IdTracker>,
        view: Arc<Mutex<ViewHost>>,
        project: Option<OpenProject>,
    ) -> Session {
        Session {
            actor: Some(doc.actor),
            handle: doc.handle,
            forwarder: Some(doc.forwarder),
            tracker,
            view,
            project,
        }
    }

    // ----- commands --------------------------------------------------------------------------

    /// `project.save`.
    pub(crate) fn save(&self) -> Result<(Rev, String), CmdError> {
        let Some(project) = &self.project else {
            return Err(CmdError::invalid(
                "",
                "the document has no project file yet; save it with project.save_as",
            ));
        };
        let rev = match &project.phase {
            Phase::Editing(e) => e.autosave.save_now().map_err(|e| io_error(&e, ""))?,
            Phase::ReadOnly(reason) => {
                return Err(CmdError::unsupported(format!(
                    "the project is open read-only ({reason}); save a copy with project.save_as"
                )));
            }
            Phase::AwaitingRecovery { .. } => {
                return Err(CmdError::unsupported(RECOVERY_PENDING));
            }
        };
        lock(&self.view).flush();
        Ok((rev, project.layout.document.to_string_lossy().into_owned()))
    }

    /// `project.set_autosave`: applies the preference to the running autosave, if there is one.
    pub(crate) fn apply_autosave(&self, pref: AutosavePreference) {
        if let Some(OpenProject {
            phase: Phase::Editing(e),
            ..
        }) = &self.project
        {
            e.autosave.apply_preference(pref);
        }
    }

    /// `project.save_as` of a journaled project: the store, its journal and its lock move to
    /// `folder`; the document and its history continue.
    pub(crate) fn save_as_in_place(&mut self, deps: &Deps, folder: &Path) -> Result<Rev, CmdError> {
        let Some(project) = &mut self.project else {
            return Err(CmdError::internal("save_as_in_place needs an open project"));
        };
        let Phase::Editing(editing) = &project.phase else {
            return Err(CmdError::internal(
                "save_as_in_place needs an editable project",
            ));
        };
        let snap = self.handle.committed_snapshot();
        // The old folder's view state goes along (best effort).
        lock(&self.view).flush();
        let new_layout = {
            let mut store = lock(&project.store);
            store
                .save_as_snapshot(folder, &snap)
                .map_err(|e| io_error(&e, "/path"))?;
            store.layout().clone()
        };
        editing.autosave.note_saved(snap.committed_rev);
        let old_ui = project.layout.ui_state_file();
        let new_ui = new_layout.ui_state_file();
        if old_ui.exists() {
            let _ = std::fs::create_dir_all(new_layout.meta_dir());
            let _ = std::fs::copy(&old_ui, &new_ui);
        }
        *lock(&self.view) = ViewHost::Stored(UiStateStore::open(&new_layout, &snap.doc, true));
        project.layout = new_layout;
        deps.events
            .publish(EngineEvent::Project(ProjectEvent::Saved(ProjectSaved {
                rev: snap.committed_rev,
                path: project.layout.document.to_string_lossy().into_owned(),
            })));
        Ok(snap.committed_rev)
    }

    /// Whether `project.save_as` can move this session in place (a journaled project), or
    /// must write a new project and reopen it (untitled or read-only).
    pub(crate) fn can_save_as_in_place(&self) -> Result<bool, CmdError> {
        match self.project.as_ref().map(|p| &p.phase) {
            Some(Phase::Editing(_)) => Ok(true),
            Some(Phase::ReadOnly(reason @ ReadOnlyReason::NewerSchema { .. })) => {
                Err(CmdError::unsupported(format!(
                    "this project was saved by a newer Cutweave and cannot be written: {reason}"
                )))
            }
            None | Some(Phase::ReadOnly(_)) => Ok(false),
            Some(Phase::AwaitingRecovery { .. }) => Err(CmdError::unsupported(RECOVERY_PENDING)),
        }
    }

    /// `project.recover` (`discard == false`) or `project.discard_recovery`: replays (or drops)
    /// the journal, starts it and replaces the read-only doc actor with a journaled one.
    pub(crate) fn decide_recovery(
        &mut self,
        deps: &Deps,
        discard: bool,
    ) -> Result<Recovered, CmdError> {
        let Some(project) = &mut self.project else {
            return Err(CmdError::invalid("", "no project is open"));
        };
        let Phase::AwaitingRecovery { base, available } = &project.phase else {
            return Err(CmdError::invalid(
                "",
                "no unsaved work awaits a decision in this project",
            ));
        };
        let options = RecoverOptions::from_clock(&*deps.clock);
        let recovered = {
            let mut store = lock(&project.store);
            if discard {
                store.discard_recovery(base, &options)
            } else {
                store.recover(base, &options)
            }
            .map_err(|e| io_error(&e, ""))?
        };
        // The document on disk is still the base: after a restore the journal's transactions
        // are unsaved (dirty), after a discard nothing is.
        let saved_rev = if discard {
            recovered.rev
        } else {
            available.base_rev
        };
        let ids = self.tracker.as_ref().map(IdTracker::state);
        let (running, tracker, editing) =
            start_editing(deps, &project.store, &recovered, saved_rev, &self.view, ids)?;
        project.phase = Phase::Editing(editing);
        let layout = project.layout.clone();
        self.swap_doc(deps, running, tracker, Some(&layout));
        Ok(recovered)
    }

    /// Installs `doc` in place of the current doc actor (which is drained and stopped), and
    /// sends `doc.reset`.
    fn swap_doc(
        &mut self,
        deps: &Deps,
        doc: Doc,
        tracker: Option<IdTracker>,
        layout: Option<&ProjectLayout>,
    ) {
        deps.subsystems.on_document_closing();
        let prev_rev = self.handle.snapshot().rev;
        let old_actor = self.actor.replace(doc.actor);
        let old_forwarder = self.forwarder.replace(doc.forwarder);
        self.handle = doc.handle;
        self.tracker = tracker;
        stop_doc(old_actor, old_forwarder);
        let snap = self.handle.snapshot();
        lock(&self.view).set_doc(&self.handle.committed_snapshot().doc);
        deps.events
            .publish(EngineEvent::Doc(Arc::new(DocEvent::Reset(DocReset {
                rev: snap.rev,
                prev_rev,
                committed_rev: snap.committed_rev,
            }))));
        deps.subsystems.on_document(&self.handle, layout);
        let view = lock(&self.view).with_ctx(|vc| vc.state().clone());
        deps.subsystems.on_view(&view, true);
    }

    /// Saves before closing when `save` says so, or by default (`None`) when autosave is on
    /// and the document is dirty. A failed save keeps the session open.
    pub(crate) fn save_before_close(&self, save: Option<bool>) -> Result<(), CmdError> {
        let Some(OpenProject {
            phase: Phase::Editing(e),
            ..
        }) = &self.project
        else {
            return match save {
                Some(true) if self.project.is_some() => Err(CmdError::unsupported(
                    "this project cannot be saved in its current state; close it with save: false",
                )),
                _ => Ok(()),
            };
        };
        let wanted = save.unwrap_or_else(|| e.autosave.config().enabled);
        if wanted && e.autosave.is_dirty() {
            e.autosave.save_now().map_err(|e| io_error(&e, ""))?;
        }
        Ok(())
    }

    /// Refuses a close that would leave changes unsaved without the caller saying so (ADR-0029):
    /// `Conflict{reason: unsaved_changes}` naming the project, with `how` (what the caller can
    /// pass instead) in the detail. `save: Some(false)` is that say-so and passes. Otherwise an
    /// untitled document with changes is refused (it has no file to save to), and so is a
    /// project with changes that the close would not save (`save` absent and autosave off). A
    /// read-only project or one awaiting a recovery decision has nothing unsaved to lose.
    pub(crate) fn guard_unsaved(&self, save: Option<bool>, how: &str) -> Result<(), CmdError> {
        if save == Some(false) {
            return Ok(());
        }
        let status = self.status();
        let at_risk = match &self.project {
            None => status.dirty,
            Some(OpenProject {
                phase: Phase::Editing(e),
                ..
            }) => status.dirty && !save.unwrap_or_else(|| e.autosave.config().enabled),
            Some(_) => false,
        };
        if !at_risk {
            return Ok(());
        }
        let detail = if self.project.is_none() {
            format!(
                "\"{}\" has unsaved changes and no file yet; save it with project.save_as, or {how}",
                status.name
            )
        } else {
            format!(
                "\"{}\" has unsaved changes; save them with project.save, or {how}",
                status.name
            )
        };
        Err(CmdError::conflict(ConflictReason::UnsavedChanges, detail))
    }

    /// Stops everything the session runs and closes its project (no save). Returns the closed
    /// project's document path.
    pub(crate) fn close(mut self, deps: &Deps) -> Option<PathBuf> {
        deps.subsystems.on_document_closing();
        let project = self.project.take();
        let (store, path) = match project {
            Some(OpenProject {
                layout,
                store,
                phase,
            }) => {
                if let Phase::Editing(editing) = phase {
                    editing.stop();
                }
                (Some(store), Some(layout.document))
            }
            None => (None, None),
        };
        stop_doc(self.actor.take(), self.forwarder.take());
        lock(&self.view).flush();
        drop(self);
        if let Some(store) = store {
            close_store(store);
        }
        path
    }
}

/// Publishes the session change after a swap.
pub(crate) fn announce_opened(deps: &Deps, status: ProjectStatus) {
    deps.events
        .publish(EngineEvent::Session(SessionEvent::Opened(status)));
}

/// Publishes `project.opening`: `actor` starts opening `path` (ADR-0029).
pub(crate) fn announce_opening(deps: &Deps, path: &Path, actor: &Actor) {
    deps.events
        .publish(EngineEvent::Session(SessionEvent::Opening(
            ProjectOpening {
                path: path.to_string_lossy().into_owned(),
                actor: actor.to_string(),
            },
        )));
}

/// Publishes `project.open_failed`: `actor`'s open of `path` failed with `error`, or was
/// refused before it started (ADR-0029).
pub(crate) fn announce_open_failed(deps: &Deps, path: &Path, actor: &Actor, error: &CmdError) {
    let reason = match error {
        CmdError::Conflict { reason, .. } => Some(reason.name().to_owned()),
        _ => None,
    };
    deps.events
        .publish(EngineEvent::Session(SessionEvent::OpenFailed(
            ProjectOpenFailed {
                path: path.to_string_lossy().into_owned(),
                actor: actor.to_string(),
                code: error.code().to_owned(),
                reason,
                message: error.message(),
            },
        )));
}

/// Publishes `project.closed`.
pub(crate) fn announce_closed(deps: &Deps, path: &Path) {
    deps.events
        .publish(EngineEvent::Session(SessionEvent::Closed(ProjectClosed {
            path: path.to_string_lossy().into_owned(),
        })));
}

/// Sends `doc.reset` from `prev_rev` to the new session's document.
pub(crate) fn announce_reset(deps: &Deps, prev_rev: Rev, session: &Session) {
    let snap = session.handle.snapshot();
    deps.events
        .publish(EngineEvent::Doc(Arc::new(DocEvent::Reset(DocReset {
            rev: snap.rev,
            prev_rev,
            committed_rev: snap.committed_rev,
        }))));
}

/// Closes a store once nothing else holds it: syncs and closes the journal, releases the lock.
fn close_store(store: SharedStore) {
    match Arc::try_unwrap(store) {
        Ok(mutex) => {
            let store = mutex.into_inner().unwrap_or_else(PoisonError::into_inner);
            if let Err(e) = store.close() {
                tracing::error!(error = %e, "the project did not close cleanly");
            }
        }
        Err(_) => {
            tracing::error!("the project store is still shared at close; it closes when released");
        }
    }
}

fn stop_doc(actor: Option<DocActor>, forwarder: Option<JoinHandle<()>>) {
    if let Some(actor) = actor
        && let Err(e) = actor.shutdown()
    {
        tracing::error!(error = %e, "the document actor did not stop cleanly");
    }
    if let Some(f) = forwarder
        && f.join().is_err()
    {
        tracing::error!("the document event forwarder panicked");
    }
}

/// A commit sink that refuses every commit and checkpoint change with `Unsupported(message)`:
/// a checkpoint made then would vanish with the recovery decision (WP-2I.03).
fn refusing(message: String) -> Box<dyn cutweave_commands::CommitSink> {
    struct Refusing(String);
    impl cutweave_commands::CommitSink for Refusing {
        fn commit(&mut self, _: &CommitRecord<'_>) -> Result<(), CmdError> {
            Err(CmdError::unsupported(self.0.clone()))
        }

        fn checkpoint(
            &mut self,
            _: &cutweave_commands::CheckpointRecord<'_>,
        ) -> Result<(), CmdError> {
            Err(CmdError::unsupported(self.0.clone()))
        }
    }
    Box::new(Refusing(message))
}

/// `meta.created` for a new document: the engine clock as `YYYY-MM-DDTHH:MM:SSZ`. Under
/// `--deterministic-ids` the clock is frozen, so this is `1970-01-01T00:00:00Z` and saved
/// documents are byte-reproducible (WP-1B.08).
fn created_stamp(clock: &dyn Clock) -> String {
    let at = std::time::UNIX_EPOCH + std::time::Duration::from_millis(clock.now().as_millis());
    let full = crate::logging::rfc3339(at);
    format!("{}Z", &full[..19])
}

/// The doc actor configuration of this engine for `doc`.
fn doc_config(deps: &Deps, tracker: Option<&IdTracker>, doc: Project) -> DocConfig {
    let config = match tracker {
        Some(t) => t.config(Arc::clone(&deps.registry), doc),
        None => DocConfig::new(Arc::clone(&deps.registry), doc),
    };
    // Transaction ids are per engine run (issue #251): a commit or cancel sent again for a
    // transaction an earlier run opened never names one this run opens.
    let config = config
        .with_clock(Arc::clone(&deps.clock))
        .with_tx_ids_per_run();
    match *lock(&deps.history_limit) {
        Some(steps) => config.with_history_limits(HistoryLimits {
            max_undo_depth: usize::try_from(steps).unwrap_or(usize::MAX),
            ..HistoryLimits::default()
        }),
        None => config,
    }
}

/// Starts the journal of `store` after a recovery decision, then the doc actor over
/// `recovered` and the autosave (module docs, step 4).
fn start_editing(
    deps: &Deps,
    store: &SharedStore,
    recovered: &Recovered,
    saved_rev: Rev,
    view: &Arc<Mutex<ViewHost>>,
    ids: Option<IdState>,
) -> Result<(Doc, Option<IdTracker>, Editing), CmdError> {
    let tracker = deps.seed.map(|seed| {
        let tracker = if recovered.ids.is_some() {
            recovered.id_tracker(seed)
        } else {
            // Nothing journaled yet: continue the streams this engine already used, so a
            // saved untitled document never sees its ids minted again.
            IdTracker::resume(seed, ids.unwrap_or_default())
        };
        // A new engine knows nothing of what earlier ones minted: skip the ids the document
        // already holds (WP-1B.08).
        tracker.past(&recovered.project)
    });
    let mut journal_config = JournalConfig::new(Arc::clone(&deps.clock))
        .with_sync(deps.journal_sync)
        .with_registry(Arc::clone(&deps.registry));
    if let Some(t) = &tracker {
        journal_config = journal_config.with_ids(t.clone());
    }
    let journal = lock(store)
        .start_journal(journal_config)
        .map_err(|e| io_error(&e, ""))?;
    let config = recovered
        .configure(doc_config(
            deps,
            tracker.as_ref(),
            recovered.project.clone(),
        ))
        .with_journal(journal.sink());
    let running = spawn_doc(deps, config, view)?;
    let settings = autosave_settings(&recovered.project);
    let mut config = deps
        .autosave
        .unwrap_or_else(|| AutosaveConfig::from(settings));
    if let Some(pref) = *lock(&deps.autosave_pref) {
        config = config.with_preference(pref);
    }
    let autosave = AutosaveService::new(
        running.handle.clone(),
        Arc::clone(store),
        Arc::clone(&deps.clock),
        config,
        saved_rev,
    );
    let events = autosave.subscribe();
    let bus = deps.events.clone();
    let forwarder = std::thread::Builder::new()
        .name("cutweave-project-events".into())
        .spawn(move || {
            for event in events {
                bus.publish(EngineEvent::Project(event));
            }
        })
        .map_err(|e| CmdError::internal(format!("cannot start a thread: {e}")))?;
    let task = autosave
        .spawn()
        .map_err(|e| CmdError::internal(format!("cannot start the autosave thread: {e}")))?;
    Ok((
        running,
        tracker,
        Editing {
            autosave,
            task: Some(task),
            forwarder: Some(forwarder),
        },
    ))
}

/// `settings.autosave` of the document (defaults when absent or malformed).
fn autosave_settings(doc: &Project) -> AutosaveSettings {
    doc.settings
        .extra
        .get("autosave")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

/// How often the document event forwarder wakes up to write debounced view state.
const VIEW_POLL: Duration = Duration::from_millis(250);

/// Spawns the doc actor and its event forwarder (document events into the fan-out, view
/// selection kept valid, debounced `ui-state.json` writes).
fn spawn_doc(deps: &Deps, config: DocConfig, view: &Arc<Mutex<ViewHost>>) -> Result<Doc, CmdError> {
    let actor = DocActor::spawn_with(config, ActorOptions::default())
        .map_err(|e| CmdError::internal(format!("cannot start the document actor: {e}")))?;
    let handle = actor.handle();
    let mut stream = handle.subscribe();
    let (bus, view, reader) = (deps.events.clone(), Arc::clone(view), handle.clone());
    let forwarder = std::thread::Builder::new()
        .name("cutweave-doc-events".into())
        .spawn(move || {
            loop {
                match stream.recv_timeout(VIEW_POLL) {
                    Ok(Some(event)) => {
                        let patched = matches!(&*event, DocEvent::Patched(_));
                        bus.publish(EngineEvent::Doc(event));
                        if patched {
                            let doc = reader.committed_snapshot();
                            let change = {
                                let mut view = lock(&view);
                                if view.set_doc(&doc.doc) {
                                    view.with_ctx(ViewCtx::take_event)
                                } else {
                                    None
                                }
                            };
                            if let Some(change) = change {
                                bus.publish(EngineEvent::View {
                                    change,
                                    origin: None,
                                });
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(RecvError::Lagged { .. }) => {
                        let snap = stream.resync();
                        bus.publish(EngineEvent::Doc(Arc::new(DocEvent::Reset(DocReset {
                            rev: snap.rev,
                            prev_rev: snap.rev,
                            committed_rev: snap.committed_rev,
                        }))));
                    }
                    Err(RecvError::Closed) => break,
                }
                lock(&view).poll();
            }
        })
        .map_err(|e| CmdError::internal(format!("cannot start a thread: {e}")))?;
    Ok(Doc {
        actor,
        handle,
        forwarder,
    })
}

/// Maps a project I/O failure to a command error; `pointer` names the path param, if any.
pub(crate) fn io_error(e: &ProjectIoError, pointer: &str) -> CmdError {
    let message = e.to_string();
    match e {
        ProjectIoError::Locked { .. }
        | ProjectIoError::NotFound(_)
        | ProjectIoError::Ambiguous(_)
        | ProjectIoError::AlreadyExists(_)
        | ProjectIoError::InvalidName { .. }
        | ProjectIoError::OutsideProject(_)
        | ProjectIoError::Document(_) => CmdError::invalid(pointer, message),
        ProjectIoError::ReadOnly(_) | ProjectIoError::Journal(JournalError::RecoveryPending) => {
            CmdError::unsupported(message)
        }
        ProjectIoError::Io { .. } | ProjectIoError::Serialize(_) | ProjectIoError::Journal(_) => {
            CmdError::internal(message)
        }
    }
}

/// Opens `path` for editing (the lock), as `project.open` and engine start do.
pub(crate) fn open_store(path: &Path) -> Result<Opened, ProjectIoError> {
    ProjectStore::open(path)
}
