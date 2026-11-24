// SPDX-License-Identifier: GPL-3.0-or-later
//! Export (WP-1E.03; arch §7, §8, §9, §14): how the engine runs `export.start`.
//!
//! `cutweave-ops` checks the request against the committed document and resolves it into an
//! [`ExportPlan`]; this module queues it as a job of kind `export` ([`Starter`], one export at a
//! time under the job manager's export budget) and runs the [`pipeline`]: render every frame
//! like `render.frame`, mix the range down, encode H.264 + AAC into an MP4. `export.status` is
//! `job.status`; `job.cancel` stops an export within a frame and removes its partial file.
//!
//! What an export renders, decodes and encodes with comes from the subsystems
//! ([`ExportServices`], [`Subsystems::export_services`](crate::Subsystems::export_services)):
//! the media backend's encoder, the clip [`ExportSources`] and the renderer shared with
//! `render.frame`. The core engine has none of them, so there `export.start` fails with
//! `Unsupported` (after validating its params).
//!
//! An output path is claimed while its export is queued or running ([`ActiveExports`]): a
//! second export to the same file fails with `Invalid` at `/path` instead of racing it.

pub mod pipeline;
pub mod sources;

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use cutweave_commands::CmdError;
use cutweave_jobs::{JobId, JobKind, JobManager, JobSpec};
use cutweave_ops::export::{ExportPlan, ExportStarter};

pub use pipeline::{
    EXPORT_CHANNELS, EXPORT_SAMPLE_RATE, ExportError, ExportServices, ExportSummary, FrameTap,
    PIPELINE_DEPTH, crf, render_frame, run_export,
};
pub use sources::{
    DecoderFrameSource, DecoderPcmFactory, DecoderSources, ExportSources, FORWARD_DECODE_LIMIT,
};

/// The job label of exports.
pub const EXPORT_LABEL: &str = "export";

/// Output files of the exports that are queued or running.
#[derive(Clone, Debug, Default)]
pub struct ActiveExports(Arc<Mutex<BTreeSet<String>>>);

impl ActiveExports {
    /// The comparison key of `path` (ASCII case-insensitive on Windows).
    fn key(path: &Path) -> String {
        let s = path.to_string_lossy();
        if cfg!(windows) {
            s.replace('/', "\\").to_ascii_lowercase()
        } else {
            s.into_owned()
        }
    }

    /// Claims `path` until the returned guard is dropped; `Invalid` at `/path` while another
    /// export holds it.
    pub fn claim(&self, path: &Path) -> Result<PathClaim, CmdError> {
        let key = ActiveExports::key(path);
        let mut set = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if !set.insert(key.clone()) {
            return Err(CmdError::invalid(
                "/path",
                format!(
                    "another export is already writing `{}`; wait for it (job.wait) or cancel it",
                    path.display()
                ),
            ));
        }
        Ok(PathClaim {
            owner: self.clone(),
            key,
        })
    }

    /// How many outputs are claimed.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).len()
    }

    /// Whether no output is claimed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// An output path claimed by one export; released on drop.
#[derive(Debug)]
pub struct PathClaim {
    owner: ActiveExports,
    key: String,
}

impl Drop for PathClaim {
    fn drop(&mut self) {
        self.owner
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.key);
    }
}

/// Queues export jobs for `export.start` (the engine's [`ExportStarter`]).
#[derive(Debug)]
pub struct Starter {
    jobs: JobManager,
    services: Option<ExportServices>,
    active: ActiveExports,
}

impl Starter {
    /// A starter submitting to `jobs`, exporting with `services` (`None`: this engine cannot
    /// export), claiming outputs in `active`.
    #[must_use]
    pub fn new(
        jobs: JobManager,
        services: Option<ExportServices>,
        active: ActiveExports,
    ) -> Starter {
        Starter {
            jobs,
            services,
            active,
        }
    }
}

impl ExportStarter for Starter {
    fn start(&mut self, plan: ExportPlan) -> Result<JobId, CmdError> {
        let services = self.services.clone().ok_or_else(|| {
            CmdError::unsupported(
                "exporting needs the media backend and the renderer, which this engine does not \
                 run",
            )
        })?;
        let caps = services.encoder.capabilities();
        for codec in ["h264", "aac"] {
            if !caps.can_encode(codec) {
                return Err(CmdError::unsupported(format!(
                    "this engine's media backend has no `{codec}` encoder"
                )));
            }
        }
        let claim = self.active.claim(&plan.path)?;
        let handle = self
            .jobs
            .submit(JobSpec::new(JobKind::Export, EXPORT_LABEL), move |ctx| {
                // Released when the job ends (or is dropped unstarted after a cancel).
                let _claim = claim;
                let summary = run_export(&plan, &services, ctx.token(), &|fraction, msg| {
                    ctx.progress(fraction, Some(msg));
                })?;
                ctx.progress(1.0, Some(&summary.message()));
                Ok(())
            });
        Ok(handle.id())
    }
}
