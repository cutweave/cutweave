// SPDX-License-Identifier: GPL-3.0-or-later
// sharedTexture probe page (WP-0C.06): fully sandboxed renderer receiving GPU textures as
// `VideoFrame`s (from its preload). On receipt each frame is copied GPU-to-GPU into a small ring
// of WebGPU textures (`copyExternalImageToTexture`; no CPU pixel copy) and the VideoFrame is
// closed at once, so the producer's texture goes back to it immediately; rAF then presents from
// the ring. Frame choice: a 2-frame FIFO by default (`policy=latest` for latest-wins). Exposes
// the same `window.__s1` shape as the other transports so `--bench` works unchanged.
// Latencies use main's clock via the preload's offset estimate.

import { type LatSummary, Samples } from "../shared/loop.ts";

interface Meta {
  seq: number;
  tPaintUs: number;
  tSendUs: number;
  width: number;
  height: number;
}
interface Rx {
  meta: Meta;
  tRecvUs: number;
  vf: VideoFrame;
}
interface Queued {
  meta: Meta;
  tRecvUs: number;
  slot: number;
  width: number;
  height: number;
}

declare global {
  interface Window {
    __s1?: unknown;
  }
}

const FRAGMENT = 0x2;
// GPUTextureUsage COPY_DST | TEXTURE_BINDING | RENDER_ATTACHMENT (not declared in TS 7 lib.dom).
const RING_USAGE = 0x02 | 0x04 | 0x10;
const SHADER = /* wgsl */ `
struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs(@builtin(vertex_index) i: u32) -> VsOut {
  var p = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
  var o: VsOut;
  o.pos = vec4<f32>(p[i], 0.0, 1.0);
  o.uv = vec2<f32>((p[i].x + 1.0) * 0.5, (1.0 - p[i].y) * 0.5);
  return o;
}
@group(0) @binding(0) var samp: sampler;
@group(0) @binding(1) var tex: texture_2d<f32>;
@fragment fn fs(in: VsOut) -> @location(0) vec4<f32> {
  return vec4<f32>(textureSample(tex, samp, in.uv).rgb, 1.0);
}
`;

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const epochUs = () => (performance.timeOrigin + performance.now()) * 1000;
let offsetUs = 0;
let clockRttUs = Number.NaN;
const mainUs = () => epochUs() + offsetUs;

/** Copied into the ring, not yet presented (oldest first). The probe has no audio clock to
 * schedule against: by default a FIFO of 2 (one frame of extra latency absorbs the phase jitter
 * between the source's and the display's 60 Hz clocks); `policy=latest` keeps only the newest. */
const latestWins = new URLSearchParams(location.search).get("policy") === "latest";
const QUEUE = latestWins ? 1 : 2;
const RING = 4;
const pending: Queued[] = [];
let ingest: ((rx: Rx) => void) | null = null;
let received = 0;
let notPresented = 0;

/** Close the VideoFrame, then let the preload release the imported texture. */
function done(rx: Rx): void {
  rx.vf.close();
  window.postMessage({ type: "s1t:done", seq: rx.meta.seq }, location.origin);
}

window.addEventListener("message", (e) => {
  if (e.source !== window || e.origin !== location.origin) return;
  const m = e.data as { type?: string };
  if (m.type === "s1t:clock") {
    const c = e.data as { offsetUs: number; rttUs: number };
    offsetUs = c.offsetUs;
    clockRttUs = c.rttUs;
  } else if (m.type === "s1t:frame") {
    const rx = e.data as Rx;
    received++;
    if (ingest) ingest(rx);
    else done(rx); // WebGPU not ready yet
  }
});
window.postMessage("s1t:want-clock", location.origin);

