// SPDX-License-Identifier: GPL-3.0-or-later
// The transport controller (WP-1M.04): every transport action is a `view.*` engine command
// (arch §10 "Wire"); seeks are latest-wins, JKL follows the engine's shuttle rule, volume and
// mute go to `master_gain`, and `playback.state` moves the view's playhead.
import { MockTransport, RpcClient, RpcError } from "@cutweave/protocol";
import { describe, expect, it, vi } from "vitest";
import { createEngineStore } from "../engine/engineStore.ts";
import { createDocStore } from "../state/docStore.ts";
import { createPlaybackStore } from "../state/playbackStore.ts";
import { createViewStore } from "../state/viewStore.ts";
import { FLICKS, sampleProject } from "../testing/fixtures.ts";
import { nextShuttleRate, sequenceEnd, TransportController } from "./transport.ts";

const state = (over: object = {}) => ({
  state: "stopped",
  playing: false,
  rate: 0,
  t: 0,
  rev: 1,
  quality: "full",
  dropped: 0,
  ...over,
});

function setup() {
  const mock = new MockTransport();
  const engine = createEngineStore();
  engine.setState({ rpc: new RpcClient(mock), connection: "ready" });
  const playback = createPlaybackStore(() => 0);
  const view = createViewStore();
  const doc = createDocStore();
  doc.getState().resync({ rev: 1, committed_rev: 1, project: sampleProject() });
  const unsupported = vi.fn();
  const errors: unknown[] = [];
  const transport = new TransportController({
    engine,
    playback,
    view,
    doc,
    onUnsupported: unsupported,
    onError: (e) => errors.push(e),
  });
  const sent = () => mock.requests.map((r) => [r.method, r.params]);
  return { mock, engine, playback, view, doc, transport, unsupported, errors, sent };
}

describe("nextShuttleRate", () => {
  it("follows the engine's JKL rule", () => {
    expect(nextShuttleRate(null, "L")).toBe(1);
    expect(nextShuttleRate(1, "L")).toBe(2);
    expect(nextShuttleRate(2, "L")).toBe(4);
    expect(nextShuttleRate(4, "L")).toBe(4);
    expect(nextShuttleRate(-2, "L")).toBe(1);
    expect(nextShuttleRate(null, "J")).toBe(-1);
    expect(nextShuttleRate(-1, "J")).toBe(-2);
    expect(nextShuttleRate(-4, "J")).toBe(-4);
    expect(nextShuttleRate(2, "J")).toBe(-1);
    expect(nextShuttleRate(2, "K")).toBeNull();
  });
});

describe("sequenceEnd", () => {
  it("is the end of the last item of the active sequence", () => {
    expect(sequenceEnd(sampleProject())).toBe(5 * FLICKS);
    expect(sequenceEnd(null)).toBe(0);
  });
});

