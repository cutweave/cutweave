// SPDX-License-Identifier: GPL-3.0-or-later
// Spike S1 preload for the MAIN EDITOR WINDOW ONLY (transport (a)).
//
// Runs non-sandboxed (sandbox:false) so it can load the napi addon, but with contextIsolation:
// the page's main world only sees the narrow `window.bridge` API below, never `require`,
// `process` or the addon itself. The UI token is fetched from main over IPC, kept in this
// closure and used only to authenticate the control WebSocket opened from this isolated world;
// it never reaches the main world, the DOM, a URL or the disk.

import path from "node:path";
import { contextBridge, ipcRenderer } from "electron";
import type * as Addon from "../../bridge/index.d.ts";
import { createPreview, type Preview } from "./shared/preview.ts";
import type {
  EngineEvent,
  EngineInfo,
  Frame,
  FramePolicy,
  Handshake,
  PixelFormatName,
  RingHeader,
  S1Bridge,
} from "./shared/types.ts";

// desktop/dist/preload.cjs -> bridge/s1_bridge.node
const addon = require(path.join(__dirname, "..", "..", "bridge", "s1_bridge.node")) as typeof Addon;

let reader: Addon.RingReader | null = null;
let info: EngineInfo | null = null;
let ws: WebSocket | null = null;
let errors = 0;
let externalArrayBuffers: boolean | null = null;
let preview: Preview | null = null;
const buffers = new Map<number, Uint8Array>();
const listeners: ((e: EngineEvent) => void)[] = [];
const pongs = new Map<number, (rttMs: number) => void>();
let pingId = 0;

function emit(e: EngineEvent): void {
  for (const l of listeners) {
    try {
      l(e);
    } catch {
      errors++;
    }
  }
}

/** Control-channel diagnostics (WP-0C.02 bench). */
const wsStats = { opens: 0, closes: 0, acks: 0, errors: 0, dropped: 0, lastError: "" };

function openControl(port: number, token: string): void {
  ws?.close();
  const sock = new WebSocket(`ws://127.0.0.1:${port}/`);
  sock.onopen = () => {
    wsStats.opens++;
    sock.send(JSON.stringify({ type: "auth", token }));
    // A (re)started engine renders at its launch size until told the preview size.
    if (lastResize) sock.send(JSON.stringify(lastResize));
  };
  sock.onmessage = (m) => {
    const msg = JSON.parse(String(m.data)) as { type: string; t?: number; message?: string };
    if (msg.type === "pong" && typeof msg.t === "number") {
      pongs.get(msg.t)?.(performance.now() - msg.t);
      pongs.delete(msg.t);
    } else if (msg.type === "ack") {
      wsStats.acks++;
    } else if (msg.type === "error") {
      wsStats.errors++;
      wsStats.lastError = String(msg.message ?? "").slice(0, 200);
    }
  };
  sock.onclose = () => {
    wsStats.closes++;
    if (ws === sock) ws = null;
  };
  ws = sock;
}

function send(msg: object): void {
  if (ws?.readyState === WebSocket.OPEN) ws.send(JSON.stringify(msg));
  else wsStats.dropped++;
}

/** Last preview size requested (WP-0C.02); replayed whenever the control socket opens. */
let lastResize: { type: "resize"; width: number; height: number } | null = null;
function sendResize(width: number, height: number): void {
  lastResize = { type: "resize", width: Math.round(width), height: Math.round(height) };
  send(lastResize);
}

let connecting = false;
async function connect(): Promise<void> {
  if (connecting) return;
  connecting = true;
  try {
    const hs = (await ipcRenderer.invoke("s1:handshake")) as Handshake;
    reader?.close();
    reader = addon.RingReader.open(hs.info.ring);
    info = hs.info;
    openControl(hs.info.port, hs.token);
    preview?.engineRestarted();
    emit({ state: "up", info: hs.info });
  } catch {
    errors++;
  } finally {
    connecting = false;
  }
}

ipcRenderer.on("s1:engine", (_e, ev: EngineEvent) => {
  if (ev.state === "down") {
    // Close our handle first so the OS object can go away (Windows: last handle closes it).
    reader?.close();
    reader = null;
    info = null;
    ws?.close();
    ws = null;
    buffers.clear();
    ipcRenderer.send("s1:ring-closed", ev.ring);
    emit(ev);
  } else {
    void connect();
  }
});

function bufferFor(len: number): Uint8Array {
  let b = buffers.get(len);
  if (!b) {
    b = new Uint8Array(len);
    // Live resize (WP-0C.02) produces many frame sizes: keep only the most recent few.
    if (buffers.size >= 4) buffers.delete(buffers.keys().next().value as number);
    buffers.set(len, b);
  }
  return b;
}

const FLICKS_PER_US = 705.6;
const ENGINE_PLAYING = 1;
let clockLatencyUs = 20_000;

