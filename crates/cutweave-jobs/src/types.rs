// SPDX-License-Identifier: GPL-3.0-or-later
//! What a job is and what the wire sees of it: [`JobKind`], [`Budget`], [`JobState`],
//! [`JobProgress`], [`JobInfo`], [`JobSpec`], [`JobFilter`], [`JobError`] and [`JobEvent`].

use cutweave_model::MediaId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::id::JobId;

/// What kind of work a job does. The kind decides which [`Budget`] it runs under.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, specta::Type,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum JobKind {
    /// Background analysis of an imported asset (index, thumbnails, waveform): decodes media.
    Analysis,
    /// Rendering and encoding a sequence to a file (WP-1E.03).
    Export,
    /// Making a media asset's proxy (WP-2F.01): decodes the original and encodes a smaller
    /// intra-frame copy.
    Proxy,
    /// Copying a project's files into another folder (WP-2F.04): disk work, no decoding.
    Collect,
}

impl JobKind {
    /// The budget jobs of this kind run under.
    #[must_use]
    pub const fn budget(self) -> Budget {
        match self {
            // File copies share the slots of the other disk-heavy background work.
            JobKind::Analysis | JobKind::Collect => Budget::Decode,
            JobKind::Export => Budget::Export,
            JobKind::Proxy => Budget::Proxy,
        }
    }
}

/// A concurrency budget: at most this many jobs of the class run at once (the GPU/IO budget,
/// WP-1D.07). Queued jobs wait for a slot without holding a thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Budget {
    /// Decode-heavy work: by default half the CPU cores (at least one).
    Decode,
    /// Exports: one at a time.
    Export,
    /// Proxy generation (WP-2F.01): two at a time by default, and at most one while an export
    /// runs.
    Proxy,
}

/// Where a job is in its lifecycle. `succeeded`, `failed` and `cancelled` are final.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, specta::Type,
)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    /// Waiting for the jobs it depends on or for a budget slot.
    Queued,
    /// Running.
    Running,
    /// Finished its work.
    Succeeded,
    /// Stopped with an error (see `error`), or skipped because a job it needs failed.
    Failed,
    /// Cancelled before or while running.
    Cancelled,
}

impl JobState {
    /// Whether the state is final.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            JobState::Succeeded | JobState::Failed | JobState::Cancelled
        )
    }
}

/// How far a job got. The counters are reported by jobs that have them (exports: frames,
/// speed, time left and bytes written, WP-2G.03a) and left out otherwise.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct JobProgress {
    /// Done fraction, `0..=1` (1 once the job succeeded).
    pub fraction: f64,
    /// What it is doing, for status lines.
    pub message: Option<String>,
    /// Units done so far (an export: frames handed to the encoder).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[specta(type = Option<specta_typescript::Number>)]
    pub frame: Option<u64>,
    /// Units in all (an export: frames in the file).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[specta(type = Option<specta_typescript::Number>)]
    pub frames: Option<u64>,
    /// Speed in units per second.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<f64>,
    /// Estimated time left, in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[specta(type = Option<specta_typescript::Number>)]
    pub eta_ms: Option<u64>,
    /// Bytes written so far.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[specta(type = Option<specta_typescript::Number>)]
    pub bytes: Option<u64>,
}

impl JobProgress {
    /// Progress with a fraction and a status line, without counters.
    #[must_use]
    pub fn new(fraction: f64, message: Option<String>) -> JobProgress {
        JobProgress {
            fraction,
            message,
            ..JobProgress::default()
        }
    }
}

/// A job as reported by `job.status`, `job.list`, `job.wait`, `job.cancel` and the
/// `job.progress` event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct JobInfo {
    /// The job.
    pub id: JobId,
    /// What kind of work it is.
    pub kind: JobKind,
    /// What it does, e.g. the analysis stage (`index`, `thumbnails`, `waveform`).
    pub label: String,
    /// The media asset it works on, if any.
    pub media_id: Option<MediaId>,
    /// Where it is in its lifecycle.
    pub state: JobState,
    /// How far it got.
    pub progress: JobProgress,
    /// Whether cancellation was requested; a running job stops when it next checks.
    pub cancel_requested: bool,
    /// Why it failed, was skipped or was cancelled.
    pub error: Option<String>,
    /// What kind of failure it was, for programs: a short stable code in lower snake case
    /// ([`error_code`]; `failed` when nothing more specific is known). Only a failed job has
    /// one; a cancelled job and a job stored before codes existed have none (ADR-0038).
    pub error_code: Option<String>,
}

