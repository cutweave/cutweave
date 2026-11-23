// SPDX-License-Identifier: GPL-3.0-or-later
//! [`DocActor`]: the thread that owns the [`DocState`], and [`DocHandle`], the cloneable,
//! thread-safe way to talk to it (arch §5.2, ADR-0005).

use std::{
    panic::{self, AssertUnwindSafe},
    time::Instant,
};

use serde_json::Value;

use super::{
    bus::{Bus, EventStream},
    event::{DocEvent, DocSnapshot},
    identity::{ActorCtx, ConnId},
    locks::LockList,
    metrics::{Counters, DocMetrics, DocStats, NoMetrics, Outcome, RequestSample},
    pending::{self, Pending, Reply},
    request::{Applied, Submit, TxClosed, TxId, TxInfo, TxKind},
    state::{DocConfig, DocState, panic_message},
    sync::{self, Arc, Cell, mpsc},
};
use crate::{ctx::QueryCtx, error::CmdError, registry::Registry};

/// Default per-subscriber event queue bound: ~4 s of 1 kHz gesture updates.
pub const DEFAULT_EVENT_CAPACITY: usize = 4096;

/// How to run a [`DocActor`].
pub struct ActorOptions {
    /// Per-subscriber event queue bound (see [`EventStream`]).
    pub event_capacity: usize,
    /// Where measurements go.
    pub metrics: std::sync::Arc<dyn DocMetrics>,
    /// The thread's name (shows in debuggers and crash reports).
    pub thread_name: String,
}

impl Default for ActorOptions {
    fn default() -> ActorOptions {
        ActorOptions {
            event_capacity: DEFAULT_EVENT_CAPACITY,
            metrics: std::sync::Arc::new(NoMetrics),
            thread_name: "cutweave-doc".to_owned(),
        }
    }
}

impl std::fmt::Debug for ActorOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActorOptions")
            .field("event_capacity", &self.event_capacity)
            .field("thread_name", &self.thread_name)
            .finish_non_exhaustive()
    }
}

/// The live and committed snapshots, swapped together so readers never see a mix.
struct SnapshotPair {
    live: std::sync::Arc<DocSnapshot>,
    committed: std::sync::Arc<DocSnapshot>,
}

/// What the actor publishes and every handle and stream reads.
struct Published {
    snapshots: Cell<SnapshotPair>,
    bus: Arc<Bus>,
    counters: Counters,
}

type Deliver = Box<dyn FnOnce() + Send>;
type Job = Box<dyn FnOnce(&mut DocState) -> (Outcome, Deliver) + Send>;

enum Request {
    Submit {
        submit: Submit,
        reply: Reply<Applied>,
        enqueued: Instant,
    },
    Run {
        name: &'static str,
        job: Job,
        enqueued: Instant,
    },
    Shutdown,
}

/// The document actor: owns the [`DocState`] on its own thread. Dropping it (or
/// [`shutdown`](Self::shutdown)) processes the requests already queued, then stops; later
/// requests from surviving handles fail with `Internal` ("actor stopped") and event streams
/// report `Closed`. Snapshots stay readable.
///
/// # Threading model
///
/// The actor runs on a **dedicated OS thread**, not a tokio task:
///
/// - Commands are synchronous, CPU-bound code (and the journal's `write()` before the ack,
///   WP-1C.02, is blocking I/O). On an async worker they would stall unrelated I/O; on a
///   dedicated thread they cannot.
/// - Serial execution does not depend on a runtime's scheduling, and the CLI, oneshot mode and
///   tests drive the document without any runtime.
/// - Async callers (the RPC layer) still `.await` a [`Pending`] reply and receive events with
///   [`EventStream::recv`]: both are runtime-agnostic futures; threads use the blocking forms.
///
/// # Per request, in order
///
/// 1. the [`DocState`] runs it (journaling a commit before it becomes visible);
/// 2. the new live and committed snapshots are published with one atomic swap;
/// 3. its events are queued for every subscriber (never blocking on a slow one);
/// 4. the reply is sent.
///
/// So a caller that got `Applied{rev}` back always reads a snapshot at `rev` or later, and a
/// subscriber that received an event for `rev` can read a snapshot that contains it. Readers
/// never take a lock: [`DocHandle::snapshot`] is a lock-free load.
pub struct DocActor {
    handle: DocHandle,
    thread: Option<sync::JoinHandle>,
}

impl std::fmt::Debug for DocActor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocActor")
            .field("running", &self.thread.is_some())
            .finish_non_exhaustive()
    }
}

