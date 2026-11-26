// SPDX-License-Identifier: GPL-3.0-or-later
//! The export queue's commands (WP-2G.03a; arch §7, §14, ADR-0030): `export.enqueue`,
//! `export.cancel`, `export.pause_queue`, `export.reorder`, `export.retry`, `export.remove` and
//! the query `export.queue`. `export.start` (in the parent module) is `export.enqueue` plus a
//! resume of a paused queue.
//!
//! Exports run one at a time (the engine's export budget), first in line first, in the
//! background while editing goes on. Each job renders the committed document of the moment it
//! was queued, so later edits never change it. The queue is the engine's
//! [`ExportQueue`](cutweave_jobs::ExportQueue); it lives in the open project
//! (`.cutweave/exports.sqlite`), so queued exports survive an engine restart (one that was
//! running starts again from the beginning). [`PlanCodec`] is how a job is stored: the resolved
//! plan and the document it renders.
//!
//! All of them are engine commands: not undoable, not journaled, in the MCP catalog (reachable
//! through `cutweave_execute` / `cutweave_query`); `export_start` and `job_status` are the core
//! tools. Their state names are `job.status`'s (`queued`, `running`, `succeeded`, `failed`,
//! `cancelled`).

use std::sync::Arc;

use cutweave_commands::{
    Annotations, CmdError, CommandSpec, Described, DocSnapshot, EngineCommand, EntryKind,
    McpExposure, Registrable, Registry, RegistryError, ResourceKind, Rev, Scope, command, wire,
};
use cutweave_jobs::queue::{QueueEntry, QueueError, QueueView, TaskCodec};
use cutweave_jobs::{JobId, JobProgress, JobState};
use cutweave_model::{Duration, FrameRate, Project, SequenceId, Time, migrate_to_current};
use serde::{Deserialize, Serialize};

use super::presets::{PlanRate, PresetWarning};
use super::{
    Collision, ExportCtx, ExportHw, ExportPlan, ExportPreset, ExportQuality, ExportRange,
    ExportRequest, ExportStartOutput, queue_request,
};

/// Registers the queue commands.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    registry
        .register::<ExportEnqueue>()?
        .register::<ExportCancel>()?
        .register::<ExportPauseQueue>()?
        .register::<ExportReorder>()?
        .register::<ExportRetry>()?
        .register::<ExportRemove>()?
        .register::<ExportQueue>()?;
    Ok(())
}

// ----- errors ----------------------------------------------------------------------------------

/// A queue refusal as a command error; `pointer` names the param it is about.
#[must_use]
pub fn queue_error(e: QueueError, pointer: &str) -> CmdError {
    match e {
        QueueError::NotFound(id) => CmdError::not_found(ResourceKind::Job, id),
        // Retrying is about the job; queueing is about the path.
        QueueError::OutputInUse(_) if pointer == "/id" => CmdError::invalid(pointer, e.to_string()),
        QueueError::OutputInUse(_) => CmdError::invalid("/path", e.to_string()),
        QueueError::Invalid(msg) => CmdError::invalid(pointer, msg),
        QueueError::Store(msg) => {
            CmdError::internal(format!("the export queue could not be saved: {msg}"))
        }
        QueueError::ShutDown => CmdError::unsupported("the engine is shutting down"),
        other => CmdError::internal(other.to_string()),
    }
}

// ----- output ----------------------------------------------------------------------------------

/// One export of the queue.
#[wire]
pub struct ExportQueueJob {
    /// The job (`job.status`, `job.wait`, `export.cancel`, …).
    pub id: JobId,
    /// `queued`, `running`, `succeeded`, `failed` or `cancelled`.
    pub state: JobState,
    /// Done fraction, frames, speed, time left and bytes written.
    pub progress: JobProgress,
    /// Whether a cancel was asked and the export has not stopped yet.
    pub cancel_requested: bool,
    /// Why it failed or was cancelled.
    pub error: Option<String>,
    /// The output file.
    pub path: String,
    /// The exported sequence.
    pub sequence: SequenceId,
    /// Range start (sequence time, flicks).
    #[serde(rename = "in")]
    pub start: Time,
    /// Range end, exclusive.
    #[serde(rename = "out")]
    pub end: Time,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Frame rate of the file.
    pub frame_rate: FrameRate,
    /// Video frames the file will hold.
    pub frames: u32,
    /// Length of the file.
    pub duration: Duration,
    /// The document revision it renders (later edits are not in the file).
    pub rev: Rev,
}

