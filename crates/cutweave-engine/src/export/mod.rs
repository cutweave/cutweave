// SPDX-License-Identifier: GPL-3.0-or-later
//! Export (WP-1E.03, WP-2G.03a; arch §7, §8, §9, §14): how the engine runs `export.start` and
//! its export queue.
//!
//! `cutweave-ops` checks the request against the committed document and resolves it into an
//! [`ExportPlan`]; this module adds it to the engine's export queue ([`ExportJobs`], a
//! [`cutweave_jobs::ExportQueue`]: one export at a time under the job manager's export budget,
//! first in line first, pausable, reorderable) through a [`Starter`], and runs each export with
//! the [`pipeline`]: render every frame like `render.frame`, mix the range down, encode the
//! plan's codec (H.264, HEVC, AV1 or ProRes, WP-2G.02) and audio (AAC, or PCM in MOV) into an
//! MP4 or MOV. `export.status` is `job.status`; `job.cancel` / `export.cancel` stop an
//! export within a frame and remove its partial file, and the next one starts.
//!
//! What an export renders, decodes and encodes with comes from the subsystems
//! ([`ExportServices`], [`Subsystems::export_services`](crate::Subsystems::export_services)):
//! the media backend's encoder, the clip [`ExportSources`] and the renderer shared with
//! `render.frame`. The core engine has none of them, so there `export.start` fails with
//! `Unsupported` (after validating its params). The services are taken when an export runs, so
//! exports restored at start wait for the subsystems.
//!
//! An output path is claimed while its export is queued or running: a second export to the same
//! file fails with `Invalid` at `/path` instead of racing it.
//!
//! **The queue's store** ([`queue_store`], ADR-0030) is the open project's
//! `.cutweave/exports.sqlite`; an untitled project's queue lives in memory and moves into the
//! project it is saved as or the next one opened. Switching to another project suspends the old
//! one's exports (a running one stops and starts again from the beginning when that project is
//! opened again); a clean stop or a crash leaves them to the next engine the same way. An
//! interrupted export's partial file (`.<name>.<pid>-<n>.partial`, written by an engine that is
//! gone) is removed before it runs again ([`remove_stale_partials`]).
//!
//! The video encoder follows `export.start{hw}` (WP-1E.02, WP-2G.02): `auto` (the first hardware
//! encoder of the plan's codec that works, else the software one, restarting in software when a
//! hardware encoder fails during the export), `off` (software) or `{"force": "<implementation>"}`,
//! which must name one of the machine's `Capabilities::hw_encode` entries for the plan's codec
//! (`Invalid` at `/hw/force` otherwise) and never falls back.

pub mod pipeline;
pub mod sources;

use std::path::Path;
use std::sync::Arc;

use cutweave_commands::CmdError;
use cutweave_jobs::queue::{
    ExportQueue, MemoryQueueStore, QUEUE_FILE, QueueItem, QueueStore, SqliteQueueStore,
};
use cutweave_jobs::{JobCtx, JobError, JobId, JobManager, JobProgress};
use cutweave_media::Capabilities;
use cutweave_ops::export::encoders::EncoderEntry;
use cutweave_ops::export::queue::{PlanCodec, queue_error};
use cutweave_ops::export::{ExportHw, ExportPlan, ExportStarter};

pub use pipeline::{
    EXPORT_CHANNELS, EXPORT_SAMPLE_RATE, EXPORT_YIELD, ExportError, ExportPacer, ExportServices,
    ExportSummary, FrameTap, PIPELINE_DEPTH, PROGRESS_INTERVAL, ProgressReport, audio_codec_name,
    hw_encode, rate_control, render_frame, run_export, video_codec_id,
};
pub use sources::{
    DecoderFrameSource, DecoderPcmFactory, DecoderSources, ExportSources, FORWARD_DECODE_LIMIT,
};

use crate::Subsystems;

/// The job label of exports.
pub const EXPORT_LABEL: &str = "export";

/// The engine's export queue.
pub type ExportJobs = ExportQueue<ExportPlan>;

