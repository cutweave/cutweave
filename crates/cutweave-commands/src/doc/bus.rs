// SPDX-License-Identifier: GPL-3.0-or-later
//! Event broadcast: one bounded queue per subscriber ([`EventStream`]).
//!
//! The actor never blocks on a subscriber. A subscriber whose queue is full when an event
//! arrives loses its queued events and that one: its next receive reports
//! [`RecvError::Lagged`], and it must [`resync`](EventStream::resync) from a snapshot. Events
//! that arrive after the overflow are queued again as usual; `resync` skips the ones its
//! snapshot already contains, so snapshot + following events reproduce the live document.

use std::{
    collections::VecDeque,
    fmt,
    future::Future,
    task::{Context, Poll, Waker},
    time::{Duration, Instant},
};

use super::{
    event::{DocEvent, DocSnapshot},
    sync::{Arc, Condvar, Mutex, lock, wait, wait_timeout},
};
use crate::error::Rev;

/// Why a blocking or async receive returned no event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RecvError {
    /// The subscriber fell behind and `missed` events were dropped: call
    /// [`EventStream::resync`] and continue from the snapshot it returns.
    #[error("the subscriber fell behind and missed {missed} events; resync from a snapshot")]
    Lagged {
        /// How many events were dropped.
        missed: u64,
    },
    /// The actor stopped and every queued event was received.
    #[error("the document actor has stopped")]
    Closed,
}

/// Why a non-blocking receive returned no event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TryRecvError {
    /// No event is queued.
    #[error("no event is queued")]
    Empty,
    /// See [`RecvError::Lagged`].
    #[error("the subscriber fell behind and missed {missed} events; resync from a snapshot")]
    Lagged {
        /// How many events were dropped.
        missed: u64,
    },
    /// See [`RecvError::Closed`].
    #[error("the document actor has stopped")]
    Closed,
}

struct SubState {
    queue: VecDeque<std::sync::Arc<DocEvent>>,
    missed: u64,
    closed: bool,
    waker: Option<Waker>,
}

pub(crate) struct Sub {
    state: Mutex<SubState>,
    ready: Condvar,
}

struct BusInner {
    subs: Vec<(u64, Arc<Sub>)>,
    next_id: u64,
    closed: bool,
}

/// The subscriber registry.
pub(crate) struct Bus {
    inner: Mutex<BusInner>,
    capacity: usize,
}

impl Bus {
    pub(crate) fn new(capacity: usize) -> Bus {
        Bus {
            inner: Mutex::new(BusInner {
                subs: Vec::new(),
                next_id: 0,
                closed: false,
            }),
            capacity: capacity.max(1),
        }
    }

    pub(crate) fn subscribe(&self) -> (u64, Arc<Sub>) {
        let mut inner = lock(&self.inner);
        let sub = Arc::new(Sub {
            state: Mutex::new(SubState {
                queue: VecDeque::new(),
                missed: 0,
                closed: inner.closed,
                waker: None,
            }),
            ready: Condvar::new(),
        });
        let id = inner.next_id;
        inner.next_id += 1;
        inner.subs.push((id, Arc::clone(&sub)));
        (id, sub)
    }

    fn unsubscribe(&self, id: u64) {
        lock(&self.inner).subs.retain(|(sid, _)| *sid != id);
    }

    #[cfg(test)]
    pub(crate) fn subscribers(&self) -> usize {
        lock(&self.inner).subs.len()
    }

    /// Queues `events` for every subscriber; returns how many subscribers newly lagged.
    pub(crate) fn publish(&self, events: &[std::sync::Arc<DocEvent>]) -> usize {
        if events.is_empty() {
            return 0;
        }
        let inner = lock(&self.inner);
        let mut lagged = 0;
        for (_, sub) in &inner.subs {
            let waker = {
                let mut state = lock(&sub.state);
                for event in events {
                    if state.queue.len() >= self.capacity {
                        if state.missed == 0 {
                            lagged += 1;
                        }
                        let dropped = state.queue.len() as u64 + 1;
                        state.missed = state.missed.saturating_add(dropped);
                        state.queue.clear();
                    } else {
                        state.queue.push_back(std::sync::Arc::clone(event));
                    }
                }
                state.waker.take()
            };
            sub.ready.notify_all();
            if let Some(waker) = waker {
                waker.wake();
            }
        }
        lagged
    }

    /// Marks every subscriber closed (they drain their queues first).
    pub(crate) fn close(&self) {
        let mut inner = lock(&self.inner);
        inner.closed = true;
        for (_, sub) in &inner.subs {
            let waker = {
                let mut state = lock(&sub.state);
                state.closed = true;
                state.waker.take()
            };
            sub.ready.notify_all();
            if let Some(waker) = waker {
                waker.wake();
            }
        }
    }
}

