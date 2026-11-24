// SPDX-License-Identifier: GPL-3.0-or-later
//! [`DecoderPool`]: the video decoders of one media file (arch §7, ADR-0003, WP-1D.03).
//!
//! Opening a decoder costs a container open (and, without an index, a scan of the whole file),
//! and a long-GOP seek costs the decode-forward from its keyframe, so decoders are kept and
//! reused:
//!
//! - **Bounded.** At most [`max_live`](DecoderPool::max_live) decoders exist at once
//!   ([`DEFAULT_MAX_LIVE`](DecoderPool::DEFAULT_MAX_LIVE) = 4), checked out or idle. A checkout
//!   beyond that waits until one is released.
//! - **Reuse and LRU eviction.** A checkout takes an idle decoder opened with the same
//!   [`DecodeOpts`] (layout, mode, threads; [`DecodeOpts::same_decoder`]): the one positioned
//!   just before `near`, so a forward scrub keeps decoding instead of seeking, else the most
//!   recently used. When the pool is full and only decoders with other options are idle, the
//!   least recently used of them is closed to make room.
//! - **One index scan.** The first decoder builds (or is given) the [`FrameIndex`]; every later
//!   one is opened with it.
//! - **Cancellation at once.** A checkout carries a [`CancelToken`]. Each pooled decoder runs
//!   on its own thread, and a lease's call waits for that thread's answer *or* its token;
//!   [`CancelToken::cancel`] wakes the wait directly. A cancelled request (a checkout waiting
//!   for a free decoder, a pending seek) therefore returns as soon as its thread is scheduled,
//!   even while the decoder is inside a long FFmpeg call (a frame-threaded flush waits for every
//!   frame in flight: tens of milliseconds at 4K); the decoder thread drops the work at its next
//!   packet. The WBS budget is 10 ms. Linked tokens ([`CancelToken::linked`]) are polled every
//!   millisecond instead.
//!
//! A [`DecoderLease`] is a [`VideoDecoder`]; releasing it ([`DecoderPool::release`] or dropping
//! it) returns the decoder to the pool. A decoder that failed (anything but a cancel or an
//! out-of-range request) is closed instead of pooled.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration as Wall;

use cutweave_model::Time;

use crate::backend::{DecodeOpts, MediaBackend, VideoDecoder};
use crate::cancel::Waker;
use crate::{CancelToken, FrameIndex, MediaError, VideoFrame};

/// How long a blocked wait sleeps between looks at its token. A native cancel wakes it at
/// once; this only bounds how often linked checks are polled (the OS timer may stretch it).
const WAIT_SLICE: Wall = Wall::from_millis(1);

// ---------------------------------------------------------------------------------------------
// A decoder on its own thread
// ---------------------------------------------------------------------------------------------

enum Call {
    Next,
    Seek(Time),
}

struct Job {
    call: Call,
    cancel: CancelToken,
    reply: ReplySender,
}

struct Reply {
    result: Result<Option<VideoFrame>, MediaError>,
    is_hw: bool,
}

/// Where the decoder thread leaves its answer; the caller waits on it.
#[derive(Default)]
struct Slot {
    reply: Mutex<Option<Reply>>,
    filled: Condvar,
}

impl Slot {
    fn put(&self, reply: Reply) {
        *self.reply.lock().unwrap_or_else(PoisonError::into_inner) = Some(reply);
        self.filled.notify_all();
    }
}

/// Answers a call exactly once: dropped unanswered (the decoder panicked, the thread is gone),
/// it answers with an error so the caller never waits forever.
struct ReplySender {
    slot: Arc<Slot>,
    sent: bool,
}

impl ReplySender {
    fn send(mut self, reply: Reply) {
        self.sent = true;
        self.slot.put(reply);
    }
}

impl Drop for ReplySender {
    fn drop(&mut self) {
        if !self.sent {
            self.slot.put(Reply {
                result: Err(MediaError::Backend {
                    message: "the decoder thread stopped".to_owned(),
                }),
                is_hw: false,
            });
        }
    }
}

/// A decoder running calls on its own thread. Dropping it ends the thread after the call in
/// progress (which its token cancels); the decoder is closed there.
struct Worker {
    jobs: mpsc::Sender<Job>,
    index: Arc<FrameIndex>,
    is_hw: bool,
}

