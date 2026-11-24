// SPDX-License-Identifier: GPL-3.0-or-later
// The export flow (WP-1M.08, arch §7 "Export"): starts `export.start`, follows the job through
// `jobStore` (the engine's `job.progress` events), cancels it with `job.cancel`, and says how it
// ended with a toast. The dialog and the top bar's chip only read `exportStore` and `jobStore`,
// so the export carries on when the dialog is hidden.
//
// Every engine call goes over the editor's own connection; nothing here touches the document.

import type {
  ExportHw,
  ExportPreset,
  JobInfo,
  Project,
  RpcClient,
  SequenceId,
} from "@cutweave/protocol";
import type { StoreApi } from "zustand";
import type { EngineStoreState } from "../engine/engineStore.ts";
import type { DesktopBridge } from "../session/desktop.ts";
import { asRpcError } from "../session/errors.ts";
import type { SessionStore } from "../session/sessionStore.ts";
import { isActive, type JobStore } from "../state/jobStore.ts";
import type { ToastStore } from "../state/toastStore.ts";
import type { Translate } from "../timeline/errors.ts";
import {
  baseName,
  defaultOutputPath,
  hardwareEncoders,
  type SequenceFacts,
  sequenceFacts,
} from "./exportSettings.ts";
import type { ExportStore } from "./exportStore.ts";

export interface ExportControllerDeps {
  readonly engine: StoreApi<EngineStoreState>;
  readonly project: () => Project | null;
  readonly session: StoreApi<SessionStore>;
  readonly jobs: StoreApi<JobStore>;
  readonly state: StoreApi<ExportStore>;
  readonly toasts: StoreApi<ToastStore>;
  readonly t: Translate;
  readonly desktop: DesktopBridge | null;
}

/** The sequence an export takes (the project's first), with the facts the dialog shows. */
export interface ExportSource {
  readonly sequenceId: SequenceId;
  readonly facts: SequenceFacts;
  readonly hasItems: boolean;
}

/** The first sequence of the project, or `null` when there is none. */
export function exportSource(project: Project | null): ExportSource | null {
  const sequence = Object.values(project?.sequences ?? {})[0];
  if (sequence === undefined) return null;
  const hasItems = Object.values(sequence.tracks).some(
    (track) => Object.keys(track.items).length > 0,
  );
  return { sequenceId: sequence.id, facts: sequenceFacts(sequence), hasItems };
}

/** What the dialog asks the controller to start. */
export interface ExportRequest {
  readonly path: string;
  readonly preset: ExportPreset | null | undefined;
  readonly hw: ExportHw;
  readonly overwrite: boolean;
}

export type StartOutcome =
  | { readonly kind: "started" }
  /** The file is already there: ask before replacing it. */
  | { readonly kind: "exists" }
  | { readonly kind: "refused"; readonly message: string };

/** How long the "Reveal in folder" toast stays. */
export const DONE_TOAST_MS = 12_000;

export class ExportController {
  readonly #deps: ExportControllerDeps;
  #offs: (() => void)[] = [];

  constructor(deps: ExportControllerDeps) {
    this.#deps = deps;
  }

  // ---- wiring -------------------------------------------------------------------------------

