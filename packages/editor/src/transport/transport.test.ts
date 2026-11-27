// SPDX-License-Identifier: GPL-3.0-or-later
// The transport controller (WP-1M.04): every transport action is a `view.*` engine command
// (arch §10 "Wire"); seeks are latest-wins, JKL follows the engine's shuttle rule, volume and
// mute go to `master_gain`, and `playback.state` moves the view's playhead.
import { type MediaId, MockTransport, RpcClient, RpcError } from "@cutweave/protocol";
import { describe, expect, it, vi } from "vitest";
import { createEngineStore } from "../engine/engineStore.ts";
import { createDocStore } from "../state/docStore.ts";
import { createPlaybackStore, playbackStore } from "../state/playbackStore.ts";
import { sourceViewStore } from "../state/sourceViewStore.ts";
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
  // `playback.return_to_start` as the transport reads it (off, the default).
  const prefs = { returnToStart: false };
  const transport = new TransportController({
    engine,
    playback,
    view,
    doc,
    returnToStart: () => prefs.returnToStart,
    onUnsupported: unsupported,
    onError: (e) => errors.push(e),
  });
  const sent = () => mock.requests.map((r) => [r.method, r.params]);
  return { mock, engine, playback, view, doc, transport, unsupported, errors, sent, prefs };
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

describe("TransportController: the source target (WP-1M.15, ADR-0022)", () => {
  const MEDIA = "med_a" as MediaId;
  const SOURCE = { media: MEDIA, duration: 4 * FLICKS, fps: { num: 30, den: 1 } };
  const playingSource = (over: object = {}) =>
    state({ state: "playing", playing: true, rate: 1, source: SOURCE, ...over });

  it("play is view.set_source then view.play, hosted in the Preview by default", async () => {
    const s = setup();
    sourceViewStore.getState().setHost("inspector");
    s.mock.handle("view.set_source", () => state({ source: SOURCE }));
    s.mock.handle("view.play", () => playingSource());
    await s.transport.playSource(MEDIA);
    expect(s.sent()).toEqual([
      ["view.set_source", { media: MEDIA }],
      ["view.play", {}],
    ]);
    expect(s.playback.getState()).toMatchObject({ playing: true, source: SOURCE });
    expect(sourceViewStore.getState().host).toBe("preview");
  });

  it("a source the engine refuses (a still) leaves the host and the transport alone", async () => {
    const s = setup();
    s.mock.handle("view.set_source", () => {
      throw new RpcError("invalid", "a still cannot play");
    });
    await s.transport.playSource(MEDIA, "inspector");
    expect(s.sent()).toEqual([["view.set_source", { media: MEDIA }]]);
    expect(sourceViewStore.getState().host).toBe("preview");
    expect(s.playback.getState().source).toBeNull();
  });

  it("does not restart a source that is set, and starts from a seek position", async () => {
    const s = setup();
    s.playback.getState().applyState(playingSource());
    s.mock.handle("view.seek", () => playingSource({ t: FLICKS }));
    s.mock.handle("view.play", () => playingSource());
    await s.transport.playSource(MEDIA, "inspector", FLICKS);
    expect(s.sent()).toEqual([
      ["view.seek", { t: FLICKS }],
      ["view.play", {}],
    ]);
    expect(sourceViewStore.getState().host).toBe("inspector");
  });

  it("toggleSource pauses what plays in that host and moves it from the other one", async () => {
    const s = setup();
    s.playback.getState().applyState(playingSource());
    sourceViewStore.getState().setHost("inspector");
    s.mock.handle("view.pause", () => state({ source: SOURCE }));
    s.mock.handle("view.play", () => playingSource());
    await s.transport.toggleSource(MEDIA, "preview");
    // Playing in the Inspector: the bin's play moves it to the Preview, still playing.
    expect(s.sent()).toEqual([["view.play", {}]]);
    expect(sourceViewStore.getState().host).toBe("preview");
    await s.transport.toggleSource(MEDIA, "preview");
    expect(s.sent().at(-1)).toEqual(["view.pause", {}]);
  });

  it("seek moves the source's playhead (its own time) and never the timeline's", async () => {
    const s = setup();
    s.view.getState().setPlayhead(2 * FLICKS);
    s.playback.getState().applyState(state({ t: 2 * FLICKS }));
    s.playback.getState().applyState(playingSource({ t: 0 }));
    s.mock.handle("view.seek", (p) => playingSource({ t: (p as { t: number }).t }));
    await s.transport.seek(FLICKS);
    expect(s.sent()).toEqual([["view.seek", { t: FLICKS }]]);
    expect(s.playback.getState()).toMatchObject({ sourcePlayhead: FLICKS, playhead: 2 * FLICKS });
    expect(s.view.getState().playhead).toBe(2 * FLICKS);
  });

  it("go to end uses the source's length and rate, not the sequence's", async () => {
    const s = setup();
    s.playback.getState().applyState(playingSource());
    s.mock.handle("view.seek", (p) => playingSource({ t: (p as { t: number }).t }));
    await s.transport.toEnd();
    // 4 s long at 30 fps: the last frame starts one frame before the end.
    expect(s.sent()).toEqual([["view.seek", { t: 4 * FLICKS - FLICKS / 30 }]]);
    expect(s.view.getState().playhead).toBe(0);
  });

  it("closeSource returns to the timeline at the playhead it kept", async () => {
    const s = setup();
    s.view.getState().setPlayhead(2 * FLICKS);
    s.playback.getState().applyState(state({ t: 2 * FLICKS }));
    s.playback.getState().applyState(playingSource());
    sourceViewStore.getState().setHost("inspector");
    s.mock.handle("view.set_source", () => state({ t: 2 * FLICKS, source: null }));
    await s.transport.closeSource();
    expect(s.sent()).toEqual([["view.set_source", {}]]);
    expect(s.playback.getState()).toMatchObject({ source: null, playhead: 2 * FLICKS });
    expect(sourceViewStore.getState().host).toBe("preview");
    // With no source there is nothing to close.
    await s.transport.closeSource();
    expect(s.sent()).toHaveLength(1);
  });

  it("a source that ends by itself (removed asset, another project) returns the Preview", () => {
    playbackStore.getState().applyState(playingSource());
    sourceViewStore.getState().setHost("inspector");
    playbackStore.getState().applyState(state({ source: null }));
    expect(sourceViewStore.getState().host).toBe("preview");
  });
});