impl Worker {
    fn spawn(mut decoder: Box<dyn VideoDecoder>) -> Result<Worker, MediaError> {
        let index = decoder.index();
        let is_hw = decoder.is_hw();
        let (jobs, inbox) = mpsc::channel::<Job>();
        thread::Builder::new()
            .name("cutweave-decoder".to_owned())
            .spawn(move || {
                while let Ok(job) = inbox.recv() {
                    let result = if job.cancel.is_cancelled() {
                        Err(MediaError::Cancelled)
                    } else {
                        decoder.set_cancel(Some(job.cancel));
                        let r = match job.call {
                            Call::Next => decoder.next_frame(),
                            Call::Seek(t) => decoder.seek_exact(t).map(Some),
                        };
                        decoder.set_cancel(None);
                        r
                    };
                    // The caller may have stopped waiting (cancelled); that is fine.
                    job.reply.send(Reply {
                        result,
                        is_hw: decoder.is_hw(),
                    });
                }
            })
            .map_err(|e| MediaError::Backend {
                message: format!("cannot start a decoder thread: {e}"),
            })?;
        Ok(Worker { jobs, index, is_hw })
    }

    /// Runs `call` on the decoder thread and waits for the answer or for `cancel`.
    fn call(&mut self, call: Call, cancel: &CancelToken) -> Result<Option<VideoFrame>, MediaError> {
        cancel.check()?;
        let slot = Arc::new(Slot::default());
        // Registered before the job is sent and the token checked again: a cancel in between
        // wakes the wait below instead of being missed.
        let wake: Waker = {
            let slot = Arc::clone(&slot);
            Arc::new(move || {
                let _held = slot.reply.lock().unwrap_or_else(PoisonError::into_inner);
                slot.filled.notify_all();
            })
        };
        let _registered = cancel.on_cancel(wake);
        let job = Job {
            call,
            cancel: cancel.clone(),
            reply: ReplySender {
                slot: Arc::clone(&slot),
                sent: false,
            },
        };
        if let Err(mpsc::SendError(job)) = self.jobs.send(job) {
            drop(job); // answers "stopped"
        }
        let mut reply = slot.reply.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(r) = reply.take() {
                self.is_hw = r.is_hw;
                return r.result;
            }
            if cancel.is_cancelled() {
                // The decoder thread drops the work at its next packet.
                return Err(MediaError::Cancelled);
            }
            // The timeout only matters for linked tokens, which cannot wake the wait.
            reply = slot
                .filled
                .wait_timeout(reply, WAIT_SLICE)
                .map_or_else(|e| e.into_inner().0, |(guard, _)| guard);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The pool
// ---------------------------------------------------------------------------------------------

struct Idle {
    worker: Worker,
    opts: DecodeOpts,
    /// pts of the last frame it returned.
    position: Option<Time>,
    /// Pool clock at release (larger = more recent).
    used: u64,
}

#[derive(Default)]
struct State {
    index: Option<Arc<FrameIndex>>,
    idle: Vec<Idle>,
    busy: usize,
    opening: usize,
    clock: u64,
    opened: u64,
    evicted: u64,
}

impl State {
    fn live(&self) -> usize {
        self.idle.len() + self.busy + self.opening
    }
}

struct Shared {
    backend: Arc<dyn MediaBackend>,
    path: PathBuf,
    max_live: usize,
    state: Mutex<State>,
    changed: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The decoders of one media file (see the module docs). Cheap to clone; clones share the
/// decoders. `Send + Sync`: threads check decoders out concurrently.
#[derive(Clone)]
pub struct DecoderPool {
    shared: Arc<Shared>,
}

impl DecoderPool {
    /// Live decoders per media unless configured otherwise.
    pub const DEFAULT_MAX_LIVE: usize = 4;

    /// A pool for `path` on `backend` holding at most [`DEFAULT_MAX_LIVE`](Self::DEFAULT_MAX_LIVE)
    /// decoders. Nothing is opened until the first checkout.
    pub fn new(backend: Arc<dyn MediaBackend>, path: impl Into<PathBuf>) -> DecoderPool {
        DecoderPool::with_max_live(backend, path, DecoderPool::DEFAULT_MAX_LIVE)
    }

    /// A pool holding at most `max_live` decoders (at least 1).
    pub fn with_max_live(
        backend: Arc<dyn MediaBackend>,
        path: impl Into<PathBuf>,
        max_live: usize,
    ) -> DecoderPool {
        DecoderPool {
            shared: Arc::new(Shared {
                backend,
                path: path.into(),
                max_live: max_live.max(1),
                state: Mutex::new(State::default()),
                changed: Condvar::new(),
            }),
        }
    }

    /// The same pool with a known frame index (from `cache.sqlite`), so no decoder scans the
    /// file.
    #[must_use]
    pub fn with_index(self, index: Arc<FrameIndex>) -> DecoderPool {
        self.shared.lock().index = Some(index);
        self
    }

    /// The media file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.shared.path
    }

    /// The most decoders this pool keeps.
    #[must_use]
    pub fn max_live(&self) -> usize {
        self.shared.max_live
    }

    /// The frame index, once known.
    #[must_use]
    pub fn index(&self) -> Option<Arc<FrameIndex>> {
        self.shared.lock().index.clone()
    }

    /// Decoders that exist now: checked out, idle or being opened.
    #[must_use]
    pub fn live(&self) -> usize {
        self.shared.lock().live()
    }

    /// Idle decoders.
    #[must_use]
    pub fn idle(&self) -> usize {
        self.shared.lock().idle.len()
    }

    /// Decoders opened over the pool's life.
    #[must_use]
    pub fn opened(&self) -> u64 {
        self.shared.lock().opened
    }

    /// Idle decoders closed to make room for decoders with other options.
    #[must_use]
    pub fn evicted(&self) -> u64 {
        self.shared.lock().evicted
    }

    /// Closes every idle decoder (memory pressure, the file went away). Checked-out decoders are
    /// unaffected.
    pub fn trim(&self) {
        let idle = std::mem::take(&mut self.shared.lock().idle);
        drop(idle);
        self.shared.changed.notify_all();
    }

    /// A decoder opened with `opts` (its `index` is ignored once the pool knows one), for this
    /// caller alone until released. `near` is the media time the caller will ask for first, if
    /// known: an idle decoder that can reach it by decoding forward is preferred.
    ///
    /// Waits while [`max_live`](Self::max_live) decoders are checked out. `cancel` ends the wait
    /// with [`MediaError::Cancelled`], and stays the lease's token
    /// ([`VideoDecoder::set_cancel`] replaces it). Errors from opening a decoder are passed on
    /// ([`MediaBackend::open_decoder`]).
    pub fn checkout(
        &self,
        opts: &DecodeOpts,
        near: Option<Time>,
        cancel: &CancelToken,
    ) -> Result<DecoderLease, MediaError> {
        // A cancel wakes the wait below at once (registered before the token is checked).
        let wake: Waker = {
            let shared = Arc::clone(&self.shared);
            Arc::new(move || {
                let _held = shared.lock();
                shared.changed.notify_all();
            })
        };
        let _registered = cancel.on_cancel(wake);
        let mut st = self.shared.lock();
        if st.index.is_none() {
            st.index.clone_from(&opts.index);
        }
        loop {
            cancel.check()?;
            if let Some(i) = pick(&st.idle, opts, near) {
                let idle = st.idle.swap_remove(i);
                st.busy += 1;
                drop(st);
                return Ok(self.lease(idle.worker, idle.opts, idle.position, cancel));
            }
            // Only one decoder scans the file for the index; the others wait for it.
            let index_pending = st.index.is_none() && st.opening > 0;
            if !index_pending {
                if st.live() < self.shared.max_live {
                    return self.open(st, opts, None, cancel);
                }
                if let Some(i) = least_recently_used(&st.idle) {
                    let evicted = st.idle.swap_remove(i);
                    st.evicted += 1;
                    return self.open(st, opts, Some(evicted), cancel);
                }
            }
            st = self
                .shared
                .changed
                .wait_timeout(st, WAIT_SLICE)
                .map_or_else(|e| e.into_inner().0, |(guard, _)| guard);
        }
    }

    /// Returns a decoder to the pool (the same as dropping the lease).
    pub fn release(&self, lease: DecoderLease) {
        drop(lease);
    }

    /// Opens a decoder in a free slot (after closing `evicted`, whose slot it takes).
    fn open(
        &self,
        mut st: MutexGuard<'_, State>,
        opts: &DecodeOpts,
        evicted: Option<Idle>,
        cancel: &CancelToken,
    ) -> Result<DecoderLease, MediaError> {
        st.opening += 1;
        let mut with_index = opts.clone();
        with_index.index.clone_from(&st.index);
        if with_index.index.is_none() {
            with_index.index.clone_from(&opts.index);
        }
        drop(st);
        drop(evicted);
        let opened = self
            .shared
            .backend
            .open_decoder(&self.shared.path, &with_index)
            .and_then(Worker::spawn);
        let mut st = self.shared.lock();
        st.opening -= 1;
        let mut key = opts.clone();
        key.index = None;
        let result = match opened {
            Ok(worker) => {
                if st.index.is_none() {
                    st.index = Some(Arc::clone(&worker.index));
                }
                st.opened += 1;
                if cancel.is_cancelled() {
                    // Keep the work: the next checkout gets it.
                    st.clock += 1;
                    let used = st.clock;
                    st.idle.push(Idle {
                        worker,
                        opts: key,
                        position: None,
                        used,
                    });
                    Err(MediaError::Cancelled)
                } else {
                    st.busy += 1;
                    Ok(worker)
                }
            }
            Err(e) => Err(e),
        };
        drop(st);
        self.shared.changed.notify_all();
        result.map(|worker| self.lease(worker, opts.clone(), None, cancel))
    }

    fn lease(
        &self,
        worker: Worker,
        mut opts: DecodeOpts,
        position: Option<Time>,
        cancel: &CancelToken,
    ) -> DecoderLease {
        opts.index = None;
        DecoderLease {
            index: Arc::clone(&worker.index),
            worker: Some(worker),
            cancel: cancel.clone(),
            opts,
            position,
            broken: false,
            shared: Arc::clone(&self.shared),
        }
    }
}

impl fmt::Debug for DecoderPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let st = self.shared.lock();
        f.debug_struct("DecoderPool")
            .field("path", &self.shared.path)
            .field("max_live", &self.shared.max_live)
            .field("idle", &st.idle.len())
            .field("busy", &st.busy)
            .field("opening", &st.opening)
            .field("indexed", &st.index.is_some())
            .finish()
    }
}

