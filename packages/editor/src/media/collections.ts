// SPDX-License-Identifier: GPL-3.0-or-later
// The bin's collections as the editor reads them (WP-2F.06c, ADR-0036): the list the sidebar
// shows, what an open collection holds and in which order, the name a new one gets, and the filter
// "Save as smart collection" stores. Read from the document mirror, so the sidebar needs no
// `media.list_collections` round trip; every change is one `media.*` command
// (`collectionActions.ts`). A manual collection keeps its members in the order they were added; a
// member that names no asset is skipped (the engine drops it on the collection's next edit).
import type { CollectionId, FolderId, MediaId, MediaSort, Project } from "@cutweave/protocol";
import { type BinState, toMediaFilter } from "./binStore.ts";
import type { MediaCard } from "./cards.ts";
import { compareCodePoints, findMedia, isEmptyFilter, type MediaFilter } from "./find.ts";

/** Most members of a manual collection, and most assets one command takes (the engine's limit). */
export const MAX_COLLECTION_MEMBERS = 10_000;
/** Longest collection name, in characters. */
export const MAX_COLLECTION_NAME_CHARS = 255;

/** A collection as the sidebar lists it: what `media.list_collections` answers. */
export interface CollectionEntry {
  readonly id: CollectionId;
  readonly name: string;
  readonly kind: "manual" | "smart";
  /** The stored filter of a smart collection; `null` for a manual one. */
  readonly filter: MediaFilter | null;
  /** How many assets are on it now. */
  readonly count: number;
}

/** The form collection names are compared in (the engine's: lower case). */
export const collectionNameKey = (name: string): string => name.toLowerCase();

const byName = (a: CollectionEntry, b: CollectionEntry): number =>
  compareCodePoints(collectionNameKey(a.name), collectionNameKey(b.name)) ||
  compareCodePoints(a.name, b.name) ||
  compareCodePoints(a.id, b.id);

/** The stored members of a manual collection that still name an asset, in stored order. */
function manualMembers(
  media: readonly string[] | undefined,
  known: ReadonlySet<string>,
): MediaId[] {
  const seen = new Set<string>();
  const out: MediaId[] = [];
  for (const id of media ?? []) {
    if (!known.has(id) || seen.has(id)) continue;
    seen.add(id);
    out.push(id as MediaId);
  }
  return out;
}

/**
 * The project's collections in name order, each with its member count (a smart collection's
 * filter is run). `cards` and `usage` are the project's (`selectMediaCards`, `selectMediaUsage`).
 */
export function selectCollections(
  project: Project | null,
  cards: readonly MediaCard[],
  usage: ReadonlyMap<string, number>,
): CollectionEntry[] {
  const collections = project?.collections ?? {};
  const known = new Set<string>(cards.map((c) => c.id));
  const out: CollectionEntry[] = [];
  for (const key of Object.keys(collections)) {
    const collection = collections[key as CollectionId];
    if (collection === undefined) continue;
    const kind = collection.kind;
    if (kind.type === "manual") {
      out.push({
        id: collection.id,
        name: collection.name,
        kind: "manual",
        filter: null,
        count: manualMembers(kind.media, known).length,
      });
    } else {
      const found = findMedia(
        cards,
        usage,
        { collection: collection.id },
        null,
        project?.folders,
        collections,
      );
      out.push({
        id: collection.id,
        name: collection.name,
        kind: "smart",
        filter: kind.filter,
        count: found.cards.length,
      });
    }
  }
  return out.sort(byName);
}

/** The open collection if it still exists, else none (it was deleted, or another project opened). */
export function resolveOpenCollection(
  project: Project | null,
  open: CollectionId | null,
): CollectionId | null {
  if (open === null || project === null) return null;
  return project.collections?.[open] === undefined ? null : open;
}

/** Whether `collection` is a manual collection of the project. */
export function isManualCollection(project: Project | null, collection: string | null): boolean {
  if (collection === null) return false;
  return project?.collections?.[collection as CollectionId]?.kind.type === "manual";
}

/**
 * The cards the bin shows for an open collection: its members that also pass `filter` (the bin's
 * search and filters). A manual collection shows them in its stored order, whatever the sort; a
 * smart one in the requested order.
 */
export function collectionCards(
  project: Project | null,
  cards: readonly MediaCard[],
  usage: ReadonlyMap<string, number>,
  collection: CollectionId,
  filter: MediaFilter,
  sort: MediaSort | null,
): readonly MediaCard[] {
  const stored = project?.collections?.[collection];
  if (project === null || stored === undefined) return [];
  const found = findMedia(
    cards,
    usage,
    { ...filter, collection, folder: null, recursive: null },
    sort,
    project.folders,
    project.collections,
  ).cards;
  if (stored.kind.type !== "manual") return found;
  const byId = new Map(found.map((card) => [card.id as string, card]));
  const out: MediaCard[] = [];
  for (const id of manualMembers(stored.kind.media, new Set(byId.keys()))) {
    const card = byId.get(id);
    if (card !== undefined) out.push(card);
  }
  return out;
}

/**
 * A name no collection has yet: `base`, then "`base` 2", "`base` 3"... (names clash ignoring
 * case). The engine names nothing by itself, so the editor proposes one.
 */
export function freeCollectionName(project: Project | null, base: string): string {
  const taken = new Set(
    Object.values(project?.collections ?? {}).map((c) => collectionNameKey(c?.name ?? "")),
  );
  if (!taken.has(collectionNameKey(base))) return base;
  for (let n = 2; ; n++) {
    const name = `${base} ${n}`;
    if (!taken.has(collectionNameKey(name))) return name;
  }
}

/** Why a typed name cannot be a (new) collection's name. */
export type CollectionNameProblem = "empty" | "tooLong" | "control" | "taken";

/** Why `name` cannot be stored, or `null`; `except` is the collection being renamed. */
export function collectionNameProblem(
  project: Project | null,
  name: string,
  except: string | null = null,
): CollectionNameProblem | null {
  const text = name.trim();
  if (text === "") return "empty";
  if ([...text].length > MAX_COLLECTION_NAME_CHARS) return "tooLong";
  if (/\p{Cc}/u.test(text)) return "control";
  const key = collectionNameKey(text);
  const clash = Object.values(project?.collections ?? {}).some(
    (c) => c !== undefined && c.id !== except && collectionNameKey(c.name) === key,
  );
  return clash ? "taken" : null;
}

/**
 * The filter "Save as smart collection" stores for the bin as it is: the search text and the
 * filters, and the open folder with its subfolders when the search is scoped to "This folder".
 * `null` when no search or filter is on: a collection of everything is not a saved search.
 */
export function smartFilterOf(
  bin: Pick<BinState, "text" | "filters" | "scope">,
  open: FolderId | null,
): MediaFilter | null {
  const filter = toMediaFilter(bin);
  if (isEmptyFilter(filter)) return null;
  if (bin.scope === "folder" && open !== null) {
    filter.folder = open;
    filter.recursive = true;
  }
  return filter;
}

/** The assets of `media` that a manual collection does not hold yet (what adding would add). */
export function notYetOn(
  project: Project | null,
  collection: string,
  media: readonly string[],
): MediaId[] {
  const kind = project?.collections?.[collection as CollectionId]?.kind;
  if (project === null || kind === undefined || kind.type !== "manual") return [];
  const have = new Set<string>(kind.media ?? []);
  return media.filter(
    (id) => !have.has(id) && project.media[id as MediaId] !== undefined,
  ) as MediaId[];
}
