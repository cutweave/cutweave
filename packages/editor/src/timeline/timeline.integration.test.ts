// SPDX-License-Identifier: GPL-3.0-or-later
// WP-2A.10 end to end against a REAL headless engine (`--dev-stub-media`, so imported files
// become clips): pointer and key input goes through a real `TimelineView` (hit-testing on the
// engine's live sequence), the interaction state machine of WP-2A.08 and `TimelineHost`, out as
// engine commands and gesture transactions, and back through `doc.patched` into the stores. Only
// the canvas drawing is stubbed. Skips with a message when the engine cannot be built.
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { RpcClient, RpcConnection, type SequenceId, type TrackId } from "@cutweave/protocol";
import {
  type EngineHandle,
  resolveEngineBinary,
  startEngine,
} from "@cutweave/protocol/test-support/engine";
import { mathModuleBytes } from "@cutweave/protocol/test-support/math-wasm";
import { initMath } from "@cutweave/protocol/wasm";
import {
  type MachineAction,
  type Theme,
  type TimelineItem,
  type TimelinePointerEvent,
  TimelineView,
  type Tool,
} from "@cutweave/timeline";
import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { createEngineStore } from "../engine/engineStore.ts";
import { type EngineRuntime, startEngineRuntime } from "../engine/runtime.ts";
import { i18n } from "../i18n/index.ts";
import { createDocStore } from "../state/docStore.ts";
import { createJobStore } from "../state/jobStore.ts";
import { createPlaybackStore } from "../state/playbackStore.ts";
import { selectTimelineModel } from "../state/selectors.ts";
import { createToastStore } from "../state/toastStore.ts";
import { createViewStore } from "../state/viewStore.ts";
import { translator } from "./errors.ts";
import { TimelineHost } from "./host.ts";
import { createLocksStore, LocksFeed } from "./locks.ts";
import { quantile } from "./rttProbe.ts";

const F = 705_600_000;
const sec = (s: number) => Math.round(s * F);

let skipReason: string | undefined;
let handle: EngineHandle | undefined;
let runtime: EngineRuntime;
let rpc: RpcClient;
let mediaDir: string;
const errors: unknown[] = [];
const stores = {
  doc: createDocStore(),
  view: createViewStore(),
  playback: createPlaybackStore(),
  jobs: createJobStore(),
  engine: createEngineStore(),
};
const toasts = createToastStore();
let host: TimelineHost;
let view: TimelineView;
/** Longest synchronous handler run (pointer and key handlers, model updates), ms. */
let longestHandlerMs = 0;

// ---- a headless TimelineView ----------------------------------------------------------------

/** Token stand-ins: nothing is drawn, only the sizes matter for hit-testing. */
function theme(): Theme {
  const c = { strong: "var(--clip-media)", body: "var(--clip-media-body)" };
  return {
    surface0: "var(--surface-0)",
    surface1: "var(--surface-1)",
    surface3: "var(--surface-3)",
    lane: "var(--timeline-lane)",
    laneAlt: "var(--timeline-lane-alt)",
    borderSubtle: "var(--border-subtle)",
    textPrimary: "var(--text-primary)",
    textSecondary: "var(--text-secondary)",
    textMuted: "var(--text-muted)",
    playhead: "var(--playhead)",
    onMarigold: "var(--on-marigold)",
    marigold: "var(--marigold)",
    marigoldSubtle: "var(--marigold-subtle)",
    marigoldText: "var(--marigold-text)",
    agent: "var(--agent)",
    agentSubtle: "var(--agent-subtle)",
    clip: { media: c, audio: c, graphics: c, captions: c, effects: c },
    fontCaption: "var(--font-ui)",
    fontLabel: "var(--font-ui)",
    fontNumeric: "var(--font-mono)",
    captionPx: 11,
    numericPx: 12,
    opacityDisabled: 0.5,
    borderThin: 1,
    clipStrip: 3,
    radiusXs: 2,
    radiusSm: 4,
    controlSm: 24,
    controlMd: 28,
    iconSm: 14,
    trackSm: 32,
    trackMd: 48,
    trackLg: 72,
    space0_5: 2,
    space1: 4,
    space1_5: 6,
    space2: 8,
    space4: 16,
    space12: 48,
  };
}