impl DocActor {
    /// Starts an actor with default options.
    pub fn spawn(config: DocConfig) -> std::io::Result<DocActor> {
        DocActor::spawn_with(config, ActorOptions::default())
    }

    /// Starts an actor.
    pub fn spawn_with(config: DocConfig, options: ActorOptions) -> std::io::Result<DocActor> {
        let state = DocState::new(config);
        let live = std::sync::Arc::new(state.snapshot());
        let committed = if state.rev() == state.committed_rev() {
            std::sync::Arc::clone(&live)
        } else {
            std::sync::Arc::new(state.committed_snapshot())
        };
        let published = Arc::new(Published {
            snapshots: Cell::new(std::sync::Arc::new(SnapshotPair { live, committed })),
            bus: Arc::new(Bus::new(options.event_capacity)),
            counters: Counters::new(),
        });
        let registry = std::sync::Arc::clone(state.registry());
        let (inbox, requests) = mpsc::channel();
        let thread = {
            let published = Arc::clone(&published);
            let metrics = std::sync::Arc::clone(&options.metrics);
            sync::spawn(options.thread_name, move || {
                run(state, &requests, &published, &*metrics);
            })?
        };
        Ok(DocActor {
            handle: DocHandle {
                inbox,
                published,
                registry,
            },
            thread: Some(thread),
        })
    }

    /// A handle for submitting requests and reading snapshots.
    #[must_use]
    pub fn handle(&self) -> DocHandle {
        self.handle.clone()
    }

    /// Processes the requests already queued, then stops the thread and waits for it.
    pub fn shutdown(mut self) -> Result<(), CmdError> {
        self.stop()
    }

    fn stop(&mut self) -> Result<(), CmdError> {
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        // Fails only if the thread is already gone.
        let _ = self.handle.inbox.send(Request::Shutdown);
        thread
            .join()
            .map_err(|_| CmdError::internal("the document actor thread panicked"))
    }
}

impl Drop for DocActor {
    fn drop(&mut self) {
        if let Err(e) = self.stop() {
            tracing::error!(error = %e, "document actor did not stop cleanly");
        }
    }
}

/// Closes the event streams when the actor thread ends, also by unwinding.
struct CloseOnExit<'a>(&'a Published);

impl Drop for CloseOnExit<'_> {
    fn drop(&mut self) {
        self.0.bus.close();
    }
}

/// Shortest wait before re-checking an idle gesture (the clock may lag the deadline).
const MIN_EXPIRY_WAIT: std::time::Duration = std::time::Duration::from_millis(10);
/// Longest wait between idle-gesture checks (bounds a wall clock stepping backwards).
const MAX_EXPIRY_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

/// The next request; while a gesture is open, wakes up to cancel it once idle for
/// [`GESTURE_IDLE_TIMEOUT`](crate::GESTURE_IDLE_TIMEOUT) even if no request arrives. `None`
/// once every handle is gone.
fn next_request(
    state: &mut DocState,
    requests: &mpsc::Receiver<Request>,
    published: &Published,
    metrics: &dyn DocMetrics,
) -> Option<Request> {
    loop {
        let Some(deadline) = state.next_expiry() else {
            return requests.recv().ok();
        };
        let wait = deadline
            .saturating_since(state.now())
            .clamp(MIN_EXPIRY_WAIT, MAX_EXPIRY_WAIT);
        match requests.recv_timeout(wait) {
            Ok(request) => return Some(request),
            Err(mpsc::RecvTimeoutError::Disconnected) => return None,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if !state.expire_idle().is_empty() {
                    publish(state, published, metrics);
                }
            }
        }
    }
}

fn run(
    mut state: DocState,
    requests: &mpsc::Receiver<Request>,
    published: &Published,
    metrics: &dyn DocMetrics,
) {
    let _close = CloseOnExit(published);
    tracing::debug!(rev = %state.rev(), "document actor started");
    while let Some(request) = next_request(&mut state, requests, published, metrics) {
        published.counters.dequeued();
        match request {
            Request::Shutdown => break,
            Request::Submit {
                submit,
                reply,
                enqueued,
            } => {
                let started = Instant::now();
                let name = submit.name.clone();
                let result = state.submit(submit);
                publish(&mut state, published, metrics);
                let exec = started.elapsed();
                let outcome = Outcome::of_applied(&result);
                published.counters.record(outcome);
                reply.send(result);
                metrics.request(&RequestSample {
                    name: &name,
                    outcome,
                    queue_wait: started.saturating_duration_since(enqueued),
                    exec,
                    rev: state.rev(),
                });
            }
            Request::Run {
                name,
                job,
                enqueued,
            } => {
                let started = Instant::now();
                let (outcome, deliver) = job(&mut state);
                publish(&mut state, published, metrics);
                let exec = started.elapsed();
                published.counters.record(outcome);
                deliver();
                metrics.request(&RequestSample {
                    name,
                    outcome,
                    queue_wait: started.saturating_duration_since(enqueued),
                    exec,
                    rev: state.rev(),
                });
            }
        }
    }
    tracing::debug!(rev = %state.rev(), "document actor stopped");
}

