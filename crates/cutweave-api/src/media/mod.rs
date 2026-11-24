// SPDX-License-Identifier: GPL-3.0-or-later
//! The media HTTP endpoints under `/media` (WP-1I.03; arch §6, §7, §10, §18). The reference is
//! `docs/reference/http-media.md`.
//!
//! | Route | Answer |
//! |---|---|
//! | `GET /media/{id}/thumb?t=&w=&hash=` | JPEG of the frame at media time `t`, `w` px wide at most |
//! | `GET /media/{id}/filmstrip?w=&h=&count=&hash=&fmt=` | JPEG sprite (manifest in `x-cutweave-filmstrip`), or the manifest as JSON |
//! | `GET /media/{id}/waveform?lod=&start=&end=&track=&stream=&hash=&fmt=&w=&h=` | binary peak data (`CWPD`) or a PNG |
//! | `GET /media/frame?seq=&t=&w=&h=&fmt=&q=` | the sequence frame, the bytes `render.frame` returns |
//! | `POST /media/sign` | signed, expiring URLs for `<img src>` ([`sign`]) |
//!
//! # Request flow
//!
//! The server authenticates first (a bearer token, or a valid signature for `GET`). Then:
//!
//! 1. **Parse** the query; bounds and unknown parameters are 422 with a JSON error.
//! 2. **Prepare** ([`MediaService::prepare`]): the engine resolves the media or sequence against
//!    the open document (404, 409, 422) and computes the ETag from the request alone. No I/O and
//!    no decoding, so it runs on the async runtime.
//! 3. **`If-None-Match`** matching that ETag answers 304 at once: no decode, no cache access.
//! 4. **Produce** ([`PreparedMedia::produce`]) on the blocking pool behind a semaphore
//!    ([`MediaOptions::concurrency`]). When the client disconnects, the request future is
//!    dropped and its [`MediaCancel`] fires, which cancels the decode through the engine's
//!    `CancelToken`.
//!
//! # Caching
//!
//! Responses are a pure function of the media's content and the parameters. When the URL names
//! the content (`hash=` equal to the media's content hash) the answer is
//! `Cache-Control: private, max-age=31536000, immutable`; without it the same media id may later
//! point at other content (a relink), so it is `private, no-cache` (revalidate: a 304 costs
//! nothing). Frames depend on the whole document and are `no-store`.
//!
//! The engine implements [`MediaService`] (`cutweave-engine` depends on this crate, so the
//! contract is a trait here) and mounts [`router`] through `Mounts::media`.

mod params;
pub mod sign;

use std::sync::{
    Arc, Mutex, PoisonError,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::{
    Extension, Json, Router,
    extract::{Path, RawQuery, State, rejection::JsonRejection},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::Semaphore;

pub use params::{
    DEFAULT_THUMB_WIDTH, DEFAULT_TILE_COUNT, DEFAULT_TILE_WIDTH, DEFAULT_WAVEFORM_PNG,
};
pub use sign::{SignError, UrlSigner};

use crate::server::{Authenticated, RequestId};

/// Largest width or height of any image the routes produce (`w`, `h`).
pub const MAX_DIMENSION: u32 = 4096;
/// Most tiles in one filmstrip.
pub const MAX_FILMSTRIP_TILES: u32 = 1000;
/// Highest waveform level (a pyramid has at most 12, `0..=11`).
pub const MAX_WAVEFORM_LOD: u8 = 11;
/// Requests producing at once by default (the rest wait for a slot).
pub const DEFAULT_CONCURRENCY: usize = 4;
/// Lifetime of a signed URL when the client names none.
pub const DEFAULT_SIGN_TTL: Duration = Duration::from_hours(1);
/// Longest lifetime of a signed URL.
pub const MAX_SIGN_TTL: Duration = Duration::from_hours(24);
/// Most URLs one `POST /media/sign` signs.
pub const MAX_SIGN_BATCH: usize = 256;
/// `Cache-Control` of a response whose URL names the content.
pub const CACHE_IMMUTABLE: &str = "private, max-age=31536000, immutable";
/// `Cache-Control` of a response keyed by content the URL does not name.
pub const CACHE_REVALIDATE: &str = "private, no-cache";
/// Header carrying a filmstrip's manifest (JSON) next to its sprite.
pub const FILMSTRIP_HEADER: &str = "x-cutweave-filmstrip";
/// Header carrying the document revision a frame shows.
pub const REV_HEADER: &str = "x-cutweave-rev";

/// A time parameter as given: integer flicks, or a text form of `TimeInput` (`2.5s`, `f:60`, a
/// timecode), which the engine resolves against the media's or sequence's frame rate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TimeParam {
    /// Flicks.
    Flicks(i64),
    /// Seconds, a frame number or a timecode, as text.
    Text(String),
}

/// `GET /media/{id}/thumb`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThumbRequest {
    /// The media id.
    pub media: String,
    /// Media time of the frame (floored to the media's frame grid by the engine).
    pub t: TimeParam,
    /// Largest width in px (never upscaled).
    pub w: u32,
    /// The content hash the client expects (`hash=`): makes the answer immutable.
    pub hash: Option<String>,
}

