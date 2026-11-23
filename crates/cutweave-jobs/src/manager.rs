// SPDX-License-Identifier: GPL-3.0-or-later
//! [`JobManager`]: runs job bodies on tokio's blocking pool under per-class budgets, tracks
//! their state and progress, and broadcasts `job.progress`.
//!
//! # Threads
//!
//! Each job is one small async task on the manager's runtime: it waits for the jobs it depends
//! on, then for a slot of its [`Budget`] semaphore (without holding a thread), then runs the
//! body with `spawn_blocking` and releases the slot when the body returns. Bodies are plain
//! blocking code (decoders, file I/O) and poll their [`CancelToken`]. The runtime is either
//! owned (one worker thread for the coordination tasks; [`JobManager::new`]) or the engine's
//! ([`JobManagerBuilder::runtime`]).
//!
//! The job table sits behind one `std` mutex, held only for short bookkeeping (never across an
//! `.await` or a body), and a condition variable wakes synchronous [`JobManager::wait`]ers.

use std::{
    any::Any,
    collections::BTreeMap,
    fmt,
    panic::{self, AssertUnwindSafe},
    pin::pin,
    sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError},
    thread,
    time::{Duration, Instant},
};

use cutweave_model::{IdGen, Uuid7Gen};
use tokio::{
    runtime::{Handle, Runtime},
    sync::{Notify, Semaphore, broadcast},
};

use crate::{
    cancel::CancelToken,
    history::{DEFAULT_HISTORY_CAPACITY, JobHistory, MemoryHistory},
    id::JobId,
    types::{Budget, JobError, JobEvent, JobFilter, JobInfo, JobProgress, JobSpec, JobState},
};

/// Default capacity of the `job.progress` broadcast; a slower subscriber gets `Lagged` and
/// re-reads `job.list`.
pub const DEFAULT_EVENT_CAPACITY: usize = 1024;
/// Default number of concurrent export jobs.
pub const DEFAULT_EXPORT_SLOTS: usize = 1;
/// Smallest progress change that is broadcast (message changes always are).
pub const PROGRESS_EVENT_STEP: f64 = 0.01;

/// Default number of concurrent decode-class jobs: half the CPU cores, at least one.
#[must_use]
pub fn default_decode_slots() -> usize {
    thread::available_parallelism().map_or(1, |n| (n.get() / 2).max(1))
}

/// Sizes of the budgets and buffers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobConfig {
    /// Concurrent [`Budget::Decode`] jobs (default: [`default_decode_slots`]).
    pub decode_slots: usize,
    /// Concurrent [`Budget::Export`] jobs (default 1).
    pub export_slots: usize,
    /// Capacity of the `job.progress` broadcast.
    pub event_capacity: usize,
    /// Finished jobs kept by the default in-memory history.
    pub history_capacity: usize,
}

impl Default for JobConfig {
    fn default() -> JobConfig {
        JobConfig {
            decode_slots: default_decode_slots(),
            export_slots: DEFAULT_EXPORT_SLOTS,
            event_capacity: DEFAULT_EVENT_CAPACITY,
            history_capacity: DEFAULT_HISTORY_CAPACITY,
        }
    }
}

type Body = Box<dyn FnOnce(&JobCtx) -> Result<(), JobError> + Send + 'static>;

struct Active {
    info: JobInfo,
    token: CancelToken,
    /// Progress last broadcast, to rate-limit `job.progress`.
    sent: JobProgress,
}

type Table = BTreeMap<JobId, Active>;

