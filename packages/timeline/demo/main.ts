// SPDX-License-Identifier: GPL-3.0-or-later
// Storybook-less timeline demo (WP-2A.09): the S1 stress project (5,000 clips on 20 tracks, 50,000
// keyframes, filmstrips of 200 distinct media, waveforms, markers) in a live `TimelineView` you
// can pan (wheel, Shift+wheel, middle drag) and zoom (Ctrl+wheel at the pointer, pinch, +/-).
// The probe line on top prints frames per second, frame and draw times, what was drawn, hit-test
// latency and an estimate of the heap allocated per frame; it also logs them every 2 s.
//
// `?sweep=10` runs a scripted 10 s pan, smooth-zoom and scroll sweep through the same calls the
// wheel handler makes, after `warmup` seconds (default 3), and leaves the result in
// `window.__demoResult` (the Playwright perf test reads it). `?media=0` turns the loaders off.
import "../../ui/src/tokens/tokens.css";
import { generateTimeline, syntheticLoaders, withMarkers } from "../src/fixtures.ts";
import { FLICKS_PER_SECOND as F } from "../src/time.ts";
import { TimelineView } from "../src/timeline-view.ts";

declare global {
  interface Window {
    __demoResult?: DemoResult;
    __demoError?: string;
  }
}

export interface DemoResult {
  readonly seconds: number;
  readonly frames: number;
  readonly fps: { readonly mean: number; readonly p5: number };
  readonly frameMs: { readonly p50: number; readonly p95: number; readonly max: number };
  readonly drawMs: { readonly p50: number; readonly p95: number; readonly max: number };
  readonly visibleItemsMax: number;
  readonly tilesMax: number;
  readonly hitTestMs: { readonly p50: number; readonly p95: number };
  /** Heap growth per frame between collections (Chromium with precise memory info), bytes. */
  readonly heapBytesPerFrame: number | null;
  readonly dataset: { readonly clips: number; readonly keyframes: number; readonly media: number };
}

const percentile = (xs: ArrayLike<number>, p: number): number => {
  const s = Array.from(xs).sort((a, b) => a - b);
  if (s.length === 0) return 0;
  return s[Math.min(s.length - 1, Math.max(0, Math.ceil((p / 100) * s.length) - 1))] ?? 0;
};
const round = (x: number, d = 2): number => Math.round(x * 10 ** d) / 10 ** d;

/** Heap in use, when the browser reports it (Chromium's `performance.memory`). */
const heapUsed = (): number | null =>
  (performance as unknown as { memory?: { usedJSHeapSize: number } }).memory?.usedJSHeapSize ??
  null;

