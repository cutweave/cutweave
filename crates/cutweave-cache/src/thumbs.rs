// SPDX-License-Identifier: GPL-3.0-or-later
//! [`ThumbCache`]: thumbnails and filmstrips of video media, generated on demand and kept in the
//! disk cache (arch §7).
//!
//! - [`ThumbCache::thumb`]: one JPEG of the frame on screen at media time `t`, `w` pixels wide
//!   (never upscaled; the height follows the picture's aspect ratio).
//! - [`ThumbCache::filmstrip`]: `count` frames at even intervals over the clip, packed into one
//!   JPEG sprite (a grid of equal tiles, at most [`MAX_SPRITE_WIDTH`] wide, row-major) and a
//!   [`FilmstripManifest`] that says where tile `i` is and what time it shows.
//! - [`submit_prefetch`]: the poster thumbnail and the default filmstrip as a background job
//!   under the decode budget, started after import; cancellable.
//!
//! **Keys and ETags.** An entry is addressed by the media's content hash and a hash of the request
//! parameters (and of the generator version and JPEG quality), so a renamed or relinked file with
//! the same content reuses its entries and a change of quality or algorithm never serves stale
//! bytes. The ETag is `<content hash>-<params hash>` ([`ThumbCache::thumb_etag`]): computed from
//! the request alone, with no I/O, so an `If-None-Match` is answered without decoding or even
//! opening the cache.
//!
//! **Decoding** goes through a [`DecoderPool`] seeded with the stored frame index
//! ([`CacheDb::frame_index_for`]), asking for RGBA frames (`DecodeOpts::with_target`), and the
//! picture is shrunk with an area-averaging box filter here (decoders return full size). A
//! generation call opens its own pool and drops it when done, so no decoder or file handle
//! outlives the request. Two simultaneous requests for the same missing entry both generate it;
//! the results are byte-identical and the later write replaces the earlier.
//!
//! The pictures are the decoded frames as they are: container rotation and display aspect are the
//! decoder's business, not applied here, and colour is not converted (RGBA out of the decoder is
//! treated as sRGB).

use std::path::PathBuf;
use std::sync::Arc;

use cutweave_jobs::{JobCtx, JobError, JobHandle, JobKind, JobManager, JobSpec};
use cutweave_media::{
    CancelCheck, CancelToken, ContentHash, DecoderLease, DecoderPool, FrameIndex, HashMode,
    MediaBackend, PixelFormat, VideoDecoder, VideoFrame, content_hash,
};
use cutweave_media::{DecodeOpts, MediaError};
use cutweave_model::{Duration, MediaId, Time};
use cutweave_render::Image;
use cutweave_render::encode::{DEFAULT_JPEG_QUALITY, encode_jpeg};

use crate::disk::{DiskCache, DiskKind};
use crate::{CacheDb, CacheError};

/// Bumped when generation changes what the same parameters produce (it is part of every key).
const GENERATOR_VERSION: u32 = 1;

/// Widest thumbnail or filmstrip tile accepted.
pub const MAX_THUMB_WIDTH: u32 = 4096;
/// Most tiles in one filmstrip.
pub const MAX_FILMSTRIP_TILES: u32 = 1000;
/// Widest filmstrip sprite: tiles wrap into rows beyond this (a texture limit most GPUs share).
pub const MAX_SPRITE_WIDTH: u32 = 8192;

/// A media file to generate pictures for: where it is and what it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaRef {
    /// The file (read when something has to be decoded).
    pub path: PathBuf,
    /// Its content hash: the cache key.
    pub hash: ContentHash,
}

impl MediaRef {
    /// A file with a hash the caller already has (the media's registered hash).
    #[must_use]
    pub fn new(path: impl Into<PathBuf>, hash: ContentHash) -> MediaRef {
        MediaRef {
            path: path.into(),
            hash,
        }
    }

    /// Hashes `path` (fast mode) to make the reference.
    pub fn from_path(path: impl Into<PathBuf>) -> Result<MediaRef, CacheError> {
        let path = path.into();
        let hash = content_hash(&path, HashMode::Fast)?;
        Ok(MediaRef { path, hash })
    }
}

/// Where and how big the disk cache is, and how thumbnails are encoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThumbConfig {
    /// The cache directory (never inside a project; see [`default_dir`](Self::default_dir)).
    pub dir: PathBuf,
    /// Cap on the generated files' total size, in bytes (default 2 GiB).
    pub max_bytes: u64,
    /// JPEG quality, 1-100 (default 80).
    pub jpeg_quality: u8,
}

impl ThumbConfig {
    /// A config for `dir` with the default cap and quality.
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> ThumbConfig {
        ThumbConfig {
            dir: dir.into(),
            max_bytes: DiskCache::DEFAULT_MAX_BYTES,
            jpeg_quality: DEFAULT_JPEG_QUALITY,
        }
    }

    /// `<OS cache dir>/cutweave/media-cache` (`%LOCALAPPDATA%`, `~/Library/Caches`,
    /// `$XDG_CACHE_HOME` or `~/.cache`). `None` when the OS reports none. Tests and headless
    /// setups pass their own directory to [`new`](Self::new) instead.
    #[must_use]
    pub fn default_dir() -> Option<PathBuf> {
        dirs::cache_dir().map(|d| d.join("cutweave").join("media-cache"))
    }

    /// The same config with a different cap.
    #[must_use]
    pub fn with_max_bytes(mut self, max_bytes: u64) -> ThumbConfig {
        self.max_bytes = max_bytes;
        self
    }

    /// The same config with a different JPEG quality.
    #[must_use]
    pub fn with_quality(mut self, quality: u8) -> ThumbConfig {
        self.jpeg_quality = quality;
        self
    }
}

