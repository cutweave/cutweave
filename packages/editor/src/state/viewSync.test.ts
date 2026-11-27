// SPDX-License-Identifier: GPL-3.0-or-later
import { type MediaId, MockTransport, RpcClient, type UiState } from "@cutweave/protocol";
import { describe, expect, it } from "vitest";
import { createViewStore, initialViewState, panelLayoutFrom, panelLayoutOf } from "./viewStore.ts";
import { diffView, VIEW_SYNC_DEBOUNCE_MS, ViewSync } from "./viewSync.ts";

class FakeTimers {
  #tasks = new Map<number, { at: number; run: () => void }>();
  #now = 0;
  #next = 1;
  setTimeout(run: () => void, ms: number): unknown {
    const id = this.#next++;
    this.#tasks.set(id, { at: this.#now + ms, run });
    return id;
  }
  clearTimeout(handle: unknown): void {
    this.#tasks.delete(handle as number);
  }
  get pending(): number {
    return this.#tasks.size;
  }
  advance(ms: number): void {
    this.#now += ms;
    for (const [id, t] of [...this.#tasks]) {
      if (t.at <= this.#now) {
        this.#tasks.delete(id);
        t.run();
      }
    }
  }
}

const engineView = (over: Partial<UiState> = {}): UiState => ({
  playhead: 0,
  zoom: null,
  scroll: { x: 0, y: 0 },
  selection: [],
  panel_layout: null,
  preview_quality: "full",
  master_gain: null,
  active_sequence: null,
  source_marks: {},
  ...over,
});

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

function setup(initial: UiState = engineView()) {
  const transport = new MockTransport()
    .handle("view.get", () => initial)
    .handle("view.set", () => initial);
  const store = createViewStore();
  const timers = new FakeTimers();
  const errors: unknown[] = [];
  const sync = new ViewSync({
    store,
    rpc: new RpcClient(transport),
    timers,
    onError: (e) => errors.push(e),
  });
  const sets = () => transport.requests.filter((r) => r.method === "view.set");
  return { transport, store, timers, sync, sets, errors };
}

describe("ViewSync", () => {
  it("restores the engine's view from view.get without sending it back", async () => {
    const { store, sync, timers, sets } = setup(
      engineView({
        playhead: 705_600_000,
        zoom: 240,
        scroll: { x: 5, y: 7 },
        selection: [{ kind: "item", id: "itm_1" as never }],
        preview_quality: "half",
      }),
    );
    await sync.restore();
    const s = store.getState();
    expect(s.playhead).toBe(705_600_000);
    expect(s.zoom).toBe(240);
    expect(s.scroll).toEqual({ x: 5, y: 7 });
    expect(s.selection).toEqual([{ kind: "item", id: "itm_1" }]);
    // The project's preview quality is not restored: it is a preference now (WP-2H.10).
    expect("previewQuality" in s).toBe(false);
    timers.advance(10_000);
    await settle();
    expect(sets()).toHaveLength(0);
  });

  it("sends a change 500 ms after the last one, as one view.set with only what changed", async () => {
    const { store, sync, timers, sets } = setup();
    await sync.restore();
    store.getState().setPlayhead(100);
    timers.advance(VIEW_SYNC_DEBOUNCE_MS - 1);
    store.getState().setPlayhead(200); // restarts the wait
    timers.advance(VIEW_SYNC_DEBOUNCE_MS - 1);
    await settle();
    expect(sets()).toHaveLength(0);
    timers.advance(1);
    await settle();
    expect(sets()).toHaveLength(1);
    expect(sets()[0]?.params).toEqual({ patch: { playhead: 200 } });
  });

  it("mirrors zoom, scroll, selection and the panel layout", async () => {
    const { store, sync, timers, sets } = setup();
    await sync.restore();
    const s = store.getState();
    s.setZoom(120);
    s.setScroll(3, 4);
    s.setSelection([{ kind: "track", id: "trk_1" as never }]);
    s.switchWorkspace("long-form");
    timers.advance(VIEW_SYNC_DEBOUNCE_MS);
    await settle();
    const patch =
      (sets()[0]?.params as { patch: Record<string, unknown> } | undefined)?.patch ?? {};
    expect(patch).toMatchObject({
      zoom: 120,
      scroll: { x: 3, y: 4 },
      selection: [{ kind: "track", id: "trk_1" }],
    });
    expect("preview_quality" in patch).toBe(false);
    expect(panelLayoutFrom(patch.panel_layout)).toMatchObject({
      workspaceId: "long-form",
      aspect: "16:9",
    });
    // Nothing else changed since: no second call.
    timers.advance(5_000);
    await settle();
    expect(sets()).toHaveLength(1);
  });

  it("does not send view changes made by tool changes", async () => {
    const { store, sync, timers, sets } = setup();
    await sync.restore();
    store.getState().setToolMode("razor");
    expect(timers.pending).toBe(0);
    await settle();
    expect(sets()).toHaveLength(0);
  });

  it("adopts view.changed from another client without echoing it", async () => {
    const { transport, store, sync, timers, sets } = setup();
    await sync.restore();
    transport.emit("view.changed", engineView({ playhead: 42, selection: [] }));
    expect(store.getState().playhead).toBe(42);
    timers.advance(5_000);
    await settle();
    expect(sets()).toHaveLength(0);
  });

  it("keeps the difference when view.set fails and retries it with the next change", async () => {
    const { transport, store, sync, timers, sets, errors } = setup();
    await sync.restore();
    let fail = true;
    transport.handle("view.set", () => {
      if (fail) throw new Error("engine busy");
      return engineView();
    });
    store.getState().setPlayhead(10);
    timers.advance(VIEW_SYNC_DEBOUNCE_MS);
    await settle();
    expect(errors).toHaveLength(1);
    fail = false;
    store.getState().setZoom(50);
    timers.advance(VIEW_SYNC_DEBOUNCE_MS);
    await settle();
    expect(sets()).toHaveLength(2);
    expect(sets()[1]?.params).toEqual({ patch: { playhead: 10, zoom: 50 } });
  });

  it("stop() flushes a pending change and stops watching", async () => {
    const { store, sync, timers, sets } = setup();
    await sync.restore();
    store.getState().setPlayhead(7);
    await sync.stop();
    expect(sets()).toHaveLength(1);
    store.getState().setPlayhead(8);
    expect(timers.pending).toBe(0);
  });

  it("restoring an engine panel layout rebuilds the dock", async () => {
    const layout = panelLayoutOf({ workspaceId: "long-form", aspect: "1:1", layouts: {} });
    const { store, sync } = setup(engineView({ panel_layout: layout }));
    const before = store.getState().layoutRevision;
    await sync.restore();
    expect(store.getState().workspaceId).toBe("long-form");
    expect(store.getState().aspect).toBe("1:1");
    expect(store.getState().layoutRevision).toBe(before + 1);
  });
});

describe("diffView and panel layout", () => {
  it("sends everything the first time and nothing when unchanged", () => {
    const s = createViewStore().getState();
    const now = {
      playhead: s.playhead,
      zoom: s.zoom,
      scroll: s.scroll,
      selection: s.selection,
      sourceMarks: s.sourceMarks,
      panel: { workspaceId: s.workspaceId, aspect: s.aspect, layouts: s.layouts },
    };
    expect(Object.keys(diffView(null, now) ?? {}).sort()).toEqual([
      "panel_layout",
      "playhead",
      "scroll",
      "selection",
    ]);
    expect(diffView(now, { ...now })).toBeNull();
  });

  it("source marks go to the engine at once, per asset, and a cleared asset as {}", async () => {
    const a = "m_a" as MediaId;
    const b = "m_b" as MediaId;
    const { store, sync, sets, timers } = setup(
      engineView({ source_marks: { [b]: { in: 10, out: null } } as UiState["source_marks"] }),
    );
    await sync.restore();
    expect(store.getState().sourceMarks).toEqual({ [b]: { in: 10, out: null } });
    store.getState().setSourceMarks(a, { in: 100, out: 400 });
    await settle();
    expect(timers.pending).toBe(0);
    expect(sets().at(-1)?.params).toEqual({
      patch: { source_marks: { [a]: { in: 100, out: 400 } } },
    });
    store.getState().setSourceMarks(b, null);
    store.getState().setSourceMarks(a, { in: null, out: 400 });
    await settle();
    // The second change came while the first was on its way: it follows after the debounce.
    timers.advance(VIEW_SYNC_DEBOUNCE_MS);
    await settle();
    const sent = sets()
      .slice(1)
      .map((r) => (r.params as { patch: { source_marks: unknown } }).patch.source_marks);
    expect(Object.assign({}, ...sent)).toEqual({ [b]: {}, [a]: { out: 400 } });
    expect(store.getState().sourceMarks).toEqual({ [a]: { in: null, out: 400 } });
  });

  it("another project opened: the old marks are dropped, never sent, and its own are read", async () => {
    const a = "m_a" as MediaId;
    const b = "m_b" as MediaId;
    let held: UiState = engineView({
      source_marks: { [a]: { in: 1, out: 9 } } as UiState["source_marks"],
    });
    const transport = new MockTransport()
      .handle("view.get", () => held)
      .handle("view.set", () => held);
    const store = createViewStore();
    const timers = new FakeTimers();
    const sync = new ViewSync({ store, rpc: new RpcClient(transport), timers });
    await sync.restore();
    expect(store.getState().sourceMarks).toEqual({ [a]: { in: 1, out: 9 } });
    held = engineView({ source_marks: { [b]: { in: 2, out: null } } as UiState["source_marks"] });
    transport.emit("project.opened", { name: "Other" });
    // At once: nothing of the project before is left to send.
    expect(store.getState().sourceMarks).toEqual({});
    await settle();
    expect(store.getState().sourceMarks).toEqual({ [b]: { in: 2, out: null } });
    timers.advance(5_000);
    await settle();
    expect(transport.requests.filter((r) => r.method === "view.set")).toHaveLength(0);
    transport.emit("project.closed", {});
    expect(store.getState().sourceMarks).toEqual({});
  });

  it("panelLayoutFrom narrows the opaque engine value", () => {
    expect(panelLayoutFrom(null)).toBeNull();
    expect(panelLayoutFrom({})).toBeNull();
    expect(panelLayoutFrom({ v: 99, workspaceId: "long-form", aspect: "1:1" })).toBeNull();
    expect(panelLayoutFrom({ v: 1, workspaceId: "nope", aspect: "1:1" })).toBeNull();
    expect(panelLayoutFrom({ v: 1, workspaceId: "long-form", aspect: "1:1", layouts: 3 })).toEqual({
      workspaceId: "long-form",
      aspect: "1:1",
      layouts: {},
    });
  });

  it("the new engine-mirrored fields start at their defaults", () => {
    const s = createViewStore().getState();
    expect(s.playhead).toBe(initialViewState.playhead);
    expect(s.toolMode).toBe("select");
    expect(s.selection).toEqual([]);
  });
});