/// State shared by the manager, job handles, job contexts and the job tasks.
struct Shared {
    table: Mutex<Table>,
    /// Signalled whenever a job reaches a final state (synchronous waiters).
    finished: Condvar,
    /// Notified whenever a job reaches a final state (async waiters and dependencies).
    finished_async: Notify,
    events: broadcast::Sender<JobEvent>,
    history: Arc<dyn JobHistory>,
    ids: Mutex<Box<dyn IdGen + Send>>,
    decode: Arc<Semaphore>,
    export: Arc<Semaphore>,
    config: JobConfig,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Table> {
        self.table.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn emit(&self, info: &JobInfo) {
        // No subscribers is fine.
        let _ = self.events.send(JobEvent::Progress(info.clone()));
    }

    fn semaphore(&self, budget: Budget) -> &Arc<Semaphore> {
        match budget {
            Budget::Decode => &self.decode,
            Budget::Export => &self.export,
        }
    }

    fn lookup(&self, table: &Table, id: JobId) -> Option<JobInfo> {
        match table.get(&id) {
            Some(job) => Some(job.info.clone()),
            None => self.history.get(id),
        }
    }

    fn status(&self, id: JobId) -> Option<JobInfo> {
        let table = self.lock();
        self.lookup(&table, id)
    }

    /// Moves job `id` to its final `state` (under the caller's lock). The caller then calls
    /// [`notify_finished`](Self::notify_finished) after unlocking.
    fn finish_locked(
        &self,
        table: &mut Table,
        id: JobId,
        state: JobState,
        error: Option<String>,
    ) -> Option<JobInfo> {
        let mut job = table.remove(&id)?;
        job.info.state = state;
        if state == JobState::Succeeded {
            job.info.progress.fraction = 1.0;
        }
        if error.is_some() {
            job.info.error = error;
        }
        job.info.cancel_requested |= job.token.is_cancelled();
        self.history.record(&job.info);
        self.emit(&job.info);
        Some(job.info)
    }

    fn notify_finished(&self) {
        self.finished.notify_all();
        self.finished_async.notify_waiters();
    }

    fn finish(&self, id: JobId, state: JobState, error: Option<String>) {
        let done = {
            let mut table = self.lock();
            self.finish_locked(&mut table, id, state, error)
        };
        if done.is_some() {
            self.notify_finished();
        }
    }

    /// Marks a queued job running; `false` if it was cancelled (and finished) meanwhile.
    fn try_start(&self, id: JobId) -> bool {
        let mut table = self.lock();
        match table.get_mut(&id) {
            Some(job) if job.info.state == JobState::Queued && !job.token.is_cancelled() => {
                job.info.state = JobState::Running;
                self.emit(&job.info);
                true
            }
            _ => false,
        }
    }

    fn progress(&self, id: JobId, fraction: f64, message: Option<&str>) {
        if !fraction.is_finite() {
            return;
        }
        let fraction = fraction.clamp(0.0, 1.0);
        let mut table = self.lock();
        let Some(job) = table.get_mut(&id) else {
            return;
        };
        if job.info.state != JobState::Running {
            return;
        }
        let message = message.map(str::to_owned);
        let changed_message = message != job.sent.message;
        let moved = (fraction - job.sent.fraction).abs() >= PROGRESS_EVENT_STEP
            || (fraction >= 1.0 && job.sent.fraction < 1.0);
        job.info.progress = JobProgress { fraction, message };
        if changed_message || moved {
            job.sent = job.info.progress.clone();
            self.emit(&job.info);
        }
    }

    fn cancel(&self, id: JobId) -> Option<JobInfo> {
        let mut table = self.lock();
        let Some(job) = table.get_mut(&id) else {
            return self.history.get(id);
        };
        job.token.cancel();
        if job.info.state != JobState::Queued {
            if !job.info.cancel_requested {
                job.info.cancel_requested = true;
                self.emit(&job.info);
            }
            return Some(job.info.clone());
        }
        // Not started yet: it ends now, and its task notices when it wakes.
        let info = self.finish_locked(&mut table, id, JobState::Cancelled, None);
        drop(table);
        self.notify_finished();
        info
    }

    fn wait(&self, id: JobId, timeout: Duration) -> Option<JobInfo> {
        let deadline = Instant::now().checked_add(timeout);
        let mut table = self.lock();
        loop {
            let info = self.lookup(&table, id)?;
            if info.state.is_terminal() {
                return Some(info);
            }
            let now = Instant::now();
            let remaining = match deadline {
                Some(d) if d > now => d - now,
                Some(_) => return Some(info),
                None => Duration::from_secs(3600),
            };
            table = self
                .finished
                .wait_timeout(table, remaining)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Completes once job `id` is final (or unknown).
    async fn finished(&self, id: JobId) {
        loop {
            let mut notified = pin!(self.finished_async.notified());
            notified.as_mut().enable();
            match self.status(id) {
                Some(info) if !info.state.is_terminal() => {}
                _ => return,
            }
            notified.await;
        }
    }

    /// Why a job whose dependencies are all final must not run, if one of them did not
    /// succeed.
    fn blocked_by(&self, after: &[JobId]) -> Option<(JobState, String)> {
        for dep in after {
            let Some(info) = self.status(*dep) else {
                tracing::warn!(job = %dep, "unknown job dependency; treated as satisfied");
                continue;
            };
            match info.state {
                JobState::Succeeded => {}
                JobState::Cancelled => {
                    return Some((
                        JobState::Cancelled,
                        format!("`{}` ({dep}) was cancelled", info.label),
                    ));
                }
                JobState::Failed | JobState::Queued | JobState::Running => {
                    return Some((
                        JobState::Failed,
                        format!("skipped: `{}` ({dep}) did not succeed", info.label),
                    ));
                }
            }
        }
        None
    }

    fn cancel_all(&self) {
        let ids: Vec<JobId> = self.lock().keys().copied().collect();
        for id in ids {
            self.cancel(id);
        }
    }
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_owned())
}

/// One job's lifecycle (see the module docs).
async fn run(
    shared: Arc<Shared>,
    id: JobId,
    budget: Budget,
    after: Vec<JobId>,
    token: CancelToken,
    body: Body,
) {
    for dep in &after {
        tokio::select! {
            biased;
            () = token.cancelled() => {
                shared.finish(id, JobState::Cancelled, None);
                return;
            }
            () = shared.finished(*dep) => {}
        }
    }
    if let Some((state, why)) = shared.blocked_by(&after) {
        shared.finish(id, state, Some(why));
        return;
    }
    let semaphore = Arc::clone(shared.semaphore(budget));
    let permit = tokio::select! {
        biased;
        () = token.cancelled() => {
            shared.finish(id, JobState::Cancelled, None);
            return;
        }
        permit = semaphore.acquire_owned() => permit,
    };
    let Ok(permit) = permit else {
        shared.finish(
            id,
            JobState::Failed,
            Some("the job manager shut down".into()),
        );
        return;
    };
    if !shared.try_start(id) {
        return;
    }
    let ctx = JobCtx {
        id,
        token: token.clone(),
        shared: Arc::clone(&shared),
    };
    let result =
        tokio::task::spawn_blocking(move || panic::catch_unwind(AssertUnwindSafe(|| body(&ctx))))
            .await;
    drop(permit);
    let (state, error) = match result {
        Ok(Ok(Ok(()))) => (JobState::Succeeded, None),
        Ok(Ok(Err(JobError::Cancelled))) => (JobState::Cancelled, None),
        Ok(Ok(Err(JobError::Failed(msg)))) if token.is_cancelled() => {
            (JobState::Cancelled, Some(msg))
        }
        Ok(Ok(Err(JobError::Failed(msg)))) => (JobState::Failed, Some(msg)),
        Ok(Err(payload)) => {
            let msg = panic_message(payload.as_ref());
            tracing::error!(job = %id, panic = %msg, "job panicked");
            (JobState::Failed, Some(format!("the job panicked: {msg}")))
        }
        Err(e) => (
            JobState::Failed,
            Some(format!("the job did not run to completion: {e}")),
        ),
    };
    shared.finish(id, state, error);
}

/// What a running job body gets: its ID, its cancel token and a progress reporter.
pub struct JobCtx {
    id: JobId,
    token: CancelToken,
    shared: Arc<Shared>,
}

impl JobCtx {
    /// The job.
    #[must_use]
    pub fn id(&self) -> JobId {
        self.id
    }

    /// The job's cancel token.
    #[must_use]
    pub fn token(&self) -> &CancelToken {
        &self.token
    }

    /// Whether cancellation was requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }

    /// `Err(JobError::Cancelled)` once cancellation was requested (`ctx.check()?`).
    pub fn check(&self) -> Result<(), JobError> {
        self.token.check()
    }

    /// Reports progress: `fraction` in `0..=1` (clamped; non-finite values are ignored) and an
    /// optional status message. Broadcast as `job.progress` when the message changes or the
    /// fraction moves by at least [`PROGRESS_EVENT_STEP`].
    pub fn progress(&self, fraction: f64, message: Option<&str>) {
        self.shared.progress(self.id, fraction, message);
    }

    /// Sleeps for `duration` in short slices, returning `Err(JobError::Cancelled)` within a
    /// few milliseconds of a cancellation (for retries and back-off in job bodies).
    pub fn sleep(&self, duration: Duration) -> Result<(), JobError> {
        const SLICE: Duration = Duration::from_millis(5);
        let start = Instant::now();
        loop {
            self.check()?;
            let elapsed = start.elapsed();
            if elapsed >= duration {
                return Ok(());
            }
            thread::sleep(SLICE.min(duration.saturating_sub(elapsed)));
        }
    }
}

impl fmt::Debug for JobCtx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JobCtx")
            .field("id", &self.id)
            .field("token", &self.token)
            .finish_non_exhaustive()
    }
}

