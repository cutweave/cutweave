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
//! - **Damage.** A frame that stood in for an undecodable one (`damaged`) is shown; the asset is
//!   logged once.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use cutweave_cache::{CacheDb, ContentHash, FrameCache, FrameKey};
use cutweave_media::{
    CancelToken, DecodeMode, DecodeOpts, DecoderPool, MediaBackend, MediaError, VideoDecoder,
};
use cutweave_model::{MediaId, Time};
use cutweave_render::{FrameError, FrameRequest, FrameSource, VideoFrame};

use super::media::{CurrentDoc, MediaFile};
use crate::export::sources::{compositor_frame, media_identity};

thread_local! {
    /// The cancel token and decode mode of the render running on this thread.
    static SCOPE: RefCell<Option<(CancelToken, DecodeMode)>> = const { RefCell::new(None) };
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
        }
    }

    /// Runs `f` (a render graph build on this thread) with `cancel` stopping its decodes, which
    /// use `mode` (`Sequential` for playback, `Seek` for scrubbing and stills).
    pub(crate) fn scoped<R>(cancel: CancelToken, mode: DecodeMode, f: impl FnOnce() -> R) -> R {
        let previous = SCOPE.with(|s| s.borrow_mut().replace((cancel, mode)));
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
        // The frame shown at `t` is known from the index: look it up first.
        let shown = pool.index().and_then(|index| {
            let last = index.frame_count().checked_sub(1)?;
            let n = index.frame_at(t).min(last);
            index.time_of(n)
        });
        if let (Some(hash), Some(pts)) = (file.hash, shown)
            && let Some(frame) = self.cache.get(&FrameKey { hash, stream, pts })
        {
            return Ok(frame);
        }
        let (cancel, mode) = SCOPE
            .with(|s| s.borrow().clone())
            .unwrap_or_else(|| (CancelToken::new(), DecodeMode::Seek));
        let opts = DecodeOpts::default().with_mode(mode);
        let mut lease = pool
            .checkout(&opts, Some(t), &cancel)
            .map_err(|e| unavailable(file.id, &e))?;
        let frame = match lease.seek_exact(t) {
            Ok(frame) => frame,
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
                lease
                    .seek_exact(last)
                    .map_err(|e| unavailable(file.id, &e))?
            }
            Err(e) => return Err(unavailable(file.id, &e)),
        };
        drop(lease);
        if frame.damaged {
            self.flag_damaged(file);
        }
        let frame = Arc::new(frame);
        if let Some(hash) = file.hash {
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
