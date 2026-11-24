// SPDX-License-Identifier: GPL-3.0-or-later
//! [`MediaFrames`]: decoded source pictures for the compositor (`cutweave_render::FrameSource`)
//! from one [`DecoderPool`] per media item and the RAM [`FrameCache`] (arch §7, WP-1D.03,
//! WP-1D.05). Preview and `render.frame` share it; exports decode on their own
//! ([`DecoderSources`](crate::DecoderSources)).
//!
//! - **Resolution.** A request names a media id; the file and its content hash come from the
//!   current document ([`CurrentDoc`]), so a relink is seen at the next frame.
//! - **Cache first.** With the pool's frame index (from `cache.sqlite` when the `index` stage
//!   ran, else from the pool's first decoder) the frame shown at a source time is known
//!   before decoding: `(content hash, stream, pts)` is looked up in the [`FrameCache`].
//! - **Decode.** A miss checks a decoder out of the pool near the requested time (a forward
//!   scrub or playback keeps decoding instead of seeking) and seeks frame-exactly. A preview
//!   decode is stopped by the render's cancel token (the playback epoch). Frames arrive in
//!   coded orientation; the render graph rotates them from the probe (`FrameRequest::rotation`).
//! - **Preview against agent frames.** Preview renders ([`PreviewFrames`]) decode with
//!   hardware when the machine has it (`HwMode::Auto`, WP-1D.04) and use the frame cache.
//!   `render.frame` (the [`FrameSource`] impl) decodes in software and bypasses the cache,
//!   exactly as exports do, so an agent frame is bit-identical to the exported one (arch §8).
//! - **Layouts.** A frame in a layout the compositor does not read (10-bit planar, 4:2:2: a
//!   software decoder, or a hardware decoder that fell back) makes the media decode into
//!   `P010` / `Yuv420p` from then on (swscale in the decoder).
//! - **Damage.** A frame that stood in for an undecodable one (`damaged`) is shown; the asset is
//!   logged once.
//!
//! # The preview's render thread (arch §10 "Threads")
//!
//! [`PreviewFrames`] is the preview's own front of [`MediaFrames`], owned by its render thread:
//! per media it keeps the resolved file, pool, frame index and the last picture it served
//! (converted for the compositor), refreshed when the asset's path or content hash changes.
//! Showing a picture it already holds (a still, a paused frame re-rendered after an edit, media
//! slower than the sequence) takes no lock and allocates nothing. A picture it does not hold
//! goes to the media layer ([`Scope::Media`]: the frame cache, the decoder pool, decoding, and
//! the conversion for the compositor), which locks and allocates; moving those decodes off the
//! render thread (decode-ahead into a lock-free hand-off) is follow-up work.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use cutweave_cache::{CacheDb, ContentHash, FrameCache, FrameIndex, FrameKey};
use cutweave_commands::DocSnapshot;
use cutweave_media::{
    CancelCheck, CancelToken, DecodeMode, DecodeOpts, DecoderPool, HwMode, MediaBackend,
    MediaError, PixelFormat, VideoDecoder,
};
use cutweave_model::{MediaId, Project, Time};
use cutweave_render::alloc_scope::{self, Scope};
use cutweave_render::{FrameError, FrameRequest, FrameSource, VideoFrame};

use super::media::{CurrentDoc, MediaFile};
use crate::export::sources::{compositor_frame, compositor_target, media_identity};

/// One media item's decoders.
struct Pooled {
    path: PathBuf,
    hash: Option<ContentHash>,
    pool: Arc<DecoderPool>,
}

/// Decoded source pictures for the compositor (see the module docs).
pub struct MediaFrames {
    backend: Option<Arc<dyn MediaBackend>>,
    doc: CurrentDoc,
    cache: Arc<FrameCache>,
    db: Option<Arc<CacheDb>>,
    pools: Mutex<HashMap<MediaId, Pooled>>,
    damaged: Mutex<HashSet<MediaId>>,
    /// Media whose frames must be converted for the compositor, and into what.
    targets: Mutex<HashMap<MediaId, PixelFormat>>,
}

impl std::fmt::Debug for MediaFrames {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaFrames")
            .field("backend", &self.backend.is_some())
            .field("pools", &self.pool_count())
            .finish_non_exhaustive()
    }
}

fn unavailable(media: MediaId, e: &MediaError) -> FrameError {
    FrameError::Unavailable(format!("media {media}: {e}"))
}

fn no_backend() -> FrameError {
    FrameError::Unavailable("this engine has no media backend to decode with".into())
}