/// A submitted job: its ID plus shortcuts to its status, cancellation and completion.
#[derive(Clone)]
pub struct JobHandle {
    id: JobId,
    shared: Arc<Shared>,
}

impl JobHandle {
    /// The job.
    #[must_use]
    pub fn id(&self) -> JobId {
        self.id
    }

    /// Its current state (`None` once it fell out of the history).
    #[must_use]
    pub fn status(&self) -> Option<JobInfo> {
        self.shared.status(self.id)
    }

    /// Requests cancellation (see [`JobManager::cancel`]).
    #[allow(clippy::must_use_candidate)] // called for its effect; the state is a convenience
    pub fn cancel(&self) -> Option<JobInfo> {
        self.shared.cancel(self.id)
    }

    /// Blocks until it is final or `timeout` passes (see [`JobManager::wait`]).
    #[must_use]
    pub fn wait(&self, timeout: Duration) -> Option<JobInfo> {
        self.shared.wait(self.id, timeout)
    }
}

impl fmt::Debug for JobHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JobHandle")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// Owns the runtime (if the manager created it) and shuts everything down when the last
/// [`JobManager`] clone is dropped.
struct Owner {
    shared: Arc<Shared>,
    runtime: Mutex<Option<Runtime>>,
}

impl Drop for Owner {
    fn drop(&mut self) {
        self.shared.cancel_all();
        self.shared.decode.close();
        self.shared.export.close();
        let runtime = self
            .runtime
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(runtime) = runtime {
            // Never blocks, also when dropped from async code. Running bodies were cancelled
            // and finish on their own.
            runtime.shutdown_background();
        }
    }
}

