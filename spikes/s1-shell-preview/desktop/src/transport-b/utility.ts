// SPDX-License-Identifier: GPL-3.0-or-later
// Transport (b) frame pump (WP-0C.06): runs in an Electron utilityProcess.
//
// - Loads the napi addon (native code lives here, not in the renderer), maps the ring and waits
//   for publishes with `RingReader.waitForFrame` (poll thread; no JS busy loop).
// - Pushes every new frame, oldest first, to the sandboxed renderer over a MessagePortMain with
//   a small credit window (`maxInFlight` unacknowledged frames; older frames are dropped
//   latest-wins when the renderer falls behind).
// - Owns the control WebSocket to the engine: the UI token comes from main and never reaches
//   the renderer, which sends play/pause/seek over the port instead.
// - Answers clock-sync pings so the renderer can map its clock onto the engine host clock.
//
// Copies per frame on this path: shm -> utility heap (napi), V8 serialization, Mojo BigBuffer,
// renderer deserialization. Electron's MessagePortMain only transfers MessagePortMain objects,
// so ArrayBuffers cannot be transferred (probed below and recorded in the self-test).

import path from "node:path";
import type { MessagePortMain } from "electron";
import type * as Addon from "../../../bridge/index.d.ts";
import { Samples } from "../shared/loop.ts";
import type { EngineInfo } from "../shared/types.ts";
import type {
  ClockSample,
  ControlMsg,
  MainToUtility,
  RendererToUtility,
  UtilityProbes,
  UtilityStats,
  UtilityToMain,
  UtilityToRenderer,
} from "./protocol.ts";

const parent = process.parentPort;
const toMain = (m: UtilityToMain) => parent.postMessage(m);

const probes: UtilityProbes = {
  addonLoaded: false,
  node: process.versions.node,
  externalArrayBuffers: null,
  arrayBufferTransfer: null,
  sharedArrayBuffer: null,
  controlSocket: null,
};

// desktop/dist/utility.cjs -> bridge/s1_bridge.node
let addon: typeof Addon;
try {
  addon = require(path.join(__dirname, "..", "..", "bridge", "s1_bridge.node")) as typeof Addon;
  probes.addonLoaded = true;
} catch (e) {
  console.error(`[utility] cannot load the addon: ${(e as Error).message}`);
  process.exit(3);
}

const POLL_US = Number(process.env.S1B_POLL_US ?? 250);
let maxInFlight = Number(process.env.S1B_MAX_IN_FLIGHT ?? 3);

let reader: Addon.RingReader | null = null;
let info: EngineInfo | null = null;
let generation = 0;
let waiting: Promise<number> | null = null;
let ws: WebSocket | null = null;
let renderer: MessagePortMain | null = null;
let lastSentSeq = 0;
let lastAckedSeq = 0;
let inFlight = 0;
let prevPostUs = 0;
const buffers = new Map<number, Uint8Array>();

const counters = {
  framesSent: 0,
  bytesSent: 0,
  creditDrops: 0,
  readErrors: 0,
  wakeups: 0,
  timeouts: 0,
  maxInFlight: 0,
};
const copyMs = new Samples();
const postMs = new Samples();
const publishToWakeMs = new Samples();

function stats(): UtilityStats {
  return {
    ...counters,
    copyMs: copyMs.summary(),
    postMs: postMs.summary(),
    publishToWakeMs: publishToWakeMs.summary(),
  };
}

function resetStats(): void {
  for (const k of Object.keys(counters) as (keyof typeof counters)[]) counters[k] = 0;
  copyMs.reset();
  postMs.reset();
  publishToWakeMs.reset();
}

function bufferFor(len: number): Uint8Array {
  let b = buffers.get(len);
  if (!b) {
    b = new Uint8Array(len);
    buffers.set(len, b);
  }
  return b;
}

// ---- control WebSocket (token stays in this process) -----------------------------------------

function openControl(port: number, token: string): void {
  ws?.close();
  const sock = new WebSocket(`ws://127.0.0.1:${port}/`);
  sock.onopen = () => {
    sock.send(JSON.stringify({ type: "auth", token }));
    probes.controlSocket = "open";
  };
  sock.onmessage = (m) => {
    const msg = JSON.parse(String(m.data)) as { type: string; t?: number };
    if (msg.type === "pong" && typeof msg.t === "number") {
      post({ type: "pong", t: msg.t });
    }
  };
  sock.onerror = () => {
    probes.controlSocket = "error";
  };
  sock.onclose = (e) => {
    if (probes.controlSocket !== "open") probes.controlSocket = `closed (${e.code})`;
    if (ws === sock) ws = null;
  };
  ws = sock;
}