/// How a decode runs: the preview's (cancellable, hardware, through the frame cache) or an
/// agent frame's (`None`: software, uncached, like exports).
type DecodeScope = Option<(CancelToken, DecodeMode)>;

impl MediaFrames {
    /// Frames of the media of `doc`, decoded with `backend`, cached in `cache`, with frame
    /// indexes from `db`.
    pub(crate) fn new(
        backend: Option<Arc<dyn MediaBackend>>,
        doc: CurrentDoc,
        cache: Arc<FrameCache>,
        db: Option<Arc<CacheDb>>,
    ) -> MediaFrames {
        MediaFrames {
            backend,
            doc,
            cache,
            db,
            pools: Mutex::new(HashMap::new()),
            damaged: Mutex::new(HashSet::new()),
            targets: Mutex::new(HashMap::new()),
        }
    }

    /// Media items with a decoder pool.
    #[must_use]
    pub(crate) fn pool_count(&self) -> usize {
        self.pools
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// Drops the pools of media that are gone from the document or whose file or content
    /// changed (a relink), and their cached frames; returns the content hashes dropped.
    pub(crate) fn forget_stale(&self) -> Vec<ContentHash> {
        let snapshot = self.doc.snapshot();
        let mut stale = Vec::new();
        let mut pools = self.pools.lock().unwrap_or_else(PoisonError::into_inner);
        pools.retain(|id, pooled| {
            let current = snapshot.as_ref().and_then(|s| MediaFile::of(s, *id));
            let keep = current
                .as_ref()
                .is_some_and(|m| m.path == pooled.path && m.hash == pooled.hash);
            if !keep && let Some(hash) = pooled.hash {
                stale.push(hash);
            }
            keep
        });
        drop(pools);
        for hash in &stale {
            self.cache.remove_media(hash);
        }
        stale
    }

    /// Closes every decoder and empties the pools (the document closed, or shutdown).
    pub(crate) fn clear(&self) {
        let pools = std::mem::take(&mut *self.pools.lock().unwrap_or_else(PoisonError::into_inner));
        for pooled in pools.values() {
            pooled.pool.trim();
        }
        drop(pools);
        self.targets
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }

    /// The pool of `file` (created on first use, with the stored frame index if any).
    fn pool(&self, backend: &Arc<dyn MediaBackend>, file: &MediaFile) -> Arc<DecoderPool> {
        let mut pools = self.pools.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(p) = pools.get(&file.id)
            && p.path == file.path
            && p.hash == file.hash
        {
            return Arc::clone(&p.pool);
        }
        let mut pool = DecoderPool::new(Arc::clone(backend), file.path.clone());
        if let (Some(db), Some(hash)) = (&self.db, &file.hash)
            && let Ok(Some(index)) = db.get_main_frame_index(hash)
        {
            pool = pool.with_index(Arc::new(index));
        }
        let pool = Arc::new(pool);
        pools.insert(
            file.id,
            Pooled {
                path: file.path.clone(),
                hash: file.hash,
                pool: Arc::clone(&pool),
            },
        );
        pool
    }

    fn flag_damaged(&self, file: &MediaFile) {
        if self
            .damaged
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(file.id)
        {
            tracing::warn!(media = %file.id, path = %file.path.display(), "damaged media: a frame could not be decoded and was replaced");
        }
    }

    /// The decoded frame at media time `t` of `file` (from `pool`), decoded as `scope` says.
    fn decode(
        &self,
        pool: &DecoderPool,
        file: &MediaFile,
        stream: u32,
        t: Time,
        scope: DecodeScope,
    ) -> Result<Arc<cutweave_media::VideoFrame>, FrameError> {
        // Preview uses the frame cache; agent frames decode exactly like exports.
        let cached = scope.is_some() && file.hash.is_some();
        if cached
            && let Some(hash) = file.hash
            // The frame shown at `t` is known from the index: look it up first.
            && let Some(pts) = pool.index().and_then(|index| shown_at(&index, t))
            && let Some(frame) = self.cache.get(&FrameKey { hash, stream, pts })
        {
            return Ok(frame);
        }
        let (cancel, opts) = match scope {
            Some((cancel, mode)) => (cancel, DecodeOpts::default().with_mode(mode)),
            None => (
                CancelToken::new(),
                DecodeOpts {
                    hw: HwMode::Off,
                    ..DecodeOpts::default()
                },
            ),
        };
        let target = self
            .targets
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&file.id)
            .copied();
        let mut frame = Self::seek(pool, file, &opts, target, t, &cancel)?;
        if target.is_none()
            && let Some(target) = compositor_target(frame.format)
        {
            // A layout the compositor does not read: convert this media's frames from now on.
            self.targets
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(file.id, target);
            frame = Self::seek(pool, file, &opts, Some(target), t, &cancel)?;
        }
        if frame.damaged {
            self.flag_damaged(file);
        }
        let frame = Arc::new(frame);
        if cached && let Some(hash) = file.hash {
            self.cache.insert(
                FrameKey {
                    hash,
                    stream,
                    pts: frame.pts,
                },
                Arc::clone(&frame),
            );
        }
        Ok(frame)
    }