/** Copy the chosen frame out of the ring into a reusable preload-owned buffer. */
function readFrame(afterSeq: number, policy: FramePolicy): Frame | null {
  if (!reader) return null;
  try {
    const latest = reader.latestHeader();
    if (!latest || latest.seq <= afterSeq) return null;
    let slot = latest.slot;
    let len = latest.payloadLen;
    if (policy === "clock") {
      const hdr = reader.readHeader();
      if (hdr.engineState === ENGINE_PLAYING) {
        // Extrapolate the engine's audio clock to "now" (same host clock in both processes).
        const clock = hdr.audioClockFlicks + (addon.nowUs() - hdr.audioClockHostUs) * FLICKS_PER_US;
        const target = clock - clockLatencyUs * FLICKS_PER_US;
        let best: (typeof hdr.slots)[number] = null;
        for (const s of hdr.slots) {
          if (s && s.seq > afterSeq && s.pts <= target && (!best || s.pts > best.pts)) best = s;
        }
        if (!best) return null; // keep showing the current frame
        slot = best.slot;
        len = best.payloadLen;
      }
    } else if (policy === "next" && afterSeq > 0) {
      // Oldest unseen frame, but never more than 2 frames behind the newest.
      const want = Math.max(afterSeq + 1, latest.seq - 2);
      const s = want < latest.seq ? reader.readHeader().slots.find((x) => x?.seq === want) : null;
      if (s) {
        slot = s.slot;
        len = s.payloadLen;
      }
    }
    const dst = bufferFor(len);
    const h = reader.readSlotInto(slot, dst);
    if (h.seq <= afterSeq || h.payloadLen !== dst.byteLength) return null;
    return { ...h, pixels: dst.buffer as ArrayBuffer, tFetchUs: addon.nowUs() };
  } catch {
    errors++; // torn after retries or format changed mid-read; the next rAF retries
    return null;
  }
}

const bridge: S1Bridge = {
  version: 1,
  // NOTE: returning `pixels` through contextBridge structured-clones it (a full copy).
  getLatestFrame: (newerThanSeq) => readFrame(newerThanSeq, "latest"),
  getFrame: (afterSeq, policy) => readFrame(afterSeq, policy),
  setClockLatencyMs: (ms) => {
    if (Number.isFinite(ms) && ms >= 0 && ms <= 200) clockLatencyUs = ms * 1000;
  },
  async presentInPreload(canvas, opts) {
    preview = await createPreview(canvas, opts, {
      drawIn: "preload",
      getFrame: readFrame,
      nowUs: addon.nowUs,
      sendSeek: (pts) => send({ type: "seek", pts: Math.max(0, Math.round(pts)) }),
      sendResize,
    });
    return preview.describe();
  },
  preloadLoop: {
    snapshot: (recentN) => preview?.snapshot(recentN) ?? null,
    reset: () => preview?.reset(),
    setVerify: (on) => preview?.setVerify(on),
    framesPresented: () => preview?.framesPresented() ?? 0,
    framesSinceUp: () => preview?.framesSinceUp() ?? 0,
    setUpload: (path) => preview?.setUpload(path),
    setEngineSize: (mode) => preview?.setEngineSize(mode),
    scrub: (pts, t0) => preview?.scrub(pts, t0),
    presented: () => preview?.presented() ?? null,
    takeSamples: () => preview?.takeSamples() ?? null,
    stage: () => preview?.stage() ?? null,
    describe: () => preview?.describe() ?? "",
  },
  readHeader(): RingHeader | null {
    try {
      return reader?.readHeader() ?? null;
    } catch {
      errors++;
      return null;
    }
  },
  nowUs: () => addon.nowUs(),
  info: () => info,
  stats() {
    const s = reader?.stats() ?? { frames: 0, torn: 0, nothingNew: 0 };
    if (externalArrayBuffers === null && reader) {
      // Can this runtime alias the mapping as an ArrayBuffer? (Electron's V8 memory cage says no.)
      try {
        const v = reader.latestView();
        if (v) externalArrayBuffers = v.zeroCopy;
      } catch {
        errors++;
      }
    }
    const open = ws?.readyState === WebSocket.OPEN;
    return { ...s, errors, externalArrayBuffers, ws: { ...wsStats, open } };
  },
  transport: {
    play: () => send({ type: "play" }),
    pause: () => send({ type: "pause" }),
    seek: (pts: number) => send({ type: "seek", pts: Math.max(0, Math.round(pts)) }),
    setFormat: (format: PixelFormatName) => send({ type: "format", format }),
    resize: sendResize,
    scrubConfig: (lowresDiv: number, fullCostMs: number) =>
      send({
        type: "scrub-config",
        lowres_div: Math.max(0, Math.round(lowresDiv)),
        full_cost_ms: Math.max(0, Math.round(fullCostMs)),
      }),
    ping: () =>
      new Promise<number>((resolve) => {
        const t = performance.now() + ++pingId * 1e-6;
        pongs.set(t, resolve);
        send({ type: "ping", t });
      }),
  },
  onEngineEvent(cb) {
    listeners.push(cb);
  },
};

contextBridge.exposeInMainWorld("bridge", bridge);
void connect();