/// Loads the current live snapshot (for [`EventStream::resync`]).
pub(crate) type LoadLive = std::sync::Arc<dyn Fn() -> std::sync::Arc<DocSnapshot> + Send + Sync>;

enum Next {
    Event(std::sync::Arc<DocEvent>),
    Lagged(u64),
    Closed,
    Empty,
}

fn next(state: &mut SubState, skip_through: Option<Rev>) -> Next {
    if state.missed > 0 {
        return Next::Lagged(std::mem::take(&mut state.missed));
    }
    while let Some(event) = state.queue.pop_front() {
        if let (Some(skip), Some(rev)) = (skip_through, event.rev())
            && rev <= skip
        {
            continue;
        }
        return Next::Event(event);
    }
    if state.closed {
        Next::Closed
    } else {
        Next::Empty
    }
}

/// A subscription to the document's events (`doc.patched` and friends), in revision order.
///
/// Each stream has its own bounded queue
/// ([`ActorOptions::event_capacity`](crate::ActorOptions::event_capacity)), and the actor
/// never blocks on it. A stream whose
/// queue is full when an event arrives loses its queued events and that one: its next receive
/// reports [`RecvError::Lagged`], and it must [`resync`](Self::resync) from a snapshot. Events
/// that arrive after the overflow are queued again as usual; `resync` skips the ones its
/// snapshot already contains, so snapshot + following events reproduce the live document.
pub struct EventStream {
    id: u64,
    sub: Arc<Sub>,
    bus: Arc<Bus>,
    live: LoadLive,
    skip_through: Option<Rev>,
}

impl fmt::Debug for EventStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventStream")
            .field("id", &self.id)
            .field("skip_through", &self.skip_through)
            .finish_non_exhaustive()
    }
}

impl EventStream {
    pub(crate) fn new(bus: Arc<Bus>, live: LoadLive) -> EventStream {
        let (id, sub) = bus.subscribe();
        EventStream {
            id,
            sub,
            bus,
            live,
            skip_through: None,
        }
    }

    /// The next event if one is queued.
    pub fn try_recv(&mut self) -> Result<std::sync::Arc<DocEvent>, TryRecvError> {
        match next(&mut lock(&self.sub.state), self.skip_through) {
            Next::Event(event) => Ok(event),
            Next::Lagged(missed) => Err(TryRecvError::Lagged { missed }),
            Next::Closed => Err(TryRecvError::Closed),
            Next::Empty => Err(TryRecvError::Empty),
        }
    }

    /// Blocks the calling thread until an event arrives (not from an async worker; `.await`
    /// [`recv`](Self::recv) there).
    pub fn recv_blocking(&mut self) -> Result<std::sync::Arc<DocEvent>, RecvError> {
        let mut state = lock(&self.sub.state);
        loop {
            match next(&mut state, self.skip_through) {
                Next::Event(event) => return Ok(event),
                Next::Lagged(missed) => return Err(RecvError::Lagged { missed }),
                Next::Closed => return Err(RecvError::Closed),
                Next::Empty => state = wait(&self.sub.ready, state),
            }
        }
    }

    /// Blocks for at most `timeout`; `Ok(None)` if no event arrived.
    pub fn recv_timeout(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<std::sync::Arc<DocEvent>>, RecvError> {
        let deadline = Instant::now() + timeout;
        let mut state = lock(&self.sub.state);
        loop {
            match next(&mut state, self.skip_through) {
                Next::Event(event) => return Ok(Some(event)),
                Next::Lagged(missed) => return Err(RecvError::Lagged { missed }),
                Next::Closed => return Err(RecvError::Closed),
                Next::Empty => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Ok(None);
                    }
                    state = wait_timeout(&self.sub.ready, state, left);
                }
            }
        }
    }

    /// Polls for the next event (registers the waker when none is queued).
    pub fn poll_recv(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Result<std::sync::Arc<DocEvent>, RecvError>> {
        let mut state = lock(&self.sub.state);
        match next(&mut state, self.skip_through) {
            Next::Event(event) => Poll::Ready(Ok(event)),
            Next::Lagged(missed) => Poll::Ready(Err(RecvError::Lagged { missed })),
            Next::Closed => Poll::Ready(Err(RecvError::Closed)),
            Next::Empty => {
                match &state.waker {
                    Some(w) if w.will_wake(cx.waker()) => {}
                    _ => state.waker = Some(cx.waker().clone()),
                }
                Poll::Pending
            }
        }
    }

    /// The next event (async, any executor).
    pub fn recv(
        &mut self,
    ) -> impl Future<Output = Result<std::sync::Arc<DocEvent>, RecvError>> + '_ {
        std::future::poll_fn(move |cx| self.poll_recv(cx))
    }

    /// Recovers from [`RecvError::Lagged`] (or starts a mirror): returns the current live
    /// snapshot and makes the stream skip every event it already contains, so applying the
    /// following events to it tracks the live document.
    pub fn resync(&mut self) -> std::sync::Arc<DocSnapshot> {
        let mut state = lock(&self.sub.state);
        let snapshot = (self.live)();
        state.missed = 0;
        let rev = snapshot.rev;
        state
            .queue
            .retain(|event| event.rev().is_none_or(|r| r > rev));
        self.skip_through = Some(rev);
        snapshot
    }
}