/// Builds a [`JobManager`] (see [`JobManager::builder`]).
pub struct JobManagerBuilder {
    config: JobConfig,
    runtime: Option<Handle>,
    ids: Option<Box<dyn IdGen + Send>>,
    history: Option<Arc<dyn JobHistory>>,
}

impl JobManagerBuilder {
    /// Budget and buffer sizes (default [`JobConfig::default`]).
    #[must_use]
    pub fn config(mut self, config: JobConfig) -> JobManagerBuilder {
        self.config = config;
        self
    }

    /// Runs jobs on this runtime instead of a runtime of the manager's own. It must have
    /// timers enabled.
    #[must_use]
    pub fn runtime(mut self, runtime: Handle) -> JobManagerBuilder {
        self.runtime = Some(runtime);
        self
    }

    /// Mints job IDs from `ids` (default: real UUIDv7s). In deterministic-ID mode the engine
    /// passes its own seeded generator, separate from the document's (job timing must never
    /// shift document IDs).
    #[must_use]
    pub fn ids(mut self, ids: Box<dyn IdGen + Send>) -> JobManagerBuilder {
        self.ids = Some(ids);
        self
    }

    /// Keeps finished jobs in `history` (default: a [`MemoryHistory`] of
    /// `config.history_capacity`).
    #[must_use]
    pub fn history(mut self, history: Arc<dyn JobHistory>) -> JobManagerBuilder {
        self.history = Some(history);
        self
    }