describe("TransportController", () => {
  it("play, pause and toggle send view.play / view.pause and apply the answer", async () => {
    const s = setup();
    s.mock.handle("view.play", () => state({ state: "playing", playing: true, rate: 1 }));
    s.mock.handle("view.pause", () => state({ t: 99 }));
    await s.transport.togglePlay();
    expect(s.playback.getState()).toMatchObject({ playing: true, transport: "available" });
    await s.transport.togglePlay();
    expect(s.playback.getState()).toMatchObject({ playing: false, playhead: 99 });
    expect(s.sent()).toEqual([
      ["view.play", {}],
      ["view.pause", {}],
    ]);
  });

  it("J/K/L send view.set_rate from the current rate, K pauses", async () => {
    const s = setup();
    s.mock.handle("view.set_rate", (p) => {
      const rate = (p as { rate: number }).rate;
      return state({ state: "playing", playing: true, rate });
    });
    s.mock.handle("view.pause", () => state());
    await s.transport.shuttle("L");
    await s.transport.shuttle("L");
    await s.transport.shuttle("J");
    await s.transport.shuttle("K");
    expect(s.sent()).toEqual([
      ["view.set_rate", { rate: 1 }],
      ["view.set_rate", { rate: 2 }],
      ["view.set_rate", { rate: -1 }],
      ["view.pause", {}],
    ]);
  });

  it("step sends whole frames", async () => {
    const s = setup();
    s.mock.handle("view.step", () => state({ t: 23_520_000 }));
    await s.transport.step(1);
    await s.transport.step(-1);
    expect(s.sent()).toEqual([
      ["view.step", { frames: 1 }],
      ["view.step", { frames: -1 }],
    ]);
  });

  it("seek is latest-wins: one request in flight, then only the newest target", async () => {
    const s = setup();
    const answers: ((v: unknown) => void)[] = [];
    s.mock.handle(
      "view.seek",
      (p) => new Promise((r) => answers.push(() => r(state({ t: (p as { t: number }).t })))),
    );
    void s.transport.seek(100);
    void s.transport.seek(200);
    void s.transport.seek(300);
    // The playhead shows the newest choice at once, in the timeline too.
    expect(s.playback.getState().playhead).toBe(300);
    expect(s.view.getState().playhead).toBe(300);
    // An event from before the seek does not move it back while the seek is on its way.
    s.playback.getState().applyState(state({ t: 5 }));
    expect(s.playback.getState().playhead).toBe(300);
    answers.shift()?.(undefined);
    await vi.waitFor(() => expect(answers).toHaveLength(1));
    answers.shift()?.(undefined);
    await vi.waitFor(() => expect(s.playback.getState().holdPlayhead).toBe(false));
    expect(s.sent()).toEqual([
      ["view.seek", { t: 100 }],
      ["view.seek", { t: 300 }],
    ]);
    expect(s.playback.getState().playhead).toBe(300);
  });

  it("go to start / end seek to 0 and to the last frame", async () => {
    const s = setup();
    s.mock.handle("view.seek", (p) => state({ t: (p as { t: number }).t }));
    await s.transport.toEnd();
    await s.transport.toStart();
    expect(s.sent()).toEqual([
      ["view.seek", { t: 5 * FLICKS - FLICKS / 30 }],
      ["view.seek", { t: 0 }],
    ]);
  });

  it("an engine without transport commands is reported once and marked unsupported", async () => {
    const s = setup();
    await s.transport.play();
    await s.transport.step(1);
    expect(s.playback.getState().transport).toBe("unsupported");
    expect(s.unsupported).toHaveBeenCalledTimes(1);
    expect(s.errors).toEqual([]);
    // Seeking still moves the view's playhead (viewSync mirrors it to the engine).
    await s.transport.seek(1234);
    expect(s.view.getState().playhead).toBe(1234);
    expect(s.playback.getState().holdPlayhead).toBe(false);
  });

  it("other failures go to onError", async () => {
    const s = setup();
    s.mock.handle("view.play", () => {
      throw new RpcError("internal", "boom");
    });
    await s.transport.play();
    expect(s.errors).toHaveLength(1);
    expect(s.playback.getState().transport).toBe("unknown");
  });

  it("does nothing without an engine", async () => {
    const s = setup();
    s.engine.setState({ rpc: null, connection: "none" });
    await s.transport.play();
    expect(s.sent()).toEqual([]);
  });

  it("volume and mute set master_gain (latest wins); mute keeps the volume to return to", async () => {
    const s = setup();
    const answers: (() => void)[] = [];
    s.mock.handle("view.set", () => new Promise((r) => answers.push(() => r({}))));
    s.transport.setVolume(0.5);
    s.transport.setVolume(0.6);
    s.transport.setVolume(0.7);
    answers.shift()?.();
    await vi.waitFor(() => expect(answers).toHaveLength(1));
    answers.shift()?.();
    s.transport.toggleMute();
    await vi.waitFor(() => expect(answers).toHaveLength(1));
    answers.shift()?.();
    expect(s.view.getState()).toMatchObject({ masterGain: 0.7, muted: true });
    s.transport.toggleMute();
    await vi.waitFor(() => expect(answers).toHaveLength(1));
    answers.shift()?.();
    expect(s.sent()).toEqual([
      ["view.set", { patch: { master_gain: 0.5 } }],
      ["view.set", { patch: { master_gain: 0.7 } }],
      ["view.set", { patch: { master_gain: 0 } }],
      ["view.set", { patch: { master_gain: 0.7 } }],
    ]);
    // Moving the slider while muted unmutes.
    s.transport.toggleMute();
    s.transport.setVolume(0.3);
    expect(s.view.getState().muted).toBe(false);
  });

  it("attach() moves the view's playhead with playback.state", () => {
    const s = setup();
    const off = s.transport.attach();
    s.playback.getState().applyState(state({ state: "playing", playing: true, rate: 1, t: 4242 }));
    expect(s.view.getState().playhead).toBe(4242);
    off();
    s.playback.getState().applyState(state({ t: 7 }));
    expect(s.view.getState().playhead).toBe(4242);
  });
});
