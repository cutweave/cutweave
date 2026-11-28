// SPDX-License-Identifier: GPL-3.0-or-later
// Browser side of the timeline benchmark: a scripted pan/zoom/scroll sweep over the 5,000-clip /
// 50,000-keyframe S1 project with synthetic filmstrip and waveform loaders, driving the same
// `TimelineView` API the wheel and drag handlers use. Frame time is the rAF delta; draw time is
// the wall time of `view.flush()`. `run.mjs` loads this page in headless Chromium and prints the
// result. URL params: `seconds` (default 30); `lanes=1` opens the keyframe lanes of every clip
// (WP-2C.05: two lanes each, the clip's 10 keys in both), the worst case for the lanes;
// `fixture=1` reads the model and the lanes of a real project from `fixture.json` instead (the
// P-5K benchmark project, written by `packages/editor/src/timeline/keyframes.p5k.test.ts`).
import "../../ui/src/tokens/tokens.css";
import { generateTimeline, syntheticLoaders } from "../src/fixtures.ts";
import type { ItemLanes } from "../src/keyframes/lanes.ts";
import type { TimelineModel } from "../src/model.ts";
import { FLICKS_PER_SECOND as F } from "../src/time.ts";
import { TimelineView } from "../src/timeline-view.ts";

interface Stats {
  p50: number;
  p95: number;
  p99: number;
  max: number;
}

declare global {
  interface Window {
    __benchResult?: unknown;
    __benchError?: string;
  }
}

const ZOOM_PERIOD_S = 12;
const PAN_PERIOD_S = 17;
const SCROLL_PERIOD_S = 9;
/** Whole project in view .. a few clips per screen. */
const MIN_PPS = 0.5;
const MAX_PPS = 300;

const percentile = (sorted: readonly number[], p: number): number => {
  if (sorted.length === 0) return 0;
  const i = Math.min(sorted.length - 1, Math.max(0, Math.ceil((p / 100) * sorted.length) - 1));
  return sorted[i] ?? 0;
};
const round = (x: number, d = 2): number => Math.round(x * 10 ** d) / 10 ** d;
const stats = (xs: number[]): Stats => {
  const s = xs.slice().sort((a, b) => a - b);
  return {
    p50: round(percentile(s, 50)),
    p95: round(percentile(s, 95)),
    p99: round(percentile(s, 99)),
    max: round(s[s.length - 1] ?? 0),
  };
};

