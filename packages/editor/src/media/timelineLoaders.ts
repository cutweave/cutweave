// SPDX-License-Identifier: GPL-3.0-or-later
// The timeline's media loaders (`MediaLoaders` of `@cutweave/timeline`) over the engine's media
// HTTP endpoints (WP-1I.03): video clips draw filmstrip tiles, audio clips draw waveforms.
//
// - Filmstrip tiles come from the strip the import analysis caches (`/media/{id}/filmstrip`,
//   100 tiles of 160 px: one request per asset, no decode), cropped per tile. Zoomed in further
//   than the strip's spacing, a tile asks for its own frame (`/media/{id}/thumb`), which decodes.
// - Waveforms are the asset's peak pyramid at the coarsest level that still gives the
//   resolution asked for (`/media/{id}/waveform?lod=`), as min/max per bin.
// - Every URL carries `hash=` (the asset's content hash), so the engine answers it as immutable
//   and the browser cache keeps it; the token goes in the `Authorization` header.
// - Cancellation: the timeline aborts a tile or peaks request (its `AbortSignal`) when it
//   scrolls out of view; the request is aborted and the engine stops the decode.
// - "Not there yet" (no picture, no sound, the analysis has not written it) resolves `null`;
//   `refresh()` forgets those answers so a finished analysis is picked up.
import type { MediaId } from "@cutweave/protocol";
import type {
  FilmstripRequest,
  MediaLoaders,
  WaveformPeaks,
  WaveformRequest,
} from "@cutweave/timeline";
import { type MediaHttp, MediaHttpError, mediaPath } from "./mediaHttp.ts";

/** The strip the import analysis caches: 100 tiles of 160 px. */
export const STRIP_TILE_WIDTH = 160;
export const STRIP_TILES = 100;
/** Bins of waveform level 0, and the factor between levels (docs/reference/waveform-format.md). */
export const WAVEFORM_BASE_BIN = 256;
export const WAVEFORM_LEVEL_FACTOR = 4;
export const MAX_WAVEFORM_LOD = 11;
/** Sprites kept decoded (one per asset on screen). */
const MAX_SPRITES = 8;

/** What the loaders need to know about an asset of the open document. */
export interface LoaderMedia {
  /** Content hash (`blake3-fast:…`); `null` when the document has none. */
  readonly hash: string | null;
  /** Sample rate of its main audio stream, if known. */
  readonly sampleRate: number | null;
}

/** The layout of a filmstrip sprite (the engine's `x-cutweave-filmstrip` manifest). */
export interface FilmstripManifest {
  readonly tile_w: number;
  readonly tile_h: number;
  readonly count: number;
  readonly columns: number;
  /** Media time of tile 0, flicks. */
  readonly start: number;
  /** Media time between tiles, flicks. */
  readonly interval: number;
}

interface Sprite {
  readonly bitmap: ImageBitmap;
  readonly manifest: FilmstripManifest;
}

export interface HttpLoaderOptions {
  readonly http: Pick<MediaHttp, "get">;
  readonly media: (id: MediaId) => LoaderMedia | null;
  /** Decodes an image (default `createImageBitmap`). */
  readonly decode?: (blob: Blob) => Promise<ImageBitmap>;
  /** Cuts a tile out of a sprite (default `createImageBitmap(source, sx, sy, sw, sh)`). */
  readonly crop?: (
    source: ImageBitmap,
    sx: number,
    sy: number,
    sw: number,
    sh: number,
  ) => Promise<ImageBitmap>;
  /** Device pixels per CSS pixel (default `devicePixelRatio`). */
  readonly pixelRatio?: () => number;
}

export interface HttpMediaLoaders extends MediaLoaders {
  /** Forgets the "not there yet" answers (an analysis finished); keeps what loaded. */
  refresh(): void;
  /** Releases the decoded sprites. */
  dispose(): void;
}

/** The coarsest waveform level whose bins still give `peaksPerSecond` at `sampleRate`. */
export function waveformLod(sampleRate: number, peaksPerSecond: number): number {
  let lod = 0;
  while (
    lod < MAX_WAVEFORM_LOD &&
    sampleRate / (WAVEFORM_BASE_BIN * WAVEFORM_LEVEL_FACTOR ** (lod + 1)) >= peaksPerSecond
  ) {
    lod += 1;
  }
  return lod;
}

/** The engine's binary peak data (`CWPD`, docs/reference/waveform-format.md) as peaks. */
export function parsePeaks(buffer: ArrayBuffer): WaveformPeaks {
  const view = new DataView(buffer);
  const magic = String.fromCharCode(...new Uint8Array(buffer, 0, Math.min(4, buffer.byteLength)));
  if (buffer.byteLength < 32 || magic !== "CWPD" || view.getUint8(4) !== 1) {
    throw new Error("not CWPD v1 peak data");
  }
  const rate = view.getUint32(12, true);
  const perBin = view.getUint32(16, true);
  const firstBin = Number(view.getBigUint64(20, true));
  const bins = view.getUint32(28, true);
  if (rate === 0 || perBin === 0 || buffer.byteLength < 32 + 4 * bins) {
    throw new Error("truncated peak data");
  }
  const min = new Float32Array(firstBin + bins);
  const max = new Float32Array(firstBin + bins);
  for (let i = 0; i < bins; i++) {
    min[firstBin + i] = view.getInt16(32 + 4 * i, true) / 32767;
    max[firstBin + i] = view.getInt16(34 + 4 * i, true) / 32767;
  }
  return { peaksPerSecond: rate / perBin, min, max };
}