/// A generated thumbnail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thumb {
    /// The JPEG file.
    pub jpeg: Vec<u8>,
    /// Its width in pixels (the requested width, or the picture's if that is smaller).
    pub width: u32,
    /// Its height in pixels.
    pub height: u32,
    /// The entity tag of this request (see the module docs).
    pub etag: String,
}

/// What filmstrip to make.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FilmstripSpec {
    /// Number of tiles, `1..=`[`MAX_FILMSTRIP_TILES`].
    pub count: u32,
    /// Width of each tile in pixels, `1..=`[`MAX_THUMB_WIDTH`] (never upscaled).
    pub tile_w: u32,
}

impl Default for FilmstripSpec {
    /// 100 tiles of 160 px.
    fn default() -> FilmstripSpec {
        FilmstripSpec {
            count: 100,
            tile_w: 160,
        }
    }
}

/// Where the tiles of a filmstrip sprite are and what they show.
///
/// Tile `i` (`0 <= i < count`) sits at column `i % columns`, row `i / columns`, i.e. at pixel
/// `((i % columns) * tile_w, (i / columns) * tile_h)`, and shows the frame on screen at
/// `start + i * interval`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FilmstripManifest {
    /// Width of one tile in pixels.
    pub tile_w: u32,
    /// Height of one tile in pixels.
    pub tile_h: u32,
    /// Number of tiles.
    pub count: u32,
    /// Tiles per row of the sprite.
    pub columns: u32,
    /// Media time of tile 0 (the stream's first frame).
    pub start: Time,
    /// Media time between tiles.
    pub interval: Duration,
}

impl FilmstripManifest {
    /// Rows in the sprite.
    #[must_use]
    pub fn rows(&self) -> u32 {
        self.count.div_ceil(self.columns)
    }

    /// The sprite's size in pixels.
    #[must_use]
    pub fn sprite_size(&self) -> (u32, u32) {
        (self.columns * self.tile_w, self.rows() * self.tile_h)
    }

    fn to_meta(self) -> String {
        format!(
            "{} {} {} {} {} {}",
            self.tile_w,
            self.tile_h,
            self.count,
            self.columns,
            self.start.flicks(),
            self.interval.flicks()
        )
    }

    fn from_meta(meta: &str) -> Option<FilmstripManifest> {
        let mut it = meta.split(' ');
        let mut next = || it.next()?.parse::<i64>().ok();
        let (tile_w, tile_h, count, columns) = (next()?, next()?, next()?, next()?);
        let (start, interval) = (next()?, next()?);
        let m = FilmstripManifest {
            tile_w: u32::try_from(tile_w).ok()?,
            tile_h: u32::try_from(tile_h).ok()?,
            count: u32::try_from(count).ok()?,
            columns: u32::try_from(columns).ok()?,
            start: Time::from_flicks(start).ok()?,
            interval: Duration::from_flicks(interval).ok()?,
        };
        (m.columns > 0).then_some(m)
    }
}

/// A generated filmstrip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filmstrip {
    /// The sprite, a JPEG of [`FilmstripManifest::sprite_size`].
    pub jpeg: Vec<u8>,
    /// The layout.
    pub manifest: FilmstripManifest,
    /// The entity tag of this request.
    pub etag: String,
}

/// What [`ThumbCache::prefetch`] and [`submit_prefetch`] generate after import.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrefetchSpec {
    /// Width of the poster thumbnail at the stream's start.
    pub poster_width: u32,
    /// The filmstrip.
    pub filmstrip: FilmstripSpec,
}

impl Default for PrefetchSpec {
    /// A 320 px poster and the default filmstrip.
    fn default() -> PrefetchSpec {
        PrefetchSpec {
            poster_width: 320,
            filmstrip: FilmstripSpec::default(),
        }
    }
}

/// Thumbnails and filmstrips over a [`MediaBackend`] and a disk cache (see the module docs).
pub struct ThumbCache {
    backend: Arc<dyn MediaBackend>,
    db: Arc<CacheDb>,
    disk: DiskCache,
    quality: u8,
}

impl std::fmt::Debug for ThumbCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThumbCache")
            .field("disk", &self.disk)
            .field("quality", &self.quality)
            .finish_non_exhaustive()
    }
}

impl ThumbCache {
    /// A cache decoding with `backend`, indexing frames in `db` and keeping files as `config`
    /// says. Nothing is created on disk until the first entry is written.
    #[must_use]
    pub fn new(
        backend: Arc<dyn MediaBackend>,
        db: Arc<CacheDb>,
        config: ThumbConfig,
    ) -> ThumbCache {
        ThumbCache {
            disk: DiskCache::new(Arc::clone(&db), config.dir, config.max_bytes),
            backend,
            db,
            quality: config.jpeg_quality.clamp(1, 100),
        }
    }

    /// The disk cache (usage, cap, [`remove_media`](DiskCache::remove_media)).
    #[must_use]
    pub fn disk(&self) -> &DiskCache {
        &self.disk
    }

    fn params_hash(&self, what: &str, parts: &[i64]) -> String {
        let mut s = format!("{what}/v{GENERATOR_VERSION}/q{}", self.quality);
        for p in parts {
            s.push('/');
            s.push_str(&p.to_string());
        }
        let hex = blake3::hash(s.as_bytes()).to_hex();
        hex.as_str()[..16].to_owned()
    }

    fn etag(media: &MediaRef, params: &str) -> String {
        format!("{}-{params}", media.hash)
    }

    fn thumb_params(&self, t: Time, w: u32) -> String {
        self.params_hash("thumb", &[t.flicks(), i64::from(w)])
    }

    fn filmstrip_params(&self, spec: FilmstripSpec) -> String {
        self.params_hash(
            "filmstrip",
            &[i64::from(spec.count), i64::from(spec.tile_w)],
        )
    }

    /// The ETag [`thumb`](Self::thumb) returns for these parameters (no I/O).
    pub fn thumb_etag(&self, media: &MediaRef, t: Time, w: u32) -> Result<String, CacheError> {
        check_width(w)?;
        Ok(ThumbCache::etag(media, &self.thumb_params(t, w)))
    }

