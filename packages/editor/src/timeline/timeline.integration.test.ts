// SPDX-License-Identifier: GPL-3.0-or-later
// WP-1M.06 end to end against a REAL headless engine (`--dev-stub-media`, so imported files
// become clips): pointer and key input goes through a real `TimelineView` (hit-testing on the
// engine's live sequence), the interaction state machine and `TimelineController`, out as
// engine commands and gesture transactions, and back through `doc.patched` into the stores.
// Only the canvas drawing is stubbed. Skips with a message when the engine cannot be built.
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { RpcClient, RpcConnection, type SequenceId, type TrackId } from "@cutweave/protocol";
import {
  type EngineHandle,
  resolveEngineBinary,
  startEngine,
} from "@cutweave/protocol/test-support/engine";
import {
  type Theme,
  type TimelineItem,
  type TimelinePointerEvent,
  TimelineView,
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
import { TimelineController } from "./controller.ts";
import { translator } from "./errors.ts";
import { createLocksStore, LocksFeed } from "./locks.ts";

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
let controller: TimelineController;
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
const HEADER = 4 * 48; // 4 x space-12
const RULER = 28; // control-md
const LANE = 48 + 2 * 4; // track-md + 2 x space-1
/** Canvas y of the middle of track `i` (0 = top). */
const laneY = (i: number) => RULER + i * LANE + LANE / 2;
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
  view.resize(1600, 400, 1);
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
  controller = new TimelineController({
    doc: stores.doc,
    view: stores.view,
    engine: stores.engine,
    toasts,
    t: translator(i18n.t.bind(i18n)),
  });
  controller.attach(view);
}, 20 * 60_000);

