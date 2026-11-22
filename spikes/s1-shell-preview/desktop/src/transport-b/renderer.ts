// SPDX-License-Identifier: GPL-3.0-or-later
// Transport (b) page (WP-0C.06): the main editor window is FULLY SANDBOXED (sandbox:true,
// contextIsolation, no Node, no native code). Frames arrive from the frame-pump utility process
// over a MessagePort handed over by the preload; nothing else is exposed to this page.
//
// URL options (main passes them through from its CLI flags):
//   draw=main|worker    present loop on the page's main thread, or in a dedicated worker with
//                       an OffscreenCanvas (keeps the UI thread free)
//   policy=latest|next|clock, latency=<ms>, present=gpu|2d, verify=1|0  as in transport (a)
// Same PresentLoop / presenters / integrity checks as (a), so the numbers are comparable.

import { type LoopSnapshot, PresentLoop } from "../shared/loop.ts";
import { createPresenter } from "../shared/present.ts";
import {
  type EngineEvent,
  type EngineInfo,
  FLICKS_PER_SECOND,
  FORMAT_NV12,
  type FramePolicy,
} from "../shared/types.ts";
import { type ControlMsg, PORT_DELIVERY, WANT_PORT } from "./protocol.ts";
import { FrameReceiver, type ReceiverStats } from "./receiver.ts";
import { sizedToFrame } from "./sized.ts";
import type { PageToWorker, WorkerToPage } from "./worker.ts";

declare global {
  interface Window {
    __s1?: unknown;
  }
}

const params = new URLSearchParams(location.search);
const drawIn = params.get("draw") === "worker" ? "worker" : "main";
const policyParam = params.get("policy");
const policy: FramePolicy =
  policyParam === "next" || policyParam === "clock" ? policyParam : "latest";
const latencyMs = Number(params.get("latency") ?? "20");
const preferGpu = params.get("present") !== "2d";
let verify = params.get("verify") !== "0";
const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

interface Snapshot {
  loop: LoopSnapshot | null;
  receiver: ReceiverStats | null;
  engineDropped: number | null;
}

/** What the HUD, the buttons and main's self-test/bench need, for either draw mode. */
interface Host {
  framesPresented(): number;
  framesSinceUp(): number;
  generation(): number;
  reset(): void;
  setVerify(on: boolean): void;
  control(msg: ControlMsg): void;
  ping(): Promise<number>;
  snapshot(recentN?: number): Promise<Snapshot>;
  onEngine(cb: (e: EngineEvent) => void): void;
  onPort(port: MessagePort): void;
}

/** Ports arrive from the preload via window.postMessage (initially and after utility restarts). */
function listenForPorts(cb: (port: MessagePort) => void): void {
  window.addEventListener("message", (e) => {
    if (e.source === window && e.origin === location.origin && e.data === PORT_DELIVERY) {
      const port = e.ports[0];
      if (port) cb(port);
    }
  });
  window.postMessage(WANT_PORT, location.origin);
}

function firstPort(): Promise<MessagePort> {
  return new Promise((resolve) => {
    let host: Host | null = null;
    let first = true;
    listenForPorts((port) => {
      if (first) {
        first = false;
        resolve(port);
      } else {
        host?.onPort(port);
      }
    });
    hostReady = (h) => {
      host = h;
    };
  });
}
let hostReady: (h: Host) => void = () => {};

async function mainThreadHost(port: MessagePort, canvas: HTMLCanvasElement): Promise<Host> {
  let rx = new FrameReceiver(port);
  rx.latencyUs = latencyMs * 1000;
  const engineCbs: ((e: EngineEvent) => void)[] = [];
  const target = sizedToFrame(await createPresenter(canvas, preferGpu), canvas);
  const loop = new PresentLoop(
    "main-thread (sandboxed renderer)",
    (after, pol) => rx.getFrame(after, pol),
    () => rx.nowUs(),
    target,
    policy,
    verify,
  );
  const wire = (r: FrameReceiver) => {
    r.onEngine = (ev) => {
      if (ev.state === "up") loop.engineRestarted();
      for (const cb of engineCbs) cb(ev);
    };
  };
  wire(rx);
  loop.start();
  return {
    framesPresented: () => loop.framesPresented,
    framesSinceUp: () => loop.framesSinceUp,
    generation: () => rx.generation,
    reset: () => {
      loop.reset();
      rx.resetStats();
    },
    setVerify: (on) => {
      loop.verify = on;
    },
    control: (msg) => rx.control(msg),
    ping: () => rx.ping(),
    snapshot: async (n) => ({
      loop: loop.snapshot(n),
      receiver: rx.stats(),
      engineDropped: rx.clockSample()?.engineDropped ?? null,
    }),
    onEngine: (cb) => {
      engineCbs.push(cb);
      if (rx.info) cb({ state: "up", info: rx.info });
    },
    onPort: (p) => {
      rx.close();
      rx = new FrameReceiver(p);
      rx.latencyUs = latencyMs * 1000;
      wire(rx);
    },
  };
}