/// The engine's export queue on `jobs`: each export takes its services from `subsystems` when
/// it runs; at most `slots` run at once. Nothing starts before
/// [`start`](ExportQueue::start).
#[must_use]
pub fn export_queue(jobs: JobManager, subsystems: Arc<dyn Subsystems>, slots: usize) -> ExportJobs {
    let runner = Arc::new(move |plan: &ExportPlan, ctx: &JobCtx| {
        let services = subsystems.export_services().ok_or_else(|| {
            JobError::failed(
                "this engine cannot export now: its media backend or renderer is not running",
            )
        })?;
        remove_stale_partials(&plan.path);
        let summary = run_export(plan, &services, ctx.token(), &|p| ctx.report(p))?;
        // The closing report keeps the counters: every frame, nothing left, the file's size.
        ctx.report(JobProgress {
            fraction: 1.0,
            message: Some(summary.message()),
            frame: Some(summary.encoded.video_frames),
            frames: Some(u64::from(plan.frames)),
            fps: None,
            eta_ms: Some(0),
            bytes: std::fs::metadata(&plan.path).ok().map(|m| m.len()),
        });
        Ok(())
    });
    ExportQueue::new(jobs, runner, Arc::new(PlanCodec), slots)
}

/// Where a session's exports are kept: `<meta>/exports.sqlite` for a writable project (its
/// `.cutweave` folder), memory otherwise or when the file cannot be opened (logged).
#[must_use]
pub fn queue_store(meta_dir: Option<&Path>) -> Box<dyn QueueStore> {
    let Some(meta) = meta_dir else {
        return Box::new(MemoryQueueStore);
    };
    let path = meta.join(QUEUE_FILE);
    let opened = std::fs::create_dir_all(meta)
        .map_err(|e| e.to_string())
        .and_then(|()| SqliteQueueStore::open(&path).map_err(|e| e.to_string()));
    match opened {
        Ok(store) => Box::new(store),
        Err(e) => {
            tracing::warn!(file = %path.display(), error = %e, "cannot open the export queue; queued exports will not survive a restart");
            Box::new(MemoryQueueStore)
        }
    }
}

/// Removes partial files of `output` left by engines that are gone
/// (`.<name>.<pid>-<n>.partial` with another process id), so an interrupted export leaves no
/// debris when it runs again. Best effort (logged).
pub fn remove_stale_partials(output: &Path) {
    let (Some(folder), Some(name)) = (output.parent(), output.file_name()) else {
        return;
    };
    let prefix = format!(".{}.", name.to_string_lossy());
    let own = format!("{}-", std::process::id());
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let file = entry.file_name();
        let file = file.to_string_lossy();
        let Some(tag) = file
            .strip_prefix(&prefix)
            .and_then(|rest| rest.strip_suffix(".partial"))
        else {
            continue;
        };
        let Some((pid, n)) = tag.split_once('-') else {
            continue;
        };
        if tag.starts_with(&own)
            || pid.is_empty()
            || n.is_empty()
            || !pid.bytes().chain(n.bytes()).all(|b| b.is_ascii_digit())
        {
            continue;
        }
        match std::fs::remove_file(entry.path()) {
            Ok(()) => {
                tracing::info!(file = %entry.path().display(), "removed the partial file of an interrupted export");
            }
            Err(e) => {
                tracing::warn!(file = %entry.path().display(), error = %e, "cannot remove an interrupted export's partial file");
            }
        }
    }
}

/// Adds exports to the engine's queue for `export.start` / `export.enqueue` (the engine's
/// [`ExportStarter`]) and hands the queue to the queue commands.
pub struct Starter {
    queue: ExportJobs,
    subsystems: Arc<dyn Subsystems>,
}

impl std::fmt::Debug for Starter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Starter")
            .field("queue", &self.queue)
            .finish_non_exhaustive()
    }
}

impl Starter {
    /// A starter adding to `queue`, checking `subsystems`' export services first.
    #[must_use]
    pub fn new(queue: ExportJobs, subsystems: Arc<dyn Subsystems>) -> Starter {
        Starter { queue, subsystems }
    }

