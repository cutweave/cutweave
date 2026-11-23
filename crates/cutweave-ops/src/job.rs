// SPDX-License-Identifier: GPL-3.0-or-later
//! The `job` namespace (WP-1D.07; arch §5.1, §7, §14): `job.status`, `job.list`, `job.wait`
//! and `job.cancel` over the engine's [`JobManager`].
//!
//! Jobs are engine state, not document state, so these are engine commands whose `Context` is
//! the [`JobManager`]. The three readers are read-only engine commands (`readOnlyHint`,
//! idempotent, never journaled), like `edit.history`; `job.status` and `job.wait` are MCP core
//! tools (arch §14, R21). `job.cancel` changes only job state, which is not persisted: it is
//! not undoable and is declared `journaled = false`, so it is never journaled or replayed.
//!
//! `job.wait` blocks its caller for up to [`MAX_WAIT_MS`]: the engine must dispatch it off
//! async executor threads (or call [`JobManager::wait_async`] directly).

use std::time::Duration;

use cutweave_commands::{
    Annotations, CmdError, CommandSpec, Described, EngineCommand, EntryKind, McpExposure,
    Registrable, Registry, RegistryError, ResourceKind, Scope, command, wire,
};
use cutweave_jobs::{JobFilter, JobId, JobInfo, JobKind, JobManager, JobState};
use cutweave_model::MediaId;

/// `job.wait` timeout when none is given.
pub const DEFAULT_WAIT_MS: u32 = 30_000;
/// Longest `job.wait` timeout; longer ones are capped to it.
pub const MAX_WAIT_MS: u32 = 60_000;
/// Default page size of `job.list`.
pub const DEFAULT_JOB_LIST_LIMIT: u32 = 50;
/// Largest page size of `job.list`.
pub const MAX_JOB_LIST_LIMIT: u32 = 500;

/// Registers the `job.*` commands.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    registry
        .register::<JobStatus>()?
        .register::<JobList>()?
        .register::<JobWait>()?
        .register::<JobCancel>()?;
    Ok(())
}

