// SPDX-License-Identifier: GPL-3.0-or-later
//! The synchronization primitives of the actor host, swappable for `loom`'s model-checked
//! ones (`RUSTFLAGS="--cfg cutweave_loom"`), so the loom tests exercise the production code.

use std::sync::PoisonError;

#[cfg(not(cutweave_loom))]
mod imp {
    pub(crate) use std::sync::{
        Arc, Condvar, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
        mpsc,
    };

    pub(crate) type JoinHandle = std::thread::JoinHandle<()>;

    pub(crate) fn spawn(
        name: String,
        f: impl FnOnce() + Send + 'static,
    ) -> std::io::Result<JoinHandle> {
        std::thread::Builder::new().name(name).spawn(f)
    }

    /// The published snapshot cell: lock-free loads (`arc-swap`), so readers never block, not
    /// even on the writer.
    pub(crate) struct Cell<T>(arc_swap::ArcSwap<T>);

    impl<T> Cell<T> {
        pub(crate) fn new(value: std::sync::Arc<T>) -> Cell<T> {
            Cell(arc_swap::ArcSwap::new(value))
        }

        /// Reads the current value without touching its reference count.
        pub(crate) fn read<R>(&self, f: impl FnOnce(&T) -> R) -> R {
            f(&self.0.load())
        }

        pub(crate) fn store(&self, value: std::sync::Arc<T>) {
            self.0.store(value);
        }
    }
}

#[cfg(cutweave_loom)]
mod imp {
    pub(crate) use loom::sync::{
        Arc, Condvar, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    };

    /// `std::sync::mpsc` semantics on loom's primitives. loom's own channel does not model
    /// disconnection (a send after the receiver is gone counts as a leaked message), which is
    /// exactly what shutdown races exercise.
    pub(crate) mod mpsc {
        pub(crate) use std::sync::mpsc::RecvTimeoutError;
        use std::{
            collections::VecDeque,
            sync::mpsc::{RecvError, SendError},
        };

        use super::{Arc, Condvar, Mutex};
        use crate::doc::sync::{lock, wait, wait_timeout};

        struct State<T> {
            queue: VecDeque<T>,
            senders: usize,
            receiver: bool,
        }

        struct Chan<T> {
            state: Mutex<State<T>>,
            ready: Condvar,
        }

        pub(crate) struct Sender<T>(Arc<Chan<T>>);

        pub(crate) struct Receiver<T>(Arc<Chan<T>>);

        pub(crate) fn channel<T>() -> (Sender<T>, Receiver<T>) {
            let chan = Arc::new(Chan {
                state: Mutex::new(State {
                    queue: VecDeque::new(),
                    senders: 1,
                    receiver: true,
                }),
                ready: Condvar::new(),
            });
            (Sender(Arc::clone(&chan)), Receiver(chan))
        }

        impl<T> Sender<T> {
            pub(crate) fn send(&self, value: T) -> Result<(), SendError<T>> {
                let mut state = lock(&self.0.state);
                if !state.receiver {
                    return Err(SendError(value));
                }
                state.queue.push_back(value);
                drop(state);
                self.0.ready.notify_one();
                Ok(())
            }
        }

        impl<T> Clone for Sender<T> {
            fn clone(&self) -> Sender<T> {
                lock(&self.0.state).senders += 1;
                Sender(Arc::clone(&self.0))
            }
        }

        impl<T> Drop for Sender<T> {
            fn drop(&mut self) {
                lock(&self.0.state).senders -= 1;
                self.0.ready.notify_one();
            }
        }

        impl<T> Receiver<T> {
            pub(crate) fn recv(&self) -> Result<T, RecvError> {
                let mut state = lock(&self.0.state);
                loop {
                    if let Some(value) = state.queue.pop_front() {
                        return Ok(value);
                    }
                    if state.senders == 0 {
                        return Err(RecvError);
                    }
                    state = wait(&self.0.ready, state);
                }
            }

            /// loom does not model time: one (possibly spurious) timed wait, then `Timeout`
            /// if nothing arrived.
            pub(crate) fn recv_timeout(
                &self,
                timeout: std::time::Duration,
            ) -> Result<T, RecvTimeoutError> {
                let mut state = lock(&self.0.state);
                for waited in [false, true] {
                    if let Some(value) = state.queue.pop_front() {
                        return Ok(value);
                    }
                    if state.senders == 0 {
                        return Err(RecvTimeoutError::Disconnected);
                    }
                    if !waited {
                        state = wait_timeout(&self.0.ready, state, timeout);
                    }
                }
                Err(RecvTimeoutError::Timeout)
            }
        }

        impl<T> Drop for Receiver<T> {
            fn drop(&mut self) {
                let dropped = {
                    let mut state = lock(&self.0.state);
                    state.receiver = false;
                    std::mem::take(&mut state.queue)
                };
                drop(dropped);
            }
        }
    }

    pub(crate) type JoinHandle = loom::thread::JoinHandle<()>;

    pub(crate) fn spawn(
        name: String,
        f: impl FnOnce() + Send + 'static,
    ) -> std::io::Result<JoinHandle> {
        loom::thread::Builder::new().name(name).spawn(f)
    }

    /// Under loom the cell is a modelled lock, so every interleaving of a publication with
    /// loads and replies is explored (arc-swap itself is not loom-instrumented).
    pub(crate) struct Cell<T>(Mutex<std::sync::Arc<T>>);

    impl<T> Cell<T> {
        pub(crate) fn new(value: std::sync::Arc<T>) -> Cell<T> {
            Cell(Mutex::new(value))
        }

        pub(crate) fn read<R>(&self, f: impl FnOnce(&T) -> R) -> R {
            f(&super::lock(&self.0))
        }

        pub(crate) fn store(&self, value: std::sync::Arc<T>) {
            *super::lock(&self.0) = value;
        }
    }
}

pub(crate) use imp::*;

/// Locks, ignoring poisoning: the guarded state is updated atomically (no user code runs
/// under these locks), so it is consistent even if a holder panicked.
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Waits on `cv`, ignoring poisoning (see [`lock`]).
pub(crate) fn wait<'a, T>(cv: &Condvar, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
    cv.wait(guard).unwrap_or_else(PoisonError::into_inner)
}

/// Waits on `cv` for at most `timeout`, ignoring poisoning.
pub(crate) fn wait_timeout<'a, T>(
    cv: &Condvar,
    guard: MutexGuard<'a, T>,
    timeout: std::time::Duration,
) -> MutexGuard<'a, T> {
    match cv.wait_timeout(guard, timeout) {
        Ok((guard, _)) => guard,
        Err(poisoned) => poisoned.into_inner().0,
    }
}
