// SPDX-License-Identifier: GPL-3.0-or-later
//! [`Pending`]: the reply to a request sent to the document actor, usable from async code (it
//! is a `Future`, runtime-agnostic) and from plain threads ([`Pending::wait`]).

use std::{
    fmt,
    future::Future,
    pin::Pin,
    task::{Context, Poll, Waker},
    time::{Duration, Instant},
};

use super::sync::{Arc, Condvar, Mutex, lock, wait, wait_timeout};
use crate::error::CmdError;

enum State<T> {
    Waiting(Option<Waker>),
    Done(Result<T, CmdError>),
    Taken,
}

struct Slot<T> {
    state: Mutex<State<T>>,
    ready: Condvar,
}

impl<T> Slot<T> {
    fn fill(&self, result: Result<T, CmdError>) {
        let waker = {
            let mut state = lock(&self.state);
            match std::mem::replace(&mut *state, State::Done(result)) {
                State::Waiting(waker) => waker,
                // Only one `Reply` exists per slot, so it is filled once; keep the first.
                previous => {
                    *state = previous;
                    None
                }
            }
        };
        self.ready.notify_all();
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

/// The error a request gets when the actor stopped before answering it.
pub(crate) fn stopped() -> CmdError {
    CmdError::internal("the document actor has stopped")
}

fn taken() -> CmdError {
    CmdError::internal("the result was already taken")
}

/// A one-shot reply channel.
pub(crate) fn channel<T>() -> (Reply<T>, Pending<T>) {
    let slot = Arc::new(Slot {
        state: Mutex::new(State::Waiting(None)),
        ready: Condvar::new(),
    });
    (
        Reply {
            slot: Some(Arc::clone(&slot)),
        },
        Pending { slot },
    )
}

/// The sending half: dropping it unsent answers "actor stopped".
pub(crate) struct Reply<T> {
    slot: Option<Arc<Slot<T>>>,
}

impl<T> Reply<T> {
    pub(crate) fn send(mut self, result: Result<T, CmdError>) {
        if let Some(slot) = self.slot.take() {
            slot.fill(result);
        }
    }
}

impl<T> Drop for Reply<T> {
    fn drop(&mut self) {
        if let Some(slot) = self.slot.take() {
            slot.fill(Err(stopped()));
        }
    }
}

/// The eventual result of a request to the document actor.
///
/// `.await` it (any executor), or block on it with [`wait`](Self::wait) from a thread that is
/// not an async worker. If the actor stops before answering, the result is an `Internal`
/// error. Dropping it does not cancel the request.
#[must_use = "the request runs anyway; its result is lost unless awaited or waited for"]
pub struct Pending<T> {
    slot: Arc<Slot<T>>,
}

// No structural pinning: the value lives behind the `Arc` and is only moved out.
impl<T> Unpin for Pending<T> {}

impl<T> fmt::Debug for Pending<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pending")
            .field("ready", &self.is_ready())
            .finish()
    }
}

impl<T> Pending<T> {
    /// Whether the result is available.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        !matches!(*lock(&self.slot.state), State::Waiting(_))
    }

    /// Blocks the calling thread until the result is available.
    pub fn wait(self) -> Result<T, CmdError> {
        let mut state = lock(&self.slot.state);
        loop {
            match std::mem::replace(&mut *state, State::Taken) {
                State::Done(result) => return result,
                State::Taken => return Err(taken()),
                waiting @ State::Waiting(_) => {
                    *state = waiting;
                    state = wait(&self.slot.ready, state);
                }
            }
        }
    }

    /// Blocks for at most `timeout`; `None` if the result is not available yet (wait again).
    pub fn wait_timeout(&mut self, timeout: Duration) -> Option<Result<T, CmdError>> {
        let deadline = Instant::now() + timeout;
        let mut state = lock(&self.slot.state);
        loop {
            match std::mem::replace(&mut *state, State::Taken) {
                State::Done(result) => return Some(result),
                State::Taken => return Some(Err(taken())),
                waiting @ State::Waiting(_) => {
                    *state = waiting;
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return None;
                    }
                    state = wait_timeout(&self.slot.ready, state, left);
                }
            }
        }
    }
}

impl<T> Future for Pending<T> {
    type Output = Result<T, CmdError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = lock(&self.slot.state);
        match std::mem::replace(&mut *state, State::Taken) {
            State::Done(result) => Poll::Ready(result),
            State::Taken => Poll::Ready(Err(taken())),
            State::Waiting(waker) => {
                let waker = match waker {
                    Some(w) if w.will_wake(cx.waker()) => w,
                    _ => cx.waker().clone(),
                };
                *state = State::Waiting(Some(waker));
                Poll::Pending
            }
        }
    }
}

#[cfg(all(test, not(cutweave_loom)))]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        task::Wake,
    };

    use super::*;

    struct CountingWaker(AtomicUsize);

    impl Wake for CountingWaker {
        fn wake(self: std::sync::Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn reply_then_wait() {
        let (reply, pending) = channel::<u32>();
        reply.send(Ok(7));
        assert_eq!(pending.wait().unwrap(), 7);
    }

    #[test]
    fn wait_blocks_until_reply_from_another_thread() {
        let (reply, pending) = channel::<u32>();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            reply.send(Ok(9));
        });
        assert_eq!(pending.wait().unwrap(), 9);
        t.join().unwrap();
    }

    #[test]
    fn dropped_reply_reports_a_stopped_actor() {
        let (reply, pending) = channel::<u32>();
        drop(reply);
        assert_eq!(pending.wait().unwrap_err(), stopped());
    }

    #[test]
    fn wait_timeout_returns_none_then_the_value() {
        let (reply, mut pending) = channel::<u32>();
        assert!(pending.wait_timeout(Duration::from_millis(5)).is_none());
        assert!(!pending.is_ready());
        reply.send(Ok(1));
        assert!(pending.is_ready());
        assert_eq!(pending.wait_timeout(Duration::ZERO).unwrap().unwrap(), 1);
    }

    #[test]
    fn polling_registers_the_waker_and_completes() {
        let (reply, mut pending) = channel::<u32>();
        let counter = std::sync::Arc::new(CountingWaker(AtomicUsize::new(0)));
        let waker = Waker::from(std::sync::Arc::clone(&counter));
        let mut cx = Context::from_waker(&waker);
        assert!(Pin::new(&mut pending).poll(&mut cx).is_pending());
        reply.send(Ok(3));
        assert_eq!(counter.0.load(Ordering::SeqCst), 1);
        match Pin::new(&mut pending).poll(&mut cx) {
            Poll::Ready(r) => assert_eq!(r.unwrap(), 3),
            Poll::Pending => panic!("not ready"),
        }
        // Polling again after completion is an error, not a hang.
        assert!(matches!(
            Pin::new(&mut pending).poll(&mut cx),
            Poll::Ready(Err(_))
        ));
    }
}
