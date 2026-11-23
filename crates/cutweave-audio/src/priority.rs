// SPDX-License-Identifier: GPL-3.0-or-later
//! Thread priority for the threads that feed the real-time callback (Spike S2 finding 6).
//!
//! Under heavy system load, normal-priority decode workers let the lane rings drain towards an
//! underrun; at `THREAD_PRIORITY_HIGHEST` they did not. The audio callback thread itself is
//! promoted by cpal (`realtime` feature: MMCSS on Windows). macOS (QoS user-interactive) and
//! Linux (rtkit/nice) are not done yet: there this returns `false`.

/// Raises the calling thread above normal priority. Returns whether it did.
#[must_use]
pub(crate) fn raise_current_thread() -> bool {
    imp::raise()
}

#[cfg(windows)]
mod imp {
    // Justification: two kernel32 calls (always linked by std on Windows); no crate offers just
    // these without pulling a large binding crate. The block is scoped to this module.
    #![allow(unsafe_code)]

    unsafe extern "system" {
        fn GetCurrentThread() -> isize;
        fn SetThreadPriority(thread: isize, priority: i32) -> i32;
    }

    /// `THREAD_PRIORITY_HIGHEST` (winbase.h): two steps above normal, below time-critical.
    const THREAD_PRIORITY_HIGHEST: i32 = 2;

    pub(super) fn raise() -> bool {
        // SAFETY: `GetCurrentThread` takes no arguments and returns a pseudo-handle that is
        // always valid for the calling thread; `SetThreadPriority` only reads that handle and an
        // integer, and reports failure through its return value.
        unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST) != 0 }
    }
}

#[cfg(not(windows))]
mod imp {
    pub(super) fn raise() -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn raising_works_where_implemented() {
        let raised = std::thread::spawn(super::raise_current_thread)
            .join()
            .unwrap_or(false);
        assert_eq!(raised, cfg!(windows));
    }
}