/** The tile of `manifest` that shows media time `t` (flicks). */
export function tileIndex(manifest: FilmstripManifest, t: number): number {
  const i = Math.floor((t - manifest.start) / Math.max(1, manifest.interval));
  return Math.min(manifest.count - 1, Math.max(0, i));
}

function parseManifest(header: string | null): FilmstripManifest | null {
  if (header === null) return null;
  try {
    const m = JSON.parse(header) as Partial<FilmstripManifest>;
    const fields = [m.tile_w, m.tile_h, m.count, m.columns, m.start, m.interval];
    return fields.every((v) => typeof v === "number") && (m.columns ?? 0) > 0
      ? (m as FilmstripManifest)
      : null;
  } catch {
    return null;
  }
}

/** A missing picture, sound or analysis: show the placeholder, ask again after `refresh()`. */
const absent = (e: unknown): boolean => e instanceof MediaHttpError && e.status === 404;

export function httpMediaLoaders(options: HttpLoaderOptions): HttpMediaLoaders {
  const decode = options.decode ?? ((blob: Blob) => createImageBitmap(blob));
  const crop =
    options.crop ??
    ((source: ImageBitmap, sx: number, sy: number, sw: number, sh: number) =>
      createImageBitmap(source, sx, sy, sw, sh));
  const pixelRatio = options.pixelRatio ?? (() => globalThis.devicePixelRatio || 1);
  /** Sprite loads by `id|hash`, most recent last; `null` = the asset has none (yet). */
  const sprites = new Map<string, Promise<Sprite | null>>();
  /** Keys whose load answered `null` ("not there yet"). */
  const missing = new Set<string>();

  const dropSprite = (key: string) => {
    const old = sprites.get(key);
    sprites.delete(key);
    void old?.then((s) => s?.bitmap.close()).catch(() => undefined);
  };

  function sprite(id: MediaId, hash: string): Promise<Sprite | null> {
    const key = `${id}|${hash}`;
    const cached = sprites.get(key);
    if (cached !== undefined) {
      sprites.delete(key);
      sprites.set(key, cached);
      return cached;
    }
    // Not tied to one tile's signal: every tile of the asset waits for it.
    const load = (async () => {
      const path = mediaPath(id, "filmstrip", {
        w: STRIP_TILE_WIDTH,
        count: STRIP_TILES,
        hash,
      });
      try {
        const response = await options.http.get(path);
        const manifest = parseManifest(response.headers.get("x-cutweave-filmstrip"));
        if (manifest === null) {
          missing.add(key);
          return null;
        }
        return { bitmap: await decode(await response.blob()), manifest };
      } catch (e) {
        if (absent(e)) {
          missing.add(key);
          return null;
        }
        // A failure is not remembered: the next tile asks again.
        sprites.delete(key);
        throw e;
      }
    })();
    sprites.set(key, load);
    while (sprites.size > MAX_SPRITES) {
      const oldest = sprites.keys().next().value;
      if (oldest === undefined) break;
      dropSprite(oldest);
    }
    return load;
  }

  async function filmstripTile(
    req: FilmstripRequest,
    signal: AbortSignal,
  ): Promise<ImageBitmap | null> {
    const media = options.media(req.mediaId as MediaId);
    if (media === null || media.hash === null) return null;
    const strip = await sprite(req.mediaId as MediaId, media.hash);
    signal.throwIfAborted();
    if (strip === null) return null;
    const { manifest } = strip;
    const center = req.sourceTime + req.tileDuration / 2;
    if (req.tileDuration * 2 < manifest.interval) {
      // Zoomed in past the strip: this tile's own frame.
      const w = Math.min(640, Math.max(32, Math.ceil((req.width * pixelRatio()) / 32) * 32));
      const path = mediaPath(req.mediaId, "thumb", { t: Math.floor(center), w, hash: media.hash });
      try {
        const response = await options.http.get(path, signal);
        return await decode(await response.blob());
      } catch (e) {
        if (absent(e)) return null;
        throw e;
      }
    }
    const i = tileIndex(manifest, center);
    const sx = (i % manifest.columns) * manifest.tile_w;
    const sy = Math.floor(i / manifest.columns) * manifest.tile_h;
    return crop(strip.bitmap, sx, sy, manifest.tile_w, manifest.tile_h);
  }

  async function waveform(
    req: WaveformRequest,
    signal: AbortSignal,
  ): Promise<WaveformPeaks | null> {
    const media = options.media(req.mediaId as MediaId);
    if (media === null || media.hash === null) return null;
    const lod = waveformLod(media.sampleRate ?? 48_000, req.peaksPerSecond);
    const path = mediaPath(req.mediaId, "waveform", { lod, hash: media.hash });
    try {
      const response = await options.http.get(path, signal);
      return parsePeaks(await response.arrayBuffer());
    } catch (e) {
      if (absent(e)) return null;
      throw e;
    }
  }

  return {
    filmstripTile,
    waveform,
    refresh() {
      for (const key of missing) sprites.delete(key);
      missing.clear();
    },
    dispose() {
      for (const key of [...sprites.keys()]) dropSprite(key);
    },
  };
}
