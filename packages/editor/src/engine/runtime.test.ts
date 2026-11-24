// SPDX-License-Identifier: GPL-3.0-or-later
import type { PatchOp, Project } from "@cutweave/protocol";
import {
  FakeEngine,
  flushPromises,
  ManualTimers,
  type SentMessage,
} from "@cutweave/protocol/test-support/fake-socket";
import { describe, expect, it } from "vitest";
import { createPrefsStore } from "../prefs/prefsStore.ts";
import { createDocStore } from "../state/docStore.ts";
import { createJobStore } from "../state/jobStore.ts";
import { createPlaybackStore } from "../state/playbackStore.ts";
import { createViewStore } from "../state/viewStore.ts";
import { createEngineStore, type SupervisorStatus } from "./engineStore.ts";
import {
  bridgeFromWindow,
  type EngineBridge,
  startEngineRuntime,
  supervisorHint,
} from "./runtime.ts";

const project = (name: string): Project =>
  ({ meta: { name }, settings: {}, media: {}, sequences: {}, custom: {} }) as unknown as Project;

/** Sets the saving preferences the way the Settings dialog does (through the store). */
function setAutosave(
  stores: { prefs: ReturnType<typeof createPrefsStore> },
  change: { enabled?: boolean; intervalS?: 30 | 60 | 120 | 300; keepBackups?: 5 | 10 | 20 },
): void {
  const values = stores.prefs.getState().values;
  stores.prefs.setState({
    values: {
      ...values,
      ...(change.enabled === undefined ? {} : { "saving.autosave": change.enabled }),
      ...(change.intervalS === undefined ? {} : { "saving.interval": change.intervalS }),
      ...(change.keepBackups === undefined ? {} : { "saving.backups": change.keepBackups }),
    },
  });
}

function setup() {
  const engine = new FakeEngine();
  const timers = new ManualTimers();
  let live = { rev: 3, name: "one" };
  engine.handler = (_socket, m: SentMessage) => {
    switch (m.method) {
      case "project.snapshot":
        return { rev: live.rev, committed_rev: live.rev, project: project(live.name) };
      case "view.get":
        return {
          playhead: 11,
          zoom: 90,
          scroll: { x: 0, y: 0 },
          selection: [],
          panel_layout: null,
          preview_quality: "full",
          master_gain: null,
        };
      case "job.list":
        return { jobs: [], active: 0 };
      default:
        return {};
    }
  };
  let port = 5000;
  const bridge: EngineBridge = {
    getConnection: () =>
      Promise.resolve({ port, wsUrl: `ws://127.0.0.1:${port}/rpc`, token: "tok" }),
  };
  const stores = {
    prefs: createPrefsStore(),
    doc: createDocStore(),
    view: createViewStore(),
    playback: createPlaybackStore(),
    jobs: createJobStore(),
    engine: createEngineStore(),
  };
  const errors: unknown[] = [];
  const runtime = startEngineRuntime(bridge, stores, {
    connection: { createSocket: engine.factory, timers },
    view: { timers },
    onError: (e) => errors.push(e),
  });
  return {
    errors,
    engine,
    timers,
    stores,
    runtime,
    live: (next: { rev: number; name: string }) => {
      live = next;
    },
    setPort: (p: number) => {
      port = p;
    },
  };
}

const methods = (engine: FakeEngine, index = 0) => engine.sockets[index]?.methods() ?? [];

