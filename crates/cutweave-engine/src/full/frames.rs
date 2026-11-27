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
//!   `/media/frame` ([`MediaFrames::agent_scoped`], WP-1I.03) decodes the same way, with a
//!   cancel token.
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
//!
//! # Proxies (WP-2F.01, WP-2F.02)
//!
//! Which file a picture is decoded from is the [`MediaResolver`]'s decision, by purpose. A
//! preview layer ([`MediaPurpose::Preview`]) is served from the asset's ready proxy
//! ([`FullProxies`]) when the preview uses proxies ([`FrameRequest::proxy`]: the transport's
//! toggle, `view.set_preview_quality`), the asset has one, and the proxy holds the pixels the
//! layer is drawn at ([`FrameRequest::drawn_long_edge`]: playback, scrub and paused frames
//! alike; a full-resolution preview that needs more than the proxy has shows the original).
//! Proxy frame `n` is original frame `n`, so the frame shown at a source time is found through
//! the original's index stored with the proxy, decoded from the proxy's own pool (its frames
//! cached under the proxy's identity, never the original's) and placed with the original's
//! probed size, so the picture lands exactly where the original's would.
//!
//! - **Hot swap.** The preview holds the proxy it resolved until the store's generation moves
//!   (a proxy became ready, was evicted or deleted) or the asset's proxy mode changes: one
//!   atomic load per frame, so a proxy finished while the file is open is used from the next
//!   frame on, and the original's decoders stay open for the frames that need them.
//! - **No hitch in playback.** While a media item plays (frames decoded in order), it stays on
//!   the proxy it is reading when the decision turns to the original (the toggle was switched
//!   off, the quality ladder climbed to a size the proxy does not cover): the original would
//!   need a seek into its GOP. The original shows at the next seek, pause or step. A change to
//!   the proxy (intra-frame) applies to the next frame ([`MediaResolver::streaming`]).
//! - **Fallback.** A missing, evicted or failing proxy falls back to the original.
//! - **Offline originals.** Media whose original is not on this computer shows the "media
//!   offline" slate, never its proxy: the lookup finds no proxy for it, and a paused frame, a
//!   seek or a scrub looks at the disk again before a proxy is served (playback does not: a
//!   file that goes away while it plays is found gone at the next pause or seek).
//! - **Never.** Agent frames, `/media/frame`, thumbnails, analysis and exports read originals
//!   ([`MediaPurpose::Export`], `Thumbnail`, `Analysis`).
//!
//! # Reverse playback (WP-2D.01b)
//!
//! The preview notes, per media item, the source time it was last asked for: a request up to
//! a second before it reads the media backwards (a reversed clip, or the transport playing or
//! scrubbing backwards). Then:
//!
//! - **Playback prefers the proxy.** A playback frame read backwards is served from the asset's
//!   ready proxy whatever size it is drawn at (intra-frame: every frame costs one decode),
//!   while the preview uses proxies.
//! - **The original is read back.** Read backwards in small steps (at most a quarter of a
//!   window: playback, a step, a drag), a long-GOP original (one whose frames do not all decode
//!   on their own) is decoded forward in windows ([`ReadBack`]: from the GOP's keyframe, at
//!   most [`READBACK_BYTES`] of pictures), converted for the compositor once, and served from
//!   the window until it is used up: a hit takes no lock and allocates nothing. A frame the RAM
//!   frame cache holds (a stretch played forwards before) comes from there instead, and a jump
//!   back decodes the one frame as before. At most [`READBACK_MEDIA`] media items keep a window (the least recently used one
//!   is let go), and a window unused for [`READBACK_IDLE_RENDERS`] renders is let go.
//! - **Proxies are asked for.** The engine requests a proxy for long-GOP media that a reversed
//!   clip plays and warns while it builds ([`FullProxies::reversed`](super::proxy::FullProxies)).

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use cutweave_cache::proxy::ReadyProxy;
use cutweave_cache::{CacheDb, ContentHash, FrameCache, FrameIndex, FrameKey};
use cutweave_commands::DocSnapshot;
use cutweave_media::{
    CancelCheck, CancelToken, DecodeMode, DecodeOpts, DecoderPool, HwMode, MediaBackend,
    MediaError, MediaPurpose, MediaResolver, PixelFormat, PreviewQuality, ProxyFile, ReadBack,
    Rendition, VideoDecoder,
};
use cutweave_model::{FLICKS_PER_SECOND, MediaAsset, MediaId, ProbeInfo, Project, ProxyMode, Time};
use cutweave_render::alloc_scope::{self, Scope};
use cutweave_render::{FrameError, FrameRequest, FrameSource, VideoFrame};

