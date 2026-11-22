// SPDX-License-Identifier: GPL-3.0-or-later
// Pluggable media loaders and the bitmap/peaks caches in front of them (arch §16: thumbnails and
// waveforms come from the HTTP media endpoints as cached bitmaps). The package never fetches by
// itself: the host passes loaders (typically `fetch` against `/media/{id}/filmstrip` and
// `/media/{id}/waveform` of the local engine). Tests pass fakes. No network in tests.

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

interface Pending<V> {
  load: (signal: AbortSignal) => Promise<V | null>;
  seen: number;
}

/**
 * LRU cache with a byte budget, de-duplicated async loads, bounded concurrency and stale-request
 * dropping: `get` marks a key as wanted in the current frame (`beginFrame`), and queued loads that
 * were not wanted in the previous frame are discarded, so scrubbing and zooming never build a
 * backlog of tiles that scrolled away. Newest requests load first.
 */
export class LoadingCache<V> {
  private readonly entries = new Map<string, { value: V; bytes: number }>();
  private readonly queued = new Map<string, Pending<V>>();
  private readonly inflight = new Map<string, AbortController>();
  private readonly failed = new Set<string>();
  private bytes = 0;
  private frame = 0;
  private disposed = false;

  constructor(private readonly opts: LoadingCacheOptions<V>) {}

  get byteSize(): number {
    return this.bytes;
  }

  get size(): number {
    return this.entries.size;
  }

  get pending(): number {
    return this.queued.size + this.inflight.size;
  }

  /** Call once per rendered frame, before the frame's `get`s. */
  beginFrame(): void {
    this.frame++;
  }

  /** Cached value (refreshing its recency) or undefined; starts a load on a miss. */
  get(key: string, load: ((signal: AbortSignal) => Promise<V | null>) | undefined): V | undefined {
    const hit = this.entries.get(key);
    if (hit) {
      this.entries.delete(key);
      this.entries.set(key, hit);
      return hit.value;
    }
    if (!load || this.disposed || this.failed.has(key) || this.inflight.has(key)) return undefined;
    const q = this.queued.get(key);
    if (q) {
      q.seen = this.frame;
      return undefined;
    }
    this.queued.set(key, { load, seen: this.frame });
    queueMicrotask(() => this.pump());
    return undefined;
  }

  has(key: string): boolean {
    return this.entries.has(key);
  }

  /** Drop everything (aborting loads); keeps the cache usable. */
  clear(): void {
    for (const c of this.inflight.values()) c.abort();
    this.inflight.clear();
    this.queued.clear();
    this.failed.clear();
    for (const e of this.entries.values()) this.opts.dispose?.(e.value);
    this.entries.clear();
    this.bytes = 0;
  }

  dispose(): void {
    this.clear();
    this.disposed = true;
  }

  private pump(): void {
    const limit = this.opts.concurrency ?? 6;
    while (!this.disposed && this.inflight.size < limit && this.queued.size > 0) {
      // Newest first (Map preserves insertion order).
      const key = Array.from(this.queued.keys()).pop() as string;
      const p = this.queued.get(key) as Pending<V>;
      this.queued.delete(key);
      if (p.seen < this.frame - 1) continue; // scrolled away before it started
      const ctl = new AbortController();
      this.inflight.set(key, ctl);
      p.load(ctl.signal).then(
        (value) => this.settle(key, ctl, value, false),
        () => this.settle(key, ctl, null, true),
      );
    }
  }

  private settle(key: string, ctl: AbortController, value: V | null, errored: boolean): void {
    if (this.inflight.get(key) !== ctl) {
      // Cleared while loading: the result is stale.
      if (value !== null) this.opts.dispose?.(value);
      return;
    }
    this.inflight.delete(key);
    if (errored) this.failed.add(key);
    else if (value !== null) {
      const bytes = this.opts.sizeOf(value);
      this.entries.set(key, { value, bytes });
      this.bytes += bytes;
      this.evict(key);
      this.opts.onLoaded();
    } else {
      this.failed.add(key); // "no tile": do not ask again until clear()
    }
    this.pump();
  }

  private evict(keep: string): void {
    for (const [k, e] of this.entries) {
      if (this.bytes <= this.opts.maxBytes) break;
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
  /** Filmstrip byte budget (default 64 MiB). */
  readonly filmstripBytes?: number;
  /** Waveform byte budget (default 16 MiB). */
  readonly waveformBytes?: number;
}

/** Both caches plus their loaders. */
export class MediaCaches {
  readonly filmstrip: LoadingCache<ImageBitmap>;
  readonly waveform: LoadingCache<WaveformPeaks>;

  constructor(readonly opts: MediaCacheOptions) {
    this.filmstrip = new LoadingCache<ImageBitmap>({
      maxBytes: opts.filmstripBytes ?? 64 * 1024 * 1024,
      sizeOf: (b) => b.width * b.height * 4,
      dispose: (b) => b.close(),
      onLoaded: opts.onLoaded,
    });
    this.waveform = new LoadingCache<WaveformPeaks>({
      maxBytes: opts.waveformBytes ?? 16 * 1024 * 1024,
      sizeOf: (p) => p.min.byteLength + p.max.byteLength,
      onLoaded: opts.onLoaded,
      concurrency: 2,
    });
  }

  beginFrame(): void {
    this.filmstrip.beginFrame();
    this.waveform.beginFrame();
  }

  get hasFilmstripLoader(): boolean {
    return this.opts.loaders.filmstripTile !== undefined;
  }

  get hasWaveformLoader(): boolean {
    return this.opts.loaders.waveform !== undefined;
  }

  tile(key: string, req: FilmstripRequest): ImageBitmap | undefined {
    const load = this.opts.loaders.filmstripTile;
    return this.filmstrip.get(key, load ? (signal) => load(req, signal) : undefined);
  }

  peaks(key: string, req: WaveformRequest): WaveformPeaks | undefined {
    const load = this.opts.loaders.waveform;
    return this.waveform.get(key, load ? (signal) => load(req, signal) : undefined);
  }

  dispose(): void {
    this.filmstrip.dispose();
    this.waveform.dispose();
  }
}