async function main(): Promise<void> {
  const canvas = $<HTMLCanvasElement>("preview");
  const hud = $<HTMLPreElement>("hud");
  const adapter = await navigator.gpu?.requestAdapter({ powerPreference: "high-performance" });
  if (!adapter) {
    hud.textContent = "no WebGPU adapter";
    return;
  }
  const device = await adapter.requestDevice();
  const ctx = canvas.getContext("webgpu") as GPUCanvasContext | null;
  if (!ctx) throw new Error("no webgpu context");
  const format = navigator.gpu.getPreferredCanvasFormat();
  ctx.configure({ device, format, alphaMode: "opaque" });
  const module = device.createShaderModule({ code: SHADER });
  const layout = device.createBindGroupLayout({
    entries: [
      { binding: 0, visibility: FRAGMENT, sampler: {} },
      { binding: 1, visibility: FRAGMENT, texture: {} },
    ],
  });
  const pipeline = device.createRenderPipeline({
    layout: device.createPipelineLayout({ bindGroupLayouts: [layout] }),
    vertex: { module, entryPoint: "vs" },
    fragment: { module, entryPoint: "fs", targets: [{ format }] },
    primitive: { topology: "triangle-list" },
  });
  const sampler = device.createSampler({ magFilter: "linear", minFilter: "linear" });
  const info = adapter.info;
  const presenter = `webgpu copyExternalImageToTexture ring (${[info.vendor, info.architecture].filter(Boolean).join(" / ")})`;

  const lat = {
    renderToPresentMs: new Samples(), // OSR paint event in main -> present submit
    publishToPresentMs: new Samples(), // main sendSharedTexture -> present submit
    publishToReceiveMs: new Samples(), // main sendSharedTexture -> VideoFrame in the page
    copyMs: new Samples(), // copyExternalImageToTexture (CPU side of the GPU copy)
    uploadDrawMs: new Samples(), // draw from the ring + submit (CPU)
    rafIntervalMs: new Samples(),
  };

  let ring: GPUTexture[] = [];
  let bindGroups: GPUBindGroup[] = [];
  let ringDims = "";
  let nextSlot = 0;
  ingest = (rx) => {
    const w = rx.vf.codedWidth;
    const h = rx.vf.codedHeight;
    if (`${w}x${h}` !== ringDims) {
      for (const t of ring) t.destroy();
      ring = Array.from({ length: RING }, () =>
        device.createTexture({ size: [w, h], format: "rgba8unorm", usage: RING_USAGE }),
      );
      bindGroups = ring.map((t) =>
        device.createBindGroup({
          layout,
          entries: [
            { binding: 0, resource: sampler },
            { binding: 1, resource: t.createView() },
          ],
        }),
      );
      ringDims = `${w}x${h}`;
      pending.length = 0;
    }
    const slot = nextSlot;
    nextSlot = (nextSlot + 1) % RING;
    const t0 = performance.now();
    const tex = ring[slot];
    if (tex) device.queue.copyExternalImageToTexture({ source: rx.vf }, { texture: tex }, [w, h]);
    lat.copyMs.push(performance.now() - t0);
    done(rx); // the queued GPU copy holds what it needs; the source texture goes back now
    pending.push({ meta: rx.meta, tRecvUs: rx.tRecvUs, slot, width: w, height: h });
    while (pending.length > QUEUE) {
      pending.shift();
      notPresented++;
    }
  };

  let framesPresented = 0;
  let seqGaps = 0;
  let seqViolations = 0;
  let longRafs = 0;
  let lastSeq = 0;
  let lastRaf = 0;
  let startedAt = performance.now();
  let size = "";

  const tick = (now: number) => {
    if (lastRaf) {
      lat.rafIntervalMs.push(now - lastRaf);
      if (now - lastRaf > 25) longRafs++;
    }
    lastRaf = now;
    const f = pending.shift();
    const bg = f ? bindGroups[f.slot] : undefined;
    if (f && bg) {
      const t0 = performance.now();
      if (canvas.width !== f.width || canvas.height !== f.height) {
        canvas.width = f.width;
        canvas.height = f.height;
      }
      size = `${f.width}x${f.height}`;
      const enc = device.createCommandEncoder();
      const pass = enc.beginRenderPass({
        colorAttachments: [
          {
            view: ctx.getCurrentTexture().createView(),
            loadOp: "clear",
            storeOp: "store",
            clearValue: { r: 0, g: 0, b: 0, a: 1 },
          },
        ],
      });
      pass.setPipeline(pipeline);
      pass.setBindGroup(0, bg);
      pass.draw(3);
      pass.end();
      device.queue.submit([enc.finish()]);
      const t1 = performance.now();
      const nowMain = mainUs();
      lat.uploadDrawMs.push(t1 - t0);
      lat.renderToPresentMs.push((nowMain - f.meta.tPaintUs) / 1000);
      lat.publishToPresentMs.push((nowMain - f.meta.tSendUs) / 1000);
      lat.publishToReceiveMs.push((f.tRecvUs - f.meta.tSendUs) / 1000);
      if (lastSeq > 0 && f.meta.seq <= lastSeq) seqViolations++;
      if (lastSeq > 0 && f.meta.seq > lastSeq + 1) seqGaps += f.meta.seq - lastSeq - 1;
      lastSeq = f.meta.seq;
      framesPresented++;
    }
    requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);

  const snapshot = (recentN?: number) => {
    const secs = (performance.now() - startedAt) / 1000;
    const r1 = (x: number) => Math.round(x * 10) / 10;
    const L = Object.fromEntries(
      Object.entries(lat).map(([k, v]) => [k, v.summary(recentN)]),
    ) as Record<keyof typeof lat, LatSummary>;
    return {
      seconds: r1(secs),
      drawIn: "main-thread (sandboxed renderer, sharedTexture)",
      policy: latestWins ? "latest" : "next",
      presenter,
      framesPresented,
      presentFps: Math.round((framesPresented / secs) * 100) / 100,
      seqGaps,
      seqGapsPerMin: r1((seqGaps / secs) * 60),
      seqViolations,
      checksumFailures: 0,
      rowStampFailures: 0,
      longRafs,
      verify: false,
      format: 1,
      size,
      lastPts: 0,
      latency: L,
      receiver: { received, notPresented, clockOffsetUs: Math.round(offsetUs), clockRttUs },
    };
  };
  const reset = () => {
    framesPresented = 0;
    seqGaps = 0;
    seqViolations = 0;
    longRafs = 0;
    received = 0;
    notPresented = 0;
    startedAt = performance.now();
    for (const s of Object.values(lat)) s.reset();
  };

  setInterval(() => {
    const s = snapshot(120);
    const l = (x: LatSummary) => `${x.p50.toFixed(2).padStart(6)} / ${x.p95.toFixed(2).padStart(6)}`;
    hud.textContent = [
      `Cutweave S1 spike | sharedTexture probe (GPU handoff) | SANDBOXED renderer | ${presenter}`,
      `source: offscreen window (stand-in for the engine) | ${s.size} | frame choice ${s.policy}`,
      `presented ${s.framesPresented} (${s.presentFps} fps avg) | seq gaps ${s.seqGaps} | long rAF ${s.longRafs} | rx ${received} | not presented ${notPresented}`,
      `clock offset to main ${(offsetUs / 1000).toFixed(2)} ms (rtt ${(clockRttUs / 1000).toFixed(2)} ms)`,
      "latency p50 / p95 ms (last 2 s, informal)",
      `  OSR paint (main) -> present submit ${l(s.latency.renderToPresentMs)}`,
      `  main send -> VideoFrame in page   ${l(s.latency.publishToReceiveMs)}`,
      `  main send -> present submit       ${l(s.latency.publishToPresentMs)}`,
      `  GPU copy into ring (CPU side)     ${l(s.latency.copyMs)}`,
      `  draw (CPU)                        ${l(s.latency.uploadDrawMs)}`,
      `  rAF interval                      ${l(s.latency.rafIntervalMs)}`,
    ].join("\n");
  }, 250);

  window.__s1 = {
    transport: "texture",
    get framesPresented() {
      return framesPresented;
    },
    get framesSinceUp() {
      return framesPresented;
    },
    generation: 1,
    reset,
    snapshot: () => ({ ...snapshot(), engineDropped: null }),
  };
}

void main();
