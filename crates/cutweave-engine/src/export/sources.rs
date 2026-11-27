// SPDX-License-Identifier: GPL-3.0-or-later
//! Clip pictures and sound for an export, from a [`MediaBackend`]'s decoders: the
//! [`ExportSources`] seam and [`DecoderSources`], its implementation over the media layer.
//!
//! - [`DecoderFrameSource`] is the compositor's [`FrameSource`] for one export. It keeps one
//!   decoder per media asset and walks it **forward** (`next_frame`) as the export advances,
//!   seeking (`seek_exact`) only on the first request, on a backward jump or a jump of more
//!   than [`FORWARD_DECODE_LIMIT`]: an export decodes each clip once, sequentially, with no
//!   per-frame seek. The frame shown for a source time is the last frame whose pts is at or
//!   before it, exactly as `seek_exact` defines it, so both paths give the same pictures.
//! - [`DecoderPcmFactory`] is the mixdown's [`SourceFactory`]: each clip reads interleaved
//!   `f32` from an `AudioDecoder` through `decode_range`, which joins consecutive ranges without
//!   gaps.
//!
//! Media are resolved through the exported document snapshot (`MediaAsset::path_abs`), so a
//! relink after the export started does not change it. Planes move from the decoder's frame
//! into the compositor's without a copy (same layout, see `cutweave_media::frame`).

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use cutweave_audio::{PcmFormat, PcmSource, SourceError, SourceFactory, SourceRequest};
use cutweave_media::{AudioDecoder, DecodeOpts, HwMode, MediaBackend, MediaError, VideoDecoder};
use cutweave_model::{FLICKS_PER_SECOND, MediaAsset, MediaId, Project, Time};
use cutweave_render::{FrameError, FrameId, FrameRequest, FrameSource, VideoFrame};

/// A source time further ahead of the frame shown last than this many flicks (2 s) is reached
/// by a seek instead of decoding forward (a jump between clips of the same media).
pub const FORWARD_DECODE_LIMIT: i64 = 2 * FLICKS_PER_SECOND;

/// Where an export's clip pictures and sound come from. The full engine (WP-1I.05) passes
/// [`DecoderSources`] over its media backend (or its decoder pool / frame cache once they
/// exist); tests pass `FakeBackend`.
pub trait ExportSources: Send + Sync {
    /// The frame source for exporting `doc` (one per export: it holds that export's decoders).
    fn video(&self, doc: &Project) -> Arc<dyn FrameSource + Send + Sync>;

    /// The audio sources for mixing `doc` down.
    fn audio(&self, doc: &Project) -> Arc<dyn SourceFactory>;

    /// Whether the file of `asset` is not on this computer, so its clips export as the "media
    /// offline" slate and silence (WP-2F.03c). Sources that cannot tell say `false`.
    fn offline(&self, _asset: &MediaAsset) -> bool {
        false
    }
}

/// [`ExportSources`] over a [`MediaBackend`]'s decoders (see the module docs).
#[derive(Clone)]
pub struct DecoderSources {
    backend: Arc<dyn MediaBackend>,
}

impl DecoderSources {
    /// Sources decoding with `backend`.
    #[must_use]
    pub fn new(backend: Arc<dyn MediaBackend>) -> DecoderSources {
        DecoderSources { backend }
    }
}

impl std::fmt::Debug for DecoderSources {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecoderSources").finish_non_exhaustive()
    }
}

impl ExportSources for DecoderSources {
    fn video(&self, doc: &Project) -> Arc<dyn FrameSource + Send + Sync> {
        Arc::new(DecoderFrameSource::new(Arc::clone(&self.backend), doc))
    }

    fn audio(&self, doc: &Project) -> Arc<dyn SourceFactory> {
        Arc::new(DecoderPcmFactory::new(Arc::clone(&self.backend), doc))
    }

    fn offline(&self, asset: &MediaAsset) -> bool {
        media_offline(&*self.backend, asset)
    }
}

/// 64-bit FNV-1a, for frame identities.
struct Fnv(u64);