/// The machine codes of failed jobs ([`JobInfo::error_code`], ADR-0038): lower snake case, few
/// and stable. Export names the specific ones; every other failure is
/// [`FAILED`](error_code::FAILED).
pub mod error_code {
    /// The general code: the job failed and nothing more specific is known.
    pub const FAILED: &str = "failed";
    /// The drive ran out of space while the output was written.
    pub const DISK_FULL: &str = "disk_full";
    /// The output file appeared after the job was queued and it may not replace it.
    pub const OUTPUT_EXISTS: &str = "output_exists";
    /// The output file could not be created, written or moved into place.
    pub const OUTPUT_UNWRITABLE: &str = "output_unwritable";
    /// Media the job needs is offline.
    pub const MEDIA_OFFLINE: &str = "media_offline";
    /// The encoder could not be opened or failed.
    pub const ENCODER_FAILED: &str = "encoder_failed";
    /// A frame could not be rendered.
    pub const RENDER_FAILED: &str = "render_failed";
    /// The engine part the job needs (media backend, renderer) is not running.
    pub const ENGINE_UNAVAILABLE: &str = "engine_unavailable";
}

/// What to run: given to [`JobManager::submit`](crate::JobManager::submit) with the job's body.
#[derive(Clone, Debug, PartialEq)]
pub struct JobSpec {
    /// What kind of work it is (decides the budget).
    pub kind: JobKind,
    /// A short label (`index`, `thumbnails`, `export`).
    pub label: String,
    /// The media asset it works on, if any.
    pub media_id: Option<MediaId>,
    /// Jobs that must succeed first. The job stays queued (without a budget slot) until they
    /// finish; if one fails it fails too ("skipped"), if one is cancelled it is cancelled.
    pub after: Vec<JobId>,
}

impl JobSpec {
    /// A job of `kind` labelled `label`, with no media and no dependencies.
    #[must_use]
    pub fn new(kind: JobKind, label: impl Into<String>) -> JobSpec {
        JobSpec {
            kind,
            label: label.into(),
            media_id: None,
            after: Vec::new(),
        }
    }

    /// The media asset it works on.
    #[must_use]
    pub fn media(mut self, media_id: MediaId) -> JobSpec {
        self.media_id = Some(media_id);
        self
    }

    /// Runs only after `jobs` succeeded.
    #[must_use]
    pub fn after(mut self, jobs: impl IntoIterator<Item = JobId>) -> JobSpec {
        self.after.extend(jobs);
        self
    }
}

/// Which jobs `job.list` returns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JobFilter {
    /// Only jobs on this asset.
    pub media_id: Option<MediaId>,
    /// Only jobs in this state.
    pub state: Option<JobState>,
    /// Only jobs of this kind.
    pub kind: Option<JobKind>,
}

impl JobFilter {
    /// Whether `job` passes the filter.
    #[must_use]
    pub fn matches(&self, job: &JobInfo) -> bool {
        self.media_id.is_none_or(|m| job.media_id == Some(m))
            && self.state.is_none_or(|s| job.state == s)
            && self.kind.is_none_or(|k| job.kind == k)
    }
}

/// How a job body ends when it does not succeed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum JobError {
    /// It noticed its [`CancelToken`](crate::CancelToken) and stopped.
    #[error("cancelled")]
    Cancelled,
    /// It failed.
    #[error("{message}")]
    Failed {
        /// Why, for people.
        message: String,
        /// What kind of failure, for programs ([`error_code`]).
        code: String,
    },
}

impl JobError {
    /// A failure with a message and the general code, [`error_code::FAILED`].
    pub fn failed(msg: impl Into<String>) -> JobError {
        JobError::coded(error_code::FAILED, msg)
    }

