// SPDX-License-Identifier: GPL-3.0-or-later
// WP-0C.02 benchmark / self-test, run from Electron main with `--bench[=quick|<seconds>]`
// (`pnpm bench`). Informal, one machine; WP-0C.04 owns the formal measurement.
//
// Phases (the window must stay visible; real input goes through webContents.sendInputEvent):
//   1. playback matrix at a fixed 1920x1080 engine size: {RGBA8, NV12} x {writeTexture,
//      VideoFrame + importExternalTexture}: fps, drops/min, upload/draw cost, CPU per second;
//   2. playback at the canvas pixel size with the fastest path;
//   3. scripted scrub (pointer drags at several speeds + click jumps on the scrub bar):
//      scrub-to-pixel p50/p95 (input event -> first frame for it on screen), full-res too;
//   4. live window resize (discrete steps + a drag-like sweep): blank-frame instrumentation,
//      captured-pixel flicker check, engine size renegotiation, overlay alignment;
//   5. overlay: elementFromPoint hit tests, real drags of the transform box and its handles,
//      keyboard nudge, deselect on empty overlay, scrub bar reachable.
// Writes results/bench-<platform>-<stamp>.json: per-second metrics
// `{ts, phase, fps, dropped, engineDropped, scrub_ms[], cpu{engine, main, renderer, gpu}, align}`
// plus phase summaries and checks.

import { type ChildProcess, spawn } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { createInterface } from "node:readline";
import { app, type BrowserWindow, screen } from "electron";
import type { EngineInfo } from "./shared/types.ts";

export interface BenchDeps {
  win: BrowserWindow;
  spikeRoot: string;
  engineInfo(): EngineInfo | null;
  enginePid(): number | null;
  options: Record<string, unknown>;
}

interface Lat {
  n: number;
  p50: number;
  p95: number;
  p99: number;
  max: number;
  mean: number;
}

interface Align {
  overlayPx: number;
  tboxPx: number;
  backingPx: number;
  canvas: [number, number];
  frame: [number, number] | null;
  ok: boolean;
  /** Frames the renderer waited for a transient mismatch to settle (0 = aligned at once). */
  settleFrames?: number;
}

interface Sample {
  framesPresented: number;
  seqGaps: number;
  engineDropped: number | null;
  scrubPixelMs: number[];
  renegotiateMs: number[];
  frame: [number, number] | null;
  align: Align;
}

interface Snapshot {
  seconds: number;
  presenter: string;
  framesPresented: number;
  presentFps: number;
  seqGaps: number;
  seqGapsPerMin: number;
  checksumFailures: number;
  rowStampFailures: number;
  seqViolations: number;
  drawErrors: number;
  lastDrawError: string;
  size: string;
  latency: Record<string, Lat>;
  scrub: {
    requests: number;
    resolved: number;
    lost: number;
    pending: number;
    lowresPresented: number;
    latency: Record<string, Lat>;
  };
  engineDropped: number | null;
  stage: {
    canvasWidth: number;
    canvasHeight: number;
    dpr: number;
    resizes: number;
    blankResizes: number;
    scaledFrames: number;
    renegotiateMs: { n: number; p50: number; p95: number; max: number };
    pixelExact: boolean;
    expected: [number, number] | null;
  };
}

interface MetricRow {
  ts: number;
  phase: string;
  fps: number;
  dropped: number;
  engineDropped: number;
  scrub_ms: number[];
  renegotiate_ms: number[];
  cpu: { engine: number | null; main: number; renderer: number; gpu: number };
  frame: [number, number] | null;
  align: Align;
}