  /** Follows the jobs and the native menu's Export… (macOS). Returns the disconnect function. */
  attach(): () => void {
    const { jobs, desktop } = this.#deps;
    this.#offs.push(jobs.subscribe(() => this.#settle()));
    if (desktop?.menu !== undefined) {
      this.#offs.push(
        desktop.menu.onCommand((command) => {
          if (command === "export") this.open();
        }),
      );
    }
    return () => this.detach();
  }

  detach(): void {
    for (const off of this.#offs.splice(0)) off();
  }

  // ---- the dialog ---------------------------------------------------------------------------

  /** Opens the dialog (the progress view while an export runs); does nothing with an empty timeline. */
  open(): void {
    const { state } = this.#deps;
    if (state.getState().active === null && !this.available()) return;
    state.getState().setDialogOpen(true);
    if (state.getState().active === null) void this.loadEncoders();
  }

  close(): void {
    this.#deps.state.getState().setDialogOpen(false);
  }

  /** Whether there is something to export and an engine to do it. */
  available(): boolean {
    return exportSource(this.#deps.project())?.hasItems === true;
  }

  /** Where the file goes unless the person picks another place. */
  defaultPath(): string {
    const { session, project } = this.#deps;
    const named = project()?.meta.name;
    const name = named !== undefined && named !== "" ? named : session.getState().name || "Export";
    return defaultOutputPath(session.getState().path, name);
  }

  /** The native "save as" for the file: the chosen path, or `null` (cancelled or no desktop). */
  async chooseFile(current: string): Promise<string | null> {
    const pick = this.#deps.desktop?.dialog.chooseExportFile;
    if (pick === undefined) return null;
    try {
      return await pick(current === "" ? this.defaultPath() : current);
    } catch {
      return null;
    }
  }

  /**
   * Asks the engine which hardware encoders there are. The first answer pays for the probe
   * (about a second), so it is asked as the dialog opens, before Export is pressed. An engine
   * without the query leaves the encoder unnamed; "auto" still picks one.
   */
  async loadEncoders(): Promise<void> {
    const { state, engine } = this.#deps;
    if (state.getState().probe.status === "loading" || state.getState().probe.status === "ready") {
      return;
    }
    const rpc = engine.getState().rpc;
    if (rpc === null) return;
    state.getState().setProbe({ status: "loading" });
    try {
      const answer = await (rpc as unknown as UntypedRpc).call("media.capabilities", {});
      const encoders = hardwareEncoders(answer);
      state
        .getState()
        .setProbe(encoders === null ? { status: "unavailable" } : { status: "ready", encoders });
    } catch {
      state.getState().setProbe({ status: "unavailable" });
    }
  }

  // ---- running it ---------------------------------------------------------------------------

  /** Starts the export. Resolves with what the dialog should do next. */
  async start(request: ExportRequest): Promise<StartOutcome> {
    const { engine, state, t } = this.#deps;
    const source = exportSource(this.#deps.project());
    if (source === null || !source.hasItems) {
      return { kind: "refused", message: t("export.errors.start", { message: "—" }) };
    }
    const { rpc, blocked } = engine.getState();
    if (rpc === null || blocked !== null) {
      return {
        kind: "refused",
        message: t("export.errors.start", { message: "the engine is not available" }),
      };
    }
    try {
      const out = await rpc.call("export.start", {
        sequence: source.sequenceId,
        path: request.path,
        preset: request.preset ?? null,
        hw: request.hw,
        overwrite: request.overwrite,
      });
      state.getState().setActive({
        jobId: out.job_id,
        path: out.path,
        fileName: baseName(out.path),
        seen: false,
      });
      // The first `job.progress` may have arrived before this answer did.
      this.#settle();
      return { kind: "started" };
    } catch (error) {
      const failure = asRpcError(error);
      if (failure.code === "invalid" && failure.path?.endsWith("/overwrite") === true) {
        return { kind: "exists" };
      }
      return {
        kind: "refused",
        message: t("export.errors.start", { message: failure.message }),
      };
    }
  }

  /** Asks the engine to stop the export; the job ends `cancelled` and no file is left. */
  async cancel(): Promise<void> {
    const { engine, state, jobs } = this.#deps;
    const active = state.getState().active;
    const rpc: RpcClient | null = engine.getState().rpc;
    if (active === null || rpc === null) return;
    state.getState().setCancelling(true);
    try {
      const info = await rpc.call("job.cancel", { id: active.jobId });
      jobs.getState().applyProgress(info);
    } catch (error) {
      state.getState().setCancelling(false);
      this.#deps.toasts.getState().push({
        tone: "danger",
        title: this.#deps.t("export.failed.title"),
        message: asRpcError(error).message,
      });
    }
  }

  // ---- how it ended -------------------------------------------------------------------------

  /** Looks at the job we follow: marks it seen, and reports how it ended. */
  #settle(): void {
    const { state, jobs } = this.#deps;
    const active = state.getState().active;
    if (active === null) return;
    const info = jobs.getState().jobs[active.jobId];
    if (info === undefined) {
      // It was in the list and is gone: the engine restarted and knows nothing of it.
      if (active.seen) this.#lost();
      return;
    }
    if (!active.seen) state.getState().markSeen();
    if (!isActive(info)) this.#finish(active.path, active.fileName, info);
  }

  #end(): void {
    const { state } = this.#deps;
    state.getState().setActive(null);
    state.getState().setDialogOpen(false);
  }

  #finish(path: string, fileName: string, info: JobInfo): void {
    const { toasts, t, desktop } = this.#deps;
    this.#end();
    if (info.state === "succeeded") {
      const reveal = desktop?.shell?.showItemInFolder;
      const detail = info.progress.message;
      toasts.getState().push({
        tone: "success",
        title: t("export.done.title"),
        message: detail === null || detail === "" ? fileName : `${fileName}: ${detail}`,
        duration: DONE_TOAST_MS,
        ...(reveal === undefined
          ? {}
          : {
              actions: [
                {
                  label: t("export.done.reveal"),
                  onClick: () => void reveal(path).catch(() => undefined),
                },
              ],
            }),
      });
    } else if (info.state === "cancelled") {
      toasts.getState().push({
        tone: "info",
        title: t("export.cancelled.title"),
        message: t("export.cancelled.message"),
      });
    } else {
      toasts.getState().push({
        tone: "danger",
        title: t("export.failed.title"),
        message: info.error ?? t("export.failed.unknown"),
      });
    }
  }

  #lost(): void {
    this.#end();
    this.#deps.toasts.getState().push({
      tone: "danger",
      title: this.#deps.t("export.lost.title"),
      message: this.#deps.t("export.lost.message"),
    });
  }
}

/** The client without its schema: for a query the generated map does not know yet. */
interface UntypedRpc {
  call(name: string, input: unknown): Promise<unknown>;
}
