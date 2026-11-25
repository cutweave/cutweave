// SPDX-License-Identifier: GPL-3.0-or-later
//! [`ExportQueue`]: the background export queue (WP-2G.03a; arch §7, ADR-0030).
//!
//! The queue holds jobs of kind [`JobKind::Export`] in order and runs them on the
//! [`JobManager`]: each is submitted as soon as it is queued (so `job.status`, `job.wait`,
//! `job.cancel` and `job.progress` see it from the start), held back by a [`Hold`] until its
//! turn, then released into the export budget. At most `slots` jobs run at once
//! ([`JobConfig::export_slots`](crate::JobConfig), default 1), first in line first; a paused
//! queue starts none (running ones finish).
//!
//! - **Persistence.** Jobs, their order and the pause live in a [`QueueStore`]: in memory for an
//!   untitled project, `<project>/.cutweave/exports.sqlite` for a project
//!   ([`SqliteQueueStore`]). A job's payload (for exports: the resolved plan and the committed
//!   document it renders) is encoded by the queue's [`TaskCodec`] when it is queued.
//! - **Restart.** [`attach`](ExportQueue::attach) loads a store: finished jobs are remembered by
//!   the manager ([`JobManager::record`]); queued ones are queued again with their IDs; a job that
//!   was running when the engine stopped or was killed is queued again from the beginning (its
//!   runner cleans up the partial output). Nothing starts before [`start`](ExportQueue::start).
//! - **Switching stores** (another project opened): the current jobs either move into the new
//!   store and keep running (`Attach::Carry`, an untitled project's queue) or stay in the old
//!   one (`Attach::Suspend`): its running jobs stop there and start again from the beginning
//!   when that project is opened again.
//! - **Shutdown** ([`shutdown`](ExportQueue::shutdown)): the queue stops writing, so the jobs the
//!   engine then cancels keep their stored state and run again next time.
//!
//! State changes come from the manager's completion callback ([`Submit::on_done`]), before
//! `job.wait` returns, so `export.queue` is up to date when a waiter wakes. The queue's lock is
//! never held while calling into the manager in a way that can call back (cancelling).

pub mod state;
pub mod store;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

pub use state::{QueueEvent, next_state};
pub use store::{
    MemoryQueueStore, QUEUE_FILE, QUEUE_SCHEMA_VERSION, QueueStore, SqliteQueueStore, StoreError,
    StoredEntry, StoredQueue,
};

use crate::{
    hold::Hold,
    id::JobId,
    manager::{JobCtx, JobManager, Submit, SubmitError},
    types::{JobError, JobInfo, JobKind, JobProgress, JobSpec, JobState},
};

/// Encodes a queue's jobs for its store.
pub trait TaskCodec<T>: Send + Sync {
    /// The bytes stored for `task`.
    ///
    /// # Errors
    /// When the task cannot be encoded (the job is then not queued).
    fn encode(&self, task: &T) -> Result<Vec<u8>, String>;
    /// The task stored as `bytes`.
    ///
    /// # Errors
    /// When the bytes are not a task (the job is then dropped from the store and recorded as
    /// failed).
    fn decode(&self, bytes: &[u8]) -> Result<T, String>;
}

/// Runs one queued job: the body of its manager job.
pub type Runner<T> = Arc<dyn Fn(&T, &JobCtx) -> Result<(), JobError> + Send + Sync>;

/// A job to queue.
#[derive(Clone, Debug)]
pub struct QueueItem<T> {
    /// The job label (`export`).
    pub label: String,
    /// The file it writes: two unfinished jobs never write the same file.
    pub output: Option<PathBuf>,
    /// What to run.
    pub task: T,
}

/// A queued job as [`ExportQueue::view`] reports it.
#[derive(Debug)]
pub struct QueueEntry<T> {
    /// Its live state (from the manager while it knows it).
    pub info: JobInfo,
    /// The file it writes.
    pub output: Option<PathBuf>,
    /// What it runs.
    pub task: Arc<T>,
}

impl<T> Clone for QueueEntry<T> {
    fn clone(&self) -> QueueEntry<T> {
        QueueEntry {
            info: self.info.clone(),
            output: self.output.clone(),
            task: Arc::clone(&self.task),
        }
    }
}