/// The export queue: result of `export.queue` and the queue commands.
#[wire]
pub struct ExportQueueOutput {
    /// Whether the queue is paused: no export starts (a running one finishes).
    pub paused: bool,
    /// How many exports run at once.
    pub parallel: u32,
    /// The exports, first in line first; finished ones stay until `export.remove`.
    pub jobs: Vec<ExportQueueJob>,
}

/// One entry of the queue as the wire sees it.
#[must_use]
pub fn queue_job(entry: &QueueEntry<ExportPlan>) -> ExportQueueJob {
    let plan = &entry.task;
    ExportQueueJob {
        id: entry.info.id,
        state: entry.info.state,
        progress: entry.info.progress.clone(),
        cancel_requested: entry.info.cancel_requested,
        error: entry.info.error.clone(),
        path: plan.path.to_string_lossy().into_owned(),
        sequence: plan.sequence,
        start: plan.start,
        end: plan.end,
        width: plan.width,
        height: plan.height,
        frame_rate: plan.frame_rate,
        frames: plan.frames,
        duration: plan.duration,
        rev: plan.snapshot.rev,
    }
}

/// The queue as the wire sees it.
#[must_use]
pub fn queue_output(view: &QueueView<ExportPlan>) -> ExportQueueOutput {
    ExportQueueOutput {
        paused: view.paused,
        parallel: u32::try_from(view.slots).unwrap_or(u32::MAX),
        jobs: view.entries.iter().map(queue_job).collect(),
    }
}

// ----- export.enqueue --------------------------------------------------------------------------

/// Adds an export of a sequence (or a range of it) to an MP4 file (H.264 + AAC) to the end of
/// the export queue and returns its job. It runs when its turn comes; while the queue is paused
/// it waits (`export.start` adds and resumes). It renders the project as it is now; later edits
/// do not change it. The same params and path rules as `export.start`.
#[command(
    name = "export.enqueue",
    summary = "Add an export to the export queue (waits while the queue is paused); returns job_id",
    engine,
    undoable = false,
    journaled = false,
    mcp = "catalog",
    annotations(destructive = true, idempotent = false)
)]
pub struct ExportEnqueue {
    /// Sequence; optional when the project has one.
    pub sequence: Option<SequenceId>,
    /// Absolute path of the output file (`.mp4`).
    pub path: String,
    /// Part of the sequence to export (default: all of it): {kind: "sequence" | "in_out" |
    /// "custom" | "markers", in, out, markers: [ids], pair_mode: "duration" | "pairs"}, as in
    /// export.start.
    pub range: Option<ExportRange>,
    /// "h264-mp4-1080p" (default), a preset id or {"custom": {...}}, as in export.start.
    pub preset: Option<ExportPreset>,
    /// Hardware encoder: "auto" (default), "off", or {"force": "h264_nvenc"}.
    pub hw: Option<ExportHw>,
    /// Replace an existing file at `path` (default false); same as `collision: "overwrite"`.
    pub overwrite: Option<bool>,
    /// An existing file: "fail" (default), "rename" or "overwrite".
    pub collision: Option<Collision>,
}

impl EngineCommand for ExportEnqueue {
    type Context = ExportCtx;
    type Output = ExportStartOutput;

    fn run(&self, ctx: &mut ExportCtx) -> Result<ExportStartOutput, CmdError> {
        let request = ExportRequest {
            sequence: self.sequence,
            path: &self.path,
            range: self.range.as_ref(),
            preset: self.preset.as_ref(),
            hw: self.hw.as_ref(),
            overwrite: self.overwrite,
            collision: self.collision,
        };
        queue_request(&request, ctx, false)
    }
}

// ----- the queue commands ----------------------------------------------------------------------

/// Cancels a queued or running export: a queued one ends at once, a running one stops within a
/// second and its partial file is deleted; the next export in line then starts. Cancelling a
/// finished export changes nothing. Returns the export.
#[command(
    name = "export.cancel",
    summary = "Cancel a queued or running export; the queue goes on with the next one",
    engine,
    undoable = false,
    journaled = false,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct ExportCancel {
    /// The export's job (from export.start, export.enqueue or export.queue).
    pub id: JobId,
}

