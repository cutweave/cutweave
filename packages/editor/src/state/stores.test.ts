// SPDX-License-Identifier: GPL-3.0-or-later
import {
  type DocPatched,
  DocSync,
  type JobId,
  type JobInfo,
  MockTransport,
  type PatchOp,
  type Project,
  RpcClient,
} from "@cutweave/protocol";
import { describe, expect, it } from "vitest";
import { createDocStore, docSink } from "./docStore.ts";
import { createJobStore, isActive, selectActiveJobs } from "./jobStore.ts";
import { createPlaybackStore, parsePlaybackState } from "./playbackStore.ts";

const project = (name: string): Project =>
  ({ meta: { name }, settings: {}, media: {}, sequences: {}, custom: {} }) as unknown as Project;

const patched = (rev: number, prev: number, from: string, to: string): DocPatched =>
  ({
    rev,
    prev_rev: prev,
    committed_rev: rev,
    actor: "user",
    tx_id: `tx_${rev}`,
    cause: "command",
    request_id: null,
    ops: [{ op: "replace", path: "/meta/name", old: from, value: to }] as PatchOp[],
  }) as unknown as DocPatched;

describe("docStore", () => {
  it("starts empty; applyEvent and resync are its only writers, republish changes nothing", () => {
    const store = createDocStore();
    const s = store.getState();
    expect(s.status).toBe("empty");
    expect(s.project).toBeNull();
    const actions = Object.keys(s).filter((k) => typeof s[k as keyof typeof s] === "function");
    expect(actions.sort()).toEqual(["applyEvent", "republish", "resync"]);
    // Republishing (the editor shows another sequence, WP-2B.03) tells the subscribers to read
    // the document again: the same revision and the same contents, every part shared.
    store.getState().republish(); // no document: nothing to publish
    expect(store.getState().project).toBeNull();
    store.getState().resync({ rev: 3, committed_rev: 3, project: project("a") });
    const before = store.getState();
    let published = 0;
    store.subscribe(() => published++);
    store.getState().republish();
    const after = store.getState();
    expect(published).toBe(1);
    expect(after.rev).toBe(before.rev);
    expect(after.project).toEqual(before.project);
    expect(after.project).not.toBe(before.project);
    expect(after.project?.sequences).toBe(before.project?.sequences);
    expect(after.project?.media).toBe(before.project?.media);
  });

  it("applies the next revision, drops stale ones and reports gaps without changing", () => {
    const store = createDocStore();
    store.getState().resync({ rev: 1, committed_rev: 1, project: project("a") });
    expect(store.getState().applyEvent(patched(2, 1, "a", "b"))).toBe("applied");
    expect(store.getState().project?.meta.name).toBe("b");
    expect(store.getState().applyEvent(patched(2, 1, "a", "b"))).toBe("stale");
    const before = store.getState().project;
    expect(store.getState().applyEvent(patched(9, 8, "b", "c"))).toBe("gap");
    expect(store.getState().project).toBe(before);
    expect(store.getState().rev).toBe(2);
  });

  it("shares unchanged parts of the document between revisions", () => {
    const store = createDocStore();
    const doc = project("a");
    store.getState().resync({ rev: 1, committed_rev: 1, project: doc });
    store.getState().applyEvent(patched(2, 1, "a", "b"));
    expect(store.getState().project?.sequences).toBe(doc.sequences);
  });

  it("is driven by DocSync: a forced gap costs exactly one snapshot", async () => {
    let live = { rev: 1, name: "a" };
    const transport = new MockTransport().handle("project.snapshot", () => ({
      rev: live.rev,
      committed_rev: live.rev,
      project: project(live.name),
    }));
    const store = createDocStore();
    const sync = new DocSync(new RpcClient(transport), docSink(store));
    await sync.start();
    live = { rev: 7, name: "seven" };
    transport.emit("doc.patched", patched(7, 6, "a", "seven"));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(transport.requests.filter((r) => r.method === "project.snapshot")).toHaveLength(2);
    expect(store.getState().rev).toBe(7);
  });
});