async function main(): Promise<void> {
  const params = new URLSearchParams(location.search);
  const seconds = Number(params.get("seconds") ?? 30);
  const media = params.get("media") !== "0";
  const warmup = Number(params.get("warmup") ?? 12);
  const canvas = document.getElementById("tl") as HTMLCanvasElement;
  document.body.classList.add("body");
  await document.fonts.ready;

  const fixture =
    params.get("fixture") === "1"
      ? ((await (await fetch("./fixture.json")).json()) as {
          model: TimelineModel;
          lanes: ItemLanes[];
        })
      : null;
  const model = fixture?.model ?? generateTimeline();
  // The sweep flushes by hand each frame to time the draw; the view's own scheduler stays idle.
  const view = new TimelineView(canvas, {
    ...(media && { loaders: syntheticLoaders() }),
    // Large budgets so the measured sweep never re-fetches (the product default is 64 MiB / 16 MiB).
    filmstripBytes: 512 * 1024 * 1024,
    waveformBytes: 256 * 1024 * 1024,
    requestFrame: () => 1,
    cancelFrame: () => {},
  });
  const dpr = window.devicePixelRatio || 1;
  view.resize(window.innerWidth, window.innerHeight, dpr);
  const buildT0 = performance.now();
  view.setModel(model);
  const modelBuildMs = performance.now() - buildT0;
  const withLanes = params.get("lanes") === "1";
  let lanesBuildMs = 0;
  if (withLanes) {
    const lanes: ItemLanes[] =
      fixture?.lanes ??
      model.items.map((item) => {
        const times = Float64Array.from(item.keyframes ?? []);
        const glyphs = Uint8Array.from(times, (_, i) => i % 4);
        return {
          itemId: item.id,
          lanes: [
            { prop: "opacity", label: "Opacity", times, glyphs, selected: [0] },
            { prop: "transform.position", label: "Position", times, glyphs },
          ],
        };
      });
    const t0 = performance.now();
    view.setKeyframeLanes(lanes);
    lanesBuildMs = performance.now() - t0;
  }
  const end = Math.max(...model.items.map((i) => i.start + i.duration));
  const laneW = window.innerWidth - 4 * 48;
  // With every lane open each track is two rows taller.
  const maxScrollY = 20 * (withLanes ? 108 : 60);

  const frameMs: number[] = [];
  const drawMs: number[] = [];
  const visible: number[] = [];
  const drawCalls: number[] = [];
  let labelsMax = 0;
  let tilesMax = 0;
  let keyframesMax = 0;

  const pose = (tSec: number): void => {
    const z = 0.5 - 0.5 * Math.cos((2 * Math.PI * tSec) / ZOOM_PERIOD_S);
    const pps = Math.exp(Math.log(MIN_PPS) + z * (Math.log(MAX_PPS) - Math.log(MIN_PPS)));
    const centre = end / 2 + (end / 2.2) * Math.sin((2 * Math.PI * tSec) / PAN_PERIOD_S);
    const scrollX = Math.round(centre - (laneW / pps / 2) * F);
    const scrollY = maxScrollY * (0.5 - 0.5 * Math.cos((2 * Math.PI * tSec) / SCROLL_PERIOD_S));
    view.setViewState({ pxPerSec: pps, scrollX, scrollY, playhead: Math.round(centre) });
  };

  const sweep = (duration: number, record: boolean): Promise<void> =>
    new Promise((resolve) => {
      let last = 0;
      let start = 0;
      const step = (now: number): void => {
        if (start === 0) start = now;
        const t = (now - start) / 1000;
        if (t > duration) {
          resolve();
          return;
        }
        pose(t);
        const d0 = performance.now();
        view.flush();
        const d1 = performance.now();
        if (record && last !== 0) {
          frameMs.push(now - last);
          drawMs.push(d1 - d0);
          visible.push(view.stats.visibleItems);
          drawCalls.push(view.stats.drawCalls);
          labelsMax = Math.max(labelsMax, view.stats.labels);
          tilesMax = Math.max(tilesMax, view.stats.tiles);
          keyframesMax = Math.max(keyframesMax, view.stats.visibleKeyframes);
        }
        last = now;
        requestAnimationFrame(step);
      };
      requestAnimationFrame(step);
    });

  // Warm-up sweep (untimed) fills the media caches, so the measured sweep times rendering, not the
  // synthetic loaders (which run on the main thread here; real loaders are network and decode).
  if (media) await sweep(warmup, false);
  await sweep(seconds, true);

  // Hit-test latency at random points and zoom levels (worst case included).
  let seed = 12345;
  const rnd = (): number => {
    seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0;
    return seed / 4294967296;
  };
  const hitMs: number[] = [];
  let hits = 0;
  const N = 20_000;
  for (let i = 0; i < N; i++) {
    const pps = Math.exp(Math.log(MIN_PPS) + rnd() * (Math.log(MAX_PPS) - Math.log(MIN_PPS)));
    view.setViewState({
      pxPerSec: pps,
      scrollX: Math.round(rnd() * end),
      scrollY: rnd() * maxScrollY,
    });
    const x = rnd() * window.innerWidth;
    const y = rnd() * window.innerHeight;
    const a = performance.now();
    const h = view.hitTest(x, y);
    hitMs.push(performance.now() - a);
    if (h.itemId) hits++;
  }

  const fm = frameMs.slice(1);
  const sf = fm.slice().sort((a, b) => a - b);
  const mem = (performance as unknown as { memory?: { usedJSHeapSize: number } }).memory;
  window.__benchResult = {
    seconds,
    viewport: { width: window.innerWidth, height: window.innerHeight, dpr },
    dataset: {
      clips: model.items.length,
      tracks: model.tracks.length,
      keyframes: withLanes
        ? view
            .getKeyframeLanes()
            .reduce((n, g) => n + g.lanes.reduce((m, l) => m + l.times.length, 0), 0)
        : model.items.reduce((n, i) => n + (i.keyframes?.length ?? 0), 0),
    },
    modelBuildMs: round(modelBuildMs),
    fixture: fixture !== null,
    lanes: withLanes
      ? { open: view.getKeyframeLanes().length, buildMs: round(lanesBuildMs) }
      : null,
    frames: fm.length,
    fps: {
      mean: round(1000 / (fm.reduce((p, q) => p + q, 0) / fm.length)),
      p50: round(1000 / percentile(sf, 50)),
      // p5 of the frame *rate* is the rate at the p95 of frame time.
      p5: round(1000 / percentile(sf, 95)),
    },
    frameMs: stats(fm),
    drawMs: stats(drawMs.slice(1)),
    visible: {
      itemsMax: Math.max(...visible),
      itemsP50: percentile(
        visible.slice().sort((a, b) => a - b),
        50,
      ),
      keyframesMax,
      tilesMax,
      labelsMax,
      drawCallsMax: Math.max(...drawCalls),
    },
    hitTest: {
      calls: N,
      hitRate: round(hits / N, 3),
      meanMs: round(hitMs.reduce((p, q) => p + q, 0) / N, 4),
      ...stats(hitMs),
    },
    cache: view.cacheStats,
    jsHeapUsedMB: mem ? round(mem.usedJSHeapSize / 1048576, 1) : null,
    userAgent: navigator.userAgent,
  };
}

main().catch((e: unknown) => {
  window.__benchError = e instanceof Error ? `${e.message}\n${e.stack ?? ""}` : String(e);
});