    /// Seeks a decoder of `pool` (opened with `opts`, converting to `target`) to `t`.
    fn seek(
        pool: &DecoderPool,
        file: &MediaFile,
        opts: &DecodeOpts,
        target: Option<PixelFormat>,
        t: Time,
        cancel: &CancelToken,
    ) -> Result<cutweave_media::VideoFrame, FrameError> {
        let opts = match target {
            Some(target) => opts.clone().with_target(target),
            None => opts.clone(),
        };
        let mut lease = pool
            .checkout(&opts, Some(t), cancel)
            .map_err(|e| unavailable(file.id, &e))?;
        match lease.seek_exact(t) {
            Ok(frame) => Ok(frame),
            // Past the last frame (a clip outlasting its media): show the last one.
            Err(MediaError::OutOfRange { .. }) => {
                let index = lease.index();
                let last = index
                    .frame_count()
                    .checked_sub(1)
                    .and_then(|n| index.time_of(n))
                    .ok_or_else(|| {
                        FrameError::Unavailable(format!("media {} has no frames", file.id))
                    })?;
                lease.seek_exact(last).map_err(|e| unavailable(file.id, &e))
            }
            Err(e) => Err(unavailable(file.id, &e)),
        }
    }
}

/// The pts of the frame `index` shows at `t` (the last frame past the end).
fn shown_at(index: &FrameIndex, t: Time) -> Option<Time> {
    let last = index.frame_count().checked_sub(1)?;
    index.time_of(index.frame_at(t).min(last))
}

/// The frame time a request asks for.
fn request_time(request: &FrameRequest) -> Time {
    if request.still {
        Time::ZERO
    } else {
        request.source_time.max(Time::ZERO)
    }
}

/// The compositor's picture of a decoded frame of media `identity`.
fn convert(
    identity: u64,
    frame: &cutweave_media::VideoFrame,
) -> Result<Arc<VideoFrame>, FrameError> {
    // The cache keeps the decoded frame; the compositor gets its own copy of the planes.
    compositor_frame(identity, frame.clone())
}

impl FrameSource for MediaFrames {
    fn frame(&self, request: &FrameRequest) -> Result<Arc<VideoFrame>, FrameError> {
        let backend = self.backend.as_ref().ok_or_else(no_backend)?;
        let file = self
            .doc
            .snapshot()
            .and_then(|s| MediaFile::of(&s, request.media))
            .ok_or_else(|| {
                FrameError::Unavailable(format!("media {} has no file", request.media))
            })?;
        let pool = self.pool(backend, &file);
        let frame = self.decode(
            &pool,
            &file,
            request.stream.unwrap_or(0),
            request_time(request),
            None,
        )?;
        let identity = media_identity(&file.hash.map_or_else(
            || file.path.to_string_lossy().into_owned(),
            |h| h.to_string(),
        ));
        convert(identity, &frame)
    }
}

/// What the preview holds for one media item (see [`PreviewFrames`]).
struct Held {
    /// The asset's path and content hash when this entry was made (a change is a relink).
    path: String,
    hash: Option<String>,
    file: MediaFile,
    /// Identity of the media's pictures for the compositor's decoded-frame cache.
    identity: u64,
    pool: Arc<DecoderPool>,
    /// The pool's frame index, once it has one.
    index: Option<Arc<FrameIndex>>,
    /// The last picture served: stream, pts and the compositor's frame.
    last: Option<(u32, Time, Arc<VideoFrame>)>,
}

/// The preview render thread's front of [`MediaFrames`] (see the module docs). Not `Sync`: it
/// belongs to one render thread.
pub(crate) struct PreviewFrames {
    frames: Arc<MediaFrames>,
    held: RefCell<HashMap<MediaId, Held>>,
}

impl std::fmt::Debug for PreviewFrames {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreviewFrames")
            .field("media", &self.held.borrow().len())
            .finish_non_exhaustive()
    }
}

