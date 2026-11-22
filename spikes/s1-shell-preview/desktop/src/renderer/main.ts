// SPDX-License-Identifier: GPL-3.0-or-later
// Spike S1 renderer (main world): presents frames from `window.bridge` with an FPS/latency HUD.
//
// URL options (main passes them through from its CLI flags):
//   draw=main|preload   where the rAF present loop runs (main world = pixels cloned across
//                       contextBridge; preload = isolated world draws into our canvas, no clone)
//   policy=latest|next|clock  latest-wins, oldest unseen frame (<= 2 behind), or newest frame
//                       with pts <= audio clock - latency (latency=<ms>, default 20)
//   present=gpu|2d      WebGPU writeTexture (default) or Canvas2D putImageData (RGBA8 only)
//   verify=1|0          per-frame checksum + row-stamp verification
// The production "pts <= audio clock" scheduler is WP-0C.02; policy=clock is a first cut.

import { type LoopSnapshot, PresentLoop } from "../shared/loop.ts";
import { createPresenter } from "../shared/present.ts";
import {
  type EngineEvent,
  type EngineInfo,
  FLICKS_PER_SECOND,
  FORMAT_NV12,
  type FramePolicy,
  type S1Bridge,
} from "../shared/types.ts";

declare global {
  interface Window {
    bridge?: S1Bridge;
    __s1?: unknown;
  }
}

const params = new URLSearchParams(location.search);
const drawIn = params.get("draw") === "preload" ? "preload" : "main";
const policyParam = params.get("policy");
const policy: FramePolicy =
  policyParam === "next" || policyParam === "clock" ? policyParam : "latest";
const clockLatencyMs = Number(params.get("latency") ?? "20");
const preferGpu = params.get("present") !== "2d";
let verify = params.get("verify") !== "0";
const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

interface LoopHandle {
  snapshot(recentN?: number): LoopSnapshot | null;
  reset(): void;
  setVerify(on: boolean): void;
  framesPresented(): number;
  framesSinceUp(): number;
  engineRestarted(): void;
}

let generation = 0;
let engineMsg = "";
let rttMs = Number.NaN;

function describe(i: EngineInfo): string {
  return `engine gen ${i.generation} pid ${i.pid} | ${i.adapter} (${i.backend}, ${i.deviceType}) | ${i.width}x${i.height}@${i.fps} | ${i.slots} slots`;
}

