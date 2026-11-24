// SPDX-License-Identifier: GPL-3.0-or-later
// What the media bin shows (WP-1M.12): the search text, the filters and the sort. The filters and
// the sort are the panel's view state and come back on the next launch; the text is kept for the
// session and cleared when another project opens (`clearSearch`, called from the engine runtime).
import type { MediaKind, MediaSort } from "@cutweave/protocol";
import { FLICKS_PER_SECOND } from "@cutweave/timeline";
import { useStore } from "zustand";
import { createJSONStorage, persist, type StateStorage } from "zustand/middleware";
import { createStore } from "zustand/vanilla";
import type { MediaFilter } from "./find.ts";

/** The kinds the Filters menu offers (a compound clip is not something to import and search). */
export const BIN_KINDS = ["video", "audio", "image"] as const satisfies readonly MediaKind[];
export type BinKind = (typeof BIN_KINDS)[number];

/** Length ranges of the Filters menu. Bounds are inclusive, so the ranges do not overlap. */
export const DURATION_BUCKETS = {
  under10s: { max: 10 * FLICKS_PER_SECOND - 1 },
  "10sTo1m": { min: 10 * FLICKS_PER_SECOND, max: 60 * FLICKS_PER_SECOND - 1 },
  "1To5m": { min: 60 * FLICKS_PER_SECOND, max: 300 * FLICKS_PER_SECOND - 1 },
  over5m: { min: 300 * FLICKS_PER_SECOND },
} as const satisfies Record<string, { min?: number; max?: number }>;
export type DurationBucket = keyof typeof DURATION_BUCKETS;
export const DURATION_BUCKET_IDS = Object.keys(DURATION_BUCKETS) as DurationBucket[];

export type UseFilter = "used" | "unused";

export interface BinFilters {
  readonly kinds: readonly BinKind[];
  readonly duration: DurationBucket | null;
  readonly use: UseFilter | null;
  readonly problems: boolean;
}

export const NO_FILTERS: BinFilters = { kinds: [], duration: null, use: null, problems: false };

/** Newest import first. */
export const DEFAULT_BIN_SORT: Required<MediaSort> = { by: "imported", order: "desc" };

/** The sorts the menu offers, in order. */
export const SORT_CHOICES: readonly Required<MediaSort>[] = [
  { by: "imported", order: "desc" },
  { by: "imported", order: "asc" },
  { by: "name", order: "asc" },
  { by: "name", order: "desc" },
  { by: "duration", order: "asc" },
  { by: "duration", order: "desc" },
  { by: "kind", order: "asc" },
];

export interface BinState {
  /** The search field; session only. */
  readonly text: string;
  readonly filters: BinFilters;
  readonly sort: Required<MediaSort>;
}

export interface BinActions {
  setText(text: string): void;
  toggleKind(kind: BinKind): void;
  setDuration(duration: DurationBucket | null): void;
  setUse(use: UseFilter | null): void;
  setProblems(on: boolean): void;
  setSort(sort: Required<MediaSort>): void;
  /** Drops the filters, keeps the text. */
  clearFilters(): void;
  /** Drops the text and the filters. */
  clearAll(): void;
  /** Another project opened: forget the search text, keep the view settings. */
  clearSearch(): void;
}

export type BinStore = BinState & BinActions;

/** The engine's `media.list` filter that the bin's state stands for. */
export function toMediaFilter(state: Pick<BinState, "text" | "filters">): MediaFilter {
  const { text, filters } = state;
  const range = filters.duration === null ? null : DURATION_BUCKETS[filters.duration];
  const filter: MediaFilter = {};
  if (text.trim() !== "") filter.text = text;
  if (filters.kinds.length > 0) filter.kinds = [...filters.kinds];
  if (range !== null && "min" in range) filter.min_duration = range.min;
  if (range !== null && "max" in range) filter.max_duration = range.max;
  if (filters.use !== null) filter.used = filters.use === "used";
  if (filters.problems) filter.problems = true;
  return filter;
}