/// The idle decoder to hand out for `opts` and `near`: among those opened with the same
/// options, the one whose last frame is closest before `near`, else the most recently used.
fn pick(idle: &[Idle], opts: &DecodeOpts, near: Option<Time>) -> Option<usize> {
    let candidates = || {
        idle.iter()
            .enumerate()
            .filter(|(_, d)| d.opts.same_decoder(opts))
    };
    let before = near.and_then(|t| {
        candidates()
            .filter(|(_, d)| d.position.is_some_and(|p| p <= t))
            .max_by_key(|(_, d)| d.position)
            .map(|(i, _)| i)
    });
    before.or_else(|| candidates().max_by_key(|(_, d)| d.used).map(|(i, _)| i))
}

fn least_recently_used(idle: &[Idle]) -> Option<usize> {
    idle.iter()
        .enumerate()
        .min_by_key(|(_, d)| d.used)
        .map(|(i, _)| i)
}

/// A decoder checked out of a [`DecoderPool`]; a [`VideoDecoder`] itself, whose calls run on
/// the decoder's thread and return [`MediaError::Cancelled`] as soon as the token is cancelled.
/// Dropping it (or [`DecoderPool::release`]) returns the decoder to the pool.
pub struct DecoderLease {
    worker: Option<Worker>,
    index: Arc<FrameIndex>,
    cancel: CancelToken,
    opts: DecodeOpts,
    position: Option<Time>,
    broken: bool,
    shared: Arc<Shared>,
}