use super::media::{CurrentDoc, MediaFile};
use super::proxy::FullProxies;
use crate::export::sources::{compositor_frame, compositor_target, frame_error, media_identity};

/// Bytes of decoded pictures the read-back of one media item may hold while the preview reads
/// it backwards (WP-2D.01b): 64 frames of 1080p 4:2:0, 16 of 4K.
pub(crate) const READBACK_BYTES: usize = 192 << 20;
/// Media items whose read-back the preview keeps at once; the least recently used one goes.
pub(crate) const READBACK_MEDIA: usize = 2;
/// Renders after which a read-back nobody used is let go (3 s of playback at 30 fps).
pub(crate) const READBACK_IDLE_RENDERS: u64 = 90;
/// The longest step back in source time that counts as reading backwards (1 s: reverse
/// playback up to 30× at 30 fps).
const REVERSE_STEP_FLICKS: i64 = FLICKS_PER_SECOND;

/// Frames of `width × height` a read-back holds within [`READBACK_BYTES`], as the compositor
/// gets them: 4:2:0, in 16-bit samples when `pixel_format` (the probe's) is 10 or 12-bit.
pub(crate) fn readback_frames(width: u32, height: u32, pixel_format: Option<&str>) -> usize {
    let pixels = usize::try_from(u64::from(width) * u64::from(height)).unwrap_or(usize::MAX);
    let deep = pixel_format.is_some_and(|f| f.contains("10") || f.contains("12"));
    let bytes = if deep {
        pixels.saturating_mul(3)
    } else {
        pixels.saturating_mul(3) / 2
    };
    ReadBack::<()>::capacity_for(READBACK_BYTES, bytes)
}

/// Whether the preview's decoders may use the hardware (`media.set_decode_policy`, WP-1D.08).
/// Read by every preview decode, so a change reaches decoders opened from then on; exports and
/// agent frames decode in software whatever it says.
#[derive(Debug, Default)]
pub(crate) struct DecodePolicy {
    software_only: AtomicBool,
}

impl DecodePolicy {
    /// The hardware mode preview decoders are opened with.
    pub(crate) fn hw(&self) -> HwMode {
        if self.software_only.load(Ordering::Relaxed) {
            HwMode::Off
        } else {
            HwMode::Auto
        }
    }

    /// Sets the mode; returns whether it changed.
    pub(crate) fn set(&self, hw: HwMode) -> bool {
        let off = hw == HwMode::Off;
        self.software_only.swap(off, Ordering::Relaxed) != off
    }
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
    policy: Arc<DecodePolicy>,
    pools: Mutex<HashMap<MediaId, Pooled>>,
    damaged: Mutex<HashSet<MediaId>>,
    /// Media whose frames must be converted for the compositor, and into what.
    targets: Mutex<HashMap<MediaId, PixelFormat>>,
    /// Decides which file a reader opens: the preview may get a proxy (WP-2F.01, WP-2F.02).
    resolver: Resolver,
}

/// The engine's [`MediaResolver`]: over the proxies of the cache.
type Resolver = MediaResolver<Arc<FullProxies>>;

impl std::fmt::Debug for MediaFrames {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaFrames")
            .field("backend", &self.backend.is_some())
            .field("pools", &self.pool_count())
            .finish_non_exhaustive()
    }
}

/// The compositor's error for a media-layer failure: offline when the file is not there
/// ([`frame_error`]).
fn unavailable(media: MediaId, e: &MediaError) -> FrameError {
    frame_error(media, e)
}

fn no_backend() -> FrameError {
    FrameError::Unavailable("this engine has no media backend to decode with".into())
}

/// How a decode runs: the preview's (cancellable, hardware, through the frame cache) or an
/// agent frame's (`None`: software, uncached, like exports).
type DecodeScope = Option<(CancelToken, DecodeMode)>;

thread_local! {
    /// The cancel token of the agent frame (`/media/frame`) rendering on this thread; not on
    /// the preview's render thread.
    static AGENT_CANCEL: RefCell<Option<CancelToken>> = const { RefCell::new(None) };
}

