// SPDX-License-Identifier: GPL-3.0-or-later
// Filmstrip tiles and waveforms inside clip bodies. Both come from the media caches, keyed by a
// level of detail picked from the zoom (half-octave buckets). A frame never waits for a load: a
// tile or peaks array that is not cached yet is asked for, and meanwhile the nearest zoom level
// already cached stands in (zooming shows the old level scaled, not a flash of placeholders);
// with nothing cached a placeholder shade is drawn. Nothing here allocates on a cache hit.
import { bucketPxPerSec, MAX_TILE_INDEX, type WaveformPeaks, zoomBucket } from "../cache.ts";
import { BODY_MEDIA_MIN_PX, TILE_ASPECT } from "../geometry.ts";
import type { IndexList } from "../interval-index.ts";
import type { MediaSlice } from "../model.ts";
import { FLICKS_PER_SECOND } from "../time.ts";
import type { RenderState } from "./state.ts";

/** Waveform bar spacing, CSS px. */
const WAVE_STEP = 2;
/** At most this many tiles per clip and frame. */
const MAX_TILES_PER_ITEM = 256;
/** Neighbouring zoom buckets tried, nearest first, while a level loads. */
const FALLBACK_REACH = 4;

/** Media flicks one tile spans at zoom bucket `bucket` for tiles `tilePx` wide. */
export function tileDuration(tilePx: number, bucket: number): number {
  return Math.max(1, Math.round((tilePx / bucketPxPerSec(bucket)) * FLICKS_PER_SECOND));
}

/** One filmstrip tile on an item: tile `j` of the media, from item offset `at` for `span` flicks. */
export interface TileSpan {
  readonly j: number;
  readonly at: number;
  readonly span: number;
}

/**
 * The filmstrip tiles (media tiles of `dur` flicks) an item of `len` flicks shows over its offsets
 * `[u0, u1)` (WP-2D.01): at speed `s` a tile spans `dur / s` timeline flicks, reversed the tiles run
 * from the end of the range, and a freeze frame (`s` 0) repeats its frame's tile. At most 256.
 * (The draw loop walks the same tiles without building this list.)
 */
export function tileSpans(
  slice: MediaSlice,
  len: number,
  dur: number,
  u0: number,
  u1: number,
): TileSpan[] {
  const out: TileSpan[] = [];
  forEachTile(slice, len, dur, u0, u1, (j, at, span) => {
    out.push({ j, at, span });
  });
  return out;
}

/** Calls `fn(j, at, span)` for each tile of {@link tileSpans}, in order. */
function forEachTile(
  slice: MediaSlice,
  len: number,
  dur: number,
  u0: number,
  u1: number,
  fn: (j: number, at: number, span: number) => void,
): void {
  const s = slice.speed ?? 1;
  if (s <= 0) {
    const j = Math.floor(slice.sourceIn / dur);
    let count = 0;
    for (let n = Math.floor(u0 / dur); n * dur < u1 && count < MAX_TILES_PER_ITEM; n++, count++) {
      fn(j, n * dur, dur);
    }
    return;
  }
  const end = slice.sourceIn + len * s;
  const rev = slice.reverse === true;
  const m0 = rev ? end - u1 * s : slice.sourceIn + u0 * s;
  const m1 = rev ? end - u0 * s : slice.sourceIn + u1 * s;
  const j0 = Math.floor(m0 / dur);
  const j1 = Math.floor(m1 / dur);
  for (let j = j0; j <= j1 && j - j0 < MAX_TILES_PER_ITEM; j++) {
    fn(j, rev ? (end - (j + 1) * dur) / s : (j * dur - slice.sourceIn) / s, dur / s);
  }
}

/**
 * Draws the filmstrip tiles of `bucket`'s items into the body area `[areaY, areaY + areaH)`.
 * Placeholders for tiles not loaded yet go into one batched shade.
 */
