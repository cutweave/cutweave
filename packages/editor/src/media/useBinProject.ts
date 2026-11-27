// SPDX-License-Identifier: GPL-3.0-or-later
import type { Project } from "@cutweave/protocol";
import { useMemo } from "react";
import { useDocStore } from "../state/docStore.ts";
import { selectMediaUsage } from "./cards.ts";

/**
 * The part of the project the bin shows: its media, folders and collections, as a `Project` that
 * keeps its identity until one of those changes. A clip dragged on the timeline changes the
 * document on every update (issue #244), and the bin must not draw again for it. It carries no
 * sequences: the usage counts come from `useMediaUsage`, and a bin function that reads
 * `sequences` from it fails loudly rather than reading a stale copy.
 */
export function useBinProject(): Project | null {
  const present = useDocStore((s) => s.project !== null);
  const media = useDocStore((s) => s.project?.media);
  const folders = useDocStore((s) => s.project?.folders);
  const collections = useDocStore((s) => s.project?.collections);
  return useMemo(
    () => (present ? ({ media, folders, collections } as unknown as Project) : null),
    [present, media, folders, collections],
  );
}

/** How many clips use each asset; the same map until the counts change. */
export function useMediaUsage(): ReadonlyMap<string, number> {
  return useDocStore((s) => selectMediaUsage(s.project));
}
