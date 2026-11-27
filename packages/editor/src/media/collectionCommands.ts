// SPDX-License-Identifier: GPL-3.0-or-later
// The collection actions more than one place starts (WP-2F.06c): the sidebar, the card and bin
// menus, the command palette and a drop. They live here, not in the menu tables, so the panel can
// call them without importing the menus. Each document change is one catalog command
// (`collectionActions.ts`); what is asked first (a name, a yes) is a dialog of `menus/dialogs.ts`.
import type { CollectionId, MediaId } from "@cutweave/protocol";
import type { TKey } from "../i18n/index.ts";
import { menuDialogStore } from "../menus/dialogs.ts";
import { docStore } from "../state/docStore.ts";
import { binStore } from "./binStore.ts";
import { selectMediaCards, selectMediaUsage } from "./cards.ts";
import {
  addToCollection,
  createCollection,
  deleteCollection,
  setCollectionFilter,
} from "./collectionActions.ts";
import { freeCollectionName, notYetOn, selectCollections, smartFilterOf } from "./collections.ts";
import type { MediaFilter } from "./find.ts";
import { currentOpenFolder } from "./folderUi.ts";
import { binDeps } from "./requestDelete.ts";
import { currentOpenCollection, sidebarUiStore } from "./sidebar.ts";

/**
 * The filter "Save as smart collection" and "Update from current filters" would store now, or why
 * there is none: no search or filter is on, or a collection is open (a smart collection may not
 * name a collection, ADR-0036).
 */
export function currentSmartFilter():
  | { readonly filter: MediaFilter; readonly reason: null }
  | { readonly filter: null; readonly reason: TKey } {
  if (currentOpenCollection() !== null) {
    return { filter: null, reason: "mediaBin.collections.reasonInCollection" };
  }
  const filter = smartFilterOf(binStore.getState(), currentOpenFolder());
  return filter === null
    ? { filter: null, reason: "mediaBin.collections.reasonNoFilter" }
    : { filter, reason: null };
}

/**
 * "New collection" in the sidebar: creates an empty manual collection under a free name and
 * starts renaming it in place, as New folder does.
 */
export async function newCollectionInPlace(): Promise<CollectionId | null> {
  const deps = binDeps();
  const name = freeCollectionName(
    docStore.getState().project,
    deps.t("mediaBin.collections.defaultName"),
  );
  const id = await createCollection(deps, name, { type: "manual", media: [] });
  if (id !== null) sidebarUiStore.getState().startRename(id);
  return id;
}

/** "New collection…" from a menu or the palette: asks for a name; `media` become its members. */
export function openNewCollectionDialog(media: readonly string[] = []): void {
  menuDialogStore.getState().open({ kind: "new-collection", media: [...media], filter: null });
}

/**
 * "Save as smart collection…": asks for a name for the current search and filters. With nothing
 * to save it says why instead.
 */
export function openSaveSmartCollection(): void {
  const { filter, reason } = currentSmartFilter();
  if (filter === null) {
    const deps = binDeps();
    deps.toasts.getState().push({ tone: "info", title: deps.t(reason) });
    return;
  }
  menuDialogStore.getState().open({ kind: "new-collection", media: [], filter });
}

/** "Add to collection…" from the palette: a list of the manual collections for `media`. */
export function openAddToCollectionDialog(media: readonly string[]): void {
  if (media.length === 0) return;
  menuDialogStore.getState().open({ kind: "add-to-collection", media: [...media] });
}

/** Adds `media` to the manual collection and says how many were new to it. One undo step. */
export async function addMediaToCollection(
  collection: CollectionId,
  media: readonly string[],
): Promise<void> {
  const deps = binDeps();
  const project = docStore.getState().project;
  const name = project?.collections?.[collection]?.name ?? "";
  const fresh = notYetOn(project, collection, media);
  if (fresh.length === 0) {
    deps.toasts
      .getState()
      .push({ tone: "info", title: deps.t("mediaBin.collections.addedNone", { name }) });
    return;
  }
  const added = await addToCollection(deps, collection, fresh as MediaId[]);
  if (added === null) return;
  deps.toasts.getState().push({
    tone: "success",
    title:
      added === 1
        ? deps.t("mediaBin.collections.addedOne", { name })
        : deps.t("mediaBin.collections.addedMany", { name, count: String(added) }),
  });
}

/** "Update from current filters": the smart collection takes the bin's search and filters. */
export async function updateCollectionFromFilters(collection: CollectionId): Promise<void> {
  const deps = binDeps();
  const { filter, reason } = currentSmartFilter();
  if (filter === null) {
    deps.toasts.getState().push({ tone: "info", title: deps.t(reason) });
    return;
  }
  const name = docStore.getState().project?.collections?.[collection]?.name ?? "";
  const members = await setCollectionFilter(deps, collection, filter);
  if (members === null) return;
  deps.toasts
    .getState()
    .push({ tone: "success", title: deps.t("mediaBin.collections.updated", { name }) });
}

/**
 * Deletes a collection: at once when it is empty, after a yes when it has members (the dialog says
 * the media stay in the project).
 */
export async function requestCollectionDelete(collection: CollectionId): Promise<void> {
  const project = docStore.getState().project;
  const entry = selectCollections(
    project,
    selectMediaCards(project),
    selectMediaUsage(project),
  ).find((c) => c.id === collection);
  if (entry === undefined) return;
  if (entry.count > 0) {
    menuDialogStore.getState().open({ kind: "delete-collection", collection });
    return;
  }
  await deleteCollection(binDeps(), collection, entry.name);
}
