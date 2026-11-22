// SPDX-License-Identifier: GPL-3.0-or-later
import type { SerializedDockview } from "dockview-react";
import { describe, expect, it } from "vitest";
import type { StateStorage } from "zustand/middleware";
import {
  ASPECTS,
  createViewStore,
  VIEW_STORAGE_KEY,
  WORKSPACE_DEFS,
  WORKSPACES,
} from "./viewStore.ts";

const layout = (tag: string) => ({ tag }) as unknown as SerializedDockview;

function memory(
  initial: Record<string, string> = {},
): StateStorage & { data: Map<string, string> } {
  const data = new Map(Object.entries(initial));
  return {
    data,
    getItem: (k) => data.get(k) ?? null,
    setItem: (k, v) => void data.set(k, v),
    removeItem: (k) => void data.delete(k),
  };
}

describe("viewStore", () => {
  it("starts dark, in the Short-form workspace, on the Media tab", () => {
    const s = createViewStore().getState();
    expect(s.theme).toBe("dark");
    expect(s.workspaceId).toBe("short-form");
    expect(s.assetTab).toBe("media");
    expect(s.aspect).toBe("9:16");
    expect(s.layouts).toEqual({});
  });

  it("sets and toggles the theme", () => {
    const store = createViewStore();
    store.getState().toggleTheme();
    expect(store.getState().theme).toBe("light");
    store.getState().toggleTheme();
    expect(store.getState().theme).toBe("dark");
    store.getState().setTheme("light");
    expect(store.getState().theme).toBe("light");
  });

  it("defines exactly the two saved workspaces with distinct proportions", () => {
    expect([...WORKSPACES]).toEqual(["short-form", "long-form"]);
    expect(WORKSPACE_DEFS["short-form"].defaultAspect).toBe("9:16");
    expect(WORKSPACE_DEFS["long-form"].defaultAspect).toBe("16:9");
    expect(WORKSPACE_DEFS["long-form"].timelineHeight).toBeGreaterThan(
      WORKSPACE_DEFS["short-form"].timelineHeight,
    );
  });

  it("switching workspace changes it and adopts that workspace's preview aspect", () => {
    const store = createViewStore();
    store.getState().switchWorkspace("long-form");
    expect(store.getState().workspaceId).toBe("long-form");
    expect(store.getState().aspect).toBe("16:9");
    store.getState().setAspect("1:1");
    store.getState().switchWorkspace("long-form"); // same workspace: no change
    expect(store.getState().aspect).toBe("1:1");
    store.getState().switchWorkspace("short-form");
    expect(store.getState().aspect).toBe("9:16");
  });

  it("keeps a separate saved layout per workspace", () => {
    const store = createViewStore();
    store.getState().saveLayout("short-form", layout("a"));
    store.getState().saveLayout("long-form", layout("b"));
    store.getState().switchWorkspace("long-form");
    expect(store.getState().layouts["short-form"]).toEqual(layout("a"));
    expect(store.getState().layouts["long-form"]).toEqual(layout("b"));
  });

  it("reset drops only that workspace's layout and bumps the revision", () => {
    const store = createViewStore();
    store.getState().saveLayout("short-form", layout("a"));
    store.getState().saveLayout("long-form", layout("b"));
    const before = store.getState().layoutRevision;
    store.getState().resetWorkspace("short-form");
    expect(store.getState().layouts["short-form"]).toBeUndefined();
    expect(store.getState().layouts["long-form"]).toEqual(layout("b"));
    expect(store.getState().layoutRevision).toBe(before + 1);
  });

  it("offers every preview aspect the switcher shows", () => {
    expect(ASPECTS.map((a) => a.id)).toEqual(["9:16", "1:1", "4:5", "16:9"]);
    const store = createViewStore();
    store.getState().setAspect("4:5");
    expect(store.getState().aspect).toBe("4:5");
  });

  it("persists theme, workspace and layouts, and restores them", () => {
    const storage = memory();
    const first = createViewStore(storage);
    first.getState().setTheme("light");
    first.getState().switchWorkspace("long-form");
    first.getState().saveLayout("long-form", layout("kept"));
    expect(storage.data.has(VIEW_STORAGE_KEY)).toBe(true);

    const second = createViewStore(storage);
    expect(second.getState().theme).toBe("light");
    expect(second.getState().workspaceId).toBe("long-form");
    expect(second.getState().layouts["long-form"]).toEqual(layout("kept"));
    // The reset counter is session state, not persisted.
    expect(second.getState().layoutRevision).toBe(0);
  });

  it("ignores malformed persisted values instead of breaking startup", () => {
    const blob = JSON.stringify({
      state: { theme: "neon", workspaceId: "nope", assetTab: 7, aspect: "3:2", layouts: "x" },
      version: 1,
    });
    const s = createViewStore(memory({ [VIEW_STORAGE_KEY]: blob })).getState();
    expect(s.theme).toBe("dark");
    expect(s.workspaceId).toBe("short-form");
    expect(s.assetTab).toBe("media");
    expect(s.aspect).toBe("9:16");
    expect(s.layouts).toEqual({});
  });
});