impl MediaFrames {
    /// Runs `f` (a `render.frame` render on this thread) with `cancel` stopping its decodes,
    /// which decode in software and skip the frame cache exactly as `render.frame` does, so the
    /// pixels are `render.frame`'s (`/media/frame`, WP-1I.03).
    pub(crate) fn agent_scoped<R>(cancel: CancelToken, f: impl FnOnce() -> R) -> R {
        let previous = AGENT_CANCEL.with(|c| c.borrow_mut().replace(cancel));
        let out = f();
        AGENT_CANCEL.with(|c| *c.borrow_mut() = previous);
        out
    }

    /// Frames of the media of `doc`, decoded with `backend`, cached in `cache`, with frame
    /// indexes from `db`.
    pub(crate) fn new(
        backend: Option<Arc<dyn MediaBackend>>,
        doc: CurrentDoc,
        cache: Arc<FrameCache>,
        db: Option<Arc<CacheDb>>,
        policy: Arc<DecodePolicy>,
    ) -> MediaFrames {
        MediaFrames {
            backend,
            doc,
            cache,
            db,
            policy,
            pools: Mutex::new(HashMap::new()),
            damaged: Mutex::new(HashSet::new()),
            targets: Mutex::new(HashMap::new()),
            resolver: MediaResolver::new(None),
        }
    }

