// SPDX-License-Identifier: GPL-3.0-or-later
//! Background analysis of imported media (WP-1D.07; arch §7): after `media.import` registers
//! an asset, one job per applicable [`AnalysisStage`] runs on the [`JobManager`] (index →
//! thumbnails → waveform as WP-1D.02/1D.05/1D.06 land; none are registered before that).
//!
//! - **Analysis never mutates the document.** Its results live in the global caches outside
//!   the project (arch §7, §17), and its status is job state: a stage that fails is a failed
//!   job (`job.status`, `job.list{media_id}` give the stage label and error) and is announced
//!   as `media.analysis_failed` ([`MediaEvent::AnalysisFailed`]) so the UI can badge the asset
//!   live. After an engine restart analysis simply runs again (cheap once the caches exist).
//! - A stage that [builds the index](AnalysisStage::builds_index) announces `media.indexed`
//!   ([`MediaEvent::Indexed`]) when it succeeds.
//! - A stage may [require](AnalysisStage::requires) earlier stages: its job waits for theirs
//!   (without holding a budget slot) and is skipped if one fails.
//! - Events reach subscribers through the engine's [`AnalysisSink`].

use std::{
    collections::BTreeMap,
    panic::{self, AssertUnwindSafe},
    sync::Arc,
};

use cutweave_commands::wire;
use cutweave_jobs::{JobCtx, JobError, JobId, JobKind, JobManager, JobSpec};
use cutweave_model::{MediaId, MediaKind, ProbeInfo};
use serde::{Deserialize, Serialize};

/// The events of the `media` namespace, as `(name, summary)`: input for the engine's event
/// catalog (WP-1N.01).
pub const EVENTS: &[(&str, &str)] = &[
    (
        "media.indexed",
        "A media asset's frame index is ready (frame-exact seeking and decoding can use it)",
    ),
    (
        "media.analysis_failed",
        "A background analysis stage failed on a media asset (it stays usable; previews may be missing)",
    ),
    (
        "media.import_progress",
        "A media import is probing its files: how many are done of how many",
    ),
];

// ----- events --------------------------------------------------------------------------------

/// `media.indexed`: the asset's frame index is ready.
#[wire]
#[derive(Copy)]
pub struct MediaIndexed {
    /// The asset.
    pub media_id: MediaId,
}

/// `media.analysis_failed`: a background analysis stage failed on an asset. The asset stays
/// usable; what the stage produces (thumbnails, waveform, …) may be missing.
#[wire]
pub struct MediaAnalysisFailed {
    /// The asset.
    pub media_id: MediaId,
    /// The stage (`index`, `thumbnails`, `waveform`): the job's label.
    pub stage: String,
    /// The failed job (`job.status` shows it).
    pub job_id: JobId,
    /// What went wrong (the job's error).
    pub error: String,
}

/// `media.import_progress`: a `media.import` of several files (a folder, WP-2F.09) has probed
/// `done` of its `total` files. The call itself answers when all are done.
#[wire]
#[derive(Copy)]
pub struct MediaImportProgress {
    /// Files probed so far.
    pub done: u32,
    /// Files to probe in this call.
    pub total: u32,
}

/// The events of the `media` namespace (see [`EVENTS`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event")]
#[non_exhaustive]
pub enum MediaEvent {
    /// `media.indexed`.
    #[serde(rename = "media.indexed")]
    Indexed(MediaIndexed),
    /// `media.analysis_failed`.
    #[serde(rename = "media.analysis_failed")]
    AnalysisFailed(MediaAnalysisFailed),
    /// `media.import_progress`.
    #[serde(rename = "media.import_progress")]
    ImportProgress(MediaImportProgress),
}

// ----- stages --------------------------------------------------------------------------------

/// What an analysis stage gets about the asset it analyzes.
#[derive(Clone, Debug, PartialEq)]
pub struct AnalysisInput {
    /// The registered asset.
    pub media_id: MediaId,
    /// What it is.
    pub kind: MediaKind,
    /// Its absolute path.
    pub path: String,
    /// Its content hash (cache key).
    pub content_hash: String,
    /// What the import probe found.
    pub probe: ProbeInfo,
}

/// One kind of background analysis run on every newly imported asset it
/// [applies to](Self::applies_to): the frame index (WP-1D.02), thumbnails (WP-1D.05), the
/// waveform pyramid (WP-1D.06). The engine registers the stages in [`AnalysisStages`].
///
/// [`run`](Self::run) is blocking code on a job thread under the decode budget: it must poll
/// `ctx.check()?` between units of work (a cancelled job stops within 500 ms) and should report
/// `ctx.progress(..)`. It writes its results to the caches, never to the document. Returning
/// `Err(JobError::Failed(..))` (or panicking) fails the job and announces
/// `media.analysis_failed`.
pub trait AnalysisStage: Send + Sync {
    /// A short stable name: the job label (`index`, `thumbnails`, `waveform`). Lowercase
    /// ASCII letters, digits and `_`, unique among the registered stages.
    fn name(&self) -> &'static str;

