// SPDX-License-Identifier: GPL-3.0-or-later
// WP-1I.03 editor side: the media HTTP client, the cards' signed thumbnails and the timeline's
// filmstrip and waveform loaders, against a fake `fetch` (no network in tests).
import type { MediaId } from "@cutweave/protocol";
import { describe, expect, it, vi } from "vitest";
import { HttpThumbnailSource, POSTER_WIDTH, RESIGN_MARGIN_MS } from "./httpThumbnails.ts";
import { MediaHttp, MediaHttpError, mediaPath } from "./mediaHttp.ts";
import {
  coverPart,
  type FilmstripManifest,
  httpMediaLoaders,
  loadersForWindow,
  parsePeaks,
  tileIndex,
  waveformLod,
} from "./timelineLoaders.ts";

const ENGINE = { httpUrl: "http://127.0.0.1:4321/", token: "secret-ui-token" };
const HASH = "blake3-fast:abcd";
const SEC = 705_600_000;

interface Call {
  readonly url: string;
  readonly init: RequestInit | undefined;
}

function fakeFetch(answer: (url: string, init?: RequestInit) => Response | Promise<Response>) {
  const calls: Call[] = [];
  const fn = (async (url: string, init?: RequestInit) => {
    calls.push({ url, init });
    return answer(url, init);
  }) as typeof fetch;
  return { fn, calls };
}

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

const flush = () => new Promise<void>((r) => setTimeout(r, 0));

describe("MediaHttp", () => {
  it("sends the token in the Authorization header, never in the URL", async () => {
    const { fn, calls } = fakeFetch(() => new Response("x", { status: 200 }));
    const http = new MediaHttp({ connection: async () => ENGINE, fetch: fn });
    const signal = new AbortController().signal;
    await http.get(mediaPath("med_1", "thumb", { t: 0, w: 320, hash: HASH }), signal);
    expect(calls[0]?.url).toBe(
      "http://127.0.0.1:4321/media/med_1/thumb?t=0&w=320&hash=blake3-fast%3Aabcd",
    );
    expect(calls[0]?.url).not.toContain(ENGINE.token);
    expect(new Headers(calls[0]?.init?.headers).get("authorization")).toBe(
      `Bearer ${ENGINE.token}`,
    );
    expect(calls[0]?.init?.signal).toBe(signal);
  });

  it("rejects with the status and the engine's error code", async () => {
    const { fn } = fakeFetch(() => json(404, { error: { code: "not_ready", message: "later" } }));
    const http = new MediaHttp({ connection: async () => ENGINE, fetch: fn });
    await expect(http.get("/media/m/waveform")).rejects.toMatchObject({
      status: 404,
      code: "not_ready",
    });
    const offline = new MediaHttp({ connection: async () => null, fetch: fn });
    await expect(offline.get("/media/m/thumb")).rejects.toBeInstanceOf(MediaHttpError);
  });

  it("signs paths with one POST and returns absolute URLs with the expiry in ms", async () => {
    const { fn, calls } = fakeFetch(() =>
      json(200, { urls: ["/media/a/thumb?exp=5&sig=ab"], expires: 5 }),
    );
    const http = new MediaHttp({ connection: async () => ENGINE, fetch: fn });
    const signed = await http.sign(["/media/a/thumb"]);
    expect(signed).toEqual({
      urls: ["http://127.0.0.1:4321/media/a/thumb?exp=5&sig=ab"],
      expires: 5000,
    });
    expect(calls[0]?.url).toBe("http://127.0.0.1:4321/media/sign");
    expect(calls[0]?.init?.method).toBe("POST");
    expect(JSON.parse(String(calls[0]?.init?.body))).toEqual({ paths: ["/media/a/thumb"] });
  });
});

