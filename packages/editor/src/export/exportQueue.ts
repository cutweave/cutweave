// SPDX-License-Identifier: GPL-3.0-or-later
// The export queue as the queue panel shows it (WP-2G.04 over WP-2G.03a, ADR-0030): the list
// `export.queue` answers, kept fresh when an export job starts, ends or appears (the engine's
// `job.progress` events), and the queue's commands: pause, cancel, retry, move, remove. The
// rows read each job's live progress from `jobStore`, so a progress event re-renders its row
// and nothing else; the list is asked again only when a job changes state.

import type { ExportQueueJob, JobId, JobInfo, RpcClient } from "@cutweave/protocol";
import { useStore } from "zustand";
import { createStore, type StoreApi } from "zustand/vanilla";
import type { EngineStoreState } from "../engine/engineStore.ts";
import type { DesktopBridge } from "../session/desktop.ts";
import { asRpcError } from "../session/errors.ts";
import type { SessionStore } from "../session/sessionStore.ts";
import type { JobStore } from "../state/jobStore.ts";
import type { ToastStore } from "../state/toastStore.ts";
import type { Translate } from "../timeline/errors.ts";

export interface QueueState {
  readonly status: "idle" | "loading" | "ready" | "unavailable";
  /** No export starts while paused (a running one finishes). */
  readonly paused: boolean;
  /** First in line first; finished ones stay until removed. */
  readonly jobs: readonly ExportQueueJob[];
}

export interface QueueStore extends QueueState {
  set(state: Partial<QueueState>): void;
}

export function createQueueStore() {
  return createStore<QueueStore>()((set) => ({
    status: "idle",
    paused: false,
    jobs: [],
    set: (state) => set(state),
  }));
}

export const queueStore = createQueueStore();

export function useQueueStore<T>(selector: (state: QueueStore) => T): T {
  return useStore(queueStore, selector);
}

/** Whether a queue job is waiting for its turn (the only state `export.reorder` moves). */
export const isWaiting = (job: Pick<ExportQueueJob, "state">): boolean => job.state === "queued";

/** Whether it is over (done, failed or cancelled): it can be retried or removed. */
export const isFinished = (job: Pick<ExportQueueJob, "state">): boolean =>
  job.state === "succeeded" || job.state === "failed" || job.state === "cancelled";

/**
 * Where `export.reorder` puts a waiting job to move it one place up or down among the waiting
 * ones: the id to put it before, `null` for the end, `undefined` when it cannot move that way.
 */
export function moveTarget(
  jobs: readonly ExportQueueJob[],
  id: JobId,
  direction: "up" | "down",
): JobId | null | undefined {
  const waiting = jobs.filter(isWaiting).map((j) => j.id);
  const at = waiting.indexOf(id);
  if (at < 0) return undefined;
  if (direction === "up") return at === 0 ? undefined : (waiting[at - 1] as JobId);
  if (at === waiting.length - 1) return undefined;
  return (waiting[at + 2] as JobId | undefined) ?? null;
}

/** The state of each export job, to notice a change worth asking the list again for. */
function exportStates(jobs: Readonly<Record<string, JobInfo>>): string {
  return Object.values(jobs)
    .filter((j) => j.kind === "export")
    .map((j) => `${j.id}:${j.state}`)
    .sort()
    .join(",");
}

export interface ExportQueueDeps {
  readonly engine: StoreApi<EngineStoreState>;
  readonly jobs: StoreApi<JobStore>;
  readonly state: StoreApi<QueueStore>;
  readonly toasts: StoreApi<ToastStore>;
  readonly t: Translate;
  readonly desktop: DesktopBridge | null;
  readonly clipboard?: { writeText(text: string): Promise<void> } | undefined;
  /** The open project: its queue is asked for again when another one opens. */
  readonly session?: StoreApi<SessionStore> | undefined;
}

export class ExportQueue {
  readonly #deps: ExportQueueDeps;
  #offs: (() => void)[] = [];
  #seen = "";
  #pending: ReturnType<typeof setTimeout> | null = null;
  #asking = 0;

  constructor(deps: ExportQueueDeps) {
    this.#deps = deps;
  }