async function main(): Promise<void> {
  const bridge = window.bridge;
  const hud = $<HTMLPreElement>("hud");
  if (!bridge) {
    hud.textContent = "window.bridge missing: this window has no preload";
    return;
  }
  const canvas = $<HTMLCanvasElement>("preview");
  bridge.setClockLatencyMs(clockLatencyMs);

  let loop: LoopHandle;
  if (drawIn === "preload") {
    await bridge.presentInPreload(canvas, { policy, verify, preferGpu });
    const p = bridge.preloadLoop;
    loop = {
      snapshot: (n) => p.snapshot(n),
      reset: () => p.reset(),
      setVerify: (on) => p.setVerify(on),
      framesPresented: () => p.framesPresented(),
      framesSinceUp: () => p.framesSinceUp(),
      engineRestarted: () => {}, // the preload resets its own loop on reconnect
    };
  } else {
    const target = await createPresenter(canvas, preferGpu);
    const l = new PresentLoop(
      "main-world",
      (after, pol) => bridge.getFrame(after, pol),
      () => bridge.nowUs(),
      target,
      policy,
      verify,
    );
    l.start();
    loop = {
      snapshot: (n) => l.snapshot(n),
      reset: () => l.reset(),
      setVerify: (on) => {
        l.verify = on;
      },
      framesPresented: () => l.framesPresented,
      framesSinceUp: () => l.framesSinceUp,
      engineRestarted: () => l.engineRestarted(),
    };
  }

  const onEngineEvent = (e: EngineEvent) => {
    if (e.state === "up") {
      generation = e.info.generation;
      loop.engineRestarted();
      engineMsg = describe(e.info);
    } else {
      const d = e.detectMs === null ? "n/a" : `${e.detectMs.toFixed(1)} ms`;
      engineMsg = `engine DOWN (code ${e.code}, signal ${e.signal}); main saw the exit after ${d}; ring closed; restarting...`;
    }
  };
  bridge.onEngineEvent(onEngineEvent);
  const initial = bridge.info();
  if (initial) onEngineEvent({ state: "up", info: initial });

  // HUD (4 Hz, last ~2 s of samples) and control-channel round trip (1 Hz).
  setInterval(() => {
    const s = loop.snapshot(120);
    if (!s) return;
    const hdr = bridge.readHeader();
    const rs = bridge.stats();
    const l = (k: keyof LoopSnapshot["latency"]) =>
      `${s.latency[k].p50.toFixed(2).padStart(6)} / ${s.latency[k].p95.toFixed(2).padStart(6)}`;
    const nv12 = s.format === FORMAT_NV12;
    const [w, h] = s.size.split("x").map(Number);
    const fps = 1000 / s.latency.rafIntervalMs.p50;
    hud.textContent = [
      `Cutweave S1 spike | transport (a) | loop in ${s.drawIn} | policy ${s.policy} | ${s.presenter}`,
      engineMsg,
      `presented ${s.framesPresented} (${s.presentFps} fps avg) | seq gaps ${s.seqGaps} | engine drops ${hdr?.engineDropped ?? "-"} | long rAF ${s.longRafs} | rAF ~${fps.toFixed(0)} Hz`,
      `integrity ${s.verify ? "ON" : "off"}: checksum fail ${s.checksumFailures} | row-stamp fail ${s.rowStampFailures} | seq violations ${s.seqViolations} | read errors ${rs.errors}`,
      `${nv12 ? "NV12" : "RGBA8"} ${s.size} (${(((w ?? 0) * (h ?? 0) * (nv12 ? 1.5 : 4)) / 1e6).toFixed(1)} MB/frame) | pts ${(s.lastPts / FLICKS_PER_SECOND).toFixed(3)} s | audio clock ${((hdr?.audioClockFlicks ?? 0) / FLICKS_PER_SECOND).toFixed(3)} s | ws rtt ${rttMs.toFixed(2)} ms`,
      "latency p50 / p95 ms (last 2 s, informal)",
      `  engine render -> present submit   ${l("renderToPresentMs")}`,
      `  ring publish  -> present submit   ${l("publishToPresentMs")}`,
      `  addon copy (shm -> JS heap)       ${l("addonCopyMs")}`,
      `  contextBridge clone               ${l("cloneMs")}`,
      `  verify (checksum + stamps)        ${l("verifyMs")}`,
      `  upload + draw (CPU)               ${l("uploadDrawMs")}`,
      `  rAF interval                      ${l("rafIntervalMs")}`,
    ].join("\n");
  }, 250);
  setInterval(() => {
    void bridge.transport.ping().then((ms) => {
      rttMs = ms;
    });
  }, 1000);

  // Controls.
  let playing = true;
  const lastPts = () => loop.snapshot(1)?.lastPts ?? 0;
  $<HTMLButtonElement>("play").onclick = (ev) => {
    playing = !playing;
    if (playing) bridge.transport.play();
    else bridge.transport.pause();
    (ev.currentTarget as HTMLButtonElement).textContent = playing ? "Pause" : "Play";
  };
  $<HTMLButtonElement>("back").onclick = () => bridge.transport.seek(lastPts() - FLICKS_PER_SECOND);
  $<HTMLButtonElement>("fwd").onclick = () => bridge.transport.seek(lastPts() + FLICKS_PER_SECOND);
  $<HTMLButtonElement>("format").onclick = () =>
    bridge.transport.setFormat(loop.snapshot(1)?.format === FORMAT_NV12 ? "rgba8" : "nv12");
  $<HTMLButtonElement>("verify").onclick = (ev) => {
    verify = !verify;
    loop.setVerify(verify);
    (ev.currentTarget as HTMLButtonElement).textContent = `Verify: ${verify ? "on" : "off"}`;
  };
  $<HTMLButtonElement>("reset").onclick = () => loop.reset();

  // Hooks for main's --selftest (main world only; contains no secrets).
  window.__s1 = {
    get framesPresented() {
      return loop.framesPresented();
    },
    get framesSinceUp() {
      return loop.framesSinceUp();
    },
    get generation() {
      return generation;
    },
    reset: () => loop.reset(),
    snapshot: () => ({ ...loop.snapshot(), reader: bridge.stats(), engineDropped: bridge.readHeader()?.engineDropped ?? null }),
  };
}

void main();