describe("playbackStore", () => {
  it("reads the fields of a playback.state payload it understands", () => {
    const store = createPlaybackStore();
    store.getState().applyState({ state: "playing", rate_milli: 2000, pts: 705_600_000 });
    expect(store.getState()).toMatchObject({ mode: "playing", rate: 2, playhead: 705_600_000 });
    store.getState().applyState({ state: "stopped", rate: 1, quality: "half" });
    expect(store.getState()).toMatchObject({ mode: "stopped", rate: 1, quality: "half" });
  });

  it("reads the engine's PlaybackState (t, playing, dropped) and stamps each playhead sample", () => {
    let now = 10;
    const store = createPlaybackStore(() => now);
    store.getState().applyState({
      state: "playing",
      playing: true,
      rate: 1,
      t: 2_000,
      rev: 3,
      quality: "quarter",
      dropped: 2,
    });
    expect(store.getState()).toMatchObject({
      mode: "playing",
      playing: true,
      playhead: 2_000,
      quality: "quarter",
      dropped: 2,
      sampledAt: 10,
    });
    now = 20;
    store.getState().applyState({ state: "stopped", rate: 0 });
    expect(store.getState()).toMatchObject({ playing: false, sampledAt: 10 });
  });

  it("reads whether the preview uses proxies and whether the frame shows one (WP-2F.02)", () => {
    const store = createPlaybackStore(() => 0);
    // Until the engine says otherwise: proxies on, nothing from a proxy on screen.
    expect(store.getState()).toMatchObject({ useProxies: true, proxy: false });
    store.getState().applyState({ state: "stopped", use_proxies: true, proxy: true });
    expect(store.getState()).toMatchObject({ useProxies: true, proxy: true });
    store.getState().applyState({ use_proxies: false, proxy: false });
    expect(store.getState()).toMatchObject({ useProxies: false, proxy: false });
    // A payload without the fields (an older engine, the fake) changes neither.
    store.getState().applyState({ state: "playing", proxy: "yes", use_proxies: 1 });
    expect(store.getState()).toMatchObject({ useProxies: false, proxy: false, mode: "playing" });
  });

  it("holds the playhead while a seek is on its way", () => {
    const store = createPlaybackStore(() => 0);
    store.getState().setPlayhead(500);
    store.getState().setHoldPlayhead(true);
    store.getState().applyState({ state: "scrubbing", t: 1 });
    expect(store.getState()).toMatchObject({ mode: "scrubbing", playhead: 500 });
    store.getState().setHoldPlayhead(false);
    store.getState().applyState({ t: 1 });
    expect(store.getState().playhead).toBe(1);
  });

  it("ignores unusable payloads", () => {
    const store = createPlaybackStore();
    const before = store.getState();
    for (const bad of [null, 3, "x", { state: "flying", rate: "fast", playhead: -1 }]) {
      store.getState().applyState(bad);
    }
    expect(store.getState()).toBe(before);
    expect(parsePlaybackState({ quality: "huge" })).toEqual({});
    for (const source of [3, "med_x", { media: "med_x" }, { media: "m", duration: 1, fps: {} }]) {
      expect(parsePlaybackState({ source })).toEqual({});
    }
  });

  it("types playback.state.source and keeps the timeline's playhead while a source plays", () => {
    let now = 1;
    const store = createPlaybackStore(() => now);
    store.getState().applyState({ state: "stopped", playing: false, t: 705_600_000, source: null });
    expect(store.getState()).toMatchObject({ playhead: 705_600_000, source: null, sampledAt: 1 });
    // `view.set_source{media}` answers with the source's state (ADR-0022).
    now = 2;
    const source = { media: "med_01", duration: 7_056_000_000, fps: { num: 25, den: 1 } };
    store.getState().applyState({ state: "stopped", playing: false, rate: 0, t: 0, source });
    expect(store.getState()).toMatchObject({ source, sourcePlayhead: 0, playhead: 705_600_000 });
    // Its playback.state events move the source's playhead, never the timeline's.
    store
      .getState()
      .applyState({ state: "playing", playing: true, rate: 1, t: 28_224_000, source });
    store.getState().applyState({ t: 56_448_000 });
    expect(store.getState()).toMatchObject({
      mode: "playing",
      playhead: 705_600_000,
      sourcePlayhead: 56_448_000,
      sampledAt: 1,
    });
    // Back on the timeline (`view.set_source{}`, or the asset was removed).
    now = 3;
    store.getState().applyState({ state: "stopped", playing: false, t: 705_600_000, source: null });
    expect(store.getState()).toMatchObject({ source: null, playhead: 705_600_000, sampledAt: 3 });
  });
});

describe("jobStore", () => {
  const job = (id: string, state: JobInfo["state"], fraction: number | null): JobInfo =>
    ({
      id,
      kind: "analysis",
      label: "index",
      media_id: null,
      state,
      progress: { fraction, message: null },
      cancel_requested: false,
    }) as unknown as JobInfo;

  it("keeps the latest state of each job and lists the unfinished ones", () => {
    const store = createJobStore();
    store.getState().applyProgress(job("job_1", "running", 0.2));
    store.getState().applyProgress(job("job_2", "queued", null));
    store.getState().applyProgress(job("job_1", "running", 0.7));
    expect(store.getState().jobs["job_1" as JobId]?.progress.fraction).toBe(0.7);
    store.getState().applyProgress(job("job_2", "succeeded", 1));
    expect(selectActiveJobs(store.getState()).map((j) => j.id)).toEqual(["job_1"]);
    expect(isActive(job("x", "failed", null))).toBe(false);
  });

  it("reset replaces everything", () => {
    const store = createJobStore();
    store.getState().applyProgress(job("job_1", "running", 0.2));
    store.getState().reset([job("job_9", "queued", null)]);
    expect(Object.keys(store.getState().jobs)).toEqual(["job_9"]);
  });
});
