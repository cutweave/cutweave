// SPDX-License-Identifier: GPL-3.0-or-later
// WP-1M.03 end to end against a REAL headless engine (`--dev-stub-media`: files with video, audio
// or image extensions probe as media, anything else is refused). Import, the bin's cards, removal
// and the drag-to-timeline flow run through the editor's own code (`importMedia`, `removeMedia`,
// `TimelineController` over a real `TimelineView` hit-test, `insertAtPlayhead`) and the engine's
// `doc.patched` events back into the stores. Only canvas drawing is stubbed. This is the "e2e"
// of the WP in Vitest-node form (as WP-1M.06 did): Playwright-Electron needs a built bridge
// addon, a display and Electron's native file dialog, none of which the WP's logic depends on.
// Skips with a message when the engine cannot be built.
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { RpcClient, SequenceId, TrackId } from "@cutweave/protocol";
import {
  type EngineHandle,
  resolveEngineBinary,
  startEngine,
} from "@cutweave/protocol/test-support/engine";
import { type Theme, TimelineView } from "@cutweave/timeline";
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
import { TimelineController } from "../timeline/controller.ts";
import { translator } from "../timeline/errors.ts";
import { type MediaCard, selectMediaCards } from "./cards.ts";
import { importMedia } from "./import.ts";
import { insertAtPlayhead } from "./insert.ts";
import { createMediaStore } from "./mediaStore.ts";
import { removeMedia } from "./remove.ts";

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
  media: createMediaStore(),
  engine: createEngineStore(),
};
const toasts = createToastStore();
const t = translator(i18n.t.bind(i18n));
const deps = { engine: stores.engine, toasts, t };
let controller: TimelineController;
let view: TimelineView;

// ---- a headless TimelineView (as in the WP-1M.06 suite) -------------------------------------

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

const noopContext = new Proxy({}, { get: () => () => undefined }) as CanvasRenderingContext2D;
const HEADER = 4 * 48; // 4 x space-12
const RULER = 28; // control-md
const LANE = 48 + 2 * 4; // track-md + 2 x space-1
/** Canvas y of the middle of track `i` (0 = top). */
const laneY = (i: number) => RULER + i * LANE + LANE / 2;
/** Canvas x of timeline time `t` at 100 px/s, no scroll. */
const xOf = (time: number) => HEADER + (time / F) * 100;

// ---- engine and stores ----------------------------------------------------------------------

beforeAll(async () => {
  const binary = resolveEngineBinary();
  if (binary.unavailable !== undefined) {
    skipReason = binary.unavailable;
    console.warn(`media bin e2e tests skipped: ${skipReason}`);
    return;
  }
  mediaDir = mkdtempSync(join(tmpdir(), "cutweave-media-e2e-"));
  // Generated here, never committed: the stub backend probes by extension, and equal content is
  // one asset (duplicate), so every file gets bytes of its own.
  for (const [i, name] of ["a.mp4", "b.mov", "c.wav", "d.png", "e.mp4"].entries()) {
    writeFileSync(join(mediaDir, name), Buffer.alloc(4096, i + 1));
  }
  writeFileSync(join(mediaDir, "notes.txt"), "not media");
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
  stores.doc.subscribe((s) => view.setModel(selectTimelineModel(s.project)));
  stores.view.subscribe((s) =>
    view.setViewState({
      pxPerSec: s.zoom ?? 100,
      scrollX: s.scroll.x,
      scrollY: s.scroll.y,
      playhead: s.playhead,
      selection: new Set(s.selection.map((r) => r.id)),
    }),
  );
  controller = new TimelineController({
    doc: stores.doc,
    view: stores.view,
    engine: stores.engine,
    toasts,
    t,
  });
  controller.attach(view);
}, 20 * 60_000);

afterAll(async () => {
  await runtime?.stop();
  await handle?.stop();
  if (mediaDir !== undefined) rmSync(mediaDir, { recursive: true, force: true });
});

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