  /** Follows the export jobs; returns the disconnect function. */
  attach(): () => void {
    const { jobs, engine } = this.#deps;
    this.#seen = exportStates(jobs.getState().jobs);
    this.#offs.push(
      jobs.subscribe((s) => {
        const now = exportStates(s.jobs);
        if (now === this.#seen) return;
        this.#seen = now;
        this.#soon();
      }),
      engine.subscribe((s, before) => {
        if (s.rpc !== null && s.connection === "ready" && before.connection !== "ready")
          this.#soon();
      }),
    );
    // Another project has another queue (it lives in the project, ADR-0030).
    const session = this.#deps.session;
    if (session !== undefined) {
      this.#offs.push(
        session.subscribe((s, before) => {
          if (s.path !== before.path) this.#soon();
        }),
      );
    }
    return () => this.detach();
  }

  detach(): void {
    for (const off of this.#offs.splice(0)) off();
    if (this.#pending !== null) clearTimeout(this.#pending);
    this.#pending = null;
  }

  /** Asks the list again shortly (several events in a row make one call). */
  #soon(): void {
    if (this.#pending !== null) return;
    this.#pending = setTimeout(() => {
      this.#pending = null;
      void this.refresh();
    }, 50);
  }

  #rpc(): RpcClient | null {
    const { rpc, blocked } = this.#deps.engine.getState();
    return rpc === null || blocked !== null ? null : rpc;
  }

  /** Asks the engine for the queue. */
  async refresh(): Promise<void> {
    const { state } = this.#deps;
    const rpc = this.#rpc();
    if (rpc === null) return;
    const ticket = ++this.#asking;
    if (state.getState().status === "idle") state.getState().set({ status: "loading" });
    try {
      const out = await rpc.call("export.queue", {});
      if (ticket === this.#asking) {
        state.getState().set({ status: "ready", paused: out.paused, jobs: out.jobs });
      }
    } catch {
      if (ticket === this.#asking && state.getState().status !== "ready") {
        state.getState().set({ status: "unavailable" });
      }
    }
  }

  /** Runs a queue command and takes the queue it answers (or asks for it again). */
  async #run(
    call: (rpc: RpcClient) => Promise<{ paused: boolean; jobs: ExportQueueJob[] } | unknown>,
  ): Promise<void> {
    const rpc = this.#rpc();
    if (rpc === null) return;
    try {
      const out = await call(rpc);
      if (typeof out === "object" && out !== null && "jobs" in out && "paused" in out) {
        const queue = out as { paused: boolean; jobs: ExportQueueJob[] };
        this.#asking++;
        this.#deps.state
          .getState()
          .set({ status: "ready", paused: queue.paused, jobs: queue.jobs });
      } else {
        await this.refresh();
      }
    } catch (error) {
      this.#deps.toasts.getState().push({
        tone: "danger",
        title: this.#deps.t("exportQueue.failed"),
        message: asRpcError(error).message,
      });
      await this.refresh();
    }
  }

  setPaused(paused: boolean): Promise<void> {
    return this.#run((rpc) => rpc.call("export.pause_queue", { paused }));
  }

  cancel(id: JobId): Promise<void> {
    return this.#run((rpc) => rpc.call("export.cancel", { id }));
  }

  retry(id: JobId): Promise<void> {
    return this.#run((rpc) => rpc.call("export.retry", { id }));
  }

  remove(id: JobId): Promise<void> {
    return this.#run((rpc) => rpc.call("export.remove", { id }));
  }

  /** Puts a waiting export before another waiting one, or at the end (`null`). */
  reorder(id: JobId, before: JobId | null): Promise<void> {
    return this.#run((rpc) =>
      rpc.call("export.reorder", before === null ? { id } : { id, before }),
    );
  }

  /** Moves a waiting export one place up or down; does nothing at the end of the line. */
  async move(id: JobId, direction: "up" | "down"): Promise<void> {
    const before = moveTarget(this.#deps.state.getState().jobs, id, direction);
    if (before === undefined) return;
    await this.reorder(id, before);
  }

  /** Takes every finished export off the list (their files stay). */
  async clearFinished(): Promise<void> {
    for (const job of this.#deps.state.getState().jobs.filter(isFinished)) {
      await this.remove(job.id);
    }
  }

  /** Shows the file in the OS file manager. */
  reveal(path: string): void {
    void this.#deps.desktop?.shell?.showItemInFolder(path).catch(() => undefined);
  }

  canReveal(): boolean {
    return this.#deps.desktop?.shell?.showItemInFolder !== undefined;
  }

  /** Copies a failed export's reason. */
  async copyError(text: string): Promise<void> {
    await this.#deps.clipboard?.writeText(text).catch(() => undefined);
  }
}
