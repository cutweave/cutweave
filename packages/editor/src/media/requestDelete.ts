// SPDX-License-Identifier: GPL-3.0-or-later
// "Delete" in the bin (WP-2F.08): the selected assets are removed from the project in one undo
// step (clips that use one refuse it, with the engine's message, as for a single asset); each
// selected folder deletes at once when it is empty and otherwise asks what to do with its contents
// (the dialog in `menus/MenuDialogs.tsx`).
import type { FolderId, MediaId } from "@cutweave/protocol";
import { engineStore } from "../engine/engineStore.ts";
import { i18n } from "../i18n/index.ts";
import { menuDialogStore } from "../menus/dialogs.ts";
import { docStore } from "../state/docStore.ts";
import { toastStore } from "../state/toastStore.ts";
import { translator } from "../timeline/errors.ts";
import { type BinDeps, deleteFolder, removeManyMedia } from "./binActions.ts";
import { selectMediaCards, selectMediaUsage } from "./cards.ts";
import { planFolderDelete } from "./folders.ts";
import { selectedTiles } from "./folderUi.ts";
import { requestMediaRemoval } from "./requestRemoval.ts";

export const binDeps = (): BinDeps => ({
  engine: engineStore,
  toasts: toastStore,
  t: translator(i18n.t.bind(i18n)),
});

/** Deletes `folder` now when it is empty, else opens the question; `rest` are asked about after. */
export async function requestFolderDelete(
  folder: FolderId,
  rest: readonly string[] = [],
): Promise<void> {
  const project = docStore.getState().project;
  const cards = selectMediaCards(project);
  const plan = planFolderDelete(project, cards, selectMediaUsage(project), folder);
  if (plan === null) {
    const next = rest[0];
    if (next !== undefined) await requestFolderDelete(next as FolderId, rest.slice(1));
    return;
  }
  if (!plan.empty) {
    menuDialogStore.getState().open({ kind: "delete-folder", folder, rest });
    return;
  }
  await deleteFolder(binDeps(), folder, plan.name, "move_up");
  const next = rest[0];
  if (next !== undefined) await requestFolderDelete(next as FolderId, rest.slice(1));
}

/** Removes `media` from the project: one asset asks when clips use it; several go as one step. */
export async function requestMediaDelete(media: readonly MediaId[]): Promise<void> {
  const project = docStore.getState().project;
  const cards = selectMediaCards(project);
  const usage = selectMediaUsage(project);
  if (media.length === 1) {
    await requestMediaRemoval(media[0] as MediaId);
    return;
  }
  const items = media.flatMap((id) => {
    const card = cards.find((c) => c.id === id);
    return card === undefined ? [] : [{ id, name: card.name, used: (usage.get(id) ?? 0) > 0 }];
  });
  await removeManyMedia(binDeps(), items);
}

/** Delete on the bin's selection: the assets first, then each folder. */
export async function requestDeleteSelection(): Promise<void> {
  const { media, folders } = selectedTiles();
  if (media.length > 0) await requestMediaDelete(media);
  const first = folders[0];
  if (first !== undefined) await requestFolderDelete(first, folders.slice(1));
}