/// The queue as [`ExportQueue::view`] reports it.
#[derive(Debug)]
pub struct QueueView<T> {
    /// Whether no job starts.
    pub paused: bool,
    /// How many jobs run at once.
    pub slots: usize,
    /// Where the jobs are kept (a file, or `memory`).
    pub store: String,
    /// The jobs, first in line first (finished ones stay until removed).
    pub entries: Vec<QueueEntry<T>>,
}

/// How [`ExportQueue::attach`] treats the jobs of the store it replaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attach {
    /// They move into the new store and keep running (an untitled project's queue).
    Carry,
    /// They stay in the old store: running ones stop and start again when it is attached
    /// again.
    Suspend,
}

/// Why a queue operation was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum QueueError {
    /// No job of the queue has this ID.
    #[error("the export queue has no job `{0}`")]
    NotFound(JobId),
    /// The operation does not apply to the job in its state (the message says why and what to
    /// do).
    #[error("{0}")]
    Invalid(String),
    /// Another unfinished job writes this file.
    #[error(
        "another export is already writing `{}`; wait for it (job.wait) or cancel it",
        .0.display()
    )]
    OutputInUse(PathBuf),
    /// The store failed.
    #[error("the export queue could not be saved: {0}")]
    Store(String),
    /// The queue was shut down.
    #[error("the export queue is shut down")]
    ShutDown,
}

impl From<StoreError> for QueueError {
    fn from(e: StoreError) -> QueueError {
        QueueError::Store(e.0)
    }
}

/// One job in the queue.
struct Entry<T> {
    id: JobId,
    label: String,
    output: Option<PathBuf>,
    task: Arc<T>,
    /// The state as the queue knows it (and stored): `running` from its dispatch.
    state: JobState,
    error: Option<String>,
    /// Released at dispatch.
    hold: Option<Hold>,
}

impl<T> Entry<T> {
    /// What the manager would report, from the queue's own knowledge.
    fn info(&self) -> JobInfo {
        JobInfo {
            id: self.id,
            kind: JobKind::Export,
            label: self.label.clone(),
            media_id: None,
            state: self.state,
            progress: JobProgress::new(
                if self.state == JobState::Succeeded {
                    1.0
                } else {
                    0.0
                },
                None,
            ),
            cancel_requested: false,
            error: self.error.clone(),
        }
    }
}

struct Inner<T> {
    entries: Vec<Entry<T>>,
    paused: bool,
    /// Jobs may start (the engine is up).
    started: bool,
    /// Shut down: nothing is written or started any more.
    detached: bool,
    store: Box<dyn QueueStore>,
    /// Bumped whenever the jobs of a store leave the queue: their callbacks are ignored.
    generation: u64,
}

impl<T> Inner<T> {
    fn position(&self, id: JobId) -> Result<usize, QueueError> {
        self.entries
            .iter()
            .position(|e| e.id == id)
            .ok_or(QueueError::NotFound(id))
    }

    /// Writes a state change, logging a failure (the in-memory queue goes on).
    fn persist(&mut self, id: JobId, state: JobState, error: Option<&str>) {
        if let Err(e) = self.store.update(id, state, error) {
            tracing::warn!(job = %id, error = %e, store = %self.store.describe(), "cannot save an export job's state");
        }
    }
}

struct Shared<T> {
    jobs: JobManager,
    runner: Runner<T>,
    codec: Arc<dyn TaskCodec<T>>,
    slots: usize,
    inner: Mutex<Inner<T>>,
}

/// The comparison key of an output file (ASCII case-insensitive on Windows).
fn output_key(path: &Path) -> String {
    let s = path.to_string_lossy();
    if cfg!(windows) {
        s.replace('/', "\\").to_ascii_lowercase()
    } else {
        s.into_owned()
    }
}

