// SPDX-License-Identifier: GPL-3.0-or-later
// Transport (b) wiring in Electron main (WP-0C.06), selected with `--transport=utility`.
//
// - Forks the frame-pump utilityProcess (desktop/dist/utility.cjs), which loads the napi addon,
//   owns the ring reader and the control WebSocket (the UI token goes main -> utility only).
// - Main editor window: sandbox:true, contextIsolation, no Node; its preload (preload-b.cjs)
//   only hands over a MessagePort. Main creates a MessageChannelMain per request and gives one
//   end to the utility and the other to the window; frames then flow utility -> renderer
//   directly (no hop through main).
// - Engine supervision (spawn, crash detection, restart) stays in main.ts; engine up/down are
//   forwarded to the utility, which closes its ring view and acknowledges on engine-down.
// - `--selftest=<s>` runs the (b) self-test; `--bench=<s>` the shared A/B benchmark.
// Main and the renderer load no native code in this mode.

import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import {
  app,
  BrowserWindow,
  type IpcMainEvent,
  ipcMain,
  MessageChannelMain,
  type UtilityProcess,
  utilityProcess,
  type WebContents,
} from "electron";
import { cpuPercent, runBench, sampleCpu, waitFor } from "../bench.ts";
import type { LoopSnapshot } from "../shared/loop.ts";
import type { EngineEvent, EngineInfo, Handshake } from "../shared/types.ts";
import type { MainToUtility, UtilityProbes, UtilityStats, UtilityToMain } from "./protocol.ts";
import { startTextureProbe } from "./texture-main.ts";

/** The slice of main.ts's engine supervisor that transport (b) needs. */
export interface EngineHandle {
  info: EngineInfo | null;
  readonly pid: number | null;
  lastPipeCloseMs: number | null;
  onUp: (info: EngineInfo, announce: boolean) => void;
  onDown: (e: EngineEvent) => void;
  takeHandshake(): Handshake | null;
  whenDown(): Promise<EngineEvent>;
  crash(): void;
  stop(): void;
}

export interface TransportBOptions {
  engine: EngineHandle;
  launch: (announce: boolean) => Promise<void>;
  origin: string;
  spikeRoot: string;
  /** Renderer query string (draw, policy, latency, present, verify). */
  query: string;
  selftestSeconds: number | null;
  benchSeconds: number | null;
  warmupSeconds: number;
  options: Record<string, unknown>;
}

/** Served with transport (b) pages: makes app://s1 cross-origin isolated (SharedArrayBuffer
 * probe; a SAB still cannot cross from the utility process, see the self-test). */
export const CROSS_ORIGIN_ISOLATION: Record<string, string> = {
  "cross-origin-opener-policy": "same-origin",
  "cross-origin-embedder-policy": "require-corp",
  "cross-origin-resource-policy": "same-origin",
};

type Reply = Extract<UtilityToMain, { id: number }>;

/** Supervises the frame-pump utility process. */
class FramePump {
  child: UtilityProcess | null = null;
  probes: UtilityProbes | null = null;
  onRingClosed: (ring: string) => void = () => {};
  onRestart: () => void = () => {};
  private nextId = 1;
  private readonly waiters = new Map<number, (m: Reply) => void>();
  private quitting = false;

  fork(): Promise<void> {
    const child = utilityProcess.fork(path.join(__dirname, "utility.cjs"), [], {
      serviceName: "Cutweave S1 frame pump",
      stdio: "pipe",
    });
    this.child = child;
    const log = (d: Buffer) => {
      for (const line of d.toString().split(/\r?\n/)) if (line) console.log(`[utility] ${line}`);
    };
    child.stdout?.on("data", log);
    child.stderr?.on("data", log);
    child.on("message", (m: UtilityToMain) => {
      if (m.type === "ready" || m.type === "probes") this.probes = m.probes;
      else if (m.type === "ring-closed") this.onRingClosed(m.ring);
      else {
        this.waiters.get(m.id)?.(m);
        this.waiters.delete(m.id);
      }
    });
    child.once("exit", (code) => {
      if (this.child !== child) return;
      this.child = null;
      console.log(`[main] frame-pump utility exited (code ${code})`);
      if (!this.quitting) setTimeout(() => void this.fork().then(() => this.onRestart()), 300);
    });
    return new Promise((resolve) => child.once("spawn", () => resolve()));
  }

