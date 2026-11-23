// SPDX-License-Identifier: GPL-3.0-or-later
// The editor against a REAL headless engine (the demo path of WP-1M.02): an edit made on the
// engine shows up in the timeline panel's data, and the view state round-trips through
// `view.set` / `view.get`. Skips with a message when the engine binary cannot be built here.
import type { RpcClient } from "@cutweave/protocol";
import {
  type EngineHandle,
  resolveEngineBinary,
  startEngine,
} from "@cutweave/protocol/test-support/engine";
import { mulberry32, randomEdits, seedProject } from "@cutweave/protocol/test-support/scenario";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { createDocStore } from "../state/docStore.ts";
import { createJobStore } from "../state/jobStore.ts";
import { createPlaybackStore } from "../state/playbackStore.ts";
import { selectTimelineModel } from "../state/selectors.ts";
import { createViewStore } from "../state/viewStore.ts";
import { createEngineStore } from "./engineStore.ts";
import { type EngineRuntime, startEngineRuntime } from "./runtime.ts";

let skipReason: string | undefined;
let handle: EngineHandle | undefined;
let runtime: EngineRuntime;
let rpc: RpcClient;
const errors: unknown[] = [];
const stores = {
  doc: createDocStore(),
  view: createViewStore(),
  playback: createPlaybackStore(),
  jobs: createJobStore(),
  engine: createEngineStore(),
};

beforeAll(async () => {
  const binary = resolveEngineBinary();
  if (binary.unavailable !== undefined) {
    skipReason = binary.unavailable;
    console.warn(`editor engine integration tests skipped: ${skipReason}`);
    return;
  }
  handle = await startEngine(binary.path);
  const info = handle.info;
  runtime = startEngineRuntime(
    { getConnection: () => Promise.resolve({ ...info, port: handle?.port ?? 0 }) },
    stores,
    { view: { debounceMs: 50 }, onError: (e) => errors.push(e) },
  );
  rpc = runtime.rpc;
  await runtime.connection.whenReady();
  await until(() => stores.doc.getState().status === "ready");
}, 20 * 60_000);

afterAll(async () => {
  await runtime?.stop();
  await handle?.stop();
});

async function until(condition: () => boolean, timeoutMs = 10_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!condition()) {
    if (Date.now() > deadline) {
      throw new Error("timed out waiting for the stores to catch up with the engine");
    }
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}

const model = () => selectTimelineModel(stores.doc.getState().project);

const live = (name: string, body: () => Promise<void>) =>
  it(name, async (ctx) => {
    if (skipReason !== undefined) {
      ctx.skip();
      return;
    }
    await body();
  }, 120_000);

describe("editor stores against the real engine", () => {
  live(
    "a timeline.insert on the engine shows up as an item in the timeline panel's data",
    async () => {
      const seed = await seedProject(rpc, "Demo");
      await until(() => model().tracks.length === 2);
      expect(model().tracks.map((t) => t.kind)).toEqual(["media", "audio"]);
      expect(model().items).toEqual([]);

      const out = await rpc.call("timeline.insert", {
        sequence: seed.sequence,
        track: seed.videoTrack,
        media: seed.media,
        at: "2s",
        duration: "3s",
      });
      await until(() => model().items.length === 1);
      const [item] = model().items;
      expect(item).toMatchObject({
        id: out.clips[0],
        trackId: seed.videoTrack,
        kind: "media",
        start: out.at,
        duration: out.duration,
        label: "clip.mp4",
      });
      expect(item?.start).toBe(2 * 705_600_000);
      expect(item?.duration).toBe(3 * 705_600_000);
    },
  );

  live("the panel's data follows later edits: split, then undo", async () => {
    const before = model().items;
    expect(before).toHaveLength(1);
    const first = before[0];
    await rpc.call("clip.split", { item: first?.id as never, at: "3s" });
    await until(() => model().items.length === 2);
    expect(model().items.map((i) => i.start)).toEqual([2 * 705_600_000, 3 * 705_600_000]);
    await rpc.call("edit.undo", {});
    await until(() => model().items.length === 1);
    expect(model().items[0]?.duration).toBe(3 * 705_600_000);
  });

  live("the docStore mirror equals the engine snapshot after 200 more random events", async () => {
    const seed = {
      sequence: Object.keys(stores.doc.getState().project?.sequences ?? {})[0],
      videoTrack: model().tracks[0]?.id,
      audioTrack: model().tracks[1]?.id,
      media: Object.keys(stores.doc.getState().project?.media ?? {})[0],
    } as unknown as Parameters<typeof randomEdits>[1];
    let events = 0;
    const off = rpc.on("doc.patched", () => {
      events += 1;
    });
    await randomEdits(rpc, seed, mulberry32(7), () => events >= 200);
    off();
    expect(events).toBeGreaterThanOrEqual(200);
    const snapshot = await rpc.snapshot();
    await until(() => stores.doc.getState().rev >= snapshot.rev);
    expect(stores.doc.getState().rev).toBe(snapshot.rev);
    expect(stores.doc.getState().project).toEqual(snapshot.project);
  });

  live("the view round-trips through view.set (debounced) and view.get", async () => {
    stores.view.getState().setPlayhead(1_411_200_000);
    stores.view.getState().setZoom(240);
    stores.view.getState().switchWorkspace("long-form");
    let ui = await rpc.call("view.get", {});
    const deadline = Date.now() + 5_000;
    while (ui.playhead !== 1_411_200_000 && Date.now() < deadline) {
      await new Promise((resolve) => setTimeout(resolve, 25));
      ui = await rpc.call("view.get", {});
    }
    expect(ui.playhead).toBe(1_411_200_000);
    expect(ui.zoom).toBe(240);
    expect(ui.panel_layout).toMatchObject({ v: 1, workspaceId: "long-form" });

    // A fresh store (a new window on the same engine) restores it.
    const fresh = createViewStore();
    const info = handle?.info;
    if (info === undefined) throw new Error("no engine");
    const probe = startEngineRuntime(
      { getConnection: () => Promise.resolve({ ...info, port: 0 }) },
      { ...stores, view: fresh, doc: createDocStore(), engine: createEngineStore() },
      { view: { debounceMs: 50 }, onError: (e) => errors.push(e) },
    );
    await probe.connection.whenReady();
    await until(() => fresh.getState().zoom === 240);
    expect(fresh.getState()).toMatchObject({ playhead: 1_411_200_000, workspaceId: "long-form" });
    await probe.stop();
  });

  live("nothing failed in the background", async () => {
    expect(errors).toEqual([]);
  });
});
