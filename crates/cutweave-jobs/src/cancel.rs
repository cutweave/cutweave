// SPDX-License-Identifier: GPL-3.0-or-later
//! [`CancelToken`]: cooperative cancellation shared by a job's body, the manager and callers.

use std::{
    fmt,
    pin::pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use tokio::sync::Notify;

use crate::types::JobError;

#[derive(Default)]
struct Inner {
    cancelled: AtomicBool,
    notify: Notify,
}

/// A cancellation flag. Clones share it. Job bodies poll [`is_cancelled`](Self::is_cancelled)
/// (or [`check`](Self::check)) between units of work, often enough to stop within the
/// 500 ms the job manager promises (WP-1D.07); async code awaits
/// [`cancelled`](Self::cancelled).
#[derive(Clone, Default)]
pub struct CancelToken(Arc<Inner>);

impl CancelToken {
    /// A token that is not cancelled.
    #[must_use]
    pub fn new() -> CancelToken {
        CancelToken::default()
    }

    /// Requests cancellation. Idempotent.
    pub fn cancel(&self) {
        if !self.0.cancelled.swap(true, Ordering::SeqCst) {
            self.0.notify.notify_waiters();
        }
    }

    /// Whether cancellation was requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::SeqCst)
    }

    /// `Err(JobError::Cancelled)` once cancellation was requested: `token.check()?` between
    /// units of work.
    pub fn check(&self) -> Result<(), JobError> {
        if self.is_cancelled() {
            Err(JobError::Cancelled)
        } else {
            Ok(())
        }
    }

    /// Completes once cancellation is requested.
    pub async fn cancelled(&self) {
        loop {
            let mut notified = pin!(self.0.notify.notified());
            // Register before checking the flag, so a `cancel` in between is not missed.
            notified.as_mut().enable();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

impl fmt::Debug for CancelToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CancelToken")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::time::Duration;

    use super::*;

    #[test]
    fn clones_share_the_flag() {
        let a = CancelToken::new();
        let b = a.clone();
        assert!(!b.is_cancelled());
        assert_eq!(b.check(), Ok(()));
        a.cancel();
        a.cancel();
        assert!(b.is_cancelled());
        assert_eq!(b.check(), Err(JobError::Cancelled));
        assert!(format!("{b:?}").contains("true"));
    }

    #[test]
    fn cancelled_wakes_async_waiters() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let token = CancelToken::new();
        let waiter = token.clone();
        rt.block_on(async move {
            let wait = tokio::spawn(async move { waiter.cancelled().await });
            tokio::time::sleep(Duration::from_millis(10)).await;
            token.cancel();
            tokio::time::timeout(Duration::from_secs(5), wait)
                .await
                .unwrap()
                .unwrap();
            // Already cancelled: returns at once.
            token.cancelled().await;
        });
    }
}
