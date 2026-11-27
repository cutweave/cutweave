# Media HTTP endpoints

Thumbnails, filmstrips, waveforms and sequence frames over plain HTTP, for the editor (`<img>`, `fetch`) and for agents. Implemented in `cutweave_api::media` (routes, auth, caching; WP-1I.03) over the engine's `cutweave_engine::media_http` (the caches of [§7](../01-architecture.md#7-media-pipeline-cutweave-media-cutweave-cache) and the renderer of [§8](../01-architecture.md#8-render-pipeline-cutweave-render-cutweave-text)); mounted by `cutweave_engine::api_mounts` / `media_mounts`. Architecture: [§6](../01-architecture.md#6-api-surfaces-cutweave-api), [§10](../01-architecture.md#10-playback--preview-delivery-cutweave-playback-cutweave-bridge) (the JPEG preview fallback), [§18](../01-architecture.md#18-security--privacy). The waveform wire format is in [waveform-format.md](waveform-format.md).

## Routes

All routes live on the engine's server, `http://127.0.0.1:<port>`, behind the Host/Origin check of [rpc.md](rpc.md#connecting). `{id}` is a media id of the open project (`med_…`).

| Route | Answer |
|---|---|
| `GET /media/{id}/thumb?t=&w=&hash=` | `image/jpeg`: the frame on screen at media time `t`, `w` px wide at most (never upscaled; the height keeps the picture's aspect) |
| `GET /media/{id}/filmstrip?w=&h=&count=&hash=&fmt=` | `image/jpeg`: a sprite of `count` tiles, with its manifest in the `x-cutweave-filmstrip` header; `fmt=json`: the manifest alone (`application/json`) |
| `GET /media/{id}/waveform?lod=&start=&end=&track=&stream=&hash=&fmt=&w=&h=` | `application/octet-stream`: peak data (`CWPD`, [waveform-format.md](waveform-format.md#peak-data-query-result-and-wire-form)); `fmt=png`: the peaks drawn into a `w × h` PNG |
| `GET /media/frame?seq=&t=&w=&h=&fmt=&q=` | `image/jpeg` or `image/png`: the sequence frame at `t`, the bytes `render.frame` returns |
| `POST /media/sign` | Signed URLs for clients that cannot send a header (see [Auth](#auth)) |

`HEAD` works wherever `GET` does.

### Parameters

| Parameter | Routes | Values | Default |
|---|---|---|---|
| `t` | thumb, frame | Flicks (integer), or the text forms of a time: seconds `2.5s`, a frame `f:60`, a timecode. Thumbnails resolve frames and timecodes at the media's rate, frames at the sequence's. | thumb: `0`; frame: required |
| `w` | all | `1..=4096` px. Thumb: largest width. Filmstrip: tile width. Waveform PNG: image width. Frame: largest width. | thumb `320`, filmstrip `160`, waveform PNG `1024`, frame: see `h` |
| `h` | filmstrip, waveform, frame | `1..=4096` px. Filmstrip: largest tile height (the tile narrows to keep the aspect). Waveform PNG: height. Frame: largest height. | waveform PNG `128` |
| `count` | filmstrip | `1..=1000` tiles | `100` |
| `hash` | thumb, filmstrip, waveform | The content hash the client expects (`blake3-fast:<hex>`, the asset's `content_hash`) | none |
| `fmt` | filmstrip, waveform, frame | filmstrip `jpeg`, `json`; waveform `peaks`, `png`; frame `jpeg`, `png` | the first |
| `lod` | waveform | `0..=11`: bins of `256 × 4^lod` samples | `0` |
| `start`, `end` | waveform | Media times as for `t`; `end` after `start` | the whole audio |
| `track` | waveform | `mix`, or a 0-based channel number (needs the per-channel pyramid) | `mix` |
| `stream` | waveform | An audio stream's container index | the first audio stream |
| `seq` | frame | A sequence id | the project's only sequence |
| `q` | frame (JPEG) | JPEG quality `1..=100` | `80` |

The defaults of `/thumb` (`t=0&w=320`) and `/filmstrip` (`w=160&count=100`) are what the import analysis caches, so they cost no decode.

- **Orientation:** thumbnails and filmstrip tiles are upright, turned by the media's display rotation (a phone clip shot upright, a photo's EXIF orientation), as the preview shows the picture. `w`, `h`, `tile_w` and `tile_h` are sizes of the upright picture: a 1920 × 1080 clip with a quarter turn gives tiles taller than wide.
- **Thumbnails:** `t` is floored to the media's frame grid (its nominal rate), so a scrub never makes more entries than the clip has frames; a `t` before the stream gives the first frame, after it the last.
- **Filmstrip manifest:** `{tile_w, tile_h, count, columns, rows, sprite_w, sprite_h, start, interval}`. Tile `i` sits at column `i % columns`, row `i / columns` (the sprite wraps at 8192 px) and shows the frame on screen at media time `start + i × interval` (flicks).
- **Frames:** the image keeps the sequence's aspect and fits inside `w × h` (either may be absent; 768 px longest side when both are). `/media/frame?w=W` is `render.frame{max_size}` with the largest `max_size` whose image fits, so `/media/frame?t=2s&w=640` and `render.frame{t: "2s", max_size: 640}` return the same bytes for a landscape sequence. It decodes in software without the frame cache, exactly as `render.frame` and exports do, and reflects the live document; the `x-cutweave-rev` header names its revision.
- **Unknown parameters** are refused (422): a typo is an error, not a silent default. `exp` and `sig` belong to signed URLs.

Not served: proxies, video streams, range requests.

## Auth

Every route needs a credential; without one the answer is **401** with `WWW-Authenticate: Bearer`.

- **Bearer token:** `Authorization: Bearer <token>` with the UI token (the editor's `fetch`) or an agent token from the run-file. A token is never accepted in a URL.
- **Signed URL** (`GET` and `HEAD` only): for an `<img src>`, which cannot send headers. The client holding a token asks the engine to sign exact URLs:

  ```http
  POST /media/sign
  Authorization: Bearer <token>
  Content-Type: application/json

  {"paths": ["/media/med_…/thumb?t=0&w=320&hash=blake3-fast:…"], "ttl_s": 3600}
  ```

  ```json
  {"urls": ["/media/med_…/thumb?t=0&w=320&hash=blake3-fast%3A…&exp=1764000000&sig=6f…"], "expires": 1764000000}
  ```

  - `sig` is HMAC-SHA256 (RFC 2104) over `<path>?<query>` up to and including `exp`, with a 256-bit key drawn from the OS random source when the engine starts. The key never leaves the engine process; a restart invalidates every URL.
  - The signature covers the whole path and query: it opens that one resource, and changing any parameter breaks it.
  - `ttl_s` is `1..=86400` (default 3600, an hour); clients re-sign before `expires` (Unix seconds).
  - The signer re-encodes the query canonically (`application/x-www-form-urlencoded`), so browsers send the URL unchanged; `sig` is always the last parameter.
  - Up to 256 paths per call. Each must be a `/media/…` route other than `/media/sign`, ASCII letters, digits and `/-_.~` in the path, without `exp` or `sig`; otherwise 422 naming `paths/<i>`.
  - `/media/sign` itself takes only a bearer token.

| Refusal | Status | `error.code` |
|---|---|---|
| No token, no signature | 401 | `unauthorized` |
| Unknown token (even on a signed URL) | 401 | `unauthorized` |
| Signature does not match, is malformed, or the method is not `GET`/`HEAD` | 401 | `bad_signature` |
| Signature expired | 401 | `expired_signature` |

A bad or expired signature is a **401** (not 403): the request carries no valid credential, and the client's remedy is to get one (sign again).

## Caching

Every answer is a pure function of the media's content and the parameters.

- **ETag:** thumbnails and filmstrips use the cache's tags, `"<content hash>-<16 hex of the parameters>"` (a filmstrip's `fmt=json` adds `-json`); waveforms `"<content hash>-<16 hex>"` over level, range, track, stream and format. They are computed from the request alone.
- **`If-None-Match`** with a matching tag (a list, weak tags and `*` are understood) answers **304** before anything else runs: no decode, no cache read.
- **`Cache-Control`:** `private, max-age=31536000, immutable` when the URL names the content (`hash=` equal to the media's current content hash); `private, no-cache` without `hash=` (the media id may later point at other content after a relink, so clients revalidate; a 304 is free). Frames: `no-store`. Errors: `no-store`.
- **`hash=` that no longer matches:** 409 `content_changed` (the client's document is out of date; ask again with the current hash).

## Errors

JSON, like the server's other errors: `{"error": {"code", "message", "param"?}, "request_id"}`.

| Status | `error.code` | When |
|---|---|---|
| 404 | `not_found` | Unknown media or sequence |
| 404 | `no_stream` | A thumbnail or filmstrip of media without pictures, a waveform of media without sound |
| 404 | `not_ready` | The waveform is not computed yet (its `waveform` analysis job writes it; the route never computes one) |
| 404 | `media_offline` | The file is not where the document says |
| 409 | `content_changed` | `hash=` names other content |
| 422 | `invalid_params` | A parameter out of bounds, unknown or repeated (`param` names it), e.g. `w=100000`; a waveform `lod` or `track` the pyramid lacks |
| 503 | `unavailable` | The engine has no media backend (`--dev-stub-media`, a build without FFmpeg), no usable cache directory, or no open document |
| 500 | `media_error` | Decoding or encoding failed |

## Concurrency and cancellation

- Requests that produce (decode, read a cache, render) take one of 4 slots; the rest wait. A 304 or an error needs no slot.
- Production runs on the blocking pool. When the client disconnects, the request's decode is cancelled through its `CancelToken` (measured: about 1 ms with the test backend, about 20 ms for a 4K FFmpeg filmstrip; the budget is 100 ms). A cancelled thumbnail or filmstrip stores nothing.
- Frames share the renderer with `render.frame` and exports, one at a time.

## The editor

- **Media bin cards** (`packages/editor/src/media/httpThumbnails.ts`): the cached poster (`t=0&w=320&hash=…`), signed in one batch per render and re-signed five minutes before it expires; a video asks once it is indexed, sound keeps the icon tile.
- **Timeline** (`packages/editor/src/media/timelineLoaders.ts`): filmstrip tiles are cut from the cached strip (`w=160&count=100`, one request per asset); zoomed in past the strip's spacing, a tile asks `/thumb` for its own frame. A tile is the centred part of the picture with the slot's shape, so an upright clip is cropped, not stretched. Waveforms load the coarsest level that keeps the zoom's resolution. Requests carry the bearer token in a header and are aborted when their tile scrolls out of view.
- **JPEG preview fallback** (`packages/editor/src/preview/jpegPreview.ts`): `/media/frame?seq=&t=&w=&h=&fmt=jpeg` with the bearer token, at most 15 a second.
- Installed only when the engine reports `media.capabilities{available: true}`; a finished `thumbnails` or `waveform` job makes the timeline ask again (`packages/editor/src/media/mediaWiring.ts`).
