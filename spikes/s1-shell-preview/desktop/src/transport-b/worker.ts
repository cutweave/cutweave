// SPDX-License-Identifier: GPL-3.0-or-later
// Transport (b), draw=worker (WP-0C.06): the whole receive + present loop in a dedicated worker
// of the sandboxed renderer. The page transfers the utility MessagePort and an OffscreenCanvas
// (transferControlToOffscreen) here, so frame deserialization, verification and the WebGPU
// upload never touch the UI thread that React/overlays (WP-0C.02) run on.

import { type LoopSnapshot, PresentLoop } from "../shared/loop.ts";
import { createPresenter } from "../shared/present.ts";
import type { EngineEvent, FramePolicy } from "../shared/types.ts";
import type { ControlMsg } from "./protocol.ts";
import { FrameReceiver, type ReceiverStats } from "./receiver.ts";
import { sizedToFrame } from "./sized.ts";

export type PageToWorker =
  | {
      type: "init";
      port: MessagePort;
      canvas: OffscreenCanvas;
      policy: FramePolicy;
      verify: boolean;
      preferGpu: boolean;
      latencyMs: number;
    }
  | { type: "port"; port: MessagePort }
  | { type: "reset" }
  | { type: "verify"; on: boolean }
  | { type: "control"; msg: ControlMsg }
  | { type: "ping"; id: number }
  | { type: "snapshot"; id: number; recentN?: number | undefined };

export interface WorkerStatus {
  framesPresented: number;
  framesSinceUp: number;
  generation: number;
  presenter: string;
}

export type WorkerToPage =
  | { type: "status"; status: WorkerStatus; hud: LoopSnapshot; receiver: ReceiverStats }
  | { type: "engine"; ev: EngineEvent }
  | { type: "pong"; id: number; ms: number }
  | {
      type: "snapshot";
      id: number;
      snapshot: LoopSnapshot | null;
      receiver: ReceiverStats | null;
      engineDropped: number | null;
    }
  | { type: "error"; message: string };

interface WorkerScope {
  onmessage: ((e: MessageEvent) => void) | null;
  postMessage(m: WorkerToPage): void;
}
const scope = globalThis as unknown as WorkerScope;
const toPage = (m: WorkerToPage) => scope.postMessage(m);

let rx: FrameReceiver | null = null;
let loop: PresentLoop | null = null;
let presenter = "";
let latencyUs = 20_000;

function attach(port: MessagePort): void {
  rx?.close();
  const r = new FrameReceiver(port);
  r.latencyUs = latencyUs;
  r.onEngine = (ev) => {
    if (ev.state === "up") loop?.engineRestarted();
    toPage({ type: "engine", ev });
  };
  rx = r;
}

scope.onmessage = (e: MessageEvent) => {
  const m = e.data as PageToWorker;
  switch (m.type) {
    case "init":
      latencyUs = m.latencyMs * 1000;
      attach(m.port);
      void (async () => {
        try {
          // OffscreenCanvas has the same getContext("webgpu"/"2d") + width/height surface.
          const target = sizedToFrame(
            await createPresenter(m.canvas as unknown as HTMLCanvasElement, m.preferGpu),
            m.canvas,
          );
          presenter = `${target.kind} (${target.adapter})`;
          loop = new PresentLoop(
            "worker (sandboxed renderer)",
            (after, pol) => rx?.getFrame(after, pol) ?? null,
            () => rx?.nowUs() ?? (performance.timeOrigin + performance.now()) * 1000,
            target,
            m.policy,
            m.verify,
          );
          loop.start();
        } catch (err) {
          toPage({ type: "error", message: (err as Error).message });
        }
      })();
      setInterval(() => {
        if (!loop || !rx) return;
        toPage({
          type: "status",
          status: {
            framesPresented: loop.framesPresented,
            framesSinceUp: loop.framesSinceUp,
            generation: rx.generation,
            presenter,
          },
          hud: loop.snapshot(120),
          receiver: rx.stats(),
        });
      }, 250);
      break;
    case "port":
      attach(m.port);
      break;
    case "reset":
      loop?.reset();
      rx?.resetStats();
      break;
    case "verify":
      if (loop) loop.verify = m.on;
      break;
    case "control":
      rx?.control(m.msg);
      break;
    case "ping":
      void rx?.ping().then((ms) => toPage({ type: "pong", id: m.id, ms }));
      break;
    case "snapshot":
      toPage({
        type: "snapshot",
        id: m.id,
        snapshot: loop?.snapshot(m.recentN) ?? null,
        receiver: rx?.stats() ?? null,
        engineDropped: rx?.clockSample()?.engineDropped ?? null,
      });
      break;
  }
};
