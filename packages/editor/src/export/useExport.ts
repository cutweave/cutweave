// SPDX-License-Identifier: GPL-3.0-or-later
// Binds `ExportController` and the export queue to the app (WP-1M.08, WP-2G.04): one controller
// and one queue on the global stores, the wiring the shell mounts once, and the "can we export
// now" the menu and the top bar share.
import { useEffect } from "react";
import { engineStore } from "../engine/engineStore.ts";
import { i18n } from "../i18n/index.ts";
import { preferences } from "../prefs/prefsStore.ts";
import { desktopFromWindow } from "../session/desktop.ts";
import { sessionStore } from "../session/sessionStore.ts";
import { docStore, useDocStore } from "../state/docStore.ts";
import { jobStore } from "../state/jobStore.ts";
import { toastStore } from "../state/toastStore.ts";
import { translator } from "../timeline/errors.ts";
import { openPanel } from "../workspace/dockHost.ts";
import { ExportController, exportSource } from "./exportController.ts";
import { ExportQueue, queueStore } from "./exportQueue.ts";
import { exportStore } from "./exportStore.ts";

/** The dock id of the export queue panel. */
export const QUEUE_PANEL = "exports";

const t = translator((key: string, vars?: object) => i18n.t(key as never, vars as never));

let shared: ExportController | null = null;
let sharedQueue: ExportQueue | null = null;

/** The app's export controller, created on first use over the global stores. */
export function exportController(): ExportController {
  shared ??= new ExportController({
    engine: engineStore,
    project: () => docStore.getState().project,
    session: sessionStore,
    jobs: jobStore,
    state: exportStore,
    toasts: toastStore,
    t,
    desktop: desktopFromWindow(),
    prefs: preferences,
    showQueue: () => openPanel(QUEUE_PANEL),
  });
  return shared;
}

/** The app's export queue, created on first use over the global stores. */
export function exportQueue(): ExportQueue {
  sharedQueue ??= new ExportQueue({
    engine: engineStore,
    jobs: jobStore,
    state: queueStore,
    toasts: toastStore,
    t,
    desktop: desktopFromWindow(),
    clipboard: typeof navigator === "undefined" ? undefined : navigator.clipboard,
    session: sessionStore,
  });
  return sharedQueue;
}

/** Forgets the shared controller and queue so the next use builds them again (tests). */
export function resetExportController(): void {
  shared?.detach();
  shared = null;
  sharedQueue?.detach();
  sharedQueue = null;
}

/** Follows export jobs, the queue and the native Export… command for the life of the shell. */
export function useExportWiring(): void {
  useEffect(() => {
    const offController = exportController().attach();
    const offQueue = exportQueue().attach();
    return () => {
      offController();
      offQueue();
    };
  }, []);
}

/** Whether the timeline has anything to export. */
export function useTimelineHasItems(): boolean {
  return useDocStore((s) => exportSource(s.project)?.hasItems === true);
}