impl<T: Send + Sync + 'static> Shared<T> {
    fn lock(&self) -> MutexGuard<'_, Inner<T>> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Submits entry `index` to the manager, held until its turn.
    fn submit(self: &Arc<Self>, inner: &mut Inner<T>, index: usize) -> Result<(), SubmitError> {
        let generation = inner.generation;
        let entry = &mut inner.entries[index];
        let hold = Hold::new();
        let weak: Weak<Shared<T>> = Arc::downgrade(self);
        let runner = Arc::clone(&self.runner);
        let task = Arc::clone(&entry.task);
        self.jobs.submit_with(
            JobSpec::new(JobKind::Export, entry.label.clone()),
            Submit {
                id: Some(entry.id),
                hold: Some(hold.clone()),
                on_done: Some(Box::new(move |info: &JobInfo| {
                    if let Some(shared) = weak.upgrade() {
                        shared.finished(generation, info);
                    }
                })),
            },
            move |ctx| runner(&task, ctx),
        )?;
        entry.state = JobState::Queued;
        entry.hold = Some(hold);
        Ok(())
    }

    /// A job of the queue became final.
    fn finished(&self, generation: u64, info: &JobInfo) {
        let mut inner = self.lock();
        if inner.detached || inner.generation != generation {
            return;
        }
        let Ok(index) = inner.position(info.id) else {
            return; // removed meanwhile
        };
        let entry = &mut inner.entries[index];
        let Some(state) = next_state(entry.state, QueueEvent::Finish(info.state)) else {
            return;
        };
        entry.state = state;
        entry.error.clone_from(&info.error);
        entry.hold = None;
        let error = entry.error.clone();
        inner.persist(info.id, state, error.as_deref());
        self.dispatch(&mut inner);
    }

    /// Starts the first queued jobs while slots are free.
    fn dispatch(&self, inner: &mut Inner<T>) {
        if !inner.started || inner.paused || inner.detached {
            return;
        }
        let running = inner
            .entries
            .iter()
            .filter(|e| e.state == JobState::Running)
            .count();
        let mut free = self.slots.saturating_sub(running);
        let mut started = Vec::new();
        for entry in &mut inner.entries {
            if free == 0 {
                break;
            }
            if entry.state != JobState::Queued {
                continue;
            }
            let Some(state) = next_state(entry.state, QueueEvent::Dispatch) else {
                continue;
            };
            if let Some(hold) = entry.hold.take() {
                hold.release();
            }
            entry.state = state;
            started.push(entry.id);
            free -= 1;
        }
        for id in started {
            inner.persist(id, JobState::Running, None);
        }
    }

    /// Whether an unfinished job other than `except` writes `output`.
    fn output_busy(inner: &Inner<T>, output: &Path, except: Option<JobId>) -> bool {
        let key = output_key(output);
        inner.entries.iter().any(|e| {
            Some(e.id) != except
                && !e.state.is_terminal()
                && e.output.as_deref().is_some_and(|o| output_key(o) == key)
        })
    }
}

/// The export queue (see the module docs). Cheap to clone.
pub struct ExportQueue<T> {
    shared: Arc<Shared<T>>,
}

impl<T> Clone for ExportQueue<T> {
    fn clone(&self) -> ExportQueue<T> {
        ExportQueue {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl<T> std::fmt::Debug for ExportQueue<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExportQueue")
            .field("slots", &self.shared.slots)
            .finish_non_exhaustive()
    }
}

impl<T: Send + Sync + 'static> ExportQueue<T> {
    /// An empty, unpaused queue in memory, running jobs with `runner` on `jobs`, at most
    /// `slots` at once (at least one). Nothing starts before [`start`](Self::start).
    #[must_use]
    pub fn new(
        jobs: JobManager,
        runner: Runner<T>,
        codec: Arc<dyn TaskCodec<T>>,
        slots: usize,
    ) -> ExportQueue<T> {
        ExportQueue {
            shared: Arc::new(Shared {
                jobs,
                runner,
                codec,
                slots: slots.max(1),
                inner: Mutex::new(Inner {
                    entries: Vec::new(),
                    paused: false,
                    started: false,
                    detached: false,
                    store: Box::new(MemoryQueueStore),
                    generation: 0,
                }),
            }),
        }
    }

    /// Lets jobs start (the engine can run them now).
    pub fn start(&self) {
        let mut inner = self.shared.lock();
        inner.started = true;
        self.shared.dispatch(&mut inner);
    }

    /// Stops writing and starting: the jobs the engine cancels next keep their stored state
    /// (running ones start again next time).
    pub fn shutdown(&self) {
        let mut inner = self.shared.lock();
        inner.detached = true;
        inner.generation += 1;
    }

    /// Whether the queue owns the job `id` (an unfinished or finished job still listed).
    #[must_use]
    pub fn owns(&self, id: JobId) -> bool {
        self.shared.lock().entries.iter().any(|e| e.id == id)
    }

