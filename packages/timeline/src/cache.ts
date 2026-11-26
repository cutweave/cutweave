// SPDX-License-Identifier: GPL-3.0-or-later
// Pluggable media loaders and the bitmap/peaks caches in front of them (arch §16: thumbnails and
// waveforms come from the HTTP media endpoints as cached bitmaps). The package never fetches by
// itself: the host passes loaders (typically `fetch` against `/media/{id}/filmstrip` and
// `/media/{id}/waveform?lod=` of the local engine). Tests pass fakes. No network in tests.
//
// The draw loop asks these caches for every visible tile on every frame, so a lookup allocates
// nothing: keys are numbers packed from interned media ids, tile sizes, zoom buckets and tile
// indices; recency is a counter stamped on the entry; a load (its request and closure) is created
// only on a miss that is not already pending.

/** One filmstrip tile: the frame strip of `mediaId` over `[sourceTime, sourceTime + tileDuration)`. */
export interface FilmstripRequest {
  readonly mediaId: string;
  /** Source time of the tile's left edge, flicks. */
  readonly sourceTime: number;
  /** Source time the tile spans, flicks. */
  readonly tileDuration: number;
  /** Nominal tile size, CSS px (the loader may return any size; drawing scales). */
  readonly width: number;
  readonly height: number;
}

export interface WaveformRequest {
  readonly mediaId: string;
  /** Wanted resolution; the loader may return a different one (see `WaveformPeaks`). */
  readonly peaksPerSecond: number;
}

/** Min/max amplitude (-1..1) per peak bucket, index 0 = source time 0. */
export interface WaveformPeaks {
  readonly peaksPerSecond: number;
  readonly min: Float32Array;
  readonly max: Float32Array;
}

export interface MediaLoaders {
  /** Resolve `null` for "no tile" (the placeholder stays); reject on error (not retried). */
  filmstripTile?(req: FilmstripRequest, signal: AbortSignal): Promise<ImageBitmap | null>;
  waveform?(req: WaveformRequest, signal: AbortSignal): Promise<WaveformPeaks | null>;
}

/** The filmstrip bitmap budget (WP-2A.09: the bitmap cache is bounded to 256 MB). */
export const FILMSTRIP_BYTES = 256 * 1024 * 1024;
/** The waveform peaks budget. */
export const WAVEFORM_BYTES = 16 * 1024 * 1024;

/** Half-octave zoom bucket: cache keys and tile sizes are stable while zooming inside one. */
export function zoomBucket(pxPerSec: number): number {
  return Math.round(Math.log2(pxPerSec) * 2);
}

export function bucketPxPerSec(bucket: number): number {
  return 2 ** (bucket / 2);
}

export interface LoadingCacheOptions<V> {
  /** Byte budget; least recently used entries are evicted past it. */
  readonly maxBytes: number;
  readonly sizeOf: (value: V) => number;
  readonly dispose?: (value: V) => void;
  /** Called (not synchronously from `get`) when a load settles with a value. */
  readonly onLoaded: () => void;
  /** Concurrent loads (default 6). */
  readonly concurrency?: number;
}

export type Load<V> = (signal: AbortSignal) => Promise<V | null>;

interface Entry<V> {
  readonly value: V;
  readonly bytes: number;
  /** Recency stamp (higher = used more recently). */
  used: number;
}

interface Pending<V> {
  readonly load: Load<V>;
  seen: number;
}

interface InFlight {
  readonly ctl: AbortController;
  seen: number;
}

/** Once over budget, evict down to this share of it so a full cache does not evict per load. */
const EVICT_TO = 0.9;

/**
 * LRU cache with a byte budget, de-duplicated async loads, bounded concurrency and stale-request
 * dropping: `get` marks a key as wanted in the current frame (`beginFrame`), and queued loads that
 * were not wanted in the previous frame are discarded, so scrubbing and zooming never build a
 * backlog of tiles that scrolled away. A load already running that the last full frame did not
 * want is aborted through its `AbortSignal` (a partial redraw, such as the playhead's, does not
 * ask for every visible tile, so only full frames decide). Newest requests load first.
 *
 * Keys are numbers so a lookup never allocates; `get` on a hit only stamps the entry's recency.
 */
export class LoadingCache<V> {
  private readonly entries = new Map<number, Entry<V>>();
  private readonly queued = new Map<number, Pending<V>>();
  /** Queued keys, newest last (may still hold keys that started or were dropped). */
  private readonly order: number[] = [];
  private readonly inflight = new Map<number, InFlight>();
  private readonly failed = new Set<number>();
  private bytes = 0;
  private frame = 0;
  /** The frame number of the last full frame. */
  private lastFull = 0;
  private clock = 0;
  private disposed = false;
  private pumpScheduled = false;
  // Bound once: the per-frame bookkeeping passes these instead of fresh closures.
  private readonly pumpNow = (): void => {
    this.pumpScheduled = false;
    this.pump();
  };
  private readonly abortUnwanted = (f: InFlight, key: number): void => {
    if (f.seen >= this.lastFull) return;
    f.ctl.abort();
    this.inflight.delete(key);
  };
  private readonly dropStale = (p: Pending<V>, key: number): void => {
    if (p.seen < this.frame - 1) this.queued.delete(key);
  };

