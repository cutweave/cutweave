// SPDX-License-Identifier: GPL-3.0-or-later
// A/B benchmark mode shared by transport (a) and (b) (WP-0C.06): `--bench=<seconds>`.
//
// Waits for frames, warms up (`--warmup=<s>`, default 5), resets the page's counters, runs for
// <seconds>, then records present fps, skipped frames/min (seq gaps), engine submit -> present
// submit latency and per-process CPU (renderer, frame-pump utility, GPU, main, engine), writes
// results/bench-*.json and exits. `scripts/bench-ab.mjs` runs the A/B matrix and prints the table.
// Informal numbers: the formal multi-OS measurement is WP-0C.04.

import { execFileSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { app, type BrowserWindow } from "electron";
import type { LatSummary, LoopSnapshot } from "./shared/loop.ts";
import type { EngineInfo } from "./shared/types.ts";

export interface BenchContext {
  win: BrowserWindow;
  transport: "preload" | "utility" | "texture";
  seconds: number;
  warmupSeconds: number;
  engineInfo: () => EngineInfo | null;
  enginePid: () => number | null;
  /** Transport (b) frame-pump utility process. */
  utilityPid: () => number | null;
  /** sharedTexture probe: the offscreen window standing in for the engine. */
  producerPid?: () => number | null;
  options: Record<string, unknown>;
  spikeRoot: string;
  /** Extra per-transport data (e.g. utility stats); called with reset=true before the run. */
  extra?: (reset: boolean) => Promise<Record<string, unknown>>;
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

export async function waitFor<T>(
  what: string,
  fn: () => Promise<T | null | undefined | false>,
  ms: number,
): Promise<T> {
  const until = Date.now() + ms;
  for (;;) {
    const v = await fn().catch(() => null);
    if (v) return v;
    if (Date.now() > until) throw new Error(`timeout waiting for ${what}`);
    await sleep(100);
  }
}

/** Engine CPU time (ms) from the OS; Windows only in the spike. */
export function engineCpuMs(pid: number | null): number | null {
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

export interface ProcSample {
  t: number;
  engineMs: number | null;
  byPid: Map<number, { type: string; cpuS: number; serviceName?: string }>;
}

export function sampleCpu(enginePid: number | null): ProcSample {
  const byPid = new Map<number, { type: string; cpuS: number; serviceName?: string }>();
  for (const m of app.getAppMetrics()) {
    byPid.set(m.pid, {
      type: m.type,
      cpuS: m.cpu.cumulativeCPUUsage ?? 0,
      ...(m.serviceName ? { serviceName: m.serviceName } : {}),
    });
  }
  return { t: performance.now(), engineMs: engineCpuMs(enginePid), byPid };
}

/** CPU % of one core between two samples, for the processes the A/B table cares about. */
export function cpuPercent(
  a: ProcSample,
  b: ProcSample,
  pids: { renderer: number; utility: number | null; producer?: number | null },
): Record<string, number | null> {
  const wallS = (b.t - a.t) / 1000;
  const pct = (s: number) => Math.round((s / wallS) * 1000) / 10;
  const pidPct = (pid: number | null) => {
    if (pid === null) return null;
    const x = a.byPid.get(pid);
    const y = b.byPid.get(pid);
    return x && y ? pct(y.cpuS - x.cpuS) : null;
  };
  const typePct = (type: string) => {
    let s = 0;
    for (const [pid, y] of b.byPid) {
      if (y.type !== type) continue;
      s += y.cpuS - (a.byPid.get(pid)?.cpuS ?? y.cpuS);
    }
    return pct(s);
  };
  return {
    renderer: pidPct(pids.renderer),
    utility: pidPct(pids.utility),
    ...(pids.producer === undefined ? {} : { producer: pidPct(pids.producer) }),
    gpu: typePct("GPU"),
    main: typePct("Browser"),
    engine:
      a.engineMs !== null && b.engineMs !== null
        ? Math.round(((b.engineMs - a.engineMs) / 1000 / wallS) * 1000) / 10
        : null,
  };
}

const pick = (l: LatSummary | undefined) => (l ? { p50: l.p50, p95: l.p95 } : null);

export async function runBench(ctx: BenchContext): Promise<boolean> {
  const { win } = ctx;
  const js = <T>(code: string) => win.webContents.executeJavaScript(code) as Promise<T>;
  await waitFor("renderer frames", () => js<number>("window.__s1?.framesPresented ?? 0"), 30_000);
  await sleep(ctx.warmupSeconds * 1000);
  // The ring's engine_dropped counter is cumulative since engine start: take the delta.
  const dropped0 = await js<number | null>(
    "(async () => (await window.__s1.snapshot()).engineDropped ?? null)()",
  );
  await js("window.__s1.reset()");
  await ctx.extra?.(true);
  const rendererPid = win.webContents.getOSProcessId();
  const c0 = sampleCpu(ctx.enginePid());
  await sleep(ctx.seconds * 1000);
  const snap = await js<LoopSnapshot & Record<string, unknown>>("window.__s1.snapshot()");
  const c1 = sampleCpu(ctx.enginePid());
  const extra = (await ctx.extra?.(false)) ?? {};
  const cpu = cpuPercent(c0, c1, {
    renderer: rendererPid,
    utility: ctx.utilityPid(),
    ...(ctx.producerPid ? { producer: ctx.producerPid() } : {}),
  });
  const info = ctx.engineInfo();
  const L = snap.latency;
  // OS view of the processes that touch frames (security comparison in the README).
  const osOf = (pid: number | null) => {
    const m = app.getAppMetrics().find((x) => x.pid === pid);
    return m ? { sandboxed: m.sandboxed ?? null, integrityLevel: m.integrityLevel ?? null } : null;
  };
  const os = { renderer: osOf(rendererPid), utility: osOf(ctx.utilityPid()) };
  const summary = {
    transport: ctx.transport,
    drawIn: snap.drawIn,
    policy: snap.policy,
    presenter: snap.presenter,
    size: snap.size,
    format: snap.format === 2 ? "nv12" : "rgba8",
    seconds: snap.seconds,
    presentFps: snap.presentFps,
    skippedPerMin: snap.seqGapsPerMin,
    skipped: snap.seqGaps,
    longRafs: snap.longRafs,
    tornFrames: snap.checksumFailures + snap.rowStampFailures,
    seqViolations: snap.seqViolations,
    renderToPresentMs: pick(L.renderToPresentMs),
    publishToPresentMs: pick(L.publishToPresentMs),
    uploadDrawMs: pick(L.uploadDrawMs),
    cpu,
    os,
    /** Frames the engine itself skipped during the run (producer stalls, not transport). */
    engineDropped:
      typeof snap.engineDropped === "number" && typeof dropped0 === "number"
        ? snap.engineDropped - dropped0
        : null,
  };
  const result = {
    kind: "s1-bench",
    informal: true,
    startedAt: new Date().toISOString(),
    platform: `${process.platform}-${process.arch}`,
    electron: process.versions.electron,
    chrome: process.versions.chrome,
    options: ctx.options,
    engine: info,
    summary,
    snapshot: snap,
    extra,
  };
  const dir = path.join(ctx.spikeRoot, "results");
  mkdirSync(dir, { recursive: true });
  const tag = `${ctx.transport}-${snap.drawIn.split(" ")[0]}-${snap.size}-${summary.format}`;
  const file = path.join(dir, `bench-${process.platform}-${tag}-${Date.now()}.json`);
  writeFileSync(file, `${JSON.stringify(result, null, 2)}\n`);
  console.log(`[bench] ${file}`);
  console.log(`[bench] ${JSON.stringify(summary)}`);
  return summary.presentFps > 0;
}
