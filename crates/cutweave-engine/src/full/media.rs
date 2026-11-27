// SPDX-License-Identifier: GPL-3.0-or-later
//! The media half of the full engine (WP-1I.05; arch §7): the prober `media.import` uses, the
//! background analysis stages (frame index → thumbnails, waveform) and the caches they fill,
//! and [`MediaState`], what the `/media` HTTP endpoints of WP-1I.03 serve.
//!
//! - [`BackendProbe`] probes with the [`MediaBackend`] and fingerprints the file with the fast
//!   BLAKE3 content hash (`blake3-fast:<hex>`), the key of every cache entry.
//! - Stages (`media.import` queues one job per stage that applies, WP-1D.07): `index` builds
//!   the frame index into `cache.sqlite` (announces `media.indexed`), `thumbnails` generates
//!   the poster and the filmstrip (video only, after the index), `waveform` the peak pyramid
//!   (media with sound; a file without audio is not an error).
//! - The caches are global and live in the OS cache directory, never in a project (arch §7,
//!   §17): `<cache>/cutweave/cache.sqlite`, `<cache>/cutweave/media-cache` (thumbnails and
//!   filmstrips) and `<cache>/cutweave/waveforms`.

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, PoisonError, RwLock};

use cutweave_cache::{
    CacheDb, CacheError, ContentHash, FrameCache, MediaRef, PrefetchSpec, ThumbCache, ThumbConfig,
    WaveformCache, WaveformRequest,
};
use cutweave_commands::{DocHandle, DocSnapshot};
use cutweave_jobs::{JobCtx, JobError};
use cutweave_media::{CancelCheck, CancelToken, HashMode, MediaBackend, MediaError};
use cutweave_model::{MediaId, MediaKind, ProbeInfo, StreamKind};
use cutweave_ops::media::{AnalysisInput, AnalysisStage, MediaProbe, ProbeError, Probed};
use cutweave_render::query::SharedBackend;

/// Cap of the frame indexes in `cache.sqlite`.
pub const INDEX_CACHE_BYTES: u64 = 256 << 20;

/// The document the media services resolve media ids against: the engine's current one,
/// swapped with every project change. Reads are a short read lock and a snapshot load.
#[derive(Clone, Default)]
pub struct CurrentDoc(Arc<RwLock<Option<DocHandle>>>);

impl std::fmt::Debug for CurrentDoc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CurrentDoc")
            .field("set", &self.handle().is_some())
            .finish()
    }
}

impl CurrentDoc {
    /// Installs `doc` (or none).
    pub fn set(&self, doc: Option<DocHandle>) {
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = doc;
    }

    /// The current document handle.
    #[must_use]
    pub fn handle(&self) -> Option<DocHandle> {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The live snapshot of the current document.
    #[must_use]
    pub fn snapshot(&self) -> Option<Arc<DocSnapshot>> {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(DocHandle::snapshot)
    }
}

/// A media asset of the current document, as the media services see it.
#[derive(Clone, Debug, PartialEq)]
pub struct MediaFile {
    /// The asset.
    pub id: MediaId,
    /// Its absolute path.
    pub path: PathBuf,
    /// Its content hash (the cache key), when it parses as one of `cutweave-media`'s.
    pub hash: Option<ContentHash>,
    /// What the import probe found (rotation, streams, duration).
    pub probe: ProbeInfo,
}

impl MediaFile {
    /// The asset `id` of `snapshot`, if it has a file.
    #[must_use]
    ///
    /// This is the **original** (the file, or the locator of an image sequence, ADR-0035):
    /// what agent frames, thumbnails, filmstrips, waveforms and analysis read. Only a preview
    /// picture is ever read from a proxy, and that is the `MediaResolver`'s decision in
    /// `PreviewFrames` (WP-2F.02).
    pub fn of(snapshot: &DocSnapshot, id: MediaId) -> Option<MediaFile> {
        let asset = snapshot.doc.media.get(&id)?;
        let path = crate::export::sources::export_path(asset)?;
        Some(MediaFile {
            id,
            path: PathBuf::from(path),
            hash: asset
                .content_hash
                .as_deref()
                .and_then(|h| ContentHash::from_str(h).ok()),
            probe: asset.probe.clone(),
        })
    }
}

/// What the media HTTP endpoints (`/media/{id}/thumb`, `/filmstrip`, `/waveform`,
/// `/media/frame`; WP-1I.03, arch §6) work with. Cheap to clone; every method is blocking
/// (call it on the blocking pool).
#[derive(Clone)]
pub struct MediaState {
    /// Decoding (and probing) backend.
    pub backend: Arc<dyn MediaBackend>,
    /// Thumbnails and filmstrips (`ThumbCache::thumb`, `filmstrip`, their ETags), when the
    /// cache directory is usable.
    pub thumbs: Option<Arc<ThumbCache>>,
    /// Waveform pyramids (`WaveformCache::waveform` reads; `ensure` computes, as a job).
    pub waveforms: Option<Arc<WaveformCache>>,
    /// `cache.sqlite` (frame indexes).
    pub db: Option<Arc<CacheDb>>,
    /// The RAM frame cache shared with preview and `render.frame`.
    pub frames: Arc<FrameCache>,
    /// The renderer of `render.frame` (for `/media/frame`: run `render_frame` with it).
    pub renderer: SharedBackend,
    pub(crate) doc: CurrentDoc,
}

impl std::fmt::Debug for MediaState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaState")
            .field("thumbs", &self.thumbs.is_some())
            .field("waveforms", &self.waveforms.is_some())
            .field("db", &self.db.is_some())
            .field("doc", &self.doc)
            .finish_non_exhaustive()
    }
}