impl Drop for EventStream {
    fn drop(&mut self) {
        self.bus.unsubscribe(self.id);
    }
}

#[cfg(all(test, not(cutweave_loom)))]
mod tests {
    #![allow(clippy::unwrap_used)]

    use cutweave_model::Project;

    use super::*;
    use crate::{
        doc::{
            event::{DocPatched, PatchCause},
            identity::Actor,
            request::TxId,
        },
        patch::Patch,
    };

    struct Source {
        bus: Arc<Bus>,
        live: std::sync::Arc<std::sync::Mutex<std::sync::Arc<DocSnapshot>>>,
    }

    impl Source {
        fn stream(&self) -> EventStream {
            let live = std::sync::Arc::clone(&self.live);
            EventStream::new(
                Arc::clone(&self.bus),
                std::sync::Arc::new(move || live.lock().unwrap().clone()),
            )
        }
    }

    fn source(capacity: usize) -> Arc<Source> {
        Arc::new(Source {
            bus: Arc::new(Bus::new(capacity)),
            live: std::sync::Arc::new(std::sync::Mutex::new(std::sync::Arc::new(DocSnapshot {
                rev: Rev(0),
                committed_rev: Rev(0),
                doc: Project::new("x"),
            }))),
        })
    }

    fn event(rev: u64) -> std::sync::Arc<DocEvent> {
        std::sync::Arc::new(DocEvent::Patched(DocPatched {
            rev: Rev(rev),
            prev_rev: Rev(rev - 1),
            committed_rev: Rev(rev),
            actor: Actor::User,
            tx_id: TxId::new(rev),
            cause: PatchCause::Command,
            request_id: None,
            ops: Patch::new(),
        }))
    }

    fn set_rev(src: &Source, rev: u64) {
        *src.live.lock().unwrap() = std::sync::Arc::new(DocSnapshot {
            rev: Rev(rev),
            committed_rev: Rev(rev),
            doc: Project::new("x"),
        });
    }

    #[test]
    fn events_arrive_in_order() {
        let src = source(8);
        let mut stream = src.stream();
        src.bus.publish(&[event(1), event(2)]);
        assert_eq!(stream.try_recv().unwrap().rev(), Some(Rev(1)));
        assert_eq!(stream.recv_blocking().unwrap().rev(), Some(Rev(2)));
        assert_eq!(stream.try_recv().unwrap_err(), TryRecvError::Empty);
        assert!(
            stream
                .recv_timeout(Duration::from_millis(5))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn overflow_reports_lag_once_then_resync_skips_stale_events() {
        let src = source(2);
        let mut stream = src.stream();
        let lagged = src.bus.publish(&[event(1), event(2), event(3)]);
        assert_eq!(lagged, 1);
        // 1 and 2 were queued; 3 overflowed: all three dropped.
        src.bus.publish(&[event(4)]);
        assert_eq!(
            stream.try_recv().unwrap_err(),
            TryRecvError::Lagged { missed: 3 }
        );
        // The snapshot already contains 4 and 5; only 6 is new.
        set_rev(&src, 5);
        src.bus.publish(&[event(5)]);
        let snap = stream.resync();
        assert_eq!(snap.rev, Rev(5));
        src.bus.publish(&[event(6)]);
        assert_eq!(stream.try_recv().unwrap().rev(), Some(Rev(6)));
        assert_eq!(stream.try_recv().unwrap_err(), TryRecvError::Empty);
    }

    #[test]
    fn close_drains_then_reports_closed_and_drop_unsubscribes() {
        let src = source(4);
        let mut stream = src.stream();
        assert_eq!(src.bus.subscribers(), 1);
        src.bus.publish(&[event(1)]);
        src.bus.close();
        assert!(stream.recv_blocking().is_ok());
        assert_eq!(stream.recv_blocking().unwrap_err(), RecvError::Closed);
        drop(stream);
        assert_eq!(src.bus.subscribers(), 0);
        // Subscribing after close yields a closed stream.
        let mut late = src.stream();
        assert_eq!(late.try_recv().unwrap_err(), TryRecvError::Closed);
    }

    #[test]
    fn blocking_receive_wakes_on_publish() {
        let src = source(4);
        let mut stream = src.stream();
        let publisher = {
            let src = src.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(20));
                src.bus.publish(&[event(1)]);
            })
        };
        assert_eq!(stream.recv_blocking().unwrap().rev(), Some(Rev(1)));
        publisher.join().unwrap();
    }
}