    /// Queues `item` last and returns its job. With `resume`, a paused queue resumes.
    ///
    /// # Errors
    /// `OutputInUse` when an unfinished job writes the same file, `Store` when it cannot be
    /// stored, `ShutDown` after [`shutdown`](Self::shutdown).
    pub fn enqueue(&self, item: QueueItem<T>, resume: bool) -> Result<JobId, QueueError> {
        let QueueItem {
            label,
            output,
            task,
        } = item;
        let task = Arc::new(task);
        let persistent = self.shared.lock().store.persistent();
        // Encoding (a whole document for exports) happens outside the lock.
        let payload = if persistent {
            self.shared.codec.encode(&task).map_err(QueueError::Store)?
        } else {
            Vec::new()
        };
        let mut inner = self.shared.lock();
        if inner.detached {
            return Err(QueueError::ShutDown);
        }
        if let Some(out) = &output
            && Shared::output_busy(&inner, out, None)
        {
            return Err(QueueError::OutputInUse(out.clone()));
        }
        let id = self.shared.jobs.mint();
        let stored = StoredEntry {
            id,
            label: label.clone(),
            output: output.as_ref().map(|p| p.to_string_lossy().into_owned()),
            state: JobState::Queued,
            error: None,
            payload,
        };
        if inner.store.persistent() == persistent {
            inner.store.insert(&stored)?;
        } else {
            // The store changed while encoding: encode again under the lock.
            let payload = if inner.store.persistent() {
                self.shared.codec.encode(&task).map_err(QueueError::Store)?
            } else {
                Vec::new()
            };
            inner.store.insert(&StoredEntry { payload, ..stored })?;
        }
        inner.entries.push(Entry {
            id,
            label,
            output,
            task,
            state: JobState::Queued,
            error: None,
            hold: None,
        });
        let index = inner.entries.len() - 1;
        if let Err(e) = self.shared.submit(&mut inner, index) {
            inner.entries.pop();
            let _ = inner.store.remove(id);
            return Err(QueueError::Invalid(e.to_string()));
        }
        if resume && inner.paused {
            inner.paused = false;
            if let Err(e) = inner.store.set_paused(false) {
                tracing::warn!(error = %e, "cannot save the export queue's pause");
            }
        }
        self.shared.dispatch(&mut inner);
        Ok(id)
    }

    /// Pauses (no job starts; running ones finish) or resumes the queue.
    ///
    /// # Errors
    /// `ShutDown` after [`shutdown`](Self::shutdown).
    pub fn pause(&self, paused: bool) -> Result<(), QueueError> {
        let mut inner = self.shared.lock();
        if inner.detached {
            return Err(QueueError::ShutDown);
        }
        inner.paused = paused;
        if let Err(e) = inner.store.set_paused(paused) {
            tracing::warn!(error = %e, "cannot save the export queue's pause");
        }
        self.shared.dispatch(&mut inner);
        Ok(())
    }

    /// Moves the queued job `id` before `before` (`None`: to the end of the queue).
    ///
    /// # Errors
    /// `NotFound` for an unknown job (either one), `Invalid` when `id` is no longer waiting for
    /// its turn or is `before` itself.
    pub fn reorder(&self, id: JobId, before: Option<JobId>) -> Result<(), QueueError> {
        let mut inner = self.shared.lock();
        let from = inner.position(id)?;
        let state = inner.entries[from].state;
        if state != JobState::Queued {
            return Err(QueueError::Invalid(format!(
                "`{id}` is {}; only an export waiting for its turn can move",
                state_word(state)
            )));
        }
        if before == Some(id) {
            return Err(QueueError::Invalid(format!(
                "`{id}` cannot move before itself"
            )));
        }
        if let Some(before) = before {
            inner.position(before)?;
        }
        let entry = inner.entries.remove(from);
        let to = match before {
            Some(before) => inner.position(before)?,
            None => inner.entries.len(),
        };
        inner.entries.insert(to, entry);
        let order: Vec<JobId> = inner.entries.iter().map(|e| e.id).collect();
        if let Err(e) = inner.store.reorder(&order) {
            tracing::warn!(error = %e, "cannot save the export queue's order");
        }
        self.shared.dispatch(&mut inner);
        Ok(())
    }

