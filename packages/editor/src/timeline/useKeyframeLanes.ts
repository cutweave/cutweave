// SPDX-License-Identifier: GPL-3.0-or-later
// The keyframe lanes the timeline canvas draws (WP-2C.05): the clips whose lanes are open
// (`viewStore.keyframeLanes`), their animated properties from the document, and the selected
// keys. The same array comes back while none of them changed, so the canvas rebuilds nothing.
import type { Project } from "@cutweave/protocol";
import type { ItemLanes } from "@cutweave/timeline";
import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { useStore } from "zustand";
import { PROP_LABELS } from "../inspector/labels.ts";
import { selectActiveSequenceId } from "../state/selectors.ts";
import { useViewStore } from "../state/viewStore.ts";
import { EMPTY_KEY_SELECTION, keySelectionStore, laneGroups, NO_GROUPS } from "./keyframes.ts";

export function useKeyframeLanes(project: Project | null): readonly ItemLanes[] {
  const { t } = useTranslation();
  const open = useViewStore((s) => s.keyframeLanes);
  const selection = useStore(keySelectionStore);
  // With no lane open neither the document nor the key selection is a dependency: a patch (every
  // update of a drag) recomputes nothing here.
  const none = open.length === 0;
  const doc = none ? null : project;
  const keys = none ? EMPTY_KEY_SELECTION : selection;
  return useMemo(
    () =>
      doc === null
        ? NO_GROUPS
        : laneGroups(doc, selectActiveSequenceId(doc), open, keys, (prop) => {
            const key = PROP_LABELS[prop];
            return key === undefined ? prop : t(key);
          }),
    [doc, open, keys, t],
  );
}