describe("HttpThumbnailSource", () => {
  function setup(hash: string | null = HASH, analysing?: (id: MediaId) => boolean) {
    let now = 1_000_000;
    const sign = vi.fn(async (paths: readonly string[]) => ({
      urls: paths.map((p) => `http://e${p}&exp=1&sig=00`),
      expires: now + 3_600_000,
    }));
    const onChange = vi.fn();
    const source = new HttpThumbnailSource({
      http: { sign },
      contentHash: () => hash,
      onChange,
      now: () => now,
      ...(analysing && { analysing }),
    });
    return {
      source,
      sign,
      onChange,
      advance: (ms: number) => {
        now += ms;
      },
    };
  }
  const req = (id: string, kind: "video" | "audio" | "image", indexed: number) => ({
    id: id as MediaId,
    kind,
    width: 160,
    indexed,
  });

  it("keeps the tile for sound, unindexed video and assets without a hash", async () => {
    const { source, sign } = setup();
    expect(source.thumbnailFor(req("a", "audio", 1))).toEqual({ kind: "placeholder" });
    expect(source.thumbnailFor(req("v", "video", 0))).toEqual({ kind: "placeholder" });
    expect(setup(null).source.thumbnailFor(req("v", "video", 1))).toEqual({
      kind: "placeholder",
    });
    await flush();
    expect(sign).not.toHaveBeenCalled();
  });

  it("a video analysed in an earlier session asks at once; one being analysed waits", async () => {
    // Reopening a project: no `media.indexed` arrives for assets indexed before.
    const busy = new Set(["v2"]);
    const { source, sign } = setup(HASH, (id) => busy.has(id));
    expect(source.thumbnailFor(req("v1", "video", 0))).toEqual({ kind: "placeholder" });
    expect(source.thumbnailFor(req("v2", "video", 0))).toEqual({ kind: "placeholder" });
    await flush();
    expect(sign).toHaveBeenCalledTimes(1);
    expect(sign.mock.calls[0]?.[0]).toEqual([
      `/media/v1/thumb?t=0&w=${POSTER_WIDTH}&hash=blake3-fast%3Aabcd`,
    ]);
    expect(source.thumbnailFor(req("v1", "video", 0)).kind).toBe("image");
    // Its analysis done, the other one asks too.
    busy.clear();
    source.thumbnailFor(req("v2", "video", 0));
    await flush();
    expect(sign).toHaveBeenCalledTimes(2);
  });

  it("signs the cached poster of every card in one call, then shows the images", async () => {
    const { source, sign, onChange } = setup();
    expect(source.thumbnailFor(req("v", "video", 1))).toEqual({ kind: "placeholder" });
    expect(source.thumbnailFor(req("i", "image", 0))).toEqual({ kind: "placeholder" });
    await flush();
    expect(sign).toHaveBeenCalledTimes(1);
    expect(sign.mock.calls[0]?.[0]).toEqual([
      `/media/v/thumb?t=0&w=${POSTER_WIDTH}&hash=blake3-fast%3Aabcd`,
      `/media/i/thumb?t=0&w=${POSTER_WIDTH}&hash=blake3-fast%3Aabcd`,
    ]);
    expect(onChange).toHaveBeenCalledTimes(1);
    const thumb = source.thumbnailFor(req("v", "video", 1));
    expect(thumb).toEqual({
      kind: "image",
      src: `http://e/media/v/thumb?t=0&w=${POSTER_WIDTH}&hash=blake3-fast%3Aabcd&exp=1&sig=00`,
    });
    // Asking again costs nothing.
    source.thumbnailFor(req("v", "video", 1));
    await flush();
    expect(sign).toHaveBeenCalledTimes(1);
  });

  it("signs a large bin in batches the engine accepts", async () => {
    const { source, sign } = setup();
    for (let i = 0; i < 300; i++) source.thumbnailFor(req(`v${i}`, "video", 1));
    await flush();
    expect(sign.mock.calls.map((c) => c[0].length)).toEqual([256, 44]);
    expect(source.thumbnailFor(req("v299", "video", 1)).kind).toBe("image");
  });

  it("re-signs before a URL expires and forgets every URL on reset", async () => {
    const { source, sign, advance, onChange } = setup();
    source.thumbnailFor(req("v", "video", 1));
    await flush();
    advance(3_600_000 - RESIGN_MARGIN_MS + 1);
    // Still shown while the new one is signed.
    expect(source.thumbnailFor(req("v", "video", 1)).kind).toBe("image");
    await flush();
    expect(sign).toHaveBeenCalledTimes(2);
    source.reset();
    expect(onChange).toHaveBeenCalled();
    expect(source.thumbnailFor(req("v", "video", 1))).toEqual({ kind: "placeholder" });
  });
});

