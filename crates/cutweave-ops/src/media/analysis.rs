// SPDX-License-Identifier: GPL-3.0-or-later
//! Background analysis of imported media (WP-1D.07; arch §7): after `media.import` registers
//! an asset, one job per applicable [`AnalysisStage`] runs on the [`JobManager`] (index →
//! thumbnails → waveform as WP-1D.02/1D.05/1D.06 land; none are registered before that).
//!
//! - A stage that fails records `analysis_error` on the asset through the bus
//!   ([`MediaSetAnalysisError`], via the engine's [`AnalysisSink`]), so the UI can badge it.
//! - A stage that [builds the index](AnalysisStage::builds_index) announces `media.indexed`
//!   ([`MediaEvent::Indexed`]) when it succeeds.
//! - A stage may [require](AnalysisStage::requires) earlier stages: its job waits for theirs
//!   (without holding a budget slot) and is skipped if one fails.
//!
//! **Where `analysis_error` lives.** Model v1's `MediaAsset` has no typed field for it yet
//! (the arch §4.2 `analysis{}` block needs a model change and migration). Until then it is
//! kept in the asset's `extra` map under [`ANALYSIS_ERROR_KEY`], which serializes as a
//! top-level `analysis_error` field of the asset, exactly where the typed field will be.

use std::{
    collections::BTreeMap,
    panic::{self, AssertUnwindSafe},
    sync::Arc,
};

use cutweave_commands::{CmdError, Command, QueryCtx, Tx, command, wire};
use cutweave_jobs::{JobCtx, JobError, JobId, JobKind, JobManager, JobSpec};
use cutweave_model::{MediaAsset, MediaId, MediaKind, ProbeInfo};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The asset field that holds the last analysis failure (see the module docs).
pub const ANALYSIS_ERROR_KEY: &str = "analysis_error";
/// Longest stored analysis error, in characters (longer messages are cut).
pub const MAX_ANALYSIS_ERROR_CHARS: usize = 500;

/// The events of the `media` namespace, as `(name, summary)`: input for the engine's event
/// catalog (WP-1N.01).
pub const EVENTS: &[(&str, &str)] = &[(
    "media.indexed",
    "A media asset's frame index is ready (frame-exact seeking and decoding can use it)",
)];

/// The last analysis failure recorded on `media`, if any.
#[must_use]
pub fn analysis_error(media: &MediaAsset) -> Option<&str> {
    media.extra.get(ANALYSIS_ERROR_KEY).and_then(Value::as_str)
}

// ----- media.set_analysis_error --------------------------------------------------------------

/// Result of `media.set_analysis_error`.
#[wire]
#[derive(Copy)]
pub struct AnalysisErrorSet {
    /// The asset.
    pub media_id: MediaId,
}

/// Records (or, with no `error`, clears) the failure of a background analysis stage on a
/// media asset, so the UI can show a badge. Internal plumbing of the analysis jobs, applied by
/// the engine as the `system` actor; not undoable (it reports, it does not edit).
#[command(
    name = "media.set_analysis_error",
    summary = "Record or clear a background analysis failure on a media asset (internal)",
    undoable = false,
    mcp = "hidden",
    annotations(destructive = false, idempotent = true)
)]
pub struct MediaSetAnalysisError {
    /// The asset.
    pub media_id: MediaId,
    /// What failed (1..=500 characters), or absent to clear it.
    pub error: Option<String>,
}

impl Command for MediaSetAnalysisError {
    type Output = AnalysisErrorSet;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        ctx.media(self.media_id)?;
        if let Some(error) = &self.error {
            let chars = error.chars().count();
            if chars == 0 || chars > MAX_ANALYSIS_ERROR_CHARS {
                return Err(CmdError::invalid(
                    "/error",
                    format!("the error must be 1..={MAX_ANALYSIS_ERROR_CHARS} characters"),
                ));
            }
        }
        Ok(())
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<AnalysisErrorSet, CmdError> {
        self.validate(&tx.query_ctx())?;
        tx.update_media(self.media_id, |media| {
            match &self.error {
                Some(error) => {
                    media
                        .extra
                        .insert(ANALYSIS_ERROR_KEY, Value::String(error.clone()));
                }
                None => {
                    media.extra.remove(ANALYSIS_ERROR_KEY);
                }
            }
            Ok(())
        })?;
        Ok(AnalysisErrorSet {
            media_id: self.media_id,
        })
    }
}

// ----- events --------------------------------------------------------------------------------

/// `media.indexed`: the asset's frame index is ready.
#[wire]
#[derive(Copy)]
pub struct MediaIndexed {
    /// The asset.
    pub media_id: MediaId,
}

/// The events of the `media` namespace (see [`EVENTS`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event")]
#[non_exhaustive]
pub enum MediaEvent {
    /// `media.indexed`.
    #[serde(rename = "media.indexed")]
    Indexed(MediaIndexed),
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
/// `ctx.progress(..)`. Returning `Err(JobError::Failed(..))` (or panicking) fails the job and
/// records `analysis_error` on the asset.
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

/// How analysis jobs reach the rest of the engine. The engine implements it.
pub trait AnalysisSink: Send + Sync {
    /// Applies `command` through the command bus as the `system` actor, in its own
    /// transaction. Failures (for example the asset was removed meanwhile) are the engine's to
    /// log; the job's own outcome does not change.
    fn set_analysis_error(&self, command: MediaSetAnalysisError);

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
    /// Runs `stages` on `jobs`, reporting through `sink`.
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
            ids.push(job.id());
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

fn truncate(text: String, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((cut, _)) => text[..cut].to_owned(),
        None => text,
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
        Err(JobError::Failed(msg)) if !ctx.is_cancelled() => {
            let error = truncate(format!("{}: {msg}", stage.name()), MAX_ANALYSIS_ERROR_CHARS);
            sink.set_analysis_error(MediaSetAnalysisError {
                media_id: media.media_id,
                error: Some(error),
            });
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
    fn long_errors_are_cut_on_a_char_boundary() {
        assert_eq!(truncate("héllo".into(), 2), "hé");
        assert_eq!(truncate("ok".into(), 5), "ok");
    }

    #[test]
    fn media_indexed_has_its_wire_name() {
        let e = MediaEvent::Indexed(MediaIndexed {
            media_id: MediaId::from_u128(1),
        });
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "event": "media.indexed",
                "media_id": "med_00000000000000000000000000000001"
            })
        );
        assert_eq!(serde_json::from_value::<MediaEvent>(v).unwrap(), e);
    }
}