/// The spec of a read-only engine command (the `#[command]` macro cannot declare one).
const fn reader(
    name: &'static str,
    summary: &'static str,
    mcp: McpExposure,
    rust_type: &'static str,
) -> CommandSpec {
    CommandSpec {
        name,
        summary,
        kind: EntryKind::Engine,
        undoable: false,
        view: false,
        scope: Scope::Any,
        mcp,
        annotations: Annotations {
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        secret_fields: &[],
        rust_type,
    }
}

fn unknown(id: JobId) -> CmdError {
    CmdError::not_found(ResourceKind::Job, id)
}

// ----- job.status ----------------------------------------------------------------------------

/// Gets a background job's state (`queued`, `running`, `succeeded`, `failed`, `cancelled`),
/// progress and error. Finished jobs are remembered for a while (the last few hundred), then
/// `NotFound`.
#[wire]
pub struct JobStatus {
    /// The job (from `media.import`'s `job_ids`, `export.start`, …).
    pub id: JobId,
}

impl Described for JobStatus {
    const SPEC: CommandSpec = reader(
        "job.status",
        "Get a background job's state, progress and error (analysis, export)",
        McpExposure::Core,
        "JobStatus",
    );
}

impl Registrable for JobStatus {
    fn register_into(registry: &mut Registry) -> Result<(), RegistryError> {
        registry.add_engine_command::<JobStatus>().map(drop)
    }
}

impl EngineCommand for JobStatus {
    type Context = JobManager;
    type Output = JobInfo;

    fn run(&self, jobs: &mut JobManager) -> Result<JobInfo, CmdError> {
        jobs.status(self.id).ok_or_else(|| unknown(self.id))
    }
}

// ----- job.list ------------------------------------------------------------------------------

/// Lists background jobs, newest first: unfinished ones and recently finished ones, optionally
/// only those of one asset, state or kind.
#[wire]
pub struct JobList {
    /// Only jobs working on this media asset.
    #[serde(default)]
    pub media_id: Option<MediaId>,
    /// Only jobs in this state (e.g. `running`).
    #[serde(default)]
    pub state: Option<JobState>,
    /// Only jobs of this kind.
    #[serde(default)]
    pub kind: Option<JobKind>,
    /// Most jobs returned, `1..=500` (default 50).
    #[serde(default)]
    pub limit: Option<u32>,
}

/// Result of `job.list`.
#[wire]
pub struct JobListOutput {
    /// The jobs, newest first.
    pub jobs: Vec<JobInfo>,
    /// How many jobs are unfinished (queued or running) in total, whatever the filter.
    pub active: u32,
}

impl Described for JobList {
    const SPEC: CommandSpec = reader(
        "job.list",
        "List background jobs, newest first (filter by media, state or kind)",
        McpExposure::Catalog,
        "JobList",
    );
}

impl Registrable for JobList {
    fn register_into(registry: &mut Registry) -> Result<(), RegistryError> {
        registry.add_engine_command::<JobList>().map(drop)
    }
}

impl EngineCommand for JobList {
    type Context = JobManager;
    type Output = JobListOutput;

    fn run(&self, jobs: &mut JobManager) -> Result<JobListOutput, CmdError> {
        let limit = self.limit.unwrap_or(DEFAULT_JOB_LIST_LIMIT);
        if !(1..=MAX_JOB_LIST_LIMIT).contains(&limit) {
            return Err(CmdError::invalid(
                "/limit",
                format!("the limit must be 1..={MAX_JOB_LIST_LIMIT}"),
            ));
        }
        let filter = JobFilter {
            media_id: self.media_id,
            state: self.state,
            kind: self.kind,
        };
        Ok(JobListOutput {
            jobs: jobs.list(&filter, limit as usize),
            active: u32::try_from(jobs.active()).unwrap_or(u32::MAX),
        })
    }
}

// ----- job.wait ------------------------------------------------------------------------------

/// Waits for a background job to finish, at most `timeout_ms` (default 30 s, capped at 60 s),
/// and returns its state: at once for a finished job, as soon as it finishes, or still
/// `queued`/`running` with its progress when the time is up (call again to keep waiting).
#[wire]
pub struct JobWait {
    /// The job.
    pub job_id: JobId,
    /// How long to wait in milliseconds (default 30000; values above 60000 are capped; 0
    /// returns the current state).
    #[serde(default)]
    pub timeout_ms: Option<u32>,
}

impl Described for JobWait {
    const SPEC: CommandSpec = reader(
        "job.wait",
        "Wait up to timeout_ms (max 60 s) for a background job to finish; returns its state",
        McpExposure::Core,
        "JobWait",
    );
}

impl Registrable for JobWait {
    fn register_into(registry: &mut Registry) -> Result<(), RegistryError> {
        registry.add_engine_command::<JobWait>().map(drop)
    }
}

impl JobWait {
    /// The time this call waits at most.
    #[must_use]
    pub fn timeout(&self) -> Duration {
        let ms = self.timeout_ms.unwrap_or(DEFAULT_WAIT_MS).min(MAX_WAIT_MS);
        Duration::from_millis(u64::from(ms))
    }
}

impl EngineCommand for JobWait {
    type Context = JobManager;
    type Output = JobInfo;

    fn run(&self, jobs: &mut JobManager) -> Result<JobInfo, CmdError> {
        jobs.wait(self.job_id, self.timeout())
            .ok_or_else(|| unknown(self.job_id))
    }
}

// ----- job.cancel ----------------------------------------------------------------------------

/// Cancels a background job. A queued job is cancelled at once; a running one stops within
/// 500 ms (`running` with `cancel_requested` until then; `job.wait` to see it end). Cancelling
/// a finished job changes nothing. Returns the job's state.
#[command(
    name = "job.cancel",
    summary = "Cancel a background job (stops within 500 ms; no effect once it finished)",
    engine,
    undoable = false,
    journaled = false,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct JobCancel {
    /// The job.
    pub id: JobId,
}

impl EngineCommand for JobCancel {
    type Context = JobManager;
    type Output = JobInfo;

    fn run(&self, jobs: &mut JobManager) -> Result<JobInfo, CmdError> {
        jobs.cancel(self.id).ok_or_else(|| unknown(self.id))
    }
}