  constructor(private readonly opts: LoadingCacheOptions<V>) {}

  get byteSize(): number {
    return this.bytes;
  }

  get maxBytes(): number {
    return this.opts.maxBytes;
  }

  get size(): number {
    return this.entries.size;
  }

  get pending(): number {
    return this.queued.size + this.inflight.size;
  }

  /**
   * Call once per rendered frame, before the frame's `get`s. `full`: the frame draws everything
   * in view (not a partial redraw), so loads it does not ask for have scrolled away.
   */
  beginFrame(full = true): void {
    this.frame++;
    if (!full) return;
    if (this.lastFull > 0 && this.inflight.size > 0) this.inflight.forEach(this.abortUnwanted);
    this.lastFull = this.frame;
    if (this.queued.size > 0) {
      this.queued.forEach(this.dropStale);
      this.schedulePump();
    }
  }

  /** Cached value (refreshing its recency) or undefined. Marks a pending load as still wanted. */
  get(key: number): V | undefined {
    const hit = this.entries.get(key);
    if (hit !== undefined) {
      hit.used = ++this.clock;
      return hit.value;
    }
    const running = this.inflight.get(key);
    if (running !== undefined) {
      running.seen = this.frame;
      return undefined;
    }
    const q = this.queued.get(key);
    if (q !== undefined) q.seen = this.frame;
    return undefined;
  }

  /** Cached value without touching its recency or starting anything (fallback lookups). */
  peek(key: number): V | undefined {
    return this.entries.get(key)?.value;
  }

  has(key: number): boolean {
    return this.entries.has(key);
  }

  /** A miss that nobody is loading yet: the caller should `load` it. */
  needsLoad(key: number): boolean {
    return (
      !this.disposed &&
      !this.entries.has(key) &&
      !this.inflight.has(key) &&
      !this.queued.has(key) &&
      !this.failed.has(key)
    );
  }

  /** Queue a load for `key` (ignored when it is cached, pending or failed). */
  load(key: number, load: Load<V>): void {
    if (!this.needsLoad(key)) return;
    this.queued.set(key, { load, seen: this.frame });
    this.order.push(key);
    this.schedulePump();
  }

  /** `get`, then `load` on a miss (convenience for callers that do not mind the closure). */
  getOrLoad(key: number, load: Load<V> | undefined): V | undefined {
    const v = this.get(key);
    if (v === undefined && load !== undefined) this.load(key, load);
    return v;
  }

  /** Drop everything (aborting loads); keeps the cache usable. */
  clear(): void {
    for (const f of this.inflight.values()) f.ctl.abort();
    this.inflight.clear();
    this.queued.clear();
    this.order.length = 0;
    this.failed.clear();
    for (const e of this.entries.values()) this.opts.dispose?.(e.value);
    this.entries.clear();
    this.bytes = 0;
  }

  dispose(): void {
    this.clear();
    this.disposed = true;
  }

  private schedulePump(): void {
    if (this.pumpScheduled) return;
    this.pumpScheduled = true;
    queueMicrotask(this.pumpNow);
  }

  private pump(): void {
    const limit = this.opts.concurrency ?? 6;
    while (!this.disposed && this.inflight.size < limit && this.order.length > 0) {
      // Newest first.
      const key = this.order.pop() as number;
      const p = this.queued.get(key);
      if (p === undefined) continue; // started already, or dropped as stale
      this.queued.delete(key);
      if (p.seen < this.frame - 1) continue; // scrolled away before it started
      const ctl = new AbortController();
      this.inflight.set(key, { ctl, seen: p.seen });
      p.load(ctl.signal).then(
        (value) => this.settle(key, ctl, value, false),
        () => this.settle(key, ctl, null, true),
      );
    }
    if (this.queued.size === 0) this.order.length = 0;
  }

  private settle(key: number, ctl: AbortController, value: V | null, errored: boolean): void {
    if (this.inflight.get(key)?.ctl !== ctl) {
      // Cleared or aborted while loading: the result is stale.
      if (value !== null) this.opts.dispose?.(value);
      return;
    }
    this.inflight.delete(key);
    if (errored) this.failed.add(key);
    else if (value !== null) {
      const bytes = this.opts.sizeOf(value);
      this.entries.set(key, { value, bytes, used: ++this.clock });
      this.bytes += bytes;
      this.evict(key);
      this.opts.onLoaded();
    } else {
      this.failed.add(key); // "no tile": do not ask again until clear()
    }
    this.pump();
  }

  /** Past the budget: evict least recently used entries (never `keep`) down to `EVICT_TO`. */
  private evict(keep: number): void {
    if (this.bytes <= this.opts.maxBytes) return;
    const goal = this.opts.maxBytes * EVICT_TO;
    const oldest = Array.from(this.entries).sort((a, b) => a[1].used - b[1].used);
    for (const [k, e] of oldest) {
      if (this.bytes <= goal) break;
      if (k === keep) continue;
      this.entries.delete(k);
      this.bytes -= e.bytes;
      this.opts.dispose?.(e.value);
    }
  }
}

