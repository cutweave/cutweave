// SPDX-License-Identifier: GPL-3.0-or-later
// Binds `ExportController` to the app (WP-1M.08): one controller on the global stores, the
// wiring the shell mounts once, and the "can we export now" the menu and the top bar share.
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
import { ExportController, exportSource } from "./exportController.ts";
import { exportStore } from "./exportStore.ts";

let shared: ExportController | null = null;

/** The app's export controller, created on first use over the global stores. */
export function exportController(): ExportController {
  shared ??= new ExportController({
    engine: engineStore,
    project: () => docStore.getState().project,
    session: sessionStore,
    jobs: jobStore,
    state: exportStore,
    toasts: toastStore,
    t: translator((key: string, vars?: object) => i18n.t(key as never, vars as never)),
    desktop: desktopFromWindow(),
    prefs: preferences,
  });
  return shared;
}

/** Forgets the shared controller so the next use builds one again (tests that swap the bridge). */
export function resetExportController(): void {
  shared?.detach();
  shared = null;
}

/** Follows export jobs and the native Export… command for the life of the shell. */
export function useExportWiring(): void {
  useEffect(() => exportController().attach(), []);
}

/** Whether the timeline has anything to export. */
export function useTimelineHasItems(): boolean {
  return useDocStore((s) => exportSource(s.project)?.hasItems === true);
}
