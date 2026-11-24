// SPDX-License-Identifier: GPL-3.0-or-later
// The JPEG fallback presenter (arch §10 fallback, WP-1M.04) against a fake `/media/frame`:
// polling at most 15 fps, frames presented by pts against the playhead clock, latest-wins
// seeks while stopped, and back-off on errors.
import { describe, expect, it } from "vitest";
import {
  type DecodedFrame,
  FALLBACK_MAX_FPS,
  FrameFetchError,
  type FrameFetcher,
  type FrameRequest,
  frameUrl,
  httpFrameFetcher,
  JpegPreview,
  type PreviewTicker,
} from "./jpegPreview.ts";
import type { PlayheadSample } from "./presenter.ts";

const FLICKS = 705_600_000;
const F30 = FLICKS / 30;

interface Pending {
  req: FrameRequest;
  resolve: (f: DecodedFrame) => void;
  reject: (e: unknown) => void;
}

/** A fetcher whose requests the test answers by hand. */
function fakeFetcher() {
  const pending: Pending[] = [];
  const closed: number[] = [];
  const fetcher: FrameFetcher = {
    fetchFrame: (req) =>
      new Promise<DecodedFrame>((resolve, reject) => {
        pending.push({ req, resolve, reject });
        req.signal.addEventListener("abort", () =>
          reject(new DOMException("aborted", "AbortError")),
        );
      }),
  };
  const frame = (pts: number): DecodedFrame => ({
    pts,
    width: 4,
    height: 2,
    image: { pts } as unknown as CanvasImageSource,
    close: () => closed.push(pts),
  });
  return {
    fetcher,
    pending,
    closed,
    /** Answers the oldest open request with the frame it asked for. */
    async answer(): Promise<number> {
      const p = pending.shift();
      if (p === undefined) throw new Error("no request");
      p.resolve(frame(p.req.t));
      await Promise.resolve();
      await Promise.resolve();
      return p.req.t;
    },
    async fail(error: unknown): Promise<void> {
      pending.shift()?.reject(error);
      await Promise.resolve();
      await Promise.resolve();
    },
  };
}

/** Manual rAF and clock. */
function manualTicker() {
  let pending: (() => void) | null = null;
  let t = 0;
  const ticker: PreviewTicker = {
    now: () => t,
    schedule: (cb) => {
      pending = cb;
      return 1;
    },
    cancel: () => {
      pending = null;
    },
  };
  return {
    ticker,
    /** Advances `ms` and runs one frame. */
    step(ms = 1000 / 60) {
      t += ms;
      const cb = pending;
      pending = null;
      cb?.();
    },
    get now() {
      return t;
    },
  };
}

function setup(initial: Partial<PlayheadSample> = {}) {
  const f = fakeFetcher();
  const m = manualTicker();
  const drawn: number[] = [];
  let playhead: PlayheadSample = { t: 0, rate: 0, playing: false, at: 0, ...initial };
  const preview = new JpegPreview({
    fetcher: f.fetcher,
    surface: {
      draw: (frame) => drawn.push(frame.pts),
      clear: () => drawn.push(-1),
    },
    ticker: m.ticker,
    playhead: () => playhead,
    size: () => ({ width: 640, height: 360 }),
    frameRate: () => ({ num: 30, den: 1 }),
  });
  return {
    f,
    m,
    drawn,
    preview,
    setPlayhead(next: Partial<PlayheadSample>) {
      playhead = { ...playhead, at: m.now, ...next };
    },
  };
}