    /// The ETag [`filmstrip`](Self::filmstrip) returns for this spec (no I/O).
    pub fn filmstrip_etag(
        &self,
        media: &MediaRef,
        spec: FilmstripSpec,
    ) -> Result<String, CacheError> {
        check_spec(spec)?;
        Ok(ThumbCache::etag(media, &self.filmstrip_params(spec)))
    }

    /// The thumbnail if it is already cached (never decodes).
    pub fn cached_thumb(
        &self,
        media: &MediaRef,
        t: Time,
        w: u32,
    ) -> Result<Option<Thumb>, CacheError> {
        check_width(w)?;
        let params = self.thumb_params(t, w);
        let Some(entry) = self
            .disk
            .get(DiskKind::Thumb, &media.hash, &format!("{params}.jpg"))?
        else {
            return Ok(None);
        };
        let Some((width, height)) = parse_size(&entry.meta) else {
            return Ok(None);
        };
        Ok(Some(Thumb {
            jpeg: entry.bytes,
            width,
            height,
            etag: ThumbCache::etag(media, &params),
        }))
    }

    /// The thumbnail of the frame on screen at media time `t` (the first frame for a `t` before
    /// the stream, the last for one after it), `w` pixels wide at most: from the cache, or
    /// decoded, shrunk, encoded and stored.
    ///
    /// Errors: [`CacheError::InvalidParams`] for `w` outside `1..=`[`MAX_THUMB_WIDTH`];
    /// [`CacheError::Media`] for unreadable media, media without pictures and
    /// [`MediaError::Cancelled`] once `cancel` is cancelled.
    pub fn thumb(
        &self,
        media: &MediaRef,
        t: Time,
        w: u32,
        cancel: &CancelToken,
    ) -> Result<Thumb, CacheError> {
        if let Some(hit) = self.cached_thumb(media, t, w)? {
            return Ok(hit);
        }
        cancel.check()?;
        let index = self.index(media)?;
        let mut lease = self.checkout(media, &index, t, cancel)?;
        let frame = lease.seek_exact(clamp_to_stream(&index, t))?;
        let (tw, th) = tile_size(&frame, w);
        let pixels = shrink(&frame, tw, th)?;
        let jpeg = self.encode(&Image {
            width: tw,
            height: th,
            data: pixels,
        })?;
        let params = self.thumb_params(t, w);
        self.disk.put(
            DiskKind::Thumb,
            &media.hash,
            &format!("{params}.jpg"),
            &format!("{tw}x{th}"),
            &jpeg,
        )?;
        Ok(Thumb {
            jpeg,
            width: tw,
            height: th,
            etag: ThumbCache::etag(media, &params),
        })
    }

    /// The filmstrip if it is already cached (never decodes).
    pub fn cached_filmstrip(
        &self,
        media: &MediaRef,
        spec: FilmstripSpec,
    ) -> Result<Option<Filmstrip>, CacheError> {
        check_spec(spec)?;
        let params = self.filmstrip_params(spec);
        let Some(entry) =
            self.disk
                .get(DiskKind::Filmstrip, &media.hash, &format!("{params}.jpg"))?
        else {
            return Ok(None);
        };
        let Some(manifest) = FilmstripManifest::from_meta(&entry.meta) else {
            return Ok(None);
        };
        Ok(Some(Filmstrip {
            jpeg: entry.bytes,
            manifest,
            etag: ThumbCache::etag(media, &params),
        }))
    }

    /// [`filmstrip_with_progress`](Self::filmstrip_with_progress) without progress reports.
    pub fn filmstrip(
        &self,
        media: &MediaRef,
        spec: FilmstripSpec,
        cancel: &CancelToken,
    ) -> Result<Filmstrip, CacheError> {
        self.filmstrip_with_progress(media, spec, cancel, &mut |_| {})
    }

    /// `spec.count` frames at even intervals over the clip (tile `i` at `start + i * interval`,
    /// `interval` = the stream's duration / `count`), as one sprite: from the cache, or decoded
    /// front to back through one decoder, shrunk, packed, encoded and stored. `progress` is
    /// called with the done fraction (`0..=1`) after each tile.
    ///
    /// Cancelling returns [`MediaError::Cancelled`] within a tile's decode and stores nothing.
    /// Errors as [`thumb`](Self::thumb), with [`CacheError::InvalidParams`] for a bad spec.
    pub fn filmstrip_with_progress(
        &self,
        media: &MediaRef,
        spec: FilmstripSpec,
        cancel: &CancelToken,
        progress: &mut dyn FnMut(f64),
    ) -> Result<Filmstrip, CacheError> {
        if let Some(hit) = self.cached_filmstrip(media, spec)? {
            progress(1.0);
            return Ok(hit);
        }
        cancel.check()?;
        let index = self.index(media)?;
        let start = index.start_time().ok_or_else(|| no_pictures(media))?;
        let total = index.duration().map_or(0, Duration::flicks);
        let interval = (total / i64::from(spec.count)).max(1);
        let tile_at = |i: u32| {
            let t = start.flicks().saturating_add(interval * i64::from(i));
            clamp_to_stream(&index, Time::from_flicks(t).unwrap_or(start))
        };

        let mut lease = self.checkout(media, &index, start, cancel)?;
        let mut sprite: Option<(Image, FilmstripManifest)> = None;
        for i in 0..spec.count {
            cancel.check()?;
            let frame = lease.seek_exact(tile_at(i))?;
            let (img, m) = sprite.get_or_insert_with(|| {
                // The first picture fixes the tile size.
                let (tw, th) = tile_size(&frame, spec.tile_w);
                let m = FilmstripManifest {
                    tile_w: tw,
                    tile_h: th,
                    count: spec.count,
                    columns: (MAX_SPRITE_WIDTH / tw).clamp(1, spec.count),
                    start,
                    interval: Duration::from_flicks(interval).unwrap_or(Duration::ZERO),
                };
                let (sw, sh) = m.sprite_size();
                (Image::filled(sw, sh, [0, 0, 0, 255]), m)
            });
            let tile = shrink(&frame, m.tile_w, m.tile_h)?;
            blit(img, &tile, m, i);
            progress(f64::from(i + 1) / f64::from(spec.count));
        }
        let (img, manifest) = sprite.ok_or_else(|| no_pictures(media))?;
        let jpeg = self.encode(&img)?;
        let params = self.filmstrip_params(spec);
        self.disk.put(
            DiskKind::Filmstrip,
            &media.hash,
            &format!("{params}.jpg"),
            &manifest.to_meta(),
            &jpeg,
        )?;
        Ok(Filmstrip {
            jpeg,
            manifest,
            etag: ThumbCache::etag(media, &params),
        })
    }