/** The renderer is untrusted from here: rebuild the message from validated fields only, so a
 * compromised page cannot use this process (which holds the token) to send arbitrary engine
 * commands. Same surface as (a)'s `bridge.transport`. */
function sanitize(m: unknown): ControlMsg | null {
  if (typeof m !== "object" || m === null) return null;
  const x = m as Record<string, unknown>;
  const num = (v: unknown) => typeof v === "number" && Number.isFinite(v);
  switch (x.type) {
    case "play":
    case "pause":
      return { type: x.type };
    case "seek":
      return num(x.pts) ? { type: "seek", pts: Math.max(0, Math.round(x.pts as number)) } : null;
    case "format":
      return x.format === "rgba8" || x.format === "nv12" ? { type: "format", format: x.format } : null;
    case "ping":
      return num(x.t) ? { type: "ping", t: x.t as number } : null;
    default:
      return null;
  }
}

function control(raw: unknown): void {
  const msg = sanitize(raw);
  if (msg && ws?.readyState === WebSocket.OPEN) ws.send(JSON.stringify(msg));
}

// ---- frame pump --------------------------------------------------------------------------------

function post(m: UtilityToRenderer): void {
  try {
    renderer?.postMessage(m);
  } catch {
    // Port closed under us; the renderer asks for a new one when it reloads.
  }
}

function clockSample(h: Addon.RingHeader): ClockSample {
  return {
    flicks: h.audioClockFlicks,
    hostUs: h.audioClockHostUs,
    state: h.engineState,
    engineDropped: h.engineDropped,
  };
}

/** Send every unsent frame (oldest first) the credit window allows. */
function pushNewFrames(r: Addon.RingReader): void {
  const port = renderer;
  if (!port) return;
  let hdr: Addon.RingHeader;
  try {
    hdr = r.readHeader();
  } catch {
    counters.readErrors++;
    return;
  }
  const fresh = hdr.slots
    .filter((s): s is Addon.FrameHeader => s !== null && s.seq > lastSentSeq)
    .sort((a, b) => a.seq - b.seq);
  if (fresh.length === 0) return;
  const avail = Math.max(0, maxInFlight - inFlight);
  if (fresh.length > avail) {
    // Latest-wins: skip the oldest unsent frames; the renderer's seq-gap counter sees them.
    const skip = fresh.splice(0, fresh.length - avail);
    counters.creditDrops += skip.length;
    lastSentSeq = skip[skip.length - 1]?.seq ?? lastSentSeq;
  }
  const clock = clockSample(hdr);
  for (const s of fresh) {
    const dst = bufferFor(s.payloadLen);
    let h: Addon.FrameHeader;
    try {
      h = r.readSlotInto(s.slot, dst);
    } catch {
      counters.readErrors++;
      continue;
    }
    if (h.seq !== s.seq) {
      counters.readErrors++; // overwritten between snapshot and copy
      continue;
    }
    copyMs.push(h.readUs / 1000);
    const t0 = performance.now();
    try {
      port.postMessage({
        type: "frame",
        h,
        clock,
        tSendUs: addon.nowUs(),
        prevPostUs,
        pixels: dst.buffer as ArrayBuffer,
      } satisfies UtilityToRenderer);
    } catch {
      return;
    }
    const dt = performance.now() - t0;
    prevPostUs = dt * 1000;
    postMs.push(dt);
    lastSentSeq = h.seq;
    inFlight++;
    counters.framesSent++;
    counters.bytesSent += h.payloadLen;
    counters.maxInFlight = Math.max(counters.maxInFlight, inFlight);
  }
}

/** No frame for a while: is the engine still alive? (It refreshes heartbeat_us every loop,
 * also while paused.) Reported once per engine generation. */
const HEARTBEAT_STALE_US = 250_000;
let stallReported = false;
function checkHeartbeat(r: Addon.RingReader): void {
  if (stallReported || !info) return;
  try {
    const h = r.readHeader();
    const age = addon.nowUs() - h.heartbeatUs;
    if (h.heartbeatUs > 0 && age > HEARTBEAT_STALE_US) {
      stallReported = true;
      toMain({ type: "engine-stalled", ring: info.ring, ageMs: age / 1000 });
    }
  } catch {
    counters.readErrors++;
  }
}

async function pump(gen: number, r: Addon.RingReader): Promise<void> {
  let seen = r.readHeader().writeIndex;
  while (gen === generation && reader === r) {
    let wi: number;
    try {
      waiting = r.waitForFrame(seen, 20, POLL_US);
      wi = await waiting;
    } catch {
      break;
    } finally {
      waiting = null;
    }
    if (gen !== generation || reader !== r) break;
    if (wi <= seen) {
      counters.timeouts++;
      checkHeartbeat(r);
      continue;
    }
    seen = wi;
    counters.wakeups++;
    if (probes.externalArrayBuffers === null) {
      try {
        const v = r.latestView(); // Electron's V8 cage: expected to copy (zeroCopy false)
        if (v) probes.externalArrayBuffers = v.zeroCopy;
      } catch {
        // ignore
      }
    }
    try {
      const latest = r.latestHeader();
      if (latest) publishToWakeMs.push((addon.nowUs() - latest.tPublishUs) / 1000);
    } catch {
      // ignore
    }
    pushNewFrames(r);
  }
}

