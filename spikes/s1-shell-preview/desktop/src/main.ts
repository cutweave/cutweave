// SPDX-License-Identifier: GPL-3.0-or-later
// Spike S1 Electron main process (WP-0C.01, transport (a)).
//
// - Picks the control port (arch §6 "stable port"), spawns `s1-engine --port P --ring R`, reads
//   the one-line stdout handshake (port + one-time 256-bit UI token; never written to disk) and
//   supervises the child (crash detection, ring cleanup handshake with the preload, restart).
// - Serves the renderer from `app://s1/` only, with a strict CSP.
// - Main editor window: non-sandboxed preload (needed to load the napi addon), contextIsolation
//   on, nodeIntegration off. Every other window is sandboxed and gets no preload.
// - `--selftest=<seconds>` runs the WP-0C.01 acceptance checks and writes results/*.json.

import { execFileSync, spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import path from "node:path";
import { createInterface } from "node:readline";
import {
  app,
  BrowserWindow,
  type IpcMainEvent,
  type IpcMainInvokeEvent,
  ipcMain,
  protocol,
  session,
} from "electron";
import type * as Addon from "../../bridge/index.d.ts";
import { runBench } from "./bench.ts";
import type { EngineEvent, EngineInfo, Handshake } from "./shared/types.ts";

const SPIKE_ROOT = path.resolve(__dirname, "..", "..");
const RENDERER_ROOT = path.join(__dirname, "renderer");
const ORIGIN = "app://s1";
const ENGINE_BIN =
  process.env.S1_ENGINE ??
  path.join(SPIKE_ROOT, "target", "release", process.platform === "win32" ? "s1-engine.exe" : "s1-engine");

// ---- options --------------------------------------------------------------------------------

function flag(name: string): string | undefined {
  const hit = process.argv.find((a) => a === `--${name}` || a.startsWith(`--${name}=`));
  if (hit === undefined) return undefined;
  const eq = hit.indexOf("=");
  return eq < 0 ? "" : hit.slice(eq + 1);
}

const opts = {
  selftestSeconds: flag("selftest") === undefined ? null : Number(flag("selftest") || 60),
  format: flag("format") === "nv12" ? "nv12" : "rgba8",
  gpu: flag("gpu") ?? "high",
  size: flag("size") ?? "1920x1080",
  // Renderer options, passed through as URL query parameters (see renderer/main.ts).
  draw: flag("draw") === "preload" ? "preload" : "main",
  policy: ["next", "clock"].includes(flag("policy") ?? "") ? (flag("policy") as string) : "latest",
  latency: flag("latency") ?? "20",
  present: flag("present") === "2d" ? "2d" : "gpu",
  verify: flag("verify") === "0" ? "0" : "1",
  chromiumDgpu: flag("chromium-dgpu") !== undefined,
  // WP-0C.02: WebGPU upload path, engine output size (canvas | WxH), `--bench[=quick|<s>]`.
  upload: flag("upload") === "videoFrame" ? "videoFrame" : "writeTexture",
  engineSize: flag("engine-size") ?? "canvas",
  bench: flag("bench"),
};
const rendererQuery = new URLSearchParams({
  draw: opts.draw,
  policy: opts.policy,
  latency: opts.latency,
  present: opts.present,
  verify: opts.verify,
  upload: opts.upload,
  engineSize: opts.engineSize,
}).toString();
const [engineWidth, engineHeight] = opts.size.split("x").map(Number) as [number, number];

// Keep rAF running when the window is covered (spike measurement hygiene; Windows only).
app.commandLine.appendSwitch("disable-features", "CalculateNativeWinOcclusion");
// Hybrid-GPU laptops: Chromium's GPU process (and so WebGPU) defaults to the iGPU; this puts it
// on the dGPU, i.e. the same adapter wgpu's HighPerformance picks for the engine.
if (flag("chromium-dgpu") !== undefined) app.commandLine.appendSwitch("force_high_performance_gpu");

protocol.registerSchemesAsPrivileged([
  { scheme: "app", privileges: { standard: true, secure: true, supportFetchAPI: true } },
]);

// ---- port + CSP -----------------------------------------------------------------------------

function pickPort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const srv = createServer();
    srv.once("error", reject);
    srv.listen(0, "127.0.0.1", () => {
      const addr = srv.address();
      const port = typeof addr === "object" && addr ? addr.port : 0;
      srv.close(() => resolve(port));
    });
  });
}

let port = 0;