  get pid(): number | null {
    return this.child?.pid ?? null;
  }

  send(m: MainToUtility, ports: Electron.MessagePortMain[] = []): void {
    this.child?.postMessage(m, ports);
  }

  request(m: { type: "stats"; reset: boolean }): Promise<Extract<Reply, { type: "stats" }>>;
  request(m: { type: "ring-exists"; ring: string }): Promise<Extract<Reply, { type: "ring-exists" }>>;
  request(
    m: { type: "stats"; reset: boolean } | { type: "ring-exists"; ring: string },
  ): Promise<Reply> {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      if (!this.child) {
        reject(new Error("frame-pump utility not running"));
        return;
      }
      this.waiters.set(id, resolve);
      this.send({ ...m, id });
    });
  }

  stop(): void {
    this.quitting = true;
    this.child?.kill();
  }
}

export async function startTransportB(o: TransportBOptions): Promise<void> {
  if (o.options.transport === "texture") {
    await startTextureProbe({
      origin: o.origin,
      spikeRoot: o.spikeRoot,
      size: String(o.options.size ?? "1920x1080"),
      benchSeconds: o.benchSeconds,
      selftestSeconds: o.selftestSeconds,
      warmupSeconds: o.warmupSeconds,
      options: o.options,
    });
    return;
  }
  const pump = new FramePump();
  let win: BrowserWindow | null = null;
  let handshake: Handshake | null = null;
  let preloadReport: Record<string, unknown> | null = null;
  const ringClosedWaiters = new Map<string, () => void>();
  pump.onRingClosed = (ring) => {
    ringClosedWaiters.get(ring)?.();
    ringClosedWaiters.delete(ring);
  };

  const fromWindow = (e: IpcMainEvent) =>
    win !== null &&
    e.sender === win.webContents &&
    e.senderFrame !== null &&
    e.senderFrame === e.sender.mainFrame &&
    e.senderFrame.url.startsWith(`${o.origin}/`);

  /** New channel: one end to the utility, the other to the window's preload. */
  const sendPort = (wc: WebContents) => {
    const { port1, port2 } = new MessageChannelMain();
    pump.send({ type: "renderer-port" }, [port1]);
    wc.postMessage("s1b:port", null, [port2]);
  };

  pump.onRestart = () => {
    // A fresh utility needs the ring/token again and a new port to the page.
    if (handshake) pump.send({ type: "engine-up", info: handshake.info, token: handshake.token });
    if (win) sendPort(win.webContents);
  };

  o.engine.onUp = () => {
    const hs = o.engine.takeHandshake();
    if (!hs) return;
    handshake = hs; // kept in main memory only, to re-arm a restarted utility
    pump.send({ type: "engine-up", info: hs.info, token: hs.token });
  };
  o.engine.onDown = (ev) => {
    handshake = null;
    if (ev.state === "down") pump.send({ type: "engine-down", ev });
  };

  ipcMain.on("s1b:request-port", (e) => {
    if (fromWindow(e)) sendPort(e.sender);
  });
  ipcMain.on("s1b:preload-report", (e, r: unknown) => {
    if (fromWindow(e) && typeof r === "object" && r !== null) {
      preloadReport = r as Record<string, unknown>;
    }
  });
  app.on("before-quit", () => pump.stop());
  app.on("window-all-closed", () => pump.stop());

  // draw=main|worker for this page ((a) uses draw=main|preload).
  const drawArg = process.argv.find((a) => a.startsWith("--draw="))?.slice("--draw=".length);
  const drawB = drawArg === "worker" ? "worker" : "main";
  const query = new URLSearchParams(o.query);
  query.set("draw", drawB);
  const options = { ...o.options, drawB };

  await pump.fork();
  await o.launch(false);
  win = new BrowserWindow({
    width: 1280,
    height: 820,
    backgroundColor: "#0e0f12",
    title: "Cutweave S1 spike: transport (b)",
    webPreferences: {
      preload: path.join(__dirname, "preload-b.cjs"),
      sandbox: true, // transport (b): the whole renderer keeps the OS sandbox
      contextIsolation: true,
      nodeIntegration: false,
      nodeIntegrationInSubFrames: false,
      webviewTag: false,
      spellcheck: false,
      backgroundThrottling: false,
    },
  });
  win.on("closed", () => {
    win = null;
  });
  void win.loadURL(`${o.origin}/b/index.html?${query.toString()}`);

  const finish = (ok: boolean) => {
    pump.stop();
    o.engine.stop();
    app.exit(ok ? 0 : 1);
  };
  const utilityExtra = async (reset: boolean) => {
    const s = await pump.request({ type: "stats", reset });
    return { utility: s.stats, probes: pump.probes };
  };

  if (o.benchSeconds !== null) {
    const w = win;
    runBench({
      win: w,
      transport: "utility",
      seconds: o.benchSeconds,
      warmupSeconds: o.warmupSeconds,
      engineInfo: () => o.engine.info,
      enginePid: () => o.engine.pid,
      utilityPid: () => pump.pid,
      options,
      spikeRoot: o.spikeRoot,
      extra: utilityExtra,
    }).then(finish, (e: Error) => {
      console.error(`[bench] error: ${e.stack ?? e.message}`);
      finish(false);
    });
  } else if (o.selftestSeconds !== null) {
    runSelftestB(o.selftestSeconds).then(finish, (e: Error) => {
      console.error(`[selftest] error: ${e.stack ?? e.message}`);
      finish(false);
    });
  }

  // ---- self-test (WP-0C.06 acceptance + the WP-0C.01 checks, for transport (b)) ---------------

  async function runSelftestB(seconds: number): Promise<boolean> {
    const w = win;
    if (!w) throw new Error("no window");
    const js = <T>(code: string) => w.webContents.executeJavaScript(code) as Promise<T>;
    const result: Record<string, unknown> = {
      kind: "s1-selftest",
      transport: "utility",
      informal: true,
      startedAt: new Date().toISOString(),
      platform: `${process.platform}-${process.arch}`,
      electron: process.versions.electron,
      chrome: process.versions.chrome,
      seconds,
      options,
    };
    const checks: Record<string, boolean> = {};

    await waitFor("renderer frames", () => js<number>("window.__s1?.framesPresented ?? 0"), 30_000);
    result.engine = o.engine.info;

    // The page's main world: no Node, no bridge, frames arrive anyway.
    const iso = await js<Record<string, unknown>>(`({
      require: typeof require, process: typeof process, module: typeof module,
      bridge: typeof window.bridge, electron: typeof window.electron,
      crossOriginIsolated: self.crossOriginIsolated, sharedArrayBuffer: typeof SharedArrayBuffer,
      framesPresented: window.__s1.framesPresented,
    })`);
    result.isolation = iso;
    checks.mainWorldHasNoRequire = iso.require === "undefined";
    checks.mainWorldHasNoProcess = iso.process === "undefined";
    checks.mainWorldHasNoBridge = iso.bridge === "undefined";
    checks.framesArriveInSandboxedRenderer = Number(iso.framesPresented) > 0;

    // The preload itself is sandboxed: no Node modules, no native addon.
    await waitFor("preload report", async () => preloadReport, 5_000);
    const pr = preloadReport ?? {};
    result.preload = pr;
    checks.preloadProcessSandboxed = pr.sandboxed === true && pr.contextIsolated === true;
    checks.preloadHasNoNode =
      String(pr.requireFs).startsWith("blocked") &&
      String(pr.requireChildProcess).startsWith("blocked");
    checks.preloadHasNoNativeModules =
      String(pr.requireNativeAddon).startsWith("blocked") && pr.processDlopen === "undefined";

    // OS view: the renderer process is OS-sandboxed; the utility is the privileged one.
    const rendererPid = w.webContents.getOSProcessId();
    const metric = (pid: number | null) => {
      const m = app.getAppMetrics().find((x) => x.pid === pid);
      return m
        ? { pid, type: m.type, sandboxed: m.sandboxed ?? null, integrityLevel: m.integrityLevel ?? null }
        : null;
    };
    const os = { renderer: metric(rendererPid), utility: metric(pump.pid) };
    result.os = os;
    if (process.platform === "win32" || process.platform === "darwin") {
      checks.rendererOsSandboxed = os.renderer?.sandboxed === true;
    }

    // A second window (sandboxed, no preload) gets neither a port nor a bridge.
    const w2 = new BrowserWindow({
      width: 480,
      height: 240,
      show: false,
      webPreferences: { sandbox: true, contextIsolation: true, nodeIntegration: false },
    });
    await w2.loadURL(`${o.origin}/sandboxed.html`);
    result.sandboxedWindow = (await w2.webContents.executeJavaScript(
      "({ bridge: typeof window.bridge, require: typeof require, process: typeof process, s1: typeof window.__s1 })",
    )) as Record<string, string>;
    w2.destroy();
    checks.otherWindowHasNoBridge =
      (result.sandboxedWindow as Record<string, string>).bridge === "undefined";

    result.probes = pump.probes;

    // N-second run: monotonic seq, zero torn frames (checksum + row stamps in the renderer).
    await js("window.__s1.reset()");
    await pump.request({ type: "stats", reset: true });
    const c0 = sampleCpu(o.engine.pid);
    await sleep(seconds * 1000);
    const run = await js<LoopSnapshot & Record<string, unknown>>("window.__s1.snapshot()");
    const c1 = sampleCpu(o.engine.pid);
    result.run = run;
    result.utility = (await pump.request({ type: "stats", reset: false })).stats satisfies UtilityStats;
    result.cpuPercentOfOneCore = cpuPercent(c0, c1, { renderer: rendererPid, utility: pump.pid });
    checks.framesPresented = (run.framesPresented ?? 0) > seconds * 30;
    checks.seqMonotonic = run.seqViolations === 0;
    checks.zeroTornFrames = run.checksumFailures === 0 && run.rowStampFailures === 0;
    const dir = path.join(o.spikeRoot, "results");
    mkdirSync(dir, { recursive: true });
    const opt = options as Record<string, string>;
    const tag = `utility-${opt.format}-${opt.drawB}-${opt.policy}-${opt.present}`;
    const stamp = Date.now();
    writeFileSync(
      path.join(dir, `selftest-${process.platform}-${tag}-${stamp}.png`),
      (await w.webContents.capturePage()).toPNG(),
    );

    // kill -9 the engine: main detects it < 1 s; the utility closes the ring and acks.
    const ring = o.engine.info?.ring ?? "";
    const ringClosed = new Promise<number>((r) => {
      const t = performance.now();
      ringClosedWaiters.set(ring, () => r(performance.now() - t));
    });
    const down = o.engine.whenDown();
    o.engine.crash();
    const ev = await down;
    const detectMs = ev.state === "down" ? ev.detectMs : null;
    const closeAckMs = await Promise.race([ringClosed, sleep(3000).then(() => null)]);
    const exists = (await pump.request({ type: "ring-exists", ring })).exists;
    result.crash = {
      detectMs,
      pipeCloseMs: o.engine.lastPipeCloseMs,
      closeAckMs,
      ringExistsAfterReaderClose: exists,
    };
    checks.crashDetectedUnder1s = detectMs !== null && detectMs < 1000;
    checks.ringReleased = closeAckMs !== null && !exists;

    // Restart: frames flow again from a new engine generation.
    const t0 = performance.now();
    await waitFor(
      "frames after restart",
      () => js<boolean>("window.__s1.generation >= 2 && window.__s1.framesSinceUp > 10"),
      20_000,
    );
    result.restart = { msToFramesFlowing: performance.now() - t0, engine: o.engine.info };
    checks.restartedAndFlowing = true;

    result.checks = checks;
    result.pass = Object.values(checks).every(Boolean);
    const file = path.join(dir, `selftest-${process.platform}-${tag}-${stamp}.json`);
    writeFileSync(file, `${JSON.stringify(result, null, 2)}\n`);
    console.log(`[selftest] ${result.pass ? "PASS" : "FAIL"} -> ${file}`);
    console.log(
      JSON.stringify(
        { checks, crash: result.crash, cpu: result.cpuPercentOfOneCore, probes: pump.probes },
        null,
        2,
      ),
    );
    return result.pass as boolean;
  }
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