    /// Starts the manager.
    ///
    /// # Errors
    /// When the manager's own runtime cannot be created.
    pub fn build(self) -> std::io::Result<JobManager> {
        let JobManagerBuilder {
            config,
            runtime,
            ids,
            history,
        } = self;
        let (handle, owned) = if let Some(handle) = runtime {
            (handle, None)
        } else {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                // Only the light coordination tasks run here; bodies use the blocking pool.
                .worker_threads(1)
                .thread_name("cutweave-jobs")
                .enable_time()
                .build()?;
            (runtime.handle().clone(), Some(runtime))
        };
        let (events, _) = broadcast::channel(config.event_capacity.max(1));
        let shared = Arc::new(Shared {
            table: Mutex::new(Table::new()),
            finished: Condvar::new(),
            finished_async: Notify::new(),
            events,
            history: history
                .unwrap_or_else(|| Arc::new(MemoryHistory::new(config.history_capacity))),
            ids: Mutex::new(ids.unwrap_or_else(|| Box::new(Uuid7Gen))),
            decode: Arc::new(Semaphore::new(config.decode_slots.max(1))),
            export: Arc::new(Semaphore::new(config.export_slots.max(1))),
            config,
        });
        Ok(JobManager {
            owner: Arc::new(Owner {
                shared: Arc::clone(&shared),
                runtime: Mutex::new(owned),
            }),
            shared,
            runtime: handle,
        })
    }
}

/// The job manager (WP-1D.07): submits jobs, reports their state and progress, cancels and
/// waits for them. Cheap to clone; the last clone dropped cancels every unfinished job and
/// stops the manager's own runtime.
#[derive(Clone)]
pub struct JobManager {
    shared: Arc<Shared>,
    owner: Arc<Owner>,
    runtime: Handle,
}

impl JobManager {
    /// A builder with the defaults: own runtime, real UUIDv7 IDs, in-memory history.
    #[must_use]
    pub fn builder() -> JobManagerBuilder {
        JobManagerBuilder {
            config: JobConfig::default(),
            runtime: None,
            ids: None,
            history: None,
        }
    }

    /// A manager with its own runtime and the given sizes.
    ///
    /// # Errors
    /// When the runtime cannot be created.
    pub fn new(config: JobConfig) -> std::io::Result<JobManager> {
        JobManager::builder().config(config).build()
    }

    /// The sizes it runs with.
    #[must_use]
    pub fn config(&self) -> &JobConfig {
        &self.shared.config
    }