describe("timeline loaders", () => {
  it("pick the coarsest waveform level that keeps the resolution", () => {
    // 48 kHz: level 0 = 187.5 peaks/s, level 1 = 46.9, level 2 = 11.7.
    expect(waveformLod(48_000, 200)).toBe(0);
    expect(waveformLod(48_000, 187)).toBe(0);
    expect(waveformLod(48_000, 40)).toBe(1);
    expect(waveformLod(48_000, 10)).toBe(2);
    expect(waveformLod(48_000, 0.000001)).toBe(11);
  });

  function peakData(lod: number, firstBin: number, bins: [number, number][]): ArrayBuffer {
    const buf = new ArrayBuffer(32 + 4 * bins.length);
    const v = new DataView(buf);
    for (const [i, c] of [..."CWPD"].entries()) v.setUint8(i, c.charCodeAt(0));
    v.setUint8(4, 1);
    v.setUint8(5, lod);
    v.setUint32(12, 48_000, true);
    v.setUint32(16, 256 * 4 ** lod, true);
    v.setBigUint64(20, BigInt(firstBin), true);
    v.setUint32(28, bins.length, true);
    bins.forEach(([lo, hi], i) => {
      v.setInt16(32 + 4 * i, lo, true);
      v.setInt16(34 + 4 * i, hi, true);
    });
    return buf;
  }

  it("parse the engine's peak data", () => {
    const peaks = parsePeaks(peakData(1, 2, [[-32767, 16384]]));
    expect(peaks.peaksPerSecond).toBeCloseTo(48_000 / 1024);
    expect(peaks.min.length).toBe(3);
    expect(peaks.min[2]).toBeCloseTo(-1);
    expect(peaks.max[2]).toBeCloseTo(0.5, 3);
    expect(peaks.max[0]).toBe(0);
    expect(() => parsePeaks(new ArrayBuffer(8))).toThrow();
  });

  const manifest: FilmstripManifest = {
    tile_w: 160,
    tile_h: 90,
    count: 100,
    columns: 51,
    start: 0,
    interval: SEC / 10,
  };

  it("map a time to its strip tile", () => {
    expect(tileIndex(manifest, 0)).toBe(0);
    expect(tileIndex(manifest, SEC / 10 - 1)).toBe(0);
    expect(tileIndex(manifest, SEC)).toBe(10);
    expect(tileIndex(manifest, 100 * SEC)).toBe(99);
    expect(tileIndex(manifest, -SEC)).toBe(0);
  });

  function loaderSetup(answer: (url: string) => Response) {
    const { fn, calls } = fakeFetch(answer);
    const http = new MediaHttp({ connection: async () => ENGINE, fetch: fn });
    const sprite = { width: 8160, height: 180, close: vi.fn() } as unknown as ImageBitmap;
    const decode = vi.fn(async () => sprite);
    const crop = vi.fn(
      async (_s: ImageBitmap, sx: number, sy: number, sw: number, sh: number) =>
        ({ width: sw, height: sh, sx, sy, close() {} }) as unknown as ImageBitmap,
    );
    const media = {
      hash: HASH as string | null,
      sampleRate: 48_000 as number | null,
      makingFilmstrip: false,
    };
    const loaders = httpMediaLoaders({
      http,
      media: () => media,
      decode,
      crop,
      pixelRatio: () => 2,
    });
    return { loaders, calls, decode, crop, media, sprite };
  }

  const stripResponse = () =>
    new Response(new Blob([new Uint8Array([0xff, 0xd8])]), {
      status: 200,
      headers: { "x-cutweave-filmstrip": JSON.stringify(manifest) },
    });

  const tile = (sourceTime: number, tileDuration: number) => ({
    mediaId: "med_1",
    sourceTime,
    tileDuration,
    width: 71,
    height: 40,
  });

  it("cut filmstrip tiles out of one sprite request per asset", async () => {
    const { loaders, calls, crop } = loaderSetup(stripResponse);
    const signal = new AbortController().signal;
    const [a, b] = await Promise.all([
      loaders.filmstripTile?.(tile(0, SEC), signal),
      loaders.filmstripTile?.(tile(12 * SEC, SEC), signal),
    ]);
    expect(calls).toHaveLength(1);
    expect(calls[0]?.url).toBe(
      "http://127.0.0.1:4321/media/med_1/filmstrip?w=160&count=100&hash=blake3-fast%3Aabcd",
    );
    // Tile centres: 0.5 s → tile 5; 12.5 s → tile 125, past the end: the last one (99).
    expect(a).toMatchObject({ sx: 5 * 160, sy: 0, width: 160, height: 90 });
    expect(b).toMatchObject({ sx: (99 % 51) * 160, sy: 90 });
    expect(crop).toHaveBeenCalledTimes(2);
  });

  it("cut the middle of an upright picture to the slot's shape, not stretch it", async () => {
    // A phone clip (#207): the engine's tiles are upright, 160×284, in a 71×40 slot.
    const tall = { ...manifest, tile_h: 284, columns: 51 };
    const { loaders, crop, sprite } = loaderSetup((url) =>
      url.includes("/filmstrip")
        ? new Response(new Blob([new Uint8Array([0xff, 0xd8])]), {
            status: 200,
            headers: { "x-cutweave-filmstrip": JSON.stringify(tall) },
          })
        : new Response("jpeg", { status: 200 }),
    );
    const signal = new AbortController().signal;
    const a = await loaders.filmstripTile?.(tile(0, SEC), signal);
    // The slot's shape at the tile's width: 160 × 90, from the middle of the 284 rows.
    expect(a).toMatchObject({ sx: 5 * 160, sy: 97, width: 160, height: 90 });
    // Its own frame (zoomed in): cropped the same way, and the whole picture released.
    crop.mockClear();
    const own = await loaders.filmstripTile?.(tile(SEC, SEC / 100), signal);
    // The fake decoder answers an 8160×180 picture: the middle 320×180 has the slot's shape.
    expect(own).toMatchObject({ sx: 3920, sy: 0, width: 320, height: 180 });
    expect(crop).toHaveBeenCalledTimes(1);
    expect(sprite.close).toHaveBeenCalledTimes(1);
  });

  it("find the part of a picture that fills a slot", () => {
    expect(coverPart(160, 90, 71, 40)).toEqual({ x: 0, y: 0, w: 160, h: 90 });
    expect(coverPart(160, 284, 160, 90)).toEqual({ x: 0, y: 97, w: 160, h: 90 });
    expect(coverPart(400, 90, 160, 90)).toEqual({ x: 120, y: 0, w: 160, h: 90 });
    expect(coverPart(160, 90, 0, 40)).toEqual({ x: 0, y: 0, w: 160, h: 90 });
  });

  it("ask for the tile's own frame when zoomed in past the strip, with its signal", async () => {
    const { loaders, calls } = loaderSetup((url) =>
      url.includes("/filmstrip") ? stripResponse() : new Response("jpeg", { status: 200 }),
    );
    const controller = new AbortController();
    await loaders.filmstripTile?.(tile(SEC, SEC / 100), controller.signal);
    expect(calls[1]?.url).toBe(
      `http://127.0.0.1:4321/media/med_1/thumb?t=${SEC + SEC / 200}&w=160&hash=blake3-fast%3Aabcd`,
    );
    expect(calls[1]?.init?.signal).toBe(controller.signal);
  });

  it("size a frame for the density of the window the canvas is in", async () => {
    const { loaders, calls } = loaderSetup((url) =>
      url.includes("/filmstrip") ? stripResponse() : new Response("jpeg", { status: 200 }),
    );
    let density = 3;
    const detached = loadersForWindow(loaders, () => density);
    await detached.filmstripTile?.(tile(SEC, SEC / 100), new AbortController().signal);
    // 71 CSS px at 3x, rounded up to 32: 224 (the main window's 2x would ask for 160).
    expect(calls[1]?.url).toContain("&w=224&");
    density = 1;
    await detached.filmstripTile?.(tile(2 * SEC, SEC / 100), new AbortController().signal);
    expect(calls[2]?.url).toContain("&w=96&");
  });

  it("show the placeholder for missing data until refreshed", async () => {
    let ready = false;
    const { loaders, calls } = loaderSetup((url) =>
      ready && url.includes("/filmstrip")
        ? stripResponse()
        : json(404, { error: { code: "not_ready" } }),
    );
    const signal = new AbortController().signal;
    expect(await loaders.filmstripTile?.(tile(0, SEC), signal)).toBeNull();
    expect(await loaders.filmstripTile?.(tile(0, SEC), signal)).toBeNull();
    expect(calls).toHaveLength(1);
    expect(await loaders.waveform?.({ mediaId: "med_1", peaksPerSecond: 50 }, signal)).toBeNull();
    ready = true;
    loaders.refresh();
    expect(await loaders.filmstripTile?.(tile(0, SEC), signal)).not.toBeNull();
  });

  it("load waveform peaks at the level the zoom needs, with the signal", async () => {
    const { loaders, calls } = loaderSetup(
      () => new Response(peakData(1, 0, [[-100, 100]]), { status: 200 }),
    );
    const controller = new AbortController();
    const peaks = await loaders.waveform?.(
      { mediaId: "med_1", peaksPerSecond: 40 },
      controller.signal,
    );
    expect(calls[0]?.url).toBe(
      "http://127.0.0.1:4321/media/med_1/waveform?lod=1&hash=blake3-fast%3Aabcd",
    );
    expect(calls[0]?.init?.signal).toBe(controller.signal);
    expect(peaks?.max.length).toBe(1);
  });

  it("wait for the import analysis instead of making the strip a second time", async () => {
    const { loaders, calls, media } = loaderSetup(stripResponse);
    media.makingFilmstrip = true;
    const signal = new AbortController().signal;
    expect(await loaders.filmstripTile?.(tile(0, SEC), signal)).toBeNull();
    expect(calls).toHaveLength(0);
    media.makingFilmstrip = false;
    expect(await loaders.filmstripTile?.(tile(0, SEC), signal)).not.toBeNull();
  });

  it("load nothing for an asset without a content hash", async () => {
    const { loaders, calls, media } = loaderSetup(stripResponse);
    media.hash = null;
    const signal = new AbortController().signal;
    expect(await loaders.filmstripTile?.(tile(0, SEC), signal)).toBeNull();
    expect(await loaders.waveform?.({ mediaId: "med_1", peaksPerSecond: 50 }, signal)).toBeNull();
    expect(calls).toHaveLength(0);
  });

  it("let an aborted tile request go without failing the asset", async () => {
    const { loaders } = loaderSetup((url) => {
      if (url.includes("/filmstrip")) return stripResponse();
      throw new DOMException("aborted", "AbortError");
    });
    const controller = new AbortController();
    controller.abort();
    await expect(
      loaders.filmstripTile?.(tile(SEC, SEC / 100), controller.signal),
    ).rejects.toThrow();
    // The strip is still there for the next tile.
    const next = await loaders.filmstripTile?.(tile(0, SEC), new AbortController().signal);
    expect(next).not.toBeNull();
  });
});