function workerHost(port: MessagePort, canvas: HTMLCanvasElement): Host {
  const worker = new Worker("./worker.js", { type: "module" });
  const offscreen = canvas.transferControlToOffscreen();
  const post = (m: PageToWorker, transfer: Transferable[] = []) => worker.postMessage(m, transfer);
  post({ type: "init", port, canvas: offscreen, policy, verify, preferGpu, latencyMs }, [
    port,
    offscreen,
  ]);
  let status = { framesPresented: 0, framesSinceUp: 0, generation: 0, presenter: "" };
  const engineCbs: ((e: EngineEvent) => void)[] = [];
  const waiters = new Map<number, (m: WorkerToPage) => void>();
  let nextId = 1;
  let lastEngine: EngineEvent | null = null;
  worker.onmessage = (e: MessageEvent) => {
    const m = e.data as WorkerToPage;
    if (m.type === "status") status = m.status;
    else if (m.type === "engine") {
      lastEngine = m.ev;
      for (const cb of engineCbs) cb(m.ev);
    } else if (m.type === "pong" || m.type === "snapshot") {
      waiters.get(m.id)?.(m);
      waiters.delete(m.id);
    } else if (m.type === "error") {
      $<HTMLPreElement>("hud").textContent = `worker error: ${m.message}`;
    }
  };
  const ask = (m: PageToWorker & { id: number }) =>
    new Promise<WorkerToPage>((resolve) => {
      waiters.set(m.id, resolve);
      post(m);
    });
  return {
    framesPresented: () => status.framesPresented,
    framesSinceUp: () => status.framesSinceUp,
    generation: () => status.generation,
    reset: () => post({ type: "reset" }),
    setVerify: (on) => post({ type: "verify", on }),
    control: (msg) => post({ type: "control", msg }),
    ping: async () => {
      const r = await ask({ type: "ping", id: nextId++ });
      return r.type === "pong" ? r.ms : Number.NaN;
    },
    snapshot: async (n) => {
      const r = await ask({ type: "snapshot", id: nextId++, recentN: n });
      return r.type === "snapshot"
        ? { loop: r.snapshot, receiver: r.receiver, engineDropped: r.engineDropped }
        : { loop: null, receiver: null, engineDropped: null };
    },
    onEngine: (cb) => {
      engineCbs.push(cb);
      if (lastEngine) cb(lastEngine);
    },
    onPort: (p) => post({ type: "port", port: p }, [p]),
  };
}

function describe(i: EngineInfo): string {
  return `engine gen ${i.generation} pid ${i.pid} | ${i.adapter} (${i.backend}, ${i.deviceType}) | ${i.width}x${i.height}@${i.fps} | ${i.slots} slots`;
}