/// Publishes what the last request changed: snapshots first, then events.
fn publish(state: &mut DocState, published: &Published, metrics: &dyn DocMetrics) {
    let changes = state.take_changes();
    if changes.any() {
        let live = std::sync::Arc::new(state.snapshot());
        let committed = if state.rev() == state.committed_rev() {
            // Same document (the state's invariant): share it.
            std::sync::Arc::clone(&live)
        } else if changes.committed {
            std::sync::Arc::new(state.committed_snapshot())
        } else {
            published
                .snapshots
                .read(|pair| std::sync::Arc::clone(&pair.committed))
        };
        published
            .snapshots
            .store(std::sync::Arc::new(SnapshotPair { live, committed }));
    }
    let events = state.take_events();
    if events.is_empty() {
        return;
    }
    let events: Vec<std::sync::Arc<DocEvent>> =
        events.into_iter().map(std::sync::Arc::new).collect();
    for event in &events {
        if let DocEvent::CommandPanicked(p) = &**event {
            published.counters.panicked();
            metrics.command_panicked(&p.command);
        }
    }
    let lagged = published.bus.publish(&events);
    if lagged > 0 {
        published.counters.lagged(lagged);
        metrics.subscribers_lagged(lagged);
    }
}

/// A cloneable, thread-safe handle to a [`DocActor`].
///
/// Mutations go through the actor's inbox and are answered with a [`Pending`]; reads
/// ([`snapshot`](Self::snapshot), [`query`](Self::query)) happen on the calling thread
/// against an immutable snapshot, without locking.
#[derive(Clone)]
pub struct DocHandle {
    inbox: mpsc::Sender<Request>,
    published: Arc<Published>,
    registry: std::sync::Arc<Registry>,
}

impl std::fmt::Debug for DocHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocHandle")
            .field("rev", &self.snapshot().rev)
            .finish_non_exhaustive()
    }
}

impl DocHandle {
    fn send<T>(&self, request: Request, pending: Pending<T>) -> Pending<T> {
        self.published.counters.enqueued();
        if self.inbox.send(request).is_err() {
            // The request (and its reply) were dropped: `pending` reports "actor stopped".
            self.published.counters.dequeued();
        }
        pending
    }

    /// Submits a request (see [`DocState::submit`]); the reply comes once its effects are
    /// published.
    pub fn submit(&self, submit: Submit) -> Pending<Applied> {
        let (reply, pending) = pending::channel();
        self.send(
            Request::Submit {
                submit,
                reply,
                enqueued: Instant::now(),
            },
            pending,
        )
    }

