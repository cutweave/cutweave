// SPDX-License-Identifier: GPL-3.0-or-later
// Importing files into the project (WP-1M.03): paths from the native dialog or an OS drop become
// one `media.import` (arch §7: partial success, `failed[]` with a code per file, one undo step).
// The engine is the authority on what it can read; this file only refuses what cannot be a path
// (the preload and main validate their side too) and says what happened.
import {
  type FolderId,
  type ImportFailure,
  type ImportFailureCode,
  RpcError,
} from "@cutweave/protocol";
import type { StoreApi } from "zustand";
import type { EngineStoreState } from "../engine/engineStore.ts";
import type { TKey } from "../i18n/index.ts";
import type { ToastStore } from "../state/toastStore.ts";
import type { Translate } from "../timeline/errors.ts";

/** The engine takes 1..=256 paths per call. */
export const MAX_IMPORT_PATHS = 256;
/** The longest path accepted (same bound as the desktop IPC). */
export const MAX_PATH_LENGTH = 4096;

/** Plain strings that could be a file path: non-empty, bounded, no NUL. Order kept, repeats dropped. */
export function validateImportPaths(candidates: readonly unknown[]): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const candidate of candidates) {
    if (
      typeof candidate !== "string" ||
      candidate.length === 0 ||
      candidate.length > MAX_PATH_LENGTH ||
      candidate.includes("\0") ||
      seen.has(candidate)
    ) {
      continue;
    }
    seen.add(candidate);
    out.push(candidate);
  }
  return out;
}

const FAILURE_REASON: Readonly<Record<ImportFailureCode, TKey>> = {
  not_found: "mediaBin.import.reason.notFound",
  unsupported: "mediaBin.import.reason.unsupported",
  timed_out: "mediaBin.import.reason.timedOut",
  io: "mediaBin.import.reason.io",
  invalid: "mediaBin.import.reason.invalid",
};

const baseName = (path: string): string => path.split(/[\\/]/).pop() || path;

/** The toast body for one refused file: the reason and the next step; the engine's text last. */
export function describeImportFailure(failure: ImportFailure, t: Translate): string {
  const key = FAILURE_REASON[failure.code];
  return key === undefined ? failure.error : t(key);
}

export interface ImportDeps {
  readonly engine: StoreApi<EngineStoreState>;
  readonly toasts: StoreApi<ToastStore>;
  readonly t: Translate;
}

export interface ImportSummary {
  /** Assets added to the project. */
  readonly added: number;
  /** Files whose content was in the project already. */
  readonly duplicates: number;
  readonly failed: readonly ImportFailure[];
  /** Bin folders made for the directories of imported folders. */
  readonly folders: number;
  /** What was left out of imported folders (WP-2F.09). */
  readonly skipped: {
    readonly unsupported: number;
    readonly unreadable: number;
    readonly tooDeep: number;
  };
}

export interface ImportOptions {
  /**
   * Whether folders in the paths keep their directories as bin folders (the caller passes the
   * `media.keep_structure` setting). Only `false` is sent; the engine's default is on.
   */
  readonly keepStructure?: boolean;
}

/** "1 file" / "214 files". */
function files(n: number, t: Translate): string {
  return n === 1
    ? t("mediaBin.import.fileOne")
    : t("mediaBin.import.fileMany", { count: String(n) });
}

/** "1 folder" / "12 folders". */
function folders(n: number, t: Translate): string {
  return n === 1
    ? t("mediaBin.import.folderOne")
    : t("mediaBin.import.folderMany", { count: String(n) });
}

/** "9 unsupported, 2 unreadable": what the engine left out of folders; empty when nothing. */
export function describeSkipped(skipped: ImportSummary["skipped"], t: Translate): string {
  const parts: string[] = [];
  if (skipped.unsupported > 0) {
    parts.push(t("mediaBin.import.skippedUnsupported", { count: String(skipped.unsupported) }));
  }
  if (skipped.unreadable > 0) {
    parts.push(t("mediaBin.import.skippedUnreadable", { count: String(skipped.unreadable) }));
  }
  if (skipped.tooDeep > 0) {
    parts.push(t("mediaBin.import.skippedTooDeep", { count: String(skipped.tooDeep) }));
  }
  return parts.join(", ");
}