    /// Generates what the UI shows first for a new clip: the poster thumbnail (the first 5% of
    /// the progress) and the filmstrip. Already-cached parts cost nothing. `progress` runs from
    /// 0 to 1.
    pub fn prefetch(
        &self,
        media: &MediaRef,
        spec: PrefetchSpec,
        cancel: &CancelToken,
        progress: &mut dyn FnMut(f64),
    ) -> Result<(), CacheError> {
        self.thumb(media, Time::ZERO, spec.poster_width, cancel)?;
        progress(0.05);
        self.filmstrip_with_progress(media, spec.filmstrip, cancel, &mut |f| {
            progress(0.05 + 0.95 * f);
        })?;
        Ok(())
    }

    fn index(&self, media: &MediaRef) -> Result<Arc<FrameIndex>, CacheError> {
        Ok(Arc::new(self.db.frame_index_for(
            &*self.backend,
            &media.path,
            &media.hash,
        )?))
    }

    fn checkout(
        &self,
        media: &MediaRef,
        index: &Arc<FrameIndex>,
        near: Time,
        cancel: &CancelToken,
    ) -> Result<DecoderLease, CacheError> {
        // One decoder; the pool dies with the lease, closing the file.
        let pool = DecoderPool::with_max_live(Arc::clone(&self.backend), &media.path, 1)
            .with_index(Arc::clone(index));
        let opts = DecodeOpts::default().with_target(PixelFormat::Rgba8);
        let mut lease = pool.checkout(&opts, Some(near), cancel)?;
        lease.set_cancel(Some(cancel.clone()));
        Ok(lease)
    }

    fn encode(&self, image: &Image) -> Result<Vec<u8>, CacheError> {
        let mut out = Vec::new();
        encode_jpeg(image, self.quality, &mut out)
            .map_err(|e| CacheError::Encode(e.to_string()))?;
        Ok(out)
    }
}

/// Queues [`ThumbCache::prefetch`] of `media` as a background job (kind
/// [`JobKind::Analysis`], label `thumbnails`, so it runs under the decode budget). The job
/// reports progress, and cancelling it (`job.cancel`) cancels the decode in flight.
///
/// Submit it for media with a picture stream only: audio-only media fail with "no Video stream".
pub fn submit_prefetch(
    manager: &JobManager,
    cache: Arc<ThumbCache>,
    media: MediaRef,
    media_id: Option<MediaId>,
    spec: PrefetchSpec,
) -> JobHandle {
    let mut job = JobSpec::new(JobKind::Analysis, "thumbnails");
    job.media_id = media_id;
    manager.submit(job, move |ctx: &JobCtx| {
        let token = ctx.token().clone();
        let cancel = CancelToken::linked(CancelCheck::new(move || token.is_cancelled()));
        cache
            .prefetch(&media, spec, &cancel, &mut |f| {
                ctx.progress(f, Some("thumbnails"));
            })
            .map_err(|e| match e {
                CacheError::Media(MediaError::Cancelled) => JobError::Cancelled,
                other => JobError::failed(other.to_string()),
            })
    })
}

fn check_width(w: u32) -> Result<(), CacheError> {
    if (1..=MAX_THUMB_WIDTH).contains(&w) {
        Ok(())
    } else {
        Err(CacheError::InvalidParams(format!(
            "width {w} is outside 1..={MAX_THUMB_WIDTH}"
        )))
    }
}

fn check_spec(spec: FilmstripSpec) -> Result<(), CacheError> {
    check_width(spec.tile_w)?;
    if (1..=MAX_FILMSTRIP_TILES).contains(&spec.count) {
        Ok(())
    } else {
        Err(CacheError::InvalidParams(format!(
            "tile count {} is outside 1..={MAX_FILMSTRIP_TILES}",
            spec.count
        )))
    }
}