    /// Checks that this engine can run `plan` now, then queues it.
    fn add(&self, plan: ExportPlan, resume: bool) -> Result<JobId, CmdError> {
        let services = self.subsystems.export_services().ok_or_else(|| {
            CmdError::unsupported(
                "exporting needs the media backend and the renderer, which this engine does not \
                 run",
            )
        })?;
        let caps = services.encoder.capabilities();
        let video = video_codec_id(plan.output.video_codec);
        for codec in [video, audio_codec_name(plan.output.audio_codec)] {
            if !caps.can_encode(codec) {
                return Err(CmdError::unsupported(format!(
                    "this engine's media backend has no `{codec}` encoder"
                )));
            }
        }
        if let ExportHw::Force(name) = &plan.hw {
            check_forced(&caps, name, video)?;
        }
        let item = QueueItem {
            label: EXPORT_LABEL.to_owned(),
            output: Some(plan.path.clone()),
            task: plan,
        };
        self.queue
            .enqueue(item, resume)
            .map_err(|e| queue_error(e, "/path"))
    }
}

/// `Invalid` at `/hw/force` unless `name` is one of the machine's hardware encoders of `codec`.
fn check_forced(caps: &Capabilities, name: &str, codec: &str) -> Result<(), CmdError> {
    let available: Vec<&str> = caps
        .hw_encode
        .iter()
        .filter(|c| c.codec == codec)
        .map(|c| c.implementation.as_str())
        .collect();
    if available.contains(&name) {
        return Ok(());
    }
    let msg = if available.is_empty() {
        format!("`{name}` is not available: this machine has no hardware {codec} encoder")
    } else {
        format!(
            "`{name}` is not available here; the hardware {codec} encoders are: {}",
            available.join(", ")
        )
    };
    Err(CmdError::invalid("/hw/force", msg))
}

impl ExportStarter for Starter {
    fn start(&mut self, plan: ExportPlan) -> Result<JobId, CmdError> {
        self.add(plan, true)
    }

    fn enqueue(&mut self, plan: ExportPlan) -> Result<JobId, CmdError> {
        self.add(plan, false)
    }

    fn queue(&mut self) -> Result<ExportJobs, CmdError> {
        Ok(self.queue.clone())
    }

    fn encoders(&mut self) -> Result<Vec<EncoderEntry>, CmdError> {
        let services = self.subsystems.export_services().ok_or_else(|| {
            CmdError::unsupported("this engine has no media backend to list encoders from")
        })?;
        Ok(encoder_entries(&services.encoder.capabilities()))
    }
}

/// The video encoders of `caps` for `export.list_encoders`: the software ones of the export
/// codecs, then the hardware ones that passed the trial encode, in `auto` order.
fn encoder_entries(caps: &Capabilities) -> Vec<EncoderEntry> {
    let mut entries = Vec::new();
    for id in ["h264", "hevc", "av1", "prores"] {
        if let Some(codec) = caps.codec(id) {
            entries.extend(codec.encoders.iter().map(|name| EncoderEntry {
                codec: id.to_owned(),
                implementation: name.clone(),
                hardware: false,
                device: None,
            }));
        }
        entries.extend(
            caps.hw_encode
                .iter()
                .filter(|c| c.codec == id)
                .map(|c| EncoderEntry {
                    codec: id.to_owned(),
                    implementation: c.implementation.clone(),
                    hardware: true,
                    device: Some(c.device.clone()),
                }),
        );
    }
    entries
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn stale_partials_of_other_processes_are_removed_and_ours_and_others_kept() {
        let dir =
            std::env::temp_dir().join(format!("cutweave-stale-partials-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ours = format!(".out.mp4.{}-3.partial", std::process::id());
        for name in [
            ".out.mp4.1-0.partial",
            ".out.mp4.99999999-12.partial",
            ours.as_str(),
            ".other.mp4.1-0.partial",
            ".out.mp4.x-1.partial",
            "out.mp4",
        ] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        remove_stale_partials(&dir.join("out.mp4"));
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        let mut expected = vec![
            ours,
            ".other.mp4.1-0.partial".to_owned(),
            ".out.mp4.x-1.partial".to_owned(),
            "out.mp4".to_owned(),
        ];
        expected.sort();
        assert_eq!(left, expected);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_untitled_project_queues_in_memory_and_a_project_in_its_meta_folder() {
        assert!(!queue_store(None).persistent());
        let dir = std::env::temp_dir().join(format!("cutweave-queue-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let meta = dir.join(".cutweave");
        let store = queue_store(Some(&meta));
        assert!(store.persistent());
        assert!(meta.join(QUEUE_FILE).is_file());
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