async function until(condition: () => boolean, timeoutMs = 10_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!condition()) {
    if (Date.now() > deadline) throw new Error("timed out waiting for the engine and the stores");
    await sleep(5);
  }
}

const cards = (): readonly MediaCard[] => selectMediaCards(stores.doc.getState().project);
const model = () => selectTimelineModel(stores.doc.getState().project);
const settledDoc = () => stores.doc.getState().rev === stores.doc.getState().committedRev;
const historyLength = async () => (await rpc.call("edit.history", { limit: 500 })).entries.length;
const path = (name: string) => join(mediaDir, name);
const byName = (name: string): MediaCard => {
  const card = cards().find((c) => c.name === name);
  if (card === undefined) throw new Error(`no card for ${name}`);
  return card;
};

let projectReady = false;
let sequence: SequenceId;
let video: TrackId;
let audio: TrackId;

const live = (name: string, body: () => Promise<void>) =>
  it(name, async (ctx) => {
    if (skipReason !== undefined) {
      ctx.skip();
      return;
    }
    await body();
  }, 120_000);

beforeEach(async () => {
  if (skipReason !== undefined) return;
  toasts.getState().clear();
  stores.view.setState({ zoom: 100, scroll: { x: 0, y: 0 }, playhead: 0, selection: [] });
  stores.view.getState().setSnapping(true);
  // One project for the file (`project.new` only initializes an empty project); each test starts
  // from an empty timeline and an empty bin. Tracks a drop created stay, which is harmless.
  if (!projectReady) {
    const created = await rpc.call("project.new", { name: "Media bin" });
    projectReady = true;
    sequence = created.sequence;
    video = created.video_track;
    audio = created.audio_track;
    await until(() => stores.doc.getState().status === "ready");
  }
  const leftovers = model().items.map((i) => i.id);
  if (leftovers.length > 0) await rpc.call("item.delete", { items: leftovers as never });
  for (const card of cards()) await rpc.call("media.remove", { id: card.id });
  await until(() => cards().length === 0 && model().items.length === 0 && settledDoc());
});

describe("importing into the bin", () => {
  live("three files at once give three cards within 2 s, with probe info", async () => {
    const before = await historyLength();
    const t0 = Date.now();
    const summary = await importMedia(deps, [path("a.mp4"), path("c.wav"), path("d.png")]);
    await until(() => cards().length === 3, 2_000);
    expect(Date.now() - t0).toBeLessThan(2_000);
    expect(summary).toMatchObject({ added: 3, duplicates: 0, failed: [] });
    expect(cards().map((c) => c.name)).toEqual(expect.arrayContaining(["a.mp4", "c.wav", "d.png"]));
    expect(byName("a.mp4")).toMatchObject({ kind: "video", broken: false });
    expect(byName("a.mp4").duration).toBeGreaterThan(0);
    expect(byName("c.wav").kind).toBe("audio");
    expect(byName("d.png").kind).toBe("image");
    expect(toasts.getState().notices.map((n) => n.tone)).toEqual(["success"]);
    // One undo step for the whole import (arch §7).
    expect(await historyLength()).toBe(before + 1);
  });

  live(
    "an unsupported file is reported in failed[] as an error toast and gets no card",
    async () => {
      const summary = await importMedia(deps, [
        path("a.mp4"),
        path("notes.txt"),
        path("missing.mp4"),
      ]);
      await until(() => cards().length === 1);
      expect(summary?.failed.map((f) => f.code).sort()).toEqual(["not_found", "unsupported"]);
      const danger = toasts.getState().notices.filter((n) => n.tone === "danger");
      expect(danger.map((n) => n.title).sort()).toEqual([
        "missing.mp4 not imported",
        "notes.txt not imported",
      ]);
      expect(danger.every((n) => (n.message ?? "").length > 0)).toBe(true);
      expect(cards().map((c) => c.name)).toEqual(["a.mp4"]);
    },
  );

  live("importing the same file again says it is a duplicate and adds no card", async () => {
    await importMedia(deps, [path("a.mp4")]);
    await until(() => cards().length === 1);
    toasts.getState().clear();
    const again = await importMedia(deps, [path("a.mp4")]);
    expect(again).toMatchObject({ added: 0, duplicates: 1 });
    expect(cards()).toHaveLength(1);
    expect(toasts.getState().notices[0]?.message).toBe("1 was already in the project.");
  });

  live("a path the engine refuses outright becomes one error toast", async () => {
    const summary = await importMedia(deps, ["relative/not-absolute.mp4"]);
    expect(summary).toBeNull();
    expect(toasts.getState().notices.map((n) => n.title)).toEqual(["Import failed"]);
  });
});

