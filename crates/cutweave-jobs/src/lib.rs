// SPDX-License-Identifier: GPL-3.0-or-later
//! `cutweave-jobs`: background jobs with progress, cancellation and a decode/export budget
//! (WP-1D.07; arch §7, §14).
//!
//! - [`JobManager`]: submit a job ([`JobSpec`] plus a blocking body), then follow it by
//!   [`JobId`]: [`status`](JobManager::status), [`list`](JobManager::list),
//!   [`cancel`](JobManager::cancel), [`wait`](JobManager::wait) (or
//!   [`wait_async`](JobManager::wait_async)), and the `job.progress` stream
//!   ([`subscribe`](JobManager::subscribe)).
//! - Bodies run on tokio's blocking pool under a per-class [`Budget`] semaphore: at most
//!   cores/2 decode-class jobs ([`JobKind::Analysis`]) and one export at a time. A job may
//!   depend on others ([`JobSpec::after`]) and then runs only if they succeeded.
//! - [`CancelToken`] / [`JobCtx`]: what a body polls and reports progress through.
//! - [`JobHistory`]: where finished jobs go. [`MemoryHistory`] for now; the `jobs` table of
//!   `cache.sqlite` arrives with WP-1D.05.
//! - The engine exposes it through the `job.*` commands of `cutweave-ops` (`job.status`,
//!   `job.list`, `job.wait`, `job.cancel`); the MCP tools `job_status` / `job_wait` are core
//!   (arch §14).
//!
//! Engine-side only (needs threads and tokio); not part of the wasm32-clean crates.

mod cancel;
mod history;
mod id;
mod manager;
mod types;

pub use cancel::CancelToken;
pub use history::{DEFAULT_HISTORY_CAPACITY, JobHistory, MemoryHistory};
pub use id::{JOB_ID_PATTERN, JobId, JobIdError};
pub use manager::{
    DEFAULT_EVENT_CAPACITY, DEFAULT_EXPORT_SLOTS, JobConfig, JobCtx, JobHandle, JobManager,
    JobManagerBuilder, PROGRESS_EVENT_STEP, default_decode_slots,
};
pub use types::{
    Budget, JobError, JobEvent, JobFilter, JobInfo, JobKind, JobProgress, JobSpec, JobState,
};

/// The events this crate broadcasts, as `(name, summary)`: input for the engine's event
/// catalog (WP-1N.01).
pub const EVENTS: &[(&str, &str)] = &[(
    "job.progress",
    "A job was queued, started, progressed, was asked to cancel or finished (the whole job)",
)];