    /// Queues a job. `body` runs on a blocking thread once every job in `spec.after`
    /// succeeded and a slot of `spec.kind`'s budget is free; it must poll
    /// [`JobCtx::check`] often enough to stop within 500 ms of a cancellation. A panic in
    /// `body` fails the job.
    pub fn submit<F>(&self, spec: JobSpec, body: F) -> JobHandle
    where
        F: FnOnce(&JobCtx) -> Result<(), JobError> + Send + 'static,
    {
        let id = {
            let mut ids = self
                .shared
                .ids
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            JobId::mint(&mut **ids)
        };
        let token = CancelToken::new();
        let JobSpec {
            kind,
            label,
            media_id,
            after,
        } = spec;
        let info = JobInfo {
            id,
            kind,
            label,
            media_id,
            state: JobState::Queued,
            progress: JobProgress::default(),
            cancel_requested: false,
            error: None,
        };
        {
            let mut table = self.shared.lock();
            self.shared.emit(&info);
            table.insert(
                id,
                Active {
                    info,
                    token: token.clone(),
                    sent: JobProgress::default(),
                },
            );
        }
        self.runtime.spawn(run(
            Arc::clone(&self.shared),
            id,
            kind.budget(),
            after,
            token,
            Box::new(body),
        ));
        JobHandle {
            id,
            shared: Arc::clone(&self.shared),
        }
    }

    /// A handle on job `id`, if it is known (running or in the history).
    #[must_use]
    pub fn handle(&self, id: JobId) -> Option<JobHandle> {
        self.shared.status(id).map(|_| JobHandle {
            id,
            shared: Arc::clone(&self.shared),
        })
    }

    /// The job's current state, or `None` if it is unknown (never existed, or fell out of
    /// the history).
    #[must_use]
    pub fn status(&self, id: JobId) -> Option<JobInfo> {
        self.shared.status(id)
    }

    /// Jobs passing `filter`, newest first, at most `limit`: unfinished ones and the finished
    /// ones still in the history.
    #[must_use]
    pub fn list(&self, filter: &JobFilter, limit: usize) -> Vec<JobInfo> {
        let table = self.shared.lock();
        let mut jobs: Vec<JobInfo> = table
            .values()
            .rev()
            .filter(|j| filter.matches(&j.info))
            .take(limit)
            .map(|j| j.info.clone())
            .collect();
        jobs.extend(
            self.shared
                .history
                .list(filter, limit)
                .into_iter()
                .filter(|j| !table.contains_key(&j.id)),
        );
        drop(table);
        jobs.sort_by_key(|j| std::cmp::Reverse(j.id));
        jobs.truncate(limit);
        jobs
    }

    /// Number of unfinished (queued or running) jobs.
    #[must_use]
    pub fn active(&self) -> usize {
        self.shared.lock().len()
    }

    /// Requests cancellation of job `id` and returns its state afterwards (`None` if
    /// unknown). A queued job is cancelled at once; a running one keeps `running` with
    /// `cancel_requested` until its body notices. Cancelling a finished job changes nothing.
    #[allow(clippy::must_use_candidate)] // called for its effect; the state is a convenience
    pub fn cancel(&self, id: JobId) -> Option<JobInfo> {
        self.shared.cancel(id)
    }

    /// Cancels every unfinished job.
    pub fn cancel_all(&self) {
        self.shared.cancel_all();
    }

    /// Blocks the calling thread until job `id` is final or `timeout` passes, and returns its
    /// state then (`None` if unknown). Returns at once for a finished job. Do not call it on
    /// an async executor thread; use [`wait_async`](Self::wait_async) there.
    #[must_use]
    pub fn wait(&self, id: JobId, timeout: Duration) -> Option<JobInfo> {
        self.shared.wait(id, timeout)
    }

    /// [`wait`](Self::wait) for async callers (needs a runtime with timers).
    pub async fn wait_async(&self, id: JobId, timeout: Duration) -> Option<JobInfo> {
        let _ = tokio::time::timeout(timeout, self.shared.finished(id)).await;
        self.shared.status(id)
    }

    /// Subscribes to `job.progress`: every state change and progress step of every job, in
    /// order per job.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<JobEvent> {
        self.shared.events.subscribe()
    }
}

impl fmt::Debug for JobManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JobManager")
            .field("config", &self.shared.config)
            .field("active", &self.active())
            .field("owns_runtime", &{
                self.owner
                    .runtime
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .is_some()
            })
            .finish_non_exhaustive()
    }
}