    /// The same frames, serving preview layers from `proxies` when the resolver says so.
    pub(crate) fn with_proxies(mut self, proxies: Option<Arc<FullProxies>>) -> MediaFrames {
        self.resolver = MediaResolver::new(proxies);
        self
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

    /// Closes every idle decoder, so the next seek of each media item opens one under the
    /// current [`DecodePolicy`] (decoders checked out now are not found again: the pool matches
    /// decoders by their options).
    pub(crate) fn reopen_decoders(&self) {
        let pools = self.pools.lock().unwrap_or_else(PoisonError::into_inner);
        for pooled in pools.values() {
            pooled.pool.trim();
        }
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
            Some((cancel, mode)) => (
                cancel,
                DecodeOpts::default()
                    .with_mode(mode)
                    .with_hw(self.policy.hw()),
            ),
            None => (
                AGENT_CANCEL
                    .with(|c| c.borrow().clone())
                    .unwrap_or_default(),
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

    /// Fills `readback` with the window of `file` (decoded from `pool`; `index` is its frame
    /// index) that ends at frame `n`, decoded like the preview's other frames (hardware per the
    /// policy, run in `mode`, stopped by `cancel`) and converted for the compositor as media
    /// `identity`; returns frame `n` (WP-2D.01b). `None` when the stream ended before it.
    #[allow(clippy::too_many_arguments)]
    fn read_back(
        &self,
        pool: &DecoderPool,
        file: &MediaFile,
        identity: u64,
        readback: &mut ReadBack<Arc<VideoFrame>>,
        index: &FrameIndex,
        n: usize,
        (cancel, mode): (CancelToken, DecodeMode),
    ) -> Result<Option<Arc<VideoFrame>>, FrameError> {
        let mut opts = DecodeOpts::default()
            .with_mode(mode)
            .with_hw(self.policy.hw());
        if let Some(target) = self
            .targets
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&file.id)
            .copied()
        {
            opts = opts.with_target(target);
        }
        let near = ReadBack::<()>::window(index, n, readback.capacity())
            .and_then(|w| index.time_of(*w.start()));
        let mut lease = pool
            .checkout(&opts, near, &cancel)
            .map_err(|e| unavailable(file.id, &e))?;
        let mut damaged = false;
        let filled = readback.fill(&mut lease, index, n, |frame| {
            damaged |= frame.damaged;
            compositor_frame(identity, frame).map_err(Fill::Frame)
        });
        if damaged {
            self.flag_damaged(file);
        }
        filled.map_err(|e| match e {
            Fill::Media(e) => unavailable(file.id, &e),
            Fill::Frame(e) => e,
        })
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
    shown_frame(index, t).map(|(_, pts)| pts)
}

/// The frame `index` shows at `t` (the last frame past the end): its presentation index and pts.
fn shown_frame(index: &FrameIndex, t: Time) -> Option<(usize, Time)> {
    let last = index.frame_count().checked_sub(1)?;
    let n = index.frame_at(t).min(last);
    Some((n, index.time_of(n)?))
}

/// Why a read-back stopped: the decoder, or the conversion for the compositor.
enum Fill {
    Media(MediaError),
    Frame(FrameError),
}

impl From<MediaError> for Fill {
    fn from(e: MediaError) -> Fill {
        Fill::Media(e)
    }
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
    /// The asset's ready proxy, as resolved at `proxy_resolved`: the store's generation and
    /// the asset's proxy mode then (`None`: look again).
    proxy: Option<HeldProxy>,
    proxy_resolved: Option<(u64, ProxyMode)>,
    /// The last picture served was a playback frame from the proxy: the stream it is part of
    /// stays on the proxy (see the module docs).
    streaming_proxy: bool,
    /// The source time last asked for: a request before it reads the media backwards.
    requested: Option<Time>,
    /// Whether some frames of the media decode only from an earlier keyframe (from the index).
    long_gop: Option<bool>,
    /// Frames decoded forward for reading backwards (WP-2D.01b), of stream `readback_stream`.
    readback: Option<ReadBack<Arc<VideoFrame>>>,
    readback_stream: u32,
    /// The render (`PreviewFrames::renders`) that last used the read-back.
    readback_used: u64,
    /// Bytes of the media's last picture for the compositor (0: none yet).
    frame_bytes: usize,
    /// The last decode found the file gone (WP-2F.03c): playback shows the slate without
    /// asking the media layer again; a paused frame, a seek or a scrub asks again.
    offline: bool,
}

impl Held {
    /// Whether some frames decode only from an earlier keyframe (computed once per item).
    fn long_gop(&mut self) -> bool {
        match (self.long_gop, &self.index) {
            (Some(long), _) => long,
            (None, Some(index)) => *self.long_gop.insert(index.max_decode_distance() > 1),
            (None, None) => false,
        }
    }
}

/// Bytes of the planes of `frame`.
fn frame_bytes(frame: &VideoFrame) -> usize {
    frame.planes.iter().map(|p| p.data.len()).sum()
}

/// A ready proxy the preview holds for one media item.
struct HeldProxy {
    ready: Arc<ReadyProxy>,
    /// The proxy file as the media layer decodes it: its own id, path and identity hash.
    file: MediaFile,
    pool: Arc<DecoderPool>,
    identity: u64,
    /// The last picture served from it: the original time it shows and the frame.
    last: Option<(Time, Arc<VideoFrame>)>,
}

impl HeldProxy {
    /// The preview's hold on `ready` (allocates: once per proxy).
    fn new(backend: &Arc<dyn MediaBackend>, ready: Arc<ReadyProxy>) -> HeldProxy {
        let digest = ready.identity.digest();
        let mut id = [0_u8; 16];
        id.copy_from_slice(&digest[..16]);
        let file = MediaFile {
            id: MediaId::from_u128(u128::from_le_bytes(id)),
            path: ready.record.path.clone(),
            hash: Some(ready.identity),
            probe: ProbeInfo::default(),
        };
        let pool = Arc::new(
            DecoderPool::new(Arc::clone(backend), file.path.clone())
                .with_index(Arc::clone(&ready.index)),
        );
        HeldProxy {
            identity: media_identity(&ready.identity.to_string()),
            ready,
            file,
            pool,
            last: None,
        }
    }
}

/// The preview render thread's front of [`MediaFrames`] (see the module docs). Not `Sync`: it
/// belongs to one render thread.
pub(crate) struct PreviewFrames {
    frames: Arc<MediaFrames>,
    held: RefCell<HashMap<MediaId, Held>>,
    /// Renders so far (one per [`request`](Self::request)): the read-backs' clock.
    renders: Cell<u64>,
    /// Bytes of pictures one media item's read-back holds ([`READBACK_BYTES`]).
    readback_budget: usize,
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
            renders: Cell::new(0),
            readback_budget: READBACK_BYTES,
        }
    }

    /// The same front with read-backs of `bytes` per media item (tests).
    #[cfg(all(test, feature = "ffmpeg"))]
    pub(crate) fn with_readback_budget(mut self, bytes: usize) -> PreviewFrames {
        self.readback_budget = bytes;
        self
    }

    /// Windows the read-back of `media` decoded so far (tests).
    #[cfg(all(test, feature = "ffmpeg"))]
    pub(crate) fn readback_fills(&self, media: MediaId) -> u64 {
        self.held
            .borrow()
            .get(&media)
            .and_then(|h| h.readback.as_ref())
            .map_or(0, ReadBack::fills)
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
    /// Lets go of read-backs unused for [`READBACK_IDLE_RENDERS`] renders.
    pub(crate) fn request<'a>(
        &'a self,
        doc: &'a DocSnapshot,
        cancel: &'a cutweave_playback::transport::CancelToken,
        mode: DecodeMode,
    ) -> PreviewRequest<'a> {
        let now = self.renders.get().wrapping_add(1);
        self.renders.set(now);
        for held in self.held.borrow_mut().values_mut() {
            if let Some(rb) = held.readback.as_mut()
                && !rb.is_empty()
                && now.saturating_sub(held.readback_used) > READBACK_IDLE_RENDERS
            {
                // The pictures belong to the media layer.
                alloc_scope::within(Scope::Media, || rb.clear());
            }
        }
        PreviewRequest {
            frames: self,
            doc,
            cancel,
            mode,
            showed_proxy: Cell::new(false),
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
            proxy: None,
            proxy_resolved: None,
            streaming_proxy: false,
            requested: None,
            long_gop: None,
            readback: None,
            readback_stream: 0,
            readback_used: 0,
            frame_bytes: 0,
            offline: false,
        })
    }
}