    /// Names of earlier-registered stages that must succeed before this one runs on the same
    /// asset (for example thumbnails need the index). A required stage that does not apply to
    /// the asset is ignored.
    fn requires(&self) -> &'static [&'static str] {
        &[]
    }

    /// Whether success means the asset's frame index is ready (announced as `media.indexed`).
    fn builds_index(&self) -> bool {
        false
    }

    /// Whether this stage runs for `media` (for example a waveform only for media with sound).
    fn applies_to(&self, media: &AnalysisInput) -> bool {
        let _ = media;
        true
    }

    /// Analyzes `media`.
    fn run(&self, media: &AnalysisInput, ctx: &JobCtx) -> Result<(), JobError>;
}

/// A stage that cannot be registered.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum StageError {
    /// The name is empty or not lowercase ASCII letters, digits and `_`.
    #[error("invalid analysis stage name `{0}`")]
    InvalidName(String),
    /// Two stages share a name.
    #[error("duplicate analysis stage `{0}`")]
    Duplicate(String),
    /// A requirement names no earlier stage.
    #[error("analysis stage `{stage}` requires `{requires}`, which is not registered before it")]
    UnknownRequirement {
        /// The stage.
        stage: String,
        /// The missing stage.
        requires: String,
    },
}

/// The registered analysis stages, in run order. Empty until the real stages land
/// (then `media.import` returns no `job_ids`).
#[derive(Clone, Default)]
pub struct AnalysisStages {
    stages: Vec<Arc<dyn AnalysisStage>>,
}

impl AnalysisStages {
    /// No stages.
    #[must_use]
    pub fn new() -> AnalysisStages {
        AnalysisStages::default()
    }

    /// Appends `stage`. Its requirements must name stages added before it.
    pub fn add(
        &mut self,
        stage: Arc<dyn AnalysisStage>,
    ) -> Result<&mut AnalysisStages, StageError> {
        let name = stage.name();
        let valid = !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
        if !valid {
            return Err(StageError::InvalidName(name.to_owned()));
        }
        if self.names().any(|n| n == name) {
            return Err(StageError::Duplicate(name.to_owned()));
        }
        if let Some(missing) = stage
            .requires()
            .iter()
            .find(|r| !self.names().any(|n| n == **r))
        {
            return Err(StageError::UnknownRequirement {
                stage: name.to_owned(),
                requires: (*missing).to_owned(),
            });
        }
        self.stages.push(stage);
        Ok(self)
    }

    /// The stage names, in run order.
    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.stages.iter().map(|s| s.name())
    }

    /// Number of stages.
    #[must_use]
    pub fn len(&self) -> usize {
        self.stages.len()
    }

    /// Whether there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.stages.is_empty()
    }
}

impl std::fmt::Debug for AnalysisStages {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.names()).finish()
    }
}

/// Where analysis jobs publish their events: the engine implements it by forwarding into its
/// event fan-out. It is called from job threads.
pub trait AnalysisSink: Send + Sync {
    /// Broadcasts a media event to subscribers.
    fn publish(&self, event: MediaEvent);
}

/// Spawns the analysis jobs of newly imported assets (part of
/// [`ImportCtx`](super::ImportCtx)).
#[derive(Clone)]
pub struct Analyzer {
    jobs: JobManager,
    stages: Arc<AnalysisStages>,
    sink: Arc<dyn AnalysisSink>,
}

impl Analyzer {
    /// Runs `stages` on `jobs`, publishing events through `sink`.
    #[must_use]
    pub fn new(
        jobs: JobManager,
        stages: Arc<AnalysisStages>,
        sink: Arc<dyn AnalysisSink>,
    ) -> Analyzer {
        Analyzer { jobs, stages, sink }
    }

    /// The job manager.
    #[must_use]
    pub fn jobs(&self) -> &JobManager {
        &self.jobs
    }

    /// Queues one job per stage that applies to `media`, in stage order, and returns their
    /// IDs (none when no stage is registered).
    #[allow(clippy::must_use_candidate)] // queues the jobs; the IDs are for waiting on them
    pub fn spawn(&self, media: AnalysisInput) -> Vec<JobId> {
        self.spawn_stages(media)
            .into_iter()
            .map(|(_, id)| id)
            .collect()
    }