function csp(): string {
  return [
    "default-src 'none'",
    "script-src 'self'",
    "style-src 'self'",
    "img-src 'self' data:",
    `connect-src ws://127.0.0.1:${port}`,
    "base-uri 'none'",
    "form-action 'none'",
    "frame-ancestors 'none'",
    "object-src 'none'",
  ].join("; ");
}

const MIME: Record<string, string> = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".map": "application/json",
};

function serveApp(req: Request): Response {
  const url = new URL(req.url);
  const rel = decodeURIComponent(url.pathname).replace(/^\/+/, "") || "index.html";
  const file = path.resolve(RENDERER_ROOT, rel);
  if (url.host !== "s1" || !file.startsWith(RENDERER_ROOT + path.sep) || !existsSync(file)) {
    return new Response("not found", { status: 404 });
  }
  return new Response(readFileSync(file), {
    headers: {
      "content-type": MIME[path.extname(file)] ?? "application/octet-stream",
      "content-security-policy": csp(),
      "x-content-type-options": "nosniff",
    },
  });
}

// ---- engine supervisor ----------------------------------------------------------------------

class Engine {
  info: EngineInfo | null = null;
  private child: ReturnType<typeof spawn> | null = null;
  private pending: Handshake | null = null;
  private upWaiters: ((i: EngineInfo) => void)[] = [];
  private exitWaiters: ((e: EngineEvent) => void)[] = [];
  private generation = 0;
  private killIssuedAt: number | null = null;
  private pipeClosedAt: number | null = null;
  /** ms from kill() to the stdout pipe closing, for the last kill main issued. */
  lastPipeCloseMs: number | null = null;
  private quitting = false;
  private suppressRestart = false;
  onUp: (info: EngineInfo, announce: boolean) => void = () => {};
  onDown: (e: EngineEvent) => void = () => {};

  start(announce = true): Promise<EngineInfo> {
    const ring = `cw-s1-${randomBytes(6).toString("hex")}`;
    const args = [
      "--ring", ring,
      "--port", String(port),
      "--allow-origin", ORIGIN,
      "--format", opts.format,
      "--gpu", opts.gpu,
      "--width", String(engineWidth),
      "--height", String(engineHeight),
    ];
    const child = spawn(ENGINE_BIN, args, { stdio: ["pipe", "pipe", "pipe"], windowsHide: true });
    this.child = child;
    this.killIssuedAt = null;
    this.pipeClosedAt = null;
    // The handshake pipe closes during process rundown, which can precede the 'exit' event
    // (the process object is signalled only after driver/GPU teardown).
    child.stdout?.once("close", () => {
      this.pipeClosedAt = performance.now();
    });
    child.stderr?.on("data", (d: Buffer) => {
      for (const line of d.toString().split(/\r?\n/)) if (line) console.log(`[engine] ${line}`);
    });
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        child.kill();
        reject(new Error("engine handshake timeout"));
      }, 20_000);
      const rl = createInterface({ input: child.stdout! });
      rl.once("line", (line) => {
        clearTimeout(timer);
        rl.close();
        child.stdout?.resume();
        let msg: Record<string, unknown>;
        try {
          msg = JSON.parse(line) as Record<string, unknown>;
        } catch {
          reject(new Error("engine handshake was not JSON"));
          return;
        }
        if (msg.s1 !== "ready" || msg.ring !== ring || typeof msg.token !== "string") {
          reject(new Error("unexpected engine handshake"));
          return;
        }
        const info: EngineInfo = {
          pid: Number(msg.pid),
          port: Number(msg.port),
          ring,
          width: Number(msg.width),
          height: Number(msg.height),
          fps: Number(msg.fps),
          slots: Number(msg.slots),
          adapter: String(msg.adapter),
          backend: String(msg.backend),
          deviceType: String(msg.device_type),
          generation: ++this.generation,
        };
        // The token lives only in this object until the preload takes it.
        this.pending = { info, token: msg.token };
        this.info = info;
        console.log(`[main] engine up: pid ${info.pid}, ring ${ring}, ${info.adapter} (${info.backend})`);
        resolve(info);
        for (const w of this.upWaiters.splice(0)) w(info);
        this.onUp(info, announce);
      });
      child.once("exit", (code, signal) => {
        clearTimeout(timer);
        this.handleExit(child, ring, code, signal);
        reject(new Error(`engine exited (${code ?? signal}) before handshake`));
      });
    });
  }

  private handleExit(
    child: ReturnType<typeof spawn>,
    ring: string,
    code: number | null,
    signal: string | null,
  ): void {
    const detectMs = this.killIssuedAt === null ? null : performance.now() - this.killIssuedAt;
    this.lastPipeCloseMs =
      this.killIssuedAt === null || this.pipeClosedAt === null
        ? null
        : this.pipeClosedAt - this.killIssuedAt;
    if (this.child !== child) return;
    this.child = null;
    this.info = null;
    this.pending = null;
    const ev: EngineEvent = { state: "down", ring, code, signal, detectMs };
    console.log(`[main] engine down: code=${code} signal=${signal} detectMs=${detectMs?.toFixed(2)}`);
    this.onDown(ev);
    for (const w of this.exitWaiters.splice(0)) w(ev);
    if (!this.quitting && !this.suppressRestart) {
      setTimeout(() => void launch(true), 300);
    }
  }

  /** Hand out the token once per launch. */
  takeHandshake(): Handshake | null {
    const p = this.pending;
    this.pending = null;
    return p;
  }

  whenUp(): Promise<EngineInfo> {
    return this.info ? Promise.resolve(this.info) : new Promise((r) => this.upWaiters.push(r));
  }

  whenDown(): Promise<EngineEvent> {
    return new Promise((r) => this.exitWaiters.push(r));
  }

  /** `kill -9` equivalent (TerminateProcess on Windows). */
  crash(): void {
    this.killIssuedAt = performance.now();
    this.child?.kill("SIGKILL");
  }

  /** Graceful restart without announcing (renderer reload needs a fresh token). */
  async restartQuietly(): Promise<void> {
    if (this.child) {
      this.suppressRestart = true;
      const down = this.whenDown();
      this.child.stdin?.end();
      await down;
      this.suppressRestart = false;
    }
    await this.start(false);
  }

  get pid(): number | null {
    return this.child?.pid ?? null;
  }

  stop(): void {
    this.quitting = true;
    const c = this.child;
    if (!c) return;
    c.stdin?.end(); // engine exits on stdin EOF
    setTimeout(() => c.kill("SIGKILL"), 2000).unref();
  }
}

