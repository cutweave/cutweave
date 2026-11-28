// SPDX-License-Identifier: GPL-3.0-or-later
// The UI benchmarks (WP-2J.02, arch §19): the built app with the real engine on the generated
// benchmark projects (WP-2J.01), driven like the user flows and measured from outside:
//
// - app start: from the app process's creation (`process.getCreationTime()` in main) to the frame
//   in which the editor shows "Engine connected" on an untitled project; once on a fresh profile
//   and again on the same profile.
// - project open: from the click on File > Open project… (the OS picker is stubbed, so a person's
//   picking time is not in it) to the second animation frame after the editor's document holds
//   every item of the project: the first frame the timeline has drawn it in.
// - memory: the working sets of the app's processes (`app.getAppMetrics()`) and of the engine.
// - a clip dragged at a hand's pace (`handPacedDrag` of the flows): the app's own round-trip probe
//   (User Timing `cutweave.timeline.rtt`: UI, engine, patch, UI), the frame intervals, and the
//   main-thread script and layout time per update (Chromium's counters, `Performance.getMetrics`).
// - the timeline panned with the wheel and zoomed with Ctrl+wheel, events sent at a steady pace
//   whether or not the page keeps up: frame intervals from an animation-frame loop in the page.
//
// Nothing in the app is changed or hooked for this: the numbers come from `performance`, the
// DevTools protocol and what the flows already read. `ui.perf.ts` runs it at full size
// (`pnpm --filter @cutweave/desktop perf:ui`); `flows/perf-smoke.spec.ts` runs it once on a tiny
// project inside the user-flow suite, so it cannot rot.
import { spawnSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import type { CDPSession, ElectronApplication, Page } from "@playwright/test";
import {
  desktopDir,
  editorCall,
  editorHandle,
  editorState,
  expect,
  type LaunchedApp,
  launchApp,
  realEngine,
} from "../fixtures.ts";
import {
  chooseMenu,
  editorCaughtUp,
  engineChip,
  FLICKS,
  type FlowAppOptions,
  flowAppEnv,
  handPacedDrag,
  laneY,
  pickFiles,
  profileDir,
  removeDir,
  startFrameProbe,
  stopFrameProbe,
  type TimelineGeometry,
  timelineCanvas,
  timelineGeometry,
  timelineTools,
  viewSettled,
  type Workspace,
  waitProfileFree,
  xAt,
} from "../flows/helpers.ts";
import {
  type BenchResult,
  buildResult,
  frameStats,
  type Metric,
  median,
  metric,
  quantile,
} from "./result.ts";

export const repoRoot = path.resolve(desktopDir, "..", "..");

/** `bench-fixture.json` of a generated project (`cutweave_bench::fixture::FixtureManifest`). */
export interface FixtureManifest {
  spec: { name: string; items: number; tracks: number; keyframes: number };
  sha256: string;
  sequence: string;
  /** Flicks per frame. */
  frame: number;
  /** The source files, by name inside the project's `media/` folder. */
  media: { file: string }[];
  tracks: {
    id: string;
    video: boolean;
    main: boolean;
    /** Frames: the item and the stretch of track it has to itself. */
    items: { id: string; start: number; duration: number; slot_start: number; slot_len: number }[];
  }[];
}

/** A generated benchmark project: its folder and its manifest. */
export interface BenchProject {
  folder: string;
  manifest: FixtureManifest;
  items: number;
  /** The source files were made (`gen-bench-project --with-media`); else the clips are offline. */
  sources: boolean;
}

export function readProject(folder: string): BenchProject {
  const manifest = JSON.parse(
    readFileSync(path.join(folder, "bench-fixture.json"), "utf8"),
  ) as FixtureManifest;
  return {
    folder,
    manifest,
    items: manifest.tracks.reduce((n, t) => n + t.items.length, 0),
    sources: manifest.media.every((m) => existsSync(path.join(folder, "media", m.file))),
  };
}

/** How much the suite does. */
export interface Scale {
  /** The result's suite name: `ui`, or `ui-smoke` for the run that only proves the suite works. */
  suite: "ui" | "ui-smoke";
  /** Starts on the profile of the first start. */
  warmStarts: number;
  /** Times the one-hour project is opened. */
  opens: number;
  panSeconds: number;
  zoomSeconds: number;
  /** Round trips wanted from the drags. */
  dragUpdates: number;
  /** Pointer moves per drag (60 a second). */
  dragMoves: number;
  maxDrags: number;
}

/** WBS WP-2J.02: a 20 s pan, zoom sweeps, 500 drag updates, a cold and a warm start. */
export const FULL: Scale = {
  suite: "ui",
  warmStarts: 3,
  opens: 3,
  panSeconds: 20,
  zoomSeconds: 10,
  dragUpdates: 500,
  dragMoves: 300,
  maxDrags: 6,
};

/** One short pass over every probe. */
export const SMOKE: Scale = {
  suite: "ui-smoke",
  warmStarts: 0,
  opens: 1,
  panSeconds: 1,
  zoomSeconds: 1,
  dragUpdates: 1,
  dragMoves: 60,
  maxDrags: 1,
};

/** The projects a run opens. */
export interface Projects {
  /** The one-hour, 400-item project (P-400): open time and memory. */
  hour: BenchProject;
  /** The 5,000-item project (P-5K): open time, memory, the drag, pan and zoom. */
  stress: BenchProject;
  /** Opened between two opens of `hour` (P-short); needed when `opens` is more than 1. */
  other?: BenchProject;
}

export interface SuiteOptions {
  scale: Scale;
  projects: Projects;
  /** The projects' sources exist and the engine decodes them (it was built with FFmpeg). */
  realMedia: boolean;
  /**
   * Slow the editor's main thread down by this factor (Chromium's CPU throttling, DevTools
   * protocol) while the timeline probes run: the stand-in for a slowed build, which must fail
   * the budgets. 1: off.
   */
  cpuThrottle: number;
  log(line: string): void;
}

/** Ids every run reports, the smoke run too (it has no warm start). */
export const REQUIRED_IDS = [
  "app_start_ms",
  "project_open_1h_ui_ms",
  "project_open_p5k_ui_ms",
  "app_memory_1h_mb",
  "app_memory_p5k_mb",
  "ui_command_rtt_p95_ms",
  "ui_command_rtt_p50_ms",
  "ui_drag_frame_p95_ms",
  "ui_drag_script_ms_per_update",
  "ui_drag_layout_ms_per_update",
  "timeline_panzoom_5000_fps",
  "timeline_panzoom_5000_frame_p95_ms",
  "timeline_pan_fps",
  "timeline_zoom_fps",
  "ui_idle_fps",
] as const;

/** A start may take this long before the run gives up (a loaded machine: issue #224). */
const START_TIMEOUT_MS = 90_000;
/** So may opening a project. */
const OPEN_TIMEOUT_MS = 180_000;
/** How often a paced wheel event is sent, ms. */
const WHEEL_TICK_MS = 16;
/** One trip of the pan out and back, and of the zoom in and out, s. */
const SWEEP_PERIOD_S = 5;
/** Fastest pan, px a second: a hard flick of the wheel. */
const PAN_PEAK_PX_S = 3000;
/** Fastest zoom, wheel px a second; a tick stays under the 50 px of a wheel notch, like a pinch. */
const ZOOM_PEAK_PX_S = 1200;
/** The pan's second half shows this many px a second or fewer: some hundred clips on screen. */
const OVERVIEW_PX_PER_SEC = 3;

const round = (v: number, digits = 1): number => Math.round(v * 10 ** digits) / 10 ** digits;

/** A throwaway folder the engine may read and write; the projects are copied into it. */
function benchWorkspace(): Workspace {
  const dir = mkdtempSync(path.join(tmpdir(), "cutweave-perf-"));
  const media = path.join(dir, "media");
  mkdirSync(media);
  return { dir, media, file: (name) => path.join(media, name), remove: () => removeDir(dir) };
}

/**
 * A copy of a generated project in the workspace, so a run never changes the generated one. The
 * sources stay where they were made (the document names them by their full path).
 */
function copyProject(project: BenchProject, work: Workspace, name: string): string {
  const dest = path.join(work.dir, name, path.basename(project.folder));
  cpSync(project.folder, dest, {
    recursive: true,
    filter: (source) => path.relative(project.folder, source).split(path.sep)[0] !== "media",
  });
  return dest;
}

interface Started {
  launched: LaunchedApp;
  /** Process creation to "Engine connected" on an untitled project, ms. */
  startMs: number;
  /** Process creation to the window's first contentful paint, ms; NaN when the page has none. */
  firstPaintMs: number;
}

/** Starts the app and times it. */
async function timedStart(
  work: Workspace,
  home: string,
  options: FlowAppOptions,
): Promise<Started> {
  const before = Date.now();
  const launched = await launchApp(flowAppEnv(work, options), [], { home });
  const { app, page } = launched;
  const created = (await app.evaluate(() => process.getCreationTime())) ?? before;
  // Checked every animation frame, in the page: the time is the frame it became true in.
  const ready = await page.waitForFunction(
    () =>
      document.querySelector(".cw-engine-status")?.textContent?.trim() === "Engine connected" &&
      document.title === "Untitled — Cutweave"
        ? performance.timeOrigin + performance.now()
        : 0,
    undefined,
    { polling: "raf", timeout: START_TIMEOUT_MS },
  );
  const readyAt = (await ready.jsonValue()) as number;
  const paintAt = await page.evaluate(() => {
    const entry = performance.getEntriesByName("first-contentful-paint")[0];
    return entry === undefined ? Number.NaN : performance.timeOrigin + entry.startTime;
  });
  // Editable: the editor's own connection is up too.
  await editorHandle(page);
  await expect.poll(async () => (await editorState(page)).connection).toBe("ready");
  await expect(engineChip(page)).toHaveText("Engine connected");
  return { launched, startMs: readyAt - created, firstPaintMs: paintAt - created };
}

declare global {
  /** The page's record of a timed project open (`timedOpen`). */
  var __perfOpen: { up: number; done: number } | undefined;
}

/**
 * File > Open project… on `folder` and the time it took, ms: from the click on the menu item to
 * the second animation frame after the editor's document holds `items` items under the project's
 * title (the frame after the one the timeline drew them in). A project whose source files were
 * not made then says so ("6 media files are missing"); the answer is "Not now", as a person who
 * wants to look at the timeline gives it.
 */
async function timedOpen(
  app: ElectronApplication,
  page: Page,
  folder: string,
  project: BenchProject,
): Promise<number> {
  const items = project.items;
  await pickFiles(app, [folder]);
  await page.evaluate(
    ({ items, name }) => {
      const state = { up: 0, done: 0 };
      globalThis.__perfOpen = state;
      // The last release of the pointer before the project is there is the click on the item.
      const onUp = (event: PointerEvent) => {
        state.up = event.timeStamp;
      };
      document.addEventListener("pointerup", onUp, true);
      const check = () => {
        const held = (globalThis.__cutweaveEditor?.state() as { items: number } | undefined)?.items;
        if (held !== items || !document.title.startsWith(`${name} `)) {
          requestAnimationFrame(check);
          return;
        }
        document.removeEventListener("pointerup", onUp, true);
        requestAnimationFrame(() =>
          requestAnimationFrame(() => {
            state.done = performance.now();
          }),
        );
      };
      requestAnimationFrame(check);
    },
    { items, name: path.basename(folder) },
  );
  await chooseMenu(page, app, "File", "Open project…");
  await page.waitForFunction(() => (globalThis.__perfOpen?.done ?? 0) > 0, undefined, {
    timeout: OPEN_TIMEOUT_MS,
  });
  const { up, done } = await page.evaluate(() => globalThis.__perfOpen ?? { up: 0, done: 0 });
  if (!project.sources) {
    const missing = page.getByRole("dialog", { name: /media files? (is|are) missing/ });
    await expect(missing).toBeVisible();
    await missing.getByRole("button", { name: "Not now" }).click();
    await expect(missing).toBeHidden();
  }
  await expect(timelineCanvas(page)).toBeVisible();
  return done - up;
}

/** Resident memory of process `pid`, MB; NaN when it cannot be read. */
function residentMb(pid: number): number {
  const run =
    process.platform === "win32"
      ? spawnSync(
          "powershell",
          ["-NoProfile", "-NonInteractive", "-Command", `(Get-Process -Id ${pid}).WorkingSet64`],
          { encoding: "utf8", timeout: 30_000 },
        )
      : spawnSync("ps", ["-o", "rss=", "-p", String(pid)], { encoding: "utf8", timeout: 30_000 });
  const value = Number.parseInt((run.stdout ?? "").trim(), 10);
  if (!Number.isFinite(value)) return Number.NaN;
  return process.platform === "win32" ? value / 2 ** 20 : value / 1024;
}

/** The app's memory with a project open: every Electron process and the engine, MB. */
async function appMemory(app: ElectronApplication): Promise<{ electron: number; engine: number }> {
  const { kb, enginePid } = await app.evaluate(({ app }) => ({
    kb: app.getAppMetrics().reduce((sum, p) => sum + p.memory.workingSetSize, 0),
    enginePid: globalThis.__cutweaveTest?.enginePid() ?? null,
  }));
  return { electron: kb / 1024, engine: enginePid === null ? Number.NaN : residentMb(enginePid) };
}

/** Chromium's running totals for the page (seconds, bytes). */
async function pageCounters(cdp: CDPSession): Promise<Record<string, number>> {
  const { metrics } = await cdp.send("Performance.getMetrics");
  return Object.fromEntries(metrics.map((m) => [m.name, m.value]));
}

/** The frame rate of the page when nothing happens: the display's rate, the ceiling of the rest. */
async function idleFps(page: Page): Promise<number> {
  await startFrameProbe(page);
  await page.waitForFunction(
    () => (globalThis as unknown as { __flowFrames: { at: number[] } }).__flowFrames.at.length > 60,
    undefined,
    { timeout: 30_000 },
  );
  return frameStats(await stopFrameProbe(page)).fps;
}

/**
 * Wheel events at a steady pace for `seconds`, not waiting for the page (a person's wheel does
 * not): `delta(t, dt)` is what the wheel moved in the `dt` seconds up to `t`. Returns the page's
 * frame intervals over the sweep.
 */
async function wheelSweep(
  page: Page,
  at: { x: number; y: number },
  seconds: number,
  delta: (t: number, dt: number) => { dx: number; dy: number },
): Promise<number[]> {
  await page.mouse.move(at.x, at.y);
  await startFrameProbe(page);
  const sent: Promise<void>[] = [];
  const began = performance.now();
  let last = 0;
  for (;;) {
    // The timer paces the input; it does not wait for the app.
    await new Promise((resolve) => setTimeout(resolve, WHEEL_TICK_MS));
    const t = Math.min(seconds, (performance.now() - began) / 1000);
    const { dx, dy } = delta(t, t - last);
    last = t;
    sent.push(page.mouse.wheel(dx, dy));
    if (t >= seconds) break;
  }
  await Promise.all(sent);
  return stopFrameProbe(page);
}

/** What the wheel moved between `t - dt` and `t` at a speed of `peak × sin(2πt / period)`. */
const swing = (peak: number, t: number, dt: number): number => {
  const w = (2 * Math.PI) / SWEEP_PERIOD_S;
  return (peak / w) * (Math.cos(w * (t - dt)) - Math.cos(w * t));
};

const viewZoom = async (page: Page): Promise<number> =>
  (await editorCall<{ zoom: number | null }>(page, "view.get")).zoom ?? 100;

/** Clicks the toolbar's Zoom out until `enough(px per second)`; at most `clicks` times. */
async function zoomOutUntil(
  page: Page,
  enough: (pxPerSec: number) => boolean | Promise<boolean>,
  clicks = 14,
): Promise<boolean> {
  const zoomOut = timelineTools(page).getByRole("button", { name: "Zoom out", exact: true });
  for (let n = 0; n < clicks; n += 1) {
    const before = await viewZoom(page);
    if (await enough(before)) return true;
    if (!(await zoomOut.isEnabled())) return false;
    await zoomOut.click();
    await expect.poll(() => viewZoom(page)).toBeLessThan(before);
  }
  return enough(await viewZoom(page));
}

interface DragTarget {
  from: { x: number; y: number };
  reach: number;
  item: string;
}

/**
 * A clip to drag: on a video track that is not the main track (so it moves freely inside the
 * stretch of track it has to itself, touching nothing), in a lane that shows, wide enough to
 * grab, with room to its right for the drag.
 */
function dragTarget(g: TimelineGeometry, manifest: FixtureManifest): DragTarget | null {
  const left = g.box.x + g.headerWidth;
  const right = g.box.x + g.box.width - 8;
  const px = (frames: number) => (frames * manifest.frame * g.pxPerSec) / FLICKS;
  for (const track of manifest.tracks) {
    if (!track.video || track.main) continue;
    const name = g.tracks.find((t) => t.id === track.id)?.name;
    if (name === undefined) continue;
    const y = laneY(g, name);
    if (y < g.box.y + g.rulerHeight + 4 || y > g.box.y + g.box.height - 4) continue;
    for (const item of track.items.slice(0, 40)) {
      const x = xAt(g, item.start * manifest.frame);
      const grab = x + Math.min(px(item.duration) / 2, 20);
      const room = px(item.slot_start + item.slot_len - item.start - item.duration);
      const reach = Math.min(room - 8, 240, right - grab);
      if (px(item.duration) >= 24 && x >= left + 4 && reach >= 60) {
        return { from: { x: grab, y }, reach, item: item.id };
      }
    }
  }
  return null;
}

/** The drag, pan and zoom probes on the open stress project. */
async function timelineProbes(page: Page, options: SuiteOptions): Promise<Metric[]> {
  const { scale, projects, log } = options;
  const manifest = projects.stress.manifest;
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("Performance.enable");
  const idle = await idleFps(page);
  if (options.cpuThrottle > 1) {
    await cdp.send("Emulation.setCPUThrottlingRate", { rate: options.cpuThrottle });
  }

  // ---- a clip dragged at a hand's pace ----
  await viewSettled(page);
  const found: { target: DragTarget | null } = { target: null };
  await zoomOutUntil(page, async () => {
    found.target = dragTarget(await timelineGeometry(page), manifest);
    return found.target !== null;
  });
  if (found.target === null) throw new Error("no clip of the project can be dragged on screen");
  const { from, reach, item } = found.target;
  const started = (await editorCall<{ item: { start: number } }>(page, "item.get", { item })).item
    .start;
  // What the check after each drag costs by itself (it reads the whole timeline back).
  let c0 = await pageCounters(cdp);
  await editorCaughtUp(page);
  let c1 = await pageCounters(cdp);
  const checkCost = {
    script: (c1.ScriptDuration ?? 0) - (c0.ScriptDuration ?? 0),
    layout:
      (c1.LayoutDuration ?? 0) +
      (c1.RecalcStyleDuration ?? 0) -
      (c0.LayoutDuration ?? 0) -
      (c0.RecalcStyleDuration ?? 0),
  };
  const rtt: number[] = [];
  const dragFrames: number[] = [];
  let drags = 0;
  c0 = await pageCounters(cdp);
  while (drags < scale.maxDrags && rtt.length < scale.dragUpdates) {
    const paced = await handPacedDrag(page, from, reach, scale.dragMoves);
    rtt.push(...paced.rtt);
    dragFrames.push(...paced.frames);
    drags += 1;
  }
  c1 = await pageCounters(cdp);
  if (rtt.length === 0) throw new Error("the drag made no update: nothing to measure");
  // The clip is back where it was: the drags left the document as it was.
  expect(
    (await editorCall<{ item: { start: number } }>(page, "item.get", { item })).item.start,
    "the dragged clip's start",
  ).toBe(started);
  const perUpdate = (total: number, check: number): number =>
    (Math.max(0, total - drags * check) * 1000) / rtt.length;
  const script = perUpdate((c1.ScriptDuration ?? 0) - (c0.ScriptDuration ?? 0), checkCost.script);
  const layout = perUpdate(
    (c1.LayoutDuration ?? 0) +
      (c1.RecalcStyleDuration ?? 0) -
      (c0.LayoutDuration ?? 0) -
      (c0.RecalcStyleDuration ?? 0),
    checkCost.layout,
  );
  const drag = frameStats(dragFrames);
  log(
    `drag: ${drags} drag(s), ${rtt.length} updates, round trip p50 ${round(quantile(rtt, 0.5))} ` +
      `p95 ${round(quantile(rtt, 0.95))} max ${round(Math.max(...rtt))} ms; frames ` +
      `${round(drag.fps)} fps, p95 ${round(drag.p95)} ms, max ${round(drag.max)} ms; script ` +
      `${round(script, 2)} ms and layout ${round(layout, 2)} ms per update`,
  );

  // ---- pan: the wheel, sideways, out and back ----
  let g = await timelineGeometry(page);
  const middle = {
    x: g.box.x + g.headerWidth + (g.box.width - g.headerWidth) / 2,
    y: g.box.y + g.rulerHeight + (g.box.height - g.rulerHeight) / 2,
  };
  const pan = (t: number, dt: number) => ({ dx: swing(PAN_PEAK_PX_S, t, dt), dy: 0 });
  const panFrames = await wheelSweep(page, middle, scale.panSeconds / 2, pan);
  // Again with far more clips on screen.
  await zoomOutUntil(page, (pxPerSec) => pxPerSec <= OVERVIEW_PX_PER_SEC);
  await viewSettled(page);
  g = await timelineGeometry(page);
  panFrames.push(...(await wheelSweep(page, middle, scale.panSeconds / 2, pan)));

  // ---- zoom: Ctrl+wheel, in and out, anchored at the pointer ----
  await page.keyboard.down("Control");
  let zoomFrames: number[];
  try {
    zoomFrames = await wheelSweep(page, middle, scale.zoomSeconds, (t, dt) => ({
      dx: 0,
      dy: -swing(ZOOM_PEAK_PX_S, t, dt),
    }));
  } finally {
    await page.keyboard.up("Control");
  }
  if (options.cpuThrottle > 1) await cdp.send("Emulation.setCPUThrottlingRate", { rate: 1 });
  await cdp.detach();

  const panStats = frameStats(panFrames);
  const zoomStats = frameStats(zoomFrames);
  const both = frameStats([...panFrames, ...zoomFrames]);
  log(
    `pan: ${round(panStats.fps)} fps, frame p95 ${round(panStats.p95)} max ${round(panStats.max)} ms ` +
      `(${panStats.frames} frames); zoom: ${round(zoomStats.fps)} fps, frame p95 ` +
      `${round(zoomStats.p95)} max ${round(zoomStats.max)} ms (${zoomStats.frames} frames); ` +
      `idle ${round(idle)} fps`,
  );
  return [
    metric("ui_command_rtt_p95_ms", quantile(rtt, 0.95), "ms", rtt.length),
    metric("ui_command_rtt_p50_ms", quantile(rtt, 0.5), "ms", rtt.length),
    metric("ui_command_rtt_max_ms", Math.max(...rtt), "ms", rtt.length),
    metric("ui_drag_fps", drag.fps, "fps", drag.frames),
    metric("ui_drag_frame_p95_ms", drag.p95, "ms", drag.frames),
    metric("ui_drag_frame_max_ms", drag.max, "ms", drag.frames),
    metric("ui_drag_script_ms_per_update", script, "ms", rtt.length),
    metric("ui_drag_layout_ms_per_update", layout, "ms", rtt.length),
    metric("timeline_panzoom_5000_fps", both.fps, "fps", both.frames),
    metric("timeline_panzoom_5000_frame_p95_ms", both.p95, "ms", both.frames),
    metric("timeline_panzoom_5000_frame_max_ms", both.max, "ms", both.frames),
    metric("timeline_pan_fps", panStats.fps, "fps", panStats.frames),
    metric("timeline_pan_frame_p95_ms", panStats.p95, "ms", panStats.frames),
    metric("timeline_zoom_fps", zoomStats.fps, "fps", zoomStats.frames),
    metric("timeline_zoom_frame_p95_ms", zoomStats.p95, "ms", zoomStats.frames),
    metric("ui_idle_fps", idle, "fps", 60),
  ];
}

/** The graphics card as the window sees it, and the window's size. */
async function displayInfo(page: Page): Promise<Record<string, unknown>> {
  return page.evaluate(() => {
    const gl = document.createElement("canvas").getContext("webgl");
    const info = gl?.getExtension("WEBGL_debug_renderer_info");
    const gpu = gl && info ? (gl.getParameter(info.UNMASKED_RENDERER_WEBGL) as string) : "unknown";
    gl?.getExtension("WEBGL_lose_context")?.loseContext();
    return {
      gpu,
      window: {
        width: window.innerWidth,
        height: window.innerHeight,
        devicePixelRatio: window.devicePixelRatio,
      },
    };
  });
}

/**
 * Runs every probe and returns the result (`bench/result.schema.json`). Every app it starts is
 * quit again, also when a probe throws.
 */
export async function runUiSuite(options: SuiteOptions): Promise<BenchResult> {
  const { scale, projects, log } = options;
  const launch: FlowAppOptions = {
    realMedia: options.realMedia,
    extraRoots: [projects.hour, projects.stress, ...(projects.other ? [projects.other] : [])].map(
      (p) => p.folder,
    ),
  };
  const work = benchWorkspace();
  const home = profileDir();
  const metrics: Metric[] = [];
  const details: Record<string, unknown> = {};
  let electron: string | null = null;
  let open: LaunchedApp | null = null;
  const close = async () => {
    const launched = open;
    open = null;
    await launched?.close();
    await waitProfileFree(home);
  };
  try {
    // ---- start: a fresh profile, then the same profile again ----
    const cold = await timedStart(work, home, launch);
    open = cold.launched;
    electron = await open.app.evaluate(() => process.versions.electron ?? null);
    Object.assign(details, await displayInfo(open.page));
    log(
      `start: ${round(cold.startMs)} ms on a fresh profile (first paint ${round(cold.firstPaintMs)} ms)`,
    );
    metrics.push(metric("app_start_ms", cold.startMs, "ms", 1));
    if (Number.isFinite(cold.firstPaintMs)) {
      metrics.push(metric("app_first_paint_ms", cold.firstPaintMs, "ms", 1));
    }
    const warm: number[] = [];
    for (let n = 0; n < scale.warmStarts; n += 1) {
      await close();
      const again = await timedStart(work, home, launch);
      open = again.launched;
      warm.push(again.startMs);
    }
    if (warm.length > 0) {
      log(`start: ${warm.map((v) => round(v)).join(", ")} ms on the same profile`);
      metrics.push(metric("app_start_warm_ms", median(warm), "ms", warm.length));
    }
    if (open === null) throw new Error("no app is running");

    // ---- the one-hour project: open time and memory ----
    // The smoke run has one project and opens it once: the same open is both numbers.
    const shared = projects.hour.folder === projects.stress.folder;
    const hour = copyProject(projects.hour, work, "hour");
    const opens: number[] = [];
    opens.push(await timedOpen(open.app, open.page, hour, projects.hour));
    await viewSettled(open.page);
    const hourMemory = await appMemory(open.app);
    for (let n = 1; n < scale.opens; n += 1) {
      if (projects.other === undefined) throw new Error("opening twice needs a second project");
      const other = copyProject(projects.other, work, `other-${n}`);
      await timedOpen(open.app, open.page, other, projects.other);
      opens.push(await timedOpen(open.app, open.page, hour, projects.hour));
    }
    log(
      `open ${projects.hour.manifest.spec.name}: ${opens.map((v) => round(v)).join(", ")} ms; memory ` +
        `${round(hourMemory.electron)} MB app + ${round(hourMemory.engine)} MB engine`,
    );
    metrics.push(
      metric("project_open_1h_ui_ms", median(opens), "ms", opens.length),
      metric("project_open_1h_ui_first_ms", opens[0] as number, "ms", 1),
      metric("project_open_1h_ui_max_ms", Math.max(...opens), "ms", opens.length),
      metric("app_memory_1h_mb", hourMemory.electron + hourMemory.engine, "MB", 1),
      metric("engine_memory_1h_mb", hourMemory.engine, "MB", 1),
    );

    // ---- the 5,000-item project: open time, memory, drag, pan and zoom ----
    let stressOpen = opens[0] as number;
    let stressMemory = hourMemory;
    if (!shared) {
      await close();
      open = await launchAndWait(work, home, launch);
      const stress = copyProject(projects.stress, work, "stress");
      stressOpen = await timedOpen(open.app, open.page, stress, projects.stress);
      await viewSettled(open.page);
      stressMemory = await appMemory(open.app);
    }
    const heap = await open.page.evaluate(
      () =>
        (performance as unknown as { memory?: { usedJSHeapSize: number } }).memory
          ?.usedJSHeapSize ?? Number.NaN,
    );
    log(
      `open ${projects.stress.manifest.spec.name}: ${round(stressOpen)} ms; memory ` +
        `${round(stressMemory.electron)} MB app + ${round(stressMemory.engine)} MB engine`,
    );
    metrics.push(
      metric("project_open_p5k_ui_ms", stressOpen, "ms", 1),
      metric("app_memory_p5k_mb", stressMemory.electron + stressMemory.engine, "MB", 1),
      metric("engine_memory_p5k_mb", stressMemory.engine, "MB", 1),
    );
    if (Number.isFinite(heap))
      metrics.push(metric("editor_js_heap_p5k_mb", heap / 2 ** 20, "MB", 1));
    metrics.push(...(await timelineProbes(open.page, options)));
  } finally {
    try {
      await close();
    } finally {
      work.remove();
      removeDir(home);
    }
  }
  const fixture = (p: BenchProject) => ({
    name: p.manifest.spec.name,
    items: p.items,
    tracks: p.manifest.tracks.length,
    keyframes: p.manifest.spec.keyframes,
    sha256: p.manifest.sha256,
    sources: p.sources ? "made" : "missing (clips offline)",
  });
  return buildResult(
    repoRoot,
    scale.suite,
    metrics.filter((m) => Number.isFinite(m.value)),
    electron,
    {
      ...details,
      engine: path.relative(repoRoot, realEngine),
      media: options.realMedia ? "real" : "stub",
      cpuThrottle: options.cpuThrottle,
      projects: [fixture(projects.hour), fixture(projects.stress)],
    },
  );
}

/** Starts the app on `home` and waits for the engine, untimed. */
async function launchAndWait(
  work: Workspace,
  home: string,
  options: FlowAppOptions,
): Promise<LaunchedApp> {
  return (await timedStart(work, home, options)).launched;
}
