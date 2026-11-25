// SPDX-License-Identifier: GPL-3.0-or-later
// "Remove from project" as the bin and its context menu both ask for it: an asset no clip uses is
// removed at once; one that clips use asks first (the dialog in `menus/MenuDialogs.tsx`), because
// the engine refuses while clips still play it.
import type { MediaId } from "@cutweave/protocol";
import { engineStore } from "../engine/engineStore.ts";
import { i18n } from "../i18n/index.ts";
import { menuDialogStore } from "../menus/dialogs.ts";
import { docStore } from "../state/docStore.ts";
import { toastStore } from "../state/toastStore.ts";
import { translator } from "../timeline/errors.ts";
import { selectMediaCards, selectMediaUsage } from "./cards.ts";
import { removeMedia } from "./remove.ts";

/** Removes `id`, or asks first when clips use it. Resolves once it is removed or the ask is open. */
export async function requestMediaRemoval(id: MediaId): Promise<void> {
  const project = docStore.getState().project;
  const card = selectMediaCards(project).find((c) => c.id === id);
  if (card === undefined) return;
  const uses = selectMediaUsage(project).get(id) ?? 0;
  if (uses > 0) {
    menuDialogStore.getState().open({ kind: "remove-media", id, name: card.name, uses });
    return;
  }
  await removeMedia(
    { engine: engineStore, toasts: toastStore, t: translator(i18n.t.bind(i18n)) },
    id,
    card.name,
  );
}