/// What `GET /media/{id}/filmstrip` returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilmstripFormat {
    /// The JPEG sprite (manifest in the `x-cutweave-filmstrip` header).
    Jpeg,
    /// The manifest alone, as JSON (`fmt=json`).
    Manifest,
}

/// `GET /media/{id}/filmstrip`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilmstripRequest {
    /// The media id.
    pub media: String,
    /// Tile width in px.
    pub w: u32,
    /// Largest tile height in px: the tile narrows to keep the picture's aspect.
    pub h: Option<u32>,
    /// Number of tiles.
    pub count: u32,
    /// The expected content hash.
    pub hash: Option<String>,
    /// Sprite or manifest.
    pub format: FilmstripFormat,
}

/// Which signal a waveform shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaveformTrack {
    /// The mono mix (`track=mix`, the default).
    Mix,
    /// One source channel, 0-based (`track=1`).
    Channel(u16),
}

/// What `GET /media/{id}/waveform` returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaveformFormat {
    /// The binary peak data of docs/reference/waveform-format.md (`fmt=peaks`, the default).
    Peaks,
    /// The peaks drawn into a PNG of this size.
    Png {
        /// Width in px.
        width: u32,
        /// Height in px.
        height: u32,
    },
}

/// `GET /media/{id}/waveform`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WaveformRequest {
    /// The media id.
    pub media: String,
    /// The pyramid level (bins of `256 × 4^lod` samples).
    pub lod: u8,
    /// Range start (media time).
    pub start: TimeParam,
    /// Range end (media time); `None`: the end of the audio.
    pub end: Option<TimeParam>,
    /// The signal.
    pub track: WaveformTrack,
    /// The audio stream (container index); `None`: the first.
    pub stream: Option<u32>,
    /// The expected content hash.
    pub hash: Option<String>,
    /// Peak data or PNG.
    pub format: WaveformFormat,
}

/// The encoding of a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameFormat {
    /// JPEG (the default).
    Jpeg,
    /// PNG.
    Png,
}

/// `GET /media/frame`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameRequest {
    /// The sequence id; `None`: the project's only sequence.
    pub sequence: Option<String>,
    /// Timeline time.
    pub t: TimeParam,
    /// Largest width; the frame keeps the sequence's aspect inside `w × h`.
    pub w: Option<u32>,
    /// Largest height.
    pub h: Option<u32>,
    /// JPEG or PNG.
    pub format: FrameFormat,
    /// JPEG quality 1-100 (`q`; default 80, as `render.frame`).
    pub quality: Option<u8>,
}

/// A parsed, bounds-checked media request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MediaRequest {
    /// A thumbnail.
    Thumb(ThumbRequest),
    /// A filmstrip.
    Filmstrip(FilmstripRequest),
    /// A waveform.
    Waveform(WaveformRequest),
    /// A sequence frame.
    Frame(FrameRequest),
}