/// Makes room for a read-back of `keep`: lets go of the least recently used windows of other
/// media until fewer than [`READBACK_MEDIA`] remain.
fn make_room(held: &mut HashMap<MediaId, Held>, keep: MediaId) {
    loop {
        let mut live = 0;
        let mut oldest: Option<(u64, MediaId)> = None;
        for (id, h) in held.iter() {
            if *id == keep || h.readback.as_ref().is_none_or(ReadBack::is_empty) {
                continue;
            }
            live += 1;
            if oldest.is_none_or(|(used, _)| h.readback_used < used) {
                oldest = Some((h.readback_used, *id));
            }
        }
        match oldest {
            Some((_, id)) if live >= READBACK_MEDIA => {
                if let Some(rb) = held.get_mut(&id).and_then(|h| h.readback.as_mut()) {
                    rb.clear();
                }
            }
            _ => return,
        }
    }
}

/// One preview render's [`FrameSource`] (see [`PreviewFrames::request`]).
pub(crate) struct PreviewRequest<'a> {
    frames: &'a PreviewFrames,
    doc: &'a DocSnapshot,
    cancel: &'a cutweave_playback::transport::CancelToken,
    mode: DecodeMode,
    /// A picture of this render came from a proxy.
    showed_proxy: Cell<bool>,
}

