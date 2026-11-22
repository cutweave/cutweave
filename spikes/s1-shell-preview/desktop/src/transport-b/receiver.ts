// SPDX-License-Identifier: GPL-3.0-or-later
// Transport (b) renderer side (WP-0C.06): receives frames pushed by the utility process over a
// MessagePort, keeps the last few, and hands the shared PresentLoop the frame its policy asks
// for. DOM-free so it also runs in a dedicated worker (draw=worker).
//
// Clock: the sandboxed renderer has no addon and so no direct access to the engine host clock
// (µs since the Unix epoch). It estimates the offset of `performance.timeOrigin + now()` from
// the utility's `nowUs()` with NTP-style pings over the same port (min-RTT sample of a sliding
// window), then extrapolates the engine audio clock exactly like the preload does in (a).

import { type LatSummary, Samples } from "../shared/loop.ts";
import type { EngineEvent, EngineInfo, Frame, FramePolicy } from "../shared/types.ts";
import type { ClockSample, ControlMsg, RendererToUtility, UtilityToRenderer } from "./protocol.ts";

const FLICKS_PER_US = 705.6;
const ENGINE_PLAYING = 1;
const KEEP = 4;

export interface ReceiverStats {
  framesReceived: number;
  bytesReceived: number;
  /** Frames dropped from the receive queue before any rAF looked at them. */
  queueDrops: number;
  /** Unacked-window skips reported by the utility are in its own stats. */
  clockOffsetUs: number;
  clockRttMs: number;
  /** `MessageEvent.data` access: Blink deserializes the frame into a fresh ArrayBuffer here. */
  deserializeMs: LatSummary;
  /** ring publish -> frame available in the renderer heap (host clock). */
  publishToReceiveMs: LatSummary;
  /** utility postMessage start -> renderer receipt. */
  sendToReceiveMs: LatSummary;
  /** utility shm -> heap copy, as reported per frame. */
  utilityCopyMs: LatSummary;
  /** utility postMessage duration, as reported per frame. */
  utilityPostMs: LatSummary;
  /** Time between consecutive frame messages. */
  interArrivalMs: LatSummary;
}

export class FrameReceiver {
  generation = 0;
  info: EngineInfo | null = null;
  latencyUs = 20_000;
  onEngine: (e: EngineEvent) => void = () => {};
  private frames: Frame[] = [];
  private clock: ClockSample | null = null;
  private offsetUs = 0;
  private syncs: { rtt: number; offset: number }[] = [];
  private syncTimer: ReturnType<typeof setTimeout> | null = null;
  private syncCount = 0;
  private lastArrival = 0;
  private readonly pongs = new Map<number, (ms: number) => void>();
  private received = 0;
  private bytes = 0;
  private queueDrops = 0;
  private lastServedSeq = 0;
  private readonly s = {
    deserializeMs: new Samples(),
    publishToReceiveMs: new Samples(),
    sendToReceiveMs: new Samples(),
    utilityCopyMs: new Samples(),
    utilityPostMs: new Samples(),
    interArrivalMs: new Samples(),
  };

  constructor(private readonly port: MessagePort) {
    port.onmessage = this.onMessage;
    port.start();
    this.scheduleSync();
  }

  /** Engine host clock (µs since the Unix epoch), estimated. */
  readonly nowUs = (): number => (performance.timeOrigin + performance.now()) * 1000 + this.offsetUs;

  private send(m: RendererToUtility): void {
    this.port.postMessage(m);
  }

  control(msg: ControlMsg): void {
    this.send({ type: "control", msg });
  }

  setMaxInFlight(n: number): void {
    this.send({ type: "config", maxInFlight: n });
  }

  ping(): Promise<number> {
    return new Promise((resolve) => {
      const t = performance.now();
      this.pongs.set(t, resolve);
      this.control({ type: "ping", t });
    });
  }

  private scheduleSync(): void {
    // 20 quick samples, then one per second (drift tracking).
    const delay = this.syncCount < 20 ? 50 : 1000;
    this.syncTimer = setTimeout(() => {
      this.syncCount++;
      this.send({ type: "sync", t0: performance.now() });
      this.scheduleSync();
    }, delay);
  }

  close(): void {
    if (this.syncTimer) clearTimeout(this.syncTimer);
    this.port.close();
  }