const engine = new Engine();
let mainWindow: BrowserWindow | null = null;
const ringClosedWaiters = new Map<string, () => void>();

async function launch(announce: boolean): Promise<void> {
  for (let attempt = 0; attempt < 3; attempt++) {
    try {
      await engine.start(announce);
      return;
    } catch (e) {
      console.error(`[main] engine start failed: ${(e as Error).message}`);
      if (attempt < 2) {
        // arch §6: if the bind fails, pick a new port and reload the renderer (CSP changes).
        port = await pickPort();
        mainWindow?.webContents.reload();
      }
    }
  }
}

function fromMainEditorFrame(e: IpcMainEvent | IpcMainInvokeEvent): boolean {
  return (
    mainWindow !== null &&
    e.sender === mainWindow.webContents &&
    e.senderFrame !== null &&
    e.senderFrame === e.sender.mainFrame &&
    e.senderFrame.url.startsWith(`${ORIGIN}/`)
  );
}

ipcMain.handle("s1:handshake", async (e): Promise<Handshake> => {
  if (!fromMainEditorFrame(e)) throw new Error("forbidden");
  await engine.whenUp();
  let hs = engine.takeHandshake();
  if (!hs) {
    // Token already handed out for this launch (e.g. renderer reload): mint a new one.
    await engine.restartQuietly();
    hs = engine.takeHandshake();
  }
  if (!hs) throw new Error("engine unavailable");
  return hs;
});

ipcMain.on("s1:ring-closed", (e, ring: unknown) => {
  if (!fromMainEditorFrame(e) || typeof ring !== "string") return;
  ringClosedWaiters.get(ring)?.();
  ringClosedWaiters.delete(ring);
});

engine.onDown = (ev) => mainWindow?.webContents.send("s1:engine", ev);
engine.onUp = (info, announce) => {
  if (announce) mainWindow?.webContents.send("s1:engine", { state: "up", info } satisfies EngineEvent);
};

// ---- windows --------------------------------------------------------------------------------

app.on("web-contents-created", (_e, wc) => {
  wc.setWindowOpenHandler(() => ({ action: "deny" }));
  wc.on("will-navigate", (ev, url) => {
    if (!url.startsWith(`${ORIGIN}/`)) ev.preventDefault();
  });
  wc.on("will-attach-webview", (ev) => ev.preventDefault());
});