    /// Queues the failed or cancelled job `id` again, in its place, with the same ID and the
    /// same snapshot.
    ///
    /// # Errors
    /// `NotFound`, `Invalid` when it did not fail and was not cancelled, `OutputInUse` when an
    /// unfinished job writes its file meanwhile.
    pub fn retry(&self, id: JobId) -> Result<(), QueueError> {
        let mut inner = self.shared.lock();
        if inner.detached {
            return Err(QueueError::ShutDown);
        }
        let index = inner.position(id)?;
        let entry = &inner.entries[index];
        let Some(state) = next_state(entry.state, QueueEvent::Retry) else {
            return Err(QueueError::Invalid(match entry.state {
                JobState::Succeeded => {
                    format!("`{id}` succeeded; export again with export.start or export.enqueue")
                }
                other => format!(
                    "`{id}` is {}; only a failed or cancelled export can be retried",
                    state_word(other)
                ),
            }));
        };
        if let Some(out) = &entry.output
            && Shared::output_busy(&inner, out, Some(id))
        {
            return Err(QueueError::OutputInUse(out.clone()));
        }
        if let Err(e) = self.shared.submit(&mut inner, index) {
            return Err(QueueError::Invalid(e.to_string()));
        }
        let entry = &mut inner.entries[index];
        entry.state = state;
        entry.error = None;
        inner.persist(id, state, None);
        self.shared.dispatch(&mut inner);
        Ok(())
    }

    /// Takes job `id` off the queue: a finished one is forgotten (its file stays), a queued
    /// one is cancelled first.
    ///
    /// # Errors
    /// `NotFound`, `Invalid` while it runs (cancel it first).
    pub fn remove(&self, id: JobId) -> Result<(), QueueError> {
        let cancel = {
            let mut inner = self.shared.lock();
            let index = inner.position(id)?;
            let state = inner.entries[index].state;
            if state == JobState::Running {
                return Err(QueueError::Invalid(format!(
                    "`{id}` is running; cancel it first (export.cancel)"
                )));
            }
            inner.entries.remove(index);
            if let Err(e) = inner.store.remove(id) {
                tracing::warn!(job = %id, error = %e, "cannot remove an export job from the store");
            }
            state == JobState::Queued
        };
        if cancel {
            // Outside the lock: the manager's callback finds the job gone.
            let _ = self.shared.jobs.cancel(id);
        }
        Ok(())
    }

    /// Cancels job `id`: a queued one ends at once, a running one stops within a second (its
    /// partial file removed) and the next one starts. Cancelling a finished job changes
    /// nothing. Returns its state afterwards.
    ///
    /// # Errors
    /// `NotFound` when the queue has no such job.
    pub fn cancel(&self, id: JobId) -> Result<JobInfo, QueueError> {
        let known = {
            let inner = self.shared.lock();
            let index = inner.position(id)?;
            inner.entries[index].info()
        };
        // Outside the lock: a queued job's callback runs inside `cancel`.
        Ok(self.shared.jobs.cancel(id).unwrap_or(known))
    }

    /// The queue now.
    #[must_use]
    pub fn view(&self) -> QueueView<T> {
        let inner = self.shared.lock();
        QueueView {
            paused: inner.paused,
            slots: self.shared.slots,
            store: inner.store.describe(),
            entries: inner
                .entries
                .iter()
                .map(|e| QueueEntry {
                    info: self.live(e),
                    output: e.output.clone(),
                    task: Arc::clone(&e.task),
                })
                .collect(),
        }
    }

    /// The job `id` of the queue.
    ///
    /// # Errors
    /// `NotFound`.
    pub fn entry(&self, id: JobId) -> Result<QueueEntry<T>, QueueError> {
        let inner = self.shared.lock();
        let index = inner.position(id)?;
        let e = &inner.entries[index];
        Ok(QueueEntry {
            info: self.live(e),
            output: e.output.clone(),
            task: Arc::clone(&e.task),
        })
    }

    /// The manager's state of `e` (with its progress), or the queue's own once the manager
    /// forgot the job.
    fn live(&self, e: &Entry<T>) -> JobInfo {
        self.shared.jobs.status(e.id).unwrap_or_else(|| e.info())
    }