export function drawTiles(s: RenderState, bucket: IndexList, areaY: number, areaH: number): void {
  const { ctx, f, k, rx0, rx1 } = s;
  const { layout, vp, theme: th, caches } = f;
  const bk = zoomBucket(vp.pxPerSec);
  const tilePx = Math.round(areaH * TILE_ASPECT);
  const dur = tileDuration(tilePx, bk);
  const size = caches.sizeSlot(tilePx, areaH);
  let placeholders = false;
  ctx.beginPath(); // the placeholder batch
  for (let n = 0; n < bucket.length; n++) {
    const i = bucket.data[n] as number;
    const slice = layout.items[i]?.thumbnails;
    if (slice === undefined) continue;
    const start = layout.start[i] ?? 0;
    const len = (layout.end[i] ?? 0) - start;
    const x = vp.headerWidth + (start - vp.scrollX) * k;
    const w = len * k;
    if (w < BODY_MEDIA_MIN_PX) continue;
    const v0 = Math.max(x, rx0, vp.headerWidth);
    const v1 = Math.min(x + w, rx1);
    if (v1 <= v0) continue;
    const media = caches.mediaSlot(slice.mediaId);
    // The tile walk of `forEachTile`, inlined so the hot loop makes no closure.
    const sp = slice.speed ?? 1;
    const u0 = (v0 - x) / k;
    const u1 = (v1 - x) / k;
    // A freeze frame (speed 0) repeats its frame's tile at the tile pitch.
    const frozen = sp <= 0;
    const end = slice.sourceIn + len * sp;
    const rev = slice.reverse === true;
    let j0: number;
    let j1: number;
    if (frozen) {
      j0 = Math.floor(u0 / dur);
      j1 = Math.ceil(u1 / dur) - 1;
    } else {
      j0 = Math.floor((rev ? end - u1 * sp : slice.sourceIn + u0 * sp) / dur);
      j1 = Math.floor((rev ? end - u0 * sp : slice.sourceIn + u1 * sp) / dur);
    }
    const frozenTile = Math.floor(slice.sourceIn / dur);
    const tw = frozen ? dur * k : (dur / sp) * k;
    for (let n = j0; n <= j1 && n - j0 < MAX_TILES_PER_ITEM; n++) {
      const at = frozen
        ? n * dur
        : rev
          ? (end - (n + 1) * dur) / sp
          : (n * dur - slice.sourceIn) / sp;
      const tx = x + at * k;
      const d0 = Math.max(tx, v0);
      const d1 = Math.min(tx + tw, v1);
      if (d1 <= d0) continue;
      const j = frozen ? frozenTile : n;
      if (tile(s, slice.mediaId, media, size, bk, j, dur, tilePx, tx, tw, d0, d1, areaY, areaH)) {
        continue;
      }
      ctx.rect(d0, areaY, d1 - d0, areaH);
      s.stats.placeholders++;
      placeholders = true;
    }
  }
  if (placeholders) {
    ctx.save();
    ctx.globalAlpha *= 0.5;
    ctx.fillStyle = th.surface3;
    ctx.fill();
    ctx.restore();
    s.stats.drawCalls++;
  }
}

/**
 * Draws tile `j` (media flicks `[j * dur, (j + 1) * dur)`) of a media at canvas `[tx, tx + tw)`,
 * clipped to `[d0, d1)`, in the body area. Returns false when neither its own level nor a
 * neighbouring one is cached: the caller draws a placeholder.
 */
function tile(
  s: RenderState,
  mediaId: string,
  media: number,
  size: number,
  bk: number,
  j: number,
  dur: number,
  tilePx: number,
  tx: number,
  tw: number,
  d0: number,
  d1: number,
  areaY: number,
  areaH: number,
): boolean {
  if (j < 0 || j >= MAX_TILE_INDEX) return false;
  const { ctx, f } = s;
  const caches = f.caches;
  const key = caches.tileKey(media, size, bk, j);
  const bmp = caches.filmstrip.get(key);
  if (bmp !== undefined) {
    const sx = ((d0 - tx) / tw) * bmp.width;
    const sw = ((d1 - d0) / tw) * bmp.width;
    ctx.drawImage(bmp, sx, 0, sw, bmp.height, d0, areaY, d1 - d0, areaH);
    s.stats.tiles++;
    s.stats.drawCalls++;
    return true;
  }
  if (caches.filmstrip.needsLoad(key)) {
    caches.requestTile(key, {
      mediaId,
      sourceTime: j * dur,
      tileDuration: dur,
      width: tilePx,
      height: areaH,
    });
  }
  // Meanwhile: the tile of a neighbouring zoom level that shows the middle of this one.
  const mid = (j + 0.5) * dur;
  for (let r = 1; r <= FALLBACK_REACH; r++) {
    for (let side = -1; side <= 1; side += 2) {
      const b = bk + side * r;
      const jb = Math.floor(mid / tileDuration(tilePx, b));
      if (jb < 0 || jb >= MAX_TILE_INDEX) continue;
      const alt = caches.filmstrip.peek(caches.tileKey(media, size, b, jb));
      if (alt === undefined) continue;
      const sx = ((d0 - tx) / tw) * alt.width;
      const sw = ((d1 - d0) / tw) * alt.width;
      ctx.drawImage(alt, sx, 0, sw, alt.height, d0, areaY, d1 - d0, areaH);
      s.stats.fallbackTiles++;
      s.stats.drawCalls++;
      return true;
    }
  }
  return false;
}

