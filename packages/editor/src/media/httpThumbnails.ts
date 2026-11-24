// SPDX-License-Identifier: GPL-3.0-or-later
// The media bin's real thumbnails (WP-1I.03): a `ThumbnailSource` over `GET /media/{id}/thumb`.
// Cards load it with `<img src>`, which cannot send the token, so every URL is signed by the
// engine (`POST /media/sign`); the token never appears in a URL.
//
// - What: the poster at t = 0, 320 px wide (the one the import analysis caches, so it costs no
//   decode), with `hash=` the asset's content hash: the URL names the content, the engine sends
//   it as immutable, and a relink (new content) is a new URL.
// - When: a video asks only once its frame index exists (`indexed` > 0); a still asks at once;
//   sound has no picture and keeps the tile. Until a URL is signed the card keeps its tile.
// - Signing: requests made while the panel renders are batched into one call; URLs are
//   re-signed a few minutes before they expire. A failed signing waits before it is tried again.
// - `onChange` runs when new URLs arrive, so the panel can ask again (`setThumbnailSource` with
//   a fresh wrapper).
import type { MediaId } from "@cutweave/protocol";
import type { MediaHttp } from "./mediaHttp.ts";
import { mediaPath } from "./mediaHttp.ts";
import type { Thumbnail, ThumbnailRequest, ThumbnailSource } from "./thumbnails.ts";

/** The poster width the import analysis caches. */
export const POSTER_WIDTH = 320;
/** Re-sign this long before a URL expires. */
export const RESIGN_MARGIN_MS = 5 * 60_000;
/** Wait this long after a failed signing. */
export const SIGN_RETRY_MS = 5_000;

const PLACEHOLDER: Thumbnail = { kind: "placeholder" };

interface Signed {
  readonly url: string;
  readonly expires: number;
}

export interface HttpThumbnailOptions {
  readonly http: Pick<MediaHttp, "sign">;
  /** The asset's content hash in the open document; `null` when it has none (yet). */
  readonly contentHash: (id: MediaId) => string | null;
  /** New URLs arrived: the cards should ask again. */
  readonly onChange: () => void;
  readonly now?: () => number;
  /** Runs the batched signing call (default: a microtask). */
  readonly schedule?: (run: () => void) => void;
  /** Runs `run` after `ms` (default: `setTimeout`): asking again after a failed signing. */
  readonly later?: (run: () => void, ms: number) => void;
}

export class HttpThumbnailSource implements ThumbnailSource {
  readonly #options: HttpThumbnailOptions;
  readonly #now: () => number;
  readonly #schedule: (run: () => void) => void;
  #signed = new Map<string, Signed>();
  #wanted = new Set<string>();
  #pending = new Set<string>();
  #retryAt = 0;
  #queued = false;
  #epoch = 0;

  constructor(options: HttpThumbnailOptions) {
    this.#options = options;
    this.#now = options.now ?? (() => Date.now());
    this.#schedule = options.schedule ?? ((run) => queueMicrotask(run));
  }

  /** The thumbnail path of an asset (before signing). */
  static path(id: MediaId, hash: string): string {
    return mediaPath(id, "thumb", { t: 0, w: POSTER_WIDTH, hash });
  }

  thumbnailFor(request: ThumbnailRequest): Thumbnail {
    if (request.kind === "audio" || request.kind === "sequence_ref") return PLACEHOLDER;
    if (request.kind === "video" && request.indexed === 0) return PLACEHOLDER;
    const hash = this.#options.contentHash(request.id);
    if (hash === null) return PLACEHOLDER;
    const path = HttpThumbnailSource.path(request.id, hash);
    const signed = this.#signed.get(path);
    const now = this.#now();
    if (signed === undefined || signed.expires - now < RESIGN_MARGIN_MS) this.#want(path);
    return signed !== undefined && signed.expires > now
      ? { kind: "image", src: signed.url }
      : PLACEHOLDER;
  }

  /** Forgets every URL (the engine restarted: its signing key is new). */
  reset(): void {
    this.#epoch += 1;
    this.#signed = new Map();
    this.#wanted = new Set();
    this.#pending = new Set();
    this.#retryAt = 0;
    this.#options.onChange();
  }

  #want(path: string): void {
    if (this.#pending.has(path) || this.#now() < this.#retryAt) return;
    this.#wanted.add(path);
    if (this.#queued) return;
    this.#queued = true;
    this.#schedule(() => {
      this.#queued = false;
      void this.#flush();
    });
  }

  async #flush(): Promise<void> {
    const paths = [...this.#wanted];
    this.#wanted.clear();
    if (paths.length === 0) return;
    const epoch = this.#epoch;
    for (const p of paths) this.#pending.add(p);
    try {
      const { urls, expires } = await this.#options.http.sign(paths);
      if (epoch !== this.#epoch) return;
      paths.forEach((path, i) => {
        const url = urls[i];
        if (url !== undefined) this.#signed.set(path, { url, expires });
      });
      this.#options.onChange();
    } catch {
      if (epoch === this.#epoch) {
        this.#retryAt = this.#now() + SIGN_RETRY_MS;
        const later = this.#options.later ?? ((run, ms) => setTimeout(run, ms));
        later(() => {
          if (epoch === this.#epoch) this.#options.onChange();
        }, SIGN_RETRY_MS);
      }
    } finally {
      if (epoch === this.#epoch) for (const p of paths) this.#pending.delete(p);
    }
  }
}