export interface MediaCacheOptions {
  readonly loaders: MediaLoaders;
  /** Called when a tile or peaks array arrives so the view can redraw. */
  readonly onLoaded: () => void;
  /** Filmstrip byte budget (default {@link FILMSTRIP_BYTES}, 256 MiB). */
  readonly filmstripBytes?: number;
  /** Waveform byte budget (default {@link WAVEFORM_BYTES}, 16 MiB). */
  readonly waveformBytes?: number;
}

// Key packing: ((media * SIZES + size) * BUCKETS + bucket) * TILES + tile < 2^53, so every key is
// an exact integer. Interned media ids and tile sizes past their slots reset the caches.
const MAX_MEDIA = 2 ** 15;
const MAX_SIZES = 32;
const BUCKETS = 64;
const BUCKET_OFFSET = 32;
/** Tiles per media and zoom bucket that have a key (tile indices at or past it draw nothing). */
export const MAX_TILE_INDEX = 2 ** 27;

const bucketSlot = (bucket: number): number =>
  Math.min(BUCKETS - 1, Math.max(0, bucket + BUCKET_OFFSET));

/** Both caches plus their loaders, and the key packing the renderer uses for them. */
export class MediaCaches {
  readonly filmstrip: LoadingCache<ImageBitmap>;
  readonly waveform: LoadingCache<WaveformPeaks>;
  private readonly mediaSlots = new Map<string, number>();
  private readonly sizeSlots = new Map<number, number>();

  constructor(readonly opts: MediaCacheOptions) {
    this.filmstrip = new LoadingCache<ImageBitmap>({
      maxBytes: opts.filmstripBytes ?? FILMSTRIP_BYTES,
      sizeOf: (b) => b.width * b.height * 4,
      dispose: (b) => b.close(),
      onLoaded: opts.onLoaded,
    });
    this.waveform = new LoadingCache<WaveformPeaks>({
      maxBytes: opts.waveformBytes ?? WAVEFORM_BYTES,
      sizeOf: (p) => p.min.byteLength + p.max.byteLength,
      onLoaded: opts.onLoaded,
      concurrency: 2,
    });
  }

  /** See {@link LoadingCache.beginFrame}. */
  beginFrame(full = true): void {
    this.filmstrip.beginFrame(full);
    this.waveform.beginFrame(full);
  }

  get hasFilmstripLoader(): boolean {
    return this.opts.loaders.filmstripTile !== undefined;
  }

  get hasWaveformLoader(): boolean {
    return this.opts.loaders.waveform !== undefined;
  }

  /** Small integer for a media id (stable until the caches reset). */
  mediaSlot(mediaId: string): number {
    let slot = this.mediaSlots.get(mediaId);
    if (slot === undefined) {
      if (this.mediaSlots.size >= MAX_MEDIA) this.reset();
      slot = this.mediaSlots.size;
      this.mediaSlots.set(mediaId, slot);
    }
    return slot;
  }

  /** Small integer for a tile size in CSS px (stable until the caches reset). */
  sizeSlot(width: number, height: number): number {
    const k = width * 65_536 + height;
    let slot = this.sizeSlots.get(k);
    if (slot === undefined) {
      if (this.sizeSlots.size >= MAX_SIZES) this.reset();
      slot = this.sizeSlots.size;
      this.sizeSlots.set(k, slot);
    }
    return slot;
  }

  /** Key of filmstrip tile `tile` (`0 <= tile < MAX_TILE_INDEX`) of a media, size and bucket. */
  tileKey(media: number, size: number, bucket: number, tile: number): number {
    return ((media * MAX_SIZES + size) * BUCKETS + bucketSlot(bucket)) * MAX_TILE_INDEX + tile;
  }

  /** Key of a media's peaks at a zoom bucket. */
  peaksKey(media: number, bucket: number): number {
    return media * BUCKETS + bucketSlot(bucket);
  }

  /** Queue the tile `req` under `key` (call only after `filmstrip.needsLoad(key)`). */
  requestTile(key: number, req: FilmstripRequest): void {
    const load = this.opts.loaders.filmstripTile;
    if (load) this.filmstrip.load(key, (signal) => load(req, signal));
  }

  /** Queue the peaks `req` under `key` (call only after `waveform.needsLoad(key)`). */
  requestPeaks(key: number, req: WaveformRequest): void {
    const load = this.opts.loaders.waveform;
    if (load) this.waveform.load(key, (signal) => load(req, signal));
  }

  /** Drop everything, including the interned ids. */
  reset(): void {
    this.filmstrip.clear();
    this.waveform.clear();
    this.mediaSlots.clear();
    this.sizeSlots.clear();
  }

  dispose(): void {
    this.filmstrip.dispose();
    this.waveform.dispose();
    this.mediaSlots.clear();
    this.sizeSlots.clear();
  }
}
