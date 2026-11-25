// SPDX-License-Identifier: GPL-3.0-or-later
//! [`Hold`]: keeps a queued job from taking its budget slot until someone releases it (the
//! export queue's turn, WP-2G.03a).

use std::{
    fmt,
    pin::pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use tokio::sync::Notify;

#[derive(Default)]
struct Inner {
    released: AtomicBool,
    notify: Notify,
}

/// A one-way latch on a queued job: the job waits (without a thread or a budget slot) until
/// [`release`](Self::release) is called. Clones share it. A held job can still be cancelled.
#[derive(Clone, Default)]
pub struct Hold(Arc<Inner>);

impl Hold {
    /// A hold that is not released.
    #[must_use]
    pub fn new() -> Hold {
        Hold::default()
    }

    /// Lets the job go on to its budget slot. Idempotent.
    pub fn release(&self) {
        if !self.0.released.swap(true, Ordering::SeqCst) {
            self.0.notify.notify_waiters();
        }
    }

    /// Whether it was released.
    #[must_use]
    pub fn is_released(&self) -> bool {
        self.0.released.load(Ordering::SeqCst)
    }

    /// Completes once released.
    pub async fn released(&self) {
        loop {
            let mut notified = pin!(self.0.notify.notified());
            // Register before checking the flag, so a `release` in between is not missed.
            notified.as_mut().enable();
            if self.is_released() {
                return;
            }
            notified.await;
        }
    }
}

impl fmt::Debug for Hold {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Hold")
            .field("released", &self.is_released())
            .finish()
    }
}