describe("TransportController: go back to where playback started (WP-2H.10)", () => {
  const playing = (t: number, over: object = {}) =>
    state({ state: "playing", playing: true, rate: 1, t, ...over });

  /** A transport that follows `playback.state`, paused at 1 s, with the usual engine answers. */
  function started(returnToStart: boolean) {
    const s = setup();
    s.prefs.returnToStart = returnToStart;
    s.transport.attach();
    s.playback.getState().applyState(state({ t: FLICKS }));
    s.mock.handle("view.play", () => playing(FLICKS));
    s.mock.handle("view.pause", () => state({ t: 3 * FLICKS }));
    s.mock.handle("view.seek", (p) => state({ t: (p as { t: number }).t }));
    s.mock.handle("view.step", () => state({ t: 2 * FLICKS }));
    return s;
  }

  it("off (the default): a pause stays where playback stopped", async () => {
    const s = started(false);
    await s.transport.play();
    await s.transport.pause();
    expect(s.sent()).toEqual([
      ["view.play", {}],
      ["view.pause", {}],
    ]);
    expect(s.view.getState().playhead).toBe(3 * FLICKS);
  });

  it("on: a pause seeks back to the position playback started from", async () => {
    const s = started(true);
    await s.transport.togglePlay();
    s.playback.getState().applyState(playing(2 * FLICKS));
    await s.transport.togglePlay();
    await vi.waitFor(() => expect(s.sent().at(-1)).toEqual(["view.seek", { t: FLICKS }]));
    await vi.waitFor(() => expect(s.playback.getState().holdPlayhead).toBe(false));
    expect(s.view.getState().playhead).toBe(FLICKS);
    expect(s.playback.getState()).toMatchObject({ playing: false, playhead: FLICKS });
  });

  it("follows the preference when it changes, with no restart", async () => {
    const s = started(false);
    await s.transport.play();
    s.prefs.returnToStart = true;
    await s.transport.pause();
    await vi.waitFor(() => expect(s.sent().at(-1)).toEqual(["view.seek", { t: FLICKS }]));
  });

  it("K returns too; a seek made while playing is where it returns to", async () => {
    const s = started(true);
    s.mock.handle("view.set_rate", () => playing(FLICKS));
    s.mock.handle("view.seek", (p) => playing((p as { t: number }).t));
    await s.transport.shuttle("L");
    await s.transport.seek(2 * FLICKS);
    s.mock.handle("view.seek", (p) => state({ t: (p as { t: number }).t }));
    await s.transport.shuttle("K");
    await vi.waitFor(() => expect(s.sent().at(-1)).toEqual(["view.seek", { t: 2 * FLICKS }]));
  });

  it("playback that reaches the end returns; started at the end, it returns to the start", async () => {
    const s = started(true);
    await s.transport.play();
    // The engine stops by itself at the end of the content (5 s in the sample project).
    s.playback.getState().applyState(state({ t: 5 * FLICKS }));
    await vi.waitFor(() => expect(s.sent().at(-1)).toEqual(["view.seek", { t: FLICKS }]));
    await vi.waitFor(() => expect(s.playback.getState().holdPlayhead).toBe(false));
    // Play at the end starts again from 0 (`view.play`): that is where it began.
    s.playback.getState().applyState(state({ t: 5 * FLICKS }));
    s.mock.handle("view.play", () => playing(0));
    await s.transport.play();
    await s.transport.pause();
    await vi.waitFor(() => expect(s.sent().at(-1)).toEqual(["view.seek", { t: 0 }]));
  });

  it("a step, or a pause someone else made (an agent), stays where it is", async () => {
    const s = started(true);
    await s.transport.play();
    await s.transport.step(1);
    expect(s.sent().at(-1)).toEqual(["view.step", { frames: 1 }]);
    expect(s.view.getState().playhead).toBe(2 * FLICKS);
    await s.transport.play();
    s.playback.getState().applyState(state({ t: 3 * FLICKS }));
    expect(s.sent().filter(([method]) => method === "view.seek")).toEqual([]);
    expect(s.view.getState().playhead).toBe(3 * FLICKS);
  });

  it("a source returns in its own time and leaves the timeline's playhead alone", async () => {
    const media = "med_a" as MediaId;
    const source = { media, duration: 4 * FLICKS, fps: { num: 30, den: 1 } };
    const s = started(true);
    s.playback.getState().applyState(state({ source, t: FLICKS / 2 }));
    s.mock.handle("view.play", () => playing(FLICKS / 2, { source }));
    s.mock.handle("view.pause", () => state({ source, t: 2 * FLICKS }));
    s.mock.handle("view.seek", (p) => state({ source, t: (p as { t: number }).t }));
    await s.transport.play();
    await s.transport.pause();
    await vi.waitFor(() => expect(s.sent().at(-1)).toEqual(["view.seek", { t: FLICKS / 2 }]));
    expect(s.playback.getState().sourcePlayhead).toBe(FLICKS / 2);
    expect(s.view.getState().playhead).toBe(FLICKS);
  });
});
