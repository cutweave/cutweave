// SPDX-License-Identifier: GPL-3.0-or-later
// WP-2F.03: the editor's picture of missing media. The engine is asked when a project opens, when
// the window is back and when a media path changes; a project that opens with missing media
// shows the relink dialog once; a relink is one catalog command and its summary.
import type { MediaId, MissingMedia, Project, RpcClient } from "@cutweave/protocol";
import { RpcError } from "@cutweave/protocol";
import { describe, expect, it, vi } from "vitest";
import { createStore } from "zustand/vanilla";
import type { EngineStoreState } from "../engine/engineStore.ts";
import type { ToastStore } from "../state/toastStore.ts";
import {
  createHealthStore,
  refreshHealth,
  relinkFromFolder,
  relinkOne,
  sortMissing,
  startHealthWiring,
} from "./health.ts";

const med = (n: number) => `med_${n}` as MediaId;
const gone = (n: number, name: string): MissingMedia => ({
  media_id: med(n),
  name,
  path: `/old/${name}`,
  status: "missing",
});

/** An engine whose disk has lost `missing`; relinking a folder finds everything. */
function setup(missing: MissingMedia[]) {
  const disk = { missing };
  const call = vi.fn(async (name: string, _args: unknown) => {
    if (name === "media.find_missing") return { missing: disk.missing, checked: 3 };
    if (name === "media.relink_folder") {
      const relinked = disk.missing.map((m) => ({ media_id: m.media_id }));
      disk.missing = [];
      return { relinked, still_missing: [], scanned: 4, truncated: false };
    }
    if (name === "media.relink") {
      disk.missing = disk.missing.slice(1);
      return {};
    }
    throw new Error(`unexpected ${name}`);
  });
  const engine = createStore<EngineStoreState>()(
    () => ({ rpc: { call } as unknown as RpcClient, connection: "ready" }) as EngineStoreState,
  );
  const pushed: { title: string; message?: string }[] = [];
  const toasts = {
    getState: () => ({ push: (toast: { title: string }) => pushed.push(toast) }),
  } as unknown as import("zustand").StoreApi<ToastStore>;
  const health = createHealthStore();
  const deps = { engine, toasts, health, t: ((key: string) => key) as never };
  return { disk, call, engine, health, deps, pushed };
}

const settle = async () => {
  for (let i = 0; i < 5; i += 1) await Promise.resolve();
};

describe("media health", () => {
  it("stores what the engine says is missing, sorted by name for the dialog", async () => {
    const s = setup([gone(2, "b.mp4"), gone(1, "a.mp4")]);
    expect(await refreshHealth(s.deps)).toBe(2);
    expect(s.health.getState().missing.has(med(1))).toBe(true);
    expect(sortMissing(s.health.getState().missing).map((m) => m.name)).toEqual(["a.mp4", "b.mp4"]);
  });

  it("keeps the last answer when there is no engine or the call fails", async () => {
    const s = setup([gone(1, "a.mp4")]);
    await refreshHealth(s.deps);
    s.call.mockRejectedValueOnce(new Error("down"));
    expect(await refreshHealth(s.deps)).toBeNull();
    s.engine.setState({ rpc: null });
    expect(await refreshHealth(s.deps)).toBeNull();
    expect(s.health.getState().missing.size).toBe(1);
  });

  it("relinks from a folder with one command and says what it found", async () => {
    const s = setup([gone(1, "a.mp4"), gone(2, "b.mp4")]);
    await refreshHealth(s.deps);
    const summary = await relinkFromFolder(s.deps, "/moved");
    expect(s.call).toHaveBeenCalledWith("media.relink_folder", {
      folder: "/moved",
      recursive: null,
    });
    expect(summary).toEqual({ relinked: 2, stillMissing: 0, truncated: false });
    expect(s.health.getState().last).toEqual(summary);
    expect(s.health.getState().missing.size).toBe(0);
    expect(s.health.getState().busy).toBe(false);
  });

  it("relinks one asset to a chosen file", async () => {
    const s = setup([gone(1, "a.mp4"), gone(2, "b.mp4")]);
    await refreshHealth(s.deps);
    expect(await relinkOne(s.deps, med(1), "/new/a.mp4")).toBe(true);
    expect(s.call).toHaveBeenCalledWith("media.relink", { media: med(1), path: "/new/a.mp4" });
    expect(s.health.getState().last?.stillMissing).toBe(1);
  });

  it("shows the engine's message when a relink is refused", async () => {
    const s = setup([gone(1, "a.mp4")]);
    await refreshHealth(s.deps);
    s.call.mockRejectedValueOnce(new RpcError("invalid", "this file is shorter than the original"));
    expect(await relinkOne(s.deps, med(1), "/new/short.mp4")).toBe(false);
    expect(s.pushed).toEqual([
      expect.objectContaining({
        title: "relink.failed.file",
        message: "this file is shorter than the original",
      }),
    ]);
    expect(s.health.getState().busy).toBe(false);
  });
});

describe("health wiring", () => {
  const project = (path: string): Project =>
    ({ media: { [med(1)]: { id: med(1), path_abs: path } } }) as unknown as Project;

  function wired(missing: MissingMedia[]) {
    const s = setup(missing);
    const doc = createStore<{ project: Project | null }>()(() => ({ project: project("/old/a") }));
    const session = createStore<{ path: string | null; opening: unknown }>()(() => ({
      path: "/p/Film.cutweave",
      opening: null,
    }));
    const listeners = new Map<string, () => void>();
    const window = {
      addEventListener: (name: string, fn: () => void) => listeners.set(name, fn),
      removeEventListener: (name: string) => listeners.delete(name),
    } as unknown as Window;
    const stop = startHealthWiring({ engine: s.engine, doc, session, health: s.health, window });
    return { ...s, doc, session, listeners, stop };
  }

  it("opens the dialog once when the project has missing media", async () => {
    const w = wired([gone(1, "a.mp4")]);
    await settle();
    expect(w.health.getState().dialogOpen).toBe(true);
    // Closing it is respected: a later check does not bring it back.
    w.health.getState().closeDialog();
    w.listeners.get("focus")?.();
    await settle();
    expect(w.health.getState().dialogOpen).toBe(false);
    expect(w.health.getState().missing.size).toBe(1);
    w.stop();
  });

  it("stays quiet for a healthy project and asks again when a path changes", async () => {
    const w = wired([]);
    await settle();
    expect(w.health.getState().dialogOpen).toBe(false);
    const calls = w.call.mock.calls.length;
    // An undo of a relink puts an old path back: the file there is gone.
    w.disk.missing = [gone(1, "a.mp4")];
    w.doc.setState({ project: project("/older/a") });
    await settle();
    expect(w.call.mock.calls.length).toBe(calls + 1);
    expect(w.health.getState().missing.size).toBe(1);
    expect(w.health.getState().dialogOpen).toBe(false);
    w.stop();
  });

  it("announces the next project that opens", async () => {
    const w = wired([]);
    await settle();
    w.disk.missing = [gone(1, "a.mp4"), gone(2, "b.mp4")];
    w.session.setState({ opening: { path: "/p/Other" } });
    w.session.setState({ path: "/p/Other/Other.cutweave", opening: null });
    await settle();
    expect(w.health.getState().dialogOpen).toBe(true);
    expect(w.health.getState().missing.size).toBe(2);
    w.stop();
    expect(w.listeners.size).toBe(0);
  });
});