  private readonly onMessage = (ev: MessageEvent): void => {
    const t0 = performance.now();
    const m = ev.data as UtilityToRenderer; // Blink deserializes on first access
    const t1 = performance.now();
    switch (m.type) {
      case "frame": {
        const rx = this.nowUs();
        const f: Frame = { ...m.h, readUs: 0, pixels: m.pixels, tFetchUs: rx };
        this.frames.push(f);
        if (this.frames.length > KEEP) {
          const old = this.frames.shift();
          if (old && old.seq > this.lastServedSeq) this.queueDrops++;
        }
        this.clock = m.clock;
        this.received++;
        this.bytes += m.h.payloadLen;
        this.s.deserializeMs.push(t1 - t0);
        this.s.publishToReceiveMs.push((rx - m.h.tPublishUs) / 1000);
        this.s.sendToReceiveMs.push((rx - m.tSendUs) / 1000);
        this.s.utilityCopyMs.push(m.h.readUs / 1000);
        if (m.prevPostUs > 0) this.s.utilityPostMs.push(m.prevPostUs / 1000);
        if (this.lastArrival) this.s.interArrivalMs.push(t1 - this.lastArrival);
        this.lastArrival = t1;
        this.send({ type: "ack", seq: m.h.seq });
        break;
      }
      case "sync": {
        const t2 = performance.now();
        const rtt = t2 - m.t0;
        const offset = m.hostUs - (performance.timeOrigin + (m.t0 + t2) / 2) * 1000;
        this.syncs.push({ rtt, offset });
        if (this.syncs.length > 30) this.syncs.shift();
        let best = this.syncs[0];
        for (const x of this.syncs) if (best && x.rtt < best.rtt) best = x;
        if (best) this.offsetUs = best.offset;
        break;
      }
      case "engine":
        if (m.ev.state === "up") {
          this.generation = m.ev.info.generation;
          this.info = m.ev.info;
        } else {
          this.info = null;
          this.clock = null;
        }
        this.frames = [];
        this.lastServedSeq = 0;
        this.onEngine(m.ev);
        break;
      case "pong":
        this.pongs.get(m.t)?.(performance.now() - m.t);
        this.pongs.delete(m.t);
        break;
      case "probe":
        break;
    }
  };

  /** Same policies as the (a) preload's readFrame, over the receive queue. */
  readonly getFrame = (afterSeq: number, policy: FramePolicy): Frame | null => {
    const f = this.pick(afterSeq, policy);
    if (f) this.lastServedSeq = f.seq;
    return f;
  };

  private pick(afterSeq: number, policy: FramePolicy): Frame | null {
    const q = this.frames;
    const latest = q[q.length - 1];
    if (!latest || latest.seq <= afterSeq) return null;
    if (policy === "clock" && this.clock && this.clock.state === ENGINE_PLAYING) {
      const c = this.clock;
      const now = this.clock.flicks + (this.nowUs() - c.hostUs) * FLICKS_PER_US;
      const target = now - this.latencyUs * FLICKS_PER_US;
      let best: Frame | null = null;
      for (const f of q) if (f.seq > afterSeq && f.pts <= target && (!best || f.pts > best.pts)) best = f;
      return best;
    }
    if (policy === "next" && afterSeq > 0) {
      const want = Math.max(afterSeq + 1, latest.seq - 2);
      return q.find((f) => f.seq >= want) ?? latest;
    }
    return latest;
  }

  /** Engine state from the newest frame message (for the HUD). */
  clockSample(): ClockSample | null {
    return this.clock;
  }

  stats(): ReceiverStats {
    const minRtt = this.syncs.length ? Math.min(...this.syncs.map((x) => x.rtt)) : Number.NaN;
    return {
      framesReceived: this.received,
      bytesReceived: this.bytes,
      queueDrops: this.queueDrops,
      clockOffsetUs: Math.round(this.offsetUs),
      clockRttMs: Math.round(minRtt * 1000) / 1000,
      deserializeMs: this.s.deserializeMs.summary(),
      publishToReceiveMs: this.s.publishToReceiveMs.summary(),
      sendToReceiveMs: this.s.sendToReceiveMs.summary(),
      utilityCopyMs: this.s.utilityCopyMs.summary(),
      utilityPostMs: this.s.utilityPostMs.summary(),
      interArrivalMs: this.s.interArrivalMs.summary(),
    };
  }

  resetStats(): void {
    this.received = 0;
    this.bytes = 0;
    this.queueDrops = 0;
    for (const x of Object.values(this.s)) x.reset();
  }
}
