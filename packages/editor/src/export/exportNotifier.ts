// SPDX-License-Identifier: GPL-3.0-or-later
// Says when an export is over (WP-2G.04b, Settings > Notifications): every export job this
// window watched while it was waiting or running and that then ends done or failed is announced.
// The desktop app's main process shows an OS notification when the person wants one
// (`notify.export_done`) and Cutweave is in the background; the editor plays a short sound when
// `notify.sound` is on. A cancelled export is the person's own doing and is not announced, and
// neither is an export that was already over when this window first heard of it.

import type { JobId, JobInfo } from "@cutweave/protocol";
import type { StoreApi } from "zustand";
import type { PreferencesClient } from "../prefs/prefsStore.ts";
import type { DesktopBridge } from "../session/desktop.ts";
import { isActive, type JobStore } from "../state/jobStore.ts";
import type { Translate } from "../timeline/errors.ts";
import type { QueueStore } from "./exportQueue.ts";
import { baseName } from "./exportSettings.ts";
import type { ExportStore } from "./exportStore.ts";

/** The longest title and body main takes (apps/desktop `MAX_NOTICE_TITLE`, `MAX_NOTICE_BODY`). */
export const MAX_NOTICE_TITLE = 120;
export const MAX_NOTICE_BODY = 400;

export interface ExportNotifierDeps {
  readonly jobs: StoreApi<JobStore>;
  /** Where a job's file is read from: the queue's list, else the export this window started. */
  readonly queue: StoreApi<QueueStore>;
  readonly state: StoreApi<ExportStore>;
  readonly t: Translate;
  readonly desktop: DesktopBridge | null;
  readonly prefs: Pick<PreferencesClient, "get">;
  /** Plays the sound of a finished export. */
  readonly chime: () => void;
}

const clip = (text: string, max: number): string =>
  text.length <= max ? text : `${text.slice(0, max - 1)}…`;

export class ExportNotifier {
  readonly #deps: ExportNotifierDeps;
  /** The export jobs seen waiting or running. */
  readonly #open = new Set<JobId>();
  #off: (() => void) | null = null;

  constructor(deps: ExportNotifierDeps) {
    this.#deps = deps;
  }

  /** Follows the export jobs; returns the disconnect function. */
  attach(): () => void {
    this.#look(this.#deps.jobs.getState().jobs);
    this.#off = this.#deps.jobs.subscribe((s) => this.#look(s.jobs));
    return () => this.detach();
  }

  detach(): void {
    this.#off?.();
    this.#off = null;
    this.#open.clear();
  }

  #look(jobs: Readonly<Record<string, JobInfo>>): void {
    for (const info of Object.values(jobs)) {
      if (info.kind !== "export") continue;
      if (isActive(info)) {
        this.#open.add(info.id);
      } else if (this.#open.delete(info.id) && info.state !== "cancelled") {
        this.#announce(info);
      }
    }
    // A job the engine no longer lists (it restarted) is not one that finished.
    for (const id of this.#open) if (jobs[id] === undefined) this.#open.delete(id);
  }

  /** The job's file name, as far as this window knows it. */
  #fileName(id: JobId): string {
    const path =
      this.#deps.queue.getState().jobs.find((j) => j.id === id)?.path ??
      this.#deps.state.getState().active?.files.find((f) => f.jobId === id)?.path;
    return path === undefined ? "" : baseName(path);
  }

  #announce(info: JobInfo): void {
    const { t, desktop, prefs, chime } = this.#deps;
    if (prefs.get("notify.sound")) chime();
    const send = desktop?.notify?.exportFinished;
    if (send === undefined) return;
    const name = this.#fileName(info.id);
    const failed = info.state === "failed";
    const reason = info.error ?? t("export.failed.unknown");
    const body = !failed
      ? name
      : name === ""
        ? reason
        : t("export.notify.failed", { name, message: reason });
    void send({
      title: clip(t(failed ? "export.failed.title" : "export.done.title"), MAX_NOTICE_TITLE),
      body: clip(body, MAX_NOTICE_BODY),
    }).catch(() => undefined);
  }
}
