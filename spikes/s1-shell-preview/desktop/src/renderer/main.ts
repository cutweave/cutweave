// SPDX-License-Identifier: GPL-3.0-or-later
// Spike S1 renderer (main world), WP-0C.01 + WP-0C.02.
//
// Layout: the preview canvas (WebGPU, drawn by the preview loop) with a DOM overlay on top
// (transform box, safe-area guides), a scrub bar with timecode, spike controls and a stats HUD.
// The main world owns the layout and the overlay; the preview loop owns the canvas pixels:
// in draw=preload mode (default for WP-0C.02 runs) it lives in the preload's isolated world
// and only small control/stat objects cross contextBridge.
//
// URL options (main passes them through from its CLI flags):
//   draw=main|preload   where the rAF present loop runs
//   policy=latest|next|clock  frame choice (clock: newest frame with pts <= audio clock - latency)
//   latency=<ms>        latency target of policy=clock (default 20)
//   present=gpu|2d      WebGPU (default) or Canvas2D putImageData (RGBA8 only)
//   upload=writeTexture|videoFrame  WebGPU upload path (also switchable in the window)
//   engineSize=canvas|<W>x<H>       engine renders at the canvas size (default) or a fixed size
//   verify=1|0          per-frame checksum + row-stamp verification

import type { LoopSnapshot } from "../shared/loop.ts";
import {
  createPreview,
  type EngineSizeMode,
  type Preview,
  type PreviewControls,
  type UploadPath,
} from "../shared/preview.ts";
import {
  type EngineEvent,
  type EngineInfo,
  FLICKS_PER_SECOND,
  FORMAT_NV12,
  type FramePolicy,
  type PixelFormatName,
  type S1Bridge,
} from "../shared/types.ts";
import { TransformBox } from "./overlay.ts";
import { ScrubBar } from "./scrub.ts";

declare global {
  interface Window {
    bridge?: S1Bridge;
    __s1?: unknown;
  }
}

/** Length of the synthetic sequence the scrub bar spans. */
const DURATION_S = 300;

const params = new URLSearchParams(location.search);
const drawIn = params.get("draw") === "preload" ? "preload" : "main";
const policyParam = params.get("policy");
const policy: FramePolicy =
  policyParam === "next" || policyParam === "clock" ? policyParam : "latest";
const clockLatencyMs = Number(params.get("latency") ?? "20");
const preferGpu = params.get("present") !== "2d";
const upload: UploadPath = params.get("upload") === "videoFrame" ? "videoFrame" : "writeTexture";
const parseSize = (s: string | null): EngineSizeMode => {
  const m = /^(\d+)x(\d+)$/.exec(s ?? "");
  return m ? { width: Number(m[1]), height: Number(m[2]) } : "canvas";
};
let engineSize = parseSize(params.get("engineSize"));
let verify = params.get("verify") !== "0";
const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

let generation = 0;
let engineMsg = "";
let rttMs = Number.NaN;

function describe(i: EngineInfo): string {
  return `engine gen ${i.generation} pid ${i.pid} | ${i.adapter} (${i.backend}, ${i.deviceType}) | ${i.fps} fps | ${i.slots} slots`;
}

/**
 * Size the stage to the largest 16:9 box that fits the viewer, in whole device pixels with
 * width % 4 == 0 and even height (the engine's NV12 constraint), so the canvas backing store,
 * the screen pixels and the engine's frame all match 1:1.
 */
function layoutStage(viewer: HTMLElement, stage: HTMLElement): void {
  const dpr = window.devicePixelRatio || 1;
  const r = viewer.getBoundingClientRect();
  const availW = Math.floor(r.width * dpr);
  const availH = Math.floor(r.height * dpr);
  let w = Math.floor(availW / 4) * 4;
  let h = Math.floor((w * 9) / 16 / 2) * 2;
  if (h > availH) {
    h = Math.floor(availH / 2) * 2;
    w = Math.floor((h * 16) / 9 / 4) * 4;
  }
  w = Math.max(w, 64);
  h = Math.max(h, 36);
  const s = stage.style;
  s.left = `${Math.floor((availW - w) / 2) / dpr}px`;
  s.top = `${Math.floor((availH - h) / 2) / dpr}px`;
  s.width = `${w / dpr}px`;
  s.height = `${h / dpr}px`;
}