impl PreviewFrames {
    pub(crate) fn new(frames: Arc<MediaFrames>) -> PreviewFrames {
        PreviewFrames {
            frames,
            held: RefCell::new(HashMap::new()),
        }
    }

    /// Lets go of media that `doc` no longer has, or has relinked (call after an edit): their
    /// pools and pictures go back to the media layer.
    pub(crate) fn retain_current(&self, doc: &Project) {
        alloc_scope::within(Scope::Media, || {
            self.held.borrow_mut().retain(|id, held| {
                doc.media.get(id).is_some_and(|asset| {
                    asset.path_abs.as_deref() == Some(held.path.as_str())
                        && asset.content_hash == held.hash
                })
            });
        });
    }

    /// The pictures of one render: `doc`'s media, decodes stopped by `cancel` and run in `mode`.
    pub(crate) fn request<'a>(
        &'a self,
        doc: &'a DocSnapshot,
        cancel: &'a cutweave_playback::transport::CancelToken,
        mode: DecodeMode,
    ) -> PreviewRequest<'a> {
        PreviewRequest {
            frames: self,
            doc,
            cancel,
            mode,
        }
    }

    /// A new entry for `id` of `doc` (allocates: a new or relinked media item).
    fn resolve(&self, doc: &DocSnapshot, id: MediaId) -> Result<Held, FrameError> {
        let backend = self.frames.backend.as_ref().ok_or_else(no_backend)?;
        let asset = doc
            .doc
            .media
            .get(&id)
            .ok_or_else(|| FrameError::Unavailable(format!("media {id} has no file")))?;
        let file = MediaFile::of(doc, id)
            .ok_or_else(|| FrameError::Unavailable(format!("media {id} has no file")))?;
        let pool = self.frames.pool(backend, &file);
        let identity = media_identity(&file.hash.map_or_else(
            || file.path.to_string_lossy().into_owned(),
            |h| h.to_string(),
        ));
        Ok(Held {
            path: asset.path_abs.clone().unwrap_or_default(),
            hash: asset.content_hash.clone(),
            index: pool.index(),
            file,
            identity,
            pool,
            last: None,
        })
    }
}

/// One preview render's [`FrameSource`] (see [`PreviewFrames::request`]).
pub(crate) struct PreviewRequest<'a> {
    frames: &'a PreviewFrames,
    doc: &'a DocSnapshot,
    cancel: &'a cutweave_playback::transport::CancelToken,
    mode: DecodeMode,
}

impl PreviewRequest<'_> {
    /// A picture the preview does not hold: from the frame cache or a decode, converted.
    fn fetch(&self, held: &mut Held, stream: u32, t: Time) -> Result<Arc<VideoFrame>, FrameError> {
        let token = self.cancel.clone();
        let cancel = CancelToken::linked(CancelCheck::new(move || token.is_cancelled()));
        let frame = self.frames.frames.decode(
            &held.pool,
            &held.file,
            stream,
            t,
            Some((cancel, self.mode)),
        )?;
        let converted = convert(held.identity, &frame)?;
        held.last = Some((stream, frame.pts, Arc::clone(&converted)));
        Ok(converted)
    }
}

impl FrameSource for PreviewRequest<'_> {
    fn frame(&self, request: &FrameRequest) -> Result<Arc<VideoFrame>, FrameError> {
        let mut map = self.frames.held.borrow_mut();
        let asset = self.doc.doc.media.get(&request.media);
        let current = |held: &Held| {
            asset.is_some_and(|a| {
                a.path_abs.as_deref() == Some(held.path.as_str()) && a.content_hash == held.hash
            })
        };
        if !map.get(&request.media).is_some_and(current) {
            // A media item seen for the first time, or relinked.
            let held = alloc_scope::within(Scope::Media, || {
                self.frames.resolve(self.doc, request.media)
            })?;
            alloc_scope::within(Scope::Media, || map.insert(request.media, held));
        }
        let held = map
            .get_mut(&request.media)
            .ok_or_else(|| FrameError::Unavailable(format!("media {}", request.media)))?;
        let stream = request.stream.unwrap_or(0);
        let t = request_time(request);
        if held.index.is_none() {
            held.index = alloc_scope::within(Scope::Media, || held.pool.index());
        }
        // The picture shown at `t`, if the preview already holds it.
        if let Some(pts) = held.index.as_ref().and_then(|index| shown_at(index, t))
            && let Some((s, p, frame)) = &held.last
            && *s == stream
            && *p == pts
        {
            return Ok(Arc::clone(frame));
        }
        alloc_scope::within(Scope::Media, || self.fetch(held, stream, t))
    }
}
