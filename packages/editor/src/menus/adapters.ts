// SPDX-License-Identifier: GPL-3.0-or-later
// How the three selectable kinds are read and set for the target rules (`target.ts`): timeline
// items and tracks (a right-click on an empty lane keeps them) and bin cards (an empty bin
// clears them). Selection lives in `viewStore` and syncs to the engine as `view.set`: it is view
// state, never an undo step.
import type { SelectionRef } from "@cutweave/protocol";
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
export const mediaSelection: SelectionAdapter = adapter("media", "clear");