afterAll(async () => {
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

/**
 * An empty timeline (one project for the file, cleared per test) with one 10 s stub file
 * inserted at 1 s for 4 s (video + linked audio).
 */
async function seed(sourceIn = 0): Promise<Seeded> {
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
  await until(() => model().items.length === 0 && settledDoc());
  const inserted = await rpc.call("timeline.insert", {
    sequence: project.sequence,
    track: project.video,
    media: project.media as never,
    at: "1s",
    duration: "4s",
    source_in: sourceIn,
  });
  await until(() => model().items.length === 2 && settledDoc());
  const [clip, partner] = inserted.clips;
  if (clip === undefined || partner === undefined) throw new Error("expected video + linked audio");
  return { ...project, clip, partner };
}

/** One pointer event through the real hit-test and the controller, like `TimelineCanvas`. */
function pointer(
  type: TimelinePointerEvent["type"],
  x: number,
  y: number,
  mods: Partial<Pick<TimelinePointerEvent, "altKey" | "shiftKey">> = {},
): void {
  const hit = view.hitTest(x, y);
  timed(() =>
    controller.handlePointer({
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

function key(k: string): boolean {
  return timed(() => controller.handleKey({ key: k }));
}

/** Press at (x0, y), move in `steps` pointer moves `ms` apart to (x1, y), release. */
async function drag(x0: number, x1: number, y: number, steps = 10, ms = 8): Promise<void> {
  pointer("down", x0, y);
  for (let i = 1; i <= steps; i++) {
    pointer("move", x0 + ((x1 - x0) * i) / steps, y);
    await sleep(ms);
  }
  pointer("up", x1, y);
  await controller.settled();
}

const live = (name: string, body: () => Promise<void>) =>
  it(name, async (ctx) => {
    if (skipReason !== undefined) {
      ctx.skip();
      return;
    }
    await body();
  }, 120_000);

beforeEach(() => {
  if (skipReason !== undefined) return;
  toasts.getState().clear();
  stores.view.setState({ zoom: 100, scroll: { x: 0, y: 0 }, playhead: 0, selection: [] });
  stores.view.getState().setSnapping(true);
});

describe("timeline interactions against the real engine", () => {
  live("dragging a clip 100 px moves it 1 s, frame-aligned, as one undo step", async () => {
    const s = await seed();
    const before = await historyLength();
    const frame = Math.round((F * model().fps.den) / model().fps.num);
    const mid = xOf(sec(3));

    await drag(mid, mid + 100, laneY(0));
    await until(() => itemById(s.clip)?.start === sec(2) && settledDoc());

    expect(itemById(s.clip)?.start).toBe(sec(2));
    expect((itemById(s.clip)?.start ?? 1) % frame).toBe(0);
    expect(itemById(s.partner)?.start).toBe(sec(2)); // the linked audio follows
    expect(stores.view.getState().selection).toEqual([{ kind: "item", id: s.clip }]);
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

    await drag(right, right - 100, laneY(0));
    await until(() => itemById(s.clip)?.duration === sec(3) && settledDoc());

    expect(itemById(s.clip)).toMatchObject({ start: sec(1), duration: sec(3) });
    expect(itemById(s.partner)?.duration).toBe(sec(3));
    expect(await historyLength()).toBe(before + 1);

    await rpc.call("edit.undo", {});
    await until(() => itemById(s.clip)?.duration === sec(4));
  });

  live(
    "trimming the start edge moves start and source position together, down to the media's first frame",
    async () => {
      const s = await seed(sec(1)); // reads the file from 1 s
      const before = await historyLength();
      const left = xOf(sec(1)) + 2;

      await drag(left, left + 50, laneY(0));
      await until(() => itemById(s.clip)?.start === sec(1.5) && settledDoc());
      expect(itemById(s.clip)).toMatchObject({ duration: sec(3.5) });
      expect(itemById(s.clip)?.source?.sourceIn).toBe(sec(1.5));

      // Far past the media's first frame: the edge stops where the file starts (timeline 0 s).
      const edge = xOf(sec(1.5)) + 2;
      await drag(edge, -300, laneY(0));
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

    await drag(right, 5000, laneY(0));
    await until(() => itemById(s.clip)?.duration === sec(9) && settledDoc());
    expect(itemById(s.clip)).toMatchObject({ start: sec(1), duration: sec(9) });
    expect(toasts.getState().notices).toEqual([]);
  });

  live("S splits the clip under the playhead at a frame; the left half keeps its id", async () => {
    const s = await seed();
    const before = await historyLength();
    stores.view.getState().setPlayhead(sec(2.5) + 1234); // between frames

    expect(key("s")).toBe(true);
    await controller.settled();
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

    pointer("down", xOf(sec(3)), laneY(0));
    pointer("up", xOf(sec(3)), laneY(0));
    expect(stores.view.getState().selection).toEqual([{ kind: "item", id: s.clip }]);
    expect(key("Delete")).toBe(true);
    await controller.settled();
    await until(() => model().items.length === 1);

    expect(model().items[0]).toMatchObject({ id: later.clips[0], start: sec(7) });
    expect(stores.view.getState().selection).toEqual([]);
    expect(await historyLength()).toBe(before + 1);
  });

  live("Esc during a drag rolls the live edit back and leaves no history", async () => {
    const s = await seed();
    const before = await historyLength();
    const mid = xOf(sec(3));

    pointer("down", mid, laneY(0));
    for (let i = 1; i <= 10; i++) {
      pointer("move", mid + i * 10, laneY(0));
      await sleep(8);
    }
    // The engine applies gesture updates live (rev ahead of the committed revision).
    await until(() => (itemById(s.clip)?.start ?? 0) > sec(1));
    expect(settledDoc()).toBe(false);

    expect(key("Escape")).toBe(true);
    await controller.settled();
    // A rollback bumps the live revision (it is a change) back to the committed content.
    await until(() => itemById(s.clip)?.start === sec(1));
    expect(await openTransactions()).toBe(0);
    expect(itemById(s.partner)?.start).toBe(sec(1));
    expect(await historyLength()).toBe(before);
    expect(controller.state.mode.name).toBe("idle");
  });

  live(
    "a drop onto another clip is refused: the gesture reverts and a toast says why",
    async () => {
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

      await drag(mid, mid + 300, laneY(0)); // 4 s .. 8 s would overlap 7 s .. 9 s
      await until(() => itemById(s.clip)?.start === sec(1));

      expect(await openTransactions()).toBe(0);
      expect(await historyLength()).toBe(before);
      expect(toasts.getState().notices).toEqual([
        expect.objectContaining({
          tone: "warning",
          title: "Clip not moved",
          message: i18n.t("timeline.reason.overlap"),
        }),
      ]);
    },
  );

  live(
    "60 pointer moves in one drag: at most 60 engine updates, exactly one history entry, no long task",
    async () => {
      const s = await seed();
      const before = await historyLength();
      const mid = xOf(sec(3));
      longestHandlerMs = 0;

      pointer("down", mid, laneY(0));
      for (let i = 1; i <= 60; i++) {
        pointer("move", mid + i * 2.37, laneY(0)); // fractional pixels: frame snapping at work
        await sleep(4);
      }
      pointer("up", mid + 60 * 2.37, laneY(0));
      await controller.settled();
      await until(() => settledDoc());

      const updates = controller.lastGesture?.updates ?? 0;
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

        // The person drags it: refused at once, nothing moves, an agent-blue toast says who.
        const mid = xOf(sec(8));
        await drag(mid, mid + 100, laneY(0));
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
        await drag(start, start + 100, laneY(0));
        await until(() => itemById(s.clip)?.start === sec(2) && settledDoc());
        expect(toasts.getState().notices).toEqual([]);
      } finally {
        feed.stop();
        agentConnection.close();
      }
    },
  );

  live("nothing failed in the background", async () => {
    expect(errors).toEqual([]);
  });
});