impl PreviewRequest<'_> {
    /// Whether a picture served for this render so far was decoded from a proxy (the
    /// preview's "Proxy" / "Original" chip, WP-2F.02).
    pub(crate) fn showed_proxy(&self) -> bool {
        self.showed_proxy.get()
    }

    /// Why `request` reads its media: a preview picture, with proxies as the request says and
    /// at the size it is drawn at. Playback read backwards (`backward`) takes a proxy of any
    /// size (WP-2D.01b).
    fn purpose(&self, request: &FrameRequest, backward: bool) -> MediaPurpose {
        let any_size = backward && self.mode == DecodeMode::Sequential;
        MediaPurpose::Preview(PreviewQuality {
            use_proxies: request.proxy,
            long_edge: if any_size {
                None
            } else {
                request.drawn_long_edge
            },
        })
    }

    /// The picture of `asset`'s ready proxy shown at original time `t`, when the resolver
    /// gives `purpose` the proxy (see the module docs); `None` falls back to the original.
    /// `streaming`: the item is in the middle of a playback stream it reads from the proxy,
    /// which it keeps while the proxy it holds is still the store's.
    fn proxy_frame(
        &self,
        held: &mut Held,
        asset: Option<&MediaAsset>,
        t: Time,
        purpose: MediaPurpose,
        streaming: bool,
    ) -> Option<Arc<VideoFrame>> {
        let wanted = purpose.may_use_proxy();
        if !wanted && !streaming {
            return None;
        }
        let resolver = &self.frames.frames.resolver;
        let proxies = resolver.proxies()?;
        let asset = asset?;
        let at = (proxies.store().generation(), asset.proxy_mode);
        if held.proxy_resolved != Some(at) {
            if !wanted {
                // The proxy the stream was on may be gone, and this frame asks for none.
                return None;
            }
            // A proxy became ready or went away since this item was resolved, or the asset's
            // proxies were turned on or off (allocates).
            held.proxy = alloc_scope::within(Scope::Media, || {
                let backend = self.frames.frames.backend.as_ref()?;
                let ready = resolver.candidate(asset, purpose)?;
                Some(HeldProxy::new(backend, ready))
            });
            held.proxy_resolved = Some(at);
        }
        let long_edge = held.proxy.as_ref()?.ready.long_edge();
        // The resolution rule: a picture drawn larger than the proxy reads the original. A
        // stream on the proxy keeps it until its next seek.
        let rule = if Resolver::serves(purpose, long_edge) {
            Rendition::Proxy
        } else {
            Rendition::Original
        };
        let current = if streaming {
            Rendition::Proxy
        } else {
            Rendition::Original
        };
        if Resolver::streaming(current, rule) == Rendition::Original {
            return None;
        }
        // A paused frame, a seek or a scrub looks at the disk again: an original that went
        // away shows the slate, not its proxy (one stat; playback does not look).
        if self.mode == DecodeMode::Seek
            && !alloc_scope::within(Scope::Media, || {
                FullProxies::original_there(Some(held.path.as_str()))
            })
        {
            return None;
        }
        let proxy = held.proxy.as_mut()?;
        let (n, shown) = proxy.ready.frame_at(t)?;
        if let Some((pts, frame)) = &proxy.last
            && *pts == shown
        {
            return Some(Arc::clone(frame));
        }
        let at = proxy.ready.index.time_of(n)?;
        let token = self.cancel.clone();
        let frames = &self.frames.frames;
        let mode = self.mode;
        let decoded = alloc_scope::within(Scope::Media, || {
            let cancel = CancelToken::linked(CancelCheck::new(move || token.is_cancelled()));
            frames
                .decode(&proxy.pool, &proxy.file, 0, at, Some((cancel, mode)))
                .and_then(|frame| convert(proxy.identity, &frame))
        });
        match decoded {
            Ok(frame) => {
                proxy.last = Some((shown, Arc::clone(&frame)));
                Some(frame)
            }
            Err(e) => {
                if !self.cancel.is_cancelled() {
                    tracing::debug!(error = %e, "the proxy could not be decoded; using the original");
                }
                None
            }
        }
    }

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
        held.frame_bytes = frame_bytes(&converted);
        held.last = Some((stream, frame.pts, Arc::clone(&converted)));
        Ok(converted)
    }

    /// Frames of `held`'s original a read-back window holds: the budget over the size of its
    /// last picture (else of a 4:2:0 picture of `size`, else of 1080p).
    fn readback_capacity(&self, held: &Held, size: Option<(u32, u32)>) -> usize {
        let bytes = match held.frame_bytes {
            0 => size.map_or(1920 * 1080 * 3 / 2, |(w, h)| {
                usize::try_from(u64::from(w) * u64::from(h) * 3 / 2).unwrap_or(usize::MAX)
            }),
            b => b,
        };
        ReadBack::<()>::capacity_for(self.frames.readback_budget, bytes)
    }

    /// Frame `n` of `held`'s original read backwards (see the module docs) into windows of
    /// `capacity` frames: from the RAM frame cache when it is there, else from a new read-back
    /// window. `None`: not read back (the media layer could not; the caller decodes it the
    /// ordinary way).
    fn read_back(
        &self,
        held: &mut Held,
        stream: u32,
        n: usize,
        capacity: usize,
    ) -> Result<Option<Arc<VideoFrame>>, FrameError> {
        let Some(index) = held.index.clone() else {
            return Ok(None);
        };
        let frames = &self.frames.frames;
        // A stretch played forwards before is in the frame cache.
        if let Some(hash) = held.file.hash
            && let Some(pts) = index.time_of(n)
            && let Some(frame) = frames.cache.get(&FrameKey { hash, stream, pts })
        {
            let converted = convert(held.identity, &frame)?;
            held.last = Some((stream, frame.pts, Arc::clone(&converted)));
            return Ok(Some(converted));
        }
        let rb = held.readback.get_or_insert_with(|| ReadBack::new(capacity));
        rb.set_capacity(capacity);
        held.readback_stream = stream;
        held.readback_used = self.frames.renders.get();
        let token = self.cancel.clone();
        let cancel = CancelToken::linked(CancelCheck::new(move || token.is_cancelled()));
        let read = frames.read_back(
            &held.pool,
            &held.file,
            held.identity,
            rb,
            &index,
            n,
            (cancel, self.mode),
        );
        match read {
            Ok(Some(frame)) => {
                held.frame_bytes = frame_bytes(&frame);
                Ok(Some(frame))
            }
            Ok(None) => Ok(None),
            Err(e) => {
                if !self.cancel.is_cancelled() {
                    tracing::debug!(error = %e, "reading back failed; decoding the frame");
                }
                Ok(None)
            }
        }
    }
}

