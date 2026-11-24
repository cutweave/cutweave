// SPDX-License-Identifier: GPL-3.0-or-later
//! [`CancelToken`]: cancels decoder requests (WP-1D.03).
//!
//! A token is handed to [`DecoderPool::checkout`](crate::DecoderPool::checkout) or set on a
//! decoder with [`VideoDecoder::set_cancel`](crate::VideoDecoder::set_cancel). Decoders poll it
//! between packets, so a decoder called directly returns [`MediaError::Cancelled`] once the
//! FFmpeg call in progress ends. Through the pool, [`CancelToken::cancel`] also wakes the
//! blocked caller directly (a checkout waiting for a free decoder, a lease waiting for its
//! decoder thread), so the request returns at once (the WBS budget is 10 ms).
//!
//! Other crates have their own cancellation (`cutweave-jobs`' job token, `cutweave-playback`'s
//! render epochs). [`CancelToken::linked`] bridges them without a dependency: the token then
//! also reads cancelled whenever their check does. A linked check can only be polled, and a
//! blocked wait polls it every millisecond, which the OS timer can stretch (about 15 ms on
//! Windows); [`CancelToken::cancel`] wakes waits at once, so call it where the 10 ms budget
//! matters.

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use crate::{CancelCheck, MediaError};

/// Called once when the token is cancelled, to wake a blocked wait.
pub(crate) type Waker = Arc<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct Inner {
    cancelled: AtomicBool,
    linked: Option<CancelCheck>,
    wakers: Mutex<Vec<(u64, Waker)>>,
    next_waker: AtomicU64,
}

/// A cancellation flag for decoder requests. Clones share it; polling is one atomic load (plus
/// the linked check, if any).
#[derive(Clone, Default)]
pub struct CancelToken(Arc<Inner>);

impl CancelToken {
    /// A token that is not cancelled.
    #[must_use]
    pub fn new() -> CancelToken {
        CancelToken::default()
    }

    /// A token that is also cancelled whenever `check` answers `true`, for example
    /// `CancelCheck::new(move || job_token.is_cancelled())`. `check` is polled between packets,
    /// so it must be cheap and must not block.
    #[must_use]
    pub fn linked(check: CancelCheck) -> CancelToken {
        CancelToken(Arc::new(Inner {
            linked: Some(check),
            ..Inner::default()
        }))
    }

    /// Requests cancellation and wakes the waits blocked on this token (a checkout waiting for
    /// a decoder, a lease waiting for its decoder thread). Idempotent; never blocks for long.
    pub fn cancel(&self) {
        if !self.0.cancelled.swap(true, Ordering::AcqRel) {
            let wakers: Vec<Waker> = self
                .0
                .wakers
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .iter()
                .map(|(_, w)| Arc::clone(w))
                .collect();
            // Called without holding the list: a waker takes its waiter's lock.
            for wake in wakers {
                wake();
            }
        }
    }

    /// Calls `wake` when [`cancel`](Self::cancel) is called, until the guard is dropped. The
    /// waiter registers before it checks [`is_cancelled`](Self::is_cancelled) and blocks; `wake`
    /// takes the waiter's lock before notifying, so no cancel is missed.
    pub(crate) fn on_cancel(&self, wake: Waker) -> WakerGuard<'_> {
        let id = self.0.next_waker.fetch_add(1, Ordering::Relaxed);
        self.0
            .wakers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((id, wake));
        WakerGuard { token: self, id }
    }

    /// Whether cancellation was requested (here or through the linked check).
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Acquire)
            || self
                .0
                .linked
                .as_ref()
                .is_some_and(CancelCheck::is_cancelled)
    }

    /// `Err(MediaError::Cancelled)` once cancellation was requested: `token.check()?` between
    /// units of work.
    pub fn check(&self) -> Result<(), MediaError> {
        if self.is_cancelled() {
            Err(MediaError::Cancelled)
        } else {
            Ok(())
        }
    }

    /// The token as an encoder's [`CancelCheck`] (an export reading through the decoder pool and
    /// writing through an encoder cancels both with one token).
    #[must_use]
    pub fn to_check(&self) -> CancelCheck {
        let token = self.clone();
        CancelCheck::new(move || token.is_cancelled())
    }

    /// Whether both are clones of the same token.
    #[must_use]
    pub fn same(&self, other: &CancelToken) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl fmt::Debug for CancelToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CancelToken")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

/// Unregisters a waker ([`CancelToken::on_cancel`]) when dropped.
pub(crate) struct WakerGuard<'a> {
    token: &'a CancelToken,
    id: u64,
}

impl Drop for WakerGuard<'_> {
    fn drop(&mut self) {
        self.token
            .0
            .wakers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|(id, _)| *id != self.id);
    }
}

/// `Ok(())` without a token, else [`CancelToken::check`].
pub(crate) fn check(token: Option<&CancelToken>) -> Result<(), MediaError> {
    token.map_or(Ok(()), CancelToken::check)
}

#[cfg(test)]
mod tests {
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
        assert_eq!(b.check(), Err(MediaError::Cancelled));
        assert!(a.same(&b));
        assert!(!a.same(&CancelToken::new()));
        assert!(format!("{b:?}").contains("true"));
    }

    #[test]
    fn linked_checks_cancel_too() {
        let flag = Arc::new(AtomicBool::new(false));
        let token = CancelToken::linked(CancelCheck::from_flag(Arc::clone(&flag)));
        assert!(!token.is_cancelled());
        flag.store(true, Ordering::Relaxed);
        assert!(token.is_cancelled());
        assert!(token.to_check().is_cancelled());
    }

    #[test]
    fn cancel_calls_registered_wakers_once() {
        use std::sync::atomic::AtomicUsize;
        let token = CancelToken::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let waker: Waker = {
            let calls = Arc::clone(&calls);
            Arc::new(move || {
                calls.fetch_add(1, Ordering::Relaxed);
            })
        };
        let gone = token.on_cancel(Arc::clone(&waker));
        drop(gone);
        let _kept = token.on_cancel(waker);
        token.cancel();
        token.cancel();
        assert_eq!(
            calls.load(Ordering::Relaxed),
            1,
            "one live waker, woken once"
        );
    }

    #[test]
    fn no_token_never_cancels() {
        assert_eq!(check(None), Ok(()));
        let t = CancelToken::new();
        t.cancel();
        assert_eq!(check(Some(&t)), Err(MediaError::Cancelled));
    }
}