async function main(): Promise<void> {
  const bridge = window.bridge;
  const hud = $<HTMLPreElement>("hud");
  if (!bridge) {
    hud.textContent = "window.bridge is missing: this window has no preload.";
    return;
  }
  const viewer = $<HTMLElement>("viewer");
  const stageEl = $<HTMLElement>("stage");
  const canvas = $<HTMLCanvasElement>("preview");
  const overlay = $<HTMLElement>("overlay");
  const tboxEl = $<HTMLElement>("tbox");
  const scrubEl = $<HTMLElement>("scrub");
  new ResizeObserver(() => layoutStage(viewer, stageEl)).observe(viewer);
  layoutStage(viewer, stageEl);
  bridge.setClockLatencyMs(clockLatencyMs);

  const opts = { policy, verify, preferGpu, upload, engineSize };
  let preview: PreviewControls;
  let local: Preview | null = null;
  if (drawIn === "preload") {
    await bridge.presentInPreload(canvas, opts);
    preview = bridge.preloadLoop;
  } else {
    local = await createPreview(canvas, opts, {
      drawIn: "main-world",
      getFrame: (after, pol) => bridge.getFrame(after, pol),
      nowUs: () => bridge.nowUs(),
      sendSeek: (pts) => bridge.transport.seek(pts),
      sendResize: (w, h) => bridge.transport.resize(w, h),
    });
    preview = local;
  }

  const onEngineEvent = (e: EngineEvent) => {
    if (e.state === "up") {
      generation = e.info.generation;
      local?.engineRestarted(); // the preload resets its own loop on reconnect
      engineMsg = describe(e.info);
    } else {
      const d = e.detectMs === null ? "n/a" : `${e.detectMs.toFixed(1)} ms`;
      engineMsg = `Engine stopped (code ${e.code}, signal ${e.signal}); main saw the exit after ${d}. Restarting.`;
    }
  };
  bridge.onEngineEvent(onEngineEvent);
  const initial = bridge.info();
  if (initial) onEngineEvent({ state: "up", info: initial });

  // ---- transport + scrub bar -------------------------------------------------------------
  let playing = true;
  const playBtn = $<HTMLButtonElement>("play");
  const setPlaying = (on: boolean) => {
    playing = on;
    if (on) bridge.transport.play();
    else bridge.transport.pause();
    playBtn.textContent = on ? "Pause" : "Play";
  };
  playBtn.onclick = () => setPlaying(!playing);
  const fps = bridge.info()?.fps ?? 60;
  let resumeAfterScrub = false;
  const scrub = new ScrubBar(
    scrubEl,
    $("playhead"),
    $("tc"),
    $("dur"),
    DURATION_S * FLICKS_PER_SECOND,
    fps,
    {
      start: () => {
        resumeAfterScrub = playing;
        if (playing) setPlaying(false);
      },
      seek: (pts, t0) => preview.scrub(pts, t0),
      end: () => {
        if (resumeAfterScrub) setPlaying(true);
      },
    },
  );
  const followPlayhead = () => {
    const p = preview.presented();
    if (p) scrub.setPosition(p.pts);
    requestAnimationFrame(followPlayhead);
  };
  requestAnimationFrame(followPlayhead);

  // ---- overlay ----------------------------------------------------------------------------
  const box = new TransformBox(overlay, tboxEl);
  const guides = $<HTMLElement>("guides");

  // ---- controls ---------------------------------------------------------------------------
  const lastPts = () => preview.presented()?.pts ?? 0;
  $<HTMLButtonElement>("back").onclick = (e) =>
    preview.scrub(Math.max(0, lastPts() - FLICKS_PER_SECOND), e.timeStamp);
  $<HTMLButtonElement>("fwd").onclick = (e) =>
    preview.scrub(lastPts() + FLICKS_PER_SECOND, e.timeStamp);
  const formatBtn = $<HTMLButtonElement>("format");
  let format: PixelFormatName = "rgba8";
  const setFormat = (f: PixelFormatName) => {
    format = f;
    bridge.transport.setFormat(f);
    formatBtn.textContent = `Format: ${f === "nv12" ? "NV12" : "RGBA8"}`;
  };
  formatBtn.onclick = () => setFormat(format === "nv12" ? "rgba8" : "nv12");
  const uploadBtn = $<HTMLButtonElement>("upload");
  let uploadPath = upload;
  const setUpload = (u: UploadPath) => {
    uploadPath = u;
    preview.setUpload(u);
    uploadBtn.textContent = `Upload: ${u}`;
  };
  setUpload(upload);
  uploadBtn.onclick = () => setUpload(uploadPath === "videoFrame" ? "writeTexture" : "videoFrame");
  const sizeBtn = $<HTMLButtonElement>("size");
  const setEngineSize = (m: EngineSizeMode) => {
    engineSize = m;
    preview.setEngineSize(m);
    sizeBtn.textContent = `Engine size: ${m === "canvas" ? "canvas" : `${m.width} × ${m.height}`}`;
  };
  setEngineSize(engineSize);
  sizeBtn.onclick = () =>
    setEngineSize(engineSize === "canvas" ? { width: 1920, height: 1080 } : "canvas");
  const toggle = (id: string, apply: (on: boolean) => void) => {
    const b = $<HTMLButtonElement>(id);
    b.onclick = () => {
      const on = b.getAttribute("aria-pressed") !== "true";
      b.setAttribute("aria-pressed", String(on));
      apply(on);
    };
  };
  toggle("guides-toggle", (on) => {
    guides.hidden = !on;
  });
  toggle("stats", (on) => {
    hud.hidden = !on;
  });
  toggle("verify", (on) => {
    verify = on;
    preview.setVerify(on);
  });
  $<HTMLButtonElement>("reset").onclick = () => preview.reset();

  // ---- HUD (4 Hz, last ~2 s of samples) and control-channel round trip (1 Hz) ------------
  setInterval(() => {
    if (hud.hidden) return;
    const s = preview.snapshot(120);
    if (!s) return;
    const hdr = bridge.readHeader();
    const rs = bridge.stats();
    const st = preview.stage();
    const l = (k: keyof LoopSnapshot["latency"]) =>
      `${s.latency[k].p50.toFixed(2).padStart(6)} / ${s.latency[k].p95.toFixed(2).padStart(6)}`;
    const sc = s.scrub.latency;
    const nv12 = s.format === FORMAT_NV12;
    const fpsNow = 1000 / s.latency.rafIntervalMs.p50;
    hud.textContent = [
      `Transport (a) | loop in ${s.drawIn} | policy ${s.policy} | ${s.presenter}`,
      engineMsg,
      `Presented ${s.framesPresented} (${s.presentFps} fps avg) | seq gaps ${s.seqGaps} | engine drops ${hdr?.engineDropped ?? "-"} | long rAF ${s.longRafs} | rAF ~${fpsNow.toFixed(0)} Hz`,
      `Integrity ${s.verify ? "on" : "off"}: checksum ${s.checksumFailures} | row stamps ${s.rowStampFailures} | seq violations ${s.seqViolations} | read errors ${rs.errors} | draw errors ${s.drawErrors}`,
      `${nv12 ? "NV12" : "RGBA8"} frame ${s.size}${s.lastFlags & 1 ? " (low-res)" : ""} | canvas ${st ? `${st.canvasWidth}x${st.canvasHeight} @${st.dpr}x${st.pixelExact ? " 1:1" : " scaled"}` : "-"} | pts ${(s.lastPts / FLICKS_PER_SECOND).toFixed(3)} s | ws rtt ${rttMs.toFixed(2)} ms`,
      `Resize: ${st?.resizes ?? 0} | blank ${st?.blankResizes ?? 0} | renegotiate p50/p95 ${st?.renegotiateMs.p50 ?? "-"} / ${st?.renegotiateMs.p95 ?? "-"} ms`,
      `Scrub: ${s.scrub.requests} requests | to pixel p50/p95 ${sc.firstPixelMs.p50} / ${sc.firstPixelMs.p95} ms | full-res ${sc.fullPixelMs.p50} / ${sc.fullPixelMs.p95} ms | lost ${s.scrub.lost}`,
      "Latency p50 / p95 ms (last 2 s, informal)",
      `  engine render -> present submit   ${l("renderToPresentMs")}`,
      `  addon copy (shm -> JS heap)       ${l("addonCopyMs")}`,
      `  contextBridge clone               ${l("cloneMs")}`,
      `  verify (checksum + stamps)        ${l("verifyMs")}`,
      `  upload + draw (CPU)               ${l("uploadDrawMs")}`,
      `  submit -> GPU work done           ${l("gpuDoneMs")}`,
      `  rAF interval                      ${l("rafIntervalMs")}`,
    ].join("\n");
  }, 250);
  setInterval(() => {
    void bridge.transport.ping().then((ms) => {
      rttMs = ms;
    });
  }, 1000);

  // ---- self-test / benchmark hooks (main world only; no secrets) -------------------------
  const rectOf = (el: Element) => {
    const b = el.getBoundingClientRect();
    return { x: b.left, y: b.top, w: b.width, h: b.height };
  };
  const nameOf = (el: Element | null): string => {
    if (!el) return "null";
    if (el.id) return `#${el.id}`;
    const h = el instanceof HTMLElement && el.dataset.h ? `[${el.dataset.h}]` : "";
    return `${el.tagName.toLowerCase()}.${String(el.className).split(" ")[0]}${h}`;
  };
  /** DOM rects vs canvas rect, in device pixels (WP-0C.02 acceptance: pixel-aligned). */
  const alignment = () => {
    const dpr = window.devicePixelRatio || 1;
    const c = canvas.getBoundingClientRect();
    const o = overlay.getBoundingClientRect();
    const t = tboxEl.getBoundingClientRect();
    const m = box.rect;
    const d = (a: number, b: number) => Math.abs(a - b) * dpr;
    const overlayPx = Math.max(
      d(c.left, o.left),
      d(c.top, o.top),
      d(c.width, o.width),
      d(c.height, o.height),
    );
    const tboxPx = Math.max(
      d(t.left, c.left + m.x * c.width),
      d(t.top, c.top + m.y * c.height),
      d(t.width, m.w * c.width),
      d(t.height, m.h * c.height),
    );
    const backingPx = Math.max(
      Math.abs(canvas.width - c.width * dpr),
      Math.abs(canvas.height - c.height * dpr),
    );
    const p = preview.presented();
    const r3 = (x: number) => Math.round(x * 1000) / 1000;
    return {
      overlayPx: r3(overlayPx),
      tboxPx: r3(tboxPx),
      backingPx: r3(backingPx),
      canvas: [canvas.width, canvas.height],
      frame: p ? [p.width, p.height] : null,
      ok: overlayPx <= 0.5 && tboxPx <= 1 && backingPx <= 1,
    };
  };
  /** After the next rendering update (rAF, layout, ResizeObservers, paint). */
  const nextFrame = () =>
    new Promise<void>((r) => requestAnimationFrame(() => setTimeout(r, 0)));
  /**
   * Alignment as the compositor sees it. A script task can run between a window resize's
   * layout and the ResizeObserver that resizes the canvas in the same rendering update, so a
   * mismatch is re-measured after up to 3 frames; only a mismatch that survives is real.
   */
  const settledAlignment = async () => {
    let a = alignment();
    let frames = 0;
    while (!a.ok && frames < 3) {
      await nextFrame();
      frames++;
      a = alignment();
    }
    return { ...a, settleFrames: frames };
  };
  /** elementFromPoint checks: overlay above canvas, guides/HUD click-through, handles hit. */
  const hitTest = () => {
    const out: { label: string; hit: string; ok: boolean }[] = [];
    const check = (label: string, x: number, y: number, ok: (el: Element | null) => boolean) => {
      const el = document.elementFromPoint(x, y);
      out.push({ label, hit: nameOf(el), ok: ok(el) });
    };
    box.select(true);
    const c = canvas.getBoundingClientRect();
    const t = tboxEl.getBoundingClientRect();
    const inBox = (x: number, y: number) =>
      x >= t.left && x <= t.right && y >= t.top && y <= t.bottom;
    check("transform box body", t.left + t.width / 2, t.top + t.height / 2, (e) => e === tboxEl);
    for (const [h, el] of box.handles) {
      const r = el.getBoundingClientRect();
      check(`handle ${h}`, r.left + r.width / 2, r.top + r.height / 2, (e) => e === el);
    }
    const ex = c.left + c.width * 0.15;
    const ey = c.top + c.height * 0.85;
    if (!inBox(ex, ey)) check("empty overlay area", ex, ey, (e) => e === overlay);
    const gx = c.left + c.width * 0.035 + 0.5;
    const gy = c.top + c.height * 0.5;
    if (!inBox(gx, gy)) check("safe-area guide line is click-through", gx, gy, (e) => e === overlay);
    const hr = hud.getBoundingClientRect();
    if (!hud.hidden) {
      check("stats panel is click-through", hr.left + 4, hr.top + 4, (e) => e !== null && e !== hud);
    }
    const sr = scrubEl.getBoundingClientRect();
    check("scrub bar", sr.left + sr.width / 2, sr.top + sr.height / 2, (e) =>
      e !== null ? scrubEl.contains(e) : false,
    );
    for (const fx of [0.05, 0.5, 0.95]) {
      for (const fy of [0.05, 0.5, 0.95]) {
        check(
          `canvas point ${fx},${fy} is covered by the overlay`,
          c.left + c.width * fx,
          c.top + c.height * fy,
          (e) => e !== null && e !== canvas && overlay.contains(e),
        );
      }
    }
    return out;
  };

  window.__s1 = {
    get framesPresented() {
      return preview.framesPresented();
    },
    get framesSinceUp() {
      return preview.framesSinceUp();
    },
    get generation() {
      return generation;
    },
    get playing() {
      return playing;
    },
    reset: () => preview.reset(),
    snapshot: () => ({
      ...preview.snapshot(),
      reader: bridge.stats(),
      engineDropped: bridge.readHeader()?.engineDropped ?? null,
      stage: preview.stage(),
    }),
    play: () => setPlaying(true),
    pause: () => setPlaying(false),
    scrubConfig: (lowresDiv: number, fullCostMs: number) =>
      bridge.transport.scrubConfig(lowresDiv, fullCostMs),
    setScrubWait: (ms: number) => preview.setScrubWait(ms),
    control: () => bridge.stats().ws ?? null,
    ping: () => bridge.transport.ping(),
    setFormat,
    setUpload,
    setEngineSize,
    /** One per-second metrics sample (counters are cumulative; sample arrays are drained). */
    sample: async () => {
      const snap = preview.snapshot(1);
      const p = preview.presented();
      const s = preview.takeSamples();
      return {
        framesPresented: p?.framesPresented ?? 0,
        seqGaps: snap?.seqGaps ?? 0,
        engineDropped: bridge.readHeader()?.engineDropped ?? null,
        scrubPixelMs: s?.scrubPixelMs ?? [],
        renegotiateMs: s?.renegotiateMs ?? [],
        frame: p ? [p.width, p.height] : null,
        align: await settledAlignment(),
      };
    },
    alignment: settledAlignment,
    hitTest,
    stage: () => preview.stage(),
    rects: () => ({
      dpr: window.devicePixelRatio,
      viewer: rectOf(viewer),
      canvas: rectOf(canvas),
      scrub: rectOf(scrubEl),
      tbox: rectOf(tboxEl),
      handles: Object.fromEntries([...box.handles].map(([h, el]) => [h, rectOf(el)])),
    }),
    tbox: () => ({ ...box.rect, gestures: box.gestures, selected: tboxEl.classList.contains("selected") }),
    scrubRequests: () => scrub.requests,
  };
}

void main();
