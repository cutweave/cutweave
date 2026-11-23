// SPDX-License-Identifier: GPL-3.0-or-later
// `useImport()` (WP-1M.03 interface): the import actions of the media panel as a hook. The picker
// and the OS drop both end in `importMedia` (one `media.import`).
import { useCallback, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { engineStore } from "../engine/engineStore.ts";
import { toastStore } from "../state/toastStore.ts";
import { translator } from "../timeline/errors.ts";
import { mediaFilesBridge, pickedPaths } from "./bridge.ts";
import { type ImportSummary, importMedia } from "./import.ts";

export interface UseImport {
  /** An import is under way (the engine probes each file before it answers). */
  readonly busy: boolean;
  /** Whether the native picker exists (the desktop app) and an engine is connected. */
  readonly canPick: boolean;
  /** Imports these paths. Resolves to the summary, or `null` when nothing was sent. */
  importPaths(paths: readonly unknown[]): Promise<ImportSummary | null>;
  /** Opens the native file dialog and imports what it returns. */
  pickAndImport(): Promise<ImportSummary | null>;
  /** The paths of files dropped from the OS (`webUtils.getPathForFile` in the preload). */
  pathsOf(files: readonly File[]): string[] | null;
}

export function useImport(): UseImport {
  const { t } = useTranslation();
  const [busy, setBusy] = useState(false);
  const translate = useRef(translator(t));
  translate.current = translator(t);
  const bridge = useMemo(() => mediaFilesBridge(), []);

  const importPaths = useCallback(async (paths: readonly unknown[]) => {
    setBusy(true);
    try {
      return await importMedia(
        {
          engine: engineStore,
          toasts: toastStore,
          t: (key, vars) => translate.current(key, vars),
        },
        paths,
      );
    } finally {
      setBusy(false);
    }
  }, []);

  const pickAndImport = useCallback(async () => {
    if (bridge.pickMediaFiles === undefined) {
      toastStore
        .getState()
        .push({ tone: "info", title: translate.current("mediaBin.import.desktopOnly") });
      return null;
    }
    const answer = await bridge.pickMediaFiles().catch(() => null);
    const paths = pickedPaths(answer);
    return paths.length === 0 ? null : importPaths(paths);
  }, [bridge, importPaths]);

  const pathsOf = useCallback(
    (files: readonly File[]) =>
      bridge.pathsForDrop === undefined ? null : bridge.pathsForDrop(files),
    [bridge],
  );

  return {
    busy,
    canPick: bridge.pickMediaFiles !== undefined,
    importPaths,
    pickAndImport,
    pathsOf,
  };
}