async function main(): Promise<void> {
  const hud = $<HTMLPreElement>("hud");
  const canvas = $<HTMLCanvasElement>("preview");
  hud.textContent = "waiting for the frame port...";
  const port = await firstPort();
  const host = drawIn === "worker" ? workerHost(port, canvas) : await mainThreadHost(port, canvas);
  hostReady(host);

  let engineMsg = "";
  let rttMs = Number.NaN;
  let generation = 0;
  host.onEngine((e) => {
    if (e.state === "up") {
      generation = e.info.generation;
      engineMsg = describe(e.info);
    } else {
      const d = e.detectMs === null ? "n/a" : `${e.detectMs.toFixed(1)} ms`;
      engineMsg = `engine DOWN (code ${e.code}, signal ${e.signal}); main saw the exit after ${d}; utility closed the ring; restarting...`;
    }
  });

  // HUD (4 Hz, last ~2 s of samples) and control round trip via utility -> engine WS (1 Hz).
  setInterval(() => {
    void host.snapshot(120).then(({ loop: s, receiver: r, engineDropped }) => {
      if (!s || !r) return;
      const l = (x: { p50: number; p95: number }) =>
        `${x.p50.toFixed(2).padStart(6)} / ${x.p95.toFixed(2).padStart(6)}`;
      const nv12 = s.format === FORMAT_NV12;
      const [w, h] = s.size.split("x").map(Number);
      hud.textContent = [
        `Cutweave S1 spike | transport (b): utilityProcess + MessagePort | SANDBOXED renderer | loop in ${s.drawIn} | policy ${s.policy} | ${s.presenter}`,
        engineMsg,
        `presented ${s.framesPresented} (${s.presentFps} fps avg) | seq gaps ${s.seqGaps} | engine drops ${engineDropped ?? "-"} | long rAF ${s.longRafs} | rAF ~${(1000 / s.latency.rafIntervalMs.p50).toFixed(0)} Hz`,
        `integrity ${s.verify ? "ON" : "off"}: checksum fail ${s.checksumFailures} | row-stamp fail ${s.rowStampFailures} | seq violations ${s.seqViolations} | rx queue drops ${r.queueDrops}`,
        `${nv12 ? "NV12" : "RGBA8"} ${s.size} (${(((w ?? 0) * (h ?? 0) * (nv12 ? 1.5 : 4)) / 1e6).toFixed(1)} MB/frame) | pts ${(s.lastPts / FLICKS_PER_SECOND).toFixed(3)} s | clock offset ${(r.clockOffsetUs / 1000).toFixed(1)} ms (rtt ${r.clockRttMs.toFixed(2)}) | ctl rtt ${rttMs.toFixed(2)} ms`,
        "latency p50 / p95 ms (last 2 s, informal)",
        `  engine render -> present submit   ${l(s.latency.renderToPresentMs)}`,
        `  ring publish  -> present submit   ${l(s.latency.publishToPresentMs)}`,
        `  ring publish  -> renderer receipt ${l(r.publishToReceiveMs)}`,
        `  utility copy (shm -> heap)        ${l(r.utilityCopyMs)}`,
        `  utility postMessage (serialize)   ${l(r.utilityPostMs)}`,
        `  renderer deserialize (ev.data)    ${l(r.deserializeMs)}`,
        `  verify (checksum + stamps)        ${l(s.latency.verifyMs)}`,
        `  upload + draw (CPU)               ${l(s.latency.uploadDrawMs)}`,
        `  rAF interval                      ${l(s.latency.rafIntervalMs)}`,
      ].join("\n");
    });
  }, 250);
  setInterval(() => {
    void host.ping().then((ms) => {
      rttMs = ms;
    });
  }, 1000);

  // Controls (sent over the port; the utility forwards them to the engine's WebSocket).
  let playing = true;
  let lastPts = 0;
  let lastFormat = 0;
  setInterval(() => {
    void host.snapshot(1).then(({ loop: s }) => {
      lastPts = s?.lastPts ?? 0;
      lastFormat = s?.format ?? 0;
    });
  }, 200);
  $<HTMLButtonElement>("play").onclick = (ev) => {
    playing = !playing;
    host.control({ type: playing ? "play" : "pause" });
    (ev.currentTarget as HTMLButtonElement).textContent = playing ? "Pause" : "Play";
  };
  $<HTMLButtonElement>("back").onclick = () =>
    host.control({ type: "seek", pts: Math.max(0, Math.round(lastPts - FLICKS_PER_SECOND)) });
  $<HTMLButtonElement>("fwd").onclick = () =>
    host.control({ type: "seek", pts: Math.round(lastPts + FLICKS_PER_SECOND) });
  $<HTMLButtonElement>("format").onclick = () =>
    host.control({ type: "format", format: lastFormat === FORMAT_NV12 ? "rgba8" : "nv12" });
  $<HTMLButtonElement>("verify").onclick = (ev) => {
    verify = !verify;
    host.setVerify(verify);
    (ev.currentTarget as HTMLButtonElement).textContent = `Verify: ${verify ? "on" : "off"}`;
  };
  $<HTMLButtonElement>("reset").onclick = () => host.reset();

  // Hooks for main's --selftest / --bench (same shape as transport (a); no secrets here).
  window.__s1 = {
    transport: "utility",
    get framesPresented() {
      return host.framesPresented();
    },
    get framesSinceUp() {
      return host.framesSinceUp();
    },
    get generation() {
      return Math.max(generation, host.generation());
    },
    reset: () => host.reset(),
    snapshot: async () => {
      const s = await host.snapshot();
      return { ...s.loop, receiver: s.receiver, engineDropped: s.engineDropped };
    },
  };
}

void main();
