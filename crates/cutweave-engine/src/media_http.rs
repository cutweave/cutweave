// SPDX-License-Identifier: GPL-3.0-or-later
//! The engine behind the `/media` HTTP endpoints (WP-1I.03; arch §6, §7, §10): `EngineMedia`
//! implements `cutweave-api`'s [`MediaService`] over [`EngineHandle::media_state`], and
//! [`media_mounts`] mounts it. The routes, auth and caching rules are `cutweave_api::media`'s;
//! this module resolves requests against the open document and produces the bytes.
//!
//! - **Thumbnails and filmstrips** come from the `ThumbCache` (WP-1D.05): the cached file when
//!   the import analysis made it, else decoded now. `t` is floored to the media's frame grid
//!   so scrubbing cannot create unbounded cache entries. ETags are the cache's own
//!   (`<content hash>-<params>`), computed without I/O.
//! - **Waveforms** are read from the `WaveformCache` (WP-1D.06), never computed here: before the
//!   `waveform` analysis job wrote the pyramid the answer is 404 `not_ready`.
//! - **Frames** run `cutweave_render::query::render_frame` on the engine's renderer against the
//!   live document, decoding in software without the frame cache, so the bytes are the ones
//!   `render.frame` returns; the request's cancel token stops the decode.
//! - **Cancellation:** every decode gets a `CancelToken` that the request's [`MediaCancel`]
//!   cancels (waking the decoder at once) when the client disconnects.

use std::fmt::Write as _;
use std::sync::{Arc, PoisonError};

use cutweave_api::Mounts;
use cutweave_api::media::{
    self, FILMSTRIP_HEADER, FilmstripFormat, FilmstripRequest, FrameFormat, FrameRequest,
    MediaBody, MediaCancel, MediaError, MediaOptions, MediaRequest, MediaService, PreparedMedia,
    REV_HEADER, ThumbRequest, TimeParam, WaveformFormat, WaveformRequest, WaveformTrack,
};
use cutweave_cache::{
    CacheError, ContentHash, FilmstripManifest, FilmstripSpec, MediaRef, ThumbCache, Track,
    WaveformCache, WaveformStyle, render_png,
};
use cutweave_commands::{Access, CmdError, DocSnapshot, QueryCtx};
use cutweave_media::CancelToken;
use cutweave_model::{
    FrameRate, MediaId, ProbeInfo, Rounding, SequenceId, StreamKind, Time, TimeInput,
};
use cutweave_render::encode::{DEFAULT_MAX_SIZE, ImageFormat, fit_within};
use cutweave_render::geometry::quarter_turns;
use cutweave_render::query::{MAX_FRAME_SIZE, MIN_MAX_SIZE, RenderFrame, render_frame};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::config::MountFactory;
use crate::engine::EngineHandle;
use crate::full::frames::MediaFrames;
use crate::full::{MediaFile, MediaState};

/// Mounts `/media` over the engine with `options`.
pub fn media_mounts(options: MediaOptions) -> MountFactory {
    Box::new(move |handle: EngineHandle| Mounts {
        media: Some(media::router(Arc::new(EngineMedia { handle }), options)),
        ..Mounts::default()
    })
}

/// The media service of a running engine.
struct EngineMedia {
    handle: EngineHandle,
}

impl MediaService for EngineMedia {
    fn prepare(&self, request: MediaRequest) -> Result<Box<dyn PreparedMedia>, MediaError> {
        let state = self.handle.media_state().ok_or_else(|| {
            MediaError::Unavailable(
                "this engine serves no media (it has no media backend, or it is stopping)".into(),
            )
        })?;
        match request {
            MediaRequest::Thumb(r) => prepare_thumb(&state, &r),
            MediaRequest::Filmstrip(r) => prepare_filmstrip(&state, &r),
            MediaRequest::Waveform(r) => prepare_waveform(&state, r),
            MediaRequest::Frame(r) => prepare_frame(state, &r),
        }
    }
}