impl Fnv {
    fn new() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    fn bytes(mut self, bytes: &[u8]) -> Fnv {
        for &b in bytes {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
        self
    }
}

/// A media asset as the export sees it.
#[derive(Clone, Debug)]
struct Asset {
    path: PathBuf,
    /// Hash of the content hash (or path): part of every frame id, so the GPU decode cache
    /// never mixes up two files that one asset id named at different times.
    identity: u64,
    /// The layout to decode into when the decoder's own is not one the compositor reads.
    target: Option<cutweave_media::PixelFormat>,
}

/// The identity of a media file in frame ids: a hash of its content hash (or path).
pub(crate) fn media_identity(key: &str) -> u64 {
    Fnv::new().bytes(key.as_bytes()).0
}

/// The layout to convert `format` into for the compositor, which reads `Nv12`, `P010`,
/// `Yuv420p` and `Rgba8`: 10-bit planar to `P010`, 8-bit 4:2:2 to `Yuv420p` (the decoder
/// converts with swscale). `None`: the compositor reads it as is.
pub(crate) fn compositor_target(
    format: cutweave_media::PixelFormat,
) -> Option<cutweave_media::PixelFormat> {
    use cutweave_media::PixelFormat as M;
    match format {
        M::Yuv420p10 | M::Yuv422p10 => Some(M::P010),
        M::Yuv422p => Some(M::Yuv420p),
        _ => None,
    }
}

/// [`compositor_target`] for a software decoder of a stream whose FFmpeg pixel format is
/// `name` (as the probe reports it): 10/12-bit Y'CbCr to `P010`, 8-bit 4:2:2 and 4:4:4 to
/// `Yuv420p` (the decoder returns those in layouts the compositor does not read).
pub(crate) fn target_for_pixel_format(name: &str) -> Option<cutweave_media::PixelFormat> {
    use cutweave_media::PixelFormat as M;
    let name = name.to_ascii_lowercase();
    let yuv = name.starts_with("yuv");
    if yuv && (name.contains("p10") || name.contains("p12")) {
        Some(M::P010)
    } else if yuv && (name.contains("422") || name.contains("444") || name.contains("411")) {
        Some(M::Yuv420p)
    } else {
        None
    }
}

fn assets(doc: &Project) -> HashMap<MediaId, Asset> {
    doc.media
        .values()
        .filter_map(|m| {
            // The file, or the locator of an image sequence (ADR-0035).
            let path = m.source_path()?;
            let key = m.content_hash.as_deref().unwrap_or(&path);
            let target = m
                .probe
                .streams
                .iter()
                .find(|s| s.kind == cutweave_model::StreamKind::Video)
                .and_then(|s| s.pixel_format.as_deref())
                .and_then(target_for_pixel_format);
            Some((
                m.id,
                Asset {
                    identity: media_identity(key),
                    path: PathBuf::from(&path),
                    target,
                },
            ))
        })
        .collect()
}

/// The compositor's frame for a decoded one of the media with `identity`
/// ([`media_identity`]): same planes, an id from the media and the pts.
pub(crate) fn compositor_frame(
    identity: u64,
    frame: cutweave_media::VideoFrame,
) -> Result<Arc<VideoFrame>, FrameError> {
    use cutweave_media::PixelFormat as M;
    use cutweave_render::PixelFormat as R;
    let format = match frame.format {
        M::Nv12 => R::Nv12,
        M::P010 => R::P010,
        M::Yuv420p => R::Yuv420p,
        M::Rgba8 => R::Rgba8,
        other => {
            return Err(FrameError::Unavailable(format!(
                "the compositor cannot read {other:?} frames"
            )));
        }
    };
    let id = FrameId(
        Fnv::new()
            .bytes(&identity.to_le_bytes())
            .bytes(&frame.pts.flicks().to_le_bytes())
            .bytes(format!("{format:?}").as_bytes())
            .bytes(&frame.width.to_le_bytes())
            .bytes(&frame.height.to_le_bytes())
            .0,
    );
    let planes = frame
        .planes
        .into_iter()
        .map(|p| cutweave_render::Plane {
            data: p.data,
            stride: p.stride,
        })
        .collect();
    VideoFrame::new(id, frame.width, frame.height, format, planes, frame.color).map(Arc::new)
}

/// One media's decoder and where it is.
struct Stream {
    decoder: Box<dyn VideoDecoder>,
    /// The frame shown last and its pts.
    current: Option<(Time, Arc<VideoFrame>)>,
    /// The frame after it, decoded but not shown yet.
    ahead: Option<cutweave_media::VideoFrame>,
    /// `next_frame` reported the end of the stream.
    ended: bool,
}

/// How exports decode: in **software** (`HwMode::Off`), so an export is the same file on every
/// machine (hardware decoders are not bit-exact across vendors) and matches `render.frame`,
/// which decodes the same way; into a layout the compositor reads (`target`).
fn export_decode_opts(target: Option<cutweave_media::PixelFormat>) -> DecodeOpts {
    let mut opts = DecodeOpts {
        hw: HwMode::Off,
        ..DecodeOpts::default()
    };
    if let Some(target) = target {
        opts = opts.with_target(target);
    }
    opts
}

/// The compositor's frame source for one export (see the module docs).
pub struct DecoderFrameSource {
    backend: Arc<dyn MediaBackend>,
    assets: HashMap<MediaId, Asset>,
    streams: Mutex<HashMap<MediaId, Stream>>,
    /// Media whose file was not there when the export first asked for it: offline for the
    /// whole export (one picture per clip, whatever happens on the disk meanwhile).
    offline: Mutex<HashSet<MediaId>>,
}

impl std::fmt::Debug for DecoderFrameSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecoderFrameSource")
            .field("assets", &self.assets.len())
            .finish_non_exhaustive()
    }
}