describe("startEngineRuntime", () => {
  it("connects, subscribes, then fills the stores from the engine", async () => {
    const { engine, stores, runtime, errors } = setup();
    await runtime.connection.whenReady();
    await flushPromises(50);
    // Subscriptions before the snapshot (rpc.md resync rule).
    const order = methods(engine);
    expect(order.indexOf("rpc.subscribe")).toBeGreaterThan(-1);
    expect(order.indexOf("rpc.subscribe")).toBeLessThan(order.indexOf("project.snapshot"));
    expect(stores.doc.getState()).toMatchObject({ status: "ready", rev: 3 });
    expect(stores.view.getState()).toMatchObject({ playhead: 11, zoom: 90 });
    expect(stores.engine.getState()).toMatchObject({ connection: "ready" });
    expect(stores.engine.getState().rpc).toBe(runtime.rpc);
    expect(errors).toEqual([]);
    await runtime.stop();
  });

  it("applies doc.patched, job.progress and playback.state events", async () => {
    const { engine, stores, runtime } = setup();
    await runtime.connection.whenReady();
    await flushPromises(50);
    const ops: PatchOp[] = [
      { op: "replace", path: "/meta/name", old: "one", value: "two" } as PatchOp,
    ];
    engine.last.emit("doc.patched", {
      rev: 4,
      prev_rev: 3,
      committed_rev: 4,
      actor: "user",
      tx_id: "tx_1",
      cause: "command",
      ops,
    });
    engine.last.emit("job.progress", {
      id: "job_1",
      kind: "analysis",
      label: "index",
      media_id: null,
      state: "running",
      progress: { fraction: 0.5, message: null },
      cancel_requested: false,
    });
    engine.last.emit("playback.state", { state: "playing", rate: 1, playhead: 99 });
    expect(stores.doc.getState().project?.meta.name).toBe("two");
    expect(stores.jobs.getState().jobs["job_1" as never]?.state).toBe("running");
    expect(stores.playback.getState()).toMatchObject({ mode: "playing", playhead: 99 });
    await runtime.stop();
  });

  it("records the preview frame ring each handshake announces (rpc.hello shm, WP-1M.04)", async () => {
    const { engine, timers, stores, runtime } = setup();
    expect(stores.engine.getState().previewRing).toBeUndefined();
    await runtime.connection.whenReady();
    await flushPromises(50);
    // This engine publishes no ring.
    expect(stores.engine.getState().previewRing).toBeNull();
    const ring = { name: "cwp-00000001-0000000000000002", epoch: "0x0000000000000003" };
    engine.hello = { ...engine.hello, shm: { ...ring, layout_version: 1, mapping_size: 4096 } };
    engine.last.serverClose(1006);
    timers.advance(250);
    await runtime.connection.whenReady();
    await flushPromises(50);
    expect(stores.engine.getState().previewRing).toEqual(ring);
    await runtime.stop();
    expect(stores.engine.getState().previewRing).toBeUndefined();
  });

  it("after a reconnect it resubscribes and takes one fresh snapshot", async () => {
    const { engine, timers, stores, runtime, live } = setup();
    await runtime.connection.whenReady();
    await flushPromises(50);
    live({ rev: 20, name: "after restart" });
    engine.last.serverClose(1006);
    expect(stores.engine.getState().connection).toBe("reconnecting");
    timers.advance(250);
    await runtime.connection.whenReady();
    await flushPromises(50);
    expect(engine.sockets).toHaveLength(2);
    expect(methods(engine, 1).filter((m) => m === "project.snapshot")).toHaveLength(1);
    expect(methods(engine, 1).indexOf("rpc.subscribe")).toBeLessThan(
      methods(engine, 1).indexOf("project.snapshot"),
    );
    expect(stores.doc.getState()).toMatchObject({ rev: 20 });
    expect(stores.doc.getState().project?.meta.name).toBe("after restart");
    await runtime.stop();
    expect(stores.engine.getState()).toMatchObject({ connection: "none", rpc: null });
  });

  it("tells the engine the autosave preference after every handshake and when it changes (WP-1M.10)", async () => {
    const { engine, timers, stores, runtime, errors } = setup();
    setAutosave(stores, { enabled: false, intervalS: 120, keepBackups: 20 });
    await runtime.connection.whenReady();
    await flushPromises(50);
    const sent = (index: number) =>
      (engine.sockets[index]?.sent ?? []).filter((m) => m.method === "project.set_autosave");
    expect(sent(0).map((m) => m.params)).toMatchObject([
      { enabled: false, interval_s: 120, keep_backups: 20 },
    ]);
    // Before the document is read: an engine that saves must know how before edits arrive.
    expect(methods(engine).indexOf("project.set_autosave")).toBeLessThan(
      methods(engine).indexOf("project.snapshot"),
    );
    setAutosave(stores, { enabled: true });
    await flushPromises(50);
    expect(
      sent(0)
        .map((m) => m.params)
        .at(-1),
    ).toMatchObject({
      enabled: true,
      interval_s: 120,
      keep_backups: 20,
    });
    // A new engine launch (reconnect) gets it again.
    engine.last.serverClose(1006);
    timers.advance(250);
    await runtime.connection.whenReady();
    await flushPromises(50);
    expect(sent(1).map((m) => m.params)).toMatchObject([
      { enabled: true, interval_s: 120, keep_backups: 20 },
    ]);
    expect(errors).toEqual([]);
    await runtime.stop();
  });

  it("tells the engine this computer's cache and hardware preferences after every handshake and when they change (WP-1D.08)", async () => {
    const { engine, timers, stores, runtime, errors } = setup();
    const set = (change: Record<string, unknown>) =>
      stores.prefs.setState({ values: { ...stores.prefs.getState().values, ...change } });
    set({ "performance.hw_decode": "off", "cache.thumbnails_limit": 1_073_741_824 });
    await runtime.connection.whenReady();
    await flushPromises(50);
    const sent = (index: number, method: string) =>
      (engine.sockets[index]?.sent ?? []).filter((m) => m.method === method).map((m) => m.params);
    expect(sent(0, "media.set_decode_policy")).toMatchObject([{ hw: "off" }]);
    expect(sent(0, "media.set_cache_config")).toMatchObject([
      {
        limits: { thumbnails: 1_073_741_824, waveforms: 536_870_912 },
        frame_cache_bytes: 0,
      },
    ]);
    set({ "performance.frame_memory": 2_147_483_648 });
    await flushPromises(50);
    expect(sent(0, "media.set_cache_config").at(-1)).toMatchObject({
      frame_cache_bytes: 2_147_483_648,
    });
    set({ "performance.hw_decode": "auto" });
    await flushPromises(50);
    expect(sent(0, "media.set_decode_policy").at(-1)).toMatchObject({ hw: "auto" });
    // The cache folder is main's (a launch flag), not an engine command.
    const before = sent(0, "media.set_cache_config").length;
    set({ "cache.folder": "D:\\cache" });
    await flushPromises(50);
    expect(sent(0, "media.set_cache_config")).toHaveLength(before);
    // A new engine launch gets all of it again.
    engine.last.serverClose(1006);
    timers.advance(250);
    await runtime.connection.whenReady();
    await flushPromises(50);
    expect(sent(1, "media.set_decode_policy")).toMatchObject([{ hw: "auto" }]);
    expect(sent(1, "media.set_cache_config")).toHaveLength(1);
    expect(errors).toEqual([]);
    await runtime.stop();
  });

  it("bridgeFromWindow finds window.cutweave.engine or reports none", () => {
    const engine: EngineBridge = { getConnection: () => Promise.resolve(null) };
    expect(bridgeFromWindow({ cutweave: { engine } })).toBe(engine);
    expect(bridgeFromWindow({})).toBeNull();
  });
});

