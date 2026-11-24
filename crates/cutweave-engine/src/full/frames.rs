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
//!   scrub or playback keeps decoding instead of seeking) and seeks frame-exactly. The calling
//!   thread's cancel token ([`MediaFrames::scoped`]: the playback render's epoch) stops a pending
//!   seek. Frames arrive in coded orientation; the render graph rotates them from the probe
//!   (`FrameRequest::rotation`).
//! - **Preview against agent frames.** Preview renders ([`MediaFrames::scoped`]) decode with
//!   hardware when the machine has it (`HwMode::Auto`, WP-1D.04) and use the frame cache.
//!   `render.frame` (no scope) decodes in software and bypasses the cache, exactly as exports
//!   do, so an agent frame is bit-identical to the exported one (arch §8). `/media/frame`
//!   ([`MediaFrames::agent_scoped`], WP-1I.03) decodes the same way, with a cancel token.
//! - **Layouts.** A frame in a layout the compositor does not read (10-bit planar, 4:2:2: a
//!   software decoder, or a hardware decoder that fell back) makes the media decode into
//!   `P010` / `Yuv420p` from then on (swscale in the decoder).
//! - **Damage.** A frame that stood in for an undecodable one (`damaged`) is shown; the asset is
//!   logged once.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use cutweave_cache::{CacheDb, ContentHash, FrameCache, FrameKey};
use cutweave_media::{
    CancelToken, DecodeMode, DecodeOpts, DecoderPool, HwMode, MediaBackend, MediaError,
    PixelFormat, VideoDecoder,
};
use cutweave_model::{MediaId, Time};
use cutweave_render::{FrameError, FrameRequest, FrameSource, VideoFrame};

use super::media::{CurrentDoc, MediaFile};
use crate::export::sources::{compositor_frame, compositor_target, media_identity};

/// How the render running on this thread decodes.
#[derive(Clone)]
struct Scope {
    /// Stops its decodes.
    cancel: CancelToken,
    /// `Some(mode)`: a preview render (hardware decode when the machine has it, the frame
    /// cache). `None`: an agent frame (software decode, no cache), as without a scope.
    preview: Option<DecodeMode>,
}

thread_local! {
    /// The scope of the render running on this thread.
    static SCOPE: RefCell<Option<Scope>> = const { RefCell::new(None) };
}

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

    /// Runs `f` (a render graph build on this thread) with `cancel` stopping its decodes, which
    /// use `mode` (`Sequential` for playback, `Seek` for scrubbing and stills).
    pub(crate) fn scoped<R>(cancel: CancelToken, mode: DecodeMode, f: impl FnOnce() -> R) -> R {
        Self::with_scope(
            Scope {
                cancel,
                preview: Some(mode),
            },
            f,
        )
    }

    /// Runs `f` (a `render.frame` render on this thread) with `cancel` stopping its decodes,
    /// which decode in software and skip the frame cache exactly as without a scope, so the
    /// pixels are `render.frame`'s (`/media/frame`, WP-1I.03).
    pub(crate) fn agent_scoped<R>(cancel: CancelToken, f: impl FnOnce() -> R) -> R {
        Self::with_scope(
            Scope {
                cancel,
                preview: None,
            },
            f,
        )
    }

    fn with_scope<R>(scope: Scope, f: impl FnOnce() -> R) -> R {
        let previous = SCOPE.with(|s| s.borrow_mut().replace(scope));
        let out = f();
        SCOPE.with(|s| *s.borrow_mut() = previous);
        out
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

    /// The decoded frame at media time `t` of `file`.
    fn decode(
        &self,
        backend: &Arc<dyn MediaBackend>,
        file: &MediaFile,
        stream: u32,
        t: Time,
    ) -> Result<Arc<cutweave_media::VideoFrame>, FrameError> {
        let pool = self.pool(backend, file);
        let scope = SCOPE.with(|s| s.borrow().clone());
        let preview = scope.as_ref().and_then(|s| s.preview);
        // Preview uses the frame cache; agent frames decode exactly like exports.
        let cached = preview.is_some() && file.hash.is_some();
        if cached
            && let Some(hash) = file.hash
            // The frame shown at `t` is known from the index: look it up first.
            && let Some(pts) = pool.index().and_then(|index| {
                let last = index.frame_count().checked_sub(1)?;
                index.time_of(index.frame_at(t).min(last))
            })
            && let Some(frame) = self.cache.get(&FrameKey { hash, stream, pts })
        {
            return Ok(frame);
        }
        let cancel = scope.map_or_else(CancelToken::new, |s| s.cancel);
        let opts = match preview {
            Some(mode) => DecodeOpts::default().with_mode(mode),
            None => DecodeOpts {
                hw: HwMode::Off,
                ..DecodeOpts::default()
            },
        };
        let target = self
            .targets
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&file.id)
            .copied();
        let mut frame = Self::seek(&pool, file, &opts, target, t, &cancel)?;
        if target.is_none()
            && let Some(target) = compositor_target(frame.format)
        {
            // A layout the compositor does not read: convert this media's frames from now on.
            self.targets
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(file.id, target);
            frame = Self::seek(&pool, file, &opts, Some(target), t, &cancel)?;
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

impl FrameSource for MediaFrames {
    fn frame(&self, request: &FrameRequest) -> Result<Arc<VideoFrame>, FrameError> {
        let backend = self.backend.as_ref().ok_or_else(|| {
            FrameError::Unavailable("this engine has no media backend to decode with".into())
        })?;
        let file = self
            .doc
            .snapshot()
            .and_then(|s| MediaFile::of(&s, request.media))
            .ok_or_else(|| {
                FrameError::Unavailable(format!("media {} has no file", request.media))
            })?;
        let t = if request.still {
            Time::ZERO
        } else {
            request.source_time.max(Time::ZERO)
        };
        let frame = self.decode(backend, &file, request.stream.unwrap_or(0), t)?;
        let identity = media_identity(&file.hash.map_or_else(
            || file.path.to_string_lossy().into_owned(),
            |h| h.to_string(),
        ));
        // The cache keeps the decoded frame; the compositor gets its own copy of the planes.
        compositor_frame(identity, (*frame).clone())
    }
}