impl MediaState {
    /// The asset `id` of the current document (path, content hash, probe), if it has a file.
    #[must_use]
    pub fn media(&self, id: MediaId) -> Option<MediaFile> {
        MediaFile::of(&*self.doc.snapshot()?, id)
    }

    /// The live snapshot of the current document (`/media/frame` renders it).
    #[must_use]
    pub fn snapshot(&self) -> Option<Arc<DocSnapshot>> {
        self.doc.snapshot()
    }

    /// The cache reference of `file`: its registered hash, or a fast hash of the file, and the
    /// probe's display rotation (thumbnails are upright, as the preview shows the picture).
    pub fn media_ref(file: &MediaFile) -> Result<MediaRef, CacheError> {
        let reference = match file.hash {
            Some(hash) => MediaRef::new(file.path.clone(), hash),
            None => MediaRef::from_path(file.path.clone())?,
        };
        Ok(reference.with_rotation(file.probe.rotation))
    }
}

// ----- caches ----------------------------------------------------------------------------------

/// The disk caches (any of them may be missing when the cache directory is unusable).
#[derive(Clone, Default)]
pub(crate) struct Caches {
    /// The folder the caches live in (also when it could not be created).
    pub(crate) root: Option<PathBuf>,
    pub(crate) db: Option<Arc<CacheDb>>,
    pub(crate) thumbs: Option<Arc<ThumbCache>>,
    pub(crate) waveforms: Option<Arc<WaveformCache>>,
}

impl Caches {
    /// Opens the caches under `root` (`None`: the OS cache directory). Problems are returned
    /// as a reason; what could be opened is kept.
    pub(crate) fn open(
        backend: &Arc<dyn MediaBackend>,
        root: Option<&Path>,
    ) -> (Caches, Option<String>) {
        let root = root
            .map(Path::to_path_buf)
            .or_else(|| dirs_cache().map(|d| d.join("cutweave")));
        let Some(root) = root else {
            return (
                Caches::default(),
                Some("the OS reports no cache directory".into()),
            );
        };
        if let Err(e) = std::fs::create_dir_all(&root) {
            return (
                Caches {
                    root: Some(root.clone()),
                    ..Caches::default()
                },
                Some(format!("cannot create {}: {e}", root.display())),
            );
        }
        let mut problem = None;
        let db = match CacheDb::open_or_reset(&root.join("cache.sqlite")) {
            Ok(db) => Some(Arc::new(db)),
            Err(e) => {
                problem = Some(format!("cache.sqlite: {e}"));
                None
            }
        };
        let thumbs = db.as_ref().map(|db| {
            Arc::new(ThumbCache::new(
                Arc::clone(backend),
                Arc::clone(db),
                ThumbConfig::new(root.join("media-cache")),
            ))
        });
        let waveforms = Some(Arc::new(WaveformCache::new(root.join("waveforms"))));
        (
            Caches {
                root: Some(root),
                db,
                thumbs,
                waveforms,
            },
            problem,
        )
    }
}