    /// Replaces the store with `store` and loads its jobs (see the module docs). The jobs of
    /// the old store move along (`Carry`) or stay behind (`Suspend`). Returns the outputs of
    /// jobs that were running when the store was last written (interrupted: their partial
    /// files may need cleaning; the runner does that when the job runs again).
    #[allow(clippy::must_use_candidate)] // called for its effect; the list is informative
    pub fn attach(&self, store: Box<dyn QueueStore>, mode: Attach) -> Vec<PathBuf> {
        let mut store = store;
        let mut stop = Vec::new();
        let interrupted = {
            let mut inner = self.shared.lock();
            if inner.detached {
                return Vec::new();
            }
            let carried: Vec<Entry<T>> = match mode {
                Attach::Carry => std::mem::take(&mut inner.entries),
                Attach::Suspend => {
                    stop = inner
                        .entries
                        .iter()
                        .filter(|e| !e.state.is_terminal())
                        .map(|e| e.id)
                        .collect();
                    inner.entries.clear();
                    inner.generation += 1;
                    Vec::new()
                }
            };
            let loaded = store.load().unwrap_or_else(|e| {
                tracing::warn!(store = %store.describe(), error = %e, "cannot read the export queue; starting it empty");
                StoredQueue::default()
            });
            inner.paused = loaded.paused || (mode == Attach::Carry && inner.paused);
            inner.store = store;
            if mode == Attach::Carry && inner.paused {
                let _ = inner.store.set_paused(true);
            }
            let interrupted = self.restore(&mut inner, loaded.entries, &carried);
            // Carried jobs follow the stored ones, written into the new store as they are.
            for entry in carried {
                self.save_carried(&mut inner, &entry);
                inner.entries.push(entry);
            }
            self.shared.dispatch(&mut inner);
            interrupted
        };
        // Outside the lock: their callbacks belong to the old generation and are ignored.
        for id in stop {
            let _ = self.shared.jobs.cancel(id);
        }
        interrupted
    }

    /// Puts the stored jobs into the queue (see [`attach`](Self::attach)); returns the outputs
    /// of interrupted ones.
    fn restore(
        &self,
        inner: &mut Inner<T>,
        stored: Vec<StoredEntry>,
        carried: &[Entry<T>],
    ) -> Vec<PathBuf> {
        let mut interrupted = Vec::new();
        for stored in stored {
            if carried.iter().any(|c| c.id == stored.id) {
                continue;
            }
            let task = match self.shared.codec.decode(&stored.payload) {
                Ok(task) => Arc::new(task),
                Err(e) => {
                    tracing::warn!(job = %stored.id, error = %e, "dropping an unreadable queued export");
                    let _ = inner.store.remove(stored.id);
                    continue;
                }
            };
            if stored.state == JobState::Running {
                tracing::info!(job = %stored.id, "an export was interrupted; it starts again from the beginning");
                if let Some(out) = &stored.output {
                    interrupted.push(PathBuf::from(out));
                }
            }
            let state = if stored.state.is_terminal() {
                stored.state
            } else {
                next_state(stored.state, QueueEvent::Interrupt).unwrap_or(JobState::Queued)
            };
            let id = stored.id;
            inner.entries.push(Entry {
                id,
                label: stored.label,
                output: stored.output.map(PathBuf::from),
                task,
                state,
                error: stored.error,
                hold: None,
            });
            let index = inner.entries.len() - 1;
            if state.is_terminal() {
                self.shared.jobs.record(&inner.entries[index].info());
                continue;
            }
            if let Err(e) = self.shared.submit(inner, index) {
                tracing::warn!(job = %id, error = %e, "cannot queue a stored export again");
                inner.entries.pop();
                continue;
            }
            inner.persist(id, JobState::Queued, None);
        }
        interrupted
    }

    /// Writes a carried job into the (new) store.
    fn save_carried(&self, inner: &mut Inner<T>, entry: &Entry<T>) {
        if !inner.store.persistent() {
            return;
        }
        let saved = self
            .shared
            .codec
            .encode(&entry.task)
            .map_err(StoreError)
            .and_then(|payload| {
                inner.store.insert(&StoredEntry {
                    id: entry.id,
                    label: entry.label.clone(),
                    output: entry
                        .output
                        .as_ref()
                        .map(|p| p.to_string_lossy().into_owned()),
                    state: entry.state,
                    error: entry.error.clone(),
                    payload,
                })
            });
        if let Err(e) = saved {
            tracing::warn!(job = %entry.id, error = %e, "cannot save a queued export");
        }
    }
}

/// `queued`, `running`, … for messages.
fn state_word(state: JobState) -> &'static str {
    match state {
        JobState::Queued => "queued",
        JobState::Running => "running",
        JobState::Succeeded => "finished",
        JobState::Failed => "failed",
        JobState::Cancelled => "cancelled",
    }
}

#[cfg(test)]
mod tests;
