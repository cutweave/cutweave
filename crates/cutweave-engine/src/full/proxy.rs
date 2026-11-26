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
//! - **Size and jobs** (WP-2F.03b). The longest side of new proxies (540, 720 or 960 pixels)
//!   and how many proxy jobs run at once are this computer's settings
//!   (`media.set_proxy_config`, UI-only). Another size is another profile, so another proxy:
//!   a change stops the generations of the old size, queues what the project wants at the new
//!   one, and leaves ready proxies of other sizes in the cache until its limit removes them.
//! - **Policy.** [`DEFAULT_POLICY`] (`auto`) for every project until ADR-0027 stores one in
//!   the project.
//! - **Reversed clips** (WP-2D.01b, [`FullProxies::reversed`]). Reverse playback reads the
//!   original back in windows of [`READBACK_BYTES`](super::frames::READBACK_BYTES); a file whose
//!   keyframes are further apart than a window holds at its size (from the stored frame index;
//!   H.264 and HEVC count as such until it is stored) costs a decode from the keyframe per
//!   window. When a clip plays such a file backwards and the file has no proxy, the engine
//!   queues one (whatever the policy; once per file per session, never again after it was
//!   evicted or failed) and warns while it builds: a log line and `media.reverse_slow`
//!   `{media_id, reason, job_id}`. `media.proxy_status` reports it `wanted` with that reason.
//!   The document watcher checks after every edit; attaching a document checks its clips.

use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration as StdDuration, Instant};

use cutweave_cache::proxy::{
    GenerateOptions, Pacer, ProxyCodec, ProxyGenerator, ProxyProfile, ProxyRecord, ProxySource,
    ProxyState, ProxyStore, ReadyProxy, submit_proxy,
};
use cutweave_cache::{CacheDb, ContentHash};
use cutweave_commands::{CmdError, DocHandle};
use cutweave_jobs::{JobId, JobManager, JobState};
use cutweave_media::{CancelToken, MediaBackend};
use cutweave_model::{Item, MediaAsset, MediaId, Project, StreamKind, TrackKind};
use cutweave_ops::media::proxy::{
    DEFAULT_POLICY, ImportProxy, ProxyInfo, ProxyService, ProxyStatus, proxyable, wants_proxy,
};

use super::frames::readback_frames;
use super::media::CurrentDoc;
use crate::subsystems::{EngineLink, REVERSE_SLOW_EVENT, SubsystemEvents};

/// How long `media.delete_proxy` waits for a cancelled generation to stop before removing the
/// files.
const DELETE_WAIT: StdDuration = StdDuration::from_secs(5);

type Key = (ContentHash, String);

/// Codecs whose streams count as long-GOP for reverse playback while their frame index is not
/// stored yet (FFmpeg names).
const LONG_GOP_CODECS: [&str; 2] = ["h264", "hevc"];

/// The media that clips of `doc` play backwards on video tracks (freeze frames excluded).
fn reversed_media(doc: &Project) -> Vec<MediaId> {
    let mut media: Vec<MediaId> = doc
        .sequences
        .values()
        .flat_map(|s| s.tracks.values())
        .filter(|t| t.kind == TrackKind::Video)
        .flat_map(|t| t.items.iter())
        .filter_map(|item| match item {
            Item::Clip(c) if c.reverse && c.freeze.is_none() => Some(c.media),
            _ => None,
        })
        .collect();
    media.sort_unstable();
    media.dedup();
    media
}

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
    /// The best proxy codec the backend encodes; `None`: it makes no proxies.
    codec: Option<ProxyCodec>,
    /// The longest side of the proxies made from now on, in pixels.
    long_edge: AtomicU32,
    doc: CurrentDoc,
    link: EngineLink,
    /// Generations queued or running, by original and profile.
    jobs: Arc<Mutex<HashMap<Key, JobId>>>,
    /// Interactive preview activity that pauses generations (issue 153).
    activity: Arc<Activity>,
    /// Reconcile threads not joined yet ([`FullProxies::stop`]).
    reconciles: Mutex<Vec<JoinHandle<()>>>,
    /// Frame indexes (the reverse rule's GOP lengths).
    db: Arc<CacheDb>,
    /// Where `media.reverse_slow` goes.
    events: Option<SubsystemEvents>,
    /// What the reverse rule decided per file this session: why a proxy was asked for, or
    /// `None` (not needed).
    reverse: Mutex<HashMap<ContentHash, Option<String>>>,
}