/** Draws the waveforms of `bucket`'s items into the body area as one stroked path. */
export function drawWaveforms(
  s: RenderState,
  bucket: IndexList,
  color: string,
  areaY: number,
  areaH: number,
): void {
  const { ctx, f, k, rx0, rx1 } = s;
  const { layout, vp } = f;
  const bk = zoomBucket(vp.pxPerSec);
  const mid = areaY + areaH / 2;
  const half = areaH / 2 - 1;
  let any = false;
  ctx.beginPath();
  for (let n = 0; n < bucket.length; n++) {
    const i = bucket.data[n] as number;
    const slice = layout.items[i]?.waveform;
    if (slice === undefined) continue;
    const start = layout.start[i] ?? 0;
    const len = (layout.end[i] ?? 0) - start;
    const x = vp.headerWidth + (start - vp.scrollX) * k;
    const w = len * k;
    if (w < BODY_MEDIA_MIN_PX) continue;
    const peaks = peaksFor(s, slice.mediaId, bk);
    if (peaks === undefined) continue;
    const c0 = Math.max(Math.floor(x), Math.floor(rx0), vp.headerWidth);
    const c1 = Math.min(x + w, rx1);
    const P = peaks.peaksPerSecond;
    const nPeaks = peaks.max.length;
    // WP-2D.01: media time at a timeline offset follows the item's speed and direction.
    const sp = slice.speed ?? 1;
    const out = slice.sourceIn + len * sp;
    const rev = slice.reverse === true;
    const stepSecs = (WAVE_STEP / vp.pxPerSec) * sp;
    for (let c = c0; c < c1; c += WAVE_STEP) {
      const u = (c - x) / k;
      const srcA = (rev ? out - u * sp : slice.sourceIn + u * sp) / FLICKS_PER_SECOND;
      const from = rev ? srcA - stepSecs : srcA;
      const a = Math.floor(from * P);
      const b = Math.max(a, Math.floor((from + stepSecs) * P));
      if (b < 0 || a >= nPeaks) continue;
      const stride = Math.max(1, Math.floor((b - a) / 16));
      let lo = 0;
      let hi = 0;
      for (let p = Math.max(0, a); p <= b && p < nPeaks; p += stride) {
        const mn = peaks.min[p] ?? 0;
        const mx = peaks.max[p] ?? 0;
        if (mn < lo) lo = mn;
        if (mx > hi) hi = mx;
      }
      const top = mid - hi * half;
      const bottom = Math.max(mid - lo * half, top + 1);
      ctx.moveTo(c + 0.5, top);
      ctx.lineTo(c + 0.5, bottom);
      any = true;
    }
  }
  if (any) {
    ctx.strokeStyle = color;
    ctx.lineWidth = 1;
    ctx.stroke();
    s.stats.drawCalls++;
  }
}

/** The peaks of a media at zoom bucket `bk` (asked for when missing), else a cached neighbour. */
function peaksFor(s: RenderState, mediaId: string, bk: number): WaveformPeaks | undefined {
  const caches = s.f.caches;
  const media = caches.mediaSlot(mediaId);
  const key = caches.peaksKey(media, bk);
  const own = caches.waveform.get(key);
  if (own !== undefined) return own;
  if (caches.waveform.needsLoad(key)) {
    caches.requestPeaks(key, {
      mediaId,
      peaksPerSecond: Math.ceil(bucketPxPerSec(bk) * 2),
    });
  }
  for (let r = 1; r <= 2 * FALLBACK_REACH; r++) {
    const finer = caches.waveform.peek(caches.peaksKey(media, bk + r));
    if (finer !== undefined) return finer;
    const coarser = caches.waveform.peek(caches.peaksKey(media, bk - r));
    if (coarser !== undefined) return coarser;
  }
  return undefined;
}