/// Why a media request failed. Each maps to an HTTP status and a JSON error code.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MediaError {
    /// Unknown media or sequence (404 `not_found`).
    #[error("{0}")]
    NotFound(String),
    /// The media has no stream of the kind asked for: a thumbnail of sound, a waveform of a
    /// silent file (404 `no_stream`).
    #[error("{0}")]
    NoStream(String),
    /// The data is not computed yet (a waveform before its analysis finished; 404 `not_ready`).
    #[error("{0}")]
    NotReady(String),
    /// The media file is not where the document says (404 `media_offline`).
    #[error("{0}")]
    Offline(String),
    /// A parameter is out of bounds or does not fit the media (422 `invalid_params`).
    #[error("{param}: {message}")]
    Invalid {
        /// The query parameter (empty for the request as a whole).
        param: String,
        /// What is wrong.
        message: String,
    },
    /// `hash=` names content the media no longer has (409 `content_changed`).
    #[error("{0}")]
    ContentChanged(String),
    /// The engine serves no media: no backend, or it is stopping (503 `unavailable`).
    #[error("{0}")]
    Unavailable(String),
    /// The client went away; nothing is sent.
    #[error("cancelled")]
    Cancelled,
    /// Decoding or encoding failed (500 `media_error`).
    #[error("{0}")]
    Internal(String),
}

impl MediaError {
    fn status_and_code(&self) -> (StatusCode, &'static str) {
        match self {
            MediaError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
            MediaError::NoStream(_) => (StatusCode::NOT_FOUND, "no_stream"),
            MediaError::NotReady(_) => (StatusCode::NOT_FOUND, "not_ready"),
            MediaError::Offline(_) => (StatusCode::NOT_FOUND, "media_offline"),
            MediaError::Invalid { .. } => (StatusCode::UNPROCESSABLE_ENTITY, "invalid_params"),
            MediaError::ContentChanged(_) => (StatusCode::CONFLICT, "content_changed"),
            MediaError::Unavailable(_) => (StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
            // Never sent (the client is gone); 499 is nginx's "client closed request".
            MediaError::Cancelled => (
                StatusCode::from_u16(499).unwrap_or(StatusCode::BAD_REQUEST),
                "cancelled",
            ),
            MediaError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "media_error"),
        }
    }

    fn into_response(self, request_id: Option<&str>) -> Response {
        let (status, code) = self.status_and_code();
        let mut error = json!({ "code": code, "message": self.to_string() });
        if let MediaError::Invalid { param, message } = &self {
            error["param"] = json!(param);
            error["message"] = json!(message);
        }
        let body = json!({ "error": error, "request_id": request_id });
        let mut response = (status, Json(body)).into_response();
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        response
    }
}

/// The body of a produced response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaBody {
    /// The encoded bytes.
    pub bytes: Vec<u8>,
    /// `image/jpeg`, `image/png`, `application/json` or `application/octet-stream`.
    pub content_type: &'static str,
    /// Extra headers (`x-cutweave-filmstrip`, `x-cutweave-rev`).
    pub headers: Vec<(&'static str, String)>,
    /// The ETag, when [`PreparedMedia::etag`] could not know it (unquoted).
    pub etag: Option<String>,
}

/// A request the engine resolved (see the module docs).
pub trait PreparedMedia: Send + 'static {
    /// The response's entity tag (unquoted), computed from the request alone; `None` when it
    /// cannot be known without I/O or when the answer is not cacheable.
    fn etag(&self) -> Option<String>;

    /// Whether the URL names the content, so the answer can be cached as immutable.
    fn immutable(&self) -> bool;

    /// Produces the body. Blocking (it decodes and reads the caches): runs on the blocking
    /// pool. Returns [`MediaError::Cancelled`] soon after `cancel` fires.
    fn produce(self: Box<Self>, cancel: &MediaCancel) -> Result<MediaBody, MediaError>;
}

/// What the media routes serve from (the engine implements it).
pub trait MediaService: Send + Sync + 'static {
    /// Resolves `request` against the open document without I/O or decoding: unknown media or
    /// sequence, parameters that do not fit the media, and the response's cache identity.
    /// Runs on the async runtime, so it must not block.
    fn prepare(&self, request: MediaRequest) -> Result<Box<dyn PreparedMedia>, MediaError>;
}

type Hook = Box<dyn FnOnce() + Send>;

#[derive(Default)]
struct CancelInner {
    cancelled: AtomicBool,
    hooks: Mutex<Vec<Hook>>,
}

/// Cancellation of one media request: fires when the client disconnects. The engine bridges it
/// to its decoders' `CancelToken` with [`on_cancel`](Self::on_cancel). Clones share it.
#[derive(Clone, Default)]
pub struct MediaCancel(Arc<CancelInner>);

impl std::fmt::Debug for MediaCancel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaCancel")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

impl MediaCancel {
    /// A request that is not cancelled.
    #[must_use]
    pub fn new() -> MediaCancel {
        MediaCancel::default()
    }

