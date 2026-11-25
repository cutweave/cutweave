// SPDX-License-Identifier: GPL-3.0-or-later
//! [`FullProxies`]: the full engine's [`ProxyService`] (WP-2F.01; arch §7).
//!
//! - **Store and generator.** Proxies live in `<cache>/proxies` with their rows in
//!   `cache.sqlite` ([`ProxyStore`], 50 GiB LRU by default, `media.set_cache_config
//!   {limits.proxies}`); the [`ProxyGenerator`] makes them with the media backend in the best
//!   profile it can encode (ProRes Proxy at 960 pixels on FFmpeg). A backend that encodes no
//!   proxy codec makes none.
//! - **Jobs.** One [`JobKind::Proxy`](cutweave_jobs::JobKind::Proxy) job per proxy (two at a
//!   time, one while an export runs), deduplicated by content hash and profile, so two assets
//!   with the same content, an import and a reconcile never make the same proxy twice.
//! - **When.** `media.import` queues what the policy wants after the asset's `index` job;
//!   attaching a document ([`FullProxies::reconcile`]: engine start, `project.open`) queues
//!   wanted proxies that were never made or were interrupted (a killed engine resumes from the
//!   finished parts); `media.generate_proxy` queues any. Evicted and failed proxies are made
//!   again only on request, so a cache too small for the project never thrashes.
//! - **Threads.** A reconcile runs on its own `cw-proxy-reconcile` thread, which
//!   [`FullProxies::stop`] joins at shutdown; generations run as jobs on the engine runtime's
//!   blocking pool, cancelled and joined with it.
//! - **Yielding.** A generation takes its next frame only when the preview has served no
//!   picture for [`IDLE_BEFORE_PROXIES`] (playback, scrub, Source view; issue 153); finished
//!   parts stay, so a pause loses nothing. Exports yield too, less: while the preview is busy
//!   each exported frame waits [`EXPORT_YIELD`](crate::export::EXPORT_YIELD) (WP-2G.03a).
//! - **Policy.** [`DEFAULT_POLICY`] (`auto`) for every project until ADR-0027 stores one in
//!   the project.

use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration as StdDuration, Instant};

use cutweave_cache::proxy::{
    GenerateOptions, Pacer, ProxyGenerator, ProxyProfile, ProxyRecord, ProxySource, ProxyState,
    ProxyStore, ReadyProxy, submit_proxy,
};
use cutweave_cache::{CacheDb, ContentHash};
use cutweave_commands::{CmdError, DocHandle};
use cutweave_jobs::{JobId, JobManager, JobState};
use cutweave_media::{CancelToken, MediaBackend};
use cutweave_model::{MediaAsset, MediaId, Project};
use cutweave_ops::media::proxy::{
    DEFAULT_POLICY, ImportProxy, ProxyInfo, ProxyService, ProxyStatus, proxyable, wants_proxy,
};

use super::media::CurrentDoc;
use crate::subsystems::EngineLink;

/// How long `media.delete_proxy` waits for a cancelled generation to stop before removing the
/// files.
const DELETE_WAIT: StdDuration = StdDuration::from_secs(5);

type Key = (ContentHash, String);

/// How long the engine must have served no preview picture before proxy jobs take frames again.
///
/// Playback asks for a picture every frame (at most ~100 ms apart even at 10 fps) and a scrub
/// keeps asking while the pointer moves, so 1.5 s of silence means the user stopped (paused,
/// let go of the scrub, left the Source view). It is long enough not to start a decode in
/// every pause between two scrubs or between pressing pause and play again, and short enough
/// that an idle engine finishes proxies right after the user looks away. Jobs resume at part
/// granularity of work already on disk, so a pause costs nothing but time.
pub(crate) const IDLE_BEFORE_PROXIES: StdDuration = StdDuration::from_millis(1500);

/// How often a paused proxy job looks at the activity again (and at its cancel token).
const PAUSE_POLL: StdDuration = StdDuration::from_millis(50);

/// Time for [`Activity`] (a manual one in tests).
pub(crate) trait Clock: Send + Sync {
    /// Milliseconds since some fixed point; never goes back.
    fn now_ms(&self) -> u64;
}

struct WallClock(Instant);