function probeTransfers(port: MessagePortMain): void {
  if (probes.arrayBufferTransfer === null) {
    const ab = new ArrayBuffer(64);
    try {
      // Typed as MessagePortMain[]: deliberately wrong to record what Electron does.
      port.postMessage({ type: "probe" }, [ab as unknown as MessagePortMain]);
      probes.arrayBufferTransfer = `accepted (detached: ${ab.byteLength === 0})`;
    } catch (e) {
      probes.arrayBufferTransfer = `throws: ${(e as Error).message}`;
    }
  }
  if (probes.sharedArrayBuffer === null) {
    try {
      port.postMessage({ type: "probe", sab: new SharedArrayBuffer(64) });
      probes.sharedArrayBuffer = "accepted";
    } catch (e) {
      probes.sharedArrayBuffer = `throws: ${(e as Error).message}`;
    }
  }
}

function onRendererMessage(e: Electron.MessageEvent): void {
  const m = e.data as RendererToUtility;
  if (typeof m !== "object" || m === null) return;
  switch (m.type) {
    case "ack":
      if (typeof m.seq !== "number") return;
      // Freed credit is used on the next publish (<= 1 frame later). Pushing from here would
      // call `readSlotInto` (&mut) while `waitForFrame` holds napi-rs's borrow of the reader.
      if (m.seq > lastAckedSeq) {
        lastAckedSeq = m.seq;
        inFlight = Math.max(0, inFlight - 1);
      }
      break;
    case "sync":
      if (typeof m.t0 === "number") post({ type: "sync", t0: m.t0, hostUs: addon.nowUs() });
      break;
    case "control":
      control(m.msg);
      break;
    case "config":
      if (typeof m.maxInFlight === "number" && m.maxInFlight >= 1 && m.maxInFlight <= 8) {
        maxInFlight = m.maxInFlight;
      }
      break;
  }
}

// ---- main channel ------------------------------------------------------------------------------

async function engineDown(ring: string): Promise<void> {
  generation++;
  const r = reader;
  reader = null;
  info = null;
  ws?.close();
  ws = null;
  // A pending waitForFrame borrows the reader and keeps the mapping alive until it resolves
  // (<= 20 ms); close afterwards so the OS object can go away.
  await waiting?.catch(() => 0);
  r?.close();
  buffers.clear();
  lastSentSeq = 0;
  lastAckedSeq = 0;
  inFlight = 0;
  toMain({ type: "ring-closed", ring });
}

async function engineUp(i: EngineInfo, token: string): Promise<void> {
  generation++;
  const old = reader;
  reader = null;
  await waiting?.catch(() => 0);
  old?.close();
  try {
    reader = addon.RingReader.open(i.ring);
  } catch (e) {
    console.error(`[utility] cannot open ring ${i.ring}: ${(e as Error).message}`);
    return;
  }
  info = i;
  stallReported = false;
  lastSentSeq = 0;
  lastAckedSeq = 0;
  inFlight = 0;
  openControl(i.port, token);
  post({ type: "engine", ev: { state: "up", info: i } });
  void pump(generation, reader);
}

parent.on("message", (e: Electron.MessageEvent) => {
  const m = e.data as MainToUtility;
  switch (m.type) {
    case "engine-up":
      void engineUp(m.info, m.token);
      break;
    case "engine-down":
      post({ type: "engine", ev: m.ev });
      void engineDown(m.ev.ring);
      break;
    case "renderer-port": {
      const port = e.ports[0];
      if (!port) return;
      renderer?.close();
      renderer = port;
      lastAckedSeq = lastSentSeq;
      inFlight = 0;
      port.on("message", onRendererMessage);
      port.on("close", () => {
        if (renderer === port) renderer = null;
      });
      port.start();
      probeTransfers(port);
      toMain({ type: "probes", probes });
      if (info) post({ type: "engine", ev: { state: "up", info } });
      break;
    }
    case "ring-exists":
      toMain({ type: "ring-exists", id: m.id, exists: addon.ringExists(m.ring) });
      break;
    case "stats":
      toMain({ type: "stats", id: m.id, stats: stats() });
      if (m.reset) resetStats();
      break;
  }
});

toMain({ type: "ready", probes });