impl FrameSource for PreviewRequest<'_> {
    fn frame(&self, request: &FrameRequest) -> Result<Arc<VideoFrame>, FrameError> {
        // Proxy jobs yield while pictures are being served (playback, scrub, Source view).
        if let Some(proxies) = self.frames.frames.resolver.proxies() {
            proxies.note_preview_activity();
        }
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
        // Offline media during playback: the slate, with no lock, no allocation and no trip to
        // a disk that may be slow to say "not there" (a drive that is not connected).
        if held.offline && self.mode == DecodeMode::Sequential {
            return Err(FrameError::Offline(request.media));
        }
        let stream = request.stream.unwrap_or(0);
        let t = request_time(request);
        // Read backwards: a reversed clip, or the transport playing or scrubbing back (a jump
        // back, or another clip of the media further on, is not).
        let previous = held.requested;
        let backward = !request.still
            && previous.is_some_and(|p| t < p && p.flicks() - t.flicks() <= REVERSE_STEP_FLICKS);
        held.requested = Some(t);
        // The resolver decides: the proxy when the preview uses proxies and it holds the
        // pixels the layer is drawn at (WP-2F.02).
        // Offline media shows the slate even when it has a proxy (one picture per clip).
        // A playback frame next to the one before it (within a second, either way) is part of
        // the same stream.
        let playback = self.mode == DecodeMode::Sequential;
        let streaming = playback
            && held.streaming_proxy
            && previous.is_some_and(|p| (t.flicks() - p.flicks()).abs() <= REVERSE_STEP_FLICKS);
        if !held.offline
            && let Some(frame) =
                self.proxy_frame(held, asset, t, self.purpose(request, backward), streaming)
        {
            self.showed_proxy.set(true);
            held.streaming_proxy = playback;
            return Ok(frame);
        }
        // A render that was cancelled (a newer request: the toggle, an edit) says nothing
        // about the stream: the frame after it goes on from the proxy.
        held.streaming_proxy = streaming && self.cancel.is_cancelled();
        if held.index.is_none() {
            held.index = alloc_scope::within(Scope::Media, || held.pool.index());
        }
        let shown = held.index.as_ref().and_then(|index| shown_frame(index, t));
        // The picture shown at `t`, if the preview already holds it.
        if let Some((_, pts)) = shown
            && let Some((s, p, frame)) = &held.last
            && *s == stream
            && *p == pts
        {
            return Ok(Arc::clone(frame));
        }
        if let Some((n, _)) = shown {
            // A frame read back: no lock, no allocation.
            if held.readback_stream == stream
                && let Some(frame) = held.readback.as_ref().and_then(|rb| rb.get(n))
            {
                let frame = Arc::clone(frame);
                held.readback_used = self.frames.renders.get();
                return Ok(frame);
            }
            // Read back when the step back is small enough for a window to serve the next few
            // (playback, a step, a drag); a jump back decodes the one frame.
            let capacity = self.readback_capacity(held, request.source_size);
            let step = previous
                .and_then(|p| held.index.as_ref().and_then(|index| shown_frame(index, p)))
                .map_or(usize::MAX, |(m, _)| m.saturating_sub(n));
            if backward && step <= (capacity / 4).max(1) && held.long_gop() {
                let read = alloc_scope::within(Scope::Media, || {
                    make_room(&mut map, request.media);
                    match map.get_mut(&request.media) {
                        Some(held) => self.read_back(held, stream, n, capacity),
                        None => Ok(None),
                    }
                })?;
                if let Some(frame) = read {
                    return Ok(frame);
                }
            }
        }
        let held = map
            .get_mut(&request.media)
            .ok_or_else(|| FrameError::Unavailable(format!("media {}", request.media)))?;
        let fetched = alloc_scope::within(Scope::Media, || self.fetch(held, stream, t));
        let offline = matches!(fetched, Err(FrameError::Offline(_)));
        if held.offline && !offline {
            // The file is back: its proxy may be served again (look it up anew).
            held.proxy_resolved = None;
        }
        held.offline = offline;
        fetched
    }
}