/// The OS cache directory (`ThumbConfig::default_dir` without its last two parts), else the one
/// the environment names (`WaveformCache::default_dir`: `%LOCALAPPDATA%`, `~/Library/Caches`,
/// `$XDG_CACHE_HOME` or `~/.cache`), which also works when the shell's known folder does not
/// exist yet (a fresh or redirected profile).
fn dirs_cache() -> Option<PathBuf> {
    // Both are `<cache dir>/cutweave/<what>`.
    ThumbConfig::default_dir()
        .or_else(WaveformCache::default_dir)
        .and_then(|d| d.parent().and_then(Path::parent).map(Path::to_path_buf))
}

/// The machine's RAM in bytes, for the frame cache budget (`FrameCache::default_budget`).
/// `None` where it cannot be read without FFI (Windows: the budget stays at 1 GiB).
#[must_use]
pub fn total_ram() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let text = std::fs::read_to_string("/proc/meminfo").ok()?;
        let line = text.lines().find(|l| l.starts_with("MemTotal:"))?;
        let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
        Some(kib * 1024)
    }
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .ok()?;
        String::from_utf8_lossy(&out.stdout).trim().parse().ok()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

// ----- probing ---------------------------------------------------------------------------------

/// `media.import`'s prober over a [`MediaBackend`]: its probe plus the fast content hash.
pub struct BackendProbe {
    backend: Arc<dyn MediaBackend>,
}

impl BackendProbe {
    /// A prober over `backend`.
    #[must_use]
    pub fn new(backend: Arc<dyn MediaBackend>) -> BackendProbe {
        BackendProbe { backend }
    }
}

impl std::fmt::Debug for BackendProbe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackendProbe").finish_non_exhaustive()
    }
}

fn probe_error(path: &str, e: MediaError) -> ProbeError {
    match e {
        MediaError::NotFound { .. } => ProbeError::NotFound(path.to_owned()),
        MediaError::Io { message, .. } => ProbeError::Io {
            path: path.to_owned(),
            reason: message,
        },
        other => ProbeError::Unsupported {
            path: path.to_owned(),
            reason: other.to_string(),
        },
    }
}

impl MediaProbe for BackendProbe {
    fn probe(&self, path: &str) -> Result<Probed, ProbeError> {
        let file = Path::new(path);
        let probe = self.backend.probe(file).map_err(|e| probe_error(path, e))?;
        let hash =
            cutweave_media::content_hash(file, HashMode::Fast).map_err(|e| probe_error(path, e))?;
        Ok(Probed {
            probe,
            content_hash: hash.to_string(),
        })
    }
}

// ----- analysis stages -------------------------------------------------------------------------

/// The cache key of an analysed asset: its registered hash, else a fast hash of the file.
fn hash_of(media: &AnalysisInput) -> Result<ContentHash, JobError> {
    match ContentHash::from_str(&media.content_hash) {
        Ok(hash) => Ok(hash),
        Err(_) => cutweave_media::content_hash(Path::new(&media.path), HashMode::Fast)
            .map_err(|e| JobError::failed(e.to_string())),
    }
}

/// A media cancel token that follows the job's.
fn job_cancel(ctx: &JobCtx) -> CancelToken {
    let token = ctx.token().clone();
    CancelToken::linked(CancelCheck::new(move || token.is_cancelled()))
}

fn cache_failure(e: CacheError) -> JobError {
    match e {
        CacheError::Media(MediaError::Cancelled) => JobError::Cancelled,
        other => JobError::failed(other.to_string()),
    }
}

fn has_stream(probe: &ProbeInfo, kind: StreamKind) -> bool {
    probe.streams.iter().any(|s| s.kind == kind)
}

/// A video file or an image sequence: media with pictures over time.
fn moving_picture(media: &AnalysisInput) -> bool {
    matches!(media.kind, MediaKind::Video | MediaKind::ImageSequence)
        && has_stream(&media.probe, StreamKind::Video)
}

/// `index`: the frame index of a video, into `cache.sqlite` (announces `media.indexed`).
pub(crate) struct IndexStage {
    pub(crate) backend: Arc<dyn MediaBackend>,
    pub(crate) db: Arc<CacheDb>,
}