// ----- engine restarts (WP-1L.03) ----------------------------------------------------------------

/** A runtime under a fake supervisor: the test drives `status` and what `getConnection` returns. */
function supervised() {
  const engine = new FakeEngine();
  const timers = new ManualTimers();
  engine.handler = (_socket, m: SentMessage) => {
    switch (m.method) {
      case "project.snapshot":
        return { rev: 1, committed_rev: 1, project: project("p") };
      case "view.get":
        return {
          playhead: 0,
          zoom: 100,
          scroll: { x: 0, y: 0 },
          selection: [],
          panel_layout: null,
          preview_quality: "full",
          master_gain: null,
        };
      case "job.list":
        return { jobs: [], active: 0 };
      default:
        // The first engine never answers a command; later ones do.
        return engine.sockets.length > 1 ? { track: "trk_new" } : undefined;
    }
  };
  let status: SupervisorStatus = { state: "ready", generation: 1 };
  const listeners = new Set<(s: SupervisorStatus) => void>();
  const bridge: EngineBridge = {
    getConnection: () =>
      Promise.resolve(
        status.state === "ready"
          ? {
              wsUrl: "ws://127.0.0.1:5000/rpc",
              token: `tok-${status.generation}`,
              generation: status.generation ?? 0,
            }
          : null,
      ),
    getStatus: () => Promise.resolve(status),
    onStatus: (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
  const stores = {
    doc: createDocStore(),
    view: createViewStore(),
    playback: createPlaybackStore(),
    jobs: createJobStore(),
    engine: createEngineStore(),
  };
  const errors: unknown[] = [];
  const runtime = startEngineRuntime(bridge, stores, {
    connection: { createSocket: engine.factory, timers },
    view: { timers },
    onError: (e) => errors.push(e),
  });
  const states: string[] = [];
  runtime.connection.onStateChange((s) => states.push(s));
  return {
    engine,
    timers,
    stores,
    runtime,
    errors,
    states,
    emit(next: SupervisorStatus) {
      status = next;
      for (const listener of listeners) listener(next);
    },
  };
}

const trackAdd = { sequence: "seq_1" as never, kind: "video" as never };
const requestIdOf = (m: SentMessage | undefined) =>
  (m?.params._meta as { request_id?: string } | undefined)?.request_id;

describe("startEngineRuntime across engine restarts", () => {
  it("re-sends an unanswered command after the new snapshot, under its request_id", async () => {
    const s = supervised();
    await s.runtime.connection.whenReady();
    await flushPromises(50);
    const call = s.runtime.rpc.call("track.add", trackAdd);
    await flushPromises();
    const first = s.engine.last.sent.find((m) => m.method === "track.add");
    s.engine.last.serverClose(1006);
    s.emit({ state: "crashed", generation: 1 });
    s.emit({ state: "restarting", generation: 1 });
    // A command fired during the outage waits as well.
    const during = s.runtime.rpc.call("track.add", trackAdd);
    s.emit({
      state: "ready",
      generation: 2,
      recovery: { project: "/w/film.cutweave", applied: 1, damaged: false },
    });
    await s.runtime.connection.whenReady();
    await flushPromises(80);
    const order = methods(s.engine, 1);
    expect(order.indexOf("project.snapshot")).toBeGreaterThan(-1);
    expect(order.indexOf("project.snapshot")).toBeLessThan(order.indexOf("track.add"));
    const resent = s.engine.last.sent.filter((m) => m.method === "track.add");
    expect(resent.map(requestIdOf)).toEqual([requestIdOf(first), expect.any(String)]);
    expect(requestIdOf(resent[1])).not.toBe(requestIdOf(first));
    await expect(call).resolves.toEqual({ track: "trk_new" });
    await expect(during).resolves.toEqual({ track: "trk_new" });
    // The new engine was reached with the new token, and the notice says what came back.
    expect(s.engine.last.protocols).toEqual(["cutweave.token.tok-2"]);
    expect(s.stores.engine.getState().notice).toMatchObject({
      recovery: { project: "/w/film.cutweave", applied: 1 },
    });
    expect(s.errors).toEqual([]);
    await s.runtime.stop();
  });

  it("waits out a long restart without giving up, then reconnects at once on ready", async () => {
    const s = supervised();
    await s.runtime.connection.whenReady();
    await flushPromises(50);
    s.emit({ state: "restarting", generation: 1 });
    s.engine.last.serverClose(1006);
    for (let i = 0; i < 120; i++) {
      s.timers.advance(1000);
      await flushPromises();
    }
    expect(s.states).not.toContain("failed");
    expect(s.runtime.connection.state).toBe("reconnecting");
    s.emit({ state: "ready", generation: 2 });
    await s.runtime.connection.whenReady();
    await flushPromises(50);
    expect(s.engine.last.protocols).toEqual(["cutweave.token.tok-2"]);
    // No project was open: the notice says nothing could be restored.
    expect(s.stores.engine.getState().notice).toMatchObject({ recovery: null });
    await s.runtime.stop();
  });

  it("leaves a socket to a replaced engine as soon as main reports the new one", async () => {
    const s = supervised();
    await s.runtime.connection.whenReady();
    await flushPromises(50);
    s.emit({ state: "ready", generation: 2 });
    expect(s.runtime.connection.state).toBe("reconnecting");
    s.timers.advance(0);
    await s.runtime.connection.whenReady();
    await flushPromises(50);
    expect(s.engine.sockets).toHaveLength(2);
    expect(s.engine.sockets[0]?.closedByClient).not.toBeNull();
    expect(s.engine.last.protocols).toEqual(["cutweave.token.tok-2"]);
    await s.runtime.stop();
  });

  it("blocks editing while main reports failed, rejecting what waits; ready unblocks", async () => {
    const s = supervised();
    await s.runtime.connection.whenReady();
    await flushPromises(50);
    s.engine.last.serverClose(1006);
    s.emit({ state: "crashed", generation: 1 });
    const waiting = s.runtime.rpc.call("track.add", trackAdd);
    s.emit({ state: "failed", generation: 1, message: "The engine stopped 4 times in 60 s." });
    await expect(waiting).rejects.toMatchObject({ code: "unsupported" });
    expect(s.stores.engine.getState().blocked).toEqual({
      kind: "engine-failed",
      message: "The engine stopped 4 times in 60 s.",
    });
    s.timers.advance(2000);
    await flushPromises(20);
    expect(s.runtime.connection.state).toBe("failed");
    // "Restart" in main's dialog.
    s.emit({ state: "ready", generation: 2 });
    await s.runtime.connection.whenReady();
    await flushPromises(50);
    expect(s.stores.engine.getState().blocked).toBeNull();
    await s.runtime.stop();
  });

  it("blocks editing on a schema mismatch and never re-sends to that engine", async () => {
    const s = supervised();
    await s.runtime.connection.whenReady();
    await flushPromises(50);
    const call = s.runtime.rpc.call("track.add", trackAdd);
    await flushPromises();
    s.engine.last.serverClose(1006);
    s.engine.hello = { ...s.engine.hello, schema_hash: "another", engine_version: "9.9.9" };
    s.emit({ state: "ready", generation: 2 });
    await s.runtime.connection.whenReady();
    await flushPromises(50);
    await expect(call).rejects.toMatchObject({ code: "unsupported" });
    expect(s.engine.last.methods()).not.toContain("track.add");
    expect(s.stores.engine.getState().blocked).toEqual({
      kind: "schema-mismatch",
      engineVersion: "9.9.9",
    });
    await s.runtime.stop();
  });

  it("maps the supervisor's states to reconnect hints", () => {
    expect(supervisorHint(null)).toBe("unknown");
    expect(supervisorHint({ state: "ready" })).toBe("up");
    for (const state of ["starting", "crashed", "restarting"]) {
      expect(supervisorHint({ state })).toBe("restarting");
    }
    for (const state of ["failed", "stopped", "unavailable"]) {
      expect(supervisorHint({ state })).toBe("down");
    }
  });
});