interface PhaseResult {
  phase: string;
  seconds: number;
  presentFps: number;
  seqGapsPerMin: number;
  engineDropsPerMin: number;
  dropsPerMin: number;
  cpuMeanPercentOfOneCore: { engine: number | null; main: number; renderer: number; gpu: number };
  alignOk: boolean;
  snapshot: Snapshot;
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const r2 = (x: number) => Math.round(x * 100) / 100;
const pct = (v: number[], p: number) => {
  if (v.length === 0) return Number.NaN;
  const s = [...v].sort((a, b) => a - b);
  return s[Math.min(s.length - 1, Math.floor((p / 100) * s.length))] ?? Number.NaN;
};

/** Engine CPU time (ms) from one long-lived PowerShell sampler (Windows only). */
class EngineCpu {
  ms: number | null = null;
  private child: ChildProcess | null = null;
  start(pid: number | null): void {
    if (pid === null || process.platform !== "win32") return;
    const loop = `while ($true) { try { (Get-Process -Id ${pid} -ErrorAction Stop).TotalProcessorTime.TotalMilliseconds } catch { 'x' }; Start-Sleep -Milliseconds 250 }`;
    this.child = spawn("powershell", ["-NoProfile", "-Command", loop], {
      stdio: ["ignore", "pipe", "ignore"],
      windowsHide: true,
    });
    const child = this.child;
    if (child.stdout) {
      createInterface({ input: child.stdout }).on("line", (l) => {
        const v = Number(l.trim().replace(",", "."));
        this.ms = Number.isFinite(v) ? v : null;
      });
    }
  }
  stop(): void {
    this.child?.kill();
    this.child = null;
  }
}

function electronCpuSeconds(): Record<string, number> {
  const out: Record<string, number> = {};
  for (const m of app.getAppMetrics()) {
    out[m.type] = (out[m.type] ?? 0) + (m.cpu.cumulativeCPUUsage ?? 0);
  }
  return out;
}

export async function runBench(deps: BenchDeps, arg: string): Promise<boolean> {
  const { win } = deps;
  const quick = arg === "quick";
  const playSeconds = quick ? 5 : Number(arg) > 0 ? Number(arg) : 20;
  const scrubSeconds = quick ? 5 : 12;
  const js = <T>(code: string) => win.webContents.executeJavaScript(code) as Promise<T>;
  const metrics: MetricRow[] = [];
  const checks: Record<string, boolean> = {};
  const notes: string[] = [];
  const cpu = new EngineCpu();
  const log = (m: string) => console.log(`[bench] ${m}`);

  // ---- setup -------------------------------------------------------------------------------
  const display = screen.getDisplayMatching(win.getBounds());
  win.show();
  win.focus();
  win.setContentSize(1280, 800);
  const until = Date.now() + 30_000;
  while (!(await js<number>("window.__s1?.framesPresented ?? 0").catch(() => 0))) {
    if (Date.now() > until) throw new Error("no frames presented within 30 s");
    await sleep(100);
  }
  cpu.start(deps.enginePid());
  await js("window.__s1.play()");
  await sleep(3000); // warm-up (GPU init, first resizes)
  // The control channel carries seeks and resizes; without it every later phase is moot.
  const rtt = await js<number | null>(
    "Promise.race([window.__s1.ping(), new Promise((r) => setTimeout(() => r(null), 2000))])",
  );
  checks.controlChannel = rtt !== null;
  log(`control channel: ${rtt === null ? "NO PONG within 2 s" : `rtt ${r2(rtt)} ms`}`);
  if (rtt === null) notes.push("control channel did not answer a ping at start");

  const rects = () =>
    js<{
      dpr: number;
      canvas: { x: number; y: number; w: number; h: number };
      scrub: { x: number; y: number; w: number; h: number };
      tbox: { x: number; y: number; w: number; h: number };
      handles: Record<string, { x: number; y: number; w: number; h: number }>;
    }>("window.__s1.rects()");

  // ---- per-second collection ----------------------------------------------------------------
  async function collect(
    phase: string,
    seconds: number,
    driver?: () => Promise<void>,
  ): Promise<PhaseResult> {
    await js("window.__s1.reset()");
    let prev = await js<Sample>("window.__s1.sample()");
    let prevCpu = { eng: cpu.ms, el: electronCpuSeconds(), t: performance.now() };
    const t0 = performance.now();
    const rows: MetricRow[] = [];
    let driving = driver !== undefined;
    const drive = driver
      ? driver().finally(() => {
          driving = false;
        })
      : Promise.resolve();
    let next = t0 + 1000;
    while (driving || performance.now() < t0 + seconds * 1000) {
      await sleep(Math.max(0, next - performance.now()));
      next += 1000;
      const s = await js<Sample>("window.__s1.sample()");
      const now = { eng: cpu.ms, el: electronCpuSeconds(), t: performance.now() };
      const dt = (now.t - prevCpu.t) / 1000;
      const d = (k: string) => r2((((now.el[k] ?? 0) - (prevCpu.el[k] ?? 0)) / dt) * 100);
      const row: MetricRow = {
        ts: Date.now(),
        phase,
        fps: s.framesPresented - prev.framesPresented,
        dropped: s.seqGaps - prev.seqGaps,
        engineDropped: (s.engineDropped ?? 0) - (prev.engineDropped ?? 0),
        scrub_ms: s.scrubPixelMs,
        renegotiate_ms: s.renegotiateMs,
        cpu: {
          engine:
            now.eng !== null && prevCpu.eng !== null
              ? r2(((now.eng - prevCpu.eng) / 1000 / dt) * 100)
              : null,
          main: d("Browser"),
          renderer: d("Tab"),
          gpu: d("GPU"),
        },
        frame: s.frame,
        align: s.align,
      };
      rows.push(row);
      prev = s;
      prevCpu = now;
    }
    await drive;
    const snap = await js<Snapshot>("window.__s1.snapshot()");
    metrics.push(...rows);
    const secs = (performance.now() - t0) / 1000;
    const engineDrops = rows.reduce((a, r) => a + r.engineDropped, 0);
    const mean = (f: (r: MetricRow) => number | null) => {
      const v = rows.map(f).filter((x): x is number => x !== null);
      return v.length ? r2(v.reduce((a, b) => a + b, 0) / v.length) : null;
    };
    const res: PhaseResult = {
      phase,
      seconds: r2(secs),
      presentFps: snap.presentFps,
      seqGapsPerMin: snap.seqGapsPerMin,
      engineDropsPerMin: r2((engineDrops / secs) * 60),
      dropsPerMin: r2(((snap.seqGaps + engineDrops) / secs) * 60),
      cpuMeanPercentOfOneCore: {
        engine: mean((r) => r.cpu.engine),
        main: mean((r) => r.cpu.main) ?? 0,
        renderer: mean((r) => r.cpu.renderer) ?? 0,
        gpu: mean((r) => r.cpu.gpu) ?? 0,
      },
      alignOk: rows.every((r) => r.align.ok),
      snapshot: snap,
    };
    log(
      `${phase}: ${res.presentFps} fps, drops/min ${res.dropsPerMin} (gaps ${res.seqGapsPerMin}, engine ${res.engineDropsPerMin}), ` +
        `upload+draw p50 ${snap.latency.uploadDrawMs?.p50} ms, gpu done p50 ${snap.latency.gpuDoneMs?.p50} ms, ` +
        `renderer CPU ${res.cpuMeanPercentOfOneCore.renderer}%, ${snap.presenter}`,
    );
    return res;
  }

  const phases: PhaseResult[] = [];
  const integrityOk = (p: PhaseResult) =>
    p.snapshot.checksumFailures === 0 &&
    p.snapshot.rowStampFailures === 0 &&
    p.snapshot.seqViolations === 0 &&
    p.snapshot.drawErrors === 0;

  // ---- 1. playback matrix (fixed 1080p engine size) ----------------------------------------
  await js(`window.__s1.setEngineSize({ width: 1920, height: 1080 })`);
  const matrix: { format: "rgba8" | "nv12"; upload: "writeTexture" | "videoFrame" }[] = [
    { format: "rgba8", upload: "writeTexture" },
    { format: "nv12", upload: "writeTexture" },
    { format: "rgba8", upload: "videoFrame" },
    { format: "nv12", upload: "videoFrame" },
  ];
  const matrixResults: (PhaseResult & { format: string; upload: string })[] = [];
  for (const m of matrix) {
    await js(`window.__s1.setFormat("${m.format}"); window.__s1.setUpload("${m.upload}")`);
    await sleep(1500);
    const r = await collect(`play-1080p-${m.format}-${m.upload}`, playSeconds);
    phases.push(r);
    matrixResults.push({ ...r, format: m.format, upload: m.upload });
  }
  checks.bothUploadPathsPresent = matrixResults.every(
    (r) => r.snapshot.framesPresented > playSeconds * 30 && r.snapshot.presenter.includes(r.upload),
  );
  // Fastest = least upload+draw CPU time (presenting thread) + submit-to-GPU-done time. Not
  // fps: on a shared machine fps mostly reflects producer-side drops, not the upload path.
  const cost = (r: PhaseResult) =>
    (r.snapshot.latency.uploadDrawMs?.p50 ?? 99) + (r.snapshot.latency.gpuDoneMs?.p50 ?? 99);
  const best = [...matrixResults].sort((a, b) => cost(a) - cost(b))[0];
  if (!best) throw new Error("no playback results");
  log(`fastest path: ${best.format} + ${best.upload}`);
  await js(`window.__s1.setFormat("${best.format}"); window.__s1.setUpload("${best.upload}")`);

  // ---- 2. playback at canvas pixel size ------------------------------------------------------
  await js(`window.__s1.setEngineSize("canvas")`);
  await sleep(1500);
  const canvasPlay = await collect(`play-canvas-${best.format}-${best.upload}`, playSeconds);
  phases.push(canvasPlay);
  checks.playback60fps = best.presentFps >= 59.5 && canvasPlay.presentFps >= 59.5;
  checks.dropsUnder2PerMin = best.dropsPerMin < 2 && canvasPlay.dropsPerMin < 2;

  // ---- 3. scripted scrub ------------------------------------------------------------------
  const mouse = (type: "mouseDown" | "mouseUp" | "mouseMove", x: number, y: number) =>
    win.webContents.sendInputEvent({
      type,
      x: Math.round(x),
      y: Math.round(y),
      button: "left",
      clickCount: 1,
      modifiers: type === "mouseMove" ? ["leftbuttondown"] : [],
    });
  let seed = 0x5eed;
  const rand = () => {
    seed = (seed * 1103515245 + 12345) & 0x7fffffff;
    return seed / 0x7fffffff;
  };
  const scrubDriver = async () => {
    const sr = (await rects()).scrub;
    const y = sr.y + sr.h / 2;
    const end = performance.now() + scrubSeconds * 1000 * 0.75;
    const speeds = [2, 6, 15, 40]; // px per move, one move per ~16 ms
    let si = 0;
    while (performance.now() < end) {
      // A drag sweep at one speed (~1.2 s), bouncing between 5 % and 95 % of the bar.
      let x = sr.x + sr.w * (0.05 + 0.9 * rand());
      let dir = rand() < 0.5 ? -1 : 1;
      const v = speeds[si++ % speeds.length] ?? 6;
      mouse("mouseDown", x, y);
      const stop = performance.now() + 1200;
      while (performance.now() < stop) {
        await sleep(16);
        x += dir * v;
        if (x > sr.x + sr.w * 0.95 || x < sr.x + sr.w * 0.05) dir = -dir;
        mouse("mouseMove", x, y);
      }
      mouse("mouseUp", x, y);
      await sleep(150);
    }
    // Click jumps (seek without drag), 250 ms apart.
    const clicksEnd = performance.now() + scrubSeconds * 1000 * 0.25;
    while (performance.now() < clicksEnd) {
      const x = sr.x + sr.w * (0.02 + 0.96 * rand());
      mouse("mouseDown", x, y);
      await sleep(30);
      mouse("mouseUp", x, y);
      await sleep(220);
    }
    await sleep(600); // let the last requests land
  };
  await js("window.__s1.pause()"); // scrubbing a paused preview (the bar also pauses on drag)
  await sleep(300);
  // Variants: the synthetic engine renders a full frame in ~1 ms; real full-resolution frames
  // can take tens of ms (long-GOP decode). The engine emulates that cost (scrub.rs) so the
  // value of "low-res first, then refine" (arch §10 step 5) is visible.
  // The last variant lets the rAF callback wait up to 8 ms for a just-requested frame (see
  // loop.ts `scrubWaitMs`), an opt-in experiment; the default does not wait.
  const scrubVariants = [
    { name: "lowres-first, full cost 0 ms", lowresDiv: 2, costMs: 0, waitMs: 0 },
    { name: "lowres-first, full cost 50 ms", lowresDiv: 2, costMs: 50, waitMs: 0 },
    { name: "full only, full cost 50 ms", lowresDiv: 0, costMs: 50, waitMs: 0 },
    { name: "lowres-first, full cost 0 ms, rAF waits <= 8 ms", lowresDiv: 2, costMs: 0, waitMs: 8 },
  ];
  const scrubResults: {
    name: string;
    lowresDiv: number;
    costMs: number;
    waitMs: number;
    scrub: Snapshot["scrub"];
    fps: number;
  }[] = [];
  for (const v of scrubVariants) {
    await js(`window.__s1.scrubConfig(${v.lowresDiv}, ${v.costMs}); window.__s1.setScrubWait(${v.waitMs})`);
    await sleep(200);
    const tag = `scrub-${v.lowresDiv > 1 ? "lowres" : "fullonly"}-${v.costMs}ms${v.waitMs ? `-wait${v.waitMs}` : ""}`;
    const res = await collect(tag, scrubSeconds, scrubDriver);
    phases.push(res);
    const sl = res.snapshot.scrub;
    scrubResults.push({ ...v, scrub: sl, fps: res.presentFps });
    log(
      `${tag}: ${sl.requests} requests, to pixel p50/p95 ${sl.latency.firstPixelMs?.p50}/${sl.latency.firstPixelMs?.p95} ms, ` +
        `full-res ${sl.latency.fullPixelMs?.p50}/${sl.latency.fullPixelMs?.p95} ms, low-res frames ${sl.lowresPresented}, lost ${sl.lost}`,
    );
  }
  await js("window.__s1.scrubConfig(2, 0); window.__s1.setScrubWait(0)");
  const sl = (scrubResults[0] as (typeof scrubResults)[number]).scrub;
  checks.scrubToPixelP95Under80ms =
    sl.latency.firstPixelMs !== undefined &&
    sl.latency.firstPixelMs.n > 50 &&
    sl.latency.firstPixelMs.p95 < 80;
  checks.scrubAllRequestsAnswered = scrubResults.every(
    (r) => r.scrub.lost === 0 && r.scrub.pending === 0 && r.scrub.requests > 50,
  );
  await js("window.__s1.play()");

  // ---- 4. live resize -----------------------------------------------------------------------
  const work = display.workAreaSize;
  const fit = (w: number, h: number): [number, number] => [
    Math.min(w, work.width - 40),
    Math.min(h, work.height - 80),
  ];
  const captures: { blankFraction: number; size: string }[] = [];
  let staleCaptures = 0;
  let capturing = true;
  const sameRect = (a: { x: number; y: number; w: number; h: number }, b: typeof a) =>
    a.x === b.x && a.y === b.y && a.w === b.w && a.h === b.h;
  /** Sample the canvas region of a captured page: share of near-black / unpainted pixels.
   * A capture is kept only if the canvas rect was the same before and after it (otherwise the
   * window resized in between and the region may include non-canvas pixels). */
  const captureLoop = async () => {
    while (capturing) {
      try {
        const c = (await rects()).canvas;
        const img = await win.webContents.capturePage({
          x: Math.round(c.x),
          y: Math.round(c.y),
          width: Math.max(1, Math.floor(c.w)),
          height: Math.max(1, Math.floor(c.h)),
        });
        if (!sameRect(c, (await rects()).canvas)) {
          staleCaptures++;
          continue;
        }
        const { width, height } = img.getSize();
        const bmp = img.toBitmap(); // BGRA
        let dark = 0;
        let n = 0;
        for (let gy = 0; gy < 24; gy++) {
          for (let gx = 0; gx < 40; gx++) {
            const fx = (gx + 0.5) / 40;
            const fy = (gy + 0.5) / 24;
            if (fx < 0.45 && fy < 0.35) continue; // HUD + the frame's dark glyph box
            const px = Math.floor(fx * width);
            const py = Math.floor(fy * height);
            const o = (py * width + px) * 4;
            const m = Math.max(bmp[o] ?? 0, bmp[o + 1] ?? 0, bmp[o + 2] ?? 0);
            if (m <= 24) dark++;
            n++;
          }
        }
        const blankFraction = r2(dark / Math.max(1, n));
        captures.push({ blankFraction, size: `${width}x${height}` });
        if (blankFraction > 0.01) {
          // Evidence for inspection (results/*.png is git-ignored).
          mkdirSync(path.join(deps.spikeRoot, "results"), { recursive: true });
          writeFileSync(
            path.join(deps.spikeRoot, "results", `bench-blank-${Date.now()}.png`),
            img.toPNG(),
          );
        }
      } catch {
        // window mid-resize; try again
      }
      await sleep(60);
    }
  };
  const resizeSteps: {
    size: [number, number];
    canvas: [number, number];
    frame: [number, number] | null;
    expected: [number, number] | null;
    align: Align;
    ok: boolean;
  }[] = [];
  const resizeDriver = async () => {
    const steps: [number, number][] = [
      [960, 640],
      [1500, 900],
      [1100, 700],
      [1600, 1000],
      [900, 600],
      [1366, 768],
      [1024, 700],
      [1440, 900],
      [1200, 760],
      [1700, 1000],
      [1280, 800],
    ];
    for (const [w, h] of steps) {
      const [cw, ch] = fit(w, h);
      win.setContentSize(cw, ch);
      await sleep(450);
      const st = await js<Snapshot["stage"]>("window.__s1.stage()");
      const a = await js<Align>("window.__s1.alignment()");
      const ok =
        a.ok &&
        st.expected !== null &&
        a.frame !== null &&
        a.frame[0] === st.expected[0] &&
        a.frame[1] === st.expected[1];
      resizeSteps.push({ size: [cw, ch], canvas: a.canvas, frame: a.frame, expected: st.expected, align: a, ok });
    }
    // Drag-like sweep: the width changes every ~16 ms, like an interactive edge drag.
    const [bw, bh] = fit(1280, 800);
    for (let i = 0; i < 120; i++) {
      const dw = Math.round(Math.sin((i / 120) * Math.PI * 2) * 240);
      const dh = Math.round(Math.sin((i / 120) * Math.PI * 4) * 80);
      win.setContentSize(...fit(bw + dw, bh + dh));
      await sleep(16);
    }
    win.setContentSize(bw, bh);
    await sleep(600);
    const a = await js<Align>("window.__s1.alignment()");
    const st = await js<Snapshot["stage"]>("window.__s1.stage()");
    const ok =
      a.ok &&
      st.expected !== null &&
      a.frame !== null &&
      a.frame[0] === st.expected[0] &&
      a.frame[1] === st.expected[1];
    resizeSteps.push({ size: [bw, bh], canvas: a.canvas, frame: a.frame, expected: st.expected, align: a, ok });
  };
  const capture = captureLoop();
  const resize = await collect(`resize-${best.format}-${best.upload}`, 1, resizeDriver);
  capturing = false;
  await capture;
  phases.push(resize);
  const blankCaptures = captures.filter((c) => c.blankFraction > 0.01);
  checks.resizeNoBlankFrames = resize.snapshot.stage.blankResizes === 0 && blankCaptures.length === 0;
  checks.resizeRenegotiatesEngineSize = resizeSteps.every((s) => s.ok);
  log(
    `resize: ${resize.snapshot.stage.resizes} canvas resizes, blank ${resize.snapshot.stage.blankResizes}, ` +
      `captures ${captures.length} (blank ${blankCaptures.length}), renegotiate p50/p95 ` +
      `${resize.snapshot.stage.renegotiateMs.p50}/${resize.snapshot.stage.renegotiateMs.p95} ms, ` +
      `steps ok ${resizeSteps.filter((s) => s.ok).length}/${resizeSteps.length}, ${resize.presentFps} fps`,
  );

  // ---- 5. overlay hit testing + real pointer interaction ---------------------------------
  await js("window.__s1.pause()");
  await sleep(300);
  const hits = await js<{ label: string; hit: string; ok: boolean }[]>("window.__s1.hitTest()");
  checks.overlayHitTests = hits.length > 10 && hits.every((h) => h.ok);
  type Box = { x: number; y: number; w: number; h: number; gestures: number; selected: boolean };
  const tbox = () => js<Box>("window.__s1.tbox()");
  const drag = async (x0: number, y0: number, dx: number, dy: number) => {
    mouse("mouseDown", x0, y0);
    for (let i = 1; i <= 10; i++) {
      await sleep(16);
      mouse("mouseMove", x0 + (dx * i) / 10, y0 + (dy * i) / 10);
    }
    await sleep(16);
    mouse("mouseUp", x0 + dx, y0 + dy);
    await sleep(120);
  };
  const interactions: { name: string; ok: boolean; detail: string }[] = [];
  const near = (a: number, b: number, tolPx: number, sizePx: number) =>
    Math.abs(a - b) <= tolPx / sizePx;
  {
    const r = await rects();
    const b0 = await tbox();
    await drag(r.tbox.x + r.tbox.w / 2, r.tbox.y + r.tbox.h / 2, 80, 40);
    const b1 = await tbox();
    const ok =
      near(b1.x - b0.x, 80 / r.canvas.w, 1, r.canvas.w) &&
      near(b1.y - b0.y, 40 / r.canvas.h, 1, r.canvas.h) &&
      near(b1.w, b0.w, 0.5, r.canvas.w) &&
      b1.gestures === b0.gestures + 1;
    interactions.push({ name: "drag box body by (80, 40) px", ok, detail: JSON.stringify({ b0, b1 }) });
  }
  {
    const r = await rects();
    const se = r.handles.se;
    const b0 = await tbox();
    if (se) await drag(se.x + se.w / 2, se.y + se.h / 2, 60, 30);
    const b1 = await tbox();
    const ok =
      near(b1.w - b0.w, 60 / r.canvas.w, 1, r.canvas.w) &&
      near(b1.h - b0.h, 30 / r.canvas.h, 1, r.canvas.h) &&
      near(b1.x, b0.x, 0.5, r.canvas.w);
    interactions.push({ name: "resize from the se handle by (60, 30) px", ok, detail: JSON.stringify({ b0, b1 }) });
  }
  {
    const r = await rects();
    const nw = r.handles.nw;
    const b0 = await tbox();
    if (nw) await drag(nw.x + nw.w / 2, nw.y + nw.h / 2, -40, -20);
    const b1 = await tbox();
    const ok =
      near(b0.x - b1.x, 40 / r.canvas.w, 1, r.canvas.w) &&
      near(b1.w - b0.w, 40 / r.canvas.w, 1, r.canvas.w) &&
      near(b1.h - b0.h, 20 / r.canvas.h, 1, r.canvas.h);
    interactions.push({ name: "resize from the nw handle by (-40, -20) px", ok, detail: JSON.stringify({ b0, b1 }) });
  }
  {
    const b0 = await tbox();
    for (let i = 0; i < 10; i++) {
      win.webContents.sendInputEvent({ type: "keyDown", keyCode: "Right" });
      win.webContents.sendInputEvent({ type: "keyUp", keyCode: "Right" });
    }
    await sleep(150);
    const b1 = await tbox();
    const ok = near(b1.x - b0.x, 10 / 1920, 0.01, 1);
    interactions.push({ name: "arrow keys nudge the focused box 10 px", ok, detail: JSON.stringify({ b0, b1 }) });
  }
  {
    const r = await rects();
    const x = r.canvas.x + r.canvas.w * 0.12;
    const y = r.canvas.y + r.canvas.h * 0.9;
    mouse("mouseDown", x, y);
    mouse("mouseUp", x, y);
    await sleep(120);
    const deselected = !(await tbox()).selected;
    const tb = (await rects()).tbox;
    mouse("mouseDown", tb.x + tb.w / 2, tb.y + tb.h / 2);
    mouse("mouseUp", tb.x + tb.w / 2, tb.y + tb.h / 2);
    await sleep(120);
    const reselected = (await tbox()).selected;
    interactions.push({
      name: "empty overlay deselects, box click selects",
      ok: deselected && reselected,
      detail: JSON.stringify({ deselected, reselected }),
    });
  }
  {
    const r = await rects();
    const n0 = await js<number>("window.__s1.scrubRequests()");
    mouse("mouseDown", r.scrub.x + r.scrub.w * 0.3, r.scrub.y + r.scrub.h / 2);
    mouse("mouseUp", r.scrub.x + r.scrub.w * 0.3, r.scrub.y + r.scrub.h / 2);
    await sleep(150);
    const n1 = await js<number>("window.__s1.scrubRequests()");
    interactions.push({ name: "scrub bar click seeks", ok: n1 === n0 + 1, detail: `${n0} -> ${n1}` });
  }
  const alignAfter = await js<Align>("window.__s1.alignment()");
  checks.overlayInteraction = interactions.every((i) => i.ok);
  checks.overlayAlignedEverySecond = phases.every((p) => p.alignOk) && alignAfter.ok;
  checks.integrity = phases.every(integrityOk);
  for (const i of interactions) log(`overlay: ${i.ok ? "ok  " : "FAIL"} ${i.name}`);
  for (const h of hits.filter((x) => !x.ok)) log(`hit test FAIL: ${h.label} -> ${h.hit}`);

  // ---- report -------------------------------------------------------------------------------
  cpu.stop();
  const pass = Object.values(checks).every(Boolean);
  const scrubAll = metrics.flatMap((m) => m.scrub_ms);
  const result = {
    kind: "s1-bench",
    wp: "WP-0C.02",
    informal: true,
    startedAt: new Date(metrics[0]?.ts ?? Date.now()).toISOString(),
    platform: `${process.platform}-${process.arch}`,
    electron: process.versions.electron,
    chrome: process.versions.chrome,
    options: deps.options,
    engine: deps.engineInfo(),
    display: { scaleFactor: display.scaleFactor, size: display.size, workArea: work },
    config: { playSeconds, scrubSeconds },
    fastest: { format: best.format, upload: best.upload },
    matrix: matrixResults.map((r) => ({
      format: r.format,
      upload: r.upload,
      presenter: r.snapshot.presenter,
      presentFps: r.presentFps,
      dropsPerMin: r.dropsPerMin,
      seqGapsPerMin: r.seqGapsPerMin,
      engineDropsPerMin: r.engineDropsPerMin,
      uploadDrawMs: r.snapshot.latency.uploadDrawMs,
      gpuDoneMs: r.snapshot.latency.gpuDoneMs,
      renderToPresentMs: r.snapshot.latency.renderToPresentMs,
      addonCopyMs: r.snapshot.latency.addonCopyMs,
      cpu: r.cpuMeanPercentOfOneCore,
    })),
    scrub: {
      variants: scrubResults,
      allVariantsScrubPixelP50: r2(pct(scrubAll, 50)),
      allVariantsScrubPixelP95: r2(pct(scrubAll, 95)),
    },
    control: await js<unknown>("window.__s1.control()"),
    resize: {
      stage: resize.snapshot.stage,
      steps: resizeSteps,
      captures: {
        n: captures.length,
        stale: staleCaptures,
        blank: blankCaptures.length,
        worst: Math.max(0, ...captures.map((c) => c.blankFraction)),
      },
      fps: resize.presentFps,
    },
    overlay: { hitTests: hits, interactions, alignAfter },
    phases,
    metrics,
    checks,
    notes,
    pass,
  };
  const dir = path.join(deps.spikeRoot, "results");
  mkdirSync(dir, { recursive: true });
  const stamp = Date.now();
  const file = path.join(dir, `bench-${process.platform}-${stamp}.json`);
  writeFileSync(file, `${JSON.stringify(result, null, 2)}\n`);
  // Visual evidence (git-ignored): the window as it looked at the end of the run.
  writeFileSync(
    path.join(dir, `bench-${process.platform}-${stamp}.png`),
    (await win.webContents.capturePage()).toPNG(),
  );
  log(`${pass ? "PASS" : "FAIL"} -> ${file}`);
  log(JSON.stringify(checks));
  return pass;
}