// A canvas context that draws nothing; text measures 0 wide. A resize draws at once (#163).
const noopContext = new Proxy(
  {},
  { get: (_t, prop) => (prop === "measureText" ? () => ({ width: 0 }) : () => undefined) },
) as CanvasRenderingContext2D;
const WIDTH = 1600;
const HEIGHT = 400;
const HEADER = 4 * 48; // 4 x space-12
const RULER = 28; // control-md
const LANE = 48 + 2 * 4; // track-md + 2 x space-1
/** Canvas x of timeline time `t` at 100 px/s, no scroll. */
const xOf = (t: number) => HEADER + (t / F) * 100;

function timed<T>(fn: () => T): T {
  const t0 = performance.now();
  try {
    return fn();
  } finally {
    longestHandlerMs = Math.max(longestHandlerMs, performance.now() - t0);
  }
}

// ---- engine and stores ----------------------------------------------------------------------

beforeAll(async () => {
  const binary = resolveEngineBinary();
  if (binary.unavailable !== undefined) {
    skipReason = binary.unavailable;
    console.warn(`timeline interaction e2e tests skipped: ${skipReason}`);
    return;
  }
  await initMath(mathModuleBytes());
  mediaDir = mkdtempSync(join(tmpdir(), "cutweave-timeline-e2e-"));
  // Generated here, never committed: the stub backend probes by extension (10 s, video + audio).
  writeFileSync(join(mediaDir, "clip.mp4"), Buffer.alloc(4096, 7));
  handle = await startEngine(binary.path, ["--dev-stub-media", "--allow-root", mediaDir]);
  const info = handle.info;
  runtime = startEngineRuntime(
    { getConnection: () => Promise.resolve({ ...info, port: handle?.port ?? 0 }) },
    stores,
    { view: { debounceMs: 50 }, onError: (e) => errors.push(e) },
  );
  rpc = runtime.rpc;
  await runtime.connection.whenReady();
  await until(() => stores.doc.getState().status === "ready");

  view = new TimelineView({ width: 0, height: 0, style: {} } as unknown as HTMLCanvasElement, {
    theme: theme(),
    context: noopContext,
    requestFrame: () => 1,
    cancelFrame: () => undefined,
  });
  view.resize(WIDTH, HEIGHT, 1);
  // What `TimelineCanvas` does in the app: follow the document and the view store.
  stores.doc.subscribe((s) => timed(() => view.setModel(selectTimelineModel(s.project))));
  stores.view.subscribe((s) =>
    timed(() =>
      view.setViewState({
        pxPerSec: s.zoom ?? 100,
        scrollX: s.scroll.x,
        scrollY: s.scroll.y,
        playhead: s.playhead,
        selection: new Set(s.selection.map((r) => r.id)),
      }),
    ),
  );
  host = new TimelineHost({
    doc: stores.doc,
    view: stores.view,
    engine: stores.engine,
    toasts,
    t: translator(i18n.t.bind(i18n)),
  });
  host.attach(view, null, () => ({ left: HEADER, right: WIDTH, top: RULER, bottom: HEIGHT }));
  host.start();
}, 20 * 60_000);

afterAll(async () => {
  host?.dispose();
  await runtime?.stop();
  await handle?.stop();
  if (mediaDir !== undefined) rmSync(mediaDir, { recursive: true, force: true });
});

async function until(condition: () => boolean, timeoutMs = 10_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!condition()) {
    if (Date.now() > deadline) throw new Error("timed out waiting for the engine and the stores");
    await sleep(5);
  }
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const model = () => selectTimelineModel(stores.doc.getState().project);
const itemById = (id: string): TimelineItem | undefined => model().items.find((i) => i.id === id);
const settledDoc = () => stores.doc.getState().rev === stores.doc.getState().committedRev;
const historyLength = async () => (await rpc.call("edit.history", { limit: 500 })).entries.length;
const openTransactions = async () => (await rpc.call("edit.locks", {})).transactions.length;
/** Canvas y of the middle of a track's lane (tracks added by a test shift the others). */
const laneOf = (track: string) => {
  const i = model().tracks.findIndex((t) => t.id === track);
  if (i < 0) throw new Error(`no track ${track}`);
  return RULER + i * LANE + LANE / 2;
};
const onTrack = (track: string) =>
  model()
    .items.filter((i) => i.trackId === track)
    .sort((a, b) => a.start - b.start);