/// The compositor's error for a media-layer failure on `media`: [`FrameError::Offline`] when
/// its file is not there (the layer then shows the "media offline" slate, WP-2F.03c), else
/// [`FrameError::Unavailable`] (the layer is left out).
pub(crate) fn frame_error(media: MediaId, e: &MediaError) -> FrameError {
    match e {
        MediaError::NotFound { .. } => FrameError::Offline(media),
        _ => FrameError::Unavailable(format!("media {media}: {e}")),
    }
}

fn unavailable(media: MediaId, e: &MediaError) -> FrameError {
    frame_error(media, e)
}

/// Whether the file of `asset` is not on this computer, as `backend` sees it: a file the disk
/// has is there without asking the backend; otherwise the backend's probe says whether it can
/// find it ([`MediaError::NotFound`]). An image sequence is asked for by its first picture,
/// then by its locator. Media without a file is never offline.
#[must_use]
pub fn media_offline(backend: &dyn MediaBackend, asset: &MediaAsset) -> bool {
    let Some(first) = asset.path_abs.as_deref() else {
        return false;
    };
    if std::path::Path::new(first).is_file() {
        return false;
    }
    let path = asset.source_path().unwrap_or_else(|| first.to_owned());
    matches!(
        backend.probe(std::path::Path::new(&path)),
        Err(MediaError::NotFound { .. })
    )
}

impl DecoderFrameSource {
    /// A source for the media of `doc`, decoding with `backend`.
    #[must_use]
    pub fn new(backend: Arc<dyn MediaBackend>, doc: &Project) -> DecoderFrameSource {
        DecoderFrameSource {
            backend,
            assets: assets(doc),
            streams: Mutex::new(HashMap::new()),
            offline: Mutex::new(HashSet::new()),
        }
    }

    /// The compositor's frame for a decoded one: same planes, an id from the asset and pts.
    fn convert(
        asset: &Asset,
        frame: cutweave_media::VideoFrame,
    ) -> Result<Arc<VideoFrame>, FrameError> {
        compositor_frame(asset.identity, frame)
    }

    /// The frame on screen at `t` in `stream` (see the module docs).
    fn frame_at(
        media: MediaId,
        asset: &Asset,
        stream: &mut Stream,
        t: Time,
    ) -> Result<Arc<VideoFrame>, FrameError> {
        let forward = match &stream.current {
            Some((pts, _)) => {
                *pts <= t
                    && t.duration_since(*pts)
                        .is_ok_and(|d| d.flicks() <= FORWARD_DECODE_LIMIT)
            }
            None => false,
        };
        if !forward {
            let frame = stream
                .decoder
                .seek_exact(t)
                .map_err(|e| unavailable(media, &e))?;
            let pts = frame.pts;
            stream.current = Some((pts, Self::convert(asset, frame)?));
            stream.ahead = None;
            stream.ended = false;
        }
        loop {
            let next = match stream.ahead.take() {
                Some(f) => f,
                None if stream.ended => break,
                None => {
                    let decoded = stream
                        .decoder
                        .next_frame()
                        .map_err(|e| unavailable(media, &e))?;
                    let Some(f) = decoded else {
                        stream.ended = true;
                        break;
                    };
                    f
                }
            };
            if next.pts <= t {
                let pts = next.pts;
                stream.current = Some((pts, Self::convert(asset, next)?));
            } else {
                stream.ahead = Some(next);
                break;
            }
        }
        stream
            .current
            .as_ref()
            .map(|(_, f)| Arc::clone(f))
            .ok_or_else(|| FrameError::Unavailable(format!("media {media} has no frame at {t}")))
    }
}

