// SPDX-License-Identifier: GPL-3.0-or-later
// What the media bin shows (WP-1M.12): the search text, the filters and the sort. The filters and
// the sort are the panel's view state and come back on the next launch; the text is kept for the
// session and cleared when another project opens (`clearSearch`, called from the engine runtime).
// Tags (WP-2F.06b) are a project's own words, so the tag filter goes with the text: it is not
// saved and another project starts without it. The minimum rating is saved like the other filters.
import type { CollectionId, FolderId, MediaKind, MediaSort } from "@cutweave/protocol";
import { FLICKS_PER_SECOND } from "@cutweave/timeline";
import { useStore } from "zustand";
import { createJSONStorage, persist, type StateStorage } from "zustand/middleware";
import { createStore } from "zustand/vanilla";
import type { MediaFilter } from "./find.ts";
import { MAX_TAGS_PER_CALL, RATINGS, tagKey } from "./tags.ts";

/** The kinds the Filters menu offers (a compound clip is not something to import and search). */
export const BIN_KINDS = [
  "video",
  "audio",
  "image",
  "image_sequence",
] as const satisfies readonly MediaKind[];
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
  /** Tags an asset must all have (ADR-0036); session only. */
  readonly tags: readonly string[];
  /** At least this many stars, 1 to 5; `null`: any rating. */
  readonly minRating: number | null;
}

export const NO_FILTERS: BinFilters = {
  kinds: [],
  duration: null,
  use: null,
  problems: false,
  tags: [],
  minRating: null,
};

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

/** Where a search looks (WP-2F.08): the whole bin, or the open folder with its subfolders. */
export const SEARCH_SCOPES = ["all", "folder"] as const;
export type SearchScope = (typeof SEARCH_SCOPES)[number];

/** How many projects keep their open folder or collection (the oldest are forgotten). */
const MAX_OPEN_FOLDERS = 50;

export interface BinState {
  /** The search field; session only. */
  readonly text: string;
  readonly filters: BinFilters;
  readonly sort: Required<MediaSort>;
  /** All media (default) or This folder. */
  readonly scope: SearchScope;
  /**
   * The folder the bin has open, per project (its file path; an untitled project is ""). A folder
   * that no longer exists reads as the top level (`resolveOpenFolder`).
   */
  readonly openFolders: Readonly<Record<string, FolderId>>;
  /**
   * The collection the bin has open instead of a folder, per project (WP-2F.06c). A project has
   * an open folder or an open collection, never both; a collection that no longer exists reads as
   * none (`resolveOpenCollection`).
   */
  readonly openCollections: Readonly<Record<string, CollectionId>>;
  /** Whether the bin's sidebar (folders and collections) is shown: editor state (ADR-0036). */
  readonly sidebar: boolean;
}

export interface BinActions {
  setText(text: string): void;
  toggleKind(kind: BinKind): void;
  setDuration(duration: DurationBucket | null): void;
  setUse(use: UseFilter | null): void;
  setProblems(on: boolean): void;
  /** Adds the tag to the filter, or takes it out (compared ignoring case and accents). */
  toggleTag(tag: string): void;
  setMinRating(stars: number | null): void;
  setSort(sort: Required<MediaSort>): void;
  setScope(scope: SearchScope): void;
  /** Opens `folder` (`null`: the top level) for the project at `project`. */
  openFolder(project: string, folder: FolderId | null): void;
  /** Opens `collection` for the project at `project` (it leaves the folder); `null` closes it. */
  openCollection(project: string, collection: CollectionId | null): void;
  setSidebar(shown: boolean): void;
  /** Drops the filters, keeps the text. */
  clearFilters(): void;
  /** Drops the text and the filters. */
  clearAll(): void;
  /** Another project opened: forget the search text and the tag filter, keep the view settings. */
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
  if (filters.tags.length > 0) filter.tags = [...filters.tags];
  if (filters.minRating !== null) filter.min_rating = filters.minRating;
  return filter;
}

/** Filters that are on (the Filters button's badge); the search text is not one. */
export function activeFilterCount(filters: BinFilters): number {
  return (
    filters.kinds.length +
    (filters.duration === null ? 0 : 1) +
    (filters.use === null ? 0 : 1) +
    (filters.problems ? 1 : 0) +
    filters.tags.length +
    (filters.minRating === null ? 0 : 1)
  );
}

export const BIN_STORAGE_KEY = "cutweave.bin";

const isOneOf = <T extends string>(list: readonly T[], value: unknown): value is T =>
  typeof value === "string" && (list as readonly string[]).includes(value);

/** Keeps only well-formed persisted values, so a stale or hand-edited blob cannot break the bin. */
function sanitize(
  persisted: unknown,
): Partial<
  Pick<BinState, "filters" | "sort" | "scope" | "openFolders" | "openCollections" | "sidebar">
