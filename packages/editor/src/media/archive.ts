// SPDX-License-Identifier: GPL-3.0-or-later
// Archive project (WP-2F.04b): the state and actions of the "Archive project…" dialog. The dialog
// asks for a name, a location and a format (a folder, or one zip file), shows what
// `project.collect_estimate` says for that place (files, size, free space, missing media) and
// starts `project.archive`: a background job whose progress comes from `jobStore`. Both are
// catalog commands, so MCP and the CLI make the same calls. The open project is never changed,
// so nothing here is an undo step.
import { type ArchiveFormat, type CollectEstimate, type JobId, RpcError } from "@cutweave/protocol";
import type { StoreApi } from "zustand";
import { createStore } from "zustand/vanilla";
import type { EngineStoreState } from "../engine/engineStore.ts";
import { destOf } from "./collect.ts";

export interface ArchiveForm {
  /** The folder the archive goes into. */
  readonly parent: string;
  /** The name of the archive: its folder, or its zip file without `.zip`. */
  readonly name: string;
  /** A project folder with its file list, or one zip file holding it. */
  readonly format: ArchiveFormat;
  /** Leave out the media no timeline uses. */
  readonly trimUnused: boolean;
}

export interface ArchiveState extends ArchiveForm {
  readonly open: boolean;
  /** What the engine says the archive takes; `null` until asked (or while the form is incomplete). */
  readonly estimate: CollectEstimate | null;
  /** Why the estimate or the start was refused (the engine's message). */
  readonly error: string | null;
  /** The archive job, once started. */
  readonly jobId: JobId | null;
  /** The folder or zip file of the running or finished archive. */
  readonly dest: string | null;
  /** The format of the running or finished archive (the form may change after it). */
  readonly madeFormat: ArchiveFormat | null;
  show(name: string): void;
  close(): void;
  edit(change: Partial<ArchiveForm>): void;
  set(
    change: Partial<Pick<ArchiveState, "estimate" | "error" | "jobId" | "dest" | "madeFormat">>,
  ): void;
}

export function createArchiveStore(): StoreApi<ArchiveState> {
  return createStore<ArchiveState>()((set) => ({
    open: false,
    parent: "",
    name: "",
    format: "zip",
    trimUnused: false,
    estimate: null,
    error: null,
    jobId: null,
    dest: null,
    madeFormat: null,
    // The location and the format are kept from the last time.
    show: (name) => set({ open: true, name, estimate: null, error: null, jobId: null, dest: null }),
    close: () => set({ open: false }),
    // A changed form makes the last estimate stale.
    edit: (change) => set({ ...change, estimate: null, error: null }),
    set: (change) => set(change),
  }));
}

export const archiveStore = createArchiveStore();

/** The folder or the zip file the archive is, or `null` while the form does not name one. */
export function archiveDestOf(form: ArchiveForm): string | null {
  const folder = destOf(form);
  if (folder === null) return null;
  return form.format === "zip" ? `${folder}.zip` : folder;
}

export interface ArchiveDeps {
  readonly engine: StoreApi<EngineStoreState>;
  readonly state?: StoreApi<ArchiveState>;
}

const messageOf = (error: unknown): string =>
  error instanceof RpcError ? error.message : String(error);

/**
 * Asks the engine what the archive takes and stores the answer (or why it cannot be made). The
 * question is the collect estimate for the archive's own path: the files and the space are the
 * same (a zip stores media as they are). An answer for a form that changed meanwhile is dropped.
 */
export async function estimateArchive(deps: ArchiveDeps): Promise<void> {
  const state = deps.state ?? archiveStore;
  const form = state.getState();
  const dest = archiveDestOf(form);
  const rpc = deps.engine.getState().rpc;
  if (dest === null || rpc === null) return;
  const current = () =>
    archiveDestOf(state.getState()) === dest && state.getState().trimUnused === form.trimUnused;
  try {
    const estimate = await rpc.call("project.collect_estimate", {
      dest,
      mode: null,
      trim_unused: form.trimUnused,
      include_generated: null,
    });
    if (current()) state.getState().set({ estimate, error: null });
  } catch (error) {
    if (current()) state.getState().set({ estimate: null, error: messageOf(error) });
  }
}

/** Starts the archive (`project.archive`); the dialog then follows its job. */
export async function startArchive(deps: ArchiveDeps): Promise<boolean> {
  const state = deps.state ?? archiveStore;
  const form = state.getState();
  const dest = archiveDestOf(form);
  const rpc = deps.engine.getState().rpc;
  if (dest === null || rpc === null) return false;
  try {
    const out = await rpc.call("project.archive", {
      dest,
      format: form.format,
      trim_unused: form.trimUnused,
      include_generated: null,
    });
    state.getState().set({
      jobId: out.job_id,
      dest,
      madeFormat: form.format,
      estimate: out.estimate,
      error: null,
    });
    return true;
  } catch (error) {
    state.getState().set({ error: messageOf(error) });
    return false;
  }
}

/** Stops the running archive. A zip leaves no file; a folder keeps what was copied. */
export async function cancelArchive(deps: ArchiveDeps): Promise<void> {
  const state = deps.state ?? archiveStore;
  const jobId = state.getState().jobId;
  const rpc = deps.engine.getState().rpc;
  if (jobId === null || rpc === null) return;
  await rpc.call("job.cancel", { id: jobId }).catch(() => undefined);
}