describe("JpegPreview", () => {
  it("stopped: fetches the playhead frame once and draws it when it arrives", async () => {
    const s = setup({ t: 5 * F30 });
    s.preview.start();
    s.m.step();
    expect(s.f.pending.map((p) => p.req)).toMatchObject([{ t: 5 * F30, width: 640, height: 360 }]);
    s.m.step();
    s.m.step();
    expect(s.f.pending).toHaveLength(1); // nothing new to ask while the playhead holds
    await s.f.answer();
    s.m.step();
    expect(s.drawn).toEqual([5 * F30]);
    s.m.step();
    expect(s.drawn).toEqual([5 * F30]);
    expect(s.preview.stats()).toMatchObject({ framesFetched: 1, framesPresented: 1, errors: 0 });
  });

  it("stopped: a newer seek aborts the request in flight (latest wins)", async () => {
    const s = setup({ t: 0 });
    s.preview.start();
    s.m.step();
    s.setPlayhead({ t: 30 * F30 });
    s.m.step(100);
    // The first request was aborted; the newest target is the only one answered.
    await s.f.fail(new DOMException("aborted", "AbortError"));
    expect(s.f.pending.map((p) => p.req.t)).toEqual([30 * F30]);
    await s.f.answer();
    s.m.step();
    expect(s.drawn).toEqual([30 * F30]);
    expect(s.preview.stats().errors).toBe(0);
  });

  it("snaps requests to the frame grid", () => {
    const s = setup({ t: 5 * F30 + 123 });
    s.preview.start();
    s.m.step();
    expect(s.f.pending[0]?.req.t).toBe(5 * F30);
  });

  it(`playing: polls at most ${FALLBACK_MAX_FPS} fps and presents each frame when its pts is due`, async () => {
    const s = setup({ t: 0, rate: 1, playing: true, at: 0 });
    s.preview.start();
    let requests = 0;
    const seen = new Set<number>();
    // Two seconds at 60 Hz; every request is answered at once.
    for (let i = 0; i < 120; i++) {
      s.m.step();
      while (s.f.pending.length > 0) {
        requests++;
        const t = await s.f.answer();
        seen.add(t);
      }
    }
    expect(requests).toBeLessThanOrEqual(2 * FALLBACK_MAX_FPS + 1);
    expect(requests).toBeGreaterThanOrEqual(2 * FALLBACK_MAX_FPS - 2);
    // Requests run ahead of the clock; each one is drawn no earlier than its pts.
    expect(s.drawn.length).toBeGreaterThanOrEqual(25);
    for (let i = 1; i < s.drawn.length; i++) {
      expect(s.drawn[i] ?? 0).toBeGreaterThan(s.drawn[i - 1] ?? 0);
    }
    const stats = s.preview.stats();
    expect(stats.framesPresented).toBe(s.drawn.length);
    expect(stats.lastPts).toBeLessThanOrEqual(2 * FLICKS);
  });

  it("playing: never draws a frame before its pts and drops (closes) passed frames", async () => {
    const s = setup({ t: 0, rate: 1, playing: true, at: 0 });
    s.preview.start();
    s.m.step(); // asks for a frame ahead of the clock
    const asked = s.f.pending[0]?.req.t ?? 0;
    expect(asked).toBeGreaterThan(0);
    await s.f.answer();
    s.m.step(1); // the clock has not reached it yet
    expect(s.drawn).toEqual([]);
    while (s.m.now / 1000 < asked / FLICKS) s.m.step();
    s.m.step();
    expect(s.drawn).toEqual([asked]);
    s.preview.stop();
    expect(s.f.closed).toContain(asked);
  });

  it("reverse: requests run backwards from the playhead", () => {
    const s = setup({ t: 60 * F30, rate: -1, playing: true, at: 0 });
    s.preview.start();
    s.m.step();
    expect(s.f.pending[0]?.req.t).toBeLessThan(60 * F30);
  });

  it("an error is counted and retried after a back-off; an abort is not an error", async () => {
    const s = setup({ t: 0 });
    s.preview.start();
    s.m.step();
    await s.f.fail(new FrameFetchError(404, "no frame"));
    expect(s.preview.stats()).toMatchObject({ errors: 1, lastError: "no frame" });
    s.m.step(100);
    expect(s.f.pending).toHaveLength(0);
    s.m.step(1000);
    expect(s.f.pending).toHaveLength(1);
  });

  it("invalidate() fetches the current frame again (document or size changed)", async () => {
    const s = setup({ t: 2 * F30 });
    s.preview.start();
    s.m.step();
    await s.f.answer();
    s.m.step();
    s.preview.invalidate();
    s.m.step();
    expect(s.f.pending.map((p) => p.req.t)).toEqual([2 * F30]);
  });

  it("stop() cancels the loop and closes held frames", async () => {
    const s = setup({ t: 0 });
    s.preview.start();
    s.m.step();
    await s.f.answer();
    s.m.step();
    s.preview.stop();
    s.m.step();
    expect(s.f.pending).toHaveLength(0);
    expect(s.f.closed).toEqual([0]);
  });
});

describe("httpFrameFetcher", () => {
  it("asks /media/frame with the bearer token in a header, never in the URL", async () => {
    const calls: { url: string; init: RequestInit | undefined }[] = [];
    const fetchImpl = async (url: string, init?: RequestInit) => {
      calls.push({ url, init });
      return new Response(new Blob([new Uint8Array([1, 2, 3])], { type: "image/jpeg" }), {
        status: 200,
        headers: { "content-type": "image/jpeg" },
      });
    };
    const fetcher = httpFrameFetcher({
      connection: async () => ({ httpUrl: "http://127.0.0.1:4321", token: "secret" }),
      sequence: () => "seq_1",
      fetch: fetchImpl as typeof fetch,
      decode: async () => ({ width: 64, height: 36, close() {} }) as unknown as ImageBitmap,
    });
    const frame = await fetcher.fetchFrame({
      t: 5 * F30,
      width: 640,
      height: 360,
      signal: new AbortController().signal,
    });
    expect(frame).toMatchObject({ pts: 5 * F30, width: 64, height: 36 });
    expect(calls[0]?.url).toBe(
      `http://127.0.0.1:4321/media/frame?seq=seq_1&t=${5 * F30}&w=640&h=360&fmt=jpeg`,
    );
    expect(calls[0]?.url).not.toContain("secret");
    expect(new Headers(calls[0]?.init?.headers).get("authorization")).toBe("Bearer secret");
  });

  it("rejects with the status for a non-image answer, and without a connection", async () => {
    const fetcher = httpFrameFetcher({
      connection: async () => ({ httpUrl: "http://127.0.0.1:1", token: "t" }),
      sequence: () => null,
      fetch: (async () => new Response("{}", { status: 404 })) as typeof fetch,
      decode: async () => ({}) as ImageBitmap,
    });
    const req = { t: 0, width: 2, height: 2, signal: new AbortController().signal };
    await expect(fetcher.fetchFrame(req)).rejects.toMatchObject({ status: 404 });
    const offline = httpFrameFetcher({
      connection: async () => null,
      sequence: () => null,
      fetch: fetch,
      decode: async () => ({}) as ImageBitmap,
    });
    await expect(offline.fetchFrame(req)).rejects.toBeInstanceOf(FrameFetchError);
  });

  it("builds the frame URL without a sequence when none is known", () => {
    expect(frameUrl("http://h:1", null, { t: 1, width: 2, height: 4 })).toBe(
      "http://h:1/media/frame?t=1&w=2&h=4&fmt=jpeg",
    );
  });
});
