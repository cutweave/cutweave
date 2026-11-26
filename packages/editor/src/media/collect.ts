// SPDX-License-Identifier: GPL-3.0-or-later
// Collect files (WP-2F.04): the state and actions of the "Collect files…" dialog. The dialog asks
// for a folder and a name, shows what `project.collect_estimate` says (files, size, free space,
// missing media) and starts `project.collect`: a background job whose progress comes from
// `jobStore`. Both are catalog commands, so MCP and the CLI make the same calls. The open project
// is never changed, so nothing here is an undo step.
import {
  type CollectEstimate,
  type JobId,
  type JobInfo,
  RpcError,
} from "@cutweave/protocol";
import type { StoreApi } from "zustand";
import { createStore } from "zustand/vanilla";
import type { EngineStoreState } from "../engine/engineStore.ts";

export interface CollectForm {
  /** The folder the copy's folder goes into. */
  readonly parent: string;
  /** The name of the copy's folder (and of its project file). */
  readonly name: string;
  /** Leave out the media no timeline uses. */
  readonly trimUnused: boolean;
}

export interface CollectState extends CollectForm {
  readonly open: boolean;
  /** What the engine says the copy takes; `null` until asked (or while the form is incomplete). */
  readonly estimate: CollectEstimate | null;
  /** Why the estimate or the start was refused (the engine's message). */
  readonly error: string | null;
  /** The copy job, once started. */
  readonly jobId: JobId | null;
  /** The folder the running or finished copy goes into. */
  readonly dest: string | null;
  show(name: string): void;
  close(): void;
  edit(change: Partial<CollectForm>): void;
  set(change: Partial<Pick<CollectState, "estimate" | "error" | "jobId" | "dest">>): void;
}

export function createCollectStore(): StoreApi<CollectState> {
  return createStore<CollectState>()((set) => ({
    open: false,
    parent: "",
    name: "",
    trimUnused: false,
    estimate: null,
    error: null,
    jobId: null,
    dest: null,
    show: (name) =>
      set((s) => ({
        open: true,
        name,
        parent: s.parent,
        estimate: null,
        error: null,
        jobId: null,
        dest: null,
      })),
    close: () => set({ open: false }),
    // A changed form makes the last estimate stale.
    edit: (change) => set({ ...change, estimate: null, error: null }),
    set: (change) => set(change),
  }));
}

export const collectStore = createCollectStore();

/** Characters a folder name may not have (Windows is the strictest). */
const BAD_NAME = /[<>:"/\\|?*]/;

/** Whether `name` can be a folder name. Control characters are refused too. */
export function validFolderName(name: string): boolean {
  const trimmed = name.trim();
  if (trimmed === "" || trimmed.length > 255 || BAD_NAME.test(trimmed)) return false;
  if (trimmed.endsWith(".") || trimmed === "..") return false;
  return ![...trimmed].some((c) => c.charCodeAt(0) < 32);
}

/** The folder the copy goes into, or `null` while the form does not name one. */
export function destOf(form: CollectForm): string | null {
  const parent = form.parent.trim();
  if (parent === "" || !validFolderName(form.name)) return null;
  const sep = parent.includes("\\") && !parent.includes("/") ? "\\" : "/";
  return `${parent.replace(/[\\/]+$/, "")}${sep}${form.name.trim()}`;
}

export interface CollectDeps {
  readonly engine: StoreApi<EngineStoreState>;
  readonly state?: StoreApi<CollectState>;
}

const messageOf = (error: unknown): string =>
  error instanceof RpcError ? error.message : String(error);

const paramsOf = (dest: string, form: CollectForm) => ({
  dest,
  mode: null,
  trim_unused: form.trimUnused,
  include_generated: null,
});

/**
 * Asks the engine what the copy takes and stores the answer (or why it cannot be made). An answer
 * for a form that changed meanwhile is dropped.
 */
export async function estimateCollect(deps: CollectDeps): Promise<void> {
  const state = deps.state ?? collectStore;
  const form = state.getState();
  const dest = destOf(form);
  const rpc = deps.engine.getState().rpc;
  if (dest === null || rpc === null) return;
  const current = () => destOf(state.getState()) === dest && state.getState().trimUnused === form.trimUnused;
  try {
    const estimate = await rpc.call("project.collect_estimate", paramsOf(dest, form));
    if (current()) state.getState().set({ estimate, error: null });
  } catch (error) {
    if (current()) state.getState().set({ estimate: null, error: messageOf(error) });
  }
}

/** Starts the copy (`project.collect`); the dialog then follows its job. */
export async function startCollect(deps: CollectDeps): Promise<boolean> {
  const state = deps.state ?? collectStore;
  const form = state.getState();
  const dest = destOf(form);
  const rpc = deps.engine.getState().rpc;
  if (dest === null || rpc === null) return false;
  try {
    const out = await rpc.call("project.collect", paramsOf(dest, form));
    state.getState().set({ jobId: out.job_id, dest, estimate: out.estimate, error: null });
    return true;
  } catch (error) {
    state.getState().set({ error: messageOf(error) });
    return false;
  }
}

/** Stops the running copy; calling Collect again later continues it. */
export async function cancelCollect(deps: CollectDeps): Promise<void> {
  const state = deps.state ?? collectStore;
  const jobId = state.getState().jobId;
  const rpc = deps.engine.getState().rpc;
  if (jobId === null || rpc === null) return;
  await rpc.call("job.cancel", { id: jobId }).catch(() => undefined);
}

/** Where the copy stands, from its job. */
export type CollectPhase = "form" | "running" | "done" | "failed" | "cancelled";

export function phaseOf(jobId: JobId | null, job: JobInfo | undefined): CollectPhase {
  if (jobId === null) return "form";
  switch (job?.state) {
    case "succeeded":
      return "done";
    case "failed":
      return "failed";
    case "cancelled":
      return "cancelled";
    default:
      return "running";
  }
}