async function main(): Promise<void> {
  const params = new URLSearchParams(location.search);
  const sweep = Number(params.get("sweep") ?? 0);
  const warmup = Number(params.get("warmup") ?? 3);
  const media = params.get("media") !== "0";
  const canvas = document.getElementById("tl") as HTMLCanvasElement;
  const probe = document.getElementById("probe") as HTMLElement;
  document.body.classList.add("body");
  await document.fonts.ready;

  const model = withMarkers(generateTimeline({ mediaCount: 200 }));
  const drawMs: number[] = [];
  const view = new TimelineView(canvas, {
    ...(media && { loaders: syntheticLoaders() }),
    // Time each frame the view draws (its own requestAnimationFrame loop).
    requestFrame: (cb) =>
      requestAnimationFrame(() => {
        const a = performance.now();
        cb();
        drawMs.push(performance.now() - a);
      }),
    cancelFrame: (id) => cancelAnimationFrame(id),
  });
  view.bind();
  view.setModel(model);
  view.setViewState({ pxPerSec: 20, scrollX: 600 * F, playhead: 610 * F });
  view.onPointer((e) => {
    if (e.type === "down" && e.hit.itemId !== null) {
      view.setViewState({ selection: new Set([e.hit.itemId]) });
    }
  });
  canvas.focus();

  // Probe: frame intervals from our own rAF loop, hit-test latency from pointer moves.
  const frameMs: number[] = [];
  const hitMs: number[] = [];
  let visibleMax = 0;
  let tilesMax = 0;
  let heapGrowth = 0;
  let heapFrames = 0;
  let lastHeap = heapUsed();
  canvas.addEventListener("pointermove", (e) => {
    const r = canvas.getBoundingClientRect();
    const a = performance.now();
    view.hitTest(e.clientX - r.left, e.clientY - r.top);
    hitMs.push(performance.now() - a);
  });
  let last = 0;
  let recording = sweep === 0;
  const onFrame = (now: number): void => {
    if (last !== 0 && recording) {
      frameMs.push(now - last);
      visibleMax = Math.max(visibleMax, view.stats.visibleItems);
      tilesMax = Math.max(tilesMax, view.stats.tiles);
      const heap = heapUsed();
      if (heap !== null && lastHeap !== null) {
        // Growth only: a drop is a collection, not a frame that freed memory.
        if (heap > lastHeap) heapGrowth += heap - lastHeap;
        heapFrames++;
      }
      lastHeap = heap;
    }
    last = now;
    requestAnimationFrame(onFrame);
  };
  requestAnimationFrame(onFrame);

  const report = (): DemoResult => {
    const fm = frameMs;
    const mean = fm.reduce((p, q) => p + q, 0) / Math.max(1, fm.length);
    return {
      seconds: sweep,
      frames: fm.length,
      fps: { mean: round(1000 / mean), p5: round(1000 / percentile(fm, 95)) },
      frameMs: {
        p50: round(percentile(fm, 50)),
        p95: round(percentile(fm, 95)),
        max: round(Math.max(0, ...fm)),
      },
      drawMs: {
        p50: round(percentile(drawMs, 50)),
        p95: round(percentile(drawMs, 95)),
        max: round(Math.max(0, ...drawMs)),
      },
      visibleItemsMax: visibleMax,
      tilesMax,
      hitTestMs: { p50: round(percentile(hitMs, 50), 4), p95: round(percentile(hitMs, 95), 4) },
      heapBytesPerFrame: heapFrames > 0 ? Math.round(heapGrowth / heapFrames) : null,
      dataset: {
        clips: model.items.length,
        keyframes: model.items.reduce((n, i) => n + (i.keyframes?.length ?? 0), 0),
        media: new Set(model.items.map((i) => i.thumbnails?.mediaId).filter(Boolean)).size,
      },
    };
  };

  const show = (r: DemoResult, log: boolean): void => {
    const text =
      `fps ${r.fps.mean} (p5 ${r.fps.p5})  frame p95 ${r.frameMs.p95} ms  draw p95 ${r.drawMs.p95} ms  ` +
      `visible ${view.stats.visibleItems} clips  tiles ${view.stats.tiles} (+${view.stats.fallbackTiles} fallback, ` +
      `${view.stats.placeholders} pending)  hit-test p95 ${round(r.hitTestMs.p95 * 1000, 1)} us  ` +
      `heap +${r.heapBytesPerFrame ?? "?"} B/frame`;
    probe.textContent = text;
    if (log) console.log(`[timeline demo] ${text}`);
  };

  if (sweep === 0) {
    // Interactive: a rolling window of the last second.
    let n = 0;
    setInterval(() => {
      if (frameMs.length > 120) frameMs.splice(0, frameMs.length - 120);
      if (drawMs.length > 120) drawMs.splice(0, drawMs.length - 120);
      if (hitMs.length > 500) hitMs.splice(0, hitMs.length - 500);
      show(report(), ++n % 4 === 0);
    }, 500);
    return;
  }

  // Scripted sweep: per frame a horizontal pan, a smooth-zoom notch every 8 frames at a moving
  // pointer (zooming in, then out, between whole-project and a few clips per screen), and a slow
  // vertical scroll: the calls the wheel handler makes.
  const width = canvas.getBoundingClientRect().width;
  const end = Math.max(...model.items.map((i) => i.start + i.duration));
  let frame = 0;
  let zoomIn = true;
  let dir = 1;
  const t0 = performance.now();
  await new Promise<void>((resolve) => {
    const step = (): void => {
      const elapsed = (performance.now() - t0) / 1000;
      if (elapsed > warmup + sweep) {
        resolve();
        return;
      }
      if (!recording && elapsed > warmup) {
        recording = true;
        frameMs.length = 0;
        drawMs.length = 0;
        heapGrowth = 0;
        heapFrames = 0;
      }
      frame++;
      const st = view.getViewState();
      if (st.scrollX > end * 0.9) dir = -1;
      else if (st.scrollX <= 0) dir = 1;
      view.scrollBy(dir * 24, frame % 120 < 60 ? 6 : -6);
      if (frame % 8 === 0) {
        if (st.pxPerSec > 250) zoomIn = false;
        else if (st.pxPerSec < 1) zoomIn = true;
        const anchor = 200 + ((frame * 37) % Math.max(1, width - 260));
        view.zoomSmooth(zoomIn ? 1.5 : 1 / 1.5, anchor);
      }
      requestAnimationFrame(step);
    };
    requestAnimationFrame(step);
  });

  // Hit-test latency at random points and zoom levels.
  let seed = 12345;
  const rnd = (): number => {
    seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0;
    return seed / 4294967296;
  };
  const rect = canvas.getBoundingClientRect();
  for (let i = 0; i < 5000; i++) {
    view.setViewState({
      pxPerSec: Math.exp(Math.log(0.5) + rnd() * (Math.log(300) - Math.log(0.5))),
      scrollX: Math.round(rnd() * end),
    });
    const a = performance.now();
    view.hitTest(rnd() * rect.width, rnd() * rect.height);
    hitMs.push(performance.now() - a);
  }
  const result = report();
  show(result, true);
  window.__demoResult = result;
}

main().catch((e: unknown) => {
  window.__demoError = e instanceof Error ? `${e.message}\n${e.stack ?? ""}` : String(e);
});
