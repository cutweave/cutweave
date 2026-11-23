// SPDX-License-Identifier: GPL-3.0-or-later
import type { PatchOp, Project } from "@cutweave/protocol";
import {
  FakeEngine,
  flushPromises,
  ManualTimers,
  type SentMessage,
} from "@cutweave/protocol/test-support/fake-socket";
import { describe, expect, it } from "vitest";
import { createDocStore } from "../state/docStore.ts";
import { createJobStore } from "../state/jobStore.ts";
import { createPlaybackStore } from "../state/playbackStore.ts";
import { createViewStore } from "../state/viewStore.ts";
import { createEngineStore } from "./engineStore.ts";
import { bridgeFromWindow, type EngineBridge, startEngineRuntime } from "./runtime.ts";

const project = (name: string): Project =>
  ({ meta: { name }, settings: {}, media: {}, sequences: {}, custom: {} }) as unknown as Project;

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

  it("bridgeFromWindow finds window.cutweave.engine or reports none", () => {
    const engine: EngineBridge = { getConnection: () => Promise.resolve(null) };
    expect(bridgeFromWindow({ cutweave: { engine } })).toBe(engine);
    expect(bridgeFromWindow({})).toBeNull();
  });
});
