// SPDX-License-Identifier: GPL-3.0-or-later
// How the three selectable kinds are read and set for the target rules (`target.ts`): timeline
// items and tracks (a right-click on an empty lane keeps them) and bin cards (an empty bin
// clears them). Selection lives in `viewStore` and syncs to the engine as `view.set`: it is view
// state, never an undo step.
import type { FolderId, SelectionRef } from "@cutweave/protocol";
import { folderUiStore } from "../media/folderUi.ts";
import { viewStore } from "../state/viewStore.ts";
import type { SelectionAdapter } from "./types.ts";

type Kind = SelectionRef["kind"];

function adapter(kind: Kind, empty: SelectionAdapter["empty"]): SelectionAdapter {
  return {
    empty,
    current: () =>
      viewStore
        .getState()
        .selection.filter((ref) => ref.kind === kind)
        .map((ref) => ref.id as string),
    set: (ids) => {
      const view = viewStore.getState();
      // Replace this kind; refs of other kinds go with it (one selection at a time, as a click does).
      view.setSelection(ids.map((id) => ({ kind, id }) as SelectionRef));
    },
  };
}

export const itemSelection: SelectionAdapter = adapter("item", "keep");
export const trackSelection: SelectionAdapter = adapter("track", "keep");
const media = adapter("media", "clear");

/** Bin cards: selecting them leaves no folder selected (the bin has one selection, WP-2F.08). */
export const mediaSelection: SelectionAdapter = {
  ...media,
  set: (ids) => {
    media.set(ids);
    if (folderUiStore.getState().folders.length > 0) folderUiStore.getState().setFolders([]);
  },
};

/** Bin folder tiles: folders are selected in the bin's own store, and the assets are let go. */
export const folderSelection: SelectionAdapter = {
  empty: "clear",
  current: () => folderUiStore.getState().folders,
  set: (ids) => {
    folderUiStore.getState().setFolders(ids as FolderId[]);
    const view = viewStore.getState();
    if (view.selection.some((ref) => ref.kind === "media")) {
      view.setSelection(view.selection.filter((ref) => ref.kind !== "media"));
    }
  },
};