/** Filters that are on (the Filters button's badge); the search text is not one. */
export function activeFilterCount(filters: BinFilters): number {
  return (
    filters.kinds.length +
    (filters.duration === null ? 0 : 1) +
    (filters.use === null ? 0 : 1) +
    (filters.problems ? 1 : 0)
  );
}

export const BIN_STORAGE_KEY = "cutweave.bin";

const isOneOf = <T extends string>(list: readonly T[], value: unknown): value is T =>
  typeof value === "string" && (list as readonly string[]).includes(value);

/** Keeps only well-formed persisted values, so a stale or hand-edited blob cannot break the bin. */
function sanitize(persisted: unknown): Partial<Pick<BinState, "filters" | "sort">> {
  if (typeof persisted !== "object" || persisted === null) return {};
  const p = persisted as Record<string, unknown>;
  const out: { filters?: BinFilters; sort?: Required<MediaSort> } = {};
  if (typeof p.filters === "object" && p.filters !== null) {
    const f = p.filters as Record<string, unknown>;
    const kinds: unknown[] = Array.isArray(f.kinds) ? f.kinds : [];
    out.filters = {
      kinds: BIN_KINDS.filter((k) => kinds.includes(k)),
      duration: isOneOf(DURATION_BUCKET_IDS, f.duration) ? f.duration : null,
      use: isOneOf(["used", "unused"] as const, f.use) ? f.use : null,
      problems: f.problems === true,
    };
  }
  if (typeof p.sort === "object" && p.sort !== null) {
    const s = p.sort as Record<string, unknown>;
    const match = SORT_CHOICES.find((c) => c.by === s.by && c.order === s.order);
    if (match !== undefined) out.sort = match;
  }
  return out;
}

function browserStorage(): StateStorage | undefined {
  try {
    const probe = `${BIN_STORAGE_KEY}.probe`;
    globalThis.localStorage.setItem(probe, "1");
    globalThis.localStorage.removeItem(probe);
    return globalThis.localStorage;
  } catch {
    return undefined;
  }
}

function memoryStorage(): StateStorage {
  const data = new Map<string, string>();
  return {
    getItem: (key) => data.get(key) ?? null,
    setItem: (key, value) => void data.set(key, value),
    removeItem: (key) => void data.delete(key),
  };
}

export const initialBinState: BinState = { text: "", filters: NO_FILTERS, sort: DEFAULT_BIN_SORT };

/** Builds a bin store; pass `storage` to persist the filters and the sort (the app uses localStorage). */
export function createBinStore(storage?: StateStorage) {
  const medium = storage ?? memoryStorage();
  return createStore<BinStore>()(
    persist(
      (set) => ({
        ...initialBinState,
        setText: (text) => set(() => ({ text })),
        toggleKind: (kind) =>
          set((s) => ({
            filters: {
              ...s.filters,
              kinds: s.filters.kinds.includes(kind)
                ? s.filters.kinds.filter((k) => k !== kind)
                : BIN_KINDS.filter((k) => k === kind || s.filters.kinds.includes(k)),
            },
          })),
        setDuration: (duration) => set((s) => ({ filters: { ...s.filters, duration } })),
        setUse: (use) => set((s) => ({ filters: { ...s.filters, use } })),
        setProblems: (problems) => set((s) => ({ filters: { ...s.filters, problems } })),
        setSort: (sort) => set(() => ({ sort })),
        clearFilters: () => set(() => ({ filters: NO_FILTERS })),
        clearAll: () => set(() => ({ text: "", filters: NO_FILTERS })),
        clearSearch: () => set(() => ({ text: "" })),
      }),
      {
        name: BIN_STORAGE_KEY,
        version: 1,
        storage: createJSONStorage(() => medium),
        partialize: (s) => ({ filters: s.filters, sort: s.sort }),
        merge: (persisted, current) => ({ ...current, ...sanitize(persisted) }),
      },
    ),
  );
}

export const binStore = createBinStore(browserStorage());

export function useBinStore<T>(selector: (state: BinStore) => T): T {
  return useStore(binStore, selector);
}