// ----- shared helpers ----------------------------------------------------------------------------

fn invalid(param: &str, message: impl Into<String>) -> MediaError {
    MediaError::Invalid {
        param: param.to_owned(),
        message: message.into(),
    }
}

/// A decoder cancel token that the request's cancellation cancels (waking blocked waits).
fn decode_token(cancel: &MediaCancel) -> CancelToken {
    let token = CancelToken::new();
    let linked = token.clone();
    cancel.on_cancel(move || linked.cancel());
    token
}

fn media_failure(e: cutweave_media::MediaError) -> MediaError {
    use cutweave_media::MediaError as M;
    match e {
        M::Cancelled => MediaError::Cancelled,
        M::NoStream { .. } => MediaError::NoStream(e.to_string()),
        M::NotFound { .. } => MediaError::Offline(e.to_string()),
        other => MediaError::Internal(other.to_string()),
    }
}

fn cache_failure(e: CacheError) -> MediaError {
    match e {
        CacheError::InvalidParams(message) => invalid("", message),
        CacheError::Media(m) => media_failure(m),
        other => MediaError::Internal(other.to_string()),
    }
}

/// A media asset of the open document with the cache identity the request names.
struct Resolved {
    file: MediaFile,
    /// The cache key, when the document records the content hash (no I/O needed).
    reference: Option<MediaRef>,
    /// The URL named the content (`hash=` equals the media's content hash).
    immutable: bool,
}

impl Resolved {
    /// The cache key, hashing the file when the document has no hash (blocking).
    fn reference(&self) -> Result<MediaRef, MediaError> {
        match &self.reference {
            Some(r) => Ok(r.clone()),
            None => MediaState::media_ref(&self.file).map_err(cache_failure),
        }
    }
}

fn resolve(state: &MediaState, id: &str, expected: Option<&str>) -> Result<Resolved, MediaError> {
    let not_found = || MediaError::NotFound(format!("no media `{id}` in the open project"));
    let media: MediaId = id.parse().map_err(|_| not_found())?;
    let file = state.media(media).ok_or_else(not_found)?;
    let immutable = match (expected, file.hash) {
        (None, _) => false,
        (Some(want), Some(have)) if have.to_string() == want => true,
        (Some(want), have) => {
            return Err(MediaError::ContentChanged(format!(
                "media `{id}` has content hash {}, not {want}; ask again without `hash` or with \
                 the current one",
                have.map_or_else(|| "(none)".to_owned(), |h| h.to_string())
            )));
        }
    };
    let reference = file
        .hash
        .map(|hash| MediaRef::new(file.path.clone(), hash).with_rotation(file.probe.rotation));
    Ok(Resolved {
        file,
        reference,
        immutable,
    })
}

fn has_stream(probe: &ProbeInfo, kind: StreamKind) -> bool {
    probe.streams.iter().any(|s| s.kind == kind)
}

/// `param` as a time; `f:N` and timecodes need `rate`.
fn resolve_time(
    param: &TimeParam,
    rate: Option<FrameRate>,
    name: &str,
) -> Result<Time, MediaError> {
    let input = match param {
        TimeParam::Flicks(flicks) => TimeInput::Absolute(
            Time::from_flicks(*flicks).map_err(|e| invalid(name, e.to_string()))?,
        ),
        TimeParam::Text(text) => text
            .parse::<TimeInput>()
            .map_err(|e| invalid(name, e.to_string()))?,
    };
    input
        .resolve(rate)
        .map_err(|e| invalid(name, e.to_string()))
}

/// `t` floored to the start of the frame on screen at `rate` (no change without a rate).
fn floor_to_frame(t: Time, rate: Option<FrameRate>) -> Time {
    let Some(rate) = rate else { return t };
    rate.frame_to_time(rate.time_to_frame(t, Rounding::Floor))
        .unwrap_or(t)
}

