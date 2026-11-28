// SPDX-License-Identifier: GPL-3.0-or-later
// Which sequence the editor shows (WP-2B.03). The choice lives in `viewStore.activeSequence`
// (the engine keeps it with the project's view and plays that sequence); the timeline, the
// preview and every menu read "the active sequence" from the document mirror through
// `selectActiveSequenceId`. This binds the two: a new choice becomes the selectors' answer, and
// the document is published again (the same contents under a new object) so everything derived
// from it is derived again, exactly as after a patch. Nothing here runs on a pointer move or a
// document patch: only when the choice changes.
import type { SequenceId } from "@cutweave/protocol";
import type { StoreApi } from "zustand/vanilla";
import type { DocStore } from "./docStore.ts";
import { selectActiveSequenceId, setChosenSequence } from "./selectors.ts";
import type { ViewStore } from "./viewStore.ts";

/** Follows `view.activeSequence` until the returned function is called. */
export function bindShownSequence(view: StoreApi<ViewStore>, doc: StoreApi<DocStore>): () => void {
  const show = (sequence: SequenceId | null): void => {
    const project = doc.getState().project;
    const before = selectActiveSequenceId(project);
    setChosenSequence(sequence);
    // A sequence the mirror does not hold yet (its patch is on the way) shows when it arrives.
    if (project !== null && selectActiveSequenceId(project) !== before) {
      doc.getState().republish();
    }
  };
  show(view.getState().activeSequence);
  const off = view.subscribe((state, previous) => {
    if (state.activeSequence !== previous.activeSequence) show(state.activeSequence);
  });
  return () => {
    off();
    setChosenSequence(null);
  };
}