    /// Whether the client went away.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Acquire)
    }

    /// Runs `hook` when the request is cancelled (at once if it already is). Hooks run on the
    /// thread that cancels; they must be quick and must not block.
    pub fn on_cancel(&self, hook: impl FnOnce() + Send + 'static) {
        let mut hooks = self.0.hooks.lock().unwrap_or_else(PoisonError::into_inner);
        if self.is_cancelled() {
            drop(hooks);
            hook();
        } else {
            hooks.push(Box::new(hook));
        }
    }

    /// Cancels the request and runs the hooks. Idempotent.
    pub fn cancel(&self) {
        let hooks = {
            let mut hooks = self.0.hooks.lock().unwrap_or_else(PoisonError::into_inner);
            if self.0.cancelled.swap(true, Ordering::AcqRel) {
                return;
            }
            std::mem::take(&mut *hooks)
        };
        for hook in hooks {
            hook();
        }
    }
}

/// Counters of the media routes (tests, diagnostics).
#[derive(Debug, Default)]
pub struct MediaStats {
    produced: AtomicU64,
    not_modified: AtomicU64,
    cancelled: AtomicU64,
    in_flight: AtomicU64,
}

/// A reading of [`MediaStats`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MediaStatsSnapshot {
    /// Requests that reached [`PreparedMedia::produce`] (the only path that decodes or reads a
    /// cache).
    pub produced: u64,
    /// Requests answered 304 from the ETag alone.
    pub not_modified: u64,
    /// Requests whose client went away while they produced.
    pub cancelled: u64,
    /// Requests producing right now.
    pub in_flight: u64,
}

impl MediaStats {
    /// The current counts.
    #[must_use]
    pub fn snapshot(&self) -> MediaStatsSnapshot {
        MediaStatsSnapshot {
            produced: self.produced.load(Ordering::Acquire),
            not_modified: self.not_modified.load(Ordering::Acquire),
            cancelled: self.cancelled.load(Ordering::Acquire),
            in_flight: self.in_flight.load(Ordering::Acquire),
        }
    }
}

/// Options of the `/media` mount.
#[derive(Clone, Debug)]
pub struct MediaOptions {
    /// Requests producing at once (decodes, cache reads, renders); the rest wait.
    pub concurrency: usize,
    /// The counters (pass your own to read them in tests).
    pub stats: Arc<MediaStats>,
}

impl Default for MediaOptions {
    fn default() -> MediaOptions {
        MediaOptions {
            concurrency: DEFAULT_CONCURRENCY,
            stats: Arc::default(),
        }
    }
}

#[derive(Clone)]
struct App {
    service: Arc<dyn MediaService>,
    permits: Arc<Semaphore>,
    stats: Arc<MediaStats>,
}

/// The `/media` mount over `service`. The server authenticates every request before it
/// reaches these routes.
pub fn router(service: Arc<dyn MediaService>, options: MediaOptions) -> Router {
    let app = App {
        service,
        permits: Arc::new(Semaphore::new(options.concurrency.max(1))),
        stats: options.stats,
    };
    Router::new()
        .route("/frame", get(frame))
        .route("/sign", post(sign_urls))
        .route("/{id}/thumb", get(thumb))
        .route("/{id}/filmstrip", get(filmstrip))
        .route("/{id}/waveform", get(waveform))
        .with_state(app)
}

fn rid(id: Option<&Extension<RequestId>>) -> Option<String> {
    id.map(|Extension(r)| r.0.clone())
}

async fn thumb(
    State(app): State<App>,
    Path(id): Path<String>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    request_id: Option<Extension<RequestId>>,
) -> Response {
    let parsed = params::thumb(id, query.as_deref()).map(MediaRequest::Thumb);
    serve(app, &headers, rid(request_id.as_ref()), parsed).await
}

async fn filmstrip(
    State(app): State<App>,
    Path(id): Path<String>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    request_id: Option<Extension<RequestId>>,
) -> Response {
    let parsed = params::filmstrip(id, query.as_deref()).map(MediaRequest::Filmstrip);
    serve(app, &headers, rid(request_id.as_ref()), parsed).await
}

async fn waveform(
    State(app): State<App>,
    Path(id): Path<String>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    request_id: Option<Extension<RequestId>>,
) -> Response {
    let parsed = params::waveform(id, query.as_deref()).map(MediaRequest::Waveform);
    serve(app, &headers, rid(request_id.as_ref()), parsed).await
}