function createMainWindow(): BrowserWindow {
  const win = new BrowserWindow({
    width: 1280,
    height: 820,
    backgroundColor: "#0e0f12",
    title: "Cutweave S1 spike: transport (a)",
    webPreferences: {
      preload: path.join(__dirname, "preload.cjs"),
      sandbox: false, // transport (a): the preload must load the napi addon
      contextIsolation: true,
      nodeIntegration: false,
      nodeIntegrationInSubFrames: false,
      webviewTag: false,
      spellcheck: false,
      backgroundThrottling: false,
    },
  });
  win.on("closed", () => {
    mainWindow = null;
  });
  void win.loadURL(`${ORIGIN}/index.html?${rendererQuery}`);
  return win;
}

/** Any other window: sandboxed, no preload. */
function createSandboxedWindow(show: boolean): BrowserWindow {
  return new BrowserWindow({
    width: 480,
    height: 240,
    show,
    title: "Sandboxed window",
    webPreferences: { sandbox: true, contextIsolation: true, nodeIntegration: false },
  });
}

// ---- self-test ------------------------------------------------------------------------------

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function waitFor<T>(what: string, fn: () => Promise<T | null | undefined | false>, ms: number): Promise<T> {
  const until = Date.now() + ms;
  for (;;) {
    const v = await fn().catch(() => null);
    if (v) return v;
    if (Date.now() > until) throw new Error(`timeout waiting for ${what}`);
    await sleep(100);
  }
}

function engineCpuMs(pid: number | null): number | null {
  if (pid === null || process.platform !== "win32") return null;
  try {
    const out = execFileSync(
      "powershell",
      ["-NoProfile", "-Command", `(Get-Process -Id ${pid}).TotalProcessorTime.TotalMilliseconds`],
      { encoding: "utf8", windowsHide: true },
    );
    return Number(out.trim().replace(",", "."));
  } catch {
    return null;
  }
}

function electronCpuSeconds(): Record<string, number> {
  const out: Record<string, number> = {};
  for (const m of app.getAppMetrics()) {
    out[m.type] = (out[m.type] ?? 0) + (m.cpu.cumulativeCPUUsage ?? 0);
  }
  return out;
}

