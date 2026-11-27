// SPDX-License-Identifier: GPL-3.0-or-later
// "Duration…" on an image card (#215; ADR-0035, Decision 1): the length the next insert of that
// picture uses. It is UI state of this run of the app, not a model field and not a command: an
// insert of the card then sends an explicit `duration` with its `timeline.insert`. Without a
// choice the engine gives a still the project's still duration.
import type { Project } from "@cutweave/protocol";
import { FLICKS_PER_SECOND } from "@cutweave/timeline";
import { createStore } from "zustand/vanilla";
import { parseSeconds } from "./slideshow.ts";

export interface InsertDurationStore {
  /** The chosen length of each picture, in flicks, by media id. */
  readonly byMedia: ReadonlyMap<string, number>;
  /** Sets the length the next inserts of `media` use; `null` goes back to the project's. */
  set(media: string, flicks: number | null): void;
  /** Forgets every choice. */
  reset(): void;
}

export const insertDurationStore = createStore<InsertDurationStore>()((set) => ({
  byMedia: new Map(),
  set: (media, flicks) =>
    set((state) => {
      const byMedia = new Map(state.byMedia);
      if (flicks === null) byMedia.delete(media);
      else byMedia.set(media, flicks);
      return { byMedia };
    }),
  reset: () => set({ byMedia: new Map() }),
}));

/**
 * The length (flicks) an insert of `media` sends, or `undefined` to leave it to the engine: only
 * a still picture of `project` that has a choice.
 */
export function insertDurationFor(project: Project | null, media: string): number | undefined {
  const asset = project?.media[media as keyof Project["media"]];
  if (asset?.kind !== "image") return undefined;
  return insertDurationStore.getState().byMedia.get(media);
}

/** The seconds a person typed, as flicks; `null` when it is not a usable length. */
export function secondsToFlicks(text: string): number | null {
  const seconds = parseSeconds(text);
  return seconds === null ? null : Math.round(seconds * FLICKS_PER_SECOND);
}

/** Flicks as the seconds the field shows ("2.5"). */
export function flicksToSeconds(flicks: number): string {
  return String(Math.round((flicks / FLICKS_PER_SECOND) * 1000) / 1000);
}