    /// [`spawn`](Self::spawn), with each job's stage name.
    #[allow(clippy::must_use_candidate)] // queues the jobs; the IDs are for waiting on them
    pub fn spawn_stages(&self, media: AnalysisInput) -> Vec<(&'static str, JobId)> {
        let media = Arc::new(media);
        let mut spawned: BTreeMap<&'static str, JobId> = BTreeMap::new();
        let mut ids = Vec::new();
        for stage in &self.stages.stages {
            if !stage.applies_to(&media) {
                continue;
            }
            let after = stage
                .requires()
                .iter()
                .filter_map(|r| spawned.get(r).copied());
            let spec = JobSpec::new(JobKind::Analysis, stage.name())
                .media(media.media_id)
                .after(after);
            let (stage, input, sink) = (
                Arc::clone(stage),
                Arc::clone(&media),
                Arc::clone(&self.sink),
            );
            let name = stage.name();
            let job = self
                .jobs
                .submit(spec, move |ctx| run_stage(&*stage, &input, &*sink, ctx));
            spawned.insert(name, job.id());
            ids.push((name, job.id()));
        }
        ids
    }
}

impl std::fmt::Debug for Analyzer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Analyzer")
            .field("stages", &self.stages)
            .finish_non_exhaustive()
    }
}

fn run_stage(
    stage: &dyn AnalysisStage,
    media: &AnalysisInput,
    sink: &dyn AnalysisSink,
    ctx: &JobCtx,
) -> Result<(), JobError> {
    ctx.check()?;
    let result = panic::catch_unwind(AssertUnwindSafe(|| stage.run(media, ctx)))
        .unwrap_or_else(|_| Err(JobError::failed("the stage crashed")));
    match &result {
        Ok(()) => {
            if stage.builds_index() {
                sink.publish(MediaEvent::Indexed(MediaIndexed {
                    media_id: media.media_id,
                }));
            }
        }
        Err(JobError::Failed(error)) if !ctx.is_cancelled() => {
            sink.publish(MediaEvent::AnalysisFailed(MediaAnalysisFailed {
                media_id: media.media_id,
                stage: stage.name().to_owned(),
                job_id: ctx.id(),
                error: error.clone(),
            }));
        }
        Err(_) => {}
    }
    result
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    struct Named(&'static str, &'static [&'static str]);

    impl AnalysisStage for Named {
        fn name(&self) -> &'static str {
            self.0
        }
        fn requires(&self) -> &'static [&'static str] {
            self.1
        }
        fn run(&self, _: &AnalysisInput, _: &JobCtx) -> Result<(), JobError> {
            Ok(())
        }
    }

    #[test]
    fn stages_register_in_order_with_checked_names_and_requirements() {
        let mut stages = AnalysisStages::new();
        assert!(stages.is_empty());
        stages.add(Arc::new(Named("index", &[]))).unwrap();
        stages
            .add(Arc::new(Named("thumbnails", &["index"])))
            .unwrap();
        assert_eq!(
            stages.add(Arc::new(Named("index", &[]))).unwrap_err(),
            StageError::Duplicate("index".into())
        );
        assert_eq!(
            stages.add(Arc::new(Named("Bad-Name", &[]))).unwrap_err(),
            StageError::InvalidName("Bad-Name".into())
        );
        assert!(matches!(
            stages.add(Arc::new(Named("waveform", &["later"]))),
            Err(StageError::UnknownRequirement { .. })
        ));
        assert_eq!(stages.names().collect::<Vec<_>>(), ["index", "thumbnails"]);
        assert_eq!(stages.len(), 2);
        assert_eq!(format!("{stages:?}"), r#"["index", "thumbnails"]"#);
    }

    #[test]
    fn media_events_have_their_wire_names() {
        let indexed = MediaEvent::Indexed(MediaIndexed {
            media_id: MediaId::from_u128(1),
        });
        let failed = MediaEvent::AnalysisFailed(MediaAnalysisFailed {
            media_id: MediaId::from_u128(1),
            stage: "thumbnails".into(),
            job_id: JobId::from_u128(2),
            error: "decoder error".into(),
        });
        let (vi, vf) = (
            serde_json::to_value(&indexed).unwrap(),
            serde_json::to_value(&failed).unwrap(),
        );
        assert_eq!(
            vi,
            serde_json::json!({
                "event": "media.indexed",
                "media_id": "med_00000000000000000000000000000001"
            })
        );
        assert_eq!(
            vf,
            serde_json::json!({
                "event": "media.analysis_failed",
                "media_id": "med_00000000000000000000000000000001",
                "stage": "thumbnails",
                "job_id": "job_00000000000000000000000000000002",
                "error": "decoder error"
            })
        );
        assert_eq!(serde_json::from_value::<MediaEvent>(vi).unwrap(), indexed);
        assert_eq!(serde_json::from_value::<MediaEvent>(vf).unwrap(), failed);
        assert!(EVENTS.iter().any(|(n, _)| *n == "media.analysis_failed"));
    }
}