impl Clock for WallClock {
    fn now_ms(&self) -> u64 {
        u64::try_from(self.0.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

/// When the engine last served a picture to the preview (playback, scrubbing, the Source
/// view). The preview touches it per picture with one atomic store: no lock, no allocation.
/// Proxy generations pause on it ([`Pacer`]).
pub(crate) struct Activity {
    clock: Arc<dyn Clock>,
    /// `now_ms + 1` of the last touch (0: never).
    last: AtomicU64,
    idle_after: StdDuration,
}

impl Activity {
    pub(crate) fn new(clock: Arc<dyn Clock>, idle_after: StdDuration) -> Activity {
        Activity {
            clock,
            last: AtomicU64::new(0),
            idle_after,
        }
    }

    fn wall() -> Activity {
        Activity::new(Arc::new(WallClock(Instant::now())), IDLE_BEFORE_PROXIES)
    }

    /// Records that a preview picture was just asked for.
    pub(crate) fn touch(&self) {
        self.last
            .store(self.clock.now_ms().saturating_add(1), Ordering::Relaxed);
    }

    /// Whether a picture was asked for within the idle time.
    pub(crate) fn busy(&self) -> bool {
        match self.last.load(Ordering::Relaxed) {
            0 => false,
            t => {
                let idle = u64::try_from(self.idle_after.as_millis()).unwrap_or(u64::MAX);
                self.clock.now_ms().saturating_add(1).saturating_sub(t) < idle
            }
        }
    }
}

/// Exports give the preview the GPU (WP-2G.03a): while it is busy, each exported frame waits
/// [`EXPORT_YIELD`](crate::export::EXPORT_YIELD) first.
impl crate::export::ExportPacer for Activity {
    fn before_frame(&self, cancel: &cutweave_jobs::CancelToken) {
        if self.busy() && !cancel.is_cancelled() {
            std::thread::sleep(crate::export::EXPORT_YIELD);
        }
    }
}

impl Pacer for Activity {
    fn wait(&self, cancel: &CancelToken) {
        while self.busy() && !cancel.is_cancelled() {
            std::thread::sleep(PAUSE_POLL);
        }
    }
}

/// The full engine's proxies (see the module docs). Cheap to share (`Arc`).
pub struct FullProxies {
    store: Arc<ProxyStore>,
    generator: Arc<ProxyGenerator>,
    profile: Option<ProxyProfile>,
    doc: CurrentDoc,
    link: EngineLink,
    /// Generations queued or running, by original and profile.
    jobs: Arc<Mutex<HashMap<Key, JobId>>>,
    /// Interactive preview activity that pauses generations (issue 153).
    activity: Arc<Activity>,
    /// Reconcile threads not joined yet ([`FullProxies::stop`]).
    reconciles: Mutex<Vec<JoinHandle<()>>>,
}

impl std::fmt::Debug for FullProxies {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FullProxies")
            .field("profile", &self.profile)
            .field("root", &self.store.root())
            .finish_non_exhaustive()
    }
}

fn hash_of(asset: &MediaAsset) -> Option<ContentHash> {
    asset
        .content_hash
        .as_deref()
        .and_then(|h| ContentHash::from_str(h).ok())
}

fn failure(what: &str, e: &cutweave_cache::CacheError) -> CmdError {
    CmdError::internal(format!("{what}: {e}"))
}

impl FullProxies {
    /// Proxies made with `backend` under `root` (the store's directory), accounted in `db`,
    /// for the media of `doc`; jobs run on the engine's job manager (through `link`).
    pub(crate) fn new(
        backend: &Arc<dyn MediaBackend>,
        db: Arc<CacheDb>,
        root: PathBuf,
        doc: CurrentDoc,
        link: EngineLink,
    ) -> FullProxies {
        let store = Arc::new(ProxyStore::new(
            Arc::clone(&db),
            root,
            ProxyStore::DEFAULT_MAX_BYTES,
        ));
        let activity = Arc::new(Activity::wall());
        let generator = Arc::new(
            ProxyGenerator::new(
                Arc::clone(backend),
                db,
                Arc::clone(&store),
                GenerateOptions::default(),
            )
            .with_pacer(Arc::clone(&activity) as Arc<dyn Pacer>),
        );
        let profile = generator.profile();
        if profile.is_none() {
            tracing::info!("the media backend encodes no proxy codec: no proxies are made");
        }
        FullProxies {
            store,
            generator,
            profile,
            doc,
            link,
            jobs: Arc::default(),
            activity,
            reconciles: Mutex::default(),
        }
    }

    /// [`reconcile`](Self::reconcile)s `doc` on its own thread (the per-project hooks call it
    /// under the session lock); [`stop`](Self::stop) joins the thread.
    pub(crate) fn reconcile_in_background(self: &Arc<Self>, doc: &DocHandle) {
        let snapshot = doc.snapshot();
        let proxies = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("cw-proxy-reconcile".into())
            .spawn(move || proxies.reconcile(&snapshot.doc));
        let thread = match spawned {
            Ok(thread) => thread,
            Err(e) => {
                tracing::warn!(error = %e, "cannot queue the project's proxies");
                return;
            }
        };
        let finished = {
            let mut threads = self
                .reconciles
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let (finished, running) = std::mem::take(&mut *threads)
                .into_iter()
                .partition(JoinHandle::is_finished);
            *threads = running;
            threads.push(thread);
            finished
        };
        join_reconciles(finished);
    }

    /// Waits for the reconcile threads (engine shutdown), so none outlives the engine; they
    /// only read the cache database and queue jobs, which the engine cancels next.
    pub(crate) fn stop(&self) {
        let threads = std::mem::take(
            &mut *self
                .reconciles
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        join_reconciles(threads);
    }

    /// Records that the preview is serving a picture: proxy jobs wait for it to go quiet.
    pub(crate) fn note_preview_activity(&self) {
        self.activity.touch();
    }

    /// The preview's activity, which also paces exports.
    pub(crate) fn activity(&self) -> Arc<Activity> {
        Arc::clone(&self.activity)
    }

    /// The store.
    pub(crate) fn store(&self) -> &Arc<ProxyStore> {
        &self.store
    }

    /// The ready proxy of `asset`, if any (the preview's lookup; marks it recently used).
    pub(crate) fn ready(&self, asset: &MediaAsset) -> Option<Arc<ReadyProxy>> {
        if !proxyable(asset) {
            return None;
        }
        let (hash, profile) = (hash_of(asset)?, self.profile?);
        match self.store.ready(&hash, &profile.key()) {
            Ok(ready) => ready,
            Err(e) => {
                tracing::debug!(error = %e, "cannot read a proxy");
                None
            }
        }
    }

    fn running(&self) -> std::sync::MutexGuard<'_, HashMap<Key, JobId>> {
        self.jobs.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The generation job of `key` that is still queued or running.
    fn job_of(&self, jobs: Option<&JobManager>, key: &Key) -> Option<(JobId, JobState, f64)> {
        let id = *self.running().get(key)?;
        let info = jobs?.status(id)?;
        (!info.state.is_terminal() && !info.cancel_requested).then_some((
            id,
            info.state,
            info.progress.fraction,
        ))
    }

    /// Queues the generation of `asset`'s proxy after `after` unless one is queued or running.
    fn queue(&self, jobs: &JobManager, asset: &MediaAsset, after: &[JobId]) -> Option<JobId> {
        let (hash, profile) = (hash_of(asset)?, self.profile?);
        let path = PathBuf::from(asset.path_abs.as_ref()?);
        let key = (hash, profile.key());
        let mut running = self.running();
        // A job asked to stop (a project swap cancels every job) is no longer the one making
        // it: it ends at its next frame, and a new one continues from the finished parts.
        if let Some(id) = running.get(&key)
            && jobs
                .status(*id)
                .is_some_and(|i| !i.state.is_terminal() && !i.cancel_requested)
        {
            return Some(*id);
        }
        let source = ProxySource {
            path,
            hash,
            probe: asset.probe.clone(),
        };
        let (done_jobs, done_key) = (Arc::clone(&self.jobs), key.clone());
        let media = asset.id;
        let handle = submit_proxy(
            jobs,
            Arc::clone(&self.generator),
            source,
            profile,
            Some(media),
            after.to_vec(),
            Box::new(move |result| {
                match result {
                    Ok(report) => tracing::info!(
                        media = %media,
                        frames = report.proxy.frames,
                        bytes = report.proxy.bytes,
                        reused_parts = report.reused_parts,
                        evicted = report.evicted.len(),
                        "proxy ready"
                    ),
                    Err(e) => tracing::warn!(media = %media, error = %e, "proxy not made"),
                }
                done_jobs
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(&done_key);
            }),
        );
        running.insert(key, handle.id());
        Some(handle.id())
    }

    /// The status of `asset` (with `jobs` to read a generation's progress).
    fn info(&self, jobs: Option<&JobManager>, asset: &MediaAsset) -> ProxyInfo {
        let mut info = ProxyInfo::none(asset.id);
        if !proxyable(asset) {
            info.reason = Some("only video files have proxies".to_owned());
            return info;
        }
        let reason = wants_proxy(DEFAULT_POLICY, asset);
        info.wanted = reason.is_some();
        info.reason = reason;
        let (Some(hash), Some(profile)) = (hash_of(asset), self.profile) else {
            if self.profile.is_none() {
                info.reason = Some("this engine cannot encode proxies".to_owned());
            }
            return info;
        };
        let key = profile.key();
        info.profile = Some(key.clone());
        if let Some((id, state, progress)) = self.job_of(jobs, &(hash, key.clone())) {
            info.job_id = Some(id);
            if state == JobState::Running {
                info.status = ProxyStatus::Generating;
                info.progress = Some(progress);
            } else {
                info.status = ProxyStatus::Queued;
            }
            return info;
        }
        match self.store.get(&hash, &key) {
            Ok(Some(record)) => fill(&mut info, &record),
            Ok(None) => {}
            Err(e) => info.error = Some(e.to_string()),
        }
        info
    }

    /// Queues what the policy wants for `doc` and was never made or was interrupted (engine
    /// start, `project.open`: generations resume after a restart).
    pub(crate) fn reconcile(&self, doc: &Project) {
        let (Some(jobs), Some(profile)) = (self.link.jobs(), self.profile) else {
            return;
        };
        let key = profile.key();
        let mut queued = 0;
        for asset in doc.media.values() {
            if wants_proxy(DEFAULT_POLICY, asset).is_none() {
                continue;
            }
            let Some(hash) = hash_of(asset) else { continue };
            let state = match self.store.get(&hash, &key) {
                Ok(row) => row.map(|r| r.state),
                Err(e) => {
                    tracing::debug!(error = %e, "cannot read a proxy row");
                    continue;
                }
            };
            if matches!(state, None | Some(ProxyState::Building))
                && self.queue(&jobs, asset, &[]).is_some()
            {
                queued += 1;
            }
        }
        if queued > 0 {
            tracing::info!(queued, "proxies queued for the open project");
        }
    }
}

fn join_reconciles(threads: Vec<JoinHandle<()>>) {
    for thread in threads {
        if thread.join().is_err() {
            tracing::error!("a proxy reconcile panicked");
        }
    }
}

/// Copies a stored row into `info`.
fn fill(info: &mut ProxyInfo, record: &ProxyRecord) {
    info.status = match record.state {
        ProxyState::Building => ProxyStatus::Incomplete,
        ProxyState::Ready if record.path.is_file() => ProxyStatus::Ready,
        ProxyState::Ready | ProxyState::Evicted => ProxyStatus::Evicted,
        ProxyState::Failed => ProxyStatus::Failed,
    };
    if info.status == ProxyStatus::Ready {
        info.path = Some(record.path.to_string_lossy().into_owned());
        info.bytes = Some(record.bytes);
    }
    if record.size.0 > 0 {
        info.size = Some([record.size.0, record.size.1]);
    }
    info.error.clone_from(&record.error);
}

impl ProxyService for FullProxies {
    fn status(&self, doc: &Project, media: &[MediaId]) -> Vec<ProxyInfo> {
        let jobs = self.link.jobs();
        media
            .iter()
            .map(|id| match doc.media.get(id) {
                Some(asset) => self.info(jobs.as_ref(), asset),
                None => ProxyInfo::none(*id),
            })
            .collect()
    }

    fn generate(&self, doc: &Project, media: &[MediaId]) -> Result<Vec<ProxyInfo>, CmdError> {
        if self.profile.is_none() {
            return Err(CmdError::unsupported(
                "this engine's media backend cannot encode proxies",
            ));
        }
        let jobs = self
            .link
            .jobs()
            .ok_or_else(|| CmdError::unsupported("the engine is stopping"))?;
        for id in media {
            let Some(asset) = doc.media.get(id) else {
                continue;
            };
            if hash_of(asset).is_none() {
                return Err(CmdError::invalid(
                    "/media",
                    format!("media `{id}` has no content hash to key its proxy"),
                ));
            }
            if self.ready(asset).is_none() {
                self.queue(&jobs, asset, &[]);
            }
        }
        Ok(self.status(doc, media))
    }

    fn delete(&self, doc: &Project, media: &[MediaId]) -> Result<Vec<ProxyInfo>, CmdError> {
        let jobs = self.link.jobs();
        let Some(profile) = self.profile else {
            return Ok(self.status(doc, media));
        };
        let key = profile.key();
        for id in media {
            let Some(hash) = doc.media.get(id).and_then(hash_of) else {
                continue;
            };
            let k = (hash, key.clone());
            let job = self.running().remove(&k);
            if let (Some(job), Some(jobs)) = (job, jobs.as_ref()) {
                jobs.cancel(job);
                let _ = jobs.wait(job, DELETE_WAIT);
            }
            self.store
                .evict(&hash, &key)
                .map_err(|e| failure("proxies", &e))?;
        }
        Ok(self.status(doc, media))
    }

    fn imported(&self, media: MediaId, after: &[JobId]) -> ImportProxy {
        let Some(snapshot) = self.doc.snapshot() else {
            return ImportProxy::None;
        };
        let Some(asset) = snapshot.doc.media.get(&media) else {
            return ImportProxy::None;
        };
        if self.ready(asset).is_some() {
            return ImportProxy::Ready;
        }
        let (Some(jobs), Some(profile), Some(hash)) =
            (self.link.jobs(), self.profile, hash_of(asset))
        else {
            return ImportProxy::None;
        };
        if wants_proxy(DEFAULT_POLICY, asset).is_none() {
            return ImportProxy::None;
        }
        // An evicted or failed proxy is made again only on request (no thrashing).
        let state = self.store.get(&hash, &profile.key()).ok().flatten();
        if state.is_some_and(|r| matches!(r.state, ProxyState::Evicted | ProxyState::Failed)) {
            return ImportProxy::None;
        }
        match self.queue(&jobs, asset, after) {
            Some(_) => ImportProxy::Queued,
            None => ImportProxy::None,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use cutweave_cache::FrameCache;
    use cutweave_commands::{DocActor, DocConfig, Registry};
    use cutweave_media::testing::{FakeBackend, FakeMedia, FakeVideo};
    use cutweave_media::{CancelToken, Capabilities, CodecSupport, DecodeMode, HashMode};
    use cutweave_model::{ColorTags, Duration, FrameRate, MediaKind, StreamKind, Time};
    use cutweave_render::{FrameRequest, FrameSource};

    use super::*;
    use crate::full::frames::{MediaFrames, PreviewFrames};

    struct Manual(AtomicU64);

    impl Clock for Manual {
        fn now_ms(&self) -> u64 {
            self.0.load(Ordering::Relaxed)
        }
    }

    #[test]
    fn proxy_work_waits_while_pictures_are_served_and_goes_on_after_the_idle_time() {
        let clock = Arc::new(Manual(AtomicU64::new(10_000)));
        let activity = Arc::new(Activity::new(clock.clone(), IDLE_BEFORE_PROXIES));
        // Nothing served yet: proxies run.
        assert!(!activity.busy());
        activity.touch();
        assert!(activity.busy());
        clock.0.store(10_000 + 1_499, Ordering::Relaxed);
        assert!(activity.busy());
        // Playback keeps touching: still busy past the first touch's idle time.
        activity.touch();
        clock.0.store(10_000 + 2_000, Ordering::Relaxed);
        assert!(activity.busy());
        clock.0.store(10_000 + 1_499 + 1_500, Ordering::Relaxed);
        assert!(!activity.busy());

        // A waiting job is released by the idle time, and by a cancel.
        activity.touch();
        let cancel = CancelToken::new();
        let waiter = {
            let (activity, cancel) = (Arc::clone(&activity), cancel.clone());
            std::thread::spawn(move || activity.wait(&cancel))
        };
        std::thread::sleep(StdDuration::from_millis(120));
        assert!(!waiter.is_finished(), "paused while busy");
        clock.0.fetch_add(1_500, Ordering::Relaxed);
        waiter.join().unwrap();

        activity.touch();
        let waiter = {
            let (activity, cancel) = (Arc::clone(&activity), cancel.clone());
            std::thread::spawn(move || activity.wait(&cancel))
        };
        cancel.cancel();
        waiter.join().unwrap();
    }

    fn codec(name: &str, kind: StreamKind) -> CodecSupport {
        CodecSupport {
            name: name.to_owned(),
            kind,
            decoders: vec![name.to_owned()],
            encoders: vec![name.to_owned()],
        }
    }

    #[test]
    fn scrub_requests_use_a_ready_proxy_from_the_next_frame_and_others_the_original() {
        let dir =
            std::env::temp_dir().join(format!("cutweave-proxy-select-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("wide.mp4");
        std::fs::write(&path, "wide").unwrap();
        let media = FakeMedia::video(FakeVideo::lasting(
            2560,
            1440,
            FrameRate::FPS_30,
            Duration::from_secs(2).unwrap(),
        ));
        let probe = media.probe().unwrap();
        let backend: Arc<dyn MediaBackend> = Arc::new(
            FakeBackend::new()
                .with_capabilities(Capabilities::with_codecs([
                    codec("rawvideo", StreamKind::Video),
                    codec("prores", StreamKind::Video),
                ]))
                .with_output_files()
                .with_media(&path, media),
        );
        let id = MediaId::from_u128(7);
        let mut asset = MediaAsset::file(id, MediaKind::Video, path.to_string_lossy(), probe);
        let hash = cutweave_media::content_hash(&path, HashMode::Fast).unwrap();
        asset.content_hash = Some(hash.to_string());
        let mut project = Project::new("proxy selection");
        project.media.insert(id, asset.clone());
        let actor = DocActor::spawn(DocConfig::new(Arc::new(Registry::new()), project)).unwrap();
        let current = CurrentDoc::default();
        current.set(Some(actor.handle().clone()));
        let db = Arc::new(CacheDb::open_in_memory().unwrap());
        let proxies = Arc::new(FullProxies::new(
            &backend,
            Arc::clone(&db),
            dir.join("proxies"),
            current.clone(),
            EngineLink::detached(),
        ));
        let frames = Arc::new(
            MediaFrames::new(
                Some(Arc::clone(&backend)),
                current,
                Arc::new(FrameCache::new(64 << 20)),
                Some(db),
                Arc::default(),
            )
            .with_proxies(Some(Arc::clone(&proxies))),
        );
        let preview = PreviewFrames::new(frames);
        let snapshot = actor.handle().snapshot();
        let cancel = cutweave_playback::transport::CancelToken::never();
        let request = |proxy: bool, t: Time| FrameRequest {
            media: id,
            stream: Some(0),
            source_time: t,
            source_size: Some((2560, 1440)),
            rotation: 0,
            color: ColorTags::default(),
            still: false,
            proxy,
        };
        let size = |proxy: bool, t: Time| {
            let f = preview
                .request(&snapshot, &cancel, DecodeMode::Seek)
                .frame(&request(proxy, t))
                .unwrap();
            (f.width, f.height)
        };
        let t = FrameRate::FPS_30.frame_to_time(17).unwrap();
        // No proxy yet: the original.
        assert_eq!(size(true, t), (2560, 1440));
        // Made while the file is open: the next scrub frame uses it.
        let source = ProxySource {
            path: path.clone(),
            hash,
            probe: asset.probe.clone(),
        };
        proxies
            .generator
            .generate(
                &source,
                &proxies.profile.unwrap(),
                &CancelToken::new(),
                &mut |_| {},
            )
            .unwrap();
        assert_eq!(size(true, t), (960, 540));
        assert_eq!(
            size(true, FrameRate::FPS_30.frame_to_time(18).unwrap()),
            (960, 540)
        );
        // Requests that do not accept a proxy (paused frames, exports, agent frames) get the
        // original.
        assert_eq!(size(false, t), (2560, 1440));
        // Deleted: the next scrub frame is the original's again.
        proxies.delete(&snapshot.doc, &[id]).unwrap();
        assert_eq!(size(true, t), (2560, 1440));
        drop(actor);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