async fn frame(
    State(app): State<App>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    request_id: Option<Extension<RequestId>>,
) -> Response {
    let parsed = params::frame(query.as_deref()).map(MediaRequest::Frame);
    serve(app, &headers, rid(request_id.as_ref()), parsed).await
}

/// Whether `If-None-Match` (a list of tags, possibly weak, or `*`) matches `etag`.
fn none_match(headers: &HeaderMap, etag: &str) -> bool {
    let quoted = format!("\"{etag}\"");
    headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(str::trim)
        .any(|tag| tag == "*" || tag.strip_prefix("W/").unwrap_or(tag) == quoted)
}

/// Sets `ETag` and `Cache-Control` on a 200 or 304.
fn cache_headers(headers: &mut HeaderMap, etag: Option<&str>, immutable: bool) {
    let policy = match (etag, immutable) {
        (Some(_), true) => CACHE_IMMUTABLE,
        (Some(_), false) => CACHE_REVALIDATE,
        (None, _) => "no-store",
    };
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(policy));
    if let Some(value) = etag.and_then(|e| HeaderValue::from_str(&format!("\"{e}\"")).ok()) {
        headers.insert(header::ETAG, value);
    }
}

/// Cancels the request when the handler future is dropped before it finished (the client
/// disconnected).
struct CancelOnDrop(Option<MediaCancel>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(cancel) = self.0.take() {
            cancel.cancel();
        }
    }
}

/// Counts a request as in flight while it lives.
struct InFlight(Arc<MediaStats>);

impl InFlight {
    fn start(stats: Arc<MediaStats>) -> InFlight {
        stats.in_flight.fetch_add(1, Ordering::AcqRel);
        stats.produced.fetch_add(1, Ordering::AcqRel);
        InFlight(stats)
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.in_flight.fetch_sub(1, Ordering::AcqRel);
    }
}

