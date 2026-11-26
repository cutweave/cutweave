// SPDX-License-Identifier: GPL-3.0-or-later
//! The state machine of a queued export (WP-2G.03a): what may follow what.
//!
//! ```text
//!            dispatch                finish
//! queued ───────────────▶ running ─────────────▶ succeeded | failed | cancelled
//!   │  ▲                    │                              │
//!   │  └──── interrupt ─────┘ (engine stopped or killed,   │
//!   │                          project switched away)      │
//!   │  ▲                                                   │
//!   │  └──────────────────── retry ────────────────────────┘ (failed or cancelled only)
//!   └──── cancel ──▶ cancelled
//! ```
//!
//! The WBS names the final states `done | failed | canceled`; the queue uses the job states of
//! `job.status` (`succeeded`, `failed`, `cancelled`), so `export.queue` and `job.status` agree.

use crate::types::JobState;

/// What happens to a queued export.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueEvent {
    /// Its turn came: it takes an export slot.
    Dispatch,
    /// It ended (or was cancelled before its turn) with this final state.
    Finish(JobState),
    /// The engine stopped (or was killed) or the project was switched away while it ran: it
    /// starts again from the beginning next time.
    Interrupt,
    /// `export.retry`: a failed or cancelled export is queued again.
    Retry,
}

/// The state after `event`, or `None` when the event cannot happen in `from`.
#[must_use]
pub fn next_state(from: JobState, event: QueueEvent) -> Option<JobState> {
    use JobState::{Cancelled, Failed, Queued, Running, Succeeded};
    match (from, event) {
        (Queued, QueueEvent::Dispatch) => Some(Running),
        (Queued, QueueEvent::Finish(Cancelled)) => Some(Cancelled),
        // A queued job whose dispatch raced its end (the manager finished it first).
        (Queued | Running, QueueEvent::Finish(end @ (Succeeded | Failed | Cancelled))) => Some(end),
        (Running | Queued, QueueEvent::Interrupt) | (Failed | Cancelled, QueueEvent::Retry) => {
            Some(Queued)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use JobState::{Cancelled, Failed, Queued, Running, Succeeded};

    const ALL: [JobState; 5] = [Queued, Running, Succeeded, Failed, Cancelled];

    #[test]
    fn the_happy_path_and_its_ends() {
        assert_eq!(next_state(Queued, QueueEvent::Dispatch), Some(Running));
        for end in [Succeeded, Failed, Cancelled] {
            assert_eq!(next_state(Running, QueueEvent::Finish(end)), Some(end));
        }
        assert_eq!(
            next_state(Queued, QueueEvent::Finish(Cancelled)),
            Some(Cancelled)
        );
    }

    #[test]
    fn final_states_only_leave_by_retry_and_success_never_does() {
        for end in [Succeeded, Failed, Cancelled] {
            for event in [
                QueueEvent::Dispatch,
                QueueEvent::Interrupt,
                QueueEvent::Finish(Succeeded),
                QueueEvent::Finish(Failed),
                QueueEvent::Finish(Cancelled),
            ] {
                assert_eq!(next_state(end, event), None, "{end:?} on {event:?}");
            }
        }
        assert_eq!(next_state(Failed, QueueEvent::Retry), Some(Queued));
        assert_eq!(next_state(Cancelled, QueueEvent::Retry), Some(Queued));
        assert_eq!(next_state(Succeeded, QueueEvent::Retry), None);
    }

    #[test]
    fn an_interrupted_export_starts_again_and_running_ones_are_not_dispatched_twice() {
        assert_eq!(next_state(Running, QueueEvent::Interrupt), Some(Queued));
        assert_eq!(next_state(Queued, QueueEvent::Interrupt), Some(Queued));
        assert_eq!(next_state(Running, QueueEvent::Dispatch), None);
        for from in ALL {
            assert_ne!(
                next_state(from, QueueEvent::Finish(Queued)),
                Some(Queued),
                "queued is not an end"
            );
            assert_eq!(next_state(from, QueueEvent::Finish(Running)), None);
        }
        // Nothing but a retry leaves a final state; nothing reaches `running` but a dispatch.
        for from in ALL {
            for event in [
                QueueEvent::Dispatch,
                QueueEvent::Interrupt,
                QueueEvent::Retry,
                QueueEvent::Finish(Cancelled),
            ] {
                if next_state(from, event) == Some(Running) {
                    assert_eq!((from, event), (Queued, QueueEvent::Dispatch));
                }
            }
        }
    }
}
