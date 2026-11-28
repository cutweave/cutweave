// SPDX-License-Identifier: GPL-3.0-or-later
//! Finished jobs: the [`JobHistory`] seam and its in-memory default, [`MemoryHistory`].
//!
//! The WBS keeps job history in a `jobs` table of `cache.sqlite`; that database arrives with
//! WP-1D.05, which adds a sqlite-backed [`JobHistory`]. Until then history lives in memory and
//! is lost on restart (resuming jobs across restarts is P5 either way).

use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Mutex, PoisonError},
};

use crate::{
    id::JobId,
    types::{JobFilter, JobInfo},
};

/// Default number of finished jobs [`MemoryHistory`] keeps.
pub const DEFAULT_HISTORY_CAPACITY: usize = 500;

/// Where finished jobs go. The manager moves a job here the moment it reaches a final state,
/// so `job.status` / `job.wait` / `job.list` keep finding it.
///
/// The manager calls these with its own lock held: implementations must be quick (buffer slow
/// writes, e.g. to sqlite, on their own thread) and must not call back into the manager.
pub trait JobHistory: Send + Sync {
    /// Remembers a finished job (its final [`JobInfo`]).
    fn record(&self, job: &JobInfo);

    /// The finished job `id`, if remembered.
    fn get(&self, id: JobId) -> Option<JobInfo>;

    /// Finished jobs passing `filter`, newest (highest ID) first, at most `limit`.
    fn list(&self, filter: &JobFilter, limit: usize) -> Vec<JobInfo>;
}

#[derive(Debug, Default)]
struct Ring {
    jobs: BTreeMap<JobId, JobInfo>,
    /// Finish order, oldest first, for eviction.
    order: VecDeque<JobId>,
}

/// A bounded in-memory [`JobHistory`]: the last `capacity` finished jobs (oldest finished
/// evicted first).
#[derive(Debug)]
pub struct MemoryHistory {
    capacity: usize,
    ring: Mutex<Ring>,
}

impl MemoryHistory {
    /// Keeps at most `capacity` finished jobs (at least one).
    #[must_use]
    pub fn new(capacity: usize) -> MemoryHistory {
        MemoryHistory {
            capacity: capacity.max(1),
            ring: Mutex::new(Ring::default()),
        }
    }

    /// Number of remembered jobs.
    #[must_use]
    pub fn len(&self) -> usize {
        self.ring
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .jobs
            .len()
    }

    /// Whether nothing is remembered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for MemoryHistory {
    fn default() -> MemoryHistory {
        MemoryHistory::new(DEFAULT_HISTORY_CAPACITY)
    }
}

impl JobHistory for MemoryHistory {
    fn record(&self, job: &JobInfo) {
        let mut ring = self.ring.lock().unwrap_or_else(PoisonError::into_inner);
        if ring.jobs.insert(job.id, job.clone()).is_none() {
            ring.order.push_back(job.id);
        }
        while ring.jobs.len() > self.capacity {
            let Some(oldest) = ring.order.pop_front() else {
                break;
            };
            ring.jobs.remove(&oldest);
        }
    }

    fn get(&self, id: JobId) -> Option<JobInfo> {
        self.ring
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .jobs
            .get(&id)
            .cloned()
    }

    fn list(&self, filter: &JobFilter, limit: usize) -> Vec<JobInfo> {
        self.ring
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .jobs
            .values()
            .rev()
            .filter(|j| filter.matches(j))
            .take(limit)
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{JobKind, JobProgress, JobState};

    fn job(n: u128, state: JobState) -> JobInfo {
        JobInfo {
            id: JobId::from_u128(n),
            kind: JobKind::Analysis,
            label: "index".into(),
            media_id: None,
            state,
            progress: JobProgress::default(),
            cancel_requested: false,
            error: None,
            error_code: None,
        }
    }

    #[test]
    fn keeps_the_last_finished_jobs_newest_first() {
        let h = MemoryHistory::new(3);
        assert!(h.is_empty());
        for n in 1..=5 {
            h.record(&job(n, JobState::Succeeded));
        }
        assert_eq!(h.len(), 3);
        assert!(h.get(JobId::from_u128(2)).is_none());
        assert_eq!(h.get(JobId::from_u128(5)).map(|j| j.id.as_u128()), Some(5));
        let ids: Vec<u128> = h
            .list(&JobFilter::default(), 10)
            .iter()
            .map(|j| j.id.as_u128())
            .collect();
        assert_eq!(ids, [5, 4, 3]);
        assert_eq!(h.list(&JobFilter::default(), 1).len(), 1);
    }

    #[test]
    fn filters_and_rerecording_do_not_duplicate() {
        let h = MemoryHistory::new(10);
        h.record(&job(1, JobState::Failed));
        h.record(&job(2, JobState::Succeeded));
        h.record(&job(1, JobState::Failed));
        assert_eq!(h.len(), 2);
        let failed = JobFilter {
            state: Some(JobState::Failed),
            ..JobFilter::default()
        };
        assert_eq!(h.list(&failed, 10).len(), 1);
    }
}