fn parse_size(meta: &str) -> Option<(u32, u32)> {
    let (w, h) = meta.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

fn no_pictures(media: &MediaRef) -> CacheError {
    CacheError::Media(MediaError::NoStream {
        path: media.path.display().to_string(),
        kind: cutweave_model::StreamKind::Video,
        index: None,
    })
}

/// `t` limited to the stream's frames (a seek at or past the end is out of range).
fn clamp_to_stream(index: &FrameIndex, t: Time) -> Time {
    let last = index
        .frame_count()
        .checked_sub(1)
        .and_then(|n| index.time_of(n));
    last.map_or(t, |l| t.min(l))
}

/// The size of a `frame` shrunk to at most `w` wide (aspect kept, at least 1 px each way).
fn tile_size(frame: &VideoFrame, w: u32) -> (u32, u32) {
    let tw = w.min(frame.width).max(1);
    let th = (u64::from(frame.height) * u64::from(tw) + u64::from(frame.width) / 2)
        / u64::from(frame.width.max(1));
    (tw, u32::try_from(th).unwrap_or(u32::MAX).max(1))
}

/// `frame` (RGBA) shrunk to exactly `dw × dh` with an area-averaging box filter; packed RGBA with
/// opaque alpha.
fn shrink(frame: &VideoFrame, dw: u32, dh: u32) -> Result<Vec<u8>, CacheError> {
    let plane = frame
        .planes
        .first()
        .filter(|_| frame.format == PixelFormat::Rgba8)
        .ok_or_else(|| {
            CacheError::Encode(format!(
                "the decoder returned {:?}, not the requested Rgba8",
                frame.format
            ))
        })?;
    let (sw, sh) = (frame.width as usize, frame.height as usize);
    let stride = plane.stride as usize;
    if sw == 0 || sh == 0 || plane.data.len() < stride * (sh - 1) + sw * 4 {
        return Err(CacheError::Encode(
            "the decoded picture is empty".to_owned(),
        ));
    }
    let (dw, dh) = (dw as usize, dh as usize);
    // Source column range of each destination column.
    let xs: Vec<(usize, usize)> = (0..dw)
        .map(|dx| {
            let x0 = dx * sw / dw;
            (x0, ((dx + 1) * sw / dw).max(x0 + 1))
        })
        .collect();
    let mut out = vec![255_u8; dw * dh * 4];
    let mut acc = vec![0_u32; dw * 3];
    for dy in 0..dh {
        let y0 = dy * sh / dh;
        let y1 = ((dy + 1) * sh / dh).max(y0 + 1);
        acc.fill(0);
        for y in y0..y1 {
            let row = &plane.data[y * stride..y * stride + sw * 4];
            for (dx, &(x0, x1)) in xs.iter().enumerate() {
                let a = &mut acc[dx * 3..dx * 3 + 3];
                for px in row[x0 * 4..x1 * 4].as_chunks::<4>().0 {
                    a[0] += u32::from(px[0]);
                    a[1] += u32::from(px[1]);
                    a[2] += u32::from(px[2]);
                }
            }
        }
        for (dx, &(x0, x1)) in xs.iter().enumerate() {
            let n = u32::try_from((y1 - y0) * (x1 - x0)).unwrap_or(u32::MAX);
            let o = (dy * dw + dx) * 4;
            for c in 0..3 {
                // Rounded mean of at most 255 per sample: fits a byte.
                out[o + c] = u8::try_from((acc[dx * 3 + c] + n / 2) / n).unwrap_or(255);
            }
        }
    }
    Ok(out)
}

/// Copies packed RGBA `tile` into tile slot `i` of `sprite`.
fn blit(sprite: &mut Image, tile: &[u8], m: &FilmstripManifest, i: u32) {
    let (col, row) = (i % m.columns, i / m.columns);
    let (tw, th) = (m.tile_w as usize, m.tile_h as usize);
    let sprite_stride = sprite.width as usize * 4;
    let (x0, y0) = (col as usize * tw, row as usize * th);
    for y in 0..th {
        let dst = (y0 + y) * sprite_stride + x0 * 4;
        sprite.data[dst..dst + tw * 4].copy_from_slice(&tile[y * tw * 4..(y + 1) * tw * 4]);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration as Wall;

    use cutweave_jobs::{JobConfig, JobState};
    use cutweave_media::testing::contract::TempDir;
    use cutweave_media::testing::{FakeMedia, FakeVideo};
    use cutweave_media::{FakeBackend, Plane};
    use cutweave_model::{ColorTags, FrameRate};

    use super::*;

    fn secs(s: i64) -> Duration {
        Duration::from_secs(s).expect("duration")
    }

    fn at(s: i64) -> Time {
        Time::from_secs(s).expect("time")
    }

    /// A temp dir, a real file in it (so it hashes) served as a `w × h` 30 fps clip, and a cache
    /// whose directory is a sibling of the file's (never inside it).
    struct Rig {
        _dir: TempDir,
        cache_dir: PathBuf,
        media: MediaRef,
        backend: FakeBackend,
        cache: ThumbCache,
        db: Arc<CacheDb>,
    }

    fn rig(tag: &str, w: u32, h: u32, seconds: i64, max_bytes: u64) -> Rig {
        let dir = TempDir::new(tag);
        let path = dir.path().join("project").join("clip.mp4");
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, tag.as_bytes()).expect("write");
        let media = MediaRef::from_path(&path).expect("hash");
        let video = FakeVideo::lasting(w, h, FrameRate::FPS_30, secs(seconds));
        let backend = FakeBackend::new().with_media(path, FakeMedia::video(video));
        let db = Arc::new(CacheDb::open_in_memory().expect("db"));
        let cache_dir = dir.path().join("os-cache");
        let cache = ThumbCache::new(
            Arc::new(backend.clone()),
            Arc::clone(&db),
            ThumbConfig::new(&cache_dir).with_max_bytes(max_bytes),
        );
        Rig {
            _dir: dir,
            cache_dir,
            media,
            backend,
            cache,
            db,
        }
    }

    fn decode_jpeg(bytes: &[u8]) -> Image {
        use zune_jpeg::JpegDecoder;
        use zune_jpeg::zune_core::bytestream::ZCursor;
        use zune_jpeg::zune_core::colorspace::ColorSpace;
        use zune_jpeg::zune_core::options::DecoderOptions;
        let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
        let mut d = JpegDecoder::new_with_options(ZCursor::new(bytes), options);
        let data = d.decode().expect("valid JPEG");
        let info = d.info().expect("info");
        Image {
            width: u32::from(info.width),
            height: u32::from(info.height),
            data,
        }
    }

    #[test]
    fn a_thumb_is_a_decodable_jpeg_of_the_requested_width() {
        let r = rig("thumb-jpeg", 320, 180, 2, u64::MAX);
        let t = r
            .cache
            .thumb(&r.media, at(1), 100, &CancelToken::new())
            .expect("thumb");
        assert_eq!((t.width, t.height), (100, 56));
        let img = decode_jpeg(&t.jpeg);
        assert_eq!((img.width, img.height), (100, 56));
        // Never upscaled: a request wider than the picture gets the picture's width.
        let big = r
            .cache
            .thumb(&r.media, at(1), 1000, &CancelToken::new())
            .expect("thumb");
        assert_eq!((big.width, big.height), (320, 180));
    }

    #[test]
    fn the_second_request_for_a_thumb_decodes_nothing() {
        let r = rig("thumb-twice", 64, 36, 2, u64::MAX);
        let cancel = CancelToken::new();
        let a = r.cache.thumb(&r.media, at(1), 32, &cancel).expect("first");
        let (frames, opened, scans) = (
            r.backend.decoded_frames(),
            r.backend.decoders_opened(),
            r.backend.index_scans(),
        );
        assert!(frames > 0, "the first request decodes");
        let b = r.cache.thumb(&r.media, at(1), 32, &cancel).expect("second");
        assert_eq!(a, b);
        assert_eq!(r.backend.decoded_frames(), frames, "0 decodes");
        assert_eq!(r.backend.decoders_opened(), opened);
        assert_eq!(r.backend.index_scans(), scans);
        // A different time or width is a different entry.
        r.cache.thumb(&r.media, at(1), 16, &cancel).expect("other");
        assert!(r.backend.decoded_frames() > frames);
        assert_eq!(r.cache.disk().usage().expect("usage").entries, 2);
    }

    #[test]
    fn a_time_past_the_end_gives_the_last_frame_and_one_before_the_start_the_first() {
        let r = rig("thumb-clamp", 64, 36, 1, u64::MAX);
        let c = CancelToken::new();
        r.cache.thumb(&r.media, at(100), 32, &c).expect("past end");
        r.cache
            .thumb(&r.media, Time::from_flicks(-5).expect("t"), 32, &c)
            .expect("before start");
    }

    #[test]
    fn the_cache_evicts_the_oldest_entries_over_its_cap() {
        // Size one entry first, then cap the cache at two and a half of them.
        let probe = rig("evict-probe", 64, 36, 2, u64::MAX);
        let c = CancelToken::new();
        let one = probe
            .cache
            .thumb(&probe.media, at(0), 64, &c)
            .expect("thumb")
            .jpeg
            .len() as u64;

        let r = rig("evict", 64, 36, 2, one * 5 / 2);
        let times: Vec<Time> = (0..5)
            .map(|i| Time::from_flicks(i * 1000).expect("t"))
            .collect();
        for &t in &times {
            r.cache.thumb(&r.media, t, 64, &c).expect("thumb");
        }
        let usage = r.cache.disk().usage().expect("usage");
        assert!(usage.bytes <= one * 5 / 2, "{usage:?} over the cap");
        assert!(usage.entries < 5, "something was evicted");
        // The newest survived, the oldest did not: no decode for one, a decode for the other.
        let before = r.backend.decoded_frames();
        r.cache.thumb(&r.media, times[4], 64, &c).expect("newest");
        assert_eq!(r.backend.decoded_frames(), before, "newest is cached");
        assert!(
            r.cache
                .cached_thumb(&r.media, times[0], 64)
                .expect("lookup")
                .is_none(),
            "oldest was evicted"
        );
        // And the file went with its row.
        let files = walk(&r.cache_dir).len();
        assert_eq!(files as u64, usage.entries);
    }

    fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(dir) else {
            return out;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
        out
    }

    #[test]
    fn the_least_recently_used_entry_goes_first_not_the_oldest_written() {
        let probe = rig("lru-probe", 64, 36, 2, u64::MAX);
        let c = CancelToken::new();
        let one = probe
            .cache
            .thumb(&probe.media, at(0), 64, &c)
            .expect("thumb")
            .jpeg
            .len() as u64;
        let r = rig("lru", 64, 36, 2, one * 5 / 2);
        let (t0, t1, t2) = (at(0), at(1), Time::from_flicks(7).expect("t"));
        r.cache.thumb(&r.media, t0, 64, &c).expect("0");
        r.cache.thumb(&r.media, t1, 64, &c).expect("1");
        // Reading t0 makes t1 the least recently used.
        r.cache.thumb(&r.media, t0, 64, &c).expect("0 again");
        r.cache.thumb(&r.media, t2, 64, &c).expect("2");
        assert!(r.cache.cached_thumb(&r.media, t0, 64).expect("q").is_some());
        assert!(r.cache.cached_thumb(&r.media, t1, 64).expect("q").is_none());
    }

    #[test]
    fn deleting_the_cache_directory_breaks_nothing_and_regenerates() {
        let r = rig("regen", 64, 36, 2, u64::MAX);
        let c = CancelToken::new();
        let a = r.cache.thumb(&r.media, at(1), 32, &c).expect("thumb");
        let strip = r
            .cache
            .filmstrip(
                &r.media,
                FilmstripSpec {
                    count: 4,
                    tile_w: 16,
                },
                &c,
            )
            .expect("strip");
        assert!(r.cache_dir.exists());
        // The cache lives beside the project, never in it.
        assert!(!r.cache_dir.starts_with(r.media.path.parent().expect("dir")));

        std::fs::remove_dir_all(&r.cache_dir).expect("rm cache dir");
        let before = r.backend.decoded_frames();
        let b = r.cache.thumb(&r.media, at(1), 32, &c).expect("regenerated");
        assert_eq!(a, b, "same bytes");
        assert!(r.backend.decoded_frames() > before, "decoded again");
        let strip2 = r
            .cache
            .filmstrip(
                &r.media,
                FilmstripSpec {
                    count: 4,
                    tile_w: 16,
                },
                &c,
            )
            .expect("strip again");
        assert_eq!(strip, strip2);
        assert_eq!(r.cache.disk().usage().expect("usage").entries, 2);
        // The project's own file is untouched.
        assert!(r.media.path.exists());
    }

    #[test]
    fn a_filmstrip_packs_even_tiles_with_a_manifest_and_decodes_once() {
        let r = rig("strip", 160, 90, 10, u64::MAX);
        let c = CancelToken::new();
        let spec = FilmstripSpec {
            count: 10,
            tile_w: 40,
        };
        let mut seen = Vec::new();
        let fs = r
            .cache
            .filmstrip_with_progress(&r.media, spec, &c, &mut |f| seen.push(f))
            .expect("strip");
        let m = fs.manifest;
        assert_eq!((m.tile_w, m.tile_h, m.count), (40, 23, 10));
        assert_eq!(m.columns, 10, "all ten fit one row");
        assert_eq!(m.start, Time::ZERO);
        assert_eq!(m.interval, secs(1));
        assert_eq!(seen.len(), 10);
        assert!((seen[9] - 1.0).abs() < 1e-9 && seen.windows(2).all(|w| w[0] < w[1]));

        let img = decode_jpeg(&fs.jpeg);
        assert_eq!((img.width, img.height), m.sprite_size());
        // Tiles show different frames: the fake picture changes with the frame number.
        let tile_px = |i: u32| {
            img.pixel(
                (i % m.columns) * m.tile_w + 5,
                (i / m.columns) * m.tile_h + 5,
            )
        };
        assert_ne!(tile_px(0), tile_px(1));
        assert_ne!(tile_px(1), tile_px(2));

        // Second request: nothing decoded, same bytes and manifest.
        let before = r.backend.decoded_frames();
        let again = r.cache.filmstrip(&r.media, spec, &c).expect("cached");
        assert_eq!(again, fs);
        assert_eq!(r.backend.decoded_frames(), before);
    }

    #[test]
    fn a_wide_filmstrip_wraps_into_rows_at_the_sprite_width_limit() {
        let r = rig("strip-wrap", 640, 360, 4, u64::MAX);
        let fs = r
            .cache
            .filmstrip(
                &r.media,
                FilmstripSpec {
                    count: 30,
                    tile_w: 640,
                },
                &CancelToken::new(),
            )
            .expect("strip");
        let m = fs.manifest;
        assert_eq!(m.columns, 12, "8192 / 640");
        assert_eq!(m.rows(), 3);
        assert!(m.sprite_size().0 <= MAX_SPRITE_WIDTH);
        let img = decode_jpeg(&fs.jpeg);
        assert_eq!((img.width, img.height), m.sprite_size());
    }

    #[test]
    fn cancelling_stops_generation_and_stores_nothing() {
        let r = rig("cancel", 64, 36, 10, u64::MAX);
        // Cancelled before it starts.
        let c = CancelToken::new();
        c.cancel();
        assert!(matches!(
            r.cache.thumb(&r.media, at(1), 32, &c),
            Err(CacheError::Media(MediaError::Cancelled))
        ));
        // Cancelled after the third tile.
        let c = CancelToken::new();
        let mut tiles = 0;
        let res = r.cache.filmstrip_with_progress(
            &r.media,
            FilmstripSpec {
                count: 20,
                tile_w: 16,
            },
            &c,
            &mut |_| {
                tiles += 1;
                if tiles == 3 {
                    c.cancel();
                }
            },
        );
        assert!(matches!(res, Err(CacheError::Media(MediaError::Cancelled))));
        assert_eq!(tiles, 3, "no tile after the cancel");
        assert_eq!(r.cache.disk().usage().expect("usage").entries, 0);
        assert_eq!(walk(&r.cache_dir).len(), 0);
    }

    #[test]
    fn bad_parameters_are_rejected_before_any_work() {
        let r = rig("params", 64, 36, 1, u64::MAX);
        let c = CancelToken::new();
        for w in [0, MAX_THUMB_WIDTH + 1] {
            assert!(matches!(
                r.cache.thumb(&r.media, at(0), w, &c),
                Err(CacheError::InvalidParams(_))
            ));
        }
        for (count, tile_w) in [(0, 16), (MAX_FILMSTRIP_TILES + 1, 16), (4, 0), (4, 5000)] {
            assert!(matches!(
                r.cache
                    .filmstrip(&r.media, FilmstripSpec { count, tile_w }, &c),
                Err(CacheError::InvalidParams(_))
            ));
        }
        assert_eq!(r.backend.decoders_opened(), 0);
    }

    #[test]
    fn etags_follow_the_content_hash_and_the_parameters_without_io() {
        let r = rig("etag", 64, 36, 1, u64::MAX);
        let e = |t: Time, w| r.cache.thumb_etag(&r.media, t, w).expect("etag");
        assert_eq!(e(at(0), 32), e(at(0), 32));
        assert_ne!(e(at(0), 32), e(at(0), 33));
        assert_ne!(e(at(0), 32), e(at(1), 32));
        assert!(e(at(0), 32).starts_with(&r.media.hash.to_string()));
        // The generated thumb carries the same tag; nothing was decoded to compute it.
        assert_eq!(r.backend.decoders_opened(), 0);
        let t = r
            .cache
            .thumb(&r.media, at(0), 32, &CancelToken::new())
            .expect("thumb");
        assert_eq!(t.etag, e(at(0), 32));
        // A different quality is a different tag (cached bytes differ).
        let hq = ThumbCache::new(
            Arc::new(r.backend.clone()),
            Arc::clone(&r.db),
            ThumbConfig::new(&r.cache_dir).with_quality(95),
        );
        assert_ne!(
            hq.thumb_etag(&r.media, at(0), 32).expect("etag"),
            e(at(0), 32)
        );
        let s = FilmstripSpec {
            count: 4,
            tile_w: 16,
        };
        assert_ne!(
            r.cache.filmstrip_etag(&r.media, s).expect("etag"),
            r.cache
                .filmstrip_etag(&r.media, FilmstripSpec { count: 5, ..s })
                .expect("etag")
        );
    }

    #[test]
    fn media_without_pictures_is_a_no_stream_error() {
        let dir = TempDir::new("nopic");
        let path = dir.path().join("tone.wav");
        std::fs::write(&path, b"x").expect("write");
        let backend = FakeBackend::new().with_media(
            path.clone(),
            FakeMedia::audio(cutweave_media::testing::FakeAudio::tone(
                cutweave_model::SampleRate::HZ_48_000,
                1,
                secs(1),
            )),
        );
        let cache = ThumbCache::new(
            Arc::new(backend),
            Arc::new(CacheDb::open_in_memory().expect("db")),
            ThumbConfig::new(dir.path().join("c")),
        );
        let media = MediaRef::from_path(&path).expect("media");
        assert!(matches!(
            cache.thumb(&media, at(0), 32, &CancelToken::new()),
            Err(CacheError::Media(MediaError::NoStream { .. }))
        ));
    }

    #[test]
    fn the_prefetch_job_warms_the_cache_and_reports_progress() {
        let r = rig("job", 64, 36, 4, u64::MAX);
        let manager = JobManager::new(JobConfig::default()).expect("jobs");
        let cache = Arc::new(r.cache);
        let spec = PrefetchSpec {
            poster_width: 32,
            filmstrip: FilmstripSpec {
                count: 8,
                tile_w: 16,
            },
        };
        let handle = submit_prefetch(&manager, Arc::clone(&cache), r.media.clone(), None, spec);
        let info = handle.wait(Wall::from_secs(30)).expect("known job");
        assert_eq!(info.state, JobState::Succeeded, "{info:?}");
        assert!((info.progress.fraction - 1.0).abs() < 1e-9);

        let before = r.backend.decoded_frames();
        let cancel = CancelToken::new();
        cache
            .thumb(&r.media, Time::ZERO, 32, &cancel)
            .expect("poster");
        cache
            .filmstrip(&r.media, spec.filmstrip, &cancel)
            .expect("strip");
        assert_eq!(r.backend.decoded_frames(), before, "both were prefetched");
    }

    #[test]
    fn cancelling_the_prefetch_job_stops_the_decode() {
        // Slow frames, so the job is still decoding when it is cancelled.
        let dir = TempDir::new("job-cancel");
        let path = dir.path().join("clip.mp4");
        std::fs::write(&path, b"slow").expect("write");
        let media = MediaRef::from_path(&path).expect("media");
        let video = FakeVideo::lasting(64, 36, FrameRate::FPS_30, secs(60));
        let backend = FakeBackend::new()
            .with_media(path, FakeMedia::video(video))
            .with_frame_cost(Wall::from_millis(5));
        let cache = Arc::new(ThumbCache::new(
            Arc::new(backend.clone()),
            Arc::new(CacheDb::open_in_memory().expect("db")),
            ThumbConfig::new(dir.path().join("c")),
        ));
        let manager = JobManager::new(JobConfig::default()).expect("jobs");
        let handle = submit_prefetch(
            &manager,
            cache.clone(),
            media,
            None,
            PrefetchSpec::default(),
        );
        while backend.decoded_frames() < 3 {
            std::thread::sleep(Wall::from_millis(2));
        }
        let t0 = std::time::Instant::now();
        handle.cancel();
        let info = handle.wait(Wall::from_secs(10)).expect("known job");
        assert_eq!(info.state, JobState::Cancelled, "{info:?}");
        assert!(t0.elapsed() < Wall::from_millis(500), "stopped promptly");
        assert_eq!(
            cache.disk().usage().expect("usage").entries,
            1,
            "only the poster"
        );
    }

    #[test]
    fn the_box_filter_averages_and_keeps_flat_colour() {
        let frame = |w: u32, h: u32, px: &dyn Fn(u32, u32) -> [u8; 4]| {
            let mut data = Vec::new();
            for y in 0..h {
                for x in 0..w {
                    data.extend_from_slice(&px(x, y));
                }
            }
            VideoFrame {
                pts: Time::ZERO,
                width: w,
                height: h,
                format: PixelFormat::Rgba8,
                planes: vec![Plane {
                    data,
                    stride: w * 4,
                }],
                color: ColorTags::default(),
                damaged: false,
            }
        };
        let flat = frame(7, 5, &|_, _| [10, 200, 90, 255]);
        let out = shrink(&flat, 3, 2).expect("shrink");
        assert!(
            out.as_chunks::<4>()
                .0
                .iter()
                .all(|p| *p == [10, 200, 90, 255])
        );

        // Left half black, right half white, shrunk to 2x1: exactly the halves.
        let split = frame(4, 2, &|x, _| if x < 2 { [0, 0, 0, 255] } else { [255; 4] });
        assert_eq!(
            shrink(&split, 2, 1).expect("shrink"),
            [0, 0, 0, 255, 255, 255, 255, 255]
        );
        // Checkerboard shrunk to one pixel: the mean.
        let board = frame(2, 2, &|x, y| {
            if (x + y) % 2 == 0 { [0; 4] } else { [255; 4] }
        });
        assert_eq!(shrink(&board, 1, 1).expect("shrink")[..3], [128, 128, 128]);
        // Not RGBA: refused, not misread.
        let mut yuv = flat.clone();
        yuv.format = PixelFormat::Yuv420p;
        assert!(shrink(&yuv, 3, 2).is_err());
    }

    #[test]
    fn the_default_cache_dir_is_the_os_cache_not_the_working_directory() {
        if let Some(dir) = ThumbConfig::default_dir() {
            assert!(dir.is_absolute());
            assert!(dir.ends_with("cutweave/media-cache"));
            assert!(!dir.starts_with(std::env::current_dir().expect("cwd")));
        }
    }
}