async function runSelftest(seconds: number): Promise<void> {
  const win = mainWindow;
  if (!win) throw new Error("no main window");
  const js = <T>(code: string) => win.webContents.executeJavaScript(code) as Promise<T>;
  const result: Record<string, unknown> = {
    kind: "s1-selftest",
    informal: true,
    startedAt: new Date().toISOString(),
    platform: `${process.platform}-${process.arch}`,
    electron: process.versions.electron,
    chrome: process.versions.chrome,
    seconds,
    options: opts,
  };
  const checks: Record<string, boolean> = {};

  await waitFor("renderer frames", () => js<number>("window.__s1?.framesPresented ?? 0"), 30_000);
  result.engine = engine.info;

  // AC 1a: main world has no Node, but has the bridge and gets frames.
  const iso = await js<Record<string, unknown>>(`(() => {
    const f = window.bridge.getLatestFrame(0);
    return {
      require: typeof require, process: typeof process, module: typeof module,
      bridge: typeof window.bridge, bridgeKeys: Object.keys(window.bridge).sort(),
      frame: f ? { seq: f.seq, bytes: f.pixels.byteLength, isArrayBuffer: f.pixels instanceof ArrayBuffer } : null,
    };
  })()`);
  result.isolation = iso;
  checks.mainWorldHasNoRequire = iso.require === "undefined";
  checks.mainWorldHasNoProcess = iso.process === "undefined";
  checks.bridgeReturnsFrames = iso.bridge === "object" && iso.frame !== null;

  // AC 1b: a second, sandboxed window has no bridge.
  const w2 = createSandboxedWindow(false);
  await w2.loadURL(`${ORIGIN}/sandboxed.html`);
  const sandboxed = (await w2.webContents.executeJavaScript(
    "({ bridge: typeof window.bridge, require: typeof require, process: typeof process })",
  )) as Record<string, string>;
  w2.destroy();
  result.sandboxedWindow = sandboxed;
  checks.sandboxedWindowHasNoBridge = sandboxed.bridge === "undefined";

  // AC 2: N-second run, monotonic seq, zero torn frames.
  await js("window.__s1.reset()");
  const cpu0 = { engineMs: engineCpuMs(engine.pid), electron: electronCpuSeconds(), t: performance.now() };
  await sleep(seconds * 1000);
  const run = await js<Record<string, number>>("window.__s1.snapshot()");
  const wallS = (performance.now() - cpu0.t) / 1000;
  const engineMs1 = engineCpuMs(engine.pid);
  const electron1 = electronCpuSeconds();
  result.run = run;
  result.cpuPercentOfOneCore = {
    engine: cpu0.engineMs !== null && engineMs1 !== null ? ((engineMs1 - cpu0.engineMs) / 1000 / wallS) * 100 : null,
    ...Object.fromEntries(
      Object.entries(electron1).map(([k, v]) => [k, ((v - (cpu0.electron[k] ?? 0)) / wallS) * 100]),
    ),
  };
  checks.framesPresented = (run.framesPresented ?? 0) > seconds * 30;
  checks.seqMonotonic = run.seqViolations === 0;
  checks.zeroTornFrames = run.checksumFailures === 0 && run.rowStampFailures === 0;
  const dir = path.join(SPIKE_ROOT, "results");
  mkdirSync(dir, { recursive: true });
  const tag = `${opts.format}-${opts.draw}-${opts.policy}-${opts.present}`;
  const stamp = Date.now();
  // Visual evidence (not committed): what the window showed at the end of the run.
  writeFileSync(path.join(dir, `selftest-${process.platform}-${tag}-${stamp}.png`), (await win.webContents.capturePage()).toPNG());

  // AC 3: kill -9 the engine; main detects < 1 s; ring cleaned up.
  const ring = engine.info?.ring ?? "";
  const ringClosed = new Promise<number>((r) => {
    const t = performance.now();
    ringClosedWaiters.set(ring, () => r(performance.now() - t));
  });
  const down = engine.whenDown();
  engine.crash();
  const ev = await down;
  const detectMs = ev.state === "down" ? ev.detectMs : null;
  const closeAckMs = await Promise.race([ringClosed, sleep(3000).then(() => null)]);
  const addon = require(path.join(SPIKE_ROOT, "bridge", "s1_bridge.node")) as typeof Addon;
  const ringExistsAfter = addon.ringExists(ring);
  result.crash = {
    detectMs,
    pipeCloseMs: engine.lastPipeCloseMs,
    closeAckMs,
    ringExistsAfterReaderClose: ringExistsAfter,
  };
  checks.crashDetectedUnder1s = detectMs !== null && detectMs < 1000;
  checks.ringReleased = closeAckMs !== null && !ringExistsAfter;

  // Restart: frames flow again from a new engine generation.
  const restartT0 = performance.now();
  await waitFor("frames after restart", () =>
    js<boolean>("window.__s1.generation >= 2 && window.__s1.framesSinceUp > 10"), 20_000);
  result.restart = { msToFramesFlowing: performance.now() - restartT0, engine: engine.info };
  checks.restartedAndFlowing = true;

  result.checks = checks;
  result.pass = Object.values(checks).every(Boolean);
  const file = path.join(dir, `selftest-${process.platform}-${tag}-${stamp}.json`);
  writeFileSync(file, `${JSON.stringify(result, null, 2)}\n`);
  console.log(`[selftest] ${result.pass ? "PASS" : "FAIL"} -> ${file}`);
  console.log(JSON.stringify({ checks, run, crash: result.crash, cpu: result.cpuPercentOfOneCore }, null, 2));
  engine.stop();
  app.exit(result.pass ? 0 : 1);
}

// ---- lifecycle ------------------------------------------------------------------------------

app.whenReady().then(async () => {
  port = await pickPort();
  protocol.handle("app", serveApp);
  session.defaultSession.setPermissionRequestHandler((_wc, _perm, cb) => cb(false));
  if (!existsSync(ENGINE_BIN)) {
    console.error(`[main] engine binary not found at ${ENGINE_BIN}; run \`pnpm build\` first`);
    app.exit(2);
    return;
  }
  await launch(false);
  mainWindow = createMainWindow();
  if (opts.selftestSeconds !== null) {
    runSelftest(opts.selftestSeconds).catch((e: Error) => {
      console.error(`[selftest] error: ${e.stack ?? e.message}`);
      engine.stop();
      app.exit(1);
    });
  } else if (opts.bench !== undefined) {
    const win = mainWindow;
    const deps = {
      win,
      spikeRoot: SPIKE_ROOT,
      engineInfo: () => engine.info,
      enginePid: () => engine.pid,
      options: opts,
    };
    runBench(deps, opts.bench)
      .then((pass) => {
        engine.stop();
        app.exit(pass ? 0 : 1);
      })
      .catch((e: Error) => {
        console.error(`[bench] error: ${e.stack ?? e.message}`);
        engine.stop();
        app.exit(1);
      });
  }
});

app.on("window-all-closed", () => {
  engine.stop();
  app.quit();
});

app.on("before-quit", () => engine.stop());
