// SPDX-License-Identifier: GPL-3.0-or-later
// What the inspector inspects: the selected clips (with their sequence's frame rate) or one
// selected media asset. Selection is by ID (`viewStore.selection`); IDs that no longer exist in
// the document are dropped.

import type { MediaAsset, Project, SelectionRef } from "@cutweave/protocol";
import type { SelectedClip } from "./fields.ts";

/** The selected timeline clips, in selection order. */
export function selectSelectedClips(
  project: Project | null,
  selection: readonly SelectionRef[],
): SelectedClip[] {
  if (project === null) return [];
  const wanted = new Set<string>(selection.filter((r) => r.kind === "item").map((r) => r.id));
  if (wanted.size === 0) return [];
  const found = new Map<string, SelectedClip>();
  for (const sequence of Object.values(project.sequences)) {
    const fps = {
      num: sequence.format.frame_rate.num,
      den: sequence.format.frame_rate.den,
    };
    for (const track of Object.values(sequence.tracks)) {
      for (const item of Object.values(track.items)) {
        if (wanted.has(item.id)) {
          found.set(item.id, {
            item,
            trackId: track.id,
            sequence: sequence.id,
            fps,
            locked: track.locked,
          });
        }
      }
    }
  }
  const ordered: SelectedClip[] = [];
  for (const ref of selection) {
    const clip = ref.kind === "item" ? found.get(ref.id) : undefined;
    if (clip !== undefined) ordered.push(clip);
  }
  return ordered;
}

/** The media asset the selection names, when it names exactly one and no clip. */
export function selectSelectedMedia(
  project: Project | null,
  selection: readonly SelectionRef[],
): MediaAsset | null {
  if (project === null || selection.some((r) => r.kind === "item")) return null;
  const media = selection.filter((r) => r.kind === "media");
  const only = media.length === 1 ? media[0] : undefined;
  return only === undefined ? null : (project.media[only.id as keyof typeof project.media] ?? null);
}