    fn job<T: Send + 'static>(
        &self,
        name: &'static str,
        f: impl FnOnce(&mut DocState) -> Result<T, CmdError> + Send + 'static,
    ) -> Pending<T> {
        let (reply, pending) = pending::channel();
        let job: Job = Box::new(move |state| {
            let result = panic::catch_unwind(AssertUnwindSafe(|| f(state))).unwrap_or_else(
                |payload| {
                    let message = panic_message(payload.as_ref());
                    tracing::error!(task = name, %message, "an engine task panicked on the document actor");
                    Err(CmdError::internal(format!(
                        "engine task `{name}` panicked: {message}"
                    )))
                },
            );
            let outcome = Outcome::of(&result);
            let deliver: Deliver = Box::new(move || reply.send(result));
            (outcome, deliver)
        });
        self.send(
            Request::Run {
                name,
                job,
                enqueued: Instant::now(),
            },
            pending,
        )
    }

    /// Runs `f` on the actor thread, serialized with every request, and publishes whatever it
    /// changed. For engine plumbing (lifecycle, idle-timeout sweeps, journal rotation); edits
    /// of the document go through registered commands.
    pub fn run<R, F>(&self, f: F) -> Pending<R>
    where
        R: Send + 'static,
        F: FnOnce(&mut DocState) -> R + Send + 'static,
    {
        self.job("run", move |state| Ok(f(state)))
    }

    /// Opens a transaction owned by `ctx`'s connection ([`DocState::begin_tx`]).
    pub fn begin_tx(
        &self,
        ctx: &ActorCtx,
        kind: TxKind,
        label: impl Into<String>,
    ) -> Pending<TxInfo> {
        let (ctx, label) = (ctx.clone(), label.into());
        self.job("begin_tx", move |state| state.begin_tx(&ctx, kind, label))
    }

    /// Commits `ctx`'s open transaction ([`DocState::commit_tx`]).
    pub fn commit_tx(&self, ctx: &ActorCtx, tx: TxId) -> Pending<TxClosed> {
        let ctx = ctx.clone();
        self.job("commit_tx", move |state| state.commit_tx(&ctx, tx))
    }

    /// Cancels `ctx`'s open transaction ([`DocState::cancel_tx`]).
    pub fn cancel_tx(&self, ctx: &ActorCtx, tx: TxId) -> Pending<TxClosed> {
        let ctx = ctx.clone();
        self.job("cancel_tx", move |state| state.cancel_tx(&ctx, tx))
    }

    /// The connection `conn` closed: rolls back its open transaction and releases its soft
    /// locks ([`DocState::close_connection`]). Send it after the last request read from that
    /// connection (the inbox is FIFO, so those run first).
    pub fn close_connection(&self, conn: ConnId) -> Pending<Vec<TxClosed>> {
        self.job("close_connection", move |state| {
            Ok(state.close_connection(conn))
        })
    }

    /// The open transactions and the items each soft-locks, as `edit.locks` reports them to
    /// `ctx` ([`DocState::locks`]).
    pub fn locks(&self, ctx: &ActorCtx) -> Pending<LockList> {
        let conn = ctx.conn();
        self.job("locks", move |state| Ok(state.locks(Some(conn))))
    }

    /// Cancels the gestures idle for [`GESTURE_IDLE_TIMEOUT`](crate::GESTURE_IDLE_TIMEOUT) now
    /// ([`DocState::expire_idle`]). The actor also does this on its own timer and before every
    /// request; this is for injected clocks (tests) and hosts that want it immediately.
    pub fn expire_idle(&self) -> Pending<Vec<TxClosed>> {
        self.job("expire_idle", |state| Ok(state.expire_idle()))
    }

    /// The live document (lock-free; never blocks, whatever the actor is doing).
    #[must_use]
    pub fn snapshot(&self) -> std::sync::Arc<DocSnapshot> {
        self.published
            .snapshots
            .read(|pair| std::sync::Arc::clone(&pair.live))
    }

    /// The last committed document: what saves, autosave and the journal use (excludes open
    /// gestures and proposals).
    #[must_use]
    pub fn committed_snapshot(&self) -> std::sync::Arc<DocSnapshot> {
        self.published
            .snapshots
            .read(|pair| std::sync::Arc::clone(&pair.committed))
    }

    /// A new event subscription, starting with the next published event (call
    /// [`EventStream::resync`] first to mirror the document).
    #[must_use]
    pub fn subscribe(&self) -> EventStream {
        let published = Arc::clone(&self.published);
        EventStream::new(
            Arc::clone(&self.published.bus),
            std::sync::Arc::new(move || {
                published
                    .snapshots
                    .read(|pair| std::sync::Arc::clone(&pair.live))
            }),
        )
    }

    /// Runs a query against the live snapshot, on the calling thread.
    pub fn query(&self, ctx: &ActorCtx, name: &str, params: Value) -> Result<Value, CmdError> {
        let snapshot = self.snapshot();
        let qctx = QueryCtx::new(&snapshot.doc, ctx.access()).with_rev(snapshot.rev);
        self.registry.query_json(name, params, &qctx)
    }

    /// Checks a call against the live snapshot without running it (dry run).
    pub fn validate(&self, ctx: &ActorCtx, name: &str, params: Value) -> Result<(), CmdError> {
        let snapshot = self.snapshot();
        let qctx = QueryCtx::new(&snapshot.doc, ctx.access()).with_rev(snapshot.rev);
        self.registry.validate_json(name, params, &qctx)
    }

    /// The registry.
    #[must_use]
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// The actor's counters.
    #[must_use]
    pub fn stats(&self) -> DocStats {
        self.published.counters.snapshot()
    }
}