> {
  if (typeof persisted !== "object" || persisted === null) return {};
  const p = persisted as Record<string, unknown>;
  const out: {
    filters?: BinFilters;
    sort?: Required<MediaSort>;
    scope?: SearchScope;
    openFolders?: Record<string, FolderId>;
    openCollections?: Record<string, CollectionId>;
    sidebar?: boolean;
  } = {};
  if (isOneOf(SEARCH_SCOPES, p.scope)) out.scope = p.scope;
  if (typeof p.sidebar === "boolean") out.sidebar = p.sidebar;
  if (typeof p.openCollections === "object" && p.openCollections !== null) {
    out.openCollections = Object.fromEntries(
      Object.entries(p.openCollections).filter(
        (entry): entry is [string, CollectionId] => typeof entry[1] === "string",
      ),
    );
  }
  if (typeof p.openFolders === "object" && p.openFolders !== null) {
    out.openFolders = Object.fromEntries(
      Object.entries(p.openFolders).filter(
        (entry): entry is [string, FolderId] => typeof entry[1] === "string",
      ),
    );
  }
  if (typeof p.filters === "object" && p.filters !== null) {
    const f = p.filters as Record<string, unknown>;
    const kinds: unknown[] = Array.isArray(f.kinds) ? f.kinds : [];
    out.filters = {
      kinds: BIN_KINDS.filter((k) => kinds.includes(k)),
      duration: isOneOf(DURATION_BUCKET_IDS, f.duration) ? f.duration : null,
      use: isOneOf(["used", "unused"] as const, f.use) ? f.use : null,
      problems: f.problems === true,
      tags: [],
      minRating: (RATINGS as readonly unknown[]).includes(f.minRating)
        ? (f.minRating as number)
        : null,
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

export const initialBinState: BinState = {
  text: "",
  filters: NO_FILTERS,
  sort: DEFAULT_BIN_SORT,
  scope: "all",
  openFolders: {},
  openCollections: {},
  sidebar: false,
};

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
        toggleTag: (tag) =>
          set((s) => {
            const key = tagKey(tag);
            const without = s.filters.tags.filter((have) => tagKey(have) !== key);
            const tags =
              without.length < s.filters.tags.length
                ? without
                : // The engine's filter takes 16 tags; one more replaces nothing.
                  s.filters.tags.length >= MAX_TAGS_PER_CALL
                  ? s.filters.tags
                  : [...s.filters.tags, tag];
            return { filters: { ...s.filters, tags } };
          }),
        setMinRating: (minRating) => set((s) => ({ filters: { ...s.filters, minRating } })),
        setSort: (sort) => set(() => ({ sort })),
        setScope: (scope) => set(() => ({ scope })),
        openFolder: (project, folder) =>
          set((s) => {
            const { [project]: _gone, ...rest } = s.openFolders;
            // A folder (or the top level) is the place now: the collection closes.
            const { [project]: _closed, ...openCollections } = s.openCollections;
            if (folder === null) return { openFolders: rest, openCollections };
            // Newest last; the oldest projects fall off.
            const kept = Object.entries(rest).slice(-(MAX_OPEN_FOLDERS - 1));
            return {
              openFolders: { ...Object.fromEntries(kept), [project]: folder },
              openCollections,
            };
          }),
        openCollection: (project, collection) =>
          set((s) => {
            const { [project]: _gone, ...rest } = s.openCollections;
            if (collection === null) return { openCollections: rest };
            const { [project]: _left, ...openFolders } = s.openFolders;
            const kept = Object.entries(rest).slice(-(MAX_OPEN_FOLDERS - 1));
            return {
              openCollections: { ...Object.fromEntries(kept), [project]: collection },
              openFolders,
            };
          }),
        setSidebar: (sidebar) => set(() => ({ sidebar })),
        clearFilters: () => set(() => ({ filters: NO_FILTERS })),
        clearAll: () => set(() => ({ text: "", filters: NO_FILTERS })),
        clearSearch: () => set((s) => ({ text: "", filters: { ...s.filters, tags: [] } })),
      }),
      {
        name: BIN_STORAGE_KEY,
        version: 1,
        storage: createJSONStorage(() => medium),
        partialize: (s) => ({
          filters: { ...s.filters, tags: [] },
          sort: s.sort,
          scope: s.scope,
          openFolders: s.openFolders,
          openCollections: s.openCollections,
          sidebar: s.sidebar,
        }),
        merge: (persisted, current) => ({ ...current, ...sanitize(persisted) }),
      },
    ),
  );
}

export const binStore = createBinStore(browserStorage());

export function useBinStore<T>(selector: (state: BinStore) => T): T {
  return useStore(binStore, selector);
}
