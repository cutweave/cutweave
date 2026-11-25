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
}

/**
 * Imports `candidates` (validated first) and reports on it with toasts: one `danger` toast per
 * refused file (no card appears for it), one `success` toast with the counts. Resolves with the
 * summary (into `folder` of the bin, else the top level), or `null` when nothing was sent (no engine, no usable path, or the call itself
 * failed, which shows its own toast).
 */
export async function importMedia(
  deps: ImportDeps,
  candidates: readonly unknown[],
  folder: FolderId | null = null,
): Promise<ImportSummary | null> {
  const { toasts, t } = deps;
  const paths = validateImportPaths(candidates);
  const rpc = deps.engine.getState().rpc;
  if (rpc === null || paths.length === 0) return null;

  const progress = toasts.getState().push({
    tone: "info",
    title: t(paths.length === 1 ? "mediaBin.import.workingOne" : "mediaBin.import.workingMany", {
      count: String(paths.length),
    }),
  });
  let added = 0;
  let duplicates = 0;
  const failed: ImportFailure[] = [];
  try {
    for (let i = 0; i < paths.length; i += MAX_IMPORT_PATHS) {
      const out = await rpc.call("media.import", {
        paths: paths.slice(i, i + MAX_IMPORT_PATHS),
        ...(folder === null ? {} : { folder }),
      });
      for (const m of out.media) {
        if (m.duplicate) duplicates += 1;
        else added += 1;
      }
      failed.push(...out.failed);
    }
  } catch (error) {
    toasts.getState().dismiss(progress);
    toasts.getState().push({
      tone: "danger",
      title: t("mediaBin.import.callFailed"),
      message: error instanceof RpcError ? error.message : String(error),
    });
    return null;
  }
  toasts.getState().dismiss(progress);

  for (const failure of failed) {
    toasts.getState().push({
      tone: "danger",
      title: t("mediaBin.import.failedTitle", { name: baseName(failure.path) }),
      message: describeImportFailure(failure, t),
    });
  }
  if (added > 0 || duplicates > 0) {
    const parts: string[] = [];
    if (added > 0) {
      parts.push(
        added === 1
          ? t("mediaBin.import.addedOne")
          : t("mediaBin.import.addedMany", { count: String(added) }),
      );
    }
    if (duplicates > 0) {
      parts.push(
        duplicates === 1
          ? t("mediaBin.import.duplicateOne")
          : t("mediaBin.import.duplicateMany", { count: String(duplicates) }),
      );
    }
    toasts
      .getState()
      .push({ tone: "success", title: t("mediaBin.import.doneTitle"), message: parts.join(" ") });
  }
  return { added, duplicates, failed };
}