impl DecoderLease {
    /// Closes the decoder instead of returning it to the pool (its state is suspect).
    pub fn discard(mut self) {
        self.broken = true;
    }

    /// The pts of the last frame this decoder returned (in this lease or an earlier one).
    #[must_use]
    pub fn position(&self) -> Option<Time> {
        self.position
    }

    /// The options the decoder was opened with (without the index).
    #[must_use]
    pub fn opts(&self) -> &DecodeOpts {
        &self.opts
    }

    /// Runs `call`, recording the position of a returned frame or that the decoder failed.
    fn run(&mut self, call: Call) -> Result<Option<VideoFrame>, MediaError> {
        let Some(worker) = self.worker.as_mut() else {
            return Err(MediaError::Backend {
                message: "the decoder lease was already released".to_owned(),
            });
        };
        let result = worker.call(call, &self.cancel);
        match &result {
            Ok(frame) => {
                if let Some(f) = frame {
                    self.position = Some(f.pts);
                }
            }
            Err(MediaError::Cancelled | MediaError::OutOfRange { .. }) => {}
            Err(_) => self.broken = true,
        }
        result
    }
}

impl VideoDecoder for DecoderLease {
    fn next_frame(&mut self) -> Result<Option<VideoFrame>, MediaError> {
        self.run(Call::Next)
    }

    fn seek_exact(&mut self, t: Time) -> Result<VideoFrame, MediaError> {
        self.run(Call::Seek(t))?.ok_or_else(|| MediaError::Backend {
            message: "the decoder answered a seek without a frame".to_owned(),
        })
    }

    fn index(&self) -> Arc<FrameIndex> {
        Arc::clone(&self.index)
    }

    /// Replaces the lease's token (`None`: a token that is never cancelled).
    fn set_cancel(&mut self, cancel: Option<CancelToken>) {
        self.cancel = cancel.unwrap_or_default();
    }

    fn is_hw(&self) -> bool {
        self.worker.as_ref().is_some_and(|w| w.is_hw)
    }
}

impl Drop for DecoderLease {
    fn drop(&mut self) {
        let Some(worker) = self.worker.take() else {
            return;
        };
        let mut st = self.shared.lock();
        st.busy -= 1;
        if self.broken {
            drop(st);
            drop(worker);
        } else {
            st.clock += 1;
            let used = st.clock;
            st.idle.push(Idle {
                worker,
                opts: std::mem::take(&mut self.opts),
                position: self.position,
                used,
            });
            drop(st);
        }
        self.shared.changed.notify_all();
    }
}

impl fmt::Debug for DecoderLease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DecoderLease")
            .field("path", &self.shared.path)
            .field("opts", &self.opts)
            .field("position", &self.position)
            .finish_non_exhaustive()
    }
}
