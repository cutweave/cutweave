// SPDX-License-Identifier: GPL-3.0-or-later
// Where the scrubbing pictures come from (WP-1M.14), over the engine's existing media endpoints
// (docs/reference/http-media.md), with the UI token in a header (never in a URL):
//
// - A card's hover reads the filmstrip sprite the import analysis caches (one request per asset,
//   shared with nothing else, then every position is a crop of the decoded sprite: no request
//   while the pointer moves).
// - The Inspector asks `/thumb?t=&w=` for the exact frame under its scrub bar, one request at
//   a time (`LatestFrames` aborts the one in flight).
// - Sound shows its waveform, drawn by the engine (`/waveform?fmt=png`).
//
// Not available (no media backend, the analysis still making the strip) answers `null`: the
// card keeps its poster and the Inspector its placeholder.
import type { MediaId } from "@cutweave/protocol";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";
import { type MediaHttp, MediaHttpError, mediaPath } from "./mediaHttp.ts";
import {
  type FilmstripManifest,
  parseManifest,
  STRIP_TILE_WIDTH,
  STRIP_TILES,
  tileIndex,
} from "./timelineLoaders.ts";

/** Sprites kept decoded (least recently used goes first). */
const MAX_SPRITES = 8;

export interface ScrubMedia {
  /** The asset's content hash in the open document; `null` when it has none (yet). */
  readonly hash: string | null;
  /** Its import analysis is making the filmstrip right now: do not ask for a second one. */
  readonly makingFilmstrip: boolean;
}

export interface Sprite {
  readonly bitmap: ImageBitmap;
  readonly manifest: FilmstripManifest;
}

/** The part of a sprite that shows media time `t`. */
export function tileOf(sprite: Sprite, t: number) {
  const { manifest } = sprite;
  const i = tileIndex(manifest, t);
  return {
    sx: (i % manifest.columns) * manifest.tile_w,
    sy: Math.floor(i / manifest.columns) * manifest.tile_h,
    sw: manifest.tile_w,
    sh: manifest.tile_h,
  };
}

export interface ScrubSourceOptions {
  readonly http: Pick<MediaHttp, "get">;
  readonly media: (id: MediaId) => ScrubMedia | null;
  /** Decodes an image (default `createImageBitmap`). */
  readonly decode?: (blob: Blob) => Promise<ImageBitmap>;
}

const absent = (e: unknown): boolean => e instanceof MediaHttpError && e.status === 404;

export class ScrubSource {
  readonly #options: ScrubSourceOptions;
  readonly #decode: (blob: Blob) => Promise<ImageBitmap>;
  #sprites = new Map<string, Promise<Sprite | null>>();

  constructor(options: ScrubSourceOptions) {
    this.#options = options;
    this.#decode = options.decode ?? ((blob) => createImageBitmap(blob));
  }

  /** The asset's filmstrip sprite; `null` while it has none. Shared by every caller. */
  sprite(id: MediaId): Promise<Sprite | null> {
    const media = this.#options.media(id);
    if (media === null || media.hash === null || media.makingFilmstrip)
      return Promise.resolve(null);
    const hash = media.hash;
    const key = `${id}|${hash}`;
    const cached = this.#sprites.get(key);
    if (cached !== undefined) {
      this.#sprites.delete(key);
      this.#sprites.set(key, cached);
      return cached;
    }
    const load = (async (): Promise<Sprite | null> => {
      try {
        const path = mediaPath(id, "filmstrip", {
          w: STRIP_TILE_WIDTH,
          count: STRIP_TILES,
          hash,
        });
        const response = await this.#options.http.get(path);
        const manifest = parseManifest(response.headers.get("x-cutweave-filmstrip"));
        if (manifest === null) {
          this.#sprites.delete(key);
          return null;
        }
        return { bitmap: await this.#decode(await response.blob()), manifest };
      } catch (e) {
        // Not remembered: the next hover asks again.
        this.#sprites.delete(key);
        if (absent(e)) return null;
        throw e;
      }
    })();
    this.#sprites.set(key, load);
    while (this.#sprites.size > MAX_SPRITES) {
      const oldest = this.#sprites.keys().next().value;
      if (oldest === undefined) break;
      const old = this.#sprites.get(oldest);
      this.#sprites.delete(oldest);
      void old?.then((s) => s?.bitmap.close()).catch(() => undefined);
    }
    return load;
  }

  /** The frame on screen at media time `t` (flicks), `width` px wide at most. */
  async frame(
    id: MediaId,
    t: number,
    width: number,
    signal: AbortSignal,
  ): Promise<ImageBitmap | null> {
    const media = this.#options.media(id);
    if (media === null || media.hash === null) return null;
    try {
      const path = mediaPath(id, "thumb", { t: Math.floor(t), w: width, hash: media.hash });
      const response = await this.#options.http.get(path, signal);
      return await this.#decode(await response.blob());
    } catch (e) {
      if (absent(e)) return null;
      throw e;
    }
  }

  /** The waveform of a sound, drawn by the engine; `null` until its analysis has measured it. */
  async waveform(
    id: MediaId,
    width: number,
    height: number,
    signal: AbortSignal,
  ): Promise<ImageBitmap | null> {
    const media = this.#options.media(id);
    if (media === null || media.hash === null) return null;
    try {
      const path = mediaPath(id, "waveform", {
        fmt: "png",
        w: width,
        h: height,
        hash: media.hash,
      });
      const response = await this.#options.http.get(path, signal);
      return await this.#decode(await response.blob());
    } catch (e) {
      if (absent(e)) return null;
      throw e;
    }
  }

  /** Releases the decoded sprites. */
  dispose(): void {
    const all = [...this.#sprites.values()];
    this.#sprites = new Map();
    for (const sprite of all) void sprite.then((s) => s?.bitmap.close()).catch(() => undefined);
  }
}

export interface ScrubSourceState {
  /** `null` while the engine has no media backend: nothing can be scrubbed. */
  readonly source: ScrubSource | null;
}

export const scrubSourceStore = createStore<ScrubSourceState>(() => ({ source: null }));

/** Installs (or, with `null`, removes) the source the cards and the Inspector scrub with. */
export function setScrubSource(source: ScrubSource | null): void {
  const old = scrubSourceStore.getState().source;
  scrubSourceStore.setState({ source });
  if (old !== null && old !== source) old.dispose();
}

export function useScrubSource(): ScrubSource | null {
  return useStore(scrubSourceStore, (s) => s.source);
}
