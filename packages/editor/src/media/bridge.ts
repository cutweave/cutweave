// SPDX-License-Identifier: GPL-3.0-or-later
// The two things the media panel needs from the desktop shell (`window.cutweave`, apps/desktop
// `shared/api.ts`): a native file picker and the filesystem path behind a dropped `File`. Both
// are optional: in plain `vite` dev (no preload) the panel says so instead of importing.
import { validateImportPaths } from "./import.ts";

export interface MediaFilesBridge {
  /** Native file picker (main validates and answers with absolute paths); `null` when cancelled. */
  pickMediaFiles?(): Promise<string[] | null>;
  /** The paths behind dropped `File` objects (`webUtils.getPathForFile`, run in the preload). */
  pathsForDrop?(files: readonly File[]): string[];
}

interface WindowBridge {
  cutweave?: {
    dialog?: { pickMediaFiles?: () => Promise<string[] | null> };
    files?: { pathForFile?: (file: File) => string };
  };
}

/** The bridge of the running window; both members are absent outside the desktop app. */
export function mediaFilesBridge(target: WindowBridge = globalThis as never): MediaFilesBridge {
  const api = target.cutweave;
  const pick = api?.dialog?.pickMediaFiles;
  const pathFor = api?.files?.pathForFile;
  return {
    ...(pick && { pickMediaFiles: () => pick() }),
    ...(pathFor && {
      // One file at a time: the bridge hands each `File` to Electron separately.
      pathsForDrop: (files: readonly File[]) => validateImportPaths(files.map((f) => pathFor(f))),
    }),
  };
}

/** The files of a native picker answer, checked once more on this side of the bridge. */
export function pickedPaths(answer: unknown): string[] {
  return Array.isArray(answer) ? validateImportPaths(answer) : [];
}
