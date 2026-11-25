// SPDX-License-Identifier: GPL-3.0-or-later
//! Proxies (WP-2F.01; arch §7): smaller intra-frame copies of video files that the preview
//! scrubs, kept in the global cache. Exports and agent frames never use them.
//!
//! - [`ProxyProfile`] / [`ProxyCodec`]: what a proxy is made of. ProRes Proxy when the backend
//!   has an encoder for it, else DNxHR LB, else MJPEG; at most 960 pixels on the long side by
//!   default, never larger than the original; 16-bit PCM sound.
//! - [`ProxyStore`]: the files under `<cache>/proxies/<hash>/<profile>/` and their rows in
//!   `cache.sqlite` (table `proxies`), keyed by the original's content hash and the profile;
//!   states building → ready → evicted (or failed); a size-capped LRU (50 GiB by default).
//! - [`ProxyGenerator`]: makes one proxy, resumably (parts joined at the end), with frame `n`
//!   of the proxy equal to frame `n` of the original (the original's index is stored with the
//!   proxy as its `pts_map`, which conforms variable frame rates).
//! - [`submit_proxy`]: a generation as a background job ([`JobKind::Proxy`], two at a time and
//!   one while an export runs), with progress and cancellation.

mod generate;
mod profile;
mod store;

use std::sync::Arc;

use cutweave_jobs::{JobCtx, JobError, JobHandle, JobId, JobKind, JobManager, JobSpec};
use cutweave_media::{CancelCheck, CancelToken, MediaError};
use cutweave_model::MediaId;

pub use generate::{
    DEFAULT_PART_FRAMES, GenerateOptions, GenerateReport, Pacer, ProxyGenerator, ProxySource,
};
pub use profile::{ProxyCodec, ProxyProfile};
pub use store::{ProxyRecord, ProxyState, ProxyStore, ReadyProxy, plan_eviction};

use crate::CacheError;

/// The label of proxy jobs (`job.list`, `job.progress`).
pub const PROXY_JOB_LABEL: &str = "proxy";

/// What a proxy job reports when it ends: called on the job thread with the outcome (the
/// engine announces it and refreshes the preview).
pub type ProxyDone = Box<dyn FnOnce(&Result<GenerateReport, CacheError>) + Send + 'static>;

/// Queues the generation of `source`'s proxy in `profile` as a [`JobKind::Proxy`] job on
/// `manager`, labelled [`PROXY_JOB_LABEL`] with `media_id`, after the jobs in `after` (the
/// asset's `index` analysis, so the original is scanned once). Cancelling the job stops it
/// between frames and keeps the finished parts. `done` gets the outcome.
#[allow(clippy::must_use_candidate)] // queues the job; the handle is for following it
pub fn submit_proxy(
    manager: &JobManager,
    generator: Arc<ProxyGenerator>,
    source: ProxySource,
    profile: ProxyProfile,
    media_id: Option<MediaId>,
    after: Vec<JobId>,
    done: ProxyDone,
) -> JobHandle {
    let mut job = JobSpec::new(JobKind::Proxy, PROXY_JOB_LABEL).after(after);
    job.media_id = media_id;
    manager.submit(job, move |ctx: &JobCtx| {
        let token = ctx.token().clone();
        let cancel = CancelToken::linked(CancelCheck::new(move || token.is_cancelled()));
        ctx.progress(0.0, Some(PROXY_JOB_LABEL));
        let result = generator.generate(&source, &profile, &cancel, &mut |f| {
            ctx.progress(f, Some(PROXY_JOB_LABEL));
        });
        done(&result);
        match result {
            Ok(_) => Ok(()),
            Err(CacheError::Media(MediaError::Cancelled)) => Err(JobError::Cancelled),
            Err(e) => Err(JobError::failed(e.to_string())),
        }
    })
}
