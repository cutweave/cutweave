// SPDX-License-Identifier: GPL-3.0-or-later
// WP-2F.04: the "Collect files…" dialog's state. It names a folder, asks the engine what the copy
// takes, starts the copy as a job and follows it; a changed form drops a stale estimate.
import type { CollectEstimate, JobId, JobInfo, RpcClient } from "@cutweave/protocol";
import { RpcError } from "@cutweave/protocol";
import { describe, expect, it, vi } from "vitest";
import { createStore } from "zustand/vanilla";
import type { EngineStoreState } from "../engine/engineStore.ts";
import {
  cancelCollect,
  createCollectStore,
  destOf,
  estimateCollect,
  phaseOf,
  startCollect,
  validFolderName,
} from "./collect.ts";

const estimateOf = (dest: string, more: Partial<CollectEstimate> = {}): CollectEstimate => ({
  dest,
  files: 3,
  bytes: 4096,
  free_bytes: 1_000_000,
  enough_space: true,
  missing: [],
  trimmed: 0,
  ...more,
});

function setup() {
  const call = vi.fn(async (name: string, args: { dest?: string }) => {
    if (name === "project.collect_estimate") return estimateOf(args.dest ?? "");
    if (name === "project.collect")
      return { job_id: "job_1" as JobId, estimate: estimateOf(args.dest ?? "") };
    if (name === "job.cancel") return {};
    throw new Error(`unexpected ${name}`);
  });
  const engine = createStore<EngineStoreState>()(
    () => ({ rpc: { call } as unknown as RpcClient, connection: "ready" }) as EngineStoreState,
  );
  const state = createCollectStore();
  return { call, engine, state, deps: { engine, state } };
}

describe("the collect form", () => {
  it("names the folder from the location and the name, with the location's separator", () => {
    expect(destOf({ parent: "C:\\Work\\", name: " Promo collected ", trimUnused: false })).toBe(
      "C:\\Work\\Promo collected",
    );
    expect(destOf({ parent: "/home/me/work", name: "Promo", trimUnused: false })).toBe(
      "/home/me/work/Promo",
    );
    expect(destOf({ parent: "", name: "Promo", trimUnused: false })).toBeNull();
    expect(destOf({ parent: "/x", name: "a/b", trimUnused: false })).toBeNull();
  });

  it("refuses names a folder cannot have", () => {
    for (const bad of ["", "  ", "a:b", "what?", "dots.", "..", "tab\tname", "x".repeat(256)]) {
      expect(validFolderName(bad), bad).toBe(false);
    }
    expect(validFolderName("Promo collected (2)")).toBe(true);
  });

  it("opens with the proposed name and keeps the last location", () => {
    const s = setup();
    s.state.getState().edit({ parent: "/work" });
    s.state.getState().set({ jobId: "job_9" as JobId, error: "old" });
    s.state.getState().show("Promo collected");
    expect(s.state.getState()).toMatchObject({
      open: true,
      name: "Promo collected",
      parent: "/work",
      jobId: null,
      error: null,
      estimate: null,
    });
  });
});

describe("collecting", () => {
  it("asks for the estimate of the folder the form names", async () => {
    const s = setup();
    s.state.getState().show("Promo collected");
    await estimateCollect(s.deps);
    expect(s.call).not.toHaveBeenCalled();
    s.state.getState().edit({ parent: "/work", trimUnused: true });
    await estimateCollect(s.deps);
    expect(s.call).toHaveBeenCalledWith("project.collect_estimate", {
      dest: "/work/Promo collected",
      mode: null,
      trim_unused: true,
      include_generated: null,
    });
    expect(s.state.getState().estimate?.files).toBe(3);
  });

  it("drops an estimate that arrives after the form changed", async () => {
    const s = setup();
    s.state.getState().show("Promo collected");
    s.state.getState().edit({ parent: "/work" });
    const pending = estimateCollect(s.deps);
    s.state.getState().edit({ parent: "/elsewhere" });
    await pending;
    expect(s.state.getState().estimate).toBeNull();
  });

  it("shows why the engine refuses a folder", async () => {
    const s = setup();
    s.state.getState().show("Promo collected");
    s.state.getState().edit({ parent: "/work" });
    s.call.mockRejectedValueOnce(new RpcError("invalid", "that folder already holds a project"));
    await estimateCollect(s.deps);
    expect(s.state.getState().error).toBe("that folder already holds a project");
    // Editing the form clears it.
    s.state.getState().edit({ name: "Another" });
    expect(s.state.getState().error).toBeNull();
  });

  it("starts the copy as a job and can cancel it", async () => {
    const s = setup();
    s.state.getState().show("Promo collected");
    s.state.getState().edit({ parent: "/work" });
    expect(await startCollect(s.deps)).toBe(true);
    expect(s.state.getState()).toMatchObject({ jobId: "job_1", dest: "/work/Promo collected" });
    await cancelCollect(s.deps);
    expect(s.call).toHaveBeenLastCalledWith("job.cancel", { id: "job_1" });
  });

  it("keeps the form when the start is refused", async () => {
    const s = setup();
    s.state.getState().show("Promo collected");
    s.state.getState().edit({ parent: "/work" });
    s.call.mockRejectedValueOnce(new RpcError("invalid", "not enough space"));
    expect(await startCollect(s.deps)).toBe(false);
    expect(s.state.getState()).toMatchObject({ jobId: null, error: "not enough space" });
  });

  it("reads where the copy stands from its job", () => {
    const job = (state: JobInfo["state"]) => ({ state }) as JobInfo;
    expect(phaseOf(null, undefined)).toBe("form");
    expect(phaseOf("job_1" as JobId, undefined)).toBe("running");
    expect(phaseOf("job_1" as JobId, job("queued"))).toBe("running");
    expect(phaseOf("job_1" as JobId, job("succeeded"))).toBe("done");
    expect(phaseOf("job_1" as JobId, job("failed"))).toBe("failed");
    expect(phaseOf("job_1" as JobId, job("cancelled"))).toBe("cancelled");
  });
});