async fn serve(
    app: App,
    headers: &HeaderMap,
    request_id: Option<String>,
    parsed: Result<MediaRequest, MediaError>,
) -> Response {
    let rid = request_id.as_deref();
    let prepared = match parsed.and_then(|request| app.service.prepare(request)) {
        Ok(prepared) => prepared,
        Err(e) => return e.into_response(rid),
    };
    let etag = prepared.etag();
    let immutable = prepared.immutable();
    // ETag first: no decode and no cache access for a client that has the bytes.
    if let Some(tag) = &etag
        && none_match(headers, tag)
    {
        app.stats.not_modified.fetch_add(1, Ordering::AcqRel);
        let mut response = StatusCode::NOT_MODIFIED.into_response();
        cache_headers(response.headers_mut(), Some(tag), immutable);
        return response;
    }
    // A disconnect while waiting for a slot drops this future: nothing was started.
    let Ok(permit) = Arc::clone(&app.permits).acquire_owned().await else {
        return MediaError::Unavailable("the media routes are shutting down".into())
            .into_response(rid);
    };
    let cancel = MediaCancel::new();
    let mut guard = CancelOnDrop(Some(cancel.clone()));
    let stats = Arc::clone(&app.stats);
    let task = tokio::task::spawn_blocking(move || {
        // The slot is held until the work really ends, even after a disconnect.
        let _permit = permit;
        let _in_flight = InFlight::start(Arc::clone(&stats));
        let result = prepared.produce(&cancel);
        if cancel.is_cancelled() {
            stats.cancelled.fetch_add(1, Ordering::AcqRel);
        }
        result
    });
    let result = task.await;
    guard.0 = None;
    let body = match result {
        Ok(Ok(body)) => body,
        Ok(Err(e)) => return e.into_response(rid),
        Err(e) => {
            tracing::error!(error = %e, "a media request panicked");
            return MediaError::Internal("the request failed inside the engine".into())
                .into_response(rid);
        }
    };
    let etag = etag.or(body.etag);
    let mut response = body.bytes.into_response();
    let out = response.headers_mut();
    out.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(body.content_type),
    );
    out.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    cache_headers(out, etag.as_deref(), immutable);
    for (name, value) in body.headers {
        if let Ok(value) = HeaderValue::from_str(&value) {
            out.insert(HeaderName::from_static(name), value);
        }
    }
    response
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SignBody {
    paths: Vec<String>,
    ttl_s: Option<u64>,
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `POST /media/sign {paths, ttl_s?}` → `{urls, expires}` (bearer token only).
async fn sign_urls(
    auth: Option<Extension<Authenticated>>,
    signer: Option<Extension<Arc<UrlSigner>>>,
    request_id: Option<Extension<RequestId>>,
    body: Result<Json<SignBody>, JsonRejection>,
) -> Response {
    let rid = rid(request_id.as_ref());
    let rid = rid.as_deref();
    let (Some(_), Some(Extension(signer))) = (auth, signer) else {
        let mut response = (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "error": { "code": "unauthorized", "message": "signing needs a bearer token" },
                "request_id": rid,
            })),
        )
            .into_response();
        response
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        return response;
    };
    let invalid = |param: &str, message: String| {
        MediaError::Invalid {
            param: param.to_owned(),
            message,
        }
        .into_response(rid)
    };
    let body = match body {
        Ok(Json(body)) => body,
        Err(e) => {
            return invalid(
                "",
                format!("expected {{\"paths\": [...], \"ttl_s\"?}}: {e}"),
            );
        }
    };
    if body.paths.len() > MAX_SIGN_BATCH {
        return invalid("paths", format!("at most {MAX_SIGN_BATCH} per request"));
    }
    let ttl = body.ttl_s.unwrap_or(DEFAULT_SIGN_TTL.as_secs());
    if !(1..=MAX_SIGN_TTL.as_secs()).contains(&ttl) {
        return invalid(
            "ttl_s",
            format!("must be within 1..={}", MAX_SIGN_TTL.as_secs()),
        );
    }
    let expires = unix_now().saturating_add(ttl);
    let mut urls = Vec::with_capacity(body.paths.len());
    for (i, path) in body.paths.iter().enumerate() {
        match signer.sign(path, expires) {
            Ok(url) => urls.push(url),
            Err(e) => return invalid(&format!("paths/{i}"), e.to_string()),
        }
    }
    let mut response = Json(json!({ "urls": urls, "expires": expires })).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn if_none_match_accepts_lists_weak_tags_and_star() {
        let mut h = HeaderMap::new();
        assert!(!none_match(&h, "a"));
        h.insert(
            header::IF_NONE_MATCH,
            HeaderValue::from_static("\"x\", W/\"a\""),
        );
        assert!(none_match(&h, "a"));
        assert!(!none_match(&h, "b"));
        h.insert(header::IF_NONE_MATCH, HeaderValue::from_static("*"));
        assert!(none_match(&h, "b"));
        h.insert(header::IF_NONE_MATCH, HeaderValue::from_static("a"));
        assert!(!none_match(&h, "a"), "tags are quoted on the wire");
    }

    #[test]
    fn cancel_hooks_run_once_and_late_hooks_run_at_once() {
        let cancel = MediaCancel::new();
        let runs = Arc::new(AtomicU64::new(0));
        let r = Arc::clone(&runs);
        cancel.on_cancel(move || {
            r.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(runs.load(Ordering::SeqCst), 0);
        cancel.cancel();
        cancel.cancel();
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        let r = Arc::clone(&runs);
        cancel.on_cancel(move || {
            r.fetch_add(10, Ordering::SeqCst);
        });
        assert_eq!(runs.load(Ordering::SeqCst), 11);
        assert!(cancel.is_cancelled());
    }

    #[test]
    fn errors_map_to_statuses_and_codes() {
        for (e, status, code) in [
            (MediaError::NotFound("x".into()), 404, "not_found"),
            (MediaError::NoStream("x".into()), 404, "no_stream"),
            (MediaError::NotReady("x".into()), 404, "not_ready"),
            (MediaError::Offline("x".into()), 404, "media_offline"),
            (
                MediaError::Invalid {
                    param: "w".into(),
                    message: "m".into(),
                },
                422,
                "invalid_params",
            ),
            (
                MediaError::ContentChanged("x".into()),
                409,
                "content_changed",
            ),
            (MediaError::Unavailable("x".into()), 503, "unavailable"),
            (MediaError::Internal("x".into()), 500, "media_error"),
        ] {
            let (s, c) = e.status_and_code();
            assert_eq!((s.as_u16(), c), (status, code));
        }
    }
}
