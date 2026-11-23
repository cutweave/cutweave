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
  it("starts empty and exposes only applyEvent and resync as actions", () => {
    const store = createDocStore();
    const s = store.getState();
    expect(s.status).toBe("empty");
    expect(s.project).toBeNull();
    const actions = Object.keys(s).filter((k) => typeof s[k as keyof typeof s] === "function");
    expect(actions.sort()).toEqual(["applyEvent", "resync"]);
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

  it("ignores unusable payloads", () => {
    const store = createPlaybackStore();
    const before = store.getState();
    for (const bad of [null, 3, "x", { state: "flying", rate: "fast", playhead: -1 }]) {
      store.getState().applyState(bad);
    }
    expect(store.getState()).toBe(before);
    expect(parsePlaybackState({ quality: "huge" })).toEqual({});
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