/// A short hash of `parts` (ETags of the waveform route).
fn params_hash(parts: &str) -> String {
    let digest = Sha256::digest(parts.as_bytes());
    digest[..8]
        .iter()
        .fold(String::with_capacity(16), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

// ----- thumbnails --------------------------------------------------------------------------------

struct PreparedThumb {
    thumbs: Arc<ThumbCache>,
    media: Resolved,
    t: Time,
    w: u32,
    etag: Option<String>,
}

fn thumb_cache(state: &MediaState) -> Result<Arc<ThumbCache>, MediaError> {
    state.thumbs.clone().ok_or_else(|| {
        MediaError::Unavailable(
            "the thumbnail cache is unavailable (no usable cache directory)".into(),
        )
    })
}

fn prepare_thumb(
    state: &MediaState,
    r: &ThumbRequest,
) -> Result<Box<dyn PreparedMedia>, MediaError> {
    let thumbs = thumb_cache(state)?;
    let media = resolve(state, &r.media, r.hash.as_deref())?;
    if !has_stream(&media.file.probe, StreamKind::Video) {
        return Err(MediaError::NoStream(format!(
            "media `{}` has no pictures",
            r.media
        )));
    }
    let rate = media.file.probe.fps;
    let t = floor_to_frame(resolve_time(&r.t, rate, "t")?.max(Time::ZERO), rate);
    let etag = match &media.reference {
        Some(reference) => Some(
            thumbs
                .thumb_etag(reference, t, r.w)
                .map_err(cache_failure)?,
        ),
        None => None,
    };
    Ok(Box::new(PreparedThumb {
        thumbs,
        media,
        t,
        w: r.w,
        etag,
    }))
}

impl PreparedMedia for PreparedThumb {
    fn etag(&self) -> Option<String> {
        self.etag.clone()
    }

    fn immutable(&self) -> bool {
        self.media.immutable
    }

    fn produce(self: Box<Self>, cancel: &MediaCancel) -> Result<MediaBody, MediaError> {
        let reference = self.media.reference()?;
        let token = decode_token(cancel);
        let thumb = self
            .thumbs
            .thumb(&reference, self.t, self.w, &token)
            .map_err(cache_failure)?;
        Ok(MediaBody {
            bytes: thumb.jpeg,
            content_type: "image/jpeg",
            headers: Vec::new(),
            etag: Some(thumb.etag),
        })
    }
}

// ----- filmstrips --------------------------------------------------------------------------------

struct PreparedFilmstrip {
    thumbs: Arc<ThumbCache>,
    media: Resolved,
    spec: FilmstripSpec,
    format: FilmstripFormat,
    etag: Option<String>,
}

/// The tile width: `w`, narrowed so the tile is at most `h` high (the aspect of the picture
/// turned by the probe's display rotation, as the tiles are drawn).
fn tile_width(probe: &ProbeInfo, w: u32, h: Option<u32>) -> u32 {
    let Some(h) = h else { return w };
    let size = probe
        .streams
        .iter()
        .find(|s| s.kind == StreamKind::Video)
        .and_then(|s| Some((s.width?, s.height?)))
        .map(|(vw, vh)| {
            if quarter_turns(probe.rotation) % 180 == 90 {
                (vh, vw)
            } else {
                (vw, vh)
            }
        });
    match size {
        Some((vw, vh)) if vw > 0 && vh > 0 => {
            let fit = u64::from(h) * u64::from(vw) / u64::from(vh);
            u32::try_from(fit).unwrap_or(w).clamp(1, w)
        }
        _ => w,
    }
}

fn manifest_json(m: &FilmstripManifest) -> String {
    let (sprite_w, sprite_h) = m.sprite_size();
    json!({
        "tile_w": m.tile_w,
        "tile_h": m.tile_h,
        "count": m.count,
        "columns": m.columns,
        "rows": m.rows(),
        "sprite_w": sprite_w,
        "sprite_h": sprite_h,
        "start": m.start.flicks(),
        "interval": m.interval.flicks(),
    })
    .to_string()
}

fn prepare_filmstrip(
    state: &MediaState,
    r: &FilmstripRequest,
) -> Result<Box<dyn PreparedMedia>, MediaError> {
    let thumbs = thumb_cache(state)?;
    let media = resolve(state, &r.media, r.hash.as_deref())?;
    if !has_stream(&media.file.probe, StreamKind::Video) {
        return Err(MediaError::NoStream(format!(
            "media `{}` has no pictures",
            r.media
        )));
    }
    let spec = FilmstripSpec {
        count: r.count,
        tile_w: tile_width(&media.file.probe, r.w, r.h),
    };
    let etag = match &media.reference {
        Some(reference) => {
            let tag = thumbs
                .filmstrip_etag(reference, spec)
                .map_err(cache_failure)?;
            Some(match r.format {
                FilmstripFormat::Jpeg => tag,
                FilmstripFormat::Manifest => format!("{tag}-json"),
            })
        }
        None => None,
    };
    Ok(Box::new(PreparedFilmstrip {
        thumbs,
        media,
        spec,
        format: r.format,
        etag,
    }))
}

impl PreparedMedia for PreparedFilmstrip {
    fn etag(&self) -> Option<String> {
        self.etag.clone()
    }

    fn immutable(&self) -> bool {
        self.media.immutable
    }

    fn produce(self: Box<Self>, cancel: &MediaCancel) -> Result<MediaBody, MediaError> {
        let reference = self.media.reference()?;
        let token = decode_token(cancel);
        let strip = self
            .thumbs
            .filmstrip(&reference, self.spec, &token)
            .map_err(cache_failure)?;
        let manifest = manifest_json(&strip.manifest);
        Ok(match self.format {
            FilmstripFormat::Jpeg => MediaBody {
                bytes: strip.jpeg,
                content_type: "image/jpeg",
                headers: vec![(FILMSTRIP_HEADER, manifest)],
                etag: Some(strip.etag),
            },
            FilmstripFormat::Manifest => MediaBody {
                bytes: manifest.into_bytes(),
                content_type: "application/json",
                headers: Vec::new(),
                etag: Some(format!("{}-json", strip.etag)),
            },
        })
    }
}

// ----- waveforms ---------------------------------------------------------------------------------

struct PreparedWaveform {
    waveforms: Arc<WaveformCache>,
    media: Resolved,
    id: String,
    request: WaveformRequest,
    range: std::ops::Range<Time>,
    /// What the ETag hashes besides the content hash.
    params: String,
    etag: Option<String>,
}

fn prepare_waveform(
    state: &MediaState,
    r: WaveformRequest,
) -> Result<Box<dyn PreparedMedia>, MediaError> {
    let waveforms = state
        .waveforms
        .clone()
        .ok_or_else(|| MediaError::Unavailable("the waveform cache is unavailable".into()))?;
    let media = resolve(state, &r.media, r.hash.as_deref())?;
    let probe = &media.file.probe;
    let audio = match r.stream {
        None => has_stream(probe, StreamKind::Audio),
        Some(index) => probe
            .streams
            .iter()
            .any(|s| s.index == index && s.kind == StreamKind::Audio),
    };
    if !audio {
        return Err(MediaError::NoStream(format!(
            "media `{}` has no sound{}",
            r.media,
            r.stream
                .map_or_else(String::new, |s| format!(" in stream {s}"))
        )));
    }
    let rate = probe.fps;
    let start = resolve_time(&r.start, rate, "start")?.max(Time::ZERO);
    let end = match &r.end {
        Some(end) => resolve_time(end, rate, "end")?,
        None => Time::MAX,
    };
    if end <= start {
        return Err(invalid("end", "must be after `start`"));
    }
    let track = match r.track {
        WaveformTrack::Mix => "mix".to_owned(),
        WaveformTrack::Channel(c) => format!("ch{c}"),
    };
    let format = match r.format {
        WaveformFormat::Peaks => "peaks".to_owned(),
        WaveformFormat::Png { width, height } => format!("png{width}x{height}"),
    };
    let stream = r
        .stream
        .map_or_else(|| "main".to_owned(), |s| s.to_string());
    let params = format!(
        "waveform/v1/{stream}/lod{}/{}/{}/{track}/{format}",
        r.lod,
        start.flicks(),
        end.flicks()
    );
    let etag = media
        .reference
        .as_ref()
        .map(|reference| format!("{}-{}", reference.hash, params_hash(&params)));
    Ok(Box::new(PreparedWaveform {
        waveforms,
        media,
        id: r.media.clone(),
        request: r,
        range: start..end,
        params,
        etag,
    }))
}

impl PreparedMedia for PreparedWaveform {
    fn etag(&self) -> Option<String> {
        self.etag.clone()
    }

    fn immutable(&self) -> bool {
        self.media.immutable
    }

    fn produce(self: Box<Self>, cancel: &MediaCancel) -> Result<MediaBody, MediaError> {
        let hash: ContentHash = self.media.reference()?.hash;
        let r = &self.request;
        let track = match r.track {
            WaveformTrack::Mix => Track::Mix,
            WaveformTrack::Channel(c) => Track::Channel(c),
        };
        let per_channel = matches!(track, Track::Channel(_));
        let Some(waveform) = self
            .waveforms
            .get(&hash, r.stream, per_channel)
            .map_err(cache_failure)?
        else {
            return Err(MediaError::NotReady(format!(
                "the waveform of media `{}` is not computed yet (its `waveform` analysis job \
                 writes it)",
                self.id
            )));
        };
        if r.lod >= waveform.levels() {
            return Err(invalid(
                "lod",
                format!(
                    "this waveform has {} levels (0..={})",
                    waveform.levels(),
                    waveform.levels().saturating_sub(1)
                ),
            ));
        }
        let Some(peaks) = waveform.peaks(r.lod, self.range.clone(), track) else {
            return Err(invalid(
                "track",
                format!(
                    "no such track: the source has {} channel(s)",
                    waveform.channels()
                ),
            ));
        };
        if cancel.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        let etag = format!("{hash}-{}", params_hash(&self.params));
        Ok(match r.format {
            WaveformFormat::Peaks => MediaBody {
                bytes: peaks.to_bytes(),
                content_type: "application/octet-stream",
                headers: Vec::new(),
                etag: Some(etag),
            },
            WaveformFormat::Png { width, height } => MediaBody {
                bytes: render_png(&peaks, width, height, &WaveformStyle::default())
                    .map_err(cache_failure)?,
                content_type: "image/png",
                headers: Vec::new(),
                etag: Some(etag),
            },
        })
    }
}

// ----- frames ------------------------------------------------------------------------------------

struct PreparedFrame {
    state: MediaState,
    snapshot: Arc<DocSnapshot>,
    params: RenderFrame,
}

/// The `render.frame` `max_size` whose image fits the `w × h` box (either may be absent),
/// keeping the sequence's aspect; 768 (the query's default) without a box.
fn max_size_for((sw, sh): (u32, u32), w: Option<u32>, h: Option<u32>) -> u32 {
    let long = u64::from(sw.max(sh).max(1));
    let by =
        |limit: Option<u32>, side: u32| limit.map(|l| u64::from(l) * long / u64::from(side.max(1)));
    let bound = match (by(w, sw), by(h, sh)) {
        (None, None) => return DEFAULT_MAX_SIZE,
        (Some(a), None) | (None, Some(a)) => a,
        (Some(a), Some(b)) => a.min(b),
    };
    let mut size = u32::try_from(bound)
        .unwrap_or(MAX_FRAME_SIZE)
        .clamp(MIN_MAX_SIZE, MAX_FRAME_SIZE);
    // `fit_within` rounds to the nearest pixel: step down until the image fits the box.
    let fits = |size: u32| {
        let (iw, ih) = fit_within((sw, sh), size);
        w.is_none_or(|w| iw <= w) && h.is_none_or(|h| ih <= h)
    };
    while size > MIN_MAX_SIZE && !fits(size) {
        size -= 1;
    }
    size
}

fn prepare_frame(
    state: MediaState,
    r: &FrameRequest,
) -> Result<Box<dyn PreparedMedia>, MediaError> {
    let snapshot = state
        .snapshot()
        .ok_or_else(|| MediaError::Unavailable("no document is open".into()))?;
    let doc = &snapshot.doc;
    let (sequence, seq) = if let Some(raw) = &r.sequence {
        let not_found = || MediaError::NotFound(format!("no sequence `{raw}` in the open project"));
        let id: SequenceId = raw.parse().map_err(|_| not_found())?;
        (id, doc.sequences.get(&id).ok_or_else(not_found)?)
    } else {
        let mut all = doc.sequences.iter();
        match (all.next(), all.next()) {
            (Some((id, seq)), None) => (*id, seq),
            (None, _) => return Err(MediaError::NotFound("the project has no sequence".into())),
            (Some(_), Some(_)) => {
                return Err(invalid(
                    "seq",
                    format!(
                        "the project has {} sequences; name one",
                        doc.sequences.len()
                    ),
                ));
            }
        }
    };
    let rate = seq.format.frame_rate;
    let t = resolve_time(&r.t, Some(rate), "t")?;
    let max_size = max_size_for((seq.format.width, seq.format.height), r.w, r.h);
    let params = RenderFrame {
        sequence: Some(sequence),
        t: TimeInput::Absolute(t),
        max_size: Some(max_size),
        format: Some(match r.format {
            FrameFormat::Jpeg => ImageFormat::Jpeg,
            FrameFormat::Png => ImageFormat::Png,
        }),
        quality: r.quality,
        limit: None,
    };
    Ok(Box::new(PreparedFrame {
        state,
        snapshot,
        params,
    }))
}

fn frame_failure(e: CmdError) -> MediaError {
    match e {
        CmdError::NotFound { .. } => MediaError::NotFound(e.to_string()),
        CmdError::Invalid { .. } => invalid("", e.to_string()),
        other => MediaError::Internal(other.to_string()),
    }
}

impl PreparedMedia for PreparedFrame {
    fn etag(&self) -> Option<String> {
        None
    }

    fn immutable(&self) -> bool {
        false
    }

    fn produce(self: Box<Self>, cancel: &MediaCancel) -> Result<MediaBody, MediaError> {
        let token = decode_token(cancel);
        let ctx = QueryCtx::new(&self.snapshot.doc, Access::STANDARD).with_rev(self.snapshot.rev);
        let renderer = Arc::clone(&self.state.renderer);
        let frame = MediaFrames::agent_scoped(token, || {
            let mut backend = renderer.lock().unwrap_or_else(PoisonError::into_inner);
            render_frame(&mut *backend, &ctx, &self.params)
        })
        .map_err(frame_failure)?;
        if cancel.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        let bytes = decode_base64(&frame.data)
            .ok_or_else(|| MediaError::Internal("render.frame returned bad base64".into()))?;
        let content_type = if frame.mime == ImageFormat::Png.mime() {
            "image/png"
        } else {
            "image/jpeg"
        };
        Ok(MediaBody {
            bytes,
            content_type,
            headers: vec![(REV_HEADER, self.snapshot.rev.to_string())],
            etag: None,
        })
    }
}

/// Decodes standard base64 with padding (`render.frame`'s `data`).
fn decode_base64(text: &str) -> Option<Vec<u8>> {
    fn value(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some(u32::from(c - b'A')),
            b'a'..=b'z' => Some(u32::from(c - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(c - b'0') + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks(4) {
        let pad = chunk.iter().rev().take_while(|&&c| c == b'=').count();
        if pad > 2 {
            return None;
        }
        let mut n = 0u32;
        for &c in &chunk[..4 - pad] {
            n = (n << 6) | value(c)?;
        }
        n <<= 6 * pad;
        let [_, a, b, c] = n.to_be_bytes();
        out.extend_from_slice(&[a, b, c][..3 - pad]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn base64_round_trips_render_frames_encoding() {
        for len in 0..40usize {
            let data: Vec<u8> = (0..len)
                .map(|i| u8::try_from(i * 37 % 256).unwrap())
                .collect();
            let text = cutweave_render::encode::base64(&data);
            assert_eq!(decode_base64(&text), Some(data), "{len}");
        }
        assert_eq!(decode_base64("abc"), None);
        assert_eq!(decode_base64("a==="), None);
        assert_eq!(decode_base64("ab!d"), None);
    }

    #[test]
    fn the_frame_box_maps_to_render_frames_max_size() {
        let hd = (1920, 1080);
        assert_eq!(max_size_for(hd, None, None), DEFAULT_MAX_SIZE);
        assert_eq!(max_size_for(hd, Some(640), None), 640);
        assert_eq!(max_size_for(hd, Some(640), Some(360)), 640);
        // A box taller than the picture: the width decides.
        assert_eq!(max_size_for(hd, Some(640), Some(640)), 640);
        // A wide box: the height decides.
        assert_eq!(max_size_for(hd, Some(4000), Some(360)), 640);
        assert_eq!(
            fit_within(hd, max_size_for(hd, Some(641), Some(361))),
            (641, 361)
        );
        // Portrait.
        let portrait = (1080, 1920);
        assert_eq!(
            fit_within(portrait, max_size_for(portrait, Some(360), None)),
            (360, 640)
        );
        // Never below the query's minimum.
        assert_eq!(max_size_for(hd, Some(2), None), MIN_MAX_SIZE);
        for w in [17, 100, 333, 999, 1919, 4096] {
            for h in [17, 100, 333, 999, 1080, 4096] {
                let (iw, ih) = fit_within(hd, max_size_for(hd, Some(w), Some(h)));
                assert!(
                    iw <= w.max(MIN_MAX_SIZE) && ih <= h.max(MIN_MAX_SIZE),
                    "{w}x{h}"
                );
            }
        }
    }

    #[test]
    fn tiles_narrow_to_fit_a_height() {
        let stream = serde_json::from_value(json!({
            "index": 0, "kind": "video", "codec": null, "profile": null, "width": 1920,
            "height": 1080, "pixel_format": null, "sample_rate": null, "channels": null,
            "language": null,
        }))
        .unwrap();
        let probe = ProbeInfo {
            streams: vec![stream],
            ..ProbeInfo::default()
        };
        assert_eq!(tile_width(&probe, 160, None), 160);
        assert_eq!(tile_width(&probe, 160, Some(45)), 80);
        assert_eq!(tile_width(&probe, 160, Some(4096)), 160);
        // A quarter turn (a phone clip shot upright): the tiles are 1080 × 1920 pictures.
        for rotation in [90, 270] {
            let turned = ProbeInfo {
                rotation,
                ..probe.clone()
            };
            assert_eq!(tile_width(&turned, 160, None), 160);
            assert_eq!(tile_width(&turned, 160, Some(160)), 90);
            assert_eq!(tile_width(&turned, 160, Some(4096)), 160);
        }
        let upside_down = ProbeInfo {
            rotation: 180,
            ..probe.clone()
        };
        assert_eq!(tile_width(&upside_down, 160, Some(45)), 80);
    }
}
