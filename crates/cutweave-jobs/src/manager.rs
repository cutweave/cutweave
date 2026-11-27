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
    hold::Hold,
    id::JobId,
    types::{Budget, JobError, JobEvent, JobFilter, JobInfo, JobProgress, JobSpec, JobState},
};

/// Default capacity of the `job.progress` broadcast; a slower subscriber gets `Lagged` and
/// re-reads `job.list`.
pub const DEFAULT_EVENT_CAPACITY: usize = 1024;
/// Default number of concurrent export jobs.
pub const DEFAULT_EXPORT_SLOTS: usize = 1;
/// Default number of concurrent proxy jobs (WP-2F.01); one fewer while an export runs.
pub const DEFAULT_PROXY_SLOTS: usize = 2;
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
    /// Concurrent [`Budget::Proxy`] jobs at the start (default 2;
    /// [`JobManager::set_proxy_slots`] changes it while the manager runs). While an export
    /// runs, only one proxy job starts at a time: the export holds the other slots (a proxy job
    /// already running is not stopped; the next one waits).
    pub proxy_slots: usize,
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
            proxy_slots: DEFAULT_PROXY_SLOTS,
            event_capacity: DEFAULT_EVENT_CAPACITY,
            history_capacity: DEFAULT_HISTORY_CAPACITY,
        }
    }
}

type Body = Box<dyn FnOnce(&JobCtx) -> Result<(), JobError> + Send + 'static>;

/// Called once with a job's final state (see [`Submit::on_done`]).
pub type OnDone = Box<dyn FnOnce(&JobInfo) + Send + 'static>;

/// Options of [`JobManager::submit_with`] beyond the [`JobSpec`].
#[derive(Default)]
pub struct Submit {
    /// The job's ID instead of a fresh one (a job queued again after a restart or a retry).
    /// It must not belong to a queued or running job.
    pub id: Option<JobId>,
    /// The job waits for this to be released before it takes a budget slot (after its
    /// dependencies). It can be cancelled while it waits.
    pub hold: Option<Hold>,
    /// Called once with the job's final state, as soon as it is final: outside the manager's
    /// lock, before [`JobManager::wait`] returns for it. It must not block for long.
    pub on_done: Option<OnDone>,
}

impl fmt::Debug for Submit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Submit")
            .field("id", &self.id)
            .field("hold", &self.hold)
            .field("on_done", &self.on_done.is_some())
            .finish()
    }
}

/// Why [`JobManager::submit_with`] refused a job.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SubmitError {
    /// The requested ID belongs to a job that is queued or running.
    #[error("job `{0}` is already queued or running")]
    InUse(JobId),
}

struct Active {
    info: JobInfo,
    token: CancelToken,
    /// Progress last broadcast, to rate-limit `job.progress`.
    sent: JobProgress,
    /// Called once the job is final.
    on_done: Option<OnDone>,
}

/// A job that just became final: its state and its completion callback.
type Finished = (JobInfo, Option<OnDone>);

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
    proxy: Arc<Semaphore>,
    /// How many proxy jobs may run at once now, and the slots still to take back.
    proxy_slots: Mutex<ProxySlots>,
    config: JobConfig,
}