    /// A failure with a message and its machine code (one of [`error_code`]).
    pub fn coded(code: impl Into<String>, msg: impl Into<String>) -> JobError {
        JobError::Failed {
            message: msg.into(),
            code: code.into(),
        }
    }
}

/// Everything the job manager broadcasts to subscribers
/// ([`JobManager::subscribe`](crate::JobManager::subscribe)).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event")]
#[non_exhaustive]
pub enum JobEvent {
    /// `job.progress`: a job was queued, started, progressed, was asked to cancel or finished.
    /// Carries the whole [`JobInfo`]; after `Lagged`, re-read `job.list`.
    #[serde(rename = "job.progress")]
    Progress(JobInfo),
}

impl JobEvent {
    /// The job the event is about.
    #[must_use]
    pub fn job(&self) -> &JobInfo {
        match self {
            JobEvent::Progress(info) => info,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use serde_json::json;

    use super::*;

    fn info() -> JobInfo {
        JobInfo {
            id: JobId::from_u128(1),
            kind: JobKind::Analysis,
            label: "index".into(),
            media_id: Some(MediaId::from_u128(2)),
            state: JobState::Running,
            progress: JobProgress::new(0.5, Some("half".into())),
            cancel_requested: false,
            error: None,
            error_code: None,
        }
    }

    #[test]
    fn a_job_written_before_error_codes_still_reads() {
        let old = json!({
            "id": "job_00000000000000000000000000000001",
            "kind": "export",
            "label": "export",
            "media_id": null,
            "state": "failed",
            "progress": {"fraction": 0.0, "message": null},
            "cancel_requested": false,
            "error": "disk full"
        });
        let job: JobInfo = serde_json::from_value(old).unwrap();
        assert_eq!(job.error.as_deref(), Some("disk full"));
        assert_eq!(job.error_code, None);
    }

    #[test]
    fn failures_carry_a_code_and_plain_ones_the_general_code() {
        let plain = JobError::failed("boom");
        assert_eq!(plain, JobError::coded(error_code::FAILED, "boom"));
        assert_eq!(plain.to_string(), "boom");
        let full = JobError::coded(error_code::DISK_FULL, "no space");
        assert!(matches!(&full, JobError::Failed { code, .. } if code == "disk_full"));
        assert_eq!(full.to_string(), "no space");
    }

    #[test]
    fn progress_event_has_its_wire_name_and_round_trips() {
        let e = JobEvent::Progress(info());
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(
            v,
            json!({
                "event": "job.progress",
                "id": "job_00000000000000000000000000000001",
                "kind": "analysis",
                "label": "index",
                "media_id": "med_00000000000000000000000000000002",
                "state": "running",
                "progress": {"fraction": 0.5, "message": "half"},
                "cancel_requested": false,
                "error": null,
                "error_code": null
            })
        );
        assert_eq!(serde_json::from_value::<JobEvent>(v).unwrap(), e);
        assert_eq!(e.job().label, "index");
    }

    #[test]
    fn terminal_states_and_budgets() {
        assert!(!JobState::Queued.is_terminal());
        assert!(!JobState::Running.is_terminal());
        assert!(JobState::Succeeded.is_terminal());
        assert!(JobState::Failed.is_terminal());
        assert!(JobState::Cancelled.is_terminal());
        assert_eq!(JobKind::Analysis.budget(), Budget::Decode);
        assert_eq!(JobKind::Export.budget(), Budget::Export);
        assert_eq!(JobKind::Proxy.budget(), Budget::Proxy);
        assert_eq!(
            serde_json::to_value(JobKind::Proxy).unwrap(),
            json!("proxy")
        );
    }

    #[test]
    fn filters_combine() {
        let job = info();
        assert!(JobFilter::default().matches(&job));
        let on = JobFilter {
            media_id: Some(MediaId::from_u128(2)),
            state: Some(JobState::Running),
            kind: Some(JobKind::Analysis),
        };
        assert!(on.matches(&job));
        assert!(
            !JobFilter {
                media_id: Some(MediaId::from_u128(3)),
                ..on
            }
            .matches(&job)
        );
        assert!(
            !JobFilter {
                state: Some(JobState::Queued),
                ..on
            }
            .matches(&job)
        );
    }
}