impl EngineCommand for ExportCancel {
    type Context = ExportCtx;
    type Output = ExportQueueJob;

    fn run(&self, ctx: &mut ExportCtx) -> Result<ExportQueueJob, CmdError> {
        let queue = ctx.starter.queue()?;
        queue.cancel(self.id).map_err(|e| queue_error(e, "/id"))?;
        let entry = queue.entry(self.id).map_err(|e| queue_error(e, "/id"))?;
        Ok(queue_job(&entry))
    }
}

/// Pauses the export queue (no export starts; a running one finishes) or resumes it. The pause
/// is kept with the project. Returns the queue.
#[command(
    name = "export.pause_queue",
    summary = "Pause or resume the export queue (a running export finishes)",
    engine,
    undoable = false,
    journaled = false,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct ExportPauseQueue {
    /// true pauses, false resumes.
    pub paused: bool,
}

impl EngineCommand for ExportPauseQueue {
    type Context = ExportCtx;
    type Output = ExportQueueOutput;

    fn run(&self, ctx: &mut ExportCtx) -> Result<ExportQueueOutput, CmdError> {
        let queue = ctx.starter.queue()?;
        queue
            .pause(self.paused)
            .map_err(|e| queue_error(e, "/paused"))?;
        Ok(queue_output(&queue.view()))
    }
}

/// Moves a queued export (one waiting for its turn) before another export of the queue, or to
/// the end. Returns the queue.
#[command(
    name = "export.reorder",
    summary = "Move a queued export before another one, or to the end of the queue",
    engine,
    undoable = false,
    journaled = false,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct ExportReorder {
    /// The export to move (it must be waiting for its turn).
    pub id: JobId,
    /// The export to put it before; absent: the end of the queue.
    pub before: Option<JobId>,
}

impl EngineCommand for ExportReorder {
    type Context = ExportCtx;
    type Output = ExportQueueOutput;

    fn run(&self, ctx: &mut ExportCtx) -> Result<ExportQueueOutput, CmdError> {
        let queue = ctx.starter.queue()?;
        queue.reorder(self.id, self.before).map_err(|e| match e {
            QueueError::NotFound(id) if Some(id) == self.before => queue_error(e, "/before"),
            QueueError::Invalid(_) if self.before == Some(self.id) => queue_error(e, "/before"),
            e => queue_error(e, "/id"),
        })?;
        Ok(queue_output(&queue.view()))
    }
}

/// Queues a failed or cancelled export again, in its place and with its job id, rendering the
/// same snapshot as before. Returns the export.
#[command(
    name = "export.retry",
    summary = "Queue a failed or cancelled export again (same job id, same snapshot)",
    engine,
    undoable = false,
    journaled = false,
    mcp = "catalog",
    annotations(destructive = true, idempotent = false)
)]
pub struct ExportRetry {
    /// The failed or cancelled export.
    pub id: JobId,
}

impl EngineCommand for ExportRetry {
    type Context = ExportCtx;
    type Output = ExportQueueJob;

    fn run(&self, ctx: &mut ExportCtx) -> Result<ExportQueueJob, CmdError> {
        let queue = ctx.starter.queue()?;
        queue.retry(self.id).map_err(|e| queue_error(e, "/id"))?;
        let entry = queue.entry(self.id).map_err(|e| queue_error(e, "/id"))?;
        Ok(queue_job(&entry))
    }
}

/// Takes an export off the queue: a finished one is forgotten (its file stays), a queued one is
/// cancelled first; a running one must be cancelled first. Returns the queue.
#[command(
    name = "export.remove",
    summary = "Take a finished or queued export off the queue (its file stays)",
    engine,
    undoable = false,
    journaled = false,
    mcp = "catalog",
    annotations(destructive = false, idempotent = false)
)]
pub struct ExportRemove {
    /// The export.
    pub id: JobId,
}

impl EngineCommand for ExportRemove {
    type Context = ExportCtx;
    type Output = ExportQueueOutput;

    fn run(&self, ctx: &mut ExportCtx) -> Result<ExportQueueOutput, CmdError> {
        let queue = ctx.starter.queue()?;
        queue.remove(self.id).map_err(|e| queue_error(e, "/id"))?;
        Ok(queue_output(&queue.view()))
    }
}