impl AnalysisStage for IndexStage {
    fn name(&self) -> &'static str {
        "index"
    }

    fn builds_index(&self) -> bool {
        true
    }

    fn applies_to(&self, media: &AnalysisInput) -> bool {
        // An image sequence is video here too: its index is computed, not scanned (ADR-0035).
        moving_picture(media)
    }

    fn run(&self, media: &AnalysisInput, ctx: &JobCtx) -> Result<(), JobError> {
        ctx.check()?;
        let hash = hash_of(media)?;
        ctx.progress(0.0, Some("index"));
        self.db
            .frame_index_for(&*self.backend, Path::new(&media.path), &hash)
            .map_err(cache_failure)?;
        if let Err(e) = self.db.evict_lru(INDEX_CACHE_BYTES) {
            tracing::debug!(error = %e, "frame index eviction failed");
        }
        ctx.progress(1.0, Some("index"));
        Ok(())
    }
}

/// `thumbnails`: the poster and the filmstrip of a video (after its index).
pub(crate) struct ThumbStage {
    pub(crate) thumbs: Arc<ThumbCache>,
}

impl AnalysisStage for ThumbStage {
    fn name(&self) -> &'static str {
        "thumbnails"
    }

    fn requires(&self) -> &'static [&'static str] {
        &["index"]
    }

    fn applies_to(&self, media: &AnalysisInput) -> bool {
        // Audio-only media would fail with `NoStream` (WP-1D.05 handoff).
        moving_picture(media)
    }

    fn run(&self, media: &AnalysisInput, ctx: &JobCtx) -> Result<(), JobError> {
        ctx.check()?;
        let reference = MediaRef::new(PathBuf::from(&media.path), hash_of(media)?)
            .with_rotation(media.probe.rotation);
        let cancel = job_cancel(ctx);
        self.thumbs
            .prefetch(&reference, PrefetchSpec::default(), &cancel, &mut |f| {
                ctx.progress(f, Some("thumbnails"));
            })
            .map_err(cache_failure)
    }
}

/// `waveform`: the peak pyramid of media with sound.
pub(crate) struct WaveformStage {
    pub(crate) backend: Arc<dyn MediaBackend>,
    pub(crate) waveforms: Arc<WaveformCache>,
}

impl AnalysisStage for WaveformStage {
    fn name(&self) -> &'static str {
        "waveform"
    }

    fn applies_to(&self, media: &AnalysisInput) -> bool {
        matches!(media.kind, MediaKind::Video | MediaKind::Audio)
            && has_stream(&media.probe, StreamKind::Audio)
    }

    fn run(&self, media: &AnalysisInput, ctx: &JobCtx) -> Result<(), JobError> {
        ctx.check()?;
        let hash = hash_of(media)?;
        let request = WaveformRequest {
            stream: None,
            per_channel: false,
            duration_hint: media.probe.duration,
        };
        let cancel = job_cancel(ctx);
        let result = self.waveforms.ensure(
            &*self.backend,
            Path::new(&media.path),
            &hash,
            &request,
            &cancel,
            &mut |f| ctx.progress(f, Some("waveform")),
        );
        match result {
            // No audio means no waveform, not a failed analysis (WP-1D.06 handoff).
            Ok(_) | Err(CacheError::Media(MediaError::NoStream { .. })) => {}
            Err(e) => return Err(cache_failure(e)),
        }
        if let Err(e) = self.waveforms.enforce_limit() {
            tracing::debug!(error = %e, "waveform eviction failed");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use cutweave_media::testing::{FakeBackend, FakeMedia};

    #[test]
    fn the_backend_probe_reports_streams_and_a_fast_content_hash() {
        let dir = std::env::temp_dir().join(format!("cutweave-full-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.mp4");
        std::fs::write(&file, b"not really a video").unwrap();
        let path = file.to_string_lossy().into_owned();
        let clip = FakeMedia::clip(
            cutweave_model::Duration::from_secs(2).unwrap(),
            cutweave_model::FrameRate::FPS_30,
        );
        let fake = FakeBackend::new().with_media(&path, clip);
        let probe = BackendProbe::new(Arc::new(fake));
        let probed = probe.probe(&path).unwrap();
        assert!(probed.content_hash.starts_with("blake3-fast:"));
        assert_eq!(probed.probe.streams.len(), 2);
        let missing = dir.join("missing.mp4").to_string_lossy().into_owned();
        assert!(matches!(
            probe.probe(&missing),
            Err(ProbeError::NotFound(_) | ProbeError::Unsupported { .. })
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
