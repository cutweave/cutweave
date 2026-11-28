// SPDX-License-Identifier: GPL-3.0-or-later
import {
  type MediaId,
  MockTransport,
  RpcClient,
  type SequenceId,
  type UiState,
} from "@cutweave/protocol";
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
      activeSequence: s.activeSequence,
      panel: {
        workspaceId: s.workspaceId,
        aspect: s.aspect,
        layouts: s.layouts,
        sequenceTabs: s.sequenceTabs,
      },
    };
    expect(Object.keys(diffView(null, now) ?? {}).sort()).toEqual([
      "panel_layout",
      "playhead",
      "scroll",
      "selection",
    ]);
    expect(diffView(now, { ...now })).toBeNull();
  });

  it("the sequence shown is restored, and a new choice goes to the engine at once (WP-2B.03)", async () => {
    const a = "seq_a" as SequenceId;
    const b = "seq_b" as SequenceId;
    const { store, sync, sets, timers } = setup(engineView({ active_sequence: a }));
    await sync.restore();
    expect(store.getState().activeSequence).toBe(a);
    expect(sets()).toHaveLength(0);
    store.getState().setActiveSequence(b);
    await settle();
    // No debounce: the engine plays what the timeline shows.
    expect(timers.pending).toBe(0);
    expect(sets().at(-1)?.params).toEqual({ patch: { active_sequence: b } });
    // The same choice again sends nothing.
    store.getState().setActiveSequence(b);
    await settle();
    expect(sets()).toHaveLength(1);
  });

  it("the sequence tabs ride the panel layout and come back with it", async () => {
    const a = "seq_a" as SequenceId;
    const b = "seq_b" as SequenceId;
    const { store, sync, sets, timers } = setup();
    await sync.restore();
    store.getState().setSequenceTabs({ order: [b, a], closed: [a] });
    timers.advance(VIEW_SYNC_DEBOUNCE_MS);
    await settle();
    const sent = (sets().at(-1)?.params as { patch: { panel_layout: unknown } }).patch.panel_layout;
    expect((sent as { sequenceTabs: unknown }).sequenceTabs).toEqual({ order: [b, a], closed: [a] });
    // Another launch reads the same blob.
    const next = setup(engineView({ panel_layout: sent as UiState["panel_layout"] }));
    await next.sync.restore();
    expect(next.store.getState().sequenceTabs).toEqual({ order: [b, a], closed: [a] });
    // A blob from before there were tabs: every sequence has one.
    expect(panelLayoutFrom({ ...(sent as object), sequenceTabs: undefined })?.sequenceTabs).toEqual({
      order: [],
      closed: [],
    });
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

  // Issue #233: a project opened (or closed) in the running app shows its own view, as a
  // launch on it does, and nothing of the project before.
  const viewA = () =>
    engineView({
      playhead: 1_411_200_000,
      zoom: 240,
      scroll: { x: 50, y: 6 },
      selection: [{ kind: "item", id: "itm_a" as never }],
      master_gain: 0.5,
      panel_layout: panelLayoutOf({ workspaceId: "long-form", aspect: "16:9", layouts: {} }),
    });
  const viewB = () =>
    engineView({
      playhead: 2_822_400_000,
      zoom: 64,
      scroll: { x: 9, y: 1 },
      selection: [{ kind: "item", id: "itm_b" as never }],
      master_gain: 2,
      panel_layout: panelLayoutOf({ workspaceId: "short-form", aspect: "1:1", layouts: {} }),
    });

  function setupTwo(first: UiState) {
    const held = { view: first, gets: 0 };
    const transport = new MockTransport()
      .handle("view.get", () => {
        held.gets += 1;
        return held.view;
      })
      .handle("view.set", () => held.view);
    const store = createViewStore();
    const timers = new FakeTimers();
    const sync = new ViewSync({ store, rpc: new RpcClient(transport), timers });
    const sets = () => transport.requests.filter((r) => r.method === "view.set");
    return { held, transport, store, timers, sync, sets };
  }

  it("another project opened: its playhead, zoom, scroll, selection, gain and layout are read", async () => {
    const { held, transport, store, timers, sync, sets } = setupTwo(viewA());
    await sync.restore();
    expect(store.getState().zoom).toBe(240);
    held.view = viewB();
    transport.emit("project.opened", { name: "B" });
    await settle();
    const s = store.getState();
    expect(s.playhead).toBe(2_822_400_000);
    expect(s.zoom).toBe(64);
    expect(s.scroll).toEqual({ x: 9, y: 1 });
    expect(s.selection).toEqual([{ kind: "item", id: "itm_b" }]);
    expect(s.masterGain).toBe(2);
    expect(s.workspaceId).toBe("short-form");
    expect(s.aspect).toBe("1:1");
    // What was read is the engine's own: none of it goes back.
    timers.advance(5_000);
    await settle();
    expect(sets()).toHaveLength(0);
    // And back: the first project's view again.
    held.view = viewA();
    transport.emit("project.opened", { name: "A" });
    await settle();
    expect(store.getState().playhead).toBe(1_411_200_000);
    expect(store.getState().zoom).toBe(240);
    expect(store.getState().workspaceId).toBe("long-form");
    timers.advance(5_000);
    await settle();
    expect(sets()).toHaveLength(0);
  });

  it("another project opened: at once nothing of the one before is left", async () => {
    const { held, transport, store, sync } = setupTwo(viewA());
    await sync.restore();
    store.getState().setKeyframeLanes(["itm_a"]);
    store.getState().setMuted(true);
    held.view = viewB();
    transport.emit("project.opened", { name: "B" });
    // Before `view.get` answers: the view of a project nothing is known about yet.
    const s = store.getState();
    expect(s.playhead).toBe(0);
    expect(s.zoom).toBeNull();
    expect(s.scroll).toEqual({ x: 0, y: 0 });
    expect(s.selection).toEqual([]);
    expect(s.keyframeLanes).toEqual([]);
    expect(s.masterGain).toBe(1);
    expect(s.muted).toBe(false);
    // The panels stay where they are until the project's own layout is known.
    expect(s.workspaceId).toBe("long-form");
    await settle();
    expect(store.getState().keyframeLanes).toEqual([]);
    expect(store.getState().selection).toEqual([{ kind: "item", id: "itm_b" }]);
  });

  it("a change still waiting when another project opens is not sent to it", async () => {
    const { held, transport, store, timers, sync, sets } = setupTwo(viewA());
    await sync.restore();
    store.getState().setZoom(300);
    store.getState().setSelection([{ kind: "item", id: "itm_a2" as never }]);
    expect(timers.pending).toBe(1);
    held.view = viewB();
    transport.emit("project.opened", { name: "B" });
    timers.advance(5_000);
    await settle();
    expect(sets()).toHaveLength(0);
    expect(store.getState().zoom).toBe(64);
    // A change made in the opened project goes to it, and only that change.
    store.getState().setZoom(80);
    timers.advance(VIEW_SYNC_DEBOUNCE_MS);
    await settle();
    expect(sets().map((r) => r.params)).toEqual([{ patch: { zoom: 80 } }]);
  });

  it("a project without a saved layout keeps the panels as they are, and sends nothing", async () => {
    const { held, transport, store, timers, sync, sets } = setupTwo(viewA());
    await sync.restore();
    held.view = engineView({ playhead: 5, zoom: 100 });
    transport.emit("project.opened", { name: "New" });
    await settle();
    expect(store.getState().workspaceId).toBe("long-form");
    expect(store.getState().aspect).toBe("16:9");
    expect(store.getState().playhead).toBe(5);
    timers.advance(5_000);
    await settle();
    expect(sets()).toHaveLength(0);
  });

  it("the project closed: the untitled document's view is read, as a launch without a project does", async () => {
    const { held, transport, store, timers, sync, sets } = setupTwo(viewA());
    await sync.restore();
    store.getState().setKeyframeLanes(["itm_a"]);
    const gets = held.gets;
    held.view = engineView({ zoom: 100 });
    transport.emit("project.closed", {});
    expect(store.getState().selection).toEqual([]);
    expect(store.getState().keyframeLanes).toEqual([]);
    expect(store.getState().playhead).toBe(0);
    await settle();
    expect(held.gets).toBe(gets + 1);
    expect(store.getState().zoom).toBe(100);
    expect(store.getState().playhead).toBe(0);
    timers.advance(5_000);
    await settle();
    expect(sets()).toHaveLength(0);
  });

  it("two opens in a row: only the view of the last one is taken", async () => {
    const answers: ((ui: UiState) => void)[] = [];
    let first = true;
    const transport = new MockTransport()
      .handle("view.get", () => {
        if (first) {
          first = false;
          return viewA();
        }
        return new Promise<UiState>((resolve) => answers.push(resolve));
      })
      .handle("view.set", () => viewA());
    const store = createViewStore();
    const sync = new ViewSync({ store, rpc: new RpcClient(transport), timers: new FakeTimers() });
    await sync.restore();
    // The second open comes while the first one's view is on its way.
    transport.emit("project.opened", { name: "B" });
    await settle();
    transport.emit("project.opened", { name: "C" });
    await settle();
    expect(answers).toHaveLength(2);
    answers[1]?.(engineView({ playhead: 33, zoom: 33 }));
    await settle();
    answers[0]?.(viewB());
    await settle();
    expect(store.getState().playhead).toBe(33);
    expect(store.getState().zoom).toBe(33);
  });

  // The view names clips of the opened project: it is read once that project's document is in
  // the mirror, as after a handshake. Read earlier, the timeline would check the selection
  // against the document of the project before and drop it (issue #233).
  it("another project opened: its view is read only once its document is in", async () => {
    const held = { view: viewA(), gets: 0 };
    const transport = new MockTransport()
      .handle("view.get", () => {
        held.gets += 1;
        return held.view;
      })
      .handle("view.set", () => held.view);
    const store = createViewStore();
    let documentIn: () => void = () => {};
    let asked = 0;
    const sync = new ViewSync({
      store,
      rpc: new RpcClient(transport),
      timers: new FakeTimers(),
      documentReady: () => {
        asked += 1;
        return new Promise<void>((resolve) => {
          documentIn = resolve;
        });
      },
    });
    await sync.restore();
    expect(asked).toBe(0); // a handshake has its document already
    const gets = held.gets;
    held.view = viewB();
    transport.emit("project.opened", { name: "B" });
    await settle();
    expect(asked).toBe(1);
    // The old view is gone at once; the new one waits for the document.
    expect(held.gets).toBe(gets);
    expect(store.getState().selection).toEqual([]);
    documentIn();
    await settle();
    expect(held.gets).toBe(gets + 1);
    expect(store.getState().selection).toEqual([{ kind: "item", id: "itm_b" }]);
    expect(store.getState().zoom).toBe(64);
  });

  it("reads the opened project's view all the same when its document could not be read", async () => {
    const held = { view: viewA() };
    const transport = new MockTransport()
      .handle("view.get", () => held.view)
      .handle("view.set", () => held.view);
    const store = createViewStore();
    const errors: unknown[] = [];
    const sync = new ViewSync({
      store,
      rpc: new RpcClient(transport),
      timers: new FakeTimers(),
      onError: (e) => errors.push(e),
      documentReady: () => Promise.reject(new Error("no snapshot")),
    });
    await sync.restore();
    held.view = viewB();
    transport.emit("project.opened", { name: "B" });
    await settle();
    expect(store.getState().zoom).toBe(64);
    expect(errors).toHaveLength(1);
  });

  it("leave() sends what is still waiting before the project is left", async () => {
    const { store, timers, sync, sets } = setupTwo(viewA());
    await sync.restore();
    store.getState().setZoom(300);
    expect(timers.pending).toBe(1);
    await sync.leave();
    expect(sets().map((r) => r.params)).toEqual([{ patch: { zoom: 300 } }]);
    expect(timers.pending).toBe(0);
    // Nothing waiting: nothing sent.
    await sync.leave();
    expect(sets()).toHaveLength(1);
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
      sequenceTabs: { order: [], closed: [] },
    });
  });

  it("the new engine-mirrored fields start at their defaults", () => {
    const s = createViewStore().getState();
    expect(s.playhead).toBe(initialViewState.playhead);
    expect(s.toolMode).toBe("select");
    expect(s.selection).toEqual([]);
  });
});