impl FrameSource for DecoderFrameSource {
    fn frame(&self, request: &FrameRequest) -> Result<Arc<VideoFrame>, FrameError> {
        let asset = self.assets.get(&request.media).ok_or_else(|| {
            FrameError::Unavailable(format!("media {} has no file", request.media))
        })?;
        let t = if request.still {
            Time::ZERO
        } else {
            request.source_time
        };
        let mut offline = self.offline.lock().unwrap_or_else(PoisonError::into_inner);
        if offline.contains(&request.media) {
            return Err(FrameError::Offline(request.media));
        }
        let mut streams = self.streams.lock().unwrap_or_else(PoisonError::into_inner);
        let stream = match streams.entry(request.media) {
            Entry::Occupied(e) => e.into_mut(),
            Entry::Vacant(e) => {
                // Exports use the original media (no proxies) in the decoder's native layout;
                // the GPU converts (arch §7).
                let decoder = self
                    .backend
                    .open_decoder(&asset.path, &export_decode_opts(asset.target))
                    .map_err(|err| {
                        let error = unavailable(request.media, &err);
                        if matches!(error, FrameError::Offline(_)) {
                            offline.insert(request.media);
                        }
                        error
                    })?;
                e.insert(Stream {
                    decoder,
                    current: None,
                    ahead: None,
                    ended: false,
                })
            }
        };
        if request.still
            && let Some((_, frame)) = &stream.current
        {
            return Ok(Arc::clone(frame));
        }
        Self::frame_at(request.media, asset, stream, t)
    }
}

// ----- audio -----------------------------------------------------------------------------------

/// The mixdown's [`SourceFactory`] over a media backend's audio decoders.
pub struct DecoderPcmFactory {
    backend: Arc<dyn MediaBackend>,
    paths: HashMap<MediaId, PathBuf>,
}

impl std::fmt::Debug for DecoderPcmFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecoderPcmFactory")
            .field("media", &self.paths.len())
            .finish_non_exhaustive()
    }
}

impl DecoderPcmFactory {
    /// Sources for the media of `doc`, decoding with `backend`.
    #[must_use]
    pub fn new(backend: Arc<dyn MediaBackend>, doc: &Project) -> DecoderPcmFactory {
        DecoderPcmFactory {
            backend,
            paths: assets(doc)
                .into_iter()
                .map(|(id, a)| (id, a.path))
                .collect(),
        }
    }
}

impl SourceFactory for DecoderPcmFactory {
    fn open(&self, request: &SourceRequest) -> Result<Box<dyn PcmSource>, SourceError> {
        let path = self
            .paths
            .get(&request.media)
            .ok_or(SourceError::UnknownMedia(request.media))?;
        let decoder = self
            .backend
            .open_audio_decoder(path, request.stream)
            .map_err(|e| match e {
                MediaError::NoStream { .. } => SourceError::NoStream {
                    media: request.media,
                    stream: request.stream,
                },
                other => SourceError::Decode(format!("media {}: {other}", request.media)),
            })?;
        let f = decoder.format();
        if f.channels == 0 {
            return Err(SourceError::Format("the stream has no channels".into()));
        }
        Ok(Box::new(DecoderPcm {
            format: PcmFormat::new(f.rate, f.channels),
            decoder,
            pos: 0,
        }))
    }
}

/// One clip's audio: `decode_range` from the current position, interleaved.
struct DecoderPcm {
    decoder: Box<dyn AudioDecoder>,
    format: PcmFormat,
    /// The next frame (sample index per channel) to read.
    pos: u64,
}

impl PcmSource for DecoderPcm {
    fn format(&self) -> PcmFormat {
        self.format
    }

    fn seek(&mut self, frame: u64) -> Result<(), SourceError> {
        self.pos = frame;
        Ok(())
    }

    fn read(&mut self, out: &mut [f32]) -> Result<usize, SourceError> {
        let channels = usize::from(self.format.channels);
        let want = out.len() / channels;
        if want == 0 {
            return Ok(0);
        }
        let rate = self.format.rate;
        let at = |n: u64| {
            i64::try_from(n)
                .ok()
                .and_then(|n| rate.samples_to_time(n).ok())
                .ok_or_else(|| SourceError::Decode(format!("sample {n} is out of range")))
        };
        let start = at(self.pos)?;
        let end = at(self.pos + want as u64)?;
        let length = end
            .duration_since(start)
            .map_err(|e| SourceError::Decode(e.to_string()))?;
        let chunk = self
            .decoder
            .decode_range(start, length)
            .map_err(|e| SourceError::Decode(e.to_string()))?;
        if usize::from(chunk.channels) != channels || chunk.planes.len() != channels {
            return Err(SourceError::Format(format!(
                "the decoder returned {} channels instead of {channels}",
                chunk.channels
            )));
        }
        let n = chunk.frames().min(want);
        for (i, frame) in out[..n * channels].chunks_exact_mut(channels).enumerate() {
            for (sample, plane) in frame.iter_mut().zip(&chunk.planes) {
                *sample = plane[i];
            }
        }
        self.pos += n as u64;
        Ok(n)
    }
}