describe("removing from the bin", () => {
  live("an unused asset goes away", async () => {
    await importMedia(deps, [path("a.mp4"), path("c.wav")]);
    await until(() => cards().length === 2);
    expect(await removeMedia(deps, byName("c.wav").id, "c.wav")).toBe(true);
    await until(() => cards().length === 1);
    expect(toasts.getState().notices.filter((n) => n.tone === "danger")).toEqual([]);
  });

  live("a referenced asset stays, and the engine's message is shown", async () => {
    await importMedia(deps, [path("a.mp4")]);
    await until(() => cards().length === 1);
    const media = byName("a.mp4").id;
    await rpc.call("timeline.insert", { sequence, track: video, media, at: 0 });
    await until(() => model().items.length > 0 && settledDoc());
    toasts.getState().clear();
    expect(await removeMedia(deps, media, "a.mp4")).toBe(false);
    const [toast] = toasts.getState().notices;
    expect(toast?.title).toBe("a.mp4 not removed");
    expect(toast?.message).toMatch(/clip|use/i);
    expect(cards()).toHaveLength(1);
  });
});

describe("dragging a card onto the timeline", () => {
  const seed = async (...names: string[]): Promise<MediaCard[]> => {
    await importMedia(deps, names.map(path));
    await until(() => cards().length === names.length);
    toasts.getState().clear();
    return names.map(byName);
  };
  const drag = (card: MediaCard) => ({
    id: card.id,
    kind: card.kind as "video" | "audio" | "image",
    duration: card.kind === "image" ? null : card.duration,
  });
  /** Canvas y of the lane showing `track` (leftover tracks of earlier tests shift the lanes). */
  const laneOf = (track: TrackId) => laneY(model().tracks.findIndex((tr) => tr.id === track));
  const frame = () => {
    const fps = model().fps;
    return (F * fps.den) / fps.num;
  };

  live(
    "drops a clip at the snapped, frame-aligned time on the track under the pointer",
    async () => {
      const [clip] = await seed("a.mp4");
      const media = drag(clip as MediaCard);
      const x = xOf(sec(2)) + 1.3; // a little past 2 s
      expect(controller.mediaDragOver(x, laneOf(video), media)).toBe(true);
      expect(controller.state.mode.name).toBe("dropping-media");
      expect(controller.overlay.getState().snapGuides).toHaveLength(1);
      const before = await historyLength();
      controller.mediaDrop(x, laneOf(video), media);
      await controller.settled();
      await until(() => model().items.length > 0 && settledDoc());
      expect(controller.state.mode.name).toBe("idle");
      expect(controller.overlay.getState().snapGuides).toHaveLength(0);
      const placed = model().items.find((i) => i.trackId === video);
      expect(placed).toBeDefined();
      expect((placed?.start ?? 1) % frame()).toBeCloseTo(0, 5);
      expect(Math.abs((placed?.start ?? 0) - sec(2))).toBeLessThanOrEqual(frame());
      // The video's own audio goes in as a linked clip: one undo step in all.
      expect(model().items.some((i) => i.trackId === audio)).toBe(true);
      expect(await historyLength()).toBe(before + 1);
      expect(stores.view.getState().selection[0]).toMatchObject({ kind: "item" });
    },
  );

  live("a drop onto an occupied spot opens a new track instead of failing", async () => {
    const [first, second] = await seed("a.mp4", "e.mp4");
    for (const card of [first, second] as MediaCard[]) {
      const media = drag(card);
      controller.mediaDragOver(xOf(sec(1)), laneOf(video), media);
      controller.mediaDrop(xOf(sec(1)), laneOf(video), media);
      await controller.settled();
      await until(() => settledDoc());
    }
    await until(() => model().items.filter((i) => i.kind === "media").length === 2);
    expect(toasts.getState().notices.filter((n) => n.tone === "warning")).toEqual([]);
    const starts = model()
      .items.filter((i) => i.kind === "media")
      .map((i) => i.start);
    expect(starts[0]).toBe(starts[1]);
    expect(
      new Set(
        model()
          .items.filter((i) => i.kind === "media")
          .map((i) => i.trackId),
      ).size,
    ).toBe(2);
  });

  live("sound dropped on a video lane lands on the audio track", async () => {
    const [sound] = await seed("c.wav");
    const media = drag(sound as MediaCard);
    controller.mediaDragOver(xOf(sec(1)), laneOf(video), media);
    controller.mediaDrop(xOf(sec(1)), laneOf(video), media);
    await controller.settled();
    await until(() => model().items.length === 1 && settledDoc());
    const landed = model().tracks.find((tr) => tr.id === model().items[0]?.trackId);
    expect(landed?.kind).toBe("audio");
  });

  live("a drop snaps to a neighbouring clip's edge within 8 px", async () => {
    const [clipA, clipB] = await seed("a.mp4", "e.mp4");
    const a = drag(clipA as MediaCard);
    controller.mediaDrop(xOf(sec(1)), laneOf(video), a);
    await controller.settled();
    await until(() => model().items.length === 2 && settledDoc());
    const end =
      (model().items.find((i) => i.trackId === video)?.start ?? 0) +
      (model().items.find((i) => i.trackId === video)?.duration ?? 0);
    const b = drag(clipB as MediaCard);
    controller.mediaDrop(xOf(end) + 5, laneOf(video), b); // 5 px past the first clip's end
    await controller.settled();
    await until(() => model().items.filter((i) => i.kind === "media").length === 2 && settledDoc());
    const starts = model()
      .items.filter((i) => i.kind === "media")
      .map((i) => i.start);
    expect(starts).toContain(end);
    expect(toasts.getState().notices).toEqual([]);
  });

  live("leaving the canvas or pressing Esc cancels without an edit", async () => {
    const [clip] = await seed("a.mp4");
    const media = drag(clip as MediaCard);
    const before = await historyLength();
    controller.mediaDragOver(xOf(sec(1)), laneOf(video), media);
    controller.mediaDragLeave();
    expect(controller.state.mode.name).toBe("idle");
    controller.mediaDragOver(xOf(sec(1)), laneOf(video), media);
    controller.handleKey({ key: "Escape" });
    expect(controller.state.mode.name).toBe("idle");
    expect(controller.overlay.getState().snapGuides).toHaveLength(0);
    await controller.settled();
    expect(model().items).toHaveLength(0);
    expect(await historyLength()).toBe(before);
  });

  live("Enter inserts at the playhead on the selected track (frame-aligned)", async () => {
    const [clip] = await seed("a.mp4");
    stores.view.getState().setPlayhead(sec(3) + 12_345);
    stores.view.getState().setSelection([{ kind: "track", id: video }]);
    const ok = await insertAtPlayhead(
      { doc: stores.doc, view: stores.view, engine: stores.engine, toasts, t },
      drag(clip as MediaCard),
    );
    expect(ok).toBe(true);
    await until(() => model().items.length > 0 && settledDoc());
    const placed = model().items.find((i) => i.trackId === video);
    expect((placed?.start ?? 1) % frame()).toBeCloseTo(0, 5);
    expect(Math.abs((placed?.start ?? 0) - sec(3))).toBeLessThanOrEqual(frame());
  });
});