/// The size of the proxy budget ([`JobManager::set_proxy_slots`]).
struct ProxySlots {
    /// Proxy jobs that may run at once.
    target: usize,
    /// Slots a lower target still has to take back: they were in use when it was set, and are
    /// taken as their jobs end.
    debt: usize,
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
            Budget::Proxy => &self.proxy,
        }
    }

    fn proxy_slots(&self) -> MutexGuard<'_, ProxySlots> {
        self.proxy_slots
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Sets how many proxy jobs may run at once (at least one). More: the waiting jobs start
    /// at once. Fewer: running jobs finish undisturbed and their slots are not handed on.
    fn set_proxy_slots(&self, slots: usize) {
        let slots = slots.max(1);
        let mut state = self.proxy_slots();
        if slots > state.target {
            let grow = slots - state.target;
            let cancelled = grow.min(state.debt);
            state.debt -= cancelled;
            self.proxy.add_permits(grow - cancelled);
        } else {
            let shrink = state.target - slots;
            state.debt += shrink - self.proxy.forget_permits(shrink);
        }
        state.target = slots;
    }

    /// Gives back proxy slots whose job ended, except those a lower target is still owed:
    /// they are taken out of the budget instead (never handed to a waiting job).
    fn release_proxy_slots(&self, mut permits: tokio::sync::OwnedSemaphorePermit) {
        let mut state = self.proxy_slots();
        let owed = state.debt.min(permits.num_permits());
        if owed > 0
            && let Some(taken) = permits.split(owed)
        {
            taken.forget();
            state.debt -= owed;
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
    /// [`done`](Self::done) after unlocking.
    fn finish_locked(
        &self,
        table: &mut Table,
        id: JobId,
        state: JobState,
        error: Option<String>,
    ) -> Option<Finished> {
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
        Some((job.info, job.on_done))
    }

    /// Runs the completion callback of a job that just became final (outside the lock), then
    /// wakes the waiters.
    fn done(&self, finished: Option<Finished>) -> Option<JobInfo> {
        let (info, on_done) = finished?;
        if let Some(on_done) = on_done {
            on_done(&info);
        }
        self.finished.notify_all();
        self.finished_async.notify_waiters();
        Some(info)
    }

    fn finish(&self, id: JobId, state: JobState, error: Option<String>) {
        let finished = {
            let mut table = self.lock();
            self.finish_locked(&mut table, id, state, error)
        };
        self.done(finished);
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

    fn progress(&self, id: JobId, mut progress: JobProgress) {
        if !progress.fraction.is_finite() {
            return;
        }
        progress.fraction = progress.fraction.clamp(0.0, 1.0);
        progress.fps = progress.fps.filter(|f| f.is_finite() && *f >= 0.0);
        let mut table = self.lock();
        let Some(job) = table.get_mut(&id) else {
            return;
        };
        if job.info.state != JobState::Running {
            return;
        }
        let fraction = progress.fraction;
        let changed_message = progress.message != job.sent.message;
        let moved = (fraction - job.sent.fraction).abs() >= PROGRESS_EVENT_STEP
            || (fraction >= 1.0 && job.sent.fraction < 1.0);
        job.info.progress = progress;
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
        let finished = self.finish_locked(&mut table, id, JobState::Cancelled, None);
        drop(table);
        self.done(finished)
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
        self.cancel_matching(&|_| true);
    }

    fn cancel_matching(&self, pred: &dyn Fn(&JobInfo) -> bool) {
        let ids: Vec<JobId> = self
            .lock()
            .values()
            .filter(|j| pred(&j.info))
            .map(|j| j.info.id)
            .collect();
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
    hold: Option<Hold>,
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
    if let Some(hold) = hold {
        tokio::select! {
            biased;
            () = token.cancelled() => {
                shared.finish(id, JobState::Cancelled, None);
                return;
            }
            () = hold.released() => {}
        }
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
    // An export holds all proxy slots but one while it runs (WP-2F.01).
    let reserved = (budget == Budget::Export).then(|| reserve_proxy_slots(&shared));
    let ctx = JobCtx {
        id,
        token: token.clone(),
        shared: Arc::clone(&shared),
    };
    let result =
        tokio::task::spawn_blocking(move || panic::catch_unwind(AssertUnwindSafe(|| body(&ctx))))
            .await;
    if budget == Budget::Proxy {
        shared.release_proxy_slots(permit);
    } else {
        drop(permit);
    }
    drop(reserved);
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

/// Holds the proxy slots an export takes while it runs: all but one of those there are when it
/// starts, so at most one proxy job runs next to an export (a budget raised while the export
/// runs adds its new slots at once). The semaphore is fair, so once the reservation waits,
/// proxy jobs queued after it wait too; a proxy job already running finishes undisturbed and its
/// slot goes to the reservation. Dropping the guard (the export ended) gives the slots back, or
/// stops waiting for them.
struct ProxyReservation(Option<tokio::sync::oneshot::Sender<()>>);

impl Drop for ProxyReservation {
    fn drop(&mut self) {
        // The receiver sees the sender go and releases the permits (or stops waiting).
        drop(self.0.take());
    }
}

fn reserve_proxy_slots(shared: &Arc<Shared>) -> ProxyReservation {
    let wanted = shared.proxy_slots().target.max(1) - 1;
    let Ok(wanted) = u32::try_from(wanted) else {
        return ProxyReservation(None);
    };
    if wanted == 0 {
        return ProxyReservation(None);
    }
    let (done, mut ended) = tokio::sync::oneshot::channel::<()>();
    let semaphore = Arc::clone(&shared.proxy);
    let shared = Arc::clone(shared);
    tokio::spawn(async move {
        tokio::select! {
            biased;
            _ = &mut ended => {}
            permits = semaphore.acquire_many_owned(wanted) => {
                if let Ok(permits) = permits {
                    let _ = ended.await;
                    shared.release_proxy_slots(permits);
                }
            }
        }
    });
    ProxyReservation(Some(done))
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
        self.shared.progress(
            self.id,
            JobProgress::new(fraction, message.map(str::to_owned)),
        );
    }

    /// Reports progress with counters (frames, speed, time left, bytes), broadcast like
    /// [`progress`](Self::progress).
    pub fn report(&self, progress: JobProgress) {
        self.shared.progress(self.id, progress);
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
        self.shared.proxy.close();
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
            proxy: Arc::new(Semaphore::new(config.proxy_slots.max(1))),
            proxy_slots: Mutex::new(ProxySlots {
                target: config.proxy_slots.max(1),
                debt: 0,
            }),
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

    /// The sizes it started with.
    #[must_use]
    pub fn config(&self) -> &JobConfig {
        &self.shared.config
    }

    /// How many proxy jobs may run at once now.
    #[must_use]
    pub fn proxy_slots(&self) -> usize {
        self.shared.proxy_slots().target
    }

    /// Sets how many proxy jobs may run at once (at least one; "Proxy jobs at once" in the
    /// app's settings). With more, waiting jobs start at once; with fewer, running jobs finish
    /// undisturbed and the next ones wait.
    pub fn set_proxy_slots(&self, slots: usize) {
        self.shared.set_proxy_slots(slots);
    }

    /// Queues a job. `body` runs on a blocking thread once every job in `spec.after`
    /// succeeded and a slot of `spec.kind`'s budget is free; it must poll
    /// [`JobCtx::check`] often enough to stop within 500 ms of a cancellation. A panic in
    /// `body` fails the job.
    pub fn submit<F>(&self, spec: JobSpec, body: F) -> JobHandle
    where
        F: FnOnce(&JobCtx) -> Result<(), JobError> + Send + 'static,
    {
        match self.submit_with(spec, Submit::default(), body) {
            Ok(handle) => handle,
            // A fresh ID is never in use.
            Err(SubmitError::InUse(id)) => unreachable!("fresh job id {id} in use"),
        }
    }

    /// [`submit`](Self::submit) with [`Submit`] options: a given ID, a [`Hold`] and a
    /// completion callback. A given ID may be one of a finished job (it is queued again and
    /// its history entry is superseded when it finishes).
    ///
    /// # Errors
    /// [`SubmitError::InUse`] when the given ID belongs to a queued or running job.
    pub fn submit_with<F>(
        &self,
        spec: JobSpec,
        options: Submit,
        body: F,
    ) -> Result<JobHandle, SubmitError>
    where
        F: FnOnce(&JobCtx) -> Result<(), JobError> + Send + 'static,
    {
        let Submit { id, hold, on_done } = options;
        let token = CancelToken::new();
        let JobSpec {
            kind,
            label,
            media_id,
            after,
        } = spec;
        let id = {
            // Lock order: the ID generator, then the table.
            let mut ids = self
                .shared
                .ids
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let mut table = self.shared.lock();
            let id = match id {
                Some(id) if table.contains_key(&id) => return Err(SubmitError::InUse(id)),
                Some(id) => id,
                // Skip IDs still known: restored jobs keep theirs, and a seeded generator
                // starts over in a new engine.
                None => loop {
                    let id = JobId::mint(&mut **ids);
                    if !table.contains_key(&id) && self.shared.history.get(id).is_none() {
                        break id;
                    }
                },
            };
            drop(ids);
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
            self.shared.emit(&info);
            table.insert(
                id,
                Active {
                    info,
                    token: token.clone(),
                    sent: JobProgress::default(),
                    on_done,
                },
            );
            id
        };
        self.runtime.spawn(run(
            Arc::clone(&self.shared),
            id,
            kind.budget(),
            after,
            hold,
            token,
            Box::new(body),
        ));
        Ok(JobHandle {
            id,
            shared: Arc::clone(&self.shared),
        })
    }

    /// A fresh job ID that no known job has, to [`submit_with`](Self::submit_with) later (a
    /// queue that records the job before it submits it).
    #[must_use]
    pub fn mint(&self) -> JobId {
        let mut ids = self
            .shared
            .ids
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let table = self.shared.lock();
        loop {
            let id = JobId::mint(&mut **ids);
            if !table.contains_key(&id) && self.shared.history.get(id).is_none() {
                return id;
            }
        }
    }

    /// Remembers a job that finished elsewhere (a queue restored after a restart) as if it had
    /// finished here: `status`, `list` and `wait` find it. A queued or running job with that
    /// ID is left alone.
    pub fn record(&self, info: &JobInfo) {
        if !info.state.is_terminal() {
            return;
        }
        let table = self.shared.lock();
        if !table.contains_key(&info.id) {
            self.shared.history.record(info);
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

    /// Cancels the unfinished jobs for which `pred` holds.
    pub fn cancel_matching(&self, pred: impl Fn(&JobInfo) -> bool) {
        self.shared.cancel_matching(&pred);
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
