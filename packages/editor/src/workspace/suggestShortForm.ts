// SPDX-License-Identifier: GPL-3.0-or-later
// "Selecting a sequence with 9:16 aspect suggests (not forces) Short-form once per project"
// (WP-2H.01). When the active sequence is taller than it is wide and the Long-form workspace is
// showing, a toast offers Short-form. It never switches by itself, and each project is asked
// once: the answer (a switch, or ignoring the toast) is remembered by project path.
import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import { sessionStore } from "../session/sessionStore.ts";
import { docStore } from "../state/docStore.ts";
import { selectSequenceSummary } from "../state/selectors.ts";
import { toastStore } from "../state/toastStore.ts";
import { viewStore } from "../state/viewStore.ts";

export const SUGGESTED_STORAGE_KEY = "cutweave.workspace.suggested";
const REMEMBER_AT_MOST = 100;
const UNTITLED = "untitled";

/** Whether a canvas is vertical (9:16, 4:5 and so on, anything taller than wide). */
export function isVertical(width: number, height: number): boolean {
  return width > 0 && height > width;
}

function read(storage: Pick<Storage, "getItem">): string[] {
  try {
    const parsed: unknown = JSON.parse(storage.getItem(SUGGESTED_STORAGE_KEY) ?? "[]");
    return Array.isArray(parsed) ? parsed.filter((p): p is string => typeof p === "string") : [];
  } catch {
    return [];
  }
}

/** A storage that works, or `null` (private windows, tests without one). */
function storage(): Storage | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

const askedThisRun = new Set<string>();

/** Whether this project was already offered Short-form (this run or an earlier one). */
export function wasSuggested(
  projectKey: string,
  store: Pick<Storage, "getItem"> | null = storage(),
) {
  return askedThisRun.has(projectKey) || (store !== null && read(store).includes(projectKey));
}

function remember(projectKey: string): void {
  askedThisRun.add(projectKey);
  // An untitled project has no identity to remember across launches.
  const store = projectKey === UNTITLED ? null : storage();
  if (store === null) return;
  try {
    const next = [...read(store).filter((p) => p !== projectKey), projectKey].slice(
      -REMEMBER_AT_MOST,
    );
    store.setItem(SUGGESTED_STORAGE_KEY, JSON.stringify(next));
  } catch {
    // Not remembered across launches; this run still asks once.
  }
}

/** Test seam. */
export function forgetSuggestions(): void {
  askedThisRun.clear();
  try {
    storage()?.removeItem(SUGGESTED_STORAGE_KEY);
  } catch {
    // nothing stored
  }
}

/**
 * Offers Short-form when the open project's sequence is vertical and Long-form is showing.
 * Returns whether a toast was shown. `projectKey` is the project's path, or `"untitled"`.
 */
export function suggestShortForm(
  projectKey: string,
  text: { title: string; message: string; action: string },
): boolean {
  const summary = selectSequenceSummary(docStore.getState().project);
  if (summary === null || !isVertical(summary.width, summary.height)) return false;
  if (viewStore.getState().workspaceId !== "long-form") return false;
  if (wasSuggested(projectKey)) return false;
  remember(projectKey);
  toastStore.getState().push({
    tone: "info",
    title: text.title,
    message: text.message,
    actions: [
      { label: text.action, onClick: () => viewStore.getState().switchWorkspace("short-form") },
    ],
  });
  return true;
}

/** Watches the document and the workspace; mount once in the shell. */
export function useSuggestShortForm(): void {
  const { t } = useTranslation();
  useEffect(() => {
    const run = (): void => {
      suggestShortForm(sessionStore.getState().path ?? UNTITLED, {
        title: t("workspace.suggest.title"),
        message: t("workspace.suggest.body"),
        action: t("workspace.suggest.action"),
      });
    };
    run();
    const offDoc = docStore.subscribe(run);
    const offView = viewStore.subscribe((state, previous) => {
      if (state.workspaceId !== previous.workspaceId) run();
    });
    return () => {
      offDoc();
      offView();
    };
  }, [t]);
}
