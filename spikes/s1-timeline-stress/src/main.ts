// SPDX-License-Identifier: GPL-3.0-or-later
import "../../../packages/ui/src/tokens/tokens.css";
import "./style.css";
import {
  applySweep,
  type BenchResult,
  benchHitTest,
  Collector,
  summarize,
} from "./driver.ts";
import { generateTimeline, THUMB_COUNT } from "./model.ts";
import { Renderer } from "./renderer.ts";
import { percentile, Ring, round, sortedCopy } from "./stats.ts";
import { readTheme } from "./theme.ts";
import { makeThumbnails } from "./thumbs.ts";
import { type Hit, View } from "./view.ts";

declare global {
  interface Window {
    __benchResult?: BenchResult;
    __benchError?: string;
  }
  interface Performance {
    measureUserAgentSpecificMemory?: () => Promise<{ bytes: number }>;
    memory?: { usedJSHeapSize: number };
  }
}

const params = new URLSearchParams(location.search);
const benchSeconds = params.has("bench") ? Number(params.get("bench") || 30) : 0;

async function boot(): Promise<void> {
  const canvas = document.getElementById("tl") as HTMLCanvasElement;
  const hud = document.getElementById("hud") as HTMLElement;
  const theme = readTheme();
  const tl = generateTimeline();
  const t0 = performance.now();
  const thumbs = await makeThumbnails(THUMB_COUNT, theme);
  const setupMs = round(performance.now() - t0, 0);
  const view = new View(tl, theme);
  const renderer = new Renderer(canvas, view, thumbs);
  const dpr = () => window.devicePixelRatio || 1;
  const fit = () => {
    renderer.resize(window.innerWidth, window.innerHeight, dpr());
    dirty = true;
  };
  let dirty = true;
  let auto = params.has("auto");
  let autoStart = 0;
  let lastHitUs = 0;
  fit();
  view.pps = 8;
  view.t0 = 0;
  window.addEventListener("resize", fit);

  // ---- interaction -------------------------------------------------------
  type Drag =
    | { kind: "pan"; x: number; y: number; moved: boolean; hit: Hit }
    | { kind: "marquee"; ax: number; ay: number }
    | { kind: "scrub" }
    | null;
  let drag: Drag = null;
  const local = (e: PointerEvent | WheelEvent) => {
    const r = canvas.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  };

  canvas.addEventListener(
    "wheel",
    (e) => {
      e.preventDefault();
      const { x } = local(e);
      const unit = e.deltaMode === 1 ? 16 : 1;
      const dx = e.deltaX * unit;
      const dy = e.deltaY * unit;
      if (e.ctrlKey || e.metaKey) view.zoomAt(x, Math.exp(-dy * 0.0025));
      else if (e.shiftKey) view.panBy(0, dy);
      else view.panBy(dx || dy, 0);
      dirty = true;
    },
    { passive: false },
  );

  canvas.addEventListener("pointerdown", (e) => {
    const p = local(e);
    canvas.setPointerCapture(e.pointerId);
    if (p.y < view.rulerH) {
      drag = { kind: "scrub" };
      view.playhead = Math.max(0, view.xToTime(p.x));
    } else if (e.shiftKey) {
      drag = { kind: "marquee", ax: p.x, ay: p.y };
      view.marquee = { x0: p.x, y0: p.y, x1: p.x, y1: p.y };
    } else {
      const a = performance.now();
      const hit = view.hitTest(p.x, p.y);
      lastHitUs = (performance.now() - a) * 1000;
      if (hit) view.select(hit, e.ctrlKey || e.metaKey);
      drag = { kind: "pan", x: p.x, y: p.y, moved: false, hit };
    }
    dirty = true;
  });
  canvas.addEventListener("pointermove", (e) => {
    if (!drag) return;
    const p = local(e);
    if (drag.kind === "scrub") view.playhead = Math.max(0, view.xToTime(p.x));
    else if (drag.kind === "marquee" && view.marquee) {
      view.marquee.x1 = p.x;
      view.marquee.y1 = p.y;
    } else if (drag.kind === "pan") {
      view.panBy(drag.x - p.x, drag.y - p.y);
      drag.x = p.x;
      drag.y = p.y;
      drag.moved = true;
    }
    dirty = true;
  });
  const end = (e: PointerEvent) => {
    if (!drag) return;
    const p = local(e);
    if (drag.kind === "marquee" && view.marquee) {
      view.marqueeSelect(drag.ax, drag.ay, p.x, p.y, e.ctrlKey || e.metaKey);
      view.marquee = null;
    } else if (drag.kind === "pan" && !drag.moved && !drag.hit) {
      view.select(null, false); // click on empty space clears the selection
    }
    drag = null;
    dirty = true;
  };
  canvas.addEventListener("pointerup", end);
  canvas.addEventListener("pointercancel", end);
  window.addEventListener("keydown", (e) => {
    if (e.key === " ") {
      view.playing = !view.playing;
      e.preventDefault();
    } else if (e.key === "a" || e.key === "A") {
      auto = !auto;
      autoStart = performance.now();
    } else if (e.key === "Escape") {
      view.selectedClips.clear();
      view.selectedKeyframe = -1;
    }
    dirty = true;
  });

  // ---- frame loop + HUD ----------------------------------------------------
  const liveFrame = new Ring(240);
  const liveDraw = new Ring(240);
  let collector: Collector | null = null;
  let benchStart = 0;
  let onBenchDone: (() => void) | null = null;
  let lastRaf = 0;
  let lastHud = 0;

  const frame = (now: number): void => {
    requestAnimationFrame(frame);
    const dt = lastRaf ? now - lastRaf : 0;
    lastRaf = now;
    if (collector) applySweep(view, (now - benchStart) / 1000);
    else if (auto) applySweep(view, (now - autoStart) / 1000);
    if (view.playing || collector) view.playhead += dt / 1000;
    if (!(dirty || collector || auto || view.playing)) return;
    dirty = false;
    const a = performance.now();
    renderer.draw();
    const drawMs = performance.now() - a;
    if (dt > 0 && dt < 250) {
      liveFrame.push(dt);
      liveDraw.push(drawMs);
    }
    if (collector && dt > 0) {
      collector.record(dt, drawMs, renderer);
      if (now - benchStart >= benchSeconds * 1000) onBenchDone?.();
    }
    if (now - lastHud > 250) {
      lastHud = now;
      const f = sortedCopy(liveFrame.values());
      const d = sortedCopy(liveDraw.values());
      const c = renderer.counters;
      const mean = f.length ? f.reduce((p, q) => p + q, 0) / f.length : 0;
      hud.textContent = [
        `fps        ${mean ? round(1000 / mean, 1) : "-"}`,
        `frame p95  ${round(percentile(f, 95), 1)} ms`,
        `draw  p95  ${round(percentile(d, 95), 2)} ms`,
        `draw calls ${c.drawCalls}`,
        `clips      ${c.visibleClips} / ${tl.clipCount}`,
        `keyframes  ${c.visibleKeyframes} / ${tl.keyframeCount}`,
        `thumbs     ${c.thumbTiles} tiles (${thumbs.length} bitmaps)`,
        `zoom       ${round(view.pps, 2)} px/s   selected ${view.selectedClips.size}`,
        `last hit   ${round(lastHitUs, 1)} us`,
        auto ? "[A] auto sweep ON" : "[A] auto sweep   [Space] play   ctrl+wheel zoom",
        "drag pans; shift+drag marquee; click selects",
      ].join("\n");
    }
  };
  requestAnimationFrame(frame);

  // ---- benchmark mode ------------------------------------------------------
  if (benchSeconds > 0) {
    await new Promise<void>((r) => requestAnimationFrame(() => r()));
    collector = new Collector();
    benchStart = performance.now();
    await new Promise<void>((r) => {
      onBenchDone = r;
    });
    const run = collector;
    collector = null;
    const hitTest = benchHitTest(view);
    let ua: number | null = null;
    try {
      ua = performance.measureUserAgentSpecificMemory
        ? round((await performance.measureUserAgentSpecificMemory()).bytes / 1048576, 1)
        : null;
    } catch {
      ua = null;
    }
    const heap = performance.memory ? round(performance.memory.usedJSHeapSize / 1048576, 1) : null;
    window.__benchResult = summarize(run, {
      mode: params.get("mode") ?? "default",
      seconds: benchSeconds,
      viewport: { width: view.width, height: view.height, dpr: dpr() },
      dataset: {
        clips: tl.clipCount,
        keyframes: tl.keyframeCount,
        thumbnails: thumbs.length,
        tracks: tl.trackCount,
      },
      hitTest,
      memory: { jsHeapUsedMB: heap, uaSpecificMB: ua },
      userAgent: navigator.userAgent,
    });
    console.info(`setup ${setupMs} ms`);
  }
}

boot().catch((err: unknown) => {
  window.__benchError = String(err);
  console.error(err);
});