impl std::fmt::Debug for FullProxies {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FullProxies")
            .field("profile", &self.profile())
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
                Arc::clone(&db),
                Arc::clone(&store),
                GenerateOptions::default(),
            )
            .with_pacer(Arc::clone(&activity) as Arc<dyn Pacer>),
        );
        let codec = generator.profile().map(|p| p.codec);
        if codec.is_none() {
            tracing::info!("the media backend encodes no proxy codec: no proxies are made");
        }
        FullProxies {
            store,
            generator,
            codec,
            long_edge: AtomicU32::new(ProxyProfile::DEFAULT_LONG_EDGE),
            doc,
            link,
            jobs: Arc::default(),
            activity,
            reconciles: Mutex::default(),
            db,
            events: None,
            reverse: Mutex::default(),
        }
    }

    /// Makes `asset`'s proxy now, on this thread (tests); its record.
    #[cfg(all(test, feature = "ffmpeg"))]
    #[allow(clippy::expect_used)]
    pub(crate) fn generate_now(&self, asset: &MediaAsset) -> ProxyRecord {
        let source = ProxySource {
            path: PathBuf::from(asset.source_path().unwrap_or_default()),
            hash: hash_of(asset).expect("a content hash"),
            probe: asset.probe.clone(),
        };
        let profile = self.profile().expect("a proxy codec");
        self.generator
            .generate(&source, &profile, &CancelToken::new(), &mut |_| {})
            .expect("the proxy is made")
            .proxy
    }

    /// How proxies are made now: the backend's best proxy codec at the size in force; `None`
    /// when the backend encodes no proxy codec.
    fn profile(&self) -> Option<ProxyProfile> {
        self.codec.map(|codec| ProxyProfile {
            codec,
            long_edge: self.long_edge.load(Ordering::Relaxed),
        })
    }

    /// The longest side of the proxies made from now on, in pixels.
    pub(crate) fn long_edge(&self) -> u32 {
        self.long_edge.load(Ordering::Relaxed)
    }

    /// How many proxy jobs run at once (the engine's job manager; the default while it is not
    /// reachable).
    pub(crate) fn job_slots(&self) -> usize {
        self.link
            .jobs()
            .map_or(cutweave_jobs::DEFAULT_PROXY_SLOTS, |jobs| {
                jobs.proxy_slots()
            })
    }

    /// Sets how many proxy jobs run at once.
    pub(crate) fn set_job_slots(&self, slots: usize) {
        if let Some(jobs) = self.link.jobs() {
            jobs.set_proxy_slots(slots);
        }
    }

    /// Sets the longest side of the proxies made from now on. A change stops the generations
    /// of the old size (their unfinished files are removed) and queues what the open project
    /// wants at the new size, on the reconcile thread. Ready proxies of other sizes stay in the
    /// cache, unused, until its limit removes them.
    pub(crate) fn set_long_edge(self: &Arc<Self>, long_edge: u32) {
        if self.long_edge.swap(long_edge, Ordering::Relaxed) == long_edge || self.codec.is_none() {
            return;
        }
        let Some(current) = self.profile().map(|p| p.key()) else {
            return;
        };
        let stale: Vec<(Key, JobId)> = {
            let mut running = self.running();
            let stale: Vec<Key> = running
                .keys()
                .filter(|(_, profile)| *profile != current)
                .cloned()
                .collect();
            stale
                .into_iter()
                .filter_map(|key| running.remove(&key).map(|id| (key, id)))
                .collect()
        };
        let snapshot = self.doc.snapshot();
        self.spawn_reconcile(move |proxies| {
            let jobs = proxies.link.jobs();
            for ((hash, profile), id) in stale {
                if let Some(jobs) = &jobs {
                    jobs.cancel(id);
                    let _ = jobs.wait(id, DELETE_WAIT);
                }
                let unfinished = proxies
                    .store
                    .get(&hash, &profile)
                    .ok()
                    .flatten()
                    .is_some_and(|row| row.state == ProxyState::Building);
                if unfinished && let Err(e) = proxies.store.evict(&hash, &profile) {
                    tracing::debug!(error = %e, "cannot remove an unfinished proxy");
                }
            }
            if let Some(snapshot) = snapshot {
                proxies.reconcile(&snapshot.doc);
            }
        });
    }

    /// The same proxies, sending `media.reverse_slow` to `events`.
    pub(crate) fn with_events(mut self, events: SubsystemEvents) -> FullProxies {
        self.events = Some(events);
        self
    }

    /// [`reconcile`](Self::reconcile)s `doc` on its own thread (the per-project hooks call it
    /// under the session lock); [`stop`](Self::stop) joins the thread.
    pub(crate) fn reconcile_in_background(self: &Arc<Self>, doc: &DocHandle) {
        let snapshot = doc.snapshot();
        self.spawn_reconcile(move |proxies| proxies.reconcile(&snapshot.doc));
    }

    /// Runs `work` on a `cw-proxy-reconcile` thread, which [`stop`](Self::stop) joins.
    fn spawn_reconcile(self: &Arc<Self>, work: impl FnOnce(&FullProxies) + Send + 'static) {
        let proxies = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("cw-proxy-reconcile".into())
            .spawn(move || work(&proxies));
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
        let (hash, profile) = (hash_of(asset)?, self.profile()?);
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
        let (hash, profile) = (hash_of(asset)?, self.profile()?);
        // The file, or the locator of an image sequence (ADR-0035).
        let path = PathBuf::from(asset.source_path()?);
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

    /// Why reverse playback asked for `hash`'s proxy this session, if it did.
    fn reverse_reason_of(&self, hash: &ContentHash) -> Option<String> {
        self.reverse
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(hash)
            .cloned()
            .flatten()
    }

    /// Why `asset` (content `hash`) needs a proxy to play backwards smoothly, if it does: its
    /// keyframes are further apart than a read-back window holds at its size.
    fn reverse_reason(&self, asset: &MediaAsset, hash: &ContentHash) -> Option<String> {
        let video = asset
            .probe
            .streams
            .iter()
            .find(|s| s.kind == StreamKind::Video)?;
        let (width, height) = (video.width?, video.height?);
        let window = readback_frames(width, height, video.pixel_format.as_deref());
        if let Ok(Some(index)) = self.db.get_main_frame_index(hash) {
            let (_, _, longest, _) = index.gop_stats();
            (index.max_decode_distance() > 1 && longest > window).then(|| {
                format!(
                    "played in reverse: its keyframes are up to {longest} frames apart, more than \
                     reverse playback keeps decoded at {width}x{height} ({window} frames)"
                )
            })
        } else {
            // Not indexed yet: by codec.
            let codec = video.codec.as_deref()?;
            LONG_GOP_CODECS.contains(&codec).then(|| {
                format!("played in reverse: {codec} keeps its keyframes far apart (long GOP)")
            })
        }
    }

    /// Queues the proxies of the long-GOP media that clips of `doc` play backwards and that
    /// have none (see the module docs), and warns about each: a log line and
    /// `media.reverse_slow`. Each file is decided once per session. Never call it from the
    /// per-project hooks (it reads the cache database and queues jobs): the document watcher
    /// and the reconcile thread do.
    pub(crate) fn reversed(&self, doc: &Project) {
        let Some(profile) = self.profile() else {
            return;
        };
        for media in reversed_media(doc) {
            let Some(asset) = doc.media.get(&media) else {
                continue;
            };
            let Some(hash) = hash_of(asset) else { continue };
            if !proxyable(asset)
                || self
                    .reverse
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .contains_key(&hash)
            {
                continue;
            }
            let reason = self.reverse_reason(asset, &hash);
            {
                // The watcher and a reconcile may decide the same file at once: one warns.
                let mut decided = self.reverse.lock().unwrap_or_else(PoisonError::into_inner);
                if decided.contains_key(&hash) {
                    continue;
                }
                decided.insert(hash, reason.clone());
            }
            let Some(reason) = reason else { continue };
            if self.ready(asset).is_some() {
                continue;
            }
            // An evicted or failed proxy is made again only on request (no thrashing).
            let state = self.store.get(&hash, &profile.key()).ok().flatten();
            let refused =
                state.is_some_and(|r| matches!(r.state, ProxyState::Evicted | ProxyState::Failed));
            let job = if refused {
                None
            } else {
                self.link
                    .jobs()
                    .and_then(|jobs| self.queue(&jobs, asset, &[]))
            };
            tracing::warn!(
                media = %media,
                reason = %reason,
                job = ?job,
                "reverse playback is slower until the proxy is ready"
            );
            if let Some(events) = &self.events {
                events.publish(
                    REVERSE_SLOW_EVENT,
                    serde_json::json!({
                        "media_id": media,
                        "reason": reason,
                        "job_id": job,
                    }),
                );
            }
        }
    }

    /// The status of `asset` (with `jobs` to read a generation's progress).
    fn info(&self, jobs: Option<&JobManager>, asset: &MediaAsset) -> ProxyInfo {
        let mut info = ProxyInfo::none(asset.id);
        if !proxyable(asset) {
            info.reason = Some("only video files have proxies".to_owned());
            return info;
        }
        let reason = wants_proxy(DEFAULT_POLICY, asset)
            .or_else(|| hash_of(asset).and_then(|h| self.reverse_reason_of(&h)));
        info.wanted = reason.is_some();
        info.reason = reason;
        let (Some(hash), Some(profile)) = (hash_of(asset), self.profile()) else {
            if self.codec.is_none() {
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
        let (Some(jobs), Some(profile)) = (self.link.jobs(), self.profile()) else {
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
        self.reversed(doc);
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
        if self.codec.is_none() {
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
        let Some(profile) = self.profile() else {
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
            (self.link.jobs(), self.profile(), hash_of(asset))
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

    use serde_json::{Value, json};

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
                &proxies.profile().unwrap(),
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

    /// WP-2D.01b: the reverse rule, decided once per file per session.
    #[test]
    fn reversed_long_gop_media_ask_for_a_proxy_once_and_short_gops_need_none() {
        use crate::events::EventBus;
        use cutweave_model::{
            Clip, ItemId, SampleRate, Sequence, SequenceFormat, SequenceId, Track, TrackId,
        };

        let dir =
            std::env::temp_dir().join(format!("cutweave-proxy-reverse-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let six = Duration::from_secs(6).unwrap();
        // 1080p: the read-back holds 64 frames. `long` has 120-frame GOPs, `short` the fake's
        // 12, `unindexed` no stored index (H.264 to the probe: long until indexed), `forward`
        // long GOPs but played forwards.
        let clips = [
            ("long", 120, true, true),
            ("short", 12, true, true),
            ("unindexed", 120, false, true),
            ("forward", 120, true, false),
        ];
        let mut backend = FakeBackend::new().with_capabilities(Capabilities::with_codecs([
            codec("rawvideo", StreamKind::Video),
            codec("prores", StreamKind::Video),
        ]));
        let mut project = Project::new("reverse rule");
        let seq = SequenceId::from_u128(1);
        let format = SequenceFormat::new(1920, 1080, FrameRate::FPS_30, SampleRate::HZ_48_000);
        let mut sequence = Sequence::new(seq, "Main", format);
        let v1 = TrackId::from_u128(11);
        sequence.insert_track(0, Track::new(v1, TrackKind::Video, "V1"));
        let db = Arc::new(CacheDb::open_in_memory().unwrap());
        let mut ids = Vec::new();
        for (i, (name, gop, indexed, reversed)) in clips.into_iter().enumerate() {
            let path = dir.join(format!("{name}.mp4"));
            std::fs::write(&path, name).unwrap();
            let media = FakeMedia::video(
                FakeVideo::lasting(1920, 1080, FrameRate::FPS_30, six).with_gop(gop),
            );
            let mut probe = media.probe().unwrap();
            probe.streams[0].codec = Some("h264".into());
            let hash = cutweave_media::content_hash(&path, HashMode::Fast).unwrap();
            if indexed {
                db.put_frame_index(&hash, &media.index(&path).unwrap())
                    .unwrap();
            }
            backend = backend.with_media(&path, media);
            let i = u8::try_from(i).unwrap();
            let id = MediaId::from_u128(100 + u128::from(i));
            let mut asset = MediaAsset::file(id, MediaKind::Video, path.to_string_lossy(), probe);
            asset.content_hash = Some(hash.to_string());
            project.media.insert(id, asset);
            let mut clip = Clip::new(ItemId::from_u128(200 + u128::from(i)), id, Time::ZERO, six);
            clip.start = Time::from_secs(6 * i64::from(i)).unwrap();
            clip.reverse = reversed;
            sequence.track_mut(&v1).unwrap().items.insert(clip.into());
            ids.push(id);
        }
        project.sequences.insert(seq, sequence);
        let backend: Arc<dyn MediaBackend> = Arc::new(backend);
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe();
        let proxies = FullProxies::new(
            &backend,
            db,
            dir.join("proxies"),
            CurrentDoc::default(),
            EngineLink::detached(),
        )
        .with_events(SubsystemEvents(bus));

        proxies.reversed(&project);
        let mut warned = Vec::new();
        while let Ok(e) = rx.try_recv() {
            assert_eq!(e.name(), REVERSE_SLOW_EVENT);
            warned.push(e.payload());
        }
        let warned_ids: Vec<&Value> = warned.iter().map(|w| &w["media_id"]).collect();
        assert_eq!(
            warned_ids,
            [&json!(ids[0]), &json!(ids[2])],
            "long and unindexed: {warned:?}"
        );
        assert!(
            warned[0]["reason"]
                .as_str()
                .unwrap()
                .contains("up to 120 frames apart"),
            "{warned:?}"
        );
        assert!(
            warned[1]["reason"].as_str().unwrap().contains("h264"),
            "{warned:?}"
        );
        // No engine to queue on here.
        assert_eq!(warned[0]["job_id"], Value::Null);
        // Reported by `media.proxy_status`.
        let status = proxies.status(&project, &ids);
        assert!(status[0].wanted && status[2].wanted, "{status:?}");
        assert!(!status[1].wanted && !status[3].wanted, "{status:?}");
        assert!(
            status[0].reason.as_deref().unwrap().contains("reverse"),
            "{status:?}"
        );
        // Once per file per session.
        proxies.reversed(&project);
        assert!(rx.try_recv().is_err(), "warned once");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