const lastLabel = async () => (await rpc.call("edit.history", { limit: 1 })).entries[0]?.label;

interface Seeded {
  sequence: SequenceId;
  video: TrackId;
  audio: TrackId;
  /** The video clip and its linked audio clip. */
  clip: string;
  partner: string;
  media: string;
}

let project: Omit<Seeded, "clip" | "partner"> | undefined;

/** An empty timeline: one project for the file, its items and extra tracks cleared per test. */
async function clear(): Promise<Omit<Seeded, "clip" | "partner">> {
  if (project === undefined) {
    const created = await rpc.call("project.new", { name: "Interactions" });
    const imported = await rpc.call("media.import", { paths: [join(mediaDir, "clip.mp4")] });
    expect(imported.failed).toEqual([]);
    project = {
      sequence: created.sequence,
      video: created.video_track,
      audio: created.audio_track,
      media: imported.media_ids[0] as string,
    };
  }
  const leftovers = model().items.map((i) => i.id);
  if (leftovers.length > 0) await rpc.call("item.delete", { items: leftovers as never });
  const p = project;
  for (const track of model().tracks) {
    if (track.id !== p.video && track.id !== p.audio) {
      await rpc.call("track.remove", { track: track.id as TrackId });
    }
  }
  await rpc.call("track.set", { track: p.video, locked: false });
  await rpc.call("track.set_magnetic", { track: p.video, magnetic: false });
  await until(() => model().items.length === 0 && model().tracks.length === 2 && settledDoc());
  return p;
}

/** One 10 s stub file inserted at 1 s for 4 s (video + linked audio). */
async function seed(sourceIn = 0): Promise<Seeded> {
  const p = await clear();
  const inserted = await rpc.call("timeline.insert", {
    sequence: p.sequence,
    track: p.video,
    media: p.media as never,
    at: "1s",
    duration: "4s",
    source_in: sourceIn,
  });
  await until(() => model().items.length === 2 && settledDoc());
  const [clip, partner] = inserted.clips;
  if (clip === undefined || partner === undefined) throw new Error("expected video + linked audio");
  return { ...p, clip, partner };
}

/** Unlinked video clips on the video track: `[start, duration]` in seconds, `sourceIn` 2 s. */
async function clips(...spans: [number, number][]): Promise<string[]> {
  const p = await clear();
  const ids: string[] = [];
  for (const [at, duration] of spans) {
    const out = await rpc.call("timeline.insert", {
      sequence: p.sequence,
      track: p.video,
      media: p.media as never,
      at: sec(at),
      duration: sec(duration),
      source_in: sec(2),
      link: false,
    });
    ids.push(out.clips[0] as string);
  }
  await until(() => model().items.length === spans.length && settledDoc());
  return ids;
}

/** One pointer event through the real hit-test and the host, like `TimelineCanvas`. */
function pointer(
  type: TimelinePointerEvent["type"],
  x: number,
  y: number,
  mods: Partial<Pick<TimelinePointerEvent, "altKey" | "shiftKey">> = {},
): void {
  const hit = view.hitTest(x, y);
  timed(() =>
    host.handlePointer({
      type,
      x,
      y,
      hit,
      button: 0,
      buttons: type === "up" ? 0 : 1,
      shiftKey: mods.shiftKey ?? false,
      ctrlKey: false,
      altKey: mods.altKey ?? false,
      metaKey: false,
      original: {} as MouseEvent,
    }),
  );
}

function key(k: string, action?: MachineAction | null, shiftKey = false): boolean {
  return timed(() => host.handleKey({ key: k, shiftKey }, true, action));
}

function click(x: number, y: number, mods: Partial<Pick<TimelinePointerEvent, "shiftKey">> = {}) {
  pointer("down", x, y, mods);
  pointer("up", x, y, mods);
}

/** Press at (x0, y), move in `steps` pointer moves `ms` apart to (x1, y), release. */
async function drag(x0: number, x1: number, y: number, steps = 10, ms = 8): Promise<void> {
  pointer("down", x0, y);
  for (let i = 1; i <= steps; i++) {
    pointer("move", x0 + ((x1 - x0) * i) / steps, y);
    await sleep(ms);
  }
  pointer("up", x1, y);
  await host.settled();
}