/// Lists the export queue in order: each export's state, progress (fraction, frames, fps, time
/// left, bytes), output file and what it renders, and whether the queue is paused.
#[wire]
pub struct ExportQueue {}

impl Described for ExportQueue {
    const SPEC: CommandSpec = CommandSpec {
        name: "export.queue",
        summary: "List the export queue: each export's state, progress and output file",
        kind: EntryKind::Engine,
        undoable: false,
        view: false,
        scope: Scope::Any,
        mcp: McpExposure::Catalog,
        annotations: Annotations {
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        secret_fields: &[],
        rust_type: "ExportQueue",
    };
}

impl Registrable for ExportQueue {
    fn register_into(registry: &mut Registry) -> Result<(), RegistryError> {
        registry.add_engine_command::<ExportQueue>().map(drop)
    }
}

impl EngineCommand for ExportQueue {
    type Context = ExportCtx;
    type Output = ExportQueueOutput;

    fn run(&self, ctx: &mut ExportCtx) -> Result<ExportQueueOutput, CmdError> {
        Ok(queue_output(&ctx.starter.queue()?.view()))
    }
}

// ----- storing a job ---------------------------------------------------------------------------

/// The version of [`StoredPlan`]; a payload of another version is not read.
pub const STORED_PLAN_VERSION: u32 = 1;

/// An export job as `exports.sqlite` keeps it: the resolved plan and the committed document it
/// renders (in the project format, migrated on load).
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredPlan {
    version: u32,
    rev: Rev,
    committed_rev: Rev,
    doc: serde_json::Value,
    sequence: SequenceId,
    path: String,
    overwrite: bool,
    start: Time,
    end: Time,
    width: u32,
    height: u32,
    frame_rate: FrameRate,
    frames: u32,
    duration: Duration,
    quality: ExportQuality,
    hw: ExportHw,
    rate: Option<PlanRate>,
    warnings: Vec<PresetWarning>,
}

/// Encodes export jobs for the queue's store (JSON).
#[derive(Clone, Copy, Debug, Default)]
pub struct PlanCodec;

impl TaskCodec<ExportPlan> for PlanCodec {
    fn encode(&self, plan: &ExportPlan) -> Result<Vec<u8>, String> {
        let doc = serde_json::to_value(&plan.snapshot.doc).map_err(|e| e.to_string())?;
        let stored = StoredPlan {
            version: STORED_PLAN_VERSION,
            rev: plan.snapshot.rev,
            committed_rev: plan.snapshot.committed_rev,
            doc,
            sequence: plan.sequence,
            path: plan.path.to_string_lossy().into_owned(),
            overwrite: plan.overwrite,
            start: plan.start,
            end: plan.end,
            width: plan.width,
            height: plan.height,
            frame_rate: plan.frame_rate,
            frames: plan.frames,
            duration: plan.duration,
            quality: plan.quality,
            hw: plan.hw.clone(),
            rate: plan.rate,
            warnings: plan.warnings.clone(),
        };
        serde_json::to_vec(&stored).map_err(|e| e.to_string())
    }

    fn decode(&self, bytes: &[u8]) -> Result<ExportPlan, String> {
        let stored: StoredPlan =
            serde_json::from_slice(bytes).map_err(|e| format!("not an export job: {e}"))?;
        if stored.version != STORED_PLAN_VERSION {
            return Err(format!(
                "an export job of version {} (this build reads {STORED_PLAN_VERSION})",
                stored.version
            ));
        }
        let (doc, _): (Project, _) =
            migrate_to_current(stored.doc).map_err(|e| format!("its document: {e}"))?;
        Ok(ExportPlan {
            snapshot: Arc::new(DocSnapshot {
                rev: stored.rev,
                committed_rev: stored.committed_rev,
                doc,
            }),
            sequence: stored.sequence,
            path: stored.path.into(),
            overwrite: stored.overwrite,
            start: stored.start,
            end: stored.end,
            width: stored.width,
            height: stored.height,
            frame_rate: stored.frame_rate,
            frames: stored.frames,
            duration: stored.duration,
            quality: stored.quality,
            hw: stored.hw,
            rate: stored.rate,
            warnings: stored.warnings,
        })
    }
}
