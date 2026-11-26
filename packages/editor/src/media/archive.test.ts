// SPDX-License-Identifier: GPL-3.0-or-later
// WP-2F.04b: the "Archive project…" dialog's state. It names a folder or a zip file, asks the
// engine what the archive takes, starts it as a job and follows it; a changed form drops a stale
// estimate.
import type { CollectEstimate, JobId, RpcClient } from "@cutweave/protocol";
import { RpcError } from "@cutweave/protocol";
import { describe, expect, it, vi } from "vitest";
import { createStore } from "zustand/vanilla";
import type { EngineStoreState } from "../engine/engineStore.ts";
import {
  archiveDestOf,
  cancelArchive,
  createArchiveStore,
  estimateArchive,
  startArchive,
} from "./archive.ts";

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
    if (name === "project.archive")
      return { job_id: "job_1" as JobId, estimate: estimateOf(args.dest ?? "") };
    if (name === "job.cancel") return {};
    throw new Error(`unexpected ${name}`);
  });
  const engine = createStore<EngineStoreState>()(
    () => ({ rpc: { call } as unknown as RpcClient, connection: "ready" }) as EngineStoreState,
  );
  const state = createArchiveStore();
  return { call, engine, state, deps: { engine, state } };
}

describe("the archive form", () => {
  it("names a zip file or a folder from the location, the name and the format", () => {
    const form = { parent: "C:\\Vault\\", name: " Promo archive ", trimUnused: false };
    expect(archiveDestOf({ ...form, format: "zip" })).toBe("C:\\Vault\\Promo archive.zip");
    expect(archiveDestOf({ ...form, format: "folder" })).toBe("C:\\Vault\\Promo archive");
    expect(archiveDestOf({ ...form, parent: "", format: "zip" })).toBeNull();
    expect(archiveDestOf({ ...form, name: "a/b", format: "zip" })).toBeNull();
  });

  it("opens with the proposed name and keeps the last location and format", () => {
    const s = setup();
    expect(s.state.getState().format).toBe("zip");
    s.state.getState().edit({ parent: "/vault", format: "folder" });
    s.state.getState().set({ jobId: "job_9" as JobId, error: "old" });
    s.state.getState().show("Promo archive");
    expect(s.state.getState()).toMatchObject({
      open: true,
      name: "Promo archive",
      parent: "/vault",
      format: "folder",
      jobId: null,
      error: null,
      estimate: null,
    });
  });
});

describe("archiving", () => {
  it("asks for the estimate of the place the form names", async () => {
    const s = setup();
    s.state.getState().show("Promo archive");
    await estimateArchive(s.deps);
    expect(s.call).not.toHaveBeenCalled();
    s.state.getState().edit({ parent: "/vault", trimUnused: true });
    await estimateArchive(s.deps);
    expect(s.call).toHaveBeenCalledWith("project.collect_estimate", {
      dest: "/vault/Promo archive.zip",
      mode: null,
      trim_unused: true,
      include_generated: null,
    });
    expect(s.state.getState().estimate?.files).toBe(3);
  });

  it("drops an estimate that arrives after the format changed", async () => {
    const s = setup();
    s.state.getState().show("Promo archive");
    s.state.getState().edit({ parent: "/vault" });
    const pending = estimateArchive(s.deps);
    s.state.getState().edit({ format: "folder" });
    await pending;
    expect(s.state.getState().estimate).toBeNull();
  });

  it("shows why the engine refuses a place, until the form changes", async () => {
    const s = setup();
    s.state.getState().show("Promo archive");
    s.state.getState().edit({ parent: "/vault" });
    s.call.mockRejectedValueOnce(new RpcError("invalid", "a file with that name is already there"));
    await estimateArchive(s.deps);
    expect(s.state.getState().error).toBe("a file with that name is already there");
    s.state.getState().edit({ name: "Another" });
    expect(s.state.getState().error).toBeNull();
  });

  it("starts the archive in the chosen format and remembers what it makes", async () => {
    const s = setup();
    s.state.getState().show("Promo archive");
    s.state.getState().edit({ parent: "/vault" });
    expect(await startArchive(s.deps)).toBe(true);
    expect(s.call).toHaveBeenCalledWith("project.archive", {
      dest: "/vault/Promo archive.zip",
      format: "zip",
      trim_unused: false,
      include_generated: null,
    });
    expect(s.state.getState()).toMatchObject({
      jobId: "job_1",
      dest: "/vault/Promo archive.zip",
      madeFormat: "zip",
    });
    // Changing the form afterwards does not change what the running job makes.
    s.state.getState().edit({ format: "folder" });
    expect(s.state.getState().madeFormat).toBe("zip");
    await cancelArchive(s.deps);
    expect(s.call).toHaveBeenLastCalledWith("job.cancel", { id: "job_1" });
  });

  it("keeps the form and says why when the start is refused", async () => {
    const s = setup();
    s.state.getState().show("Promo archive");
    s.state.getState().edit({ parent: "/vault", format: "folder" });
    s.call.mockRejectedValueOnce(new RpcError("invalid", "not enough space"));
    expect(await startArchive(s.deps)).toBe(false);
    expect(s.state.getState()).toMatchObject({ jobId: null, error: "not enough space" });
  });

  it("does nothing while the form names no place", async () => {
    const s = setup();
    s.state.getState().show("Promo archive");
    expect(await startArchive(s.deps)).toBe(false);
    await cancelArchive(s.deps);
    expect(s.call).not.toHaveBeenCalled();
  });
});