function useTool(tool: Tool): void {
  host.setTool(tool);
  expect(host.ui.getState().tool).toBe(tool);
}

function live(name: string, body: () => Promise<void>): void;
function live(name: string, options: { retry: number }, body: () => Promise<void>): void;
function live(
  name: string,
  a: (() => Promise<void>) | { retry: number },
  b?: () => Promise<void>,
): void {
  const body = typeof a === "function" ? a : (b as () => Promise<void>);
  const retry = typeof a === "function" ? 0 : a.retry;
  it(name, { timeout: 120_000, retry }, async (ctx) => {
    if (skipReason !== undefined) {
      ctx.skip();
      return;
    }
    await body();
  });
}

beforeEach(() => {
  if (skipReason !== undefined) return;
  toasts.getState().clear();
  stores.view.setState({ zoom: 100, scroll: { x: 0, y: 0 }, playhead: 0, selection: [] });
  stores.view.getState().setSnapping(true);
  if (host.ui.getState().tool !== "select") host.setTool("select");
  if (!host.ui.getState().linkedSelection) host.toggleLinkedSelection();
});

describe("timeline interactions against the real engine", () => {
  live("dragging a clip 100 px moves it 1 s, frame-aligned, as one undo step", async () => {
    const s = await seed();
    const before = await historyLength();
    const frame = Math.round((F * model().fps.den) / model().fps.num);
    const mid = xOf(sec(3));

    await drag(mid, mid + 100, laneOf(s.video));
    await until(() => itemById(s.clip)?.start === sec(2) && settledDoc());

    expect(itemById(s.clip)?.start).toBe(sec(2));
    expect((itemById(s.clip)?.start ?? 1) % frame).toBe(0);
    expect(itemById(s.partner)?.start).toBe(sec(2)); // the linked audio follows
    expect(
      stores.view
        .getState()
        .selection.map((r) => r.id)
        .sort(),
    ).toEqual([s.clip, s.partner].sort());
    expect(await historyLength()).toBe(before + 1);
    const [entry] = (await rpc.call("edit.history", { limit: 1 })).entries;
    expect(entry).toMatchObject({ label: "Move clip", kind: "gesture" });

    await rpc.call("edit.undo", {});
    await until(() => itemById(s.clip)?.start === sec(1));
    expect(itemById(s.partner)?.start).toBe(sec(1));
    expect(toasts.getState().notices).toEqual([]);
  });

  live("trimming the end edge shortens the clip and its partner by whole frames", async () => {
    const s = await seed();
    const before = await historyLength();
    const right = xOf(sec(5)) - 2;

    await drag(right, right - 100, laneOf(s.video));
    await until(() => itemById(s.clip)?.duration === sec(3) && settledDoc());

    expect(itemById(s.clip)).toMatchObject({ start: sec(1), duration: sec(3) });
    expect(itemById(s.partner)?.duration).toBe(sec(3));
    expect(await historyLength()).toBe(before + 1);
    expect(await lastLabel()).toBe("Trim clip");

    await rpc.call("edit.undo", {});
    await until(() => itemById(s.clip)?.duration === sec(4));
  });

  live(
    "trimming the start edge moves start and source position together, down to the media's first frame",
    async () => {
      const s = await seed(sec(1)); // reads the file from 1 s
      const before = await historyLength();
      const left = xOf(sec(1)) + 2;

      await drag(left, left + 50, laneOf(s.video));
      await until(() => itemById(s.clip)?.start === sec(1.5) && settledDoc());
      expect(itemById(s.clip)).toMatchObject({ duration: sec(3.5) });
      expect(itemById(s.clip)?.source?.sourceIn).toBe(sec(1.5));

      // Far past the media's first frame: the edge stops where the file starts (timeline 0 s).
      const edge = xOf(sec(1.5)) + 2;
      await drag(edge, HEADER + 1, laneOf(s.video), 20);
      await until(() => itemById(s.clip)?.start === 0 && settledDoc());
      expect(itemById(s.clip)).toMatchObject({ duration: sec(5) });
      expect(itemById(s.clip)?.source?.sourceIn).toBe(0);
      expect(await historyLength()).toBe(before + 2);
      expect(toasts.getState().notices).toEqual([]);
    },
  );

  live("trimming beyond the media's length stops at its last frame", async () => {
    const s = await seed(sec(1)); // 1 s .. 5 s of a 10 s file: the end can reach 10 s
    const right = xOf(sec(5)) - 2;

    await drag(right, WIDTH - 40, laneOf(s.video), 20);
    await until(() => itemById(s.clip)?.duration === sec(9) && settledDoc());
    expect(itemById(s.clip)).toMatchObject({ start: sec(1), duration: sec(9) });
    expect(toasts.getState().notices).toEqual([]);
  });

  live("S splits the clip under the playhead at a frame; the left half keeps its id", async () => {
    const s = await seed();
    const before = await historyLength();
    stores.view.getState().setPlayhead(sec(2.5) + 1234); // between frames

    expect(key("s", "split")).toBe(true);
    await host.settled();
    await until(() => model().items.length === 4 && settledDoc());

    const left = itemById(s.clip);
    expect(left).toMatchObject({ start: sec(1), duration: sec(1.5) });
    const right = model().items.find((i) => i.trackId === s.video && i.id !== s.clip);
    expect(right).toMatchObject({ start: sec(2.5), duration: sec(2.5) });
    expect(right?.source?.sourceIn).toBe(sec(1.5));
    expect(model().items.filter((i) => i.trackId === s.audio)).toHaveLength(2); // partner split
    expect(await historyLength()).toBe(before + 1);

    await rpc.call("edit.undo", {});
    await until(() => model().items.length === 2);
    expect(itemById(s.clip)?.duration).toBe(sec(4));
  });

  live("the razor splits the clip and its sound where it is clicked", async () => {
    const s = await seed();
    useTool("razor");
    const before = await historyLength();
    click(xOf(sec(3)), laneOf(s.video));
    await host.settled();
    await until(() => model().items.length === 4 && settledDoc());
    expect(itemById(s.clip)).toMatchObject({ start: sec(1), duration: sec(2) });
    expect(onTrack(s.audio).map((i) => i.start)).toEqual([sec(1), sec(3)]);
    expect(await historyLength()).toBe(before + 1);
  });

  live("Delete removes the selected clip and its partner; gaps stay", async () => {
    const s = await seed();
    const later = await rpc.call("timeline.insert", {
      sequence: s.sequence,
      track: s.video,
      media: s.media as never,
      at: "7s",
      duration: "1s",
      link: false,
    });
    await until(() => model().items.length === 3 && settledDoc());
    const before = await historyLength();

    click(xOf(sec(3)), laneOf(s.video));
    expect(
      stores.view
        .getState()
        .selection.map((r) => r.id)
        .sort(),
    ).toEqual([s.clip, s.partner].sort());
    expect(key("Delete", "delete")).toBe(true);
    await host.settled();
    await until(() => model().items.length === 1);

    expect(model().items[0]).toMatchObject({ id: later.clips[0], start: sec(7) });
    expect(stores.view.getState().selection).toEqual([]);
    expect(await historyLength()).toBe(before + 1);
  });

  live("Shift+Delete on the magnetic main track leaves no gap, as one undo step", async () => {
    const [a, b, c] = await clips([0, 1], [1, 1], [2, 1]);
    const p = project as NonNullable<typeof project>;
    await rpc.call("track.set_magnetic", { track: p.video, magnetic: true });
    await until(() => model().tracks.find((t) => t.id === p.video)?.main === true);
    const before = await historyLength();
    click(xOf(sec(1.5)), laneOf(p.video));
    expect(stores.view.getState().selection).toEqual([{ kind: "item", id: b }]);
    expect(key("Delete", "ripple_delete", true)).toBe(true);
    await host.settled();
    await until(() => model().items.length === 2 && settledDoc());
    expect(onTrack(p.video).map((i) => [i.id, i.start])).toEqual([
      [a, 0],
      [c, sec(1)],
    ]);
    expect(await historyLength()).toBe(before + 1);
    await rpc.call("edit.undo", {});
    await until(() => model().items.length === 3);
    expect(itemById(c as string)?.start).toBe(sec(2));
  });

  live("the ripple tool trims an edge and moves what follows by the same amount", async () => {
    const [a, b] = await clips([1, 2], [4, 2]);
    const p = project as NonNullable<typeof project>;
    useTool("ripple");
    const end = xOf(sec(3)) - 2;
    await drag(end, end - 100, laneOf(p.video));
    await until(() => itemById(a as string)?.duration === sec(1) && settledDoc());
    expect(itemById(b as string)?.start).toBe(sec(3));
    expect(await lastLabel()).toBe("Ripple trim");
  });

  live("the roll tool moves a cut and keeps the total length", async () => {
    const [a, b] = await clips([1, 2], [3, 2]);
    const p = project as NonNullable<typeof project>;
    useTool("roll");
    const cut = xOf(sec(3)) - 2; // the left clip's end edge, at the cut
    await drag(cut, cut + 50, laneOf(p.video));
    await until(() => itemById(a as string)?.duration === sec(2.5) && settledDoc());
    expect(itemById(b as string)).toMatchObject({ start: sec(3.5), duration: sec(1.5) });
    expect(itemById(b as string)?.source?.sourceIn).toBe(sec(2.5));
    expect(await lastLabel()).toBe("Roll edit");
  });

  live("the slip tool changes only the media a clip shows", async () => {
    const [a] = await clips([1, 2]);
    const p = project as NonNullable<typeof project>;
    useTool("slip");
    const mid = xOf(sec(2));
    await drag(mid, mid + 50, laneOf(p.video)); // drag right: earlier media
    await until(() => itemById(a as string)?.source?.sourceIn === sec(1.5) && settledDoc());
    expect(itemById(a as string)).toMatchObject({ start: sec(1), duration: sec(2) });
    expect(await lastLabel()).toBe("Slip clip");
  });

  live("the slide tool moves a clip between its neighbours, which give and take", async () => {
    const [a, b, c] = await clips([1, 2], [3, 2], [5, 2]);
    const p = project as NonNullable<typeof project>;
    useTool("slide");
    const mid = xOf(sec(4));
    await drag(mid, mid + 50, laneOf(p.video));
    await until(() => itemById(b as string)?.start === sec(3.5) && settledDoc());
    expect(itemById(a as string)).toMatchObject({ start: sec(1), duration: sec(2.5) });
    expect(itemById(b as string)).toMatchObject({ duration: sec(2) });
    expect(itemById(c as string)).toMatchObject({ start: sec(5.5), duration: sec(1.5) });
    expect(await lastLabel()).toBe("Slide clip");
  });

  live("a marquee over the lanes selects the clips it touches", async () => {
    const [a, b] = await clips([1, 1], [3, 1]);
    const p = project as NonNullable<typeof project>;
    const y = laneOf(p.video);
    // From the empty lane before the first clip, across both.
    pointer("down", xOf(sec(0.5)), y - 10);
    for (let i = 1; i <= 6; i++) pointer("move", xOf(sec(0.5)) + i * 60, y + 10);
    expect(host.ui.getState().marquee).not.toBeNull();
    pointer("up", xOf(sec(0.5)) + 360, y + 10);
    expect(host.ui.getState().marquee).toBeNull();
    expect(
      stores.view
        .getState()
        .selection.map((r) => r.id)
        .sort(),
    ).toEqual([a, b].sort());
  });

  live("with linked selection off, a drag moves the picture without its sound", async () => {
    const s = await seed();
    host.toggleLinkedSelection();
    expect(host.ui.getState().linkedSelection).toBe(false);
    const mid = xOf(sec(3));
    click(mid, laneOf(s.video));
    expect(stores.view.getState().selection).toEqual([{ kind: "item", id: s.clip }]);
    await drag(mid, mid + 100, laneOf(s.video));
    await until(() => itemById(s.clip)?.start === sec(2) && settledDoc());
    expect(itemById(s.partner)?.start).toBe(sec(1));
    // Moved alone, the picture left its group (spec INV-35): the sound no longer follows it.
    expect(itemById(s.clip)?.linkedGroup).toBeUndefined();
    expect(itemById(s.partner)?.linkedGroup).toBeUndefined();
  });

  live("a locked track refuses a drag with a toast and changes nothing", async () => {
    const s = await seed();
    host.toggleTrack(s.video, "lock");
    await host.settled();
    await until(() => model().tracks.find((t) => t.id === s.video)?.locked === true);
    const before = await historyLength();
    const mid = xOf(sec(3));
    await drag(mid, mid + 100, laneOf(s.video));
    expect(toasts.getState().notices).toMatchObject([
      { tone: "warning", title: "Clip not moved", message: expect.stringContaining("(locked)") },
    ]);
    expect(itemById(s.clip)?.start).toBe(sec(1));
    expect(await historyLength()).toBe(before);
    host.toggleTrack(s.video, "lock");
    await host.settled();
    await until(() => model().tracks.find((t) => t.id === s.video)?.locked === false);
  });

  live("Esc during a drag rolls the live edit back and leaves no history", async () => {
    const s = await seed();
    const before = await historyLength();
    const mid = xOf(sec(3));

    pointer("down", mid, laneOf(s.video));
    for (let i = 1; i <= 10; i++) {
      pointer("move", mid + i * 10, laneOf(s.video));
      await sleep(20);
    }
    // The engine applies gesture updates live (rev ahead of the committed revision).
    await until(() => (itemById(s.clip)?.start ?? 0) > sec(1));
    expect(settledDoc()).toBe(false);

    expect(key("Escape", "cancel")).toBe(true);
    await host.settled();
    // A rollback bumps the live revision (it is a change) back to the committed content.
    await until(() => itemById(s.clip)?.start === sec(1));
    expect(await openTransactions()).toBe(0);
    expect(itemById(s.partner)?.start).toBe(sec(1));
    expect(await historyLength()).toBe(before);
    expect(host.state.mode.name).toBe("idle");
  });

  live("a drag onto another clip of a free track lands on a new track above it", async () => {
    const s = await seed();
    await rpc.call("timeline.insert", {
      sequence: s.sequence,
      track: s.video,
      media: s.media as never,
      at: "7s",
      duration: "2s",
      link: false,
    });
    await until(() => model().items.length === 3 && settledDoc());
    const before = await historyLength();
    const mid = xOf(sec(3));

    await drag(mid, mid + 300, laneOf(s.video)); // 4 s .. 8 s would overlap 7 s .. 9 s
    await until(() => itemById(s.clip)?.start === sec(4) && settledDoc());
    expect(itemById(s.clip)?.trackId).not.toBe(s.video);
    expect(model().tracks.length).toBeGreaterThan(2);
    expect(await openTransactions()).toBe(0);
    expect(await historyLength()).toBe(before + 1);
    expect(toasts.getState().notices).toEqual([]);
  });

  live(
    "60 pointer moves in one drag: at most 60 engine updates, exactly one history entry, no long task",
    async () => {
      const s = await seed();
      const before = await historyLength();
      const sent = host.updatesSent;
      const mid = xOf(sec(3));
      longestHandlerMs = 0;

      pointer("down", mid, laneOf(s.video));
      for (let i = 1; i <= 60; i++) {
        pointer("move", mid + i * 2.37, laneOf(s.video)); // fractional pixels: frames at work
        await sleep(4);
      }
      pointer("up", mid + 60 * 2.37, laneOf(s.video));
      await host.settled();
      await until(() => settledDoc());

      const updates = host.updatesSent - sent;
      console.info(
        `60 pointer moves -> ${updates} engine updates; longest handler ${longestHandlerMs.toFixed(2)} ms`,
      );
      expect(updates).toBeGreaterThan(0);
      expect(updates).toBeLessThanOrEqual(60);
      expect(await historyLength()).toBe(before + 1);
      const frame = Math.round((F * model().fps.den) / model().fps.num);
      expect((itemById(s.clip)?.start ?? 1) % frame).toBe(0);
      expect(itemById(s.clip)?.start).toBeGreaterThan(sec(2));
      expect(longestHandlerMs).toBeLessThan(50);
    },
  );

  live(
    "a 100-step drag: update round trips (UI to engine to patch) have a p95 under 16 ms",
    { retry: 2 },
    async () => {
      const s = await seed();
      const y = laneOf(s.video);
      const mid = xOf(sec(3));
      // Never a change in the document but from a patch event: each store change has a new rev.
      const unpatched: number[] = [];
      const off = stores.doc.subscribe((now, prev) => {
        if (now.project !== prev.project && now.rev === prev.rev) unpatched.push(now.rev);
      });
      try {
        pointer("down", mid, y);
        for (let i = 1; i <= 100; i++) {
          pointer("move", mid + i * 3, y);
          await sleep(16); // a hand at the display's pace
        }
        pointer("up", mid + 300, y);
        await host.settled();
        await until(() => settledDoc());
      } finally {
        off();
      }
      const samples = host.probe.samples().slice(-100);
      const p95 = quantile(samples, 0.95);
      console.info(
        `RTT over ${samples.length} updates: p50 ${quantile(samples, 0.5).toFixed(2)} ms, p95 ${p95.toFixed(2)} ms`,
      );
      expect(samples.length).toBeGreaterThan(30);
      expect(p95).toBeLessThan(16);
      expect(unpatched).toEqual([]);
    },
  );

  // WP-2A.12: an agent (the run-file's `cli` token) holds a clip in an open proposal.
  live(
    "a clip an agent holds is marked in use, and the person's drag of it is refused naming the agent",
    async () => {
      const s = await seed();
      if (handle === undefined) throw new Error("no engine");
      const agentInfo = handle.agent("cli");
      const agentConnection = new RpcConnection({ resolve: () => agentInfo });
      const agent = new RpcClient(agentConnection);
      const locks = createLocksStore();
      const feed = new LocksFeed({ rpc, store: locks, onError: (e) => errors.push(e) });
      try {
        agentConnection.connect();
        await agentConnection.whenReady();
        feed.start();
        const { tx } = await agent.call("edit.begin_tx", { kind: "proposal", label: "Tidy up" });
        await agent.call("item.move", { item: s.clip as never, start: "6s" }, { tx });
        // The clip and its linked audio are marked, with who holds them.
        await until(() => locks.getState().byItem.get(s.clip) === "agent:cli");
        expect(locks.getState().byItem.get(s.partner)).toBe("agent:cli");
        await until(() => itemById(s.clip)?.start === sec(6));
        const before = await historyLength();

        // The person drags it: refused, nothing moves, an agent-blue toast says who.
        const mid = xOf(sec(8));
        await drag(mid, mid + 100, laneOf(s.video));
        await until(() => toasts.getState().notices.length > 0);
        expect(toasts.getState().notices).toMatchObject([
          {
            tone: "agent",
            title: "Clip not moved",
            message: "In use by cli. Try again when they finish. (item_in_gesture)",
          },
        ]);
        expect(itemById(s.clip)?.start).toBe(sec(6));
        expect(await historyLength()).toBe(before);
        expect(await openTransactions()).toBe(1); // only the agent's proposal

        // The agent withdraws it: the marks go, and the same drag now works.
        await agent.call("edit.cancel_tx", { tx });
        // (A rollback leaves `rev` ahead of `committed_rev` until the next commit.)
        await until(() => locks.getState().byItem.size === 0 && itemById(s.clip)?.start === sec(1));
        toasts.getState().clear();
        const start = xOf(sec(3));
        await drag(start, start + 100, laneOf(s.video));
        await until(() => itemById(s.clip)?.start === sec(2) && settledDoc());
        expect(toasts.getState().notices).toEqual([]);
      } finally {
        feed.stop();
        agentConnection.close();
      }
    },
  );

  live("M adds a marker at the playhead; I and O set the range around it", async () => {
    await seed();
    const p = project as NonNullable<typeof project>;
    const inOut = () => stores.doc.getState().project?.sequences[p.sequence]?.in_out ?? null;
    stores.view.getState().setPlayhead(sec(2));
    expect(host.runHostKey("timeline.add_marker")).toBe(true);
    stores.view.getState().setPlayhead(sec(1));
    expect(host.runHostKey("timeline.mark_in")).toBe(true);
    await host.settled();
    // The in point shows (its patch is in) before the person presses O.
    await until(() => inOut()?.start === sec(1));
    stores.view.getState().setPlayhead(sec(4));
    expect(host.runHostKey("timeline.mark_out")).toBe(true);
    await host.settled();
    await until(() => (model().markers ?? []).some((m) => m.t === sec(2)));
    await until(() => inOut()?.end === sec(4));
    expect(inOut()).toMatchObject({ start: sec(1), end: sec(4) });
    host.clearInOut();
    await host.settled();
    await until(() => inOut() === null);
    for (const m of model().markers ?? []) {
      await rpc.call("timeline.remove_marker", { marker: m.id as never });
    }
  });

  live("nothing failed in the background", async () => {
    expect(errors).toEqual([]);
  });
});