/**
 * Imports `candidates` (validated first; files or folders) and reports on it with toasts: a
 * progress toast while the engine probes, one `danger` toast per refused file (no card appears
 * for it), one `success` toast with the counts: "Imported 214 files into 12 folders; skipped 9
 * unsupported". Resolves with the summary (into `folder` of the bin, else the top level), or
 * `null` when nothing was sent (no engine, no usable path, or the call itself failed, which
 * shows its own toast).
 */
export async function importMedia(
  deps: ImportDeps,
  candidates: readonly unknown[],
  folder: FolderId | null = null,
  options: ImportOptions = {},
): Promise<ImportSummary | null> {
  const { toasts, t } = deps;
  const paths = validateImportPaths(candidates);
  const rpc = deps.engine.getState().rpc;
  if (rpc === null || paths.length === 0) return null;

  const working =
    paths.length === 1
      ? t("mediaBin.import.workingOne")
      : t("mediaBin.import.workingMany", { count: String(paths.length) });
  const progress = toasts.getState().push({ tone: "info", title: working });
  // The engine says how far it is while it probes a folder's files (`media.import_progress`).
  const stopProgress =
    typeof rpc.on === "function"
      ? rpc.on("media.import_progress", (e) =>
          toasts.getState().update(progress, {
            title: t("mediaBin.import.progress", { done: String(e.done), total: String(e.total) }),
          }),
        )
      : () => {};
  let added = 0;
  let duplicates = 0;
  let madeFolders = 0;
  const skipped = { unsupported: 0, unreadable: 0, tooDeep: 0 };
  const failed: ImportFailure[] = [];
  try {
    for (let i = 0; i < paths.length; i += MAX_IMPORT_PATHS) {
      const out = await rpc.call("media.import", {
        paths: paths.slice(i, i + MAX_IMPORT_PATHS),
        ...(folder === null ? {} : { folder }),
        ...(options.keepStructure === false ? { keep_structure: false } : {}),
      });
      for (const m of out.media) {
        if (m.duplicate) duplicates += 1;
        else added += 1;
      }
      failed.push(...out.failed);
      madeFolders += out.folders ?? 0;
      skipped.unsupported += out.skipped?.unsupported ?? 0;
      skipped.unreadable += out.skipped?.unreadable ?? 0;
      skipped.tooDeep += out.skipped?.too_deep ?? 0;
    }
  } catch (error) {
    stopProgress();
    toasts.getState().dismiss(progress);
    toasts.getState().push({
      tone: "danger",
      title: t("mediaBin.import.callFailed"),
      message: error instanceof RpcError ? error.message : String(error),
    });
    return null;
  }
  stopProgress();
  toasts.getState().dismiss(progress);

  for (const failure of failed) {
    toasts.getState().push({
      tone: "danger",
      title: t("mediaBin.import.failedTitle", { name: baseName(failure.path) }),
      message: describeImportFailure(failure, t),
    });
  }
  const summary: ImportSummary = { added, duplicates, failed, folders: madeFolders, skipped };
  const left = describeSkipped(skipped, t);
  let duplicateNote = "";
  if (duplicates === 1) duplicateNote = t("mediaBin.import.duplicateOne");
  else if (duplicates > 1) {
    duplicateNote = t("mediaBin.import.duplicateMany", { count: String(duplicates) });
  }
  if (added > 0 && (madeFolders > 0 || left !== "")) {
    // From a folder: say where the files went and what was left out.
    let message =
      madeFolders > 0
        ? t("mediaBin.import.summaryInto", {
            files: files(added, t),
            folders: folders(madeFolders, t),
          })
        : t("mediaBin.import.summaryFlat", { files: files(added, t) });
    if (left !== "") message += t("mediaBin.import.summarySkipped", { what: left });
    message += ".";
    if (duplicateNote !== "") message += ` ${duplicateNote}`;
    toasts.getState().push({ tone: "success", title: t("mediaBin.import.doneTitle"), message });
  } else if (added > 0 || duplicates > 0) {
    const parts: string[] = [];
    if (added > 0) {
      parts.push(
        added === 1
          ? t("mediaBin.import.addedOne")
          : t("mediaBin.import.addedMany", { count: String(added) }),
      );
    }
    if (duplicateNote !== "") parts.push(duplicateNote);
    toasts
      .getState()
      .push({ tone: "success", title: t("mediaBin.import.doneTitle"), message: parts.join(" ") });
  } else if (left !== "") {
    toasts.getState().push({
      tone: "info",
      title: t("mediaBin.import.nothingTitle"),
      message: t("mediaBin.import.nothingBody", { what: left }),
    });
  }
  return summary;
}
